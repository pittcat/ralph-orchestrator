import assert from 'node:assert/strict';
import { describe, it } from 'node:test';
import {
  classifyRunHealth,
  isDegraded,
  type ActivityDetail,
  type ClassifyInput,
} from '../../src/health/classify.js';
import type { PidCheckResult } from '../../src/health/pid_check.js';

const NOW = new Date('2026-10-28T12:10:00Z');
const FRESH = '2026-10-28T12:09:50Z'; // inside the 30s collector-stale window
const STALE = '2026-10-28T11:00:00Z';

const alive: PidCheckResult = { alive: true, startVerified: true };
const dead: PidCheckResult = { alive: false, startVerified: null };
const reused: PidCheckResult = { alive: true, startVerified: false };
const unverifiable: PidCheckResult = { alive: true, startVerified: null };

function input(over: Partial<ClassifyInput> = {}): ClassifyInput {
  return {
    run: {
      pid: 4242,
      started_at: '2026-10-28T12:00:00Z',
      terminal: null,
      last_event_at: FRESH,
    },
    activity: [],
    collector: {
      consecutive_failures: 0,
      last_collected_at: FRESH,
      staleMs: 30_000,
    },
    pidCheck: () => alive,
    now: NOW,
    ...over,
  };
}

function end(
  seq: number,
  detail: Record<string, unknown>,
  ts = FRESH,
): ActivityDetail {
  return { seq_in_run: seq, phase: 'end', ts, detail };
}

describe('collector layer', () => {
  it('failures>0 → error + carries latest error', () => {
    const h = classifyRunHealth(
      input({
        collector: {
          consecutive_failures: 2,
          last_collected_at: FRESH,
          error: 'spawn nonzero: 2',
          staleMs: 30_000,
        },
      }),
    );
    assert.equal(h.layers.collector.status, 'error');
    assert.match(h.layers.collector.error ?? '', /nonzero/);
    assert.equal(h.layers.collector.consecutive_failures, 2);
  });

  it('old watermark → stale', () => {
    const h = classifyRunHealth(
      input({
        collector: {
          consecutive_failures: 0,
          last_collected_at: STALE,
          staleMs: 30_000,
        },
      }),
    );
    assert.equal(h.layers.collector.status, 'stale');
  });

  it('fresh watermark + no failures → ok', () => {
    const h = classifyRunHealth(input());
    assert.equal(h.layers.collector.status, 'ok');
  });
});

describe('runtime layer', () => {
  it('pid alive + fresh events → running', () => {
    const h = classifyRunHealth(input());
    assert.equal(h.layers.runtime.status, 'running');
    assert.equal(h.terminal_converged, false);
  });

  it('pid alive + stale evidence → silent', () => {
    const h = classifyRunHealth(
      input({
        run: {
          pid: 1,
          started_at: FRESH,
          terminal: null,
          last_event_at: STALE,
        },
      }),
    );
    assert.equal(h.layers.runtime.status, 'silent');
  });

  it('pid dead + terminal → exited + converged', () => {
    const h = classifyRunHealth(
      input({
        run: {
          pid: 1,
          started_at: FRESH,
          terminal: JSON.stringify({
            reason: 'completed',
            at: '2026-10-28T12:05:00Z',
          }),
          last_event_at: STALE,
        },
        pidCheck: () => dead,
      }),
    );
    assert.equal(h.layers.runtime.status, 'exited');
    assert.equal(h.terminal_converged, true);
    // pid probe is skipped once the run has converged.
    assert.equal(h.layers.runtime.pid_alive, null);
  });

  it('pid dead + no terminal → dead_unverified', () => {
    const h = classifyRunHealth(input({ pidCheck: () => dead }));
    assert.equal(h.layers.runtime.status, 'dead_unverified');
  });

  it('alive pid but /proc start mismatch → pid_unverified', () => {
    const h = classifyRunHealth(input({ pidCheck: () => reused }));
    assert.equal(h.layers.runtime.status, 'pid_unverified');
    assert.equal(h.layers.runtime.pid_start_verified, false);
  });

  it('alive pid, starttime unverifiable → pid_unverified', () => {
    const h = classifyRunHealth(input({ pidCheck: () => unverifiable }));
    assert.equal(h.layers.runtime.status, 'pid_unverified');
  });

  it('no pid + no terminal → unknown', () => {
    const h = classifyRunHealth(
      input({
        run: {
          pid: null,
          started_at: null,
          terminal: null,
          last_event_at: null,
        },
      }),
    );
    assert.equal(h.layers.runtime.status, 'unknown');
  });

  it('terminal suppresses silent even with stale evidence', () => {
    const h = classifyRunHealth(
      input({
        run: {
          pid: 1,
          started_at: FRESH,
          terminal: JSON.stringify({
            reason: 'cancelled',
            at: '2026-10-28T12:05:00Z',
          }),
          last_event_at: STALE,
        },
      }),
    );
    assert.equal(h.layers.runtime.status, 'exited');
    assert.equal(h.terminal_converged, true);
  });
});

