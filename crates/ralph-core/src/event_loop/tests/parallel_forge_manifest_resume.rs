//! U2 (plan 2026-08-03-004): manifest-driven resume bootstrap.
//!
//! Pins the EventLoop half of the parallel-forge resume contract:
//! a validated manifest recovery publishes a TARGETED `task.resume`
//! (the existing recovery channel — no second resume message type),
//! routes the next activation to the pending hat only (no
//! round-robin drift), pins `pending_recovery_hat` at the original
//! trigger, and stays idempotent when the bootstrap repeats.

use super::*;

/// Forge-shaped hat topology: planner → guardian → executor →
/// verifier. Only the subscriptions needed for routing assertions.
fn forge_topology() -> RalphConfig {
    let yaml = r#"
hats:
  planner:
    name: "Planner"
    description: "Writes the forge plan."
    triggers: ["forge.start"]
    publishes: ["forge.plan.ready"]
  guardian:
    name: "Guardian"
    description: "Approves the forge plan."
    triggers: ["forge.plan.ready"]
    publishes: ["forge.concurrency.approved"]
  executor:
    name: "Executor"
    description: "Executes a wave unit."
    triggers: ["exec.unit.ready"]
    publishes: ["exec.unit.done"]
  verifier:
    name: "Verifier"
    description: "Verifies the integrated wave."
    triggers: ["forge.integration.done"]
    publishes: ["forge.wave.verified"]
"#;
    serde_yaml::from_str(yaml).expect("forge topology YAML must parse")
}

fn guardian_recovery() -> rejection::ManifestResumeRecovery {
    rejection::ManifestResumeRecovery {
        target_hat: HatId::new("guardian"),
        payload: serde_json::json!({
            "reason": rejection::MANIFEST_RESUME_REASON,
            "kind": rejection::MANIFEST_RESUME_REASON,
            "target_hat": "guardian",
            "original_hat": "guardian",
            "original_trigger_topic": "forge.plan.ready",
            "original_trigger_payload": {"plan_key": "pf-1"},
        })
        .to_string(),
        original_trigger_topic: "forge.plan.ready".to_string(),
        original_trigger_payload: Some("{\"plan_key\":\"pf-1\"}".to_string()),
    }
}

/// S3: the bootstrap `task.resume` routes ONLY to the pending hat.
/// No round-robin to unrelated hats; `next_hat` honours the pin and
/// consumes it exactly once.
#[test]
fn s3_manifest_resume_routes_only_to_pending_hat() {
    let mut event_loop = EventLoop::new(forge_topology());
    event_loop.initialize_manifest_resume("resume pf plan", guardian_recovery());

    // The targeted event sits in the guardian queue alone.
    let guardian_pending = event_loop
        .bus()
        .peek_pending(&HatId::new("guardian"))
        .expect("guardian must hold the bootstrap event");
    assert_eq!(guardian_pending.len(), 1);
    let bootstrap = &guardian_pending[0];
    assert_eq!(bootstrap.topic.as_str(), "task.resume");
    assert_eq!(
        bootstrap.target.as_ref().map(|h| h.as_str()),
        Some("guardian")
    );
    for other in ["planner", "executor", "verifier", "ralph"] {
        let pending = event_loop
            .bus()
            .peek_pending(&HatId::new(other))
            .map(|events| {
                events
                    .iter()
                    .filter(|event| event.topic.as_str() == "task.resume")
                    .count()
            })
            .unwrap_or(0);
        assert_eq!(pending, 0, "hat `{other}` must not receive the resume");
    }

    // The pin drives the next activation to the pending hat, then is
    // consumed (single activation, no stuck loop).
    let next = event_loop.next_hat().expect("a hat must be selected");
    assert_eq!(next.as_str(), "guardian");
    assert!(
        event_loop.state().pending_recovery_hat.is_none(),
        "the pin must be consumed by next_hat"
    );
}

/// Idempotent bootstrap: repeating the manifest bootstrap for the
/// same recovery must NOT insert a second recovery obligation into
/// the target hat's queue.
#[test]
fn manifest_resume_bootstrap_is_idempotent() {
    let mut event_loop = EventLoop::new(forge_topology());
    event_loop.initialize_manifest_resume("resume pf plan", guardian_recovery());
    event_loop.initialize_manifest_resume("resume pf plan", guardian_recovery());

    let guardian_pending = event_loop
        .bus()
        .peek_pending(&HatId::new("guardian"))
        .expect("guardian must hold the bootstrap event");
    let resumes = guardian_pending
        .iter()
        .filter(|event| event.topic.as_str() == "task.resume")
        .count();
    assert_eq!(
        resumes, 1,
        "a repeated bootstrap must not duplicate the recovery obligation"
    );
    // The pin survives the repeat so the next activation still lands
    // on the pending hat.
    assert_eq!(
        event_loop
            .state()
            .pending_recovery_hat
            .as_ref()
            .map(|h| h.as_str()),
        Some("guardian")
    );
}

/// The bootstrap keeps the objective visible to later iterations,
/// mirroring `initialize_with_topic`.
#[test]
fn manifest_resume_bootstrap_preserves_objective() {
    let mut event_loop = EventLoop::new(forge_topology());
    event_loop.initialize_manifest_resume("resume pf plan", guardian_recovery());
    let prompt = event_loop
        .build_prompt(&HatId::new("guardian"))
        .expect("guardian prompt must build");
    assert!(
        prompt.contains("task.resume"),
        "the pending hat must see its recovery event: {prompt:?}"
    );
}
