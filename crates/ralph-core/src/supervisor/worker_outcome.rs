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
/// 2026-07-25-005 plan U3: dispatcher could not construct a
/// valid control-plane path for the slot (bad wave kind /
/// missing slot resources). The slot will never succeed;
/// retrying would produce the same failure.
pub const REASON_INVALID_CONTROL_PLANE_PATH: &str = "invalid_control_plane_path";
/// Pre-spawn infrastructure failure: the dispatcher could not atomically
/// register every worker channel, so no slot was started.
pub const REASON_WAVE_CHANNEL_REGISTRATION_FAILED: &str = "wave_channel_registration_failed";

// ── Retry classifier ─────────────────────────────────────────────────────────

/// Reasons that may be retried by the dispatcher.
const RETRYABLE_REASONS: &[&str] = &[
    REASON_WORKER_TIMEOUT,
    REASON_EMPTY_WORKER_RESULT,
    REASON_MISSING_WORKER_TERMINAL,
    REASON_SLOT_NEVER_STARTED,
];

/// Reasons that are permanent failures — retrying is futile.
const NON_RETRYABLE_REASONS: &[&str] = &[
    REASON_CONFLICTING_WORKER_TERMINAL,
    REASON_INVALID_CONTROL_PLANE_PATH,
    REASON_WORKER_CANCELLED,
    // cancel/aggregate wave-level failures
    "aggregate_timeout",
    "aggregate_deadline_exceeded",
    "cancelled",
    "wave_cancelled",
];

/// Classification of a slot failure reason for dispatch decisions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailureClassification {
    /// The dispatcher may retry this slot immediately.
    Retryable,
    /// The slot has reached a permanent terminal failure; no
    /// retry or redrive will change the outcome.
    Permanent,
    /// The reason string is not recognised.  We fail-closed
    /// (Permanent) rather than risk an infinite retry loop on an
    /// unknown failure mode.
    UnknownFailClosed,
}

/// Returns `true` for known retryable slot reasons.
/// Unknown or non-retryable reasons return `false`.
pub fn is_retryable_slot_reason(reason: &str) -> bool {
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        return false;
    }
    RETRYABLE_REASONS.contains(&trimmed)
}

/// Classify a slot failure reason for dispatch decisions.
pub fn classify_failure_reason(reason: &str) -> FailureClassification {
    let trimmed = reason.trim();
    if trimmed.is_empty() {
        return FailureClassification::UnknownFailClosed;
    }
    if RETRYABLE_REASONS.contains(&trimmed) {
        return FailureClassification::Retryable;
    }
    if NON_RETRYABLE_REASONS.contains(&trimmed) {
        return FailureClassification::Permanent;
    }
    FailureClassification::UnknownFailClosed
}

// ── Failure class labels ─────────────────────────────────────────────────────

/// Consumer-facing `failure_class` labels written into the
/// `*.wave.failed` payload's `slot_failures` entries (plan
/// 2026-07-25-005 R7 / KTD7). These are stable strings meant for
/// external consumption by the failure handler and reporter; they
/// are deliberately NOT the same namespace as the frozen internal
/// `REASON_*` codes above.
pub const FAILURE_CLASS_TIMEOUT: &str = "timeout";
pub const FAILURE_CLASS_ORPHAN_OR_EMPTY_RESULT: &str = "orphan_or_empty_result";
pub const FAILURE_CLASS_IDENTITY_MISMATCH: &str = "identity_mismatch";
pub const FAILURE_CLASS_REQUIRED_SLOT_FAILURE: &str = "required_slot_failure";
pub const FAILURE_CLASS_CANCEL: &str = "cancel";
/// Fail-closed bucket: the reason is unrecognised, empty, or a
/// known permanent reason with no consumer-facing class (e.g.
/// `invalid_control_plane_path`). Downstream must treat `unknown`
/// as non-recoverable — it never enters `redrive_slots`.
pub const FAILURE_CLASS_UNKNOWN: &str = "unknown";

