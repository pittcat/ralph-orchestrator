//! Progress ↔ Task hard gate — pre-handoff consistency check.
//!
//! Plan Unit: U4 of `2026-06-17-002-feat-ce-executor-step-handoff-plan.md`.
//!
//! The gate enforces that `.ralph/agent/progress.md` (the human-facing
//! progress ledger) is consistent with `.ralph/agent/tasks.jsonl` (the
//! machine-facing task ledger) **before** a `queue.advance` or
//! `plan.complete` event is admitted.
//!
//! Rules (per plan L262-263):
//! 1. If the referenced task is `closed` but progress does not mark it
//!    as completed → mismatch (`task_closed_but_progress_missing`).
//! 2. If the event's `step` conflicts with progress's `Current Step` →
//!    mismatch (`step_mismatch`).
//!
//! The function is deliberately narrow:
//! - It only parses two narrow markdown fields (`Current Step` and
//!   `Completed Steps`); full markdown parsing is out of scope.
//! - It only reads two files; no env vars or runtime state.
//! - It is **fail-closed**: missing progress.md, missing task, missing
//!   fields → `Err`. Callers should never silently skip the check.

use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::task::{Task, TaskStatus};
use crate::task_store::TaskStore;

/// Topics this gate inspects. Other topics are never checked.
pub const GATED_TOPICS: &[&str] = &["queue.advance", "plan.complete"];

/// Returns true when the topic is one the gate inspects.
///
/// The gate must be **narrow** by design — only the two topics that
/// mutate the step counter / finalize the plan. Adding more here is a
/// semantic change and must be coordinated with the preset's
/// `publishes` block.
pub fn is_gated_topic(topic: &str) -> bool {
    GATED_TOPICS.contains(&topic)
}

/// Reason the gate rejected an event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgressTaskMismatch {
    /// Stable reason code so downstream tooling can match without
    /// pattern-matching the message string.
    pub reason: String,
    /// Human-readable detail.
    pub detail: String,
    /// Step the gate was asked to validate against (if known).
    pub step: Option<String>,
    /// Task ID the gate was asked to validate (if known).
    pub task_id: Option<String>,
}

impl fmt::Display for ProgressTaskMismatch {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "progress_task_gate: {} (step={:?}, task_id={:?})",
            self.reason, self.step, self.task_id
        )
    }
}

impl std::error::Error for ProgressTaskMismatch {}

/// Narrow view of `.ralph/agent/progress.md`.
///
/// We deliberately parse only the two fields the gate cares about.
/// Full markdown parsing would be brittle and unnecessary: the
/// instruction block for `plan-gate` already locks the format to
/// `## Current Step` / `## Completed Steps`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProgressSnapshot {
    /// Value of `## Current Step` (or `**Current Step**:`) heading.
    pub current_step: Option<String>,
    /// Steps listed under `## Completed Steps`.
    pub completed_steps: Vec<String>,
    /// Set to true when the file existed but no headings were found.
    pub empty_headings: bool,
}

