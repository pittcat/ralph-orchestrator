---
title: 修复 ce-executor-serial 因 recovery_exhausted 提前终止导致评审链未启动的问题
type: fix
status: active
date: 2026-06-29
origin: docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-diagnosis.md
---

# 修复 ce-executor-serial 因 recovery_exhausted 提前终止导致评审链未启动的问题

## 概述

本次修复针对 Loop `primary-20260628-172725` 在 `ce-executor-serial` preset 下失败的根因：

- `validator` hat 在 `work.done` 发出后未在 30 秒内 activate，runtime 的 `HandoffTracker` 正确识别了 `stall_recovery`；
- 但 `missing_event_gate` 对同一事件进行二次检测，错误地把 retry_key 锚定到 `executor`；
- `stall_recovery` 与 `missing_event_gate` 两条 retry_key 互不感知，attempt 独立累加，8 个 iteration 内触发 `EscalationLevel::Final`；
- `recovery_exhausted` 直接硬退出，没有走 preset 设计的 `plan.blocked → REVIEW_COMPLETE(fail) → report.done(fail)` 终态链路；
- 同时 `task_id` 形态漂移、inline JSON 缺 `kind`、handoff 超时 30 秒硬编码等问题放大了上述级联失败。

本计划将所有修复拆分为**纯粹串行、绝对隔离、TDD 闭环**的独立 Unit。每个 Unit 必须先写测试、后写实现，测试只验证当前 Unit 的输入输出，完成后才能进入下一个 Unit。

---

## 问题框定

用户问题："编排机制没有按流程走，修复机制失效，搞乱之后修复机制又失效。"

经过诊断，本次失败并非 preset 编排定义错误，而是 RALPH 基座机制在以下 4 个环节同时失效：

1. **修复诊断去重失效**：同一 `work.done` 事件被 `stall_recovery` 和 `missing_event_gate` 重复记录，retry_key 错锚到 `executor`。
2. **task_id / loop_id 兜底失效**：`state_projector` 在 payload 不带 `loop_id` 时不从当前 loop marker 兜底注入，导致 `TaskWrongLoop` 反复触发。
3. **终止路径偏离预设**：`RecoveryExhausted` 直接 kill loop，没有 emit `plan.blocked`，导致 shipper/reporter 整段链路未启动。
4. **handoff 超时窗口过短**：30 秒硬编码远低于实测 `validator` 响应延迟（54 秒–9 分钟），造成大量伪 stall。

---

## 需求追溯

| ID | 需求 | 来源 |
|---|---|---|
| R1 | `stall_recovery` 与 `missing_event_gate` 在同一事件上不得重复生成 envelope，retry_key 必须共享 attempt 计数 | 诊断报告 §2 P0-1 |
| R2 | payload 不带 `loop_id` 时，`state_projector` 必须从当前 loop marker 注入兜底 `loop_id`，避免 `TaskWrongLoop {actual_loop: None}` | 诊断报告 §2 P0-2 |
| R3 | `RecoveryExhausted` 触发前必须先 emit `plan.blocked`，再走 preset 设计的失败终态链路 | 诊断报告 §2 P0-3 |
| R4 | `handoff_dispatch_timeout` 默认值必须能覆盖实测 validator 延迟，或支持 preset/配置覆盖 | 诊断报告 §2 P0-4 |
| R5 | `task.resume` 的 inline JSON payload 必须带 `kind` 字段，满足 drift 字段完整度阈值 | 诊断报告 §2 P1-2 |
| R6 | `from_key:...` legacy task 在 loop_scoped 校验下不得被 hard reject | 诊断报告 §2 P1-3 |
| R7 | `progress-steward` 必须在连续 stall 时通过 `loop.stalled` 业务事件被激活兜底 | 诊断报告 §2 P2-1 |
| R8 | executor 重发必须有收敛信号（重发上限 / 读取 task_store 指引） | 诊断报告 §2 P1-4 / P2-3 |

---

## 范围边界

### 本次必须完成

- 修复 `ralph-core` 基座机制中导致 `recovery_exhausted` 提前终止的 4 个 P0 根因。
- 修复 2 个 P1 级放大器（`kind` 字段缺失、`from_key:` hard reject）。
- 修复 1 个 P2 级兜底激活（`loop.stalled` 业务事件）。
- 所有修复配套单元测试，覆盖红→绿→重构。

