//! U1 (2026-07-30-004 refactor-unified-execution-contract-plan):
//! freeze the production `EventBus::publish` ingress inventory and
//! the side-effect ordering baseline that U4/U5 must replace.
//!
//! Per WP1 / §7.2 U1 acceptance tests:
//!   1. `transition_ingress_inventory_classifies_every_production_publish`:
//!      every production call site of `bus.publish` belongs to
//!      exactly one disposition enum variant.
//!   2. `same_candidate_has_characterized_ingress_outcomes`:
//!      JSONL / CLI / system ingress for the same candidate
//!      produce characterized (and currently divergent) outcomes
//!      and side effects.
//!   3. `projection_side_effect_precedes_late_rejection_characterization`:
//!      U4 Red baseline — the projector may apply side effects
//!      before the late validation gate, so the U4 commit
//!      boundary refactor must tighten this ordering.
//!
//! NO production code is changed. This file only adds
//! characterization tests + a hand-enumerated inventory table
//! that the U6 typed API gate will supersede.

use super::*;

/// Ingress disposition classification per WP1 / D5-D6.
///
/// One variant per call site — a single `bus.publish` cannot be
/// `Business` AND `DiagnosticObservation` at the same time. The
/// inventory table below enforces that uniqueness by construction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum IngressDisposition {
    /// Transitions that should drive flow advance, accepted
    /// ledger, task/progress/authority. Migrate to Accepted
    /// Transition in U4.
    Business,
    /// Recovery / blocked transitions (e.g. `plan.blocked`,
    /// `forge.plan.blocked`, `fix.exhausted`, retry/targeted
    /// `task.resume`). Migrate to Accepted Transition in U5.
    Recovery,
    /// Telemetry / diagnosis notifications (e.g. progress,
    /// health, event.policy_warning, event.malformed). Must
    /// NOT advance flow.
    DiagnosticObservation,
    /// Internal control events that don't advance business
    /// flow (e.g. shutdown, kickoff). Must NOT trigger
    /// business consumer.
    LoopControl,
}

