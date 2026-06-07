//! [`RecoveryDiagnosisEnvelope`] and the related source / severity / outcome
//! enums used by every recovery, drift and report path.
//!
//! The envelope is a passive, additive data structure. It captures:
//!
//! - which subsystem produced the diagnosis ([`DiagnosisSource`]),
//! - how serious it is ([`DiagnosisSeverity`]),
//! - which hat is responsible ([`source_hat`]) and which hat is the
//!   recommended action target ([`target_hat`]),
//! - a stable [`retry_key`] for cross-iteration aggregation, and
//! - the observed [`DiagnosisOutcome`] once the responder has watched
//!   what happened next.
//!
//! Construct envelopes via [`RecoveryDiagnosisEnvelope::builder`]. The
//! builder is the only sanctioned entry point: it auto-fills
//! `schema_version`, `diagnosis_id`, `timestamp`, and a derived
//! `retry_key`.
//!
//! # `retry_key` stability
//!
//! [`retry_key_from_parts`] hashes `(source, target_hat, topic, reason_code,
//! field)` into a stable, snake-cased, hash-safe string. The same inputs
//! must always produce the same key, regardless of when or where they run,
//! so U3's recovery logger and U6's responder can join the same
//! `retry_key` group across iterations and loggers.
//!
//! [`retry_key`]: RecoveryDiagnosisEnvelope::retry_key
//! [`retry_key_from_parts`]: RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

/// Schema version of [`RecoveryDiagnosisEnvelope`].
///
/// Bump this whenever the JSON shape changes in a non-additive way.
pub const RECOVERY_ENVELOPE_SCHEMA_VERSION: u32 = 1;

/// Maximum character length of the [`RecoveryDiagnosisEnvelope::message`]
/// field after [`RecoveryDiagnosisEnvelopeBuilder::build`] runs. Anything
/// longer is truncated and suffixed with `\u{2026}` so a single
/// pathological payload cannot blow up `recovery.jsonl`.
pub const MAX_ENVELOPE_MESSAGE_CHARS: usize = 1024;

/// Maximum character length of [`EvidenceRef::snippet`]. Snippets are
/// hints, not full payloads; longer text should be written to a file
/// and referenced by `ref_path` instead.
pub const MAX_EVIDENCE_SNIPPET_CHARS: usize = 256;

/// Origin of a diagnosis. Maps to the recovery / drift subsystem that
/// produced the envelope.
///
/// JSON representation is stable snake_case. Renaming a variant is a
/// breaking change for the diagnostics report.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisSource {
    /// Hat produced no events for the entire iteration; the loop
    /// injected a fallback `task.resume`.
    StallRecovery,
    /// A hat had a publishing obligation but the current iteration
    /// produced nothing on its declared topic.
    MissingEventGate,
    /// Workflow phase/chain guard rejected an out-of-order event.
    WorkflowGuard,
    /// Execution contract rejected a completion event.
    ExecutionContract,
    /// Preset payload contract violation, usually a hard failure.
    PayloadContract,
    /// Drift monitor detected a degradation in field completeness,
    /// coord join rate, or emit cadence.
    DriftMonitor,
    /// External hook (pre/post agent, completion) had to be retried.
    HookRetry,
    /// Loop itself went stale (no progress across iterations); usually
    /// paired with `LoopState::LoopStale`.
    LoopStale,
}

impl DiagnosisSource {
    /// Stable string label used in JSON output and the
    /// [`RecoveryDiagnosisEnvelope::retry_key`].
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            DiagnosisSource::StallRecovery => "stall_recovery",
            DiagnosisSource::MissingEventGate => "missing_event_gate",
            DiagnosisSource::WorkflowGuard => "workflow_guard",
            DiagnosisSource::ExecutionContract => "execution_contract",
            DiagnosisSource::PayloadContract => "payload_contract",
            DiagnosisSource::DriftMonitor => "drift_monitor",
            DiagnosisSource::HookRetry => "hook_retry",
            DiagnosisSource::LoopStale => "loop_stale",
        }
    }
}

