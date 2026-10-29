/**
 * Operations API (Plan5-U2). POST creates+ dispatches synchronously;
 * launch returns `dispatched` immediately (confirm lands on the next
 * collect round), run-targeted ops return terminal states inline.
 * GET list doubles as the audit log (actor/origin/timestamps).
 */

import type { FastifyInstance, FastifyRequest } from 'fastify';
import type { MonitorDb } from '../db/index.js';
import { sendError } from '../lib/errors.js';
import { parseCookies, SID_COOKIE } from '../plugins/security.js';
import {
  createIntent,
  getOperation,
  type OperationRow,
} from '../ops/intents.js';
import { dispatchOperation, type DispatchDeps } from '../ops/dispatch.js';
import { recheckOperation } from '../ops/confirm.js';
import { presetCatalog, type CatalogDeps } from '../presets/catalog.js';

function toJson(op: OperationRow) {
  return {
    id: op.id,
    idempotencyKey: op.idempotency_key,
    kind: op.kind,
    projectId: op.project_id,
    runGid: op.run_gid,
    clientRequestId: op.client_request_id,
    params: JSON.parse(op.params),
    expectedRevision: op.expected_revision,
    state: op.state,
    receipt: op.receipt ? JSON.parse(op.receipt) : null,
    actor: op.actor,
    origin: op.origin,
    error: op.error,
    createdAt: op.created_at,
    dispatchedAt: op.dispatched_at,
    confirmedAt: op.confirmed_at,
  };
}

function actorOf(req: FastifyRequest): string | null {
  return parseCookies(req.headers.cookie)[SID_COOKIE] ?? null;
}

export interface OperationRouteDeps {
  dispatch?: DispatchDeps;
  catalog?: CatalogDeps;
  now?: () => Date;
}

export function registerOperationRoutes(
  app: FastifyInstance,
  db: MonitorDb,
  deps: OperationRouteDeps = {},
): void {
  app.post('/api/monitor/v1/operations', async (req, reply) => {
    const res = createIntent(
      db,
      req.body,
      actorOf(req),
      req.headers.origin ?? null,
      deps.now ?? (() => new Date()),
    );
    if (!res.ok) {
      return sendError(reply, res.status, res.code, res.message, res.details);
    }
    if (res.replay) {
      return reply.code(200).send({ operation: toJson(res.operation) });
    }
    const dispatched = await dispatchOperation(db, res.operation.id, {
      ralphBin: deps.dispatch?.ralphBin,
      env: deps.dispatch?.env,
      now: deps.now,
    });
    return reply.code(201).send({ operation: toJson(dispatched) });
  });

  app.get<{
    Querystring: { project_id?: string; run_gid?: string; state?: string };
  }>('/api/monitor/v1/operations', async (req, reply) => {
    const where: string[] = [];
    const params: unknown[] = [];
    if (req.query.project_id) {
      where.push('project_id = ?');
      params.push(req.query.project_id);
    }
    if (req.query.run_gid) {
      where.push('run_gid = ?');
      params.push(req.query.run_gid);
    }
    if (req.query.state) {
      where.push('state = ?');
      params.push(req.query.state);
    }
    const sql = `SELECT * FROM operations
      ${where.length ? `WHERE ${where.join(' AND ')}` : ''}
      ORDER BY created_at DESC, id LIMIT 500`;
    const rows = db.prepare(sql).all(...params) as OperationRow[];
    return reply.send({ operations: rows.map(toJson) });
  });

  app.get<{ Params: { id: string } }>(
    '/api/monitor/v1/operations/:id',
    async (req, reply) => {
      const op = getOperation(db, req.params.id);
      if (!op) {
        return sendError(reply, 404, 'operation_not_found', 'unknown operation id');
      }
      return reply.send({ operation: toJson(op) });
    },
  );

  app.post<{ Params: { id: string } }>(
    '/api/monitor/v1/operations/:id/recheck',
    async (req, reply) => {
      const op = getOperation(db, req.params.id);
      if (!op) {
        return sendError(reply, 404, 'operation_not_found', 'unknown operation id');
      }
      const updated = recheckOperation(db, req.params.id, deps.now?.() ?? new Date());
      return reply.send({ operation: toJson(updated!) });
    },
  );

  app.get<{ Params: { id: string } }>(
    '/api/monitor/v1/projects/:id/presets',
    async (req, reply) => {
      const exists = db
        .prepare('SELECT 1 FROM projects WHERE id = ?')
        .get(req.params.id);
      if (!exists) {
        return sendError(reply, 404, 'project_not_found', 'unknown project id');
      }
      const catalog = presetCatalog(db, req.params.id, {
        ralphBin: deps.catalog?.ralphBin,
        env: deps.catalog?.env,
        now: deps.now,
      });
      return reply.send(catalog);
    },
  );
}
