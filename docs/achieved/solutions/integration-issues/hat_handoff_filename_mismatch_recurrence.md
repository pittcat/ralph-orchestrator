---
title: "ce-executor-serial 第 6 次复发:hat_handoff 拒绝 + task.resume 死信 + stall detector 沉默三重叠加"
date: 2026-06-23
category: integration-issues
module: ce-executor-serial / hat_handoff / recovery / task_resume
problem_type: integration_issue
component: tooling
severity: critical
symptoms:
  - "work.ready 被 hat_handoff gate 连续拒收(filename_mismatch + notes 超词),第 3 次落盘后 executor 永不激活"
  - "task.resume 持续死信(coordinator hat 不订阅),ledger.jsonl iter 推进 vs events.jsonl 仅 4 条业务事件"
  - "loop.batch_sync 无 accept 仍推 iter counter 形成 busy-loop,stall detector 据此误判有进展"
  - "stall detector 8h+ 沉默未报警,drift.jsonl 空白,active-activations.json 仅 ralph 2 条"
  - "30 天第 6 次复发 hat_handoff_filename_mismatch + 第 5 次 task.resume 死信 + 第 4 次 stall detector 沉默"
root_cause: missing_workflow_step
resolution_type: code_fix
tags:
  - ce-executor-serial
  - hat-handoff
  - task-resume
  - dead-letter
  - stall-detector
  - ssot
  - typed-envelope
  - adversarial-review
related_components:
  - "gate::Accept"
  - "recovery.jsonl"
  - "RejectionRecord::from_typed_rejection"
  - "run_stall_detector_on_state"
  - "loop.batch_sync"
  - "CoordinatorDispatcher"
  - "presets/en/ce-executor-serial.yml"
---

# ce-executor-serial 第 6 次复发:hat_handoff 拒绝 + task.resume 死信 + stall detector 沉默三重叠加

## Problem

`hat_handoff_filename_mismatch`(30 天内第 6 次复发)、`task.resume` 死信(第 5 次复发)、stall detector 沉默(第 4 次复发)三重根因叠加,导致 `primary-20260623-025304` run 在 U1 task 上卡死 8h+:task 永 open、`ledger.jsonl` 推进 4 次但 `events.jsonl` 仅 4 条业务事件、5 条 `task.resume` 累计 0 消费、`drift.jsonl` 空白 0 报警。PROMPT.md 强调"一个个完成 Unit",但实际一个 Unit 都没实施。

## Symptoms

- `recovery.jsonl:1` `hat_handoff_filename_mismatch` reason_code:SSOT 派生命名(`compute_filename` 路径)与 agent 提交文件名路径(CLI precheck 路径)出现 iter/seq 漂移,两条路径没有共同锚点。
- `recovery.jsonl:2` `hat_handoff_structure_invalid`:handoff body 实测 58 词,触发 ≤15 词上限拒绝;同时旧 envelope 写盘仍走 `RejectionRecord::new`,未切到 typed factory。
- `events-20260623-025304.jsonl:3-6`:`task.resume` 累计 5 条 0 消费——coordinator hat 不订阅 `task.resume`,而 ralph hat 可以 `target=coordinator` 发出该事件,形成 sender ≠ receiver 的死信场景。
- `ledger.jsonl` 4 条 `loop.batch_sync` iter 推进,但 `events.jsonl` 仍只 4 条业务事件:iter counter 误报"有进展",stall detector 据此判定无 stall,实际是 busy-loop。
- `drift.jsonl` 0 报警持续 8h+,`active-activations.json` 仅 ralph 2 条记录,executor hat 永不激活——typed counter 字段已加但消费侧 0 caller,stall 维度不可见。

## What Didn't Work

