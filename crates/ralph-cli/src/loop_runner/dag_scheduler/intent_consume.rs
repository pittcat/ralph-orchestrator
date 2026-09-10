//! U7 intent_consume — tested-intent classification + recovery.
//!
//! Plan: docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md §7 U7.
//!
//! Classifies the durable tested intent's target state at recovery
//! time. Pure dispatch; real CAS / record handled by caller.
//!
//! U5 (fix-plan 2026-09-09-0917, F10/R5): replaces the unsafe
//! string-prefix ancestry check (`starts_with`) with a real
//! `git merge-base --is-ancestor` invocation, so an empty or
//! single-character `candidate_head` cannot bypass ancestry
//! validation. Adds SHA character-set + length validation on
//! every persisted commit OID, fail-closed at the classification
//! boundary.

// SKELETON-ONLY (per fix-plan 2026-09-09-0917-fix-forge-dag-p1-closure-plan U2 / U25):
// public types stay exposed for downstream unit tests but are not yet wired
// into production callers; U5 / U11 / U23 production replacement promotes
// this file to `PRODUCTION:` marker.
#![allow(dead_code)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;

/// Errors raised when consulting git for ancestry or field
/// validation. Variants are deliberately distinct so callers can
/// fail-closed on the unsafe-input class without dropping useful
/// git diagnostics on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitError {
    /// `git merge-base --is-ancestor` exited with a code other than
    /// 0 (ancestor) or 1 (not an ancestor). Git uses 128 for
    /// missing objects / not-a-repository, 129 for config errors,
    /// etc. Carries stderr for diagnostics.
    CommandFailed { stderr: String },
    /// The supplied path is not inside a git working tree.
    NotARepository { path: PathBuf },
    /// One of the commit OIDs failed character-set / length
    /// validation. The classifier refuses to proceed without a
    /// well-formed OID — F10 root cause.
    InvalidSha {
        field: &'static str,
        value: String,
    },
}

impl std::fmt::Display for GitError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            GitError::CommandFailed { stderr } => write!(f, "git failed: {stderr}"),
            GitError::NotARepository { path } => {
                write!(f, "not a git repository: {}", path.display())
            }
            GitError::InvalidSha { field, value } => {
                write!(f, "invalid SHA for field `{field}`: {value:?}")
            }
        }
    }
}

impl std::error::Error for GitError {}

/// Classification of the relationship between the persisted intent's
/// target and the actual target state at recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IntentTargetState {
    /// Target HEAD is what the intent originally expected.
    TargetExpected,
    /// Target HEAD has advanced to the intent's candidate.
    TargetCandidate,
    /// Target HEAD is a proven runtime descendant of the candidate.
    TargetProvenDescendant,
    /// Target belongs to a foreign repository / non-canonical identity.
    ForeignTarget,
    /// Target moved but CAS was not yet applied; candidate never landed.
    StaleUnapplied,
}

/// Inputs for intent classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IntentRecoveryInput {
    pub intent_target_branch: String,
    pub expected_head: String,
    pub candidate_head: String,
    pub current_target_head: String,
    pub cas_applied: bool,
    pub worktree_canonical_identity: String,
    /// Path to the git working tree used to verify ancestry between
    /// `candidate_head` and `current_target_head`. Required so the
    /// classifier can call `git merge-base --is-ancestor` rather
    /// than relying on unsafe string-prefix matching.
    pub repo_root: PathBuf,
}

impl IntentRecoveryInput {
    /// Reject empty / single-char / non-hex / wrong-length OIDs on
    /// every persisted SHA field. Accepts 40-char SHA-1 or 64-char
    /// SHA-256, lowercase hex only. F10 attack vector depends on
    /// empty or single-character candidates sliding through, so
    /// the validator runs *before* any branching.
    pub fn validate(&self) -> Result<(), GitError> {
        Self::validate_sha("expected_head", &self.expected_head)?;
        Self::validate_sha("candidate_head", &self.candidate_head)?;
        Self::validate_sha("current_target_head", &self.current_target_head)?;
        Ok(())
    }

