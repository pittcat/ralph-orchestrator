/**
 * Fetch-based SSE reader (Plan3-U4): a single incremental parser over
 * `/stream?cursor=N` — used instead of the native EventSource so the
 * whole pipeline is testable in jsdom with a stubbed fetch (plan §7).
 */

export interface SseFrame {
  /** Stream `id:` line — the monitor event seq. */
  id?: string;
  /** Stream `event:` line — the monitor event kind. */
  event?: string;
  /** Stream `data:` payload (multi-line data joined with \n). */
  data?: string;
}

/** Parse one buffer; returns [frames, remainder]. */
export function splitSseFrames(buf: string): [SseFrame[], string] {
  const frames: SseFrame[] = [];
  let idx = 0;
  for (;;) {
    const end = buf.indexOf("\n\n", idx);
    if (end === -1) break;
    const block = buf.slice(idx, end);
    idx = end + 2;
    if (!block.trim()) continue;
    const f: SseFrame = {};
    let data: string | undefined;
    for (const line of block.split("\n")) {
      if (line.startsWith(":")) continue;
      if (line.startsWith("id:")) f.id = line.slice(3).trim();
      else if (line.startsWith("event:")) f.event = line.slice(6).trim();
      else if (line.startsWith("data:")) {
        const piece = line.slice(5).replace(/^ /, "");
        data = data === undefined ? piece : data + "\n" + piece;
      }
    }
    if (data !== undefined) f.data = data;
    // Pure-comment blocks (heartbeats) carry no fields — skip them.
    if (f.id !== undefined || f.event !== undefined || f.data !== undefined) {
      frames.push(f);
    }
  }
  return [frames, buf.slice(idx)];
}

export interface SseReadOptions {
  signal: AbortSignal;
  onFrame: (frame: SseFrame) => void;
  /** Non-2xx responses surface here (rethrow to reject the read). */
  onHttpError?: (status: number) => void;
}

/**
 * Read the stream until `signal` aborts or the server closes the
 * body. Resolves on a clean close, rejects on abort/network error —
 * the caller decides whether a rejection means "retry" or "stop".
 */
export async function readSseStream(
  url: string,
  opts: SseReadOptions,
): Promise<void> {
  const res = await fetch(url, {
    credentials: "same-origin",
    signal: opts.signal,
    headers: { accept: "text/event-stream" },
  });
  if (!res.ok || !res.body) {
    opts.onHttpError?.(res.status);
    throw new Error(`stream http ${res.status}`);
  }
  const reader = res.body.getReader();
  const dec = new TextDecoder();
  let buf = "";
  try {
    for (;;) {
      const { done, value } = await reader.read();
      if (value) buf += dec.decode(value, { stream: true });
      const [frames, rest] = splitSseFrames(buf);
      buf = rest;
      for (const f of frames) opts.onFrame(f);
      if (done) break;
    }
  } finally {
    reader.cancel().catch(() => undefined);
  }
}
