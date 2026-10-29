import type { FastifyInstance } from 'fastify';
import type { MonitorDb } from '../db/index.js';
import { sendError } from '../lib/errors.js';
import {
  getCursor,
  getRun,
  parseStates,
  queryRuns,
  type RunRow,
} from '../db/run_projection.js';

const DEFAULT_LIMIT = 50;
const MAX_LIMIT = 200;

interface RunRowWithStale extends RunRow {
  stale: boolean;
}

export function toRunJson(
  machineId: string,
  row: RunRowWithStale,
): Record<string, unknown> {
  return {
    globalRunId: `${machineId}:${row.project_id}:${row.run_id}`,
    projectId: row.project_id,
    runId: row.run_id,
    kind: row.kind,
    state: row.state,
    stateEvidence: JSON.parse(row.state_evidence || '{}'),
    cwd: row.cwd,
    repoRoot: row.repo_root,
    worktreePath: row.worktree_path,
    promptPreview: row.prompt_preview,
    promptSha256: row.prompt_sha256,
    preset: row.preset,
    planPath: row.plan_path,
    startedAt: row.started_at,
    terminal: row.terminal ? JSON.parse(row.terminal) : null,
    lastEventAt: row.last_event_at,
    lastObservedAt: row.last_observed_at,
    manifestPath: row.manifest_path,
    stale: row.stale,
  };
}

function parseLimitOffset(query: Record<string, unknown>): {
  limit: number;
  offset: number;
} | null {
  const limit =
    query.limit === undefined ? DEFAULT_LIMIT : Number(query.limit);
  const offset =
    query.offset === undefined ? 0 : Number(query.offset);
  if (
    !Number.isInteger(limit) ||
    limit < 1 ||
    limit > MAX_LIMIT ||
    !Number.isInteger(offset) ||
    offset < 0
  ) {
    return null;
  }
  return { limit, offset };
}

export function registerRunRoutes(
  app: FastifyInstance,
  db: MonitorDb,
  machineId: string,
): void {
  app.get<{ Querystring: Record<string, string | undefined> }>(
    '/api/monitor/v1/runs',
    async (req, reply) => {
      const { project, state, q, sort, limit, offset } = req.query;

      if (project !== undefined) {
        const exists = db
          .prepare('SELECT 1 FROM projects WHERE id = ?')
          .get(project);
        if (!exists) {
          return sendError(
            reply,
            404,
            'project_not_found',
            'unknown project id',
          );
        }
      }

      let states;
      if (state !== undefined) {
        states = parseStates(state);
        if (!states) {
          return sendError(
            reply,
            400,
            'invalid_filter',
            `state must be a comma-separated list of monitor states`,
          );
        }
      }
      if (sort !== undefined && sort !== 'startedAt' && sort !== '-startedAt') {
        return sendError(
          reply,
          400,
          'invalid_filter',
          'sort must be `startedAt` or `-startedAt`',
        );
      }
      const page = parseLimitOffset({ limit, offset });
      if (!page) {
        return sendError(
          reply,
          400,
          'invalid_filter',
          `limit must be an integer 1..${MAX_LIMIT}, offset a non-negative integer`,
        );
      }

      const result = queryRuns(db, {
        projectId: project,
        states,
        q,
        sort: sort as 'startedAt' | '-startedAt' | undefined,
        limit: page.limit,
        offset: page.offset,
      });
      return reply.send({
        runs: result.rows.map((r) => toRunJson(machineId, r)),
        total: result.total,
        watermark: result.watermark,
      });
    },
  );

  app.get<{ Params: { globalId: string } }>(
    '/api/monitor/v1/runs/:globalId',
    async (req, reply) => {
      // global_run_id = machine_id:project_id:run_id — the first two
      // colons are delimiters; run_id itself may contain colons.
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
      const terminal = row.terminal
        ? (JSON.parse(row.terminal) as { at?: string })
        : null;
      let presetDescriptor: Record<string, unknown> | null = null;
      if (row.preset_descriptor) {
        try {
          presetDescriptor = JSON.parse(row.preset_descriptor) as Record<string, unknown>;
        } catch {
          presetDescriptor = null;
        }
      }
      return reply.send({
        run: {
          ...toRunJson(machineId, row),
          presetDescriptor,
          evidenceTimestamps: {
            lastEventAt: row.last_event_at,
            lastObservedAt: row.last_observed_at,
            terminalAt: terminal?.at ?? null,
            collectedAt:
              getCursor(db, projectId)?.last_collected_at ?? null,
          },
        },
      });
    },
  );

}