/// Severity of a diagnosis. Severity drives the report ranking and the
/// `ralph diagnose` Markdown table.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisSeverity {
    /// Informational; usually a `soft` prompt alert.
    Info,
    /// Warning; the loop can still proceed but operators should look.
    Warning,
    /// Error; a recovery action was injected or a rejection happened.
    Error,
    /// Critical; the loop is likely to terminate or has already.
    Critical,
}

impl DiagnosisSeverity {
    /// Stable snake_case label for JSON output.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            DiagnosisSeverity::Info => "info",
            DiagnosisSeverity::Warning => "warning",
            DiagnosisSeverity::Error => "error",
            DiagnosisSeverity::Critical => "critical",
        }
    }
}

/// State of a diagnosis across iterations.
///
/// Envelopes are written with [`DiagnosisOutcome::Pending`] and may be
/// updated later (via [`RecoveryDiagnosisEnvelope::with_outcome`]) when
/// the responder observes what happened next. The U3 recovery logger
/// keeps all updates in the same journal so the report can show the
/// full recovery timeline for a single `retry_key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosisOutcome {
    /// Diagnosis has been recorded but no follow-up has been observed
    /// yet. The default state for fresh envelopes.
    Pending,
    /// The responder observed the underlying issue heal in a later
    /// iteration (e.g. the target hat emitted the expected topic).
    Recovered,
    /// The same `retry_key` was seen more than once within the
    /// configured retry window.
    Repeated,
    /// The responder escalated to a higher-tier action (e.g. a hard
    /// `task.resume` or a human-guidance injection) after repeated
    /// failure.
    Escalated,
    /// The diagnosis contributed to a loop termination or hard pause.
    Failed,
    /// The diagnosis describes an issue that is not actionable by
    /// retrying (e.g. a payload contract violation that requires the
    /// preset author to fix the preset).
    NotRetriable,
}

impl DiagnosisOutcome {
    /// Stable snake_case label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            DiagnosisOutcome::Pending => "pending",
            DiagnosisOutcome::Recovered => "recovered",
            DiagnosisOutcome::Repeated => "repeated",
            DiagnosisOutcome::Escalated => "escalated",
            DiagnosisOutcome::Failed => "failed",
            DiagnosisOutcome::NotRetriable => "not_retriable",
        }
    }
}

/// Kind of evidence a [`EvidenceRef`] points at. Used by the report
/// renderer to bucket findings.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceKind {
    /// A field name in a payload schema.
    Field,
    /// A path to a file in the workspace.
    File,
    /// A log key or short log line (e.g. an error tag).
    Log,
    /// A topic name (with or without wildcard).
    Topic,
    /// A hat name.
    Hat,
    /// Anything that does not fit the other kinds; prefer a more
    /// specific kind when possible.
    Other,
}

impl EvidenceKind {
    /// Stable snake_case label.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            EvidenceKind::Field => "field",
            EvidenceKind::File => "file",
            EvidenceKind::Log => "log",
            EvidenceKind::Topic => "topic",
            EvidenceKind::Hat => "hat",
            EvidenceKind::Other => "other",
        }
    }
}

/// Reference to a piece of evidence supporting a diagnosis.
///
/// `ref_path` is a short pointer (a field name, a relative file path, a
/// log key, a topic or hat name). `snippet` is an optional,
/// already-truncated hint and MUST be `<= 256` characters. Long
/// evidence should be written to a file and referenced by `ref_path`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceRef {
    /// What kind of evidence this points at.
    pub kind: EvidenceKind,
    /// Short pointer — a field name, file path, log key, topic, hat.
    pub ref_path: String,
    /// Optional pre-truncated hint. Always `<= 256` characters.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snippet: Option<String>,
}

impl EvidenceRef {
    /// Construct a new `EvidenceRef`, truncating `snippet` to
    /// [`MAX_EVIDENCE_SNIPPET_CHARS`] characters (UTF-8 safe). If the
    /// snippet is truncated, `…` (`U+2026`) is appended.
    #[must_use]
    pub fn new(kind: EvidenceKind, ref_path: impl Into<String>, snippet: Option<String>) -> Self {
        let snippet = snippet.map(|s| truncate_evidence_snippet(&s));
        Self {
            kind,
            ref_path: ref_path.into(),
            snippet,
        }
    }
}

