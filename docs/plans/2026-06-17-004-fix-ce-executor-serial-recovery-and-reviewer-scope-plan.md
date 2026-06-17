---
title: 修复 ce-executor-serial 恢复链、reviewer 作用域与 review 所有权
type: fix
status: active
date: 2026-06-17
origin:
  - docs/brainstorms/2026-06-17-agent-recovery-mechanism-gaps-requirements.md
  - docs/brainstorms/2026-06-16-ce-executor-bootstrap-recovery-requirements.md
  - docs/solutions/integration-issues/ce-executor-serial-noble-peacock-recovery-2026-06-17.md
  - docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md
  - docs/solutions/integration-issues/ce-executor-serial-noble-peacock-review-chain-2026-06-17.md
---

# 修复 ce-executor-serial 恢复链、reviewer 作用域与 review 所有权

## Overview

本次修复聚焦 `builtin:ce-executor-serial` 在 `pittcat-dev` 分支运行时暴露的恢复机制与设计机制问题。worktree 中间产物仅用于定位故障现象，所有代码改动、配置更新与测试均基于 `pittcat-dev` 分支源码。

核心问题清单：

| 优先级 | 问题 | 根因定位 |
|--------|------|----------|
| P0 | `dimension-reviewer` 在 `testing` 维度发出 `review.passed(skip_reason=aggregate_timeout)`，且无 findings 文件 | `ralph.yml` 把 `dimension-reviewer` 指向 `pi` 后端；`pi` 携带 wave-preset 记忆并可能执行长测试 |
| P0 | `task.resume` 在“声称 emit 但未写入”路径丢失 `dimension/step/task_id/task_key` 上下文 | `crates/ralph-cli/src/loop_runner/hard_gate.rs::inject_hard_gate_guidance` 未像 missing-event 路径那样嵌入原始触发事件 |
| P0/P1 | `dimension-reviewer` 未受只读约束，可执行 `cargo test` 或修改源码 | preset 未设置 `disallowed_tools`，提示词无 HARD RULE |
| P1 | `review-coordinator.publishes` 包含 `review.passed`，与 `review-synthesizer` 所有权冲突 | `presets/en/ce-executor-serial.yml:689` |
| P1 | `ralph.yml` 注释漂移为 `ce-executor-isolated`，实际运行 `ce-executor-serial`；且 `dimension-reviewer` 被 override 到 `pi` | `ralph.yml:43,56-57` |
| P1 | `work.start` 未进入 `.ralph/events-*.jsonl` 持久事件流 | `crates/ralph-cli/src/loop_runner/runner.rs:998-1020` 仅写入 history logger |
| P2 | topic 路由/验证仍存在 HashMap 迭代顺序非确定性 | `crates/ralph-core/src/event_loop/mod.rs` 中部分索引仍用 `HashMap` |
| P2 | `review.dimension.ready` 事件偶发缺失 `source` 字段 | `ralph emit` 默认不填 `source`，业务事件序列化不一致 |

修复目标：让 serial review 链在单维度卡住时能够自愈回到正确维度；消除 `review.passed` 的多重发布者；把 reviewer 锁死在只读角色；让启动事件可 replay；让路由与序列化更稳定。

---

## Problem Frame

`ce-executor-serial` 的核心价值是把 4 个 review 维度串行走完，用确定性换取稳定性。但在 `pittcat-dev` 实际运行中，以下三类机制缺陷会叠加：

1. **恢复上下文丢失**：当 `dimension-reviewer` 因后端原因没有产出事件时，orchestrator 注入的 `task.resume` 如果不携带原始 `review.dimension.ready` 的 payload，reviewer 就无法知道当前该 review 哪个维度，导致 recovery 失败或走到错误维度。
2. **Reviewer 角色边界模糊**：`dimension-reviewer` 被配置为 `pi` 后端且未限制工具使用，使其可能把 review 阶段变成“再跑一遍测试”或“顺手改代码”，既超时又污染源码。
3. **Review 所有权冲突**：`review-coordinator` 和 `review-synthesizer` 都能发 `review.passed`，agent 在提示词漂移时容易发错 topic，且空 diff 快路径与 synthesizer 终局路径并存，产生竞义。

这些不是单一 bug，而是 orchestration contract 的多处缺口。需要一次性修补预设、CLI runner 和项目配置。

---

## Requirements Trace

