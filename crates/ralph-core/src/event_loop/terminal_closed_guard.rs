//! Terminal-closed guard (2026-07-07-002 plan Unit 3; 2026-07-07-003 fix).
//!
//! Pure decision: after `completion_honored`, which topics must be frozen vs
//! allowed. The post-completion *business* freeze is configurable via
//! `event_policy.completion_after_terminal.business_after_completion`:
//! `Reject` still freezes; `Warn` / `Ignore` fall through to the existing
//! `check_completion_guard` so it can publish the configured warning or
//! ignore-with-diagnostic. No `EventLoop` wiring in this module.

use crate::config::CompletionAfterTerminalAction;
use crate::event_loop::accepted_event::TopicClass;
use serde::{Deserialize, Serialize};

/// Outcome of the terminal-closed guard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TerminalClosedDecision {
    /// Topic may proceed to downstream commit gates.
    Allow,
    /// Business or terminal-adjacent topic after loop terminal — hard reject.
    RejectPostTerminal,
    /// Duplicate terminal-adjacent emit when completion already honored.
    IgnoreDuplicateTerminal,
}

/// Input for the terminal-closed guard.
#[derive(Debug, Clone, PartialEq)]
pub struct TerminalClosedInput<'a> {
    pub completion_honored: bool,
    pub topic: &'a str,
    pub topic_class: TopicClass,
    /// When true, the topic equals the configured completion promise (e.g. LOOP_COMPLETE).
    pub is_completion_promise: bool,
    /// When true, payload is byte-identical to an already-honored terminal-adjacent event.
    pub is_byte_duplicate: bool,
    /// Policy action for business topics after completion (`Warn` / `Ignore` /
    /// `Reject`). Only consulted for `TopicClass::Business`; non-business
    /// topics and pre-completion events ignore it. The default `Reject`
    /// preserves the 2026-07-07-002 Unit 4 freeze behavior when the runtime
    /// has no policy configuration.
    pub business_after_completion: CompletionAfterTerminalAction,
}

/// Classify a topic into business / terminal-adjacent / control / diagnostic.
#[must_use]
pub fn classify_topic(topic: &str) -> TopicClass {
    if topic.starts_with("event.") || topic.starts_with("human.") || topic == "inspect" {
        return TopicClass::Diagnostic;
    }
    if matches!(
        topic,
        "task.resume"
            | "loop.cancel"
            | "build.task.abandoned"
            | "event.isolation.boundary_violation"
    ) {
        return TopicClass::Control;
    }
    if matches!(topic, "REVIEW_COMPLETE" | "report.done" | "LOOP_COMPLETE") {
        return TopicClass::TerminalAdjacent;
    }
    TopicClass::Business
}

/// Topics frozen after completion is honored (business + handoff + review chain).
const POST_TERMINAL_FROZEN_TOPICS: &[&str] = &[
    "work.ready",
    "work.done",
    "plan.blocked",
    "REVIEW_COMPLETE",
    "report.done",
    "LOOP_COMPLETE",
];

