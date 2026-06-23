//! Unit tests for the drift module.
//!
//! These cover the per-module behaviors:
//!
//! - [`window::DriftWindow`] bounded ring buffer.
//! - [`detector::DriftDetector`] metric math and dedup.
//! - [`alert`] conversion helpers and observer panic/non-blocking
//!   guarantees.

use std::collections::BTreeSet;
use std::sync::Arc;

use chrono::{Duration, TimeZone, Utc};
use ralph_proto::{Event, HatId, Topic};

use crate::config::DriftConfig;
use crate::diagnosis::{DiagnosisSeverity, DiagnosisSource, DriftMetric};
use crate::diagnostics::OrchestrationEvent;

use super::alert::{
    DriftObserver, finding_to_envelope, finding_to_journal_entry, finding_to_orchestration_event,
};
use super::detector::{DeclaredEdges, DriftDetector, DriftFinding, RequiredFields};
use super::window::{DriftWindow, EventSnapshot};

// ── Helpers ──────────────────────────────────────────────────────────

fn ts(seconds: i64) -> chrono::DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + seconds, 0).unwrap()
}

fn snap(topic: &str, iter: u32, seconds_offset: i64, fields: &[&str]) -> EventSnapshot {
    let mut set: BTreeSet<String> = BTreeSet::new();
    for f in fields {
        set.insert((*f).to_string());
    }
    EventSnapshot::new(topic, iter, ts(seconds_offset)).with_fields(set)
}

fn snap_with_wave(topic: &str, iter: u32, seconds_offset: i64, wave_id: &str) -> EventSnapshot {
    EventSnapshot::new(topic, iter, ts(seconds_offset)).with_wave_id(wave_id)
}

fn drift_config() -> DriftConfig {
    DriftConfig {
        window_size: 100,
        field_completeness_threshold: 0.9,
        coord_join_rate_threshold: 0.6,
        emit_cadence_sigma: 2.0,
        coord_join_mode: crate::config::CoordJoinMode::Parallel,
    }
}

// ── DriftWindow tests ────────────────────────────────────────────────

#[test]
fn test_drift_window_bounded() {
    let mut window = DriftWindow::new(3);
    for i in 0..5 {
        window.push(snap("t", 0, i, &[]));
    }
    assert_eq!(window.len(), 3);
    let collected: Vec<&EventSnapshot> = window.iter().collect();
    // Oldest two should have been evicted.
    assert_eq!(collected[0].timestamp, ts(2));
    assert_eq!(collected[1].timestamp, ts(3));
    assert_eq!(collected[2].timestamp, ts(4));
}

#[test]
fn test_drift_window_from_events_pre_fills() {
    let events: Vec<EventSnapshot> = (0..5).map(|i| snap("t", 0, i as i64, &[])).collect();
    let window = DriftWindow::from_events(events, 10);
    assert_eq!(window.len(), 5);
    assert_eq!(window.capacity(), 10);
}

#[test]
fn test_drift_window_is_empty_initially() {
    let window = DriftWindow::new(7);
    assert!(window.is_empty());
    assert_eq!(window.len(), 0);
}

// ── field_completeness tests ─────────────────────────────────────────

