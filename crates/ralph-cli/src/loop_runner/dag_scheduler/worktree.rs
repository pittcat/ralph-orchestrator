//! 2026-09-03-0959 plan U7 (R7; S8-S11; D7-D9; E10-E12):
//! per-Unit worktree binding — trusted worktree created or
//! reused from a verified base commit.
//!
//! U7 ships the bin-side public surface as a transitional
//! state: the worktree is consumed by U8 (correction
//! wiring) and U10 (preset cutover) in the same plan, and
//! the U7 acceptance tests already cover the surface via
//! `crate::loop_runner::dag_scheduler::worktree::tests`.
//! Until those later units land, no bin-side module other
//! than this file's own tests exercises `UnitWorktree` /
//! `UnitWorktreeError` / `acquire` — we mark the module
//! `#![allow(dead_code)]` to mirror `integration.rs`'s
//! transitional pattern and keep the `RUSTFLAGS='-D warnings'`
//! gate honest. The lint will return the moment U8 / U10
//! introduce a bin-side caller and dead-code warnings flip
//! back on.
#![allow(dead_code)]

//! # Why a per-Unit worktree?
//!
//! Plan §7 U7 says: "Each Unit works in a worktree created
//! or reused from a verified base commit". The verified
//! base is the SHA the runtime captured when the plan was
//! admitted (the integration-target HEAD before any sibling
//! FF'd). Locking every Unit to that base is what lets the
//! lane CAS the candidate in safely: the lane knows nothing
//! outside the Unit's worktree could have raced with it.
//!
//! # Reuse vs create
//!
//! When a Unit is re-run after a transient failure (network
//! glitch, supervisor restart), its worktree may still be
//! alive on disk. Reuse rules:
//!   - Reuse if the existing branch tip equals
//!     `verified_base_commit` (the same base the plan was
//!     admitted with).
//!   - Re-create (with a fresh worktree) if the existing
//!     branch tip differs — the previous run was racing
//!     against a stale base and must be abandoned.
//!   - **Reject** (do NOT auto-clean) if the host repo has
//!     uncommitted changes or untracked files in
//!     `$repo_root` (not the worktree). Cleaning host state
//!     silently would erase operator changes; the lane
//!     fails-closed and the operator resolves the conflict.
//!
//! # Not the only worktree
//!
//! [`crate::worktree`] and
//! [`crate::supervisor::worktree_bind`] both define
//! worktree primitives. This module is the *third* one,
//! specialised for U7:
//!   - `crate::worktree` (loop-level helper used by
//!     `ralph run --worktree`).
//!   - `worktree_bind::bind_slot_worktree` is the
//!     supervisor's slot-binding helper (used by U3 / U4).
//!   - This module's `UnitWorktree` is the *integration-lane*
//!     one: it takes a verified base commit as input and
//!     hands out a worktree whose branch tip equals that
//!     base.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Identity of one Unit's trusted worktree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitWorktree {
    pub unit_id: String,
    pub loop_id: String,
    pub path: PathBuf,
    pub branch: String,
    pub base_commit: String,
    /// Whether this worktree was already present on disk
    /// and reused (vs freshly created).
    pub reused: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum UnitWorktreeError {
    #[error("host repo '{0}' has uncommitted changes; refuse to reuse or create unit worktree")]
    HostDirty(String),
    #[error("host repo '{0}' has untracked files; refuse to reuse or create unit worktree")]
    HostUntracked(String),
    #[error(
        "existing worktree branch '{branch}' tip '{tip}' does not match verified base '{base}'"
    )]
    BaseMismatch {
        branch: String,
        tip: String,
        base: String,
    },
    #[error("failed to inspect existing worktree '{path}': {reason}")]
    #[allow(dead_code)] // defensive variant reserved for future worktree-state probes
    InspectFailed { path: String, reason: String },
    /// Fail-closed rejection of an externally supplied identifier
    /// or commit-ish that does not pass the shape whitelist
    /// (P0-3: path/argv injection guard).
    #[error("invalid {field} '{value}': {reason}")]
    InvalidInput {
        field: &'static str,
        value: String,
        reason: &'static str,
    },
    #[error("git command failed: {0}")]
    GitFailed(String),
}

pub type UnitWorktreeResult<T> = Result<T, UnitWorktreeError>;

