//! 2026-06-29-007 plan U5a: `dimension_reviewer_write_path_lint`
//!
//! Reject presets that grant `dimension-reviewer` write access
//! to `docs/plans/*.md`. The reviewer is a code-only reviewer;
//! letting it touch the plan markdown lets a single bad review
//! rewrite the runbook mid-loop (the 2026-06-28 dimension-reviewer
//! scope_violation 早班 incident pattern).
//!
//! Severity: Error (structural, not stylistic). The lint fires
//! during `run_preset_lint` so the failure surfaces at
//! preset-load time rather than mid-run.

use super::{LintFinding, LintStrictness};
use crate::config::RalphConfig;

/// 2026-06-29-007 plan U5a: stable finding ID.
pub const FINDING_DIMENSION_REVIEWER_WRITE_PLAN: &str = "preset.dimension_reviewer_write_plan";

/// 2026-06-29-007 plan U5a: paths that
/// `dimension-reviewer` is NOT allowed to write. We match
/// the canonical `docs/plans/` prefix that all plan
/// documents in this repo live under; the wildcard at the
/// end is intentional so `docs/plans/foo.md` and
/// `docs/plans/sub/bar.md` both trip the rule.
const FORBIDDEN_PATH_PREFIX: &str = "docs/plans/";

/// Run the U5a lint against `config`. Returns zero or more
/// findings, sorted by hat id for determinism.
pub fn check_dimension_reviewer_write_paths(
    config: &RalphConfig,
    _strictness: LintStrictness,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    for (hat_id, hat_cfg) in &config.hats {
        if hat_id != "dimension-reviewer" {
            continue;
        }
        let Some(write_paths) = hat_cfg.allowed_write_paths.as_ref() else {
            continue;
        };
        for path in write_paths {
            if path.starts_with(FORBIDDEN_PATH_PREFIX) {
                let finding = LintFinding::error(
                    FINDING_DIMENSION_REVIEWER_WRITE_PLAN,
                    format!(
                        "dimension-reviewer.allowed_write_paths must NOT include \
                         '{FORBIDDEN_PATH_PREFIX}' entries; found '{path}'",
                    ),
                )
                .with_hat(hat_id.clone())
                .with_action_hint(format!(
                    "Remove '{path}' from dimension-reviewer.allowed_write_paths, or \
                     restrict the reviewer to a code-only subtree (e.g. 'sorts/**')."
                ));
                findings.push(finding);
            }
        }
    }
    findings.sort_by(|a, b| a.hat.cmp(&b.hat));
    findings
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{HatConfig, RalphConfig};
    use std::collections::HashMap;

    fn cfg_with_paths(paths: Option<Vec<&str>>) -> RalphConfig {
        let mut hats = HashMap::new();
        hats.insert(
            "dimension-reviewer".to_string(),
            HatConfig {
                allowed_write_paths: paths.map(|p| p.into_iter().map(String::from).collect()),
                ..HatConfig::default()
            },
        );
        RalphConfig {
            hats,
            ..RalphConfig::default()
        }
    }

    #[test]
    fn empty_paths_passes() {
        let cfg = cfg_with_paths(Some(vec![]));
        assert!(check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default).is_empty());
    }

    #[test]
    fn no_paths_declared_passes() {
        let cfg = cfg_with_paths(None);
        assert!(check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default).is_empty());
    }

    #[test]
    fn code_only_paths_pass() {
        let cfg = cfg_with_paths(Some(vec!["sorts/**", "tests/**"]));
        assert!(check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default).is_empty());
    }

    #[test]
    fn docs_plans_path_fails() {
        let cfg = cfg_with_paths(Some(vec!["sorts/**", "docs/plans/review.md"]));
        let findings = check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_DIMENSION_REVIEWER_WRITE_PLAN);
        assert!(findings[0].message.contains("docs/plans/"));
    }

    #[test]
    fn nested_docs_plans_path_fails() {
        let cfg = cfg_with_paths(Some(vec!["docs/plans/sub/notes.md"]));
        let findings = check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn other_hat_with_docs_plans_passes() {
        let mut hats = HashMap::new();
        hats.insert(
            "executor".to_string(),
            HatConfig {
                allowed_write_paths: Some(vec!["docs/plans/executor-notes.md".to_string()]),
                ..HatConfig::default()
            },
        );
        let cfg = RalphConfig {
            hats,
            ..RalphConfig::default()
        };
        assert!(check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default).is_empty());
    }
}
