---
title: 修复 Isolated 模式下 Wave 事件被丢弃及关联路由问题
type: fix
status: active
date: 2026-06-16
origin: .worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-sunny-lotus/.ralph/agent/progress.md
---

# 修复 Isolated 模式下 Wave 事件被丢弃及关联路由问题

## 概述

在 `ce-executor-isolated` preset 的 Wave Review 流程中，7 个 dimension-reviewer 工人并行产出 `review.dimension.done` 后，事件循环的 per-turn 业务事件预算错误地把大部分 wave 事件当成“额外业务事件”丢弃，导致 `review-synthesizer` 聚合器永远收不齐信号、loop 进入僵死，最终被手动停止。

本计划修复三个互相叠加的 mechanism 层问题：
1. **P0** — `crates/ralph-core/src/event_loop/mod.rs` 的 wave 延续判断把无 `wave_id` 的非 wave 事件与同一 wave 的事件混为一谈，导致带 `wave_id` 的事件被批量丢弃。
2. **P1** — `crates/ralph-cli/src/loop_runner/wave/io.rs` 在工人失败时写入的 `wave.worker.failed` 合成事件使用了错误的 source hat，被 origin guard 拒绝并注入噪音 `task.resume`。
3. **P1** — `task.resume` 注入路径缺乏 freshness TTL，会把数十分钟前的旧 rejection 重新路由到当前已关闭的 task，触发错误的 executor 激活。

---

## 问题定义

在 `ce-executor-isolated` preset 下，review 流程为：

```
review-coordinator ──wave emit──► dimension-reviewer × N
                                     │
                                     ▼
                           review.dimension.done × N
                                     │
                                     ▼
                           review-synthesizer (aggregate)
                                     │
                                     ▼
                           review.complete / review.passed / review.failed
```

实际运行中（见 `.worktrees/...sunny-lotus/.ralph/`）：

- Round 1 wave `w-18b99d1f6ba75040-26527-0`：7 维中 worker 0 超时，仅 6 个成功报告。
- Round 2 wave `w-18b99f42e17797d8-86489-0`：7 个工人都产出了 findings 文件，但 `.ralph/events-20260616-161905.jsonl` 中只残留 3 条 `review.dimension.done`，其中 2 条缺少 `wave_id`。
- 诊断日志反复出现 `Isolated mode: extra business event dropped — only one per turn`。
- `review-synthesizer` 因聚合事件 incomplete 而 stalled，loop 最终把 50 分钟前 executor 误发 `debug.step` 的旧 rejection 重新包装为 `task.resume` 投入 executor，导致 executor 连续发 `work.failed`。

根因定位：
- `event_loop/mod.rs:6843-6900` 的 `same_wave_continuation` 逻辑要求“本轮第一个业务事件的 `wave_id`”与后续事件匹配。当第一个事件没有 `wave_id` 时，`first_wave_id_accepted` 被设为 `Some(None)`，后续所有带 `wave_id` 的同一 wave 事件都被判为 false，从而被丢弃。
- `wave/io.rs:344-355` 把 `wave.worker.failed` 合成事件的 `hat`/`source` 设为 `default_source_hat`（即 `review-coordinator`），但 `review-coordinator` 的 `publishes` 不含 `wave.worker.failed`。
- `event_loop/rejection.rs` / `mod.rs` 在构建 `task.resume` 时没有检查 rejection 时间戳或目标 task 状态。

---

## 需求追溯

- **R1.** Isolated 模式下，同一 `wave_id` 的所有 wave 结果事件应在同一轮内全部进入事件总线，不被 per-turn 业务事件预算丢弃。
- **R2.** `wave.worker.failed` 合成事件必须使用合法的 source hat，避免 origin guard 拒绝和无效 `task.resume` 注入。
- **R3.** `task.resume` 注入应具备 freshness 检查，不应对已关闭的 task 或过期 rejection 重新激活。
- **R4.** 所有改动需通过 `ralph-core` / `ralph-cli` 的 nextest 回归测试，且 `ce-executor-isolated` preset lint 通过。

---

## 范围边界

- **在范围内**：`event_loop/mod.rs` 的 isolated per-turn budget、`wave/io.rs` 的合成事件 provenance、`event_loop/rejection.rs` 的 resume freshness、对应单元测试、preset publishes 同步。
- **不在范围内**：重做整个 wave 架构、移除 isolated mode、修改 dimension-reviewer 的 agent prompt、改动后端执行器（`CliExecutor` / `PtyExecutor`）的非 wave 路径。
- **Deferred 到后续**：把 `wave.worker.failed` 路由到 `review-synthesizer` 并自动触发 `plan.blocked(reason=dimension_reviewers_failed_to_converge)` 的完整机制增强（本次只保证不被 origin guard 拒绝）。

