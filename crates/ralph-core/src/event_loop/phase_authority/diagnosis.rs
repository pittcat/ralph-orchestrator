//! 2026-07-02-006 plan U26: dual-check diagnosis helper.
//!
//! Pure decision for the R14 invariant: a `plan.complete`
//! event must land on the **main** event bus, never the
//! repair sink. The helper compares the event's `topic`,
//! `source` (when present), and the message channel it
//! arrived on. The runtime surfaces a `plan.complete_dual`
//! diagnostic when the invariant breaks.

use serde::{Deserialize, Serialize};

/// Where the event landed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum Channel {
    Main,
    Repair,
    Unknown,
}

impl Default for Channel {
    fn default() -> Self {
        Channel::Unknown
    }
}

/// Inputs the runtime feeds to the helper.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DualCheckInput {
    pub topic: String,
    pub source: Option<String>,
    pub channel: Channel,
}

/// Outcome of the dual check.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub enum DualCheckOutcome {
    /// `plan.complete` arrived on the main bus.
    Ok,
    /// `plan.complete` arrived on the repair sink — invariant
    /// broken; runtime should emit a `plan.complete_dual`
    /// diagnostic.
    DualSink,
    /// `plan.complete` arrived on an unknown channel —
    /// runtime should log a warning and admit (we cannot
    /// prove the invariant either way).
    UnknownChannel,
    /// The event is not `plan.complete`; the dual check
    /// does not apply.
    NotApplicable,
}

/// Pure decision.
pub fn diagnosis_plan_complete_dual_check(input: &DualCheckInput) -> DualCheckOutcome {
    if input.topic != "plan.complete" {
        return DualCheckOutcome::NotApplicable;
    }
    match input.channel {
        Channel::Main => DualCheckOutcome::Ok,
        Channel::Repair => DualCheckOutcome::DualSink,
        Channel::Unknown => DualCheckOutcome::UnknownChannel,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn non_plan_complete_is_not_applicable() {
        let input = DualCheckInput {
            topic: "plan.blocked".to_string(),
            source: Some("coordinator".to_string()),
            channel: Channel::Repair,
        };
        assert_eq!(
            diagnosis_plan_complete_dual_check(&input),
            DualCheckOutcome::NotApplicable
        );
    }

    #[test]
    fn plan_complete_on_main_is_ok() {
        let input = DualCheckInput {
            topic: "plan.complete".to_string(),
            source: Some("coordinator".to_string()),
            channel: Channel::Main,
        };
        assert_eq!(
            diagnosis_plan_complete_dual_check(&input),
            DualCheckOutcome::Ok
        );
    }

    #[test]
    fn plan_complete_on_repair_is_dual_sink() {
        // R14 invariant broken — the runtime must surface a
        // diagnostic and refuse to forward the emit to
        // REVIEW_COMPLETE.
        let input = DualCheckInput {
            topic: "plan.complete".to_string(),
            source: Some("coordinator".to_string()),
            channel: Channel::Repair,
        };
        assert_eq!(
            diagnosis_plan_complete_dual_check(&input),
            DualCheckOutcome::DualSink
        );
    }

    #[test]
    fn plan_complete_on_unknown_channel_is_warning() {
        let input = DualCheckInput {
            topic: "plan.complete".to_string(),
            source: None,
            channel: Channel::Unknown,
        };
        assert_eq!(
            diagnosis_plan_complete_dual_check(&input),
            DualCheckOutcome::UnknownChannel
        );
    }

    #[test]
    fn plan_complete_without_source_is_still_routed_by_channel() {
        // The source is diagnostic; the channel is the
        // authoritative input. A missing source must not
        // let a `Repair` emit pass.
        let input = DualCheckInput {
            topic: "plan.complete".to_string(),
            source: None,
            channel: Channel::Repair,
        };
        assert_eq!(
            diagnosis_plan_complete_dual_check(&input),
            DualCheckOutcome::DualSink
        );
    }
}