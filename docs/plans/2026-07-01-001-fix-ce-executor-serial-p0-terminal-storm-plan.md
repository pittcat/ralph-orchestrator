---
title: fix: 加固 ce-executor-serial 终态事件守卫与 fix-unit 收尾发射
type: fix
status: active
date: 2026-07-01
origin: docs/report/2026-07-01-ce-executor-serial-primary-20260630-175407-diagnosis.md
---

# fix: 加固 ce-executor-serial 终态事件守卫与 fix-unit 收尾发射

## 概述

`ce-executor-serial` preset 的运行 `primary-20260630-175407` 在业务侧已经闭环（26/26 pytest 通过、commit 落盘），但随后出现了 `LOOP_COMPLETE` 之后的二次事件风暴：第二份 `REVIEW_COMPLETE`、两份重复的 `report.done`、以及第二份 `LOOP_COMPLETE`。与此同时，最后一个 fix-unit（`fix-02`）始终没有发出 `plan.complete`，而是把 `work.ready(fix-02)` 重发了 3 次，最终降级为 `plan.blocked`。第三个缺陷是 `fix-02` 的 `task_id` 携带了 2025 年的时间戳。

本计划修复三个 P0 根因：

1. **P0-1** `completion_after_terminal` 只在同一个 `process_output` batch 内生效；`LOOP_COMPLETE` 被 honor 后，后续 activation 不再受保护。
2. **P0-2** isolated 模式“每轮一个业务事件”的预算会把 `plan.complete` 这类终态事件静默丢弃——当同轮先出现一个 stray `work.ready` 时，`plan.complete` 就会因为预算耗尽而被丢。
3. **P0-3** fix-unit 的 `task_id` shape-1 校验只检查格式，不检查时间戳，导致手写的 2025 年时间戳通过校验。

修复以机制侧为主：加固 isolated 预算的终态事件优先级、让 `completion_honored` 守卫跨 activation 持久化、让 `CoordinatorDecisionGateStage` 强制最后一个 fix-unit 走 `plan.complete` 终态、并对 fix-unit `task_id` 增加时间戳窗口校验。preset 编排与 `plan.blocked.reason` schema 作为第二道防线同步收紧。

---

## 问题框架

`ce-executor-serial` 在 isolated 模式下运行 10 个 hat。isolated 模式规定每个 hat 每轮 activation 只能发射 **一个** 非 wave 业务事件，多出的业务事件会被丢弃。这个预算在普通工作中是正确的，但在工作流终态时会致命：同一轮中可能需要从 `work.ready` 语义切换到 `plan.complete`（最后一个 fix-unit），或者 `report.done` 与 `LOOP_COMPLETE` 紧密相邻。

在 `primary-20260630-175407` 中：

- 18:56:30 `LOOP_COMPLETE` 被 honor。
- 18:57:35 重复的 `report.done` 进入事件总线，18:57:37 出现重复 `LOOP_COMPLETE`，18:59:25 出现矛盾的 `REVIEW_COMPLETE(fail)`。
- 更早之前，18:36:13 `test.passed(fix-02)` 落地后，coordinator 在 18:39:18 和 18:48:57 重发了 `work.ready(fix-02)` 而非 `plan.complete`。isolated 预算把尝试发出的 `plan.complete` 全部丢弃，循环最终 escalated 到 `plan.blocked(reason=progress_md_validation_stale)`。
- `fix-02` 的 `work.ready` 携带 `task_id=task-1751414400-a1b2`，其中 `1751414400` 对应 2025-07-01，不在本次运行时间窗口内。

责任分界：约 70% 是机制侧问题（预算优先级、跨 batch 完成守卫、task-id fail-closed），20% 是 preset 编排问题（prompt 分支与 schema），10% 是 agent 手写旧时间戳的产物问题。

---

## 需求追溯