- **软提示架构(30 天未根治)**:让 agent 自觉修改文件名以匹配 SSOT,见 `docs/report/2026-06-21-top-3-architectural-instability-factors.md`。本次仍是同一根因的第 6 次复发。
- **仅改 `gate.rs::Accept` 行为(U2 落地)**:CLI precheck `linter.rs::lint_emit` 仍是 agent 提交文件名路径,precheck 与 runtime gate 两条路径分叉,agent 走 precheck 通过后到 runtime 仍被 gate Reject。
- **typed counter 加了但消费侧 0 caller**:round-2 fix summary 明确标为"未闭环"——`RejectionKind` 枚举已定义但 `rejection_stall` 维度从未被 stall detector 读取。
- **`CoordinatorDispatcher::dispatch` 纯函数已落但无 caller**:typed 路由在 `event_loop/mod.rs` 写好但调用方没有把它接进 task.resume emit 流程,typed dispatch 形同空壳。
- **`from_typed_rejection` 工厂方法已落但 `recovery.jsonl` 写盘仍调 `RejectionRecord::new`**:legacy envelope 静默吞 unknown reason_code,typed kind 字段在写盘那一刻已经丢失。
- **用户最初判断"只 rm 旧 handoff 文件 + 重 prepare 就够"**:实际 8h+ 后 5 条 `task.resume` 死信累计,单一文件名修复完全无法解锁——CB-1 修复后阻塞点从文件名漂移平移到 typed dispatch 死信,根因链上每一环都需要独立处理。

## Solution

修复分两轮:第一轮 8 项 CB(2026-06-23-004 plan U1-U8),第二轮 4 项对抗性审查修复(本会话对抗性审查发现 5060/5060 测试通过但仍存在 5 个语义漏洞)。所有改动在 `pittcat-dev` 分支,5066/5066 baseline 通过,**未 commit**。

### 第一轮:8 项 CB(plan 2026-06-23-004 U1-U8)

#### CB-1:SSOT-first 文件名派生覆盖 Reject 路径

**文件**:`crates/ralph-core/src/hat_handoff/gate.rs:144-219`(新函数 `read_handoff_ssot_first`)+ `event_loop/mod.rs:7402-7448`(caller)

```rust
// 新函数 read_handoff_ssot_first:在 SSOT 派生路径存在文件时优先用 SSOT 内容
pub fn read_handoff_ssot_first(
    repo_root: &Path,
    inputs: &GateInputs<'_>,
    agent_handoff_path: Option<&str>,
    consumer_hat: &str,
) -> (Option<String>, FileContent) {
    let expected_seq = inputs.current_seq + 1;
    let ssot_basename = allocator::compute_filename(
        inputs.iteration, expected_seq, inputs.from_hat, consumer_hat,
    );
    let ssot_rel = format!(".ralph/agent/hat-handoff/{ssot_basename}");
    if let Ok(ssot_abs) = allocator::resolve_jailed(repo_root, &ssot_rel) {
        if ssot_abs.is_file() {
            let content = FileContent::from_read_result(std::fs::read_to_string(&ssot_abs));
            if !matches!(content, FileContent::Missing) {
                return (Some(ssot_rel), content);
            }
        }
    }
    // fallback 到 agent 提交路径
    if let Some(path) = agent_handoff_path { /* ... */ }
    else { (None, FileContent::Missing) }
}
```

**CB-2**:`Rejection::kind` typed 字段 + `build_task_resume_payload` 加 `kind`
**文件**:`crates/ralph-core/src/event_loop/rejection.rs:159-176`(`Rejection::kind`)+ `:496-514`(payload)
```rust
pub struct Rejection {
    pub stage: RejectionStage,
    pub source_hat: Option<String>,
    pub business_hat: Option<String>,
    pub topic: String,
    pub violation: String,
    pub retry_key: String,
    pub retry_eligible: bool,
    pub non_retryable_reason: Option<NonRetryableReason>,
    pub target_hat: Option<String>,
    pub original_event_id: Option<String>,
    pub original_ts: Option<String>,
    // CB-2 新增 typed kind 字段,让 task.resume payload 能携带 typed routing 信息
    pub kind: Option<RejectionKind>,
}
```

