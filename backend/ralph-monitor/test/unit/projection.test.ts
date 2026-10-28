import assert from 'node:assert/strict';
import { test } from 'node:test';
import {
  getCursor,
  getRun,
  ingestSnapshot,
  queryRuns,
  recordCollectionError,
  snapshotSha256,
} from '../../src/db/run_projection.js';
import { parseSnapshot } from '../../src/ralph/snapshot.js';
import { insertProject, makeSnapshot } from '../helpers.js';
import { openDatabase } from '../../src/db/index.js';
import { runMigrations } from '../../src/db/migrations.js';

function db(): ReturnType<typeof openDatabase> {
  const d = openDatabase(':memory:');
  runMigrations(d);
  return d;
}

const NOW = '2026-10-28T12:00:00Z';

function snapshotOf(
  workspace: string,
  runs: Record<string, unknown>[],
): ReturnType<typeof parseSnapshot> extends { ok: true; snapshot: infer S }
  ? S
  : never {
  const parsed = parseSnapshot(makeSnapshot(workspace, runs));
  if (!parsed.ok) throw new Error(`fixture: ${parsed.message}`);
  return parsed.snapshot as never;
}

test('same snapshot ingested twice → single row (idempotent)', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  const snap = snapshotOf('/w/a', [
    {
      run_id: 'loop-x',
      kind: 'worktree',
      state: 'exited',
      terminal: { reason: 'completed', at: NOW },
      evidence: { manifest_present: true },
    },
  ]);
  ingestSnapshot(d, pid, snap, 'sha1', NOW);
  ingestSnapshot(d, pid, snap, 'sha1', '2026-10-28T12:00:10Z');
  const rows = d.prepare('SELECT * FROM runs WHERE project_id = ?').all(pid);
  assert.equal(rows.length, 1);
  assert.equal(
    (rows[0] as { last_observed_at: string }).last_observed_at,
    '2026-10-28T12:00:10Z',
  );
});

test('terminal state never regresses to running → state_conflict row', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      {
        run_id: 'loop-x',
        kind: 'worktree',
        state: 'exited',
        terminal: { reason: 'completed', at: NOW },
        evidence: { manifest_present: true },
      },
    ]),
    'sha1',
    NOW,
  );
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: 'loop-x', kind: 'worktree', state: 'running', pid: 7 },
    ]),
    'sha2',
    '2026-10-28T12:01:00Z',
  );
  const row = getRun(d, pid, 'loop-x');
  assert.equal(row?.state, 'exited_success');
  const errs = d
    .prepare('SELECT error FROM collection_errors WHERE project_id = ?')
    .all(pid) as { error: string }[];
  assert.equal(errs.length, 1);
  assert.match(errs[0].error, /state_conflict/);
});

test('"(primary)" display name is never stored as a run id (E6)', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: '(primary)', kind: 'primary', state: 'running', pid: 1 },
      { run_id: 'primary-20261028-120000', kind: 'primary', state: 'running', pid: 1 },
    ]),
    'sha',
    NOW,
  );
  const rows = d.prepare('SELECT run_id FROM runs').all() as {
    run_id: string;
  }[];
  assert.deepEqual(
    rows.map((r) => r.run_id),
    ['primary-20261028-120000'],
  );
});

test('two projects with the same run_id coexist independently (D1)', () => {
  const d = db();
  const pa = insertProject(d, '/w/a');
  const pb = insertProject(d, '/w/b');
  const run = {
    run_id: 'loop-same',
    kind: 'worktree',
    state: 'exited',
    terminal: { reason: 'completed', at: NOW },
    evidence: { manifest_present: true },
  };
  ingestSnapshot(d, pa, snapshotOf('/w/a', [run]), 'shaA', NOW);
  ingestSnapshot(
    d,
    pb,
    snapshotOf('/w/b', [
      {
        ...run,
        terminal: { reason: 'consecutive_failures', at: NOW },
      },
    ]),
    'shaB',
    NOW,
  );
  assert.equal(getRun(d, pa, 'loop-same')?.state, 'exited_success');
  assert.equal(getRun(d, pb, 'loop-same')?.state, 'exited_failed');
});

test('cursor advances on success, preserves watermark on failure', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  ingestSnapshot(d, pid, snapshotOf('/w/a', []), 'sha1', NOW);
  let cursor = getCursor(d, pid);
  assert.equal(cursor?.last_collected_at, NOW);
  assert.equal(cursor?.last_snapshot_sha256, 'sha1');
  assert.equal(cursor?.consecutive_failures, 0);

  recordCollectionError(d, pid, 'timeout: inspect run exceeded 10000ms', '2026-10-28T12:00:03Z');
  cursor = getCursor(d, pid);
  assert.equal(cursor?.consecutive_failures, 1);
  assert.equal(cursor?.last_collected_at, NOW);

  // Next success resets the failure counter.
  ingestSnapshot(d, pid, snapshotOf('/w/a', []), 'sha2', '2026-10-28T12:00:06Z');
  cursor = getCursor(d, pid);
  assert.equal(cursor?.consecutive_failures, 0);
  assert.equal(cursor?.last_collected_at, '2026-10-28T12:00:06Z');
});

test('collection_errors retention is bounded per project', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  for (let i = 0; i < 60; i++) {
    recordCollectionError(d, pid, `err-${i}`, `2026-10-28T12:${String(i).padStart(2, '0')}:00Z`);
  }
  const n = (
    d
      .prepare('SELECT COUNT(*) AS n FROM collection_errors WHERE project_id = ?')
      .get(pid) as { n: number }
  ).n;
  assert.equal(n, 50);
  const newest = d
    .prepare(
      'SELECT error FROM collection_errors WHERE project_id = ? ORDER BY id DESC LIMIT 1',
    )
    .get(pid) as { error: string };
  assert.equal(newest.error, 'err-59');
});

test('stale flag: failures or old watermark → stale, fresh cursor → not', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: 'loop-x', kind: 'worktree', state: 'running', pid: 1 },
    ]),
    'sha1',
    NOW,
  );
  const fresh = queryRuns(d, { limit: 10, offset: 0 }, new Date(NOW));
  assert.equal(fresh.rows[0].stale, false);

  recordCollectionError(d, pid, 'boom', '2026-10-28T12:00:03Z');
  const stale = queryRuns(d, { limit: 10, offset: 0 }, new Date(NOW));
  assert.equal(stale.rows[0].stale, true);

  const old = queryRuns(
    d,
    { limit: 10, offset: 0 },
    new Date('2026-10-28T12:01:00Z'),
  );
  assert.equal(old.rows[0].stale, true);
});

test('queryRuns watermark reflects freshest successful collection', () => {
  const d = db();
  const pa = insertProject(d, '/w/a');
  insertProject(d, '/w/b');
  ingestSnapshot(d, pa, snapshotOf('/w/a', []), 'sha', NOW);
  const all = queryRuns(d, { limit: 10, offset: 0 });
  assert.equal(all.watermark, NOW);
  const scoped = queryRuns(d, { projectId: pa, limit: 10, offset: 0 });
  assert.equal(scoped.watermark, NOW);
});

test('snapshotSha256 is a stable hex digest', () => {
  assert.match(snapshotSha256('{"a":1}'), /^[0-9a-f]{64}$/);
});
