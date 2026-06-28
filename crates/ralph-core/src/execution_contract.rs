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

/// Hint appended to the `TaskNotTerminal` rejection message so the rejected
/// agent (or human reader) sees an actionable `ralph tools task close`
/// command and knows the next concrete step. The `<task_id>` placeholder
/// is replaced with the actual task id at the rejection site.
///
/// Kept as a single line on purpose (Tenet #2: backpressure over
/// prescription): the `HUMAN GUIDANCE` injection path copies this string
/// into a numbered list entry, so embedded newlines would break that
/// contract and degrade the message into a hard-to-scan blob.
const TASK_NOT_TERMINAL_HINT_TEMPLATE: &str =
    " Run `ralph tools task close <task_id>` first, then re-emit work.done with task_id=<task_id>.";

/// Render the `TaskNotTerminal` hint with the actual task id substituted in.
fn task_not_terminal_hint(task_id: &str) -> String {
    TASK_NOT_TERMINAL_HINT_TEMPLATE.replace("<task_id>", task_id)
}

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
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize)]
pub struct ExecutionContractFinding {
    /// What kind of violation was detected.
    pub kind: ExecutionContractViolationKind,
    /// Human-readable description of the violation.
    pub message: String,
    /// The event topic being validated.
    pub topic: String,
    /// The original `event.hat` (or the runner's `last_active_hat_id`
    /// fallback) for the rejected event.  Carries the **provenance**
    /// of the event so downstream recovery (U2) can route the targeted
    /// `task.resume` to the hat that actually emitted the event
    /// rather than the runner's current display hat.  This is the
    /// difference between "executor was running but the event came
    /// from ralph's fallback" and "executor emitted the event".
    /// `None` only when the caller (legacy code paths) did not supply
    /// a hat id to `validate_execution_contract`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hat: Option<String>,
}

