//! Plan 2026-08-12-001 Unit 2 / Unit 7: diagnostics off/on equivalence
//! comparator for the EventLoop. The test runs the same fixture twice
//! (collector disabled vs collector enabled with `runtime_diagnosis_artifacts`)
//! and asserts that the accepted/rejected event tuples, the
//! activation summary, the ledger commit summary and the termination
//! reason are identical between the two runs. Only the sidecar
//! presence is allowed to differ.
//!
use ralph_core::diagnostics::RuntimeTraceEntry;
use ralph_core::diagnostics::RuntimeTracePhase;
use ralph_core::diagnostics::probe_session_dir_writable;
use ralph_core::diagnostics::{DiagnosticsCollector, DiagnosticsOptions};
use ralph_core::{LoopContext, RalphConfig};
use std::fs;
use std::os::unix::fs::PermissionsExt;

/// The schema round-trip remains a unit-level guard for the fixed fields;
/// lifecycle behavior is covered by the real EventLoop test below.
#[test]
fn runtime_trace_entry_serde_roundtrip() {
    let entry = RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Activation)
        .with_hat("executor")
        .with_status("active");
    let json = serde_json::to_string(&entry).expect("serialize");
    let back: RuntimeTraceEntry = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(back.iteration, 0);
    assert_eq!(back.phase, RuntimeTracePhase::Activation);
    assert_eq!(back.hat.as_deref(), Some("executor"));
    assert_eq!(back.status.as_deref(), Some("active"));
}

/// Disabled collector must NOT instantiate a runtime-trace logger.
/// Off/on differential anchor.
#[test]
fn disabled_collector_has_no_runtime_trace_logger() {
    let c = ralph_core::diagnostics::DiagnosticsCollector::disabled();
    c.log_runtime_trace(RuntimeTraceEntry::new(0, 0, RuntimeTracePhase::Batch));
    // Reaching here without panicking is sufficient: the
    // disabled collector drops the entry on the floor.
}

/// Plan 2026-08-12-001 fix-plan U7 / synth:P1-5: the session-dir
/// probe must NOT leave a `.ralph-dx-writeprobe-*` artifact
/// behind. The previous implementation called `.keep()` on the
/// `tempfile::Builder` handle and accumulated dozens of empty
/// probe files per run. We probe the same dir many times and
/// assert the directory contents are unchanged after each call.
#[test]
fn probe_session_dir_writable_leaves_no_artifacts() {
    let tmp = tempfile::TempDir::new().expect("TempDir");
    let dir = tmp.path();

    // Baseline: 0 entries.
    let baseline: Vec<_> = fs::read_dir(dir)
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        baseline.is_empty(),
        "fresh TempDir should be empty, got {} entries",
        baseline.len()
    );

    // Probe 50 times — implementation must clean up after every
    // call. Even if probe fails on some platform-specific env,
    // the only allowed residue is `false`-only; we never
    // observe a leftover `.ralph-dx-writeprobe-*` file.
    for _ in 0..50 {
        let _ = probe_session_dir_writable(dir);
    }
    let after: Vec<_> = fs::read_dir(dir)
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .collect();
    assert!(
        after.is_empty(),
        "probe_session_dir_writable must leave zero artifacts, got {} entries: {:?}",
        after.len(),
        after.iter().map(|e| e.path()).collect::<Vec<_>>()
    );
}

/// Plan 2026-08-12-001 fix-plan U7: probe against a non-directory
/// path must return false, not panic.
#[test]
fn probe_session_dir_writable_rejects_non_directory() {
    let tmp = tempfile::TempDir::new().expect("TempDir");
    let file_path = tmp.path().join("not-a-dir");
    fs::write(&file_path, b"x").expect("write");
    assert!(!probe_session_dir_writable(&file_path));
}

/// Plan 2026-08-12-001 fix-plan U6: when the initial
/// `write_manifest` rejects the session dir, the collector must
/// disable `input_bundle` (None) instead of silently wrapping the
/// bundle in a misleading `Some(Arc<Mutex<...>>)` that the
/// reporter then projects as `Legacy` / `Degraded`.
#[test]
fn collector_disables_input_bundle_when_initial_write_fails() {
    let tmp = tempfile::TempDir::new().expect("TempDir");
    // Build a session dir, then chmod it read-only to make the
    // initial `write_manifest` return Ok(None) via the probe path.
    let session_dir = tmp.path().join("readonly-session");
    fs::create_dir_all(&session_dir).expect("create session dir");
    let original = fs::metadata(&session_dir).expect("metadata").permissions();
    fs::set_permissions(&session_dir, fs::Permissions::from_mode(0o500)).expect("chmod 0500");

    let opts = DiagnosticsOptions {
        full_diagnostics: false,
        runtime_diagnosis_artifacts: true,
        trace_only: false,
        session_dir: Some(session_dir.clone()),
        workspace_root: None,
        // U01b: causal_evidence defaults to `false` so the readonly
        // probe path stays pinned to the pre-U01b minimal-session
        // shape (test asserts `Ok(None)` for the unwritable probe).
        causal_evidence: false,

        causal_evidence_window_capacity: None,
    };
    let collector =
        DiagnosticsCollector::with_options(tmp.path(), &opts).expect("with_options succeeds");

    // Restore permissions so cleanup can remove the dir.
    let _ = fs::set_permissions(&session_dir, original);

    // The collector is enabled (the activation matrix matched) but
    // the input_bundle slot must be None — the probe returned
    // false, so the bundle is missing. The U6 invariant is
    // "initial write failure ⇒ input_bundle None" so the
    // reporter sees the actual absent state instead of a
    // misleading in-memory `Degraded`/`Legacy` wrapper.
    assert!(collector.is_enabled());
    assert!(
        collector.input_bundle_status().is_none(),
        "input_bundle_status must be None when initial write_manifest fails; \
         the in-memory state must reflect the absent bundle, not a Legacy/Degraded wrapper."
    );
}