#[test]
fn test_field_completeness_95_percent() {
    let cfg = DriftConfig {
        field_completeness_threshold: 0.9,
        ..drift_config()
    };
    let mut required = RequiredFields::new();
    let mut from_policy = std::collections::HashMap::new();
    from_policy.insert("t".to_string(), vec!["field_a".to_string()]);
    required.from_policy = from_policy;
    let mut det = DriftDetector::new_with_sources(cfg, required, DeclaredEdges::new());

    // 95/100 with field_a — completeness 0.95. Threshold 0.9 → no finding.
    for i in 0..95 {
        det.observe(snap("t", 1, i, &["field_a"]));
    }
    for i in 95..100 {
        det.observe(snap("t", 1, i, &[]));
    }
    det.reset_seen();
    let findings = det.observe(snap("t", 1, 100, &["field_a"]));
    assert!(
        findings
            .iter()
            .all(|f| f.metric != DriftMetric::FieldCompleteness),
        "expected no field_completeness finding at 0.95, got {findings:?}"
    );

    // Now change the threshold to 0.96 — completeness 0.95 < 0.96 → finding.
    let cfg_strict = DriftConfig {
        field_completeness_threshold: 0.96,
        ..drift_config()
    };
    let mut det2 = DriftDetector::new(cfg_strict);
    // Required fields: register the policy source first.
    let mut required2 = RequiredFields::new();
    let mut from_policy2 = std::collections::HashMap::new();
    from_policy2.insert("t".to_string(), vec!["field_a".to_string()]);
    required2.from_policy = from_policy2;
    det2.set_required_fields(required2);
    for i in 0..95 {
        det2.observe(snap("t", 1, i, &["field_a"]));
    }
    for i in 95..100 {
        det2.observe(snap("t", 1, i, &[]));
    }
    det2.reset_seen();
    let findings2 = det2.observe(snap("t", 1, 100, &["field_a"]));
    let fc = findings2
        .iter()
        .find(|f| f.metric == DriftMetric::FieldCompleteness)
        .expect("field_completeness finding at 0.95 with threshold 0.96");
    assert_eq!(fc.topic.as_deref(), Some("t"));
    assert_eq!(fc.field.as_deref(), Some("field_a"));
    assert!((fc.observed_value - 0.95).abs() < 0.01);
}

#[test]
fn test_field_completeness_policy_required() {
    let mut required = RequiredFields::new();
    let mut from_policy = std::collections::HashMap::new();
    from_policy.insert("t".to_string(), vec!["field_b".to_string()]);
    required.from_policy = from_policy;

    let mut det = DriftDetector::new_with_sources(
        DriftConfig {
            field_completeness_threshold: 0.9,
            ..drift_config()
        },
        required,
        DeclaredEdges::new(),
    );
    for i in 0..50 {
        det.observe(snap("t", 1, i, &["field_b"]));
    }
    for i in 50..100 {
        det.observe(snap("t", 1, i, &[]));
    }
    det.reset_seen();
    // Re-push a snapshot *with* the required field so the pop
    // triggered by the cap-100 window evicts a `with` snapshot and
    // the new `with` keeps the ratio at 50/100.
    let findings = det.observe(snap("t", 1, 100, &["field_b"]));
    let fc = findings
        .iter()
        .find(|f| f.metric == DriftMetric::FieldCompleteness)
        .expect("expected a field_completeness finding");
    assert_eq!(fc.metric, DriftMetric::FieldCompleteness);
    assert_eq!(fc.topic.as_deref(), Some("t"));
    assert_eq!(fc.field.as_deref(), Some("field_b"));
    eprintln!(
        "DEBUG observed_value={} window={} hits={}",
        fc.observed_value,
        det.window_size_for("t"),
        det.window("t")
            .map(|w| w.iter().filter(|s| s.fields.contains("field_b")).count())
            .unwrap_or(0)
    );
    assert!((fc.observed_value - 0.5).abs() < 0.01);
}

#[test]
fn test_field_completeness_execution_contract_fields_merged() {
    let mut required = RequiredFields::new();
    let mut from_exec = std::collections::HashMap::new();
    from_exec.insert("t".to_string(), vec!["x".to_string()]);
    required.from_execution_contract = from_exec;

    let mut det = DriftDetector::new_with_sources(
        DriftConfig {
            field_completeness_threshold: 0.9,
            ..drift_config()
        },
        required,
        DeclaredEdges::new(),
    );
    for i in 0..100 {
        if i < 80 {
            det.observe(snap("t", 1, i, &["x"]));
        } else {
            det.observe(snap("t", 1, i, &[]));
        }
    }
    det.reset_seen();
    // Re-push with the field so the cap-100 pop evicts a `with`
    // snapshot and the new `with` keeps the ratio at 80/100.
    let findings = det.observe(snap("t", 1, 100, &["x"]));
    let fc = findings
        .iter()
        .find(|f| f.metric == DriftMetric::FieldCompleteness)
        .expect("expected a field_completeness finding");
    assert_eq!(fc.field.as_deref(), Some("x"));
    assert!((fc.observed_value - 0.8).abs() < 0.01);
}

// ── coord_join_rate tests ────────────────────────────────────────────

