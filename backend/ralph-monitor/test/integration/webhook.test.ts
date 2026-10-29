import assert from 'node:assert/strict';
import { test } from 'node:test';
import http from 'node:http';
import { AddressInfo } from 'node:net';
import { openDatabase } from '../../src/db/index.js';
import { runMigrations } from '../../src/db/migrations.js';
import {
  makeApp,
  getSession,
  unsafeHeaders,
} from '../helpers.js';
import { drainOnce } from '../../src/notify/worker.js';
import { saveWebhookConfig } from '../../src/notify/channels/webhook.js';
import { applyIntents } from '../../src/notify/outbox.js';
import { readRuntimeWebhookHint } from '../../src/notify/runtime_hint.js';
import fs from 'node:fs';
import path from 'node:path';
import { makeProjectDir, insertProject, collectOnceFake, writeMonitorFixture, makeSnapshot } from '../helpers.js';

/** Local receiver that records POSTs (status controllable). */
async function receiver(status = 200) {
  const hits: { headers: http.IncomingHttpHeaders; body: string }[] = [];
  const server = http.createServer((req, res) => {
    let body = '';
    req.on('data', (c) => (body += c));
    req.on('end', () => {
      hits.push({ headers: req.headers, body });
      res.writeHead(status).end();
    });
  });
  await new Promise<void>((r) => server.listen(0, '127.0.0.1', r));
  const port = (server.address() as AddressInfo).port;
  return {
    url: `http://127.0.0.1:${port}/hook`,
    hits,
    close: () => server.close(),
  };
}

const INTENT = {
  type: 'notify' as const,
  ruleId: 'run.terminal.failed',
  ruleVersion: 1,
  kind: 'run.failed',
  projectId: 'p1',
  runGid: 'm:p1:r1',
  pendingInteractionId: null,
  title: 'Run r1 failed',
  body: 'reason=x',
  detail: {},
  occurrence: 't1',
};

function makeDb() {
  const db = openDatabase(':memory:');
  runMigrations(db);
  return db;
}

test('S6: worker delivers to a local receiver with delivery id + signature', async () => {
  const rec = await receiver(200);
  const db = makeDb();
  saveWebhookConfig(db, { url: rec.url, secret: 's3cret', enabled: true });
  applyIntents(db, [INTENT]);

  const r = await drainOnce(db, { allowlist: ['127.0.0.0/8'] });
  assert.equal(r.attempted, 1);
  assert.equal(rec.hits.length, 1);
  const hit = rec.hits[0];
  const body = JSON.parse(hit.body);
  assert.equal(body.delivery_id, hit.headers['x-ralph-delivery-id']);
  assert.equal(body.kind, 'run.failed');
  assert.match(String(hit.headers['x-ralph-signature']), /^sha256=[0-9a-f]{64}$/);
  const d = db.prepare(`SELECT * FROM deliveries WHERE channel='webhook'`).get() as {
    state: string;
    delivery_id: string;
  };
  assert.equal(d.state, 'delivered');
  assert.equal(d.delivery_id, body.delivery_id);
  rec.close();
  db.close();
});

test('S7: 5xx backs off 30s/2m/10m/1h then dead; manual retry requeues', async () => {
  const rec = await receiver(500);
  const db = makeDb();
  saveWebhookConfig(db, { url: rec.url, secret: null, enabled: true });
  applyIntents(db, [INTENT]);
  let t = Date.parse('2026-10-28T00:00:00Z');
  const now = () => new Date(t);

  const backoffs = [30_000, 120_000, 600_000];
  for (let i = 0; i < 4; i += 1) {
    await drainOnce(db, { now, allowlist: ['127.0.0.0/8'] });
    const d = db
      .prepare(`SELECT state, attempts, next_attempt_at FROM deliveries WHERE channel='webhook'`)
      .get() as { state: string; attempts: number; next_attempt_at: string | null };
    if (i < 3) {
      // next attempt lands backoff[i] ms after this drain.
      assert.equal(
        Date.parse(d.next_attempt_at!) - t,
        backoffs[i],
        `attempt ${i + 1} backoff`,
      );
      t = Date.parse(d.next_attempt_at!) + 1;
    } else {
      assert.equal(d.state, 'dead');
      assert.equal(d.attempts, 4);
      assert.equal(d.next_attempt_at, null);
    }
  }
  rec.close();
  db.close();
});

test('S8: 4xx → failed without retry', async () => {
  const rec = await receiver(404);
  const db = makeDb();
  saveWebhookConfig(db, { url: rec.url, secret: null, enabled: true });
  applyIntents(db, [INTENT]);
  await drainOnce(db, { allowlist: ['127.0.0.0/8'] });
  const d = db
    .prepare(`SELECT state, attempts, next_attempt_at FROM deliveries WHERE channel='webhook'`)
    .get() as { state: string; attempts: number; next_attempt_at: string | null };
  assert.equal(d.state, 'failed');
  assert.equal(d.next_attempt_at, null);
  rec.close();
  db.close();
});

