//! U1 (plan 2026-08-03-004): Parallel Forge resume manifest.
//!
//! When `ralph run --worktree --reuse-worktree` reuses a completed
//! worktree, the reuse cleanup archives (and thereby destroys the live
//! view of) the previous run's runtime state. Before that happens, a
//! structured `parallel-forge-resume-manifest.v1` is captured from the
//! OLD live runtime files:
//!
//! - `.ralph/events.jsonl`, `.ralph/events-*.jsonl`,
//!   `.ralph/agent/events-*.jsonl` (hat channels) — event evidence.
//! - `.ralph/agent/accepted-transitions.jsonl` — the durable outbox:
//!   the ONLY authority for which business events were *accepted*.
//!   Artifact files on disk alone never prove completion (S5).
//! - `.ralph/agent/tasks.jsonl` — task/unit mapping snapshot.
//! - `.ralph/current-loop-id` — prior loop identity.
//!
//! The manifest is written into the reuse archive directory. Before the
//! new run starts its loop it must validate the manifest:
//! schema version, self-digest (tamper detection), bounded artifact
//! paths, completeness, and identity drift against the current
//! plan/preset/config/worktree. Any failure is fail-closed: the loop
//! must not start.
//!
//! # Boundary model
//!
//! Only entries in the accepted-transitions outbox count as hat
//! completion boundaries. The last accepted boundary's corresponding
//! event in the event log identifies the pending hat (`triggered`) and
//! carries the original trigger snapshot. When the boundary cannot be
//! determined uniquely (missing outbox, malformed log, boundary event
//! absent from the log), the manifest records the reason and marks
//! itself incomplete — validation then refuses the start.
//!
//! # Identity chain
//!
//! The first manifest for a worktree records the identity inputs of the
//! run that performs the first reuse (there is no deeper provenance for
//! plan/preset/config in the old runtime files). Later manifests inherit
//! those drift-checked identity fields from the latest prior manifest,
//! so a plan/config/preset change between runs is detected as identity
//! drift (S6). `loop_id` and `source_head_sha` are provenance fields
//! captured fresh at every reuse and are NOT part of drift checking.

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};

/// Schema version stamped into every manifest.
pub const MANIFEST_SCHEMA_VERSION: &str = "parallel-forge-resume-manifest.v1";

/// Manifest file name inside a reuse archive directory.
pub const MANIFEST_FILE_NAME: &str = "parallel-forge-resume-manifest.v1.json";

/// Byte cap for the original trigger payload snapshot stored in the
/// manifest. Payloads beyond this are truncated (marked with `…`).
pub const MAX_TRIGGER_PAYLOAD_SNAPSHOT_BYTES: usize = 8 * 1024;

/// Byte cap for a single artifact file we are willing to digest.
pub const MAX_ARTIFACT_DIGEST_BYTES: u64 = 8 * 1024 * 1024;

/// The structured resume manifest (versioned JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumeManifest {
    /// Always [`MANIFEST_SCHEMA_VERSION`].
    pub schema_version: String,
    /// RFC3339 timestamp of capture.
    pub captured_at: String,
    /// Identity binding (drift-checked fields + provenance).
    pub identity: ResumeIdentity,
    /// Accepted terminal boundary + pending hat determination.
    pub boundary: BoundaryRecord,
    /// Task/unit mapping snapshot from the old run's task ledger.
    pub tasks: Vec<TaskMappingEntry>,
    /// Referenced artifact files (worktree-relative path + digest).
    pub artifacts: Vec<ArtifactRef>,
    /// Why this manifest cannot support a resume. Empty = complete.
    pub incomplete_reasons: Vec<String>,
    /// SHA-256 over the canonical serialization of this manifest with
    /// the digest field itself emptied. Tamper detection.
    pub manifest_digest: String,
}

/// Identity binding for a resume manifest.
///
/// `plan_path`, `plan_digest`, `preset_name`, `config_digest`, and
/// `worktree_name` are drift-checked at validation time. `loop_id` and
/// `source_head_sha` are provenance recorded at capture time and are
/// never compared.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ResumeIdentity {
    /// Plan path as supplied by the operator (`""` when none).
    pub plan_path: String,
    /// SHA-256 over the plan file bytes (`""` when no plan).
    pub plan_digest: String,
    /// Active preset name (`""` when no hats source).
    pub preset_name: String,
    /// SHA-256 over the config file bytes (`""` when file-less).
    pub config_digest: String,
    /// Exact worktree name the resume is bound to.
    pub worktree_name: String,
    /// Git HEAD of the worktree at capture time (provenance only).
    pub source_head_sha: String,
    /// The old run's loop id from `.ralph/current-loop-id`
    /// (provenance only).
    pub loop_id: String,
}

/// Accepted terminal boundary + pending hat determination.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BoundaryRecord {
    /// Accepted terminal events in commit order (outbox order).
    pub accepted: Vec<AcceptedBoundary>,
    /// The hat that must activate next (from the last boundary event's
    /// `triggered` field), when determinable.
    pub pending_hat: Option<String>,
    /// Snapshot of the event that triggers the pending hat.
    pub original_trigger: Option<TriggerSnapshot>,
    /// Wave correlation metadata of the last boundary event, if any.
    pub wave: Option<WaveMetadata>,
}

/// One accepted terminal boundary entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AcceptedBoundary {
    /// Event topic accepted as a terminal transition.
    pub topic: String,
    /// Outbox `transition_id`.
    pub transition_id: String,
    /// Outbox commit timestamp.
    pub committed_at: String,
    /// Publishing hat, when the matching event was found in the log.
    pub hat: Option<String>,
    /// Whether the matching event was found in the event log.
    pub in_event_log: bool,
}

