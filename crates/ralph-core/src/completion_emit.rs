//! Completion-emit warning helper (U7 of plan
//! `docs/plans/2026-07-04-001-feat-opac-isolated-agent-discipline-plan.md`).
//!
//! After an agent closes a task with `ralph tools task close`, the loop
//! expects a follow-up **completion-class event** (e.g. `work.done`,
//! `review.complete`, `test.passed`). Forgetting that step is the classic
//! "close then silent loop stall" bug pattern. This module derives the
//! set of completion topics a hat is allowed to publish, plus a small
//! JSON-shape helper for the stderr warning so the OPAC Confirm path
//! still tells the agent exactly what to do next.
//!
//! The completion-class topic set is derived from the resolved config
//! (`event_policy.terminal_topics ∪ event_policy.business_topics`) and
//! the hat's `publishes` list — neither side is hand-written, which
//! keeps the surface in lock-step with the lint that already gates
//! preset changes (R13).

use crate::config::RalphConfig;

/// Combine the hat's `publishes` with `event_policy.{terminal,business}_topics`
/// to obtain the topics the hat is expected to emit after closing work.
///
/// Empty when the hat has no publishes, when both policy lists are empty,
/// or when there is no overlap — in all three cases, no warning should
/// fire (a reviewer hat that only publishes review notes is not expected
/// to emit a `work.done`).
pub fn derive_completion_publishes(config: &RalphConfig, hat_id: &str) -> Vec<String> {
    let Some(hat_config) = config.hats.get(hat_id) else {
        return Vec::new();
    };
    let Some(policy) = config.event_loop.event_policy.as_ref() else {
        return Vec::new();
    };
    let allowed_set: std::collections::HashSet<&str> = policy
        .terminal_topics
        .iter()
        .chain(policy.business_topics.iter())
        .map(String::as_str)
        .collect();
    let mut out: Vec<String> = hat_config
        .publishes
        .iter()
        .filter(|topic| allowed_set.contains(topic.as_str()))
        .cloned()
        .collect();
    // Stable order so two warnings on the same hat print identically
    // (helpful for BDD golden assertions and operator diffs).
    out.sort();
    out
}

/// Build the `next_step` hint string the warning carries. Kept short
/// (operator-facing) and stable across versions — agents grep for it.
pub fn next_step_hint(expected_topics: &[String]) -> String {
    if expected_topics.is_empty() {
        return "no completion-class topic is published by this hat; \
                 if work is genuinely done, emit one of the topics the hat \
                 declares (see `ralph inspect loop` → publishes[])"
            .to_string();
    }
    format!(
        "ralph emit <TOPIC> --policy-check -j '<payload>' (pick {}); \
         once --policy-check passes, drop the flag and emit again to land it on the main ledger",
        expected_topics.join(" | ")
    )
}

/// Stable machine-grepable prefix the CLI emits on stderr. BDD fixtures
/// and agent recovery logic match this literal verbatim.
pub const CLOSE_WITHOUT_COMPLETION_PREFIX: &str = "close_without_completion_emit";

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::hat::HatConfig;
    use crate::config::{EventPolicyConfig, RalphConfig};

    fn config_with(
        publishes: Vec<&str>,
        policy_terminal: Vec<&str>,
        policy_business: Vec<&str>,
    ) -> RalphConfig {
        let mut cfg = RalphConfig::default();
        let mut hat = HatConfig::default();
        hat.publishes = publishes.iter().map(|s| s.to_string()).collect();
        cfg.hats.insert("executor".to_string(), hat);
        cfg.event_loop.event_policy = Some(EventPolicyConfig {
            enabled: true,
            terminal_topics: policy_terminal.iter().map(|s| s.to_string()).collect(),
            business_topics: policy_business.iter().map(|s| s.to_string()).collect(),
            ..EventPolicyConfig::default()
        });
        cfg
    }

    #[test]
    fn completion_publishes_intersect_terminal_and_business() {
        let cfg = config_with(
            vec!["work.done", "test.passed", "chat.out"],
            vec!["work.done"],
            vec!["test.passed"],
        );
        let out = derive_completion_publishes(&cfg, "executor");
        assert_eq!(
            out,
            vec!["test.passed".to_string(), "work.done".to_string()]
        );
    }

    #[test]
    fn completion_publishes_empty_when_no_overlap() {
        let cfg = config_with(vec!["chat.out"], vec!["work.done"], vec!["test.passed"]);
        let out = derive_completion_publishes(&cfg, "executor");
        assert!(out.is_empty());
    }

    #[test]
    fn completion_publishes_empty_when_hat_unknown() {
        let cfg = config_with(vec!["work.done"], vec!["work.done"], vec![]);
        let out = derive_completion_publishes(&cfg, "unknown");
        assert!(out.is_empty());
    }

    #[test]
    fn completion_publishes_empty_when_no_publishes() {
        let cfg = config_with(vec![], vec!["work.done"], vec![]);
        let out = derive_completion_publishes(&cfg, "executor");
        assert!(out.is_empty());
    }

    #[test]
    fn next_step_hint_lists_topics() {
        let s = next_step_hint(&vec!["work.done".to_string()]);
        assert!(s.contains("work.done"), "hint missing topic: {s}");
        assert!(s.contains("--policy-check"));
    }

    #[test]
    fn next_step_hint_for_no_topics_points_to_inspect() {
        let s = next_step_hint(&[]);
        assert!(s.contains("ralph inspect loop"));
    }
}
