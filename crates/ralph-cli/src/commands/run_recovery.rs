//! U2 (plan 2026-09-01-2102): `RunIntent` classifier + fail-closed
//! combined-intent gate for the `--continue --worktree --reuse-worktree`
//! workflow.
//!
//! # Why this module exists
//!
//! Three flags together (`--continue`, `--worktree`, `--reuse-worktree`)
//! encode the **trusted worktree continuation** path: re-bind an
//! already-completed worktree to a previous loop's checkpoint and run
//! `task.resume` against it instead of bootstrapping a fresh loop. The
//! old code path treated `--reuse-worktree` and `--continue` as
//! independent flags, which produced two failure modes:
//!
//! 1. **Hidden cleanup**. `clean_worktree_runtime_artifacts` archived
//!    `.ralph/` content under `.ralph/reuse-history/<timestamp>/` even
//!    when the operator's intent was to *continue* the prior loop — the
//!    very events the resume contract depends on were wiped before
//!    `task.resume` could read them.
//! 2. **No exclusive lock**. The worktree branch has always run without
//!    `.ralph/loop.lock`, on the assumption that worktrees are fully
//!    isolated. That is true for fresh worktrees, but continuation is
//!    *exactly* the case where a stale lock from the prior run (or a
//!    parallel `--continue` from another terminal) would silently allow
//!    two loops to write to the same checkpoint.
//!
//! This module makes both failure modes impossible. The intent is
//! decided *once*, in [`classify_run_intent`], before any disk side
//! effect. The combined path then takes the worktree's `.ralph/loop.lock`
//! via [`LoopLock::try_acquire`] and asks
//! [`recovery_checkpoint::assess_checkpoint`] whether the durable
//! checkpoint state is eligible for continuation. Either step fails
//! closed: the lock is released, no archive is written, and the loop is
//! never started.
//!
//! # Contract
//!
//! - [`classify_run_intent`] is a **pure function**. It only reads the
//!   three boolean flag fields and the optional plan / worktree-name on
//!   `RunArgs`; it never touches disk, never resolves paths, never
//!   consults the environment.
//! - [`acquire_and_assess`] is the *only* function in this module that
//!   may fail the run. Its two-step ordering — `try_acquire` first, then
//!   `assess_checkpoint` — is load-bearing: holding the lock for the
//!   duration of the read-only assessment prevents a parallel
//!   `--continue --reuse-worktree` from sneaking through between the
//!   verdict and the resume.
//! - All refusal reasons are typed ([`GateError`]). Operator-facing
//!   messages are produced by the caller; this module does not embed
//!   user-visible strings beyond the structured variants.

use std::path::{Path, PathBuf};

// `clap::Parser` is only needed for `RunArgs::try_parse_from` inside the
// `#[cfg(test)]` module below; the bin target never parses argv here.
#[cfg(test)]
use clap::Parser;

use ralph_core::loop_lock::{LockError, LockGuard, LoopLock};
use ralph_core::recovery_checkpoint::{
    AssessmentError, AssessmentRefusal, AssessmentVerdict, assess_checkpoint,
};

use crate::commands::run::RunArgs;

// ---------------------------------------------------------------------------
// Intent classifier
// ---------------------------------------------------------------------------

/// Typed expression of what the operator asked for, in priority order.
///
/// The four variants are mutually exclusive; see [`classify_run_intent`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunIntent {
    /// No flags set: standard primary-loop bootstrap.
    Fresh,
    /// `--continue` without `--worktree`: continue an existing primary
    /// loop in place.
    ContinuePrimary,
    /// `--worktree --reuse-worktree` without `--continue`: archive prior
    /// runtime artifacts (if any), validate the resume manifest, and run
    /// a fresh loop inside the worktree.
    ReuseFresh,
    /// `--continue --worktree --reuse-worktree`: the trusted
    /// continuation path. Skip the archive step entirely, hold the
    /// worktree's loop lock, and gate on
    /// [`recovery_checkpoint::assess_checkpoint`].
    ContinueReusedWorktree,
}

