//! Self-describing `diagnosis-input.json` manifest for a diagnostics session.
//!
//! Plan 2026-08-12-001 Unit 1 introduces a fixed-shape manifest that
//! captures the run identity (session/loop/preset/config/baseline/capability)
//! and the integrity/status of every sidecar artifact written by the
//! collector (`trace.jsonl`, `recovery.jsonl`, `drift.jsonl`, plus the new
//! `diagnosis-summary.json`, `runtime-trace.jsonl`, `feedback.jsonl`
//! added in later Units). The manifest is written atomically via
//! [`NamedTempFile::persist`] and updated in place when the run identity
//! becomes complete (D11 two-stage metadata lifecycle).
//!
//! # Activation
//!
//! The manifest is created in the same session directory as the
//! existing `trace.jsonl`/`recovery.jsonl` writers. When the
//! [`crate::diagnostics::DiagnosticsCollector`] is in `full_diagnostics`
//! or `runtime_diagnosis_artifacts` mode, the manifest is written
//! (pending → present/finalized). When the collector is disabled
//! (or trace-only), no manifest is created.
//!
//! # Error handling
//!
//! All write paths are best-effort. Serialization, I/O, and atomic
//! replace failures emit a `tracing::warn!` and leave the in-memory
//! status at `degraded`; they never panic and never return an error
//! to the loop main path. If the manifest itself cannot be created
//! (e.g. the target path is a directory), the reporter observes
//! `status=missing` plus an evidence gap — not a `degraded`
//! claim that was never actually written.

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tempfile::NamedTempFile;

use crate::diagnostics::session::probe_session_dir_writable;

/// Status of the manifest itself, surfaced both in the on-disk
/// `status` field and via the collector's in-memory handle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestStatus {
    /// Initial state: collector created the file but `run_id`,
    /// `preset_label`, etc. have not been resolved yet (D11).
    Pending,
    /// Manifest is on disk with at least the baseline identity.
    Present,
    /// Manifest is on disk and reflects the final run identity
    /// (termination metadata applied). The reporter should treat
    /// this as the authoritative source.
    Finalized,
    /// Manifest write was attempted but at least one update failed.
    /// The on-disk file may be stale or incomplete; the reporter
    /// must surface this as an evidence gap.
    Degraded,
    /// Manifest could not be created at all (target path is a
    /// directory, permissions denied, etc.). Reporter observes
    /// `status=missing`.
    Missing,
    /// Session existed before this plan's manifest format. The
    /// reporter keeps the legacy fallback path active.
    Legacy,
    /// Diagnostics are disabled for this run. No manifest is
    /// written; the reporter treats bundle status as `not_applicable`.
    NotApplicable,
}

impl Default for ManifestStatus {
    fn default() -> Self {
        Self::Pending
    }
}

/// Status of a single artifact slot in the manifest. Independent of
/// the manifest's own [`ManifestStatus`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStatus {
    Present,
    Missing,
    Degraded,
    NotApplicable,
    Legacy,
}

impl Default for ArtifactStatus {
    fn default() -> Self {
        Self::NotApplicable
    }
}

/// Integrity record for one referenced artifact.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtifactIntegrity {
    /// Workspace-relative path of the artifact.
    pub path: String,
    /// Status of the artifact itself.
    pub status: ArtifactStatus,
    /// SHA-256 of the artifact's bytes, when present. Recorded
    /// only for files small enough to hash deterministically.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
    /// Size in bytes at the time the manifest was last updated.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size_bytes: Option<u64>,
    /// Last modification timestamp (RFC 3339), if known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_modified: Option<String>,
}

/// Run-level identity captured in the manifest. Resolved in two
/// stages per D11: created with the minimum session info in
/// `DiagnosticsCollector`, then completed with preset/config/
/// capability fields once the run-loop has parsed the config.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunMetadata {
    /// Session id (also the session directory name).
    #[serde(default)]
    pub session_id: Option<String>,
    /// Loop id from `RALPH_CURRENT_LOOP_ID` or the resolved plan key.
    #[serde(default)]
    pub loop_id: Option<String>,
    /// Resolved preset label (e.g. `builtin:ce-executor-pipeline`).
    #[serde(default)]
    pub preset_label: Option<String>,
    /// Workspace-relative path of the resolved config file.
    #[serde(default)]
    pub config_path: Option<String>,
    /// Workspace-relative path of the original plan file, if attached.
    #[serde(default)]
    pub plan_path: Option<String>,
    /// Git HEAD at the time the run started (resolved by the loop
    /// runner, not the early collector).
    #[serde(default)]
    pub baseline_sha: Option<String>,
    /// Whether the run is bound to a supervisor / wave topology,
    /// as observed by the run-loop entry. `single-chain` means no
    /// supervisor / wave evidence was seen yet.
    #[serde(default)]
    pub execution_capability: Option<String>,
}

