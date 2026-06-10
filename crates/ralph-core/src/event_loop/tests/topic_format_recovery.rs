//! Tests for the topic-format rejection recovery signal.
//!
//! P1 finding #8 (code review 2026-06-10): `check_topic_format` rejected
//! unknown topics with `Block(InvalidTopicFormat)` but the actual
//! rejection site only published an `event.topic_format.rejected`
//! diagnostic — never wrote the `recovery.jsonl` envelope that plan
//! R10 ("non-retryable, only write recovery signal") commits to.
//!
//! These tests pin the new behavior: when an unknown topic arrives, a
//! `RecoveryJournalEntry` with `source = TopicFormat` MUST land in
//! `recovery.jsonl`. We assert against the real
//! `process_events_from_jsonl` path (not a unit test of the helper) so
//! the wiring through the existing `record_recovery_envelope` plumbing
//! is covered end-to-end.

use super::common::*;
use super::*;

#[test]
fn test_unknown_topic_writes_recovery_envelope() {
    // Arrange: an event policy + a single hat that publishes `work.done`.
    // Any topic outside this whitelist (system topics excepted) must be
    // rejected and produce a recovery journal entry.
    use crate::diagnosis::{DiagnosisOutcome, DiagnosisSeverity, DiagnosisSource};
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();

    let yaml = r#"
event_loop:
  max_iterations: 10
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    completion_after_terminal:
      duplicate_terminal: warn
      business_after_completion: warn
hats:
  executor:
    name: executor
    triggers: ["work.start"]
    publishes: ["work.done"]
    terminal_events: ["work.done"]
    instructions: "Consume work."
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Act: emit an unknown topic ("bogus.unknown_topic") through the
    // JSONL pipeline. The pipeline should reject it at the topic
    // format check and write a recovery entry.
    write_event_to_jsonl(&events_path, "bogus.unknown_topic", r#"{"x":1}"#);
    let _ = event_loop
        .process_events_from_jsonl()
        .expect("process_events_from_jsonl should not error on unknown topics");

    // Assert: recovery.jsonl has exactly one entry, with the right
    // shape. We look inside `.ralph/diagnostics/<session>/recovery.jsonl`
    // mirroring how `ralph diagnose --session latest` resolves it.
    let mut session_dirs: Vec<_> = std::fs::read_dir(diagnostics_root.join(".ralph/diagnostics"))
        .expect("read diagnostics dir")
        .filter_map(Result::ok)
        .collect();
    session_dirs.sort_by_key(|entry| entry.path());
    let session_path = session_dirs
        .last()
        .expect("at least one diagnostics session")
        .path();
    let recovery_path = session_path.join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
    let entries: Vec<crate::diagnosis::RecoveryJournalEntry> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse recovery entry"))
        .collect();
    assert_eq!(
        entries.len(),
        1,
        "expected exactly one recovery entry, got: {:?}",
        entries
            .iter()
            .map(|e| &e.envelope.reason_code)
            .collect::<Vec<_>>()
    );
    let env = &entries[0].envelope;
    assert_eq!(env.source, DiagnosisSource::TopicFormat);
    assert_eq!(env.reason_code, "invalid_topic_format");
    assert_eq!(env.topic.as_deref(), Some("bogus.unknown_topic"));
    assert_eq!(env.severity, DiagnosisSeverity::Warning);
    assert_eq!(env.outcome, DiagnosisOutcome::NotRetriable);
    assert!(
        !env.safe_target,
        "topic-format rejection has no safe retry target"
    );
    // The evidence ref should carry the offending topic so `ralph
    // diagnose` can render it without re-parsing the message string.
    assert!(
        env.evidence
            .iter()
            .any(|e| e.kind == crate::diagnosis::EvidenceKind::Topic
                && e.ref_path == "bogus.unknown_topic"),
        "evidence must reference the rejected topic"
    );
}

#[test]
fn test_unknown_topic_does_not_publish_task_resume() {
    // R10: the rejection is non-retryable. The `task.resume` channel
    // is reserved for retriable failures — topic-format rejections
    // must NOT publish a `task.resume`. The legacy
    // `event.topic_format.rejected` diagnostic still fires so older
    // observers (TUI, tests) see something; the journal entry is the
    // new layer on top.
    use ralph_proto::Event;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();

    let yaml = r#"
event_loop:
  max_iterations: 10
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
hats:
  executor:
    name: executor
    triggers: ["work.start"]
    publishes: ["work.done"]
    terminal_events: ["work.done"]
    instructions: "Consume work."
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Count both diagnostic and task.resume emissions.
    let diagnostic_count = Arc::new(AtomicUsize::new(0));
    let resume_count = Arc::new(AtomicUsize::new(0));
    let d = diagnostic_count.clone();
    let r = resume_count.clone();
    event_loop
        .bus
        .add_observer(move |event: &Event| match event.topic.as_str() {
            "event.topic_format.rejected" => {
                d.fetch_add(1, Ordering::SeqCst);
            }
            "task.resume" => {
                r.fetch_add(1, Ordering::SeqCst);
            }
            _ => {}
        });

    write_event_to_jsonl(&events_path, "bogus.unknown_topic", r#"{}"#);
    let _ = event_loop.process_events_from_jsonl();

    assert!(
        diagnostic_count.load(Ordering::SeqCst) >= 1,
        "legacy event.topic_format.rejected diagnostic must still fire for back-compat"
    );
    assert_eq!(
        resume_count.load(Ordering::SeqCst),
        0,
        "R10: topic-format rejection is non-retryable, must not publish task.resume"
    );
}

#[test]
fn test_whitelisted_topic_does_not_write_recovery_envelope() {
    // Negative case: a topic inside the whitelist must not produce a
    // journal entry. We check that recovery.jsonl either does not
    // exist, or contains zero TopicFormat entries (other source
    // entries from later iterations are allowed).
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();

    let yaml = r#"
event_loop:
  max_iterations: 10
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
hats:
  executor:
    name: executor
    triggers: ["work.start"]
    publishes: ["work.done"]
    terminal_events: ["work.done"]
    instructions: "Consume work."
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // `work.done` is in the whitelist (publishes of `executor`).
    write_event_to_jsonl(&events_path, "work.done", r#"{}"#);
    let _ = event_loop.process_events_from_jsonl();

    let mut session_dirs: Vec<_> = std::fs::read_dir(diagnostics_root.join(".ralph/diagnostics"))
        .expect("read diagnostics dir")
        .filter_map(Result::ok)
        .collect();
    session_dirs.sort_by_key(|entry| entry.path());
    let session_path = session_dirs.last().unwrap().path();
    let recovery_path = session_path.join("recovery.jsonl");
    // The file may or may not exist depending on whether *any*
    // rejection fired; what matters is that no TopicFormat entry is
    // recorded for the whitelisted event.
    if recovery_path.exists() {
        let content = std::fs::read_to_string(&recovery_path).unwrap();
        let entries: Vec<crate::diagnosis::RecoveryJournalEntry> = content
            .lines()
            .filter(|line| !line.trim().is_empty())
            .map(|line| serde_json::from_str(line).expect("parse recovery entry"))
            .collect();
        for entry in &entries {
            assert_ne!(
                entry.envelope.source,
                crate::diagnosis::DiagnosisSource::TopicFormat,
                "whitelisted topic must not produce a TopicFormat envelope (entry: {:?})",
                entry.envelope
            );
        }
    }
}

#[test]
fn test_unknown_topic_recovery_envelope_has_stable_retry_key() {
    // Stable `retry_key` is the contract that lets `ralph diagnose`
    // aggregate repeated rejections across iterations into the same
    // bucket. Pin it here so a future builder tweak can't silently
    // break the aggregation.
    use tempfile::TempDir;

    let temp = TempDir::new().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();

    let yaml = r#"
event_loop:
  max_iterations: 10
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
hats:
  executor:
    name: executor
    triggers: ["work.start"]
    publishes: ["work.done"]
    terminal_events: ["work.done"]
    instructions: ""
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("Test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    write_event_to_jsonl(&events_path, "bogus.alpha", r#"{}"#);
    let _ = event_loop.process_events_from_jsonl();

    let mut session_dirs: Vec<_> = std::fs::read_dir(diagnostics_root.join(".ralph/diagnostics"))
        .expect("read diagnostics dir")
        .filter_map(Result::ok)
        .collect();
    session_dirs.sort_by_key(|entry| entry.path());
    let session_path = session_dirs.last().unwrap().path();
    let recovery_path = session_path.join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path).unwrap();
    let entries: Vec<crate::diagnosis::RecoveryJournalEntry> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse recovery entry"))
        .collect();
    let env = &entries[0].envelope;
    // Expected key shape: source|topic|reason_code in snake_case.
    // We assert the *prefix* and the reason_code substring so the
    // exact format can evolve without breaking this test, as long as
    // the source is preserved.
    assert!(
        env.retry_key.contains("topic_format"),
        "retry_key must encode the source for downstream aggregation: {}",
        env.retry_key
    );
    assert!(
        env.retry_key.contains("invalid_topic_format"),
        "retry_key must encode the reason_code: {}",
        env.retry_key
    );
}
