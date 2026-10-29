/**
 * Run-event ingest (Plan2-U2).
 *
 * Incremental byte-cursor reader over a run's event ledgers:
 * `.ralph/events-*.jsonl` plus the manifest's `events_file`. Each
 * line is loose-parsed (object with optional ts/topic/source|hat/
 * payload/iteration) — malformed lines count into run_evidence
 * without aborting the file. A trailing line without `\n` is treated
 * as in-flight and left for the next pass (cursor stays at its
 * start). seq is per-run monotonic via `MAX(seq)+1`; the
 * (file, byte_offset) unique index dedupes replays so ingest is
 * idempotent.
 */

import fs from 'node:fs';
import path from 'node:path';
import type { MonitorDb } from '../db/index.js';

/// Payloads larger than this keep the raw string but flag
/// `truncated` for the UI (E9: runtime truncates at 50000 chars).
export const TRUNCATE_THRESHOLD = 50_000;

export interface IngestWindow {
  /// `started_at` lower bound (ISO). Events with parseable ts older
  /// than this are attributed to an earlier run and skipped.
  startedAt: string | null;
  /// `terminal.at` upper bound; open-ended when the run is live.
  terminalAt: string | null;
}

export interface IngestStats {
  inserted: number;
  parseErrors: number;
  sourceResets: number;
  files: number;
}

interface EventLine {
  ts?: string;
  topic?: string;
  hat?: string;
  source?: string;
  iteration?: number;
  payload?: unknown;
}

/** Enumerate ledgers for a run: events_file first, then events-*.jsonl by name. */
export function enumerateEventFiles(
  workspace: string,
  eventsFile?: string | null,
): string[] {
  const files: string[] = [];
  if (eventsFile) {
    const abs = path.isAbsolute(eventsFile)
      ? eventsFile
      : path.join(workspace, eventsFile);
    if (fs.existsSync(abs)) files.push(abs);
  }
  const ralphDir = path.join(workspace, '.ralph');
  try {
    for (const name of fs.readdirSync(ralphDir).sort()) {
      if (/^events-.*\.jsonl$/.test(name) || name === 'events.jsonl') {
        const p = path.join(ralphDir, name);
        if (!files.includes(p)) files.push(p);
      }
    }
  } catch {
    /* no .ralph dir → only events_file */
  }
  return files;
}

function parseLine(raw: string): EventLine | null {
  let obj: unknown;
  try {
    obj = JSON.parse(raw);
  } catch {
    return null;
  }
  if (typeof obj !== 'object' || obj === null || Array.isArray(obj)) return null;
  return obj as EventLine;
}

function withinWindow(ts: string | undefined, window: IngestWindow): boolean {
  if (!ts) return true; // loose: undated lines are kept
  const t = Date.parse(ts);
  if (!Number.isFinite(t)) return true;
  if (window.startedAt) {
    const lo = Date.parse(window.startedAt);
    // 5s slack: file mtime and ts resolution disagree at boundaries.
    if (Number.isFinite(lo) && t < lo - 5_000) return false;
  }
  if (window.terminalAt) {
    const hi = Date.parse(window.terminalAt);
    if (Number.isFinite(hi) && t > hi + 5_000) return false;
  }
  return true;
}

interface CursorRow {
  byte_offset: number;
  generation: number;
}

/**
 * Ingest one run's ledgers inside the caller's transaction (better-
 * sqlite3 `db.transaction` composes — nested calls join the outer
 * transaction so snapshot+events commit atomically, plan E12).
 */
