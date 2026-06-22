//! U11-T2: per-event unified ValidationPipeline integration
//!
//! Verifies the pipeline's pre-commit API works on synthetic
//! events matching the runtime's event shape. Production wire-up
//! is exercised by the `process_parse_result` integration in
//! `event_loop/mod.rs` (guarded behind `UNIFIED_VALIDATION=1`).

use crate::config::EventLoopConfig;
use crate::event_reader::Event as JsonlEvent;
use crate::preset::engine::protocol::ProtocolView;
use crate::state::LedgerSnapshot;
use crate::validation::ValidationPipeline;

#[test]
fn pre_commit_pipeline_accepts_legal_event() {
    let view = ProtocolView::from_event_loop_with_index_and_feature(
        &EventLoopConfig::default(),
        None,
        true,
    );
    let snapshot = LedgerSnapshot::cold_start();
    let pipeline = ValidationPipeline::from_config(&view, &EventLoopConfig::default());

    let event = JsonlEvent {
        topic: "debug.step".to_string(),
        payload: Some("task_id=demo".to_string()),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    let results = pipeline.validate_pre_commit_with_view(&view, &snapshot, &event);
    assert!(
        results.iter().all(|r| r.accepted),
        "well-formed debug.step event should pass pre-commit; got rejections: {:?}",
        results
            .iter()
            .filter(|r| !r.accepted)
            .map(|r| (r.stage.as_str(), r.reason_code.as_deref().unwrap_or("")))
            .collect::<Vec<_>>()
    );
}

#[test]
fn pre_commit_pipeline_returns_one_result_per_rule() {
    let view = ProtocolView::from_event_loop_with_index_and_feature(
        &EventLoopConfig::default(),
        None,
        true,
    );
    let snapshot = LedgerSnapshot::cold_start();
    let pipeline = ValidationPipeline::from_config(&view, &EventLoopConfig::default());

    let event = JsonlEvent {
        topic: "debug.step".to_string(),
        payload: Some("not-json-at-all".to_string()),
        ts: String::new(),
        hat: None,
        triggered: None,
        source: None,
        wave_id: None,
        wave_index: None,
        wave_total: None,
        system_injected: None,
    };
    let results = pipeline.validate_pre_commit_with_view(&view, &snapshot, &event);
    // One ValidationResult per pre-commit rule, regardless of accepted/rejected.
    assert_eq!(
        results.len(),
        pipeline.pre_commit_rules.len(),
        "pipeline must return one result per pre-commit rule"
    );
}