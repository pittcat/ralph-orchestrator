//! U2 (plan 2026-06-21-002): build the `## ORCHESTRATOR CONTEXT`
//! prompt block from a [`LedgerSnapshot`].
//!
//! The block is byte-identical to the legacy
//! `RuntimeStateSnapshot::to_prompt_block()` rendering
//! (`runtime_state.rs`). U2 adds a snapshot-driven path so the
//! loop can read from the unified ledger instead of the
//! projector's in-memory `tasks_cache` / `progress_cache`
//! mirrors.
//!
//! The rendering lives in this module (rather than
//! `state_projector/mod.rs`) so a future Phase 2 PR that
//! refactors the prompt template can edit one file without
//! touching the projector or the event loop.

use std::fmt::Write as _;

use crate::config::StateProjectionConfig;
use crate::state::LedgerSnapshot;
use crate::step_handoff::ProgressSnapshot;
use crate::task::Task;

const ORCHESTRATOR_CONTEXT_HEADING: &str = "## ORCHESTRATOR CONTEXT";

/// Build the full `## ORCHESTRATOR CONTEXT` prompt block from a
/// [`LedgerSnapshot`]. Returns the same shape
/// `RuntimeStateSnapshot::to_prompt_block()` produces, so
/// `runtime_state_injection.rs` integration tests remain valid
/// when the loop switches to the U2 read path.
pub(crate) fn build_block(
    snapshot: &LedgerSnapshot,
    config: &StateProjectionConfig,
    loop_start_sha: Option<&str>,
    plan_baseline_sha: Option<&str>,
) -> String {
    let tasks = snapshot.tasks();
    let progress = snapshot.progress();
    let mut buf = String::new();
    let _ = writeln!(buf, "{ORCHESTRATOR_CONTEXT_HEADING}");
    let _ = writeln!(
        buf,
        "The orchestrator owns `.ralph/agent/tasks.jsonl` and `.ralph/agent/progress.md`; \
         treat the values below as canonical and do NOT hand-write either ledger."
    );
    let _ = writeln!(buf);
    if !config.enabled {
        let _ = writeln!(
            buf,
            "State projection is **disabled** for this preset; the values below are the \
             last known view from the projector (may be stale)."
        );
        let _ = writeln!(buf);
    }
    if let Some(plan) = derive_plan_name(tasks) {
        let _ = writeln!(buf, "- plan_name: {plan}");
    } else {
        let _ = writeln!(buf, "- plan_name: (none)");
    }
    match &progress.current_step {
        Some(step) => {
            let _ = writeln!(buf, "- current_step: {step}");
        }
        None => {
            let _ = writeln!(buf, "- current_step: (none)");
        }
    }
    if progress.completed_steps.is_empty() {
        let _ = writeln!(buf, "- completed_steps: (none)");
    } else {
        let _ = writeln!(
            buf,
            "- completed_steps: {}",
            progress.completed_steps.join(", ")
        );
    }
    render_open_tasks(&mut buf, tasks);
    if let Some(sha) = plan_baseline_sha {
        let _ = writeln!(buf, "- plan_baseline_sha: {sha}");
    } else {
        let _ = writeln!(buf, "- plan_baseline_sha: (none)");
    }
    if let Some(sha) = loop_start_sha {
        let _ = writeln!(buf, "- loop_start_sha: {sha}");
    } else {
        let _ = writeln!(buf, "- loop_start_sha: (none)");
    }
    let _ = writeln!(buf);
    buf
}

fn render_open_tasks(buf: &mut String, tasks: &[Task]) {
    if tasks.is_empty() {
        let _ = writeln!(buf, "- open_tasks: (none)");
        return;
    }
    let open: Vec<&Task> = tasks.iter().filter(|t| !t.status.is_terminal()).collect();
    if open.is_empty() {
        let _ = writeln!(buf, "- open_tasks: (none)");
        return;
    }
    let _ = writeln!(buf, "- open_tasks:");
    for task in open {
        let _ = writeln!(
            buf,
            "  - {} [{}] {}",
            task.id,
            format!("{:?}", task.status).to_lowercase(),
            task.title,
        );
    }
}

