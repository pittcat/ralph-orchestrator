//! 2026-07-02-006 plan U1: `PhaseAuthorityConfig` pure serde type.
//!
//! Round-trip pins for `PhaseAuthorityConfig` (a member of
//! `MechanismConfig`). The struct lives in
//! `event_loop::phase_authority::config` and is re-exported from
//! `config::loop_config` so the runtime can read the typed view of
//! `mechanism.phase_authority` in `presets/en/<name>.yml`.
//!
//! Pure data, no I/O, no lint, no preset wiring. U2 onward will
//! build the in-memory `PhaseAuthorityDeclaration` from this struct.

use serde::{Deserialize, Serialize};

/// Top-level config block inside `mechanism.phase_authority`.
///
/// Round-trip test fixtures (U1) live in `tests.rs`. The field set
/// must remain compatible with `KTD1` in
/// `docs/plans/2026-07-02-006-feat-ce-executor-serial-runtime-phase-authority-plan.md`:
///
/// - `enabled` master switch (R1: opt-in only)
/// - `initial_phase` (KTD9 / R3)
/// - `phases` (R2 / KTD1)
/// - `transitions` (KTD1)
/// - `violation_policy` (KTD6 resume budget)
/// - `progress_projection` (U19)
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PhaseAuthorityConfig {
    /// Master switch. When `false`, `WorkflowPhaseAuthority` is a
    /// no-op and `PhaseAuthorityStage` short-circuits.
    #[serde(default)]
    pub enabled: bool,

    /// Phase id the workflow starts in once the loop accepts the
    /// first business event. Must reference one of `phases[*].id`
    /// when validation runs in U2.
    #[serde(default)]
    pub initial_phase: Option<String>,

    /// Declarative phase table. Each phase carries its own
    /// per-hat / per-role topic whitelist (R2).
    #[serde(default)]
    pub phases: Vec<PhaseDeclConfig>,

    /// Declarative transition table (R3). Order is not
    /// significant; the engine matches by `(from, on)`.
    #[serde(default)]
    pub transitions: Vec<PhaseTransitionConfig>,

    /// Resume budget / exhaustion policy (KTD6). Defaults are
    /// applied when the field is absent (see `tests.rs`).
    #[serde(default)]
    pub violation_policy: ViolationPolicyConfig,

    /// Per-phase `progress.md` projection hooks (U19). Pure
    /// markdown-string builder; no I/O happens here.
    #[serde(default)]
    pub progress_projection: ProgressProjectionConfig,
}

/// One phase entry inside `mechanism.phase_authority.phases`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PhaseDeclConfig {
    /// Stable phase id, e.g. `unit_loop`. Referenced by
    /// `transitions[*].from`, `transitions[*].to`, and the
    /// engine's `current_phase_id`.
    pub id: String,

    /// Optional human-readable label. Used only by diagnostics
    /// and the `ralph diagnose` report — never evaluated.
    #[serde(default)]
    pub label: Option<String>,

    /// Per-role topic whitelist. Key = role/hat id (string),
    /// value = topics the role is allowed to emit while this
    /// phase is active. Engine semantics (R2) are evaluated in
    /// U4; U1 only pins the shape.
    #[serde(default)]
    pub allowed_emits: std::collections::BTreeMap<String, Vec<String>>,
}

/// One transition entry inside `mechanism.phase_authority.transitions`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct PhaseTransitionConfig {
    /// Source phase. `*` is reserved for "any phase" but is not
    /// yet interpreted in U1 (U2 validates references).
    pub from: String,

    /// Target phase when the `on` trigger fires.
    pub to: String,

    /// Trigger description. One of:
    /// - `{ event: "<topic>" }` — fires when an accepted event
    ///   carries that topic.
    /// - `{ primitive: "<name>", ...args }` — fires when the
    ///   named primitive evaluates to a target phase.
    /// U2 will validate the discriminant exhaustively.
    #[serde(default)]
    pub on: TransitionOnConfig,
}

/// Free-form transition trigger payload. Preserves the original
/// `serde_yaml::Value` so we don't lose specificity while U1 is
/// only about the shape. U2 narrows it.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(transparent)]
pub struct TransitionOnConfig(pub serde_yaml::Value);

/// `violation_policy` block — U1 only pins the shape; U22
/// (`phase_violation_resume_budget`) interprets it.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ViolationPolicyConfig {
    /// Max `task.resume(reason_code=phase_violation)` injections
    /// per `(hat, violation_kind)` tuple. Default: 3 (KTD6).
    #[serde(default = "default_max_resume_per_hat")]
    pub max_resume_per_hat: u32,

    /// Behaviour after the budget is exhausted. Default:
    /// `plan_blocked` (KTD6 step 4).
    #[serde(default = "default_on_exhausted")]
    pub on_exhausted: String,
}

impl Default for ViolationPolicyConfig {
    fn default() -> Self {
        Self {
            max_resume_per_hat: default_max_resume_per_hat(),
            on_exhausted: default_on_exhausted(),
        }
    }
}

fn default_max_resume_per_hat() -> u32 {
    3
}

fn default_on_exhausted() -> String {
    "plan_blocked".to_string()
}

/// `progress_projection` block — U1 only pins the shape; U19
/// interprets it.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
pub struct ProgressProjectionConfig {
    /// Per-phase `progress.md` hooks. Key = phase id, value =
    /// free-form settings consumed by
    /// `apply_progress_on_phase_enter` in U19.
    #[serde(default)]
    pub on_enter: std::collections::BTreeMap<String, serde_yaml::Value>,
}