//! 2026-06-18-002 plan U5: 运行时门(纯函数 + 编排包装)。
//!
//! 插入点(state machine 之后,state_projection 之前,KTD-4)。
//! 校验链:macro 判定 → payload `handoff_path` → jail → 文件名
//! seq/iteration/from/to → U3 validator → U4 publishes_check。
//! Accept:`state.hat_handoff_seq += 1`。
//! Reject:`task.resume` + envelope + `HandoffTracker::cancel_pending`
//! 抹掉 policy-accept 时记录的 phantom pending(KTD-5)。
//!
//! 关键设计:本模块**只提供纯函数** `evaluate_event`;编排动作
//! (递增 seq、cancel_pending、emit task.resume)在 `event_loop::mod.rs`
//! 里基于本模块的判定结果执行。这样 CLI policy_check 与 runtime
//! gate 走同一段逻辑(U7 落地基础)。
//!
//! ## 2026-06-23 fix plan P0-2: typed `RejectionKind`
//!
//! `GateDecision::Reject` carries a typed
//! [`crate::preset::engine::gates::RejectionKind`] in addition
//! to the legacy `reason_code: &'static str` (which mirrors the
//! kind's `reason_code()`). The typed kind lets the runtime
//! accumulate per-kind counters via
//! [`crate::event_loop::LoopState::record_typed_lint_rejection`]
//! (new in the 2026-06-23 fix) without string-substring
//! matching — the previous 6-recurrence bug in
//! `primary-20260622-182705` came from string matching on
//! `recovery.jsonl:1-4` to derive `safe_target`. The kind is
//! the new SSOT; the `reason_code` string is kept for
//! backwards-compatible diagnostics and `recovery.jsonl` grep
//! compatibility.
//!
//! ## Follow-up plan status (2026-06-23)
//!
//! This module's typed `Reject` and the typed counter on
//! `LoopState` are the **typed routing infrastructure**. The
//! follow-up plan `2026-06-21-001 U4` is the consumer that
//! turns per-kind counts into:
//!   - kind `HandoffFilenameMismatch` × 2 → drift_finding
//!   - kind `*` × 3 → `loop.circuit_breaker_trip`
//!   - kind `*` × 4 → `plan.blocked(reason=...)`.
//! Until that follow-up lands, the typed counter records
//! rejections but no caller escalates them; this is intentional
//! so the landing block is the typed call site
//! (`record_typed_lint_rejection`), not a string match.

use crate::config::HatExecutionMode;
use crate::hat_handoff::{
    HatHandoffConfig, allocator, macro_edges,
    publishes_check::{self, TopicViolation},
    validator::{self, HatHandoffViolation},
};
use crate::preset::engine::gates::RejectionKind;
use crate::workflow_contract::handoff_index::HandoffIndex;

use std::path::Path;

/// Reason code 常量,reason_code SSOT(供 CLI/runtime 共享,U7)。
///
/// 2026-06-23 fix plan P0-2: these strings remain the
/// `reason_code()` of the typed `RejectionKind`. New code
/// should pattern-match the kind directly; these constants are
/// kept for the CLI precheck path and backwards-compatible
/// diagnostics.
pub const REASON_CODE_HAT_HANDOFF_MISSING_PATH: &str = "hat_handoff_missing_path";
pub const REASON_CODE_HAT_HANDOFF_PATH_ESCAPE: &str = "hat_handoff_path_escape";
pub const REASON_CODE_HAT_HANDOFF_FILENAME_MISMATCH: &str = "hat_handoff_filename_mismatch";
pub const REASON_CODE_HAT_HANDOFF_FILE_NOT_FOUND: &str = "hat_handoff_file_not_found";
pub const REASON_CODE_HAT_HANDOFF_FILE_READ_FAIL: &str = "hat_handoff_file_read_fail";
pub const REASON_CODE_HAT_HANDOFF_STRUCTURE: &str = "hat_handoff_structure_invalid";
pub const REASON_CODE_HAT_HANDOFF_ILLEGAL_EMIT_TOPIC: &str = "hat_handoff_illegal_emit_topic";
pub const REASON_CODE_HAT_HANDOFF_NOT_REQUIRED: &str = "hat_handoff_not_required";

/// Gate 判定结果。
///
/// 2026-06-23 fix plan P0-2: `Reject` carries a typed
/// `kind: RejectionKind` so the runtime can call
/// [`crate::event_loop::LoopState::record_typed_lint_rejection`]
/// without relying on string-substring matching on `reason_code`.
/// The `reason_code` field is retained for backwards-compatible
/// diagnostics (operators grep `recovery.jsonl`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// 不是宏观边,无需 handoff(passthrough)。
    NotRequired,
    /// 校验通过:event 应被接受。
    Accept { handoff_path: String },
    /// 校验失败:event 应被拒收并向 emit hat 发 `task.resume`。
    ///
    /// `kind` is the typed classification; `reason_code` is
    /// `kind.reason_code()` (kept for diagnostics and the
    /// CLI precheck mirror). New code MUST pattern-match on
    /// `kind` rather than `reason_code`.
    Reject {
        kind: RejectionKind,
        reason_code: &'static str,
        message: String,
    },
}

/// Handoff 文件读盘结果(2026-06-18-005 U6,R5)。
///
/// 区分「文件不存在」、「存在且可读」、「存在但读失败」三种状态,
/// 使 `hat_handoff_file_read_fail` reason_code 在 runtime 与 CLI 共享
/// 同一路径可被触发。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FileContent {
    /// handoff_path 缺失(非宏观边 / payload 无 path 时不会进入本函数)。
    Missing,
    /// 读盘成功,文件内容。
    Read(String),
    /// 文件存在但读取失败(权限 / IO 错误等)。
    ReadError(String),
}

impl FileContent {
    /// 从 `std::fs::read_to_string` 的结果构造。把 `NotFound` 折叠为
    /// `Missing`(文件不存在),其余 IO 错误折叠为 `ReadError`。
    /// `resolve_jailed` 失败已经在外层映射为 `Missing`,所以这里的
    /// `Ok(Err)` 必然是「路径存在但读不到」的场景。
    pub fn from_read_result(result: std::io::Result<String>) -> Self {
        match result {
            Ok(content) => FileContent::Read(content),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => FileContent::Missing,
            Err(e) => FileContent::ReadError(e.to_string()),
        }
    }
}

