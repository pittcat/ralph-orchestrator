import assert from 'node:assert/strict';
import { describe, it, beforeEach, afterEach } from 'node:test';
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
import { readEvents } from '../src/stream/monitor_log.js';
import { readSse, listenEphemeral } from './sse_helper.js';
import type { MonitorApp } from '../src/app.js';
import type { RunSnapshot } from '../src/ralph/snapshot.js';

const NOW = new Date().toISOString();

function obs(ws: string, runId = 'run1', over = {}): Record<string, unknown> {
  return {
    run_id: runId,
    kind: 'primary',
    state: 'running',
    pid: null,
    cwd: ws,
    workspace: ws,
    started_at: NOW,
    evidence: {},
    ...over,
  };
}

function writeEvents(ws: string, name: string, lines: string): void {
  const dir = path.join(ws, '.ralph');
  fs.mkdirSync(dir, { recursive: true });
  fs.writeFileSync(path.join(dir, name), lines);
}

describe('stream + snapshot API', () => {
  let app: MonitorApp;
  let ws: string;
  let projectId: string;
  let base: string;

  beforeEach(async () => {
    app = makeApp(tmpDbPath());
    ws = makeProjectDir();
    projectId = insertProject(app.db, ws);
    base = await listenEphemeral(app.app);
  });
  afterEach(async () => {
    await app.app.close();
    fs.rmSync(ws, { recursive: true, force: true });
  });

  const ingest = (runs: Record<string, unknown>[]) =>
    ingestSnapshot(
      app.db,
      projectId,
      makeSnapshot(ws, runs) as unknown as RunSnapshot,
      'sha',
      NOW,
    );

  it('new run appears as run.created; events ingest → run.events', () => {
    writeEvents(
      ws,
      'events.jsonl',
      JSON.stringify({ ts: NOW, topic: 'work.start' }) + '\n',
    );
    ingest([obs(ws)]);
    const rows = readEvents(app.db, 0);
    const kinds = rows.map((r) => r.kind);
    assert.deepEqual(kinds, ['run.created', 'run.events']);
    const ev = rows[1];
    assert.deepEqual(JSON.parse(ev.payload!), {
      from_seq: 1,
      to_seq: 1,
      count: 1,
    });
    // Same snapshot again → no new stream rows (idempotent diff).
    ingest([obs(ws)]);
    assert.equal(readEvents(app.db, 0).length, 2);
    // Changed state → run.updated; terminal transition also notifies
    // (Plan4-U1) — notification.created lands after run.updated.
    ingest([obs(ws, 'run1', { state: 'exited', terminal: { reason: 'completed', at: NOW } })]);
    const after = readEvents(app.db, 0);
    assert.deepEqual(
      after.slice(-2).map((r) => r.kind),
      ['run.updated', 'notification.created'],
    );
  });

  it('GET /stream replays from cursor then keeps the socket open', async () => {
    ingest([obs(ws)]);
    const frames = await readSse(`${base}/api/monitor/v1/stream?cursor=0`, {
      frames: 1,
    });
    assert.equal(frames[0].event, 'run.created');
    assert.equal(frames[0].id, '1');
  });

  it('cursor skips earlier rows', async () => {
    ingest([obs(ws)]);
    ingest([obs(ws, 'run2')]);
    const frames = await readSse(`${base}/api/monitor/v1/stream?cursor=1`, {
      frames: 1,
    });
    assert.equal(frames[0].event, 'run.created');
    assert.deepEqual(JSON.parse(frames[0].data!), { state: 'running', kind: 'primary' });
    assert.equal(frames[0].id, '2');
  });

  it('invalid cursor → 400 typed error', async () => {
    for (const c of ['-1', 'abc', '1.5']) {
      const res = await fetch(`${base}/api/monitor/v1/stream?cursor=${c}`);
      assert.equal(res.status, 400);
      const body = (await res.json()) as { error: { code: string } };
      assert.equal(body.error.code, 'invalid_cursor');
    }
  });

  it('cursor below retained window → resync_required then close', async () => {
    for (let i = 0; i < 5; i++) ingest([obs(ws, `run${i}`)]);
    // prune all but the last 2 rows to simulate window slide
    app.db
      .prepare(
        `DELETE FROM monitor_events
         WHERE seq < (SELECT MAX(seq) FROM monitor_events) - 2`,
      )
      .run();
    const frames = await readSse(`${base}/api/monitor/v1/stream?cursor=1`, {
      frames: 1,
    });
    assert.equal(frames[0].event, 'resync_required');
    const data = JSON.parse(frames[0].data!);
    assert.equal(data.reason, 'cursor_below_window');
  });

  it('live tail: rows appended after connect arrive on the socket', async () => {
    ingest([obs(ws)]);
    const pending = readSse(`${base}/api/monitor/v1/stream?cursor=0`, {
      frames: 2,
      timeoutMs: 4_000,
    });
    // Let the replay land, then append live.
    await new Promise((r) => setTimeout(r, 400));
    ingest([obs(ws, 'run2')]);
    const frames = await pending;
    assert.equal(frames.length, 2);
    assert.equal(frames[1].event, 'run.created');
    assert.ok(Number(frames[1].id) > Number(frames[0].id));
  });

  it('GET /snapshot → watermark + projects + runs mirror /runs', async () => {
    ingest([obs(ws)]);
    const res = await fetch(`${base}/api/monitor/v1/snapshot`);
    assert.equal(res.status, 200);
    const body = (await res.json()) as {
      watermark: number;
      projects: { id: string; health: { degraded_runs: string[] } }[];
      runs: { globalRunId: string; runId: string }[];
    };
    const maxSeq = readEvents(app.db, 0).at(-1)!.seq;
    assert.equal(body.watermark, maxSeq);
    assert.equal(body.projects.length, 1);
    assert.equal(body.projects[0].id, projectId);
    assert.ok(Array.isArray(body.projects[0].health.degraded_runs));
    assert.equal(body.runs.length, 1);
    assert.equal(body.runs[0].globalRunId, `${app.machineId}:${projectId}:run1`);
    // Consistency with /runs (§16).
    const rest = (
      (await (
        await fetch(`${base}/api/monitor/v1/runs?project=${projectId}`)
      ).json()) as { runs: { globalRunId: string }[] }
    ).runs;
    assert.deepEqual(
      body.runs.map((r) => r.globalRunId),
      rest.map((r) => r.globalRunId),
    );
    // The watermark resumes the stream without replay (no frames).
    const frames = await readSse(
      `${base}/api/monitor/v1/stream?cursor=${body.watermark}`,
      { frames: 1, timeoutMs: 800 },
    );
    assert.equal(frames.length, 0);
  });
});
