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
    /// Aggregate timeout of the consumer hat(s) that subscribe to the
    /// worker hat's published topics.  Used by the dispatcher as the
    /// wave-level deadline so a worker hat does not need to carry its
    /// own `aggregate` block (which is reserved for aggregator hats).
    pub consumer_aggregate_timeout: Option<u64>,
}

impl DetectedWave {
    /// Returns the effective **per-worker** timeout in seconds.
    ///
    /// This governs how long a single wave worker may run before the
    /// dispatcher aborts it.  It is derived from `hat.timeout`.
    ///
    /// Default: 300s.
    pub fn per_worker_timeout_secs(&self) -> u64 {
        self.hat_config.timeout.map(u64::from).unwrap_or(300)
    }

    /// Returns the effective **wave-level aggregate** timeout in seconds.
    ///
    /// This governs how long the dispatcher waits for the whole wave
    /// (all workers / all batches) to finish before declaring an
    /// aggregate deadline exceeded.
    ///
    /// Priority:
    ///   1. `hat.aggregate.timeout` if explicitly configured on the worker hat.
    ///   2. `consumer_aggregate_timeout` inherited from the aggregator hat
    ///      that consumes the worker hat's output (e.g. review-synthesizer:300).
    ///   3. Per-worker timeout (`per_worker_timeout_secs`) as a fallback,
    ///      scaled by the dispatcher's batch calculation.
    ///
    /// Default fallback: 300s.
    pub fn aggregate_timeout_secs(&self) -> u64 {
        self.hat_config
            .aggregate
            .as_ref()
            .map(|a| u64::from(a.timeout))
            .or(self.consumer_aggregate_timeout)
            .unwrap_or_else(|| self.per_worker_timeout_secs())
    }

    /// Returns true when the hat config explicitly sets an aggregate
    /// timeout, distinguishing "I want a wave-level cap" from "fall back
    /// to per-worker scaling".
    pub fn has_explicit_aggregate_timeout(&self) -> bool {
        self.hat_config.aggregate.is_some()
    }

    /// Backwards-compatible alias for `per_worker_timeout_secs`.
    ///
    /// Display / RPC code that reports "worker timeout" should use this;
    /// dispatch deadline code should use `per_worker_timeout_secs` and
    /// `aggregate_timeout_secs` explicitly.
    pub fn timeout_secs(&self) -> u64 {
        self.per_worker_timeout_secs()
    }

    // ─── 2026-07-25-006 plan U3: idle heartbeat accessors ───
    //
    // The dispatcher / worker never reach into `hat_config` for the
    // new idle fields.  They go through these helpers so the
    // "None / Some(0) ⇒ disabled" / "Some(n>0) ⇒ enabled" / cap
    // resolution is centralized.

    /// Effective idle heartbeat window in seconds.
    ///
    /// Returns `None` when the hat has no idle heartbeat configured
    /// (`idle_heartbeat_secs` is `None` *or* `Some(0)`); callers
    /// must treat this as "idle mode disabled" and rely solely on
    /// the `timeout` wall-clock. Returns the explicit value
    /// otherwise.
    pub fn idle_heartbeat_secs(&self) -> Option<u32> {
        match self.hat_config.idle_heartbeat_secs {
            Some(0) | None => None,
            Some(n) => Some(n),
        }
    }

    /// Whether the wave worker should run the dual-clock lease.
    /// True when `idle_heartbeat_secs` is `Some(n)` with `n > 0`.
    pub fn idle_enabled(&self) -> bool {
        self.idle_heartbeat_secs().is_some()
    }

    /// Effective cap on consecutive weak-only heartbeat renewals.
    ///
    /// Resolution:
    /// 1. Explicit `Some(n)` on the hat — use as-is.
    /// 2. `None` or `Some(0)` — fall through to the operator
    ///    default in [`default_idle_weak_signal_cap`] (8).
    ///
    /// The cap exists to bound pathological weak-only streams;
    /// setting it to `0` disables weak-signal renewals entirely
    /// (only strong signals refresh the lease).
    pub fn idle_weak_signal_cap(&self) -> u32 {
        match self.hat_config.idle_weak_signal_cap {
            Some(0) | None => default_idle_weak_signal_cap(),
            Some(n) => n,
        }
    }
}

