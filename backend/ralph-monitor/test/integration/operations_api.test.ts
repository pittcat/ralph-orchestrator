import test from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {
  FAKE_RALPH_DIR,
  collectOnceFake,
  getSession,
  makeApp,
  makeProjectDir,
  makeSnapshot,
  registerProject,
  unsafeHeaders,
  writeMonitorFixture,
} from '../helpers.js';

/**
 * Plan5-U2 API journey. The fake-ralph binary must be first on PATH
 * for the in-process dispatch spawn — set once for this file's
 * process (node:test runs each file in its own process).
 */
process.env.PATH = `${FAKE_RALPH_DIR}:${process.env.PATH}`;

async function waitForFile(p: string, ms = 3000): Promise<string> {
  const deadline = Date.now() + ms;
  for (;;) {
    if (fs.existsSync(p)) return fs.readFileSync(p, 'utf8');
    if (Date.now() > deadline) throw new Error(`timed out waiting for ${p}`);
    await new Promise((r) => setTimeout(r, 50));
  }
}

function runningRun(dir: string, runId = 'loop-live') {
  return {
    run_id: runId,
    kind: 'primary',
    state: 'running',
    pid: 4242,
    cwd: dir,
    workspace: dir,
    repo_root: dir,
    events_file: path.join(dir, '.ralph', 'events.jsonl'),
    actions: [
      { id: 'stop', available: true, revision: 'rev-stop' },
      { id: 'retry', available: false, reason: 'not in merge queue', revision: 'rev-retry' },
      { id: 'merge', available: false, reason: 'no diff', revision: 'rev-merge' },
      {
        id: 'emit',
        available: true,
        revision: 'rev-emit',
        input_schema: { topics: ['loop.cancel', 'task.resume'] },
      },
    ],
    evidence: { manifest_present: true, registry_present: true },
  };
}

test('operations API: stop → confirmed receipt; audit row visible', async () => {
  const dir = makeProjectDir();
  const { app, db } = makeApp();
  try {
    const reg = await registerProject(app, dir);
    const projectId = reg.json().project.id as string;
    writeMonitorFixture(dir, { snapshot: makeSnapshot(dir, [runningRun(dir)]) });
    const round = await collectOnceFake(db);
    assert.equal(round.ok, 1);

    const session = await getSession(app);
    const gid = `m:${projectId}:loop-live`;
    const res = await app.inject({
      method: 'POST',
      url: '/api/monitor/v1/operations',
      headers: unsafeHeaders(session),
      payload: {
        kind: 'stop',
        project_id: projectId,
        run_gid: gid,
        idempotency_key: 'api-stop-1',
        expected_revision: 'rev-stop',
        params: { force: true },
      },
    });
    assert.equal(res.statusCode, 201, res.body);
    const op = res.json().operation;
    assert.equal(op.state, 'confirmed');
    assert.equal(op.receipt.ok, true);
    assert.equal(op.receipt.action, 'stop');
    assert.equal(op.actor, session.cookie.match(/monitor_sid=([^;]+)/)![1]);

    // Idempotent replay → same op, no second spawn.
    const again = await app.inject({
      method: 'POST',
      url: '/api/monitor/v1/operations',
      headers: unsafeHeaders(session),
      payload: {
        kind: 'stop',
        project_id: projectId,
        run_gid: gid,
        idempotency_key: 'api-stop-1',
        params: {},
      },
    });
    assert.equal(again.statusCode, 200);
    assert.equal(again.json().operation.id, op.id);

    // Audit list.
    const list = await app.inject({
      method: 'GET',
      url: `/api/monitor/v1/operations?run_gid=${gid}`,
    });
    const ops = list.json().operations;
    assert.equal(ops.length, 1);
    assert.equal(ops[0].actor, op.actor);

    // Stream carries operation.updated.
    const ev = db
      .prepare("SELECT kind FROM monitor_events WHERE kind = 'operation.updated'")
      .all() as { kind: string }[];
    assert.ok(ev.length >= 1);
  } finally {
    await app.close();
  }
});

