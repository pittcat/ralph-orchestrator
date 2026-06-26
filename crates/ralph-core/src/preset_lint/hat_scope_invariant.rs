// 2026-06-26 plan U2: hat scope invariant — the lint half of the
// "hat capability set is the single source of truth" mechanism.
//
// Three rules wired together:
// 1. `event_filter.enabled` MUST be `true` in isolated mode. When
//    the filter is disabled the prompt-side scope contract is
//    silently dropped, and the agent can see topics it is not
//    allowed to react to.
// 2. Every topic a hat publishes MUST be covered by an explicit
//    `topic_deny_rules` entry OR appear on the hat's own
//    `exempt` list. Without an explicit deny rule the topic is
//    publishable from any context, defeating the capability set
//    that `publishes` is supposed to enforce.
// 3. Coordinator hats MUST NOT have any of the review-chain
//    topics (`review.dimension.*`, `review.dimensions.complete`,
//    `review.complete`, `plan.complete`, `plan.blocked`) in
//    `event_filter.events`. The coordinator's job is to dispatch
//    the workflow, not to react to the verdict. Leaking these
//    topics into its prompt has historically caused the
//    `ce-executor-serial` "fix.applied" / re-review loop where
//    the coordinator pre-empts the reviewer.
//
// All three rules are `Error` severity — they protect structural
// invariants, not stylistic preferences. The `coordinator_review_leak`
// rule is the most operationally important: the `ce-executor-serial`
// preset has been losing hours of loop time to it on the
// `pittcat-dev` branch in 2026-06.
use std::collections::HashSet;

use crate::config::TopicDenyRule;
use crate::config::RalphConfig;
use crate::preset_lint::finding_id::{
    FINDING_HAT_SCOPE_COORDINATOR_REVIEW_LEAK, FINDING_HAT_SCOPE_EVENT_FILTER_DISABLED,
    FINDING_HAT_SCOPE_TOPIC_DENY_INCOMPLETE,
};
use crate::runtime_contract::{
    FindingSeverity, FindingSource, FindingStage, RuntimeContractFinding,
};

/// Topics that must never appear in a coordinator's `event_filter.events`.
/// The list is fixed; the rule is "if the coordinator sees any of these,
/// the loop will get into a re-review / pre-emption cycle".
const COORDINATOR_FORBIDDEN_TOPICS: &[&str] = &[
    "review.dimension.done",
    "review.dimension.failed",
    "review.dimensions.complete",
    "review.dimensions.failed",
    "review.complete",
    "review.passed",
    "review.failed",
    "plan.complete",
    "plan.blocked",
];

/// Run the hat scope invariant checks against a `RalphConfig`.
///
/// Returns a sorted, deterministic list of `RuntimeContractFinding`
/// entries. Each finding is `Error` severity — these are structural
/// invariants, not style warnings.
///
/// Only fires when the preset is in **isolated** execution mode
/// (multi-hat presets always go isolated, see R1/R4 in the 2026-06-26
/// plan). Coordinator-mode presets opt out of the strict scope
/// contract because their `event_filter` is a soft hint, not a
/// gating boundary.
pub fn check_hat_scope_invariant(config: &RalphConfig) -> Vec<RuntimeContractFinding> {
    use crate::config::HatExecutionMode;
    let isolated = matches!(config.event_loop.execution_mode, HatExecutionMode::Isolated);
    if !isolated {
        return Vec::new();
    }

    let mut findings = Vec::new();
    let coordinator_hats = coordinator_hat_ids(config);

    for (hat_id, hat_cfg) in &config.hats {
        let is_coordinator = coordinator_hats.contains(hat_id);

        // Rule 1: event_filter must be enabled in isolated mode.
        match hat_cfg.event_filter.as_ref() {
            Some(ef) if ef.enabled => {}
            _ => findings.push(event_filter_disabled_finding(hat_id)),
        }

        // Rule 2: every publishes topic must be covered by an
        // explicit deny rule (or the hat's own exempt list).
        let exempt: HashSet<&str> = hat_cfg
            .exempt_topics
            .iter()
            .map(|s| s.as_str())
            .collect();
        let deny = deny_for_hat(config, hat_id);
        for topic in &hat_cfg.publishes {
            if exempt.contains(topic.as_str()) {
                continue;
            }
            if !deny.contains(topic.as_str()) {
                findings.push(topic_deny_incomplete_finding(hat_id, topic));
            }
        }

        // Rule 3: coordinator must not see review-chain topics.
        if is_coordinator {
            if let Some(ef) = hat_cfg.event_filter.as_ref() {
                for topic in &ef.events {
                    if COORDINATOR_FORBIDDEN_TOPICS.contains(&topic.as_str()) {
                        findings.push(coordinator_review_leak_finding(hat_id, topic));
                    }
                }
            }
        }
    }

    findings
}

