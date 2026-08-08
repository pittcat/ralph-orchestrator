---
title: "Agent 执行契约门控：防忘操作、防错操作、防假推进"
type: feat
status: active
date: 2026-06-03
origin: "用户现场调试 ce-executor：executor 实施后忘记 emit，旧 default_publishes 将无事件伪装为 work.done"
related:
  - docs/plans/2026-05-31-001-feat-emit-hard-gate-plan.md
  - docs/plans/2026-05-31-004-feat-agent-operation-guard-plan.md
  - docs/plans/2026-06-02-004-fix-ce-executor-plan-gate-plan.md
  - docs/plans/2026-06-02-005-feat-payload-contract-validation-plan.md
---

# Agent 执行契约门控：防忘操作、防错操作、防假推进

## Overview

本计划修复一类比 `ralph emit` 更基础的问题：Agent 会忘记或搞错任何流程义务，包括 emit、关闭 task、更新进度、跑测试、写报告或携带正确 payload。当前系统大量依赖 prompt 要求 agent 自觉执行这些操作，Ralph 只在少数路径上做兜底；这会导致“忘操作”被默认事件掩盖，或者“错操作”仍推进到下一个 hat。

核心方案是把执行义务从 prompt 文字转成 Ralph 侧可观测的 contract：

1. **操作是否发生**：实施型 hat 本轮必须产出事件；没有事件就 gate。
2. **操作是否正确**：事件 payload、task 状态、git 状态、测试证据必须与完成声明一致。
3. **是否允许推进**：只有 contract 通过的 `work.done` 才能进入 bus 触发 review；失败则转成 guidance/diagnostic，不继续推进。

本计划与 payload contract 计划互补：payload contract 管字段形状；本计划管 agent 执行义务和完成真实性。

## Problem Frame

现场复现中，`ce-executor` 的 executor 被成功调度并写了代码和 scratchpad，但 events JSONL 中没有真实 `work.done` 或 `work.failed`。旧 embedded preset 的 executor 配置了 `default_publishes: "work.done"`，因此 Ralph 在 agent 没写事件时向内存 bus 注入了 `work.done`，导致 UI/后续状态像是 work pass，而 JSONL 没有真实完成事件。

这暴露出三个独立缺口：

- **忘操作不可见**：agent 没执行 `ralph emit` 时，当前 hard gate 只有在输出文本出现 `ralph emit` 才触发；完全忘记 emit 不会被 gate。
- **默认事件语义过强**：`default_publishes` 本是兜底机制，但放在 executor 上会把“未声明结果”变成“成功完成”或“失败完成”，掩盖真实原因。
- **完成声明未经验收**：即使 agent 发了 `work.done`，Ralph 也没有在进入 review 前核验 task、diff、测试和 progress 是否真的满足完成条件。

## Requirements

### Emit / Operation Occurrence

- **R1.** 对声明 `publishes` 且没有 `default_publishes` 的 hat，如果本轮没有任何有效事件，Ralph 必须 gate，而不是静默继续。
- **R2.** Gate 不再依赖 agent 输出里是否出现 `ralph emit`；“完全忘了 emit”也必须被发现。
- **R3.** Gate 必须写入可持久化 guidance，明确列出允许发布的 topics 和缺失的操作。
- **R4.** 连续 gate 必须有上限，超过上限后终止 loop，避免无限消耗。

### Completion Truth

- **R5.** `work.done` 只是 agent 的完成声明，不是完成事实；Ralph 必须在发布到 bus 前验证。
- **R6.** `work.done` payload 必须包含 contract 要求的字段，至少包括 `task_id`、`task_key`、`step`，ce-executor 还要求 `plan_name`、`plan_path`。
- **R7.** `work.done.task_id` 必须能在当前 loop 的 task store 中找到，且 task 必须属于当前 loop。
- **R8.** 对实施型 contract，task 必须处于 terminal success 状态，或由 contract 显式允许 Ralph 在验证通过后自动关闭。
- **R9.** 完成声明必须与 git worktree 状态一致：非 trivial work 不能空 diff/空 commit 通过。
- **R10.** 如果 contract 声明需要测试证据，必须能从 payload 或诊断记录中找到测试运行结果；缺失时不得推进 review。
- **R11.** Contract 失败时，原始 `work.done` 不得触发下游 review；Ralph 应发布 guidance/diagnostic，要求 agent 修正或显式 `work.failed`。

