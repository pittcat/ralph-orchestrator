//! U1: EmitResult 数据类型与 schema 版本常量的纯类型 + 常量。
//!
//! 本模块负责对外暴露 `ralph emit` 统一响应 JSON 的 SSOT（`EmitResult` /
//! `EmitError` / `EmitHandoff`），不包含 policy 接线、CLI 接线或磁盘 I/O。
//!
//! 后续 Unit 在本模块目录（`U2 map_errors` / `U3 allowed_next` / `U4
//! handoff` / `U5 assemble`）以 **纯函数** 形式扩展；U7+ 才开始 CLI
//! 接线。

//! U2：`map_policy_report_to_errors` 纯函数的子模块入口。

pub mod allowed_next;
pub mod assemble;
pub mod handoff;
pub mod map_errors;
pub mod tests;

use serde::Serialize;

/// `ralph emit` 响应 JSON 的 schema 版本常量。
///
/// 所有 `EmitResult` JSON 序列化输出都会携带此键；下游 consumer（脚本、
/// 其它 CLI）应按此版本字符串路由解析逻辑。
pub const EMIT_RESULT_SCHEMA_VERSION: &str = "emit_result.v1";

/// `ralph emit` 的统一响应 JSON。
///
/// 序列化字段顺序稳定：先 schema_version / ok / recorded / topic / phase，
/// 再 allowed_next / activate_next（按需省略空值），最后 errors / handoff。
#[derive(Debug, Clone, Serialize)]
pub struct EmitResult {
    /// JSON schema 版本（恒等于 [`EMIT_RESULT_SCHEMA_VERSION`]）。
    pub schema_version: &'static str,
    /// 业务结果。`true` 表示 emit 被接受（policy 通过），`false` 表示
    /// 被拒收（policy 拒绝 / 校验失败）。
    pub ok: bool,
    /// 是否已 **真实写盘** 到 events ledger。policy-check 阶段始终为
    /// `false`；只有 apply 阶段成功写入后才为 `true`。这是脚本判断
    ///「是否需要后续 reconcile」的关键信号。
    pub recorded: bool,
    /// 当前 emit 的业务 topic（如 `work.done` / `work.ready`）。拒收
    /// 场景下也可填充以供脚本观测。
    pub topic: String,
    /// 当前 hat 所在的 phase（由 preset phase authority 解析）。agent
    /// 据此判断 next step。Phase 未识别时为 `"unknown"`。
    pub phase: String,
    /// 当前 hat + phase 下被 phase authority 允许的 next topic 列表
    /// （按 topic 字符串原样给出），供 agent 决策下一次 emit。
    /// 为空时 **省略** 序列化键。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub allowed_next: Vec<String>,
    /// preset 显式声明的 `activate_next` 候选（如 reviewer → executor）。
    /// 与 `allowed_next` 互补；为空时省略。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub activate_next: Vec<String>,
    /// 拒收场景的错误列表；接受场景固定为空。空时省略。
    #[serde(skip_serializing_if = "Vec::is_empty", default)]
    pub errors: Vec<EmitError>,
    /// Agent 上下文交接包，仅在 preset phase authority 显式产出
    /// handoff 时填充。`None` 时省略。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub handoff: Option<EmitHandoff>,
}

/// 单条 policy / 校验错误。
///
/// agent 应直接读取 `code` 与 `message`，`field` / `suggested_command`
/// 是 **可选** 的可执行修复提示（`None` 时整个键被省略）。
#[derive(Debug, Clone, Serialize)]
pub struct EmitError {
    /// 稳定错误码（CLI agent 据此路由修复策略）。
    pub code: String,
    /// 人类可读错误描述。
    pub message: String,
    /// 触发的 payload 字段名（如缺 `task_id` 时为 `"task_id"`）。
    /// 仅当错误与具体 payload 字段相关时填充。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub field: Option<String>,
    /// 建议 agent 执行的修复命令（含 `ralph` / 字段补全模板等）。
    /// 仅当错误具备可执行修复路径时填充。
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub suggested_command: Option<String>,
}

/// Agent 上下文交接包（output side）。
///
/// 仅描述「该交接给下游 hat」时的可视字段；如何 *生成* 这条 handoff
/// 由 U4（`handoff_from_fixture_input`）给出。
#[derive(Debug, Clone, Serialize)]
pub struct EmitHandoff {
    /// 指向当前 hat 的稳定 id。
    pub from_hat: String,
    /// 接收交接的下游 hat 的稳定 id。
    pub to_hat: String,
    /// 交接原因短语（preset `handoff_reasons` 表里声明的）。
    pub reason: String,
}