/// 2026-06-23 fix plan U2 (CB-1) + adversarial review P0-1:
/// SSOT-first handoff read with shape guard.
///
/// 消除 `hat_handoff_filename_mismatch` 30 天第 6 次复发:
/// 不论 agent 提交什么 `handoff_path`,caller 在读盘前先尝试
/// SSOT 派生路径(由 `compute_filename` 算出)。如果 SSOT
/// 路径上的文件存在 + 可读,就**用 SSOT 路径**作为
/// `effective_handoff_path`(连同 SSOT 文件内容)返回。
/// Agent 提交错的文件名根本不会进入 gate 的 filename 比对,
/// 直接走 Accept + `register_pending` + `hat_handoff_seq += 1`。
///
/// **P0-1 SSOT-first 安全绕过 guard**:SSOT-first 是**救援机制**
/// (agent 写对了文件名形状但 seq 漂移),**不是绕过机制**(agent
/// 写错文件名也能通过)。当 agent 提交的 `handoff_path` 不能通过
/// `parse_filename` 解析(无 4 段式形状,或无法 parse 数字/owner)
/// 时,SSOT-first **不会启动** —— 让后续 gate 走标准的
/// `HandoffFilenameMismatch` Reject,避免恶意/无心 agent
/// 在 SSOT 派生路径上预写一个伪 handoff 来绕过 gate 的文件名
/// owner / shape 校验。
///
/// 返回 `(effective_handoff_path, file_content)` 元组:
/// - 当 SSOT 文件存在:返回 `(Some(ssot_path), Read(content))`
/// - 否则 fallback 到 agent 提交路径:返回
///   `(agent_handoff_path, agent_file_content)`
/// - 都不可用:返回 `(agent_handoff_path, Missing)`(让 gate 走
///   标准 missing-file Reject)
pub fn read_handoff_ssot_first(
    repo_root: &Path,
    inputs: &GateInputs<'_>,
    agent_handoff_path: Option<&str>,
    consumer_hat: &str,
) -> (Option<String>, FileContent) {
    use crate::hat_handoff::allocator;
    // P0-1 guard: SSOT-first is only a rescue path for agents that
    // wrote the correct filename shape. If the agent's handoff_path
    // does NOT parse (bad shape, non-numeric iter/seq, etc.) we
    // refuse to consult SSOT — gate will run the standard
    // `HandoffFilenameMismatch` Reject, content validation is
    // skipped entirely. This blocks the "pre-write SSOT file to
    // bypass filename owner check" attack.
    let agent_path_parses = agent_handoff_path
        .and_then(|p| allocator::parse_filename(p))
        .is_some();
    if !agent_path_parses {
        // Fall through to standard gate flow: parse_filename will
        // return None → filename_mismatch Reject.
        let path = agent_handoff_path.map(|s| s.to_string());
        return (path, FileContent::Missing);
    }
    let expected_seq = inputs.current_seq + 1;
    let ssot_basename = allocator::compute_filename(
        inputs.iteration,
        expected_seq,
        inputs.from_hat,
        consumer_hat,
    );
    let ssot_rel = format!(".ralph/agent/hat-handoff/{ssot_basename}");
    // SSOT 路径必须在 jail 内(resolve_jailed 失败 → 跳过 SSOT)。
    if let Ok(ssot_abs) = allocator::resolve_jailed(repo_root, &ssot_rel) {
        if ssot_abs.is_file() {
            let content = FileContent::from_read_result(std::fs::read_to_string(&ssot_abs));
            // 读失败时也走 SSOT 路径覆盖(让 ReadError 触发
            // `hat_handoff_file_read_fail` 给 agent 看到真问题),
            // 不要静默 fallback 到 agent 路径。
            if !matches!(content, FileContent::Missing) {
                return (Some(ssot_rel), content);
            }
        }
    }
    // Fallback: agent 提交路径。
    if let Some(path) = agent_handoff_path {
        let file_content = match allocator::resolve_jailed(repo_root, path) {
            Ok(abs) => FileContent::from_read_result(std::fs::read_to_string(&abs)),
            Err(_) => FileContent::Missing,
        };
        (Some(path.to_string()), file_content)
    } else {
        (None, FileContent::Missing)
    }
}

/// 输入参数(纯函数,便于 CLI 镜像)。
#[derive(Debug, Clone)]
pub struct GateInputs<'a> {
    pub config: &'a HatHandoffConfig,
    pub execution_mode: HatExecutionMode,
    pub index: &'a HandoffIndex,
    /// emit hat id(用于自环排除)。
    pub from_hat: &'a str,
    /// 事件 topic。
    pub topic: &'a str,
    /// 当前 iteration(`LoopState.iteration`)。
    pub iteration: u32,
    /// 当前 `LoopState.hat_handoff_seq`(accept 后递增的字段)。
    pub current_seq: u32,
    /// payload 中的 `handoff_path`(macro 边必填)。
    pub handoff_path: Option<&'a str>,
    /// 下游 hat publishes(供 U4 publishes_check)。
    pub downstream_publishes: &'a [String],
    /// 仓库根路径,用于读盘与 jail。
    pub repo_root: &'a Path,
    /// 2026-06-18-005 U2 (R1): 跳过文件名 seq / iter 校验。
    ///
    /// CLI `ralph emit --policy-check` 在 loop 子进程内读到
    /// `RALPH_LOOP_ITERATION` / `RALPH_HAT_HANDOFF_SEQ` 时按真实值校验;
    /// 缺失时(独立调用场景)降级为不校验 seq/iter,避免误杀合法 emit。
    /// runtime gate 始终 `false`,行为不变。
    pub skip_seq_check: bool,
    /// 2026-06-18-001 plan U1: 跳过文件名 `from`/`to` 所有者校验。
    ///
    /// CLI `hat=None` 时 caller 不知道真实 emit hat,无法构造匹配的
    /// `from_hat`;filename 形状的 owner 校验在 `evaluate_event` 内部
    /// 改为可选跳过。path jail / 文件存在 / R15 / 结构校验等不依赖
    /// 真实 from_hat 的检查仍然执行。runtime gate 始终 `false`。
    pub skip_filename_owner_check: bool,
}

