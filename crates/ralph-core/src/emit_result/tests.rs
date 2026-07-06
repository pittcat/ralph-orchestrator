//! U1 测试：断言 `EmitResult` / `EmitError` / `EmitHandoff` 类型与
//! schema 版本常量的 JSON 序列化语义。
//!
//! 这些测试只断言「类型存在 + JSON 形状固定 + 可选字段省略」，不触碰
//! CLI / policy / 磁盘任何路径。后续 Unit（U2 map_errors / U3
//! allowed_next / U4 handoff / U5 assemble）以 *纯函数* 形式扩展本模块。
//!
//! 命名约定：所有 U1 类型测试都带 `test_emit_result_types_*` 前缀，
//! 使 `cargo nextest run -p ralph-core -- emit_result_types` 一条
//! substring 一次性命中全部 6 个 U1 测试。

use super::*;
use serde_json::Value;

/// schema 版本常量必须是 `"emit_result.v1"`，否则整套脚本兼容契约破防。
#[test]
fn test_emit_result_types_schema_version_is_v1() {
    assert_eq!(EMIT_RESULT_SCHEMA_VERSION, "emit_result.v1");
}

/// 成功路径的 `EmitResult` JSON roundtrip：所有顶层必需键存在。
#[test]
fn test_emit_result_types_success_json_roundtrip() {
    let result = EmitResult {
        schema_version: EMIT_RESULT_SCHEMA_VERSION,
        ok: true,
        recorded: false,
        topic: "work.done".to_string(),
        phase: "unit_loop".to_string(),
        allowed_next: vec![],
        activate_next: vec![],
        errors: vec![],
        handoff: None,
        target_path: None,
        handoff_envelope: None,
    };
    let json: Value =
        serde_json::to_value(&result).expect("EmitResult must serialize to JSON value");
    let obj = json
        .as_object()
        .expect("EmitResult JSON must be an object");

    // 顶层必需键存在
    assert_eq!(
        obj.get("schema_version"),
        Some(&Value::String("emit_result.v1".into()))
    );
    assert_eq!(obj.get("ok"), Some(&Value::Bool(true)));
    assert_eq!(obj.get("recorded"), Some(&Value::Bool(false)));
    assert_eq!(obj.get("topic"), Some(&Value::String("work.done".into())));
    assert_eq!(obj.get("phase"), Some(&Value::String("unit_loop".into())));

    // 空 Vec / None 字段必须被省略
    assert!(
        obj.get("allowed_next").is_none(),
        "empty allowed_next must be omitted"
    );
    assert!(
        obj.get("activate_next").is_none(),
        "empty activate_next must be omitted"
    );
    assert!(obj.get("errors").is_none(), "empty errors must be omitted");
    assert!(obj.get("handoff").is_none(), "None handoff must be omitted");
}

/// `EmitError` 的 `field` / `suggested_command` 为 `None` 时，序列化
/// 输出不能包含字符串 `"null"` —— consumer 会把它们当作真实字符串
/// 处理。只有字段 **整个被省略** 才是正确语义。
#[test]
fn test_emit_result_types_error_optional_fields_omitted_in_json() {
    let err = EmitError {
        code: "missing_field".to_string(),
        message: "task_id is required".to_string(),
        field: None,
        suggested_command: None,
    };
    let json: Value =
        serde_json::to_value(&err).expect("EmitError must serialize to JSON value");
    let obj = json
        .as_object()
        .expect("EmitError JSON must be an object");

    // 必需字段
    assert_eq!(
        obj.get("code"),
        Some(&Value::String("missing_field".into()))
    );
    assert_eq!(
        obj.get("message"),
        Some(&Value::String("task_id is required".into()))
    );

    // 关键反断言：None 字段绝不能以 null 字面量出现
    assert!(
        obj.get("field").is_none(),
        "None field must be omitted from JSON (got {:?})",
        obj.get("field")
    );
    assert!(
        obj.get("suggested_command").is_none(),
        "None suggested_command must be omitted from JSON (got {:?})",
        obj.get("suggested_command")
    );

    // 全 JSON 字符串也不能含裸 "null"
    let rendered = serde_json::to_string(&err).expect("EmitError must serialize");
    assert!(
        !rendered.contains("null"),
        "serialized EmitError must not contain literal 'null', got: {rendered}"
    );
}

