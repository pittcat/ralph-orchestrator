//! Canonicalize-then-compare path containment.
//!
//! `Path::starts_with` already compares whole components, but it
//! compares them on the *unresolved* path. A symlink inside the
//! workspace can therefore point anywhere and still pass. This
//! helper canonicalizes both sides first (resolving every symlink in
//! the chain) and then walks components, so containment holds only
//! for real filesystem descendants.
//!
//! When `candidate` does not exist yet (e.g. an events file that the
//! runtime is about to create) the closest existing ancestor is
//! canonicalized instead, which still resolves any symlink used to
//! redirect an intermediate component out of the workspace.

use std::path::{Component, Path, PathBuf};

/// `Ok(true)` when `candidate` resolves to `workspace` itself or a
/// descendant of it; `Ok(false)` when it escapes. Canonicalization
/// failures are propagated so callers fail closed.
pub fn path_within_workspace(workspace: &Path, candidate: &Path) -> std::io::Result<bool> {
    let ws = workspace.canonicalize()?;
    let cand = canonicalize_existing_ancestor(candidate)?;
    Ok(components_contained(&ws, &cand))
}

/// Canonicalize `path`, falling back to the closest existing
/// ancestor when the leaf (or several leaves) do not exist yet. The
/// non-existent tail components are re-appended verbatim after
/// rejecting `..` so they cannot be used to climb out.
fn canonicalize_existing_ancestor(path: &Path) -> std::io::Result<PathBuf> {
    match path.canonicalize() {
        Ok(p) => Ok(p),
        Err(err) if err.kind() != std::io::ErrorKind::NotFound => Err(err),
        Err(err) => canonicalize_with_tail(path, err),
    }
}

/// Walk up from `path` until we hit an existing ancestor, canonicalize
/// that, and re-append the non-existent tail components. The
/// `original_err` is propagated if we walk past the filesystem root.
fn canonicalize_with_tail(path: &Path, original_err: std::io::Error) -> std::io::Result<PathBuf> {
    let mut tail: Vec<Component<'_>> = Vec::new();
    let mut cursor = path;
    loop {
        let Some(parent) = cursor.parent() else {
            return Err(original_err);
        };
        let Some(name) = cursor.file_name() else {
            return Err(original_err);
        };
        tail.push(Component::Normal(name));
        match parent.canonicalize() {
            Ok(mut base) => {
                for comp in tail.iter().rev() {
                    base.push(comp.as_os_str());
                }
                return Ok(base);
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                cursor = parent;
            }
            Err(e) => return Err(e),
        }
    }
}

/// Component-wise containment: every component of `workspace` must
/// match the corresponding component of `candidate`, and `candidate`
/// must not be shorter than `workspace`.
fn components_contained(workspace: &Path, candidate: &Path) -> bool {
    let mut ws = workspace.components();
    let mut cand = candidate.components();
    loop {
        match (ws.next(), cand.next()) {
            (None, _) => return true,
            (Some(_), None) => return false,
            (Some(a), Some(b)) if a == b => {}
            (Some(_), Some(_)) => return false,
        }
    }
}
