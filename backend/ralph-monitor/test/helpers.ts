import fs from 'node:fs';
import os from 'node:os';
import path from 'node:path';
import type { FastifyInstance } from 'fastify';
import { buildApp, type MonitorApp } from '../src/app.js';

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
