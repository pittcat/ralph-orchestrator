//! Characterization tests for plan `2026-06-16-001` U3: `task.resume`
//! freshness TTL filter.
//!
//! Root cause of the 2026-06-16 incident: the loop fed a 50-minute-old
//! `task.resume` (built from a stale rejection of a long-closed
//! `debug.step` event) back into the executor, which then ran a
//! full minute of garbage work and emitted a chain of `work.failed`.
//! The fix: any `task.resume` whose source event timestamp is older
//! than `EventLoopConfig.task_resume_ttl_seconds` (default 300s) is
//! silently dropped, the recovery envelope is recorded, and a
//! `event.isolation.boundary_violation` diagnostic is published.
//!
//! These tests exercise the isolated-mode isolated-scope-violation
//! path because that is the only rejection site that carries the
//! source event's `ts` into the rejection struct (U3
//! `original_event_id` / `original_ts`). Other rejection sites
//! (policy, execution contract) leave the fields `None` and the
//! freshness filter treats them as fresh — that is the documented
//! fallback so legacy JSONL continues to flow through the existing
//! recovery path.

use super::*;
use std::io::Write;

/// Write a single JSONL event to the events file.
fn write_event(path: &std::path::Path, topic: &str, hat: &str, ts: &str) {
    let json = serde_json::json!({
        "topic": topic,
        "payload": "{}",
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

/// Build a minimal isolated-mode event loop with the `executor` hat
/// publishing `work.ready` and `work.done` (in-scope) and the
/// `executor` hat emitting an out-of-scope `build.done` to trigger
/// the isolated-scope-violation rejection path.
fn make_isolated_loop(events_path: &std::path::Path) -> EventLoop {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics: ["work.done"]
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
    event_loop.initialize("TestU3Ttl");
    event_loop.event_reader = crate::event_reader::EventReader::new(events_path);
    event_loop
}

/// U3.HAPPY: a fresh out-of-scope event (timestamp = now) triggers
/// the isolated-scope-violation path, and the synthetic `task.resume`
/// IS injected (the existing recovery path is preserved for fresh
/// rejections).
#[test]
fn test_u3_fresh_rejection_still_injects_task_resume() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_loop(&events_path);

    // `executor` does NOT publish `build.done` — this is an isolated
    // scope violation. The current time guarantees the rejection is
    // fresh, so the existing `task.resume` path must fire.
    let now = chrono::Utc::now().to_rfc3339();
    write_event(&events_path, "build.done", "executor", &now);

    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    let _ = event_loop.process_events_from_jsonl();

    // A fresh violation must still publish a recovery diagnostic AND
    // a `task.resume` event. We assert on the diagnostic
    // (`event.isolation.boundary_violation`) because the
    // `task.resume` event is built in the same code path and lives
    // on the bus, but the test infrastructure's bus.peek is the
    // canonical surface for either event.
    let hat_ids: Vec<ralph_proto::HatId> = event_loop.bus.hat_ids().cloned().collect();
    let bus_events: Vec<ralph_proto::Event> = hat_ids
        .iter()
        .flat_map(|id| event_loop.bus.peek_pending(id).cloned().unwrap_or_default())
        .collect();

    let task_resume_count = bus_events
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    let boundary_violation_count = bus_events
        .iter()
        .filter(|e| e.topic.as_str() == "event.isolation.boundary_violation")
        .count();

    assert!(
        task_resume_count >= 1,
        "fresh rejection must still inject a task.resume; got {task_resume_count} task.resume and {boundary_violation_count} boundary_violation events"
    );
}

/// U3.STALE: a stale out-of-scope event (timestamp = 10 minutes ago,
/// > default TTL of 300s) triggers the freshness filter. The
/// `task.resume` is dropped, and a `event.isolation.boundary_violation`
/// diagnostic carrying the TTL is published instead.
#[test]
fn test_u3_stale_rejection_drops_task_resume() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_loop(&events_path);

    // 10 minutes ago — well past the default 300s TTL.
    let ten_minutes_ago = (chrono::Utc::now() - chrono::Duration::seconds(600)).to_rfc3339();
    write_event(&events_path, "build.done", "executor", &ten_minutes_ago);

    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    let _ = event_loop.process_events_from_jsonl();

    let hat_ids: Vec<ralph_proto::HatId> = event_loop.bus.hat_ids().cloned().collect();
    let bus_events: Vec<ralph_proto::Event> = hat_ids
        .iter()
        .flat_map(|id| event_loop.bus.peek_pending(id).cloned().unwrap_or_default())
        .collect();

    let task_resume_count = bus_events
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    let boundary_violation_count = bus_events
        .iter()
        .filter(|e| {
            e.topic.as_str() == "event.isolation.boundary_violation"
                && e.payload.contains("stale rejection")
        })
        .count();

    assert_eq!(
        task_resume_count, 0,
        "stale rejection must NOT inject a task.resume; got {task_resume_count}"
    );
    assert!(
        boundary_violation_count >= 1,
        "stale rejection must publish a boundary_violation diagnostic with 'stale rejection' marker; got {boundary_violation_count}"
    );
}

/// U3.TTL-DISABLED: setting `task_resume_ttl_seconds: 0` disables
/// the freshness filter entirely (every rejection is treated as
/// fresh, regardless of `original_ts`). This is the escape hatch
/// operators use to revert the U3 fix in a stuck environment.
#[test]
fn test_u3_ttl_zero_disables_freshness_filter() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_loop(&events_path);
    event_loop.config.event_loop.task_resume_ttl_seconds = Some(0);

    // 1 hour ago — far past any reasonable TTL, but with TTL=0 the
    // filter is off and the rejection flows through.
    let one_hour_ago =
        (chrono::Utc::now() - chrono::Duration::seconds(3600)).to_rfc3339();
    write_event(&events_path, "build.done", "executor", &one_hour_ago);

    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    let _ = event_loop.process_events_from_jsonl();

    let hat_ids: Vec<ralph_proto::HatId> = event_loop.bus.hat_ids().cloned().collect();
    let bus_events: Vec<ralph_proto::Event> = hat_ids
        .iter()
        .flat_map(|id| event_loop.bus.peek_pending(id).cloned().unwrap_or_default())
        .collect();

    let task_resume_count = bus_events
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    assert!(
        task_resume_count >= 1,
        "TTL=0 must disable the freshness filter; expected >= 1 task.resume, got {task_resume_count}"
    );
}

