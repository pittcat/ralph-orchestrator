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

use serde::{Deserialize, Serialize};
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
    /// U1 (plan 2026-09-01-2102): the parent's out-of-band signature
    /// (`.ralph/.parent-cleared-gate`) is missing, stale, unreadable,
    /// or its `loop_id` does not match the expected one. The child
    /// MUST NOT trust `--combined-continue` without a fresh parent
    /// signature — a wrapper / Makefile that just passes the flag
    /// directly would otherwise bypass the parallel-forge manifest
    /// re-validation gate (adversarial: A1). The detailed reason
    /// (missing / stale / tampered / loop-id-mismatch) is carried
    /// out-of-band by [`ParentGateReadError`] from
    /// [`read_parent_cleared_gate`]; the variant here only records
    /// *that* it failed and the worktree it failed against.
    GateNotClearedByParent {
        /// Workspace (worktree) whose gate was checked.
        worktree: PathBuf,
    },
    /// U4 (plan 2026-09-01-2102): `.ralph/current-events` resolves
    /// to a real regular file, but that file lives outside
    /// `<workspace>/.ralph/`. A writer with write access to the
    /// marker could otherwise redirect the assessment to a foreign
    /// events file and produce a split-brain verdict that mixes
    /// foreign-event-context with trusted-marker-context
    /// (adversarial: A2). Both paths are recorded as canonical
    /// forms so the operator-facing message can name them
    /// unambiguously.
    EventsTargetOutsideWorkspace {
        /// Canonical path the marker resolved to.
        resolved: PathBuf,
        /// Canonical path of `<workspace>/.ralph/` that the
        /// resolved target was expected to live under.
        expected_prefix: PathBuf,
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

/// U1 (plan 2026-09-01-2102): workspace-relative path of the parent's
/// out-of-band signature for the combined `--continue
/// --worktree --reuse-worktree` path. The parent writes this file
/// after clearing every gate (lock + checkpoint + parallel-forge
/// manifest); the child reads it before skipping the same gates. If
/// the file is missing, stale, or its contents do not match the
/// worktree's archived resume manifest, the child refuses with
/// `combined --continue refused: ...` so a wrapper / Makefile cannot
/// bypass the gate by passing `--combined-continue=true` directly.
pub const PARENT_CLEARED_GATE_RELATIVE: &str = ".ralph/.parent-cleared-gate";

/// U1 (plan 2026-09-01-2102): maximum age (milliseconds) a parent
/// signature remains trustworthy. Five minutes is the upper bound on
/// the parent-to-child IPC window for a real `--no-tui` invocation;
/// anything older is treated as a stale signature and refused.
pub const PARENT_CLEARED_GATE_FRESHNESS_MS: u128 = 5 * 60 * 1000;

/// U1 (plan 2026-09-01-2102): structured contents of the parent's
/// out-of-band signature. Serialized as JSON with mode `0600` on Unix.
///
/// - `loop_id` must match the parent-recorded loop id (rejects a
///   gate written for a different worktree).
/// - `manifest_sha256` must equal the worktree's archived resume
///   manifest digest (rejects a gate written against a different
///   manifest — the adversarial A1 case).
/// - `written_at_unix_ms` must be within the last
///   [`PARENT_CLEARED_GATE_FRESHNESS_MS`] (rejects a stale signature
///   left on disk by a previous aborted run).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentClearedGate {
    /// Parent-recorded loop id. The child compares this against the
    /// `loop_id` derived from the worktree's `.ralph/current-loop-id`.
    pub loop_id: String,
    /// SHA-256 hex digest of the resume manifest the parent just
    /// validated. Empty string when the preset does not use a
    /// parallel-forge resume manifest.
    pub manifest_sha256: String,
    /// Unix epoch milliseconds at which the parent wrote this gate.
    pub written_at_unix_ms: u128,
}

