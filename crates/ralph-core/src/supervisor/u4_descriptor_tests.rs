//! 2026-07-27-004 plan U4 (R11-R16 / D5 / D6): bounded redrive
//! activation descriptor. The dispatcher registers the
//! ready-event snapshot at spawn time; `ralph run --resume`
//! consumes the same descriptor to spawn a worker through the
//! existing dispatcher seam. Tests verify the persistence +
//! fail-closed semantics:
//! - persist + take → Dispatchable.
//! - take on a slot without descriptor → DescriptorUnavailable.
//! - take with a digest that does not match → DescriptorConflict.
//! - persist on an unknown wave → UnknownWave.

#[cfg(test)]
mod tests {
    use crate::supervisor::{
        InMemorySupervisorStore, RedriveTakeOutcome, SlotDescriptor, SlotResource, SupervisorStore,
        SupervisorStoreError, WaveKind,
    };

    fn register_exec(store: &InMemorySupervisorStore, key: &str) -> String {
        let wave = store.register_wave(key, WaveKind::Exec, 2, 1).unwrap();
        store
            .bind_worktree(
                &wave,
                0,
                SlotResource {
                    slot_index: 0,
                    worktree_path: Some(".ralph/u4/0".to_string()),
                    branch: Some("ralph/u4-0".to_string()),
                },
            )
            .unwrap();
        store
            .bind_worktree(
                &wave,
                1,
                SlotResource {
                    slot_index: 1,
                    worktree_path: Some(".ralph/u4/1".to_string()),
                    branch: Some("ralph/u4-1".to_string()),
                },
            )
            .unwrap();
        wave
    }

    /// R11 + R14 + S11: persist + take on the SAME key + digest
    /// yields `Dispatchable`. The bounded snapshot does not leak
    /// the full agent stdout — only the topic, payload, kind,
    /// digest, and slot index.
    #[test]
    fn u4_persist_then_take_returns_dispatchable() {
        let store = InMemorySupervisorStore::new();
        let wave = register_exec(&store, "u4-s11");

        let payload = r#"{"content_hash":"h","dimension":"default"}"#;
        let descriptor = SlotDescriptor {
            slot_index: 0,
            topic: "exec.unit.ready".to_string(),
            payload_json: payload.to_string(),
            wave_kind: WaveKind::Exec,
            payload_digest: SlotDescriptor::digest_of(payload),
            slot_index_in_parent: None,
        };
        store
            .persist_slot_descriptor(&wave, &descriptor)
            .expect("persist");

        let outcome = store
            .take_dispatchable_redrive_descriptor(&wave, 0, descriptor.payload_digest.as_str())
            .expect("take");
        match outcome {
            RedriveTakeOutcome::Dispatchable { descriptor: d } => {
                assert_eq!(d.slot_index, 0);
                assert_eq!(d.topic, "exec.unit.ready");
                assert_eq!(d.wave_kind, WaveKind::Exec);
                assert_eq!(d.payload_json, payload);
                assert_eq!(d.slot_index_in_parent, None);
            }
            other => panic!("expected Dispatchable, got {other:?}"),
        }
    }

    /// R16 / S13 (unavailable): there is no persisted descriptor
    /// for the slot (legacy pre-U4 row). The default impl returns
    /// `DescriptorUnavailable`; the in-memory override returns the
    /// same. The consumer MUST refuse to spawn a worker in this
    /// case.
    #[test]
    fn u4_take_without_persisted_descriptor_is_unavailable() {
        let store = InMemorySupervisorStore::new();
        let wave = register_exec(&store, "u4-unav");
        let outcome = store
            .take_dispatchable_redrive_descriptor(&wave, 1, "any-digest")
            .expect("take");
        assert_eq!(outcome, RedriveTakeOutcome::DescriptorUnavailable);
    }

    /// R16 / S13 (conflict): the persisted descriptor has a
    /// different `payload_digest` than the caller expects. The
    /// store MUST return `DescriptorConflict` so a mis-routed
    /// caller cannot silently dispatch a stale worker.
    #[test]
    fn u4_digest_mismatch_returns_conflict() {
        let store = InMemorySupervisorStore::new();
        let wave = register_exec(&store, "u4-conflict");
        let payload = r#"{"content_hash":"a"}"#;
        let descriptor = SlotDescriptor {
            slot_index: 0,
            topic: "exec.unit.ready".to_string(),
            payload_json: payload.to_string(),
            wave_kind: WaveKind::Exec,
            payload_digest: SlotDescriptor::digest_of(payload),
            slot_index_in_parent: None,
        };
        store
            .persist_slot_descriptor(&wave, &descriptor)
            .expect("persist");

        let outcome = store
            .take_dispatchable_redrive_descriptor(&wave, 0, "wrong-digest-from-runtime")
            .expect("take");
        assert_eq!(outcome, RedriveTakeOutcome::DescriptorConflict);
    }

