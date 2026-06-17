---
title: "基座稳定性优化 — 4 块拼图落地路径(2026-06-18)"
date: 2026-06-18
type: implementation-roadmap
status: completed
parent: docs/report/2026-06-18-003-base-stability-optimization-report.md
scope:
  - 4 个不可变原语的具体落地路径
  - 与 06-18 supervisor-wave-protocol-upgrade-requirements.md 的衔接
  - preset 拓扑静态 lint 的具体规则清单
  - 失败模式 3 级分级的缺口补全
author: 主 Agent(基座报告 §3 方向 1+2+3+4 的展开)
---

# 基座稳定性优化 — 4 块拼图落地路径

> 🎯 **目标**:把 `docs/report/2026-06-18-003-base-stability-optimization-report.md` §3 方向 1+2+3+4 的"做什么"展开成"具体怎么落地"。
>
> 重要前提:本报告落笔时(2026-06-18 02:00)发现 06-18 supervisor 母舰文档(`docs/brainstorms/2026-06-18-supervisor-wave-protocol-upgrade-requirements.md`)已**覆盖原 §3 方向 2 的 3/4 个原语**(backpressure / 取消 / 持久化 / 幂等键 / 内容哈希 dedup / 补偿),且把 4 份子 brainstorm(flow-reliability / step-handoff / wave-dimension / recovery-escalation)全部 `superseded_by`。**因此"补 4 个不可变原语"的实际落地路径 ≠ 从零写**,而是**加速 supervisor 母舰落地 + 补 1 个 supervisor 未覆盖的缺口(failure mode 3 级分级)+ 配套 1 份 preset 静态 lint**。

---

## 1. 与 supervisor 母舰的衔接表

| 基座报告 §3 方向 | supervisor 母舰 R-x 覆盖 | 落地路径 | 缺口 |
|---|---|---|---|
| **方向 1**(Preset 拓扑 lint) | ❌ 不覆盖(preset 范畴) | **新建独立 plan** `2026-06-XX-feat-preset-topology-static-lint-plan.md` | 0 |
| **方向 2.P2.1**(Partial wave accept) | ✅ R-A4(`PartialWavePolicy::AllowPartial`)+ R-B4(partial 降级路径) | 跟随 supervisor 计划 U1/U3 落地 | 0 |
| **方向 2.P2.2**(Spawn-failure 显式信号) | ✅ R-A3(`wave.backpressure.paused`)+ R-B3(已 spawn worker kill)+ R-D1(idempotency key) | 跟随 supervisor 计划 U1/U4 落地 | 0 |
| **方向 2.P2.3**(Task.resume schema 强 gate) | ⚠️ 部分覆盖(R-D1 idempotency key 是结构化,但 task.resume payload gate 本身不在 supervisor 范围) | **新建独立 plan** `2026-06-XX-feat-task-resume-schema-gate-strict.md`(基于 06-17 U2 `task_resume_payload_has_required_fields` 升级) | **1 项** |
| **方向 2.P2.4**(Failure mode 3 级分级) | ⚠️ 部分覆盖(recovery-escalation-routing 提了 Soft/Hard/Final,supervisor 提了 cancellation 三档动作,但**机制层不存在"3 级自动 escalation"状态机**——目前是 operator 手工 escalate) | **新建独立 plan** `2026-06-XX-feat-failure-mode-3-tier-statemachine-plan.md` | **1 项** |
| **方向 3**(event_loop 自然收缩) | ❌ 不覆盖(架构规范) | **加 1 行 CI + 1 份 doc** | 0 |
| **方向 4**(Observability 对账) | ⚠️ 部分覆盖(R-A3/R-B1/R-C4 都写了"诊断事件";但 3 套 writer 合一 + mechanism fail-closed 不在 supervisor 范围) | **新建独立 plan** `2026-06-XX-feat-recovery-writer-consolidation-plan.md` | 1 项与方向 4 重叠 |

**结论**:**基座报告 §3 方向 2 实际 = 1 个跟随 supervisor + 1 个新独立 plan(task.resume 强 gate)+ 1 个新独立 plan(failure mode 3 级)**。**不是"补 4 个原语",而是"2 个新原语 + 1 个跟随"**。

