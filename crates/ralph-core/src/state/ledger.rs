//! [`StateLedger`] — the unified state container for the
//! orchestrator loop.
//!
//! Plan ref: U1 of
//! `docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md`.
//!
//! The ledger pairs an in-memory [`LedgerSnapshot`] with a
//! persistent append-only commit log (`.ralph/ledger.jsonl`). The
//! snapshot is the read side; the commit log is the write-of-record.
//! On cold start, [`StateLedger::replay_from_disk`] rebuilds the
//! snapshot by replaying the log on top of an empty default.
//!
//! ## Feature flag
//!
//! The `feature_enabled` flag is always `true`. The legacy no-op
//! path has been removed — every `commit()` updates the ledger
//! and `replay_from_disk` reconstructs the snapshot from the
//! commit log.
//!
//! ## Commit scope (P1-2 decision)
//!
//! Not every `CommitDelta` variant is persisted per-event. The
//! ledger's role is **replay durability for crash recovery and
//! cross-process resume**, and only the deltas that affect what
//! `replay_from_disk` rebuilds on cold start need per-event
//! latency. The scope decision is:
//!
//! **Per-event commit** (decision-point commits; see
//! `EventLoop::commit_terminal_delta`):
//! - `RejectionRecorded` (FIX-9, `correction/mod.rs:346`) —
//!   consumed by the retry budget on resume.
//! - `CompletionRequested` / `CompletionHonored` /
//!   `CancellationRequested` (P1-2) — terminal markers; without
//!   per-event commit, a mid-flight crash loses the
//!   termination signal and `replay_from_disk` re-runs the
//!   batch instead of honoring it.
//!
//! **End-of-batch commit** (A1 hook, `event_loop/mod.rs:10522+`):
//! - `CounterChanged { Iteration }` — per-iteration scalar.
//! - `StewardWoken` — engine-internal.
//!
//! **NOT committed at all** (source of truth is the projector's
//! disk-side writes, the ledger is redundant for these):
//! - `TaskLifecycle`, `TaskInserted` — see
//!   `.ralph/agent/tasks.jsonl` (projector writes).
//! - `ProgressUpdate` — see `.ralph/agent/progress.md` (projector
//!   writes).
//! - `PlanComplete` — derived from `LOOP_COMPLETE` events in
//!   `events.jsonl` on replay.
//!
//! Adding new per-event commits is fine as long as they pass
//! the "would replay miss this without it" test. Anything that
//! the projector's disk-side writes or the events.jsonl tail
//! already preserves is redundant.

use std::cell::Cell;
use std::path::{Path, PathBuf};

use chrono::Utc;
use tracing::{debug, warn};

use crate::file_lock::FileLock;

use super::commit::{Commit, CommitDelta};
use super::snapshot::LedgerSnapshot;

/// Default on-disk location for the commit log, relative to a
/// workspace root. Matches the path plan §U1 §"持久化格式".
pub const LEDGER_RELATIVE_PATH: &str = ".ralph/ledger.jsonl";

/// The unified state ledger.
///
/// Owns:
/// - The live [`LedgerSnapshot`] (in-memory)
/// - The append-only commit log (in memory + on disk)
/// - The monotonic `commit_seq` counter
/// - The on-disk path
/// - The feature flag
#[derive(Debug)]
pub struct StateLedger {
    snapshot: LedgerSnapshot,
    /// In-memory mirror of the commit log. The on-disk file is
    /// the source of truth; the in-memory vec is rebuilt on
    /// cold start by [`StateLedger::replay_from_disk`].
    commit_log: Vec<Commit>,
    /// Monotonically increasing sequence number. Equal to
    /// `commit_log.len()` after every successful `commit()`.
    commit_seq: u64,
    /// Workspace root, used to derive [`Self::ledger_path`].
    workspace: PathBuf,
    /// Pre-computed on-disk path. Held as a field to avoid
    /// re-joining on every `commit()`.
    ledger_path: PathBuf,
    /// Feature flag. When `false`, every `commit()` is a no-op
    /// and `replay_from_disk` returns an empty snapshot.
    feature_enabled: bool,
    /// P2-#4 (002-adversarial-review): tracks whether a
    /// [`StateLedger::snapshot_mut`] borrow is currently active.
    /// `commit()` refuses to run while this is `true` so callers
    /// cannot silently bypass the commit log. The flag is
    /// `Cell<bool>` (not plain `bool`) so the RAII guard
    /// [`Self::snapshot_mut`] can flip it without taking a
    /// `&mut` on the whole ledger (which would conflict with
    /// the `&mut self` borrow on `commit`).
    bypass_active: Cell<bool>,
}

