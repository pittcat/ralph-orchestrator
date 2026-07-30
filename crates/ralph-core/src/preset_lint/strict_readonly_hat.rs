//! 2026-07-29-003 plan U1: `strict_readonly_hat` lint.
//!
//! A strict read-only hat denies BOTH `Edit` AND `Write` (the dual-deny
//! SSOT per plan §1.4 / KTD2). Such a hat MUST declare an
//! `allowed_write_paths` contract — otherwise the workspace mutation
//! guard has no allow-list to filter against, and every delta is a
//! violation. Each entry in `allowed_write_paths` must pass
//! `workspace_mutation_guard::validate_allowed_path`.
//!
//! Severity: Error (structural, not stylistic). The lint fires
//! during `run_preset_lint` so the failure surfaces at
//! preset-load time rather than mid-run.

use super::{LintFinding, LintStrictness};
use crate::config::RalphConfig;
use crate::workspace_mutation_guard::{is_strict_read_only, validate_allowed_path};

/// 2026-07-29-003 plan U1: stable finding ID — strict read-only hat
/// declares no `allowed_write_paths` contract.
pub const FINDING_STRICT_READONLY_MISSING_WRITE_CONTRACT: &str =
    "preset.strict_readonly_missing_write_contract";

/// 2026-07-29-003 plan U1: stable finding ID — an `allowed_write_paths`
/// entry fails `validate_allowed_path`.
pub const FINDING_STRICT_READONLY_INVALID_WRITE_PATH: &str =
    "preset.strict_readonly_invalid_write_path";

/// Run the U1 lint against `config`. Returns zero or more
/// findings, sorted by hat id for determinism.
pub fn check_strict_readonly_hat(
    config: &RalphConfig,
    _strictness: LintStrictness,
) -> Vec<LintFinding> {
    let mut findings = Vec::new();
    for (hat_id, hat_cfg) in &config.hats {
        if !is_strict_read_only(&hat_cfg.disallowed_tools) {
            continue;
        }
        match &hat_cfg.allowed_write_paths {
            None => {
                findings.push(
                    LintFinding::error(
                        FINDING_STRICT_READONLY_MISSING_WRITE_CONTRACT,
                        format!(
                            "{hat_id} is a strict read-only hat (denies both Edit and Write) \
                             but declares no `allowed_write_paths` contract; a strict read-only \
                             hat MUST declare the paths it is allowed to write so the workspace \
                             mutation guard can filter expected deltas."
                        ),
                    )
                    .with_hat(hat_id.clone())
                    .with_action_hint(format!(
                        "Add an `allowed_write_paths` list to {hat_id} (e.g. `reviews/**`)."
                    )),
                );
            }
            Some(paths) => {
                for path in paths {
                    if let Err(e) = validate_allowed_path(path) {
                        findings.push(
                            LintFinding::error(
                                FINDING_STRICT_READONLY_INVALID_WRITE_PATH,
                                format!(
                                    "{hat_id}.allowed_write_paths entry '{path}' is invalid: {e}"
                                ),
                            )
                            .with_hat(hat_id.clone())
                            .with_action_hint(format!(
                                "Fix or remove '{path}' from {hat_id}.allowed_write_paths."
                            )),
                        );
                    }
                }
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
    fn strict_hat_no_paths_emits_missing_finding() {
        let cfg = cfg_with_hat("reviewer", None, vec!["Edit", "Write"]);
        let findings = check_strict_readonly_hat(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        assert_eq!(
            findings[0].id,
            FINDING_STRICT_READONLY_MISSING_WRITE_CONTRACT
        );
        assert_eq!(findings[0].hat.as_deref(), Some("reviewer"));
    }

    #[test]
    fn strict_hat_dot_git_path_emits_invalid_finding() {
        let cfg = cfg_with_hat("reviewer", Some(vec![".git/HEAD"]), vec!["Edit", "Write"]);
        let findings = check_strict_readonly_hat(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_STRICT_READONLY_INVALID_WRITE_PATH);
    }

    #[test]
    fn strict_hat_absolute_path_emits_invalid_finding() {
        let cfg = cfg_with_hat("reviewer", Some(vec!["/abs"]), vec!["Edit", "Write"]);
        let findings = check_strict_readonly_hat(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_STRICT_READONLY_INVALID_WRITE_PATH);
    }

    #[test]
    fn strict_hat_dotdot_path_emits_invalid_finding() {
        let cfg = cfg_with_hat("reviewer", Some(vec![".."]), vec!["Edit", "Write"]);
        let findings = check_strict_readonly_hat(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0].id, FINDING_STRICT_READONLY_INVALID_WRITE_PATH);
    }

    #[test]
    fn strict_hat_valid_path_emits_no_findings() {
        let cfg = cfg_with_hat("reviewer", Some(vec!["reviews/**"]), vec!["Edit", "Write"]);
        let findings = check_strict_readonly_hat(&cfg, LintStrictness::Default);
        assert!(findings.is_empty());
    }

    #[test]
    fn non_strict_hat_with_invalid_path_emits_no_findings() {
        // Only denies Edit, not Write → not strict read-only
        let cfg = cfg_with_hat("executor", Some(vec![".git/HEAD"]), vec!["Edit"]);
        let findings = check_strict_readonly_hat(&cfg, LintStrictness::Default);
        assert!(findings.is_empty());
    }

    #[test]
    fn findings_sorted_by_hat_id() {
        let mut hats = HashMap::new();
        // Insert in non-alphabetical order
        hats.insert(
            "zeta-reviewer".to_string(),
            HatConfig {
                allowed_write_paths: None,
                disallowed_tools: vec!["Edit".into(), "Write".into()],
                ..HatConfig::default()
            },
        );
        hats.insert(
            "alpha-reviewer".to_string(),
            HatConfig {
                allowed_write_paths: None,
                disallowed_tools: vec!["Edit".into(), "Write".into()],
                ..HatConfig::default()
            },
        );
        let cfg = RalphConfig {
            hats,
            ..RalphConfig::default()
        };
        let findings = check_strict_readonly_hat(&cfg, LintStrictness::Default);
        assert_eq!(findings.len(), 2);
        assert_eq!(findings[0].hat.as_deref(), Some("alpha-reviewer"));
        assert_eq!(findings[1].hat.as_deref(), Some("zeta-reviewer"));
    }
}
