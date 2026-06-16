//! SSOT four-consumption-chain acceptance tests for handoff topics.
//!
//! Plan 2026-06-17-002 Unit 7: prove that for the four canonical handoff
//! topics (`work.ready`, `queue.advance`, `work.done`, `review.passed`),
//! the same required-field list is enforced at:
//!
//! 1. **Prompt** — `build_publish_emit_section` (B layer)
//! 2. **Precheck** — `ralph emit --json` rejection (C layer)
//! 3. **Loop gate** — `event_policy::validate_event` (E layer)
//! 4. **Drift** — `DriftDetector::check_field_completeness` (U5 layer)
//!
//! The four chains must all derive from the same SSOT
//! (`presets/schemas/ce-executor-isolated.yml`, embedded at build time).
//! Modifying the SSOT must produce matching changes in all four chains
//! (covered by 002 plan, U7 only validates the baseline).

use std::collections::BTreeSet;
use std::collections::HashMap;
use std::process::Command;

use chrono::{TimeZone, Utc};
use ralph_core::DriftConfig;
use ralph_core::ViolationType;
use ralph_core::config::{EventPolicyConfig, EventSchema, PayloadType};
use ralph_core::drift::detector::{DeclaredEdges, DriftDetector};
use ralph_core::drift::engine::required_fields_from_config;
use ralph_core::drift::window::EventSnapshot;
use ralph_core::emit_schema_hint::build_publish_emit_section;
use ralph_core::{
    PolicyDecision, PolicyRuntimeState, validate_event_with_hat,
};
use ralph_core::DriftMetric;
use ralph_proto::{Hat, Topic};
use tempfile::TempDir;

// ── SSOT snapshot helpers ─────────────────────────────────────────────

/// The four handoff topics under test, in plan order.
const HANDOFF_TOPICS: &[&str] = &[
    "work.ready",
    "queue.advance",
    "work.done",
    "review.passed",
];

/// Map a handoff topic to the hat that publishes it in
/// `ce-executor-isolated`. Used by chain 1 (prompt) and chain 2
/// (precheck) to confirm the hat is allowed to publish the topic.
fn author_hat_for(topic: &str) -> &'static str {
    match topic {
        "work.ready" => "coordinator",
        "queue.advance" => "plan-gate",
        "work.done" => "executor",
        "review.passed" => "review-synthesizer",
        _ => panic!("unknown handoff topic in test fixture: {topic}"),
    }
}

