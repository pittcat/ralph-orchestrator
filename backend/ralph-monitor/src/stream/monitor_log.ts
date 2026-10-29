/**
 * Durable monitor event stream (Plan3-U3).
 *
 * `appendEvent` runs inside the caller's ingest transaction — a
 * projection change and its log row commit atomically (A3). After
 * every append the retained window is pruned to `KEEP` rows; seq is
 * AUTOINCREMENT and never reused, so SSE cursors stay meaningful
 * across restarts (S9).
 */

import type { MonitorDb } from '../db/index.js';

/** Retained replay window; cursors below `max(seq) - KEEP` resync. */
export const KEEP_EVENTS = 50_000;

export type MonitorEventKind =
  | 'run.created'
  | 'run.updated'
  | 'run.events'
  | 'run.health'
  | 'project.health'
  | 'project.updated'
  | 'notification.created'
  | 'notification.updated'
  | 'operation.updated';

export interface MonitorEventRow {
  seq: number;
  kind: string;
  project_id: string | null;
  run_id: string | null;
  payload: string | null;
  at: string;
}

const appendStmt = (db: MonitorDb) =>
  db.prepare(
    `INSERT INTO monitor_events (kind, project_id, run_id, payload, at)
     VALUES (?, ?, ?, ?, ?)`,
  );

const pruneStmt = (db: MonitorDb) =>
  db.prepare(
    `DELETE FROM monitor_events
     WHERE seq < (SELECT MAX(seq) FROM monitor_events) - ?`,
  );

/**
 * Append one stream row (inside the ingest transaction). `payload` is
 * serialized here so callers pass plain objects.
 */
export function appendEvent(
  db: MonitorDb,
  kind: MonitorEventKind,
  ref: { projectId?: string | null; runId?: string | null },
  payload: unknown,
): void {
  appendStmt(db).run(
    kind,
    ref.projectId ?? null,
    ref.runId ?? null,
    payload === undefined ? null : JSON.stringify(payload),
    new Date().toISOString(),
  );
  pruneStmt(db).run(KEEP_EVENTS);
}

/** `null` when the log is empty (fresh db → cursor 0 replays live only). */
export function streamBounds(
  db: MonitorDb,
): { min: number; max: number } | null {
  const row = db
    .prepare(
      'SELECT MIN(seq) AS min, MAX(seq) AS max FROM monitor_events',
    )
    .get() as { min: number | null; max: number | null };
  if (row.min === null || row.max === null) return null;
  return { min: row.min, max: row.max };
}

/** Rows with `seq > cursor`, ascending. `limit` pages replay reads. */
export function readEvents(
  db: MonitorDb,
  cursor: number,
  limit = 1000,
): MonitorEventRow[] {
  return db
    .prepare(
      `SELECT seq, kind, project_id, run_id, payload, at
       FROM monitor_events WHERE seq > ? ORDER BY seq ASC LIMIT ?`,
    )
    .all(cursor, limit) as MonitorEventRow[];
}

/** `id` values and `data` serialization for the SSE wire. */
export function toSseFrame(row: MonitorEventRow): string {
  const data = row.payload ?? '{}';
  return `id: ${row.seq}\nevent: ${row.kind}\ndata: ${data}\n\n`;
}
