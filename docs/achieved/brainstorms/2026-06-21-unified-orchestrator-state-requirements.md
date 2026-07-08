---
date: 2026-06-21
topic: unified-orchestrator-state
title: "Ralph 编排状态统一化 — 架构减法需求文档"
related:
  - docs/report/2026-06-21-top-3-architectural-instability-factors.md
  - docs/brainstorms/2026-06-21-serial-preset-root-cause-fix-requirements.md
---

# Ralph 编排状态统一化 — 架构减法需求文档

## Summary

基于 `docs/report/2026-06-21-top-3-architectural-instability-factors.md` 的诊断，本需求定义一次**架构层面的减法与统一**：用单一状态账本（State Ledger）和单一协议视图（Protocol View）替换当前分散的内存 tracker 与多层 gate，彻底移除 `task.resume` 自指恢复循环。

核心目标不是继续为 `ce-executor-serial` 等症状打补丁，而是把编排器从「agent emit → 多层 gate 校验 → recoverable rejection → task.resume → agent 再 emit」的循环模型，改造成「agent emit → 统一校验 → 要么原子提交、要么 deterministic correction」的直线模型。

本需求吸收 `docs/brainstorms/2026-06-21-serial-preset-root-cause-fix-requirements.md` 中有价值的部分，替代其战术修复包。

## Problem Frame

当前架构有三个相互缠绕的系统性缺陷：

1. **自指恢复循环**：recoverable rejection 触发 `task.resume`，`task.resume` 经 EventBus 进入目标 hat 的 prompt，agent 再次 emit 业务事件，业务事件重新进入同一套或另一层 gate，再次被拒。同一根因在不同 stage 之间漂移时 retry key 不同，预算无法跨层累积。
2. **软提示驱动关键动作**：task 关闭、progress.md 更新、handoff artifact 生成等核心副作用依赖 agent 读取 prompt 后自觉执行。runtime 只在 emit 落盘后才做 fail-closed 拒绝，无法主动修正。
3. **多状态源竞争写入**：同一 workflow 的进度同时写入磁盘（`tasks.jsonl`、`progress.md`）和多个内存结构（`WorkflowProgress`、`ReviewStepTracker`、`HandoffTracker`、`PolicyRuntimeState` 等），各源之间没有单一提交点，gate 读取不同 snapshot 导致误判。

过去 30 天的补丁一直在加固某一层 gate（加字段、加校验、加 retry、加 artifact validation），但没有拆掉 gate 之间的循环依赖和状态源的分散写入。

## Actors

- **A1. Operator**：运行 `ralph run` 并诊断失败的人。需要失败时看一眼诊断就知道是哪道语义规则被拒，而不是 14 条无结构 recovery。
- **A2. Workflow hat**：按 preset 拓扑 emit 事件、消费事件的 agent（coordinator / executor / reviewer / fixer / plan-gate / shipper / reporter 等）。
- **A3. Orchestrator runtime**：`event_loop`、`state_projector`、gates、recovery 机制的集合。新架构下 runtime 是状态变更的唯一提交者。
- **A4. Preset maintainer**：修改 `presets/schemas/` 或 `presets/en/` 下配置的人。需要 lint、engine gate、runtime gate 对同一协议有同一答案。

## Key Flows

### F1. 事件从 emit 到提交

- **Trigger:** agent emit 一个事件。
- **Actors:** A2, A3
- **Steps:**
  1. runtime 从当前 `LedgerSnapshot` 和 preset 协议生成 `ProtocolView`。
  2. 单一验证入口按 `ProtocolView` 检查事件：origin、publisher 权限、required fields、macro-edge 契约、workflow phase、execution contract。
  3. 验证失败时，runtime 把结构化 `rejection` 记录到 ledger，并直接把 deterministic correction 写回当前 hat 的 prompt；**不** emit `task.resume`。
  4. 验证通过时，runtime 在 `StateLedger` 中原子提交所有派生状态变更（task、progress、workflow phase、review step、handoff deadline 等），然后才落盘到 `tasks.jsonl` / `progress.md`。
