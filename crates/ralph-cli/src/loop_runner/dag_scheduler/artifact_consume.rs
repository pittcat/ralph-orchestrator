//! 2026-09-13-001-fix-forge-dag-artifact-handoff-plan U4 (C2+M1):
//! typed error and helper decomposition for the artifact consume
//! path.
//!
//! The previous `consume_stage_artifact` returned
//! `Result<ArtifactRef, String>` with seven failure modes flattened
//! into a single `String` reason. Tests then had to substring-match
//! the wording, which coupled them to the `format!` chain. This
//! module introduces a typed `ArtifactConsumeError` enum so callers
//! and tests can pattern-match on the failure mode without parsing
//! strings.
//!
//! The decomposition (M1) extracts three helpers —
//! `find_stage_row`, `resolve_under_worktree`, and `verify_digest` —
//! so the new typed `consume_stage_artifact` shrinks to a flat
//! composition of typed steps.

use std::fmt;

/// Typed error returned by `consume_stage_artifact`. Each variant
/// captures one of the seven documented failure modes so callers
/// can pattern-match on the cause (the previous `String`-based
/// error forced substring matching).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArtifactConsumeError {
    /// The DAG artifact store could not be opened; the spawn
    /// seam cannot prove the prior hat's evidence persisted, so
    /// it refuses to launch the next stage.
    StoreUnavailable,
    /// The store accepted the query but returned a read error
    /// (poisoned mutex, IO failure, etc).
    StoreReadFailed,
    /// No row exists for `(plan_key, unit_key, stage, field_name)`.
    RowMissing {
        stage: String,
        field: String,
        plan_key: String,
        unit_key: String,
    },
    /// The recorded path failed the surface shape check
    /// (`is_safe_repo_relative_path`).
    PathShapeRejected {
        stage: String,
        field: String,
        path: String,
    },
    /// The resolved absolute path escaped the unit worktree.
    WorktreeEscape {
        stage: String,
        field: String,
        resolved: String,
    },
    /// The on-disk file is missing for an otherwise-valid row.
    FileMissing {
        stage: String,
        field: String,
        path: String,
    },
    /// The on-disk file cannot be read.
    FileUnreadable {
        stage: String,
        field: String,
        path: String,
        source: String,
    },
    /// The on-disk file's SHA-256 does not match the recorded
    /// digest.
    DigestDrift {
        stage: String,
        field: String,
        recorded: String,
        live: String,
    },
    /// No execution context is attached (the runtime has not
    /// observed a `forge.plan.ready` event).
    NoExecutionContext,
}

impl fmt::Display for ArtifactConsumeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::StoreUnavailable => write!(f, "DAG artifact store unavailable"),
            Self::StoreReadFailed => write!(f, "DAG artifact store read failed"),
            Self::RowMissing {
                stage,
                field,
                plan_key,
                unit_key,
            } => write!(
                f,
                "DAG artifact missing: stage={stage} field={field} plan={plan_key} \
                 unit={unit_key} (no row recorded by the prior hat — the prior \
                 stage did not publish a valid {field})"
            ),
            Self::PathShapeRejected { stage, field, path } => write!(
                f,
                "DAG artifact path unsafe: stage={stage} field={field} path=`{path}` \
                 (rejected by is_safe_repo_relative_path)"
            ),
            Self::WorktreeEscape {
                stage,
                field,
                resolved,
            } => write!(
                f,
                "DAG artifact escapes worktree: stage={stage} field={field} \
                 resolved=`{resolved}`"
            ),
            Self::FileMissing { stage, field, path } => write!(
                f,
                "DAG artifact file unreadable: stage={stage} field={field} path=`{path}` \
                 (file not found)"
            ),
            Self::FileUnreadable {
                stage,
                field,
                path,
                source,
            } => write!(
                f,
                "DAG artifact read failed: stage={stage} field={field} path=`{path}` \
                 err={source}"
            ),
            Self::DigestDrift {
                stage,
                field,
                recorded,
                live,
            } => write!(
                f,
                "DAG artifact digest drift: stage={stage} field={field} \
                 recorded={recorded} live={live}"
            ),
            Self::NoExecutionContext => write!(f, "DAG artifact: no execution context"),
        }
    }
}

impl std::error::Error for ArtifactConsumeError {}