/// Map a frozen slot/wave failure reason to a stable external
/// `failure_class` label (plan 2026-07-25-005 KTD7).
///
/// The mapping is aligned with [`classify_failure_reason`]:
/// retryable reasons (see [`RETRYABLE_REASONS`]) always resolve to
/// a concrete class, and anything the classifier would not retry
/// either gets its permanent class (`identity_mismatch`, `cancel`)
/// or fails-closed to [`FAILURE_CLASS_UNKNOWN`]. Unrecognised or
/// empty reasons never produce a concrete class, so an unknown
/// failure mode can neither be mistaken for a recoverable one nor
/// leak free-form text into the consumer contract.
pub fn map_failure_class(reason: &str) -> &'static str {
    match reason.trim() {
        // timeout family (slot-level + wave-level aggregate codes)
        REASON_WORKER_TIMEOUT | "aggregate_timeout" | "aggregate_deadline_exceeded" => {
            FAILURE_CLASS_TIMEOUT
        }
        // empty / orphan output family
        REASON_EMPTY_WORKER_RESULT | REASON_MISSING_WORKER_TERMINAL | REASON_SLOT_NEVER_STARTED => {
            FAILURE_CLASS_ORPHAN_OR_EMPTY_RESULT
        }
        // conflicting terminal identities on the worker channel
        REASON_CONFLICTING_WORKER_TERMINAL => FAILURE_CLASS_IDENTITY_MISMATCH,
        // cancel family (slot-level + wave-level aggregate codes)
        REASON_WORKER_CANCELLED | "cancelled" | "wave_cancelled" => FAILURE_CLASS_CANCEL,
        // wave-level aggregate reason used by the failed payload
        "required_slot_failure" => FAILURE_CLASS_REQUIRED_SLOT_FAILURE,
        // Fail-closed: unrecognised reasons, and known permanent
        // reasons without a consumer class (e.g.
        // REASON_INVALID_CONTROL_PLANE_PATH), collapse to `unknown`.
        _ => FAILURE_CLASS_UNKNOWN,
    }
}

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
    /// 2026-07-25-006 plan U9: idle heartbeat kill (no qualifying
    /// signal for `idle_heartbeat_secs`, weak-cap exhausted, or
    /// events-file flat). Same downstream family as `Timeout`
    /// (`worker_timeout`) but distinct exit so the operator can
    /// grep the reason string for `idle heartbeat exceeded`.
    IdleTimeout,
    Cancelled,
}

