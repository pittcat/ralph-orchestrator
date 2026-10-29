import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { describe, it } from 'node:test';
import { registerStatic, registerStaticMissing } from '../src/static/spa.js';
import { makeApp, tmpDir } from './helpers.js';

function makeDist(): string {
  const dir = tmpDir('ralph-monitor-dist-');
  fs.writeFileSync(path.join(dir, 'index.html'), '<html>SPA</html>');
  fs.writeFileSync(path.join(dir, 'app.js'), 'console.log(1)');
  fs.mkdirSync(path.join(dir, 'assets'));
  fs.writeFileSync(path.join(dir, 'assets/main.css'), 'body{}');
  return dir;
}

describe('static SPA hosting (Plan5-U4)', () => {
  it('serves index at / and deep routes fall back to index', async () => {
    const { app } = makeApp();
    registerStatic(app, makeDist());
    const root = await app.inject({ method: 'GET', url: '/' });
    assert.equal(root.statusCode, 200);
    assert.match(root.body, /SPA/);
    assert.equal(root.headers['cache-control'], 'no-store');

    const deep = await app.inject({
      method: 'GET',
      url: '/monitor/runs/machine/proj/r1',
    });
    assert.equal(deep.statusCode, 200);
    assert.match(deep.body, /SPA/);
    await app.close();
  });

  it('serves real static assets directly', async () => {
    const { app } = makeApp();
    registerStatic(app, makeDist());
    const res = await app.inject({ method: 'GET', url: '/app.js' });
    assert.equal(res.statusCode, 200);
    assert.match(res.body, /console\.log/);
    const css = await app.inject({ method: 'GET', url: '/assets/main.css' });
    assert.equal(css.statusCode, 200);
    await app.close();
  });

  it('unknown /api paths return JSON 404, not the SPA', async () => {
    const { app } = makeApp();
    registerStatic(app, makeDist());
    const res = await app.inject({ method: 'GET', url: '/api/nope' });
    assert.equal(res.statusCode, 404);
    const body = res.json();
    assert.equal(body.error.code, 'not_found');
    await app.close();
  });

  it('missing dist dir degrades to API-only hint page', async () => {
    const { app } = makeApp();
    registerStaticMissing(app, '/nonexistent/dist');
    const res = await app.inject({ method: 'GET', url: '/' });
    assert.equal(res.statusCode, 200);
    assert.match(res.body, /bundle not found/);
    const api = await app.inject({
      method: 'GET',
      url: '/api/monitor/v1/health',
    });
    assert.equal(api.statusCode, 200);
    await app.close();
  });

  it('long global run ids still match /runs/:globalId (find-my-way maxParamLength)', async () => {
    const { app, machineId } = makeApp();
    registerStatic(app, makeDist());
    // ~130-char param — past find-my-way's default maxParamLength of 100.
    const gid = `${machineId}:proj_${'a'.repeat(33)}:loop-fake-op-${'b'.repeat(36)}`;
    assert.ok(gid.length > 100);
    const res = await app.inject({
      method: 'GET',
      url: `/api/monitor/v1/runs/${encodeURIComponent(gid)}`,
    });
    assert.equal(res.statusCode, 404);
    // Routing reached the runs handler (run_not_found) rather than being
    // swallowed by the SPA wildcard (unknown endpoint).
    assert.equal(res.json().error.code, 'run_not_found');
    await app.close();
  });

  it('path traversal cannot escape the static root', async () => {
    const dist = makeDist();
    const { app } = makeApp();
    registerStatic(app, dist);
    const res = await app.inject({
      method: 'GET',
      url: '/%2e%2e/package.json',
    });
    // Blocked by the static plugin → neutralized into the SPA
    // fallback; it must never serve a file outside the root.
    assert.match(res.body, /SPA/);
    await app.close();
  });
});
