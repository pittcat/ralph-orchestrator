//! Characterization tests for U4 Plan A: Wave must traverse the same
//! isolated publish scope gate as regular events.
//!
//! These tests exercise the real `process_events_from_jsonl_with_waves()`
//! path. They are intentionally failing (RED) at the time of authorship
//! because the existing wave partition bypasses `current_isolated_hat`
//! scope enforcement (see `process_parse_result` at `event_loop/mod.rs:4042`
//! for the regular-event check that waves currently skip).
//!
//! Per KTD-U4-1 / KTD-U4-2:
//!   - Wave partition can stay, but post-partition the same isolated publish
//!     scope check must apply.
//!   - A complete Wave is one logical business emission; isolated mode
//!     only allows one distinct `wave_id` per read batch.

use super::*;

/// Helper: write a single wave-dispatch event line to a JSONL file.
///
/// `topic` is a trigger of a concurrent hat so the partition path treats
/// the event as a wave dispatch. `hat` provides origin-guard provenance.
fn write_wave_event_to_jsonl(
    path: &std::path::Path,
    topic: &str,
    payload: &str,
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
/// topology that supports `review.file` wave dispatch.
///
/// `current_isolated_hat` is left `None`; each test sets it explicitly to
/// model the runtime state set in `process_output()` (mod.rs:3508).
fn make_isolated_loop(events_path: &std::path::Path) -> EventLoop {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "review.file"
    business_topics:
      - "review.done"
  execution_mode: isolated
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.start"]
    publishes: ["review.file"]
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
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

/// Supervisor-enabled isolated loops admit multiple independent wave groups
/// in one read batch. The supervisor worker cap, rather than the legacy
/// isolated one-business-event rule, provides backpressure for this path.
fn make_supervisor_isolated_loop(events_path: &std::path::Path) -> EventLoop {
    let mut event_loop = make_isolated_loop(events_path);
    event_loop.config.event_loop.supervisor.enabled = true;
    event_loop
}

/// Collect all violation topic names published to the bus after processing.
fn collect_violation_topics(event_loop: &EventLoop) -> Vec<String> {
    let empty = Vec::new();
    event_loop
        .bus
        .hat_ids()
        .flat_map(|id| {
            event_loop
                .bus
                .peek_pending(id)
                .unwrap_or(&empty)
                .iter()
                .filter(|e| e.topic.as_str().ends_with(".scope_violation"))
                .map(|e| e.topic.to_string())
                .collect::<Vec<_>>()
        })
        .collect()
}

/// A1-1: an isolated hat publishing a topic outside its `publishes` list
/// must not let a Wave targeting that topic through.
///
/// Today this assertion is RED: `process_events_from_jsonl_with_waves`
/// does not consult `current_isolated_hat` after partition, so the wave
/// is incorrectly accepted.
#[test]
fn test_wave_isolated_out_of_scope_rejected() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_loop(&events_path);

    // Simulate the "builder" hat running in isolated mode. Its
    // `publishes` does NOT include `review.file`, so a review.file wave
    // must be rejected as an out-of-scope emission.
    let builder = HatId::new("builder");
    event_loop.state.current_isolated_hat = Some(builder.clone());

    // One wave event, dispatched by coordinator, targeting the reviewer.
    write_wave_event_to_jsonl(
        &events_path,
        "review.file",
        "src/main.rs",
        "coordinator",
        "w-1",
        0,
        1,
    );

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    assert_eq!(
        result.wave_events.len(),
        0,
        "out-of-scope wave event must be rejected; current_isolated_hat={:?} does not publish '{}'",
        event_loop.state.current_isolated_hat,
        "review.file"
    );
    assert!(
        !collect_violation_topics(&event_loop).is_empty(),
        "scope violation event should be published to the bus when a wave is rejected"
    );
}

/// Supervisor-enabled isolated mode must retain every independently scoped
/// wave in the same input batch so the dispatcher can execute them together.
#[test]
fn test_supervisor_isolated_allows_multiple_independent_waves() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_supervisor_isolated_loop(&events_path);
    event_loop.state.current_isolated_hat = Some(HatId::new("coordinator"));

    write_wave_event_to_jsonl(
        &events_path,
        "review.file",
        "file-A.rs",
        "coordinator",
        "w-Alpha",
        0,
        1,
    );
    write_wave_event_to_jsonl(
        &events_path,
        "review.file",
        "file-B.rs",
        "coordinator",
        "w-Beta",
        0,
        1,
    );

    let processed = event_loop
        .process_events_from_jsonl_with_waves()
        .unwrap();
    let mut wave_ids: Vec<_> = processed
        .wave_events
        .iter()
        .filter_map(|event| event.wave_id.as_deref())
        .collect();
    wave_ids.sort_unstable();

    assert_eq!(wave_ids, vec!["w-Alpha", "w-Beta"]);
    assert!(collect_violation_topics(&event_loop).is_empty());
}

