//! 2026-07-29-002 plan: precheck gate hat coverage lint tests.
//!
//! The lint under test
//! (`check_precheck_rule_without_synthesized_gate_hat`) inspects the
//! caller's config AS-IS and fires on the half-desugared shape:
//! `event_loop.precheck.rules.<X>` declared with `enabled: true`,
//! `<X>.proposed` already in circulation (evidence the desugar's
//! producer rewrite ran), but no `precheck-<X>` gate hat in the map.
//! It stays silent when:
//! - the gate hat is present (healthy post-normalize state),
//! - precheck is disabled or the rules table is empty,
//! - the runtime kill switch is active,
//! - the config is pre-normalize (producers still publish the bare
//!   `<X>`; `normalize()` will synthesize the gate hat at load).

use super::*;
use crate::config::RalphConfig;
use crate::config::precheck_kill_switch_guard;

// T2: synthesized `precheck-<X>` hat present → no finding.
#[test]
fn synthesized_hat_passes_clean() {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    publishes: ["work.failed", "work.failed.proposed"]
    terminal_events: ["work.failed", "work.failed.proposed"]
  precheck-work.failed:
    name: "Precheck Gate: work.failed"
    triggers: ["work.failed.proposed"]
    publishes: ["work.failed", "work.failed.rejected"]
    terminal_events: ["work.failed", "work.failed.rejected"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  precheck:
    enabled: true
    rules:
      work.failed:
        on_fail:
          target: "executor"
          retry_budget: 3
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.normalize();

    let findings =
        check_precheck_rule_without_synthesized_gate_hat(&config, LintStrictness::Default);
    assert!(
        findings.is_empty(),
        "fixture with synthesized precheck-work.failed gate hat must \
         emit zero findings; got {:?}",
        findings
            .iter()
            .map(|f| (f.id, f.severity))
            .collect::<Vec<_>>()
    );
}

// T3: precheck disabled → rules intentionally inert, no finding.
#[test]
fn precheck_disabled_silences_rule() {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    publishes: ["work.failed"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  precheck:
    enabled: false
    rules:
      work.failed:
        on_fail:
          target: "executor"
          retry_budget: 3
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

    let findings =
        check_precheck_rule_without_synthesized_gate_hat(&config, LintStrictness::Default);
    assert!(
        findings.is_empty(),
        "disabled precheck block is intentionally inert; lint must not flag"
    );
}

// T4: precheck runtime kill switch (RALPH_PRECHECK_MODE=off / test injection)
//     → no finding even on the half-desugared shape. Mirrors the runtime
//     desugar `strict no-op` contract from plan 2026-07-02-004 U2.
#[test]
fn precheck_kill_switch_silences_lint() {
    let _guard = precheck_kill_switch_guard();
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    publishes: ["work.failed.proposed"]
    terminal_events: ["work.failed.proposed"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  precheck:
    enabled: true
    rules:
      work.failed:
        on_fail:
          target: "executor"
          retry_budget: 3
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

    let findings =
        check_precheck_rule_without_synthesized_gate_hat(&config, LintStrictness::Default);
    assert!(
        findings.is_empty(),
        "kill switch must silence the lint so test fixtures and operators \
         who opt out via RALPH_PRECHECK_MODE=off are not falsely flagged"
    );
}

// T5: a normalized idiomatic preset (precheck declared, producers
//     publishing the topic) ends up with a synthesized gate hat and
//     the lint stays silent.
#[test]
fn normalize_produces_gate_hat_clears_finding() {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    publishes: ["work.failed", "work.failed.proposed"]
    terminal_events: ["work.failed", "work.failed.proposed"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  precheck:
    enabled: true
    rules:
      work.failed:
        on_fail:
          target: "executor"
          retry_budget: 3
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.normalize();

    let findings =
        check_precheck_rule_without_synthesized_gate_hat(&config, LintStrictness::Default);
    assert!(
        findings.is_empty(),
        "normalized config must contain the synthesized gate hat and the \
         lint must stay silent; got {:?}",
        findings
            .iter()
            .map(|f| (f.id, f.severity))
            .collect::<Vec<_>>()
    );
}

// T6: firing path — half-desugared shape (producer already emits
//     `work.failed.proposed` but the gate hat is missing) must produce
//     exactly one finding, at `Warn` severity in default mode.
#[test]
fn half_desugared_config_fires_warn_in_default_mode() {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    publishes: ["work.failed.proposed"]
    terminal_events: ["work.failed.proposed"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  precheck:
    enabled: true
    rules:
      work.failed:
        on_fail:
          target: "executor"
          retry_budget: 3
"#;
    // Deliberately NOT normalized: the fixture emulates a config whose
    // desugar producer rewrite ran but the synthesized gate hat was
    // lost (hand-built config or desugar regression).
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

    let findings =
        check_precheck_rule_without_synthesized_gate_hat(&config, LintStrictness::Default);
    assert_eq!(
        findings.len(),
        1,
        "half-desugared shape must produce exactly one finding; got {findings:?}"
    );
    let finding = &findings[0];
    assert_eq!(finding.severity, LintSeverity::Warn);
    assert_eq!(finding.topic.as_deref(), Some("work.failed"));
    assert_eq!(finding.hat.as_deref(), Some("precheck-work.failed"));
}

// T7: same half-desugared shape under strict mode must escalate the
//     same finding to `Error` (blocks `ralph run --strict` startup).
#[test]
fn half_desugared_config_fires_error_in_strict_mode() {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    publishes: ["work.failed.proposed"]
    terminal_events: ["work.failed.proposed"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  precheck:
    enabled: true
    rules:
      work.failed:
        on_fail:
          target: "executor"
          retry_budget: 3
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

    let findings =
        check_precheck_rule_without_synthesized_gate_hat(&config, LintStrictness::Strict);
    assert_eq!(
        findings.len(),
        1,
        "half-desugared shape must produce exactly one finding under strict; got {findings:?}"
    );
    assert_eq!(findings[0].severity, LintSeverity::Error);
}

// T8: pre-normalize raw-preset shape (producers still publish the bare
//     topic, no `.proposed` anywhere, no gate hat) must stay silent —
//     `normalize()` at load time will synthesize the gate hat. This is
//     the shape `test_all_embedded_presets_pass_strict_lint` feeds via
//     `RalphConfig::parse_yaml` (which does not normalize), so a false
//     positive here would break every embedded preset with precheck
//     rules.
#[test]
fn pre_normalize_raw_preset_shape_is_silent() {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    publishes: ["work.failed"]
    terminal_events: ["work.failed"]
event_loop:
  completion_promise: "LOOP_COMPLETE"
  precheck:
    enabled: true
    rules:
      work.failed:
        on_fail:
          target: "executor"
          retry_budget: 3
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();

    let findings =
        check_precheck_rule_without_synthesized_gate_hat(&config, LintStrictness::Strict);
    assert!(
        findings.is_empty(),
        "pre-normalize raw preset shape must be silent; `normalize()` will \
         synthesize the gate hat at load; got {:?}",
        findings
            .iter()
            .map(|f| (f.id, f.severity))
            .collect::<Vec<_>>()
    );
}

// The merge-layer regression class this lint intentionally does NOT
// cover (whole `event_loop.precheck` block stripped, so no declared
// rules survive) is pinned by
// `merge_hats_overlay_preserves_precheck_when_operator_omits_it` in
// `ralph-cli/src/preflight.rs`, and the end-to-end strict-lint
// integration test `test_all_embedded_presets_pass_strict_lint`
// (presets.rs:2472) exercises the actual preset YAML and verifies the
// lint is silent — confirming the 2026-07-29-002 fix lands end-to-end.
