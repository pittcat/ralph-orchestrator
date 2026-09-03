//! 2026-09-03-0959 plan U7 (R7; S8-S11; D7-D9; E10-E12):
//! changed-path authorisation guard.
//!
//! Every Unit's reviewed diff is validated TWICE against the same
//! allowlist:
//!   1. **Review entry** (U6 + the integrator's pre-flight): the
//!      set of changed paths must satisfy shape + symlink /
//! submodule / `.git` rules AND fall inside the lane allowlist.
//!   2. **Lane lock acquire** (U7): the integrator re-reads the
//!      diff right before grabbing the per-target lease; if any
//!      path has drifted (a hook added a `.git`-prefixed file,
//!      a symlink target was edited, etc.) the lock is refused
//!      and the candidate is rejected before any merge work is
//!      performed.
//!
//! Two checks, identical gate — the second is what makes the
//! lane safe under the hostile agent case (an agent process that
//! keeps mutating its own worktree between review-accept and
//! integrator-takeover).
//!
//! The guard is a pure data structure: it does NOT touch the
//! filesystem, git, or process state. The caller hands in the
//! diff output (already produced by `git diff-tree` outside
//! the guard); the guard's job is to assert shape + policy.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};

/// Reason an authorisation gate rejected the changed-path set.
///
/// Single enum so a caller can match exhaustively on a single
/// discriminant instead of juggling multiple result types.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangedPathRejection {
    /// Path is absolute, has a parent escape, or has other
    /// shape problems. Detected at parse time by
    /// [`ChangedPathSet::from_diff_entries`] /
    /// [`ChangedPathSet::from_diff_paths`].
    BadPathShape(String),
    /// Path falls outside the lane allowlist (no allowlist
    /// root is a prefix of the path).
    OutsideAllowlist(String),
    /// Path starts with a forbidden prefix (`.git`, `target/`,
    /// `node_modules/`, etc.).
    ForbiddenPath(String),
    /// Path is a symlink (git mode 120000) — the lane refuses
    /// to integrate anything that resolved through a symlink
    /// because the resolved target could differ across the
    /// host / lane environment.
    SymlinkPath(String),
    /// Path is a submodule (git mode 160000 / gitlink) — the
    /// integrator cannot squash a submodule pointer.
    SubmodulePath(String),
}

impl std::fmt::Display for ChangedPathRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadPathShape(p) => write!(f, "bad path shape: {p}"),
            Self::OutsideAllowlist(p) => write!(f, "outside allowlist: {p}"),
            Self::ForbiddenPath(p) => write!(f, "forbidden path: {p}"),
            Self::SymlinkPath(p) => write!(f, "symlink path: {p}"),
            Self::SubmodulePath(p) => write!(f, "submodule path: {p}"),
        }
    }
}

impl std::error::Error for ChangedPathRejection {}

/// Parse-time error returned when a single diff entry is
/// malformed. Distinguished from [`ChangedPathRejection`] (which
/// is the per-gate outcome) so callers can short-circuit before
/// even building the set.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ChangedPathError {
    #[error("path shape invalid: {0}")]
    BadPathShape(String),
}

/// One diff entry, with shape metadata captured at parse time.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiffPathEntry {
    pub path: PathBuf,
    pub is_symlink: bool,
    pub is_submodule: bool,
}

/// The bounded, deduplicated, sorted set of changed paths the
/// guard evaluates.
///
/// Constructed via [`ChangedPathSet::from_diff_paths`] (simple
/// list) or [`ChangedPathSet::from_diff_entries`] (with
/// symlink/submodule metadata). Authorised via
/// [`ChangedPathSet::is_clean_within`]. Cross-checked via
/// [`ChangedPathSet::intersects`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ChangedPathSet {
    entries: BTreeSet<DiffPathEntry>,
}

/// Forbidden top-level prefixes. Path is rejected at gate time
/// if its first component is one of these — even if it would
/// otherwise fall inside the allowlist. The list is intentionally
/// small and stable so it can be reviewed as policy.
pub const FORBIDDEN_TOP_LEVEL_PREFIXES: &[&str] =
    &[".git", "target", "node_modules", ".cargo", ".idea", ".vscode"];

impl ChangedPathSet {
    /// Empty set — useful for tests + the no-op integration case.
    pub fn empty() -> Self {
        Self {
            entries: BTreeSet::new(),
        }
    }

    /// Construct from a list of plain repo-relative paths. All
    /// entries are treated as regular files / dirs (no symlink
    /// or submodule metadata).
    pub fn from_diff_paths<I, P>(paths: I) -> Result<Self, ChangedPathError>
    where
        I: IntoIterator<Item = P>,
        P: AsRef<Path>,
    {
        let iter = paths.into_iter();
        let mut entries: BTreeSet<DiffPathEntry> = BTreeSet::new();
        for p in iter {
            let path = p.as_ref();
            validate_path_shape(path)?;
            entries.insert(DiffPathEntry {
                path: path.to_path_buf(),
                is_symlink: false,
                is_submodule: false,
            });
        }
        Ok(Self { entries })
    }

