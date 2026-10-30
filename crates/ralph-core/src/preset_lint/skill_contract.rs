//! Skill dependency contract evaluation (2026-10-30 preset skill
//! contract spec).
//!
//! The `skills.contract` list inside a preset is the single source of
//! truth for "which hat needs which skill, when, and how the dependency
//! is satisfied". Every surface that has to answer that question —
//! `ralph preset check`, `ralph preflight`, the `ralph run` startup
//! hard gate, `ralph tools skill check`, and the author/review agent
//! skills — evaluates the same declaration through this module. No
//! consumer maintains a parallel dependency list.
//!
//! What the check verifies for every declared entry:
//!
//! 1. **Resolution** — the skill resolves in the *effective* registry
//!    the runtime will use (built-in + `skills.dirs`, with
//!    `skills.overrides` applied and the `ralph-tools-tasks` /
//!    `ralph-tools-memories` removal honoured). A same-named file
//!    elsewhere on the machine does NOT satisfy a dependency.
//! 2. **Source constraint** — `source: builtin|project|any` matches
//!    the resolved `SkillSource`.
//! 3. **Hat + backend visibility** — every consuming hat can actually
//!    see the skill under the active backend (the same
//!    `is_hat_eligible` predicate the prompt injector and the
//!    `ralph tools skill load` ACL use).
//! 4. **Load timing** — `load: auto_inject` requires the effective
//!    skill to carry `auto_inject: true` (via frontmatter or
//!    `skills.overrides`).
//! 5. **External requirements** — declared `requires.tools` resolve in
//!    `PATH`, `requires.env` is set, `requires.files` exists under the
//!    workspace root.
//! 6. **Unattended fit** — filesystem skills are content-scanned for
//!    interactive-requirement markers; an `approved` verdict must be
//!    pinned to the reviewed content via `unattended.content_hash`
//!    (`sha256:<hex>`), so any edit invalidates the approval. A
//!    self-declared flag alone never suffices.
//! 7. **Declaration coverage** — every `skill load <name>` reference
//!    in hat `instructions:` / `extra_instructions:` and preset-level
//!    `core.guardrails` must be covered by a contract entry. Required
//!    deps can therefore never silently degrade to "instructions load
//!    something nobody declared".

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use regex::Regex;
use sha2::{Digest, Sha256};

use super::finding_id::*;
use super::{LintFinding, LintSeverity, LintStrictness};
use crate::config::{
    RalphConfig, SkillContractEntry, SkillLoadTiming, SkillSourceConstraint, UnattendedStatus,
};
use crate::skill::SkillSource;
use crate::skill_registry::SkillRegistry;

/// Phrases that indicate a skill body expects human interaction.
/// Matched case-insensitively against the frontmatter-stripped skill
/// content of **filesystem** skills (binary-embedded skills are
/// curated with the release and are exempt from the marker scan —
/// their unattended status is fixed by the vendored content, and the
/// builtin `ralph-html*` family deliberately mentions review flows
/// that would false-positive here).
const UNATTENDED_RISK_MARKERS: &[&str] = &[
    "ask the user",
    "ask a human",
    "wait for the user",
    "wait for user",
    "human input",
    "human confirmation",
    "human approval",
    "human-in-the-loop",
    "human in the loop",
    "interactive login",
    "interactive prompt",
    "interactive session",
    "press enter",
    "press any key",
    "confirm with the user",
    "manual login",
    "manual approval",
    "manual intervention",
    "requires manual",
    "require manual",
    "do not proceed without",
    "read from stdin",
    "reads stdin",
    "pause for confirmation",
    "authenticate interactively",
    "log in interactively",
    "requires authentication",
    // Chinese equivalents — builtin and project presets ship `-zh`
    // variants, so a zh-authored skill body must not evade the scan.
    // These only annotate `risk_markers` for manual review; they do
    // not independently block a dependency.
    "需要人工",
    "需人工",
    "人工确认",
    "人工审批",
    "人工干预",
    "人工操作",
    "交互式登录",
    "手动登录",
    "等待用户",
    "询问用户",
    "按回车",
    "用户确认",
];

/// `skill load <name>` references in instruction text. Matches the
/// canonical `ralph tools skill load <name>` command shape (the
/// `ralph tools` prefix is optional so `skill load x` phrasing is
/// also caught); the captured group is the skill name.
fn skill_load_ref_regex() -> &'static Regex {
    static RE: std::sync::OnceLock<Regex> = std::sync::OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(r"skill\s+load\s+([A-Za-z0-9][A-Za-z0-9_-]*)")
            .expect("skill-load reference regex must compile")
    })
}

