//! Drift detector — observes events, computes metrics, emits findings.
//!
//! [`DriftDetector`] is the **signal source** layer of the runtime
//! diagnosis stack. It does not manipulate the loop, escalate, or
//! publish `task.resume` — it just turns observed event streams into
//! [`DriftFinding`]s. U6 (`RecoveryResponder`) consumes the findings
//! and decides what to do with them.
//!
//! # Metrics
//!
//! Three metrics are computed per iteration of [`Self::observe`]:
//!
//! - [`field_completeness`](DriftDetector::field_completeness):
//!   per-`(topic, field)` fraction of events in the window that
//!   include the field. Required fields come from
//!   [`EventPolicyConfig::schemas`] and
//!   [`ExecutionContractsConfig::rules`] when those configs are
//!   supplied to [`DriftDetector::new_with_sources`]. The metric is
//!   a no-op when neither source declares required fields for a
//!   topic.
//! - [`coord_join_rate`](DriftDetector::coord_join_rate):
//!   per-`(from_topic, to_topic)` edge join rate. Only declared
//!   edges (i.e. the from-hat publishes a topic the to-hat
//!   subscribes to) are computed — the detector does not infer
//!   arbitrary topic edges to avoid false positives.
//! - [`emit_cadence`](DriftDetector::emit_cadence): per-topic
//!   standard-deviation of inter-emit interval. A finding is
//!   emitted when the latest interval exceeds
//!   `avg + sigma * stddev`.
//!
//! All three metrics skip their work for windows with too few
//! samples (the `min_samples` guard). The detector also de-duplicates
//! findings within an iteration: a `(metric, topic, field,
//! from_topic, to_topic)` tuple is reported at most once per
//! [`Self::observe`] call. The dedup set is reset explicitly via
//! [`Self::reset_seen`] when the caller wants a fresh window
//! (typically: at the start of each loop iteration).
//!
//! # Non-regression
//!
//! - The detector is `!Sync`; the consumer side owns it.
//! - The detector never blocks on I/O; it only reads in-memory
//!   state.
//! - The detector never panics. The constructor accepts
//!   `DriftConfig` but tolerates empty `required_fields` per topic.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::config::DriftConfig;
use crate::diagnosis::{DiagnosisSeverity, DriftMetric};

use super::window::{DriftWindow, EventSnapshot};

/// Minimum number of samples required to compute `emit_cadence`.
///
/// Hardcoded for now (U1's `DriftConfig` does not yet expose a knob
/// for it; the spec calls for this guard to prevent false positives
/// on warm-up windows).
pub const EMIT_CADENCE_MIN_SAMPLES: usize = 5;

/// A single drift finding produced by the detector.
///
/// `DriftFinding` is the detector's *internal* record type. It is
/// **not** the public `recovery.jsonl` entry — use
/// [`super::alert::finding_to_journal_entry`] to convert it for
/// persistence. The detector stores it locally so U6's responder
/// can attach additional context (e.g. `target_hat`,
/// `expected_action`) without re-parsing the JSONL stream.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftFinding {
    /// Unique id for the finding. UUIDv4 string.
    pub finding_id: String,
    /// Which metric produced the finding.
    pub metric: DriftMetric,
    /// Topic the finding is about, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,
    /// Field the finding is about, when applicable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    /// Source topic in a coord-join finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_topic: Option<String>,
    /// Target topic in a coord-join finding.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_topic: Option<String>,
    /// Observed value (the raw metric value, not the
    /// threshold-distance).
    pub observed_value: f64,
    /// Threshold the metric was compared against.
    pub threshold: f64,
    /// Severity bucket. The detector picks it from the metric and
    /// the magnitude of the breach.
    pub severity: DiagnosisSeverity,
    /// Loop iteration the finding was produced at.
    pub iteration: u32,
    /// Number of snapshots in the topic's window at finding time.
    pub window_size: usize,
    /// Human-readable explanation.
    pub message: String,
}