    /// Construct from structured entries (path + symlink /
    /// submodule flags). Used by the integrator when reading
    /// `git diff-tree` output. Same shape validation as the
    /// plain variant.
    pub fn from_diff_entries<I>(iter: I) -> Result<Self, ChangedPathError>
    where
        I: IntoIterator<Item = DiffPathEntry>,
    {
        let mut entries: BTreeSet<DiffPathEntry> = BTreeSet::new();
        for entry in iter {
            validate_path_shape(&entry.path)?;
            entries.insert(entry);
        }
        Ok(Self { entries })
    }

    /// Total number of unique changed paths in the set.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// True iff no entries are present.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Iterate over the (sorted, deduplicated) entries.
    pub fn entries(&self) -> impl Iterator<Item = &DiffPathEntry> {
        self.entries.iter()
    }

    /// True iff any path in `other` appears in this set. Used
    /// by sibling-detection logic to tell whether two Units
    /// share a touched file.
    pub fn intersects(&self, other: &[PathBuf]) -> bool {
        other.iter().any(|p| self.entries.iter().any(|e| &e.path == p))
    }

    /// Authorise the changed-path set against the lane
    /// allowlist. Returns the sorted, deduplicated list of
    /// paths when all checks pass.
    ///
    /// Checks (in this order):
    ///   1. No entry has `is_symlink == true`.
    ///   2. No entry has `is_submodule == true`.
    ///   3. No entry's first component is a forbidden prefix.
    ///   4. Every entry falls inside at least one allowlist root.
    pub fn is_clean_within(
        &self,
        allowlist: &[PathBuf],
    ) -> Result<Vec<PathBuf>, ChangedPathRejection> {
        let mut out: Vec<PathBuf> = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            if entry.is_symlink {
                return Err(ChangedPathRejection::SymlinkPath(
                    entry.path.display().to_string(),
                ));
            }
            if entry.is_submodule {
                return Err(ChangedPathRejection::SubmodulePath(
                    entry.path.display().to_string(),
                ));
            }
            let top = entry
                .path
                .components()
                .next()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .unwrap_or_default();
            if FORBIDDEN_TOP_LEVEL_PREFIXES.iter().any(|p| top == *p) {
                return Err(ChangedPathRejection::ForbiddenPath(
                    entry.path.display().to_string(),
                ));
            }
            if !is_within_allowlist(&entry.path, allowlist) {
                return Err(ChangedPathRejection::OutsideAllowlist(
                    entry.path.display().to_string(),
                ));
            }
            out.push(entry.path.clone());
        }
        Ok(out)
    }
}

fn validate_path_shape(path: &Path) -> Result<(), ChangedPathError> {
    if path.is_absolute() {
        return Err(ChangedPathError::BadPathShape(format!(
            "absolute path: {}",
            path.display()
        )));
    }
    let lossy = path.to_string_lossy();
    if lossy.contains('\0') {
        return Err(ChangedPathError::BadPathShape(
            "NUL byte in path".to_string(),
        ));
    }
    if lossy.contains('\\') {
        return Err(ChangedPathError::BadPathShape(format!(
            "backslash in path: {}",
            path.display()
        )));
    }
    for c in path.components() {
        if matches!(c, Component::ParentDir) {
            return Err(ChangedPathError::BadPathShape(format!(
                "parent escape: {}",
                path.display()
            )));
        }
    }
    Ok(())
}

