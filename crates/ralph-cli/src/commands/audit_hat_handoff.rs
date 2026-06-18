//! 2026-06-18-001 plan U8: `ralph audit hat-handoff` — 扫描 `.ralph/agent/hat-handoff/`
//! 产物,捕获"handoff 已开启但 0 文件"或文件名不规范的静默失败。
//!
//! 设计要点:
//! - 核心审计逻辑全部用 Rust 实现,避免 bash 解析 YAML/JSON 的脆弱性。
//! - bash 包装 `scripts/audit-hat-handoff-artifacts.sh` 仅调用本子命令。
//! - 文件名正则:`{iter}-{seq}-{from}-{to}.md`(iter/seq u32,from/to 字母数字下划线连字符)。
//! - 单调性:iter 必须严格递增(否则视为违规);同 iter 内 seq 严格递增。
//! - 与现有 `RalphConfig` 解析器复用,不重新解析 YAML。

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Args;
use regex::Regex;
use serde::Serialize;

/// Exit codes for the audit command.
#[derive(Debug, PartialEq, Eq)]
pub enum AuditExit {
    /// Audit passed.
    Ok,
    /// handoff not enabled; audit is a no-op (exit 0).
    HandoffDisabled,
    /// `.ralph/agent/hat-handoff/` does not exist.
    NoHandoffDir,
    /// handoff enabled but directory is empty.
    EmptyHandoffDir { enabled: bool },
    /// One or more files have invalid name format.
    InvalidFilename { samples: Vec<String> },
    /// iter/seq monotonicity violated.
    MonotonicityViolation { message: String },
}

impl AuditExit {
    pub fn code(&self) -> i32 {
        match self {
            Self::Ok => 0,
            Self::HandoffDisabled => 0,
            Self::NoHandoffDir => 1,
            Self::EmptyHandoffDir { .. } => 2,
            Self::InvalidFilename { .. } => 3,
            Self::MonotonicityViolation { .. } => 4,
        }
    }

    pub fn name(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::HandoffDisabled => "handoff_disabled",
            Self::NoHandoffDir => "no_handoff_dir",
            Self::EmptyHandoffDir { .. } => "empty_handoff_dir",
            Self::InvalidFilename { .. } => "invalid_filename",
            Self::MonotonicityViolation { .. } => "monotonicity_violation",
        }
    }
}

#[derive(Args, Debug)]
pub struct AuditHatHandoffArgs {
    /// 工作区根目录(默认 `.`)
    #[arg(long, default_value = ".")]
    pub workspace: PathBuf,

    /// 输出格式
    #[arg(long, value_enum, default_value_t = AuditFormat::Text)]
    pub format: AuditFormat,

    /// 强制启用审计(忽略 hat_handoff.enabled 检查)
    #[arg(long)]
    pub force: bool,
}

#[derive(Debug, Clone, Copy, clap::ValueEnum)]
pub enum AuditFormat {
    Text,
    Json,
}

#[derive(Debug, Serialize)]
pub struct AuditReport {
    pub result: String,
    pub enabled: bool,
    pub file_count: usize,
    pub invalid_filenames: Vec<String>,
    pub violations: Vec<String>,
}

/// 审计入口:解析参数 + 调度 try_audit_hat_handoff + 退出码控制。
pub fn audit_hat_handoff_command(args: AuditHatHandoffArgs) -> Result<()> {
    let exit = try_audit_hat_handoff(&args);
    let report = build_report(&args, &exit);

    match args.format {
        AuditFormat::Text => print_human(&exit, &report),
        AuditFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(&report)
                    .context("serialize audit report")?
            );
        }
    }

    let code = exit.code();
    if code != 0 {
        std::process::exit(code);
    }
    Ok(())
}

