//! Reproducer test for PMI-007 — `--reuse-worktree` 复用键双实现语义分叉
//! (post-merge-converge preset, .ralph/post-merge/findings/PMI-007.md)
//!
//! Invariant (PMI-007 §invariant):
//!   Worktree 复用规则 (HARD RULE 3) 的解析入口应当唯一。
//!
//! PMI-007 记录了同一概念（「--reuse-worktree 的精确复用键」）的两个平行实现:
//!   - run.rs 生产链: `worktree_file_name_prefix`（plan 分支**不排除** prompt
//!     哨兵, run.rs:746-753）→ `resolve_exact_worktree_name`（worktree_name
//!     **不过滤空串**, run.rs:777 透传 `Some("")`）。
//!   - run_recovery.rs gate 链: `exact_worktree_name_from`（plan stem 排除
//!     `prompt` 哨兵（大小写不敏感）, 空 `--worktree-name` 视为缺失,
//!     run_recovery.rs:165-183）。
//!
//! PMI-007 声称「当前行为一致」，但两条链在两个可达输入形状上分叉
//! （单元级证据: run.rs tests 的 tg_s14_* 三测，当前红）:
//!   1. `--plan PROMPT.md`（哨兵 stem）: gate 链拒绝 → None; 生产链返回
//!      Some("PROMPT")。
//!   2. `--worktree-name ""`（clap 无 value_parser 拦空值）: gate 链视为
//!      缺失回落; 生产链透传 Some("")。
//!
//! 本文件是**行为级**（integration）证据（TG-S14 integration 半边）:
//! 真实 `ralph run --worktree --reuse-worktree --plan PROMPT.md` 在沙箱
//! git repo 上跑（与 integration_resume.rs `reuse_gate` 模块同一 harness
//! 形态: setup_git_repo + backend true + ralph_bin() scrub）,2026-09-05
//! 沙箱实测（本 hat, 一次性 /tmp 沙箱, 已清理）:
//!
//! ```text
//! INFO: No existing worktree named PROMPT; creating the first exact-name worktree
//! INFO: Created worktree at /tmp/worktree/pmi007-sandbox/PROMPT on branch ralph/PROMPT
//! ```
//!
//! 即: 哨兵名 `PROMPT` 被当成精确复用键,真的创建出了名为 PROMPT 的
//! worktree —— 两参数 gate 版 `exact_worktree_name_rejects_prompt_stem`
//! 钉住的哨兵约定被生产链完全绕过（违反 PMI-007 §invariant: HARD RULE 3
//! 复用键解析入口不唯一,排除规则分散在两处）。
//!
//! Test status at HEAD (7d66123a, 2026-09-05):
//!   - `pmi_007_prompt_sentinel_plan_must_not_become_worktree_name` →
//!     FAILS（真实 CLI 创建了 PROMPT 名 worktree; 行为级证据见上）。
//!
//! 修复方向（fixer 阶段裁量,本测试只钉缺口）: 让
//! `resolve_exact_worktree_name` 委托 `exact_worktree_name_from` 并仅在
//! 尾部落 prefix 回退（PMI-007 §impact 的建议形态）——哨兵与空名过滤
//! 单点定义,两条链语义合一。修复落地后本测试转绿,保留为回归 pin
//! （与 repro_pmi_001 / repro_pmi_002 / repro_pmi_005 生命周期相同）。
//!
//! 注: 空名形状不做 integration 级（`--worktree-name ""` 的行为级后果
//! 是 `find_reusable_worktree_by_name` 静默 `Ok(None)` → 沙箱里创建空名
//! worktree,断言面与哨兵形状同构,单元级 tg_s14 已钉）; 也不测
//! `--plan prompt.md` 大小写变体的 integration 级（同构）。

use std::fs;
use std::path::Path;
use std::process::Command;

mod common;

use tempfile::TempDir;

/// Same harness shape as integration_resume.rs::reuse_gate::setup_git_repo.
fn setup_git_repo(path: &Path) {
    let git_init = Command::new("git")
        .args(["init"])
        .current_dir(path)
        .output()
        .expect("git init");
    assert!(git_init.status.success(), "git init failed");
    for (key, value) in [
        ("user.email", "test@example.com"),
        ("user.name", "Test User"),
    ] {
        let status = Command::new("git")
            .args(["config", key, value])
            .current_dir(path)
            .status()
            .expect("git config");
        assert!(status.success(), "git config {key} failed");
    }
    fs::write(path.join("README.md"), "# Test\n").expect("write README");
    let status = Command::new("git")
        .args(["add", "."])
        .current_dir(path)
        .status()
        .expect("git add");
    assert!(status.success(), "git add failed");
    let status = Command::new("git")
        .args(["commit", "-m", "Initial commit", "--quiet"])
        .current_dir(path)
        .status()
        .expect("git commit");
    assert!(status.success(), "git commit failed");
}

