//! 2026-06-29-007 plan U5b: `PlanBlockedReason` typed enum
//!
//! Coordinator hat must only emit `plan.blocked` with a
//! `reason` field that matches one of the variants below.
//! Reading `original_trigger_payload` to splice arbitrary
//! strings into the reason field is the 2026-06-28 review
//! chain "scope_violation 字符串污染 plan.blocked reason" 早班
//! pattern. The typed enum is the U5b fix: the reason field
//! is constrained to a closed set so any future
//! scope_violation or recovery text cannot leak into
//! `plan.blocked.reason`.
//!
//! U8 (typed `RejectionKind` shared) reuses this enum as
//! the source of truth for the runtime-side reject reasons.
//! The `as_str()` mapping is kept identical between the
//! two to avoid drift.

use std::fmt;

/// 2026-06-29-007 plan U5b + U8: closed set of allowed
/// `plan.blocked` reason strings. Adding a new variant
/// requires a matching case in `as_str()` so the
/// coordinator hat instructions and the BDD scenario
/// assertion can both pin the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlanBlockedReason {
    /// Stall recovery ran out of retries on a review-chain
    /// retry key (U3 cap).
    HatUnrecoverableAfterRetries,
    /// `flow_lifecycle.phase ∈ {Closed, Failed}` already —
    /// the event_loop is past the point of no return.
    FlowStateClosed,
    /// CoordinatorDecisionGate (U6b) saw a `work.ready`
    /// before `review.complete` was emitted.
    UpstreamReviewIncomplete,
    /// Stall recovery final threshold tripped
    /// (loop_stalled_max_iterations).
    LoopStalledMaxIterations,
    /// IncompleteWaveGate (2026-06-17-002 U2) saw a wave
    /// below the staleness threshold without `dimension.done`
    /// ever arriving.
    DimensionReviewersFailedToConverge,
}

impl PlanBlockedReason {
    /// Stable snake_case label used in the `reason` field.
    /// Mirrors the `as_str()` contract used by
    /// `RejectionKind` in U8.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::HatUnrecoverableAfterRetries => "hat_unrecoverable_after_2_retries",
            Self::FlowStateClosed => "flow_state_closed",
            Self::UpstreamReviewIncomplete => "upstream_review_incomplete",
            Self::LoopStalledMaxIterations => "loop_stalled_max_iterations",
            Self::DimensionReviewersFailedToConverge => "dimension_reviewers_failed_to_converge",
        }
    }

    /// Parse a reason string into the typed enum. Returns
    /// `None` when the string is not in the closed set —
    /// callers (U6b CoordinatorDecisionGate) reject those
    /// events with `reason_invalid`.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "hat_unrecoverable_after_2_retries" => Some(Self::HatUnrecoverableAfterRetries),
            "flow_state_closed" => Some(Self::FlowStateClosed),
            "upstream_review_incomplete" => Some(Self::UpstreamReviewIncomplete),
            "loop_stalled_max_iterations" => Some(Self::LoopStalledMaxIterations),
            "dimension_reviewers_failed_to_converge" => Some(Self::DimensionReviewersFailedToConverge),
            _ => None,
        }
    }
}

impl fmt::Display for PlanBlockedReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn as_str_round_trips_through_parse() {
        let all = [
            PlanBlockedReason::HatUnrecoverableAfterRetries,
            PlanBlockedReason::FlowStateClosed,
            PlanBlockedReason::UpstreamReviewIncomplete,
            PlanBlockedReason::LoopStalledMaxIterations,
            PlanBlockedReason::DimensionReviewersFailedToConverge,
        ];
        for r in all {
            assert_eq!(PlanBlockedReason::parse(r.as_str()), Some(r));
        }
    }

    #[test]
    fn unknown_reason_returns_none() {
        // The 2026-06-29 regression case: a `reason` field
        // built by splicing `original_trigger_payload`
        // strings. This is the exact value that triggered
        // the U5b redesign.
        assert!(PlanBlockedReason::parse(
            "review_never_completed_scope_violation_blocked_review_coordinator"
        )
        .is_none());
        assert!(PlanBlockedReason::parse("").is_none());
        assert!(PlanBlockedReason::parse("random").is_none());
    }
}