### Preset and Compatibility

- **R12.** `ce-executor` 的 executor 必须移除 `default_publishes`，让 no-event gate 生效。
- **R13.** Gate 型 hat 可以保留 fail-closed/block-closed 默认值，例如 `plan-gate default_publishes: "plan.blocked"`，但成功型默认值不得用于实施型 hat。
- **R14.** root preset、embedded preset、中文 preset 必须同步；修改 builtin preset 后必须跑 mirror drift 检查。
- **R15.** 现有 payload contract、origin guard、state machine、workflow guard、plan-gate、task authorization 测试不能回归。

## Scope Boundaries

### In Scope

- 改造 emit hard gate，使“无事件”本身可被 gate。
- 为 `ce-executor` executor 移除成功/失败默认发布，强制显式 emit。
- 新增 Ralph-owned `work.done` completion contract 验证路径。
- 新增 contract 配置结构和 ce-executor 默认配置。
- 增加 deterministic 单元测试、preset 静态测试、event loop replay-light 测试。
- 写入结构化 diagnostic/guidance，帮助 agent 下一轮纠正。

### Out of Scope

- 实现通用自然语言理解来判断 agent “是否真的做完”。
- 用 live LLM 作为测试 oracle。
- 重写全部 preset 的 workflow。
- 替换已有 payload contract 计划；本计划只消费其字段 schema 能力，不重复实现。
- 自动修复 agent 的代码实现错误；本计划只阻止错误状态继续推进。

### Deferred Follow-Up

- 把 testing evidence 做成统一 `ralph tools evidence` 子命令。
- 为所有 builtin preset 建立完整 execution contract 适配矩阵。
- 在 TUI 中展示 contract rejection 的专用面板。
- 将 contract 失败持久化为可查询的 loop report。

## Context & Existing Patterns

- `crates/ralph-cli/src/loop_runner.rs` 已有 hard gate 基础设施：`output_mentions_ralph_emit`、`should_hard_gate`、`inject_hard_gate_guidance`、连续 gate 计数、跳过 `default_publishes`。
- 当前 `should_hard_gate` 条件是 `publishes` 非空且 `default_publishes` 为空，但主循环只在输出文本包含 `ralph emit` 时调用它。
- `crates/ralph-core/src/event_loop/mod.rs` 的 `process_parse_result` 是事件进入 bus 前的单一入口，顺序是 scope enforcement、origin guard、event policy、state machine、workflow guard、record/publish。
- `ProcessedEvents.accepted_events` 已能告诉 loop runner 哪些事件被接受，但目前没有 contract rejection 字段。
- `crates/ralph-core/src/task.rs` 已有 `loop_id` 和 `owner_hat_id`，`TaskStore` 已有 `get`、`get_by_key_in_loop`、`close`、`has_pending_tasks` 等能力，可直接用于完成真实性验证。
- `docs/plans/2026-05-31-004-feat-agent-operation-guard-plan.md` 已为 task ownership 建立基础；本计划不重复做授权，只复用结果。
- `docs/plans/2026-06-02-005-feat-payload-contract-validation-plan.md` 已规划 payload schema 和 runtime guard；本计划只在 completion contract 中引用 payload 字段验证，不重复实现 schema_file。

## Key Technical Decisions

### KTD1. 无事件 gate 不再依赖 “口嗨 emit” 检测

当前 hard gate 只处理“agent 提到了 `ralph emit` 但没写事件”。这漏掉了最常见的失败模式：agent 完全忘了 emit。

改为两段逻辑：

- 如果输出提到 `ralph emit`，先保留 late-event recovery，避免事件文件 flush 延迟误判。
- 不管输出是否提到 `ralph emit`，只要本轮没有事件、没有 wave、当前 hat 有发布义务且无默认兜底，就触发 hard gate。

这让 gate 语义从“防口嗨”升级为“防忘出口”。

### KTD2. 实施型 hat 不使用 `default_publishes`

`default_publishes` 适合 gate 型或诊断型 hat 的 fail-closed 兜底，例如 `plan.blocked`、`fix.exhausted`、`report.done`。它不适合 executor，因为 executor 的缺省状态不是完成，也不是可验证失败，而是“结果未知”。

ce-executor 的 executor 应移除 `default_publishes`，让 no-event gate 强制其显式发布 `work.done` 或 `work.failed`。如果 agent 无法判断结果，应发布 `work.failed` 并携带 reason，而不是让 Ralph 代写事件。

