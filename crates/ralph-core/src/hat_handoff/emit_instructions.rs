//! 2026-06-18-002 plan: 自动生成上游 hat 的 handoff 发送指令。
//!
//! 避免在每个 hat 的 preset instructions 里重复粘贴几乎相同的
//! `ralph tools handoff prepare` 命令与五段式 markdown 要求。
//! 运行时根据 hat 的 `publishes` 列表和 `HandoffIndex` 拓扑推导
//! 哪些 topic 是宏观边，动态生成一段指令 prepend 到 prompt。

use ralph_proto::Hat;

use crate::config::HatExecutionMode;
use crate::hat_handoff::{HatHandoffConfig, macro_edges};
use crate::workflow_contract::HandoffIndex;

/// 为指定 hat 生成 handoff 发送指令。
///
/// 仅当 `hat_handoff.enabled == true` 且 `execution_mode == isolated`
/// 时才会生成；其余情况返回 `None`。
///
/// 生成的块包含：
/// - 该 hat 所有宏观边 topic 列表
/// - 每条边对应的 `ralph tools handoff prepare` 命令
/// - 五段式 markdown 填充要求
/// - `## next` 块契约
/// - 拒收修复指引
pub fn build_emit_instructions(
    hat: &Hat,
    config: &HatHandoffConfig,
    execution_mode: &HatExecutionMode,
    index: &HandoffIndex,
) -> Option<String> {
    if !config.enabled {
        return None;
    }
    if !matches!(execution_mode, HatExecutionMode::Isolated) {
        return None;
    }

    let mut edges: Vec<(String, String)> = Vec::new();
    for topic in &hat.publishes {
        let topic_str = topic.as_str();
        let is_macro = matches!(
            macro_edges::requires_handoff(
                config.enabled,
                execution_mode,
                index,
                topic_str,
                hat.id.as_str(),
                |t| config.is_exempt(t),
                |t| config.is_explicit_macro(t),
            ),
            macro_edges::MacroEdge::Required
        );
        if !is_macro {
            continue;
        }
        if let Some(consumer) = index.consumer_of(topic_str) {
            edges.push((topic_str.to_string(), consumer.to_string()));
        }
    }

    if edges.is_empty() {
        return None;
    }

    let mut lines = Vec::new();
    lines.push("## HAT HANDOFF EMIT REQUIREMENTS".to_string());
    lines.push(String::new());
    lines.push(
        "The following topics you publish are macro edges and MUST include `handoff_path` in their payload:"
            .to_string(),
    );
    lines.push(String::new());

    for (topic, consumer) in &edges {
        lines.push(format!(
            "- `{topic}` (→ {consumer}): `ralph tools handoff prepare --from {from} --to {to} --topic {topic}`",
            from = hat.id.as_str(),
            to = consumer,
        ));
    }

    lines.push(String::new());
    lines.push(
        "For each macro edge: run the command above, fill the returned `handoff_path` as a 5-section markdown (`## context / ## changed / ## verify / ## next / ## notes`), ensure `## next` contains `**动作**: ...` and `**阻塞**: ...`, then emit with `handoff_path: <returned-path>`."
            .to_string(),
    );
    lines.push(
        "If the gate rejects, fix the issue indicated by `task.resume(reason_code=hat_handoff_*)`."
            .to_string(),
    );

    Some(lines.join("\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RalphConfig;
    use ralph_proto::{Hat, Topic};

    fn isolated_config_with_hats() -> (RalphConfig, HatHandoffConfig) {
        let yaml = r#"
tasks:
  enabled: false
event_loop:
  starting_event: "work.start"
  completion_promise: "LOOP_COMPLETE"
  execution_mode: isolated
  hat_handoff:
    enabled: true
hats:
  plan_gate:
    name: "PlanGate"
    triggers: ["work.start"]
    publishes: ["work.ready", "queue.advance"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
  review-coordinator:
    name: "ReviewCoordinator"
    triggers: ["work.done"]
    publishes: ["review.passed"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        let handoff_config = config.event_loop.hat_handoff.clone();
        (config, handoff_config)
    }

    fn hat(id: &str, publishes: &[&str]) -> Hat {
        Hat {
            id: ralph_proto::HatId::new(id),
            name: id.to_string(),
            description: String::new(),
            subscriptions: Vec::new(),
            publishes: publishes.iter().map(|t| Topic::new(*t)).collect(),
            instructions: String::new(),
        }
    }

    #[test]
    fn disabled_returns_none() {
        let (config, _) = isolated_config_with_hats();
        let mut handoff_config = config.event_loop.hat_handoff.clone();
        handoff_config.enabled = false;
        let index = HandoffIndex::from_config(&config);
        let h = hat("plan-gate", &["work.ready"]);
        assert!(build_emit_instructions(&h, &handoff_config, &config.event_loop.execution_mode, &index).is_none());
    }

    #[test]
    fn coordinator_mode_returns_none() {
        let (config, handoff_config) = isolated_config_with_hats();
        let mut coord_config = config.clone();
        coord_config.event_loop.execution_mode = HatExecutionMode::Coordinator;
        let index = HandoffIndex::from_config(&coord_config);
        let h = hat("plan-gate", &["work.ready"]);
        assert!(build_emit_instructions(&h, &handoff_config, &coord_config.event_loop.execution_mode, &index).is_none());
    }

    #[test]
    fn plan_gate_lists_macro_edges_and_skips_self_loop() {
        let (config, handoff_config) = isolated_config_with_hats();
        let index = HandoffIndex::from_config(&config);
        let h = hat("plan-gate", &["work.ready", "queue.advance"]);
        let text = build_emit_instructions(&h, &handoff_config, &config.event_loop.execution_mode, &index).unwrap();
        assert!(text.contains("## HAT HANDOFF EMIT REQUIREMENTS"));
        assert!(text.contains("`work.ready` (→ executor)"));
        assert!(text.contains("ralph tools handoff prepare --from plan-gate --to executor --topic work.ready"));
        assert!(!text.contains("queue.advance"));
    }

    #[test]
    fn executor_lists_work_done() {
        let (config, handoff_config) = isolated_config_with_hats();
        let index = HandoffIndex::from_config(&config);
        let h = hat("executor", &["work.done"]);
        let text = build_emit_instructions(&h, &handoff_config, &config.event_loop.execution_mode, &index).unwrap();
        assert!(text.contains("`work.done` (→ review-coordinator)"));
        assert!(text.contains("ralph tools handoff prepare --from executor --to review-coordinator --topic work.done"));
    }

    #[test]
    fn no_macro_edges_returns_none() {
        let (config, handoff_config) = isolated_config_with_hats();
        let index = HandoffIndex::from_config(&config);
        // dimension-reviewer 只发微观边（默认豁免），但这里用自定义 hat 演示无宏观边。
        let h = hat("observer", &["debug.log"]);
        assert!(build_emit_instructions(&h, &handoff_config, &config.event_loop.execution_mode, &index).is_none());
    }

    #[test]
    fn includes_generic_five_section_contract() {
        let (config, handoff_config) = isolated_config_with_hats();
        let index = HandoffIndex::from_config(&config);
        let h = hat("plan-gate", &["work.ready"]);
        let text = build_emit_instructions(&h, &handoff_config, &config.event_loop.execution_mode, &index).unwrap();
        assert!(text.contains("## context / ## changed / ## verify / ## next / ## notes"));
        assert!(text.contains("**动作**: ..."));
        assert!(text.contains("**阻塞**: ..."));
        assert!(text.contains("task.resume(reason_code=hat_handoff_*)"));
    }
}
