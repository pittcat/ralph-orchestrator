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
use std::collections::{HashMap, HashSet};

use crate::config::RalphConfig;
use crate::preset_lint::finding_id::{
    FINDING_HAT_SCOPE_COORDINATOR_FORBIDDEN_PUBLISH, FINDING_HAT_SCOPE_COORDINATOR_REVIEW_LEAK,
    FINDING_HAT_SCOPE_EVENT_FILTER_DISABLED, FINDING_HAT_SCOPE_TOPIC_DENY_INCOMPLETE,
    FINDING_HAT_SCOPE_VERDICT_FIELD_UNKNOWN,
};
use crate::runtime_contract::{
    FindingSeverity, FindingSource, FindingStage, RuntimeContractFinding,
};

/// Topics that must never appear in a coordinator's `event_filter.events`.
///
/// The list is fixed. The rationale, per the 2026-06-26 plan U2:
///
/// - `review.dimension.*` and `review.dimensions.complete` are the
///   per-dimension review fan-out. The coordinator does NOT
///   participate in dimension review — it is the workflow
///   dispatcher. Leaking these topics into its prompt has
///   historically caused the `ce-executor-serial` "fix.applied" /
///   re-review loop where the coordinator pre-empts the
///   review-coordinator. (See
///   `docs/solutions/integration-issues/ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md`.)
/// - `review.failed` (the bare negative verdict, used to be emitted
///   upstream) is a sibling of `review.dimension.failed` — same
///   leak hazard.
///
/// Topics that the coordinator DOES need to see (and are therefore
/// NOT in the forbidden list):
///
/// - `review.complete` — the merged final verdict; the coordinator
///   reads its `fix_plan_file` payload to dispatch fix-units.
/// - `plan.complete` / `plan.blocked` — these are the
///   coordinator's own emits; the prompt legitimately references
///   them in instructions.
const COORDINATOR_FORBIDDEN_TOPICS: &[&str] = &[
    "review.dimension.done",
    "review.dimension.failed",
    "review.dimensions.complete",
    "review.dimensions.failed",
    "review.failed",
];

/// Topics that a coordinator hat must NEVER publish.
///
/// - `human.guidance` has been removed from the protocol (see
///   2026-06-18-004 plan U2 / R2-KTD2); the active hat prompt never
///   receives it and any emit is rejected by the runtime scope gate.
/// - `loop.stalled` is owned by loop-level fallback hats (e.g.
///   `progress-steward`); the coordinator publishing it causes the
///   `semantic_gate_violation` stalls seen in the
///   `primary-20260629-120038` run.
const COORDINATOR_FORBIDDEN_PUBLISHES: &[&str] = &["loop.stalled"];