### 明确不做的内容

- 不改动 `ce-executor-serial.yml` 的 10-hat 拓扑与 Phase Gate 设计。
- 不删除 `human.guidance` topic（该工作在 `docs/plans/2026-06-28-005-refactor-remove-human-guidance-topic-plan.md` 中处理）。
- 不重构 CLI precheck 全路径（仅对 `missing_event_gate` 与 `stall_recovery` 去重做最小修改）。
- 不引入新的外部消息通道（Slack/Telegram/Webhook）。

### 延后工作

- CLI 路径与 event_loop 路径 envelope schema 全面对齐：本次只做 `task.resume` 的 `kind` 字段，其余 schema 对齐放到 `docs/plans/2026-06-28-005-refactor-remove-human-guidance-topic-plan.md` 后续迭代。
- `executor` prompt 中"重发前先读 task_store"的完整指引：本次在 preset 中追加 HARD RULE，长期由 `docs/plans/2026-06-28-003-feat-ralph-tools-pitfalls-and-injection-hardening-plan.md` 落地。

---

## 上下文与研究

### 相关代码与模式

- `crates/ralph-core/src/event_loop/mod.rs:6080-6225` — `HandoffTracker::expired()` 处理与 `task.resume` 合成。
- `crates/ralph-core/src/event_loop/mod.rs:6104-6112` — inline JSON 拼 `task.resume` payload，缺 `kind`。
- `crates/ralph-core/src/state_projector/task.rs:76-88` — `task_id` 回退与 `loop_id` 注入逻辑。
- `crates/ralph-core/src/state_projector/mod.rs:133-173` — `ProjectionContext` 定义。
- `crates/ralph-core/src/execution_contract.rs:570-603` — `loop_scoped` 校验与 `TaskWrongLoop` hard reject。
- `crates/ralph-core/src/drift/engine.rs:392-406` — `check_termination_hint` 直接返回 `RecoveryExhausted`。
- `crates/ralph-core/src/diagnosis/responder.rs:880-919` — `classify` 中 `over_threshold + over_window → Final`。
- `crates/ralph-cli/src/loop_runner/hard_gate.rs:912-950` — `missing_event_gate` envelope 注入。
- `crates/ralph-core/src/config/workflow_contract.rs:45-50` — handoff timeout 常量。
- `crates/ralph-core/src/workflow_contract/handoff_tracker.rs:199-237` — `expired()` 与 `safe_target` 计算。
- `presets/en/ce-executor-serial.yml:255-300, 868-873, 2549-2645` — 契约、失败路径、progress-steward 兜底。

### 历史修复参考

- `docs/plans/2026-06-28-004-fix-ce-executor-serial-primary-diagnosis-plan.md`（已落地但本次仍复发）。
- `docs/plans/2026-06-28-005-refactor-remove-human-guidance-topic-plan.md`（长期移除 `human.guidance`）。
- `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md`（review terminal 三道防线）。

---

## 关键技术决策

1. **P0-1 去重策略**：不合并两个 source，而是在 `missing_event_gate` 注入前检查同事件是否已有 `stall_recovery` envelope；若存在，直接跳过。保持 `stall_recovery` 作为真实问题来源的权威性。
2. **P0-2 loop_id 兜底**：在 `ProjectionContext` 中新增 `current_loop_id` 字段，优先取 payload，其次取当前 loop marker，与 `execution_contract` 的 `current_loop_id` 同源。
3. **P0-3 终止路径**：在 `EscalationLevel::Final` 路径上增加一层 bridge：先 emit `plan.blocked(reason="recovery_exhausted:<retry_key>")` 业务事件，等待 shipper/reporter 处理完成后再标记 `RecoveryExhausted`。terminal-reason 仍保留，但业务事件链路不再断裂。
4. **P0-4 超时窗口**：将默认值从 30 秒提高到 600 秒（覆盖实测 540 秒上限 + 60 秒 buffer），同时上限从 120 秒提高到 1800 秒，允许 preset/ralph.yml 覆盖。
5. **P1-2 `kind` 字段**：将 inline JSON 替换为 `enrich_task_resume_payload_full`，显式传入 `RejectionKind::StallRecovery`，确保 `kind=handoff_dispatch_timeout`。
6. **P1-3 `from_key:` 防御**：`validate_task` 在 `loop_id=None` 但 task key 包含当前 loop_id 前缀时，视为同 loop 任务并放行（warn 级别），不 hard reject。
7. **P2-1 `loop.stalled` 兜底**：当同一 consumer 连续 stall 2 次时，额外 emit `loop.stalled` 业务事件，触发 `progress-steward` 救援。