---

## 背景调研

### 相关代码

- `crates/ralph-core/src/event_loop/mod.rs:6420-6952` — isolated 模式下单轮业务事件预算与 scope enforcement。
- `crates/ralph-core/src/event_loop/mod.rs:6843-6900` — `same_wave_continuation` 与 `first_wave_id_accepted` 逻辑（问题核心）。
- `crates/ralph-cli/src/loop_runner/wave/io.rs:227-414` — wave 结果合并到主事件文件，含 `wave.worker.failed` 合成事件。
- `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:1520-1698` — wave rejection 处理，目前 `SequentialTarget` 不 emit `plan.blocked`。
- `crates/ralph-core/src/event_loop/rejection.rs` — `build_task_resume_payload` 与 rejection 注入。
- `presets/en/ce-executor-isolated.yml` — `dimension-reviewer`、`review-coordinator`、`review-synthesizer` 的 `publishes` 列表。

### 机构知识

- `docs/solutions/` 中已有多次 wave / isolated mode 相关修复记录；本次应复用现有 `RecoveryDiagnosisEnvelope` 与 retry_key 命名规范。
- `ce-executor-isolated` preset 的 `event_policy.on_violation: reject_with_resume` 要求 mechanism 层的 recovery 信号必须精准，否则会放大噪音。

---

## 关键技术决策

1. **Wave 事件组与非 wave 业务事件分离计数**
   - 原逻辑用 `first_wave_id_accepted: Option<Option<String>>` 跟踪“第一个业务事件的 wave_id”，导致无 wave_id 事件破坏后续 wave 组。
   - 改为同时维护 `non_wave_business_event_accepted: bool` 和 `accepted_wave_ids: HashSet<String>`。同一 wave_id 的事件始终作为一个整体被接受；无 wave_id 的单事件单独占一个 slot。

2. **`wave.worker.failed` 的 source hat**
   - 选项 A：使用 `dimension-reviewer` 并在 preset 中给它加 `wave.worker.failed`。
   - 选项 B：使用 `review-synthesizer` 并在 preset 中给它加 `wave.worker.failed`（聚合语义更自然）。
   - **选择 B**：`review-synthesizer` 是 wave 结果的消费者，合成失败事件由它归因更合理，且它与 `plan.blocked` 已有天然关联。

3. **`task.resume` freshness**
   - 在注入 `task.resume` 前检查：rejection 时间戳是否在最近 N 分钟内（如 5 分钟），或目标 task 是否仍处于 open 状态。
   - 过期 rejection 直接丢弃，不注入、不记 recovery envelope，避免旧错误污染当前流程。

---

## 待决问题

### 规划中已解决

- **Q1.** 是否把 `dimension-reviewer.concurrency` 改成 1 来绕过 wave？
  - 已确认不可行：`wave_detection.rs:284` 会在 `concurrency <= 1` 时返回 `SequentialTarget`，dispatcher 直接拒绝 wave，不触发任何 reviewer。
- **Q2.** `wave.worker.failed` 用哪个 hat 作为 source？
  - 选择 `review-synthesizer`，并同步 preset 的 `publishes`。

### 实现中再确认

- **Q3.** `task.resume` freshness 的具体时间阈值（5 分钟是否合适）需在实现时结合现有测试调整。
- **Q4.** 是否需要为 wave 事件缺失 `wave_id` 增加更严格的防御（例如直接 drop 无 wave_id 的 `review.dimension.done`）？待实现时评估对现有 smoke fixture 的影响。

---

## 实现单元

- [ ] U1. **修复 isolated 模式 per-turn 业务事件预算的 wave 组处理**

**目标：** 让同一 `wave_id` 的所有事件在同一轮内全部进入事件总线，不被非 wave 事件阻断。

**需求：** R1

**依赖：** 无