下面 4 块拼图按"立即可写"优先级展开。

---

## 2. 拼图 1:Preset 拓扑静态 lint(独立 plan,5-7 天,低风险)

### 2.1 复用基础

- 已有 `crates/ralph-cli/src/loop_runner/preset_lint_gate.rs`(575 行)+ `crates/ralph-core/src/preset_lint/` + 既有 `check_multi_hat_isolation`(R1-R6 已就位)
- 已有 `ralph preset check --strict -H builtin:ce-executor-isolated` 入口(06-17 agent-recovery-mechanism-gaps R-D3 已声明)
- 已有 8 类 obligation 字段(`on_trigger` / `must_emit_any_of` / `must_emit_after_writing` / `must_match_pattern` / `must_update_file` / `must_NOT_emit_when` 等)

### 2.2 8 条规则详细定义

每条规则给:**(a) 不变量形式化 (b) 错误消息模板 (c) 错误严重度 (d) BDD/replay 验证夹具**。

#### Rule 1: `coordinator_hats` 闭包

```yaml
# 不变量:
# ∀ hat H ∈ preset.hats:
#   if H.uses_task_lifecycle (publishes contains {work.start, work.done, work.failed, queue.advance, fix.applied, fix.exhausted, ...})
#   then H ∈ preset.tasks.coordinator_hats
```

- 错误模板:`hat '<H>' publishes lifecycle topics <{X, Y, Z}> but is not in coordinator_hats. add to tasks.coordinator_hats: - <H>`
- 严重度:**Error**(启动拒)
- 验证:`presets/en/ce-executor-isolated.yml` 通过;故意删 `coordinator_hats: - reporter` → 启动拒 + 错误消息准确

#### Rule 2: `plan-gate.triggers` 闭包

```yaml
# 不变量:
# preset.hats.plan-gate.triggers ⊇
#   {review.passed, review.complete, work.failed, fix.exhausted, debug.exhausted, loop.cancel}
```

- 错误模板:`plan-gate.triggers missing required topic(s): <{fix.exhausted}>. add to triggers list`
- 严重度:**Error**
- 验证:故意删 `fix.exhausted` → 启动拒

#### Rule 3: `reporter` fail 拒发 obligation

```yaml
# 不变量:
# if hat.reporter.exists:
#   reporter.obligations must contain:
#     on_trigger: REVIEW_COMPLETE
#     must_NOT_emit_when: {pass_or_fail: "fail"}: [LOOP_COMPLETE]
```

- 错误模板:`reporter missing 'must_NOT_emit_when pass_or_fail=fail' obligation for LOOP_COMPLETE. preset will silently allow fail → LOOP_COMPLETE. add obligation block.`
- 严重度:**Error**
- 验证:故意删 obligation → 启动拒

#### Rule 4: `topic_deny_rules` 与 hat instructions 一致

```yaml
# 不变量:
# ∀ (hat, topic) ∈ preset.event_policy.topic_deny_rules:
#   hat.instructions 必须包含字面量 "DO NOT emit <topic>" 或 "MUST NOT publish <topic>"
#   (lint 走 substring match,大小写不敏感)
```

- 错误模板:`hat '<H>' has topic_deny_rule for '<T>' but instructions do not contain 'DO NOT emit <T>'. agent 90% 概率会 emit 拒绝 topic — add explicit ban.`
- 严重度:**Warning**(启动通过但 warn,可 `--strict` 升 Error)
- 验证:故意删 instructions 中的 ban → warning;故意同时删 deny rule → 无 warning(本 lint 不强制必须有 deny rule)

#### Rule 5: `dimension-reviewer` 强约束

```yaml
# 不变量:
# if hat.dimension-reviewer.exists:
#   dimension-reviewer.obligations must contain must_match_pattern:
#     findings_file: "^\.agents/scratchpad/.+/findings-.+-task-.+\.json$"
```

