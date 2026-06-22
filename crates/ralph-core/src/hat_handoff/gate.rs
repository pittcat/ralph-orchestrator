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

use crate::config::HatExecutionMode;
use crate::hat_handoff::{
    HatHandoffConfig, allocator, macro_edges,
    publishes_check::{self, TopicViolation},
    validator::{self, HatHandoffViolation},
};
use crate::workflow_contract::handoff_index::HandoffIndex;

use std::path::Path;

/// Reason code 常量,reason_code SSOT(供 CLI/runtime 共享,U7)。
pub const REASON_CODE_HAT_HANDOFF_MISSING_PATH: &str = "hat_handoff_missing_path";
pub const REASON_CODE_HAT_HANDOFF_PATH_ESCAPE: &str = "hat_handoff_path_escape";
pub const REASON_CODE_HAT_HANDOFF_FILENAME_MISMATCH: &str = "hat_handoff_filename_mismatch";
pub const REASON_CODE_HAT_HANDOFF_FILE_NOT_FOUND: &str = "hat_handoff_file_not_found";
pub const REASON_CODE_HAT_HANDOFF_FILE_READ_FAIL: &str = "hat_handoff_file_read_fail";
pub const REASON_CODE_HAT_HANDOFF_STRUCTURE: &str = "hat_handoff_structure_invalid";
pub const REASON_CODE_HAT_HANDOFF_ILLEGAL_EMIT_TOPIC: &str = "hat_handoff_illegal_emit_topic";
pub const REASON_CODE_HAT_HANDOFF_NOT_REQUIRED: &str = "hat_handoff_not_required";

/// Gate 判定结果。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    /// 不是宏观边,无需 handoff(passthrough)。
    NotRequired,
    /// 校验通过:event 应被接受。
    Accept { handoff_path: String },
    /// 校验失败:event 应被拒收并向 emit hat 发 `task.resume`。
    Reject {
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
            reason_code: REASON_CODE_HAT_HANDOFF_ILLEGAL_EMIT_TOPIC,
            message: format!(
                "`## next` action line `{action_line}` references a topic not in downstream publishes {downstream:?}",
                downstream = inputs.downstream_publishes,
            ),
        };
    }

    GateDecision::Accept {
        handoff_path: handoff_path.to_string(),
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
pub fn reject_to_task_resume(
    decision: &GateDecision,
    target_hat: &str,
) -> Option<(String, &'static str)> {
    let (reason_code, message) = match decision {
        GateDecision::Reject {
            reason_code,
            message,
        } => (*reason_code, message.clone()),
        _ => return None,
    };
    let payload = format!(
        "{{\"reason_code\":\"{reason_code}\",\"message\":\"{message}\",\"target_hat\":\"{target_hat}\"}}"
    );
    Some((payload, reason_code))
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
            GateDecision::Reject { reason_code, .. } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_MISSING_PATH);
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
            GateDecision::Reject { reason_code, .. } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_PATH_ESCAPE);
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
            GateDecision::Reject { reason_code, .. } => {
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
            GateDecision::Reject { reason_code, .. } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_FILE_NOT_FOUND);
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
            GateDecision::Reject { reason_code, .. } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_FILE_READ_FAIL);
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
            GateDecision::Reject { reason_code, .. } => {
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
            GateDecision::Reject { reason_code, .. } => {
                assert_eq!(reason_code, REASON_CODE_HAT_HANDOFF_ILLEGAL_EMIT_TOPIC);
            }
            other => panic!("expected Reject, got {other:?}"),
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
            reason_code: REASON_CODE_HAT_HANDOFF_MISSING_PATH,
            message: "missing".into(),
        };
        let (payload, code) = reject_to_task_resume(&decision, "plan-gate").unwrap();
        assert_eq!(code, REASON_CODE_HAT_HANDOFF_MISSING_PATH);
        assert!(payload.contains("reason_code"));
        assert!(payload.contains("plan-gate"));
    }
}