- **R1** 一旦 `LOOP_COMPLETE` 被 honor，任何后续 activation 的业务事件（`report.done`、`REVIEW_COMPLETE`、`work.ready`、`plan.complete`、`plan.blocked` 等）都不得进入事件总线。
- **R2** 第一次 `LOOP_COMPLETE` 被 honor 后，后续重复的 `LOOP_COMPLETE` 必须被拒绝。
- **R3** 最后一个 fix-unit 的 `test.passed` 落地后，必须且只能发出并接纳一个 `plan.complete`；同一 fix-unit 的 `work.ready` 重发不得占用每轮业务事件预算。
- **R4** fix-unit 的 `task_id` 当嵌入时间戳超出当前 loop 时间窗口时必须被拒绝。
- **R5** preset 指令必须明确指引 coordinator 在最后一个 fix-unit 后只 emit `plan.complete`，并禁止手写 `task_id` 时间戳。
- **R6** 注入给 coordinator 的上下文（`task.resume` / context payload）必须显式携带当前所处阶段与应该发射的事件类型，让 agent 不再依赖自然语言判断。

---

## 范围边界

- **在范围内**：isolated 模式预算、完成守卫持久化、`CoordinatorDecisionGateStage` 终态改写、fix-unit task-id 校验、`ce-executor-serial` preset 指令与 schema。
- **不在范围内**：更广泛的 dedup 时序问题（P1-1 prompt 文本反馈）、per-hat 事件切片 writer（P2-2 诊断面缺失）、dimension-reviewer 越权写文件的 escalation（§G）、其他 preset（如 `ce-executor-lite`）。
- **推迟到后续工作**：isolated 预算丢弃后的 typed `task.resume` 反馈；`plan.blocked.reason` 的 `recoverable` 结构化字段。

---

## 背景与研究

### 相关代码与模式

- `crates/ralph-core/src/event_loop/mod.rs` 中 isolated 模式每轮业务事件预算逻辑：先接纳第一个非 wave 业务事件，其余丢弃，并在 commit `62a40b41` 中增加了 `task.resume` 反馈注入。
- `crates/ralph-core/src/event_loop/mod.rs` 中 `LOOP_COMPLETE` 的 honor 逻辑：只对同 batch 中位于 `LOOP_COMPLETE` 之后的事件调用 `check_completion_guard`。
- `crates/ralph-core/src/event_policy.rs` 的 `PolicyRuntimeState` 在内存中维护 `completion_honored`，但没有持久化，因此无法跨 activation 拦截。
- `crates/ralph-core/src/event_loop/stages/coordinator_decision_gate_stage.rs` 在 `step.last_in_phase` 为 true 时把 `work.ready` 改写为 `plan.complete`，但只改写 topic，不强制 payload 字段。
- `crates/ralph-core/src/state_projector/task.rs` 的 `is_valid_task_id_format` 对 shape-1 `task-{ts}-{4hex}` 只校验格式，不校验时间戳。
- `presets/en/ce-executor-serial.yml` 第 810–892 行包含 coordinator fix-unit 推进指令；第 352–355 行配置 `completion_after_terminal`。
- `presets/schemas/ce-executor-serial.yml` 第 283–298 行把 `plan.blocked.reason` 定义为无限制字符串。

### 机构经验

- 2026-06-24 的 `primary-20260624-153613` 已出现同样的 `2x REVIEW_COMPLETE / 3x report.done / 2x LOOP_COMPLETE` 风暴，说明同 batch guard 不足。
- commit `62a40b41` 增加了业务事件丢弃后的 `task.resume` 反馈，但没有给终态事件更高优先级。
- `docs/plans/2026-06-30-001-fix-ce-executor-serial-fix-unit-terminal-p0-plan.md` 已处理相关 fix-unit 终态发射问题，但未关闭跨 batch 完成守卫缺口。

---

## 关键技术决策

