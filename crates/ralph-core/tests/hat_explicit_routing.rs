//! Pins the existing hat-routing contract: concrete subscribers win over
//! the builtin ralph hat (which subscribes to `*` as universal fallback).
//!
//! These tests document how the registry currently behaves. They are NOT
//! a fix for the ce-executor pipeline collapse observed in the field — that
//! collapse happens in a later layer (prompt build / hat invocation / LLM
//! output parsing), not in `get_for_topic` / `subscribers` / `find_by_trigger`.
//!
//! See `docs/report/2026-06-04-ce-executor-worktree-prod-audit.md` for the
//! full causal chain investigation.

use ralph_core::{HatRegistry, RalphConfig};

/// Load the ce-executor builtin preset YAML and build the runtime registry.
fn load_ce_executor_registry() -> HatRegistry {
    let yaml = include_str!("../../../presets/en/ce-executor.yml");
    let config: RalphConfig =
        serde_yaml::from_str(yaml).expect("ce-executor.yml must parse as RalphConfig");
    HatRegistry::from_runtime_config(&config)
}

#[test]
fn ce_executor_review_coordinator_receives_work_done_not_ralph() {
    let registry = load_ce_executor_registry();

    // ralph is registered as the universal-wildcard fallback hat.
    let ralph = registry
        .get(&ralph_proto::HatId::new("ralph"))
        .expect("ralph must be registered");
    assert!(
        ralph.is_fallback_only(),
        "ralph must be fallback-only (subscribe '*')"
    );

    // get_for_topic must return review-coordinator for work.done, never ralph.
    let selected = registry
        .get_for_topic("work.done")
        .expect("work.done must route to a hat");
    assert_eq!(
        selected.id.as_str(),
        "review-coordinator",
        "work.done must route to review-coordinator, NOT ralph"
    );
}

#[test]
fn ce_executor_full_chain_routes_to_explicit_hats() {
    let registry = load_ce_executor_registry();

    // Each topic in the ce-executor chain must route to its designated hat.
    let cases = [
        ("work.start", "coordinator"),
        ("work.ready", "executor"),
        ("work.done", "review-coordinator"),
        ("review.wave.ready", "dimension-reviewer"),
        ("review.dimension.done", "review-synthesizer"),
        ("review.failed", "fixer"),
        ("review.passed", "plan-gate"),
        ("review.complete", "plan-gate"),
        ("plan.complete", "shipper"),
        ("plan.blocked", "shipper"),
        ("REVIEW_COMPLETE", "reporter"),
    ];

    for (topic, expected_hat) in cases {
        let selected = registry
            .get_for_topic(topic)
            .unwrap_or_else(|| panic!("no hat subscribed to {topic}"));
        assert_eq!(
            selected.id.as_str(),
            expected_hat,
            "topic={topic} must route to {expected_hat}, got {}",
            selected.id
        );
    }
}

#[test]
fn orphan_topics_fall_back_to_ralph() {
    // Universal fallback: ralph with `*` subscription catches orphan events.
    let registry = load_ce_executor_registry();
    let selected = registry
        .get_for_topic("totally.unknown.topic")
        .expect("orphan topic should fall back to ralph");
    assert_eq!(selected.id.as_str(), "ralph");
    assert!(selected.is_fallback_only());
}

#[test]
fn work_failed_is_orphan_in_ce_executor_topology() {
    // Documents a known gap: `coordinator` and `executor` both publish
    // `work.failed`, but no hat subscribes to it. This is a preset defect,
    // NOT a hat-routing bug. The correct fix is to add a subscriber
    // (e.g. `plan-gate: triggers: ["work.failed", ...]`) so failure events
    // route into the plan-level state machine.
    let registry = load_ce_executor_registry();

    let selected = registry
        .get_for_topic("work.failed")
        .expect("work.failed should fall back to ralph (no explicit subscriber)");
    assert_eq!(selected.id.as_str(), "ralph");
}

#[test]
fn hatless_mode_ralph_subscribes_to_wildcard() {
    // Solo mode: ralph subscribes to `*` so it is reachable for any topic.
    let config = RalphConfig::default();
    let registry = HatRegistry::from_runtime_config(&config);

    let ralph = registry
        .get(&ralph_proto::HatId::new("ralph"))
        .expect("ralph must be registered in hatless mode");
    assert!(
        ralph.subscriptions.iter().any(|s| s.is_global_wildcard()),
        "ralph must subscribe to `*` in hatless mode"
    );

    let selected = registry
        .get_for_topic("anything.you.publish")
        .expect("hatless mode: any topic must fall back to ralph");
    assert_eq!(selected.id.as_str(), "ralph");
}