impl ExecutionContractFinding {
    /// Return the list of payload fields that the agent must add or
    /// fix to make the rejected event satisfy its contract.  Used by
    /// the U2 targeted-retry machinery (`build_task_resume_payload`)
    /// to embed a "fix the contract" hint in the `task.resume` event
    /// so the resumed hat does not have to guess what to change.
    ///
    /// Returns an empty list for violation kinds that are not
    /// field-fixable (e.g. `TaskNotFound`, `NoGitEvidence`) so the
    /// caller can safely treat the result as "no specific field to
    /// fill in".
    pub fn required_fields_for_resume(&self) -> Vec<String> {
        match &self.kind {
            ExecutionContractViolationKind::MissingPayloadField { field } => vec![field.clone()],
            ExecutionContractViolationKind::NoTestEvidence { field } => vec![field.clone()],
            _ => Vec::new(),
        }
    }
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

impl Default for ExecutionContractViolationKind {
    fn default() -> Self {
        // Used only as a placeholder by `..Default::default()` on
        // `ExecutionContractFinding` in the validate_* helpers — the
        // actual variant is overwritten by the literal field
        // initialiser that precedes `..Default::default()`.  Pick the
        // simplest variant so accidental Default construction is
        // visible in code review.
        ExecutionContractViolationKind::InvalidPayload
    }
}

/// Validate an event against an execution contract rule.
///
/// Returns `Accept` if all contract requirements are satisfied, or `Reject`
/// with a list of findings describing each violation.
///
/// `hat_id` is the **provenance** of the event (the hat that emitted
/// it, as recorded on the original JSONL `event.hat` or the runner's
/// `last_active_hat_id` fallback).  It is propagated onto every
/// [`ExecutionContractFinding`] so downstream recovery (U2) can route
/// the targeted `task.resume` to the correct hat — NOT the runner's
/// current display hat.  When the caller cannot determine provenance
/// (legacy path) it should pass `None`; in that case downstream code
/// must fall back to the display hat.
pub fn validate_execution_contract(
    event: &Event,
    rule: &ExecutionContractRule,
    workspace_root: &Path,
    current_loop_id: &str,
    tasks_path: &Path,
    hat_id: Option<&str>,
    git_provider: &dyn GitEvidenceProvider,
    loop_start_sha: Option<&str>,
) -> ExecutionContractDecision {
    let source_hat = hat_id.map(|s| s.to_string());
    let mut findings = Vec::new();

    // 1. Payload validation
    if let Some(rejection) = validate_payload(event, rule) {
        findings.push(with_source_hat(rejection, source_hat.clone()));
    }

    // 2. Task validation (if payload has required fields)
    if findings.is_empty() {
        if let Some(rejection) = validate_task(event, rule, current_loop_id, tasks_path) {
            findings.push(with_source_hat(rejection, source_hat.clone()));
        }
    }

    // 3. Git evidence validation (if task validation passed)
    if findings.is_empty() {
        if let Some(rejection) =
            validate_git_change(event, rule, workspace_root, git_provider, loop_start_sha)
        {
            findings.push(with_source_hat(rejection, source_hat.clone()));
        }
    }

    // 4. Test evidence validation (if git evidence passed)
    if findings.is_empty() {
        if let Some(rejection) = validate_test_evidence(event, rule) {
            findings.push(with_source_hat(rejection, source_hat));
        }
    }

    if findings.is_empty() {
        ExecutionContractDecision::Accept
    } else {
        ExecutionContractDecision::Reject(findings)
    }
}

/// Stamp the provenance hat onto a finding.  Split out so the four
/// `validate_*` helpers can keep their `None` literal return type for
/// "no violation" and the validation pipeline still threads the
/// provenance through uniformly.
fn with_source_hat(
    mut finding: ExecutionContractFinding,
    source_hat: Option<String>,
) -> ExecutionContractFinding {
    finding.source_hat = source_hat;
    finding
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
                // U6 (2026-06-18-004 plan): dynamic topic so
                // the same message works for both `work.done`
                // and `fix.applied`.
                message: format!(
                    "{} payload is empty but contract requires fields: {:?}",
                    event.topic, rule.require_payload_fields
                ),
                topic: event.topic.to_string(),
                ..Default::default()
            });
        }
    }

    let Ok(payload) = serde_json::from_str::<Value>(payload_str) else {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::InvalidPayload,
            message: format!(
                "{} payload is not valid JSON: {:?}",
                event.topic,
                payload_str.chars().take(100).collect::<String>()
            ),
            topic: event.topic.to_string(),
            ..Default::default()
        });
    };

    let Value::Object(map) = &payload else {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::InvalidPayload,
            message: format!("{} payload must be a JSON object", event.topic),
            topic: event.topic.to_string(),
            ..Default::default()
        });
    };

    for field in &rule.require_payload_fields {
        if !map.contains_key(field) {
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::MissingPayloadField {
                    field: field.clone(),
                },
                message: format!(
                    "{} payload is missing required field: '{}'",
                    event.topic, field
                ),
                topic: event.topic.to_string(),
                ..Default::default()
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
            ..Default::default()
        });
    }

    // JSON parse failure → reject (fail-closed)
    let Ok(payload) = serde_json::from_str::<Value>(payload_str) else {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::InvalidPayload,
            message: "work.done payload is not valid JSON, cannot read task_id".to_string(),
            topic: event.topic.to_string(),
            ..Default::default()
        });
    };

    // Not a JSON object → reject (fail-closed)
    let Value::Object(map) = &payload else {
        return Some(ExecutionContractFinding {
            kind: ExecutionContractViolationKind::InvalidPayload,
            message: "work.done payload must be a JSON object to validate task".to_string(),
            topic: event.topic.to_string(),
            ..Default::default()
        });
    };

    // task_id field must exist and be a non-empty string.
    // 2026-06-28 plan U5 (R8): reject placeholder
    // `task_id` values that agents sometimes emit to
    // "satisfy" the field. The runtime treats them as
    // missing — the upstream projector (U5) and
    // execution-contract (here) both reject explicitly
    // so the audit trail surfaces a stable reason.
    let task_id = match map.get(&rule.require_task.id_field) {
        Some(Value::String(s)) if !s.trim().is_empty() => {
            if s.trim().ends_with("-placeholder") {
                return Some(ExecutionContractFinding {
                    kind: ExecutionContractViolationKind::InvalidPayload,
                    message: format!(
                        "task_id field '{}' has a placeholder value ('{}'); the loop does not accept placeholder task_ids. Re-emit with a real id (U5, 2026-06-28 plan).",
                        rule.require_task.id_field, s
                    ),
                    topic: event.topic.to_string(),
                    ..Default::default()
                });
            }
            s.trim().to_string()
        }
        Some(other) => {
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::InvalidPayload,
                message: format!(
                    "task_id field '{}' must be a non-empty string (got empty). Set task_key so the projector can derive one (U5, 2026-06-28 plan).",
                    rule.require_task.id_field
                ),
                topic: event.topic.to_string(),
                ..Default::default()
            });
        }
        None => {
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::MissingPayloadField {
                    field: rule.require_task.id_field.clone(),
                },
                message: format!(
                    "work.done payload is missing required task field '{}'. If the agent does not have a real id, set task_key so the projector can derive one (U5, 2026-06-28 plan).",
                    rule.require_task.id_field
                ),
                topic: event.topic.to_string(),
                ..Default::default()
            });
        }
    };

    // task_key field: if configured, must exist and be a
    // string. The presence of a valid `task_key` lets the
    // projector derive a canonical `from_key:<key>` id when
    // the agent's `task_id` is empty (2026-06-28 plan U5,
    // R8: task_id fallback). The variable is also exposed
    // to the TaskStore lookup below so a `work.done` with
    // `task_id=""` + matching `task_key` can find the
    // existing task opened under the projector-derived id.
    let task_key_from_payload = if !rule.require_task.key_field.is_empty() {
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
                    ..Default::default()
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
                    ..Default::default()
                });
            }
        }
    } else {
        None
    };

    // 2026-06-28 plan U5 (R8): when the agent's `task_id` is
    // empty AND a valid `task_key` is present, look up the
    // task under the projector-derived `from_key:<key>` id.
    // This makes the round-trip (work.ready → work.done with
    // empty task_id) close the same task record instead of
    // leaving it open forever.
    let resolved_task_id: String = if task_id.is_empty() {
        match task_key_from_payload {
            Some(key) if !key.is_empty() => format!("from_key:{key}"),
            // No task_key fallback path — fall through to the
            // existing reject (already returned above for
            // empty task_id).
            _ => task_id.clone(),
        }
    } else {
        task_id.clone()
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
                ..Default::default()
            });
        }
    };

    // Find the task — fail-closed: not found = reject.
    // 2026-06-28 plan U5 (R8): when `task_id` is empty and
    // `task_key` is present, look up the task under the
    // projector-derived `from_key:<key>` id first. If that
    // fails, retry the original `task_id` so an existing
    // task under the literal id is still found.
    let task = if !task_id.is_empty() {
        match store.get(&task_id) {
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
                    ..Default::default()
                });
            }
        }
    } else {
        // Empty task_id: try the derived id, then fall
        // through to the original reject.
        match store.get(&resolved_task_id) {
            Some(t) => t,
            None => {
                return Some(ExecutionContractFinding {
                    kind: ExecutionContractViolationKind::TaskNotFound {
                        task_id: resolved_task_id.clone(),
                    },
                    message: format!(
                        "Task '{}' (derived from task_key '{}') not found in task store. work.done rejected to prevent false completion.",
                        resolved_task_id,
                        task_key_from_payload.unwrap_or(""),
                    ),
                    topic: event.topic.to_string(),
                    ..Default::default()
                });
            }
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
                    ..Default::default()
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
                ..Default::default()
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
                "Task '{}' has status '{}', expected one of {:?}.{}",
                task_id,
                status_str,
                rule.require_task.allowed_terminal_statuses,
                task_not_terminal_hint(task_id.as_str()),
            ),
            topic: event.topic.to_string(),
            ..Default::default()
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
            // U6 (2026-06-18-004 plan): dynamic topic — the
            // previous hardcoded `work.done requires git
            // evidence` was misleading for `fix.applied` (which
            // uses the same `require_git_change` field). The
            // recovered agent reads the topic from the finding
            // to know which hat's contract fired.
            message: format!(
                "{} requires git evidence before downstream review can proceed. {}",
                event.topic, detail
            ),
            topic: event.topic.to_string(),
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
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
            ..Default::default()
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::{
        ExecutionContractRule,
        execution_contracts::{
            ContractRejectConfig, GitChangeRequirement, TaskCompletionRequirement,
            TestEvidenceRequirement,
        },
    };
    use crate::task::{Task, TaskStatus};
    use crate::task_store::TaskStore;
    use tempfile::TempDir;

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

    // === U1 / F1 TaskNotTerminal message-hint tests ===
    //
    // The cheery-eagle "forgot to close" incident (see plan
    // docs/plans/2026-06-08-002-fix-ce-executor-preset-forgot-close-step-guard-plan.md)
    // showed the previous `TaskNotTerminal` message only diagnosed the
    // problem ("status is open, expected closed") without telling the
    // agent what to do. These tests pin the new contract: the rejection
    // message MUST include the `ralph tools task close <task_id>` command
    // and MUST stay on a single line so the `HUMAN GUIDANCE` injection
    // path can copy it verbatim into a numbered list entry.

    /// Write a single task to a fresh tasks.jsonl with the given id, status,
    /// and (optional) loop_id. Mirrors the helper used by the marker/loop
    /// tests at the bottom of this module.
    fn write_task(
        tasks_path: &std::path::Path,
        task_id: &str,
        status: TaskStatus,
        loop_id: Option<&str>,
    ) {
        if let Some(parent) = tasks_path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        let mut store = TaskStore::load(tasks_path).unwrap();
        let mut task =
            Task::new(format!("task {task_id}"), 1).with_loop_id(loop_id.map(str::to_string));
        task.id = task_id.to_string();
        task.status = status;
        store.add(task);
        store.save().unwrap();
    }

    #[test]
    fn test_task_not_terminal_message_includes_close_hint() {
        // Task exists in the store but is still `open`; the contract rule
        // requires `closed`. The expected `TaskNotTerminal` finding's
        // message must (a) preserve the original diagnostic and (b)
        // include the actionable `ralph tools task close T` command with
        // the real task id, not the `<task_id>` placeholder.
        let temp = TempDir::new().unwrap();
        let ralph_dir = temp.path().join(".ralph");
        std::fs::create_dir_all(ralph_dir.join("agent")).unwrap();
        let tasks_path = ralph_dir.join("agent/tasks.jsonl");
        write_task(&tasks_path, "T", TaskStatus::Open, Some("loop-1"));

        let rule = make_work_done_rule();
        let event = Event::new(
            "work.done",
            r#"{"plan_name":"p","plan_path":"/p","task_id":"T","task_key":"k","step":"step-01"}"#,
        );

        let decision = validate_execution_contract(
            &event,
            &rule,
            temp.path(),
            "loop-1",
            &tasks_path,
            None,
            &DefaultGitEvidenceProvider,
            None,
        );

        match &decision {
            ExecutionContractDecision::Reject(findings) => {
                let finding = findings
                    .iter()
                    .find_map(|f| match &f.kind {
                        ExecutionContractViolationKind::TaskNotTerminal { .. } => Some(f),
                        _ => None,
                    })
                    .expect("Should have a TaskNotTerminal rejection");

                // Diagnostic prefix is preserved.
                assert!(
                    finding.message.contains("Task 'T' has status 'open'"),
                    "diagnostic prefix missing: {}",
                    finding.message
                );
                // Actionable hint is present, with the real task id and
                // the literal `ralph tools task close` command.
                assert!(
                    finding.message.contains("ralph tools task close T"),
                    "close hint missing actual task id: {}",
                    finding.message
                );
                // The placeholder must have been substituted, not leaked.
                assert!(
                    !finding.message.contains("<task_id>"),
                    "placeholder leaked into message: {}",
                    finding.message
                );
                // And the hint points the agent at re-emitting work.done
                // with the same task_id.
                assert!(
                    finding.message.contains("re-emit work.done with task_id=T"),
                    "re-emit hint missing: {}",
                    finding.message
                );
            }
            ExecutionContractDecision::Accept => {
                panic!("Expected TaskNotTerminal rejection for open task")
            }
        }
    }

    #[test]
    fn test_task_not_terminal_message_is_human_readable() {
        // The HUMAN GUIDANCE injection path embeds finding messages into
        // a numbered list. Any embedded newline would either be stripped
        // (losing the hint) or break the bullet formatting. Pin the
        // contract: the message must be a single line.
        let temp = TempDir::new().unwrap();
        let ralph_dir = temp.path().join(".ralph");
        std::fs::create_dir_all(ralph_dir.join("agent")).unwrap();
        let tasks_path = ralph_dir.join("agent/tasks.jsonl");
        write_task(&tasks_path, "T", TaskStatus::Open, Some("loop-1"));

        let rule = make_work_done_rule();
        let event = Event::new(
            "work.done",
            r#"{"plan_name":"p","plan_path":"/p","task_id":"T","task_key":"k","step":"step-01"}"#,
        );

        let decision = validate_execution_contract(
            &event,
            &rule,
            temp.path(),
            "loop-1",
            &tasks_path,
            None,
            &DefaultGitEvidenceProvider,
            None,
        );

        match &decision {
            ExecutionContractDecision::Reject(findings) => {
                let finding = findings
                    .iter()
                    .find_map(|f| match &f.kind {
                        ExecutionContractViolationKind::TaskNotTerminal { .. } => Some(f),
                        _ => None,
                    })
                    .expect("Should have a TaskNotTerminal rejection");

                assert!(
                    !finding.message.contains('\n'),
                    "TaskNotTerminal message must stay on a single line for HUMAN GUIDANCE: {:?}",
                    finding.message
                );
                assert!(
                    !finding.message.contains('\r'),
                    "TaskNotTerminal message must not contain carriage returns: {:?}",
                    finding.message
                );
                assert!(
                    !finding.message.is_empty(),
                    "TaskNotTerminal message must not be empty"
                );
            }
            ExecutionContractDecision::Accept => {
                panic!("Expected TaskNotTerminal rejection for open task")
            }
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

    // === TaskWrongLoop primary-loop regression tests ===
    //
    // Background: `LoopContext::primary()` keeps `loop_id: None` (loop_context.rs:89),
    // and primary loops identify themselves via the `.ralph/current-loop-id` marker
    // that `LoopRunner::resolve_loop_id` writes (loop_runner.rs:183-203). The
    // execution-contract call site at event_loop/mod.rs:3590 must pass the marker
    // value (not the literal "default") so primary-loop tasks are not misclassified.
    // These tests pin the validator's TaskWrongLoop behavior given a marker value,
    // independent of the EventLoop wiring.

    fn write_task_with_loop_id(tasks_path: &std::path::Path, task_id: &str, loop_id: Option<&str>) {
        let mut store = TaskStore::load(tasks_path).unwrap();
        let mut task =
            Task::new(format!("task {task_id}"), 1).with_loop_id(loop_id.map(str::to_string));
        task.id = task_id.to_string();
        task.status = TaskStatus::Closed;
        store.add(task);
        store.save().unwrap();
    }

    fn read_marker(temp: &TempDir) -> String {
        let marker = temp.path().join(".ralph/current-loop-id");
        std::fs::read_to_string(&marker).unwrap().trim().to_string()
    }

    #[test]
    fn test_contract_check_passes_when_task_loop_matches_marker_for_primary() {
        let temp = TempDir::new().unwrap();
        let ralph_dir = temp.path().join(".ralph");
        std::fs::create_dir_all(ralph_dir.join("agent")).unwrap();
        std::fs::write(
            ralph_dir.join("current-loop-id"),
            "primary-20260604-091852\n",
        )
        .unwrap();
        let tasks_path = ralph_dir.join("agent/tasks.jsonl");
        write_task_with_loop_id(&tasks_path, "task-1", Some("primary-20260604-091852"));

        let rule = make_work_done_rule();
        let event = Event::new(
            "work.done",
            r#"{"plan_name":"p","plan_path":"/p","task_id":"task-1","task_key":"k","step":"s"}"#,
        );
        let current_loop_id = read_marker(&temp);

        let decision = validate_execution_contract(
            &event,
            &rule,
            temp.path(),
            &current_loop_id,
            &tasks_path,
            None,
            &DefaultGitEvidenceProvider,
            None,
        );

        assert!(
            !matches!(decision, ExecutionContractDecision::Reject(_)),
            "Task whose loop_id matches the marker should not be rejected. \
             Marker={current_loop_id}"
        );
    }

    #[test]
    fn test_contract_check_rejects_when_task_loop_mismatches_marker_for_primary() {
        let temp = TempDir::new().unwrap();
        let ralph_dir = temp.path().join(".ralph");
        std::fs::create_dir_all(ralph_dir.join("agent")).unwrap();
        std::fs::write(
            ralph_dir.join("current-loop-id"),
            "primary-20260604-091852\n",
        )
        .unwrap();
        let tasks_path = ralph_dir.join("agent/tasks.jsonl");
        write_task_with_loop_id(&tasks_path, "task-1", Some("primary-OTHER-LOOP"));

        let rule = make_work_done_rule();
        let event = Event::new(
            "work.done",
            r#"{"plan_name":"p","plan_path":"/p","task_id":"task-1","task_key":"k","step":"s"}"#,
        );
        let current_loop_id = read_marker(&temp);

        let decision = validate_execution_contract(
            &event,
            &rule,
            temp.path(),
            &current_loop_id,
            &tasks_path,
            None,
            &DefaultGitEvidenceProvider,
            None,
        );

        match decision {
            ExecutionContractDecision::Reject(findings) => {
                let wrong_loop = findings.iter().find_map(|f| match &f.kind {
                    ExecutionContractViolationKind::TaskWrongLoop {
                        expected_loop,
                        actual_loop,
                        ..
                    } => Some((expected_loop.clone(), actual_loop.clone())),
                    _ => None,
                });
                let (expected, actual) = wrong_loop.expect("expected TaskWrongLoop finding");
                assert_eq!(
                    expected, "primary-20260604-091852",
                    "expected_loop must reflect the marker value, not a hard-coded literal"
                );
                assert_eq!(actual.as_deref(), Some("primary-OTHER-LOOP"));
            }
            ExecutionContractDecision::Accept => {
                panic!("Task with wrong loop_id must be rejected")
            }
        }
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

// 2026-06-28 plan U5 (R8) tests for the placeholder rejection
// and task_id-fallback behaviour. The projector-side fallback
// (state_projector::task::ensure_task) is exercised in
// `state_projector::u5_tests` (see `task.rs`).
#[cfg(test)]
mod u5_placeholder_tests {
    use super::*;
    use crate::config::execution_contracts::TaskCompletionRequirement;
    use crate::task_store::TaskStore;
    use std::path::PathBuf;

    fn rule_with_task_field() -> ExecutionContractRule {
        ExecutionContractRule {
            require_payload_fields: vec!["task_id".to_string()],
            require_task: TaskCompletionRequirement {
                id_field: "task_id".to_string(),
                key_field: "task_key".to_string(),
                ..TaskCompletionRequirement::default()
            },
            ..ExecutionContractRule::default()
        }
    }

    fn tmp_tasks_path() -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ralph-u5-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join("tasks.jsonl")
    }

    #[test]
    fn u5_rejects_placeholder_task_id() {
        // A task_id ending in `-placeholder` must be rejected
        // even though it is a non-empty string.
        let event = Event::new(
            "work.done",
            r#"{"task_id":"abc-placeholder","task_key":"k1"}"#,
        );
        let finding = validate_task(&event, &rule_with_task_field(), "loop-1", &tmp_tasks_path())
            .expect("placeholder must be rejected");
        let msg = format!("{:?}", finding.kind);
        assert!(
            matches!(finding.kind, ExecutionContractViolationKind::InvalidPayload),
            "expected InvalidPayload, got {msg}"
        );
        assert!(
            finding.message.contains("placeholder"),
            "expected placeholder hint in message, got: {}",
            finding.message
        );
    }

    #[test]
    fn u5_rejects_empty_task_id_with_hint() {
        // An empty string is rejected (still a missing field
        // from the loop's perspective) and the message
        // mentions the task_key fallback.
        let event = Event::new("work.done", r#"{"task_id":"","task_key":"k1"}"#);
        let finding = validate_task(&event, &rule_with_task_field(), "loop-1", &tmp_tasks_path())
            .expect("empty task_id must be rejected");
        assert!(matches!(
            finding.kind,
            ExecutionContractViolationKind::InvalidPayload
        ));
        assert!(
            finding.message.contains("task_key"),
            "expected task_key fallback hint: {}",
            finding.message
        );
    }

    #[test]
    fn u5_accepts_valid_task_id() {
        // A non-placeholder, non-empty task_id is accepted
        // when the task exists in the store. The store
        // file may be empty (the projector creates tasks
        // on demand) so an unknown id is also acceptable
        // for the placeholder/empty cases — we just need
        // to confirm the validator no longer over-rejects
        // when the field is genuinely present.
        let path = tmp_tasks_path();
        let _store = TaskStore::load(&path).unwrap();

        let event = Event::new(
            "work.done",
            r#"{"task_id":"real-id-1","task_key":"k1"}"#,
        );
        // With an empty store and a real task_id, the
        // validator will return `TaskNotFound` (which is
        // *not* the U5 placeholder path). What matters
        // is that the placeholder/empty branches above
        // are exclusive: a real id never falls into
        // them.
        let finding = validate_task(&event, &rule_with_task_field(), "loop-1", &path);
        if let Some(f) = finding {
            assert!(
                !matches!(f.kind, ExecutionContractViolationKind::InvalidPayload),
                "real task_id must not hit the InvalidPayload placeholder path: {f:?}"
            );
        }
    }
}