impl DriftFinding {
    /// Returns the dedup tuple `(metric, topic, field, from_topic,
    /// to_topic)` used by [`DriftDetector`] to suppress duplicates
    /// within a single iteration.
    #[must_use]
    pub fn dedup_key(
        &self,
    ) -> (
        DriftMetric,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    ) {
        (
            self.metric,
            self.topic.clone(),
            self.field.clone(),
            self.from_topic.clone(),
            self.to_topic.clone(),
        )
    }
}

/// Source of required-payload-field declarations for a topic.
///
/// The detector looks here for the field list to apply to
/// `field_completeness`. Both kinds of declaration are merged into
/// one set per topic.
#[derive(Debug, Clone, Default)]
pub struct RequiredFields {
    /// `topic -> required fields` from `EventPolicyConfig::schemas`.
    pub from_policy: HashMap<String, Vec<String>>,
    /// `topic -> required fields` from
    /// `ExecutionContractsConfig::rules[*].require_payload_fields`.
    pub from_execution_contract: HashMap<String, Vec<String>>,
}

impl RequiredFields {
    /// Create an empty declaration set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Return the merged required fields for `topic`. Returns an
    /// empty `Vec` when the topic has no policy nor contract
    /// declaration (the metric is a no-op in that case).
    #[must_use]
    pub fn for_topic(&self, topic: &str) -> Vec<String> {
        let mut merged: Vec<String> = self.from_policy.get(topic).cloned().unwrap_or_default();
        if let Some(extra) = self.from_execution_contract.get(topic) {
            for f in extra {
                if !merged.iter().any(|m| m == f) {
                    merged.push(f.clone());
                }
            }
        }
        merged
    }
}

/// Declared `(from_topic, to_topic)` edges for `coord_join_rate`.
///
/// The detector only computes the join rate for edges in this set;
/// any other pair is silently skipped. This avoids inferring edges
/// the preset does not actually wire.
#[derive(Debug, Clone, Default)]
pub struct DeclaredEdges {
    /// Each tuple is `(from_topic, to_topic)`.
    pub edges: HashSet<(String, String)>,
}

impl DeclaredEdges {
    /// Create an empty edge set.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Build an edge set from a list of `(from_topic, to_topic)`
    /// pairs. Duplicate pairs are collapsed.
    #[must_use]
    pub fn from_pairs<I, S1, S2>(pairs: I) -> Self
    where
        I: IntoIterator<Item = (S1, S2)>,
        S1: Into<String>,
        S2: Into<String>,
    {
        let mut edges: HashSet<(String, String)> = HashSet::new();
        for (from, to) in pairs {
            edges.insert((from.into(), to.into()));
        }
        Self { edges }
    }

    /// True when the edge has been declared.
    #[must_use]
    pub fn contains(&self, from: &str, to: &str) -> bool {
        self.edges.contains(&(from.to_string(), to.to_string()))
    }
}

/// The drift detector. Holds a per-topic window plus the metric
/// configuration.
///
/// Not thread-safe by design. The detector is owned by the loop
/// runner; the EventBus observer pushes snapshots to it via a
/// bounded channel and the loop drains that channel between
/// iterations.
pub struct DriftDetector {
    /// Per-topic rolling window. Lazily created on first
    /// observation.
    windows: HashMap<String, DriftWindow>,
    /// Drift thresholds and window size.
    config: DriftConfig,
    /// Last dedup set (cleared at the start of every `observe`
    /// call, or via `reset_seen`).
    seen: HashSet<(
        DriftMetric,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )>,
    /// Required-field declarations for `field_completeness`.
    required_fields: RequiredFields,
    /// Declared edges for `coord_join_rate`.
    edges: DeclaredEdges,
    /// Last iteration the detector was called at. Bumped by
    /// [`Self::observe`]; the responder can read it for stale-window
    /// detection.
    last_iteration: u32,
    /// Total number of events fed into the detector since it was
    /// created. Useful for unit tests and for telemetry.
    observed_total: u64,
    /// Number of events the consumer dropped because the upstream
    /// bounded channel was full. The detector itself does not
    /// channel — the observer does — but we mirror the count so
    /// U6 can read it from the same place.
    dropped_events: u64,
}

