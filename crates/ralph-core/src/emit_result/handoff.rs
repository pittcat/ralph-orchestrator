//! U4：`handoff_from_fixture_input` 纯函数。
//!
//! 把 phase authority 提供的「next hat」描述（preset fixture 形式
//! 内联）映射为 U1 的 `EmitHandoff`。输入是本模块定义的 plain struct
//! `EmitHandoffInput`（不读 preset 磁盘、不读 `.ralph/`、不调 CLI）。
//!
//! 接线由 U7+ 完成（policy-check 拒收 → EmitResult）；本模块只做
//! 「输入 → Option<EmitHandoff>」的纯转换。
//!
//! 测试约定：所有 U4 测试带 `test_handoff_from_fixture_input_*` 前缀，
//! 使 `cargo nextest run -p ralph-core -- handoff_from_fixture`
//! substring 一次性命中全部 U4 测试。

use crate::emit_result::EmitHandoff;

/// Phase authority fixture 输入（plain struct）。
///
/// 描述「当前 hat 在某 phase 完成后想交接给下游 hat」的纯数据视图。
/// 全部字段 `Some(_)` 时才产生 `Some(EmitHandoff)`；任一字段为
/// `None` 时视为「不需要 handoff」，返回 `None`。
#[derive(Debug, Clone, Default)]
pub struct EmitHandoffInput {
    /// 指向当前 hat 的稳定 id（fixture 提供）。
    pub from_hat: Option<String>,
    /// 接收交接的下游 hat 的稳定 id（fixture 提供）。
    pub to_hat: Option<String>,
    /// 交接原因短语（fixture 提供，应匹配 preset `handoff_reasons`）。
    pub reason: Option<String>,
}

/// 把 phase authority fixture 输入转换为 `EmitHandoff`。
///
/// 返回 `None` 的条件：
/// - 任一字段为 `None`（fixture 未声明完整 handoff 三元组）；
/// - 任一字段为空字符串（视为缺失）。
///
/// 返回 `Some(EmitHandoff)` 的条件：
/// - 三个字段全部为非空字符串。
pub fn handoff_from_fixture_input(input: &EmitHandoffInput) -> Option<EmitHandoff> {
    let from_hat = input.from_hat.as_deref().filter(|s| !s.is_empty())?;
    let to_hat = input.to_hat.as_deref().filter(|s| !s.is_empty())?;
    let reason = input.reason.as_deref().filter(|s| !s.is_empty())?;

    Some(EmitHandoff {
        from_hat: from_hat.to_string(),
        to_hat: to_hat.to_string(),
        reason: reason.to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::Value;

    /// 全字段 Some → 返回 Some(EmitHandoff)，JSON 含 from_hat /
    /// to_hat / reason 三个键。
    #[test]
    fn test_handoff_from_fixture_input_all_fields_present() {
        let input = EmitHandoffInput {
            from_hat: Some("executor".to_string()),
            to_hat: Some("reviewer".to_string()),
            reason: Some("phase_complete".to_string()),
        };
        let handoff = handoff_from_fixture_input(&input);
        assert!(
            handoff.is_some(),
            "all-Some input must produce Some(EmitHandoff), got None"
        );
        let h = handoff.unwrap();

        let json: Value = serde_json::to_value(&h).expect("EmitHandoff must serialize");
        let obj = json.as_object().expect("EmitHandoff JSON must be object");
        assert_eq!(obj.get("from_hat"), Some(&Value::String("executor".into())));
        assert_eq!(obj.get("to_hat"), Some(&Value::String("reviewer".into())));
        assert_eq!(
            obj.get("reason"),
            Some(&Value::String("phase_complete".into()))
        );
    }

    /// 全 None → 返回 None（fixture 未声明完整 handoff）。
    #[test]
    fn test_handoff_from_fixture_input_all_none_yields_skip() {
        let input = EmitHandoffInput::default();
        let handoff = handoff_from_fixture_input(&input);
        assert!(
            handoff.is_none(),
            "all-None input must produce None (skip), got: {handoff:?}"
        );
    }

    /// 缺一个字段 → 返回 None。
    #[test]
    fn test_handoff_from_fixture_input_missing_one_field_yields_skip() {
        let input = EmitHandoffInput {
            from_hat: Some("executor".to_string()),
            to_hat: Some("reviewer".to_string()),
            reason: None, // 缺 reason
        };
        let handoff = handoff_from_fixture_input(&input);
        assert!(
            handoff.is_none(),
            "missing one field must yield None, got: {handoff:?}"
        );
    }

    /// 空字符串字段视为缺失 → 返回 None。
    #[test]
    fn test_handoff_from_fixture_input_empty_string_yields_skip() {
        let input = EmitHandoffInput {
            from_hat: Some("executor".to_string()),
            to_hat: Some(String::new()), // 空字符串
            reason: Some("phase_complete".to_string()),
        };
        let handoff = handoff_from_fixture_input(&input);
        assert!(
            handoff.is_none(),
            "empty-string field must be treated as missing, got: {handoff:?}"
        );
    }

    /// 输入 field 顺序无关：from_hat + to_hat + reason 全部出现即产生 handoff。
    #[test]
    fn test_handoff_from_fixture_input_field_order_independent() {
        let input = EmitHandoffInput {
            from_hat: Some("reviewer".to_string()),
            to_hat: Some("coordinator".to_string()),
            reason: Some("review_passed".to_string()),
        };
        let handoff = handoff_from_fixture_input(&input).expect("must produce handoff");
        assert_eq!(handoff.from_hat, "reviewer");
        assert_eq!(handoff.to_hat, "coordinator");
        assert_eq!(handoff.reason, "review_passed");
    }
}
