//! Plan 2026-08-12-001 Unit 4: bundle-first readers for the
//! `ralph diagnose` reporter. The readers consume the new
//! `diagnosis-input.json`, `runtime-trace.jsonl` and
//! `feedback.jsonl` sidecars and project them into typed
//! structures that the reporter attaches to `SessionData` /
//! `Report` as additive fields. Old sessions without the new
//! files fall back to `legacy` and contribute empty additive
//! sections; the readers never panic and never fail the
//! report.

use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::diagnostics::{
    ArtifactStatus, DIAGNOSIS_INPUT_SCHEMA_VERSION, DiagnosisInputBundle, ManifestStatus,
    input_bundle as bundle_schema,
    input_bundle::{BoundaryCoverageEntry, BoundaryCoverageStatus},
};

/// Public status of the bundle, surfaced both in the report and in
/// the manifest's own `status` field.
///
/// `SchemaMismatch` (plan 2026-08-12-001 fix-plan U2 / synth:P0-2)
/// carries the on-disk version and the reader's compiled
/// version so the report can surface "the on-disk bundle is
/// authoritative; re-read with a newer binary" instead of
/// silently demoting it to `Legacy`. The two strings exist for
/// forensic traceability — when the reader is newer than the
/// writer the user is on a downgrade; when the reader is older
/// the user is on an upgrade and can simply upgrade again.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleStatus {
    Present,
    Pending,
    Finalized,
    Degraded,
    #[default]
    Missing,
    Legacy,
    NotApplicable,
    /// Manifest parsed cleanly but carries a `schema_version`
    /// different from `DIAGNOSIS_INPUT_SCHEMA_VERSION`. Treated
    /// as authoritative-on-disk; the report explains the version
    /// gap instead of mapping to `Legacy` (which would imply the
    /// session predates the bundle format).
    SchemaMismatch {
        on_disk_version: String,
        reader_version: String,
    },
}

impl From<ManifestStatus> for BundleStatus {
    fn from(value: ManifestStatus) -> Self {
        match value {
            ManifestStatus::Pending => Self::Pending,
            ManifestStatus::Present => Self::Present,
            ManifestStatus::Finalized => Self::Finalized,
            ManifestStatus::Degraded => Self::Degraded,
            ManifestStatus::Missing => Self::Missing,
            ManifestStatus::Legacy => Self::Legacy,
            ManifestStatus::NotApplicable => Self::NotApplicable,
        }
    }
}

/// Status object for the input bundle. Always present in the
/// report (even on legacy sessions), so consumers can iterate
/// without null checks.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosisInputReport {
    pub status: BundleStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub schema_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset_label: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub loop_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub baseline_sha: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<bool>,
    #[serde(default)]
    pub execution_capabilities: Vec<String>,
    #[serde(default)]
    pub artifacts: Vec<ArtifactReport>,
    /// Plan 2026-08-26-1104 U07: per-boundary coverage rows
    /// projected from the manifest's `boundary_coverage[]`.
    /// Empty for v1 (legacy) sessions and for unknown schema
    /// versions (the reader never fabricates rows when the
    /// on-disk format is unrecognized). For v2 finalized
    /// manifests, the vector is always eight entries long and
    /// ordered by `CausalBoundary::ALL`.
    #[serde(default)]
    pub boundary_coverage: Vec<BoundaryCoverageReport>,
}

/// One row of the boundary coverage report, projected from
/// [`crate::diagnostics::BoundaryCoverageEntry`]. The
/// `affects` token (`"boundary:<name>"`) is what the
/// suggestion mapper keys on when emitting evidence gaps for
/// per-receipt coverage misses.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundaryCoverageReport {
    pub boundary: String,
    #[serde(default)]
    pub expected: u64,
    #[serde(default)]
    pub recorded: u64,
    pub status: BoundaryCoverageStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl From<&BoundaryCoverageEntry> for BoundaryCoverageReport {
    fn from(entry: &BoundaryCoverageEntry) -> Self {
        Self {
            boundary: entry.boundary.as_str().to_string(),
            expected: entry.expected,
            recorded: entry.recorded,
            status: entry.status,
            reason: entry.reason.clone(),
        }
    }
}

