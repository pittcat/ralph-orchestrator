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

test('non-evidenced terminal recovers to running on later live observation', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  // Mid-restart observation: dead pid, no terminal evidence.
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: 'loop-x', kind: 'worktree', state: 'unknown', pid: 111 },
    ]),
    'sha1',
    NOW,
  );
  // Dead pid verified → abnormal unrecorded exit, but recoverable
  // because no terminal evidence was ever recorded.
  assert.equal(getRun(d, pid, 'loop-x')?.state, 'exited_failed');
  // Relaunched run observed live — must not be rejected.
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: 'loop-x', kind: 'worktree', state: 'running', pid: 222 },
    ]),
    'sha2',
    '2026-10-28T12:01:00Z',
  );
  const row = getRun(d, pid, 'loop-x');
  assert.equal(row?.state, 'running');
  assert.equal(row?.pid, 222);
  assert.equal(row?.last_observed_at, '2026-10-28T12:01:00Z');
  const errs = d
    .prepare('SELECT error FROM collection_errors WHERE project_id = ?')
    .all(pid) as { error: string }[];
  assert.equal(errs.length, 0);
});

test('exited_merged still refuses regression to running', () => {
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
        terminal: { reason: 'completed', at: NOW, merge_commit: 'abc' },
      },
    ]),
    'sha1',
    NOW,
  );
  assert.equal(getRun(d, pid, 'loop-x')?.state, 'exited_merged');
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: 'loop-x', kind: 'worktree', state: 'running', pid: 7 },
    ]),
    'sha2',
    '2026-10-28T12:01:00Z',
  );
  assert.equal(getRun(d, pid, 'loop-x')?.state, 'exited_merged');
  const errs = d
    .prepare('SELECT error FROM collection_errors WHERE project_id = ?')
    .all(pid) as { error: string }[];
  assert.equal(errs.length, 1);
  assert.match(errs[0].error, /state_conflict/);
});

test('dismissed run re-imported when observation reports live running', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  d.prepare(
    'INSERT INTO dismissed_runs (project_id, run_id, dismissed_at) VALUES (?, ?, ?)',
  ).run(pid, 'loop-x', NOW);

  // Stale evidence keeps the run suppressed.
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: 'loop-x', kind: 'worktree', state: 'unknown', pid: 111 },
    ]),
    'sha2',
    '2026-10-28T12:00:30Z',
  );
  assert.equal(getRun(d, pid, 'loop-x'), undefined);

  // `running` is only emitted with a verified live pid — the run was
  // relaunched, so it un-dismisses and lands normally.
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: 'loop-x', kind: 'worktree', state: 'running', pid: 222 },
    ]),
    'sha3',
    '2026-10-28T12:01:00Z',
  );
  const row = getRun(d, pid, 'loop-x');
  assert.equal(row?.state, 'running');
  assert.equal(row?.pid, 222);
  const stillDismissed = d
    .prepare(
      'SELECT run_id FROM dismissed_runs WHERE project_id = ? AND run_id = ?',
    )
    .get(pid, 'loop-x');
  assert.equal(stillDismissed, undefined);
});

test('dismissed run with exited observation stays suppressed', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  d.prepare(
    'INSERT INTO dismissed_runs (project_id, run_id, dismissed_at) VALUES (?, ?, ?)',
  ).run(pid, 'loop-x', NOW);
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      {
        run_id: 'loop-x',
        kind: 'worktree',
        state: 'exited',
        terminal: { reason: 'completed', at: NOW },
      },
    ]),
    'sha1',
    '2026-10-28T12:00:30Z',
  );
  assert.equal(getRun(d, pid, 'loop-x'), undefined);
  const stillDismissed = d
    .prepare(
      'SELECT run_id FROM dismissed_runs WHERE project_id = ? AND run_id = ?',
    )
    .get(pid, 'loop-x');
  assert.ok(stillDismissed);
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

// ── absent-run sweep ─────────────────────────────────────────────
// A run missing from the snapshot lost every evidence source. The
// projection must not freeze its last state — the stored pid is
// re-probed (kill(0)) to decide.

