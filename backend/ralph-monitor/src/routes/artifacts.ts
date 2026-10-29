import type { FastifyInstance, FastifyReply } from 'fastify';
import type { MonitorDb } from '../db/index.js';
import { sendError } from '../lib/errors.js';
import { getRun, type RunRow } from '../db/run_projection.js';
import {
  indexArtifacts,
  resolveArtifact,
  type RunArtifactContext,
} from '../artifacts/index.js';
import { readArtifact } from '../artifacts/serve.js';

function ctxOf(row: RunRow): RunArtifactContext {
  // Effective workspace: worktree runs live in the worktree; primary
  // runs in repo_root (fallback cwd).
  const workspace = row.worktree_path ?? row.repo_root ?? row.cwd ?? '';
  let descriptor: Record<string, unknown> | null = null;
  if (row.preset_descriptor) {
    try {
      descriptor = JSON.parse(row.preset_descriptor) as Record<string, unknown>;
    } catch {
      descriptor = null;
    }
  }
  return {
    workspace,
    repoRoot: row.repo_root,
    planPath: row.plan_path,
    descriptor,
  };
}

function scope(
  db: MonitorDb,
  machineId: string,
  globalId: string,
  reply: FastifyReply,
): RunRow | null {
  const first = globalId.indexOf(':');
  const second = first === -1 ? -1 : globalId.indexOf(':', first + 1);
  if (first === -1 || second === -1) {
    sendError(reply, 400, 'invalid_run_id', 'expected `<machine_id>:<project_id>:<run_id>`');
    return null;
  }
  const row =
    globalId.slice(0, first) === machineId && globalId.slice(second + 1)
      ? getRun(db, globalId.slice(first + 1, second), globalId.slice(second + 1))
      : undefined;
  if (!row) {
    sendError(reply, 404, 'run_not_found', 'unknown run id');
    return null;
  }
  return row;
}

export function registerArtifactRoutes(
  app: FastifyInstance,
  db: MonitorDb,
  machineId: string,
): void {
  app.get<{ Params: { globalId: string } }>(
    '/api/monitor/v1/runs/:globalId/artifacts',
    async (req, reply) => {
      const row = scope(db, machineId, req.params.globalId, reply);
      if (!row) return;
      return reply.send({ artifacts: indexArtifacts(ctxOf(row)) });
    },
  );

  // Enumerated artifact ids contain `/` (e.g. `review/u1/r.md`) —
  // clients pass them percent-encoded (`review%2Fu1%2Fr.md`).
  app.get<{ Params: { globalId: string; artifactId: string } }>(
    '/api/monitor/v1/runs/:globalId/artifacts/:artifactId/content',
    async (req, reply) => {
      const row = scope(db, machineId, req.params.globalId, reply);
      if (!row) return;
      const artifactId = req.params.artifactId;
      const ctx = ctxOf(row);
      const resolved = resolveArtifact(ctx, artifactId);
      if ('error' in resolved) {
        const code = resolved.error;
        if (code === 'not_found') {
          return sendError(reply, 404, 'artifact_not_found', `artifact ${artifactId} not generated or unknown`);
        }
        if (code === 'outside_allowed_roots') {
          return sendError(reply, 403, 'path_outside_allowed_roots', `artifact ${artifactId} resolves outside allowed roots`);
        }
        return sendError(reply, 415, 'artifact_not_text', `artifact ${artifactId} is not a regular file`);
      }
      const content = readArtifact(resolved.resolved);
      if (!content.ok) {
        if (content.code === 'content_too_large') {
          return sendError(reply, 413, 'content_too_large', `artifact ${artifactId} exceeds 512KiB`);
        }
        return sendError(reply, 415, 'artifact_not_text', `artifact ${artifactId} is binary`);
      }
      return reply.send({
        id: artifactId,
        kind: resolved.slot.kind,
        content: content.content,
        truncated: content.truncated,
      });
    },
  );
}
