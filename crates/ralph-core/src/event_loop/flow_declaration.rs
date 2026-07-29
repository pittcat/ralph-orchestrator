//! `mechanism.flow` YAML declaration (U5).
//!
//! Why this exists: prior to the mechanism foundation, presets
//! described workflow as free-form `body:` lists inside
//! `event_policy` and runtime accepted whatever the hat
//! happened to publish. That allowed the 2026-06-26 incident
//! to slip a 4/8 partial-completion through `unit_loop`
//! without ever emitting `review.start` or `plan.blocked`.
//!
//! U5 introduces an explicit flow declaration — a typed view
//! of `mechanism.flow` in `presets/en/<name>.yml`. The lint
//! rules in `preset_lint::flow_declaration` reject presets
//! that have an incomplete or undeclared flow, and the
//! `FlowStepScopeStage` (U9) uses the same declaration at
//! runtime to reject emit-time out-of-step publishes.
//!
//! Cross-platform / concurrency semantics: pure data. Parsing
//! happens once at preset-load time. The same `FlowDeclaration`
//! value is then shared (immutably) between the lint pass and
//! the event-loop stages.
//!
//! # Example
//!
//! ```no_run
//! use ralph_core::event_loop::flow_declaration::FlowDeclaration;
//!
//! let yaml = r#"
//! mechanism:
//!   flow:
//!     type: declared
//!     version: 1
//!     terminal_emits: [LOOP_COMPLETE]
//!     steps:
//!       - id: unit_loop
//!         kind: foreach
//!         allowed_emits: [work.ready, work.done]
//!         terminal_when: all_done
//! "#;
//! let decl = FlowDeclaration::from_yaml(yaml).unwrap();
//! assert_eq!(decl.terminal_emits, vec!["LOOP_COMPLETE".to_string()]);
//! ```

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// One step in a `mechanism.flow` declaration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlowStepDecl {
    /// Stable id, e.g. `unit_loop`. Referenced from
    /// `emit_when` and from `StageContext.current_step.id`.
    pub id: String,
    /// `foreach` / `sequence` / `branch`. The flow runtime
    /// does not yet interpret body semantics — the value is
    /// stored verbatim and used for diagnostics.
    #[serde(default)]
    pub kind: Option<String>,
    /// Topics the step permits. Anything else is rejected by
    /// `FlowStepScopeStage`.
    #[serde(default)]
    pub allowed_emits: Vec<String>,
    /// Optional terminal condition; one of `all_done`,
    /// `any_failed`, `partial_units_done`, `all_units_done`,
    /// or unset for non-terminal steps.
    #[serde(default)]
    pub terminal_when: Option<String>,
    /// Partial-state branches. Required if `terminal_when`
    /// is in `{all_done, any_failed, partial_units_done}`
    /// (see `is_partial_state`).
    #[serde(default)]
    pub on_partial: std::collections::BTreeMap<String, String>,
    /// U12 (2026-07-24-003 plan U1, was: 2026-06-27-002 plan
    /// completion) — total number of units the step is expected
    /// to drive (e.g. 8 in the 4/8 partial scenario). When
    /// set, `StepCloseObligationStage` becomes live: the
    /// runtime drives `update_progress(step_id, done, total)`
    /// from `work.done` emit counts, and the stage rejects
    /// emits that don't satisfy `on_partial` while `done <
    /// total`. Backwards compatible: omitted = stage stays
    /// fail-open (the pre-U12 behaviour).
    #[serde(default)]
    pub total_units: Option<u32>,
    /// Optional runner binding. For `kind: side_effect` steps
    /// this names the runtime runner that owns the side-effect
    /// emit (e.g. `supervisor.review.wave`, `wave.runtime.review`).
    /// `supervisor.*` bindings imply
    /// `event_loop.supervisor.enabled: true` is required;
    /// `wave.runtime.*` bindings work without supervisor — the
    /// runtime's default wave hot path injects the
    /// corresponding `*.wave.complete` / `*.wave.failed`
    /// coordination topics.
    ///
    /// The lint graph (`preset_lint::workflow_activation`
    /// R5 `RUNNER_INJECTED_TRIGGERS` and
    /// `runtime_contract::detect_required_topic_gaps`) use
    /// this to exempt `*.wave.{complete,failed}` from the
    /// "no publisher" archetype when a wave runner binding is
    /// declared but `event_loop.supervisor.enabled` is
    /// false (the implementation-review preset path).
    #[serde(default)]
    pub runs: Option<String>,
    /// 2026-07-26-004 plan U6 (R7 / R8): single accepted topic that
    /// transitions INTO this step (`None` for the initial step / legacy
    /// linear flows). YAML key is the quoted `"on"`.
    #[serde(default, rename = "on")]
    pub on: Option<String>,
    /// 2026-07-26-004 plan U6 (R8): branching entry — any of these
    /// accepted topics transitions into this step (takes precedence over
    /// positional advance).
    #[serde(default)]
    pub on_any_of: Vec<String>,
    /// 2026-07-29-001 plan U1: explicit subset of
    /// `allowed_emits` whose acceptance advances the
    /// plan-mode current step. Empty (the default) keeps
    /// the legacy contract — every topic in
    /// `allowed_emits` is transition-capable — so
    /// presets that have not opted in keep their
    /// existing semantics.
    #[serde(default)]
    pub transition_emits: Vec<String>,
}

