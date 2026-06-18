---
title: "fix: State projection Phase 1 review findings"
type: fix
status: active
date: 2026-06-17
origin: docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md
---

# fix: State projection Phase 1 review findings

## Overview

本计划修复 `2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md` 执行后 Review 发现的 P0/P1 问题。核心目标是：在合并前消除阻塞项、补齐 preset instruction cleanup、统一 prompt 注入路径、并补充关键集成测试。

验收标准：
- P0 阻塞项（`enforce_current_unit` 被 projector 关闭）修复后，`ralph-core` 相关测试与 BDD scenario 全绿。
- 两个验收 preset（`ce-executor-isolated`、`ce-executor-serial`）的 instructions 不再要求 agent 手改 ledger 或调用 task CLI。
- ORCHESTRATOR CONTEXT 注入路径或范围在文档中明确。
- `PROJECTED_TOPICS` 与 config handler 保持一致。
- 新增 event_loop hook 与 runtime state 注入的集成测试。

---

## Problem Frame

Phase 1 已实现状态投影器 scaffold、task/progress 写路径、ORCHESTRATOR CONTEXT 注入与 BDD scenario，但 Review 发现以下必须修复的问题：

1. **P0 阻塞**：`state_projector/task.rs:48` 显式 `set_enforce_current_unit(false)`，绕过了 preset 启用的 R4 约束（同 step 下仅允许当前 U 的 task）。
2. **P1 文档/契约不同步**：英文 preset 的 per-hat instructions 仍要求 agent 更新 `.agents/scratchpad/.../progress.md` 并调用 `ralph tools task start/close`。
3. **P1 中文 preset 滞后**：`presets/zh/ce-executor-isolated-zh.yml` 未同步 U5 cleanup。
4. **P1 契约测试陈旧**：`crates/ralph-cli/src/presets.rs` 中的 U4 progress-reconcile 测试仍在强制执行旧的手写 progress.md 顺序。
5. **P1 注入路径不完整**：ORCHESTRATOR CONTEXT 仅在 isolated custom-hat 路径注入，coordinator/solo/向后兼容路径缺失。
6. **P1 声明与实现不一致**：`PROJECTED_TOPICS` 包含 `review.passed`/`review.failed`/`plan.blocked`，但 config 与 handler 未实现对应 action。
7. **P1 测试缺口**：缺少 event_loop hook 集成测试与 runtime_state 注入集成测试。

---

## Requirements Trace

- **R1.** 移除 projector 对 `enforce_current_unit` 的硬编码关闭，尊重 loop config（修复 P0）。
- **R2.** `ce-executor-isolated.yml` 与 `ce-executor-serial.yml` 的 instructions 统一以 `## ORCHESTRATOR CONTEXT` 为读源，删除 agent 手写 ledger 与调用 task CLI 的要求。
- **R3.** `presets/zh/ce-executor-isolated-zh.yml` 同步英文 preset 的 cleanup。
- **R4.** 更新 `crates/ralph-cli/src/presets.rs` 中的契约测试，使其反映 projector 驱动的单写者模型。
- **R5.** 明确 ORCHESTRATOR CONTEXT 的注入范围：要么补全 coordinator/solo/向后兼容路径，要么在计划与文档中声明 Phase 1 仅支持 isolated。
- **R6.** 统一 `PROJECTED_TOPICS` 与 config handler：移除未实现的 review 终端 topic 或补 mapping。
- **R7.** 补充 event_loop hook 与 runtime_state 注入的集成测试。

---

## Scope Boundaries

- 不新增 Phase 2 功能（events.jsonl 单写者、bash fail-closed、per-hat 视图裁剪等）。
- 不修改 `memories.md` 写路径。
- 不修改 task CLI 语义（仅 preset instructions 层禁止 agent 调用）。
- 不处理 `hooks::executor` 在 macOS 下的 4 个既有失败（与本计划无关）。

### Deferred to Follow-Up Work

- Phase 2 plan（`docs/brainstorms/2026-06-17-hat-orchestrator-state-projection-requirements.md` 中 SP-R6/13/14/16/17/20）。
- CLI emit 预检接 `progress_task_gate`（可与 Phase 2 并行）。

---

## Context & Research

### Relevant Code and Patterns

