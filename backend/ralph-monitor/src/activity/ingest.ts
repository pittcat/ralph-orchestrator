/**
 * Backend-activity ingest (Plan3-U2).
 *
 * Byte-cursor reader over a run's `.ralph/runs/<run_id>.activity.jsonl`
 * ledger — the same incremental contract as events/ingest.ts: partial
 * trailing lines wait for the next pass, file shrink/rewrite bumps the
 * cursor generation, and `(file, byte_offset, generation)` dedupes
 * replays so ingest is idempotent.
 *
 * `kind` is currently always `backend_call`; `detail` carries the raw
 * record minus ts/seq (exit_code, timed_out, is_error, duration_ms,
 * stderr_tail_hint for `end` rows) so health classification needs no
 * schema knowledge of future fields.
 */

import fs from 'node:fs';
import path from 'node:path';
import type { MonitorDb } from '../db/index.js';

export interface ActivityIngestStats {
  inserted: number;
  parseErrors: number;
  sourceResets: number;
}

interface ActivityLine {
  ts?: string;
  seq_in_run?: number;
  hat?: string;
  backend?: string;
  phase?: string;
  [key: string]: unknown;
}

/**
 * Resolve the ledger path for a run: the snapshot's `activity_file`
 * wins when present; otherwise the conventional location is probed so
 * older snapshots (no field) still pick up the file.
 */
export function activityFileFor(
  workspace: string,
  runId: string,
  activityFile?: string | null,
): string | null {
  if (activityFile) {
    const abs = path.isAbsolute(activityFile)
      ? activityFile
      : path.join(workspace, activityFile);
    if (fs.existsSync(abs)) return abs;
  }
  const derived = path.join(
    workspace,
    '.ralph',
    'runs',
    `${runId}.activity.jsonl`,
  );
  return fs.existsSync(derived) ? derived : null;
}

/**
 * Ingest one run's activity ledger inside the caller's transaction.
 * Returns silently when the run has no ledger (old runs, runs that
 * never reached a backend call) — absence is `backend.unknown` at
 * classification time, not an ingest error (plan §15).
 */
export function ingestRunActivity(
  db: MonitorDb,
  projectId: string,
  runId: string,
  workspace: string,
  activityFile: string | null | undefined,
): ActivityIngestStats {
  const stats: ActivityIngestStats = {
    inserted: 0,
    parseErrors: 0,
    sourceResets: 0,
  };
  const file = activityFileFor(workspace, runId, activityFile);
  if (!file) return stats;

  let size: number;
  try {
    size = fs.statSync(file).size;
  } catch {
    return stats;
  }

  const cursorRow = db
    .prepare(
      'SELECT byte_offset, generation FROM activity_cursors WHERE project_id = ? AND run_id = ? AND file = ?',
    )
    .get(projectId, runId, file) as
    | { byte_offset: number; generation: number }
    | undefined;
  let cursor = cursorRow?.byte_offset ?? 0;
  let generation = cursorRow?.generation ?? 0;
  if (size < cursor) {
    cursor = 0;
    generation += 1;
    stats.sourceResets += 1;
  }
  if (size === cursor) return stats;

  const fd = fs.openSync(file, 'r');
  try {
    const chunk = Buffer.alloc(Math.min(size - cursor, 8 * 1024 * 1024));
    const read = fs.readSync(fd, chunk, 0, chunk.length, cursor);
    const data = chunk.subarray(0, read);
    const lastNl = data.lastIndexOf(0x0a);
    if (lastNl === -1) return stats;
    const consumed = data.subarray(0, lastNl + 1);
    const lines = consumed.toString('utf8').split('\n');
    lines.pop();

    const insert = db.prepare(
      `INSERT OR IGNORE INTO run_activity
         (project_id, run_id, seq, file, byte_offset, generation, phase, kind, at, detail)
       VALUES (?, ?, ?, ?, ?, ?, ?, 'backend_call', ?, ?)`,
    );

    let offset = cursor;
    for (const line of lines) {
      const lineStart = offset;
      offset += Buffer.byteLength(line, 'utf8') + 1;
      if (!line.trim()) continue;
      let rec: ActivityLine;
      try {
        rec = JSON.parse(line) as ActivityLine;
      } catch {
        stats.parseErrors += 1;
        continue;
      }
      if (typeof rec !== 'object' || rec === null) {
        stats.parseErrors += 1;
        continue;
      }
      const phase = rec.phase === 'start' || rec.phase === 'end'
        ? rec.phase
        : null;
      if (phase === null || typeof rec.seq_in_run !== 'number') {
        stats.parseErrors += 1;
        continue;
      }
      const { ts, seq_in_run, phase: _p, ...rest } = rec;
      const res = insert.run(
        projectId,
        runId,
        seq_in_run,
        file,
        lineStart,
        generation,
        phase,
        typeof ts === 'string' ? ts : null,
        Object.keys(rest).length > 0 ? JSON.stringify(rest) : null,
      );
      stats.inserted += res.changes;
    }
    db.prepare(
      `INSERT INTO activity_cursors (project_id, run_id, file, byte_offset, generation)
       VALUES (?, ?, ?, ?, ?)
       ON CONFLICT(project_id, run_id, file) DO UPDATE SET
         byte_offset = excluded.byte_offset,
         generation = excluded.generation`,
    ).run(projectId, runId, file, cursor + consumed.length, generation);
  } finally {
    fs.closeSync(fd);
  }
  return stats;
}