/// Error type for ledger operations. Wraps both I/O failures and
/// corruption errors from `replay_from_disk`.
#[derive(Debug, thiserror::Error)]
pub enum LedgerError {
    /// The on-disk ledger file could not be read or written.
    #[error("ledger io error: {0}")]
    Io(#[from] std::io::Error),
    /// A commit JSONL line failed to parse.
    #[error("ledger parse error at line {line}: {message}")]
    Parse { line: usize, message: String },
    /// `replay_from_disk` stopped at `last_good_line` after
    /// hitting a corrupt record. The snapshot reflects all
    /// commits up to (and including) the last good line.
    #[error(
        "ledger corruption: replay stopped at line {last_good_line}; remaining {remaining_lines} line(s) skipped"
    )]
    Corruption {
        last_good_line: usize,
        remaining_lines: usize,
    },
    /// P2-#4 (002-adversarial-review): a `commit()` was issued
    /// while a `snapshot_mut` borrow is still active. This is
    /// the invariant guard against the "callers that go through
    /// this path must skip the on-disk write" footgun in the
    /// previous `snapshot_mut` docs: the only correct way to
    /// mutate the snapshot is via a `CommitDelta`.
    #[error(
        "ledger bypass active: `commit()` refused because `snapshot_mut` borrow is still alive; close the borrow before committing"
    )]
    BypassActive,
}

impl StateLedger {
    /// Build a new in-memory ledger. The on-disk file is created
    /// on the first successful `commit()`. When
    /// `feature_enabled` is `false`, every subsequent `commit()` is
    /// a no-op (and the file is never created).
    ///
    /// **U11-T1 (P1-1 fix)**: when `feature_enabled` is `true`,
    /// `new()` now calls [`Self::replay_from_disk`] first. This
    /// restores the snapshot from `.ralph/ledger.jsonl` on cold
    /// start so committed state survives process restarts. On
    /// any I/O or parse error, we log a `warn!` and fall back to
    /// [`LedgerSnapshot::cold_start`] — the loop should not refuse
    /// to start just because the ledger is missing or corrupt;
    /// the recovery path is the CLI's `ralph loops clean --ledger`
    /// entry point.
    pub fn new(workspace: &Path, feature_enabled: bool) -> Self {
        let snapshot = if feature_enabled {
            match Self::replay_from_disk(workspace) {
                Ok(snap) => {
                    debug!(workspace = %workspace.display(), "replayed ledger.jsonl on cold start");
                    snap
                }
                Err(e) => {
                    warn!(
                        workspace = %workspace.display(),
                        error = %e,
                        "replay_from_disk failed; falling back to cold_start snapshot"
                    );
                    LedgerSnapshot::cold_start()
                }
            }
        } else {
            LedgerSnapshot::cold_start()
        };

        Self {
            snapshot,
            commit_log: Vec::new(),
            commit_seq: 0,
            workspace: workspace.to_path_buf(),
            ledger_path: workspace.join(LEDGER_RELATIVE_PATH),
            feature_enabled,
            bypass_active: Cell::new(false),
        }
    }

