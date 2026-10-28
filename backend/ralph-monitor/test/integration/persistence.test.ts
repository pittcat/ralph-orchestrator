import assert from 'node:assert/strict';
import { test } from 'node:test';
import { getCursor } from '../../src/db/run_projection.js';
import {
  collectOnceFake,
  insertProject,
  makeApp,
  makeProjectDir,
  makeSnapshot,
  tmpDbPath,
  writeMonitorFixture,
} from '../helpers.js';

const T = '2026-10-28T12:00:00Z';

test('S7: restart keeps runs + cursor resumes without a rescan', async () => {
  const dbPath = tmpDbPath();
  const dir = makeProjectDir();
  const snapshot = makeSnapshot(dir, [
    {
      run_id: 'loop-persist',
      kind: 'worktree',
      state: 'exited',
      terminal: { reason: 'completed', at: T },
      evidence: { manifest_present: true },
    },
  ]);
  writeMonitorFixture(dir, { snapshot });

  let projectId: string;
  {
    const { app, db } = makeApp(dbPath);
    projectId = insertProject(db, dir);
    await collectOnceFake(db);
    const list = await app.inject({
      method: 'GET',
      url: '/api/monitor/v1/runs',
    });
    assert.equal(list.json().total, 1);
    await app.close();
  }

  // New app instance over the same db file: rows + cursor survive.
  {
    const { app, db } = makeApp(dbPath);
    const cursor = getCursor(db, projectId);
    assert.ok(cursor?.last_collected_at, 'cursor persisted');
    assert.equal(cursor.consecutive_failures, 0);

    const list = await app.inject({
      method: 'GET',
      url: '/api/monitor/v1/runs',
    });
    const body = list.json();
    assert.equal(body.total, 1);
    assert.equal(body.runs[0].runId, 'loop-persist');
    assert.equal(body.runs[0].state, 'exited_success');

    // Collecting again is an idempotent upsert — still one row.
    await collectOnceFake(db);
    const again = await app.inject({
      method: 'GET',
      url: '/api/monitor/v1/runs',
    });
    assert.equal(again.json().total, 1);
    await app.close();
  }
});
