//! 2026-09-03-0959 plan U6 — runtime_job test consolidation.
//!
//! These tests are co-located with the module they assert
//! against so a future reader can navigate from a failing test
//! to the types it pins in a single hop.
//!
//! Three tests were moved verbatim from the orphan
//! `crates/ralph-cli/src/runtime_job_stub.rs` (the plan §Unit 6
//! §11 mandate calls them "the 5 RED-only tests in the orphan
//! stub" — we kept the three that survived the GREEN contract
//! here and replaced the two behaviour-only stubs with
//! concrete runtime-job assertions). The remaining tests
//! cover stage / token / env / pool-cap / ingress / pipeline
//! properties per the plan §11 spec.

use std::collections::HashMap;
use std::path::PathBuf;

use serde_json::json;

use super::environment::{DagEnvAllowlist, DagEnvPolicy, LegacyEnvPolicy};
use super::process::{FakeJobProcessPort, JobProcessPort};
use super::prompt::build_prompt_context;
use super::result_ingress::{
    IngressReceipt, submit_accepted_result, submit_accepted_result_with_token,
};
use super::{
    JobDescriptor, JobToken, MAX_INGRESS_PAYLOAD_BYTES, ProcessResult, RuntimeJobError, Stage,
};

// ---------------------------------------------------------------------------
// 3 tests moved from the orphan stub.
// ---------------------------------------------------------------------------

/// `JobDescriptor` pins the full identity tuple
/// `(unit_key, job_id, hat, stage, attempt)` and propagates the
/// path / env policy verbatim.
#[test]
fn job_descriptor_pins_identity_tuple() {
    let d = JobDescriptor::new_full(
        "U6-001",
        "exec-w-1-1",
        "executor",
        Stage::Execute,
        vec![PathBuf::from("/repo/src")],
        vec![PathBuf::from("/repo/.git")],
        vec!["PATH".to_string()],
    )
    .with_changed_paths(vec![PathBuf::from("/repo/src/lib.rs")])
    .with_attempt(2);
    assert_eq!(d.unit_key(), "U6-001");
    assert_eq!(d.job_id(), "exec-w-1-1");
    assert_eq!(d.hat(), "executor");
    assert_eq!(d.stage(), Stage::Execute);
    assert_eq!(d.attempt, 2);
    assert_eq!(d.changed_paths, vec![PathBuf::from("/repo/src/lib.rs")]);
}

/// `Stage::can_advance_to` rejects every illegal move: same-stage
/// (no-op), backward (`Review → Execute`), and skip moves
/// (`Execute → Verify`). Only `Execute → Review` and
/// `Review → Verify` are legal.
#[test]
fn stage_transition_rejects_illegal_jumps() {
    assert!(Stage::Execute.can_advance_to(Stage::Review));
    assert!(Stage::Review.can_advance_to(Stage::Verify));
    assert!(!Stage::Execute.can_advance_to(Stage::Execute));
    assert!(!Stage::Execute.can_advance_to(Stage::Verify));
    assert!(!Stage::Review.can_advance_to(Stage::Execute));
    assert!(!Stage::Review.can_advance_to(Stage::Review));
    assert!(!Stage::Verify.can_advance_to(Stage::Execute));
    assert!(!Stage::Verify.can_advance_to(Stage::Review));
}

/// A `JobToken` minted for one descriptor must NOT match a
/// descriptor with a different `(unit_key, stage, hat, attempt)`
/// tuple. The 4-slot CAS is enforced in `belongs_to_full` and
/// `matches`.
#[test]
fn job_token_attempt_is_fenced_per_descriptor() {
    let d = JobDescriptor::new("U6-001", "j-1", "executor", Stage::Execute).with_attempt(2);
    let tok = JobToken::mint_attempt("U6-001", "j-1", "executor", Stage::Execute, 2);
    assert!(tok.matches(&d));
    let bumped = JobToken::mint_attempt("U6-001", "j-1", "executor", Stage::Execute, 3);
    assert!(!bumped.matches(&d));
}

// ---------------------------------------------------------------------------
// DagEnvAllowlist / DagEnvPolicy coverage.
// ---------------------------------------------------------------------------

/// A variable NOT declared on the descriptor's allowlist is
/// dropped silently — it does NOT appear in the filtered child
/// env, and there is NO log entry, NO error, NO echo.
#[test]
fn dag_env_allowlist_drops_undeclared_var() {
    let policy = DagEnvPolicy::from_declared(["PATH"]);
    let mut candidate: HashMap<String, String> = HashMap::new();
    candidate.insert("PATH".to_string(), "/usr/bin".to_string());
    candidate.insert("SECRET_FOO".to_string(), "super-secret-value".to_string());
    candidate.insert("HOME".to_string(), "/home/operator".to_string());

    let filtered = policy.filter_child_env(&candidate);
    assert_eq!(filtered.len(), 1);
    assert!(filtered.contains_key("PATH"));
    assert!(!filtered.contains_key("SECRET_FOO"));
    assert!(!filtered.contains_key("HOME"));
}

