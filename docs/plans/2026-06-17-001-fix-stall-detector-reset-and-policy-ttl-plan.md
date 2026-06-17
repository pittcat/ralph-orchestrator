---
title: 修复 2026-06-16-001 中 progress-steward stall detector 重置与 policy rejection TTL 覆盖
type: fix
status: active
date: 2026-06-17
origin: docs/plans/2026-06-16-001-fix-isolated-wave-stability-and-progress-steward-plan.md
---

# 修复 2026-06-16-001 中 progress-steward stall detector 重置与 policy rejection TTL 覆盖

## Overview

本计划是对 `docs/plans/2026-06-16-001-fix-isolated-wave-stability-and-progress-steward-plan.md` 执行结果的代码 review 后续。review 发现该 plan 已实现的 U1/U2/U4 基本到位，U3/U5 存在两处必须修复的缺口：

1. **P0**：真实 loop 入口 `process_events_from_jsonl_with_waves` 未重置 `stall_detector_had_events`，导致 progress-steward 的 stall detector 在真实运行中一旦检测到一次业务事件后就永远误判为"有进展"，无法唤醒 steward。
2. **P1**：`task.resume` freshness TTL（U3）只覆盖了 origin guard 的 isolated-scope rejection 路径，`event_policy` 产生的 `RejectWithResume` 仍直接注入 `task.resume`，不受 TTL 保护。

本计划坚持**最小化修复**：只修 bug、补测试、同步文档，不动 2026-06-16-001 已经实现的预算逻辑、wave provenance、维度裁剪或 steward 决策树。

---

## Problem Frame

### P0：progress-steward 在真实 loop 中永不唤醒

`crates/ralph-cli/src/loop_runner/runner.rs` 的主循环每轮调用的是 `EventLoop::process_events_from_jsonl_with_waves()`，而不是 `process_events_from_jsonl()`。当前代码只在 `process_events_from_jsonl()` 入口重置 `stall_detector_had_events = false`（`crates/ralph-core/src/event_loop/mod.rs:6349`），而 `process_events_from_jsonl_with_waves()` 没有这一行。

后果：
- 第一轮若接受了业务事件，`stall_detector_had_events` 被设为 `true`（`mod.rs:6986`）。
- 后续所有轮次该标志永远保持 `true`，因为没有入口重置它。
- `run_stall_detector_on_state()` 永远认为"本轮有进展"，`consecutive_no_progress_turns` 被重置，progress-steward 永远不会被唤醒。
- 这与 2026-06-16 事故要解决的"loop 卡死后无兜底"问题根因一致。

### P1：policy rejection 路径绕过 TTL

U3 的 freshness TTL 只在 origin guard isolated-scope violation 路径实现（`mod.rs:6807-6841`）。`event_policy` 产生的 `PolicyDecision::RejectWithResume` 通过 `publish_policy_rejection_resume()` 直接构造并发布 `task.resume`（`mod.rs:383-398`），没有经过 `is_rejection_stale()` 检查。如果 agent 反复 emit  schema/contract 违规事件，过期 rejection 仍可能被重新注入。

### P1：文档未同步

- `docs/guide/harness-extensions.md` 中的 `event_loop` 配置示例未补充 `task_resume_ttl_seconds` 和 `progress_steward` 说明。
- `docs/plans/2026-06-16-001...` 中对 `progress-steward` triggers 和 `task.resume` schema 的描述与实际实现不一致（plan 写 `triggers: [task.resume, loop.stalled, plan.blocked, human.guidance]` 和 `required_fields: [reason, target_task_id, target_hat, source_event_id]`，实现为 `[loop.stalled, human.guidance]` 和 `[reason, target_hat]`）。

---

## Requirements Trace

