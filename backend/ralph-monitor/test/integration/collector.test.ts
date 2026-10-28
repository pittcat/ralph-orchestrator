import assert from 'node:assert/strict';
import { test } from 'node:test';
import { getRun } from '../../src/db/run_projection.js';
import {
  collectOnceFake,
  insertProject,
  makeApp,
  makeProjectDir,
  makeSnapshot,
  writeMonitorFixture,
} from '../helpers.js';

const T = '2026-10-28T12:00:00Z';

function exitedRun(
  runId: string,
  overrides: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    run_id: runId,
    kind: 'primary',
    state: 'exited',
    pid: 4242,
    cwd: '/w',
    repo_root: '/w',
    prompt_preview: 'same prompt',
    prompt_sha256: 'a'.repeat(64),
    preset: 'builtin:ce-executor-lite',
    started_at: '2026-10-28T11:59:00Z',
    terminal: { reason: 'completed', at: T },
    manifest: `.ralph/runs/${runId}.manifest.json`,
    evidence: { manifest_present: true, registry_present: true },
    ...overrides,
  };
}

test('S1: two projects with identically-named runs stay distinct', async () => {
  const { app, db } = makeApp();
  const dirA = makeProjectDir();
  const dirB = makeProjectDir();
  const idA = insertProject(db, dirA, 'A');
  const idB = insertProject(db, dirB, 'B');
  const run = exitedRun('loop-20261028-113000-a1b2c3');
  writeMonitorFixture(dirA, { snapshot: makeSnapshot(dirA, [run]) });
  writeMonitorFixture(dirB, { snapshot: makeSnapshot(dirB, [run]) });

  const summary = await collectOnceFake(db);
  assert.equal(summary.projects, 2);
  assert.equal(summary.ok, 2);
  assert.equal(summary.failed, 0);

  const a = getRun(db, idA, 'loop-20261028-113000-a1b2c3');
  const b = getRun(db, idB, 'loop-20261028-113000-a1b2c3');
  assert.ok(a && b);
  assert.equal(a.project_id, idA);
  assert.equal(b.project_id, idB);
  assert.notEqual(a.project_id, b.project_id);
  await app.close();
});

test('S4: short-lived run discovered via manifest evidence + terminal', async () => {
  const { app, db } = makeApp();
  const dir = makeProjectDir();
  const pid = insertProject(db, dir);
  writeMonitorFixture(dir, {
    snapshot: makeSnapshot(dir, [
      exitedRun('primary-20261028-115900', {
        kind: 'primary',
        terminal: { reason: 'completed', at: T },
      }),
    ]),
  });

  await collectOnceFake(db);
  const row = getRun(db, pid, 'primary-20261028-115900');
  assert.ok(row);
  assert.equal(row.state, 'exited_success');
  const evidence = JSON.parse(row.state_evidence) as {
    manifestPresent: boolean;
    terminalReason: string;
  };
  assert.equal(evidence.manifestPresent, true);
  assert.equal(evidence.terminalReason, 'completed');
  await app.close();
});

test('S5/S6: three-way state split — success / merged / unknown', async () => {
  const { app, db } = makeApp();
  const dir = makeProjectDir();
  const pid = insertProject(db, dir);
  writeMonitorFixture(dir, {
    snapshot: makeSnapshot(dir, [
      exitedRun('primary-x'),
      exitedRun('loop-y', {
        kind: 'worktree',
        worktree_path: `${dir}/.worktrees/loop-y`,
        terminal: { reason: 'completed', at: T },
        evidence: {
          manifest_present: true,
          registry_present: true,
          merge_state: 'merged',
        },
      }),
      {
        run_id: 'primary-z',
        kind: 'primary',
        state: 'unknown',
        pid: 99999999,
        prompt_preview: 'legacy',
        started_at: '2026-10-28T10:00:00Z',
        evidence: { lock_present: true },
      },
    ]),
  });

  await collectOnceFake(db);
  assert.equal(getRun(db, pid, 'primary-x')?.state, 'exited_success');
  assert.equal(getRun(db, pid, 'loop-y')?.state, 'exited_merged');
  assert.equal(getRun(db, pid, 'primary-z')?.state, 'exited_unknown');
  await app.close();
});

test('kind + identity fields survive the projection', async () => {
  const { app, db } = makeApp();
  const dir = makeProjectDir();
  const pid = insertProject(db, dir);
  writeMonitorFixture(dir, {
    snapshot: makeSnapshot(dir, [
      exitedRun('primary-20261028-115900', { kind: 'primary' }),
      exitedRun('loop-20261028-113000-abc', {
        kind: 'worktree',
        worktree_path: `${dir}/.worktrees/loop-20261028-113000-abc`,
      }),
    ]),
  });

  await collectOnceFake(db);
  const primary = getRun(db, pid, 'primary-20261028-115900');
  const worktree = getRun(db, pid, 'loop-20261028-113000-abc');
  assert.equal(primary?.kind, 'primary');
  assert.equal(worktree?.kind, 'worktree');
  assert.ok(worktree?.worktree_path?.includes('.worktrees'));
  assert.ok(primary?.run_id.startsWith('primary-'));
  await app.close();
});
