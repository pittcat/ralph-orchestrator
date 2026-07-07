//! 2026-07-07-002 plan Unit 4: post-terminal rejection wiring tests.

use super::*;
use ralph_proto::Event;
use tempfile::TempDir;

fn completion_guard_config() -> RalphConfig {
    let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "LOOP_COMPLETE"
    business_topics:
      - "work.done"
      - "plan.blocked"
    completion_after_terminal:
      duplicate_terminal: ignore
      business_after_completion: reject
      write_diagnostic_event: true
hats:
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#;
    serde_yaml::from_str(yaml).unwrap()
}

#[test]
fn test_post_terminal_work_done_rejected_with_diagnostic() {
    let config = completion_guard_config();
    let mut event_loop = EventLoop::new(config);
    event_loop.state.completion_honored = true;

    let observed: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let observed_clone = std::sync::Arc::clone(&observed);
    event_loop.bus().add_observer(move |event: &Event| {
        observed_clone
            .lock()
            .unwrap()
            .push(event.topic.as_str().to_string());
    });

    let result = event_loop
        .process_parse_result(crate::event_reader::ParseResult {
            events: vec![crate::event_reader::Event {
                topic: "work.done".to_string(),
                payload: Some(r#"{"step":"step-01"}"#.to_string()),
                ts: "2024-01-01T00:00:00Z".to_string(),
                wave_id: None,
                hat: Some("executor".to_string()),
                triggered: None,
                source: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            }],
            malformed: vec![],
        })
        .expect("process_parse_result");

    assert!(
        !result
            .accepted_events
            .iter()
            .any(|e| e.topic.as_str() == "work.done"),
        "post-terminal work.done must not be accepted"
    );
    let topics = observed.lock().unwrap().clone();
    assert!(
        topics.iter().any(|t| t == "event.post_terminal.rejected"),
        "expected post-terminal diagnostic, got {topics:?}"
    );
}

#[test]
fn test_post_terminal_plan_blocked_not_in_accepted_events() {
    let config = completion_guard_config();
    let mut event_loop = EventLoop::new(config);
    event_loop.state.completion_honored = true;

    let result = event_loop
        .process_parse_result(crate::event_reader::ParseResult {
            events: vec![crate::event_reader::Event {
                topic: "plan.blocked".to_string(),
                payload: Some(r#"{"reason":"late_blocked"}"#.to_string()),
                ts: "2024-01-01T00:00:00Z".to_string(),
                wave_id: None,
                hat: Some("ralph".to_string()),
                triggered: None,
                source: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            }],
            malformed: vec![],
        })
        .expect("process_parse_result");

    assert!(
        !result
            .accepted_events
            .iter()
            .any(|e| e.topic.as_str() == "plan.blocked"),
        "post-terminal plan.blocked must not enter accepted events"
    );
}

#[test]
fn test_pre_terminal_report_done_chain_still_allowed() {
    let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
  required_events: ["report.done"]
hats:
  reporter:
    name: "Reporter"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done", "LOOP_COMPLETE"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    assert!(!event_loop.state.completion_honored);

    let result = event_loop
        .process_parse_result(crate::event_reader::ParseResult {
            events: vec![crate::event_reader::Event {
                topic: "report.done".to_string(),
                payload: Some(r#"{"status":"ok"}"#.to_string()),
                ts: "2024-01-01T00:00:00Z".to_string(),
                wave_id: None,
                hat: Some("reporter".to_string()),
                triggered: None,
                source: None,
                wave_index: None,
                wave_total: None,
                system_injected: None,
            }],
            malformed: vec![],
        })
        .expect("process_parse_result");

    assert!(
        result
            .accepted_events
            .iter()
            .any(|e| e.topic.as_str() == "report.done"),
        "report.done before completion must still be accepted"
    );
}
