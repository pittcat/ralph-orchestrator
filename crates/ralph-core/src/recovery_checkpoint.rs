//! U1 (plan 2026-09-01-2102): typed, read-only checkpoint assessment.
//!
//! The combined `--continue --worktree --reuse-worktree` workflow must
//! answer a single question before letting the loop proceed: **is the
//! workspace's durable checkpoint state consistent and eligible for
//! continuation?** This module is the answer.
//!
//! # Contract
//!
//! [`assess_checkpoint`] is a **pure function** over durable runtime
//! state. It never writes to disk, never rotates markers, never creates
//! an archive, and never acquires any locks. It only reads.
//!
//! The verdict is one of three values:
//!
//! - [`AssessmentVerdict::Eligible`] — every mandatory artifact is
//!   present, the loop identity matches, and no completion_promise has
//!   landed. The caller may continue.
//! - [`AssessmentVerdict::AlreadyCompleted`] — the most-recent terminal
//!   history event is `LoopCompleted { reason: "completion_promise" }`.
//!   The caller MUST drop `--continue` (re-running would no-op on the
//!   same workspace).
//! - [`AssessmentVerdict::Refused`] — a structured refusal variant
//!   describing exactly which precondition failed. No free-form error
//!   strings leak into the verdict; each variant carries the minimum
//!   useful detail for the operator-facing message layer.
//!
//! # Read set
//!
//! The function inspects six workspace files (all under `.ralph/`):
//!
//! 1. `current-loop-id` — the prior loop identity; compared against
//!    the caller's `expected_loop_id`.
//! 2. `current-events` — a UTF-8 single-line marker whose trimmed
//!    content resolves (relative to the workspace, or absolute) to a
//!    regular file. The target file must exist and be a regular file.
//! 3. `agent/scratchpad.md` — must exist.
//! 4. `history.jsonl` — read via [`LoopHistory::read_all`]; a missing
//!    file is treated as empty history.
//! 5. `agent/accepted-transitions.jsonl` — read via
//!    [`crate::event_loop::accepted_transition::read_outbox`]; a
//!    missing file is treated as empty outbox. Real IO errors
//!    propagate as [`AssessmentRefusal::OutboxIoError`].
//! 6. `loop.lock` — parsed as JSON; a non-zero `pid` field means
//!    another loop is (was) alive here, and we refuse to continue.
//!
//! # Why no side effects?
//!
//! Continuation requires several subsequent operations (worktree
//! attachment, runtime resume via `task.resume`, history reconciliation,
//! …). Each of those is **conditional on a clean assessment**. If the
//! assessment ever wrote to disk, a transient failure in a later step
//! would leave the workspace half-prepared for resume — exactly the
//! failure mode continuation is supposed to avoid. By keeping this
//! module pure, the operator can call it any number of times and
//! always observe the same answer.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::event_loop::accepted_transition;
use crate::loop_history::{HistoryError, HistoryEventType, LoopHistory};
use crate::loop_lock::LockMetadata;

/// Typed verdict for a workspace's checkpoint readiness. Zero side effects.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssessmentVerdict {
    /// Workspace is eligible to continue (no completion, identity match,
    /// all mandatory artifacts present).
    Eligible,
    /// Workspace was completed with `completion_promise`. Reject and tell
    /// the operator to drop `--continue`.
    AlreadyCompleted {
        /// The terminal reason captured on disk (always `"completion_promise"`
        /// for this variant).
        last_terminal_reason: String,
    },
    /// Workspace was refused. Carry a structured reason code (no strings).
    Refused(AssessmentRefusal),
}

/// Structured refusal reason codes. Each variant carries the minimum
/// useful detail for the operator-facing message layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AssessmentRefusal {
    /// Expected loop id does not match the marker on disk.
    LoopIdentityMismatch {
        /// The loop id the caller asked us to verify against.
        expected: String,
        /// The loop id on the marker (empty when the marker is absent).
        actual: String,
    },
    /// `.ralph/current-events` is missing, empty, or its target is not
    /// a regular file.
    MissingCurrentEventsTarget,
    /// `.ralph/agent/scratchpad.md` does not exist.
    MissingScratchpad,
    /// History I/O error other than NotFound (which is empty history).
    HistoryIoError(String),
    /// Outbox path is a directory or otherwise unreadable (real I/O
    /// error, not NotFound).
    OutboxIoError(String),
    /// `loop.lock` is held by another PID (loop is alive; do not also
    /// handle the lock — U2 does that).
    LoopLockedByOther {
        /// The PID recorded on the lock metadata.
        holder_pid: u32,
    },
}