**CB-3**:`recovery.jsonl` 写盘切到 typed factory
**文件**:`crates/ralph-core/src/state/recovery_log.rs:121-150`(新 `from_reason_code_or_legacy`)+ `correction/mod.rs:357-373`(caller 切换)
```rust
// correction/mod.rs caller 切换
let record = match rejection.kind {
    Some(kind) => RejectionRecord::from_typed_rejection(...),
    None => RejectionRecord::from_reason_code_or_legacy(...),
};
```

**CB-4**:coordinator hat 订阅 `task.resume`
**文件**:`presets/en/ce-executor-serial.yml:401-426`
```yaml
coordinator:
  name: "📋 Coordinator"
  triggers: ["work.start", "task.resume"]  # ← 新增 task.resume 订阅
  publishes: ["work.ready", "work.failed"]
  instructions: |
    ### Task Resume Reception (2026-06-23 fix plan U4, contract bug CB-4)
    When `task.resume` arrives with `payload.kind` in [...],
    the previous handoff was rejected by the gate. Recover the run by:
    1. Reading `.ralph/agent/scratchpad.md` to recover current iteration state.
    2. Re-preparing the handoff artifact via `ralph tools handoff prepare ...`
    3. Re-emitting `work.ready` with the new SSOT-derived filename.
```

**同步**:`crates/ralph-cli/src/config/ralph_config.rs:256-281` 在 `RESERVED_TRIGGERS` 校验加 `coordinator` 例外。

**CB-5**:`loop.batch_sync` guard(避免无 accept 推 iter)
**文件**:`crates/ralph-core/src/event_loop/mod.rs:9372-9404`
```rust
let batch_sync_source = if had_events || !accepted_log_events.is_empty() {
    "loop.batch_sync"
} else {
    "loop.batch_sync.no_progress"  // 不再推进 iter counter
};
```

**CB-6**:stall detector 接 typed counter + emit `stall.handoff_unconsumed`
**文件**:`crates/ralph-core/src/event_loop/mod.rs:10090-10114` + `loop_state.rs:1371-1402`(新 `detect_rejection_stall_kind`)
```rust
// loop_state.rs:1371-1402 新增 typed stall 检测
pub fn detect_rejection_stall_kind(state: &LoopState) -> Option<RejectionKind> {
    let order = [RejectionKind::HandoffFilenameMismatch, ...];
    for kind in order {
        if state.typed_lint_rejection_count(kind) >= REJECTION_WINDOW_THRESHOLD {
            return Some(kind);
        }
    }
    None
}
```

**CB-7**:`on_task_resume` 调 `CoordinatorDispatcher::dispatch` typed 路由
**文件**:`crates/ralph-core/src/event_loop/mod.rs:7643-7680`

**CB-8**:无 consumer 兜底 emit `loop.diagnostic.task_resume_dead_letter`
**文件**:`crates/ralph-core/src/event_loop/mod.rs:7644-7666`
```rust
if !self.registry.has_subscriber("task.resume") {
    let dead_letter = ralph_proto::Event::new(
        "loop.diagnostic.task_resume_dead_letter",
        format!("{{\"reason\":\"no_consumer_for_target_hat\",\"target_hat\":\"{from_hat}\",\"topic\":\"{topic}\"}}"),
    );
    warn!(target_hat = %from_hat, topic = %ev.topic,
        "task.resume has no consumer in registry — emitting dead-letter diagnostic");
    self.state.record_event(&dead_letter);
    rejected_diagnostics.push(dead_letter);
}
```

### 第二轮:4 项对抗性审查修复

第一轮 8 项 CB 修复后 5060/5060 测试通过,但对抗性审查发现 **5 个语义漏洞**(2 P0 + 2 P1 + 1 P2)。修复后 5066/5066。

#### P0-1:CB-1 SSOT-first 加 `parse_filename` 形状 guard

**文件**:`crates/ralph-core/src/hat_handoff/gate.rs:144-219`(扩展 guard 逻辑)

**漏洞**:CB-1 SSOT-first 一开始完全跳过文件名 owner/seq/iter 校验,攻击者只需预写 SSOT 派生路径就能让任意伪 handoff 通过 gate(完全绕过 5 段式结构 + downstream_publishes 检查)。

