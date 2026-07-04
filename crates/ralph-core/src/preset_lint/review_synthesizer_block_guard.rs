//! U3 of plan 2026-07-04-004 — `review-synthesizer` block-guard
//! drift lint.
//!
//! Catches presets whose `review-synthesizer` hat's `instructions:`
//! drift away from the explicit "全 6 维度 status == failed"
//! invariant. The runtime reads the synthesized text to decide
//! whether the agent published `plan.blocked` vs `plan.complete`;
//! loose wording (e.g. "All dimensions failed", "if any dimension
//! failed") is exactly what produced the 2026-07-04 silent-success
//! run where `review.dimensions.complete(verdict=blocked,
//! findings_count=0)` slipped through as a pass.
//!
//! Severity: `Warn` in default mode, `Error` in strict. The lint
//! fires at preset-load time so a casual refactor that loosens
//! the wording is caught before the runtime ever sees the preset.
//!
//! Detection strategy:
//!
//! 1. Locate the `review-synthesizer` hat in the typed
//!    `RalphConfig.hats` map.
//! 2. Read its `instructions` + `extra_instructions` text.
//! 3. Search for the canonical "All dimensions failed" /
//!    "all_dimensions_failed" phrase.
//! 4. Fire `Warn`/`Error` when the phrase is present **without**
//!    an explicit "全 6" / "6 个维度" / "all 6" qualifier.
//!
//! When the preset does not contain the phrase at all, the lint
//! stays silent — `review-synthesizer` may legitimately be absent
//! from non-serial presets.

use super::{LintFinding, LintStrictness};
use crate::config::RalphConfig;

/// 2026-07-04-004 plan U3 finding ID. Re-exported from `finding_id`
/// for parity with the rest of the lint family. Listed here as a
/// doc-comment reference; the canonical constant lives in
/// `crate::preset_lint::finding_id::FINDING_REVIEW_SYNTHESIZER_BLOCK_GUARD`.
pub use super::finding_id::FINDING_REVIEW_SYNTHESIZER_BLOCK_GUARD;

/// Hat id scanned by the lint. Kept as a constant so the contract
/// is unambiguous across tests + docs + future lint modules.
const HAT_REVIEW_SYNTHESIZER: &str = "review-synthesizer";

/// Phrases that indicate the hard-gate is referenced at all. When
/// none of these are present the lint stays silent (the preset
/// likely does not declare a synthesizer). When any of them IS
/// present, the lint then demands the "全 6" / "6 个维度" /
/// "all 6 dimensions" qualifier.
const BLOCK_GUARD_PHRASES: &[&str] = &[
    "all_dimensions_failed",
    "All dimensions failed",
    "all dimensions failed",
];

/// Phrases that signal the explicit "all 6 dimensions failed"
/// invariant. The lint requires **at least one** of these when
/// the gate is referenced.
const EXPLICIT_SIX_PHRASES: &[&str] = &[
    "全 6 维度",
    "全 6 个维度",
    "全6个维度",
    "全6维度",
    "6 个维度",
    "6个维度",
    "all 6 dimensions",
    "all six dimensions",
];

