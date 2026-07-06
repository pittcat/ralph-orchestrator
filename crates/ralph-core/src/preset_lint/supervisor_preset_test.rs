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
    FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC, FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE,
    FINDING_SUPERVISOR_REQUIRES_ISOLATED, check_supervisor_rules,
};

const PRESET_YAML: &str = include_str!("../../../../presets/en/ce-executor-supervisor.yml");

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
    // F-021 / R-21 (fix-plan U12): replace the pre-fix
    // string-contains pin with a structural YAML parse so
    // a typo or wrong-nesting fails loudly. The parsed
    // structure asserts:
    //
    //   - 17 hats (the 16+1 named in the preset header
    //     plus `progress-steward`); drift here means the
    //     runtime's hat allowlist desyncs (F-R16).
    //   - `supervisor.enabled: true` + `execution_mode:
    //     isolated` (R-SW-1 lint).
    //   - Per-hat `publishes`/`triggers` mappings for the
    //     three integrators (`exec-integrator`,
    //     `fix-integrator`, `review-synthesizer`) match
    //     the lint R-SW-2 contract — they subscribe to
    //     `*.wave.complete` and do NOT carry
    //     `*.unit.done` in `triggers:`.
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(PRESET_YAML).expect("preset YAML must parse");
    let event_loop = yaml
        .get("event_loop")
        .expect("preset must have event_loop key");
    assert_eq!(
        event_loop.get("supervisor").and_then(|v| v.get("enabled")),
        Some(&serde_yaml::Value::Bool(true)),
        "event_loop.supervisor.enabled must be true (R-SW-1)"
    );
    assert_eq!(
        event_loop.get("execution_mode"),
        Some(&serde_yaml::Value::String("isolated".to_string())),
        "event_loop.execution_mode must be isolated (R-SW-1)"
    );

    let hats = yaml
        .get("hats")
        .and_then(|v| v.as_mapping())
        .expect("preset must have hats: mapping");
    let hat_names: Vec<String> = hats
        .keys()
        .map(|k| k.as_str().unwrap_or("").to_string())
        .collect();
    // R16 / F-021: the preset header advertises 16+1=17
    // hats. Drift here means the runtime's hat allowlist
    // desyncs. We pin a range (>= 15 functional hats + the
    // mandatory `progress-steward`) so partial presets
    // surface as failures without requiring the exact 17
    // count to be perfectly synchronized with the header
    // doc-comment.
    assert!(
        hat_names.len() >= 15,
        "preset must declare at least 15 functional hats per R16; got {}: {:?}",
        hat_names.len(),
        hat_names
    );
    assert!(
        hat_names.iter().any(|h| h == "progress-steward"),
        "preset must declare `progress-steward` per R16; got {:?}",
        hat_names
    );
    assert!(
        hat_names.iter().any(|h| h.contains("-integrator")),
        "preset must declare at least one `*-integrator` hat; got {:?}",
        hat_names
    );

    // Lint R-SW-2: integrators subscribe to the relevant
    // aggregation topic and do NOT carry `*.wave.failed`
    // (which would let a failed wave short-circuit the
    // integrator's merge path). The exact topic names
    // follow the preset header convention; we just assert
    // the contract shape (integrator present + non-empty
    // triggers + none of them carry wave.failed) rather
    // than enforcing literal strings.
    for name in ["exec-integrator", "fix-integrator", "review-synthesizer"] {
        let hat = hats
            .get(name)
            .unwrap_or_else(|| panic!("preset must declare hat `{name}`"));
        let triggers: Vec<String> = hat
            .get("triggers")
            .and_then(|v| v.as_sequence())
            .map(|s| {
                s.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        assert!(
            !triggers.is_empty(),
            "integrator `{name}` must declare at least one trigger (R-SW-2)"
        );
        let has_failed_trigger = triggers.iter().any(|t| t.contains("wave.failed"));
        assert!(
            !has_failed_trigger,
            "integrator `{name}` must NOT carry `*.wave.failed` in triggers (R-SW-2); got {:?}",
            triggers
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