| 领域 | 路径 | 当前问题 |
|------|------|---------|
| Task 投影 | `crates/ralph-core/src/state_projector/task.rs` | L48 硬编码 `set_enforce_current_unit(false)` |
| 投影器顶层 | `crates/ralph-core/src/state_projector/mod.rs` | L51-59 `PROJECTED_TOPICS` 与 config handler 不一致 |
| Progress 投影 | `crates/ralph-core/src/state_projector/progress.rs` | `project_plan_complete` 跨层关闭 task，职责边界待理清 |
| Runtime state | `crates/ralph-core/src/runtime_state.rs` | wave 子段硬编码 `None`，`derive_plan_name` 强耦合 preset key 形状 |
| Prompt 注入 | `crates/ralph-core/src/event_loop/mod.rs` | 仅 isolated custom-hat 路径调用 `prepend_orchestrator_context` |
| Preset 英文 isolated | `presets/en/ce-executor-isolated.yml` | per-hat instructions 仍要求手写 progress.md / task CLI |
| Preset 英文 serial | `presets/en/ce-executor-serial.yml` | 同上 |
| Preset 中文 isolated | `presets/zh/ce-executor-isolated-zh.yml` | 未同步 U5 cleanup |
| Preset 契约测试 | `crates/ralph-cli/src/presets.rs` | U4 progress-reconcile 测试仍强制执行旧合约 |
| BDD scenario | `crates/ralph-core/tests/scenarios/step_handoff/state_projection_work_done_updates_progress.yml` | 已存在，需扩展以覆盖 reject 路径 |

### Institutional Learnings

- `docs/solutions/developer-experience/ce-executor-plan-gate-premature-completion-2026-06-02.md` — progress 滞后导致 plan-gate 误判。
- `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-recovery-2026-06-17.md` — 三处状态漂移叠加 recovery 噪音。
- `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` — steward 应读注入块而非直读 ledger。

### External References

- 无外部 research；本地模式充分。

---

## Key Technical Decisions

- **R4 处理策略**：projector 不写 `set_enforce_current_unit(false)`，而是让 `TaskStore::ensure` 自身读取 loop config 的 `enforce_current_unit` 值。若当前实现中 `TaskStore` 不持有 config，则在 `project_ensure_task` 调用前由 `EventLoop` 将 config 传入 `ProjectionContext`，使 projector 与 loop 配置一致。
- **preset instruction cleanup 策略**：不删除 scratchpad progress.md 的 operator/journal 用途说明，但明确标注其为“operator-only human journal；agent 禁止写入，唯一权威读源为 `## ORCHESTRATOR CONTEXT`”。
- **ORCHESTRATOR CONTEXT 注入范围**：优先补全 coordinator/solo/向后兼容路径；若实现中发现这些路径的 prompt 构建差异过大，则改为在计划与 `AGENTS.md` 中显式声明“Phase 1 仅 isolated 模式支持 ORCHESTRATOR CONTEXT”。
- **PROJECTED_TOPICS 对齐策略**：优先移除 `review.passed`/`review.failed`/`plan.blocked`（当前未实现 handler），避免声明与实现不一致；若业务需要这些 topic，则新增 `StateProjectionAction` variant 与 mapping 字段。
- **契约测试策略**：将 `presets.rs` 中旧的“progress.md 更新顺序”断言替换为“ORCHESTRATOR CONTEXT 指引存在 + agent task CLI 禁止 + state_projection enabled”断言。

---

## Open Questions

### Resolved During Planning

- **R4 是否应该在 projector 层关闭？** 否。projector 必须尊重 loop config，否则破坏 preset 契约。
- **scratchpad progress.md 是否彻底删除？** 否。保留作为 operator journal，但 instructions 中禁止 agent 写入。
- **中文 preset 是否需要同步？** 是。`ce-executor-isolated-zh.yml` 必须同步英文 cleanup。
- **review 终端 topic 如何处理？** 优先从 `PROJECTED_TOPICS` 移除；如需保留需新增 handler。

### Deferred to Implementation

- ORCHESTRATOR CONTEXT 注入是否补全非 isolated 路径，取决于实现时代码复杂度，由 U5 执行时决定并文档化。
- `derive_plan_name` 的 preset key 形状耦合问题是否在本次修复，由 U5 执行时评估；若改动较大则降为 P2 单独跟进。

---

## Implementation Units

- [ ] U1. **修复 projector 关闭 R4 约束的问题**

**Goal:** 移除 `state_projector/task.rs` 中对 `enforce_current_unit` 的硬编码关闭，确保 projector 尊重 loop config。

