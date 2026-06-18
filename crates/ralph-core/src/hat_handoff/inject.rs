//! 2026-06-18-002 plan U6: `## HAT HANDOFF` 注入块(KTD-16 fail-closed)。
//!
//! 实际注入逻辑由 `crate::event_loop::EventLoop::prepend_hat_handoff_from_pending`
//! 实现(需要访问私有的 `bus` / `config`)。本模块仅暴露:
//! - 纯函数 `format_block` / `truncate_preserving_next`(测试覆盖)
//! - `build_block`(供 caller 在简单场景下使用)
//!
//! 注:`find_pending_handoff_path` 已迁移到 `payload::find_in_pending`
//! (2026-06-18 P1-1 单一 SSOT);请改用
//! `crate::hat_handoff::payload::find_in_pending`。

use std::path::Path;

use crate::hat_handoff::{HatHandoffConfig, allocator::resolve_jailed};

#[cfg(test)]
use crate::hat_handoff::DEFAULT_HAT_HANDOFF_MAX_BYTES;

/// 构造 HAT HANDOFF 块(测试 + 简单 caller 共用)。
pub fn build_block(
    workspace_root: &Path,
    config: &HatHandoffConfig,
    pending: Option<&str>,
) -> Option<String> {
    if !config.enabled {
        return None;
    }
    let handoff_path = pending?;
    let abs = resolve_jailed(workspace_root, handoff_path).ok()?;
    let content = std::fs::read_to_string(&abs).ok()?;
    Some(format_block(handoff_path, &content, config.max_bytes))
}

/// 公共 helper:给定 path + 内容 + max_bytes,输出 markdown 块。
pub fn format_block(handoff_path: &str, content: &str, max_bytes: usize) -> String {
    let body = truncate_preserving_next(content, max_bytes);
    format!(
        "## HAT HANDOFF\n\
         handoff_path: `{handoff_path}`\n\
         from → to: see filename `{basename}`\n\
         \n\
         {body}\n",
        basename = std::path::Path::new(handoff_path)
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(handoff_path),
    )
}

/// 截断 content 到 `max_bytes`,但**完整保留 `## next` 段**(KTD-7)。
pub fn truncate_preserving_next(content: &str, max_bytes: usize) -> String {
    if content.len() <= max_bytes {
        return content.to_string();
    }
    let next_idx = content.find("## next");
    let next_block = match next_idx {
        Some(idx) => {
            let after = &content[idx..];
            let next_end = after[6..]
                .find("\n## ")
                .map(|p| p + 6)
                .unwrap_or(after.len());
            after[..next_end].to_string()
        }
        None => String::new(),
    };
    let budget = max_bytes.saturating_sub(next_block.len()).saturating_sub(64);
    let head = if budget == 0 {
        String::new()
    } else if content.len() <= budget {
        content.to_string()
    } else {
        let cut = content[..budget].rfind('\n').unwrap_or(budget);
        let mut s = content[..cut].to_string();
        s.push_str("\n…(truncated)\n");
        s
    };
    if next_block.is_empty() {
        head
    } else {
        format!("{head}\n{next_block}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> &'static str {
        "# Handoff: plan_gate → executor\n\
         ## context\n无\n\n\
         ## changed\n无\n\n\
         ## verify\n未验证\n\n\
         ## next\n\
         **动作**: emit work.done after task completion\n\
         **阻塞**: 无\n\n\
         ## notes\n无\n"
    }

    #[test]
    fn short_content_passthrough() {
        let s = truncate_preserving_next(sample(), DEFAULT_HAT_HANDOFF_MAX_BYTES);
        assert!(s.contains("## next"));
        assert!(s.contains("emit work.done"));
    }

    #[test]
    fn long_content_keeps_next_intact() {
        let mut long = String::from(sample());
        for _ in 0..50 {
            long.insert_str(
                long.find("## changed").unwrap() + "## changed\n".len(),
                "filler line with some words\n",
            );
        }
        let truncated = truncate_preserving_next(&long, 200);
        assert!(truncated.contains("## next"));
        assert!(truncated.contains("**动作**:"));
        assert!(truncated.contains("**阻塞**:"));
        assert!(truncated.contains("…(truncated)"));
    }

    #[test]
    fn missing_next_still_returns_something() {
        let no_next = "# Handoff: a → b\n## context\nx\n";
        let truncated = truncate_preserving_next(no_next, 50);
        assert!(truncated.starts_with("# Handoff"));
    }

    #[test]
    fn format_block_includes_path_metadata() {
        let block = format_block(
            ".ralph/agent/hat-handoff/3-2-plan_gate-executor.md",
            sample(),
            DEFAULT_HAT_HANDOFF_MAX_BYTES,
        );
        assert!(block.contains("## HAT HANDOFF"));
        assert!(block.contains("handoff_path:"));
        assert!(block.contains("3-2-plan_gate-executor.md"));
        assert!(block.contains("## next"));
    }

    #[test]
    fn build_block_disabled_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let cfg = HatHandoffConfig::default();
        assert!(build_block(dir.path(), &cfg, Some("anything.md")).is_none());
    }

    #[test]
    fn build_block_missing_file_returns_none() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        assert!(build_block(dir.path(), &cfg, Some(".ralph/agent/hat-handoff/none.md")).is_none());
    }

    #[test]
    fn build_block_happy_path() {
        let dir = tempfile::tempdir().unwrap();
        let path = ".ralph/agent/hat-handoff/3-2-plan_gate-executor.md";
        let abs = dir.path().join(path);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, sample()).unwrap();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        let block = build_block(dir.path(), &cfg, Some(path)).unwrap();
        assert!(block.contains("## HAT HANDOFF"));
        assert!(block.contains("**动作**"));
    }

    // 注:`find_pending_handoff_path` 的测试已迁移到
    // `hat_handoff::payload::tests`(2026-06-18 P1-1)。
}
