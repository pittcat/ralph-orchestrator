//! Embedded artifact templates for builtin presets.
//!
//! # Why this exists
//!
//! Builtin presets such as `parallel-forge` require agents to fill fixed
//! artifact templates (development plan, unit YAML, manager report, …).
//! Those templates live in the **source tree** at
//! `presets/templates/<preset>/` during development, but operators often
//! install only the `ralph` binary — the source tree is not present on
//! the deployment machine.
//!
//! Therefore templates are **compile-time embedded** into the binary
//! (same pattern as builtin preset YAML via `build.rs` + `include_str!`),
//! then **materialized to disk** at runtime with:
//!
//! ```text
//! ralph preset materialize-artifacts parallel-forge --plan-key <key>
//! ```
//!
//! Default output: `.ralph/forge/<plan-key>/templates/`. Hats then `cp`
//! from that directory into the business artifact paths and fill every
//! section — they never depend on a checkout of `ralph-orchestrator`.
//!
//! # Closed loop
//!
//! 1. Author edits `presets/templates/parallel-forge/*`
//! 2. `build.rs` copies them into `$OUT_DIR/artifact-templates/...`
//! 3. This module `include_str!`s them into the binary
//! 4. CLI `materialize-artifacts` writes them to the operator workspace
//! 5. Unit + integration tests assert embed + CLI write + TDD/BDD markers

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

/// One embedded template file (basename + UTF-8 contents).
pub struct ArtifactTemplate {
    pub file_name: &'static str,
    pub content: &'static str,
}

/// Expected basenames for `parallel-forge` (keep in sync with
/// `presets/templates/parallel-forge/` and `build.rs` copy).
pub const PARALLEL_FORGE_TEMPLATE_NAMES: &[&str] = &[
    "development-plan.template.md",
    "unit.template.yml",
    "execution-plan.template.yml",
    "unit-completion.template.md",
    "manager-report.template.md",
    "wave-settlement.template.md",
    "wave-failure.template.md",
    "merge-conflict.template.md",
    "correction.template.md",
    "README.md",
];

/// Expected basenames for `red-team-attack` (keep in sync with
/// `presets/templates/red-team-attack/` and `build.rs` copy).
pub const RED_TEAM_ATTACK_TEMPLATE_NAMES: &[&str] = &[
    "experiment.template.yml",
    "finding.template.yml",
    "report.template.md",
    "plan.template.md",
    "README.md",
];

/// Expected basenames for `ce-executor-pipeline` (keep in sync with
/// `presets/templates/ce-executor-pipeline/` and `build.rs` copy).
pub const CE_EXECUTOR_PIPELINE_TEMPLATE_NAMES: &[&str] = &[
    "fail-confidence-rubric.template.md",
    "settlement-evidence.template.md",
    "README.md",
];

const PARALLEL_FORGE_TEMPLATES: &[ArtifactTemplate] = &[
    ArtifactTemplate {
        file_name: "development-plan.template.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/parallel-forge/development-plan.template.md"
        )),
    },
    ArtifactTemplate {
        file_name: "unit.template.yml",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/parallel-forge/unit.template.yml"
        )),
    },
    ArtifactTemplate {
        file_name: "execution-plan.template.yml",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/parallel-forge/execution-plan.template.yml"
        )),
    },
    ArtifactTemplate {
        file_name: "unit-completion.template.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/parallel-forge/unit-completion.template.md"
        )),
    },
    ArtifactTemplate {
        file_name: "manager-report.template.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/parallel-forge/manager-report.template.md"
        )),
    },
    ArtifactTemplate {
        file_name: "wave-settlement.template.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/parallel-forge/wave-settlement.template.md"
        )),
    },
    ArtifactTemplate {
        file_name: "wave-failure.template.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/parallel-forge/wave-failure.template.md"
        )),
    },
    ArtifactTemplate {
        file_name: "merge-conflict.template.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/parallel-forge/merge-conflict.template.md"
        )),
    },
    ArtifactTemplate {
        file_name: "correction.template.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/parallel-forge/correction.template.md"
        )),
    },
    ArtifactTemplate {
        file_name: "README.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/parallel-forge/README.md"
        )),
    },
];

const RED_TEAM_ATTACK_TEMPLATES: &[ArtifactTemplate] = &[
    ArtifactTemplate {
        file_name: "experiment.template.yml",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/red-team-attack/experiment.template.yml"
        )),
    },
    ArtifactTemplate {
        file_name: "finding.template.yml",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/red-team-attack/finding.template.yml"
        )),
    },
    ArtifactTemplate {
        file_name: "report.template.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/red-team-attack/report.template.md"
        )),
    },
    ArtifactTemplate {
        file_name: "plan.template.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/red-team-attack/plan.template.md"
        )),
    },
    ArtifactTemplate {
        file_name: "README.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/red-team-attack/README.md"
        )),
    },
];

