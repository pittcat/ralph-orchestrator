//! U1 of 2026-06-11-003: Multi-hat isolation policy lint rule (R1-R5).
//!
//! Reuses the shared [`evaluate_multi_hat_isolation`] evaluator so
//! the threshold, counting, and violation shape are defined exactly
//! once in core. Lint, preflight, and runtime contract all read
//! from the same source of truth.
//!
//! The rule ALWAYS returns `Error` severity. There is no
//! `LintStrictness` downgrade path: R4/R5 forbid configuration,
//! env var, test switch, or hidden compat opt-outs.
//!
//! This rule produces a [`RuntimeContractFinding`] directly
//! (rather than a [`LintFinding`]) because the structured details
//! `actual` / `limit` / `required_mode` need to flow through to
//! the runtime contract aggregator's `details` map. The
//! `LintFinding` struct only carries `topic` / `hat` / `owner` —
//! re-using it would silently drop these policy-relevant fields.

use crate::config::{HatExecutionMode, RalphConfig, evaluate_multi_hat_isolation};
use crate::preset_lint::finding_id::FINDING_MULTI_HAT_REQUIRES_ISOLATED;
use crate::runtime_contract::{
    FindingSeverity, FindingSource, FindingStage, RuntimeContractFinding,
};

/// Check the multi-hat isolation policy against a `RalphConfig` and
/// return at most one [`RuntimeContractFinding`].
///
/// The hat count is `config.hats.len()` — R2 forbids filtering by
/// hat kind (aggregate, observer, concurrent worker, etc.),
/// reachability, lifecycle phase, or backend. The execution mode
/// is `config.event_loop.execution_mode` (default is
/// [`HatExecutionMode::Coordinator`]).
///
/// The finding is `Error` severity. Caller-supplied
/// `LintStrictness` does not affect the result: the rule is never
/// downgraded.
pub fn check_multi_hat_isolation(config: &RalphConfig) -> Vec<RuntimeContractFinding> {
    let hat_count = config.hats.len();
    let mode = config.event_loop.execution_mode.clone();
    match evaluate_multi_hat_isolation(hat_count, mode) {
        Ok(()) => Vec::new(),
        Err(violation) => vec![violation_to_contract_finding(
            violation.actual,
            violation.limit,
            violation.required_mode,
        )],
    }
}

