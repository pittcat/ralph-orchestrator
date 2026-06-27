//! U3: `run_preset_lint` integration tests.
//!
//! 4 tests covering the full lint pipeline: invalid topic format,
//! whitelist exemption, strict ownership promotion, and deterministic
//! output.

use super::*;
use crate::config::RalphConfig;
use crate::preset_lint::finding_id::FINDING_MULTI_HAT_REQUIRES_ISOLATED;
use crate::runtime_contract::FindingSeverity;

#[test]
fn run_preset_lint_invalid_topic_format_finding() {
    let yaml = r#"
topic_format_whitelist: []
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = run_preset_lint(&config, LintStrictness::Default, false, None);
    let invalid = findings
        .iter()
        .find(|f| f.id == "lint.preset.invalid_topic_format");
    assert!(
        invalid.is_some(),
        "LOOP_COMPLETE must produce invalid_topic_format finding: {:?}",
        findings
    );
    assert_eq!(
        invalid.unwrap().details.get("topic").map(String::as_str),
        Some("LOOP_COMPLETE")
    );
}

#[test]
fn run_preset_lint_whitelist_exempt_topic_is_not_error() {
    // Whitelisted topics should NOT produce any Warn/Error findings.
    // The fixture declares a two-hat chain with a shared trigger
    // surface so WAC R2 (re-emit trap) does not fire (multi-consumer
    // → not a unique handoff), R3 (egress) closes via LOOP_COMPLETE,
    // R4 (handoff pairing) does not apply (no unique consumer), and
    // R5 (asymmetry) does not apply (work.ready has a publisher).
    let yaml = r#"
topic_format_whitelist:
  - LOOP_COMPLETE
tasks:
  enabled: false
hats:
  a:
    name: "A"
    triggers: ["work.start", "work.ready"]
    publishes: ["work.ready", "LOOP_COMPLETE"]
  b:
    name: "B"
    triggers: ["work.ready"]
    publishes: ["work.done", "LOOP_COMPLETE"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings = run_preset_lint(&config, LintStrictness::Default, false, None);
    // No findings at all (Pass findings are filtered out).
    assert!(
        findings.is_empty(),
        "whitelisted LOOP_COMPLETE must produce no lint findings: {:?}",
        findings
    );
}

#[test]
fn run_preset_lint_strict_ownership_promotes_warnings() {
    let yaml = r#"
topic_owners:
  work.done:
    - non_existent
tasks:
  enabled: false
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings_strict = run_preset_lint(&config, LintStrictness::Strict, false, None);
    let owner_err = findings_strict
        .iter()
        .find(|f| f.id == "lint.preset.owner_unknown_hat");
    assert!(owner_err.is_some());
    assert_eq!(owner_err.unwrap().severity, FindingSeverity::Error);

    // Default: owner_unknown_hat is always Error (not affected by strictness).
    let findings_default = run_preset_lint(&config, LintStrictness::Default, false, None);
    let owner_err_default = findings_default
        .iter()
        .find(|f| f.id == "lint.preset.owner_unknown_hat");
    assert!(owner_err_default.is_some());
    assert_eq!(owner_err_default.unwrap().severity, FindingSeverity::Error);
}

#[test]
fn run_preset_lint_deterministic_output() {
    let yaml = r#"
topic_format_whitelist: []
topic_owners:
  alpha.topic:
    - non_existent
  beta.topic:
    - non_existent
tasks:
  enabled: false
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["work.ready"]
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let findings1 = run_preset_lint(&config, LintStrictness::Default, false, None);
    let findings2 = run_preset_lint(&config, LintStrictness::Default, false, None);
    assert_eq!(findings1.len(), findings2.len());
    for (a, b) in findings1.iter().zip(findings2.iter()) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.severity, b.severity);
        assert_eq!(a.details, b.details);
    }
}

// ──────────────────────────────────────────────────────────────────────────
// 2026-06-11-003 plan U1: Multi-hat isolation policy integration tests
// ──────────────────────────────────────────────────────────────────────────

