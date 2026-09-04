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
    #[error("intent invariant violated (reserved for future variants)")]
    Reserved,
}

/// Classify a `RunArgs` snapshot into one [`RunIntent`] variant.
///
/// This is a thin wrapper that extracts the three boolean flags and
/// delegates to [`classify_run_intent_flags`]. It exists so callers
/// that own `RunArgs` by value can still classify intent without
/// reconstructing the struct.
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
    if let Some(name) = worktree_name {
        if !name.is_empty() {
            return Some(name.to_string());
        }
    }
    if let Some(plan) = plan {
        if let Some(stem) = plan.file_stem().and_then(|s| s.to_str()) {
            let stem = stem.trim();
            if !stem.is_empty() && !stem.eq_ignore_ascii_case("prompt") {
                return Some(stem.to_string());
            }
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
    /// that contains `loop.lock`, `current-loop-id`, etc.).
    pub worktree_path: PathBuf,
    /// Held lock on `<worktree_path>/.ralph/loop.lock`.
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
    let name = exact_worktree_name_from(worktree_name, plan).ok_or(GateError::NoExactWorktreeName)?;
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
        AssessmentVerdict::AlreadyCompleted { last_terminal_reason } => {
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
        AssessmentRefusal::MissingScratchpad => ".ralph/agent/scratchpad.md is missing"
            .to_string(),
        AssessmentRefusal::HistoryIoError(msg) => format!("history I/O error: {msg}"),
        AssessmentRefusal::OutboxIoError(msg) => format!("outbox I/O error: {msg}"),
        AssessmentRefusal::LoopLockedByOther { holder_pid } => format!(
            ".ralph/loop.lock indicates another live loop (pid {holder_pid}); \
             the lock assessment is independent of the gate's own lock because \
             the prior holder crashed before releasing the flock"
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
            .chain(plan.map(|p| {
                Box::leak(format!("--plan={p}").into_boxed_str()) as &str
            }))
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
        assert_eq!(
            classify_run_intent(&args).unwrap(),
            RunIntent::ReuseFresh
        );
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
        assert_eq!(
            exact_worktree_name(&args),
            Some("wt-explicit".to_string())
        );
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
}
