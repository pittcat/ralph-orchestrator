/**
 * Webhook channel (Plan4-U2): one POST per delivery attempt.
 * `redirect:'manual'` — a 3xx answer is a failure, never a bounce
 * through an unchecked target (D5). The body carries `delivery_id`
 * and the `X-Ralph-Delivery-Id` header so receivers can dedupe
 * at-least-once repeats (D4). `X-Ralph-Signature` is an HMAC over the
 * raw body when a secret is configured; the response body is
 * discarded — only the status matters.
 */

import { createHmac } from 'node:crypto';
import type { MonitorDb } from '../../db/index.js';

export interface WebhookConfig {
  url: string;
  secret: string | null;
  enabled: boolean;
}

export interface WebhookPayload {
  delivery_id: string;
  kind: string;
  run_gid: string | null;
  title: string;
  at: string;
  link: string | null;
  test?: boolean;
}

export type DeliverResult =
  | { outcome: 'delivered'; status: number }
  | { outcome: 'failed'; status: number; error: string }
  | { outcome: 'retry'; error: string };

export function signBody(secret: string, body: string): string {
  return `sha256=${createHmac('sha256', secret).update(body).digest('hex')}`;
}

/**
 * Attempt one POST. Classification (plan §1): 2xx→delivered;
 * 3xx→failed(`redirect_rejected`); 4xx→failed (no auto-retry, S8);
 * 5xx/timeout/network→retry (S7).
 */
export async function deliverWebhook(
  url: string,
  payload: WebhookPayload,
  secret: string | null,
  opts: { timeoutMs?: number; fetchFn?: typeof fetch } = {},
): Promise<DeliverResult> {
  const fetchFn = opts.fetchFn ?? fetch;
  const body = JSON.stringify(payload);
  const headers: Record<string, string> = {
    'content-type': 'application/json',
    'x-ralph-delivery-id': payload.delivery_id,
  };
  if (secret) headers['x-ralph-signature'] = signBody(secret, body);

  let res: Response;
  try {
    res = await fetchFn(url, {
      method: 'POST',
      headers,
      body,
      redirect: 'manual',
      signal: AbortSignal.timeout(opts.timeoutMs ?? 5_000),
    });
  } catch (err) {
    return {
      outcome: 'retry',
      error: err instanceof Error ? err.message : String(err),
    };
  }
  // Discard the response body deliberately (plan §15) — only status.
  await res.arrayBuffer().catch(() => undefined);

  if (res.status >= 200 && res.status < 300) {
    return { outcome: 'delivered', status: res.status };
  }
  if (res.status >= 300 && res.status < 400) {
    return {
      outcome: 'failed',
      status: res.status,
      error: `redirect_rejected (status ${res.status})`,
    };
  }
  if (res.status >= 400 && res.status < 500) {
    return {
      outcome: 'failed',
      status: res.status,
      error: `client_error status ${res.status}`,
    };
  }
  return {
    outcome: 'retry',
    error: `server_error status ${res.status}`,
  };
}

// ── channel_config persistence ──

export function loadWebhookConfig(db: MonitorDb): WebhookConfig | null {
  const row = db
    .prepare(`SELECT config FROM channel_config WHERE channel = 'webhook'`)
    .get() as { config: string } | undefined;
  if (!row) return null;
  const c = JSON.parse(row.config) as WebhookConfig;
  return { url: c.url, secret: c.secret ?? null, enabled: !!c.enabled };
}

export function saveWebhookConfig(
  db: MonitorDb,
  config: WebhookConfig,
): void {
  db.prepare(
    `INSERT INTO channel_config (channel, config, updated_at)
     VALUES ('webhook', ?, ?)
     ON CONFLICT(channel) DO UPDATE SET
       config = excluded.config, updated_at = excluded.updated_at`,
  ).run(
    JSON.stringify({ url: config.url, secret: config.secret, enabled: config.enabled }),
    new Date().toISOString(),
  );
}

/** Masked read for API echoes (KTD4 — secret never leaves the API). */
export function maskSecret(secret: string | null): string | null {
  if (!secret) return null;
  return `•••${secret.slice(-4)}`;
}
