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

    // task.resume without hat should still work (control topic)
    write_event_to_jsonl(&events_path, "task.resume", "continue");
    let result = event_loop.process_events_from_jsonl().unwrap();

    assert!(
        result.had_events,
        "Control topic without hat should be accepted"
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
    // U9.2 baseline: no-hat trusted JSONL write is currently admitted
    // (had_events=true). If this assertion ever flips, that signals a
    // drift in the origin guard's no-hat policy — investigate before
    // assuming the characterization has merely moved.
    assert!(
        result.had_events,
        "U9.2 baseline: no-hat build.done currently accepted (drift = origin guard changed)"
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
    // U9.3 baseline: parser-shaped no-hat business event (structured
    // JSON payload) is currently admitted. If this assertion ever
    // flips, that signals a drift in the parser-path admission policy
    // — investigate whether event_policy or origin guard changed.
    assert!(
        result.had_events,
        "U9.3 baseline: parser-shaped no-hat build.done currently accepted (drift = parser-path policy changed)"
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
    // U9.4 baseline: even with strict event_policy (enforce +
    // reject_with_resume) enabled, the no-hat build.done is currently
    // admitted (result.is_ok()=true). If this assertion ever flips,
    // that signals a drift in the event_policy short-circuit behavior
    // — investigate whether policy now rejects before origin guard.
    assert!(
        result.is_ok(),
        "U9.4 baseline: strict event_policy does not short-circuit no-hat build.done (drift = event_policy pre-origin gate changed)"
    );
}

// ---- U3: close isolated terminal authority and turn-budget bypasses ----
//
// U3 spec: in isolated mode, completion promises and other agent terminal
// topics (review verdicts, report completion) must be treated as
// publishes, not as orchestrator control topics. They go through the
// normal `can_publish` + single-event budget path. Only true
// orchestrator-internal events (`task.resume`, `human.guidance`,
// `loop.cancel`, `event.*` diagnostics) bypass the budget.
//
// These tests use a private observer to inspect what reaches the bus,
// and inject an isolated hat to drive `process_parse_result` into the
// isolated-mode branch.

/// Capture all events published to the bus for inspection.
fn capture_bus_events(event_loop: &mut EventLoop) -> std::sync::Arc<std::sync::Mutex<Vec<Event>>> {
    use std::sync::{Arc, Mutex};
    let captured: Arc<Mutex<Vec<Event>>> = Arc::new(Mutex::new(Vec::new()));
    let cap = captured.clone();
    event_loop.bus.add_observer(move |event: &Event| {
        cap.lock().unwrap().push(Event {
            topic: event.topic.clone(),
            payload: event.payload.clone(),
            source: event.source.clone(),
            target: event.target.clone(),
            wave_id: event.wave_id.clone(),
            wave_index: event.wave_index,
            wave_total: event.wave_total,
            system_injected: event.system_injected,
        });
    });
    captured
}

/// U3.F4: an isolated hat that does NOT declare the completion promise
/// in its `publishes` list emits `LOOP_COMPLETE` directly. The
/// event must be rejected with an `event.isolation.boundary_violation`
/// diagnostic (no business event must be accepted, no completion
/// must be honored).  The P1 finding #11 fix moved the topic from
/// `{hat}.scope_violation` to the canonical orchestrator diagnostic
/// topic `event.isolation.boundary_violation` (hat name in payload).
/// The P1 finding #1 fix means `had_events` is true because the
/// recovery event is admitted — we therefore assert the rejection
/// via `completion_requested` and the diagnostic topic instead.
#[test]
fn test_u3_isolated_hat_undeclared_completion_rejected_with_scope_violation() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // executor publishes build.done but does NOT publish LOOP_COMPLETE.
    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Drive isolated mode: set the current isolated hat to executor.
    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));
    let captured = capture_bus_events(&mut event_loop);

    // Agent emits LOOP_COMPLETE while in executor's isolated slot.
    write_event_with_hat_to_jsonl(&events_path, "LOOP_COMPLETE", "premature done", "executor");

    let _result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !event_loop.state.completion_requested,
        "U3.F4: completion_requested must NOT be set when isolated hat has no publish scope over LOOP_COMPLETE"
    );
    // A boundary_violation diagnostic must have been published to the bus.
    let topics: Vec<String> = captured
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.topic.to_string())
        .collect();
    assert!(
        topics
            .iter()
            .any(|t| t == "event.isolation.boundary_violation"),
        "U3.F4: expected event.isolation.boundary_violation diagnostic; got topics: {topics:?}"
    );
}

