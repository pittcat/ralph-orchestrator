//! Wave event detection from JSONL events.
//!
//! Groups events by wave_id, validates consistency, and resolves
//! the target hat for wave execution.

use crate::config::HatConfig;
use crate::event_reader::Event;
use crate::hat_registry::HatRegistry;
use ralph_proto::HatId;
use std::collections::HashMap;

/// Whether to accept partial waves (fewer events than `wave_total`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialWavePolicy {
    /// Only accept complete waves (all `wave_total` events present).
    RequireComplete,
    /// Accept partial waves when some events are present but fewer than
    /// `wave_total`.  The dispatcher uses this after the staleness
    /// threshold (80% of aggregate_timeout) has been reached.
    AllowPartial,
}

/// A detected wave ready for execution.
#[derive(Debug)]
pub struct DetectedWave {
    /// Wave correlation ID.
    pub wave_id: String,
    /// Hat that should process these events.
    pub target_hat: HatId,
    /// Configuration for the target hat.
    pub hat_config: HatConfig,
    /// Individual events in this wave, ordered by wave_index.
    pub events: Vec<Event>,
    /// Total expected events in the wave (may be greater than `events.len()`
    /// when this is a partial wave).
    pub total: u32,
    /// Whether this wave was detected with fewer events than `total`.
    /// When true, the aggregator should note that some workers did not
    /// report back and list missing dimensions in Coverage.
    pub partial: bool,
}

impl DetectedWave {
    /// Returns the effective timeout in seconds for wave workers.
    ///
    /// Priority: hat.timeout > hat.aggregate.timeout > 300s default.
    pub fn timeout_secs(&self) -> u64 {
        self.hat_config
            .timeout
            .map(u64::from)
            .or_else(|| {
                self.hat_config
                    .aggregate
                    .as_ref()
                    .map(|a| u64::from(a.timeout))
            })
            .unwrap_or(300)
    }
}

/// Attempt to build a validated `DetectedWave` from a group of events sharing
/// the same `wave_id`.
///
/// When `policy` is [`PartialWavePolicy::AllowPartial`], waves with fewer
/// events than `wave_total` are accepted (marked `partial: true`).  This is
/// used by the dispatcher after the staleness threshold (80% of
/// aggregate_timeout) to force-dispatch whatever results have arrived so far.
fn try_build_wave(
    wave_id: &str,
    wave_events: Vec<&Event>,
    registry: &HatRegistry,
    policy: PartialWavePolicy,
) -> Option<DetectedWave> {
    let first = wave_events.first()?;
    let topic = &first.topic;
    let wave_total = first.wave_total?;

    if wave_total == 0 {
        tracing::warn!(wave_id, "wave_total must be > 0; skipping wave");
        return None;
    }

    for event in &wave_events {
        if event.topic != *topic {
            tracing::warn!(
                wave_id,
                expected_topic = %topic,
                actual_topic = %event.topic,
                "Inconsistent topic in wave events, skipping wave"
            );
            return None;
        }
        if event.wave_total != Some(wave_total) {
            tracing::warn!(
                wave_id,
                "Inconsistent wave_total in wave events, skipping wave"
            );
            return None;
        }
        match event.wave_index {
            Some(idx) if idx < wave_total => {}
            Some(idx) => {
                tracing::warn!(
                    wave_id,
                    wave_index = idx,
                    wave_total,
                    "wave_index out of range; skipping wave"
                );
                return None;
            }
            None => {
                tracing::warn!(wave_id, "wave event missing wave_index; skipping wave");
                return None;
            }
        }
    }

    let is_partial = wave_events.len() as u32 != wave_total;
    if is_partial && policy == PartialWavePolicy::RequireComplete {
        tracing::warn!(
            wave_id,
            expected = wave_total,
            actual = wave_events.len() as u32,
            "wave batch size does not match wave_total; skipping wave"
        );
        return None;
    }

    if is_partial {
        tracing::info!(
            wave_id,
            expected = wave_total,
            actual = wave_events.len() as u32,
            "Accepting partial wave (AllowPartial policy)"
        );
    }

    // Resolve target hat from the event topic
    let target_hat_id = registry.find_by_trigger(topic)?;
    let hat_config = registry.get_config(target_hat_id)?.clone();

    // Only trigger wave execution for hats with concurrency > 1
    if hat_config.concurrency <= 1 {
        return None;
    }

    // Sort events by wave_index for deterministic ordering
    let mut sorted_events: Vec<Event> = wave_events.into_iter().cloned().collect();
    sorted_events.sort_by_key(|e| e.wave_index.unwrap_or(0));

    Some(DetectedWave {
        wave_id: wave_id.to_string(),
        target_hat: target_hat_id.clone(),
        hat_config,
        events: sorted_events,
        total: wave_total,
        partial: is_partial,
    })
}