/// Per-dependency evaluation result — the machine-readable substrate
/// behind `ralph tools skill check`. `findings` carries the same
/// `LintFinding` values the lint path emits, so the CLI report and the
/// startup gate can never disagree about whether a dependency passes.
#[derive(Debug, Clone)]
pub struct SkillDepStatus {
    /// Declared registry name.
    pub skill: String,
    /// Consuming hats after `*` expansion.
    pub hats: Vec<String>,
    /// Whether the dep blocks startup when unmet.
    pub required: bool,
    /// Resolved in the effective registry.
    pub resolved: bool,
    /// Resolved from a binary-embedded source.
    pub builtin: bool,
    /// Filesystem source path (when `SkillSource::File`).
    pub source_path: Option<PathBuf>,
    /// `sha256:<hex>` of the resolved skill body — the value to pin in
    /// `unattended.content_hash` when approving a filesystem skill.
    pub content_hash: Option<String>,
    /// Declared hats that cannot see the skill (hat or backend filter).
    pub invisible_hats: Vec<String>,
    /// `requires.tools` entries not found in `PATH`.
    pub missing_tools: Vec<String>,
    /// `requires.env` entries not set in the environment.
    pub missing_env: Vec<String>,
    /// `requires.files` entries missing under the workspace root.
    pub missing_files: Vec<String>,
    /// Interactive-requirement markers found in the skill body
    /// (filesystem sources only).
    pub risk_markers: Vec<String>,
    /// `true` when no `Error`-severity finding targets this entry.
    pub ok: bool,
}

/// Full evaluation result: the lint findings (for `run_preset_lint`,
/// `ralph preset check`, `ralph preflight`, and the startup gate)
/// plus per-dependency statuses (for `ralph tools skill check`).
#[derive(Debug, Clone)]
pub struct SkillContractOutcome {
    /// Lint findings, sorted deterministically by id then hat.
    pub findings: Vec<LintFinding>,
    /// Per-declared-entry status, in contract order.
    pub statuses: Vec<SkillDepStatus>,
}