/// Per-artifact integrity / status, projected from the manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactReport {
    pub path: String,
    pub status: ArtifactStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
}

impl From<bundle_schema::ArtifactIntegrity> for ArtifactReport {
    fn from(value: bundle_schema::ArtifactIntegrity) -> Self {
        Self {
            path: value.path,
            status: value.status,
            sha256: value.sha256,
            size_bytes: value.size_bytes,
        }
    }
}

/// Runtime trace summary. The reporter computes the count,
/// first/last sequence and any malformed-line count from the
/// on-disk JSONL.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RuntimeTraceReport {
    pub status: BundleStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub record_count: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub first_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_sequence: Option<u64>,
    #[serde(default)]
    pub malformed_lines: u64,
    /// Plan 2026-08-12-001 fix-plan U5: `true` when the on-disk
    /// rows form a contiguous sequence (last_seq - first_seq + 1
    /// == record_count). When a write failure leaves a gap, this
    /// flips to `false` so the reporter can surface the gap as an
    /// evidence gap rather than silently undercounting.
    #[serde(default)]
    pub monotonic_sequences: bool,
}

/// One feedback row projected into the report. We only keep the
/// fields the report actually surfaces; the full row stays in
/// `feedback.jsonl` for deep-dive tooling.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackLifecycleRow {
    pub feedback_id: String,
    pub retry_key: String,
    pub phase: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action_kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub attempt: Option<u32>,
    #[serde(default)]
    pub sequence: u64,
    #[serde(default)]
    pub iteration: u64,
}

/// Feedback lifecycle summary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FeedbackLifecycleReport {
    pub status: BundleStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default)]
    pub rows: Vec<FeedbackLifecycleRow>,
    #[serde(default)]
    pub malformed_lines: u64,
    /// Plan 2026-08-12-001 fix-plan U5: `true` when the on-disk
    /// rows form a contiguous sequence (last_seq - first_seq + 1
    /// == rows.len()). When a write failure leaves a gap, this
    /// flips to `false` so the reporter can surface the gap as an
    /// evidence gap rather than silently undercounting.
    #[serde(default)]
    pub monotonic_sequences: bool,
}

/// Repair suggestion. Always non-executing; the reporter never
/// runs anything.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepairSuggestion {
    pub tier: String,
    #[serde(default)]
    pub finding_refs: Vec<String>,
    #[serde(default)]
    pub evidence_refs: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub confidence: Option<u8>,
    pub text: String,
}

/// Evidence gap surfaced in the report. Distinct from
/// warnings: a gap is "this artifact is missing or malformed and
/// the report would normally depend on it", while a warning is a
/// free-form diagnostic note.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct EvidenceGap {
    pub artifact: String,
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub affects: Option<String>,
}

/// Read the manifest from disk and project it into a report
/// status object. Returns `None` only when the file truly
/// cannot be read or parsed.
pub fn read_input_bundle_report(session_dir: &Path) -> DiagnosisInputReport {
    match bundle_schema::read_manifest(session_dir) {
        Some(b) => project_bundle(&b),
        None if bundle_schema::manifest_path(session_dir).exists() => {
            tracing::warn!(
                target: "ralph_core::diagnosis",
                artifact = "diagnosis-input.json",
                "diagnosis-input.json exists but is malformed; reporting degraded evidence"
            );
            DiagnosisInputReport {
                status: BundleStatus::Degraded,
                path: Some("diagnosis-input.json".to_string()),
                ..DiagnosisInputReport::default()
            }
        }
        None => DiagnosisInputReport {
            status: BundleStatus::Legacy,
            ..DiagnosisInputReport::default()
        },
    }
}