const CE_EXECUTOR_PIPELINE_TEMPLATES: &[ArtifactTemplate] = &[
    ArtifactTemplate {
        file_name: "fail-confidence-rubric.template.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/ce-executor-pipeline/fail-confidence-rubric.template.md"
        )),
    },
    ArtifactTemplate {
        file_name: "settlement-evidence.template.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/ce-executor-pipeline/settlement-evidence.template.md"
        )),
    },
    ArtifactTemplate {
        file_name: "README.md",
        content: include_str!(concat!(
            env!("OUT_DIR"),
            "/artifact-templates/ce-executor-pipeline/README.md"
        )),
    },
];

/// Strip optional `builtin:` prefix from preset names.
///
/// Agents and operators commonly pass `builtin:parallel-forge` (same as
/// `ralph run -H`); the embedded catalog keys omit the prefix.
pub fn normalize_preset_name(name: &str) -> &str {
    name.strip_prefix("builtin:").unwrap_or(name)
}

/// Default templates directory for a forge plan key (cwd-relative).
///
/// Layout matches parallel-forge hat instructions:
/// `.ralph/forge/<plan-key>/templates/`.
pub fn default_forge_templates_dir(plan_key: &str) -> PathBuf {
    PathBuf::from(".ralph")
        .join("forge")
        .join(plan_key)
        .join("templates")
}

/// Default templates directory for a red-team-attack plan key (cwd-relative).
///
/// Layout matches red-team-attack hat instructions:
/// `.ralph/red-team/<plan-key>/templates/`.
pub fn default_red_team_templates_dir(plan_key: &str) -> PathBuf {
    PathBuf::from(".ralph")
        .join("red-team")
        .join(plan_key)
        .join("templates")
}

/// List embedded template basenames for a preset.
pub fn list_template_names(preset: &str) -> Result<Vec<&'static str>> {
    let templates = templates_for_preset(normalize_preset_name(preset))?;
    Ok(templates.iter().map(|t| t.file_name).collect())
}

fn templates_for_preset(preset: &str) -> Result<&'static [ArtifactTemplate]> {
    match preset {
        "parallel-forge" => Ok(PARALLEL_FORGE_TEMPLATES),
        "red-team-attack" => Ok(RED_TEAM_ATTACK_TEMPLATES),
        "ce-executor-pipeline" => Ok(CE_EXECUTOR_PIPELINE_TEMPLATES),
        other => bail!(
            "no embedded artifact templates for preset '{other}' \
             (supported: parallel-forge, red-team-attack, ce-executor-pipeline)"
        ),
    }
}

