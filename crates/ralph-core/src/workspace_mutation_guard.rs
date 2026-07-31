//! 2026-07-29-003 plan U1: workspace mutation guard for strict-read-only hats.
//! Captures a protected-state Git snapshot at activation start, compares it
//! to the end-of-activation snapshot, and reports a typed `ScopeViolation`
//! on any positive delta. Companion: `crate::preset_lint::strict_readonly_hat`.

use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

/// Why a snapshot capture failed. Caller treats every variant as fail-closed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureError {
    /// Underlying `git` command exited non-zero or printed invalid UTF-8.
    Git {
        command: String,
        stderr: String,
    },
    /// `allowed_write_paths` rule failed `validate_allowed_path`.
    InvalidAllowedPath(String),
    /// `{plan_key}` placeholder present but no/unsafe plan_key supplied.
    PlanKeyMissing,
    PlanKeyUnsafe(String),
    /// Trigger payload referenced a `*_path` field that is not a regular file.
    InvalidInputArtifact(String),
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Git { command, stderr } => write!(f, "git {command}: {stderr}"),
            Self::InvalidAllowedPath(p) => write!(f, "invalid allowed_write_path: {p}"),
            Self::PlanKeyMissing => write!(f, "allowed path uses {{plan_key}} but none supplied"),
            Self::PlanKeyUnsafe(s) => write!(f, "plan_key is not a safe single path segment: {s}"),
            Self::InvalidInputArtifact(p) => write!(f, "trigger *_path field is invalid: {p}"),
        }
    }
}

impl std::error::Error for CaptureError {}

/// Why a `validate_allowed_path` call rejected a rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PathError {
    Empty,
    Absolute,
    Backslash,
    DotSegment,
    DotGitPrefix,
    UnknownVariable(String),
    NonTrailingStarStar(String),
    UnsafePlanKey(String),
}

impl std::fmt::Display for PathError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Empty => write!(f, "empty path"),
            Self::Absolute => write!(f, "absolute path not allowed"),
            Self::Backslash => write!(f, "backslash not allowed (use forward slashes)"),
            Self::DotSegment => write!(f, "'.' or '..' segment not allowed"),
            Self::DotGitPrefix => write!(f, ".git paths are never allowed"),
            Self::UnknownVariable(v) => write!(f, "unknown variable: {v}"),
            Self::NonTrailingStarStar(p) => write!(f, "only trailing /** allowed, got: {p}"),
            Self::UnsafePlanKey(s) => write!(f, "plan_key must be a single safe path segment: {s}"),
        }
    }
}

impl std::error::Error for PathError {}

/// The protected state observed at one point in time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorkspaceMutationSnapshot {
    pub head: String,
    pub symbolic_ref: Option<String>,
    pub index_digest: String,
    pub tracked_digest: String,
    pub untracked_paths: BTreeSet<String>,
    pub git_op_sentinels: BTreeMap<&'static str, String>,
    pub input_artifact_digests: BTreeMap<String, String>,
}

/// The difference between two snapshots, after allowed-root filtering.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorkspaceMutationDelta {
    pub head_changed: bool,
    pub ref_changed: bool,
    pub index_changed: bool,
    pub tracked_changed: bool,
    pub untracked_added: Vec<String>,
    pub untracked_removed: Vec<String>,
    pub git_op_changed: BTreeSet<&'static str>,
    pub input_artifact_changed: BTreeMap<String, bool>,
}

impl WorkspaceMutationDelta {
    pub fn is_clean(&self) -> bool {
        !self.head_changed
            && !self.ref_changed
            && !self.index_changed
            && !self.tracked_changed
            && self.untracked_added.is_empty()
            && self.untracked_removed.is_empty()
            && self.git_op_changed.is_empty()
            && self
                .input_artifact_changed
                .values()
                .all(|changed| !*changed)
    }
}

/// True iff the hat config forbids BOTH `Edit` AND `Write`.
/// No new `read_only` field — the dual-deny is the SSOT (per plan §1.4 / KTD2).
pub fn is_strict_read_only(disallowed_tools: &[String]) -> bool {
    disallowed_tools.iter().any(|t| t == "Edit") && disallowed_tools.iter().any(|t| t == "Write")
}

