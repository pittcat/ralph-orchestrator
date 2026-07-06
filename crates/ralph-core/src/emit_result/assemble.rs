//! U5：`EmitResult::assemble` 纯函数。
//!
//! 把 policy-check / apply 各阶段的离散信号（ok / recorded / topic /
//! phase / allowed_next / activate_next / errors / handoff）合并为
//! 一个 `EmitResult` 实例。是 `ralph emit` 响应 JSON 的唯一组装入口。
//!
//! 关键规则：
//! - `ok == false` 时强制 `recorded == false`（policy 拒收 → 一定没
//!   写盘）。调用方传 `recorded=true` 也被覆盖。
//! - `ok == true` 时 `recorded` 字段由调用方指定：policy-check 阶段
//!   传 `false`，apply 成功阶段传 `true`。
//! - 拒收时（`ok==false`）会自动清空 `allowed_next` / `activate_next`
//!   / `handoff`（这些字段在 policy 拒收场景无意义）。
//!
//! 不读磁盘、不调 CLI、不触碰 preset；纯函数。U7+ 真正接线 policy
//! report → assemble。
//!
//! 测试约定：所有 U5 测试带 `test_assemble_*` 前缀，使
//! `cargo nextest run -p ralph-core -- emit_result_assemble`
//! substring 一次性命中全部 U5 测试。

use crate::emit_result::{EMIT_RESULT_SCHEMA_VERSION, EmitResult, HandoffEnvelopeSummary};

/// policy-check / apply 阶段的离散信号输入。
#[derive(Debug, Clone, Default)]
pub struct EmitResultParts {
    /// 业务结果。`true` = 通过 / 成功；`false` = 拒收 / 校验失败。
    pub ok: bool,
    /// 是否真实写盘（apply 成功阶段为 `true`；policy-check 阶段为 `false`）。
    /// 当 `ok == false` 时此字段会被强制为 `false`（无副作用覆盖）。
    pub recorded: bool,
    /// 业务 topic（拒收场景也填充便于脚本观测）。
    pub topic: String,
    /// 当前 hat 所在的 phase（未知时为 `"unknown"`）。
    pub phase: String,
    /// 当前 hat + phase 下被 phase authority 允许的 next topic 列表。
    pub allowed_next: Vec<String>,
    /// preset 显式声明的 `activate_next` 候选。
    pub activate_next: Vec<String>,
    /// 拒收场景的错误列表；接受场景应为空。
    pub errors: Vec<crate::emit_result::EmitError>,
    /// Agent 上下文交接包（由 U4 `handoff_from_fixture_input` 生成）。
    pub handoff: Option<crate::emit_result::EmitHandoff>,
    /// U4 (R5): 真实落盘的绝对路径。`recorded=false` 或 `ok=false`
    /// 时强制为 `None`(对应 JSON 键省略)。
    pub target_path: Option<String>,
    /// 2026-07-06-004 plan U9: optional handoff envelope summary.
    /// When `Some`, the JSON serializer attaches the
    /// `handoff_envelope` key with schema_version / to_hat /
    /// success_signal / failure_signal. When `None`, the key is
    /// omitted entirely. The U9 assembly rule is: never attach
    /// the summary on rejection (`ok == false`) so agents cannot
    /// be misled into thinking a rejected envelope was
    /// recognised.
    pub handoff_envelope: Option<HandoffEnvelopeSummary>,
}

