//! 2026-06-14-003 R1 integration tests: build_prompt must surface
//! `## WAVE CONTEXT` for `review-synthesizer`.
//!
//! The smoke test is intentionally minimal — it builds a sandbox
//! workspace, drops a handful of `review.wave.ready` /
//! `review.dimension.done` events into the events file, then asks the
//! event loop to build the synthesizer prompt and asserts the wave
//! context block is present.  The unit tests in `wave_context.rs`
//! cover the resolution logic; the integration test covers the
//! end-to-end pipeline (events file → resolver → prompt prepend).

use crate::event_loop::tests::common::init_git_workspace;
use crate::event_loop::{EventLoop, HatId};
use crate::loop_context::LoopContext;
use std::fs;
use std::io::Write;
use std::path::Path;

/// Write a `review.wave.ready` or `review.dimension.done` record to
/// the events file.  The record carries the `wave_id` / `wave_total`
/// fields the resolver consumes.
fn write_wave_event(
    path: &Path,
    topic: &str,
    wave_id: &str,
    wave_total: Option<u32>,
    payload: &str,
) {
    let ts = "2026-06-14T00:00:00Z".to_string();
    let record = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": ts,
        "hat": "review-coordinator",
        "wave_id": wave_id,
        "wave_total": wave_total,
    });
    let mut f = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .expect("open events");
    writeln!(f, "{}", record).expect("write");
}

fn solo_config(workspace: &Path) -> crate::config::RalphConfig {
    let mut config = crate::config::RalphConfig::default();
    config.core.workspace_root = workspace.to_path_buf();
    config
}

#[test]
fn build_prompt_injects_wave_context_block_for_synthesizer() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let ralph_dir = dir.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).expect("ralph dir");
    let events_path = ralph_dir.join("events.jsonl");

    // Synthesize a complete review wave: 1 `ready` declaring 3
    // expected dimensions, 3 `done` events covering all of them.
    write_wave_event(
        &events_path,
        "review.wave.ready",
        "w-abc",
        Some(3),
        r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"correctness"}"#,
    );
    write_wave_event(
        &events_path,
        "review.dimension.done",
        "w-abc",
        None,
        r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"correctness","findings_count":0,"findings_file":"f.json"}"#,
    );
    write_wave_event(
        &events_path,
        "review.dimension.done",
        "w-abc",
        None,
        r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"testing","findings_count":0,"findings_file":"f.json"}"#,
    );
    write_wave_event(
        &events_path,
        "review.dimension.done",
        "w-abc",
        None,
        r#"{"plan_name":"p","task_id":"t1","task_key":"k1","step":"1","dimension":"maintainability","findings_count":0,"findings_file":"f.json"}"#,
    );

    let config = solo_config(dir.path());
    let ctx = LoopContext::primary(dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx);

    let hat = HatId::new("review-synthesizer");
    let block = event_loop
        .build_wave_context_for_synthesizer_if_match_for_test(&hat)
        .expect("synthesizer should see a wave context");

    assert_eq!(block.wave_id, "w-abc");
    assert_eq!(block.wave_total, 3);
    assert_eq!(block.received_count, 3);
    assert!(block.all_dimensions_received);
    let rendered = block.to_prompt_block();
    assert!(rendered.starts_with("## WAVE CONTEXT\n"));
    assert!(rendered.contains("\"wave_id\": \"w-abc\""));
    assert!(rendered.contains("\"ALL_DIMENSIONS_RECEIVED\": true"));
}

#[test]
fn build_wave_context_skipped_for_non_synthesizer_hats() {
    // Non-synthesizer hats must not see a wave context — the
    // aggregate context is meaningful only for `review-synthesizer`.
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let ralph_dir = dir.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).expect("ralph dir");
    let events_path = ralph_dir.join("events.jsonl");
    write_wave_event(
        &events_path,
        "review.wave.ready",
        "w-x",
        Some(1),
        r#"{"dimension":"correctness"}"#,
    );

    let config = solo_config(dir.path());
    let ctx = LoopContext::primary(dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx);

    let hat = HatId::new("executor");
    let block = event_loop.build_wave_context_for_synthesizer_if_match_for_test(&hat);
    assert!(block.is_none(), "executor must not see a wave context");
}

#[test]
fn build_wave_context_carries_aggregate_timeout_false_when_no_pin() {
    // Default (no pin) must serialise `AGGREGATE_TIMEOUT: false` so
    // the agent can rely on the field always being present.  The
    // pin-driven `true` path is exercised in the unit tests
    // (`resolve_wave_context_for_synthesizer_with_aggregate_timeout`).
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let ralph_dir = dir.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).expect("ralph dir");
    let events_path = ralph_dir.join("events.jsonl");
    write_wave_event(
        &events_path,
        "review.wave.ready",
        "w-pin",
        Some(1),
        r#"{"dimension":"correctness"}"#,
    );

    let config = solo_config(dir.path());
    let ctx = LoopContext::primary(dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx);

    let hat = HatId::new("review-synthesizer");
    let block = event_loop
        .build_wave_context_for_synthesizer_if_match_for_test(&hat)
        .expect("synthesizer should see a wave context");
    assert!(!block.aggregate_timeout);
    let rendered = block.to_prompt_block();
    assert!(rendered.contains("\"AGGREGATE_TIMEOUT\": false"));
}

#[test]
fn build_prompt_pin_is_consumed_on_first_read() {
    // R1 (2026-06-14-003 plan) §5.1.4 / adversarial S9: the
    // `pending_synthesizer_timeout` pin MUST be cleared on the
    // first read so a stale wave-1 timeout does not bleed into
    // wave-2's synthesizer activation.  This test sets the pin
    // directly on `LoopState` (the field is `pub` per the state
    // contract) and asserts the resolver's `aggregate_timeout`
    // flag flips back to `false` after a single consumption.
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let ralph_dir = dir.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).expect("ralph dir");
    let events_path = ralph_dir.join("events.jsonl");
    write_wave_event(
        &events_path,
        "review.wave.ready",
        "w-pin",
        Some(1),
        r#"{"dimension":"correctness"}"#,
    );

    let config = solo_config(dir.path());
    let ctx = LoopContext::primary(dir.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx);

    // Set the pin manually — the production writer is
    // `inject_review_aggregate_timeouts`.  The test exercises the
    // reader side: does it consume the pin on first read?
    event_loop.state_mut().pending_synthesizer_timeout = Some("w-pin".to_string());
    let hat = HatId::new("review-synthesizer");

    // First read: pin is set, so `aggregate_timeout` is `true`.
    let first = event_loop
        .build_wave_context_for_synthesizer_if_match_for_test(&hat)
        .expect("synthesizer should see a wave context");
    assert!(
        first.aggregate_timeout,
        "first read must honour the pin; got: {:?}",
        first
    );

    // Second read: pin was consumed, so `aggregate_timeout` is `false`.
    let second = event_loop
        .build_wave_context_for_synthesizer_if_match_for_test(&hat)
        .expect("wave context still present");
    assert!(
        !second.aggregate_timeout,
        "second read must NOT see a stale pin; got: {:?}",
        second
    );
}