/// Validate one `allowed_write_paths` rule. Accepts:
/// - exact relative path (`reviews/summary.md`)
/// - directory prefix ending in `/**` (`reviews/**`)
/// - single `{plan_key}` variable segment (`.ralph/forge/{plan_key}/reviews/**`)
pub fn validate_allowed_path(rule: &str) -> Result<(), PathError> {
    if rule.is_empty() {
        return Err(PathError::Empty);
    }
    if rule.starts_with('/') {
        return Err(PathError::Absolute);
    }
    if rule.contains('\\') {
        return Err(PathError::Backslash);
    }
    if rule.starts_with(".git/") || rule == ".git" || rule.starts_with(".git/**") {
        return Err(PathError::DotGitPrefix);
    }
    for seg in rule.split('/') {
        if seg == "." || seg == ".." {
            return Err(PathError::DotSegment);
        }
    }
    if rule.contains("**") && !rule.ends_with("/**") {
        return Err(PathError::NonTrailingStarStar(rule.to_string()));
    }
    // variable expansion: only `{plan_key}` allowed
    let mut var_name = String::new();
    let mut in_var = false;
    for ch in rule.chars() {
        match ch {
            '{' => {
                in_var = true;
                var_name.clear();
            }
            '}' => {
                if !in_var {
                    continue;
                }
                if var_name != "plan_key" {
                    return Err(PathError::UnknownVariable(var_name.clone()));
                }
                in_var = false;
            }
            _ if in_var => {
                var_name.push(ch);
            }
            _ => {}
        }
    }
    Ok(())
}

/// Validate that a plan_key is a single safe path segment: no `/`, no `\\`,
/// no `.`, no `..`, no empty, no leading `.`.
pub fn validate_plan_key(plan_key: &str) -> Result<(), PathError> {
    if plan_key.is_empty() {
        return Err(PathError::UnsafePlanKey(plan_key.to_string()));
    }
    if plan_key.contains('/') || plan_key.contains('\\') {
        return Err(PathError::UnsafePlanKey(plan_key.to_string()));
    }
    if plan_key == "." || plan_key == ".." {
        return Err(PathError::UnsafePlanKey(plan_key.to_string()));
    }
    if plan_key.starts_with('.') {
        return Err(PathError::UnsafePlanKey(plan_key.to_string()));
    }
    Ok(())
}

/// Expand `{plan_key}` occurrences in a validated rule. Caller MUST have
/// already run `validate_allowed_path` and `validate_plan_key`.
pub fn expand_plan_key(rule: &str, plan_key: &str) -> String {
    rule.replace("{plan_key}", plan_key)
}

/// True iff `path` is allowed by the expanded rule (after {plan_key} expansion).
/// Used by snapshot delta computation to filter allowed untracked writes.
pub fn path_allowed_by(path: &str, expanded_rules: &[String]) -> bool {
    for rule in expanded_rules {
        if let Some(prefix) = rule.strip_suffix("/**") {
            // directory prefix match: path must equal prefix or start with "prefix/"
            if path == prefix || path.starts_with(&format!("{prefix}/")) {
                return true;
            }
        } else if path == rule {
            return true;
        }
    }
    false
}

/// Compute a 64-char lowercase SHA-256 hex digest of `bytes`.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut h = Sha256::new();
    h.update(bytes);
    let digest = h.finalize();
    let mut out = String::with_capacity(64);
    for b in digest {
        out.push_str(&format!("{b:02x}"));
    }
    out
}

