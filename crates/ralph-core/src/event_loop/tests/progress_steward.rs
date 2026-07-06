//! Characterization tests for plan `2026-06-16-001` U5: the loop-
//! level `progress-steward` fallback hat.
//!
//! The runtime auto-emits a `loop.stalled` diagnostic when no
//! business event has advanced for
//! `progress_steward.max_steward_iterations` consecutive turns.
//! The `progress-steward` hat (added to the preset by U5) is the
//! recovery handler. The runtime also auto-escalates to
//! `plan.blocked(reason=loop_stalled_max_iterations)` when the
//! steward itself has been woken `max_steward_iterations` times
//! in a row without producing a forwarded business event.
//!
//! These tests exercise the runtime layer directly because the
//! actual steward agent is a hat invocation that requires the
//! full LLM machinery; the runtime's job is just to detect the
//! stall and wake the hat. The bus observer captures the
//! diagnostic events; the per-hat `peek_pending` would require
//! a `progress-steward` hat registered in the test topology
//! (which the test yaml does not declare — the production
//! preset does), so we use the observer pattern instead.

use super::*;
use ralph_proto::Event as ProtoEvent;
use std::io::Write;
use std::sync::{Arc, Mutex};

/// Install a bus observer that captures every event the bus
/// publishes during the test. Returns the shared collector so
/// the test can assert on the topic list.
fn install_bus_observer(event_loop: &mut EventLoop) -> Arc<Mutex<Vec<String>>> {
    let observed: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    let observed_clone = Arc::clone(&observed);
    event_loop.bus().add_observer(move |event: &ProtoEvent| {
        observed_clone
            .lock()
            .unwrap()
            .push(event.topic.as_str().to_string());
    });
    observed
}