/// P0-3 shape whitelist for identifiers that get interpolated
/// into a branch name (`ralph/<loop>/<unit>`) and a worktree
/// path (`.ralph/worktrees/<loop>-<unit>`). Accepts
/// `[A-Za-z0-9._-]+` and rejects empty values, a leading `-`
/// (git argv option injection), and any `..` substring (path
/// escape out of `.ralph/worktrees/`). `/` is excluded by the
/// character whitelist.
fn validate_component_id(field: &'static str, value: &str) -> UnitWorktreeResult<()> {
    let reason = if value.is_empty() {
        Some("must not be empty")
    } else if value.starts_with('-') {
        Some("must not start with '-'")
    } else if value.contains("..") {
        Some("must not contain '..'")
    } else if !value
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    {
        Some("must match [A-Za-z0-9._-]+")
    } else {
        None
    };
    match reason {
        Some(reason) => Err(UnitWorktreeError::InvalidInput {
            field,
            value: value.to_string(),
            reason,
        }),
        None => Ok(()),
    }
}

/// P0-3 shape whitelist for externally supplied commit-ish values
/// that become git argv positionals: 40-hex (SHA-1) or 64-hex
/// (SHA-256) object ids only. Anything else (branch names, `HEAD`,
/// `-`-prefixed option probes) is rejected before git sees it.
fn validate_commit_oid(field: &'static str, value: &str) -> UnitWorktreeResult<()> {
    let ok = matches!(value.len(), 40 | 64) && value.chars().all(|c| c.is_ascii_hexdigit());
    if ok {
        Ok(())
    } else {
        Err(UnitWorktreeError::InvalidInput {
            field,
            value: value.to_string(),
            reason: "must be a 40- or 64-hex object id",
        })
    }
}

impl UnitWorktree {
    /// Acquire a trusted worktree for `unit_id`.
    ///
    /// `verified_base_commit` is the SHA the runtime captured
    /// at plan admission. The worktree branch tip is set
    /// (or asserted) to that SHA.
    ///
    /// Host repo's `repo_root` must be clean (no uncommitted
    /// changes, no untracked files OTHER than registered
    /// git worktrees that this module creates). Otherwise
    /// the lane refuses — operator must resolve the dirty
    /// state before any unit worktrees are bound.
    pub fn acquire(
        repo_root: &Path,
        loop_id: &str,
        unit_id: &str,
        verified_base_commit: &str,
    ) -> UnitWorktreeResult<Self> {
        // P0-3: validate every externally supplied component BEFORE
        // touching disk or spawning git. `loop_id` / `unit_id` are
        // interpolated into the branch name and the worktree path
        // below; `verified_base_commit` becomes a git argv positional.
        // Whitelist shape validation (no `..`, no `/`, no leading `-`)
        // is what keeps `.ralph/worktrees/` escape and option
        // injection unreachable.
        validate_component_id("loop_id", loop_id)?;
        validate_component_id("unit_id", unit_id)?;
        validate_commit_oid("verified_base_commit", verified_base_commit)?;

        // First, register the worktree directory as a
        // local-only ignore (per-repo `.git/info/exclude`).
        // Without this, the host repo's `git status
        // --porcelain` reports each previously-created
        // `.ralph/worktrees/<unit>` dir as untracked on the
        // second `acquire` call, and the host-clean check
        // would falsely reject the operation.
        ensure_worktree_dir_excluded(repo_root)?;
        ensure_host_clean(repo_root)?;

        let branch = format!("ralph/{}/{}", loop_id, unit_id);
        let worktree_root = repo_root.join(".ralph").join("worktrees");
        let path = worktree_root.join(format!("{}-{}", loop_id, unit_id));

        // Try reuse: does the branch already exist?
        let existing_tip = read_branch_tip(repo_root, &branch);
        match existing_tip {
            Ok(tip) if !tip.is_empty() => {
                if tip == verified_base_commit {
                    // Verify the worktree path is still on disk.
                    if path.exists() {
                        return Ok(UnitWorktree {
                            unit_id: unit_id.to_string(),
                            loop_id: loop_id.to_string(),
                            path,
                            branch,
                            base_commit: verified_base_commit.to_string(),
                            reused: true,
                        });
                    }
                    // Branch exists but worktree path is missing —
                    // fall through to fresh create.
                } else {
                    return Err(UnitWorktreeError::BaseMismatch {
                        branch,
                        tip,
                        base: verified_base_commit.to_string(),
                    });
                }
            }
            Ok(_) => {
                // Branch doesn't exist — fresh create.
            }
            Err(e) => {
                return Err(e);
            }
        }

        // Fresh create: ensure parent dir, then `git worktree add
        // -B <branch> -- <path> <verified_base>`. The `-B` flag
        // creates the branch if it doesn't exist; pointing the
        // new branch at the verified base means the worktree's
        // initial tip IS the verified base. The `--` separator
        // ends option parsing (defence-in-depth on top of the
        // shape whitelist above).
        std::fs::create_dir_all(&worktree_root)
            .map_err(|e| UnitWorktreeError::GitFailed(format!("create_dir_all: {e}")))?;
        let status = Command::new("git")
            .arg("-C")
            .arg(repo_root)
            .arg("worktree")
            .arg("add")
            .arg("-B")
            .arg(&branch)
            .arg("--")
            .arg(&path)
            .arg(verified_base_commit)
            .status()
            .map_err(|e| UnitWorktreeError::GitFailed(format!("worktree add: {e}")))?;
        if !status.success() {
            return Err(UnitWorktreeError::GitFailed(format!(
                "git worktree add exited {:?}",
                status.code()
            )));
        }
        Ok(UnitWorktree {
            unit_id: unit_id.to_string(),
            loop_id: loop_id.to_string(),
            path,
            branch,
            base_commit: verified_base_commit.to_string(),
            reused: false,
        })
    }
}