fn project_bundle(bundle: &DiagnosisInputBundle) -> DiagnosisInputReport {
    // Plan 2026-08-12-001 fix-plan U2 / synth:P0-2: detect the
    // schema-version mismatch at projection time so the report
    // distinguishes "session predates bundle format" (Legacy)
    // from "running reader is older/newer than the on-disk
    // writer" (SchemaMismatch). The latter must NOT be
    // collapsed into Legacy — that destroyed rollback safety
    // when an operator downgraded `ralph` to an older binary.
    //
    // Plan 2026-08-26-1104 U07: v1 is now a KNOWN prior
    // version (the v1 manifest schema shipped with the prior
    // plan). Readers must keep v1 sessions on the Legacy
    // path so the existing legacy fallback still works for
    // sessions recorded before the v2 bump. Truly unknown
    // versions (anything other than v1 / v2) surface as
    // SchemaMismatch.
    let status = if bundle.schema_version == DIAGNOSIS_INPUT_SCHEMA_VERSION {
        BundleStatus::from(bundle.manifest_status)
    } else if bundle.schema_version == "run-diagnosis-input/v1" {
        BundleStatus::Legacy
    } else {
        BundleStatus::SchemaMismatch {
            on_disk_version: bundle.schema_version.clone(),
            reader_version: DIAGNOSIS_INPUT_SCHEMA_VERSION.to_string(),
        }
    };
    DiagnosisInputReport {
        status,
        path: Some("diagnosis-input.json".to_string()),
        schema_version: Some(bundle.schema_version.clone()),
        preset_label: bundle.run.preset_label.clone(),
        loop_id: bundle.run.loop_id.clone(),
        baseline_sha: bundle
            .run
            .baseline_sha
            .clone()
            .or_else(|| bundle.code_baseline.head_sha.clone()),
        worktree: Some(bundle.code_baseline.worktree),
        execution_capabilities: bundle.execution_capabilities.clone(),
        artifacts: bundle.artifacts.iter().cloned().map(Into::into).collect(),
        // Plan 2026-08-26-1104 U07: project the on-disk
        // `boundary_coverage[]` rows. The reader relies on
        // `CausalBoundary::ALL` to keep iteration order
        // stable across runs; the producer already
        // serializes in that order.
        boundary_coverage: bundle.boundary_coverage.iter().map(Into::into).collect(),
    }
}

/// Read `runtime-trace.jsonl` and project a summary. Malformed
/// lines increment `malformed_lines` and are otherwise ignored.
pub fn read_runtime_trace_report(session_dir: &Path) -> RuntimeTraceReport {
    let path = session_dir.join("runtime-trace.jsonl");
    let body = match fs::read_to_string(&path) {
        Ok(b) => b,
        Err(_) => {
            return RuntimeTraceReport {
                status: BundleStatus::Missing,
                ..RuntimeTraceReport::default()
            };
        }
    };
    // Plan 2026-08-12-001 fix-plan U8: empty / whitespace-only
    // sidecar files must NOT spoof Present with record_count=0.
    if body.trim().is_empty() {
        tracing::warn!(
            target: "ralph_core::diagnostics",
            artifact = "runtime-trace.jsonl",
            "empty sidecar file detected; treating as Missing"
        );
        return RuntimeTraceReport {
            status: BundleStatus::Missing,
            path: Some("runtime-trace.jsonl".to_string()),
            ..RuntimeTraceReport::default()
        };
    }
    let mut summary = RuntimeTraceReport {
        status: BundleStatus::Present,
        path: Some("runtime-trace.jsonl".to_string()),
        ..RuntimeTraceReport::default()
    };
    // Plan 2026-08-12-001 fix-plan U12: the line-by-line
    // summary loop is shared between the runtime-trace and
    // feedback readers (open → trim → parse → record_count +
    // sequence projection). The per-row projection logic stays
    // inline; the bookkeeping (`record_count`,
    // `first_sequence`, `last_sequence`,
    // `monotonic_sequences`) is centralized here so adding a
    // new sidecar reader is a one-liner.
    let mut first_sequence: Option<u64> = None;
    let mut last_sequence: Option<u64> = None;
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) if is_runtime_trace_record(&v) => {
                summary.record_count += 1;
                if let Some(seq) = v.get("sequence").and_then(|x| x.as_u64()) {
                    first_sequence.get_or_insert(seq);
                    last_sequence = Some(seq);
                }
            }
            Ok(_) | Err(_) => summary.malformed_lines += 1,
        }
    }
    summary.first_sequence = first_sequence;
    summary.last_sequence = last_sequence;
    if summary.malformed_lines > 0 {
        summary.status = BundleStatus::Degraded;
    }
    summary.monotonic_sequences = match (first_sequence, last_sequence) {
        (Some(first), Some(last)) => {
            last >= first
                && last.checked_sub(first).and_then(|span| span.checked_add(1))
                    == Some(summary.record_count)
        }
        _ => summary.record_count == 0,
    };
    summary
}

