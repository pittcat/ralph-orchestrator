import type { FastifyInstance } from 'fastify';
import type { MonitorDb } from '../db/index.js';
import { sendError } from '../lib/errors.js';
import { attachSse, type SseOptions } from '../stream/sse.js';

/**
 * `GET /api/monitor/v1/stream?cursor=N` — the durable monitor event
 * stream over SSE (Plan3-U3). `cursor=0`/absent replays the whole
 * retained window then tails live; a cursor below the window gets a
 * `resync_required` event and a clean close (S7).
 */
export function registerStreamRoutes(
  app: FastifyInstance,
  db: MonitorDb,
  sse: Partial<SseOptions> = {},
): void {
  app.get<{ Querystring: { cursor?: string } }>(
    '/api/monitor/v1/stream',
    async (req, reply) => {
      const rawCursor = req.query.cursor;
      let cursor = 0;
      if (rawCursor !== undefined && rawCursor !== '') {
        cursor = Number(rawCursor);
        if (!Number.isInteger(cursor) || cursor < 0) {
          return sendError(
            reply,
            400,
            'invalid_cursor',
            'cursor must be a non-negative integer',
          );
        }
      }
      // Long-lived response: take over the raw socket so fastify's
      // default timeout/buffering never truncates the stream (§19).
      reply.hijack();
      attachSse(req, reply, db, cursor, sse);
      return reply;
    },
  );
}
