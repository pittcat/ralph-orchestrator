#![cfg(test)]
use super::*;

// ─────────────────────────────────────────────────────────────────────────
// U7 (2026-07-04-003 plan): two-step gate wiring tests.
//
// These tests exercise `verify_gate_claim` (the wrapper around
// `try_claim_matching_ticket`) directly with an explicit
// `OperationContext`, so the test never has to mutate process env
// vars. The `execute_add` / `execute_ensure` integration is
// verified separately by reading the code path: each of those
// functions calls `verify_gate_claim` after `enforce_command_policy`
// and before any store mutation, so if the gate denies here, the
// execute path denies too.
// ─────────────────────────────────────────────────────────────────────────

use crate::task_verify_gate::{record_ticket, ticket_path};
use ralph_core::config::RalphConfig;

fn make_ctx(hat: &str, loop_id: &str, is_agent: bool) -> OperationContext {
    OperationContext {
        workspace_root: PathBuf::from("/tmp/wiring"),
        current_loop_id: Some(loop_id.to_string()),
        current_hat_id: Some(hat.to_string()),
        is_agent_context: is_agent,
    }
}

fn config_with_gate(gate_on: bool, unsafe_hatch: bool) -> RalphConfig {
    let yaml = format!(
        "tasks:\n  enabled: true\n  require_verify_for_cli_mutate: {gate_on}\n  \
             allow_unsafe_task_mutate: {unsafe_hatch}\n  coordinator_hats:\n    - coordinator\n"
    );
    serde_yaml::from_str(&yaml).expect("parse yaml")
}

fn add_payload() -> String {
    canonical_add_payload(&AddArgs {
        title: "x".to_string(),
        priority: 3,
        description: None,
        blocked_by: None,
        format: OutputFormat::Quiet,
    })
}

#[test]
fn test_agent_add_without_verify_denied() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    let cfg = config_with_gate(true, false);
    let ctx = make_ctx("coordinator", "loop-a", true);
    let err = verify_gate_claim(root, &cfg, &ctx, "add", &add_payload())
        .expect_err("agent add without verify must deny");
    let msg = err.to_string();
    assert!(
        msg.contains("task_verify_gate denied"),
        "stable prefix: {msg}"
    );
    assert!(msg.contains("verify"), "must explain verify: {msg}");
    let scoped = crate::task_verify_gate::scoped_ticket_path(
        root,
        "add",
        &add_payload(),
        "loop-a",
        "coordinator",
    );
    assert!(
        !scoped.exists(),
        "deny must not create a scoped ticket: {}",
        scoped.display()
    );
}

#[test]
fn test_agent_verify_then_add_ok() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    let cfg = config_with_gate(true, false);
    let ctx = make_ctx("coordinator", "loop-a", true);
    // Step 1: record a ticket with the same fingerprint.
    let (loop_id, hat_id) = gate_identifiers(&ctx);
    let fp = crate::task_verify_gate::mutation_fingerprint("add", &add_payload(), loop_id, hat_id);
    let scoped =
        crate::task_verify_gate::scoped_ticket_path(root, "add", &add_payload(), loop_id, hat_id);
    record_ticket(&scoped, &fp, loop_id, hat_id).expect("record");

    // Step 2: gate claims the ticket (prepared → marker) and passes.
    verify_gate_claim(root, &cfg, &ctx, "add", &add_payload()).expect("matching ticket must claim");
    assert!(
        !scoped.exists(),
        "successful gate claim must move the scoped ticket to the marker"
    );

    // Step 3: a successful Apply settles the claim (consume).
    settle_gate_claim(root, &ctx, "add", &add_payload(), Ok(()))
        .expect("settle must consume after successful apply");
    assert!(
        !crate::task_verify_gate::claim_marker_path(&scoped).exists(),
        "settle must remove the claim marker"
    );
}