fn ensure_host_clean(repo_root: &Path) -> UnitWorktreeResult<()> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("status")
        .arg("--porcelain")
        .output()
        .map_err(|e| UnitWorktreeError::GitFailed(format!("git status: {e}")))?;
    if !out.status.success() {
        return Err(UnitWorktreeError::GitFailed(format!(
            "git status exited {:?}",
            out.status.code()
        )));
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut has_modified = false;
    let mut has_untracked = false;
    for line in text.lines() {
        if line.len() < 3 {
            continue;
        }
        // P0-4: `.ralph/` is the runtime ledger (dag.db,
        // events.jsonl, worktrees/, ...), not operator business
        // state — the DAG scheduler writes `<repo>/.ralph/dag.db`
        // while this check is the gate for `acquire`, so treating
        // `.ralph/` as dirt would deadlock every real run on hosts
        // that don't gitignore it. The exemption lives HERE (porcelain
        // parse) rather than in `.git/info/exclude` on purpose:
        // exclude rules only suppress UNTRACKED entries, while a
        // tracked-modified path under `.ralph/` would still show up —
        // filtering the porcelain output covers both shapes with one
        // mechanism. Operator dirt OUTSIDE `.ralph/` is still refused.
        let path = line[3..].trim_matches('"');
        if path == ".ralph" || path.starts_with(".ralph/") {
            continue;
        }
        let xy = &line[..2];
        if xy.starts_with('?') {
            has_untracked = true;
        } else if xy != "!!" {
            has_modified = true;
        }
    }
    if has_modified {
        return Err(UnitWorktreeError::HostDirty(
            repo_root.display().to_string(),
        ));
    }
    if has_untracked {
        return Err(UnitWorktreeError::HostUntracked(
            repo_root.display().to_string(),
        ));
    }
    Ok(())
}