/// U3.AE5: an isolated hat that DOES declare the completion promise
/// in `publishes` emits `LOOP_COMPLETE`. The event enters the
/// normal completion safety checks (completion_requested set,
/// termination reason CompletionPromise).
#[test]
fn test_u3_isolated_hat_declared_completion_enters_safety_check() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // executor DOES publish LOOP_COMPLETE — declared authority.
    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["build.done", "LOOP_COMPLETE"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    write_event_with_hat_to_jsonl(&events_path, "LOOP_COMPLETE", "Done", "executor");

    let _ = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        event_loop.state.completion_requested,
        "U3.AE5: declared completion must set completion_requested"
    );
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "U3.AE5: declared completion must reach the existing completion safety check"
    );
}

/// U3 boundary: an isolated hat that publishes a legal business event
/// first, then a declared completion promise in the same turn —
/// the completion is the second business event and must be
/// rejected with a boundary_violation diagnostic. The first
/// event is accepted, completion is NOT honored.
#[test]
fn test_u3_isolated_hat_business_then_completion_boundary_violation() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["build.done", "LOOP_COMPLETE"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));
    let captured = capture_bus_events(&mut event_loop);

    // Order: business first, then completion — both from executor.
    write_event_with_hat_to_jsonl(&events_path, "build.done", "ok", "executor");
    write_event_with_hat_to_jsonl(&events_path, "LOOP_COMPLETE", "Done", "executor");

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        result.had_events,
        "U3 boundary: first business event must be accepted"
    );
    assert!(
        !event_loop.state.completion_requested,
        "U3 boundary: second event (completion) must be rejected — completion must NOT be honored"
    );
    let topics: Vec<String> = captured
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.topic.to_string())
        .collect();
    assert!(
        topics
            .iter()
            .any(|t| t == "event.isolation.boundary_violation"),
        "U3 boundary: expected event.isolation.boundary_violation diagnostic; got topics: {topics:?}"
    );
}

/// U3 boundary: an isolated hat publishes a declared completion
/// promise first, then a business event in the same turn — the
/// completion consumes the budget, the second event must be
/// rejected with a boundary_violation diagnostic.
#[test]
fn test_u3_isolated_hat_completion_then_business_boundary_violation() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // Use a topic that is NOT `build.done` to avoid the
    // backpressure-induced `build.blocked` synthesis that pollutes
    // the captured topics with a successful orchestration event.
    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["experiment.ready", "LOOP_COMPLETE"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));
    let captured = capture_bus_events(&mut event_loop);

    // Order: completion first, then business — both from executor.
    write_event_with_hat_to_jsonl(&events_path, "LOOP_COMPLETE", "Done", "executor");
    write_event_with_hat_to_jsonl(&events_path, "experiment.ready", "ok", "executor");

    let _ = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        event_loop.state.completion_requested,
        "U3 boundary: completion must consume the budget and set completion_requested"
    );
    let topics: Vec<String> = captured
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.topic.to_string())
        .collect();
    assert!(
        topics
            .iter()
            .any(|t| t == "event.isolation.boundary_violation"),
        "U3 boundary: expected event.isolation.boundary_violation diagnostic for second event; got topics: {topics:?}"
    );
}

/// U3 control: orchestrator-internal control topics must still
/// bypass the isolated budget — `task.resume`, `human.guidance`,
/// `loop.cancel` are produced by the orchestrator and must not be
/// filtered out by the can_publish check.
#[test]
fn test_u3_isolated_control_topics_bypass_scope() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
  cancellation_promise: loop.cancel
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));
    let captured = capture_bus_events(&mut event_loop);

    // Mix: task.resume, human.guidance, loop.cancel — all no-hat
    // orchestrator-internal signals. None of them should produce
    // a scope_violation diagnostic. All should reach the bus.
    write_event_to_jsonl(&events_path, "task.resume", "{\"target\":\"executor\"}");
    write_event_to_jsonl(&events_path, "human.guidance", "All good");
    write_event_to_jsonl(&events_path, "loop.cancel", "stop");

    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        result.had_events,
        "U3 control: orchestrator control topics must be accepted in isolated mode"
    );
    let topics: Vec<String> = captured
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.topic.to_string())
        .collect();
    assert!(
        !topics.iter().any(|t| t.contains("scope_violation")),
        "U3 control: no scope_violation diagnostic should be published for control topics; got topics: {topics:?}"
    );
}