fn is_within_allowlist(path: &Path, allowlist: &[PathBuf]) -> bool {
    if allowlist.is_empty() {
        return false;
    }
    allowlist.iter().any(|root| path.starts_with(root))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_symlink: bool, is_submodule: bool) -> DiffPathEntry {
        DiffPathEntry {
            path: PathBuf::from(path),
            is_symlink,
            is_submodule,
        }
    }

    fn allowlist(roots: &[&str]) -> Vec<PathBuf> {
        roots.iter().map(PathBuf::from).collect()
    }

    /// U7 contract: a forbidden top-level prefix (`.git`) is
    /// rejected by the gate, even when the path would otherwise
    /// fall inside the allowlist.
    #[test]
    fn changed_path_guard_rejects_forbidden_path() {
        let set = ChangedPathSet::from_diff_entries([entry(".git/HEAD", false, false)]).unwrap();
        let err = set
            .is_clean_within(&allowlist(&[".git"]))
            .expect_err("must reject");
        match err {
            ChangedPathRejection::ForbiddenPath(p) => assert_eq!(p, ".git/HEAD"),
            other => panic!("expected ForbiddenPath, got {other:?}"),
        }
    }

    /// U7 contract: a parent-escape component (`..`) is
    /// rejected at parse time.
    #[test]
    fn changed_path_guard_rejects_traversal() {
        let err = ChangedPathSet::from_diff_paths(["src/../outside.rs"])
            .expect_err("must reject");
        match err {
            ChangedPathError::BadPathShape(msg) => {
                assert!(msg.contains("parent escape"), "msg: {msg}");
            }
        }
    }

    /// U7 contract: an absolute path is rejected at parse time.
    #[test]
    fn changed_path_guard_rejects_absolute_path() {
        let err = ChangedPathSet::from_diff_paths(["/etc/passwd"]).expect_err("must reject");
        assert!(matches!(err, ChangedPathError::BadPathShape(_)));
    }

    /// U7 contract: a symlink-typed entry (mode 120000) is
    /// rejected by the gate regardless of its position relative
    /// to the allowlist.
    #[test]
    fn changed_path_guard_rejects_symlink_chain() {
        let set = ChangedPathSet::from_diff_entries([entry("src/link_to_thing", true, false)])
            .unwrap();
        let err = set
            .is_clean_within(&allowlist(&["src"]))
            .expect_err("symlink must be rejected");
        match err {
            ChangedPathRejection::SymlinkPath(p) => {
                assert_eq!(p, "src/link_to_thing");
            }
            other => panic!("expected SymlinkPath, got {other:?}"),
        }
    }

    /// U7 contract: a submodule entry (gitlink, mode 160000) is
    /// rejected by the gate regardless of its position relative
    /// to the allowlist.
    #[test]
    fn changed_path_guard_rejects_submodule_change() {
        let set = ChangedPathSet::from_diff_entries([entry("external/lib", false, true)]).unwrap();
        let err = set
            .is_clean_within(&allowlist(&["external"]))
            .expect_err("submodule must be rejected");
        match err {
            ChangedPathRejection::SubmodulePath(p) => assert_eq!(p, "external/lib"),
            other => panic!("expected SubmodulePath, got {other:?}"),
        }
    }

    /// U7 contract: a clean allowlisted diff passes the gate
    /// and yields the sorted, deduplicated path list.
    #[test]
    fn changed_path_guard_authorizes_clean_allowlisted_diff() {
        let set = ChangedPathSet::from_diff_paths(["src/a.rs", "src/b.rs", "src/a.rs"])
            .unwrap();
        let authorized = set
            .is_clean_within(&allowlist(&["src"]))
            .expect("must authorise");
        assert_eq!(
            authorized,
            vec![PathBuf::from("src/a.rs"), PathBuf::from("src/b.rs")]
        );
    }

    /// `intersects` returns true when any of the supplied
    /// paths appears in the set; false otherwise.
    #[test]
    fn intersects_detects_overlap() {
        let set = ChangedPathSet::from_diff_paths(["src/a.rs", "src/b.rs"]).unwrap();
        assert!(set.intersects(&[PathBuf::from("src/a.rs")]));
        assert!(!set.intersects(&[PathBuf::from("src/c.rs")]));
        assert!(set.intersects(&[
            PathBuf::from("src/c.rs"),
            PathBuf::from("src/b.rs"),
        ]));
    }

    /// Path outside the allowlist is rejected with
    /// `OutsideAllowlist`, not `ForbiddenPath`.
    #[test]
    fn is_clean_within_rejects_outside_allowlist() {
        let set = ChangedPathSet::from_diff_paths(["crates/ralph-x/src/lib.rs"]).unwrap();
        let err = set
            .is_clean_within(&allowlist(&["src"]))
            .expect_err("must reject");
        assert!(matches!(err, ChangedPathRejection::OutsideAllowlist(_)));
    }

    /// Empty allowlist ⇒ every path is rejected as
    /// `OutsideAllowlist` (defence-in-depth: refuse to
    /// authorise against an empty policy).
    #[test]
    fn is_clean_within_empty_allowlist_rejects_all() {
        let set = ChangedPathSet::from_diff_paths(["src/a.rs"]).unwrap();
        let err = set.is_clean_within(&[]).expect_err("must reject");
        assert!(matches!(err, ChangedPathRejection::OutsideAllowlist(_)));
    }

    /// Backslash inside a path is rejected at parse time. The
    /// guard never accepts Windows-style separators.
    #[test]
    fn rejects_backslash_path() {
        let err = ChangedPathSet::from_diff_paths(["src\\bad.rs"]).expect_err("must reject");
        assert!(matches!(err, ChangedPathError::BadPathShape(_)));
    }

    /// Two entries sharing the same path collapse to one; the
    /// resulting set has `len() == 1` and is cleaned by the gate.
    #[test]
    fn deduplicates_entries() {
        let set = ChangedPathSet::from_diff_entries([
            entry("src/a.rs", false, false),
            entry("src/a.rs", false, false),
        ])
        .unwrap();
        assert_eq!(set.len(), 1);
        assert!(set.is_clean_within(&allowlist(&["src"])).is_ok());
    }
}