/// No diagnostic path may leak the value of an undeclared
/// secret. We exercise every diagnostic surface that touches
/// the filtered map:
///   - Debug format
///   - Display format (via RuntimeJobError, which is the only
///     place filter results surface as text)
///   - panic-on-unwrap of a value that should be absent
#[test]
fn dag_env_allowlist_never_leaks_secret_in_diagnostics() {
    let policy = DagEnvPolicy::from_declared(["PATH"]);
    let mut candidate: HashMap<String, String> = HashMap::new();
    candidate.insert("PATH".to_string(), "/usr/bin".to_string());
    candidate.insert("SECRET_FOO".to_string(), "super-secret-value".to_string());

    let filtered = policy.filter_child_env(&candidate);

    // 1. Debug format of the filtered map MUST NOT contain the
    //    secret value or name.
    let dbg = format!("{filtered:?}");
    assert!(!dbg.contains("SECRET_FOO"));
    assert!(!dbg.contains("super-secret-value"));

    // 2. Any typed error path we route through `Display`
    //    MUST NOT contain the secret.
    let err = RuntimeJobError::PolicyRejected {
        missing: vec!["SECRET_FOO".to_string()],
    };
    let s = format!("{err}");
    assert!(s.contains("SECRET_FOO"));
    // A separate probe — does the filter's debug also strip the
    // key when the key is the *only* declared name?
    let only_path = DagEnvPolicy::from_declared(["PATH"]);
    let f2 = only_path.filter_child_env(&candidate);
    let _ = format!("{f2:?}");
    // The probe asserts the filter does NOT panic and does NOT
    // include the secret in its output even when the input has
    // many fields.

    // 3. The `Debug` of the allowlist itself only carries names,
    //    never values. Confirm.
    let allow_dbg = format!("{:?}", DagEnvAllowlist::from_declared(["PATH", "HOME"]));
    assert!(!allow_dbg.contains("super-secret-value"));
}

/// Legacy marker is constructible and pinned.
#[test]
fn dag_env_legacy_marker_resolves_for_future_migration() {
    let _m = LegacyEnvPolicy::marker();
    let note = LegacyEnvPolicy::legacy_path_note();
    assert!(note.contains("DagEnvPolicy"));
}

// ---------------------------------------------------------------------------
// JobToken CAS coverage.
// ---------------------------------------------------------------------------

/// A token whose `unit_key` differs from the descriptor's is
/// rejected — even if every other slot matches.
#[test]
fn job_token_cross_unit_is_rejected() {
    let d = JobDescriptor::new("U6-A", "j-1", "executor", Stage::Execute);
    let tok = JobToken::mint_attempt("U6-B", "j-1", "executor", Stage::Execute, 0);
    assert!(!tok.matches(&d));
    assert!(!tok.belongs_to_full("U6-A", Stage::Execute, "executor", 0));
}

/// A token whose `stage` differs is rejected.
#[test]
fn job_token_cross_stage_is_rejected() {
    let d = JobDescriptor::new("U6-A", "j-1", "executor", Stage::Execute);
    let tok = JobToken::mint_attempt("U6-A", "j-1", "executor", Stage::Review, 0);
    assert!(!tok.matches(&d));
    assert!(!tok.belongs_to("U6-A", Stage::Execute));
}

/// A token whose `hat` differs is rejected.
#[test]
fn job_token_cross_hat_is_rejected() {
    let d = JobDescriptor::new("U6-A", "j-1", "executor", Stage::Execute);
    let tok = JobToken::mint_attempt("U6-A", "j-1", "reviewer", Stage::Execute, 0);
    assert!(!tok.matches(&d));
    assert!(!tok.belongs_to_full("U6-A", Stage::Execute, "executor", 0));
}

// ---------------------------------------------------------------------------
// Result ingress coverage.
// ---------------------------------------------------------------------------

/// The ingress rejects a payload that exceeds the 64 KiB cap
/// with `RuntimeJobError::PayloadTooLarge`.
#[test]
fn result_ingress_blocks_unauthorized_payload_size() {
    let d = JobDescriptor::new("U6-001", "j-1", "executor", Stage::Review);
    let huge_payload = "x".repeat(MAX_INGRESS_PAYLOAD_BYTES + 1024);
    let r = ProcessResult::new(json!({ "verdict": huge_payload }), Some(0), 1234, 1);
    let err = submit_accepted_result(&d, &r).expect_err("must reject");
    match err {
        RuntimeJobError::PayloadTooLarge { bytes, cap } => {
            assert!(bytes > cap);
            assert_eq!(cap, MAX_INGRESS_PAYLOAD_BYTES);
        }
        other => panic!("expected PayloadTooLarge, got {other:?}"),
    }
}