impl WorkerExit {
    pub fn is_timeout(self) -> bool {
        matches!(self, WorkerExit::Timeout)
    }
    pub fn is_idle_timeout(self) -> bool {
        matches!(self, WorkerExit::IdleTimeout)
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

    // 2026-07-25-006 plan U9: idle heartbeat kill shares the
    // `worker_timeout` family with the hard ceiling. The two paths
    // only differ in the operator-visible reason string (the wave
    // worker emits `"idle heartbeat exceeded: ..."`; the dispatcher
    // preserves that verbatim), so we mirror the timeout branch to
    // keep downstream consumers blind to the leased-vs-wall-clock
    // distinction. We do NOT collapse IdleTimeout into
    // `is_timeout()` because the dispatcher classifier needs to
    // reason about the two arms separately when wiring the
    // adapter (e.g. for retry-budget tracking).
    if exit.is_idle_timeout() {
        if terminals.is_empty() {
            if accepted_event_count == 0 {
                return SlotOutcome::Failed {
                    reason: REASON_WORKER_TIMEOUT,
                };
            }
            return SlotOutcome::Failed {
                reason: REASON_MISSING_WORKER_TERMINAL,
            };
        }
        return match WorkerTerminalKind::from_events(terminals) {
            WorkerTerminalKind::Missing => SlotOutcome::Failed {
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

    // --- U1 (2026-07-25-005 plan, R7 / KTD7): map_failure_class table ---

    /// Retryable frozen reasons map to concrete consumer classes.
    #[test]
    fn u1_map_failure_class_retryable_reasons_get_concrete_classes() {
        assert_eq!(
            map_failure_class(REASON_WORKER_TIMEOUT),
            FAILURE_CLASS_TIMEOUT
        );
        assert_eq!(
            map_failure_class(REASON_EMPTY_WORKER_RESULT),
            FAILURE_CLASS_ORPHAN_OR_EMPTY_RESULT
        );
        assert_eq!(
            map_failure_class(REASON_MISSING_WORKER_TERMINAL),
            FAILURE_CLASS_ORPHAN_OR_EMPTY_RESULT
        );
        assert_eq!(
            map_failure_class(REASON_SLOT_NEVER_STARTED),
            FAILURE_CLASS_ORPHAN_OR_EMPTY_RESULT
        );
    }

    /// Permanent (non-retryable) frozen reasons map per KTD7.
    #[test]
    fn u1_map_failure_class_permanent_reasons_get_concrete_classes() {
        assert_eq!(
            map_failure_class(REASON_CONFLICTING_WORKER_TERMINAL),
            FAILURE_CLASS_IDENTITY_MISMATCH
        );
        assert_eq!(
            map_failure_class(REASON_WORKER_CANCELLED),
            FAILURE_CLASS_CANCEL
        );
        assert_eq!(map_failure_class("cancelled"), FAILURE_CLASS_CANCEL);
        assert_eq!(map_failure_class("wave_cancelled"), FAILURE_CLASS_CANCEL);
        assert_eq!(
            map_failure_class("aggregate_timeout"),
            FAILURE_CLASS_TIMEOUT
        );
        assert_eq!(
            map_failure_class("aggregate_deadline_exceeded"),
            FAILURE_CLASS_TIMEOUT
        );
    }

    /// Wave-level required-slot-failure reason maps to its own class.
    #[test]
    fn u1_map_failure_class_required_slot_failure_identity() {
        assert_eq!(
            map_failure_class("required_slot_failure"),
            FAILURE_CLASS_REQUIRED_SLOT_FAILURE
        );
    }

    /// Unknown / unrecognised reasons fail-closed to the stable
    /// `unknown` class — including the empty string and known
    /// permanent reasons that have no consumer-facing class.
    #[test]
    fn u1_map_failure_class_unknown_reasons_fail_closed() {
        assert_eq!(map_failure_class(""), FAILURE_CLASS_UNKNOWN);
        assert_eq!(map_failure_class("   "), FAILURE_CLASS_UNKNOWN);
        assert_eq!(
            map_failure_class("some_future_reason"),
            FAILURE_CLASS_UNKNOWN
        );
        assert_eq!(
            map_failure_class(REASON_INVALID_CONTROL_PLANE_PATH),
            FAILURE_CLASS_UNKNOWN
        );
    }

    /// Whitespace around a known reason is tolerated (same trim
    /// semantics as `classify_failure_reason`).
    #[test]
    fn u1_map_failure_class_trims_surrounding_whitespace() {
        assert_eq!(
            map_failure_class("  worker_timeout  "),
            FAILURE_CLASS_TIMEOUT
        );
    }

    /// Alignment with the retry classifier: every retryable reason
    /// resolves to a concrete class, and nothing that resolves to a
    /// concrete class is retryable-unaccounted. Guards against the
    /// two tables drifting apart.
    #[test]
    fn u1_map_failure_class_stays_aligned_with_retry_classifier() {
        let retryable = [
            REASON_WORKER_TIMEOUT,
            REASON_EMPTY_WORKER_RESULT,
            REASON_MISSING_WORKER_TERMINAL,
            REASON_SLOT_NEVER_STARTED,
        ];
        for reason in retryable {
            assert!(
                is_retryable_slot_reason(reason),
                "fixture sanity: {reason} must stay retryable"
            );
            assert_ne!(
                map_failure_class(reason),
                FAILURE_CLASS_UNKNOWN,
                "retryable reason {reason} must never fail-closed to unknown"
            );
        }
        // Unknown class inputs must never be retryable: an unknown
        // failure class must not sneak into `redrive_slots`.
        for reason in ["", "nonsense", REASON_INVALID_CONTROL_PLANE_PATH] {
            assert_eq!(map_failure_class(reason), FAILURE_CLASS_UNKNOWN);
            assert!(!is_retryable_slot_reason(reason));
        }
    }

    // --- 2026-07-25-006 plan U9: idle heartbeat kill mirrors Timeout ---

    /// IdleTimeout + zero events + empty terminals → worker_timeout
    /// (same family as the hard ceiling; the operator-visible reason
    /// string `"idle heartbeat exceeded: ..."` is preserved verbatim
    /// upstream of classify_worker_outcome).
    #[test]
    fn u9_idle_timeout_zero_events_is_worker_timeout() {
        let out = classify_worker_outcome(WorkerExit::IdleTimeout, 0, &[]);
        assert_eq!(
            out,
            SlotOutcome::Failed {
                reason: REASON_WORKER_TIMEOUT
            }
        );
    }

    /// IdleTimeout + Done marker → Completed(Done). The worker's own
    /// terminal marker wins over the idle-kill disposition, mirroring
    /// the hard-timeout branch.
    #[test]
    fn u9_idle_timeout_with_done_marker_completes() {
        let out = classify_worker_outcome(WorkerExit::IdleTimeout, 5, &[TerminalMarker::Done]);
        assert_eq!(out, SlotOutcome::Completed(WorkerTerminalKind::Done));
    }

    /// IdleTimeout + events but no terminal marker → missing_worker_terminal
    /// (same downstream classification as Timeout; U9 does not change the
    /// terminal-missing rule).
    #[test]
    fn u9_idle_timeout_events_but_no_terminal_is_missing_worker_terminal() {
        let out = classify_worker_outcome(WorkerExit::IdleTimeout, 4, &[]);
        assert_eq!(
            out,
            SlotOutcome::Failed {
                reason: REASON_MISSING_WORKER_TERMINAL
            }
        );
    }

    /// `is_idle_timeout` predicate gate — used by the dispatcher
    /// classifier to wire the IdleTimeout branch without code
    /// duplication.
    #[test]
    fn u9_is_idle_timeout_predicate() {
        assert!(WorkerExit::IdleTimeout.is_idle_timeout());
        assert!(!WorkerExit::Timeout.is_idle_timeout());
        assert!(!WorkerExit::Cancelled.is_idle_timeout());
        assert!(!WorkerExit::Exit0.is_idle_timeout());
        assert!(!WorkerExit::ExitNonZero.is_idle_timeout());
    }
}
