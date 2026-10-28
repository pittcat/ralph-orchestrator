import assert from 'node:assert/strict';
import { test } from 'node:test';
import { getCursor, getRun, queryRuns } from '../../src/db/run_projection.js';
import {
  collectOnceFake,
  insertProject,
  makeApp,
  makeProjectDir,
  makeSnapshot,
  writeMonitorFixture,
} from '../helpers.js';

const T = '2026-10-28T12:00:00Z';

test('S9: failing project does not block others; its runs stay stale', async () => {
  const { app, db } = makeApp();
  const dirA = makeProjectDir();
  const dirB = makeProjectDir();
  const idA = insertProject(db, dirA, 'A');
  const idB = insertProject(db, dirB, 'B');

  // Seed A with one good round so it has an existing run + watermark.
  writeMonitorFixture(dirA, {
    snapshot: makeSnapshot(dirA, [
      {
        run_id: 'loop-a',
        kind: 'worktree',
        state: 'exited',
        terminal: { reason: 'completed', at: T },
        evidence: { manifest_present: true },
      },
    ]),
  });
  writeMonitorFixture(dirB, {
    snapshot: makeSnapshot(dirB, [
      {
        run_id: 'loop-b',
        kind: 'worktree',
        state: 'running',
        pid: 111,
        evidence: { registry_present: true },
      },
    ]),
  });
  await collectOnceFake(db);
  assert.equal(getRun(db, idA, 'loop-a')?.state, 'exited_success');

  // A's ralph now fails; B keeps working.
  writeMonitorFixture(dirA, { fail: 'nonzero' });
  const summary = await collectOnceFake(db);
  assert.equal(summary.ok, 1);
  assert.equal(summary.failed, 1);

  // B's row is fine; A's existing run is preserved and reads stale.
  const b = getRun(db, idB, 'loop-b');
  assert.equal(b?.state, 'running');
  assert.equal(b?.stale, false);
  const a = getRun(db, idA, 'loop-a');
  assert.equal(a?.state, 'exited_success');
  assert.equal(a?.stale, true);
  assert.equal(getCursor(db, idA)?.consecutive_failures, 1);
  assert.equal(getCursor(db, idA)?.last_collected_at !== null, true);

  // Project health API surfaces A's failure with the error summary.
  const health = await app.inject({
    method: 'GET',
    url: `/api/monitor/v1/projects/${idA}/health`,
  });
  assert.equal(health.statusCode, 200);
  const body = health.json();
  assert.equal(body.collector, 'error');
  assert.match(body.error, /nonzero/);
  assert.ok(body.last_collected_at);

  const healthB = await app.inject({
    method: 'GET',
    url: `/api/monitor/v1/projects/${idB}/health`,
  });
  assert.equal(healthB.json().collector, 'ok');

  // A recovering clears the failure flag on the next round.
  writeMonitorFixture(dirA, {
    snapshot: makeSnapshot(dirA, [
      {
        run_id: 'loop-a',
        kind: 'worktree',
        state: 'exited',
        terminal: { reason: 'completed', at: T },
        evidence: { manifest_present: true },
      },
    ]),
  });
  await collectOnceFake(db);
  assert.equal(getCursor(db, idA)?.consecutive_failures, 0);
  const recovered = queryRuns(db, { projectId: idA, limit: 10, offset: 0 });
  assert.equal(recovered.rows[0].stale, false);

  await app.close();
});

test('malformed snapshot JSON → collection error, others unaffected', async () => {
  const { app, db } = makeApp();
  const dirA = makeProjectDir();
  const dirB = makeProjectDir();
  const idA = insertProject(db, dirA);
  insertProject(db, dirB);
  writeMonitorFixture(dirA, { fail: 'badjson' });
  writeMonitorFixture(dirB, {
    snapshot: makeSnapshot(dirB, [
      { run_id: 'loop-b', kind: 'worktree', state: 'running', pid: 7 },
    ]),
  });

  const summary = await collectOnceFake(db);
  assert.equal(summary.ok, 1);
  assert.equal(summary.failed, 1);
  const err = db
    .prepare(
      'SELECT error FROM collection_errors WHERE project_id = ? ORDER BY id DESC LIMIT 1',
    )
    .get(idA) as { error: string };
  assert.match(err.error, /bad_json/);
  await app.close();
});

test('unsupported schema_version → collection error with version detail', async () => {
  const { app, db } = makeApp();
  const dir = makeProjectDir();
  const id = insertProject(db, dir);
  writeMonitorFixture(dir, {
    snapshot: { schema_version: 'run-observation/v2', runs: [] },
  });
  const summary = await collectOnceFake(db);
  assert.equal(summary.failed, 1);
  const err = db
    .prepare(
      'SELECT error FROM collection_errors WHERE project_id = ? ORDER BY id DESC LIMIT 1',
    )
    .get(id) as { error: string };
  assert.match(err.error, /unsupported_version/);
  await app.close();
});
