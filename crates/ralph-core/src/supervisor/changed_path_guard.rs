//! 2026-09-03-0959 plan U7 (R7; S8-S11; D7-D9; E10-E12):
//! changed-path authorisation guard.
//!
//! Every Unit's reviewed diff is validated TWICE against the same
//! allowlist:
//!   1. **Review entry** (U6 + the integrator's pre-flight): the
//!      set of changed paths must satisfy shape + symlink /
//! submodule / `.git` / `.ralph` rules AND fall inside the lane
//! allowlist.
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
    /// Path is inside the lane allowlist but NOT in the job's
    /// declared changed-path set (R18/D23/S18 bidirectional
    /// authorisation: a job declaring `foo.rs` but writing
    /// `bar.rs` inside the allowlist is rejected). `job` is the
    /// owning job identifier (e.g. unit id) for diagnostics.
    OutsideDeclared { path: String, job: String },
    /// An allowlist / declared-set root is itself malformed:
    /// empty (`PathBuf::from("")` has zero components and
    /// `starts_with("")` would be true for EVERY path —
    /// fail-open), absolute, or containing a parent escape.
    /// Fail-closed at gate time.
    BadAllowlistRoot(String),
}

impl std::fmt::Display for ChangedPathRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadPathShape(p) => write!(f, "bad path shape: {p}"),
            Self::OutsideAllowlist(p) => write!(f, "outside allowlist: {p}"),
            Self::ForbiddenPath(p) => write!(f, "forbidden path: {p}"),
            Self::SymlinkPath(p) => write!(f, "symlink path: {p}"),
            Self::SubmodulePath(p) => write!(f, "submodule path: {p}"),
            Self::OutsideDeclared { path, job } => {
                write!(f, "path outside declared set: {path} (job: {job})")
            }
            Self::BadAllowlistRoot(p) => write!(f, "bad allowlist root: {p}"),
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
    /// A `git diff-tree --name-status` record carried a status
    /// letter the guard refuses to map (e.g. `??` untracked, `U`
    /// unmerged). Fail-closed: unknown statuses must not silently
    /// degrade to `Modified`.
    #[error("unknown diff status: {0}")]
    UnknownStatus(String),
}

/// Change kind of one diff entry. `Deleted` and `Renamed` are
/// first-class: the integration gate must be able to tell a
/// deleted file from a modified one, and a rename carries TWO
/// paths (source + target) that are both authorised against the
/// allowlist / forbidden / declared-set checks.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum DiffStatus {
    /// New file (git status `A`).
    Added,
    /// Content change to an existing file (git status `M`).
    Modified,
    /// File removed (git status `D`). `path` is the removed
    /// path; it must still fall inside the allowlist — deleting
    /// out-of-lane files is rejected like writing them.
    Deleted,
    /// File moved (git status `R*`, or `C*` copy). `path` on the
    /// owning entry is the TARGET; `from` is the SOURCE. Both
    /// must pass every gate check.
    Renamed { from: PathBuf },
}

/// One diff entry, with shape metadata captured at parse time.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiffPathEntry {
    pub path: PathBuf,
    pub status: DiffStatus,
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
pub const FORBIDDEN_TOP_LEVEL_PREFIXES: &[&str] = &[
    ".git",
    ".ralph",
    "target",
    "node_modules",
    ".cargo",
    ".idea",
    ".vscode",
];

impl ChangedPathSet {
    /// Empty set — useful for tests + the no-op integration case.
    pub fn empty() -> Self {
        Self {
            entries: BTreeSet::new(),
        }
    }