/// A reference to the run's code/config baseline plus capability.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CodeBaseline {
    /// Git HEAD at the time the manifest was last updated.
    #[serde(default)]
    pub head_sha: Option<String>,
    /// Whether the run is running in a worktree.
    #[serde(default)]
    pub worktree: bool,
    /// Workspace-relative path to the worktree root, if any.
    #[serde(default)]
    pub worktree_path: Option<String>,
}

/// Schema version for the manifest format. Bump only on breaking
/// changes (e.g. dropping a field). Additive changes keep the
/// version and rely on `Option` defaults to keep old consumers
/// working.
pub const DIAGNOSIS_INPUT_SCHEMA_VERSION: &str = "run-diagnosis-input/v1";

/// Self-describing manifest written to
/// `<session_dir>/diagnosis-input.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiagnosisInputBundle {
    pub schema_version: String,
    pub manifest_status: ManifestStatus,
    #[serde(default)]
    pub run: RunMetadata,
    #[serde(default)]
    pub code_baseline: CodeBaseline,
    /// Execution capability tags observed at run-loop entry.
    /// Always an array (possibly empty) so consumers can iterate
    /// without checking for `null`.
    #[serde(default)]
    pub execution_capabilities: Vec<String>,
    /// Per-artifact integrity and status.
    #[serde(default)]
    pub artifacts: Vec<ArtifactIntegrity>,
    /// UTC RFC 3339 timestamp the manifest was first created.
    pub created_at: String,
    /// UTC RFC 3339 timestamp the manifest was last updated.
    pub updated_at: String,
    /// When true, the manifest could not be written to disk
    /// (target path blocked, permissions, etc.). The reporter
    /// must surface this as a `missing` bundle.
    #[serde(default)]
    pub write_blocked: bool,
}

impl DiagnosisInputBundle {
    /// Builds a new pending manifest. `session_id` is taken from
    /// the session directory name; the rest is left as `None` and
    /// must be filled in by [`Self::with_completed_identity`] or
    /// [`Self::with_finalized`] before the run terminates.
    pub fn new_pending(session_dir: &Path) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        let session_id = session_dir
            .file_name()
            .and_then(|n| n.to_str())
            .map(|s| s.to_string());
        Self {
            schema_version: DIAGNOSIS_INPUT_SCHEMA_VERSION.to_string(),
            manifest_status: ManifestStatus::Pending,
            run: RunMetadata {
                session_id,
                ..RunMetadata::default()
            },
            code_baseline: CodeBaseline::default(),
            execution_capabilities: Vec::new(),
            artifacts: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
            write_blocked: false,
        }
    }

    /// Apply the run identity resolved by the run-loop entry.
    /// Returns a new bundle; the caller decides whether to
    /// persist it. Used to transition `Pending → Present`.
    pub fn with_completed_identity(
        &self,
        loop_id: Option<String>,
        preset_label: Option<String>,
        config_path: Option<String>,
        plan_path: Option<String>,
        baseline_sha: Option<String>,
        execution_capability: Option<String>,
        code_baseline: CodeBaseline,
    ) -> Self {
        let mut next = self.clone();
        next.manifest_status = ManifestStatus::Present;
        if loop_id.is_some() {
            next.run.loop_id = loop_id;
        }
        if preset_label.is_some() {
            next.run.preset_label = preset_label;
        }
        if config_path.is_some() {
            next.run.config_path = config_path;
        }
        if plan_path.is_some() {
            next.run.plan_path = plan_path;
        }
        if baseline_sha.is_some() {
            next.run.baseline_sha = baseline_sha;
        }
        if execution_capability.is_some() {
            next.run.execution_capability = execution_capability;
        }
        next.code_baseline = code_baseline;
        next.updated_at = chrono::Utc::now().to_rfc3339();
        next
    }

    /// Transition `Present → Finalized`. The caller passes the
    /// final artifact integrity list and execution capability
    /// tags. The returned bundle is what the reporter reads.
    pub fn with_finalized(
        &self,
        artifacts: Vec<ArtifactIntegrity>,
        execution_capabilities: Vec<String>,
    ) -> Self {
        let mut next = self.clone();
        next.manifest_status = ManifestStatus::Finalized;
        next.artifacts = artifacts;
        next.execution_capabilities = execution_capabilities;
        next.updated_at = chrono::Utc::now().to_rfc3339();
        next
    }

    /// Mark the manifest as degraded (an in-flight update failed)
    /// without dropping the existing fields. The reporter will
    /// surface this as an evidence gap, not as a successful bundle.
    pub fn mark_degraded(&self) -> Self {
        let mut next = self.clone();
        next.manifest_status = ManifestStatus::Degraded;
        next.updated_at = chrono::Utc::now().to_rfc3339();
        next
    }
}