export function ingestRunEvents(
  db: MonitorDb,
  projectId: string,
  runId: string,
  workspace: string,
  eventsFile: string | null | undefined,
  window: IngestWindow,
): IngestStats {
  const stats: IngestStats = {
    inserted: 0,
    parseErrors: 0,
    sourceResets: 0,
    files: 0,
  };

  const cursorStmt = db.prepare(
    'SELECT byte_offset, generation FROM event_cursors WHERE project_id = ? AND run_id = ? AND file = ?',
  );
  const cursorWrite = db.prepare(
    `INSERT INTO event_cursors (project_id, run_id, file, byte_offset, generation)
     VALUES (?, ?, ?, ?, ?)
     ON CONFLICT(project_id, run_id, file) DO UPDATE SET
       byte_offset = excluded.byte_offset,
       generation = excluded.generation`,
  );
  const maxSeqStmt = db.prepare(
    'SELECT MAX(seq) AS m FROM run_events WHERE project_id = ? AND run_id = ?',
  );
  const insertStmt = db.prepare(
    `INSERT OR IGNORE INTO run_events
       (project_id, run_id, seq, file, byte_offset, generation, ts, iteration, hat, topic, payload, truncated)
     VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)`,
  );

  for (const file of enumerateEventFiles(workspace, eventsFile)) {
    stats.files += 1;
    let size: number;
    try {
      size = fs.statSync(file).size;
    } catch {
      continue;
    }
    const cursorRow = cursorStmt.get(projectId, runId, file) as
      | CursorRow
      | undefined;
    let cursor = cursorRow?.byte_offset ?? 0;
    let generation = cursorRow?.generation ?? 0;
    if (size < cursor) {
      // File truncated/rewritten (`ralph events --clear`, rotation):
      // reset to 0 under a bumped generation so re-read bytes at
      // already-seen offsets do not dedupe against the old ledger
      // (plan §15 source_reset).
      cursor = 0;
      generation += 1;
      stats.sourceResets += 1;
    }
    if (size === cursor) continue;

    const fd = fs.openSync(file, 'r');
    try {
      const chunk = Buffer.alloc(Math.min(size - cursor, 8 * 1024 * 1024));
      const read = fs.readSync(fd, chunk, 0, chunk.length, cursor);
      const data = chunk.subarray(0, read);
      const lastNl = data.lastIndexOf(0x0a); // '\n'
      if (lastNl === -1) continue; // no complete line yet
      const consumed = data.subarray(0, lastNl + 1);
      const lines = consumed.toString('utf8').split('\n');
      lines.pop(); // trailing empty after final \n

      let offset = cursor;
      for (const line of lines) {
        const lineStart = offset;
        offset += Buffer.byteLength(line, 'utf8') + 1;
        if (!line.trim()) continue;
        const ev = parseLine(line);
        if (!ev) {
          stats.parseErrors += 1;
          continue;
        }
        const ts = typeof ev.ts === 'string' ? ev.ts : undefined;
        if (!withinWindow(ts, window)) continue;
        const hat =
          typeof ev.hat === 'string'
            ? ev.hat
            : typeof ev.source === 'string'
              ? ev.source
              : null;
        const payload =
          typeof ev.payload === 'string'
            ? ev.payload
            : ev.payload === undefined
              ? null
              : JSON.stringify(ev.payload);
        const truncated =
          payload !== null && payload.length > TRUNCATE_THRESHOLD ? 1 : 0;
        const seq =
          ((maxSeqStmt.get(projectId, runId) as { m: number | null }).m ??
            0) + 1;
        const res = insertStmt.run(
          projectId,
          runId,
          seq,
          file,
          lineStart,
          generation,
          ts ?? null,
          typeof ev.iteration === 'number' ? ev.iteration : null,
          hat,
          typeof ev.topic === 'string' ? ev.topic : null,
          payload,
          truncated,
        );
        stats.inserted += res.changes;
      }
      cursorWrite.run(projectId, runId, file, cursor + consumed.length, generation);
    } finally {
      fs.closeSync(fd);
    }
  }

  if (stats.parseErrors > 0 || stats.sourceResets > 0) {
    db.prepare(
      `INSERT INTO run_evidence (project_id, run_id, parse_errors, source_resets, updated_at)
       VALUES (?, ?, ?, ?, ?)
       ON CONFLICT(project_id, run_id) DO UPDATE SET
         parse_errors = parse_errors + excluded.parse_errors,
         source_resets = source_resets + excluded.source_resets,
         updated_at = excluded.updated_at`,
    ).run(
      projectId,
      runId,
      stats.parseErrors,
      stats.sourceResets,
      new Date().toISOString(),
    );
  }
  return stats;
}
