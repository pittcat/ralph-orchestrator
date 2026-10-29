import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {
  tmpDbPath,
  makeProjectDir,
  insertProject,
  makeApp,
  type Session,
} from './helpers.js';
import { ingestSnapshot } from '../src/db/run_projection.js';
import type { MonitorApp } from '../src/app.js';
import type { RunSnapshot } from '../src/ralph/snapshot.js';

const RUN = 'run_ev_1';
let projectId: string;
let gid: string;

function snapshot(ws: string): RunSnapshot {
  return {
    schema_version: 'run-observation/v1',
    workspace: ws,
    generated_at: '2026-10-28T12:00:00Z',
    runs: [
      {
        run_id: RUN,
        kind: 'primary',
        state: 'running',
        pid: 1234,
        workspace: ws,
        cwd: ws,
        started_at: '2026-10-28T11:59:00Z',
        preset: 'ce-executor-pipeline',
        preset_descriptor: {
          schema_version: 'monitor-descriptor/v1',
          stages: [
            { id: 'execute', label: 'Execute', topics: ['work.start', 'work.done'] },
            { id: 'review', label: 'Review', topics: ['review.start', 'review.done'] },
          ],
          dag: { enabled: true, unit_topic_prefix: 'forge.unit.' },
        },
        evidence: {
          manifest_present: true,
          manifest_corrupt: false,
          registry_present: true,
          lock_present: true,
        },
      },
    ] as RunSnapshot['runs'],
  };
}

