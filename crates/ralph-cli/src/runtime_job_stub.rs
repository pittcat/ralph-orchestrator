//! U6 stub: generic job kernel — minimal RED-only attempt.
//!
//! This file is intentionally NOT in the canonical U6 module path
//! (`loop_runner/runtime_job/{mod,worker,prompt,process,environment,result_ingress}.rs`).
//! Per the subagent mandate it lives at `src/runtime_job_stub.rs` as an
//! orphan stub so that wiring the kernel into the module tree is a separate
//! concern handled by the future U6 activation.
//!
//! The tests below reference types that the U6 spec (§Unit 6) requires the
//! generic job kernel to expose but which do not yet exist anywhere in the
//! tree. They will fail to resolve as long as the kernel has not been
//! implemented and wired into `loop_runner::runtime_job`.

#![allow(dead_code, unused_imports)]

use ralph_cli::loop_runner::runtime_job::{JobDescriptor, JobToken, Stage};

/// RED: every per-Unit kernel invocation must carry an opaque, owned
/// descriptor that pins the (unit_key, job_id, hat, stage) tuple so that
/// downstream CAS / lease logic can detect cross-Unit or cross-Stage reuse.
#[test]
fn job_descriptor_pins_identity_tuple() {
    let descriptor = JobDescriptor::new(
        "U6-001",
        "exec-w-1-1",
        "executor",
        Stage::Execute,
    );
    assert_eq!(descriptor.unit_key(), "U6-001");
    assert_eq!(descriptor.job_id(), "exec-w-1-1");
    assert_eq!(descriptor.hat(), "executor");
    assert_eq!(descriptor.stage(), Stage::Execute);
}

/// RED: stages must transition strictly executor -> review -> verify. The
/// U6 spec (§9) requires that a fast Unit reach review while a slow sibling
/// is still executing, so the type must encode ordering and reject illegal
/// jumps (e.g. Execute -> Verify, or Review -> Execute).
#[test]
fn stage_transition_rejects_illegal_jumps() {
    assert!(Stage::Execute.can_advance_to(Stage::Review));
    assert!(Stage::Review.can_advance_to(Stage::Verify));
    assert!(!Stage::Execute.can_advance_to(Stage::Verify));
    assert!(!Stage::Review.can_advance_to(Stage::Execute));
}

/// RED: every descriptor must hand out a JobToken whose attempt counter is
/// the single source of truth for the fenced CAS guard (U9 §13). Cross-Unit
/// and cross-Stage tokens must reject `mint_attempt` at the type level.
#[test]
fn job_token_attempt_is_fenced_per_descriptor() {
    let token_a = JobToken::mint("U6-001", Stage::Execute);
    let token_b = JobToken::mint("U6-001", Stage::Review);
    assert_ne!(token_a.attempt(), token_b.attempt());
    assert!(token_a.belongs_to("U6-001", Stage::Execute));
    assert!(!token_a.belongs_to("U6-001", Stage::Review));
    assert!(!token_a.belongs_to("U6-002", Stage::Execute));
}