    fn validate_sha(field: &'static str, value: &str) -> Result<(), GitError> {
        if !is_valid_sha(value) {
            return Err(GitError::InvalidSha {
                field,
                value: value.to_string(),
            });
        }
        Ok(())
    }
}

/// Lower-level SHA character-set + length check. Returns `true` for
/// 40-char SHA-1 or 64-char SHA-256 lowercase hex; rejects empty,
/// single-character, `0x`-prefixed, uppercase, or any other length.
pub fn is_valid_sha(value: &str) -> bool {
    let len = value.len();
    if len != 40 && len != 64 {
        return false;
    }
    value
        .bytes()
        .all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f'))
}

/// Outcome of intent recovery classification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntentRecoveryOutcome {
    /// Re-run original CAS with intent target=expected.
    RetryCasWithExpected { reason: String },
    /// Candidate already on target; record integration.
    ConsumeCasAndRecord { reason: String },
    /// Target is a proven descendant; just record (don't CAS).
    JustRecordDescendant { reason: String },
    /// Foreign target; refuse.
    RefusedForeign { reason: String },
    /// Stale intent (never landed); allow supersede with new generation.
    SupersedeStale { reason: String },
}

/// Ask git whether `ancestor` is a (transitive) ancestor of
/// `descendant`. Returns `Ok(true)` when `git merge-base
/// --is-ancestor` exits 0, `Ok(false)` on exit 1, and
/// `Err(GitError::CommandFailed)` on every other exit code
/// (git uses 128 for missing objects, 129 for configuration
/// errors, etc.). This replaces the unsafe `starts_with` string
/// check that F10 exploited.
pub fn is_git_ancestor(
    repo_root: &Path,
    ancestor: &str,
    descendant: &str,
) -> Result<bool, GitError> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .args(["merge-base", "--is-ancestor", ancestor, descendant])
        .output()
        .map_err(|e| GitError::CommandFailed {
            stderr: format!("spawn git: {e}"),
        })?;
    match output.status.code() {
        Some(0) => Ok(true),
        Some(1) => Ok(false),
        _ => Err(GitError::CommandFailed {
            stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        }),
    }
}

/// Pure dispatch: classify intent recovery outcome. Returns
/// `Err(GitError::InvalidSha)` when any persisted OID fails
/// validation (F10 fail-closed), or `Err(GitError::CommandFailed)`
/// when the ancestry probe cannot be answered by git.
pub fn classify_intent_recovery(
    input: &IntentRecoveryInput,
) -> Result<IntentRecoveryOutcome, GitError> {
    input.validate()?;
    if input.worktree_canonical_identity.is_empty() {
        return Ok(IntentRecoveryOutcome::RefusedForeign {
            reason: format!(
                "worktree identity empty; expected_head={}",
                short_sha(&input.expected_head)
            ),
        });
    }
    if input.current_target_head == input.expected_head {
        if !input.cas_applied {
            return Ok(IntentRecoveryOutcome::RetryCasWithExpected {
                reason: format!(
                    "target=expected ({}), CAS not yet applied",
                    short_sha(&input.expected_head)
                ),
            });
        }
        return Ok(IntentRecoveryOutcome::ConsumeCasAndRecord {
            reason: format!(
                "target=expected ({}), CAS applied",
                short_sha(&input.expected_head)
            ),
        });
    }
    if input.current_target_head == input.candidate_head {
        return Ok(IntentRecoveryOutcome::ConsumeCasAndRecord {
            reason: format!(
                "target=candidate ({}), CAS landed",
                short_sha(&input.candidate_head)
            ),
        });
    }
    if is_git_ancestor(
        &input.repo_root,
        &input.candidate_head,
        &input.current_target_head,
    )? {
        return Ok(IntentRecoveryOutcome::JustRecordDescendant {
            reason: format!(
                "target ({}) is a descendant of candidate ({})",
                short_sha(&input.current_target_head),
                short_sha(&input.candidate_head)
            ),
        });
    }
    if !input.cas_applied {
        return Ok(IntentRecoveryOutcome::SupersedeStale {
            reason: format!(
                "target moved ({}), CAS not applied; candidate ({}) never landed",
                short_sha(&input.current_target_head),
                short_sha(&input.candidate_head)
            ),
        });
    }
    Ok(IntentRecoveryOutcome::RefusedForeign {
        reason: format!(
            "target ({}) unrelated to candidate ({}) / expected ({})",
            short_sha(&input.current_target_head),
            short_sha(&input.candidate_head),
            short_sha(&input.expected_head)
        ),
    })
}