/// Capture the current workspace Git state at `workspace_root`.
/// `expanded_allowed_rules` is used to filter untracked paths.
/// `trigger_payload` is an optional JSON snapshot used to digest `*_path` inputs.
#[allow(clippy::too_many_arguments)]
pub fn capture_snapshot(
    workspace_root: &Path,
    expanded_allowed_rules: &[String],
    trigger_payload: Option<&serde_json::Value>,
) -> Result<WorkspaceMutationSnapshot, CaptureError> {
    fn git_capture(args: &[&str], cwd: &Path) -> Result<std::vec::Vec<u8>, CaptureError> {
        let out = std::process::Command::new("git")
            .args(args)
            .current_dir(cwd)
            .output()
            .map_err(|e| CaptureError::Git {
                command: args.join(" "),
                stderr: e.to_string(),
            })?;
        if !out.status.success() {
            return Err(CaptureError::Git {
                command: args.join(" "),
                stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
            });
        }
        Ok(out.stdout)
    }

    // 1. HEAD (full SHA, may be empty for unborn branch)
    let head_bytes = git_capture(&["rev-parse", "--verify", "HEAD"], workspace_root)?;
    let head = String::from_utf8_lossy(&head_bytes).trim().to_string();

    // 2. symbolic ref (None if detached)
    let symbolic_ref = match git_capture(&["symbolic-ref", "-q", "HEAD"], workspace_root) {
        Ok(b) => Some(String::from_utf8_lossy(&b).trim().to_string()),
        Err(_) => None,
    };

    // 3. index digest (raw NUL)
    let index_bytes = git_capture(
        &[
            "diff",
            "--cached",
            "--raw",
            "-z",
            "--no-renames",
            "HEAD",
            "--",
        ],
        workspace_root,
    )?;
    let index_digest = sha256_hex(&index_bytes);

    // 4. tracked digest (raw NUL)
    let tracked_bytes = git_capture(
        &["diff", "--raw", "-z", "--no-renames", "--"],
        workspace_root,
    )?;
    let tracked_digest = sha256_hex(&tracked_bytes);

    // 5. untracked paths (NUL-separated), filtered by allowed roots
    let untracked_raw = git_capture(
        &["ls-files", "--others", "--exclude-standard", "-z", "--"],
        workspace_root,
    )?;
    let mut untracked_paths: BTreeSet<String> = BTreeSet::new();
    for entry in untracked_raw.split(|b| *b == 0) {
        if entry.is_empty() {
            continue;
        }
        let s = std::str::from_utf8(entry)
            .map_err(|_| CaptureError::Git {
                command: "ls-files --others".into(),
                stderr: "non-UTF-8 path".into(),
            })?
            .to_string();
        if !path_allowed_by(&s, expanded_allowed_rules) {
            untracked_paths.insert(s);
        }
    }

    // 6. git-op sentinels (file digest or dir listing digest, per plan §3.1)
    let mut git_op_sentinels: BTreeMap<&'static str, String> = BTreeMap::new();
    for op in [
        "MERGE_HEAD",
        "CHERRY_PICK_HEAD",
        "REVERT_HEAD",
        "BISECT_LOG",
    ] {
        let rel = match git_capture(&["rev-parse", "--git-path", op], workspace_root) {
            Ok(b) => String::from_utf8_lossy(&b).trim().to_string(),
            Err(_) => continue,
        };
        let abs = if Path::new(&rel).is_absolute() {
            PathBuf::from(&rel)
        } else {
            workspace_root.join(&rel)
        };
        let digest = match std::fs::metadata(&abs) {
            Ok(m) if m.is_file() => match std::fs::read(&abs) {
                Ok(b) => sha256_hex(&b),
                Err(_) => continue,
            },
            Ok(_) => "<dir>".to_string(), // fail-closed later
            Err(_) => continue,           // absent: not a change
        };
        git_op_sentinels.insert(op, digest);
    }
    for op in ["rebase-merge", "rebase-apply"] {
        let rel = match git_capture(&["rev-parse", "--git-path", op], workspace_root) {
            Ok(b) => String::from_utf8_lossy(&b).trim().to_string(),
            Err(_) => continue,
        };
        let abs = if Path::new(&rel).is_absolute() {
            PathBuf::from(&rel)
        } else {
            workspace_root.join(&rel)
        };
        let digest = match std::fs::read_dir(&abs) {
            Ok(rd) => {
                let mut names: Vec<String> = rd
                    .filter_map(|e| e.ok())
                    .map(|e| e.file_name().to_string_lossy().to_string())
                    .collect();
                names.sort();
                sha256_hex(names.join("\n").as_bytes())
            }
            Err(_) => continue,
        };
        git_op_sentinels.insert(op, digest);
    }

    // 7. trigger input artifact digests (top-level *_path fields)
    let mut input_artifact_digests: BTreeMap<String, String> = BTreeMap::new();
    if let Some(payload) = trigger_payload.and_then(|v| v.as_object()) {
        for (key, val) in payload {
            if !key.ends_with("_path") {
                continue;
            }
            let Some(p) = val.as_str() else {
                continue;
            };
            let abs = if Path::new(p).is_absolute() {
                PathBuf::from(p)
            } else {
                workspace_root.join(p)
            };
            let meta = std::fs::metadata(&abs)
                .map_err(|_| CaptureError::InvalidInputArtifact(p.into()))?;
            if !meta.is_file() {
                return Err(CaptureError::InvalidInputArtifact(p.into()));
            }
            let bytes =
                std::fs::read(&abs).map_err(|_| CaptureError::InvalidInputArtifact(p.into()))?;
            input_artifact_digests.insert(key.clone(), sha256_hex(&bytes));
        }
    }

    Ok(WorkspaceMutationSnapshot {
        head,
        symbolic_ref,
        index_digest,
        tracked_digest,
        untracked_paths,
        git_op_sentinels,
        input_artifact_digests,
    })
}