/// 纯函数:对单一 event 做 hat_handoff gate 判定。
///
/// **不读盘**:交给 caller(`event_loop::mod.rs`)决定是同步读盘还是
/// 异步 IO;本函数签名只接受 [`FileContent`] 以便 CLI 单测并区分
/// not_found / read_fail(2026-06-18-005 U6)。
pub fn evaluate_event(inputs: &GateInputs<'_>, file_content: &FileContent) -> GateDecision {
    // 1) 宏观边判定
    let is_macro = macro_edges::requires_handoff(
        inputs.config.enabled,
        &inputs.execution_mode,
        inputs.index,
        inputs.topic,
        inputs.from_hat,
        |t| inputs.config.is_exempt(t),
        |t| inputs.config.is_explicit_macro(t),
    );
    if !matches!(is_macro, macro_edges::MacroEdge::Required) {
        return GateDecision::NotRequired;
    }

    // 2) payload 必须有 handoff_path
    let handoff_path = match inputs.handoff_path {
        Some(p) if !p.is_empty() => p,
        _ => {
            return GateDecision::Reject {
                // 2026-06-23 fix plan P0-2: typed classification.
                // Pre-existing reason_code constant retained for
                // backwards-compatible `recovery.jsonl` greps.
                kind: RejectionKind::HandoffArtifact,
                reason_code: REASON_CODE_HAT_HANDOFF_MISSING_PATH,
                message: format!(
                    "macro-edge emit `{topic}` from `{from}` requires payload `handoff_path`; use `ralph tools handoff prepare`",
                    topic = inputs.topic,
                    from = inputs.from_hat,
                ),
            };
        }
    };

    // 3) path jail
    if let Err(err) = allocator::resolve_jailed(inputs.repo_root, handoff_path) {
        return GateDecision::Reject {
            kind: RejectionKind::HandoffArtifact,
            reason_code: REASON_CODE_HAT_HANDOFF_PATH_ESCAPE,
            message: format!(
                "handoff_path `{handoff_path}` is not a safe repo-relative path: {err}"
            ),
        };
    }

    // 4) 文件名解析(iter/seq/from/to)
    let (file_iter, file_seq, file_from, file_to) = match allocator::parse_filename(handoff_path) {
        Some(parts) => parts,
        None => {
            return GateDecision::Reject {
                // 2026-06-23 fix plan P0-2 / P0-1: typed kind for the
                // filename-shape rejection. Same reason_code as the
                // existing iter/seq mismatch path because both are
                // "the filename does not match the SSOT shape", but
                // the kind is more specific for downstream counters.
                kind: RejectionKind::HandoffFilenameMismatch,
                reason_code: REASON_CODE_HAT_HANDOFF_FILENAME_MISMATCH,
                message: format!(
                    "handoff_path `{handoff_path}` does not match `{{iter}}-{{seq+1}}-{{from}}-{{to}}.md` shape",
                ),
            };
        }
    };
    let expected_seq = inputs.current_seq + 1;
    if !inputs.skip_seq_check && (file_iter != inputs.iteration || file_seq != expected_seq) {
        return GateDecision::Reject {
            kind: RejectionKind::HandoffFilenameMismatch,
            reason_code: REASON_CODE_HAT_HANDOFF_FILENAME_MISMATCH,
            message: format!(
                "handoff_path `{handoff_path}` expects iter={exp_iter}, seq={exp_seq}; got iter={got_iter}, seq={got_seq}",
                exp_iter = inputs.iteration,
                exp_seq = expected_seq,
                got_iter = file_iter,
                got_seq = file_seq,
            ),
        };
    }
    // from/to 一致性:emit hat 与 downstream hat(由 consumer_of 推出)
    // 2026-06-18-001 plan U1: hat=None 场景下 caller 无法构造真实
    // from_hat,跳过 owner 校验;path jail / R15 / 结构校验等不依赖
    // 真实 from_hat 的检查仍按原顺序执行。
    if !inputs.skip_filename_owner_check {
        if let Some(consumer) = inputs.index.consumer_of(inputs.topic) {
            let from_ok = file_from == allocator::sanitize(inputs.from_hat);
            let to_ok = file_to == allocator::sanitize(consumer);
            if !from_ok || !to_ok {
                return GateDecision::Reject {
                    kind: RejectionKind::HandoffFilenameMismatch,
                    reason_code: REASON_CODE_HAT_HANDOFF_FILENAME_MISMATCH,
                    message: format!(
                        "handoff_path `{handoff_path}` from/to mismatch: expected `{exp_from}-{exp_to}`, got `{got_from}-{got_to}`",
                        exp_from = allocator::sanitize(inputs.from_hat),
                        exp_to = allocator::sanitize(consumer),
                        got_from = file_from,
                        got_to = file_to,
                    ),
                };
            }
        }
    }

    // 5) 文件读盘(由 caller 提供)
    let content = match file_content {
        FileContent::Missing => {
            return GateDecision::Reject {
                kind: RejectionKind::HandoffArtifact,
                reason_code: REASON_CODE_HAT_HANDOFF_FILE_NOT_FOUND,
                message: format!(
                    "handoff file `{handoff_path}` not found; run `ralph tools handoff prepare --from {from} --to <downstream> --topic {topic}` first",
                    from = inputs.from_hat,
                    topic = inputs.topic,
                ),
            };
        }
        FileContent::ReadError(err) => {
            // 2026-06-18-005 U6 (R5): 文件存在但不可读时返回
            // `hat_handoff_file_read_fail`,与 `file_not_found` 区分。
            // caller 已经把 resolve_jailed 失败 (path 越界) 折叠成 Missing,
            // 所以这里 ReadError 只会是权限/IO 等"真读不到"。
            return GateDecision::Reject {
                kind: RejectionKind::HandoffArtifact,
                reason_code: REASON_CODE_HAT_HANDOFF_FILE_READ_FAIL,
                message: format!(
                    "handoff file `{handoff_path}` exists but could not be read: {err}; check workspace permissions"
                ),
            };
        }
        FileContent::Read(c) => c,
    };

    // 6) 结构校验
    if let Err(err) = validator::validate(content) {
        return GateDecision::Reject {
            // 2026-06-23 fix plan P0-2 / P2-1: typed kind for body
            // structural failures (missing section, out-of-order,
            // missing `## next` field, `## notes` > 15 words,
            // antipattern action line).
            kind: RejectionKind::HandoffStructureInvalid,
            reason_code: REASON_CODE_HAT_HANDOFF_STRUCTURE,
            message: format_violation(err),
        };
    }

    // 7) R15 topic 校验
    let action_line = extract_action_line(content).unwrap_or_default();
    if let Err(TopicViolation::IllegalEmitTopic { .. }) =
        publishes_check::validate(&action_line, inputs.downstream_publishes)
    {
        return GateDecision::Reject {
            // 2026-06-23 fix plan P0-2 / P1-1: typed kind for the
            // illegal-topic rejection. The agent's `## next` line
            // referenced a topic that the downstream hat does not
            // publish (P1-1 root cause: coordinator's `## next`
            // template mentioned `work.ready`, but executor's
            // publishes only contain `work.done` / `work.failed`).
            kind: RejectionKind::HandoffIllegalEmitTopic,
            reason_code: REASON_CODE_HAT_HANDOFF_ILLEGAL_EMIT_TOPIC,
            message: format!(
                "`## next` action line `{action_line}` references a topic not in downstream publishes {downstream:?}",
                downstream = inputs.downstream_publishes,
            ),
        };
    }

    let expected_seq = inputs.current_seq + 1;
    // U2 (plan 2026-06-23-004): handoff 文件名 SSOT 派生。
    // 不论 agent 提交什么 handoff_path,Accept 都用 allocator SSOT 重算
    // 文件名 + 写盘 + register pending。agent 错填文件名根本不会进
    // `HandoffFilenameMismatch` Reject,因为 SSOT 覆盖了它。
    let consumer = inputs.index.consumer_of(inputs.topic);
    let to_hat = consumer.as_deref().unwrap_or("unknown");
    let ssot_basename = allocator::compute_filename(
        inputs.iteration,
        expected_seq,
        inputs.from_hat,
        to_hat,
    );
    GateDecision::Accept {
        handoff_path: format!(".ralph/agent/hat-handoff/{ssot_basename}"),
    }
}