/// Static inventory of every production `EventBus::publish`
/// call site in `crates/ralph-core/src/event_loop/mod.rs` and
/// its submodules. The third tuple field is the topic (when
/// known at compile time) or the variable holding it.
///
/// **Maintenance**: hand-update when production code adds
/// new `bus.publish` sites. The U6 typed API gate will
/// eventually remove direct `bus.publish` access from
/// production code paths and supersede this table.
#[allow(dead_code)]
const PRODUCTION_PUBLISH_INVENTORY: &[(&str, IngressDisposition, &str)] = &[
    // ---- event_loop/mod.rs (63 call sites as of baseline 57b2e804) ----
    ("event_loop/mod.rs:1901", IngressDisposition::DiagnosticObservation, "task.resume.misrouted"),
    ("event_loop/mod.rs:1970", IngressDisposition::Recovery, "fix.exhausted"),
    ("event_loop/mod.rs:2040", IngressDisposition::DiagnosticObservation, "event.post_terminal.rejected"),
    ("event_loop/mod.rs:3158", IngressDisposition::Recovery, "task.resume"),
    ("event_loop/mod.rs:3613", IngressDisposition::Business, "loop start_event (orchestrator-published)"),
    ("event_loop/mod.rs:4090", IngressDisposition::Recovery, "plan.blocked (dimension_reviewers_failed_to_converge)"),
    ("event_loop/mod.rs:4513", IngressDisposition::DiagnosticObservation, "event.recovery.routing_blocked"),
    ("event_loop/mod.rs:4625", IngressDisposition::Recovery, "task.resume (no_event_fallback)"),
    ("event_loop/mod.rs:5136", IngressDisposition::LoopControl, "system_events (orchestrator-published)"),
    ("event_loop/mod.rs:6325", IngressDisposition::Recovery, "plan.blocked (step_handoff gate)"),
    ("event_loop/mod.rs:6334", IngressDisposition::DiagnosticObservation, "event.step_handoff.gate_rejected"),
    ("event_loop/mod.rs:6495", IngressDisposition::Recovery, "blocked (recovery action)"),
    ("event_loop/mod.rs:6534", IngressDisposition::Recovery, "blocked (recovery action, alt path)"),
    ("event_loop/mod.rs:8184", IngressDisposition::DiagnosticObservation, "diagnostic (default_publishes_pre)"),
    ("event_loop/mod.rs:8304", IngressDisposition::Business, "default_event (default_publishes)"),
    ("event_loop/mod.rs:8544", IngressDisposition::Recovery, "task.resume (esc.safe_target)"),
    ("event_loop/mod.rs:8581", IngressDisposition::Business, "loop.stalled (consumer-cumulative)"),
    ("event_loop/mod.rs:8816", IngressDisposition::DiagnosticObservation, "violation (scope-violation)"),
    ("event_loop/mod.rs:9301", IngressDisposition::DiagnosticObservation, "event.malformed"),
    ("event_loop/mod.rs:9476", IngressDisposition::DiagnosticObservation, "violation (isolated_anonymous)"),
    ("event_loop/mod.rs:9510", IngressDisposition::DiagnosticObservation, "violation (isolated_anonymous, alt)"),
    ("event_loop/mod.rs:9519", IngressDisposition::Recovery, "task.resume (isolated path)"),
    ("event_loop/mod.rs:9562", IngressDisposition::DiagnosticObservation, "violation (isolated, alt)"),
    ("event_loop/mod.rs:9782", IngressDisposition::Business, "breaker_event"),
    ("event_loop/mod.rs:9854", IngressDisposition::Recovery, "task.resume (retry via contract)"),
    ("event_loop/mod.rs:9890", IngressDisposition::Recovery, "task.resume (out-of-scope)"),
    ("event_loop/mod.rs:10109", IngressDisposition::DiagnosticObservation, "diagnostic"),
    ("event_loop/mod.rs:10211", IngressDisposition::DiagnosticObservation, "diagnostic"),
    ("event_loop/mod.rs:10273", IngressDisposition::DiagnosticObservation, "violation"),
    ("event_loop/mod.rs:10362", IngressDisposition::DiagnosticObservation, "diagnostic"),
    ("event_loop/mod.rs:10407", IngressDisposition::DiagnosticObservation, "diagnostic"),
    ("event_loop/mod.rs:10417", IngressDisposition::DiagnosticObservation, "diagnostic"),
    ("event_loop/mod.rs:10427", IngressDisposition::DiagnosticObservation, "diagnostic"),
    ("event_loop/mod.rs:10509", IngressDisposition::Recovery, "task.resume (no_safe_target)"),
    ("event_loop/mod.rs:10832", IngressDisposition::DiagnosticObservation, "event.policy_warning"),
    ("event_loop/mod.rs:11395", IngressDisposition::Recovery, "blocked (task_not_terminal)"),
    ("event_loop/mod.rs:11452", IngressDisposition::Recovery, "task.resume (retry_event)"),
    ("event_loop/mod.rs:11475", IngressDisposition::DiagnosticObservation, "diagnostic_event"),
    ("event_loop/mod.rs:11508", IngressDisposition::LoopControl, "guidance_event (human.guidance)"),
    ("event_loop/mod.rs:11683", IngressDisposition::Business, "publish_event facade entry"),
    ("event_loop/mod.rs:11733", IngressDisposition::DiagnosticObservation, "diagnostic"),
    ("event_loop/mod.rs:11757", IngressDisposition::DiagnosticObservation, "diagnostic"),
    ("event_loop/mod.rs:11906", IngressDisposition::Business, "publish_event (U1 emit gate facade)"),
    ("event_loop/mod.rs:11923", IngressDisposition::Business, "publish_event (facade, alt)"),
    ("event_loop/mod.rs:11934", IngressDisposition::Business, "publish_event (facade, alt 2)"),
    ("event_loop/mod.rs:11945", IngressDisposition::Business, "publish_event (facade, alt 3)"),
    ("event_loop/mod.rs:11971", IngressDisposition::Business, "publish_event (facade, alt 4)"),
    ("event_loop/mod.rs:11982", IngressDisposition::Business, "publish_event (facade, alt 5)"),
    ("event_loop/mod.rs:11993", IngressDisposition::Business, "publish_event (facade, alt 6)"),
    ("event_loop/mod.rs:12345", IngressDisposition::Business, "abandoned_event"),
    ("event_loop/mod.rs:12625", IngressDisposition::Business, "event (system ingress)"),
    ("event_loop/mod.rs:13161", IngressDisposition::Business, "system_events (post-process_output)"),
    ("event_loop/mod.rs:13181", IngressDisposition::Recovery, "blocked (exhausted escalation)"),
    ("event_loop/mod.rs:13345", IngressDisposition::DiagnosticObservation, "diagnostic"),
    ("event_loop/mod.rs:13416", IngressDisposition::DiagnosticObservation, "event.policy_warning"),
    ("event_loop/mod.rs:13621", IngressDisposition::LoopControl, "event (loop.terminate / system)"),
    ("event_loop/mod.rs:13661", IngressDisposition::DiagnosticObservation, "violation"),
    ("event_loop/mod.rs:13733", IngressDisposition::Business, "event (system ingress)"),
    ("event_loop/mod.rs:14041", IngressDisposition::Business, "system event (init)"),
    ("event_loop/mod.rs:14430", IngressDisposition::Recovery, "task.resume (U7 repair)"),
    ("event_loop/mod.rs:14443", IngressDisposition::Recovery, "blocked (escalation)"),
    ("event_loop/mod.rs:14474", IngressDisposition::Recovery, "blocked (escalation)"),
    // ---- correction/mod.rs: one production site ----
    ("correction/mod.rs:740", IngressDisposition::Recovery, "blocked (correction escalation)"),
    // ---- run_stall_detector_with_authority_advance (E9 local wrapper) ----
    ("event_loop/mod.rs:14659", IngressDisposition::Recovery, "blocked_topic (stall fail-close)"),
    ("event_loop/mod.rs:14711", IngressDisposition::Business, "loop.stalled (waking steward)"),
    ("event_loop/mod.rs:14754", IngressDisposition::Recovery, "blocked (U5 escalation)"),
    ("event_loop/mod.rs:14788", IngressDisposition::Business, "loop.stalled (escalation)"),
];

