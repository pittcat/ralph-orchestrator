---
title: 修复 Isolated Wave 稳定性并增加 Progress Steward 兜底机制
type: fix
status: completed
date: 2026-06-16
origin: .worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-sunny-lotus/.ralph/agent/progress.md
---

# 修复 Isolated Wave 稳定性并增加 Progress Steward 兜底机制

## 概述

在 `ce-executor-isolated` preset 的 Wave Review 流程中，7 个 dimension-reviewer 工人并行产出 `review.dimension.done` 后，事件循环的 per-turn 业务事件预算错误地把大部分 wave 事件当成“额外业务事件”丢弃，导致 `review-synthesizer` 聚合器永远收不齐信号、loop 进入僵死，最终被手动停止。

本计划从三个层面解决：
1. **修机制（P0）**：修复 `event_loop/mod.rs` 的 per-turn 预算逻辑，让同一 wave 的事件原子通过。
2. **减维度**：把 review 维度从 7 个降到 4 个，降低 wave 并行度、超时概率和事件风暴。
3. **加兜底**：新增 `progress-steward` hat 和运行时 fallback，当正常 hat 卡住时由 steward 读取状态并决定“下一步交给谁”，让 loop 继续向 plan 目标推进，而不是突然中断。

---

## 问题定义

### 直接触发原因

实际运行中（见 `.worktrees/...sunny-lotus/.ralph/`）：

- Round 1 wave `w-18b99d1f6ba75040-26527-0`：7 维中 worker 0 超时，仅 6 个成功报告。
- Round 2 wave `w-18b99f42e17797d8-86489-0`：7 个工人都产出了 findings 文件，但 `.ralph/events-20260616-161905.jsonl` 中只残留 3 条 `review.dimension.done`，其中 2 条缺少 `wave_id`。
- 诊断日志反复出现 `Isolated mode: extra business event dropped — only one per turn`。
- `review-synthesizer` 因聚合事件 incomplete 而 stalled，loop 最终把 50 分钟前 executor 误发 `debug.step` 的旧 rejection 重新包装为 `task.resume` 投入 executor，导致 executor 连续发 `work.failed`。

### 根因定位

- `event_loop/mod.rs:6843-6900` 的 `same_wave_continuation` 逻辑要求“本轮第一个业务事件的 `wave_id`”与后续事件匹配。当第一个事件没有 `wave_id` 时，`first_wave_id_accepted` 被设为 `Some(None)`，后续所有带 `wave_id` 的同一 wave 事件都被判为 false，从而被丢弃。
- `wave/io.rs:344-355` 把 `wave.worker.failed` 合成事件的 `hat`/`source` 设为 `default_source_hat`（即 `review-coordinator`），但 `review-coordinator` 的 `publishes` 不含 `wave.worker.failed`。
- `event_loop/rejection.rs` / `mod.rs` 在构建 `task.resume` 时没有检查 rejection 时间戳或目标 task 状态。

### 设计层面的缺口

- **7 维 review 过重**：对一个 refactor 任务来说，7 个维度并行容易超时、容易产生事件合并噪音。
- **没有 loop 级兜底**：任何 hat（包括未来其他 preset 的 hat）一旦卡住或犯错，loop 没有更高层的角色来打破僵局，只能依赖 human 手动停止。

---

## 需求追溯

- **R1.** Isolated 模式下，同一 `wave_id` 的所有 wave 结果事件应在同一轮内全部进入事件总线，不被 per-turn 业务事件预算丢弃。
- **R2.** `wave.worker.failed` 合成事件必须使用合法的 source hat，避免 origin guard 拒绝和无效 `task.resume` 注入。
- **R3.** `task.resume` 注入应具备 freshness 检查，不应对已关闭的 task 或过期 rejection 重新激活。
- **R4.** `ce-executor-isolated` 的 review 维度从 7 个裁剪到 4 个核心维度，减少 wave 并行度和失败概率。
- **R5.** 新增 `progress-steward` hat + 运行时 fallback 机制：当 loop 检测到僵局时，steward 读取当前状态并 emit 最小合法事件，把 loop 推回正常轨道或干净结束。
- **R6.** 所有改动需通过 `ralph-core` / `ralph-cli` 的 nextest 回归测试，`ce-executor-isolated` preset lint 通过，且 `presets/schemas/ce-executor-isolated.yml` 与 inline schemas 同步。

---

## 范围边界

- **在范围内**：
  - `event_loop/mod.rs` 的 isolated per-turn budget
  - `wave/io.rs` 的合成事件 provenance
  - `event_loop/rejection.rs` 的 resume freshness
  - `ce-executor-isolated` preset 的维度裁剪与 `progress-steward` hat 定义
  - 运行时 `EventLoopConfig` 的 `progress_steward` 配置
  - `presets/schemas/ce-executor-isolated.yml` 与 `presets/en/ce-executor-isolated.yml` inline schemas 同步
  - 对应单元测试与回归测试