- **R1.** `task.resume` 在“claim-but-no-write”hard gate 路径必须携带原始触发事件的 `topic` 与完整 `payload`，并在下一激活时把该触发事件 replay 到 `last_activation_events`（与 2026-06-17-004 U3 的 missing-event 路径对齐）。
- **R2.** `dimension-reviewer` 必须被约束为只读：禁止 `Bash`（不能跑测试/构建）和 `Edit`（不能修改源码），只允许 `Write` 产出 findings JSON；提示词中必须有显式 HARD RULE。
- **R3.** `review.passed` 只能由 `review-synthesizer` 发布；`review-coordinator` 在空 diff 时应发出 `review.dimensions.complete`，由 synthesizer 产出 `review.passed(skip_reason=dimensions_complete)`。
- **R4.** `ralph.yml` 的注释必须与 `builtin:ce-executor-serial` 一致，且 `dimension-reviewer` 使用与 `review-coordinator`/`review-synthesizer` 一致的 `claude` 后端。
- **R5.** 配置的 `starting_event`（`work.start`）必须在 loop 启动时写入当前 `.ralph/events-*.jsonl`，同时保证 live loop 不会重复消费该事件。
- **R6.** 影响路由、验证或诊断输出的集合遍历必须是确定性的；HashMap 的裸迭代必须被替换为 `BTreeMap` 或排序后的 `Vec`。
- **R7.** 业务事件序列化必须携带 `source` 字段；在 isolated 模式下未显式提供 `--source` 时，应默认使用 emitting hat。

---

## Scope Boundaries

- **本次覆盖**：`ce-executor-serial` preset、`ralph.yml`、CLI loop runner、EventReader、事件策略与序列化、对应 BDD/集成/单元测试。
- **不覆盖**：
  - 不修改 `pi` 后端本身（超出本仓库范围）。
  - 不重写 `ce-executor-isolated` / `ce-executor-wave` 的 wave 机制。
  - 不新增 hat，不改变 10-hat 拓扑。
  - 不自动编辑 plan 文件；plan frontmatter 漂移用 `ralph doctor plan-sync` 检测（已有机制，不在本次修复范围内）。
  - 不改动 `scripts/ralph-zsh-plugin.zsh`，因为 preset 名称未变、未新增/删除 builtin preset。

---

## Context & Research

### 相关代码与模式

- **Hard gate / recovery 注入**：`crates/ralph-cli/src/loop_runner/hard_gate.rs`
  - `inject_missing_event_hard_gate_guidance_with_triggers` 已经实现 trigger snapshot 与 `original_trigger_topic` / `original_trigger_payload` 嵌入（line ~644-828）。
  - `inject_hard_gate_guidance`（claim-but-no-write 路径，line ~477-584）仍未携带 trigger context，且 caller 在 `crates/ralph-cli/src/loop_runner/runner.rs:4054` 处未传入 `last_activation_events`。
- **Runner 启动事件**：`crates/ralph-cli/src/loop_runner/runner.rs:998-1020` 仅把 `work.start` 写入 history logger，注释明确说明“start event only appears in history, not in the trusted event stream”。
- **EventReader**：`crates/ralph-core/src/event_reader.rs:199-277` 维护 `position` 读取新事件；需要新增“跳至文件末尾”能力以避免 live loop 重复消费已写入的 `work.start`。
- **Preset 配置**：`presets/en/ce-executor-serial.yml`
  - `review-coordinator.publishes` 含 `review.passed`（line 689）。
  - `dimension-reviewer` 未设置 `disallowed_tools`。
  - `topic_deny_rules` 未防御 `review-coordinator → review.passed`。
- **项目配置**：`ralph.yml:43` 注释写为 `builtin:ce-executor-isolated`，实际使用 `ce-executor-serial`；`dimension-reviewer.backend: pi`（line 56-57）。
- **Source 字段**：`crates/ralph-cli/src/commands/emit.rs:60,116-131,746-747` 支持 `--source`，但默认不填；`ralph-core` 的 `JsonlEvent` 与 `Event` 支持 `source` 字段。

### 机构经验

- `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-recovery-2026-06-17.md` 明确“recovery must preserve trigger context”，并给出 double-track 模式：`Event::with_target` 用于路由 + `replay_obligation_triggers_to_activation_state` 用于上下文。
- `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md` 说明 `human.guidance` 不能驱动义务闭合，所有自动恢复必须走 `task.resume`。
- `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-review-chain-2026-06-17.md` 提供 U6 测试模板：smoke replay fixture + BDD silent-reviewer 场景 + `integration_emit_policy.rs` 中的 `test_noble_peacock_executor_review_passed_never_lands`。

