//! `ralph diagnose` offline report pipeline (U7 + U8).
//!
//! Reads the session artifacts written by U3 (recovery / drift /
//! orchestration / errors JSONL plus the optional
//! `diagnosis-summary.json` seed) and renders an operator-facing
//! report. The report is purely additive: it never mutates the
//! artifacts and never reaches into the live orchestrator state.
//!
//! The pipeline is split into three layers:
//!
//! 1. [`resolve_session`] — pick a session directory from `--session`
//!    (`latest` or an explicit path) under a diagnostics root.
//! 2. [`load_session`] — parse the four JSONL files plus the optional
//!    summary seed into [`SessionData`]. Malformed lines and missing
//!    files become warnings; the report is always produced.
//! 3. [`render_markdown`] / [`render_json`] — render a stable, schema
//!    versioned report from [`SessionData`].
//!
//! U8 (2026-06-21-002): the unified-state plan adds a ledger-first
//! entry point, [`report_from_ledger`].  It reads the workspace-level
//! `.ralph/recovery.jsonl` (written by the U7a
//! `state::append_rejection` path) plus the
//! `.ralph/ledger.jsonl` commit log, aggregates the rejection
//! records into structured [`RootCause`]s, and renders a
//! [`DiagnosisReport`].  Operators running `ralph diagnose` on a
//! loop that used the deterministic-correction feature flag get
//! the ledger-aligned view; legacy sessions without a
//! `.ralph/recovery.jsonl` fall back to the session-level
//! `recovery.jsonl` (the existing U3 behaviour).
//!
//! Aggregation, ranking, and the "Suggested next actions" heuristic
//! all live in [`render_markdown`] / [`render_json`] so the reporter
//! itself stays a thin I/O layer.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io::{self, BufRead, BufReader};
use std::path::{Path, PathBuf};

use serde_json::{Value, json};
use thiserror::Error;

use super::envelope::{DiagnosisOutcome, DiagnosisSeverity, RecoveryDiagnosisEnvelope};
use super::journal::{DriftJournalEntry, RecoveryJournalEntry};
use crate::diagnostics::DiagnosisSummary;
use crate::diagnostics::{OrchestrationEntry, OrchestrationEvent};
use crate::hat_lifecycle::ActivationSnapshot;

/// Schema version of the `ralph diagnose` JSON output. Bump when the
/// JSON shape changes non-additively; CI consumers key on this.
pub const DIAGNOSE_JSON_SCHEMA_VERSION: &str = "1";

/// Filename of the optional seed written by the loop on termination.
const DIAGNOSIS_SUMMARY_FILENAME: &str = "diagnosis-summary.json";

/// Filename of the recovery journal written by U3.
const RECOVERY_FILENAME: &str = "recovery.jsonl";

/// Filename of the drift journal written by U3.
const DRIFT_FILENAME: &str = "drift.jsonl";

/// Filename of the orchestration audit log written by U3.
const ORCHESTRATION_FILENAME: &str = "orchestration.jsonl";

/// Filename of the errors log.
const ERRORS_FILENAME: &str = "errors.jsonl";

/// Filename of the active activations snapshot written at termination.
const ACTIVE_ACTIVATIONS_FILENAME: &str = "active-activations.json";

/// Errors that abort `ralph diagnose` before any report is rendered.
/// `ReporterError::NoSession` is the only error the CLI turns into a
/// non-zero exit code; all other errors are surfaced as warnings
/// inside the report.
#[derive(Debug, Error)]
pub enum ReporterError {
    /// The diagnostics root is missing or empty.
    #[error("no diagnostics sessions found under {0}")]
    NoSession(PathBuf),
    /// The explicit session path does not exist or is not a directory.
    #[error("session path {0} is not a valid diagnostics session directory")]
    InvalidSession(PathBuf),
    /// I/O error when reading the diagnostics root.
    #[error("failed to read diagnostics directory {0}: {1}")]
    Io(PathBuf, io::Error),
}

/// What the CLI asked the reporter to do with `--session`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSelector<'a> {
    /// `None` or `"latest"` — pick the most recent timestamped session.
    Latest,
    /// An explicit path, relative or absolute.
    Explicit(&'a str),
}

/// All session data the reporter needs to render a report. The
/// reporter is a pure data structure — it carries warnings but never
/// the I/O state that produced them.
#[derive(Debug, Clone, Default)]
pub struct SessionData {
    /// Absolute path to the session directory.
    pub session_path: PathBuf,
    /// Optional diagnosis summary seed (loop start / end / counts).
    pub summary: Option<DiagnosisSummary>,
    /// Parsed `recovery.jsonl` entries, in file order.
    pub recovery: Vec<RecoveryJournalEntry>,
    /// Parsed `drift.jsonl` entries, in file order.
    pub drift: Vec<DriftJournalEntry>,
    /// Parsed `orchestration.jsonl` entries, in file order.
    pub orchestration: Vec<OrchestrationEntry>,
    /// Parsed `errors.jsonl` entries, in file order (raw `Value`s;
    /// the schema is not versioned here).
    pub errors: Vec<Value>,
    /// Free-form warnings (malformed lines, missing files, I/O
    /// errors). Surfaced in both Markdown and JSON reports.
    pub warnings: Vec<String>,
    /// Active hat activation snapshots written at loop termination.
    /// U4: populated from `active-activations.json` in the session dir.
    pub active_activations: Vec<ActivationSnapshot>,
}

/// Public rank for a single finding. The reporter pre-aggregates by
/// `retry_key` and sorts groups with this struct.
#[derive(Debug, Clone)]
pub struct RankedFinding {
    /// `retry_key` this group was aggregated on.
    pub retry_key: String,
    /// Latest severity in the group.
    pub severity: DiagnosisSeverity,
    /// Latest outcome in the group.
    pub outcome: DiagnosisOutcome,
    /// Diagnosis source for the latest entry in the group.
    pub source: String,
    /// Target hat for the latest entry in the group.
    pub target_hat: Option<String>,
    /// Topic for the latest entry in the group.
    pub topic: Option<String>,
    /// Reason code for the latest entry in the group.
    pub reason_code: String,
    /// Human-readable message for the latest entry in the group.
    pub message: String,
    /// Number of envelopes aggregated into this group.
    pub occurrences: u32,
    /// First iteration the group was observed at.
    pub first_iteration: Option<u32>,
    /// Latest iteration the group was observed at.
    pub last_iteration: Option<u32>,
    /// Evidence refs from the latest entry.
    pub evidence: Vec<super::envelope::EvidenceRef>,
    /// True when the latest entry had a safe target.
    pub safe_target: bool,
    /// True when the group escalated at least once (`outcome == Escalated`).
    pub escalated: bool,
}

/// Per-source-hat aggregate used in the "Recovery timeline" section.
#[derive(Debug, Clone)]
pub struct TimelineRow {
    pub iteration: Option<u32>,
    pub hat: String,
    pub severity: DiagnosisSeverity,
    pub outcome: DiagnosisOutcome,
    pub target_hat: Option<String>,
    pub topic: Option<String>,
    pub reason_code: String,
    pub message: String,
}

/// Drift finding summary, lifted from `drift.jsonl` for the
/// "Drift findings" section.
#[derive(Debug, Clone)]
pub struct DriftSummary {
    pub metric: String,
    pub topic: Option<String>,
    pub field: Option<String>,
    pub from_topic: Option<String>,
    pub to_topic: Option<String>,
    pub observed_value: f64,
    pub threshold: f64,
    pub severity: DiagnosisSeverity,
    pub iteration: u32,
    pub message: String,
}

/// Final report struct consumed by `render_markdown` / `render_json`.
/// Keeping it separate from [`SessionData`] lets the report
/// implementation pre-compute aggregations once.
#[derive(Debug, Clone)]
pub struct Report {
    /// Schema version of the report (matches
    /// [`DIAGNOSE_JSON_SCHEMA_VERSION`]).
    pub schema_version: &'static str,
    /// Path to the session directory.
    pub session_path: PathBuf,
    /// Optional summary seed (loop start / end / counts).
    pub summary: Option<DiagnosisSummary>,
    /// Aggregated top findings, ranked per the U7 plan.
    pub top_findings: Vec<RankedFinding>,
    /// Recovery timeline, one row per (latest) entry in `recovery.jsonl`.
    pub recovery_timeline: Vec<TimelineRow>,
    /// Drift findings, in file order.
    pub drift_findings: Vec<DriftSummary>,
    /// Orchestration audit (full entries).
    pub orchestration: Vec<OrchestrationEntry>,
    /// Error entries (raw `Value`s).
    pub errors: Vec<Value>,
    /// Free-form warnings (malformed lines, missing files).
    pub warnings: Vec<String>,
    /// Active hat activation snapshots (U4). Sorted by duration
    /// descending (longest active first).
    pub active_activations: Vec<ActivationSnapshot>,
}

impl Report {
    /// Build a [`Report`] from [`SessionData`] using the standard U7
    /// ranking and aggregation rules.
    #[must_use]
    pub fn from_session(data: &SessionData) -> Self {
        let top_findings = aggregate_recovery(&data.recovery);
        let recovery_timeline = recovery_timeline(&data.recovery);
        // U4: sort active activations by duration descending (longest first).
        let mut active_activations = data.active_activations.clone();
        active_activations.sort_by_key(|a| std::cmp::Reverse(a.duration));
        let drift_findings = data
            .drift
            .iter()
            .map(|d| DriftSummary {
                metric: d.metric.as_str().to_string(),
                topic: d.topic.clone(),
                field: d.field.clone(),
                from_topic: d.from_topic.clone(),
                to_topic: d.to_topic.clone(),
                observed_value: d.observed_value,
                threshold: d.threshold,
                severity: d.severity,
                iteration: d.iteration,
                message: d.message.clone(),
            })
            .collect();
        Self {
            schema_version: DIAGNOSE_JSON_SCHEMA_VERSION,
            session_path: data.session_path.clone(),
            summary: data.summary.clone(),
            top_findings,
            recovery_timeline,
            drift_findings,
            orchestration: data.orchestration.clone(),
            errors: data.errors.clone(),
            warnings: data.warnings.clone(),
            active_activations,
        }
    }
}

// ── U8 (2026-06-21-002 plan): ledger-aligned diagnosis ──────────────────
//
// The legacy `Report` is built from session-scoped JSONL files under
// `.ralph/diagnostics/<id>/`.  U8 adds a second, ledger-aligned
// entry point that reads the workspace-level
// `.ralph/recovery.jsonl` (written by `state::append_rejection`)
// plus the `.ralph/ledger.jsonl` commit log and emits a flat,
// structured [`DiagnosisReport`].  The two views coexist:
// `ralph diagnose --from-ledger` prefers the ledger view, and
// `ralph diagnose --legacy` falls back to the session-scoped view
// when the workspace has no rejection log.

/// A single structured root cause, derived from one or more
/// rejection records that share the same `reason_code`.
///
/// The reporter groups rejection records by
/// `(stage, reason_code)` and lifts the common fields into a
/// [`RootCause`].  The original records are not lost — see
/// [`RejectionSummary::record_count`] for the raw count and
/// [`DiagnosisReport::rejection_summary`] for the global
/// counters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RootCause {
    /// The pipeline stage that produced the rejection (e.g.
    /// `origin`, `policy`, `execution_contract`).  Drives
    /// `source` via [`crate::diagnosis::validation_stage_to_source`].
    pub stage: String,
    /// The stable, machine-readable reason code
    /// (e.g. `origin:missing_field`,
    /// `execution_contract:type_mismatch`).
    pub reason_code: String,
    /// How many rejection records share this `reason_code`.
    pub frequency: u32,
    /// Earliest `ts` seen in the grouped records (RFC3339).
    pub first_seen_ts: String,
    /// Latest `ts` seen in the grouped records (RFC3339).
    pub last_seen_ts: String,
    /// Distinct `hat` values seen across the grouped records
    /// (deduped, sorted).
    pub affected_hats: Vec<String>,
    /// Distinct `topic` values seen across the grouped records
    /// (deduped, sorted).
    pub affected_topics: Vec<String>,
    /// Free-form remediation hint derived from the stage +
    /// reason_code.  Kept short and operator-friendly; surfaced
    /// in the Markdown "Root causes" section.
    pub suggested_remediation: String,
    /// True when the most recent record carried a
    /// `terminal_reason` (R11 escalation).
    pub escalated: bool,
}

/// Summary counters for the rejection log.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RejectionSummary {
    /// Total number of records parsed (after skipping malformed
    /// lines).  Mirrors the per-key counts but is the headline
    /// metric for the report.
    pub record_count: u32,
    /// Number of distinct `(stage, reason_code)` pairs.
    pub distinct_reason_codes: u32,
    /// Number of records that tripped R11 escalation
    /// (carried a non-`None` `terminal_reason`).
    pub terminal_count: u32,
    /// Oldest `ts` in the log (RFC3339, empty when the log is empty).
    pub first_seen_ts: String,
    /// Newest `ts` in the log (RFC3339, empty when the log is empty).
    pub last_seen_ts: String,
    /// Distinct hats seen (deduped, sorted).
    pub affected_hats: Vec<String>,
    /// Distinct topics seen (deduped, sorted).
    pub affected_topics: Vec<String>,
}

/// Summary counters derived from the `.ralph/ledger.jsonl` commit
/// log.  U8 uses these to surface "how many state mutations did
/// this run persist" without re-replaying the full snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LedgerSummary {
    /// Path of the commit log that was inspected.
    pub commit_log_path: PathBuf,
    /// Total number of commits parsed (after skipping malformed
    /// lines).
    pub commit_count: u32,
    /// Number of `CommitDelta::RejectionRecorded` commits.
    pub rejection_commits: u32,
    /// Number of `CommitDelta::RejectionBudgetTripped` commits.
    pub escalation_commits: u32,
    /// Number of `CommitDelta::TaskLifecycle` commits.
    pub task_lifecycle_commits: u32,
    /// Number of `CommitDelta::HandoffAccepted` commits.
    pub handoff_commits: u32,
    /// True when the commit log could not be read because of a
    /// parse error (replay stopped at the first bad line).  The
    /// caller can then fall back to the legacy session view.
    pub corruption: bool,
    /// Free-form warnings (parse errors, I/O errors, etc.).
    pub warnings: Vec<String>,
}