- **isolated 预算终态优先（U1）**：在计算每轮业务事件预算时，终态事件（`LOOP_COMPLETE`、`plan.complete`、`plan.blocked`、`REVIEW_COMPLETE`、`report.done`）应优先于非终态业务事件。这样 stray `work.ready` 不会吞掉本应留给 `plan.complete` 的槽位。这一层是**通用兜底策略**，不依赖对 step 语义的识别。
- **语义层强制单终态发射（U3）**：`CoordinatorDecisionGateStage` 负责识别最后一个 fix-unit 并将 `work.ready` 改写为 `plan.complete`，同时保证改写后的 payload 字段完整。U3 是**语义正确性保证**，它把“应该发什么事件”从 agent 推断转移到运行时强制改写；U1 则是**预算层保护**，确保即使 agent 多发了非终态事件，终态事件仍有槽位。U3 不重复 U1 的预算职责，只负责改写和单终态语义。
- **持久化的 completion-honored 守卫（U2）**：把 `completion_honored` 从每 activation 的 `PolicyRuntimeState` 移动到持久 loop state（或在 `LOOP_COMPLETE` 被接纳时持久化快照），使 `check_completion_honored` 能拒绝后续 batch 中的终态后业务事件。
- **fix-unit task id 时间戳窗口 fail-closed（U4）**：shape-1 fix-unit id 的时间戳必须落在 `[loop_start - 60s, now]` 窗口内，过期或未来时间戳在投影边界被拒绝。
- **coordinator payload 阶段标记（U6）**：在 runner 注入给 coordinator 的 `task.resume` / context payload 中增加机器可读的 `expected_event`、`phase`、`last_in_phase`、`completed_steps` 字段，直接告诉 agent 本轮应该 emit 什么事件。

---

## 待解决问题

### 规划中已解决

- **Q**: 完成守卫应该在 isolated 预算层拦截，还是在 `event_policy` 层拦截？  
  **A**: 两者都拦截。isolated 预算是最早的关卡，防止事件占用唯一槽位；`event_policy` 是权威的跨 activation 关卡，必须持久化 honor 状态。

- **Q**: `CoordinatorDecisionGateStage` 在 isolated 预算之前还是之后运行？  
  **A**: 它属于 emit-stage pipeline，保持现有顺序，但让 stage 具备终态感知能力，在预算决策前改写/丢弃冲突事件。

### 推迟到实现阶段

- isolated 预算中“终态事件”的具体列表（实现时应从 `EventPolicyConfig.terminal_topics` 与 completion promise 派生）。
- `report.done` 是只由持久守卫处理，还是同时加入 preset 的 `business_after_completion` reject 列表。
- coordinator payload 阶段标记的最佳注入位置：是统一写入 `task.resume` payload，还是写入 `context.md` 的 JSON 块？实现阶段根据现有 `enrich_task_resume_payload` 与 `context.md` 生成路径决定。若实现时发现同时注入两者更简单，允许双写，但须保证字段语义一致。

---

## 实现单元

- [ ] U1. **isolated 模式每轮预算优先接纳终态事件**

**目标**：当非终态业务事件先占用了每轮唯一槽位时，终态事件仍能进入总线。U1 是**通用预算层策略**，不识别 step 语义，只根据 topic 判断事件是否为终态。

**需求**：R1、R3

**依赖**：无

**文件**：
- 修改：`crates/ralph-core/src/event_loop/mod.rs`
- 测试：`crates/ralph-core/tests/scenarios.rs`（新增场景）

**做法**：
- 在 isolated 模式准入循环中，根据 `EventPolicyConfig.terminal_topics` 与配置的 completion promise 判断当前事件是否为终态事件。
- 若已接纳非终态业务事件而当前事件为终态，则**在预算决策前对该轮 batch 进行预扫描/排序**，确保终态事件优先获得槽位；不强制要求在线驱逐已接纳事件。
- 发生优先级调整时发布诊断事件。
- 保留现有 wave-group 与 dual-publish-handoff 语义。
- **边界**：U1 只决定哪些事件被预算接纳；step 语义改写（`work.ready` → `plan.complete`）由 U3 负责。

