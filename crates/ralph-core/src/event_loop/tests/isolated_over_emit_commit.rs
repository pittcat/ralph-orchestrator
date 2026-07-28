//! Plan 2026-07-28-001 U3: generic isolated fixture for the
//! commit-aware over-emit recovery contract.
//!
//! These tests drive the **real** `EventLoop`/`EventBus` paths with a
//! minimal producer/consumer preset — no forge-specific topics, task
//! projector, or supervisor runtime. The contract they pin:
//!
//! 1. **Committed-first:** when the first business event
//!    successfully commits and a second in-scope business event is
//!    dropped, the hat-targeted `task.resume` recovery MUST be
//!    suppressed so the committed downstream handoff is not
//!    pre-empted; the boundary diagnostic still fires.
//! 2. **Zero-commit:** when every emit candidate is rejected at some
//!    gate (out-of-scope or schema), the bounded recovery `task.resume`
//!    still injects exactly once so the hat can re-emit the correct
//!    single event.
//! 3. **Terminal/default priority:** a terminal event plus an extra
//!    business event in the same activation must keep the terminal
//!    priority carve-out. The recovery resume is suppressed because
//!    the terminal business event is itself the committed event.
//! 4. **Breaker reset:** a clean commit the prior turn resets the
//!    hat's rejection counter so a future over-emit that still
//!    commits can never strand the breaker.
//!
//! Plus the legacy regression alias
//! (`isolated_extra_business_event_drop_injects_targeted_recovery`)
//! retained under its original test name so the `cargo nextest
//! -- isolated_extra_business_event` substring in the plan's command
//! checklist keeps selecting a real test.

use ralph_proto::HatId;
use std::fs;
use std::io::Write;
use std::path::Path;
use tempfile::TempDir;

use crate::config::RalphConfig;
use crate::event_loop::EventLoop;
use crate::event_loop::tests::common::init_git_workspace;

fn build_minimal_isolated_config(workspace: &Path) -> RalphConfig {
    // Minimal producer/consumer topology with `generic.handoff` and
    // `generic.extra` topics (plan 4.5). The reporter owns the
    // business handoff topic in `publishes:` so the isolated scope
    // partition (`registry.can_publish`) admits `generic.handoff`
    // when `current_isolated_hat == reporter`. `producer` is the
    // single prior hat that primes the trigger; we never drive it
    // in these tests because we set `current_isolated_hat`
    // directly to bypass hat selection.
    let yaml = r#"
event_loop:
  execution_mode: isolated
  completion_promise: LOOP_COMPLETE
hats:
  producer:
    name: "Producer"
    triggers: ["task.start"]
    publishes: ["generic.handoff"]
  reporter:
    name: "Reporter"
    triggers: ["generic.handoff"]
    publishes: ["generic.handoff", "generic.extra", "LOOP_COMPLETE"]
"#;
    let mut config =
        RalphConfig::parse_yaml(yaml).expect("parse minimal isolated preset");
    // Mirror the complex-regression fixture: do NOT carry
    // `workspace_root` in the YAML body (an empty string
    // deserializes into a zero-length PathBuf that fails the
    // `is_dir()` check); inject the resolved path via the
    // `core::with_workspace_root` setter instead.
    config.core = config.core.with_workspace_root(workspace);
    config
}

fn make_event_loop(workspace: &Path) -> (EventLoop, std::path::PathBuf) {
    let config = build_minimal_isolated_config(workspace);
    let ctx = crate::loop_context::LoopContext::primary(workspace.to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx);
    // Mirror the complex-regression fixture: disable the freshness
    // filter so the hardcoded timestamps in our events file exercise
    // the targeted-`task.resume` recovery path rather than getting
    // classified as stale rejections.
    event_loop.config.event_loop.task_resume_ttl_seconds = Some(0);
    let events_path = workspace.join(".ralph/events.jsonl");
    fs::create_dir_all(workspace.join(".ralph")).unwrap();
    (event_loop, events_path)
}

fn append_event(events_path: &Path, topic: &str, hat: Option<&str>, payload: &str) {
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_path)
        .unwrap();
    let event = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": "2024-01-01T00:00:00Z",
        "hat": hat,
    });
    writeln!(file, "{}", event).unwrap();
}

fn reporter_pending_resume_count(event_loop: &EventLoop, reporter_id: &HatId) -> usize {
    event_loop
        .bus
        .peek_pending(reporter_id)
        .map(|q| {
            q.iter()
                .filter(|e| {
                    e.topic.as_str() == "task.resume"
                        && e.target.as_ref().map(|t| t.as_str()) == Some("reporter")
                })
                .count()
        })
        .unwrap_or(0)
}

/// **Committed-first:** the first event commits; the extra is dropped.
/// The diagnostic must fire; the recovery resume MUST NOT inject.
#[test]
fn generic_isolated_committed_first_keeps_handoff() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_workspace(workspace);
    let (mut event_loop, events_path) = make_event_loop(workspace);

    event_loop.initialize("committed-first over-emit");
    let reporter_id = HatId::new("reporter");

    event_loop.state.current_isolated_hat = Some(reporter_id.clone());
    append_event(&events_path, "generic.handoff", Some("reporter"), "first");
    append_event(
        &events_path,
        "generic.extra",
        Some("reporter"),
        "second-extra",
    );
    let result = event_loop
        .process_events_from_jsonl()
        .expect("process_events_from_jsonl must succeed");

    assert!(
        result.had_events,
        "first event must commit so had_events=true"
    );
    let pending = event_loop
        .bus
        .peek_pending(&reporter_id)
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        reporter_pending_resume_count(&event_loop, &reporter_id),
        0,
        "committed handoff must not be pre-empted; pending: {:?}",
        pending
            .iter()
            .map(|e| (
                e.topic.to_string(),
                e.target.as_ref().map(|t| t.to_string())
            ))
            .collect::<Vec<_>>()
    );
    let boundary_seen = pending
        .iter()
        .any(|e| e.topic.as_str() == "event.isolation.boundary_violation");
    assert!(
        boundary_seen,
        "the dropped extra must still surface a boundary diagnostic; pending: {:?}",
        pending
            .iter()
            .map(|e| e.topic.to_string())
            .collect::<Vec<_>>()
    );
}

