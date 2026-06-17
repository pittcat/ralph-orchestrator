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
use crate::task::Task;
use crate::task_store::TaskStore;
use crate::step_handoff::progress_task_gate::ProgressSnapshot;

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
    /// True when state projection is disabled for this run; the
    /// agent is told so it does not invent its own ledger.
    pub projection_disabled: bool,
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
        let progress = &ctx.progress_cache;
        let tasks = if ctx.tasks_cache.is_empty() {
            // Cold cache; try disk before giving up.
            TaskStore::load(&ctx.tasks_path)
                .map(|s| s.all().to_vec())
                .unwrap_or_default()
        } else {
            ctx.tasks_cache.clone()
        };
        Self {
            plan_name: derive_plan_name(&tasks),
            current_step: progress.current_step.clone(),
            completed_steps: progress.completed_steps.clone(),
            open_tasks: open_task_summaries(&tasks),
            wave: None, // U4 spike deferred: wave sub-section is
                        //            duplicated with `## WAVE
                        //            CONTEXT`. We omit the
                        //            duplicate until U4 spike picks
                        //            one. Both blocks remain
                        //            available side-by-side.
            projection_disabled: !ctx.config.enabled,
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
            projection_disabled: true,
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
            let _ = writeln!(buf, "- completed_steps: {}", self.completed_steps.join(", "));
        }
        if self.open_tasks.is_empty() {
            let _ = writeln!(buf, "- open_tasks: (none)");
        } else {
            let _ = writeln!(buf, "- open_tasks:");
            for task in &self.open_tasks {
                let _ = writeln!(
                    buf,
                    "  - {} [{}] {}",
                    task.id, task.status, task.title
                );
            }
        }
        if let Some(wave) = &self.wave {
            let _ = writeln!(
                buf,
                "- wave: id={} received={}/{}",
                wave.wave_id, wave.received, wave.total
            );
        }
        let _ = writeln!(buf);
        buf
    }
}

fn derive_plan_name(tasks: &[Task]) -> Option<String> {
    // Phase 1 heuristic: pick the most-recently-touched task's
    // description prefix `plan: <name>` (set by the projector on
    // ensure). When no tasks exist, return None — the agent will
    // see `(none)`.
    tasks
        .iter()
        .rev()
        .find_map(|t| t.description.as_ref())
        .and_then(|d| d.strip_prefix("plan: ").map(|s| s.to_string()))
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
    let tasks_path = crate::state_projector::tasks_path(workspace);
    let progress_path = crate::state_projector::progress_path(workspace);
    let tasks = TaskStore::load(&tasks_path)
        .map(|s| s.all().to_vec())
        .unwrap_or_default();
    let content = std::fs::read_to_string(&progress_path).unwrap_or_default();
    let progress = ProgressSnapshot::parse(&content);
    RuntimeStateSnapshot {
        plan_name: derive_plan_name(&tasks),
        current_step: progress.current_step,
        completed_steps: progress.completed_steps,
        open_tasks: open_task_summaries(&tasks),
        wave: None,
        projection_disabled: true,
    }
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
        let proj = StateProjector::new(ProjectionContext::new(tmp.path(), cfg));
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
        std::fs::write(
            &progress_path,
            "## Current Step\nstep-03\n\n## Completed Steps\n- step-01\n- step-02\n",
        )
        .unwrap();
        let snap = snapshot_from_disk(tmp.path());
        assert_eq!(snap.current_step.as_deref(), Some("step-03"));
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
            projection_disabled: false,
        };
        let block = snap.to_prompt_block();
        assert!(block.starts_with(ORCHESTRATOR_CONTEXT_HEADING));
        assert!(block.contains("plan_name: feat-xy"));
        assert!(block.contains("current_step: step-04"));
        assert!(block.contains("step-01, step-02"));
        assert!(block.contains("t-1"));
    }

    #[test]
    fn heading_constant_is_stable() {
        // Lock the literal so log scrapers can match a single
        // fixed string. A future rename must be coordinated with
        // the docs in U4.
        assert_eq!(ORCHESTRATOR_CONTEXT_HEADING, "## ORCHESTRATOR CONTEXT");
    }
}
