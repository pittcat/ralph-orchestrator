//! U10 (plan 2026-07-30-004): Parallel Forge plan handoff verification.
//!
//! The planner hat submits an `execution-plan.yml` reference + identity +
//! digest via `forge.plan.ready`. The runtime verifies the artifact
//! against the canonical digest (U9) before projecting the task DAG.
//!
//! # Handoff protocol
//!
//! 1. The planner submits only the artifact path and plan identity.
//! 2. The CLI policy-check/apply path canonicalizes the bounded artifact and
//!    stamps the `plan_digest`; direct runtime ingress may omit the digest.
//! 3. The runtime reads the workspace-bounded artifact once, derives the task
//!    DAG, and compares any supplied digest. A mismatch rejects the handoff
//!    before task projection.

use crate::artifact_canonicalizer::{ArtifactError, CanonicalArtifact, canonicalize};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::path::Path;

/// Error from plan handoff verification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HandoffError {
    /// The artifact path escapes the workspace root or contains a parent
    /// traversal component.
    PathEscape { path: String },
    /// The artifact path is not a regular file.
    NotRegularFile { path: String },
    /// The artifact changed while it was being read.
    ChangedDuringRead { path: String },
    /// The artifact could not be read.
    Io { path: String, source: String },
    /// The digest in the event payload did not match the canonical digest.
    DigestMismatch {
        /// The digest the event payload claimed.
        expected: String,
        /// The digest computed from the artifact bytes.
        actual: String,
    },
    /// The raw artifact exceeded the 1 MiB size bound.
    ArtifactTooLarge {
        /// Observed size in bytes.
        size: usize,
        /// The configured limit (1 MiB).
        limit: usize,
    },
    /// The artifact declared too many Units.
    TooManyUnits {
        /// Observed Unit count.
        count: usize,
        /// The configured limit.
        limit: usize,
    },
    /// The artifact declared too many dependency edges.
    TooManyEdges {
        /// Observed edge count.
        count: usize,
        /// The configured limit.
        limit: usize,
    },
    /// The artifact was not valid YAML.
    ParseError {
        /// The underlying parser message.
        source: String,
    },
}

impl std::fmt::Display for HandoffError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HandoffError::PathEscape { path } => {
                write!(f, "artifact path escapes workspace: {path}")
            }
            HandoffError::NotRegularFile { path } => {
                write!(f, "artifact is not a regular file: {path}")
            }
            HandoffError::ChangedDuringRead { path } => {
                write!(f, "artifact changed during read: {path}")
            }
            HandoffError::Io { path, source } => {
                write!(f, "failed to read artifact {path}: {source}")
            }
            HandoffError::DigestMismatch { expected, actual } => write!(
                f,
                "plan digest mismatch: event claimed {expected}, artifact canonicalized to {actual}"
            ),
            HandoffError::ArtifactTooLarge { size, limit } => write!(
                f,
                "artifact is {size} bytes, exceeding the {limit}-byte (1 MiB) limit"
            ),
            HandoffError::TooManyUnits { count, limit } => {
                write!(
                    f,
                    "artifact declares {count} units, exceeding the limit of {limit}"
                )
            }
            HandoffError::TooManyEdges { count, limit } => write!(
                f,
                "artifact declares {count} dependency edges, exceeding the limit of {limit}"
            ),
            HandoffError::ParseError { source } => {
                write!(f, "failed to parse artifact YAML: {source}")
            }
        }
    }
}

impl std::error::Error for HandoffError {}

impl From<ArtifactError> for HandoffError {
    fn from(e: ArtifactError) -> Self {
        match e {
            ArtifactError::TooLarge { size, limit } => {
                HandoffError::ArtifactTooLarge { size, limit }
            }
            ArtifactError::TooManyUnits { count, limit } => {
                HandoffError::TooManyUnits { count, limit }
            }
            ArtifactError::TooManyEdges { count, limit } => {
                HandoffError::TooManyEdges { count, limit }
            }
            ArtifactError::ParseError { source } => HandoffError::ParseError { source },
            ArtifactError::AliasesForbidden => HandoffError::ParseError {
                source: "artifact uses YAML anchors/aliases, which are forbidden".to_string(),
            },
        }
    }
}

