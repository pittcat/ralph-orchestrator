import { describe, it, beforeEach, afterEach } from 'node:test';
import assert from 'node:assert/strict';
import fs from 'node:fs';
import path from 'node:path';
import {
  tmpDbPath,
  tmpDir,
  makeProjectDir,
  makeApp,
  getSession,
  unsafeHeaders,
} from '../helpers.js';
import {
  importProjectRegistrations,
  registrationFileName,
  removeProjectRegistration,
} from '../../src/collector/registry.js';
import type { MonitorApp } from '../../src/app.js';

function writeRegistration(
  dir: string,
  canonicalPath: string,
  extra: Record<string, unknown> = {},
): string {
  fs.mkdirSync(dir, { recursive: true });
  const file = path.join(dir, registrationFileName(canonicalPath));
  fs.writeFileSync(
    file,
    JSON.stringify({ version: 1, path: canonicalPath, ...extra }),
  );
  return file;
}

function projectCount(app: MonitorApp): number {
  return (
    app.db.prepare('SELECT COUNT(*) AS n FROM projects').get() as { n: number }
  ).n;
}

describe('ralph run auto-registration', () => {
  let app: MonitorApp;
  let registry: string;

  beforeEach(() => {
    app = makeApp(tmpDbPath());
    registry = tmpDir('ralph-monitor-registry-');
  });

  afterEach(async () => {
    await app.app.close();
  });

  it('imports a valid registration file as a project', () => {
    const proj = makeProjectDir();
    writeRegistration(registry, fs.realpathSync(proj));

    const summary = importProjectRegistrations(app.db, registry);
    assert.equal(summary.imported, 1);
    assert.equal(projectCount(app), 1);

    const row = app.db
      .prepare('SELECT canonical_path, display_name FROM projects')
      .get() as { canonical_path: string; display_name: string };
    assert.equal(row.canonical_path, fs.realpathSync(proj));
  });

  it('uses displayName from the file when present', () => {
    const proj = makeProjectDir();
    writeRegistration(registry, fs.realpathSync(proj), {
      displayName: 'custom name',
    });
    importProjectRegistrations(app.db, registry);
    const row = app.db
      .prepare('SELECT display_name FROM projects')
      .get() as { display_name: string };
    assert.equal(row.display_name, 'custom name');
  });

  it('is idempotent — a second import does not duplicate the project', () => {
    const proj = makeProjectDir();
    writeRegistration(registry, fs.realpathSync(proj));
    importProjectRegistrations(app.db, registry);
    const second = importProjectRegistrations(app.db, registry);
    assert.equal(second.imported, 0);
    assert.equal(second.alreadyRegistered, 1);
    assert.equal(projectCount(app), 1);
  });

  it('skips malformed files and non-git directories', () => {
    fs.mkdirSync(registry, { recursive: true });
    fs.writeFileSync(path.join(registry, 'bad.json'), 'not json {');
    const nonGit = tmpDir('ralph-monitor-nongit-');
    writeRegistration(registry, fs.realpathSync(nonGit));
    fs.writeFileSync(
      path.join(registry, 'nopath.json'),
      JSON.stringify({ version: 1 }),
    );

    const summary = importProjectRegistrations(app.db, registry);
    assert.equal(summary.imported, 0);
    assert.equal(summary.skipped, 3);
    assert.equal(projectCount(app), 0);
  });

  it('missing registry dir is a no-op', () => {
    const summary = importProjectRegistrations(
      app.db,
      path.join(registry, 'does-not-exist'),
    );
    assert.equal(summary.files, 0);
    assert.equal(summary.imported, 0);
  });

  it('records an audit row for auto-registered projects', () => {
    const proj = makeProjectDir();
    writeRegistration(registry, fs.realpathSync(proj));
    importProjectRegistrations(app.db, registry);
    const row = app.db
      .prepare("SELECT action FROM audit_log WHERE action = 'project_auto_registered'")
      .get() as { action: string } | undefined;
    assert.equal(row?.action, 'project_auto_registered');
  });

  it('DELETE /projects tombstones the drop file', async () => {
    const proj = makeProjectDir();
    const canonical = fs.realpathSync(proj);
    const file = writeRegistration(registry, canonical);
    importProjectRegistrations(app.db, registry);
    assert.equal(projectCount(app), 1);

    const projectId = (
      app.db.prepare('SELECT id FROM projects').get() as { id: string }
    ).id;
    const session = await getSession(app.app);
    // Point removal at our test registry via env for this request.
    const prev = process.env.RALPH_MONITOR_REGISTRY_DIR;
    process.env.RALPH_MONITOR_REGISTRY_DIR = registry;
    try {
      const res = await app.app.inject({
        method: 'DELETE',
        url: `/api/monitor/v1/projects/${projectId}`,
        headers: unsafeHeaders(session),
      });
      assert.equal(res.statusCode, 204);
    } finally {
      if (prev === undefined) delete process.env.RALPH_MONITOR_REGISTRY_DIR;
      else process.env.RALPH_MONITOR_REGISTRY_DIR = prev;
    }
    assert.equal(projectCount(app), 0);
    assert.equal(fs.existsSync(file), false, 'drop file must be removed');
  });

  it('removeProjectRegistration tolerates a missing file', () => {
    removeProjectRegistration('/nonexistent/path', registry);
  });
});
