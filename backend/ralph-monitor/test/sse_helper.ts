/** Minimal SSE client helper for integration tests (node fetch). */

export interface SseFrame {
  id?: string;
  event?: string;
  data?: string;
  comment?: string;
}

/** Parse a chunk-buffered SSE byte stream into frames. */
export function parseSseFrames(text: string): SseFrame[] {
  const frames: SseFrame[] = [];
  for (const block of text.split('\n\n')) {
    if (!block.trim()) continue;
    const f: SseFrame = {};
    for (const line of block.split('\n')) {
      if (line.startsWith(':')) {
        f.comment = line.slice(1);
      } else if (line.startsWith('id:')) {
        f.id = line.slice(3).trim();
      } else if (line.startsWith('event:')) {
        f.event = line.slice(6).trim();
      } else if (line.startsWith('data:')) {
        f.data =
          f.data === undefined ? line.slice(5) : f.data + '\n' + line.slice(5);
      }
    }
    frames.push(f);
  }
  return frames;
}

export interface ReadSseOpts {
  /** Stop after collecting N complete frames (default 1). */
  frames?: number;
  /** Hard cap on wait (default 5000ms) via AbortSignal.timeout. */
  timeoutMs?: number;
}

/**
 * Fetch `url` and collect at most `opts.frames` SSE frames. Aborts
 * the request at `timeoutMs` (or when enough frames arrived) — the
 * returned array is whatever was parsed up to the abort.
 */
export async function readSse(
  url: string,
  opts: ReadSseOpts = {},
): Promise<SseFrame[]> {
  const want = opts.frames ?? 1;
  const ac = new AbortController();
  const killer = setTimeout(() => ac.abort(), opts.timeoutMs ?? 5_000);
  let buf = '';
  try {
    const res = await fetch(url, { signal: ac.signal });
    if (!res.ok || !res.body) {
      throw new Error(`stream ${url} → ${res.status}`);
    }
    const reader = res.body.getReader();
    const dec = new TextDecoder();
    for (;;) {
      const { done, value } = await reader.read();
      if (done) break;
      if (value) buf += dec.decode(value, { stream: true });
      if (buf.split('\n\n').length - 1 >= want) {
        ac.abort();
        break;
      }
    }
    await reader.cancel().catch(() => {});
  } catch (err) {
    if (!(err instanceof Error && err.name === 'AbortError')) throw err;
  } finally {
    clearTimeout(killer);
  }
  return parseSseFrames(buf);
}

/** Stand up `app` on an ephemeral loopback port; returns base URL. */
export async function listenEphemeral(app: {
  listen: (o: { port: number; host: string }) => Promise<string>;
}): Promise<string> {
  return app.listen({ port: 0, host: '127.0.0.1' });
}
