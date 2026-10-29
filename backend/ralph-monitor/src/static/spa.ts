/**
 * Static SPA hosting (Plan5-U4, D8): `@fastify/static` serves the
 * built dashboard; `setNotFoundHandler` implements the SPA fallback —
 * GET/HEAD on a non-`/api` path returns index.html (`no-store` so a
 * new deploy is never masked by cache), while unknown `/api/*` keeps
 * the JSON error envelope.
 */

import fs from 'node:fs';
import path from 'node:path';
import type { FastifyInstance } from 'fastify';
import fastifyStatic from '@fastify/static';
import { sendError } from '../lib/errors.js';

export function registerStatic(
  app: FastifyInstance,
  staticDir: string,
): void {
  const indexPath = path.join(staticDir, 'index.html');

  void app.register(fastifyStatic, {
    root: staticDir,
    index: ['index.html'],
    // cacheControl disabled → setHeaders owns the policy: hashed
    // assets get a long cache, index.html/*.html stays no-store.
    cacheControl: false,
    setHeaders: (res, filePath) => {
      res.setHeader(
        'cache-control',
        filePath.endsWith('.html') ? 'no-store' : 'public, max-age=3600',
      );
    },
  });

  app.setNotFoundHandler((req, reply) => {
    const url = req.raw.url ?? '/';
    if (url.startsWith('/api/')) {
      return sendError(reply, 404, 'not_found', `unknown endpoint: ${url}`);
    }
    if (req.method !== 'GET' && req.method !== 'HEAD') {
      return sendError(reply, 404, 'not_found', `unknown endpoint: ${url}`);
    }
    return reply
      .header('cache-control', 'no-store')
      .type('text/html')
      .send(fs.readFileSync(indexPath));
  });
}

/** Probe page when the dist bundle is missing (non-fatal, §5). */
export function registerStaticMissing(app: FastifyInstance, dir: string): void {
  app.setNotFoundHandler((req, reply) => {
    const url = req.raw.url ?? '/';
    if (url.startsWith('/api/')) {
      return sendError(reply, 404, 'not_found', `unknown endpoint: ${url}`);
    }
    return reply.type('text/html').send(
      `<!doctype html><meta charset=utf-8><title>ralph monitor</title>` +
        `<p>Dashboard bundle not found at <code>${dir}</code> — ` +
        `run <code>npm run build -w @ralph-web/dashboard</code> or set ` +
        `<code>RALPH_MONITOR_STATIC_DIR</code>. The API is up at ` +
        `<code>/api/monitor/v1/*</code>.</p>`,
    );
  });
}