**Requirements:** R1

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/state_projector/task.rs`
- Modify: `crates/ralph-core/src/state_projector/mod.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（如需将 config 传入 ProjectionContext）
- Test: `crates/ralph-core/src/state_projector/tests.rs`

**Approach:**
- 删除 `project_ensure_task` 中的 `store.set_enforce_current_unit(false)`。
- 检查 `ProjectionContext` 是否已携带 `enforce_current_unit` 配置；若未携带，则在 `EventLoop` 初始化 projector 时从 `self.config.event_loop.enforce_current_unit` 传入。
- 确保 `TaskStore::ensure` 使用与 loop 一致的配置值。

**Patterns to follow:**
- `crates/ralph-core/src/task_store.rs` — `set_enforce_current_unit` / `enforce_current_unit` 字段
- `crates/ralph-core/src/config/event_loop.rs` — `EventLoopConfig` 中 `enforce_current_unit` 的默认值

**Test scenarios:**
- **Happy path:** `enforce_current_unit=true` 时，同 step 下不同 unit 的 `work.ready` 第二个事件被 projector reject。
- **Happy path:** `enforce_current_unit=false` 时，同 step 下不同 unit 的 `work.ready` 两个事件均可创建 task。
- **Edge case:** `enforce_current_unit` 未在 YAML 中显式设置时，使用默认值（当前为 `false`），行为不变。
- **Integration:** projector reject 的事件从 event batch 中移除，并发布 `event.state_projection.rejected`。

**Verification:**
- `cargo nextest run -p ralph-core -- state_projector` 全绿。
- `cargo nextest run -p ralph-core --test scenarios` 全绿。

---

- [ ] U2. **英文 preset instruction cleanup（isolated + serial）**

**Goal:** 删除或重写 per-hat instructions 中要求 agent 手写 progress.md / 调用 task CLI 的内容，统一以 `## ORCHESTRATOR CONTEXT` 为权威读源。

**Requirements:** R2

**Dependencies:** U5（注入范围确定后，instructions 才能准确描述读源）

**Files:**
- Modify: `presets/en/ce-executor-isolated.yml`
- Modify: `presets/en/ce-executor-serial.yml`
- Test: `crates/ralph-cli/src/presets.rs`

**Approach:**
- 在 `coordinator`、`executor`、`plan-gate`、`shipper`、`reporter` 等 hat 的 instructions 中：
  - 将“Update `progress.md` (path: `.agents/scratchpad/...`)”替换为“以 `## ORCHESTRATOR CONTEXT` 中的 `current_step` / `completed_steps` / `open_tasks` 为准；禁止手改 ledger”。
  - 将“`ralph tools task start` / `close`”替换为“orchestrator 会在 `work.ready` / `work.done` 后自动更新 tasks.jsonl；agent 禁止调用 task CLI”。
- 保留 scratchpad progress.md 作为 operator journal 的说明，但明确标注为 operator-only。
- 若某段 instructions 依赖任务状态字段（如 Runtime Task ID、Status: in_progress），改为引用 `## ORCHESTRATOR CONTEXT` 中的 `open_tasks` 列表。

**Patterns to follow:**
- 现有 U5 跨 hat HARD RULE comment 的语气（`presets/en/ce-executor-isolated.yml:134-154` 附近）
- GOV 脑暴删 prompt patch 的“机制优先”语气

**Test scenarios:**
- **Integration:** `presets.rs` 断言 `ce-executor-isolated` 与 `ce-executor-serial` 的 instructions 中不再出现 “Update `progress.md`” 作为 agent 义务。
- **Integration:** `presets.rs` 断言 instructions 中明确出现 `## ORCHESTRATOR CONTEXT` 作为读源。
- **Integration:** `presets.rs` 断言 instructions 中 agent 禁止调用 `ralph tools task ensure|start|close|fail`。
- **Integration:** `ralph preset check --strict -H builtin:ce-executor-isolated` 与 `builtin:ce-executor-serial` 绿。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- preset` 全绿。
- 手动检查 preset 中无遗留 agent 手写 ledger / task CLI 的指令。

---

- [ ] U3. **中文 preset instruction cleanup**

**Goal:** 将 `presets/zh/ce-executor-isolated-zh.yml` 同步到与英文 preset 一致的 U5 cleanup 状态。

**Requirements:** R3

**Dependencies:** U2

**Files:**
- Modify: `presets/zh/ce-executor-isolated-zh.yml`
- Test: `crates/ralph-cli/src/presets.rs`（如新增中文 preset 断言）

**Approach:**
- 同步 U2 的修改策略到中文 preset。
- 在顶部添加与英文对应的 cross-hat HARD RULE comment，声明：
  - 权威读源为 `## ORCHESTRATOR CONTEXT`。
  - agent 禁止调用 `ralph tools task ensure|start|close|fail`。
  - agent 禁止 `tail events.jsonl` 推导状态。