test('operations API: launch → dispatched → collect round confirms + notifies', async () => {
  const dir = makeProjectDir();
  const { app, db } = makeApp();
  try {
    const reg = await registerProject(app, dir);
    const projectId = reg.json().project.id as string;
    writeMonitorFixture(dir, { snapshot: makeSnapshot(dir, []) });
    await collectOnceFake(db);

    const session = await getSession(app);
    const res = await app.inject({
      method: 'POST',
      url: '/api/monitor/v1/operations',
      headers: unsafeHeaders(session),
      payload: {
        kind: 'launch',
        project_id: projectId,
        idempotency_key: 'api-launch-1',
        params: { preset: 'debug', prompt: 'do the thing' },
      },
    });
    assert.equal(res.statusCode, 201, res.body);
    const op = res.json().operation;
    assert.equal(op.state, 'dispatched');
    assert.match(op.clientRequestId, /^op-/);

    // The fake `ralph run` records the launch asynchronously — wait
    // for the spawned child to write, then the collect round reads it
    // back and confirms via client_request_id.
    await waitForFile(path.join(dir, '.monitor-fixture', 'launched-runs.jsonl'));
    await collectOnceFake(db);
    const got = await app.inject({
      method: 'GET',
      url: `/api/monitor/v1/operations/${op.id}`,
    });
    const updated = got.json().operation;
    assert.equal(updated.state, 'confirmed');
    assert.match(updated.runGid, /^m:.*:loop-fake-/);

    const notif = db
      .prepare("SELECT kind FROM notifications WHERE kind = 'operation.confirmed'")
      .all();
    assert.equal(notif.length, 1);
  } finally {
    await app.close();
  }
});

test('operations API: emit writes to the run events file', async () => {
  const dir = makeProjectDir();
  const { app, db } = makeApp();
  try {
    const reg = await registerProject(app, dir);
    const projectId = reg.json().project.id as string;
    const run = runningRun(dir);
    writeMonitorFixture(dir, { snapshot: makeSnapshot(dir, [run]) });
    await collectOnceFake(db);

    const session = await getSession(app);
    const res = await app.inject({
      method: 'POST',
      url: '/api/monitor/v1/operations',
      headers: unsafeHeaders(session),
      payload: {
        kind: 'emit',
        project_id: projectId,
        run_gid: `m:${projectId}:loop-live`,
        idempotency_key: 'api-emit-1',
        params: { topic: 'loop.cancel', payload: '{"by":"test"}' },
      },
    });
    assert.equal(res.statusCode, 201, res.body);
    assert.equal(res.json().operation.state, 'confirmed');
    const line = fs
      .readFileSync(path.join(dir, '.ralph', 'events.jsonl'), 'utf8')
      .trim();
    assert.match(line, /"topic":"loop.cancel"/);
  } finally {
    await app.close();
  }
});

test('operations API: error matrix (capability/revision/in-flight/not-found)', async () => {
  const dir = makeProjectDir();
  const { app, db } = makeApp();
  try {
    const reg = await registerProject(app, dir);
    const projectId = reg.json().project.id as string;
    writeMonitorFixture(dir, { snapshot: makeSnapshot(dir, [runningRun(dir)]) });
    await collectOnceFake(db);
    const session = await getSession(app);
    const gid = `m:${projectId}:loop-live`;
    const post = (payload: Record<string, unknown>) =>
      app.inject({
        method: 'POST',
        url: '/api/monitor/v1/operations',
        headers: unsafeHeaders(session),
        payload,
      });

    const missing = await post({
      kind: 'stop', project_id: projectId, run_gid: `m:${projectId}:nope`,
      idempotency_key: 'err-key-1',
    });
    assert.equal(missing.statusCode, 404);

    const cap = await post({
      kind: 'retry', project_id: projectId, run_gid: gid, idempotency_key: 'err-key-2',
    });
    assert.equal(cap.statusCode, 422);
    assert.equal(cap.json().error.code, 'capability_unavailable');
    assert.match(cap.json().error.message, /merge queue/);

    const rev = await post({
      kind: 'stop', project_id: projectId, run_gid: gid,
      idempotency_key: 'err-key-3', expected_revision: 'stale',
    });
    assert.equal(rev.statusCode, 409);
    assert.equal(rev.json().error.code, 'revision_conflict');

    const badTopic = await post({
      kind: 'emit', project_id: projectId, run_gid: gid,
      idempotency_key: 'err-key-4', params: { topic: 'hack.me' },
    });
    assert.equal(badTopic.statusCode, 422);

    const first = await post({
      kind: 'stop', project_id: projectId, run_gid: gid, idempotency_key: 'err-key-5',
    });
    assert.equal(first.statusCode, 201);
    // The stop confirmed → row is terminal → a second op is allowed.
    const second = await post({
      kind: 'stop', project_id: projectId, run_gid: gid, idempotency_key: 'err-key-6',
    });
    assert.equal(second.statusCode, 201);

    // CSRF-free request rejected.
    const noCsrf = await app.inject({
      method: 'POST',
      url: '/api/monitor/v1/operations',
      payload: { kind: 'stop', project_id: projectId, run_gid: gid, idempotency_key: 'err-key-7' },
    });
    assert.equal(noCsrf.statusCode, 403);
  } finally {
    await app.close();
  }
});

