//! Skills configuration.

use std::collections::HashMap;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::default_true;

/// Skills configuration.
///
/// Controls the skill discovery and injection system that makes tool
/// knowledge and domain expertise available to agents during loops.
///
/// Skills use a two-tier injection model: a compact skill index is always
/// present in every prompt, and the agent loads full skill content on demand
/// via `ralph tools skill load <name>`.
///
/// Example configuration:
/// ```yaml
/// skills:
///   enabled: true
///   dirs:
///     - .ralph/skills
///   overrides:
///     ralph-tools:
///       enabled: true
///       hats: [executor]
/// ```
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    /// Whether the skills system is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Directories to scan for skill files.
    /// Relative paths resolved against workspace root.
    #[serde(default)]
    pub dirs: Vec<PathBuf>,

    /// Skill dependency contract (2026-10-30 preset skill contract spec).
    ///
    /// Machine-readable list of the skills each hat depends on. This is
    /// the single source of truth for skill dependencies: preset lint,
    /// `ralph preset check`, `ralph preflight`, the `ralph run` startup
    /// gate, and `ralph tools skill check` all evaluate the same list.
    ///
    /// Entries are declarations, not mechanisms — declaring
    /// `load: auto_inject` does not inject the skill; it asserts that
    /// the preset configures injection elsewhere
    /// (`skills.overrides.<name>.auto_inject: true`).
    #[serde(default)]
    pub contract: Vec<SkillContractEntry>,

    /// Optional external contract file (YAML or JSON) merged with
    /// `contract`. Resolved relative to the preset config file's
    /// directory first, then the workspace root. The file may either
    /// be a bare list of entries or a map with a top-level
    /// `contract:` key.
    #[serde(default)]
    pub contract_file: Option<PathBuf>,

    /// Per-skill overrides keyed by skill name.
    #[serde(default)]
    pub overrides: HashMap<String, SkillOverride>,
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true, // Skills enabled by default
            dirs: vec![],
            contract: vec![],
            contract_file: None,
            overrides: HashMap::new(),
        }
    }
}

/// How a declared skill is expected to reach the hat's context.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillLoadTiming {
    /// The hat loads it on demand via `ralph tools skill load <name>`;
    /// the skill must resolve in the registry and be visible to the
    /// hat and active backend.
    #[default]
    OnDemand,
    /// The runtime must inject the content into the hat's prompt at
    /// activation. The contract asserts the preset wires this via
    /// `skills.overrides.<name>.auto_inject: true`; the lint verifies
    /// the effective skill has `auto_inject` enabled.
    AutoInject,
}

/// Constraint on where a declared skill may resolve from.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SkillSourceConstraint {
    /// Either binary-embedded or filesystem resolution satisfies the
    /// dependency.
    #[default]
    Any,
    /// Must resolve to a binary-embedded (`SkillSource::BuiltIn`)
    /// skill — e.g. the vendored `ralph-html*` family.
    Builtin,
    /// Must resolve to a filesystem skill discovered through
    /// `skills.dirs` — project-maintained or adapted copies.
    Project,
}

/// External conditions a skill needs beyond resolving in the registry.
///
/// Every entry is verifiable at check time: `tools` must be found in
/// `PATH`, `env` variables must be set, and `files` must exist under
/// the workspace root (absolute paths are honoured verbatim).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillRequirements {
    /// Executables that must be resolvable in `PATH`.
    #[serde(default)]
    pub tools: Vec<String>,

    /// Environment variables that must be set for the skill to work
    /// unattended (e.g. tokens the skill instructs the agent to read).
    /// Values are never surfaced — only presence is checked.
    #[serde(default)]
    pub env: Vec<String>,

    /// Files the skill requires, resolved against the workspace root.
    #[serde(default)]
    pub files: Vec<String>,
}