/// Evaluate the preset's skill contract against the real environment.
///
/// `strictness` only grades the *declaration-coverage* finding
/// (`preset.skill_contract_undeclared_reference`): Warn in default
/// mode, Error in strict — mirroring how the startup gate escalates
/// structural drift. Every functional failure (missing skill, hidden
/// skill, wrong source, broken auto-inject wiring, missing
/// tool/env/file, unattended-unfit required dep) is `Error` for
/// `required: true` entries and `Warn` for `required: false` entries
/// regardless of strictness — required deps never silently downgrade,
/// and optional deps never silently pass.
pub fn evaluate_skill_contract(
    config: &RalphConfig,
    strictness: LintStrictness,
) -> SkillContractOutcome {
    let mut findings: Vec<LintFinding> = Vec::new();
    let mut statuses: Vec<SkillDepStatus> = Vec::new();

    // ── 1. Resolve the effective contract (inline + contract_file). ──
    let contract = match effective_contract(config, &mut findings) {
        Some(c) => c,
        None => return SkillContractOutcome { findings, statuses },
    };

    // ── 2. Declaration-shape validation. ──
    let hat_ids: BTreeSet<&str> = config.hats.keys().map(String::as_str).collect();
    let mut valid_indices: Vec<usize> = Vec::new();
    for (idx, entry) in contract.iter().enumerate() {
        let label = entry_label(idx, entry);
        let mut invalid = false;
        if entry.skill.trim().is_empty() {
            findings.push(
                LintFinding::error(
                    FINDING_SKILL_CONTRACT_INVALID,
                    format!("skills.contract[{idx}] is missing `skill:` (registry name)"),
                )
                .with_action_hint("Set `skill:` to the name `ralph tools skill load` resolves."),
            );
            invalid = true;
        }
        if entry.hats.is_empty() {
            findings.push(
                LintFinding::error(
                    FINDING_SKILL_CONTRACT_INVALID,
                    format!("{label}: `hats` is empty — declare the consuming hat ids or `*`"),
                )
                .with_action_hint("List consuming hat ids, or `*` for every hat in the preset."),
            );
            invalid = true;
        } else {
            for hat in &entry.hats {
                if hat != "*" && !hat_ids.contains(hat.as_str()) {
                    findings.push(
                        LintFinding::error(
                            FINDING_SKILL_CONTRACT_INVALID,
                            format!(
                                "{label}: `hats` references unknown hat \"{hat}\" (not declared in preset `hats:`)"
                            ),
                        )
                        .with_hat(hat.clone())
                        .with_action_hint(
                            "Fix the hat id or add the hat to the preset's `hats:` map.",
                        ),
                    );
                    invalid = true;
                }
            }
        }
        if entry.purpose.trim().is_empty() {
            findings.push(
                LintFinding::error(
                    FINDING_SKILL_CONTRACT_INVALID,
                    format!("{label}: `purpose` is empty — record what the hat uses the skill for"),
                )
                .with_action_hint(
                    "Describe the dependency purpose so reviewers can judge necessity.",
                ),
            );
            invalid = true;
        }
        if !entry.required && entry.missing_behavior.is_none() {
            findings.push(
                LintFinding::error(
                    FINDING_SKILL_CONTRACT_INVALID,
                    format!(
                        "{label}: optional dependency without `missing_behavior` — 'optional' must never mean 'silently degraded'"
                    ),
                )
                .with_action_hint(
                    "Add `missing_behavior:` describing the documented degradation, or mark `required: true`.",
                ),
            );
            invalid = true;
        }
        if !invalid {
            valid_indices.push(idx);
        }
    }

    // ── 3. Build the effective registry the runtime will use. ──
    // Mirrors event_loop::acceptance_and_lifecycle: honour
    // `skills.enabled`, apply `skills.overrides` inside
    // `from_config`, and remove the task/memory skills when their
    // subsystems are disabled — so a declaration can never be
    // satisfied by a skill the runtime would not actually expose.
    let backend = Some(config.cli.backend.as_str());
    let registry = if config.skills.enabled {
        match SkillRegistry::from_config(&config.skills, &config.core.workspace_root, backend) {
            Ok(mut registry) => {
                if !config.tasks.enabled {
                    registry.remove("ralph-tools-tasks");
                }
                if !config.memories.enabled {
                    registry.remove("ralph-tools-memories");
                }
                Some(registry)
            }
            Err(e) => {
                findings.push(LintFinding::error(
                    FINDING_SKILL_CONTRACT_REGISTRY_ERROR,
                    format!("skill registry could not be built: {e}"),
                ));
                None
            }
        }
    } else {
        None
    };

    // ── 4. Per-entry environment evaluation. ──
    let workspace_root = config.core.workspace_root.clone();
    for (idx, entry) in contract.iter().enumerate() {
        if !valid_indices.contains(&idx) {
            // Invalid declarations were already reported; still emit a
            // status row so `ralph tools skill check` shows the entry.
            statuses.push(SkillDepStatus {
                skill: entry.skill.clone(),
                hats: expand_hats(config, entry),
                required: entry.required,
                resolved: false,
                builtin: false,
                source_path: None,
                content_hash: None,
                invisible_hats: Vec::new(),
                missing_tools: Vec::new(),
                missing_env: Vec::new(),
                missing_files: Vec::new(),
                risk_markers: Vec::new(),
                ok: false,
            });
            continue;
        }

        let label = entry_label(idx, entry);
        let entry_severity = if entry.required {
            LintSeverity::Error
        } else {
            LintSeverity::Warn
        };
        let mut entry_findings: Vec<LintFinding> = Vec::new();
        let mut status = SkillDepStatus {
            skill: entry.skill.clone(),
            hats: expand_hats(config, entry),
            required: entry.required,
            resolved: false,
            builtin: false,
            source_path: None,
            content_hash: None,
            invisible_hats: Vec::new(),
            missing_tools: Vec::new(),
            missing_env: Vec::new(),
            missing_files: Vec::new(),
            risk_markers: Vec::new(),
            ok: true,
        };

        let install_hint = entry.install_hint.as_deref().unwrap_or("");
        let remediation = |entry: &SkillContractEntry| -> String {
            if entry.required {
                format!(
                    "{install_hint} Required deps must be installable + loadable before `ralph run`; \
                     a same-named file outside `skills.dirs` does not satisfy the dependency. \
                     After fixing, re-run `ralph tools skill check` / `ralph preset check` to recheck.",
                )
            } else {
                format!(
                    "{install_hint} Optional dep documented fallback: {}",
                    entry.missing_behavior.as_deref().unwrap_or("(none)")
                )
            }
        };

        // ── 4a. skills.enabled / registry availability. ──
        let Some(registry) = registry.as_ref() else {
            if !config.skills.enabled {
                entry_findings.push(
                    dep_finding(
                        FINDING_SKILL_SKILLS_DISABLED,
                        entry_severity,
                        format!(
                            "{label}: skills subsystem is disabled (`skills.enabled: false`) — \
                         hat(s) {} cannot load it at runtime",
                            status.hats.join(", ")
                        ),
                        entry,
                    )
                    .with_action_hint(
                        "Enable `skills.enabled` or remove the dependency from `skills.contract`.",
                    ),
                );
                findings.extend(entry_findings);
            }
            // When the registry build itself failed, the
            // `FINDING_SKILL_CONTRACT_REGISTRY_ERROR` emitted above is
            // the root cause — do not misattribute it as
            // `skills.enabled: false` on every dep.
            status.ok = entry_severity != LintSeverity::Error;
            statuses.push(status);
            continue;
        };

        // ── 4b. Resolution in the effective registry. ──
        let Some(skill) = registry.get(&entry.skill) else {
            let mut msg = format!(
                "{label}: skill does not resolve in the effective registry \
                 (checked built-ins{}); hat(s) {} would fail at `ralph tools skill load {}`",
                if config.skills.dirs.is_empty() {
                    String::new()
                } else {
                    format!(" + skills.dirs {}", dirs_display(&config.skills.dirs))
                },
                status.hats.join(", "),
                entry.skill,
            );
            if !install_hint.is_empty() {
                msg.push_str(&format!("; install: {install_hint}"));
            }
            entry_findings.push(
                dep_finding(
                    if entry.required {
                        FINDING_SKILL_REQUIRED_MISSING
                    } else {
                        FINDING_SKILL_OPTIONAL_MISSING
                    },
                    entry_severity,
                    msg,
                    entry,
                )
                .with_action_hint(remediation(entry)),
            );
            status.ok = false;
            findings.extend(entry_findings);
            statuses.push(status);
            continue;
        };

        status.resolved = true;
        status.builtin = matches!(skill.source, SkillSource::BuiltIn);
        status.source_path = match &skill.source {
            SkillSource::File(p) => Some(p.clone()),
            SkillSource::BuiltIn => None,
        };
        status.content_hash = Some(skill_content_hash(&skill.content));

        // ── 4c. Source constraint. ──
        let source_ok = match entry.source {
            SkillSourceConstraint::Any => true,
            SkillSourceConstraint::Builtin => status.builtin,
            SkillSourceConstraint::Project => !status.builtin,
        };
        if !source_ok {
            entry_findings.push(
                dep_finding(
                    FINDING_SKILL_SOURCE_MISMATCH,
                    entry_severity,
                    format!(
                        "{label}: contract requires `source: {}` but resolved {} {}",
                        serde_value(&entry.source),
                        if status.builtin { "built-in" } else { "file" },
                        status
                            .source_path
                            .as_ref()
                            .map(|p| format!("at {}", p.display()))
                            .unwrap_or_else(|| "skill".to_string()),
                    ),
                    entry,
                )
                .with_action_hint(
                    "Either relax `source:` in the contract or supply the skill from the constrained source.",
                ),
            );
        }

        // ── 4d. Hat + backend visibility. ──
        if status.hats.is_empty() {
            // Solo / hatless preset: mirror `skills_for_hat(None)`.
            if !registry
                .skills_for_hat(None)
                .iter()
                .any(|s| s.name == entry.skill)
            {
                status.invisible_hats.push("<solo>".to_string());
            }
        } else {
            for hat in &status.hats {
                if !registry.is_hat_eligible(&entry.skill, hat) {
                    status.invisible_hats.push(hat.clone());
                }
            }
        }
        if !status.invisible_hats.is_empty() {
            entry_findings.push(
                dep_finding(
                    FINDING_SKILL_NOT_VISIBLE_TO_HAT,
                    entry_severity,
                    format!(
                        "{label}: skill resolves but is not visible to hat(s) {} under backend \"{}\" \
                         (hat/backend filters hide it — the hat's `ralph tools skill load {}` call will be refused)",
                        status.invisible_hats.join(", "),
                        config.cli.backend,
                        entry.skill,
                    ),
                    entry,
                )
                .with_action_hint(format!(
                    "Widen visibility via `skills.overrides.\"{}\".hats` (add the hat) / `.backends`, \
                     or fix the skill frontmatter `hats:`/`backends:` list.",
                    entry.skill
                )),
            );
        }

        // ── 4e. Load-timing wiring. ──
        if entry.load == SkillLoadTiming::AutoInject && !skill.auto_inject {
            entry_findings.push(
                dep_finding(
                    FINDING_SKILL_AUTOINJECT_MISMATCH,
                    entry_severity,
                    format!(
                        "{label}: contract declares `load: auto_inject` but the effective skill \
                         has `auto_inject: false` — the prompt injector would never deliver it",
                    ),
                    entry,
                )
                .with_action_hint(format!(
                    "Set `skills.overrides.\"{}\".auto_inject: true`, or change the contract to `load: on_demand`.",
                    entry.skill
                )),
            );
        }

        // ── 4f. External requirements. ──
        for tool in &entry.requires.tools {
            if crate::preflight::find_executable(tool).is_none() {
                status.missing_tools.push(tool.clone());
            }
        }
        if !status.missing_tools.is_empty() {
            entry_findings.push(
                dep_finding(
                    FINDING_SKILL_REQUIRES_TOOL_MISSING,
                    entry_severity,
                    format!(
                        "{label}: required tool(s) not found in PATH: {}",
                        status.missing_tools.join(", ")
                    ),
                    entry,
                )
                .with_action_hint(
                    "Install the tool(s) (see `install_hint`) or remove them from `requires.tools`.",
                ),
            );
        }
        for var in &entry.requires.env {
            if std::env::var_os(var).is_none() {
                status.missing_env.push(var.clone());
            }
        }
        if !status.missing_env.is_empty() {
            entry_findings.push(
                dep_finding(
                    FINDING_SKILL_REQUIRES_ENV_MISSING,
                    entry_severity,
                    format!(
                        "{label}: required environment variable(s) not set: {}",
                        status.missing_env.join(", ")
                    ),
                    entry,
                )
                .with_action_hint(
                    "Export the variable(s) before `ralph run` (values are never inspected — only presence).",
                ),
            );
        }
        for file in &entry.requires.files {
            let path = Path::new(file);
            let resolved = if path.is_absolute() {
                path.to_path_buf()
            } else {
                workspace_root.join(path)
            };
            if !resolved.exists() {
                status.missing_files.push(file.clone());
            }
        }
        if !status.missing_files.is_empty() {
            entry_findings.push(
                dep_finding(
                    FINDING_SKILL_REQUIRES_FILE_MISSING,
                    entry_severity,
                    format!(
                        "{label}: required companion file(s) missing under workspace root: {}",
                        status.missing_files.join(", ")
                    ),
                    entry,
                )
                .with_action_hint(
                    "Create the file(s) (see `install_hint`) or drop them from `requires.files`.",
                ),
            );
        }

        // ── 4g. Unattended fit. ──
        if !status.builtin {
            let lower = skill.content.to_lowercase();
            status.risk_markers = UNATTENDED_RISK_MARKERS
                .iter()
                .filter(|m| lower.contains(*m))
                .map(|m| m.to_string())
                .collect();
        }
        evaluate_unattended(
            entry,
            &label,
            entry_severity,
            status.builtin,
            status.content_hash.as_deref(),
            &status.risk_markers,
            &mut entry_findings,
        );

        status.ok = !entry_findings
            .iter()
            .any(|f| f.severity == LintSeverity::Error);
        findings.extend(entry_findings);
        statuses.push(status);
    }

    // ── 5. Instruction-reference coverage audit. ──
    // Every `skill load <name>` a hat actually references must be
    // covered by a contract entry — this is what makes the contract a
    // single source of truth rather than parallel documentation.
    // `core.guardrails` is preset-global text: a reference there is
    // covered when ANY entry declares the skill (hat attribution is
    // not derivable from global guidance).
    findings.extend(coverage_findings(config, &contract, strictness));

    findings.sort_by(|a, b| a.id.cmp(b.id).then(a.hat.cmp(&b.hat)));
    SkillContractOutcome { findings, statuses }
}

