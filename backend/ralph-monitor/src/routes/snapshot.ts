import type { FastifyInstance } from 'fastify';
import type { MonitorDb } from '../db/index.js';
import {
  getCursor,
  isProjectStale,
  type RunRow,
} from '../db/run_projection.js';
import { streamBounds } from '../stream/monitor_log.js';
import { projectHealth } from './health.js';
import { toRunJson } from './runs.js';
import type { PidCheck } from '../health/pid_check.js';
import { posixPidCheck } from '../health/pid_check.js';

/**
 * `GET /api/monitor/v1/snapshot` — one-shot full state for clients
 * (re)joining the stream (Plan3-U3): `watermark` is the current
 * monitor_events max seq — pass it as `?cursor=` to continue live
 * without a gap; `runs`/`projects`/`health` mirror the REST surfaces
 * of the same data source (§16 consistency diff).
 */
export function registerSnapshotRoutes(
  app: FastifyInstance,
  db: MonitorDb,
  machineId: string,
  pidCheck: PidCheck = posixPidCheck,
): void {
  app.get('/api/monitor/v1/snapshot', async (_req, reply) => {
    const now = new Date();
    const projects = (
      db
        .prepare(
          'SELECT id, canonical_path, display_name, created_at FROM projects ORDER BY id',
        )
        .all() as {
        id: string;
        canonical_path: string;
        display_name: string;
        created_at: string;
      }[]
    ).map((p) => ({ ...p, health: projectHealth(db, p.id, pidCheck) }));

    const runs = (db.prepare('SELECT * FROM runs').all() as RunRow[]).map(
      (r) =>
        toRunJson(machineId, {
          ...r,
          stale: isProjectStale(getCursor(db, r.project_id), now),
        }),
    );

    return reply.send({
      watermark: streamBounds(db)?.max ?? 0,
      generated_at: now.toISOString(),
      projects,
      runs,
    });
  });
}