---

## 待明确问题

### 规划中已解决

- **Q1：是否直接删除 `missing_event_gate`？** 否，仅做同事件去重，保留其独立检测非 stall 类 missing event 的能力。
- **Q2：`plan.blocked` 由谁 emit？** 由 `drift/engine.rs` 在 `EscalationLevel::Final` 时通过 bus 主动 emit，coordinator 订阅并正常转 shipper。
- **Q3：timeout 调高是否会掩盖真 stall？** 通过 P2-1 `loop.stalled` 兜底与 P0-1 去重保证，不会；真长时间无响应仍会被 progress-steward 处理。

### 实现中再验证

- 具体 `current_loop_id` 在 `ProjectionContext` 构造点的传参方式（需查看 3-5 个调用点）。
- `plan.blocked` emit 后是否需要等待一个 iteration 让 shipper 消费，还是同步处理。

---

## 高层技术设计

> 本图仅为方向性说明，不是实现规范。

```text
work.done (executor)
    │
    ▼
HandoffTracker::expired() ──► stall_recovery envelope (source_hat=validator)
    │                              │
    │                              ▼
    │                    task.resume → validator (带 kind)
    │
    ▼
missing_event_gate ──[P0-1 去重]──► 若已有 stall_recovery envelope，跳过
    │
    ▼
responder.classify() ──[P0-1 共享 attempt]──► 不重复计数
    │
    ▼
EscalationLevel::Final
    │
    ▼
check_termination_hint() ──[P0-3]──► emit plan.blocked
    │                                   │
    ▼                                   ▼
RecoveryExhausted (保留)         shipper → REVIEW_COMPLETE(fail)
                                          │
                                          ▼
                                     reporter → report.done(fail)
```

---

## 实现单元

> **执行纪律**：每个 Unit 必须严格串行。Unit N 的测试未全绿之前，不允许开始 Unit N+1 的编码。每个 Unit 的测试只验证本 Unit 引入的行为，不写跨 Unit 集成测试。

---

- [ ] U1. **HandoffTracker 超时默认值调整到 600 秒并支持配置覆盖**

**目标**：消除因 30 秒硬编码过短导致的伪 `handoff_dispatch_timeout` stall。

**需求**：R4

**依赖**：无

**文件**：
- 修改：`crates/ralph-core/src/config/workflow_contract.rs`
- 修改：`crates/ralph-core/src/workflow_contract/handoff_tracker.rs`（确认配置传入路径）
- 测试：`crates/ralph-core/src/config/workflow_contract.rs`（新增）或 `crates/ralph-core/src/workflow_contract/handoff_tracker.rs`

**方法**：
- 将 `HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS` 从 `30` 改为 `600`。
- 将 `HANDOFF_DISPATCH_TIMEOUT_MAX_SECONDS` 从 `120` 改为 `1800`。
- 确认 `WorkflowContractConfig` 已暴露 `handoff_dispatch_timeout_seconds` 字段，允许 `ralph.yml` / preset 覆盖。

**测试场景**：
- Happy path：使用默认配置构造 `HandoffTracker`，在 599 秒时调用 `expired()` 返回空，601 秒时返回 escalation。
- Edge case：配置值为 0 时按 1 秒兜底（或按现有 fallback 行为）。
- Error path：配置值超过 `MAX` 时按 `MAX` 截断。

**验收标准**：
- 单元测试覆盖默认 600 秒与配置覆盖两条路径。
- `cargo nextest run -p ralph-core -- handoff_tracker` 全绿。

---

- [ ] U2. **ProjectionContext 增加 current_loop_id 字段并在 task 投影时兜底注入**

**目标**：修复 `task_id` / `loop_id` 缺失时投影出的 task 永久 `loop_id=None`，导致 `TaskWrongLoop` 反复触发的问题。

**需求**：R2

**依赖**：无（与 U1 完全隔离，不依赖 timeout 改动）