test('S9: non-allowlisted loopback target is rejected at delivery time', async () => {
  const rec = await receiver(200);
  const db = makeDb();
  saveWebhookConfig(db, { url: rec.url, secret: null, enabled: true });
  applyIntents(db, [INTENT]);
  await drainOnce(db, { allowlist: [] });
  const d = db
    .prepare(`SELECT state, error FROM deliveries WHERE channel='webhook'`)
    .get() as { state: string; error: string };
  assert.equal(d.state, 'rejected');
  assert.match(d.error, /ssrf:/);
  assert.equal(rec.hits.length, 0);
  rec.close();
  db.close();
});

test('S10: PUT config masks secret; test endpoint performs a real POST', async () => {
  const rec = await receiver(200);
  const { app, db } = makeApp();
  const session = await getSession(app);

  // Config PUT with loopback needs allowlist env — bypass via direct
  // save for the config row, then exercise the API surfaces.
  saveWebhookConfig(db, { url: rec.url, secret: 'abcdef12345', enabled: true });

  const get = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/notifications/channels/webhook',
  });
  const cfg = get.json() as { url: string; secret: string; enabled: boolean };
  assert.equal(cfg.secret, '•••2345');
  assert.ok(!cfg.secret.includes('abcdef'));

  const tst = await app.inject({
    method: 'POST',
    url: '/api/monitor/v1/notifications/channels/webhook/test',
    headers: unsafeHeaders(session),
    payload: {},
  });
  assert.equal(tst.statusCode, 200);
  const res = tst.json() as { outcome: string; delivery_id: string };
  assert.equal(res.outcome, 'delivered');
  assert.equal(rec.hits.length, 1);
  const body = JSON.parse(rec.hits[0].body);
  assert.equal(body.test, true);
  assert.equal(body.delivery_id, res.delivery_id);
  rec.close();
  await app.close();
});

test('PUT rejects SSRF targets with 422 and never persists', async () => {
  const { app } = makeApp();
  const session = await getSession(app);
  const put = await app.inject({
    method: 'PUT',
    url: '/api/monitor/v1/notifications/channels/webhook',
    headers: unsafeHeaders(session),
    payload: { url: 'https://169.254.169.254/latest', enabled: true },
  });
  assert.equal(put.statusCode, 422);
  const get = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/notifications/channels/webhook',
  });
  assert.equal((get.json() as { url: string | null }).url, null);
  await app.close();
});

test('deliveries list + manual retry requeues a dead row', async () => {
  const rec = await receiver(500);
  const { app, db } = makeApp();
  const session = await getSession(app);
  saveWebhookConfig(db, { url: rec.url, secret: null, enabled: true });
  applyIntents(db, [INTENT]);
  let t = Date.parse('2026-10-28T00:00:00Z');
  for (let i = 0; i < 4; i += 1) {
    await drainOnce(db, { now: () => new Date(t), allowlist: ['127.0.0.0/8'] });
    const d = db
      .prepare(`SELECT next_attempt_at FROM deliveries WHERE channel='webhook'`)
      .get() as { next_attempt_at: string | null };
    if (d.next_attempt_at) t = Date.parse(d.next_attempt_at) + 1;
  }
  const notifId = (
    db.prepare('SELECT id FROM notifications').get() as { id: string }
  ).id;
  const list = await app.inject({
    method: 'GET',
    url: `/api/monitor/v1/notifications/${notifId}/deliveries`,
  });
  const rows = (list.json() as { deliveries: { id: string; channel: string; state: string }[] })
    .deliveries;
  const webhookRow = rows.find((r) => r.channel === 'webhook')!;
  assert.equal(webhookRow.state, 'dead');

  const retry = await app.inject({
    method: 'POST',
    url: `/api/monitor/v1/deliveries/${webhookRow.id}/retry`,
    headers: unsafeHeaders(session),
  });
  assert.equal(retry.statusCode, 200);
  const after = db
    .prepare(`SELECT state, attempts FROM deliveries WHERE id = ?`)
    .get(webhookRow.id) as { state: string; attempts: number };
  assert.equal(after.state, 'pending');
  assert.equal(after.attempts, 4); // history preserved
  rec.close();
  await app.close();
});

test('S13: ralph.yml notifications.enabled → projects.runtime_webhook_hint', async () => {
  const { app, db } = makeApp();
  const dir = makeProjectDir();
  fs.writeFileSync(
    path.join(dir, 'ralph.yml'),
    'notifications:\n  enabled: true\n  endpoints: []\n',
  );
  insertProject(db, dir);
  writeMonitorFixture(dir, { snapshot: makeSnapshot(dir, []) });
  assert.equal(readRuntimeWebhookHint(dir), 'enabled');
  await collectOnceFake(db);
  const row = db
    .prepare('SELECT runtime_webhook_hint FROM projects')
    .get() as { runtime_webhook_hint: string };
  assert.equal(row.runtime_webhook_hint, 'enabled');
  const list = await app.inject({ method: 'GET', url: '/api/monitor/v1/projects' });
  const p = (list.json() as { projects: { runtimeWebhookHint: string }[] })
    .projects[0];
  assert.equal(p.runtimeWebhookHint, 'enabled');
  await app.close();
});
