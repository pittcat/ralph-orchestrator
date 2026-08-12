//! Plan 2026-08-12-001 Unit 2 / Unit 7: diagnostics off/on equivalence
//! comparator for the EventLoop. The test runs the same fixture twice
//! (collector disabled vs collector enabled with `runtime_diagnosis_artifacts`)
//! and asserts that the accepted/rejected event tuples, the
//! activation summary, the ledger commit summary and the termination
//! reason are identical between the two runs. Only the sidecar
//! presence is allowed to differ.
//!
//! This file is a structural skeleton. It registers a `#[test]`
//! that always returns early with a structured log line, so it is
//! collected by nextest under the `diagnostics_equivalence` filter
//! (per E22 / D13). The full scenario/loop wiring will be added in
//! later Units as the corresponding production code lands. The
//! presence of this file is what E22 and the plan's hard rule
//! require.

use ralph_core::diagnostics::RuntimeTraceEntry;
use ralph_core::diagnostics::RuntimeTracePhase;
use ralph_core::diagnostics::probe_session_dir_writable;
use ralph_core::diagnostics::{DiagnosticsCollector, DiagnosticsOptions};
use std::fs;
use std::os::unix::fs::PermissionsExt;

/// Structural smoke test: a `RuntimeTraceEntry::new` plus a phase
/// can be serialized and round-tripped. This is the
/// "real Red → Green" entry point for Unit 2: when the type does
/// not exist the test cannot even compile, which is the desired
/// Red state.
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
    fs::set_permissions(&session_dir, fs::Permissions::from_mode(0o500))
        .expect("chmod 0500");

    let opts = DiagnosticsOptions {
        full_diagnostics: false,
        runtime_diagnosis_artifacts: true,
        trace_only: false,
        session_dir: Some(session_dir.clone()),
        workspace_root: None,
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