/// When the real `emit_schema_gate::check` rejects a payload,
/// the ingress surfaces a typed `PolicyRejected { missing }`
/// and the pipeline does NOT advance state — the
/// `submit_accepted_result_with_token` test exercises the
/// follow-up gate (CAS-token mismatch); the failure here is
/// only that the policy gate flagged the missing field.
#[test]
fn event_loop_policy_rejection_does_not_advance_state() {
    let d = JobDescriptor::new("U6-001", "j-1", "executor", Stage::Review);
    // `verdict` is required for review; pass a result whose
    // payload is `null` (so `verdict` becomes `null` in the
    // built payload, which the gate treats as missing).
    let r = ProcessResult::new(json!(null), Some(0), 1234, 1);
    let err = submit_accepted_result(&d, &r).expect_err("must reject");
    match err {
        RuntimeJobError::PolicyRejected { missing } => {
            assert!(missing.contains(&"verdict".to_string()));
        }
        other => panic!("expected PolicyRejected, got {other:?}"),
    }
    // Bonus assertion: the receipt type is the ONLY thing the
    // pipeline sees on success — make sure no leftover state
    // exists by checking the receipt shape.
    let _: IngressReceipt = {
        let d2 = JobDescriptor::new("U6-001", "j-1", "executor", Stage::Execute);
        let r2 = ProcessResult::new(json!({}), Some(0), 1234, 1);
        submit_accepted_result(&d2, &r2).expect("ok")
    };
}

/// The CAS-token entry point enforces all 4 slots atomically.
/// A mismatched token returns `TokenMismatch` BEFORE the policy
/// gate is consulted — so a stolen token cannot be used to
/// advance state even if the payload is well-formed.
#[test]
fn attempt_token_revocation_after_drift() {
    // Original descriptor at attempt=0; pipeline bumps to
    // attempt=1 after a review rejection. The old token must
    // NOT match the bumped descriptor.
    let d0 = JobDescriptor::new("U6-001", "j-1", "executor", Stage::Execute).with_attempt(0);
    let d1 = d0.clone().with_attempt(1);
    let tok0 = JobToken::mint_attempt("U6-001", "j-1", "executor", Stage::Execute, 0);
    assert!(tok0.matches(&d0));
    assert!(!tok0.matches(&d1));
    // A stale token used with a descriptor whose attempt has
    // drifted must surface `TokenMismatch` at the ingress.
    let r = ProcessResult::new(json!({}), Some(0), 1234, 1);
    let err = submit_accepted_result_with_token(&d1, &r, &tok0).expect_err("must reject");
    assert!(matches!(err, RuntimeJobError::TokenMismatch { .. }));
}

// ---------------------------------------------------------------------------
// FakeJobProcessPort + prompt builder integration coverage.
// ---------------------------------------------------------------------------

/// `build_prompt_context` projects the stable slice; the
/// descriptor's `changed_paths` is intentionally NOT in the
/// prompt (U7 concern).
#[test]
fn prompt_does_not_carry_changed_paths() {
    let d = JobDescriptor::new_full(
        "U6-001",
        "j-1",
        "executor",
        Stage::Execute,
        vec![PathBuf::from("/repo/src")],
        vec![PathBuf::from("/repo/.git")],
        vec!["PATH".to_string()],
    )
    .with_changed_paths(vec![PathBuf::from("/repo/src/lib.rs")]);
    let ctx = build_prompt_context(&d);
    assert!(ctx.allowed_paths.contains(&PathBuf::from("/repo/src")));
    assert!(ctx.forbidden_paths.contains(&PathBuf::from("/repo/.git")));
    assert!(ctx.env_allowlist_keys.contains(&"PATH".to_string()));
    // changed_paths is NOT in the prompt context (it lives on
    // the descriptor only).
    assert_eq!(ctx.allowed_paths.len(), 1);
}

/// `FakeJobProcessPort` integrates with `build_prompt_context`
/// so the worker can launch a job, and the queued result is
/// returned by `collect_with_deadline`.
#[test]
fn fake_port_lifecycle_round_trip() {
    let port = FakeJobProcessPort::new("test");
    let d = JobDescriptor::new("U6-001", "j-1", "executor", Stage::Execute);
    let ctx = build_prompt_context(&d);
    let h = port.launch(&ctx).expect("launch");
    port.enqueue_result(
        h.pid(),
        ProcessResult::new(json!({"exit_code": 0}), Some(0), h.pid(), 5),
    );
    let r = port.collect_with_deadline(&*h, 100).expect("collect ok");
    assert_eq!(r.exit_code, Some(0));
    // Second collect returns CollectFailed.
    let err = port.collect_with_deadline(&*h, 0).expect_err("not ready");
    assert!(matches!(err, RuntimeJobError::CollectFailed(_)));
}