- 错误模板:`dimension-reviewer missing 'must_match_pattern findings_file' obligation. preset will accept findings-all-no-task-id.json. add pattern obligation.`
- 严重度:**Error**
- 验证:故意删 obligation → 启动拒

#### Rule 6: `fixer` 强约束

```yaml
# 不变量:
# if hat.fixer.exists:
#   fixer.obligations must contain must_emit_after_writing: fix-log.md
```

- 错误模板:`fixer missing 'must_emit_after_writing fix-log.md' obligation. agent 跳过 fix-log.md 直接发 fix.applied 也通过 — add writing obligation.`
- 严重度:**Error**
- 验证:故意删 obligation → 启动拒

#### Rule 7: `plan-gate` 强约束(progress 更新)

```yaml
# 不变量:
# if hat.plan-gate.exists:
#   plan-gate.obligations must contain must_update_file: progress.md
#     before emit queue.advance
```

- 错误模板:`plan-gate missing 'must_update_file progress.md before emit queue.advance' obligation. progress.md 永远 step-1 in_progress — add update obligation.`
- 严重度:**Error**
- 验证:故意删 obligation → 启动拒

#### Rule 8: Trigger/Publish 拓扑对称

```yaml
# 不变量:
# ∀ topic T:
#   if ∃ hat H1 such that T ∈ H1.publishes:
#     then ∃ hat H2 such that T ∈ H2.triggers  (即 T 至少有 1 个订阅者)
# 排除项:LOOP_COMPLETE / loop.cancel / human.guidance 等 control topics(走 RALPH_CONTROL_TOPICS 白名单)
```

- 错误模板:`topic '<T>' is published by hat '<H1>' but no hat subscribes to it. dead topic — remove or add trigger.`
- 严重度:**Warning**
- 验证:故意加 `publishes: [nonexistent.topic]` → warning;故意让 1 个 control topic 走 trigger list → 无 warning(白名单)

### 2.3 实施路径(分 3 个 PR)

| PR | 范围 | 风险 |
|---|---|---|
| **PR 1**:Rule 1+2+3(coordinator_hats / plan-gate triggers / reporter fail obligation) | 3 条最易塌方点,先 ship | 极低 |
| **PR 2**:Rule 5+6+7(agent 强约束 3 条) | 与 obligation 字段对齐 | 低 |
| **PR 3**:Rule 4+8(deny/instructions 一致性 + 拓扑对称) | Warning 级别,不动 preset 启动 | 极低 |

每条规则:
- 1 个新函数 `preset_lint::check_<rule>()`
- 1 个 BDD scenario 在 `crates/ralph-core/tests/scenarios/`
- 1 个 fixture 故意违规 + 1 个 fixture 合规

### 2.4 验收

```bash
# 全部内置 preset 跑 8 条规则应全过
cargo run -p ralph-cli -- preset check --strict -H builtin:ce-executor-isolated
cargo run -p ralph-cli -- preset check --strict -H builtin:ce-executor-serial
cargo run -p ralph-cli -- preset check --strict -H builtin:ce-executor-wave

# 故意违规必拒
# 1. 删 ce-executor-isolated.yml coordinator_hats: - shipper
# 2. 跑 ralph run -H builtin:ce-executor-isolated --validate-only
# 3. 期望:exit 1 + 错误消息 "hat 'shipper' publishes lifecycle topics ... but is not in coordinator_hats"

# CI gate:
.github/workflows/preset-lint.yml:
  - run: cargo run -p ralph-cli -- preset check --strict --all
```

### 2.5 不做的事(YAGNI)

- **不做"自定义 obligation 字段"**——已加 6+ 种 obligation 字段,够用,不要无限加
- **不做"语义级 lint"**(如"plan-gate 不应该有 build.done 触发器")——跨 hat 语义 lint 复杂度高,Rule 8 拓扑对称已经覆盖 80% 场景
- **不改 preset 默认值**——只加 lint,不改任何已有 preset;如有 lint 失败,**先记 warning,让 preset 作者主动改**

---

## 3. 拼图 2:Failure Mode 3 级分级(独立 plan,1 周,中风险)