fn short_sha(s: &str) -> &str {
    &s[..12.min(s.len())]
}

/// Reference table: legal transitions of intent target states.
pub fn legal_intent_target_transitions() -> BTreeMap<IntentTargetState, Vec<IntentTargetState>> {
    let mut m = BTreeMap::new();
    m.insert(
        IntentTargetState::TargetExpected,
        vec![
            IntentTargetState::TargetCandidate,
            IntentTargetState::StaleUnapplied,
        ],
    );
    m.insert(
        IntentTargetState::TargetCandidate,
        vec![IntentTargetState::TargetProvenDescendant],
    );
    m.insert(IntentTargetState::TargetProvenDescendant, vec![]);
    m.insert(IntentTargetState::ForeignTarget, vec![]);
    m.insert(
        IntentTargetState::StaleUnapplied,
        vec![IntentTargetState::TargetExpected],
    );
    m
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// Initialize a tempdir git repo with three SHAs:
    ///   main: c0 → c1
    ///   side: c0 → s0  (unrelated lineage)
    /// Returns `(tmp, repo, c0, c1, s0)`.
    fn init_test_repo() -> (TempDir, PathBuf, String, String, String) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().to_path_buf();
        let run_git = |args: &[&str]| {
            let out = Command::new("git")
                .arg("-C")
                .arg(&repo)
                .args(args)
                .env("GIT_AUTHOR_NAME", "test")
                .env("GIT_AUTHOR_EMAIL", "test@test")
                .env("GIT_COMMITTER_NAME", "test")
                .env("GIT_COMMITTER_EMAIL", "test@test")
                .output()
                .expect("spawn git");
            assert!(
                out.status.success(),
                "git {:?} failed: {}",
                args,
                String::from_utf8_lossy(&out.stderr)
            );
            String::from_utf8_lossy(&out.stdout).trim().to_string()
        };
        run_git(&["init", "-q", "-b", "main"]);
        run_git(&["config", "user.email", "t@e"]);
        run_git(&["config", "user.name", "T"]);
        std::fs::write(repo.join("README.md"), "init\n").unwrap();
        run_git(&["add", "README.md"]);
        run_git(&["commit", "-q", "-m", "init"]);
        let c0 = run_git(&["rev-parse", "HEAD"]);
        std::fs::write(repo.join("extra.txt"), "extra\n").unwrap();
        run_git(&["add", "extra.txt"]);
        run_git(&["commit", "-q", "-m", "extra"]);
        let c1 = run_git(&["rev-parse", "HEAD"]);
        run_git(&["checkout", "-q", "-b", "side", &c0]);
        std::fs::write(repo.join("side.txt"), "side\n").unwrap();
        run_git(&["add", "side.txt"]);
        run_git(&["commit", "-q", "-m", "side"]);
        let s0 = run_git(&["rev-parse", "HEAD"]);
        run_git(&["checkout", "-q", "main"]);
        (tmp, repo, c0, c1, s0)
    }

    /// Build a deterministic 40-char lowercase hex SHA from any
    /// label by mixing bytes into the `[0-9a-f]` index. Only used
    /// for adversarial / variant-only tests; the descendant path
    /// uses real git-produced SHAs.
    fn fake_sha(label: &str) -> String {
        const HEX: &[u8] = b"0123456789abcdef";
        let label_bytes = label.as_bytes();
        let mut out = String::with_capacity(40);
        for i in 0..40 {
            let mix = label_bytes[i % label_bytes.len()].wrapping_add(i as u8);
            out.push(HEX[mix as usize % HEX.len()] as char);
        }
        out
    }

    fn base_input(repo: &Path, expected: &str, candidate: &str) -> IntentRecoveryInput {
        IntentRecoveryInput {
            intent_target_branch: "main".to_string(),
            expected_head: expected.to_string(),
            candidate_head: candidate.to_string(),
            current_target_head: expected.to_string(),
            cas_applied: false,
            worktree_canonical_identity: "/worktree".to_string(),
            repo_root: repo.to_path_buf(),
        }
    }

    #[test]
    fn intent_target_expected() {
        let (_tmp, repo, c0, _c1, _s0) = init_test_repo();
        let input = base_input(&repo, &c0, &c0);
        let outcome = classify_intent_recovery(&input).expect("ok");
        match outcome {
            IntentRecoveryOutcome::RetryCasWithExpected { ref reason } => {
                assert!(
                    reason.contains(&input.expected_head[..12]),
                    "reason must embed expected_head prefix, got {reason:?}"
                );
            }
            other => panic!("expected RetryCasWithExpected, got {other:?}"),
        }
    }

    #[test]
    fn intent_target_candidate() {
        let (_tmp, repo, c0, c1, _s0) = init_test_repo();
        // expected=c0, candidate=c1, current=c1 — `current == candidate`
        // must trigger ConsumeCasAndRecord (not the earlier
        // `current == expected` branch).
        let mut input = base_input(&repo, &c0, &c1);
        input.current_target_head = input.candidate_head.clone();
        let outcome = classify_intent_recovery(&input).expect("ok");
        match outcome {
            IntentRecoveryOutcome::ConsumeCasAndRecord { ref reason } => {
                assert!(
                    reason.contains(&input.candidate_head[..12]),
                    "reason must embed candidate_head prefix, got {reason:?}"
                );
                assert!(
                    reason.contains("CAS landed"),
                    "reason must mention CAS landed, got {reason:?}"
                );
            }
            other => panic!("expected ConsumeCasAndRecord, got {other:?}"),
        }
    }

    #[test]
    fn intent_target_proven_runtime_descendant() {
        let (_tmp, repo, c0, c1, _s0) = init_test_repo();
        let mut input = base_input(&repo, &c0, &c0);
        // c1 is a real descendant of c0; classifier must reach
        // git merge-base and pick `JustRecordDescendant`.
        input.expected_head = c0.clone();
        input.candidate_head = c0.clone();
        input.current_target_head = c1.clone();
        let outcome = classify_intent_recovery(&input).expect("ok");
        match outcome {
            IntentRecoveryOutcome::JustRecordDescendant { ref reason } => {
                assert!(
                    reason.contains(&c0[..12]),
                    "reason must embed ancestor (candidate) prefix, got {reason:?}"
                );
                assert!(
                    reason.contains(&c1[..12]),
                    "reason must embed descendant (current) prefix, got {reason:?}"
                );
            }
            other => panic!("expected JustRecordDescendant, got {other:?}"),
        }
    }

    #[test]
    fn foreign_target_blocks() {
        let (_tmp, repo, c0, _c1, _s0) = init_test_repo();
        let mut input = base_input(&repo, &c0, &c0);
        input.worktree_canonical_identity = "".to_string();
        let outcome = classify_intent_recovery(&input).expect("ok");
        match outcome {
            IntentRecoveryOutcome::RefusedForeign { ref reason } => {
                assert!(
                    reason.contains("empty"),
                    "reason must explain empty worktree identity, got {reason:?}"
                );
                assert!(
                    reason.contains(&input.expected_head[..12]),
                    "reason must embed expected_head prefix, got {reason:?}"
                );
            }
            other => panic!("expected RefusedForeign, got {other:?}"),
        }
    }

    #[test]
    fn stale_unapplied_intent_can_supersede() {
        let (_tmp, repo, c0, c1, s0) = init_test_repo();
        // candidate=c1 (main line), current=s0 (side branch). c1
        // is NOT an ancestor of s0 (c1 only exists on main, s0
        // descends from c0 directly), so merge-base returns
        // Ok(false) — we exercise the SupersedeStale path rather
        // than JustRecordDescendant.
        let mut input = base_input(&repo, &c0, &c1);
        input.current_target_head = s0.clone();
        input.cas_applied = false;
        let outcome = classify_intent_recovery(&input).expect("ok");
        match outcome {
            IntentRecoveryOutcome::SupersedeStale { ref reason } => {
                assert!(
                    reason.contains(&input.candidate_head[..12]),
                    "reason must embed candidate_head prefix, got {reason:?}"
                );
                assert!(
                    reason.contains("never landed"),
                    "reason must explain 'never landed', got {reason:?}"
                );
            }
            other => panic!("expected SupersedeStale, got {other:?}"),
        }
    }

    // ---- F10 adversarial inputs (fail-closed at validate) ----

    #[test]
    fn adversarial_empty_candidate_head_rejected() {
        let (_tmp, repo, c0, _c1, _s0) = init_test_repo();
        let mut input = base_input(&repo, &c0, &c0);
        input.candidate_head = "".to_string();
        let err = classify_intent_recovery(&input).expect_err("must reject empty");
        match err {
            GitError::InvalidSha { field, value } => {
                assert_eq!(field, "candidate_head");
                assert_eq!(value, "");
            }
            other => panic!("expected InvalidSha, got {other:?}"),
        }
    }

    #[test]
    fn adversarial_single_char_candidate_head_rejected() {
        let (_tmp, repo, c0, _c1, _s0) = init_test_repo();
        let mut input = base_input(&repo, &c0, &c0);
        input.candidate_head = "a".to_string();
        let err = classify_intent_recovery(&input).expect_err("must reject single char");
        match err {
            GitError::InvalidSha { field, value } => {
                assert_eq!(field, "candidate_head");
                assert_eq!(value, "a");
            }
            other => panic!("expected InvalidSha, got {other:?}"),
        }
    }

    #[test]
    fn adversarial_uppercase_sha_rejected() {
        let (_tmp, repo, c0, _c1, _s0) = init_test_repo();
        let mut input = base_input(&repo, &c0, &c0);
        input.candidate_head = "A".repeat(40);
        let err = classify_intent_recovery(&input).expect_err("must reject uppercase");
        match err {
            GitError::InvalidSha { field, .. } => {
                assert_eq!(field, "candidate_head");
            }
            other => panic!("expected InvalidSha, got {other:?}"),
        }
    }

    #[test]
    fn adversarial_short_sha_rejected() {
        let (_tmp, repo, c0, _c1, _s0) = init_test_repo();
        let mut input = base_input(&repo, &c0, &c0);
        input.current_target_head = "deadbeef".to_string();
        let err = classify_intent_recovery(&input).expect_err("must reject short sha");
        match err {
            GitError::InvalidSha { field, value } => {
                assert_eq!(field, "current_target_head");
                assert_eq!(value, "deadbeef");
            }
            other => panic!("expected InvalidSha, got {other:?}"),
        }
    }

    // ---- is_valid_sha helper unit tests (F27 strong assertions) ----

    #[test]
    fn is_valid_sha_accepts_40_and_64_lowercase_hex() {
        assert!(is_valid_sha(&"a".repeat(40)));
        assert!(is_valid_sha(&"0".repeat(40)));
        assert!(is_valid_sha(&"f".repeat(40)));
        assert!(is_valid_sha(&"0123456789abcdef".repeat(4)));
        assert!(is_valid_sha(&"f".repeat(64)));
        // SHA-256 sample from NIST examples
        assert!(is_valid_sha(
            "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
        ));
    }

    #[test]
    fn is_valid_sha_rejects_invalid_lengths_and_chars() {
        assert!(!is_valid_sha(""));
        assert!(!is_valid_sha("a"));
        assert!(!is_valid_sha(&"a".repeat(39)));
        assert!(!is_valid_sha(&"a".repeat(41)));
        assert!(!is_valid_sha(&"a".repeat(63)));
        assert!(!is_valid_sha(&"a".repeat(65)));
        assert!(!is_valid_sha(&"A".repeat(40)), "uppercase rejected");
        assert!(!is_valid_sha(&"g".repeat(40)), "non-hex rejected");
        assert!(!is_valid_sha("0x1234"), "0x prefix rejected");
    }
}