/// Ledger-aligned diagnosis report.  Built by
/// [`report_from_ledger`] from a workspace root.
///
/// Distinct from the session-level [`Report`]: the session view
/// preserves the full per-iteration timeline, while the
/// ledger view is a flat, structured root-cause summary keyed
/// off `.ralph/recovery.jsonl` + `.ralph/ledger.jsonl`.  The
/// CLI renders either view through the same `render_markdown` /
/// `render_json` pipeline, but the U8 `DiagnosisReport` has its
/// own dedicated renderer to keep the schema separate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DiagnosisReport {
    /// Schema version of the report.  Distinct from
    /// [`DIAGNOSE_JSON_SCHEMA_VERSION`] (the session-level
    /// report) so consumers can tell the two views apart.
    pub schema_version: &'static str,
    /// Workspace root the report was built from.
    pub workspace: PathBuf,
    /// True when the reporter successfully read at least one
    /// rejection record from the workspace-level recovery log.
    /// When `false`, the caller should fall back to the
    /// session-level view.
    pub used_ledger: bool,
    /// Aggregated root causes, sorted by `frequency` descending
    /// then `reason_code` ascending.
    pub root_causes: Vec<RootCause>,
    /// Summary counters for the rejection log.
    pub rejection_summary: RejectionSummary,
    /// Summary counters for the ledger commit log.
    pub ledger_summary: LedgerSummary,
}

/// Schema version of the U8 [`DiagnosisReport`].  Distinct from
/// the session-level report so CI consumers can branch on
/// `schema_version`.
pub const DIAGNOSIS_LEDGER_SCHEMA_VERSION: &str = "u8-1";

/// Errors that abort `report_from_ledger` before any report is
/// rendered.  Currently only used for "the rejection log is
/// missing AND the session-scoped fallback also failed".  The
/// `report_from_ledger` path is best-effort and returns an
/// empty `DiagnosisReport` when neither source is available.
#[derive(Debug, Error)]
pub enum LedgerReportError {
    /// The workspace is missing or unreadable.
    #[error("workspace path {0} is not a directory")]
    InvalidWorkspace(PathBuf),
    /// I/O error reading the workspace.
    #[error("failed to read workspace {0}: {1}")]
    Io(PathBuf, io::Error),
}

/// Read the rejection log from `<workspace>/.ralph/recovery.jsonl`.
///
/// Public so the CLI can reuse the helper when `--from-ledger` is
/// requested.  The session-level `ReporterError` is unaffected.
pub fn read_rejection_records(
    workspace: &Path,
) -> std::io::Result<Vec<crate::state::RejectionRecord>> {
    crate::state::read_rejection_log(workspace)
}

/// Build a [`DiagnosisReport`] from a workspace root.
///
/// Reads two artifacts:
///
/// 1. `<workspace>/.ralph/recovery.jsonl` (U7a
///    `state::append_rejection` log) — every `RejectionRecord`
///    is grouped by `(stage, reason_code)` to produce
///    [`RootCause`] entries.
/// 2. `<workspace>/.ralph/ledger.jsonl` (U1 commit log) — the
///    per-commit counters feed [`LedgerSummary`].
///
/// When the workspace-level rejection log does not exist the
/// function returns an empty report with `used_ledger = false`
/// so the CLI can fall back to the session-level
/// `ReporterError::NoSession` path.
pub fn report_from_ledger(workspace: &Path) -> Result<DiagnosisReport, LedgerReportError> {
    if !workspace.is_dir() {
        return Err(LedgerReportError::InvalidWorkspace(workspace.to_path_buf()));
    }

    // 1) Rejection log.
    let rejection_records = match read_rejection_records(workspace) {
        Ok(records) => records,
        Err(err) => {
            return Err(LedgerReportError::Io(workspace.to_path_buf(), err));
        }
    };

    // 2) Commit log (best-effort — a missing file is not an error).
    let ledger_summary = read_ledger_summary(workspace);

    // 3) Aggregate rejections into RootCause.
    let (root_causes, rejection_summary) = aggregate_rejection_records(&rejection_records);

    Ok(DiagnosisReport {
        schema_version: DIAGNOSIS_LEDGER_SCHEMA_VERSION,
        workspace: workspace.to_path_buf(),
        used_ledger: !rejection_records.is_empty() || ledger_summary.commit_count > 0,
        root_causes,
        rejection_summary,
        ledger_summary,
    })
}

/// Internal: read the commit log and produce a [`LedgerSummary`].
/// Parse errors are recorded in `warnings` and do not abort the
/// summary (the caller still gets a report).
fn read_ledger_summary(workspace: &Path) -> LedgerSummary {
    let mut summary = LedgerSummary {
        commit_log_path: workspace.join(crate::state::LEDGER_RELATIVE_PATH),
        ..LedgerSummary::default()
    };
    let commits = match crate::state::read_commit_log(workspace) {
        Ok(commits) => commits,
        Err(err) => {
            summary.warnings.push(format!(
                "failed to read commit log {}: {err}",
                summary.commit_log_path.display()
            ));
            // `LedgerError::Parse` carries the offending line;
            // surface the corruption flag so the report can
            // flag the data as partial.
            if matches!(err, crate::state::LedgerError::Parse { .. }) {
                summary.corruption = true;
            }
            return summary;
        }
    };
    summary.commit_count = commits.len() as u32;
    for commit in &commits {
        match &commit.delta {
            crate::state::CommitDelta::RejectionRecorded { .. } => {
                summary.rejection_commits = summary.rejection_commits.saturating_add(1);
            }
            crate::state::CommitDelta::RejectionBudgetTripped { .. } => {
                summary.escalation_commits = summary.escalation_commits.saturating_add(1);
            }
            crate::state::CommitDelta::TaskLifecycle { .. }
            | crate::state::CommitDelta::TaskInserted { .. } => {
                summary.task_lifecycle_commits = summary.task_lifecycle_commits.saturating_add(1);
            }
            crate::state::CommitDelta::HandoffAccepted { .. } => {
                summary.handoff_commits = summary.handoff_commits.saturating_add(1);
            }
            _ => {}
        }
    }
    summary
}

/// Internal: aggregate a slice of `RejectionRecord`s into
/// `(Vec<RootCause>, RejectionSummary)`.  Split out for testability.
fn aggregate_rejection_records(
    records: &[crate::state::RejectionRecord],
) -> (Vec<RootCause>, RejectionSummary) {
    if records.is_empty() {
        return (Vec::new(), RejectionSummary::default());
    }
    // Group by (stage, reason_code).  `stage` is the prefix of
    // the `reason_code` string (U7a writes
    // `"{stage}:{reason_class}"`); we split on the first `:`.
    let mut order: Vec<(String, String)> = Vec::new();
    let mut groups: HashMap<(String, String), RejectionGroup> = HashMap::new();
    for record in records {
        let (stage, reason_class) = split_reason_code(&record.reason_code);
        let key = (stage.to_string(), reason_class.to_string());
        let group = groups.entry(key.clone()).or_insert_with(|| {
            order.push(key.clone());
            RejectionGroup::default()
        });
        group.observe(record);
    }
    let mut summary = RejectionSummary::default();
    summary.record_count = records.len() as u32;
    summary.affected_hats = deduped_sorted(records.iter().map(|r| r.hat.clone()));
    summary.affected_topics = deduped_sorted(records.iter().map(|r| r.topic.clone()));
    // First/last seen timestamps (record.ts is RFC3339; lexical
    // ordering matches chronological ordering for fixed-width
    // RFC3339 strings).
    if let Some(first) = records.iter().map(|r| r.ts.clone()).min() {
        summary.first_seen_ts = first;
    }
    if let Some(last) = records.iter().map(|r| r.ts.clone()).max() {
        summary.last_seen_ts = last;
    }
    summary.terminal_count = records
        .iter()
        .filter(|r| r.terminal_reason.is_some())
        .count() as u32;
    summary.distinct_reason_codes = groups.len() as u32;

    let mut root_causes: Vec<RootCause> = order
        .into_iter()
        .filter_map(|key| groups.remove(&key).map(|group| group.finalize(key)))
        .collect();
    // Sort by frequency desc, then reason_code asc for stability.
    root_causes.sort_by(|a, b| {
        b.frequency
            .cmp(&a.frequency)
            .then_with(|| a.reason_code.cmp(&b.reason_code))
    });
    (root_causes, summary)
}

/// Split a `reason_code` like `"origin:missing_field"` into
/// `("origin", "missing_field")`.  When the code has no `:`,
/// the whole string is the `reason_class` and the `stage` is
/// empty.
fn split_reason_code(reason_code: &str) -> (&str, &str) {
    match reason_code.split_once(':') {
        Some((stage, class)) => (stage, class),
        None => ("", reason_code),
    }
}

fn deduped_sorted<I: Iterator<Item = String>>(iter: I) -> Vec<String> {
    use std::collections::BTreeSet;
    let set: BTreeSet<String> = iter.collect();
    set.into_iter().collect()
}

#[derive(Debug, Clone, Default)]
struct RejectionGroup {
    frequency: u32,
    first_seen_ts: String,
    last_seen_ts: String,
    hats: BTreeSet<String>,
    topics: BTreeSet<String>,
    escalated: bool,
    last_message: String,
}

impl RejectionGroup {
    fn observe(&mut self, record: &crate::state::RejectionRecord) {
        self.frequency = self.frequency.saturating_add(1);
        if self.first_seen_ts.is_empty() || record.ts < self.first_seen_ts {
            self.first_seen_ts = record.ts.clone();
        }
        if record.ts > self.last_seen_ts {
            self.last_seen_ts = record.ts.clone();
        }
        if record.terminal_reason.is_some() {
            self.escalated = true;
        }
        self.hats.insert(record.hat.clone());
        self.topics.insert(record.topic.clone());
        // Keep the most recent message (highest `ts`); ties keep
        // the previous value so the first observation wins.
        if self.last_message.is_empty() || record.ts >= self.last_seen_ts {
            self.last_message = format!(
                "{}{}",
                record.topic,
                if let Some(reason) = &record.terminal_reason {
                    format!(" [terminal: {reason}]")
                } else {
                    String::new()
                }
            );
        }
    }

    fn finalize(self, key: (String, String)) -> RootCause {
        let (stage, reason_class) = key;
        let reason_code = if stage.is_empty() {
            reason_class.clone()
        } else {
            format!("{stage}:{reason_class}")
        };
        let suggested_remediation = suggested_remediation_for(&stage, &reason_class);
        let mut affected_hats: Vec<String> = self.hats.into_iter().collect();
        affected_hats.sort();
        let mut affected_topics: Vec<String> = self.topics.into_iter().collect();
        affected_topics.sort();
        RootCause {
            stage,
            reason_code,
            frequency: self.frequency,
            first_seen_ts: self.first_seen_ts,
            last_seen_ts: self.last_seen_ts,
            affected_hats,
            affected_topics,
            suggested_remediation,
            escalated: self.escalated,
        }
    }
}

/// Free-form remediation hint keyed off `(stage, reason_class)`.
/// Kept short so the report can render it as one row in the
/// "Root causes" table.
fn suggested_remediation_for(stage: &str, reason_class: &str) -> String {
    match (stage, reason_class) {
        ("origin", "unknown_hat") => {
            "register the offending hat in the preset's `hats:` block".to_string()
        }
        ("origin", "out_of_scope") => {
            "tighten the offending hat's `publishes:` whitelist or move the emit to a \
             registered producer"
                .to_string()
        }
        ("origin", "missing_field") => {
            "ensure the offending event carries the schema-required field before publish"
                .to_string()
        }
        ("policy", "missing_field") => {
            "update the preset's `event_policy.schemas.<topic>.required_fields` for this topic"
                .to_string()
        }
        ("policy", "type_mismatch") => {
            "fix the payload to match the schema-declared type for this field".to_string()
        }
        ("policy", "invalid_topic_format") => {
            "use a topic name from the registered hat `publishes:` set or the system-control set"
                .to_string()
        }
        ("policy", "lint_failure") => {
            "rerun `ralph preset lint`; resolve the lint finding before re-emitting".to_string()
        }
        ("execution_contract", "missing_field") => {
            "add the schema-required field to the emitted payload".to_string()
        }
        ("execution_contract", "type_mismatch") => {
            "fix the payload type for the offending field".to_string()
        }
        ("execution_contract", "task_state") => {
            "ensure the referenced task is in the expected lifecycle state".to_string()
        }
        ("missing_event", _) => {
            "the hat had a publish obligation but emitted nothing; add the missing emit".to_string()
        }
        ("emit_claimed_but_not_written", _) => {
            "the agent mentioned `ralph emit` but no event was written; retry with an explicit \
             `ralph emit` line"
                .to_string()
        }
        ("hat_handoff", _) => {
            "the macro-edge `*.handoff.accepted` event failed the gate; rerun with a valid \
             handoff file"
                .to_string()
        }
        _ => "inspect the originating event payload and the preset's contract for the topic"
            .to_string(),
    }
}

/// Render a [`DiagnosisReport`] as a Markdown document.  Used by
/// `ralph diagnose --from-ledger` (U8) when the workspace-level
/// rejection log is available.
#[must_use]
pub fn render_diagnosis_report_markdown(report: &DiagnosisReport) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Ralph Diagnosis Report (U8 ledger view, schema v{})\n\n",
        report.schema_version
    ));
    out.push_str("## Workspace\n\n");
    out.push_str(&format!("- path: `{}`\n", report.workspace.display()));
    out.push_str(&format!(
        "- ledger source: {}\n",
        if report.used_ledger {
            "primary (`.ralph/recovery.jsonl` + `.ralph/ledger.jsonl`)"
        } else {
            "fallback (no rejection / commit log; session-scoped view applies)"
        }
    ));

    out.push_str("\n## Rejection summary\n\n");
    if report.rejection_summary.record_count == 0 {
        out.push_str("_No rejection records found._\n\n");
    } else {
        out.push_str(&format!(
            "- records: {}\n",
            report.rejection_summary.record_count
        ));
        out.push_str(&format!(
            "- distinct reason codes: {}\n",
            report.rejection_summary.distinct_reason_codes
        ));
        out.push_str(&format!(
            "- terminal (R11): {}\n",
            report.rejection_summary.terminal_count
        ));
        if !report.rejection_summary.first_seen_ts.is_empty() {
            out.push_str(&format!(
                "- first seen: {}\n",
                report.rejection_summary.first_seen_ts
            ));
        }
        if !report.rejection_summary.last_seen_ts.is_empty() {
            out.push_str(&format!(
                "- last seen: {}\n",
                report.rejection_summary.last_seen_ts
            ));
        }
        if !report.rejection_summary.affected_hats.is_empty() {
            out.push_str(&format!(
                "- affected hats: {}\n",
                report.rejection_summary.affected_hats.join(", ")
            ));
        }
        if !report.rejection_summary.affected_topics.is_empty() {
            out.push_str(&format!(
                "- affected topics: {}\n",
                report.rejection_summary.affected_topics.join(", ")
            ));
        }
        out.push('\n');
    }

    out.push_str("## Root causes\n\n");
    if report.root_causes.is_empty() {
        out.push_str("_No aggregated root causes._\n\n");
    } else {
        out.push_str("| reason_code | frequency | first→last | hats | topics | escalated | suggested remediation |\n");
        out.push_str("|---|---|---|---|---|---|---|\n");
        for rc in &report.root_causes {
            let hats = if rc.affected_hats.is_empty() {
                "*".to_string()
            } else {
                rc.affected_hats.join(", ")
            };
            let topics = if rc.affected_topics.is_empty() {
                "*".to_string()
            } else {
                rc.affected_topics.join(", ")
            };
            let first_last = if rc.first_seen_ts == rc.last_seen_ts {
                rc.first_seen_ts.clone()
            } else {
                format!("{} → {}", rc.first_seen_ts, rc.last_seen_ts)
            };
            out.push_str(&format!(
                "| `{}` | {} | {} | {} | {} | {} | {} |\n",
                rc.reason_code,
                rc.frequency,
                first_last,
                hats,
                topics,
                if rc.escalated { "yes" } else { "no" },
                truncate_md(&rc.suggested_remediation, 80),
            ));
        }
        out.push('\n');
    }

    out.push_str("## Ledger summary\n\n");
    if report.ledger_summary.commit_count == 0 && !report.ledger_summary.corruption {
        out.push_str("_No commit log found (or empty)._\n\n");
    } else {
        out.push_str(&format!(
            "- commit log: `{}`\n",
            report.ledger_summary.commit_log_path.display()
        ));
        if report.ledger_summary.commit_count > 0 {
            out.push_str(&format!(
                "- total commits: {}\n",
                report.ledger_summary.commit_count
            ));
            out.push_str(&format!(
                "- rejection recorded: {}\n",
                report.ledger_summary.rejection_commits
            ));
            out.push_str(&format!(
                "- escalation (R11): {}\n",
                report.ledger_summary.escalation_commits
            ));
            out.push_str(&format!(
                "- task lifecycle: {}\n",
                report.ledger_summary.task_lifecycle_commits
            ));
            out.push_str(&format!(
                "- handoff accepted: {}\n",
                report.ledger_summary.handoff_commits
            ));
        } else {
            out.push_str("- total commits: 0 (commit log could not be parsed)\n");
        }
        if report.ledger_summary.corruption {
            out.push_str(
                "- **warning**: commit log corruption detected; counters may be partial\n",
            );
        }
        out.push('\n');
    }
    if !report.ledger_summary.warnings.is_empty() {
        out.push_str("## Ledger warnings\n\n");
        for w in &report.ledger_summary.warnings {
            out.push_str(&format!("- {w}\n"));
        }
        out.push('\n');
    }
    out
}

