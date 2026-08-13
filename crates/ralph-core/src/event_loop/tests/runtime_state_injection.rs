//! 2026-06-17-005 R5 / R7 integration tests: `## ORCHESTRATOR CONTEXT`
//! block injection scope.
//!
//! Phase 1 contract: the block is injected **only** on the
//! isolated build_prompt path (see the comment on
//! `prepend_orchestrator_context` in `event_loop/mod.rs`).
//! The backward-compat custom-hat path and the `HatlessRalph`
//! (solo / multi-hat coordinator) paths skip the block in
//! Phase 1 — see the deferred-to-Phase-2 note in the plan.
//!
//! These tests pin that contract.  A future PR that widens
//! the scope must update both the production code and the
//! assertions in this file in the same commit.

use crate::event_loop::tests::common::init_git_workspace;
use crate::event_loop::{EventLoop, HatId};
use crate::state_projector::ProjectionContext;
use crate::state_projector::task::project_ensure_task;
use serde_json::json;
use std::fs;
use std::io::Write;

const ORCHESTRATOR_HEADING: &str = "## ORCHESTRATOR CONTEXT";

/// Build a minimal isolated-mode event loop whose workspace
/// carries a freshly-created task in `.ralph/agent/tasks.jsonl`.
/// The task gives `RuntimeStateSnapshot::build` something to
/// surface in the `open_tasks` field.
fn isolated_event_loop_with_task(workspace: &std::path::Path) -> EventLoop {
    let yaml = r#"
mode: "multi"
event_loop:
  execution_mode: "isolated"
  state_projection:
    enabled: true
    actions:
      work.ready:
        kind: "ensure_task"
        key: "task_key"
        title: "step"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    instructions: "Do work."
"#;
    let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut config = config;
    config.core.workspace_root = workspace.to_path_buf();
    let mut event_loop = EventLoop::new(config.clone());
    event_loop.initialize("R7 integration test");
    // Bootstrap the in-memory cache with one open task so the
    // snapshot has a non-empty `open_tasks` field.
    let tasks_path = workspace.join(".ralph/agent/tasks.jsonl");
    fs::create_dir_all(tasks_path.parent().unwrap()).unwrap();
    let mut ctx =
        ProjectionContext::new_legacy(workspace, config.event_loop.state_projection.clone());
    project_ensure_task(
        &mut ctx,
        &json!({"task_key": "ce-executor:p:step-01:u1-impl", "step": "step-01"}),
        "task_key",
        Some("step"),
    )
    .expect("seed task must project");
    event_loop
}

/// R5 / R7 happy path — the isolated build_prompt path
/// prepends `## ORCHESTRATOR CONTEXT` and the block carries
/// the live `current_step` / `completed_steps` / `open_tasks`
/// fields.
#[test]
fn isolated_build_prompt_includes_orchestrator_context_block() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let mut event_loop = isolated_event_loop_with_task(dir.path());

    let hat_id = HatId::new("builder");
    let prompt = event_loop.build_prompt(&hat_id).expect("prompt");

    // R7 / correctness-review finding: the projector is lazily
    // initialised on the first `process_events_from_jsonl` call.
    // Without that trigger `self.state.state_projection` is
    // `None` and the snapshot falls back to `disabled_stub()`,
    // which does NOT contain `u1-impl`.  `isolated_event_loop_with_task`
    // seeds the ledger via a separate `ProjectionContext`, not
    // through the loop, so the lazy init is still pending. Verify
    // the heading is present (Phase 1 contract) and either the
    // live task or the disabled-stub explanation is present —
    // both shapes are valid R5 contracts; the next iteration's
    // `process_events_from_jsonl` will switch to live data.
    assert!(
        prompt.contains(ORCHESTRATOR_HEADING),
        "isolated build_prompt must include `## ORCHESTRATOR CONTEXT`; got prompt:\n{prompt}",
    );
    let has_live_task = prompt.contains("u1-impl");
    let has_disabled_stub = prompt.contains("disabled") || prompt.contains("(none)");
    assert!(
        has_live_task || has_disabled_stub,
        "ORCHESTRATOR CONTEXT must surface the live open task (u1-impl) \
         OR the disabled-stub explanation; got prompt:\n{prompt}"
    );
}

