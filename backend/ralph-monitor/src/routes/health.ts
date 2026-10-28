import type { FastifyInstance } from 'fastify';
import type { MonitorDb } from '../db/index.js';
import { sendError } from '../lib/errors.js';
import { getCursor, latestCollectionError } from '../db/run_projection.js';

/**
 * Per-project collector health: `{collector, error?, last_collected_at}`
 * (plan U3 §3). `error` carries the newest collection_errors entry
 * when the collector is in a failing state.
 */
export function registerHealthRoutes(
  app: FastifyInstance,
  db: MonitorDb,
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
      const failing = (cursor?.consecutive_failures ?? 0) > 0;
      return reply.send({
        collector: failing ? 'error' : 'ok',
        ...(failing
          ? { error: latestCollectionError(db, req.params.id) }
          : {}),
        last_collected_at: cursor?.last_collected_at ?? null,
      });
    },
  );
}