/// Render a [`DiagnosisReport`] as a stable JSON document.
#[must_use]
pub fn render_diagnosis_report_json(report: &DiagnosisReport) -> Value {
    json!({
        "schema_version": report.schema_version,
        "workspace": report.workspace,
        "used_ledger": report.used_ledger,
        "rejection_summary": {
            "record_count": report.rejection_summary.record_count,
            "distinct_reason_codes": report.rejection_summary.distinct_reason_codes,
            "terminal_count": report.rejection_summary.terminal_count,
            "first_seen_ts": report.rejection_summary.first_seen_ts,
            "last_seen_ts": report.rejection_summary.last_seen_ts,
            "affected_hats": report.rejection_summary.affected_hats,
            "affected_topics": report.rejection_summary.affected_topics,
        },
        "ledger_summary": {
            "commit_log_path": report.ledger_summary.commit_log_path,
            "commit_count": report.ledger_summary.commit_count,
            "rejection_commits": report.ledger_summary.rejection_commits,
            "escalation_commits": report.ledger_summary.escalation_commits,
            "task_lifecycle_commits": report.ledger_summary.task_lifecycle_commits,
            "handoff_commits": report.ledger_summary.handoff_commits,
            "corruption": report.ledger_summary.corruption,
            "warnings": report.ledger_summary.warnings,
        },
        "root_causes": report.root_causes.iter().map(|rc| {
            json!({
                "stage": rc.stage,
                "reason_code": rc.reason_code,
                "frequency": rc.frequency,
                "first_seen_ts": rc.first_seen_ts,
                "last_seen_ts": rc.last_seen_ts,
                "affected_hats": rc.affected_hats,
                "affected_topics": rc.affected_topics,
                "suggested_remediation": rc.suggested_remediation,
                "escalated": rc.escalated,
                "source": crate::diagnosis::validation_stage_to_source(&rc.stage),
            })
        }).collect::<Vec<_>>(),
    })
}

/// Resolve `--session` against a diagnostics root. Returns the
/// absolute path of the chosen session directory.
///
/// - `Latest` picks the lexicographically largest timestamped
///   sub-directory of `diagnostics_root`. The `logs/` sub-directory
///   and any root-level `payload-contract-error-*.json` files are
///   ignored.
/// - `Explicit(s)` accepts both relative and absolute paths. When
///   `s` is an existing directory it is returned as-is. When it
///   looks like a timestamp (e.g. `2026-06-05T10-20-30`) it is
///   resolved against `diagnostics_root`. When neither applies, an
///   `InvalidSession` error is returned.
pub fn resolve_session<'a>(
    selector: SessionSelector<'a>,
    diagnostics_root: &Path,
) -> Result<PathBuf, ReporterError> {
    match selector {
        SessionSelector::Latest => resolve_latest(diagnostics_root),
        SessionSelector::Explicit(value) => resolve_explicit(value, diagnostics_root),
    }
}

fn resolve_latest(diagnostics_root: &Path) -> Result<PathBuf, ReporterError> {
    if !diagnostics_root.exists() {
        return Err(ReporterError::NoSession(diagnostics_root.to_path_buf()));
    }
    let entries = fs::read_dir(diagnostics_root)
        .map_err(|e| ReporterError::Io(diagnostics_root.to_path_buf(), e))?;
    let mut candidates: Vec<PathBuf> = Vec::new();
    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                // A single unreadable entry should not abort the
                // whole resolution: skip it but record nothing here
                // (no warnings API in this layer).
                eprintln!(
                    "ralph diagnose: skipping unreadable entry under {}: {e}",
                    diagnostics_root.display()
                );
                continue;
            }
        };
        let path = entry.path();
        if !path.is_dir() {
            // Ignore root-level files such as
            // `payload-contract-error-*.json` per the U7 plan.
            continue;
        }
        let name = match path.file_name().and_then(|n| n.to_str()) {
            Some(n) => n,
            None => continue,
        };
        if name == "logs" {
            // TUI mode always logs to `.ralph/diagnostics/logs/`.
            continue;
        }
        // Only accept timestamp-shaped directory names.
        if !looks_like_session_timestamp(name) {
            continue;
        }
        candidates.push(path);
    }
    let latest = candidates
        .into_iter()
        .max_by(|a, b| {
            let an = a.file_name().and_then(|n| n.to_str()).unwrap_or("");
            let bn = b.file_name().and_then(|n| n.to_str()).unwrap_or("");
            an.cmp(bn)
        })
        .ok_or_else(|| ReporterError::NoSession(diagnostics_root.to_path_buf()))?;
    Ok(latest)
}

fn resolve_explicit(value: &str, diagnostics_root: &Path) -> Result<PathBuf, ReporterError> {
    let candidate = PathBuf::from(value);
    let resolved = if candidate.is_absolute() {
        candidate
    } else if candidate.exists() {
        // Caller pointed at an existing path relative to the cwd.
        candidate
    } else if looks_like_session_timestamp(value) {
        // Treat as a session id relative to the diagnostics root.
        diagnostics_root.join(value)
    } else {
        return Err(ReporterError::InvalidSession(candidate));
    };
    if !resolved.is_dir() {
        return Err(ReporterError::InvalidSession(resolved));
    }
    Ok(resolved)
}

/// Heuristic for "this string is a timestamped session id". The
/// collector writes `2026-06-05T10-20-30` style ids. We accept
/// `YYYY-MM-DDTHH-MM-SS` plus a small extension to keep room for
/// future microseconds.
fn looks_like_session_timestamp(name: &str) -> bool {
    // Trim optional `.suffix` so callers can pass a sub-path.
    let head = name.split('.').next().unwrap_or(name);
    if head.len() < "YYYY-MM-DDTHH-MM-SS".len() {
        return false;
    }
    let bytes = head.as_bytes();
    let is_digit = |i: usize| bytes.get(i).is_some_and(|b| b.is_ascii_digit());
    let is_sep = |i: usize, c: char| bytes.get(i).copied() == Some(c as u8);
    is_digit(0)
        && is_digit(1)
        && is_digit(2)
        && is_digit(3)
        && is_sep(4, '-')
        && is_digit(5)
        && is_digit(6)
        && is_sep(7, '-')
        && is_digit(8)
        && is_digit(9)
        && (head.as_bytes().get(10) == Some(&b'T') || is_sep(10, 'T'))
}

/// Load all available session artifacts. Missing files and malformed
/// lines become warnings; the returned [`SessionData`] always
/// contains the session path and the warnings list, even when no
/// file could be read.
pub fn load_session(session_dir: &Path) -> SessionData {
    let mut warnings = Vec::new();
    let summary = read_summary(&session_dir.join(DIAGNOSIS_SUMMARY_FILENAME), &mut warnings);
    let recovery = read_recovery_journal(&session_dir.join(RECOVERY_FILENAME), &mut warnings);
    // U4 (2026-06-17-004): when the session recovery journal is
    // missing or empty, fall back to the workspace-level
    // `.ralph/recovery.jsonl` (cli_emit / wave_dimension_guard
    // rejections persist there). The session layout is
    // `<workspace>/.ralph/diagnostics/<id>/`, so the workspace
    // root is three parents up from the session dir.
    let recovery = if recovery.is_empty() {
        let workspace_root = session_dir
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent());
        match workspace_root {
            Some(root) => {
                let workspace_path = root.join(".ralph").join("recovery.jsonl");
                if workspace_path.exists() {
                    read_recovery_journal(&workspace_path, &mut warnings)
                } else {
                    Vec::new()
                }
            }
            None => recovery,
        }
    } else {
        recovery
    };
    let drift = read_drift_journal(&session_dir.join(DRIFT_FILENAME), &mut warnings);
    let orchestration =
        read_orchestration(&session_dir.join(ORCHESTRATION_FILENAME), &mut warnings);
    let errors = read_errors(&session_dir.join(ERRORS_FILENAME), &mut warnings);
    let active_activations = read_active_activations(
        &session_dir.join(ACTIVE_ACTIVATIONS_FILENAME),
        &mut warnings,
    );
    SessionData {
        session_path: session_dir.to_path_buf(),
        summary,
        recovery,
        drift,
        orchestration,
        errors,
        warnings,
        active_activations,
    }
}

fn read_summary(path: &Path, warnings: &mut Vec<String>) -> Option<DiagnosisSummary> {
    match fs::read_to_string(path) {
        Ok(content) => match serde_json::from_str::<DiagnosisSummary>(&content) {
            Ok(summary) => Some(summary),
            Err(err) => {
                push_warning(
                    warnings,
                    format!(
                        "{}: failed to parse diagnosis-summary.json ({err}); ignoring seed",
                        path.display()
                    ),
                );
                None
            }
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => None,
        Err(err) => {
            push_warning(
                warnings,
                format!(
                    "{}: I/O error reading diagnosis-summary.json: {err}",
                    path.display()
                ),
            );
            None
        }
    }
}

fn read_recovery_journal(path: &Path, warnings: &mut Vec<String>) -> Vec<RecoveryJournalEntry> {
    let display = path.display().to_string();
    read_jsonl(path, "recovery.jsonl", warnings, |line| {
        serde_json::from_str::<RecoveryJournalEntry>(line)
            .map_err(|err| format!("{display}: malformed recovery.jsonl line: {err}"))
    })
}

fn read_drift_journal(path: &Path, warnings: &mut Vec<String>) -> Vec<DriftJournalEntry> {
    let display = path.display().to_string();
    read_jsonl(path, "drift.jsonl", warnings, |line| {
        serde_json::from_str::<DriftJournalEntry>(line)
            .map_err(|err| format!("{display}: malformed drift.jsonl line: {err}"))
    })
}

fn read_orchestration(path: &Path, warnings: &mut Vec<String>) -> Vec<OrchestrationEntry> {
    let display = path.display().to_string();
    read_jsonl(path, "orchestration.jsonl", warnings, |line| {
        serde_json::from_str::<OrchestrationEntry>(line)
            .map_err(|err| format!("{display}: malformed orchestration.jsonl line: {err}"))
    })
}

fn read_errors(path: &Path, warnings: &mut Vec<String>) -> Vec<Value> {
    let display = path.display().to_string();
    read_jsonl(path, "errors.jsonl", warnings, |line| {
        serde_json::from_str::<Value>(line)
            .map_err(|err| format!("{display}: malformed errors.jsonl line: {err}"))
    })
}

/// Read `active-activations.json` — a JSON array of
/// [`ActivationSnapshot`]s written at loop termination (U4).
/// Missing file is not a warning (the file is only present when
/// diagnostics were enabled AND the loop terminated with activations).
fn read_active_activations(path: &Path, warnings: &mut Vec<String>) -> Vec<ActivationSnapshot> {
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(err) if err.kind() == io::ErrorKind::NotFound => return Vec::new(),
        Err(err) => {
            push_warning(
                warnings,
                format!(
                    "active-activations.json I/O error: {err} (path={})",
                    path.display()
                ),
            );
            return Vec::new();
        }
    };
    match serde_json::from_str::<Vec<ActivationSnapshot>>(&content) {
        Ok(v) => v,
        Err(err) => {
            push_warning(
                warnings,
                format!("active-activations.json: malformed JSON ({err}); ignoring",),
            );
            Vec::new()
        }
    }
}

/// Generic JSONL reader: each line is `String::trim()`ed, blank
/// lines are skipped, and the per-line parser converts the line to
/// `Result<T, String>` where the `Err` variant is the warning to
/// record for that line.
fn read_jsonl<T, F>(path: &Path, label: &str, warnings: &mut Vec<String>, mut parse: F) -> Vec<T>
where
    F: FnMut(&str) -> Result<T, String>,
{
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(err) if err.kind() == io::ErrorKind::NotFound => {
            push_warning(
                warnings,
                format!(
                    "{label} not found in session (expected at {})",
                    path.display()
                ),
            );
            return Vec::new();
        }
        Err(err) => {
            push_warning(
                warnings,
                format!("{label} I/O error: {err} (path={})", path.display()),
            );
            return Vec::new();
        }
    };
    let mut out = Vec::new();
    let reader = BufReader::new(content.as_bytes());
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(err) => {
                push_warning(
                    warnings,
                    format!("{label}: read error ({err}); skipping remainder"),
                );
                break;
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match parse(trimmed) {
            Ok(value) => out.push(value),
            Err(warning) => push_warning(warnings, warning),
        }
    }
    out
}

fn push_warning(warnings: &mut Vec<String>, message: String) {
    warnings.push(message);
}