test('absent run with dead stored pid → exited_failed + run.updated', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: 'loop-x', kind: 'worktree', state: 'running', pid: 111 },
    ]),
    'sha1',
    NOW,
  );
  assert.equal(getRun(d, pid, 'loop-x')?.state, 'running');

  // Next round the run is gone from the snapshot entirely
  // (manifest deleted, registry pruned, worktree removed).
  ingestSnapshot(d, pid, snapshotOf('/w/a', []), 'sha2', '2026-10-28T12:01:00Z', {
    pidAlive: () => false,
  });
  const row = getRun(d, pid, 'loop-x');
  assert.equal(row?.state, 'exited_failed');
  assert.equal(row?.last_observed_at, '2026-10-28T12:01:00Z');
  const evidence = JSON.parse(row!.state_evidence) as {
    snapshotState: string;
    absentFromSnapshot?: boolean;
  };
  assert.equal(evidence.snapshotState, 'running');
  assert.equal(evidence.absentFromSnapshot, true);
  const events = d
    .prepare(
      `SELECT kind, payload FROM monitor_events
       WHERE project_id = ? AND run_id = 'loop-x' ORDER BY seq`,
    )
    .all(pid) as { kind: string; payload: string }[];
  const updated = events
    .filter((e) => e.kind === 'run.updated')
    .map((e) => JSON.parse(e.payload) as Record<string, unknown>);
  assert.ok(
    updated.some(
      (p) =>
        p.reason === 'absent_from_snapshot' &&
        JSON.stringify(p.changed).includes('exited_failed'),
    ),
    'expected run.updated absent_from_snapshot event',
  );
});

test('absent run with live stored pid keeps its state (no fabricated death)', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: 'loop-x', kind: 'worktree', state: 'running', pid: 111 },
    ]),
    'sha1',
    NOW,
  );
  ingestSnapshot(d, pid, snapshotOf('/w/a', []), 'sha2', '2026-10-28T12:01:00Z', {
    pidAlive: () => true,
  });
  // Evidence vanished under a live process — keep the last state.
  assert.equal(getRun(d, pid, 'loop-x')?.state, 'running');
});

test('legacy exited_unknown row with dead stored pid → exited_failed', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  // Rows persisted by the old mapping (dead pid, no terminal →
  // exited_unknown) are remediated by the sweep once their evidence
  // is gone — no data migration needed.
  d.prepare(
    `INSERT INTO runs (project_id, run_id, kind, state, state_evidence,
                       last_observed_at, pid)
     VALUES (?, 'loop-x', 'worktree', 'exited_unknown', ?, ?, 111)`,
  ).run(pid, JSON.stringify({ snapshotState: 'unknown', pid: 111 }), NOW);
  ingestSnapshot(d, pid, snapshotOf('/w/a', []), 'sha2', '2026-10-28T12:01:00Z', {
    pidAlive: () => false,
  });
  assert.equal(getRun(d, pid, 'loop-x')?.state, 'exited_failed');
});

test('absent run with no stored pid → exited_unknown (nothing verifiable)', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: 'loop-x', kind: 'unknown', state: 'unknown' },
    ]),
    'sha1',
    NOW,
  );
  assert.equal(getRun(d, pid, 'loop-x')?.state, 'exited_unknown');
  ingestSnapshot(d, pid, snapshotOf('/w/a', []), 'sha2', '2026-10-28T12:01:00Z', {
    pidAlive: () => {
      throw new Error('pidAlive must not run for pid-less rows');
    },
  });
  assert.equal(getRun(d, pid, 'loop-x')?.state, 'exited_unknown');
});

test('terminal-evidenced rows are never swept (absent ≠ regressed)', () => {
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
        pid: 111,
        terminal: { reason: 'completed', at: NOW },
      },
    ]),
    'sha1',
    NOW,
  );
  assert.equal(getRun(d, pid, 'loop-x')?.state, 'exited_success');
  ingestSnapshot(d, pid, snapshotOf('/w/a', []), 'sha2', '2026-10-28T12:01:00Z', {
    pidAlive: () => {
      throw new Error('pidAlive must not run for terminal rows');
    },
  });
  assert.equal(getRun(d, pid, 'loop-x')?.state, 'exited_success');
});

test('absent run relaunched under same id still recovers to running', () => {
  const d = db();
  const pid = insertProject(d, '/w/a');
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: 'loop-x', kind: 'worktree', state: 'running', pid: 111 },
    ]),
    'sha1',
    NOW,
  );
  // Evidence vanishes, pid dead → swept to exited_failed (no
  // terminal evidence recorded, so regression guard stays open).
  ingestSnapshot(d, pid, snapshotOf('/w/a', []), 'sha2', '2026-10-28T12:01:00Z', {
    pidAlive: () => false,
  });
  assert.equal(getRun(d, pid, 'loop-x')?.state, 'exited_failed');
  // Relaunch re-creates the manifest — the run is observed live again.
  ingestSnapshot(
    d,
    pid,
    snapshotOf('/w/a', [
      { run_id: 'loop-x', kind: 'worktree', state: 'running', pid: 222 },
    ]),
    'sha3',
    '2026-10-28T12:02:00Z',
    { pidAlive: () => false },
  );
  assert.equal(getRun(d, pid, 'loop-x')?.state, 'running');
});