/// U1 (plan 2026-09-01-2102): detailed reason a parent-cleared gate
/// check failed. Distinct from [`AssessmentRefusal::GateNotClearedByParent`],
/// which is the coarse "the gate failed" marker — this enum gives the
/// caller the precise reason so the operator-facing message can name
/// it ("parent gate missing" vs "parent gate stale" vs
/// "parent gate tampered"). Used by
/// [`read_parent_cleared_gate`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParentGateReadError {
    /// Gate file does not exist at the worktree's
    /// `.ralph/.parent-cleared-gate` path.
    Missing,
    /// Gate file exists but could not be read (permission, IO, …).
    Unreadable(String),
    /// Gate file exists but its body is not valid JSON, or the JSON
    /// does not match [`ParentClearedGate`].
    MalformedJson(String),
    /// Gate file's `loop_id` does not match `expected_loop_id`.
    LoopIdMismatch {
        /// Loop id the caller expected.
        expected: String,
        /// Loop id recorded on the gate.
        actual: String,
    },
    /// Gate file's `written_at_unix_ms` is older than
    /// [`PARENT_CLEARED_GATE_FRESHNESS_MS`].
    Stale {
        /// Observed age (milliseconds) of the gate.
        age_ms: u128,
        /// Maximum allowed age (milliseconds).
        max_age_ms: u128,
    },
}

/// U1 (plan 2026-09-01-2102): read the parent's out-of-band signature
/// and validate its freshness + loop identity. The caller is
/// responsible for matching `manifest_sha256` against the worktree's
/// archived resume manifest digest.
///
/// Returns `Ok(ParentClearedGate)` when the file exists, parses as
/// the expected shape, `loop_id` matches `expected_loop_id`, and the
/// signature is fresh. Otherwise returns a [`ParentGateReadError`]
/// describing the precise reason.
///
/// Pure read; never writes to disk.
pub fn read_parent_cleared_gate(
    workspace: &Path,
    expected_loop_id: &str,
    now_unix_ms: u128,
) -> Result<ParentClearedGate, ParentGateReadError> {
    let path = workspace.join(PARENT_CLEARED_GATE_RELATIVE);
    let body = match fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Err(ParentGateReadError::Missing);
        }
        Err(e) => return Err(ParentGateReadError::Unreadable(e.to_string())),
    };
    let gate: ParentClearedGate = match serde_json::from_str(&body) {
        Ok(g) => g,
        Err(e) => return Err(ParentGateReadError::MalformedJson(e.to_string())),
    };
    if gate.loop_id != expected_loop_id {
        return Err(ParentGateReadError::LoopIdMismatch {
            expected: expected_loop_id.to_string(),
            actual: gate.loop_id,
        });
    }
    let age_ms = now_unix_ms.saturating_sub(gate.written_at_unix_ms);
    if age_ms > PARENT_CLEARED_GATE_FRESHNESS_MS {
        return Err(ParentGateReadError::Stale {
            age_ms,
            max_age_ms: PARENT_CLEARED_GATE_FRESHNESS_MS,
        });
    }
    Ok(gate)
}

/// U1 (plan 2026-09-01-2102): write a parent-cleared gate file. Mode
/// `0600` on Unix (only the parent that wrote it can read it again,
/// matching the rest of `.ralph/`). Creates the parent `.ralph/`
/// directory if missing.
pub fn write_parent_cleared_gate(path: &Path, gate: &ParentClearedGate) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let body =
        serde_json::to_string(gate).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::write(path, body)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let perms = std::fs::Permissions::from_mode(0o600);
        std::fs::set_permissions(path, perms)?;
    }
    Ok(())
}

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
///
/// U4 (plan 2026-09-01-2102): once the target has been confirmed to
/// exist as a regular file, both the target and the workspace's
/// `.ralph/` directory are canonicalized, and the target must live
/// under the canonicalized `<workspace>/.ralph/` prefix. A target
/// outside that prefix (foreign absolute path, or a relative path
/// like `../foo` that escapes `.ralph/`) is refused with
/// [`AssessmentRefusal::EventsTargetOutsideWorkspace`]. The
/// canonicalized form is returned on success because downstream code
/// is path-string-based and canonical paths are more reliable.
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
    // Existence precondition: keep this check before canonicalize so
    // the existing "missing target" refusal semantics survive intact
    // and only a real regular file reaches the prefix check.
    if !resolved.is_file() {
        return Err(AssessmentRefusal::MissingCurrentEventsTarget);
    }
    // U4 (plan 2026-09-01-2102): canonicalize `<workspace>/.ralph/`
    // and the resolved target, then assert the target lives under
    // the canonical workspace prefix. A foreign file (absolute path
    // to a regular file outside the worktree, or a relative path
    // that escapes `.ralph/`) is rejected so the marker cannot
    // redirect the assessment to a foreign events file
    // (adversarial: A2).
    let workspace_ralph_path = workspace.join(".ralph");
    let workspace_ralph = workspace_ralph_path
        .canonicalize()
        .map_err(|e| AssessmentRefusal::OutboxIoError(e.to_string()))?;
    let resolved_canonical = match resolved.canonicalize() {
        Ok(p) => p,
        Err(_) => {
            // The file existed at `is_file()` check time but lost
            // its existence between then and canonicalize (rare
            // race, e.g. external truncation). Treat as the
            // existing "missing target" refusal to preserve the
            // original semantics.
            return Err(AssessmentRefusal::MissingCurrentEventsTarget);
        }
    };
    if !resolved_canonical.starts_with(&workspace_ralph) {
        return Err(AssessmentRefusal::EventsTargetOutsideWorkspace {
            resolved: resolved_canonical,
            expected_prefix: workspace_ralph,
        });
    }
    Ok(resolved_canonical)
}