// Topics that **no** hat may publish.
//
// 2026-06-28-005: `human.guidance` was the lone entry; the topic
// was deleted in this same plan so the list is empty. We keep
// the constant so the lint infrastructure stays in place for
// future forbidden topics; the matching tests
// (rule4_fires_when_coordinator_publishes_human_guidance,
// rule3_5_fires_when_non_coordinator_publishes_human_guidance)
// are removed because no rule can fire against an empty list.
const GLOBALLY_FORBIDDEN_PUBLISHES: &[&str] = &[];

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
///
/// 2026-06-26 Root-Cause Review P1 #1: a coordinator-mode multi-hat
/// preset would bypass the entire hat-scope invariant. The
/// current `multi_hat` lint forces every 4+ hat preset to
/// `isolated` (see [`crate::preset_lint::check_multi_hat_isolation`]),
/// so a coordinator-mode preset is by construction ≤3 hats and
/// the `event_filter` field is conventionally a no-op there. We
/// keep this rule isolated-only in this iteration; promoting
/// the check to coordinator mode is `Deferred to Follow-Up Work`
/// in the 2026-06-26 plan.
pub fn check_hat_scope_invariant(config: &RalphConfig) -> Vec<RuntimeContractFinding> {
    use crate::config::HatExecutionMode;
    let isolated = matches!(config.event_loop.execution_mode, HatExecutionMode::Isolated);
    if !isolated {
        return Vec::new();
    }

    let mut findings = Vec::new();
    let coordinator_hats = coordinator_hat_ids(config);

    // 2026-06-26 Root-Cause Review P1 #2: when a preset opts
    // into the typed Verdict path (`verdict_field: Some(...)`),
    // the typed parser returns `MissingField` for every payload
    // that does not carry that exact field. The gate treats
    // that as "not failing" — i.e. `verdict_fail` is silently
    // swallowed if the upstream emit uses a different field
    // name. We do a sanity check here: the configured
    // `verdict_field` MUST equal `verdict` or `pass_or_fail`
    // (the two known aliases), or the lint reports it. A
    // custom field name is allowed but the lint cannot verify
    // it (it is the operator's responsibility to keep the
    // upstream payload in sync).
    if let Some(gate) = &config.event_loop.verdict_gate
        && let Some(vf) = gate.verdict_field.as_deref()
            && vf != "verdict" && vf != "pass_or_fail" {
                findings.push(verdict_field_known_alias_finding(vf));
            }

    for (hat_id, hat_cfg) in &config.hats {
        let is_coordinator = coordinator_hats.contains(hat_id);

        // Rule 1: event_filter must be enabled in isolated mode.
        match hat_cfg.event_filter.as_ref() {
            Some(ef) if ef.enabled => {}
            _ => findings.push(event_filter_disabled_finding(hat_id)),
        }

        // Rule 2: every publishes topic must be covered by an
        // explicit deny rule (or the hat's own exempt list).
        //
        // 2026-06-26 plan U7: the scope-pinning mechanism is
        // **the hat's own `exempt_topics` list**. The hat
        // declares the topics it WILL emit. A topic on
        // `exempt_topics` is treated as owner-pinned to the
        // hat that declares it. The `topic_deny_rules` are a
        // secondary signal — they bind the topic to a single
        // owner by blocking all other hats.
        //
        // Why we accept the hat's self-declaration:
        //
        // - `topic_deny_rules` is a **negative** schema
        //   (FORBIDS the listed hat from publishing the
        //   topic); using it to model "this hat owns this
        //   topic" requires a deny rule from every other
        //   hat, which is brittle and noisy.
        // - `exempt_topics` is **positive** (DECLARES the
        //   hat's emit scope); a single declaration is
        //   sufficient.
        // - The runtime already enforces the deny rules; the
        //   lint only needs to confirm the scope is
        //   **declared**, not that the runtime will reject
        //   out-of-scope emits. The runtime check is the
        //   primary mechanism.
        //
        // The lint therefore fires only when the topic is
        // neither on the hat's `exempt_topics` list nor pinned
        // to this hat by a deny rule on some other hat.
        let exempt: HashSet<&str> = hat_cfg.exempt_topics.iter().map(|s| s.as_str()).collect();
        // Pinned by `exempt_topics` is the primary signal.
        // We additionally accept the old deny-rule shape
        // (some other hat is denied this topic) as a
        // backwards-compatible alias — presets that have
        // not yet added `exempt_topics` keep working.
        let denied_for_topic = deny_for_all_hats(config);
        for topic in &hat_cfg.publishes {
            if exempt.contains(topic.as_str()) {
                continue;
            }
            let pinned_by_deny = denied_for_topic
                .get(topic.as_str())
                .map(|denied_hats| denied_hats.iter().any(|h| *h != hat_id.as_str()))
                .unwrap_or(false);
            if !pinned_by_deny {
                findings.push(topic_deny_incomplete_finding(hat_id, topic));
            }
        }

        // Rule 3: coordinator must not see review-chain topics.
        if is_coordinator
            && let Some(ef) = hat_cfg.event_filter.as_ref() {
                for topic in &ef.events {
                    if COORDINATOR_FORBIDDEN_TOPICS.contains(&topic.as_str()) {
                        findings.push(coordinator_review_leak_finding(hat_id, topic));
                    }
                }
            }

        // Rule 3.5 (2026-06-29-007 U2): no hat may publish topics
        // that have been removed from the agent protocol. These emits
        // are rejected at runtime by the semantic gate; the lint fails
        // the preset at authoring time so the mistake never reaches a
        // live loop.
        for topic in &hat_cfg.publishes {
            if GLOBALLY_FORBIDDEN_PUBLISHES.contains(&topic.as_str()) {
                findings.push(globally_forbidden_publish_finding(hat_id, topic));
            }
        }
        if let Some(default) = hat_cfg.default_publishes.as_deref()
            && GLOBALLY_FORBIDDEN_PUBLISHES.contains(&default) {
                findings.push(globally_forbidden_publish_finding(hat_id, default));
            }

        // Rule 4 (2026-06-29-007 U2): coordinator must not publish
        // out-of-scope topics. These emits are rejected at runtime by
        // the semantic gate; the lint fails the preset at authoring
        // time so the mistake never reaches a live loop.
        if is_coordinator {
            for topic in &hat_cfg.publishes {
                if COORDINATOR_FORBIDDEN_PUBLISHES.contains(&topic.as_str()) {
                    findings.push(coordinator_forbidden_publish_finding(hat_id, topic));
                }
            }
            if let Some(default) = hat_cfg.default_publishes.as_deref()
                && COORDINATOR_FORBIDDEN_PUBLISHES.contains(&default) {
                    findings.push(coordinator_forbidden_publish_finding(hat_id, default));
                }
        }
    }

    findings
}

