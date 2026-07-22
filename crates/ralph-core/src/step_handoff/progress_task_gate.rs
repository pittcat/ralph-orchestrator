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
    ///
    /// **Deprecated for read access as of U1 of plan 2026-07-05-005.**
    /// This field is still populated by [`Self::parse`] (for backwards
    /// compatibility with the on-disk markdown format and external
    /// tools), but readers MUST go through the derived
    /// [`Self::current_step`] accessor which returns
    /// `completed_steps.last()`. Reading this field directly is a
    /// KTD-1 violation; the writer also renders the heading from the
    /// derived value so the on-disk shape and the derived view can
    /// never diverge.
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

        // `empty_headings` is computed from the on-disk parsed
        // `current_step` (NOT the derived accessor) — it answers
        // "did the on-disk markdown have any headings at all?",
        // which is a parse-time invariant independent of the
        // derived `current_step()` view added by U1 of plan
        // 2026-07-05-005.
        snap.empty_headings = snap.current_step.is_none() && snap.completed_steps.is_empty();
        snap
    }

    /// Returns true when `step` is listed under "Completed Steps".
    pub fn is_step_completed(&self, step: &str) -> bool {
        let target = step.trim();
        self.completed_steps.iter().any(|s| s.trim() == target)
    }

    /// Derived accessor for the "current step".
    ///
    /// **U1 of plan 2026-07-05-005 (KTD-1)**: this method is the
    /// single source of truth for the project's current step. It
    /// returns `completed_steps.last()`, so the value cannot drift
    /// from the on-disk list. The legacy `current_step` field is
    /// still populated by [`Self::parse`] for backwards compatibility
    /// but MUST NOT be read directly — readers (gate, projector,
    /// orchestrator context) all go through this method.
    ///
    /// Shadow semantics also move here: the old `current_step ==
    /// completed_steps.last()` shadow check is now structurally
    /// impossible because both come from the same `completed_steps`
    /// vector.
    pub fn current_step(&self) -> Option<&str> {
        self.completed_steps.last().map(String::as_str)
    }
}

fn split_heading(section: &str) -> Option<(&str, &str)> {
    // Review fix #7 (code-review-2026-06-17-002): the previous
    // `split_once(':')` truncated step names that contained colons
    // (e.g. `## Current Step: step-02: validate-foo` produced
    // `("Current Step", "step-02")`). The parser now identifies the
    // heading name first and consumes the rest of the line as the
    // value verbatim.
    //
    // `## Current Step: step-02` → ("Current Step", "step-02")
    // `## Current Step: step-02: validate-foo` → ("Current Step", "step-02: validate-foo")
    // `## Current Step` (no inline value) → None (caller reads next line as value)
    if let Some((name, value)) = section.split_once(':') {
        let name = name.trim();
        if matches!(name, "Current Step" | "Completed Steps") {
            // Return the raw post-colon value; do NOT re-trim a
            // colon — value may legitimately contain colons.
            return Some((name, value.trim_start()));
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
///
/// 2026-06-23 fix: renamed from `GateDecision` to
/// `TaskProgressDecision` to remove the name collision with
/// `crate::preset::engine::gates::GateDecision`. Both enums share
/// the suffix `GateDecision` but model semantically different
/// decisions; using the same name across crates made it easy
/// to extend one enum's variant set and forget to update
/// downstream match arms in the other module. Alias
/// `GateDecision` is preserved as a deprecated re-export for
/// one release to keep external crates compiling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TaskProgressDecision {
    /// Gate is off; topic is not gated; pass through.
    Inert,
    /// Topic is gated and progress.md ↔ tasks.jsonl align; pass through.
    Aligned,
    /// Topic is gated but progress.md ↔ tasks.jsonl disagree; reject.
    Mismatch(ProgressTaskMismatch),
}

/// 2026-06-23 fix: deprecated alias kept for downstream
/// crates that previously imported
/// `crate::step_handoff::progress_task_gate::GateDecision`.
/// New code MUST use `TaskProgressDecision` directly. This
/// alias is intentionally a `pub use` of the new name so
/// downstream `match GateDecision::Inert` arms keep working
/// without a string-level rename.
#[deprecated(
    since = "0.1.0",
    note = "renamed to TaskProgressDecision to remove name collision with crate::preset::engine::gates::GateDecision; the variant set is the same"
)]
pub use TaskProgressDecision as GateDecision;

