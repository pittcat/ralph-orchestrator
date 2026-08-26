//! Plan 2026-08-12-001 U1: integration tests for the four new
//! diagnostics APIs wired into the production EventLoop path:
//! - `log_runtime_trace(Activation)` at hat selection
//! - `log_runtime_trace(AcceptedEvent)` at each accepted publish
//! - `log_feedback(Action/Validation/Final)` across the feedback lifecycle
//! - `update_input_bundle_identity` at run start
//! - `finalize_input_bundle` at run termination
//!
//! These tests verify the sidecar files are created and contain
//! the expected rows when diagnostics are enabled, and are absent
//! when they are disabled. The off/on tuple-shape equivalence
//! (D16 invariant) is also verified.

use ralph_core::diagnostics::{
    DiagnosticsCollector, DiagnosticsOptions, FeedbackEntry, RuntimeTraceEntry, RuntimeTracePhase,
};
use ralph_core::event_loop::EventLoop;
use ralph_core::{LoopContext, RalphConfig};
use serde::Deserialize;
use std::fs::File;
use std::io::{BufRead, BufReader};
use tempfile::TempDir;

fn make_enabled_collector(temp: &TempDir) -> DiagnosticsCollector {
    let opts = DiagnosticsOptions {
        full_diagnostics: false,
        runtime_diagnosis_artifacts: true,
        trace_only: false,
        session_dir: None,
        workspace_root: None,
        // DT3 (plan §17): explicit `causal_evidence = false` pins the
        // "minimal-session enabled via runtime_diagnosis_artifacts" shape
        // independently of the new field's default. Without this the
        // helper would silently switch to the causal-evidence row when
        // U01a's telemetry bridge flips `causal_evidence` to `true` by
        // default.
        causal_evidence: false,

        causal_evidence_window_capacity: None,
    };
    DiagnosticsCollector::with_options(temp.path(), &opts).expect("collector")
}

fn make_disabled_collector() -> DiagnosticsCollector {
    DiagnosticsCollector::disabled()
}

/// Find the timestamped session directory inside `.ralph/diagnostics/`.
fn find_session_dir(diagnostics_root: &std::path::Path) -> std::path::PathBuf {
    let diag_dir = diagnostics_root.join(".ralph").join("diagnostics");
    let entries: Vec<_> = std::fs::read_dir(&diag_dir)
        .expect("diagnostics dir should exist")
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1, "expected exactly one session dir");
    entries[0].path()
}

// ── RuntimeTrace tests ────────────────────────────────────────────────────────

#[test]
fn enabled_collector_produces_runtime_trace_rows() {
    let temp = TempDir::new().expect("TempDir");
    let config = RalphConfig::default();
    let diagnostics = make_enabled_collector(&temp);
    let context = LoopContext::primary(temp.path().to_path_buf());
    let mut loop_ =
        EventLoop::with_context_and_diagnostics(config, context, diagnostics).expect("eventloop");

    // Run one iteration (process_output increments iteration counter)
    loop_.process_output(&"ralph".into(), "some output", true);

    let session = find_session_dir(temp.path());
    let trace_path = session.join("runtime-trace.jsonl");

    // File must exist
    assert!(
        trace_path.is_file(),
        "runtime-trace.jsonl should exist at {}",
        trace_path.display()
    );

    // Must have at least one line
    let body = std::fs::read_to_string(&trace_path).expect("read runtime-trace");
    let lines: Vec<&str> = body.lines().collect();
    assert!(
        !lines.is_empty(),
        "runtime-trace.jsonl must have at least one row"
    );

    // At least one row must decode as RuntimeTraceEntry with phase == Activation
    let has_activation = lines.iter().any(|line| {
        if let Ok(entry) = serde_json::from_str::<RuntimeTraceEntry>(line) {
            entry.phase == RuntimeTracePhase::Activation
        } else {
            false
        }
    });
    assert!(
        has_activation,
        "expected at least one RuntimeTraceEntry with phase=Activation in {:?}",
        lines
    );
}

