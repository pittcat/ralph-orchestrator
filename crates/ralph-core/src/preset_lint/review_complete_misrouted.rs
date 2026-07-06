//! U4 of plan 2026-07-04-004 — `review.complete` misrouting
//! drift lint.
//!
//! Catches presets whose `coordinator` hat `instructions:` drift
//! away from the explicit `findings_count == 0 → plan.complete`
//! routing rule. The runtime accepts
//! `review.complete(verdict=blocked, findings_count=0)` as a
//! legitimate block shape today; without the explicit rule the
//! agent ignores `findings_count` and routes on `verdict` alone,
//! which is exactly what produced the 2026-07-04 silent-success
//! run.
//!
//! Severity: `Warn` in default mode, `Error` in strict. The lint
//! fires at preset-load time so a casual refactor that loosens
//! the wording is caught before the runtime ever sees the preset.
//!
//! Detection strategy:
//!
//! 1. Locate the `coordinator` hat in the typed
//!    `RalphConfig.hats` map.
//! 2. Read its `instructions` + `extra_instructions` text.
//! 3. Verify the text contains an explicit `findings_count == 0`
//!    + `plan.complete` routing phrase.
//! 4. If the hat only routes on `verdict` (`if verdict=blocked
//!    then plan.blocked`) without mentioning `findings_count`,
//!    fire `Error` (the silent-success root cause).
//! 5. If the hat mentions `fix_plan_file == "null"` routing but
//!    NOT `findings_count`, fire `Warn` (drift signal, not yet a
//!    full regression).
//!
//! When the preset does not declare a `coordinator` hat, the lint
//! stays silent — the rule is coordinator-specific.

use super::{LintFinding, LintStrictness};
use crate::config::RalphConfig;

/// 2026-07-04-004 plan U4 finding ID. Canonical constant lives
/// in `crate::preset_lint::finding_id::FINDING_REVIEW_COMPLETE_MISROUTED`.
pub use super::finding_id::FINDING_REVIEW_COMPLETE_MISROUTED;

/// Hat id scanned by the lint.
const HAT_COORDINATOR: &str = "coordinator";

/// Phrases that signal the canonical "findings_count == 0 →
/// plan.complete" routing is referenced. At least one of these
/// must be present for the routing to be considered explicit.
const FINDINGS_COUNT_RULE_PHRASES: &[&str] = &[
    "findings_count == 0",
    "findings_count==0",
    "findings_count == 0 always",
    "findings_count is 0",
    "final_findings_count",
];

/// Phrases that signal the hat ONLY routes on the `verdict`
/// field. These are the silent-success anti-pattern: a
/// `verdict=blocked` triggers `plan.blocked` regardless of
/// `findings_count`. The lint fires `Error` when the hat carries
/// these without an accompanying `findings_count` rule.
const VERDICT_ONLY_ROUTING_PHRASES: &[&str] = &[
    "verdict=blocked",
    "verdict == \"blocked\"",
    "verdict == \"blocked\" then",
    "verdict=blocked → plan.blocked",
    "verdict=blocked → plan.blocked",
    "if verdict=blocked",
    "if verdict == blocked",
];

/// Phrases that signal the legacy `fix_plan_file == "null"`
/// routing. The presence of these without an explicit
/// `findings_count` rule is a drift warning (Warn), not a full
/// regression — the runtime can still route correctly via the
/// `findings_count` field when the hat explicitly checks it.
const FIX_PLAN_NULL_PHRASES: &[&str] = &[
    "fix_plan_file == \"null\"",
    "fix_plan_file == \"null\" route",
    "fix_plan_file == null",
];