/// Declared unattended-fit status for a dependency.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum UnattendedStatus {
    /// Declared but not yet reviewed for unattended operation.
    /// Required deps in this state are not start-ready.
    #[default]
    NeedsReview,
    /// Content was reviewed and found fit for unattended runs. For
    /// filesystem skills `content_hash` must pin the reviewed version;
    /// a stale hash voids the approval.
    Approved,
    /// The skill inherently requires human interaction (waits for
    /// answers, manual steps, interactive login). Required deps in
    /// this state block startup — the preset or the skill must be
    /// adapted instead of silently degrading.
    Interactive,
}

/// Unattended-fit declaration for a contract dependency.
///
/// A self-declared flag alone is not sufficient evidence: for
/// filesystem skills the check additionally scans the skill content
/// for interactive-requirement markers, and `content_hash` binds the
/// approval to the reviewed version so a content change forces
/// re-review.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillUnattended {
    /// Fit status (see [`UnattendedStatus`]).
    #[serde(default)]
    pub status: UnattendedStatus,

    /// `sha256:<hex>` of the reviewed skill body, as printed by
    /// `ralph tools skill check`. Required for `status: approved` on
    /// filesystem skills; optional for binary-embedded skills.
    #[serde(default)]
    pub content_hash: Option<String>,

    /// Provenance / review notes — who adapted the skill, what was
    /// checked, why it is unattended-fit despite any markers.
    #[serde(default)]
    pub notes: Option<String>,
}

/// One declared skill dependency in the preset contract.
///
/// The contract records which hat(s) need which skill, for what
/// purpose, whether the dependency is required or optional, how and
/// when it is loaded, where it may resolve from, and what unattended
/// operation requires. `hats` accepts the wildcard `"*"` meaning
/// every hat declared in the preset.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SkillContractEntry {
    /// Registry name of the skill (`ralph tools skill load <name>`).
    #[serde(default)]
    pub skill: String,

    /// Consuming hat ids. `"*"` expands to every hat in the preset.
    #[serde(default)]
    pub hats: Vec<String>,

    /// What the hat uses the skill for — required so reviewers can
    /// judge whether the dependency is justified.
    #[serde(default)]
    pub purpose: String,

    /// Whether the dependency is mandatory. Required deps that fail
    /// any check block loop startup; optional deps degrade to
    /// warnings and must document `missing_behavior`.
    #[serde(default = "default_true")]
    pub required: bool,

    /// How the skill reaches the hat (see [`SkillLoadTiming`]).
    #[serde(default)]
    pub load: SkillLoadTiming,

    /// Where the skill may resolve from (see [`SkillSourceConstraint`]).
    #[serde(default)]
    pub source: SkillSourceConstraint,

    /// Stage/timing note (e.g. "only before the final report").
    /// Free text — the machine-enforced timing is `load`.
    #[serde(default)]
    pub when: Option<String>,

    /// Installation or repair guidance surfaced verbatim in failure
    /// diagnostics when the dependency is missing.
    #[serde(default)]
    pub install_hint: Option<String>,

    /// Provenance of a project-adapted copy (upstream skill name,
    /// version, or URL) for traceability.
    #[serde(default)]
    pub adapted_from: Option<String>,

    /// Documented behaviour when an optional dependency is missing —
    /// required on `required: false` entries so "optional" never
    /// means "silently degraded".
    #[serde(default)]
    pub missing_behavior: Option<String>,

    /// External conditions (tools / env / files) the skill needs.
    #[serde(default)]
    pub requires: SkillRequirements,

    /// Unattended-fit declaration (see [`SkillUnattended`]).
    #[serde(default)]
    pub unattended: Option<SkillUnattended>,
}

/// Per-skill configuration override.
///
/// Allows enabling/disabling individual skills and overriding their
/// frontmatter fields (hats, backends, tags, auto_inject).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillOverride {
    /// Disable a discovered skill.
    #[serde(default)]
    pub enabled: Option<bool>,

    /// Restrict skill to specific hats.
    #[serde(default)]
    pub hats: Vec<String>,

    /// Restrict skill to specific backends.
    #[serde(default)]
    pub backends: Vec<String>,

    /// Tags for categorization.
    #[serde(default)]
    pub tags: Vec<String>,

    /// Inject full content into prompt (not just index entry).
    #[serde(default)]
    pub auto_inject: Option<bool>,
}