/// A1-2: a legal Wave carrying multiple events must be preserved as a
/// single unit; per-event single-business-emission rules must not chop it.
///
/// We use 7 events with the same `wave_id`. The post-partition scope
/// check must validate the wave as one business emission, not enforce
/// the regular-event "first business event accepted" rule.
#[test]
fn test_wave_isolated_legal_seven_event_wave_preserved() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_loop(&events_path);

    // coordinator publishes review.file, so the wave topic IS in its
    // publishes — the wave should pass the isolated scope gate.
    event_loop.state.current_isolated_hat = Some(HatId::new("coordinator"));

    for i in 0..7 {
        write_wave_event_to_jsonl(
            &events_path,
            "review.file",
            &format!("src/file-{}.rs", i),
            "coordinator",
            "w-legal",
            i,
            7,
        );
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    assert_eq!(
        result.wave_events.len(),
        7,
        "all 7 events of a legal wave must be preserved as a single business emission"
    );
    let distinct_wave_ids: std::collections::HashSet<&str> = result
        .wave_events
        .iter()
        .map(|e| e.wave_id.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        distinct_wave_ids.len(),
        1,
        "preserved wave must carry exactly one distinct wave_id, got {:?}",
        distinct_wave_ids
    );
}

/// A1-3: a single read batch containing two distinct `wave_id`s must
/// accept only the first; the second is an isolated multi-business-
/// emission violation and is rejected.
///
/// This pins KTD-U4-2 "isolated 模式只允许一个 distinct `wave_id`".
#[test]
fn test_wave_isolated_second_wave_id_rejected() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_loop(&events_path);

    event_loop.state.current_isolated_hat = Some(HatId::new("coordinator"));

    // First wave: 3 events
    for i in 0..3 {
        write_wave_event_to_jsonl(
            &events_path,
            "review.file",
            &format!("first-{}", i),
            "coordinator",
            "w-A",
            i,
            3,
        );
    }
    // Second wave: 3 events (different wave_id)
    for i in 0..3 {
        write_wave_event_to_jsonl(
            &events_path,
            "review.file",
            &format!("second-{}", i),
            "coordinator",
            "w-B",
            i,
            3,
        );
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    let accepted_wave_ids: std::collections::HashSet<String> = result
        .wave_events
        .iter()
        .filter_map(|e| e.wave_id.clone())
        .collect();

    assert_eq!(
        accepted_wave_ids.len(),
        1,
        "isolated activation must accept only one distinct wave_id, got {:?}",
        accepted_wave_ids
    );
    assert!(
        accepted_wave_ids.contains("w-A"),
        "the first wave_id observed in the batch should be the one accepted"
    );
    assert!(
        !accepted_wave_ids.contains("w-B"),
        "the second distinct wave_id must be rejected as a multi-business-emission"
    );
}

