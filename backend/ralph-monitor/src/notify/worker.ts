/**
 * Delivery worker (Plan4-U2): one loop draining `deliveries` due rows
 * for the webhook channel — serial per batch (LIMIT 20) so the
 * worker never floods itself (§16). Backoff schedule
 * 30s/2m/10m/1h → `dead`; a crashed process simply finds stale
 * `next_attempt_at` rows due on the next boot (§14). Clock and fetch
 * are injectable for deterministic tests.
 */

import type { MonitorDb } from '../db/index.js';
import {
  deliverWebhook,
  loadWebhookConfig,
  type WebhookPayload,
} from './channels/webhook.js';
import { validateWebhookUrl, readAllowlistEnv } from './ssrf.js';
import { notificationLink } from './outbox.js';
import { randomUUID } from 'node:crypto';

export const WORKER_INTERVAL_MS = 5_000;
export const BATCH_LIMIT = 20;
export const BACKOFF_MS = [30_000, 120_000, 600_000, 3_600_000];
/** After this many failed attempts the delivery is `dead`. */
export const MAX_ATTEMPTS = 4;

interface DueRow {
  id: string;
  notification_id: string;
  delivery_id: string | null;
  attempts: number;
  kind: string;
  run_gid: string | null;
  title: string;
}

function dueDeliveries(db: MonitorDb, nowIso: string): DueRow[] {
  return db
    .prepare(
      `SELECT d.id, d.notification_id, d.delivery_id, d.attempts,
              n.kind, n.run_gid, n.title
       FROM deliveries d
       JOIN notifications n ON n.id = d.notification_id
       WHERE d.channel = 'webhook' AND d.state = 'pending'
         AND (d.next_attempt_at IS NULL OR d.next_attempt_at <= ?)
       ORDER BY d.created_at ASC
       LIMIT ?`,
    )
    .all(nowIso, BATCH_LIMIT) as DueRow[];
}

export interface WorkerDeps {
  now?: () => Date;
  fetchFn?: typeof fetch;
  allowlist?: string[];
}

/**
 * One drain pass — exported for tests. Returns per-row outcomes.
 */
export async function drainOnce(
  db: MonitorDb,
  deps: WorkerDeps = {},
): Promise<{ attempted: number }> {
  const now = deps.now ?? (() => new Date());
  const cfg = loadWebhookConfig(db);
  if (!cfg || !cfg.enabled || !cfg.url) return { attempted: 0 };

  const due = dueDeliveries(db, now().toISOString());
  let attempted = 0;
  for (const row of due) {
    attempted += 1;
    const deliveryId = row.delivery_id ?? randomUUID();
    const payload: WebhookPayload = {
      delivery_id: deliveryId,
      kind: row.kind,
      run_gid: row.run_gid,
      title: row.title,
      at: now().toISOString(),
      link: notificationLink(row.run_gid),
    };

    // SSRF re-validation per attempt (D5) — a config change or DNS
    // rebinding between queue and send is still checked.
    const verdict = await validateWebhookUrl(cfg.url, {
      allowlist: deps.allowlist ?? readAllowlistEnv(),
    });
    if (!verdict.ok) {
      db.prepare(
        `UPDATE deliveries SET state='rejected', attempts=?,
         next_attempt_at=NULL, error=?, updated_at=? WHERE id=?`,
      ).run(
        row.attempts + 1,
        `ssrf: ${verdict.code}`,
        now().toISOString(),
        row.id,
      );
      continue;
    }

    const res = await deliverWebhook(cfg.url, payload, cfg.secret, {
      fetchFn: deps.fetchFn,
    });
    const attempts = row.attempts + 1;
    if (res.outcome === 'delivered') {
      db.prepare(
        `UPDATE deliveries SET state='delivered', attempts=?, delivery_id=?,
         next_attempt_at=NULL, error=NULL, updated_at=? WHERE id=?`,
      ).run(attempts, deliveryId, now().toISOString(), row.id);
    } else if (res.outcome === 'failed') {
      // 4xx/3xx → failed, no automatic retry (S8).
      db.prepare(
        `UPDATE deliveries SET state='failed', attempts=?, delivery_id=?,
         next_attempt_at=NULL, error=?, updated_at=? WHERE id=?`,
      ).run(attempts, deliveryId, res.error, now().toISOString(), row.id);
    } else {
      const dead = attempts >= MAX_ATTEMPTS;
      const nextAt = dead
        ? null
        : new Date(now().getTime() + BACKOFF_MS[Math.min(attempts - 1, BACKOFF_MS.length - 1)]).toISOString();
      db.prepare(
        `UPDATE deliveries SET state=?, attempts=?, delivery_id=?,
         next_attempt_at=?, error=?, updated_at=? WHERE id=?`,
      ).run(
        dead ? 'dead' : 'pending',
        attempts,
        deliveryId,
        nextAt,
        res.error,
        now().toISOString(),
        row.id,
      );
    }
  }
  return { attempted };
}

export interface WorkerHandle {
  stop(): void;
}

export function startDeliveryWorker(
  db: MonitorDb,
  deps: WorkerDeps & { intervalMs?: number; log?: (m: string, e?: unknown) => void } = {},
): WorkerHandle {
  const interval = deps.intervalMs ?? WORKER_INTERVAL_MS;
  let stopped = false;
  let timer: NodeJS.Timeout | undefined;
  const tick = async () => {
    try {
      await drainOnce(db, deps);
    } catch (err) {
      deps.log?.('delivery worker tick failed', err);
    } finally {
      if (!stopped) timer = setTimeout(tick, interval);
    }
  };
  timer = setTimeout(tick, interval);
  return {
    stop() {
      stopped = true;
      if (timer) clearTimeout(timer);
    },
  };
}