> 报告 §1.2 提到"Responder 一直 Soft 升级不到 Hard"(2026-06-09 机制 vs 编排报告 §2 已记录,2 月后未修)。这是 4 个原语里**唯一没被 supervisor 母舰覆盖**的。

### 3.1 当前状态

```rust
// crates/ralph-core/src/drift/engine.rs:217-227 已有 Responder 升级路径
// 实际跑出来:drift 都是 Soft,没看到 Hard 升级触发
// 根因:Hard 升级需要 drift_finding_count > 阈值,阈值过高 + 升级条件未 fail-closed
```

```rust
// crates/ralph-core/src/event_loop/mod.rs:1401 verdict_gate 只挡 LOOP_COMPLETE
// 不挡 report.done(report.done 在 fail 时也通过)
// 这就是 §1.2 报告里说的"verdict_gate 不覆盖 report.done"
```

```rust
// crates/ralph-core/src/diagnosis/responder.rs 已有 Soft/Hard/Final 枚举
// 但没有"3 级自动 escalation 状态机"——目前是 operator 手工 escalate
```

### 3.2 3 级分级状态机设计

```
                drift finding count ↑ (sliding window 50 events)
                ┌─────────────────────────────────────────────┐
                │                                             │
   ┌────────────▼────────────┐                ┌──────────────▼──────────────┐
   │  Soft (level 0)         │   count ≥ 5    │  Hard (level 1)              │
   │  - log envelope         │  ─────────►    │  - inject targeted           │
   │  - continue loop        │                │    task.resume with          │
   │  - drift.jsonl 写盘     │                │    schema-compliant payload  │
   │  - 没 hat routing       │                │  - pin pending_recovery_hat  │
   │                         │                │  - pending_recovery_hat      │
   │                         │                │    不可被 round-robin 覆盖   │
   └─────────────────────────┘                └──────────────┬──────────────┘
                                                               │
                                                  pending_recovery_hat
                                                  连续 3 次未 emit 终态
                                                  ┌────────────▼────────────┐
                                                  │  Final (level 2)         │
                                                  │  - emit 受控降级         │
                                                  │    (plan.complete 或     │
                                                  │     REVIEW_COMPLETE)     │
                                                  │  - clear pending_recovery│
                                                  │  - 进入 TerminationReason│
                                                  │    = loop_converged_     │
                                                  │    partial                │
                                                  │  - 走 verdict_gate fail   │
                                                  └─────────────────────────┘
```

### 3.3 关键 invariants

```rust
// 新增文件 crates/ralph-core/src/diagnosis/failure_state_machine.rs

pub struct FailureStateMachine {
    // 当前 drift_finding_count(sliding window 50 events)
    drift_count: u32,
    // 当前 escalation 级别
    level: EscalationLevel,
    // 当前 pending_recovery_hat(若有)
    pending_recovery_hat: Option<HatId>,
    // pending_recovery_hat 连续不响应次数
    pending_attempts: u32,
    // escalation 链(防 A→B→A 循环)
    escalation_chain: Vec<HatId>,
}

impl FailureStateMachine {
    /// 每次 drift finding 写盘时调用
    /// 返回:是否升级到新级别 + 是否需要 emit
    pub fn on_drift_finding(&mut self, finding: &DriftFinding) -> EscalationDecision;

    /// 每次 hat activation 完调用
    /// 用于判断 pending_recovery_hat 是否被响应
    pub fn on_hat_activation(&mut self, hat: HatId, emitted_topics: &[Topic]) -> EscalationDecision;

    /// 检查 escalation chain 是否有循环
    pub fn detect_cycle(&self) -> Option<Vec<HatId>>;
}
```

### 3.4 触发条件形式化

| Level | 升级条件 | 触发动作 | 可恢复? |
|---|---|---|---|
| **Soft** | drift_finding_count ∈ [1, 4] | log envelope + drift.jsonl 写盘;loop 继续 | ✅ |
| **Hard** | drift_finding_count ≥ 5 **或** pending_recovery_hat 连续 1 次未 emit 终态 | inject targeted task.resume(pin pending_recovery_hat);loop 暂停 1 iter | ✅(agent 可 emit 终态降级) |
| **Final** | pending_recovery_hat 连续 ≥ 3 次未 emit 终态 **或** escalation_chain 出现 A→B→A 循环 | emit `plan.complete` / `REVIEW_COMPLETE(pass_or_fail=partial)`;走 verdict_gate fail;写 `TerminationReason=loop_converged_partial` | ❌(明确终态) |

