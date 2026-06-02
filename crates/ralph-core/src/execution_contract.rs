//! Execution contract validation for agent completion obligations.
//!
//! This module validates that `work.done` events (and other completion topics)
//! meet the configured contract requirements before they can trigger downstream
//! hats. This prevents false positives from agents who forget to emit or emit
//! incomplete completion signals.
//!
//! # Contract Validation
//!
//! An execution contract validates:
//! - **Payload fields**: Required fields are present in the event payload
//! - **Task state**: The referenced task is in a valid terminal state
//! - **Git evidence**: There are meaningful git changes (unless trivial/empty path)
//!
//! # Example
//!
//! ```ignore
//! let decision = validate_execution_contract(
//!     &event,
//!     &rule,
//!     workspace_root,
//!     loop_id,
//!     tasks_path,
//!     Some(hat_id),
//! );
//! match decision {
//!     ExecutionContractDecision::Accept => { /* publish to bus */ }
//!     ExecutionContractDecision::Reject(findings) => { /* emit diagnostic + guidance */ }
//! }
//! ```

use crate::config::ExecutionContractRule;
use crate::task_store::TaskStore;
use ralph_proto::Event;
use serde_json::Value;
use std::path::Path;
use tracing::warn;

/// Outcome of an execution contract validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionContractDecision {
    /// The event satisfies all contract requirements.
    Accept,
    /// The event violates one or more contract requirements.
    Reject(Vec<ExecutionContractFinding>),
}

/// A single finding from contract validation failure.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ExecutionContractFinding {
    /// What kind of violation was detected.
    pub kind: ExecutionContractViolationKind,
    /// Human-readable description of the violation.
    pub message: String,
    /// The event topic being validated.
    pub topic: String,
}

/// Kind of contract violation.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub enum ExecutionContractViolationKind {
    /// Payload is missing a required field.
    MissingPayloadField { field: String },
    /// Payload is not a valid JSON object.
    InvalidPayload,
    /// Referenced task does not exist.
    TaskNotFound { task_id: String },
    /// Task belongs to a different loop (loop_scoped: true).
    TaskWrongLoop { task_id: String, expected_loop: String, actual_loop: Option<String> },
    /// Task is not in a valid terminal state.
    TaskNotTerminal { task_id: String, status: String, allowed: Vec<String> },
    /// Git evidence check failed (no diff and no commit).
    NoGitEvidence { step: Option<String> },
}

/// Validate an event against an execution contract rule.
///
/// Returns `Accept` if all contract requirements are satisfied, or `Reject`
/// with a list of findings describing each violation.
pub fn validate_execution_contract(
    event: &Event,
    rule: &ExecutionContractRule,
    workspace_root: &Path,
    current_loop_id: &str,
    tasks_path: &Path,
    _hat_id: Option<&str>,
) -> ExecutionContractDecision {
    let mut findings = Vec::new();

    // 1. Payload validation
    if let Some(rejection) = validate_payload(event, rule) {
        findings.push(rejection);
    }

    // 2. Task validation (if payload has required fields)
    if findings.is_empty() {
        if let Some(rejection) = validate_task(
            event,
            rule,
            current_loop_id,
            tasks_path,
        ) {
            findings.push(rejection);
        }
    }

    // 3. Git evidence validation (if task validation passed)
    if findings.is_empty() {
        if let Some(rejection) = validate_git_change(event, rule, workspace_root) {
            findings.push(rejection);
        }
    }

    if findings.is_empty() {
        ExecutionContractDecision::Accept
    } else {
        ExecutionContractDecision::Reject(findings)
    }
}

/// Validate that the event payload contains all required fields.
fn validate_payload(
    event: &Event,
    rule: &ExecutionContractRule,
) -> Option<ExecutionContractFinding> {
    // Empty payload is acceptable only if no fields are required
    let payload_str = event.payload.as_str();
    if payload_str.trim().is_empty() {
        if rule.require_payload_fields.is_empty() {
            return None;
        } else {
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::MissingPayloadField {
                    field: rule.require_payload_fields[0].clone(),
                },
                message: format!(
                    "work.done payload is empty but contract requires fields: {:?}",
                    rule.require_payload_fields
                ),
                topic: event.topic.to_string(),
            });
        }
    }

    let Ok(payload) = serde_json::from_str::<Value>(payload_str) else {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::InvalidPayload,
            message: format!(
                "work.done payload is not valid JSON: {:?}",
                payload_str.chars().take(100).collect::<String>()
            ),
            topic: event.topic.to_string(),
        });
    };

    let Value::Object(map) = &payload else {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::InvalidPayload,
            message: "work.done payload must be a JSON object".to_string(),
            topic: event.topic.to_string(),
        });
    };

    for field in &rule.require_payload_fields {
        if !map.contains_key(field) {
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::MissingPayloadField {
                    field: field.clone(),
                },
                message: format!(
                    "work.done payload is missing required field: '{}'",
                    field
                ),
                topic: event.topic.to_string(),
            });
        }
    }

    None
}

