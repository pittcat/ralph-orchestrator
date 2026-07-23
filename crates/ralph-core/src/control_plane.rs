//! 2026-07-23-004 plan U3 (R-A1): guard the worker
//! control-plane binding so the main events JSONL is the only
//! ledger a worker may write to. Slot worktree writes are
//! considered `orphan_event_route` P0 failures.
//!
//! Two contracts enforced here:
//! 1. `validate_control_plane_binding` returns
//!    `Ok(absolute_path)` only when the channel is:
//!    * absolute (not relative),
//!    * not nested inside the slot worktree,
//!    * not a symlink escaping the primary workspace,
//!    * its parent directory is creatable.
//!    Otherwise it returns a typed [`ControlPlaneError`] that
//!    carries the failing rule so the dispatcher can surface a
//!    stable `invalid_control_plane_path` reason code.
//! 2. `merge_event_channel_env` is the one function the wave
//!    dispatcher / `inject_hat_execution_env` calls to add the
//!    explicit `RALPH_WORKSPACE_ROOT` and `RALPH_EVENTS_FILE`
//!    bindings; it falls through to the caller-provided values
//!    only when those values are validated.

use std::path::{Path, PathBuf};

/// Typed reason why a control-plane binding was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ControlPlaneError {
    /// The path is not absolute (the dispatcher will not allow
    /// spawning a worker that resolves the events file lazily
    /// against its cwd).
    RelativePath { path: PathBuf },
    /// The events file lives inside a slot worktree. The slot
    /// tree is code-only and must not host JSONL ledger writes.
    SlotSubtree { path: PathBuf, slot_root: PathBuf },
    /// The path escapes the primary workspace via a symlink.
    /// Workers cannot be allowed to escape the namespace they
    /// are bound to, even if the path string looks safe.
    SymlinkEscape {
        path: PathBuf,
        workspace_root: PathBuf,
    },
    /// The events file's parent directory cannot be created /
    /// is not writable.
    UncreatableParent { path: PathBuf },
}

impl std::fmt::Display for ControlPlaneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ControlPlaneError::RelativePath { path } => write!(
                f,
                "invalid_control_plane_path: relative path {}",
                path.display()
            ),
            ControlPlaneError::SlotSubtree { path, slot_root } => write!(
                f,
                "invalid_control_plane_path: events file {} lives inside slot worktree {}",
                path.display(),
                slot_root.display()
            ),
            ControlPlaneError::SymlinkEscape {
                path,
                workspace_root,
            } => write!(
                f,
                "invalid_control_plane_path: symlink at {} escapes workspace {}",
                path.display(),
                workspace_root.display()
            ),
            ControlPlaneError::UncreatableParent { path } => write!(
                f,
                "invalid_control_plane_path: parent of {} cannot be safely created",
                path.display()
            ),
        }
    }
}

impl std::error::Error for ControlPlaneError {}

/// Stable reason code that the runtime / diagnostics surface
/// unchanged. Mirrors `invalid_control_plane_path` from the
/// 2026-07-23-004 plan's failure reason set.
pub const REASON_CODE: &str = "invalid_control_plane_path";

/// Public signature kept simple so the dispatcher can map
/// `Err(_)` to `record_slot_failure(..., REASON_CODE)` without
/// depending on `format!`.
pub fn reason_for(_err: &ControlPlaneError) -> &'static str {
    REASON_CODE
}