    /// R16 / S13 (unknown wave): persisting a descriptor for a
    /// wave that was never registered returns `UnknownWave`.
    #[test]
    fn u4_persist_on_unknown_wave_errors() {
        let store = InMemorySupervisorStore::new();
        let descriptor = SlotDescriptor {
            slot_index: 0,
            topic: "exec.unit.ready".to_string(),
            payload_json: "{}".to_string(),
            wave_kind: WaveKind::Exec,
            payload_digest: SlotDescriptor::digest_of("{}"),
            slot_index_in_parent: None,
        };
        let err = store
            .persist_slot_descriptor("never-registered", &descriptor)
            .expect_err("unknown wave must fail closed");
        assert!(
            matches!(err, SupervisorStoreError::UnknownWave(_)),
            "got {err:?}"
        );
    }
}

/// 2026-07-28-002 plan G2 / G3 / T2 / T3: rusqlite-backed parity
/// coverage for the descriptor three-state contract and the
/// redrive copy/remap semantics. The in-memory variants above pin
/// the same behavior; these pin the production store (real v10
/// schema, real SQL paths).
#[cfg(all(test, feature = "supervisor-db"))]
mod rusqlite_backed_tests {
    use crate::supervisor::{
        RedriveTakeOutcome, RusqliteSupervisorStore, SlotDescriptor, SlotResource, SupervisorStore,
        WaveKind,
    };

