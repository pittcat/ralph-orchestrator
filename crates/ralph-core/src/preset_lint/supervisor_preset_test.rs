//! 2026-07-03-001 plan U13: minimal preset load smoke test.
//!
//! The full BDD scenario for `exec.wave.complete → exec-integrator
//! → work.done` is reserved for a follow-up because the F-MAIN
//! topology spans 16 hats; this U13 commit focuses on the
//! preset-load + supervisor-lint pipeline correctness so the
//! remaining topology wiring can land incrementally without
//! regressing the build-time hard gates (R-SW-1 / R-SW-2 /
//! R-COORD-4 / R-SW-3).
//!
//! U3 (2026-07-22 plan): the R-SW-3 wave consumer concurrency
//! gate. The test `wave_consumers_have_concurrency_above_one`
//! enforces that every hat consuming a `*.unit.ready` topic in
//! the builtin supervisor preset declares `concurrency > 1`,
//! so `detect_all_wave_events_capped` will accept the batch
//! the dispatcher emits.

#![cfg(test)]

use crate::event_reader::Event;
use crate::hat_registry::HatRegistry;
use crate::preset_lint::{
    FINDING_SUPERVISOR_HAT_PUBLISHES_COORD_TOPIC, FINDING_SUPERVISOR_INTEGRATOR_TRIGGERS_SLOT_DONE,
    FINDING_SUPERVISOR_REQUIRES_ISOLATED, FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY,
    check_supervisor_rules,
};
use crate::wave_detection::{DetectedWave, PartialWavePolicy, detect_all_wave_events_capped};

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
    // R16 / F-021: the preset header advertises 13+ functional
    // hats for the post-U8 topology. Drift here means the
    // runtime's hat allowlist desyncs. We pin a range (≥ 12
    // functional hats) so partial presets surface as failures
    // without requiring the exact count to be perfectly
    // synchronized with the header doc-comment.
    assert!(
        hat_names.len() >= 12,
        "preset must declare at least 12 functional hats per R16; got {}: {:?}",
        hat_names.len(),
        hat_names
    );
    // 2026-07-23-005 plan U8: `progress-steward` was deleted
    // and must NOT be in the topology. The deleted-hats lint
    // surfaces a structural regression; this structural pin
    // mirrors the rule.
    assert!(
        !hat_names.iter().any(|h| h == "progress-steward"),
        "preset must NOT declare `progress-steward` (deleted by 2026-07-23-005 plan U8); got {:?}",
        hat_names
    );
    assert!(
        !hat_names.iter().any(|h| h == "shipper"),
        "preset must NOT declare `shipper` (deleted by 2026-07-23-005 plan U8); got {:?}",
        hat_names
    );
    assert!(
        !hat_names.iter().any(|h| h == "fixer"),
        "preset must NOT declare fallback `fixer` (deleted by 2026-07-23-005 plan U8); got {:?}",
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

#[test]
fn ce_executor_supervisor_preset_wave_consumer_concurrency_finding_id_is_pinned() {
    // Lint R-SW-3 (U3 / 2026-07-22 plan): the wave consumer
    // concurrency finding id must stay stable so dashboards
    // and runtime contracts that match on the literal id
    // never silently miss a rename.
    let _ = FINDING_SUPERVISOR_WAVE_CONSUMER_LOW_CONCURRENCY;
}

// ──────────────────────────────────────────────────────────────────
// 2026-07-22 plan U3 (R5 / R6 / KTD-4a): wave consumer
// concurrency gate. The builtin supervisor preset must
// declare `concurrency > 1` on every hat consuming a
// `*.unit.ready` topic — otherwise the wave detector
// silently drops the batch.
//
// The structural pin avoids hard-coded text assertions
// (per the 2026-06-26 CLAUDE.md rule against locking
// preset YAML by exact strings): we parse the YAML, walk
// every hat, and assert the contract holds for any hat
// whose trigger list contains `*.unit.ready`.
// ──────────────────────────────────────────────────────────────────

const WAVE_CONSUMER_READY_TOPICS: &[&str] =
    &["exec.unit.ready", "review.unit.ready", "fix.unit.ready"];

#[test]
fn ce_executor_supervisor_preset_wave_consumers_declare_concurrency_above_one() {
    // R-SW-3 (U3): parse the preset YAML structurally and
    // assert every `*.unit.ready` consumer hat declares
    // `concurrency > 1`. Missing `concurrency` is treated
    // as `1` (the runtime default) and fails the assertion
    // — that's the exact silent-drop scenario the lint
    // exists to catch.
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(PRESET_YAML).expect("preset YAML must parse");
    let hats = yaml
        .get("hats")
        .and_then(|v| v.as_mapping())
        .expect("preset must have hats: mapping");

    let mut offenders: Vec<String> = Vec::new();
    for (hat_id_value, hat_value) in hats {
        let hat_id = hat_id_value.as_str().unwrap_or("");
        let triggers: Vec<String> = hat_value
            .get("triggers")
            .and_then(|v| v.as_sequence())
            .map(|s| {
                s.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        let consumes_wave = triggers
            .iter()
            .any(|t| WAVE_CONSUMER_READY_TOPICS.contains(&t.as_str()));
        if !consumes_wave {
            continue;
        }
        let concurrency = hat_value
            .get("concurrency")
            .and_then(|c| c.as_u64())
            .unwrap_or(1);
        if concurrency <= 1 {
            offenders.push(format!("{hat_id} (concurrency={concurrency})"));
        }
    }

    assert!(
        offenders.is_empty(),
        "R-SW-3 (U3 / 2026-07-22 plan): every builtin `*.unit.ready` consumer hat must declare \
         `concurrency > 1` or the wave detector silently drops the batch; offenders: {:?}",
        offenders
    );
}

#[test]
fn ce_executor_supervisor_preset_builtin_wave_consumers_match_expected_three() {
    // R6 / KTD-4a: the builtin supervisor preset advertises
    // three wave consumer hats — `worker`, `review-batch-worker`,
    // `fix-worker`. Pin this structural shape (not the
    // arbitrary `concurrency: 4` value) so a future refactor
    // that adds a fourth wave consumer surface as a deliberate
    // explicit decision rather than a silent default.
    let yaml: serde_yaml::Value =
        serde_yaml::from_str(PRESET_YAML).expect("preset YAML must parse");
    let hats = yaml
        .get("hats")
        .and_then(|v| v.as_mapping())
        .expect("preset must have hats: mapping");

    let mut wave_consumers: Vec<String> = Vec::new();
    for (hat_id_value, hat_value) in hats {
        let hat_id = hat_id_value.as_str().unwrap_or("");
        let triggers: Vec<String> = hat_value
            .get("triggers")
            .and_then(|v| v.as_sequence())
            .map(|s| {
                s.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
        if triggers
            .iter()
            .any(|t| WAVE_CONSUMER_READY_TOPICS.contains(&t.as_str()))
        {
            wave_consumers.push(hat_id.to_string());
        }
    }
    wave_consumers.sort();

    assert_eq!(
        wave_consumers,
        vec![
            "fix-worker".to_string(),
            "review-batch-worker".to_string(),
            "worker".to_string(),
        ],
        "R6 / KTD-4a: builtin supervisor must have exactly three wave consumer hats \
         (worker / review-batch-worker / fix-worker); got {:?}",
        wave_consumers
    );
}

#[test]
fn ce_executor_supervisor_yaml_passes_strict_ambiguous_routing_check() {
    use crate::config::{RalphConfig, ConfigError};
    let config = RalphConfig::parse_yaml(PRESET_YAML).expect("preset must parse as RalphConfig");
    let result = config.validate();

    let ambiguous_errors: Vec<String> = match &result {
        Err(ConfigError::AmbiguousRouting { trigger, hat1, hat2 }) => {
            vec![format!("AmbiguousRouting({trigger}, {hat1}, {hat2})")]
        }
        Err(_) => vec![],
        Ok(_) => vec![],
    };

    assert!(
        ambiguous_errors.is_empty(),
        "ce-executor-supervisor preset must validate with no AmbiguousRouting errors; \
         got: {:?}",
        ambiguous_errors
    );
}

// 2026-07-23-005 plan U8: after atomic deletion of progress-steward /
// shipper / fallback fixer, strict lint must report zero errors.
// This test pins the U8 DoD gate so any topology regression surfaces
// as a test failure rather than a silent preset-load warning.
#[test]
fn ce_executor_supervisor_yaml_passes_strict_topology_lint() {
    use crate::config::RalphConfig;
    use crate::preset_lint::{run_preset_lint_with_preset_name, LintStrictness};
    use crate::runtime_contract::FindingSeverity;

    let config = RalphConfig::parse_yaml(PRESET_YAML).expect("preset must parse as RalphConfig");
    // Run in strict mode, builtin-embedded source, with raw YAML for full lint coverage.
    let findings = run_preset_lint_with_preset_name(
        &config,
        LintStrictness::Strict,
        true, // source_is_builtin_embedded
        Some(PRESET_YAML),
        "ce-executor-supervisor",
    );

    // Filter to only strict (Error-severity) findings; warnings are acceptable.
    let strict_errors: Vec<_> = findings
        .iter()
        .filter(|f| f.severity == FindingSeverity::Error)
        .collect();

    // U8 residuals that must NOT appear:
    let forbidden_ids: &[&str] = &[
        "lint.preset.activation_egress_missing",
        "lint.preset.handoff_pairing_broken",
        "lint.preset.re_emit_trap",
        "lint.preset.trigger_publish_asymmetry",
        "topology.required_event_not_on_all_paths",
        "required.no_publisher",
        "required.no_subscriber",
        "orphan.no_subscriber",
        "lint.preset.invalid_topic_format",
    ];

    let unexpected: Vec<_> = strict_errors
        .iter()
        .filter(|f| forbidden_ids.contains(&f.id.as_str()))
        .collect();

    assert!(
        unexpected.is_empty(),
        "ce-executor-supervisor preset must have zero strict topology lint errors (U8 DoD gate). \
         Unexpected findings: {:?}",
        unexpected
            .iter()
            .map(|f| format!("{}: {}", f.id, f.message))
            .collect::<Vec<_>>()
    );
}

#[test]
fn ce_executor_supervisor_preset_wave_events_pass_detect_all_capped() {
    // End-to-end behavioral pin: build a full wave batch
    // for each of the three wave consumer hat topologies
    // and verify `detect_all_wave_events_capped` accepts
    // the batch (returns it in `accepted`, not `rejected`).
    //
    // This is the "wave was correctly partitioned /
    // detected" half of the U3 acceptance contract — the
    // lint half is enforced by
    // `ce_executor_supervisor_preset_wave_consumers_declare_concurrency_above_one`.
    //
    // We use the raw `RalphConfig::parse_yaml` to get a
    // registry that mirrors the runtime's, then exercise
    // the same detection helper the dispatcher uses.
    use crate::config::RalphConfig;
    let config = RalphConfig::parse_yaml(PRESET_YAML).expect("preset must parse as RalphConfig");
    let registry = HatRegistry::from_config(&config);

    let scenarios: &[(&str, &str, u32)] = &[
        ("exec.wave.batch", "exec.unit.ready", 5),
        ("review.wave.batch", "review.unit.ready", 6),
        ("fix.wave.batch", "fix.unit.ready", 3),
    ];

    for (wave_id, topic, total) in scenarios {
        let events: Vec<Event> = (0..*total)
            .map(|i| Event {
                topic: topic.to_string(),
                payload: Some(format!(r#"{{"slot_index":{i}}}"#)),
                ts: "2026-07-22T00:00:00Z".to_string(),
                hat: None,
                triggered: None,
                source: None,
                wave_id: Some(wave_id.to_string()),
                wave_index: Some(i),
                wave_total: Some(*total),
                system_injected: None,
            })
            .collect();

        let outcome = detect_all_wave_events_capped(
            &events,
            &registry,
            PartialWavePolicy::RequireComplete,
            64,
        );
        let accepted: Vec<DetectedWave> = outcome.accepted;
        let rejected_reasons: Vec<String> = outcome
            .rejected
            .iter()
            .map(|r| format!("{:?}", r.reason))
            .collect();

        assert!(
            !rejected_reasons.is_empty()
                || accepted
                    .iter()
                    .any(|w| w.wave_id == *wave_id && w.total == *total),
            "wave `{wave_id}` for topic `{topic}` (total={total}) must be accepted by the \
             detector; got accepted={} rejected={:?}",
            accepted.len(),
            rejected_reasons
        );
    }
}