/// Read `feedback.jsonl` and project a summary.
pub fn read_feedback_lifecycle_report(session_dir: &Path) -> FeedbackLifecycleReport {
    let path = session_dir.join("feedback.jsonl");
    let body = match fs::read_to_string(&path) {
        Ok(b) => b,
        Err(_) => {
            return FeedbackLifecycleReport {
                status: BundleStatus::Missing,
                ..FeedbackLifecycleReport::default()
            };
        }
    };
    // Plan 2026-08-12-001 fix-plan U8: empty / whitespace-only
    // sidecar files must NOT spoof Present with record_count=0.
    if body.trim().is_empty() {
        tracing::warn!(
            target: "ralph_core::diagnostics",
            artifact = "feedback.jsonl",
            "empty sidecar file detected; treating as Missing"
        );
        return FeedbackLifecycleReport {
            status: BundleStatus::Missing,
            path: Some("feedback.jsonl".to_string()),
            ..FeedbackLifecycleReport::default()
        };
    }
    let mut report = FeedbackLifecycleReport {
        status: BundleStatus::Present,
        path: Some("feedback.jsonl".to_string()),
        ..FeedbackLifecycleReport::default()
    };
    let mut first_sequence: Option<u64> = None;
    let mut last_sequence: Option<u64> = None;
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) if is_feedback_record(&v) => {
                let seq = v.get("sequence").and_then(|x| x.as_u64()).unwrap_or(0);
                first_sequence.get_or_insert(seq);
                last_sequence = Some(seq);
                let row = FeedbackLifecycleRow {
                    feedback_id: v
                        .get("feedback_id")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    retry_key: v
                        .get("retry_key")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    phase: v
                        .get("phase")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                    action_kind: v
                        .get("action_kind")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string()),
                    outcome: v
                        .get("outcome")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string()),
                    status: v
                        .get("status")
                        .and_then(|x| x.as_str())
                        .map(|s| s.to_string()),
                    evidence_refs: v
                        .get("evidence_refs")
                        .and_then(|x| x.as_array())
                        .map(|refs| {
                            refs.iter()
                                .filter_map(|reference| reference.as_str().map(str::to_string))
                                .collect()
                        })
                        .unwrap_or_default(),
                    attempt: v.get("attempt").and_then(|x| x.as_u64()).map(|n| n as u32),
                    sequence: seq,
                    iteration: v.get("iteration").and_then(|x| x.as_u64()).unwrap_or(0),
                };
                report.rows.push(row);
            }
            Ok(_) | Err(_) => report.malformed_lines += 1,
        }
    }
    report.monotonic_sequences = match (first_sequence, last_sequence) {
        (Some(first), Some(last)) => {
            last >= first
                && last.checked_sub(first).and_then(|span| span.checked_add(1))
                    == Some(report.rows.len() as u64)
        }
        _ => report.rows.is_empty(),
    };
    if report.malformed_lines > 0 {
        report.status = BundleStatus::Degraded;
    }
    report
}

fn is_runtime_trace_record(value: &Value) -> bool {
    value.is_object()
        && value
            .get("schema_version")
            .and_then(Value::as_str)
            .is_some()
        && value.get("ts").and_then(Value::as_str).is_some()
        && value.get("iteration").and_then(Value::as_u64).is_some()
        && value.get("sequence").and_then(Value::as_u64).is_some()
        && value.get("phase").and_then(Value::as_str).is_some()
        && value.get("kind").and_then(Value::as_str).is_some()
}

fn is_feedback_record(value: &Value) -> bool {
    value.is_object()
        && value
            .get("schema_version")
            .and_then(Value::as_str)
            .is_some()
        && value.get("ts").and_then(Value::as_str).is_some()
        && value.get("iteration").and_then(Value::as_u64).is_some()
        && value.get("sequence").and_then(Value::as_u64).is_some()
        && value.get("feedback_id").and_then(Value::as_str).is_some()
        && value.get("retry_key").and_then(Value::as_str).is_some()
        && value.get("phase").and_then(Value::as_str).is_some()
}

