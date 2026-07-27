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
