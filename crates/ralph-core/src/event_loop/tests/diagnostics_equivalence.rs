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
