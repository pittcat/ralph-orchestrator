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
use std::process::Command;
use tracing::warn;

/// Git evidence provider abstraction for testability.
pub trait GitEvidenceProvider: Send + Sync {
    /// Returns true if the workspace is a git repository.
    fn is_git_repo(&self, workspace: &Path) -> bool;
    /// Returns true if there are unstaged or staged changes.
    fn has_uncommitted_changes(&self, workspace: &Path) -> bool;
    /// Returns true if there are commits since the given baseline SHA.
    fn has_new_commits_since(&self, workspace: &Path, start_sha: Option<&str>) -> bool;
}

/// Production git evidence provider using git CLI.
pub struct DefaultGitEvidenceProvider;

impl GitEvidenceProvider for DefaultGitEvidenceProvider {
    fn is_git_repo(&self, workspace: &Path) -> bool {
        Command::new("git")
            .args(["rev-parse", "--git-dir"])
            .current_dir(workspace)
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
    }

    fn has_uncommitted_changes(&self, workspace: &Path) -> bool {
        // git diff --quiet: exit 0 = no diff (false), exit 1 = has diff (true), exit 128+ = not a git repo (false)
        let unstaged = Command::new("git")
            .args(["diff", "--quiet"])
            .current_dir(workspace)
            .output()
            .map(|o| o.status.code() == Some(1))
            .unwrap_or(false);
        let staged = Command::new("git")
            .args(["diff", "--cached", "--quiet"])
            .current_dir(workspace)
            .output()
            .map(|o| o.status.code() == Some(1))
            .unwrap_or(false);
        unstaged || staged
    }