/// Detect wave events from a set of events.
///
/// Groups events by wave_id, validates that all events in a wave are consistent
/// (same topic, matching wave_total), and resolves the target hat from the registry.
///
/// v1: Returns the first detected wave (one wave per iteration).
/// Events without wave metadata are ignored.
/// Only complete waves are returned (use [`detect_all_wave_events_with_policy`]
/// for partial wave support).
pub fn detect_wave_events(events: &[Event], registry: &HatRegistry) -> Option<DetectedWave> {
    // Group events by wave_id
    let mut wave_groups: HashMap<&str, Vec<&Event>> = HashMap::new();
    for event in events {
        if let Some(ref wave_id) = event.wave_id {
            wave_groups.entry(wave_id.as_str()).or_default().push(event);
        }
    }

    if wave_groups.is_empty() {
        return None;
    }

    // v1: Take the lexicographically first wave_id (deterministic, one wave per iteration)
    let wave_id = *wave_groups.keys().min()?;
    if wave_groups.len() > 1 {
        tracing::warn!(
            selected = wave_id,
            total_waves = wave_groups.len(),
            "Multiple waves detected in single iteration, processing only the first"
        );
    }
    let wave_events = wave_groups.remove(wave_id)?;
    try_build_wave(wave_id, wave_events, registry, PartialWavePolicy::RequireComplete)
}

/// Detect **all** valid wave events from a set of events.
///
/// Unlike [`detect_wave_events`], this returns every well-formed, complete wave
/// found in the event batch — not just the lexicographically first one.  This
/// prevents silent drops when a hat emits multiple waves in a single iteration
/// (e.g. review-coordinator retrying after an empty payload).
///
/// Waves are sorted by `wave_id` for deterministic execution order.
/// Events without wave metadata, or belonging to an invalid/incomplete wave,
/// are ignored.
///
/// Only complete waves are returned.  For partial wave support (after
/// staleness threshold), use [`detect_all_wave_events_with_policy`].
pub fn detect_all_wave_events(events: &[Event], registry: &HatRegistry) -> Vec<DetectedWave> {
    detect_all_wave_events_with_policy(events, registry, PartialWavePolicy::RequireComplete)
}