fn format_violation(v: HatHandoffViolation) -> String {
    v.to_string()
}

fn extract_action_line(content: &str) -> Option<String> {
    let mut in_next = false;
    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("## next") {
            in_next = true;
            continue;
        }
        if in_next && trimmed.starts_with("## ") {
            return None; // next 段结束
        }
        if in_next && trimmed.starts_with("**动作**:") {
            return Some(trimmed.trim_start_matches("**动作**:").trim().to_string());
        }
    }
    None
}

/// Reject → task.resume 文案构造(纯函数)。
///
/// `target_hat` 通常是 emit hat(`inputs.from_hat`);若上游 hat
/// 是 plan-gate 且 task.resume 自身不应回到 plan-gate,可由 caller
/// 走 `resolve_target_hat` 决定。
///
/// 2026-06-23 fix plan P0-2: the returned `reason_code` is the
/// typed `RejectionKind::reason_code()` (a stable
/// `&'static str`), so callers that grep
/// `recovery.jsonl:reason_code` see the same string as before.
///
/// 2026-06-23 fix (mechanism review layer 3, anti-pattern 4):
/// the returned tuple now carries the typed `kind` so the
/// `task.resume` consumer can dispatch on the kind instead of
/// scanning the `reason_code` string. The payload JSON also
/// includes an explicit `"kind"` field (`RejectionKind`
/// serialised as its `reason_code()` string for backwards
/// compatibility — the literal `kind` enum string is the new
/// SSOT for typed routing, the `reason_code` field stays for
/// operator grep compatibility).
pub fn reject_to_task_resume(
    decision: &GateDecision,
    target_hat: &str,
) -> Option<RejectTaskResume> {
    let (kind, reason_code, message) = match decision {
        GateDecision::Reject {
            kind,
            reason_code,
            message,
            ..
        } => (*kind, *reason_code, message.clone()),
        _ => return None,
    };
    let payload = format!(
        "{{\"reason_code\":\"{reason_code}\",\"kind\":\"{kind_code}\",\"message\":\"{message}\",\"target_hat\":\"{target_hat}\"}}",
        kind_code = kind.reason_code(),
    );
    Some(RejectTaskResume {
        payload,
        reason_code,
        kind,
    })
}

/// Typed triple returned by [`reject_to_task_resume`]. The
/// `payload` field is the JSON string the loop emits as
/// `task.resume`; `reason_code` is the stable operator-facing
/// string; `kind` is the new typed SSOT for routing decisions.
///
/// 2026-06-23 fix: this struct makes the typed kind part of
/// the function's stable surface so downstream consumers
/// (loop_runner, stall detector, task.resume consumer wiring)
/// can read it directly instead of re-deriving the kind from
/// `reason_code` strings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RejectTaskResume {
    pub payload: String,
    pub reason_code: &'static str,
    pub kind: RejectionKind,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::RalphConfig;

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
    publishes: ["work.ready"]
  executor:
    name: "Executor"
    triggers: ["work.ready"]
    publishes: ["work.done"]