**文件**：
- 修改：`crates/ralph-core/src/state_projector/mod.rs`
- 修改：`crates/ralph-core/src/state_projector/task.rs`
- 测试：`crates/ralph-core/src/state_projector/task.rs`（新增 `#[cfg(test)]` 模块）

**方法**：
- 在 `ProjectionContext` 中新增 `pub current_loop_id: Option<String>`。
- 在 `project_ensure_task` 中，优先使用 `ctx_loop_id(payload)`；若 payload 无 `loop_id` 但 `ctx.current_loop_id` 有值，则调用 `task.with_loop_id(Some(ctx.current_loop_id.clone()))`。
- 查找所有构造 `ProjectionContext` 的调用点，传入当前 loop_id（优先使用已有的 `current_loop_id_for_contract()` 等价来源）。

**测试场景**：
- Happy path：payload 带 `loop_id=loop-A`，投影结果 `loop_id=loop-A`。
- Edge case：payload 不带 `loop_id` 但 `ctx.current_loop_id=loop-A`，投影结果 `loop_id=loop-A`。
- Edge case：payload 不带 `loop_id` 且 `ctx.current_loop_id=None`，投影结果 `loop_id=None`（保留旧行为，不破坏非 loop_scoped preset）。

**验收标准**：
- 新增 3 个单元测试全绿。
- 不破坏 `state_projector` 现有 150+ 测试。

---

- [ ] U3. **execution_contract 对同 loop legacy task 放行而非 hard reject**

**目标**：作为 U2 的防御层，当 `loop_id=None` 但 task key 包含当前 loop_id 前缀时，不触发 `TaskWrongLoop`。

**需求**：R6

**依赖**：U2（U3 的测试会构造 `loop_id=None` 但 key 含 loop_id 的任务；U2 是更根本的修复，U3 是兜底防御）

**文件**：
- 修改：`crates/ralph-core/src/execution_contract.rs`
- 测试：`crates/ralph-core/src/execution_contract.rs`（新增）

**方法**：
- 在 `validate_task` 的 `loop_scoped` 分支中，当 `task.loop_id == None` 时，检查 `task.key` 是否包含 `current_loop_id`。
- 若包含，返回 `None`（接受），并通过 `tracing::warn!` 记录一次防御性放行。
- 若不包含，保留原有 hard reject。

**测试场景**：
- Happy path：`task.loop_id=None`，`task.key="from_key:loop-A:..."`，`current_loop_id="loop-A"`，校验通过。
- Error path：`task.loop_id=None`，`task.key` 不含当前 loop_id，返回 `TaskWrongLoop {actual_loop: None}`。
- Edge case：`task.loop_id=Some("loop-B")`，`current_loop_id="loop-A"`，仍返回 `TaskWrongLoop {actual_loop: Some("loop-B")}`。

**验收标准**：
- 新增测试全绿。
- 现有 `execution_contract` 测试无回归。

---

- [ ] U4. **handoff_dispatch_timeout 路径使用 enrich_task_resume_payload_full 补 kind 字段**

**目标**：消除 inline JSON 导致 `task.resume` 缺 `kind` 字段、触发 drift 字段完整度告警的问题。

**需求**：R5

**依赖**：无（与 U1-U3 隔离）

**文件**：
- 修改：`crates/ralph-core/src/event_loop/mod.rs`
- 测试：`crates/ralph-core/src/event_loop/mod.rs`（已有测试模块）或新增 `crates/ralph-core/src/event_loop/stage_resume_payload_tests.rs`

**方法**：
- 将 `event_loop/mod.rs:6104-6112` 的 inline `serde_json::json!({...})` 替换为调用 `crate::event_loop::rejection::enrich_task_resume_payload_full`。
- 参数：`free_form_message` 为原 message；`reason_hint="handoff_dispatch_timeout"`；`target_hat=Some(&esc.safe_target)`；`stage=Some(RejectionStage::StallNoEvents)`；`kind=Some(RejectionKind::StallRecovery)`；`allowed_topics` 为 safe_target hat 的 publishes 列表。
- 保留 payload 中原有的 `topic`、`consumer`、`event_id`、`safe_target`、`details` 等字段（`enrich_task_resume_payload_full` 可扩展或在调用后 merge）。