/// Aggregate `recovery.jsonl` into [`RankedFinding`]s grouped by
/// `retry_key`. The latest entry in the group wins for severity /
/// outcome / message; occurrence count and first/last iteration are
/// collected across the whole group.
fn aggregate_recovery(entries: &[RecoveryJournalEntry]) -> Vec<RankedFinding> {
    if entries.is_empty() {
        return Vec::new();
    }
    // Preserve insertion order of first occurrences so the rank step
    // is deterministic regardless of HashMap's random seed.
    let mut order: Vec<String> = Vec::new();
    let mut groups: HashMap<String, GroupState> = HashMap::new();
    for entry in entries {
        let key = entry.envelope.retry_key.clone();
        if let Some(state) = groups.get_mut(&key) {
            state.observe(&entry.envelope);
        } else {
            order.push(key.clone());
            let mut state = GroupState::new(&entry.envelope);
            state.observe(&entry.envelope);
            groups.insert(key, state);
        }
    }
    let mut findings: Vec<RankedFinding> = order
        .into_iter()
        .filter_map(|key| groups.remove(&key).map(|state| state.finalize(key)))
        .collect();
    rank_findings(&mut findings);
    findings
}

struct GroupState {
    occurrences: u32,
    first_iteration: Option<u32>,
    last_iteration: Option<u32>,
    latest: RecoveryDiagnosisEnvelope,
    escalated: bool,
    failure_count: u32,
}

impl GroupState {
    fn new(env: &RecoveryDiagnosisEnvelope) -> Self {
        Self {
            occurrences: 0,
            first_iteration: env.iteration,
            last_iteration: env.iteration,
            latest: env.clone(),
            escalated: matches!(env.outcome, DiagnosisOutcome::Escalated),
            failure_count: u32::from(matches!(env.outcome, DiagnosisOutcome::Failed)),
        }
    }

    fn observe(&mut self, env: &RecoveryDiagnosisEnvelope) {
        self.occurrences = self.occurrences.saturating_add(1);
        if env.iteration < self.first_iteration {
            self.first_iteration = env.iteration;
        }
        if env.iteration > self.last_iteration {
            self.last_iteration = env.iteration;
        }
        if matches!(env.outcome, DiagnosisOutcome::Escalated) {
            self.escalated = true;
        }
        if matches!(env.outcome, DiagnosisOutcome::Failed) {
            self.failure_count = self.failure_count.saturating_add(1);
        }
        // Use the entry with the highest iteration as the "latest"
        // so severity/outcome track the most recent observation.
        if env.iteration >= self.latest.iteration {
            self.latest = env.clone();
        }
    }

    fn finalize(self, retry_key: String) -> RankedFinding {
        RankedFinding {
            retry_key,
            severity: self.latest.severity,
            outcome: self.latest.outcome,
            source: self.latest.source.as_str().to_string(),
            target_hat: self.latest.target_hat.clone(),
            topic: self.latest.topic.clone(),
            reason_code: self.latest.reason_code.clone(),
            message: self.latest.message.clone(),
            occurrences: self.occurrences,
            first_iteration: self.first_iteration,
            last_iteration: self.last_iteration,
            evidence: self.latest.evidence.clone(),
            safe_target: self.latest.safe_target,
            escalated: self.escalated,
        }
    }
}

/// Rank findings: severity (high first), then escalated, then
/// failure, then terminal-paused (`Failed` outcome), then
/// occurrences, then most recent iteration. Stable tiebreaker on
/// `retry_key` to keep tests deterministic.
fn rank_findings(findings: &mut [RankedFinding]) {
    findings.sort_by(|a, b| {
        // Higher severity ranks first: Critical > Error > Warning > Info.
        let sev = b.severity.cmp(&a.severity);
        if sev != std::cmp::Ordering::Equal {
            return sev;
        }
        // Escalated groups outrank non-escalated at the same severity.
        let esc = b.escalated.cmp(&a.escalated);
        if esc != std::cmp::Ordering::Equal {
            return esc;
        }
        // Terminal failure (Failed) outranks Recovered / Pending.
        let a_fail = u8::from(matches!(a.outcome, DiagnosisOutcome::Failed));
        let b_fail = u8::from(matches!(b.outcome, DiagnosisOutcome::Failed));
        let failure = b_fail.cmp(&a_fail);
        if failure != std::cmp::Ordering::Equal {
            return failure;
        }
        // More occurrences rank first.
        let occ = b.occurrences.cmp(&a.occurrences);
        if occ != std::cmp::Ordering::Equal {
            return occ;
        }
        // Most recent iteration first.
        let iter = b.last_iteration.cmp(&a.last_iteration);
        if iter != std::cmp::Ordering::Equal {
            return iter;
        }
        a.retry_key.cmp(&b.retry_key)
    });
}

fn recovery_timeline(entries: &[RecoveryJournalEntry]) -> Vec<TimelineRow> {
    entries
        .iter()
        .map(|e| TimelineRow {
            iteration: e.iteration,
            hat: e
                .envelope
                .source_hat
                .clone()
                .or_else(|| e.envelope.target_hat.clone())
                .unwrap_or_else(|| "(none)".to_string()),
            severity: e.envelope.severity,
            outcome: e.envelope.outcome,
            target_hat: e.envelope.target_hat.clone(),
            topic: e.envelope.topic.clone(),
            reason_code: e.envelope.reason_code.clone(),
            message: e.envelope.message.clone(),
        })
        .collect()
}

/// Render the report as a Markdown document. Stable structure
/// consumed by humans; CI should use [`render_json`].
#[must_use]
pub fn render_markdown(report: &Report) -> String {
    let mut out = String::new();
    out.push_str(&format!(
        "# Ralph Diagnose Report (schema v{})\n\n",
        report.schema_version
    ));
    out.push_str("## Run summary\n\n");
    out.push_str(&format!(
        "- session path: `{}`\n",
        report.session_path.display()
    ));
    if let Some(summary) = &report.summary {
        out.push_str(&format!("- session id: `{}`\n", summary.session_id));
        if let Some(started) = summary.loop_started_at {
            out.push_str(&format!(
                "- loop started: {}\n",
                format_datetime_utc(started)
            ));
        }
        if let Some(terminated) = summary.loop_terminated_at {
            out.push_str(&format!(
                "- loop terminated: {}\n",
                format_datetime_utc(terminated)
            ));
        }
        // U3: compute loop duration from timestamps (not pre-computed).
        if let Some(dur) = compute_duration(summary.loop_started_at, summary.loop_terminated_at) {
            out.push_str(&format!("- loop duration: {}\n", format_duration(dur)));
        }
        if let Some(iters) = summary.total_iterations {
            out.push_str(&format!("- total iterations: {iters}\n"));
        }
        if let Some(reason) = &summary.termination_reason {
            out.push_str(&format!("- termination reason: {reason}\n"));
        }
        out.push_str(&format!(
            "- recovery journal entries: {}\n",
            summary.recovery_count
        ));
        out.push_str(&format!(
            "- drift findings: {}\n",
            summary.drift_finding_count
        ));
        for note in &summary.notes {
            out.push_str(&format!("- note: {note}\n"));
        }
    } else {
        out.push_str("- summary seed: not present (run did not write diagnosis-summary.json)\n");
    }
    out.push('\n');

    push_top_findings_md(&mut out, &report.top_findings);
    push_recovery_timeline_md(&mut out, &report.recovery_timeline);
    push_drift_findings_md(&mut out, &report.drift_findings);
    push_preset_topology_md(&mut out, &report.orchestration);
    push_contract_health_md(&mut out, report);
    push_errors_md(&mut out, &report.errors);
    push_active_activations_md(&mut out, &report.active_activations);
    push_suggested_actions_md(&mut out, report);
    push_warnings_md(&mut out, &report.warnings);
    out
}

fn push_top_findings_md(out: &mut String, findings: &[RankedFinding]) {
    out.push_str("## Top findings\n\n");
    if findings.is_empty() {
        out.push_str("_无 recovery journal。_\n\n");
        return;
    }
    out.push_str("| severity | source | target | topic | occurrences | first→last iter | outcome | retry_key |\n");
    out.push_str("|---|---|---|---|---|---|---|---|\n");
    for f in findings {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {}→{} | {} | `{}` |\n",
            f.severity.as_str(),
            f.source,
            f.target_hat.as_deref().unwrap_or("*"),
            f.topic.as_deref().unwrap_or("*"),
            f.occurrences,
            f.first_iteration
                .map(|i| i.to_string())
                .unwrap_or_else(|| "pre".to_string()),
            f.last_iteration
                .map(|i| i.to_string())
                .unwrap_or_else(|| "pre".to_string()),
            f.outcome.as_str(),
            f.retry_key,
        ));
    }
    out.push('\n');
}

fn push_recovery_timeline_md(out: &mut String, rows: &[TimelineRow]) {
    out.push_str("## Recovery timeline\n\n");
    if rows.is_empty() {
        out.push_str("_无 recovery journal。_\n\n");
        return;
    }
    out.push_str("| iter | hat | severity | outcome | target | topic | reason | message |\n");
    out.push_str("|---|---|---|---|---|---|---|---|\n");
    for r in rows {
        let message = truncate_md(&r.message, 120);
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} | {} | {} | {} |\n",
            r.iteration
                .map(|i| i.to_string())
                .unwrap_or_else(|| "pre".to_string()),
            r.hat,
            r.severity.as_str(),
            r.outcome.as_str(),
            r.target_hat.as_deref().unwrap_or("*"),
            r.topic.as_deref().unwrap_or("*"),
            r.reason_code,
            message,
        ));
    }
    out.push('\n');
}

fn push_drift_findings_md(out: &mut String, findings: &[DriftSummary]) {
    out.push_str("## Drift findings\n\n");
    if findings.is_empty() {
        out.push_str("_无 drift findings。_\n\n");
        return;
    }
    out.push_str("| metric | topic | field | observed | threshold | severity | iter | message |\n");
    out.push_str("|---|---|---|---|---|---|---|---|\n");
    for d in findings {
        out.push_str(&format!(
            "| {} | {} | {} | {:.4} | {:.4} | {} | {} | {} |\n",
            d.metric,
            d.topic.as_deref().unwrap_or("*"),
            d.field.as_deref().unwrap_or("*"),
            d.observed_value,
            d.threshold,
            d.severity.as_str(),
            d.iteration,
            truncate_md(&d.message, 120),
        ));
    }
    out.push('\n');
}

fn push_preset_topology_md(out: &mut String, orch: &[OrchestrationEntry]) {
    out.push_str("## Preset topology health\n\n");
    if orch.is_empty() {
        out.push_str("_无 orchestration.jsonl（diagnostics 处于 minimal 模式或 orchestration logger 不可用）。_\n\n");
        return;
    }
    // Count hat activity and rejection events.
    let mut by_hat: BTreeMap<String, HatStats> = BTreeMap::new();
    for entry in orch {
        let stats = by_hat.entry(entry.hat.clone()).or_default();
        stats.events = stats.events.saturating_add(1);
        match &entry.event {
            OrchestrationEvent::HatSelected { .. } => {
                stats.selected = stats.selected.saturating_add(1);
            }
            OrchestrationEvent::EventPublished { .. } => {
                stats.published = stats.published.saturating_add(1);
            }
            OrchestrationEvent::BackpressureTriggered { .. } => {
                stats.backpressure = stats.backpressure.saturating_add(1);
            }
            OrchestrationEvent::ExecutionContractRejected { .. } => {
                stats.contract_rejections = stats.contract_rejections.saturating_add(1);
            }
            OrchestrationEvent::ContractRecoveryRouted { retry_target, .. } => {
                if retry_target.is_none() {
                    stats.contract_no_target = stats.contract_no_target.saturating_add(1);
                } else {
                    stats.contract_routed = stats.contract_routed.saturating_add(1);
                }
            }
            _ => {}
        }
    }
    out.push_str("| hat | selected | published | backpressure | contract rejections | routed | no target |\n");
    out.push_str("|---|---|---|---|---|---|---|\n");
    for (hat, stats) in &by_hat {
        out.push_str(&format!(
            "| {hat} | {} | {} | {} | {} | {} | {} |\n",
            stats.selected,
            stats.published,
            stats.backpressure,
            stats.contract_rejections,
            stats.contract_routed,
            stats.contract_no_target,
        ));
    }
    out.push('\n');
}

#[derive(Default)]
struct HatStats {
    events: u32,
    selected: u32,
    published: u32,
    backpressure: u32,
    contract_rejections: u32,
    contract_routed: u32,
    contract_no_target: u32,
}

fn push_contract_health_md(out: &mut String, report: &Report) {
    out.push_str("## Contract health\n\n");
    let rejected: Vec<&OrchestrationEntry> = report
        .orchestration
        .iter()
        .filter(|e| {
            matches!(
                e.event,
                OrchestrationEvent::ExecutionContractRejected { .. }
                    | OrchestrationEvent::ContractRecoveryRouted { .. }
            )
        })
        .collect();
    if rejected.is_empty() {
        out.push_str("_无 contract rejections / recovery routings。_\n\n");
        return;
    }
    out.push_str("| iter | hat | type | topic | detail |\n");
    out.push_str("|---|---|---|---|---|\n");
    for entry in rejected {
        match &entry.event {
            OrchestrationEvent::ExecutionContractRejected {
                topic,
                violation_kind,
                message,
            } => {
                out.push_str(&format!(
                    "| {} | {} | execution_contract_rejected | {} | {} (kind={}) |\n",
                    entry.iteration,
                    entry.hat,
                    topic,
                    truncate_md(message, 80),
                    violation_kind,
                ));
            }
            OrchestrationEvent::ContractRecoveryRouted {
                topic,
                retry_target,
                no_retry_reason,
            } => {
                let detail = match (retry_target, no_retry_reason) {
                    (Some(target), _) => format!("routed to {target}"),
                    (None, Some(reason)) => format!("no target: {reason}"),
                    (None, None) => "no target".to_string(),
                };
                out.push_str(&format!(
                    "| {} | {} | contract_recovery_routed | {} | {} |\n",
                    entry.iteration, entry.hat, topic, detail,
                ));
            }
            _ => {}
        }
    }
    out.push('\n');
}

fn push_errors_md(out: &mut String, errors: &[Value]) {
    out.push_str("## Errors\n\n");
    if errors.is_empty() {
        out.push_str("_无 errors.jsonl entries。_\n\n");
        return;
    }
    out.push_str("| iter | hat | error_type | message |\n");
    out.push_str("|---|---|---|---|\n");
    for entry in errors {
        let iter = entry
            .get("iteration")
            .and_then(Value::as_u64)
            .map(|n| n.to_string())
            .unwrap_or_else(|| "-".to_string());
        let hat = entry
            .get("hat")
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string();
        let kind = entry
            .get("error_type")
            .and_then(Value::as_str)
            .unwrap_or("-")
            .to_string();
        let msg = entry
            .get("message")
            .and_then(Value::as_str)
            .map(|m| truncate_md(m, 120))
            .unwrap_or_default();
        out.push_str(&format!("| {iter} | {hat} | {kind} | {msg} |\n"));
    }
    out.push('\n');
}

