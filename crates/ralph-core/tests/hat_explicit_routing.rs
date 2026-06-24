//! Pins the hat-routing fallback contract: the builtin ralph hat subscribes
//! to `*` as universal fallback for orphan topics, and hatless mode keeps
//! that wildcard subscription.
//!
//! 2026-06-24: preset-text-specific routing tests (hardcoded
//! work.done→review-coordinator, review.passed→plan-gate,
//! work.failed→plan-gate, etc.) were removed. The preset only needs to
//! pass strict validation; per-topic routing is covered by the
//! preset_lint suite and the SSOT merge tests.

use ralph_core::{HatRegistry, RalphConfig};

/// Load the ce-executor builtin preset YAML and build the runtime registry.
fn load_ce_executor_registry() -> HatRegistry {
    let yaml = include_str!("../../../presets/en/ce-executor-serial.yml");
    let config: RalphConfig =
        serde_yaml::from_str(yaml).expect("ce-executor-serial.yml must parse as RalphConfig");
    HatRegistry::from_runtime_config(&config)
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