describe('events/progress/graph API', () => {
  let app: MonitorApp;
  let ws: string;
  let session: Session | undefined;
  let eventsFile: string;

  beforeEach(async () => {
    app = makeApp(tmpDbPath());
    ws = makeProjectDir();
    projectId = insertProject(app.db, ws);
    gid = `${app.machineId}:${projectId}:${RUN}`;
    const ralph = path.join(ws, '.ralph');
    fs.mkdirSync(ralph, { recursive: true });
    eventsFile = path.join(ralph, 'events.jsonl');
    fs.writeFileSync(eventsFile,
      JSON.stringify({ ts: '2026-10-28T12:00:01Z', topic: 'work.start', source: 'executor', iteration: 1, payload: '{"x":1}' }) + '\n' +
      JSON.stringify({ ts: '2026-10-28T12:00:02Z', topic: 'forge.unit.ready', source: 'forge', payload: '{"unit":"U1"}' }) + '\n' +
      JSON.stringify({ ts: '2026-10-28T12:00:03Z', topic: 'forge.unit.executed', source: 'forge', payload: '{"unit":"U1"}' }) + '\n' +
      JSON.stringify({ ts: '2026-10-28T12:00:04Z', topic: 'forge.unit.ready', source: 'forge', payload: '{"unit":"U2"}' }) + '\n' +
      JSON.stringify({ ts: '2026-10-28T12:00:05Z', topic: 'work.done', source: 'executor', iteration: 1, payload: '{}' }) + '\n' +
      JSON.stringify({ ts: '2026-10-28T12:00:06Z', topic: 'review.start', source: 'reviewer' }) + '\n');
    // Collector transaction: snapshot ingest → run row + events.
    ingestSnapshot(app.db, projectId, snapshot(ws), 'sha-test', '2026-10-28T12:00:10Z');
  });

  afterEach(async () => {
    await app.app.close();
    fs.rmSync(ws, { recursive: true, force: true });
  });

  const get = (url: string) => app.app.inject({ method: 'GET', url });

  it('GET /runs/:id/events returns projected rows ordered by seq', async () => {
    const res = await get(`/api/monitor/v1/runs/${gid}/events`);
    assert.equal(res.statusCode, 200);
    const body = res.json();
    assert.equal(body.events.length, 6);
    assert.equal(body.events[0].seq, 1);
    assert.equal(body.events[0].topic, 'work.start');
    assert.equal(body.events[1].hat, 'forge');
    assert.ok(body.watermark);
    assert.ok(body.evidence_timestamps.last_event_at);
    assert.equal(body.evidence_timestamps.last_event_at, '2026-10-28T12:00:06Z');
  });

  it('cursor pagination: next_cursor + has_more', async () => {
    const p1 = (await get(`/api/monitor/v1/runs/${gid}/events?limit=2`)).json();
    assert.equal(p1.events.length, 2);
    assert.equal(p1.has_more, true);
    const p2 = (await get(`/api/monitor/v1/runs/${gid}/events?limit=2&cursor=${p1.next_cursor}`)).json();
    assert.equal(p2.events[0].seq, 3);
  });

  it('filters: hat / topic / q', async () => {
    const byHat = (await get(`/api/monitor/v1/runs/${gid}/events?hat=forge`)).json();
    assert.equal(byHat.events.length, 3);
    const byTopic = (await get(`/api/monitor/v1/runs/${gid}/events?topic=forge.unit.ready`)).json();
    assert.equal(byTopic.events.length, 2);
    const byQ = (await get(`/api/monitor/v1/runs/${gid}/events?q=review`)).json();
    assert.equal(byQ.events.length, 1);
  });

  it('invalid cursor → 400; unknown gid → 404', async () => {
    assert.equal((await get(`/api/monitor/v1/runs/${gid}/events?cursor=-1`)).statusCode, 400);
    assert.equal((await get(`/api/monitor/v1/runs/bad-id/events`)).statusCode, 400);
    assert.equal(
      (await get(`/api/monitor/v1/runs/${app.machineId}:${projectId}:nope/events`)).statusCode,
      404,
    );
    // foreign machine id → 404
    assert.equal(
      (await get(`/api/monitor/v1/runs/other:${projectId}:${RUN}/events`)).statusCode,
      404,
    );
  });

  it('GET /runs/:id/progress derives stage + units', async () => {
    const res = await get(`/api/monitor/v1/runs/${gid}/progress`);
    assert.equal(res.statusCode, 200);
    const body = res.json();
    assert.equal(body.stage, 'review'); // review.start seen → entered stage 2
    assert.equal(body.stage_label, 'Review');
    assert.equal(body.stage_history.length, 2);
    assert.equal(body.stage_history[0].stage, 'execute');
    assert.equal(body.current_hat, 'reviewer');
    assert.equal(body.iteration, 1);
    assert.equal(body.units.done, 1); // U1 executed
    assert.equal(body.units.total, 2); // U1+U2 distinct keys
    assert.equal(body.units.source, 'forge-events');
    assert.equal(body.denominator, 'stages');
    assert.ok(body.elapsed_ms >= 0);
  });

  it('GET /runs/:id/graph derives unit nodes from forge.unit.*', async () => {
    const res = await get(`/api/monitor/v1/runs/${gid}/graph`);
    assert.equal(res.statusCode, 200);
    const body = res.json();
    assert.equal(body.source, 'forge-events');
    assert.equal(body.nodes.length, 2);
    assert.equal(body.nodes[0].unit, 'U1');
    assert.equal(body.nodes[0].status, 'executed');
    assert.equal(body.nodes[1].unit, 'U2');
    assert.equal(body.nodes[1].status, 'running'); // run live, only ready seen
    assert.equal(body.illegal_transitions, 0);
  });

  it('run row last_event_at reflects ledger', () => {
    const row = app.db.prepare('SELECT last_event_at FROM runs WHERE run_id=?').get(RUN) as any;
    assert.equal(row.last_event_at, '2026-10-28T12:00:06Z');
  });

  it('events of one run are not attributed to another (window)', async () => {
    // second run with a later started_at → earlier events excluded
    const ws2 = ws;
    const snap2 = snapshot(ws2);
    snap2.runs = [{ ...snap2.runs[0], run_id: 'run_later', started_at: '2026-10-28T13:00:00Z' }];
    ingestSnapshot(app.db, projectId, snap2, 'sha-2', '2026-10-28T13:00:10Z');
    const res = await get(`/api/monitor/v1/runs/${app.machineId}:${projectId}:run_later/events`);
    assert.equal(res.json().events.length, 0);
  });
});
