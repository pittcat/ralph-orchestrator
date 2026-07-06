//! Characterization tests for plan `2026-06-16-001` U1: the isolated
//! per-turn business-event budget must admit a complete wave group
//! (all events sharing one `wave_id`) atomically, regardless of whether
//! a non-wave business event preceded it.
//!
//! Root cause of the upstream incident: the legacy
//! `first_wave_id_accepted: Option<Option<String>>` slot was poisoned
//! by a preceding no-wave business event. After the first non-wave
//! business event was admitted, `first_wave_id_accepted` was set to
//! `Some(None)`, and any subsequent `review.dimension.done` carrying a
//! `wave_id` failed the `same_wave_continuation` match and was dropped
//! by the budget.
//!
//! The new budget is two independent slots:
//! - `non_wave_business_event_accepted: bool` — the single non-wave
//!   business event slot per turn.
//! - `accepted_wave_id: Option<String>` — the wave group slot per turn.
//!
//! A wave group is admitted in full when its `wave_id` matches
//! `accepted_wave_id`, OR when no wave has been admitted yet. A
//! distinct second `wave_id` is still rejected (one business emission
//! per turn for the wave group). The `queue.advance` + `work.ready`
//! dual-publish handoff (2026-06-15-003 U1) is preserved verbatim.
//!
//! Note on test method: `process_events_from_jsonl_with_waves`
//! returns `ProcessedEventsWithWaves.wave_events` directly as
//! `event_reader::Event` (with hat/wave_id preserved). This is the
//! canonical post-partition surface and the right place to assert
//! budget decisions. `process_events_from_jsonl` rebuilds events
//! through `Event::new(topic, payload)` at the bus-publish boundary
//! (mod.rs:7615-7621) which discards the source/wave fields, so we
//! route wave tests through the wave partition entry point instead.

use super::*;
use crate::event_reader::Event as JsonlEvent;
use std::io::Write;

/// Write a non-wave business event (no wave_id) into the events file.
fn write_business_event(path: &std::path::Path, topic: &str, payload: &str, hat: &str) {
    let ts = chrono::Utc::now().to_rfc3339();
    let json = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": ts,
        "hat": hat,
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(f, "{}", json).unwrap();
}

/// Write a wave result event (with wave_id) into the events file.
fn write_wave_result_event(
    path: &std::path::Path,
    topic: &str,
    payload: &str,
    hat: &str,
    wave_id: &str,
    wave_index: u32,
    wave_total: u32,
) {
    let ts = chrono::Utc::now().to_rfc3339();
    let json = serde_json::json!({
        "topic": topic,
        "payload": payload,
        "ts": ts,
        "hat": hat,
        "wave_id": wave_id,
        "wave_index": wave_index,
        "wave_total": wave_total,
    });
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .unwrap();
    writeln!(f, "{}", json).unwrap();
}

/// Build a minimal isolated-mode event loop with a coordinator + reviewer
/// topology that supports `review.file` wave dispatch and a
/// `review.dimension.done` wave-result merge path.
fn make_isolated_budget_loop(events_path: &std::path::Path) -> EventLoop {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "review.file"
    business_topics:
      - "review.done"
  execution_mode: isolated
hats:
  coordinator:
    name: "Coordinator"
    triggers: ["task.start"]
    publishes: ["review.file", "queue.advance"]
  reviewer:
    name: "Reviewer"
    triggers: ["review.file"]
    publishes: ["review.done"]
    concurrency: 4
    instructions: "Review."
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("TestU1Budget");
    event_loop.event_reader = crate::event_reader::EventReader::new(events_path);
    event_loop
}

/// Build an isolated-mode loop with non-wave business events only —
/// `executor` publishes `work.ready` + `work.done`, used to test the
/// non-wave budget slot independently of wave partition.
fn make_isolated_non_wave_loop(events_path: &std::path::Path) -> EventLoop {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics: []
    business_topics: ["work.ready", "work.done"]
  execution_mode: isolated
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.ready", "work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("TestU1NonWave");
    event_loop.event_reader = crate::event_reader::EventReader::new(events_path);
    event_loop
}

