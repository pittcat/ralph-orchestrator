//! U3: `run_preset_lint` integration tests.
//!
//! 4 tests covering the full lint pipeline: invalid topic format,
//! whitelist exemption, strict ownership promotion, and deterministic
//! output.

use super::*;
use crate::config::RalphConfig;
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
    let findings = run_preset_lint(&config, LintStrictness::Default);
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
    let yaml = r#"
topic_format_whitelist:
  - LOOP_COMPLETE
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
    let findings = run_preset_lint(&config, LintStrictness::Default);
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
    let findings_strict = run_preset_lint(&config, LintStrictness::Strict);
    let owner_err = findings_strict
        .iter()
        .find(|f| f.id == "lint.preset.owner_unknown_hat");
    assert!(owner_err.is_some());
    assert_eq!(owner_err.unwrap().severity, FindingSeverity::Error);

    // Default: owner_unknown_hat is always Error (not affected by strictness).
    let findings_default = run_preset_lint(&config, LintStrictness::Default);
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
    let findings1 = run_preset_lint(&config, LintStrictness::Default);
    let findings2 = run_preset_lint(&config, LintStrictness::Default);
    assert_eq!(findings1.len(), findings2.len());
    for (a, b) in findings1.iter().zip(findings2.iter()) {
        assert_eq!(a.id, b.id);
        assert_eq!(a.severity, b.severity);
        assert_eq!(a.details, b.details);
    }
}
