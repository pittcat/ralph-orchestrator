import assert from 'node:assert/strict';
import { test } from 'node:test';
import { ingestSnapshot } from '../../src/db/run_projection.js';
import { parseSnapshot } from '../../src/ralph/snapshot.js';
import {
  getSession,
  insertProject,
  makeApp,
  makeSnapshot,
  unsafeHeaders,
} from '../helpers.js';

const T = '2026-10-28T12:00:00Z';

function ingest(
  db: Parameters<typeof ingestSnapshot>[0],
  projectId: string,
  workspace: string,
  runs: Record<string, unknown>[],
  at = T,
): void {
  const parsed = parseSnapshot(makeSnapshot(workspace, runs));
  if (!parsed.ok) throw new Error(`fixture: ${parsed.message}`);
  ingestSnapshot(db, projectId, parsed.snapshot, 'sha', at);
}

function run(
  runId: string,
  overrides: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    run_id: runId,
    kind: 'primary',
    state: 'exited',
    prompt_preview: 'task alpha',
    preset: 'builtin:debug',
    started_at: '2026-10-28T10:00:00Z',
    terminal: { reason: 'completed', at: T },
    evidence: { manifest_present: true },
    ...overrides,
  };
}

test('GET /runs filters by project/state/q + sort + pagination + total + watermark', async () => {
  const { app, db, machineId } = makeApp();
  const pa = insertProject(db, '/w/a', 'A');
  const pb = insertProject(db, '/w/b', 'B');
  ingest(db, pa, '/w/a', [
    run('run-1', {
      prompt_preview: 'alpha fix',
      started_at: '2026-10-28T10:00:00Z',
    }),
    run('run-2', {
      prompt_preview: 'beta work',
      started_at: '2026-10-28T11:00:00Z',
      terminal: { reason: 'consecutive_failures', at: T },
    }),
  ]);
  ingest(db, pb, '/w/b', [run('run-3', { prompt_preview: 'gamma task' })], '2026-10-28T12:30:00Z');

  // Unfiltered: 3 runs, newest first, total+watermark present.
  let res = await app.inject({ method: 'GET', url: '/api/monitor/v1/runs' });
  assert.equal(res.statusCode, 200);
  let body = res.json();
  assert.equal(body.total, 3);
  assert.equal(body.runs.length, 3);
  assert.equal(body.watermark, '2026-10-28T12:30:00Z');
  assert.equal(body.runs[0].runId, 'run-2'); // -startedAt default
  assert.equal(
    body.runs[0].globalRunId,
    `${machineId}:${pa}:run-2`,
  );

  // state filter (exited_failed only).
  res = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/runs?state=exited_failed',
  });
  body = res.json();
  assert.equal(body.total, 1);
  assert.equal(body.runs[0].runId, 'run-2');

  // csv states.
  res = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/runs?state=exited_success,exited_failed',
  });
  assert.equal(res.json().total, 3);

  // project filter.
  res = await app.inject({
    method: 'GET',
    url: `/api/monitor/v1/runs?project=${pa}`,
  });
  body = res.json();
  assert.equal(body.total, 2);
  assert.equal(body.watermark, T);

  // keyword matches prompt_preview.
  res = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/runs?q=alpha',
  });
  body = res.json();
  assert.equal(body.total, 1);
  assert.equal(body.runs[0].runId, 'run-1');

  // ascending sort flips order.
  res = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/runs?sort=startedAt',
  });
  assert.equal(res.json().runs[0].runId, 'run-1');

  // pagination: limit=1 page through.
  res = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/runs?limit=1&offset=1',
  });
  body = res.json();
  assert.equal(body.total, 3);
  assert.equal(body.runs.length, 1);
  assert.equal(body.runs[0].runId, 'run-1');

  await app.close();
});

test('GET /runs rejects invalid filters with typed 4xx', async () => {
  const { app } = makeApp();
  for (const [qs, code] of [
    ['state=bogus', 'invalid_filter'],
    ['sort=runId', 'invalid_filter'],
    ['limit=0', 'invalid_filter'],
    ['limit=201', 'invalid_filter'],
    ['offset=-1', 'invalid_filter'],
  ] as const) {
    const res = await app.inject({
      method: 'GET',
      url: `/api/monitor/v1/runs?${qs}`,
    });
    assert.equal(res.statusCode, 400, qs);
    assert.equal(res.json().error.code, code, qs);
  }
  const res = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/runs?project=proj_missing',
  });
  assert.equal(res.statusCode, 404);
  assert.equal(res.json().error.code, 'project_not_found');
  await app.close();
});

test('GET /runs/:globalId → detail with evidence timestamps; 404s typed', async () => {
  const { app, db, machineId } = makeApp();
  const pa = insertProject(db, '/w/a');
  ingest(db, pa, '/w/a', [
    run('loop-z', {
      state: 'unknown',
      terminal: null,
      pid: 99999999,
      evidence: { lock_present: true },
    }),
  ]);

  const res = await app.inject({
    method: 'GET',
    url: `/api/monitor/v1/runs/${machineId}:${pa}:loop-z`,
  });
  assert.equal(res.statusCode, 200);
  const detail = res.json().run;
  assert.equal(detail.state, 'exited_unknown');
  assert.equal(detail.stateEvidence.lockPresent, true);
  // S6: unknown shows evidence timestamps, not success flags.
  assert.equal(detail.evidenceTimestamps.lastObservedAt, T);
  assert.equal(detail.evidenceTimestamps.collectedAt, T);
  assert.equal(detail.evidenceTimestamps.terminalAt, null);
  assert.equal(detail.evidenceTimestamps.lastEventAt, null);

  // foreign machine id → 404
  let bad = await app.inject({
    method: 'GET',
    url: `/api/monitor/v1/runs/other-machine:${pa}:loop-z`,
  });
  assert.equal(bad.statusCode, 404);
  assert.equal(bad.json().error.code, 'run_not_found');

  // malformed id → 400
  bad = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/runs/not-three-segments',
  });
  assert.equal(bad.statusCode, 400);
  assert.equal(bad.json().error.code, 'invalid_run_id');

  // unknown run → 404
  bad = await app.inject({
    method: 'GET',
    url: `/api/monitor/v1/runs/${machineId}:${pa}:nope`,
  });
  assert.equal(bad.statusCode, 404);
  assert.equal(bad.json().error.code, 'run_not_found');

  await app.close();
});

test('DELETE project cascades runs/cursors and writes an audit row', async () => {
  const { app, db } = makeApp();
  const pa = insertProject(db, '/w/a');
  ingest(db, pa, '/w/a', [run('run-1')]);

  const session = await getSession(app);
  const del = await app.inject({
    method: 'DELETE',
    url: `/api/monitor/v1/projects/${pa}`,
    headers: unsafeHeaders(session),
  });
  assert.equal(del.statusCode, 204);

  const runs = db
    .prepare('SELECT COUNT(*) AS n FROM runs WHERE project_id = ?')
    .get(pa) as { n: number };
  assert.equal(runs.n, 0);
  const cursor = db
    .prepare('SELECT COUNT(*) AS n FROM source_cursors WHERE project_id = ?')
    .get(pa) as { n: number };
  assert.equal(cursor.n, 0);
  const audit = db
    .prepare(
      'SELECT action, project_id FROM audit_log WHERE project_id = ?',
    )
    .all(pa) as { action: string; project_id: string }[];
  assert.equal(audit.length, 1);
  assert.equal(audit[0].action, 'project_deleted');

  await app.close();
});
