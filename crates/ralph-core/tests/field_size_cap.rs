//! Plan 2026-08-12-001 fix-plan U9: per-field byte cap on sidecar
//! JSONL writers. The contract is that no string or JSON field
//! in a single row may exceed `MAX_SIDECAR_FIELD_BYTES` (8 KiB)
//! after serialization; oversized fields are truncated at the
//! writer boundary and one `tracing::warn!` is emitted per
//! offending field. Operators with pathological upstream inputs
//! (50 MiB `source_ref`, runaway `fields` blobs) get a bounded
//! on-disk row instead of a session-eating JSONL line.

use ralph_core::diagnostics::{
    FeedbackEntry, FeedbackLogger, FeedbackPhase, MAX_SIDECAR_FIELD_BYTES, RuntimeTraceEntry,
    RuntimeTraceLogger, RuntimeTracePhase,
};
use std::fs;
use std::sync::{Arc, Mutex};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::Layer;

/// Capture `tracing::warn!` events from the diagnostics target.
#[derive(Default, Clone)]
struct WarnCapture {
    events: Arc<Mutex<Vec<String>>>,
}

impl<S: tracing::Subscriber + for<'a> tracing_subscriber::registry::LookupSpan<'a>> Layer<S>
    for WarnCapture
{
    fn on_event(
        &self,
        event: &tracing::Event<'_>,
        _ctx: tracing_subscriber::layer::Context<'_, S>,
    ) {
        if event.metadata().target() == "ralph_core::diagnostics"
            && *event.metadata().level() == tracing::Level::WARN
        {
            struct Visitor(Vec<String>);
            impl tracing::field::Visit for Visitor {
                fn record_debug(&mut self, field: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                    self.0.push(format!("{}={:?}", field.name(), value));
                }
                fn record_str(&mut self, field: &tracing::field::Field, value: &str) {
                    self.0.push(format!("{}={}", field.name(), value));
                }
            }
            let mut visitor = Visitor(Vec::new());
            event.record(&mut visitor);
            self.events.lock().unwrap().push(visitor.0.join(" "));
        }
    }
}

#[test]
fn cap_string_field_truncates_oversized_input() {
    // The cap_string_field helper is pub(crate); we exercise it
    // indirectly through the writer path below. This test
    // asserts the constant itself is set to the contract value.
    assert_eq!(MAX_SIDECAR_FIELD_BYTES, 8 * 1024);
}

#[test]
fn feedback_logger_caps_oversized_source_ref() {
    let capture = WarnCapture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let tmp = tempfile::TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    let mut logger = FeedbackLogger::new(&session).expect("FeedbackLogger::new");

    let huge = "x".repeat(50 * 1024 * 1024);
    let entry = FeedbackEntry::new(0, "id-1", "retry-1", FeedbackPhase::Action)
        .with_source_ref(huge);
    logger.append(entry);

    let path = session.join("feedback.jsonl");
    let body = fs::read_to_string(&path).expect("read feedback.jsonl");
    assert!(
        body.len() <= MAX_SIDECAR_FIELD_BYTES + 256,
        "feedback.jsonl row must fit under ~{} bytes (8 KiB field cap + ~256 bytes envelope), got {} bytes",
        MAX_SIDECAR_FIELD_BYTES + 256,
        body.len()
    );
    // The truncated marker must be present.
    assert!(body.contains("...[truncated]"));

    // At least one warn event must have fired for the offending
    // `feedback.source_ref` field.
    let events = capture.events.lock().unwrap().clone();
    assert!(
        events.iter().any(|e| e.contains("feedback.source_ref")),
        "expected at least one warn event naming feedback.source_ref, got {:?}",
        events
    );
}

#[test]
fn runtime_trace_logger_caps_oversized_source_ref() {
    let capture = WarnCapture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let tmp = tempfile::TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    let mut logger = RuntimeTraceLogger::new(&session).expect("RuntimeTraceLogger::new");

    let huge = "y".repeat(50 * 1024 * 1024);
    let entry = RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Activation).with_source_ref(huge);
    logger.append(entry);

    let path = session.join("runtime-trace.jsonl");
    let body = fs::read_to_string(&path).expect("read runtime-trace.jsonl");
    assert!(
        body.len() <= MAX_SIDECAR_FIELD_BYTES + 256,
        "runtime-trace.jsonl row must fit under ~{} bytes, got {} bytes",
        MAX_SIDECAR_FIELD_BYTES + 256,
        body.len()
    );
    assert!(body.contains("...[truncated]"));

    let events = capture.events.lock().unwrap().clone();
    assert!(
        events.iter().any(|e| e.contains("runtime_trace.source_ref")),
        "expected at least one warn event naming runtime_trace.source_ref, got {:?}",
        events
    );
}

#[test]
fn small_field_passes_through_writer_unchanged() {
    let capture = WarnCapture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let tmp = tempfile::TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    let mut logger = FeedbackLogger::new(&session).expect("FeedbackLogger::new");

    let entry = FeedbackEntry::new(0, "id-1", "retry-1", FeedbackPhase::Action)
        .with_source_ref("recovery.jsonl:42");
    logger.append(entry);

    let path = session.join("feedback.jsonl");
    let body = fs::read_to_string(&path).expect("read feedback.jsonl");
    assert!(body.contains("recovery.jsonl:42"));
    assert!(!body.contains("...[truncated]"));

    // No warns for small inputs.
    let events = capture.events.lock().unwrap().clone();
    let truncation_warns: Vec<_> = events
        .iter()
        .filter(|e| e.contains("truncated"))
        .collect();
    assert!(
        truncation_warns.is_empty(),
        "no truncation warns expected for small input, got {:?}",
        truncation_warns
    );
}

#[test]
fn unicode_source_ref_is_truncated_without_panicking() {
    let tmp = tempfile::TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    let mut logger = RuntimeTraceLogger::new(&session).expect("RuntimeTraceLogger::new");

    let entry = RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Activation)
        .with_source_ref("中".repeat(10_000));
    logger.append(entry);

    let body = fs::read_to_string(session.join("runtime-trace.jsonl"))
        .expect("read runtime-trace.jsonl");
    assert!(body.len() <= MAX_SIDECAR_FIELD_BYTES + 512);
    assert!(body.contains("...[truncated]"));
    assert!(serde_json::from_str::<serde_json::Value>(body.trim()).is_ok());
}

#[test]
fn nested_json_field_is_bounded() {
    let tmp = tempfile::TempDir::new().expect("TempDir");
    let session = tmp.path().join("session");
    fs::create_dir_all(&session).expect("create session dir");
    let mut logger = RuntimeTraceLogger::new(&session).expect("RuntimeTraceLogger::new");

    let entry = RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Batch).with_fields(
        serde_json::json!({
            "nested": { "payload": "x".repeat(50 * 1024) },
            "items": ["y".repeat(50 * 1024), "z".repeat(50 * 1024)]
        }),
    );
    logger.append(entry);

    let body = fs::read_to_string(session.join("runtime-trace.jsonl"))
        .expect("read runtime-trace.jsonl");
    assert!(body.len() <= MAX_SIDECAR_FIELD_BYTES + 512);
}