/// Compare two snapshots. `expanded_allowed_rules` is used to filter untracked.
pub fn compute_delta(
    before: &WorkspaceMutationSnapshot,
    after: &WorkspaceMutationSnapshot,
    _expanded_allowed_rules: &[String],
) -> WorkspaceMutationDelta {
    let head_changed = before.head != after.head;
    let ref_changed = before.symbolic_ref != after.symbolic_ref;
    let index_changed = before.index_digest != after.index_digest;
    let tracked_changed = before.tracked_digest != after.tracked_digest;

    let mut untracked_added: Vec<String> = after
        .untracked_paths
        .difference(&before.untracked_paths)
        .cloned()
        .collect();
    untracked_added.sort();
    let mut untracked_removed: Vec<String> = before
        .untracked_paths
        .difference(&after.untracked_paths)
        .cloned()
        .collect();
    untracked_removed.sort();

    let mut git_op_changed: BTreeSet<&'static str> = BTreeSet::new();
    for (k, v) in &before.git_op_sentinels {
        if after.git_op_sentinels.get(k) != Some(v) {
            git_op_changed.insert(*k);
        }
    }
    for k in after.git_op_sentinels.keys() {
        if !before.git_op_sentinels.contains_key(k) {
            git_op_changed.insert(*k);
        }
    }

    let mut input_artifact_changed: BTreeMap<String, bool> = BTreeMap::new();
    for (k, v) in &before.input_artifact_digests {
        let changed = after.input_artifact_digests.get(k) != Some(v);
        input_artifact_changed.insert(k.clone(), changed);
    }
    for k in after.input_artifact_digests.keys() {
        if !before.input_artifact_digests.contains_key(k) {
            input_artifact_changed.insert(k.clone(), true);
        }
    }

    WorkspaceMutationDelta {
        head_changed,
        ref_changed,
        index_changed,
        tracked_changed,
        untracked_added,
        untracked_removed,
        git_op_changed,
        input_artifact_changed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn strict_read_only_detects_dual_deny() {
        assert!(is_strict_read_only(&["Edit".into(), "Write".into()]));
        assert!(!is_strict_read_only(&["Edit".into()]));
        assert!(!is_strict_read_only(&["Write".into()]));
        assert!(!is_strict_read_only(&[]));
        assert!(!is_strict_read_only(&["Edit".into(), "Bash".into()]));
    }

    #[test]
    fn validate_allowed_path_accepts_exact_and_dir_prefix() {
        assert!(validate_allowed_path("reviews/summary.md").is_ok());
        assert!(validate_allowed_path("reviews/**").is_ok());
        assert!(validate_allowed_path(".ralph/forge/{plan_key}/reviews/**").is_ok());
    }

    #[test]
    fn validate_allowed_path_rejects_illegal() {
        assert!(matches!(validate_allowed_path(""), Err(PathError::Empty)));
        assert!(matches!(
            validate_allowed_path("/abs"),
            Err(PathError::Absolute)
        ));
        assert!(matches!(
            validate_allowed_path("a\\b"),
            Err(PathError::Backslash)
        ));
        assert!(matches!(
            validate_allowed_path("a/./b"),
            Err(PathError::DotSegment)
        ));
        assert!(matches!(
            validate_allowed_path("a/../b"),
            Err(PathError::DotSegment)
        ));
        assert!(matches!(
            validate_allowed_path(".git/HEAD"),
            Err(PathError::DotGitPrefix)
        ));
        assert!(matches!(
            validate_allowed_path(".git/**"),
            Err(PathError::DotGitPrefix)
        ));
        assert!(matches!(
            validate_allowed_path("reviews/**/nested"),
            Err(PathError::NonTrailingStarStar(_))
        ));
        assert!(matches!(
            validate_allowed_path("reviews/**.bak"),
            Err(PathError::NonTrailingStarStar(_))
        ));
        assert!(matches!(
            validate_allowed_path("a/{other}/b"),
            Err(PathError::UnknownVariable(_))
        ));
    }

    #[test]
    fn plan_key_validation_rejects_unsafe_segments() {
        assert!(validate_plan_key("demo").is_ok());
        assert!(validate_plan_key("2026-07-29-003-foo").is_ok());
        assert!(validate_plan_key("").is_err());
        assert!(validate_plan_key("a/b").is_err());
        assert!(validate_plan_key("a\\b").is_err());
        assert!(validate_plan_key(".").is_err());
        assert!(validate_plan_key("..").is_err());
        assert!(validate_plan_key(".hidden").is_err());
    }

    #[test]
    fn path_allowed_by_matches_exact_and_dir_prefix() {
        let rules = vec!["reviews/**".into(), "summary.md".into()];
        assert!(path_allowed_by("reviews/a.md", &rules));
        assert!(path_allowed_by("reviews/sub/b.md", &rules));
        assert!(path_allowed_by("summary.md", &rules));
        assert!(!path_allowed_by("other/a.md", &rules));
        assert!(
            !path_allowed_by("reviews-old/a.md", &rules),
            "no string-prefix leak"
        );
    }

    #[test]
    fn expand_plan_key_substitutes_only_plan_key_variable() {
        assert_eq!(expand_plan_key("reviews/**", "demo"), "reviews/**");
        assert_eq!(
            expand_plan_key(".ralph/forge/{plan_key}/reviews/**", "demo"),
            ".ralph/forge/demo/reviews/**"
        );
    }

    #[test]
    fn sha256_hex_is_lowercase_64_char_hex() {
        let h = sha256_hex(b"hello");
        assert_eq!(h.len(), 64);
        assert!(
            h.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        );
        // sha256("hello") = 2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824
        assert_eq!(
            h,
            "2cf24dba5fb0a30e26e83b2ac5b9e29e1b161e5c1fa7425e73043362938b9824"
        );
    }

    #[test]
    fn delta_is_clean_when_no_change() {
        let snap = WorkspaceMutationSnapshot {
            head: "abc".into(),
            symbolic_ref: Some("refs/heads/main".into()),
            index_digest: "x".into(),
            tracked_digest: "y".into(),
            untracked_paths: BTreeSet::new(),
            git_op_sentinels: BTreeMap::new(),
            input_artifact_digests: BTreeMap::new(),
        };
        let d = compute_delta(&snap, &snap, &[]);
        assert!(d.is_clean());
    }

    #[test]
    fn delta_detects_head_change() {
        let a = WorkspaceMutationSnapshot {
            head: "abc".into(),
            symbolic_ref: None,
            index_digest: "x".into(),
            tracked_digest: "y".into(),
            untracked_paths: BTreeSet::new(),
            git_op_sentinels: BTreeMap::new(),
            input_artifact_digests: BTreeMap::new(),
        };
        let b = WorkspaceMutationSnapshot {
            head: "def".into(),
            ..a.clone()
        };
        let d = compute_delta(&a, &b, &[]);
        assert!(d.head_changed);
        assert!(!d.is_clean());
    }

    #[test]
    fn delta_detects_tracked_and_untracked_and_git_op_changes() {
        let a = WorkspaceMutationSnapshot {
            head: "abc".into(),
            symbolic_ref: None,
            index_digest: "x".into(),
            tracked_digest: "y".into(),
            untracked_paths: BTreeSet::new(),
            git_op_sentinels: BTreeMap::from([("MERGE_HEAD", "h1".to_string())]),
            input_artifact_digests: BTreeMap::new(),
        };
        let b = WorkspaceMutationSnapshot {
            tracked_digest: "z".into(),
            untracked_paths: BTreeSet::from(["src/sneaky.rs".into()]),
            git_op_sentinels: BTreeMap::from([
                ("MERGE_HEAD", "h2".to_string()),
                ("REVERT_HEAD", "h3".to_string()),
            ]),
            ..a.clone()
        };
        let d = compute_delta(&a, &b, &[]);
        assert!(d.tracked_changed);
        assert_eq!(d.untracked_added, vec!["src/sneaky.rs".to_string()]);
        assert!(d.git_op_changed.contains("MERGE_HEAD"));
        assert!(d.git_op_changed.contains("REVERT_HEAD"));
    }

    #[test]
    fn delta_detects_input_artifact_digest_change() {
        let a = WorkspaceMutationSnapshot {
            head: "abc".into(),
            symbolic_ref: None,
            index_digest: "x".into(),
            tracked_digest: "y".into(),
            untracked_paths: BTreeSet::new(),
            git_op_sentinels: BTreeMap::new(),
            input_artifact_digests: BTreeMap::from([(
                "readonly_evidence_path".into(),
                "d1".into(),
            )]),
        };
        let b = WorkspaceMutationSnapshot {
            input_artifact_digests: BTreeMap::from([(
                "readonly_evidence_path".into(),
                "d2".into(),
            )]),
            ..a.clone()
        };
        let d = compute_delta(&a, &b, &[]);
        assert_eq!(
            d.input_artifact_changed.get("readonly_evidence_path"),
            Some(&true)
        );
    }
}