/// Detect **all** valid wave events from a set of events, with configurable
/// partial-wave policy.
///
/// When `policy` is [`PartialWavePolicy::AllowPartial`], waves with fewer
/// events than `wave_total` are also returned (marked `partial: true`).
/// The dispatcher calls this after the staleness threshold to force-dispatch
/// whatever results have arrived.
pub fn detect_all_wave_events_with_policy(
    events: &[Event],
    registry: &HatRegistry,
    policy: PartialWavePolicy,
) -> Vec<DetectedWave> {
    // Group events by wave_id
    let mut wave_groups: HashMap<&str, Vec<&Event>> = HashMap::new();
    for event in events {
        if let Some(ref wave_id) = event.wave_id {
            wave_groups.entry(wave_id.as_str()).or_default().push(event);
        }
    }

    let mut detected = Vec::new();
    for (wave_id, wave_events) in wave_groups {
        if let Some(wave) = try_build_wave(wave_id, wave_events, registry, policy) {
            detected.push(wave);
        }
    }

    // Deterministic ordering by wave_id
    detected.sort_by(|a, b| a.wave_id.cmp(&b.wave_id));
    detected
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RalphConfig;

    fn make_wave_event(topic: &str, payload: &str, wave_id: &str, index: u32, total: u32) -> Event {
        Event {
            topic: topic.to_string(),
            payload: Some(payload.to_string()),
            ts: "2025-01-01T00:00:00Z".to_string(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: Some(wave_id.to_string()),
            wave_index: Some(index),
            wave_total: Some(total),
        }
    }

    fn make_registry_with_concurrent_hat() -> HatRegistry {
        let yaml = r#"
            hats:
              reviewer:
                name: "Reviewer"
                triggers: ["review.file"]
                publishes: ["review.done"]
                instructions: "Review files"
                concurrency: 4
        "#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        HatRegistry::from_config(&config)
    }

    fn make_registry_with_sequential_hat() -> HatRegistry {
        let yaml = r#"
            hats:
              builder:
                name: "Builder"
                triggers: ["build.start"]
                publishes: ["build.done"]
                instructions: "Build code"
        "#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        HatRegistry::from_config(&config)
    }

    #[test]
    fn test_detect_wave_events_basic() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![
            make_wave_event("review.file", "src/main.rs", "w-abc", 0, 3),
            make_wave_event("review.file", "src/lib.rs", "w-abc", 1, 3),
            make_wave_event("review.file", "src/config.rs", "w-abc", 2, 3),
        ];

        let wave = detect_wave_events(&events, &registry).unwrap();
        assert_eq!(wave.wave_id, "w-abc");
        assert_eq!(wave.total, 3);
        assert_eq!(wave.events.len(), 3);
        assert!(!wave.partial, "complete wave must not be marked partial");
        assert_eq!(wave.target_hat.as_str(), "reviewer");
        assert_eq!(wave.hat_config.concurrency, 4);
    }

    #[test]
    fn test_detect_ignores_non_wave_events() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![Event {
            topic: "review.file".to_string(),
            payload: Some("src/main.rs".to_string()),
            ts: "2025-01-01T00:00:00Z".to_string(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: None,
            wave_index: None,
            wave_total: None,
        }];

        assert!(detect_wave_events(&events, &registry).is_none());
    }

    #[test]
    fn test_detect_ignores_sequential_hat() {
        let registry = make_registry_with_sequential_hat();
        let events = vec![
            make_wave_event("build.start", "payload", "w-abc", 0, 2),
            make_wave_event("build.start", "payload", "w-abc", 1, 2),
        ];

        // Hat has concurrency=1 (default), so wave detection returns None
        assert!(detect_wave_events(&events, &registry).is_none());
    }

    #[test]
    fn test_detect_rejects_inconsistent_topics() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![
            make_wave_event("review.file", "src/main.rs", "w-abc", 0, 2),
            make_wave_event("review.other", "src/lib.rs", "w-abc", 1, 2),
        ];

        assert!(detect_wave_events(&events, &registry).is_none());
    }

    #[test]
    fn test_detect_sorts_by_index() {
        let registry = make_registry_with_concurrent_hat();
        // Events arrive out of order
        let events = vec![
            make_wave_event("review.file", "third", "w-abc", 2, 3),
            make_wave_event("review.file", "first", "w-abc", 0, 3),
            make_wave_event("review.file", "second", "w-abc", 1, 3),
        ];

        let wave = detect_wave_events(&events, &registry).unwrap();
        assert_eq!(wave.events[0].payload.as_deref(), Some("first"));
        assert_eq!(wave.events[1].payload.as_deref(), Some("second"));
        assert_eq!(wave.events[2].payload.as_deref(), Some("third"));
    }

    #[test]
    fn test_empty_events_returns_none() {
        let registry = make_registry_with_concurrent_hat();
        assert!(detect_wave_events(&[], &registry).is_none());
    }

    #[test]
    fn test_unknown_topic_returns_none() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![make_wave_event("unknown.topic", "payload", "w-abc", 0, 1)];

        assert!(detect_wave_events(&events, &registry).is_none());
    }

    // ---- P6: stricter wave shape validation tests ----

    fn wave_event_with_index_total(
        topic: &str,
        wave_id: &str,
        index: Option<u32>,
        total: Option<u32>,
    ) -> Event {
        Event {
            topic: topic.to_string(),
            payload: Some("p".to_string()),
            ts: "2025-01-01T00:00:00Z".to_string(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: Some(wave_id.to_string()),
            wave_index: index,
            wave_total: total,
        }
    }

    #[test]
    fn test_detect_rejects_wave_total_zero() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![wave_event_with_index_total(
            "review.file",
            "w-zero",
            Some(0),
            Some(0),
        )];
        assert!(detect_wave_events(&events, &registry).is_none());
    }

    #[test]
    fn test_detect_rejects_wave_index_equal_total() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![wave_event_with_index_total(
            "review.file",
            "w-eq",
            Some(2),
            Some(2),
        )];
        assert!(detect_wave_events(&events, &registry).is_none());
    }

    #[test]
    fn test_detect_rejects_wave_index_above_total() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![wave_event_with_index_total(
            "review.file",
            "w-above",
            Some(5),
            Some(2),
        )];
        assert!(detect_wave_events(&events, &registry).is_none());
    }

    #[test]
    fn test_detect_rejects_wave_missing_index() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![wave_event_with_index_total(
            "review.file",
            "w-no-idx",
            None,
            Some(1),
        )];
        assert!(detect_wave_events(&events, &registry).is_none());
    }

    #[test]
    fn test_detect_rejects_batch_size_mismatch() {
        let registry = make_registry_with_concurrent_hat();
        // 2 events but total=3
        let events = vec![
            make_wave_event("review.file", "p1", "w-mm", 0, 3),
            make_wave_event("review.file", "p2", "w-mm", 1, 3),
        ];
        assert!(detect_wave_events(&events, &registry).is_none());
    }

    #[test]
    fn test_detect_rejects_inconsistent_wave_total() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![
            wave_event_with_index_total("review.file", "w-it", Some(0), Some(2)),
            wave_event_with_index_total("review.file", "w-it", Some(1), Some(3)),
        ];
        assert!(detect_wave_events(&events, &registry).is_none());
    }

    #[test]
    fn test_detect_accepts_complete_well_formed_wave() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![
            make_wave_event("review.file", "a", "w-good", 0, 2),
            make_wave_event("review.file", "b", "w-good", 1, 2),
        ];
        let wave = detect_wave_events(&events, &registry).expect("valid wave must be detected");
        assert_eq!(wave.total, 2);
        assert_eq!(wave.events.len(), 2);
        assert!(!wave.partial);
    }

    // ---- detect_all_wave_events tests (Bug #1 regression) ----

    #[test]
    fn test_detect_all_returns_multiple_waves() {
        let registry = make_registry_with_concurrent_hat();
        // Three independent waves in one batch
        let events = vec![
            make_wave_event("review.file", "a1", "w-alpha", 0, 2),
            make_wave_event("review.file", "a2", "w-alpha", 1, 2),
            make_wave_event("review.file", "b1", "w-beta", 0, 1),
            make_wave_event("review.file", "c1", "w-gamma", 0, 2),
            make_wave_event("review.file", "c2", "w-gamma", 1, 2),
        ];

        let waves = detect_all_wave_events(&events, &registry);
        assert_eq!(waves.len(), 3, "expected three valid waves");
        assert_eq!(waves[0].wave_id, "w-alpha");
        assert_eq!(waves[0].total, 2);
        assert_eq!(waves[1].wave_id, "w-beta");
        assert_eq!(waves[1].total, 1);
        assert_eq!(waves[2].wave_id, "w-gamma");
        assert_eq!(waves[2].total, 2);
    }

    #[test]
    fn test_detect_all_skips_invalid_waves_but_keeps_valid_ones() {
        let registry = make_registry_with_concurrent_hat();
        // w-broken has batch size mismatch; w-good is valid
        let events = vec![
            make_wave_event("review.file", "bad", "w-broken", 0, 3),
            make_wave_event("review.file", "ok1", "w-good", 0, 2),
            make_wave_event("review.file", "ok2", "w-good", 1, 2),
        ];

        let waves = detect_all_wave_events(&events, &registry);
        assert_eq!(waves.len(), 1, "only the valid wave should be returned");
        assert_eq!(waves[0].wave_id, "w-good");
    }

    #[test]
    fn test_detect_all_returns_empty_when_no_waves() {
        let registry = make_registry_with_concurrent_hat();
        let waves = detect_all_wave_events(&[], &registry);
        assert!(waves.is_empty());
    }

    #[test]
    fn test_detect_all_ignores_sequential_hat() {
        let registry = make_registry_with_sequential_hat();
        let events = vec![
            make_wave_event("build.start", "p1", "w-seq", 0, 2),
            make_wave_event("build.start", "p2", "w-seq", 1, 2),
        ];
        let waves = detect_all_wave_events(&events, &registry);
        assert!(waves.is_empty(), "sequential hats should not produce waves");
    }

    #[test]
    fn test_detect_all_skips_incomplete_waves() {
        let registry = make_registry_with_concurrent_hat();
        // w-partial is missing index 1 (only has 0 and 2 out of 3)
        let events = vec![
            make_wave_event("review.file", "p0", "w-partial", 0, 3),
            make_wave_event("review.file", "p2", "w-partial", 2, 3),
        ];
        let waves = detect_all_wave_events(&events, &registry);
        assert!(waves.is_empty(), "incomplete wave should be skipped");
    }

    // ---- U1: partial wave detection tests ----

    #[test]
    fn test_allow_partial_accepts_incomplete_wave() {
        let registry = make_registry_with_concurrent_hat();
        // 2 out of 3 events present — RequireComplete rejects, AllowPartial accepts
        let events = vec![
            make_wave_event("review.file", "p0", "w-part", 0, 3),
            make_wave_event("review.file", "p2", "w-part", 2, 3),
        ];

        // RequireComplete → no wave
        let waves = detect_all_wave_events(&events, &registry);
        assert!(waves.is_empty(), "RequireComplete must skip partial wave");

        // AllowPartial → wave with partial=true
        let waves = detect_all_wave_events_with_policy(
            &events,
            &registry,
            PartialWavePolicy::AllowPartial,
        );
        assert_eq!(waves.len(), 1);
        assert!(waves[0].partial, "partial wave must be marked");
        assert_eq!(waves[0].total, 3, "total must reflect expected count");
        assert_eq!(waves[0].events.len(), 2, "events only contains arrived results");
    }

    #[test]
    fn test_allow_partial_complete_wave_not_marked_partial() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![
            make_wave_event("review.file", "a", "w-full", 0, 2),
            make_wave_event("review.file", "b", "w-full", 1, 2),
        ];
        let waves = detect_all_wave_events_with_policy(
            &events,
            &registry,
            PartialWavePolicy::AllowPartial,
        );
        assert_eq!(waves.len(), 1);
        assert!(!waves[0].partial, "complete wave must not be marked partial even with AllowPartial");
    }

    #[test]
    fn test_allow_partial_zero_events_still_skipped() {
        let registry = make_registry_with_concurrent_hat();
        // No events at all — nothing to detect
        let waves = detect_all_wave_events_with_policy(
            &[],
            &registry,
            PartialWavePolicy::AllowPartial,
        );
        assert!(waves.is_empty());
    }
}