#[test]
fn test_agent_second_add_needs_reverify() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    let cfg = config_with_gate(true, false);
    let ctx = make_ctx("coordinator", "loop-a", true);
    // First pass: record + verify.
    let (loop_id, hat_id) = gate_identifiers(&ctx);
    let fp = crate::task_verify_gate::mutation_fingerprint("add", &add_payload(), loop_id, hat_id);
    let scoped =
        crate::task_verify_gate::scoped_ticket_path(root, "add", &add_payload(), loop_id, hat_id);
    record_ticket(&scoped, &fp, loop_id, hat_id).expect("record");
    verify_gate_claim(root, &cfg, &ctx, "add", &add_payload()).expect("first pass claims");

    // Second pass: prepared ticket was claimed (and never settled
    // back) → must deny.
    let err = verify_gate_claim(root, &cfg, &ctx, "add", &add_payload())
        .expect_err("second pass without re-verify must deny");
    assert!(err.to_string().contains("task_verify_gate denied"));
}

/// U1 (STAB-OPAC-GATES-001): a failed Apply must restore the
/// claimed ticket so the corrected retry re-claims without a
/// fresh verify.
#[test]
fn test_settle_gate_claim_restores_on_failure() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    let cfg = config_with_gate(true, false);
    let ctx = make_ctx("coordinator", "loop-a", true);
    let (loop_id, hat_id) = gate_identifiers(&ctx);
    let fp = crate::task_verify_gate::mutation_fingerprint("add", &add_payload(), loop_id, hat_id);
    let scoped =
        crate::task_verify_gate::scoped_ticket_path(root, "add", &add_payload(), loop_id, hat_id);
    record_ticket(&scoped, &fp, loop_id, hat_id).expect("record");
    verify_gate_claim(root, &cfg, &ctx, "add", &add_payload()).expect("claim");

    // Apply fails → settle restores the prepared record.
    let err = settle_gate_claim(
        root,
        &ctx,
        "add",
        &add_payload(),
        Err(anyhow::anyhow!("simulated store failure")),
    )
    .expect_err("settle must surface the mutation error");
    assert!(err.to_string().contains("simulated store failure"));
    assert!(
        scoped.exists(),
        "failed apply must restore the prepared ticket for retry"
    );
    assert!(
        !crate::task_verify_gate::claim_marker_path(&scoped).exists(),
        "restore must remove the claim marker"
    );

    // The corrected retry re-claims without a fresh verify.
    verify_gate_claim(root, &cfg, &ctx, "add", &add_payload())
        .expect("retry must re-claim the restored ticket");
}

/// U1 (STAB-OPAC-GATES-001): settle is a no-op for callers the
/// gate bypassed (human CLI / gate off) — no claim marker was
/// ever created.
#[test]
fn test_settle_gate_claim_noop_when_gate_inactive() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    let ctx = make_ctx("coordinator", "loop-a", false);
    settle_gate_claim(root, &ctx, "add", &add_payload(), Ok(())).expect("human Ok noop");
    settle_gate_claim(
        root,
        &ctx,
        "add",
        &add_payload(),
        Err(anyhow::anyhow!("simulated")),
    )
    .expect_err("human Err still surfaces");
}

#[test]
fn test_human_add_without_verify_ok() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    let cfg = config_with_gate(true, false);
    let ctx = make_ctx("coordinator", "loop-a", false);
    // Human: no env, no ticket — gate must bypass.
    verify_gate_claim(root, &cfg, &ctx, "add", &add_payload())
        .expect("human CLI must bypass the gate");
}

#[test]
fn test_agent_gate_off_bypasses() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    let cfg = config_with_gate(false, false);
    let ctx = make_ctx("coordinator", "loop-a", true);
    verify_gate_claim(root, &cfg, &ctx, "add", &add_payload())
        .expect("gate-off must bypass for agent");
}

#[test]
fn test_unsafe_escape_hatch_bypasses_for_agent() {
    let temp_dir = tempfile::TempDir::new().expect("temp dir");
    let root = temp_dir.path();
    let cfg = config_with_gate(true, true);
    let ctx = make_ctx("coordinator", "loop-a", true);
    verify_gate_claim(root, &cfg, &ctx, "add", &add_payload())
        .expect("unsafe escape hatch must bypass");
}

#[test]
fn test_ticket_file_path_constant_stable() {
    // Defensive: the relative path is part of the public
    // contract (humans and agents both grep for it). If it
    // ever changes, the wire format breaks.
    assert_eq!(
        ticket_path(std::path::Path::new("/workspace")),
        std::path::PathBuf::from("/workspace/.ralph/agent/.ralph-task-verify-ticket")
    );
}
