//! Plan 2026-08-26-1104 Unit 6 acceptance tests: bounded frozen
//! evidence window (`evidence-window.jsonl`).
//!
//! Locks the contract that the manifest v2 reader (U7) and the
//! deterministic attribution engine (U8) both consume:
//!
//! - `EvidenceWindowWriter` is created under the same activation
//!   rule as the runtime-trace logger (`full_diagnostics ||
//!   causal_evidence || runtime_diagnosis_artifacts`).
//! - `DiagnosticsCollector::with_options` returns a collector whose
//!   session dir contains the frozen-window sidecar after one of
//!   the five anomaly triggers flushes it.
//! - S6.1: a normal LOOP_COMPLETE termination never flushes the
//!   sidecar — the file must not exist on disk after the loop
//!   closes (anomaly triggers are the only flush path).
//! - S6.2: a single anomaly trigger emits a file whose first line
//!   is the anomaly descriptor (trigger_kind/ts/iteration) and
//!   whose remaining lines are the buffered candidate lines plus
//!   the post-trigger lines supplied at flush time.
//! - S6.3: the buffer is bounded — pushing more than `capacity`
//!   candidate lines drops the oldest, so the frozen file holds at
//!   most `capacity` pre-trigger lines.
//! - S6.4: oversized string/JSON fields in any candidate or
//!   post-trigger row are capped at the same 8 KiB threshold that
//!   the runtime-trace logger applies (`MAX_SIDECAR_FIELD_BYTES`)
//!   and a `tracing::warn!` is emitted naming the field. The
//!   file is bounded — no row contains a full prompt or a full
//!   model output.
//!
//! The tests deliberately drive the production
//! `DiagnosticsCollector` API rather than a stand-in stub so the
//! activation matrix contract (which collector modes spawn the
//! window writer) is locked down alongside the row format.

use std::fs;
use std::sync::{Arc, Mutex};

use ralph_core::diagnostics::{
    AnomalyDescriptor, DiagnosticsCollector, DiagnosticsOptions, EVIDENCE_WINDOW_SCHEMA_VERSION,
    EvidenceWindowWriter,
};
use serde_json::{Value, json};
use tempfile::TempDir;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;

const WINDOW_CAPACITY: usize = 200;

/// Capture `tracing::warn!` events emitted by the diagnostics
/// target so we can assert per-field truncation warnings (S6.4).
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
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
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

fn causal_evidence_options() -> DiagnosticsOptions {
    DiagnosticsOptions {
        full_diagnostics: false,
        runtime_diagnosis_artifacts: false,
        trace_only: false,
        session_dir: None,
        workspace_root: None,
        causal_evidence: true,

        causal_evidence_window_capacity: None,
    }
}

fn disabled_options() -> DiagnosticsOptions {
    DiagnosticsOptions {
        full_diagnostics: false,
        runtime_diagnosis_artifacts: false,
        trace_only: false,
        session_dir: None,
        workspace_root: None,
        causal_evidence: false,

        causal_evidence_window_capacity: None,
    }
}

fn full_options() -> DiagnosticsOptions {
    DiagnosticsOptions {
        full_diagnostics: true,
        runtime_diagnosis_artifacts: false,
        trace_only: false,
        session_dir: None,
        workspace_root: None,
        causal_evidence: true,

        causal_evidence_window_capacity: None,
    }
}

fn read_window_lines(session: &std::path::Path) -> Vec<Value> {
    let path = session.join("evidence-window.jsonl");
    let body = fs::read_to_string(&path).expect("read evidence-window.jsonl");
    body.lines()
        .filter(|l| !l.is_empty())
        .map(|l| serde_json::from_str(l).expect("parse window row"))
        .collect()
}

#[test]
fn evidence_window_schema_version_constant() {
    assert!(
        EVIDENCE_WINDOW_SCHEMA_VERSION.starts_with("run-diagnosis-evidence-window/"),
        "schema version constant must follow the sidecar naming convention, got {}",
        EVIDENCE_WINDOW_SCHEMA_VERSION,
    );
}

