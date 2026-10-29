/**
 * Notifications + pending-interactions REST (Plan4-U1):
 * `GET /notifications` (state/kind/q filters + 50-row paging),
 * `POST /:id/read`, `POST /read-all`, `POST /:id/mute` — all
 * idempotent — and `GET /pending-interactions`. `link` carries the
 * deep link back to the run evidence (S5).
 */

import type { FastifyInstance } from 'fastify';
import { z } from 'zod';
import type { MonitorDb } from '../db/index.js';
import { sendError } from '../lib/errors.js';
import { notificationLink } from '../notify/outbox.js';

const LIST_STATES = new Set(['unread', 'read', 'muted', 'resolved']);

interface NotificationRow {
  id: string;
  kind: string;
  project_id: string;
  run_gid: string | null;
  pending_interaction_id: string | null;
  title: string;
  body: string | null;
  detail: string | null;
  state: string;
  created_at: string;
  resolved_at: string | null;
}

function toJson(row: NotificationRow) {
  return {
    id: row.id,
    kind: row.kind,
    project_id: row.project_id,
    run_gid: row.run_gid,
    pending_interaction_id: row.pending_interaction_id,
    title: row.title,
    body: row.body,
    detail: row.detail ? JSON.parse(row.detail) : null,
    state: row.state,
    created_at: row.created_at,
    resolved_at: row.resolved_at,
    link: notificationLink(row.run_gid),
  };
}

const listQuery = z.object({
  state: z.string().optional(),
  kind: z.string().optional(),
  q: z.string().optional(),
  project: z.string().optional(),
  limit: z.coerce.number().int().min(1).max(200).default(50),
  offset: z.coerce.number().int().min(0).default(0),
});

export function registerNotificationRoutes(
  app: FastifyInstance,
  db: MonitorDb,
): void {
  const byId = db.prepare('SELECT * FROM notifications WHERE id = ?');
  const markState = db.prepare(
    `UPDATE notifications SET state = ? WHERE id = ? AND state != 'resolved'`,
  );

  app.get('/api/monitor/v1/notifications', async (req, reply) => {
    const parsed = listQuery.safeParse(req.query);
    if (!parsed.success) {
      return sendError(reply, 400, 'invalid_query', 'bad notification query', {
        issues: parsed.error.issues,
      });
    }
    const q = parsed.data;
    const where: string[] = [];
    const params: unknown[] = [];
    if (q.state) {
      const states = q.state
        .split(',')
        .map((s) => s.trim())
        .filter(Boolean);
      if (!states.every((s) => LIST_STATES.has(s))) {
        return sendError(reply, 400, 'invalid_query', 'unknown state filter');
      }
      where.push(`state IN (${states.map(() => '?').join(',')})`);
      params.push(...states);
    } else {
      // Default view hides muted+resolved rows (S12 — recoverable via filter).
      where.push("state IN ('unread','read')");
    }
    if (q.kind) {
      where.push('kind = ?');
      params.push(q.kind);
    }
    if (q.project) {
      where.push('project_id = ?');
      params.push(q.project);
    }
    if (q.q) {
      where.push('(title LIKE ? OR body LIKE ?)');
      params.push(`%${q.q}%`, `%${q.q}%`);
    }
    const whereSql = where.length ? `WHERE ${where.join(' AND ')}` : '';
    const total = (
      db
        .prepare(`SELECT COUNT(*) AS n FROM notifications ${whereSql}`)
        .get(...params) as { n: number }
    ).n;
    const rows = db
      .prepare(
        `SELECT * FROM notifications ${whereSql}
         ORDER BY created_at DESC, id ASC LIMIT ? OFFSET ?`,
      )
      .all(...params, q.limit, q.offset) as NotificationRow[];
    return reply.send({
      notifications: rows.map(toJson),
      total,
      limit: q.limit,
      offset: q.offset,
    });
  });

  app.post<{ Params: { id: string } }>(
    '/api/monitor/v1/notifications/:id/read',
    async (req, reply) => {
      const row = byId.get(req.params.id) as NotificationRow | undefined;
      if (!row) {
        return sendError(reply, 404, 'not_found', 'unknown notification');
      }
      if (row.state !== 'resolved') markState.run('read', row.id);
      return reply.send({ notification: toJson({ ...row, state: 'read' }) });
    },
  );

  app.post('/api/monitor/v1/notifications/read-all', async (_req, reply) => {
    const res = db
      .prepare(`UPDATE notifications SET state = 'read' WHERE state = 'unread'`)
      .run();
    return reply.send({ updated: res.changes });
  });

  app.post<{ Params: { id: string } }>(
    '/api/monitor/v1/notifications/:id/mute',
    async (req, reply) => {
      const row = byId.get(req.params.id) as NotificationRow | undefined;
      if (!row) {
        return sendError(reply, 404, 'not_found', 'unknown notification');
      }
      if (row.state !== 'resolved') markState.run('muted', row.id);
      return reply.send({ notification: toJson({ ...row, state: 'muted' }) });
    },
  );

  app.get('/api/monitor/v1/pending-interactions', async (req, reply) => {
    const q = req.query as { project?: string };
    const rows = (
      q.project
        ? db
            .prepare(
              `SELECT * FROM pending_interactions
               WHERE status = 'pending' AND project_id = ?
               ORDER BY created_at DESC`,
            )
            .all(q.project)
        : db
            .prepare(
              `SELECT * FROM pending_interactions
               WHERE status = 'pending' ORDER BY created_at DESC`,
            )
            .all()
    ) as {
      id: string;
      project_id: string;
      session_id: string;
      run_gid: string | null;
      prompt: string | null;
      status: string;
      created_at: string;
    }[];
    return reply.send({
      pending: rows.map((r) => ({
        id: r.id,
        project_id: r.project_id,
        session_id: r.session_id,
        run_gid: r.run_gid,
        prompt: r.prompt,
        status: r.status,
        created_at: r.created_at,
        link: notificationLink(r.run_gid),
      })),
    });
  });
}