---

## Key Technical Decisions

- **KTD-1.** `dimension-reviewer` 固定使用 `claude` 后端。`pi` 是外部不可控后端，无法保证其遵循 serial preset 的 event contract；切换到 `claude` 是最稳的止损方案。
- **KTD-2.** `review.passed` 单所有权归 `review-synthesizer`。`review-coordinator` 的空 diff 快路径改为 emit `review.dimensions.complete`（所有维度标记为 `done` 但无 findings_file），由 synthesizer 统一产出 `review.passed`。这消除了双重发布者，也让 synthesizer 的 verdict gate 始终生效。
- **KTD-3.** `dimension-reviewer` 只读约束使用现有 `HatConfig.disallowed_tools`（`["Bash", "Edit"]`）+ 提示词 HARD RULE；保留 `Write` 用于 findings JSON。`allowed_shell_commands` / `allowed_tools` 在仓库中不存在，不发明新机制。
- **KTD-4.** `work.start` 持久化采用“先写盘、再 skip EventReader”。在创建 `events-{run_id}.jsonl` 与 marker 后，把 `work.start` JSONL 写入文件，并立即将 `EventReader.position` 推进到文件末尾，避免 live loop 把它当作新事件再消费一次。resume 模式复用同一逻辑，不重复注入。
- **KTD-5.** claim-but-no-write hard gate 复用 missing-event 路径的 trigger snapshot/replay 机制，新增 `inject_hard_gate_guidance_with_triggers` 以保持 API 稳定，caller 改为传入 `event_loop.state().last_activation_events`。
- **KTD-6.** HashMap 顺序问题通过审计 + 替换解决：只改“遍历顺序会影响输出/路由/诊断”的位置，纯查找用途的 HashMap 保留。
- **KTD-7.** `source` 字段默认值在 isolated 模式下采用 emitting hat，让业务事件序列化保持一致；不把它加入 `event_policy.schemas` 的 `required_fields`（因为 `source` 是 top-level 而非 payload），但新增集成测试保证输出包含 `source`。

---

## Open Questions

### 已在本计划内解决

- **Q1:** 空 diff 时 `review-coordinator` 不发 `review.passed` 后发什么？
  - **解决：** 发 `review.dimensions.complete`，`dimensions` 数组中每项 `status: done` 且 `findings_file` 为 `null`；synthesizer 随后发 `review.passed(skip_reason=dimensions_complete)`。
- **Q2:** `work.start` 写入 events.jsonl 时用什么 `hat`？
  - **解决：** 不写 `hat`（与 orchestrator 内部 emit 等价），仅写 `topic/payload/ts/source`，`EventReader` 跳过避免重复消费。
- **Q3:** `source` 默认取 emitting hat 是否会影响已有测试？
  - **解决：** 在 isolated 模式 + 业务 topic + 未显式提供 `source` 时启用；单元/集成测试需同步更新断言，计划 U8 负责回归矩阵。

### 保留到实施阶段

- **Q4:** `EventReader` 的“跳至末尾”应作为公开方法还是 runner 内部 helper？
  - 实施时根据 `EventReader` 字段可见性决定，本计划只要求行为契约。
- **Q5:** `work.start` payload 中是否应包含 `loop_id` / `prompt_file`？
  - 实施时与 `EventLoop::initialize_with_topic` 的现有 payload 对齐，不引入新字段。

---

## Implementation Units

### Phase 1 — 预设与项目配置（先落地，改变运行行为）

- [ ] U1. **移除 `review-coordinator` 的 `review.passed` 发布权并改造空 diff 路径**

**Goal:** 让 `review.passed` 仅由 `review-synthesizer` 发布，消除所有权冲突。

**Requirements:** R3

**Dependencies:** 无

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml`
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_serial_review_silent_reviewer_recovers.yml`
- Test: `crates/ralph-core/tests/scenarios.rs`
- Test: `crates/ralph-cli/tests/integration_emit_policy.rs`

