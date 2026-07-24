//! 2026-07-24-005 plan U1 review fix: every production
//! `plan.blocked` synthesis path must target `reporter`
//! (not the deleted `shipper` hat).
//!
//! Call sites covered:
//! 1. `apply_runtime_recovery_actions` / `ForcePlanBlocked`
//! 2. `run_stall_detector` steward-disabled fail-close
//! 3. `run_stall_detector` consecutive-steward escalation
//! 4. `maybe_emit_incomplete_wave_blocked`

use super::*;
use crate::event_reader::Event as JsonlEvent;
use crate::recovery_runtime::RuntimeContext;
use ralph_proto::Event as ProtoEvent;
use ralph_proto::HatId;
use std::sync::{Arc, Mutex};
use std::time::Duration;

type Observed = Arc<Mutex<Vec<(String, Option<String>)>>>;

fn install_target_observer(event_loop: &mut EventLoop) -> Observed {
    let observed: Observed = Arc::new(Mutex::new(Vec::new()));
    let observed_clone = Arc::clone(&observed);
    event_loop.bus().add_observer(move |event: &ProtoEvent| {
        observed_clone.lock().unwrap().push((
            event.topic.as_str().to_string(),
            event.target.as_ref().map(|t| t.as_str().to_string()),
        ));
    });
    observed
}

fn assert_plan_blocked_targets_reporter(observed: &Observed, context: &str) {
    let rows = observed.lock().unwrap().clone();
    let blocked: Vec<_> = rows
        .iter()
        .filter(|(topic, _)| topic == "plan.blocked")
        .collect();
    assert!(
        !blocked.is_empty(),
        "{context}: expected at least one plan.blocked; observed={rows:?}"
    );
    for (topic, target) in &blocked {
        assert_eq!(
            target.as_deref(),
            Some("reporter"),
            "{context}: {topic} must target reporter, got {target:?}; full={rows:?}"
        );
    }
}

fn make_isolated_loop(events_path: &std::path::Path, steward_enabled: bool) -> EventLoop {
    let yaml = format!(
        r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics: ["work.done"]
    business_topics: ["work.ready", "work.done"]
  execution_mode: isolated
  progress_steward:
    enabled: {steward_enabled}
    steward_hat_id: "progress-steward"
    max_steward_iterations: 3
  workflow_contract:
    incomplete_wave_gate:
      enabled: true
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.ready", "work.done"]
  progress-steward:
    name: "Progress Steward"
    triggers: ["loop.stalled", "task.resume"]
    publishes: ["work.ready", "plan.blocked"]
  reporter:
    name: "Reporter"
    triggers: ["plan.blocked"]
    publishes: ["LOOP_COMPLETE"]
  review-synthesizer:
    name: "Review Synthesizer"
    triggers: ["review.dimensions.complete"]
    publishes: ["plan.blocked", "review.passed"]
"#
    );
    let config: RalphConfig = serde_yaml::from_str(&yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U1ReporterTarget");
    event_loop.event_reader = crate::event_reader::EventReader::new(events_path);
    event_loop
}

fn write_empty_events(path: &std::path::Path) {
    let _ = std::fs::File::create(path).unwrap();
}

#[test]
fn u1_force_plan_blocked_targets_reporter() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    write_empty_events(&events_path);
    let mut event_loop = make_isolated_loop(&events_path, false);
    let observed = install_target_observer(&mut event_loop);

    // Drive the production `ForcePlanBlocked` arm via review-chain
    // retry-cap escalation (recovery_runtime::detect_retry_cap_escalation).
    let mut ctx = RuntimeContext {
        current_iteration: 1,
        current_hat: Some("review-synthesizer".to_string()),
        current_retry_key: Some("review.wave.ready:stall".to_string()),
        ..RuntimeContext::default()
    };
    ctx.retry_key_states
        .push(crate::recovery_runtime::RetryKeyState {
            retry_key: "review.wave.ready:stall".to_string(),
            last_outcome: "retry".to_string(),
            outcome_history: vec!["retry".to_string(); 4],
            attempt_count: 99,
        });
    event_loop.apply_runtime_recovery_actions(&ctx);

    assert_plan_blocked_targets_reporter(&observed, "ForcePlanBlocked");
}

#[test]
fn u1_steward_disabled_fail_close_targets_reporter() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    write_empty_events(&events_path);
    let mut event_loop = make_isolated_loop(&events_path, false);
    let observed = install_target_observer(&mut event_loop);

    event_loop.state.consecutive_no_progress_turns = 5;
    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));
    let _ = event_loop.process_events_from_jsonl();

    assert_plan_blocked_targets_reporter(&observed, "steward-disabled fail-close");
}

#[test]
fn u1_consecutive_steward_escalation_targets_reporter() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    write_empty_events(&events_path);
    let mut event_loop = make_isolated_loop(&events_path, true);
    let observed = install_target_observer(&mut event_loop);

    event_loop.state.consecutive_no_progress_turns = 3;
    event_loop.state.consecutive_steward_activations = 3;
    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));
    let _ = event_loop.process_events_from_jsonl();

    assert_plan_blocked_targets_reporter(&observed, "consecutive-steward escalation");
}

#[test]
fn u1_incomplete_wave_blocked_targets_reporter() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    write_empty_events(&events_path);
    let mut event_loop = make_isolated_loop(&events_path, false);
    let observed = install_target_observer(&mut event_loop);

    // Seed an open stalled review wave (4/11) and backdate
    // last_dimension so IncompleteWaveGate fires.
    let wave = JsonlEvent {
        topic: "review.wave.ready".to_string(),
        payload: Some(
            r#"{"plan_name":"u1","task_id":"t1","task_key":"k1","step":"1"}"#.to_string(),
        ),
        ts: String::new(),
        hat: Some("review-coordinator".to_string()),
        triggered: None,
        source: None,
        wave_id: Some("w-u1".to_string()),
        wave_index: None,
        wave_total: Some(11),
        system_injected: None,
    };
    event_loop.state.review_step_tracker.observe_accepted(&wave);
    for dim in ["d1", "d2", "d3", "d4"] {
        let dim_evt = JsonlEvent {
            topic: "review.dimension.done".to_string(),
            payload: Some(format!(
                r#"{{"plan_name":"u1","task_id":"t1","task_key":"k1","step":"1","dimension":"{dim}"}}"#
            )),
            ts: String::new(),
            hat: Some("dimension-reviewer".to_string()),
            triggered: None,
            source: None,
            wave_id: Some("w-u1".to_string()),
            wave_index: None,
            wave_total: Some(11),
            system_injected: None,
        };
        event_loop
            .state
            .review_step_tracker
            .observe_accepted(&dim_evt);
    }
    event_loop
        .state
        .review_step_tracker
        .backdate_last_dimension_for_test("u1", "t1", "1", Duration::from_secs(600));

    let emitted = event_loop.maybe_emit_incomplete_wave_blocked();
    assert!(
        emitted,
        "incomplete-wave gate must emit plan.blocked for stalled 4/11 wave"
    );
    assert_plan_blocked_targets_reporter(&observed, "incomplete-wave blocked");
}