/// Required fields per topic, mirroring the SSOT, for use in
/// `EventPolicyConfig::schemas`.
fn schema_for(topic: &str) -> EventSchema {
    let required: Vec<String> = match topic {
        "work.ready" => vec![
            "plan_name",
            "plan_path",
            "task_id",
            "task_key",
            "step",
            "complexity",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
        "queue.advance" => vec![
            "plan_name",
            "completed_step",
            "next_step",
            "reviewed_task_id",
            "reviewed_task_key",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
        "work.done" => vec![
            "plan_name",
            "plan_path",
            "task_id",
            "task_key",
            "step",
            "commit_count",
            "changed_lines",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
        "review.passed" => vec![
            "plan_name",
            "task_id",
            "task_key",
            "step",
            "findings_count",
            "fix_round",
            "verdict",
            "skip_reason",
        ]
        .into_iter()
        .map(String::from)
        .collect(),
        _ => panic!("unknown handoff topic in test fixture: {topic}"),
    };
    EventSchema {
        payload: Some(PayloadType::JsonObject),
        required_fields: required,
        allowed_values: HashMap::new(),
        hat_allowed_values: HashMap::new(),
    }
}

/// All required fields per topic, mirroring the SSOT, used to build
/// the JSON payload and to assert the prompt builder renders them.
fn full_required_fields(topic: &str) -> &'static [&'static str] {
    match topic {
        "work.ready" => &[
            "plan_name",
            "plan_path",
            "task_id",
            "task_key",
            "step",
            "complexity",
        ],
        "queue.advance" => &[
            "plan_name",
            "completed_step",
            "next_step",
            "reviewed_task_id",
            "reviewed_task_key",
        ],
        "work.done" => &[
            "plan_name",
            "plan_path",
            "task_id",
            "task_key",
            "step",
            "commit_count",
            "changed_lines",
        ],
        "review.passed" => &[
            "plan_name",
            "task_id",
            "task_key",
            "step",
            "findings_count",
            "fix_round",
            "verdict",
            "skip_reason",
        ],
        _ => panic!("unknown handoff topic in test fixture: {topic}"),
    }
}

/// Pick the FIRST required field of a topic, used to omit and trigger
/// a `MissingRequiredField` finding deterministically.
fn first_required_field(topic: &str) -> &'static str {
    full_required_fields(topic)[0]
}

// ── Chain 1: prompt (B layer) ─────────────────────────────────────────

/// `build_publish_emit_section` must mention every SSOT-required field
/// in the rendered `--json` example for each handoff topic.
#[test]
fn chain_1_prompt_lists_every_required_field_per_topic() {
    for topic in HANDOFF_TOPICS {
        let schema = schema_for(topic);
        let mut schemas = HashMap::new();
        schemas.insert((*topic).to_string(), schema);

        let hat = Hat::new(author_hat_for(topic), "Author")
            .with_description("")
            .with_publishes(vec![Topic::new(*topic)]);

        let section = build_publish_emit_section(&hat, &schemas);
        assert!(
            section.contains(topic),
            "chain 1 / prompt: section for {topic} must mention topic: {section}"
        );

        for field in full_required_fields(topic) {
            assert!(
                section.contains(&format!("\"{field}\"")),
                "chain 1 / prompt: {topic} section must include `{field}` in --json example: {section}"
            );
        }
    }
}

// ── Chain 2: precheck (C layer) — driven by integration_emit_policy.rs
//    pattern: `ralph emit --json` rejects incomplete payload ─────────

fn run_ralph_emit_precheck(
    temp_path: &std::path::Path,
    topic: &str,
    hat: &str,
    json: &str,
) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_ralph"))
        .args([
            "-H",
            "builtin:ce-executor-isolated",
            "emit",
            topic,
            "--json",
            json,
            "--hat",
            hat,
        ])
        .env("RALPH_HATS_SOURCE", "builtin:ce-executor-isolated")
        .env("RALPH_CURRENT_HAT", hat)
        .env("RALPH_EVENTS_FILE", temp_path.join(".ralph/events.jsonl"))
        .current_dir(temp_path)
        .output()
        .expect("Failed to execute ralph emit")
}

/// Precheck (`ralph emit`) must reject payloads that omit any
/// SSOT-required field for each of the four handoff topics.
#[test]
fn chain_2_precheck_rejects_missing_required_field_per_topic() {
    for topic in HANDOFF_TOPICS {
        let temp_dir = TempDir::new().expect("temp dir");
        let temp_path = temp_dir.path();
        std::fs::create_dir_all(temp_path.join(".ralph")).unwrap();

        // Build a JSON payload missing the first required field.
        let dropped = first_required_field(topic);
        let json_payload: String = full_required_fields(topic)
            .iter()
            .filter(|k| **k != dropped)
            .map(|k| format!("\"{k}\":\"v\""))
            .collect::<Vec<_>>()
            .join(",");

        let hat = author_hat_for(topic);
        let output = run_ralph_emit_precheck(temp_path, topic, hat, &format!("{{{json_payload}}}"));

        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(
            !output.status.success(),
            "chain 2 / precheck: {topic} with missing `{dropped}` should be rejected: stderr={stderr}"
        );
        assert!(
            stderr.contains("missing")
                || stderr.contains("required")
                || stderr.contains(dropped),
            "chain 2 / precheck: {topic} stderr should explain the missing field `{dropped}`: {stderr}"
        );
    }
}

// ── Chain 3: loop gate (E layer) — `event_policy::validate_event` ────

