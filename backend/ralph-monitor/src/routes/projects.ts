import { randomUUID } from 'node:crypto';
import path from 'node:path';
import type { FastifyInstance } from 'fastify';
import { z } from 'zod';
import type { MonitorDb } from '../db/index.js';
import { sendError } from '../lib/errors.js';
import { isGitRepo, normalizeUserPath } from '../lib/paths.js';

interface ProjectRow {
  id: string;
  canonical_path: string;
  display_name: string;
  created_at: string;
}

export interface ProjectJson {
  id: string;
  canonicalPath: string;
  displayName: string;
  createdAt: string;
}

function toJson(row: ProjectRow): ProjectJson {
  return {
    id: row.id,
    canonicalPath: row.canonical_path,
    displayName: row.display_name,
    createdAt: row.created_at,
  };
}

const registerBody = z
  .object({
    path: z.string().min(1).max(4096),
    displayName: z.string().min(1).max(200).optional(),
  })
  .strict();

export function registerProjectRoutes(
  app: FastifyInstance,
  db: MonitorDb,
): void {
  const byCanonical = db.prepare(
    'SELECT id, canonical_path, display_name, created_at FROM projects WHERE canonical_path = ?',
  );
  const byId = db.prepare(
    'SELECT id, canonical_path, display_name, created_at FROM projects WHERE id = ?',
  );
  const insert = db.prepare(
    'INSERT INTO projects (id, canonical_path, display_name, created_at) VALUES (?, ?, ?, ?)',
  );
  const remove = db.prepare('DELETE FROM projects WHERE id = ?');
  const audit = db.prepare(
    'INSERT INTO audit_log (at, project_id, action, details) VALUES (?, ?, ?, ?)',
  );
  const list = db.prepare(
    'SELECT id, canonical_path, display_name, created_at FROM projects ORDER BY created_at, id',
  );

  app.post('/api/monitor/v1/projects', async (req, reply) => {
    const parsed = registerBody.safeParse(req.body);
    if (!parsed.success) {
      return sendError(reply, 400, 'invalid_body', 'request body is not a valid project registration', {
        issues: parsed.error.issues.map((i) => ({
          path: i.path.join('.'),
          message: i.message,
        })),
      });
    }

    const normalized = normalizeUserPath(parsed.data.path);
    if (!normalized.ok) {
      return sendError(reply, 422, 'invalid_path', normalized.message);
    }
    if (!isGitRepo(normalized.canonical)) {
      return sendError(
        reply,
        422,
        'not_git_repo',
        `目录不是 git 仓库（缺少 .git）: ${parsed.data.path}`,
      );
    }

    const existing = byCanonical.get(normalized.canonical) as
      | ProjectRow
      | undefined;
    if (existing) {
      return sendError(
        reply,
        409,
        'project_exists',
        'project already registered for this path',
        { projectId: existing.id },
      );
    }

    const project: ProjectJson = {
      id: `proj_${randomUUID()}`,
      canonicalPath: normalized.canonical,
      displayName:
        parsed.data.displayName ?? path.basename(normalized.canonical),
      createdAt: new Date().toISOString(),
    };
    insert.run(
      project.id,
      project.canonicalPath,
      project.displayName,
      project.createdAt,
    );
    return reply.code(201).send({ project });
  });

  app.get('/api/monitor/v1/projects', async (_req, reply) => {
    const rows = list.all() as ProjectRow[];
    return reply.send({ projects: rows.map(toJson) });
  });

  app.get<{ Params: { id: string } }>(
    '/api/monitor/v1/projects/:id',
    async (req, reply) => {
      const row = byId.get(req.params.id) as ProjectRow | undefined;
      if (!row) {
        return sendError(reply, 404, 'project_not_found', 'unknown project id');
      }
      return reply.send({ project: toJson(row) });
    },
  );

  app.delete<{ Params: { id: string } }>(
    '/api/monitor/v1/projects/:id',
    async (req, reply) => {
      const row = byId.get(req.params.id) as ProjectRow | undefined;
      if (!row) {
        return sendError(reply, 404, 'project_not_found', 'unknown project id');
      }
      // Runs/cursors/collection_errors cascade via FK; the audit row
      // survives so the deletion stays observable (plan U3 §13).
      db.transaction(() => {
        audit.run(
          new Date().toISOString(),
          row.id,
          'project_deleted',
          JSON.stringify({ canonicalPath: row.canonical_path }),
        );
        remove.run(req.params.id);
      })();
      return reply.code(204).send();
    },
  );
}