/// U3.MISSING-TS: a rejection whose source event has no timestamp
/// is treated as fresh (the freshness filter cannot compute age
/// without a timestamp). This preserves the legacy recovery path
/// for hand-written or historical JSONL that pre-dates the filter.
#[test]
fn test_u3_missing_original_ts_treated_as_fresh() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_loop(&events_path);

    // An empty string is unparseable by `DateTime::parse_from_rfc3339`
    // — the filter must treat it as fresh and admit the recovery.
    write_event(&events_path, "build.done", "executor", "");

    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    let _ = event_loop.process_events_from_jsonl();

    let hat_ids: Vec<ralph_proto::HatId> = event_loop.bus.hat_ids().cloned().collect();
    let bus_events: Vec<ralph_proto::Event> = hat_ids
        .iter()
        .flat_map(|id| event_loop.bus.peek_pending(id).cloned().unwrap_or_default())
        .collect();

    let task_resume_count = bus_events
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    assert!(
        task_resume_count >= 1,
        "missing/unparseable original_ts must be treated as fresh; expected >= 1 task.resume, got {task_resume_count}"
    );
}

/// U3.CUSTOM-TTL: an operator-tuned TTL (e.g. 60s) drops a
/// 90-second-old rejection that would pass the default 300s TTL.
#[test]
fn test_u3_custom_ttl_respected() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");
    let mut event_loop = make_isolated_loop(&events_path);
    // Tune the TTL to 60 seconds.
    event_loop.config.event_loop.task_resume_ttl_seconds = Some(60);

    // 90 seconds ago — past the tuned TTL but inside the default.
    let ninety_seconds_ago =
        (chrono::Utc::now() - chrono::Duration::seconds(90)).to_rfc3339();
    write_event(&events_path, "build.done", "executor", &ninety_seconds_ago);

    event_loop.state.current_isolated_hat = Some(HatId::new("executor"));

    let _ = event_loop.process_events_from_jsonl();

    let hat_ids: Vec<ralph_proto::HatId> = event_loop.bus.hat_ids().cloned().collect();
    let bus_events: Vec<ralph_proto::Event> = hat_ids
        .iter()
        .flat_map(|id| event_loop.bus.peek_pending(id).cloned().unwrap_or_default())
        .collect();

    let task_resume_count = bus_events
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    assert_eq!(
        task_resume_count, 0,
        "90s-old rejection must be dropped under TTL=60s; got {task_resume_count} task.resume"
    );
}