impl ProgressSnapshot {
    /// Parse `progress.md` content.
    ///
    /// We accept two on-disk shapes for compatibility with both the
    /// spec template and the agent's free-form output:
    ///
    /// - Markdown heading: `## Current Step` (followed by value on
    ///   subsequent line, or `## Current Step: foo`).
    /// - Inline bold label: `**Current Step**: foo`.
    pub fn parse(content: &str) -> Self {
        let mut snap = ProgressSnapshot::default();
        let mut current_section: Option<&str> = None;

        for line in content.lines() {
            let trimmed = line.trim();

            // Section headings
            if let Some(rest) = trimmed.strip_prefix("## ") {
                let section = rest.trim();
                if let Some((name, inline_value)) = split_heading(section) {
                    let inline = inline_value.trim();
                    match name {
                        "Current Step" if !inline.is_empty() => {
                            snap.current_step = Some(inline.to_string())
                        }
                        "Completed Steps" => {
                            // Inline value treated as a comma-separated list.
                            for entry in inline.split(',') {
                                let e = entry.trim();
                                if !e.is_empty() {
                                    snap.completed_steps.push(e.to_string());
                                }
                            }
                        }
                        _ => {}
                    }
                    // Once a section is opened, body lines (if any) below
                    // are processed by the body-line arm — for headings
                    // with inline values there are none, but we still set
                    // the section so subsequent lines are scoped to it.
                    current_section = Some(name);
                    continue;
                }
                current_section = Some(section);
                continue;
            }

            // Bold-label inline form (e.g. agent writes **Current Step**: step-02)
            if let Some(value) = strip_bold_label(trimmed, "Current Step") {
                let v = value.trim();
                if !v.is_empty() {
                    snap.current_step = Some(v.to_string());
                }
                continue;
            }
            if let Some(value) = strip_bold_label(trimmed, "Completed Steps") {
                // Bold label without inline value opens the section.
                // Bold label with inline value is a comma-separated list.
                let v = value.trim();
                if v.is_empty() {
                    current_section = Some("Completed Steps");
                } else {
                    for entry in v.split(',') {
                        let e = entry.trim();
                        if !e.is_empty() {
                            snap.completed_steps.push(e.to_string());
                        }
                    }
                }
                continue;
            }

            // Body lines that belong to the Completed Steps section
            if current_section == Some("Completed Steps")
                && let Some(entry) = parse_completed_entry(trimmed)
            {
                snap.completed_steps.push(entry);
            }

            // Body line directly under "## Current Step" — the
            // canonical spec-template form. Only the FIRST body line
            // is consumed; subsequent ones are treated as
            // end-of-section (the next `##` heading will reset the
            // section anyway).
            if current_section == Some("Current Step") && !trimmed.is_empty() {
                snap.current_step = Some(trimmed.to_string());
                current_section = None;
            }
        }

        snap.empty_headings = snap.current_step.is_none() && snap.completed_steps.is_empty();
        snap
    }

    /// Returns true when `step` is listed under "Completed Steps".
    pub fn is_step_completed(&self, step: &str) -> bool {
        let target = step.trim();
        self.completed_steps.iter().any(|s| s.trim() == target)
    }
}

fn split_heading(section: &str) -> Option<(&str, &str)> {
    // `Current Step: step-02` → ("Current Step", "step-02")
    if let Some((name, value)) = section.split_once(':') {
        let name = name.trim();
        if matches!(name, "Current Step" | "Completed Steps") {
            return Some((name, value.trim()));
        }
    }
    None
}

fn strip_bold_label<'a>(line: &'a str, label: &str) -> Option<&'a str> {
    // `**Current Step**: step-02` — value after the closing `**:`.
    let needle = format!("**{label}**:");
    if let Some(rest) = line.strip_prefix(&needle) {
        return Some(rest.trim());
    }
    let needle_alt = format!("**{label}** :");
    if let Some(rest) = line.strip_prefix(&needle_alt) {
        return Some(rest.trim());
    }
    None
}

fn parse_completed_entry(line: &str) -> Option<String> {
    // Accept `- step-02`, `* step-02`, `1. step-02`, or a bare token.
    let stripped: Option<String> = line
        .strip_prefix("- ")
        .or_else(|| line.strip_prefix("* "))
        .map(|s| s.to_string())
        .or_else(|| {
            // Numbered list: `1. step-02`
            let mut chars = line.chars();
            let prefix: String = chars.by_ref().take_while(|c| c.is_ascii_digit()).collect();
            if prefix.is_empty() {
                return None;
            }
            let rest: String = chars.collect();
            rest.strip_prefix(". ").map(|s| s.to_string())
        });
    match stripped {
        Some(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                None
            } else {
                Some(trimmed.to_string())
            }
        }
        None => {
            if line.is_empty() {
                None
            } else {
                Some(line.to_string())
            }
        }
    }
}

/// Effective gate result for a single event check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// Gate is off; topic is not gated; pass through.
    Inert,
    /// Topic is gated and progress.md ↔ tasks.jsonl align; pass through.
    Aligned,
    /// Topic is gated but progress.md ↔ tasks.jsonl disagree; reject.
    Mismatch(ProgressTaskMismatch),
}