#[test]
fn ring_buffer_push_drops_oldest_past_capacity() {
    // Direct unit test of the ring buffer semantics — drives
    // `push` past capacity and asserts the queue only retains the
    // last `capacity` entries (the S6.3 contract).
    let temp = TempDir::new().expect("TempDir");
    let mut writer =
        EvidenceWindowWriter::new(temp.path(), WINDOW_CAPACITY).expect("EvidenceWindowWriter::new");

    for i in 0..(WINDOW_CAPACITY + 1) {
        writer.push(json!({"seq": i}));
    }

    assert_eq!(
        writer.buffer_len(),
        WINDOW_CAPACITY,
        "ring buffer must hold at most capacity entries",
    );
    let snapshot = writer.snapshot_buffer();
    assert_eq!(snapshot.len(), WINDOW_CAPACITY);
    assert_eq!(snapshot.first().unwrap()["seq"], json!(1));
    assert_eq!(snapshot.last().unwrap()["seq"], json!(WINDOW_CAPACITY));
}

#[test]
fn flush_writes_anomaly_descriptor_first() {
    // S6.2: anomaly descriptor is the first line of the file and
    // carries trigger_kind / ts / iteration. The frozen file is
    // not created until flush is called (S6.1).
    let temp = TempDir::new().expect("TempDir");
    let mut writer =
        EvidenceWindowWriter::new(temp.path(), WINDOW_CAPACITY).expect("EvidenceWindowWriter::new");

    let window_path = temp.path().join("evidence-window.jsonl");
    assert!(
        !window_path.exists(),
        "no file should exist before the first flush (S6.1)",
    );

    writer.push(json!({"event": "policy_receipt", "kind": "accept"}));
    writer.push(json!({"event": "policy_receipt", "kind": "reject"}));

    let anomaly = AnomalyDescriptor {
        trigger_kind: "watchdog_timeout".to_string(),
        ts: "2026-08-26T12:00:00Z".to_string(),
        iteration: 7,
        details: Some(json!({"backend": "claude"})),
    };
    writer
        .flush(anomaly, vec![json!({"event": "termination"})])
        .expect("flush");

    let lines = read_window_lines(temp.path());
    assert_eq!(lines.len(), 4, "1 anomaly + 2 pre-trigger + 1 post-trigger");

    let first = &lines[0];
    assert_eq!(
        first["schema_version"],
        json!(EVIDENCE_WINDOW_SCHEMA_VERSION)
    );
    assert_eq!(first["kind"], json!("anomaly"));
    assert_eq!(first["trigger_kind"], json!("watchdog_timeout"));
    assert_eq!(first["iteration"], json!(7));
    assert_eq!(first["details"]["backend"], json!("claude"));

    assert_eq!(lines[1]["event"], json!("policy_receipt"));
    assert_eq!(lines[2]["event"], json!("policy_receipt"));
    assert_eq!(lines[3]["event"], json!("termination"));
}

#[test]
fn flush_with_empty_buffer_still_emits_anomaly_descriptor() {
    // S6.2 edge case: an anomaly fires before any candidate line
    // has been buffered. The file is still written with only the
    // anomaly line (plus any post-trigger lines supplied by the
    // caller).
    let temp = TempDir::new().expect("TempDir");
    let mut writer =
        EvidenceWindowWriter::new(temp.path(), WINDOW_CAPACITY).expect("EvidenceWindowWriter::new");

    let anomaly = AnomalyDescriptor {
        trigger_kind: "non_zero_exit".to_string(),
        ts: "2026-08-26T12:01:00Z".to_string(),
        iteration: 0,
        details: None,
    };
    writer.flush(anomaly, vec![]).expect("flush");

    let lines = read_window_lines(temp.path());
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["kind"], json!("anomaly"));
    assert_eq!(lines[0]["trigger_kind"], json!("non_zero_exit"));
    assert_eq!(lines[0]["iteration"], json!(0));
    assert!(lines[0].get("details").is_none() || lines[0]["details"].is_null());
}

#[test]
fn ring_buffer_caps_pre_trigger_rows_at_capacity() {
    // S6.3: anomaly preceded by >capacity candidate lines; only
    // the last `capacity` pre-trigger rows land in the file. Older
    // rows are silently dropped (ring buffer semantics).
    let temp = TempDir::new().expect("TempDir");
    let mut writer =
        EvidenceWindowWriter::new(temp.path(), WINDOW_CAPACITY).expect("EvidenceWindowWriter::new");

    for i in 0..1000 {
        writer.push(json!({"seq": i, "payload": "candidate"}));
    }

    let anomaly = AnomalyDescriptor {
        trigger_kind: "precheck_exhausted".to_string(),
        ts: "2026-08-26T12:02:00Z".to_string(),
        iteration: 42,
        details: None,
    };
    writer.flush(anomaly, vec![]).expect("flush");

    let lines = read_window_lines(temp.path());
    // 1 anomaly + 200 pre-trigger rows.
    assert_eq!(lines.len(), WINDOW_CAPACITY + 1);
    assert_eq!(lines[0]["kind"], json!("anomaly"));
    // First retained pre-trigger line should be seq=800 (1000 - 200).
    assert_eq!(lines[1]["seq"], json!(1000 - WINDOW_CAPACITY));
    // Last retained pre-trigger line should be seq=999.
    assert_eq!(lines[WINDOW_CAPACITY]["seq"], json!(999));
}

