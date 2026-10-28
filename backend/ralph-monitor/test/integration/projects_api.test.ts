import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import { test } from 'node:test';
import type { FastifyInstance } from 'fastify';
import {
  getSession,
  gitInit,
  makeApp,
  tmpDbPath,
  tmpDir,
  unsafeHeaders,
} from '../helpers.js';

async function registerProject(
  app: FastifyInstance,
  dir: string,
  displayName?: string,
) {
  const session = await getSession(app);
  return app.inject({
    method: 'POST',
    url: '/api/monitor/v1/projects',
    headers: unsafeHeaders(session),
    payload: { path: dir, ...(displayName ? { displayName } : {}) },
  });
}

test('register → 201, canonical path dedup → 409 with existing id', async () => {
  const { app } = makeApp();
  const dir = tmpDir();
  gitInit(dir);

  const res = await registerProject(app, dir);
  assert.equal(res.statusCode, 201, res.body);
  const { project } = res.json();
  assert.equal(project.canonicalPath, fs.realpathSync(dir));
  assert.equal(project.displayName, path.basename(dir));
  assert.match(project.id, /^proj_/);

  // Same path through a symlink alias must dedup to 409.
  const alias = path.join(tmpDir(), 'alias');
  fs.symlinkSync(dir, alias);
  const dup = await registerProject(app, alias);
  assert.equal(dup.statusCode, 409, dup.body);
  const err = dup.json().error;
  assert.equal(err.code, 'project_exists');
  assert.equal(err.details.projectId, project.id);

  await app.close();
});

test('register invalid paths → 422 invalid_path', async () => {
  const { app } = makeApp();
  const dir = tmpDir();

  // nonexistent
  let res = await registerProject(app, path.join(dir, 'missing'));
  assert.equal(res.statusCode, 422);
  assert.equal(res.json().error.code, 'invalid_path');

  // file, not a directory
  const file = path.join(dir, 'a-file');
  fs.writeFileSync(file, 'x');
  res = await registerProject(app, file);
  assert.equal(res.statusCode, 422);
  assert.equal(res.json().error.code, 'invalid_path');

  // traversal
  res = await registerProject(app, '/tmp/../etc');
  assert.equal(res.statusCode, 422);

  // a real directory that is not a git repo → not_git_repo (S8)
  res = await registerProject(app, dir);
  assert.equal(res.statusCode, 422);
  assert.equal(res.json().error.code, 'not_git_repo');
  assert.match(res.json().error.message, /git/);

  // malformed body shape
  const session = await getSession(app);
  res = await app.inject({
    method: 'POST',
    url: '/api/monitor/v1/projects',
    headers: unsafeHeaders(session),
    payload: { path: 42 },
  });
  assert.equal(res.statusCode, 400);
  assert.equal(res.json().error.code, 'invalid_body');

  await app.close();
});

test('GET list/detail/DELETE lifecycle', async () => {
  const { app } = makeApp();
  const dir = tmpDir();
  gitInit(dir);

  const created = await registerProject(app, dir, 'my project');
  const { project } = created.json();
  assert.equal(project.displayName, 'my project');

  const list = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/projects',
  });
  assert.equal(list.statusCode, 200);
  assert.equal(list.json().projects.length, 1);

  const detail = await app.inject({
    method: 'GET',
    url: `/api/monitor/v1/projects/${project.id}`,
  });
  assert.equal(detail.statusCode, 200);
  assert.equal(detail.json().project.id, project.id);

  const missing = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/projects/proj_nope',
  });
  assert.equal(missing.statusCode, 404);
  assert.equal(missing.json().error.code, 'project_not_found');

  const session = await getSession(app);
  const del = await app.inject({
    method: 'DELETE',
    url: `/api/monitor/v1/projects/${project.id}`,
    headers: unsafeHeaders(session),
  });
  assert.equal(del.statusCode, 204);

  const after = await app.inject({
    method: 'GET',
    url: `/api/monitor/v1/projects/${project.id}`,
  });
  assert.equal(after.statusCode, 404);

  await app.close();
});

test('restart persists projects on the same db file', async () => {
  const dbPath = tmpDbPath();
  const dir = tmpDir();
  gitInit(dir);

  let { app } = makeApp(dbPath);
  await registerProject(app, dir);
  await app.close();

  // New app instance over the same db file sees the same set.
  ({ app } = makeApp(dbPath));
  const res = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/projects',
  });
  assert.equal(res.statusCode, 200);
  const projects = res.json().projects;
  assert.equal(projects.length, 1);
  assert.equal(projects[0].canonicalPath, fs.realpathSync(dir));
  await app.close();
});

test('security: cross-origin POST 403, missing CSRF 403, GET exempt', async () => {
  const { app } = makeApp();
  const dir = tmpDir();
  gitInit(dir);
  const session = await getSession(app);

  // Cross Origin POST → 403 forbidden_origin.
  let res = await app.inject({
    method: 'POST',
    url: '/api/monitor/v1/projects',
    headers: {
      ...unsafeHeaders(session),
      origin: 'http://evil.example.com',
    },
    payload: { path: dir },
  });
  assert.equal(res.statusCode, 403);
  assert.equal(res.json().error.code, 'forbidden_origin');

  // Unsafe method without CSRF header → 403 csrf.
  res = await app.inject({
    method: 'POST',
    url: '/api/monitor/v1/projects',
    headers: { 'content-type': 'application/json' },
    payload: { path: dir },
  });
  assert.equal(res.statusCode, 403);
  assert.equal(res.json().error.code, 'csrf');

  // DELETE without CSRF → 403.
  res = await app.inject({
    method: 'DELETE',
    url: '/api/monitor/v1/projects/proj_x',
  });
  assert.equal(res.statusCode, 403);

  // GET with a hostile Origin still passes (safe method exempt).
  res = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/projects',
    headers: { origin: 'http://evil.example.com' },
  });
  assert.equal(res.statusCode, 200);

  // Host outside the allowlist is rejected even on GET.
  res = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/projects',
    headers: { host: 'evil.example.com' },
  });
  assert.equal(res.statusCode, 403);
  assert.equal(res.json().error.code, 'forbidden_host');

  await app.close();
});

test('health endpoint reports ok + machine id', async () => {
  const { app, machineId } = makeApp();
  const res = await app.inject({
    method: 'GET',
    url: '/api/monitor/v1/health',
  });
  assert.equal(res.statusCode, 200);
  assert.equal(res.json().ok, true);
  assert.equal(res.json().machine_id, machineId);
  await app.close();
});
