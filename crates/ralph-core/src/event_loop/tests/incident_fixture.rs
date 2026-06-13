//! 2026-06-13-004 U6: incident fixture regression test.
//!
//! Reproduces the 2026-06-13 ce-executor-isolated incident end-to-end
//! from the anonymized JSONL fixture in
//! `tests/fixtures/wave-isolated-dimension-done/`. The 8 events all
//! carry `hat=dimension-reviewer` (correct worker provenance) and
//! `topic=review.dimension.done`. Before U1+U2 they were dropped by
//! the isolated scope check (which used `current_isolated_hat`
//! instead of `event.hat`). After U1+U2 they all flow through.
//!
//! KTD-1: merge writes `hat` per record. KTD-2: isolated scope uses
//! `event.hat` as the scope anchor. KTD-3: same-wave result batch
//! is exempt from the per-turn business-event budget.

use std::io::Write;

use ralph_proto::HatId;

use crate::event_loop::EventLoop;

/// Anonymized fixture path.
const INCIDENT_FIXTURE: &str =
    "tests/fixtures/wave-isolated-dimension-done/8-dimension-done.jsonl";

/// The fixture's wave_id (anonymized from the original
/// `wave_id=w-2026-06-13-001`).
const FIXTURE_WAVE_ID: &str = "w-2026-06-13-001";

/// KTD-2 + KTD-3 happy path: 8 `review.dimension.done` from
/// `dimension-reviewer` workers must all be accepted when
/// `current_isolated_hat=review-coordinator` and the `review-coordinator`
/// is the orchestrator's current isolated hat.
///
/// Pre-U1+U2: 0/8 accepted (scope check anchored on
/// `current_isolated_hat=review-coordinator` which does not publish
/// `review.dimension.done`, so all 8 dropped).
/// Post-U1+U2: 8/8 accepted (U1 merges `hat=dimension-reviewer` per
/// record; U2 scope uses `event.hat` for the scope anchor).
#[test]
fn u6_incident_fixture_eight_dimension_done_all_accepted() {
    // Stage the anonymized fixture into a temp JSONL so the
    // `make_isolated_loop` helper (which expects the JSONL to live
    // at a path the EventReader can scan) finds it.
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let staged = temp_dir.path().join("events.jsonl");
    let fixture = std::fs::read_to_string(INCIDENT_FIXTURE).expect("read incident fixture");
    let mut out = std::fs::File::create(&staged).expect("create staged");
    out.write_all(fixture.as_bytes()).expect("stage fixture");

    // Build the minimal topology. The fixture's `hat` is
    // `dimension-reviewer`, so we need a topology that registers
    // that hat.
    let mut event_loop = make_isolated_topology(&staged);

    // Simulate the runtime: orchestrator is in isolated mode running
    // `review-coordinator`; the wave is fanning out to 8
    // `dimension-reviewer` workers that all publish
    // `review.dimension.done`. The fixture is the re-publish step
    // (what `merge_wave_results_to_events_file` would write).
    event_loop.state.current_isolated_hat = Some(HatId::new("review-coordinator"));

    // Process the staged fixture via the non-wave partition path —
    // the same path the runner takes when wave dispatch is absent
    // (replay, smoke tests, recovery). This is the path the incident
    // ultimately traversed.
    let result = event_loop
        .process_events_from_jsonl()
        .expect("process staged fixture");

    assert!(
        result.had_events,
        "U6 KTD-2: all 8 review.dimension.done must be accepted; got had_events=false"
    );
    let review_done_count = result
        .accepted_events
        .iter()
        .filter(|e| e.topic.as_str() == "review.dimension.done")
        .count();
    assert_eq!(
        review_done_count, 8,
        "U6 KTD-2: 8 review.dimension.done events expected post-fix; got {review_done_count}"
    );

    // KTD-3: the aggregator's pending queue must see all 8 events.
    // `dimension-reviewer` writes `review.dimension.done` and
    // `aggregator` subscribes to it. The wave batch is exempt from
    // the per-turn business-event budget when all events share a
    // `wave_id` (KTD-3 / U3).
    let aggregator_id = HatId::new("aggregator");
    let agg_pending: Vec<ralph_proto::Event> = event_loop
        .bus
        .peek_pending(&aggregator_id)
        .cloned()
        .unwrap_or_default();
    let agg_review_done = agg_pending
        .iter()
        .filter(|e| e.topic.as_str() == "review.dimension.done")
        .count();
    assert_eq!(
        agg_review_done, 8,
        "U6 KTD-3: aggregator must see all 8 review.dimension.done after U3 wave-batch exemption; got {agg_review_done}"
    );
}

/// Negative regression guard: if the merge layer (U1) drops the
/// `hat` field, U2 falls back to `current_isolated_hat` and the
/// scope check drops all 8 events again. This test strips `hat`
/// from the fixture and asserts the events get dropped (proving
/// U1 is the upstream fix, not just U2).
#[test]
fn u6_incident_fixture_without_hat_field_drops_all_eight() {
    let temp_dir = tempfile::tempdir().expect("tempdir");
    let staged = temp_dir.path().join("events.jsonl");
    // Strip `hat` from every line, simulating a pre-U1 merge
    // (worker hat provenance not preserved).
    let fixture = std::fs::read_to_string(INCIDENT_FIXTURE).expect("read incident fixture");
    let mut out = std::fs::File::create(&staged).expect("create staged");
    for line in fixture.lines() {
        let mut value: serde_json::Value =
            serde_json::from_str(line).expect("parse line");
        if let Some(obj) = value.as_object_mut() {
            obj.remove("hat");
        }
        writeln!(out, "{}", serde_json::to_string(&value).unwrap()).unwrap();
    }
    drop(out);

    let mut event_loop = make_isolated_topology(&staged);
    event_loop.state.current_isolated_hat = Some(HatId::new("review-coordinator"));

    let result = event_loop
        .process_events_from_jsonl()
        .expect("process stripped fixture");

    let review_done_count = result
        .accepted_events
        .iter()
        .filter(|e| e.topic.as_str() == "review.dimension.done")
        .count();
    assert_eq!(
        review_done_count, 0,
        "U6 negative: without U1's hat field, U2's scope check should drop all 8 events (proving U1 is the upstream fix); got {review_done_count}"
    );
}

/// Build a minimal isolated-mode event loop with
/// `review-coordinator`, `dimension-reviewer`, and `aggregator`
/// hats wired per `topology.yml`.
fn make_isolated_topology(events_path: &std::path::Path) -> EventLoop {
    let yaml = r#"
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    terminal_topics:
      - "review.coordinator.done"
    business_topics:
      - "review.dimension.done"
  execution_mode: isolated
hats:
  review-coordinator:
    name: "Review Coordinator"
    triggers: ["plan.review.requested"]
    publishes: ["review.coordinator.done"]
  dimension-reviewer:
    name: "Dimension Reviewer"
    triggers: ["review.coordinator.done"]
    publishes: ["review.dimension.done"]
    concurrency: 8
    instructions: "Review a single dimension."
  aggregator:
    name: "Aggregator"
    triggers: ["review.dimension.done"]
    publishes: ["synthesize.complete"]
    aggregate:
      mode: wait_for_all
      timeout: 60
"#;
    let config: crate::config::RalphConfig = serde_yaml::from_str(yaml).unwrap();
    let mut event_loop = EventLoop::new(config);
    event_loop.initialize("U6 incident fixture");
    event_loop.event_reader = crate::event_reader::EventReader::new(events_path);
    event_loop
}