**Approach:**
1. 在 `review-coordinator` 中把 `publishes` 从 `["review.dimension.ready", "review.dimensions.complete", "review.passed"]` 改为 `["review.dimension.ready", "review.dimensions.complete"]`。
2. 更新 `obligations`：把 `must_emit_any_of` 与 `conditional_must_emit` 里的 `review.passed` 全部移除，统一为 `review.dimension.ready` / `review.dimensions.complete`。
3. 在 `review-coordinator` instructions 中，把“空 diff 发 `review.passed`”的 step 5 改为：
   - 若序列已无 pending 维度且 diff 为空，emit `review.dimensions.complete`，`dimensions` 数组每项 `status: done`，`findings_file: null`。
   - synthesizer 读到全 done 且无 findings 后，emit `review.passed(skip_reason=dimensions_complete)`。
4. 在 `topic_deny_rules` 增加一道保险：`{hat_id: review-coordinator, topic: review.passed}`。
5. 同步修改两个 BDD scenario 的 `review-coordinator.publishes`，移除 `review.passed`。

**Patterns to follow:**
- 参考 `review-synthesizer` instructions 中已有的 `review.passed(skip_reason=dimensions_complete)` 决策逻辑（line ~1287）。

**Test scenarios:**
- **Happy path:** `review-coordinator` 在空 diff 时 emit `review.dimensions.complete` → scenario 仍到达 `LOOP_COMPLETE`。
- **Error path:** `review-coordinator` 尝试 emit `review.passed` 时，CLI precheck 或 runtime origin guard 拒绝，返回 `isolated_scope_violation` / `out-of-scope topic`。
- **Integration:** `cargo nextest run -p ralph-core --test scenarios ce_executor_serial` 对基础场景和 silent-reviewer 场景均通过。

**Verification:**
- `ralph preset check --strict -H builtin:ce-executor-serial` 通过。
- 两个 serial BDD scenario 通过。
- 新增/更新的 integration test 断言 `review-coordinator` 发 `review.passed` 时被拒。

---

- [ ] U2. **把 `dimension-reviewer` 锁死在只读角色**

**Goal:** 防止 reviewer 执行测试、构建或修改源码。

**Requirements:** R2

**Dependencies:** 无

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`
- Test: `crates/ralph-cli/tests/integration_emit_policy.rs`
- Test: `crates/ralph-cli/src/loop_runner/tests.rs`

**Approach:**
1. 在 `dimension-reviewer` 配置中增加 `disallowed_tools: ["Bash", "Edit"]`。
2. 在 `dimension-reviewer` instructions 开头增加 HARD RULE 块：
   - “你是只读代码 reviewer。禁止运行任何 shell 命令（包括 `cargo test`、`cargo nextest`、`cargo build`、`cargo clippy`）。禁止修改源码。你唯一能写的文件是 findings JSON。”
   - “验证是 executor/shipper 的职责，不是你的。”
3. 保留 `Write` 不在 `disallowed_tools` 中，以便产出 `.agents/scratchpad/ce-executor/{plan_name}/findings-{dimension}-{task_id}.json`。

**Patterns to follow:**
- `crates/ralph-core/src/config/hat.rs:439-445` 已定义 `disallowed_tools`，且对 `Edit`/`Write` 有迭代后硬审计。

**Test scenarios:**
- **Happy path:** `ralph preset check --strict -H builtin:ce-executor-serial` 通过；`dimension-reviewer` 配置可反序列化出 `disallowed_tools`。
- **Error path:** 在 mock run 中若 `dimension-reviewer` 调用了 `Edit` 或 `Bash`，runner 应产出 `scope_violation` / `task.resume` 并记录到 `recovery.jsonl`。
- **Integration:** 集成测试验证 `dimension-reviewer` 通过 `ralph emit review.dimension.done` 写入 findings 文件的事件可正常通过；而带 `Bash`/`Edit` 的迭代被审计拒绝。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- disallowed` 或对应子集通过。
- 手动 `ralph run -H builtin:ce-executor-serial --dry-run`（或等效命令）加载 preset 不报错。

---

- [ ] U3. **修正 `ralph.yml` 的 preset 注释与 `dimension-reviewer` 后端**

**Goal:** 让项目配置与实际运行 preset 一致，并切断 `pi` 后端。

**Requirements:** R4

**Dependencies:** 无

**Files:**
- Modify: `ralph.yml`
- Test: 手动验证 / `crates/ralph-cli/tests/config_load.rs`（若存在）

**Approach:**
1. 把第 43 行注释改为 `# Per-hat backend overrides for builtin:ce-executor-serial (merged on top of preset).`
2. 把第 44 行注释改为 `# Implementation chain → claude; review / gate / ship / report → claude (pi disabled for dimension-reviewer due to event contract drift).`
3. 把 `dimension-reviewer.backend` 从 `pi` 改为 `claude`（或删除该 override 使其继承 `cli.backend: claude`；为可读性建议显式写 `claude`）。

