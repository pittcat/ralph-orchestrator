//! U2：`map_policy_report_to_errors` 纯函数。
//!
//! 把 policy report 里按索引配对的「reason codes + suggestions」扁平
//! 列表拼装成 `Vec<EmitError>`。**纯函数**：不读盘、不触碰 CLI、不
//! 触碰 preset JSONL。
//!
//! 接线由 U7（policy-check 拒收 → EmitResult）完成；本模块不依赖 CLI。
//!
//! 测试约定：所有 U2 测试带 `test_map_policy_report_to_errors_*`
//! 前缀，使 `cargo nextest run -p ralph-core -- map_policy_report_to_errors`
//! substring 一次性命中全部 6 个 U2 测试。

use crate::emit_result::EmitError;

/// 将 policy report 的扁平 lists 按 index 拼装为 `EmitError` 序列。
///
/// - 两 slice 长度可不等；缺失的 suggestion 视为空字符串（不填充
///   `suggested_command`）。不会静默丢弃多余的 `reason_codes`。
/// - 空 / 不匹配的 suggestion 字符串映射为 `None`（U1 的
///   `skip_serializing_if` 保证 JSON 输出里整键被省略）。
pub fn map_policy_report_to_errors(
    reason_codes: &[String],
    suggestions: &[String],
) -> Vec<EmitError> {
    reason_codes
        .iter()
        .enumerate()
        .map(|(idx, code)| {
            let suggestion = suggestions.get(idx).map(String::as_str).unwrap_or("");
            let suggested_command = if suggestion.is_empty() {
                None
            } else {
                Some(suggestion.to_string())
            };
            EmitError {
                code: code.clone(),
                message: code.clone(),
                field: None,
                suggested_command,
                ..EmitError::default()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 空 reason_codes + 空 suggestions → 空 Vec。
    #[test]
    fn test_map_policy_report_to_errors_empty_yields_empty_errors() {
        let errors = map_policy_report_to_errors(&[] as &[String], &[] as &[String]);
        assert!(
            errors.is_empty(),
            "empty inputs must yield empty Vec<EmitError>, got: {errors:?}"
        );
    }

    /// 索引 0 对 0：code 与 suggested_command 必须在同一 index 上配对。
    #[test]
    fn test_map_policy_report_to_errors_pairs_code_and_suggestion_by_index() {
        let codes = vec!["missing_task_id".to_string()];
        let suggestions = vec!["ralph tools task list".to_string()];
        let errors = map_policy_report_to_errors(&codes, &suggestions);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "missing_task_id");
        assert_eq!(errors[0].message, "missing_task_id");
        assert_eq!(
            errors[0].suggested_command.as_deref(),
            Some("ralph tools task list")
        );
        // `field` 未在本函数中显式填充，保持 None（U1 已保证 None → 省略键）
        assert!(errors[0].field.is_none());
    }

    /// suggestion 是空字符串 → 该字段视为「无建议」，suggested_command
    /// 必须为 None。
    #[test]
    fn test_map_policy_report_to_errors_empty_suggestion_omits_field() {
        let codes = vec!["missing_task_id".to_string()];
        let suggestions = vec!["".to_string()];
        let errors = map_policy_report_to_errors(&codes, &suggestions);
        assert_eq!(errors.len(), 1);
        assert_eq!(errors[0].code, "missing_task_id");
        assert!(
            errors[0].suggested_command.is_none(),
            "empty suggestion must serialize as None (got {:?})",
            errors[0].suggested_command
        );
    }

    /// 多条 code + 等长 suggestions：按 index 一对一拼装。
    #[test]
    fn test_map_policy_report_to_errors_multiple_pairs_align_by_index() {
        let codes = vec![
            "missing_task_id".to_string(),
            "invalid_phase".to_string(),
            "coordinator_hats_mismatch".to_string(),
        ];
        let suggestions = vec![
            "ralph tools task list".to_string(),
            "".to_string(), // 无建议
            "ralph preset check".to_string(),
        ];
        let errors = map_policy_report_to_errors(&codes, &suggestions);
        assert_eq!(errors.len(), 3);

        assert_eq!(errors[0].code, "missing_task_id");
        assert_eq!(
            errors[0].suggested_command.as_deref(),
            Some("ralph tools task list")
        );

        assert_eq!(errors[1].code, "invalid_phase");
        assert!(
            errors[1].suggested_command.is_none(),
            "index 1 has empty suggestion, expected None"
        );

        assert_eq!(errors[2].code, "coordinator_hats_mismatch");
        assert_eq!(
            errors[2].suggested_command.as_deref(),
            Some("ralph preset check")
        );
    }

    /// 序列化 roundtrip：函数输出可直接喂给 `serde_json::to_value`，
    /// 得到的 JSON 保持 U1 约定的形状（code/message 在场，空 suggestion
    /// 缺 suggested_command）。
    #[test]
    fn test_map_policy_report_to_errors_output_serializes_per_u1_contract() {
        let codes = vec![
            "missing_task_id".to_string(),
            "no_suggestion_code".to_string(),
        ];
        let suggestions = vec!["ralph tools task list".to_string(), "".to_string()];
        let errors = map_policy_report_to_errors(&codes, &suggestions);

        let json: Vec<serde_json::Value> = errors
            .iter()
            .map(|e| serde_json::to_value(e).expect("EmitError must serialize"))
            .collect();
        assert_eq!(json.len(), 2);

        let e0 = json[0].as_object().expect("error must be object");
        assert_eq!(
            e0.get("code"),
            Some(&serde_json::Value::String("missing_task_id".into()))
        );
        assert_eq!(
            e0.get("suggested_command"),
            Some(&serde_json::Value::String("ralph tools task list".into()))
        );

        let e1 = json[1].as_object().expect("error must be object");
        assert_eq!(
            e1.get("code"),
            Some(&serde_json::Value::String("no_suggestion_code".into()))
        );
        assert!(
            e1.get("suggested_command").is_none(),
            "empty suggestion must be omitted in JSON, got: {:?}",
            e1.get("suggested_command")
        );
    }

    /// message 字段必须为每条 error 填充（U1 contract：`message` 不可
    /// 空）。用非常规 code 防止「message == code」巧合命中。
    #[test]
    fn test_map_policy_report_to_errors_message_filled_for_every_code() {
        let codes = vec!["orphan_channel_validation".to_string()];
        let suggestions = vec!["".to_string()];
        let errors = map_policy_report_to_errors(&codes, &suggestions);
        assert_eq!(errors.len(), 1);
        assert!(
            !errors[0].message.is_empty(),
            "message must be non-empty for every EmitError (code = {})",
            errors[0].code
        );
    }

    /// suggestions 短于 reason_codes 时仍输出全部 errors（不静默截断）。
    #[test]
    fn test_map_policy_report_to_errors_extra_codes_without_suggestions() {
        let codes = vec![
            "missing_task_id".to_string(),
            "orphan_channel_validation".to_string(),
        ];
        let suggestions = vec!["ralph tools task list".to_string()];
        let errors = map_policy_report_to_errors(&codes, &suggestions);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].code, "missing_task_id");
        assert_eq!(
            errors[0].suggested_command.as_deref(),
            Some("ralph tools task list")
        );
        assert_eq!(errors[1].code, "orphan_channel_validation");
        assert!(errors[1].suggested_command.is_none());
    }
}