- **R1.** 真实 loop 入口（`process_events_from_jsonl_with_waves`）下，progress-steward 的 stall detector 必须能正确识别连续无进展轮次并唤醒 steward。
- **R2.** `event_policy::RejectWithResume` 产生的 `task.resume` 注入前必须受 `task_resume_ttl_seconds` TTL 保护，与 origin guard 路径行为一致。
- **R3.** `docs/guide/harness-extensions.md` 必须补充 `task_resume_ttl_seconds` 和 `progress_steward` 配置说明。
- **R4.** `docs/plans/2026-06-16-001...` 中对 triggers/schema 的描述必须与实际实现一致，或明确记录"实现有调整"的决策。
- **R5.** `progress_steward.enabled` 默认改为 `false`，仅 `ce-executor-isolated` preset 显式开启，避免波及其他 isolated preset。

---

## Scope Boundaries

- **在范围内**：
  - 修复 `process_events_from_jsonl_with_waves` 入口的状态重置（P0）。
  - 给 `publish_policy_rejection_resume` 调用链增加 TTL 过滤（P1）。
  - 补充/更新上述两条路径的回归测试。
  - 同步 `docs/guide/harness-extensions.md` 和 `docs/plans/2026-06-16-001...` 文档。
  - 将 `progress_steward.enabled` 默认改为 `false`，并在 `ce-executor-isolated` EN/ZH preset 中显式开启。
- **不在范围内**：
  - 不重做 U1 的 per-turn 预算逻辑。
  - 不改 U2 的 `wave.worker.failed` provenance 或 payload 形状。
  - 不改 U4 的 review 维度裁剪。
  - 不调整 steward 的 instructions 或决策树。
  - 不动执行层 `CliExecutor` / `PtyExecutor` 的非 wave 路径。
- **Deferred 到后续**：
  - 把 progress-steward 抽象为跨 preset 通用运行时策略（已在 2026-06-16-001 中 deferred）。

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/event_loop/mod.rs:6343-6351` — `process_events_from_jsonl()` 入口，已重置 `stall_detector_had_events`。
- `crates/ralph-core/src/event_loop/mod.rs:8554-8580` — `process_events_from_jsonl_with_waves()` 入口，缺少重置。
- `crates/ralph-core/src/event_loop/mod.rs:383-398` — `publish_policy_rejection_resume()`，直接 publish `task.resume`。
- `crates/ralph-core/src/event_loop/mod.rs:6700-6896` — origin guard isolated-scope rejection 路径，已实现 TTL 过滤。
- `crates/ralph-core/src/event_loop/mod.rs:9004-9135` — `run_stall_detector_on_state()` 辅助函数。
- `crates/ralph-core/src/event_loop/mod.rs:9137-9169` — `is_rejection_stale()` helper。
- `crates/ralph-cli/src/loop_runner/runner.rs:3308-3320` — 真实 loop 主入口，调用 `process_events_from_jsonl_with_waves()`。
- `crates/ralph-core/src/config/loop_config.rs:244-260` — `ProgressStewardConfig` 当前默认 `enabled: true`。

### Institutional Learnings

- `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` 已记录 U1-U5 的设计决策与验证结果，本次修复后应追加一行"后续修复"记录。

---

## Key Technical Decisions

1. **最小化修复 P0**：不在 `process_parse_result()` 开头统一重置（会改变其语义），而是在 `process_events_from_jsonl_with_waves()` 入口与 `process_events_from_jsonl()` 对称地加一行 `self.state.stall_detector_had_events = false;`。这样改动最小，不影响已有测试。
2. **复用 `is_rejection_stale`**：policy rejection 路径不新建 TTL 逻辑，直接复用已有的 helper。需要在 `publish_policy_rejection_resume` 调用处传入 TTL 配置与源事件时间戳；若源事件无 `ts` 则按 fresh 处理，保持与 U3 一致的 fallback 语义。
3. **默认策略选择 Option B**：`ProgressStewardConfig.enabled` 从 `true` 改为 `false` 默认，仅 `ce-executor-isolated` preset 显式开启。这符合 2026-06-16-001 "本次先在 ce-executor-isolated 验证"的边界，避免波及其他 isolated preset。

---

## Open Questions

### Resolved During Planning

- **Q1. 是否把重置放到 `process_parse_result()` 开头？** 否。`process_parse_result()` 是两个 public 方法的内部共享实现，在其开头重置会改变调用语义；在 public 入口对称重置更最小化、风险更低。
- **Q2. policy rejection TTL 是否覆盖 Hold 分支？** 否。Hold 分支不注入 `task.resume`，不在 U3 范围内。只覆盖 `RejectWithResume` 分支。
- **Q3. progress_steward.enabled 默认策略？** 选择 Option B：默认 `false`，仅 `ce-executor-isolated` 显式开启。已记录决策 rationale。

### Deferred to Implementation

- 无。

---

## Implementation Units

- [ ] U1. **修复 `process_events_from_jsonl_with_waves` 入口的 stall detector 状态重置（P0）**

**Goal:** 让真实 loop 入口下的 progress-steward stall detector 能正确识别连续无进展轮次。

**Requirements:** R1

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/event_loop/tests/progress_steward.rs`