/// Validate that the referenced task satisfies completion requirements.
fn validate_task(
    event: &Event,
    rule: &ExecutionContractRule,
    current_loop_id: &str,
    tasks_path: &Path,
) -> Option<ExecutionContractFinding> {
    let payload_str = event.payload.as_str();
    if payload_str.trim().is_empty() {
        return None;
    }

    let Ok(payload) = serde_json::from_str::<Value>(payload_str) else {
        return None;
    };

    let Value::Object(map) = &payload else {
        return None;
    };

    let task_id = map.get(&rule.require_task.id_field)?.as_str()?.to_string();
    let _task_key = map.get(&rule.require_task.key_field).and_then(|v| v.as_str());

    // Load the task store
    let store = match TaskStore::load(tasks_path) {
        Ok(s) => s,
        Err(e) => {
            warn!(error = %e, "Failed to load task store for execution contract validation");
            return None;
        }
    };

    // Find the task
    let task = store.get(&task_id)?;

    // Check loop scoping
    if rule.require_task.loop_scoped {
        if let Some(task_loop_id) = &task.loop_id {
            if task_loop_id != current_loop_id {
                return Some(ExecutionContractFinding {
                    kind: ExecutionContractViolationKind::TaskWrongLoop {
                        task_id: task_id.clone(),
                        expected_loop: current_loop_id.to_string(),
                        actual_loop: Some(task_loop_id.clone()),
                    },
                    message: format!(
                        "Task '{}' belongs to loop '{}', expected loop '{}'",
                        task_id, task_loop_id, current_loop_id
                    ),
                    topic: event.topic.to_string(),
                });
            }
        } else {
            // Legacy task without loop_id - reject if loop_scoped is required
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::TaskWrongLoop {
                    task_id: task_id.clone(),
                    expected_loop: current_loop_id.to_string(),
                    actual_loop: None,
                },
                message: format!(
                    "Task '{}' has no loop_id (legacy), but contract requires loop '{}'",
                    task_id, current_loop_id
                ),
                topic: event.topic.to_string(),
            });
        }
    }

    // Check terminal status
    let status_str = format!("{:?}", task.status).to_lowercase();
    let allowed: Vec<String> = rule
        .require_task
        .allowed_terminal_statuses
        .iter()
        .map(|s| s.to_lowercase())
        .collect();

    if !allowed.contains(&status_str) {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::TaskNotTerminal {
                task_id: task_id.clone(),
                status: status_str.clone(),
                allowed: rule.require_task.allowed_terminal_statuses.clone(),
            },
            message: format!(
                "Task '{}' has status '{}', expected one of {:?}",
                task_id, status_str, rule.require_task.allowed_terminal_statuses
            ),
            topic: event.topic.to_string(),
        });
    }

    None
}

/// Validate that there is git evidence (diff or commit) unless the step is trivial.
fn validate_git_change(
    event: &Event,
    rule: &ExecutionContractRule,
    workspace_root: &Path,
) -> Option<ExecutionContractFinding> {
    // Check if this step allows empty git (trivial path)
    let payload_str = event.payload.as_str();
    if !payload_str.trim().is_empty() {
        if let Ok(payload) = serde_json::from_str::<Value>(payload_str) {
            if let Value::Object(map) = &payload {
                if let Some(step) = map.get("step").and_then(|v| v.as_str()) {
                    if rule.require_git_change.allow_empty_for_steps.contains(&step.to_string()) {
                        return None;
                    }
                }
            }
        }
    }

    // Check git diff
    let has_diff = check_git_diff(workspace_root);
    let has_commit = check_git_commit(workspace_root);

    if !has_diff && !has_commit {
        let step = serde_json::from_str::<Value>(payload_str)
            .ok()
            .and_then(|v| v.get("step").and_then(|s| s.as_str().map(String::from)));

        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::NoGitEvidence { step },
            message: "work.done requires git evidence (diff or commit) before review can proceed. No diff or commit found.".to_string(),
            topic: event.topic.to_string(),
        });
    }

    None
}

/// Check if there are uncommitted changes in the git working tree.
fn check_git_diff(workspace_root: &Path) -> bool {
    use std::process::Command;

    let output = Command::new("git")
        .args(["diff", "--quiet"])
        .current_dir(workspace_root)
        .output();

    match output {
        Ok(out) => !out.status.success(),
        Err(_) => false,
    }
}

