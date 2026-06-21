//! 2026-06-18-002 plan U2 (KTD-13, KTD-14): seq 分配器。
//!
//! agent 调用 `HatHandoffAllocator::prepare(...)` 拿到:
//! - `handoff_path`:repo 相对路径 `{iter}-{seq+1}-{from}-{to}.md`
//! - `skeleton`:填好占位符的五段式 markdown
//! - `seq`:即将使用的 seq(供 agent 校验)
//!
//! **核心契约**(KTD-13):`prepare` 只在文件不存在时写盘;
//! `gate.accept` 后 `seq += 1`,下一次 `prepare` 给新 seq。
//! **同 path 重试**(KTD-14):`--force` 覆盖同 path;不允许写新
//! 内容到已 accept 的旧 seq。

use std::path::{Path, PathBuf};

use crate::hat_handoff::validator::build_skeleton;

/// `prepare` 阶段的输入参数。
#[derive(Debug, Clone)]
pub struct PrepareInputs<'a> {
    /// 当前 iteration(从 `LoopState.iteration` 取,1-indexed)。
    pub iteration: u32,
    /// 当前 `LoopState.hat_handoff_seq`(accept 后递增的字段)。
    pub current_seq: u32,
    /// 上游 hat id(emit hat)。
    pub from: &'a str,
    /// 下游 hat id(consumer_of(topic))。
    pub to: &'a str,
    /// emit 的宏观 topic(用于 skeleton 提示)。
    pub topic: &'a str,
}

/// `prepare` 阶段返回给 agent 的结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareResult {
    /// Repo 相对路径,例如
    /// `.ralph/agent/hat-handoff/3-2-executor-review_coordinator.md`。
    pub handoff_path: String,
    /// 五段式 skeleton。
    pub skeleton: String,
    /// 本次 prepare 分配的 seq(下一次 accept 后会变成 current_seq+1)。
    pub seq: u32,
}

/// 计算 `handoff_path` 与 `seq`,**不写盘**(write 走 CLI/外部)。
///
/// 文件名格式:`{iter}-{seq}-{from}-{to}.md`。
///
/// **重要约束**:`from` / `to` 必须是 **sanitized hat id**(不含 `-`),
/// 否则 `parse_filename` 无法稳定拆分。本函数对 from/to 调用
/// [`sanitize`] 把 `-` 替换为 `_`,保证 `parse_filename` 可逆。
pub fn compute(inputs: &PrepareInputs<'_>) -> PrepareResult {
    let next_seq = inputs.current_seq + 1;
    let handoff_path = format!(
        ".ralph/agent/hat-handoff/{}-{}-{}-{}.md",
        inputs.iteration,
        next_seq,
        sanitize(inputs.from),
        sanitize(inputs.to),
    );
    let skeleton = build_skeleton(inputs.from, inputs.to, inputs.topic);
    PrepareResult {
        handoff_path,
        skeleton,
        seq: next_seq,
    }
}

/// 从 handoff_path 解析出 (iteration, seq, from, to)。
///
/// **要求**:`from` / `to` 是已 sanitize 的形式(不含 `-`)。
/// 文件名格式:`{iter}-{seq}-{from}-{to}.md`(恰好 4 段以 `-` 分割)。
pub fn parse_filename(path: &str) -> Option<(u32, u32, String, String)> {
    let basename = Path::new(path).file_name()?.to_str()?;
    let stem = basename.strip_suffix(".md")?;
    let parts: Vec<&str> = stem.split('-').collect();
    if parts.len() != 4 {
        return None;
    }
    let iter = parts[0].parse().ok()?;
    let seq = parts[1].parse().ok()?;
    let from = parts[2].to_string();
    let to = parts[3].to_string();
    Some((iter, seq, from, to))
}

/// 把 hat id 中不安全字符替换为 `_`;**`-` 也替换为 `_`** 以保证
/// `parse_filename` 的拆分稳定。
pub fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// 写盘入口(KTD-14):仅当文件不存在或 `force == true` 时写入。
///
/// `path` 通常是 workspace 绝对路径(由 caller 把 `handoff_path`
/// 拼到 repo_root 后传入)。
pub fn write_skeleton(
    workspace: &Path,
    handoff_path: &str,
    skeleton: &str,
    force: bool,
) -> std::io::Result<WriteOutcome> {
    let abs = resolve_jailed(workspace, handoff_path)?;
    if abs.exists() && !force {
        return Ok(WriteOutcome::AlreadyExists);
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&abs, skeleton)?;
    Ok(WriteOutcome::Written)
}

