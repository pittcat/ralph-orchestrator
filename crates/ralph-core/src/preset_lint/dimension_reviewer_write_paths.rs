//! 2026-06-29-007 plan U5a: `dimension_reviewer_write_path_lint`
//!
//! Reject presets that grant read-only dimension reviewer hats write access
//! to `docs/plans/*.md`. These are code-only reviewers;
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
/// Read-only dimension reviewer hats are NOT allowed to write these paths. We match
/// the canonical `docs/plans/` prefix that all plan
/// documents in this repo live under; the wildcard at the
/// end is intentional so `docs/plans/foo.md` and
/// `docs/plans/sub/bar.md` both trip the rule.
const FORBIDDEN_PATH_PREFIX: &str = "docs/plans/";

fn is_read_only_dimension_reviewer(hat_id: &str, disallowed_tools: &[String]) -> bool {
    (hat_id == "dimension-reviewer" || hat_id.starts_with("dim:"))
        && disallowed_tools
            .iter()
            .any(|tool| matches!(tool.as_str(), "Edit" | "Write"))
}

/// Run the U5a lint against `config`. Returns zero or more
/// findings, sorted by hat id for determinism.
pub fn check_dimension_reviewer_write_paths(
    config: &RalphConfig,
    _strictness: LintStrictness,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    for (hat_id, hat_cfg) in &config.hats {
        if !is_read_only_dimension_reviewer(hat_id, &hat_cfg.disallowed_tools) {
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
                        "{hat_id}.allowed_write_paths must NOT include \
                         '{FORBIDDEN_PATH_PREFIX}' entries for read-only dimension reviewers; \
                         found '{path}'",
                    ),
                )
                .with_hat(hat_id.clone())
                .with_action_hint(format!(
                    "Remove '{path}' from {hat_id}.allowed_write_paths, or restrict the \
                     reviewer to review-product output paths such as '.ralph/review/**'."
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

    fn cfg_with_hat(
        hat_id: &str,
        paths: Option<Vec<&str>>,
        disallowed_tools: Vec<&str>,
    ) -> RalphConfig {
        let mut hats = HashMap::new();
        hats.insert(
            hat_id.to_string(),
            HatConfig {
                allowed_write_paths: paths.map(|p| p.into_iter().map(String::from).collect()),
                disallowed_tools: disallowed_tools.into_iter().map(String::from).collect(),
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
        let cfg = cfg_with_hat("dimension-reviewer", Some(vec![]), vec!["Edit"]);
        assert!(check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default).is_empty());
    }

    #[test]
    fn no_paths_declared_passes() {
        let cfg = cfg_with_hat("dimension-reviewer", None, vec!["Edit"]);
        assert!(check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default).is_empty());
    }

    #[test]
    fn code_only_paths_pass() {
        let cfg = cfg_with_hat(
            "dimension-reviewer",
            Some(vec!["sorts/**", "tests/**"]),
            vec!["Edit"],
        );
        assert!(check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default).is_empty());
    }

    #[test]
    fn docs_plans_path_fails() {
        let cfg = cfg_with_hat(
            "dimension-reviewer",
            Some(vec!["sorts/**", "docs/plans/review.md"]),
            vec!["Edit"],
        );
        let findings = check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_DIMENSION_REVIEWER_WRITE_PLAN);
        assert!(findings[0].message.contains("docs/plans/"));
    }

    #[test]
    fn nested_docs_plans_path_fails() {
        let cfg = cfg_with_hat(
            "dimension-reviewer",
            Some(vec!["docs/plans/sub/notes.md"]),
            vec!["Edit"],
        );
        let findings = check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
    }

    #[test]
    fn split_dim_hat_with_docs_plans_path_fails() {
        let cfg = cfg_with_hat(
            "dim:testing",
            Some(vec!["docs/plans/review.md"]),
            vec!["Edit"],
        );
        let findings = check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].hat.as_deref(), Some("dim:testing"));
    }

    #[test]
    fn split_dim_hat_without_edit_or_write_disallow_passes() {
        let cfg = cfg_with_hat("dim:testing", Some(vec!["docs/plans/review.md"]), vec![]);
        assert!(check_dimension_reviewer_write_paths(&cfg, LintStrictness::Default).is_empty());
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