**测试场景**：
- Happy path：触发 handoff escalation 后，生成的 `task.resume` payload 包含 `kind="handoff_dispatch_timeout"`。
- Edge case：payload 同时包含 `reason`、`target_hat`、`message`、`allowed_topics`。
- Error path：若 `allowed_topics` 为空，payload 仍包含 `kind`。

**验收标准**：
- 新增单元测试解析 payload 并断言 `kind` 字段存在且值正确。
- 现有 event_loop 模块测试无回归。

---

- [ ] U5. **missing_event_gate 注入前检查同事件 stall_recovery envelope 并跳过**

**目标**：消除同一 `work.done` 事件被 `stall_recovery` 和 `missing_event_gate` 双发 envelope 的问题。

**需求**：R1

**依赖**：U1、U4（U1 减少伪 stall 源，U4 保证 `task.resume` 带 kind；U5 自身是独立去重逻辑）

**文件**：
- 修改：`crates/ralph-cli/src/loop_runner/hard_gate.rs`
- 测试：`crates/ralph-cli/src/loop_runner/hard_gate.rs`（新增测试）

**方法**：
- 在 `inject_missing_event_hard_gate_guidance_with_triggers` 函数入口（`hard_gate.rs:912` 附近）增加 guard：
  - 构造候选 `stall_recovery` retry_key：`stall_recovery:{source_hat}:{topic}:handoff_dispatch_timeout:*`。
  - 若 `event_loop.state().recovery_responder.state` 中已存在该 key，则直接 `return`，不写 `MissingEventGate` envelope。
- 仅在 `reason_code="missing_event"` 且 `topic` 属于 handoff 路径时生效，避免误伤普通 missing event。

**测试场景**：
- Happy path：已有 `stall_recovery:validator:work.done:handoff_dispatch_timeout:*` envelope 时，`missing_event_gate` 不再写入新 envelope。
- Edge path：已有 `stall_recovery` 但 reason_code 不是 `handoff_dispatch_timeout` 时，正常写入 `missing_event_gate`。
- Error path：无 `stall_recovery` envelope 时，`missing_event_gate` 正常写入。

**验收标准**：
- 新增单元测试全绿。
- 不破坏 `ralph-cli` 现有测试（注意 `ralph-cli` 全包串行）。

---

- [ ] U6. **responder.classify 共享 stall_recovery 与 missing_event_gate 的 attempt 计数**

**目标**：即使去重 guard 漏过（例如跨 iteration 写入），两个同源 retry_key 也不得独立累加 attempt。

**需求**：R1

**依赖**：U5

**文件**：
- 修改：`crates/ralph-core/src/diagnosis/responder.rs`
- 测试：`crates/ralph-core/src/diagnosis/responder.rs`（新增）

**方法**：
- 在 `classify` 函数中，当 `retry_key` 前缀为 `missing_event_gate:{hat}:{topic}:missing_event:*` 时，额外查找 `stall_recovery:{hat}:{topic}:handoff_dispatch_timeout:*` 的 attempt_count，取两者最大值作为当前 attempt_count。
- 反向同理：当处理 `stall_recovery` retry_key 时，也考虑同 topic 的 `missing_event_gate` attempt_count。

**测试场景**：
- Happy path：`stall_recovery` attempt=2，`missing_event_gate` attempt=1，classify 结果按 attempt=2 计算，不触发 Final。
- Edge case：两者均未达到 threshold，返回 Soft。
- Error path：合并后 attempt=2 且 over_window，返回 Final。

**验收标准**：
- 新增测试覆盖同 topic 双 source 的 attempt 合并。
- 现有 responder 测试无回归。

---

- [ ] U7. **RecoveryExhausted 路径先 emit plan.blocked 再走终态链路**

**目标**：让 loop 在 recovery 窗口耗尽时仍走 preset 设计的失败路径，至少生成 `REVIEW_COMPLETE(fail)` 和 `report.done(fail)`。

**需求**：R3

**依赖**：U5、U6（确保 retry_key 不再误判 Final 后，U7 才会在真 Final 时触发）

**文件**：
- 修改：`crates/ralph-core/src/drift/engine.rs`
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（termination 路径）
- 测试：`crates/ralph-core/src/drift/engine.rs`（新增）