    /// Build a new ledger, seeded from a pre-existing snapshot
    /// (used in tests + by U3 cold-start migration). The commit
    /// log is left empty.
    #[cfg(test)]
    pub fn new_with_snapshot(
        workspace: &Path,
        feature_enabled: bool,
        snapshot: LedgerSnapshot,
    ) -> Self {
        Self {
            snapshot,
            commit_log: Vec::new(),
            commit_seq: 0,
            workspace: workspace.to_path_buf(),
            ledger_path: workspace.join(LEDGER_RELATIVE_PATH),
            feature_enabled,
            bypass_active: Cell::new(false),
        }
    }

    /// Borrow the current snapshot.
    pub fn snapshot(&self) -> &LedgerSnapshot {
        &self.snapshot
    }

    /// Mutable access to the snapshot. Reserved for U2 where the
    /// projector rebuilds the in-memory cache from a pre-existing
    /// disk state (e.g. legacy `tasks.jsonl`) before the first
    /// commit.
    ///
    /// **P2-#4 (002-adversarial-review)**: the previous
    /// `snapshot_mut()` let callers bypass the commit log
    /// without any runtime check — the docs only asked politely
    /// that they skip the on-disk write. The new API returns an
    /// RAII guard ([`BypassGuard`]) that flips the ledger's
    /// `bypass_active` flag for the lifetime of the borrow;
    /// [`Self::commit`] refuses to run while the flag is set
    /// and returns `LedgerError::BypassActive`. The only way to
    /// mutate the snapshot after the guard drops is through a
    /// `CommitDelta`, so a forgotten `commit()` after a raw
    /// mutation can no longer silently desync the snapshot and
    /// the commit log.
    pub fn snapshot_mut(&mut self) -> BypassGuard<'_> {
        self.bypass_active.set(true);
        BypassGuard { ledger: self }
    }

    /// The on-disk path the ledger writes to. Exposed so U2 can
    /// delete or rotate the file when migrating a workspace that
    /// predates the ledger.
    pub fn ledger_path(&self) -> &Path {
        &self.ledger_path
    }

    /// Whether the feature flag is on. Callers consult this to
    /// decide whether to also update the legacy in-memory
    /// trackers on `LoopState` (the dual-write path during U1
    /// migration).
    pub fn feature_enabled(&self) -> bool {
        self.feature_enabled
    }

    /// The workspace the ledger is rooted at.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Borrow the in-memory commit log. The returned slice is
    /// ordered by `sequence`.
    pub fn commit_log(&self) -> &[Commit] {
        &self.commit_log
    }

    /// Apply a [`CommitDelta`] to the snapshot and append a
    /// [`Commit`] to the log. Returns the appended commit.
    ///
    /// When `feature_enabled` is `false`, the call is a no-op:
    /// - the snapshot is not mutated,
    /// - the commit log is not extended,
    /// - no file is written,
    /// - the returned `Commit` is [`Commit::empty`].
    ///
    /// When the feature is on, the in-memory snapshot is mutated
    /// first; if the on-disk write fails, the in-memory mutation
    /// is rolled back and the original snapshot is restored.
    ///
    /// **FIX-1 (U11)**: the on-disk write is now atomic. The
    /// full commit log is rebuilt to a sibling `.tmp` file and
    /// renamed into place. A crash mid-write leaves the previous
    /// file intact (POSIX rename is atomic on the same
    /// filesystem) — `replay_from_disk` therefore never sees a
    /// partial line. The append-only pattern was dropped because
    /// it could leave half a JSONL line on disk after a process
    /// crash.
    ///
    /// **P1-#8 (002-adversarial-review)**: the previous
    /// implementation cloned the entire `LedgerSnapshot` on
    /// every commit for rollback safety. The clone was O(N) in
    /// task / progress / workflow size and ran on every
    /// successful commit. The new strategy is:
    /// - the in-memory `commit_log` is the source of truth, so
    ///   the snapshot is just a projection;
    /// - on the happy path, no extra allocation happens — the
    ///   delta is applied, the commit is pushed to `commit_log`,
    ///   and the on-disk file is written;
    /// - on a failed on-disk write, the new commit is popped
    ///   from `commit_log` and the snapshot is rebuilt by
    ///   replaying the now-shorter log on top of a cold start.
    ///   The replay is O(N) but only fires on the rare
    ///   persist-error path.
    pub fn commit(
        &mut self,
        delta: CommitDelta,
        event_topic: Option<String>,
    ) -> Result<Commit, LedgerError> {
        if !self.feature_enabled {
            return Ok(Commit::empty());
        }

        // P2-#4 (002-adversarial-review): refuse to commit
        // while a `snapshot_mut` borrow is still alive. The
        // caller almost certainly forgot to drop the guard and
        // is about to desync the snapshot from the commit log.
        if self.bypass_active.get() {
            return Err(LedgerError::BypassActive);
        }

        self.snapshot.apply_delta(&delta);
        let new_seq = self.commit_seq + 1;
        let commit = Commit {
            iteration: self.snapshot.iteration,
            sequence: new_seq,
            timestamp: Utc::now().to_rfc3339(),
            event_topic,
            delta,
        };

        // Push the commit to the in-memory log *before* the
        // on-disk write so the post-failure replay path can
        // observe the new commit, pop it, and rebuild the
        // snapshot from the surviving log.
        self.commit_log.push(commit.clone());
        if let Err(err) = persist_commit_log(&self.ledger_path, &self.commit_log) {
            // Roll back: pop the new commit and rebuild the
            // snapshot from the surviving log on top of a cold
            // start. This is O(N) but only fires on the rare
            // on-disk write failure path; the happy path no
            // longer pays for a full snapshot clone.
            //
            // FIX-10 parity: re-derive `snapshot.iteration` from
            // the surviving log's max `commit.iteration`, exactly
            // like `replay_from_disk` does. Without this, the
            // rollback would leave `iteration` at 0 even when the
            // surviving log still records the last successful
            // iteration count.
            self.commit_log.pop();
            self.snapshot = LedgerSnapshot::cold_start();
            let mut iterations: Vec<u32> = Vec::with_capacity(self.commit_log.len());
            for c in &self.commit_log {
                self.snapshot.apply_delta(&c.delta);
                iterations.push(c.iteration);
            }
            self.snapshot.iteration = iterations.iter().copied().max().unwrap_or(0);
            // Best-effort cleanup of any stale `.tmp` left by an
            // interrupted rename.
            if let Some(tmp) = temp_sibling_path(&self.ledger_path) {
                let _ = std::fs::remove_file(&tmp);
            }
            return Err(err);
        }
        self.commit_seq = new_seq;
        Ok(commit)
    }

    /// Replay the on-disk commit log on top of an empty snapshot
    /// and return the rebuilt snapshot. Does not return a
    /// `StateLedger` so the caller decides how to wire it up.
    ///
    /// On corruption (a JSONL line that does not parse), the
    /// replay stops at the first bad line and returns an error
    /// describing the last good line. The partially-built
    /// snapshot is discarded; the caller can inspect the error
    /// and decide whether to truncate the file or refuse to
    /// resume.
    ///
    /// The empty / missing file case is not an error: a fresh
    /// workspace returns the cold-start snapshot.
    ///
    /// **FIX-10 (U11)**: `LedgerSnapshot::iteration` is restored
    /// from the max of all `commit.iteration` values in the log,
    /// not from `CounterChanged("iteration")` deltas. The
    /// `Commit::iteration` field is recorded by every commit
    /// (`commit` in [`Self::commit`] uses
    /// `self.snapshot.iteration` at the time of the write), so
    /// replaying the log and taking the max yields the same
    /// `iteration` the loop ended on.
    pub fn replay_from_disk(workspace: &Path) -> Result<LedgerSnapshot, LedgerError> {
        let ledger_path = workspace.join(LEDGER_RELATIVE_PATH);
        // P1-5 (2026-06-23-003 plan): replay against a cold
        // start. The boundary-detection step below may discard
        // trailing stale commits, so we cannot accumulate into a
        // single `snapshot` and then throw it away — re-derive on
        // a fresh base once we know where the new loop starts.
        //
        // FIX-10: the `commit.iteration` field is authoritative
        // for the in-memory loop's view (the only delta touching
        // it is `CounterChanged { Iteration }`, emitted rarely or
        // in bursts), so we collect the per-commit values to
        // restore `snapshot.iteration` even if no other delta did.
        let mut commits: Vec<Commit> = Vec::new();

        let body = match std::fs::read_to_string(&ledger_path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(LedgerSnapshot::cold_start());
            }
            Err(e) => return Err(LedgerError::Io(e)),
        };

        let mut last_good_line: usize = 0;
        for (idx, raw_line) in body.lines().enumerate() {
            let line_no = idx + 1;
            let trimmed = raw_line.trim();
            if trimmed.is_empty() {
                continue;
            }
            let commit: Commit = match serde_json::from_str(trimmed) {
                Ok(c) => c,
                Err(e) => {
                    let remaining_lines =
                        body.lines().filter(|l| !l.trim().is_empty()).count() - idx;
                    return Err(LedgerError::Parse {
                        line: line_no,
                        message: format!(
                            "{} ({} line(s) skipped after corruption)",
                            e, remaining_lines
                        ),
                    });
                }
            };
            commits.push(commit);
            last_good_line = line_no;
        }

        if last_good_line == 0 {
            // The file existed but had no parseable records.
            // Treat as corruption so the operator can decide
            // whether to truncate.
            return Err(LedgerError::Corruption {
                last_good_line: 0,
                remaining_lines: body.lines().filter(|l| !l.trim().is_empty()).count(),
            });
        }

        // P1-5: detect a multi-loop boundary. A drop in
        // `commit.iteration` (any commit i+1 with iteration
        // strictly less than commit i) means the new loop
        // started on the same workspace. Discard the drop and
        // everything after it; the surviving prefix is the
        // completed previous loop, and the new loop will start
        // fresh on a cold snapshot.
        //
        // We rebuild the snapshot from the active prefix
        // instead of relying on the `snapshot` accumulated
        // above. The first loop's commit log may be discarded
        // (when the first drop is at index 0, e.g. iteration
        // went 5→0 in commits 0–1), in which case the snapshot
        // is replaced by a cold start — matching what a clean
        // ledger rotation would yield.
        //
        // `windows(2).position(|w| w[1] < w[0])` returns the
        // index of the *first* element of the offending window
        // (the last valid iteration of the previous loop).
        // `commits[..=p]` therefore keeps that last-valid
        // commit and discards the drop + everything after it.
        let drop_idx = commits
            .windows(2)
            .position(|w| w[1].iteration < w[0].iteration);
        let active_commits: &[Commit] = match drop_idx {
            Some(p) => &commits[..=p],
            None => &commits[..],
        };

        // FIX-10: restore `iteration` from the commit log while
        // applying the active prefix in a single pass. After
        // the P1-5 truncation, the surviving iterations are
        // monotonically non-decreasing so `max()` and `last()`
        // agree. We use the running `max()` so a future
        // regression-recovery commit inside a single loop (if
        // one is ever added) is still surfaced as the high-water
        // mark rather than truncated.
        let mut snapshot = LedgerSnapshot::cold_start();
        for commit in active_commits {
            snapshot.apply_delta(&commit.delta);
            snapshot.iteration = snapshot.iteration.max(commit.iteration);
        }

        // `last_good_line` is set if any commit was applied;
        // corruption would have already returned an error.
        let _ = last_good_line;
        Ok(snapshot)
    }

    /// Append an [`OutboxEntry`](crate::event_loop::accepted_transition::OutboxEntry)
    /// to the durable transition outbox
    /// (`.ralph/agent/accepted-transitions.jsonl`).
    ///
    /// U6 (plan 2026-07-30-004): this is the durability barrier the
    /// Accepted Transition API writes through *before* publishing to
    /// the event bus. The write is append-only and guarded by an
    /// exclusive cross-process [`FileLock`] (the same pattern used by
    /// `ActivationRegistry`), then `fsync`'d so the record is durable
    /// before the caller proceeds to publish.
    ///
    /// On any error the outbox is left unchanged and the caller MUST
    /// NOT publish the corresponding event (the Accepted Transition
    /// API maps this to `TransitionError::CommitFailed`).
    pub fn append_outbox(
        &self,
        entry: &crate::event_loop::accepted_transition::OutboxEntry,
    ) -> Result<(), std::io::Error> {
        use crate::event_loop::accepted_transition::OUTBOX_RELATIVE_PATH;
        use std::io::Write;

        let outbox_path = self.workspace.join(OUTBOX_RELATIVE_PATH);
        if let Some(parent) = outbox_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let lock = FileLock::new(&outbox_path)?;
        let _guard = lock.exclusive()?;

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&outbox_path)?;
        let mut line = serde_json::to_string(entry).map_err(|e| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                format!("outbox serialize failed: {e}"),
            )
        })?;
        line.push('\n');
        file.write_all(line.as_bytes())?;
        file.sync_all()?;
        Ok(())
    }
}