/// Plan 2026-08-12-001 Unit 4 / Unit 5: layer repair
/// suggestions and evidence gaps from the bundle state plus
/// the legacy top findings. The mapper is pure and never
/// invokes any I/O or external command.
pub mod suggestions {
    use super::{
        BundleStatus, DiagnosisInputReport, EvidenceGap, FeedbackLifecycleReport, RepairSuggestion,
        RuntimeTraceReport,
    };
    use crate::diagnostics::input_bundle as bundle_schema;
    use std::path::Path;

    /// Build a deterministic set of repair suggestions and
    /// evidence gaps from the bundle state. Suggestions
    /// are tier-labeled (`short` / `mid` / `long`) and never
    /// carry executable commands.
    pub fn build_suggestions_and_gaps(
        input: &DiagnosisInputReport,
        trace: &RuntimeTraceReport,
        feedback: &FeedbackLifecycleReport,
        warnings: &[String],
        session_dir: &Path,
    ) -> (Vec<RepairSuggestion>, Vec<EvidenceGap>) {
        let mut suggestions = Vec::new();
        let mut gaps = Vec::new();

        match &input.status {
            BundleStatus::Missing | BundleStatus::Legacy => {
                gaps.push(EvidenceGap {
                    artifact: "diagnosis-input.json".to_string(),
                    reason: "bundle missing; using legacy fallback".to_string(),
                    affects: Some("bundle".to_string()),
                });
                suggestions.push(RepairSuggestion {
                    tier: "short".to_string(),
                    finding_refs: vec!["bundle.missing".to_string()],
                    evidence_refs: vec!["diagnosis-input.json".to_string()],
                    confidence: Some(50),
                    text: "Re-run with diagnostics enabled (full or runtime_diagnosis_artifacts) to populate the new bundle; the legacy report is still produced for backwards compatibility."
                        .to_string(),
                });
            }
            BundleStatus::Degraded => {
                gaps.push(EvidenceGap {
                    artifact: "diagnosis-input.json".to_string(),
                    reason: "bundle write failed mid-run".to_string(),
                    affects: Some("bundle".to_string()),
                });
                suggestions.push(RepairSuggestion {
                    tier: "short".to_string(),
                    finding_refs: vec!["bundle.degraded".to_string()],
                    evidence_refs: vec!["diagnosis-input.json".to_string()],
                    confidence: Some(60),
                    text: "Bundle write failed mid-run. Check filesystem quota and the session directory permissions."
                        .to_string(),
                });
            }
            BundleStatus::SchemaMismatch {
                on_disk_version,
                reader_version,
            } => {
                // Plan 2026-08-12-001 fix-plan U2 / synth:P0-2:
                // do NOT collapse into "re-run with diagnostics
                // enabled" — that path is misleading when the
                // on-disk bundle is intact and only the
                // reader/writer versions differ.
                gaps.push(EvidenceGap {
                    artifact: "diagnosis-input.json".to_string(),
                    reason: format!(
                        "schema version mismatch: on-disk={on_disk_version}, reader={reader_version}; the on-disk bundle is authoritative"
                    ),
                    affects: Some("bundle".to_string()),
                });
                suggestions.push(RepairSuggestion {
                    tier: "short".to_string(),
                    finding_refs: vec!["bundle.schema_mismatch".to_string()],
                    evidence_refs: vec!["diagnosis-input.json".to_string()],
                    confidence: Some(75),
                    text: format!(
                        "Bundle schema-version mismatch (on-disk={on_disk_version}, reader={reader_version}). The on-disk bundle is authoritative. Re-read with a `ralph` binary whose compiled `DIAGNOSIS_INPUT_SCHEMA_VERSION` matches {on_disk_version} (or upgrade the writer so future bundles match {reader_version})."
                    ),
                });
            }
            _ => {}
        }

        // Plan 2026-08-26-1104 U07: per-boundary gap evidence.
        // For v2 manifests the producer already stamped a
        // `reason` onto every `Gap` row; the mapper re-emits
        // one `evidence_gap` per gap row with
        // `affects="boundary:<name>"` so the suggestion
        // mapper's downstream consumers (the report's
        // "Causal Attribution" section, the offline `ralph
        // diagnose` report) can pin the missing receipt
        // kind. v1 / legacy / schema-mismatch paths skip
        // this branch — the on-disk manifest never carried
        // `boundary_coverage[]` so we must not fabricate
        // gaps that don't exist on disk.
        if matches!(
            input.status,
            BundleStatus::Finalized | BundleStatus::Present | BundleStatus::Degraded
        ) {
            for entry in &input.boundary_coverage {
                if entry.status != bundle_schema::BoundaryCoverageStatus::Gap {
                    continue;
                }
                let name = entry.boundary.clone();
                let producer_reason = entry
                    .reason
                    .clone()
                    .unwrap_or_else(|| "expected != recorded".to_string());
                gaps.push(EvidenceGap {
                    artifact: "diagnosis-input.json".to_string(),
                    reason: format!(
                        "{} boundary gap: expected={}, recorded={}, reason={producer_reason}",
                        name, entry.expected, entry.recorded,
                    ),
                    affects: Some(format!("boundary:{name}")),
                });
            }
            if input
                .boundary_coverage
                .iter()
                .any(|e| e.status == bundle_schema::BoundaryCoverageStatus::Gap)
            {
                suggestions.push(RepairSuggestion {
                    tier: "short".to_string(),
                    finding_refs: vec!["bundle.boundary_gap".to_string()],
                    evidence_refs: vec!["diagnosis-input.json".to_string()],
                    confidence: Some(70),
                    text: "Boundary coverage has gap(s) — the affected receipt kind was attempted but not persisted. Check the runtime-trace.jsonl writer state and the per-receipt emit counters."
                        .to_string(),
                });
            }
        }

        if trace.status == BundleStatus::Missing {
            gaps.push(EvidenceGap {
                artifact: "runtime-trace.jsonl".to_string(),
                reason: "structured trace missing; only raw trace.jsonl is available".to_string(),
                affects: Some("runtime_trace".to_string()),
            });
            suggestions.push(RepairSuggestion {
                tier: "mid".to_string(),
                finding_refs: vec!["runtime_trace.missing".to_string()],
                evidence_refs: vec!["runtime-trace.jsonl".to_string()],
                confidence: Some(50),
                text: "Re-run with diagnostics enabled to populate runtime-trace.jsonl; the raw trace.jsonl is still the authoritative source for accepted events."
                    .to_string(),
            });
        } else if trace.malformed_lines > 0 {
            gaps.push(EvidenceGap {
                artifact: "runtime-trace.jsonl".to_string(),
                reason: format!(
                    "{} malformed line(s); lifecycle summary is incomplete",
                    trace.malformed_lines
                ),
                affects: Some("runtime_trace".to_string()),
            });
            suggestions.push(RepairSuggestion {
                tier: "mid".to_string(),
                finding_refs: vec!["runtime_trace.malformed".to_string()],
                evidence_refs: vec!["runtime-trace.jsonl".to_string()],
                confidence: Some(70),
                text: "runtime-trace.jsonl contains malformed lines. Treat the summary as advisory; the underlying events still ran on the bus."
                    .to_string(),
            });
        }
        if !trace.monotonic_sequences && trace.record_count > 0 {
            gaps.push(EvidenceGap {
                artifact: "runtime-trace.jsonl".to_string(),
                reason: "runtime trace sequence has a gap or is out of order".to_string(),
                affects: Some("runtime_trace".to_string()),
            });
        }

        if feedback.status == BundleStatus::Missing {
            gaps.push(EvidenceGap {
                artifact: "feedback.jsonl".to_string(),
                reason: "feedback lifecycle missing; recovery sources remain in recovery.jsonl"
                    .to_string(),
                affects: Some("feedback_lifecycle".to_string()),
            });
            suggestions.push(RepairSuggestion {
                tier: "mid".to_string(),
                finding_refs: vec!["feedback_lifecycle.missing".to_string()],
                evidence_refs: vec!["feedback.jsonl".to_string()],
                confidence: Some(40),
                text: "Re-run with diagnostics enabled to populate feedback.jsonl; the recovery.jsonl record stays the authoritative source."
                    .to_string(),
            });
        } else if feedback.malformed_lines > 0 {
            gaps.push(EvidenceGap {
                artifact: "feedback.jsonl".to_string(),
                reason: format!(
                    "{} malformed line(s); feedback lifecycle is incomplete",
                    feedback.malformed_lines
                ),
                affects: Some("feedback_lifecycle".to_string()),
            });
        }
        if !feedback.monotonic_sequences && !feedback.rows.is_empty() {
            gaps.push(EvidenceGap {
                artifact: "feedback.jsonl".to_string(),
                reason: "feedback sequence has a gap or is out of order".to_string(),
                affects: Some("feedback_lifecycle".to_string()),
            });
        }

        if !warnings.is_empty() {
            suggestions.push(RepairSuggestion {
                tier: "short".to_string(),
                finding_refs: vec!["report.warnings".to_string()],
                evidence_refs: vec![format!("warnings:{}", warnings.len())],
                confidence: Some(30),
                text: format!(
                    "Report surfaced {} warning(s). See the warnings section for raw I/O or parse errors.",
                    warnings.len()
                ),
            });
        }

        if suggestions.is_empty()
            && matches!(
                input.status,
                BundleStatus::Present | BundleStatus::Finalized
            )
        {
            suggestions.push(RepairSuggestion {
                tier: "long".to_string(),
                finding_refs: vec!["bundle.intact".to_string()],
                evidence_refs: vec![session_dir.display().to_string()],
                confidence: Some(20),
                text: "Bundle is intact. No specific short/mid-tier recommendation. Consider running the agent on a more diverse scenario corpus to harden coverage."
                    .to_string(),
            });
        }

        (suggestions, gaps)
    }
}