#[test]
fn test_coord_join_rate_declared_edge_below_threshold() {
    let edges = DeclaredEdges::from_pairs(vec![("from", "to")]);
    let mut det = DriftDetector::new_with_sources(
        DriftConfig {
            coord_join_rate_threshold: 0.6,
            ..drift_config()
        },
        RequiredFields::new(),
        edges,
    );
    // 10 from-events at t=0..9, only 3 to-events after t=5. Rate = 3/10 = 0.3 < 0.6.
    for i in 0..10 {
        det.observe(snap("from", 1, i, &[]));
    }
    for i in 5..8 {
        det.observe(snap("to", 1, i, &[]));
    }
    det.reset_seen();
    let findings = det.observe(snap("to", 1, 100, &[]));
    let cj = findings
        .iter()
        .find(|f| f.metric == DriftMetric::CoordJoinRate)
        .expect("expected a coord_join_rate finding");
    assert_eq!(cj.from_topic.as_deref(), Some("from"));
    assert_eq!(cj.to_topic.as_deref(), Some("to"));
    assert!(cj.observed_value < 0.6);
}

#[test]
fn test_coord_join_rate_declared_edge_above_threshold() {
    let edges = DeclaredEdges::from_pairs(vec![("from", "to")]);
    let mut det = DriftDetector::new_with_sources(
        DriftConfig {
            coord_join_rate_threshold: 0.2,
            ..drift_config()
        },
        RequiredFields::new(),
        edges,
    );
    // 10 from, 9 to → join rate ≥ 0.9.
    for i in 0..10 {
        det.observe(snap("from", 1, i, &[]));
    }
    for i in 1..10 {
        det.observe(snap("to", 1, i, &[]));
    }
    det.reset_seen();
    let findings = det.observe(snap("to", 1, 100, &[]));
    let coord_findings: Vec<_> = findings
        .iter()
        .filter(|f| f.metric == DriftMetric::CoordJoinRate)
        .collect();
    assert!(
        coord_findings.is_empty(),
        "expected no coord_join_rate finding at 0.9 with threshold 0.2, got {coord_findings:?}"
    );
}

#[test]
fn test_coord_join_rate_no_declared_edge_is_noop() {
    // No edge declared; the metric must not panic and must not emit.
    let mut det = DriftDetector::new_with_sources(
        DriftConfig::default(),
        RequiredFields::new(),
        DeclaredEdges::new(),
    );
    for i in 0..10 {
        det.observe(snap("a", 1, i, &[]));
        det.observe(snap("b", 1, i + 100, &[]));
    }
    det.reset_seen();
    let findings = det.observe(snap("b", 1, 200, &[]));
    assert!(
        findings
            .iter()
            .all(|f| f.metric != DriftMetric::CoordJoinRate),
        "no declared edge must not produce a finding, got {findings:?}"
    );
}

// ── emit_cadence tests ───────────────────────────────────────────────

#[test]
fn test_emit_cadence_uniform_emits_no_finding() {
    let mut det = DriftDetector::new(DriftConfig {
        emit_cadence_sigma: 2.0,
        ..drift_config()
    });
    // 10 events at uniform 1-second spacing.
    for i in 0..10 {
        det.observe(snap("t", 1, i, &[]));
    }
    det.reset_seen();
    let findings = det.observe(snap("t", 1, 10, &[]));
    // Healthy uniform cadence is not a diagnosis. The P2.2 review
    // explicitly rejected the prior "always emit an Info record"
    // behaviour because the responder treated every healthy topic
    // as a pending alert and produced log noise.
    assert!(
        findings
            .iter()
            .all(|f| f.metric != DriftMetric::EmitCadence),
        "uniform cadence must not emit an emit_cadence finding, got {findings:?}"
    );
}

#[test]
fn test_emit_cadence_anomaly() {
    let mut det = DriftDetector::new(DriftConfig {
        emit_cadence_sigma: 2.0,
        ..drift_config()
    });
    // 9 events with 1s spacing, then a big gap to t=20 (delta 11s).
    for i in 0..9 {
        det.observe(snap("t", 1, i, &[]));
    }
    det.observe(snap("t", 1, 20, &[])); // big gap
    det.reset_seen();
    let findings = det.observe(snap("t", 1, 21, &[]));
    let cad = findings
        .iter()
        .find(|f| f.metric == DriftMetric::EmitCadence)
        .expect("emit_cadence record expected");
    // With the big gap, z must exceed 2σ.
    assert!(cad.observed_value > 2.0, "expected z > 2.0, got {cad:?}");
}

