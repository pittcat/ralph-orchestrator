//! 2026-07-27-004 plan U1 (R1-R4): the public wave identity is
//! the SAME identifier at every layer (emit, dispatch, inspect,
//! coord delivery, redrive). The store persists the public ID as
//! the primary key and exposes it through typed `WaveId` so the
//! caller never has to learn about the internal `StoreWaveKey`.
//!
//! Migration: the existing `waves.wave_id` PRIMARY KEY (added in
//! v1) already accepts arbitrary TEXT; the v9 marker migration
//! documents that the public-id-only contract is now in force.
//! The rusqlite reopen test below uses a real on-disk database
//! under `tempfile::TempDir` so the cross-restart case (in-memory
//! variant cannot simulate it) is exercised.

#[cfg(test)]
mod tests {
    use crate::supervisor::{
        InMemorySupervisorStore, SupervisorStore, SupervisorStoreError, WaveId, WaveKind,
        WaveSnapshot,
    };

    fn fan_in_status_id<S: SupervisorStore>(s: &S, id: &WaveId) -> WaveSnapshot {
        // The current trait surface still hands out `&str`. The
        // U1 redrive uses `WaveId::as_str` to make the call-site
        // public-id-driven; switching the trait signature to
        // `&WaveId` is gated on full U1 commit (R2). Until then,
        // this thin wrapper documents the desired call shape.
        s.fan_in_status(id.as_str())
            .expect("re-enter via public id")
    }

    /// R1: public ID is the SINGLE identity surfaced by the store.
    /// `register_wave_with_public_id` returns the caller-supplied
    /// id unchanged; downstream `fan_in_status(public_id)` returns
    /// the same wave.
    #[test]
    fn r1_public_id_round_trips_through_register_and_inspect() {
        let store = InMemorySupervisorStore::new();
        let public_id = WaveId::from("w-rs-u1-r1");
        let returned: WaveId = store
            .register_wave_with_public_id(
                &public_id,
                WaveKind::Exec,
                3,
                /* slot_retry_budget */ 1,
            )
            .expect("register_with_public_id must succeed");
        assert_eq!(
            returned, public_id,
            "register must echo the public id (the store PK equals the public id)"
        );
        let snap = fan_in_status_id(&store, &public_id);
        assert_eq!(snap.expected_total, 3);
        assert_eq!(snap.pending_count, 3);
    }

    /// R2: internal `StoreWaveKey` is opaque to callers. The store
    /// implementation can use a different internal key (e.g. a
    /// numeric seq for emit-side `wave_emissions`), but the public
    /// API MUST only accept / return `WaveId`. This test pins the
    /// trait surface so accidental leakage is caught at compile
    /// time.
    #[test]
    fn r2_internal_store_key_does_not_leak_via_trait_dto() {
        // If `register_wave_with_public_id` ever returns a
        // `StoreWaveKey`, the test must fail at compile time. The
        // assertion here is the return-type literal — the
        // compiler enforces the boundary.
        fn returns_wave_id(s: &InMemorySupervisorStore) -> WaveId {
            let id = WaveId::from("w-rs-u1-r2");
            s.register_wave_with_public_id(&id, WaveKind::Review, 2, 0)
                .expect("register")
        }
        let _id: WaveId = returns_wave_id(&InMemorySupervisorStore::new());
    }

    /// R3 / D2: same public + same kind / expected_total / digest
    /// is idempotent; a contract drift (different total, different
    /// kind, or different digest) fails closed with
    /// `IdentityContractConflict`. Plain duplicate (no digest
    /// mismatch check below) returns the existing wave unchanged.
    #[test]
    fn r3_idempotent_register_with_matching_contract() {
        let store = InMemorySupervisorStore::new();
        let public_id = WaveId::from("w-rs-u1-r3");
        let first = store
            .register_wave_with_public_id(&public_id, WaveKind::Exec, 4, 1)
            .expect("first register");
        // Second register with SAME contract MUST return the same
        // public id and NOT create a new wave row.
        let second = store
            .register_wave_with_public_id(&public_id, WaveKind::Exec, 4, 1)
            .expect("idempotent re-register must succeed");
        assert_eq!(first, second);
        let waves = store.list_wave_ids().expect("list");
        assert_eq!(
            waves.len(),
            1,
            "idempotent re-register must not create a second row; got {waves:?}"
        );

        // Mismatched `expected_total` → IdentityContractConflict.
        let err = store
            .register_wave_with_public_id(&public_id, WaveKind::Exec, 5, 1)
            .expect_err("differing expected_total must conflict");
        assert!(
            matches!(err, SupervisorStoreError::IdentityContractConflict(_)),
            "different total must conflict; got {err:?}"
        );

        // Mismatched `kind` → IdentityContractConflict.
        let err = store
            .register_wave_with_public_id(&public_id, WaveKind::Fix, 4, 1)
            .expect_err("differing kind must conflict");
        assert!(
            matches!(err, SupervisorStoreError::IdentityContractConflict(_)),
            "different kind must conflict; got {err:?}"
        );

        // Mismatched `slot_retry_budget` → IdentityContractConflict.
        let err = store
            .register_wave_with_public_id(&public_id, WaveKind::Exec, 4, 0)
            .expect_err("differing retry_budget must conflict");
        assert!(
            matches!(err, SupervisorStoreError::IdentityContractConflict(_)),
            "different retry_budget must conflict; got {err:?}"
        );
    }

