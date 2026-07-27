//! Standalone copy of the enum-string invariants from U2. Kept in
//! a dedicated module so the trait-definition file (`mod.rs`) can
//! stay focused on the public API without dragging test imports
//! into it; the in-memory store lives in `memory.rs` and the
//! rusqlite store will live in `rusqlite.rs` (U5).
//!
//! 2026-07-03-001 plan U2 scope: type-level invariants — Display
//! vs serde, enum snake_case serialization, snapshot round-trip.

#[cfg(test)]
mod tests {
    use crate::supervisor::{
        IsolationMode, SlotResource, SlotStatus, SupervisorStoreError, WaveDeliveryState, WaveKind,
        WavePhase, WaveSnapshot,
    };
    use std::time::SystemTime;

    #[test]
    fn wave_kind_serializes_to_snake_case_strings() {
        for (kind, expected) in [
            (WaveKind::Exec, "\"exec\""),
            (WaveKind::Fix, "\"fix\""),
            (WaveKind::Review, "\"review\""),
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            assert_eq!(json, expected, "WaveKind serialization mismatch");
        }
    }

    #[test]
    fn isolation_mode_serializes_to_snake_case_strings() {
        for (mode, expected) in [
            (IsolationMode::Worktree, "\"worktree\""),
            (IsolationMode::SharedReadonly, "\"shared_readonly\""),
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(json, expected, "IsolationMode serialization mismatch");
        }
    }

    #[test]
    fn wave_phase_serializes_to_snake_case_strings() {
        for (phase, expected) in [
            (WavePhase::Dispatch, "\"dispatch\""),
            (WavePhase::Collect, "\"collect\""),
            (WavePhase::Integrate, "\"integrate\""),
            (WavePhase::Done, "\"done\""),
            (WavePhase::Failed, "\"failed\""),
        ] {
            let json = serde_json::to_string(&phase).unwrap();
            assert_eq!(json, expected, "WavePhase serialization mismatch");
        }
    }

    #[test]
    fn slot_status_serializes_to_snake_case_strings() {
        for (status, expected) in [
            (SlotStatus::Pending, "\"pending\""),
            (SlotStatus::Dispatched, "\"dispatched\""),
            (SlotStatus::Running, "\"running\""),
            (SlotStatus::Completed, "\"completed\""),
            (SlotStatus::Failed, "\"failed\""),
            (SlotStatus::Cancelled, "\"cancelled\""),
        ] {
            let json = serde_json::to_string(&status).unwrap();
            assert_eq!(json, expected, "SlotStatus serialization mismatch");
        }
    }

    #[test]
    fn display_matches_serde_for_each_enum() {
        // The runtime diagnostics + log lines + JSON payloads all
        // assume `Display` and serde agree (e.g. `merged_to_events`
        // recovery is keyed on the phase string).
        assert_eq!(WaveKind::Exec.to_string(), "exec");
        assert_eq!(WaveKind::Fix.to_string(), "fix");
        assert_eq!(WaveKind::Review.to_string(), "review");
        assert_eq!(IsolationMode::Worktree.to_string(), "worktree");
        assert_eq!(IsolationMode::SharedReadonly.to_string(), "shared_readonly");
        assert_eq!(WavePhase::Dispatch.to_string(), "dispatch");
        assert_eq!(WavePhase::Collect.to_string(), "collect");
        assert_eq!(WavePhase::Integrate.to_string(), "integrate");
        assert_eq!(WavePhase::Done.to_string(), "done");
        assert_eq!(WavePhase::Failed.to_string(), "failed");
        assert_eq!(SlotStatus::Pending.to_string(), "pending");
        assert_eq!(SlotStatus::Dispatched.to_string(), "dispatched");
        assert_eq!(SlotStatus::Running.to_string(), "running");
        assert_eq!(SlotStatus::Completed.to_string(), "completed");
        assert_eq!(SlotStatus::Failed.to_string(), "failed");
        assert_eq!(SlotStatus::Cancelled.to_string(), "cancelled");
    }

    #[test]
    fn slot_resource_distinguishes_worktree_vs_shared_readonly() {
        let exec_binding = SlotResource {
            slot_index: 0,
            worktree_path: Some(".ralph/worktrees/u1".to_string()),
            branch: Some("ralph/u1".to_string()),
        };
        assert!(!exec_binding.is_shared_readonly());
        let review_binding = SlotResource {
            slot_index: 0,
            worktree_path: None,
            branch: None,
        };
        assert!(review_binding.is_shared_readonly());
    }

    #[test]
    fn supervisor_store_error_renders_context() {
        // The runtime + tests consume `Display` for context
        // messages (`task.resume` payload violation field), so the
        // error variants must carry the data through Display.
        let err = SupervisorStoreError::UnknownSlot {
            wave_id: "w-1".to_string(),
            slot_index: 2,
        };
        let rendered = err.to_string();
        assert!(
            rendered.contains("w-1") && rendered.contains('2'),
            "Display must carry slot context; got {rendered}"
        );
    }

    #[test]
    fn wave_snapshot_round_trips_through_serde() {
        let snapshot = WaveSnapshot {
            wave_id: "w-snap".to_string(),
            kind: WaveKind::Exec,
            phase: WavePhase::Collect,
            expected_total: 5,
            completed_count: 2,
            failed_count: 1,
            pending_count: 2,
            in_flight_count: 0,
            cancel_requested: false,
            delivery_state: WaveDeliveryState::CoordinationCommitted,
            started_at: SystemTime::UNIX_EPOCH,
            slots: vec![(0, SlotStatus::Completed), (1, SlotStatus::Failed)],
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let back: WaveSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(back, snapshot);
    }
}