#[test]
fn oversized_string_field_is_truncated_in_window_row() {
    // S6.4: per-field byte cap mirrors runtime-trace. A 50 MiB
    // prompt lands in the file capped at MAX_SIDECAR_FIELD_BYTES
    // (8 KiB) and a `tracing::warn!` names the offending field.
    let capture = WarnCapture::default();
    let subscriber = tracing_subscriber::registry().with(capture.clone());
    let _guard = tracing::subscriber::set_default(subscriber);

    let temp = TempDir::new().expect("TempDir");
    let mut writer =
        EvidenceWindowWriter::new(temp.path(), WINDOW_CAPACITY).expect("EvidenceWindowWriter::new");

    let huge = "x".repeat(50 * 1024 * 1024);
    writer.push(json!({"prompt": huge.clone()}));

    let anomaly = AnomalyDescriptor {
        trigger_kind: "abnormal_activation_outcome".to_string(),
        ts: "2026-08-26T12:03:00Z".to_string(),
        iteration: 3,
        details: None,
    };
    writer.flush(anomaly, vec![]).expect("flush");

    let body =
        fs::read_to_string(temp.path().join("evidence-window.jsonl")).expect("read window file");
    assert!(
        body.len() <= ralph_core::diagnostics::MAX_SIDECAR_FIELD_BYTES + 512,
        "evidence-window.jsonl row must fit under ~{} bytes (8 KiB field cap + ~512 bytes envelope), got {} bytes",
        ralph_core::diagnostics::MAX_SIDECAR_FIELD_BYTES + 512,
        body.len(),
    );
    assert!(body.contains("...[truncated]"));

    let events = capture.events.lock().unwrap().clone();
    assert!(
        events.iter().any(|e| e.contains("evidence_window")),
        "expected at least one warn event naming evidence_window field, got {:?}",
        events,
    );
}

#[test]
fn collector_causal_evidence_mode_creates_evidence_window_writer() {
    // The activation matrix contract for U6: when causal_evidence
    // is true, the collector must wire a working
    // `EvidenceWindowWriter` and the sidecar must appear once a
    // flush is invoked.
    let temp = TempDir::new().expect("TempDir");
    let collector = DiagnosticsCollector::with_options(temp.path(), &causal_evidence_options())
        .expect("collector");

    let session = collector.session_dir().expect("session dir").to_path_buf();

    collector.push_evidence_window_line(json!({"event": "decision_receipt"}));
    collector.push_evidence_window_line(json!({"event": "policy_receipt"}));

    collector
        .flush_evidence_window(
            AnomalyDescriptor {
                trigger_kind: "recovery_exhausted".to_string(),
                ts: "2026-08-26T12:04:00Z".to_string(),
                iteration: 9,
                details: Some(json!({"retry_key": "abc"})),
            },
            vec![json!({"event": "termination"})],
        )
        .expect("flush");

    let window_path = session.join("evidence-window.jsonl");
    assert!(
        window_path.is_file(),
        "evidence-window.jsonl must be created after flush in causal_evidence mode",
    );

    let lines = read_window_lines(&session);
    assert_eq!(lines.len(), 4, "1 anomaly + 2 pre-trigger + 1 post-trigger");
    assert_eq!(lines[0]["kind"], json!("anomaly"));
    assert_eq!(lines[0]["trigger_kind"], json!("recovery_exhausted"));
    assert_eq!(lines[1]["event"], json!("decision_receipt"));
    assert_eq!(lines[2]["event"], json!("policy_receipt"));
    assert_eq!(lines[3]["event"], json!("termination"));
}

#[test]
fn collector_disabled_mode_creates_no_evidence_window() {
    // S6.1: nothing is enabled ⇒ no session dir ⇒ no
    // `evidence-window.jsonl`. Flush becomes a no-op that cannot
    // create a file.
    let temp = TempDir::new().expect("TempDir");
    let collector =
        DiagnosticsCollector::with_options(temp.path(), &disabled_options()).expect("collector");

    collector
        .flush_evidence_window(
            AnomalyDescriptor {
                trigger_kind: "watchdog_timeout".to_string(),
                ts: "2026-08-26T12:05:00Z".to_string(),
                iteration: 0,
                details: None,
            },
            vec![],
        )
        .expect("flush on disabled collector is a no-op");

    assert!(!temp.path().join(".ralph").join("diagnostics").exists());
}