/// Step runner-binding namespaces recognised by the runtime.
///
/// New runner bindings must land here AND extend the
/// `is_wave_runner_binding` / `is_supervisor_runner_binding`
/// classification helpers so the lint graph remains the
/// authoritative source of truth for capability-triggered
/// exemptions.
pub const RUNNER_BINDING_WAVE_PREFIX: &str = "wave.runtime.";
pub const RUNNER_BINDING_SUPERVISOR_PREFIX: &str = "supervisor.";

/// Top-level flow declaration.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FlowDeclaration {
    /// Always `declared` in this version. Reserved so a
    /// future `inferred` mode can co-exist without a
    /// breaking schema change.
    #[serde(rename = "type", default = "default_declared_type")]
    pub flow_type: String,
    #[serde(default = "default_version")]
    pub version: u32,
    #[serde(default)]
    pub terminal_emits: Vec<String>,
    #[serde(default)]
    pub steps: Vec<FlowStepDecl>,
    /// Default `repair_budget` for the repair stream. U7
    /// reads this when constructing the per-loop
    /// `RepairStateMachine`.
    #[serde(default = "default_repair_budget")]
    pub repair_budget: u32,
    /// `hard` means the emit-time schema gate rejects
    /// missing-field events. `soft` is a future option and
    /// is rejected at preset-load time.
    #[serde(default = "default_enforce_schema")]
    pub enforce_schema: String,
    /// `required` means state files must carry
    /// `_idempotency_key`. Anything else is a lint error.
    #[serde(default = "default_state_idempotency")]
    pub state_idempotency: String,
}

fn default_declared_type() -> String {
    "declared".to_string()
}
fn default_version() -> u32 {
    1
}
fn default_repair_budget() -> u32 {
    3
}
fn default_enforce_schema() -> String {
    "hard".to_string()
}
fn default_state_idempotency() -> String {
    "required".to_string()
}

/// Classifier helpers used by the preset_lint and
/// runtime_contract layers to recognise which runner a
/// `kind: side_effect` step delegates to.
///
/// `wave.runtime.*` bindings are exempt from the supervisor
/// enablement guard because the default wave hot path
/// (`wave_detection.rs` + `crate::wave::SharedReadonlySlots`)
/// produces `*.wave.complete` / `*.wave.failed` even when
/// `event_loop.supervisor.enabled` is false.
pub fn is_wave_runner_binding(runs: Option<&str>) -> bool {
    runs.map(|r| r.starts_with(RUNNER_BINDING_WAVE_PREFIX))
        .unwrap_or(false)
}

pub fn is_supervisor_runner_binding(runs: Option<&str>) -> bool {
    runs.map(|r| r.starts_with(RUNNER_BINDING_SUPERVISOR_PREFIX))
        .unwrap_or(false)
}

#[derive(Debug, Error)]
pub enum FlowParseError {
    #[error("YAML parse error: {0}")]
    Yaml(#[from] serde_yaml::Error),
    #[error("flow declaration is missing from `mechanism:` block")]
    MissingMechanismFlow,
    #[error("`mechanism.flow.type` must be `declared`; got `{0}`")]
    UnsupportedFlowType(String),
    #[error("`enforce_schema` must be `hard`; got `{0}`")]
    UnsupportedEnforceSchema(String),
    #[error("`state_idempotency` must be `required`; got `{0}`")]
    UnsupportedStateIdempotency(String),
    #[error("duplicate step id `{0}`")]
    DuplicateStepId(String),
    #[error("step `{0}` references itself via emit_when")]
    SelfReferentialStep(String),
}

/// A `terminal_when` value that requires an `on_partial` map.
///
/// `all_units_done` is intentionally NOT in this set — it is
/// the "all units completed successfully" case, which is not
/// a partial state and does not need an `on_partial` branch.
pub fn is_partial_state(terminal_when: &str) -> bool {
    matches!(
        terminal_when,
        "all_done" | "any_failed" | "partial_units_done"
    )
}

impl FlowDeclaration {
    /// Convert the typed config view (`FlowDeclarationConfig`)
    /// into the runtime `FlowDeclaration` without a YAML
    /// round-trip.
    ///
    /// The previous `serde_yaml::to_string` +
    /// `format!("mechanism:\\n  flow:\\n{flow}")` path left the
    /// serialized body unindented, so `mechanism.flow` parsed
    /// as null and `FlowStepScopeStage` rejected every business
    /// emit with `flow_step_undeclared` (supervisor primary-path
    /// E2E: `work.ready` never reached `task-planner`).
    pub fn from_config(cfg: &crate::config::FlowDeclarationConfig) -> Result<Self, FlowParseError> {
        let decl = FlowDeclaration {
            flow_type: cfg.flow_type.clone(),
            version: cfg.version,
            terminal_emits: cfg.terminal_emits.clone(),
            steps: cfg
                .steps
                .iter()
                .map(|s| FlowStepDecl {
                    id: s.id.clone(),
                    kind: s.kind.clone(),
                    allowed_emits: s.allowed_emits.clone(),
                    terminal_when: s.terminal_when.clone(),
                    on_partial: s.on_partial.clone(),
                    total_units: None,
                    transition_emits: s.transition_emits.clone(),
                    runs: s.runs.clone(),
                    on: s.on.clone(),
                    on_any_of: s.on_any_of.clone(),
                })
                .collect(),
            repair_budget: cfg.repair_budget,
            enforce_schema: cfg.enforce_schema.clone(),
            state_idempotency: cfg.state_idempotency.clone(),
        };
        decl.validate()
    }