- **不在范围内**：重做整个 wave 架构、移除 isolated mode、修改后端执行器（`CliExecutor` / `PtyExecutor`）的非 wave 路径、改动 `ralph` CLI 的顶层命令。
- **Deferred 到后续**：把 `progress-steward` 机制抽象成跨 preset 通用运行时策略（本次先在 `ce-executor-isolated` 验证）。

---

## 背景调研

### 相关代码

- `crates/ralph-core/src/event_loop/mod.rs:6420-6952` — isolated 模式下单轮业务事件预算与 scope enforcement。
- `crates/ralph-core/src/event_loop/mod.rs:6843-6900` — `same_wave_continuation` 与 `first_wave_id_accepted` 逻辑（问题核心）。
- `crates/ralph-cli/src/loop_runner/wave/io.rs:227-414` — wave 结果合并到主事件文件，含 `wave.worker.failed` 合成事件。
- `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:1520-1698` — wave rejection 处理。
- `crates/ralph-core/src/event_loop/rejection.rs` — `build_task_resume_payload` 与 rejection 注入。
- `crates/ralph-core/src/config/event_loop.rs`（或等价位置）— `EventLoopConfig` 定义。
- `presets/en/ce-executor-isolated.yml` — hats、triggers、publishes、instructions。
- `presets/schemas/ce-executor-isolated.yml` — schema SSOT。
- `crates/ralph-cli/src/presets.rs` — 内嵌 preset 元数据。

### 机构知识

- `docs/solutions/` 中已有多次 wave / isolated mode 相关修复记录；本次应复用现有 `RecoveryDiagnosisEnvelope` 与 retry_key 命名规范。
- `ce-executor-isolated` preset 的 `event_policy.on_violation: reject_with_resume` 要求 mechanism 层的 recovery 信号必须精准，否则会放大噪音。

---

## 关键技术决策

### 1. Wave 事件组与非 wave 业务事件分离计数

原逻辑用 `first_wave_id_accepted: Option<Option<String>>` 跟踪“第一个业务事件的 wave_id”，导致无 wave_id 事件破坏后续 wave 组。

改为同时维护：
- `non_wave_business_event_accepted: bool`
- `accepted_wave_id: Option<String>`

同一 `wave_id` 的事件始终作为一个整体被接受；无 `wave_id` 的单事件单独占一个 slot。`is_dual_publish_step_handoff` carve-out 用于 `queue.advance` + `work.ready` 双发：允许同一轮内 `queue.advance` 之后再接受一个 `work.ready`，因为这是一个 handoff 步骤，不是两个独立业务事件。

### 2. `wave.worker.failed` 的 source hat

把合成事件的 `"hat"` / `"source"` 从 `default_source_hat`（`review-coordinator`）改为 `"review-synthesizer"`，因为 synthesizer 是 wave 结果的消费者，与 `plan.blocked` 有天然关联。同时在 preset 和 schema SSOT 中给 `review-synthesizer` 增加 `wave.worker.failed` 并定义其 schema。

### 3. `task.resume` freshness

在注入 `task.resume` 前检查 rejection 时间戳，超过 `task_resume_ttl_seconds`（默认 300s，可配置）直接丢弃。同时增加目标 task 状态检查：若 task 已 closed 且 rejection 与当前任务无关，也丢弃。

### 4. Review 维度裁剪

从 7 维裁剪到 4 维：

| 保留维度 | 理由 |
|---|---|
| `correctness` | 逻辑正确性、边界、错误传播 |
| `testing` | 本次 refactor 对象是 tests，测试质量必审 |
| `maintainability` | 拆分后的结构、死代码、过度抽象 |
| `requirements` | 确保实现与 plan.md U-ID 对齐 |

- `work.done` 首轮默认 review 删除：`standards`（可合并到 maintainability/requirements）、`agent-native`（内部 refactor 不新增 agent 接口）、`learnings`（advisory，信噪比低）。
- `standards` 保留在 `fix.applied` 触发路径使用，因为 fix 可能引入格式化/风格回归。

### 5. Schema SSOT 与 inline schemas 同步

`presets/schemas/ce-executor-isolated.yml` 是 `event_policy.schemas` 的 authoring SSOT，`build.rs` 在编译时 deep-merge 到内嵌 preset；但 `presets/en/ce-executor-isolated.yml` 仍保留一个 inline `schemas:` 覆盖层。本次改动新增/调整以下 topic，必须**同时更新两处**，避免 SSOT 与 inline 不一致导致 lint 或运行时 contract 失败：

