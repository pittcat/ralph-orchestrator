//! U3 (P0 #2 fix) — End-to-end tests for the hat lifecycle tracker that
//! drive `process_events_from_jsonl`, not the lower-level tracker API.
//!
//! # Why this file exists
//!
//! The original `hat_lifecycle_integration.rs` constructed `ActivationKey`
//! values directly and called `tracker.activate(...) / complete(...)` in
//! isolation. P0 code-review finding #2 flagged this as "test false
//! confidence": the unit tests all passed even when production paths
//! (the activate/complete branches inside `process_events_from_jsonl`)
//! were broken. In particular, the legacy `can_publish`-based
//! `trigger_identity` reverse lookup (P0 #1) leaked every activation in
//! production while leaving every unit test green.
//!
//! These tests write real JSONL events to `.ralph/events.jsonl`, drive
//! `EventLoop::process_events_from_jsonl()` (the production code path
//! that calls `tracker.complete` for terminal events), and assert on the
//! public `hat_lifecycle_tracker()` API. If P0 #1 ever regresses,
//! `tracker.complete()` will hit the `None` branch and only warn, leaving
//! the activation leaked.

use super::*;
use super::common::*;
use ralph_proto::HatId;

/// Minimal event-loop config used by every test in this file.
///
/// The topology mirrors the ce-executor preset that P0 #1 used as a
/// reproducer: `executor` triggers on `work.start` and publishes
/// `work.done`; `work.done` is a terminal event for the executor. The
/// `completion_promise` is intentionally distinct from `work.done` so
/// `process_parse_result` does not short-circuit `work.done` into the
/// completion-promise branch (which bypasses `validated_events` and
/// therefore the lifecycle-tracker update path).
fn build_lifecycle_config(workspace_root: &std::path::Path) -> RalphConfig {
    let yaml = r#"
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.done", "progress.update"]
    terminal_events: ["work.done"]
    instructions: "Execute work."
event_loop:
  starting_event: "work.start"
  completion_promise: "task.complete"
tasks:
  enabled: false
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).expect("parse test config");
    config.core.workspace_root = workspace_root.to_path_buf();
    config
}