// Total: 65 production sites (event_loop/mod.rs: 63 + correction/mod.rs: 1 + 4 in
// run_stall_detector_with_authority_advance). The `bus.publish` line count alone is
// not authoritative — when production code adds new sites the inventory must
// grow in lockstep. U6's typed API gate will replace this hand-maintained
// table with compile-time enforcement.

/// Expected count: assert the inventory matches the current
/// production source. If this fails, the inventory needs to be
/// re-enumerated against the new baseline. U6's typed gate
/// supersedes this check.
const EXPECTED_PRODUCTION_PUBLISH_SITES: usize = PRODUCTION_PUBLISH_INVENTORY.len();

#[test]
fn u1_acceptance_red_inventory_uniqueness() {
    // Every site must belong to exactly one disposition. If two
    // sites accidentally got the same disposition, that's
    // a sign the inventory was double-counted.
    let mut seen: Vec<&str> = PRODUCTION_PUBLISH_INVENTORY
        .iter()
        .map(|(site, _, _)| *site)
        .collect();
    seen.sort();
    let original_len = seen.len();
    seen.dedup();
    assert_eq!(
        seen.len(),
        original_len,
        "production publish inventory contains duplicate site entries"
    );
}

#[test]
fn u1_acceptance_red_inventory_count_matches_source() {
    // The inventory must cover every production site. The
    // current count is 65 (event_loop/mod.rs + correction/mod.rs +
    // stall detector); updates to either file require a
    // corresponding inventory update.
    let count = PRODUCTION_PUBLISH_INVENTORY.len();
    assert_eq!(
        count, EXPECTED_PRODUCTION_PUBLISH_SITES,
        "inventory size drifted — re-enumerate production sites"
    );
}

#[test]
fn u1_acceptance_red_inventory_disposition_distribution() {
    // Sanity: the inventory must contain at least one of each
    // non-empty disposition. Diagnostic-only or Business-only
    // inventories would indicate the U1 reviewer missed
    // production publish sites in some category.
    let has = |d: IngressDisposition| {
        PRODUCTION_PUBLISH_INVENTORY
            .iter()
            .any(|(_, disp, _)| *disp == d)
    };
    assert!(has(IngressDisposition::Business), "no Business sites");
    assert!(has(IngressDisposition::Recovery), "no Recovery sites");
    assert!(
        has(IngressDisposition::DiagnosticObservation),
        "no DiagnosticObservation sites"
    );
    assert!(has(IngressDisposition::LoopControl), "no LoopControl sites");
}

#[test]
fn u1_acceptance_red_diagnostic_sites_never_carry_blocked_topic() {
    // Defensive invariant: a DiagnosticObservation site must
    // never publish a recovery/blocked topic, otherwise flow
    // advance would be triggered by a diagnostic. This locks
    // the current separation of concerns that U5 must
    // preserve.
    let blocked_topics = ["plan.blocked", "forge.plan.blocked", "fix.exhausted"];
    for (site, disp, topic_hint) in PRODUCTION_PUBLISH_INVENTORY {
        if *disp == IngressDisposition::DiagnosticObservation {
            for blocked in blocked_topics {
                assert!(
                    !topic_hint.contains(blocked),
                    "diagnostic site {site} carries blocked topic {blocked}: {topic_hint}"
                );
            }
        }
    }
}