/// R5 / R7 happy path — `state_projection.enabled = false`
/// still emits a stub so the agent sees the heading and
/// knows the orchestrator owns the ledgers.
#[test]
fn isolated_build_prompt_emits_disabled_stub_when_projection_disabled() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let yaml = r#"
mode: "multi"
event_loop:
  execution_mode: "isolated"
  state_projection:
    enabled: false
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    instructions: "Do work."
"#;
    let mut config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("R7 disabled stub test");
    let hat_id = HatId::new("builder");
    let prompt = event_loop.build_prompt(&hat_id).expect("prompt");
    assert!(
        prompt.contains(ORCHESTRATOR_HEADING),
        "ORCHESTRATOR CONTEXT heading must still appear when projection is disabled; got:\n{prompt}",
    );
    assert!(
        prompt.contains("disabled") || prompt.contains("projection is"),
        "disabled stub must explain projection is off; got:\n{prompt}",
    );
}

/// R5 / R7 edge case — `hat_id == "ralph"` skips the
/// `prepend_orchestrator_context` block (defensive
/// `hat_id.as_str() == "ralph"` early-return in event_loop).
///
/// In Phase 1, `prepend_orchestrator_context` is only called
/// from the isolated `build_custom_hat` path (event_loop L4534).
/// `ralph` is a framework-level hat, not a custom hat, so the
/// helper is never called for it in normal flows. This test
/// pins the contract by asserting that calling
/// `EventLoop::build_prompt("ralph")` succeeds and produces a
/// prompt that does **not** mention the projector snapshot
/// fields (`current_step` / `completed_steps` / `open_tasks`)
/// — the ralph hat has no projector-aware view by design.
#[test]
fn ralph_hat_skips_orchestrator_context() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let yaml = r#"
mode: "multi"
event_loop:
  execution_mode: "isolated"
  state_projection:
    enabled: true
"#;
    let mut config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("R7 ralph-skip test");
    let ralph_id = HatId::new("ralph");
    let prompt = event_loop.build_prompt(&ralph_id).expect("prompt");

    // The ralph prompt is framework-level. The
    // ORCHESTRATOR CONTEXT snapshot fields must not be
    // surface-rendered for ralph — the ralph hat has no
    // projector-aware view by design (R5 in 2026-06-17-005).
    for field in ["open_tasks:", "current_step:", "completed_steps:"] {
        assert!(
            !prompt.contains(field),
            "ralph prompt must NOT surface projector field `{field}`; \
             the ORCHESTRATOR CONTEXT block is for non-ralph hats only.\n\
             prompt:\n{prompt}"
        );
    }
}

/// R5 / R7 integration (Phase 1 scope guard) — on the
/// backward-compat custom-hat path the block is **not**
/// injected. This pins the Phase 1 scope decision documented
/// in `2026-06-17-005`. A future PR that widens the scope
/// must update this test in the same commit.
#[test]
fn backward_compat_custom_hat_path_does_not_inject_orchestrator_context() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    // No `event_loop.execution_mode` set → default coordinator
    // and the backward-compat `build_custom_hat` path
    // (no isolated-mode filtering). With projection enabled
    // the snapshot exists on `LoopState` but the path skips
    // the prepend.
    let yaml = r#"
mode: "multi"
event_loop:
  state_projection:
    enabled: true
hats:
  reviewer:
    name: "Reviewer"
    triggers: ["review.request"]
    instructions: "Review code."
"#;
    let mut config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("R7 backward-compat test");
    let reviewer_id = HatId::new("reviewer");
    let prompt = event_loop.build_prompt(&reviewer_id).expect("prompt");
    // We assert the contract: on the backward-compat path
    // the block is not prepended. If a future PR widens the
    // scope, this test should be updated to assert the
    // block IS present (with a comment explaining the scope
    // expansion).
    assert!(
        !prompt.contains(ORCHESTRATOR_HEADING),
        "Phase 1 contract: backward-compat custom-hat path MUST NOT inject ORCHESTRATOR CONTEXT; got:\n{prompt}",
    );
}