impl DriftDetector {
    /// Create a detector with the given `config`, no required
    /// fields, and no declared edges. Useful when the caller has
    /// not parsed a `RalphConfig` (e.g. unit tests).
    #[must_use]
    pub fn new(config: DriftConfig) -> Self {
        Self {
            windows: HashMap::new(),
            config,
            seen: HashSet::new(),
            required_fields: RequiredFields::new(),
            edges: DeclaredEdges::new(),
            last_iteration: 0,
            observed_total: 0,
            dropped_events: 0,
        }
    }

    /// Create a detector pre-loaded with required-field and
    /// edge-declaration sources.
    #[must_use]
    pub fn new_with_sources(
        config: DriftConfig,
        required_fields: RequiredFields,
        edges: DeclaredEdges,
    ) -> Self {
        Self {
            windows: HashMap::new(),
            config,
            seen: HashSet::new(),
            required_fields,
            edges,
            last_iteration: 0,
            observed_total: 0,
            dropped_events: 0,
        }
    }

    /// Replace the required-field sources. Use this when the
    /// caller wants to re-bind the detector to a different preset
    /// (e.g. between hat re-configurations).
    pub fn set_required_fields(&mut self, required_fields: RequiredFields) {
        self.required_fields = required_fields;
    }

    /// Replace the declared-edges set.
    pub fn set_edges(&mut self, edges: DeclaredEdges) {
        self.edges = edges;
    }

    /// Reset the per-iteration dedup set. Call this between loop
    /// iterations if you want a fresh window of findings.
    pub fn reset_seen(&mut self) {
        self.seen.clear();
    }

    /// Read-only access to the drift configuration.
    #[must_use]
    pub fn config(&self) -> &DriftConfig {
        &self.config
    }

    /// Last iteration the detector was called at.
    #[must_use]
    pub fn last_iteration(&self) -> u32 {
        self.last_iteration
    }

    /// Total number of events the detector has consumed since it
    /// was created. Includes dropped events at the consumer side.
    #[must_use]
    pub fn observed_total(&self) -> u64 {
        self.observed_total
    }

    /// Number of events the consumer dropped because the upstream
    /// bounded channel was full. U6 should be able to read this
    /// from the same struct; the observer side increments it.
    #[must_use]
    pub fn dropped_events(&self) -> u64 {
        self.dropped_events
    }

    /// Increment the dropped-event counter. The observer (not the
    /// detector) calls this when its bounded channel is full. Kept
    /// on the detector so U6 has one read point.
    pub fn record_dropped_event(&mut self) {
        self.dropped_events = self.dropped_events.saturating_add(1);
    }

    /// Window size for `topic`, or 0 if the topic has not been
    /// observed yet.
    #[must_use]
    pub fn window_size_for(&self, topic: &str) -> usize {
        self.windows.get(topic).map_or(0, DriftWindow::len)
    }

    /// Feed a single snapshot into the detector and return any new
    /// findings it produced for the current iteration.
    ///
    /// `observe` does **not** clear the per-iteration dedup set —
    /// the caller is expected to call [`Self::reset_seen`] between
    /// loop iterations, which matches the per-iteration scoping of
    /// other findings (the dedup set is intentionally NOT cleared
    /// per snapshot so multiple `observe` calls within one
    /// iteration collapse into a single finding per metric tuple).
    pub fn observe(&mut self, snapshot: EventSnapshot) -> Vec<DriftFinding> {
        self.observed_total = self.observed_total.saturating_add(1);
        self.last_iteration = snapshot.iteration;

        let topic = snapshot.topic.clone();

        // Lazily allocate the per-topic window. We use the raw
        // `HashMap::entry` API to avoid borrowing `self.config` while
        // also mutating `self.windows`.
        let capacity = self.config.window_size.max(1);
        let window = self
            .windows
            .entry(topic.clone())
            .or_insert_with(|| DriftWindow::new(capacity));
        window.push(snapshot);

        let mut findings = Vec::new();
        self.check_field_completeness(&topic, &mut findings);
        self.check_coord_join_rate(&topic, &mut findings);
        self.check_emit_cadence(&topic, &mut findings);

        findings
    }

    // ── field_completeness ────────────────────────────────────────