**文件：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`
- 测试：`crates/ralph-core/src/event_loop/tests/wave_isolated_scope.rs`（已有，扩展）或新建 `crates/ralph-core/src/event_loop/tests/isolated_wave_budget.rs`

**方案：**
- 在 `process_parse_result` 的 isolated 分支中，把 `first_wave_id_accepted: Option<Option<String>>` 替换为：
  - `non_wave_business_event_accepted: bool`
  - `accepted_wave_ids: HashSet<String>`
- 事件处理规则：
  - 有 `wave_id` 且在 `accepted_wave_ids` 中 → 允许（同 wave 延续）。
  - 有 `wave_id` 且 `accepted_wave_ids` 为空、同时 `non_wave_business_event_accepted == false` → 允许（新 wave 组开始），把 wave_id 加入集合。
  - 无 `wave_id` 且 `non_wave_business_event_accepted == false` 且 `accepted_wave_ids` 为空 → 允许（单业务事件），设置标志。
  - 其他情况 → 按现有逻辑 drop 并发布 `event.isolation.boundary_violation`。
- 保留 `is_dual_publish_step_handoff`  carve-out（`queue.advance` + `work.ready`）。

**测试场景：**
- Happy path：一轮读取 7 个同 wave_id 的 `review.dimension.done`，全部被接受。
- Edge case：一轮内先读到一个无 `wave_id` 的 `review.dimension.done`，再读到 7 个同 wave_id 的 `review.dimension.done`，7 个 wave 事件仍被接受，非 wave 事件也被接受。
- Edge case：两个不同 wave_id 的事件在同一轮出现，第二个 wave 被 drop。
- Error path：无 wave_id 的单业务事件已占用 slot，后续第二个无 wave_id 业务事件被 drop。
- Integration：验证 dispatcher merge 的 7 条事件经 event loop 后全部到达 `review-synthesizer` 聚合器。

**验收：**
- `cargo nextest run -p ralph-core -- isolated_wave` 通过。
- 不再出现 `Isolated mode: extra business event dropped — only one per turn` 丢弃同 wave 事件的情况。

---

- [ ] U2. **修复 `wave.worker.failed` 合成事件的 source hat**

**目标：** 让 wave 工人失败时的合成事件通过 origin guard，避免无效 `task.resume` 注入 review-coordinator。

**需求：** R2

**依赖：** 无（可与 U1 并行）

**文件：**
- 修改：`crates/ralph-cli/src/loop_runner/wave/io.rs`
- 修改：`presets/en/ce-executor-isolated.yml`
- 测试：`crates/ralph-cli/src/loop_runner/tests.rs`

**方案：**
- 在 `merge_wave_results_to_events_file` 中，把 `wave.worker.failed` 合成事件的 `"hat"` / `"source"` 从 `default_source_hat` 改为 `"review-synthesizer"`。
- 在 `presets/en/ce-executor-isolated.yml` 中给 `review-synthesizer.publishes` 增加 `wave.worker.failed`。
- 同步更新 `crates/ralph-cli/src/presets.rs` 的 `PRESETS` 数组、`presets/manifest.yml`、`presets/index.json`（若存在）。

**测试场景：**
- Happy path：dispatcher 写入 `wave.worker.failed` 后，origin guard 不拒绝，不生成 `isolated_scope_violation` recovery envelope。
- Integration：wave 含 1 个失败工人 + 6 个成功工人，event loop 能正常推进到 review-synthesizer。
- Error path：仍验证其他越权 topic（如 executor 发 `build.done`）继续被 origin guard 拒绝。

**验收：**
- `cargo nextest run -p ralph-cli -- wave` 相关测试通过。
- `ralph preset check builtin:ce-executor-isolated` 通过（publishes 扩展后仍一致）。

---

- [ ] U3. **给 `task.resume` 注入增加 freshness TTL**

**目标：** 防止过期 rejection 在目标 task 已关闭后重新激活错误 hat。

**需求：** R3

**依赖：** 无（可与 U1、U2 并行）

**文件：**
- 修改：`crates/ralph-core/src/event_loop/rejection.rs` 或 `crates/ralph-core/src/event_loop/mod.rs`
- 测试：`crates/ralph-core/src/event_loop/tests/stale_breaker.rs`（已有，扩展）或新建测试

**方案：**
- 在 `build_task_resume_payload` 的调用方（或注入点）增加过滤：
  - 若 rejection 时间戳距现在超过 `task_resume_ttl_seconds`（默认 300s，可从 `EventLoopConfig` 读取，无配置则默认），直接丢弃。
  - 或检查 `required_fields` 为空且 violation 为 hat scope 本身不允许时，直接判定为 non-recoverable，不注入。
- 优先采用**时间戳 TTL**，因为它覆盖所有 stale rejection，不限于 `debug.step`。
- 丢弃时可选发布一条 `event.isolation.boundary_violation` 诊断事件，便于排查。

**测试场景：**
- Happy path：新鲜的 rejection（< TTL）正常注入 `task.resume`。
- Edge case：过期 rejection（> TTL）被丢弃，不注入，不触发 hat 激活。
- Edge case：目标 task 已 closed 时，即使 rejection 在 TTL 内也丢弃（若实现该检查）。
- Error path：连续多次相同 rejection 仍在 TTL 内时，circuit breaker 逻辑保持生效。

**验收：**
- `cargo nextest run -p ralph-core -- stale` 通过。
- `.worktrees/.../.ralph/recovery.jsonl` 中不再出现 50 分钟前的 rejection 被重新注入。

---

- [ ] U4. **回归测试与 preset lint 同步**

**目标：** 确保三个修复不会破坏现有行为，且 preset 元数据一致。

**需求：** R4

**依赖：** U1、U2、U3

**文件：**
- 修改：`crates/ralph-cli/src/loop_runner/tests.rs`（必要时更新断言）
- 修改：`crates/ralph-cli/src/presets.rs`
- 修改：`presets/manifest.yml`（若需要）
- 修改：`presets/index.json`（若需要）

**方案：**
- 运行 `cargo nextest run --workspace --exclude ralph-e2e`，确认无回归。
- 运行 `cargo test --doc --workspace --exclude ralph-e2e`。
- 运行 `ralph preset check builtin:ce-executor-isolated`。
- 若新增/修改了预设 publishes，同步 `crates/ralph-cli/src/presets.rs` 的 `PRESETS` 数组。

**测试场景：**
- Integration：完整的 `work.start → work.ready → work.done → review.wave.ready → 7×review.dimension.done → review.complete → plan.complete` 流程在 isolated 模式下跑通。
- Integration：wave 含 1 个失败工人时，loop 最终能进入 `plan.blocked` 或 synthesizer 路径，而不是卡死。

**验收：**
- `./scripts/run-tests.sh` 或等价的 `cargo nextest run --workspace --exclude ralph-e2e` + `cargo test --doc` 全绿。
- `ralph preset check builtin:ce-executor-isolated` 无错误。

---

## 系统影响

- **事件总线行为**：isolated 模式下同一轮可接受“一个非 wave 业务事件 + 一个完整 wave 事件组”，比之前更宽松，但不会允许两个不同 wave 或两个非 wave 业务事件。
- **Wave dispatcher**：`wave.worker.failed` 不再触发 review-coordinator 的 scope violation，减少 recovery noise。
- **Recovery 层**：过期 rejection 被静默丢弃，降低 stale `task.resume` 对当前流程的污染。
- **不变性**：非 wave 路径的 per-turn 预算、`queue.advance`/`work.ready` dual-publish、completion promise 等逻辑保持不变。

---

## 风险与依赖

| 风险 | 缓解 |
|---|---|
| 修改 event loop 预算逻辑可能误放多个不同 wave | 用 `accepted_wave_ids` 严格限制只接受同一个 wave_id；第二个不同 wave 仍被 drop |
| 改变 `wave.worker.failed` source hat 影响现有 smoke fixture | 在 `loop_runner/tests.rs` 中搜索 `wave.worker.failed` 断言并同步更新 |
| TTL 阈值选择过严可能漏掉合法 recovery | 默认 300s，可从配置覆盖；先跑测试再微调 |
| preset publishes 修改后 manifest 不一致 | U4 专门做 lint 同步 |

---

## 文档与运行说明

- 如新增 `EventLoopConfig` 字段（如 `task_resume_ttl_seconds`），同步更新 `docs/guide/harness-extensions.md` 或相关配置文档。
- 在 `docs/solutions/` 新增一篇简短的事故复盘，记录“isolated wave 事件被 per-turn budget 丢弃”的根因与修复，便于后续 preset 维护者参考。

---

## 来源与参考

- **触发来源：** `.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-sunny-lotus/.ralph/agent/progress.md` 与 `fix-log.md`
- 相关代码：`crates/ralph-core/src/event_loop/mod.rs:6843-6900`、`crates/ralph-cli/src/loop_runner/wave/io.rs:344-355`、`crates/ralph-core/src/event_loop/rejection.rs`
- 相关 preset：`presets/en/ce-executor-isolated.yml`
