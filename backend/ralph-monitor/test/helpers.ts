import { randomUUID } from 'node:crypto';
import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import { fileURLToPath } from 'node:url';
import type { FastifyInstance } from 'fastify';
import { buildApp, type MonitorApp } from '../src/app.js';
import { buildChildEnv, inspectRun, type InspectResult } from '../src/ralph/client.js';
import { collectOnce, type CollectSummary } from '../src/collector/collector.js';
import type { MonitorDb } from '../src/db/index.js';

export function tmpDir(prefix = 'ralph-monitor-test-'): string {
  return fs.mkdtempSync(path.join(os.tmpdir(), prefix));
}

export function tmpDbPath(prefix = 'ralph-monitor-test-'): string {
  return path.join(tmpDir(prefix), 'monitor.db');
}

export function makeApp(dbPath = tmpDbPath()): MonitorApp {
  return buildApp({ dbPath });
}

export interface Session {
  cookie: string;
  csrf: string;
}

/// Acquire a monitor session: any GET issues the sid+csrf cookie pair
/// the double-submit scheme replays on unsafe methods.
export async function getSession(app: FastifyInstance): Promise<Session> {
  const res = await app.inject({ method: 'GET', url: '/api/monitor/v1/health' });
  if (res.statusCode !== 200) {
    throw new Error(`session GET failed: ${res.statusCode}`);
  }
  const setCookies = res.headers['set-cookie'];
  const raw = Array.isArray(setCookies)
    ? setCookies
    : setCookies
      ? [setCookies]
      : [];
  const jar: Record<string, string> = {};
  for (const c of raw) {
    const [pair] = c.split(';');
    const eq = pair.indexOf('=');
    jar[pair.slice(0, eq).trim()] = pair.slice(eq + 1).trim();
  }
  if (!jar.monitor_sid || !jar.monitor_csrf) {
    throw new Error(`session cookies missing: ${JSON.stringify(raw)}`);
  }
  return {
    cookie: `monitor_sid=${jar.monitor_sid}; monitor_csrf=${jar.monitor_csrf}`,
    csrf: jar.monitor_csrf,
  };
}

/// Headers for an unsafe-method request under the double-submit
/// scheme. `content-type` is left to inject so empty-body DELETEs are
/// not misread as empty JSON payloads.
export function unsafeHeaders(session: Session): Record<string, string> {
  return {
    cookie: session.cookie,
    'x-monitor-csrf': session.csrf,
  };
}

export function gitInit(dir: string): void {
  fs.mkdirSync(path.join(dir, '.git'), { recursive: true });
}

/// Create a fresh tmp dir pre-initialized as a git repo (registrable).
export function makeProjectDir(prefix = 'ralph-monitor-proj-'): string {
  const dir = tmpDir(prefix);
  gitInit(dir);
  return dir;
}

export async function registerProject(
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

// ── fake-ralph fixture plumbing (plan U3 fixture strategy: real
// executable on a test PATH, never a mocked spawn) ──

export const FAKE_RALPH_DIR = fileURLToPath(
  new URL('./fixtures/fake-ralph', import.meta.url),
);

/**
 * Minimal child env: fake-ralph dir first on PATH plus the system
 * defaults the shell script needs. Everything else is scrubbed by the
 * client allowlist anyway.
 */
export function fakeRalphEnv(
  extra: NodeJS.ProcessEnv = {},
): NodeJS.ProcessEnv {
  return buildChildEnv({
    PATH: `${FAKE_RALPH_DIR}:${path.dirname(process.execPath)}:/usr/bin:/bin`,
    HOME: os.homedir(),
    ...extra,
  });
}

export function fakeInspect(
  cwd: string,
  opts: Parameters<typeof inspectRun>[1] = {},
): Promise<InspectResult> {
  return inspectRun(cwd, { env: fakeRalphEnv(), ...opts });
}

/// Write `.monitor-fixture/` control files inside a project dir.
export function writeMonitorFixture(
  projectDir: string,
  fixture: { snapshot?: unknown; fail?: string },
): void {
  const dir = path.join(projectDir, '.monitor-fixture');
  fs.mkdirSync(dir, { recursive: true });
  fs.rmSync(path.join(dir, 'fail'), { force: true });
  fs.rmSync(path.join(dir, 'snapshot.json'), { force: true });
  if (fixture.fail !== undefined) {
    fs.writeFileSync(path.join(dir, 'fail'), fixture.fail);
  }
  if (fixture.snapshot !== undefined) {
    fs.writeFileSync(
      path.join(dir, 'snapshot.json'),
      typeof fixture.snapshot === 'string'
        ? fixture.snapshot
        : JSON.stringify(fixture.snapshot),
    );
  }
}

/// Build a valid run-observation/v1 payload for a workspace.
export function makeSnapshot(
  workspace: string,
  runs: Record<string, unknown>[],
  generatedAt = '2026-10-28T12:00:00Z',
): Record<string, unknown> {
  return {
    schema_version: 'run-observation/v1',
    workspace,
    generated_at: generatedAt,
    runs,
  };
}

/// One collection round against the fake-ralph PATH.
export function collectOnceFake(db: MonitorDb): Promise<CollectSummary> {
  return collectOnce(db, (cwd) => fakeInspect(cwd));
}

/// Insert a project row directly (collector tests bypass the API).
export function insertProject(
  db: MonitorDb,
  canonicalPath: string,
  displayName = 'test project',
): string {
  const id = `proj_${randomUUID()}`;
  db.prepare(
    'INSERT INTO projects (id, canonical_path, display_name, created_at) VALUES (?, ?, ?, ?)',
  ).run(id, canonicalPath, displayName, new Date().toISOString());
  return id;
}