    fn check_field_completeness(&mut self, topic: &str, out: &mut Vec<DriftFinding>) {
        let required = self.required_fields.for_topic(topic);
        if required.is_empty() {
            return;
        }
        let Some(window) = self.windows.get(topic) else {
            return;
        };
        let total = window.len();
        if total == 0 {
            return;
        }
        for field in &required {
            let key = (
                DriftMetric::FieldCompleteness,
                Some(topic.to_string()),
                Some(field.clone()),
                None,
                None,
            );
            if self.seen.contains(&key) {
                continue;
            }
            let hits = window.iter().filter(|s| s.fields.contains(field)).count();
            let observed = hits as f64 / total as f64;
            if observed < self.config.field_completeness_threshold {
                let severity = pick_severity(
                    self.config.field_completeness_threshold - observed,
                    self.config.field_completeness_threshold,
                );
                let finding = DriftFinding {
                    finding_id: uuid::Uuid::new_v4().to_string(),
                    metric: DriftMetric::FieldCompleteness,
                    topic: Some(topic.to_string()),
                    field: Some(field.clone()),
                    from_topic: None,
                    to_topic: None,
                    observed_value: round4(observed),
                    threshold: self.config.field_completeness_threshold,
                    severity,
                    iteration: self.last_iteration,
                    window_size: total,
                    message: format!(
                        "field `{field}` on topic `{topic}` present in {hits}/{total} events ({:.1}%); required threshold {:.1}%",
                        observed * 100.0,
                        self.config.field_completeness_threshold * 100.0,
                    ),
                };
                self.seen.insert(key);
                out.push(finding);
            }
        }
    }

    // ── coord_join_rate ───────────────────────────────────────────

    fn check_coord_join_rate(&mut self, topic: &str, out: &mut Vec<DriftFinding>) {
        // The window we just observed is `topic`. We look for any
        // declared edge where `to_topic == topic` and walk back
        // through the from-topic window to count how many
        // from-emissions were followed by a to-emission.
        let Some(this_window) = self.windows.get(topic) else {
            return;
        };
        let to_size = this_window.len();
        if to_size == 0 {
            return;
        }
        // Collect candidates: any declared edge whose `to` is this
        // topic. We collect up front to avoid borrowing `self.edges`
        // while we also walk `self.windows`.
        let froms: Vec<String> = self
            .edges
            .edges
            .iter()
            .filter(|(_, to)| to == topic)
            .map(|(from, _)| from.clone())
            .collect();
        for from_topic in froms {
            let key = (
                DriftMetric::CoordJoinRate,
                None,
                None,
                Some(from_topic.clone()),
                Some(topic.to_string()),
            );
            if self.seen.contains(&key) {
                continue;
            }
            let Some(from_window) = self.windows.get(&from_topic) else {
                continue;
            };
            let from_size = from_window.len();
            if from_size == 0 {
                continue;
            }
            // Naive implementation: count how many from-topic events
            // are followed by a to-topic event within the window
            // ordering. We project timestamps into a sorted
            // sequence on each side and count overlapping pairs.
            //
            // The rate formula is:
            //   joined = count of to-events whose timestamp >=
            //            the latest from-event timestamp seen so far
            //   rate   = min(1.0, joined / from_size)
            let from_timestamps: Vec<DateTime<Utc>> =
                from_window.iter().map(|s| s.timestamp).collect();
            let to_timestamps: Vec<DateTime<Utc>> =
                this_window.iter().map(|s| s.timestamp).collect();
            let joined = count_joined(&from_timestamps, &to_timestamps);
            let rate = (joined as f64 / from_size as f64).min(1.0);
            if rate < self.config.coord_join_rate_threshold {
                let severity = pick_severity(
                    self.config.coord_join_rate_threshold - rate,
                    self.config.coord_join_rate_threshold,
                );
                let finding = DriftFinding {
                    finding_id: uuid::Uuid::new_v4().to_string(),
                    metric: DriftMetric::CoordJoinRate,
                    topic: None,
                    field: None,
                    from_topic: Some(from_topic.clone()),
                    to_topic: Some(topic.to_string()),
                    observed_value: round4(rate),
                    threshold: self.config.coord_join_rate_threshold,
                    severity,
                    iteration: self.last_iteration,
                    window_size: from_size + to_size,
                    message: format!(
                        "coord join rate `{from_topic} -> {topic}` is {joined}/{from_size} ({:.1}%); required threshold {:.1}%",
                        rate * 100.0,
                        self.config.coord_join_rate_threshold * 100.0,
                    ),
                };
                self.seen.insert(key);
                out.push(finding);
            }
        }
    }

