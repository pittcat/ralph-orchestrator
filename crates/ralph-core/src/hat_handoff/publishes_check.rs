//! 2026-06-18-002 plan U4 (KTD-15): R15 topic 校验。
//!
//! 从 `## next` 动作行抽取 `` `topic.name` `` / `emit topic.name` 字面量,
//! 对照 downstream hat 的 `publishes` 列表:
//! - 未抽取到 topic → 跳过(纯阅读类动作合法)。
//! - 抽取到 topic 且不在 publishes → `IllegalEmitTopic`。

use regex::Regex;
use std::collections::HashSet;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TopicViolation {
    /// `## next` 动作行引用了不属于下游 hat publishes 的 topic。
    IllegalEmitTopic { topic: String, allowed: Vec<String> },
}

impl std::fmt::Display for TopicViolation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::IllegalEmitTopic { topic, allowed } => {
                write!(f, "next action line references topic `{topic}` but downstream hat publishes only {allowed:?}")
            }
        }
    }
}

impl std::error::Error for TopicViolation {}

fn topic_regex() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // 匹配两种字面量: `topic.name` 和 `emit topic.name`(大小写不敏感)。
        // topic 名称允许: 字母/数字/点/下划线/连字符。
        Regex::new(r"(?i)(?:`([a-z][a-z0-9._-]*)`|\bemit\s+([a-z][a-z0-9._-]*)\b)").unwrap()
    })
}

/// 从 next action 行抽取所有 topic 字面量(deduplicated)。
pub fn extract_topics(action_line: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for cap in topic_regex().captures_iter(action_line) {
        let topic = cap
            .get(1)
            .or_else(|| cap.get(2))
            .map(|m| m.as_str().to_string());
        if let Some(t) = topic {
            if seen.insert(t.clone()) {
                out.push(t);
            }
        }
    }
    out
}

/// 校验 next action 行的 topic 引用是否合法。
///
/// - `action_line`:`## next` 中 `**动作**:` 行的内容(不含前缀)。
/// - `downstream_publishes`:下游 hat 声明的 publishes 列表。
///
/// 返回 `Ok(())` 当抽取到 0 个 topic(纯阅读动作)或所有 topic 都在 publishes 内。
pub fn validate(action_line: &str, downstream_publishes: &[String]) -> Result<(), TopicViolation> {
    let topics = extract_topics(action_line);
    for t in topics {
        if !downstream_publishes.iter().any(|p| p == &t) {
            return Err(TopicViolation::IllegalEmitTopic {
                topic: t,
                allowed: downstream_publishes.to_vec(),
            });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_backtick_topic() {
        let topics = extract_topics("emit `work.ready` after task creation");
        assert_eq!(topics, vec!["work.ready".to_string()]);
    }

    #[test]
    fn extract_bare_emit_topic() {
        let topics = extract_topics("emit work.ready");
        assert_eq!(topics, vec!["work.ready".to_string()]);
    }

    #[test]
    fn extract_multiple() {
        let topics = extract_topics("emit `a.b` then `c.d` then emit e.f");
        assert_eq!(
            topics,
            vec!["a.b".to_string(), "c.d".to_string(), "e.f".to_string()]
        );
    }

    #[test]
    fn no_topics_returns_empty() {
        let topics = extract_topics("read the worktree file and summarize");
        assert!(topics.is_empty());
    }

    #[test]
    fn case_insensitive_extract() {
        let topics = extract_topics("emit `Work.Ready`");
        assert_eq!(topics, vec!["Work.Ready".to_string()]);
    }

    #[test]
    fn executor_cannot_emit_queue_advance() {
        // executor downstream publishes 不含 queue.advance → 拒收。
        let downstream = vec!["work.done".to_string(), "report.done".to_string()];
        let err =
            validate("emit `queue.advance` after summary", &downstream).unwrap_err();
        assert_eq!(
            err,
            TopicViolation::IllegalEmitTopic {
                topic: "queue.advance".to_string(),
                allowed: downstream.clone(),
            }
        );
    }

    #[test]
    fn review_coordinator_can_emit_review_wave_ready() {
        let downstream = vec![
            "review.wave.ready".to_string(),
            "report.done".to_string(),
        ];
        validate("emit `review.wave.ready`", &downstream).unwrap();
    }

    #[test]
    fn pure_read_action_passes() {
        let downstream = vec!["work.done".to_string()];
        validate("read the file and verify", &downstream).unwrap();
    }
}