/// YAML builder for N-hat configs with a given `execution_mode` block.
fn yaml_with_n_hats(n: usize, mode_block: &str) -> String {
    let mut hats = String::new();
    for i in 0..n {
        if i > 0 {
            hats.push('\n');
        }
        hats.push_str(&format!(
            "  h{i}:\n    name: \"H{i}\"\n    triggers: [\"work.start\"]\n    publishes: [\"work.done\"]"
        ));
    }
    format!(
        r#"
tasks:
  enabled: false
topic_format_whitelist:
  - LOOP_COMPLETE
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  {mode_block}
hats:
{hats}
"#
    )
}

fn find_multi_hat_finding<'a>(
    findings: &'a [crate::runtime_contract::RuntimeContractFinding],
) -> Option<&'a crate::runtime_contract::RuntimeContractFinding> {
    findings
        .iter()
        .find(|f| f.id == format!("lint.{}", FINDING_MULTI_HAT_REQUIRES_ISOLATED))
}

/// AE1: 3 hats with default (coordinator) mode → no multi-hat finding.
#[test]
fn u1_three_hats_default_mode_run_preset_lint_passes_multi_hat_policy() {
    let yaml = yaml_with_n_hats(3, "");
    let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
    let findings = run_preset_lint(&config, LintStrictness::Default, false, None);
    assert!(
        find_multi_hat_finding(&findings).is_none(),
        "3 hats default mode must not produce multi-hat finding, got: {findings:?}"
    );
}

/// AE2: 4 hats with default mode → multi-hat error finding, details
/// carry `actual=4` and `limit=3`.
#[test]
fn u1_four_hats_default_mode_run_preset_lint_produces_error_with_details() {
    let yaml = yaml_with_n_hats(4, "");
    let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
    let findings = run_preset_lint(&config, LintStrictness::Default, false, None);
    let finding = find_multi_hat_finding(&findings)
        .expect("4 hats default mode must produce multi-hat finding");
    assert_eq!(finding.severity, FindingSeverity::Error);
    assert_eq!(
        finding.details.get("actual").map(String::as_str),
        Some("4"),
        "details.actual must be 4: {:?}",
        finding.details
    );
    assert_eq!(
        finding.details.get("limit").map(String::as_str),
        Some("3"),
        "details.limit must be 3: {:?}",
        finding.details
    );
    assert_eq!(
        finding.details.get("required_mode").map(String::as_str),
        Some("isolated")
    );
    assert!(
        finding.action_hint.is_some(),
        "action_hint must be set on the multi-hat finding"
    );
    let msg = &finding.message;
    assert!(msg.contains("4"));
    assert!(msg.contains("3"));
}

/// AE3: 4 hats with explicit `execution_mode: coordinator` →
/// identical-shape error to the default (coordinator) case.
#[test]
fn u1_four_hats_explicit_coordinator_run_preset_lint_matches_default() {
    let yaml_default = yaml_with_n_hats(4, "");
    let yaml_explicit = yaml_with_n_hats(4, "execution_mode: coordinator");
    let config_default: RalphConfig = serde_yaml::from_str(&yaml_default).unwrap();
    let config_explicit: RalphConfig = serde_yaml::from_str(&yaml_explicit).unwrap();

    let default_findings = run_preset_lint(&config_default, LintStrictness::Default, false, None);
    let explicit_findings = run_preset_lint(&config_explicit, LintStrictness::Default, false, None);

    let default =
        find_multi_hat_finding(&default_findings).expect("default must produce multi-hat finding");
    let explicit = find_multi_hat_finding(&explicit_findings)
        .expect("explicit coordinator must produce multi-hat finding");

    assert_eq!(default.id, explicit.id);
    assert_eq!(default.severity, explicit.severity);
    assert_eq!(default.message, explicit.message);
    assert_eq!(default.details, explicit.details);
}