    // ── emit_cadence ──────────────────────────────────────────────

    fn check_emit_cadence(&mut self, topic: &str, out: &mut Vec<DriftFinding>) {
        let key = (
            DriftMetric::EmitCadence,
            Some(topic.to_string()),
            None,
            None,
            None,
        );
        if self.seen.contains(&key) {
            return;
        }
        let Some(window) = self.windows.get(topic) else {
            return;
        };
        let snapshots: Vec<&EventSnapshot> = window.iter().collect();
        // Low-sample guard: we need at least `EMIT_CADENCE_MIN_SAMPLES`
        // events to compute a meaningful average and standard
        // deviation. Below the floor the detector stays silent — a
        // uniform short stream is *not* a drift signal, and the
        // P2.2 review explicitly rejected the prior "always emit
        // Info" behaviour because the responder was treating every
        // healthy topic as a pending alert. We mark the key as
        // seen so subsequent snapshots in the same iteration stay
        // silent.
        if snapshots.len() < EMIT_CADENCE_MIN_SAMPLES {
            self.seen.insert(key);
            return;
        }
        // Compute inter-emit intervals. Events that share a
        // `wave_id` are excluded: a wave fires N workers in
        // parallel, and their "interval" is not a real cadence
        // signal. We collapse them by treating the entire wave as
        // one logical emit at the wave's first timestamp.
        let intervals = compute_intervals(&snapshots);
        if intervals.is_empty() {
            // All events in the window belong to waves, so we have
            // no real cadence to measure. Same silent path as
            // low-samples.
            self.seen.insert(key);
            return;
        }
        let (avg, stddev) = mean_stddev(&intervals);
        // `worst_z` is the worst positive z-score across the
        // window's inter-emit intervals. Uniform cadence yields
        // `worst_z == 0.0`; the metric must NOT emit a finding in
        // that case. We only push a finding when the z-score is
        // genuinely above the configured `sigma` threshold.
        let worst_z = if stddev <= 0.0 {
            0.0
        } else {
            intervals
                .iter()
                .map(|iv| (iv - avg).max(0.0) / stddev)
                .fold(0.0_f64, f64::max)
        };
        let breached = worst_z > self.config.emit_cadence_sigma;
        self.seen.insert(key);
        if !breached {
            // Healthy uniform cadence — not a diagnosis. Skip.
            return;
        }
        let severity = pick_severity(
            (worst_z - self.config.emit_cadence_sigma) / self.config.emit_cadence_sigma.max(1.0),
            1.0,
        );
        out.push(DriftFinding {
            finding_id: uuid::Uuid::new_v4().to_string(),
            metric: DriftMetric::EmitCadence,
            topic: Some(topic.to_string()),
            field: None,
            from_topic: None,
            to_topic: None,
            observed_value: round4(worst_z),
            threshold: self.config.emit_cadence_sigma,
            severity,
            iteration: self.last_iteration,
            window_size: snapshots.len(),
            message: format!(
                "emit cadence on `{topic}` worst interval is {worst_z:.2}σ above the rolling average (avg={avg:?}, stddev={stddev:?}); required threshold {:.2}σ",
                self.config.emit_cadence_sigma
            ),
        });
    }
}

impl DriftDetector {
    /// Read-only view of the window for `topic`, when present.
    #[must_use]
    pub fn window(&self, topic: &str) -> Option<&DriftWindow> {
        self.windows.get(topic)
    }

    /// All topic names that currently have a window.
    #[must_use]
    pub fn observed_topics(&self) -> Vec<&str> {
        self.windows.keys().map(String::as_str).collect()
    }
}

