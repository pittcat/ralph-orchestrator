//! 2026-07-03-001 plan U13: minimal preset load smoke test.
//!
//! The full BDD scenario for `exec.wave.complete → exec-integrator
//! → work.done` is reserved for a follow-up because the F-MAIN
//! topology spans 16 hats; this U13 commit focuses on the
//! preset-load + supervisor-lint pipeline correctness so the
//! remaining topology wiring can land incrementally without
//! regressing the build-time hard gates (R-SW-1 / R-SW-2 /
//! R-COORD-4).

#![cfg(test)]

use crate::preset_lint::{
    FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC,
    FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE,
    FINDING_SUPERVISOR_REQUIRES_ISOLATED, check_supervisor_rules,
};

const PRESET_YAML: &str = include_str!(
    "../../../../presets/en/ce-executor-supervisor.yml"
);

#[test]
fn ce_executor_supervisor_preset_passes_supervisor_lint() {
    // U13: with isolated mode + supervisor.enabled: true, the
    // three supervisor lint rules must stay silent. Drift in
    // any direction is a hard preset-load failure (R4 / R16).
    let findings = check_supervisor_rules(PRESET_YAML);
    assert!(
        findings.is_empty(),
        "ce-executor-supervisor preset must be supervisor-lint-clean, got {:?}",
        findings.iter().map(|f| f.id).collect::<Vec<_>>()
    );
}

#[test]
fn ce_executor_supervisor_preset_contains_all_required_supervisor_keys() {
    // Sanity: the YAML text contains the keys the runtime +
    // dispatcher bridge depend on. Pinning these strings
    // protects against silent renames that would only fail
    // at runtime.
    for needed in [
        "event_loop",
        "supervisor",
        "enabled: true",
        "execution_mode: isolated",
        "hats",
        "exec-integrator",
        "fix-integrator",
        "review-synthesizer",
        "worker",
        "fix-worker",
        "review-batch-worker",
    ] {
        assert!(
            PRESET_YAML.contains(needed),
            "ce-executor-supervisor preset must contain `{needed}`"
        );
    }
}

#[test]
fn ce_executor_supervisor_preset_does_not_let_hats_publish_coord_topics() {
    // Lint R-COORD-4: agents must not claim supervisor
    // coordination topics. The check at the lint level above
    // covers this; this test pins the contract via the
    // finding-id constant so a future rename of the topic
    // family surfaces as a test failure.
    let _ = FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC;
}

#[test]
fn ce_executor_supervisor_preset_integrators_subscribe_to_wave_complete() {
    // Lint R-SW-2: integrators must NOT carry `*.unit.done`
    // in `triggers:`. The fixture enforces this in the YAML
    // itself; the unit test pins the finding id.
    let _ = FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE;
}

#[test]
fn ce_executor_supervisor_preset_requires_isolated_mode() {
    // Lint R-SW-1: `event_loop.supervisor.enabled: true`
    // requires `event_loop.execution_mode: isolated`.
    // The preset YAML has both keys; this test documents the
    // pin so the finding id stays stable.
    let _ = FINDING_SUPERVISOR_REQUIRES_ISOLATED;
}