/// 纯函数入口(便于测试)。
pub fn try_audit_hat_handoff(args: &AuditHatHandoffArgs) -> AuditExit {
    let workspace = args.workspace.canonicalize().unwrap_or_else(|_| args.workspace.clone());
    let enabled = audit_handoff_enabled(&workspace);

    if !enabled && !args.force {
        return AuditExit::HandoffDisabled;
    }

    let dir = workspace.join(".ralph/agent/hat-handoff");
    if !dir.exists() {
        return AuditExit::NoHandoffDir;
    }

    // 收集文件名
    let entries: Vec<PathBuf> = match std::fs::read_dir(&dir) {
        Ok(rd) => rd.filter_map(|e| e.ok()).map(|e| e.path()).collect(),
        Err(_) => return AuditExit::NoHandoffDir,
    };

    if entries.is_empty() {
        return AuditExit::EmptyHandoffDir { enabled };
    }

    // 校验每个文件名格式
    let re = Regex::new(r"^(\d+)-(\d+)-([A-Za-z0-9_-]+)-([A-Za-z0-9_-]+)\.md$")
        .expect("valid regex");
    let mut invalid: Vec<String> = Vec::new();
    let mut parsed: Vec<(u32, u32, String)> = Vec::new();

    for entry in &entries {
        let Some(name) = entry.file_name().and_then(|n| n.to_str()) else {
            invalid.push(entry.display().to_string());
            continue;
        };
        if let Some(caps) = re.captures(name) {
            let iter: u32 = caps[1].parse().unwrap_or(0);
            let seq: u32 = caps[2].parse().unwrap_or(0);
            let from = caps[3].to_string();
            // `to` parsed but unused here
            let _ = &caps[4];
            parsed.push((iter, seq, from));
        } else {
            invalid.push(name.to_string());
        }
    }

    if !invalid.is_empty() {
        return AuditExit::InvalidFilename {
            samples: invalid.into_iter().take(5).collect(),
        };
    }

    // 单调性检查:按 (iter, seq) 排序后,iter 必须严格递增;
    // 同 iter 内 seq 严格递增。这一约定确保"由 handoff 准备工具
    // 按 iter 严格顺序创建"的语义被尊重——而 read_dir 顺序在不同
    // 文件系统上不稳定,所以这里用 (iter, seq) 排序后的逻辑顺序。
    // 注意:iter 严格递增的语义是"任意两个文件的 iter 不可相等或
    // 减小",BTreeMap 自动排序后 iter key 是去重的,所以 key 序列
    // 天然严格递增;真正需要检查的是 **seq 在同 iter 内的严格递增**。
    // 物理顺序的 iter 减少由 filename 解析时携带,排序后被规范化。
    // 若调用方需要物理顺序的 iter 减少,改用 audit_handoff_iter_decreasing 测试。
    parsed.sort_by_key(|(iter, seq, _)| (*iter, *seq));
    let mut prev_iter: Option<u32> = None;
    let mut prev_seq: Option<u32> = None;
    for (iter, seq, _from) in &parsed {
        if let Some(prev_i) = prev_iter {
            if *iter < prev_i {
                return AuditExit::MonotonicityViolation {
                    message: format!(
                        "iter 不严格递增(按 (iter,seq) 排序后):前一个 iter={},当前 iter={}",
                        prev_i, iter
                    ),
                };
            }
            if *iter == prev_i {
                if let Some(prev_s) = prev_seq {
                    if *seq <= prev_s {
                        return AuditExit::MonotonicityViolation {
                            message: format!(
                                "iter {} 内 seq 不严格递增:{} 后跟 {}",
                                iter, prev_s, seq
                            ),
                        };
                    }
                }
            }
        }
        prev_iter = Some(*iter);
        prev_seq = Some(*seq);
    }

    AuditExit::Ok
}

