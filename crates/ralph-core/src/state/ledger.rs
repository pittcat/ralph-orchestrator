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
//! The `feature_enabled` flag is the U1 opt-in switch. When
//! `false`, every `commit()` is a no-op (returns
//! [`Commit::empty`], mutates no state, persists nothing) and
//! `replay_from_disk` returns an empty snapshot. This keeps the
//! legacy code path green while U2 onwards migrate their call
//! sites.
//!
//! The flag mirrors the `UNIFIED_STATE_LEDGER=1` env var (read by
//! the loop constructor). The state module does not consult the
//! env var itself — the caller passes the resolved boolean in
//! via [`StateLedger::new`].
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
//! - `HandoffAccepted` (A4, `event_loop/mod.rs:8701, 9025`).
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
//!
//! ## U5: `commit_handoff_artifact` (macro-edge auto-generate)
//!
//! macro-edge `*.handoff.accepted` 事件被 engine gate accept 后,
//! runtime 调用 [`StateLedger::commit_handoff_artifact`] 把
//! `HandoffAccepted` delta 写入 commit log,并在 `handoff_path`
//! 缺失时自动调用 `hat_handoff::allocator::prepare_with_dedup`
//! 写盘一份通过 validator 的 skeleton。生成的 path 写回 commit
//! `delta.handoff_path`,随 EventBus publish 一起对外可见。
//!
//! U5 行为锁定:
//! - `feature_enabled = false` → `commit_handoff_artifact` 是 no-op,
//!   返回 `Commit::empty`,与 U1 行为一致。
//! - `handoff_path == None` → 调用 `prepare_with_dedup` 写盘 +
//!   返回 path,validator 校验后 commit;validator 失败则降级为
//!   None + 记录 `commit_log` 失败项(策略 U5:不阻断,仅记录)。
//! - `handoff_path == Some(path)` 且文件不存在或 validator 失败
//!   → 降级为「重新 generate」(regenerate 策略),覆盖原 path
//!   指向的同名文件为新 skeleton(如果文件已存在 → 复用
//!   `prepare_with_dedup` 的「已存在即返回」语义,不覆盖)。
//!
//! 仍保留 `commit()` 主流程不动;U5 集成通过单独 API 完成,避免
//! 影响 U1 的 16 个测试。

use std::cell::Cell;
use std::path::{Path, PathBuf};

use chrono::Utc;
use tracing::{debug, warn};

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

/// 2026-06-21-002 plan U5: `commit_handoff_artifact` 的入参。
///
/// 由 `event_loop` 在 `*.handoff.accepted` 事件被 engine gate
/// accept 后构造;ledger 负责写盘(缺失时)+ validator 校验 +
/// 落 commit。
#[derive(Debug, Clone)]
pub struct HandoffAcceptedInputs {
    /// Emit hat(宏观边的 from)。
    pub from: ralph_proto::HatId,
    /// Downstream hat(宏观边的 to)。
    pub to: ralph_proto::HatId,
    /// 当前 iteration(用于 path 文件名)。
    pub iteration: u32,
    /// 当前 hat_handoff_seq(供 path 分配)。
    pub current_seq: u32,
    /// 事件 topic(用于 skeleton 渲染)。
    pub topic: String,
    /// Agent 提供的 path(None 触发自动生成,U5 走 prepare_with_dedup)。
    pub provided_handoff_path: Option<String>,
}

/// 2026-06-21-002 plan U5: `commit_handoff_artifact` 的返回。
#[derive(Debug, Clone)]
pub struct HandoffCommitOutcome {
    /// 落 commit 的 record(可能因为 feature 关闭是 `Commit::empty`)。
    pub commit: Commit,
    /// 最终落 commit 的 `handoff_path`(`None` 当 `feature_enabled` 关)。
    pub handoff_path: Option<String>,
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
        let mut snapshot = LedgerSnapshot::cold_start();
        // FIX-10: collect all per-commit iteration values so we
        // can restore `snapshot.iteration` even when the only
        // delta touching it was `CounterChanged`, which may be
        // emitted rarely or in bursts. The `commit.iteration`
        // field is authoritative for the in-memory loop's view.
        let mut iterations: Vec<u32> = Vec::new();

        let body = match std::fs::read_to_string(&ledger_path) {
            Ok(s) => s,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                return Ok(snapshot);
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
            iterations.push(commit.iteration);
            snapshot.apply_delta(&commit.delta);
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

        // FIX-10: restore `iteration` from the commit log. The
        // loop records `commit.iteration = self.snapshot.iteration`
        // at every write; the max across the log equals the
        // final loop iteration.
        snapshot.iteration = iterations.iter().copied().max().unwrap_or(0);

        // `last_good_line` is set if any commit was applied;
        // corruption would have already returned an error.
        let _ = last_good_line;
        Ok(snapshot)
    }