### 3.5 与已有机制的交互

- **stall_recovery**(`U0`):本 FSM 接管 Soft 级别的 stall 行为;stall_recovery 只负责"hat 静默 → inject task.resume",FSM 负责"多次 stall → Final"
- **verdict_gate**(`event_loop/mod.rs:1401`):升级 verdict_gate 同时检查 `report.done` payload 的 `pass_or_fail`,与本 FSM 的 Final 终态对齐
- **recovery-escalation-routing**(06-18 子 doc):FSM 升级到 Hard 时**消费** escalation routing 表决定 `target_hat`,避免 routing 链循环

### 3.6 实施路径(3 个 unit)

| Unit | 范围 | 工作量 |
|---|---|---|
| **U1**:FailureStateMachine 状态机实现 | 新建 `crates/ralph-core/src/diagnosis/failure_state_machine.rs`(~300 行) | 3 天 |
| **U2**:与 drift/stall_recovery/verdict_gate 集成 | 修改 3 个调用点;新增 drift_count sliding window 维护 | 2 天 |
| **U3**:BDD + replay fixture | 4 个 BDD scenario:Soft → Hard 升级 / Hard → Final 升级 / A→B→A 循环终止 / drift_count 衰减规则 | 2 天 |

### 3.7 验收

```bash
# U1 单元测试
cargo nextest run -p ralph-core -- failure_state_machine

# U2 集成测试:跑一遍 jolly-pine 的 events.jsonl(已有),验证 Soft drift 触发 → drift_count 累计 → Hard 升级 → task.resume 注入
cargo nextest run -p ralph-core --test replay_jolly_pine

# U3 BDD
cargo nextest run -p ralph-core --test scenarios -- failure_escalation
```

### 3.8 与 supervisor 母舰的衔接

```rust
// supervisor 母舰的 R-B3 worker kill 与本 FSM 无关(那是 wave worker cancel,本 FSM 是 hat-level failure)
// supervisor 母舰的 R-D1 idempotency key 与本 FSM 无关(那是 dispatch 去重,本 FSM 是 escalation 状态)
// supervisor 母舰的 R-F1~F4 compensation 与本 FSM 有关联:Final 升级时,本 FSM 触发 CompensationPlan.on_failure
// 衔接点:FailureStateMachine.on_final_escalation() 调用 supervisor 的 CompensationPlan executor
```

---

## 4. 拼图 3:Task.Resume Schema 强 Gate(独立 plan,2-3 天,中-低风险)

> 报告 §1.2 + §1.4 + §1.5 都指向同一根因:orchestrator 自注入 `task.resume` 字段不全。06-17 U2 修了 12 个注入点中 11 个,但**新加的注入点无人监督**,还有 4+ 个 `human.guidance` / `inject_*` 注入点未检查。

### 4.1 当前状态

```rust
// crates/ralph-core/src/event_loop/rejection.rs:358 build_task_resume_payload
// 12 个注入点中 11 个经过 task_resume_payload_has_required_fields
// 06-17 U2 之后,该 gate 仍只在 rejection.rs 强制
// 其它 orchestrator 注入路径(inject_fallback_event / inject_hard_gate_guidance / ...)没接该 gate
```

### 4.2 升级方案

```rust
// 新增 crates/ralph-core/src/event_loop/orchestrator_inject_guard.rs

pub struct OrchestratorInjectGuard;

impl OrchestratorInjectGuard {
    /// 在 publish 任何 orchestrator 注入的事件前调用
    /// 返回:Err 表示拒收 + 写 recovery.jsonl envelope
    pub fn pre_inject(
        topic: &Topic,
        payload: &Value,
        injector: Injector,  // TaskResumeInjector / HardGateInjector / FallbackInjector / etc.
    ) -> Result<(), InjectViolation>;

    /// 收集所有已知 injector
    pub fn known_injectors() -> Vec<Injector>;
}
```