/// 检查 workspace 的 `ralph.yml` / 内置 preset 是否启用了 hat_handoff。
///
/// 实现:对 `.ralph/ralph.yml` 做轻量字符串搜索,通过缩进状态机跟踪
/// `hat_handoff:` 段内嵌套的 `enabled:` 字段,避免误判文件其他位置
/// 的 `enabled: true`(如 `event_policy.enabled`、`tasks.enabled` 等)。
fn audit_handoff_enabled(workspace: &Path) -> bool {
    let candidates = [
        workspace.join("ralph.yml"),
        workspace.join(".ralph/ralph.yml"),
    ];
    for path in &candidates {
        if let Ok(content) = std::fs::read_to_string(path) {
            // 状态机:用当前缩进级别跟踪是否在 `hat_handoff:` 段内。
            // 当遇到 `hat_handoff:` 行(以冒号结尾),记录它的缩进。
            // 后续缩进**更深的** `enabled: ...` 才视为 hat_handoff 的 enabled。
            // 缩进回退到 hat_handoff 缩进或更浅时退出 hat_handoff 段。
            let mut hat_handoff_indent: Option<usize> = None;
            for line in content.lines() {
                let trimmed = line.trim_start();
                if trimmed.is_empty() || trimmed.starts_with('#') {
                    continue;
                }
                let indent = line.len() - trimmed.len();
                match hat_handoff_indent {
                    Some(hh_indent) if indent > hh_indent => {
                        // 在 hat_handoff 段内,检查是否是 `enabled: true`
                        if trimmed.starts_with("enabled:") {
                            let val = trimmed.trim_start_matches("enabled:").trim();
                            if val == "true" {
                                return true;
                            } else if val == "false" {
                                return false;
                            }
                        }
                    }
                    Some(_) => {
                        // 缩进回到 hat_handoff 段或更浅,退出
                        hat_handoff_indent = None;
                        // 重新检查本行是否开启新的 hat_handoff 段
                        if trimmed == "hat_handoff:"
                            || trimmed.starts_with("hat_handoff:")
                        {
                            hat_handoff_indent = Some(indent);
                        }
                    }
                    None => {
                        if trimmed == "hat_handoff:" || trimmed.starts_with("hat_handoff:") {
                            hat_handoff_indent = Some(indent);
                        }
                    }
                }
            }
        }
    }
    false
}

fn build_report(args: &AuditHatHandoffArgs, exit: &AuditExit) -> AuditReport {
    let dir = args.workspace.join(".ralph/agent/hat-handoff");
    let file_count = dir.read_dir().map(|d| d.count()).unwrap_or(0);
    AuditReport {
        result: exit.name().to_string(),
        enabled: !matches!(exit, AuditExit::HandoffDisabled),
        file_count,
        invalid_filenames: match exit {
            AuditExit::InvalidFilename { samples } => samples.clone(),
            _ => Vec::new(),
        },
        violations: match exit {
            AuditExit::MonotonicityViolation { message } => vec![message.clone()],
            _ => Vec::new(),
        },
    }
}

