import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { openDatabase } from '../../src/db/index.js';
import { runMigrations } from '../../src/db/migrations.js';
import {
  createIntent,
  parseRunGid,
  resolveWithin,
  type OperationRow,
} from '../../src/ops/intents.js';
import { buildArgv, parseReceipt } from '../../src/ops/dispatch.js';
import { confirmPendingOps } from '../../src/ops/confirm.js';
import { tmpDir, tmpDbPath, insertProject } from '../helpers.js';

function freshDb() {
  const db = openDatabase(tmpDbPath());
  runMigrations(db);
  return db;
}

const ACTIONS = [
  { id: 'stop', available: true, revision: 'rev-stop' },
  { id: 'retry', available: false, reason: 'not in merge queue', revision: 'rev-retry' },
  { id: 'merge', available: false, reason: 'no diff', revision: 'rev-merge' },
  {
    id: 'emit',
    available: true,
    revision: 'rev-emit',
    input_schema: { topics: ['loop.cancel', 'task.resume'] },
  },
];

function insertRun(
  db: ReturnType<typeof freshDb>,
  projectId: string,
  runId: string,
  actions: unknown = ACTIONS,
) {
  db.prepare(
    `INSERT INTO runs (project_id, run_id, kind, state, state_evidence,
       last_observed_at, actions, events_file)
     VALUES (?, ?, 'primary', 'running', '{}', '2026-10-28T12:00:00Z', ?, ?)`,
  ).run(projectId, runId, JSON.stringify(actions), '/proj/.ralph/events.jsonl');
  return `m:${projectId}:${runId}`;
}

test('parseRunGid accepts only m:<project>:<run> form', () => {
  assert.deepEqual(parseRunGid('m:p:r'), { projectId: 'p', runId: 'r' });
  assert.equal(parseRunGid('p:r'), null);
  assert.equal(parseRunGid('m:p'), null);
  assert.equal(parseRunGid('m::r'), null);
});

test('resolveWithin confines to project root', () => {
  const dir = tmpDir();
  fs.writeFileSync(path.join(dir, 'prompt.md'), 'x');
  const ok = resolveWithin(dir, 'prompt.md');
  assert.equal(ok.ok, true);
  const escape = resolveWithin(dir, '../outside.txt');
  assert.equal(escape.ok, false);
  const missing = resolveWithin(dir, 'nope.txt');
  assert.equal(missing.ok, false);
});

test('createIntent: invalid body → 400; unknown project → 404', () => {
  const db = freshDb();
  const bad = createIntent(db, { kind: 'bogus' }, 'sid', null);
  assert.equal(bad.ok, false);
  if (!bad.ok) assert.equal(bad.status, 400);

  const nope = createIntent(
    db,
    { kind: 'stop', project_id: 'nope', run_gid: 'm:nope:r', idempotency_key: 'aaaaaaaa' },
    'sid',
    null,
  );
  assert.equal(nope.ok, false);
  if (!nope.ok) assert.equal(nope.status, 404);
  db.close();
});