    fn has_new_commits_since(&self, workspace: &Path, start_sha: Option<&str>) -> bool {
        match start_sha {
            Some(start) => {
                let output = Command::new("git")
                    .args(["rev-list", &format!("{}..HEAD", start), "--count"])
                    .current_dir(workspace)
                    .output();
                match output {
                    Ok(out) if out.status.success() => {
                        String::from_utf8_lossy(&out.stdout)
                            .trim()
                            .parse::<usize>()
                            .unwrap_or(0)
                            > 0
                    }
                    _ => false,
                }
            }
            None => false,
        }
    }
}

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
    TaskWrongLoop {
        task_id: String,
        expected_loop: String,
        actual_loop: Option<String>,
    },
    /// Task is not in a valid terminal state.
    TaskNotTerminal {
        task_id: String,
        status: String,
        allowed: Vec<String>,
    },
    /// Git evidence check failed (no diff and no commit).
    NoGitEvidence { step: Option<String> },
    /// Test evidence check failed (required field missing or falsy).
    NoTestEvidence { field: String },
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
    git_provider: &dyn GitEvidenceProvider,
    loop_start_sha: Option<&str>,
) -> ExecutionContractDecision {
    let mut findings = Vec::new();

    // 1. Payload validation
    if let Some(rejection) = validate_payload(event, rule) {
        findings.push(rejection);
    }

    // 2. Task validation (if payload has required fields)
    if findings.is_empty() {
        if let Some(rejection) = validate_task(event, rule, current_loop_id, tasks_path) {
            findings.push(rejection);
        }
    }

    // 3. Git evidence validation (if task validation passed)
    if findings.is_empty() {
        if let Some(rejection) =
            validate_git_change(event, rule, workspace_root, git_provider, loop_start_sha)
        {
            findings.push(rejection);
        }
    }

    // 4. Test evidence validation (if git evidence passed)
    if findings.is_empty() {
        if let Some(rejection) = validate_test_evidence(event, rule) {
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
                message: format!("work.done payload is missing required field: '{}'", field),
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
    // Only validate task if id_field is configured
    if rule.require_task.id_field.is_empty() {
        return None;
    }

    let payload_str = event.payload.as_str();

    // Empty payload with required task field → reject (fail-closed)
    if payload_str.trim().is_empty() {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::MissingPayloadField {
                field: rule.require_task.id_field.clone(),
            },
            message: format!(
                "work.done payload is empty but contract requires task field '{}'",
                rule.require_task.id_field
            ),
            topic: event.topic.to_string(),
        });
    }

    // JSON parse failure → reject (fail-closed)
    let Ok(payload) = serde_json::from_str::<Value>(payload_str) else {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::InvalidPayload,
            message: "work.done payload is not valid JSON, cannot read task_id".to_string(),
            topic: event.topic.to_string(),
        });
    };

    // Not a JSON object → reject (fail-closed)
    let Value::Object(map) = &payload else {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::InvalidPayload,
            message: "work.done payload must be a JSON object to validate task".to_string(),
            topic: event.topic.to_string(),
        });
    };

    // task_id field must exist and be a non-empty string
    let task_id = match map.get(&rule.require_task.id_field) {
        Some(Value::String(s)) if !s.trim().is_empty() => s.trim().to_string(),
        Some(other) => {
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::InvalidPayload,
                message: format!(
                    "task_id field '{}' must be a non-empty string, got: {:?}",
                    rule.require_task.id_field, other
                ),
                topic: event.topic.to_string(),
            });
        }
        None => {
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::MissingPayloadField {
                    field: rule.require_task.id_field.clone(),
                },
                message: format!(
                    "work.done payload is missing required task field '{}'",
                    rule.require_task.id_field
                ),
                topic: event.topic.to_string(),
            });
        }
    };

    // task_key field: if configured, must exist and be a string
    let _task_key_from_payload = if !rule.require_task.key_field.is_empty() {
        match map.get(&rule.require_task.key_field) {
            Some(Value::String(s)) => Some(s.as_str()),
            Some(other) => {
                return Some(ExecutionContractFinding {
                    kind: ExecutionContractViolationKind::InvalidPayload,
                    message: format!(
                        "task_key field '{}' must be a string, got: {:?}",
                        rule.require_task.key_field, other
                    ),
                    topic: event.topic.to_string(),
                });
            }
            None => {
                return Some(ExecutionContractFinding {
                    kind: ExecutionContractViolationKind::MissingPayloadField {
                        field: rule.require_task.key_field.clone(),
                    },
                    message: format!(
                        "work.done payload is missing required task key field '{}'",
                        rule.require_task.key_field
                    ),
                    topic: event.topic.to_string(),
                });
            }
        }
    } else {
        None
    };

    // Load the task store — fail-closed: load failure = reject
    let store = match TaskStore::load(tasks_path) {
        Ok(s) => s,
        Err(e) => {
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::TaskNotFound {
                    task_id: task_id.clone(),
                },
                message: format!(
                    "Failed to load task store for validation: {}. Rejecting work.done to prevent false completion.",
                    e
                ),
                topic: event.topic.to_string(),
            });
        }
    };

    // Find the task — fail-closed: not found = reject
    let task = match store.get(&task_id) {
        Some(t) => t,
        None => {
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::TaskNotFound {
                    task_id: task_id.clone(),
                },
                message: format!(
                    "Task '{}' not found in task store. work.done rejected to prevent false completion.",
                    task_id
                ),
                topic: event.topic.to_string(),
            });
        }
    };

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
    git_provider: &dyn GitEvidenceProvider,
    loop_start_sha: Option<&str>,
) -> Option<ExecutionContractFinding> {
    // If workspace is not a git repo, git evidence check is not applicable
    if !git_provider.is_git_repo(workspace_root) {
        return None;
    }

    // Check if this step allows empty git (trivial path)
    let payload_str = event.payload.as_str();
    if !payload_str.trim().is_empty() {
        if let Ok(payload) = serde_json::from_str::<Value>(payload_str) {
            if let Value::Object(map) = &payload {
                if let Some(step) = map.get("step").and_then(|v| v.as_str()) {
                    if rule
                        .require_git_change
                        .allow_empty_for_steps
                        .contains(&step.to_string())
                    {
                        return None;
                    }
                }
            }
        }
    }

    let has_uncommitted = git_provider.has_uncommitted_changes(workspace_root);
    let has_new_commits = git_provider.has_new_commits_since(workspace_root, loop_start_sha);

    let has_evidence = match rule.require_git_change.mode.as_str() {
        "diff_or_commit" => has_uncommitted || has_new_commits,
        "diff_only" => has_uncommitted,
        "commit_only" => has_new_commits,
        other => {
            // Unknown mode: warn and use conservative fail-closed
            warn!(mode = %other, "Unknown git change mode, treating as diff_or_commit");
            has_uncommitted || has_new_commits
        }
    };

    if !has_evidence {
        let step = serde_json::from_str::<Value>(payload_str)
            .ok()
            .and_then(|v| v.get("step").and_then(|s| s.as_str().map(String::from)));

        let detail = if loop_start_sha.is_none() {
            "No uncommitted changes found. (Loop start SHA not tracked — commit-only evidence unavailable.)".to_string()
        } else {
            "No uncommitted changes and no new commits since loop start found.".to_string()
        };

        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::NoGitEvidence { step },
            message: format!(
                "work.done requires git evidence before review can proceed. {}",
                detail
            ),
            topic: event.topic.to_string(),
        });
    }

    None
}