describe('backend layer', () => {
  it('no activity rows → unknown', () => {
    const h = classifyRunHealth(input());
    assert.equal(h.layers.backend.status, 'unknown');
  });

  it('recent ok end → ok', () => {
    const h = classifyRunHealth(
      input({
        activity: [
          { seq_in_run: 1, phase: 'start', ts: FRESH, detail: { hat: 'executor' } },
          end(2, { exit_code: 0, timed_out: false, is_error: false }),
        ],
      }),
    );
    assert.equal(h.layers.backend.status, 'ok');
    assert.equal(h.layers.backend.last_end_at, FRESH);
  });

  it('timed_out end → timeout', () => {
    const h = classifyRunHealth(
      input({
        activity: [end(2, { exit_code: null, timed_out: true, is_error: true })],
      }),
    );
    assert.equal(h.layers.backend.status, 'timeout');
  });

  it('exit_code != 0 → exit_error', () => {
    const h = classifyRunHealth(
      input({
        activity: [end(2, { exit_code: 1, timed_out: false, is_error: true })],
      }),
    );
    assert.equal(h.layers.backend.status, 'exit_error');
  });

  it('is_error end → exit_error', () => {
    const h = classifyRunHealth(
      input({
        activity: [end(2, { exit_code: 0, timed_out: false, is_error: true })],
      }),
    );
    assert.equal(h.layers.backend.status, 'exit_error');
  });

  it('stderr hint surfaces on the end row', () => {
    const h = classifyRunHealth(
      input({
        activity: [
          end(2, {
            exit_code: 1,
            timed_out: false,
            is_error: true,
            stderr_tail_hint: 'rate_limited',
          }),
        ],
      }),
    );
    assert.equal(h.layers.backend.status, 'exit_error');
    assert.equal(h.layers.backend.hint, 'rate_limited');
  });

  it('start without end + alive pid → unknown + pending', () => {
    const h = classifyRunHealth(
      input({
        activity: [
          { seq_in_run: 5, phase: 'start', ts: FRESH, detail: { hat: 'executor' } },
        ],
      }),
    );
    assert.equal(h.layers.backend.status, 'unknown');
    assert.equal(h.layers.backend.pending, true);
  });

  it('start without end + dead pid → transport_unknown', () => {
    const h = classifyRunHealth(
      input({
        pidCheck: () => dead,
        activity: [
          { seq_in_run: 5, phase: 'start', ts: FRESH, detail: { hat: 'executor' } },
        ],
      }),
    );
    assert.equal(h.layers.backend.status, 'transport_unknown');
  });
});

describe('whole-health invariants', () => {
  it('last_valid_at is the freshest of event/activity/collector', () => {
    const h = classifyRunHealth(
      input({
        activity: [end(2, { exit_code: 0 }, '2026-10-28T12:09:30Z')],
      }),
    );
    // collector's 12:09:50 watermark beats the 09:30 activity ts.
    assert.equal(h.last_valid_at, '2026-10-28T12:09:50Z');
    const older = classifyRunHealth(
      input({
        run: {
          pid: 1,
          started_at: FRESH,
          terminal: null,
          last_event_at: '2026-10-28T12:00:00Z',
        },
        collector: {
          consecutive_failures: 0,
          last_collected_at: '2026-10-28T12:00:00Z',
          staleMs: 30_000,
        },
        activity: [end(2, { exit_code: 0 }, '2026-10-28T12:09:30Z')],
      }),
    );
    assert.equal(older.last_valid_at, '2026-10-28T12:09:30Z');
  });

  it('degraded: silent live run counts, converged terminal does not', () => {
    const silent = classifyRunHealth(
      input({
        run: {
          pid: 1,
          started_at: FRESH,
          terminal: null,
          last_event_at: STALE,
        },
      }),
    );
    assert.equal(isDegraded(silent), true);
    const done = classifyRunHealth(
      input({
        run: {
          pid: 1,
          started_at: FRESH,
          terminal: JSON.stringify({ reason: 'completed', at: STALE }),
          last_event_at: STALE,
        },
        pidCheck: () => dead,
      }),
    );
    assert.equal(isDegraded(done), false);
  });

  it('degraded: backend exit_error on a live run counts', () => {
    const h = classifyRunHealth(
      input({ activity: [end(2, { exit_code: 2, is_error: true })] }),
    );
    assert.equal(isDegraded(h), true);
  });
});
