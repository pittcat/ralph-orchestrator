//! 2026-06-29-007 plan U8: typed `RejectionKind` shared
//! between `missing_event_gate` and `stall_recovery`.
//!
//! Both recovery paths compute a retry key from the
//! rejection reason. Before U8 they used a `from_reason`
//! string fallback that let typos silently drift into
//! new retry keys (each misspelling would be treated as
//! a fresh retry-key, defeating the cap). The typed enum
//! is the SSOT for retry-key names.
//!
//! Migration guide (KTD-7): historical records that lack
//! the `retry_attempt` field are reconciled on startup
//! from the `outcome` value:
//! - escalated → 2
//! - recovered → 1
//! - pending → 0

use std::fmt;

/// 2026-06-29-007 plan U8: closed set of rejection
/// reasons that drive retry-key computation. Adding a
/// variant here is the only way to introduce a new
/// retry key — the legacy string fallback is gone.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RejectionKind {
    MissingEvent,
    StallNoEvents,
    HandoffTimeout,
    ScopeViolation,
    TargetSelfLoop,
    TargetLastHopLoop,
    FlowStateClosed,
    UpstreamReviewIncomplete,
    RetryCap,
    ReasonInvalid,
}

impl RejectionKind {
    /// Stable snake_case label. Used as the second
    /// segment of the retry key (after `source`).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::MissingEvent => "missing_event",
            Self::StallNoEvents => "stall_no_events",
            Self::HandoffTimeout => "handoff_dispatch_timeout",
            Self::ScopeViolation => "scope_violation",
            Self::TargetSelfLoop => "target_self_loop",
            Self::TargetLastHopLoop => "target_last_hop_loop",
            Self::FlowStateClosed => "flow_state_closed",
            Self::UpstreamReviewIncomplete => "upstream_review_incomplete",
            Self::RetryCap => "retry_cap",
            Self::ReasonInvalid => "reason_invalid",
        }
    }

    /// Parse a label back into the typed enum. Returns
    /// `None` for unknown values so callers can detect
    /// drift from old `from_reason` string keys.
    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "missing_event" => Some(Self::MissingEvent),
            "stall_no_events" => Some(Self::StallNoEvents),
            "handoff_dispatch_timeout" => Some(Self::HandoffTimeout),
            "scope_violation" => Some(Self::ScopeViolation),
            "target_self_loop" => Some(Self::TargetSelfLoop),
            "target_last_hop_loop" => Some(Self::TargetLastHopLoop),
            "flow_state_closed" => Some(Self::FlowStateClosed),
            "upstream_review_incomplete" => Some(Self::UpstreamReviewIncomplete),
            "retry_cap" => Some(Self::RetryCap),
            "reason_invalid" => Some(Self::ReasonInvalid),
            _ => None,
        }
    }
}

impl fmt::Display for RejectionKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Compute a retry key from `(source, kind)`. Two
/// reject paths that share the same `(source, kind)`
/// pair share an attempt counter; paths that differ on
/// either field get independent counters.
///
/// Format: `{source}:{kind_label}`. The trailing
/// wildcard (`*`) from the legacy `stall_recovery:`
/// prefix is intentionally dropped — the typed kind
/// already disambiguates, so the wildcard is dead
/// metadata.
#[must_use]
pub fn compute_retry_key(source: &str, kind: RejectionKind) -> String {
    format!("{}:{}", source, kind.as_str())
}

/// Migration helper (KTD-7): recover the
/// `retry_attempt` counter from the legacy
/// `outcome` field of a pre-U8 envelope.
///
/// | outcome   | retry_attempt |
/// | pending   | 0             |
/// | recovered | 1             |
/// | escalated | 2             |
/// | *         | 0 (defensive) |
#[must_use]
pub fn retry_attempt_from_outcome(outcome: &str) -> u8 {
    match outcome {
        "pending" => 0,
        "recovered" => 1,
        "escalated" => 2,
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    #[test]
    fn as_str_round_trips_through_parse() {
        let all = [
            RejectionKind::MissingEvent,
            RejectionKind::StallNoEvents,
            RejectionKind::HandoffTimeout,
            RejectionKind::ScopeViolation,
            RejectionKind::TargetSelfLoop,
            RejectionKind::TargetLastHopLoop,
            RejectionKind::FlowStateClosed,
            RejectionKind::UpstreamReviewIncomplete,
            RejectionKind::RetryCap,
            RejectionKind::ReasonInvalid,
        ];
        for k in all {
            assert_eq!(RejectionKind::parse(k.as_str()), Some(k));
        }
    }

    #[test]
    fn unknown_string_returns_none() {
        // The 2026-06-29 drift case: a typo or legacy
        // `from_reason` string that slipped past the
        // typed gate. U8 catches it.
        assert!(RejectionKind::parse("missing-evt").is_none());
        assert!(RejectionKind::parse("").is_none());
    }

    #[test]
    fn compute_retry_key_shares_counter_per_kind() {
        // Same source + same kind → same retry key
        assert_eq!(
            compute_retry_key("stall_recovery", RejectionKind::StallNoEvents),
            compute_retry_key("stall_recovery", RejectionKind::StallNoEvents)
        );
        // Different kind → different retry key
        assert_ne!(
            compute_retry_key("stall_recovery", RejectionKind::StallNoEvents),
            compute_retry_key("stall_recovery", RejectionKind::MissingEvent)
        );
        // Different source → different retry key
        assert_ne!(
            compute_retry_key("stall_recovery", RejectionKind::MissingEvent),
            compute_retry_key("missing_event_gate", RejectionKind::MissingEvent)
        );
    }

    #[test]
    fn retry_attempt_from_outcome_table() {
        assert_eq!(retry_attempt_from_outcome("pending"), 0);
        assert_eq!(retry_attempt_from_outcome("recovered"), 1);
        assert_eq!(retry_attempt_from_outcome("escalated"), 2);
        assert_eq!(retry_attempt_from_outcome("anything-else"), 0);
    }

    #[test]
    fn all_kinds_have_distinct_strings() {
        let labels: HashSet<&str> = [
            RejectionKind::MissingEvent,
            RejectionKind::StallNoEvents,
            RejectionKind::HandoffTimeout,
            RejectionKind::ScopeViolation,
            RejectionKind::TargetSelfLoop,
            RejectionKind::TargetLastHopLoop,
            RejectionKind::FlowStateClosed,
            RejectionKind::UpstreamReviewIncomplete,
            RejectionKind::RetryCap,
            RejectionKind::ReasonInvalid,
        ]
        .iter()
        .map(|k| k.as_str())
        .collect();
        assert_eq!(labels.len(), 10, "as_str() labels must be unique");
    }
}