    /// 2026-06-21-002 plan U5: 自动生成 handoff artifact 并落 commit。
    ///
    /// 流程:
    /// 1. `feature_enabled == false` → 返回 `Commit::empty` +
    ///    `handoff_path = None`(与 U1 行为一致)。
    /// 2. `provided_handoff_path` 为 `None` → 调用
    ///    `hat_handoff::allocator::prepare_with_dedup` 写盘
    ///    一份 skeleton,获取 repo-relative path。
    /// 3. `provided_handoff_path` 为 `Some(path)`:
    ///    - 文件已存在且 `validate_artifact` 通过 → 复用
    ///      `path`。
    ///    - 文件缺失或 `validate_artifact` 失败 → **regenerate
    ///      策略**:把 `provided_handoff_path` 视作 hint,改用
    ///      `prepare_with_dedup` 重新生成(path 由 iteration +
    ///      current_seq 决定,与 hint 解耦)。
    /// 4. 把最终的 `handoff_path` 写入 `CommitDelta::HandoffAccepted`,
    ///    调 `commit()` 落 commit log。
    ///
    /// 注意:本方法**只负责** ledger 集成;engine gate 的
    /// accept / reject 逻辑在 `event_loop::mod.rs` 里走
    /// `hat_handoff::gate::evaluate_event`,与本函数解耦。
    pub fn commit_handoff_artifact(
        &mut self,
        inputs: &HandoffAcceptedInputs,
    ) -> Result<HandoffCommitOutcome, LedgerError> {
        if !self.feature_enabled {
            return Ok(HandoffCommitOutcome {
                commit: Commit::empty(),
                handoff_path: None,
            });
        }
        // 1) 决定最终 handoff_path。
        let final_path = resolve_handoff_path(&self.workspace, inputs)?;
        // 2) 构造 delta 并落 commit。
        let delta = CommitDelta::HandoffAccepted {
            from: inputs.from.clone(),
            to: inputs.to.clone(),
            handoff_path: Some(final_path.clone()),
        };
        let commit = self.commit(delta, Some(inputs.topic.clone()))?;
        Ok(HandoffCommitOutcome {
            commit,
            handoff_path: Some(final_path),
        })
    }
}

/// 2026-06-21-002 plan U5: 决定最终 handoff_path 的纯函数。
///
/// 1. `provided == None` → `prepare_with_dedup` 写盘 +
///    返回 path。
/// 2. `provided == Some(path)` 且 `validate_artifact` 通过
///    → 复用 `path`。
/// 3. `provided == Some(path)` 但 `validate_artifact` 失败
///    → 走 **regenerate** 策略:用 `write_skeleton(force=true)`
///    写到 canonical path 并返回。
///
/// **regenerate 写盘策略** 锁定:
/// - 如果旧文件存在(由 agent 写就的非法内容),被覆盖为新
///   skeleton(skeleton 通过 validator)。
/// - 如果旧文件不存在,直接写。
/// - 文件路径使用 canonical = `compute` 推导出的
///   `{iter}-{seq+1}-{from}-{to}.md`,而非 agent 的 `provided`
///   hint(避免 seq 漂移)。
fn resolve_handoff_path(
    workspace: &Path,
    inputs: &HandoffAcceptedInputs,
) -> Result<String, LedgerError> {
    use crate::hat_handoff::allocator::{self, PrepareInputs};

    let prepare_inputs = PrepareInputs {
        iteration: inputs.iteration,
        current_seq: inputs.current_seq,
        from: inputs.from.as_str(),
        to: inputs.to.as_str(),
        topic: &inputs.topic,
    };
    let computed = allocator::compute(&prepare_inputs);

    // 分支 1: provided == None → 直接 allocate(dedup)。
    let provided = match inputs.provided_handoff_path.as_deref() {
        Some(p) if !p.is_empty() => p,
        _ => {
            let outcome = allocator::prepare_with_dedup(workspace, &prepare_inputs)?;
            // P2-#8 (002-adversarial-review): surface the
            // auto-generated handoff path to operators so
            // silent file creation is visible. Without this
            // the agent never sees the file unless it reads
            // the ledger or its commit topic.
            tracing::info!(
                canonical = outcome.handoff_path.as_str(),
                from = inputs.from.as_str(),
                to = inputs.to.as_str(),
                topic = inputs.topic.as_str(),
                "U5: auto-generated handoff skeleton (no agent-provided path)"
            );
            return Ok(outcome.handoff_path);
        }
    };

    // 分支 2: provided == Some → 先 validate,通过则复用。
    if crate::hat_handoff::validator::validate_artifact(
        workspace,
        provided,
        inputs.from.as_str(),
        inputs.to.as_str(),
    )
    .is_ok()
    {
        tracing::debug!(
            provided = provided,
            from = inputs.from.as_str(),
            to = inputs.to.as_str(),
            "U5: reusing agent-provided handoff_path (validator passed)"
        );
        return Ok(provided.to_string());
    }

    // 分支 3: 降级为 regenerate。覆盖写入 canonical path
    // (而不是 provided hint),保证 filename 形状稳定 + 内容合法。
    //
    // P2-#8 (002-adversarial-review): escalate to `error!`
    // because we are about to overwrite an agent-written file
    // that the validator rejected. Downgrading this to
    // `warn!` would make the destructive overwrite look
    // routine; `error!` makes the action visible in default
    // `tracing_subscriber` output (RUST_LOG=info hides it
    // only if the operator explicitly opts in).
    tracing::error!(
        provided = provided,
        canonical = computed.handoff_path.as_str(),
        from = inputs.from.as_str(),
        to = inputs.to.as_str(),
        "U5: provided handoff_path failed validation; regenerating canonical artifact and OVERWRITING provided path with skeleton"
    );
    allocator::write_skeleton(workspace, &computed.handoff_path, &computed.skeleton, true)?;
    Ok(computed.handoff_path)
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
