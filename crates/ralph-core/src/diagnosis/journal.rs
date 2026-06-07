//! JSONL record types for `.ralph/diagnostics/<session>/recovery.jsonl`
//! and `drift.jsonl`.
//!
//! These are pure data types. U3 will write them to disk; U7's
//! `ralph diagnose` will read them back. The report pipeline never
//! reaches into the in-memory orchestrator state.
//!
//! Each line of the JSONL files is one [`RecoveryJournalEntry`] or one
//! [`DriftJournalEntry`]. Entries are forward-compatible — adding
//! optional fields does not break existing readers.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use super::envelope::{
    DiagnosisSeverity, EvidenceKind, EvidenceRef, RecoveryDiagnosisEnvelope,
    RecoveryDiagnosisEnvelopeBuilder,
};

/// Schema version of [`RecoveryJournalEntry`].
pub const RECOVERY_JOURNAL_SCHEMA_VERSION: u32 = 1;
/// Schema version of [`DriftJournalEntry`].
pub const DRIFT_JOURNAL_SCHEMA_VERSION: u32 = 1;

/// One record in `recovery.jsonl`.
///
/// Carries the full [`RecoveryDiagnosisEnvelope`] plus a free-form
/// `notes` field for contextual information from the caller (e.g. the
/// contract that was rejected, or a short hint about the iteration
/// context). `iteration` and `timestamp` are duplicated on the entry
/// so the report can sort and bucket without dereferencing the
/// envelope.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryJournalEntry {
    /// Schema version. Always [`RECOVERY_JOURNAL_SCHEMA_VERSION`].
    pub schema_version: u32,

    /// The envelope being recorded.
    pub envelope: RecoveryDiagnosisEnvelope,

    /// Loop iteration. Mirrors [`RecoveryDiagnosisEnvelope::iteration`]
    /// for easy top-level sort.
    pub iteration: u32,

    /// Wall-clock time. Mirrors [`RecoveryDiagnosisEnvelope::timestamp`].
    pub timestamp: DateTime<Utc>,

    /// Free-form contextual notes from the caller. Typically empty,
    /// but useful when the envelope alone is not enough to
    /// disambiguate a finding.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub notes: Vec<String>,
}

impl RecoveryJournalEntry {
    /// Build a journal entry from an envelope and a list of notes.
    /// `iteration` and `timestamp` are copied from the envelope.
    #[must_use]
    pub fn from_envelope(envelope: RecoveryDiagnosisEnvelope, notes: Vec<String>) -> Self {
        Self {
            schema_version: RECOVERY_JOURNAL_SCHEMA_VERSION,
            iteration: envelope.iteration,
            timestamp: envelope.timestamp,
            envelope,
            notes,
        }
    }
}

impl From<RecoveryDiagnosisEnvelope> for RecoveryJournalEntry {
    fn from(envelope: RecoveryDiagnosisEnvelope) -> Self {
        Self::from_envelope(envelope, Vec::new())
    }
}

/// One record in `drift.jsonl`. Drift findings are produced by U5 and
/// converted into recovery envelopes by [`DriftJournalEntry::into_envelope`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DriftJournalEntry {
    /// Schema version. Always [`DRIFT_JOURNAL_SCHEMA_VERSION`].
    pub schema_version: u32,

    /// Unique id for this finding. UUIDv4 string.
    pub finding_id: String,

    /// Which metric produced the finding.
    pub metric: DriftMetric,

    /// Observed value of the metric (e.g. `0.42` for `field_completeness`).
    pub observed_value: f64,

    /// The threshold the metric was compared against.
    pub threshold: f64,

    /// Severity bucket.
    pub severity: DiagnosisSeverity,

    /// Topic the finding is about (when applicable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,

    /// Field the finding is about (when applicable).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,

    /// Source topic in a coord-join finding. `None` for non-coord
    /// metrics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_topic: Option<String>,

    /// Target topic in a coord-join finding. `None` for non-coord
    /// metrics.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to_topic: Option<String>,

    /// Number of iterations the drift window covered.
    pub window_iterations: u32,

    /// Human-readable explanation.
    pub message: String,

    /// Wall-clock time the finding was produced.
    pub timestamp: DateTime<Utc>,

    /// Loop iteration the finding was produced at.
    pub iteration: u32,
}

impl DriftJournalEntry {
    /// Construct a [`DriftJournalEntry`]. `finding_id` is auto-filled
    /// with a fresh UUIDv4 if `None` is passed.
    #[must_use]
    pub fn builder() -> DriftJournalEntryBuilder {
        DriftJournalEntryBuilder::default()
    }