fn violation_to_contract_finding(
    actual: usize,
    limit: usize,
    required_mode: HatExecutionMode,
) -> RuntimeContractFinding {
    let id = format!("lint.{}", FINDING_MULTI_HAT_REQUIRES_ISOLATED);
    let message = format!(
        "preset declares {actual} hats which exceeds the coordinator limit of {limit}; \
         set `event_loop.execution_mode: isolated` to run this preset"
    );
    let hint =
        format!("Set `event_loop.execution_mode: isolated` ({actual} hats > {limit} hat limit)");
    let required_mode_label = match required_mode {
        HatExecutionMode::Coordinator => "coordinator",
        HatExecutionMode::Isolated => "isolated",
    };
    RuntimeContractFinding::try_new_core(
        id,
        FindingSource::Lint,
        FindingSeverity::Error,
        FindingStage::Authoring,
        message,
    )
    .expect("Lint source is not the reserved Preflight source")
    .with_detail("actual", actual.to_string())
    .with_detail("limit", limit.to_string())
    .with_detail("required_mode", required_mode_label.to_string())
    .with_action_hint(hint)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RalphConfig;

    fn make_config_with_n_hats(n: usize, mode_yaml: &str) -> RalphConfig {
        // Build a minimal valid YAML with N hats. The hats themselves
        // are irrelevant — the policy counts them by length, not by
        // shape — so we emit identical entries.
        let mut hats_yaml = String::new();
        for i in 0..n {
            if i > 0 {
                hats_yaml.push('\n');
            }
            hats_yaml.push_str(&format!(
                "  h{i}:\n    name: \"H{i}\"\n    triggers: [\"work.start\"]\n    publishes: [\"work.done\"]"
            ));
        }
        let yaml = format!(
            r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  {mode_yaml}
hats:
{hats_yaml}
"#
        );
        serde_yaml::from_str(&yaml).expect("parse test config")
    }

    fn make_config_default_mode(n: usize) -> RalphConfig {
        make_config_with_n_hats(n, "")
    }

    fn make_config_explicit_isolated(n: usize) -> RalphConfig {
        make_config_with_n_hats(n, "execution_mode: isolated")
    }

    fn make_config_explicit_coordinator(n: usize) -> RalphConfig {
        make_config_with_n_hats(n, "execution_mode: coordinator")
    }

    // ── AE1: 3 hats, default mode → lint passes ────────────────

    #[test]
    fn three_hats_default_mode_lint_passes() {
        let config = make_config_default_mode(3);
        let findings = check_multi_hat_isolation(&config);
        assert!(
            findings.is_empty(),
            "3 hats with default mode must produce no finding, got: {findings:?}"
        );
    }

    // ── AE2: 4 hats, default mode → lint produces error with details ─

    #[test]
    fn four_hats_default_mode_lint_produces_error() {
        let config = make_config_default_mode(4);
        let findings = check_multi_hat_isolation(&config);
        assert_eq!(findings.len(), 1, "must produce exactly one finding");
        let finding = &findings[0];
        assert_eq!(
            finding.id,
            format!("lint.{}", FINDING_MULTI_HAT_REQUIRES_ISOLATED)
        );
        assert_eq!(finding.severity, FindingSeverity::Error);
        assert_eq!(finding.source, FindingSource::Lint);
        assert_eq!(finding.stage, FindingStage::Authoring);
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
            Some("isolated"),
            "details.required_mode must be 'isolated': {:?}",
            finding.details
        );
        let msg = &finding.message;
        assert!(
            msg.contains("4"),
            "message must include actual count: {msg}"
        );
        assert!(msg.contains("3"), "message must include limit: {msg}");
        assert!(
            finding.action_hint.is_some(),
            "action_hint must be set: {finding:?}"
        );
    }

    // ── AE3: 4 hats, explicit Coordinator → same error shape ──

    #[test]
    fn four_hats_explicit_coordinator_lint_matches_default() {
        let default_config = make_config_default_mode(4);
        let explicit_config = make_config_explicit_coordinator(4);
        let default_findings = check_multi_hat_isolation(&default_config);
        let explicit_findings = check_multi_hat_isolation(&explicit_config);
        assert_eq!(default_findings.len(), 1);
        assert_eq!(explicit_findings.len(), 1);
        assert_eq!(default_findings[0].id, explicit_findings[0].id);
        assert_eq!(default_findings[0].severity, explicit_findings[0].severity);
        assert_eq!(default_findings[0].message, explicit_findings[0].message);
        assert_eq!(default_findings[0].details, explicit_findings[0].details);
    }

    // ── 4 hats, explicit Isolated → lint passes ───────────────

    #[test]
    fn four_hats_isolated_mode_lint_passes() {
        let config = make_config_explicit_isolated(4);
        let findings = check_multi_hat_isolation(&config);
        assert!(
            findings.is_empty(),
            "4 hats with isolated mode must produce no finding, got: {findings:?}"
        );
    }

    // ── AE4: 8 hats including aggregate / observer / concurrent
    //    worker — policy still counts as 8 hats. R2 forbids filtering
    //    by hat kind. The lint counts `config.hats.len()` only.

    #[test]
    fn eight_hats_with_special_kinds_still_counts_as_eight() {
        let yaml = r#"
tasks:
  enabled: false
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
        let config: RalphConfig = serde_yaml::from_str(yaml).expect("parse test config");
        assert_eq!(
            config.hats.len(),
            8,
            "fixture must declare 8 hats for AE4 to be meaningful"
        );
        // Default (Coordinator) at 8 hats → must fail.
        let default_findings = check_multi_hat_isolation(&config);
        assert_eq!(default_findings.len(), 1, "8 hats default mode must fail");
        assert_eq!(
            default_findings[0]
                .details
                .get("actual")
                .map(String::as_str),
            Some("8")
        );
        // Explicit isolated at 8 hats → must pass.
        let mut isolated_config = config.clone();
        isolated_config.event_loop.execution_mode = HatExecutionMode::Isolated;
        let isolated_findings = check_multi_hat_isolation(&isolated_config);
        assert!(
            isolated_findings.is_empty(),
            "8 hats explicit isolated must pass, got: {isolated_findings:?}"
        );
    }

    // ── Lint rule is always Error — not affected by LintStrictness.

    #[test]
    fn four_hats_default_mode_always_error() {
        let config = make_config_default_mode(4);
        let findings = check_multi_hat_isolation(&config);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].severity, FindingSeverity::Error);
    }

    // ── Larger counts surface the same finding id with the
    //    actual count in details.

    #[test]
    fn ten_hats_default_mode_finding_carries_actual_count() {
        let config = make_config_default_mode(10);
        let findings = check_multi_hat_isolation(&config);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].details.get("actual").map(String::as_str),
            Some("10")
        );
        let msg = &findings[0].message;
        assert!(msg.contains("10"), "message must include actual=10: {msg}");
        assert!(msg.contains("3"), "message must include limit=3: {msg}");
    }

    // ── Smoke: 1 hat with default mode → no finding (sanity check
    //    that the rule is not over-eager for tiny presets).

    #[test]
    fn single_hat_default_mode_lint_passes() {
        let config = make_config_default_mode(1);
        let findings = check_multi_hat_isolation(&config);
        assert!(findings.is_empty());
    }
}