**Approach:**
- 在 `process_events_from_jsonl_with_waves()` 方法开头（读取 JSONL 之后、分区之前）添加 `self.state.stall_detector_had_events = false;`，与 `process_events_from_jsonl()` 对称。
- 确认 `steward_woken_this_turn` 不需要在此处重置：该标志在 stall detector 检测到业务事件进展时已被清除（`run_stall_detector_on_state` 的 reset 分支），且它只在当前 turn 内抑制递归唤醒，跨 turn 不影响。

**Patterns to follow:**
- 与 `process_events_from_jsonl()` 入口已有重置代码（`mod.rs:6349`）保持对称。

**Test scenarios:**
- **Happy path:** 在 `progress_steward.rs` 中新增一个测试，通过 `process_events_from_jsonl_with_waves()` 连续处理 3 轮空 JSONL，验证第 3 轮 emit `loop.stalled`。
- **Edge case:** 先通过 `process_events_from_jsonl_with_waves()` 接受一个 `work.ready`，再连续 3 轮空 JSONL，验证计数正确重置后再次触发 `loop.stalled`。
- **Regression:** 验证在修复前会失败的场景——先接受一个业务事件，再连续 3 轮空 JSONL，必须触发 `loop.stalled`（修复前此场景不会触发）。

**Verification:**
- 新增测试通过。
- 现有 `progress_steward.rs` 5 个测试仍然通过。

---

- [ ] U2. **给 policy rejection 路径增加 TTL freshness 过滤（P1）**

**Goal:** 让 `event_policy::RejectWithResume` 产生的 `task.resume` 在注入前受 `task_resume_ttl_seconds` TTL 保护。

**Requirements:** R2