/// Run the U3 lint against `config`. Returns zero or more
/// findings. Empty findings means the synthesizer either is not
/// declared OR carries the explicit "all 6" qualifier.
pub fn check_review_synthesizer_block_guard(
    config: &RalphConfig,
    strictness: LintStrictness,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    let Some(hat_cfg) = config.hats.get(HAT_REVIEW_SYNTHESIZER) else {
        return findings;
    };

    // Stitch `instructions` + `extra_instructions` together so the
    // lint catches drift in either field. The runtime renders both
    // blocks back-to-back at activation time, so a missing
    // qualifier in either half is operationally equivalent.
    let mut combined = hat_cfg.instructions.clone();
    for extra in &hat_cfg.extra_instructions {
        combined.push('\n');
        combined.push_str(extra);
    }

    // First check: is the gate phrase present at all? If not, the
    // lint stays silent — the preset may be a non-serial template
    // (e.g. ce-executor-pipeline) that does not declare a
    // synthesizer.
    let references_gate = BLOCK_GUARD_PHRASES
        .iter()
        .any(|phrase| combined.contains(phrase));
    if !references_gate {
        return findings;
    }

    // Second check: does the gate carry the "全 6" qualifier? If
    // yes, the lint stays silent; if not, fire a Warn/Error
    // depending on strictness.
    let has_explicit_six = EXPLICIT_SIX_PHRASES
        .iter()
        .any(|phrase| combined.contains(phrase));
    if has_explicit_six {
        return findings;
    }

    let severity = strictness.ownership_severity();
    let finding = LintFinding::new(FINDING_REVIEW_SYNTHESIZER_BLOCK_GUARD, format!(
        "review-synthesizer 'all_dimensions_failed' hard-gate wording drifted away from \
         the explicit 'all 6 dimensions status == failed' invariant. The runtime relies on \
         the explicit phrasing to decide between plan.blocked (all 6 failed) and the \
         residual-risks path (mixed done+failed). Loose wording such as 'All dimensions failed' \
         or 'all_dimensions_failed' without '全 6' / '6 个维度' / 'all 6' lets the silent-success \
         shape (verdict=blocked + findings_count=0) slip through as a pass."
    ))
    .with_hat(HAT_REVIEW_SYNTHESIZER.to_string())
    .with_action_hint(format!(
        "Rewrite the block-guard phrase to make the 6-dimension scope explicit, e.g.: \
         'ONLY when all 6 dimensions have status == \"failed\" publish \
         plan.blocked(reason=\"all_dimensions_failed\"). Mixed (some done + some failed): \
         route through normal verdict path; failed dimensions count toward residual_risks.'"
    ));
    // Override the default severity (LintFinding::new is hard-coded
    // to Error) to match the lint's actual contract — Warn in
    // default mode, Error in strict.
    let mut finding = finding;
    finding.severity = severity;
    findings.push(finding);

    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HatConfig, RalphConfig};
    use std::collections::HashMap;

    fn cfg_with_synthesizer_instructions(instructions: &str) -> RalphConfig {
        let mut hats = HashMap::new();
        hats.insert(
            HAT_REVIEW_SYNTHESIZER.to_string(),
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
    fn test_no_findings_when_no_synthesizer_declared() {
        let cfg = RalphConfig::default();
        assert!(
            check_review_synthesizer_block_guard(&cfg, LintStrictness::Default).is_empty()
        );
    }

    #[test]
    fn test_no_findings_when_no_block_guard_text() {
        // Synthesizer declares "all_dimensions_failed" handling but
        // does NOT use the trigger phrase at all → lint stays silent.
        let cfg = cfg_with_synthesizer_instructions(
            "Synthesize review verdicts from per-dimension outputs.",
        );
        assert!(
            check_review_synthesizer_block_guard(&cfg, LintStrictness::Default).is_empty()
        );
    }

    #[test]
    fn test_warning_when_block_guard_text_uses_vague_word() {
        // Loose wording ("All dimensions failed") without an
        // explicit "全 6" qualifier → Warn in default mode.
        let cfg = cfg_with_synthesizer_instructions(
            "- **All dimensions failed** (every `status: \"failed\"`): emit \
             `plan.blocked` with `reason: \"all_dimensions_failed\"`.",
        );
        let findings =
            check_review_synthesizer_block_guard(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_REVIEW_SYNTHESIZER_BLOCK_GUARD);
        assert!(matches!(findings[0].severity, super::super::LintSeverity::Warn));
        // Verify the message points at the drift root cause.
        assert!(
            findings[0].message.contains("all 6 dimensions"),
            "message must point at the 6-dimension drift root cause, got: {:?}",
            findings[0].message
        );
    }

    #[test]
    fn test_error_when_block_guard_text_uses_vague_word_in_strict_mode() {
        // Same loose wording under strict mode → Error.
        let cfg = cfg_with_synthesizer_instructions(
            "- **All dimensions failed** (every `status: \"failed\"`): emit \
             `plan.blocked` with `reason: \"all_dimensions_failed\"`.",
        );
        let findings =
            check_review_synthesizer_block_guard(&cfg, LintStrictness::Strict);
        assert_eq!(findings.len(), 1);
        assert!(matches!(findings[0].severity, super::super::LintSeverity::Error));
    }

    #[test]
    fn test_no_findings_when_block_guard_text_explicit_six_dimensions() {
        // Explicit qualifier (Chinese "全 6 维度") → lint stays silent.
        let cfg = cfg_with_synthesizer_instructions(
            "# All dimensions failed check:\n\
             # ONLY when all 6 dimensions have status == \"failed\" publish \
             plan.blocked(reason=\"all_dimensions_failed\").\n\
             # Mixed (some done + some failed): route through normal verdict path.",
        );
        assert!(
            check_review_synthesizer_block_guard(&cfg, LintStrictness::Default).is_empty(),
            "explicit 'all 6 dimensions' wording must pass"
        );
    }

    #[test]
    fn test_no_findings_when_block_guard_text_explicit_six_dimensions_english() {
        // English variant of the explicit qualifier.
        let cfg = cfg_with_synthesizer_instructions(
            "# ONLY when all 6 dimensions have status == failed publish \
             plan.blocked(reason=all_dimensions_failed).",
        );
        assert!(
            check_review_synthesizer_block_guard(&cfg, LintStrictness::Default).is_empty()
        );
    }

    #[test]
    fn test_finding_carries_action_hint() {
        let cfg = cfg_with_synthesizer_instructions(
            "- **All dimensions failed**: emit plan.blocked.",
        );
        let findings =
            check_review_synthesizer_block_guard(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        let hint = findings[0]
            .action_hint
            .as_ref()
            .expect("action_hint must be Some");
        assert!(
            hint.contains("ONLY when all 6 dimensions"),
            "action hint must include the canonical rewrite: got {hint:?}"
        );
    }

    #[test]
    fn test_finding_carries_hat_id() {
        let cfg = cfg_with_synthesizer_instructions(
            "- **All dimensions failed**: emit plan.blocked.",
        );
        let findings =
            check_review_synthesizer_block_guard(&cfg, LintStrictness::Default);
        assert_eq!(
            findings[0].hat.as_deref(),
            Some(HAT_REVIEW_SYNTHESIZER),
            "finding must carry the hat id so dashboards can group by hat"
        );
    }
}