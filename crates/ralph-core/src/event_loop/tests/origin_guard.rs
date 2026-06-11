//! Tests for origin_guard.

use super::common::*;
use super::*;

#[test]
fn test_origin_guard_accepts_valid_hat_event() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_with_hat_to_jsonl(&events_path, "build.done", "done", "builder");
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        result.had_events,
        "Valid hat + scope event should be accepted"
    );
}

#[test]
fn test_origin_guard_rejects_unknown_hat_event() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Event from an unknown hat (strategist is not registered)
    write_event_with_hat_to_jsonl(&events_path, "experiment.planned", "plan1", "strategist");
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !result.had_events,
        "Unknown hat event should be rejected by origin guard"
    );
}

#[test]
fn test_origin_guard_rejects_out_of_scope_event() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // builder does not publish plan.approved
    write_event_with_hat_to_jsonl(&events_path, "plan.approved", "approved", "builder");
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !result.had_events,
        "Out-of-scope event from registered hat should be rejected"
    );
}

#[test]
fn test_origin_guard_wave_event_unknown_hat_rejected() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "review.file"
    business_topics:
      - "task.done"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 2
    instructions: "Review."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Wave dispatch event from unknown hat
    {
        use std::io::Write;
        let event = serde_json::json!({
            "topic": "review.file",
            "payload": "src/main.rs",
            "ts": chrono::Utc::now().to_rfc3339(),
            "hat": "strategist",
            "wave_id": "w-1",
            "wave_index": 0,
            "wave_total": 1,
        });
        writeln!(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path)
                .unwrap(),
            "{}",
            event
        )
        .unwrap();
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();
    assert_eq!(
        result.wave_events.len(),
        0,
        "Wave event from unknown hat should be rejected by origin guard"
    );
}

#[test]
fn test_origin_guard_wave_event_valid_hat_accepted() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "review.file"
    business_topics:
      - "task.done"
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.start"]
    publishes: ["review.file"]
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 2
    instructions: "Review."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Wave dispatch event from registered hat (coordinator publishes review.file)
    // The coordinator dispatches the wave, and reviewer receives it (concurrency > 1)
    {
        use std::io::Write;
        let event = serde_json::json!({
            "topic": "review.file",
            "payload": "src/main.rs",
            "ts": chrono::Utc::now().to_rfc3339(),
            "hat": "coordinator",
            "wave_id": "w-1",
            "wave_index": 0,
            "wave_total": 1,
        });
        writeln!(
            std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&events_path)
                .unwrap(),
            "{}",
            event
        )
        .unwrap();
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();
    assert_eq!(
        result.wave_events.len(),
        1,
        "Wave event from valid hat should be accepted by origin guard"
    );
}

#[test]
fn test_origin_guard_control_topic_without_hat_accepted() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // human.interact without hat should still work
    write_event_to_jsonl(&events_path, "human.interact", "What now?");
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        result.had_events,
        "Control topic without hat should be accepted"
    );
    assert!(
        result.human_interact_context.is_some(),
        "human.interact should produce interaction context"
    );
}

#[test]
fn test_origin_guard_mixed_batch_drops_invalid_only() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Write a mix: valid event, unknown hat event, valid event
    write_event_with_hat_to_jsonl(&events_path, "build.done", "first", "builder");
    write_event_with_hat_to_jsonl(&events_path, "plan.approved", "bad", "strategist");
    write_event_with_hat_to_jsonl(&events_path, "build.done", "second", "builder");

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        result.had_events,
        "Batch with at least one valid event should have had_events"
    );
}

// ---- U9: build.done path characterization tests ----
//
// Goal (KTD-8 / plan §U9): record whether `build.done` actually reaches
// the EventBus through 4 distinct paths, *before* any code change to
// EventOriginGuard. If any of these reach the bus, the origin guard
// fix path is known; if all are rejected, the bug lies elsewhere
// (parser/active-hat attribution) and we should not touch
// EventOriginGuard.
//
// These tests deliberately use the existing test helpers and do NOT
// modify production code.

/// U9 scenario 1: isolated executor writes `build.done` directly to
/// the trusted JSONL, with explicit `hat=executor` provenance.
#[test]
fn test_u9_build_done_with_isolated_executor_hat() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_with_hat_to_jsonl(&events_path, "build.done", "ok", "builder");
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        result.had_events,
        "U9.1: builder's build.done must reach the bus (sanity baseline)"
    );
}

/// U9 scenario 2: same trusted JSONL write, but event has NO `hat` field.
/// This is the path the original 2026-06-10 report flagged as a
/// potential scope/origin bypass — characterize the actual behavior.
#[test]
fn test_u9_build_done_no_hat_field() {
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // No `hat` field — this is the "agent output parser produced a
    // no-hat build.done" path mentioned in plan §U9 / KTD-8.
    write_event_to_jsonl(&events_path, "build.done", "ok");
    let result = event_loop.process_events_from_jsonl().unwrap();
    // RECORD ONLY — do not assert pass/fail. The point of the
    // characterization is to surface what the *current* behavior is,
    // so a future change can re-record the baseline.
    eprintln!(
        "U9.2 build.done (no hat): had_events={} — characterize whether \
         the no-hat path is currently admitted (control topics are, per \
         test_origin_guard_control_topic_without_hat_accepted; \
         business topics may differ).",
        result.had_events
    );
}

/// U9 scenario 3: a no-hat `build.done` produced by the agent output
/// parser path (e.g. an isolated executor worker streaming a `done`
/// marker that gets serialized without provenance). We use
/// `write_event_to_jsonl` (no hat) plus a payload — same shape as the
/// scenario the original report flagged.
#[test]
fn test_u9_build_done_no_hat_via_trusted_path() {
    // Reuse scenario 2 setup but with a payload that looks like a
    // real agent emitted build.done.
    let yaml = r#"
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(
        &events_path,
        "build.done",
        r#"{"status":"ok","changed_files":["src/main.rs"]}"#,
    );
    let result = event_loop.process_events_from_jsonl().unwrap();
    eprintln!(
        "U9.3 build.done (no hat, structured payload): had_events={} — \
         characterize whether parser-shaped no-hat business events are \
         admitted.",
        result.had_events
    );
}

/// U9 scenario 4: enable `event_policy` with strict mode and check
/// whether the no-hat `build.done` is rejected at the policy layer
/// (independent of origin guard).
#[test]
fn test_u9_build_done_no_hat_with_strict_event_policy() {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
hats:
  builder:
    name: "Builder"
    triggers: ["build.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "build.done", "ok");
    let result = event_loop.process_events_from_jsonl();
    eprintln!(
        "U9.4 build.done (no hat, strict policy): result_ok={}, \
         characterize whether event_policy short-circuits the no-hat \
         path before origin guard runs.",
        result.is_ok()
    );
}
