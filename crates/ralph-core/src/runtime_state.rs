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
    /// 2026-06-18-005 U3 (R3): hat_handoff 已 accept 的 seq
    /// (`LoopState.hat_handoff_seq`)。None when `hat_handoff.enabled=false`。
    pub hat_handoff_seq: Option<u32>,
    /// 2026-06-18-005 U3 (R3): 下一个应使用的 seq
    /// (= `hat_handoff_seq + 1`)。None when disabled。
    pub hat_handoff_next_seq: Option<u32>,
    /// 2026-06-18-005 U3 (R3): handoff 文件目录,固定
    /// `.ralph/agent/hat-handoff`。None when disabled。
    pub hat_handoff_dir: Option<String>,
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
    ///
    /// `hat_handoff_state` carries `(enabled, current_seq)` from
    /// `LoopState` so the snapshot can expose the U3 handoff fields
    /// without coupling `runtime_state` to the event loop. Pass
    /// `None` to omit handoff fields (default / disabled / tests).
    pub fn build(
        projector: &StateProjector,
        hat_handoff_state: Option<HandoffSnapshotState>,
    ) -> Self {
        let ctx = projector.context();
        // Read through the dual-source accessors: the U2 path
        // returns the wired `LedgerSnapshot` when present; the
        // legacy path returns the `tasks_cache` / `progress_cache`
        // mirrors. Both are kept in sync by every `apply` /
        // `apply_from_ledger` call.
        let (tasks_ref, _from_ledger) = ctx.task_snapshot();
        let (progress_ref, _from_ledger) = ctx.progress_snapshot();
        let (tasks, progress) = if tasks_ref.is_empty() && progress_ref.current_step.is_none()
            && progress_ref.completed_steps.is_empty()
        {
            // Cold cache; try disk before giving up. The progress
            // path is already cached (or empty), so the disk read
            // for tasks is the only fall-through.
            crate::state_projector::read_state_from_disk(&ctx.workspace_root)
        } else {
            (tasks_ref.to_vec(), progress_ref.clone())
        };
        let handoff = hat_handoff_state.and_then(|h| h.into_fields());
        Self {
            plan_name: derive_plan_name(&tasks),
            current_step: progress.current_step,
            completed_steps: progress.completed_steps,
            open_tasks: open_task_summaries(&tasks),
            wave: None, // U4 spike deferred: wave sub-section is
            //            duplicated with `## WAVE
            //            CONTEXT`. We omit the
            //            duplicate until U4 spike picks
            //            one. Both blocks remain
            //            available side-by-side.
            projection_disabled: !ctx.config.enabled,
            hat_handoff_seq: handoff.as_ref().map(|(s, _)| *s),
            hat_handoff_next_seq: handoff.as_ref().map(|(_, n)| *n),
            hat_handoff_dir: handoff.map(|_| HAT_HANDOFF_DEFAULT_DIR.to_string()),
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
            hat_handoff_seq: None,
            hat_handoff_next_seq: None,
            hat_handoff_dir: None,
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
        if let Some(wave) = &self.wave {
            let _ = writeln!(
                buf,
                "- wave: id={} received={}/{}",
                wave.wave_id, wave.received, wave.total
            );
        }
        // 2026-06-18-005 U3 (R3): hat_handoff 三行,enabled 时输出。
        if let (Some(seq), Some(next), Some(dir)) = (
            self.hat_handoff_seq,
            self.hat_handoff_next_seq,
            self.hat_handoff_dir.as_ref(),
        ) {
            let _ = writeln!(buf, "- hat_handoff_seq: {seq}");
            let _ = writeln!(buf, "- hat_handoff_next_seq: {next}");
            let _ = writeln!(buf, "- hat_handoff_dir: {dir}");
        }
        let _ = writeln!(buf);
        buf
    }
}

/// 2026-06-18-005 U3 (R3): handoff 状态输入参数。
///
/// caller(`EventLoop::prepend_orchestrator_context`)负责把
/// `LoopState.hat_handoff_seq` 与 `event_loop.hat_handoff.enabled`
/// 折叠成这个轻量结构,避免 `runtime_state` 反向依赖 event_loop。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HandoffSnapshotState {
    pub enabled: bool,
    pub current_seq: u32,
}