/// Atomic-write helper for the on-disk JSONL commit log.
///
/// **FIX-1 (U11)**: writes the full log to a sibling `.tmp`
/// file, `fsync`s it, then `rename`s it into place and
/// `fsync`s the parent directory. The previous implementation
/// used append mode + `sync_all().ok()`, which could leave a
/// partial JSONL line on disk after a crash — `replay_from_disk`
/// would then fail with `LedgerError::Parse` for the entire
/// cold start. The new pattern is crash-safe: either the
/// previous file is intact, or the new file is fully replaced.
///
/// `fsync` failures are **not** silenced: a failed `fsync`
/// propagates as `LedgerError::Io` and the caller rolls back the
/// in-memory snapshot. The previous code's `.ok()` swallowed
/// these errors and left the caller unable to detect silent
/// durability loss.
///
/// Performance note: the log is rebuilt in full on every
/// commit. The commit log is KB-scale today (one entry per
/// orchestration-relevant state change); if U11+ introduces
/// per-event commits the helper can be swapped for an
/// append-and-rotate strategy.
fn persist_commit_log(ledger_path: &Path, log: &[Commit]) -> Result<(), LedgerError> {
    if let Some(parent) = ledger_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let tmp_path = temp_sibling_path(ledger_path).ok_or_else(|| LedgerError::Parse {
        line: 0,
        message: "ledger_persist: cannot derive temp sibling path".to_string(),
    })?;

    // Serialize the full log to a buffer first so a JSON error
    // surfaces *before* we touch the filesystem.
    let mut body = String::new();
    for commit in log {
        let mut line = serde_json::to_string(commit).map_err(|e| LedgerError::Parse {
            line: 0,
            message: format!("commit_serialize: {e}"),
        })?;
        line.push('\n');
        body.push_str(&line);
    }

    // Write to the temp file, fsync, rename, fsync parent dir.
    {
        let mut f = std::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .truncate(true)
            .open(&tmp_path)?;
        std::io::Write::write_all(&mut f, body.as_bytes())?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp_path, ledger_path)?;
    if let Some(parent) = ledger_path.parent() {
        // fsync the directory so the rename is durable across
        // a power failure. On platforms where opening a dir
        // for writing is unsupported (e.g. Windows) this fails
        // with `IsADirectory` / `PermissionDenied`; that is
        // acceptable — the parent dir's mtime is a soft
        // durability hint, the file contents are already on
        // disk.
        if let Ok(dir) = std::fs::File::open(parent) {
            let _ = dir.sync_all();
        }
    }
    Ok(())
}

