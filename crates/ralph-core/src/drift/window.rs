//! Drift observation window.
//!
//! [`DriftWindow`] is a bounded, allocation-aware ring buffer of
//! [`EventSnapshot`]s. The detector keeps one window per topic; older
//! snapshots are evicted when the window is full.
//!
//! # Thread safety
//!
//! The window is intentionally `!Sync` / `!Send`-able via a single
//! `&mut self` borrow on the consumer side. The observer side does
//! not touch the window directly — it sends [`EventSnapshot`]s over a
//! bounded channel that the consumer (loop side) drains and feeds
//! into [`DriftDetector::observe`].
//!
//! [`DriftDetector::observe`]: crate::drift::detector::DriftDetector::observe

use std::collections::BTreeSet;
use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Lightweight projection of an `Event` used by the drift detector.
///
/// `EventSnapshot` deliberately drops the full payload and only keeps
/// what the three drift metrics need:
///
/// - `topic` / `source_hat` — for `field_completeness` and
///   `coord_join_rate`.
/// - `fields` — the top-level field names of a JSON-object payload,
///   used for `field_completeness`. For non-JSON payloads, this is
///   empty.
/// - `iteration` / `timestamp` — for `emit_cadence` and for
///   per-iteration dedup in [`crate::drift::detector::DriftDetector`].
/// - `wave_id` — when two snapshots share the same `wave_id` they
///   are treated as siblings from the same wave, not as a cadence
///   anomaly.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventSnapshot {
    /// The event topic.
    pub topic: String,
    /// The hat that published the event (when known).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hat: Option<String>,
    /// Top-level field names of the JSON-object payload. Empty for
    /// non-JSON payloads.
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub fields: BTreeSet<String>,
    /// The loop iteration the event was observed at.
    pub iteration: u32,
    /// Wall-clock time the event was observed.
    pub timestamp: DateTime<Utc>,
    /// Wave correlation id, when the event is part of a wave.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub wave_id: Option<String>,
}

impl EventSnapshot {
    /// Build a new `EventSnapshot` with `iteration` and `timestamp`
    /// defaulted to caller-controlled values.
    #[must_use]
    pub fn new(topic: impl Into<String>, iteration: u32, timestamp: DateTime<Utc>) -> Self {
        Self {
            topic: topic.into(),
            source_hat: None,
            fields: BTreeSet::new(),
            iteration,
            timestamp,
            wave_id: None,
        }
    }

    /// Builder-style setter for `source_hat`.
    #[must_use]
    pub fn with_source_hat(mut self, hat: impl Into<String>) -> Self {
        self.source_hat = Some(hat.into());
        self
    }

    /// Builder-style setter for `fields`.
    #[must_use]
    pub fn with_fields(mut self, fields: BTreeSet<String>) -> Self {
        self.fields = fields;
        self
    }

    /// Builder-style setter for `wave_id`.
    #[must_use]
    pub fn with_wave_id(mut self, wave_id: impl Into<String>) -> Self {
        self.wave_id = Some(wave_id.into());
        self
    }
}

/// A bounded ring buffer of [`EventSnapshot`]s.
///
/// The window is the detector's only state: it remembers the most
/// recent `capacity` snapshots and discards older ones on overflow.
/// Iteration order is preserved (`push` appends, `iter` walks in
/// insertion order).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftWindow {
    /// Storage. A `VecDeque` gives O(1) push-back and pop-front.
    #[serde(skip)]
    storage: VecDeque<EventSnapshot>,
    /// Hard upper bound on the number of snapshots kept. Eviction
    /// triggers once `len() == capacity`.
    capacity: usize,
}

impl DriftWindow {
    /// Create an empty window with the given capacity. `capacity`
    /// must be greater than zero; the detector validates this in
    /// [`crate::config::telemetry::DriftConfig::validate`] before
    /// constructing a [`DriftDetector`].
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            storage: VecDeque::with_capacity(capacity),
            capacity,
        }
    }

    /// Create a window pre-loaded with `events`, capped at
    /// `capacity`. Useful for tests and for replaying a recorded
    /// session.
    #[must_use]
    pub fn from_events(events: Vec<EventSnapshot>, capacity: usize) -> Self {
        let mut window = Self::new(capacity);
        for e in events {
            window.push(e);
        }
        window
    }

    /// Append `snapshot` to the window. If the window is full, the
    /// oldest snapshot is evicted to make room.
    pub fn push(&mut self, snapshot: EventSnapshot) {
        if self.capacity == 0 {
            // Defensive: capacity=0 was rejected by config validation,
            // but the unit tests for the window accept it as a degenerate
            // "always-evict" window. Push/pop would be a busy loop, so
            // we silently drop instead.
            return;
        }
        if self.storage.len() == self.capacity {
            self.storage.pop_front();
        }
        self.storage.push_back(snapshot);
    }

    /// Iterate over snapshots in insertion order (oldest first).
    pub fn iter(&self) -> impl Iterator<Item = &EventSnapshot> {
        self.storage.iter()
    }

    /// Number of snapshots currently held.
    #[must_use]
    pub fn len(&self) -> usize {
        self.storage.len()
    }

    /// True when the window holds no snapshots.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.storage.is_empty()
    }

    /// Maximum number of snapshots the window can hold.
    #[must_use]
    pub fn capacity(&self) -> usize {
        self.capacity
    }
}
