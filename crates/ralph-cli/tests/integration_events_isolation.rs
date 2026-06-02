//! Integration tests for events isolation (Unit 4: raw output history vs trusted events).
//!
//! These tests verify that the history logger (used for raw output parsing,
//! orphan events, and termination events) writes to a SEPARATE file from
//! the trusted events file consumed by EventReader.

use ralph_core::{EventHistory, EventLogger, EventReader, EventRecord, LoopContext};
use ralph_proto::Event;
use tempfile::TempDir;

/// Creates a LoopContext with a current-events marker pointing to a timestamped file.
fn setup_context(tmp: &TempDir) -> LoopContext {
    let ctx = LoopContext::primary(tmp.path().to_path_buf());
    let marker = ctx.current_events_marker();
    std::fs::create_dir_all(marker.parent().unwrap()).unwrap();
    std::fs::write(&marker, ".ralph/events-20260602-120000.jsonl").unwrap();
    ctx
}

/// Happy path: agent really emits `work.done`, trusted events file contains it.
#[test]
fn test_real_emit_appears_in_trusted_events_file() {
    let tmp = TempDir::new().unwrap();
    let ctx = setup_context(&tmp);

    // Simulate ralph emit: write to the trusted events file
    let mut trusted_logger = EventLogger::from_context(&ctx);
    let event = Event::new("work.done", "task completed successfully");
    trusted_logger
        .log_event(1, "loop", &event, None, None)
        .unwrap();

    // EventReader reads from the same trusted file
    let trusted_path = tmp.path().join(".ralph/events-20260602-120000.jsonl");
    let mut reader = EventReader::new(&trusted_path);
    let result = reader.read_new_events().unwrap();

    assert_eq!(result.events.len(), 1);
    assert_eq!(result.events[0].topic, "work.done");
    assert_eq!(
        result.events[0].payload.as_deref(),
        Some("task completed successfully")
    );
}

/// Regression: raw output contains XML event tags, history has it, trusted does not.
#[test]
fn test_xml_events_in_output_only_in_history() {
    let tmp = TempDir::new().unwrap();
    let ctx = setup_context(&tmp);

    // Simulate raw output containing an XML event tag (fake/demo event)
    let raw_output = r#"Here is my analysis:
<event topic="debug.step">stepping through code</event>
Done with analysis."#;

    // Parse events from output (simulates EventParser behavior)
    let parser = ralph_core::EventParser::new();
    let parsed_events = parser.parse(raw_output);

    // There should be a parsed event from the XML
    assert!(
        !parsed_events.is_empty(),
        "parser should extract XML events from raw output"
    );

    // Write parsed events to the HISTORY logger (not trusted)
    let mut history_logger = EventLogger::history_from_context(&ctx);
    for event in &parsed_events {
        let record = EventRecord::new(1, "loop", event, None, None);
        history_logger.log(&record).unwrap();
    }

    // Also simulate a real ralph emit in the trusted file
    let mut trusted_logger = EventLogger::from_context(&ctx);
    let real_event = Event::new("work.done", "real work");
    trusted_logger
        .log_event(1, "loop", &real_event, None, None)
        .unwrap();

    // History file should contain the parsed XML event
    let history_path = tmp
        .path()
        .join(".ralph/events-history-20260602-120000.jsonl");
    assert!(history_path.exists());
    let history = EventHistory::new(&history_path);
    let history_records = history.read_all().unwrap();
    assert!(
        history_records.iter().any(|r| r.topic == "debug.step"),
        "history should contain the parsed XML event: {:?}",
        history_records.iter().map(|r| &r.topic).collect::<Vec<_>>()
    );

    // Trusted file should ONLY contain the real emit, NOT the parsed XML event
    let trusted_path = tmp.path().join(".ralph/events-20260602-120000.jsonl");
    let mut reader = EventReader::new(&trusted_path);
    let result = reader.read_new_events().unwrap();

    assert_eq!(result.events.len(), 1);
    assert_eq!(result.events[0].topic, "work.done");
    assert!(
        result.events.iter().all(|e| e.topic != "debug.step"),
        "trusted events file should not contain fake XML events"
    );
}

/// Regression: raw output contains JSON lines — EventParser does NOT extract them
/// as events (only XML `<event>` tags are parsed). JSON lines in raw output
/// are just text and should never enter any events file.
#[test]
fn test_json_lines_in_output_not_extracted_as_events() {
    let tmp = TempDir::new().unwrap();
    let _ctx = setup_context(&tmp);

    // Simulate raw output containing JSON event lines
    let raw_output = r#"{"topic":"experiment.planned","payload":{"task_key":"x"},"ts":"2026-06-02T12:00:00Z"}
{"topic":"analysis.complete","ts":"2026-06-02T12:00:01Z"}"#;

    // EventParser only extracts XML <event> tags, not raw JSON lines
    let parser = ralph_core::EventParser::new();
    let parsed_events = parser.parse(raw_output);

    // The parser should NOT extract JSON lines as events
    assert!(
        parsed_events.is_empty(),
        "EventParser should not extract raw JSON lines as events, got: {:?}",
        parsed_events.iter().map(|e| &e.topic).collect::<Vec<_>>()
    );

    // Therefore no events should be written to any file
    // Trusted file should NOT exist
    let trusted_path = tmp.path().join(".ralph/events-20260602-120000.jsonl");
    assert!(
        !trusted_path.exists(),
        "trusted events file should not be created from raw JSON lines in output"
    );

    // History file should also NOT exist (no events were parsed)
    let history_path = tmp
        .path()
        .join(".ralph/events-history-20260602-120000.jsonl");
    assert!(
        !history_path.exists(),
        "history file should not be created when no events are parsed"
    );
}