/// Snapshot of the original trigger event for the pending hat.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TriggerSnapshot {
    pub topic: String,
    /// Payload snapshot (truncated to
    /// [`MAX_TRIGGER_PAYLOAD_SNAPSHOT_BYTES`]).
    pub payload: Option<String>,
    pub hat: Option<String>,
    pub triggered: Option<String>,
    pub ts: String,
}

/// Wave correlation metadata.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WaveMetadata {
    pub wave_id: String,
    pub wave_index: u32,
    pub wave_total: u32,
}

/// One task ledger entry snapshot.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TaskMappingEntry {
    /// Stable task key (falls back to the task id).
    pub task_key: String,
    /// Task status string as recorded in the ledger.
    pub status: String,
}

/// A referenced artifact file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ArtifactRef {
    /// Worktree-relative path (bounded: no `..`, not absolute).
    pub path: String,
    /// SHA-256 over the artifact bytes at capture time.
    pub digest: String,
}

/// Operator/run identity inputs captured at reuse time. The same struct
/// is re-supplied at validation time as the expected identity.
#[derive(Debug, Clone, PartialEq)]
pub struct CaptureInputs {
    pub plan_path: String,
    pub plan_digest: String,
    pub preset_name: String,
    pub config_digest: String,
    pub worktree_name: String,
}

/// Errors from manifest read / validation.
#[derive(Debug, Clone, PartialEq)]
pub enum ResumeManifestError {
    /// The manifest file could not be read.
    Io { path: String, source: String },
    /// The manifest file is not valid JSON / wrong shape.
    Parse { path: String, source: String },
    /// Schema version mismatch.
    SchemaVersion { found: String },
    /// The manifest's self-digest does not match its content.
    DigestMismatch { recorded: String, computed: String },
    /// The manifest records incomplete boundary evidence.
    Incomplete { reasons: Vec<String> },
    /// The manifest identity drifted from the current run inputs.
    IdentityDrift { fields: Vec<String> },
    /// A manifest path escapes the worktree.
    UnboundedPath { path: String },
}

impl std::fmt::Display for ResumeManifestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(f, "cannot read resume manifest {path}: {source}")
            }
            Self::Parse { path, source } => {
                write!(f, "resume manifest {path} is not valid JSON: {source}")
            }
            Self::SchemaVersion { found } => {
                write!(
                    f,
                    "resume manifest schema version mismatch: found '{found}', \
                     expected '{MANIFEST_SCHEMA_VERSION}'"
                )
            }
            Self::DigestMismatch { recorded, computed } => {
                write!(
                    f,
                    "resume manifest digest mismatch (tamper detected): \
                     recorded {recorded}, computed {computed}"
                )
            }
            Self::Incomplete { reasons } => {
                write!(f, "resume manifest is incomplete: {}", reasons.join("; "))
            }
            Self::IdentityDrift { fields } => {
                write!(
                    f,
                    "resume manifest identity drift in field(s): {}",
                    fields.join(", ")
                )
            }
            Self::UnboundedPath { path } => {
                write!(f, "resume manifest path escapes the worktree: {path}")
            }
        }
    }
}

impl std::error::Error for ResumeManifestError {}

impl ResumeManifest {
    /// Canonical serialization used for the self-digest: the manifest
    /// with `manifest_digest` emptied, serialized in declaration order.
    pub fn canonical_bytes(&self) -> Vec<u8> {
        let mut canonical = self.clone();
        canonical.manifest_digest.clear();
        serde_json::to_vec(&canonical).expect("ResumeManifest serializes")
    }

    /// SHA-256 over [`Self::canonical_bytes`].
    pub fn compute_digest(&self) -> String {
        sha256_hex(&self.canonical_bytes())
    }

    /// Set `manifest_digest` to [`Self::compute_digest`].
    pub fn finalize_digest(&mut self) {
        self.manifest_digest = self.compute_digest();
    }

    /// True when no incompleteness reasons are recorded.
    pub fn is_complete(&self) -> bool {
        self.incomplete_reasons.is_empty()
    }
}