| Topic | 引入单元 | 变更 | Schema 位置 |
|---|---|---|---|
| `wave.worker.failed` | U2 | 新增 topic，`required_fields: [reason, wave_id, wave_index]`，`payload: json_object` | `presets/schemas/ce-executor-isolated.yml` + inline `schemas:` |
| `loop.stalled` | U5 | 新增 diagnostic topic，`required_fields: [reason]`，`payload: json_object` | `presets/schemas/ce-executor-isolated.yml` + inline `schemas:` |
| `task.resume` | U3/U5 | 补充 schema：`required_fields: [reason, target_hat]`，`payload: json_object`；runtime 无法从 Rejection 重建 task id，故去掉 `target_task_id` 和 `source_event_id` | `presets/schemas/ce-executor-isolated.yml` + inline `schemas:` |
| `human.guidance` | U5 | 如当前未在 schema 中定义，补充最小 schema：`required_fields: [message]`，`payload: json_object`；如为系统 topic 免检，需在 preset 注释中显式说明 | `presets/schemas/ce-executor-isolated.yml` + inline `schemas:` |
| `review.wave.ready` | U4/U5 | 字段不变，但 steward 会复用；确保 idempotency key 字段不要求为 required | 检查现有 schema 是否兼容 |

**验证命令**：
- `ralph preset check builtin:ce-executor-isolated`
- `cargo build -p ralph-cli`（触发 `build.rs` 的 schema merge 并编译内嵌 preset）

### 6. Progress Steward 机制

#### 运行时层

在 `EventLoopConfig` 增加：

```yaml
event_loop:
  task_resume_ttl_seconds: 300         # rejection freshness TTL，U3/U5 共用
  progress_steward:
    enabled: true
    steward_hat_id: "progress-steward"   # fallback target
    max_steward_iterations: 3            # 连续激活上限
```

行为：
- 当 `stall_recovery` / `missing_event_gate` 要注入 `task.resume` 时，按以下决策矩阵路由：
  - **rejection 已过期（> TTL）** → 丢弃，不发 `task.resume`。
  - **源 hat 的 `publishes` 包含 rejection 原 topic，且目标 task 仍 open** → 路由给源 hat（正常恢复）。
  - **violation 为 hat scope 不允许、或目标 task 已 closed、或源 hat 无法安全恢复** → 改路由给 `steward_hat_id`。
- 连续 `max_steward_iterations` 轮没有 accepted 业务事件，自动 emit `loop.stalled` 诊断事件并强制唤醒 steward。
- steward 连续激活 `max_steward_iterations` 次仍无进展，强制 emit `plan.blocked(reason=loop_stalled_max_iterations)` 干净结束。
- **steward 自保护**：steward 自己 emit 的事件（无论 origin 是谁）不再触发新一轮 steward 路由；steward 只对非 steward 源头的 recovery 信号响应。这样即使 steward emit 非法 topic 被 origin guard 拒绝，也不会形成自循环。

#### Preset 层

新增 hat：

```yaml
progress-steward:
  name: "🛟 Progress Steward"
  triggers: ["loop.stalled", "human.guidance"]
  publishes: ["work.ready", "queue.advance", "review.wave.ready", "task.resume", "plan.blocked"]
```

> **说明**：review 后发现 `task.resume` 是 ralph pseudo-hat 的保留 trigger、`plan.blocked` 与 shipper 路由冲突，故实际实现保留 `[loop.stalled, human.guidance]`，steward 通过 `loop.stalled` 被 runtime 唤醒。

#### Steward 决策树与 handoff

Steward 被唤醒后读取 `plan.md`、`progress.md`、`tasks.jsonl`、`events.jsonl`，然后：

| 当前状态 | Steward emit | 下一个 Hat | 说明 |
|---|---|---|---|
| 当前 step 有 open task，但 executor 无响应 | `work.ready(...)` | executor | 把任务派给 executor |
| 当前 step task 已 closed，但 review 一直没聚合 | `review.wave.ready(...)` | review-coordinator → dimension-reviewer wave | 用 4 维重新 kick review |
| review 已通过/失败，但 plan-gate 没推进 | `queue.advance` 后接 `work.ready` | plan-gate 记录 → executor 下一步 | 标准双发；必须同一轮内按 `queue.advance` → `work.ready` 顺序发出，以命中 U1 的 `is_dual_publish_step_handoff` carve-out |
| 所有 step 已完成，但 plan-gate 没发 `plan.complete` | `task.resume` 给 plan-gate | plan-gate | 提示它 emit `plan.complete` |
| 无法判断安全动作，或已达 max_steward_iterations | `plan.blocked(reason=loop_stalled_max_iterations)` | shipper → reporter | 干净结束，产出报告 |

#### Steward 安全约束

