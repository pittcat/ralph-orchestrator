//! 2026-07-23-007 plan U4 (R-W5): project each supervisor slot's
//! lifecycle onto a stable `tasks.jsonl` row so the operator
//! surface (`ralph tools task list`) sees the same start / done /
//! failed transitions as the supervisor store. The slot → task
//! mapping is deterministic: every slot gets a stable
//! `task_key = format!("supervisor:{loop_id}:wave-{wave_id}:slot-{slot_index}")`,
//! re-derived on every projection call. The projector is the
//! SOLE writer of these rows — worker processes never touch
//! `tasks.jsonl` directly.
//!
//! Idempotency: the projector always loads the existing
//! `tasks.jsonl` before any mutation. `TaskStore::{start,close,fail}`
//! are no-ops when the task is already in the target state, so
//! repeated projection calls across slot re-report (U3
//! first-terminal-wins replay) and recover-on-restart produce
//! the same final task state without duplicate rows.
//!
//! Recovery: a crash between the supervisor-store mutation and
//! the `tasks.jsonl` write leaves the slot terminal in the store
//! but the task stuck at `started` (or worse, never started).
//! `recover_pending_projections` walks the active waves and
//! replays the terminal state; the projection is idempotent, so
//! a recovered state still produces the right `done` / `failed`
//! row.

use std::path::Path;

use ralph_core::supervisor::{SlotStatus, SupervisorStore, WaveSnapshot};
use ralph_core::{Task, TaskStore};

/// Stable task key for a (wave, slot) pair. Re-derived on every
/// projection call so the projector can rebuild the row on
/// recovery without persisting any extra mapping state.
pub fn slot_task_key(loop_id: &str, wave_id: &str, slot_index: u32) -> String {
    format!("supervisor:{loop_id}:wave-{wave_id}:slot-{slot_index}")
}

/// Outcome class used to pick the right `TaskStore` mutation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SlotProjection {
    Started,
    Completed,
    Failed,
}

/// Apply a slot projection to `tasks.jsonl`. Loads the store,
/// `ensure`s the task with the derived stable key, then mutates
/// its status. The call is idempotent: a re-projection of the
/// same outcome is a no-op. A panic or IO error is logged and
/// swallowed so the dispatch loop can continue (the supervisor
/// store is still the source of truth; recovery will replay on
/// the next `recover_pending_projections` call).
pub fn project_slot(
    tasks_path: &Path,
    loop_id: &str,
    wave_id: &str,
    slot_index: u32,
    projection: SlotProjection,
) {
    let task_key = slot_task_key(loop_id, wave_id, slot_index);
    let summary = format!("supervisor slot {slot_index} of wave {wave_id}");
    let mut store = match TaskStore::load(tasks_path) {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(
                path = %tasks_path.display(),
                %err,
                "U4: TaskStore::load failed; skipping slot projection"
            );
            return;
        }
    };
    let task = Task::new(summary, 3)
        .with_loop_id(Some(loop_id.to_string()))
        .with_key(Some(task_key.clone()));
    store.ensure(task);
    // Find the task id by stable key.
    let task_id = match store
        .all()
        .iter()
        .find(|t| t.key.as_deref() == Some(task_key.as_str()))
        .map(|t| t.id.clone())
    {
        Some(id) => id,
        None => {
            tracing::warn!(
                task_key,
                "U4: task not found after ensure; skipping slot projection"
            );
            return;
        }
    };
    // 2026-07-23-007 plan U4: `TaskStore::close` requires the task
    // to have been started first; a worker slot that completes
    // without an intermediate projection (e.g. first dispatch
    // straight to terminal) must still get `started` so the close
    // applies. `start` is idempotent — calling it on an
    // already-started task is a no-op.
    store.start(&task_id);
    let result = match projection {
        SlotProjection::Started => store.start(&task_id),
        SlotProjection::Completed => store.close(&task_id),
        SlotProjection::Failed => store.fail(&task_id),
    };
    // `start` / `close` / `fail` return `Option<&Task>` —
    // `None` means the task was already in the target state
    // (idempotent no-op); `Some` means we transitioned it.
    let _ = result;
    if let Err(err) = store.save() {
        tracing::warn!(
            path = %tasks_path.display(),
            %err,
            "U4: TaskStore::save failed; slot projection is lost (will recover on next replay)"
        );
    }
}