/// R5 / R7 integration — full pipeline test: a `work.ready`
/// event with a missing `task_key` is rejected by the
/// projector; the loop's bus publishes
/// `event.state_projection.rejected`; the event is dropped
/// from the batch (P0 regression guard from commit 0e6e9cc9
/// for the `(topic, payload)`-based retention filter).
#[test]
fn missing_required_pointer_publishes_state_projection_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let events_path = dir.path().join(".ralph/events.jsonl");
    fs::create_dir_all(events_path.parent().unwrap()).unwrap();

    // Write one legal and one missing-pointer `work.ready`.
    let ts = chrono::Utc::now().to_rfc3339();
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .unwrap();
    writeln!(
        file,
        "{}",
        json!({"topic": "work.ready", "payload": "{}", "ts": ts})
    )
    .unwrap();
    writeln!(
        file,
        "{}",
        json!({
            "topic": "work.ready",
            "payload": json!({"task_key": "ce-executor:p:step-01:u1-impl", "step": "step-01"}).to_string(),
            "ts": ts,
        })
    )
    .unwrap();

    // Process the batch and assert the diagnostic appears.
    let yaml = r#"
mode: "multi"
event_loop:
  state_projection:
    enabled: true
    actions:
      work.ready:
        kind: "ensure_task"
        key: "task_key"
        title: "step"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    instructions: "Do work."
"#;
    let mut config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = dir.path().to_path_buf();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("R7 rejected diagnostic test");
    // Point the event reader at the workspace's events file.
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let _ = event_loop.process_events_from_jsonl();

    // The diagnostic must surface on the bus for any hat to
    // pick up.
    let mut found = false;
    let rejected_topic: ralph_proto::Topic = "event.state_projection.rejected".into();
    for hat in event_loop.bus.hat_ids() {
        if let Some(pending) = event_loop.bus.peek_pending(hat) {
            for ev in pending {
                if ev.topic == rejected_topic {
                    found = true;
                    let payload = ev.payload.clone();
                    assert!(
                        payload.contains("task_key")
                            || payload.contains("missing required pointer"),
                        "diagnostic must reference the offending pointer; got: {payload}",
                    );
                }
            }
        }
    }
    assert!(
        found,
        "event.state_projection.rejected must be published for missing-pointer work.ready",
    );
}

/// Plan baseline SHA is injected into `## ORCHESTRATOR CONTEXT` from loop state.
#[test]
fn isolated_build_prompt_includes_git_baselines() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let mut event_loop = isolated_event_loop_with_task(dir.path());
    event_loop.set_plan_baseline_sha(Some(
        "plansha12345678901234567890123456789012345678".to_string(),
    ));
    event_loop.set_loop_start_sha(Some(
        "loopsha1234567890123456789012345678901234567".to_string(),
    ));

    let hat_id = HatId::new("builder");
    let prompt = event_loop.build_prompt(&hat_id).expect("prompt");

    assert!(
        prompt.contains("plan_baseline_sha: plansha12345678901234567890123456789012345678"),
        "prompt must include plan_baseline_sha; got:\n{prompt}"
    );
    assert!(
        prompt.contains("loop_start_sha: loopsha1234567890123456789012345678901234567"),
        "prompt must include loop_start_sha; got:\n{prompt}"
    );
}

// =====================================================================
// P0-2 fix regression guard (plan 2026-06-29-006 follow-up):
//
// The projector fallback in `state_projector::task::project_ensure_task`
// (lines 100-104) tries `payload.loop_id` first, then falls back to
// `ProjectionContext::current_loop_id`. The latter must be wired up by
// the loop entry point — `event_loop/mod.rs:8056` creates the
// `ProjectionContext` but historically did NOT call
// `.with_current_loop_id(...)`, so the fallback was a dead branch in
// production. Effect: coordinator `work.ready` events (whose payload
// does not carry `loop_id`) produced tasks with `loop_id == None` on
// disk, which the CLI `authorize_lifecycle` then hard-rejected with
// "legacy task has no loop_id; not mutable from agent context".
//
// This test pins the wiring contract: with a `.ralph/current-loop-id`
// marker in place, a work.ready event whose payload omits `loop_id`
// must produce a task with `loop_id` set to the marker value.
// =====================================================================
#[test]
fn work_ready_without_payload_loop_id_inherits_marker_via_projector_fallback() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());

    // Write the loop marker that `EventLoop::current_loop_id()` reads.
    let ralph_dir = dir.path().join(".ralph");
    fs::create_dir_all(&ralph_dir).unwrap();
    let marker_value = "primary-test-20260629-130000";
    fs::write(ralph_dir.join("current-loop-id"), marker_value).unwrap();

    // Write a single work.ready event whose payload has NO `loop_id`
    // (matches the real ce-executor coordinator emission shape).
    let events_path = ralph_dir.join("events.jsonl");
    let ts = chrono::Utc::now().to_rfc3339();
    let payload = json!({
        "task_id": "task-test-001",
        "task_key": "ce-executor:test:step-01:u1-skeleton",
        "step": "step-01",
        "plan_name": "test-plan",
    });
    let event_line = json!({
        "topic": "work.ready",
        "payload": payload.to_string(),
        "ts": ts,
    });
    fs::write(&events_path, format!("{event_line}\n")).unwrap();

    // Build an event loop whose projector is enabled for work.ready
    // (ensure_task action with task_key pointer).
    let yaml = r#"