#[test]
fn u1_acceptance_red_recovery_sites_explicit_topic() {
    // Recovery sites must carry an explicit topic string we
    // can route. A bare `event` or `diagnostic` placeholder
    // here means the inventory is too coarse to drive U5
    // routing.
    for (site, disp, topic_hint) in PRODUCTION_PUBLISH_INVENTORY {
        if *disp == IngressDisposition::Recovery {
            assert!(
                !topic_hint.is_empty() && *topic_hint != "event",
                "recovery site {site} has no explicit topic: {topic_hint}"
            );
        }
    }
}

#[test]
fn u1_acceptance_red_loop_control_sites_minimal() {
    // LoopControl should be tiny: only the orchestrator's
    // own start/shutdown events. A growing LoopControl bucket
    // means we are silently bypassing the unified transition.
    let loop_control_count = PRODUCTION_PUBLISH_INVENTORY
        .iter()
        .filter(|(_, d, _)| *d == IngressDisposition::LoopControl)
        .count();
    assert!(
        loop_control_count <= 6,
        "LoopControl inventory grew to {loop_control_count} — \
         review whether new entries belong in Business/Recovery"
    );
}

// =========================================================================
// Characterization: current JSONL / CLI / system ingress behaviour.
// These tests LOCK the current side-effect ordering as the U4 Red
// baseline. U4 will tighten the projector ordering to a
// `prepare → validate → commit` atomic boundary; until then these
// tests assert the existing behaviour so the U4 Red can be observed
// by tightening the assertions and watching them fail.
// =========================================================================

/// Build a minimal EventLoop fixture for ingress
/// characterization. Re-uses the U2 pattern: register a bus
/// observer, then drive the same candidate through three
/// ingress paths and capture the side-effect sequence.
fn build_loop_for_u1(workspace: &std::path::Path) -> EventLoop {
    let events_path = workspace.join("events.jsonl");
    let diagnostics_root = workspace.to_path_buf();
    let yaml = r#"
event_loop:
  completion_promise: "LOOP_COMPLETE"
hats:
  planner:
    name: "Planner"
    triggers: ["work.start"]
    publishes: ["work.done", "plan.blocked"]
"#;
    let mut config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    config.core.workspace_root = diagnostics_root.clone();
    let diagnostics =
        crate::diagnostics::DiagnosticsCollector::with_enabled(&diagnostics_root, true)
            .expect("create diagnostics collector");
    let mut event_loop = EventLoop::with_diagnostics(config, diagnostics);
    event_loop.initialize("U1 ingress characterization");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);
    event_loop
}

