/**
 * SSE connection management for `/api/monitor/v1/stream` (Plan3-U3).
 *
 * Protocol: `id: <seq>` / `event: <kind>` / `data: <json>` frames;
 * `:hb` comment heartbeats; a `resync_required` *event* when the
 * client's cursor fell below the retained window (or a mid-replay
 * pruning hole is detected, plan §20).
 *
 * Live tailing polls the durable log (pollMs) rather than an in-band
 * pub/sub — the DB is the single ordering authority, a writer that
 * commits between two polls is never missed, and a socket that dies
 * mid-replay needs no registration cleanup beyond `close`.
 */

import type { FastifyReply, FastifyRequest } from 'fastify';
import type { MonitorDb } from '../db/index.js';
import {
  readEvents,
  streamBounds,
  toSseFrame,
} from './monitor_log.js';

export interface SseOptions {
  heartbeatMs: number;
  pollMs: number;
  /** Cut the connection once the socket's buffered bytes exceed this. */
  maxBufferedBytes: number;
  pageLimit: number;
}

export const DEFAULT_SSE_OPTIONS: SseOptions = {
  heartbeatMs: 15_000,
  pollMs: 250,
  maxBufferedBytes: 1_048_576,
  pageLimit: 500,
};

/**
 * Attach `req`'s socket to the stream at `cursor` (rows with
 * `seq > cursor` are delivered; `0` replays the whole retained
 * window). Returns false when the cursor is unrecoverably stale —
 * caller responds 4xx in that case before hijacking... no: resync is
 * an *event*, handled here. Caller must have already validated the
 * cursor is a non-negative integer.
 */
export function attachSse(
  req: FastifyRequest,
  reply: FastifyReply,
  db: MonitorDb,
  cursor: number,
  opts: Partial<SseOptions> = {},
): void {
  const o = { ...DEFAULT_SSE_OPTIONS, ...opts };
  const raw = reply.raw;
  raw.writeHead(200, {
    'content-type': 'text/event-stream',
    'cache-control': 'no-cache',
    connection: 'keep-alive',
    'x-accel-buffering': 'no',
  });

  let closed = false;
  let buffered = 0;
  raw.on('drain', () => {
    buffered = 0;
  });
  const write = (frame: string): boolean => {
    if (closed) return false;
    buffered += Buffer.byteLength(frame);
    raw.write(frame);
    // Logical pending budget: once undrained bytes exceed the cap the
    // consumer is too slow — drop the underlying socket rather than
    // grow memory (S10). `raw.destroy()` alone only ends the response
    // object and can leave the TCP connection half-alive.
    if (buffered > o.maxBufferedBytes) {
      cleanup();
      raw.socket?.destroy();
      return false;
    }
    return true;
  };

  const bounds = streamBounds(db);

  const resync = () => {
    // A genuine stream event (not a comment) so EventSource clients
    // see `kind = resync_required` before the socket closes.
    write(
      `event: resync_required\ndata: ${JSON.stringify({
        reason: 'cursor_below_window',
        min: bounds?.min ?? 0,
        max: bounds?.max ?? 0,
      })}\n\n`,
    );
    cleanup();
    raw.end();
  };

  // Stale cursor: the client asked for rows older than the retained
  // window — those bytes are gone for good (S7).
  if (cursor > 0 && bounds !== null && cursor < bounds.min - 1) {
    resync();
    return;
  }

  let lastSeq = cursor;
  let heartbeat: NodeJS.Timeout | undefined;
  let poll: NodeJS.Timeout | undefined;

  const drain = (): boolean => {
    const rows = readEvents(db, lastSeq, o.pageLimit);
    for (const row of rows) {
      // Pruning hole detected mid-stream (§20): expected seq was
      // deleted out from under us → the only honest answer is resync.
      if (row.seq > lastSeq + 1 && lastSeq >= (bounds?.min ?? row.seq)) {
        resync();
        return false;
      }
      lastSeq = row.seq;
      if (!write(toSseFrame(row))) return false;
    }
    return true;
  };

  function cleanup() {
    closed = true;
    if (heartbeat) clearInterval(heartbeat);
    if (poll) clearInterval(poll);
  }

  req.raw.on('close', cleanup);
  raw.on('close', cleanup);
  raw.on('error', cleanup);

  // Initial replay, then the live tail.
  if (!drain()) return;
  heartbeat = setInterval(() => {
    if (!write(':hb\n\n')) return;
  }, o.heartbeatMs);
  poll = setInterval(() => {
    drain();
  }, o.pollMs);
}