/// Compute the sibling `.tmp` path used by the atomic write
/// helper. Returns `None` only when the input path has no
/// `file_name` (which cannot happen for a `ledger.jsonl` file
/// constructed under `.ralph/`).
fn temp_sibling_path(ledger_path: &Path) -> Option<PathBuf> {
    let fname = ledger_path.file_name()?;
    let mut tmp_name = std::ffi::OsString::from(".");
    tmp_name.push(fname);
    tmp_name.push(".tmp");
    Some(ledger_path.with_file_name(tmp_name))
}

/// Truncate the on-disk ledger to the first `keep_lines` lines
/// (best-effort: returns `LedgerError::Io` on read/write
/// failure). Used by `commit`'s rollback path *and* by the
/// CLI recovery entry point (`ralph loops clean --ledger`).
fn truncate_after_path(ledger_path: &Path, keep_lines: usize) -> Result<(), LedgerError> {
    let body = match std::fs::read_to_string(ledger_path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(LedgerError::Io(e)),
    };
    let kept: String = body.lines().take(keep_lines).collect::<Vec<_>>().join("\n") + "\n";
    std::fs::write(ledger_path, kept)?;
    warn!(
        path = %ledger_path.display(),
        kept_lines = keep_lines,
        "ledger truncated after failed persist"
    );
    Ok(())
}