// ── Helpers ──────────────────────────────────────────────────────────

/// Round to four decimal places to keep the JSONL output stable
/// across runs (and to dodge floating-point display noise in the
/// tests).
fn round4(v: f64) -> f64 {
    (v * 10_000.0).round() / 10_000.0
}

/// Map a relative distance from threshold to a severity bucket.
///
/// `distance` is `(threshold - observed) / threshold` (always in
/// `[0.0, 1.0]` for unit-interval thresholds). The mapping is
/// intentionally coarse:
///
/// - `>= 0.5` → Critical
/// - `>= 0.2` → Error
/// - `>= 0.05` → Warning
/// - else → Info
///
/// Cadence findings have no unit interval, so `pick_severity` is
/// passed a custom `unit` value. The detector scales the cadence
/// z-distance into a `[0, 1]` range before calling.
fn pick_severity(distance: f64, unit: f64) -> DiagnosisSeverity {
    let normalized = if unit > 0.0 {
        (distance / unit).clamp(0.0, 1.0)
    } else {
        0.0
    };
    if normalized >= 0.5 {
        DiagnosisSeverity::Critical
    } else if normalized >= 0.2 {
        DiagnosisSeverity::Error
    } else if normalized >= 0.05 {
        DiagnosisSeverity::Warning
    } else {
        DiagnosisSeverity::Info
    }
}

/// Compute inter-emit intervals for a topic window.
///
/// We collapse consecutive events with the same `wave_id` into a
/// single logical emit (using the earliest timestamp) so a wave of
/// parallel workers does not register as a `1ms` cadence
/// anomaly. Returns the intervals in seconds as `f64` (chrono
/// `Duration::num_milliseconds` / 1000.0).
fn compute_intervals(snapshots: &[&EventSnapshot]) -> Vec<f64> {
    // Build the logical emit sequence: one timestamp per wave
    // group, plus one for each non-wave event.
    let mut emits: Vec<DateTime<Utc>> = Vec::with_capacity(snapshots.len());
    let mut last_wave: Option<&str> = None;
    for snap in snapshots {
        match &snap.wave_id {
            Some(wid) if Some(wid.as_str()) == last_wave => {
                // Same wave as the previous snapshot — keep the
                // earliest timestamp; the current one is already
                // represented.
            }
            Some(wid) => {
                emits.push(snap.timestamp);
                last_wave = Some(wid.as_str());
            }
            None => {
                emits.push(snap.timestamp);
                last_wave = None;
            }
        }
    }
    emits.sort();
    if emits.len() < 2 {
        return Vec::new();
    }
    emits
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).num_milliseconds() as f64 / 1000.0)
        .filter(|v| v.is_finite() && *v >= 0.0)
        .collect()
}

/// Mean and population standard deviation of `xs`. Returns `(0, 0)`
/// for empty input.
fn mean_stddev(xs: &[f64]) -> (f64, f64) {
    if xs.is_empty() {
        return (0.0, 0.0);
    }
    let n = xs.len() as f64;
    let mean = xs.iter().sum::<f64>() / n;
    if xs.len() < 2 {
        return (mean, 0.0);
    }
    let var = xs.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / n;
    (mean, var.sqrt())
}

/// Count how many `to_timestamps` are at-or-after the earliest
/// `from_timestamp`. Used by `coord_join_rate`.
///
/// The current implementation reports the *distinct* to-events
/// that follow at least one from-event in the same window. The
/// rate formula in `check_coord_join_rate` divides this by
/// `from_size`, so the metric answers "of all from-emissions, what
/// fraction was followed by at least one matching to-emission".
fn count_joined(from_timestamps: &[DateTime<Utc>], to_timestamps: &[DateTime<Utc>]) -> usize {
    if from_timestamps.is_empty() || to_timestamps.is_empty() {
        return 0;
    }
    let mut to_sorted: Vec<DateTime<Utc>> = to_timestamps.to_vec();
    to_sorted.sort();
    let earliest_from = *from_timestamps.iter().min().expect("non-empty");
    to_sorted.iter().filter(|t| **t >= earliest_from).count()
}