/// Regression: event.orphaned only in history/diagnostics, not consumed by EventReader.
#[test]
fn test_orphaned_events_only_in_history() {
    let tmp = TempDir::new().unwrap();
    let ctx = setup_context(&tmp);

    // Write an orphan event to the history logger
    let mut history_logger = EventLogger::history_from_context(&ctx);
    let orphan_event = Event::new("event.orphaned", "Event 'debug.step' has no subscriber hat");
    let record = EventRecord::new(1, "loop", &orphan_event, None, None);
    history_logger.log(&record).unwrap();

    // History file should contain the orphan event
    let history_path = tmp
        .path()
        .join(".ralph/events-history-20260602-120000.jsonl");
    assert!(history_path.exists());
    let history = EventHistory::new(&history_path);
    let history_records = history.read_all().unwrap();
    assert_eq!(history_records.len(), 1);
    assert_eq!(history_records[0].topic, "event.orphaned");

    // Trusted file should NOT exist
    let trusted_path = tmp.path().join(".ralph/events-20260602-120000.jsonl");
    assert!(
        !trusted_path.exists(),
        "event.orphaned should not appear in trusted events file"
    );
}

/// Edge case: state-machine candidate events file is not polluted by history logger.
#[test]
fn test_candidate_events_not_polluted_by_history() {
    let tmp = TempDir::new().unwrap();
    let ctx = setup_context(&tmp);

    // Write to the candidate events file (simulating ralph emit in state-machine mode)
    let candidate_path = tmp
        .path()
        .join(".ralph/event-candidates-20260602-120000.jsonl");
    let mut candidate_logger = EventLogger::new(&candidate_path);
    let real_event = Event::new("experiment.ready", "ready for evaluation");
    candidate_logger
        .log_event(1, "loop", &real_event, None, None)
        .unwrap();

    // Write to history logger (raw output parsing)
    let mut history_logger = EventLogger::history_from_context(&ctx);
    let fake_event = Event::new("experiment.planned", "fake from output text");
    let record = EventRecord::new(1, "loop", &fake_event, None, None);
    history_logger.log(&record).unwrap();

    // Candidate file should only contain the real emit
    let candidate_history = EventHistory::new(&candidate_path);
    let candidate_records = candidate_history.read_all().unwrap();
    assert_eq!(candidate_records.len(), 1);
    assert_eq!(candidate_records[0].topic, "experiment.ready");

    // History file should only contain the fake event
    let history_path = tmp
        .path()
        .join(".ralph/events-history-20260602-120000.jsonl");
    let history_history = EventHistory::new(&history_path);
    let history_records = history_history.read_all().unwrap();
    assert_eq!(history_records.len(), 1);
    assert_eq!(history_records[0].topic, "experiment.planned");
}

/// Edge case: termination events go to history only.
#[test]
fn test_terminate_event_only_in_history() {
    let tmp = TempDir::new().unwrap();
    let ctx = setup_context(&tmp);

    // Write termination event to history logger
    let mut history_logger = EventLogger::history_from_context(&ctx);
    let terminate_event = Event::new("loop.terminate", "completion promise detected");
    let record = EventRecord::new(5, "loop", &terminate_event, None, None);
    history_logger.log(&record).unwrap();

    // History file should contain it
    let history_path = tmp
        .path()
        .join(".ralph/events-history-20260602-120000.jsonl");
    let history = EventHistory::new(&history_path);
    let records = history.read_all().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].topic, "loop.terminate");
    assert_eq!(records[0].iteration, 5);

    // Trusted file should NOT exist
    let trusted_path = tmp.path().join(".ralph/events-20260602-120000.jsonl");
    assert!(!trusted_path.exists());
}

/// Regression: raw output contains LOOP_COMPLETE but no real emit — no fake JSONL event.
#[test]
fn test_loop_complete_text_in_output_no_fake_event() {
    let tmp = TempDir::new().unwrap();
    let ctx = setup_context(&tmp);

    // Simulate raw output that mentions LOOP_COMPLETE but has no real event
    let raw_output = r#"I have completed all tasks. LOOP_COMPLETE
The work is done and all tests pass."#;

    // Parse events from output — should NOT produce any events
    // (LOOP_COMPLETE is detected by text fallback, not as a JSONL business event)
    let parser = ralph_core::EventParser::new();
    let parsed_events = parser.parse(raw_output);

    // If the parser produced any events from this text, write them to history
    if !parsed_events.is_empty() {
        let mut history_logger = EventLogger::history_from_context(&ctx);
        for event in &parsed_events {
            let record = EventRecord::new(1, "loop", event, None, None);
            history_logger.log(&record).unwrap();
        }
    }

    // Trusted file should NOT exist
    let trusted_path = tmp.path().join(".ralph/events-20260602-120000.jsonl");
    assert!(
        !trusted_path.exists(),
        "LOOP_COMPLETE text should not create fake events in trusted file"
    );
}