/// SHA-256 hex digest helper shared with the CLI.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// SHA-256 over the drift-checked identity fields (length-prefixed
/// framing so distinct field tuples cannot collide). Provenance fields
/// (`loop_id`, `source_head_sha`) do not participate.
pub fn identity_digest(identity: &ResumeIdentity) -> String {
    let mut hasher = Sha256::new();
    for field in [
        &identity.plan_path,
        &identity.plan_digest,
        &identity.preset_name,
        &identity.config_digest,
        &identity.worktree_name,
    ] {
        hasher.update((field.len() as u64).to_le_bytes());
        hasher.update(field.as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// Validate that a manifest path is worktree-relative and bounded:
/// non-empty, not absolute, no `..` component.
pub fn validate_bounded_path(raw: &str) -> Result<(), ResumeManifestError> {
    let reject = || ResumeManifestError::UnboundedPath {
        path: raw.to_string(),
    };
    if raw.trim().is_empty() {
        return Err(reject());
    }
    let path = Path::new(raw);
    if path.is_absolute() {
        return Err(reject());
    }
    if path.components().any(|c| {
        matches!(
            c,
            Component::ParentDir | Component::RootDir | Component::Prefix(_)
        )
    }) {
        return Err(reject());
    }
    Ok(())
}

/// Capture a resume manifest from the OLD live runtime state inside
/// `worktree_path`. Must run BEFORE the reuse cleanup moves or deletes
/// any live file. Capture never fails; untrustworthy evidence is
/// recorded in `incomplete_reasons` instead.
pub fn capture_manifest(worktree_path: &Path, inputs: &CaptureInputs) -> ResumeManifest {
    let mut reasons: Vec<String> = Vec::new();
    let ralph_dir = worktree_path.join(".ralph");
    let agent_dir = ralph_dir.join("agent");

    let loop_id = std::fs::read_to_string(ralph_dir.join("current-loop-id"))
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let source_head_sha = read_head_sha(worktree_path);

    // Identity: baseline from the current inputs, chained from the
    // latest prior manifest when one exists (S6 drift detection).
    let mut identity = ResumeIdentity {
        plan_path: inputs.plan_path.clone(),
        plan_digest: inputs.plan_digest.clone(),
        preset_name: inputs.preset_name.clone(),
        config_digest: inputs.config_digest.clone(),
        worktree_name: inputs.worktree_name.clone(),
        source_head_sha,
        loop_id,
    };
    match latest_archived_manifest(worktree_path) {
        Ok(Some((_, prior))) => {
            if prior.compute_digest() != prior.manifest_digest {
                reasons.push(
                    "prior resume manifest digest mismatch; identity chain broken".to_string(),
                );
            } else {
                identity.plan_path.clone_from(&prior.identity.plan_path);
                identity.plan_digest.clone_from(&prior.identity.plan_digest);
                identity.preset_name.clone_from(&prior.identity.preset_name);
                identity
                    .config_digest
                    .clone_from(&prior.identity.config_digest);
                identity
                    .worktree_name
                    .clone_from(&prior.identity.worktree_name);
            }
        }
        Ok(None) => {}
        Err(e) => reasons.push(format!("prior resume manifest unreadable: {e}")),
    }

    // Event evidence: main log + hat channels.
    let mut events: Vec<crate::Event> = Vec::new();
    for path in event_log_paths(&ralph_dir, &agent_dir) {
        let rel = display_relative(&path, worktree_path);
        match std::fs::read_to_string(&path) {
            Ok(body) => {
                let mut malformed = 0usize;
                for line in body.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str::<crate::Event>(trimmed) {
                        Ok(event) => events.push(event),
                        Err(_) => malformed += 1,
                    }
                }
                if malformed > 0 {
                    reasons.push(format!("event log {rel} has {malformed} malformed line(s)"));
                }
            }
            Err(e) => reasons.push(format!("event log {rel} unreadable: {e}")),
        }
    }

    // Accepted transitions outbox — the ONLY acceptance authority.
    // Parsed strictly (no salvage): a torn line means the acceptance
    // record cannot be trusted for a resume boundary.
    let mut outbox: Vec<crate::event_loop::accepted_transition::OutboxEntry> = Vec::new();
    let outbox_path = agent_dir.join("accepted-transitions.jsonl");
    if outbox_path.exists() {
        match std::fs::read_to_string(&outbox_path) {
            Ok(body) => {
                let mut malformed = 0usize;
                for line in body.lines() {
                    let trimmed = line.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    match serde_json::from_str(trimmed) {
                        Ok(entry) => outbox.push(entry),
                        Err(_) => malformed += 1,
                    }
                }
                if malformed > 0 {
                    reasons.push(format!(
                        "accepted-transitions outbox has {malformed} malformed line(s)"
                    ));
                }
            }
            Err(e) => reasons.push(format!("accepted-transitions outbox unreadable: {e}")),
        }
    }

    // Boundary determination: outbox order is authoritative. Each entry
    // is cross-referenced against the event log by topic + payload
    // digest for the hat / trigger snapshot.
    let mut accepted: Vec<AcceptedBoundary> = Vec::new();
    let mut matched_events: Vec<crate::Event> = Vec::new();
    for entry in &outbox {
        let matched = events.iter().find(|event| {
            event.topic == entry.topic
                && sha256_hex(event.payload.as_deref().unwrap_or_default().as_bytes())
                    == entry.payload_digest
        });
        if let Some(event) = matched {
            matched_events.push(event.clone());
        }
        accepted.push(AcceptedBoundary {
            topic: entry.topic.clone(),
            transition_id: entry.transition_id.clone(),
            committed_at: entry.committed_at.clone(),
            hat: matched.and_then(|event| event.hat.clone()),
            in_event_log: matched.is_some(),
        });
    }

    let mut pending_hat: Option<String> = None;
    let mut original_trigger: Option<TriggerSnapshot> = None;
    let mut wave: Option<WaveMetadata> = None;
    if outbox.is_empty() {
        if events.is_empty() {
            reasons.push(
                "no accepted terminal boundary: no accepted transitions recorded".to_string(),
            );
        } else {
            reasons.push(
                "no accepted terminal boundary: event log has events but the \
                 accepted-transitions outbox is empty"
                    .to_string(),
            );
        }
    } else {
        let last_in_log = accepted.last().is_some_and(|last| last.in_event_log);
        match matched_events.last() {
            Some(event) if last_in_log => {
                if event.triggered.is_none() {
                    reasons
                        .push("last accepted boundary event carries no triggered hat".to_string());
                }
                pending_hat.clone_from(&event.triggered);
                original_trigger = Some(TriggerSnapshot {
                    topic: event.topic.clone(),
                    payload: event.payload.clone().map(truncate_payload_snapshot),
                    hat: event.hat.clone(),
                    triggered: event.triggered.clone(),
                    ts: event.ts.clone(),
                });
                if let (Some(wave_id), Some(wave_index), Some(wave_total)) =
                    (&event.wave_id, event.wave_index, event.wave_total)
                {
                    wave = Some(WaveMetadata {
                        wave_id: wave_id.clone(),
                        wave_index,
                        wave_total,
                    });
                }
            }
            _ => reasons.push("last accepted boundary event not found in event log".to_string()),
        }
    }

    // Task/unit mapping snapshot from the old run's task ledger.
    let tasks = capture_tasks(&agent_dir.join("tasks.jsonl"), &mut reasons);

    // Artifact references: `*_path` string fields in the payloads of
    // accepted boundary events, bounded and digested.
    let artifacts = capture_artifacts(worktree_path, &matched_events, &mut reasons);

    let mut manifest = ResumeManifest {
        schema_version: MANIFEST_SCHEMA_VERSION.to_string(),
        captured_at: chrono::Utc::now().to_rfc3339(),
        identity,
        boundary: BoundaryRecord {
            accepted,
            pending_hat,
            original_trigger,
            wave,
        },
        tasks,
        artifacts,
        incomplete_reasons: reasons,
        manifest_digest: String::new(),
    };
    manifest.finalize_digest();
    manifest
}

/// Serialize the manifest into `archive_dir`. Returns the written path.
pub fn write_manifest(manifest: &ResumeManifest, archive_dir: &Path) -> std::io::Result<PathBuf> {
    let path = archive_dir.join(MANIFEST_FILE_NAME);
    let json = serde_json::to_string_pretty(manifest).map_err(|e| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("failed to serialize resume manifest: {e}"),
        )
    })?;
    std::fs::write(&path, json)?;
    Ok(path)
}