/// A1-4: a multi-event Wave counts as ONE business emission, not N.
/// This is a characterization test for the counting rule, not the
/// rejection rule. We simply assert that all 5 events are accepted and
/// the implementation does not split them into multiple "emissions"
/// that would each need their own scope decision.
#[test]
fn test_wave_isolated_multi_event_wave_is_single_emission() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_loop(&events_path);

    event_loop.state.current_isolated_hat = Some(HatId::new("coordinator"));

    let event_count = 5;
    for i in 0..event_count {
        write_wave_event_to_jsonl(
            &events_path,
            "review.file",
            &format!("payload-{}", i),
            "coordinator",
            "w-multi",
            i,
            event_count,
        );
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    // All events come through (single business emission, not N).
    assert_eq!(
        result.wave_events.len(),
        event_count as usize,
        "all events of a single-wave batch must be retained, not split"
    );
    // And critically, they share a single wave_id — the post-partition
    // path must not re-classify them into multiple distinct waves.
    let distinct_wave_ids: std::collections::HashSet<&str> = result
        .wave_events
        .iter()
        .map(|e| e.wave_id.as_deref().unwrap_or(""))
        .collect();
    assert_eq!(
        distinct_wave_ids.len(),
        1,
        "multi-event wave must remain a single wave_id group, got {:?}",
        distinct_wave_ids
    );
}

/// A1-5: regression guard — when no isolated hat is set, wave events
/// must continue to flow through unchanged. This test pins the
/// non-isolated behavior so that the new scope check is a strict
/// refinement, not a behavioral change for non-isolated runs.
#[test]
fn test_wave_non_isolated_behavior_unchanged() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_loop(&events_path);

    // Explicitly clear any default isolated state.
    event_loop.state.current_isolated_hat = None;
    assert!(
        event_loop.state.current_isolated_hat.is_none(),
        "test precondition: no isolated hat should be set"
    );

    for i in 0..3 {
        write_wave_event_to_jsonl(
            &events_path,
            "review.file",
            &format!("non-isolated-{}", i),
            "coordinator",
            "w-non-iso",
            i,
            3,
        );
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    assert_eq!(
        result.wave_events.len(),
        3,
        "non-isolated runs must keep all wave events flowing through"
    );
    assert!(
        collect_violation_topics(&event_loop).is_empty(),
        "no scope violation should fire in non-isolated mode"
    );
}

// ---------------------------------------------------------------------------
// B2 / KTD-U4-4: isolated wave rejection must record a recovery envelope
// ---------------------------------------------------------------------------

/// Build an isolated-mode event loop with file-backed diagnostics
/// so that `record_recovery_envelope` writes to `recovery.jsonl`.
fn make_isolated_loop_with_diagnostics(
    events_path: &std::path::Path,
    diagnostics_root: &std::path::Path,
) -> (EventLoop, std::path::PathBuf) {
    use crate::diagnostics::DiagnosticsCollector;

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "review.file"
    business_topics:
      - "review.done"
  execution_mode: isolated
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.start"]
    publishes: ["review.file"]
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 3
    instructions: "Review."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let diagnostics =
        DiagnosticsCollector::with_enabled(diagnostics_root, true).expect("diagnostics enabled");
    let session_dir = diagnostics.session_dir().unwrap().to_path_buf();
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("B2-test");
    event_loop.event_reader = crate::event_reader::EventReader::new(events_path);
    (event_loop, session_dir)
}

/// Read all `RecoveryJournalEntry` records from the session's
/// `recovery.jsonl` file.
fn read_recovery_journal(
    session_dir: &std::path::Path,
) -> Vec<crate::diagnosis::RecoveryJournalEntry> {
    use std::io::Read as _;

    let recovery_path = session_dir.join("recovery.jsonl");
    let mut content = String::new();
    std::fs::File::open(&recovery_path)
        .unwrap_or_else(|e| panic!("open {}: {e}", recovery_path.display()))
        .read_to_string(&mut content)
        .expect("read recovery.jsonl");
    content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse recovery entry"))
        .collect()
}