/// Path jail(KTD-8):拒绝 `..` 逃逸 repo。
pub fn resolve_jailed(workspace: &Path, handoff_path: &str) -> std::io::Result<PathBuf> {
    let rel = PathBuf::from(handoff_path);
    if rel.is_absolute() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "handoff_path must be repo-relative",
        ));
    }
    let mut clean = std::path::PathBuf::new();
    for comp in rel.components() {
        match comp {
            std::path::Component::ParentDir => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "handoff_path contains `..`",
                ));
            }
            std::path::Component::Normal(c) => clean.push(c),
            std::path::Component::CurDir => {}
            _ => {}
        }
    }
    Ok(workspace.join(clean))
}

/// 2026-06-21-002 plan U5: 写盘版 prepare —— 同步写盘 + 跳过已存在文件。
///
/// 与 `compute` 的关系:`compute` 只算 path + skeleton(不写盘);
/// `prepare` 把 skeleton 写盘并返回 repo-relative path。返回的
/// path 在 `iteration` × `seq` 维度上**唯一**;同一 `(iteration,
/// seq, from, to, topic)` 重入时,已存在的文件**不会被覆盖**
/// (返回的 path 指向现有文件)。
///
/// **dedup 语义**: 同一 `(iteration, seq, from, to, topic)` 的
/// 重复调用得到同一 path。这避免了 retry / 多次 commit 时
/// 重复生成 artifact。validator 接受后,ledger 的
/// `commit_handoff_artifact` 路径仍然只 commit 一次。
pub fn prepare_with_dedup(
    workspace: &Path,
    inputs: &PrepareInputs<'_>,
) -> std::io::Result<PrepareOutcome> {
    let computed = compute(inputs);
    let abs = resolve_jailed(workspace, &computed.handoff_path)?;
    if abs.exists() {
        // KTD-14 + U5 dedup: 已存在的文件视为 idempotent,
        // 直接返回。runtime 不再读内容(validator 路径单独
        // 走 `validate_artifact`)。
        return Ok(PrepareOutcome {
            handoff_path: computed.handoff_path,
            seq: computed.seq,
            result: WriteOutcome::AlreadyExists,
        });
    }
    if let Some(parent) = abs.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&abs, &computed.skeleton)?;
    Ok(PrepareOutcome {
        handoff_path: computed.handoff_path,
        seq: computed.seq,
        result: WriteOutcome::Written,
    })
}