/// Read and parse a manifest file (schema-version check included; the
/// self-digest / completeness / identity gates are `validate_manifest`).
///
/// The schema version is checked on the raw JSON value first, so a
/// foreign-version document yields [`ResumeManifestError::SchemaVersion`]
/// even when its shape does not match this version's struct.
pub fn read_manifest(path: &Path) -> Result<ResumeManifest, ResumeManifestError> {
    let display = path.display().to_string();
    let body = std::fs::read_to_string(path).map_err(|e| ResumeManifestError::Io {
        path: display.clone(),
        source: e.to_string(),
    })?;
    let value: serde_json::Value =
        serde_json::from_str(&body).map_err(|e| ResumeManifestError::Parse {
            path: display.clone(),
            source: e.to_string(),
        })?;
    let found = value
        .get("schema_version")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();
    if found != MANIFEST_SCHEMA_VERSION {
        return Err(ResumeManifestError::SchemaVersion { found });
    }
    serde_json::from_value(value).map_err(|e| ResumeManifestError::Parse {
        path: display,
        source: e.to_string(),
    })
}

/// Full validation gate: schema version, self-digest, bounded paths,
/// completeness, identity drift. Any failure means the loop must not
/// start.
pub fn validate_manifest(
    manifest: &ResumeManifest,
    expected: &CaptureInputs,
) -> Result<(), ResumeManifestError> {
    if manifest.schema_version != MANIFEST_SCHEMA_VERSION {
        return Err(ResumeManifestError::SchemaVersion {
            found: manifest.schema_version.clone(),
        });
    }
    let computed = manifest.compute_digest();
    if computed != manifest.manifest_digest {
        return Err(ResumeManifestError::DigestMismatch {
            recorded: manifest.manifest_digest.clone(),
            computed,
        });
    }
    for artifact in &manifest.artifacts {
        validate_bounded_path(&artifact.path)?;
    }
    if !manifest.is_complete() {
        return Err(ResumeManifestError::Incomplete {
            reasons: manifest.incomplete_reasons.clone(),
        });
    }
    let drifted: Vec<String> = [
        (
            "plan_path",
            manifest.identity.plan_path.as_str(),
            expected.plan_path.as_str(),
        ),
        (
            "plan_digest",
            manifest.identity.plan_digest.as_str(),
            expected.plan_digest.as_str(),
        ),
        (
            "preset_name",
            manifest.identity.preset_name.as_str(),
            expected.preset_name.as_str(),
        ),
        (
            "config_digest",
            manifest.identity.config_digest.as_str(),
            expected.config_digest.as_str(),
        ),
        (
            "worktree_name",
            manifest.identity.worktree_name.as_str(),
            expected.worktree_name.as_str(),
        ),
    ]
    .into_iter()
    .filter(|(_, recorded, current)| recorded != current)
    .map(|(name, _, _)| name.to_string())
    .collect();
    if !drifted.is_empty() {
        return Err(ResumeManifestError::IdentityDrift { fields: drifted });
    }
    Ok(())
}

/// Path of the newest manifest found among `.ralph/reuse-history/`
/// archives (newest archive directory first), if any.
pub fn latest_archived_manifest_path(worktree_path: &Path) -> Option<PathBuf> {
    let history = worktree_path.join(".ralph/reuse-history");
    let mut dirs: Vec<PathBuf> = std::fs::read_dir(&history)
        .ok()?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| path.is_dir())
        .collect();
    dirs.sort();
    for dir in dirs.iter().rev() {
        let candidate = dir.join(MANIFEST_FILE_NAME);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// Read the newest archived manifest. Parse failures propagate (the
/// caller fails closed); `Ok(None)` means no archive carries one.
pub fn latest_archived_manifest(
    worktree_path: &Path,
) -> Result<Option<(PathBuf, ResumeManifest)>, ResumeManifestError> {
    match latest_archived_manifest_path(worktree_path) {
        Some(path) => {
            let manifest = read_manifest(&path)?;
            Ok(Some((path, manifest)))
        }
        None => Ok(None),
    }
}

/// Event log files to scan, in deterministic order: the main log first,
/// then `events-*.jsonl` hat channels under `.ralph/` and
/// `.ralph/agent/`, each sorted by file name.
fn event_log_paths(ralph_dir: &Path, agent_dir: &Path) -> Vec<PathBuf> {
    let mut paths = Vec::new();
    let main = ralph_dir.join("events.jsonl");
    if main.is_file() {
        paths.push(main);
    }
    for dir in [ralph_dir, agent_dir] {
        let Ok(entries) = std::fs::read_dir(dir) else {
            continue;
        };
        let mut channels: Vec<PathBuf> = entries
            .filter_map(|entry| entry.ok())
            .map(|entry| entry.path())
            .filter(|path| {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                path.is_file() && name.starts_with("events-") && name.ends_with(".jsonl")
            })
            .collect();
        channels.sort();
        paths.extend(channels);
    }
    paths
}

/// Task ledger snapshot. Malformed lines are recorded as reasons.
fn capture_tasks(tasks_path: &Path, reasons: &mut Vec<String>) -> Vec<TaskMappingEntry> {
    let mut tasks = Vec::new();
    if !tasks_path.is_file() {
        return tasks;
    }
    let Ok(body) = std::fs::read_to_string(tasks_path) else {
        reasons.push(format!("task ledger {} unreadable", tasks_path.display()));
        return tasks;
    };
    let mut malformed = 0usize;
    for line in body.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        match serde_json::from_str::<serde_json::Value>(trimmed) {
            Ok(value) => {
                let id = value.get("id").and_then(|v| v.as_str()).unwrap_or_default();
                let key = value
                    .get("key")
                    .and_then(|v| v.as_str())
                    .filter(|k| !k.trim().is_empty())
                    .unwrap_or(id);
                let status = value
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default();
                if key.trim().is_empty() {
                    malformed += 1;
                    continue;
                }
                tasks.push(TaskMappingEntry {
                    task_key: key.to_string(),
                    status: status.to_string(),
                });
            }
            Err(_) => malformed += 1,
        }
    }
    if malformed > 0 {
        reasons.push(format!("task ledger has {malformed} malformed line(s)"));
    }
    tasks
}