    fn open_store() -> (tempfile::TempDir, RusqliteSupervisorStore) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("supervisor.db");
        let store = RusqliteSupervisorStore::open(&path).expect("open");
        (tmp, store)
    }

    fn register_exec(store: &RusqliteSupervisorStore, key: &str) -> String {
        let wave = store.register_wave(key, WaveKind::Exec, 2, 1).unwrap();
        for i in 0..2u32 {
            store
                .bind_worktree(
                    &wave,
                    i,
                    SlotResource {
                        slot_index: i,
                        worktree_path: Some(format!(".ralph/u4r/{key}-{i}")),
                        branch: Some(format!("ralph/u4r-{key}-{i}")),
                    },
                )
                .unwrap();
        }
        wave
    }

    /// G2 + S3 seal: persist + take round-trip on real sqlite. The
    /// `slot_descriptor` read-back after persist doubles as the
    /// regression guard for the UPDATE-only bug (first persist
    /// matched zero rows and silently dropped the descriptor).
    #[test]
    fn rusqlite_persist_then_take_returns_dispatchable() {
        let (_tmp, store) = open_store();
        let wave = register_exec(&store, "u4r-happy");

        let payload = r#"{"content_hash":"h"}"#;
        let descriptor = SlotDescriptor {
            slot_index: 0,
            topic: "exec.unit.ready".to_string(),
            payload_json: payload.to_string(),
            wave_kind: WaveKind::Exec,
            payload_digest: SlotDescriptor::digest_of(payload),
            slot_index_in_parent: None,
        };
        store
            .persist_slot_descriptor(&wave, &descriptor)
            .expect("persist");

        // First-persist seal: the row must actually exist now.
        let read = store.slot_descriptor(&wave, 0).expect("read");
        assert!(
            read.is_some(),
            "first persist must store the descriptor (UPDATE-only regression)"
        );

        match store
            .take_dispatchable_redrive_descriptor(&wave, 0, descriptor.payload_digest.as_str())
            .expect("take")
        {
            RedriveTakeOutcome::Dispatchable { descriptor: d } => {
                assert_eq!(d.topic, "exec.unit.ready");
                assert_eq!(d.wave_kind, WaveKind::Exec);
                assert_eq!(d.payload_json, payload);
                assert_eq!(d.slot_index_in_parent, None);
            }
            other => panic!("expected Dispatchable, got {other:?}"),
        }
    }

    /// G2 (unavailable state) on real sqlite.
    #[test]
    fn rusqlite_take_without_descriptor_is_unavailable() {
        let (_tmp, store) = open_store();
        let wave = register_exec(&store, "u4r-unav");
        let outcome = store
            .take_dispatchable_redrive_descriptor(&wave, 1, "any-digest")
            .expect("take");
        assert_eq!(outcome, RedriveTakeOutcome::DescriptorUnavailable);
    }

    /// G2 (conflict state) on real sqlite.
    #[test]
    fn rusqlite_digest_mismatch_returns_conflict() {
        let (_tmp, store) = open_store();
        let wave = register_exec(&store, "u4r-conflict");
        let payload = r#"{"content_hash":"a"}"#;
        let descriptor = SlotDescriptor {
            slot_index: 0,
            topic: "exec.unit.ready".to_string(),
            payload_json: payload.to_string(),
            wave_kind: WaveKind::Exec,
            payload_digest: SlotDescriptor::digest_of(payload),
            slot_index_in_parent: None,
        };
        store
            .persist_slot_descriptor(&wave, &descriptor)
            .expect("persist");
        let outcome = store
            .take_dispatchable_redrive_descriptor(&wave, 0, "wrong-digest-from-runtime")
            .expect("take");
        assert_eq!(outcome, RedriveTakeOutcome::DescriptorConflict);
    }

    /// C1 / R-F6 parity: `create_redrive_wave` copies the parent
    /// descriptor into the child key; `take` returns
    /// `descriptor.slot_index == parent_slot` (same as the
    /// in-memory store) with `slot_index_in_parent = Some(parent)`,
    /// and the enriched list carries `expected_digest` from the
    /// copied row.
    #[test]
    fn rusqlite_redrive_copy_preserves_parent_anchor() {
        let (_tmp, store) = open_store();
        let parent = register_exec(&store, "u4r-anchor");

        let payload = r#"{"unit":"u1"}"#;
        store
            .persist_slot_descriptor(
                &parent,
                &SlotDescriptor {
                    slot_index: 1,
                    topic: "exec.unit.ready".to_string(),
                    payload_json: payload.to_string(),
                    wave_kind: WaveKind::Exec,
                    payload_digest: SlotDescriptor::digest_of(payload),
                    slot_index_in_parent: None,
                },
            )
            .unwrap();
        store.record_slot_failure(&parent, 1, "synthetic").unwrap();

        let redrive = store.create_redrive_wave(&parent, Some(&[1])).unwrap();
        let child = redrive.child_wave_id;

        // Enriched list: child slot 0 ← parent slot 1, digest present.
        let pending = store.list_redrive_pending_child_waves().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].child_wave_id, child);
        assert_eq!(pending[0].slots.len(), 1);
        assert_eq!(pending[0].slots[0].child_slot_index, 0);
        assert_eq!(pending[0].slots[0].parent_slot_index, 1);
        assert_eq!(
            pending[0].slots[0].expected_digest.as_deref(),
            Some(SlotDescriptor::digest_of(payload).as_str())
        );

        // Take returns the PARENT slot index in descriptor.slot_index
        // (in-memory parity) + the parent anchor field.
        match store
            .take_dispatchable_redrive_descriptor(
                &child,
                0,
                SlotDescriptor::digest_of(payload).as_str(),
            )
            .unwrap()
        {
            RedriveTakeOutcome::Dispatchable { descriptor: d } => {
                assert_eq!(
                    d.slot_index, 1,
                    "descriptor.slot_index must be the parent slot"
                );
                assert_eq!(d.slot_index_in_parent, Some(1));
                assert_eq!(d.payload_json, payload);
            }
            other => panic!("expected Dispatchable, got {other:?}"),
        }
    }

    /// A1 / R-F4: a dispatcher-side re-persist with
    /// `slot_index_in_parent = None` must NOT clobber the anchor
    /// seeded by `create_redrive_wave` (COALESCE semantics).
    #[test]
    fn rusqlite_second_persist_preserves_parent_anchor() {
        let (_tmp, store) = open_store();
        let parent = register_exec(&store, "u4r-coalesce");
        let payload = r#"{"unit":"u0"}"#;
        store
            .persist_slot_descriptor(
                &parent,
                &SlotDescriptor {
                    slot_index: 0,
                    topic: "exec.unit.ready".to_string(),
                    payload_json: payload.to_string(),
                    wave_kind: WaveKind::Exec,
                    payload_digest: SlotDescriptor::digest_of(payload),
                    slot_index_in_parent: None,
                },
            )
            .unwrap();
        store.record_slot_failure(&parent, 0, "synthetic").unwrap();
        let redrive = store.create_redrive_wave(&parent, Some(&[0])).unwrap();
        let child = redrive.child_wave_id;

        // Dispatcher-side second persist passes None anchor.
        store
            .persist_slot_descriptor(
                &child,
                &SlotDescriptor {
                    slot_index: 0,
                    topic: "exec.unit.ready".to_string(),
                    payload_json: payload.to_string(),
                    wave_kind: WaveKind::Exec,
                    payload_digest: SlotDescriptor::digest_of(payload),
                    slot_index_in_parent: None,
                },
            )
            .unwrap();
        let read = store
            .slot_descriptor(&child, 0)
            .unwrap()
            .expect("row exists");
        assert_eq!(
            read.slot_index_in_parent,
            Some(0),
            "COALESCE must preserve the seeded parent anchor"
        );
    }

    /// C7 / R-F6 parity: a legacy child slot WITHOUT a descriptor
    /// row stays in the enriched list with `expected_digest = None`
    /// (boot fails closed on it), mirroring the in-memory store
    /// instead of being silently dropped.
    #[test]
    fn rusqlite_legacy_child_slot_surfaces_none_digest() {
        let (_tmp, store) = open_store();
        let parent = register_exec(&store, "u4r-legacy");
        // No descriptors persisted — simulate pre-U4 rows.
        store.record_slot_failure(&parent, 0, "synthetic").unwrap();
        store.record_slot_failure(&parent, 1, "synthetic").unwrap();

        let redrive = store.create_redrive_wave(&parent, None).unwrap();
        let pending = store.list_redrive_pending_child_waves().unwrap();
        assert_eq!(
            pending.len(),
            1,
            "child wave must be listed even without descriptors"
        );
        assert_eq!(pending[0].child_wave_id, redrive.child_wave_id);
        assert_eq!(
            pending[0].slots.len(),
            2,
            "legacy slots must NOT be dropped"
        );
        assert!(
            pending[0].slots.iter().all(|s| s.expected_digest.is_none()),
            "legacy slots surface expected_digest = None for boot fail-close"
        );
    }

    /// Review P0: take must NOT delete the descriptor. Simulates the
    /// crash window (take → process exit → reopen DB → resume): the
    /// descriptor must still be Dispatchable so boot can re-dispatch.
    /// After the slot leaves Pending, list must exclude it (idempotency
    /// via slot status, not destructive take).
    #[test]
    fn rusqlite_take_survives_reopen_crash_window() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let path = tmp.path().join("supervisor.db");
        let digest;
        let child;
        {
            let store = RusqliteSupervisorStore::open(&path).expect("open");
            let parent = register_exec(&store, "u4r-crash");
            let payload = r#"{"unit":"crash-window"}"#;
            digest = SlotDescriptor::digest_of(payload);
            store
                .persist_slot_descriptor(
                    &parent,
                    &SlotDescriptor {
                        slot_index: 0,
                        topic: "exec.unit.ready".to_string(),
                        payload_json: payload.to_string(),
                        wave_kind: WaveKind::Exec,
                        payload_digest: digest.clone(),
                        slot_index_in_parent: None,
                    },
                )
                .unwrap();
            store.record_slot_failure(&parent, 0, "synthetic").unwrap();
            // Fail only slot 0 so the child has one Pending slot.
            let redrive = store.create_redrive_wave(&parent, Some(&[0])).unwrap();
            child = redrive.child_wave_id;

            match store
                .take_dispatchable_redrive_descriptor(&child, 0, digest.as_str())
                .unwrap()
            {
                RedriveTakeOutcome::Dispatchable { .. } => {}
                other => panic!("expected Dispatchable before crash, got {other:?}"),
            }
            // Process "crashes" here — drop store without spawning.
        }

        // Reopen: descriptor must still be present (no DELETE on take).
        let store = RusqliteSupervisorStore::open(&path).expect("reopen");
        match store
            .take_dispatchable_redrive_descriptor(&child, 0, digest.as_str())
            .unwrap()
        {
            RedriveTakeOutcome::Dispatchable { descriptor: d } => {
                assert_eq!(d.payload_digest, digest);
            }
            other => {
                panic!("resume after take-without-spawn must still see Dispatchable, got {other:?}")
            }
        }
        let pending = store.list_redrive_pending_child_waves().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].expected_total, 1);
        assert_eq!(pending[0].slots.len(), 1);

        // Mark slot terminal via failure — list must drop it (Pending filter).
        store
            .record_slot_failure(&child, 0, "post-reopen-terminal")
            .unwrap();
        let pending_after = store.list_redrive_pending_child_waves().unwrap();
        assert!(
            pending_after
                .iter()
                .all(|c| c.child_wave_id != child || c.slots.is_empty())
                || pending_after.is_empty()
                || pending_after.iter().all(|c| c.child_wave_id != child),
            "non-Pending slots must leave the boot pending list; got {pending_after:?}"
        );
    }
}
