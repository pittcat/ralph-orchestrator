import assert from 'node:assert/strict';
import { test } from 'node:test';
import fs from 'node:fs';
import path from 'node:path';
import {
  collectOnceFake,
  insertProject,
  makeApp,
  makeProjectDir,
  makeSnapshot,
  writeMonitorFixture,
} from '../helpers.js';
import { readEvents } from '../../src/stream/monitor_log.js';

const T = '2026-10-28T12:00:00Z';

function exitedRun(runId: string, reason = 'completed') {
  return {
    run_id: runId,
    kind: 'primary',
    state: 'exited',
    cwd: '/w',
    repo_root: '/w',
    terminal: { reason, at: T },
    manifest: `.ralph/runs/${runId}.manifest.json`,
    evidence: { manifest_present: true },
  };
}

function writeSession(
  projectDir: string,
  sessionId: string,
  status: string,
  updatedAt = '2026-10-28T11:00:00Z',
): void {
  const dir = path.join(projectDir, '.ralph', 'planning-sessions', sessionId);
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(
    path.join(dir, 'session.json'),
    JSON.stringify({
      id: sessionId,
      prompt: 'should I continue?',
      status,
      createdAt: '2026-10-28T10:00:00Z',
      updatedAt,
      iterations: 2,
    }),
  );
}

test('S1/S2: terminal transitions create one typed notification each', async () => {
  const { app, db } = makeApp();
  const dir = makeProjectDir();
  insertProject(db, dir, 'proj');
  writeMonitorFixture(dir, {
    snapshot: makeSnapshot(dir, [
      exitedRun('loop-ok', 'completed'),
      exitedRun('loop-bad', 'interrupted'),
    ]),
  });
  await collectOnceFake(db);

  const rows = db
    .prepare('SELECT kind, run_gid, state FROM notifications ORDER BY kind')
    .all() as { kind: string; run_gid: string; state: string }[];
  assert.deepEqual(
    rows.map((r) => [r.kind, r.state]),
    [
      ['run.completed', 'unread'],
      ['run.failed', 'unread'],
    ],
  );
  const kinds = readEvents(db, 0).map((e) => e.kind);
  assert.ok(kinds.includes('notification.created'));
  await app.close();
});

test('S3: re-ingesting the same terminal evidence never duplicates', async () => {
  const { app, db } = makeApp();
  const dir = makeProjectDir();
  insertProject(db, dir);
  writeMonitorFixture(dir, {
    snapshot: makeSnapshot(dir, [exitedRun('loop-ok')]),
  });
  await collectOnceFake(db);
  await collectOnceFake(db);
  await collectOnceFake(db);
  assert.equal(
    (db.prepare('SELECT COUNT(*) n FROM notifications').get() as { n: number })
      .n,
    1,
  );
  await app.close();
});

test('S4: session pending → notification + pending row; resolved消退', async () => {
  const { app, db } = makeApp();
  const dir = makeProjectDir();
  insertProject(db, dir);
  writeMonitorFixture(dir, { snapshot: makeSnapshot(dir, []) });
  writeSession(dir, 'sess-1', 'waiting_for_input');
  await collectOnceFake(db);

  const pi = db
    .prepare(`SELECT * FROM pending_interactions WHERE session_id = 'sess-1'`)
    .get() as { status: string } | undefined;
  assert.equal(pi?.status, 'pending');
  const n = db
    .prepare(`SELECT * FROM notifications WHERE kind = 'interaction.pending'`)
    .get() as { state: string } | undefined;
  assert.equal(n?.state, 'unread');

  // Responded (status flips) → row resolves + notification resolves.
  writeSession(dir, 'sess-1', 'completed', '2026-10-28T11:05:00Z');
  await collectOnceFake(db);
  const pi2 = db
    .prepare(`SELECT status FROM pending_interactions WHERE session_id = 'sess-1'`)
    .get() as { status: string };
  assert.equal(pi2.status, 'resolved');
  const n2 = db
    .prepare(`SELECT state FROM notifications WHERE kind = 'interaction.pending'`)
    .get() as { state: string };
  assert.equal(n2.state, 'resolved');
  await app.close();
});

test('deleted session file resolves the pending row conservatively', async () => {
  const { app, db } = makeApp();
  const dir = makeProjectDir();
  insertProject(db, dir);
  writeMonitorFixture(dir, { snapshot: makeSnapshot(dir, []) });
  writeSession(dir, 'sess-9', 'waiting_for_input');
  await collectOnceFake(db);
  fs.rmSync(path.join(dir, '.ralph', 'planning-sessions', 'sess-9'), {
    recursive: true,
  });
  await collectOnceFake(db);
  const pi = db
    .prepare(`SELECT status FROM pending_interactions WHERE session_id = 'sess-9'`)
    .get() as { status: string };
  assert.equal(pi.status, 'resolved');
  await app.close();
});

test('malformed session files are skipped, never fatal', async () => {
  const { app, db } = makeApp();
  const dir = makeProjectDir();
  insertProject(db, dir);
  writeMonitorFixture(dir, { snapshot: makeSnapshot(dir, []) });
  const sdir = path.join(dir, '.ralph', 'planning-sessions', 'broken');
  fs.mkdirSync(sdir, { recursive: true });
  fs.writeFileSync(path.join(sdir, 'session.json'), '{not json');
  const summary = await collectOnceFake(db);
  assert.equal(summary.ok, 1);
  assert.equal(
    (db.prepare('SELECT COUNT(*) n FROM pending_interactions').get() as {
      n: number;
    }).n,
    0,
  );
  await app.close();
});
