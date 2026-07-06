//! U3：`allowed_next_for_hat_phase` 纯函数。
//!
//! 给定 hat id + phase id + 内联 phase authority 声明 fixture，返回
//! `Vec<String>`：该 hat 在该 phase 下被 phase authority 白名单允许
//! emit 的 topic 列表（**去重**）。fixture **完全内联**——本模块不读
//! preset 磁盘、不调用 CLI、不触碰 `.ralph/`。
//!
//! 本模块只读借用 `event_loop/phase_authority/whitelist.rs::allows` 已有
//! 的纯函数（不修改它），复用其 `allowed_topics` 字段聚合逻辑。U7+ 才
//! 真正接入运行时路径。
//!
//! 测试约定：所有 U3 测试带 `test_allowed_next_for_hat_phase_*` 前缀，
//! 使 `cargo nextest run -p ralph-core -- allowed_next_for_hat_phase`
//! substring 一次性命中全部 U3 测试。

use crate::event_loop::phase_authority::PhaseAuthorityDeclaration;

/// 给定 hat + phase + 声明 fixture，返回该 hat 在该 phase 下被
/// phase authority 允许 emit 的 topic 列表（去重，保持 stable order）。
///
/// - `phase_id` 在 fixture 中不存在 → 返回空 Vec。
/// - 命中精确 hat 条目 ∪ `*` 通配条目 → 返回并集去重列表。
/// - hat_id 自身就是 `"*"` 时不再合并通配（避免重复）。
pub fn allowed_next_for_hat_phase(
    hat_id: &str,
    phase_id: &str,
    decl: &PhaseAuthorityDeclaration,
) -> Vec<String> {
    let Some(phase) = decl.phases.iter().find(|p| p.id == phase_id) else {
        return Vec::new();
    };

    let mut topics = Vec::new();
    if let Some(hat_topics) = phase.allowed_emits.get(hat_id) {
        topics.extend(hat_topics.iter().cloned());
    }
    if hat_id != "*" {
        if let Some(wildcard_topics) = phase.allowed_emits.get("*") {
            topics.extend(wildcard_topics.iter().cloned());
        }
    }

    // 去重（保持稳定顺序）
    let mut seen = std::collections::HashSet::new();
    topics.retain(|t| seen.insert(t.clone()));
    topics
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::event_loop::phase_authority::config::PhaseDeclConfig;
    use std::collections::BTreeMap;

    /// 内联 fixture 构造辅助：单 phase + per-hat allow list。
    fn fixture_with_unit_loop() -> PhaseAuthorityDeclaration {
        let mut allowed_emits = BTreeMap::new();
        // coordinator 在 unit_loop phase 允许 work.ready / work.done
        allowed_emits.insert(
            "coordinator".to_string(),
            vec!["work.ready".to_string(), "work.done".to_string()],
        );
        // executor 仅允许 work.done
        allowed_emits.insert(
            "executor".to_string(),
            vec!["work.done".to_string()],
        );
        // 通配：所有 hat 都被允许 review.start
        allowed_emits.insert(
            "*".to_string(),
            vec!["review.start".to_string()],
        );

        let phases = vec![PhaseDeclConfig {
            id: "unit_loop".to_string(),
            label: None,
            allowed_emits,
        }];

        PhaseAuthorityDeclaration {
            phases,
            transitions: vec![],
            initial_phase: Some("unit_loop".to_string()),
        }
    }

    /// hat=coordinator, phase=unit_loop 必须含 work.ready 与
    /// review.start（通配），且不含 worker-only topic。
    #[test]
    fn test_allowed_next_for_hat_phase_coordinator_unit_loop() {
        let decl = fixture_with_unit_loop();
        let next = allowed_next_for_hat_phase("coordinator", "unit_loop", &decl);

        assert!(
            next.contains(&"work.ready".to_string()),
            "coordinator must be allowed work.ready in unit_loop, got: {next:?}"
        );
        assert!(
            next.contains(&"work.done".to_string()),
            "coordinator must be allowed work.done in unit_loop, got: {next:?}"
        );
        assert!(
            next.contains(&"review.start".to_string()),
            "coordinator must inherit wildcard review.start in unit_loop, got: {next:?}"
        );

        // coordinator 不应单独得到 *通配* 的 fallback 之外的内容
        // （fixture 中只有 coordinator 自己 + * 通配两条来源）
        assert_eq!(
            next.len(),
            3,
            "coordinator unit_loop must have exactly 3 unique allowed topics, got: {next:?}"
        );
    }

    /// 未知 phase → 空 Vec。
    #[test]
    fn test_allowed_next_for_hat_phase_unknown_phase_empty() {
        let decl = fixture_with_unit_loop();
        let next = allowed_next_for_hat_phase("coordinator", "nope", &decl);

        assert!(
            next.is_empty(),
            "unknown phase must yield empty Vec, got: {next:?}"
        );
    }

    /// hat 在 fixture 中无精确条目但存在 `*` 通配 → 继承通配允许的
    /// topics。
    #[test]
    fn test_allowed_next_for_hat_phase_unknown_hat_inherits_wildcard() {
        let decl = fixture_with_unit_loop();
        let next = allowed_next_for_hat_phase("unknown_hat", "unit_loop", &decl);

        // unknown_hat 不在 fixture.allowed_emits 中，但 fixture 有 "*"
        // 通配条目 review.start——所以它会继承通配条目
        assert!(
            next.contains(&"review.start".to_string()),
            "unknown_hat must inherit wildcard review.start, got: {next:?}"
        );
        assert_eq!(next.len(), 1);
    }

    /// hat=executor（unit_loop）：精确条目 work.done + 通配 review.start。
    #[test]
    fn test_allowed_next_for_hat_phase_executor_unit_loop() {
        let decl = fixture_with_unit_loop();
        let next = allowed_next_for_hat_phase("executor", "unit_loop", &decl);

        assert_eq!(
            next,
            vec!["work.done".to_string(), "review.start".to_string()],
            "executor unit_loop allowed_next must be work.done + review.start"
        );
    }

    /// 全部 hat 都无允许的 phase → 空 Vec。
    #[test]
    fn test_allowed_next_for_hat_phase_empty_phase() {
        let phases = vec![PhaseDeclConfig {
            id: "terminal".to_string(),
            label: None,
            allowed_emits: BTreeMap::new(),
        }];
        let decl = PhaseAuthorityDeclaration {
            phases,
            transitions: vec![],
            initial_phase: Some("terminal".to_string()),
        };
        let next = allowed_next_for_hat_phase("coordinator", "terminal", &decl);
        assert!(next.is_empty());
    }
}