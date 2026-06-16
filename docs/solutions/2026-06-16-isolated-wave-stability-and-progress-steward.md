---
date: 2026-06-16
title: Isolated Wave 稳定性 + Progress Steward 兜底机制修复复盘
module: ralph-core/ralph-cli
tags: [isolated-mode, wave, per-turn-budget, scope-guard, freshness-ttl, progress-steward, ce-executor-isolated, plan-001]
problem_type: incident-postmortem
---

# 2026-06-16 Isolated Wave 卡死 + Progress Steward 兜底

## Context

`ce-executor-isolated` preset 在 2026-06-16 一次长程执行中把 review wave 卡死了 50 分钟——`review-synthesizer` 等不到 `review.dimension.done` 而 stall，loop 把 50 分钟前 executor 误发的 `debug.step` 的旧 rejection 重新包装成 `task.resume` 投入 executor，executor 又连续发 `work.failed`。最终被人工停止。

诊断链条：7 维 review → 6 worker 成功 + 1 worker 超时 → wave 结果中只残留 3 条 `review.dimension.done`，且其中 2 条缺 `wave_id` → `event-loop` 的 per-turn 业务事件预算判定失败 → review-synthesizer 收不齐聚合信号 → stall → 兜底机制把 stale rejection 重新投递给 executor → 失败循环。

## Root Cause（按时间顺序）

### RC1（U1 修复）：per-turn 业务事件预算污染

`event_loop/mod.rs:6843-6900` 的 `same_wave_continuation` 逻辑用 `first_wave_id_accepted: Option<Option<String>>` 跟踪"第一个业务事件的 wave_id"：
- 第一次 emit `queue.advance`（无 wave_id）→ 内部状态变为 `Some(None)`。
- 后续带 `wave_id` 的 `review.dimension.done` 因 `first_wave_id_accepted.as_ref().and_then(|inner| inner.as_deref()) == Some(wid)` 为 false 而被丢弃。

本质：单个 slot 同时承担"非 wave 业务事件"和"wave 业务事件"两种语义，无法表达"两个 slot 独立"。

修复：拆为两个独立 slot——`non_wave_business_event_accepted: bool` 和 `accepted_wave_id: Option<String>`，外加保留 `queue.advance` + `work.ready` dual-publish 豁免（2026-06-15-003 U1）。

### RC2（U2 修复）：合成事件 provenance 错误

`crates/ralph-cli/src/loop_runner/wave/io.rs:344-355` 写 `wave.worker.failed` 合成事件时 `hat`/`source` 字段填的是 `default_source_hat`（即 wave target hat，例如 `review-coordinator`）。但 `review-coordinator` 的 `publishes` 不包含 `wave.worker.failed`——origin guard 拒绝该记录并发出 unsolicited `task.resume` 注入 `review-coordinator`，在已部分失败的 wave 上制造额外停顿。

修复：把合成事件的 provenance 改为 `review-synthesizer`（wave 结果的最终消费者），payload 改为结构化 JSON object `{reason, wave_id, wave_index, error}`，同步在 preset 和 schema SSOT 中补 `wave.worker.failed` 定义和 `review-synthesizer.publishes`。

### RC3（U3 修复）：stale rejection 重新激活

`event_loop/rejection.rs::build_task_resume_payload` 在调用前没有 freshness 检查。`recovery.jsonl` 中 50 分钟前的 rejection 在新的 `task.resume` 注入路径中被复用。

修复：
- `EventLoopConfig.task_resume_ttl_seconds: Option<u64>` 默认 300s。
- `Rejection` 增加 `original_event_id: Option<String>` 和 `original_ts: Option<String>`。
- 新增 `is_rejection_stale` helper，TTL 过期直接丢弃并发出 `event.isolation.boundary_violation` diagnostic。
- `task_resume_ttl_seconds = 0` 关闭 filter（escape hatch）。

### RC4（U4 修复）：7 维 review 过度

7 维并行 review 在长程 refactor 中频繁超时（1/7 worker 失败 → wave 走 aggregate-timeout），且产生过多事件合并噪音。

修复：把 `work.done` 触发路径的必含维度从 7 降到 4（`correctness` / `testing` / `maintainability` / `requirements`）。`standards` 合并到 `maintainability` + `requirements`，保留在 `fix.applied` 路径；`agent-native` 和 `learnings` 完全删除。conditional 维度（security/performance/api-contract/reliability/adversarial）保持按需触发。

### RC5（U5 修复）：loop 级兜底缺失

任何 hat 卡住后，整个 loop 没有更高层角色打破僵局，只能等人工。

修复：新增 `progress-steward` hat + 运行时 fallback：
- `EventLoopConfig.progress_steward: ProgressStewardConfig` 默认 enabled=true, max_iterations=3。
- 运行时 stall 检测：连续 N 轮无业务事件 → 自动 emit `loop.stalled`（带 `target=progress-steward`）→ 唤醒 steward。
- steward 读 plan.md/progress.md/tasks.jsonl/events.jsonl，决策 emit `work.ready` / `review.wave.ready` / `queue.advance` + `work.ready` / `task.resume` / `plan.blocked` 中的**一个**。
- steward 自身被连续 N 次唤醒仍未产生进展 → 自动 emit `plan.blocked(reason=loop_stalled_max_iterations)`，通过 shipper → reporter 干净结束。
- steward 自保护：steward 自身 emit 失败不会递归重唤醒（`steward_woken_this_turn` 标志）。

## 关键设计决策

