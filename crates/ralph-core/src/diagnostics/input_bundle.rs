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
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestStatus {
    /// Initial state: collector created the file but `run_id`,
    /// `preset_label`, etc. have not been resolved yet (D11).
    #[default]
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

/// Status of a single artifact slot in the manifest. Independent of
/// the manifest's own [`ManifestStatus`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ArtifactStatus {
    Present,
    Missing,
    Degraded,
    #[default]
    NotApplicable,
    Legacy,
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
///
/// Plan 2026-08-26-1104 Unit 7 (U07): v2 introduces the
/// `boundary_coverage` segment listing the eight causal
/// boundaries (`effective_contract` / `activation` /
/// `backend_outcome` / `event_candidate` / `policy_decision` /
/// `state_commit` / `recovery_action` / `termination`) together
/// with `expected` / `recorded` counters and a per-row `status`.
/// The on-disk format is authoritative; v1 readers must surface
/// the absence as `Legacy` and never invent boundary rows.
pub const DIAGNOSIS_INPUT_SCHEMA_VERSION: &str = "run-diagnosis-input/v2";

// ============================================================================
// Plan 2026-08-26-1104 Unit 7 (U07): 8-boundary coverage manifest v2.
// The boundary table below is the single source of truth for the
// categorization: every receipt emitter that lands a row in
// `runtime-trace.jsonl` must map to exactly one boundary here.
// Adding a ninth category is a breaking change to the manifest
// schema and must go through the same plan-driven review as adding
// a new receipt kind.
// ============================================================================

/// The eight causal boundaries the diagnostics pipeline must
/// produce evidence for, per U07 §6 / development-plan §6.
///
/// The order matters: `CausalBoundary::ALL` is the canonical
/// iteration order for serializing the manifest, and the variant
/// discriminant (plus the `snake_case` rename) defines the
/// `kind -> boundary` mapping consumed by
/// [`crate::diagnostics::DiagnosticsCollector`].
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize,
)]
#[serde(rename_all = "snake_case")]
pub enum CausalBoundary {
    /// `kind=contract_receipt` — the bootstrap effective contract
    /// (`emit_contract_receipt`, U2).
    #[default]
    EffectiveContract,
    /// `kind=activation` — hat activation events recorded by the
    /// loop runner.
    Activation,
    /// `kind=hat_activation_outcome` — backend hat activation
    /// outcomes (success / failure / merge_failed / unreadable).
    BackendOutcome,
    /// `kind=event_batch_accepted` — the event-batch receipt that
    /// names the topics accepted by the unified validation
    /// pipeline per iteration.
    EventCandidate,
    /// `kind=policy_receipt` — per-event accept/reject decisions
    /// (`emit_policy_receipt`, U3).
    PolicyDecision,
    /// `kind=commit_receipt` — StateMachine projection commit /
    /// rollback confirmations (`emit_commit_receipt`, U4).
    StateCommit,
    /// `kind=recovery_receipt` — recovery decision receipts
    /// (`emit_recovery_receipt`, U5).
    RecoveryAction,
    /// `kind=termination` — final termination event recorded by
    /// the loop runner.
    Termination,
}

impl CausalBoundary {
    /// All eight boundaries in canonical iteration order.
    pub const ALL: [CausalBoundary; 8] = [
        CausalBoundary::EffectiveContract,
        CausalBoundary::Activation,
        CausalBoundary::BackendOutcome,
        CausalBoundary::EventCandidate,
        CausalBoundary::PolicyDecision,
        CausalBoundary::StateCommit,
        CausalBoundary::RecoveryAction,
        CausalBoundary::Termination,
    ];

    /// Stable `snake_case` identifier; matches the on-disk rename
    /// and the `kind` prefix used by the receipt emitters.
    pub fn as_str(self) -> &'static str {
        match self {
            CausalBoundary::EffectiveContract => "effective_contract",
            CausalBoundary::Activation => "activation",
            CausalBoundary::BackendOutcome => "backend_outcome",
            CausalBoundary::EventCandidate => "event_candidate",
            CausalBoundary::PolicyDecision => "policy_decision",
            CausalBoundary::StateCommit => "state_commit",
            CausalBoundary::RecoveryAction => "recovery_action",
            CausalBoundary::Termination => "termination",
        }
    }

    /// Iterator over all eight boundaries in canonical order.
    pub fn all() -> impl ExactSizeIterator<Item = CausalBoundary> {
        Self::ALL.into_iter()
    }
}

