/**
 * Planning-session enumeration (Plan4-U1): read
 * `<workspace>/.ralph/planning-sessions/<id>/session.json` files —
 * the pending-interaction evidence source (E5). Missing dir → empty
 * set; a malformed session file is skipped and counted, never fatal.
 */

import { existsSync, readdirSync, readFileSync } from 'node:fs';
import { join } from 'node:path';

export interface SessionObservation {
  sessionId: string;
  prompt: string;
  status: string;
  updatedAt: string;
}

export interface SessionScan {
  sessions: SessionObservation[];
  /** Files skipped because JSON/shape was malformed. */
  malformed: number;
}

const SESSIONS_DIR = join('.ralph', 'planning-sessions');
const SESSION_FILE = 'session.json';

export function scanPlanningSessions(workspaceRoot: string): SessionScan {
  const dir = join(workspaceRoot, SESSIONS_DIR);
  const out: SessionObservation[] = [];
  let malformed = 0;
  if (!existsSync(dir)) return { sessions: out, malformed };

  let entries: string[];
  try {
    entries = readdirSync(dir, { withFileTypes: true })
      .filter((e) => e.isDirectory())
      .map((e) => e.name);
  } catch {
    return { sessions: out, malformed };
  }

  for (const sessionId of entries) {
    try {
      const raw = readFileSync(join(dir, sessionId, SESSION_FILE), 'utf8');
      const meta = JSON.parse(raw) as {
        prompt?: string;
        status?: string;
        updatedAt?: string;
      };
      if (typeof meta.status !== 'string' || typeof meta.updatedAt !== 'string') {
        malformed += 1;
        continue;
      }
      out.push({
        sessionId,
        prompt: typeof meta.prompt === 'string' ? meta.prompt : '',
        status: meta.status,
        updatedAt: meta.updatedAt,
      });
    } catch {
      malformed += 1;
    }
  }
  return { sessions: out, malformed };
}
