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
    input_bundle as bundle_schema, ArtifactStatus, DiagnosisInputBundle, ManifestStatus,
};

/// Public status of the bundle, surfaced both in the report and in
/// the manifest's own `status` field.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BundleStatus {
    Present,
    Pending,
    Finalized,
    Degraded,
    Missing,
    Legacy,
    NotApplicable,
}

impl Default for BundleStatus {
    fn default() -> Self {
        Self::Missing
    }
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
        None => DiagnosisInputReport {
            status: BundleStatus::Legacy,
            ..DiagnosisInputReport::default()
        },
    }
}

fn project_bundle(bundle: &DiagnosisInputBundle) -> DiagnosisInputReport {
    DiagnosisInputReport {
        status: BundleStatus::from(bundle.manifest_status),
        path: Some("diagnosis-input.json".to_string()),
        schema_version: Some(bundle.schema_version.clone()),
        preset_label: bundle.run.preset_label.clone(),
        loop_id: bundle.run.loop_id.clone(),
        baseline_sha: bundle.run.baseline_sha.clone().or_else(|| {
            bundle.code_baseline.head_sha.clone()
        }),
        worktree: Some(bundle.code_baseline.worktree),
        execution_capabilities: bundle.execution_capabilities.clone(),
        artifacts: bundle.artifacts.iter().cloned().map(Into::into).collect(),
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
    let mut summary = RuntimeTraceReport {
        status: BundleStatus::Present,
        path: Some("runtime-trace.jsonl".to_string()),
        ..RuntimeTraceReport::default()
    };
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) => {
                summary.record_count += 1;
                if let Some(seq) = v.get("sequence").and_then(|x| x.as_u64()) {
                    summary.first_sequence.get_or_insert(seq);
                    summary.last_sequence = Some(seq);
                }
            }
            Err(_) => summary.malformed_lines += 1,
        }
    }
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
    let mut report = FeedbackLifecycleReport {
        status: BundleStatus::Present,
        path: Some("feedback.jsonl".to_string()),
        ..FeedbackLifecycleReport::default()
    };
    for line in body.lines() {
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<Value>(line) {
            Ok(v) => {
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
                    attempt: v.get("attempt").and_then(|x| x.as_u64()).map(|n| n as u32),
                    sequence: v.get("sequence").and_then(|x| x.as_u64()).unwrap_or(0),
                    iteration: v.get("iteration").and_then(|x| x.as_u64()).unwrap_or(0),
                };
                report.rows.push(row);
            }
            Err(_) => report.malformed_lines += 1,
        }
    }
    report
}

/// Plan 2026-08-12-001 Unit 4 / Unit 5: layer repair
/// suggestions and evidence gaps from the bundle state plus
/// the legacy top findings. The mapper is pure and never
/// invokes any I/O or external command.
pub mod suggestions {
    use super::{
        BundleStatus, DiagnosisInputReport, EvidenceGap, FeedbackLifecycleReport,
        RepairSuggestion, RuntimeTraceReport,
    };
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

        match input.status {
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
            _ => {}
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
            suggestions.push(RepairSuggestion {
                tier: "mid".to_string(),
                finding_refs: vec!["runtime_trace.malformed".to_string()],
                evidence_refs: vec!["runtime-trace.jsonl".to_string()],
                confidence: Some(70),
                text: "runtime-trace.jsonl contains malformed lines. Treat the summary as advisory; the underlying events still ran on the bus."
                    .to_string(),
            });
        }

        if feedback.status == BundleStatus::Missing {
            gaps.push(EvidenceGap {
                artifact: "feedback.jsonl".to_string(),
                reason: "feedback lifecycle missing; recovery sources remain in recovery.jsonl".to_string(),
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
            && matches!(input.status, BundleStatus::Present | BundleStatus::Finalized)
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

