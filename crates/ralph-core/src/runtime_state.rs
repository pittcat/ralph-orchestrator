//! Runtime state snapshot — the source for the `## ORCHESTRATOR CONTEXT`
//! prompt block.
//!
//! Plan ref: U4 of
//! `docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md`.
//!
//! Phase 1 narrow surface: the snapshot reads from the projector's
//! in-memory cache (populated by `bootstrap_from_disk` and kept in
//! sync by every `apply`). The hat never has to hand-write a ledger
//! or tail `events.jsonl` to figure out "where am I?" — the snapshot
//! is the answer.
//!
//! The block is opt-in: when `state_projection.enabled` is `false`
//! the runtime falls back to a stub that explains the state is
//! disabled, so the agent still sees *some* context without
//! touching the ledgers (per plan U4 happy-path "edge case").

use std::fmt::Write as _;
use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::state_projector::StateProjector;
use crate::state_projector::review::{ReviewDimensionsView, render_review_summary_block};
use crate::task::{Task, TaskStatus};
use crate::task_store::is_fix_unit_key;

/// Heading the loop prepends. Logged in the prompt verbatim so
/// agents and grep-based scrapers can match a single literal.
pub const ORCHESTRATOR_CONTEXT_HEADING: &str = "## ORCHESTRATOR CONTEXT";

/// Read-only snapshot of the orchestrator's view of the run. The
/// fields are deliberately narrow — see U4 token-budget rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeStateSnapshot {
    /// Plan name from the most recent event that carried one, or
    /// `(none)` when the loop has not seen a plan yet.
    pub plan_name: Option<String>,
    /// Current step from `progress.md`.
    pub current_step: Option<String>,
    /// Steps already marked completed in `progress.md`.
    pub completed_steps: Vec<String>,
    /// IDs / keys of open tasks (kept short — `id: title` joined).
    pub open_tasks: Vec<OpenTaskSummary>,
    /// Wave received / total when a wave is active. `None` outside
    /// wave context (mirrors the `WaveContext` model in
    /// `wave_context.rs`).
    pub wave: Option<WaveSummary>,
    /// The git HEAD SHA at the moment the loop was started.
    pub loop_start_sha: Option<String>,
    /// The git HEAD SHA at the moment the plan was first started.
    /// This is the review diff base for plan-driven presets.
    pub plan_baseline_sha: Option<String>,
    /// True when state projection is disabled for this run; the
    /// agent is told so it does not invent its own ledger.
    pub projection_disabled: bool,
    /// U3 (2026-07-01-002 plan): structured snapshot of the
    /// `fix-NN` task progress as derived from
    /// `.ralph/agent/tasks.jsonl`.  Drives the coordinator's
    /// `next_expected` decision instead of letting it count
    /// `### U{N}.` headings in `fix-plan.md`.
    #[serde(default)]
    pub fix_unit_state: Option<FixUnitState>,
    /// U3 of plan 2026-07-05-005: latest `review.dimensions.complete`
    /// view for the `## REVIEW SUMMARY` prompt block.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub review_dimensions: Option<ReviewDimensionsView>,
}