"#;
        let config: RalphConfig = serde_yaml::from_str(yaml).unwrap();
        HandoffIndex::from_config(&config)
    }

    fn temp_repo() -> tempfile::TempDir {
        tempfile::tempdir().unwrap()
    }

    fn valid_handoff() -> &'static str {
        "# Handoff: plan_gate → executor\n\
         ## context\n无\n\n\
         ## changed\n无\n\n\
         ## verify\n未验证\n\n\
         ## next\n\
         **动作**: emit work.done after task completion\n\
         **阻塞**: 无\n\n\
         ## notes\n无\n"
    }

    fn make_inputs<'a>(
        repo_root: &'a std::path::Path,
        index: &'a HandoffIndex,
        config: &'a HatHandoffConfig,
        handoff_path: Option<&'a str>,
    ) -> GateInputs<'a> {
        static DOWNSTREAM: &[String] = &[];
        GateInputs {
            config,
            execution_mode: HatExecutionMode::Isolated,
            index,
            from_hat: "plan-gate",
            topic: "work.ready",
            iteration: 3,
            current_seq: 1,
            handoff_path,
            downstream_publishes: DOWNSTREAM,
            repo_root,
            skip_seq_check: false,
            skip_filename_owner_check: false,
        }
    }

    #[test]
    fn not_required_passthrough() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let cfg = HatHandoffConfig::default(); // disabled
        let inputs = make_inputs(repo.path(), &idx, &cfg, None);
        match evaluate_event(&inputs, &FileContent::Missing) {
            GateDecision::NotRequired => {}
            other => panic!("expected NotRequired, got {other:?}"),
        }
    }

    #[test]
    fn macro_edge_missing_path_rejected() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        let inputs = make_inputs(repo.path(), &idx, &cfg, None);
        match evaluate_event(&inputs, &FileContent::Missing) {
            GateDecision::Reject {
                kind, reason_code, ..
            } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_MISSING_PATH);
                // 2026-06-23 fix (adversarial review P1-4):
                // explicit kind assertion so a kind flip is caught.
                assert_eq!(
                    kind,
                    crate::preset::engine::gates::RejectionKind::HandoffArtifact,
                    "missing-path rejection MUST keep HandoffArtifact kind"
                );
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn path_escape_rejected() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        let inputs = make_inputs(repo.path(), &idx, &cfg, Some("../escape.md"));
        match evaluate_event(&inputs, &FileContent::Missing) {
            GateDecision::Reject {
                kind, reason_code, ..
            } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_PATH_ESCAPE);
                assert_eq!(
                    kind,
                    crate::preset::engine::gates::RejectionKind::HandoffArtifact,
                    "path-escape rejection MUST keep HandoffArtifact kind"
                );
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn filename_seq_mismatch_rejected() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        // current_seq=1 期望 seq=2;给一个 seq=3 文件名。
        let inputs = make_inputs(
            repo.path(),
            &idx,
            &cfg,
            Some(".ralph/agent/hat-handoff/3-3-plan_gate-executor.md"),
        );
        match evaluate_event(&inputs, &FileContent::Missing) {
            GateDecision::Reject {
                kind, reason_code, ..
            } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_FILENAME_MISMATCH);
                // 2026-06-23 fix (adversarial review P1-4):
                // explicit kind assertion — without this, a kind
                // flip to `HandoffArtifact` would silently break
                // the typed escalation chain.
                assert_eq!(
                    kind,
                    crate::preset::engine::gates::RejectionKind::HandoffFilenameMismatch,
                    "filename mismatch MUST keep HandoffFilenameMismatch kind"
                );
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    /// 2026-06-23 fix plan P0-2: the filename iter/seq
    /// mismatch MUST produce a typed `HandoffFilenameMismatch`
    /// rejection, distinct from the older `HandoffArtifact`
    /// (missing path / missing file) rejections. Without this
    /// typing, the runtime cannot differentiate "agent wrote
    /// the wrong filename" from "agent forgot handoff_path
    /// entirely", and the typed escalation accumulator
    /// (`LoopState::record_typed_lint_rejection`) cannot
    /// promote only the filename-mismatch path to a drift
    /// finding.
    #[test]
    fn filename_seq_mismatch_carries_typed_kind() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        let inputs = make_inputs(
            repo.path(),
            &idx,
            &cfg,
            Some(".ralph/agent/hat-handoff/3-3-plan_gate-executor.md"),
        );
        match evaluate_event(&inputs, &FileContent::Missing) {
            GateDecision::Reject {
                kind, reason_code, ..
            } => {
                assert_eq!(
                    kind,
                    crate::preset::engine::gates::RejectionKind::HandoffFilenameMismatch
                );
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_FILENAME_MISMATCH);
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn file_not_found_rejected() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        let inputs = make_inputs(
            repo.path(),
            &idx,
            &cfg,
            Some(".ralph/agent/hat-handoff/3-2-plan_gate-executor.md"),
        );
        match evaluate_event(&inputs, &FileContent::Missing) {
            GateDecision::Reject {
                kind, reason_code, ..
            } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_FILE_NOT_FOUND);
                // 2026-06-23 fix (adversarial review P1-4):
                // explicit kind assertion.
                assert_eq!(
                    kind,
                    crate::preset::engine::gates::RejectionKind::HandoffArtifact,
                    "file-not-found rejection MUST keep HandoffArtifact kind"
                );
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn file_read_error_rejected() {
        // 2026-06-18-005 U6: 区分 file_not_found 与 file_read_fail。
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        let inputs = make_inputs(
            repo.path(),
            &idx,
            &cfg,
            Some(".ralph/agent/hat-handoff/3-2-plan_gate-executor.md"),
        );
        match evaluate_event(
            &inputs,
            &FileContent::ReadError("permission denied".to_string()),
        ) {
            GateDecision::Reject {
                kind, reason_code, ..
            } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_FILE_READ_FAIL);
                assert_eq!(
                    kind,
                    crate::preset::engine::gates::RejectionKind::HandoffArtifact,
                    "file-read-error rejection MUST keep HandoffArtifact kind"
                );
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn structure_violation_rejected() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        // 写一个文件但内容不合法(缺 ## verify)
        let abs = repo
            .path()
            .join(".ralph/agent/hat-handoff/3-2-plan_gate-executor.md");
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(
            &abs,
            "# Handoff: plan_gate → executor\n## context\nx\n## changed\ny\n## next\n**动作**: foo\n**阻塞**: 无\n",
        )
        .unwrap();
        let inputs = make_inputs(
            repo.path(),
            &idx,
            &cfg,
            Some(".ralph/agent/hat-handoff/3-2-plan_gate-executor.md"),
        );
        let content = std::fs::read_to_string(&abs).unwrap();
        match evaluate_event(&inputs, &FileContent::Read(content)) {
            GateDecision::Reject {
                kind, reason_code, ..
            } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_STRUCTURE);
                // 2026-06-23 fix (adversarial review P1-4):
                // explicit kind assertion.
                assert_eq!(
                    kind,
                    crate::preset::engine::gates::RejectionKind::HandoffStructureInvalid,
                    "structure-violation rejection MUST keep HandoffStructureInvalid kind"
                );
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    /// 2026-06-23 fix plan P0-2 / P2-1: a missing
    /// `## verify` section produces a typed
    /// `HandoffStructureInvalid` rejection so the runtime
    /// can promote it through the escalation chain (drift
    /// finding on the 2nd occurrence). The historical
    /// `recovery.jsonl:3` (35-word `## notes` from
    /// `primary-20260622-182705`) was correctly classified
    /// as `hat_handoff_structure_invalid`, but the kind was
    /// string-only — the typed kind makes the upgrade path
    /// explicit.
    #[test]
    fn structure_violation_carries_typed_kind() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        let abs = repo
            .path()
            .join(".ralph/agent/hat-handoff/3-2-plan_gate-executor.md");
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        // 2026-06-23 fix plan P0 (CB-6): the `## notes` word
        // cap was raised from 15 to 50 (see validator.rs).
        // Use a 60-word `## notes` here to exercise the
        // new cap.
        let body = "# Handoff: plan_gate → executor\n\
                    ## context\nx\n\
                    ## changed\ny\n\
                    ## verify\nz\n\
                    ## next\n**动作**: emit work.done after task completion\n**阻塞**: 无\n\
                    ## notes\nthis is a very long notes section that has well over fifty words now because the cap was raised from fifteen to fifty in the same plan, so we need at least sixty words here to trigger the new NotesTooLong rejection path and keep the test exercising the same code branch it did before the cap change.\n";
        std::fs::write(&abs, body).unwrap();
        let inputs = make_inputs(
            repo.path(),
            &idx,
            &cfg,
            Some(".ralph/agent/hat-handoff/3-2-plan_gate-executor.md"),
        );
        let downstream = vec!["work.done".to_string()];
        let content = std::fs::read_to_string(&abs).unwrap();
        let mut inputs = inputs;
        inputs.downstream_publishes = &downstream;
        match evaluate_event(&inputs, &FileContent::Read(content)) {
            GateDecision::Reject {
                kind, reason_code, ..
            } => {
                assert_eq!(
                    kind,
                    crate::preset::engine::gates::RejectionKind::HandoffStructureInvalid
                );
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_STRUCTURE);
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    #[test]
    fn illegal_emit_topic_rejected() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        let path = ".ralph/agent/hat-handoff/3-2-plan_gate-executor.md";
        let abs = repo.path().join(path);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        // valid skeleton but next action emits queue.advance (not in
        // executor publishes).
        std::fs::write(
            &abs,
            "# Handoff: plan_gate → executor\n## context\nx\n## changed\ny\n## verify\nz\n## next\n**动作**: emit queue.advance\n**阻塞**: 无\n## notes\n无\n",
        )
        .unwrap();
        let downstream = vec!["work.done".to_string()];
        let content = std::fs::read_to_string(&abs).unwrap();
        let mut inputs = make_inputs(repo.path(), &idx, &cfg, Some(path));
        inputs.downstream_publishes = &downstream;
        match evaluate_event(&inputs, &FileContent::Read(content)) {
            GateDecision::Reject {
                kind, reason_code, ..
            } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_ILLEGAL_EMIT_TOPIC);
                // 2026-06-23 fix (adversarial review P1-4):
                // explicit kind assertion so a kind flip is caught.
                assert_eq!(
                    kind,
                    crate::preset::engine::gates::RejectionKind::HandoffIllegalEmitTopic,
                    "illegal-emit-topic rejection MUST keep HandoffIllegalEmitTopic kind"
                );
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    /// 2026-06-23 fix plan P0-2 / P1-1: illegal `## next`
    /// topic MUST carry typed `HandoffIllegalEmitTopic`,
    /// matching the `recovery.jsonl:4` failure from
    /// `primary-20260622-182705` (coordinator's `## next`
    /// mentioned `work.ready`, not in executor publishes).
    #[test]
    fn illegal_emit_topic_carries_typed_kind() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        let path = ".ralph/agent/hat-handoff/3-2-plan_gate-executor.md";
        let abs = repo.path().join(path);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        // body claims executor will emit work.ready (not in
        // executor's publishes [work.done, work.failed]).
        std::fs::write(
            &abs,
            "# Handoff: plan_gate → executor\n## context\nx\n## changed\ny\n## verify\nz\n## next\n**动作**: emit work.ready\n**阻塞**: 无\n## notes\n无\n",
        )
        .unwrap();
        let downstream = vec!["work.done".to_string(), "work.failed".to_string()];
        let content = std::fs::read_to_string(&abs).unwrap();
        let mut inputs = make_inputs(repo.path(), &idx, &cfg, Some(path));
        inputs.downstream_publishes = &downstream;
        match evaluate_event(&inputs, &FileContent::Read(content)) {
            GateDecision::Reject {
                kind, reason_code, ..
            } => {
                assert_eq!(
                    kind,
                    crate::preset::engine::gates::RejectionKind::HandoffIllegalEmitTopic
                );
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_ILLEGAL_EMIT_TOPIC);
            }
            other => panic!("expected Reject, got {other:?}"),
        }
    }

    /// 2026-06-23 fix plan U2 (CB-1): agent 提交错误文件名(seq=3,
    /// 但 current_seq=1 期望 seq=2)时,SSOT 派生路径(3-2-...)上
    /// 的文件存在 + 可读,SSOT-first read 必须覆盖 agent 提交,
    /// 返回 `(SSOT_path, SSOT_content)`。这是消除 30 天第 6 次
    /// `hat_handoff_filename_mismatch` 复发的核心机制。
    #[test]
    fn ssot_overrides_mismatched_filename_on_accept() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        // 在 SSOT 派生路径(3-2-...)和 agent 错填路径(3-3-...)都写文件。
        let ssot_rel = ".ralph/agent/hat-handoff/3-2-plan_gate-executor.md";
        let wrong_rel = ".ralph/agent/hat-handoff/3-3-plan_gate-executor.md";
        let ssot_abs = repo.path().join(ssot_rel);
        let wrong_abs = repo.path().join(wrong_rel);
        std::fs::create_dir_all(ssot_abs.parent().unwrap()).unwrap();
        std::fs::write(&ssot_abs, valid_handoff()).unwrap();
        std::fs::write(&wrong_abs, valid_handoff()).unwrap();
        // 构造 inputs:agent 提交错误文件名(3-3 而非 3-2)。
        let inputs = make_inputs(repo.path(), &idx, &cfg, Some(wrong_rel));
        let (effective_path, content) = read_handoff_ssot_first(
            repo.path(),
            &inputs,
            inputs.handoff_path,
            // topic=work.ready 的 consumer 是 executor
            "executor",
        );
        // 关键断言 1:SSOT 派生路径覆盖了 agent 提交路径。
        assert_eq!(
            effective_path.as_deref(),
            Some(ssot_rel),
            "SSOT-first read MUST override agent-submitted wrong filename"
        );
        assert_ne!(
            effective_path.as_deref(),
            Some(wrong_rel),
            "effective path MUST NOT echo agent's wrong filename"
        );
        // 关键断言 2:读到的是 SSOT 文件内容(Missing 才表示 read 失败)。
        let body = match &content {
            FileContent::Read(c) => c.clone(),
            FileContent::Missing => panic!("SSOT file exists, expected Read content"),
            FileContent::ReadError(e) => panic!("SSOT file read failed: {e}"),
        };
        assert!(
            body.contains("## context"),
            "SSOT body should be valid_handoff"
        );
        // 关键断言 3:用 effective_path 重跑 gate,Accept 返回 SSOT basename。
        let mut inputs_override = inputs.clone();
        inputs_override.handoff_path = effective_path.as_deref();
        let downstream = vec!["work.done".to_string()];
        let mut inputs_override = inputs_override;
        inputs_override.downstream_publishes = &downstream;
        match evaluate_event(&inputs_override, &FileContent::Read(body)) {
            GateDecision::Accept { handoff_path } => {
                assert_eq!(
                    handoff_path, ssot_rel,
                    "Accept MUST echo SSOT basename (already verified at commit 91043081)"
                );
            }
            other => panic!("expected Accept (SSOT override), got {other:?}"),
        }
    }

    /// 2026-06-23 fix plan U2 (CB-1): SSOT 文件不存在时,fallback
    /// 到 agent 提交路径(允许 caller 处理 file_not_found / 其他错误)。
    #[test]
    fn ssot_first_falls_back_to_agent_path_when_ssot_missing() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        let agent_rel = ".ralph/agent/hat-handoff/3-2-plan_gate-executor.md";
        let agent_abs = repo.path().join(agent_rel);
        std::fs::create_dir_all(agent_abs.parent().unwrap()).unwrap();
        std::fs::write(&agent_abs, valid_handoff()).unwrap();
        let inputs = make_inputs(repo.path(), &idx, &cfg, Some(agent_rel));
        let (effective_path, content) =
            read_handoff_ssot_first(repo.path(), &inputs, Some(agent_rel), "executor");
        // 当 agent 路径 == SSOT 派生路径时,正常返回 agent 路径。
        assert_eq!(effective_path.as_deref(), Some(agent_rel));
        match content {
            FileContent::Read(_) => {}
            other => panic!("expected Read, got {other:?}"),
        }
    }

    #[test]
    fn happy_path_accept() {
        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        let path = ".ralph/agent/hat-handoff/3-2-plan_gate-executor.md";
        let abs = repo.path().join(path);
        std::fs::create_dir_all(abs.parent().unwrap()).unwrap();
        std::fs::write(&abs, valid_handoff()).unwrap();
        let downstream = vec!["work.done".to_string()];
        let content = std::fs::read_to_string(&abs).unwrap();
        let mut inputs = make_inputs(repo.path(), &idx, &cfg, Some(path));
        inputs.downstream_publishes = &downstream;
        match evaluate_event(&inputs, &FileContent::Read(content)) {
            GateDecision::Accept { handoff_path } => {
                assert_eq!(handoff_path, path);
            }
            other => panic!("expected Accept, got {other:?}"),
        }
    }

    // 2026-06-18-005 U7 (T9): policy-accept 记录 phantom pending
    // → hat_handoff gate 拒收 → cancel_pending 把 phantom 抹掉。
    // 模拟 event_loop 的两步调用:`on_handoff_accepted` + gate reject。
    #[test]
    fn t9_policy_accept_then_gate_reject_clears_phantom() {
        use crate::workflow_contract::handoff_tracker::HandoffTracker;
        use std::time::Instant;

        let repo = temp_repo();
        let idx = two_hat_index();
        let mut cfg = HatHandoffConfig::default();
        cfg.enabled = true;
        let mut tracker = HandoffTracker::new();

        // Step 1: policy accept adds a pending entry
        let event_id = "2026-06-18T00:00:00Z:work.ready".to_string();
        tracker.on_handoff_accepted("work.ready", "executor", &event_id, Instant::now());
        assert_eq!(
            tracker.pending_count(),
            1,
            "policy accept must record pending"
        );

        // Step 2: gate evaluates the same event but handoff_path
        // is missing → Reject
        let inputs = make_inputs(repo.path(), &idx, &cfg, None);
        let decision = evaluate_event(&inputs, &FileContent::Missing);
        match decision {
            GateDecision::Reject { reason_code, .. } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_MISSING_PATH);
            }
            other => panic!("expected Reject, got {other:?}"),
        }

        // Step 3: caller cancels the pending record (mimicking
        // event_loop's gate reject branch)
        let removed = tracker.cancel_pending(&event_id);
        assert!(removed, "cancel_pending must remove the phantom");
        assert_eq!(
            tracker.pending_count(),
            0,
            "after cancel_pending no phantom should remain"
        );
    }

    #[test]
    fn reject_to_task_resume_extracts_reason_code() {
        let decision = GateDecision::Reject {
            kind: crate::preset::engine::gates::RejectionKind::HandoffArtifact,
            reason_code: REASON_CODE_HAT_HANDOFF_MISSING_PATH,
            message: "missing".into(),
        };
        let rtr = reject_to_task_resume(&decision, "plan-gate").unwrap();
        assert_eq!(rtr.reason_code, REASON_CODE_HAT_HANDOFF_MISSING_PATH);
        assert!(rtr.payload.contains("reason_code"));
        assert!(rtr.payload.contains("plan-gate"));
    }

    /// 2026-06-23 fix (mechanism review layer 3, anti-pattern 4):
    /// `task.resume` payload MUST carry the typed kind so the
    /// consumer (coordinator hat in primary-20260622-182705's case)
    /// can dispatch on the kind instead of substring-matching the
    /// reason_code. The payload JSON exposes `kind` as the same
    /// string as `reason_code` so backwards-compatible `recovery.jsonl`
    /// greps continue to work, but the typed field on the in-memory
    /// return value is the new SSOT.
    #[test]
    fn reject_to_task_resume_carries_typed_kind() {
        let decision = GateDecision::Reject {
            kind: crate::preset::engine::gates::RejectionKind::HandoffFilenameMismatch,
            reason_code: REASON_CODE_HAT_HANDOFF_FILENAME_MISMATCH,
            message: "iter/seq drift".into(),
        };
        let rtr = reject_to_task_resume(&decision, "coordinator").unwrap();
        assert_eq!(
            rtr.kind,
            crate::preset::engine::gates::RejectionKind::HandoffFilenameMismatch,
            "typed kind must surface from the Reject decision"
        );
        assert_eq!(rtr.reason_code, REASON_CODE_HAT_HANDOFF_FILENAME_MISMATCH);
        assert!(
            rtr.payload
                .contains("\"kind\":\"hat_handoff_filename_mismatch\""),
            "payload must carry explicit `kind` field for the typed consumer; got: {}",
            rtr.payload
        );
        assert!(
            rtr.payload.contains("coordinator"),
            "payload must carry target_hat for the consumer to route"
        );
    }

    /// 2026-06-23 fix (mechanism review layer 3, anti-pattern 4):
    /// the `task.resume` consumer (coordinator hat in
    /// `primary-20260622-182705`) MUST be able to read the
    /// typed kind from the payload JSON without scanning the
    /// `reason_code` string. The payload carries `kind` as
    /// `RejectionKind::reason_code()`, and a downstream
    /// `coordinator` hat can dispatch on the kind directly.
    ///
    /// This test simulates the consumer side: parse the
    /// payload and verify both `kind` and `reason_code` are
    /// present and equal (typed SSOT + operator-grep SSOT in
    /// the same field set).
    #[test]
    fn reject_to_task_resume_payload_is_consumer_dispatchable() {
        use serde_json::Value;
        let decision = GateDecision::Reject {
            kind: crate::preset::engine::gates::RejectionKind::HandoffIllegalEmitTopic,
            reason_code: REASON_CODE_HAT_HANDOFF_ILLEGAL_EMIT_TOPIC,
            message: "## next action topic not in downstream publishes".into(),
        };
        let rtr = reject_to_task_resume(&decision, "coordinator").unwrap();
        let parsed: Value =
            serde_json::from_str(&rtr.payload).expect("task.resume payload must be valid JSON");
        let kind = parsed
            .get("kind")
            .and_then(|v| v.as_str())
            .expect("payload MUST carry explicit `kind` field for typed consumer dispatch");
        let reason_code = parsed
            .get("reason_code")
            .and_then(|v| v.as_str())
            .expect("payload MUST carry `reason_code` for operator grep compatibility");
        let target_hat = parsed
            .get("target_hat")
            .and_then(|v| v.as_str())
            .expect("payload MUST carry `target_hat` for routing");
        assert_eq!(
            kind, "hat_handoff_illegal_emit_topic",
            "kind field must equal RejectionKind::reason_code() so a consumer can match without parsing"
        );
        assert_eq!(reason_code, "hat_handoff_illegal_emit_topic");
        assert_eq!(target_hat, "coordinator");
    }

    // 2026-06-23 fix plan adversarial review P0-1 (CB-1 SSOT-first
    // 安全绕过):SSOT-first is a rescue path for agents that wrote
    // the correct filename shape but had seq/iter drift. It is NOT
    // a bypass path for agents that wrote a malformed filename.
    // Without the shape guard, a malicious agent could pre-write
    // an arbitrary SSOT file to slip past filename owner checks.
    mod ssot_first_shape_guard {
        use super::*;

        /// P0-1 (CB-1 SSOT-first 安全绕过):agent 提交的文件名无
        /// 4 段式形状(bad-name.md),即便 SSOT 路径上预写了一个
        /// 看起来合法的 handoff,gate 也必须走标准
        /// `HandoffFilenameMismatch` Reject。
        #[test]
        fn ssot_does_not_bypass_when_agent_path_malformed() {
            let repo = temp_repo();
            let idx = two_hat_index();
            let mut cfg = HatHandoffConfig::default();
            cfg.enabled = true;
            // 攻击者预写 SSOT 路径文件,内容看起来合法
            let ssot_basename = allocator::compute_filename(3, 2, "plan_gate", "executor");
            let ssot_abs = repo
                .path()
                .join(".ralph/agent/hat-handoff")
                .join(&ssot_basename);
            std::fs::create_dir_all(ssot_abs.parent().unwrap()).unwrap();
            std::fs::write(&ssot_abs, valid_handoff()).unwrap();
            // 攻击者提交的 handoff_path 形状非法 (bad-name.md)
            let inputs = make_inputs(repo.path(), &idx, &cfg, Some("bad-name.md"));
            // gate 走标准 filename_mismatch Reject
            match evaluate_event(&inputs, &FileContent::Missing) {
                GateDecision::Reject {
                    kind, reason_code, ..
                } => {
                    assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_FILENAME_MISMATCH);
                    assert_eq!(
                        kind,
                        crate::preset::engine::gates::RejectionKind::HandoffFilenameMismatch,
                        "malformed handoff_path MUST surface as HandoffFilenameMismatch Reject, NOT bypass via SSOT"
                    );
                }
                GateDecision::Accept { .. } => panic!(
                    "P0-1 SECURITY: malformed filename MUST NOT Accept via SSOT-first bypass"
                ),
                GateDecision::NotRequired => panic!(
                    "P0-1 SECURITY: macro-edge emit MUST NOT skip filename validation"
                ),
            }
        }

        /// P0-1 (CB-1 SSOT-first 安全绕过):agent 提交合法形状
        /// 文件名,但 SSOT 路径上预写的文件内容里 `## next` 引用
        /// 非法 topic(不在 downstream publishes 中)。即便 SSOT
        /// first 接管文件路径,content validation 仍要触发
        /// `HandoffIllegalEmitTopic` Reject。
        #[test]
        fn ssot_does_not_skip_content_validation() {
            let repo = temp_repo();
            let idx = two_hat_index();
            let mut cfg = HatHandoffConfig::default();
            cfg.enabled = true;
            // 攻击者预写 SSOT 路径,但 ## next 引用 work.deleted
            // (不在 downstream publishes [work.done] 中)
            let bad_body = "# Handoff: plan_gate → executor\n\
                           ## context\n无\n\n\
                           ## changed\n无\n\n\
                           ## verify\n未验证\n\n\
                           ## next\n\
                           **动作**: emit work.deleted after task completion\n\
                           **阻塞**: 无\n\n\
                           ## notes\n无\n";
            let ssot_basename = allocator::compute_filename(3, 2, "plan_gate", "executor");
            let ssot_abs = repo
                .path()
                .join(".ralph/agent/hat-handoff")
                .join(&ssot_basename);
            std::fs::create_dir_all(ssot_abs.parent().unwrap()).unwrap();
            std::fs::write(&ssot_abs, bad_body).unwrap();
            // agent 提交合法形状文件名(但没文件 —— SSOT 接管)
            let inputs_path = ".ralph/agent/hat-handoff/3-2-plan_gate-executor.md";
            let mut inputs = make_inputs(repo.path(), &idx, &cfg, Some(inputs_path));
            let downstream = vec!["work.done".to_string()];
            inputs.downstream_publishes = &downstream;
            // SSOT-first 接管,读 SSOT 内容
            let (eff_path, content) = read_handoff_ssot_first(
                repo.path(),
                &inputs,
                Some(inputs_path),
                "executor",
            );
            // 验证 SSOT-first 确实接管了路径
            assert!(eff_path.is_some());
            let content_owned = match content {
                FileContent::Read(s) => s,
                other => panic!("expected Read content from SSOT, got {other:?}"),
            };
            // 即便路径被 SSOT 接管,content validation 仍要走
            match evaluate_event(&inputs, &FileContent::Read(content_owned)) {
                GateDecision::Reject {
                    kind, reason_code, ..
                } => {
                    assert_eq!(
                        reason_code,
                        REASON_CODE_HAT_HANDOFF_ILLEGAL_EMIT_TOPIC
                    );
                    assert_eq!(
                        kind,
                        crate::preset::engine::gates::RejectionKind::HandoffIllegalEmitTopic,
                        "P0-1 SECURITY: SSOT-first MUST NOT skip content validation; illegal topic Reject MUST fire"
                    );
                }
                GateDecision::Accept { .. } => panic!(
                    "P0-1 SECURITY: SSOT with illegal ## next topic MUST NOT Accept"
                ),
                other => panic!("expected Reject, got {other:?}"),
            }
        }
    }
}