fn push_suggested_actions_md(out: &mut String, report: &Report) {
    out.push_str("## Suggested next actions\n\n");
    if report.top_findings.is_empty()
        && report.drift_findings.is_empty()
        && report.errors.is_empty()
        && report.orchestration.is_empty()
    {
        out.push_str("_无可用诊断数据；无需操作。_\n\n");
        return;
    }
    let mut printed = 0;
    for f in &report.top_findings {
        if printed >= 5 {
            break;
        }
        for action in suggested_actions_for_finding(f) {
            if printed >= 5 {
                break;
            }
            out.push_str(&format!("- {action}\n"));
            printed += 1;
        }
    }
    if printed == 0 {
        out.push_str("- 收集更多次 loop 后再次运行 `ralph diagnose`；当前 session 没有 critical/error 级 finding。\n");
    }
    out.push('\n');
}

fn suggested_actions_for_finding(f: &RankedFinding) -> Vec<String> {
    let mut out = Vec::new();
    match f.source.as_str() {
        "payload_contract" => {
            out.push(format!(
                "修改 preset 的 `event_policy.schemas.<topic>.required_fields`，给 topic `{}` 补齐缺失字段",
                f.topic.as_deref().unwrap_or("?")
            ));
        }
        "execution_contract" => {
            out.push(format!(
                "更新 hat `{}` 的 instructions，确保 emit `{}` 时附上 `{}` 字段",
                f.target_hat.as_deref().unwrap_or("?"),
                f.topic.as_deref().unwrap_or("?"),
                f.reason_code,
            ));
        }
        "missing_event_gate" => {
            out.push(format!(
                "在 hat `{}` 的 publishing contract 里强制补上 `{}` 事件",
                f.target_hat.as_deref().unwrap_or("?"),
                f.topic.as_deref().unwrap_or("?"),
            ));
        }
        "workflow_guard" => {
            out.push(format!(
                "检查 preset 的 workflow phase 顺序，确认 hat `{}` 在正确的 phase emit `{}`",
                f.target_hat.as_deref().unwrap_or("?"),
                f.topic.as_deref().unwrap_or("?"),
            ));
        }
        "drift_monitor" => {
            out.push(
                "调整 `telemetry.runtime_diagnosis.drift` 阈值，或修复 hat instructions 中缺失字段"
                    .to_string(),
            );
        }
        "stall_recovery" => {
            out.push(
                "检查 hat 是否被 OOM / 网络中断打断；考虑调低 max_iterations 触发更早的 steering"
                    .to_string(),
            );
        }
        "hook_retry" => {
            out.push("检查 `pre_agent` / `post_agent` hook 是否有超时或非零退出码".to_string());
        }
        "loop_stale" => {
            out.push(
                "loop 进入 stale 状态；运行 `ralph loops` 确认是否有并行 loop 在 hold state"
                    .to_string(),
            );
        }
        "flow_lifecycle" => match f.reason_code.as_str() {
            "wave_spawn_failed" | "phase_transition" => {
                out.push(format!(
                    "Inspect the flow registry state for {}; phase {} indicates a transition that may need a forced close.",
                    f.topic.as_deref().unwrap_or("?"),
                    f.reason_code,
                ));
            }
            "wave_timeout_drift" => {
                out.push(format!(
                    "Wave {} actual wait diverged from configured aggregate budget; tune `aggregate.timeout` or check per-worker timeout cascade.",
                    f.topic.as_deref().unwrap_or("?"),
                ));
            }
            "wave_timeout_early" => {
                out.push(format!(
                    "Wave {} fired its aggregate deadline early; this is a dispatcher timing bug, not a worker issue.",
                    f.topic.as_deref().unwrap_or("?"),
                ));
            }
            _ => {}
        },
        _ => {
            out.push(format!(
                "复现 retry_key `{}` 并查 hat `{}` 的 instructions",
                f.retry_key,
                f.target_hat.as_deref().unwrap_or("?"),
            ));
        }
    }
    if f.escalated {
        out.push(format!(
            "retry_key `{}` 已 escalation {} 次（{}→{}），建议人工介入或调高 `telemetry.runtime_diagnosis.max_repeated_recoveries`",
            f.retry_key,
            f.occurrences,
            f.first_iteration.map(|i| i.to_string()).unwrap_or_else(|| "pre".to_string()),
            f.last_iteration.map(|i| i.to_string()).unwrap_or_else(|| "pre".to_string())
        ));
    }
    if !f.safe_target && matches!(f.outcome, DiagnosisOutcome::Failed) {
        out.push(format!(
            "retry_key `{}` 没有 safe target，确认 preset 里是否注册了对应 hat",
            f.retry_key
        ));
    }
    out
}

fn push_warnings_md(out: &mut String, warnings: &[String]) {
    if warnings.is_empty() {
        return;
    }
    out.push_str("## Warnings\n\n");
    for w in warnings {
        out.push_str(&format!("- {w}\n"));
    }
    out.push('\n');
}

/// Format a `Duration` as a human-readable string.
///
/// Examples: `"30s"`, `"5m 12s"`, `"1h 23m 45s"`, `"0s"`.
fn format_duration(d: std::time::Duration) -> String {
    let total_secs = d.as_secs();
    let hours = total_secs / 3600;
    let mins = (total_secs % 3600) / 60;
    let secs = total_secs % 60;
    match (hours, mins, secs) {
        (0, 0, 0) => "0s".to_string(),
        (0, 0, s) => format!("{s}s"),
        (0, m, 0) => format!("{m}m"),
        (0, m, s) => format!("{m}m {s}s"),
        (h, 0, 0) => format!("{h}h"),
        (h, 0, s) => format!("{h}h {s}s"),
        (h, m, 0) => format!("{h}h {m}m"),
        (h, m, s) => format!("{h}h {m}m {s}s"),
    }
}

/// Format a `SystemTime` as a dual-timezone string: UTC + local.
///
/// Example: `"2026-06-05 10:20:30 UTC / 18:20:30 CST"`.
///
/// Conversion `SystemTime → DateTime<Utc>` is infallible, so there is no
/// fallback path: the local half is always rendered.
fn format_system_time(t: std::time::SystemTime) -> String {
    let dt_utc: chrono::DateTime<chrono::Utc> = t.into();
    format_datetime_utc(dt_utc)
}

/// Format a `DateTime<Utc>` as a dual-timezone string: UTC + local.
///
/// Example: `"2026-06-05 10:20:30 UTC / 18:20:30 CST (Asia/Shanghai)"`.
///
/// The local half always includes the IANA zone name in parentheses so
/// short timezone abbreviations (CST, PST, EST — each maps to multiple
/// regions) are unambiguous to humans and to machine consumers that
/// pair on the bracketed suffix.
fn format_datetime_utc(dt: chrono::DateTime<chrono::Utc>) -> String {
    let dt_local: chrono::DateTime<chrono::Local> = dt.into();
    format!(
        "{} / {} ({})",
        dt.format("%Y-%m-%d %H:%M:%S UTC"),
        dt_local.format("%H:%M:%S %Z"),
        iana_tz_name(),
    )
}

/// Best-effort IANA timezone name for the current process. Falls back
/// to "UTC" when the local zone cannot be resolved (e.g. very old
/// glibc with no `/etc/localtime` symlink target).
fn iana_tz_name() -> &'static str {
    use std::sync::OnceLock;
    static CACHE: OnceLock<String> = OnceLock::new();
    CACHE
        .get_or_init(|| {
            // chrono::Local returns a DateTime bound to the system's
            // IANA zone. Asking for `%Z` gives the abbreviation; the
            // `tzname()` lookup on the chrono side gives the IANA
            // name when available.
            let _local: chrono::DateTime<chrono::Local> =
                chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0)
                    .unwrap_or_else(chrono::Utc::now)
                    .into();
            // chrono 0.4 does not expose a public IANA resolver on
            // DateTime<Local>; rely on `Local::now()`'s zone display
            // via `%Z` and `chrono::LocalResult` introspection. As a
            // portable fallback we use the TZ env var if set, else
            // `/etc/localtime` symlink target via std (best-effort).
            if let Ok(tz) = std::env::var("TZ") {
                if !tz.is_empty() {
                    return tz;
                }
            }
            std::fs::read_link("/etc/localtime")
                .ok()
                .and_then(|p| {
                    p.to_str()
                        .map(|s| s.split("zoneinfo/").last().unwrap_or(s).to_string())
                })
                .unwrap_or_else(|| "UTC".to_string())
        })
        .as_str()
}

/// Compute the duration between two `DateTime<Utc>` values.
///
/// Returns `None` when `start > end` (e.g. clock skew) or when
/// either timestamp is absent.
fn compute_duration(
    start: Option<chrono::DateTime<chrono::Utc>>,
    end: Option<chrono::DateTime<chrono::Utc>>,
) -> Option<std::time::Duration> {
    let s = start?;
    let e = end?;
    e.signed_duration_since(s).to_std().ok()
}

/// Render the `## Active Hat Activations` section (U4).
fn push_active_activations_md(out: &mut String, activations: &[ActivationSnapshot]) {
    out.push_str("## Active Hat Activations\n\n");
    if activations.is_empty() {
        out.push_str("_No active hat activations._\n\n");
        return;
    }
    out.push_str("| Hat | Activated at | Last event at | Duration | Task |\n");
    out.push_str("|---|---|---|---|---|\n");
    for a in activations {
        out.push_str(&format!(
            "| {} | {} | {} | {} | {} |\n",
            a.hat_id,
            format_system_time(a.activated_at),
            format_system_time(a.last_event_at),
            format_duration(a.duration),
            a.linked_task_id.as_ref().map(|t| t.as_str()).unwrap_or("-"),
        ));
    }
    out.push_str(&format!(
        "\n_{} active activation{}, sorted by duration descending._\n\n",
        activations.len(),
        if activations.len() == 1 { "" } else { "s" },
    ));
}

/// Render the report as a stable JSON document. The structure is
/// versioned via [`DIAGNOSE_JSON_SCHEMA_VERSION`] and intended for
/// CI consumption.
#[must_use]
pub fn render_json(report: &Report) -> Value {
    let findings: Vec<Value> = report
        .top_findings
        .iter()
        .map(|f| {
            json!({
                "retry_key": f.retry_key,
                "severity": f.severity.as_str(),
                "source": f.source,
                "target_hat": f.target_hat,
                "topic": f.topic,
                "reason_code": f.reason_code,
                "message": f.message,
                "occurrences": f.occurrences,
                "first_iteration": f.first_iteration,
                "last_iteration": f.last_iteration,
                "outcome": f.outcome.as_str(),
                "safe_target": f.safe_target,
                "escalated": f.escalated,
                "evidence": f.evidence.iter().map(|e| {
                    json!({
                        "kind": e.kind.as_str(),
                        "ref_path": e.ref_path,
                        "snippet": e.snippet,
                    })
                }).collect::<Vec<_>>(),
            })
        })
        .collect();

    let timeline: Vec<Value> = report
        .recovery_timeline
        .iter()
        .map(|r| {
            json!({
                "iteration": r.iteration,
                "hat": r.hat,
                "severity": r.severity.as_str(),
                "outcome": r.outcome.as_str(),
                "target_hat": r.target_hat,
                "topic": r.topic,
                "reason_code": r.reason_code,
                "message": r.message,
            })
        })
        .collect();

    let drift: Vec<Value> = report
        .drift_findings
        .iter()
        .map(|d| {
            json!({
                "metric": d.metric,
                "topic": d.topic,
                "field": d.field,
                "from_topic": d.from_topic,
                "to_topic": d.to_topic,
                "observed_value": d.observed_value,
                "threshold": d.threshold,
                "severity": d.severity.as_str(),
                "iteration": d.iteration,
                "message": d.message,
            })
        })
        .collect();

    let orch: Vec<Value> = report
        .orchestration
        .iter()
        .map(|e| {
            json!({
                "timestamp": e.timestamp,
                "iteration": e.iteration,
                "hat": e.hat,
                "event": e.event,
            })
        })
        .collect();

    let summary = report.summary.as_ref().map(|s| {
        let loop_duration_secs =
            compute_duration(s.loop_started_at, s.loop_terminated_at).map(|d| d.as_secs());
        json!({
            "schema_version": s.schema_version,
            "session_id": s.session_id,
            "generated_at": s.generated_at,
            "loop_started_at": s.loop_started_at,
            "loop_terminated_at": s.loop_terminated_at,
            "loop_duration_secs": loop_duration_secs,
            "total_iterations": s.total_iterations,
            "termination_reason": s.termination_reason,
            "recovery_journal_path": s.recovery_journal_path,
            "drift_journal_path": s.drift_journal_path,
            "orchestration_log_path": s.orchestration_log_path,
            "errors_log_path": s.errors_log_path,
            "recovery_count": s.recovery_count,
            "drift_finding_count": s.drift_finding_count,
            "notes": s.notes,
        })
    });

    json!({
        "schema_version": report.schema_version,
        "session_path": report.session_path,
        "summary": summary,
        "top_findings": findings,
        "recovery_timeline": timeline,
        "drift_findings": drift,
        "orchestration": orch,
        "errors": report.errors,
        "warnings": report.warnings,
        "active_activations": report.active_activations.iter().map(|a| {
            // U3: duration computed from timestamps (same path as
            // markdown summary / json summary), not from the
            // pre-computed `a.duration` field which is captured at sweep
            // time and can drift from the visible timestamps.
            let activated_utc: chrono::DateTime<chrono::Utc> = a.activated_at.into();
            let last_event_utc: chrono::DateTime<chrono::Utc> = a.last_event_at.into();
            let computed_duration = compute_duration(Some(activated_utc), Some(last_event_utc))
                .map(|d| d.as_secs())
                .unwrap_or(0);
            json!({
                "hat_id": a.hat_id,
                "trigger_topic": a.trigger_topic,
                "trigger_identity": a.trigger_identity,
                "activated_at": activated_utc.to_rfc3339(),
                "last_event_at": last_event_utc.to_rfc3339(),
                "activated_at_display": format_system_time(a.activated_at),
                "last_event_at_display": format_system_time(a.last_event_at),
                "duration_secs": computed_duration,
                "linked_task_id": a.linked_task_id,
            })
        }).collect::<Vec<_>>(),
    })
}

fn truncate_md(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let keep = max.saturating_sub(1);
    let mut out: String = s.chars().take(keep).collect();
    out.push('\u{2026}');
    out
}