**修复**:SSOT-first 只在**以下条件满足时**启用:
1. agent 提交路径形状合法(`parse_filename` 返回 Some)
2. 形状合法但 seq/iter 漂移时覆盖
3. 形状不合法(parse 返回 None)时**仍走原 filename_mismatch Reject**
4. SSOT 路径读到的内容必须通过完整 `validate(content)` + `downstream_publishes check` + `from/to owner check`

**新测试**:
- `ssot_does_not_bypass_when_agent_path_malformed`:agent 提交 `bad-name.md`,SSOT 路径写伪 handoff,期望 gate 仍走 filename_mismatch Reject
- `ssot_does_not_skip_content_validation`:agent 提交合法形状但 SSOT 文件 `## next` 引用非法 topic,期望 gate 仍走 illegal_emit_topic Reject

#### P0-2:删 instructions 第 4 条(消除与 CB-7 dispatch 冲突)

**文件**:`presets/en/ce-executor-serial.yml:422-423` → 删除

**漏洞**:原 instructions 第 4 条写"3 consecutive same-kind task.resume → emit plan.blocked",但 CB-7 `CoordinatorDispatcher::dispatch` 在 typed counter ≥ 3 时**已自动** emit `plan.blocked`——两条路径同时触发,`plan.blocked` 双发导致 loop 进入 plan.blocked 终态污染。

**修复**:删除 instructions 第 4 条,加一行解释"`plan.blocked` is emitted automatically by `CoordinatorDispatcher` after 3 consecutive same-kind `task.resume` — do not emit it manually"。

#### P1-3:`LegacyKindStatus` 枚举 + warning(legacy envelope 不静默吞)

**文件**:`crates/ralph-core/src/state/recovery_log.rs:121-218` + `correction/mod.rs:358-389`

**漏洞**:`RejectionRecord::from_reason_code_or_legacy` 在 unknown reason_code 时静默 fallback 到字符串,丢 typed kind 信息。

**修复**:加 `LegacyKindStatus { Typed(RejectionKind), LegacyFromReasonCode(RejectionKind), UnknownReasonCode(String) }` 枚举;`correction/mod.rs` None 分支加 `tracing::warn!`;3 个新测试覆盖 unknown / typed / legacy-with-known-reason 3 种 case。

#### P1-4:plan.blocked 与 task.resume 互斥(match 拆分两路径)

**文件**:`crates/ralph-core/src/event_loop/mod.rs:7669-7738`

**漏洞**:CB-7 在 reject 路径上同时 emit `task.resume` + `plan.blocked`(dispatch 返回 PlanBlocked 时),两条路径同时触发。

**修复**:match 拆分两路径——`PlanBlocked` 早返直接 emit `plan.blocked` 并跳过 `task.resume`;`ThresholdProgressing` 才走 `task.resume` + `has_subscriber` 兜底。

**新测试**:`plan_blocked_skips_task_resume_emit`

## Why This Works

### 根因 1:多模块合同不一致

`compute_filename`(`allocator.rs:79`,SSOT 派生)与 `parse_filename`(`allocator.rs:93`,反向解析)共享同一文件名约定,但 caller 路径走 agent 提交而非 SSOT 派生。**修复**:"用 SSOT"从"覆盖选项"变成"默认行为"——`read_handoff_ssot_first` 在形状合法时强制采用 SSOT 派生值,agent 提交值仅作为 drift 记录不再用于阻断。形状 guard 单独由 `parse_filename` 守住,SSOT 不再"包揽一切"。

### 根因 2:typed routing 缺失

typed kind 在 `RejectionKind` enum 已定义(`gates.rs:135`+),但从 gate reject 到 task.resume consumer 的整条链路每一步都"已就绪但 0 caller"。**修复**:统一把 caller 接上——CB-2 在 `Rejection` 加 typed `kind` 字段,CB-3 写盘切到 typed factory,CB-7 dispatch 走 typed action,CB-6 stall detector 读 typed counter。每一步都有显式 caller,typed 链路从"半成品"变成"可观测可调试"。

### 根因 3:orchestrator 与 agent 合同不稳定