/// Lint-path convenience wrapper — used by `run_preset_lint` so the
/// startup gate, `ralph preset check`, and `ralph preflight` inherit
/// the same verdicts without duplicating logic.
pub fn check_skill_contract(config: &RalphConfig, strictness: LintStrictness) -> Vec<LintFinding> {
    evaluate_skill_contract(config, strictness).findings
}

// ── helpers ─────────────────────────────────────────────────────────────

/// Merge `skills.contract` with `skills.contract_file` contents.
/// Returns `None` (after pushing a finding) when the contract file is
/// declared but unreadable/invalid.
fn effective_contract(
    config: &RalphConfig,
    findings: &mut Vec<LintFinding>,
) -> Option<Vec<SkillContractEntry>> {
    let mut contract = config.skills.contract.clone();

    if let Some(path) = &config.skills.contract_file {
        let resolved = resolve_contract_file(config, path);
        match resolved {
            Some(file_path) => match load_contract_file(&file_path) {
                Ok(entries) => contract.extend(entries),
                Err(e) => {
                    findings.push(
                        LintFinding::error(
                            FINDING_SKILL_CONTRACT_FILE_ERROR,
                            format!(
                                "skills.contract_file {} could not be parsed: {e}",
                                file_path.display()
                            ),
                        )
                        .with_action_hint(
                            "Fix the contract file (YAML/JSON list of entries or a map with `contract:`) or remove `contract_file`.",
                        ),
                    );
                    return None;
                }
            },
            None => {
                findings.push(
                    LintFinding::error(
                        FINDING_SKILL_CONTRACT_FILE_ERROR,
                        format!(
                            "skills.contract_file {} not found (resolved against the declaring preset/config dir and workspace root {})",
                            path.display(),
                            config.core.workspace_root.display()
                        ),
                    )
                    .with_action_hint(
                        "Create the contract file or remove the `skills.contract_file` reference.",
                    ),
                );
                return None;
            }
        }
    }

    // The merged list can carry the same (skill, hats) pair twice —
    // e.g. declared both inline and via `contract_file`, or by the
    // preset and the operator config in the `-H` union. Identical
    // duplicates collapse silently; *conflicting* duplicates are an
    // Error because two different verdicts could otherwise be applied
    // to the same hat's dependency — that ambiguity is exactly what
    // the single-source-of-truth rule exists to prevent. The key uses
    // the *expanded* hat set, so `hats: ["*"]` collides with an
    // explicit `hats: [a, b]` when the preset declares hats a,b.
    let mut seen: HashMap<(String, Vec<String>), usize> = HashMap::new();
    let mut deduped: Vec<SkillContractEntry> = Vec::with_capacity(contract.len());
    for mut entry in contract {
        entry.skill = entry.skill.trim().to_string();
        entry.hats.sort();
        entry.hats.dedup();
        let key = (entry.skill.clone(), expand_hats(config, &entry));
        match seen.get(&key) {
            Some(&idx) if deduped[idx] == entry => continue,
            Some(&idx) => {
                findings.push(
                    LintFinding::error(
                        FINDING_SKILL_CONTRACT_INVALID,
                        format!(
                            "conflicting contract declarations for skill '{}' with hats [{}]; the first declaration wins",
                            deduped[idx].skill,
                            key.1.join(", ")
                        ),
                    )
                    .with_action_hint(
                        "Keep a single declaration per skill+hat-set — merge the differing fields into one entry.",
                    ),
                );
            }
            None => {
                seen.insert(key, deduped.len());
                deduped.push(entry);
            }
        }
    }

    Some(deduped)
}

