//! 2026-06-18-002 plan P1-1: 单一 handoff_path 解析入口(SSOT)。
//!
//! 同时支持两种 payload 形态:
//! - EventParser 默认输出 raw key:value 形态:
//!   `task_id: "..."\nhandoff_path: ".ralph/..."`
//! - CLI `ralph emit --json` 输出标准 JSON 形态:
//!   `{"handoff_path": "...", ...}`
//!
//! gate 与 inject 共享 `extract_handoff_path`,避免 SSOT 漂移
//! (此前 `inject::find_pending_handoff_path` 支持双格式而 gate
//! 阶段的自实现只支持 JSON,导致同一 payload 在 gate 阶段被判
//! 「无 handoff_path」但 inject 阶段能找到 path 的不一致)。
//!
//! 2026-06-18 P0-1 fix:`EventLoop::parse_handoff_path_from_payload`
//! 已删除(原本只支持 JSON,现在 gate 直接调用 `extract_handoff_path`
//! SSOT,消除 SSOT 漂移)。

use ralph_proto::Event;

/// 从 event payload 中抽取 `handoff_path`。
///
/// 规则(顺序敏感):
/// 1. payload 为空 → `None`。
/// 2. JSON 解析成功 → 取顶层 `handoff_path` 字段;若字段不是字符串
///    (如嵌套对象/数组/数字)→ 视为不存在(`None`)。
/// 3. JSON 解析失败 → fallback 到 raw 行扫描,匹配以下任一前缀:
///    - `handoff_path:`
///    - `"handoff_path":`
///    - `'handoff_path':`
///    提取后 trim 逗号、引号;空值视为不存在。
/// 4. 全部失败 → `None`。
///
/// 这是 gate 与 inject 共享的 SSOT;不要在外部重新实现。
pub fn extract_handoff_path(payload: &str) -> Option<String> {
    if payload.is_empty() {
        return None;
    }

    // 1) JSON 优先:解析成功即按 JSON 取值(不再 fallback 到 raw,
    //    因为如果 payload 是合法 JSON,我们认为它就是 JSON,
    //    不应再退化到行扫描)。
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(payload) {
        return value
            .get("handoff_path")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string());
    }

    // 2) Fallback 到 raw key:value 行扫描(EventParser 默认输出)。
    for line in payload.lines() {
        let line = line.trim();
        if let Some(rest) = line
            .strip_prefix("handoff_path:")
            .or_else(|| line.strip_prefix("\"handoff_path\":"))
            .or_else(|| line.strip_prefix("'handoff_path':"))
        {
            let value = rest.trim().trim_matches(',').trim();
            let value = value.trim_matches('"').trim_matches('\'');
            if !value.is_empty() {
                return Some(value.to_string());
            }
        }
    }

    None
}