    /// Parse a YAML document that contains a `mechanism:` top
    /// level key. Returns the inner flow declaration.
    pub fn from_yaml(yaml: &str) -> Result<Self, FlowParseError> {
        // The preset YAML nests flow under `mechanism:`, so
        // we parse the full document and pull out the flow
        // map. Accepting the wrapper keeps the lint and
        // runtime decoupled from how presets nest their
        // metadata.
        let value: serde_yaml::Value = serde_yaml::from_str(yaml)?;
        let flow_value = value
            .get("mechanism")
            .and_then(|m| m.get("flow"))
            .ok_or(FlowParseError::MissingMechanismFlow)?;
        let decl: FlowDeclaration = serde_yaml::from_value(flow_value.clone())?;
        decl.validate()
    }

    /// Shared post-parse / post-config invariants.
    fn validate(self) -> Result<Self, FlowParseError> {
        if self.flow_type != "declared" {
            return Err(FlowParseError::UnsupportedFlowType(self.flow_type));
        }
        if self.enforce_schema != "hard" {
            return Err(FlowParseError::UnsupportedEnforceSchema(
                self.enforce_schema,
            ));
        }
        if self.state_idempotency != "required" {
            return Err(FlowParseError::UnsupportedStateIdempotency(
                self.state_idempotency,
            ));
        }

        // Detect duplicate step ids early so a malformed
        // preset fails fast at load time rather than at
        // runtime stage check.
        let mut seen = std::collections::HashSet::new();
        for step in &self.steps {
            if !seen.insert(step.id.clone()) {
                return Err(FlowParseError::DuplicateStepId(step.id.clone()));
            }
        }

        Ok(self)
    }

    /// Return the step with the given id, if present.
    pub fn step(&self, id: &str) -> Option<&FlowStepDecl> {
        self.steps.iter().find(|s| s.id == id)
    }

    /// True if `topic` is a permitted emit for `step_id`.
    pub fn allows(&self, step_id: &str, topic: &str) -> bool {
        self.step(step_id)
            .map(|s| s.allowed_emits.iter().any(|t| t == topic))
            .unwrap_or(false)
    }

    /// 2026-07-24-003 plan U1 / capability-gap fix: returns
    /// `true` when at least one step declares a
    /// `wave.runtime.*` runner binding. Used by the lint
    /// graph (`preset_lint::workflow_activation` R5
    /// `RUNNER_INJECTED_TRIGGERS` exemption + the topology
    /// `detect_required_topic_gaps` exemption in
    /// `runtime_contract`) to recognise that the
    /// `*.wave.complete` / `*.wave.failed` coordination
    /// topics are runtime-injected by the default wave hot
    /// path even when `event_loop.supervisor.enabled` is
    /// false.
    ///
    /// The check is capability-triggered (not preset-name
    /// pinned): any preset that declares a `wave.runtime.*`
    /// runner binding qualifies, mirroring the supervisor
    /// exemption in `runtime_contract.rs:844`.
    pub fn uses_wave_runtime(&self) -> bool {
        self.steps
            .iter()
            .any(|s| is_wave_runner_binding(s.runs.as_deref()))
    }
}

/// 2026-07-24-003 plan U1 / capability-gap fix: top-level
/// predicate for downstream callers (preset_validator's
/// `build_topology_graph`, BFS reachability, etc.) that need
/// the same "does this preset declare a `wave.runtime.*` step?"
/// answer without depending on the typed view's struct. The
/// typed view's `uses_wave_runtime` is the authoritative
/// source of truth; this wrapper exposes it to consumers that
/// take `&RalphConfig` (not `&FlowDeclaration`).
pub fn is_wave_runner_binding_preset(config: &crate::config::RalphConfig) -> bool {
    config
        .mechanism
        .as_ref()
        .and_then(|m| m.flow.as_ref())
        .map(|f| f.uses_wave_runtime())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests;