fn coordinator_hat_ids(config: &RalphConfig) -> HashSet<String> {
    let mut set: HashSet<String> = config
        .tasks
        .coordinator_hats
        .iter()
        .cloned()
        .collect();
    // Convention: any hat named `coordinator` (the implicit default
    // role) is always a coordinator, even if `tasks.coordinator_hats`
    // is not configured.
    if config.hats.contains_key("coordinator") {
        set.insert("coordinator".to_string());
    }
    set
}

fn deny_for_hat<'a>(config: &'a RalphConfig, hat_id: &str) -> HashSet<&'a str> {
    config
        .event_loop
        .event_policy
        .as_ref()
        .map(|p| {
            p.topic_deny_rules
                .iter()
                .filter(|r: &&TopicDenyRule| r.hat_id == hat_id)
                .map(|r| r.topic.as_str())
                .collect()
        })
        .unwrap_or_default()
}

fn event_filter_disabled_finding(hat_id: &str) -> RuntimeContractFinding {
    let id = format!("lint.{}", FINDING_HAT_SCOPE_EVENT_FILTER_DISABLED);
    RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Lint,
        FindingSeverity::Error,
        FindingStage::Authoring,
        format!(
            "hat `{hat_id}` has event_filter disabled in isolated mode; \
             the scope invariant requires it to be enabled so the prompt \
             cannot leak topics the hat is not allowed to react to"
        ),
    )
    .expect("Lint source is not the reserved Preflight source")
    .with_detail("hat", hat_id.to_string())
    .with_action_hint(format!(
        "Set `hats.{hat_id}.event_filter.enabled: true` and declare the \
         hat's allowlist in `hats.{hat_id}.event_filter.events`"
    ))
}

fn topic_deny_incomplete_finding(hat_id: &str, topic: &str) -> RuntimeContractFinding {
    let id = format!("lint.{}", FINDING_HAT_SCOPE_TOPIC_DENY_INCOMPLETE);
    RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Lint,
        FindingSeverity::Error,
        FindingStage::Authoring,
        format!(
            "hat `{hat_id}` publishes `{topic}` but the topic is not covered \
             by `event_policy.topic_deny_rules` and is not on the hat's \
             `exempt_topics` list. Without an explicit deny rule, the topic \
             can be published from any context, bypassing the scope invariant."
        ),
    )
    .expect("Lint source is not the reserved Preflight source")
    .with_detail("hat", hat_id.to_string())
    .with_detail("topic", topic.to_string())
    .with_action_hint(format!(
        "Add an explicit `topic_deny_rules` entry for (hat_id={hat_id}, \
         topic={topic}) OR add the topic to `hats.{hat_id}.exempt_topics`"
    ))
}