/// Write a single JSONL event to the events file.
fn write_event(path: &std::path::Path, topic: &str, hat: &str) {
    let ts = chrono::Utc::now().to_rfc3339();
    let json = serde_json::json!({
        "topic": topic,
        "payload": "{}",
        "ts": ts,
        "hat": hat,
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(f, "{}", json).unwrap();
}

/// Build a minimal isolated-mode event loop with `executor` as the
/// publishing hat. The `progress-steward` hat IS declared here
/// because the runtime's stall detector cross-validates the
/// configured `steward_hat_id` against the runtime registry
/// (F-REL-002) — a missing hat causes the wake to be skipped.
/// Without this declaration the wake branch logs a warn and
/// skips the `loop.stalled` emit.
fn make_isolated_stall_loop(events_path: &std::path::Path) -> EventLoop {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics: ["work.done"]
    business_topics: ["work.ready", "work.done"]
  execution_mode: isolated
  progress_steward:
    enabled: true
    steward_hat_id: "progress-steward"
    max_steward_iterations: 3
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.ready", "work.done"]
  progress-steward:
    name: "🛟 Progress Steward"
    triggers: ["loop.stalled", "task.resume"]
    publishes: ["work.ready", "queue.advance", "review.wave.ready", "task.resume", "plan.blocked"]
    terminal_events: ["plan.blocked"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("TestU5Steward");
    event_loop.event_reader = crate::event_reader::EventReader::new(events_path);
    event_loop
}

/// U5.HAPPY: a turn that admits a business event (`work.ready`)
/// keeps `consecutive_no_progress_turns = 0`. A subsequent
/// no-progress turn increments the counter; after 3 such turns
/// the runtime auto-emits `loop.stalled` with `target =
/// progress-steward`.
///
/// We test the runtime's stall detector by writing a no-event
/// JSONL (the loop sees an empty events file) and asserting on
/// the diagnostic + bus state after the runtime's stall detector
/// runs. The exact diagnostic shape is checked via the bus
/// observer (the per-hat `peek_pending` would require a
/// `progress-steward` hat registered in the test topology).
#[test]
fn test_u5_stall_detector_emits_loop_stalled_after_threshold() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_stall_loop(&events_path);
    let observed = install_bus_observer(&mut event_loop);
    // Pre-fill the counter as if the loop had two prior
    // no-progress turns. The next no-progress turn must reach
    // `max_steward_iterations = 3` and trigger `loop.stalled`.
    event_loop.state.consecutive_no_progress_turns = 2;
    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    // Empty events file → no business event admitted → no-progress turn.
    let _ = event_loop.process_events_from_jsonl();

    let observed_topics = observed.lock().unwrap().clone();

    assert!(
        observed_topics.iter().any(|t| t == "loop.stalled"),
        "loop.stalled must be emitted on the 3rd consecutive no-progress turn; observed: {observed_topics:?}"
    );

    // Steward activation counter incremented.
    assert_eq!(
        event_loop.state.consecutive_steward_activations, 1,
        "steward activation counter must be incremented after the wake"
    );
    assert!(
        event_loop.state.steward_woken_this_turn,
        "steward_woken_this_turn flag must be set to suppress recursive wakes"
    );
}

/// U5.PROGRESS-RESETS: a turn that admits a business event
/// resets `consecutive_no_progress_turns` to 0 and the steward
/// activation counter to 0. The next stall detection starts
/// from a clean slate.
#[test]
fn test_u5_admitted_business_event_resets_stall_counter() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_stall_loop(&events_path);
    let _observed = install_bus_observer(&mut event_loop);
    // Pre-fill counters to simulate a previously stalled loop.
    event_loop.state.consecutive_no_progress_turns = 2;
    event_loop.state.consecutive_steward_activations = 1;
    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    // This turn admits `work.ready` → business event → reset.
    write_event(&events_path, "work.ready", "executor");
    let _ = event_loop.process_events_from_jsonl();

    assert_eq!(
        event_loop.state.consecutive_no_progress_turns, 0,
        "consecutive_no_progress_turns must be reset to 0 when a business event is admitted"
    );
    assert_eq!(
        event_loop.state.consecutive_steward_activations, 0,
        "consecutive_steward_activations must be reset to 0 when a business event is admitted (steward produced progress)"
    );
    assert!(
        !event_loop.state.steward_woken_this_turn,
        "steward_woken_this_turn must be cleared after a successful business event"
    );
}

/// U5.ESCALATION: after `max_steward_iterations` consecutive
/// steward activations without a forwarded business event, the
/// runtime auto-emits `plan.blocked(reason=loop_stalled_max_iterations)`
/// and resets the counters. This terminates the loop cleanly
/// via shipper → reporter.
#[test]
fn test_u5_steward_self_loop_escalates_to_plan_blocked() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_stall_loop(&events_path);
    let observed = install_bus_observer(&mut event_loop);

    // Simulate the steward having been woken `max_iter` times in
    // a row without producing a forwarded business event. The
    // runtime's `process_events_from_jsonl` will see an empty
    // events file and run the stall detector with
    // `consecutive_steward_activations >= max_iter`. The
    // escalation branch must emit `plan.blocked`.
    event_loop.state.consecutive_no_progress_turns = 3;
    event_loop.state.consecutive_steward_activations = 3;
    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    let _ = event_loop.process_events_from_jsonl();

    let observed_topics = observed.lock().unwrap().clone();

    assert!(
        observed_topics.iter().any(|t| t == "plan.blocked"),
        "plan.blocked must be emitted when the steward is itself stuck; observed: {observed_topics:?}"
    );

    // Counters reset so the next loop (e.g. a follow-up
    // diagnostic or operator restart) starts from a clean state.
    assert_eq!(
        event_loop.state.consecutive_no_progress_turns, 0,
        "consecutive_no_progress_turns must be reset after plan.blocked escalation"
    );
    assert_eq!(
        event_loop.state.consecutive_steward_activations, 0,
        "consecutive_steward_activations must be reset after plan.blocked escalation"
    );
}

/// U5.DISABLED: `progress_steward.enabled = false` skips
/// `loop.stalled` wake but still fail-closes with `plan.blocked`
/// after `max_steward_iterations` no-progress turns (R9).
#[test]
fn test_u5_disabled_steward_fail_closes_without_loop_stalled() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_stall_loop(&events_path);
    event_loop.config.event_loop.progress_steward.enabled = false;
    let observed = install_bus_observer(&mut event_loop);

    // Pre-fill counter; the disabled steward must not fire.
    event_loop.state.consecutive_no_progress_turns = 5;
    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    let _ = event_loop.process_events_from_jsonl();

    let observed_topics = observed.lock().unwrap().clone();
    let stalled_count = observed_topics
        .iter()
        .filter(|t| *t == "loop.stalled")
        .count();
    let plan_blocked_count = observed_topics
        .iter()
        .filter(|t| *t == "plan.blocked")
        .count();

    assert_eq!(
        stalled_count, 0,
        "disabled steward must not emit loop.stalled; got {stalled_count}"
    );
    assert_eq!(
        plan_blocked_count, 1,
        "disabled steward must fail-close with plan.blocked when counter \
         exceeds max_steward_iterations; got {plan_blocked_count}"
    );
}

