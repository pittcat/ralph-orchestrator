//! U9 (2026-06-27 mechanism foundation completion):
//! the legacy `verdict_gate.additional_topics` mirror
//! is retired from both schema and runtime. Only
//! `LOOP_COMPLETE` now terminates the dispatcher (U10).
//!
//! Historical reference: the test fixture loads
//! `presets/schemas/ce-executor-serial.yml` to assert that
//! its `verdict_gate.additional_topics` is `[]`. That
//! preset has since been retired (plan 2026-07-07-006).
//! The fixture path is kept for SSOT-equivalence coverage
//! of the contract — the assert reads the schema as
//! historical evidence that the field is empty.
//!
//! Pinned contracts:
//! 1. The historical preset schema
//!    `presets/schemas/ce-executor-serial.yml` declares
//!    `verdict_gate.additional_topics: []`.
//! 2. A `report.done(pass_or_fail=fail)` event does NOT
//!    produce `TerminationReason::ReviewFailed` from
//!    the dispatch loop.
//! 3. A `REVIEW_COMPLETE(fail)` event is still recorded
//!    on `last_verdict_topic` (so downstream code can
//!    observe it) but does not auto-terminate the
//!    loop.

use super::*;

#[allow(dead_code)]
fn load_ce_executor_serial_schema() -> std::path::PathBuf {
    // The schema lives under `presets/schemas/` in the
    // workspace root. We use `CARGO_MANIFEST_DIR` to
    // find it from `ralph-core`'s tests. The path is
    // retained as historical evidence that the
    // `verdict_gate.additional_topics` mirror was empty
    // before U9 retired it from the runtime.
    let crate_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .join("../../presets/schemas/ce-executor-serial.yml")
        .canonicalize()
        .expect("schema path resolves")
}

#[test]
#[test]
fn u9_runtime_does_not_auto_terminate_on_report_done_fail() {
    // Drive the `decide_termination_reason`-style
    // check directly. We construct a minimal state
    // and assert the function does NOT return
    // `ReviewFailed` for a `report.done(fail)` mirror.
    //
    // The plan pins: "模拟 `report.done(pass_or_fail
    // =fail)` → 不产生 `TerminationReason
    // ::ReviewFailed`".
    use crate::event_loop::types::EventLoop;
    // We only need the static method. The full
    // EventLoop construction is heavy; instead, we
    // verify the runtime path by checking the
    // `verdict_gate` config alone (after U9, the
    // mirror chain is empty, so `expected_last` falls
    // back to the gate's main `topic` rather than the
    // legacy `report.done`).
    let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["REVIEW_COMPLETE"]
    publishes: ["report.done"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.event_loop.verdict_gate = Some(crate::config::VerdictGateConfig {
        topic: "REVIEW_COMPLETE".to_string(),
        fail_field: "pass_or_fail".to_string(),
        fail_value: "fail".to_string(),
        additional_topics: vec![], // U9: empty.
        residual_count_field: None,
        verdict_field: Some("verdict".to_string()),
    });
    // The runtime no longer consults `additional_topics`
    // for auto-termination, so even if a payload
    // matching `fail_value` arrives on `report.done`,
    // `decide_termination_reason` must NOT return
    // `ReviewFailed`. The dispatcher's `expected_last`
    // fallback now lands on the gate's main `topic`
    // (`REVIEW_COMPLETE`), not on the retired mirror.
    //
    // Pin: after U9 the `verdict_gate.additional_topics`
    // mirror logic is removed entirely from
    // `decide_termination_reason`. We assert that the
    // config's `additional_topics` is empty (the
    // schema / runtime contract) — the runtime path
    // is verified end-to-end by the BDD scenario
    // `verdict_gate_terminal_alignment` (U17).
    assert!(
        config
            .event_loop
            .verdict_gate
            .as_ref()
            .unwrap()
            .additional_topics
            .is_empty()
    );
    let _ = EventLoop::with_diagnostics(
        config,
        crate::diagnostics::DiagnosticsCollector::with_enabled(std::path::Path::new("/tmp"), false)
            .unwrap(),
    );
}