/// B2-1: an isolated wave rejected for scope violation MUST produce a
/// recovery envelope in `recovery.jsonl`. The envelope's `retry_key`
/// must use the wave-scoped `wave_dispatcher:<id>:<reason>` format
/// (from B1), and `outcome` must be `NotRetriable`.
#[test]
fn test_wave_isolated_scope_violation_records_recovery_envelope() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let diagnostics_root = temp_dir.path().to_path_buf();
    let (mut event_loop, session_dir) =
        make_isolated_loop_with_diagnostics(&events_path, &diagnostics_root);

    // builder is not in coordinator's publishes → scope violation
    event_loop.state.current_isolated_hat = Some(HatId::new("builder"));

    write_wave_event_to_jsonl(
        &events_path,
        "review.file",
        "src/main.rs",
        "coordinator",
        "w-001",
        0,
        1,
    );

    let _ = event_loop.process_events_from_jsonl_with_waves().unwrap();

    let entries = read_recovery_journal(&session_dir);
    assert_eq!(
        entries.len(),
        1,
        "exactly one recovery envelope expected, got: {:?}",
        entries
            .iter()
            .map(|e| &e.envelope.reason_code)
            .collect::<Vec<_>>()
    );
    let env = &entries[0].envelope;
    assert_eq!(
        env.source,
        crate::diagnosis::DiagnosisSource::WaveDispatcher
    );
    assert_eq!(env.reason_code, "wave_isolated_scope_violation");
    assert_eq!(
        env.outcome,
        crate::diagnosis::DiagnosisOutcome::NotRetriable
    );
    assert!(!env.safe_target);
    assert_eq!(env.source_hat.as_deref(), Some("builder"));
    assert!(
        env.retry_key.starts_with("wave_dispatcher:"),
        "retry key must use wave namespace, got: {}",
        env.retry_key
    );
    assert!(
        env.retry_key.contains("w_001"),
        "retry key must contain normalized wave_id, got: {}",
        env.retry_key
    );
}

/// B2-2: two distinct waves rejected for the same reason MUST produce
/// two separate recovery envelopes with different `retry_key`s. This
/// is the end-to-end validation of B1 + B2 working together.
#[test]
fn test_wave_isolated_two_different_waves_two_envelopes() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let diagnostics_root = temp_dir.path().to_path_buf();
    let (mut event_loop, session_dir) =
        make_isolated_loop_with_diagnostics(&events_path, &diagnostics_root);

    event_loop.state.current_isolated_hat = Some(HatId::new("builder"));

    // Two waves, both out-of-scope for the current isolated hat.
    write_wave_event_to_jsonl(
        &events_path,
        "review.file",
        "file-A.rs",
        "coordinator",
        "w-Alpha",
        0,
        1,
    );
    write_wave_event_to_jsonl(
        &events_path,
        "review.file",
        "file-B.rs",
        "coordinator",
        "w-Beta",
        0,
        1,
    );

    let _ = event_loop.process_events_from_jsonl_with_waves().unwrap();

    let entries = read_recovery_journal(&session_dir);
    // The two waves are in different wave_id groups. The scope check
    // runs on the first accepted wave (which is out of scope), and
    // also rejects subsequent waves via IsolatedMultipleBusinessEmissions.
    // We expect at least 2 envelopes — one per wave.
    assert!(
        entries.len() >= 2,
        "at least two envelopes expected for two distinct waves, got: {:?}",
        entries
            .iter()
            .map(|e| (&e.envelope.reason_code, &e.envelope.retry_key))
            .collect::<Vec<_>>()
    );
    let retry_keys: std::collections::HashSet<String> = entries
        .iter()
        .map(|e| e.envelope.retry_key.clone())
        .collect();
    assert_eq!(
        retry_keys.len(),
        entries.len(),
        "each wave must have a distinct retry key"
    );
}

/// B2-3: when no isolated hat is set, no recovery envelope is recorded.
/// This is the regression guard from A1-5, verified at the envelope level.
#[test]
fn test_wave_non_isolated_does_not_record_envelope() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let diagnostics_root = temp_dir.path().to_path_buf();
    let (mut event_loop, session_dir) =
        make_isolated_loop_with_diagnostics(&events_path, &diagnostics_root);

    event_loop.state.current_isolated_hat = None;

    write_wave_event_to_jsonl(
        &events_path,
        "review.file",
        "file.rs",
        "coordinator",
        "w-safe",
        0,
        1,
    );

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();
    assert_eq!(result.wave_events.len(), 1, "wave must be accepted");

    // recovery.jsonl should not exist (no rejections).
    let recovery_path = session_dir.join("recovery.jsonl");
    if recovery_path.exists() {
        // If it exists for some other reason, it must be empty.
        let content = std::fs::read_to_string(&recovery_path).unwrap_or_default();
        assert!(
            content.trim().is_empty(),
            "non-isolated runs must not produce recovery envelopes"
        );
    }
}