**遵循模式**：
- 现有 `should_admit` 逻辑。
- 现有 `event.isolation.boundary_violation` 诊断事件。

**测试场景**：
- 正常路径：一轮中 emit `work.ready` 后 emit `plan.complete`，最终只有 `plan.complete` 被接纳。
- 边界情况：一轮只 emit `work.ready`，正常接纳。
- 错误路径：一轮 emit `work.ready`、重复 `work.ready`、`plan.complete`；重复被丢弃，`plan.complete` 被接纳。
- 集成：用 fix-02 终态序列的场景回放，断言 `plan.complete` 被接纳、`work.ready` 重复被拒绝。

**验收标准**：
- 新增 BDD 场景在 `cargo nextest run -p ralph-core --test scenarios` 下通过。
- 现有 isolated 模式测试保持通过。

---

- [ ] U2. **持久化 completion_honored 守卫到跨 activation**

**目标**：阻止 `LOOP_COMPLETE` 被 honor 后，后续 batch 中的业务事件进入总线。

**需求**：R1、R2

**依赖**：U1

**文件**：
- 修改：`crates/ralph-core/src/event_policy.rs`、`crates/ralph-core/src/event_loop/mod.rs`、`crates/ralph-core/src/event_loop/loop_state.rs`
- 测试：`crates/ralph-core/tests/scenarios/completion_honored_cross_batch.yml`（新增）

**做法**：
- 在 loop state（如 `LoopState` 或 `StateLedger`）中增加持久化的 `completion_honored: bool` 标志，使其跨 `process_output` 调用存活；**loop 重启/重新水合时显式清零**。
- 在 `LOOP_COMPLETE` 被接纳时设置该持久标志。
- 在主事件校验循环中，对每个事件都调用 `check_completion_honored`，使用持久标志而不仅是同 batch 快速路径。
- 保留同 batch 内 `completion_seen_in_batch` 快速路径用于诊断，但当持久标志已设置时所有事件都经过完成守卫。

**新增测试场景**：
- `LOOP_COMPLETE` 被 honor 后，后续 batch 中 emit `plan.blocked(reason=progress_md_validation_stale)`，被直接拒绝，无法进入事件总线。
- 集成：回放 `primary-20260630-175407` 的 #35（`plan.blocked`）与 #39–#42（终态后事件），断言全部被拒绝。

**遵循模式**：
- `event_policy.rs` 中的 `check_completion_guard`。
- `commit_terminal_delta` 的状态账本更新方式。

**测试场景**：
- 正常路径：`LOOP_COMPLETE` 后下一轮 emit `report.done`，被拒绝。
- 边界情况：后续 batch 中重复 `LOOP_COMPLETE` 被拒绝。
- 集成：回放 `primary-20260630-175407` 的 #37–#42 序列，断言没有任何终态后业务事件被接纳。

**验收标准**：
- 新增跨 batch BDD 场景通过。
- 现有 `completion_honored` 单元测试保持通过。

---

- [ ] U3. **让 CoordinatorDecisionGateStage 强制最后一个 fix-unit 单终态发射**

**目标**：保证最后一个 fix-unit 的 activation 只 emit 一个 `plan.complete`，不再夹杂 competing `work.ready`。U3 是**语义层强制改写**，与 U1 的预算层策略职责分离：U3 决定“应该发出什么事件”，U1 决定“预算是否允许该事件进入总线”。

**需求**：R3

**依赖**：无（U3 改写 topic 后，U1 负责保证改写后的终态事件优先获得槽位）

**文件**：
- 修改：`crates/ralph-core/src/event_loop/stages/coordinator_decision_gate_stage.rs`
- 测试：`crates/ralph-core/src/event_loop/stages/coordinator_decision_gate_stage.rs`（扩展现有测试）