impl HandoffSnapshotState {
    /// `None` 表示 handoff 字段完全不输出(enabled=false)。
    /// `Some((current_seq, current_seq + 1))` 表示输出三行。
    fn into_fields(self) -> Option<(u32, u32)> {
        if self.enabled {
            Some((self.current_seq, self.current_seq + 1))
        } else {
            None
        }
    }
}

/// 2026-06-18-005 U3 (R3): handoff 文件目录常量,SSOT 之一。
///
/// 与 `hat_handoff::allocator::DEFAULT_HANDOFF_DIR` 同值,这里
/// 复制一份常量字符串避免 runtime_state 依赖 allocator(防止循环依赖)。
pub const HAT_HANDOFF_DEFAULT_DIR: &str = ".ralph/agent/hat-handoff";

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
    RuntimeStateSnapshot {
        plan_name: derive_plan_name(&tasks),
        current_step: progress.current_step,
        completed_steps: progress.completed_steps,
        open_tasks: open_task_summaries(&tasks),
        wave: None,
        projection_disabled: true,
        hat_handoff_seq: None,
        hat_handoff_next_seq: None,
        hat_handoff_dir: None,
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
        let proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let snap = RuntimeStateSnapshot::build(&proj, None);
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
            hat_handoff_seq: None,
            hat_handoff_next_seq: None,
            hat_handoff_dir: None,
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

    // 2026-06-18-005 U3 (R3): hat_handoff 三行在 enabled 时输出,
    // disabled 时不出现。

    #[test]
    fn handoff_enabled_emits_three_lines() {
        let tmp = workspace();
        let cfg = StateProjectionConfig::default();
        let proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let snap = RuntimeStateSnapshot::build(
            &proj,
            Some(HandoffSnapshotState {
                enabled: true,
                current_seq: 1,
            }),
        );
        assert_eq!(snap.hat_handoff_seq, Some(1));
        assert_eq!(snap.hat_handoff_next_seq, Some(2));
        assert_eq!(
            snap.hat_handoff_dir.as_deref(),
            Some(".ralph/agent/hat-handoff")
        );
        let block = snap.to_prompt_block();
        assert!(block.contains("hat_handoff_seq: 1"));
        assert!(block.contains("hat_handoff_next_seq: 2"));
        assert!(block.contains("hat_handoff_dir: .ralph/agent/hat-handoff"));
    }

    #[test]
    fn handoff_disabled_omits_three_lines() {
        let tmp = workspace();
        let cfg = StateProjectionConfig::default();
        let proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let snap = RuntimeStateSnapshot::build(
            &proj,
            Some(HandoffSnapshotState {
                enabled: false,
                current_seq: 0,
            }),
        );
        assert_eq!(snap.hat_handoff_seq, None);
        assert_eq!(snap.hat_handoff_next_seq, None);
        assert_eq!(snap.hat_handoff_dir, None);
        let block = snap.to_prompt_block();
        assert!(!block.contains("hat_handoff_seq"));
        assert!(!block.contains("hat_handoff_next_seq"));
        assert!(!block.contains("hat_handoff_dir"));
    }

    #[test]
    fn handoff_state_none_omits_three_lines() {
        let tmp = workspace();
        let cfg = StateProjectionConfig::default();
        let proj = StateProjector::new(ProjectionContext::new_legacy(tmp.path(), cfg));
        let snap = RuntimeStateSnapshot::build(&proj, None);
        assert!(snap.hat_handoff_seq.is_none());
        let block = snap.to_prompt_block();
        assert!(!block.contains("hat_handoff_seq"));
    }

    #[test]
    fn handoff_disabled_stub_omits_fields() {
        let snap = RuntimeStateSnapshot::disabled_stub();
        assert!(snap.hat_handoff_seq.is_none());
        assert!(snap.hat_handoff_next_seq.is_none());
        assert!(snap.hat_handoff_dir.is_none());
    }
}