**方法**：
- 在 `check_termination_hint` 的 `EscalationLevel::Final` 分支中：
  1. 若 `hint.retry_key` 对应 `safe_target=true` 且存在可路由的 coordinator hat，构造并 emit `plan.blocked` 业务事件到 bus。
  2. 设置一个一次性 flag `pending_recovery_exhausted`，让 event_loop 在下一个 iteration 或当前 iteration 末尾检查 shipper/reporter 是否已处理 `plan.blocked`。
  3. 处理完成后返回 `TerminationReason::RecoveryExhausted`。
- 简化实现：在当前 iteration 内同步 emit `plan.blocked`，然后允许 event_loop 继续一个短暂的处理周期，再终止。

**测试场景**：
- Happy path：`EscalationLevel::Final` 触发后，bus 中出现 `plan.blocked` 事件，payload 包含 reason="recovery_exhausted:<retry_key>"。
- Edge case：`safe_target=false` 时，直接返回 `RecoveryExhausted`，不 emit `plan.blocked`（无 hat 可接收）。
- Error path：`coordinator` 订阅 `plan.blocked` 并转发给 `shipper` 的集成路径在 BDD 中验证（不属于本 Unit 单元测试范围，仅验证 emit 行为）。

**验收标准**：
- 单元测试断言 `check_termination_hint` 返回 `RecoveryExhausted` 前已 emit `plan.blocked`。
- 不破坏现有 `drift` 模块测试。

---

- [ ] U8. **连续 stall 时 emit loop.stalled 业务事件激活 progress-steward**

**目标**：让 `progress-steward` 兜底在连续 stall 时真正被唤醒，避免 loop 空转。

**需求**：R7

**依赖**：U1、U4

**文件**：
- 修改：`crates/ralph-core/src/event_loop/mod.rs`
- 测试：`crates/ralph-core/src/event_loop/mod.rs`（新增）

**方法**：
- 在 `handoff_tracker.expired()` 处理循环中，统计同一 `consumer` 在当前 session 中的累计 stall 次数。
- 当累计次数 >= 2 时，在写入 `stall_recovery` envelope 后，额外 emit `loop.stalled(reason="<retry_key>")` 业务事件到 bus。
- `progress-steward` 已在 preset 中订阅 `loop.stalled`，无需修改 preset。

**测试场景**：
- Happy path：同一 consumer 第一次 stall 不 emit `loop.stalled`，第二次 stall emit。
- Edge case：不同 consumer 的 stall 分别计数。
- Error path：`progress-steward` 未在 preset 中定义时，事件被正常路由到 bus 但无消费者（不 crash）。

**验收标准**：
- 单元测试断言第二次 stall 后 bus 中出现 `loop.stalled` 事件。
- 不破坏现有 event_loop 测试。

---

- [ ] U9. **在 ce-executor-serial preset 中追加 executor 重发收敛指引**

**目标**：通过 preset 指令减少 executor 在收到 `task.resume` 后反复重发同一 `work.done` 的噪声。

**需求**：R8

**依赖**：无（纯 preset 文本修改）

**文件**：
- 修改：`presets/en/ce-executor-serial.yml`
- 测试：`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`

**方法**：
- 在 `presets/en/ce-executor-serial.yml` 的 executor instructions 段追加 "Re-emission Protocol (HARD RULE)"：
  - 重发前读取 `.ralph/agent/tasks.jsonl` 获取当前 task_id。
  - 使用 task_store 中的 task_id，禁止 `""` 和 `from_key:...`。
  - 同 task_key 在一个 iteration 窗口内最多重发 2 次；超过则 emit `work.failed(reason="re-emit_exhausted")`。

**测试场景**：
- 纯配置改动，无新增单元测试。
- 验证：`preset_lint` 通过，`cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded` 通过。

**验收标准**：
- `preset_lint` 与 SSOT byte-equality 测试全绿。

---

- [ ] U10. **全量回归测试与 e2e 验证**

**目标**：确认 9 个 Unit 落地后，`ce-executor-serial` 不再因 recovery_exhausted 提前终止。

**需求**：R1-R8

**依赖**：U1-U9

