//! U3 (plan 2026-08-16-1015): target routing static lint tests.
//!
//! Validates `EventSchema.required_target_hat` contract declarations:
//! - `terminal_target_not_registered` — topic declares contract but no hat subscribes
//! - `terminal_target_consumer_mismatch` — declared hat doesn't match registered consumer
//! - `terminal_target_contract_empty_string` — defensive `required_target_hat = ""` fallback

use std::collections::HashMap;

use crate::config::{EventPolicyConfig, EventSchema, RalphConfig};
use crate::preset_lint::target_routing::check_target_routing;
use crate::preset_lint::LintSeverity;

/// Build a minimal config with two hats that can be wired as
/// upstream/downstream for the tests.
fn minimal_config() -> RalphConfig {
    let mut hats = HashMap::new();
    hats.insert(
        "executor".to_string(),
        crate::config::HatConfig {
            name: "Executor".to_string(),
            triggers: vec!["work.start".to_string()],
            publishes: vec!["report.done".to_string()],
            ..Default::default()
        },
    );
    hats.insert(
        "reporter".to_string(),
        crate::config::HatConfig {
            name: "Reporter".to_string(),
            triggers: vec!["report.done".to_string()],
            publishes: vec![],
            ..Default::default()
        },
    );
    // Coordinator mode forces all HandoffIndex consumer lookups to None,
    // breaking the routing check. Set Isolated so consumer derivation works.
    let mut event_loop = crate::config::EventLoopConfig::default();
    event_loop.execution_mode = crate::config::HatExecutionMode::Isolated;
    RalphConfig { hats, event_loop, ..Default::default() }
}

/// Add a schema for `topic` with `required_target_hat = Some(hat)` to `config`.
fn add_schema_with_required_target(
    config: &mut RalphConfig,
    topic: &str,
    required_target_hat: Option<&str>,
) {
    let policy = config.event_loop.event_policy.get_or_insert_with(EventPolicyConfig::default);
    let mut schema = EventSchema::default();
    schema.required_fields = vec!["report".to_string()];
    schema.required_target_hat = required_target_hat.map(String::from);
    policy.schemas.insert(topic.to_string(), schema);
}

/// Test 1: `required_target_hat = ""` (empty-string defensive fallback) → Warn finding.
#[test]
fn target_routing_empty_string_contracts() {
    let mut config = minimal_config();
    add_schema_with_required_target(&mut config, "report.done", Some(""));
    let findings = check_target_routing(&config);
    let ids: Vec<_> = findings.iter().map(|f| f.id).collect();
    assert!(
        ids.contains(&crate::preset_lint::finding_id::FINDING_TERMINAL_TARGET_CONTRACT_EMPTY_STRING),
        "empty-string required_target_hat must produce \
         terminal_target_contract_empty_string Warn finding; got: {findings:?}"
    );
    let finding = findings.iter().find(|f| f.id == crate::preset_lint::finding_id::FINDING_TERMINAL_TARGET_CONTRACT_EMPTY_STRING).unwrap();
    assert_eq!(finding.severity, LintSeverity::Warn, "empty-string finding must be Warn");
    assert!(finding.message.contains("report.done"));
    assert!(finding.message.contains("required_target_hat"));
}

/// Test 2: topic declares `required_target_hat` but no hat subscribes
///         → `terminal_target_not_registered` Error.
#[test]
fn target_routing_topic_declares_contract_but_no_consumer() {
    let mut config = minimal_config();
    // `audit.log` — no hat publishes or triggers it; reporter only triggers `report.done`.
    add_schema_with_required_target(&mut config, "audit.log", Some("reporter"));
    let findings = check_target_routing(&config);
    let ids: Vec<_> = findings.iter().map(|f| f.id).collect();
    assert!(
        ids.contains(&crate::preset_lint::finding_id::FINDING_TERMINAL_TARGET_NOT_REGISTERED),
        "topic with required_target_hat but no consumer must produce \
         terminal_target_not_registered Error; got: {findings:?}"
    );
    let finding = findings.iter().find(|f| f.id == crate::preset_lint::finding_id::FINDING_TERMINAL_TARGET_NOT_REGISTERED).unwrap();
    assert_eq!(finding.severity, LintSeverity::Error);
    assert!(finding.message.contains("audit.log"));
    assert!(finding.message.contains("reporter"));
}

