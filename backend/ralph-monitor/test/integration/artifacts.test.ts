import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {
  tmpDbPath,
  tmpDir,
  makeProjectDir,
  insertProject,
  makeApp,
} from '../helpers.js';
import { ingestSnapshot } from '../../src/db/run_projection.js';
import type { MonitorApp } from '../../src/app.js';
import type { RunSnapshot } from '../../src/ralph/snapshot.js';

const RUN = 'run_art_1';
let gid: string;

describe('artifacts API', () => {
  let app: MonitorApp;
  let ws: string;
  let outside: string;
  let projectId: string;

  const get = (url: string) => app.app.inject({ method: 'GET', url });

  beforeEach(() => {
    app = makeApp(tmpDbPath());
    ws = makeProjectDir();
    outside = tmpDir('ralph-monitor-outside-');
    projectId = insertProject(app.db, ws);
    gid = `${app.machineId}:${projectId}:${RUN}`;

    // Fixture tree: builtin slot + review enumeration + plan dir + escapes.
    const ralph = path.join(ws, '.ralph');
    fs.mkdirSync(path.join(ralph, 'agent'), { recursive: true });
    fs.mkdirSync(path.join(ralph, 'review', 'u1'), { recursive: true });
    fs.mkdirSync(path.join(ralph, 'post-merge'), { recursive: true });
    fs.writeFileSync(path.join(ralph, 'agent', 'summary.md'), '# Summary\nall good');
    fs.writeFileSync(path.join(ralph, 'review', 'u1', 'report.md'), '# Review U1');
    fs.writeFileSync(path.join(ralph, 'post-merge', 'receipt.json'), '{"ok":true}');
    fs.writeFileSync(path.join(outside, 'secret.txt'), 'sekret');
    fs.symlinkSync(path.join(outside, 'secret.txt'), path.join(ralph, 'review', 'leak.md'));
    fs.writeFileSync(path.join(ralph, 'big.md'), 'x'.repeat(600 * 1024));
    fs.writeFileSync(path.join(ralph, 'bin.md'), Buffer.from([0, 1, 2]));
    fs.mkdirSync(path.join(ws, 'docs', 'plans'), { recursive: true });
    fs.writeFileSync(path.join(ws, 'docs', 'plans', 'p.md'), '# Plan');

    const snap = {
      schema_version: 'run-observation/v1',
      workspace: ws,
      generated_at: '2026-10-28T12:00:00Z',
      runs: [
        {
          run_id: RUN,
          kind: 'primary',
          state: 'running',
          workspace: ws,
          cwd: ws,
          repo_root: ws,
          plan_path: 'docs/plans/p.md',
          started_at: '2026-10-28T11:59:00Z',
          preset: 'ce-executor-pipeline',
          preset_descriptor: {
            schema_version: 'monitor-descriptor/v1',
            stages: [],
            artifacts: [
              { id: 'summary', path_glob: '.ralph/agent/summary.md', kind: 'markdown' },
              { id: 'missing-slot', path_glob: '.ralph/agent/nope.md', kind: 'markdown' },
              { id: 'plan-doc', path_glob: 'docs/plans/p.md', kind: 'markdown' },
            ],
          },
          evidence: { manifest_present: true },
        },
      ],
    } as unknown as RunSnapshot;
    ingestSnapshot(app.db, projectId, snap, 'sha', '2026-10-28T12:00:10Z');
  });

  afterEach(async () => {
    await app.app.close();
    fs.rmSync(ws, { recursive: true, force: true });
    fs.rmSync(outside, { recursive: true, force: true });
  });

  it('index: declared + builtin + enumerated slots with status', async () => {
    const res = await get(`/api/monitor/v1/runs/${gid}/artifacts`);
    assert.equal(res.statusCode, 200);
    const arts = res.json().artifacts as any[];
    const byId = new Map(arts.map((a) => [a.id, a]));
    assert.equal(byId.get('summary').status, 'present');
    assert.equal(byId.get('summary').kind, 'markdown');
    assert.ok(byId.get('summary').size > 0);
    assert.equal(byId.get('missing-slot').status, 'not_generated');
    assert.equal(byId.get('handoff').status, 'not_generated');
    assert.equal(byId.get('plan-doc').status, 'present'); // plan_path dir allowed
    // enumerated one level deep under review/
    assert.ok([...byId.keys()].some((k) => k === 'review/u1'));
    assert.ok([...byId.keys()].some((k) => k === 'post-merge/receipt.json'));
    // symlink escape surfaces as outside_allowed, not silently dropped
    assert.equal(byId.get('review/leak.md').status, 'outside_allowed');
  });

  it('content: present artifact returns text', async () => {
    const res = await get(`/api/monitor/v1/runs/${gid}/artifacts/summary/content`);
    assert.equal(res.statusCode, 200);
    const body = res.json();
    assert.equal(body.id, 'summary');
    assert.equal(body.kind, 'markdown');
    assert.match(body.content, /all good/);
    assert.equal(body.truncated, false);
  });

  it('content: enumerated review artifact via slash id', async () => {
    const res = await get(`/api/monitor/v1/runs/${gid}/artifacts/${encodeURIComponent('review/u1')}/content`);
    // review/u1 is a directory → 415 not_text; file under it isn't one-level enumerated
    assert.equal(res.statusCode, 415);
  });

  it('content: unknown artifact → 404 artifact_not_found', async () => {
    const res = await get(`/api/monitor/v1/runs/${gid}/artifacts/nope-xyz/content`);
    assert.equal(res.statusCode, 404);
    assert.equal(res.json().error.code, 'artifact_not_found');
  });

  it('content: missing slot → 404 (slot not declared has same code)', async () => {
    const res = await get(`/api/monitor/v1/runs/${gid}/artifacts/missing-slot/content`);
    assert.equal(res.statusCode, 404);
  });

  it('content: symlink escape → 403 path_outside_allowed_roots', async () => {
    const res = await get(`/api/monitor/v1/runs/${gid}/artifacts/${encodeURIComponent('review/leak.md')}/content`);
    assert.equal(res.statusCode, 403);
    assert.equal(res.json().error.code, 'path_outside_allowed_roots');
  });

  it('content: >512KiB → 413 content_too_large', async () => {
    const res = await get(`/api/monitor/v1/runs/${gid}/artifacts/big.md/content`);
    assert.equal(res.statusCode, 404); // big.md at .ralph root is not in slots → 404
    // declared slot for it:
    const snap2 = {
      schema_version: 'run-observation/v1',
      workspace: ws,
      generated_at: '2026-10-28T12:01:00Z',
      runs: [
        {
          run_id: RUN,
          kind: 'primary',
          state: 'running',
          workspace: ws,
          cwd: ws,
          repo_root: ws,
          plan_path: 'docs/plans/p.md',
          started_at: '2026-10-28T11:59:00Z',
          preset: 'x',
          preset_descriptor: {
            schema_version: 'monitor-descriptor/v1',
            artifacts: [{ id: 'big', path_glob: '.ralph/big.md', kind: 'markdown' }],
          },
          evidence: {},
        },
      ],
    } as unknown as RunSnapshot;
    ingestSnapshot(app.db, projectId, snap2, 'sha2', '2026-10-28T12:01:00Z');
    const res2 = await get(`/api/monitor/v1/runs/${gid}/artifacts/big/content`);
    assert.equal(res2.statusCode, 413);
    assert.equal(res2.json().error.code, 'content_too_large');
  });

  it('content: binary file → 415 artifact_not_text', async () => {
    // declare bin slot
    const snap2 = {
      schema_version: 'run-observation/v1',
      workspace: ws,
      generated_at: '2026-10-28T12:02:00Z',
      runs: [
        {
          run_id: RUN,
          kind: 'primary',
          state: 'running',
          workspace: ws,
          cwd: ws,
          repo_root: ws,
          preset: 'x',
          preset_descriptor: {
            schema_version: 'monitor-descriptor/v1',
            artifacts: [{ id: 'bin', path_glob: '.ralph/bin.md', kind: 'markdown' }],
          },
          evidence: {},
        },
      ],
    } as unknown as RunSnapshot;
    ingestSnapshot(app.db, projectId, snap2, 'sha3', '2026-10-28T12:02:00Z');
    const res = await get(`/api/monitor/v1/runs/${gid}/artifacts/bin/content`);
    assert.equal(res.statusCode, 415);
  });

  it('unknown run → 404; foreign machine → 404; bad gid → 400', async () => {
    assert.equal((await get(`/api/monitor/v1/runs/${app.machineId}:${projectId}:nope/artifacts`)).statusCode, 404);
    assert.equal((await get(`/api/monitor/v1/runs/bad/artifacts`)).statusCode, 400);
  });

  // ── HTML preview (monitor auto-register plan): docs/report/*.html is
  // enumerated as kind 'html' and served rendered under a CSP sandbox. ──

  it('preview: docs/report html enumerated and served as text/html', async () => {
    fs.mkdirSync(path.join(ws, 'docs', 'report'), { recursive: true });
    fs.writeFileSync(
      path.join(ws, 'docs', 'report', 'diag.html'),
      '<html><body><h1>report</h1></body></html>',
    );
    const list = await get(`/api/monitor/v1/runs/${gid}/artifacts`);
    const byId = new Map(
      (list.json().artifacts as any[]).map((a) => [a.id, a]),
    );
    const entry = byId.get('docs/report/diag.html') as any;
    assert.equal(entry.status, 'present');
    assert.equal(entry.kind, 'html');

    const res = await get(
      `/api/monitor/v1/runs/${gid}/artifacts/${encodeURIComponent('docs/report/diag.html')}/preview`,
    );
    assert.equal(res.statusCode, 200);
    assert.match(res.headers['content-type'] ?? '', /text\/html/);
    assert.match(res.body, /<h1>report<\/h1>/);
    assert.match(
      res.headers['content-security-policy'] ?? '',
      /^sandbox allow-scripts allow-popups$/,
    );
    assert.equal(res.headers['x-content-type-options'], 'nosniff');
  });

  it('preview: non-html artifact → 415 preview_not_html', async () => {
    const res = await get(
      `/api/monitor/v1/runs/${gid}/artifacts/summary/preview`,
    );
    assert.equal(res.statusCode, 415);
    assert.equal(res.json().error.code, 'preview_not_html');
  });

  it('preview: unknown artifact → 404', async () => {
    const res = await get(
      `/api/monitor/v1/runs/${gid}/artifacts/nope-xyz/preview`,
    );
    assert.equal(res.statusCode, 404);
    assert.equal(res.json().error.code, 'artifact_not_found');
  });

  it('preview: html escape via declared slot → 403', async () => {
    fs.writeFileSync(path.join(outside, 'evil.html'), '<html>evil</html>');
    const snap2 = {
      schema_version: 'run-observation/v1',
      workspace: ws,
      generated_at: '2026-10-28T12:03:00Z',
      runs: [
        {
          run_id: RUN,
          kind: 'primary',
          state: 'running',
          workspace: ws,
          cwd: ws,
          repo_root: ws,
          preset: 'x',
          preset_descriptor: {
            schema_version: 'monitor-descriptor/v1',
            artifacts: [
              { id: 'evil', path_glob: `${outside}/evil.html`, kind: 'html' },
            ],
          },
          evidence: {},
        },
      ],
    } as unknown as RunSnapshot;
    ingestSnapshot(app.db, projectId, snap2, 'sha4', '2026-10-28T12:03:00Z');
    const res = await get(
      `/api/monitor/v1/runs/${gid}/artifacts/evil/preview`,
    );
    assert.equal(res.statusCode, 403);
    assert.equal(res.json().error.code, 'path_outside_allowed_roots');
  });
});
