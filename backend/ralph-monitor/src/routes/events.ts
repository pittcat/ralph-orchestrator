import type { FastifyInstance, FastifyReply } from 'fastify';
import type { MonitorDb } from '../db/index.js';
import { sendError } from '../lib/errors.js';
import { getCursor, getRun, type RunRow } from '../db/run_projection.js';
import { deriveProgress } from '../events/progress.js';
import { deriveGraph } from '../events/graph.js';

const DEFAULT_LIMIT = 200;
const MAX_LIMIT = 1000;

interface ScopedRun {
  row: RunRow & { stale: boolean };
  projectId: string;
  runId: string;
}

/**
 * `global_run_id = machine:project:run` — first two colons delimit;
 * run_id may itself contain colons (same rule as routes/runs.ts).
 */
function scopeRun(
  db: MonitorDb,
  machineId: string,
  globalId: string,
  reply: FastifyReply,
): ScopedRun | null {
  const first = globalId.indexOf(':');
  const second = first === -1 ? -1 : globalId.indexOf(':', first + 1);
  if (first === -1 || second === -1) {
    sendError(reply, 400, 'invalid_run_id', 'expected `<machine_id>:<project_id>:<run_id>`');
    return null;
  }
  const machine = globalId.slice(0, first);
  const projectId = globalId.slice(first + 1, second);
  const runId = globalId.slice(second + 1);
  const row = machine === machineId && runId ? getRun(db, projectId, runId) : undefined;
  if (!row) {
    sendError(reply, 404, 'run_not_found', 'unknown run id');
    return null;
  }
  return { row, projectId, runId };
}

function jsonColumn(raw: string | null): Record<string, unknown> | null {
  if (!raw) return null;
  try {
    const v = JSON.parse(raw);
    return typeof v === 'object' && v !== null ? (v as Record<string, unknown>) : null;
  } catch {
    return null;
  }
}

export function registerEventRoutes(
  app: FastifyInstance,
  db: MonitorDb,
  machineId: string,
): void {
  app.get<{
    Params: { globalId: string };
    Querystring: Record<string, string | undefined>;
  }>('/api/monitor/v1/runs/:globalId/events', async (req, reply) => {
    const scoped = scopeRun(db, machineId, req.params.globalId, reply);
    if (!scoped) return;
    const { row, projectId, runId } = scoped;

    const cursor = req.query.cursor === undefined ? 0 : Number(req.query.cursor);
    const limit =
      req.query.limit === undefined ? DEFAULT_LIMIT : Number(req.query.limit);
    if (!Number.isInteger(cursor) || cursor < 0 ||
        !Number.isInteger(limit) || limit < 1 || limit > MAX_LIMIT) {
      return sendError(reply, 400, 'invalid_filter',
        `cursor must be a non-negative integer, limit 1..${MAX_LIMIT}`);
    }

    const where = ['project_id = ?', 'run_id = ?', 'seq > ?'];
    const params: unknown[] = [projectId, runId, cursor];
    const { hat, topic, q } = req.query;
    if (hat) { where.push('hat = ?'); params.push(hat); }
    if (topic) { where.push('topic = ?'); params.push(topic); }
    if (q) { where.push('(payload LIKE ? OR topic LIKE ? OR hat LIKE ?)'); params.push(`%${q}%`, `%${q}%`, `%${q}%`); }

    const events = db
      .prepare(
        `SELECT seq, ts, iteration, hat, topic, payload, truncated
         FROM run_events WHERE ${where.join(' AND ')}
         ORDER BY seq ASC LIMIT ?`,
      )
      .all(...params, limit + 1) as {
        seq: number; ts: string | null; iteration: number | null;
        hat: string | null; topic: string | null;
        payload: string | null; truncated: number;
      }[];
    const hasMore = events.length > limit;
    const page = hasMore ? events.slice(0, limit) : events;

    return reply.send({
      events: page.map((e) => ({ ...e, truncated: !!e.truncated })),
      next_cursor: page.length ? page[page.length - 1].seq : cursor,
      has_more: hasMore,
      watermark: getCursor(db, projectId)?.last_collected_at ?? null,
      evidence_timestamps: {
        last_event_at: row.last_event_at,
        last_observed_at: row.last_observed_at,
        terminal_at: row.terminal
          ? (JSON.parse(row.terminal) as { at?: string }).at ?? null
          : null,
        collected_at: getCursor(db, projectId)?.last_collected_at ?? null,
      },
    });
  });

  app.get<{ Params: { globalId: string } }>(
    '/api/monitor/v1/runs/:globalId/progress',
    async (req, reply) => {
      const scoped = scopeRun(db, machineId, req.params.globalId, reply);
      if (!scoped) return;
      const { row, projectId, runId } = scoped;
      const rows = db
        .prepare(
          `SELECT seq, ts, iteration, hat, topic, payload
           FROM run_events WHERE project_id = ? AND run_id = ?
           ORDER BY seq ASC`,
        )
        .all(projectId, runId) as Parameters<typeof deriveProgress>[0];
      return reply.send(
        deriveProgress(rows, jsonColumn(row.preset_descriptor), {
          started_at: row.started_at,
          terminal: row.terminal,
        }),
      );
    },
  );

  app.get<{ Params: { globalId: string } }>(
    '/api/monitor/v1/runs/:globalId/graph',
    async (req, reply) => {
      const scoped = scopeRun(db, machineId, req.params.globalId, reply);
      if (!scoped) return;
      const { row, projectId, runId } = scoped;
      const rows = db
        .prepare(
          `SELECT seq, ts, iteration, hat, topic, payload
           FROM run_events WHERE project_id = ? AND run_id = ?
           ORDER BY seq ASC`,
        )
        .all(projectId, runId) as Parameters<typeof deriveGraph>[0];
      return reply.send(
        deriveGraph(
          rows,
          jsonColumn(row.preset_descriptor),
          jsonColumn(row.scheduler),
          row.terminal !== null,
        ),
      );
    },
  );
}