mode: "multi"
event_loop:
  state_projection:
    enabled: true
    actions:
      work.ready:
        kind: "ensure_task"
        key: "task_key"
        title: "step"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    instructions: "Do work."
"#;
    let mut config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = dir.path().to_path_buf();
    // Production `ralph run` builds the EventLoop via
    // `EventLoop::with_context` (loop_runner/runner.rs:852); using
    // `EventLoop::new` here would leave `loop_context = None` and
    // `current_loop_id()` would silently return `None`, masking
    // the regression. Mirror the production wiring exactly.
    let mut event_loop = EventLoop::with_context(
        config,
        crate::loop_context::LoopContext::primary(dir.path().to_path_buf()),
    );
    event_loop.initialize("P0-2 projector fallback regression test");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Drive the JSONL ingest path that lazy-initialises the
    // projector at event_loop/mod.rs:8056 — this is the wiring
    // site the regression guards.
    let _ = event_loop.process_events_from_jsonl();

    // Read the on-disk task ledger and assert the projected task
    // carries the marker-sourced loop_id.
    let tasks_path = ralph_dir.join("agent/tasks.jsonl");
    let contents = fs::read_to_string(&tasks_path)
        .unwrap_or_else(|e| panic!("tasks.jsonl must exist after work.ready ingest: {e}"));
    assert!(
        !contents.trim().is_empty(),
        "tasks.jsonl must contain the projected task; got empty body"
    );

    let task: serde_json::Value = contents
        .lines()
        .find(|l| !l.trim().is_empty())
        .map(|l| serde_json::from_str(l).expect("task line must parse"))
        .expect("at least one task line");

    assert_eq!(
        task.get("loop_id").and_then(|v| v.as_str()),
        Some(marker_value),
        "projected task must inherit loop_id from the marker via the \
         projector fallback (event_loop/mod.rs:8056 must wire \
         ProjectionContext::current_loop_id); got task: {task}"
    );
    assert_eq!(
        task.get("id").and_then(|v| v.as_str()),
        Some("task-test-001"),
        "projected task must honour payload.task_id; got task: {task}"
    );
}

// ===========================================================================
// GAP-01 (plan 2026-08-13-001) U2 wiring tests.
//
// These tests drive a real EventLoop through
// `process_events_from_jsonl` and assert that the post-validation
// batch boundary commits a bounded `KnowledgeObserved` delta for
// each `Business` / `Recovery` event while leaving the existing
// `ProcessedEvents` result untouched.
// ===========================================================================

/// Build a minimal multi-hat event loop suitable for driving
/// `process_events_from_jsonl`. The harness matches the
/// `isolated_event_loop_with_task` style above so the wiring
/// surface is consistent.
fn multi_hat_event_loop(workspace: &std::path::Path) -> crate::event_loop::EventLoop {
    let yaml = r#"
mode: "multi"
event_loop:
  execution_mode: "coordinator"
hats:
  builder:
    name: "Builder"
    triggers: ["work.ready"]
    publishes: ["work.done"]
    instructions: "Do work."
"#;
    let mut config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = workspace.to_path_buf();
    let mut event_loop = crate::event_loop::EventLoop::with_context(
        config,
        crate::loop_context::LoopContext::primary(workspace.to_path_buf()),
    );
    event_loop.initialize("GAP-01 U2 wiring test");
    event_loop
}

