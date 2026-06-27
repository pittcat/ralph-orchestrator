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
}

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

        if decl.flow_type != "declared" {
            return Err(FlowParseError::UnsupportedFlowType(decl.flow_type));
        }
        if decl.enforce_schema != "hard" {
            return Err(FlowParseError::UnsupportedEnforceSchema(
                decl.enforce_schema,
            ));
        }
        if decl.state_idempotency != "required" {
            return Err(FlowParseError::UnsupportedStateIdempotency(
                decl.state_idempotency,
            ));
        }

        // Detect duplicate step ids early so a malformed
        // preset fails fast at load time rather than at
        // runtime stage check.
        let mut seen = std::collections::HashSet::new();
        for step in &decl.steps {
            if !seen.insert(step.id.clone()) {
                return Err(FlowParseError::DuplicateStepId(step.id.clone()));
            }
        }

        Ok(decl)
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
}

#[cfg(test)]
mod tests;