/// Resolve `contract_file` relative to the directory of the file that
/// declared it first (preset dir when the `-H` preset won the merged
/// value, else the config file's dir), then the workspace root.
fn resolve_contract_file(config: &RalphConfig, path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return path.exists().then(|| path.to_path_buf());
    }
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(dir) = &config.contract_file_origin_dir {
        candidates.push(dir.join(path));
    }
    if let Some(dir) = config.config_path.as_ref().and_then(|p| p.parent()) {
        candidates.push(dir.join(path));
    }
    candidates.push(config.core.workspace_root.join(path));
    candidates.into_iter().find(|c| c.is_file())
}

/// Parse a contract file: either a bare list of entries or a map with
/// a top-level `contract:` key. YAML and JSON are both accepted.
fn load_contract_file(path: &Path) -> Result<Vec<SkillContractEntry>, String> {
    let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    let value: serde_yaml::Value =
        serde_yaml::from_str(&text).map_err(|e| format!("YAML parse failed: {e}"))?;
    if value.is_sequence() {
        return serde_yaml::from_value(value).map_err(|e| e.to_string());
    }
    if let serde_yaml::Value::Mapping(map) = &value {
        let key = serde_yaml::Value::String("contract".to_string());
        if let Some(inner) = map.get(&key) {
            return serde_yaml::from_value(inner.clone()).map_err(|e| e.to_string());
        }
    }
    Err("expected a list of entries or a map with a `contract:` key".to_string())
}