/// Appends one JSONL event line to the workspace's events file.
///
/// The JSONL field name is `source` (matching `event_reader::Event`'s
/// serde model — see `crates/ralph-core/src/event_reader.rs`). Note:
/// even with `source` set, the production code path in
/// `process_parse_result` constructs `ralph_proto::Event` via
/// `Event::new(topic, payload)` (which does NOT propagate `source`).
/// The lifecycle-tracker's `complete` path falls back to
/// `last_active_hat_ids.first()` when `ralph_proto::Event::source` is
/// `None`, so we drive the test through that fallback — this is the
/// path that the P0 #1 fix was meant to keep working.
fn append_jsonl_event(events_path: &std::path::Path, topic: &str, _source: Option<&str>) {
    use std::io::Write as _;
    if let Some(parent) = events_path.parent() {
        std::fs::create_dir_all(parent).expect("create events dir");
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(events_path)
        .expect("open events.jsonl for append");
    writeln!(
        file,
        r#"{{"topic":"{}","payload":"e2e","ts":"2024-01-01T00:00:00Z"}}"#,
        topic
    )
    .expect("write event line");
    file.flush().expect("flush event line");
}

/// Build a fresh EventLoop pinned to a tempdir-shaped `.ralph/`.
fn make_loop(temp: &tempfile::TempDir) -> EventLoop {
    let config = build_lifecycle_config(temp.path());
    let ctx = LoopContext::primary(temp.path().to_path_buf());
    let mut event_loop = EventLoop::with_context(config, ctx);
    // U11 fail-closed: the lifecycle config drives work.* through
    // process_events_from_jsonl, which now routes every business topic
    // through the stage pipeline. Install a unit_loop flow that
    // admits the executor's emits.
    install_admitting_flow(
        &mut event_loop,
        &[
            "work.start",
            "work.done",
            "progress.update",
            "task.complete",
        ],
    );
    event_loop
}

/// Pre-activates the executor via the public tracker API so we can
/// isolate the close path. This matches what the production code's
/// `build_prompt` does — it calls `tracker.activate(...)` once an
/// active hat is selected. Mirroring that call here keeps the test
/// focused on the `process_events_from_jsonl` close path (P0 #1
/// regression gate).
fn preactivate_executor(event_loop: &mut EventLoop, iteration: u32, trigger_topic: &str) {
    let key = crate::hat_lifecycle::ActivationKey {
        loop_id: "primary".to_string(),
        iteration,
        hat_id: "executor".to_string(),
    };
    // Also seed last_active_hat_ids and state.iteration so the
    // complete-side's `event.source.as_ref().or(last_active_hat_ids.first())`
    // fallback resolves to "executor" and the iteration field on the
    // key matches what `process_events_from_jsonl` reads.
    event_loop.state.iteration = iteration;
    event_loop.state.last_active_hat_ids = vec![HatId::new("executor")];
    event_loop
        .hat_lifecycle_tracker_mut()
        .activate(key, trigger_topic.to_string(), None);
}

/// T-JSONL-1: A terminal event arriving via JSONL MUST close a
/// previously-activated executor activation.
///
/// This is the direct regression gate for P0 finding #1. Before the
/// fix, the activate and complete paths disagreed on `trigger_identity`
/// (`"unknown"` vs `topic_str`), so the tracker's `HashMap` lookup
/// missed and the activation leaked. After the fix the key is just
/// `(loop_id, iteration, hat_id)`, and `complete()` correctly finds the
/// stored activation via the `last_active_hat_ids` fallback.
#[test]
fn terminal_event_from_source_hat_closes_activation() {
    let temp = tempfile::tempdir().expect("tempdir");
    let events_path = temp.path().join(".ralph/events.jsonl");
    let mut event_loop = make_loop(&temp);

    // Pre-activate (mirrors what `build_prompt` does in production).
    preactivate_executor(&mut event_loop, 0, "work.start");
    assert_eq!(event_loop.hat_lifecycle_tracker().active_count(), 1);

    // Write terminal event and process through the production path.
    append_jsonl_event(&events_path, "work.done", Some("executor"));
    let _ = event_loop
        .process_events_from_jsonl()
        .expect("process_events_from_jsonl");

    // P0 #1 regression gate: after a terminal event from the source
    // hat, the activation must be closed. Before the fix this assertion
    // failed because activate/complete keys disagreed on
    // `trigger_identity`.
    assert_eq!(
        event_loop.hat_lifecycle_tracker().active_count(),
        0,
        "after work.done via JSONL, no activation may remain \
         (P0 #1 regression gate: complete() must find the activation \
         the activate() call inserted)"
    );
    assert_eq!(
        event_loop.hat_lifecycle_tracker().total_count(),
        1,
        "the closed activation must still be remembered in total_count"
    );
}

/// T-JSONL-2: A non-terminal event arriving via JSONL MUST NOT close
/// the activation; only the configured terminal event closes it.
#[test]
fn non_terminal_event_from_source_hat_keeps_activation_open() {
    let temp = tempfile::tempdir().expect("tempdir");
    let events_path = temp.path().join(".ralph/events.jsonl");
    let mut event_loop = make_loop(&temp);

    preactivate_executor(&mut event_loop, 0, "work.start");
    assert_eq!(event_loop.hat_lifecycle_tracker().active_count(), 1);

    // `progress.update` is in the executor's `publishes` list but NOT
    // in `terminal_events`, so the production code path treats it as a
    // non-terminal accepted event — must NOT close the activation.
    append_jsonl_event(&events_path, "progress.update", Some("executor"));
    let _ = event_loop
        .process_events_from_jsonl()
        .expect("process_events_from_jsonl");

    assert_eq!(
        event_loop.hat_lifecycle_tracker().active_count(),
        1,
        "non-terminal events must not close the activation"
    );

    // Now close it with the terminal event.
    append_jsonl_event(&events_path, "work.done", Some("executor"));
    let _ = event_loop
        .process_events_from_jsonl()
        .expect("process_events_from_jsonl");
    assert_eq!(
        event_loop.hat_lifecycle_tracker().active_count(),
        0,
        "terminal event must close the activation"
    );
}

/// T-JSONL-3: Many terminal-event-via-JSONL cycles must not leak
/// activations. Each cycle activates (via the public tracker API,
/// mirroring `build_prompt`) and closes (via `process_events_from_jsonl`).
/// After N cycles, `active_count` must be 0 and `total_count` must be N.
#[test]
fn many_cycles_do_not_leak_activations() {
    let temp = tempfile::tempdir().expect("tempdir");
    let events_path = temp.path().join(".ralph/events.jsonl");
    let mut event_loop = make_loop(&temp);

    let cycles = 5;
    for i in 0..cycles {
        // Mirror build_prompt: pre-activate executor for this cycle.
        // Iteration is bumped per cycle so the activation key is unique
        // (otherwise `activate` is a no-op for an existing key).
        preactivate_executor(&mut event_loop, i as u32, "work.start");
        assert_eq!(event_loop.hat_lifecycle_tracker().active_count(), 1);

        // Terminal event through the production path closes it.
        append_jsonl_event(&events_path, "work.done", Some("executor"));
        let _ = event_loop
            .process_events_from_jsonl()
            .expect("process_events_from_jsonl");

        assert_eq!(
            event_loop.hat_lifecycle_tracker().active_count(),
            0,
            "cycle {}: terminal event must close the activation \
             (P0 #1 regression gate over many cycles)",
            i
        );
    }

    assert_eq!(
        event_loop.hat_lifecycle_tracker().total_count(),
        cycles,
        "all {} cycles must be remembered in total_count",
        cycles
    );
}
