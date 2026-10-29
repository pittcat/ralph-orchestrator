/**
 * Pid liveness + identity check (Plan3-U2).
 *
 * Two probes behind one injectable interface so classify stays pure:
 *
 * - `kill0`: `process.kill(pid, 0)` — ESRCH means dead, EPERM means
 *   alive-but-not-ours (still counts alive, plan §9).
 * - `/proc/<pid>/stat` starttime cross-check (Linux only): pids are
 *   reused, so a live pid is not proof of identity. The process start
 *   epoch is compared against the manifest's `started_at`; outside the
 *   tolerance (or /proc missing/unreadable, e.g. macOS) the identity
 *   is *unverified* — never misreported as confirmed (plan §20).
 */

import fs from 'node:fs';

export interface PidCheckResult {
  /** kill0 verdict; EPERM (permission denied) counts as alive. */
  alive: boolean;
  /**
   * `/proc` starttime cross-check: `true` the pid really started near
   * `startedAt`, `false` a different process owns the pid (reuse),
   * `null` unverifiable (no /proc, unparseable stat, no started_at).
   */
  startVerified: boolean | null;
}

export type PidCheck = (pid: number, startedAt: string | null) => PidCheckResult;

/** Seconds of slack between process start and manifest `started_at`. */
export const PID_START_TOLERANCE_S = 60;

/** Linux jiffies-per-second for /proc starttime (CLK_TCK). */
const CLK_TCK = 100;

function kill0(pid: number): boolean {
  try {
    process.kill(pid, 0);
    return true;
  } catch (err) {
    // EPERM = exists but owned by another user → still alive.
    return (
      typeof err === 'object' &&
      err !== null &&
      (err as NodeJS.ErrnoException).code === 'EPERM'
    );
  }
}

let cachedBootEpochS: number | null = null;

function bootEpochS(): number | null {
  if (cachedBootEpochS !== null) return cachedBootEpochS;
  try {
    const stat = fs.readFileSync('/proc/stat', 'utf8');
    for (const line of stat.split('\n')) {
      if (line.startsWith('btime ')) {
        const v = Number(line.slice(6).trim());
        if (Number.isFinite(v) && v > 0) {
          cachedBootEpochS = v;
          return v;
        }
      }
    }
  } catch {
    /* no /proc (macOS, containers) */
  }
  return null;
}

/**
 * `/proc/<pid>/stat` field 22 (starttime, jiffies since boot). The
 * comm field may contain spaces/parens — parse from the last `)`.
 */
function procStartEpochS(pid: number): number | null {
  let raw: string;
  try {
    raw = fs.readFileSync(`/proc/${pid}/stat`, 'utf8');
  } catch {
    return null;
  }
  const close = raw.lastIndexOf(')');
  if (close === -1) return null;
  const fields = raw.slice(close + 2).split(' ');
  // fields[0] = state (field 3) … fields[19] = starttime (field 22).
  const startJiffies = Number(fields[19]);
  const boot = bootEpochS();
  if (!Number.isFinite(startJiffies) || boot === null) return null;
  return boot + startJiffies / CLK_TCK;
}

/** Default POSIX implementation; tests inject fakes via `PidCheck`. */
export const posixPidCheck: PidCheck = (pid, startedAt) => {
  if (!Number.isInteger(pid) || pid <= 0) {
    return { alive: false, startVerified: null };
  }
  const alive = kill0(pid);
  if (!alive) return { alive: false, startVerified: null };
  if (!startedAt) return { alive: true, startVerified: null };
  const start = procStartEpochS(pid);
  const expect = Date.parse(startedAt) / 1000;
  if (start === null || !Number.isFinite(expect)) {
    return { alive: true, startVerified: null };
  }
  return {
    alive: true,
    startVerified: Math.abs(start - expect) <= PID_START_TOLERANCE_S,
  };
};