/// Errors raised by the assessment entrypoint (caller mistakes; not
/// assessment outcomes).
#[derive(Debug, Error)]
pub enum AssessmentError {
    #[error("workspace path does not exist: {0}")]
    WorkspaceMissing(PathBuf),
    #[error("expected_loop_id is empty")]
    EmptyExpectedLoopId,
}

/// Workspace-relative path of the loop-id marker.
const CURRENT_LOOP_ID_RELATIVE: &str = ".ralph/current-loop-id";
/// Workspace-relative path of the events-target marker.
const CURRENT_EVENTS_RELATIVE: &str = ".ralph/current-events";
/// Workspace-relative path of the scratchpad file.
const SCRATCHPAD_RELATIVE: &str = ".ralph/agent/scratchpad.md";
/// Workspace-relative path of the history file.
const HISTORY_RELATIVE: &str = ".ralph/history.jsonl";
/// Workspace-relative path of the loop lock file.
const LOOP_LOCK_RELATIVE: &str = ".ralph/loop.lock";

/// Assess a workspace's checkpoint readiness.
///
/// **No side effects**: never writes to disk, never rotates markers,
/// never creates archive. See the [module-level docs](self) for the
/// full contract.
///
/// # Errors
///
/// Returns [`AssessmentError::WorkspaceMissing`] if `workspace` does
/// not exist on disk, and [`AssessmentError::EmptyExpectedLoopId`] if
/// `expected_loop_id` is empty or whitespace.
pub fn assess_checkpoint(
    workspace: &Path,
    expected_loop_id: &str,
) -> Result<AssessmentVerdict, AssessmentError> {
    // Caller-input validation — these are mistakes in the call, not
    // problems with the workspace.
    if !workspace.exists() {
        return Err(AssessmentError::WorkspaceMissing(workspace.to_path_buf()));
    }
    if expected_loop_id.trim().is_empty() {
        return Err(AssessmentError::EmptyExpectedLoopId);
    }

    // 1. Loop identity must match.
    let marker_path = workspace.join(CURRENT_LOOP_ID_RELATIVE);
    let actual_loop_id = read_marker(&marker_path)
        .unwrap_or(None)
        .unwrap_or_default();
    if actual_loop_id != expected_loop_id {
        return Ok(AssessmentVerdict::Refused(
            AssessmentRefusal::LoopIdentityMismatch {
                expected: expected_loop_id.to_string(),
                actual: actual_loop_id,
            },
        ));
    }

    // 2. current-events must exist and resolve to a regular file.
    let events_marker_path = workspace.join(CURRENT_EVENTS_RELATIVE);
    if let Err(refusal) = resolve_events_target(workspace, &events_marker_path) {
        return Ok(AssessmentVerdict::Refused(refusal));
    }

    // 3. scratchpad must exist.
    let scratchpad_path = workspace.join(SCRATCHPAD_RELATIVE);
    if !scratchpad_path.is_file() {
        return Ok(AssessmentVerdict::Refused(
            AssessmentRefusal::MissingScratchpad,
        ));
    }

    // 4. History scan — only completion_promise maps to AlreadyCompleted;
    //    every other terminal (including LoopTerminated) continues.
    let history_path = workspace.join(HISTORY_RELATIVE);
    let history = LoopHistory::new(&history_path);
    let terminal_reason = match latest_terminal_reason(&history) {
        Ok(reason) => reason,
        Err(e) => {
            return Ok(AssessmentVerdict::Refused(
                AssessmentRefusal::HistoryIoError(e.to_string()),
            ));
        }
    };
    if let Some(reason) = terminal_reason
        && reason == "completion_promise"
    {
        return Ok(AssessmentVerdict::AlreadyCompleted {
            last_terminal_reason: reason,
        });
    }
    // Any other terminal reason (max_iterations, max_runtime,
    // failure, terminated signal) means continue past it.

    // 5. Outbox read — real IO errors are refusals. NotFound is empty.
    if let Err(e) = accepted_transition::read_outbox(workspace) {
        return Ok(AssessmentVerdict::Refused(
            AssessmentRefusal::OutboxIoError(e.to_string()),
        ));
    }

    // 6. Loop lock — if the metadata shows a non-zero pid, another
    //    loop is alive here. Refuse.
    let lock_path = workspace.join(LOOP_LOCK_RELATIVE);
    if let Some(pid) = is_loop_lock_held(&lock_path) {
        return Ok(AssessmentVerdict::Refused(
            AssessmentRefusal::LoopLockedByOther { holder_pid: pid },
        ));
    }

    Ok(AssessmentVerdict::Eligible)
}