**文件**：
- 运行：`./scripts/run-tests.sh`
- 运行：`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
- 运行：`cargo nextest run -p ralph-core -- preset_lint`
- 可选：跑一次 `ralph-e2e` mock loop 验证 `REVIEW_COMPLETE` 终态出现。

**方法**：
- 依次跑单元测试、integration 测试、BDD scenarios、preset_lint、SSOT byte-equality。
- 若 `ralph-cli` 测试 flake，按 AGENTS.md 走 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底。

**测试场景**：
- Integration：一个最小 ce-executor-serial mock 事件流中，`work.done → test.passed → review.start → review.complete → plan.complete → REVIEW_COMPLETE → report.done → LOOP_COMPLETE` 链路完整。
- Integration（失败路径）：模拟 validator 连续不响应，最终触发 `plan.blocked → REVIEW_COMPLETE(fail) → report.done(fail)`，而非 `recovery_exhausted` 直接终止。

**验收标准**：
- `./scripts/run-tests.sh` 全绿（或 serial fallback 绿）。
- E2E mock 验证终态事件存在。

---

## 系统级影响

- **交互图**：修改涉及 `event_loop`、`state_projector`、`execution_contract`、`diagnosis/responder`、`drift/engine`、`workflow_contract`、`loop_runner/hard_gate`、`presets` 8 个模块。
- **错误传播**：`plan.blocked` emit 后，coordinator → shipper → reporter 的现有订阅路径自动承接，不引入新错误通道。
- **状态生命周期风险**：`ProjectionContext` 新增字段需保证所有构造点初始化，否则可能 panic 或缺 loop_id。
- **API 表面对等性**：timeout 配置字段属于 `WorkflowContractConfig`，已存在序列化/反序列化路径，改动对外部配置兼容。
- **未变更不变量**：不修改 `Event`、`HatId`、`Topic`、`EventBus` 等基座类型；不修改 preset 10-hat 拓扑。

---

## 风险与依赖

| 风险 | 缓解 |
|---|---|
| timeout 调到 600 秒后，真 dispatch 失败时被掩盖 | 保留 `loop.stalled` 兜底（U8）与 `RecoveryExhausted` 终态（U7），真失败仍会终止 |
| `plan.blocked` 同步 emit 后 shipper 未及时处理 | 在 event_loop termination 路径中增加一个 iteration 等待窗口，或同步调用一次 event processing |
| U2 新增 `current_loop_id` 字段导致构造点编译失败 | 优先使用已有 `current_loop_id_for_contract()` 来源，逐个调用点补字段 |
| `ralph-cli` 测试串行且时间敏感，可能 flake | 严格按 AGENTS.md 使用 `cargo nextest run -p ralph-cli --bin ralph -- <subset>`，全量 fallback 走 `RALPH_BASELINE_SERIAL=1` |
| preset 文本修改触发 lint fail-closed | 改完后立即跑 `preset_lint` 与 SSOT byte-equality |

---

## 文档与操作说明

- 更新 `crates/ralph-core/data/ralph-tools.md` 中关于 `ralph run`、`ralph loops`、`ralph diagnose` 的描述（若涉及命令行为变更）。
- 更新本计划对应的 `AGENTS.md` / `CLAUDE.md` 中 Presets & Hats 段（若新增 preset 配置字段）。
- 运行 `scripts/check-cli-doc-drift.sh` 检查文档漂移。

---

## 来源与参考

- **源诊断报告**：`docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-diagnosis.md`
- 已落地但未覆盖本次场景的计划：`docs/plans/2026-06-28-004-fix-ce-executor-serial-primary-diagnosis-plan.md`
- 长期移除 `human.guidance`：`docs/plans/2026-06-28-005-refactor-remove-human-guidance-topic-plan.md`
- 相关源码：
  - `crates/ralph-core/src/event_loop/mod.rs:6080-6225`
  - `crates/ralph-core/src/state_projector/task.rs:76-88`
  - `crates/ralph-core/src/state_projector/mod.rs:133-173`
  - `crates/ralph-core/src/execution_contract.rs:570-603`
  - `crates/ralph-core/src/drift/engine.rs:392-406`
  - `crates/ralph-core/src/diagnosis/responder.rs:880-919`
  - `crates/ralph-cli/src/loop_runner/hard_gate.rs:912-950`
  - `crates/ralph-core/src/config/workflow_contract.rs:45-50`
  - `crates/ralph-core/src/workflow_contract/handoff_tracker.rs:199-237`
  - `presets/en/ce-executor-serial.yml:255-300, 868-873, 2549-2645`