test('createIntent: run op happy path + capability/revision gates', () => {
  const db = freshDb();
  const proj = insertProject(db, '/p');
  const gid = insertRun(db, proj, 'loop-1');

  const ok = createIntent(
    db,
    {
      kind: 'stop',
      project_id: proj,
      run_gid: gid,
      idempotency_key: 'idem-stop-1',
      expected_revision: 'rev-stop',
    },
    'sid-1',
    'http://localhost:5173',
  );
  assert.equal(ok.ok, true);
  if (ok.ok) {
    assert.equal(ok.operation.state, 'recorded');
    assert.equal(ok.operation.actor, 'sid-1');
    assert.equal(ok.operation.origin, 'http://localhost:5173');
  }

  // Idempotent replay → same row, no second insert.
  const replay = createIntent(
    db,
    {
      kind: 'stop',
      project_id: proj,
      run_gid: gid,
      idempotency_key: 'idem-stop-1',
      expected_revision: 'rev-stop',
    },
    'sid-2',
    null,
  );
  assert.equal(replay.ok, true);
  if (replay.ok && ok.ok) {
    assert.equal(replay.replay, true);
    assert.equal(replay.operation.id, ok.operation.id);
  }
  const count = (
    db.prepare('SELECT COUNT(*) AS n FROM operations').get() as { n: number }
  ).n;
  assert.equal(count, 1);

  // Second op on the same run while first is non-terminal → 409.
  const dup = createIntent(
    db,
    { kind: 'stop', project_id: proj, run_gid: gid, idempotency_key: 'idem-stop-2' },
    'sid',
    null,
  );
  assert.equal(dup.ok, false);
  if (!dup.ok) assert.equal(dup.code, 'operation_in_flight');

  // Capability unavailable → 422 with runtime reason.
  const cap = createIntent(
    db,
    { kind: 'retry', project_id: proj, run_gid: gid, idempotency_key: 'idem-retry-1' },
    'sid',
    null,
  );
  assert.equal(cap.ok, false);
  if (!cap.ok) {
    assert.equal(cap.status, 422);
    assert.equal(cap.code, 'capability_unavailable');
    assert.match(cap.message, /merge queue/);
  }

  // Revision conflict → 409 with current revision.
  const rev = createIntent(
    db,
    {
      kind: 'stop',
      project_id: proj,
      run_gid: gid,
      idempotency_key: 'idem-stop-3',
      expected_revision: 'stale-rev',
    },
    'sid',
    null,
  );
  assert.equal(rev.ok, false);
  if (!rev.ok) assert.equal(rev.code, 'revision_conflict');

  // run_gid not matching project → 422.
  const gidMismatch = createIntent(
    db,
    { kind: 'stop', project_id: proj, run_gid: 'm:other:loop-1', idempotency_key: 'idem-x-key-1' },
    'sid',
    null,
  );
  assert.equal(gidMismatch.ok, false);
  if (!gidMismatch.ok) assert.equal(gidMismatch.code, 'invalid_run_gid');

  db.close();
});

test('createIntent: emit topic whitelist from input_schema', () => {
  const db = freshDb();
  const proj = insertProject(db, '/p');
  const gid = insertRun(db, proj, 'loop-2');

  const off = createIntent(
    db,
    {
      kind: 'emit',
      project_id: proj,
      run_gid: gid,
      idempotency_key: 'idem-emit-bad',
      params: { topic: 'not.allowed' },
    },
    'sid',
    null,
  );
  assert.equal(off.ok, false);
  if (!off.ok) assert.equal(off.code, 'capability_unavailable');

  const on = createIntent(
    db,
    {
      kind: 'emit',
      project_id: proj,
      run_gid: gid,
      idempotency_key: 'idem-emit-ok',
      params: { topic: 'loop.cancel', payload: '{}' },
    },
    'sid',
    null,
  );
  assert.equal(on.ok, true);
  db.close();
});

test('createIntent: launch validates params + path confinement', () => {
  const db = freshDb();
  const dir = tmpDir();
  fs.writeFileSync(path.join(dir, 'prompt.md'), 'do it');
  const proj = insertProject(db, dir);

  const noPreset = createIntent(
    db,
    {
      kind: 'launch',
      project_id: proj,
      idempotency_key: 'idem-launch-1',
      params: { prompt: 'hi' },
    },
    'sid',
    null,
  );
  assert.equal(noPreset.ok, false);

  const escaped = createIntent(
    db,
    {
      kind: 'launch',
      project_id: proj,
      idempotency_key: 'idem-launch-2',
      params: { preset: 'x', prompt_file: '../etc/passwd' },
    },
    'sid',
    null,
  );
  assert.equal(escaped.ok, false);
  if (!escaped.ok) assert.equal(escaped.code, 'invalid_params');

  const ok = createIntent(
    db,
    {
      kind: 'launch',
      project_id: proj,
      idempotency_key: 'idem-launch-3',
      params: { preset: 'debug', prompt_file: 'prompt.md', max_iterations: 3 },
    },
    'sid',
    null,
  );
  assert.equal(ok.ok, true);
  if (ok.ok) {
    assert.match(ok.operation.client_request_id!, /^op-/);
    assert.equal(ok.operation.run_gid, null);
  }
  db.close();
});