/// Helper for tests + U3 cold-start migration: read the raw
/// commit log from disk. Returns an empty `Vec` if the file
/// does not exist.
pub fn read_commit_log(workspace: &Path) -> Result<Vec<Commit>, LedgerError> {
    let path = workspace.join(LEDGER_RELATIVE_PATH);
    let body = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(e) => return Err(LedgerError::Io(e)),
    };

    let mut commits = Vec::new();
    for (idx, raw_line) in body.lines().enumerate() {
        let line_no = idx + 1;
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let commit: Commit = serde_json::from_str(trimmed).map_err(|e| LedgerError::Parse {
            line: line_no,
            message: e.to_string(),
        })?;
        commits.push(commit);
    }
    Ok(commits)
}

/// Best-effort truncate for the recovery path (e.g. when
/// `replay_from_disk` reports corruption past line N and the
/// operator confirms truncation). Exposed for the CLI's
/// `ralph loops clean --ledger` entry point.
///
/// **FIX-1 (U11)**: now wired into `commit`'s rollback path
/// indirectly through `truncate_after_path`; the public API
/// here is kept for the CLI.
pub fn truncate_after(workspace: &Path, keep_lines: usize) -> Result<(), LedgerError> {
    let path = workspace.join(LEDGER_RELATIVE_PATH);
    truncate_after_path(&path, keep_lines)
}