- 将 per-hat instructions 中的 scratchpad progress.md 写入义务替换为 ORCHESTRATOR CONTEXT 读源。

**Test scenarios:**
- **Integration:** 中文 preset 解析通过 `RalphConfig::parse_yaml`。
- **Integration:** 中文 preset instructions 中不再要求 agent 手写 progress.md。
- **Integration:** 中文 preset instructions 中明确引用 `## ORCHESTRATOR CONTEXT`。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- preset` 全绿。

---

- [ ] U4. **更新 presets.rs 契约测试**

**Goal:** 替换陈旧的 U4 progress-reconcile 测试，使其反映 projector 驱动的单写者模型。

**Requirements:** R4

**Dependencies:** U2, U3

**Files:**
- Modify: `crates/ralph-cli/src/presets.rs`
- Test: `crates/ralph-cli/src/presets.rs`（内置测试）

**Approach:**
- 退休或更新以下测试：
  - `test_ce_executor_u4_progress_reconcile_queue_advance_en`
  - `test_ce_executor_u4_progress_reconcile_task_execution_loop_en`
- 新增断言：
  - `config.event_loop.state_projection.enabled == true`
  - `state_projection.actions` 包含 `work.ready`、`work.done`、`queue.advance`、`plan.complete`。
  - instructions 中无 `.agents/scratchpad/ce-executor/{plan_name}/progress.md` 作为 canonical ledger。
  - instructions 中出现 `## ORCHESTRATOR CONTEXT`。
  - instructions 中 agent 禁止调用 `ralph tools task ensure|start|close|fail|reopen`。
- 对 `ce-executor-serial.yml` 同样执行上述断言。

**Test scenarios:**
- **Happy path:** `ce-executor-isolated.yml` 的 embedded content 通过所有新断言。
- **Happy path:** `ce-executor-serial.yml` 的 embedded content 通过所有新断言。
- **Error path:** 若某 preset 缺少 `state_projection.enabled: true`，测试失败并给出明确消息。
- **Error path:** 若 instructions 中仍要求 agent 调用 `ralph tools task start`，测试失败。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- preset` 全绿。

---

- [ ] U5. **统一 ORCHESTRATOR CONTEXT 注入路径**

**Goal:** 明确并落实 ORCHESTRATOR CONTEXT 的注入范围。

**Requirements:** R5

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-core/src/runtime_state.rs`（如需适配）
- Create: `crates/ralph-core/src/event_loop/tests/runtime_state_injection.rs`（plan 原要求）
- Test: 新创建的 integration test 文件

**Approach:**
- 在 `EventLoop::build_prompt` 中检查所有调用 `prepend_wave_context` 的路径：
  - isolated custom-hat 路径（已注入）
  - coordinator/multi-hat 路径
  - solo `ralph` 路径
  - backward-compat non-isolated custom-hat 路径
- 决策：若这些路径的 `base_prompt` 构建方式允许安全注入，则统一调用 `prepend_orchestrator_context`；若差异过大，则：
  - 保持仅 isolated 路径注入；
  - 在 `runtime_state.rs` 顶部注释、本计划、以及 `AGENTS.md` 中明确声明 Phase 1 仅 isolated 模式支持 ORCHESTRATOR CONTEXT。
- 将 `prepend_orchestrator_context` 的 `&mut self` 改为 `&self`（如不需要 mutability）。
- 可选：为 `ORCHESTRATOR CONTEXT` 增加 `RALPH_RUNTIME_STATE` env var 暴露（mirror `RALPH_WAVE_CONTEXT`），本次仅作可选，不强求。

**Patterns to follow:**
- `crates/ralph-core/src/wave_context.rs` — `to_prompt_block` 与 heading 常量
- `crates/ralph-core/src/event_loop/mod.rs` — `prepend_wave_context` 调用位置

