/**
 * Monitor run-state vocabulary (plan D8):
 * `running | silent | exited_success | exited_failed | exited_merged |
 * exited_unknown`.
 *
 * `silent` (running but no recent activity) is intentionally not
 * emitted in this unit — liveness needs the activity ledger from
 * Plan 3. `exited_*` rows are terminal, but only evidenced ones
 * (`exited_success`/`exited_failed` with a terminal record, or
 * `exited_merged`) are final: the projection records a
 * `state_conflict` if they regress to `running`. `exited_failed`
 * also covers runs verified dead without a terminal record
 * (killed/crashed before `record_terminal` ran) — an abnormal,
 * unrecorded exit, not "we don't know". `exited_unknown` is
 * reserved for observations with no process identity at all
 * (corrupt manifest, unrecognised state) — a later live
 * observation legitimately recovers it.
 */
export const MONITOR_STATES = [
  'running',
  'silent',
  'exited_success',
  'exited_failed',
  'exited_merged',
  'exited_unknown',
] as const;

export type MonitorState = (typeof MONITOR_STATES)[number];

export function isMonitorState(v: string): v is MonitorState {
  return (MONITOR_STATES as readonly string[]).includes(v);
}

export function isTerminalState(state: MonitorState): boolean {
  return state.startsWith('exited_');
}

export interface StateEvidence {
  /** Coarse vocabulary reported by `inspect run`. */
  snapshotState: 'running' | 'exited' | 'unknown';
  manifestPresent: boolean;
  manifestCorrupt: boolean;
  registryPresent: boolean;
  lockPresent: boolean;
  mergeState: string | null;
  terminalReason: string | null;
  mergeCommit: string | null;
  pid: number | null;
  /**
   * Set when the state was assigned by the absent-run sweep: the
   * run vanished from the snapshot (all evidence sources gone)
   * and the projection re-verified liveness through its stored
   * pid instead of through a fresh observation.
   */
  absentFromSnapshot?: boolean;
}

export interface ObservationStateInput {
  state: string;
  pid: number | null;
  terminal: { reason: string; at: string; mergeCommit: string | null } | null;
  evidence: {
    manifestPresent: boolean;
    manifestCorrupt: boolean;
    registryPresent: boolean;
    lockPresent: boolean;
    mergeState: string | null;
  };
}

/**
 * Terminal reasons that mean the run delivered successfully
 * (`TerminationReason::as_str()` vocabulary, E10). `completed` is the
 * only unambiguous success; `cancelled`/`interrupted`/`stopped`/
 * `restart_requested` are deliberate non-success terminations and map
 * to `exited_failed` with the raw reason preserved in evidence.
 * A verified-dead pid with no terminal record maps the same way —
 * the process exited abnormally before it could record anything.
 */
const SUCCESS_REASONS = new Set(['completed']);

/**
 * Map one snapshot observation to the monitor state + evidence pair.
 *
 * Ordering (S5): merge evidence beats everything — a run whose merge
 * queue recorded `merged` (or whose terminal carries a merge commit)
 * is `exited_merged` regardless of the termination reason. Next,
 * `completed` → `exited_success`, any other terminal reason →
 * `exited_failed`. A live pid → `running`. A snapshot `unknown`
 * carrying a concrete pid means the inspector verified that process
 * dead (kill(0)) → `exited_failed`; pid-less `unknown` →
 * `exited_unknown` (never fabricate success — R2, and never
 * fabricate liveness either).
 */
export function mapObservationState(input: ObservationStateInput): {
  state: MonitorState;
  evidence: StateEvidence;
} {
  const mergeState = input.evidence.mergeState ?? null;
  const terminalReason = input.terminal?.reason ?? null;
  const mergeCommit = input.terminal?.mergeCommit ?? null;

  const evidence: StateEvidence = {
    snapshotState:
      input.state === 'running' || input.state === 'exited'
        ? input.state
        : 'unknown',
    manifestPresent: input.evidence.manifestPresent,
    manifestCorrupt: input.evidence.manifestCorrupt,
    registryPresent: input.evidence.registryPresent,
    lockPresent: input.evidence.lockPresent,
    mergeState,
    terminalReason,
    mergeCommit,
    pid: input.pid,
  };

  if (input.state === 'running') {
    return { state: 'running', evidence };
  }
  // Merge evidence is authoritative on its own: a `merged` queue row
  // or in-loop merge commit proves the run converged even when the
  // snapshot lacks terminal evidence.
  if (mergeState === 'merged' || mergeCommit !== null) {
    return { state: 'exited_merged', evidence };
  }
  if (input.state === 'exited') {
    if (terminalReason !== null && SUCCESS_REASONS.has(terminalReason)) {
      return { state: 'exited_success', evidence };
    }
    return { state: 'exited_failed', evidence };
  }
  // `unknown` with a recorded pid: the inspector verified that
  // concrete process dead (kill(0)) — a conclusive abnormal exit,
  // not an unknowable one. Only pid-less observations (corrupt
  // manifest, identity-less rows) and unrecognised state strings
  // stay fail-closed `exited_unknown`.
  if (input.state === 'unknown' && input.pid !== null) {
    return { state: 'exited_failed', evidence };
  }
  return { state: 'exited_unknown', evidence };
}