// ---------------------------------------------------------------------------
// U2 (2026-06-17-001 plan): policy-rejection TTL freshness
//
// The TTL filter that was added for origin-guard isolated-scope violations
// (U3) is now also applied to `event_policy` `RejectWithResume` decisions.
// This closes the gap where stale policy rejections could re-inject
// `task.resume` into the loop long after the source event was emitted.
//
// The tests below exercise the `publish_policy_rejection_resume` path
// through the completion-guard `RejectWithResume` branch.
// ---------------------------------------------------------------------------

/// U2.HAPPY: policy rejects a fresh event (timestamp = now). The
/// `task.resume` IS injected — the TTL filter admits fresh rejections.
#[test]
fn test_u2_fresh_policy_rejection_still_injects_task_resume() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    // Policy rejects events that don't match the declared schema.
    // The `work.done` schema requires 5 fields (`plan_name`,
    // `plan_path`, `task_id`, `task_key`, `step`); an empty payload
    // `{}` produces a `MissingRequiredField` finding that escalates
    // to `RejectWithResume` under the enforce / reject_with_resume
    // policy mode.
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics: ["work.done"]
    business_topics: ["work.ready", "work.done"]
    schemas:
      work.done:
        payload: json_object
        required_fields: ["plan_name", "plan_path", "task_id", "task_key", "step"]
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.ready", "work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("TestU2Fresh");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Write a `work.done` with current timestamp and an empty payload
    // (triggers completion-guard rejection).
    let now = chrono::Utc::now().to_rfc3339();
    let json = serde_json::json!({
        "topic": "work.done",
        "payload": "{}",  // empty — completion guard requires plan_path
        "ts": now,
        "hat": "executor",
    });
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .unwrap()
        .write_all(format!("{}\n", json).as_bytes())
        .unwrap();

    let _ = event_loop.process_events_from_jsonl();

    let hat_ids: Vec<ralph_proto::HatId> = event_loop.bus.hat_ids().cloned().collect();
    let bus_events: Vec<ralph_proto::Event> = hat_ids
        .iter()
        .flat_map(|id| event_loop.bus.peek_pending(id).cloned().unwrap_or_default())
        .collect();

    let task_resume_count = bus_events
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    assert!(
        task_resume_count >= 1,
        "fresh policy rejection must still inject task.resume; got {task_resume_count}"
    );
}

/// U2.STALE: policy rejects an event whose timestamp is 10 minutes ago
/// (default TTL = 300s). The `task.resume` is dropped and an
/// `event.isolation.boundary_violation` diagnostic is published instead.
#[test]
fn test_u2_stale_policy_rejection_drops_task_resume() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics: ["work.done"]
    business_topics: ["work.ready", "work.done"]
    schemas:
      work.done:
        payload: json_object
        required_fields: ["plan_name", "plan_path", "task_id", "task_key", "step"]
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.ready", "work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("TestU2Stale");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // 10 minutes ago — past the default 300s TTL.
    let ten_minutes_ago =
        (chrono::Utc::now() - chrono::Duration::seconds(600)).to_rfc3339();
    let json = serde_json::json!({
        "topic": "work.done",
        "payload": "{}",  // empty — completion guard requires plan_path
        "ts": ten_minutes_ago,
        "hat": "executor",
    });
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .unwrap()
        .write_all(format!("{}\n", json).as_bytes())
        .unwrap();

    let _ = event_loop.process_events_from_jsonl();

    let hat_ids: Vec<ralph_proto::HatId> = event_loop.bus.hat_ids().cloned().collect();
    let bus_events: Vec<ralph_proto::Event> = hat_ids
        .iter()
        .flat_map(|id| event_loop.bus.peek_pending(id).cloned().unwrap_or_default())
        .collect();

    let task_resume_count = bus_events
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    let boundary_violation_count = bus_events
        .iter()
        .filter(|e| {
            e.topic.as_str() == "event.isolation.boundary_violation"
                && e.payload.contains("Policy rejection")
        })
        .count();

    assert_eq!(
        task_resume_count, 0,
        "stale policy rejection must NOT inject task.resume; got {task_resume_count}"
    );
    assert!(
        boundary_violation_count >= 1,
        "stale policy rejection must publish boundary_violation diagnostic; got {boundary_violation_count}"
    );
}