/// Test 3: topic declares `required_target_hat` and the registered consumer
///         matches exactly → no findings.
#[test]
fn target_routing_topic_declares_contract_and_consumer_matches() {
    let mut config = minimal_config();
    // `report.done` is published by executor and required_target_hat = "reporter".
    // The HandoffIndex derives reporter as the unique consumer.
    add_schema_with_required_target(&mut config, "report.done", Some("reporter"));
    let findings = check_target_routing(&config);
    assert!(
        findings.is_empty(),
        "matching consumer must produce no findings; got: {findings:?}"
    );
}

/// Test 4: topic declares `required_target_hat` but the registered consumer
///         is a different hat → `terminal_target_consumer_mismatch` Error.
#[test]
fn target_routing_topic_declares_contract_and_consumer_mismatches() {
    let mut config = minimal_config();
    // Add a second downstream hat that triggers a *different* topic.
    // `audit.done` is published by executor and only triggered by auditor
    // (reporter does NOT trigger it), making auditor the unique consumer.
    config.hats.insert(
        "auditor".to_string(),
        crate::config::HatConfig {
            name: "Auditor".to_string(),
            triggers: vec!["audit.done".to_string()],
            publishes: vec![],
            ..Default::default()
        },
    );
    // executor publishes `audit.done` (via publishes inheritance from minimal_config)
    // but minimal_config's executor only publishes `report.done`. Add `audit.done`.
    config.hats.get_mut("executor").unwrap().publishes.push("audit.done".to_string());

    // required_target_hat = "reporter" but auditor is the unique consumer.
    add_schema_with_required_target(&mut config, "audit.done", Some("reporter"));
    let findings = check_target_routing(&config);
    let ids: Vec<_> = findings.iter().map(|f| f.id).collect();
    assert!(
        ids.contains(&crate::preset_lint::finding_id::FINDING_TERMINAL_TARGET_CONSUMER_MISMATCH),
        "mismatched consumer must produce terminal_target_consumer_mismatch Error; got: {findings:?}"
    );
    let finding = findings.iter().find(|f| f.id == crate::preset_lint::finding_id::FINDING_TERMINAL_TARGET_CONSUMER_MISMATCH).unwrap();
    assert_eq!(finding.severity, LintSeverity::Error);
    assert!(finding.message.contains("audit.done"));
    assert!(finding.message.contains("reporter")); // declared
    assert!(finding.message.contains("auditor")); // actual
}

/// Test 5: terminal topics (declared in any hat's `terminal_events`)
///         are exempt from the routing checks — they self-close the
///         loop and have no subscribers by design. The
///         `TerminalTargetGuardStage` enforces the contract at emit time.
#[test]
fn target_routing_terminal_topics_are_exempt() {
    let mut config = minimal_config();
    // Mark `report.done` as a terminal event on the reporter hat.
    // Without the exemption, lint would flag `terminal_target_not_registered`
    // because no hat subscribes to `report.done`.
    config
        .hats
        .get_mut("reporter")
        .unwrap()
        .terminal_events
        .push("report.done".to_string());
    add_schema_with_required_target(&mut config, "report.done", Some("reporter"));
    let findings = check_target_routing(&config);
    assert!(
        findings.is_empty(),
        "terminal topics must be exempt from the routing checks; got: {findings:?}"
    );
}

/// Test 6: a NON-terminal topic with no subscribers must still fire
///         `terminal_target_not_registered` (regression for the
///         terminal-topic exemption: do not over-broaden the waiver).
#[test]
fn target_routing_non_terminal_unregistered_topic_still_fires() {
    let mut config = minimal_config();
    add_schema_with_required_target(&mut config, "audit.log", Some("reporter"));
    let findings = check_target_routing(&config);
    let ids: Vec<_> = findings.iter().map(|f| f.id).collect();
    assert!(
        ids.contains(&crate::preset_lint::finding_id::FINDING_TERMINAL_TARGET_NOT_REGISTERED),
        "non-terminal unregistered topic must still produce NOT_REGISTERED; got: {findings:?}"
    );
}