fn coordinator_hat_ids(config: &RalphConfig) -> HashSet<String> {
    let mut set: HashSet<String> = config.tasks.coordinator_hats.iter().cloned().collect();
    // Convention: any hat named `coordinator` (the implicit default
    // role) is always a coordinator, even if `tasks.coordinator_hats`
    // is not configured.
    if config.hats.contains_key("coordinator") {
        set.insert("coordinator".to_string());
    }
    set
}

/// 2026-06-26 plan U7: invert the deny lookup. We want to know
/// "for each topic, which hats are explicitly DENIED from
/// publishing it?" — a topic that is denied for some other
/// hat has its emit scope pinned to a single owner.
fn deny_for_all_hats(config: &RalphConfig) -> HashMap<String, HashSet<String>> {
    let mut map: HashMap<String, HashSet<String>> = HashMap::new();
    let Some(p) = config.event_loop.event_policy.as_ref() else {
        return map;
    };
    for r in &p.topic_deny_rules {
        map.entry(r.topic.clone())
            .or_default()
            .insert(r.hat_id.clone());
    }
    map
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

fn coordinator_forbidden_publish_finding(hat_id: &str, topic: &str) -> RuntimeContractFinding {
    let id = format!("lint.{}", FINDING_HAT_SCOPE_COORDINATOR_FORBIDDEN_PUBLISH);
    RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Lint,
        FindingSeverity::Error,
        FindingStage::Authoring,
        format!(
            "coordinator hat `{hat_id}` declares `{topic}` in its publish \
             scope. `{topic}` is owned by loop-level fallback hats (e.g. \
             `progress-steward`); the coordinator publishing it causes \
             `semantic_gate_violation` stalls."
        ),
    )
    .expect("Lint source is not the reserved Preflight source")
    .with_detail("hat", hat_id.to_string())
    .with_detail("topic", topic.to_string())
    .with_action_hint(format!(
        "Remove `{topic}` from `hats.{hat_id}.publishes` (and \
         `default_publishes` if set); only the loop-level fallback hat \
         should publish this topic"
    ))
}

fn globally_forbidden_publish_finding(hat_id: &str, topic: &str) -> RuntimeContractFinding {
    let id = format!("lint.{}", FINDING_HAT_SCOPE_COORDINATOR_FORBIDDEN_PUBLISH);
    RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Lint,
        FindingSeverity::Error,
        FindingStage::Authoring,
        format!(
            "hat `{hat_id}` declares `{topic}` in its publish scope. \
             `{topic}` has been removed from the agent protocol and is \
             reserved for human operators / orchestrator injection; any \
             hat emit is rejected by the runtime scope gate."
        ),
    )
    .expect("Lint source is not the reserved Preflight source")
    .with_detail("hat", hat_id.to_string())
    .with_detail("topic", topic.to_string())
    .with_action_hint(format!(
        "Remove `{topic}` from `hats.{hat_id}.publishes` (and \
         `default_publishes` if set)"
    ))
}