**做法**：
- 扩展 `CoordinatorDecisionGateStage::rewrite_work_ready_topic`：当识别为 `FixUnitLast` 时，将 topic 改写为 `plan.complete`，并**构造/补全 `plan.complete` 所需 payload 字段**（`step`、`plan_name`、`task_id`、`completed_steps`）。若原 payload 缺少字段，按 stage 可及的状态填充；无法填充时拒绝该事件。
- 该 stage 只负责改写一个事件；同一 batch 中若存在多个来自同 hat 的业务事件，由 U1 的预算优先级处理，U3 不再额外丢弃事件。
- 可选：当为 `plan.complete` 改写时发布诊断事件 `event.coordinator.phase_override`。

**遵循模式**：
- 现有 `rewrite_work_ready_topic` 与 `classify_work_ready`。
- 现有 `StageReject` 语义。

**测试场景**：
- 正常路径：`work.ready` 携带 `step.fix-02.last_in_phase=true` 被改写为 `plan.complete`。
- 边界情况：batch 中同时存在 `work.ready(fix-02, last_in_phase=true)` 与 stray `work.ready(fix-02)`，stray 被丢弃。
- 错误路径：缺少 `step` 字段的畸形 `plan.complete` payload 被拒绝。
- 集成：BDD 场景 `2026-06-30-001-u3-fix-unit-terminal-guard` 通过。

**验收标准**：
- `coordinator_decision_gate_stage.rs` 单元测试通过。
- 相关 BDD 场景通过。

---

- [ ] U4. **为 fix-unit task id 增加时间戳窗口校验**

**目标**：拒绝时间戳超出当前 loop 窗口的 fix-unit `task_id`。

**需求**：R4

**依赖**：无

**文件**：
- 修改：`crates/ralph-core/src/state_projector/task.rs`
- 测试：`crates/ralph-core/src/state_projector/task.rs`（扩展现有测试）

**做法**：
- 在 `is_valid_task_id_format` 中校验 shape-1 fix-unit id 的 unix 时间戳。
- 如果时间戳早于 `loop_start_ts - 60s` 或晚于 `now + 60s` 则拒绝。窗口要足够宽以容忍正常时钟 skew，但足以捕获手写的 2025 年时间戳。
- 对 shape-2 id（`task-{slug}-fix{NN}u{NN}-{ts_hex}`）的 hex 时间戳应用同样窗口。
- 拒绝信息必须提示 coordinator 使用 `Task::fix_unit_task_id`。

**遵循模式**：
- 现有 `p0_b_invalid_task_id_format_for_fix_unit_is_rejected` 测试。
- 现有 `Task::fix_unit_task_id` helper。

**测试场景**：
- 正常路径：当前 loop 时间下 `Task::fix_unit_task_id` 输出被接受。
- 边界情况：比 loop 开始早 1 秒的时间戳仍被接受（在 60s 窗口内）。
- 错误路径：`task-1751414400-a1b2`（2025 年时间戳）被拒绝。
- 错误路径：未来 1 小时的时间戳被拒绝。
- 集成：投影携带 stale `task_id` 的 `work.ready(fix-02)` 时， surface `invalid_task_id_format`。

**验收标准**：
- `state_projector/task.rs` 单元测试通过。
- 现有 `test_fix_unit_task_id_must_be_helper_derived` 测试保持通过。

---

- [ ] U5. **收紧 ce-executor-serial preset 指令与 schema**

**目标**：消除 coordinator 指令歧义，并加固 `plan.blocked.reason` 的路由依据。

**需求**：R3、R5、R6

**依赖**：U3、U4（U6 为辅助增强，不阻塞 U5 的主体 schema/指令收紧）

**文件**：
- 修改：`presets/en/ce-executor-serial.yml`
- 修改：`presets/schemas/ce-executor-serial.yml`
- 测试：`crates/ralph-cli/src/presets.rs`（SSOT byte-equality）、`crates/ralph-core/src/preset_lint/`

