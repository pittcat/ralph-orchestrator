//! 2026-06-18-002 plan U1: 宏观边解析(KTD-2)。
//!
//! "宏观边" = 唯一消费者 topic ∧ 非自环 ∧ 非豁免。
//! plan-gate 双发场景:`work.ready` 必须 handoff(→ executor);
//! `queue.advance` 豁免(自环 → plan-gate → plan-gate,已在
//! `from_hat == to_hat` 排除)。

use crate::config::HatExecutionMode;
use crate::workflow_contract::handoff_index::HandoffIndex;

/// 宏观边判定结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacroEdge {
    /// 是宏观边,emit 必须带 `handoff_path`。
    Required,
    /// 不是宏观边(微观边/豁免/自环/coordinator 模式/disabled)。
    NotRequired,
}

/// 判定一个 emit 事件是否走宏观边(KTD-2)。
///
/// - `enabled`:从 `HatHandoffConfig.enabled` 传入。
/// - `execution_mode`:从 `EventLoopConfig.execution_mode` 传入;
///   非 isolated 一律返回 `NotRequired`。
/// - `index`:`HandoffIndex`,由 `LoopContext` 持有。
/// - `topic`:事件 topic。
/// - `from_hat`:emit hat id(用于自环排除)。
/// - `config_exempt` + `config_explicit_macro`:来自
///   `HatHandoffConfig`,供 `is_exempt` / `is_explicit_macro` 使用。
pub fn requires_handoff(
    enabled: bool,
    execution_mode: HatExecutionMode,
    index: &HandoffIndex,
    topic: &str,
    from_hat: &str,
    config_exempt: impl Fn(&str) -> bool,
    config_explicit_macro: impl Fn(&str) -> bool,
) -> MacroEdge {
    if !enabled {
        return MacroEdge::NotRequired;
    }
    if !matches!(execution_mode, HatExecutionMode::Isolated) {
        return MacroEdge::NotRequired;
    }

    let consumer = index.consumer_of(topic);
    let to_hat = match consumer {
        Some(c) => c,
        None => {
            // 多消费者 / wildcard / 不在 index 内 → 微观边;
            // 但 `macro_topics` 显式列表可强制要求 handoff。
            return if config_explicit_macro(topic) && !config_exempt(topic) {
                MacroEdge::Required
            } else {
                MacroEdge::NotRequired
            };
        }
    };

    // 自环排除(KTD-2)。
    if from_hat == to_hat {
        return MacroEdge::NotRequired;
    }

    // 默认豁免 + 用户豁免。
    if config_exempt(topic) {
        return MacroEdge::NotRequired;
    }

    MacroEdge::Required
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::HatExecutionMode;
    use crate::config::RalphConfig;
    use crate::workflow_contract::handoff_index::HandoffIndex;

    fn two_hat_index() -> HandoffIndex {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  execution_mode: isolated
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready", "queue.advance"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        HandoffIndex::from_config(&config)
    }

    #[test]
    fn disabled_means_never_required() {
        let idx = two_hat_index();
        let r = requires_handoff(
            false,
            HatExecutionMode::Isolated,
            &idx,
            "work.ready",
            "plan-gate",
            |_| false,
            |_| false,
        );
        assert_eq!(r, MacroEdge::NotRequired);
    }

    #[test]
    fn coordinator_mode_never_required() {
        let idx = two_hat_index();
        let r = requires_handoff(
            true,
            HatExecutionMode::Coordinator,
            &idx,
            "work.ready",
            "plan-gate",
            |_| false,
            |_| false,
        );
        assert_eq!(r, MacroEdge::NotRequired);
    }

    #[test]
    fn macro_edge_with_unique_consumer_required() {
        let idx = two_hat_index();
        // work.ready: plan_gate → executor, unique consumer.
        let r = requires_handoff(
            true,
            HatExecutionMode::Isolated,
            &idx,
            "work.ready",
            "plan-gate",
            |_| false,
            |_| false,
        );
        assert_eq!(r, MacroEdge::Required);
    }

    #[test]
    fn self_loop_excluded() {
        let idx = two_hat_index();
        // queue.advance: plan-gate → plan-gate (self loop).
        let r = requires_handoff(
            true,
            HatExecutionMode::Isolated,
            &idx,
            "queue.advance",
            "plan-gate",
            |_| false,
            |_| false,
        );
        assert_eq!(r, MacroEdge::NotRequired);
    }

    #[test]
    fn default_exempt_topics_skipped() {
        let idx = two_hat_index();
        let r = requires_handoff(
            true,
            HatExecutionMode::Isolated,
            &idx,
            "review.dimension.done",
            "review-coordinator",
            |_| false,
            |_| false,
        );
        assert_eq!(r, MacroEdge::NotRequired);
    }

    #[test]
    fn explicit_macro_topics_force_required() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  execution_mode: isolated
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["x"]
  b:
    name: "B"
    triggers: ["x"]
  c:
    name: "C"
    triggers: ["x"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let idx = HandoffIndex::from_config(&config);
        // topic "x" has 2 consumers → no unique consumer,
        // normally NotRequired. With explicit macro_topics,
        // it must be Required (unless exempted).
        let r = requires_handoff(
            true,
            HatExecutionMode::Isolated,
            &idx,
            "x",
            "a",
            |_| false,
            |t| t == "x",
        );
        assert_eq!(r, MacroEdge::Required);
    }

    #[test]
    fn exempt_overrides_explicit_macro() {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  execution_mode: isolated
hats:
  a:
    name: "A"
    triggers: ["work.start"]
    publishes: ["x"]
  b:
    name: "B"
    triggers: ["x"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let idx = HandoffIndex::from_config(&config);
        // "x" has unique consumer (b). Both macro_topics and
        // exempt list contain "x" → exempt wins.
        let r = requires_handoff(
            true,
            HatExecutionMode::Isolated,
            &idx,
            "x",
            "a",
            |t| t == "x",
            |t| t == "x",
        );
        assert_eq!(r, MacroEdge::NotRequired);
    }
}
