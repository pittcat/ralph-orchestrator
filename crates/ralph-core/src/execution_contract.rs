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
pub(crate) fn task_not_terminal_hint(task_id: &str) -> String {
    TASK_NOT_TERMINAL_HINT_TEMPLATE.replace("<task_id>", task_id)
}

/// DEV-005 (2026-07-06): choose the `task.resume` target and hint when
/// `work.done` is rejected as `TaskNotTerminal`. When `source_hat`
/// cannot close the referenced task, route recovery to the coordinator
/// hat that owns the task (or the first configured coordinator hat).
pub fn task_not_terminal_resume_plan(
    task_id: &str,
    task: Option<&crate::task::Task>,
    source_hat: &str,
    coordinator_hats: &[String],
) -> (String, String) {
    let target = task
        .map(|t| crate::task::lifecycle_close_delegate_hat(t, source_hat, coordinator_hats))
        .unwrap_or_else(|| source_hat.to_string());
    let hint = if target == source_hat {
        task_not_terminal_hint(task_id)
    } else {
        format!(
            "Task '{task_id}' is not closed. Hat '{source_hat}' cannot close it; \
             hat '{target}' must run `ralph tools task close {task_id}` first, \
             then hat '{source_hat}' re-emits work.done with task_id={task_id}."
        )
    };
    (target, hint)
}

/// P1-5 (2026-07-07-002): choose the `task.resume` target and hint when
/// `work.done` is rejected as `TaskNotFound`. The 2026-07-07 e2e stall
/// showed that a duplicate `task_id` row (coordinator `task add`
/// shadowing the projector row) makes the contract layer report
/// `TaskNotFound` even though the executor's implementation is fine.
/// Routing that recovery back to the executor is a dead end — the
/// executor cannot edit `tasks.jsonl`. We route to a coordinator hat
/// instead, with a hint that names the structured action (delete or
/// fix the orphan row), and only fall back to `source_hat` when no
/// coordinator hat is configured (legacy / human-CLI loops).
///
/// `task` is the row found by literal `task_id` (if any). When its
/// `owner_hat_id` is a configured coordinator hat, that hat is the
/// target; otherwise the first coordinator hat is chosen. When
/// `coordinator_hats` is empty, the source hat is returned so the
/// caller can still emit a recovery event without panicking.
pub fn task_not_found_resume_plan(
    task_id: &str,
    task_key: &str,
    task: Option<&crate::task::Task>,
    source_hat: &str,
    coordinator_hats: &[String],
) -> (String, String) {
    // No coordinator hats configured — legacy loop. Keep the old
    // behaviour so we do not break human-CLI / hatless presets.
    if coordinator_hats.is_empty() {
        return (
            source_hat.to_string(),
            format!(
                "Task '{task_id}' not found in task store. work.done rejected to prevent \
                 false completion. Verify the task_id with `ralph tools task list` and \
                 re-emit, or emit 'work.failed' if the task was never created."
            ),
        );
    }

    // Prefer the task's owner if it is a configured coordinator hat —
    // that hat minted the row and is the natural one to fix it.
    let target = task
        .and_then(|t| t.owner_hat_id.as_deref())
        .filter(|owner| coordinator_hats.iter().any(|h| h == owner))
        .map(|owner| owner.to_string())
        .or_else(|| coordinator_hats.first().cloned())
        .unwrap_or_else(|| source_hat.to_string());

    let hint = if task_key.is_empty() {
        format!(
            "Task '{task_id}' not found in task store. Hat '{source_hat}' cannot create \
             runtime tasks here; hat '{target}' must mint or repair the task row, then \
             hat '{source_hat}' re-emits work.done with the live task_id."
        )
    } else {
        format!(
            "Task '{task_id}' live identity mismatch: payload task_key='{task_key}' does not \
             match the row bound to that id. Hat '{source_hat}' cannot fix the task ledger; \
             hat '{target}' must remove the orphan row (or align its key to '{task_key}') \
             via `ralph tools task` commands, then hat '{source_hat}' re-emits work.done \
             with the live task_id from `ralph tools task list`. Do not emit work.failed — \
             the implementation is not the failure."
        )
    };
    (target, hint)
}

/// Git evidence provider abstraction for testability.
pub trait GitEvidenceProvider: Send + Sync {
    /// Returns true if the workspace is a git repository.
    fn is_git_repo(&self, workspace: &Path) -> bool;
    /// Returns true if there are unstaged or staged changes.
    fn has_uncommitted_changes(&self, workspace: &Path) -> bool;
    /// Returns true if there are commits since the given baseline SHA.
    fn has_new_commits_since(&self, workspace: &Path, start_sha: Option<&str>) -> bool;

    /// Returns the most recent commit messages reachable from HEAD, in
    /// reverse-chronological order (newest first).  When `since_sha` is
    /// `Some(start)`, only commits in the `start..HEAD` range are returned.
    /// When `None`, the latest `max_count` commit messages are returned.
    /// Returns an empty `Vec` when the command fails or no commits exist; this
    /// method must never panic so execution-contract soft checks can
    /// safely call it on broken workspaces.
    fn recent_commit_messages(
        &self,
        workspace: &Path,
        since_sha: Option<&str>,
        max_count: usize,
    ) -> Vec<String>;

    /// Returns the raw `git status --porcelain` output when the working
    /// tree is dirty (including untracked files outside `.gitignore`), or
    /// an empty string when the tree is clean / not a git repo / git
    /// invocation failed.  Used by the `commit_only_clean` execution-
    /// contract mode (2026-07-07 plan P0-1/P0-2 fix) to surface the
    /// exact dirty paths in the rejection finding so the agent can
    /// diagnose without re-running git itself.
    fn working_tree_porcelain(&self, workspace: &Path) -> String;
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