- 不订阅正常业务事件，只在 stall/recovery 路径激活。
- 每次 emit 必须带 `reason` payload，写入 `recovery.jsonl` 和 `progress.md`。
- 同一轮次内 steward 自身不能成为唯一被激活的 hat 超过 `max_steward_iterations` 次。
- steward 的 `review.wave.ready` 必须使用 idempotency key，避免无限重发 wave。采用 `ce-review:{plan_name}:{task_id}:{step}:steward-round-{N}`（N 为 steward 本次激活计数），与 review-coordinator 的 `ce-review:{plan_name}:{task_id}:{step}:round-{fix_round}` 不冲突。
- steward 虽然订阅 `plan.blocked`，但 `plan.blocked` 是终态事件；一旦 emit，loop 进入结束流程，不会再次唤醒 steward。

---

## 待决问题

### 规划中已解决

- **Q1.** 是否把 `dimension-reviewer.concurrency` 改成 1 来绕过 wave？
  - 不可行：`wave_detection.rs:284` 会在 `concurrency <= 1` 时返回 `SequentialTarget`，dispatcher 直接拒绝 wave。
- **Q2.** `wave.worker.failed` 用哪个 hat 作为 source？
  - 选择 `review-synthesizer`。
- **Q3.** steward 能不能直接 emit `review.wave.ready`？
  - 可以，这是打破 review 卡住最直接的方式。
- **Q4.** steward 能不能直接 emit `plan.complete`？
  - 不给，先 `task.resume` 给 plan-gate；若 plan-gate 仍不动，再 `plan.blocked` 干净结束。

### 实现中再确认

- **Q5.** `task_resume_ttl_seconds` 默认值 300s 是否合适？
  - 合适。`loop_runner/tests.rs` 的时序测试使用 500ms sleep，300s >> 500ms；针对 TTL 过期路径的单元测试应显式构造过期 rejection 或临时把 TTL 设短。
- **Q6.** `wave.worker.failed` 的 payload 当前是字符串，改为 JSON object 后是否影响现有测试断言？
  - U2 负责更新 `loop_runner/tests.rs` 中断言；若测试只检查事件存在性则不受影响，若检查 payload 字符串则同步改为 JSON object。
- **Q7.** steward 的 `review.wave.ready` idempotency key 公式？
  - 使用 `ce-review:{plan_name}:{task_id}:{step}:steward-round-{N}`，N 为 steward 本次 loop 中的激活计数；与 review-coordinator 的 `round-{fix_round}` 不冲突。

---

## 实现顺序

建议按以下顺序合并，以控制集成风险：

1. **U1 + U2 + U3 + U4 可并行开发**，彼此无代码依赖；但 U4 的测试验证建议等 U1 合并后再跑完整 wave，避免维度减少掩盖事件丢弃问题。
2. **U5 在 U1、U3、U4 之后合并**：先让 wave 基础路径和 freshness 稳定，再叠加 steward 兜底。
3. **U6 最后跑全量回归**：同步 schema/manifest/embedded preset，补解决方案文档，跑 `./scripts/run-tests.sh`。

## 实现单元

- [ ] U1. **修复 isolated 模式 per-turn 业务事件预算的 wave 组处理**

**目标：** 让同一 `wave_id` 的所有事件在同一轮内全部进入事件总线，不被非 wave 事件阻断。

**需求：** R1

**依赖：** 无

