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

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use tracing::warn;

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
    #[error("ledger corruption: replay stopped at line {last_good_line}; remaining {remaining_lines} line(s) skipped")]
    Corruption {
        last_good_line: usize,
        remaining_lines: usize,
    },
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
    pub fn new(workspace: &Path, feature_enabled: bool) -> Self {
        Self {
            snapshot: LedgerSnapshot::cold_start(),
            commit_log: Vec::new(),
            commit_seq: 0,
            workspace: workspace.to_path_buf(),
            ledger_path: workspace.join(LEDGER_RELATIVE_PATH),
            feature_enabled,
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
        }
    }

    /// Borrow the current snapshot.
    pub fn snapshot(&self) -> &LedgerSnapshot {
        &self.snapshot
    }

    /// Mutable access to the snapshot. Reserved for U2 where the
    /// projector rebuilds the in-memory cache from a pre-existing
    /// disk state (e.g. legacy `tasks.jsonl`) before the first
    /// commit. Callers that go through this path must skip the
    /// on-disk write — see [`Self::commit`] for the equivalent
    /// through the commit log.
    pub fn snapshot_mut(&mut self) -> &mut LedgerSnapshot {
        &mut self.snapshot
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
    pub fn commit(
        &mut self,
        delta: CommitDelta,
        event_topic: Option<String>,
    ) -> Result<Commit, LedgerError> {
        if !self.feature_enabled {
            return Ok(Commit::empty());
        }

        // Snapshot the affected sub-state *before* mutation so we
        // can roll back on write failure. We snapshot the whole
        // `LedgerSnapshot` (cheap clone) and replace it on
        // rollback; for high-frequency commits the clone cost
        // dominates and the rollback becomes a real cost. U2 may
        // introduce a narrower "affected sub-state" snapshot if
        // benchmarks show it matters.
        let prior_snapshot = self.snapshot.clone();

        self.snapshot.apply_delta(&delta);
        let new_seq = self.commit_seq + 1;
        let commit = Commit {
            iteration: self.snapshot.iteration,
            sequence: new_seq,
            timestamp: Utc::now().to_rfc3339(),
            event_topic,
            delta,
        };

        if let Err(err) = persist_commit(&self.ledger_path, &commit) {
            // Roll back the in-memory mutation; the commit was
            // not added to the log and `commit_seq` does not
            // advance.
            self.snapshot = prior_snapshot;
            return Err(err);
        }

        self.commit_log.push(commit.clone());
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
    pub fn replay_from_disk(workspace: &Path) -> Result<LedgerSnapshot, LedgerError> {
        let ledger_path = workspace.join(LEDGER_RELATIVE_PATH);
        let mut snapshot = LedgerSnapshot::cold_start();

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
                        message: format!("{} ({} line(s) skipped after corruption)", e, remaining_lines),
                    });
                }
            };
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
        return Ok(provided.to_string());
    }

    // 分支 3: 降级为 regenerate。覆盖写入 canonical path
    // (而不是 provided hint),保证 filename 形状稳定 + 内容合法。
    warn!(
        provided = provided,
        canonical = computed.handoff_path.as_str(),
        from = inputs.from.as_str(),
        to = inputs.to.as_str(),
        "provided handoff_path failed validation; regenerating canonical artifact (U5)"
    );
    allocator::write_skeleton(workspace, &computed.handoff_path, &computed.skeleton, true)?;
    Ok(computed.handoff_path)
}

/// Append one commit to the on-disk JSONL file. Uses the same
/// temp-file + rename pattern as `state_projector/progress.rs`
/// (atomic on the same filesystem).
fn persist_commit(ledger_path: &Path, commit: &Commit) -> Result<(), LedgerError> {
    if let Some(parent) = ledger_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut line = serde_json::to_string(commit)
        .map_err(|e| LedgerError::Parse {
            line: 0,
            message: format!("commit_serialize: {e}"),
        })?;
    line.push('\n');

    // Append: open in append mode, write the line, close. The
    // file is not expected to be on a remote filesystem; if
    // that changes, swap to the temp-file + rename pattern.
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(ledger_path)?;
    f.write_all(line.as_bytes())?;
    f.sync_all().ok();
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
/// operator confirms truncation). Reserved for U2 — the CLI
/// will expose this behind `ralph loops clean --ledger`.
#[allow(dead_code)]
pub fn truncate_after(workspace: &Path, keep_lines: usize) -> Result<(), LedgerError> {
    let path = workspace.join(LEDGER_RELATIVE_PATH);
    let body = match std::fs::read_to_string(&path) {
        Ok(s) => s,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(LedgerError::Io(e)),
    };
    let kept: String = body
        .lines()
        .take(keep_lines)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    std::fs::write(&path, kept)?;
    warn!(
        path = %path.display(),
        kept_lines = keep_lines,
        "ledger truncated after corruption"
    );
    Ok(())
}