`coordinator hat` 不订阅 `task.resume` 但 ralph hat 能发 `target=coordinator` 的 `task.resume`,**控制 topic 设计允许 sender ≠ receiver**,导致死信无信号。**修复**:CB-4 + CB-8 双管齐下——CB-4 让 coordinator 订阅 `task.resume` 并通过 `RESERVED_TRIGGERS` 白名单控制只允许 coordinator 订阅,CB-8 在无 consumer 时额外 emit `loop.diagnostic.task_resume_dead_letter` 作为兜底信号。两条路径同时修,正常路径(订阅)走主流程,异常路径(无订阅)发诊断。

### 根因 4:busy-loop 信号失真

`loop.batch_sync` 把"无业务事件"也推 iter counter,stall detector 看 iter 推进误判有进展。**修复**:CB-5 加 guard 后,iter counter 仅在 `had_events || !accepted_log_events.is_empty()` 时推进,否则 emit `loop.batch_sync.no_progress`。iter counter 与 events 同步,stall detector 据此才能正确识别 busy-loop 状态。

### 根因 5:对抗性审查发现的语义欺骗(关键教训)

CB-1 SSOT-first 一开始完全跳过文件名 owner/seq/iter 校验,攻击者只需预写 SSOT 路径就能让任意伪 handoff 通过 gate。**修复**:P0-1 加形状 guard + P0-2 删冲突 instructions 是 **测试通过但语义欺骗** 的典型修复——5060/5060 测试通过不等于"安全",必须跑对抗性审查(预写攻击、互斥语义、unknown 兼容性)才能发现。

## Prevention

### 代码层

- 所有 hat_handoff 相关函数必须在 doc comment 写明"agent 不参与文件名构造,SSOT 派生"。`compute_filename` / `parse_filename` / `read_handoff_ssot_first` 函数 doc comment 强制带此约束。
- typed routing 链路必须有 caller,命名约定 `test_typed_*_caller`——任何新增 typed 字段(`RejectionKind` 变体、`CoordinatorAction` 变体)必须有对应 caller 测试,不允许"已定义未调用"。
- any reject → emit `task.resume` 时,必须先检查 `registry.has_subscriber("task.resume", target_hat)`,无 consumer 时**额外** emit `loop.diagnostic.task_resume_dead_letter`。该规则在 `emit_task_resume` 函数 doc comment 中强制。
- **SSOT-first 不取代形状校验**:`read_handoff_ssot_first` 内部必须先 `parse_filename` 形状合法再用 SSOT 覆盖;否则变成攻击向量。

### 测试层

- 新增 BDD scenario 覆盖攻击场景:
  - `tests/scenarios/hat_handoff/ssot_abuse_attack.yml`——预写 SSOT 路径伪 handoff,期望 gate Reject
  - `tests/scenarios/serial_lint/plan_blocked_exclusive.yml`——threshold 到达时仅 emit `plan.blocked`,不 emit `task.resume`
  - `tests/scenarios/recovery/legacy_envelope_warn.yml`——unknown reason_code 触发 `LegacyWithWarn` 状态,日志含 warning
- `./scripts/run-tests.sh` 5066 baseline 不可降低——baseline 是回归门槛,任何 PR 必须维持。
- 跑对抗性审查作为合并前必须项:测试通过率不替代语义审查,5060/5060 通过但有 5 个语义漏洞的教训必须落地。

### 流程层

任何"fix filename mismatch"类型 commit 必须包含:
1. `read_handoff_ssot_first` 形状 guard(防止攻击者预写 SSOT 路径)
2. CLI precheck 与 runtime gate 走同一 SSOT 派生路径(`linter.rs::lint_emit` Accept 分支)
3. BDD 攻击场景测试(`ssot_abuse_attack.yml`)

任何"fix task.resume"类型 commit 必须包含:
1. coordinator hat 订阅 `task.resume`
2. `CoordinatorDispatcher::dispatch` typed 路由 caller
3. `loop.diagnostic.task_resume_dead_letter` 兜底
4. `plan.blocked` 与 `task.resume` 互斥(match 拆分两路径)