#[test]
fn collector_full_diagnostics_mode_also_creates_evidence_window_writer() {
    // Full diagnostics subsumes the causal_evidence row; the
    // frozen window writer is also wired under the full session so
    // a watchdog-timeout on a full-diagnostics run produces the
    // same sidecar as a causal-only run.
    let temp = TempDir::new().expect("TempDir");
    let collector =
        DiagnosticsCollector::with_options(temp.path(), &full_options()).expect("collector");

    collector.push_evidence_window_line(json!({"event": "trace"}));

    collector
        .flush_evidence_window(
            AnomalyDescriptor {
                trigger_kind: "non_zero_exit".to_string(),
                ts: "2026-08-26T12:06:00Z".to_string(),
                iteration: 1,
                details: None,
            },
            vec![],
        )
        .expect("flush");

    let session = collector.session_dir().expect("session dir").to_path_buf();
    let lines = read_window_lines(&session);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["kind"], json!("anomaly"));
    assert_eq!(lines[1]["event"], json!("trace"));
}

#[test]
fn post_trigger_lines_are_serialized_after_pre_trigger() {
    // The contract: anomaly → pre-trigger (from ring buffer) →
    // post-trigger (caller-supplied). Even if the post-trigger
    // payload contains oversized fields, it must be capped.
    let temp = TempDir::new().expect("TempDir");
    let mut writer =
        EvidenceWindowWriter::new(temp.path(), WINDOW_CAPACITY).expect("EvidenceWindowWriter::new");

    writer.push(json!({"seq": 0}));
    writer.push(json!({"seq": 1}));

    let anomaly = AnomalyDescriptor {
        trigger_kind: "watchdog_timeout".to_string(),
        ts: "2026-08-26T12:07:00Z".to_string(),
        iteration: 5,
        details: None,
    };
    let huge = "z".repeat(20 * 1024 * 1024);
    writer
        .flush(anomaly, vec![json!({"phase": "termination", "log": huge})])
        .expect("flush");

    let body =
        fs::read_to_string(temp.path().join("evidence-window.jsonl")).expect("read window file");
    assert!(body.contains("...[truncated]"));

    let lines = read_window_lines(temp.path());
    assert_eq!(lines.len(), 4);
    assert_eq!(lines[3]["phase"], json!("termination"));
    // Post-trigger truncation keeps the serialized row bounded.
    let last = lines.last().unwrap();
    assert!(
        last["log"].as_str().unwrap().len() <= ralph_core::diagnostics::MAX_SIDECAR_FIELD_BYTES,
        "post-trigger truncated field must fit in MAX_SIDECAR_FIELD_BYTES",
    );
}

#[test]
fn degraded_writer_emits_warn_and_drops_subsequent_writes() {
    // Once a flush fails the writer is degraded and stops trying;
    // a second flush must not panic or rewrite the file.
    // The writer opens the sidecar lazily, so the failure must
    // be triggered by removing the parent directory between
    // construction and the first flush.
    let temp = TempDir::new().expect("TempDir");
    let session = temp.path().join("session");
    fs::create_dir_all(&session).expect("create session");
    let mut writer = EvidenceWindowWriter::new(&session, 8).expect("writer");

    // Force the first flush to fail by removing the parent dir.
    fs::remove_dir_all(&session).expect("remove session");

    writer.push(json!({"seq": 0}));
    let anomaly = AnomalyDescriptor {
        trigger_kind: "watchdog_timeout".to_string(),
        ts: "2026-08-26T12:08:00Z".to_string(),
        iteration: 0,
        details: None,
    };
    let result = writer.flush(anomaly, vec![]);
    assert!(result.is_err(), "flush to nonexistent dir must error");
    assert!(writer.is_degraded());

    // Subsequent push + flush must be silent no-ops (no panic).
    writer.push(json!({"seq": 1}));
    let anomaly2 = AnomalyDescriptor {
        trigger_kind: "non_zero_exit".to_string(),
        ts: "2026-08-26T12:09:00Z".to_string(),
        iteration: 1,
        details: None,
    };
    let result2 = writer.flush(anomaly2, vec![]);
    assert!(result2.is_err());
}