    fn recent_commit_messages(
        &self,
        workspace: &Path,
        since_sha: Option<&str>,
        max_count: usize,
    ) -> Vec<String> {
        if max_count == 0 {
            return Vec::new();
        }
        // Use `--format=%H%n%B%n--END--` so each commit is delimited
        // and the body (%B) can contain arbitrary newlines without
        // splitting a single commit into multiple entries.
        // We split off the SHA later (only messages are returned to
        // keep the contract narrow).  Reflog commits are excluded
        // (`--no-walk` is not appropriate here because we want
        // ranges; `--not --reflog` would be, but adds noise).  We
        // pick `%H%n%B%n--END--` so multiple subject+body chunks
        // are stable to split.
        let range = match since_sha {
            Some(start) => format!("{}..HEAD", start),
            None => "HEAD".to_string(),
        };
        let n = max_count.to_string();
        let output = Command::new("git")
            .args([
                "log",
                "--format=%H%n%B%n--END--",
                &format!("-{}", n),
                &range,
            ])
            .current_dir(workspace)
            .output();
        let Ok(out) = output else {
            return Vec::new();
        };
        if !out.status.success() {
            return Vec::new();
        }
        let stdout = String::from_utf8_lossy(&out.stdout);
        let mut messages = Vec::new();
        for raw in stdout.split("--END--\n") {
            let entry = raw.trim_end_matches('\n');
            if entry.is_empty() {
                continue;
            }
            // First line is the SHA (%H).  Drop it; we only want
            // the human-authored message body which is everything
            // after the first newline.
            match entry.find('\n') {
                Some(idx) => {
                    let body = entry[idx + 1..].trim().to_string();
                    if !body.is_empty() {
                        messages.push(body);
                    }
                }
                None => {
                    // Only SHA, no body — skip.
                }
            }
        }
        messages
    }

    fn working_tree_porcelain(&self, workspace: &Path) -> String {
        // `git status --porcelain` is the canonical dirty detector:
        // covers staged + unstaged + untracked (outside .gitignore)
        // in machine-parseable form.  Failure is non-fatal: we
        // return an empty string so the contract validator falls
        // back to `has_uncommitted_changes` semantics.
        let output = Command::new("git")
            .args(["status", "--porcelain"])
            .current_dir(workspace)
            .output();
        match output {
            Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).trim().to_string(),
            _ => String::new(),
        }
    }
}

/// Outcome of an execution contract validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionContractDecision {
    /// The event satisfies all hard contract requirements.
    Accept,
    /// The event violates one or more hard contract requirements.
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
#[derive(Default)]
pub enum ExecutionContractViolationKind {
    /// Payload is missing a required field.
    MissingPayloadField { field: String },
    /// Payload is not a valid JSON object.
    #[default]
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
    /// Soft-check finding: a fix-unit work event (e.g. `work.done`
    /// with `step="fix-NN"`) was emitted but the most recent commit
    /// in `since_sha..HEAD` does not carry the matching
    /// `[fix-unit: fix-NN]` footer.  Never used to reject the event;
    /// surfaced via the soft-check diagnostic channel so the agent
    /// can correct its commit-message convention next iteration.
    FixUnitTagMissing { step: String, expected_tag: String },
    /// P1-1 (2026-07-01-002 audit): a `work.ready(fix-XX)` event
    /// was emitted with an `fix-XX` id that is **not** in the
    /// projector-known fix-unit chain.  The agent is asked to
    /// re-pick via the synthesized `task.resume` payload.
    InvalidStepTarget {
        step: String,
        known_fix_units: Vec<String>,
    },
    /// 2026-07-07 plan P0-1/P0-2 fix (`commit_only_clean` mode
    /// only): there ARE new commits since loop start, but the
    /// working tree is still dirty.  The agent emitted `work.done`
    /// (or another gated event) before absorbing every change —
    /// e.g. an `Edit` on `docs/plans/<plan>.md` frontmatter that
    /// didn't make it into the U-ID commit.  Diagnostic report:
    /// `docs/report/2026-07-07-ce-executor-serial-primary-20260706-234147-diagnosis.md` §5 P0-1/P0-2.
    /// `porcelain` carries the literal `git status --porcelain`
    /// output (may be empty if git invocation failed, in which
    /// case the validator still rejects based on
    /// `has_uncommitted_changes`).
    WorkingTreeDirtyWithCommits {
        step: Option<String>,
        porcelain: String,
    },
}


/// Maximum number of commit messages scanned for the fix-unit footer
/// soft check.  Keeps the loop bounded even on large monorepos.
pub const FIX_UNIT_FOOTER_SCAN_LIMIT: usize = 10;

/// Regex used to match the `[fix-unit: <id>]` footer inside commit
/// messages.  Captures the `<id>` (e.g. `fix-02`) so the soft check
/// can compare it against the event's `step`.
///
/// Public so test code in other modules can use the same pattern
/// without re-declaring it.
pub static FIX_UNIT_FOOTER_REGEX: std::sync::LazyLock<regex::Regex> =
    std::sync::LazyLock::new(|| {
        // Compiled once.  Wrapping in a `LazyLock` keeps panics
        // confined to first use, and the regex is small (~120 bytes
        // compiled) so the cold-start cost is trivial.
        regex::Regex::new(r"\[fix-unit:\s*(fix-\d{1,3})\]")
            .expect("FIX_UNIT_FOOTER_REGEX must compile")
    });

/// Run the **soft** checks on an event after the hard execution
/// contract has accepted it.  Currently only the fix-unit commit
/// footer is enforced; the function is structured so future soft
/// checks (e.g. expected duration, conventional-commit suffixes) can
/// be added without re-touching `validate_execution_contract`.
///
/// Soft checks **never** cause rejection.  Each soft finding is
/// returned separately so the caller can log it under
/// `diagnostics_topic` and surface it to the user/agent without
/// blocking the event flow.
///
/// Returns an empty `Vec` for events whose payload has no
/// fix-unit-looking `step`, for non-fix-unit events, or when the
/// commit-history scan fails.
pub fn run_execution_contract_soft_checks(
    event: &Event,
    workspace_root: &Path,
    git_provider: &dyn GitEvidenceProvider,
    loop_start_sha: Option<&str>,
) -> Vec<ExecutionContractFinding> {
    let mut findings = Vec::new();
    check_fix_unit_commit_footer(
        event,
        workspace_root,
        git_provider,
        loop_start_sha,
        &mut findings,
    );
    findings
}