/// Write all embedded templates for `preset` into `dest_dir` (created if missing).
///
/// Idempotent: re-running overwrites the same basenames with the binary's
/// current embedded content. Returns the absolute-or-relative paths written
/// (joined under `dest_dir`).
pub fn materialize(preset: &str, dest_dir: &Path) -> Result<Vec<PathBuf>> {
    let preset_key = normalize_preset_name(preset);
    let templates = templates_for_preset(preset_key)?;
    if preset_key == "parallel-forge" && templates.len() != PARALLEL_FORGE_TEMPLATE_NAMES.len() {
        bail!(
            "internal catalog drift for parallel-forge: embedded {} files, \
             expected {}",
            templates.len(),
            PARALLEL_FORGE_TEMPLATE_NAMES.len()
        );
    }
    if preset_key == "red-team-attack" && templates.len() != RED_TEAM_ATTACK_TEMPLATE_NAMES.len() {
        bail!(
            "internal catalog drift for red-team-attack: embedded {} files, \
             expected {}",
            templates.len(),
            RED_TEAM_ATTACK_TEMPLATE_NAMES.len()
        );
    }
    if preset_key == "ce-executor-pipeline"
        && templates.len() != CE_EXECUTOR_PIPELINE_TEMPLATE_NAMES.len()
    {
        bail!(
            "internal catalog drift for ce-executor-pipeline: embedded {} files, \
             expected {}",
            templates.len(),
            CE_EXECUTOR_PIPELINE_TEMPLATE_NAMES.len()
        );
    }

    fs::create_dir_all(dest_dir).with_context(|| {
        format!(
            "failed to create artifact templates directory {}",
            dest_dir.display()
        )
    })?;

    let mut written = Vec::with_capacity(templates.len());
    for template in templates {
        let path = dest_dir.join(template.file_name);
        fs::write(&path, template.content)
            .with_context(|| format!("failed to write artifact template {}", path.display()))?;
        written.push(path);
    }

    // Fail-closed if a catalog entry failed to write under an unexpected name.
    let expected_names = list_template_names(preset)?;
    for name in &expected_names {
        if !dest_dir.join(name).is_file() {
            bail!(
                "materialize incomplete: expected {} under {}",
                name,
                dest_dir.display()
            );
        }
    }
    Ok(written)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_forge_embeds_expected_count_and_names() {
        assert_eq!(
            PARALLEL_FORGE_TEMPLATES.len(),
            PARALLEL_FORGE_TEMPLATE_NAMES.len()
        );
        let names = list_template_names("parallel-forge").unwrap();
        for expected in PARALLEL_FORGE_TEMPLATE_NAMES {
            assert!(
                names.contains(expected),
                "missing embedded template {expected}"
            );
        }
        for t in PARALLEL_FORGE_TEMPLATES {
            assert!(!t.content.is_empty(), "{} must be non-empty", t.file_name);
            assert!(
                !t.content.contains('\0'),
                "{} must be valid UTF-8 text",
                t.file_name
            );
        }
    }

    #[test]
    fn normalize_strips_builtin_prefix() {
        assert_eq!(normalize_preset_name("parallel-forge"), "parallel-forge");
        assert_eq!(
            normalize_preset_name("builtin:parallel-forge"),
            "parallel-forge"
        );
    }

    #[test]
    fn unknown_preset_lists_supported() {
        let err = list_template_names("no-such-preset").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("no-such-preset"));
        assert!(msg.contains("parallel-forge"));
    }

    #[test]
    fn default_dir_uses_plan_key() {
        let p = default_forge_templates_dir("my-plan");
        assert_eq!(p, PathBuf::from(".ralph/forge/my-plan/templates"));
    }

    #[test]
    fn materialize_writes_all_files_with_content() {
        let dir = tempfile::tempdir().unwrap();
        let paths = materialize("parallel-forge", dir.path()).unwrap();
        assert_eq!(paths.len(), PARALLEL_FORGE_TEMPLATE_NAMES.len());
        for name in PARALLEL_FORGE_TEMPLATE_NAMES {
            let path = dir.path().join(name);
            assert!(path.is_file(), "{name} missing");
            let on_disk = fs::read_to_string(&path).unwrap();
            let embedded = PARALLEL_FORGE_TEMPLATES
                .iter()
                .find(|t| t.file_name == *name)
                .unwrap()
                .content;
            assert_eq!(on_disk, embedded, "{name} content mismatch");
        }
    }

    #[test]
    fn materialize_accepts_builtin_prefix() {
        let dir = tempfile::tempdir().unwrap();
        let paths = materialize("builtin:parallel-forge", dir.path()).unwrap();
        assert_eq!(paths.len(), PARALLEL_FORGE_TEMPLATE_NAMES.len());
    }

    #[test]
    fn materialize_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let first = materialize("parallel-forge", dir.path()).unwrap();
        let second = materialize("parallel-forge", dir.path()).unwrap();
        assert_eq!(first.len(), second.len());
        let marker = dir.path().join("development-plan.template.md");
        assert!(
            fs::read_to_string(&marker)
                .unwrap()
                .contains("## 3. BDD 行为规格")
        );
    }

    #[test]
    fn materialize_unknown_preset_fails_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let err = materialize("no-such-preset", dir.path()).unwrap_err();
        assert!(err.to_string().contains("no embedded artifact templates"));
        assert!(fs::read_dir(dir.path()).unwrap().next().is_none());
    }

    // plan 2026-07-29-001 U2: ce-executor-pipeline template registration.
    // Three templates (rubric + settlement-evidence + README) ship in the
    // binary so executor / fixer / precheck gates can materialize them
    // without a repo checkout.

    #[test]
    fn ce_executor_pipeline_embeds_expected_count_and_names() {
        assert_eq!(
            CE_EXECUTOR_PIPELINE_TEMPLATES.len(),
            CE_EXECUTOR_PIPELINE_TEMPLATE_NAMES.len()
        );
        let names = list_template_names("ce-executor-pipeline").unwrap();
        for expected in CE_EXECUTOR_PIPELINE_TEMPLATE_NAMES {
            assert!(
                names.contains(expected),
                "missing embedded template {expected}"
            );
        }
        for t in CE_EXECUTOR_PIPELINE_TEMPLATES {
            assert!(!t.content.is_empty(), "{} must be non-empty", t.file_name);
            assert!(
                !t.content.contains('\0'),
                "{} must be valid UTF-8 text",
                t.file_name
            );
        }
    }

    #[test]
    fn ce_executor_pipeline_materialize_writes_all_files_with_content() {
        let dir = tempfile::tempdir().unwrap();
        let paths = materialize("ce-executor-pipeline", dir.path()).unwrap();
        assert_eq!(paths.len(), CE_EXECUTOR_PIPELINE_TEMPLATE_NAMES.len());
        for name in CE_EXECUTOR_PIPELINE_TEMPLATE_NAMES {
            let path = dir.path().join(name);
            assert!(path.is_file(), "{name} missing");
            let on_disk = fs::read_to_string(&path).unwrap();
            let embedded = CE_EXECUTOR_PIPELINE_TEMPLATES
                .iter()
                .find(|t| t.file_name == *name)
                .unwrap()
                .content;
            assert_eq!(on_disk, embedded, "{name} content mismatch");
        }
    }

    #[test]
    fn ce_executor_pipeline_materialize_is_idempotent() {
        let dir = tempfile::tempdir().unwrap();
        let first = materialize("ce-executor-pipeline", dir.path()).unwrap();
        let second = materialize("ce-executor-pipeline", dir.path()).unwrap();
        assert_eq!(first.len(), second.len());
        let marker = dir.path().join("fail-confidence-rubric.template.md");
        assert!(
            fs::read_to_string(&marker)
                .unwrap()
                .contains("## §1. 四维度评分对照表")
        );
    }

    #[test]
    fn ce_executor_pipeline_rubric_template_keeps_four_sections() {
        let body = CE_EXECUTOR_PIPELINE_TEMPLATES
            .iter()
            .find(|t| t.file_name == "fail-confidence-rubric.template.md")
            .unwrap()
            .content;
        for marker in [
            "## §1. 四维度评分对照表",
            "## §2. 阈值表",
            "## §3. Coverage 计算规则",
            "## §4. 六类 failed_checks 命名清单",
            "missing_attempt_record",
            "single_angle_retries",
            "broken_causal_chain",
            "unverifiable_evidence",
            "confidence_inflated",
            "uneliminated_alternatives",
        ] {
            assert!(
                body.contains(marker),
                "fail-confidence-rubric.template.md missing marker: {marker}"
            );
        }
    }

    #[test]
    fn ce_executor_pipeline_evidence_template_keeps_unit_section_structure() {
        let body = CE_EXECUTOR_PIPELINE_TEMPLATES
            .iter()
            .find(|t| t.file_name == "settlement-evidence.template.md")
            .unwrap()
            .content;
        for marker in [
            "### 尝试记录（1 初始 + 3 retry，共 4 次）",
            "### 最终假设的因果链",
            "### 假因排除记录",
            "### 证据来源清单",
            "## 自评（Self-Assessment）",
        ] {
            assert!(
                body.contains(marker),
                "settlement-evidence.template.md missing marker: {marker}"
            );
        }
    }

    /// Contract: development-plan template keeps Spec-First / BDD / TDD scaffolding.
    #[test]
    fn development_plan_template_retains_bdd_tdd_sections() {
        let body = PARALLEL_FORGE_TEMPLATES
            .iter()
            .find(|t| t.file_name == "development-plan.template.md")
            .unwrap()
            .content;
        for marker in [
            "## 3. BDD 行为规格",
            "Scenario:",
            "TDD 最小行为拆分",
            "Red → Green → Refactor",
        ] {
            assert!(
                body.contains(marker),
                "development-plan.template.md missing marker: {marker}"
            );
        }
    }

    /// Contract: unit template keeps acceptance / TDD fields for executor RED-GREEN.
    #[test]
    fn unit_template_retains_acceptance_and_tdd_fields() {
        let body = PARALLEL_FORGE_TEMPLATES
            .iter()
            .find(|t| t.file_name == "unit.template.yml")
            .unwrap()
            .content;
        for marker in ["acceptance_criteria:", "acceptance_test:", "tdd:"] {
            assert!(
                body.contains(marker),
                "unit.template.yml missing marker: {marker}"
            );
        }
    }

    /// Contract: manager report keeps Scenario + TDD summary sections.
    #[test]
    fn manager_report_template_retains_scenario_and_tdd() {
        let body = PARALLEL_FORGE_TEMPLATES
            .iter()
            .find(|t| t.file_name == "manager-report.template.md")
            .unwrap()
            .content;
        for marker in ["## 6. Scenario 验收结果", "**TDD 执行情况**"] {
            assert!(
                body.contains(marker),
                "manager-report.template.md missing marker: {marker}"
            );
        }
    }

    /// Contract: README documents binary materialize path (not repo-only copy).
    #[test]
    fn readme_documents_materialize_cli() {
        let body = PARALLEL_FORGE_TEMPLATES
            .iter()
            .find(|t| t.file_name == "README.md")
            .unwrap()
            .content;
        assert!(body.contains("ralph preset materialize-artifacts"));
        assert!(body.contains("binary") || body.contains("二进制"));
    }
}