test('operations API: dispatch failure → failed state with stderr tail', async () => {
  const dir = makeProjectDir();
  const { app, db } = makeApp();
  try {
    const reg = await registerProject(app, dir);
    const projectId = reg.json().project.id as string;
    writeMonitorFixture(dir, { snapshot: makeSnapshot(dir, [runningRun(dir)]) });
    await collectOnceFake(db);
    fs.writeFileSync(path.join(dir, '.monitor-fixture', 'ops-fail'), 'stop');

    const session = await getSession(app);
    const res = await app.inject({
      method: 'POST',
      url: '/api/monitor/v1/operations',
      headers: unsafeHeaders(session),
      payload: {
        kind: 'stop', project_id: projectId,
        run_gid: `m:${projectId}:loop-live`,
        idempotency_key: 'fail-key-1', params: {},
      },
    });
    assert.equal(res.statusCode, 201);
    const op = res.json().operation;
    assert.equal(op.state, 'failed');
    assert.match(op.error, /exit 5|fixture-forced/);
  } finally {
    await app.close();
  }
});

test('operations API: confirm_timeout then POST recheck re-opens window', async () => {
  const dir = makeProjectDir();
  const { app, db } = makeApp();
  try {
    const reg = await registerProject(app, dir);
    const projectId = reg.json().project.id as string;
    writeMonitorFixture(dir, { snapshot: makeSnapshot(dir, []) });
    await collectOnceFake(db);

    const session = await getSession(app);
    const res = await app.inject({
      method: 'POST',
      url: '/api/monitor/v1/operations',
      headers: unsafeHeaders(session),
      payload: {
        kind: 'launch', project_id: projectId,
        idempotency_key: 'slow-launch', params: { preset: 'debug', prompt: 'x' },
      },
    });
    const op = res.json().operation;
    assert.equal(op.state, 'dispatched');

    // Hide the launched-runs evidence so the manifest match can't
    // happen; age the row past the confirm window → timeout.
    const launched = path.join(dir, '.monitor-fixture', 'launched-runs.jsonl');
    const stash = await waitForFile(launched);
    fs.rmSync(launched);
    db.prepare("UPDATE operations SET created_at = '2020-01-01T00:00:00Z' WHERE id = ?")
      .run(op.id);
    await collectOnceFake(db);
    let current = (
      await app.inject({ method: 'GET', url: `/api/monitor/v1/operations/${op.id}` })
    ).json().operation;
    assert.equal(current.state, 'confirm_timeout');

    // Recheck re-opens the window; restoring the launched-runs entry
    // (what the runtime manifest would eventually publish) lets the
    // client_request_id match land → confirmed.
    fs.writeFileSync(launched, stash);
    await collectOnceFake(db);
    const recheck = await app.inject({
      method: 'POST',
      url: `/api/monitor/v1/operations/${op.id}/recheck`,
      headers: unsafeHeaders(session),
    });
    assert.equal(recheck.statusCode, 200);
    current = recheck.json().operation;
    assert.equal(current.state, 'confirmed');
    assert.match(current.runGid, /loop-fake-/);
  } finally {
    await app.close();
  }
});

test('presets catalog: builtin list + custom hats dir', async () => {
  const dir = makeProjectDir();
  fs.mkdirSync(path.join(dir, '.ralph', 'hats'), { recursive: true });
  fs.writeFileSync(path.join(dir, '.ralph', 'hats', 'reviewer.yml'), 'name: reviewer\n');
  fs.writeFileSync(path.join(dir, '.ralph', 'hats', 'fixer.yaml'), 'name: fixer\n');
  const { app } = makeApp();
  try {
    const reg = await registerProject(app, dir);
    const projectId = reg.json().project.id as string;
    const res = await app.inject({
      method: 'GET',
      url: `/api/monitor/v1/projects/${projectId}/presets`,
    });
    assert.equal(res.statusCode, 200);
    const body = res.json();
    const builtin = body.presets.filter((p: { source: string }) => p.source === 'builtin');
    const custom = body.presets.filter((p: { source: string }) => p.source === 'custom');
    assert.ok(builtin.some((p: { name: string }) => p.name === 'ce-executor-pipeline'));
    assert.deepEqual(
      custom.map((p: { id: string }) => p.id).sort(),
      ['directory:fixer', 'directory:reviewer'],
    );

    const notFound = await app.inject({
      method: 'GET',
      url: '/api/monitor/v1/projects/proj_nope/presets',
    });
    assert.equal(notFound.statusCode, 404);
  } finally {
    await app.close();
  }
});