/// Expand `hats: ["*"]` to every hat declared in the preset.
fn expand_hats(config: &RalphConfig, entry: &SkillContractEntry) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for hat in &entry.hats {
        if hat == "*" {
            out.extend(config.hats.keys().cloned());
        } else {
            out.push(hat.clone());
        }
    }
    out.sort();
    out.dedup();
    out
}

/// sha256 digest of the resolved skill body — binds
/// `unattended.content_hash` approvals to the exact reviewed content.
pub fn skill_content_hash(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    let digest = hasher.finalize();
    let mut out = String::with_capacity(7 + digest.len() * 2);
    out.push_str("sha256:");
    for byte in digest {
        out.push_str(&format!("{byte:02x}"));
    }
    out
}

fn entry_label(idx: usize, entry: &SkillContractEntry) -> String {
    if entry.skill.trim().is_empty() {
        format!("skills.contract[{idx}]")
    } else {
        format!("skills.contract[{}] (skill \"{}\")", idx, entry.skill)
    }
}

fn dep_finding(
    id: &'static str,
    severity: LintSeverity,
    message: impl Into<String>,
    entry: &SkillContractEntry,
) -> LintFinding {
    let mut finding = LintFinding::new(id, message);
    finding.severity = severity;
    if let Some(hat) = entry.hats.first()
        && hat != "*"
    {
        finding.hat = Some(hat.clone());
    }
    finding
}