### KTD3. Completion contract 插在 bus publish 前

`work.done` 必须在触发 review 前被验证。最小侵入点是在 `process_parse_result` 中 origin/payload/state/workflow guard 之后、`record_event` 和 `bus.publish` 之前增加 execution contract validation。

理由：

- 此时事件来源、payload 基础结构和 workflow 顺序已通过现有 guard。
- 此时尚未 publish 到 bus，下游 hat 不会看到被拒绝的 `work.done`。
- 失败可以统一写 `event.execution_contract.rejected` 和 `human.guidance`，并返回到 `ProcessedEvents` 供 loop runner 诊断。

### KTD4. Contract 先聚焦 `work.done`

先只实现 `work.done` contract，不急于泛化所有 topics。这样能直接修复 ce-executor 的问题，同时避免一开始就把所有 preset 拖入适配。

后续可把同一机制扩展到 `review.complete`、`REVIEW_COMPLETE`、`report.done` 等主题。

### KTD5. Contract 失败是 backpressure，不是自动修复

Ralph 不应猜测 agent 想做什么。Contract 失败时：

- 不把原事件推进到 bus。
- 发布结构化 diagnostic。
- 发布 guidance 告诉 agent 缺少哪些可观测状态。
- 让下一轮 agent 修复并重新 emit，或显式 `work.failed`。

只有 `auto_close_task_on_valid_work_done` 这种低风险动作可以作为显式配置；默认不自动关闭 task。

## Proposed Configuration

新增配置建议放在 `event_loop.execution_contracts` 下，避免混入 payload policy：

```yaml
event_loop:
  execution_contracts:
    enabled: true
    rules:
      work.done:
        require_payload_fields: ["plan_name", "plan_path", "task_id", "task_key", "step"]
        require_task:
          id_field: "task_id"
          key_field: "task_key"
          loop_scoped: true
          allowed_terminal_statuses: ["closed"]
          auto_close_on_valid: false
        require_git_change:
          mode: diff_or_commit
          allow_empty_for_steps: ["trivial"]
        require_test_evidence:
          mode: optional
        reject:
          diagnostic_topic: "event.execution_contract.rejected"
          guidance_topic: "human.guidance"
```

初始实现可以只支持 `work.done` 所需字段，保留结构扩展点。

## Implementation Units

### U1. Emit Obligation Gate v2

**Goal:** 让 Ralph 在 agent 完全忘记 emit 时也能拦住，而不是只处理“口嗨 emit”。

**Requirements:** R1, R2, R3, R4

**Dependencies:** 无

**Files:**

- Modify: `crates/ralph-cli/src/loop_runner.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-cli/src/loop_runner.rs`
- Test: `crates/ralph-core/src/event_loop/tests.rs`

**Approach:**

- 保留 `recover_expected_emit_after_output`，但只把它作为 late-event recovery，不作为 hard gate 的唯一入口。
- 新增 helper：`should_gate_missing_events(display_hat, event_loop, agent_wrote_events, wave_events, hard_gate_already_triggered)`.
- 条件：
  - `agent_wrote_events == false`
  - `wave_events.is_empty()`
  - 当前 display hat 有 `publishes`
  - 当前 display hat 没有 `default_publishes`
  - 当前迭代尚未触发 hard gate
- 触发后：
  - `event_loop.increment_hard_gate_count()`
  - `inject_hard_gate_guidance(...)`
  - 跳过 `check_default_publishes`
  - 进入下一轮，让 agent 根据 guidance 修复
- 如果后续读取到事件或 termination，则 `reset_hard_gate_count()`。
- 连续 3 次后保留现有 `TerminationReason::Stopped` 行为。

**Test scenarios:**

- Happy path: 有 `publishes`、无 `default_publishes`、本轮无事件、输出没提 `ralph emit`，触发 hard gate。
- Happy path: 输出提到 `ralph emit` 且 late event 成功写入，不触发 hard gate。
- Edge: 有 `default_publishes` 的 hat 无事件，不触发 hard gate，继续走默认兜底。
- Edge: 无 `publishes` 的 silent hat 无事件，不触发 hard gate。
- Failure: 连续 3 次 missing-event hard gate 后 loop 终止。

**Verification:**