/// The shared recovery / diagnosis payload. Every recovery, drift and
/// report path funnels into this shape.
///
/// Construct via [`RecoveryDiagnosisEnvelope::builder`]. The builder
/// stamps `schema_version`, `diagnosis_id`, `timestamp`, and the
/// derived `retry_key`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecoveryDiagnosisEnvelope {
    /// Schema version. Always [`RECOVERY_ENVELOPE_SCHEMA_VERSION`] for
    /// envelopes built via [`RecoveryDiagnosisEnvelope::builder`].
    pub schema_version: u32,

    /// Unique identifier for this envelope. UUIDv4 string.
    pub diagnosis_id: String,

    /// Loop iteration at which the envelope was produced.
    pub iteration: u32,

    /// Origin subsystem.
    pub source: DiagnosisSource,

    /// Severity bucket.
    pub severity: DiagnosisSeverity,

    /// The hat that produced the offending event (or `None` for
    /// hat-less diagnostics such as `payload_contract` or
    /// `loop_stale`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source_hat: Option<String>,

    /// The hat that is expected to act on this diagnosis. `None` means
    /// "no safe target" — i.e. the responder should fall back to a
    /// pause / report-only path rather than emitting `task.resume`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_hat: Option<String>,

    /// Topic related to the diagnosis (e.g. the rejected topic).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub topic: Option<String>,

    /// Stable, machine-readable reason code (e.g. `missing_field`,
    /// `out_of_order_phase`, `no_git_evidence`).
    pub reason_code: String,

    /// Human-readable explanation. Truncated to
    /// [`MAX_ENVELOPE_MESSAGE_CHARS`] characters by
    /// [`RecoveryDiagnosisEnvelopeBuilder::build`].
    pub message: String,

    /// Recommended next action (free-form). Surfaced in prompt
    /// injection and the report.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_action: Option<String>,

    /// Supporting evidence. Kept short by
    /// [`EvidenceRef::new`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub evidence: Vec<EvidenceRef>,

    /// Stable cross-iteration aggregation key. Built from
    /// `(source, target_hat, topic, reason_code, field)` via
    /// [`RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts`].
    /// Two envelopes with the same `retry_key` are aggregated into
    /// the same recovery group in `ralph diagnose`.
    pub retry_key: String,

    /// 0-based attempt counter for the same `retry_key` group.
    /// `0` means the first time the issue is observed; a positive
    /// value means it has been re-observed. U6's responder
    /// increments this when it sees the same `retry_key` again
    /// inside the configured retry window.
    pub retry_attempt: u32,

    /// True when the responder has a safe hat to route the recovery
    /// action to. False signals "no safe target" — the responder
    /// should not synthesize a fake target.
    pub safe_target: bool,

    /// Current observed outcome. Defaults to [`DiagnosisOutcome::Pending`]
    /// and is updated via [`RecoveryDiagnosisEnvelope::with_outcome`]
    /// by U6.
    pub outcome: DiagnosisOutcome,

    /// Wall-clock time the envelope was created. Stamped by
    /// [`RecoveryDiagnosisEnvelopeBuilder::build`].
    pub timestamp: DateTime<Utc>,

    /// Diagnostics session id (the timestamped directory name). Set
    /// by U3's recovery logger; not required by callers that just
    /// want to construct an envelope.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl RecoveryDiagnosisEnvelope {
    /// Begin building an envelope.
    #[must_use]
    pub fn builder() -> RecoveryDiagnosisEnvelopeBuilder {
        RecoveryDiagnosisEnvelopeBuilder::default()
    }

    /// Returns a new envelope with the same identity (`diagnosis_id`,
    /// `retry_key`, ...) but updated `outcome` and `retry_attempt`.
    ///
    /// U6 (Recovery Responder) calls this to record the observed
    /// outcome of a recovery action. All other fields are preserved
    /// verbatim, so the updated envelope can be appended to the same
    /// `recovery.jsonl` group as the original.
    #[must_use]
    pub fn with_outcome(&self, outcome: DiagnosisOutcome, retry_attempt: u32) -> Self {
        Self {
            schema_version: self.schema_version,
            diagnosis_id: self.diagnosis_id.clone(),
            iteration: self.iteration,
            source: self.source,
            severity: self.severity,
            source_hat: self.source_hat.clone(),
            target_hat: self.target_hat.clone(),
            topic: self.topic.clone(),
            reason_code: self.reason_code.clone(),
            message: self.message.clone(),
            expected_action: self.expected_action.clone(),
            evidence: self.evidence.clone(),
            retry_key: self.retry_key.clone(),
            retry_attempt,
            safe_target: self.safe_target,
            outcome,
            timestamp: self.timestamp,
            session_id: self.session_id.clone(),
        }
    }
}