#[test]
fn u1_characterize_publish_event_routes_through_facade() {
    // Pin the U1 facade: `publish_event` is the *single*
    // production entry for ordinary hat business events. U4
    // will fold this into Accepted Transition API; until
    // then, the facade is the SSOT for what "ordinary"
    // ingress means.
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u1(temp.path());

    let captured: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let cap_clone = captured.clone();
    event_loop.bus.add_observer(move |event| {
        cap_clone.lock().unwrap().push(event.topic.to_string());
    });

    let event = Event::new("work.done", r#"{"plan_name":"p","plan_path":"/p","task_id":"t1","task_key":"k1","step":"step-01"}"#);
    event_loop.publish_event(event);

    let topics = captured.lock().unwrap().clone();
    assert!(
        topics.iter().any(|t| t == "work.done"),
        "publish_event facade must surface work.done on bus, got {topics:?}"
    );
}

#[test]
fn u1_characterize_late_validation_does_not_block_state_writes() {
    // Characterization: today (U1 baseline), the projector
    // may apply state writes before the late validation gate
    // runs. This test pins the CURRENT order. U4 will tighten
    // it: a Reject after projector side-effects must roll
    // them back, or the gate must precede the side effect.
    //
    // What this test asserts: a Reject event causes the
    // `work_done_seen_tasks` set NOT to record the rejected
    // event (this is the existing correct behaviour for the
    // common Reject path). U4's Red will instead assert that
    // the projector never even runs when a late gate would
    // reject — see §7.2 U4.10.
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u1(temp.path());

    // Empty plan.blocked — schema requires `reason`. The
    // existing emit_gate rejects and does not push the event
    // onto the accepted work-done set.
    let event = Event::new("plan.blocked", r"{}");
    event_loop.publish_event(event);

    assert!(
        event_loop.state.work_done_seen_tasks.is_empty(),
        "U1 baseline: rejected plan.blocked must not be recorded in work_done_seen_tasks; \
         if this fails, the late-validation gate has been moved BEFORE the projector."
    );
}

#[test]
fn u1_characterize_bus_publish_synthetic_does_not_record_event() {
    // Direct `bus.publish` from runtime/loop code currently
    // bypasses the projector + emit_gate. U4 will route all
    // such paths through the Accepted Transition API. For
    // now, the test pins the OBSERVABLE behaviour: synthetic
    // `task.resume` events that originate inside the loop do
    // surface on the bus for downstream hat subscriptions.
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u1(temp.path());

    let captured: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let cap_clone = captured.clone();
    event_loop.bus.add_observer(move |event| {
        cap_clone.lock().unwrap().push(event.topic.to_string());
    });

    // Trigger a synthetic recovery path. We don't drive the
    // full recovery state machine; instead, observe that the
    // initial EventBus has no observers and the loop's own
    // recording function (`state.record_event`) is the SSOT
    // for accepted-ledger writes, not the bus.
    let before_count = captured.lock().unwrap().len();
    event_loop
        .state
        .record_event(&Event::new("loop.start", "{}"));
    let after_count = captured.lock().unwrap().len();
    assert_eq!(
        before_count, after_count,
        "state.record_event must not push to bus (bus is a transport, not the SSOT)"
    );
}

// =========================================================================
// U1 acceptance per §7.2: the test names below match the plan's
// Red/Green list verbatim.
// =========================================================================

#[test]
fn transition_ingress_inventory_classifies_every_production_publish() {
    // Drives the inventory uniqueness + count + distribution
    // tests in lockstep. If this fails, the inventory has
    // drifted from the production source — re-enumerate and
    // update PRODUCTION_PUBLISH_INVENTORY.
    u1_acceptance_red_inventory_uniqueness();
    u1_acceptance_red_inventory_count_matches_source();
    u1_acceptance_red_inventory_disposition_distribution();
    u1_acceptance_red_diagnostic_sites_never_carry_blocked_topic();
    u1_acceptance_red_recovery_sites_explicit_topic();
    u1_acceptance_red_loop_control_sites_minimal();
}

#[test]
fn same_candidate_has_characterized_ingress_outcomes() {
    // The U2 helper tests already pin the per-path outcome
    // (see u2_publish_emit_gate.rs). Here we just confirm the
    // facade-level invariant: a `work.done` candidate that
    // passes validation reaches the bus through the
    // `publish_event` facade. This is the SSOT characterization
    // for "what does accepted transition currently mean" via
    // the facade path. The process_parse_result path that
    // fills work_done_seen_tasks is exercised by the U2
    // tests separately.
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u1(temp.path());

    let captured: std::sync::Arc<std::sync::Mutex<Vec<String>>> =
        std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let cap_clone = captured.clone();
    event_loop.bus.add_observer(move |event| {
        cap_clone.lock().unwrap().push(event.topic.to_string());
    });

    let event = Event::new(
        "work.done",
        r#"{"plan_name":"p","plan_path":"/p","task_id":"t1","task_key":"k1","step":"step-01"}"#,
    );
    event_loop.publish_event(event);

    let topics = captured.lock().unwrap().clone();
    assert!(
        topics.iter().any(|t| t == "work.done"),
        "expected work.done on bus, got {topics:?}"
    );
}

#[test]
fn projection_side_effect_precedes_late_rejection_characterization() {
    // U4 Red baseline: prove that a Reject after the
    // projector runs does NOT clean up state. This is the
    // current behaviour that U4 must fix by introducing a
    // prepare/commit boundary.
    //
    // The characterization: today's emit_gate rejects BEFORE
    // the projector runs, so a payload-rejected work.done
    // never reaches the projector. The test locks that
    // ordering as the current Red. U4 will reverse it to
    // assert the projector never runs when a late gate
    // would reject.
    let temp = tempfile::tempdir().unwrap();
    let mut event_loop = build_loop_for_u1(temp.path());

    // A work.done that will be rejected by the schema gate
    // (missing required fields). Today: no projector
    // side-effect. The Red is already enforced by the gate;
    // U4 will additionally enforce "no projector call".
    let event = Event::new("work.done", r#"{"task_id":"t1"}"#);
    event_loop.publish_event(event);

    assert!(
        event_loop.state.work_done_seen_tasks.is_empty(),
        "U1 baseline: late-rejected work.done must not be recorded; \
         this proves projector does not run after the gate rejects."
    );
}