#[test]
fn enabled_collector_produces_feedback_rows() {
    let temp = TempDir::new().expect("TempDir");
    let config = RalphConfig::default();
    let diagnostics = make_enabled_collector(&temp);
    let context = LoopContext::primary(temp.path().to_path_buf());
    let mut loop_ =
        EventLoop::with_context_and_diagnostics(config, context, diagnostics).expect("eventloop");

    // Write a recovery envelope directly into the loop to exercise log_feedback
    use ralph_core::diagnosis::{DiagnosisSeverity, DiagnosisSource, RecoveryDiagnosisEnvelope};
    let envelope = RecoveryDiagnosisEnvelope::builder()
        .source(DiagnosisSource::StallRecovery)
        .severity(DiagnosisSeverity::Error)
        .reason_code("stall")
        .message("test stall".to_string())
        .target_hat("ralph".to_string())
        .build();
    loop_.record_recovery_envelope(&envelope, Vec::new());

    let session = find_session_dir(temp.path());
    let feedback_path = session.join("feedback.jsonl");

    assert!(
        feedback_path.is_file(),
        "feedback.jsonl should exist at {}",
        feedback_path.display()
    );

    let body = std::fs::read_to_string(&feedback_path).expect("read feedback");
    let lines: Vec<&str> = body.lines().collect();
    assert!(
        !lines.is_empty(),
        "feedback.jsonl must have at least one row"
    );

    // At least one row must decode as FeedbackEntry
    let has_feedback = lines
        .iter()
        .any(|line| serde_json::from_str::<FeedbackEntry>(line).is_ok());
    assert!(
        has_feedback,
        "expected at least one FeedbackEntry in {:?}",
        lines
    );
}

#[test]
fn disabled_collector_produces_no_sidecars() {
    let temp = TempDir::new().expect("TempDir");
    let config = RalphConfig::default();
    let diagnostics = make_disabled_collector();
    let context = LoopContext::primary(temp.path().to_path_buf());
    let _loop_ =
        EventLoop::with_context_and_diagnostics(config, context, diagnostics).expect("eventloop");

    // With a disabled collector, no .ralph/diagnostics/ directory should be created
    let diag_dir = temp.path().join(".ralph").join("diagnostics");
    assert!(
        !diag_dir.exists(),
        "disabled collector must not create diagnostics directory"
    );

    // Nor any of the individual sidecar files
    assert!(!temp.path().join("runtime-trace.jsonl").exists());
    assert!(!temp.path().join("feedback.jsonl").exists());
    assert!(!temp.path().join("diagnosis-input.json").exists());
}

// ── Off/On equivalence (D16) ──────────────────────────────────────────────────

/// Exact business-event tuple shape for off/on comparison. Diagnostics
/// sidecars are intentionally excluded; accepted event routing and payload
/// semantics must remain byte/structure equivalent.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct EventTupleShape {
    #[serde(default)]
    iteration: Option<u64>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    target: Option<String>,
    topic: String,
    payload: serde_json::Value,
    error: Option<String>,
}

impl EventTupleShape {
    fn from_line(line: &str) -> Option<Self> {
        #[derive(Deserialize)]
        struct RawEvent {
            topic: String,
            #[serde(default)]
            payload: serde_json::Value,
            #[serde(default)]
            iteration: Option<u64>,
            #[serde(default)]
            source: Option<String>,
            #[serde(default)]
            target: Option<String>,
            #[serde(default)]
            error: Option<String>,
        }
        let raw: RawEvent = serde_json::from_str(line).ok()?;
        Some(EventTupleShape {
            iteration: raw.iteration,
            source: raw.source,
            target: raw.target,
            topic: raw.topic,
            payload: raw.payload,
            error: raw.error,
        })
    }
}

/// Verify that enabling/disabling diagnostics does not change the
/// bytes written to events.jsonl for an identical business event (D16 invariant).
#[test]
fn off_on_business_event_equivalence() {
    fn run_once(collector: &DiagnosticsCollector, temp: &TempDir) -> Vec<EventTupleShape> {
        let config = RalphConfig::default();
        let context = LoopContext::primary(temp.path().to_path_buf());
        let mut loop_ = EventLoop::with_context_and_diagnostics(config, context, collector.clone())
            .expect("eventloop");

        // One iteration
        loop_.process_output(
            &"ralph".into(),
            r#"{"topic":"work.done","payload":{"task_id":"t1"}}"#,
            true,
        );

        // Read events.jsonl and extract the shapes
        let events_path = temp.path().join(".ralph").join("events.jsonl");
        if !events_path.is_file() {
            return Vec::new();
        }
        let file = File::open(&events_path).expect("open events");
        let reader = BufReader::new(file);
        reader
            .lines()
            .map_while(Result::ok)
            .filter_map(|line| EventTupleShape::from_line(&line))
            .collect()
    }

    let temp_on = TempDir::new().expect("TempDir");
    let collector_on = make_enabled_collector(&temp_on);
    let shapes_on = run_once(&collector_on, &temp_on);

    let temp_off = TempDir::new().expect("TempDir");
    let collector_off = make_disabled_collector();
    let shapes_off = run_once(&collector_off, &temp_off);

    assert_eq!(
        shapes_on, shapes_off,
        "off/on business event tuple shapes must be byte-identical: \
         enable shapes={:?}, disable shapes={:?}",
        shapes_on, shapes_off
    );
}