/// Caller-side errors that prevent even classifying the intent.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum IntentError {
    /// `--continue` was set but the caller did not supply a reusable
    /// worktree pair (`--worktree` + `--reuse-worktree`). The
    /// continuation path without worktree isolation is the primary
    /// loop and is reported as [`RunIntent::ContinuePrimary`], not an
    /// error; this variant only fires when the args are self-contradictory
    /// in a future extension. Currently a placeholder.
    #[allow(dead_code)]
    #[error("intent invariant violated (reserved for future variants)")]
    Reserved,
}

/// Classify a `RunArgs` snapshot into one [`RunIntent`] variant.
///
/// This is a thin wrapper that extracts the three boolean flags and
/// delegates to [`classify_run_intent_flags`]. It exists so callers
/// that own `RunArgs` by value can still classify intent without
/// reconstructing the struct.
#[allow(dead_code)]
pub fn classify_run_intent(args: &RunArgs) -> Result<RunIntent, IntentError> {
    classify_run_intent_flags(args.continue_mode, args.worktree, args.reuse_worktree)
}

/// Classify intent from the three boolean flags alone.
///
/// Priority order (top wins; later branches only run when earlier ones
/// do not match):
///
/// 1. `continue_mode && worktree && reuse_worktree` →
///    [`RunIntent::ContinueReusedWorktree`].
/// 2. `continue_mode && !worktree` → [`RunIntent::ContinuePrimary`].
///    `--reuse-worktree` without `--worktree` is rejected by clap, so it
///    cannot appear here.
/// 3. `worktree && reuse_worktree && !continue_mode` →
///    [`RunIntent::ReuseFresh`].
/// 4. Otherwise → [`RunIntent::Fresh`].
pub fn classify_run_intent_flags(
    continue_mode: bool,
    worktree: bool,
    reuse_worktree: bool,
) -> Result<RunIntent, IntentError> {
    if continue_mode && worktree && reuse_worktree {
        Ok(RunIntent::ContinueReusedWorktree)
    } else if continue_mode && !worktree {
        Ok(RunIntent::ContinuePrimary)
    } else if worktree && reuse_worktree && !continue_mode {
        Ok(RunIntent::ReuseFresh)
    } else {
        Ok(RunIntent::Fresh)
    }
}

// ---------------------------------------------------------------------------
// Worktree name resolution
// ---------------------------------------------------------------------------

/// Return the **exact** worktree name for the `--reuse-worktree` path,
/// or `None` if neither `--worktree-name` nor `--plan` is supplied.
///
/// This mirrors [`super::run::resolve_exact_worktree_name`] for the
/// `plan_file = None` (callers without a plan path) case. It exists
/// here so [`acquire_and_assess`] can refuse the combined path with a
/// structured error when the operator did not pin a worktree name — the
/// gate must know which worktree it is locking.
#[allow(dead_code)]
pub fn exact_worktree_name(args: &RunArgs) -> Option<String> {
    exact_worktree_name_from(args.worktree_name.as_deref(), args.plan.as_deref())
}

/// Same as [`exact_worktree_name`] but takes the two raw fields. This
/// avoids forcing callers that have already partially destructured
/// `RunArgs` to take a `&RunArgs` borrow (which the borrow checker
/// rejects after any non-`Copy` partial move).
pub fn exact_worktree_name_from(
    worktree_name: Option<&str>,
    plan: Option<&Path>,
) -> Option<String> {
    if let Some(name) = worktree_name
        && !name.is_empty()
    {
        return Some(name.to_string());
    }
    if let Some(plan) = plan
        && let Some(stem) = plan.file_stem().and_then(|s| s.to_str())
    {
        let stem = stem.trim();
        if !stem.is_empty() && !stem.eq_ignore_ascii_case("prompt") {
            return Some(stem.to_string());
        }
    }
    None
}

// ---------------------------------------------------------------------------
// Combined-intent gate
// ---------------------------------------------------------------------------