**做法**：
- 在 `presets/en/ce-executor-serial.yml` 最后一个 fix-unit 的指令中，强化“本轮只能 emit 一个事件”的说明，并明确指出 `CoordinatorDecisionGateStage` 会把最后一个 `work.ready` 改写为 `plan.complete`。
- 在 fix-unit `task_id` 指令中强制要求调用 `Task::fix_unit_task_id`，禁止手写时间戳。
- 在 coordinator 指令中明确要求读取 `task.resume` / context payload 中的 `expected_event` 与 `phase` 字段，按字段要求发射事件，不再依赖自然语言推断。
- 在 `presets/schemas/ce-executor-serial.yml` 中为 `plan.blocked.reason` 增加 `allowed_values` 或 regex 约束，使 `progress_md_validation_stale` 这类 narrative 不再被接受；将 `progress_md_validation_stale` 从允许的 reason 列表中移除。
- 按 AGENTS.md builtin preset 改动的 4/5 处同步规则，更新 `presets/manifest.yml`、`crates/ralph-cli/src/presets.rs`、`presets/index.json`、`scripts/ralph-zsh-plugin.zsh`、`AGENTS.md`/`CLAUDE.md`。

**遵循模式**：
- AGENTS.md 中 preset SSOT 4/5 处同步规则。
- 现有 schema `required_fields` 风格。

**测试场景**：
- 正常路径：schema 收紧后 `preset_lint` 仍通过。
- 错误路径：`plan.blocked` payload 中 `reason=progress_md_validation_stale` 被 schema 校验拒绝。
- 集成：SSOT byte-equality 测试通过。

**验收标准**：
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
- `cargo nextest run -p ralph-core -- preset_lint`
- `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`
- `./scripts/run-tests.sh`

---

- [ ] U6. **在 coordinator 上下文 payload 中注入阶段与预期事件标记**

**目标**：让 coordinator 每次被激活时都能从 payload 中直接读到当前阶段和应该发射的事件，不再靠自然语言或启发式推断。**U6 是辅助增强单元**，不是 P0 修复的必经之路；它通过降低 agent 误判概率来减少未来 regression 风险。

**需求**：R6

**依赖**：无（可独立实现；U3 的 stage 改写是最终兜底）

**文件**：
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（task.resume / context 注入点）
- 修改：`crates/ralph-core/src/event_loop/context_builder.rs` 或等价的上下文生成模块（如存在）
- 测试：`crates/ralph-core/src/event_loop/mod.rs` 现有测试、新增 BDD 场景

**做法**：
- 在 progress-steward 或 event-loop 注入 `task.resume` / context 给 coordinator 时，附带结构化 JSON 字段：
  - `phase`: `"plan_unit" | "fix_unit" | "review_walk" | "ship" | "terminal"`
  - `expected_event`: `"work.ready" | "review.start" | "plan.complete" | "plan.blocked" | "LOOP_COMPLETE"`
  - `last_in_phase`: bool
  - `completed_steps`: 已关闭 step 列表
  - `total_fix_units`: fix-unit 总数
  - `current_fix_unit_index`: 当前 fix-unit 序号
  - `next_step`: 下一步 step id（当 `expected_event=work.ready` 时）
  - `reason`: 人类可读说明
- 这些字段应写入 `task.resume` payload 的 `orchestrator_state` 对象，或写入 `context.md` 的独立 JSON 块，确保 coordinator prompt 能引用。
- 更新 `presets/en/ce-executor-serial.yml` 中 coordinator 的 prompt 模板，要求 agent 优先按 payload 中的 `expected_event` 发射，而不是自己判断阶段。

**遵循模式**：
- 现有 `enrich_task_resume_payload` 函数。
- 现有 `context.md` 生成逻辑。