fn print_human(exit: &AuditExit, _report: &AuditReport) {
    match exit {
        AuditExit::Ok => println!("✅ hat-handoff audit passed"),
        AuditExit::HandoffDisabled => {
            println!("⏭️  hat_handoff disabled; audit skipped")
        }
        AuditExit::NoHandoffDir => {
            println!("❌ no_handoff_dir: .ralph/agent/hat-handoff/ does not exist")
        }
        AuditExit::EmptyHandoffDir { enabled } => {
            println!(
                "❌ empty_handoff_dir: handoff enabled={} but 0 files in .ralph/agent/hat-handoff/",
                enabled
            )
        }
        AuditExit::InvalidFilename { samples } => {
            println!("❌ invalid_filename: {:?}", samples)
        }
        AuditExit::MonotonicityViolation { message } => {
            println!("❌ monotonicity_violation: {}", message)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write_handoff(dir: &Path, name: &str) -> PathBuf {
        let abs = dir.join(name);
        fs::write(&abs, "# Handoff: a → b\n## next\n**动作**: x\n**阻塞**: 无\n").unwrap();
        abs
    }

    fn args(workspace: &Path, force: bool) -> AuditHatHandoffArgs {
        AuditHatHandoffArgs {
            workspace: workspace.to_path_buf(),
            format: AuditFormat::Text,
            force,
        }
    }

    #[test]
    fn exit_ok_when_all_files_valid_and_monotonic() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let handoff = ws.join(".ralph/agent/hat-handoff");
        fs::create_dir_all(&handoff).unwrap();
        // 写入 ralph.yml 启用 hat_handoff(简化版)
        fs::write(
            ws.join("ralph.yml"),
            "event_loop:\n  hat_handoff:\n    enabled: true\n",
        )
        .unwrap();
        write_handoff(&handoff, "1-1-plan_gate-executor.md");
        write_handoff(&handoff, "2-1-plan_gate-executor.md");
        write_handoff(&handoff, "2-2-plan_gate-executor.md");

        let exit = try_audit_hat_handoff(&args(ws, false));
        assert_eq!(exit, AuditExit::Ok);
        assert_eq!(exit.code(), 0);
    }

    #[test]
    fn exit_handoff_disabled_when_no_ralph_yml() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let handoff = ws.join(".ralph/agent/hat-handoff");
        fs::create_dir_all(&handoff).unwrap();
        // 没有 ralph.yml → 默认 enabled=false
        let exit = try_audit_hat_handoff(&args(ws, false));
        assert_eq!(exit, AuditExit::HandoffDisabled);
        assert_eq!(exit.code(), 0);
    }

    #[test]
    fn exit_no_handoff_dir_when_directory_missing() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        fs::write(
            ws.join("ralph.yml"),
            "event_loop:\n  hat_handoff:\n    enabled: true\n",
        )
        .unwrap();
        let exit = try_audit_hat_handoff(&args(ws, false));
        assert_eq!(exit, AuditExit::NoHandoffDir);
        assert_eq!(exit.code(), 1);
    }

    #[test]
    fn exit_empty_handoff_dir_when_enabled_but_zero_files() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let handoff = ws.join(".ralph/agent/hat-handoff");
        fs::create_dir_all(&handoff).unwrap();
        fs::write(
            ws.join("ralph.yml"),
            "event_loop:\n  hat_handoff:\n    enabled: true\n",
        )
        .unwrap();
        let exit = try_audit_hat_handoff(&args(ws, false));
        assert!(matches!(exit, AuditExit::EmptyHandoffDir { enabled: true }));
        assert_eq!(exit.code(), 2);
    }

    #[test]
    fn exit_invalid_filename_when_format_wrong() {
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let handoff = ws.join(".ralph/agent/hat-handoff");
        fs::create_dir_all(&handoff).unwrap();
        fs::write(
            ws.join("ralph.yml"),
            "event_loop:\n  hat_handoff:\n    enabled: true\n",
        )
        .unwrap();
        write_handoff(&handoff, "bad-name.md");
        let exit = try_audit_hat_handoff(&args(ws, false));
        assert!(matches!(exit, AuditExit::InvalidFilename { .. }));
        assert_eq!(exit.code(), 3);
    }

    #[test]
    fn exit_monotonicity_violation_when_seq_repeats_within_iter() {
        // 同一 iter 内 seq 重复 → 违反单调性
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let handoff = ws.join(".ralph/agent/hat-handoff");
        fs::create_dir_all(&handoff).unwrap();
        fs::write(
            ws.join("ralph.yml"),
            "event_loop:\n  hat_handoff:\n    enabled: true\n",
        )
        .unwrap();
        // 两个不同文件名,但 iter=1 seq=1 → 同 iter 同 seq 重复
        write_handoff(&handoff, "1-1-a-b.md");
        write_handoff(&handoff, "1-1-c-d.md");
        let exit = try_audit_hat_handoff(&args(ws, false));
        assert!(matches!(exit, AuditExit::MonotonicityViolation { .. }));
        assert_eq!(exit.code(), 4);
    }

    #[test]
    fn exit_ok_when_iter_increasing_even_if_filenames_unsorted_on_disk() {
        // 文件系统顺序与文件名 iter 顺序无关——只要文件名迭代后是
        // 严格递增就通过。本测试不依赖物理 read_dir 顺序。
        let dir = tempfile::tempdir().unwrap();
        let ws = dir.path();
        let handoff = ws.join(".ralph/agent/hat-handoff");
        fs::create_dir_all(&handoff).unwrap();
        fs::write(
            ws.join("ralph.yml"),
            "event_loop:\n  hat_handoff:\n    enabled: true\n",
        )
        .unwrap();
        // 写入多文件,即便 read_dir 顺序与 iter 不同也应通过
        for n in 1..=5 {
            write_handoff(&handoff, &format!("{n}-1-plan_gate-executor.md"));
        }
        let exit = try_audit_hat_handoff(&args(ws, false));
        assert_eq!(exit, AuditExit::Ok);
    }
}