/// Count `event.isolation.boundary_violation` events written to the bus
/// by the per-turn budget in this read batch.
fn count_budget_violations(event_loop: &EventLoop) -> usize {
    let hat_ids: Vec<ralph_proto::HatId> = event_loop.bus.hat_ids().cloned().collect();
    hat_ids
        .iter()
        .flat_map(|id| event_loop.bus.peek_pending(id).cloned().unwrap_or_default())
        .filter(|e| e.topic.as_str() == "event.isolation.boundary_violation")
        .count()
}

/// U1.HAPPY: a turn that reads 4 `review.file` events sharing one
/// `wave_id` must admit all 4 into `wave_events` (the wave partition
/// preserves event_reader::Event with wave_id intact, so we can assert
/// budget admission by counting wave_events).
#[test]
fn test_u1_wave_group_with_wave_id_admitted_atomically() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_budget_loop(&events_path);

    // coordinator publishes review.file; this is the in-scope wave topic.
    event_loop.state.current_isolated_hat = Some(HatId::new("coordinator"));

    for i in 0..4u32 {
        write_wave_result_event(
            &events_path,
            "review.file",
            &format!("src/file-{}.rs", i),
            "coordinator",
            "w-u1-happy",
            i,
            4,
        );
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    assert_eq!(
        result.wave_events.len(),
        4,
        "all 4 review.file events in the same wave must be admitted; got {} (raw={})",
        result.wave_events.len(),
        result.wave_raw_count
    );
    let distinct_wave_ids: std::collections::HashSet<&str> = result
        .wave_events
        .iter()
        .filter_map(|e| e.wave_id.as_deref())
        .collect();
    assert_eq!(
        distinct_wave_ids.len(),
        1,
        "preserved wave must carry exactly one distinct wave_id"
    );
    assert_eq!(
        count_budget_violations(&event_loop),
        0,
        "no boundary violation should fire for a single-wave group"
    );
}

/// U1.MIXED: a turn that first sees a non-wave `queue.advance` from
/// the coordinator, then sees 4 `review.file` from the same wave, must
/// admit all 5 events. The non-wave slot and the wave-group slot are
/// independent, so neither poisons the other.
///
/// We test this via the non-wave partition: the `processed.accepted_events`
/// list reflects the post-budget accepted non-wave events, and the
/// `wave_events` list reflects the post-budget accepted wave group. The
/// `processed.accepted_events` list (built from `Event::new(topic, payload)`)
/// discards hat/wave fields, so we only assert counts and topics here.
#[test]
fn test_u1_mixed_non_wave_then_wave_group_admitted() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_budget_loop(&events_path);

    event_loop.state.current_isolated_hat = Some(HatId::new("coordinator"));

    write_business_event(&events_path, "queue.advance", "{}", "coordinator");
    for i in 0..4u32 {
        write_wave_result_event(
            &events_path,
            "review.file",
            &format!("src/file-{}.rs", i),
            "coordinator",
            "w-u1-mixed",
            i,
            4,
        );
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    // The non-wave business event (queue.advance) should be admitted.
    // Note: `processed.accepted_events` is `Vec<ralph_proto::Event>`
    // (built from `Event::new(topic, payload)` at the bus-publish
    // boundary, mod.rs:7615-7621), so the source/wave fields are
    // discarded at this surface. We can only assert by topic here.
    let non_wave_accepted: Vec<&ralph_proto::Event> = result
        .processed
        .accepted_events
        .iter()
        .filter(|e| e.topic.as_str() == "queue.advance")
        .collect();
    assert_eq!(
        non_wave_accepted.len(),
        1,
        "preceding queue.advance (non-wave business) must be admitted"
    );

    // The wave group (4 review.file with wave_id=w-u1-mixed) should be
    // admitted in full — this is the regression the U1 fix addresses.
    let wave_accepted: Vec<&JsonlEvent> = result
        .wave_events
        .iter()
        .filter(|e| e.wave_id.as_deref() == Some("w-u1-mixed"))
        .collect();
    assert_eq!(
        wave_accepted.len(),
        4,
        "wave group of 4 review.file must be admitted even when a preceding non-wave queue.advance was accepted; got {}",
        wave_accepted.len()
    );

    assert_eq!(
        count_budget_violations(&event_loop),
        0,
        "mixed batch should produce no budget violations"
    );
}