- `cargo test -p ralph-cli test_should_hard_gate -- --nocapture`
- `cargo test -p ralph-cli test_missing_event_hard_gate -- --nocapture`
- `cargo test -p ralph-core test_hard_gate_terminates_after_max -- --nocapture`

### U2. ce-executor 实施型 hat 去默认兜底

**Goal:** 让 ce-executor executor 必须显式发布 `work.done` 或 `work.failed`，不能由 Ralph 默认代发。

**Requirements:** R12, R13, R14

**Dependencies:** U1

**Files:**

- Modify: `presets/ce-executor.yml`
- Modify: `presets/ce-executor-zh.yml`
- Modify: `crates/ralph-cli/presets/ce-executor.yml`
- Modify: `crates/ralph-cli/presets/ce-executor-zh.yml`
- Modify: `crates/ralph-cli/src/presets.rs`
- Modify: `crates/ralph-core/src/preset_validator.rs`

**Approach:**

- 从 executor hat 删除 `default_publishes`。
- 保持 `publishes: ["work.done", "work.failed"]`。
- 在 executor instructions 中强调：完成或失败都必须显式 emit；未 emit 会被 Ralph hard gate。
- 不改变 `plan-gate`、`fixer`、`shipper`、`reporter` 等 gate/report 型 hat 的 fail-closed 默认值。
- 同步中文 preset 和 embedded mirror。
- 更新 preset validator 中 ce-executor fixture。
- 增加静态测试：
  - executor `default_publishes == None`
  - executor 仍声明 `work.done` 和 `work.failed`
  - gate 型 hat 的 block/fail 默认值保持不变
  - root preset 与 embedded mirror 无漂移

**Test scenarios:**

- Regression: executor 不得默认 `work.done`。
- Regression: executor 不得默认 `work.failed`，因为这仍会绕过“必须显式 emit”的训练。
- Regression: plan-gate 默认 `plan.blocked` 保持可用。
- Regression: 中文/英文 executor triggers/publishes/default_publishes 一致。

**Verification:**

- `proxy ./scripts/sync-embedded-files.sh check`
- `cargo test -p ralph-cli test_ce_executor_executor_default_publishes -- --nocapture`
- `cargo test -p ralph-core preset_validator::tests::ce_executor_topology_is_valid -- --nocapture`

### U3. Execution Contract 配置模型

**Goal:** 在 config 中增加轻量 execution contract 结构，为 `work.done` 验证提供配置来源。

**Requirements:** R5, R6, R7, R8, R9, R10, R11

**Dependencies:** 无

**Files:**

- Modify: `crates/ralph-core/src/config.rs`
- Test: `crates/ralph-core/src/config.rs`

**Approach:**

- 新增：
  - `ExecutionContractsConfig`
  - `ExecutionContractRule`
  - `TaskCompletionRequirement`
  - `GitChangeRequirement`
  - `TestEvidenceRequirement`
  - `ContractRejectConfig`
- `EventLoopConfig` 增加 `execution_contracts: Option<ExecutionContractsConfig>`。
- 默认值初期建议为 disabled，避免在全 preset 适配前影响用户；ce-executor 显式启用。
- 字段均加 `serde(default)`，保证旧配置可解析。
- `rules` 使用 topic string 作为 key，初期只实现 `work.done`。

**Test scenarios:**

- Happy: 不配置 execution_contracts 时 parse 通过且 disabled。
- Happy: 配置 `work.done` rule 后字段正确解析。
- Edge: 缺少 optional 子块时使用默认值。
- Compatibility: 旧 preset YAML 不包含 execution_contracts 时行为不变。

**Verification:**

- `cargo test -p ralph-core config::tests::test_execution_contract -- --nocapture`

### U4. Work Done Contract Validator

**Goal:** 实现 Ralph-owned `work.done` 验证器，检查 payload、task、git、测试证据，并返回 accept/reject。

**Requirements:** R5, R6, R7, R8, R9, R10, R11

**Dependencies:** U3

**Files:**

- Create: `crates/ralph-core/src/execution_contract.rs`
- Modify: `crates/ralph-core/src/lib.rs`
- Test: `crates/ralph-core/src/execution_contract.rs`

**Approach:**

- 新增核心类型：
  - `ExecutionContractDecision::{Accept, Reject}`
  - `ExecutionContractFinding`
  - `ExecutionContractViolationKind`
  - `ExecutionContractContext`