/// Walk every active wave's slots and project any terminal /
/// dispatched state that has not yet been reflected in
/// `tasks.jsonl`. The function is idempotent — running it twice
/// produces the same end state.
#[allow(dead_code)] // Wired by recovery startup (out-of-scope for executor).
pub fn recover_pending_projections(
    tasks_path: &Path,
    loop_id: &str,
    store: &dyn SupervisorStore,
) {
    let snapshots = match store.recover_active_waves() {
        Ok(s) => s,
        Err(err) => {
            tracing::warn!(
                %err,
                "U4: recover_pending_projections: store.recover_active_waves failed; skipping"
            );
            return;
        }
    };
    for snap in snapshots {
        replay_snapshot(tasks_path, loop_id, &snap);
    }
}

fn replay_snapshot(tasks_path: &Path, loop_id: &str, snap: &WaveSnapshot) {
    for (slot_index, status) in &snap.slots {
        let projection = match status {
            SlotStatus::Pending => continue,
            SlotStatus::Dispatched | SlotStatus::Running => Some(SlotProjection::Started),
            SlotStatus::Completed => Some(SlotProjection::Completed),
            SlotStatus::Failed | SlotStatus::Cancelled => Some(SlotProjection::Failed),
        };
        if let Some(p) = projection {
            project_slot(tasks_path, loop_id, &snap.wave_id, *slot_index, p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ralph_core::supervisor::{WaveKind, WavePhase, WaveSnapshot};

    #[test]
    fn slot_task_key_is_stable_across_calls() {
        let k1 = slot_task_key("loop-1", "w-1", 3);
        let k2 = slot_task_key("loop-1", "w-1", 3);
        assert_eq!(k1, k2);
        assert!(k1.starts_with("supervisor:loop-1:wave-w-1:slot-3"));
    }

    #[test]
    fn replay_snapshot_skips_pending_slots() {
        let snap = WaveSnapshot {
            wave_id: "w-1".to_string(),
            kind: WaveKind::Exec,
            phase: WavePhase::Collect,
            expected_total: 2,
            completed_count: 0,
            failed_count: 1,
            pending_count: 1,
            in_flight_count: 0,
            cancel_requested: false,
            merged_to_events: false,
            slots: vec![(0, SlotStatus::Failed), (1, SlotStatus::Pending)],
            started_at: std::time::SystemTime::UNIX_EPOCH,
        };
        let tmp = tempfile::tempdir().expect("temp dir");
        let tasks_path = tmp.path().join("tasks.jsonl");
        replay_snapshot(&tasks_path, "loop-1", &snap);
        let store = TaskStore::load(&tasks_path).expect("load");
        let keys: Vec<_> = store
            .all()
            .iter()
            .filter_map(|t| t.key.clone())
            .collect();
        assert!(
            !keys.iter().any(|k| k.contains("slot-1")),
            "Pending slot must not be projected; got {keys:?}"
        );
        assert!(
            keys.iter().any(|k| k.contains("slot-0")),
            "Failed slot must be projected; got {keys:?}"
        );
    }

    #[test]
    fn project_slot_is_idempotent() {
        let tmp = tempfile::tempdir().expect("temp dir");
        let tasks_path = tmp.path().join("tasks.jsonl");
        project_slot(&tasks_path, "loop-1", "w-1", 0, SlotProjection::Completed);
        project_slot(&tasks_path, "loop-1", "w-1", 0, SlotProjection::Completed);
        let store = TaskStore::load(&tasks_path).expect("load");
        assert_eq!(
            store.all().len(),
            1,
            "idempotent projection must not duplicate rows; got {:?}",
            store
                .all()
                .iter()
                .map(|t| (t.key.clone(), t.status))
                .collect::<Vec<_>>()
        );
    }
}