/// Convenience helper: resolve + load + build a [`Report`] in one
/// call. Returns `Err(ReporterError::NoSession)` when no session
/// can be located.
pub fn build_report(
    selector: SessionSelector<'_>,
    diagnostics_root: &Path,
) -> Result<Report, ReporterError> {
    let session_path = resolve_session(selector, diagnostics_root)?;
    let data = load_session(&session_path);
    Ok(Report::from_session(&data))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diagnosis::envelope::DiagnosisSource;
    use crate::diagnosis::envelope::EvidenceKind;
    use crate::diagnosis::envelope::EvidenceRef;
    use crate::diagnosis::journal::DriftMetric;
    use tempfile::TempDir;

    fn env(
        retry_key: &str,
        iteration: u32,
        severity: DiagnosisSeverity,
    ) -> RecoveryDiagnosisEnvelope {
        RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::MissingEventGate)
            .severity(severity)
            .iteration(iteration)
            .reason_code("no_emit")
            .message(format!("builder did not emit work.done @ {retry_key}"))
            .source_hat("builder")
            .target_hat("builder")
            .topic("work.done")
            .retry_key(retry_key)
            .safe_target(true)
            .build()
    }

    #[test]
    fn resolve_latest_ignores_logs_and_payload_contract_files() {
        let tmp = TempDir::new().unwrap();
        let diag = tmp.path().join(".ralph/diagnostics");
        fs::create_dir_all(&diag).unwrap();
        // TUI log dir — must be ignored.
        fs::create_dir_all(diag.join("logs")).unwrap();
        // Root-level violation report — must be ignored.
        fs::write(diag.join("payload-contract-error-2026-06-05.json"), "{}").unwrap();
        // Old and new sessions.
        fs::create_dir_all(diag.join("2026-06-05T10-20-30")).unwrap();
        fs::create_dir_all(diag.join("2026-06-06T09-00-00")).unwrap();

        let resolved = resolve_session(SessionSelector::Latest, &diag).unwrap();
        assert_eq!(resolved, diag.join("2026-06-06T09-00-00"));
    }

    #[test]
    fn resolve_latest_errors_when_no_sessions() {
        let tmp = TempDir::new().unwrap();
        let diag = tmp.path().join(".ralph/diagnostics");
        fs::create_dir_all(&diag).unwrap();
        fs::create_dir_all(diag.join("logs")).unwrap();
        let err = resolve_session(SessionSelector::Latest, &diag).unwrap_err();
        assert!(matches!(err, ReporterError::NoSession(_)));
    }

    #[test]
    fn resolve_latest_errors_when_root_missing() {
        let tmp = TempDir::new().unwrap();
        let diag = tmp.path().join("missing");
        let err = resolve_session(SessionSelector::Latest, &diag).unwrap_err();
        assert!(matches!(err, ReporterError::NoSession(_)));
    }

    #[test]
    fn resolve_explicit_accepts_absolute_and_relative() {
        let tmp = TempDir::new().unwrap();
        let abs = tmp.path().join("2026-06-05T10-20-30");
        fs::create_dir_all(&abs).unwrap();
        let resolved =
            resolve_session(SessionSelector::Explicit(abs.to_str().unwrap()), &abs).unwrap();
        assert_eq!(resolved, abs);
    }

    #[test]
    fn resolve_explicit_resolves_timestamp_against_root() {
        let tmp = TempDir::new().unwrap();
        let diag = tmp.path().join(".ralph/diagnostics");
        let session = diag.join("2026-06-05T10-20-30");
        fs::create_dir_all(&session).unwrap();
        let resolved =
            resolve_session(SessionSelector::Explicit("2026-06-05T10-20-30"), &diag).unwrap();
        assert_eq!(resolved, session);
    }

    #[test]
    fn resolve_explicit_rejects_invalid_path() {
        let tmp = TempDir::new().unwrap();
        let diag = tmp.path().join(".ralph/diagnostics");
        fs::create_dir_all(&diag).unwrap();
        let err = resolve_session(SessionSelector::Explicit("not-a-timestamp"), &diag).unwrap_err();
        assert!(matches!(err, ReporterError::InvalidSession(_)));
    }

    #[test]
    fn load_session_collects_warnings_for_missing_files() {
        let tmp = TempDir::new().unwrap();
        let data = load_session(tmp.path());
        assert!(data.summary.is_none());
        assert!(data.recovery.is_empty());
        assert!(data.drift.is_empty());
        assert!(data.orchestration.is_empty());
        assert!(data.errors.is_empty());
        // 4 missing-file warnings.
        assert!(
            data.warnings.iter().any(|w| w.contains("recovery.jsonl")),
            "warnings: {:?}",
            data.warnings
        );
        assert!(
            data.warnings.iter().any(|w| w.contains("drift.jsonl")),
            "warnings: {:?}",
            data.warnings
        );
        assert!(
            data.warnings
                .iter()
                .any(|w| w.contains("orchestration.jsonl")),
            "warnings: {:?}",
            data.warnings
        );
        assert!(
            data.warnings.iter().any(|w| w.contains("errors.jsonl")),
            "warnings: {:?}",
            data.warnings
        );
    }

    #[test]
    fn load_session_collects_warnings_for_malformed_jsonl() {
        let tmp = TempDir::new().unwrap();
        fs::write(tmp.path().join("recovery.jsonl"), b"not json\n{\"k\":1}\n").unwrap();
        let data = load_session(tmp.path());
        // Malformed line produces a warning; the second line is
        // unrelated but should also be a warning because it does
        // not deserialize into RecoveryJournalEntry.
        assert!(
            data.warnings
                .iter()
                .any(|w| w.contains("malformed recovery.jsonl")),
            "warnings: {:?}",
            data.warnings
        );
    }

    #[test]
    fn load_session_parses_recovery_and_drift_journal() {
        let tmp = TempDir::new().unwrap();
        let entry =
            RecoveryJournalEntry::from_envelope(env("rk:1", 1, DiagnosisSeverity::Warning), vec![]);
        fs::write(
            tmp.path().join("recovery.jsonl"),
            serde_json::to_string(&entry).unwrap(),
        )
        .unwrap();
        let drift = DriftJournalEntry::builder()
            .metric(DriftMetric::FieldCompleteness)
            .observed_value(0.5)
            .threshold(0.9)
            .severity(DiagnosisSeverity::Warning)
            .topic("work.done")
            .field("plan_name")
            .iteration(2)
            .message("plan_name missing 50% of the time")
            .build();
        fs::write(
            tmp.path().join("drift.jsonl"),
            serde_json::to_string(&drift).unwrap(),
        )
        .unwrap();
        let data = load_session(tmp.path());
        assert_eq!(data.recovery.len(), 1);
        assert_eq!(data.drift.len(), 1);
        // orchestration.jsonl and errors.jsonl are absent in this
        // test; the loader must still record the missing-file
        // warnings but must not fail.
        assert!(
            data.warnings
                .iter()
                .any(|w| w.contains("orchestration.jsonl")),
            "warnings: {:?}",
            data.warnings
        );
        assert!(
            data.warnings.iter().any(|w| w.contains("errors.jsonl")),
            "warnings: {:?}",
            data.warnings
        );
        assert!(!data.recovery.is_empty());
        assert!(!data.drift.is_empty());
    }

    #[test]
    fn aggregate_groups_by_retry_key_and_keeps_latest() {
        let entries = vec![
            RecoveryJournalEntry::from_envelope(env("a:1", 1, DiagnosisSeverity::Warning), vec![]),
            RecoveryJournalEntry::from_envelope(env("a:1", 2, DiagnosisSeverity::Error), vec![]),
            RecoveryJournalEntry::from_envelope(env("b:2", 1, DiagnosisSeverity::Critical), vec![]),
        ];
        let findings = aggregate_recovery(&entries);
        assert_eq!(findings.len(), 2);
        let a = findings.iter().find(|f| f.retry_key == "a:1").unwrap();
        assert_eq!(a.occurrences, 2);
        assert_eq!(a.first_iteration, Some(1));
        assert_eq!(a.last_iteration, Some(2));
        assert_eq!(a.severity, DiagnosisSeverity::Error);
    }

    #[test]
    fn rank_orders_severity_first() {
        let mut findings = vec![
            RankedFinding {
                retry_key: "z:1".into(),
                severity: DiagnosisSeverity::Info,
                outcome: DiagnosisOutcome::Pending,
                source: "stall_recovery".into(),
                target_hat: None,
                topic: None,
                reason_code: "r".into(),
                message: "m".into(),
                occurrences: 5,
                first_iteration: Some(1),
                last_iteration: Some(10),
                evidence: vec![],
                safe_target: false,
                escalated: false,
            },
            RankedFinding {
                retry_key: "a:1".into(),
                severity: DiagnosisSeverity::Critical,
                outcome: DiagnosisOutcome::Failed,
                source: "execution_contract".into(),
                target_hat: Some("builder".into()),
                topic: Some("work.done".into()),
                reason_code: "missing_field".into(),
                message: "m".into(),
                occurrences: 3,
                first_iteration: Some(1),
                last_iteration: Some(9),
                evidence: vec![],
                safe_target: true,
                escalated: false,
            },
        ];
        rank_findings(&mut findings);
        assert_eq!(findings[0].retry_key, "a:1");
        assert_eq!(findings[1].retry_key, "z:1");
    }

    #[test]
    fn rank_prefers_escalated_over_non_at_same_severity() {
        let mut findings = vec![
            RankedFinding {
                retry_key: "no".into(),
                severity: DiagnosisSeverity::Error,
                outcome: DiagnosisOutcome::Pending,
                source: "x".into(),
                target_hat: None,
                topic: None,
                reason_code: "r".into(),
                message: "m".into(),
                occurrences: 1,
                first_iteration: Some(1),
                last_iteration: Some(1),
                evidence: vec![],
                safe_target: true,
                escalated: false,
            },
            RankedFinding {
                retry_key: "esc".into(),
                severity: DiagnosisSeverity::Error,
                outcome: DiagnosisOutcome::Escalated,
                source: "x".into(),
                target_hat: None,
                topic: None,
                reason_code: "r".into(),
                message: "m".into(),
                occurrences: 1,
                first_iteration: Some(1),
                last_iteration: Some(1),
                evidence: vec![],
                safe_target: true,
                escalated: true,
            },
        ];
        rank_findings(&mut findings);
        assert_eq!(findings[0].retry_key, "esc");
    }

    #[test]
    fn render_markdown_contains_all_sections() {
        let entry =
            RecoveryJournalEntry::from_envelope(env("rk:1", 1, DiagnosisSeverity::Error), vec![]);
        let data = SessionData {
            session_path: PathBuf::from("/tmp/session"),
            recovery: vec![entry],
            warnings: vec!["warning-a".into()],
            ..SessionData::default()
        };
        let report = Report::from_session(&data);
        let md = render_markdown(&report);
        for header in [
            "# Ralph Diagnose Report",
            "## Run summary",
            "## Top findings",
            "## Recovery timeline",
            "## Drift findings",
            "## Preset topology health",
            "## Contract health",
            "## Active Hat Activations",
            "## Suggested next actions",
            "## Warnings",
        ] {
            assert!(md.contains(header), "missing header: {header}\n{md}");
        }
        assert!(md.contains("warning-a"));
        // Schema version appears in the heading line.
        assert!(md.contains("schema v1"));
    }

    #[test]
    fn render_markdown_missing_recovery_says_no_journal() {
        let data = SessionData::default();
        let report = Report::from_session(&data);
        let md = render_markdown(&report);
        assert!(md.contains("无 recovery journal"));
    }

    #[test]
    fn render_json_has_schema_version_and_finds_finding() {
        let entry = RecoveryJournalEntry::from_envelope(
            env("rk:1", 1, DiagnosisSeverity::Critical),
            vec![],
        );
        let mut data = SessionData::default();
        data.recovery = vec![entry];
        let report = Report::from_session(&data);
        let value = render_json(&report);
        assert_eq!(
            value["schema_version"],
            Value::String(DIAGNOSE_JSON_SCHEMA_VERSION.to_string())
        );
        let findings = value["top_findings"].as_array().unwrap();
        assert_eq!(findings.len(), 1);
        assert_eq!(findings[0]["retry_key"], "rk:1");
        assert_eq!(findings[0]["severity"], "critical");
        assert_eq!(findings[0]["occurrences"], 1);
    }

    #[test]
    fn render_json_does_not_contain_markdown_headings() {
        let entry =
            RecoveryJournalEntry::from_envelope(env("rk:1", 1, DiagnosisSeverity::Warning), vec![]);
        let data = SessionData {
            session_path: PathBuf::from("/tmp/session"),
            recovery: vec![entry],
            ..SessionData::default()
        };
        let report = Report::from_session(&data);
        let value = render_json(&report);
        let serialized = serde_json::to_string(&value).unwrap();
        assert!(!serialized.contains("## "));
        assert!(!serialized.contains("## Top findings"));
    }

    #[test]
    fn build_report_no_session_returns_err() {
        let tmp = TempDir::new().unwrap();
        let diag = tmp.path().join(".ralph/diagnostics");
        fs::create_dir_all(&diag).unwrap();
        let err = build_report(SessionSelector::Latest, &diag).unwrap_err();
        assert!(matches!(err, ReporterError::NoSession(_)));
    }

    #[test]
    fn evidence_ref_is_preserved_through_aggregation() {
        let env = RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::ExecutionContract)
            .severity(DiagnosisSeverity::Error)
            .iteration(1)
            .reason_code("missing_field")
            .message("plan_name required")
            .evidence(EvidenceRef::new(
                EvidenceKind::Field,
                "plan_name",
                Some("not present".to_string()),
            ))
            .retry_key("a:1")
            .safe_target(true)
            .build();
        let entries = vec![RecoveryJournalEntry::from_envelope(env.clone(), vec![])];
        let findings = aggregate_recovery(&entries);
        assert_eq!(findings[0].evidence.len(), 1);
        assert_eq!(findings[0].evidence[0].kind, EvidenceKind::Field);
        assert_eq!(findings[0].evidence[0].ref_path, "plan_name");
    }

    // ── U4: Active Hat Activations tests ─────────────────────────────────

    use crate::hat_lifecycle::{ActivationKey, ActivationSnapshot};
    use std::time::Duration;

    fn make_snapshot(
        hat_id: &str,
        duration_secs: u64,
        task_id: Option<&str>,
    ) -> ActivationSnapshot {
        let now = std::time::SystemTime::now();
        ActivationSnapshot {
            hat_id: hat_id.to_string(),
            trigger_topic: "work.start".to_string(),
            trigger_identity: format!("{hat_id}:trigger"),
            activated_at: now - Duration::from_secs(duration_secs),
            last_event_at: now - Duration::from_secs(duration_secs / 2),
            duration: Duration::from_secs(duration_secs),
            linked_task_id: task_id.map(crate::hat_lifecycle::TaskId::from),
            key: ActivationKey {
                loop_id: "loop-1".to_string(),
                iteration: 1,
                hat_id: hat_id.to_string(),
            },
        }
    }

    #[test]
    fn u4_empty_activations_renders_placeholder() {
        let data = SessionData::default();
        let report = Report::from_session(&data);
        let md = render_markdown(&report);
        assert!(
            md.contains("_No active hat activations._"),
            "expected placeholder in:\n{md}"
        );
    }

    #[test]
    fn u4_active_activations_renders_table() {
        let mut data = SessionData::default();
        data.active_activations = vec![make_snapshot("executor", 120, Some("task-abc"))];
        let report = Report::from_session(&data);
        let md = render_markdown(&report);
        assert!(md.contains("## Active Hat Activations"));
        assert!(md.contains("| executor |"));
        assert!(md.contains("task-abc"));
        assert!(md.contains("2m"));
        assert!(md.contains("1 active activation, sorted by duration descending."));
    }

    #[test]
    fn u4_multiple_activations_sorted_by_duration() {
        let mut data = SessionData::default();
        data.active_activations = vec![
            make_snapshot("fast", 10, None),
            make_snapshot("slow", 3661, None), // 1h 1m 1s
        ];
        let report = Report::from_session(&data);
        let md = render_markdown(&report);
        // slow (longer duration) should appear before fast.
        let slow_pos = md.find("| slow |").unwrap();
        let fast_pos = md.find("| fast |").unwrap();
        assert!(
            slow_pos < fast_pos,
            "slow should appear before fast for duration-descending sort"
        );
        assert!(md.contains("1h 1m 1s"));
        assert!(md.contains("2 active activations"));
    }

    #[test]
    fn u4_completed_activation_not_in_section() {
        let mut data = SessionData::default();
        // Only completed activations (empty active list) → placeholder.
        data.active_activations = vec![];
        let report = Report::from_session(&data);
        let md = render_markdown(&report);
        assert!(md.contains("_No active hat activations._"));
    }

    #[test]
    fn u4_json_includes_active_activations() {
        let mut data = SessionData::default();
        data.active_activations = vec![make_snapshot("reviewer", 60, Some("task-xyz"))];
        let report = Report::from_session(&data);
        let value = render_json(&report);
        let activations = value["active_activations"].as_array().unwrap();
        assert_eq!(activations.len(), 1);
        assert_eq!(activations[0]["hat_id"], "reviewer");
        assert_eq!(activations[0]["linked_task_id"], "task-xyz");
        // U3: duration_secs is now derived from the
        // (activated_at, last_event_at) timestamps via
        // compute_duration, matching the markdown summary path.
        // `make_snapshot(_, 60, _)` places activated_at = now-60s and
        // last_event_at = now-30s, so the visible duration is 30s,
        // not the 60s stored in the pre-computed `a.duration` field.
        assert_eq!(activations[0]["duration_secs"], 30);
    }

    #[test]
    fn u4_json_empty_activations_is_empty_array() {
        let data = SessionData::default();
        let report = Report::from_session(&data);
        let value = render_json(&report);
        let activations = value["active_activations"].as_array().unwrap();
        assert!(activations.is_empty());
    }

    #[test]
    fn u4_format_duration_variants() {
        assert_eq!(format_duration(Duration::from_secs(0)), "0s");
        assert_eq!(format_duration(Duration::from_secs(30)), "30s");
        assert_eq!(format_duration(Duration::from_secs(300)), "5m");
        assert_eq!(format_duration(Duration::from_secs(312)), "5m 12s");
        assert_eq!(format_duration(Duration::from_secs(3600)), "1h");
        assert_eq!(format_duration(Duration::from_secs(3660)), "1h 1m");
        assert_eq!(format_duration(Duration::from_secs(5025)), "1h 23m 45s");
    }

    #[test]
    fn u4_load_session_reads_activations_json() {
        let tmp = TempDir::new().unwrap();
        let activations = vec![make_snapshot("executor", 120, Some("task-1"))];
        let json = serde_json::to_string_pretty(&activations).unwrap();
        fs::write(tmp.path().join("active-activations.json"), json.as_bytes()).unwrap();
        let data = load_session(tmp.path());
        assert_eq!(data.active_activations.len(), 1);
        assert_eq!(data.active_activations[0].hat_id, "executor");
    }

    #[test]
    fn u4_load_session_missing_activations_file_is_not_warning() {
        let tmp = TempDir::new().unwrap();
        let data = load_session(tmp.path());
        assert!(data.active_activations.is_empty());
        // Missing file should NOT produce a warning.
        assert!(
            !data
                .warnings
                .iter()
                .any(|w| w.contains("active-activations")),
            "unexpected warning: {:?}",
            data.warnings
        );
    }

    #[test]
    fn looks_like_session_timestamp_edge_cases() {
        // Exactly 19 chars: valid ISO timestamp (YYYY-MM-DDTHH-MM-SS)
        assert!(looks_like_session_timestamp("2025-01-15T10-30-00"));
        // Less than 19 chars: too short
        assert!(!looks_like_session_timestamp("2025-01-15T10-30"));
        // More than 19 chars with suffix
        assert!(looks_like_session_timestamp(
            "2025-01-15T10-30-00.something"
        ));
        // Non-timestamp strings
        assert!(!looks_like_session_timestamp("ralph-diagnostic"));
        assert!(!looks_like_session_timestamp("events"));
    }

    // Regression coverage for the shallow-format heuristic in
    // `looks_like_session_timestamp`. The function intentionally checks
    // only the first 11 byte positions (YYYY-MM-DDTH) plus the minimum
    // 19-char length, so most of these tests pin the *current* behavior
    // — including the documented gaps that code review finding #12
    // (timezone suffixes, leap-second seconds component, multi-byte
    // unicode headers, hex digits in the year) raised as edge cases.
    //
    // IMPORTANT: if any of these expectations flip, that means the
    // heuristic was tightened/loosened — review the impact on
    // `latest_session_for_root` and `resolve_explicit` first.

    /// Boundary tests: minimum length, exact length, and one-past.
    #[test]
    fn looks_like_session_timestamp_length_boundaries() {
        // 18 chars: one short of the 19-char minimum — must reject.
        assert!(
            !looks_like_session_timestamp("2025-01-15T10-30-0"),
            "18 chars should be rejected (below min)"
        );
        // 19 chars: exact minimum — must accept.
        assert!(
            looks_like_session_timestamp("2025-01-15T10-30-00"),
            "19 chars should be accepted"
        );
        // 20 chars: trailing char after the SS digits is not validated,
        // so any non-control char is accepted.
        assert!(
            looks_like_session_timestamp("2025-01-15T10-30-00Z"),
            "20-char tz 'Z' suffix slips past the shallow check"
        );
        assert!(
            looks_like_session_timestamp("2025-01-15T10-30-000"),
            "20-char digit-only suffix slips past the shallow check"
        );
        // 21 chars with a non-digit tail still passes (only positions
        // 0..=10 are checked).
        assert!(
            looks_like_session_timestamp("2025-01-15T10-30-00!@#"),
            "trailing garbage past position 10 is ignored"
        );
    }

    /// First-character gate: any non-digit at position 0 must reject.
    #[test]
    fn looks_like_session_timestamp_first_char_must_be_digit() {
        // Leading '-' (e.g. a negative offset, or accidental CLI flag).
        assert!(
            !looks_like_session_timestamp("-025-01-15T10-30-00"),
            "leading '-' at position 0 must reject"
        );
        // Leading '+' (e.g. an explicit positive sign — not legal in ISO).
        assert!(
            !looks_like_session_timestamp("+025-01-15T10-30-00"),
            "leading '+' at position 0 must reject"
        );
        // Leading letter (e.g. an env prefix or hex digit).
        assert!(
            !looks_like_session_timestamp("a025-01-15T10-30-00"),
            "leading ASCII letter at position 0 must reject"
        );
        // Leading space (e.g. a stray-quoted CLI argument).
        assert!(
            !looks_like_session_timestamp(" 025-01-15T10-30-00"),
            "leading space at position 0 must reject"
        );
    }

    /// Hex / alphabetic characters in the year segment must reject
    /// because the heuristic explicitly demands `is_ascii_digit` for
    /// positions 0..=3 and 5..=6 and 8..=9.
    #[test]
    fn looks_like_session_timestamp_hex_or_alpha_year_rejected() {
        // Single hex digit in position 0 (e.g. 'a').
        assert!(
            !looks_like_session_timestamp("a025-01-15T10-30-00"),
            "hex digit in position 0 must reject"
        );
        // Hex digit in position 3 (the last year digit).
        assert!(
            !looks_like_session_timestamp("202a-01-15T10-30-00"),
            "hex digit in position 3 must reject"
        );
        // Hex digit in month slot (position 5).
        assert!(
            !looks_like_session_timestamp("2025-a1-15T10-30-00"),
            "hex digit in month must reject"
        );
        // Hex digit in day slot (position 8).
        assert!(
            !looks_like_session_timestamp("2025-01-a5T10-30-00"),
            "hex digit in day must reject"
        );
    }

    /// Separator slots (positions 4, 7) are hard-coded to '-'.
    /// Any other character (or missing bytes) must reject.
    #[test]
    fn looks_like_session_timestamp_separator_strictness() {
        // Slash separators instead of dashes.
        assert!(
            !looks_like_session_timestamp("2025/01/15T10-30-00"),
            "'/' as year/month separator must reject"
        );
        // Dots as separators.
        assert!(
            !looks_like_session_timestamp("2025.01.15T10-30-00"),
            "'.' as separator must reject (note: '.' splits head)"
        );
        // Missing separator at position 4 (digit instead).
        assert!(
            !looks_like_session_timestamp("20250-1-15T10-30-00"),
            "digit instead of '-' at position 4 must reject"
        );
    }

    /// Position 10 must be 'T' (or have the byte `b'T'`); this is the
    /// date/time separator in ISO-8601.
    #[test]
    fn looks_like_session_timestamp_position_10_t_strict() {
        // Space instead of 'T' (RFC 3339 allows this; the heuristic does not).
        assert!(
            !looks_like_session_timestamp("2025-01-15 10-30-00"),
            "' ' instead of 'T' must reject"
        );
        // Lowercase 't'.
        assert!(
            !looks_like_session_timestamp("2025-01-15t10-30-00"),
            "lowercase 't' must reject (heuristic is case-sensitive)"
        );
        // Underscore.
        assert!(
            !looks_like_session_timestamp("2025-01-15_10-30-00"),
            "underscore at position 10 must reject"
        );
        // Digit at position 10 (no separator at all).
        assert!(
            !looks_like_session_timestamp("2025-01-1510-30-00"),
            "digit at position 10 must reject"
        );
    }

    /// Multi-byte UTF-8 headers must reject: `as_bytes()` will produce
    /// >1 byte for non-ASCII leading characters, so position 0 won't
    /// be a digit and the heuristic naturally returns false.
    #[test]
    fn looks_like_session_timestamp_unicode_header_rejected() {
        // CJK leading char.
        assert!(
            !looks_like_session_timestamp("会话2025-01-15T10-30-00"),
            "CJK prefix must reject (multi-byte UTF-8 misaligns positions)"
        );
        // Emoji-leading.
        assert!(
            !looks_like_session_timestamp("🦀025-01-15T10-30-00"),
            "emoji prefix must reject"
        );
        // Cyrillic.
        assert!(
            !looks_like_session_timestamp("Год025-01-15T10-30-00"),
            "Cyrillic prefix must reject"
        );
        // Accented Latin.
        assert!(
            !looks_like_session_timestamp("É025-01-15T10-30-00"),
            "accented Latin prefix must reject"
        );
    }

    /// Multi-byte UTF-8 characters at *interior* positions also break
    /// the byte-offset heuristic — they slide the rest of the string
    /// by their UTF-8 width, so a later position that should be '-'
    /// will see the second byte of the multi-byte sequence instead.
    #[test]
    fn looks_like_session_timestamp_unicode_in_body_breaks_alignment() {
        // '日' is 3 bytes in UTF-8; it lands at byte offset 4..=6 and
        // displaces the year/month '-' separator from position 4 to 7.
        assert!(
            !looks_like_session_timestamp("2025日01-15T10-30-00"),
            "UTF-8 char in body must reject (displaces byte offsets)"
        );
    }

    /// Empty / whitespace-only / non-string inputs must reject.
    #[test]
    fn looks_like_session_timestamp_empty_and_whitespace_rejected() {
        assert!(
            !looks_like_session_timestamp(""),
            "empty string must reject"
        );
        assert!(
            !looks_like_session_timestamp(" "),
            "single space must reject"
        );
        assert!(
            !looks_like_session_timestamp("   "),
            "whitespace-only must reject"
        );
        assert!(
            !looks_like_session_timestamp("\t\n"),
            "control characters must reject"
        );
    }

    /// Timezone / sub-second suffixes: the heuristic trims at the
    /// first `.`, then checks only 11 byte positions on the *head*.
    /// Documented gaps from finding #12:
    ///   - `.123` (milliseconds) → head is "2025-01-15T10-30-00",
    ///     so it accepts even though the original had no sub-seconds.
    ///   - `Z` (UTC marker) at the end is accepted because positions
    ///     11.. are not validated.
    ///   - `+0800` / `-05:00` (numeric offsets) at the end are also
    ///     accepted.
    /// These tests pin the *current* permissive behavior so any future
    /// tightening is a deliberate, visible change.
    #[test]
    fn looks_like_session_timestamp_timezone_and_subsecond_gaps_documented() {
        // `.123` sub-second suffix is trimmed by split('.'); the head
        // is still a valid 19-char timestamp, so it accepts.
        assert!(
            looks_like_session_timestamp("2025-01-15T10-30-00.123"),
            "millisecond suffix is trimmed and accepted (documented gap)"
        );
        // 'Z' UTC marker is *not* trimmed by split('.'), but the
        // heuristic only checks positions 0..=10 — 'Z' is at position
        // 19 and ignored. The string still passes because positions
        // 0..=10 are all valid.
        assert!(
            looks_like_session_timestamp("2025-01-15T10-30-00Z"),
            "'Z' tz marker at the tail slips past the shallow check"
        );
        // Numeric offset '+0800' (no ':') — same story: only position
        // 0..=10 are validated, the tail is ignored.
        assert!(
            looks_like_session_timestamp("2025-01-15T10-30-00+0800"),
            "numeric tz offset '+0800' slips past the shallow check"
        );
        // Numeric offset with colon '-05:00' — same story.
        assert!(
            looks_like_session_timestamp("2025-01-15T10-30-00-05:00"),
            "numeric tz offset '-05:00' slips past the shallow check"
        );
        // Leap second: the seconds field "60" is *not* numerically
        // validated, so this passes. (UTC allows 23:59:60.)
        assert!(
            looks_like_session_timestamp("2025-06-30T23-59-60"),
            "leap-second '60' in the seconds slot is accepted (documented gap)"
        );
    }

    /// The diagnostic collector names session dirs in `UTC` using the
    /// format `YYYY-MM-DDTHH-MM-SS`. Pin a representative real-world
    /// value alongside a few plausible near-misses that *should* still
    /// be accepted (round-trip safe).
    #[test]
    fn looks_like_session_timestamp_realistic_session_ids() {
        // Same instant as the original baseline test, but expressed as
        // a real UTC timestamp without dashes in the time portion:
        // this is the format the collector actually writes.
        assert!(
            looks_like_session_timestamp("2026-06-10T08-15-22"),
            "realistic UTC session id must accept"
        );
        // Same instant with millisecond suffix (also a real collector
        // variant).
        assert!(
            looks_like_session_timestamp("2026-06-10T08-15-22.456"),
            "realistic UTC session id with ms suffix must accept"
        );
        // Midnight and end-of-day boundaries.
        assert!(
            looks_like_session_timestamp("2026-01-01T00-00-00"),
            "midnight UTC must accept"
        );
        assert!(
            looks_like_session_timestamp("2026-12-31T23-59-59"),
            "end-of-year must accept"
        );
    }

    #[test]
    fn format_datetime_utc_fallback_on_overflow() {
        // SystemTime::from_secs(0) converts to 1970-01-01 UTC.
        let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(0, 0).unwrap();
        let result = format_datetime_utc(dt);
        assert!(result.contains("1970-01-01"), "got: {result}");
    }

    #[test]
    fn compute_duration_returns_none_when_missing() {
        assert_eq!(compute_duration(None, Some(chrono::Utc::now())), None);
        assert_eq!(compute_duration(Some(chrono::Utc::now()), None), None);
        assert_eq!(compute_duration(None, None), None);
    }

    #[test]
    fn compute_duration_returns_correct_seconds() {
        let start = chrono::DateTime::<chrono::Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let end = chrono::DateTime::<chrono::Utc>::from_timestamp(1_800_000_120, 0).unwrap(); // +120s
        let dur = compute_duration(Some(start), Some(end)).unwrap();
        assert_eq!(dur.as_secs(), 120);
    }

    #[test]
    fn compute_duration_returns_none_when_start_after_end() {
        let start = chrono::DateTime::<chrono::Utc>::from_timestamp(1_800_000_120, 0).unwrap();
        let end = chrono::DateTime::<chrono::Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        assert_eq!(compute_duration(Some(start), Some(end)), None);
    }

    #[test]
    fn format_datetime_utc_shows_both_utc_and_local() {
        // Use a reasonably recent stable timestamp.
        let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let result = format_datetime_utc(dt);
        // Must contain the UTC part.
        assert!(
            result.contains("UTC"),
            "expected UTC time in output, got: {result}"
        );
        // Must contain local time (exact offset depends on TZ).
        assert!(
            result.contains(" / "),
            "expected local time separator ' / ' in output, got: {result}"
        );
        // Must include the IANA zone name in parentheses — disambiguates
        // short TZ abbreviations (CST, PST, EST — each maps to multiple
        // regions).
        assert!(
            result.contains(" (") && result.contains(")"),
            "expected IANA zone name in parens (e.g. '(UTC)'), got: {result}"
        );
    }

    #[test]
    fn format_system_time_routes_through_datetime_utc() {
        // Pin a deterministic UTC instant via SystemTime so we can assert
        // both halves appear with the same anchor (no time skew between
        // UTC and local views of the same SystemTime).
        let dt = chrono::DateTime::<chrono::Utc>::from_timestamp(1_800_000_000, 0).unwrap();
        let sys: std::time::SystemTime = dt.into();
        let via_system_time = format_system_time(sys);
        let via_datetime_utc = format_datetime_utc(dt);
        assert_eq!(
            via_system_time, via_datetime_utc,
            "format_system_time must delegate to format_datetime_utc (single source of truth)"
        );
    }

    // ── U8 (2026-06-21-002) tests ─────────────────────────────────────────

    fn rejection(
        hat: &str,
        topic: &str,
        reason_code: &str,
        retry_count: u32,
        ts: &str,
    ) -> crate::state::RejectionRecord {
        crate::state::RejectionRecord {
            ts: ts.to_string(),
            hat: hat.to_string(),
            topic: topic.to_string(),
            reason_code: reason_code.to_string(),
            retry_count,
            terminal_reason: None,
        }
    }

    #[test]
    fn validation_stage_to_source_covers_all_unified_stages() {
        // Every stage in the unified validation pipeline must
        // round-trip through the mapping.  Adding a new stage in
        // U9+ requires extending this test (it is the SSOT
        // contract for `validation_stage_to_source`).
        use crate::diagnosis::validation_stage_to_source;
        assert_eq!(validation_stage_to_source("origin"), "origin_guard");
        assert_eq!(validation_stage_to_source("publisher"), "event_policy");
        // Legacy alias: U7a rejection logs use "policy" (the
        // `RejectionStage::Policy::as_str()` value) before U4a
        // renamed the stage to "publisher".  Both must map to
        // the same `event_policy` source so legacy records stay
        // visible in the new reporter.
        assert_eq!(validation_stage_to_source("policy"), "event_policy");
        assert_eq!(
            validation_stage_to_source("required_fields"),
            "engine_required"
        );
        assert_eq!(
            validation_stage_to_source("execution_contract"),
            "execution_contract"
        );
        assert_eq!(
            validation_stage_to_source("workflow_guard"),
            "workflow_guard"
        );
        assert_eq!(
            validation_stage_to_source("step_handoff"),
            "step_handoff_gate"
        );
        assert_eq!(
            validation_stage_to_source("hat_handoff"),
            "hat_handoff_gate"
        );
        // Unknown stages fall back to "unknown" rather than
        // panicking — the caller still gets a usable report.
        assert_eq!(validation_stage_to_source("not_a_stage"), "unknown");
        assert_eq!(validation_stage_to_source(""), "unknown");
    }

    #[test]
    fn u8_empty_rejection_log_yields_empty_report() {
        // Edge case: rejection log is absent → empty report, no
        // panic.  `used_ledger` is false so the CLI knows to fall
        // back to the session-level view.
        let tmp = tempfile::TempDir::new().unwrap();
        let report = report_from_ledger(tmp.path()).expect("report");
        assert!(report.root_causes.is_empty());
        assert_eq!(report.rejection_summary.record_count, 0);
        assert!(!report.used_ledger);
        assert_eq!(report.workspace, tmp.path());
    }

    #[test]
    fn u8_invalid_workspace_returns_err() {
        let tmp = tempfile::TempDir::new().unwrap();
        let bogus = tmp.path().join("does-not-exist");
        let err = report_from_ledger(&bogus).unwrap_err();
        assert!(matches!(err, LedgerReportError::InvalidWorkspace(_)));
    }

    #[test]
    fn u8_single_root_cause_groups_records_by_reason_code() {
        let tmp = tempfile::TempDir::new().unwrap();
        // Three records on the same (stage, reason_code) tuple.
        let r1 = rejection(
            "executor",
            "work.done",
            "execution_contract:missing_field",
            1,
            "2026-06-22T01:00:00Z",
        );
        let r2 = rejection(
            "executor",
            "work.done",
            "execution_contract:missing_field",
            2,
            "2026-06-22T01:00:10Z",
        );
        let r3 = rejection(
            "executor",
            "work.done",
            "execution_contract:missing_field",
            3,
            "2026-06-22T01:00:20Z",
        );
        for r in [&r1, &r2, &r3] {
            crate::state::append_rejection(tmp.path(), r).unwrap();
        }
        let report = report_from_ledger(tmp.path()).unwrap();
        assert!(report.used_ledger);
        assert_eq!(report.rejection_summary.record_count, 3);
        assert_eq!(report.rejection_summary.distinct_reason_codes, 1);
        assert_eq!(report.root_causes.len(), 1);
        let rc = &report.root_causes[0];
        assert_eq!(rc.reason_code, "execution_contract:missing_field");
        assert_eq!(rc.stage, "execution_contract");
        assert_eq!(rc.frequency, 3);
        assert_eq!(rc.affected_hats, vec!["executor".to_string()]);
        assert_eq!(rc.affected_topics, vec!["work.done".to_string()]);
        assert!(!rc.escalated);
    }

    #[test]
    fn u8_root_cause_sorted_by_frequency_then_reason_code() {
        let records = vec![
            rejection(
                "a",
                "t.x",
                "origin:missing_field",
                1,
                "2026-06-22T00:00:00Z",
            ),
            rejection(
                "b",
                "t.y",
                "origin:missing_field",
                1,
                "2026-06-22T00:00:01Z",
            ),
            rejection(
                "c",
                "t.z",
                "origin:missing_field",
                1,
                "2026-06-22T00:00:02Z",
            ),
            rejection(
                "d",
                "t.w",
                "execution_contract:type_mismatch",
                1,
                "2026-06-22T00:00:03Z",
            ),
        ];
        let (causes, summary) = aggregate_rejection_records(&records);
        assert_eq!(summary.distinct_reason_codes, 2);
        // `origin:missing_field` has frequency 3 > 1, so it wins.
        assert_eq!(causes[0].reason_code, "origin:missing_field");
        assert_eq!(causes[0].frequency, 3);
        assert_eq!(causes[1].reason_code, "execution_contract:type_mismatch");
        assert_eq!(causes[1].frequency, 1);
    }

    #[test]
    fn u8_root_cause_flags_escalation_when_terminal_reason_present() {
        let mut rec = rejection(
            "executor",
            "work.done",
            "execution_contract:missing_field",
            3,
            "2026-06-22T02:00:00Z",
        );
        rec.terminal_reason = Some("retry budget exhausted".to_string());
        let (causes, summary) = aggregate_rejection_records(&[rec]);
        assert_eq!(summary.terminal_count, 1);
        assert!(causes[0].escalated);
    }

    #[test]
    fn u8_ledger_summary_counts_commit_deltas() {
        let tmp = tempfile::TempDir::new().unwrap();
        let mut ledger = crate::state::StateLedger::new(tmp.path(), true);
        ledger
            .commit(
                crate::state::CommitDelta::RejectionRecorded {
                    key: "policy::executor::work.done::missing_field".to_string(),
                    message: None,
                    topic: Some("work.done".to_string()),
                },
                Some("work.done".to_string()),
            )
            .unwrap();
        ledger
            .commit(
                crate::state::CommitDelta::RejectionBudgetTripped {
                    key: "policy::executor::work.done::missing_field".to_string(),
                    terminal_reason: "retry budget exhausted".to_string(),
                },
                None,
            )
            .unwrap();
        let summary = read_ledger_summary(tmp.path());
        assert_eq!(summary.commit_count, 2);
        assert_eq!(summary.rejection_commits, 1);
        assert_eq!(summary.escalation_commits, 1);
        assert!(!summary.corruption);
        assert!(summary.warnings.is_empty());
    }

    #[test]
    fn u8_render_markdown_contains_root_cause_table() {
        let mut rec = rejection(
            "executor",
            "work.done",
            "origin:missing_field",
            1,
            "2026-06-22T03:00:00Z",
        );
        rec.terminal_reason = Some("escalated".to_string());
        let (causes, summary) = aggregate_rejection_records(&[rec]);
        let report = DiagnosisReport {
            schema_version: DIAGNOSIS_LEDGER_SCHEMA_VERSION,
            workspace: PathBuf::from("/tmp/ws"),
            used_ledger: true,
            root_causes: causes,
            rejection_summary: summary,
            ledger_summary: LedgerSummary::default(),
        };
        let md = render_diagnosis_report_markdown(&report);
        assert!(md.contains("# Ralph Diagnosis Report"));
        assert!(md.contains("## Root causes"));
        assert!(md.contains("`origin:missing_field`"));
        assert!(md.contains("executor"));
        assert!(md.contains("work.done"));
        // Escalated flag rendered as "yes".
        assert!(md.contains("| yes |"));
    }

    #[test]
    fn u8_render_json_carries_schema_version_and_root_causes() {
        let rec = rejection(
            "executor",
            "work.done",
            "execution_contract:type_mismatch",
            1,
            "2026-06-22T04:00:00Z",
        );
        let (causes, summary) = aggregate_rejection_records(&[rec]);
        let report = DiagnosisReport {
            schema_version: DIAGNOSIS_LEDGER_SCHEMA_VERSION,
            workspace: PathBuf::from("/tmp/ws"),
            used_ledger: true,
            root_causes: causes,
            rejection_summary: summary,
            ledger_summary: LedgerSummary::default(),
        };
        let value = render_diagnosis_report_json(&report);
        assert_eq!(value["schema_version"], "u8-1");
        let causes_json = value["root_causes"].as_array().unwrap();
        assert_eq!(causes_json.len(), 1);
        assert_eq!(
            causes_json[0]["reason_code"],
            "execution_contract:type_mismatch"
        );
        // `source` is the validation_stage_to_source mapping.
        assert_eq!(causes_json[0]["source"], "execution_contract");
        // JSON must NOT include markdown headings.
        let serialized = serde_json::to_string(&value).unwrap();
        assert!(!serialized.contains("## "));
    }

    #[test]
    fn u8_corrupt_commit_log_surfaces_warning_and_corruption_flag() {
        // A parse error in the commit log must surface as both
        // a warning and the `corruption` flag — the report stays
        // usable, just partial.
        let tmp = tempfile::TempDir::new().unwrap();
        let log_path = tmp.path().join(".ralph").join("ledger.jsonl");
        std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();
        // First a valid commit, then garbage.
        let mut ledger = crate::state::StateLedger::new(tmp.path(), true);
        ledger
            .commit(
                crate::state::CommitDelta::RejectionRecorded {
                    key: "x".into(),
                    message: None,
                    topic: None,
                },
                None,
            )
            .unwrap();
        let mut f = std::fs::OpenOptions::new()
            .append(true)
            .open(&log_path)
            .unwrap();
        use std::io::Write as _;
        f.write_all(b"not-json\n").unwrap();
        let summary = read_ledger_summary(tmp.path());
        assert_eq!(summary.commit_count, 0, "parse error → zero commits");
        assert!(summary.corruption);
        assert!(!summary.warnings.is_empty());
    }

    #[test]
    fn u8_empty_report_does_not_panic_in_render() {
        let report = DiagnosisReport {
            schema_version: DIAGNOSIS_LEDGER_SCHEMA_VERSION,
            workspace: PathBuf::from("/tmp/ws"),
            used_ledger: false,
            root_causes: Vec::new(),
            rejection_summary: RejectionSummary::default(),
            ledger_summary: LedgerSummary::default(),
        };
        let md = render_diagnosis_report_markdown(&report);
        assert!(md.contains("# Ralph Diagnosis Report"));
        assert!(md.contains("No rejection records found"));
        assert!(md.contains("No aggregated root causes"));
        assert!(md.contains("No commit log found"));
        let value = render_diagnosis_report_json(&report);
        assert_eq!(value["used_ledger"], false);
        assert_eq!(value["root_causes"].as_array().unwrap().len(), 0);
    }
}