/// Builder for [`RecoveryDiagnosisEnvelope`]. `source` and `severity` are
/// required; everything else is optional and may be filled in across
/// multiple calls before [`RecoveryDiagnosisEnvelopeBuilder::build`].
#[derive(Debug, Default, Clone)]
pub struct RecoveryDiagnosisEnvelopeBuilder {
    iteration: Option<u32>,
    source: Option<DiagnosisSource>,
    severity: Option<DiagnosisSeverity>,
    source_hat: Option<String>,
    target_hat: Option<String>,
    topic: Option<String>,
    reason_code: Option<String>,
    message: Option<String>,
    expected_action: Option<String>,
    evidence: Vec<EvidenceRef>,
    retry_key: Option<String>,
    retry_attempt: u32,
    safe_target: bool,
    outcome: Option<DiagnosisOutcome>,
    session_id: Option<String>,
}

impl RecoveryDiagnosisEnvelopeBuilder {
    /// Start a builder pre-loaded with the required `source` and
    /// `severity`. `retry_attempt` defaults to `0`, `outcome` to
    /// `Pending`, and `safe_target` to `false`.
    #[must_use]
    pub fn new(source: DiagnosisSource, severity: DiagnosisSeverity) -> Self {
        Self {
            source: Some(source),
            severity: Some(severity),
            ..Self::default()
        }
    }

    /// Set the iteration.
    #[must_use]
    pub fn iteration(mut self, iteration: u32) -> Self {
        self.iteration = Some(iteration);
        self
    }

    /// Override the source. Useful when adapting a builder constructed
    /// via [`RecoveryDiagnosisEnvelope::builder`] (which has no
    /// source) into a specific source — for example, when
    /// [`crate::diagnosis::DriftJournalEntry::into_envelope`]
    /// funnels a drift finding into a `DriftMonitor` envelope.
    #[must_use]
    pub fn source(mut self, source: DiagnosisSource) -> Self {
        self.source = Some(source);
        self
    }

    /// Override the severity. See [`Self::source`] for usage.
    #[must_use]
    pub fn severity(mut self, severity: DiagnosisSeverity) -> Self {
        self.severity = Some(severity);
        self
    }

    /// Set the source hat (the hat that produced the offending event).
    #[must_use]
    pub fn source_hat(mut self, hat: impl Into<String>) -> Self {
        self.source_hat = Some(hat.into());
        self
    }

    /// Set the target hat (the hat that is expected to act).
    #[must_use]
    pub fn target_hat(mut self, hat: impl Into<String>) -> Self {
        self.target_hat = Some(hat.into());
        self
    }

    /// Set the related topic.
    #[must_use]
    pub fn topic(mut self, topic: impl Into<String>) -> Self {
        self.topic = Some(topic.into());
        self
    }

    /// Set the reason code (stable, machine-readable).
    #[must_use]
    pub fn reason_code(mut self, code: impl Into<String>) -> Self {
        self.reason_code = Some(code.into());
        self
    }

    /// Set the human-readable message. Truncated to
    /// [`MAX_ENVELOPE_MESSAGE_CHARS`] by [`Self::build`].
    #[must_use]
    pub fn message(mut self, message: impl Into<String>) -> Self {
        self.message = Some(message.into());
        self
    }

    /// Set the expected next action (free-form).
    #[must_use]
    pub fn expected_action(mut self, action: impl Into<String>) -> Self {
        self.expected_action = Some(action.into());
        self
    }

    /// Append an evidence reference.
    #[must_use]
    pub fn evidence(mut self, ev: EvidenceRef) -> Self {
        self.evidence.push(ev);
        self
    }