impl EmitResult {
    /// 把 policy-check / apply 各阶段离散信号合并为 `EmitResult`。
    ///
    /// 规则见模块级文档：`ok==false` → `recorded=false`、清空
    /// allowed_next / activate_next / handoff / target_path / handoff_envelope。
    pub fn assemble(parts: EmitResultParts) -> Self {
        let (
            effective_recorded,
            effective_allowed_next,
            effective_activate_next,
            effective_handoff,
            effective_target_path,
            effective_handoff_envelope,
        ) = if parts.ok {
            (
                parts.recorded,
                parts.allowed_next,
                parts.activate_next,
                parts.handoff,
                // target_path:仅在 apply 成功路径下填充(由调用方
                // 判断)。这里仅防御 `recorded=false` 时的 None。
                if parts.recorded {
                    parts.target_path
                } else {
                    None
                },
                parts.handoff_envelope,
            )
        } else {
            // 拒收：recorded 强制 false；清空 allowed_next /
            // activate_next / handoff / target_path /
            // handoff_envelope(关键 U9 规则:拒绝时不能凭 payload
            // 硬塞 summary,避免给 agent 错觉以为 envelope 被识别)。
            (false, Vec::new(), Vec::new(), None, None, None)
        };

        Self {
            schema_version: EMIT_RESULT_SCHEMA_VERSION,
            ok: parts.ok,
            recorded: effective_recorded,
            topic: parts.topic,
            phase: parts.phase,
            allowed_next: effective_allowed_next,
            activate_next: effective_activate_next,
            errors: parts.errors,
            handoff: effective_handoff,
            target_path: effective_target_path,
            handoff_envelope: effective_handoff_envelope,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emit_result::{EmitError, EmitHandoff};
    use serde_json::Value;

    /// `ok=false` + errors 非空 → recorded 强制 false，且
    /// allowed_next / activate_next / handoff 被清空。
    #[test]
    fn test_assemble_rejection() {
        let parts = EmitResultParts {
            ok: false,
            recorded: true, // 调用方误传：拒收场景强制覆盖
            topic: "work.done".to_string(),
            phase: "unit_loop".to_string(),
            allowed_next: vec!["work.ready".to_string()],
            activate_next: vec!["executor".to_string()],
            errors: vec![EmitError {
                code: "missing_task_id".to_string(),
                message: "task_id is required".to_string(),
                field: Some("task_id".to_string()),
                suggested_command: None,
            }],
            handoff: Some(EmitHandoff {
                from_hat: "executor".to_string(),
                to_hat: "coordinator".to_string(),
                reason: "phase_complete".to_string(),
            }),
            target_path: None,
            handoff_envelope: None,
        };
        let result = EmitResult::assemble(parts);

        assert!(!result.ok, "ok must be false on rejection");
        assert!(
            !result.recorded,
            "recorded must be false on rejection (override), got: {}",
            result.recorded
        );
        assert!(
            result.allowed_next.is_empty(),
            "allowed_next must be cleared on rejection, got: {:?}",
            result.allowed_next
        );
        assert!(
            result.activate_next.is_empty(),
            "activate_next must be cleared on rejection, got: {:?}",
            result.activate_next
        );
        assert!(
            result.handoff.is_none(),
            "handoff must be None on rejection, got: {:?}",
            result.handoff
        );
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].code, "missing_task_id");
    }

    /// `ok=true, recorded=false` → 形状正确，JSON 不含 errors 键。
    #[test]
    fn test_assemble_policy_check_ok() {
        let parts = EmitResultParts {
            ok: true,
            recorded: false,
            topic: "work.done".to_string(),
            phase: "unit_loop".to_string(),
            allowed_next: vec!["work.ready".to_string()],
            activate_next: vec![],
            errors: vec![],
            handoff: None,
            target_path: None,
            handoff_envelope: None,
        };
        let result = EmitResult::assemble(parts);

        assert!(result.ok);
        assert!(!result.recorded, "policy-check must report recorded=false");
        assert_eq!(result.topic, "work.done");
        assert_eq!(result.phase, "unit_loop");
        assert_eq!(result.allowed_next, vec!["work.ready".to_string()]);
        assert!(
            result.errors.is_empty(),
            "policy-check OK must have empty errors"
        );

        // JSON 形状：errors 键被省略（空 Vec → skip_serializing_if）
        let json: Value =
            serde_json::to_value(&result).expect("EmitResult must serialize");
        let obj = json.as_object().expect("must be object");
        assert_eq!(obj.get("ok"), Some(&Value::Bool(true)));
        assert_eq!(obj.get("recorded"), Some(&Value::Bool(false)));
        assert!(
            obj.get("errors").is_none(),
            "empty errors must be omitted in JSON"
        );
    }

    /// `ok=true, recorded=true` → recorded 字段保留 true。
    #[test]
    fn test_assemble_apply_ok() {
        let parts = EmitResultParts {
            ok: true,
            recorded: true,
            topic: "work.done".to_string(),
            phase: "unit_loop".to_string(),
            allowed_next: vec![],
            activate_next: vec![],
            errors: vec![],
            handoff: None,
            target_path: None,
            handoff_envelope: None,
        };
        let result = EmitResult::assemble(parts);

        assert!(result.ok);
        assert!(
            result.recorded,
            "apply OK must report recorded=true, got: {}",
            result.recorded
        );
        assert_eq!(result.topic, "work.done");
        assert_eq!(result.phase, "unit_loop");

        let json: Value = serde_json::to_value(&result).expect("must serialize");
        let obj = json.as_object().expect("must be object");
        assert_eq!(obj.get("recorded"), Some(&Value::Bool(true)));
    }

    /// schema_version 必须是 `emit_result.v1`（assembly 不应改写）。
    #[test]
    fn test_assemble_schema_version_preserved() {
        let parts = EmitResultParts {
            ok: true,
            recorded: false,
            topic: "x".to_string(),
            phase: "unknown".to_string(),
            allowed_next: vec![],
            activate_next: vec![],
            errors: vec![],
            handoff: None,
            target_path: None,
            handoff_envelope: None,
        };
        let result = EmitResult::assemble(parts);
        assert_eq!(result.schema_version, EMIT_RESULT_SCHEMA_VERSION);
        assert_eq!(result.schema_version, "emit_result.v1");
    }