/// U3 coordinator: in coordinator (non-isolated) mode the existing
/// behavior must be preserved. An undeclared completion promise
/// from a non-isolated hat must still be admitted (coordinator
/// mode owns the can_publish check separately).
#[test]
fn test_u3_coordinator_mode_preserves_existing_completion_behavior() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  execution_mode: coordinator
  completion_promise: LOOP_COMPLETE
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["build.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // No current_isolated_hat set — coordinator mode path.
    assert!(event_loop.state.current_isolated_hat.is_none());

    write_event_to_jsonl(&events_path, "LOOP_COMPLETE", "Done");

    let _ = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        event_loop.state.completion_requested,
        "U3 coordinator: existing completion flow must still work in coordinator mode"
    );
    let reason = event_loop.check_completion_event();
    assert_eq!(
        reason,
        Some(TerminationReason::CompletionPromise),
        "U3 coordinator: completion must be honored in coordinator mode"
    );
}

// ---------------------------------------------------------------------------
// U3 P0 fix (post-review) — default_publishes must NOT bypass U3 authority
// in isolated mode. Two regressions:
//   (a) gate 1 (publish scope): if `default_publishes` is not in `publishes`,
//       the injection is dropped with a `{hat}.scope_violation` diagnostic
//       and `completion_requested` must NOT be set even when the topic
//       matches the completion promise.
//   (b) gate 2 (per-turn budget): if a JSONL business event was already
//       accepted in the current turn, a `default_publishes` business-topic
//       injection must be dropped with `event.isolation.boundary_violation`.
// ---------------------------------------------------------------------------

#[test]
fn test_u3_p0_default_publishes_out_of_scope_rejected_with_scope_violation() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // executor declares `default_publishes: LOOP_COMPLETE` but its
    // `publishes` list only contains `build.done` — the default topic
    // is *not* in the hat's declared scope. The P0 fix must drop the
    // injection with a scope_violation diagnostic, NOT set
    // completion_requested.
    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["build.done"]
    default_publishes: LOOP_COMPLETE
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Drive isolated mode: this hat is the currently-running isolated hat.
    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));
    let captured = capture_bus_events(&mut event_loop);

    // No JSONL events written by agent — `check_default_publishes` path triggers.
    // (process_events_from_jsonl returns Ok(false) for empty file.)
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(
        !result.had_events,
        "no JSONL events expected for empty file"
    );

    // The P0 fix: this call must NOT silently inject completion; it must
    // emit a scope_violation diagnostic and return without publishing.
    event_loop.check_default_publishes(&HatId::new("executor"));

    assert!(
        !event_loop.state.completion_requested,
        "P0 fix: completion_requested must NOT be set when default_publishes \
         is not in the hat's publishes list (this is the U3 bypass we are closing)"
    );

    // Verify the diagnostic was published to the bus.
    let topics: Vec<String> = captured
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.topic.to_string())
        .collect();
    assert!(
        topics.iter().any(|t| t == "executor.scope_violation"),
        "P0 fix: expected executor.scope_violation diagnostic; got topics: {topics:?}"
    );
    // And the default_publishes event itself must NOT have been published.
    assert!(
        !topics.iter().any(|t| t == "LOOP_COMPLETE"),
        "P0 fix: LOOP_COMPLETE must NOT be published when out-of-scope; got topics: {topics:?}"
    );
}