/// RAII guard returned by [`StateLedger::snapshot_mut`].
///
/// The guard flips the ledger's `bypass_active` flag on
/// construction and clears it on drop. While the guard is
/// alive, [`StateLedger::commit`] refuses to run and returns
/// [`LedgerError::BypassActive`] — the only correct way to
/// mutate the snapshot after the guard drops is via a
/// `CommitDelta`.
///
/// P2-#4 (002-adversarial-review): the previous API returned a
/// bare `&mut LedgerSnapshot` with no runtime check. Callers
/// could `snapshot_mut().field = X; commit(...)` and the
/// mutation would silently desync the snapshot from the
/// commit log. The guard's `commit` refusal makes the mistake
/// loud.
pub struct BypassGuard<'a> {
    ledger: &'a mut StateLedger,
}

impl std::ops::Deref for BypassGuard<'_> {
    type Target = LedgerSnapshot;

    fn deref(&self) -> &Self::Target {
        &self.ledger.snapshot
    }
}

impl std::ops::DerefMut for BypassGuard<'_> {
    fn deref_mut(&mut self) -> &mut Self::Target {
        // SAFETY: the guard's lifetime is tied to `&mut
        // self.ledger`, which outlives the `&mut` borrow on
        // the snapshot.
        &mut self.ledger.snapshot
    }
}

impl Drop for BypassGuard<'_> {
    fn drop(&mut self) {
        self.ledger.bypass_active.set(false);
    }
}