/// Verify a `forge.plan.ready` event payload against the raw artifact bytes.
///
/// Pipeline:
/// 1. Canonicalize the artifact via U9 (bounds enforcement + deterministic
///    digest).
/// 2. Extract `plan_digest` from the event payload.
/// 3. Compare the canonical digest against the payload's claimed digest.
/// 4. Return `Ok(canonical)` on match, `Err(DigestMismatch)` on mismatch.
///
/// # Errors
///
/// Returns [`HandoffError::DigestMismatch`] when the payload's `plan_digest`
/// does not match the canonical digest, or any [`HandoffError`] variant
/// propagated from U9 canonicalization (size / unit / edge / parse errors).
pub fn verify_plan_handoff(
    payload: &Value,
    artifact_bytes: &[u8],
) -> Result<crate::artifact_canonicalizer::CanonicalArtifact, HandoffError> {
    let canonical = canonicalize(artifact_bytes)?;

    let expected_digest = payload
        .get("plan_digest")
        .and_then(Value::as_str)
        .unwrap_or("");

    if !expected_digest.is_empty() && canonical.digest != expected_digest {
        return Err(HandoffError::DigestMismatch {
            expected: expected_digest.to_string(),
            actual: canonical.digest.clone(),
        });
    }

    Ok(canonical)
}

/// Read an artifact once from a workspace-bounded regular file.
fn read_artifact_bytes(workspace: &Path, artifact_path: &Path) -> Result<Vec<u8>, HandoffError> {
    if artifact_path
        .components()
        .any(|component| matches!(component, std::path::Component::ParentDir))
    {
        return Err(HandoffError::PathEscape {
            path: artifact_path.display().to_string(),
        });
    }
    let root = workspace.canonicalize().map_err(|error| HandoffError::Io {
        path: workspace.display().to_string(),
        source: error.to_string(),
    })?;
    let candidate = if artifact_path.is_absolute() {
        artifact_path.to_path_buf()
    } else {
        workspace.join(artifact_path)
    };
    let resolved = candidate.canonicalize().map_err(|error| HandoffError::Io {
        path: candidate.display().to_string(),
        source: error.to_string(),
    })?;
    if !resolved.starts_with(&root) {
        return Err(HandoffError::PathEscape {
            path: artifact_path.display().to_string(),
        });
    }
    let before = std::fs::metadata(&resolved).map_err(|error| HandoffError::Io {
        path: resolved.display().to_string(),
        source: error.to_string(),
    })?;
    if !before.is_file() {
        return Err(HandoffError::NotRegularFile {
            path: resolved.display().to_string(),
        });
    }
    let bytes = std::fs::read(&resolved).map_err(|error| HandoffError::Io {
        path: resolved.display().to_string(),
        source: error.to_string(),
    })?;
    let after = std::fs::metadata(&resolved).map_err(|error| HandoffError::Io {
        path: resolved.display().to_string(),
        source: error.to_string(),
    })?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(HandoffError::ChangedDuringRead {
            path: resolved.display().to_string(),
        });
    }
    Ok(bytes)
}

/// Read and verify an artifact from a contract-bounded workspace path.
/// The file is opened once, checked as a regular file, read completely, and
/// checked again before canonicalization to detect replacement/TOCTOU races.
pub fn verify_plan_handoff_path(
    payload: &Value,
    workspace: &Path,
    artifact_path: &Path,
) -> Result<crate::artifact_canonicalizer::CanonicalArtifact, HandoffError> {
    let bytes = read_artifact_bytes(workspace, artifact_path)?;
    verify_plan_handoff(payload, &bytes)
}

/// One runtime-derived task specification from `execution-plan.yml`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CanonicalTaskSpec {
    pub unit_id: String,
    pub task_key: String,
    pub title: String,
    pub depends_on_task_keys: Vec<String>,
    pub execution_wave: u32,
    pub integration_order: u32,
}

/// Verified Parallel Forge plan handoff. Derived task data never comes from
/// the event payload; the execution-plan artifact is the only authority.
#[derive(Debug, Clone)]
pub struct CanonicalPlanHandoff {
    pub artifact: CanonicalArtifact,
    pub plan_key: String,
    pub tasks: Vec<CanonicalTaskSpec>,
    pub wave_total: u32,
}

#[derive(Debug, Deserialize)]
struct ExecutionPlanArtifact {
    plan_key: String,
    units: Vec<ExecutionPlanUnit>,
}

#[derive(Debug, Deserialize)]
struct ExecutionPlanUnit {
    id: String,
    title: String,
    #[serde(default)]
    depends_on: Vec<String>,
    execution_wave: u32,
    integration_order: u32,
}

/// Read, verify, and derive the canonical Parallel Forge task schedule.
///
/// The event payload may only identify the artifact (`execution_plan_path`,
/// `plan_key`, `plan_digest`). Derived fields such as `unit_tasks` are rejected
/// so the planner cannot override the on-disk DAG.
pub fn load_plan_handoff(
    payload: &Value,
    workspace: &Path,
) -> Result<CanonicalPlanHandoff, HandoffError> {
    if payload.get("unit_tasks").is_some() {
        return Err(HandoffError::ParseError {
            source: "forge.plan.ready must not contain derived field 'unit_tasks'".to_string(),
        });
    }

    let artifact_path = payload
        .get("execution_plan_path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| HandoffError::ParseError {
            source: "forge.plan.ready requires non-empty execution_plan_path".to_string(),
        })?;
    let artifact = verify_plan_handoff_path(payload, workspace, Path::new(artifact_path))?;
    derive_plan_handoff(payload, artifact)
}