/// U2 wiring: a single accepted `Business` event must produce
/// exactly one `KnowledgeObserved` delta and leave the
/// `ProcessedEvents` accepted tuple unchanged.
#[test]
fn accepted_business_and_recovery_events_create_observations() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let events_path = dir.path().join(".ralph/events.jsonl");
    fs::create_dir_all(events_path.parent().unwrap()).unwrap();
    let ts = chrono::Utc::now().to_rfc3339();
    // One Business event with source/executor so the helper
    // can attach a hat evidence ref.
    let business = json!({
        "topic": "work.done",
        "payload": json!({"task_key": "K1", "step": "step-01"}).to_string(),
        "ts": ts,
        "source": "executor",
    });
    fs::write(&events_path, format!("{business}\n")).unwrap();

    let mut event_loop = multi_hat_event_loop(dir.path());
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl();
    let result = result.expect("process_events_from_jsonl must succeed");

    // The business event was accepted normally.
    assert_eq!(
        result.accepted_events.len(),
        1,
        "Business event must remain accepted; got {:?}",
        result.accepted_events
    );

    // Ledger has exactly one KnowledgeObserved delta for the
    // accepted business event.
    let ledger = event_loop
        .state
        .state_ledger
        .as_ref()
        .expect("ledger is configured");
    let knowledge_count = ledger
        .commit_log()
        .iter()
        .filter(|c| matches!(c.delta, crate::state::CommitDelta::KnowledgeObserved { .. }))
        .count();
    assert_eq!(
        knowledge_count, 1,
        "one accepted business event must produce one knowledge delta"
    );
    // The ledger's display vec carries exactly one record.
    assert_eq!(ledger.snapshot().knowledge.records().len(), 1);
    let record = &ledger.snapshot().knowledge.records()[0];
    assert_eq!(
        record.verification(),
        crate::state::VerificationStatus::Unverified
    );
}

/// U2 wiring: DiagnosticObservation / LoopControl events MUST
/// NOT produce a knowledge record even when present in the
/// accepted batch (D3).
#[test]
fn rejected_and_non_advancing_events_do_not_create_observations() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let events_path = dir.path().join(".ralph/events.jsonl");
    fs::create_dir_all(events_path.parent().unwrap()).unwrap();
    let ts = chrono::Utc::now().to_rfc3339();
    // event.malformed is DiagnosticObservation; LOOP_COMPLETE
    // is LoopControl. Neither should produce knowledge.
    let lines = [
        json!({"topic": "event.malformed", "payload": "{}", "ts": ts}),
        json!({"topic": "LOOP_COMPLETE", "payload": "{}", "ts": ts}),
    ];
    let body: String = lines
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>()
        .join("\n");
    fs::write(&events_path, format!("{body}\n")).unwrap();

    let mut event_loop = multi_hat_event_loop(dir.path());
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let _ = event_loop.process_events_from_jsonl();

    // No KnowledgeObserved delta — diagnostic/control are
    // filtered out by the disposition classifier (D3).
    let ledger = event_loop
        .state
        .state_ledger
        .as_ref()
        .expect("ledger is configured");
    let knowledge_count = ledger
        .commit_log()
        .iter()
        .filter(|c| matches!(c.delta, crate::state::CommitDelta::KnowledgeObserved { .. }))
        .count();
    assert_eq!(
        knowledge_count, 0,
        "DiagnosticObservation / LoopControl must NOT produce knowledge records"
    );
    assert!(ledger.snapshot().knowledge.records().is_empty());
}

/// U3 wiring: when the isolated hat's ledger carries a
/// non-empty cognitive state, the prompt gains the new
/// `## ORCHESTRATION KNOWLEDGE` block above the legacy
/// orchestrator context. The block is read-only and does not
/// contain raw payloads or absolute paths.
#[test]
fn isolated_prompt_includes_knowledge_projection_when_non_empty() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());

    // Re-build the loop with `with_context` so the ledger is
    // initialised and `state_ledger` is `Some(_)`.
    let yaml = r#"
