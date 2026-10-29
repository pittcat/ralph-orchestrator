/**
 * Operation confirmation (Plan5-U2, D5/D9): launch ops confirm when
 * the collector sees a run whose manifest `client_request_id` matches
 * the operation's. The match can only happen inside the ingest
 * transaction's aftermath — runs rows are the projection evidence.
 * Unconfirmed ops older than the confirm window → `confirm_timeout`
 * (never auto-retried; S11 recheck is an explicit endpoint).
 */

import type { MonitorDb } from '../db/index.js';
import { appendEvent } from '../stream/monitor_log.js';
import { applyIntents } from '../notify/outbox.js';
import { loadRules } from '../notify/rules.js';
import type { OperationRow } from './intents.js';

export const DEFAULT_CONFIRM_WINDOW_MS = 15_000;

export function readConfirmWindowMs(): number {
  const raw = process.env.RALPH_MONITOR_CONFIRM_WINDOW_MS;
  const n = raw === undefined ? NaN : Number(raw);
  return Number.isFinite(n) && n > 0 ? n : DEFAULT_CONFIRM_WINDOW_MS;
}

function transition(
  db: MonitorDb,
  op: OperationRow,
  state: 'confirmed' | 'confirm_timeout',
  at: string,
  receipt?: unknown,
): void {
  db.prepare(
    `UPDATE operations SET state = ?,
       confirmed_at = COALESCE(?, confirmed_at),
       receipt = COALESCE(?, receipt)
     WHERE id = ?`,
  ).run(
    state,
    state === 'confirmed' ? at : null,
    receipt === undefined ? null : JSON.stringify(receipt),
    op.id,
  );
  appendEvent(
    db,
    'operation.updated',
    { projectId: op.project_id, runId: op.run_gid },
    { id: op.id, kind: op.kind, state, run_gid: op.run_gid },
  );
}

/**
 * Evaluate open launch operations for one project against the runs
 * projection. Called right after `ingestSnapshot` so a freshly
 * matched manifest confirms in the same collect round.
 */
export function confirmPendingOps(
  db: MonitorDb,
  projectId: string,
  now: Date = new Date(),
  confirmWindowMs: number = readConfirmWindowMs(),
): void {
  const open = db
    .prepare(
      `SELECT * FROM operations
       WHERE project_id = ? AND kind = 'launch'
         AND state IN ('recorded','dispatching','dispatched')`,
    )
    .all(projectId) as OperationRow[];

  for (const op of open) {
    const match = db
      .prepare(
        `SELECT run_id FROM runs
         WHERE project_id = ? AND client_request_id = ?`,
      )
      .get(projectId, op.client_request_id) as
      | { run_id: string }
      | undefined;

    if (match) {
      db.transaction(() => {
        transition(db, op, 'confirmed', now.toISOString(), {
          ok: true,
          kind: 'launch',
          run_id: match.run_id,
        });
        const gid = `m:${op.project_id}:${match.run_id}`;
        const rules = loadRules(db);
        applyIntents(
          db,
          rules
            .filter((r) => r.matcher.on === 'operation_confirmed')
            .map((r) => ({
              type: 'notify' as const,
              ruleId: r.id,
              ruleVersion: r.version,
              kind: 'operation.confirmed',
              projectId: op.project_id,
              runGid: gid,
              pendingInteractionId: null,
              title: `Operation ${op.kind} confirmed`,
              body: `client_request_id=${op.client_request_id}`,
              detail: {
                operation_id: op.id,
                kind: op.kind,
                run_id: match.run_id,
              },
              occurrence: `${op.id}:confirmed`,
            })),
          now.toISOString(),
        );
        db.prepare('UPDATE operations SET run_gid = ? WHERE id = ?').run(
          gid,
          op.id,
        );
      })();
      continue;
    }

    const age = now.getTime() - Date.parse(op.created_at);
    if (Number.isFinite(age) && age > confirmWindowMs) {
      db.transaction(() =>
        transition(db, op, 'confirm_timeout', now.toISOString()),
      )();
    }
  }
}

/**
 * `POST /operations/:id/recheck` — re-run the launch match for one
 * op. For dispatch-confirmed kinds this is a no-op (they're already
 * terminal); for `confirm_timeout` this re-opens the window so a
 * late-appearing manifest can still confirm.
 */
export function recheckOperation(
  db: MonitorDb,
  opId: string,
  now: Date = new Date(),
): OperationRow | undefined {
  const op = db
    .prepare('SELECT * FROM operations WHERE id = ?')
    .get(opId) as OperationRow | undefined;
  if (!op) return undefined;
  if (op.kind !== 'launch') return op;
  if (!['recorded', 'dispatching', 'dispatched', 'confirm_timeout'].includes(op.state)) {
    return op;
  }

  db.transaction(() => {
    if (op.state === 'confirm_timeout') {
      // Reopen the window: created_at resets the age budget for the
      // recheck round (S11 — explicit human action, not auto-retry).
      db.prepare(
        `UPDATE operations SET state = 'dispatched', created_at = ?
         WHERE id = ?`,
      ).run(now.toISOString(), op.id);
      appendEvent(
        db,
        'operation.updated',
        { projectId: op.project_id, runId: op.run_gid },
        { id: op.id, kind: op.kind, state: 'dispatched', recheck: true },
      );
    }
  })();

  confirmPendingOps(db, op.project_id, now, readConfirmWindowMs());
  return db
    .prepare('SELECT * FROM operations WHERE id = ?')
    .get(opId) as OperationRow;
}