/// U2.MISSING-TS: policy rejects an event with no timestamp field.
// The TTL filter treats missing ts as fresh (backwards-compatible fallback),
// so `task.resume` is injected.
#[test]
fn test_u2_missing_ts_policy_rejection_treated_as_fresh() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics: ["work.done"]
    business_topics: ["work.ready", "work.done"]
    schemas:
      work.done:
        payload: json_object
        required_fields: ["plan_name", "plan_path", "task_id", "task_key", "step"]
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.ready", "work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("TestU2MissingTs");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Write a `work.done` with no `ts` field at all.
    let json = serde_json::json!({
        "topic": "work.done",
        "payload": "{}",
        "hat": "executor",
    });
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .unwrap()
        .write_all(format!("{}\n", json).as_bytes())
        .unwrap();

    let _ = event_loop.process_events_from_jsonl();

    let hat_ids: Vec<ralph_proto::HatId> = event_loop.bus.hat_ids().cloned().collect();
    let bus_events: Vec<ralph_proto::Event> = hat_ids
        .iter()
        .flat_map(|id| event_loop.bus.peek_pending(id).cloned().unwrap_or_default())
        .collect();

    let task_resume_count = bus_events
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    assert!(
        task_resume_count >= 1,
        "policy rejection with missing ts must be treated as fresh; got {task_resume_count} task.resume"
    );
}

/// U2.FUTURE-TS: policy rejects an event whose timestamp is in the future
/// (clock skew or forged ts). The `is_rejection_stale` helper treats
/// future timestamps as stale, so `task.resume` is dropped and a
/// boundary_violation diagnostic is published.
#[test]
fn test_u2_future_ts_policy_rejection_treated_as_stale() {
    let temp_dir = tempfile::tempdir().unwrap();
    let events_path = temp_dir.path().join("events.jsonl");

    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics: ["work.done"]
    business_topics: ["work.ready", "work.done"]
    schemas:
      work.done:
        payload: json_object
        required_fields: ["plan_name", "plan_path", "task_id", "task_key", "step"]
hats:
  executor:
    name: "Executor"
    triggers: ["work.start"]
    publishes: ["work.ready", "work.done"]
"#;
    let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("TestU2FutureTs");
    event_loop.event_reader = crate::event_reader::EventReader::new(&events_path);

    // Timestamp 60s in the future. Stays inside
    // `EventReader::MAX_FUTURE_TS_SKEW_SECS` (300s) so the event
    // survives the read-time future-window check, but the
    // `is_rejection_stale` helper at the policy-rejection site
    // treats any `source_unix > now_unix` as stale.
    let sixty_seconds_future =
        (chrono::Utc::now() + chrono::Duration::seconds(60)).to_rfc3339();
    let json = serde_json::json!({
        "topic": "work.done",
        "payload": "{}",
        "ts": sixty_seconds_future,
        "hat": "executor",
    });
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&events_path)
        .unwrap()
        .write_all(format!("{}\n", json).as_bytes())
        .unwrap();

    let _ = event_loop.process_events_from_jsonl();

    let hat_ids: Vec<ralph_proto::HatId> = event_loop.bus.hat_ids().cloned().collect();
    let bus_events: Vec<ralph_proto::Event> = hat_ids
        .iter()
        .flat_map(|id| event_loop.bus.peek_pending(id).cloned().unwrap_or_default())
        .collect();

    let task_resume_count = bus_events
        .iter()
        .filter(|e| e.topic.as_str() == "task.resume")
        .count();
    let boundary_violation_count = bus_events
        .iter()
        .filter(|e| e.topic.as_str() == "event.isolation.boundary_violation")
        .count();

    assert_eq!(
        task_resume_count, 0,
        "future-ts policy rejection must be treated as stale; got {task_resume_count} task.resume"
    );
    assert!(
        boundary_violation_count >= 1,
        "future-ts policy rejection must publish boundary_violation; got {boundary_violation_count}"
    );
}