/// Run the U4 lint against `config`. Returns zero or more
/// findings. Empty findings means the coordinator either is not
/// declared OR carries the explicit `findings_count == 0`
/// routing rule.
pub fn check_review_complete_misrouted(
    config: &RalphConfig,
    strictness: LintStrictness,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let Some(hat_cfg) = config.hats.get(HAT_COORDINATOR) else {
        return findings;
    };

    // Stitch `instructions` + `extra_instructions` together so the
    // lint catches drift in either field. The runtime renders both
    // blocks back-to-back at activation time.
    let mut combined = hat_cfg.instructions.clone();
    for extra in &hat_cfg.extra_instructions {
        combined.push('\n');
        combined.push_str(extra);
    }

    let has_findings_count_rule = FINDINGS_COUNT_RULE_PHRASES
        .iter()
        .any(|phrase| combined.contains(phrase));
    let has_verdict_only_routing = VERDICT_ONLY_ROUTING_PHRASES
        .iter()
        .any(|phrase| combined.contains(phrase));
    let has_fix_plan_null_routing = FIX_PLAN_NULL_PHRASES
        .iter()
        .any(|phrase| combined.contains(phrase));

    // Case 1: routing on verdict only, no findings_count rule.
    // This is the silent-success root cause — escalate to Error
    // regardless of strictness (the rule is structural).
    if has_verdict_only_routing && !has_findings_count_rule {
        let mut finding = LintFinding::new(
            FINDING_REVIEW_COMPLETE_MISROUTED,
            format!(
                "coordinator 'review.complete' routing relies on the `verdict` field alone \
                 (e.g. 'if verdict=blocked then plan.blocked') without an explicit \
                 `findings_count == 0` rule. The runtime accepts \
                 `review.complete(verdict=blocked, findings_count=0)` as a legitimate block \
                 shape today; without the explicit rule the agent routes on verdict alone, \
                 which is exactly the 2026-07-04 silent-success pattern."
            ),
        )
        .with_hat(HAT_COORDINATOR.to_string())
        .with_action_hint(format!(
            "Add an explicit `findings_count == 0` rule BEFORE the verdict-based routing. \
             Canonical wording: 'When review.complete payload has findings_count == 0, ALWAYS \
             publish plan.complete(verdict=\"pass_with_residuals\", final_findings_count=0), \
             REGARDLESS of the verdict field. Only when findings_count > 0 does the verdict \
             field drive plan.blocked routing.'"
        ));
        finding.severity = super::LintSeverity::Error;
        findings.push(finding);
        return findings;
    }

    // Case 2: `fix_plan_file == "null"` routing without an
    // explicit `findings_count` rule. This is a drift signal
    // (Warn in default, Error in strict) rather than a full
    // regression — the runtime can still route correctly when
    // the hat also checks `findings_count`, but a casual
    // refactor that drops the rule would re-introduce the
    // silent-success lane.
    if has_fix_plan_null_routing && !has_findings_count_rule {
        let severity = strictness.ownership_severity();
        let mut finding = LintFinding::new(
            FINDING_REVIEW_COMPLETE_MISROUTED,
            format!(
                "coordinator 'review.complete' routing mentions `fix_plan_file == \"null\"` \
                 but lacks an explicit `findings_count == 0` rule. The legacy \
                 `fix_plan_file == \"null\"` heuristic is brittle — a future event shape \
                 that omits `findings_count` or sets it to a non-zero value without \
                 `fix_plan_file` will fall through to the default routing and may \
                 regress into the silent-success lane."
            ),
        )
        .with_hat(HAT_COORDINATOR.to_string())
        .with_action_hint(format!(
            "Add the explicit `findings_count == 0` rule alongside the `fix_plan_file == \
             \"null\"` check. The findings_count check is the authoritative gate; the \
             fix_plan_file check is the legacy fallback. Always check findings_count \
             first."
        ));
        finding.severity = severity;
        findings.push(finding);
    }

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HatConfig, RalphConfig};
    use std::collections::HashMap;

    fn cfg_with_coordinator_instructions(instructions: &str) -> RalphConfig {
        let mut hats = HashMap::new();
        hats.insert(
            HAT_COORDINATOR.to_string(),
            HatConfig {
                instructions: instructions.to_string(),
                ..HatConfig::default()
            },
        );
        RalphConfig {
            hats,
            ..RalphConfig::default()
        }
    }

    #[test]
    fn test_no_findings_when_no_coordinator_declared() {
        let cfg = RalphConfig::default();
        assert!(check_review_complete_misrouted(&cfg, LintStrictness::Default).is_empty());
    }

    #[test]
    fn test_no_findings_when_coordinator_has_findings_count_zero_rule() {
        // Canonical wording with the explicit findings_count==0 rule.
        let cfg = cfg_with_coordinator_instructions(
            "# HARD RULE — review.complete routing:\n\
             # When review.complete payload has findings_count == 0, ALWAYS publish \
             # plan.complete(verdict=\"pass_with_residuals\", final_findings_count=0), \
             # REGARDLESS of the verdict field. Only when findings_count > 0 does the \
             # verdict field drive plan.blocked routing.",
        );
        assert!(
            check_review_complete_misrouted(&cfg, LintStrictness::Default).is_empty(),
            "explicit findings_count==0 rule must pass"
        );
    }

    #[test]
    fn test_warning_when_coordinator_only_has_fix_plan_file_null_rule() {
        // Legacy `fix_plan_file == "null"` routing WITHOUT explicit
        // findings_count rule → Warn in default mode.
        let cfg = cfg_with_coordinator_instructions(
            "# When fix_plan_file == \"null\" route to plan.complete; otherwise plan.blocked.",
        );
        let findings = check_review_complete_misrouted(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_REVIEW_COMPLETE_MISROUTED);
        assert!(matches!(
            findings[0].severity,
            super::super::LintSeverity::Warn
        ));
    }

    #[test]
    fn test_error_when_coordinator_only_routes_on_verdict_field() {
        // Routing on verdict alone, no findings_count rule →
        // Error regardless of strictness (structural anti-pattern).
        let cfg = cfg_with_coordinator_instructions(
            "# If verdict=blocked then plan.blocked; else plan.complete.",
        );
        let findings = check_review_complete_misrouted(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        assert!(matches!(
            findings[0].severity,
            super::super::LintSeverity::Error
        ));
        assert!(
            findings[0].message.contains("verdict"),
            "message must point at the verdict-only drift root cause"
        );
    }

    #[test]
    fn test_no_findings_when_other_hat_has_vague_rule() {
        // The lint is coordinator-specific; a vague rule in
        // another hat must NOT trigger.
        let mut hats = HashMap::new();
        hats.insert(
            "executor".to_string(),
            HatConfig {
                instructions: "If verdict=blocked then fail.".to_string(),
                ..HatConfig::default()
            },
        );
        let cfg = RalphConfig {
            hats,
            ..RalphConfig::default()
        };
        assert!(
            check_review_complete_misrouted(&cfg, LintStrictness::Default).is_empty(),
            "lint must be scoped to the coordinator hat"
        );
    }

    #[test]
    fn test_verdict_only_routing_with_findings_count_rule_passes() {
        // Hat carries both verdict-only routing AND explicit
        // findings_count rule → no finding (verdict is consulted
        // only after findings_count gates).
        let cfg = cfg_with_coordinator_instructions(
            "# HARD RULE — review.complete routing:\n\
             # When review.complete payload has findings_count == 0, ALWAYS publish \
             # plan.complete(verdict=\"pass_with_residuals\"), REGARDLESS of verdict.\n\
             # Only when findings_count > 0 does the verdict field drive plan.blocked \
             # routing. (if verdict=blocked then plan.blocked)",
        );
        assert!(
            check_review_complete_misrouted(&cfg, LintStrictness::Default).is_empty(),
            "verdict-only routing guarded by findings_count==0 must pass"
        );
    }

    #[test]
    fn test_finding_carries_hat_id_and_action_hint() {
        let cfg = cfg_with_coordinator_instructions("# If verdict=blocked then plan.blocked.");
        let findings = check_review_complete_misrouted(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].hat.as_deref(), Some(HAT_COORDINATOR));
        let hint = findings[0]
            .action_hint
            .as_ref()
            .expect("action_hint must be Some");
        assert!(
            hint.contains("findings_count == 0"),
            "action hint must point at the canonical findings_count rule"
        );
    }
}