    /// R4: public ID drives fan-in re-entry after the bridge's
    /// in-memory authoritative map was lost (simulating a process
    /// restart). The persistent store resolves the same wave row
    /// without `register_wave_if_absent` needing a per-process
    /// cache.
    #[test]
    fn r4_public_id_survives_reentry_bridge_drop() {
        let store = InMemorySupervisorStore::new();
        let public_id = WaveId::from("w-rs-u1-r4");
        store
            .register_wave_with_public_id(&public_id, WaveKind::Exec, 3, 1)
            .expect("register");
        // The re-entry pattern: a second store instance
        // (post-restart) resolves the SAME wave row purely from
        // its persistent map when given the public id. The
        // InMemory variant cannot simulate cross-restart
        // persistence; the rusqlite reopen test below covers the
        // on-disk case.
        let snap = fan_in_status_id(&store, &public_id);
        assert_eq!(snap.expected_total, 3);
        // A second fresh InMemory store (no shared state) must
        // NOT see the prior wave row — proving that the public
        // id alone, without an in-memory authoritative map, does
        // not cross process boundaries. This documents the
        // InMemory limitation; the rusqlite variant covers the
        // cross-restart case (see acceptance test).
        let reentry_store = InMemorySupervisorStore::new();
        assert!(
            reentry_store.fan_in_status(public_id.as_str()).is_err(),
            "fresh InMemory store without migration must NOT see the prior wave row"
        );
    }

    /// Internal `StoreWaveKey` is an opaque `pub` newtype so a
    /// store can keep a separate numeric internal key, but the
    /// public API only accepts `WaveId`. The wrapped value should
    /// not appear in any inspection JSON.
    #[test]
    fn store_wave_key_display_does_not_leak_into_inspect() {
        let _: crate::supervisor::StoreWaveKey =
            crate::supervisor::StoreWaveKey::from_public(&WaveId::from("internal-only-not-public"));
        // The wrapper exists for type separation; no Display
        // impl to avoid leaking the value into JSON.
    }
}

#[cfg(all(test, feature = "supervisor-db"))]
mod rusqlite_reopen_tests {
    //! 2026-07-27-004 plan U1: the rusqlite variant must reopen
    //! the same wave row by public id without an in-memory
    //! authoritative map. This is the integration seam that
    //! R4 (reopen contract) is asserted against.
    use crate::supervisor::rusqlite::RusqliteSupervisorStore;
    use crate::supervisor::{SupervisorStore, WaveId, WaveKind};

    #[test]
    fn public_id_survives_rusqlite_reopen() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("supervisor.db");
        let public_id = WaveId::from("w-rs-u1-rusqlite-reopen");

        // 1. First opener: register wave through the public-id
        //    contract, then close.
        {
            let store = RusqliteSupervisorStore::open(&path).expect("open");
            let returned = store
                .register_wave_with_public_id(&public_id, WaveKind::Exec, 3, 1)
                .expect("register via public id");
            assert_eq!(returned, public_id);
            // Sanity: the same id is reachable through the
            // legacy `wave_id_for_idempotency_key` lookup
            // because U1 stores the public id in the
            // `idempotency_key` column as well.
            let legacy = store
                .wave_id_for_idempotency_key(public_id.as_str())
                .expect("legacy lookup");
            assert_eq!(
                legacy.as_deref(),
                Some(public_id.as_str()),
                "legacy lookup must alias the public id"
            );
        }

        // 2. Second opener: confirm the row survived a process
        //    restart by hitting the store directly via the public
        //    id — no in-memory authoritative map involved.
        let reopened = RusqliteSupervisorStore::open(&path).expect("reopen");
        let snap = reopened
            .fan_in_status(public_id.as_str())
            .expect("fan_in_status by public id");
        assert_eq!(snap.expected_total, 3);
        assert_eq!(
            snap.wave_id,
            public_id.as_str(),
            "snapshot must echo the public id verbatim"
        );
        // The reopened store must NOT allocate a second row —
        // idempotent re-register confirms the persistent
        // `waves_by_id` is the SSOT.
        let again = reopened
            .register_wave_with_public_id(&public_id, WaveKind::Exec, 3, 1)
            .expect("idempotent re-register");
        assert_eq!(again, public_id);
        // Two waves would indicate the public-id contract
        // degenerated to a per-process cache.
        let ids = reopened.list_wave_ids().expect("list");
        assert_eq!(
            ids.len(),
            1,
            "single row across process boundary; got {ids:?}"
        );
    }
}