fn dirs_display(dirs: &[PathBuf]) -> String {
    let names: Vec<String> = dirs.iter().map(|d| d.display().to_string()).collect();
    format!("[{}]", names.join(", "))
}

fn serde_value<T: serde::Serialize>(v: &T) -> String {
    serde_yaml::to_string(v)
        .unwrap_or_default()
        .trim()
        .to_string()
}

/// Unattended-fit evaluation for one resolved dependency.
///
/// Rules:
/// - `unattended` absent on a filesystem dep: `required` →
///   `skill_unattended_undeclared` (Error); `optional` → Warn. The
///   contract must record the unattended story for every external dep.
/// - `status: needs_review` → `skill_unattended_unverified`.
/// - `status: interactive` → `skill_unattended_interactive` — the dep
///   cannot run unattended; adapt the skill or change the preset.
/// - `status: approved` on a filesystem dep requires `content_hash`;
///   a missing hash → `skill_unattended_missing_hash`, a mismatch →
///   `skill_unattended_stale` (the approval is void — content changed
///   since review). Risk markers found in the body are surfaced in the
///   finding detail so the reviewer sees what triggered re-review.
fn evaluate_unattended(
    entry: &SkillContractEntry,
    label: &str,
    severity: LintSeverity,
    builtin: bool,
    current_hash: Option<&str>,
    risk_markers: &[String],
    findings: &mut Vec<LintFinding>,
) {
    let marker_note = |markers: &[String]| -> String {
        if markers.is_empty() {
            String::new()
        } else {
            format!(
                " Content scan flags interactive-requirement marker(s): {}.",
                markers.join(", ")
            )
        }
    };

    match &entry.unattended {
        None => {
            if builtin {
                // Binary-embedded skills are curated with the release —
                // an absent block carries no risk signal.
                return;
            }
            findings.push(
                dep_finding(
                    FINDING_SKILL_UNATTENDED_UNDECLARED,
                    severity,
                    format!(
                        "{label}: filesystem skill has no `unattended` declaration — \
                         presence on disk is not evidence it can run unattended.{}",
                        marker_note(risk_markers)
                    ),
                    entry,
                )
                .with_action_hint(
                    "Review the skill for interactive steps, then add an `unattended:` block \
                     (`status: approved` + `content_hash` from `ralph tools skill check`).",
                ),
            );
        }
        Some(unattended) => match unattended.status {
            UnattendedStatus::NeedsReview => {
                findings.push(
                    dep_finding(
                        FINDING_SKILL_UNATTENDED_UNVERIFIED,
                        severity,
                        format!(
                            "{label}: `unattended.status: needs_review` — unattended fit was never established.{}",
                            marker_note(risk_markers)
                        ),
                        entry,
                    )
                    .with_action_hint(
                        "Complete the unattended review (content scan, required inputs, tool checks), \
                         then set `status: approved` with the current `content_hash`.",
                    ),
                );
            }
            UnattendedStatus::Interactive => {
                findings.push(
                    dep_finding(
                        FINDING_SKILL_UNATTENDED_INTERACTIVE,
                        severity,
                        format!(
                            "{label}: `unattended.status: interactive` — the skill requires human \
                             interaction and cannot satisfy an unattended loop dependency.",
                        ),
                        entry,
                    )
                    .with_action_hint(
                        "Adapt the skill (record the adaptation via `adapted_from`/`notes`), \
                         supply the human inputs via `requires.env`/`requires.files`, \
                         or change the preset so the dep is not needed.",
                    ),
                );
            }
            UnattendedStatus::Approved => {
                if builtin {
                    return; // embedded content is versioned with the binary
                }
                match (&unattended.content_hash, current_hash) {
                    (None, _) => {
                        findings.push(
                            dep_finding(
                                FINDING_SKILL_UNATTENDED_MISSING_HASH,
                                severity,
                                format!(
                                    "{label}: `unattended.status: approved` without `content_hash` — \
                                     a self-declared flag is not evidence; pin the reviewed content.",
                                ),
                                entry,
                            )
                            .with_action_hint(format!(
                                "Set `unattended.content_hash` to the current hash {}.",
                                current_hash.unwrap_or("<unresolved>")
                            )),
                        );
                    }
                    (Some(declared), Some(current)) => {
                        let declared = declared.trim();
                        if !declared.starts_with("sha256:") {
                            findings.push(
                                dep_finding(
                                    FINDING_SKILL_UNATTENDED_MISSING_HASH,
                                    severity,
                                    format!(
                                        "{label}: `unattended.content_hash` \"{declared}\" is not a \
                                         `sha256:<hex>` digest.",
                                    ),
                                    entry,
                                )
                                .with_action_hint(format!(
                                    "Use the hash printed by `ralph tools skill check` (current: {current}).",
                                )),
                            );
                        } else if declared != current {
                            findings.push(
                                dep_finding(
                                    FINDING_SKILL_UNATTENDED_STALE,
                                    severity,
                                    format!(
                                        "{label}: `unattended.content_hash` is stale — approved \
                                         digest {declared} != current content {current}. The skill \
                                         changed since review; the approval is void.{}",
                                        marker_note(risk_markers)
                                    ),
                                    entry,
                                )
                                .with_action_hint(format!(
                                    "Re-review the changed content, then update `content_hash` to {current}.",
                                )),
                            );
                        }
                    }
                    _ => {}
                }
            }
        },
    }
}