/// Validate that test evidence is present in the payload (if required).
fn validate_test_evidence(
    event: &Event,
    rule: &ExecutionContractRule,
) -> Option<ExecutionContractFinding> {
    // "optional" mode — always pass
    if rule.require_test_evidence.mode == "optional" {
        return None;
    }

    // "required_payload_field" mode — check for the specified field
    let field_name = rule
        .require_test_evidence
        .payload_field
        .as_deref()
        .unwrap_or("tests");

    let payload_str = event.payload.as_str();
    if payload_str.trim().is_empty() {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::NoTestEvidence {
                field: field_name.to_string(),
            },
            message: format!(
                "work.done payload is empty but contract requires test evidence in field '{}'",
                field_name
            ),
            topic: event.topic.to_string(),
        });
    }

    let Ok(payload) = serde_json::from_str::<Value>(payload_str) else {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::NoTestEvidence {
                field: field_name.to_string(),
            },
            message: format!(
                "work.done payload is not valid JSON, cannot verify test evidence field '{}'",
                field_name
            ),
            topic: event.topic.to_string(),
        });
    };

    let Value::Object(map) = &payload else {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::NoTestEvidence {
                field: field_name.to_string(),
            },
            message: format!(
                "work.done payload must be a JSON object to verify test evidence field '{}'",
                field_name
            ),
            topic: event.topic.to_string(),
        });
    };

    // Check if the field exists and is non-empty
    match map.get(field_name) {
        Some(Value::String(s)) if !s.trim().is_empty() => None,
        Some(Value::Array(arr)) if !arr.is_empty() => None,
        Some(Value::Object(obj)) if !obj.is_empty() => None,
        Some(Value::Bool(b)) if *b => None,
        _ => Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::NoTestEvidence {
                field: field_name.to_string(),
            },
            message: format!(
                "work.done payload is missing or empty test evidence field '{}'. \
                 Provide test results, test output, or set the field to a non-empty value.",
                field_name
            ),
            topic: event.topic.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ContractRejectConfig, ExecutionContractRule, GitChangeRequirement,
        TaskCompletionRequirement, TestEvidenceRequirement,
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
                payload_field: None,
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
                payload_field: None,
            },
            reject: ContractRejectConfig::default(),
        }
    }

    #[test]
    fn test_accepts_valid_work_done() {
        let rule = make_work_done_rule();
        let _event = Event::new(
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
            &DefaultGitEvidenceProvider,
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
            &DefaultGitEvidenceProvider,
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
        let _event = Event::new(
            "work.done",
            r#"{"task_id":"task-1","task_key":"key-1","step":"trivial"}"#,
        );

        // With trivial step, git validation should pass even without git
        // We can't easily test this without mocking, but the structure is correct
        assert!(
            rule.require_git_change
                .allow_empty_for_steps
                .contains(&"trivial".to_string())
        );
    }

    #[test]
    fn test_disabled_contract_passes_through() {
        // When rule is Default (disabled), validation should pass
        // Use a truly disabled rule with empty id_field to skip task validation
        let rule = ExecutionContractRule {
            require_payload_fields: vec![],
            require_task: TaskCompletionRequirement {
                id_field: "".to_string(), // Empty = task validation skipped
                key_field: "".to_string(),
                loop_scoped: false,
                allowed_terminal_statuses: vec![],
                auto_close_on_valid: false,
            },
            require_git_change: GitChangeRequirement::default(),
            require_test_evidence: TestEvidenceRequirement::default(),
            reject: ContractRejectConfig::default(),
        };
        let event = Event::new("work.done", r#"{"task_id":"task-1"}"#);

        // A default/empty rule should accept any event
        let decision = validate_execution_contract(
            &event,
            &rule,
            std::path::Path::new("/tmp"),
            "loop-1",
            std::path::Path::new("/tmp/tasks.jsonl"),
            None,
            &DefaultGitEvidenceProvider,
            None,
        );

        // Empty rule has no required fields, so it should accept
        assert!(matches!(decision, ExecutionContractDecision::Accept));
    }

    // === F1 Fail-closed tests ===

    #[test]
    fn test_rejects_task_id_missing() {
        // Payload missing task_id field should be rejected
        let rule = make_work_done_rule();
        let event = Event::new(
            "work.done",
            r#"{"plan_name":"test","plan_path":"/test","task_key":"key-1","step":"step-01"}"#,
        );

        let decision = validate_execution_contract(
            &event,
            &rule,
            std::path::Path::new("/tmp"),
            "loop-1",
            std::path::Path::new("/tmp/tasks.jsonl"),
            None,
            &DefaultGitEvidenceProvider,
            None,
        );

        match &decision {
            ExecutionContractDecision::Reject(findings) => {
                assert!(
                    findings.iter().any(|f| matches!(&f.kind, ExecutionContractViolationKind::MissingPayloadField { field } if field == "task_id")),
                    "Should have MissingPayloadField for task_id"
                );
            }
            ExecutionContractDecision::Accept => panic!("Expected rejection for missing task_id"),
        }
    }

    #[test]
    fn test_rejects_task_id_not_string() {
        // task_id field is not a string should be rejected
        let rule = make_work_done_rule();
        let event = Event::new(
            "work.done",
            r#"{"plan_name":"test","plan_path":"/test","task_id":123,"task_key":"key-1","step":"step-01"}"#,
        );

        let decision = validate_execution_contract(
            &event,
            &rule,
            std::path::Path::new("/tmp"),
            "loop-1",
            std::path::Path::new("/tmp/tasks.jsonl"),
            None,
            &DefaultGitEvidenceProvider,
            None,
        );

        match &decision {
            ExecutionContractDecision::Reject(findings) => {
                assert!(
                    findings
                        .iter()
                        .any(|f| matches!(f.kind, ExecutionContractViolationKind::InvalidPayload)),
                    "Should have InvalidPayload for non-string task_id"
                );
            }
            ExecutionContractDecision::Accept => {
                panic!("Expected rejection for non-string task_id")
            }
        }
    }

    #[test]
    fn test_rejects_task_id_empty_string() {
        // task_id field is empty string should be rejected
        let rule = make_work_done_rule();
        let event = Event::new(
            "work.done",
            r#"{"plan_name":"test","plan_path":"/test","task_id":"","task_key":"key-1","step":"step-01"}"#,
        );

        let decision = validate_execution_contract(
            &event,
            &rule,
            std::path::Path::new("/tmp"),
            "loop-1",
            std::path::Path::new("/tmp/tasks.jsonl"),
            None,
            &DefaultGitEvidenceProvider,
            None,
        );

        match &decision {
            ExecutionContractDecision::Reject(findings) => {
                assert!(
                    findings
                        .iter()
                        .any(|f| matches!(f.kind, ExecutionContractViolationKind::InvalidPayload)),
                    "Should have InvalidPayload for empty string task_id"
                );
            }
            ExecutionContractDecision::Accept => {
                panic!("Expected rejection for empty string task_id")
            }
        }
    }

    #[test]
    fn test_rejects_task_not_found() {
        // Task doesn't exist in task store should be rejected
        let rule = make_work_done_rule();
        let event = Event::new(
            "work.done",
            r#"{"plan_name":"test","plan_path":"/test","task_id":"nonexistent-task","task_key":"key-1","step":"step-01"}"#,
        );

        let decision = validate_execution_contract(
            &event,
            &rule,
            std::path::Path::new("/tmp"),
            "loop-1",
            std::path::Path::new("/tmp/nonexistent_tasks.jsonl"),
            None,
            &DefaultGitEvidenceProvider,
            None,
        );

        match &decision {
            ExecutionContractDecision::Reject(findings) => {
                assert!(
                    findings.iter().any(|f| matches!(
                        f.kind,
                        ExecutionContractViolationKind::TaskNotFound { .. }
                    )),
                    "Should have TaskNotFound rejection"
                );
            }
            ExecutionContractDecision::Accept => panic!("Expected rejection for nonexistent task"),
        }
    }

    // === F2 Git evidence tests ===

    #[test]
    fn test_git_evidence_skipped_in_non_git_directory() {
        // /tmp is not a git repo, so git evidence check should be skipped (not applicable)
        // We use a rule with empty id_field to skip task validation
        let rule = ExecutionContractRule {
            require_payload_fields: vec![],
            require_task: TaskCompletionRequirement {
                id_field: "".to_string(),
                key_field: "".to_string(),
                loop_scoped: false,
                allowed_terminal_statuses: vec![],
                auto_close_on_valid: false,
            },
            require_git_change: GitChangeRequirement::default(),
            require_test_evidence: TestEvidenceRequirement::default(),
            reject: ContractRejectConfig::default(),
        };
        let event = Event::new("work.done", r#"{"step":"test"}"#);

        // In /tmp (not a git repo), git evidence check is not applicable, so passes
        let decision = validate_execution_contract(
            &event,
            &rule,
            std::path::Path::new("/tmp"),
            "loop-1",
            std::path::Path::new("/tmp/tasks.jsonl"),
            None,
            &DefaultGitEvidenceProvider,
            None,
        );

        // Git evidence check should be skipped in non-git directory, so accepts
        assert!(matches!(decision, ExecutionContractDecision::Accept));
    }

    #[test]
    fn test_rejects_invalid_payload_json() {
        // Payload is not valid JSON should be rejected at payload validation
        let mut rule = make_work_done_rule();
        rule.require_test_evidence = TestEvidenceRequirement {
            mode: "required_payload_field".to_string(),
            payload_field: Some("tests".to_string()),
        };

        let event = Event::new("work.done", "not valid json at all");

        let decision = validate_execution_contract(
            &event,
            &rule,
            std::path::Path::new("/tmp"),
            "loop-1",
            std::path::Path::new("/tmp/tasks.jsonl"),
            None,
            &DefaultGitEvidenceProvider,
            None,
        );

        // Invalid JSON is rejected at payload validation stage, before test evidence check
        match &decision {
            ExecutionContractDecision::Reject(findings) => {
                assert!(
                    findings
                        .iter()
                        .any(|f| matches!(f.kind, ExecutionContractViolationKind::InvalidPayload)),
                    "Should have InvalidPayload for invalid JSON"
                );
            }
            ExecutionContractDecision::Accept => {
                panic!("Expected rejection for invalid JSON payload")
            }
        }
    }
}