**关键教训**:**测试通过不等于语义正确**。对抗性审查必须跑——8 项 CB 修复后 5060 测试通过但有 5 个语义漏洞,本次会话的对抗性审查发现 P0-1 / P0-2 / P1-3 / P1-4 共 4 项语义修复。

## Related

### 同区域 5 次复发历史(30 天)
- `docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md` — 首次 filename_mismatch
- `docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md` — 二次复发 + 3 机制层 gap
- `docs/report/2026-06-18-003-perky-maple-loop-link-diagnosis.md` — 三次复发 + dedup root cause
- `docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md` — 四次复发 + 0 触发铁证
- `docs/report/2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md` — B1-B4 4 叠加 bug

### 总账 / 闭环验证
- `docs/report/2026-06-23-mechanism-review-layer3-history-patterns.md` — 30 天反模式 1-4 归纳
- `docs/report/2026-06-23-mechanism-review-layer2-similar-risks.md` — 未覆盖同类隐患
- `docs/plans/2026-06-23-004-fix-ce-executor-serial-mechanism-close-loop-plan.md` — 闭环 plan
- `docs/report/2026-06-23-005-mechanism-close-loop-verification.md` — 本轮闭环验证报告
- `docs/report/2026-06-23-round-2-fix-summary.md` — 二轮修复总结 + 5 项未闭环
- `docs/report/2026-06-23-final-verification.md` — 最终验证 + 9 项残留风险
- `docs/report/2026-06-23-adversarial-review-of-mechanism-fix.md` — 对抗审查(v1 修复 5 处 destructure 缺 `..`)

### 本次 run 诊断源头
- `docs/report/2026-06-23-ralph-e2e-ce-executor-serial-loop-20260622-182705-diagnosis.md` — 实际 loop_id 是 `primary-20260622-182705`(用户给的 `primary-20260623-025304` 在 docs 中 0 命中)

### 同区域不同视角 solutions
- `docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md` — 5 道机制排查 runbook(可作为现场排查入口)
- `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md` — merry-lotus 跟进
- `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-review-chain-2026-06-17.md` — 3 机制层 gap 系统化总结
- `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-recovery-2026-06-17.md` — U6 验证层
- `docs/solutions/integration-issues/ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md` — perky-maple dedup 修复(KTD1 dedup prune > plan-gate trigger,与本次 4 反模式闭环平行)
- `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` — isolated preset dispatch gap(同架构层)
- `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` — 2026-06-16 stall detector TTL + 7 维→4 维降维

### Auto memory(auto memory [claude])
- `ce-executor task ownership` — coordinator 创建的 task 不可由 executor 启动(preset 未配 coordinator_hats),需新增 task 或配 coordinator_hats
- `ce-executor wave emit policy` — ce-executor wave worker 调 `ralph emit` 时若被 policy 拒,需 `unset RALPH_EVENTS_FILE` 走 marker 兜底
- `payload contract preset baseline` — 旧 `extract_payload_field_refs` 对 5/8 builtin 沉默,8 builtin strict validate 当前 0/0;Phase 2 不能依赖旧 regex 找字段
- `agent kill self parent ralph` — ralph 内的 backend agent 看到 `ps grep ralph` 出来的 PID 不能 kill
- `ce-executor-isolated dispatch gap` — plan-gate→executor 推进时缺桥接事件
- `WAC rollout 003 baseline` — WAC 规则/HandoffIndex/HandoffTracker 已有单测,缺 run_preset_lint 接线 + 主循环集成 + builtin strict 门
- `review-coordinator aggregate-timeout handling` — ce-executor-isolated 下 task.resume 收到 ≥5/7 dim done 且无 P0/P1 → emit review.passed with skip_reason=aggregate_timeout
- `ce-executor stale activation work.done closure` — HARD GATE 静默激活 + U commit 已落地但 work.done 未发时:先提交 plan delta(v9.1 段)再 emit work.done with 真实 task_id
- `ralph emit hat channel routing` — isolated mode 下 `ralph emit` 路由到 `current-hat-events` 指向的 hat-channel