/// 2026-07-25-006 plan U3 / KTD7: operator-visible default for the
/// idle weak-signal cap when the hat does not pin one.  Mirrors
/// the value recommended for `worker` / `fix-worker` /
/// `review-batch-worker` so a hat with idle heartbeat enabled but
/// no explicit cap still gets a sensible ceiling.
pub fn default_idle_weak_signal_cap() -> u32 {
    8
}

/// Typed reason a wave was rejected by the detector.
///
/// Replaces the historical silent `Option<DetectedWave>` path where cap
/// overshoot and other reasons were all compressed into `None` and
/// silently swallowed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WaveRejection {
    /// wave_total == 0
    ZeroTotal,
    /// wave_total exceeded the configured `max_wave_total` cap.
    /// This is the U2 cap rejection — caller MUST publish one
    /// `plan.blocked` with structured payload, regardless of how many
    /// events were in the malformed batch.
    TotalExceedsCap { actual: u32, cap: u32 },
    /// Events within the same wave_id carry different topics.
    InconsistentTopic,
    /// Events within the same wave_id carry different wave_total values.
    InconsistentTotal,
    /// Some events are missing `wave_index`.
    MissingIndex,
    /// Some event has `wave_index >= wave_total`.
    IndexOutOfRange,
    /// No target hat registered for the wave topic, or target hat
    /// has concurrency <= 1 (sequential target — wave path is invalid).
    NoTargetHat,
    /// Hat is registered for the topic but `concurrency <= 1`.
    SequentialTarget,
    /// Isolated mode: a wave event targets a topic that the current
    /// isolated hat is not allowed to publish. The whole wave is dropped.
    /// See U4 plan §4 KTD-U4-1 / A3.
    IsolatedScopeViolation {
        wave_id: String,
        topic: String,
        isolated_hat: String,
    },
    /// Isolated mode: a second distinct `wave_id` was observed in the
    /// same read batch, violating the "one business emission per
    /// isolated activation" rule. The second wave is dropped.
    /// See U4 plan §3 KTD-U4-2 / A3.
    IsolatedMultipleBusinessEmissions {
        wave_id: String,
        isolated_hat: String,
    },
}

/// A wave that the detector decided not to dispatch, with the reason.
#[derive(Debug, Clone)]
pub struct RejectedWave {
    pub wave_id: String,
    pub topic: String,
    pub actual: u32,
    pub reason: WaveRejection,
}

/// Outcome of running the detector over a batch of events.
#[derive(Debug, Default)]
pub struct WaveDetectionOutcome {
    /// Waves accepted for dispatch.
    pub accepted: Vec<DetectedWave>,
    /// Waves rejected, with typed reasons. Each input `wave_id` appears
    /// at most once here even if many events in the batch shared it.
    pub rejected: Vec<RejectedWave>,
}