/// U1.WAVE-COLLISION: a turn that sees wave group `w-A` (3 events)
/// followed by a distinct wave group `w-B` (3 events) must admit the
/// first wave's 3 events and reject the second wave's 3 events as a
/// second business emission.
///
/// Note: wave dispatch events go through the wave partition
/// (`enforce_wave_isolated_scope`) which has its own multi-wave
/// rejection path — it does NOT route through the `process_parse_result`
/// budget we are testing here. The budget logic in U1 is the gate
/// applied to non-wave business events. So we test the wave-group
/// rejection through the wave partition's own diagnostic surface
/// (`.scope_violation` on the bus) and rely on the existing
/// `wave_isolated_scope` test suite for partition-level coverage.
#[test]
fn test_u1_two_distinct_wave_ids_rejects_second_wave() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_budget_loop(&events_path);

    event_loop.state.current_isolated_hat = Some(HatId::new("coordinator"));

    for i in 0..3u32 {
        write_wave_result_event(
            &events_path,
            "review.file",
            &format!("first-{}", i),
            "coordinator",
            "w-A",
            i,
            3,
        );
    }
    for i in 0..3u32 {
        write_wave_result_event(
            &events_path,
            "review.file",
            &format!("second-{}", i),
            "coordinator",
            "w-B",
            i,
            3,
        );
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    let wave_a: Vec<&JsonlEvent> = result
        .wave_events
        .iter()
        .filter(|e| e.wave_id.as_deref() == Some("w-A"))
        .collect();
    let wave_b: Vec<&JsonlEvent> = result
        .wave_events
        .iter()
        .filter(|e| e.wave_id.as_deref() == Some("w-B"))
        .collect();

    assert_eq!(
        wave_a.len(),
        3,
        "first wave group w-A must be admitted in full"
    );
    assert_eq!(
        wave_b.len(),
        0,
        "second wave group w-B must be rejected as a second business emission"
    );
    // The wave partition emits `*.scope_violation` diagnostics, not
    // `event.isolation.boundary_violation` — verifying the budget did
    // not also flag the partition is sufficient.
    assert_eq!(
        count_budget_violations(&event_loop),
        0,
        "wave partition handles its own multi-wave rejection; budget layer should not double-fire"
    );
}

/// U1.NON-WAVE-COLLISION: a turn that sees two independent non-wave
/// business events (NOT a dual-publish pair) must admit the first and
/// reject the second. The non-wave slot is single-use per turn.
#[test]
fn test_u1_two_independent_non_wave_events_rejects_second() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_non_wave_loop(&events_path);

    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    // Two non-wave business events: first admitted, second rejected
    // (work.ready → work.done is NOT the dual-publish pair; the pair is
    // queue.advance → work.ready, both with the same hat).
    write_business_event(&events_path, "work.ready", "{}", "executor");
    write_business_event(&events_path, "work.done", "{}", "executor");

    let result = event_loop.process_events_from_jsonl().unwrap();

    let work_ready_count = result
        .accepted_events
        .iter()
        .filter(|e| e.topic.as_str() == "work.ready")
        .count();
    let work_done_count = result
        .accepted_events
        .iter()
        .filter(|e| e.topic.as_str() == "work.done")
        .count();

    assert_eq!(
        work_ready_count, 1,
        "first non-wave business event (work.ready) must be admitted"
    );
    assert_eq!(
        work_done_count, 0,
        "second independent non-wave business event (work.done) must be rejected"
    );
    assert!(
        count_budget_violations(&event_loop) >= 1,
        "rejected event should publish a budget-violation diagnostic"
    );
}