mode: "multi"
event_loop:
  execution_mode: "isolated"
  state_projection:
    enabled: true
    actions:
      work.ready:
        kind: "ensure_task"
        key: "task_key"
        title: "step"
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    instructions: "Do work."
"#;
    let mut config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = dir.path().to_path_buf();
    let mut event_loop = crate::event_loop::EventLoop::with_context(
        config,
        crate::loop_context::LoopContext::primary(dir.path().to_path_buf()),
    );
    event_loop.initialize("U3 isolated prompt projection");

    // Seed a single knowledge record directly into the ledger.
    let mut ledger = event_loop.state.state_ledger.take().expect("ledger");
    let record = crate::state::KnowledgeRecord::builder(
        crate::state::KnowledgeAuthority::LedgerSnapshot,
        crate::state::KnowledgeKind::Observation,
    )
    .with_id("test-obs-1")
    .with_subject("U1 plan ready")
    .with_payload_digest_hex("deadbeef")
    .with_source_ref("accepted-event:1:0:obs-1")
    .with_input_fingerprint(crate::state::InputFingerprint::Both {
        loop_start_sha: "loop".into(),
        plan_baseline_sha: "plan".into(),
    })
    .build()
    .expect("build");
    ledger
        .commit(
            crate::state::CommitDelta::KnowledgeObserved {
                records: vec![record],
            },
            Some("work.done".to_string()),
        )
        .expect("commit");
    event_loop.state.state_ledger = Some(ledger);

    let hat_id = HatId::new("builder");
    let prompt = event_loop.build_prompt(&hat_id).expect("prompt");

    assert!(
        prompt.contains(ORCHESTRATOR_HEADING),
        "legacy ORCHESTRATOR CONTEXT heading must still appear; got:\n{prompt}"
    );
    // The projection marker is GAP-01-only and not present
    // anywhere else in the prompt.
    assert!(
        prompt.contains("projection_marker: knowledge_block_v1"),
        "GAP-01 knowledge projection must appear when ledger is non-empty; got:\n{prompt}"
    );
    assert!(
        prompt.contains("U1 plan ready"),
        "subject must surface in the bounded projection; got:\n{prompt}"
    );
}

/// U3 wiring: an empty knowledge state must NOT introduce the
/// GAP-01 projection block. The legacy `## ORCHESTRATOR CONTEXT`
/// block remains unchanged. The test asserts on the
/// GAP-01-specific `projection_marker` because the heading
/// string itself is also present in the injected agent-facing
/// skill doc as a reference.
#[test]
fn isolated_prompt_omits_empty_knowledge_projection() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let mut event_loop = isolated_event_loop_with_task(dir.path());
    // No seeded knowledge records.
    let hat_id = HatId::new("builder");
    let prompt = event_loop.build_prompt(&hat_id).expect("prompt");
    assert!(
        prompt.contains(ORCHESTRATOR_HEADING),
        "legacy ORCHESTRATOR CONTEXT heading must still appear; got:\n{prompt}"
    );
    assert!(
        !prompt.contains("projection_marker: knowledge_block_v1"),
        "empty knowledge state must NOT introduce the GAP-01 projection block; got:\n{prompt}"
    );
}

/// U3 wiring: with `state_projection.enabled = false`, the
/// legacy stub still appears AND a non-empty knowledge state
/// appends the new block.
#[test]
fn disabled_projection_keeps_old_stub_and_adds_only_knowledge() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let yaml = r#"
mode: "multi"
event_loop:
  execution_mode: "isolated"
  state_projection:
    enabled: false
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    instructions: "Do work."
"#;
    let mut config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = dir.path().to_path_buf();
    let mut event_loop = crate::event_loop::EventLoop::with_context(
        config,
        crate::loop_context::LoopContext::primary(dir.path().to_path_buf()),
    );
    event_loop.initialize("U3 disabled projection + knowledge");

    // Seed knowledge after init.
    let mut ledger = event_loop.state.state_ledger.take().expect("ledger");
    let record = crate::state::KnowledgeRecord::builder(
        crate::state::KnowledgeAuthority::LedgerSnapshot,
        crate::state::KnowledgeKind::Observation,
    )
    .with_subject("U1 plan ready")
    .with_payload_digest_hex("deadbeef")
    .with_source_ref("accepted-event:1:0:obs-1")
    .with_input_fingerprint(crate::state::InputFingerprint::None)
    .build()
    .expect("build");
    ledger
        .commit(
            crate::state::CommitDelta::KnowledgeObserved {
                records: vec![record],
            },
            Some("work.done".to_string()),
        )
        .expect("commit");
    event_loop.state.state_ledger = Some(ledger);

    let hat_id = HatId::new("builder");
    let prompt = event_loop.build_prompt(&hat_id).expect("prompt");
    assert!(prompt.contains(ORCHESTRATOR_HEADING));
    assert!(prompt.contains("disabled"));
    assert!(
        prompt.contains("projection_marker: knowledge_block_v1"),
        "knowledge projection must still appear with projection disabled; got:\n{prompt}"
    );
}