- **Outcome:** 事件要么一次通过，要么在同一 turn 内得到明确修正指令；不再跨 turn 循环。
- **Covered by:** R1, R2, R3, R4

### F2. hat 切换与 handoff artifact

- **Trigger:** 一个 macro-edge 事件通过验证。
- **Actors:** A3
- **Steps:**
  1. `ProtocolView` 统一判定该 topic 是否为 macro-edge 以及需要哪种 handoff artifact。
  2. runtime 自动在 workspace 下写出 artifact，并把 `handoff_path` 作为事件元数据的一部分注入下游 hat 的 prompt。
  3. 下游 hat 的 prompt 从 `LedgerSnapshot` 读取上游状态，而不是直接读磁盘。
- **Outcome:** lint、engine gate、runtime gate 对宏观边契约有三同一答案；agent 不写或写错路径在验证阶段即被拒绝。
- **Covered by:** R5, R6

### F3. work.done 后状态不漂移

- **Trigger:** executor emit `work.done`。
- **Actors:** A2, A3
- **Steps:**
  1. 统一验证确认 `work.done` 满足 execution contract（task 已关闭、step 已标记等）。
  2. `StateLedger` 原子提交：close task → mark step completed → advance workflow phase。
  3. `StateProjector` 把同一提交写入 `tasks.jsonl` 和 `progress.md`。
  4. 下游 `queue.advance` / `plan.complete` 验证时读取 `LedgerSnapshot`，不再绕过 projector 直接读磁盘。
- **Outcome:** 不会因 tasks.jsonl 与 progress.md 更新不同步导致 plan-gate 误拒。
- **Covered by:** R7, R8

### F4. Recovery 收敛为可观测信号

- **Trigger:** 同一 hat 连续 emit 不符合契约的事件。
- **Actors:** A2, A3, A1
- **Steps:**
  1. 每次拒绝写入结构化 `reason_code`（如 `origin:ralph_control_only`、`protocol:missing_required_field`、`contract:task_not_closed`）。
  2. runtime 累计同一 hat+reason_code 的拒绝次数。
  3. 达到阈值后直接升级 `human.guidance`，不再通过 `task.resume` 让 agent 自己猜。
  4. `ralph diagnose` 从 ledger 的 rejection log 直接呈现根因。
- **Outcome:** recovery.jsonl 不再被无结构噪音填满；operator 能快速定位。
- **Covered by:** R9, R10

## Requirements

### 统一状态账本（State Ledger）

- **R1.** 引入单一 `StateLedger` 结构，替代 `WorkflowProgress`、`ReviewStepTracker`、`HandoffTracker`、`PolicyRuntimeState` 等独立内存 tracker。
- **R2.** 所有状态变更（task 状态、progress 步骤、workflow phase、review step、handoff deadline、flow lifecycle、terminal 承诺）必须在 `StateLedger::commit()` 中按顺序原子提交。
- **R3.** `StateProjector::apply()` 必须从 `StateLedger` 的提交日志派生，禁止 projector 与 ledger 各自维护并行状态。
- **R4.** 启动或恢复 loop 时，runtime 从磁盘 `tasks.jsonl` / `progress.md` 重建 `StateLedger` 到一致快照，而不是让各 tracker 自行 bootstrap。

### 统一协议视图（Protocol View）

- **R5.** 定义单一 `ProtocolView`，由当前 `LedgerSnapshot` + preset 拓扑 + schema 派生；lint、engine gate、runtime gate 必须基于同一 `ProtocolView` 做判断。
- **R6.** `ProtocolView` 必须统一回答以下问题：某 topic 是否允许当前 hat 发布、是否为 macro-edge、是否需要 handoff artifact、required fields 是什么、当前 phase 允许哪些 topics。
- **R7.** 禁止任何 gate 绕过 `ProtocolView` 读取私有状态或直接读磁盘做验证。
- **R8.** CLI lint（`ralph emit --policy-check` 等）与 loop 内验证使用同一 `ProtocolView` 实现，确保「CLI 早失败、loop 终裁」但两者结论一致。