    /// Override the retry key. When unset, [`Self::build`] derives it
    /// from `(source, target_hat, topic, reason_code, field)` via
    /// [`Self::retry_key_from_parts`] using the last [`EvidenceKind::Field`]
    /// entry in [`Self::evidence`] as the `field` part.
    #[must_use]
    pub fn retry_key(mut self, key: impl Into<String>) -> Self {
        self.retry_key = Some(key.into());
        self
    }

    /// Set the retry attempt counter. Defaults to `0`.
    #[must_use]
    pub fn retry_attempt(mut self, n: u32) -> Self {
        self.retry_attempt = n;
        self
    }

    /// Set the safe-target flag. Defaults to `false` so callers must
    /// explicitly opt in to a targeted recovery.
    #[must_use]
    pub fn safe_target(mut self, b: bool) -> Self {
        self.safe_target = b;
        self
    }

    /// Set the initial outcome. Defaults to
    /// [`DiagnosisOutcome::Pending`].
    #[must_use]
    pub fn outcome(mut self, outcome: DiagnosisOutcome) -> Self {
        self.outcome = Some(outcome);
        self
    }

    /// Set the diagnostics session id (the timestamped directory name).
    #[must_use]
    pub fn session_id(mut self, id: impl Into<String>) -> Self {
        self.session_id = Some(id.into());
        self
    }

    /// Derive a stable, snake-cased retry key from its parts.
    ///
    /// Format: `"{source}:{target_or_*}:{topic_or_*}:{reason_code}:{field_or_*}"`
    /// where each part has been normalized to lowercase snake_case and
    /// any non-`[a-z0-9_]` character replaced with `_`. `None` parts
    /// become the literal `*` placeholder. The same inputs must
    /// always produce the same key.
    #[must_use]
    pub fn retry_key_from_parts(
        source: DiagnosisSource,
        target_hat: Option<&str>,
        topic: Option<&str>,
        reason_code: &str,
        field: Option<&str>,
    ) -> String {
        let target = target_hat
            .map(normalize_part)
            .unwrap_or_else(|| "*".to_string());
        let topic = topic.map(normalize_part).unwrap_or_else(|| "*".to_string());
        let reason = normalize_part(reason_code);
        let field = field.map(normalize_part).unwrap_or_else(|| "*".to_string());
        format!(
            "{}:{}:{}:{}:{}",
            source.as_str(),
            target,
            topic,
            reason,
            field
        )
    }

    /// Finalize the builder into a [`RecoveryDiagnosisEnvelope`].
    ///
    /// Stamps `schema_version = 1`, `diagnosis_id` (fresh UUIDv4),
    /// `timestamp = Utc::now()`, derives a default `retry_key` if none
    /// was provided, and truncates `message` to
    /// [`MAX_ENVELOPE_MESSAGE_CHARS`].
    #[must_use]
    pub fn build(self) -> RecoveryDiagnosisEnvelope {
        let source = self.source.unwrap_or(DiagnosisSource::StallRecovery);
        let severity = self.severity.unwrap_or(DiagnosisSeverity::Info);
        let message_raw = self.message.unwrap_or_default();
        let message = truncate_envelope_message(&message_raw);
        let reason_code = self.reason_code.unwrap_or_default();
        let topic = self.topic.as_deref();
        let target = self.target_hat.as_deref();
        let field = self
            .evidence
            .iter()
            .rev()
            .find(|e| matches!(e.kind, EvidenceKind::Field))
            .map(|e| e.ref_path.as_str());
        let retry_key = self.retry_key.unwrap_or_else(|| {
            Self::retry_key_from_parts(source, target, topic, &reason_code, field)
        });
        RecoveryDiagnosisEnvelope {
            schema_version: RECOVERY_ENVELOPE_SCHEMA_VERSION,
            diagnosis_id: uuid::Uuid::new_v4().to_string(),
            iteration: self.iteration.unwrap_or(0),
            source,
            severity,
            source_hat: self.source_hat,
            target_hat: self.target_hat,
            topic: self.topic,
            reason_code,
            message,
            expected_action: self.expected_action,
            evidence: self.evidence,
            retry_key,
            retry_attempt: self.retry_attempt,
            safe_target: self.safe_target,
            outcome: self.outcome.unwrap_or(DiagnosisOutcome::Pending),
            timestamp: Utc::now(),
            session_id: self.session_id,
        }
    }
}