/// Attempt to build a validated `DetectedWave` from a group of events sharing
/// the same `wave_id`.
///
/// When `policy` is [`PartialWavePolicy::AllowPartial`], waves with fewer
/// events than `wave_total` are accepted (marked `partial: true`).  This is
/// used by the dispatcher after the staleness threshold (80% of
/// aggregate_timeout) to force-dispatch whatever results have arrived so far.
///
/// `max_wave_total` caps the fan-out. When `wave_total > max_wave_total`,
/// the wave is rejected with [`WaveRejection::TotalExceedsCap`]. Cap check
/// runs BEFORE partial/incomplete checks so a 200/335 partial batch is
/// also capped.
fn try_build_wave(
    wave_id: &str,
    wave_events: Vec<&Event>,
    registry: &HatRegistry,
    policy: PartialWavePolicy,
    max_wave_total: u32,
) -> Result<DetectedWave, WaveRejection> {
    let first = wave_events.first().ok_or(WaveRejection::ZeroTotal)?;
    let topic = first.topic.clone();
    let wave_total = first.wave_total.ok_or(WaveRejection::ZeroTotal)?;

    if wave_total == 0 {
        tracing::warn!(wave_id, "wave_total must be > 0; skipping wave");
        return Err(WaveRejection::ZeroTotal);
    }

    // U2 cap check runs BEFORE partial / index range checks so a 200/335
    // partial batch is still rejected as cap-overshoot (not partial).
    if wave_total > max_wave_total {
        tracing::warn!(
            wave_id,
            actual = wave_total,
            cap = max_wave_total,
            "wave_total exceeds configured max_wave_total cap; rejecting wave"
        );
        return Err(WaveRejection::TotalExceedsCap {
            actual: wave_total,
            cap: max_wave_total,
        });
    }

    for event in &wave_events {
        if event.topic != topic {
            tracing::warn!(
                wave_id,
                expected_topic = %topic,
                actual_topic = %event.topic,
                "Inconsistent topic in wave events, skipping wave"
            );
            return Err(WaveRejection::InconsistentTopic);
        }
        if event.wave_total != Some(wave_total) {
            tracing::warn!(
                wave_id,
                "Inconsistent wave_total in wave events, skipping wave"
            );
            return Err(WaveRejection::InconsistentTotal);
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
                return Err(WaveRejection::IndexOutOfRange);
            }
            None => {
                tracing::warn!(wave_id, "wave event missing wave_index; skipping wave");
                return Err(WaveRejection::MissingIndex);
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
        // Treat batch-size mismatch as InconsistentTotal (the protocol is
        // violated because events that should be present are not).
        return Err(WaveRejection::InconsistentTotal);
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
    let target_hat_id = match registry.find_by_trigger(&topic) {
        Some(id) => id,
        None => {
            tracing::warn!(wave_id, topic = %topic, "no target hat for wave topic");
            return Err(WaveRejection::NoTargetHat);
        }
    };
    let hat_config = match registry.get_config(target_hat_id) {
        Some(c) => c.clone(),
        None => {
            tracing::warn!(
                wave_id,
                hat = %target_hat_id,
                "no hat config for target hat"
            );
            return Err(WaveRejection::NoTargetHat);
        }
    };

    // Only trigger wave execution for hats with concurrency > 1
    if hat_config.concurrency <= 1 {
        return Err(WaveRejection::SequentialTarget);
    }

    // Sort events by wave_index for deterministic ordering
    let mut sorted_events: Vec<Event> = wave_events.into_iter().cloned().collect();
    sorted_events.sort_by_key(|e| e.wave_index.unwrap_or(0));

    // Inherit the aggregate timeout from the consumer hat(s) that subscribe
    // to the worker hat's published topics.  This lets the dispatcher cap
    // the wave wait to the aggregator's patience (e.g. review-synthesizer's
    // 300s aggregate window) without forcing the worker hat to carry an
    // `aggregate` block, which is reserved for aggregator hats.
    let consumer_aggregate_timeout = consumer_aggregate_timeout_for(&hat_config, registry);

    Ok(DetectedWave {
        wave_id: wave_id.to_string(),
        target_hat: target_hat_id.clone(),
        hat_config,
        events: sorted_events,
        total: wave_total,
        partial: is_partial,
        consumer_aggregate_timeout,
    })
}

/// Look at the hats that subscribe to each topic the worker hat publishes,
/// and return the smallest explicit aggregate timeout among those consumers.
///
/// Returns `None` when no consumer has an explicit aggregate timeout.
fn consumer_aggregate_timeout_for(hat_config: &HatConfig, registry: &HatRegistry) -> Option<u64> {
    hat_config
        .publishes
        .iter()
        .filter_map(|topic| {
            registry
                .find_by_trigger(topic)
                .and_then(|consumer_id| registry.get_config(consumer_id))
                .and_then(|cfg| cfg.aggregate.as_ref())
                .map(|agg| u64::from(agg.timeout))
        })
        .min()
}

/// Convert a `Result<DetectedWave, WaveRejection>` into the
/// `(Option<DetectedWave>, Option<RejectedWave>)` shape used by the
/// legacy `Vec<DetectedWave>` callers. Used by `detect_wave_events`
/// and `detect_all_wave_events` to preserve the historical single-wave
/// API surface while making the cap rejection observable.
fn partition_group(
    result: Result<DetectedWave, WaveRejection>,
    wave_id: &str,
    topic: &str,
    actual: u32,
) -> (Option<DetectedWave>, Option<RejectedWave>) {
    match result {
        Ok(wave) => (Some(wave), None),
        Err(reason) => (
            None,
            Some(RejectedWave {
                wave_id: wave_id.to_string(),
                topic: topic.to_string(),
                actual,
                reason,
            }),
        ),
    }
}

/// Default cap for wave fan-out (U2). `EventLoopConfig.max_wave_total`
/// takes precedence; this is the safety net when no cap is configured.
pub const DEFAULT_MAX_WAVE_TOTAL: u32 = 64;

/// Detect wave events from a set of events.
///
/// Groups events by wave_id, validates that all events in a wave are consistent
/// (same topic, matching wave_total), and resolves the target hat from the registry.
///
/// v1: Returns the first detected wave (one wave per iteration).
/// Events without wave metadata are ignored.
/// Only complete waves are returned (use [`detect_all_wave_events_with_policy`]
/// for partial wave support).
///
/// `max_wave_total` caps fan-out. Overshoot yields `None` and the
/// dispatcher must use [`detect_wave_events_capped`] to observe the
/// typed [`WaveRejection::TotalExceedsCap`] reason.
pub fn detect_wave_events(
    events: &[Event],
    registry: &HatRegistry,
    max_wave_total: u32,
) -> Option<DetectedWave> {
    let outcome = detect_wave_events_capped(events, registry, max_wave_total);
    // For the single-wave API we discard rejections — callers that need
    // structured rejection reasons should use `detect_wave_events_capped`.
    outcome.accepted.into_iter().next()
}

/// Single-wave variant with typed rejection. Returns both the accepted
/// wave (if any) and the rejection reason (if the wave was capped or
/// otherwise invalid).
pub fn detect_wave_events_capped(
    events: &[Event],
    registry: &HatRegistry,
    max_wave_total: u32,
) -> WaveDetectionOutcome {
    // Group events by wave_id
    let mut wave_groups: HashMap<&str, Vec<&Event>> = HashMap::new();
    for event in events {
        if let Some(ref wave_id) = event.wave_id {
            wave_groups.entry(wave_id.as_str()).or_default().push(event);
        }
    }

    if wave_groups.is_empty() {
        return WaveDetectionOutcome::default();
    }

    // v1: Take the lexicographically first wave_id (deterministic, one wave per iteration)
    let wave_id = match wave_groups.keys().min() {
        Some(id) => *id,
        None => return WaveDetectionOutcome::default(),
    };
    if wave_groups.len() > 1 {
        tracing::warn!(
            selected = wave_id,
            total_waves = wave_groups.len(),
            "Multiple waves detected in single iteration, processing only the first"
        );
    }
    let wave_events = wave_groups.remove(wave_id).unwrap_or_default();
    let topic = wave_events
        .first()
        .map(|e| e.topic.clone())
        .unwrap_or_default();
    let actual = wave_events.first().and_then(|e| e.wave_total).unwrap_or(0);

    let result = try_build_wave(
        wave_id,
        wave_events,
        registry,
        PartialWavePolicy::RequireComplete,
        max_wave_total,
    );
    let (accepted, rejected) = partition_group(result, wave_id, &topic, actual);
    WaveDetectionOutcome {
        accepted: accepted.into_iter().collect(),
        rejected: rejected.into_iter().collect(),
    }
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
/// are ignored. Overshoot waves are returned as `rejected`.
///
/// Only complete waves are returned.  For partial wave support (after
/// staleness threshold), use [`detect_all_wave_events_with_policy`].
pub fn detect_all_wave_events(
    events: &[Event],
    registry: &HatRegistry,
    max_wave_total: u32,
) -> Vec<DetectedWave> {
    detect_all_wave_events_capped(
        events,
        registry,
        PartialWavePolicy::RequireComplete,
        max_wave_total,
    )
    .accepted
}

/// Detect all waves with a configured partial-wave policy (legacy helper).
/// For the full outcome (accepted + rejected) use
/// [`detect_all_wave_events_capped`].
pub fn detect_all_wave_events_with_policy(
    events: &[Event],
    registry: &HatRegistry,
    policy: PartialWavePolicy,
    max_wave_total: u32,
) -> Vec<DetectedWave> {
    detect_all_wave_events_capped(events, registry, policy, max_wave_total).accepted
}

/// Detect **all** waves with typed outcomes (accepted + rejected).
///
/// Each `wave_id` appears at most once in either `accepted` or `rejected`,
/// even if many events in the batch shared it. This is the primary
/// entry point used by the U2/U4 dispatcher.
pub fn detect_all_wave_events_capped(
    events: &[Event],
    registry: &HatRegistry,
    policy: PartialWavePolicy,
    max_wave_total: u32,
) -> WaveDetectionOutcome {
    // Group events by wave_id
    let mut wave_groups: HashMap<&str, Vec<&Event>> = HashMap::new();
    for event in events {
        if let Some(ref wave_id) = event.wave_id {
            wave_groups.entry(wave_id.as_str()).or_default().push(event);
        }
    }

    let mut outcome = WaveDetectionOutcome::default();
    for (wave_id, wave_events) in wave_groups {
        let topic = wave_events
            .first()
            .map(|e| e.topic.clone())
            .unwrap_or_default();
        let actual = wave_events.first().and_then(|e| e.wave_total).unwrap_or(0);
        let result = try_build_wave(wave_id, wave_events, registry, policy, max_wave_total);
        let (acc, rej) = partition_group(result, wave_id, &topic, actual);
        if let Some(w) = acc {
            outcome.accepted.push(w);
        }
        if let Some(r) = rej {
            outcome.rejected.push(r);
        }
    }

    // Deterministic ordering by wave_id
    outcome.accepted.sort_by(|a, b| a.wave_id.cmp(&b.wave_id));
    outcome.rejected.sort_by(|a, b| a.wave_id.cmp(&b.wave_id));
    outcome
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
            system_injected: None,
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

        let wave = detect_wave_events(&events, &registry, 64).unwrap();
        assert_eq!(wave.wave_id, "w-abc");
        assert_eq!(wave.total, 3);
        assert_eq!(wave.events.len(), 3);
        assert!(!wave.partial, "complete wave must not be marked partial");
        assert_eq!(wave.target_hat.as_str(), "reviewer");
        assert_eq!(wave.hat_config.concurrency, 4);
    }

    // ─── 2026-07-25-006 plan U3: idle heartbeat accessors ───
    //
    // These helpers are the only public way the dispatcher / worker
    // should learn the idle knobs. The behavior table below must
    // remain stable so legacy presets (no idle fields) keep working
    // and explicit pins are honored.

    fn detected_wave_with_hat_timeout(
        timeout: Option<u32>,
        idle: Option<u32>,
        cap: Option<u32>,
    ) -> DetectedWave {
        let yaml = format!(
            r#"
hats:
  worker:
    name: "Worker"
    triggers: ["work.ready"]
    publishes: ["work.done"]
    instructions: "do work"
    concurrency: 2
    timeout: {timeout_val}
    idle_heartbeat_secs: {idle_val}
    idle_weak_signal_cap: {cap_val}
"#,
            timeout_val = timeout
                .map(|n| n.to_string())
                .unwrap_or_else(|| "~".to_string()),
            idle_val = idle
                .map(|n| n.to_string())
                .unwrap_or_else(|| "~".to_string()),
            cap_val = cap
                .map(|n| n.to_string())
                .unwrap_or_else(|| "~".to_string()),
        );
        let config: RalphConfig = serde_yaml::from_str(&yaml).expect("parse test yaml");
        let registry = HatRegistry::from_config(&config);
        let events = vec![Event {
            topic: "work.ready".to_string(),
            payload: Some("p".to_string()),
            ts: "2025-01-01T00:00:00Z".to_string(),
            hat: None,
            triggered: None,
            source: None,
            wave_id: Some("w-1".to_string()),
            wave_index: Some(0),
            wave_total: Some(1),
            system_injected: None,
        }];
        detect_wave_events(&events, &registry, 64).expect("wave detected")
    }

    #[test]
    fn idle_heartbeat_secs_accessor_table() {
        // None → disabled (idle_heartbeat_secs() == None, idle_enabled false)
        let w = detected_wave_with_hat_timeout(Some(600), None, None);
        assert_eq!(w.idle_heartbeat_secs(), None);
        assert!(!w.idle_enabled());
        // timeout must NOT silently flip to 0 due to idle defaults
        assert_eq!(w.per_worker_timeout_secs(), 600);

        // Some(0) → disabled (explicit opt-out)
        let w = detected_wave_with_hat_timeout(Some(600), Some(0), None);
        assert_eq!(w.idle_heartbeat_secs(), None);
        assert!(!w.idle_enabled());
        assert_eq!(w.per_worker_timeout_secs(), 600);

        // Some(n>0) → enabled
        let w = detected_wave_with_hat_timeout(Some(1800), Some(120), Some(8));
        assert_eq!(w.idle_heartbeat_secs(), Some(120));
        assert!(w.idle_enabled());
        assert_eq!(w.per_worker_timeout_secs(), 1800);
    }

    #[test]
    fn idle_weak_signal_cap_accessor_table() {
        // None → operator default (8)
        let w = detected_wave_with_hat_timeout(Some(600), Some(120), None);
        assert_eq!(w.idle_weak_signal_cap(), 8);
        // Some(0) → operator default (explicit "no cap" still falls back)
        let w = detected_wave_with_hat_timeout(Some(600), Some(120), Some(0));
        assert_eq!(w.idle_weak_signal_cap(), 8);
        // Some(n) → use as-is
        let w = detected_wave_with_hat_timeout(Some(600), Some(120), Some(3));
        assert_eq!(w.idle_weak_signal_cap(), 3);
        // Idle disabled path still resolves a stable cap value
        // (the worker loop won't consume it, but the accessor must
        // never panic or change under the hood).
        let w = detected_wave_with_hat_timeout(Some(600), None, Some(2));
        assert_eq!(w.idle_weak_signal_cap(), 2);
    }

    #[test]
    fn idle_accessor_does_not_affect_aggregate_timeout_priority() {
        // Even when idle is enabled, aggregate_timeout_secs() must
        // continue to walk the existing priority chain
        // (aggregate block → consumer → per-worker). This guards
        // against a future refactor that mistakenly re-uses
        // idle_heartbeat_secs to scale aggregate timeouts.
        let w = detected_wave_with_hat_timeout(Some(600), Some(120), Some(8));
        // No aggregate block, no consumer → fall back to per_worker.
        assert_eq!(w.aggregate_timeout_secs(), 600);
        assert!(!w.has_explicit_aggregate_timeout());
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
            system_injected: None,
        }];

        assert!(detect_wave_events(&events, &registry, 64).is_none());
    }

    #[test]
    fn test_detect_ignores_sequential_hat() {
        let registry = make_registry_with_sequential_hat();
        let events = vec![
            make_wave_event("build.start", "payload", "w-abc", 0, 2),
            make_wave_event("build.start", "payload", "w-abc", 1, 2),
        ];

        // Hat has concurrency=1 (default), so wave detection returns None
        assert!(detect_wave_events(&events, &registry, 64).is_none());
    }

    #[test]
    fn test_detect_rejects_inconsistent_topics() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![
            make_wave_event("review.file", "src/main.rs", "w-abc", 0, 2),
            make_wave_event("review.other", "src/lib.rs", "w-abc", 1, 2),
        ];

        assert!(detect_wave_events(&events, &registry, 64).is_none());
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

        let wave = detect_wave_events(&events, &registry, 64).unwrap();
        assert_eq!(wave.events[0].payload.as_deref(), Some("first"));
        assert_eq!(wave.events[1].payload.as_deref(), Some("second"));
        assert_eq!(wave.events[2].payload.as_deref(), Some("third"));
    }

    #[test]
    fn test_empty_events_returns_none() {
        let registry = make_registry_with_concurrent_hat();
        assert!(detect_wave_events(&[], &registry, 64).is_none());
    }

    #[test]
    fn test_unknown_topic_returns_none() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![make_wave_event("unknown.topic", "payload", "w-abc", 0, 1)];

        assert!(detect_wave_events(&events, &registry, 64).is_none());
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
            system_injected: None,
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
        assert!(detect_wave_events(&events, &registry, 64).is_none());
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
        assert!(detect_wave_events(&events, &registry, 64).is_none());
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
        assert!(detect_wave_events(&events, &registry, 64).is_none());
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
        assert!(detect_wave_events(&events, &registry, 64).is_none());
    }

    #[test]
    fn test_detect_rejects_batch_size_mismatch() {
        let registry = make_registry_with_concurrent_hat();
        // 2 events but total=3
        let events = vec![
            make_wave_event("review.file", "p1", "w-mm", 0, 3),
            make_wave_event("review.file", "p2", "w-mm", 1, 3),
        ];
        assert!(detect_wave_events(&events, &registry, 64).is_none());
    }

    #[test]
    fn test_detect_rejects_inconsistent_wave_total() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![
            wave_event_with_index_total("review.file", "w-it", Some(0), Some(2)),
            wave_event_with_index_total("review.file", "w-it", Some(1), Some(3)),
        ];
        assert!(detect_wave_events(&events, &registry, 64).is_none());
    }

    #[test]
    fn test_detect_accepts_complete_well_formed_wave() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![
            make_wave_event("review.file", "a", "w-good", 0, 2),
            make_wave_event("review.file", "b", "w-good", 1, 2),
        ];
        let wave = detect_wave_events(&events, &registry, 64).expect("valid wave must be detected");
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

        let waves = detect_all_wave_events(&events, &registry, 64);
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

        let waves = detect_all_wave_events(&events, &registry, 64);
        assert_eq!(waves.len(), 1, "only the valid wave should be returned");
        assert_eq!(waves[0].wave_id, "w-good");
    }

    #[test]
    fn test_detect_all_returns_empty_when_no_waves() {
        let registry = make_registry_with_concurrent_hat();
        let waves = detect_all_wave_events(&[], &registry, 64);
        assert!(waves.is_empty());
    }

    #[test]
    fn test_detect_all_ignores_sequential_hat() {
        let registry = make_registry_with_sequential_hat();
        let events = vec![
            make_wave_event("build.start", "p1", "w-seq", 0, 2),
            make_wave_event("build.start", "p2", "w-seq", 1, 2),
        ];
        let waves = detect_all_wave_events(&events, &registry, 64);
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
        let waves = detect_all_wave_events(&events, &registry, 64);
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
        let waves = detect_all_wave_events(&events, &registry, 64);
        assert!(waves.is_empty(), "RequireComplete must skip partial wave");

        // AllowPartial → wave with partial=true
        let waves = detect_all_wave_events_with_policy(
            &events,
            &registry,
            PartialWavePolicy::AllowPartial,
            64,
        );
        assert_eq!(waves.len(), 1);
        assert!(waves[0].partial, "partial wave must be marked");
        assert_eq!(waves[0].total, 3, "total must reflect expected count");
        assert_eq!(
            waves[0].events.len(),
            2,
            "events only contains arrived results"
        );
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
            64,
        );
        assert_eq!(waves.len(), 1);
        assert!(
            !waves[0].partial,
            "complete wave must not be marked partial even with AllowPartial"
        );
    }

    #[test]
    fn test_allow_partial_zero_events_still_skipped() {
        let registry = make_registry_with_concurrent_hat();
        // No events at all — nothing to detect
        let waves =
            detect_all_wave_events_with_policy(&[], &registry, PartialWavePolicy::AllowPartial, 64);
        assert!(waves.is_empty());
    }

    // ---- U2: max_wave_total cap + typed rejection tests ----

    #[test]
    fn test_cap_64_accepts_64_events() {
        let registry = make_registry_with_concurrent_hat();
        let events: Vec<Event> = (0..64)
            .map(|i| make_wave_event("review.file", "p", "w-64", i, 64))
            .collect();
        let outcome = detect_all_wave_events_capped(
            &events,
            &registry,
            PartialWavePolicy::RequireComplete,
            64,
        );
        assert!(outcome.rejected.is_empty(), "64/64 must not be rejected");
        assert_eq!(outcome.accepted.len(), 1);
        assert_eq!(outcome.accepted[0].total, 64);
    }

    #[test]
    fn test_cap_64_rejects_65_events() {
        let registry = make_registry_with_concurrent_hat();
        let events: Vec<Event> = (0..65)
            .map(|i| make_wave_event("review.file", "p", "w-65", i, 65))
            .collect();
        let outcome = detect_all_wave_events_capped(
            &events,
            &registry,
            PartialWavePolicy::RequireComplete,
            64,
        );
        assert!(outcome.accepted.is_empty(), "65/64 must not be accepted");
        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(outcome.rejected[0].wave_id, "w-65");
        assert_eq!(outcome.rejected[0].actual, 65);
        assert_eq!(
            outcome.rejected[0].reason,
            WaveRejection::TotalExceedsCap {
                actual: 65,
                cap: 64
            }
        );
    }

    #[test]
    fn test_cap_64_rejects_335_events_with_one_rejection() {
        let registry = make_registry_with_concurrent_hat();
        // The exact 335-worker bug from the report.
        let events: Vec<Event> = (0..335)
            .map(|i| make_wave_event("review.file", "p", "w-335", i, 335))
            .collect();
        let outcome = detect_all_wave_events_capped(
            &events,
            &registry,
            PartialWavePolicy::RequireComplete,
            64,
        );
        assert_eq!(
            outcome.rejected.len(),
            1,
            "335 events → 1 rejection (not 335)"
        );
        assert_eq!(
            outcome.rejected[0].reason,
            WaveRejection::TotalExceedsCap {
                actual: 335,
                cap: 64
            }
        );
    }

    #[test]
    fn test_cap_64_rejects_u32_max_total() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![wave_event_with_index_total(
            "review.file",
            "w-huge",
            Some(0),
            Some(u32::MAX),
        )];
        let outcome = detect_all_wave_events_capped(
            &events,
            &registry,
            PartialWavePolicy::RequireComplete,
            64,
        );
        assert!(outcome.accepted.is_empty());
        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(
            outcome.rejected[0].reason,
            WaveRejection::TotalExceedsCap {
                actual: u32::MAX,
                cap: 64
            }
        );
    }

    #[test]
    fn test_cap_runs_before_partial_check() {
        let registry = make_registry_with_concurrent_hat();
        // 200/335 — partial AND over cap. Cap rejection wins.
        let events: Vec<Event> = (0..200)
            .map(|i| make_wave_event("review.file", "p", "w-200of335", i, 335))
            .collect();
        let outcome =
            detect_all_wave_events_capped(&events, &registry, PartialWavePolicy::AllowPartial, 64);
        assert!(outcome.accepted.is_empty());
        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(
            outcome.rejected[0].reason,
            WaveRejection::TotalExceedsCap {
                actual: 335,
                cap: 64
            }
        );
    }

    #[test]
    fn test_zero_total_still_independent_rejection() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![wave_event_with_index_total(
            "review.file",
            "w-zero",
            Some(0),
            Some(0),
        )];
        let outcome = detect_all_wave_events_capped(
            &events,
            &registry,
            PartialWavePolicy::RequireComplete,
            64,
        );
        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(outcome.rejected[0].reason, WaveRejection::ZeroTotal);
    }

    #[test]
    fn test_no_target_hat_independent_rejection() {
        let registry = make_registry_with_concurrent_hat();
        let events = vec![
            make_wave_event("unknown.topic", "p", "w-unk", 0, 2),
            make_wave_event("unknown.topic", "p", "w-unk", 1, 2),
        ];
        let outcome = detect_all_wave_events_capped(
            &events,
            &registry,
            PartialWavePolicy::RequireComplete,
            64,
        );
        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(outcome.rejected[0].reason, WaveRejection::NoTargetHat);
    }

    #[test]
    fn test_overshoot_and_valid_in_same_batch() {
        let registry = make_registry_with_concurrent_hat();
        // 335-event wave + 4-event valid wave
        let mut events: Vec<Event> = (0..335)
            .map(|i| make_wave_event("review.file", "p", "w-huge", i, 335))
            .collect();
        events.push(make_wave_event("review.file", "a", "w-valid", 0, 2));
        events.push(make_wave_event("review.file", "b", "w-valid", 1, 2));

        let outcome = detect_all_wave_events_capped(
            &events,
            &registry,
            PartialWavePolicy::RequireComplete,
            64,
        );

        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(outcome.rejected[0].wave_id, "w-huge");
        assert_eq!(outcome.accepted.len(), 1);
        assert_eq!(outcome.accepted[0].wave_id, "w-valid");
    }

    #[test]
    fn test_detect_wave_events_capped_returns_outcome() {
        let registry = make_registry_with_concurrent_hat();
        let events: Vec<Event> = (0..100)
            .map(|i| make_wave_event("review.file", "p", "w-100", i, 100))
            .collect();
        let outcome = detect_wave_events_capped(&events, &registry, 64);
        assert!(outcome.accepted.is_empty());
        assert_eq!(outcome.rejected.len(), 1);
        assert_eq!(
            outcome.rejected[0].reason,
            WaveRejection::TotalExceedsCap {
                actual: 100,
                cap: 64
            }
        );
    }

    #[test]
    fn test_default_max_wave_total_is_64() {
        assert_eq!(DEFAULT_MAX_WAVE_TOTAL, 64);
    }
}