### 移除 `task.resume` 循环

- **R9.** 删除 `publish_policy_rejection_resume` 及相关 recoverable rejection → `task.resume` 路径。reject 不进入 EventBus，不交给 agent 重 emit。
- **R10.** recoverable rejection 的处理方式改为：把「上次拒绝的精确原因 + 期望 payload 模板」直接写入当前 hat 下一次 prompt 的 deterministic instruction 区块。
- **R11.** 同一 hat 在短窗口内对同一 reason_code 拒绝 ≥ N 次（建议 N=3）时，直接升级 `human.guidance` 或 `loop.suspend`，不再循环 retry。
- **R12.** `ralph diagnose` 读取 ledger 中的 rejection log 和 drift counter，输出结构化根因，而不是 recovery.jsonl 的原始条目列表。

### handoff 与 prompt 状态源

- **R13.** macro-edge 的 handoff artifact 必须由 runtime 在验证通过后自动写出，`handoff_path` 作为事件元数据注入下游 prompt；agent emit 时不应自行构造 handoff 路径。
- **R14.** 下游 hat 的 prompt 中，`## ORCHESTRATOR CONTEXT` 区块必须读取 `LedgerSnapshot`（或 projector 缓存），禁止直接读取 `tasks.jsonl` / `progress.md`。
- **R15.** `progress-steward` 等决策型 hat 的 instructions 从「直读四文件决策树」改为「读取 `## ORCHESTRATOR CONTEXT`」。

## Acceptance Examples

- **AE1. 状态统一**：Given executor emit `work.done`，when 验证通过，then `StateLedger` 中 task closed、step completed、phase advanced 同时提交；`tasks.jsonl` 与 `progress.md` 不会出现一个已更新、另一个未更新的情况。
- **AE2. gate 一致**：Given `ce-executor-serial` preset，when CLI lint 和 loop runtime 分别判断 `review.dimension.ready` 是否为 macro-edge，then 两者结论一致，且 coordinator 自环 `queue.advance` 不会被误判为宏观边。
- **AE3. 无 task.resume 循环**：Given executor emit 的 `work.done` 缺少 `task_id`，then runtime 拒绝并直接把 `reason_code=contract:missing_task_id` 和期望模板写回 executor prompt；**不**生成 `task.resume` 事件。
- **AE4. deterministic correction**：Given coordinator 连续 3 次 emit 非法 `work.ready`，then runtime 升级 `human.guidance`，recovery.jsonl 中只保留 3 条结构化 rejection + 1 条 escalation，而非 14 条无分类 recovery。
- **AE5. handoff 自动生成**：Given `work.ready` 被判定为 macro-edge，then runtime 自动在 `.ralph/handoffs/` 下写出 artifact，并把路径注入 executor prompt；agent 不写路径也能通过验证。

## Success Criteria

- **SC1.** `ce-executor-serial` 在统一架构下能跑通 `coordinator → executor → review → fix → plan-gate → shipper → reporter → LOOP_COMPLETE` 全链路，无 `consecutive_failures`、无用户 abort。
- **SC2.** 一次失败 run 中，`recovery.jsonl` 不再被 `hat_handoff_*` 或 `task_resume_*` 条目占满；单次可纠正错误只产生 1 条结构化 rejection。
- **SC3.** `ralph diagnose` 对失败 run 能给出单一结构化根因（如 `protocol:macro_edge_mismatch`、`contract:task_not_closed`），而非症状列表。
- **SC4.** lint、engine gate、runtime gate 对同一事件给出相同结论；修改 preset 后 `cargo build` 能通过一致性校验。
- **SC5.** 全 workspace nextest（ralph-cli 串行，其余并行）在重构后保持绿色。