/// Pure-function check that takes the snapshot directly instead
/// of reading from disk. The U4 validation pipeline
/// (`crates/ralph-core/src/validation/rules_step_handoff.rs`)
/// consumes this signature so the gate stops touching
/// `std::fs::*` from inside the validation hot path.
///
/// `progress` is the parsed markdown snapshot, `tasks` is the
/// U1 of plan 2026-07-02-005: `payload_completed_steps` carries
/// the `completed_steps` array from the inbound event payload
/// (only meaningful for `plan.complete`). When non-empty, the
/// gate uses array-vs-snapshot set inclusion as the primary
/// acceptance criterion under the `Current Step=None` branch,
/// instead of falling back to the single-step heuristic above.
///
/// All other behaviour mirrors [`check_progress_task_alignment`]
/// including the cold-start exemption and fail-closed defaults.
pub fn check_alignment_with_snapshot(
    progress: &ProgressSnapshot,
    tasks: &[Task],
    topic: &str,
    step: Option<&str>,
    task_id: Option<&str>,
    payload_completed_steps: Option<&[String]>,
    workflow_phase_id: Option<&str>,
) -> GateDecision {
    if !is_gated_topic(topic) {
        return GateDecision::Inert;
    }

    if crate::event_loop::phase_authority::progress_gate_helper::progress_gate_should_skip_missing_current_step(
        workflow_phase_id,
    ) {
        return GateDecision::Aligned;
    }

    // 1. Empty / missing headings → fail-closed.
    if progress.empty_headings {
        return GateDecision::Mismatch(ProgressTaskMismatch {
            reason: "progress_missing_headings".to_string(),
            detail: "progress snapshot has no `Current Step` or `Completed Steps` headings"
                .to_string(),
            step: step.map(|s| s.to_string()),
            task_id: task_id.map(|t| t.to_string()),
        });
    }

    // 2. Step alignment. U1 of plan 2026-07-02-005: when
    //    `progress.current_step` is `None` and `topic == "plan.complete"`,
    //    accept if the event's `completed_steps` array (from payload)
    //    is entirely a subset of `snapshot.completed_steps` (the
    //    single-step fallback also remains: `step ∈ completed`). This
    //    covers the `pass_with_residuals` terminal path where the agent
    //    ships `plan.complete` with a `completed_steps` payload listing
    //    every step but does not maintain a `Current Step` pointer
    //    (the agent has already advanced past every step).
    if let Some(step_value) = step {
        // U1 of plan 2026-07-05-005 (KTD-1): read the derived
        // `current_step()` accessor (== `completed_steps.last()`),
        // not the deprecated `current_step` field.
        match progress.current_step() {
            None => {
                // Fallback (2026-07-01 fix for primary-20260701-140149):
                // when progress.md has no Completed Steps entries
                // AND the inbound step is not already completed, the
                // agent has nothing to anchor against. The
                // `progress_missing_current_step` mismatch is still
                // fail-closed in this shape; the relaxation only
                // applies when the target step is already in the
                // completed list (the `fix_plan_file="null"` happy
                // path).
                if !progress.is_step_completed(step_value) {
                    return GateDecision::Mismatch(ProgressTaskMismatch {
                        reason: "progress_missing_current_step".to_string(),
                        detail: format!(
                            "event step='{}' but progress.md has no Completed Steps entry to derive Current Step from",
                            step_value
                        ),
                        step: Some(step_value.to_string()),
                        task_id: task_id.map(|t| t.to_string()),
                    });
                }
            }
            Some(current) if current.trim() != step_value.trim() => {
                if !progress.is_step_completed(step_value) {
                    return GateDecision::Mismatch(ProgressTaskMismatch {
                        reason: "step_mismatch".to_string(),
                        detail: format!(
                            "event step='{}' but progress Current Step='{}' and '{}' is not in Completed Steps",
                            step_value, current, step_value
                        ),
                        step: Some(step_value.to_string()),
                        task_id: task_id.map(|t| t.to_string()),
                    });
                }
            }
            Some(_) => {}
        }
    }

    // U1 of plan 2026-07-02-005 (EXTEND): pay-load-driven
    // `completed_steps` array check. When the topic is `plan.complete`,
    // the agent may ship the terminal event with a
    // `completed_steps: string[]` payload whose every element is
    // already listed under Completed Steps in `progress.md`. This is
    // the `pass_with_residuals` / 140149-shaped path. Require every
    // entry to be present; even one missing entry is a
    // `progress_missing_current_step` mismatch.
    //
    // U1 of plan 2026-07-05-005 (KTD-1) adaptation: the original
    // guard `progress.current_step.is_none()` is dropped because
    // `current_step` is now derived from
    // `completed_steps.last()`, which is `Some(_)` after the agent
    // has completed the first step. The payload array is the
    // authoritative signal that the agent is in the terminal
    // `pass_with_residuals` shape — when it is non-empty the gate
    // switches to payload-driven alignment regardless of the
    // derived current step. A `None` or empty payload keeps the
    // legacy single-step fallback above as the only path.
    if topic == "plan.complete"
        && let Some(payload_steps) = payload_completed_steps
        && !payload_steps.is_empty()
    {
        let missing: Vec<&str> = payload_steps
            .iter()
            .map(|s| s.as_str())
            .filter(|s| !progress.is_step_completed(s))
            .collect();
        if !missing.is_empty() {
            return GateDecision::Mismatch(ProgressTaskMismatch {
                reason: "progress_missing_current_step".to_string(),
                detail: format!(
                    "plan.complete completed_steps={:?} but progress.md Completed Steps is missing entries: {}",
                    payload_steps,
                    missing.join(", ")
                ),
                step: step.map(|s| s.to_string()),
                task_id: task_id.map(|t| t.to_string()),
            });
        }
        // Accepted: every payload completed step is in snapshot.
        return GateDecision::Aligned;
    }

    // 3. Task alignment (closed-but-not-marked).
    if let Some(task_id_value) = task_id {
        match tasks.iter().find(|t| t.id == task_id_value) {
            None => {
                return GateDecision::Mismatch(ProgressTaskMismatch {
                    reason: "task_not_found".to_string(),
                    detail: format!(
                        "event references task_id='{}' which is not in the task ledger",
                        task_id_value
                    ),
                    step: step.map(|s| s.to_string()),
                    task_id: Some(task_id_value.to_string()),
                });
            }
            Some(task) if is_task_closed(task) => {
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
                            task_id_value, s
                        ),
                        step: step.map(|st| st.to_string()),
                        task_id: Some(task_id_value.to_string()),
                    });
                }
            }
            Some(_) => {}
        }
    }

    GateDecision::Aligned
}

