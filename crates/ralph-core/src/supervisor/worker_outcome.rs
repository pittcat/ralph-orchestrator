//! 2026-07-23-004 plan U4 (R-A3): truth table that turns a
//! worker's process outcome + accepted event stream into a
//! validated slot terminal classification.
//!
//! The worker outcome table from the plan:
//!
//! | exit | event_count | terminal kind | accepted? | reason |
//! |------|-------------|---------------|-----------|--------|
//! | 0    | 0           | none          | no        | empty_worker_result |
//! | 0    | >0          | none          | no        | missing_worker_terminal |
//! | 0    | >0          | done          | yes       | — |
//! | 0    | >0          | failed        | yes       | — |
//! | !=0  | 0           | none          | no        | empty_worker_result |
//! | !=0  | >0          | none          | no        | missing_worker_terminal |
//! | !=0  | >0          | done          | yes       | — |
//! | !=0  | >0          | failed        | yes       | — |
//! | timeout | 0           | none          | no        | worker_timeout |
//! | timeout | >0          | none          | no (partial evidence kept) | missing_worker_terminal |
//! | timeout | >0          | done/failed   | yes       | (terminal wins: Completed) |
//! | cancel | any        | none          | no        | worker_cancelled |
//!
//! Distinct error reasons are kept stable so the dispatcher
//! writes the same string into
//! `record_slot_failure(..., reason=...)` regardless of the
//! store backend.

use std::fmt;

/// Stable reason codes matching the plan's "Frozen Failure
/// Reasons" table.
pub const REASON_EMPTY_WORKER_RESULT: &str = "empty_worker_result";
pub const REASON_MISSING_WORKER_TERMINAL: &str = "missing_worker_terminal";
pub const REASON_CONFLICTING_WORKER_TERMINAL: &str = "conflicting_worker_terminal";
pub const REASON_WORKER_TIMEOUT: &str = "worker_timeout";
pub const REASON_WORKER_CANCELLED: &str = "worker_cancelled";
/// 2026-07-25-004 plan U4 (R4 / R5 / AE4): a slot that was
/// registered but never reached `Dispatched`/`Running` before
/// the wave was marked `Failed`. Distinct from `worker_timeout`
/// (for slots that did dispatch but never reported a terminal).
pub const REASON_SLOT_NEVER_STARTED: &str = "slot_never_started";

/// Terminal kind inferred from the worker event stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerTerminalKind {
    /// worker emitted at least one `*.unit.done` event.
    Done,
    /// worker emitted at least one `*.unit.failed` event.
    Failed,
    /// worker did not emit a terminal topic.
    Missing,
}

impl WorkerTerminalKind {
    pub fn from_events(events: &[TerminalMarker]) -> WorkerTerminalKind {
        // 2026-07-23-004 plan U4 A3.3: first-terminal-wins.
        // Two distinct terminal kinds in the same batch is a
        // conflict (the second one is rejected); the keeper
        // is whichever kind lands first. We capture that here
        // without picking a winner, letting the dispatcher
        // drive the dedup order if it cares.
        let mut seen_done = false;
        let mut seen_failed = false;
        for marker in events {
            match marker {
                TerminalMarker::Done => {
                    if seen_failed {
                        return WorkerTerminalKind::Failed;
                    }
                    seen_done = true;
                }
                TerminalMarker::Failed => {
                    if seen_done {
                        return WorkerTerminalKind::Done;
                    }
                    seen_failed = true;
                }
            }
        }
        match (seen_done, seen_failed) {
            (true, _) => WorkerTerminalKind::Done,
            (false, true) => WorkerTerminalKind::Failed,
            _ => WorkerTerminalKind::Missing,
        }
    }
}

/// Tagged marker for a terminal-emission observation. The
/// production code substitutes one of these for each
/// `(unit.done, unit.failed)` event the worker emitted; tests
/// can plug arbitrary sequences into the truth table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TerminalMarker {
    Done,
    Failed,
}

/// Worker process exit status reported back by the dispatcher.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerExit {
    Exit0,
    ExitNonZero,
    Timeout,
    Cancelled,
}

impl WorkerExit {
    pub fn is_timeout(self) -> bool {
        matches!(self, WorkerExit::Timeout)
    }
    pub fn is_cancelled(self) -> bool {
        matches!(self, WorkerExit::Cancelled)
    }
}

/// Validated outcome returned by [`classify_worker_outcome`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlotOutcome {
    /// slot completes successfully.
    Completed(WorkerTerminalKind),
    /// slot fails with a structured reason.
    Failed { reason: &'static str },
}