/// Check progress.md vs tasks.jsonl alignment for the given step/task.
///
/// Returns `Aligned` when both ledgers agree, `Mismatch` with the
/// reason when they disagree, and `Inert` when the topic is not in
/// [`GATED_TOPICS`] (the caller should skip the gate entirely).
///
/// The function is **pure**: no filesystem side effects, no clock
/// reads, no logging. The caller decides whether to surface the
/// mismatch as `RejectWithResume` and emit `plan.blocked`.
///
/// `step` and `task_id` come from the inbound event's payload. The
/// function is lenient about `None` values: a `None` step means the
/// gate cannot verify step-level alignment and falls back to the
/// task-level check; a `None` task_id skips the closed-but-not-marked
/// check and only verifies the `Current Step` field exists.
pub fn check_progress_task_alignment(
    topic: &str,
    step: Option<&str>,
    task_id: Option<&str>,
    workspace: &Path,
) -> GateDecision {
    if !is_gated_topic(topic) {
        return GateDecision::Inert;
    }

    let progress_path = workspace.join(".ralph").join("agent").join("progress.md");
    let tasks_path = workspace.join(".ralph").join("agent").join("tasks.jsonl");

    // 1. Load progress.md (fail-closed on missing or empty headings).
    let progress = match read_progress(&progress_path) {
        Ok(snap) => snap,
        Err(reason) => {
            return GateDecision::Mismatch(ProgressTaskMismatch {
                reason: reason.to_string(),
                detail: format!("could not read {}", progress_path.display()),
                step: step.map(|s| s.to_string()),
                task_id: task_id.map(|t| t.to_string()),
            });
        }
    };

    if progress.empty_headings {
        return GateDecision::Mismatch(ProgressTaskMismatch {
            reason: "progress_missing_headings".to_string(),
            detail: format!(
                "{} has no 'Current Step' or 'Completed Steps' headings",
                progress_path.display()
            ),
            step: step.map(|s| s.to_string()),
            task_id: task_id.map(|t| t.to_string()),
        });
    }

    // 2. Load tasks.jsonl (fail-closed on read error; not on
    //    "no tasks yet", because that's a legitimate pre-step state).
    let task_store = match TaskStore::load(&tasks_path) {
        Ok(store) => store,
        Err(e) => {
            return GateDecision::Mismatch(ProgressTaskMismatch {
                reason: "tasks_unreadable".to_string(),
                detail: format!("{}: {e}", tasks_path.display()),
                step: step.map(|s| s.to_string()),
                task_id: task_id.map(|t| t.to_string()),
            });
        }
    };

    // 3. If a step is provided, verify it matches progress.Current Step.
    if let Some(step) = step {
        match progress.current_step.as_deref() {
            None => {
                return GateDecision::Mismatch(ProgressTaskMismatch {
                    reason: "progress_missing_current_step".to_string(),
                    detail: format!(
                        "event step='{}' but progress.md has no Current Step heading",
                        step
                    ),
                    step: Some(step.to_string()),
                    task_id: task_id.map(|t| t.to_string()),
                });
            }
            Some(current) if current.trim() != step.trim() => {
                // Allow step == current. If the event advances past
                // current, the gate is permissive: the agent is
                // allowed to advance; progress.md is just a snapshot
                // of what the agent claimed last. The strict check
                // fires only when there is *no overlap at all*.
                if !progress.is_step_completed(step) {
                    return GateDecision::Mismatch(ProgressTaskMismatch {
                        reason: "step_mismatch".to_string(),
                        detail: format!(
                            "event step='{}' but progress Current Step='{}' and '{}' is not in Completed Steps",
                            step, current, step
                        ),
                        step: Some(step.to_string()),
                        task_id: task_id.map(|t| t.to_string()),
                    });
                }
            }
            Some(_) => {}
        }
    }

    // 4. If a task_id is provided and the task exists, verify
    //    closed-task ⇔ completed-step consistency.
    if let Some(task_id) = task_id {
        match task_store.get(task_id) {
            None => {
                return GateDecision::Mismatch(ProgressTaskMismatch {
                    reason: "task_not_found".to_string(),
                    detail: format!(
                        "event references task_id='{}' which is not in {}",
                        task_id,
                        tasks_path.display()
                    ),
                    step: step.map(|s| s.to_string()),
                    task_id: Some(task_id.to_string()),
                });
            }
            Some(task) if is_task_closed(task) => {
                // Task is closed → progress must mark the task's step
                // as completed. Prefer the task's own `title` (its
                // canonical step identity) over the inbound event's
                // `step` field: the agent may be advancing to a new
                // step while closing the previous one, so checking
                // against the event step would falsely mismatch.
                let step_to_check = if task.title.trim().is_empty() {
                    step.map(|s| s.to_string())
                } else {
                    Some(task.title.clone())
                };
                if let Some(ref s) = step_to_check
                    && !progress.is_step_completed(s)
                {
                    return GateDecision::Mismatch(ProgressTaskMismatch {
                        reason: "task_closed_but_progress_missing".to_string(),
                        detail: format!(
                            "task '{}' is closed but step '{}' is not in progress.md Completed Steps",
                            task_id, s
                        ),
                        step: step.map(|st| st.to_string()),
                        task_id: Some(task_id.to_string()),
                    });
                }
            }
            Some(_) => {
                // Task exists but is not closed; nothing to check.
            }
        }
    }

    GateDecision::Aligned
}