    /// Construct from a list of plain repo-relative paths. All
    /// entries are treated as regular, modified files (no symlink
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
                status: DiffStatus::Modified,
                is_symlink: false,
                is_submodule: false,
            });
        }
        Ok(Self { entries })
    }

    /// Construct from structured entries (path + status +
    /// symlink / submodule flags). Used by the integrator when
    /// reading `git diff-tree` output. Same shape validation as
    /// the plain variant; a rename's SOURCE path is validated
    /// too.
    pub fn from_diff_entries<I>(iter: I) -> Result<Self, ChangedPathError>
    where
        I: IntoIterator<Item = DiffPathEntry>,
    {
        let mut entries: BTreeSet<DiffPathEntry> = BTreeSet::new();
        for entry in iter {
            validate_path_shape(&entry.path)?;
            if let DiffStatus::Renamed { from } = &entry.status {
                validate_path_shape(from)?;
            }
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
        other
            .iter()
            .any(|p| self.entries.iter().any(|e| &e.path == p))
    }

    /// Authorise the changed-path set against the lane
    /// allowlist AND the job's declared changed-path set. Returns
    /// the sorted, deduplicated list of paths when all checks pass.
    ///
    /// `declared_paths` is the job's declared changed-set (R18/D23/S18
    /// bidirectional authorisation): every actual changed path must
    /// be `⊆ declared_paths` as well as `⊆ allowlist`. `job` is the
    /// owning job identifier (e.g. unit id) embedded in an
    /// [`ChangedPathRejection::OutsideDeclared`] rejection for
    /// diagnostics.
    ///
    /// Checks (in this order):
    ///   1. Every allowlist / declared-set root is well-formed
    ///      (non-empty, relative, no parent escape). A malformed
    ///      root is `BadAllowlistRoot` — fail closed, because
    ///      `PathBuf::from("")` would otherwise make
    ///      `starts_with("")` true for EVERY path (fail-open).
    ///   2. No entry has `is_symlink == true`.
    ///   3. No entry has `is_submodule == true`.
    ///   4. No entry's first component is a forbidden prefix.
    ///   5. Every entry falls inside at least one allowlist root.
    ///   6. Every entry falls inside at least one declared path
    ///      (prefix match, same semantics as the allowlist check).
    ///      Empty `declared_paths` + non-empty actual ⇒ fail closed.
    ///
    /// `Deleted` entries are authorised by their (removed) path:
    /// deleting a path outside the allowlist is rejected exactly
    /// like writing it. `Renamed` entries authorise BOTH the
    /// source (`from`) and the target (`path`) through checks
    /// 4-6; either one failing rejects the whole set.
    pub fn is_clean_within(
        &self,
        allowlist: &[PathBuf],
        declared_paths: &[PathBuf],
        job: &str,
    ) -> Result<Vec<PathBuf>, ChangedPathRejection> {
        validate_roots(allowlist)?;
        validate_roots(declared_paths)?;
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
            authorise_path(&entry.path, allowlist, declared_paths, job)?;
            if let DiffStatus::Renamed { from } = &entry.status {
                authorise_path(from, allowlist, declared_paths, job)?;
            }
            out.push(entry.path.clone());
        }
        Ok(out)
    }
}