- Validator 输入：
  - `Event`
  - `ExecutionContractRule`
  - workspace root
  - current loop id
  - tasks path
  - optional active hat id
- Payload 解析：
  - 支持 JSON object payload。
  - 缺 required field 返回 reject。
  - payload 不是 JSON object 时 reject。
- Task 验证：
  - 从 `task_id` 字段读取 task id。
  - 用 `TaskStore::load` 读取 task。
  - task 不存在 reject。
  - `loop_scoped` 为 true 时，要求 task.loop_id == current_loop_id。
  - `allowed_terminal_statuses` 初期只支持 `closed`。
  - `auto_close_on_valid` 初期不启用；若启用，必须在其他验证通过后关闭 task 并保存。
- Git 验证：
  - 初期实现 `mode: diff_or_commit`。
  - 允许通过 `git diff --quiet` / `git diff --cached --quiet` / `git log --oneline` 的组合判断是否有未提交 diff 或本 loop commit。
  - 不在 core 里直接依赖 shell 时，可把 git evidence 作为 injectable trait，单元测试用 fake evidence；loop runner 实际提供实现。
  - 如果实现成本高，U4 初期只做 task/payload，git evidence 留到 U6，但 config 字段保留。
- Test evidence：
  - 初期支持 `optional` 和 `required_payload_field` 两种模式。
  - required 模式检查 payload 中是否有 `tests` 或配置字段。

**Test scenarios:**

- Happy: payload 完整、task closed、loop_id 匹配，accept。
- Failure: payload 缺 `task_id`，reject，finding 指出字段。
- Failure: task 不存在，reject。
- Failure: task 属于其他 loop，reject。
- Failure: task 仍 open/in_progress，reject。
- Failure: required test evidence 缺失，reject。
- Edge: legacy task loop_id 为 None，在 loop_scoped true 下 reject。
- Edge: disabled rule 不影响事件。

**Verification:**

- `cargo test -p ralph-core execution_contract -- --nocapture`

### U5. Event Loop 接入：拒绝假 `work.done`

**Goal:** 在事件 publish 到 bus 前应用 execution contract，失败时不触发下游 review。

**Requirements:** R5, R11, R15

**Dependencies:** U3, U4

**Files:**

- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/event_loop/tests.rs`

**Approach:**

- 扩展 `ProcessedEvents`：
  - `contract_rejections: Vec<ExecutionContractFinding>`
- 在 `process_parse_result` 中，origin guard、event policy、state machine、workflow guard 之后，record/publish 之前调用 validator。
- 对每个事件：
  - 没有 matching rule：通过。
  - rule disabled：通过。
  - rule accept：通过。
  - rule reject：不 record、不 publish 原事件。
- Reject 时 publish 两类事件：
  - `event.execution_contract.rejected`：结构化 JSON finding。
  - `human.guidance`：面向 agent 的修复指导。
- Guidance 文案必须包含：
  - 被拒 topic。
  - 缺失/错误状态。
  - 允许的修复动作：关闭 task 后重新 `work.done`，或明确 `work.failed`。
- 确保 rejected 原事件不会出现在 `accepted_events`。

**Test scenarios:**

- Happy: valid `work.done` 被 publish，review-coordinator 可收到 pending event。
- Failure: open task 的 `work.done` 被拒绝，review-coordinator 收不到事件。
- Failure: rejected `work.done` 产生 diagnostic 和 guidance。
- Regression: 没有配置 execution_contracts 时，现有事件流不变。
- Regression: payload contract rejection 与 execution contract rejection 不互相吞掉。

**Verification:**

- `cargo test -p ralph-core event_loop::tests::test_execution_contract -- --nocapture`
- `cargo test -p ralph-core event_policy -- --nocapture`
- `cargo test -p ralph-core event_origin -- --nocapture`

### U6. Loop Runner 诊断与 TUI/日志可观测性

**Goal:** 让 contract rejection 不只是内部事件，而是对 operator 和下一轮 agent 都清晰可见。

**Requirements:** R3, R11

**Dependencies:** U5

**Files:**

- Modify: `crates/ralph-cli/src/loop_runner.rs`
- Modify if needed: `crates/ralph-core/src/diagnostics/`
- Test: `crates/ralph-cli/src/loop_runner.rs`

**Approach:**

- 在每次 `process_events_from_jsonl*` 返回后读取 `contract_rejections`。
- 如果存在 rejection：
  - 记录 `warn!`，包含 topic、hat、reason、task_id。
  - 在 RPC/TUI 事件中显示短摘要。
  - 不终止 loop；让 injected guidance 驱动下一轮修复。
- 若连续 contract rejection 达到阈值，可复用 hard gate 终止策略或新增 `consecutive_contract_rejections`。初期建议只记录，不终止，避免误判造成过早停止。
- 若 `RALPH_DIAGNOSTICS=1`，写入 `.ralph/diagnostics/.../execution-contract.jsonl`。

**Test scenarios:**

- Rejection 出现时 loop runner 记录摘要。
- Rejection 不触发 default_publishes。
- Rejection 后下一轮 prompt 包含 guidance。

**Verification:**

- `cargo test -p ralph-cli test_execution_contract_rejection_reporting -- --nocapture`

### U7. ce-executor Contract 启用与静态回归测试

**Goal:** 在 ce-executor 中显式启用 `work.done` execution contract，保护本次失败路径。

**Requirements:** R6, R7, R8, R9, R10, R12, R14

**Dependencies:** U2, U3, U4, U5

**Files:**

- Modify: `presets/ce-executor.yml`
- Modify: `presets/ce-executor-zh.yml`
- Modify: `crates/ralph-cli/presets/ce-executor.yml`
- Modify: `crates/ralph-cli/presets/ce-executor-zh.yml`
- Modify: `crates/ralph-cli/src/presets.rs`
- Modify: `crates/ralph-core/src/preset_validator.rs`

**Approach:**

- 在 ce-executor preset 增加 `event_loop.execution_contracts`：
  - `work.done.require_payload_fields`: `plan_name`, `plan_path`, `task_id`, `task_key`, `step`
  - `require_task.loop_scoped: true`
  - `require_task.allowed_terminal_statuses: ["closed"]`
  - `require_git_change.mode: diff_or_commit`
  - `require_test_evidence.mode: optional`，后续再升级为 required。
- 更新 executor instructions：
  - 完成前必须关闭 task。
  - 完成前必须确保 git diff/commit 非空，除非 trivial path。
  - 如果无法满足 contract，发布 `work.failed` 而不是 `work.done`。
- 同步中文和 embedded mirror。
- 增加 preset tests：
  - ce-executor 启用 work.done execution contract。
  - work.done required fields 覆盖 review-coordinator 需要的字段。
  - executor 无 default_publishes。
  - root/embedded/zh 同步。

**Test scenarios:**

- Happy: preset YAML parse 后能读取 execution contract rule。
- Regression: embedded ce-executor 的 executor 没有 default_publishes。
- Regression: 中文 preset 和英文 preset 的 contract required fields 一致。
- Regression: `scripts/sync-embedded-files.sh check` 无漂移。

**Verification:**

- `proxy ./scripts/sync-embedded-files.sh check`
- `cargo test -p ralph-cli presets::tests -- --nocapture`
- `cargo test -p ralph-core preset_validator::tests -- --nocapture`

### U8. Replay-Light 集成测试

**Goal:** 用确定性测试证明本次现场问题不会复发。

**Requirements:** R1, R2, R5, R11, R15

**Dependencies:** U1, U5, U7

**Files:**

- Modify: `crates/ralph-core/src/event_loop/tests.rs`
- Modify if useful: `crates/ralph-core/tests/scenarios/`
- Modify if useful: `crates/ralph-cli/tests/`

**Approach:**

- 构造最小 hat topology：
  - executor triggers `work.ready`
  - executor publishes `work.done`, `work.failed`
  - review triggers `work.done`
  - executor 无 default_publishes
- 测试 A：executor 本轮无事件，loop runner helper 判断 hard gate，而不是 default。
- 测试 B：executor emit `work.done` 但 task open，event loop 拒绝，review 收不到 pending event。
- 测试 C：executor emit `work.done` 且 task closed，event loop 接受，review 收到 pending event。
- 测试 D：旧式 executor 配置 `default_publishes: work.done` 的 fixture 应明确失败或被测试标记为 forbidden，防回归。

**Verification:**

- `cargo test -p ralph-core test_work_done_contract -- --nocapture`
- `cargo test -p ralph-cli test_missing_event_hard_gate -- --nocapture`

### U9. 文档与学习沉淀

**Goal:** 让 preset 作者知道什么时候可以用 `default_publishes`，什么时候必须使用 explicit emit + execution contract。

**Requirements:** R13, R14, R15

**Dependencies:** U1-U8

**Files:**

- Modify: `presets/COLLECTION.md`
- Create: `docs/guide/execution-contracts.md`
- Create: `docs/solutions/developer-experience/agent-execution-contract-gates-2026-06-03.md`
- Modify if needed: `docs/guide/presets.md`

**Approach:**

- 文档明确分类：
  - 实施型 hat：不得使用成功型 `default_publishes`；建议无 default + execution contract。
  - gate 型 hat：可使用 fail/block default。
  - report 型 hat：可使用 report default，但 completion 前必须有防御性检查。
- 记录本次事故模式：
  - agent 做了部分实现但没 emit。
  - 旧 embedded preset 默认 `work.done`。
  - JSONL 无事件但 bus 被默认事件推进。
- 给出 preset 作者 checklist：
  - 这个 hat 忘 emit 时，默认事件是否会造成假成功？
  - 这个 topic 进入下游前，有没有 Ralph-owned 验收？
  - 完成状态是否能从 task/git/test 证据重建？

**Verification:**

- 文档路径存在且 repo-relative。
- 若修改 `crates/ralph-core/data/*.md`，按 AGENTS.md 做源码行号反向验证；本计划默认不修改 tools 文档。

## Sequencing

1. **先止血：** U1 + U2。让 executor 忘 emit 时无法再被默认事件掩盖。
2. **建 contract 能力：** U3 + U4。先做 config 和纯 validator。
3. **接入事件管道：** U5 + U6。确保假 `work.done` 不能 publish 到 bus。
4. **启用 ce-executor：** U7。同步 root/zh/embedded。
5. **补回归测试：** U8。固定现场失败路径。
6. **沉淀文档：** U9。

## Test Matrix

| Area | Command |
|---|---|
| Hard gate helpers | `cargo test -p ralph-cli test_should_hard_gate -- --nocapture` |
| Missing-event gate | `cargo test -p ralph-cli test_missing_event_hard_gate -- --nocapture` |
| Execution contract config | `cargo test -p ralph-core config::tests::test_execution_contract -- --nocapture` |
| Execution contract validator | `cargo test -p ralph-core execution_contract -- --nocapture` |
| Event loop contract rejection | `cargo test -p ralph-core event_loop::tests::test_execution_contract -- --nocapture` |
| Preset sync | `proxy ./scripts/sync-embedded-files.sh check` |
| Preset tests | `cargo test -p ralph-cli presets::tests -- --nocapture` |
| Core preset validator | `cargo test -p ralph-core preset_validator::tests -- --nocapture` |
| Broader regression | `cargo test -p ralph-core event_policy event_origin -- --nocapture` |
| Final gate | `cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output` |

## Risks and Mitigations

- **Risk: no-event gate blocks legitimate silent turns.**  
  Mitigation: only gate hats with non-empty `publishes` and no `default_publishes`; silent hats remain unaffected. Preset authors must choose explicit default or explicit emit.

- **Risk: requiring task closed conflicts with current executor ownership.**  
  Mitigation: use existing `owner_hat_id`/coordinator authorization rules. If executor cannot close coordinator-owned tasks, fix task ownership or coordinator permissions before enabling strict contract.

- **Risk: git change detection is hard to make deterministic.**  
  Mitigation: implement validator with injectable evidence provider; unit tests use fake evidence. CLI/runtime implementation can start with conservative `diff_or_commit`.

- **Risk: contract rejection causes loop churn.**  
  Mitigation: guidance is persisted, and consecutive hard gate already has a stop threshold. Contract rejection threshold can be added after observing behavior.

- **Risk: overlap with payload contract plan.**  
  Mitigation: payload contract remains about event field schema; execution contract only checks workflow obligations and observable state.

## Acceptance Criteria

- ce-executor executor 没有 `default_publishes`。
- executor 忘 emit 时，Ralph 触发 missing-event hard gate，写入 guidance，不注入默认 `work.done`。
- executor emit `work.done` 但 task 仍 open 时，`work.done` 被拒绝，review 不触发。
- executor emit `work.done` 且 task closed、payload 完整、git evidence 满足时，review 正常触发。
- root/zh/embedded ce-executor 同步。
- 相关 unit tests、preset tests、core validator tests 通过。
- 文档明确说明 `default_publishes` 的适用边界和 execution contract 的使用方式。
