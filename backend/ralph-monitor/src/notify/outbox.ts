/**
 * Notification outbox (Plan4-U1): intents → durable rows, all inside
 * the caller's ingest transaction. The `notifications.idem_key`
 * UNIQUE constraint is the dedupe authority — `INSERT OR IGNORE`
 * makes replay a no-op, so only a truly new intent produces rows,
 * deliveries, and the `notification.created` stream event (R3/S3).
 */

import { createHash, randomUUID } from 'node:crypto';
import type { MonitorDb } from '../db/index.js';
import { appendEvent } from '../stream/monitor_log.js';
import { loadWebhookConfig } from './channels/webhook.js';
import type { Intent, NotifyIntent, ResolveIntent } from './rules.js';

export function idemKey(i: NotifyIntent): string {
  return createHash('sha256')
    .update(
      `${i.runGid ?? i.pendingInteractionId ?? ''}|${i.ruleId}|v${i.ruleVersion}|${i.occurrence}`,
    )
    .digest('hex');
}

/** Deep link back to the run evidence (S5). */
export function notificationLink(runGid: string | null): string | null {
  return runGid
    ? `/monitor/runs/${encodeURIComponent(runGid)}?source=notification`
    : null;
}

function applyNotify(db: MonitorDb, i: NotifyIntent, at: string): void {
  const id = randomUUID();
  const res = db
    .prepare(
      `INSERT OR IGNORE INTO notifications
         (id, idem_key, kind, project_id, run_gid, pending_interaction_id,
          title, body, detail, state, created_at)
       VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, 'unread', ?)`,
    )
    .run(
      id,
      idemKey(i),
      i.kind,
      i.projectId,
      i.runGid,
      i.pendingInteractionId,
      i.title,
      i.body,
      JSON.stringify(i.detail),
      at,
    );
  if (res.changes === 0) return; // dedup hit — full no-op (S3)

  // in_app lands delivered immediately; the browser channel registers
  // a pending row — page-side dispatch happens via the stream (D9).
  const insDelivery = db.prepare(
    `INSERT INTO deliveries
       (id, notification_id, channel, state, attempts, delivery_id, created_at, updated_at)
     VALUES (?, ?, ?, ?, 0, ?, ?, ?)`,
  );
  insDelivery.run(
    randomUUID(),
    id,
    'in_app',
    'delivered',
    randomUUID(),
    at,
    at,
  );
  insDelivery.run(randomUUID(), id, 'browser', 'pending', null, at, at);
  // Webhook rows only exist while the channel is enabled (Plan4-U2):
  // a disabled channel leaves no half-queued deliveries to surprise
  // the user when they turn it on later.
  const webhook = loadWebhookConfig(db);
  if (webhook?.enabled && webhook.url) {
    insDelivery.run(randomUUID(), id, 'webhook', 'pending', null, at, at);
  }

  appendEvent(
    db,
    'notification.created',
    { projectId: i.projectId, runId: i.runGid },
    { id, kind: i.kind, title: i.title, link: notificationLink(i.runGid) },
  );
}

function applyResolve(db: MonitorDb, i: ResolveIntent, at: string): void {
  const res = db
    .prepare(
      `UPDATE pending_interactions
       SET status = 'resolved', resolved_at = ?
       WHERE id = ? AND status = 'pending'`,
    )
    .run(at, i.pendingInteractionId);
  if (res.changes === 0) return;

  const linked = db
    .prepare(
      `UPDATE notifications
       SET state = 'resolved', resolved_at = ?
       WHERE pending_interaction_id = ? AND state IN ('unread', 'read', 'muted')`,
    )
    .run(at, i.pendingInteractionId);
  if (linked.changes > 0) {
    appendEvent(
      db,
      'notification.updated',
      { projectId: i.projectId },
      {
        pending_interaction_id: i.pendingInteractionId,
        state: 'resolved',
        reason: i.reason,
      },
    );
  }
}

/**
 * Apply intents inside the current transaction. Returns counts for
 * tests/diagnostics.
 */
export function applyIntents(
  db: MonitorDb,
  intents: Intent[],
  at: string = new Date().toISOString(),
): { notified: number; resolved: number } {
  let notified = 0;
  let resolved = 0;
  for (const intent of intents) {
    if (intent.type === 'notify') {
      applyNotify(db, intent, at);
      notified += 1;
    } else {
      applyResolve(db, intent, at);
      resolved += 1;
    }
  }
  return { notified, resolved };
}

/**
 * Upsert a pending_interactions row (same transaction). `occurrence`
 * distinguishes rounds of waiting within one session.
 */
export function upsertPendingInteraction(
  db: MonitorDb,
  row: {
    id: string;
    projectId: string;
    sessionId: string;
    runGid?: string | null;
    prompt?: string | null;
    occurrence: string;
    at: string;
  },
): void {
  db.prepare(
    `INSERT INTO pending_interactions
       (id, project_id, session_id, run_gid, prompt, status, occurrence, created_at)
     VALUES (?, ?, ?, ?, ?, 'pending', ?, ?)
     ON CONFLICT(project_id, session_id) DO UPDATE SET
       run_gid = excluded.run_gid,
       prompt = excluded.prompt,
       status = 'pending',
       occurrence = excluded.occurrence,
       resolved_at = NULL`,
  ).run(
    row.id,
    row.projectId,
    row.sessionId,
    row.runGid ?? null,
    row.prompt ?? null,
    row.occurrence,
    row.at,
  );
}