1. **预算分离而非扩大**：U1 选择拆 slot 而非提高 per-turn 业务事件预算上限。后者会稀释 isolated 模式的"每轮一个业务事件"语义，引入更难调度的并发问题。
2. **TTL 而非 LRU**：U3 选择时间窗口丢弃而非缓存清理。后者无法应对 cron / 重启后的重启文件读取，时间窗口更接近 root cause（rejection 跨长 period 不再有效）。
3. **Steward 一次性 emit**：U5 要求 steward 只 emit 一个事件，然后退出。这是和正常 hat 的关键区别——steward 是"读状态 + 决策 + 派发"，不做工作。
4. **Steward 自保护**：steward emit 失败不递归。这是预见到"steward 自己 emit 了非法 topic 被 origin guard 拒绝"的 self-loop 防御。
5. **Schema SSOT + inline 双写**：U5 在 `presets/schemas/ce-executor-isolated.yml`（SSOT）和 `presets/en/ce-executor-isolated.yml`（inline）两处同步新增 3 个 schema 条目。原因：`build.rs` 的 schema merge 当前还会被 inline 覆盖层影响；迁移期内必须 lockstep，否则 lint 或运行时 contract 会失败。

## Verification

- **单元测试**：U1 新增 5 个 `isolated_wave_budget.rs` 测试；U3 新增 5 个 `task_resume_ttl.rs` 测试；U5 新增 5 个 `progress_steward.rs` 测试。
- **集成测试**：所有 2244 ralph-core + 1073 ralph-cli 测试通过。
- **预设 lint**：`ralph preset check builtin:ce-executor-isolated` 返回 PASS（含 strict lint）。
- **fixture 时间戳迁移**：U3 暴露了 `isolated_complex_regression.rs` 和 `run_workflow_guard_scenario` 用 hardcoded `2024-01-01T00:00:00Z` 时间戳的事实——把 fixture-driven 测试改为 `task_resume_ttl_seconds = Some(0)` 关闭 TTL filter。
- **`LOOP_RUNNER_INTERNAL_TOPICS` 扩展**：U5 把 `loop.stalled` / `human.guidance` / `task.resume` 加入该 allowlist。这些 topic 是 runtime 内部 publish（不是 hat publish），lint 不应判为 "no publisher in the hat graph"。
- **`workflow_activation` lint 扩展**：U5 把上述三个 topic 加入 runner-injected triggers allowlist——避免 lint 把 hat triggers 当成"永远不会关闭的 workflow stage"。

## 系统影响

- **运行时行为变化**：isolated 模式下同一轮可接受"一个非 wave 业务事件 + 一个完整 wave 事件组"。同一 wave_id 的所有 dimension 结果原子通过。
- **wave partition 路径不变**：`process_events_from_jsonl_with_waves` 仍然由 `enforce_wave_isolated_scope` 处理 distinct wave_id 拒绝——U1 不影响这条路径。
- **recovery 层变化**：过期 rejection 静默丢弃；不可自愈的 rejection 路由给 progress-steward 而非反复骚扰源 hat。
- **新增 hat**：`progress-steward` 仅在 stall/recovery 路径激活，正常路径不干预。
- **review 流程变化**：默认 4 维 review，conditional 按需追加，并行负载降低约 43%。

## 不变性与延期

- 不变性：非 wave 路径的 per-turn 预算、`queue.advance`/`work.ready` dual-publish、completion promise 等逻辑保持不变。
- 不在范围内：重做整个 wave 架构、移除 isolated mode、修改后端执行器的非 wave 路径。
- 延期到后续：把 progress-steward 抽象成跨 preset 通用运行时策略；当前先在 `ce-executor-isolated` 验证。

## 关键文件变更清单

| 文件 | 变更 |
|------|------|
| `crates/ralph-core/src/event_loop/mod.rs` | U1 预算拆分、U3 freshness filter、U5 stall detector |
| `crates/ralph-core/src/event_loop/rejection.rs` | U3 Rejection 加 `original_event_id`/`original_ts` |
| `crates/ralph-core/src/event_loop/loop_state.rs` | U5 加 4 个 stall 字段 |
| `crates/ralph-core/src/event_loop/tests/isolated_wave_budget.rs` | U1 新增 5 测试 |
| `crates/ralph-core/src/event_loop/tests/task_resume_ttl.rs` | U3 新增 5 测试 |
| `crates/ralph-core/src/event_loop/tests/progress_steward.rs` | U5 新增 5 测试 |
| `crates/ralph-core/src/event_loop/tests/isolated_complex_regression.rs` | U3 fixture 关闭 TTL |
| `crates/ralph-core/tests/scenarios.rs` | U3 fixture 关闭 TTL |
| `crates/ralph-core/src/config/loop_config.rs` | U3 `task_resume_ttl_seconds`、U5 `ProgressStewardConfig` |
| `crates/ralph-core/src/config/mod.rs` | U5 re-export `ProgressStewardConfig` |
| `crates/ralph-core/src/runtime_contract.rs` | U5 扩 `LOOP_RUNNER_INTERNAL_TOPICS` |
| `crates/ralph-core/src/preset_lint/workflow_activation.rs` | U5 扩 runner-injected triggers allowlist |
| `crates/ralph-core/src/summary_writer.rs` | U5 test fixture 加新字段 |
| `crates/ralph-cli/src/loop_runner/wave/io.rs` | U2 `failure_source_hat` 参数 + 改 source hat + JSON object payload |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | U2 调用点加 `None` |
| `crates/ralph-cli/src/loop_runner/tests.rs` | U2 测试调用点加 `None` + 改断言 |
| `presets/en/ce-executor-isolated.yml` | U2 review-synthesizer publishes、U4 维度裁剪、U5 progress-steward hat、schema 同步 |
| `presets/zh/ce-executor-isolated-zh.yml` | U4 ZH 维度裁剪、U5 ZH steward |
| `presets/schemas/ce-executor-isolated.yml` | U2 wave.worker.failed、U5 loop.stalled/task.resume/human.guidance |
