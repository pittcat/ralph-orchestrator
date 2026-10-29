import assert from 'node:assert/strict';
import { describe, it, beforeEach, afterEach, after } from 'node:test';
import { spawn } from 'node:child_process';
import fs from 'node:fs';
import path from 'node:path';
import {
  tmpDbPath,
  makeProjectDir,
  insertProject,
  makeApp,
  makeSnapshot,
} from './helpers.js';
import { ingestSnapshot } from '../src/db/run_projection.js';
import type { MonitorApp } from '../src/app.js';
import type { RunSnapshot } from '../src/ralph/snapshot.js';

const NOW = new Date().toISOString();

function writeActivity(ws: string, runId: string, lines: string): void {
  const dir = path.join(ws, '.ralph', 'runs');
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, `${runId}.activity.jsonl`), lines);
}

const line = (o: Record<string, unknown>) => JSON.stringify(o) + '\n';

function runObs(
  ws: string,
  over: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    run_id: 'run1',
    kind: 'primary',
    state: 'running',
    pid: null,
    cwd: ws,
    workspace: ws,
    started_at: NOW,
    evidence: { manifest_present: true },
    ...over,
  };
}

function snapshot(ws: string, runs: Record<string, unknown>[]): RunSnapshot {
  return makeSnapshot(ws, runs) as unknown as RunSnapshot;
}

describe('health API', () => {
  const children: ReturnType<typeof spawn>[] = [];
  after(() => {
    for (const c of children) c.kill('SIGKILL');
  });

  let app: MonitorApp;
  let ws: string;
  let projectId: string;
  const gid = (runId: string) => `${app.machineId}:${projectId}:${runId}`;

  beforeEach(async () => {
    app = makeApp(tmpDbPath());
    ws = makeProjectDir();
    projectId = insertProject(app.db, ws);
  });
  afterEach(async () => {
    await app.app.close();
    fs.rmSync(ws, { recursive: true, force: true });
  });

  it('GET /runs/:gid/health → 404 unknown run', async () => {
    const res = await app.app.inject({
      method: 'GET',
      url: `/api/monitor/v1/runs/${gid('nope')}/health`,
    });
    assert.equal(res.statusCode, 404);
    assert.equal(res.json().error.code, 'run_not_found');
  });

  it('live pid + fresh evidence → runtime running, backend unknown', async () => {
    const child = spawn('sleep', ['60']);
    children.push(child);
    ingestSnapshot(
      app.db,
      projectId,
      snapshot(ws, [runObs(ws, { pid: child.pid })]),
      'sha',
      NOW,
    );
    app.db
      .prepare('UPDATE runs SET last_event_at = ? WHERE project_id = ?')
      .run(NOW, projectId);
    const res = await app.app.inject({
      method: 'GET',
      url: `/api/monitor/v1/runs/${gid('run1')}/health`,
    });
    assert.equal(res.statusCode, 200);
    const body = res.json();
    assert.equal(body.run_id, 'run1');
    assert.equal(body.layers.collector.status, 'ok');
    assert.equal(body.layers.backend.status, 'unknown');
    assert.equal(body.terminal_converged, false);
    // Linux: live pid + matching start → running; no /proc →
    // pid_unverified. Both are non-degraded happy paths.
    assert.ok(
      ['running', 'pid_unverified'].includes(body.layers.runtime.status),
      `unexpected runtime status ${body.layers.runtime.status}`,
    );
  });

  it('activity end ok → backend.ok with last_end_at', async () => {
    const child = spawn('sleep', ['60']);
    children.push(child);
    writeActivity(
      ws,
      'run1',
      line({ ts: NOW, seq_in_run: 1, hat: 'executor', backend: 'claude', phase: 'start' }) +
        line({
          ts: NOW,
          seq_in_run: 2,
          phase: 'end',
          exit_code: 0,
          timed_out: false,
          is_error: false,
          duration_ms: 10,
        }),
    );
    ingestSnapshot(
      app.db,
      projectId,
      snapshot(ws, [runObs(ws, { pid: child.pid })]),
      'sha',
      NOW,
    );
    const body = (
      await app.app.inject({
        method: 'GET',
        url: `/api/monitor/v1/runs/${gid('run1')}/health`,
      })
    ).json();
    assert.equal(body.layers.backend.status, 'ok');
    assert.equal(body.layers.backend.last_end_at, NOW);
  });

  it('timed-out end → backend.timeout', async () => {
    const child = spawn('sleep', ['60']);
    children.push(child);
    writeActivity(
      ws,
      'run1',
      line({ ts: NOW, seq_in_run: 1, phase: 'start' }) +
        line({
          ts: NOW,
          seq_in_run: 2,
          phase: 'end',
          exit_code: null,
          timed_out: true,
          is_error: true,
        }),
    );
    ingestSnapshot(
      app.db,
      projectId,
      snapshot(ws, [runObs(ws, { pid: child.pid })]),
      'sha',
      NOW,
    );
    const body = (
      await app.app.inject({
        method: 'GET',
        url: `/api/monitor/v1/runs/${gid('run1')}/health`,
      })
    ).json();
    assert.equal(body.layers.backend.status, 'timeout');
  });

  it('terminal run → runtime exited + terminal_converged', async () => {
    ingestSnapshot(
      app.db,
      projectId,
      snapshot(ws, [
        runObs(ws, {
          state: 'exited',
          terminal: { reason: 'completed', at: NOW },
        }),
      ]),
      'sha',
      NOW,
    );
    const body = (
      await app.app.inject({
        method: 'GET',
        url: `/api/monitor/v1/runs/${gid('run1')}/health`,
      })
    ).json();
    assert.equal(body.layers.runtime.status, 'exited');
    assert.equal(body.terminal_converged, true);
  });

  it('GET /projects/:id/health → collector object + degraded_runs', async () => {
    // dead pid + no terminal → degraded run.
    ingestSnapshot(
      app.db,
      projectId,
      snapshot(ws, [
        runObs(ws, { pid: 4_000_000, run_id: 'run1' }),
        runObs(ws, {
          run_id: 'run2',
          state: 'exited',
          terminal: { reason: 'completed', at: NOW },
        }),
      ]),
      'sha',
      NOW,
    );
    const res = await app.app.inject({
      method: 'GET',
      url: `/api/monitor/v1/projects/${projectId}/health`,
    });
    assert.equal(res.statusCode, 200);
    const body = res.json();
    assert.equal(body.collector.status, 'ok');
    assert.equal(body.collector.consecutive_failures, 0);
    assert.ok(Array.isArray(body.degraded_runs));
    assert.ok(body.degraded_runs.includes('run1'));
    assert.ok(!body.degraded_runs.includes('run2'));
  });
});