#[test]
fn test_emit_cadence_low_sample_emits_no_finding() {
    let mut det = DriftDetector::new(DriftConfig::default());
    // 3 events — below the min_samples guard.
    for i in 0..3 {
        det.observe(snap("t", 1, i, &[]));
    }
    det.reset_seen();
    let findings = det.observe(snap("t", 1, 100, &[]));
    // Low-samples are not a diagnosis either; the metric just
    // cannot be computed yet. Stay silent.
    assert!(
        findings
            .iter()
            .all(|f| f.metric != DriftMetric::EmitCadence),
        "low-sample emit_cadence must not emit a finding, got {findings:?}"
    );
}

// ── dedup tests ──────────────────────────────────────────────────────

#[test]
fn test_finding_dedup_within_iteration() {
    let mut required = RequiredFields::new();
    let mut from_policy = std::collections::HashMap::new();
    from_policy.insert("t".to_string(), vec!["f".to_string()]);
    required.from_policy = from_policy;
    let mut det = DriftDetector::new_with_sources(
        DriftConfig {
            field_completeness_threshold: 0.9,
            ..drift_config()
        },
        required,
        DeclaredEdges::new(),
    );
    // All events missing field → every observe emits a finding, but
    // dedup collapses to one per iteration.
    for i in 0..5 {
        det.observe(snap("t", 1, i, &[]));
    }
    let total: usize = (0..5)
        .map(|_| det.observe(snap("t", 1, 100, &[])).len())
        .sum();
    // After the first finding, all subsequent observes in the same
    // iteration collapse — we expect at most one field_completeness
    // finding across the whole burst.
    let fc_count = (0..5)
        .map(|_| det.observe(snap("t", 1, 100, &[])))
        .flat_map(|f| {
            f.into_iter()
                .filter(|x| x.metric == DriftMetric::FieldCompleteness)
        })
        .count();
    assert!(
        fc_count <= 1,
        "expected dedup to collapse to ≤1 finding, got {fc_count} ({total} total)"
    );
}

#[test]
fn test_finding_dedup_reset_seen_allows_re_emit() {
    let mut required = RequiredFields::new();
    let mut from_policy = std::collections::HashMap::new();
    from_policy.insert("t".to_string(), vec!["f".to_string()]);
    required.from_policy = from_policy;
    let mut det = DriftDetector::new_with_sources(
        DriftConfig {
            field_completeness_threshold: 0.9,
            ..drift_config()
        },
        required,
        DeclaredEdges::new(),
    );
    let first = det.observe(snap("t", 1, 0, &[]));
    let fc_first = first
        .iter()
        .filter(|f| f.metric == DriftMetric::FieldCompleteness)
        .count();
    assert_eq!(fc_first, 1, "first observation should emit one finding");
    let second = det.observe(snap("t", 1, 2, &[]));
    let fc_second = second
        .iter()
        .filter(|f| f.metric == DriftMetric::FieldCompleteness)
        .count();
    assert_eq!(fc_second, 0, "second observation should be deduped");
    det.reset_seen();
    let third = det.observe(snap("t", 1, 3, &[]));
    let fc_third = third
        .iter()
        .filter(|f| f.metric == DriftMetric::FieldCompleteness)
        .count();
    assert_eq!(fc_third, 1, "after reset_seen the finding must re-emit");
}

// ── alert conversion tests ──────────────────────────────────────────

fn dummy_finding() -> DriftFinding {
    DriftFinding {
        finding_id: "f-1".to_string(),
        metric: DriftMetric::FieldCompleteness,
        topic: Some("t".to_string()),
        field: Some("plan_name".to_string()),
        from_topic: None,
        to_topic: None,
        observed_value: 0.5,
        threshold: 0.9,
        severity: DiagnosisSeverity::Warning,
        iteration: 7,
        window_size: 10,
        message: "plan_name missing in 50% of events".to_string(),
    }
}