/// Decide whether a candidate may pass the terminal-closed gate.
#[must_use]
pub fn evaluate_terminal_closed(input: &TerminalClosedInput<'_>) -> TerminalClosedDecision {
    if !input.completion_honored {
        return TerminalClosedDecision::Allow;
    }

    if input.topic_class == TopicClass::Diagnostic || input.topic_class == TopicClass::Control {
        return TerminalClosedDecision::Allow;
    }

    if input.is_byte_duplicate
        && (input.is_completion_promise || input.topic_class == TopicClass::TerminalAdjacent)
    {
        return TerminalClosedDecision::IgnoreDuplicateTerminal;
    }

    // Business + frozen-subset topics: the post-completion freeze is
    // configurable. `Reject` keeps the 2026-07-07-002 Unit 4 hard-fail
    // behavior; `Warn` / `Ignore` fall through to the downstream
    // `check_completion_guard` so it can publish the configured warning or
    // ignore-with-diagnostic.
    if input.topic_class == TopicClass::Business
        || POST_TERMINAL_FROZEN_TOPICS.contains(&input.topic)
    {
        return match input.business_after_completion {
            CompletionAfterTerminalAction::Reject => TerminalClosedDecision::RejectPostTerminal,
            CompletionAfterTerminalAction::Warn | CompletionAfterTerminalAction::Ignore => {
                TerminalClosedDecision::Allow
            }
        };
    }

    TerminalClosedDecision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::CompletionAfterTerminalAction;

    fn frozen_input(topic: &str) -> TerminalClosedInput<'_> {
        TerminalClosedInput {
            completion_honored: true,
            topic,
            topic_class: classify_topic(topic),
            is_completion_promise: false,
            is_byte_duplicate: false,
            business_after_completion: CompletionAfterTerminalAction::Reject,
        }
    }

    #[test]
    fn test_pre_terminal_business_allowed() {
        let input = TerminalClosedInput {
            completion_honored: false,
            topic: "work.done",
            topic_class: TopicClass::Business,
            is_completion_promise: false,
            is_byte_duplicate: false,
            business_after_completion: CompletionAfterTerminalAction::Reject,
        };
        assert_eq!(
            evaluate_terminal_closed(&input),
            TerminalClosedDecision::Allow
        );
    }

    #[test]
    fn test_post_terminal_work_done_rejected() {
        for topic in ["work.done", "plan.blocked", "REVIEW_COMPLETE", "work.ready"] {
            let input = frozen_input(topic);
            assert_eq!(
                evaluate_terminal_closed(&input),
                TerminalClosedDecision::RejectPostTerminal,
                "topic {topic} must be post-terminal rejected under Reject action"
            );
        }
    }

    #[test]
    fn test_post_terminal_duplicate_loop_complete_ignored() {
        let input = TerminalClosedInput {
            completion_honored: true,
            topic: "LOOP_COMPLETE",
            topic_class: TopicClass::TerminalAdjacent,
            is_completion_promise: true,
            is_byte_duplicate: true,
            business_after_completion: CompletionAfterTerminalAction::Reject,
        };
        assert_eq!(
            evaluate_terminal_closed(&input),
            TerminalClosedDecision::IgnoreDuplicateTerminal
        );
    }

    #[test]
    fn test_post_terminal_diagnostic_allowed() {
        let input = TerminalClosedInput {
            completion_honored: true,
            topic: "event.completion.blocked",
            topic_class: TopicClass::Diagnostic,
            is_completion_promise: false,
            is_byte_duplicate: false,
            business_after_completion: CompletionAfterTerminalAction::Reject,
        };
        assert_eq!(
            evaluate_terminal_closed(&input),
            TerminalClosedDecision::Allow
        );
    }

    #[test]
    fn test_post_terminal_control_allowed() {
        let input = TerminalClosedInput {
            completion_honored: true,
            topic: "task.resume",
            topic_class: TopicClass::Control,
            is_completion_promise: false,
            is_byte_duplicate: false,
            business_after_completion: CompletionAfterTerminalAction::Reject,
        };
        assert_eq!(
            evaluate_terminal_closed(&input),
            TerminalClosedDecision::Allow
        );
    }

    #[test]
    fn test_classify_topic_matches_frozen_set() {
        assert_eq!(classify_topic("work.done"), TopicClass::Business);
        assert_eq!(
            classify_topic("REVIEW_COMPLETE"),
            TopicClass::TerminalAdjacent
        );
        assert_eq!(
            classify_topic("event.policy_warning"),
            TopicClass::Diagnostic
        );
    }

    // 2026-07-07-003 fix: business + frozen-subset topics fall through
    // to `check_completion_guard` when the operator configured
    // `Warn` / `Ignore`. The earlier 2026-07-07-002 hard-freeze for
    // these actions was a regression: existing presets that relied on
    // `Warn` (e.g. default `RalphConfig`) lost their post-completion
    // business routing.
    #[test]
    fn test_post_terminal_business_warn_action_allows_through() {
        for topic in ["work.done", "plan.blocked", "experiment.planned"] {
            let input = TerminalClosedInput {
                completion_honored: true,
                topic,
                topic_class: classify_topic(topic),
                is_completion_promise: false,
                is_byte_duplicate: false,
                business_after_completion: CompletionAfterTerminalAction::Warn,
            };
            assert_eq!(
                evaluate_terminal_closed(&input),
                TerminalClosedDecision::Allow,
                "Warn action must allow {topic} to reach check_completion_guard"
            );
        }
    }

    #[test]
    fn test_post_terminal_business_ignore_action_allows_through() {
        for topic in ["work.done", "plan.blocked", "experiment.planned"] {
            let input = TerminalClosedInput {
                completion_honored: true,
                topic,
                topic_class: classify_topic(topic),
                is_completion_promise: false,
                is_byte_duplicate: false,
                business_after_completion: CompletionAfterTerminalAction::Ignore,
            };
            assert_eq!(
                evaluate_terminal_closed(&input),
                TerminalClosedDecision::Allow,
                "Ignore action must allow {topic} to reach check_completion_guard"
            );
        }
    }

    #[test]
    fn test_post_terminal_business_reject_action_freezes() {
        for topic in ["work.done", "plan.blocked", "experiment.planned"] {
            let input = TerminalClosedInput {
                completion_honored: true,
                topic,
                topic_class: classify_topic(topic),
                is_completion_promise: false,
                is_byte_duplicate: false,
                business_after_completion: CompletionAfterTerminalAction::Reject,
            };
            assert_eq!(
                evaluate_terminal_closed(&input),
                TerminalClosedDecision::RejectPostTerminal,
                "Reject action must freeze {topic}"
            );
        }
    }

    #[test]
    fn test_post_terminal_duplicate_terminal_unaffected_by_warn() {
        let input = TerminalClosedInput {
            completion_honored: true,
            topic: "LOOP_COMPLETE",
            topic_class: TopicClass::TerminalAdjacent,
            is_completion_promise: true,
            is_byte_duplicate: true,
            business_after_completion: CompletionAfterTerminalAction::Warn,
        };
        assert_eq!(
            evaluate_terminal_closed(&input),
            TerminalClosedDecision::IgnoreDuplicateTerminal,
        );
    }

    #[test]
    fn test_post_terminal_duplicate_terminal_unaffected_by_ignore() {
        let input = TerminalClosedInput {
            completion_honored: true,
            topic: "LOOP_COMPLETE",
            topic_class: TopicClass::TerminalAdjacent,
            is_completion_promise: true,
            is_byte_duplicate: true,
            business_after_completion: CompletionAfterTerminalAction::Ignore,
        };
        assert_eq!(
            evaluate_terminal_closed(&input),
            TerminalClosedDecision::IgnoreDuplicateTerminal,
        );
    }

    #[test]
    fn test_post_terminal_duplicate_terminal_unaffected_by_reject() {
        let input = TerminalClosedInput {
            completion_honored: true,
            topic: "LOOP_COMPLETE",
            topic_class: TopicClass::TerminalAdjacent,
            is_completion_promise: true,
            is_byte_duplicate: true,
            business_after_completion: CompletionAfterTerminalAction::Reject,
        };
        assert_eq!(
            evaluate_terminal_closed(&input),
            TerminalClosedDecision::IgnoreDuplicateTerminal,
        );
    }
}