/// The state captured after a successful [`acquire_and_assess`].
///
/// The lock guard is held for the lifetime of the `ContinueContext`,
/// which means the worktree's `.ralph/loop.lock` is released only when
/// the run command returns / drops the context. That is intentional —
/// `task.resume` then runs under the same lock the assessment took.
#[derive(Debug)]
pub struct ContinueContext {
    /// The exact worktree name (also the loop id for the prior run).
    pub loop_id: String,
    /// Absolute path to the worktree's `.ralph/` root (the directory
    /// that contains `loop.lock`, `current-loop-id`, etc.). Stored so
    /// the caller can introspect the bound worktree; not read by the
    /// production run path, which only uses `loop_id`.
    #[allow(dead_code)]
    pub worktree_path: PathBuf,
    /// Held lock on `<worktree_path>/.ralph/loop.lock`. Released only
    /// when this guard drops (i.e. when `ContinueContext` goes out of
    /// scope at run exit); intentionally never read.
    #[allow(dead_code)]
    pub lock_guard: LockGuard,
}

/// Refusal reasons from [`acquire_and_assess`].
///
/// Every variant carries the structured detail needed to build an
/// operator-facing message without re-parsing free-form strings.
#[derive(Debug, thiserror::Error)]
pub enum GateError {
    /// `--reuse-worktree` was set but neither `--worktree-name` nor
    /// `--plan` resolved to a usable worktree name.
    #[error("--reuse-worktree requires `--plan <plan.md>` or `--worktree-name <name>`")]
    NoExactWorktreeName,

    /// The worktree directory does not exist on disk. Continuing a
    /// missing worktree is impossible — there is no `.ralph/`.
    #[error("worktree '{name}' does not exist at {path}")]
    WorktreeMissing {
        /// The exact worktree name the gate was given.
        name: String,
        /// The path the gate expected to find it at.
        path: PathBuf,
    },

    /// Another live loop is holding the worktree's `.ralph/loop.lock`.
    /// The struct's caller cannot proceed until that loop exits.
    #[error(
        "worktree '{name}' is locked by another live loop (pid {pid}); \
         refusing to attach. Stop the other loop or wait for it to exit."
    )]
    WorktreeLive {
        /// The exact worktree name.
        name: String,
        /// PID recorded in the lock metadata.
        pid: u32,
    },

    /// `LoopLock::try_acquire` returned an unexpected error (IO, parse,
    /// unsupported platform). The lock is *not* held.
    #[error("failed to acquire worktree lock at {path}: {source}")]
    LockBusy {
        /// Path to `.ralph/loop.lock` inside the worktree.
        path: PathBuf,
        /// Underlying `LoopLock` error.
        #[source]
        source: LockError,
    },

    /// `recovery_checkpoint::assess_checkpoint` produced a structured
    /// refusal. The detail is intentionally a single string so the
    /// caller can `format!("{detail}")` into a banner without juggling
    /// the typed variants.
    #[error("checkpoint refused continuation: {detail}")]
    Checkpoint {
        /// Human-readable summary of the structured refusal.
        detail: String,
    },
}

