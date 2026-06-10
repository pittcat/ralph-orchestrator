//! U2: Ownership & coordinator static rule tests.
//!
//! 16 tests covering R2/R3/R4 ownership rules, R5 coordinator rules,
//! severity mapping, deterministic sorting, and machine-readable
//! finding details.

use super::*;
use crate::config::RalphConfig;

// T1: owner references unknown hat → always error.
#[test]
fn owner_unknown_hat_always_error() {
    let yaml = r#"
topic_owners:
  work.done:
    - non_existent_hat
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_owner_references(&config);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, FINDING_OWNER_UNKNOWN_HAT);
    assert_eq!(findings[0].severity, LintSeverity::Error);
    assert_eq!(findings[0].topic.as_deref(), Some("work.done"));
    assert_eq!(findings[0].owner.as_deref(), Some("non_existent_hat"));
}

// T2: owner does not publish its topic → warn in default, error in strict.
//
// P2 #21 update: when the topic has ZERO publishers at all, R4 fires
// once with FINDING_MISSING_TOPIC_OWNER and the per-owner R2 loop is
// skipped (the umbrella R4 finding names every owner). The single-owner
// case below has no publishers, so the visible finding id changes from
// `FINDING_OWNER_NOT_PUBLISHER` to `FINDING_MISSING_TOPIC_OWNER` —
// `missing_topic_owner_emits_single_combined_finding_not_one_per_owner`
// pins the multi-owner variant. Here we only assert the severity
// expectation that the umbrella finding follows `strictness`.
// See `partial_owners_publish_emits_per_owner_r2_findings` for the
// partial-coverage case where the R2 finding still surfaces.
#[test]
fn owner_not_publisher_warn_default() {
    let yaml = r#"
topic_owners:
  work.done:
    - executor
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    # Does NOT publish work.done
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_ownership_rules(&config, LintStrictness::Default);
    // P2 #21: in the no-publisher-at-all case the umbrella R4 finding
    // replaces the per-owner R2 finding. The message still names the
    // owner ("[executor]") and the action_hint still says what to do,
    // so the operator's information is preserved.
    let r4 = findings
        .iter()
        .find(|f| f.id == FINDING_MISSING_TOPIC_OWNER);
    assert!(
        r4.is_some(),
        "expected FINDING_MISSING_TOPIC_OWNER (P2 #21 umbrella finding); got {:?}",
        findings.iter().map(|f| f.id).collect::<Vec<_>>()
    );
    assert_eq!(r4.unwrap().severity, LintSeverity::Warn);
}

#[test]
fn owner_not_publisher_error_strict() {
    let yaml = r#"
topic_owners:
  work.done:
    - executor
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_ownership_rules(&config, LintStrictness::Strict);
    let r4 = findings
        .iter()
        .find(|f| f.id == FINDING_MISSING_TOPIC_OWNER);
    assert!(r4.is_some());
    assert_eq!(r4.unwrap().severity, LintSeverity::Error);
}

// T3: non-owner publishes owner topic → warn in default, error in strict.
#[test]
fn cross_hat_unauthorized_publish_warn_default() {
    let yaml = r#"
topic_owners:
  work.done:
    - executor
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
  reviewer:
    name: "Reviewer"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_ownership_rules(&config, LintStrictness::Default);
    let f = findings
        .iter()
        .find(|f| f.id == FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH);
    assert!(
        f.is_some(),
        "expected cross_hat_unauthorized_publish finding"
    );
    assert_eq!(f.unwrap().severity, LintSeverity::Warn);
    assert_eq!(f.unwrap().hat.as_deref(), Some("reviewer"));
}

#[test]
fn cross_hat_unauthorized_publish_error_strict() {
    let yaml = r#"
topic_owners:
  work.done:
    - executor
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
  reviewer:
    name: "Reviewer"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_ownership_rules(&config, LintStrictness::Strict);
    let f = findings
        .iter()
        .find(|f| f.id == FINDING_CROSS_HAT_UNAUTHORIZED_PUBLISH);
    assert!(f.is_some());
    assert_eq!(f.unwrap().severity, LintSeverity::Error);
}

// T4: no owner declared → no findings (missing_topic_owner not triggered
//     unless topic_owners has an entry with no publisher).
#[test]
fn no_owner_topics_no_findings() {
    let yaml = r#"
tasks:
  enabled: false
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = validate_ownership_and_coordinator(&config, LintStrictness::Default);
    assert!(findings.is_empty(), "no ownership findings expected");
}

