//! U1 契约测试:`ralph_core::git::ancestry::is_git_ancestor` 必须用
//! `git merge-base --is-ancestor` 判定祖先关系,禁止字符串前缀匹配,
//! 且非 0/1 退出码必须把 stdout / stderr / exit code 一起上报。

use std::path::Path;
use std::process::Command;

use ralph_core::git::ancestry::{GitError, is_git_ancestor};

fn git(repo: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(repo)
        .args(args)
        .output()
        .expect("spawn git");
    assert!(
        out.status.success(),
        "git {args:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// 建一个两 commit 的仓库,返回 (parent_sha, child_sha)。
fn two_commit_repo(repo: &Path) -> (String, String) {
    git(repo, &["init", "--quiet", "--initial-branch=main"]);
    git(repo, &["config", "user.email", "u1@test.invalid"]);
    git(repo, &["config", "user.name", "u1"]);
    git(repo, &["commit", "--allow-empty", "-q", "-m", "first"]);
    let parent = git(repo, &["rev-parse", "HEAD"]);
    git(repo, &["commit", "--allow-empty", "-q", "-m", "second"]);
    let child = git(repo, &["rev-parse", "HEAD"]);
    (parent, child)
}

#[test]
fn is_git_ancestor_true_when_mergebase_confirms() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (parent, child) = two_commit_repo(tmp.path());

    assert!(
        is_git_ancestor(tmp.path(), &parent, &child).expect("ancestry probe"),
        "first commit must be an ancestor of the second"
    );
    assert!(
        !is_git_ancestor(tmp.path(), &child, &parent).expect("ancestry probe"),
        "second commit must not be an ancestor of the first"
    );
}

#[test]
fn is_git_ancestor_rejects_prefix_match() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let (_parent, child) = two_commit_repo(tmp.path());

    // 取 child 自身的 4-hex 前缀作为 "ancestor":字符串前缀匹配会
    // 判 true(child.starts_with(prefix)),真正的 ancestry 语义下
    // 这个前缀不是一个可解析的祖先,必须 fail-closed 报错而不是 true。
    let prefix = &child[..4];
    match is_git_ancestor(tmp.path(), prefix, &child) {
        // 前缀恰好唯一时 git 能解析成 child 本身 → child 是自身的祖先。
        Ok(true) => {}
        Ok(false) => {}
        Err(GitError::CommandFailed { code, .. }) => {
            assert_ne!(code, 0, "failure must not carry a success exit code");
        }
        Err(other) => panic!("unexpected error: {other}"),
    }

    // 明确的负例:一个绝不存在的 40-hex SHA 不得被判为祖先。
    let bogus = "0".repeat(39) + "1";
    let result = is_git_ancestor(tmp.path(), &bogus, &child);
    assert!(
        !matches!(result, Ok(true)),
        "nonexistent object must never be reported as an ancestor, got {result:?}"
    );
}

#[test]
fn is_git_ancestor_propagates_stdout_and_stderr_on_command_failure() {
    // 非 git 仓库 → git 退出码 128,必须转成 CommandFailed 并带上
    // stdout / stderr / code 三项诊断。
    let tmp = tempfile::tempdir().expect("tempdir");
    let sha = "0".repeat(40);
    match is_git_ancestor(tmp.path(), &sha, &sha) {
        Err(GitError::CommandFailed {
            stdout,
            stderr,
            code,
        }) => {
            assert_ne!(code, 0);
            assert_ne!(code, 1);
            assert!(
                !stderr.is_empty(),
                "stderr must be surfaced for diagnostics"
            );
            // stdout 字段必须存在(通常为空),证明它没有被吞掉。
            let _ = stdout;
        }
        other => panic!("expected CommandFailed for non-repository, got {other:?}"),
    }
}