**Test scenarios:**
- **Happy path:** 加载 `ralph.yml` 后，`config.hats["dimension-reviewer"].backend.to_cli_backend() == "claude"`。
- **Regression:** `pi` 不再作为 `dimension-reviewer` 后端出现在有效配置中。

**Verification:**
- 运行 `ralph config show`（或等效）或单元测试，确认 `dimension-reviewer` 后端为 `claude`。
- 实际运行一次 `ralph run -H builtin:ce-executor-serial` 时，不再 spawn `~/.pi/agent` 处理 reviewer。

---

### Phase 2 — Runner 恢复与启动事件持久化

- [ ] U4. **扩展 claim-but-no-write hard gate，使其携带原始触发上下文**

**Goal:** 当 `dimension-reviewer` 声称 emit 了 `review.dimension.done` 但实际上没写事件时，注入的 `task.resume` 能让它在下一迭代回到正确的 `dimension`。

**Requirements:** R1

**Dependencies:** 无（但最好与 U1 一起验证，因为都改变 serial review 链）

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner/hard_gate.rs`
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`
- Test: `crates/ralph-cli/src/loop_runner/tests.rs`
- Test: `crates/ralph-core/tests/scenarios/ce_executor_serial_review_silent_reviewer_recovers.yml`（如需要扩展）

**Approach:**
1. 在 `hard_gate.rs` 中新增 `inject_hard_gate_guidance_with_triggers(ctx, event_loop, hat_id, expected_topics, obligation_triggers)`，内部复用 `inject_hard_gate_guidance` 的 JSONL 写入与 `pending_recovery_hat` pinning，但额外：
   - 调用 `enrich_task_resume_payload_with_stage(..., Some(RejectionStage::EmitClaimedButNotWritten))`（若 `RejectionStage` 无该变体则新增）。
   - 在 payload 中写入 `original_trigger_topic` 与 `original_trigger_payload`。
   - JSONL 记录增加顶层的 `target: <hat_id>`，与 missing-event 路径一致。
2. 在 `runner.rs` 的 claim-but-no-write 调用点（`inject_hard_gate_guidance` 调用处，line ~4054）改为：
   - 先 snapshot `event_loop.state().last_activation_events.clone()`。
   - 调用 `inject_hard_gate_guidance_with_triggers(..., &triggers)`。
   - 随后调用 `event_loop.state_mut().replay_obligation_triggers_to_activation_state()`（参考 missing-event 路径 line ~4267-4277）。
3. 保留旧的 `inject_hard_gate_guidance` 作为 wrapper（可标记 `#[deprecated]` 或保留给无 trigger 的 legacy caller），保持现有测试编译。

**Technical design:**
> 方向性示意：claim-but-no-write 路径的 resume payload 结构与 missing-event 路径对齐，仅在 `stage` 与 `reason` 上区分。

**Patterns to follow:**
- `inject_missing_event_hard_gate_guidance_with_triggers` 已实现的 snapshot/replay 模式（`hard_gate.rs:644-828`）。
- `Event::with_target`  routing 模式（已用于 missing-event 路径）。

**Test scenarios:**
- **Happy path:** `dimension-reviewer` 声称 emit 但无事件 → 注入的 `task.resume` payload 含 `original_trigger_topic=review.dimension.ready`、`original_trigger_payload.dimension=testing`、`target=dimension-reviewer`、`stage=emit_claimed_but_not_written`。
- **Edge case:** 无 `last_activation_events`（legacy/异常路径）→ 仍产生合法 `task.resume`，只是不含 `original_trigger_*` 字段。
- **Error path:** 未 replay trigger 时，下一迭代的 `should_gate_missing_events` 无法评估 obligation，导致连续 gate；测试验证 replay 后 gate 不再误触发。
- **Integration:** BDD silent-reviewer 场景断言第一次 silent 后能在第二次激活发出正确的 `review.dimension.done`。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- inject_hard_gate` 子集通过。
- 新增/更新 test 在 `loop_runner/tests.rs` 中覆盖 trigger embedding 与 replay。

---

- [ ] U5. **把配置的 `starting_event` 持久化到 `.ralph/events-*.jsonl`**

**Goal:** 让 `work.start` 进入可信事件流，便于 `ralph diagnose`、replay 与审计，同时避免 live loop 重复消费。

**Requirements:** R5

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`
- Modify: `crates/ralph-core/src/event_reader.rs`（新增 skip-to-end helper）
- Test: `crates/ralph-cli/src/loop_runner/tests.rs`
- Test: `crates/ralph-core/tests/smoke_runner.rs`（replay fixture 应包含 `work.start`）