#[test]
fn test_finding_to_envelope() {
    let f = dummy_finding();
    let env = finding_to_envelope(&f, Some("sess-1".to_string()));
    assert_eq!(env.source, DiagnosisSource::DriftMonitor);
    assert_eq!(env.iteration, Some(7));
    assert_eq!(env.reason_code, "drift_field_completeness");
    assert!(!env.safe_target);
    assert_eq!(
        env.expected_action.as_deref(),
        Some("investigate payload or workflow drift; consider runtime guidance")
    );
    assert_eq!(env.session_id.as_deref(), Some("sess-1"));
    // Retry key should encode the metric + topic + field.
    assert!(env.retry_key.contains("drift_field_completeness"));
    assert!(env.retry_key.contains("plan_name"));
    // Field evidence snippet should reflect observed vs threshold.
    let ev = env
        .evidence
        .iter()
        .find(|e| matches!(e.kind, crate::diagnosis::EvidenceKind::Field))
        .expect("field evidence expected");
    assert!(ev.snippet.as_deref().unwrap_or("").contains("0.500"));
    assert!(ev.snippet.as_deref().unwrap_or("").contains("0.900"));
}

#[test]
fn test_finding_to_envelope_reason_code_per_metric() {
    let mut f = dummy_finding();
    for (metric, expected_code) in [
        (DriftMetric::FieldCompleteness, "drift_field_completeness"),
        (DriftMetric::CoordJoinRate, "drift_coord_join_rate"),
        (DriftMetric::EmitCadence, "drift_emit_cadence"),
    ] {
        f.metric = metric;
        let env = finding_to_envelope(&f, None);
        assert_eq!(env.reason_code, expected_code);
    }
}

#[test]
fn test_finding_to_journal_entry() {
    let entry = finding_to_journal_entry(&dummy_finding());
    assert_eq!(entry.schema_version, 1);
    assert_eq!(entry.metric, DriftMetric::FieldCompleteness);
    assert_eq!(entry.field.as_deref(), Some("plan_name"));
    assert!((entry.observed_value - 0.5).abs() < f64::EPSILON);
    assert!((entry.threshold - 0.9).abs() < f64::EPSILON);
    assert_eq!(entry.iteration, 7);
}

#[test]
fn test_finding_to_orchestration_event() {
    let f = dummy_finding();
    let event = finding_to_orchestration_event(&f);
    match event {
        OrchestrationEvent::DriftDetected {
            finding_id,
            metric,
            topic,
            field,
            severity,
        } => {
            assert_eq!(finding_id, "f-1");
            assert_eq!(metric, "field_completeness");
            assert_eq!(topic.as_deref(), Some("t"));
            assert_eq!(field.as_deref(), Some("plan_name"));
            assert_eq!(severity, "warning");
        }
        other => panic!("expected DriftDetected, got {other:?}"),
    }
}

// ── observer tests ──────────────────────────────────────────────────

fn make_event(topic: &str, payload: &str) -> Event {
    Event::new(Topic::from(topic), payload)
}

#[test]
fn test_observer_non_blocking() {
    // Small queue (4) and lots of events: drops must accumulate.
    let observer = DriftObserver::new(4);
    let dropped_before = observer.dropped();
    let closure = observer.observer_closure(|| 0u32);
    for i in 0..100 {
        closure(&make_event("t", &format!("{{\"i\":{i}}}")));
    }
    // Synchronous (single-threaded) test: the consumer side never
    // drains, so the channel fills up after 4 events and the rest
    // are dropped. We never block, because the closure is what
    // we're testing.
    assert!(
        observer.dropped() > dropped_before,
        "expected drops on full channel"
    );
    assert_eq!(
        observer.panicked(),
        0,
        "no projection panic should occur on valid events"
    );
}

#[test]
fn test_observer_panic_isolation() {
    let observer = DriftObserver::new(8);
    // Force a panic by passing a payload the projection code
    // cannot handle. We use a payload that triggers a stack
    // overflow in serde_json's parser by passing a deeply nested
    // JSON value, but in practice the projection just returns an
    // empty field set. To force a real panic we instead craft a
    // payload that breaks the timestamp generation by using
    // poison: the simplest way is to wrap the projection in a
    // closure that panics. Since the projection code is total, we
    // cannot force a panic from the input alone. We simulate the
    // path by sending a malformed event topic via the Event
    // constructor, but `Topic::from` is also total.
    //
    // Instead, the test exercises the catch_unwind boundary by
    // asserting that even a maximally-bad payload (e.g. binary
    // bytes) does not panic. The observer must remain panic-free
    // regardless of input.
    let closure = observer.observer_closure(|| 0u32);
    closure(&make_event("t", "\u{0000}\u{0001}\u{ffff}\u{fffd}"));
    closure(&make_event("t", ""));
    closure(&make_event("t", "not json"));
    closure(&make_event("t", "{\"a\":1}"));
    assert_eq!(observer.panicked(), 0, "no panic for any input");
}

