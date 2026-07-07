//! Terminal-closed guard (2026-07-07-002 plan Unit 3).
//!
//! Pure decision: after `completion_honored`, which topics must be frozen vs
//! allowed. No EventLoop wiring in this module.

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TerminalClosedInput<'a> {
    pub completion_honored: bool,
    pub topic: &'a str,
    pub topic_class: TopicClass,
    /// When true, the topic equals the configured completion promise (e.g. LOOP_COMPLETE).
    pub is_completion_promise: bool,
    /// When true, payload is byte-identical to an already-honored terminal-adjacent event.
    pub is_byte_duplicate: bool,
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
    if matches!(
        topic,
        "REVIEW_COMPLETE" | "report.done" | "LOOP_COMPLETE"
    ) {
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
        && (input.is_completion_promise
            || input.topic_class == TopicClass::TerminalAdjacent)
    {
        return TerminalClosedDecision::IgnoreDuplicateTerminal;
    }

    if POST_TERMINAL_FROZEN_TOPICS.contains(&input.topic) {
        return TerminalClosedDecision::RejectPostTerminal;
    }

    if input.topic_class == TopicClass::Business {
        return TerminalClosedDecision::RejectPostTerminal;
    }

    TerminalClosedDecision::Allow
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pre_terminal_business_allowed() {
        let input = TerminalClosedInput {
            completion_honored: false,
            topic: "work.done",
            topic_class: TopicClass::Business,
            is_completion_promise: false,
            is_byte_duplicate: false,
        };
        assert_eq!(
            evaluate_terminal_closed(&input),
            TerminalClosedDecision::Allow
        );
    }

    #[test]
    fn test_post_terminal_work_done_rejected() {
        for topic in ["work.done", "plan.blocked", "REVIEW_COMPLETE", "work.ready"] {
            let input = TerminalClosedInput {
                completion_honored: true,
                topic,
                topic_class: classify_topic(topic),
                is_completion_promise: false,
                is_byte_duplicate: false,
            };
            assert_eq!(
                evaluate_terminal_closed(&input),
                TerminalClosedDecision::RejectPostTerminal,
                "topic {topic} must be post-terminal rejected"
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
        };
        assert_eq!(
            evaluate_terminal_closed(&input),
            TerminalClosedDecision::Allow
        );
    }

    #[test]
    fn test_classify_topic_matches_frozen_set() {
        assert_eq!(classify_topic("work.done"), TopicClass::Business);
        assert_eq!(classify_topic("REVIEW_COMPLETE"), TopicClass::TerminalAdjacent);
        assert_eq!(classify_topic("event.policy_warning"), TopicClass::Diagnostic);
    }
}