test('buildArgv maps each kind to an arg array (never a shell)', () => {
  const base: OperationRow = {
    id: 'op1',
    idempotency_key: 'k',
    kind: 'stop',
    project_id: 'p',
    run_gid: 'm:p:loop-9',
    client_request_id: null,
    params: '{}',
    expected_revision: null,
    state: 'recorded',
    receipt: null,
    actor: null,
    origin: null,
    error: null,
    created_at: 't',
    dispatched_at: null,
    confirmed_at: null,
  };
  assert.deepEqual(buildArgv({ ...base, params: '{"force":true}' }), [
    'loops', 'stop', 'loop-9', '--force', '--format', 'json',
  ]);
  assert.deepEqual(
    buildArgv({ ...base, kind: 'retry', params: '{"steering":"focus U2"}' }),
    ['loops', 'retry', 'loop-9', '--steering', 'focus U2', '--format', 'json'],
  );
  assert.deepEqual(buildArgv({ ...base, kind: 'merge' }), [
    'loops', 'merge', 'loop-9', '--format', 'json',
  ]);
  assert.deepEqual(
    buildArgv(
      { ...base, kind: 'emit', params: '{"topic":"loop.cancel","payload":"{}"}' },
      '/w/.ralph/events.jsonl',
    ),
    ['emit', 'loop.cancel', '{}', '--file', '/w/.ralph/events.jsonl'],
  );
  assert.deepEqual(
    buildArgv({
      ...base,
      kind: 'launch',
      run_gid: null,
      client_request_id: 'op-x',
      params: JSON.stringify({
        preset: 'debug',
        prompt: 'hi',
        plan: 'plan.md',
        max_iterations: 5,
        backend: 'claude',
      }),
    }),
    [
      'run', '-a', '-H', 'debug', '-p', 'hi',
      '--plan', 'plan.md', '--max-iterations', '5', '--backend', 'claude',
      '--client-request-id', 'op-x',
    ],
  );
});

test('parseReceipt reads the last JSON line only', () => {
  assert.deepEqual(parseReceipt('noise\n{"ok":true,"action":"stop"}'), {
    ok: true,
    action: 'stop',
  });
  assert.equal(parseReceipt('no json here'), null);
  assert.equal(parseReceipt(''), null);
});

test('confirmPendingOps: client_request_id match → confirmed + notification', () => {
  const db = freshDb();
  const proj = insertProject(db, '/p');
  const gid = insertRun(db, proj, 'loop-launched');
  db.prepare('UPDATE runs SET client_request_id = ? WHERE project_id = ?')
    .run('op-123', proj);
  const now = new Date('2026-10-28T12:00:00Z');

  db.prepare(
    `INSERT INTO operations
       (id, idempotency_key, kind, project_id, run_gid, client_request_id,
        params, state, created_at)
     VALUES ('op1', 'k1', 'launch', ?, NULL, 'op-123', '{}', 'dispatched', ?)`,
  ).run(proj, new Date(now.getTime() - 1000).toISOString());

  confirmPendingOps(db, proj, now);

  const op = db.prepare('SELECT * FROM operations WHERE id = ?')
    .get('op1') as OperationRow;
  assert.equal(op.state, 'confirmed');
  assert.equal(op.run_gid, gid);
  const notif = db
    .prepare("SELECT kind, run_gid FROM notifications WHERE kind = 'operation.confirmed'")
    .all() as { kind: string; run_gid: string }[];
  assert.equal(notif.length, 1);
  assert.equal(notif[0].run_gid, gid);
  const events = db
    .prepare("SELECT kind FROM monitor_events WHERE kind = 'operation.updated'")
    .all() as { kind: string }[];
  assert.ok(events.length >= 1);
  db.close();
});

test('confirmPendingOps: aged unmatched op → confirm_timeout', () => {
  const db = freshDb();
  const proj = insertProject(db, '/p');
  const now = new Date('2026-10-28T12:00:00Z');
  db.prepare(
    `INSERT INTO operations
       (id, idempotency_key, kind, project_id, client_request_id,
        params, state, created_at)
     VALUES ('op2', 'k2', 'launch', ?, 'op-missing', '{}', 'dispatched', ?)`,
  ).run(proj, new Date(now.getTime() - 60_000).toISOString());

  confirmPendingOps(db, proj, now, 15_000);
  const op = db.prepare('SELECT state FROM operations WHERE id = ?')
    .get('op2') as { state: string };
  assert.equal(op.state, 'confirm_timeout');
  db.close();
});