**测试场景**：
- 正常路径：最后一个 fix-unit 完成后，注入的 `task.resume` payload 中 `expected_event=plan.complete`，coordinator 按此 emit。
- 正常路径：中间 plan unit 完成后，`expected_event=work.ready`，`next_step=step-02`。
- 错误路径：coordinator 忽略 `expected_event` 而 emit 其他事件时，被 isolated 预算或 stage gate 拒绝。
- 集成：BDD 场景回放 `primary-20260630-175407` 中 fix-02 完成后阶段，断言 `task.resume` payload 包含 `expected_event=plan.complete`。

**验收标准**：
- `cargo nextest run -p ralph-core -- test` 通过。
- 新增 BDD 场景通过。
- 现有 coordinator 相关测试保持通过。

---

## 系统级影响

- **交互图**：isolated 预算改动影响所有 isolated 模式 preset，不只是 `ce-executor-serial`；完成守卫改动影响所有配置了 `completion_after_terminal` 的 preset。
- **错误传播**：被拒绝的终态后事件会产生 `event.completion.blocked` 诊断事件；配置为 `reject` 时也会静默拦截。
- **状态生命周期风险**：持久化的 `completion_honored` 必须在 loop 重启/重新水合时清零，避免新 loop 继承旧终态标志。
- **API 表面对齐**：无 CLI/API 变更。coordinator payload 新增 `orchestrator_state` 字段，`task.resume` 的 schema 不因此收紧（向后兼容，缺失该字段时 coordinator 仍按现有 prompt 规则推断）。
- **集成覆盖**：BDD 场景必须走真实 `EventLoop` runner，禁止使用仅断言 iteration 数的 `run_scenario` stub。U2 验收标准中增加对 `plan.blocked` 在 `LOOP_COMPLETE` 后被直接拒绝的覆盖。

---

## 风险与依赖

| 风险 | 缓解措施 |
|------|---------|
| 改动 isolated 预算优先级影响其他 preset | 跑全量 `./scripts/run-tests.sh`；终态 topic 列表从配置派生 |
| 持久化完成标志在重新水合时未清零 | 在 loop 构造/回放路径中显式重置（已写入 U2 做法） |
| `CoordinatorDecisionGateStage` 丢弃事件可能误伤合法 `work.ready` | U3 不再负责丢弃，只负责改写；竞争事件由 U1 的预算优先级处理 |
| 时间戳窗口误拒 resumed loop 的合法 id | 使用 `loop_start_ts` 而非进程启动时间；60s 容差覆盖正常情况 |
| coordinator payload 字段缺失导致旧行为被依赖 | 字段设计为可选；prompt 同时保留 fallback 规则，逐步迁移 |
| `plan.blocked` 的“陈旧”误判根因未直接修复 | U2 的持久完成守卫 + U5 的 reason 白名单从机制上让该路径无法生效；验收中增加对该序列的断言 |

---

## 文档与运营说明

- 若 `crates/ralph-core/data/ralph-tools*.md` 中引用的命令行为或事件拓扑发生变化，需同步更新。
- 若 `ce-executor-serial` 描述或 builtin preset 列表变化，需同步更新 `AGENTS.md` 与 `CLAUDE.md`（推荐 `cp CLAUDE.md AGENTS.md`）。
- 若 preset 名称或 builtin 列表变化，需更新 `scripts/ralph-zsh-plugin.zsh`。

---

## 来源与参考

- **来源文档：** `docs/report/2026-07-01-ce-executor-serial-primary-20260630-175407-diagnosis.md`
- 相关计划：`docs/plans/2026-06-30-001-fix-ce-executor-serial-fix-unit-terminal-p0-plan.md`
- 机制代码：`crates/ralph-core/src/event_loop/mod.rs`、`crates/ralph-core/src/event_policy.rs`
- Stage 代码：`crates/ralph-core/src/event_loop/stages/coordinator_decision_gate_stage.rs`、`crates/ralph-core/src/event_loop/stages/terminal_state_guard_stage.rs`
- 投影代码：`crates/ralph-core/src/state_projector/task.rs`
- Preset/Schema：`presets/en/ce-executor-serial.yml`、`presets/schemas/ce-executor-serial.yml`
