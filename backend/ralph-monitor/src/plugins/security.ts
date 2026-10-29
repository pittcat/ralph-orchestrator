import { randomUUID } from 'node:crypto';
import type { FastifyInstance, FastifyReply, FastifyRequest } from 'fastify';
import { sendError } from '../lib/errors.js';

/**
 * Security baseline for a loopback-only monitor service.
 *
 * 1. Host allowlist — only loopback hostnames are accepted on any
 *    request (DNS-rebinding defense).
 * 2. Origin check — unsafe methods carrying an `Origin` header must
 *    name a loopback origin; `Origin: null` is rejected.
 * 3. CSRF double-submit — unsafe methods must present
 *    `x-monitor-csrf` equal to BOTH the `monitor_sid` (HttpOnly,
 *    SameSite=Lax) and `monitor_csrf` (readable, SameSite=Lax) cookie
 *    values. The session token is issued on any response when absent,
 *    so a browser client acquires it via the first GET and replays it
 *    in the header — something a cross-site form/script can never do.
 *
 * Safe methods (GET/HEAD/OPTIONS) skip Origin+CSRF but still honor
 * the Host allowlist.
 */

export const SID_COOKIE = 'monitor_sid';
export const CSRF_COOKIE = 'monitor_csrf';
export const CSRF_HEADER = 'x-monitor-csrf';

const SAFE_METHODS = new Set(['GET', 'HEAD', 'OPTIONS']);
export const LOOPBACK_HOSTNAMES = new Set([
  'localhost',
  '127.0.0.1',
  '::1',
  '[::1]',
]);

export function isAllowedHost(
  hostHeader: string | undefined,
  allowed: ReadonlySet<string> = LOOPBACK_HOSTNAMES,
): boolean {
  if (!hostHeader) return false;
  // Strip the port — the service may run behind `ralph web` port
  // forwarding; loopback hostnames are trusted at any port. A suffix
  // only counts as a port when it is all digits, so
  // `localhost:3200.evil.com` keeps the full (rejected) hostname.
  const host = hostHeader.trim().toLowerCase();
  let hostname: string;
  if (host.startsWith('[')) {
    hostname = host.match(/^\[[^\]]+\]/)?.[0] ?? host;
  } else {
    const idx = host.lastIndexOf(':');
    hostname =
      idx > 0 && /^\d+$/.test(host.slice(idx + 1))
        ? host.slice(0, idx)
        : host;
  }
  return allowed.has(hostname);
}

export function isAllowedOrigin(
  origin: string | undefined,
  allowed: ReadonlySet<string> = LOOPBACK_HOSTNAMES,
): boolean {
  if (origin === undefined) {
    // Non-browser clients (curl, server-to-server) send no Origin.
    return true;
  }
  if (origin === 'null' || origin === '') {
    return false;
  }
  try {
    const url = new URL(origin);
    if (url.protocol !== 'http:' && url.protocol !== 'https:') {
      return false;
    }
    return allowed.has(url.hostname.toLowerCase());
  } catch {
    return false;
  }
}

export function parseCookies(header: string | undefined): Record<string, string> {
  const out: Record<string, string> = {};
  if (!header) return out;
  for (const part of header.split(';')) {
    const eq = part.indexOf('=');
    if (eq === -1) continue;
    const name = part.slice(0, eq).trim();
    if (!name) continue;
    try {
      out[name] = decodeURIComponent(part.slice(eq + 1).trim());
    } catch {
      out[name] = part.slice(eq + 1).trim();
    }
  }
  return out;
}

/// True when the double-submit contract is satisfied: all three of
/// (sid cookie, csrf cookie, csrf header) exist and are identical.
export function csrfSatisfied(req: FastifyRequest): boolean {
  const cookies = parseCookies(req.headers.cookie);
  const sid = cookies[SID_COOKIE];
  const csrf = cookies[CSRF_COOKIE];
  const header = req.headers[CSRF_HEADER];
  const headerValue = Array.isArray(header) ? header[0] : header;
  return Boolean(sid) && sid === csrf && headerValue === csrf;
}

function forbidden(reply: FastifyReply, code: string, message: string) {
  return sendError(reply, 403, code, message);
}

/// `allowedHosts` widens the loopback allowlist for LAN mode — callers
/// pass the union of loopback names + local interface addresses so the
/// DNS-rebinding defense still rejects arbitrary foreign hostnames.
export function registerSecurity(
  app: FastifyInstance,
  allowedHosts: ReadonlySet<string> = LOOPBACK_HOSTNAMES,
): void {
  app.addHook('onRequest', async (req, reply) => {
    if (!isAllowedHost(req.headers.host, allowedHosts)) {
      return forbidden(reply, 'forbidden_host', 'Host header is not allowed');
    }
    if (SAFE_METHODS.has(req.method)) {
      return;
    }
    if (!isAllowedOrigin(req.headers.origin, allowedHosts)) {
      return forbidden(reply, 'forbidden_origin', 'Origin header is not allowed');
    }
    if (!csrfSatisfied(req)) {
      return forbidden(
        reply,
        'csrf',
        'missing or mismatched CSRF token; GET a session first and echo monitor_csrf via x-monitor-csrf',
      );
    }
  });

  app.addHook('onSend', async (req, reply) => {
    const cookies = parseCookies(req.headers.cookie);
    const sid = cookies[SID_COOKIE];
    const csrf = cookies[CSRF_COOKIE];
    // Issue a fresh session when absent or tampered (sid ≠ csrf).
    if (!sid || sid !== csrf) {
      const token = randomUUID();
      reply.header('set-cookie', [
        `${SID_COOKIE}=${token}; Path=/; HttpOnly; SameSite=Lax`,
        `${CSRF_COOKIE}=${token}; Path=/; SameSite=Lax`,
      ]);
    }
  });
}