#[test]
fn test_u3_p0_default_publishes_after_business_event_rejected_with_boundary_violation() {
    use tempfile::TempDir;

    let temp_dir = TempDir::new().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // worker has `publishes: [work.done, LOOP_COMPLETE]` and
    // `default_publishes: LOOP_COMPLETE`. In a normal turn the agent
    // writes ONE business event (`work.done`); the P0 fix must prevent
    // the default_publishes injection from being accepted as a SECOND
    // business event in the same turn.
    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
hats:
  worker:
    name: "Worker"
    triggers: ["work.start"]
    publishes: ["work.done", "LOOP_COMPLETE"]
    default_publishes: LOOP_COMPLETE
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    event_loop.state.current_isolated_hat = Some(HatId::new("worker"));
    let captured = capture_bus_events(&mut event_loop);

    // Agent writes exactly one business event this turn.
    write_event_with_hat_to_jsonl(&events_path, "work.done", "done", "worker");
    let result = event_loop.process_events_from_jsonl().unwrap();
    assert!(result.had_events, "expected one accepted business event");
    // Sticky per-turn budget flag is set after the JSONL business event is
    // accepted — this is the cross-call handshake with check_default_publishes.
    assert!(
        event_loop.state.isolated_turn_business_event_accepted,
        "P0 fix: isolated_turn_business_event_accepted must be set after a \
         JSONL business event is accepted in the isolated branch"
    );

    // Now the orchestrator's process_output flow would call
    // check_default_publishes because the loop wants to ensure a default
    // event is published when the agent has not explicitly emitted one.
    // P0 fix: this must be rejected with a boundary_violation diagnostic.
    event_loop.check_default_publishes(&HatId::new("worker"));

    let topics: Vec<String> = captured
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.topic.to_string())
        .collect();

    assert!(
        topics
            .iter()
            .any(|t| t == "event.isolation.boundary_violation"),
        "P0 fix: expected event.isolation.boundary_violation diagnostic when \
         default_publishes would exceed the per-turn business-event budget; \
         got topics: {topics:?}"
    );
    // The default_publishes event must NOT have been injected (no LOOP_COMPLETE
    // from the default path this turn).
    assert!(
        !topics.iter().any(|t| t == "LOOP_COMPLETE"),
        "P0 fix: LOOP_COMPLETE must NOT be injected when budget is exhausted; \
         got topics: {topics:?}"
    );
    // And completion_requested must remain false (no second business event
    // means no completion honor).
    assert!(
        !event_loop.state.completion_requested,
        "P0 fix: completion_requested must NOT be set when default_publishes \
         was rejected by the per-turn budget gate"
    );
}

#[test]
fn test_u3_p0_default_publishes_in_scope_still_works() {
    // Positive test: when `default_publishes` IS in `publishes` AND the
    // per-turn budget has not been consumed, the injection proceeds
    // exactly like baseline behavior. This protects against an
    // over-zealous fix that would break the happy path.
    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["build.done", "LOOP_COMPLETE"]
    default_publishes: LOOP_COMPLETE
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    // No JSONL business event — budget slot is free.
    assert!(!event_loop.state.isolated_turn_business_event_accepted);

    event_loop.check_default_publishes(&HatId::new("executor"));

    assert!(
        event_loop.state.completion_requested,
        "P0 fix: completion_requested MUST still be set when default_publishes \
         is in scope and the budget is free (happy path preservation)"
    );
    assert!(
        event_loop.state.isolated_turn_business_event_accepted,
        "P0 fix: budget slot must be claimed so a subsequent JSONL business \
         event in the same turn would hit the boundary gate"
    );
}

/// R6/U2: publish_event guards ralph pseudo-hat business topics.
#[test]
fn test_publish_event_rejects_ralph_business_topic() {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let observer_count = Arc::new(AtomicUsize::new(0));
    let observer_count_clone = Arc::clone(&observer_count);
    event_loop
        .bus
        .add_observer(move |_event: &ralph_proto::Event| {
            observer_count_clone.fetch_add(1, Ordering::SeqCst);
        });

    // ralph hat publishing a business topic — must be rejected
    let bad_event =
        ralph_proto::Event::new("work.done", "{}").with_source(ralph_proto::HatId::new("ralph"));
    event_loop.publish_event(bad_event);

    // The violation event should be published instead of the original business event
    // Observer sees exactly 1 event (the boundary_violation event)
    assert_eq!(
        observer_count.load(Ordering::SeqCst),
        1,
        "ralph business topic must publish exactly one boundary_violation event"
    );
}

/// R6/U2: publish_event allows ralph control topics.
#[test]
fn test_publish_event_accepts_ralph_control_topic() {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("Test");

    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let observer_count = Arc::new(AtomicUsize::new(0));
    let observer_count_clone = Arc::clone(&observer_count);
    event_loop
        .bus
        .add_observer(move |_event: &ralph_proto::Event| {
            observer_count_clone.fetch_add(1, Ordering::SeqCst);
        });

    // ralph hat publishing a control topic — must be accepted
    let control_event =
        ralph_proto::Event::new("loop.cancel", "{}").with_source(ralph_proto::HatId::new("ralph"));
    event_loop.publish_event(control_event);

    // Observer sees exactly 1 event (the control event itself)
    assert_eq!(
        observer_count.load(Ordering::SeqCst),
        1,
        "ralph control topic must be published directly"
    );
}
