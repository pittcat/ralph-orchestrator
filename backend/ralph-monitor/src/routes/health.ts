import type { FastifyInstance } from 'fastify';
import type { MonitorDb } from '../db/index.js';
import { sendError } from '../lib/errors.js';
import {
  DEFAULT_STALE_MS,
  getCursor,
  getRun,
  latestCollectionError,
  type RunRow,
} from '../db/run_projection.js';
import {
  classifyRunHealth,
  isDegraded,
  type ActivityDetail,
  type ClassifyRun,
  type RunHealth,
} from '../health/classify.js';
import { posixPidCheck, type PidCheck } from '../health/pid_check.js';

/** `RALPH_MONITOR_SILENT_MS` overrides the 5-minute silent window. */
function silentMsFromEnv(): number {
  const raw = process.env.RALPH_MONITOR_SILENT_MS;
  const n = Number(raw);
  return raw !== undefined && Number.isFinite(n) && n > 0
    ? n
    : 5 * 60 * 1000;
}

function classifyRun(
  db: MonitorDb,
  row: RunRow,
  pidCheck: PidCheck,
): RunHealth {
  const activity = db
    .prepare(
      `SELECT seq, phase, at, detail FROM run_activity
       WHERE project_id = ? AND run_id = ?
       ORDER BY generation DESC, seq DESC LIMIT 10`,
    )
    .all(row.project_id, row.run_id) as {
    seq: number;
    phase: 'start' | 'end';
    at: string | null;
    detail: string | null;
  }[];
  const rows: ActivityDetail[] = activity.map((r) => ({
    seq_in_run: r.seq,
    phase: r.phase,
    ts: r.at,
    detail: r.detail
      ? (JSON.parse(r.detail) as Record<string, unknown>)
      : null,
  }));
  const cursor = getCursor(db, row.project_id);
  const run: ClassifyRun = {
    pid: row.pid,
    started_at: row.started_at,
    terminal: row.terminal,
    last_event_at: row.last_event_at,
  };
  return classifyRunHealth({
    run,
    activity: rows,
    collector: {
      consecutive_failures: cursor?.consecutive_failures ?? 0,
      last_collected_at: cursor?.last_collected_at ?? null,
      error: latestCollectionError(db, row.project_id),
      staleMs: DEFAULT_STALE_MS,
    },
    pidCheck,
    now: new Date(),
    silentMs: silentMsFromEnv(),
  });
}

/**
 * Health routes (Plan1-U3 project collector health; Plan3-U2 adds the
 * run-level four-layer endpoint + `degraded_runs` on the project
 * endpoint). The browser-connection layer is client-side by design
 * (plan §3) and never enters this API.
 */
export function registerHealthRoutes(
  app: FastifyInstance,
  db: MonitorDb,
  machineId: string,
  pidCheck: PidCheck = posixPidCheck,
): void {
  app.get<{ Params: { id: string } }>(
    '/api/monitor/v1/projects/:id/health',
    async (req, reply) => {
      const project = db
        .prepare('SELECT id FROM projects WHERE id = ?')
        .get(req.params.id);
      if (!project) {
        return sendError(
          reply,
          404,
          'project_not_found',
          'unknown project id',
        );
      }
      const cursor = getCursor(db, req.params.id);
      const degraded_runs = (
        db
          .prepare('SELECT * FROM runs WHERE project_id = ?')
          .all(req.params.id) as RunRow[]
      )
        .filter((r) => isDegraded(classifyRun(db, r, pidCheck)))
        .map((r) => r.run_id);
      return reply.send({
        collector: {
          status: (cursor?.consecutive_failures ?? 0) > 0
            ? 'error'
            : 'ok',
          ...(cursor?.consecutive_failures
            ? { error: latestCollectionError(db, req.params.id) }
            : {}),
          consecutive_failures: cursor?.consecutive_failures ?? 0,
          last_collected_at: cursor?.last_collected_at ?? null,
        },
        degraded_runs,
      });
    },
  );

  app.get<{ Params: { globalId: string } }>(
    '/api/monitor/v1/runs/:globalId/health',
    async (req, reply) => {
      const id = req.params.globalId;
      const first = id.indexOf(':');
      const second = first === -1 ? -1 : id.indexOf(':', first + 1);
      if (first === -1 || second === -1) {
        return sendError(
          reply,
          400,
          'invalid_run_id',
          'expected `<machine_id>:<project_id>:<run_id>`',
        );
      }
      const machine = id.slice(0, first);
      const projectId = id.slice(first + 1, second);
      const runId = id.slice(second + 1);
      if (machine !== machineId || !runId) {
        return sendError(
          reply,
          404,
          'run_not_found',
          'unknown run id for this monitor',
        );
      }
      const row = getRun(db, projectId, runId);
      if (!row) {
        return sendError(reply, 404, 'run_not_found', 'unknown run id');
      }
      const health = classifyRun(db, row, pidCheck);
      return reply.send({
        run_id: runId,
        global_run_id: id,
        ...health,
      });
    },
  );
}
