/**
 * Four-layer health classification (Plan3-U2, D8: server-side rules).
 *
 * Pure function over projection rows + injected pid probe — every
 * branch is unit-testable without fs/sqlite. Vocabulary:
 *
 * - collector: `ok | error | stale`
 * - runtime:   `running | silent | exited | dead_unverified |
 *               pid_unverified | unknown`
 * - backend:   `ok | timeout | exit_error | transport_unknown |
 *               unknown` (+ `hint`, `pending`)
 *
 * Invariants (plan §5): no evidence → `unknown`, never a fabricated
 * status; a converged terminal state sets `terminal_converged` and
 * suppresses `silent` (S12).
 */

import type { PidCheck, PidCheckResult } from './pid_check.js';

export type CollectorStatus = 'ok' | 'error' | 'stale';
export type RuntimeStatus =
  | 'running'
  | 'silent'
  | 'exited'
  | 'dead_unverified'
  | 'pid_unverified'
  | 'unknown';
export type BackendStatus =
  | 'ok'
  | 'timeout'
  | 'exit_error'
  | 'transport_unknown'
  | 'unknown';

export interface CollectorLayer {
  status: CollectorStatus;
  error?: string;
  consecutive_failures: number;
  last_collected_at: string | null;
}

export interface RuntimeLayer {
  status: RuntimeStatus;
  pid: number | null;
  pid_alive: boolean | null;
  pid_start_verified: boolean | null;
  last_event_at: string | null;
  last_activity_at: string | null;
}

export interface BackendLayer {
  status: BackendStatus;
  hint?: string;
  pending?: boolean;
  last_end_at: string | null;
}

export interface RunHealth {
  layers: {
    collector: CollectorLayer;
    runtime: RuntimeLayer;
    backend: BackendLayer;
  };
  terminal_converged: boolean;
  last_valid_at: string | null;
}

export interface ClassifyRun {
  pid: number | null;
  started_at: string | null;
  terminal: string | null;
  last_event_at: string | null;
}

export interface ActivityDetail {
  ts: string | null;
  phase: 'start' | 'end';
  seq_in_run: number;
  detail: Record<string, unknown> | null;
}

export interface CollectorInput {
  consecutive_failures: number;
  last_collected_at: string | null;
  error?: string | null;
  /** `last_collected_at` older than this (ms) reads `stale`. */
  staleMs: number;
}

/** Silent window — a live run with no fresh evidence reads `silent`. */
export const DEFAULT_SILENT_MS = 5 * 60 * 1000;

export interface ClassifyInput {
  run: ClassifyRun;
  /** Newest activity rows (any order) — the last seq_in_run pair wins. */
  activity: ActivityDetail[];
  collector: CollectorInput;
  pidCheck: PidCheck;
  now: Date;
  silentMs?: number;
}

function latest(a: (string | null)[]): string | null {
  let best: string | null = null;
  let bestMs = -Infinity;
  for (const v of a) {
    if (!v) continue;
    const t = Date.parse(v);
    if (Number.isFinite(t) && t > bestMs) {
      best = v;
      bestMs = t;
    }
  }
  return best;
}

function classifyCollector(c: CollectorInput, now: Date): CollectorLayer {
  const base = {
    consecutive_failures: c.consecutive_failures,
    last_collected_at: c.last_collected_at,
  };
  if (c.consecutive_failures > 0) {
    return {
      status: 'error',
      ...(c.error ? { error: c.error } : {}),
      ...base,
    };
  }
  if (c.last_collected_at) {
    const age = now.getTime() - Date.parse(c.last_collected_at);
    if (Number.isFinite(age) && age > c.staleMs) {
      return { status: 'stale', ...base };
    }
  }
  return { status: 'ok', ...base };
}