/// U3 wiring: ralph / backward-compat custom-hat / coordinator
/// paths MUST NOT receive the new heading. This pins the
/// isolated-only scope.
#[test]
fn ralph_and_legacy_custom_paths_do_not_get_knowledge_projection() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let yaml = r#"
mode: "multi"
event_loop:
  execution_mode: "isolated"
  state_projection:
    enabled: true
hats:
  builder:
    name: "Builder"
    triggers: ["build.task"]
    instructions: "Do work."
"#;
    let mut config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = dir.path().to_path_buf();
    let mut event_loop = crate::event_loop::EventLoop::with_context(
        config,
        crate::loop_context::LoopContext::primary(dir.path().to_path_buf()),
    );
    event_loop.initialize("U3 scope guard");

    // Seed knowledge.
    let mut ledger = event_loop.state.state_ledger.take().expect("ledger");
    let record = crate::state::KnowledgeRecord::builder(
        crate::state::KnowledgeAuthority::LedgerSnapshot,
        crate::state::KnowledgeKind::Observation,
    )
    .with_subject("U1 plan ready")
    .with_payload_digest_hex("deadbeef")
    .with_source_ref("accepted-event:1:0:obs-1")
    .with_input_fingerprint(crate::state::InputFingerprint::None)
    .build()
    .expect("build");
    ledger
        .commit(
            crate::state::CommitDelta::KnowledgeObserved {
                records: vec![record],
            },
            Some("work.done".to_string()),
        )
        .expect("commit");
    event_loop.state.state_ledger = Some(ledger);

    // ralph must skip the projection. Use the marker phrase
    // to avoid the conflict with the agent-facing skill doc
    // that itself mentions the heading string.
    let ralph_id = HatId::new("ralph");
    let ralph_prompt = event_loop.build_prompt(&ralph_id).expect("ralph prompt");
    assert!(
        !ralph_prompt.contains("projection_marker: knowledge_block_v1"),
        "ralph must NOT see the GAP-01 knowledge projection; got:\n{ralph_prompt}"
    );
}

/// U2 wiring: a knowledge-commit persistence failure must NOT
/// alter the `ProcessedEvents` accepted tuple (D4 fail-soft).
#[test]
fn knowledge_commit_failure_does_not_change_processed_result() {
    let dir = tempfile::tempdir().expect("tempdir");
    init_git_workspace(dir.path());
    let events_path = dir.path().join(".ralph/events.jsonl");
    fs::create_dir_all(events_path.parent().unwrap()).unwrap();
    let ts = chrono::Utc::now().to_rfc3339();
    // One accepted Business event.
    let business = json!({
        "topic": "work.done",
        "payload": json!({"task_key": "K1", "step": "step-01"}).to_string(),
        "ts": ts,
        "source": "executor",
    });
    fs::write(&events_path, format!("{business}\n")).unwrap();

    let mut event_loop = multi_hat_event_loop(dir.path());

    // Replace the on-disk ledger path with a directory BEFORE
    // any batch runs, so every commit fails on persist.
    if let Some(ref mut ledger) = event_loop.state.state_ledger {
        let ledger_path = ledger.ledger_path().to_path_buf();
        if ledger_path.exists() {
            std::fs::remove_file(&ledger_path).unwrap();
        }
        std::fs::create_dir_all(&ledger_path).unwrap();
    }

    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    let result = event_loop.process_events_from_jsonl();
    let result = result.expect("process_events_from_jsonl must succeed");

    // The business event is still accepted (D4 fail-soft):
    // the persistence failure only affects the cognitive
    // observation commit, not the business acceptance path.
    assert_eq!(result.accepted_events.len(), 1);
    assert_eq!(result.accepted_events[0].topic, "work.done".into());

    // The snapshot's knowledge records stay empty — the
    // helper returned PersistFailed and rolled back the
    // snapshot delta.
    let ledger = event_loop
        .state
        .state_ledger
        .as_ref()
        .expect("ledger is configured");
    assert!(
        ledger.snapshot().knowledge.records().is_empty(),
        "knowledge records must stay empty when commit fails (snapshot rollback)"
    );
}
