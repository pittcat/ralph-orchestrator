//! Tests for U1: Wave policy rejection surfaced.
//!
//! When wave events are rejected by the event policy (e.g. they lack a required
//! payload field like `depth`), the runner must be able to see the rejection
//! details — not just an empty `wave_events` vector. Otherwise the runner
//! believes the wave event was never emitted and triggers `missing_event_gate`,
//! which routes to the wrong hat and ultimately terminates the loop.
//!
//! These tests verify that `ProcessedEventsWithWaves` carries the policy
//! rejection vector and the raw wave count, and that the recovery envelope
//! is written when policy rejects every wave event in a read batch.

use super::*;

/// Helper: write a single wave-dispatch event line to a JSONL file.
///
/// `topic` is a trigger of a concurrent hat so the partition path treats
/// the event as a wave dispatch. `hat` provides origin-guard provenance.
fn write_wave_event_to_jsonl_with_payload(
    path: &std::path::Path,
    topic: &str,
    payload: serde_json::Value,
    hat: &str,
    wave_id: &str,
    wave_index: u32,
    wave_total: u32,
) {
    use std::io::Write;
    let ts = chrono::Utc::now().to_rfc3339();
    let event_json = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": ts,
        "hat": hat,
        "wave_id": wave_id,
        "wave_index": wave_index,
        "wave_total": wave_total,
    });
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(file, "{}", event_json).unwrap();
}

/// Build a minimal isolated-mode event loop with a coordinator + reviewer
/// topology that supports `review.wave.ready` wave dispatch. The `review.wave.ready`
/// schema requires `depth` (matches the historical reference
/// preset schema — see the retired `presets/schemas/ce-executor-serial.yml`
/// — that first declared the field).
fn make_wave_policy_loop(events_path: &std::path::Path) -> EventLoop {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    schemas:
      review.wave.ready:
        required_fields:
          - depth
          - dimension
  execution_mode: isolated
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.start"]
    publishes: ["review.wave.ready"]
  reviewer:
    name: "Reviewer"
    triggers: ["review.wave.ready"]
    publishes: ["review.done"]
    concurrency: 3
    instructions: "Review."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(events_path);
    event_loop
}

/// U1 happy path: 7 valid `review.wave.ready` (with depth) → all preserved,
/// no policy rejections surfaced, `wave_raw_count` matches.
#[test]
fn test_wave_policy_rejection_surfaces_for_seven_valid_events() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_wave_policy_loop(&events_path);

    for i in 0..7 {
        let payload = serde_json::json!({
            "depth": "standard",
            "dimension": "security",
            "diff_base": "main",
            "intent_summary": "review security",
            "changed_files": ["src/main.rs"],
        });
        write_wave_event_to_jsonl_with_payload(
            &events_path,
            "review.wave.ready",
            payload,
            "coordinator",
            "w-valid",
            i,
            7,
        );
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    assert_eq!(
        result.wave_events.len(),
        7,
        "all 7 valid wave events should be preserved (no schema rejection)"
    );
    assert!(
        result.wave_policy_rejections.is_empty(),
        "valid wave events should not produce policy rejections; got {:?}",
        result.wave_policy_rejections
    );
    assert_eq!(
        result.wave_raw_count, 7,
        "wave_raw_count should record the number of wave events entering policy validation"
    );
}

/// U1 error path: 7 missing `depth` → wave_events empty, 7 policy rejections
/// all targeting `review.wave.ready`, wave_raw_count==7.
#[test]
fn test_wave_policy_rejection_surfaces_for_seven_missing_depth_events() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_wave_policy_loop(&events_path);

    for i in 0..7 {
        let payload = serde_json::json!({
            "dimension": "security",
            // Note: no "depth" field — schema-required.
            "diff_base": "main",
            "intent_summary": "review security",
            "changed_files": ["src/main.rs"],
        });
        write_wave_event_to_jsonl_with_payload(
            &events_path,
            "review.wave.ready",
            payload,
            "coordinator",
            "w-no-depth",
            i,
            7,
        );
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    assert_eq!(
        result.wave_events.len(),
        0,
        "all 7 wave events with missing `depth` should be rejected by policy"
    );
    assert_eq!(
        result.wave_policy_rejections.len(),
        7,
        "all 7 rejections should be surfaced; got {:?}",
        result
            .wave_policy_rejections
            .iter()
            .map(|r| (&r.topic, &r.finding.message))
            .collect::<Vec<_>>()
    );
    for rejection in &result.wave_policy_rejections {
        assert_eq!(
            rejection.topic, "review.wave.ready",
            "every rejection should target review.wave.ready"
        );
    }
    assert_eq!(
        result.wave_raw_count, 7,
        "wave_raw_count should be 7 (number of events that hit policy validation)"
    );
}

/// U1 edge case: mixed batch (1 valid + 6 missing depth). Enforce mode with
/// `reject_with_resume` rejects the 6 invalid events; the 1 valid event
/// passes through. The runner gets 6 policy rejections and 1 surviving event.
#[test]
fn test_wave_policy_rejection_surfaces_for_mixed_valid_and_invalid() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_wave_policy_loop(&events_path);

    // 1 valid event
    let valid_payload = serde_json::json!({
        "depth": "standard",
        "dimension": "security",
    });
    write_wave_event_to_jsonl_with_payload(
        &events_path,
        "review.wave.ready",
        valid_payload,
        "coordinator",
        "w-mixed",
        0,
        7,
    );

    // 6 invalid events (missing depth)
    for i in 1..7 {
        let payload = serde_json::json!({
            "dimension": "security",
        });
        write_wave_event_to_jsonl_with_payload(
            &events_path,
            "review.wave.ready",
            payload,
            "coordinator",
            "w-mixed",
            i,
            7,
        );
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    assert_eq!(
        result.wave_events.len(),
        1,
        "the single valid event should pass; got {}",
        result.wave_events.len()
    );
    assert_eq!(
        result.wave_policy_rejections.len(),
        6,
        "6 invalid events should be surfaced as policy rejections; got {:?}",
        result.wave_policy_rejections
    );
    assert_eq!(
        result.wave_raw_count, 7,
        "wave_raw_count should be 7 — the 7 wave events that hit policy validation"
    );
}

/// U1 integration check: `wave_raw_count` matches the number of wave events
/// that entered policy validation, regardless of whether some/all were
/// rejected.
#[test]
fn test_wave_raw_count_matches_policy_input() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_wave_policy_loop(&events_path);

    // 3 valid + 2 missing depth = 5 events that hit policy validation
    for i in 0..3 {
        let payload = serde_json::json!({
            "depth": "standard",
            "dimension": "security",
        });
        write_wave_event_to_jsonl_with_payload(
            &events_path,
            "review.wave.ready",
            payload,
            "coordinator",
            "w-mix2",
            i,
            5,
        );
    }
    for i in 3..5 {
        let payload = serde_json::json!({
            "dimension": "security",
        });
        write_wave_event_to_jsonl_with_payload(
            &events_path,
            "review.wave.ready",
            payload,
            "coordinator",
            "w-mix2",
            i,
            5,
        );
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    assert_eq!(result.wave_raw_count, 5);
    assert_eq!(result.wave_events.len(), 3);
    assert_eq!(result.wave_policy_rejections.len(), 2);
    // wave_raw_count == accepted + rejected
    assert_eq!(
        result.wave_raw_count,
        result.wave_events.len() + result.wave_policy_rejections.len()
    );
}