**Approach:**
1. 在 `runner.rs` 创建 `events-{run_id}.jsonl` 与 marker 之后、`EventLoop::with_context` 之前：
   - 解析 `config.event_loop.starting_event`（默认 `task.start`，serial preset 下为 `work.start`）。
   - 用与 `ralph emit` 相同的 JSONL 形状写入一行：`{topic, payload, ts, source: "loop-bootstrap"}`，不写 `hat`（与 orchestrator 内部 emit 等价，可被 origin guard 接受）。
2. 在 `EventReader` 新增 `fn skip_to_end(&mut self) -> io::Result<()>`，把 `self.position` 设为文件长度。
3. `EventLoop` 新增包装方法 `fn skip_event_reader_to_end(&mut self)`（或暴露 `event_reader_mut`）。
4. 在 `EventLoop` 构造完成后立即调用 `skip_event_reader_to_end`，再调用 `event_loop.initialize(...)`。这样 `EventReader` 不会把刚写入的启动事件再读回 bus。
5. `--continue` / resume 模式不重复注入新的 `work.start`；resume 使用 `task.resume`，由 `initialize_resume` 处理。

**Technical design:**
> 方向性示意：启动事件写盘与 EventReader 跳过是一对原子化操作，保证“持久化”与“不重复消费”同时成立。

**Patterns to follow:**
- `hard_gate.rs` 中已有向 `resolve_current_events_path(ctx)` 追加 JSONL 的实现。
- `EventReader::read_new_events` 以 `position` 为边界的模式。

**Test scenarios:**
- **Happy path:** 新鲜 loop 启动后，`.ralph/events-{run_id}.jsonl` 第一行为 `work.start`，且 `EventReader.position` 等于文件长度。
- **Edge case:** resume 模式下不新增 `work.start` 行。
- **Error path:** 若跳过失败，runner 应至少 warn，且不应导致 `work.start` 被 process 两次。
- **Integration:** smoke replay fixture 包含 `work.start` 作为首行仍能正常 replay。

**Verification:**
- 单元测试断言 events 文件内容与 EventReader 位置。
- `cargo nextest run -p ralph-core --features recording --test smoke_runner` 对含 `work.start` 的 fixture 不 panic。

---

### Phase 3 — 稳定性与序列化收尾

- [ ] U6. **审计并消除路由/验证中的 HashMap 顺序非确定性**

**Goal:** 防止因迭代顺序不同导致 hat 路由、验证输出或诊断结果不稳定。