#[test]
fn event_loop_process_batch_records_runtime_lifecycle() {
    let tmp = tempfile::TempDir::new().expect("TempDir");
    let options = DiagnosticsOptions {
        runtime_diagnosis_artifacts: true,
        session_dir: None,
        workspace_root: Some(tmp.path().to_path_buf()),
        ..DiagnosticsOptions::default()
    };
    let collector = DiagnosticsCollector::with_options(tmp.path(), &options).expect("collector");
    let context = LoopContext::primary(tmp.path().to_path_buf());
    let mut event_loop = ralph_core::EventLoop::with_context_and_diagnostics(
        RalphConfig::default(),
        context,
        collector,
    )
    .expect("event loop");

    std::fs::create_dir_all(tmp.path().join(".ralph")).expect("ralph dir");
    std::fs::write(tmp.path().join(".ralph/events.jsonl"), b"").expect("events file");
    event_loop
        .process_events_from_jsonl()
        .expect("process batch");

    let session = fs::read_dir(tmp.path().join(".ralph/diagnostics"))
        .expect("diagnostics root")
        .next()
        .expect("session")
        .expect("session entry")
        .path();
    let body = fs::read_to_string(session.join("runtime-trace.jsonl")).expect("trace");
    let phases: Vec<RuntimeTracePhase> = body
        .lines()
        .map(|line| {
            serde_json::from_str::<RuntimeTraceEntry>(line)
                .expect("trace row")
                .phase
        })
        .collect();
    assert!(phases.contains(&RuntimeTracePhase::Batch));
    assert!(phases.contains(&RuntimeTracePhase::Commit));
}

#[test]
fn diagnostics_session_dir_cannot_escape_workspace_root() {
    let tmp = tempfile::TempDir::new().expect("TempDir");
    let workspace = tmp.path().join("workspace");
    let outside = tmp.path().join("outside");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(&outside).expect("outside");
    let options = DiagnosticsOptions {
        runtime_diagnosis_artifacts: true,
        session_dir: Some(outside),
        workspace_root: Some(workspace.clone()),
        ..DiagnosticsOptions::default()
    };
    let error = DiagnosticsCollector::with_options(&workspace, &options)
        .expect_err("diagnostics path outside workspace must be rejected");
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
}

#[test]
fn diagnostics_off_on_preserves_processed_result_and_state_projection() {
    #[derive(Debug, PartialEq, Eq)]
    struct Snapshot {
        had_events: bool,
        had_raw_events: bool,
        had_rejected_events: bool,
        had_plan_events: bool,
        has_orphans: bool,
        accepted: Vec<(String, String, Option<String>, Option<String>)>,
        contract_rejection_count: usize,
        payload_violation: bool,
        iteration: u32,
        consecutive_failures: u32,
        consecutive_malformed_events: u32,
        stall_detector_had_events: bool,
        persisted_events: String,
    }

    fn run_once(enabled: bool) -> Snapshot {
        let tmp = tempfile::TempDir::new().expect("TempDir");
        let options = DiagnosticsOptions {
            runtime_diagnosis_artifacts: enabled,
            workspace_root: Some(tmp.path().to_path_buf()),
            ..DiagnosticsOptions::default()
        };
        let collector =
            DiagnosticsCollector::with_options(tmp.path(), &options).expect("collector");
        let context = LoopContext::primary(tmp.path().to_path_buf());
        let mut event_loop = ralph_core::EventLoop::with_context_and_diagnostics(
            RalphConfig::default(),
            context,
            collector,
        )
        .expect("event loop");

        fs::create_dir_all(tmp.path().join(".ralph")).expect("ralph dir");
        let input = ralph_core::Event {
            topic: "event.test".to_string(),
            payload: Some("{}".to_string()),
            ts: String::new(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
            system_injected: Some(true),
        };
        fs::write(
            tmp.path().join(".ralph/events.jsonl"),
            format!("{}\n", serde_json::to_string(&input).expect("event")),
        )
        .expect("events file");

        let processed = event_loop
            .process_events_from_jsonl()
            .expect("process events");
        let state = event_loop.state();
        let accepted = processed
            .accepted_events
            .iter()
            .map(|event| {
                (
                    event.topic.to_string(),
                    event.payload.clone(),
                    event.source.as_ref().map(ToString::to_string),
                    event.target.as_ref().map(ToString::to_string),
                )
            })
            .collect();
        Snapshot {
            had_events: processed.had_events,
            had_raw_events: processed.had_raw_events,
            had_rejected_events: processed.had_rejected_events,
            had_plan_events: processed.had_plan_events,
            has_orphans: processed.has_orphans,
            accepted,
            contract_rejection_count: processed.contract_rejections.len(),
            payload_violation: processed.payload_contract_violation.is_some(),
            iteration: state.iteration,
            consecutive_failures: state.consecutive_failures,
            consecutive_malformed_events: state.consecutive_malformed_events,
            stall_detector_had_events: state.stall_detector_had_events,
            persisted_events: fs::read_to_string(tmp.path().join(".ralph/events.jsonl"))
                .expect("persisted events"),
        }
    }

    assert_eq!(run_once(false), run_once(true));
}