/// Artifact references from `*_path` payload fields of accepted
/// boundary events. Paths must be bounded; missing / oversized /
/// unreadable artifacts are recorded as reasons.
fn capture_artifacts(
    worktree_path: &Path,
    matched_events: &[crate::Event],
    reasons: &mut Vec<String>,
) -> Vec<ArtifactRef> {
    let mut artifacts: std::collections::BTreeMap<String, ArtifactRef> =
        std::collections::BTreeMap::new();
    for event in matched_events {
        let Some(payload) = event.payload.as_deref() else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) else {
            continue; // Non-JSON payloads carry no artifact references.
        };
        let Some(object) = value.as_object() else {
            continue;
        };
        for (key, field) in object {
            if !key.ends_with("_path") {
                continue;
            }
            let Some(raw) = field.as_str() else {
                continue;
            };
            if raw.trim().is_empty() {
                continue;
            }
            if let Err(e) = validate_bounded_path(raw) {
                reasons.push(format!("declared artifact path escapes worktree: {e}"));
                continue;
            }
            if artifacts.contains_key(raw) {
                continue;
            }
            let absolute = worktree_path.join(normalize_relative(raw));
            match std::fs::metadata(&absolute) {
                Ok(meta) if meta.is_file() => {
                    if meta.len() > MAX_ARTIFACT_DIGEST_BYTES {
                        reasons.push(format!(
                            "declared artifact {raw} exceeds the {}-byte digest limit",
                            MAX_ARTIFACT_DIGEST_BYTES
                        ));
                        continue;
                    }
                    match std::fs::read(&absolute) {
                        Ok(bytes) => {
                            artifacts.insert(
                                raw.to_string(),
                                ArtifactRef {
                                    path: raw.to_string(),
                                    digest: sha256_hex(&bytes),
                                },
                            );
                        }
                        Err(e) => {
                            reasons.push(format!("declared artifact {raw} unreadable: {e}"));
                        }
                    }
                }
                _ => reasons.push(format!("declared artifact missing: {raw}")),
            }
        }
    }
    artifacts.into_values().collect()
}