    /// Convert this finding into a [`RecoveryDiagnosisEnvelope`]
    /// (source = [`super::envelope::DiagnosisSource::DriftMonitor`])
    /// so it can flow through the same recovery journal / report
    /// pipeline.
    #[must_use]
    pub fn into_envelope(self) -> RecoveryDiagnosisEnvelope {
        let mut builder = RecoveryDiagnosisEnvelopeBuilder::new(
            super::envelope::DiagnosisSource::DriftMonitor,
            self.severity,
        )
        .iteration(self.iteration)
        .reason_code(self.metric.reason_code())
        .message(self.message)
        .retry_attempt(0)
        .safe_target(false)
        .expected_action(format!(
            "Investigate {} drift: observed {:.4} below threshold {:.4}",
            self.metric.as_str(),
            self.observed_value,
            self.threshold,
        ))
        .retry_key(format!(
            "drift_monitor:{}:{}:{}:{}",
            self.metric.as_str(),
            self.topic.as_deref().unwrap_or("*"),
            self.from_topic.as_deref().unwrap_or("*"),
            self.to_topic.as_deref().unwrap_or("*"),
        ));
        if let Some(topic) = self.topic {
            builder = builder.topic(topic);
        }
        if let Some(field) = self.field {
            builder = builder.evidence(EvidenceRef {
                kind: EvidenceKind::Field,
                ref_path: field,
                snippet: None,
            });
        }
        builder.build()
    }
}

impl From<DriftJournalEntry> for RecoveryDiagnosisEnvelope {
    fn from(entry: DriftJournalEntry) -> Self {
        entry.into_envelope()
    }
}

/// Drift metrics tracked by U5. Each variant maps to a stable
/// `reason_code` used in [`RecoveryDiagnosisEnvelope::retry_key`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DriftMetric {
    /// Per-`(topic, field)` field completeness ratio.
    FieldCompleteness,
    /// Per-`(from_topic, to_topic)` coordination join rate.
    CoordJoinRate,
    /// Per-topic emit cadence (inter-emit interval).
    EmitCadence,
}

impl DriftMetric {
    /// Stable snake_case label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            DriftMetric::FieldCompleteness => "field_completeness",
            DriftMetric::CoordJoinRate => "coord_join_rate",
            DriftMetric::EmitCadence => "emit_cadence",
        }
    }

    /// Stable reason code used inside [`RecoveryDiagnosisEnvelope::reason_code`].
    #[must_use]
    pub const fn reason_code(self) -> &'static str {
        match self {
            DriftMetric::FieldCompleteness => "drift_field_completeness",
            DriftMetric::CoordJoinRate => "drift_coord_join_rate",
            DriftMetric::EmitCadence => "drift_emit_cadence",
        }
    }
}

/// Builder for [`DriftJournalEntry`]. Drift findings are produced in
/// U5; this builder is the only sanctioned way to construct one.
#[derive(Debug, Default, Clone)]
pub struct DriftJournalEntryBuilder {
    finding_id: Option<String>,
    metric: Option<DriftMetric>,
    observed_value: Option<f64>,
    threshold: Option<f64>,
    severity: Option<DiagnosisSeverity>,
    topic: Option<String>,
    field: Option<String>,
    from_topic: Option<String>,
    to_topic: Option<String>,
    window_iterations: Option<u32>,
    message: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    iteration: Option<u32>,
}

impl DriftJournalEntryBuilder {
    /// Override the finding id. When unset, [`Self::build`] stamps a
    /// fresh UUIDv4.
    #[must_use]
    pub fn finding_id(mut self, id: impl Into<String>) -> Self {
        self.finding_id = Some(id.into());
        self
    }

    /// Set the metric.
    #[must_use]
    pub fn metric(mut self, m: DriftMetric) -> Self {
        self.metric = Some(m);
        self
    }

    /// Set the observed value.
    #[must_use]
    pub fn observed_value(mut self, v: f64) -> Self {
        self.observed_value = Some(v);
        self
    }

    /// Set the threshold.
    #[must_use]
    pub fn threshold(mut self, v: f64) -> Self {
        self.threshold = Some(v);
        self
    }

    /// Set the severity.
    #[must_use]
    pub fn severity(mut self, s: DiagnosisSeverity) -> Self {
        self.severity = Some(s);
        self
    }

    /// Set the topic.
    #[must_use]
    pub fn topic(mut self, t: impl Into<String>) -> Self {
        self.topic = Some(t.into());
        self
    }

