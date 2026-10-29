import type { FastifyReply } from 'fastify';

/**
 * Canonical monitor error envelope: `{ error: { code, message, details? } }`.
 */
export function sendError(
  reply: FastifyReply,
  status: number,
  code: string,
  message: string,
  details?: Record<string, unknown>,
): FastifyReply {
  return reply.code(status).send({
    error: details === undefined ? { code, message } : { code, message, details },
  });
}