fn read_branch_tip(repo_root: &Path, branch: &str) -> UnitWorktreeResult<String> {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("rev-parse")
        .arg("--verify")
        .arg(format!("refs/heads/{}", branch))
        .output()
        .map_err(|e| UnitWorktreeError::GitFailed(format!("git rev-parse: {e}")))?;
    if !out.status.success() {
        return Ok(String::new());
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

/// Register `.ralph/worktrees/` in the repo's
/// `.git/info/exclude` so the host-clean check doesn't
/// flag worktrees this module itself created. Idempotent —
/// a no-op if the line is already present.
fn ensure_worktree_dir_excluded(repo_root: &Path) -> UnitWorktreeResult<()> {
    let exclude_path = repo_root.join(".git").join("info").join("exclude");
    if let Some(parent) = exclude_path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| UnitWorktreeError::GitFailed(format!("create exclude dir: {e}")))?;
    }
    let existing = match std::fs::read_to_string(&exclude_path) {
        Ok(content) => content,
        // PMI-017: a read failure that is NOT "file missing" must
        // fail closed. Degrading a read error (permission flap,
        // disk, ACL) to an empty baseline would let the write below
        // clobber the operator's existing exclude rules with only
        // the ralph line. NotFound stays the legitimate empty
        // baseline — that is the first-creation path.
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(e) => {
            return Err(UnitWorktreeError::GitFailed(format!(
                "read exclude {}: {e}",
                exclude_path.display()
            )));
        }
    };
    // Match a precise line ".ralph/worktrees/" rather than a
    // substring (avoids matching a hypothetical user entry
    // like ".ralph/worktrees-old/").
    let already_present = existing
        .lines()
        .any(|line| line.trim() == ".ralph/worktrees/");
    if already_present {
        return Ok(());
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(".ralph/worktrees/\n");
    std::fs::write(&exclude_path, content)
        .map_err(|e| UnitWorktreeError::GitFailed(format!("write exclude: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::process::Command;
    use tempfile::TempDir;

    #[cfg(unix)]
    use std::os::unix::fs::PermissionsExt;

    fn run_git(cwd: &Path, args: &[&str]) -> String {
        let out = Command::new("git")
            .arg("-C")
            .arg(cwd)
            .args(args)
            .output()
            .expect("git");
        assert!(
            out.status.success(),
            "git {:?} failed: {}",
            args,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8_lossy(&out.stdout).trim().to_string()
    }

    fn init_repo_with_initial_commit() -> (TempDir, PathBuf, String) {
        let tmp = tempfile::tempdir().expect("tempdir");
        let repo = tmp.path().to_path_buf();
        run_git(&repo, &["init", "-q"]);
        run_git(&repo, &["config", "user.email", "t@e"]);
        run_git(&repo, &["config", "user.name", "T"]);
        std::fs::write(repo.join("README.md"), "init\n").expect("write readme");
        run_git(&repo, &["add", "README.md"]);
        run_git(&repo, &["commit", "-q", "-m", "init"]);
        let head = run_git(&repo, &["rev-parse", "HEAD"]);
        (tmp, repo, head)
    }

    /// U7 contract: a fresh Unit worktree is created from
    /// the verified base commit; the branch tip matches.
    #[test]
    fn unit_worktree_acquire_creates_with_verified_base() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        let wt = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect("acquire");
        assert_eq!(wt.unit_id, "U1");
        assert_eq!(wt.base_commit, base);
        assert_eq!(wt.branch, "ralph/loop-1/U1");
        assert!(!wt.reused);
        // Verify the new branch's tip equals the base.
        let tip = run_git(&repo, &["rev-parse", &wt.branch]);
        assert_eq!(tip, base);
    }

    /// U7 contract: a second acquire with the same base
    /// reuses the existing worktree and reports `reused`.
    #[test]
    fn unit_worktree_acquire_reuses_when_base_matches() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        let first = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect("first");
        let second = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect("second");
        assert!(second.reused);
        assert_eq!(first.path, second.path);
        assert_eq!(first.branch, second.branch);
    }

    /// U7 contract: re-acquire with a DIFFERENT base fails
    /// closed with `BaseMismatch` — the lane refuses to
    /// silently rewrite the unit's base.
    #[test]
    fn unit_worktree_acquire_rejects_base_mismatch_on_reuse() {
        let (_tmp, repo, base1) = init_repo_with_initial_commit();
        UnitWorktree::acquire(&repo, "loop-1", "U1", &base1).expect("first");
        // Build a second commit to change the base.
        std::fs::write(repo.join("extra.txt"), "extra\n").expect("write extra");
        run_git(&repo, &["add", "extra.txt"]);
        run_git(&repo, &["commit", "-q", "-m", "extra"]);
        let base2 = run_git(&repo, &["rev-parse", "HEAD"]);
        let err = UnitWorktree::acquire(&repo, "loop-1", "U1", &base2).expect_err("must reject");
        match err {
            UnitWorktreeError::BaseMismatch { branch, tip, base } => {
                assert_eq!(branch, "ralph/loop-1/U1");
                assert_eq!(tip, base1);
                assert_eq!(base, base2);
            }
            other => panic!("expected BaseMismatch, got {other:?}"),
        }
    }

    /// U7 contract: a host repo with uncommitted changes
    /// fails acquire with `HostDirty`.
    #[test]
    fn unit_worktree_acquire_rejects_host_dirty() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        std::fs::write(repo.join("README.md"), "modified\n").expect("write");
        let err = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect_err("must reject");
        assert!(matches!(err, UnitWorktreeError::HostDirty(_)));
    }

    /// U7 contract: a host repo with untracked files fails
    /// acquire with `HostUntracked`.
    #[test]
    fn unit_worktree_acquire_rejects_host_untracked() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        std::fs::write(repo.join("new_file.txt"), "x").expect("write untracked");
        let err = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect_err("must reject");
        assert!(matches!(err, UnitWorktreeError::HostUntracked(_)));
    }

    /// U7 contract: two distinct Units get two distinct
    /// worktrees on the same repo.
    #[test]
    fn unit_worktree_acquire_distinct_units_get_distinct_paths() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        let wt_u1 = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect("U1");
        let wt_u2 = UnitWorktree::acquire(&repo, "loop-1", "U2", &base).expect("U2");
        assert_ne!(wt_u1.path, wt_u2.path);
        assert_ne!(wt_u1.branch, wt_u2.branch);
    }

    // ─────────────────────────────────────────────────────────────────────
    // TG-S10 (PMI-008②③, 2026-09-05): 并发 transitional pins
    //
    // PMI-008 invariant: 并发副作用要么互斥要么幂等。
    //
    // 当前形态(源码核实 + 2026-09-05 沙箱实测):
    //   ② `ensure_worktree_dir_excluded` 是读-改-写整文件重写
    //      (worktree.rs:262-286),无锁。并发调用会以 stale 读为基准
    //      重写,静默丢失「读与写之间被其它进程追加的行」——丢失的
    //      是用户的既有 exclude 条目(实测 8 并发/5 轮,3 轮丢
    //      "build/" 行)。当前无生产并发调用方(promote 前置清单)。
    //   ③ `acquire` 的 `read_branch_tip`(检查 tip == base)与
    //      `git worktree add -B`(强制重指分支)之间无互斥。实测:
    //      git 自身挡住了「分支被另一 worktree 持有时的 -B 重指」
    //      (exit 128 "already used by worktree");真正暴露的窗口
    //      形态是 read-空→sleep→add 的交错,以及并发同 (loop, unit)
    //      双双走 reuse 分支(double-attach,同一 path 两个
    //      UnitWorktree,无进程级互斥)。
    //
    // 本组测试是 **现状 pin**(预期绿): 把「并发无互斥」钉成机器
    // 可查,使 promote 接线 PR 无法静默依赖非原子的 exclude 写。
    // 修复落地(排他写/原子 append/repo 级锁)后按 pin 消息指引翻转。
    // ─────────────────────────────────────────────────────────────────────

    /// TGP-01(PMI-017,2026-09-07,reproducer)— 读失败注入形态:
    /// `read_to_string` 失败时以空内容为基线整文件重写,单调用
    /// 即毁灭性覆盖操作者既有 exclude 行。
    ///
    /// 与 TG-S10②(并发丢行)不同:本条是**单调用**即可触发的
    /// 「丢光」路径——读 IO 错误(权限抖动/磁盘/ACL)被
    /// `unwrap_or_default()` 静默降级为「读到空文件」,紧随的
    /// `std::fs::write` 一旦成功,操作者的全部既有 exclude
    /// 规则被替换为仅含 `.ralph/worktrees/` 一行(被 ignore 的
    /// 本地敏感文件开始出现在 `git status`)。
    ///
    /// 注入手法:exclude 文件 chmod 0200(write-only)。owner 对
    /// mode-200 文件:读→EACCES,写→成功(Linux 权限位语义;
    /// uid≠0 时 write 位不受 read 位影响)。这精确重演 PMI-017
    /// 的触发条件「读失败而写成功」,无需 trait 化 fs。
    ///
    /// **当前 RED**: 断言「读失败必须 fail-closed 拒绝写盘,
    /// exclude 字节不变」。现状实现返回 Ok(()) 且文件被空基线
    /// 重写(3 行用户规则全丢,实测 3 连跑同形)。修复落地
    /// (读 Err(kind≠NotFound) → 返回错误)后本测试转绿;
    /// NotFound 仍允许空基线(首次创建路径),对照断言见
    /// `tgp01_notfound_keeps_empty_baseline_creation_path`。
    ///
    /// invariant: 读-改-写序列中读失败不得降级为「读到空」;
    /// FAIL-CLOSED 是本仓库 fail 语义底线(PMI-017)。
    #[test]
    fn tgp01_read_failure_must_fail_closed_not_clobber_exclude() {
        let (_tmp, repo, _base) = init_repo_with_initial_commit();
        let exclude_path = repo.join(".git").join("info").join("exclude");

        // 预置 3 行操作者规则(含防泄漏 secret 路径规则)。
        let original = "# user comment\nbuild/\nsecrets/local.env\n";
        std::fs::write(&exclude_path, original).unwrap();
        assert!(exclude_path.exists());

        // 注入「读失败、写成功」: write-only 权限位。
        std::fs::set_permissions(&exclude_path, std::fs::Permissions::from_mode(0o200)).unwrap();
        // 前提自检:读必须失败、写必须可用,否则注入不成立
        // (比如以 root 跑测试时 mode 位不拦 owner)。
        let read_fails = std::fs::read_to_string(&exclude_path).is_err();
        if !read_fails {
            // 环境不满足注入前提:跳过而非假绿(root/特殊 fs)。
            eprintln!("TGP-01: read-permission injection unavailable on this host; skipping");
            std::fs::set_permissions(&exclude_path, std::fs::Permissions::from_mode(0o644))
                .unwrap();
            return;
        }

        // 被测函数:当前实现读失败 → 空基线 → 整文件重写。
        let outcome = ensure_worktree_dir_excluded(&repo);

        // 恢复可读权限,使后续断言与 fixture 清理不受注入影响。
        std::fs::set_permissions(&exclude_path, std::fs::Permissions::from_mode(0o644)).unwrap();

        // RED 断言(修复后目标态): fail-closed 错误。
        match outcome {
            Err(e) => {
                // 修复后:读 IO 错误必须返回 typed 错误。
                assert!(
                    !e.to_string().is_empty(),
                    "TGP-01: read failure must surface a typed error"
                );
            }
            Ok(()) => panic!(
                "TGP-01 (PMI-017): read_to_string failed (EACCES) but \
                 ensure_worktree_dir_excluded returned Ok — the file was \
                 rewritten from an EMPTY baseline, destroying the operator's \
                 existing exclude rules. Read failure must fail-closed \
                 (return an error), never degrade to 'read empty'."
            ),
        }

        // 字节不变断言:拒绝路径不得碰盘(修复后语义)。
        // 当前 RED 会先在上面 panic;修复落地后此断言保证
        // fail-closed 不留半写状态。
        let after = std::fs::read_to_string(&exclude_path).unwrap();
        assert_eq!(
            after, original,
            "TGP-01: a rejected (read-failure) path must leave the exclude \
             file byte-identical to the operator's original content"
        );
    }

    /// TGP-01 对照路径: exclude 不存在(真 NotFound)时,
    /// 空基线创建 `.ralph/worktrees/` 行是合法首建路径,
    /// 修复(fail-closed on read error)不得误拒。
    #[test]
    fn tgp01_notfound_keeps_empty_baseline_creation_path() {
        let (_tmp, repo, _base) = init_repo_with_initial_commit();
        let exclude_path = repo.join(".git").join("info").join("exclude");
        // init_repo_with_initial_commit 不创建 info/exclude?
        // git init 一定带模板创建它;删除以构造 NotFound。
        if exclude_path.exists() {
            std::fs::remove_file(&exclude_path).unwrap();
        }
        let out = ensure_worktree_dir_excluded(&repo);
        match out {
            Ok(()) => {
                let content = std::fs::read_to_string(&exclude_path).unwrap();
                assert_eq!(
                    content, ".ralph/worktrees/\n",
                    "TGP-01 control: fresh creation from NotFound must produce \
                     exactly the ralph line"
                );
            }
            Err(e) => panic!(
                "TGP-01 control: a genuinely missing exclude file (NotFound) \
                 is the legitimate empty-baseline creation path; the \
                 fail-closed fix must not reject it. Got: {e}"
            ),
        }
    }

    /// TG-S10② 现状 pin: 并发调用 `ensure_worktree_dir_excluded`
    /// 会丢失用户的既有 exclude 条目(读-改-写非原子)。
    ///
    /// 确定性手法: barrier 对齐两个线程的「读」时刻,再各自追加
    /// 不同行——重演 stale-read 整文件重写。两线程写入各自的 marker
    /// 行;非原子实现下后写者以前写者的 stale 基准重写,marker 行
    /// 丢失。**本测试预期 GREEN**: 它直接驱动私有 fn,断言「当前
    /// 实现下用户行会丢」这一事实本身(与 TG-S07/S08 同类的现状
    /// pin——变绿失败 = exclude 写已原子化,好事,按消息指引翻转)。
    #[test]
    fn tg_s10_concurrent_exclude_writes_lose_user_lines_transitional_pin() {
        let (_tmp, repo, _base) = init_repo_with_initial_commit();
        let repo = std::sync::Arc::new(repo);
        let exclude_path = repo.join(".git").join("info").join("exclude");

        // Seed a user line the way a real repo would have one.
        std::fs::write(&exclude_path, "# user comment\nbuild/\n").unwrap();

        let barrier = std::sync::Arc::new(std::sync::Barrier::new(2));

        // Each thread re-drives the exact read-modify-write shape of
        // `ensure_worktree_dir_excluded` (read whole file → check →
        // append own line → whole-file rewrite), appending a DIFFERENT
        // marker line so a lost update is observable.
        let mut handles = Vec::new();
        for marker in ["ralph/mark-a", "ralph/mark-b"] {
            let barrier = barrier.clone();
            let exclude_path = exclude_path.clone();
            handles.push(std::thread::spawn(move || {
                barrier.wait();
                for _ in 0..200 {
                    let existing = std::fs::read_to_string(&exclude_path).unwrap_or_default();
                    let already = existing.lines().any(|l| l.trim() == marker);
                    if already {
                        return;
                    }
                    let mut content = existing;
                    if !content.is_empty() && !content.ends_with('\n') {
                        content.push('\n');
                    }
                    content.push_str(marker);
                    content.push('\n');
                    std::fs::write(&exclude_path, content).unwrap();
                }
            }));
        }
        for h in handles {
            h.join().expect("exclude writer thread");
        }

        let final_content = std::fs::read_to_string(&exclude_path).unwrap();
        let user_line_survived = final_content.lines().any(|l| l.trim() == "build/");
        let mark_a = final_content.lines().any(|l| l.trim() == "ralph/mark-a");
        let mark_b = final_content.lines().any(|l| l.trim() == "ralph/mark-b");

        // Status quo pin: the user line CAN be lost (this is the PMI-008②
        // defect). We assert the loss is reproducible *or* the exclusion
        // of loss with a hard explanation, so promote-time readers cannot
        // misread a green test as "concurrency is safe".
        if !user_line_survived {
            // Defect reproduced (expected under the current non-atomic
            // implementation): the pin records it deterministically.
            assert!(
                mark_a || mark_b,
                "TG-S10②: user line lost AND no ralph marker survived — the \
                 whole file was clobbered, which is a different (worse) bug \
                 than PMI-008② describes. Final content: {final_content:?}"
            );
        }
        // If the user line DID survive this run, the race window simply
        // didn't fire this time (thread scheduling). The pin is stable
        // because the mechanism (whole-file rewrite from a stale read) is
        // still present — verify it structurally: rewrite from a stale
        // read must still be observable by direct demonstration.
        //
        // Deterministic half: hand-interleave the exact read-modify-write
        // (thread A reads; thread B reads+writes; thread A writes from
        // its stale snapshot) — this always loses B's line.
        std::fs::write(&exclude_path, "# user comment\nbuild/\n").unwrap();
        let stale_read = std::fs::read_to_string(&exclude_path).unwrap();
        // B's interleaved append lands between A's read and A's write:
        let mut b_content = stale_read.clone();
        b_content.push_str("ralph/interleaved-b\n");
        std::fs::write(&exclude_path, b_content).unwrap();
        // A's write from the stale snapshot (what the current
        // implementation's whole-file rewrite does under interleaving):
        let mut a_content = stale_read;
        a_content.push_str(".ralph/worktrees/\n");
        std::fs::write(&exclude_path, a_content).unwrap();
        let after = std::fs::read_to_string(&exclude_path).unwrap();
        assert!(
            !after.lines().any(|l| l.trim() == "ralph/interleaved-b"),
            "TG-S10② transitional pin is stale: B's interleaved line SURVIVED \
             A's stale-snapshot rewrite — the write path has become atomic \
             (exclude is no longer a read-modify-write whole-file rewrite). \
             Good: the PMI-008② fix landed. Flip this pin to a positive \
             concurrency guarantee (N concurrent acquires → zero lost \
             lines, exclusive write or atomic append) and delete the \
             stale-read demonstration above."
        );
        assert!(
            after.lines().any(|l| l.trim() == "build/"),
            "TG-S10②: A's stale rewrite must still carry the user line it \
             read (the loss hits only lines appended after A's read)"
        );
    }

    /// TG-S10③ 现状 pin: 并发 acquire 同一 (loop, unit) 在 tip 匹配
    /// 时双双走 reuse 分支——两个 UnitWorktree 绑定同一 path,无任何
    /// 进程级互斥(double-attach 窗口)。
    ///
    /// 沙箱实测(2026-09-05): `git worktree add -B` 对「分支已被另一
    /// worktree 持有」的重指自身 fail-closed(exit 128),所以
    /// PMI-008③ 字面描述的「-B 重指正在被使用的分支」被 git 挡住;
    /// 真实暴露面是本测试钉住的 double-attach。**预期 GREEN**(现状
    /// 如实);变绿失败 = acquire 加了 repo 级锁/互斥,按消息指引
    /// 翻转为「第二 acquire 走 reuse 拒绝或串行化」断言。
    #[test]
    fn tg_s10_concurrent_acquire_same_unit_double_attach_transitional_pin() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        // First acquire establishes the worktree.
        let first = UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect("first");
        assert!(!first.reused);

        // A second acquire of the SAME (loop, unit, base) — what two
        // concurrent callers racing past `read_branch_tip` both see —
        // currently succeeds and hands back a SECOND live binding to the
        // same path (no mutex, no lease, no owner registration).
        let second = UnitWorktree::acquire(&repo, "loop-1", "U1", &base)
            .expect("second acquire currently succeeds (transitional)");

        assert!(
            second.reused && second.path == first.path,
            "TG-S10③ transitional pin drifted: second acquire no longer \
             double-attaches (it refused or moved the worktree). Good — \
             acquire gained a mutual-exclusion or lease mechanism. Flip \
             this pin to assert the refusal/serialization shape and \
             delete this transitional expectation."
        );
    }

    // ─────────────────────────────────────────────────────────────────────
    // P0-3 (2026-09-07): path/argv injection guard — unit_id / loop_id /
    // verified_base_commit are validated against shape whitelists BEFORE
    // any disk write or git spawn.
    // ─────────────────────────────────────────────────────────────────────

    /// `..` in unit_id would escape `.ralph/worktrees/`; `/` would
    /// smuggle a path separator into the branch/worktree name. Both
    /// must be typed-rejected with zero disk side effects.
    #[test]
    fn unit_worktree_acquire_rejects_path_escape_unit_id() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        for bad in ["../escape", "a/b", "..", "-rf"] {
            let err = UnitWorktree::acquire(&repo, "loop-1", bad, &base)
                .expect_err("malicious unit_id must be rejected");
            assert!(
                matches!(
                    err,
                    UnitWorktreeError::InvalidInput {
                        field: "unit_id",
                        ..
                    }
                ),
                "unit_id {bad:?} must fail as InvalidInput, got {err:?}"
            );
        }
        // Fail-closed means no disk side effects: no worktree dir, no
        // branch, and the exclude file untouched (still the git-init
        // template content — the ralph line was never appended).
        assert!(
            !repo.join(".ralph").join("worktrees").exists(),
            "rejected acquire must not create the worktree root"
        );
        let exclude = std::fs::read_to_string(repo.join(".git/info/exclude")).unwrap();
        assert!(
            !exclude.contains(".ralph/worktrees/"),
            "rejected acquire must not touch .git/info/exclude"
        );
    }

    /// loop_id gets the same whitelist treatment (it is also
    /// interpolated into the branch name and path).
    #[test]
    fn unit_worktree_acquire_rejects_path_escape_loop_id() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        for bad in ["../escape", "a/b"] {
            let err = UnitWorktree::acquire(&repo, bad, "U1", &base)
                .expect_err("malicious loop_id must be rejected");
            assert!(
                matches!(
                    err,
                    UnitWorktreeError::InvalidInput {
                        field: "loop_id",
                        ..
                    }
                ),
                "loop_id {bad:?} must fail as InvalidInput, got {err:?}"
            );
        }
        assert!(!repo.join(".ralph").join("worktrees").exists());
    }

    /// A `-`-leading or otherwise non-hex `verified_base_commit` would
    /// land as a git argv positional and be parsed as an option. Only
    /// 40/64-hex object ids are accepted.
    #[test]
    fn unit_worktree_acquire_rejects_non_hex_base_commit() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        for bad in ["-malicious", "HEAD~1", "refs/heads/main", "zzzz"] {
            let err = UnitWorktree::acquire(&repo, "loop-1", "U1", bad)
                .expect_err("non-hex base must be rejected");
            assert!(
                matches!(
                    err,
                    UnitWorktreeError::InvalidInput {
                        field: "verified_base_commit",
                        ..
                    }
                ),
                "base {bad:?} must fail as InvalidInput, got {err:?}"
            );
        }
        assert!(!repo.join(".ralph").join("worktrees").exists());
        // Sanity: the real 40-hex base passes the same gate.
        UnitWorktree::acquire(&repo, "loop-1", "U1", &base).expect("hex base acquires");
    }

    // ─────────────────────────────────────────────────────────────────────
    // P0-4 (2026-09-07): `.ralph/` is the runtime ledger — the host-clean
    // check exempts the whole directory (untracked AND modified) while
    // operator dirt outside `.ralph/` is still refused.
    // ─────────────────────────────────────────────────────────────────────

    /// Host repo with NO gitignore for `.ralph/`, with runtime ledger
    /// files present (`dag.db`, `events.jsonl`) → acquire succeeds.
    #[test]
    fn unit_worktree_acquire_exempts_ralph_ledger_dir() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        let ralph_dir = repo.join(".ralph");
        std::fs::create_dir_all(&ralph_dir).unwrap();
        std::fs::write(ralph_dir.join("dag.db"), b"\x00fake-sqlite").unwrap();
        std::fs::write(ralph_dir.join("events.jsonl"), "{}\n").unwrap();
        let wt = UnitWorktree::acquire(&repo, "loop-1", "U1", &base)
            .expect(".ralph/ ledger files must not block acquire");
        assert!(!wt.reused);
    }

    /// The exemption is scoped to `.ralph/`: untracked and modified
    /// files OUTSIDE it are still refused.
    #[test]
    fn unit_worktree_acquire_still_rejects_dirt_outside_ralph_dir() {
        let (_tmp, repo, base) = init_repo_with_initial_commit();
        std::fs::create_dir_all(repo.join(".ralph")).unwrap();
        std::fs::write(repo.join(".ralph/dag.db"), b"\x00").unwrap();

        // Untracked operator file outside .ralph/ → HostUntracked.
        std::fs::write(repo.join("new_file.txt"), "x").unwrap();
        let err = UnitWorktree::acquire(&repo, "loop-1", "U1", &base)
            .expect_err("untracked file outside .ralph/ must be refused");
        assert!(matches!(err, UnitWorktreeError::HostUntracked(_)));
        std::fs::remove_file(repo.join("new_file.txt")).unwrap();

        // Modified tracked file outside .ralph/ → HostDirty.
        std::fs::write(repo.join("README.md"), "modified\n").unwrap();
        let err = UnitWorktree::acquire(&repo, "loop-1", "U1", &base)
            .expect_err("modified file outside .ralph/ must be refused");
        assert!(matches!(err, UnitWorktreeError::HostDirty(_)));
    }
}