    /// Set the field.
    #[must_use]
    pub fn field(mut self, f: impl Into<String>) -> Self {
        self.field = Some(f.into());
        self
    }

    /// Set the source topic of a coord-join finding. The name mirrors
    /// the JSON field name on [`DriftJournalEntry`]; the method is a
    /// pure setter and consumes `self` by builder convention.
    #[allow(clippy::wrong_self_convention)]
    #[must_use]
    pub fn from_topic(mut self, t: impl Into<String>) -> Self {
        self.from_topic = Some(t.into());
        self
    }

    /// Set the target topic of a coord-join finding. The name mirrors
    /// the JSON field name on [`DriftJournalEntry`]; the method is a
    /// pure setter and consumes `self` by builder convention.
    #[allow(clippy::wrong_self_convention)]
    #[must_use]
    pub fn to_topic(mut self, t: impl Into<String>) -> Self {
        self.to_topic = Some(t.into());
        self
    }

    /// Set the window size in iterations.
    #[must_use]
    pub fn window_iterations(mut self, n: u32) -> Self {
        self.window_iterations = Some(n);
        self
    }

    /// Set the human-readable message.
    #[must_use]
    pub fn message(mut self, m: impl Into<String>) -> Self {
        self.message = Some(m.into());
        self
    }

    /// Set the timestamp. Defaults to `Utc::now()`.
    #[must_use]
    pub fn timestamp(mut self, t: DateTime<Utc>) -> Self {
        self.timestamp = Some(t);
        self
    }

    /// Set the loop iteration.
    #[must_use]
    pub fn iteration(mut self, i: u32) -> Self {
        self.iteration = Some(i);
        self
    }

    /// Finalize the entry.
    #[must_use]
    pub fn build(self) -> DriftJournalEntry {
        DriftJournalEntry {
            schema_version: DRIFT_JOURNAL_SCHEMA_VERSION,
            finding_id: self
                .finding_id
                .unwrap_or_else(|| uuid::Uuid::new_v4().to_string()),
            metric: self.metric.unwrap_or(DriftMetric::FieldCompleteness),
            observed_value: self.observed_value.unwrap_or(0.0),
            threshold: self.threshold.unwrap_or(0.0),
            severity: self.severity.unwrap_or(DiagnosisSeverity::Info),
            topic: self.topic,
            field: self.field,
            from_topic: self.from_topic,
            to_topic: self.to_topic,
            window_iterations: self.window_iterations.unwrap_or(0),
            message: self.message.unwrap_or_default(),
            timestamp: self.timestamp.unwrap_or_else(Utc::now),
            iteration: self.iteration.unwrap_or(0),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnosis::envelope::DiagnosisSource;

    #[test]
    fn from_envelope_round_trips() {
        let env = RecoveryDiagnosisEnvelope::builder()
            .iteration(3)
            .reason_code("r")
            .message("m")
            .build();
        let entry = RecoveryJournalEntry::from_envelope(env.clone(), vec!["note".to_string()]);
        assert_eq!(entry.schema_version, RECOVERY_JOURNAL_SCHEMA_VERSION);
        assert_eq!(entry.envelope, env);
        assert_eq!(entry.iteration, 3);
        assert_eq!(entry.notes, vec!["note".to_string()]);

        let s = serde_json::to_string(&entry).unwrap();
        let back: RecoveryJournalEntry = serde_json::from_str(&s).unwrap();
        assert_eq!(back, entry);
    }

    #[test]
    fn drift_entry_into_envelope_uses_drift_source() {
        let entry = DriftJournalEntry::builder()
            .metric(DriftMetric::FieldCompleteness)
            .observed_value(0.4)
            .threshold(0.9)
            .severity(DiagnosisSeverity::Warning)
            .topic("work.done")
            .field("plan_name")
            .window_iterations(20)
            .iteration(7)
            .message("plan_name missing in 60% of events")
            .build();
        let env: RecoveryDiagnosisEnvelope = entry.into_envelope();
        assert_eq!(env.source, DiagnosisSource::DriftMonitor);
        assert!(env.retry_key.starts_with("drift_monitor:"));
        assert_eq!(env.severity, DiagnosisSeverity::Warning);
        assert_eq!(env.iteration, 7);
        assert!(!env.evidence.is_empty());
    }

    #[test]
    fn drift_metric_serde_uses_snake_case() {
        for metric in [
            DriftMetric::FieldCompleteness,
            DriftMetric::CoordJoinRate,
            DriftMetric::EmitCadence,
        ] {
            let s = serde_json::to_string(&metric).unwrap();
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v.as_str().unwrap(), metric.as_str());
        }
    }
}