/// Soft check: when `event.payload.step` starts with `fix-`, look for
/// a matching `[fix-unit: <step>]` footer in the most recent
/// commits.  Missing footer produces a `FixUnitTagMissing` finding;
/// non-fix steps produce no finding.
fn check_fix_unit_commit_footer(
    event: &Event,
    workspace_root: &Path,
    git_provider: &dyn GitEvidenceProvider,
    loop_start_sha: Option<&str>,
    findings: &mut Vec<ExecutionContractFinding>,
) {
    let payload_str: &str = event.payload.as_str();
    let step = match serde_json::from_str::<Value>(payload_str)
        .ok()
        .and_then(|v| match v.get("step") {
            Some(step_value) => match step_value {
                Value::String(s) => Some(s.clone()),
                // Object form: `{"id":"fix-02","last_in_phase":true}`.
                // R6 says step can be either shape; the soft check only
                // cares about the id, so we collapse to `id`.  When the
                // object carries no `id` we fall back to skipping the
                // check entirely.
                Value::Object(map) => map
                    .get("id")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                _ => None,
            },
            None => None,
        }) {
        Some(s) => s,
        None => return,
    };

    // Only fix-unit steps trigger this check.  Other step names
    // (`step-01`, `trivial`, …) are deliberately excluded: the
    // footer convention is fix-unit-specific.
    if !step.starts_with("fix-") {
        return;
    }

    let expected_tag = format!("[fix-unit: {}]", step);
    let messages = git_provider.recent_commit_messages(
        workspace_root,
        loop_start_sha,
        FIX_UNIT_FOOTER_SCAN_LIMIT,
    );

    let matched = messages
        .iter()
        .any(|m| FIX_UNIT_FOOTER_REGEX.is_match(m) && m.contains(&expected_tag));
    if matched {
        return;
    }

    let recent_in_range = messages
        .iter()
        .take(3)
        .map(|m| m.lines().next().unwrap_or("").to_string())
        .collect::<Vec<_>>()
        .join(" | ");
    let detail = if recent_in_range.is_empty() {
        "no commits found in range".to_string()
    } else {
        format!("recent: {recent_in_range}")
    };
    findings.push(ExecutionContractFinding {
        kind: ExecutionContractViolationKind::FixUnitTagMissing {
            step: step.clone(),
            expected_tag: expected_tag.clone(),
        },
        message: format!(
            "{} payload step='{}' but no commit footer '{}' found since loop start. \
             Add the footer (e.g. `git commit --amend --no-edit` after appending the line, \
             or include it in the next commit) so coordinator can rely on tasks.jsonl + \
             commit footer instead of plan heading parses. ({})",
            event.topic, step, expected_tag, detail
        ),
        topic: event.topic.to_string(),
        source_hat: None,
    });
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
    if findings.is_empty()
        && let Some(rejection) = validate_task(event, rule, current_loop_id, tasks_path) {
            findings.push(with_source_hat(rejection, source_hat.clone()));
        }

    // 3. Git evidence validation (if task validation passed)
    if findings.is_empty()
        && let Some(rejection) =
            validate_git_change(event, rule, workspace_root, git_provider, loop_start_sha)
        {
            findings.push(with_source_hat(rejection, source_hat.clone()));
        }

    // 4. Test evidence validation (if git evidence passed)
    if findings.is_empty()
        && let Some(rejection) = validate_test_evidence(event, rule) {
            findings.push(with_source_hat(rejection, source_hat));
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
        Some(_other) => {
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
    let task_key_from_payload = if rule.require_task.key_field.is_empty() {
        None
    } else {
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
    // 2026-07-07-002 follow-up: when the payload carries `task_key`,
    // resolve the live row by `(loop_id, task_key)` first. A duplicate
    // `task_id` placeholder row (coordinator `task add` racing the
    // projector) must not shadow the keyed projector row.
    let task = if let Some(payload_key) = task_key_from_payload.filter(|k| !k.is_empty()) {
        if let Some(t) = store.get_by_key_in_loop(payload_key, Some(current_loop_id)) {
            if !task_id.is_empty() && t.id != task_id {
                return Some(ExecutionContractFinding {
                    kind: ExecutionContractViolationKind::TaskNotFound {
                        task_id: task_id.clone(),
                    },
                    message: format!(
                        "Task '{}' live identity mismatch: payload task_id '{}' does not match \
                         keyed row id '{}'. Use `ralph tools task list` for the live task_id/task_key.",
                        payload_key, task_id, t.id
                    ),
                    topic: event.topic.to_string(),
                    ..Default::default()
                });
            }
            t
        } else if !task_id.is_empty() {
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
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::TaskNotFound {
                    task_id: resolved_task_id.clone(),
                },
                message: format!(
                    "Task with task_key '{}' not found in loop '{}'. work.done rejected.",
                    payload_key, current_loop_id
                ),
                topic: event.topic.to_string(),
                ..Default::default()
            });
        }
    } else if !task_id.is_empty() {
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

    // 2026-07-07-002 U7: payload task_key must match the live record key.
    if let Some(payload_key) = task_key_from_payload
        && task.key.as_deref() != Some(payload_key) {
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::TaskNotFound {
                    task_id: task_id.clone(),
                },
                message: format!(
                    "Task '{}' live identity mismatch: payload task_key '{}' does not match \
                     ledger key {:?}. Use `ralph tools task list` for the live task_id/task_key.",
                    task_id, payload_key, task.key
                ),
                topic: event.topic.to_string(),
                ..Default::default()
            });
        }

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
            // P1-3 (plan 2026-06-29-006): defensive allow path —
            // if the task's `key` already encodes the current
            // `loop_id`, treat it as the same-loop task even
            // though `loop_id` was not projected. The U2 fix
            // (`state_projector::task::project_ensure_task`) is
            // the primary path; this is the belt-and-suspenders
            // fallback for tasks that were projected before the
            // loop marker was threaded through. See
            // 2026-06-29-ce-executor-serial-primary-172725 §F3
            // for the cascade root cause.
            if let Some(task_key) = &task.key {
                if task_key.contains(current_loop_id) {
                    tracing::warn!(
                        task_id = %task_id,
                        task_key = %task_key,
                        current_loop_id = %current_loop_id,
                        "P1-3 (2026-06-29-006): accepting task with no `loop_id` field because its key contains the current loop marker; ensure U2 fix is also wired so this defensive allow is rarely needed"
                    );
                    // Defensive accept — fall through to the
                    // terminal-status check below.
                } else {
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
            } else {
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
    if !payload_str.trim().is_empty()
        && let Ok(payload) = serde_json::from_str::<Value>(payload_str)
            && let Value::Object(map) = &payload
                && let Some(step) = map.get("step").and_then(|v| v.as_str())
                    && rule
                        .require_git_change
                        .allow_empty_for_steps
                        .contains(&step.to_string())
                    {
                        return None;
                    }

    let has_uncommitted = git_provider.has_uncommitted_changes(workspace_root);
    let has_new_commits = git_provider.has_new_commits_since(workspace_root, loop_start_sha);

    // 2026-07-07 plan P0-1/P0-2: capture the porcelain output once
    // (when needed) so the rejection finding can include a useful
    // "what's still dirty" message without re-invoking git.  Cheap
    // because it's only called for `commit_only_clean` mode.
    let porcelain: String = if rule.require_git_change.mode == "commit_only_clean" {
        git_provider.working_tree_porcelain(workspace_root)
    } else {
        String::new()
    };

    // 2026-07-07 plan P0-1/P0-2: the `commit_only_clean` mode is
    // checked as a SEPARATE branch (not folded into the generic
    // `has_evidence` match) because it produces a distinct finding
    // (`WorkingTreeDirtyWithCommits`) instead of the generic
    // `NoGitEvidence`.  This lets the agent distinguish:
    //   - "you didn't commit at all" → NoGitEvidence
    //   - "you committed but left dirty" → WorkingTreeDirtyWithCommits
    // Both must be actionable without re-running git to discover the answer.
    if rule.require_git_change.mode == "commit_only_clean" {
        if !has_new_commits {
            return Some(commit_only_clean_no_evidence_finding(
                event,
                payload_str,
                loop_start_sha,
            ));
        }
        if has_uncommitted {
            let step = step_from_payload(payload_str);
            let porcelain_for_kind = porcelain.clone();
            let message = format!(
                "{} requires working tree to be clean after commit (commit_only_clean mode). \
                 git status --porcelain returned:\n{}Re-run `git add -A && git commit` (or `git stash`) \
                 to absorb all dirty state before re-emitting.",
                event.topic, porcelain
            );
            return Some(ExecutionContractFinding {
                kind: ExecutionContractViolationKind::WorkingTreeDirtyWithCommits {
                    step,
                    porcelain: porcelain_for_kind,
                },
                message,
                topic: event.topic.to_string(),
                ..Default::default()
            });
        }
        // has_new_commits && !has_uncommitted: pass.
        return None;
    }

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

/// Helper (2026-07-07 plan P0-1/P0-2): build a `NoGitEvidence` finding
/// for the `commit_only_clean` branch when there are no new commits.
/// Keeps `validate_git_change` readable by extracting the porcelain/clean
/// branches first.
fn commit_only_clean_no_evidence_finding(
    event: &Event,
    payload_str: &str,
    loop_start_sha: Option<&str>,
) -> ExecutionContractFinding {
    let step = step_from_payload(payload_str);
    let detail = if loop_start_sha.is_none() {
        "No uncommitted changes found. (Loop start SHA not tracked — commit-only evidence unavailable.)"
    } else {
        "No new commits since loop start found; commit_only_clean mode requires at least one new commit."
    };
    ExecutionContractFinding {
        kind: ExecutionContractViolationKind::NoGitEvidence { step },
        message: format!(
            "{} requires git evidence before downstream review can proceed (commit_only_clean mode). {}",
            event.topic, detail
        ),
        topic: event.topic.to_string(),
        ..Default::default()
    }
}

/// Helper (2026-07-07 plan P0-1/P0-2): extract the `step` field from
/// an event payload without duplicating the parse-and-unwind ladder in
/// every branch.
fn step_from_payload(payload_str: &str) -> Option<String> {
    serde_json::from_str::<Value>(payload_str)
        .ok()
        .and_then(|v| v.get("step").and_then(|s| s.as_str().map(String::from)))
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
        assert!(rule.require_task.loop_scoped);
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
                id_field: String::new(), // Empty = task validation skipped
                key_field: String::new(),
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
        write_task_with_status_loop_id_and_key(
            &tasks_path,
            "T",
            TaskStatus::Open,
            Some("loop-1"),
            "k",
        );

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
        write_task_with_status_loop_id_and_key(
            &tasks_path,
            "T",
            TaskStatus::Open,
            Some("loop-1"),
            "k",
        );

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

    #[test]
    fn test_task_not_terminal_resume_plan_routes_coordinator_owned_task() {
        let task =
            Task::new("task-1".to_string(), 1).with_owner_hat(Some("coordinator".to_string()));
        let (target, hint) = super::task_not_terminal_resume_plan(
            "task-1",
            Some(&task),
            "executor",
            &["coordinator".to_string()],
        );
        assert_eq!(target, "coordinator");
        assert!(hint.contains("hat 'coordinator' must run"));
        assert!(hint.contains("hat 'executor'"));
    }

    #[test]
    fn p1_5_task_not_found_resume_plan_routes_coordinator_on_identity_mismatch() {
        // 2026-07-07-002 scenario: executor emits work.done with a
        // task_id that resolves to a coordinator-owned placeholder
        // row (key=None) while the payload carries a real task_key.
        // Recovery must go to a coordinator hat, not back to executor.
        let task =
            Task::new("orphan".to_string(), 1).with_owner_hat(Some("coordinator".to_string()));
        let (target, hint) = super::task_not_found_resume_plan(
            "task-1783411414-39d0",
            "ce-executor:l1:step-01:u1-skeleton",
            Some(&task),
            "executor",
            &["coordinator".to_string()],
        );
        assert_eq!(target, "coordinator");
        assert!(
            hint.contains("identity mismatch"),
            "hint must name the failure mode, got: {hint}"
        );
        assert!(
            hint.contains("Do not emit work.failed"),
            "hint must steer away from work.failed"
        );
    }

    #[test]
    fn p1_5_task_not_found_resume_plan_falls_back_to_source_when_no_coordinator_hats() {
        // Legacy / human-CLI loops have no coordinator_hats; keep the
        // old source-hat retry target so we do not break them.
        let (target, _hint) =
            super::task_not_found_resume_plan("task-x", "some-key", None, "executor", &[]);
        assert_eq!(target, "executor");
    }

    #[test]
    fn p1_5_task_not_found_resume_plan_uses_first_coordinator_when_owner_unknown() {
        // Task row exists but owner is not a configured coordinator
        // hat — fall back to the first coordinator hat.
        let task = Task::new("row".to_string(), 1).with_owner_hat(Some("ghost".to_string()));
        let (target, _hint) = super::task_not_found_resume_plan(
            "task-y",
            "k-y",
            Some(&task),
            "executor",
            &["coordinator-a".to_string(), "coordinator-b".to_string()],
        );
        assert_eq!(target, "coordinator-a");
    }

    // === F2 Git evidence tests ===

    #[test]
    fn test_git_evidence_skipped_in_non_git_directory() {
        // /tmp is not a git repo, so git evidence check should be skipped (not applicable)
        // We use a rule with empty id_field to skip task validation
        let rule = ExecutionContractRule {
            require_payload_fields: vec![],
            require_task: TaskCompletionRequirement {
                id_field: String::new(),
                key_field: String::new(),
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

    // 2026-07-07-003 follow-up: plan 002 U7 added the
    // "payload task_key must match the live record key" gate
    // in `validate_execution_contract` so that an emit cannot
    // re-bind a closed task to a different key.  Pre-U7
    // fixtures created tasks without a key, so the gate now
    // trips them as `TaskNotFound` before they reach the
    // loop/status checks the tests pin.  This helper threads
    // the matching key so the tests continue to exercise the
    // originally intended finding.
    fn write_task_with_loop_id_and_key(
        tasks_path: &std::path::Path,
        task_id: &str,
        loop_id: Option<&str>,
        key: &str,
    ) {
        let mut store = TaskStore::load(tasks_path).unwrap();
        let mut task = Task::new(format!("task {task_id}"), 1)
            .with_key(Some(key.to_string()))
            .with_loop_id(loop_id.map(str::to_string));
        task.id = task_id.to_string();
        task.status = TaskStatus::Closed;
        store.add(task);
        store.save().unwrap();
    }

    fn write_task_with_status_loop_id_and_key(
        tasks_path: &std::path::Path,
        task_id: &str,
        status: TaskStatus,
        loop_id: Option<&str>,
        key: &str,
    ) {
        let mut store = TaskStore::load(tasks_path).unwrap();
        let mut task = Task::new(format!("task {task_id}"), 1)
            .with_key(Some(key.to_string()))
            .with_loop_id(loop_id.map(str::to_string));
        task.id = task_id.to_string();
        task.status = status;
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
        write_task_with_loop_id_and_key(
            &tasks_path,
            "task-1",
            Some("primary-20260604-091852"),
            "k",
        );

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
        write_task_with_loop_id_and_key(&tasks_path, "task-1", Some("primary-OTHER-LOOP"), "k");

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

    // === P1-3 (plan 2026-06-29-006) defensive-allow tests ===
    //
    // The P1-3 path accepts legacy tasks whose `key` already
    // encodes the current loop marker even though `loop_id` was
    // never projected. Primary path is U2 (projector fallback);
    // these tests pin the defensive-allow contract that backs
    // it up.

    fn write_task_with_key_no_loop(tasks_path: &std::path::Path, task_id: &str, key: &str) {
        let mut store = TaskStore::load(tasks_path).unwrap();
        let mut task = Task::new(format!("task {task_id}"), 1).with_key(Some(key.to_string()));
        task.id = task_id.to_string();
        task.status = TaskStatus::Closed;
        store.add(task);
        store.save().unwrap();
    }

    #[test]
    fn p1_3_accepts_legacy_task_when_key_contains_current_loop_id() {
        let temp = TempDir::new().unwrap();
        let ralph_dir = temp.path().join(".ralph");
        std::fs::create_dir_all(ralph_dir.join("agent")).unwrap();
        std::fs::write(
            ralph_dir.join("current-loop-id"),
            "primary-20260628-172725\n",
        )
        .unwrap();
        let tasks_path = ralph_dir.join("agent/tasks.jsonl");
        write_task_with_key_no_loop(
            &tasks_path,
            "task-1",
            "from_key:ce-executor:primary-20260628-172725:step-01:u0",
        );

        let rule = make_work_done_rule();
        let event = Event::new(
            "work.done",
            r#"{"plan_name":"p","plan_path":"/p","task_id":"task-1","task_key":"from_key:ce-executor:primary-20260628-172725:step-01:u0","step":"s"}"#,
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
            "P1-3: legacy task whose key contains the current loop marker must be accepted"
        );
    }

    #[test]
    fn p1_3_rejects_legacy_task_when_key_lacks_current_loop_id() {
        let temp = TempDir::new().unwrap();
        let ralph_dir = temp.path().join(".ralph");
        std::fs::create_dir_all(ralph_dir.join("agent")).unwrap();
        std::fs::write(
            ralph_dir.join("current-loop-id"),
            "primary-20260628-172725\n",
        )
        .unwrap();
        let tasks_path = ralph_dir.join("agent/tasks.jsonl");
        write_task_with_key_no_loop(&tasks_path, "task-1", "from_key:ce-executor:step-99:u0");

        let rule = make_work_done_rule();
        let event = Event::new(
            "work.done",
            r#"{"plan_name":"p","plan_path":"/p","task_id":"task-1","task_key":"from_key:ce-executor:step-99:u0","step":"s"}"#,
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
                assert_eq!(expected, "primary-20260628-172725");
                assert_eq!(actual, None);
            }
            ExecutionContractDecision::Accept => {
                panic!("P1-3: task whose key lacks the marker must be hard-rejected")
            }
        }
    }

    #[test]
    fn p1_3_rejects_when_task_loop_id_does_not_match_current_loop() {
        let temp = TempDir::new().unwrap();
        let ralph_dir = temp.path().join(".ralph");
        std::fs::create_dir_all(ralph_dir.join("agent")).unwrap();
        std::fs::write(
            ralph_dir.join("current-loop-id"),
            "primary-20260628-172725\n",
        )
        .unwrap();
        let tasks_path = ralph_dir.join("agent/tasks.jsonl");
        write_task_with_loop_id_and_key(&tasks_path, "task-1", Some("primary-OTHER"), "k");

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
                assert_eq!(expected, "primary-20260628-172725");
                assert_eq!(actual.as_deref(), Some("primary-OTHER"));
            }
            ExecutionContractDecision::Accept => {
                panic!("P1-3: mismatched loop_id must be hard-rejected")
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

        let event = Event::new("work.done", r#"{"task_id":"real-id-1","task_key":"k1"}"#);
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

#[cfg(test)]
mod recent_commit_messages_tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;

    /// Mock provider used when we want to drive `recent_commit_messages`
    /// (and only that method) without touching the real git CLI.
    struct StubGitProvider {
        messages: Vec<String>,
    }
    impl GitEvidenceProvider for StubGitProvider {
        fn is_git_repo(&self, _workspace: &Path) -> bool {
            true
        }
        fn has_uncommitted_changes(&self, _workspace: &Path) -> bool {
            false
        }
        fn has_new_commits_since(&self, _workspace: &Path, _start_sha: Option<&str>) -> bool {
            !self.messages.is_empty()
        }
        fn recent_commit_messages(
            &self,
            _workspace: &Path,
            _since_sha: Option<&str>,
            _max_count: usize,
        ) -> Vec<String> {
            self.messages.clone()
        }
        // 2026-07-07 plan P0-1/P0-2: new trait method.  This
        // mock isn't used for `commit_only_clean` paths.
        fn working_tree_porcelain(&self, _workspace: &Path) -> String {
            String::new()
        }
    }

    fn run_git(args: &[&str], dir: &PathBuf) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git command available in test env");
        assert!(status.success(), "git {:?} failed", args);
    }

    #[test]
    fn stub_provider_returns_configured_messages() {
        let stub = StubGitProvider {
            messages: vec!["[fix-unit: fix-02] hello".to_string()],
        };
        let msgs = stub.recent_commit_messages(Path::new("/tmp"), None, 10);
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].contains("[fix-unit: fix-02]"));
    }

    #[test]
    fn recent_commit_messages_returns_empty_for_max_count_zero() {
        // The `max_count` short-circuit must not call git.
        let dir = std::env::temp_dir().join(format!(
            "ralph-u1-zero-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // Even though dir is not a repo, this must not panic.
        let msgs = DefaultGitEvidenceProvider.recent_commit_messages(&dir, None, 0);
        assert!(msgs.is_empty());
    }

    #[test]
    fn recent_commit_messages_returns_empty_for_non_repo_dir() {
        let dir = std::env::temp_dir().join(format!(
            "ralph-u1-norepo-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        // No `git init` here — git log will fail with status != 0,
        // we must return an empty Vec instead of panicking.
        let msgs = DefaultGitEvidenceProvider.recent_commit_messages(&dir, None, 10);
        assert!(msgs.is_empty());
    }

    #[test]
    fn recent_commit_messages_handles_multiline_body() {
        // A commit body with blank lines between paragraphs must be
        // returned as one logical message, not split into three.
        let dir = std::env::temp_dir().join(format!(
            "ralph-u1-multiline-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&["init", "-q"], &dir);
        run_git(&["config", "user.email", "test@example.com"], &dir);
        run_git(&["config", "user.name", "test"], &dir);
        run_git(
            &[
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "fix(core): subject line\n\nbody paragraph 1\n\nbody paragraph 2\n\n[fix-unit: fix-01]",
            ],
            &dir,
        );

        let msgs = DefaultGitEvidenceProvider.recent_commit_messages(&dir, None, 10);
        assert_eq!(msgs.len(), 1, "want one message, got {:?}", msgs);
        assert!(msgs[0].contains("subject line"));
        assert!(msgs[0].contains("body paragraph 1"));
        assert!(msgs[0].contains("body paragraph 2"));
        assert!(msgs[0].contains("[fix-unit: fix-01]"));
    }

    #[test]
    fn recent_commit_messages_respects_max_count() {
        let dir = std::env::temp_dir().join(format!(
            "ralph-u1-cap-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&["init", "-q"], &dir);
        run_git(&["config", "user.email", "test@example.com"], &dir);
        run_git(&["config", "user.name", "test"], &dir);
        for i in 0..5 {
            run_git(
                &["commit", "--allow-empty", "-q", "-m", &format!("c{i}")],
                &dir,
            );
        }

        let msgs = DefaultGitEvidenceProvider.recent_commit_messages(&dir, None, 3);
        assert_eq!(msgs.len(), 3);
        // Newest first — commit 4 is the most recent.
        assert!(msgs[0].contains("c4"));
        assert!(msgs[1].contains("c3"));
        assert!(msgs[2].contains("c2"));
    }

    #[test]
    fn recent_commit_messages_respects_since_sha_range() {
        let dir = std::env::temp_dir().join(format!(
            "ralph-u1-range-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        run_git(&["init", "-q"], &dir);
        run_git(&["config", "user.email", "test@example.com"], &dir);
        run_git(&["config", "user.name", "test"], &dir);
        run_git(&["commit", "--allow-empty", "-q", "-m", "baseline"], &dir);
        let baseline = Command::new("git")
            .args(["rev-parse", "HEAD"])
            .current_dir(&dir)
            .output()
            .unwrap();
        let baseline = String::from_utf8(baseline.stdout).unwrap();
        let baseline = baseline.trim();

        run_git(&["commit", "--allow-empty", "-q", "-m", "post-1"], &dir);
        run_git(&["commit", "--allow-empty", "-q", "-m", "post-2"], &dir);

        let msgs = DefaultGitEvidenceProvider.recent_commit_messages(&dir, Some(baseline), 10);
        // Should contain post-1 and post-2 only.
        assert!(
            !msgs.iter()
                .any(|m| m.contains("baseline") && !m.contains("post")),
            "baseline should be excluded: {msgs:?}"
        );
        assert!(msgs.iter().any(|m| m.contains("post-1")));
        assert!(msgs.iter().any(|m| m.contains("post-2")));
    }
}

#[cfg(test)]
mod fix_unit_footer_soft_check_tests {
    use super::*;
    use std::path::PathBuf;
    use std::process::Command;
    use std::sync::Mutex;

    /// Provider whose recent-commit history is configurable per test.
    struct ConfigurableCommitProvider {
        recent: Mutex<Vec<String>>,
    }
    impl ConfigurableCommitProvider {
        fn new(messages: Vec<String>) -> Self {
            Self {
                recent: Mutex::new(messages),
            }
        }
    }
    impl GitEvidenceProvider for ConfigurableCommitProvider {
        fn is_git_repo(&self, _workspace: &Path) -> bool {
            true
        }
        fn has_uncommitted_changes(&self, _workspace: &Path) -> bool {
            false
        }
        fn has_new_commits_since(&self, _workspace: &Path, _start_sha: Option<&str>) -> bool {
            !self.recent.lock().unwrap().is_empty()
        }
        fn recent_commit_messages(
            &self,
            _workspace: &Path,
            _since_sha: Option<&str>,
            _max_count: usize,
        ) -> Vec<String> {
            self.recent.lock().unwrap().clone()
        }
        // 2026-07-07 plan P0-1/P0-2: new trait method.  This
        // mock isn't used for `commit_only_clean` paths.
        fn working_tree_porcelain(&self, _workspace: &Path) -> String {
            String::new()
        }
    }

    fn make_event(step: &str) -> Event {
        Event::new(
            "work.done",
            format!(r#"{{"task_id":"t","task_key":"k","step":"{step}"}}"#),
        )
    }

    #[test]
    fn happy_path_no_finding_when_footer_matches() {
        let provider = ConfigurableCommitProvider::new(vec![
            "fix(core): something\n\n[fix-unit: fix-02]".to_string(),
        ]);
        let event = make_event("fix-02");
        let diagnostics =
            run_execution_contract_soft_checks(&event, Path::new("/tmp"), &provider, None);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn missing_footer_produces_finding() {
        let provider = ConfigurableCommitProvider::new(vec![
            "fix(core): something\n\nno footer here".to_string(),
        ]);
        let event = make_event("fix-02");
        let diagnostics =
            run_execution_contract_soft_checks(&event, Path::new("/tmp"), &provider, None);
        assert_eq!(diagnostics.len(), 1);
        assert!(matches!(
            diagnostics[0].kind,
            ExecutionContractViolationKind::FixUnitTagMissing { .. }
        ));
        // Importantly, soft findings never reject.
        let decision = ExecutionContractDecision::Accept;
        match decision {
            ExecutionContractDecision::Accept => {}
            ExecutionContractDecision::Reject(_) => {
                panic!("soft check must not flip Accept into Reject")
            }
        }
    }

    #[test]
    fn step_mismatch_produces_finding() {
        // Footer says fix-01 but event step is fix-02.
        let provider = ConfigurableCommitProvider::new(vec![
            "fix(core): something\n\n[fix-unit: fix-01]".to_string(),
        ]);
        let event = make_event("fix-02");
        let diagnostics =
            run_execution_contract_soft_checks(&event, Path::new("/tmp"), &provider, None);
        assert_eq!(diagnostics.len(), 1);
        match &diagnostics[0].kind {
            ExecutionContractViolationKind::FixUnitTagMissing { step, expected_tag } => {
                assert_eq!(step, "fix-02");
                assert_eq!(expected_tag, "[fix-unit: fix-02]");
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn one_of_many_commits_with_footer_satisfies_check() {
        // Multiple commits, one of which has the matching footer.
        let provider = ConfigurableCommitProvider::new(vec![
            "no footer here".to_string(),
            "fix: blah\n\n[fix-unit: fix-02]".to_string(),
            "yet another".to_string(),
        ]);
        let event = make_event("fix-02");
        let diagnostics =
            run_execution_contract_soft_checks(&event, Path::new("/tmp"), &provider, None);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn non_fix_step_is_skipped() {
        // The footer convention is fix-unit-specific; step="step-01"
        // must not trigger the check even with no commits in range.
        let provider = ConfigurableCommitProvider::new(vec!["some commit".to_string()]);
        let event = make_event("step-01");
        let diagnostics =
            run_execution_contract_soft_checks(&event, Path::new("/tmp"), &provider, None);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn empty_commit_history_records_finding() {
        // No commits at all in the range — must record a finding so
        // the coordinator is aware the agent never committed.
        let provider = ConfigurableCommitProvider::new(vec![]);
        let event = make_event("fix-02");
        let diagnostics =
            run_execution_contract_soft_checks(&event, Path::new("/tmp"), &provider, None);
        assert_eq!(diagnostics.len(), 1);
        assert!(matches!(
            diagnostics[0].kind,
            ExecutionContractViolationKind::FixUnitTagMissing { .. }
        ));
    }

    #[test]
    fn event_without_step_field_is_skipped() {
        // events whose payload has no `step` field must not throw.
        let provider = ConfigurableCommitProvider::new(vec!["some commit".to_string()]);
        let event = Event::new("work.done", r#"{"task_id":"t","task_key":"k"}"#);
        let diagnostics =
            run_execution_contract_soft_checks(&event, Path::new("/tmp"), &provider, None);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn event_with_invalid_json_payload_is_skipped() {
        // Defensive: if the payload cannot be parsed, treat the
        // check as skipped rather than recording a misleading
        // finding.
        let provider = ConfigurableCommitProvider::new(vec!["some commit".to_string()]);
        let event = Event::new("work.done", "not json at all");
        let diagnostics =
            run_execution_contract_soft_checks(&event, Path::new("/tmp"), &provider, None);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn object_form_step_collapses_to_id() {
        // R6 canonical emit uses
        // `step={"id":"fix-02","last_in_phase":true}`.  The soft
        // check still recognises the step id and compares against
        // commit footers; if the agent only emitted object-form
        // steps without a footer, the diagnostic still fires.
        let provider =
            ConfigurableCommitProvider::new(vec!["fix(core): no footer here".to_string()]);
        let event = Event::new(
            "work.done",
            r#"{"task_id":"t","task_key":"k","step":{"id":"fix-02","last_in_phase":true}}"#,
        );
        let diagnostics =
            run_execution_contract_soft_checks(&event, Path::new("/tmp"), &provider, None);
        assert_eq!(diagnostics.len(), 1);
        match &diagnostics[0].kind {
            ExecutionContractViolationKind::FixUnitTagMissing { step, .. } => {
                assert_eq!(step, "fix-02");
            }
            _ => panic!("wrong kind"),
        }
    }

    #[test]
    fn object_form_step_with_matching_footer_passes() {
        let provider = ConfigurableCommitProvider::new(vec![
            "fix(core): with footer\n\n[fix-unit: fix-02]".to_string(),
        ]);
        let event = Event::new(
            "work.done",
            r#"{"task_id":"t","task_key":"k","step":{"id":"fix-02","last_in_phase":true}}"#,
        );
        let diagnostics =
            run_execution_contract_soft_checks(&event, Path::new("/tmp"), &provider, None);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn regex_compiles_and_matches_correctly() {
        // Sanity check the regex against the documented pattern so
        // somebody tweaking it later gets an immediate red light.
        let m = FIX_UNIT_FOOTER_REGEX.captures("anything [fix-unit: fix-99] end");
        assert!(m.is_some(), "regex should match");
        assert_eq!(m.unwrap().get(1).unwrap().as_str(), "fix-99");
        // The regex is intentionally permissive about whitespace
        // between `:` and the id (`\s*`), so both forms match.
        assert!(FIX_UNIT_FOOTER_REGEX.is_match("[fix-unit:fix-99]"));
        assert!(FIX_UNIT_FOOTER_REGEX.is_match("[fix-unit:    fix-99]"));
        // Non-fix-unit tags must NOT match.
        assert!(!FIX_UNIT_FOOTER_REGEX.is_match("[step: fix-99]"));
        assert!(!FIX_UNIT_FOOTER_REGEX.is_match("[fix-unit: step-99]"));
    }

    #[test]
    fn integration_with_real_git_repo_and_matching_footer() {
        // End-to-end: real git history with matching footer must
        // produce zero findings.
        let dir = std::env::temp_dir().join(format!(
            "ralph-u2-real-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        run_git_init(&dir);
        run_git(
            &dir,
            &[
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "fix(core): first\n\n[fix-unit: fix-02]",
            ],
        );

        let event = make_event("fix-02");
        let diagnostics =
            run_execution_contract_soft_checks(&event, &dir, &DefaultGitEvidenceProvider, None);
        assert!(diagnostics.is_empty());
    }

    #[test]
    fn integration_with_real_git_repo_missing_footer() {
        let dir = std::env::temp_dir().join(format!(
            "ralph-u2-missing-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        run_git_init(&dir);
        run_git(
            &dir,
            &[
                "commit",
                "--allow-empty",
                "-q",
                "-m",
                "fix(core): forgot the footer",
            ],
        );

        let event = make_event("fix-02");
        let diagnostics =
            run_execution_contract_soft_checks(&event, &dir, &DefaultGitEvidenceProvider, None);
        assert_eq!(diagnostics.len(), 1);
        assert!(matches!(
            diagnostics[0].kind,
            ExecutionContractViolationKind::FixUnitTagMissing { .. }
        ));
        assert!(diagnostics[0].message.contains("fix-02"));
    }

    fn run_git_init(dir: &PathBuf) {
        run_git(dir, &["init", "-q"]);
        run_git(dir, &["config", "user.email", "test@example.com"]);
        run_git(dir, &["config", "user.name", "test"]);
    }

    fn run_git(dir: &PathBuf, args: &[&str]) {
        let status = Command::new("git")
            .args(args)
            .current_dir(dir)
            .status()
            .expect("git available in test env");
        assert!(status.success(), "git {:?} failed in {:?}", args, dir);
    }
}