/// Map a `RuntimeTraceEntry::kind` value to its causal boundary.
///
/// Receipt kinds currently produced by the diagnostics pipeline
/// (`contract_receipt` / `policy_receipt` / `commit_receipt` /
/// `recovery_receipt`) and the four coverage-only categories
/// (`activation` / `hat_activation_outcome` / `event_batch_accepted`
/// / `termination`) are recognized. Anything else returns `None`
/// so the counter path stays a no-op for unrelated trace rows
/// (`log_runtime_trace` is also used by orchestration /
/// performance / hook runs loggers that are NOT boundary rows).
pub fn kind_to_boundary(kind: &str) -> Option<CausalBoundary> {
    match kind {
        "contract_receipt" => Some(CausalBoundary::EffectiveContract),
        "activation" => Some(CausalBoundary::Activation),
        "hat_activation_outcome" => Some(CausalBoundary::BackendOutcome),
        "event_batch_accepted" => Some(CausalBoundary::EventCandidate),
        "policy_receipt" => Some(CausalBoundary::PolicyDecision),
        "commit_receipt" => Some(CausalBoundary::StateCommit),
        "recovery_receipt" => Some(CausalBoundary::RecoveryAction),
        "termination" => Some(CausalBoundary::Termination),
        _ => None,
    }
}

/// Counter pair for one boundary. `expected` is bumped at the
/// entry of the recording method; `recorded` is bumped after the
/// row is successfully appended to `runtime-trace.jsonl`. When
/// the underlying logger is disabled or degraded, `expected`
/// still bumps but `recorded` stops increasing, which surfaces
/// as a `BoundaryCoverageStatus::Gap` in the manifest.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundaryCounter {
    #[serde(default)]
    pub expected: u64,
    #[serde(default)]
    pub recorded: u64,
}

impl BoundaryCounter {
    /// Returns `true` when the boundary was attempted at least
    /// once and every attempt was recorded successfully.
    pub fn is_covered(&self) -> bool {
        self.expected == self.recorded
    }
}

/// Per-row status of the boundary coverage.
///
/// `Covered` means `expected == recorded` (zero attempts count
/// as covered so a session with no events still serializes eight
/// covered rows; the reporter can detect "no events" by looking
/// at `expected == 0 && recorded == 0` rather than absence).
/// `Gap` means `expected > recorded`; the row also carries a
/// reason describing the underlying writer failure.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BoundaryCoverageStatus {
    #[default]
    Covered,
    Gap,
}

/// One row of the `boundary_coverage` manifest segment. Always
/// emitted (the eight rows are unconditional; the receiver can
/// always iterate without null checks).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct BoundaryCoverageEntry {
    pub boundary: CausalBoundary,
    #[serde(default)]
    pub expected: u64,
    #[serde(default)]
    pub recorded: u64,
    #[serde(default)]
    pub status: BoundaryCoverageStatus,
    /// Populated only when `status == Gap`; explains the
    /// underlying writer failure (e.g. "logger write failed",
    /// "commit_receipt missing"). `None` for covered rows so the
    /// serialized form stays compact.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

impl BoundaryCoverageEntry {
    /// Build an entry from a snapshot counter. `reason` is the
    /// precomputed reason string for `Gap` rows; pass `None` when
    /// the caller has no specific failure to surface.
    pub fn new(
        boundary: CausalBoundary,
        counter: &BoundaryCounter,
        reason: Option<String>,
    ) -> Self {
        let status = if counter.is_covered() {
            BoundaryCoverageStatus::Covered
        } else {
            BoundaryCoverageStatus::Gap
        };
        // Drop the reason on covered rows to keep the serialized
        // form compact (skip_serializing_if on the field handles
        // `None`, but we also normalize empty strings to `None`
        // for symmetry with callers that pass `Some("".into())`).
        let reason = if status == BoundaryCoverageStatus::Gap {
            reason.filter(|s| !s.is_empty())
        } else {
            None
        };
        Self {
            boundary,
            expected: counter.expected,
            recorded: counter.recorded,
            status,
            reason,
        }
    }
}

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
    /// Plan 2026-08-26-1104 U07: per-boundary coverage evidence.
    /// Always serialized as a (possibly empty) array so legacy
    /// v1 manifests that omit the field deserialize as `[]`. The
    /// reader distinguishes "v1 / legacy" by `schema_version`,
    /// not by the presence of the array — the absence in v1 is
    /// normal, but the array IS populated for v2 even when the
    /// producer never fired any boundary (the eight canonical
    /// rows are always present with `expected=0, recorded=0`).
    #[serde(default)]
    pub boundary_coverage: Vec<BoundaryCoverageEntry>,
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
            boundary_coverage: Vec::new(),
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
    /// final artifact integrity list, execution capability
    /// tags, and the snapshot of boundary coverage counters.
    /// The returned bundle is what the reporter reads.
    ///
    /// `boundary_coverage` is the snapshot taken by
    /// [`crate::diagnostics::DiagnosticsCollector::snapshot_boundary_coverage`]
    /// (or, in tests, a hand-built vector). When the underlying
    /// logger was degraded mid-run, the caller should have
    /// populated the `reason` field on every `Gap` row before
    /// passing the vector here.
    pub fn with_finalized(
        &self,
        artifacts: Vec<ArtifactIntegrity>,
        execution_capabilities: Vec<String>,
        boundary_coverage: Vec<BoundaryCoverageEntry>,
    ) -> Self {
        let mut next = self.clone();
        next.manifest_status = ManifestStatus::Finalized;
        next.artifacts = artifacts;
        next.execution_capabilities = execution_capabilities;
        next.boundary_coverage = boundary_coverage;
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