fn is_task_closed(task: &Task) -> bool {
    matches!(task.status, TaskStatus::Closed)
}

fn read_progress(path: &Path) -> Result<ProgressSnapshot, &'static str> {
    let content = std::fs::read_to_string(path).map_err(|_| "progress_unreadable")?;
    let snap = ProgressSnapshot::parse(&content);
    Ok(snap)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_file(dir: &Path, rel: &str, content: &str) -> std::path::PathBuf {
        let path = dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, content).unwrap();
        path
    }

    fn workspace() -> tempfile::TempDir {
        let tmp = tempfile::tempdir().unwrap();
        // Touch the .ralph/agent dir so relative paths resolve.
        std::fs::create_dir_all(tmp.path().join(".ralph").join("agent")).unwrap();
        tmp
    }

    fn write_task(tmp: &tempfile::TempDir, id: &str, status: TaskStatus, title: &str) {
        let mut task = Task::new(title.to_string(), 3);
        task.id = id.to_string();
        task.status = status;
        let line = serde_json::to_string(&task).unwrap();
        let path = tmp.path().join(".ralph").join("agent").join("tasks.jsonl");
        let mut existing = std::fs::read_to_string(&path).unwrap_or_default();
        existing.push_str(&line);
        existing.push('\n');
        std::fs::write(&path, existing).unwrap();
    }

    #[test]
    fn happy_path_task_closed_and_progress_marks_step_completed() {
        let tmp = workspace();
        write_task(&tmp, "task-1", TaskStatus::Closed, "step-01");
        write_file(
            tmp.path(),
            ".ralph/agent/progress.md",
            "## Current Step\nstep-02\n\n## Completed Steps\n- step-01\n",
        );

        let decision = check_progress_task_alignment(
            "queue.advance",
            Some("step-02"),
            Some("task-1"),
            tmp.path(),
        );
        assert_eq!(decision, GateDecision::Aligned);
    }

    #[test]
    fn plan_complete_is_also_gated() {
        let tmp = workspace();
        write_task(&tmp, "task-1", TaskStatus::Closed, "step-01");
        write_file(
            tmp.path(),
            ".ralph/agent/progress.md",
            "## Current Step: step-final\n\n## Completed Steps\n- step-01\n",
        );

        let decision = check_progress_task_alignment(
            "plan.complete",
            Some("step-final"),
            Some("task-1"),
            tmp.path(),
        );
        assert_eq!(decision, GateDecision::Aligned);
    }

    #[test]
    fn mismatch_task_closed_but_progress_missing() {
        let tmp = workspace();
        write_task(&tmp, "task-1", TaskStatus::Closed, "step-01");
        // progress.md does NOT list step-01 under Completed Steps
        write_file(
            tmp.path(),
            ".ralph/agent/progress.md",
            "## Current Step\nstep-02\n\n## Completed Steps\n- step-02\n",
        );

        let decision = check_progress_task_alignment(
            "queue.advance",
            Some("step-02"),
            Some("task-1"),
            tmp.path(),
        );
        match decision {
            GateDecision::Mismatch(m) => {
                assert_eq!(m.reason, "task_closed_but_progress_missing");
                assert_eq!(m.task_id.as_deref(), Some("task-1"));
            }
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn mismatch_step_not_in_progress() {
        let tmp = workspace();
        write_file(
            tmp.path(),
            ".ralph/agent/progress.md",
            "## Current Step\nstep-05\n\n## Completed Steps\n- step-01\n- step-02\n",
        );

        let decision = check_progress_task_alignment(
            "queue.advance",
            Some("step-09"),
            None,
            tmp.path(),
        );
        match decision {
            GateDecision::Mismatch(m) => {
                assert_eq!(m.reason, "step_mismatch");
                assert_eq!(m.step.as_deref(), Some("step-09"));
            }
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn empty_progress_md_is_rejected() {
        let tmp = workspace();
        write_file(tmp.path(), ".ralph/agent/progress.md", "# nothing here\n\n");

        let decision = check_progress_task_alignment(
            "queue.advance",
            Some("step-01"),
            None,
            tmp.path(),
        );
        match decision {
            GateDecision::Mismatch(m) => {
                assert_eq!(m.reason, "progress_missing_headings");
            }
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn missing_progress_md_is_rejected() {
        let tmp = workspace();
        // No progress.md at all.
        let decision = check_progress_task_alignment(
            "queue.advance",
            Some("step-01"),
            None,
            tmp.path(),
        );
        match decision {
            GateDecision::Mismatch(m) => {
                assert_eq!(m.reason, "progress_unreadable");
            }
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn empty_tasks_missing_task_id_is_rejected() {
        let tmp = workspace();
        write_file(
            tmp.path(),
            ".ralph/agent/progress.md",
            "## Current Step\nstep-01\n\n## Completed Steps\n",
        );
        // tasks.jsonl is empty (file does not exist), task_id not found.
        let decision = check_progress_task_alignment(
            "queue.advance",
            Some("step-01"),
            Some("task-missing"),
            tmp.path(),
        );
        match decision {
            GateDecision::Mismatch(m) => {
                assert_eq!(m.reason, "task_not_found");
                assert_eq!(m.task_id.as_deref(), Some("task-missing"));
            }
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn inert_for_non_gated_topic() {
        let tmp = workspace();
        // No files at all — gate should be inert, not mismatch.
        let decision = check_progress_task_alignment(
            "review.dimension.done",
            Some("step-01"),
            Some("task-1"),
            tmp.path(),
        );
        assert_eq!(decision, GateDecision::Inert);
    }

    #[test]
    fn progress_snapshot_parses_markdown_heading_with_inline_value() {
        let content = "## Current Step: step-03\n\n## Completed Steps: step-01, step-02\n";
        let snap = ProgressSnapshot::parse(content);
        assert_eq!(snap.current_step.as_deref(), Some("step-03"));
        // Inline value is split on commas so each entry is independently
        // matchable in `is_step_completed`.
        assert_eq!(snap.completed_steps, vec!["step-01", "step-02"]);
    }

    #[test]
    fn progress_snapshot_parses_bold_label_form() {
        let content =
            "**Current Step**: step-04\n\n**Completed Steps**:\n- step-01\n- step-02\n* step-03\n";
        let snap = ProgressSnapshot::parse(content);
        assert_eq!(snap.current_step.as_deref(), Some("step-04"));
        assert_eq!(
            snap.completed_steps,
            vec!["step-01", "step-02", "step-03"]
        );
    }

    #[test]
    fn progress_snapshot_handles_blank_file() {
        let snap = ProgressSnapshot::parse("");
        assert!(snap.empty_headings);
        assert!(snap.current_step.is_none());
        assert!(snap.completed_steps.is_empty());
    }

    #[test]
    fn gated_topics_list_is_narrow() {
        // Belt-and-suspenders: lock the gate's topic set so a future
        // refactor cannot silently widen it without a test failure.
        assert_eq!(GATED_TOPICS, &["queue.advance", "plan.complete"]);
    }

    #[test]
    fn task_in_progress_is_not_a_mismatch() {
        let tmp = workspace();
        write_task(&tmp, "task-1", TaskStatus::InProgress, "step-01");
        write_file(
            tmp.path(),
            ".ralph/agent/progress.md",
            "## Current Step\nstep-01\n\n## Completed Steps\n",
        );
        let decision = check_progress_task_alignment(
            "queue.advance",
            Some("step-01"),
            Some("task-1"),
            tmp.path(),
        );
        assert_eq!(decision, GateDecision::Aligned);
    }
}