fn coordinator_review_leak_finding(hat_id: &str, topic: &str) -> RuntimeContractFinding {
    let id = format!("lint.{}", FINDING_HAT_SCOPE_COORDINATOR_REVIEW_LEAK);
    RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Lint,
        FindingSeverity::Error,
        FindingStage::Authoring,
        format!(
            "coordinator hat `{hat_id}` declares `{topic}` in its \
             `event_filter.events`. The coordinator must NOT see the review \
             chain — leaking these topics causes the re-review / pre-emption \
             cycle that the `ce-executor-serial` preset has been losing \
             hours to on the `pittcat-dev` branch in 2026-06."
        ),
    )
    .expect("Lint source is not the reserved Preflight source")
    .with_detail("hat", hat_id.to_string())
    .with_detail("topic", topic.to_string())
    .with_action_hint(format!(
        "Remove `{topic}` from `hats.{hat_id}.event_filter.events`; the \
         coordinator dispatches the workflow, it does not react to verdicts"
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_with_hats(yaml: &str) -> RalphConfig {
        let yaml = format!(
            r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
{hats_yaml}
"#,
            hats_yaml = yaml
        );
        serde_yaml::from_str(&yaml).expect("test config must parse")
    }

    #[test]
    fn happy_path_passes_when_filter_enabled_and_topics_denied() {
        let yaml = r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
    topic_deny_rules:
      - hat_id: worker
        topic: work.done
hats:
  worker:
    name: Worker
    triggers: ["work.start"]
    publishes: ["work.done"]
    event_filter:
      enabled: true
      events: ["work.start"]
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).expect("test config must parse");
        let findings = check_hat_scope_invariant(&cfg);
        assert!(
            findings.is_empty(),
            "expected no findings, got: {findings:#?}"
        );
    }

    #[test]
    fn rule1_fires_when_event_filter_disabled() {
        let cfg = isolated_with_hats(
            r#"
hats:
  worker:
    name: Worker
    triggers: ["work.start"]
    publishes: ["work.done"]
    event_filter:
      enabled: false
"#,
        );
        let findings = check_hat_scope_invariant(&cfg);
        assert!(
            findings
                .iter()
                .any(|f| {
                    f.id == *format!(
                        "lint.{}",
                        FINDING_HAT_SCOPE_EVENT_FILTER_DISABLED
                    )
                }),
            "expected event_filter_disabled finding, got: {findings:#?}"
        );
    }

    #[test]
    fn rule1_fires_when_event_filter_omitted() {
        let cfg = isolated_with_hats(
            r#"
hats:
  worker:
    name: Worker
    triggers: ["work.start"]
    publishes: ["work.done"]
"#,
        );
        let findings = check_hat_scope_invariant(&cfg);
        assert!(
            findings
                .iter()
                .any(|f| {
                    f.id == *format!(
                        "lint.{}",
                        FINDING_HAT_SCOPE_EVENT_FILTER_DISABLED
                    )
                }),
            "omitted event_filter must also fire rule 1"
        );
    }

    #[test]
    fn rule2_fires_when_publishes_topic_not_denied() {
        let cfg = isolated_with_hats(
            r#"
hats:
  worker:
    name: Worker
    triggers: ["work.start"]
    publishes: ["work.done", "work.failed"]
    event_filter:
      enabled: true
      events: ["work.start"]
"#,
        );
        let findings = check_hat_scope_invariant(&cfg);
        let ids: Vec<_> = findings.iter().map(|f| f.id.clone()).collect();
        assert!(
            ids.iter()
                .any(|id| *id == *format!("lint.{}", FINDING_HAT_SCOPE_TOPIC_DENY_INCOMPLETE)),
            "expected topic_deny_incomplete finding for `work.failed`, got: {ids:#?}"
        );
    }

    #[test]
    fn rule2_respects_exempt_topics() {
        let yaml = r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
    topic_deny_rules:
      - hat_id: worker
        topic: work.done
hats:
  worker:
    name: Worker
    triggers: ["work.start"]
    publishes: ["work.done", "work.internal"]
    event_filter:
      enabled: true
      events: ["work.start"]
    exempt_topics: ["work.internal"]
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).expect("test config must parse");
        let findings = check_hat_scope_invariant(&cfg);
        let ids: Vec<_> = findings.iter().map(|f| f.id.clone()).collect();
        assert!(
            !ids
                .iter()
                .any(|id| *id == *format!("lint.{}", FINDING_HAT_SCOPE_TOPIC_DENY_INCOMPLETE)),
            "exempt topic must not fire rule 2: {ids:#?}"
        );
    }

    #[test]
    fn rule3_fires_when_coordinator_sees_review_complete() {
        let cfg = isolated_with_hats(
            r#"
hats:
  coordinator:
    name: Coordinator
    triggers: ["work.start"]
    publishes: ["work.ready"]
    event_filter:
      enabled: true
      events: ["work.start", "review.complete"]
tasks:
  enabled: true
  coordinator_hats: ["coordinator"]
"#,
        );
        let findings = check_hat_scope_invariant(&cfg);
        assert!(
            findings
                .iter()
                .any(|f| {
                    f.id == *format!(
                        "lint.{}",
                        FINDING_HAT_SCOPE_COORDINATOR_REVIEW_LEAK
                    )
                }),
            "expected coordinator_review_leak finding, got: {findings:#?}"
        );
    }

    #[test]
    fn rule3_does_not_fire_for_non_coordinator() {
        // A non-coordinator hat with `review.complete` in its
        // event_filter is intentional (the reviewer needs to see
        // its own output). Rule 3 must NOT fire.
        let cfg = isolated_with_hats(
            r#"
hats:
  reviewer:
    name: Reviewer
    triggers: ["work.ready"]
    publishes: ["review.complete"]
    event_filter:
      enabled: true
      events: ["review.complete"]
"#,
        );
        let findings = check_hat_scope_invariant(&cfg);
        let ids: Vec<_> = findings.iter().map(|f| f.id.clone()).collect();
        assert!(
            !ids
                .iter()
                .any(|id| *id == *format!("lint.{}", FINDING_HAT_SCOPE_COORDINATOR_REVIEW_LEAK)),
            "non-coordinator must not trigger rule 3: {ids:#?}"
        );
    }

    #[test]
    fn rule_skipped_in_coordinator_mode() {
        // Coordinator mode keeps the soft-hint semantics; the
        // rules do not fire regardless of hat config.
        let yaml = r#"
event_loop:
  execution_mode: coordinator
hats:
  worker:
    name: Worker
    triggers: ["work.start"]
    publishes: ["work.done"]
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).expect("coordinator mode parses");
        let findings = check_hat_scope_invariant(&cfg);
        assert!(findings.is_empty(), "coordinator mode must skip: {findings:#?}");
    }
}