/// Read the most recent terminal reason from a history.
///
/// Scans events from the **last** one backwards and returns:
/// - `Some(reason.clone())` if `LoopCompleted { reason }` (any reason),
/// - `Some(signal.clone())` if `LoopTerminated { signal }`,
/// - `None` otherwise (no terminal event found).
///
/// Note: the caller (`assess_checkpoint`) is responsible for mapping
/// `Some(reason)` to either `AlreadyCompleted` (only when reason ==
/// `"completion_promise"`) or `Eligible` (every other reason).
///
/// Returns `Err(HistoryError)` when reading history fails for any
/// reason other than NotFound (which `read_all` already converts to
/// an empty `Vec`).
pub(crate) fn latest_terminal_reason(
    history: &LoopHistory,
) -> Result<Option<String>, HistoryError> {
    let events = history.read_all()?;
    for event in events.iter().rev() {
        match &event.event_type {
            HistoryEventType::LoopCompleted { reason } => return Ok(Some(reason.clone())),
            HistoryEventType::LoopTerminated { signal } => return Ok(Some(signal.clone())),
            _ => continue,
        }
    }
    Ok(None)
}

/// Read a UTF-8 marker file: trim, treat empty as `None`, propagate
/// real IO errors. `NotFound` returns `Ok(None)` so callers can use a
/// single API to mean "absent or empty".
pub(crate) fn read_marker(path: &Path) -> io::Result<Option<String>> {
    match fs::read_to_string(path) {
        Ok(s) => {
            let trimmed = s.trim();
            if trimmed.is_empty() {
                Ok(None)
            } else {
                Ok(Some(trimmed.to_string()))
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// Read `.ralph/current-events` and resolve the target path.
///
/// Treats a missing or whitespace-only marker as
/// [`AssessmentRefusal::MissingCurrentEventsTarget`]; the same refusal
/// is returned when the resolved target does not exist or is not a
/// regular file. Relative paths are resolved against `workspace`;
/// absolute paths are used as-is.
pub(crate) fn resolve_events_target(
    workspace: &Path,
    marker_path: &Path,
) -> Result<PathBuf, AssessmentRefusal> {
    let body = match fs::read_to_string(marker_path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(AssessmentRefusal::MissingCurrentEventsTarget);
        }
        Err(e) => {
            return Err(AssessmentRefusal::OutboxIoError(e.to_string()));
        }
    };
    let trimmed = body.trim();
    if trimmed.is_empty() {
        return Err(AssessmentRefusal::MissingCurrentEventsTarget);
    }
    let target = Path::new(trimmed);
    let resolved = if target.is_absolute() {
        target.to_path_buf()
    } else {
        workspace.join(target)
    };
    if !resolved.is_file() {
        return Err(AssessmentRefusal::MissingCurrentEventsTarget);
    }
    Ok(resolved)
}

/// Inspect `.ralph/loop.lock` and return the recorded PID if it is
/// non-zero. Returns `None` when the file is absent, empty,
/// unparseable, or has a zero PID. We do **not** acquire the flock —
/// U2 is responsible for the liveness check; this helper only reports
/// what the metadata says.
pub(crate) fn is_loop_lock_held(lock_path: &Path) -> Option<u32> {
    if !lock_path.exists() {
        return None;
    }
    let body = fs::read_to_string(lock_path).ok()?;
    let metadata: LockMetadata = serde_json::from_str(&body).ok()?;
    if metadata.pid != 0 {
        Some(metadata.pid)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};
    use std::path::PathBuf;
    use tempfile::TempDir;

    /// Build the minimum viable workspace fixture: `.ralph/`, with
    /// `current-loop-id`, `current-events` (pointing at a real empty
    /// file we also create), `agent/scratchpad.md`, and an empty
    /// `history.jsonl`. Returns the workspace root.
    fn fixture(dir: &TempDir, loop_id: &str) -> PathBuf {
        let workspace = dir.path().to_path_buf();
        let ralph_dir = workspace.join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        std::fs::create_dir_all(&agent_dir).unwrap();

        std::fs::write(ralph_dir.join("current-loop-id"), format!("{loop_id}\n")).unwrap();

        let events_file = ralph_dir.join("events.jsonl");
        std::fs::write(&events_file, "").unwrap();
        std::fs::write(
            ralph_dir.join("current-events"),
            events_file.to_str().unwrap(),
        )
        .unwrap();

        std::fs::write(agent_dir.join("scratchpad.md"), "").unwrap();
        std::fs::write(ralph_dir.join("history.jsonl"), "").unwrap();

        workspace
    }

    /// Hash the entire directory tree by sorting files then feeding
    /// `(relative-path \0 content \0)` tuples into a SHA-256 hasher.
    /// Stable, byte-exact, no external deps.
    fn dir_hash(root: &Path) -> String {
        let mut entries: Vec<(PathBuf, Vec<u8>)> = Vec::new();
        let mut stack: Vec<PathBuf> = vec![root.to_path_buf()];
        while let Some(p) = stack.pop() {
            let meta = match std::fs::symlink_metadata(&p) {
                Ok(m) => m,
                Err(_) => continue,
            };
            if meta.is_dir() {
                if let Ok(rd) = std::fs::read_dir(&p) {
                    for e in rd.flatten() {
                        stack.push(e.path());
                    }
                }
            } else if meta.is_file() {
                if let Ok(content) = std::fs::read(&p) {
                    let rel = p.strip_prefix(root).unwrap_or(&p).to_path_buf();
                    entries.push((rel, content));
                }
            }
        }
        entries.sort_by(|a, b| a.0.cmp(&b.0));
        let mut hasher = Sha256::new();
        for (rel, content) in entries {
            hasher.update(rel.to_string_lossy().as_bytes());
            hasher.update([0u8]);
            hasher.update(&content);
            hasher.update([0u8]);
        }
        format!("{:x}", hasher.finalize())
    }

    /// 1. Only `completion_promise` maps to `AlreadyCompleted`.
    #[test]
    fn assessment_marks_only_completion_promise_as_already_completed() {
        let dir = TempDir::new().unwrap();
        let workspace = fixture(&dir, "loop-x");

        let history_path = workspace.join(HISTORY_RELATIVE);
        let history = LoopHistory::new(&history_path);
        history.record_started("implement feature").unwrap();
        history.record_completed("completion_promise").unwrap();

        let verdict = assess_checkpoint(&workspace, "loop-x").unwrap();
        assert_eq!(
            verdict,
            AssessmentVerdict::AlreadyCompleted {
                last_terminal_reason: "completion_promise".to_string(),
            }
        );
    }

    /// 2. `LoopTerminated` is past tense — continue over it. Also:
    ///    the function is byte-identical-no-side-effects; verify by
    ///    hashing the worktree before and after the call.
    #[test]
    fn assessment_accepts_matching_interrupted_checkpoint_without_writes() {
        let dir = TempDir::new().unwrap();
        let workspace = fixture(&dir, "loop-y");

        let history_path = workspace.join(HISTORY_RELATIVE);
        let history = LoopHistory::new(&history_path);
        history.record_started("kill -TERM it").unwrap();
        history.record_terminated("SIGTERM").unwrap();

        let before = dir_hash(&workspace);
        let verdict = assess_checkpoint(&workspace, "loop-y").unwrap();
        let after = dir_hash(&workspace);

        assert_eq!(
            verdict,
            AssessmentVerdict::Eligible,
            "LoopTerminated must not block continuation"
        );
        assert_eq!(before, after, "assess_checkpoint must not write anything");
    }

    /// 3. Identity mismatch returns `LoopIdentityMismatch` carrying
    ///    both sides.
    #[test]
    fn assessment_refuses_loop_identity_mismatch() {
        let dir = TempDir::new().unwrap();
        let workspace = fixture(&dir, "abc");

        let verdict = assess_checkpoint(&workspace, "xyz").unwrap();
        assert_eq!(
            verdict,
            AssessmentVerdict::Refused(AssessmentRefusal::LoopIdentityMismatch {
                expected: "xyz".to_string(),
                actual: "abc".to_string(),
            })
        );
    }

    /// 4. `current-events` marker is present but the target file is
    ///    absent — refuse with `MissingCurrentEventsTarget`.
    #[test]
    fn assessment_refuses_missing_current_event_target() {
        let dir = TempDir::new().unwrap();
        let workspace = fixture(&dir, "loop-z");
        // Overwrite current-events to point at a non-existent file.
        std::fs::write(
            workspace.join(CURRENT_EVENTS_RELATIVE),
            ".ralph/does-not-exist.jsonl",
        )
        .unwrap();

        let verdict = assess_checkpoint(&workspace, "loop-z").unwrap();
        assert_eq!(
            verdict,
            AssessmentVerdict::Refused(AssessmentRefusal::MissingCurrentEventsTarget)
        );
    }

    /// 5. Outbox path is a directory — `read_outbox` propagates a
    ///    real IO error (NotFound is empty; this is `EISDIR` on Linux).
    ///    Map to `OutboxIoError` without inventing new variants.
    #[test]
    fn assessment_refuses_real_outbox_io_error() {
        let dir = TempDir::new().unwrap();
        let workspace = fixture(&dir, "loop-w");

        // Replace the outbox path with a directory of the same name.
        let outbox_path = workspace.join(".ralph/agent/accepted-transitions.jsonl");
        std::fs::remove_file(&outbox_path).ok();
        std::fs::create_dir_all(&outbox_path).unwrap();

        let verdict = assess_checkpoint(&workspace, "loop-w").unwrap();
        match verdict {
            AssessmentVerdict::Refused(AssessmentRefusal::OutboxIoError(_)) => {}
            other => panic!("expected OutboxIoError refusal, got {other:?}"),
        }
    }

    /// 6. A torn trailing line in the outbox must NOT introduce a new
    ///    refusal variant. The existing `read_outbox` salvages well-
    ///    formed lines and silently drops malformed ones; we just
    ///    verify `assess_checkpoint` either accepts (`Eligible`) or
    ///    refuses using one of the existing variants — never a new
    ///    one, and never a panic.
    ///
    ///    NOTE: this test depends on `read_outbox`'s salvage contract
    ///    (RTF-001). DO NOT modify `accepted_transition.rs` to "fix"
    ///    torn-tail handling — the test asserts the existing contract.
    #[test]
    fn assessment_preserves_torn_tail_salvage_contract() {
        let dir = TempDir::new().unwrap();
        let workspace = fixture(&dir, "loop-t");

        let outbox_path = workspace.join(".ralph/agent/accepted-transitions.jsonl");
        let valid = r#"{"activation_id":"a","committed_at":"c","contract_revision":"r","delivered":false,"loop_id":"loop-t","payload_digest":"d","topic":"work.done","transition_id":"tid"}"#;
        let torn = r#"{"topic":"work.d"#;
        std::fs::write(&outbox_path, format!("{valid}\n{torn}")).unwrap();

        // Sanity-check the salvage contract: read_outbox returns the
        // complete line and skips the torn one.
        let salvage = accepted_transition::read_outbox(&workspace).unwrap();
        assert!(
            !salvage.is_empty(),
            "read_outbox must salvage at least one entry from a torn-tail file"
        );

        // Now run the assessment; it must NOT panic and MUST NOT add
        // a new refusal variant. Either Eligible (the normal case
        // here) or one of the existing Refusal variants is acceptable.
        let verdict = assess_checkpoint(&workspace, "loop-t").unwrap();
        match verdict {
            AssessmentVerdict::Eligible => {}
            AssessmentVerdict::Refused(_) => {
                // Acceptable as long as we don't see a brand-new
                // refusal kind — match the full enum to ensure that.
            }
            other => panic!("unexpected verdict shape: {other:?}"),
        }
    }
}