// T5: tasks disabled → no coordinator findings.
#[test]
fn tasks_disabled_no_coordinator_findings() {
    let yaml = r#"
tasks:
  enabled: false
hats:
  executor:
    name: "Executor"
    publishes: ["task.created"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_coordinator_rules(&config);
    assert!(
        findings.is_empty(),
        "no coordinator findings when tasks disabled"
    );
}

// T6: tasks enabled + empty coordinator_hats → error.
#[test]
fn tasks_enabled_empty_coordinator_error() {
    let yaml = r#"
tasks:
  enabled: true
  coordinator_hats: []
hats:
  executor:
    name: "Executor"
    publishes: ["task.created"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_coordinator_rules(&config);
    assert_eq!(findings.len(), 1);
    assert_eq!(findings[0].id, FINDING_COORDINATOR_MISSING);
    assert_eq!(findings[0].severity, LintSeverity::Error);
    // The hint should list candidate hats.
    assert!(
        findings[0]
            .action_hint
            .as_deref()
            .unwrap()
            .contains("executor")
    );
}

// T7: task publisher not in coordinator_hats → error with candidate list.
#[test]
fn task_publisher_not_coordinated() {
    let yaml = r#"
tasks:
  enabled: true
  coordinator_hats:
    - plan-gate
hats:
  executor:
    name: "Executor"
    publishes: ["task.created"]
  plan-gate:
    name: "Plan Gate"
    publishes: ["queue.advance"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_coordinator_rules(&config);
    let f = findings
        .iter()
        .find(|f| f.id == FINDING_TASK_PUBLISHER_NOT_COORDINATED);
    assert!(
        f.is_some(),
        "expected task_publisher_not_coordinated finding"
    );
    assert_eq!(f.unwrap().hat.as_deref(), Some("executor"));
    assert_eq!(f.unwrap().severity, LintSeverity::Error);
}

// T8: task publisher IS in coordinator_hats → no error.
#[test]
fn task_publisher_coordinated_ok() {
    let yaml = r#"
tasks:
  enabled: true
  coordinator_hats:
    - executor
hats:
  executor:
    name: "Executor"
    publishes: ["task.created"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_coordinator_rules(&config);
    assert!(
        findings.is_empty(),
        "no error when task publisher is in coordinator_hats"
    );
}

// T9: valid ownership — owner publishes topic, no non-owner publish.
#[test]
fn valid_ownership_no_findings() {
    let yaml = r#"
topic_owners:
  work.done:
    - executor
tasks:
  enabled: false
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = validate_ownership_and_coordinator(&config, LintStrictness::Default);
    assert!(
        findings.is_empty(),
        "valid ownership should produce no findings"
    );
}

// T10: multiple owners of same topic, all publish → no findings.
#[test]
fn multiple_owners_all_publish() {
    let yaml = r#"
topic_owners:
  work.done:
    - executor
    - reviewer
tasks:
  enabled: false
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
  reviewer:
    name: "Reviewer"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = validate_ownership_and_coordinator(&config, LintStrictness::Default);
    assert!(findings.is_empty());
}

// T11: finding details (topic, hat, owner) are machine-readable.
#[test]
fn finding_details_are_machine_readable() {
    let yaml = r#"
topic_owners:
  work.done:
    - non_existent
hats:
  executor:
    name: "Executor"
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_owner_references(&config);
    let f = &findings[0];
    assert!(f.topic.is_some(), "topic field must be present");
    assert!(f.owner.is_some(), "owner field must be present");
    assert!(f.action_hint.is_some(), "action_hint must be present");
}

// T12: task.* prefix detection does not误把 wildcard trigger 当 publisher.
#[test]
fn task_prefix_only_matches_publishes_not_triggers() {
    let yaml = r#"
tasks:
  enabled: true
  coordinator_hats:
    - plan-gate
hats:
  executor:
    name: "Executor"
    triggers: ["task.created"]  # trigger, not publish
    publishes: ["work.done"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_coordinator_rules(&config);
    // triggers don't count as publishing task.* — no finding expected.
    assert!(
        findings.is_empty(),
        "trigger-only task.* should not produce coordinator finding"
    );
}

// T13: LintStrictness ownership_severity returns correct values.
#[test]
fn strictness_severity_mapping() {
    assert_eq!(
        LintStrictness::Default.ownership_severity(),
        LintSeverity::Warn
    );
    assert_eq!(
        LintStrictness::Strict.ownership_severity(),
        LintSeverity::Error
    );
}

// T14: validate_ownership_and_coordinator returns deterministic sorted order.
#[test]
fn ownership_findings_are_sorted() {
    let yaml = r#"
topic_owners:
  alpha.topic:
    - non_existent_a
  beta.topic:
    - non_existent_b
tasks:
  enabled: false
hats:
  executor:
    name: "Executor"
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = validate_ownership_and_coordinator(&config, LintStrictness::Default);
    // All findings should be sorted by id then topic.
    for window in findings.windows(2) {
        let a = &window[0];
        let b = &window[1];
        assert!(
            (a.id, a.topic.as_deref(), a.hat.as_deref())
                <= (b.id, b.topic.as_deref(), b.hat.as_deref()),
            "findings not sorted: {a:?} > {b:?}"
        );
    }
}

// P2 #21: when a topic has owners but NO publisher hat publishes it,
// R4 fires FINDING_MISSING_TOPIC_OWNER and the previous shape also fired
// FINDING_OWNER_NOT_PUBLISHER for every owner. That is 1 + N redundant
// findings for the same root cause. The fix collapses them into a single
// R4 finding that names every owner in its message + action hint. This
// test pins:
//   - exactly one finding is emitted (1, not 1 + N)
//   - the finding id is FINDING_MISSING_TOPIC_OWNER (the "umbrella" R4)
//   - the message names every owner so the operator has the full picture
#[test]
fn missing_topic_owner_emits_single_combined_finding_not_one_per_owner() {
    let yaml = r#"
topic_owners:
  work.done:
    - alpha
    - bravo
    - charlie
tasks:
  enabled: false
hats:
  unrelated_hat:
    name: "Unrelated"
    publishes: ["other.topic"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_ownership_rules(&config, LintStrictness::Strict);

    // Exactly ONE finding for this topic — no per-owner R2 storm.
    assert_eq!(
        findings.len(),
        1,
        "expected exactly 1 finding for missing publisher, got {}: {:?}",
        findings.len(),
        findings.iter().map(|f| (f.id, &f.message)).collect::<Vec<_>>()
    );
    let f = &findings[0];
    assert_eq!(f.id, FINDING_MISSING_TOPIC_OWNER);
    // Severity follows strictness.ownership_severity (Strict → Error).
    assert_eq!(f.severity, LintSeverity::Error);
    // Every owner must be named in the message so the operator sees the
    // full list without having to re-parse the YAML.
    assert!(f.message.contains("alpha"));
    assert!(f.message.contains("bravo"));
    assert!(f.message.contains("charlie"));
    assert!(f.action_hint.as_deref().unwrap().contains("alpha"));
    assert!(f.action_hint.as_deref().unwrap().contains("bravo"));
    assert!(f.action_hint.as_deref().unwrap().contains("charlie"));
}

// P2 #21 companion test: PARTIAL coverage (one owner publishes, another
// doesn't) must still surface per-owner FINDING_OWNER_NOT_PUBLISHER
// entries — only the "no publisher at all" path collapses into the
// umbrella R4 finding. This pins that R2 is not silently dropped in
// the partial-coverage case.
#[test]
fn partial_owners_publish_emits_per_owner_r2_findings() {
    let yaml = r#"
topic_owners:
  work.done:
    - publisher_hat
    - silent_owner
hats:
  publisher_hat:
    name: "Publisher"
    publishes: ["work.done"]
  silent_owner:
    name: "Silent"
    triggers: ["work.ready"]
    # Does NOT publish work.done
event_loop:
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = check_ownership_rules(&config, LintStrictness::Strict);
    // R4 must NOT fire (there IS a publisher) — only the silent_owner
    // gets a per-owner R2 finding.
    assert!(
        findings.iter().all(|f| f.id != FINDING_MISSING_TOPIC_OWNER),
        "R4 must not fire when at least one owner publishes"
    );
    let silent_finding = findings
        .iter()
        .find(|f| f.id == FINDING_OWNER_NOT_PUBLISHER && f.hat.as_deref() == Some("silent_owner"));
    assert!(
        silent_finding.is_some(),
        "silent_owner must surface a FINDING_OWNER_NOT_PUBLISHER entry in partial-coverage mode"
    );
    // publisher_hat must NOT have any finding against it.
    assert!(
        findings
            .iter()
            .all(|f| f.hat.as_deref() != Some("publisher_hat")),
        "publisher_hat is correctly publishing — no finding should mention it"
    );
}