/// U1.LEGACY: a turn that admits a non-wave business event FIRST and
/// then a wave group must still admit the wave group in full. This
/// is the regression scenario from the 2026-06-16 incident: the wave
/// events used to be dropped because the first non-wave business event
/// had set `first_wave_id_accepted = Some(None)`.
#[test]
fn test_u1_regression_non_wave_first_then_wave_admitted() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_budget_loop(&events_path);

    // The pre-fix code set `first_wave_id_accepted = Some(None)` after
    // admitting the first non-wave business event, which caused all
    // subsequent wave events to fail the `same_wave_continuation` match
    // (None != Some("w-...")). We model that scenario here.
    event_loop.state.current_isolated_hat = Some(HatId::new("coordinator"));

    write_business_event(&events_path, "queue.advance", "{}", "coordinator");
    for i in 0..7u32 {
        write_wave_result_event(
            &events_path,
            "review.file",
            &format!("src/regr-{}.rs", i),
            "coordinator",
            "w-regression",
            i,
            7,
        );
    }

    let result = event_loop.process_events_from_jsonl_with_waves().unwrap();

    let wave_accepted: Vec<&JsonlEvent> = result
        .wave_events
        .iter()
        .filter(|e| e.wave_id.as_deref() == Some("w-regression"))
        .collect();

    assert_eq!(
        wave_accepted.len(),
        7,
        "regression: 7 wave events must be admitted after a preceding non-wave event; got {}",
        wave_accepted.len()
    );

    let non_wave_accepted: Vec<&ralph_proto::Event> = result
        .processed
        .accepted_events
        .iter()
        .filter(|e| e.topic.as_str() == "queue.advance")
        .collect();
    assert_eq!(
        non_wave_accepted.len(),
        1,
        "preceding non-wave business event must be admitted alongside the wave group"
    );
}

// 2026-07-03-005 plan (P0 fix M-1): declared serial walk exemption.
// The isolated-mode per-turn single-business-event budget normally
// drops subsequent business events after the first. A hat that has
// declared an exempt_topics entry for a topic may emit that topic
// repeatedly without consuming the slot. The classic case is
// review-coordinator walking 6 review.dimension.ready events.

#[test]
fn m1_exempt_topic_admits_repeated_emits_in_serial_walk() {
    // 1. Set up an isolated hat whose `exempt_topics` lists the
    //    topic we want to emit. The build path for this in a real
    //    preset is `hats.<hat>.exempt_topics: [...]`; here we use
    //    the in-process HatRegistry::from_config so the test is
    //    self-contained.
    let yaml = r#"
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["review.dimension.done"]
    publishes: ["review.dimension.ready", "review.dimensions.complete"]
    exempt_topics: ["review.dimension.ready"]
"#;
    let cfg: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let registry = crate::hat_registry::HatRegistry::from_config(&cfg);
    let hat = ralph_proto::HatId::from("review-coordinator");

    // 2. Without exemption, the second emit would be dropped.
    let dropped = !crate::event_loop::is_isolated_exempt_topic(
        registry.get_config(&ralph_proto::HatId::from("coordinator")),
        "review.dimension.ready",
        &[],
        &[],
    );
    assert!(dropped, "M-1: non-exempt hat must report false");

    // 3. With exemption, all 6 emits in a serial walk are admitted.
    let admitted = (0..6).all(|_| {
        crate::event_loop::is_isolated_exempt_topic(
            registry.get_config(&hat),
            "review.dimension.ready",
            &[],
            &[],
        )
    });
    assert!(admitted, "M-1: declared serial walk must admit all 6 emits");

    // 4. Negative: an exempt topic is a positive list — other topics
    //    on the same hat are NOT exempt.
    let not_exempt = !crate::event_loop::is_isolated_exempt_topic(
        registry.get_config(&hat),
        "work.ready",
        &[],
        &[],
    );
    assert!(
        not_exempt,
        "M-1: exempt_topics is a positive list, not a wildcard"
    );
}