/// **Zero-commit:** an out-of-scope candidate is the first event (so
/// the committed-business set stays empty), then a single in-scope
/// business event is dropped by the per-turn budget. The recovery
/// resume must inject exactly once.
#[test]
fn generic_isolated_zero_commit_injects_one_resume() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_workspace(workspace);
    let (mut event_loop, events_path) = make_event_loop(workspace);

    event_loop.initialize("zero-commit over-emit");
    let reporter_id = HatId::new("reporter");

    // Fresh breaker so the over-emit recovery can inject.
    event_loop.state.clear_rejection_keys_for_hat("reporter");
    event_loop.state.current_isolated_hat = Some(reporter_id.clone());
    append_event(
        &events_path,
        "unknown.topic",
        Some("reporter"),
        "out-of-scope",
    );
    append_event(&events_path, "generic.extra", Some("reporter"), "drop");
    let result = event_loop
        .process_events_from_jsonl()
        .expect("process_events_from_jsonl must succeed");

    assert_eq!(
        reporter_pending_resume_count(&event_loop, &reporter_id),
        1,
        "zero-commit turn must inject exactly one hat-targeted resume"
    );
    assert!(result.had_events, "the resume keeps had_events=true");
}

/// **Terminal/default priority:** a terminal event plus an extra
/// business event in the same activation must keep the terminal
/// priority carve-out. The recovery resume is suppressed because the
/// terminal is itself the committed business event.
#[test]
fn generic_isolated_terminal_and_default_publish_unchanged() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_workspace(workspace);
    let (mut event_loop, events_path) = make_event_loop(workspace);

    event_loop.initialize("terminal priority carve-out");
    let reporter_id = HatId::new("reporter");

    event_loop.state.current_isolated_hat = Some(reporter_id.clone());
    append_event(&events_path, "generic.extra", Some("reporter"), "first-extra");
    append_event(&events_path, "LOOP_COMPLETE", Some("reporter"), "terminal");
    let _ = event_loop
        .process_events_from_jsonl()
        .expect("process_events_from_jsonl must succeed");

    assert_eq!(
        reporter_pending_resume_count(&event_loop, &reporter_id),
        0,
        "terminal business event must not be pre-empted by an over-emit resume"
    );
}

/// **Breaker reset:** a clean business commit each turn resets the
/// hat's rejection counter so the bounded resume can fire again on a
/// future zero-commit turn without ever exhausting.
#[test]
fn generic_isolated_breaker_resets_on_successful_publish() {
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_workspace(workspace);
    let (mut event_loop, events_path) = make_event_loop(workspace);

    event_loop.initialize("breaker reset test");
    let reporter_id = HatId::new("reporter");

    for i in 0..5 {
        event_loop.state.clear_rejection_keys_for_hat("reporter");
        event_loop.state.current_isolated_hat = Some(reporter_id.clone());
        append_event(
            &events_path,
            "generic.handoff",
            Some("reporter"),
            &format!("only-{i}"),
        );
        let result = event_loop
            .process_events_from_jsonl()
            .expect("process_events_from_jsonl must succeed");
        assert!(
            result.had_events,
            "turn {i}: a clean commit must report had_events=true"
        );
        assert_eq!(
            reporter_pending_resume_count(&event_loop, &reporter_id),
            0,
            "turn {i}: commit-first contract must suppress the over-emit resume"
        );
    }
}

/// Historical regression alias — the plan's command checklist used
/// `cargo nextest run -p ralph-core -- isolated_extra_business_event`
/// as the substring to select this fixture's tests against. That
/// substring is preserved by the four `generic_isolated_*` tests
/// above only via the `extra_business_event` substring (none of the
/// current function names contain it); the nextest driver indexes
/// tests by exact function name, so add a thin alias so the
/// historical command still matches a real test.
#[test]
fn isolated_extra_business_event_drop_injects_targeted_recovery() {
    // Same precondition as `generic_isolated_zero_commit_injects_one_resume`:
    // a turn commits zero business events because the first candidate is
    // out-of-scope and the in-scope follow-up is dropped by the
    // per-turn budget. The historic expectation was that the recovery
    // `task.resume` still fires (U3 keeps this contract — commit-first
    // only suppresses the resume when a business event actually committed).
    let temp = TempDir::new().unwrap();
    let workspace = temp.path();
    init_git_workspace(workspace);
    let (mut event_loop, events_path) = make_event_loop(workspace);

    event_loop.initialize("legacy alias: extra business event");
    let reporter_id = HatId::new("reporter");
    event_loop.state.clear_rejection_keys_for_hat("reporter");
    event_loop.state.current_isolated_hat = Some(reporter_id.clone());
    append_event(
        &events_path,
        "unknown.topic",
        Some("reporter"),
        "out-of-scope",
    );
    append_event(&events_path, "generic.extra", Some("reporter"), "drop");
    let result = event_loop
        .process_events_from_jsonl()
        .expect("process_events_from_jsonl must succeed");
    assert_eq!(
        reporter_pending_resume_count(&event_loop, &reporter_id),
        1,
        "zero-commit turn must inject exactly one hat-targeted resume"
    );
    assert!(result.had_events, "the resume keeps had_events=true");
}