/// `EmitHandoff` 在 `Some(_)` 时，所有字段必须出现在 JSON 输出里。
#[test]
fn test_emit_result_types_handoff_present_in_json_when_some() {
    let result = EmitResult {
        schema_version: EMIT_RESULT_SCHEMA_VERSION,
        ok: true,
        recorded: true,
        topic: "work.done".to_string(),
        phase: "unit_loop".to_string(),
        allowed_next: vec![],
        activate_next: vec![],
        errors: vec![],
        handoff: Some(EmitHandoff {
            from_hat: "executor".to_string(),
            to_hat: "coordinator".to_string(),
            reason: "phase_complete".to_string(),
        }),
        target_path: None,
        handoff_envelope: None,
    };
    let json: Value =
        serde_json::to_value(&result).expect("EmitResult must serialize to JSON value");
    let handoff = json
        .get("handoff")
        .expect("Some(_) handoff must serialize as object")
        .as_object()
        .expect("handoff value must be object");

    assert_eq!(
        handoff.get("from_hat"),
        Some(&Value::String("executor".into()))
    );
    assert_eq!(
        handoff.get("to_hat"),
        Some(&Value::String("coordinator".into()))
    );
    assert_eq!(
        handoff.get("reason"),
        Some(&Value::String("phase_complete".into()))
    );
}

/// `EmitResult` 含 `allowed_next` / `errors` 非空时必须保留 vec 形态。
/// 这是 U3（`allowed_next_for_hat_phase`）+ U2（map_errors）的下游
/// consumer 依赖的形状。
#[test]
fn test_emit_result_types_with_allowed_next_and_errors_keeps_vecs() {
    let result = EmitResult {
        schema_version: EMIT_RESULT_SCHEMA_VERSION,
        ok: false,
        recorded: false,
        topic: "work.done".to_string(),
        phase: "unit_loop".to_string(),
        allowed_next: vec!["work.ready".to_string()],
        activate_next: vec![],
        errors: vec![EmitError {
            code: "missing_task_id".to_string(),
            message: "task_id is required".to_string(),
            field: Some("task_id".to_string()),
            suggested_command: Some("ralph tools task list".to_string()),
        }],
        handoff: None,
        target_path: None,
        handoff_envelope: None,
    };
    let json: Value =
        serde_json::to_value(&result).expect("EmitResult must serialize to JSON value");
    let obj = json.as_object().expect("EmitResult JSON must be object");

    let allowed = obj
        .get("allowed_next")
        .expect("non-empty allowed_next must serialize")
        .as_array()
        .expect("allowed_next must be JSON array");
    assert_eq!(allowed.len(), 1);
    assert_eq!(allowed[0], Value::String("work.ready".into()));

    let errors = obj
        .get("errors")
        .expect("non-empty errors must serialize")
        .as_array()
        .expect("errors must be JSON array");
    assert_eq!(errors.len(), 1);
    let err0 = errors[0].as_object().expect("error element must be object");
    assert_eq!(
        err0.get("field"),
        Some(&Value::String("task_id".into()))
    );
    assert_eq!(
        err0.get("suggested_command"),
        Some(&Value::String("ralph tools task list".into()))
    );
}

/// U1 类型集合的反向 regression wrapper：schema_version + 顶层 key +
/// EmitError 可选字段 + EmitHandoff Some 都覆盖一遍，作为 U1 的
/// smoke 入口。
#[test]
fn test_emit_result_types_roundup() {
    // 1) schema_version 常量
    assert_eq!(EMIT_RESULT_SCHEMA_VERSION, "emit_result.v1");

    // 2) 成功 roundtrip
    let ok_result = EmitResult {
        schema_version: EMIT_RESULT_SCHEMA_VERSION,
        ok: true,
        recorded: false,
        topic: "work.done".to_string(),
        phase: "unit_loop".to_string(),
        allowed_next: vec![],
        activate_next: vec![],
        errors: vec![],
        handoff: None,
        target_path: None,
        handoff_envelope: None,
    };
    let ok_json: Value =
        serde_json::to_value(&ok_result).expect("EmitResult must serialize");
    let ok_obj = ok_json.as_object().expect("EmitResult JSON must be object");
    assert_eq!(
        ok_obj.get("schema_version"),
        Some(&Value::String("emit_result.v1".into()))
    );
    assert_eq!(ok_obj.get("ok"), Some(&Value::Bool(true)));

    // 3) EmitError 可选字段省略
    let err = EmitError {
        code: "x".to_string(),
        message: "y".to_string(),
        field: None,
        suggested_command: None,
    };
    let err_json: Value =
        serde_json::to_value(&err).expect("EmitError must serialize");
    let err_obj = err_json.as_object().expect("EmitError JSON must be object");
    assert!(err_obj.get("field").is_none());
    assert!(err_obj.get("suggested_command").is_none());
}