fn derive_plan_handoff(
    payload: &Value,
    artifact: CanonicalArtifact,
) -> Result<CanonicalPlanHandoff, HandoffError> {
    let plan: ExecutionPlanArtifact =
        serde_yaml::from_slice(&artifact.canonical_bytes).map_err(|error| {
            HandoffError::ParseError {
                source: format!("invalid Parallel Forge execution plan: {error}"),
            }
        })?;

    let payload_plan_key = payload
        .get("plan_key")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| HandoffError::ParseError {
            source: "forge.plan.ready requires non-empty plan_key".to_string(),
        })?;
    if plan.plan_key != payload_plan_key {
        return Err(HandoffError::ParseError {
            source: format!(
                "plan_key mismatch: payload declares '{payload_plan_key}', artifact declares '{}'",
                plan.plan_key
            ),
        });
    }
    if plan.units.is_empty() {
        return Err(HandoffError::ParseError {
            source: "execution plan units must be non-empty".to_string(),
        });
    }

    let mut unit_ids = HashSet::with_capacity(plan.units.len());
    let mut task_keys = HashMap::with_capacity(plan.units.len());
    for unit in &plan.units {
        if unit.id.trim().is_empty() || !unit_ids.insert(unit.id.clone()) {
            return Err(HandoffError::ParseError {
                source: format!(
                    "execution plan has empty or duplicate unit id '{}'",
                    unit.id
                ),
            });
        }
        task_keys.insert(
            unit.id.clone(),
            format!("forge:{}:{}", plan.plan_key, unit.id),
        );
    }

    let mut tasks = Vec::with_capacity(plan.units.len());
    for unit in plan.units {
        let depends_on_task_keys = unit
            .depends_on
            .iter()
            .map(|dependency| {
                task_keys
                    .get(dependency)
                    .cloned()
                    .ok_or_else(|| HandoffError::ParseError {
                        source: format!(
                            "unit '{}' depends on unknown unit '{dependency}'",
                            unit.id
                        ),
                    })
            })
            .collect::<Result<Vec<_>, _>>()?;
        tasks.push(CanonicalTaskSpec {
            unit_id: unit.id.clone(),
            task_key: task_keys[&unit.id].clone(),
            title: unit.title,
            depends_on_task_keys,
            execution_wave: unit.execution_wave,
            integration_order: unit.integration_order,
        });
    }
    let wave_total = tasks
        .iter()
        .map(|task| task.execution_wave)
        .max()
        .unwrap_or(0);

    Ok(CanonicalPlanHandoff {
        artifact,
        plan_key: plan.plan_key,
        tasks,
        wave_total,
    })
}