**Dependencies:** U1（无强依赖，但同属本修复批次，建议 U1 之后实施）

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/event_loop/tests/task_resume_ttl.rs`

**Approach:**
- 在 `publish_policy_rejection_resume()` 内部或调用方增加 TTL 检查。
- 推荐实现：把 `JsonlEvent.ts` 传入 `publish_policy_rejection_resume`，构造一个最小 `Rejection { original_ts: event.ts, .. }` 或直接用 `is_rejection_stale(&rejection, ttl_seconds)` 判断。
- 若 stale：不 publish `task.resume`，改 publish `event.isolation.boundary_violation` diagnostic（payload 含 `stale rejection` 标记），与 origin guard 路径行为一致。
- 若 fresh：保持现有 `task.resume` publish 行为。
- 若源事件无 `ts`：按 fresh 处理，保持 backward compatibility。

**Patterns to follow:**
- 与 origin guard 路径的 stale rejection 处理（`mod.rs:6807-6841`）保持一致的日志与 diagnostic 格式。

**Test scenarios:**
- **Happy path:** policy 因 schema 缺失字段 reject 一个新鲜事件，验证 `task.resume` 仍被注入。
- **Edge case:** policy reject 一个 `ts` 为 10 分钟前的事件（默认 TTL 300s），验证 `task.resume` 被丢弃并生成 `event.isolation.boundary_violation`。
- **Edge case:** policy reject 一个无 `ts` 的事件，验证按 fresh 处理、`task.resume` 仍被注入。
- **Error path:** policy reject 一个未来时间戳事件，验证按 stale 处理（复用 `is_rejection_stale` 的 future-timestamp 分支）。

**Verification:**
- 新增测试通过。
- 现有 `task_resume_ttl.rs` 测试仍然通过。
- `cargo nextest run -p ralph-core -- task_resume_ttl` 通过。

---

- [ ] U3. **更新 `docs/guide/harness-extensions.md` 配置说明（P1）**

**Goal:** 让配置文档反映新增的 `task_resume_ttl_seconds` 和 `progress_steward` 字段。

**Requirements:** R3

**Dependencies:** U5

**Files:**
- Modify: `docs/guide/harness-extensions.md`

**Approach:**
- 在 `event_loop:` 配置示例区域补充两个字段的 YAML 示例与说明。
- `task_resume_ttl_seconds`: 默认 300s，0 表示关闭，用于过滤 stale rejection。
- `progress_steward.enabled` / `steward_hat_id` / `max_steward_iterations`: 说明 steward 只在 stall/recovery 路径激活，不订阅正常业务事件；默认 `false`，需在 preset 中显式开启。

**Test scenarios:**
- Test expectation: none — 纯文档变更，手动检查文档渲染与字段描述准确性。

**Verification:**
- 文档中两个字段均有示例与中文说明。
- `ralph preset check builtin:ce-executor-isolated` 仍通过（文档不影响 lint）。

---

- [ ] U4. **同步 `docs/plans/2026-06-16-001...` 中 triggers/schema 描述（P1）**

**Goal:** 消除 plan 文档与实际实现的不一致，避免后续维护者困惑。

**Requirements:** R4

**Dependencies:** 无

**Files:**
- Modify: `docs/plans/2026-06-16-001-fix-isolated-wave-stability-and-progress-steward-plan.md`

**Approach:**
- 在 plan 的"关键技术决策 6 / Preset 层"段落，把 `progress-steward.triggers` 从 `[task.resume, loop.stalled, plan.blocked, human.guidance]` 改为 `[loop.stalled, human.guidance]`，并追加一句说明："review 后发现 `task.resume` 是 ralph pseudo-hat 的保留 trigger、`plan.blocked` 与 shipper 路由冲突，故实际实现保留 `[loop.stalled, human.guidance]`，steward 通过 `loop.stalled` 被 runtime 唤醒。"
- 在"Schema SSOT 与 inline schemas 同步"表格中，把 `task.resume` 的 `required_fields` 从 `[reason, target_task_id, target_hat, source_event_id]` 改为 `[reason, target_hat]`，并说明"runtime 无法从 Rejection 重建 task id，故去掉 `target_task_id` 和 `source_event_id`。
- 标记该 plan 为 `status: completed` 或追加一个"后续修复"小节（不修改原实现单元勾选状态，仅追加说明），指向本计划。

**Test scenarios:**
- Test expectation: none — 纯文档变更。

**Verification:**
- 文档中 triggers 与 schema 描述与实际 preset/schema 文件一致。

---

- [ ] U5. **将 `progress_steward.enabled` 默认改为 `false`，并在 `ce-executor-isolated` 显式开启（P1）**

**Goal:** 避免 progress-steward 默认影响所有 preset，仅让 `ce-executor-isolated` 启用。

**Requirements:** R5

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/config/loop_config.rs`
- Modify: `presets/en/ce-executor-isolated.yml`
- Modify: `presets/zh/ce-executor-isolated-zh.yml`
- Test: `crates/ralph-core/src/event_loop/tests/progress_steward.rs`（确认已有测试仍通过）

**Approach：**
- 修改 `default_progress_steward_enabled()` 返回 `false`。
- 在 `presets/en/ce-executor-isolated.yml` 和 `presets/zh/ce-executor-isolated-zh.yml` 的 `event_loop:` 块中显式添加：
  ```yaml
  progress_steward:
    enabled: true
    steward_hat_id: "progress-steward"
    max_steward_iterations: 3
  ```
