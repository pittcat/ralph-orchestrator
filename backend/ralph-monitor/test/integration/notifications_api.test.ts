import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  collectOnceFake,
  getSession,
  insertProject,
  makeApp,
  makeProjectDir,
  makeSnapshot,
  unsafeHeaders,
  writeMonitorFixture,
} from '../helpers.js';

const T = '2026-10-28T12:00:00Z';

async function seedOne(app: ReturnType<typeof makeApp>['app'], db: ReturnType<typeof makeApp>['db']) {
  const dir = makeProjectDir();
  insertProject(db, dir);
  writeMonitorFixture(dir, {
    snapshot: makeSnapshot(dir, [
      {
        run_id: 'loop-1',
        kind: 'primary',
        state: 'exited',
        cwd: '/w',
        terminal: { reason: 'completed', at: T },
        evidence: { manifest_present: true },
      },
    ]),
  });
  await collectOnceFake(db);
}

test('notifications API: list/read/read-all/mute + pending endpoint', async () => {
  const { app, db } = makeApp();
  await seedOne(app, db);
  const session = await getSession(app);

  const list = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/notifications',
  });
  assert.equal(list.statusCode, 200);
  const body = list.json() as {
    notifications: { id: string; kind: string; state: string; link: string }[];
    total: number;
  };
  assert.equal(body.total, 1);
  const n = body.notifications[0];
  assert.equal(n.kind, 'run.completed');
  assert.equal(n.state, 'unread');
  assert.match(n.link, /monitor\/runs\/.+source=notification/);

  // read is idempotent.
  for (let i = 0; i < 2; i += 1) {
    const r = await app.inject({
      method: 'POST',
      url: `/api/monitor/v1/notifications/${n.id}/read`,
      headers: unsafeHeaders(session),
    });
    assert.equal(r.statusCode, 200);
    assert.equal((r.json() as { notification: { state: string } }).notification.state, 'read');
  }

  // mute hides it from the default list but evidence survives (S12).
  const mute = await app.inject({
    method: 'POST',
    url: `/api/monitor/v1/notifications/${n.id}/mute`,
    headers: unsafeHeaders(session),
  });
  assert.equal(mute.statusCode, 200);
  const def = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/notifications',
  });
  assert.equal((def.json() as { total: number }).total, 0);
  const muted = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/notifications?state=muted',
  });
  assert.equal((muted.json() as { total: number }).total, 1);

  const readAll = await app.inject({
    method: 'POST',
    url: '/api/monitor/v1/notifications/read-all',
    headers: unsafeHeaders(session),
  });
  assert.equal(readAll.statusCode, 200);

  const pending = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/pending-interactions',
  });
  assert.equal(pending.statusCode, 200);
  assert.deepEqual((pending.json() as { pending: unknown[] }).pending, []);

  // Unknown id → 404.
  const nf = await app.inject({
    method: 'POST',
    url: '/api/monitor/v1/notifications/nope/read',
    headers: unsafeHeaders(session),
  });
  assert.equal(nf.statusCode, 404);
  await app.close();
});

test('notification kinds filter + pagination params validate', async () => {
  const { app, db } = makeApp();
  await seedOne(app, db);
  const bad = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/notifications?state=bogus',
  });
  assert.equal(bad.statusCode, 400);
  const kind = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/notifications?kind=run.failed',
  });
  assert.equal((kind.json() as { total: number }).total, 0);
  const lim = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/notifications?limit=1&offset=0',
  });
  const j = lim.json() as { notifications: unknown[]; limit: number };
  assert.equal(j.limit, 1);
  await app.close();
});