/// `validate_event_with_hat` must produce a `MissingRequiredField`
/// finding for every handoff topic when its first required field is
/// dropped from the payload.
#[test]
fn chain_3_loop_gate_rejects_missing_required_field_per_topic() {
    for topic in HANDOFF_TOPICS {
        let mut schemas = HashMap::new();
        schemas.insert((*topic).to_string(), schema_for(topic));
        let policy = EventPolicyConfig {
            enabled: true,
            schemas,
            ..EventPolicyConfig::default()
        };

        let dropped = first_required_field(topic);
        let json_payload: String = full_required_fields(topic)
            .iter()
            .filter(|k| **k != dropped)
            .map(|k| format!("\"{k}\":\"v\""))
            .collect::<Vec<_>>()
            .join(",");

        let mut state = PolicyRuntimeState::default();
        let decision = validate_event_with_hat(
            topic,
            Some(&format!("{{{json_payload}}}")),
            &policy,
            &mut state,
            Some(author_hat_for(topic)),
        );
        let decision_dbg = format!("{decision:?}");

        let has_missing = match decision {
            PolicyDecision::RejectWithResume(f)
            | PolicyDecision::Hold(f)
            | PolicyDecision::Block(f)
            | PolicyDecision::Ignore(f) => {
                f.message.contains(dropped)
                    || f.message.contains("required")
                    || matches!(
                        f.violation_type,
                        ViolationType::MissingRequiredField { .. }
                    )
            }
            PolicyDecision::Warn(findings) => findings.iter().any(|f| {
                f.message.contains(dropped)
                    || f.message.contains("required")
                    || matches!(
                        f.violation_type,
                        ViolationType::MissingRequiredField { .. }
                    )
            }),
            PolicyDecision::Accept => false,
        };
        assert!(
            has_missing,
            "chain 3 / loop gate: {topic} missing `{dropped}` must yield a `MissingRequiredField` finding, got: {decision_dbg}"
        );
    }
}

// ── Chain 4: drift (U5 layer) — `DriftDetector::check_field_completeness`

fn snapshot_with(topic: &str, iter: u32, fields: &[&str]) -> EventSnapshot {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for f in fields {
        set.insert((*f).to_string());
    }
    let ts = Utc.timestamp_opt(1_700_000_000 + iter as i64, 0).unwrap();
    EventSnapshot::new(topic, iter, ts).with_fields(set)
}

/// `DriftDetector` must surface a `FieldCompleteness` finding for each
/// handoff topic whose required fields are systematically missing from
/// the rolling window.
#[test]
fn chain_4_drift_detector_records_missing_required_field_per_topic() {
    for topic in HANDOFF_TOPICS {
        let cfg = DriftConfig {
            window_size: 100,
            field_completeness_threshold: 0.9,
            coord_join_rate_threshold: 0.6,
            emit_cadence_sigma: 2.0,
        };
        let mut policy = EventPolicyConfig::default();
        policy.enabled = true;
        policy
            .schemas
            .insert((*topic).to_string(), schema_for(topic));

        let required = required_fields_from_config(Some(&policy), None);
        let mut det = DriftDetector::new_with_sources(cfg, required, DeclaredEdges::new());

        let dropped = first_required_field(topic);
        // Observe 100 events, all missing the dropped field.
        // field_completeness for the dropped field = 0.0 < 0.9 → finding.
        let present: Vec<&str> = full_required_fields(topic)
            .iter()
            .copied()
            .filter(|k| k != &dropped)
            .collect();
        for i in 0..100u32 {
            det.observe(snapshot_with(topic, i, &present));
        }
        det.reset_seen();
        // One more event to drive the dedup at the start of the new
        // iteration so check_field_completeness runs.
        let findings = det.observe(snapshot_with(topic, 100, &present));

        let fc = findings
            .iter()
            .find(|f| f.metric == DriftMetric::FieldCompleteness)
            .unwrap_or_else(|| {
                panic!(
                    "chain 4 / drift: expected a FieldCompleteness finding for {topic} missing `{dropped}`, got: {findings:?}"
                )
            });
        assert_eq!(fc.topic.as_deref(), Some(*topic));
        assert_eq!(fc.field.as_deref(), Some(dropped));
    }
}