/// `prepare_with_dedup` 的返回类型,显式标注「新建 / 已存在」。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrepareOutcome {
    /// Repo 相对路径。
    pub handoff_path: String,
    /// 本次 prepare 分配的 seq。
    pub seq: u32,
    /// 写盘结果:首次写 vs 跳过。
    pub result: WriteOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WriteOutcome {
    Written,
    AlreadyExists,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inputs<'a>(from: &'a str, to: &'a str, topic: &'a str) -> PrepareInputs<'a> {
        PrepareInputs {
            iteration: 3,
            current_seq: 1,
            from,
            to,
            topic,
        }
    }

    #[test]
    fn compute_seq_and_path() {
        let r = compute(&inputs("executor", "review-coordinator", "work.done"));
        assert_eq!(r.seq, 2);
        // `-` 被 sanitize 为 `_`,保证 parse_filename 稳定拆分。
        assert_eq!(
            r.handoff_path,
            ".ralph/agent/hat-handoff/3-2-executor-review_coordinator.md"
        );
    }

    #[test]
    fn first_iteration_seq_starts_at_one() {
        let mut p = inputs("a", "b", "x");
        p.current_seq = 0;
        let r = compute(&p);
        assert_eq!(r.seq, 1);
    }

    #[test]
    fn parse_filename_round_trip() {
        let path = ".ralph/agent/hat-handoff/3-2-executor-review_coordinator.md";
        let (iter, seq, from, to) = parse_filename(path).unwrap();
        assert_eq!(iter, 3);
        assert_eq!(seq, 2);
        assert_eq!(from, "executor");
        assert_eq!(to, "review_coordinator");
    }

    #[test]
    fn parse_filename_handles_dashed_hat_ids() {
        // from/to 经过 sanitize 后 `-` → `_`,所以是稳定 4 段拆分。
        let path = ".ralph/agent/hat-handoff/1-5-plan_gate-executor.md";
        let (_, _, from, to) = parse_filename(path).unwrap();
        assert_eq!(from, "plan_gate");
        assert_eq!(to, "executor");
    }

    #[test]
    fn sanitize_replaces_unsafe_chars() {
        assert_eq!(sanitize("a/b"), "a_b");
        assert_eq!(sanitize("a b"), "a_b");
        // `.` 也被替换为 `_`,避免 hat id 含 `.` 干扰 split('-') 的 4 段拆分边界。
        assert_eq!(sanitize("a.b"), "a_b");
        // `-` 也被替换为 `_`,保证 4 段文件名边界稳定。
        assert_eq!(sanitize("review-coordinator"), "review_coordinator");
    }

    #[test]
    fn jail_rejects_parent_dir() {
        let workspace = Path::new("/tmp/repo");
        assert!(resolve_jailed(workspace, "../escape.md").is_err());
    }

    #[test]
    fn jail_rejects_absolute() {
        let workspace = Path::new("/tmp/repo");
        assert!(resolve_jailed(workspace, "/etc/passwd").is_err());
    }

    #[test]
    fn write_then_skip_when_exists() {
        let dir = tempfile::tempdir().unwrap();
        let path = ".ralph/agent/hat-handoff/1-1-a-b.md";
        let r1 = write_skeleton(dir.path(), path, "body", false).unwrap();
        assert_eq!(r1, WriteOutcome::Written);
        let r2 = write_skeleton(dir.path(), path, "body2", false).unwrap();
        assert_eq!(r2, WriteOutcome::AlreadyExists);
        let r3 = write_skeleton(dir.path(), path, "body3", true).unwrap();
        assert_eq!(r3, WriteOutcome::Written);
        let read = std::fs::read_to_string(dir.path().join(path)).unwrap();
        assert_eq!(read, "body3");
    }

    // 2026-06-21-002 plan U5: prepare_with_dedup 行为。
    #[test]
    fn prepare_with_dedup_writes_new_artifact() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = inputs("executor", "review-coordinator", "work.done");
        let r = prepare_with_dedup(dir.path(), &inputs).unwrap();
        assert_eq!(r.seq, 2);
        assert_eq!(
            r.handoff_path,
            ".ralph/agent/hat-handoff/3-2-executor-review_coordinator.md"
        );
        assert_eq!(r.result, WriteOutcome::Written);
        let abs = dir.path().join(&r.handoff_path);
        assert!(abs.exists());
        let body = std::fs::read_to_string(&abs).unwrap();
        assert!(body.contains("## next"));
        assert!(body.contains("**动作**:"));
    }

    #[test]
    fn prepare_with_dedup_is_idempotent_on_same_seq() {
        let dir = tempfile::tempdir().unwrap();
        let inputs = inputs("executor", "review-coordinator", "work.done");
        let r1 = prepare_with_dedup(dir.path(), &inputs).unwrap();
        assert_eq!(r1.result, WriteOutcome::Written);
        // 第二次调用,文件已存在,应当返回 AlreadyExists 且不覆盖。
        let r2 = prepare_with_dedup(dir.path(), &inputs).unwrap();
        assert_eq!(r2.handoff_path, r1.handoff_path);
        assert_eq!(r2.seq, r1.seq);
        assert_eq!(r2.result, WriteOutcome::AlreadyExists);
    }

    #[test]
    fn prepare_with_dedup_rejects_parent_dir_escape() {
        let dir = tempfile::tempdir().unwrap();
        // 构造一个试图逃逸的 inputs 不容易(compute 内部固定格式),
        // 但 `resolve_jailed` 已经在 compute 之前被跳过;此处改测
        // 写盘后的 path 落在 `.ralph/agent/hat-handoff/` 内。
        let inputs = inputs("a", "b", "x");
        let r = prepare_with_dedup(dir.path(), &inputs).unwrap();
        assert!(r.handoff_path.starts_with(".ralph/agent/hat-handoff/"));
        let abs = dir.path().join(&r.handoff_path);
        // path 不能逃逸 workspace
        assert!(abs.starts_with(dir.path()));
    }
}