## Scope Boundaries

本次覆盖 Ralph core orchestration 的 state management、gating model、recovery model。

### Deferred for later

- wave worker 共享状态抽象 / supervisor 协议 6 件套升级。
- `ce-executor-isolated` 与 `ce-executor-wave` 的移除或重构。
- `ralph-tools*.md` 文档同步（可在架构验证后批量补）。
- `loop.cancel` 与 `loop.terminate` 的语义统一。
- 为 isolated/wave preset 设计新的 handoff 协议。

### Outside this product's identity

- 把 Ralph 改造成通用 workflow DSL 或可视化编排器。
- 重写整个 EventBus / StateMachine 为分布式系统。
- 引入外部数据库替代 JSONL 文件存储。

## Key Decisions

- **D-1. 根因优先，不做 21 个补丁。** 症状是分散架构的必然结果；修架构比修症状更省长期成本。
- **D-2. 保留核心能力，减去实现复杂度。** 多 hat 隔离、handoff artifact、execution contract、state projection 都保留，但它们的内部实现被统一。
- **D-3. 一次性重构到终态（big-bang）。** 不渐进迁移，避免新旧两套模型并存产生更多不一致。
- **D-4. 新架构吸收并替代 serial-preset 战术修复包。** 有价值的约束被吸收进 `ProtocolView` 和 `StateLedger`，重复或临时的修复项被废弃。
- **D-5. agent 关键副作用仍按 serial-preset 修复包推进，不纳入本次架构 redesign。** 本次 focus 在「状态与 gate 统一」和「recovery 循环移除」。
- **D-6. 单一事实源落在 `StateLedger` + `ProtocolView`。** 不复用已废弃的 `ralph-proto/serial_protocol` 方案，也不让各 gate 各自维护状态。

## Dependencies / Assumptions

- **DEP-1.** `crates/ralph-core/src/state_projector/` 已具备 `apply`、`bootstrap_from_disk`、task/progress 写能力，可作为 `StateLedger` 的底层落盘层。
- **DEP-2.** `HandoffIndex`、`ProtocolView` 的索引化视图已在 `crates/ralph-core/src/preset/engine/` 存在，可被扩展为统一视图。
- **DEP-3.** `RALPH_CONTROL_TOPICS` 已在 `crates/ralph-core/src/event_origin.rs` 定义，新架构的 origin 规则可直接继承。
- **DEP-4.** nextest 测试环境可用，`ralph-core` BDD scenarios 可复用。

- **ASSUM-1.** 重构期间允许调整 `presets/schemas/ce-executor-serial.yml` 中与 state_projection、macro-edge 相关的配置。
- **ASSUM-2.** Operator 接受 `ralph` hat 不再能发任何业务 topic；紧急情况走 operator CLI 或 bypass 机制。
- **ASSUM-3.** 团队能承担 2-4 周的 big-bang 重构窗口，期间 serial preset 主线可能不稳定。

## Outstanding Questions

### Resolve Before Planning

- （无）

### Deferred to Planning

- **Technical:** `StateLedger` 是严格追加日志（append-only log）还是 mutable snapshot + commit history？前者更利于 replay 和诊断，后者迁移成本更低。
- **Technical:** `ProtocolView` 的生成频率是每 turn 一次、每事件一次，还是缓存到 ledger 变更时刷新？
- **Technical:** 现有 `event_loop/mod.rs` 中的 `process_parse_result` 调用链如何重构为单一验证入口？是否需要保留内部阶段用于诊断输出？
- **Needs research:** 哪些现有测试依赖 `task.resume` 的副作用？需要预先列出并改写。

## Next Steps

-> 使用 `/ce-plan` 或进入 planning 阶段，输出结构化实施计划，包括：
1. `StateLedger` 数据结构与提交协议设计
2. `ProtocolView` 统一入口与现有 gate 的融合/废弃方案
3. `task.resume` 移除的精确改动点
4. 测试迁移与验收矩阵