**Requirements:** R6

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-core/src/event_policy.rs`（如需要）
- Test: `crates/ralph-core/src/event_loop/tests/*.rs` 或新增 `crates/ralph-core/tests/deterministic_routing.rs`

**Approach:**
1. 扫描 `event_loop/mod.rs` 与 `event_policy.rs` 中所有 `HashMap` 的 `iter()` 使用：
   - 仅用于查找的保留。
   - 用于生成列表、路由决策、诊断输出或验证顺序的，改为 `BTreeMap<String, Vec<String>>` 或在遍历前 `collect::<Vec<_>>().sort_by_key(...)`。
2. 重点关注：
   - `source_hats_by_topic` / `target_hats_by_topic`（line ~7310-7313）虽然当前用于查找，但若未来被遍历会影响归因输出；统一改为 `BTreeMap`。
   - 任何 `for (hat_id, _) in &self.config.hats` 遍历（配置解析顺序本身由 YAML 决定，但运行时集合应排序）。
3. 新增或扩展一个“跑 N 次结果一致”的回归测试。

**Test scenarios:**
- **Happy path:** 同一输入运行 10 次，`next_hat()` 返回顺序、诊断输出、`policy_rejections` 顺序完全一致。
- **Edge case:** 在 hat 数量、topic 数量变化后仍保持确定性。
- **Regression:** 现有并行测试不因此变慢。

**Verification:**
- 新增 determinism test 通过。
- `./scripts/run-tests.sh` 全量通过（无新 flake）。

---

- [ ] U7. **规范业务事件的 `source` 字段序列化**

**Goal:** 消除 `review.dimension.ready` 等事件缺失 `source` 字段的序列化不一致。

**Requirements:** R7

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-cli/src/commands/emit.rs`
- Modify: `presets/en/ce-executor-serial.yml`（在 dimension-reviewer emit 示例中显式展示 `--source`）
- Test: `crates/ralph-cli/tests/integration_emit_policy.rs`

**Approach:**
1. 在 `emit.rs` 的 `resolve_provenance` 或写 record 阶段：当 `config.event_loop.execution_mode == Isolated`、topic 为业务 topic、且 `source` 仍为 `None` 时，把 `source` 默认设为 `hat`（如果 `hat` 已知）。
2. 对控制 topic（`RALPH_CONTROL_TOPICS`）保持现有行为不变，避免影响 orchestrator 内部事件。
3. 在 `dimension-reviewer` instructions 的 emit 示例中增加 `--source dimension-reviewer`。
4. 更新 `ce-executor-serial` 的 BDD scenario mock response，为 `review.dimension.ready` 等事件增加 `source="dimension-reviewer"` 属性（如果 scenario parser 支持）。

**Test scenarios:**
- **Happy path:** `ralph emit review.dimension.ready --hat dimension-reviewer ...` 写出的 JSONL 包含 `"source":"dimension-reviewer"`。
- **Edge case:** 用户显式 `--source cli` 时优先使用用户值。
- **Regression:** 控制 topic（如 `loop.cancel`）的 `source` 行为不变。

**Verification:**
- 新增 integration test 断言 business topic 默认 source 为 hat。
- 现有 `emit.rs` 单元测试中涉及 source 的断言同步更新。

---

### Phase 4 — 回归测试矩阵与文档

- [ ] U8. **补齐测试矩阵、BDD 配置与运行诊断文档**

**Goal:** 所有修复都有回归保护，operator 能根据文档排查同类故障。

**Requirements:** R1–R7

**Dependencies:** U1–U7

**Files:**
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml`
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_serial_review_silent_reviewer_recovers.yml`
- Modify: `crates/ralph-core/tests/scenarios.rs`
- Modify: `crates/ralph-cli/tests/integration_emit_policy.rs`
- Modify: `crates/ralph-cli/src/loop_runner/tests.rs`
- Modify: `docs/guide/runtime-diagnosis.md`（如存在）
- Modify: `AGENTS.md` / `CLAUDE.md` 中 Presets & Hats 段的行为描述（如有变化）

**Approach:**
1. 更新两个 BDD scenario 中 `review-coordinator.publishes`，移除 `review.passed`。
2. 扩展 `ce_executor_serial_review_silent_reviewer_recovers.yml`：增加一个 iter，模拟 DR “claim emit 但没写” 后由 U4 修复机制恢复的场景。
3. 在 `integration_emit_policy.rs` 新增：
   - `test_ce_executor_serial_dimension_reviewer_bash_edit_rejected`
   - `test_ce_executor_serial_coordinator_review_passed_rejected`
   - `test_work_start_persisted_to_events_jsonl`
4. 在 `loop_runner/tests.rs` 新增：
   - `test_hard_gate_guidance_embeds_original_trigger`
   - `test_event_reader_skips_bootstrap_start_event`
5. 在 `runtime-diagnosis.md` 增加一节：
   - “Serial review 链 recovery 形状”：说明 `task.resume` 应含 `original_trigger_topic/payload` 与 `stage`。
   - “`work.start` 未进入 events.jsonl” 排查：检查第一行是否有 `work.start`。
6. 若 `AGENTS.md` / `CLAUDE.md` 的 Presets & Hats 段提到 `ce-executor-serial` 行为，同步更新（预设名称未变，不更新 zsh 补全）。

**Test scenarios:**
- **Happy path:** 完整测试矩阵全部通过：
  - `cargo nextest run -p ralph-core --test scenarios ce_executor_serial`
  - `cargo nextest run -p ralph-core --features recording --test smoke_runner -- noble_peacock`
  - `cargo nextest run -p ralph-cli --bin ralph -- inject_hard_gate`（或对应子集，×3 防 flake）
  - `./scripts/run-tests.sh`
- **Error path:** 故意把任一修复回退一个，对应 regression test 失败。

**Verification:**
- `./scripts/run-tests.sh` 全绿。
- `cargo test --doc --workspace --exclude ralph-e2e` 通过（doctest）。
- 手动运行一次 `ralph run -H builtin:ce-executor-serial -p docs/plans/xxx.md` 冒烟（可选，但推荐）。

---

## System-Wide Impact

- **Interaction graph：**
  - `review-coordinator` 不再发 `review.passed`，下游 `plan-gate` 的 trigger 不变（仍订阅 `review.passed`），但事件来源唯一化为 `review-synthesizer`。
  - `dimension-reviewer` 的 `Bash`/`Edit` 被禁后，任何越权调用会触发 `scope_violation` → `task.resume` → 同 hat 重试。
  - `work.start` 进入 events.jsonl 后，`ralph diagnose` 与 replay fixture 都能读取，但 live loop 通过 EventReader skip 避免重复消费。
- **Error propagation：**
  - claim-but-no-write 路径若未正确 replay trigger，会导致连续 hard gate；U4 的测试必须验证 gate 不再二次触发。
  - 空 diff 路径若 synthesizer 未正确处理 `findings_file: null`，会导致 `review.dimensions.complete` schema 失败；U1 的 BDD 验证覆盖。
- **State lifecycle risks：**
  - `EventReader.position` 跳过启动事件后，resume 模式不应再次 skip；需在 `initialize_resume` 路径排除。
  - `source` 默认改为 emitting hat 后，所有 isolated 模式 business topic 的 JSONL 都会多一个字段，可能影响严格比较 JSON 字符串的测试；U8 负责同步。
- **Unchanged invariants：**
  - 10-hat 拓扑、topic 集合、`event_policy.schemas` 的 payload `required_fields` 不变（除 scenario 配置外）。
  - `human.guidance` 仍仅用于 operator/RObot，不用于自动恢复。
  - wave preset 行为不受本次修改影响。

---

## Risks & Dependencies

| Risk | 影响 | 缓解 |
|------|------|------|
| U1 改 `review-coordinator` obligations 后，现有空 diff 用例路径断裂 | 中 | 两个 BDD scenario + 新增 integration test 覆盖空 diff → `review.dimensions.complete` → `review.passed` 全链 |
| `dimension-reviewer` 切到 `claude` 增加 API 成本 | 低-中 | 这是稳定性优先的决策；reviewer 维度最多 4 个 per step，成本可控 |
| `work.start` 写盘 + skip 逻辑在 resume 模式出错，导致重复或丢失 | 中 | U5 单独增加 resume 路径测试；`initialize_resume` 明确不注入新的 `work.start` |
| `source` 默认值改动影响大量 emit 单元测试 | 低 | 只在 isolated + business topic + source 未显式提供时启用；U8 同步更新断言 |
| HashMap 改 BTreeMap 引入性能退化 | 低 | 仅替换遍历用集合，查找仍 O(log n) 且 hat/topic 数量很小 |

---

## Documentation / Operational Notes

- 更新 `docs/guide/runtime-diagnosis.md`：新增 “Serial review recovery shape” 与 “`work.start` persistence” 排查节。
- 若 `AGENTS.md` / `CLAUDE.md` 中对 `ce-executor-serial` 行为有描述，同步更新； preset 名称未变，不更新 zsh 补全。
- 在 `presets/en/ce-executor-serial.yml` 的 `dimension-reviewer` 与 `review-coordinator` instructions 中，用注释或 HARD RULE 固化本次决策，防止未来提示词漂移。

---

## Sources & References

- **Origin docs:**
  - `docs/brainstorms/2026-06-17-agent-recovery-mechanism-gaps-requirements.md`
  - `docs/brainstorms/2026-06-16-ce-executor-bootstrap-recovery-requirements.md`
- **Solutions / prior art:**
  - `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-recovery-2026-06-17.md`
  - `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md`
  - `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-review-chain-2026-06-17.md`
- **Key code:**
  - `crates/ralph-cli/src/loop_runner/hard_gate.rs`
  - `crates/ralph-cli/src/loop_runner/runner.rs`
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/event_reader.rs`
  - `crates/ralph-cli/src/commands/emit.rs`
  - `presets/en/ce-executor-serial.yml`
  - `ralph.yml`
- **Tests:**
  - `crates/ralph-cli/src/loop_runner/tests.rs`
  - `crates/ralph-core/tests/scenarios.rs`
  - `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml`
  - `crates/ralph-core/tests/scenarios/ce_executor_serial_review_silent_reviewer_recovers.yml`
  - `crates/ralph-cli/tests/integration_emit_policy.rs`
  - `crates/ralph-core/tests/smoke_runner.rs`