- 检查 `progress_steward.rs` 测试是否依赖默认值：现有测试已在 YAML 中显式设置 `enabled: true`，通常无需改动；若有失败测试，补显式配置。

**Test scenarios:**
- **Happy path：** `ce-executor-isolated` preset lint 通过，`ralph preset check builtin:ce-executor-isolated` 仍 PASS。
- **Edge case：** 一个无 `progress_steward` 块的 isolated preset 默认不启用 steward，连续 3 轮无进展不触发 `loop.stalled`。
- **Regression：** `progress_steward.rs` 中显式设置 `enabled: true` 的测试仍然通过。

**Verification:**
- `./scripts/run-tests.sh` 全绿（用户已说明测试入口）。
- `ralph preset check builtin:ce-executor-isolated` PASS。

---

## System-Wide Impact

- **Interaction graph：**
  - U1 只改动 `process_events_from_jsonl_with_waves()` 入口，影响所有通过 wave partition 读取事件的入口（主 loop 和 `dispatcher.rs` 的 re-read）。
  - U2 影响 `event_policy` 所有 `RejectWithResume` 分支的下游行为，但只增加过滤，不改变 fresh rejection 的路径。
  - U5 影响所有未显式配置 `progress_steward` 的 preset 的默认行为：默认不再启用 steward。
- **Error propagation：**
  - U1 修复后，steward 唤醒失败会导致 `plan.blocked` 干净结束，不会无限 loop。
  - U2 修复后，stale policy rejection 会生成 diagnostic 事件，不会错误注入 `task.resume`。
- **State lifecycle risks：**
  - U1 的改动新增一个 per-turn 标志重置点，需确保 `process_events_from_jsonl_with_waves()` 与 `process_events_from_jsonl()` 不会在同一次外部调用链中被连续调用导致重复重置。当前主循环只调用前者，late_events 只调用后者，不存在重复重置。
- **Unchanged invariants：**
  - U1 的 per-turn 预算逻辑（`non_wave_business_event_accepted` / `accepted_wave_id`）不变。
  - `queue.advance` + `work.ready` dual-publish carve-out 不变。
  - `wave.worker.failed` provenance 和 payload 形状不变。
  - review 维度集合不变。

---

## Risks & Dependencies

| Risk | Mitigation |
|---|---|
| U1 新增重置点在某些测试路径中被重复调用 | 只加在 public 入口，不放在 `process_parse_result`；审查所有调用点确认不重复。 |
| U2 给 policy rejection 加 TTL 后，某些 fixture 测试因旧 rejection 被过滤而失败 | 参考 `isolated_complex_regression.rs` 的做法，在 fixture 驱动测试中显式设置 `task_resume_ttl_seconds = Some(0)`。 |
| U5 改变默认行为导致其他 preset 测试失败 | 先跑 `./scripts/run-tests.sh` 全量；若失败，在相关 preset 或测试中显式设置 `progress_steward.enabled = true/false`。 |
| 文档同步遗漏 | U3/U4 完成后由第二位 reviewer 对照实现代码检查。 |

---

## Documentation / Operational Notes

- `docs/guide/harness-extensions.md` 需要补充新字段说明（U3）。
- `docs/plans/2026-06-16-001-fix-isolated-wave-stability-and-progress-steward-plan.md` 需要追加 triggers/schema 实际实现的说明（U4）。
- `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` 建议追加一条"后续修复"记录，指向本计划。

---

## Sources & References

- **Origin document:** `docs/plans/2026-06-16-001-fix-isolated-wave-stability-and-progress-steward-plan.md`
- **Review output:** 本次 review 结论（用户对话）
- **Related code:**
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/config/loop_config.rs`
  - `crates/ralph-core/src/event_loop/tests/progress_steward.rs`
  - `crates/ralph-core/src/event_loop/tests/task_resume_ttl.rs`
  - `crates/ralph-cli/src/loop_runner/runner.rs`
- **Related docs:**
  - `docs/guide/harness-extensions.md`
  - `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md`