/// Step 1 of the combined-intent gate: take the worktree's lock.
/// Step 2: ask `assess_checkpoint` whether the durable state is eligible.
///
/// On success, the returned [`ContinueContext`] holds an exclusive lock
/// on the worktree's `.ralph/loop.lock` for as long as the caller keeps
/// it. On any failure, the lock is released before the function returns
/// (via `Drop` on a local guard) and no archive is written.
///
/// The `worktree_name` / `plan` references are the same fields the
/// `RunArgs` struct already exposes; the signature takes them
/// individually (rather than `&RunArgs`) so callers that have already
/// partially destructured `RunArgs` can still pass the live borrows.
///
/// `workspace_root` is the parent repository root (used to compute the
/// `<root>/.worktrees/<name>` location). `prompt_summary` is the same
/// string passed to the rest of the run command and is written into the
/// lock metadata so other loops can see why this loop is running.
pub fn acquire_and_assess(
    worktree_name: Option<&str>,
    plan: Option<&Path>,
    workspace_root: &Path,
    prompt_summary: &str,
) -> Result<ContinueContext, GateError> {
    // Step 0: pin the worktree name. Without it we cannot locate the
    // target worktree and a default would silently attach to the wrong
    // one.
    let name =
        exact_worktree_name_from(worktree_name, plan).ok_or(GateError::NoExactWorktreeName)?;
    let worktree_path = workspace_root.join(".worktrees").join(&name);

    // Step 0a: confirm the directory exists. The worktree must be on
    // disk before we can either lock or assess it.
    if !worktree_path.is_dir() {
        return Err(GateError::WorktreeMissing {
            name,
            path: worktree_path,
        });
    }

    // Step 1: take the worktree's exclusive loop lock.
    //
    // `LoopLock::try_acquire` writes the lock metadata (PID + started
    // + prompt) on success and returns AlreadyLocked on contention.
    // The returned guard's Drop truncates the file before releasing
    // the flock, which is why we bind it to `lock_guard` rather than
    // dropping it implicitly inside a match arm.
    let lock_guard = match LoopLock::try_acquire(&worktree_path, prompt_summary) {
        Ok(guard) => guard,
        Err(LockError::AlreadyLocked(metadata)) => {
            return Err(GateError::WorktreeLive {
                name,
                pid: metadata.pid,
            });
        }
        Err(source) => {
            return Err(GateError::LockBusy {
                path: worktree_path.join(LoopLock::LOCK_FILE),
                source,
            });
        }
    };

    // Step 2: ask the read-only checkpoint whether the worktree's
    // durable state is eligible for continuation. The assessment is
    // pure — it never writes — but holding the lock across it removes
    // the race where two `--continue` calls see Eligible at the same
    // instant and both proceed.
    //
    // If the assessment fails for any reason we must drop the guard
    // BEFORE returning so the operator can retry without manually
    // clearing the lock file.
    let verdict = match assess_checkpoint(&worktree_path, &name) {
        Ok(v) => v,
        Err(AssessmentError::WorkspaceMissing(path)) => {
            drop(lock_guard);
            return Err(GateError::WorktreeMissing { name, path });
        }
        Err(AssessmentError::EmptyExpectedLoopId) => {
            // Defensive: exact_worktree_name() already filters empty
            // names. Surface this as a Checkpoint refusal rather than
            // a panic — the lock is dropped automatically.
            drop(lock_guard);
            return Err(GateError::Checkpoint {
                detail: "expected loop id is empty".to_string(),
            });
        }
    };

    let eligible = match verdict {
        AssessmentVerdict::Eligible => true,
        AssessmentVerdict::AlreadyCompleted {
            last_terminal_reason,
        } => {
            drop(lock_guard);
            return Err(GateError::Checkpoint {
                detail: format!(
                    "worktree was already completed (terminal reason: {last_terminal_reason}); \
                     drop --continue and re-run without it, or use --remove-worktree-and-continue"
                ),
            });
        }
        AssessmentVerdict::Refused(refusal) => {
            let detail = render_refusal(&refusal);
            drop(lock_guard);
            return Err(GateError::Checkpoint { detail });
        }
    };

    // Sanity: `eligible == true` here, but the boolean form keeps the
    // compiler from collapsing the branches above.
    debug_assert!(eligible, "verdict should have been Eligible here");

    Ok(ContinueContext {
        loop_id: name,
        worktree_path,
        lock_guard,
    })
}

