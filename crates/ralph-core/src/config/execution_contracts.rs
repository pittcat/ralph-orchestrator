//! Execution contract configuration for validating agent completion obligations.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Execution contract configuration for validating agent completion obligations.
///
/// Each rule in `rules` maps an event topic (e.g. "work.done") to its validation
/// requirements. Rules are only applied when the matching topic is published.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExecutionContractsConfig {
    /// When true, execution contracts are enforced. When false (default), contracts
    /// are parsed but not applied, preserving backward compatibility.
    #[serde(default)]
    pub enabled: bool,

    /// Topic-level contract rules. Key is the event topic (e.g. "work.done").
    #[serde(default)]
    pub rules: HashMap<String, ExecutionContractRule>,
}

/// A single execution contract rule for one topic.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ExecutionContractRule {
    /// Fields that must be present in the event payload.
    #[serde(default)]
    pub require_payload_fields: Vec<String>,

    /// Task completion requirements.
    #[serde(default)]
    pub require_task: TaskCompletionRequirement,

    /// Git change requirements.
    #[serde(default)]
    pub require_git_change: GitChangeRequirement,

    /// Test evidence requirements.
    #[serde(default)]
    pub require_test_evidence: TestEvidenceRequirement,

    /// What to publish when the contract is rejected.
    #[serde(default)]
    pub reject: ContractRejectConfig,
}

/// Task completion requirement for execution contract validation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TaskCompletionRequirement {
    /// JSON field name containing the task ID.
    #[serde(default = "default_task_id_field")]
    pub id_field: String,

    /// JSON field name containing the task key.
    #[serde(default = "default_task_key_field")]
    pub key_field: String,

    /// When true, the task must belong to the current loop.
    #[serde(default = "default_loop_scoped")]
    pub loop_scoped: bool,

    /// Terminal task statuses that satisfy the contract.
    #[serde(default = "default_allowed_terminal_statuses")]
    pub allowed_terminal_statuses: Vec<String>,

    /// When true, automatically close the task if contract is satisfied.
    #[serde(default)]
    pub auto_close_on_valid: bool,
}

impl Default for TaskCompletionRequirement {
    fn default() -> Self {
        Self {
            id_field: default_task_id_field(),
            key_field: default_task_key_field(),
            loop_scoped: default_loop_scoped(),
            allowed_terminal_statuses: default_allowed_terminal_statuses(),
            auto_close_on_valid: false,
        }
    }
}

fn default_task_id_field() -> String {
    "task_id".to_string()
}

fn default_task_key_field() -> String {
    "task_key".to_string()
}

fn default_loop_scoped() -> bool {
    true
}

fn default_allowed_terminal_statuses() -> Vec<String> {
    vec!["closed".to_string()]
}

/// Git change requirement for execution contract validation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitChangeRequirement {
    /// Mode of git evidence acceptance.
    #[serde(default = "default_git_change_mode")]
    pub mode: String,

    /// Steps that are allowed to have empty diff or commit.
    #[serde(default)]
    pub allow_empty_for_steps: Vec<String>,
}

impl Default for GitChangeRequirement {
    fn default() -> Self {
        Self {
            mode: default_git_change_mode(),
            allow_empty_for_steps: Vec::new(),
        }
    }
}

fn default_git_change_mode() -> String {
    "diff_or_commit".to_string()
}

/// Test evidence requirement for execution contract validation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct TestEvidenceRequirement {
    /// Mode: "optional" or "required_payload_field".
    #[serde(default = "default_test_evidence_mode")]
    pub mode: String,

    /// Payload field name to check for test evidence (used when mode is "required_payload_field").
    /// Common values: "tests", "test_results", "test_output".
    #[serde(default)]
    pub payload_field: Option<String>,
}

impl Default for TestEvidenceRequirement {
    fn default() -> Self {
        Self {
            mode: default_test_evidence_mode(),
            payload_field: None,
        }
    }
}

fn default_test_evidence_mode() -> String {
    "optional".to_string()
}

/// What to publish when an execution contract is rejected.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContractRejectConfig {
    /// Topic for the structured rejection diagnostic event.
    #[serde(default = "default_reject_diagnostic_topic")]
    pub diagnostic_topic: String,

    /// Topic for the human-readable guidance event.
    ///
    /// Plan 2026-06-28-005 changed the default from `human.guidance`
    /// (the deleted operator channel) to `plan.blocked`, which is
    /// the existing structured terminal-orchestrator topic. Operators
    /// can still override this field, but the value MUST be a
    /// terminal orchestrator topic (`plan.blocked`, `loop.cancel`,
    /// `LOOP_COMPLETE`); setting it to a non-terminal topic such as
    /// `task.resume` or the removed `human.guidance` will cause the
    /// engine to ignore the override and emit a warning at runtime.
    #[serde(default = "default_reject_guidance_topic")]
    pub guidance_topic: String,
}

impl Default for ContractRejectConfig {
    fn default() -> Self {
        Self {
            diagnostic_topic: default_reject_diagnostic_topic(),
            guidance_topic: default_reject_guidance_topic(),
        }
    }
}

fn default_reject_diagnostic_topic() -> String {
    "event.execution_contract.rejected".to_string()
}

fn default_reject_guidance_topic() -> String {
    "plan.blocked".to_string()
}