/// Run the forbidden-prefix + allowlist + declared-set checks
/// for one path (rename entries call this twice: target and
/// source).
fn authorise_path(
    path: &Path,
    allowlist: &[PathBuf],
    declared_paths: &[PathBuf],
    job: &str,
) -> Result<(), ChangedPathRejection> {
    let top = path
        .components()
        .next()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .unwrap_or_default();
    if FORBIDDEN_TOP_LEVEL_PREFIXES.iter().any(|p| top == *p) {
        return Err(ChangedPathRejection::ForbiddenPath(
            path.display().to_string(),
        ));
    }
    if !is_within_allowlist(path, allowlist) {
        return Err(ChangedPathRejection::OutsideAllowlist(
            path.display().to_string(),
        ));
    }
    // U8 (R18/D23/S18): bidirectional authorisation. A job
    // declaring `foo.rs` but writing `bar.rs` (still inside
    // the lane allowlist) is rejected here. Empty declared
    // set + non-empty actual ⇒ fail closed.
    if !is_within_allowlist(path, declared_paths) {
        return Err(ChangedPathRejection::OutsideDeclared {
            path: path.display().to_string(),
            job: job.to_string(),
        });
    }
    Ok(())
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

/// Fail-closed root validation for allowlist / declared sets.
/// A root must be a relative path with at least one NORMAL
/// component: `PathBuf::from("")` (or `"."`) has no usable
/// prefix and `starts_with("")` would be true for every path
/// (fail-open); absolute roots and `..` escapes are policy
/// bugs, not valid lane prefixes.
fn validate_roots(roots: &[PathBuf]) -> Result<(), ChangedPathRejection> {
    for root in roots {
        let mut components = root.components();
        match components.next() {
            Some(Component::Normal(_)) => {}
            _ => {
                return Err(ChangedPathRejection::BadAllowlistRoot(format!(
                    "allowlist root must be a non-empty relative path: {}",
                    root.display()
                )));
            }
        }
        for c in components {
            if !matches!(c, Component::Normal(_)) {
                return Err(ChangedPathRejection::BadAllowlistRoot(format!(
                    "allowlist root has a non-normal component: {}",
                    root.display()
                )));
            }
        }
    }
    Ok(())
}

/// Parse `git diff-tree --no-commit-id --name-status -z -r`
/// output (the same `-z` record shape `git status --porcelain -z`
/// emits for the index column) into entries.
///
/// Record layout with `-z`: `<status>\0<path>\0`, except renames
/// / copies which carry TWO paths: `<status>\0<from>\0<to>\0`.
/// Status mapping: `A` → [`DiffStatus::Added`], `M` →
/// [`DiffStatus::Modified`], `D` → [`DiffStatus::Deleted`],
/// `R*` / `C*` → [`DiffStatus::Renamed`] (target in `path`,
/// source in `from`; both shape-validated by
/// [`ChangedPathSet::from_diff_entries`]). `T` (typechange)
/// maps to `Modified`. Anything else (`??` untracked, `U`
/// unmerged, unknown letters) fails closed with
/// [`ChangedPathError::UnknownStatus`].
///
/// Symlink / submodule flags are NOT derivable from
/// `--name-status` (no mode column); callers needing them must
/// parse `--raw` mode fields and set the flags themselves.
pub fn parse_name_status_z(raw: &[u8]) -> Result<Vec<DiffPathEntry>, ChangedPathError> {
    let mut entries = Vec::new();
    let mut fields = raw.split(|b| *b == 0).filter(|f| !f.is_empty());
    while let Some(status) = fields.next() {
        let status = std::str::from_utf8(status)
            .map_err(|_| ChangedPathError::UnknownStatus("non-UTF-8 status".to_string()))?;
        let letter = status.chars().next().unwrap_or('\0');
        let mut next_path = || -> Result<PathBuf, ChangedPathError> {
            let field = fields
                .next()
                .ok_or_else(|| ChangedPathError::BadPathShape("truncated -z record".to_string()))?;
            let s = std::str::from_utf8(field)
                .map_err(|_| ChangedPathError::BadPathShape("non-UTF-8 path".to_string()))?;
            Ok(PathBuf::from(s))
        };
        let entry = match letter {
            'A' => DiffPathEntry {
                path: next_path()?,
                status: DiffStatus::Added,
                is_symlink: false,
                is_submodule: false,
            },
            'M' | 'T' => DiffPathEntry {
                path: next_path()?,
                status: DiffStatus::Modified,
                is_symlink: false,
                is_submodule: false,
            },
            'D' => DiffPathEntry {
                path: next_path()?,
                status: DiffStatus::Deleted,
                is_symlink: false,
                is_submodule: false,
            },
            'R' | 'C' => {
                let from = next_path()?;
                let to = next_path()?;
                DiffPathEntry {
                    path: to,
                    status: DiffStatus::Renamed { from },
                    is_symlink: false,
                    is_submodule: false,
                }
            }
            other => {
                return Err(ChangedPathError::UnknownStatus(other.to_string()));
            }
        };
        entries.push(entry);
    }
    Ok(entries)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_symlink: bool, is_submodule: bool) -> DiffPathEntry {
        DiffPathEntry {
            path: PathBuf::from(path),
            status: DiffStatus::Modified,
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
            .is_clean_within(&allowlist(&[".git"]), &declared(&[".git"]), "U1")
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
        let err = ChangedPathSet::from_diff_paths(["src/../outside.rs"]).expect_err("must reject");
        match err {
            ChangedPathError::BadPathShape(msg) => {
                assert!(msg.contains("parent escape"), "msg: {msg}");
            }
            other => panic!("expected BadPathShape, got {other:?}"),
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
        let set =
            ChangedPathSet::from_diff_entries([entry("src/link_to_thing", true, false)]).unwrap();
        let err = set
            .is_clean_within(&allowlist(&["src"]), &declared(&["src"]), "U1")
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
            .is_clean_within(&allowlist(&["external"]), &declared(&["external"]), "U1")
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
        let set = ChangedPathSet::from_diff_paths(["src/a.rs", "src/b.rs", "src/a.rs"]).unwrap();
        let authorized = set
            .is_clean_within(&allowlist(&["src"]), &declared(&["src"]), "U1")
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
        assert!(set.intersects(&[PathBuf::from("src/c.rs"), PathBuf::from("src/b.rs"),]));
    }

    /// Path outside the allowlist is rejected with
    /// `OutsideAllowlist`, not `ForbiddenPath`.
    #[test]
    fn is_clean_within_rejects_outside_allowlist() {
        let set = ChangedPathSet::from_diff_paths(["crates/ralph-x/src/lib.rs"]).unwrap();
        let err = set
            .is_clean_within(&allowlist(&["src"]), &declared(&["src"]), "U1")
            .expect_err("must reject");
        assert!(matches!(err, ChangedPathRejection::OutsideAllowlist(_)));
    }

    /// Empty allowlist ⇒ every path is rejected as
    /// `OutsideAllowlist` (defence-in-depth: refuse to
    /// authorise against an empty policy).
    #[test]
    fn is_clean_within_empty_allowlist_rejects_all() {
        let set = ChangedPathSet::from_diff_paths(["src/a.rs"]).unwrap();
        let err = set
            .is_clean_within(&[], &declared(&["src"]), "U1")
            .expect_err("must reject");
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
        assert!(
            set.is_clean_within(&allowlist(&["src"]), &declared(&["src/a.rs"]), "U1")
                .is_ok()
        );
    }

    // =========================================================================
    // U8 (R18/D23/S18) bidirectional authorisation: a job declaring
    // `[foo.rs]` but writing `bar.rs` (inside the lane allowlist) must be
    // rejected with `OutsideDeclared`. Before U8 the guard only checked
    // `actual ⊆ lane-allowlist` (single-direction); the second check
    // `actual ⊆ job-declared-paths` closes the A1 gap.
    // =========================================================================

    fn declared(paths: &[&str]) -> Vec<PathBuf> {
        paths.iter().map(PathBuf::from).collect()
    }

    /// U8 happy path: actual `[src/a.rs]`, declared `[src/a.rs]`,
    /// allowlist contains `src` → clean (authorised both ways).
    #[test]
    fn u8_clean_when_actual_matches_declared() {
        let set = ChangedPathSet::from_diff_paths(["src/a.rs"]).unwrap();
        let authorized = set
            .is_clean_within(&allowlist(&["src"]), &declared(&["src/a.rs"]), "U1")
            .expect("must authorise");
        assert_eq!(authorized, vec![PathBuf::from("src/a.rs")]);
    }

    /// U8 happy path: a declared directory (`src/`) authorises any
    /// actual path under it (`src/anything.rs`), mirroring the
    /// allowlist prefix-matching semantics.
    #[test]
    fn u8_declared_directory_authorises_descendants() {
        let set = ChangedPathSet::from_diff_paths(["src/a.rs", "src/sub/b.rs"]).unwrap();
        let authorized = set
            .is_clean_within(&allowlist(&["src"]), &declared(&["src"]), "U1")
            .expect("must authorise");
        assert_eq!(
            authorized,
            vec![PathBuf::from("src/a.rs"), PathBuf::from("src/sub/b.rs")]
        );
    }

    /// A1 RED→GREEN: a job declaring `[src/a.rs]` but writing
    /// `src/b.rs` (still inside the lane allowlist `["src"]`) is
    /// rejected with `OutsideDeclared`. Before U8 this passed
    /// (single-direction authorisation gap).
    #[test]
    fn u8_rejects_actual_inside_allowlist_but_outside_declared() {
        let set = ChangedPathSet::from_diff_paths(["src/b.rs"]).unwrap();
        let err = set
            .is_clean_within(&allowlist(&["src"]), &declared(&["src/a.rs"]), "U1")
            .expect_err("must reject undeclared path");
        match err {
            ChangedPathRejection::OutsideDeclared { path, job: _ } => {
                assert_eq!(path, "src/b.rs");
            }
            other => panic!("expected OutsideDeclared, got {other:?}"),
        }
    }

    /// U8 edge: actual path inside the lane-allowlist but NOT in the
    /// declared set → `OutsideDeclared` (was clean before U8).
    #[test]
    fn u8_rejects_path_in_allowlist_not_in_declared() {
        let set = ChangedPathSet::from_diff_paths(["src/c.rs"]).unwrap();
        let err = set
            .is_clean_within(
                &allowlist(&["src"]),
                &declared(&["src/a.rs", "src/b.rs"]),
                "U1",
            )
            .expect_err("must reject");
        assert!(matches!(err, ChangedPathRejection::OutsideDeclared { .. }));
    }

    /// U8 edge: empty `declared_paths` + non-empty actual →
    /// `OutsideDeclared` (fail closed). A job that declared nothing
    /// must not be authorised for any actual change.
    #[test]
    fn u8_empty_declared_with_nonempty_actual_fails_closed() {
        let set = ChangedPathSet::from_diff_paths(["src/a.rs"]).unwrap();
        let err = set
            .is_clean_within(&allowlist(&["src"]), &declared(&[]), "U1")
            .expect_err("must fail closed");
        assert!(matches!(err, ChangedPathRejection::OutsideDeclared { .. }));
    }

    // =========================================================================
    // Status-aware gates (rename / delete / untracked) + allowlist-root
    // fail-closed + `.ralph` forbidden prefix.
    // =========================================================================

    fn renamed(from: &str, to: &str) -> DiffPathEntry {
        DiffPathEntry {
            path: PathBuf::from(to),
            status: DiffStatus::Renamed {
                from: PathBuf::from(from),
            },
            is_symlink: false,
            is_submodule: false,
        }
    }

    fn deleted(path: &str) -> DiffPathEntry {
        DiffPathEntry {
            path: PathBuf::from(path),
            status: DiffStatus::Deleted,
            is_symlink: false,
            is_submodule: false,
        }
    }

    /// Rename whose TARGET lands in a forbidden area is rejected,
    /// even though the source is allowlisted.
    #[test]
    fn rename_to_forbidden_area_rejected() {
        let set = ChangedPathSet::from_diff_entries([renamed("src/a.rs", ".git/hooks/pre-push")])
            .unwrap();
        let err = set
            .is_clean_within(&allowlist(&["src"]), &declared(&["src"]), "U1")
            .expect_err("rename into .git must be rejected");
        assert!(matches!(err, ChangedPathRejection::ForbiddenPath(_)));
    }

    /// Rename whose SOURCE sits in a forbidden area is rejected,
    /// even though the target is allowlisted — the source path
    /// goes through the same checks as the target.
    #[test]
    fn rename_from_forbidden_area_rejected() {
        let set = ChangedPathSet::from_diff_entries([renamed(".ralph/events.jsonl", "src/a.rs")])
            .unwrap();
        let err = set
            .is_clean_within(
                &allowlist(&["src", ".ralph"]),
                &declared(&["src", ".ralph"]),
                "U1",
            )
            .expect_err("rename from .ralph must be rejected");
        assert!(matches!(err, ChangedPathRejection::ForbiddenPath(_)));
    }

    /// Rename whose target escapes the allowlist is rejected as
    /// `OutsideAllowlist` (target must be in-lane).
    #[test]
    fn rename_to_outside_allowlist_rejected() {
        let set = ChangedPathSet::from_diff_entries([renamed("src/a.rs", "docs/b.md")]).unwrap();
        let err = set
            .is_clean_within(&allowlist(&["src"]), &declared(&["src"]), "U1")
            .expect_err("must reject");
        assert!(matches!(err, ChangedPathRejection::OutsideAllowlist(_)));
    }

    /// Rename fully inside the allowlist + declared set is
    /// allowed (same semantics as a modify).
    #[test]
    fn rename_within_allowlist_allowed() {
        let set = ChangedPathSet::from_diff_entries([renamed("src/a.rs", "src/b.rs")]).unwrap();
        let authorised = set
            .is_clean_within(&allowlist(&["src"]), &declared(&["src"]), "U1")
            .expect("in-lane rename must pass");
        assert_eq!(authorised, vec![PathBuf::from("src/b.rs")]);
    }

    /// Delete of an allowlisted + declared path is allowed;
    /// delete of an out-of-allowlist path is rejected.
    #[test]
    fn delete_allowed_in_allowlist_rejected_outside() {
        let set = ChangedPathSet::from_diff_entries([deleted("src/a.rs")]).unwrap();
        set.is_clean_within(&allowlist(&["src"]), &declared(&["src"]), "U1")
            .expect("in-lane delete must pass");

        let err = set
            .is_clean_within(&allowlist(&["crates"]), &declared(&["crates"]), "U1")
            .expect_err("out-of-lane delete must be rejected");
        assert!(matches!(err, ChangedPathRejection::OutsideAllowlist(_)));
    }

    /// Hostile sequence: file deleted, then a symlink re-created
    /// under the same name. The symlink entry must still trip the
    /// symlink gate (delete + recreate do not launder it).
    #[test]
    fn delete_then_recreate_as_symlink_rejected() {
        let set = ChangedPathSet::from_diff_entries([
            deleted("src/link"),
            entry("src/link", true, false),
        ])
        .unwrap();
        let err = set
            .is_clean_within(&allowlist(&["src"]), &declared(&["src"]), "U1")
            .expect_err("recreated symlink must be rejected");
        assert!(matches!(err, ChangedPathRejection::SymlinkPath(_)));
    }

    /// Empty allowlist root (`PathBuf::from("")`) has zero
    /// components; `starts_with("")` would be true for EVERY
    /// path. The gate must fail closed with `BadAllowlistRoot`
    /// instead of authorising everything.
    #[test]
    fn empty_allowlist_root_fails_closed() {
        let set = ChangedPathSet::from_diff_paths(["src/a.rs"]).unwrap();
        let err = set
            .is_clean_within(&[PathBuf::from("")], &declared(&["src"]), "U1")
            .expect_err("empty root must fail closed");
        assert!(matches!(err, ChangedPathRejection::BadAllowlistRoot(_)));
        // Same hole in the declared set.
        let err = set
            .is_clean_within(&allowlist(&["src"]), &[PathBuf::from(".")], "U1")
            .expect_err("dot root must fail closed");
        assert!(matches!(err, ChangedPathRejection::BadAllowlistRoot(_)));
        // Absolute / parent-escape roots are policy bugs too.
        let err = set
            .is_clean_within(&allowlist(&["src/../src"]), &declared(&["src"]), "U1")
            .expect_err("parent-escape root must fail closed");
        assert!(matches!(err, ChangedPathRejection::BadAllowlistRoot(_)));
    }

    /// `.ralph/` is the runtime ledger directory — agent changes
    /// to runtime state files are rejected even when the
    /// allowlist would cover them.
    #[test]
    fn ralph_runtime_ledger_path_rejected() {
        let set = ChangedPathSet::from_diff_paths([".ralph/agent/tasks.jsonl"]).unwrap();
        let err = set
            .is_clean_within(&allowlist(&[".ralph"]), &declared(&[".ralph"]), "U1")
            .expect_err(".ralph must be rejected");
        match err {
            ChangedPathRejection::ForbiddenPath(p) => assert_eq!(p, ".ralph/agent/tasks.jsonl"),
            other => panic!("expected ForbiddenPath, got {other:?}"),
        }
    }

    /// `parse_name_status_z` maps A/M/D/R records correctly:
    /// rename yields one entry with target in `path` and source
    /// in `Renamed.from`.
    #[test]
    fn parse_name_status_z_maps_statuses() {
        let raw = b"M\0src/a.rs\0A\0src/new.rs\0D\0src/old.rs\0R100\0src/b.rs\0src/c.rs\0";
        let entries = parse_name_status_z(raw).expect("parse");
        assert_eq!(entries.len(), 4);
        assert_eq!(entries[0].status, DiffStatus::Modified);
        assert_eq!(entries[1].status, DiffStatus::Added);
        assert_eq!(entries[2].status, DiffStatus::Deleted);
        assert_eq!(
            entries[3].status,
            DiffStatus::Renamed {
                from: PathBuf::from("src/b.rs")
            }
        );
        assert_eq!(entries[3].path, PathBuf::from("src/c.rs"));
    }

    /// Untracked (`??`) and unknown statuses fail closed instead
    /// of degrading to `Modified`.
    #[test]
    fn parse_name_status_z_rejects_untracked_and_unknown() {
        assert!(matches!(
            parse_name_status_z(b"??\0src/untracked.rs\0"),
            Err(ChangedPathError::UnknownStatus(_))
        ));
        assert!(matches!(
            parse_name_status_z(b"X\0src/a.rs\0"),
            Err(ChangedPathError::UnknownStatus(_))
        ));
        // Truncated rename record (missing target path).
        assert!(matches!(
            parse_name_status_z(b"R100\0src/a.rs\0"),
            Err(ChangedPathError::BadPathShape(_))
        ));
    }
}