/// Render an [`AssessmentRefusal`] into a single-line operator message.
///
/// Kept in this module (not the `recovery_checkpoint` core) because the
/// exact wording is a CLI-layer concern; the core stays string-free.
///
/// `pub(crate)` so [`crate::commands::run`] can reuse the same wording
/// when it surfaces the gate verdict as an anyhow error in the U1
/// parent-cleared gate path (the in-line check there uses the same
/// single-line message as the typed verdict here, for consistency).
pub(crate) fn render_refusal(refusal: &AssessmentRefusal) -> String {
    match refusal {
        AssessmentRefusal::LoopIdentityMismatch { expected, actual } => format!(
            "loop identity mismatch: expected '{expected}', found '{actual}' in \
             .ralph/current-loop-id"
        ),
        AssessmentRefusal::MissingCurrentEventsTarget => ".ralph/current-events marker is missing \
             or its target is not a regular file"
            .to_string(),
        AssessmentRefusal::MissingScratchpad => ".ralph/agent/scratchpad.md is missing".to_string(),
        AssessmentRefusal::HistoryIoError(msg) => format!("history I/O error: {msg}"),
        AssessmentRefusal::OutboxIoError(msg) => format!("outbox I/O error: {msg}"),
        AssessmentRefusal::LoopLockedByOther { holder_pid } => format!(
            ".ralph/loop.lock indicates another live loop (pid {holder_pid}); \
             the lock assessment is independent of the gate's own lock because \
             the prior holder crashed before releasing the flock. \
             Delete the worktree's .ralph/loop.lock and retry if that pid is dead."
        ),
        AssessmentRefusal::GateNotClearedByParent { worktree } => format!(
            "parent-cleared gate at {worktree_display} is missing/stale/tampered; \
             combined --continue cannot proceed without a fresh parent signature",
            worktree_display = worktree.display()
        ),
        AssessmentRefusal::EventsTargetOutsideWorkspace {
            resolved,
            expected_prefix,
        } => format!(
            ".ralph/current-events resolves to {resolved_display} which is \
             outside the workspace .ralph/ prefix ({prefix_display}); refusing \
             to continue with a foreign events file",
            resolved_display = resolved.display(),
            prefix_display = expected_prefix.display()
        ),
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a `RunArgs` with every flag set to its zero value except
    /// the ones we explicitly pass. clap's `Parser` derive does not
    /// provide `Default`, so we synthesise the minimum fields needed
    /// for the classifier.
    fn mk_args(
        continue_mode: bool,
        worktree: bool,
        reuse_worktree: bool,
        worktree_name: Option<&str>,
        plan: Option<&str>,
    ) -> RunArgs {
        let argv: Vec<&str> = std::iter::empty()
            .chain(std::iter::once("ralph"))
            .chain(continue_mode.then_some("--continue"))
            .chain(worktree.then_some("--worktree"))
            .chain(reuse_worktree.then_some("--reuse-worktree"))
            .chain(worktree_name.map(|n| {
                // Workaround: --worktree-name requires --worktree at
                // the clap level. When worktree is false we still
                // want to test the plan-only path; clap lets us pass
                // the flag without --worktree at parse time as long
                // as long as we use try_parse_from with permissive
                // flags. Since we control the argv, we just always
                // pass --worktree in the worktree_name case to keep
                // the parser happy.
                Box::leak(format!("--worktree-name={n}").into_boxed_str()) as &str
            }))
            .chain(plan.map(|p| Box::leak(format!("--plan={p}").into_boxed_str()) as &str))
            .collect();
        RunArgs::try_parse_from(argv).expect("argv should parse")
    }

    #[test]
    fn classify_fresh_when_no_flags() {
        let args = mk_args(false, false, false, None, None);
        assert_eq!(classify_run_intent(&args).unwrap(), RunIntent::Fresh);
    }

    #[test]
    fn classify_continue_primary_when_continue_without_worktree() {
        let args = mk_args(true, false, false, None, None);
        assert_eq!(
            classify_run_intent(&args).unwrap(),
            RunIntent::ContinuePrimary
        );
    }

    #[test]
    fn classify_reuse_fresh_when_worktree_reuse_without_continue() {
        let args = mk_args(false, true, true, Some("wt-x"), None);
        assert_eq!(classify_run_intent(&args).unwrap(), RunIntent::ReuseFresh);
    }

    #[test]
    fn classify_combined_when_all_three_flags_set() {
        let args = mk_args(true, true, true, Some("wt-x"), None);
        assert_eq!(
            classify_run_intent(&args).unwrap(),
            RunIntent::ContinueReusedWorktree
        );
    }

    #[test]
    fn combined_path_wins_over_continue_primary_when_worktree_pair_present() {
        // The triple-true branch must beat the continue_primary
        // branch. Both `continue_mode` and `worktree+reuse_worktree`
        // are true; the combined variant is the only correct answer.
        let args = mk_args(true, true, true, Some("wt-y"), None);
        assert_eq!(
            classify_run_intent(&args).unwrap(),
            RunIntent::ContinueReusedWorktree
        );
    }

    #[test]
    fn exact_worktree_name_from_explicit_flag() {
        let args = mk_args(true, true, true, Some("wt-explicit"), None);
        assert_eq!(exact_worktree_name(&args), Some("wt-explicit".to_string()));
    }

    #[test]
    fn exact_worktree_name_from_plan_stem() {
        let args = mk_args(true, true, true, None, Some("docs/plans/2026-09-01-foo.md"));
        assert_eq!(
            exact_worktree_name(&args),
            Some("2026-09-01-foo".to_string())
        );
    }

    #[test]
    fn exact_worktree_name_none_when_neither_supplied() {
        let args = mk_args(true, true, true, None, None);
        assert_eq!(exact_worktree_name(&args), None);
    }

    #[test]
    fn exact_worktree_name_rejects_prompt_stem() {
        // A `--plan PROMPT.md` is intentionally not used as a
        // worktree name (that would collide with the default
        // prompt-file contract).
        let args = mk_args(true, true, true, None, Some("PROMPT.md"));
        assert_eq!(exact_worktree_name(&args), None);
    }

    #[test]
    fn acquire_and_assess_rejects_missing_worktree() {
        // Build a temp dir as workspace root; never create .worktrees/x.
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = acquire_and_assess(Some("missing-wt"), None, tmp.path(), "prompt").unwrap_err();
        match err {
            GateError::WorktreeMissing { name, .. } => {
                assert_eq!(name, "missing-wt");
            }
            other => panic!("expected WorktreeMissing, got {other:?}"),
        }
    }

    #[test]
    fn acquire_and_assess_rejects_when_no_exact_name() {
        // Combined path requires an exact worktree name; without
        // --plan or --worktree-name, classify still picks
        // ContinueReusedWorktree (the flags ARE set), but the gate
        // refuses on NoExactWorktreeName.
        let tmp = tempfile::tempdir().expect("tempdir");
        let err = acquire_and_assess(None, None, tmp.path(), "prompt").unwrap_err();
        assert!(
            matches!(err, GateError::NoExactWorktreeName),
            "expected NoExactWorktreeName, got {err:?}"
        );
    }

    #[test]
    fn classify_run_intent_flags_matches_full_classifier() {
        // The two classifier entrypoints must agree across the full
        // truth table. We always pass --worktree when worktree is
        // true (clap requires --worktree-name to imply --worktree),
        // and only add --worktree-name when --worktree is set so the
        // parser accepts argv.
        for &(c, w, r) in &[
            (false, false, false),
            (true, false, false),
            (false, true, false),
            (true, true, false),
            (false, true, true),
            (true, true, true),
        ] {
            let from_full = RunArgs::try_parse_from({
                let mut argv = vec!["ralph"];
                if c {
                    argv.push("--continue");
                }
                if w {
                    argv.push("--worktree");
                }
                if r {
                    argv.push("--reuse-worktree");
                }
                // Only add --worktree-name when --worktree is set;
                // otherwise clap refuses the parse (it treats
                // --worktree-name as requiring --worktree).
                if w {
                    argv.push("--worktree-name=wt");
                }
                argv
            })
            .expect("argv parses");
            let a = classify_run_intent(&from_full).unwrap();
            let b = classify_run_intent_flags(c, w, r).unwrap();
            assert_eq!(a, b, "mismatch for c={c} w={w} r={r}");
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // TG-S09 (PMI-008①, 2026-09-05): LoopLockedByOther 渲染缺恢复指引
    //
    // PMI-008 invariant: 崩溃恢复的每条 typed 拒绝都必须给出操作员可
    // 执行的下一步（WorktreeLive 说 wait/stop、AlreadyCompleted 说 drop
    // --continue）。`render_refusal` 的 LoopLockedByOther 分支
    // (run_recovery.rs:408-412) 只解释「prior holder crashed before
    // releasing the flock」而不给任何动作——与 primary 路径
    // (run.rs:1741-1748: LockStatus::Stale → remove_file + 重取) 的
    // stale 自动清理不对称。本测试把缺口钉成机器可查: 修复(消息补
    // 「删除 .ralph/loop.lock 后重试」指引或对死 PID 自动降级)落地后
    // 断言翻转。
    //
    // 附注(行为级实测, 2026-09-05 沙箱): PMI-008① trigger 描述的
    // 「combined path 撞死 PID 残锁 → LoopLockedByOther」不可达——
    // acquire_and_assess 的 Step1 try_acquire 先把死 PID metadata 覆写
    // 为自身 pid,Step6 的 is_loop_lock_held 过滤自身 → 永远 Eligible。
    // LoopLockedByOther 只在直接调 assess_checkpoint 的路径(或极窄的
    // 并发窗口)可达;缺口因此是「渲染质量」而非「每次崩溃都撞」。
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn tg_s09_loop_locked_by_other_render_gives_no_recovery_action() {
        let refusal = AssessmentRefusal::LoopLockedByOther { holder_pid: 999999 };
        let msg = render_refusal(&refusal);

        // The message must still explain the situation (diagnostic half).
        assert!(
            msg.contains("999999"),
            "TG-S09: the rendered refusal must name the holder PID: {msg}"
        );

        // PMI-008① pin — the message must give the operator an executable
        // next step (imperative-verb keywords: delete/retry/wait/stop/clean).
        let actionable = ["remove", "delete", "retry", "wait", "stop", "clean"]
            .iter()
            .any(|kw| msg.to_lowercase().contains(kw));
        assert!(
            actionable,
            "TG-S09 (PMI-008①): LoopLockedByOther render gives a dead-PID \
             explanation but NO executable recovery action. The primary \
             path (run.rs Stale branch) auto-cleans the stale lock and \
             retries; the combined --continue path only explains. Rendered: \
             {msg:?}. Fix directions: (a) extend the message with the \
             recovery action (delete the worktree's .ralph/loop.lock and \
             re-run), or (b) auto-degrade dead-PID metadata in \
             is_loop_lock_held/assess_checkpoint (flock probe) so the \
             refusal never fires for crashed holders."
        );
    }

    /// TG-S09 对照半边: GateError 的其它 typed 拒绝都带动作指引
    /// (WorktreeLive → "Stop the other loop or wait"),证明「每个
    /// 拒绝都给下一步」是本模块既有语义,LoopLockedByOther 的渲染
    /// 是缺口而非新约定。
    #[test]
    fn tg_s09_worktree_live_render_does_give_recovery_action() {
        let err = GateError::WorktreeLive {
            name: "wt-x".to_string(),
            pid: 4242,
        };
        let msg = err.to_string();
        assert!(
            msg.to_lowercase().contains("wait"),
            "TG-S09 control: WorktreeLive render must keep its actionable \
             guidance (stop/wait): {msg}"
        );
    }

    /// TG-S09 直接调 assess_checkpoint (绕过 acquire_and_assess 的
    /// try_acquire 覆写)证明: 带死 PID metadata 的 loop.lock 在纯
    /// assessment 视角下确实产出 LoopLockedByOther 拒绝——这正是
    /// render_refusal 缺指引的场景本体 (is_loop_lock_held 只读
    /// metadata, pid != 0 且 != current → Some(pid))。
    #[test]
    fn tg_s09_assess_checkpoint_refuses_dead_pid_lock_metadata() {
        use ralph_core::recovery_checkpoint::assess_checkpoint;

        let tmp = tempfile::tempdir().expect("tempdir");
        let workspace = tmp.path();
        let ralph_dir = workspace.join(".ralph");
        let agent_dir = ralph_dir.join("agent");
        std::fs::create_dir_all(&agent_dir).unwrap();
        let events_file = ralph_dir.join("events.jsonl");
        std::fs::write(&events_file, "").unwrap();
        std::fs::write(
            ralph_dir.join("current-events"),
            events_file.to_str().unwrap(),
        )
        .unwrap();
        std::fs::write(ralph_dir.join("current-loop-id"), "tg-s09\n").unwrap();
        std::fs::write(agent_dir.join("scratchpad.md"), "# s\n").unwrap();
        std::fs::write(ralph_dir.join("history.jsonl"), "").unwrap();
        // Dead-PID crash residue: flock NOT held (file written directly,
        // no flock), metadata JSON intact.
        std::fs::write(
            ralph_dir.join("loop.lock"),
            r#"{"pid": 999999, "started": "2026-09-05T10:00:00Z", "prompt": "crashed"}"#,
        )
        .unwrap();

        let verdict = assess_checkpoint(workspace, "tg-s09").expect("assessment IO");
        match verdict {
            ralph_core::recovery_checkpoint::AssessmentVerdict::Refused(
                AssessmentRefusal::LoopLockedByOther { holder_pid },
            ) => {
                assert_eq!(holder_pid, 999999);
            }
            other => panic!(
                "TG-S09: expected Refused(LoopLockedByOther) for dead-PID lock \
                 metadata, got {other:?}"
            ),
        }
    }
}