/// Normalize a relative path by dropping `./` components.
fn normalize_relative(raw: &str) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in Path::new(raw).components() {
        match component {
            Component::CurDir => {}
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// Truncate a payload snapshot at a char boundary.
fn truncate_payload_snapshot(payload: String) -> String {
    if payload.len() <= MAX_TRIGGER_PAYLOAD_SNAPSHOT_BYTES {
        return payload;
    }
    let cut = crate::text::floor_char_boundary(&payload, MAX_TRIGGER_PAYLOAD_SNAPSHOT_BYTES);
    format!("{}…", &payload[..cut])
}

/// Git HEAD of the worktree (provenance; empty when unavailable).
fn read_head_sha(worktree_path: &Path) -> String {
    std::process::Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(worktree_path)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Display a path relative to the worktree root (for diagnostics).
fn display_relative(path: &Path, worktree_path: &Path) -> String {
    path.strip_prefix(worktree_path)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn base_inputs() -> CaptureInputs {
        CaptureInputs {
            plan_path: "docs/plans/my-plan.md".to_string(),
            plan_digest: sha256_hex(b"plan body"),
            preset_name: "parallel-forge".to_string(),
            config_digest: sha256_hex(b"config body"),
            worktree_name: "my-plan".to_string(),
        }
    }

    /// Seed a minimal old-run runtime inside `worktree_path`.
    fn seed_runtime(worktree_path: &Path) {
        let ralph_dir = worktree_path.join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        fs::create_dir_all(&agent_dir).unwrap();
        fs::write(ralph_dir.join("current-loop-id"), "old-loop-1\n").unwrap();
    }

    /// Append an event line to `.ralph/events.jsonl`.
    fn append_event(worktree_path: &Path, line: &str) {
        use std::io::Write;
        let path = worktree_path.join(".ralph/events.jsonl");
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{line}").unwrap();
    }

    fn forge_plan_ready_line(payload: &str) -> String {
        format!(
            "{{\"ts\":\"2026-08-03T00:00:00Z\",\"iteration\":1,\"hat\":\"planner\",\"topic\":\"forge.plan.ready\",\"triggered\":\"guardian\",\"payload\":{}}}",
            serde_json::to_string(payload).unwrap()
        )
    }

    /// Append an accepted-transitions outbox entry for a topic/payload.
    fn append_outbox(worktree_path: &Path, topic: &str, payload: &str, hat: &str) {
        use std::io::Write;
        let payload_digest = sha256_hex(payload.as_bytes());
        let transition_id =
            crate::event_loop::accepted_transition::AcceptedTransition::compute_transition_id(
                "old-loop-1",
                &format!("{hat}:1"),
                "rev-1",
                &format!("{topic}:{hat}"),
                &payload_digest,
            );
        let entry = serde_json::json!({
            "activation_id": format!("{hat}:1"),
            "committed_at": "2026-08-03T00:00:01Z",
            "contract_revision": "rev-1",
            "delivered": false,
            "loop_id": "old-loop-1",
            "payload_digest": payload_digest,
            "topic": topic,
            "transition_id": transition_id,
        });
        let path = worktree_path.join(".ralph/agent/accepted-transitions.jsonl");
        let mut f = fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(f, "{}", entry).unwrap();
    }

    #[test]
    fn schema_roundtrip_and_digest_determinism() {
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\"}";
        append_event(dir.path(), &forge_plan_ready_line(payload));
        append_outbox(dir.path(), "forge.plan.ready", payload, "planner");

        let inputs = base_inputs();
        let manifest = capture_manifest(dir.path(), &inputs);

        assert_eq!(manifest.schema_version, MANIFEST_SCHEMA_VERSION);

        // Roundtrip
        let json = serde_json::to_string(&manifest).unwrap();
        let back: ResumeManifest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, manifest);

        // Digest is deterministic and non-empty.
        assert!(!manifest.manifest_digest.is_empty());
        assert_eq!(manifest.compute_digest(), manifest.manifest_digest);
        let again = capture_manifest(dir.path(), &inputs);
        // captured_at differs, but the canonical digest input excludes
        // nothing — so compare field-level determinism instead:
        assert_eq!(again.identity, manifest.identity);
        assert_eq!(again.boundary, manifest.boundary);
    }

    #[test]
    fn digest_tamper_detected_by_validate() {
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\"}";
        append_event(dir.path(), &forge_plan_ready_line(payload));
        append_outbox(dir.path(), "forge.plan.ready", payload, "planner");

        let inputs = base_inputs();
        let mut manifest = capture_manifest(dir.path(), &inputs);
        assert!(validate_manifest(&manifest, &inputs).is_ok());

        // Tamper with a recorded field.
        manifest.identity.plan_digest = sha256_hex(b"different plan");
        let err = validate_manifest(&manifest, &inputs).unwrap_err();
        assert!(
            matches!(err, ResumeManifestError::DigestMismatch { .. }),
            "tampered manifest must fail on self-digest, got {err:?}"
        );
    }

    #[test]
    fn identity_digest_stable_and_sensitive() {
        let inputs = base_inputs();
        let identity = ResumeIdentity {
            plan_path: inputs.plan_path.clone(),
            plan_digest: inputs.plan_digest.clone(),
            preset_name: inputs.preset_name.clone(),
            config_digest: inputs.config_digest.clone(),
            worktree_name: inputs.worktree_name.clone(),
            source_head_sha: "aaa".to_string(),
            loop_id: "old-loop-1".to_string(),
        };
        let d1 = identity_digest(&identity);
        assert_eq!(d1, identity_digest(&identity));

        // Provenance fields do not participate.
        let mut provenance_only = identity.clone();
        provenance_only.source_head_sha = "bbb".to_string();
        provenance_only.loop_id = "other-loop".to_string();
        assert_eq!(d1, identity_digest(&provenance_only));

        // Every drift-checked field is sensitive.
        let mut changed = identity.clone();
        changed.plan_digest = sha256_hex(b"x");
        assert_ne!(d1, identity_digest(&changed));
        let mut changed = identity.clone();
        changed.preset_name = "other".to_string();
        assert_ne!(d1, identity_digest(&changed));
        let mut changed = identity.clone();
        changed.config_digest = sha256_hex(b"y");
        assert_ne!(d1, identity_digest(&changed));
        let mut changed = identity.clone();
        changed.worktree_name = "other-name".to_string();
        assert_ne!(d1, identity_digest(&changed));
        let mut changed = identity.clone();
        changed.plan_path = "other.md".to_string();
        assert_ne!(d1, identity_digest(&changed));

        // Length-prefixed framing: no field-boundary collision.
        let a = ResumeIdentity {
            plan_path: "ab".into(),
            plan_digest: "c".into(),
            preset_name: String::new(),
            config_digest: String::new(),
            worktree_name: String::new(),
            source_head_sha: String::new(),
            loop_id: String::new(),
        };
        let b = ResumeIdentity {
            plan_path: "a".into(),
            plan_digest: "bc".into(),
            preset_name: String::new(),
            config_digest: String::new(),
            worktree_name: String::new(),
            source_head_sha: String::new(),
            loop_id: String::new(),
        };
        assert_ne!(identity_digest(&a), identity_digest(&b));
    }

    #[test]
    fn accepted_boundary_with_pending_hat_and_trigger() {
        // S1 shape: accepted forge.plan.ready, nothing after it.
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\",\"execution_plan_path\":\"execution-plan.yml\"}";
        append_event(dir.path(), &forge_plan_ready_line(payload));
        append_outbox(dir.path(), "forge.plan.ready", payload, "planner");
        // The declared artifact exists, so the manifest stays complete
        // and records a bounded artifact reference.
        fs::write(dir.path().join("execution-plan.yml"), "units: []\n").unwrap();

        let manifest = capture_manifest(dir.path(), &base_inputs());

        assert!(
            manifest.is_complete(),
            "manifest must be complete: {:?}",
            manifest.incomplete_reasons
        );
        assert_eq!(manifest.boundary.accepted.len(), 1);
        let ab = &manifest.boundary.accepted[0];
        assert_eq!(ab.topic, "forge.plan.ready");
        assert!(ab.in_event_log);
        assert_eq!(ab.hat.as_deref(), Some("planner"));
        assert_eq!(manifest.boundary.pending_hat.as_deref(), Some("guardian"));
        let trigger = manifest.boundary.original_trigger.as_ref().unwrap();
        assert_eq!(trigger.topic, "forge.plan.ready");
        assert_eq!(trigger.triggered.as_deref(), Some("guardian"));
        assert!(trigger.payload.as_deref().unwrap().contains("pf-1"));
        assert!(manifest.boundary.wave.is_none());

        // Artifact referenced by the accepted event is recorded.
        assert_eq!(manifest.artifacts.len(), 1);
        assert_eq!(manifest.artifacts[0].path, "execution-plan.yml");
        assert_eq!(manifest.artifacts[0].digest, sha256_hex(b"units: []\n"));
    }

    #[test]
    fn boundary_requires_accepted_terminal() {
        // Event present but NOT accepted (no outbox entry): no boundary.
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\"}";
        append_event(dir.path(), &forge_plan_ready_line(payload));

        let manifest = capture_manifest(dir.path(), &base_inputs());
        assert!(!manifest.is_complete());
        assert!(manifest.boundary.accepted.is_empty());
        assert!(manifest.boundary.pending_hat.is_none());
        let err = validate_manifest(&manifest, &base_inputs()).unwrap_err();
        assert!(
            matches!(err, ResumeManifestError::Incomplete { .. }),
            "unaccepted event must not form a boundary, got {err:?}"
        );
    }

    #[test]
    fn artifact_only_is_not_completion() {
        // S5 shape: artifact files exist on disk, but there is no
        // accepted terminal evidence at all.
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        fs::write(dir.path().join("execution-plan.yml"), "units: []\n").unwrap();
        fs::write(dir.path().join("REPORT.md"), "# done\n").unwrap();

        let manifest = capture_manifest(dir.path(), &base_inputs());
        assert!(
            !manifest.is_complete(),
            "artifact presence alone must not prove completion"
        );
        assert!(manifest.boundary.accepted.is_empty());
    }

    #[test]
    fn unaccepted_events_after_boundary_do_not_move_it() {
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\"}";
        append_event(dir.path(), &forge_plan_ready_line(payload));
        append_outbox(dir.path(), "forge.plan.ready", payload, "planner");
        // A later event that was never accepted.
        append_event(
            dir.path(),
            "{\"ts\":\"2026-08-03T00:05:00Z\",\"iteration\":2,\"hat\":\"guardian\",\"topic\":\"forge.wave.worktrees.ready\",\"triggered\":\"forge-dispatcher\",\"payload\":\"{}\"}",
        );

        let manifest = capture_manifest(dir.path(), &base_inputs());
        assert!(manifest.is_complete(), "{:?}", manifest.incomplete_reasons);
        assert_eq!(manifest.boundary.accepted.len(), 1);
        assert_eq!(manifest.boundary.accepted[0].topic, "forge.plan.ready");
        assert_eq!(manifest.boundary.pending_hat.as_deref(), Some("guardian"));
    }

    #[test]
    fn malformed_event_log_marks_incomplete() {
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\"}";
        append_event(dir.path(), &forge_plan_ready_line(payload));
        append_outbox(dir.path(), "forge.plan.ready", payload, "planner");
        append_event(dir.path(), "{\"broken\":");

        let manifest = capture_manifest(dir.path(), &base_inputs());
        assert!(!manifest.is_complete());
        assert!(
            manifest
                .incomplete_reasons
                .iter()
                .any(|r| r.contains("malformed")),
            "{:?}",
            manifest.incomplete_reasons
        );
    }

    #[test]
    fn boundary_event_missing_from_log_marks_incomplete() {
        // Outbox says accepted, but the event log lost the event.
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\"}";
        append_outbox(dir.path(), "forge.plan.ready", payload, "planner");

        let manifest = capture_manifest(dir.path(), &base_inputs());
        assert!(!manifest.is_complete());
        assert!(manifest.boundary.pending_hat.is_none());
    }

    #[test]
    fn bounded_path_validation_rules() {
        assert!(validate_bounded_path("execution-plan.yml").is_ok());
        assert!(validate_bounded_path("docs/plan.yml").is_ok());
        for bad in ["", "../escape.yml", "a/../../b.yml", "/abs.yml", "a/../b"] {
            assert!(
                validate_bounded_path(bad).is_err(),
                "path {bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn identity_drift_rejected() {
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\"}";
        append_event(dir.path(), &forge_plan_ready_line(payload));
        append_outbox(dir.path(), "forge.plan.ready", payload, "planner");

        let inputs = base_inputs();
        let manifest = capture_manifest(dir.path(), &inputs);
        assert!(validate_manifest(&manifest, &inputs).is_ok());

        // Plan content changed between runs.
        let mut drifted = inputs.clone();
        drifted.plan_digest = sha256_hex(b"edited plan");
        let err = validate_manifest(&manifest, &drifted).unwrap_err();
        match err {
            ResumeManifestError::IdentityDrift { fields } => {
                assert!(fields.contains(&"plan_digest".to_string()));
            }
            other => panic!("expected IdentityDrift, got {other:?}"),
        }

        // Preset changed between runs.
        let mut drifted = inputs.clone();
        drifted.preset_name = "ce-executor-pipeline".to_string();
        assert!(matches!(
            validate_manifest(&manifest, &drifted),
            Err(ResumeManifestError::IdentityDrift { .. })
        ));
    }

    #[test]
    fn prior_manifest_chain_inherits_identity() {
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\"}";
        append_event(dir.path(), &forge_plan_ready_line(payload));
        append_outbox(dir.path(), "forge.plan.ready", payload, "planner");

        // First capture establishes the identity baseline.
        let first_inputs = base_inputs();
        let first = capture_manifest(dir.path(), &first_inputs);
        let archive = dir.path().join(".ralph/reuse-history/20260101T000000Z");
        fs::create_dir_all(&archive).unwrap();
        write_manifest(&first, &archive).unwrap();

        // Second capture with DIFFERENT current inputs inherits the
        // chained identity from the prior manifest.
        let mut second_inputs = first_inputs.clone();
        second_inputs.plan_digest = sha256_hex(b"changed plan");
        let second = capture_manifest(dir.path(), &second_inputs);
        assert_eq!(
            second.identity.plan_digest, first.identity.plan_digest,
            "chained identity must come from the prior manifest"
        );
        // Validation against the changed inputs fails closed.
        assert!(matches!(
            validate_manifest(&second, &second_inputs),
            Err(ResumeManifestError::IdentityDrift { .. })
        ));
        // Validation against the original inputs passes (minus digest
        // recompute — second was finalized at capture).
        assert!(validate_manifest(&second, &first_inputs).is_ok());
    }

    #[test]
    fn tampered_prior_manifest_marks_incomplete() {
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\"}";
        append_event(dir.path(), &forge_plan_ready_line(payload));
        append_outbox(dir.path(), "forge.plan.ready", payload, "planner");

        let first = capture_manifest(dir.path(), &base_inputs());
        let archive = dir.path().join(".ralph/reuse-history/20260101T000000Z");
        fs::create_dir_all(&archive).unwrap();
        let mut tampered = first.clone();
        tampered.identity.plan_digest = sha256_hex(b"forged");
        write_manifest(&tampered, &archive).unwrap();

        let second = capture_manifest(dir.path(), &base_inputs());
        assert!(
            second
                .incomplete_reasons
                .iter()
                .any(|r| r.contains("prior resume manifest")),
            "{:?}",
            second.incomplete_reasons
        );
    }

    #[test]
    fn missing_declared_artifact_marks_incomplete() {
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\",\"execution_plan_path\":\"execution-plan.yml\"}";
        append_event(dir.path(), &forge_plan_ready_line(payload));
        append_outbox(dir.path(), "forge.plan.ready", payload, "planner");
        // NOTE: execution-plan.yml is NOT written to disk.

        let manifest = capture_manifest(dir.path(), &base_inputs());
        assert!(
            manifest
                .incomplete_reasons
                .iter()
                .any(|r| r.contains("execution-plan.yml")),
            "{:?}",
            manifest.incomplete_reasons
        );
    }

    #[test]
    fn tasks_snapshot_from_task_ledger() {
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\"}";
        append_event(dir.path(), &forge_plan_ready_line(payload));
        append_outbox(dir.path(), "forge.plan.ready", payload, "planner");
        fs::write(
            dir.path().join(".ralph/agent/tasks.jsonl"),
            concat!(
                "{\"id\":\"task-1\",\"title\":\"U1\",\"key\":\"forge:pf-1:U1\",\"status\":\"closed\",\"priority\":1,\"created\":\"2026-08-03T00:00:00Z\"}\n",
                "{\"id\":\"task-2\",\"title\":\"U2\",\"key\":\"forge:pf-1:U2\",\"status\":\"open\",\"priority\":1,\"created\":\"2026-08-03T00:00:00Z\"}\n",
            ),
        )
        .unwrap();

        let manifest = capture_manifest(dir.path(), &base_inputs());
        assert_eq!(manifest.tasks.len(), 2);
        assert_eq!(manifest.tasks[0].task_key, "forge:pf-1:U1");
        assert_eq!(manifest.tasks[0].status, "closed");
        assert_eq!(manifest.tasks[1].task_key, "forge:pf-1:U2");
        assert_eq!(manifest.tasks[1].status, "open");
    }

    #[test]
    fn read_manifest_rejects_wrong_schema_and_garbage() {
        let dir = tempfile::TempDir::new().unwrap();
        let path = dir.path().join("m.json");
        fs::write(&path, "{\"schema_version\":\"other\"}").unwrap();
        assert!(matches!(
            read_manifest(&path),
            Err(ResumeManifestError::SchemaVersion { .. })
        ));
        fs::write(&path, "{not json").unwrap();
        assert!(matches!(
            read_manifest(&path),
            Err(ResumeManifestError::Parse { .. })
        ));
        assert!(matches!(
            read_manifest(&dir.path().join("missing.json")),
            Err(ResumeManifestError::Io { .. })
        ));
    }

    #[test]
    fn latest_archived_manifest_prefers_newest_archive() {
        let dir = tempfile::TempDir::new().unwrap();
        seed_runtime(dir.path());
        let payload = "{\"plan_key\":\"pf-1\"}";
        append_event(dir.path(), &forge_plan_ready_line(payload));
        append_outbox(dir.path(), "forge.plan.ready", payload, "planner");

        let manifest = capture_manifest(dir.path(), &base_inputs());
        for ts in ["20260101T000000Z", "20260202T000000Z"] {
            let archive = dir.path().join(".ralph/reuse-history").join(ts);
            fs::create_dir_all(&archive).unwrap();
            write_manifest(&manifest, &archive).unwrap();
        }

        let (path, read) = latest_archived_manifest(dir.path())
            .unwrap()
            .expect("archived manifest must be found");
        assert!(path.to_string_lossy().contains("20260202T000000Z"));
        assert_eq!(read.manifest_digest, manifest.manifest_digest);
    }
}