/// Per-fix-round view derived from `tasks.jsonl`.  All fields are
/// computed, never hand-written, so the schema is stable for
/// downstream agents (and BDD scenarios) to parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FixUnitState {
    /// Number of fix-NN tasks currently visible in tasks.jsonl.
    /// Equals `1` only when one fix-unit is open, `2+` for multi-unit
    /// plans, `0` for plans that never reached fix-phase.
    pub total: u32,
    /// Identifiers (`fix-01`, `fix-02`, …) of the fix-units already
    /// in a terminal state (`Closed` or `Failed`), sorted ascending.
    pub completed: Vec<String>,
    /// Identifier of the fix-unit the coordinator should be working
    /// on right now.  `None` when the loop has moved past fix-phase
    /// (e.g. all `Closed`) or never entered it.  This mirrors
    /// `progress.current_step` but is computed from `tasks.jsonl`
    /// only — the canonical ledger for fix-units.
    pub current: Option<String>,
    /// Coordinator hint derived from `current` vs `total`.
    ///
    /// * `Some("plan.complete")` when `current` is the last open
    ///   fix-unit, signalling `next_expected = plan.complete`.
    /// * `Some("work.ready(fix-NN+1)")` while another fix-unit is
    ///   pending.
    /// * `None` when the loop is not in fix-phase.
    pub next_expected: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenTaskSummary {
    pub id: String,
    pub title: String,
    pub status: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WaveSummary {
    pub wave_id: String,
    pub received: u32,
    pub total: u32,
}

impl RuntimeStateSnapshot {
    /// Build a snapshot from the projector's in-memory cache. When
    /// the cache is empty (cold cache, no bootstrap), the function
    /// falls back to reading the canonical ledgers directly — the
    /// loop runs in a single process so disk reads are cheap.
    pub fn build(projector: &StateProjector) -> Self {
        let ctx = projector.context();
        // Read through the dual-source accessors: the U2 path
        // returns the wired `LedgerSnapshot` when present; the
        // legacy path returns the `tasks_cache` / `progress_cache`
        // mirrors. Both are kept in sync by every `apply` /
        // `apply_from_ledger` call.
        let (tasks_ref, _from_ledger) = ctx.task_snapshot();
        let (progress_ref, _from_ledger) = ctx.progress_snapshot();
        let (tasks, progress) = if tasks_ref.is_empty()
            // U1 of plan 2026-07-05-005 (KTD-1): the cold-cache
            // detector reads the derived accessor; an empty
            // `completed_steps` list also means an empty derived
            // `current_step`, so the original two-branch check is
            // equivalent and stays correct.
            && progress_ref.current_step().is_none()
            && progress_ref.completed_steps.is_empty()
        {
            // Cold cache; try disk before giving up. The progress
            // path is already cached (or empty), so the disk read
            // for tasks is the only fall-through.
            crate::state_projector::read_state_from_disk(&ctx.workspace_root)
        } else {
            (tasks_ref.to_vec(), progress_ref.clone())
        };
        let progress_done = progress.completed_steps.clone();
        Self {
            plan_name: derive_plan_name(&tasks),
            // U1 of plan 2026-07-05-005 (KTD-1): `current_step` is
            // derived from `completed_steps.last()` — the
            // `ProgressSnapshot::current_step()` accessor is the
            // single source of truth. Reading the deprecated field
            // here is a KTD-1 violation.
            current_step: progress.current_step().map(str::to_string),
            completed_steps: progress.completed_steps,
            open_tasks: open_task_summaries(&tasks),
            wave: None, // U4 spike deferred: wave sub-section is
            //            duplicated with `## WAVE
            //            CONTEXT`. We omit the
            //            duplicate until U4 spike picks
            //            one. Both blocks remain
            //            available side-by-side.
            loop_start_sha: None,
            plan_baseline_sha: None,
            projection_disabled: !ctx.config.enabled,
            fix_unit_state: derive_fix_unit_state(&tasks, &progress_done),
            review_dimensions: ctx.review_dimensions_snapshot(),
        }
    }

    /// Build a "disabled" stub. Used when the runtime runs without
    /// `state_projection.enabled` — the agent still sees the
    /// heading, with an explanation instead of values, so it knows
    /// the orchestrator owns the ledgers but is not running the
    /// projector for this preset.
    pub fn disabled_stub() -> Self {
        Self {
            plan_name: None,
            current_step: None,
            completed_steps: Vec::new(),
            open_tasks: Vec::new(),
            wave: None,
            loop_start_sha: None,
            plan_baseline_sha: None,
            projection_disabled: true,
            fix_unit_state: None,
            review_dimensions: None,
        }
    }

    /// Render the `## ORCHESTRATOR CONTEXT` block. Empty when
    /// projection is disabled AND the caller explicitly opted out —
    /// the [build_prompt] caller decides which behaviour to use.
    pub fn to_prompt_block(&self) -> String {
        let mut buf = String::new();
        let _ = writeln!(buf, "{ORCHESTRATOR_CONTEXT_HEADING}");
        let _ = writeln!(
            buf,
            "The orchestrator owns `.ralph/agent/tasks.jsonl` and `.ralph/agent/progress.md`; \
             treat the values below as canonical and do NOT hand-write either ledger."
        );
        let _ = writeln!(buf);
        if self.projection_disabled {
            let _ = writeln!(
                buf,
                "State projection is **disabled** for this preset; the values below are the \
                 last known view from the projector (may be stale)."
            );
            let _ = writeln!(buf);
        }
        if let Some(plan) = &self.plan_name {
            let _ = writeln!(buf, "- plan_name: {plan}");
        } else {
            let _ = writeln!(buf, "- plan_name: (none)");
        }
        match &self.current_step {
            Some(step) => {
                let _ = writeln!(buf, "- current_step: {step}");
            }
            None => {
                let _ = writeln!(buf, "- current_step: (none)");
            }
        }
        if self.completed_steps.is_empty() {
            let _ = writeln!(buf, "- completed_steps: (none)");
        } else {
            let _ = writeln!(
                buf,
                "- completed_steps: {}",
                self.completed_steps.join(", ")
            );
        }
        if self.open_tasks.is_empty() {
            let _ = writeln!(buf, "- open_tasks: (none)");
        } else {
            let _ = writeln!(buf, "- open_tasks:");
            for task in &self.open_tasks {
                let _ = writeln!(buf, "  - {} [{}] {}", task.id, task.status, task.title);
            }
        }
        if let Some(sha) = &self.plan_baseline_sha {
            let _ = writeln!(buf, "- plan_baseline_sha: {sha}");
        } else {
            let _ = writeln!(buf, "- plan_baseline_sha: (none)");
        }
        if let Some(sha) = &self.loop_start_sha {
            let _ = writeln!(buf, "- loop_start_sha: {sha}");
        } else {
            let _ = writeln!(buf, "- loop_start_sha: (none)");
        }
        if let Some(wave) = &self.wave {
            let _ = writeln!(
                buf,
                "- wave: id={} received={}/{}",
                wave.wave_id, wave.received, wave.total
            );
        }
        match &self.fix_unit_state {
            Some(state) => {
                let _ = writeln!(buf, "- fix_unit_state:");
                let _ = writeln!(buf, "    total: {}", state.total);
                let _ = writeln!(
                    buf,
                    "    completed: {}",
                    if state.completed.is_empty() {
                        "(none)".to_string()
                    } else {
                        state.completed.join(", ")
                    }
                );
                let _ = writeln!(
                    buf,
                    "    current: {}",
                    state.current.as_deref().unwrap_or("(none)")
                );
                let _ = writeln!(
                    buf,
                    "    next_expected: {}",
                    state
                        .next_expected
                        .as_deref()
                        .unwrap_or("(none — not in fix-phase)")
                );
            }
            None => {
                let _ = writeln!(buf, "- fix_unit_state: (none — no fix-unit tasks seen)");
            }
        }
        let _ = writeln!(buf);
        if let Some(view) = &self.review_dimensions {
            buf.push_str(&render_review_summary_block(view));
        }
        buf
    }
}

fn derive_plan_name(tasks: &[Task]) -> Option<String> {
    // Phase 1 heuristic: the projector stamps every task with a
    // stable `key` shaped `ce-executor:<plan_name>:<step>:<unit>`.
    // The second `:`-delimited segment is the plan name. This is
    // more robust than parsing the free-form `description` field
    // (which the agent may overwrite), and keeps the `key` as the
    // single source of truth for plan identity. P2 fix — see
    // review notes. When no tasks exist, the snapshot reports
    // `(none)`.
    tasks
        .iter()
        .rev()
        .filter_map(|t| t.key.as_deref())
        .find_map(|key| {
            // Skip the leading `ce-executor:` prefix; the second
            // segment is the plan name. Anything that does not
            // match the canonical 4-segment shape falls through
            // to legacy parsing so we never misclassify a foreign
            // key.
            let mut parts = key.splitn(4, ':');
            let prefix = parts.next()?;
            if prefix != "ce-executor" {
                return None;
            }
            let plan = parts.next()?;
            if plan.is_empty() {
                return None;
            }
            // The shape has 4 segments total: prefix, plan, step,
            // unit. Reject anything with fewer or more parts so
            // we don't pick up `legacy-key` style entries.
            if parts.next().is_none() || parts.next().is_none() {
                return None;
            }
            Some(plan.to_string())
        })
}

fn open_task_summaries(tasks: &[Task]) -> Vec<OpenTaskSummary> {
    tasks
        .iter()
        .filter(|t| !t.status.is_terminal())
        .map(|t| OpenTaskSummary {
            id: t.id.clone(),
            title: t.title.clone(),
            status: format!("{:?}", t.status).to_lowercase(),
        })
        .collect()
}

/// Helper used by the event loop: read the canonical ledgers from
/// disk when no projector is wired up (U4 test path, cold paths).
pub fn snapshot_from_disk(workspace: &Path) -> RuntimeStateSnapshot {
    let (tasks, progress) = crate::state_projector::read_state_from_disk(workspace);
    let progress_done = progress.completed_steps.clone();
    RuntimeStateSnapshot {
        plan_name: derive_plan_name(&tasks),
        current_step: progress.current_step().map(str::to_string),
        completed_steps: progress.completed_steps,
        open_tasks: open_task_summaries(&tasks),
        wave: None,
        loop_start_sha: None,
        plan_baseline_sha: None,
        projection_disabled: true,
        fix_unit_state: derive_fix_unit_state(&tasks, &progress_done),
        review_dimensions: None,
    }
}

/// U3 (2026-07-01-002 plan): compute the fix-unit state view from the
/// tasks ledger.  Mirrors what
/// `review_step_state::prefill_fix_steps_from_plan` used to drive at
/// the tracker level, but here it stays in the snapshot layer so the
/// coordinator prompt can render it without the runtime needing to
/// re-parse `fix-plan.md` markdown.
///
/// **双源对账 (P1-2 fix):** `progress_completed_steps` must be the
/// authoritative "done" signal.  Tasks.jsonl `status` is updated by
/// projector after `task close`, which may race with the coordinator
/// activation following a `test.passed(fix-NN)`.  When the two
/// sources disagree (e.g. progress lists `fix-02` as completed but
/// tasks.jsonl still shows it `Open`), we trust progress — otherwise
/// the coordinator would emit a stray `work.ready(fix-03)` during the
/// brief window before task-close lands.  This is the exact regression
/// the original plan set out to prevent.
///
/// Behaviour:
/// * If no fix-unit keys exist, returns `None` (the loop is not in
///   fix-phase, nothing to advertise).
/// * Otherwise groups fix-NN tasks by step id, sorts by numeric id,
///   and emits:
///   - `total` — number of distinct fix-unit ids observed,
///   - `completed` — union of (terminal task status) ∪ (fix-NN ids
///     appearing in `progress_completed_steps`), sorted ascending,
///   - `current` — id of the lowest non-terminal fix-unit **not** in
///     `progress_completed_steps`, `None` when every fix-unit is
///     either terminal or otherwise marked done,
///   - `next_expected` — `Some("plan.complete")` when `current` is
///     the highest id, otherwise
///     `Some("work.ready(fix-{NN+1})")` with the next id.  Uses the
///     id found in `current`, not a derived counter, so hand-edited
///     or out-of-order keys still surface the right hint.
fn derive_fix_unit_state(
    tasks: &[Task],
    progress_completed_steps: &[String],
) -> Option<FixUnitState> {
    use std::collections::BTreeSet;
    // Pre-compute the set of `fix-NN` ids appearing in progress.md's
    // `Completed Steps` — the high-water mark for "executor declared
    // this unit done".  We treat any such id as terminal even if
    // tasks.jsonl still reports Open, because progress.md is written
    // synchronously by the projector right after `test.passed` lands.
    let progress_done: BTreeSet<String> = progress_completed_steps
        .iter()
        .filter_map(|s| fix_id_from_string(s))
        .collect();
    let mut all: BTreeSet<String> = BTreeSet::new();
    let mut completed: BTreeSet<String> = BTreeSet::new();
    let mut first_open: Option<String> = None;
    for t in tasks {
        let key = match t.key.as_deref() {
            Some(k) if is_fix_unit_key(k) => k,
            _ => continue,
        };
        let Some(id) = fix_id_from_key(key) else {
            continue;
        };
        all.insert(id.clone());
        if t.status.is_terminal() || progress_done.contains(&id) {
            completed.insert(id.clone());
        } else if first_open.is_none() {
            first_open = Some(id.clone());
        }
    }
    if all.is_empty() {
        return None;
    }
    let total = all.len() as u32;
    let completed: Vec<String> = completed.into_iter().collect();
    let current = first_open;
    let last_id = all.iter().last().cloned();
    let next_expected = match (current.as_ref(), last_id) {
        (Some(curr), Some(last)) => {
            if curr == &last {
                Some("plan.complete".to_string())
            } else {
                // Find the next id strictly greater than `curr`.
                let next_in_seq = all.iter().find(|id| is_strictly_greater(id, curr)).cloned();
                next_in_seq.map(|n| format!("work.ready({n})"))
            }
        }
        (None, _) => Some("plan.complete".to_string()),
        (Some(_), None) => None,
    };
    Some(FixUnitState {
        total,
        completed,
        current,
        next_expected,
    })
}

/// Extract the `fix-NN` id from a free-form string (e.g. a value
/// pulled verbatim from `progress.md`'s `## Completed Steps` list).
/// Returns `None` for anything that does not match the canonical
/// `fix-<digits>` shape; never panics on oddly-shaped legacy entries.
fn fix_id_from_string(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if !trimmed.starts_with("fix-") || trimmed.len() <= 4 {
        return None;
    }
    if !trimmed.as_bytes()[4..].iter().all(|b| b.is_ascii_digit()) {
        return None;
    }
    Some(trimmed.to_string())
}

/// Public entry point for the event loop: collect the set of
/// `fix-NN` ids currently observed in `tasks.jsonl`.  Used by
/// the fix-unit emit-gate (P1-1, 2026-07-01-002 audit) to reject
/// `work.ready(fix-XX)` events whose id is **not** in this set —
/// e.g. a stale coordinator emitting `work.ready(fix-03)` after
/// the chain only ever had `fix-01`/`fix-02`.  When the
/// projector cache is empty we fall back to a single disk read,
/// which is bounded (`tasks.jsonl` is small).
pub fn fix_unit_known_ids(
    projector: &crate::state_projector::StateProjector,
) -> std::collections::BTreeSet<String> {
    let ctx = projector.context();
    let (tasks_ref, _from_ledger) = ctx.task_snapshot();
    let tasks = if tasks_ref.is_empty() {
        let (disk_tasks, _prog) = crate::state_projector::read_state_from_disk(&ctx.workspace_root);
        disk_tasks
    } else {
        tasks_ref.to_vec()
    };
    let mut ids = std::collections::BTreeSet::new();
    for t in &tasks {
        let Some(k) = t.key.as_deref() else { continue };
        if is_fix_unit_key(k) {
            if let Some(id) = fix_id_from_key(k) {
                ids.insert(id);
            }
        }
    }
    ids
}

/// Extract the `fix-NN` id segment from a key shaped like
/// `<plan>:step:fix-NN:uNN…`.  Returns `None` for keys that don't
/// match the canonical shape.
fn fix_id_from_key(key: &str) -> Option<String> {
    key.split(':').find_map(|seg| {
        if seg.starts_with("fix-")
            && seg.len() > 4
            && seg.as_bytes()[4..].iter().all(|b| b.is_ascii_digit())
        {
            Some(seg.to_string())
        } else {
            None
        }
    })
}

/// Compare two fix-unit ids by their trailing numeric suffix.
/// `fix-01` < `fix-02`, `fix-09` < `fix-10`, etc.  Lexical sort is
/// wrong when ids reach double digits, hence the parse.
fn is_strictly_greater(candidate: &str, baseline: &str) -> bool {
    let cand_n = trailing_digits(candidate);
    let base_n = trailing_digits(baseline);
    match (cand_n, base_n) {
        (Some(c), Some(b)) => c > b,
        // If either side cannot be parsed, fall back to lexical so
        // the snapshot still surfaces a hint instead of failing
        // silently.
        _ => candidate > baseline,
    }
}

fn trailing_digits(s: &str) -> Option<u32> {
    let idx = s.find('-')?;
    s[idx + 1..].parse::<u32>().ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StateProjectionConfig;
    use crate::state_projector::ProjectionContext;
    use tempfile::TempDir;

    fn workspace() -> TempDir {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".ralph").join("agent")).unwrap();
        tmp
    }

    #[test]
    fn disabled_stub_explains_itself() {
        let snap = RuntimeStateSnapshot::disabled_stub();
        let block = snap.to_prompt_block();
        assert!(block.starts_with(ORCHESTRATOR_CONTEXT_HEADING));
        assert!(block.contains("State projection is **disabled**"));
    }

    #[test]
    fn empty_projector_yields_minimal_block() {
        let tmp = workspace();
        let cfg = StateProjectionConfig::default();
        let proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let snap = RuntimeStateSnapshot::build(&proj);
        // projection_disabled reflects the config (default false
        // because config.enabled defaults to false → !false = true
        // → we mark the snapshot as "disabled" so the agent sees
        // the stub explanation).
        assert!(snap.projection_disabled);
        assert!(snap.to_prompt_block().contains("(none)"));
    }

    #[test]
    fn snapshot_from_disk_reads_canonical_layout() {
        let tmp = workspace();
        let progress_path = tmp.path().join(".ralph/agent/progress.md");
        // U1 (KTD-1): `current_step` is derived from
        // `completed_steps.last()` — keep the on-disk headings aligned
        // with that SSOT so `snapshot_from_disk` is self-consistent.
        std::fs::write(
            &progress_path,
            "## Current Step\nstep-02\n\n## Completed Steps\n- step-01\n- step-02\n",
        )
        .unwrap();
        let snap = snapshot_from_disk(tmp.path());
        assert_eq!(snap.current_step.as_deref(), Some("step-02"));
        assert_eq!(snap.completed_steps, vec!["step-01", "step-02"]);
    }

    #[test]
    fn prompt_block_contains_heading_and_values() {
        let snap = RuntimeStateSnapshot {
            plan_name: Some("feat-xy".to_string()),
            current_step: Some("step-04".to_string()),
            completed_steps: vec!["step-01".to_string(), "step-02".to_string()],
            open_tasks: vec![OpenTaskSummary {
                id: "t-1".to_string(),
                title: "U1-impl".to_string(),
                status: "open".to_string(),
            }],
            wave: None,
            loop_start_sha: None,
            plan_baseline_sha: None,
            projection_disabled: false,
            fix_unit_state: None,
            review_dimensions: None,
        };
        let block = snap.to_prompt_block();
        assert!(block.starts_with(ORCHESTRATOR_CONTEXT_HEADING));
        assert!(block.contains("plan_name: feat-xy"));
        assert!(block.contains("current_step: step-04"));
        assert!(block.contains("step-01, step-02"));
        assert!(block.contains("t-1"));
        assert!(block.contains("plan_baseline_sha: (none)"));
        assert!(block.contains("loop_start_sha: (none)"));
    }

    #[test]
    fn prompt_block_renders_git_baselines() {
        let snap = RuntimeStateSnapshot {
            plan_baseline_sha: Some("plansha12345678901234567890123456789012345678".to_string()),
            loop_start_sha: Some("loopsha1234567890123456789012345678901234567".to_string()),
            ..RuntimeStateSnapshot::default()
        };
        let block = snap.to_prompt_block();
        assert!(block.contains("plan_baseline_sha: plansha12345678901234567890123456789012345678"));
        assert!(block.contains("loop_start_sha: loopsha1234567890123456789012345678901234567"));
    }

    #[test]
    fn heading_constant_is_stable() {
        // Lock the literal so log scrapers can match a single
        // fixed string. A future rename must be coordinated with
        // the docs in U4.
        assert_eq!(ORCHESTRATOR_CONTEXT_HEADING, "## ORCHESTRATOR CONTEXT");
    }

    // P2 fix (review 2026-06-17-003): derive_plan_name must read
    // the plan from the canonical key shape
    // `ce-executor:<plan>:<step>:<unit>`, not the free-form
    // `description` field. The legacy description-based path
    // could be hijacked by an agent that overwrites the
    // description; the key is stamped by the projector itself
    // and never modified.
    fn task_with(key: Option<&str>, description: Option<&str>) -> Task {
        let mut t = Task::new("step-01".to_string(), 1);
        t.key = key.map(|s| s.to_string());
        t.description = description.map(|s| s.to_string());
        t
    }

    fn fix_unit_task_with(step_id: &str, status: TaskStatus) -> Task {
        // Mirrors the projector-generated key shape
        // `<plan>:step:<step_id>:<unit>-impl`.  We pick a stable
        // plan name so the tests can group multiple fix-units under
        // the same plan.
        let mut t = Task::new(format!("fix-unit {step_id}"), 1);
        t.key = Some(format!("ce-executor:test-plan:step:{step_id}:u1-impl"));
        t.status = status;
        t
    }

    #[test]
    fn derive_plan_name_reads_canonical_key() {
        let tasks = vec![
            task_with(Some("ce-executor:demo-plan:step-01:u1-impl"), None),
            task_with(Some("ce-executor:demo-plan:step-02:u2-impl"), None),
        ];
        // The reverse iterator picks the most recent task; the
        // plan name is the second `:` segment of its key.
        let snap = RuntimeStateSnapshot {
            plan_name: derive_plan_name(&tasks),
            ..RuntimeStateSnapshot::default()
        };
        assert_eq!(snap.plan_name.as_deref(), Some("demo-plan"));
    }

    #[test]
    fn derive_plan_name_falls_back_to_legacy_key_only_when_safe() {
        // No canonical 4-segment keys at all → plan_name is
        // unknown. The agent sees `(none)`, which is correct
        // because the projector is the only writer of the
        // canonical shape.
        let tasks = vec![task_with(Some("legacy-key"), None)];
        let snap = RuntimeStateSnapshot {
            plan_name: derive_plan_name(&tasks),
            ..RuntimeStateSnapshot::default()
        };
        assert_eq!(snap.plan_name, None);
    }

    #[test]
    fn derive_plan_name_ignores_free_form_description() {
        // A task with a fake `plan: something` in its description
        // but a non-canonical key must NOT be picked up: the
        // legacy description-based path is gone, so the agent's
        // free-form text cannot poison the snapshot.
        let tasks = vec![task_with(
            Some("ce-executor:trusted-plan:step-01:u1-impl"),
            Some("plan: attacker-plan"),
        )];
        let snap = RuntimeStateSnapshot {
            plan_name: derive_plan_name(&tasks),
            ..RuntimeStateSnapshot::default()
        };
        assert_eq!(snap.plan_name.as_deref(), Some("trusted-plan"));
    }

    // U3 (2026-07-01-002 plan) tests: fix_unit_state derivation and
    // prompt-block rendering.

    #[test]
    fn fix_unit_state_returns_none_when_no_fix_tasks() {
        // A plan with only non-fix tasks must not advertise a
        // fix-phase.  Coordinator sees `(none)` and keeps its old
        // logic.
        let tasks = vec![task_with(
            Some("ce-executor:test-plan:step-01:u1-impl"),
            None,
        )];
        assert!(derive_fix_unit_state(&tasks, &[]).is_none());
    }

    #[test]
    fn fix_unit_state_marks_last_unit_with_plan_complete() {
        let tasks = vec![
            fix_unit_task_with("fix-01", TaskStatus::Closed),
            fix_unit_task_with("fix-02", TaskStatus::Open),
        ];
        let state = derive_fix_unit_state(&tasks, &[]).expect("fix-unit state");
        assert_eq!(state.total, 2);
        assert_eq!(state.completed, vec!["fix-01".to_string()]);
        assert_eq!(state.current.as_deref(), Some("fix-02"));
        assert_eq!(state.next_expected.as_deref(), Some("plan.complete"));
    }

    #[test]
    fn fix_unit_state_next_expected_is_work_ready_for_middle() {
        let tasks = vec![
            fix_unit_task_with("fix-01", TaskStatus::Closed),
            fix_unit_task_with("fix-02", TaskStatus::Open),
            fix_unit_task_with("fix-03", TaskStatus::Open),
        ];
        let state = derive_fix_unit_state(&tasks, &[]).expect("fix-unit state");
        assert_eq!(state.total, 3);
        assert_eq!(state.current.as_deref(), Some("fix-02"));
        assert_eq!(state.next_expected.as_deref(), Some("work.ready(fix-03)"));
    }

    #[test]
    fn fix_unit_state_handles_double_digit_sorting() {
        // Lexical sort would mis-rank `fix-10` below `fix-02`, so
        // we must use numeric ordering.
        let tasks = vec![
            fix_unit_task_with("fix-02", TaskStatus::Open),
            fix_unit_task_with("fix-10", TaskStatus::Open),
            fix_unit_task_with("fix-01", TaskStatus::Closed),
        ];
        let state = derive_fix_unit_state(&tasks, &[]).expect("fix-unit state");
        assert_eq!(state.total, 3);
        assert_eq!(state.completed, vec!["fix-01".to_string()]);
        assert_eq!(state.current.as_deref(), Some("fix-02"));
        // After fix-02 we expect work.ready(fix-10), skipping the
        // notional fix-03..fix-09 ids (the snapshot is descriptive,
        // not prescriptive — the coordinator decides whether to
        // spawn a fix-03).
        assert_eq!(state.next_expected.as_deref(), Some("work.ready(fix-10)"));
    }

    #[test]
    fn fix_unit_state_handles_no_first_open_when_all_terminal() {
        // Two terminal fix-units and nothing more → next_expected
        // defaults to plan.complete (the loop is past fix-phase).
        let tasks = vec![
            fix_unit_task_with("fix-01", TaskStatus::Closed),
            fix_unit_task_with("fix-02", TaskStatus::Failed),
        ];
        let state = derive_fix_unit_state(&tasks, &[]).expect("fix-unit state");
        assert_eq!(state.completed, vec!["fix-01", "fix-02"]);
        assert_eq!(state.current, None);
        assert_eq!(state.next_expected.as_deref(), Some("plan.complete"));
    }

    #[test]
    fn fix_unit_state_prompt_block_contains_all_fields() {
        let snap = RuntimeStateSnapshot {
            fix_unit_state: Some(FixUnitState {
                total: 2,
                completed: vec!["fix-01".to_string()],
                current: Some("fix-02".to_string()),
                next_expected: Some("plan.complete".to_string()),
            }),
            ..RuntimeStateSnapshot::default()
        };
        let block = snap.to_prompt_block();
        assert!(block.contains("fix_unit_state:"), "block: {block}");
        assert!(block.contains("total: 2"));
        assert!(block.contains("completed: fix-01"));
        assert!(block.contains("current: fix-02"));
        assert!(block.contains("next_expected: plan.complete"));
    }

    #[test]
    fn fix_unit_state_prompt_block_handles_no_fix_state() {
        let snap = RuntimeStateSnapshot::default();
        let block = snap.to_prompt_block();
        assert!(block.contains("fix_unit_state: (none"));
    }

    #[test]
    fn is_strictly_greater_handles_double_digits() {
        assert!(is_strictly_greater("fix-10", "fix-02"));
        assert!(!is_strictly_greater("fix-02", "fix-02"));
        assert!(!is_strictly_greater("fix-01", "fix-10"));
    }

    #[test]
    fn fix_id_from_key_extracts_canonical_fix_segment() {
        assert_eq!(
            fix_id_from_key("ce-executor:test-plan:step:fix-07:u1-impl"),
            Some("fix-07".to_string())
        );
        // Legacy / odd shapes must not panic.
        assert_eq!(fix_id_from_key("nonsense"), None);
        assert_eq!(fix_id_from_key("ce-executor:plan:step:step-99:u1"), None);
    }

    #[test]
    fn progress_completed_steps_takes_precedence_over_task_status() {
        // P1-2 (2026-07-01-002 audit): when progress.md lists fix-02
        // as completed but tasks.jsonl still has it Open, we must
        // trust progress.  Otherwise the coordinator would emit a
        // stray work.ready(fix-03) during the brief window between
        // test.passed landing and the projector closing the task.
        let tasks = vec![
            fix_unit_task_with("fix-01", TaskStatus::Closed),
            // status Open: simulates the race window.
            fix_unit_task_with("fix-02", TaskStatus::Open),
        ];
        let progress_done = vec!["fix-02".to_string()];
        let state = derive_fix_unit_state(&tasks, &progress_done).expect("fix-unit state");
        assert_eq!(state.completed, vec!["fix-01", "fix-02"]);
        assert_eq!(state.current, None);
        assert_eq!(state.next_expected.as_deref(), Some("plan.complete"));
    }

    #[test]
    fn progress_completed_steps_partial_progress_is_mirrored() {
        // Only fix-01 is in progress; fix-02 stays current even
        // though task status is also Open for both.  The two Open
        // entries must NOT both be promoted to completed — only
        // the one progress.md acknowledges.  With total=2 and
        // current=last_id, `next_expected` is `plan.complete`.
        let tasks = vec![
            fix_unit_task_with("fix-01", TaskStatus::Open),
            fix_unit_task_with("fix-02", TaskStatus::Open),
        ];
        let progress_done = vec!["fix-01".to_string()];
        let state = derive_fix_unit_state(&tasks, &progress_done).expect("fix-unit state");
        assert_eq!(state.completed, vec!["fix-01"]);
        assert_eq!(state.current.as_deref(), Some("fix-02"));
        // total=2, current=last → must emit plan.complete.
        assert_eq!(state.next_expected.as_deref(), Some("plan.complete"));
    }

    #[test]
    fn progress_completed_steps_ignores_non_fix_entries() {
        // Non-fix-unit entries in progress.md (e.g. step-01) must
        // not pollute the fix-unit completed list.
        let tasks = vec![fix_unit_task_with("fix-01", TaskStatus::Open)];
        let progress_done = vec![
            "step-01".to_string(),
            "fix-01".to_string(),
            "trivial".to_string(),
        ];
        let state = derive_fix_unit_state(&tasks, &progress_done).expect("fix-unit state");
        assert_eq!(state.completed, vec!["fix-01"]);
    }

    #[test]
    fn fix_id_from_string_accepts_bare_fix_nn() {
        assert_eq!(fix_id_from_string("fix-02"), Some("fix-02".to_string()));
        assert_eq!(fix_id_from_string(" fix-02 "), Some("fix-02".to_string()));
        assert_eq!(fix_id_from_string("step-02"), None);
        assert_eq!(fix_id_from_string("fix-ab"), None);
        assert_eq!(fix_id_from_string(""), None);
    }
}