    // ------------------------------------------------------------------
    // 2026-07-06-004 plan U9: optional HandoffEnvelopeSummary
    // ------------------------------------------------------------------

    fn summary_fixture() -> HandoffEnvelopeSummary {
        HandoffEnvelopeSummary {
            schema_version: "handoff-envelope.v1".to_string(),
            to_hat: "goal-alignment-reviewer".to_string(),
            success_signal: "work.done".to_string(),
            failure_signal: "work.failed".to_string(),
        }
    }

    /// When the assembly is OK and a summary is supplied, the
    /// `handoff_envelope` key is present in the assembled struct
    /// (and therefore in the JSON). The four required fields
    /// round-trip cleanly.
    #[test]
    fn emit_result_includes_handoff_envelope_summary_when_present() {
        let parts = EmitResultParts {
            ok: true,
            recorded: false,
            topic: "work.done".to_string(),
            phase: "unit_loop".to_string(),
            allowed_next: vec![],
            activate_next: vec![],
            errors: vec![],
            handoff: None,
            target_path: None,
            handoff_envelope: Some(summary_fixture()),
        };
        let result = EmitResult::assemble(parts);
        let summary = result
            .handoff_envelope
            .as_ref()
            .expect("accepted path must carry the summary");
        assert_eq!(summary.schema_version, "handoff-envelope.v1");
        assert_eq!(summary.to_hat, "goal-alignment-reviewer");
        assert_eq!(summary.success_signal, "work.done");
        assert_eq!(summary.failure_signal, "work.failed");

        // The JSON form exposes the `handoff_envelope` key with
        // exactly the four documented fields.
        let json: Value = serde_json::to_value(&result).expect("must serialize");
        let obj = json.as_object().expect("must be object");
        let env = obj
            .get("handoff_envelope")
            .and_then(|v| v.as_object())
            .expect("handoff_envelope must appear in JSON");
        assert_eq!(env.len(), 4, "summary must contain exactly 4 fields");
        assert_eq!(
            env.get("schema_version"),
            Some(&Value::String("handoff-envelope.v1".into()))
        );
    }

    /// When the summary is `None` (e.g. disabled or absent from
    /// the payload) the `handoff_envelope` key is omitted in
    /// JSON. Pre-existing emit consumers that don't read this
    /// field see no change (regression defence contract).
    #[test]
    fn emit_result_omits_handoff_envelope_summary_when_disabled_or_absent() {
        let parts = EmitResultParts {
            ok: true,
            recorded: true,
            topic: "work.done".to_string(),
            phase: "unit_loop".to_string(),
            allowed_next: vec![],
            activate_next: vec![],
            errors: vec![],
            handoff: None,
            target_path: Some("/tmp/x.jsonl".to_string()),
            handoff_envelope: None,
        };
        let result = EmitResult::assemble(parts);
        assert!(result.handoff_envelope.is_none());

        let json: Value = serde_json::to_value(&result).expect("must serialize");
        let obj = json.as_object().expect("must be object");
        assert!(
            obj.get("handoff_envelope").is_none(),
            "None summary must omit the JSON key"
        );
    }

    /// Critical U9 contract: on rejection (`ok == false`) the
    /// summary is *cleared* even when the caller supplied one.
    /// Agents must not be misled into thinking a rejected
    /// envelope was recognised by the validator.
    #[test]
    fn emit_result_rejection_does_not_invent_handoff_envelope_summary() {
        let parts = EmitResultParts {
            ok: false,
            recorded: true, // caller error; assemble forces false
            topic: "work.done".to_string(),
            phase: "unit_loop".to_string(),
            allowed_next: vec!["work.ready".to_string()],
            activate_next: vec![],
            errors: vec![EmitError {
                code: "missing_task_id".to_string(),
                message: "task_id is required".to_string(),
                field: Some("task_id".to_string()),
                suggested_command: None,
            }],
            handoff: None,
            target_path: None,
            // Caller pre-populated a summary; assemble must still
            // drop it on rejection.
            handoff_envelope: Some(summary_fixture()),
        };
        let result = EmitResult::assemble(parts);
        assert!(!result.ok);
        assert!(
            result.handoff_envelope.is_none(),
            "rejection path must clear the summary; got {:?}",
            result.handoff_envelope
        );
        let json: Value = serde_json::to_value(&result).expect("must serialize");
        let obj = json.as_object().expect("must be object");
        assert!(obj.get("handoff_envelope").is_none());
    }
}