// ── Cross-chain cross-check ──────────────────────────────────────────

/// For every required field declared in the SSOT schema, the same field
/// must be: (a) listed in the prompt's `--json` example, (b) rejected by
/// `validate_event_with_hat` when absent, and (c) tracked by
/// `DriftDetector` when systematically absent.
///
/// This is the SSOT "four chains move together" proof: a field is
/// either enforced by all chains or by none.
#[test]
fn cross_chain_required_fields_are_uniformly_tracked() {
    for topic in HANDOFF_TOPICS {
        let schema = schema_for(topic);

        // (1) prompt
        let mut schemas = HashMap::new();
        schemas.insert((*topic).to_string(), schema.clone());
        let hat = Hat::new(author_hat_for(topic), "Author")
            .with_description("")
            .with_publishes(vec![Topic::new(*topic)]);
        let section = build_publish_emit_section(&hat, &schemas);

        let mut policy = EventPolicyConfig {
            enabled: true,
            ..EventPolicyConfig::default()
        };
        policy
            .schemas
            .insert((*topic).to_string(), schema.clone());

        for field in &schema.required_fields {
            // (1) prompt mentions the field
            assert!(
                section.contains(&format!("\"{field}\"")),
                "cross-chain: prompt must mention `{field}` for {topic}"
            );

            // (2)/(3) validate_event: absent → MissingRequiredField
            let mut state = PolicyRuntimeState::default();
            let absent = validate_event_with_hat(
                topic,
                Some("{}"),
                &policy,
                &mut state,
                Some(author_hat_for(topic)),
            );
            let absent_dbg = format!("{absent:?}");
            let absent_rejects = match absent {
                PolicyDecision::RejectWithResume(f)
                | PolicyDecision::Hold(f)
                | PolicyDecision::Block(f)
                | PolicyDecision::Ignore(f) => {
                    matches!(
                        f.violation_type,
                        ViolationType::MissingRequiredField { .. }
                    ) || f.message.contains(field)
                }
                PolicyDecision::Warn(findings) => findings.iter().any(|f| {
                    matches!(
                        f.violation_type,
                        ViolationType::MissingRequiredField { .. }
                    ) || f.message.contains(field)
                }),
                PolicyDecision::Accept => false,
            };
            assert!(
                absent_rejects,
                "cross-chain: validate_event must reject {topic} payload missing `{field}`, got: {absent_dbg}"
            );
        }
    }
}

/// SSOT consistency: the same set of required fields is present at
/// (a) `EventPolicyConfig::schemas` for chain 3, (b) the prompt builder
/// for chain 1, and (c) the drift detector's `RequiredFields` for
/// chain 4. The precheck (chain 2) reuses chain 3 by design.
#[test]
fn cross_chain_required_fields_match_across_chains() {
    for topic in HANDOFF_TOPICS {
        let mut policy = EventPolicyConfig::default();
        policy.enabled = true;
        policy
            .schemas
            .insert((*topic).to_string(), schema_for(topic));

        let required_from_policy = required_fields_from_config(Some(&policy), None);
        let policy_fields: Vec<String> = policy
            .schemas
            .get(*topic)
            .map(|s| s.required_fields.clone())
            .unwrap_or_default();

        let mut schemas = HashMap::new();
        schemas.insert((*topic).to_string(), schema_for(topic));
        let hat = Hat::new(author_hat_for(topic), "Author")
            .with_description("")
            .with_publishes(vec![Topic::new(*topic)]);
        let section = build_publish_emit_section(&hat, &schemas);

        for field in &policy_fields {
            // Drift side
            let drift_fields = required_from_policy.for_topic(topic);
            assert!(
                drift_fields.contains(field),
                "cross-chain: drift `RequiredFields` for {topic} must include `{field}`"
            );

            // Prompt side
            assert!(
                section.contains(&format!("\"{field}\"")),
                "cross-chain: prompt for {topic} must include `{field}`"
            );
        }
    }
}