/// Truncate a message to [`MAX_ENVELOPE_MESSAGE_CHARS`] characters,
/// appending `\u{2026}` (the Unicode horizontal ellipsis) when truncated.
fn truncate_envelope_message(s: &str) -> String {
    let char_count = s.chars().count();
    if char_count <= MAX_ENVELOPE_MESSAGE_CHARS {
        return s.to_string();
    }
    // Keep `MAX_ENVELOPE_MESSAGE_CHARS - 1` chars, then append `…`.
    let keep = MAX_ENVELOPE_MESSAGE_CHARS.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('\u{2026}');
    out
}

/// Truncate a snippet to [`MAX_EVIDENCE_SNIPPET_CHARS`] characters,
/// appending `\u{2026}` (the Unicode horizontal ellipsis) when
/// truncated. Empty snippets stay empty.
fn truncate_evidence_snippet(s: &str) -> String {
    let char_count = s.chars().count();
    if char_count == 0 {
        return String::new();
    }
    if char_count <= MAX_EVIDENCE_SNIPPET_CHARS {
        return s.to_string();
    }
    let keep = MAX_EVIDENCE_SNIPPET_CHARS.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('\u{2026}');
    out
}

/// Normalize a `retry_key` part to lowercase snake_case.
fn normalize_part(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut prev_underscore = false;
    for c in s.chars() {
        if c.is_ascii_alphanumeric() {
            for lc in c.to_lowercase() {
                if lc.is_ascii_alphanumeric() {
                    out.push(lc);
                    prev_underscore = false;
                }
            }
        } else if !prev_underscore {
            out.push('_');
            prev_underscore = true;
        }
    }
    let trimmed = out.trim_matches('_').to_string();
    if trimmed.is_empty() {
        "_".to_string()
    } else {
        trimmed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn source_as_str_matches_serde() {
        for source in [
            DiagnosisSource::StallRecovery,
            DiagnosisSource::MissingEventGate,
            DiagnosisSource::WorkflowGuard,
            DiagnosisSource::ExecutionContract,
            DiagnosisSource::PayloadContract,
            DiagnosisSource::DriftMonitor,
            DiagnosisSource::HookRetry,
            DiagnosisSource::LoopStale,
        ] {
            let s = serde_json::to_string(&source).unwrap();
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v.as_str().unwrap(), source.as_str());
        }
    }

    #[test]
    fn severity_as_str_matches_serde() {
        for severity in [
            DiagnosisSeverity::Info,
            DiagnosisSeverity::Warning,
            DiagnosisSeverity::Error,
            DiagnosisSeverity::Critical,
        ] {
            let s = serde_json::to_string(&severity).unwrap();
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v.as_str().unwrap(), severity.as_str());
        }
    }

    #[test]
    fn outcome_as_str_matches_serde() {
        for outcome in [
            DiagnosisOutcome::Pending,
            DiagnosisOutcome::Recovered,
            DiagnosisOutcome::Repeated,
            DiagnosisOutcome::Escalated,
            DiagnosisOutcome::Failed,
            DiagnosisOutcome::NotRetriable,
        ] {
            let s = serde_json::to_string(&outcome).unwrap();
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v.as_str().unwrap(), outcome.as_str());
        }
    }

    #[test]
    fn evidence_kind_as_str_matches_serde() {
        for kind in [
            EvidenceKind::Field,
            EvidenceKind::File,
            EvidenceKind::Log,
            EvidenceKind::Topic,
            EvidenceKind::Hat,
            EvidenceKind::Other,
        ] {
            let s = serde_json::to_string(&kind).unwrap();
            let v: serde_json::Value = serde_json::from_str(&s).unwrap();
            assert_eq!(v.as_str().unwrap(), kind.as_str());
        }
    }

    #[test]
    fn retry_key_from_parts_is_stable() {
        let key_a = RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
            DiagnosisSource::MissingEventGate,
            Some("builder"),
            Some("work.done"),
            "no_emit",
            Some("plan_name"),
        );
        let key_b = RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
            DiagnosisSource::MissingEventGate,
            Some("builder"),
            Some("work.done"),
            "no_emit",
            Some("plan_name"),
        );
        assert_eq!(key_a, key_b);
    }

    #[test]
    fn retry_key_from_parts_normalizes_parts() {
        let key = RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
            DiagnosisSource::ExecutionContract,
            Some("Builder Hat"),
            Some("work.Done"),
            "Missing Field",
            Some("plan-name.v2"),
        );
        assert_eq!(
            key,
            "execution_contract:builder_hat:work_done:missing_field:plan_name_v2"
        );
    }

    #[test]
    fn retry_key_from_parts_substitutes_wildcards() {
        let key = RecoveryDiagnosisEnvelopeBuilder::retry_key_from_parts(
            DiagnosisSource::LoopStale,
            None,
            None,
            "stale",
            None,
        );
        assert_eq!(key, "loop_stale:*:*:stale:*");
    }

    #[test]
    fn evidence_ref_truncates_long_snippet() {
        let long = "x".repeat(MAX_EVIDENCE_SNIPPET_CHARS + 50);
        let r = EvidenceRef::new(EvidenceKind::Log, "log", Some(long));
        let snippet = r.snippet.unwrap();
        assert_eq!(snippet.chars().count(), MAX_EVIDENCE_SNIPPET_CHARS);
        assert!(snippet.ends_with('\u{2026}'));
    }

    #[test]
    fn evidence_ref_keeps_short_snippet_intact() {
        let r = EvidenceRef::new(EvidenceKind::Log, "log", Some("ERROR: foo".to_string()));
        assert_eq!(r.snippet.as_deref(), Some("ERROR: foo"));
    }

    #[test]
    fn build_stamps_schema_version_and_id_and_timestamp() {
        let before = Utc::now();
        let env = RecoveryDiagnosisEnvelope::builder()
            .iteration(7)
            .reason_code("test")
            .message("hello")
            .build();
        let after = Utc::now();
        assert_eq!(env.schema_version, RECOVERY_ENVELOPE_SCHEMA_VERSION);
        assert!(!env.diagnosis_id.is_empty());
        // UUIDv4 strings are 36 characters.
        assert_eq!(env.diagnosis_id.len(), 36);
        assert!(env.timestamp >= before && env.timestamp <= after);
    }

    #[test]
    fn with_outcome_preserves_identity() {
        let env = RecoveryDiagnosisEnvelope::builder()
            .iteration(2)
            .reason_code("r1")
            .message("m")
            .retry_attempt(0)
            .safe_target(true)
            .build();
        let next = env.with_outcome(DiagnosisOutcome::Recovered, 1);
        assert_eq!(next.diagnosis_id, env.diagnosis_id);
        assert_eq!(next.retry_key, env.retry_key);
        assert_eq!(next.source, env.source);
        assert_eq!(next.reason_code, env.reason_code);
        assert_eq!(next.iteration, env.iteration);
        assert_eq!(next.outcome, DiagnosisOutcome::Recovered);
        assert_eq!(next.retry_attempt, 1);
        assert_eq!(next.safe_target, env.safe_target);
    }

    #[test]
    fn long_message_is_truncated() {
        let long = "a".repeat(MAX_ENVELOPE_MESSAGE_CHARS + 200);
        let env = RecoveryDiagnosisEnvelope::builder()
            .reason_code("r")
            .message(long)
            .build();
        assert_eq!(env.message.chars().count(), MAX_ENVELOPE_MESSAGE_CHARS);
        assert!(env.message.ends_with('\u{2026}'));
    }

    #[test]
    fn default_retry_key_uses_field_evidence_ref() {
        let env = RecoveryDiagnosisEnvelope::builder()
            .reason_code("missing_field")
            .message("m")
            .evidence(EvidenceRef::new(EvidenceKind::File, "src/x.rs", None))
            .evidence(EvidenceRef::new(EvidenceKind::Field, "plan_name", None))
            .build();
        // The builder should pick the most recent Field evidence as the
        // `field` part of the retry key.
        assert!(
            env.retry_key.ends_with(":plan_name"),
            "retry key = {}",
            env.retry_key
        );
    }

    #[test]
    fn explicit_retry_key_overrides_derivation() {
        let env = RecoveryDiagnosisEnvelope::builder()
            .reason_code("r")
            .message("m")
            .retry_key("custom:key")
            .build();
        assert_eq!(env.retry_key, "custom:key");
    }
}