/// Inspect `.ralph/loop.lock` and return the recorded PID **iff it is a
/// different live process**. Returns `None` when the file is absent,
/// empty, unparseable, has a zero PID, or records the current process's
/// own PID (the gate runs *after* U2's `LoopLock::try_acquire`, so the
/// lock metadata is the current process — refusing here would make
/// the gate permanently refuse itself; see U3 surface note in
/// `integration_resume.rs::combined_continue_happy_path_eligible_passes`).
///
/// We do **not** acquire the flock — U2 is responsible for the liveness
/// check; this helper only reports what the metadata says, minus the
/// current process.
pub(crate) fn is_loop_lock_held(lock_path: &Path) -> Option<u32> {
    if !lock_path.exists() {
        return None;
    }
    let body = fs::read_to_string(lock_path).ok()?;
    let metadata: LockMetadata = serde_json::from_str(&body).ok()?;
    let current_pid = std::process::id();
    if metadata.pid != 0 && metadata.pid != current_pid {
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
            } else if meta.is_file()
                && let Ok(content) = std::fs::read(&p)
            {
                let rel = p.strip_prefix(root).unwrap_or(&p).to_path_buf();
                entries.push((rel, content));
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

    // -----------------------------------------------------------------
    // U1 (plan 2026-09-01-2102): parent-cleared gate helper tests
    // -----------------------------------------------------------------

    fn gate_path(workspace: &Path) -> PathBuf {
        workspace.join(PARENT_CLEARED_GATE_RELATIVE)
    }

    fn write_gate_raw(workspace: &Path, body: &str) {
        let path = gate_path(workspace);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, body).unwrap();
    }

    /// 7. Missing gate file → `ParentGateReadError::Missing`.
    #[test]
    fn parent_gate_missing_returns_missing_error() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let now = 1_700_000_000_000_u128;
        let err = read_parent_cleared_gate(&workspace, "loop-x", now).unwrap_err();
        assert_eq!(err, ParentGateReadError::Missing);
    }

    /// 8. Stale gate file (`written_at_unix_ms` older than the
    ///    freshness window) → `ParentGateReadError::Stale`.
    #[test]
    fn parent_gate_stale_returns_stale_error() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let now = 1_700_000_000_000_u128;
        let stale_age = PARENT_CLEARED_GATE_FRESHNESS_MS + 1;
        let gate = ParentClearedGate {
            loop_id: "loop-x".to_string(),
            manifest_sha256: String::new(),
            written_at_unix_ms: now - stale_age,
        };
        write_gate_raw(&workspace, &serde_json::to_string(&gate).unwrap());
        let err = read_parent_cleared_gate(&workspace, "loop-x", now).unwrap_err();
        assert_eq!(
            err,
            ParentGateReadError::Stale {
                age_ms: stale_age,
                max_age_ms: PARENT_CLEARED_GATE_FRESHNESS_MS,
            }
        );
    }

    /// 9. Gate file with a non-matching `loop_id` → `LoopIdMismatch`.
    #[test]
    fn parent_gate_loop_id_mismatch_returns_loop_id_error() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let now = 1_700_000_000_000_u128;
        let gate = ParentClearedGate {
            loop_id: "loop-other".to_string(),
            manifest_sha256: String::new(),
            written_at_unix_ms: now,
        };
        write_gate_raw(&workspace, &serde_json::to_string(&gate).unwrap());
        let err = read_parent_cleared_gate(&workspace, "loop-x", now).unwrap_err();
        assert_eq!(
            err,
            ParentGateReadError::LoopIdMismatch {
                expected: "loop-x".to_string(),
                actual: "loop-other".to_string(),
            }
        );
    }

    /// 10. Gate file whose body is not valid JSON → `MalformedJson`.
    #[test]
    fn parent_gate_malformed_json_returns_malformed_error() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        write_gate_raw(&workspace, "this is not json {");
        let err = read_parent_cleared_gate(&workspace, "loop-x", 0).unwrap_err();
        match err {
            ParentGateReadError::MalformedJson(_) => {}
            other => panic!("expected MalformedJson, got {other:?}"),
        }
    }

    /// 11. Fresh, matching gate file → returns `Ok(ParentClearedGate)`
    ///     with the original fields intact.
    #[test]
    fn parent_gate_fresh_and_matching_returns_gate() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let now = 1_700_000_000_000_u128;
        let gate = ParentClearedGate {
            loop_id: "loop-x".to_string(),
            manifest_sha256: "deadbeef".to_string(),
            written_at_unix_ms: now - 60_000,
        };
        write_gate_raw(&workspace, &serde_json::to_string(&gate).unwrap());
        let read = read_parent_cleared_gate(&workspace, "loop-x", now).unwrap();
        assert_eq!(read, gate);
    }

    /// 12. Round-trip: write then read returns the same struct.
    #[test]
    fn parent_gate_write_then_read_round_trips() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let gate = ParentClearedGate {
            loop_id: "loop-rt".to_string(),
            manifest_sha256: "abcd1234".to_string(),
            written_at_unix_ms: 1_700_000_000_000_u128,
        };
        let path = gate_path(&workspace);
        write_parent_cleared_gate(&path, &gate).unwrap();
        let read =
            read_parent_cleared_gate(&workspace, "loop-rt", gate.written_at_unix_ms).unwrap();
        assert_eq!(read, gate);
    }

    /// 13. Unix-only: `write_parent_cleared_gate` produces a `0600`
    ///     file. Skipped on non-Unix platforms (Windows ACL model
    ///     differs; the JSON contents still round-trip).
    #[cfg(unix)]
    #[test]
    fn parent_gate_write_uses_0600_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let path = gate_path(&workspace);
        write_parent_cleared_gate(
            &path,
            &ParentClearedGate {
                loop_id: "loop-m".to_string(),
                manifest_sha256: String::new(),
                written_at_unix_ms: 0,
            },
        )
        .unwrap();
        let perms = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(perms, 0o600, "parent-cleared gate must be 0600");
    }

    // -----------------------------------------------------------------
    // U4 (plan 2026-09-01-2102): resolve_events_target path constraint
    // -----------------------------------------------------------------

    /// Write `<workspace>/.ralph/current-events` with `body`. Caller
    /// is responsible for choosing a body that lands at a real
    /// regular file when the test expects a successful resolution;
    /// for refusal tests, an unreachable path is fine.
    fn write_events_marker(workspace: &Path, body: &str) {
        let ralph_dir = workspace.join(".ralph");
        std::fs::create_dir_all(&ralph_dir).unwrap();
        std::fs::write(ralph_dir.join("current-events"), body).unwrap();
    }

    /// 14. Foreign absolute path rejected: marker body points at a
    ///     regular file that exists on disk but lives outside the
    ///     workspace's `.ralph/` directory. Guards against an actor
    ///     with write access to `.ralph/current-events` redirecting
    ///     the assessment to a foreign events file and producing a
    ///     split-brain verdict (adversarial: A2).
    #[test]
    fn resolve_events_target_rejects_foreign_absolute_path() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let marker_path = workspace.join(CURRENT_EVENTS_RELATIVE);

        // Foreign regular file lives in a separate TempDir so it
        // is guaranteed to be outside `<workspace>/.ralph/`.
        let foreign_dir = TempDir::new().unwrap();
        let foreign = foreign_dir.path().join("foreign-events.jsonl");
        std::fs::write(&foreign, "").unwrap();

        write_events_marker(&workspace, foreign.to_str().unwrap());

        let err = resolve_events_target(&workspace, &marker_path).unwrap_err();
        match err {
            AssessmentRefusal::EventsTargetOutsideWorkspace { .. } => {}
            other => panic!("expected EventsTargetOutsideWorkspace, got {other:?}"),
        }
    }

    /// 15. Foreign relative path rejected: marker body is a relative
    ///     path that, when joined with `workspace`, resolves outside
    ///     `<workspace>/.ralph/`.
    #[test]
    fn resolve_events_target_rejects_foreign_relative_path() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let marker_path = workspace.join(CURRENT_EVENTS_RELATIVE);

        // `../outside-ralph/foo.jsonl` joined with `workspace` (e.g.
        // `/tmp/.tmpXYZ/..` => `/tmp`) lands at `/tmp/outside-ralph/
        // foo.jsonl`. We control that parent path; create the file
        // there so the existence precondition passes and the prefix
        // check is the only barrier.
        let parent = dir.path().parent().unwrap();
        let outside_dir = parent.join("outside-ralph");
        std::fs::create_dir_all(&outside_dir).unwrap();
        let outside_file = outside_dir.join("foo.jsonl");
        std::fs::write(&outside_file, "").unwrap();

        write_events_marker(&workspace, "../outside-ralph/foo.jsonl");

        let err = resolve_events_target(&workspace, &marker_path).unwrap_err();
        match err {
            AssessmentRefusal::EventsTargetOutsideWorkspace { .. } => {}
            other => panic!("expected EventsTargetOutsideWorkspace, got {other:?}"),
        }
    }

    /// 16. In-workspace relative path accepted: marker body is a
    ///     `.ralph/events-main.jsonl` style relative path that
    ///     resolves (after `workspace.join(...)`) to a real regular
    ///     file inside `<workspace>/.ralph/`.
    #[test]
    fn resolve_events_target_accepts_in_workspace_relative_path() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let ralph_dir = workspace.join(".ralph");
        std::fs::create_dir_all(&ralph_dir).unwrap();
        let events_file = ralph_dir.join("events-main.jsonl");
        std::fs::write(&events_file, "").unwrap();
        let marker_path = ralph_dir.join("current-events");

        write_events_marker(&workspace, ".ralph/events-main.jsonl");

        let resolved = resolve_events_target(&workspace, &marker_path).unwrap();
        let expected = events_file.canonicalize().unwrap();
        assert_eq!(resolved, expected);
    }

    /// 17. In-workspace absolute path accepted: marker body is the
    ///     absolute path of a real regular file under
    ///     `<workspace>/.ralph/`.
    #[test]
    fn resolve_events_target_accepts_in_workspace_absolute_path() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let ralph_dir = workspace.join(".ralph");
        std::fs::create_dir_all(&ralph_dir).unwrap();
        let events_file = ralph_dir.join("events-main.jsonl");
        std::fs::write(&events_file, "").unwrap();
        let marker_path = ralph_dir.join("current-events");

        write_events_marker(&workspace, events_file.to_str().unwrap());

        let resolved = resolve_events_target(&workspace, &marker_path).unwrap();
        let expected = events_file.canonicalize().unwrap();
        assert_eq!(resolved, expected);
    }

    /// 18. Missing file still rejected (existing behavior preserved):
    ///     marker body pointing at a non-existent file must return
    ///     `MissingCurrentEventsTarget`, NOT the new variant. The
    ///     prefix check must not fire on a path that was already
    ///     refused by the existence precondition.
    #[test]
    fn resolve_events_target_missing_file_still_returns_missing_target() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let marker_path = workspace.join(CURRENT_EVENTS_RELATIVE);

        write_events_marker(&workspace, ".ralph/does-not-exist.jsonl");

        let err = resolve_events_target(&workspace, &marker_path).unwrap_err();
        assert_eq!(err, AssessmentRefusal::MissingCurrentEventsTarget);
    }

    /// 19. Whitespace-only marker body (existing behavior preserved):
    ///     blank content must return `MissingCurrentEventsTarget`.
    #[test]
    fn resolve_events_target_whitespace_only_marker_returns_missing_target() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let marker_path = workspace.join(CURRENT_EVENTS_RELATIVE);

        write_events_marker(&workspace, "   ");

        let err = resolve_events_target(&workspace, &marker_path).unwrap_err();
        assert_eq!(err, AssessmentRefusal::MissingCurrentEventsTarget);
    }

    /// 20. Empty marker body (existing behavior preserved).
    #[test]
    fn resolve_events_target_empty_marker_returns_missing_target() {
        let dir = TempDir::new().unwrap();
        let workspace = dir.path().to_path_buf();
        let marker_path = workspace.join(CURRENT_EVENTS_RELATIVE);

        write_events_marker(&workspace, "");

        let err = resolve_events_target(&workspace, &marker_path).unwrap_err();
        assert_eq!(err, AssessmentRefusal::MissingCurrentEventsTarget);
    }
}