/// Check progress.md vs tasks.jsonl alignment for the given step/task.
///
/// Returns `Aligned` when both ledgers agree, `Mismatch` with the
/// reason when they disagree, and `Inert` when the topic is not in
/// [`GATED_TOPICS`] (the caller should skip the gate entirely).
///
/// The function is **pure** (with respect to memory): it reads
/// progress.md and tasks.jsonl from disk. The caller decides
/// whether to surface the mismatch as `RejectWithResume` and emit
/// `plan.blocked`.
///
/// `step` and `task_id` come from the inbound event's payload. The
/// function is lenient about `None` values: a `None` step means the
/// gate cannot verify step-level alignment and falls back to the
/// task-level check; a `None` task_id skips the closed-but-not-marked
/// check and only verifies the `Current Step` field exists.
///
/// **U4c**: prefer [`check_alignment_with_snapshot`] when the
/// caller already has a [`LedgerSnapshot`] (or any pre-loaded
/// `ProgressSnapshot` + `&[Task]`). This function remains as a
/// convenience for legacy callers that do not yet participate in
/// the snapshot pipeline.
#[deprecated(
    since = "0.1.0",
    note = "prefer check_alignment_with_snapshot (pure, no disk I/O)"
)]
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
    //
    // Review fix #5 (code-review-2026-06-17-002): cold-start
    // exemption. When `progress.md` does not exist yet (workspace is
    // brand-new, agent hasn't created the ledger), a strict
    // fail-closed reject on the very first `queue.advance` triggers a
    // dead loop: every iteration rejects → plan.blocked → plan-gate
    // emits queue.advance → rejected again. The exemption: when
    // `progress.md` is missing AND the inbound step looks like a
    // cold-start step (digit-1 prefix, no dash number indicating a
    // later step), skip the gate. The agent will create progress.md
    // on its first iteration and subsequent steps will go through
    // the full check.
    let progress = match read_progress(&progress_path) {
        Ok(snap) => snap,
        Err(reason) => {
            // Review fix #5 (code-review-2026-06-17-002): cold-start
            // exemption. When `progress.md` does not exist yet
            // (workspace is brand-new, agent hasn't created the
            // ledger), a strict fail-closed reject on the very first
            // `queue.advance` triggers a dead loop. The exemption:
            // when read fails AND the inbound step looks like a
            // cold-start step (digit-1 prefix), skip the gate. The
            // agent will create progress.md on its first iteration
            // and subsequent steps will go through the full check.
            //
            // Only a missing file qualifies for the cold-start
            // exemption. Real I/O errors (permissions, corruption,
            // etc.) remain fail-closed and produce a Mismatch.
            let looks_like_cold_start = step.map(is_cold_start_step).unwrap_or(false);
            if reason == "progress_not_found" && looks_like_cold_start {
                return GateDecision::Aligned;
            }
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
    // U1 of plan 2026-07-05-005 (KTD-1): read the derived
    // `current_step()` accessor (== `completed_steps.last()`).
    if let Some(step) = step {
        match progress.current_step() {
            None => {
                // Fallback (2026-07-01 fix for primary-20260701-140149):
                // mirror of the snapshot variant above. When
                // `progress.md` has no Current Step heading but the
                // target step is already listed under Completed Steps,
                // treat as aligned (this is the `fix_plan_file="null"`
                // happy path: all units closed, no fix-unit expected,
                // shipper takes over directly). Conservative — only
                // relaxes the `None` branch when the step name matches
                // an already-completed entry in the same progress.md.
                if !progress.is_step_completed(step) {
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
    match std::fs::read_to_string(path) {
        Ok(content) => Ok(ProgressSnapshot::parse(&content)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err("progress_not_found"),
        Err(_) => Err("progress_unreadable"),
    }
}

/// U6 of plan 2026-07-02-005: PreCommit-time progress snapshot
/// reconciliation. The runtime's in-memory `LedgerSnapshot.progress`
/// is sometimes stale (175407: the projector flushed `progress.md`
/// to disk but the snapshot mirror kept the pre-flush view). The
/// gate then sees a `progress_missing_current_step` mismatch that
/// is in fact a stale-mirror false positive.
///
/// This function is the cheap reconciliation layer: it reads
/// `progress.md` from disk and, **only if** the parsed
/// fingerprint differs from the in-memory snapshot's
/// fingerprint, overwrites the snapshot with the on-disk view.
///
/// Staleness fingerprint = `(current_step, completed_steps.join(",")).
/// Return value: `true` if the snapshot was refreshed, `false`
/// otherwise (including all read errors — the caller falls back
/// to the in-memory view on a read error so the gate stays
/// fail-closed on real I/O issues).
pub fn refresh_progress_snapshot_if_stale(
    progress_path: &Path,
    snapshot: &mut ProgressSnapshot,
) -> bool {
    let disk = match read_progress(progress_path) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let disk_fp = progress_fingerprint(&disk);
    let mem_fp = progress_fingerprint(snapshot);
    if disk_fp == mem_fp {
        return false;
    }
    *snapshot = disk;
    true
}

fn progress_fingerprint(snap: &ProgressSnapshot) -> (Option<String>, String) {
    // U1 of plan 2026-07-05-005 (KTD-1): fingerprint uses the derived
    // `current_step()` accessor (== `completed_steps.last()`) so the
    // staleness check compares apples to apples — both disk and
    // memory produce the same value for the same underlying list.
    (
        snap.current_step().map(str::to_string),
        snap.completed_steps.join(","),
    )
}

#[cfg(test)]
mod refresh_tests {
    use super::*;
    use std::io::Write;

    fn write_progress(path: &Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::fs::File::create(path).unwrap();
        f.write_all(body.as_bytes()).unwrap();
    }

    #[test]
    fn refresh_progress_snapshot_if_stale_returns_false_on_match() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("progress.md");
        write_progress(
            &path,
            "## Current Step\nstep-02\n\n## Completed Steps\n- step-01\n",
        );
        let mut snap = ProgressSnapshot::parse(&std::fs::read_to_string(&path).unwrap());
        let refreshed = refresh_progress_snapshot_if_stale(&path, &mut snap);
        assert!(!refreshed, "matching fingerprint must NOT report refresh");
    }

    #[test]
    fn refresh_progress_snapshot_if_stale_overwrites_on_mismatch() {
        // 175407 root cause scenario: in-memory progress is stale
        // (empty / pre-flush), but disk has the real completed
        // list. The reconciler must overwrite.
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("progress.md");
        write_progress(&path, "## Completed Steps\n- step-01\n- step-02\n");
        let mut snap = ProgressSnapshot::default(); // stale, empty
        let refreshed = refresh_progress_snapshot_if_stale(&path, &mut snap);
        assert!(refreshed, "stale mirror must be reported as refreshed");
        assert_eq!(snap.completed_steps, vec!["step-01", "step-02"]);
        assert_eq!(
            snap.current_step, None,
            "no ## Current Step heading → current_step is None"
        );
    }

    #[test]
    fn refresh_progress_snapshot_if_stale_returns_false_on_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("no-such.md");
        let mut snap = ProgressSnapshot::default();
        let refreshed = refresh_progress_snapshot_if_stale(&path, &mut snap);
        assert!(!refreshed, "missing file must NOT report refresh");
        // The snapshot is untouched.
        assert!(snap.completed_steps.is_empty());
    }
}

/// Review fix #5 (code-review-2026-06-17-002): helper for the
/// cold-start exemption. Returns true when the inbound step looks
/// like the very first step of a fresh plan (digit-1 prefix, no
/// trailing number indicating a later step). Examples:
/// `step-1` → true; `step-2` → false; `u1-step-1` → true;
/// `01-introduction` → true; `phase-1-implementation` → true
/// (matches `-1` substring at a non-prefix position to permit
/// namespaced first steps).
///
/// Conservative: anything we cannot pattern-match confidently
/// returns false so the strict fail-closed path stays the default.
fn is_cold_start_step(step: &str) -> bool {
    // Strip an optional leading "u<digits>-" / "U<digits>-" unit prefix
    // that the task system uses, then look at the remainder.
    let s = step
        .strip_prefix(|c: char| c.is_ascii_alphabetic())
        .and_then(|rest| {
            let digit_count = rest.chars().take_while(|c| c.is_ascii_digit()).count();
            if digit_count == 0 {
                return None;
            }
            rest[digit_count..].strip_prefix('-')
        })
        .unwrap_or(step);

    // Accept a single digit `1` preceded by a separator (`-`, `.`, `_`, `/`)
    // and not followed by another digit. This handles `step-1`, `u1-step-1`,
    // `01.1`, `phase_1`, etc., while rejecting `step-10` or `step-2`.
    let bytes = s.as_bytes();
    for i in 0..bytes.len().saturating_sub(1) {
        let sep = bytes[i];
        if !matches!(sep, b'-' | b'.' | b'_' | b'/') {
            continue;
        }
        let digit = bytes[i + 1];
        if !digit.is_ascii_digit() {
            continue;
        }
        if digit != b'1' {
            continue;
        }
        let next_is_digit = bytes
            .get(i + 2)
            .map(|b| b.is_ascii_digit())
            .unwrap_or(false);
        if !next_is_digit {
            return true;
        }
    }
    false
}

#[cfg(test)]
#[allow(deprecated)]
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
        // U1 of plan 2026-07-05-005 (KTD-1): the on-disk markdown
        // heading is still parsed, but the gate's read path
        // derives `current_step` from `completed_steps.last()`.
        // To make the derive return step-02, the completed list
        // must end with step-02; the legacy `## Current Step`
        // heading is ignored at read time.
        write_file(
            tmp.path(),
            ".ralph/agent/progress.md",
            "## Current Step\nstep-99\n\n## Completed Steps\n- step-01\n- step-02\n",
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
        // U1 of plan 2026-07-05-005 (KTD-1): the on-disk markdown
        // heading `## Current Step` is still populated by the
        // parser, but the gate's read path now derives the
        // current step from `completed_steps.last()`. Pin the
        // contract to that derived view: the `## Current Step`
        // heading is ignored on read, so the test only needs
        // `step-01` in `## Completed Steps` to make the derived
        // `current_step()` return `step-01`.
        write_file(
            tmp.path(),
            ".ralph/agent/progress.md",
            "## Current Step: step-final\n\n## Completed Steps\n- step-01\n",
        );

        let decision = check_progress_task_alignment(
            "plan.complete",
            Some("step-01"),
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

        let decision =
            check_progress_task_alignment("queue.advance", Some("step-09"), None, tmp.path());
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

        let decision =
            check_progress_task_alignment("queue.advance", Some("step-01"), None, tmp.path());
        match decision {
            GateDecision::Mismatch(m) => {
                assert_eq!(m.reason, "progress_missing_headings");
            }
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn missing_progress_md_is_rejected_for_non_cold_start() {
        let tmp = workspace();
        // No progress.md and a non-cold-start step → fail-closed.
        let decision =
            check_progress_task_alignment("queue.advance", Some("step-02"), None, tmp.path());
        match decision {
            GateDecision::Mismatch(m) => {
                assert_eq!(m.reason, "progress_not_found");
            }
            other => panic!("expected mismatch, got {other:?}"),
        }
    }

    #[test]
    fn missing_progress_md_cold_start_step_is_exempt() {
        let tmp = workspace();
        // No progress.md but a cold-start step → exempt so the agent
        // can create the ledger on its first iteration.
        let decision =
            check_progress_task_alignment("queue.advance", Some("step-1"), None, tmp.path());
        assert_eq!(decision, GateDecision::Aligned);
    }

    #[test]
    fn empty_tasks_missing_task_id_is_rejected() {
        let tmp = workspace();
        // U1 of plan 2026-07-05-005 (KTD-1): the gate's
        // `current_step` is derived from `completed_steps.last()`.
        // To pass the step-alignment branch (which runs before
        // task alignment), the completed list must end with
        // step-01 so the derived view matches the inbound step.
        write_file(
            tmp.path(),
            ".ralph/agent/progress.md",
            "## Current Step\nstep-99\n\n## Completed Steps\n- step-01\n",
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
        assert_eq!(snap.completed_steps, vec!["step-01", "step-02", "step-03"]);
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
        // U1 of plan 2026-07-05-005 (KTD-1): the gate's current
        // step is derived from `completed_steps.last()`. To pass
        // the step-alignment branch the completed list must end
        // with step-01; the legacy `## Current Step` heading is
        // ignored at read time.
        write_file(
            tmp.path(),
            ".ralph/agent/progress.md",
            "## Current Step\nstep-99\n\n## Completed Steps\n- step-01\n",
        );
        let decision = check_progress_task_alignment(
            "queue.advance",
            Some("step-01"),
            Some("task-1"),
            tmp.path(),
        );
        assert_eq!(decision, GateDecision::Aligned);
    }

    // U1 of plan 2026-07-02-005 — payload `completed_steps`
    // array drives the gate under `Current Step=None`. The shape
    // mirrors the 140149 fix-unit terminal: progress.md is left
    // without a `## Current Step` heading (every step is done)
    // while the agent emits `plan.complete` with a
    // `completed_steps: [..]` payload listing each completed
    // step.

    fn snapshot_with_completed(steps: &[&str]) -> ProgressSnapshot {
        ProgressSnapshot {
            current_step: None,
            completed_steps: steps.iter().map(|s| s.to_string()).collect(),
            empty_headings: false,
        }
    }

    #[test]
    fn u1_plan_complete_completed_steps_subset_is_aligned() {
        let snap = snapshot_with_completed(&["step-01", "step-02"]);
        let payload = vec!["step-01".to_string(), "step-02".to_string()];
        let decision = check_alignment_with_snapshot(
            &snap,
            &[],
            "plan.complete",
            Some("step-02"),
            Some("task-1"),
            Some(&payload),
            None,
        );
        assert_eq!(decision, GateDecision::Aligned);
    }

    #[test]
    fn u1_plan_complete_completed_steps_last_step_event_step_aligned() {
        let snap = snapshot_with_completed(&["step-01", "step-02"]);
        let payload = vec!["step-01".to_string(), "step-02".to_string()];
        let decision = check_alignment_with_snapshot(
            &snap,
            &[],
            "plan.complete",
            Some("step-02"),
            Some("task-1"),
            Some(&payload),
            None,
        );
        assert_eq!(
            decision,
            GateDecision::Aligned,
            "event's `step` is the terminal step AND `completed_steps` array intersects snapshot"
        );
    }

    #[test]
    fn u1_plan_complete_completed_steps_missing_entry_is_mismatch() {
        let snap = snapshot_with_completed(&["step-01", "step-02"]);
        let payload = vec![
            "step-01".to_string(),
            // step-02 缺失
            "step-03".to_string(),
        ];
        let decision = check_alignment_with_snapshot(
            &snap,
            &[],
            "plan.complete",
            Some("step-02"),
            Some("task-1"),
            Some(&payload),
            None,
        );
        match decision {
            GateDecision::Mismatch(m) => {
                assert_eq!(
                    m.reason, "progress_missing_current_step",
                    "missing `completed_steps` entry must surface as progress_missing_current_step"
                );
            }
            other => panic!("expected Mismatch, got {other:?}"),
        }
    }

    #[test]
    fn u1_plan_complete_completed_steps_none_falls_back_to_single_step() {
        // No `completed_steps` in payload → falls back to the
        // pre-existing single-step branch: `step` in
        // snapshot.completed → Aligned.
        let snap = snapshot_with_completed(&["step-02"]);
        let decision = check_alignment_with_snapshot(
            &snap,
            &[],
            "plan.complete",
            Some("step-02"),
            None,
            None,
            None,
        );
        assert_eq!(decision, GateDecision::Aligned);
    }

    #[test]
    fn u1_plan_complete_completed_steps_empty_falls_back_to_single_step() {
        // Empty `completed_steps` → same fall-through as `None`.
        let snap = snapshot_with_completed(&["step-02"]);
        let decision = check_alignment_with_snapshot(
            &snap,
            &[],
            "plan.complete",
            Some("step-02"),
            None,
            Some(&[]),
            None,
        );
        assert_eq!(decision, GateDecision::Aligned);
    }

    #[test]
    fn u1_queue_advance_completed_steps_payload_ignored() {
        // payload_completed_steps is only meaningful for
        // `plan.complete`; queue.advance still uses the single-
        // step alignment path.
        let snap = snapshot_with_completed(&["step-02"]);
        let payload = vec!["step-01".to_string()]; // bogus non-subset
        let decision = check_alignment_with_snapshot(
            &snap,
            &[],
            "queue.advance",
            Some("step-02"),
            None,
            Some(&payload),
            None,
        );
        assert_eq!(decision, GateDecision::Aligned);
    }

    /// U1 of plan 2026-07-02-005: when `progress.md` has only
    /// `Completed Steps` (no `Current Step`) and `payload.
    /// completed_steps` covers every step in snapshot, the
    /// `task_closed_but_progress_missing` rule still fires for
    /// any closed task whose `title` is NOT covered. This pins
    /// the rule ordering: completed_steps accept runs first,
    /// then task alignment runs as a separate guard. We
    /// exercise the latter by omitting the array (forcing
    /// fall-through to task alignment) and a closed task that
    /// lacks progress.
    #[test]
    fn u1_plan_complete_closed_task_still_routes_to_task_alignment_branch() {
        let snap = snapshot_with_completed(&["step-01", "step-02"]);
        let mut task = Task::new("other-step".to_string(), 1);
        task.id = "task-1".to_string();
        task.status = TaskStatus::Closed;
        // No payload `completed_steps` → falls through to the
        // legacy single-step + task alignment path.
        let decision = check_alignment_with_snapshot(
            &snap,
            std::slice::from_ref(&task),
            "plan.complete",
            Some("step-02"),
            Some("task-1"),
            None,
            None,
        );
        match decision {
            GateDecision::Mismatch(m) => {
                assert_eq!(m.reason, "task_closed_but_progress_missing");
            }
            other => panic!("expected task_closed_but_progress_missing Mismatch, got {other:?}"),
        }
    }
}