/// U5.SELF-PROTECTION: when the steward is woken in the current
/// turn, the stall detector does NOT fire a second `loop.stalled`
/// on the same turn. This prevents a recursive stall when the
/// steward's own emit is rejected by the origin guard.
#[test]
fn test_u5_steward_woken_this_turn_prevents_recurisve_wake() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_stall_loop(&events_path);
    let observed = install_bus_observer(&mut event_loop);
    // Pre-set the flag as if a previous sub-step (or the
    // runtime's own emit of loop.stalled) had already woken the
    // steward in this turn. The stall detector must skip the
    // wake so we don't recursively fire loop.stalled.
    event_loop.state.consecutive_no_progress_turns = 5;
    event_loop.state.consecutive_steward_activations = 1;
    event_loop.state.steward_woken_this_turn = true;
    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    let _ = event_loop.process_events_from_jsonl();

    let observed_topics = observed.lock().unwrap().clone();
    let stalled_count = observed_topics
        .iter()
        .filter(|t| *t == "loop.stalled")
        .count();
    assert_eq!(
        stalled_count, 0,
        "steward_woken_this_turn must suppress recursive loop.stalled emits; got {stalled_count}"
    );
}

/// U1.HAPPY-via-WAVES: process_events_from_jsonl_with_waves() resets
/// the stall detector at the start of each call. Three consecutive
/// empty JSONL calls via this entry point must emit `loop.stalled`
/// on the 3rd turn — the same as `process_events_from_jsonl()`.
#[test]
fn test_u1_loop_stalled_via_process_events_from_jsonl_with_waves() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_stall_loop(&events_path);
    let observed = install_bus_observer(&mut event_loop);
    // Pre-fill: two prior no-progress turns, so the 3rd empty call
    // (after the two we do below) triggers the threshold.
    event_loop.state.consecutive_no_progress_turns = 2;
    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    // Turn 1: empty JSONL → no-progress.
    let _ = event_loop.process_events_from_jsonl_with_waves().unwrap();
    assert_eq!(
        event_loop.state.consecutive_no_progress_turns, 3,
        "consecutive_no_progress_turns must reach 3 after 3 empty turns"
    );

    let observed_topics = observed.lock().unwrap().clone();
    assert!(
        observed_topics.iter().any(|t| t == "loop.stalled"),
        "loop.stalled must be emitted on the 3rd consecutive no-progress turn via process_events_from_jsonl_with_waves; observed: {observed_topics:?}"
    );
}

/// U1.REGRESSION: pre-2026-06-17 the stall detector state was NOT
/// reset in process_events_from_jsonl_with_waves(), so a
/// `work.ready` event processed via that entry point would NOT
/// reset the counter. After the fix, a business event admitted
/// through process_events_from_jsonl_with_waves() correctly resets
/// the counter, and subsequent empty turns eventually trigger
/// `loop.stalled` again.
#[test]
fn test_u1_work_ready_via_process_events_from_jsonl_with_waves_resets_counter() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_stall_loop(&events_path);
    let observed = install_bus_observer(&mut event_loop);

    // Simulate a previously-stalled loop.
    event_loop.state.consecutive_no_progress_turns = 2;
    event_loop.state.consecutive_steward_activations = 1;
    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    // This turn admits work.ready via process_events_from_jsonl_with_waves → reset.
    write_event(&events_path, "work.ready", "executor");
    let _ = event_loop.process_events_from_jsonl_with_waves().unwrap();

    assert_eq!(
        event_loop.state.consecutive_no_progress_turns, 0,
        "consecutive_no_progress_turns must be reset to 0 when work.ready is admitted via process_events_from_jsonl_with_waves"
    );
    assert_eq!(
        event_loop.state.consecutive_steward_activations, 0,
        "consecutive_steward_activations must be reset to 0 after a business event admitted via process_events_from_jsonl_with_waves"
    );

    // Now simulate two more empty turns to confirm the counter
    // advances again and triggers loop.stalled on the 3rd.
    event_loop.state.consecutive_no_progress_turns = 2;
    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    let _ = event_loop.process_events_from_jsonl_with_waves().unwrap();

    let observed_topics = observed.lock().unwrap().clone();
    assert!(
        observed_topics.iter().any(|t| t == "loop.stalled"),
        "loop.stalled must fire on 3rd empty turn after reset via process_events_from_jsonl_with_waves; observed: {observed_topics:?}"
    );
}