/// Declaration-coverage audit — every `skill load <name>` reference
/// found in instruction text must be covered by a contract entry.
fn coverage_findings(
    config: &RalphConfig,
    contract: &[SkillContractEntry],
    strictness: LintStrictness,
) -> Vec<LintFinding> {
    let re = skill_load_ref_regex();
    let mut findings = Vec::new();

    // hat_id -> referenced skill names
    let mut hat_refs: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for (hat_id, hat) in &config.hats {
        for name in re
            .captures_iter(&hat.instructions)
            .chain(
                hat.extra_instructions
                    .iter()
                    .flat_map(|e| re.captures_iter(e)),
            )
            .map(|c| c[1].to_string())
        {
            hat_refs.entry(hat_id.clone()).or_default().insert(name);
        }
    }
    // preset-global guardrails text
    let mut preset_refs: BTreeSet<String> = BTreeSet::new();
    for line in &config.core.guardrails {
        for name in re.captures_iter(line).map(|c| c[1].to_string()) {
            preset_refs.insert(name);
        }
    }

    if hat_refs.is_empty() && preset_refs.is_empty() {
        return findings;
    }

    // Severity policy: a preset with a contract has opted into the
    // single-source-of-truth invariant, so uncovered references are
    // graded by strictness (Error at the `ralph run` gate which runs
    // strict). A preset with no contract at all only earns a Warn —
    // we surface the audit signal without forcing adoption.
    let has_contract = !contract.is_empty();
    let uncovered_severity = if has_contract {
        strictness.ownership_severity()
    } else {
        LintSeverity::Warn
    };

    let covers = |skill: &str, hat: &str| -> bool {
        contract
            .iter()
            .any(|e| e.skill == skill && (e.hats.iter().any(|h| h == hat || h == "*")))
    };
    let covers_preset_level = |skill: &str| -> bool { contract.iter().any(|e| e.skill == skill) };

    for (hat, names) in &hat_refs {
        for name in names {
            if !covers(name, hat) {
                let mut finding = LintFinding::new(
                    FINDING_SKILL_CONTRACT_UNDECLARED_REFERENCE,
                    format!(
                        "hat \"{hat}\" instructions reference `skill load {name}` but \
                         `skills.contract` has no entry covering it — declare the \
                         dependency (purpose/hats/required/unattended) so startup \
                         validation can verify it"
                    ),
                )
                .with_hat(hat.clone())
                .with_action_hint(format!(
                    "Add a `skills.contract` entry for \"{name}\" listing hat \"{hat}\" \
                     (or drop the instruction reference).",
                ));
                finding.severity = uncovered_severity;
                findings.push(finding);
            }
        }
    }
    for name in &preset_refs {
        if !covers_preset_level(name) {
            let mut finding = LintFinding::new(
                FINDING_SKILL_CONTRACT_UNDECLARED_REFERENCE,
                format!(
                    "`core.guardrails` references `skill load {name}` but \
                     `skills.contract` has no entry for it — declare the dependency \
                     so startup validation can verify it"
                ),
            )
            .with_action_hint(format!(
                "Add a `skills.contract` entry for \"{name}\" covering the consuming hats.",
            ));
            finding.severity = uncovered_severity;
            findings.push(finding);
        }
    }

    findings
}

#[cfg(test)]
mod tests;