/// 2026-06-26 Root-Cause Review P1 #2: warn the operator when
/// `verdict_gate.verdict_field` is configured to a name that
/// the typed `Verdict::from_payload` parser does not
/// understand. The two known aliases are `verdict` and
/// `pass_or_fail`. Anything else is a footgun: the gate
/// falls through to "not failing" for any payload that
/// does not carry the configured field, which silently
/// masks a `verdict_fail` upstream.
fn verdict_field_known_alias_finding(field: &str) -> RuntimeContractFinding {
    let id = format!("lint.{}", FINDING_HAT_SCOPE_VERDICT_FIELD_UNKNOWN);
    RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Lint,
        FindingSeverity::Warn,
        FindingStage::Authoring,
        format!(
            "verdict_gate.verdict_field is `{field}` which is not one of the \
             known aliases (`verdict` / `pass_or_fail`). Upstream payloads that \
             do not carry the `{field}` field will be treated as 'not failing' \
             by the typed Verdict parser — a typo here silently masks \
             verdict_fail. If `{field}` is intentional, ensure every payload \
             on `{}` and `additional_topics` carries it.",
            field
        ),
    )
    .expect("Lint source is not the reserved Preflight source")
    .with_detail("verdict_field", field.to_string())
    .with_action_hint("Rename `verdict_gate.verdict_field` to `verdict` (preferred) or \
         `pass_or_fail` (legacy). Custom names are accepted but the lint \
         cannot verify upstream payload consistency.".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn isolated_with_hats(yaml: &str) -> RalphConfig {
        let yaml = format!(
            r"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
{hats_yaml}
",
            hats_yaml = yaml
        );
        serde_yaml::from_str(&yaml).expect("test config must parse")
    }

    #[test]
    fn happy_path_passes_when_filter_enabled_and_topics_denied() {
        // `worker` is the owner of `work.done`; `ralph` (some
        // OTHER hat) is denied it. That is what makes the
        // topic scope unambiguous.
        let yaml = r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
    topic_deny_rules:
      - hat_id: ralph
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
                .any(|f| { f.id == *format!("lint.{}", FINDING_HAT_SCOPE_EVENT_FILTER_DISABLED) }),
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
                .any(|f| { f.id == *format!("lint.{}", FINDING_HAT_SCOPE_EVENT_FILTER_DISABLED) }),
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
        // `worker` is the owner of `work.done` and `work.internal`.
        // `ralph` (some OTHER hat) is denied both. `work.internal`
        // is also on `worker.exempt_topics` to make the self-
        // declared scope explicit.
        let yaml = r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
    topic_deny_rules:
      - hat_id: ralph
        topic: work.done
      - hat_id: ralph
        topic: work.internal
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
            !ids.iter()
                .any(|id| *id == *format!("lint.{}", FINDING_HAT_SCOPE_TOPIC_DENY_INCOMPLETE)),
            "exempt topic must not fire rule 2: {ids:#?}"
        );
    }

    #[test]
    fn rule3_fires_when_coordinator_sees_review_complete() {
        // The forbidden set is `review.dimension.*` and
        // `review.dimensions.*` (per-dimension review fan-out).
        // `review.complete` itself is allowed because the
        // coordinator needs its `fix_plan_file` payload to
        // dispatch fix-units. The dimension-level topics are
        // what the lint guards against — they leak the
        // review-chain into the workflow dispatcher.
        let cfg = isolated_with_hats(
            r#"
hats:
  coordinator:
    name: Coordinator
    triggers: ["work.start"]
    publishes: ["work.ready"]
    event_filter:
      enabled: true
      events: ["work.start", "review.dimension.done"]
tasks:
  enabled: true
  coordinator_hats: ["coordinator"]
"#,
        );
        let findings = check_hat_scope_invariant(&cfg);
        assert!(
            findings.iter().any(|f| {
                f.id == *format!("lint.{}", FINDING_HAT_SCOPE_COORDINATOR_REVIEW_LEAK)
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
            !ids.iter()
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
        assert!(
            findings.is_empty(),
            "coordinator mode must skip: {findings:#?}"
        );
    }

    #[test]
    fn rule_p1_2_warns_when_verdict_field_not_known_alias() {
        // 2026-06-26 Root-Cause Review P1 #2: a typo in
        // `verdict_gate.verdict_field` would silently mask
        // verdict_fail (the typed parser returns MissingField
        // for any payload without the field, which the gate
        // treats as "not failing"). The lint must surface
        // this footgun as a Warn.
        let yaml = r#"
event_loop:
  execution_mode: isolated
  verdict_gate:
    topic: "REVIEW_COMPLETE"
    fail_field: "pass_or_fail"
    fail_value: "fail"
    verdict_field: "verdicts"  # typo
hats:
  worker:
    name: Worker
    triggers: ["work.start"]
    publishes: ["work.done"]
    event_filter:
      enabled: true
      events: ["work.start"]
    exempt_topics: ["work.done"]
"#;
        let cfg: RalphConfig = serde_yaml::from_str(yaml).expect("parses");
        let findings = check_hat_scope_invariant(&cfg);
        assert!(
            findings
                .iter()
                .any(|f| f.id == *format!("lint.{}", FINDING_HAT_SCOPE_VERDICT_FIELD_UNKNOWN)),
            "expected verdict_field_unknown warning, got: {findings:#?}"
        );
    }

    #[test]
    fn rule_p1_2_silent_on_known_aliases() {
        // `verdict` and `pass_or_fail` are the two known
        // aliases — neither should warn.
        for vf in ["verdict", "pass_or_fail"] {
            let yaml = format!(
                r#"
event_loop:
  execution_mode: isolated
  verdict_gate:
    topic: "REVIEW_COMPLETE"
    fail_field: "pass_or_fail"
    fail_value: "fail"
    verdict_field: "{}"
hats:
  worker:
    name: Worker
    triggers: ["work.start"]
    publishes: ["work.done"]
    event_filter:
      enabled: true
      events: ["work.start"]
    exempt_topics: ["work.done"]
"#,
                vf
            );
            let cfg: RalphConfig = serde_yaml::from_str(&yaml).expect("parses");
            let findings = check_hat_scope_invariant(&cfg);
            assert!(
                !findings
                    .iter()
                    .any(|f| f.id == *format!("lint.{}", FINDING_HAT_SCOPE_VERDICT_FIELD_UNKNOWN)),
                "known alias `{vf}` must NOT warn, got: {findings:#?}"
            );
        }
    }
}