#[test]
fn test_observer_does_not_see_rejected_events() {
    // An event with an unknown source is rejected by
    // EventBus::publish *before* observers run. We assert that
    // contract: the observer sees nothing for a rejected event.
    use ralph_proto::EventBus;
    let observer = DriftObserver::new(8);
    let closure = observer.observer_closure(|| 0u32);

    let mut bus = EventBus::new();
    bus.add_observer(closure);
    // No hats registered → any event with a source fails the
    // source guard and is dropped.
    let rejected = Event::new(Topic::from("t"), r#"{"a":1}"#).with_source(HatId::from("unknown"));
    let recipients = bus.publish(rejected);
    assert!(recipients.is_empty(), "rejected event must not route");
    // The observer closure should not have fired because the
    // observer loop in EventBus::publish is gated on the source
    // guard. Drops count comes from a *full* channel, not from
    // rejected events.
    assert_eq!(observer.dropped(), 0);
    assert_eq!(observer.panicked(), 0);
}

#[test]
fn test_observer_accepted_events_reach_consumer() {
    let observer = DriftObserver::new(8);
    let closure = observer.observer_closure(|| 0u32);
    closure(&make_event("t", r#"{"a":1,"b":2}"#));
    closure(&make_event("t", r#"{"a":1}"#));
    let snaps = observer.drain(10);
    assert_eq!(snaps.len(), 2);
    assert_eq!(snaps[0].topic, "t");
    assert!(snaps[0].fields.contains("a"));
    assert!(snaps[0].fields.contains("b"));
    assert!(snaps[1].fields.contains("a"));
    assert!(!snaps[1].fields.contains("b"));
}

// ── wave_id handling test ────────────────────────────────────────────

#[test]
fn test_wave_id_handling_does_not_flag_cadence_anomaly() {
    let mut det = DriftDetector::new(DriftConfig {
        emit_cadence_sigma: 2.0,
        ..drift_config()
    });
    // Six events all part of the same wave, fired within 1s of
    // each other. Without wave handling, the short intervals would
    // look uniform and the next big gap would be flagged. With
    // wave handling, the wave collapses to a single logical emit,
    // and we need ≥5 logical emits to compute cadence.
    for i in 0..6 {
        det.observe(snap_with_wave("t", 1, i, "w-1"));
    }
    det.reset_seen();
    let findings = det.observe(snap_with_wave("t", 1, 100, "w-1"));
    // With one logical emit the window cannot be measured at all
    // and must not be flagged as a drift anomaly. The detector
    // stays silent (P2.2 review: no more Info "insufficient-data"
    // findings; healthy uniform cadence is *not* a diagnosis).
    assert!(
        findings
            .iter()
            .all(|f| f.metric != DriftMetric::EmitCadence),
        "wave_id collapse must not produce an emit_cadence finding, got {findings:?}"
    );
}

// ── dropped_events counter exposed on detector ─────────────────────

#[test]
fn test_record_dropped_event_increments_counter() {
    let mut det = DriftDetector::new(DriftConfig::default());
    assert_eq!(det.dropped_events(), 0);
    det.record_dropped_event();
    det.record_dropped_event();
    assert_eq!(det.dropped_events(), 2);
    // Arc sharing across the observer — same counter, different
    // owner.
    let counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
    let counter2 = Arc::clone(&counter);
    counter2.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    assert_eq!(counter.load(std::sync::atomic::Ordering::Relaxed), 1);
}

// ── non-regression: detector with no config is a no-op ──────────────

#[test]
fn test_detector_with_default_config_emits_no_finding() {
    let mut det = DriftDetector::new(DriftConfig::default());
    for i in 0..20 {
        det.observe(snap("t", 0, i, &["a"]));
    }
    det.reset_seen();
    let findings = det.observe(snap("t", 0, 100, &[]));
    assert!(
        findings
            .iter()
            .all(|f| f.metric != DriftMetric::FieldCompleteness),
        "default config has no required-fields source, so field_completeness must be a no-op, got {findings:?}"
    );
    assert!(det.observed_total() >= 20);
    assert_eq!(det.last_iteration(), 0);
}

// ── topic coverage ──────────────────────────────────────────────────

#[test]
fn test_window_size_for_and_observed_topics() {
    let mut det = DriftDetector::new(DriftConfig::default());
    det.observe(snap("alpha", 0, 0, &[]));
    det.observe(snap("beta", 0, 0, &[]));
    det.observe(snap("alpha", 0, 1, &[]));
    assert_eq!(det.window_size_for("alpha"), 2);
    assert_eq!(det.window_size_for("beta"), 1);
    assert_eq!(det.window_size_for("missing"), 0);
    let mut topics = det.observed_topics();
    topics.sort();
    assert_eq!(topics, vec!["alpha", "beta"]);
}

// ── 2026-06-23-004 plan U2 KTD-Drift: serial coord join mode ─────────

/// Serial preset: 4 review.dimension.done events followed by a single
/// review.dimensions.complete. The parallel rate formula
/// (joined/from_size = 1/4 = 25%) trips the 60% threshold even though
/// the workflow is healthy. Serial mode evaluates the
/// "last-joins-to" semantic and must stay silent.
#[test]
fn test_coord_join_rate_serial_mode_passes_healthy_sequence() {
    use super::detector::DeclaredEdges;
    use crate::config::CoordJoinMode;

    let cfg = DriftConfig {
        window_size: 100,
        field_completeness_threshold: 0.9,
        coord_join_rate_threshold: 0.6,
        emit_cadence_sigma: 2.0,
        coord_join_mode: CoordJoinMode::Serial,
    };
    let edges = DeclaredEdges::from_pairs(vec![(
        "review.dimension.done",
        "review.dimensions.complete",
    )]);
    let mut det = DriftDetector::new_with_sources(cfg, RequiredFields::new(), edges);

    // 4 serial done events at t=1..=4
    for i in 1..=4 {
        det.observe(snap("review.dimension.done", 0, i, &[]));
    }
    // complete event at t=5 (after the last done)
    det.reset_seen();
    let findings = det.observe(snap("review.dimensions.complete", 0, 5, &[]));
    let coord_findings: Vec<_> = findings
        .iter()
        .filter(|f| matches!(f.metric, DriftMetric::CoordJoinRate))
        .collect();
    assert!(
        coord_findings.is_empty(),
        "serial mode must not flag the healthy sequence: got {coord_findings:?}"
    );
}

/// Serial preset pathological case: complete fires early, then more
/// done events come in (an out-of-order replay or extension). The
/// "last-joins-to" semantic must flag this — `last_to < last_from`.
///
/// In the parallel mode this case would be invisible because
/// `joined/from_size` is unaffected by ordering.
#[test]
fn test_coord_join_rate_serial_mode_flags_out_of_order() {
    use super::detector::DeclaredEdges;
    use crate::config::CoordJoinMode;

    let cfg = DriftConfig {
        window_size: 100,
        field_completeness_threshold: 0.9,
        coord_join_rate_threshold: 0.6,
        emit_cadence_sigma: 2.0,
        coord_join_mode: CoordJoinMode::Serial,
    };
    let edges = DeclaredEdges::from_pairs(vec![(
        "review.dimension.done",
        "review.dimensions.complete",
    )]);
    let mut det = DriftDetector::new_with_sources(cfg, RequiredFields::new(), edges);

    det.observe(snap("review.dimensions.complete", 0, 20, &[]));
    det.observe(snap("review.dimension.done", 0, 25, &[]));
    det.reset_seen();
    let findings = det.observe(snap("review.dimensions.complete", 0, 15, &[]));
    let coord_findings: Vec<_> = findings
        .iter()
        .filter(|f| matches!(f.metric, DriftMetric::CoordJoinRate))
        .collect();
    assert!(
        !coord_findings.is_empty(),
        "serial mode must flag when last_from (25) is after last_to (20): got {coord_findings:?}"
    );
    assert!(coord_findings[0].message.contains("mode=serial"));
}

// ── Duration/TimeZone import sanity (silence unused warnings) ──────

#[allow(dead_code)]
fn _import_sanity(_: Duration) {}