#[cfg(test)]
mod u2_schema_mismatch_tests {
    //! Plan 2026-08-12-001 fix-plan U2 / synth:P0-2: in-crate
    //! verification that the suggestion mapper distinguishes
    //! SchemaMismatch from Missing/Legacy. The public API
    //! (`build_suggestions_and_gaps`) lives in
    //! [`super::suggestions`]; this module exists so the
    //! Mapper-only contract is locked next to its impl.

    use super::suggestions::build_suggestions_and_gaps;
    use super::{BundleStatus, DiagnosisInputReport, FeedbackLifecycleReport, RuntimeTraceReport};
    use std::path::Path;

    #[test]
    fn schema_mismatch_arm_emits_version_specific_suggestion() {
        let report = DiagnosisInputReport {
            status: BundleStatus::SchemaMismatch {
                on_disk_version: "run-diagnosis-input/v999".to_string(),
                reader_version: "run-diagnosis-input/v1".to_string(),
            },
            ..DiagnosisInputReport::default()
        };
        // Pre-populate the trace/feedback reports with
        // matching Present status so they do not inject
        // "Re-run with diagnostics enabled" suggestions
        // unrelated to the SchemaMismatch arm under test.
        let mut trace = RuntimeTraceReport::default();
        trace.status = BundleStatus::Present;
        let mut feedback = FeedbackLifecycleReport::default();
        feedback.status = BundleStatus::Present;
        let (suggestions, gaps) =
            build_suggestions_and_gaps(&report, &trace, &feedback, &[], Path::new("/tmp/x"));
        assert!(
            gaps.iter()
                .any(|g| g.reason.contains("run-diagnosis-input/v999")),
            "evidence gap must mention on-disk version, got {:?}",
            gaps
        );
        let schema_mismatch_suggestions: Vec<_> = suggestions
            .iter()
            .filter(|s| s.finding_refs.iter().any(|r| r == "bundle.schema_mismatch"))
            .collect();
        assert!(
            schema_mismatch_suggestions
                .iter()
                .any(|s| s.text.contains("schema-version mismatch")
                    && s.text.contains("run-diagnosis-input/v999")),
            "bundle.schema_mismatch suggestion must reference schema-version mismatch and on-disk version, got {:?}",
            suggestions
        );
        // Hard contract: the SchemaMismatch-tagged
        // suggestion must NEVER be the misleading "Re-run
        // with diagnostics enabled" path (that path is
        // reserved for Missing/Legacy bundle status).
        for s in &schema_mismatch_suggestions {
            assert!(
                !s.text.contains("Re-run with diagnostics enabled"),
                "SchemaMismatch must not produce 'Re-run with diagnostics enabled' suggestion: {:?}",
                s
            );
        }
    }
}