/// 从 hat pending events 中抽取首个匹配事件的 `handoff_path`。
///
/// 语义同原 `inject::find_pending_handoff_path`,但内部走
/// `extract_handoff_path` SSOT。返回首个 payload 含 `handoff_path`
/// 字段的事件;全部未找到 → `None`。
pub fn find_in_pending(pending: &[Event]) -> Option<String> {
    pending
        .iter()
        .find_map(|ev| extract_handoff_path(&ev.payload))
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- extract_handoff_path tests ---

    #[test]
    fn extract_json_form() {
        let payload = r#"{"handoff_path":".ralph/agent/hat-handoff/3-2-a-b.md"}"#;
        assert_eq!(
            extract_handoff_path(payload).as_deref(),
            Some(".ralph/agent/hat-handoff/3-2-a-b.md")
        );
    }

    #[test]
    fn extract_json_with_extra_fields() {
        let payload =
            r#"{"task_id":"42","handoff_path":"foo.md","from":"a","to":"b"}"#;
        assert_eq!(extract_handoff_path(payload).as_deref(), Some("foo.md"));
    }

    #[test]
    fn extract_raw_key_value_form() {
        let payload = "task_id: \"42\"\nhandoff_path: \".ralph/foo.md\"\nfrom: a";
        assert_eq!(
            extract_handoff_path(payload).as_deref(),
            Some(".ralph/foo.md")
        );
    }

    #[test]
    fn extract_raw_quoted_key_form() {
        let payload = "\"handoff_path\": \"bar.md\"";
        assert_eq!(extract_handoff_path(payload).as_deref(), Some("bar.md"));
    }

    #[test]
    fn extract_raw_single_quoted_key_form() {
        let payload = "'handoff_path': 'baz.md'";
        assert_eq!(extract_handoff_path(payload).as_deref(), Some("baz.md"));
    }

    #[test]
    fn extract_raw_with_trailing_comma() {
        let payload = "handoff_path: \"x.md\",\ntask_id: \"1\"";
        assert_eq!(extract_handoff_path(payload).as_deref(), Some("x.md"));
    }

    #[test]
    fn extract_json_nested_object_value_returns_none() {
        // 合法 JSON,但 handoff_path 是对象而非字符串 → 视为不存在。
        let payload = r#"{"handoff_path":{"nested":"oops"}}"#;
        assert_eq!(extract_handoff_path(payload), None);
    }

    #[test]
    fn extract_json_array_value_returns_none() {
        let payload = r#"{"handoff_path":["a","b"]}"#;
        assert_eq!(extract_handoff_path(payload), None);
    }

    #[test]
    fn extract_json_null_value_returns_none() {
        let payload = r#"{"handoff_path":null}"#;
        assert_eq!(extract_handoff_path(payload), None);
    }

    #[test]
    fn extract_json_missing_field_returns_none() {
        let payload = r#"{"foo":"bar"}"#;
        assert_eq!(extract_handoff_path(payload), None);
    }

    #[test]
    fn extract_empty_payload_returns_none() {
        assert_eq!(extract_handoff_path(""), None);
    }

    #[test]
    fn extract_raw_no_match_returns_none() {
        let payload = "task_id: \"1\"\nfrom: a\nto: b";
        assert_eq!(extract_handoff_path(payload), None);
    }

    #[test]
    fn extract_raw_empty_value_returns_none() {
        // 空字符串值 → 视为不存在。
        let payload = "handoff_path: \"\"";
        assert_eq!(extract_handoff_path(payload), None);
    }

    #[test]
    fn extract_raw_malformed_json_falls_through() {
        // 半截 JSON(EventParser 中常见:前缀合法但末尾缺 brace)。
        // 必须能 fallback 到 raw 解析。
        let payload = r#"task_id: "1"
handoff_path: "ok.md"
"#;
        assert_eq!(extract_handoff_path(payload).as_deref(), Some("ok.md"));
    }

    // --- find_in_pending tests ---

    #[test]
    fn find_in_pending_json_form() {
        let pending = vec![Event::new(
            "work.ready",
            r#"{"handoff_path":".ralph/agent/hat-handoff/3-2-a-b.md"}"#,
        )];
        assert_eq!(
            find_in_pending(&pending).as_deref(),
            Some(".ralph/agent/hat-handoff/3-2-a-b.md")
        );
    }

    #[test]
    fn find_in_pending_raw_form() {
        let pending = vec![Event::new(
            "work.ready",
            "handoff_path: \".ralph/x.md\"",
        )];
        assert_eq!(find_in_pending(&pending).as_deref(), Some(".ralph/x.md"));
    }

    #[test]
    fn find_in_pending_returns_none_when_absent() {
        let pending = vec![Event::new("work.ready", r#"{"foo":"bar"}"#)];
        assert!(find_in_pending(&pending).is_none());
    }

    #[test]
    fn find_in_pending_returns_none_for_empty_payload() {
        let pending = vec![Event::new("work.ready", "")];
        assert!(find_in_pending(&pending).is_none());
    }

    #[test]
    fn find_in_pending_returns_first_match() {
        // 第二个事件才有 handoff_path → 应返回第二个。
        let pending = vec![
            Event::new("a", r#"{"foo":"bar"}"#),
            Event::new("b", r#"{"handoff_path":"second.md"}"#),
            Event::new("c", r#"{"handoff_path":"third.md"}"#),
        ];
        assert_eq!(find_in_pending(&pending).as_deref(), Some("second.md"));
    }

    #[test]
    fn find_in_pending_empty_slice_returns_none() {
        let pending: Vec<Event> = vec![];
        assert!(find_in_pending(&pending).is_none());
    }
}