**文件：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`
- 测试：优先扩展 `crates/ralph-core/src/event_loop/tests/wave_isolated_scope.rs`；若现有测试结构不适合新增场景，再新建 `crates/ralph-core/src/event_loop/tests/isolated_wave_budget.rs`

**方案：**
- 在 `process_parse_result` 的 isolated 分支中，把 `first_wave_id_accepted: Option<Option<String>>` 替换为显式状态机：
  - `non_wave_business_event_accepted: bool`（是否已接受一个非 wave 业务事件）
  - `accepted_wave_id: Option<String>`（已接受的 wave 组 id）
- **预算只针对业务事件**；diagnostic / rejection / `loop.stalled` / `task.resume` 等 recovery 信号不计入本预算。
- 事件处理规则（同一轮内）：
  1. **有 `wave_id` 且等于 `accepted_wave_id`** → 允许（同一 wave 组成员）。
  2. **有 `wave_id` 且 `accepted_wave_id` 为 `None`** → 允许（新 wave 组开始），设置 `accepted_wave_id`。
  3. **有 `wave_id` 且 `accepted_wave_id` 为另一个 id** → drop（防止多 wave 抢道）。
  4. **无 `wave_id` 的业务事件，且 `non_wave_business_event_accepted == false`** → 允许（单业务事件 slot），设置标志。
  5. **无 `wave_id` 的业务事件，且 `non_wave_business_event_accepted == true`，但当前事件是 `work.ready` 并且前一个已接受事件是 `queue.advance`（即 `is_dual_publish_step_handoff`）** → 允许，不额外占用 slot。
  6. **其他无 `wave_id` 的业务事件** → drop。
- 结果：同一轮内可接受“一个非 wave 业务事件 + 一个完整 wave 事件组”，但不会接受两个不同 wave 或两个独立非 wave 业务事件。

**测试场景：**
- Happy path：一轮读取 4 个同 wave_id 的 `review.dimension.done`，全部被接受。
- Mixed batch：先读到 `queue.advance`（无 wave_id），再读到 `work.ready`（无 wave_id），再读到 4 个同 wave_id 的 `review.dimension.done`，全部被接受。
- Edge case：先读到无 wave_id 的 `review.dimension.done`（实际应为 merge 后带 wave_id；如缺 wave_id 视为独立业务事件占用 slot），再读到 4 个同 wave_id 的事件，wave 事件仍被接受（wave 组与非 wave 单事件各占独立 slot）。
- Edge case：两个不同 wave_id 的事件在同一轮出现，第二个 wave 被 drop。
- Error path：两个独立非 wave 业务事件（非 dual-publish 对）在同一轮出现，第二个被 drop。
- Integration：dispatcher merge 的 4 条事件经 event loop 后全部到达 `review-synthesizer`。

**验收：**
- `cargo nextest run -p ralph-core -- isolated_wave` 通过。
- 不再出现丢弃同 wave 事件的 `extra business event dropped` 日志。

---

- [ ] U2. **修复 `wave.worker.failed` 合成事件的 source hat 并补 schema**

**目标：** 让 wave 工人失败时的合成事件通过 origin guard，避免无效 `task.resume` 注入 review-coordinator。

**需求：** R2、R6

**依赖：** 无（可与 U1 并行）

**文件：**
- 修改：`crates/ralph-cli/src/loop_runner/wave/io.rs`
- 修改：`presets/en/ce-executor-isolated.yml`
- 修改：`presets/schemas/ce-executor-isolated.yml`
- 测试：`crates/ralph-cli/src/loop_runner/tests.rs`

**方案：**
- 把 `wave.worker.failed` 合成事件的 `"hat"` / `"source"` 从 `default_source_hat` 改为 `"review-synthesizer"`。rationale：synthesizer 是 wave 结果的最终消费者，`wave.worker.failed` 对它而言是聚合输入之一；这也避免 review-coordinator 因未声明该 topic 被 origin guard 拒绝。
- 把 payload 从字符串改为 JSON object：`{"reason": "...", "wave_id": "...", "wave_index": N, "error": "..."}`。
- 在 `presets/schemas/ce-executor-isolated.yml` 增加 `wave.worker.failed` schema：
  - `required_fields`: [reason, wave_id, wave_index]
  - `payload`: json_object
- 在 `presets/en/ce-executor-isolated.yml` 的 inline `event_policy.schemas:` 中也增加 `wave.worker.failed` schema（过渡期必须同步，否则 SSOT 与 inline 覆盖层不一致）。
- 在 `presets/en/ce-executor-isolated.yml` 的 `review-synthesizer.publishes` 增加 `wave.worker.failed`。
- `review-synthesizer` 在 aggregate 逻辑中消费 `wave.worker.failed`：把它视为一个维度结果缺失，走 aggregate timeout / incomplete wave 路径（与 R6 incomplete wave 机制衔接）。
- 内嵌 preset 同步：修改 `presets/en/ce-executor-isolated.yml` 后执行 `cargo build -p ralph-cli`，由 `build.rs` 重新生成 `$OUT_DIR/presets/ce-executor-isolated.yml`；`crates/ralph-cli/src/presets.rs` 通过 `include_str!` 自动读取生成产物，无需手动改 content。`presets/manifest.yml` 与 `presets/index.json` 因 preset 名称不变，通常无需改动。

**测试场景：**
- Happy path：dispatcher 写入 `wave.worker.failed` 后，origin guard 不拒绝。
- Integration：wave 含 1 个失败工人 + 6 个成功工人，event loop 能正常推进到 review-synthesizer。
- Error path：其他越权 topic（如 executor 发 `build.done`）继续被 origin guard 拒绝。

**验收：**
- `cargo nextest run -p ralph-cli -- wave` 相关测试通过。
- `ralph preset check builtin:ce-executor-isolated` 通过。

---

- [ ] U3. **给 `task.resume` 注入增加 freshness TTL**

**目标：** 防止过期 rejection 在目标 task 已关闭后重新激活错误 hat。

**需求：** R3

**依赖：** 无（可与 U1、U2 并行）

**文件：**
- 修改：`crates/ralph-core/src/event_loop/rejection.rs` 与 `crates/ralph-core/src/event_loop/mod.rs`
- 修改：`crates/ralph-core/src/config/event_loop.rs`（增加 `task_resume_ttl_seconds`）
- 测试：扩展 `crates/ralph-core/src/event_loop/tests/stale_breaker.rs` 或新建测试

**方案：**
- 在 `EventLoopConfig` 增加 `task_resume_ttl_seconds: Option<u64>`，默认 300。该字段作为**唯一 freshness 配置**，U5 的 steward 也引用它，不重复定义。
- 给 `Rejection` 结构体增加 `original_event_id: Option<String>` 与 `original_ts: Option<String>` 字段，所有 rejection 构造点（origin guard / event policy / execution contract / workflow guard）在创建 Rejection 时把源事件的 `id` 和 `ts` 传入。
- TTL 计算：用 rejection 的 `original_ts` 与当前事件时间比较；若 `original_ts` 缺失则回退到 Rejection 创建时间（用于现有测试路径）。
- 在 `build_task_resume_payload` 调用方增加过滤：
  - rejection 时间戳距现在超过 TTL → 丢弃。
  - 目标 task 已 closed 且 rejection 的 topic 不在该 hat 当前可恢复范围内 → 丢弃（可恢复范围指该 hat 的 `publishes` 明确包含 rejection 原 topic 或相关恢复 topic）。
- 丢弃时发布 `event.isolation.boundary_violation` 诊断事件，payload 含 `rejected_topic`、`source_hat`、`reason`（`expired` / `task_closed`），便于排查。
- `task.resume` schema（U2/U5 同步到 schema SSOT 与 inline schemas）：
  - `required_fields`: [reason, target_hat]（runtime 无法从 Rejection 重建 task id，故去掉 `target_task_id` 和 `source_event_id`）
  - `payload`: json_object

**测试场景：**
- Happy path：新鲜 rejection（< TTL）正常注入 `task.resume`。
- Edge case：过期 rejection（> TTL）被丢弃，不注入。
- Edge case：目标 task 已 closed 的 rejection 被丢弃。
- Error path：连续多次相同 rejection 仍在 TTL 内时，circuit breaker 逻辑保持生效。

**验收：**
- `cargo nextest run -p ralph-core -- stale` 通过。
- recovery.jsonl 中不再出现数十分钟前的 rejection 被重新注入。

---

- [ ] U4. **把 review 维度从 7 个裁剪到 4 个**

**目标：** 降低 wave 并行度、超时概率和事件合并噪音。

**需求：** R4

**依赖：** 无（可与 U1-U3 并行）

**文件：**
- 修改：`presets/en/ce-executor-isolated.yml`（`review-coordinator.instructions` 中维度选择部分）
- 修改：`presets/en/ce-executor-isolated.yml`（`dimension-reviewer.instructions` 中维度 checklist 部分）
- 修改：`presets/zh/ce-executor-isolated-zh.yml`（同步维度裁剪、新增 `progress-steward` hat 与 schema）
- 测试：`crates/ralph-cli/src/loop_runner/tests.rs` 中涉及 wave 维度数量的断言；跑 EN/ZH parity 测试
- 注意：`presets/zh/ce-executor-isolated-zh.yml` 必须同步，否则 `test_ce_executor_en_and_zh_completion_gate_consistent` 等 parity 测试会失败。

**方案：**
- 在 `review-coordinator` instructions 中，把 `work.done` 触发的 required dimensions 从 7 个改为 4 个：
  - `correctness`
  - `testing`
  - `maintainability`
  - `requirements`
- `fix.applied` 触发时仍保留 `standards` 维度（因为 fix 可能引入格式化/风格回归），所以 `dimension-reviewer.instructions` 的 `standards` checklist 块**不能删除**，改为仅在 `fix.applied` 路径使用，并明确标记“不在 `work.done` 首轮 review 默认维度中”。
- 删除 `agent-native`、`learnings` 的 checklist 块（或保留但标记为“不再使用”）。
- 保留 conditional dimensions（`security`、`performance` 等）逻辑，但说明只在 diff 明确触及时加入，不作为默认。

**测试场景：**
- Happy path：`work.done` 触发时 `review-coordinator` emit 的 `review.wave.ready` wave_total = 4。
- Edge case：diff 触及 auth/payments 时仍正确加入 `security` 维度，wave_total = 5。
- Edge case：`fix.applied` 触发时 `review.wave.ready` wave_total = 5（4 核心 + standards）。
- Error path：`work.done` 触发时不允许回到 7 维默认列表。

**验收：**
- `ralph preset check builtin:ce-executor-isolated` 通过。
- 跑一个完整 step 时，`.ralph/agent/.events-hat-review-coordinator-*.idempotency.jsonl` 中 `count` 为 4（或 conditional 触发时为 5）。

---

- [ ] U5. **新增 `progress-steward` hat 与运行时 fallback 机制**

**目标：** 当正常 hat 卡住时，由 steward 总结状态并 emit 最小合法事件，把 loop 推回正轨或干净结束。

**需求：** R5

**依赖：** U1、U2、U3、U4（先让 wave 基础路径、worker failed provenance、task resume freshness 稳定，再叠加 steward 兜底）

**文件：**
- 修改：`crates/ralph-core/src/config/event_loop.rs`（新增 `ProgressStewardConfig`）
- 修改：`crates/ralph-core/src/event_loop/mod.rs`（stall recovery 路由到 steward、强制 `loop.stalled`、steward 迭代计数）
- 修改：`presets/en/ce-executor-isolated.yml`（新增 `progress-steward` hat 定义与 instructions；同步中文 `presets/zh/ce-executor-isolated-zh.yml`）
- 修改：`presets/schemas/ce-executor-isolated.yml`（新增 `loop.stalled` schema；如 `task.resume`、`human.guidance` 尚未定义也一并补充）
- 修改：`presets/en/ce-executor-isolated.yml` 的 inline `event_policy.schemas:`（同步新增 `loop.stalled` / `task.resume` / `human.guidance`）
- 测试：新建 `crates/ralph-core/src/event_loop/tests/progress_steward.rs`

**方案：**
- 配置结构：
  ```rust
  struct ProgressStewardConfig {
      enabled: bool,
      steward_hat_id: String,       // default "progress-steward"
      max_steward_iterations: u32,  // default 3
  }
  ```
  freshness TTL 复用 U3 的 `EventLoopConfig.task_resume_ttl_seconds`，不重复定义。
- 运行时行为：
  - `stall_recovery` / `missing_event_gate` 注入 `task.resume` 前，检查 rejection 是否可恢复：若 violation 为 hat scope 本身不允许或 rejection 已过期（TTL），改路由到 `steward_hat_id`。
  - 若连续 `max_steward_iterations` 轮无 accepted 业务事件，自动 emit `loop.stalled`（diagnostic topic，不占用业务预算；计数只统计 accepted business events，diagnostic/rejection 不算）并唤醒 steward。
  - steward 自身连续激活 `max_steward_iterations` 次无进展，强制 emit `plan.blocked(reason=loop_stalled_max_iterations)`，loop 干净结束。
- Preset 中 `progress-steward` hat：
  - `triggers`: `["loop.stalled", "human.guidance"]`（review 后确认：`task.resume` 为 ralph pseudo-hat 保留，`plan.blocked` 与 shipper 路由冲突，实际实现仅保留 `[loop.stalled, human.guidance]`）
  - `publishes`: `["work.ready", "queue.advance", "review.wave.ready", "task.resume", "plan.blocked"]`
  - instructions 中实现决策树：读 plan.md / progress.md / tasks.jsonl / events.jsonl → 选择 emit 事件 → 明确下一个 hat。

**测试场景：**
- Happy path：review synthesizer 卡住，steward emit `review.wave.ready` → review-coordinator 被激活 → wave 重新跑 → loop 继续。
- Happy path：plan-gate 未推进，steward emit `queue.advance` + `work.ready` → executor 进入下一步。
- Edge case：steward 连续 3 次无进展，强制 emit `plan.blocked` → shipper → reporter 干净结束。
- Error path：steward 自身 emit 非法 topic，被 origin guard 拒绝并再次唤醒 steward（验证不会无限循环）。
- Integration：完整跑一次 `ce-executor-isolated` mock，验证 steward 不干扰正常路径。

**验收：**
- `cargo nextest run -p ralph-core -- progress_steward` 通过。
- `.worktrees/.../.ralph/recovery.jsonl` 中不再出现 stale `task.resume` 反复注入 executor。

---

- [ ] U6. **回归测试与 preset/schema lint 同步**

**目标：** 确保所有改动不破坏现有行为，preset、schema、内嵌元数据一致。

**需求：** R6

**依赖：** U1、U2、U3、U4、U5

**文件：**
- 触发内嵌 preset 重新生成：修改 `presets/en/ce-executor-isolated.yml` 和 `presets/schemas/ce-executor-isolated.yml` 后执行 `cargo build -p ralph-cli`，`build.rs` 会 deep-merge schema 到 `$OUT_DIR/presets/ce-executor-isolated.yml`；`crates/ralph-cli/src/presets.rs` 通过 `include_str!` 自动读取生成产物，**无需手动改 `content` 字段**。
- 修改：`presets/manifest.yml`（如有 embedded 列表变更；本次 preset 名称不变，无需改动）
- 修改：`presets/index.json`（如有用户可见 preset 列表变更；本次无需改动）
- 修改：`crates/ralph-cli/src/loop_runner/tests.rs`（必要时更新断言）
- 新增：`docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md`（事故复盘，覆盖 wave 稳定性与 steward 兜底两个主题）

**方案：**
- 运行 `./scripts/run-tests.sh` 或等价 `cargo nextest run --workspace --exclude ralph-e2e` + `cargo test --doc`。
- 运行 `ralph preset check builtin:ce-executor-isolated`。
- 检查 `presets/schemas/ce-executor-isolated.yml` 与 `presets/en/ce-executor-isolated.yml` inline schemas 无冲突；新增 topic 必须同时存在于两处。
- 检查 `presets/zh/ce-executor-isolated-zh.yml` 与英文 preset 在维度列表、`progress-steward` hat、schema 上保持一致。
- 确认 `cargo build -p ralph-cli` 成功重新生成 `$OUT_DIR/presets/ce-executor-isolated.yml`（必要时 `touch crates/ralph-cli/build.rs` 强制 rerun）。
- 新增解决方案文档，记录根因、修复点、steward 决策树，便于后续维护。
- 优先扩展现有测试文件：`crates/ralph-core/src/event_loop/tests/wave_isolated_scope.rs`（U1）、`crates/ralph-core/src/event_loop/tests/stale_breaker.rs`（U3），必要时再新建文件。

**测试场景：**
- Integration：完整 `work.start → work.ready → work.done → review.wave.ready(4) → 4×review.dimension.done → review.complete → plan.complete` 在 isolated 模式下跑通。
- Integration：wave 有 1 个工人失败时，steward 或机制层最终把 loop 推进到 `plan.blocked` / shipper / reporter，而不是卡死。
- Integration：正常路径下 steward 不被激活。

**验收：**
- `./scripts/run-tests.sh` 全绿。
- `ralph preset check builtin:ce-executor-isolated` 无错误。
- `docs/solutions/` 复盘文档已合并。

---

## 系统影响

- **事件总线行为**：isolated 模式下同一轮可接受“一个非 wave 业务事件 + 一个完整 wave 事件组”，但不会允许两个不同 wave 或两个非 wave 业务事件。该改动影响所有 isolated mode preset，不仅是 `ce-executor-isolated`。
- **Wave dispatcher**：`wave.worker.failed` 不再触发 review-coordinator 的 scope violation。
- **Recovery 层**：过期 rejection 被静默丢弃；不可自愈的 rejection 被路由给 steward 而不是反复骚扰源 hat。
- **新增 hat**：`progress-steward` 成为 isolated preset 的兜底角色，只在 stall/recovery 路径激活，正常路径不干预。
- **Review 流程**：默认 4 维 review，conditional 维度按需追加，降低并行负载。
- **不变性**：非 wave 路径的 per-turn 预算、`queue.advance`/`work.ready` dual-publish、completion promise 等逻辑保持不变。

---

## 风险与依赖

| 风险 | 缓解 |
|---|---|
| 修改 event loop 预算逻辑可能误放多个不同 wave | 用 `accepted_wave_id` 严格限制只接受同一个 wave_id；第二个 wave 仍被 drop |
| U1 改动影响所有 isolated mode preset | 回归测试覆盖 `ralph-core` 所有 isolated 相关测试（含 `wave_isolated_scope.rs`、`scope_enforcement.rs` 等），不仅验证 `ce-executor-isolated` |
| Steward 权力过大，可能绕过 isolated mode 安全边界 | 只订阅 recovery/diagnostic 事件；每次 emit 必须写 `reason`；max iterations 强制结束 |
| 改变 `wave.worker.failed` payload 形状影响现有 fixture | U2 同步更新 `loop_runner/tests.rs` 断言；测试覆盖 |
| TTL 阈值过严漏掉合法 recovery | 默认 300s 可配置；先跑测试再微调 |
| Dimension 裁剪降低 review 覆盖 | 保留 conditional 维度触发逻辑；核心 4 维覆盖 correctness/testing/structure/alignment |
| preset publishes / schema / manifest 不一致 | U6 专门做 lint 同步 |

---

## 文档与运行说明

- 新增 `EventLoopConfig` 字段后，同步更新 `docs/guide/harness-extensions.md` 中的配置说明。
- 新增 `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` 复盘文档。
- steward 的 instructions 中必须明确 handoff 表和 `reason` payload 格式，避免 agent 误用。

---

## 来源与参考

- **触发来源：** `.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-sunny-lotus/.ralph/agent/progress.md` 与 `fix-log.md`
- 相关代码：
  - `crates/ralph-core/src/event_loop/mod.rs:6843-6900`
  - `crates/ralph-cli/src/loop_runner/wave/io.rs:344-355`
  - `crates/ralph-core/src/event_loop/rejection.rs`
  - `crates/ralph-core/src/config/event_loop.rs`
- 相关 preset/schema：
  - `presets/en/ce-executor-isolated.yml`
  - `presets/schemas/ce-executor-isolated.yml`
  - `crates/ralph-cli/src/presets.rs`

---

## 后续修复

- **2026-06-17-001**（本计划）：修正 `progress-steward.triggers` 从 `[task.resume, loop.stalled, plan.blocked, human.guidance]` 更新为 `[loop.stalled, human.guidance]`；修正 `task.resume` 的 `required_fields` 从 `[reason, target_task_id, target_hat, source_event_id]` 更新为 `[reason, target_hat]`。