function classifyRuntime(
  run: ClassifyRun,
  pidRes: PidCheckResult | null,
  lastEvidenceAt: string | null,
  silentMs: number,
  now: Date,
): RuntimeLayer {
  const base = {
    pid: run.pid,
    pid_alive: pidRes?.alive ?? null,
    pid_start_verified: pidRes?.startVerified ?? null,
    last_event_at: run.last_event_at,
    last_activity_at: null as string | null,
  };
  if (run.terminal) return { status: 'exited', ...base };
  if (run.pid === null) return { status: 'unknown', ...base };
  if (pidRes === null) return { status: 'unknown', ...base };
  if (!pidRes.alive) return { status: 'dead_unverified', ...base };
  // Alive but identity unconfirmed (pid reuse, no /proc, no
  // started_at) — fail-closed vocabulary, plan §15/§20.
  if (pidRes.startVerified !== true) {
    return { status: 'pid_unverified', ...base };
  }
  const fresh =
    lastEvidenceAt !== null &&
    now.getTime() - Date.parse(lastEvidenceAt) <= silentMs;
  return { status: fresh ? 'running' : 'silent', ...base };
}

function classifyBackend(
  activity: ActivityDetail[],
  runtimeAlive: boolean,
): BackendLayer {
  if (activity.length === 0) {
    return { status: 'unknown', last_end_at: null };
  }
  const last = activity[activity.length - 1];
  if (last.phase === 'start') {
    // A start with no end: in-flight while the pid lives, suspect
    // transport otherwise (plan §11).
    return runtimeAlive
      ? { status: 'unknown', pending: true, last_end_at: null }
      : { status: 'transport_unknown', last_end_at: null };
  }
  const d = last.detail ?? {};
  const hint =
    typeof d['stderr_tail_hint'] === 'string'
      ? (d['stderr_tail_hint'] as string)
      : undefined;
  const last_end_at = last.ts;
  if (d['timed_out'] === true) {
    return { status: 'timeout', ...(hint ? { hint } : {}), last_end_at };
  }
  const exit = d['exit_code'];
  const exitBad = typeof exit === 'number' && exit !== 0;
  if (d['is_error'] === true || exitBad) {
    return { status: 'exit_error', ...(hint ? { hint } : {}), last_end_at };
  }
  return { status: 'ok', ...(hint ? { hint } : {}), last_end_at };
}

export function classifyRunHealth(input: ClassifyInput): RunHealth {
  const silentMs = input.silentMs ?? DEFAULT_SILENT_MS;
  const sorted = [...input.activity].sort(
    (a, b) => a.seq_in_run - b.seq_in_run,
  );
  const lastActivityAt = latest(sorted.map((a) => a.ts));
  const terminalAt = input.run.terminal
    ? (JSON.parse(input.run.terminal) as { at?: string }).at ?? null
    : null;
  const lastEvidenceAt = latest([input.run.last_event_at, lastActivityAt]);
  const last_valid_at = latest([
    input.run.last_event_at,
    lastActivityAt,
    terminalAt,
    input.collector.last_collected_at,
  ]);

  const pidRes =
    input.run.pid !== null && !input.run.terminal
      ? input.pidCheck(input.run.pid, input.run.started_at)
      : null;
  const runtime = classifyRuntime(
    input.run,
    pidRes,
    lastEvidenceAt,
    silentMs,
    input.now,
  );
  runtime.last_activity_at = lastActivityAt;
  const backend = classifyBackend(sorted, pidRes?.alive === true);
  const collector = classifyCollector(input.collector, input.now);

  return {
    layers: { collector, runtime, backend },
    terminal_converged: input.run.terminal !== null,
    last_valid_at,
  };
}

/** Runtime/backend statuses that mark a run `degraded` (S11). */
export function isDegraded(h: RunHealth): boolean {
  if (h.terminal_converged) return false;
  return (
    h.layers.runtime.status === 'silent' ||
    h.layers.runtime.status === 'dead_unverified' ||
    h.layers.runtime.status === 'pid_unverified' ||
    h.layers.backend.status === 'timeout' ||
    h.layers.backend.status === 'exit_error' ||
    h.layers.backend.status === 'transport_unknown'
  );
}