/// 4 hats with explicit `execution_mode: isolated` → no multi-hat finding.
#[test]
fn u1_four_hats_isolated_mode_run_preset_lint_passes() {
    let yaml = yaml_with_n_hats(4, "execution_mode: isolated");
    let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
    let findings = run_preset_lint(&config, LintStrictness::Default, false, None);
    assert!(
        find_multi_hat_finding(&findings).is_none(),
        "4 hats isolated mode must not produce multi-hat finding, got: {findings:?}"
    );
}

/// AE4: 8 hats including aggregate, observer, and concurrent worker
/// → still counted as 8. R2 forbids filtering by hat kind.
#[test]
fn u1_eight_hats_with_special_kinds_run_preset_lint_counts_all() {
    let yaml = r#"
tasks:
  enabled: false
topic_format_whitelist:
  - LOOP_COMPLETE
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
  reviewer:
    name: "Reviewer"
    triggers: ["work.done"]
    publishes: ["review.passed"]
  observer:
    name: "Observer"
    triggers: ["work.start"]
    publishes: ["observe.signal"]
  aggregator:
    name: "Aggregator"
    triggers: ["work.ready"]
    publishes: ["work.aggregated"]
    aggregate:
      mode: wait_for_all
      timeout: 60
  wave_worker:
    name: "Wave Worker"
    triggers: ["work.start"]
    publishes: ["wave.partial"]
    concurrency: 4
  secondary:
    name: "Secondary"
    triggers: ["work.start"]
    publishes: ["work.secondary"]
  reporter:
    name: "Reporter"
    triggers: ["work.start"]
    publishes: ["LOOP_COMPLETE"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(config.hats.len(), 8, "fixture must declare 8 hats");

    let findings = run_preset_lint(&config, LintStrictness::Default, false, None);
    let finding = find_multi_hat_finding(&findings)
        .expect("8 hats default mode must produce multi-hat finding");
    assert_eq!(finding.severity, FindingSeverity::Error);
    assert_eq!(finding.details.get("actual").map(String::as_str), Some("8"));
}

/// Lint strictness must NOT downgrade the multi-hat finding. Both
/// `Default` and `Strict` produce `Error` severity.
#[test]
fn u1_lint_strictness_does_not_change_multi_hat_severity() {
    let yaml = yaml_with_n_hats(4, "");
    let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();

    let default_findings = run_preset_lint(&config, LintStrictness::Default, false, None);
    let strict_findings = run_preset_lint(&config, LintStrictness::Strict, false, None);

    let default = find_multi_hat_finding(&default_findings)
        .expect("default strictness must produce multi-hat finding");
    let strict = find_multi_hat_finding(&strict_findings)
        .expect("strict strictness must produce multi-hat finding");

    assert_eq!(
        default.severity,
        FindingSeverity::Error,
        "Default strictness must keep multi-hat at Error"
    );
    assert_eq!(
        strict.severity,
        FindingSeverity::Error,
        "Strict strictness must keep multi-hat at Error (no upgrade path)"
    );
    // id and details are identical — strictness does not affect
    // the policy finding's shape at all.
    assert_eq!(default.id, strict.id);
    assert_eq!(default.details, strict.details);
    assert_eq!(default.message, strict.message);
}

/// Runtime contract shape: `source=lint`, `stage=authoring`,
/// stable finding ID, fix hint. Mirrors the contract guarantees
/// from the other lint families.
#[test]
fn u1_runtime_contract_finding_shape_preserved() {
    let yaml = yaml_with_n_hats(5, "");
    let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
    let findings = run_preset_lint(&config, LintStrictness::Default, false, None);
    let finding = find_multi_hat_finding(&findings)
        .expect("5 hats default mode must produce multi-hat finding");
    assert_eq!(finding.source, crate::runtime_contract::FindingSource::Lint);
    assert_eq!(
        finding.stage,
        crate::runtime_contract::FindingStage::Authoring
    );
    assert_eq!(
        finding.id,
        format!("lint.{}", FINDING_MULTI_HAT_REQUIRES_ISOLATED)
    );
    assert!(finding.action_hint.is_some());
    assert!(!finding.message.is_empty());
}