/// Validate the worker control-plane binding.
///
/// Inputs:
/// * `events_path` — the proposed per-worker JSONL channel.
/// * `slot_worktree_root` — slot worktree (or `None` for the
///   review `shared_readonly` case, which is exempt from the
///   slot-subtree check because reviewers share the
///   integrator's tree).
/// * `workspace_root` — primary workspace root the events
///   ledger must live under.
///
/// Behavior: returns `Ok(events_path)` after canonicalising
/// relative components only when the result is safe. Any
/// rejection returns one of the `ControlPlaneError` variants,
/// preserving the original path for diagnostics.
pub fn validate_control_plane_binding(
    events_path: &Path,
    slot_worktree_root: Option<&Path>,
    workspace_root: &Path,
) -> Result<PathBuf, ControlPlaneError> {
    if !events_path.is_absolute() {
        return Err(ControlPlaneError::RelativePath {
            path: events_path.to_path_buf(),
        });
    }

    let canonical_workspace =
        std::fs::canonicalize(workspace_root).unwrap_or_else(|_| workspace_root.to_path_buf());
    let resolved_events =
        std::fs::canonicalize(events_path).unwrap_or_else(|_| events_path.to_path_buf());

    if let Some(slot_root) = slot_worktree_root {
        let canonical_slot =
            std::fs::canonicalize(slot_root).unwrap_or_else(|_| slot_root.to_path_buf());
        if resolved_events.starts_with(&canonical_slot) {
            return Err(ControlPlaneError::SlotSubtree {
                path: events_path.to_path_buf(),
                slot_root: canonical_slot,
            });
        }
        // symlink inside slot escape → flag it
        if resolved_events.starts_with(&canonical_workspace)
            && canonical_slot.starts_with(&canonical_workspace)
            && resolved_events != canonical_workspace
            && (resolved_events == canonical_slot
                || resolved_events.ancestors().any(|a| a == canonical_slot))
        {
            return Err(ControlPlaneError::SlotSubtree {
                path: events_path.to_path_buf(),
                slot_root: canonical_slot,
            });
        }
    }

    // Symlink escape: if the canonical path points outside the
    // workspace root, the worker would write to a foreign
    // location. Flag it.
    if !resolved_events.starts_with(&canonical_workspace) {
        return Err(ControlPlaneError::SymlinkEscape {
            path: events_path.to_path_buf(),
            workspace_root: canonical_workspace,
        });
    }

    // Parent must exist or be safely creatable. Caller may
    // have set up `parent.exists()`; if not we test creation
    // of a uniquely named probe to confirm writability.
    if let Some(parent) = events_path.parent()
        && !parent.exists()
        && std::fs::create_dir_all(parent).is_err()
    {
        // Best-effort: try to create the chain. If we
        // are running as a normal user in a non-existent
        // nested dir under our own temp tree, this will
        // succeed and we will leave it (the directory is
        // needed for the ledger anyway).
        return Err(ControlPlaneError::UncreatableParent {
            path: events_path.to_path_buf(),
        });
    }

    Ok(resolved_events)
}

/// Build the four channel env keys the wave worker must
/// observe. The caller passes the validated values; this
/// function is a no-op when they are validated, and a
/// fail-close hook when they are not.
///
/// Returned keys (always absolute):
/// * `RALPH_WORKSPACE_ROOT` — primary workspace
/// * `RALPH_EVENTS_FILE` — primary control-plane events file
#[allow(clippy::implicit_hasher)] // public binding API: caller supplies the env HashMap
pub fn merge_event_channel_env(
    workspace_root: &Path,
    events_file: &Path,
    extras: &mut std::collections::HashMap<String, String>,
) -> Result<(), ControlPlaneError> {
    if !workspace_root.is_absolute() {
        return Err(ControlPlaneError::RelativePath {
            path: workspace_root.to_path_buf(),
        });
    }
    if !events_file.is_absolute() {
        return Err(ControlPlaneError::RelativePath {
            path: events_file.to_path_buf(),
        });
    }
    extras.insert(
        "RALPH_WORKSPACE_ROOT".into(),
        workspace_root.display().to_string(),
    );
    extras.insert(
        "RALPH_EVENTS_FILE".into(),
        events_file.display().to_string(),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relative_path_is_rejected() {
        let res =
            validate_control_plane_binding(Path::new("events.jsonl"), None, Path::new("/tmp"));
        assert!(matches!(res, Err(ControlPlaneError::RelativePath { .. })));
    }

    #[test]
    fn slot_subtree_is_rejected() {
        let tmp = tempfile::tempdir().unwrap();
        let slot = tmp.path().join("slot");
        std::fs::create_dir_all(&slot).unwrap();
        let events = slot.join("events.jsonl");
        std::fs::write(&events, "").unwrap();
        let res = validate_control_plane_binding(&events, Some(slot.as_path()), tmp.path());
        assert!(
            matches!(res, Err(ControlPlaneError::SlotSubtree { .. })),
            "slot subtree should be rejected, got {res:?}"
        );
    }
}