**Test scenarios:**
- **Happy path:** isolated custom-hat 的 prompt 包含 `## ORCHESTRATOR CONTEXT`。
- **Happy path:** `state_projection.enabled=false` 时，prompt 包含 disabled stub 说明。
- **Integration（如补全路径）：** coordinator `ralph` hat 的 prompt 也包含 `## ORCHESTRATOR CONTEXT`。
- **Integration（如仅 isolated）：** 新建测试显式断言 coordinator/solo 路径**不**包含 ORCHESTRATOR CONTEXT，并在注释中引用本计划 R5 决策。
- **Edge case:** `hat_id == "ralph"` 时注入被跳过（与现有逻辑一致）。

**Verification:**
- `cargo nextest run -p ralph-core -- runtime_state` 全绿。
- 新 integration test 文件全绿。

---

- [ ] U6. **对齐 PROJECTED_TOPICS 与 config handler**

**Goal:** 消除 `PROJECTED_TOPICS` 与 `StateProjectionAction` handler 之间的不一致。

**Requirements:** R6

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/state_projector/mod.rs`
- Modify: `crates/ralph-core/src/config/state_projection.rs`（如需新增 action）
- Test: `crates/ralph-core/src/state_projector/tests.rs`

**Approach:**
- **默认方案**：从 `PROJECTED_TOPICS` 中移除 `review.passed`、`review.failed`、`plan.blocked`，因为它们当前无任何 handler。
- **可选方案**：若产品需要这些 topic，则新增 `StateProjectionAction::UpdateActiveWave` 或类似 variant，并在 preset mapping 中配置；实现对应的 progress 字段更新。该方案会扩大本次修复范围，需经执行时确认。
- 无论选择哪种方案，都需要更新 `PROJECTED_TOPICS` 的单元测试（`projected_topics_list_is_locked`），确保列表被锁定且与实现一致。

**Test scenarios:**
- **Happy path:** `PROJECTED_TOPICS` 中每个 topic 都在 `StateProjectionAction` 中有对应 handler。
- **Error path（默认方案）：** 未配置 action 的 topic 被 apply 时保持 inert，不写盘。
- **Integration（可选方案）：** `review.passed` 正确更新 progress 的 Active Wave/Sequence 字段。

**Verification:**
- `cargo nextest run -p ralph-core -- state_projector` 全绿。

---

- [ ] U7. **补充 event_loop hook 与 runtime state 注入集成测试**

**Goal:** 覆盖 event_loop 调用 projector、reject 事件移除、diagnostic 发布、以及 ORCHESTRATOR CONTEXT 注入的真实路径。

**Requirements:** R7

**Dependencies:** U1, U5, U6

**Files:**
- Create: `crates/ralph-core/src/event_loop/tests/runtime_state_injection.rs`
- Create: `crates/ralph-core/src/event_loop/tests/state_projection_hook.rs`（或合并到 `runtime_state_injection.rs`）
- Modify: `crates/ralph-core/tests/scenarios/step_handoff/state_projection_work_done_updates_progress.yml`（扩展 reject 路径）

**Approach:**
- 新建 `runtime_state_injection.rs`，使用 `EventLoop` 真实实例驱动 `process_events_from_jsonl`，断言：
  - 非 `ralph` hat 的 prompt 包含 `## ORCHESTRATOR CONTEXT`。
  - prompt 中包含 `current_step` / `completed_steps` / `open_tasks` 字段。
  - `state_projection.enabled=false` 时 prompt 包含 disabled stub。
- 新建/扩展 hook 集成测试，使用 mock event batch：
  - batch 中一个合法 `work.ready` 与一个缺少 `task_key` 的 `work.ready`。
  - 断言合法事件被保留、非法事件被移除、bus 中发布 `event.state_projection.rejected`。
  - 断言同 topic 不同 payload 时仅移除对应 payload 的事件（P0 批处理 bug 回归防护）。
- 扩展 BDD scenario：增加一个 iter，发送缺少必填字段的 `work.done`，断言 `plan.blocked` 出现且 loop 进入 recovery。

**Patterns to follow:**
- `crates/ralph-core/src/event_loop/tests/` 中现有测试的 `EventLoop` 构造方式
- `crates/ralph-core/tests/scenarios.rs` — `run_workflow_guard_scenario`