/// Computes the canonical manifest path inside `session_dir`.
pub fn manifest_path(session_dir: &Path) -> PathBuf {
    session_dir.join("diagnosis-input.json")
}

/// Atomically writes the manifest to `<session_dir>/diagnosis-input.json`.
///
/// Returns the final on-disk path on success. The write uses
/// `NamedTempFile::persist` so a half-written manifest is never
/// visible. If the target path is unwritable the function returns
/// `Ok(None)` after emitting a `tracing::warn!`; callers can decide
/// how to react (e.g. mark the bundle as missing).
pub fn write_manifest(
    session_dir: &Path,
    bundle: &DiagnosisInputBundle,
) -> std::io::Result<Option<PathBuf>> {
    if !probe_session_dir_writable(session_dir) {
        tracing::warn!(
            target: "ralph_core::diagnostics",
            session_dir = %session_dir.display(),
            "diagnosis-input.json target is not writable; manifest will be missing"
        );
        return Ok(None);
    }
    let path = manifest_path(session_dir);
    let body = match serde_json::to_vec_pretty(bundle) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(
                target: "ralph_core::diagnostics",
                error = %err,
                "failed to serialize diagnosis-input.json; manifest dropped"
            );
            return Ok(None);
        }
    };
    let mut tmp = match NamedTempFile::new_in(session_dir) {
        Ok(t) => t,
        Err(err) => {
            tracing::warn!(
                target: "ralph_core::diagnostics",
                error = %err,
                "failed to create diagnosis-input.json temp file; manifest dropped"
            );
            return Ok(None);
        }
    };
    if let Err(err) = std::io::Write::write_all(&mut tmp, &body) {
        tracing::warn!(
            target: "ralph_core::diagnostics",
            error = %err,
            "failed to write diagnosis-input.json temp file; manifest dropped"
        );
        return Ok(None);
    }
    if let Err(err) = tmp.persist(&path) {
        tracing::warn!(
            target: "ralph_core::diagnostics",
            error = %err,
            "failed to atomically replace diagnosis-input.json; manifest dropped"
        );
        return Ok(None);
    }
    Ok(Some(path))
}

/// Reads and deserializes a manifest from disk, returning `None`
/// if the file is missing or malformed.
///
/// **Schema-version policy (plan 2026-08-12-001 fix-plan U2 / synth:P0-2):**
/// when the parsed bundle's `schema_version` differs from
/// [`DIAGNOSIS_INPUT_SCHEMA_VERSION`], the function **still**
/// returns the bundle — the on-disk bytes are authoritative
/// and the reporter must distinguish "wrong version" from
/// "session predates this format". The reporter projection in
/// [`crate::diagnosis::bundle::project_bundle`] maps the
/// version mismatch to `BundleStatus::SchemaMismatch { on_disk_version,
/// reader_version }` so the report can surface the gap instead of
/// silently demoting the bundle to `Legacy`.
pub fn read_manifest(session_dir: &Path) -> Option<DiagnosisInputBundle> {
    let path = manifest_path(session_dir);
    let body = match fs::read(&path) {
        Ok(b) => b,
        Err(_) => return None,
    };
    let bundle: DiagnosisInputBundle = match serde_json::from_slice(&body) {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!(
                target: "ralph_core::diagnostics",
                path = %path.display(),
                error = %err,
                "diagnosis-input.json exists but is malformed; ignoring"
            );
            return None;
        }
    };
    if bundle.schema_version != DIAGNOSIS_INPUT_SCHEMA_VERSION {
        // Per fix-plan U2 / synth:P0-2: do NOT swallow the bundle.
        // The on-disk format is authoritative; the reader is just
        // one version ahead/behind. Surface the gap to the
        // reporter instead of mapping to Legacy.
        tracing::warn!(
            target: "ralph_core::diagnostics",
            schema = %bundle.schema_version,
            expected = %DIAGNOSIS_INPUT_SCHEMA_VERSION,
            "diagnosis-input.json schema version mismatch; reporting as SchemaMismatch"
        );
    }
    Some(bundle)
}
