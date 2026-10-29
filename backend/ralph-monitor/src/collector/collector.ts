import type { MonitorDb } from '../db/index.js';
import {
  ingestSnapshot,
  recordCollectionError,
  snapshotSha256,
} from '../db/run_projection.js';
import { inspectRun, type InspectResult } from '../ralph/client.js';
import { parseSnapshot } from '../ralph/snapshot.js';
import { scanPlanningSessions } from '../notify/sessions.js';

/**
 * Per-project serial collector (plan D7): one `inspect run` child at
 * a time across all registered projects; a failing project records a
 * collection error + bumps its failure counter without blocking or
 * clearing the others (S9).
 */

export const DEFAULT_POLL_MS = 3_000;

export type InspectFn = (cwd: string) => Promise<InspectResult>;

interface ProjectRow {
  id: string;
  canonical_path: string;
}

export interface CollectSummary {
  projects: number;
  ok: number;
  failed: number;
}

/**
 * One collection round over every registered project. Exported so
 * tests drive rounds deterministically instead of waiting on timers.
 */
export async function collectOnce(
  db: MonitorDb,
  inspect: InspectFn = inspectRun,
  now: () => Date = () => new Date(),
): Promise<CollectSummary> {
  const projects = db
    .prepare('SELECT id, canonical_path FROM projects ORDER BY id')
    .all() as ProjectRow[];

  let ok = 0;
  let failed = 0;
  for (const project of projects) {
    try {
      const result = await inspect(project.canonical_path);
      if (!result.ok) {
        recordCollectionError(
          db,
          project.id,
          `${result.error.kind}: ${result.error.message}`,
          now().toISOString(),
        );
        failed += 1;
        continue;
      }
      const parsed = parseSnapshot(result.json);
      if (!parsed.ok) {
        recordCollectionError(
          db,
          project.id,
          `${parsed.code}: ${parsed.message}`,
          now().toISOString(),
        );
        failed += 1;
        continue;
      }
      // Plan4-U1: planning-session files join the same ingest — the
      // pending-interaction diff lands in the snapshot transaction.
      const sessions = scanPlanningSessions(project.canonical_path);
      ingestSnapshot(
        db,
        project.id,
        parsed.snapshot,
        snapshotSha256(result.raw),
        now().toISOString(),
        { sessions: sessions.sessions },
      );
      ok += 1;
    } catch (err) {
      // Never let one project abort the round.
      recordCollectionError(
        db,
        project.id,
        `internal: ${err instanceof Error ? err.message : String(err)}`,
        now().toISOString(),
      );
      failed += 1;
    }
  }
  return { projects: projects.length, ok, failed };
}

export interface CollectorHandle {
  stop(): void;
}

/**
 * Start the polling loop: an immediate round, then one round every
 * `pollMs` (rounds never overlap — the next is scheduled only after
 * the previous finishes).
 */
export function startCollector(
  db: MonitorDb,
  opts: {
    pollMs?: number;
    inspect?: InspectFn;
    log?: (msg: string, err?: unknown) => void;
  } = {},
): CollectorHandle {
  const pollMs = opts.pollMs ?? readPollMsFromEnv() ?? DEFAULT_POLL_MS;
  const inspect = opts.inspect ?? inspectRun;
  let timer: NodeJS.Timeout | undefined;
  let stopped = false;

  const tick = async () => {
    try {
      await collectOnce(db, inspect);
    } catch (err) {
      opts.log?.('collector round failed', err);
    } finally {
      if (!stopped) timer = setTimeout(tick, pollMs);
    }
  };
  timer = setTimeout(tick, 0);

  return {
    stop() {
      stopped = true;
      if (timer) clearTimeout(timer);
    },
  };
}

function readPollMsFromEnv(): number | undefined {
  const raw = process.env.RALPH_MONITOR_POLL_MS;
  if (raw === undefined) return undefined;
  const n = Number(raw);
  return Number.isFinite(n) && n > 0 ? n : undefined;
}
