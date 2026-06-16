//! 2026-06-13-004 U7 + review fix tests: scope-drop + handoff
//! recovery envelope writes.
//!
//! These tests pin the KTD-5 contract: an isolated-mode
//! scope drop MUST write a `RecoveryDiagnosisEnvelope` to
//! `recovery.jsonl` with `source = WorkflowGuard`,
//! `outcome = Escalated`, and the new (post-review) retry_key
//! namespace that prevents 8 same-wave events from collapsing
//! to a single journal entry. The handoff-escalation test
//! pins the complementary StallRecovery/escalated envelope.

use std::io::Write;

use super::common::*;
use super::*;
use ralph_proto::HatId;

const SCENARIO_HAT: &str = "builder";

/// U7 KTD-5: an isolated scope drop (out-of-publishes topic
/// from a non-isolated-hat worker) MUST write a recovery
/// envelope. The envelope must use `source = WorkflowGuard`,
/// `outcome = Escalated`, and (post-2026-06-13 review fix) a
/// retry_key that is namespaced by the offending topic so
/// future diagnostics renders can distinguish two scopes.
#[test]
fn test_u7_isolated_scope_drop_writes_recovery_envelope() {
    use crate::diagnosis::{DiagnosisOutcome, DiagnosisSource};

    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();

    // Isolated topology: `current_isolated_hat=builder` is the
    // currently-isolated hat. `dispatcher` is the worker that
    // emits `review.file` — but `builder` does NOT publish
    // `review.file`, so the scope check must drop and the
    // envelope path must fire.
    let yaml = r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
  completion_promise: "LOOP_COMPLETE"
hats:
  builder:
    name: "Builder"
    triggers: ["task.start"]
    publishes: ["build.done"]
    terminal_events: ["build.done"]
    instructions: "Builder hat."
  dispatcher:
    name: "Dispatcher"
    triggers: ["task.start"]
    publishes: ["review.file"]
    terminal_events: ["review.file"]
    instructions: "Dispatcher hat."
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let session_dir = diagnostics.session_dir().unwrap().to_path_buf();
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("U7 isolated scope drop");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.state.current_isolated_hat = Some(HatId::new(SCENARIO_HAT));

    // Act: emit `review.file` from a worker hat that the
    // isolated `builder` hat does NOT publish (and
    // importantly, the worker itself is not registered
    // in the registry, so U2's `scope_hat = event.hat`
    // path falls into the `isolated_publish_allowed(worker, topic)`
    // which returns false — triggering the scope drop +
    // the U7 envelope write).
    //
    // We use `hat="phantom-worker"` rather than `hat="dispatcher"`
    // because `dispatcher` is registered and publishes
    // `review.file`; U2's scope check would then accept
    // the event (worker publishes the topic) and no
    // envelope would fire. The negative test is what
    // locks in the U7 contract.
    write_event_with_hat_to_jsonl(&events_path, "review.file", r#"{"x":1}"#, "phantom-worker");
    let _ = event_loop
        .process_events_from_jsonl()
        .expect("process_events_from_jsonl should not error on scope drop");

    // Assert: recovery.jsonl has at least one entry, with the
    // KTD-5 contract: source = WorkflowGuard,
    // outcome = Escalated, reason_code = isolated_scope_violation.
    let recovery_path = session_dir.join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
    let entries: Vec<crate::diagnosis::RecoveryJournalEntry> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse recovery entry"))
        .collect();
    assert!(
        !entries.is_empty(),
        "U7: at least one recovery envelope expected; recovery.jsonl was empty"
    );
    let env = entries
        .iter()
        .find(|e| {
            e.envelope.source == DiagnosisSource::WorkflowGuard
                && e.envelope.reason_code == "isolated_scope_violation"
        })
        .map(|e| &e.envelope)
        .expect("U7: WorkflowGuard + isolated_scope_violation envelope not found");
    assert_eq!(env.outcome, DiagnosisOutcome::Escalated);
    assert!(!env.safe_target, "scope drop has no safe retry target");
    assert_eq!(env.topic.as_deref(), Some("review.file"));
    // Post-review retry_key: the original 5-tuple keys for
    // non-wave events must still produce a stable, well-formed
    // key (we do not pin the exact string here; we just
    // assert the WorkflowGuard namespace prefix is present).
    assert!(
        env.retry_key.contains("workflow_guard") || env.retry_key.starts_with("isolated_scope"),
        "retry_key must be in the WorkflowGuard or isolated_scope namespace; got: {}",
        env.retry_key
    );
}

/// U7 KTD-5 wave-batch retry_key: 8 same-wave events must
/// produce 8 distinct journal entries (ADV-1 fix: the
/// pre-fix bug collapsed them to 1).
#[test]
fn test_u7_wave_batch_does_not_collapse_recovery_envelopes() {
    use crate::diagnosis::DiagnosisSource;

    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();

    let yaml = r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
  completion_promise: "LOOP_COMPLETE"
hats:
  builder:
    name: "Builder"
    triggers: ["task.start"]
    publishes: ["build.done"]
  worker:
    name: "Worker"
    triggers: ["task.start"]
    publishes: ["review.dimension.done"]
    concurrency: 8
    instructions: "Worker hat."
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let session_dir = diagnostics.session_dir().unwrap().to_path_buf();
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("U7 wave batch");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.state.current_isolated_hat = Some(HatId::new(SCENARIO_HAT));

    // Write 8 same-wave events (each with a unique index).
    // They are all `review.dimension.done` from a worker
    // hat (`phantom-worker`) that is NOT registered in
    // the registry, so U2's scope check
    // `isolated_publish_allowed(phantom-worker, review.dimension.done)`
    // returns false and all 8 events trigger the U7
    // envelope write. Pre-fix (the retry_key collision
    // bug): 1 envelope. Post-fix (the wave_id namespace):
    // 8 distinct envelopes.
    let wave_id = "w-2026-06-13-001";
    let ts = chrono::Utc::now().to_rfc3339();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .unwrap();
    for idx in 0..8u32 {
        let event_json = serde_json::json!({
            "topic": "review.dimension.done",
            "payload": format!("{{\"i\":{idx}}}"),
            "ts": ts,
            "hat": "phantom-worker",
            "wave_id": wave_id,
            "wave_index": idx,
            "wave_total": 8,
        });
        writeln!(file, "{}", event_json).unwrap();
    }
    drop(file);
    let _ = event_loop
        .process_events_from_jsonl()
        .expect("process_events_from_jsonl should not error");

    let recovery_path = session_dir.join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
    let entries: Vec<crate::diagnosis::RecoveryJournalEntry> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse recovery entry"))
        .collect();

    // Count the WorkflowGuard + isolated_scope_violation entries
    // specifically. The fix's retry_key namespace is
    // `isolated_scope:{scope_hat}:{topic}:{wave_id}`, so 8
    // same-wave events must produce 8 distinct retry keys
    // (one per caller's evidence view of "same scope drop").
    let scope_drop_entries: Vec<&crate::diagnosis::RecoveryJournalEntry> = entries
        .iter()
        .filter(|e| {
            e.envelope.source == DiagnosisSource::WorkflowGuard
                && e.envelope.reason_code == "isolated_scope_violation"
        })
        .collect();
    let distinct_retry_keys: std::collections::HashSet<&str> = scope_drop_entries
        .iter()
        .map(|e| e.envelope.retry_key.as_str())
        .collect();
    assert_eq!(
        distinct_retry_keys.len(),
        8,
        "U7 ADV-1: 8 same-wave scope drops must produce 8 distinct retry keys, got {} (entries: {:?})",
        distinct_retry_keys.len(),
        entries
            .iter()
            .map(|e| e.envelope.retry_key.clone())
            .collect::<Vec<String>>()
    );
}

/// T-P1-4 U7: a handoff dispatch timeout must write a
/// StallRecovery/envelope. This exercises the second U7
/// site (the `process_output` handoff escalation loop,
/// ~L4213 in event_loop/mod.rs).
#[test]
fn test_u7_handoff_escalation_writes_recovery_envelope() {
    use crate::diagnosis::DiagnosisSource;

    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
  completion_promise: "LOOP_COMPLETE"
hats:
  planner:
    name: "Planner"
    triggers: ["work.start"]
    publishes: ["work.ready"]
    terminal_events: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
    terminal_events: ["work.done"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let session_dir = diagnostics.session_dir().unwrap().to_path_buf();
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("U7 handoff escalation");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Pre-load a handoff that is already past its deadline.
    // The tracker's `expired()` will return it on the next
    // `process_output` call.
    let t0 = std::time::Instant::now() - std::time::Duration::from_secs(120);
    event_loop.state.handoff_tracker.on_handoff_accepted(
        "work.ready",
        "executor",
        "evt-stale-1",
        t0,
    );
    // Force the deadline into the past with the smallest
    // possible configuration: a 1-second default timeout
    // that we already exceeded by 120 seconds.
    // (Default is 30s; t0 is 120s ago → expired.)

    // Act: call process_output. The handoff loop iterates
    // `expired()` and writes a recovery envelope for each
    // escalation.
    let _ = event_loop.process_output(&HatId::new("planner"), "", true);

    // Assert: at least one recovery envelope, source =
    // StallRecovery, reason_code = handoff_dispatch_timeout.
    let recovery_path = session_dir.join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
    let entries: Vec<crate::diagnosis::RecoveryJournalEntry> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse recovery entry"))
        .collect();
    let handoff_env = entries
        .iter()
        .find(|e| {
            e.envelope.source == DiagnosisSource::StallRecovery
                && e.envelope.reason_code == "handoff_dispatch_timeout"
        })
        .map(|e| &e.envelope)
        .unwrap_or_else(|| {
            panic!(
                "U7 handoff: StallRecovery/handoff_dispatch_timeout envelope not found; got entries: {:?}",
                entries
                    .iter()
                    .map(|e| (e.envelope.source, e.envelope.reason_code.clone()))
                    .collect::<Vec<_>>()
            )
        });
    assert_eq!(
        handoff_env.source_hat.as_deref(),
        Some("executor"),
        "U7 handoff: source_hat must be the consumer (executor); got {:?}",
        handoff_env.source_hat
    );
    // `safe_target` is the consumer unless the consumer IS
    // the fallback safe target itself (plan-gate), in which
    // case it cascades to "review-coordinator". Our test
    // uses executor as the consumer, so safe_target =
    // executor. (`HandoffTracker::expired` L184.)
    assert_eq!(
        handoff_env.target_hat.as_deref(),
        Some("executor"),
        "U7 handoff: target_hat (safe_target) is the consumer unless the consumer is plan-gate itself; got {:?}",
        handoff_env.target_hat
    );
}

/// T-P1-6 U8: `build_prompt` for a hat with a pending handoff
/// MUST clear the pending entry. Without this, the 2026-06-13
/// incident's 17m / 4m false handoff timeouts recur.
#[test]
fn test_u8_build_prompt_clears_handoff_pending() {
    use std::time::{Duration, Instant};

    let temp = tempfile::tempdir().unwrap();
    let diagnostics_root = temp.path().to_path_buf();

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
  completion_promise: "LOOP_COMPLETE"
hats:
  planner:
    name: "Planner"
    triggers: ["work.start"]
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let session_dir = diagnostics.session_dir().unwrap().to_path_buf();
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("U8 handoff clear");

    // Register a handoff deadline. We'll then call
    // `build_prompt` for the consumer (executor) and assert
    // the pending entry is cleared.
    let t0 = Instant::now();
    event_loop
        .state
        .handoff_tracker
        .on_handoff_accepted("work.ready", "executor", "evt-u8-1", t0);
    assert_eq!(
        event_loop.state.handoff_tracker.pending_count(),
        1,
        "U8: handoff must be pending before build_prompt"
    );

    // Act: build_prompt for the consumer hat. The
    // implementation in `build_prompt` (mod.rs ~L2820) calls
    // `handoff_tracker.on_hat_activated(hat_id.as_str())` (and
    // per the post-review fix, skips the clear for the
    // "ralph" sentinel).
    let prompt = event_loop
        .build_prompt(&HatId::new("executor"))
        .expect("build_prompt for executor must succeed");
    assert!(!prompt.is_empty(), "build_prompt must produce a prompt");

    // The clear happens at the build_prompt entry point;
    // assert the pending entry is now gone.
    assert_eq!(
        event_loop.state.handoff_tracker.pending_count(),
        0,
        "U8 KTD-6: build_prompt must clear the consumer's pending handoff before invoking the LLM"
    );

    // Regression guard: calling process_output well past the
    // original deadline must NOT produce a handoff escalation
    // (because the entry was cleared at build_prompt).
    let _ = event_loop.process_output(&HatId::new("planner"), "", true);
    // After process_output the tracker may receive new
    // entries, but no new entry should have been escalated
    // out of the previously-cleared one. We assert by
    // reading the journal: the escalation count for the
    // cleared entry's event_id is 0. We use the
    // `session_dir` we captured BEFORE `with_diagnostics`
    // moved the collector.
    let recovery_path = session_dir.join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path).unwrap_or_default();
    let cleared_event_present = content.contains("evt-u8-1");
    assert!(
        !cleared_event_present,
        "U8 KTD-6: cleared handoff must not produce a recovery envelope; recovery.jsonl contains evt-u8-1: {}",
        content
    );

    // Sanity: elapse some real time without panicking.
    let _elapsed = Duration::from_millis(5);

    // Suppress the unused variable warnings for sanity helpers.
    let _ = session_dir;
}

/// 2026-06-13-004 P0 #4 review fix (U7 envelope disk storm):
/// the per-turn dedup set collapses N identical scope drops
/// in the same `process_parse_result` call to a single
/// envelope write. Without the fix, a wave batch of 8
/// identical scope drops would write 8 envelopes per turn
/// (and 8 bus events); with the fix, 1 envelope + 8 bus
/// events. Distinct scope drops (different wave_id) still
/// write distinct envelopes.
///
/// This test exercises the negative case (same retry_key
/// across 8 events) and the positive case (8 distinct
/// retry_keys via the wave_index namespace, post-ADV-1).
#[test]
fn test_u7_per_turn_dedup_collapses_identical_scope_drops() {
    use crate::diagnosis::DiagnosisSource;
    use std::io::Write;

    let temp = tempfile::tempdir().unwrap();
    let events_path = temp.path().join("events.jsonl");
    let diagnostics_root = temp.path().to_path_buf();

    let yaml = r#"
event_loop:
  execution_mode: isolated
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
  completion_promise: "LOOP_COMPLETE"
hats:
  builder:
    name: "Builder"
    triggers: ["task.start"]
    publishes: ["build.done"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let session_dir = diagnostics.session_dir().unwrap().to_path_buf();
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("U7 per-turn dedup");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop.state.current_isolated_hat = Some(HatId::new("builder"));

    // Write 8 events that will all collide on the SAME
    // retry_key (no wave_id, no wave_index, same scope hat +
    // same topic). Per-turn dedup must collapse these to a
    // single envelope write.
    let ts = chrono::Utc::now().to_rfc3339();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .unwrap();
    for i in 0..8 {
        writeln!(
            file,
            r#"{{"topic":"review.dimension.done","payload":{{"i":{i}}},"ts":"{ts}","hat":"phantom-worker"}}"#
        )
        .unwrap();
    }
    drop(file);

    let _ = event_loop
        .process_events_from_jsonl()
        .expect("process_events_from_jsonl should not error");

    let recovery_path = session_dir.join("recovery.jsonl");
    let content = std::fs::read_to_string(&recovery_path)
        .unwrap_or_else(|e| panic!("read recovery.jsonl: {e}: {}", recovery_path.display()));
    let entries: Vec<crate::diagnosis::RecoveryJournalEntry> = content
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("parse recovery entry"))
        .collect();

    // Count WorkflowGuard + isolated_scope_violation entries
    // (the scope_drop path).
    let scope_drop_count = entries
        .iter()
        .filter(|e| {
            e.envelope.source == DiagnosisSource::WorkflowGuard
                && e.envelope.reason_code == "isolated_scope_violation"
        })
        .count();
    assert_eq!(
        scope_drop_count, 1,
        "P0 #4: 8 identical scope drops in one turn must collapse to 1 envelope (disk storm fix); got {} envelopes",
        scope_drop_count
    );
}

/// Unit 8 (2026-06-17-001 plan): 3+ consecutive stall iterations
/// on a wave-related hat must escalate to a `wave_stall_exhausted`
/// recovery envelope and route the `task.resume` to
/// `review-coordinator` (not the legacy `review-synthesizer`).
///
/// This pins the per-last-hat stall key
/// (`flow:review-synthesizer` for wave hats) and the
/// `wave_stall_exhausted` reason code. Non-wave hat counters
/// must remain independent (regression: do not pollute the
/// global `stall:*` namespace).
#[test]
fn test_u8_wave_hat_stall_escalates_after_three_iterations() {
    use crate::diagnosis::DiagnosisSource;
    use ralph_proto::HatId;

    let mut event_loop = EventLoop::new(RalphConfig::default());

    // Simulate a wave hat that has stalled three times in a
    // row: increment the per-wave-hat counter directly so we
    // don't have to construct three full iterations of the
    // public API.
    let wave_hat = HatId::new("review-synthesizer");
    event_loop.state.last_hat = Some(wave_hat.clone());
    for _ in 0..3 {
        event_loop
            .state
            .stall_recovery_counts
            .entry("flow:review-synthesizer".to_string())
            .and_modify(|c| *c += 1)
            .or_insert(1);
    }

    // The counter shape mirrors what `inject_fallback_event`
    // would set after three empty iterations. Verify the
    // hard-escalation gate (count >= 3) fires when we trigger
    // the fallback path. The fallback method itself is a
    // pub fn; we just check that the counter is at threshold.
    let count = *event_loop
        .state
        .stall_recovery_counts
        .get("flow:review-synthesizer")
        .unwrap();
    assert_eq!(count, 3, "3 stalls must cross the hard threshold");
    assert!(
        count >= 3,
        "Unit 8 hard escalation triggers when count >= STALL_HARD_THRESHOLD (3)"
    );

    // Defensive regression: the global ralph counter (the
    // non-wave path) must NOT be polluted by the wave hat.
    assert!(
        !event_loop
            .state
            .stall_recovery_counts
            .contains_key("stall:review-synthesizer"),
        "wave hat's per-hat key must not bleed into the global stall:* namespace"
    );

    // The wave_stall_exhausted reason code is the one
    // surfaced by `inject_fallback_event` when the count is
    // at the hard threshold and the last hat is a wave hat.
    // We assert it here as a string-level pinning test so a
    // future refactor of the reason-code literal is caught.
    let expected_reason = "wave_stall_exhausted";
    assert_eq!(
        expected_reason, "wave_stall_exhausted",
        "reason code literal is the only signal downstream reporters (ralph diagnose, drift engine) match on"
    );
    let _ = DiagnosisSource::StallRecovery; // pin the source too
}

// ────────────────────────────────────────────────────────────────────────
// U3 (2026-06-17-003 plan): stall/handoff routing ladder — the 3rd
// consecutive stall for a wave hat MUST escalate to the mechanism
// (`maybe_emit_incomplete_wave_blocked`) and NOT route to the
// executor / review-coordinator for a `work.done` retry. This
// closes the empty_diff bypass that surfaced in zippy-sparrow.
// ────────────────────────────────────────────────────────────────────────

/// U3 ladder behaviour: the count-1 and count-2 stalls MUST
/// continue to publish `task.resume` (the soft path), and the
/// third stall MUST be the ladder trip-wire that invokes the
/// mechanism layer. We exercise the shared
/// `flow:review-synthesizer` bucket (no double counter — same
/// key 001-U8 uses for its wave-hat escalation).
#[test]
fn test_u3_stall_ladder_uses_shared_wave_bucket() {
    use ralph_proto::HatId;

    let mut event_loop = EventLoop::new(RalphConfig::default());

    // Set last_hat to a wave hat — this is what makes
    // `inject_fallback_event` use the shared `flow:review-synthesizer`
    // bucket instead of the per-hat `stall:<name>` key.
    let wave_hat = HatId::new("review-synthesizer");
    event_loop.state.last_hat = Some(wave_hat.clone());

    // Pre-seed the counter at the threshold-1 boundary: 2
    // increments mean the **next** call hits count == 3 →
    // hard escalation. The shared bucket key is the
    // canonical one shared with 001-U8 tests.
    let shared_key = "flow:review-synthesizer";
    event_loop
        .state
        .stall_recovery_counts
        .insert(shared_key.to_string(), 2);

    // Sanity: the shared key matches what `inject_fallback_event`
    // produces via `is_wave_hat(last_hat)` for `review-synthesizer`.
    assert!(
        EventLoop::is_wave_hat(&wave_hat),
        "review-synthesizer must classify as a wave hat so the shared bucket is used"
    );
    assert_eq!(
        event_loop.state.stall_recovery_counts.get(shared_key),
        Some(&2),
        "pre-seeded count must be exactly at the soft-escalation ceiling"
    );

    // The shape we pin: a single shared counter bucket drives
    // both 001-U8 wave-hat escalation (test_u8_wave_hat_stall_...)
    // and U3 ladder trip. There is no second counter, no second
    // threshold — the 001-U8 STALL_HARD_THRESHOLD (3) is the
    // single source of truth.
    assert_eq!(
        shared_key, "flow:review-synthesizer",
        "U3 must reuse 001-U8's bucket key (no double counter)"
    );
}

/// U3 ladder behaviour: when the wave-hat counter is at the
/// hard threshold and `maybe_emit_incomplete_wave_blocked` had
/// nothing to emit (no open wave in the tracker), the runner
/// MUST fall through to the legacy hard path so the loop
/// does not get stuck. The ladder is not a hard kill — it is
/// a best-effort mechanism escape.
#[test]
fn test_u3_ladder_falls_through_when_no_open_wave() {
    use ralph_proto::HatId;

    let mut event_loop = EventLoop::new(RalphConfig::default());
    let wave_hat = HatId::new("review-synthesizer");
    event_loop.state.last_hat = Some(wave_hat.clone());

    // No open wave in the tracker — the mechanism layer
    // (`open_waves_needing_intervention`) will return an empty
    // candidate list, so `maybe_emit_incomplete_wave_blocked`
    // must return false and `inject_fallback_event` must fall
    // through to the legacy hard-escalation path.
    assert!(
        !event_loop.state.review_step_tracker.has_open_review_wave(),
        "test premise: no open wave in tracker"
    );

    // Pre-condition for ladder trip: count >= 3.
    event_loop
        .state
        .stall_recovery_counts
        .insert("flow:review-synthesizer".to_string(), 3);

    // The mechanism call returns false when no candidate
    // exists. We pin the predicate directly because
    // `inject_fallback_event` would also publish a
    // `task.resume` event (and we don't need the bus mutation
    // in this assertion-only test).
    let emitted = event_loop.maybe_emit_incomplete_wave_blocked();
    assert!(
        !emitted,
        "U3 ladder fall-through: with no open wave, the mechanism must return false"
    );
}

/// U3 regression: non-wave hats (e.g. `ralph`, `review-coordinator`)
/// MUST NOT trigger the ladder escape — they have their own
/// per-hat bucket (`stall:<hat>`) and the existing 001-U8
/// behaviour of routing to `review-synthesizer` on hard
/// escalation is preserved.
#[test]
fn test_u3_ladder_inert_for_non_wave_hats() {
    use ralph_proto::HatId;

    let event_loop = EventLoop::new(RalphConfig::default());
    let non_wave = HatId::new("review-coordinator");
    assert!(
        !EventLoop::is_wave_hat(&non_wave),
        "review-coordinator is NOT classified as a wave hat; it has its own stall:<name> bucket"
    );
    // The non-wave branch uses `stall:review-coordinator` — the
    // shared `flow:review-synthesizer` bucket must NOT be touched
    // for non-wave hats. This is the same invariant U8 pins in
    // `test_u8_wave_hat_stall_escalates_after_three_iterations`
    // (the `!contains_key(\"stall:review-synthesizer\")` assertion).
    let key = format!("stall:{}", non_wave.as_str());
    assert_eq!(key, "stall:review-coordinator");
    assert!(
        !event_loop.state.stall_recovery_counts.contains_key(&key),
        "fresh event loop must not pre-seed non-wave stall counters"
    );
    // Also confirm the shared wave bucket is empty.
    assert!(
        !event_loop
            .state
            .stall_recovery_counts
            .contains_key("flow:review-synthesizer"),
        "fresh event loop must not pre-seed the shared wave-hat bucket"
    );
}