#[test]
fn m1_exempt_topic_falls_back_to_default_when_unregistered() {
    // Unknown hat → no exemption (default behaviour preserved).
    let registry =
        crate::hat_registry::HatRegistry::from_config(&crate::config::RalphConfig::default());
    let unknown = ralph_proto::HatId::from("does-not-exist");
    assert!(
        !crate::event_loop::is_isolated_exempt_topic(
            registry.get_config(&unknown),
            "any.topic",
            &[],
            &[],
        ),
        "M-1: unknown hat must report false (no crash, no false-positive exemption)"
    );
}

#[test]
fn u13_business_topics_carve_out_admits_serial_walk() {
    // 2026-07-04-001 plan U13 (KTD-11): a hat that has a topic in its
    // `publishes` list AND the same topic is declared in
    // `event_policy.business_topics` is exempt from the per-turn
    // budget — even without an explicit `exempt_topics` declaration.
    // This is the SSOT for the carve-out: derive from config, not
    // per-hat whitelist.
    let yaml = r#"
hats:
  review-coordinator:
    name: "Review Coordinator"
    subscribes_to: [queue.advance]
    publishes:
      - review.dimension.ready
  coordinator:
    name: "Coordinator"
    subscribes_to: [task.start]
    publishes:
      - queue.advance
"#;
    let cfg: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let registry = crate::hat_registry::HatRegistry::from_config(&cfg);

    let business_topics = vec!["review.dimension.ready".to_string()];
    let terminal_topics: Vec<String> = vec![];

    // review-coordinator: business_topics has the topic AND hat
    // publishes it → exempt.
    let admitted = crate::event_loop::is_isolated_exempt_topic(
        registry.get_config(&ralph_proto::HatId::from("review-coordinator")),
        "review.dimension.ready",
        &business_topics,
        &terminal_topics,
    );
    assert!(
        admitted,
        "U13: hat whose publishes intersects business_topics must be exempt"
    );

    // coordinator: hat does NOT publish review.dimension.ready → not
    // exempt even though the topic is in business_topics (SSOT).
    let not_admitted = !crate::event_loop::is_isolated_exempt_topic(
        registry.get_config(&ralph_proto::HatId::from("coordinator")),
        "review.dimension.ready",
        &business_topics,
        &terminal_topics,
    );
    assert!(
        not_admitted,
        "U13: hat that does not publish the topic must NOT be exempt"
    );
}

#[test]
fn u13_terminal_topics_carve_out_admits_serial_walk() {
    // 2026-07-04-001 plan U13 (KTD-11): the same carve-out also
    // applies to `event_policy.terminal_topics`. Per-step terminal
    // events declared in terminal_topics are exempt from the budget
    // when the calling hat publishes them.
    let yaml = r#"
hats:
  fixer:
    name: "Fixer"
    subscribes_to: [fix.unit.ready]
    publishes:
      - fix.applied
"#;
    let cfg: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let registry = crate::hat_registry::HatRegistry::from_config(&cfg);

    let business_topics: Vec<String> = vec![];
    let terminal_topics = vec!["fix.applied".to_string()];

    let admitted = crate::event_loop::is_isolated_exempt_topic(
        registry.get_config(&ralph_proto::HatId::from("fixer")),
        "fix.applied",
        &business_topics,
        &terminal_topics,
    );
    assert!(
        admitted,
        "U13: terminal_topics carve-out must admit hat that publishes the topic"
    );
}

#[test]
fn u13_business_topics_carve_out_does_not_admit_unrelated_hat() {
    // Negative: a topic not declared in business_topics or
    // terminal_topics is NOT exempt even if the hat publishes it.
    let yaml = r#"
hats:
  implementer:
    name: "Implementer"
    subscribes_to: [work.ready]
    publishes:
      - work.done
"#;
    let cfg: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let registry = crate::hat_registry::HatRegistry::from_config(&cfg);

    let business_topics = vec!["something.else".to_string()];
    let terminal_topics: Vec<String> = vec![];

    let not_admitted = !crate::event_loop::is_isolated_exempt_topic(
        registry.get_config(&ralph_proto::HatId::from("implementer")),
        "work.done",
        &business_topics,
        &terminal_topics,
    );
    assert!(
        not_admitted,
        "U13: topics not in business_topics/terminal_topics must NOT be exempt"
    );
}