/// Replicates `derive_plan_name` from `runtime_state.rs`. We
/// keep a local copy because the original is private to that
/// module; the snippet is small and the SSOT (the `key` shape)
/// is documented at the original site.
fn derive_plan_name(tasks: &[Task]) -> Option<String> {
    tasks
        .iter()
        .rev()
        .filter_map(|t| t.key.as_deref())
        .find_map(|key| {
            let mut parts = key.splitn(4, ':');
            let prefix = parts.next()?;
            if prefix != "ce-executor" {
                return None;
            }
            let plan = parts.next()?;
            if plan.is_empty() {
                return None;
            }
            if parts.next().is_none() || parts.next().is_none() {
                return None;
            }
            Some(plan.to_string())
        })
}

/// Return the heading constant for tests that match the literal
/// (mirrors `runtime_state::ORCHESTRATOR_CONTEXT_HEADING`).
#[allow(dead_code)]
pub(crate) fn heading() -> &'static str {
    ORCHESTRATOR_CONTEXT_HEADING
}

/// Render a `ProgressSnapshot` to the legacy `progress.md`
/// dialect. Exposed for tests that want to assert the on-disk
/// shape without going through [`super::progress::write_progress`].
#[allow(dead_code)]
pub(crate) fn render_progress_markdown(snap: &ProgressSnapshot) -> String {
    let mut buf = String::from("# Progress\n\n");
    match &snap.current_step {
        Some(step) => {
            buf.push_str("## Current Step\n");
            buf.push_str(step);
            buf.push_str("\n\n");
        }
        None => {
            buf.push_str("## Current Step\n(none)\n\n");
        }
    }
    buf.push_str("## Completed Steps\n");
    if snap.completed_steps.is_empty() {
        buf.push_str("(none)\n");
    } else {
        for step in &snap.completed_steps {
            buf.push_str("- ");
            buf.push_str(step);
            buf.push('\n');
        }
    }
    buf
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::StateProjectionConfig;

    #[test]
    fn heading_constant_is_stable() {
        assert_eq!(heading(), "## ORCHESTRATOR CONTEXT");
    }

    #[test]
    fn build_block_uses_snapshot_tasks_and_progress() {
        let mut snap = LedgerSnapshot::cold_start();
        let mut task = Task::new("step-01".to_string(), 1);
        task.key = Some("ce-executor:demo-plan:step-01:u1-impl".to_string());
        task.id = "t-1".to_string();
        snap.tasks.push(task);
        snap.progress.current_step = Some("step-01".to_string());
        snap.progress.completed_steps.push("step-00".to_string());

        let block = build_block(&snap, &StateProjectionConfig::default(), None, None);
        assert!(block.starts_with("## ORCHESTRATOR CONTEXT"));
        assert!(block.contains("plan_name: demo-plan"));
        assert!(block.contains("current_step: step-01"));
        assert!(block.contains("step-00"));
        assert!(block.contains("t-1"));
        assert!(block.contains("plan_baseline_sha: (none)"));
        assert!(block.contains("loop_start_sha: (none)"));
    }

    #[test]
    fn build_block_handles_empty_snapshot() {
        let snap = LedgerSnapshot::cold_start();
        let block = build_block(&snap, &StateProjectionConfig::default(), None, None);
        assert!(block.contains("plan_name: (none)"));
        assert!(block.contains("current_step: (none)"));
        assert!(block.contains("completed_steps: (none)"));
        assert!(block.contains("open_tasks: (none)"));
    }

    #[test]
    fn build_block_marks_disabled_projection() {
        let snap = LedgerSnapshot::cold_start();
        let cfg = StateProjectionConfig {
            enabled: false,
            ..Default::default()
        };
        let block = build_block(&snap, &cfg, None, None);
        assert!(block.contains("State projection is **disabled**"));
    }

    #[test]
    fn build_block_renders_git_baselines() {
        let snap = LedgerSnapshot::cold_start();
        let block = build_block(
            &snap,
            &StateProjectionConfig::default(),
            Some("loopsha1234567890123456789012345678901234567"),
            Some("plansha12345678901234567890123456789012345678"),
        );
        assert!(block.contains("plan_baseline_sha: plansha12345678901234567890123456789012345678"));
        assert!(block.contains("loop_start_sha: loopsha1234567890123456789012345678901234567"));
    }

    #[test]
    fn render_progress_markdown_round_trips_basic() {
        let mut snap = ProgressSnapshot::default();
        snap.current_step = Some("step-02".to_string());
        snap.completed_steps.push("step-01".to_string());
        let md = render_progress_markdown(&snap);
        assert!(md.contains("## Current Step"));
        assert!(md.contains("step-02"));
        assert!(md.contains("- step-01"));
    }
}