impl fmt::Display for SlotOutcome {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            SlotOutcome::Completed(kind) => write!(f, "completed:{:?}", kind),
            SlotOutcome::Failed { reason } => write!(f, "failed:{}", reason),
        }
    }
}

/// Classify a worker outcome into a slot result, applying the
/// U4 truth table.
pub fn classify_worker_outcome(
    exit: WorkerExit,
    accepted_event_count: usize,
    terminals: &[TerminalMarker],
) -> SlotOutcome {
    // Cancel and timeout are higher-priority than the empty /
    // missing-terminal checks because they are about the
    // dispatcher's lease on the worker, not about the
    // worker's output.
    //
    // 2026-07-23-007 plan U3 (R-W4): cancel always wins — even if
    // a Done marker slipped through before the cancel, a cancelled
    // worker's terminal must be `worker_cancelled`, not Completed.
    if exit.is_cancelled() {
        return SlotOutcome::Failed {
            reason: REASON_WORKER_CANCELLED,
        };
    }
    if exit.is_timeout() {
        // KTD9: three-way split on terminals + accepted_event_count
        if terminals.is_empty() {
            // Empty terminals: distinguish by whether we have events.
            if accepted_event_count == 0 {
                return SlotOutcome::Failed {
                    reason: REASON_WORKER_TIMEOUT,
                };
            }
            // Events seen but no terminal marker emitted → KTD9 row 3
            return SlotOutcome::Failed {
                reason: REASON_MISSING_WORKER_TERMINAL,
            };
        }
        // Non-empty terminals: the worker's own terminal markers
        // take precedence — the timeout only means the dispatcher's
        // lease expired, not that the worker's output is invalid.
        return match WorkerTerminalKind::from_events(terminals) {
            WorkerTerminalKind::Missing => SlotOutcome::Failed {
                // events > 0 but no terminal marker → KTD9
                reason: REASON_MISSING_WORKER_TERMINAL,
            },
            WorkerTerminalKind::Done | WorkerTerminalKind::Failed => {
                SlotOutcome::Completed(WorkerTerminalKind::from_events(terminals))
            }
        };
    }

    if accepted_event_count == 0 {
        // Empty stream — exit 0 or not, both fail.
        return SlotOutcome::Failed {
            reason: REASON_EMPTY_WORKER_RESULT,
        };
    }

    match WorkerTerminalKind::from_events(terminals) {
        WorkerTerminalKind::Missing => SlotOutcome::Failed {
            reason: REASON_MISSING_WORKER_TERMINAL,
        },
        WorkerTerminalKind::Done | WorkerTerminalKind::Failed => {
            SlotOutcome::Completed(WorkerTerminalKind::from_events(terminals))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_a3_1_exit_0_zero_events_fails_empty() {
        let out = classify_worker_outcome(WorkerExit::Exit0, 0, &[]);
        assert_eq!(
            out,
            SlotOutcome::Failed {
                reason: REASON_EMPTY_WORKER_RESULT
            }
        );
    }

    #[test]
    fn table_a3_2_exit_0_with_events_no_terminal_fails_missing() {
        let out = classify_worker_outcome(
            WorkerExit::Exit0,
            3,
            &[TerminalMarker::Done, TerminalMarker::Done],
        );
        // Plan says "non-terminal events only": we feed
        // TerminalMarker::Done as a stand-in for "non-terminal
        // coverage"; the worker DID see a terminal, so the
        // expected behavior is Completed. We assert the
        // missing branch with zero-terminal events directly.
        let out2 = classify_worker_outcome(WorkerExit::Exit0, 1, &[]);
        assert_eq!(
            out2,
            SlotOutcome::Failed {
                reason: REASON_MISSING_WORKER_TERMINAL
            }
        );
        // Demonstrate that a Done marker with `events > 0` is
        // accepted (this is the R-A3 success path):
        assert_eq!(out, SlotOutcome::Completed(WorkerTerminalKind::Done));
    }

    #[test]
    fn table_a3_3_first_terminal_wins_for_done_then_failed() {
        let out = classify_worker_outcome(
            WorkerExit::ExitNonZero,
            4,
            &[TerminalMarker::Done, TerminalMarker::Failed],
        );
        // "Done came first → Completed(WorkerTerminalKind::Done)"
        assert_eq!(out, SlotOutcome::Completed(WorkerTerminalKind::Done));
    }

    /// U2 (KTD9): Timeout + non-empty terminals → Completed
    /// (terminal wins over the dispatcher's lease expiry).
    #[test]
    fn table_a3_4_timeout_with_done_marker_completes() {
        let out = classify_worker_outcome(WorkerExit::Timeout, 5, &[TerminalMarker::Done]);
        assert_eq!(out, SlotOutcome::Completed(WorkerTerminalKind::Done));
    }

    /// U1 characterization — U2 will flip this assertion: after U2 lands,
    /// Timeout + zero accepted events MUST resolve to
    /// `SlotOutcome::Completed(WorkerTerminalKind::Missing)` (exit=0, no terminal,
    /// partial evidence kept), not `SlotOutcome::Failed { reason: REASON_WORKER_TIMEOUT }`.
    /// Do NOT change the assertion; change the comment to match the flip.
    #[test]
    fn timeout_zero_events_is_timeout_not_empty() {
        let out = classify_worker_outcome(WorkerExit::Timeout, 0, &[]);
        assert_eq!(
            out,
            SlotOutcome::Failed {
                reason: REASON_WORKER_TIMEOUT
            }
        );
    }

    #[test]
    fn cancel_zero_events_is_cancelled_not_empty() {
        let out = classify_worker_outcome(WorkerExit::Cancelled, 0, &[]);
        assert_eq!(
            out,
            SlotOutcome::Failed {
                reason: REASON_WORKER_CANCELLED
            }
        );
    }

    #[test]
    fn exit_nonzero_with_terminal_still_completes() {
        let out = classify_worker_outcome(WorkerExit::ExitNonZero, 1, &[TerminalMarker::Failed]);
        assert_eq!(out, SlotOutcome::Completed(WorkerTerminalKind::Failed));
    }

    #[test]
    fn done_only_completes() {
        let out = classify_worker_outcome(WorkerExit::Exit0, 7, &[TerminalMarker::Done]);
        assert_eq!(out, SlotOutcome::Completed(WorkerTerminalKind::Done));
    }

    /// 2026-07-23-007 plan U3 (R-W4): cancel with a Done marker
    /// already in the event stream — the cancel classification
    /// MUST still win. The legacy implementation required
    /// zero events AND zero terminals for `worker_cancelled`,
    /// which let a Done marker slip through and lift the slot
    /// to Completed. The new contract always returns
    /// `worker_cancelled` for `WorkerExit::Cancelled`.
    #[test]
    fn cancel_with_done_marker_still_returns_worker_cancelled() {
        let out = classify_worker_outcome(
            WorkerExit::Cancelled,
            5,
            &[TerminalMarker::Done, TerminalMarker::Done],
        );
        assert_eq!(
            out,
            SlotOutcome::Failed {
                reason: REASON_WORKER_CANCELLED
            }
        );
    }

    // --- U2 acceptance tests (KTD9 / R1 / R2 / R-W4) ---

    /// AE1: Timeout + Done marker → Completed(Done).
    #[test]
    fn u2_timeout_with_done_marker_completes() {
        let out = classify_worker_outcome(WorkerExit::Timeout, 5, &[TerminalMarker::Done]);
        assert_eq!(out, SlotOutcome::Completed(WorkerTerminalKind::Done));
    }

    /// AE2: Timeout + Failed marker → Completed(Failed).
    #[test]
    fn u2_timeout_with_failed_marker_completes_as_failed_terminal() {
        let out = classify_worker_outcome(WorkerExit::Timeout, 7, &[TerminalMarker::Failed]);
        assert_eq!(out, SlotOutcome::Completed(WorkerTerminalKind::Failed));
    }

    /// KTD9 row 1: Timeout + zero events + empty terminals → worker_timeout.
    #[test]
    fn u2_timeout_zero_events_is_worker_timeout() {
        let out = classify_worker_outcome(WorkerExit::Timeout, 0, &[]);
        assert_eq!(
            out,
            SlotOutcome::Failed {
                reason: REASON_WORKER_TIMEOUT
            }
        );
    }

    /// KTD9 row 3: Timeout + events but no terminal marker → missing_worker_terminal.
    #[test]
    fn u2_timeout_events_but_no_terminal_marker_is_missing_worker_terminal() {
        let out = classify_worker_outcome(WorkerExit::Timeout, 4, &[]);
        assert_eq!(
            out,
            SlotOutcome::Failed {
                reason: REASON_MISSING_WORKER_TERMINAL
            }
        );
    }

    /// KTD1 regression: cancel still wins even with a Done marker present.
    #[test]
    fn u2_cancel_still_wins_over_timeout_with_terminal() {
        let out = classify_worker_outcome(WorkerExit::Cancelled, 3, &[TerminalMarker::Done]);
        assert_eq!(
            out,
            SlotOutcome::Failed {
                reason: REASON_WORKER_CANCELLED
            }
        );
    }
}
