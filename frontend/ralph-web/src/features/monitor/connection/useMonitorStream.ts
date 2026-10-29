/**
 * Monitor stream driver (Plan3-U4): snapshot → baseline seed →
 * `?cursor=watermark` tail → exponential-backoff reconnect →
 * `resync_required` re-snapshot. React-free core — the provider hooks
 * it into a component effect; unit tests drive it with fakes.
 */

import type { QueryClient } from "@tanstack/react-query";
import { apiGetSnapshot, MONITOR_API_BASE } from "../api";
import { readSseStream } from "./sseClient";

export type ConnectionStatus =
  | "connecting"
  | "connected"
  | "reconnecting"
  | "offline";

export interface MonitorStreamState {
  status: ConnectionStatus;
  /** Last applied stream seq — reconnects resume from here. */
  lastSeq: number;
  /** Server-reported freshest evidence timestamp. */
  lastValidAt: string | null;
  /** Consecutive failed attempts (drives the badge). */
  attempts: number;
}

export interface StreamDriverOptions {
  fetchSnapshot?: () => ReturnType<typeof apiGetSnapshot>;
  openStream?: typeof readSseStream;
  onChange?: (state: MonitorStreamState) => void;
  /** Raw frame hook (browser notifications, tests). Runs before invalidate. */
  onEvent?: (frame: { kind: string; data: unknown }) => void;
  /** Backoff tuning (tests shrink to 0). */
  baseDelayMs?: number;
  maxDelayMs?: number;
  jitter?: number;
  sleep?: (ms: number) => Promise<void>;
  now?: () => Date;
}

export interface StreamDriver {
  stop: () => void;
}

export const BACKOFF_BASE_MS = 500;
export const BACKOFF_MAX_MS = 30_000;

export function backoffDelay(
  attempt: number,
  base = BACKOFF_BASE_MS,
  max = BACKOFF_MAX_MS,
  jitter = 0.25,
): number {
  const exp = Math.min(max, base * 2 ** attempt);
  const j = exp * jitter;
  return exp - j + Math.random() * 2 * j;
}

/** Map a stream kind → the query keys it should invalidate. */
export function invalidationsFor(kind: string): string[][] {
  switch (kind) {
    case "run.created":
    case "run.updated":
      return [["monitor", "runs"], ["monitor", "run"], ["monitor", "overview"]];
    case "run.events":
      return [
        ["monitor", "events"],
        ["monitor", "progress"],
        ["monitor", "graph"],
      ];
    case "run.health":
      return [["monitor", "health"], ["monitor", "run-health"]];
    case "project.health":
    case "project.updated":
      return [["monitor", "projects"], ["monitor", "health"]];
    case "notification.created":
    case "notification.updated":
      return [
        ["monitor", "notifications"],
        ["monitor", "pending-interactions"],
      ];
    default:
      return [];
  }
}

const RUN_KEYS = ["runs", "run", "progress", "graph", "events", "artifacts"];

/**
 * Apply the snapshot baseline into the query cache: projects + runs
 * become the initial data for the list queries, and each run detail
 * key is seeded for its run page.
 */
export function seedSnapshot(
  qc: QueryClient,
  snap: { watermark: number; projects: unknown[]; runs: unknown[] },
): void {
  qc.setQueryData(["monitor", "snapshot"], snap);
  for (const run of snap.runs as { globalRunId?: string }[]) {
    if (run.globalRunId) {
      qc.setQueryData(["monitor", "snapshot-run", run.globalRunId], run);
    }
  }
}

export function startMonitorStream(
  qc: QueryClient,
  opts: StreamDriverOptions = {},
): StreamDriver {
  const fetchSnapshot = opts.fetchSnapshot ?? apiGetSnapshot;
  const openStream = opts.openStream ?? readSseStream;
  const sleep =
    opts.sleep ?? ((ms: number) => new Promise<void>((r) => setTimeout(r, ms)));
  const now = opts.now ?? (() => new Date());

  const state: MonitorStreamState = {
    status: "connecting",
    lastSeq: 0,
    lastValidAt: null,
    attempts: 0,
  };
  const ac = new AbortController();
  let stopped = false;

  const emit = () => opts.onChange?.({ ...state });
  const setStatus = (s: ConnectionStatus) => {
    state.status = s;
    emit();
  };

  const dispatch = (kind: string) => {
    for (const key of invalidationsFor(kind)) {
      void qc.invalidateQueries({ queryKey: key });
    }
  };

  const loop = async () => {
    for (;;) {
      if (stopped) return;
      try {
        const snap = await fetchSnapshot();
        if (stopped) return;
        seedSnapshot(qc, snap);
        state.lastSeq = Math.max(state.lastSeq, snap.watermark);
        state.lastValidAt = snap.generated_at;
        state.attempts = 0;
        setStatus("connected");
      } catch {
        if (stopped) return;
        state.attempts += 1;
        setStatus(state.attempts > 5 ? "offline" : "reconnecting");
        await sleep(
          backoffDelay(
            state.attempts,
            opts.baseDelayMs,
            opts.maxDelayMs,
            opts.jitter,
          ),
        );
        continue;
      }

      try {
        await openStream(
          `${MONITOR_API_BASE}/stream?cursor=${state.lastSeq}`,
          {
            signal: ac.signal,
            onFrame: (f) => {
              const seq = f.id ? Number(f.id) : NaN;
              if (f.event === "resync_required") {
                // Server pruned our cursor — re-baseline (S7).
                state.lastSeq = 0;
                throw new ResyncRequired();
              }
              if (Number.isFinite(seq)) {
                if (seq <= state.lastSeq) return; // dedupe (plan §5)
                state.lastSeq = seq;
              }
              state.lastValidAt = now().toISOString();
              if (f.event) {
                let data: unknown;
                try {
                  data = f.data ? JSON.parse(f.data) : undefined;
                } catch {
                  data = undefined;
                }
                opts.onEvent?.({ kind: f.event, data });
                dispatch(f.event);
              }
            },
          },
        );
        // Clean close — the next snapshot+reopen keeps us effectively
        // connected; only an actual failure degrades the badge. The
        // floor delay avoids a hot loop if the server closes fast.
        if (stopped) return;
        await sleep(opts.baseDelayMs ?? BACKOFF_BASE_MS);
        continue;
      } catch (err) {
        if (stopped) return;
        if (err instanceof ResyncRequired) {
          setStatus("connecting");
          continue; // re-snapshot immediately, no backoff
        }
        state.attempts += 1;
        setStatus(state.attempts > 5 ? "offline" : "reconnecting");
        await sleep(backoffDelay(state.attempts, opts.baseDelayMs, opts.maxDelayMs, opts.jitter));
      }
    }
  };

  void loop();
  return {
    stop: () => {
      stopped = true;
      ac.abort();
    },
  };
}

class ResyncRequired extends Error {}

/** All run-scoped keys for a globalRunId — used by run.health dispatch. */
export function runScopedKeys(globalRunId: string): string[][] {
  return RUN_KEYS.map((k) => ["monitor", k, globalRunId]);
}
