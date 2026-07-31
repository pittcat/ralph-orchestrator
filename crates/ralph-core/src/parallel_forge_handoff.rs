//! U10 (plan 2026-07-30-004): Parallel Forge plan handoff verification.
//!
//! The planner hat submits an `execution-plan.yml` reference + identity +
//! digest via `forge.plan.ready`. The runtime verifies the artifact
//! against the canonical digest (U9) before projecting the task DAG.
//!
//! # Handoff protocol
//!
//! 1. The planner canonicalizes the artifact locally (U9) and embeds the
//!    resulting SHA-256 digest in the `forge.plan.ready` payload under
//!    the `plan_digest` key.
//! 2. On receipt the runtime calls [`verify_plan_handoff`] with the event
//!    payload and the raw artifact bytes.
//! 3. Verification re-canonicalizes the artifact (enforcing all U9 bounds)
//!    and compares the digest. A mismatch means the artifact was mutated
//!    after the planner stamped it — the handoff is rejected.

use crate::artifact_canonicalizer::{canonicalize, ArtifactError};
use serde_json::Value;
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
            HandoffError::PathEscape { path } => write!(f, "artifact path escapes workspace: {path}"),
            HandoffError::NotRegularFile { path } => write!(f, "artifact is not a regular file: {path}"),
            HandoffError::ChangedDuringRead { path } => write!(f, "artifact changed during read: {path}"),
            HandoffError::Io { path, source } => write!(f, "failed to read artifact {path}: {source}"),
            HandoffError::DigestMismatch { expected, actual } => write!(
                f,
                "plan digest mismatch: event claimed {expected}, artifact canonicalized to {actual}"
            ),
            HandoffError::ArtifactTooLarge { size, limit } => write!(
                f,
                "artifact is {size} bytes, exceeding the {limit}-byte (1 MiB) limit"
            ),
            HandoffError::TooManyUnits { count, limit } => {
                write!(f, "artifact declares {count} units, exceeding the limit of {limit}")
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

    if canonical.digest != expected_digest {
        return Err(HandoffError::DigestMismatch {
            expected: expected_digest.to_string(),
            actual: canonical.digest.clone(),
        });
    }

    Ok(canonical)
}

/// Read and verify an artifact from a contract-bounded workspace path.
/// The file is opened once, checked as a regular file, read completely, and
/// checked again before canonicalization to detect replacement/TOCTOU races.
pub fn verify_plan_handoff_path(
    payload: &Value,
    workspace: &Path,
    artifact_path: &Path,
) -> Result<crate::artifact_canonicalizer::CanonicalArtifact, HandoffError> {
    if artifact_path.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(HandoffError::PathEscape { path: artifact_path.display().to_string() });
    }
    let root = workspace.canonicalize().map_err(|e| HandoffError::Io {
        path: workspace.display().to_string(), source: e.to_string(),
    })?;
    let candidate = if artifact_path.is_absolute() {
        artifact_path.to_path_buf()
    } else {
        workspace.join(artifact_path)
    };
    let resolved = candidate.canonicalize().map_err(|e| HandoffError::Io {
        path: candidate.display().to_string(), source: e.to_string(),
    })?;
    if !resolved.starts_with(&root) {
        return Err(HandoffError::PathEscape { path: artifact_path.display().to_string() });
    }
    let before = std::fs::metadata(&resolved).map_err(|e| HandoffError::Io {
        path: resolved.display().to_string(), source: e.to_string(),
    })?;
    if !before.is_file() {
        return Err(HandoffError::NotRegularFile { path: resolved.display().to_string() });
    }
    let bytes = std::fs::read(&resolved).map_err(|e| HandoffError::Io {
        path: resolved.display().to_string(), source: e.to_string(),
    })?;
    let after = std::fs::metadata(&resolved).map_err(|e| HandoffError::Io {
        path: resolved.display().to_string(), source: e.to_string(),
    })?;
    if before.len() != after.len() || before.modified().ok() != after.modified().ok() {
        return Err(HandoffError::ChangedDuringRead { path: resolved.display().to_string() });
    }
    verify_plan_handoff(payload, &bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Minimal valid artifact: 1 Unit, 0 edges.
    const MINIMAL_ARTIFACT: &[u8] = b"units:\n  - id: u0\nedges: []\n";

    #[test]
    fn u10_forge_plan_ready_carries_canonical_digest() {
        let canonical =
            crate::artifact_canonicalizer::canonicalize(MINIMAL_ARTIFACT).expect("must canonicalize");

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
        let canonical =
            crate::artifact_canonicalizer::canonicalize(MINIMAL_ARTIFACT).expect("must canonicalize");

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