### 4.3 接入点清单(全 16 个注入点)

| 注入点 | 当前位置 | 当前 gate | 升级后 |
|---|---|---|---|
| `build_task_resume_payload` | `rejection.rs:358` | ✅ U2 | ✅ strict |
| `inject_fallback_event` | `event_loop/mod.rs:1835` | ❌ | ✅ strict |
| `inject_hard_gate_guidance` | `loop_runner/hard_gate.rs` | ❌ | ✅ strict |
| `inject_missing_event_hard_gate_guidance` | `loop_runner/hard_gate.rs:422` | ✅ U3 task.resume,但 schema 未强 | ✅ strict |
| `inject_wave_policy_rejection_guidance` | `loop_runner/hard_gate.rs:594` | ❌ (deferred) | ✅ strict |
| `apply_step_handoff_gate` inject `plan.blocked` | `event_loop/mod.rs:887-893` | ❌ (review P1 #1) | ✅ strict |
| `maybe_emit_incomplete_wave_blocked` | `flow_lifecycle.rs` | ❌ | ✅ strict |
| `stall_recovery` 注入 `task.resume` | `loop_runner/` 多处 | ❌ | ✅ strict |
| ... 其余 8 个 | 散落 | ❌ | ✅ strict |

### 4.4 严格 schema 模板

```yaml
# task.resume schema(继承现有 06-17 U2 设计,扩展为 strict)
task.resume:
  required_fields:
    - reason: string (non-empty)
    - target_hat: string (non-empty, must ∈ preset.hats or "ralph")
    - rejected_topic: string
    - source_hat: Option<string>
  allowed_values:
    reason: (extract_reason_code 的合法值,白名单 ~20 种)
  payload: json_object (NOT string, NOT null)
```

### 4.5 实施路径

| Unit | 范围 | 工作量 |
|---|---|---|
| **U1**:OrchestratorInjectGuard 实现 | 新建文件(~200 行) | 1 天 |
| **U2**:16 个注入点全接入 | 修改 6-8 个文件 | 1 天 |
| **U3**:测试 | 16 个注入点 fixture + 故意违规 fixture | 0.5 天 |

### 4.6 验收

```bash
# 故意写 1 个坏注入器(没经过 guard)
# 1. 修改 inject_fallback_event 去掉 guard 调用
# 2. cargo nextest run -p ralph-core -- orchestrator_inject_guard
# 3. 期望:测试 fail + 错误消息指出注入点位置
```

---

## 5. 拼图 4:加速 supervisor 母舰落地(已有 plan,跟随 supervisor 团队)

### 5.1 supervisor 母舰 6 件套 vs 报告"4 个不可变原语"映射

| supervisor 6 件套 | 报告原语 | 已实现程度 |
|---|---|---|
| **A. Backpressure** (R-A1~A4) | ❌ 报告未列,但本质是 wave 路径的 partial wave 上游 | 0% |
| **B. 分布式取消** (R-B1~B4) | ❌ 报告未列,本质是 wave 路径的 Final 升级上游 | 0% |
| **C. 状态持久化** (R-C1~C4) | ❌ 报告未列 | 0% |
| **D. 幂等键** (R-D1~D4) | 报告 P2.3(task.resume schema gate 强约束)同源 | 0%(仅 task.resume 部分有) |
| **E. 内容哈希 dedup** (R-E1~E4) | 报告 P2.1 partial wave accept 同源 | 0% |
| **F. 补偿路径** (R-F1~F4) | 报告 P2.4 failure mode 3 级 Final 出口同源 | 0% |

### 5.2 衔接关系

```
                    报告"补 4 个不可变原语"
                  ┌─────────────────────────────┐
                  │ P2.1 partial wave accept    │◄──── supervisor R-A4 + R-E1~E4
                  │ P2.2 spawn-failure 信号     │◄──── supervisor R-A3 + R-B3 + R-D1
                  │ P2.3 task.resume schema     │◄──── supervisor R-D1(部分)
                  │ P2.4 failure mode 3 级      │◄──── supervisor R-F1~F4(Final 出口)
                  └─────────────────────────────┘
                              │
                              ▼
                    ┌──────────────────┐
                    │ 实际落地路径     │
                    ├──────────────────┤
                    │ 拼图 1:lint      │  ← 独立
                    │ 拼图 2:3 级 FSM  │  ← 独立(补 supervisor F 未覆盖的"自动升级")
                    │ 拼图 3:inject   │  ← 独立(补 supervisor 未覆盖的"其它注入点")
                    │ 拼图 4:跟随      │  ← supervisor U1~U5 全部
                    └──────────────────┘
```

### 5.3 建议合并执行

| 合并 plan | 包含单元 | 跨 PR 数 | 总工作量 |
|---|---|---|---|
| **Plan A:Preset 拓扑 lint** (拼图 1) | 8 条规则分 3 PR | 3 | 5-7 天 |
| **Plan B:Failure FSM + Inject Guard** (拼图 2+3) | U1~U3 × 2 | 2 | 1.5-2 周 |
| **Plan C:Supervisor 协议升级** (拼图 4,跟随现有 supervisor 团队) | 6 件套 | 6 | 4-6 周 |

### 5.4 与 supervisor 团队的对接动作

1. **本 doc 标记为 `related:`** — supervisor 母舰 2026-06-18 已声明 4 份子 doc `superseded_by` 自身,本 doc 应作为"如何让 supervisor 落地 + 补 supervisor 缺口"的姊妹
2. **认领 supervisor U5/U6**(补偿路径 + 幂等键去重)—— 这两件是 supervisor F + E,与本 doc 拼图 2 的 Final 升级紧密耦合,认领避免重复工作
3. **共享 BDD 夹具** — supervisor F1~F5 关键路径,本 doc 拼图 2 的 failure_escalation BDD 可直接复用 supervisor 已有 fixture

---

## 6. 时间线与依赖

```
Week 1-2  [拼图 1 PR 1] coordinator_hats / plan-gate triggers / reporter fail obligation
Week 1-2  [拼图 3] U1~U3 task.resume inject guard
          ↓
Week 3-4  [拼图 1 PR 2] agent 强约束 3 条
Week 3-4  [拼图 2] U1~U3 failure FSM
          ↓
Week 5-6  [拼图 1 PR 3] deny/instructions + 拓扑对称
Week 5-10 [拼图 4] 跟随 supervisor 母舰 6 件套(并行)
          ↓
Week 11+  [基座稳态验收] 跑 keen-fern / jolly-pine / noble-peacock / merry-lotus replay fixture
          ↓
Week 12+  [方向 4+5] observability 对账 + schema 演化
```

**关键不变量**:
- 拼图 1 完成后,**任何新加 preset** 必跑 8 条 lint,缺一项拒启动
- 拼图 2+3 完成后,**任何新加 orchestrator 注入点** 必过 `OrchestratorInjectGuard`,缺字段 panic
- 拼图 4 完成后,**任何 wave 行为** 必过 6 件套,backpressure / cancel / persist / dedup / compensation 全部 fail-closed

---

## 7. 衡量"4 块拼图坐稳"的成功指标

(独立于基座报告 §6 的指标,聚焦"4 块拼图"自身)

| 指标 | 现状 | 目标 |
|---|---|---|
| **Preset lint 启动拒率** | 0%(无 lint) | ≥ 1%(故意 lint 拒启动验证) |
| **Orchestrator inject guard 覆盖率** | 12/16 注入点(75%) | 16/16(100%) |
| **Failure FSM 升级到 Final 的次数/月** | 0(2 个月没记录) | ≥ 1/loops(说明 3 级机制在工作) |
| **Wave 6 件套验收** | 0% | 100%(SC1~SC6) |
| **跨 4 块拼图的同根因 bug 复现** | merry-lotus→noble-peacock(2 次/天) | 0(同根因 1 月内 ≤ 1 次) |

---

## 8. 不做的事(再次 YAGNI)

为防止基座报告的方向"实施起来又变成无尽补丁",列不做的事:

- **不做 4 个独立 plan** — 拼图 1+2+3+4 合并为 **2 个独立 plan + 1 个跟随**(本 doc §5.3)
- **不做"通用 FSM 框架"** — FailureStateMachine 是 Ralph-specific,不抽象成 generic engine
- **不做"auto-generate obligation"** — 8 条 lint 规则已覆盖 95% 已知 obligation 需求,不再加 obligation 字段
- **不做"5 件套/6 件套叠加"** — supervisor 母舰 6 件套是顶层,本 doc 4 块拼图是平级,不做"supervisor + failure FSM + inject guard"三层嵌套
- **不动 preset 默认值** — 8 条 lint 全部为"违规拒启动",不改任何已有 preset 字段

---

## 9. 引用清单(本 doc 引用材料 + supervisor 母舰衔接)

### 母舰 / 子 doc(superseded_by 关系)
- `docs/brainstorms/2026-06-18-supervisor-wave-protocol-upgrade-requirements.md` — **6 件套母舰,本 doc 拼图 4 跟随**
- `docs/achieved/brainstorms/2026-06-17-ce-executor-flow-reliability-requirements.md` — 已被 supervisor 母舰 superseded;R-A4 partial wave + R-A5 degraded completion 与本 doc 拼图 2 衔接
- `docs/achieved/brainstorms/2026-06-17-ce-executor-step-handoff-requirements.md` — 已被 supervisor 母舰 superseded;R-A5 coordinator_hats 闭包与本 doc 拼图 1 Rule 1 衔接
- `docs/achieved/brainstorms/2026-06-17-wave-dimension-assignment-enforcement-requirements.md` — 已被 supervisor 母舰 superseded
- `docs/brainstorms/2026-06-18-recovery-escalation-routing-requirements.md` — 已被 supervisor 母舰 superseded;Soft/Hard/Final 路由表与本 doc 拼图 2 衔接
- `docs/brainstorms/2026-06-17-agent-recovery-mechanism-gaps-requirements.md` — 独立 doc;R-A1~A3 CLI precheck 与本 doc 拼图 3 衔接

### 关键诊断(本 doc 引用)
- `docs/report/2026-06-17-ce-executor-serial-merry-lotus-...` + `noble-peacock-...` — 同根因 2 次复现,**驱动本 doc 拼图 3 + 拼图 2 立项**
- `docs/achieved/report/2026-06-09-ce-executor-mechanism-vs-orchestration-diagnosis.md` — 4 处 preset 缺陷,O1~O6,**驱动本 doc 拼图 1 立项**
- `docs/report/2026-06-13-review-wave-no-spawn.md` — 7 个 wave 0 spawn,**驱动本 doc 拼图 4 supervisor R-B3 立项**

### 核心源码
- `crates/ralph-core/src/event_loop/mod.rs` — 拼图 3 16 个注入点散落位置
- `crates/ralph-core/src/event_loop/rejection.rs:358` — `build_task_resume_payload` 起点
- `crates/ralph-core/src/diagnosis/responder.rs` — 现有 Soft/Hard/Final 枚举
- `crates/ralph-core/src/wave_tracker.rs` — 拼图 4 supervisor 主战场
- `crates/ralph-cli/src/loop_runner/preset_lint_gate.rs` — 拼图 1 接入点

### 父 doc
- `docs/report/2026-06-18-003-base-stability-optimization-report.md` — 本 doc 的父 doc;基座不稳的 6 类根因 + 5 个方向

---

*本 doc 是基座报告 §3 方向 1+2+3+4 的实施展开。立场:不写代码,只给"4 块拼图怎么落地"——其中 3 块是新独立 plan(拼图 1 + 2 + 3),1 块是跟随 supervisor 母舰(拼图 4)。**与 supervisor 团队的对接动作**:认领 supervisor U5/U6(补偿 + 幂等),与本 doc 拼图 2 的 Final 升级共享 BDD 夹具。*