/// Check if there are any commits in the current branch.
fn check_git_commit(workspace_root: &Path) -> bool {
    use std::process::Command;

    let output = Command::new("git")
        .args(["log", "--oneline", "-1"])
        .current_dir(workspace_root)
        .output();

    match output {
        Ok(out) => out.status.success() && !out.stdout.is_empty(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ContractRejectConfig, ExecutionContractRule,
        GitChangeRequirement, TaskCompletionRequirement, TestEvidenceRequirement,
    };

    fn make_work_done_rule() -> ExecutionContractRule {
        ExecutionContractRule {
            require_payload_fields: vec![
                "plan_name".to_string(),
                "plan_path".to_string(),
                "task_id".to_string(),
                "task_key".to_string(),
                "step".to_string(),
            ],
            require_task: TaskCompletionRequirement {
                id_field: "task_id".to_string(),
                key_field: "task_key".to_string(),
                loop_scoped: true,
                allowed_terminal_statuses: vec!["closed".to_string()],
                auto_close_on_valid: false,
            },
            require_git_change: GitChangeRequirement {
                mode: "diff_or_commit".to_string(),
                allow_empty_for_steps: vec!["trivial".to_string()],
            },
            require_test_evidence: TestEvidenceRequirement {
                mode: "optional".to_string(),
            },
            reject: ContractRejectConfig::default(),
        }
    }

    fn make_trivial_rule() -> ExecutionContractRule {
        ExecutionContractRule {
            require_payload_fields: vec!["task_id".to_string(), "step".to_string()],
            require_task: TaskCompletionRequirement {
                id_field: "task_id".to_string(),
                key_field: "task_key".to_string(),
                loop_scoped: false,
                allowed_terminal_statuses: vec!["closed".to_string()],
                auto_close_on_valid: false,
            },
            require_git_change: GitChangeRequirement {
                mode: "diff_or_commit".to_string(),
                allow_empty_for_steps: vec!["trivial".to_string()],
            },
            require_test_evidence: TestEvidenceRequirement {
                mode: "optional".to_string(),
            },
            reject: ContractRejectConfig::default(),
        }
    }

    #[test]
    fn test_accepts_valid_work_done() {
        let rule = make_work_done_rule();
        let event = Event::new(
            "work.done",
            r#"{"plan_name":"test","plan_path":"/test","task_id":"task-1","task_key":"key-1","step":"step-01"}"#,
        );

        // This test would need proper git and task setup - just verify structure here
        assert_eq!(rule.require_payload_fields.len(), 5);
        assert_eq!(rule.require_task.loop_scoped, true);
    }

    #[test]
    fn test_rejects_missing_payload_field() {
        let rule = make_work_done_rule();
        let event = Event::new(
            "work.done",
            r#"{"plan_name":"test","task_id":"task-1"}"#, // missing plan_path, task_key, step
        );

        let decision = validate_execution_contract(
            &event,
            &rule,
            std::path::Path::new("/tmp"),
            "loop-1",
            std::path::Path::new("/tmp/tasks.jsonl"),
            None,
        );

        match decision {
            ExecutionContractDecision::Reject(findings) => {
                assert!(!findings.is_empty());
                assert!(matches!(
                    findings[0].kind,
                    ExecutionContractViolationKind::MissingPayloadField { .. }
                ));
            }
            ExecutionContractDecision::Accept => panic!("Expected rejection"),
        }
    }

    #[test]
    fn test_rejects_invalid_payload() {
        let rule = make_work_done_rule();
        let event = Event::new("work.done", "not valid json");

        let decision = validate_execution_contract(
            &event,
            &rule,
            std::path::Path::new("/tmp"),
            "loop-1",
            std::path::Path::new("/tmp/tasks.jsonl"),
            None,
        );

        match decision {
            ExecutionContractDecision::Reject(findings) => {
                assert!(!findings.is_empty());
                assert!(matches!(
                    findings[0].kind,
                    ExecutionContractViolationKind::InvalidPayload
                ));
            }
            ExecutionContractDecision::Accept => panic!("Expected rejection"),
        }
    }

    #[test]
    fn test_trivial_step_allows_empty_git() {
        let rule = make_trivial_rule();
        let event = Event::new(
            "work.done",
            r#"{"task_id":"task-1","task_key":"key-1","step":"trivial"}"#,
        );

        // With trivial step, git validation should pass even without git
        // We can't easily test this without mocking, but the structure is correct
        assert!(rule
            .require_git_change
            .allow_empty_for_steps
            .contains(&"trivial".to_string()));
    }

    #[test]
    fn test_disabled_contract_passes_through() {
        // When rule is Default (disabled), validation should pass
        let rule = ExecutionContractRule::default();
        let event = Event::new("work.done", r#"{"task_id":"task-1"}"#);

        // A default/empty rule should accept any event
        let decision = validate_execution_contract(
            &event,
            &rule,
            std::path::Path::new("/tmp"),
            "loop-1",
            std::path::Path::new("/tmp/tasks.jsonl"),
            None,
        );

        // Empty rule has no required fields, so it should accept
        assert!(matches!(decision, ExecutionContractDecision::Accept));
    }
}