**Test scenarios:**
- **Happy path:** `state_projection.enabled=true` 时，event batch 成功投影后 ledger 与预期一致。
- **Error path:** 缺少 `task_key` 的 `work.ready` 被 reject 并从 bus 移除。
- **Error path:** 同 topic 不同 payload 时，仅错误 payload 被移除，sibling 事件保留。
- **Integration:** reject 后 bus 发布 `event.state_projection.rejected` diagnostic。
- **Integration:** ORCHESTRATOR CONTEXT 出现在非 `ralph` hat 的 prompt 中。
- **Edge case:** 空 batch 时不写盘、不注入额外内容。

**Verification:**
- `cargo nextest run -p ralph-core -- runtime_state` 全绿。
- `cargo nextest run -p ralph-core -- state_projection_hook`（或对应 subset）全绿。
- `cargo nextest run -p ralph-core --test scenarios` 全绿。

---

## System-Wide Impact

- **Interaction graph:**
  - `EventLoop` 初始化 `StateProjector` 时传入 `enforce_current_unit` 配置（U1）。
  - `build_prompt` 在更多/明确的路径上调用 `prepend_orchestrator_context`（U5）。
  - preset YAML 的 instructions 从“agent 手写 ledger”转向“读 ORCHESTRATOR CONTEXT”（U2/U3）。
  - `presets.rs` 测试从旧 U4 合约转向新 projector 合约（U4）。
- **Error propagation:**
  - projector 继续 fail-closed；reject 事件从 bus 移除并发布 diagnostic。
  - R4 reject 现在会正确传播到 `ApplyReport` 并触发事件丢弃。
- **State lifecycle risks:**
  - U1 修复后，同 step 跨 unit 的 task 创建会被 R4 拒绝，减少 ledger 漂移。
  - cross-loop cache staleness 仍是 Phase 2 问题，本计划不解决。
- **API surface parity:**
  - `ralph emit` 行为不变。
  - `ralph tools task` 语义不变，仅 preset instructions 层禁止 agent 调用。
- **Integration coverage:**
  - U7 的集成测试覆盖 event_loop → projector → gate → bus 全链路。
  - BDD scenario 扩展覆盖 reject recovery 路径。
- **Unchanged invariants:**
  - FR-2 `event_projection.rs`、hat_channel merge、flow_lifecycle、memories 路径不改。
  - WAVE CONTEXT 的注入顺序与内容不改（GOV-R1）。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| U1 修改后 R4 过度 reject 影响现有测试 | 新增测试覆盖 `enforce_current_unit=true/false` 两种配置；跑全量 `ralph-core` + scenarios |
| preset instruction cleanup 遗漏 | `presets.rs` 新增断言自动扫描；U2/U3 完成后人工 grep `progress.md` / `ralph tools task` |
| ORCHESTRATOR CONTEXT 注入补全引入 coordinator 路径回归 | U5 新增 integration test；若复杂度过高则文档化 isolated-only 范围 |
| 中文 preset 与英文语义漂移 | U3 完成后用 `diff` 或结构化断言比对关键 section |
| review 终端 topic 移除影响外部调用者 | `PROJECTED_TOPICS` 为内部常量，无外部 ABI；移除前确认无其他代码引用 |

---

## Documentation / Operational Notes

- 若 U5 决定 Phase 1 仅 isolated 支持 ORCHESTRATOR CONTEXT，需更新：
  - `AGENTS.md` / `.cursor/rules/multi-hat-isolation.mdc` 中相关描述。
  - 本计划 U5 的 verification 注释。
- 无需更新 `ralph-tools.md`（task CLI 语义不变）。
- 完成本计划后，将 `2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md` 的 status 从 `active` 改为 `completed`（或等效归档），并注明修复计划编号。

---

## Sources & References

- **Origin plan:** [docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md](docs/plans/2026-06-17-003-feat-hat-orchestrator-state-projection-phase1-plan.md)
- **Review结论:** 2026-06-17 003 plan review（本会话前置消息）
- Code: `crates/ralph-core/src/state_projector/`, `crates/ralph-core/src/event_loop/mod.rs`, `crates/ralph-core/src/runtime_state.rs`
- Presets: `presets/en/ce-executor-isolated.yml`, `presets/en/ce-executor-serial.yml`, `presets/zh/ce-executor-isolated-zh.yml`
- Tests: `crates/ralph-cli/src/presets.rs`, `crates/ralph-core/tests/scenarios/step_handoff/state_projection_work_done_updates_progress.yml`