/// Normalize a `forge.plan.ready` payload into the runtime-owned summary.
///
/// `plan_digest`, `unit_count`, and `wave_total` are always overwritten from
/// the verified artifact. This function is shared by CLI precheck/apply and
/// EventLoop ingress so both paths accept the same bytes and produce the same
/// canonical payload.
pub fn canonicalize_plan_ready_payload(
    payload_text: &str,
    workspace: &Path,
) -> Result<String, HandoffError> {
    let mut payload: Value =
        serde_json::from_str(payload_text).map_err(|error| HandoffError::ParseError {
            source: format!("forge.plan.ready payload must be a JSON object: {error}"),
        })?;
    let object = payload
        .as_object_mut()
        .ok_or_else(|| HandoffError::ParseError {
            source: "forge.plan.ready payload must be a JSON object".to_string(),
        })?;
    if object.contains_key("unit_tasks") {
        return Err(HandoffError::ParseError {
            source: "forge.plan.ready must not contain derived field 'unit_tasks'".to_string(),
        });
    }

    let artifact_path = object
        .get("execution_plan_path")
        .and_then(Value::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| HandoffError::ParseError {
            source: "forge.plan.ready requires non-empty execution_plan_path".to_string(),
        })?
        .to_string();
    let bytes = read_artifact_bytes(workspace, Path::new(&artifact_path))?;
    let computed = canonicalize(&bytes)?;
    if let Some(claimed) = object.get("plan_digest").and_then(Value::as_str)
        && !claimed.is_empty()
        && claimed != computed.digest
    {
        return Err(HandoffError::DigestMismatch {
            expected: claimed.to_string(),
            actual: computed.digest.clone(),
        });
    }
    object.insert(
        "plan_digest".to_string(),
        Value::String(computed.digest.clone()),
    );

    let handoff = derive_plan_handoff(&payload, computed)?;
    let object = payload.as_object_mut().expect("validated object");
    object.insert(
        "unit_count".to_string(),
        Value::from(u64::try_from(handoff.tasks.len()).unwrap_or(u64::MAX)),
    );
    object.insert("wave_total".to_string(), Value::from(handoff.wave_total));
    serde_json::to_string(&payload).map_err(|error| HandoffError::ParseError {
        source: format!("failed to serialize canonical forge.plan.ready payload: {error}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Minimal valid artifact: 1 Unit, 0 edges.
    const MINIMAL_ARTIFACT: &[u8] = b"units:\n  - id: u0\nedges: []\n";

    const PLAN_ARTIFACT: &[u8] = br#"version: 1
plan_key: pf-test
units:
  - id: U1
    title: Foundation
    depends_on: []
    execution_wave: 1
    integration_order: 1
  - id: U2
    title: Feature
    depends_on: [U1]
    execution_wave: 2
    integration_order: 2
"#;

    #[test]
    fn artifact_first_handoff_derives_tasks_and_schedule() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("execution-plan.yml");
        std::fs::write(&path, PLAN_ARTIFACT).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-test",
        });

        let handoff = load_plan_handoff(&payload, temp.path()).expect("valid handoff");
        assert_eq!(handoff.tasks.len(), 2);
        assert_eq!(handoff.wave_total, 2);
        assert_eq!(handoff.tasks[0].task_key, "forge:pf-test:U1");
        assert_eq!(
            handoff.tasks[1].depends_on_task_keys,
            vec!["forge:pf-test:U1"]
        );
    }

    #[test]
    fn artifact_first_handoff_rejects_payload_derived_tasks() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), PLAN_ARTIFACT).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-test",
            "unit_tasks": [],
        });

        let error = load_plan_handoff(&payload, temp.path())
            .expect_err("derived payload tasks must be rejected");
        assert!(
            error
                .to_string()
                .contains("must not contain derived field 'unit_tasks'")
        );
    }

    #[test]
    fn cli_payload_normalization_adds_runtime_summary() {
        let temp = tempfile::tempdir().expect("tempdir");
        std::fs::write(temp.path().join("execution-plan.yml"), PLAN_ARTIFACT).expect("write plan");
        let payload = json!({
            "execution_plan_path": "execution-plan.yml",
            "plan_key": "pf-test",
        })
        .to_string();

        let normalized =
            canonicalize_plan_ready_payload(&payload, temp.path()).expect("payload must normalize");
        let value: Value = serde_json::from_str(&normalized).expect("normalized JSON");
        assert_eq!(value["unit_count"], 2);
        assert_eq!(value["wave_total"], 2);
        assert!(
            value["plan_digest"]
                .as_str()
                .is_some_and(|value| !value.is_empty())
        );
        assert!(value.get("unit_tasks").is_none());
    }

    #[test]
    fn u10_forge_plan_ready_carries_canonical_digest() {
        let canonical = crate::artifact_canonicalizer::canonicalize(MINIMAL_ARTIFACT)
            .expect("must canonicalize");

        let payload = json!({
            "plan_digest": canonical.digest,
            "execution_wave": 1,
        });

        let result = verify_plan_handoff(&payload, MINIMAL_ARTIFACT);
        assert!(result.is_ok(), "matching digest must succeed: {result:?}");

        let verified = result.unwrap();
        assert_eq!(verified.digest, canonical.digest);
        assert_eq!(verified.unit_count, 1);
        assert_eq!(verified.edge_count, 0);
    }

    #[test]
    fn u10_forge_plan_ready_rejects_digest_mismatch() {
        let canonical = crate::artifact_canonicalizer::canonicalize(MINIMAL_ARTIFACT)
            .expect("must canonicalize");

        let payload = json!({
            "plan_digest": "wrong_digest",
        });

        let err = verify_plan_handoff(&payload, MINIMAL_ARTIFACT)
            .expect_err("mismatched digest must be rejected");

        assert!(
            matches!(
                &err,
                HandoffError::DigestMismatch { expected, actual }
                    if expected == "wrong_digest" && actual == &canonical.digest
            ),
            "unexpected error: {err:?}"
        );
    }

    #[test]
    fn u10_forge_plan_ready_rejects_oversized_artifact() {
        // 1 MiB + 1 byte — exceeds the U9 size bound.
        let oversized = vec![b'a'; crate::artifact_canonicalizer::MAX_ARTIFACT_BYTES + 1];

        let payload = json!({ "plan_digest": "any" });

        let err = verify_plan_handoff(&payload, &oversized)
            .expect_err("oversized artifact must be rejected");

        assert!(
            matches!(
                err,
                HandoffError::ArtifactTooLarge { size, limit }
                    if size == crate::artifact_canonicalizer::MAX_ARTIFACT_BYTES + 1
                    && limit == crate::artifact_canonicalizer::MAX_ARTIFACT_BYTES
            ),
            "unexpected error: {err:?}"
        );
    }
}