/// Worktree root the CLI actually uses (mirrors integration_resume.rs
/// `default_worktree_root`: `<tmp>/worktree/<repo-name>`).
fn worktree_root(main_repo: &Path) -> std::path::PathBuf {
    // CARGO_TARGET_TMPDIR is per-test-invocation under nextest, but the
    // worktree base is derived from the repo path itself
    // (`/tmp` + repo dir name), same as the sandbox evidence.
    let parent = main_repo.parent().expect("tempdir must have a parent");
    parent
        .join("worktree")
        .join(main_repo.file_name().expect("repo dir must have a name"))
}

/// PMI-007 行为级核心断言（TG-S14 integration）: `--plan PROMPT.md`
/// （默认 prompt 哨兵文件作 plan）不得把哨兵 stem `PROMPT` 当成
/// worktree 精确复用键——不得创建名为 PROMPT 的 worktree。
///
/// 判据不依赖 stderr 文案（文案会随日志级别/措辞演进),直接检查
/// 副作用: worktree 目录清单里**不存在** PROMPT 名条目,git worktree
/// 列表里不存在 `ralph/PROMPT` 分支条目。
#[test]
fn pmi_007_prompt_sentinel_plan_must_not_become_worktree_name() {
    let temp_dir = TempDir::new().expect("temp dir");
    let main_repo = temp_dir.path();
    setup_git_repo(main_repo);

    // The default prompt-file sentinel exists in the repo root, so the
    // `--plan PROMPT.md` path is a real, resolvable plan file (prompt
    // sources double as plan sources at run.rs:895-899).
    fs::write(
        main_repo.join("PROMPT.md"),
        "plan: PMI-007 sentinel repro\n",
    )
    .expect("write PROMPT.md");

    // Minimal `true` backend so the run dies at the preset lint gate (or
    // completes instantly) without ever starting a real agent — the reuse
    // key resolution and worktree creation happen BEFORE the backend.
    fs::write(
        main_repo.join("ralph.yml"),
        r#"event_loop:
  completion_promise: "loop_complete"
  max_iterations: 1
cli:
  backend: "custom"
  command: "true"
"#,
    )
    .expect("write ralph.yml");

    let output = common::ralph_bin()
        .args([
            "run",
            "--worktree",
            "--reuse-worktree",
            "--no-tui",
            "--skip-preflight",
            "--plan",
            "PROMPT.md",
        ])
        .current_dir(main_repo)
        .output()
        .expect("execute ralph run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);

    // The exact-name worktree the production chain created (sandbox
    // evidence: `Created worktree at <root>/PROMPT on branch ralph/PROMPT`).
    let sentinel_worktree = worktree_root(main_repo).join("PROMPT");
    let created_sentinel = sentinel_worktree.exists();

    let git_worktrees = Command::new("git")
        .args(["worktree", "list", "--porcelain"])
        .current_dir(main_repo)
        .output()
        .expect("git worktree list");
    let worktree_listing = String::from_utf8_lossy(&git_worktrees.stdout);
    let branch_created = worktree_listing.contains("ralph/PROMPT");

    assert!(
        !(created_sentinel || branch_created),
        "PMI-007: `--plan PROMPT.md` handed the prompt-file sentinel stem \
         `PROMPT` to the reuse-key resolution chain and it materialized as a \
         real worktree (dir exists: {created_sentinel}, branch ralph/PROMPT \
         in git worktree list: {branch_created}). The gate-side resolver \
         (`exact_worktree_name_from`) excludes the sentinel stem \
         case-insensitively; the production chain \
         (`worktree_file_name_prefix` plan branch → \
         `resolve_exact_worktree_name`) has no such filter — the two \
         parallel resolvers have drifted (PMI-007 §invariant: the reuse-key \
         resolution entry point must be single-source). \
         stdout: {stdout}\nstderr: {stderr}"
    );

    // Cleanup: drop the sentinel worktree if the buggy path created it, so
    // the TempDir teardown does not fight git's administrative state.
    if created_sentinel || branch_created {
        let _ = Command::new("git")
            .args(["worktree", "remove", "--force", "PROMPT"])
            .current_dir(main_repo)
            .output();
    }
}
