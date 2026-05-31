---
title: "Emit Hard Gate — 阻止 Agent 口嗨 emit 事件"
type: feat
status: active
date: 2026-05-31
origin: 用户现场调试 ce-executor preset 死循环问题
---

# Emit Hard Gate — 阻止 Agent 口嗨 emit 事件

## Overview

在 Coordinator 模式下，Agent 作为 `ralph` hat 运行，需要主动执行 `ralph emit` 命令来推进 workflow。但 LLM Agent 经常"口嗨"——在输出文本中写 `ralph emit LOOP_COMPLETE`，却没有真正调用 bash 工具执行命令。这导致事件未写入 events 文件，workflow 断裂，甚至陷入死循环。

本计划在 `loop_runner.rs` 中增加 **Emit Hard Gate** 机制：当 Agent 声称 emit 了事件但实际未写入，且当前 hat 有 `publishes` 但无 `default_publishes` 兜底时，系统直接拦截并注入强制 guidance，要求 Agent 在下一轮必须真正执行 emit。

---

## Problem Frame

### 现场症状
- `ralph -H builtin:ce-executor run` 运行 26+ 次迭代不终止
- Agent 在 scratchpad 中标记所有任务完成，但 loop 永不结束
- 日志反复出现：`Output indicated 'ralph emit', but no event became readable before fallback logic`
- events 文件只有 3 条记录，后续无任何新事件
- `check_completion_event()` 因 `required_events` 未满足而拒绝 LOOP_COMPLETE，注入 `task.resume`
- Agent 再次检查 scratchpad → 再次尝试 LOOP_COMPLETE → 无限循环

### 根因
1. **Agent 口嗨 emit**：Agent 生成文本中包含 `ralph emit`，但未调用 bash 工具实际执行
2. **系统仅 WARN 不拦截**：`recover_expected_emit_after_output()` 失败只打 warn，loop 继续进入下一轮
3. **default_publishes 掩盖问题**：部分 hat 有自动兜底，但 dimension-reviewer 等 hat 无兜底，Agent 不写事件就导致 workflow 断裂
4. **task.resume 加剧死循环**：event_loop 注入 `task.resume` 要求继续，Agent 再次口嗨，循环往复

---

## Requirements Trace

- **R1.** 当 Agent 声称 emit 事件但实际未写入时，系统必须能检测并拦截，而非仅打 warn
- **R2.** 拦截逻辑只作用于**有 publishes 但无 default_publishes** 的 hat，避免破坏现有兜底机制
- **R3.** 拦截后必须向 Agent 提供明确的重试指令，告知其上一轮 emit 失败及正确做法
- **R4.** 连续拦截次数必须有上限，超过上限应优雅终止 loop，避免无限消耗 token
- **R5.** 预设无需大规模适配，改动集中在 `loop_runner.rs` 和 `event_loop`
- **R6.** 必须有测试覆盖拦截触发条件、guidance 注入内容、连续拦截终止逻辑

---

## Scope Boundaries

- **In scope**：
  - `loop_runner.rs` Hard Gate 检测与拦截逻辑
  - `event_loop` fallback/guidance 增强
  - 连续拦截计数器与终止逻辑
  - 测试覆盖
- **Out of scope**：
  - 修改 preset instructions（prompt 层面的强化属于独立工作）
  - 全局 Two-Step 迭代协议（架构改动过大，本计划聚焦 loop_runner 层面）
  - MCP 工具化 `ralph emit`（属于中期探索方向）
  - Isolated 模式改造

---

## Context & Research

### 相关代码

- **`crates/ralph-cli/src/loop_runner.rs:2671-2690`**：`recover_expected_emit_after_output()` 失败后的 WARN 逻辑
- **`crates/ralph-cli/src/loop_runner.rs:2708-2728`**：`check_default_publishes` 兜底注入
- **`crates/ralph-cli/src/loop_runner.rs:2886-2898`**：`has_pending_events()` 检测 Agent 未 publish
- **`crates/ralph-core/src/event_loop/mod.rs:1355-1395`**：`inject_fallback_event()` 注入 `task.resume`
- **`crates/ralph-core/src/event_loop/mod.rs:1342-1347`**：`get_hat_publishes()` 获取 hat 的 publishes 列表
- **`crates/ralph-core/src/config.rs:2656`**：`HatConfig.default_publishes` 字段定义

### 关键发现

- `default_publishes` 是现有兜底机制：Agent 不写事件时自动注入默认事件
- ce-executor 中 7 个 hat 有 `default_publishes`，只有 **dimension-reviewer** 无兜底
- `guidance_messages` 每轮重新创建，不能跨轮复用；直接写入 events 文件更可靠
- `event_reader.read_new_events()` 从上次位置继续读取，迭代末尾写入的事件下一轮可见

---

## Key Technical Decisions

### KD1. 拦截时机：emit recovery 失败后立即拦截

在 `recover_expected_emit_after_output()` 返回 `NoLateEvents` 时判断 Hard Gate 条件。这是唯一知道"Agent 声称 emit 但没成功"的时间点。

**替代方案**：在 `has_pending_events()` 检测时拦截。但此时不知道 Agent 是否"声称"emit 过，会误伤正常静默迭代。

### KD2. 重试方式：直接写入 `human.guidance` 事件到 events 文件

不修改 prompt 构建逻辑，而是把 hard gate 消息作为 `human.guidance` 事件直接 append 到 events 文件。下一轮 `process_events_from_jsonl()` 会自动读取并注入 prompt。

**替代方案**：修改 `build_prompt()` 注入额外内容。但 `build_prompt()` 在 `event_loop/mod.rs` 中，改动面更大，且 guidance 事件已有成熟的 prompt 注入路径。

### KD3. 跳过 default_publishes 当 Hard Gate 触发时

当前逻辑：Agent 不写事件 → `check_default_publishes()` 兜底注入。

Hard Gate 触发时，Agent 明确声称要 emit 但没成功，此时不应兜底。兜底会掩盖 Agent 的 compliance 问题，让 Agent 永远学不会真正 emit。

### KD4. 连续拦截上限：3 次

超过 3 次连续 Hard Gate 触发，说明 Agent 无法理解或执行 emit 命令，应终止 loop 并报错，避免无限消耗 token。

---

## Implementation Units

- [ ] U1. **Hard Gate 检测函数**

**Goal:** 在 `loop_runner.rs` 中新增 `should_hard_gate()` 和 `inject_hard_gate_guidance()`

**Requirements:** R1, R2, R3

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner.rs`

**Approach:**
- 新增 `fn should_hard_gate(hat_id: &HatId, event_loop: &EventLoop) -> bool`
  - 通过 `event_loop.registry().get_config(hat_id)` 获取 hat config
  - 返回 `!publishes.is_empty() && default_publishes.is_none()`
- 新增 `fn inject_hard_gate_guidance(ctx: &LoopContext, hat_id: &HatId, expected_topics: &[String])`
  - 调用 `resolve_current_events_path(ctx)` 获取 events 文件路径
  - 构造 `human.guidance` 事件，payload 格式：
    ```
    ⚠️ HARD GATE TRIGGERED: Previous iteration by hat `{hat_id}` claimed to emit an event,
    but NO EVENT WAS WRITTEN to the events file.
    
    You MUST use the bash tool to execute: ralph emit <topic>
    Allowed topics: {expected_topics}
    
    Writing `ralph emit` in prose or comments is NOT sufficient.
    The turn is incomplete until the command succeeds and the event appears in the events file.
    ```
  - 直接 append JSONL 到 events 文件（参考现有 guidance flush 的写入模式）

**Test scenarios:**
- Happy path: `should_hard_gate` 对无 publishes 的 hat 返回 false
- Happy path: `should_hard_gate` 对有 publishes + default_publishes 的 hat 返回 false
- Happy path: `should_hard_gate` 对有 publishes 无 default_publishes 的 hat 返回 true
- Edge case: hat 不在 registry 中，返回 false

**Verification:**
- `cargo test -p ralph-cli` 中新增单元测试覆盖 `should_hard_gate`
- 手动验证 guidance 事件格式正确

---

- [ ] U2. **主循环 Hard Gate 集成与连续拦截计数器**

**Goal:** 在 `run_loop_impl` 主循环中集成 Hard Gate 逻辑，增加连续拦截计数器

**Requirements:** R1, R3, R4

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner.rs`

**Approach:**
- 在主循环外部（`run_loop_impl` 函数开头）新增 `let mut consecutive_hard_gates: u32 = 0;`
- 在 `recover_expected_emit_after_output()` 返回 `NoLateEvents` 的分支中：
  ```rust
  if should_hard_gate(&display_hat, &event_loop) {
      consecutive_hard_gates += 1;
      if consecutive_hard_gates > MAX_CONSECUTIVE_HARD_GATES {
          error!("Hard gate triggered {} consecutive times. Agent is unable to emit events. Terminating.", consecutive_hard_gates);
          // 返回 TerminationReason::Stopped 或新增 TerminationReason::HardGateExceeded
          return Ok(TerminationReason::Stopped);
      }
      inject_hard_gate_guidance(&ctx, &display_hat, &event_loop.get_hat_publishes(&display_hat));
      // 关键：跳过 default_publishes 检查
      // 直接进入 cooldown / 下一轮
  } else {
      consecutive_hard_gates = 0; // 重置
      warn!(...);
  }
  ```
- 在 `agent_wrote_events = true` 的路径中（PendingWork / Terminate），重置 `consecutive_hard_gates = 0`
- 新增常量 `const MAX_CONSECUTIVE_HARD_GATES: u32 = 3;`

**Test scenarios:**
- Happy path: Agent 第一次口嗨，Hard Gate 触发，guidance 注入，下一轮 Agent 成功 emit，计数器重置
- Error path: Agent 连续 3 次口嗨，第 4 次触发终止，loop 以 Stopped 结束
- Edge case: 有 default_publishes 的 hat 口嗨，不触发 Hard Gate，正常兜底

**Verification:**
- 运行 smoke test 验证正常 workflow 不受影响
- 手动测试：构造 Agent 口嗨输出，验证 Hard Gate 行为

---

- [ ] U3. **Event Loop Fallback Payload 增强**

**Goal:** 让 `inject_fallback_event()` 在 Agent 口嗨场景下提供更精确的 recovery 提示

**Requirements:** R3

**Dependencies:** None（可与 U1/U2 并行）

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`

**Approach:**
- 修改 `inject_fallback_event()` 中构造 fallback payload 的逻辑
- 当 `last_hat` 有 publishes 时，payload 中增加提示：
  ```
  If you attempted to emit an event in the previous turn but it was not recorded,
  you must use the bash tool to execute `ralph emit` — prose mentions are not sufficient.
  ```
- 保持现有 fallback 逻辑不变，仅增强 payload 文本

**Test scenarios:**
- Happy path: fallback payload 包含增强提示
- Happy path: 无 publishes 的 hat fallback 保持原有简洁 payload

**Verification:**
- `cargo test -p ralph-core` 中更新相关 fallback 测试

---

- [ ] U4. **冒烟测试与集成测试**

**Goal:** 验证 Hard Gate 在真实 preset 场景下的行为

**Requirements:** R5, R6

**Dependencies:** U1, U2, U3

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner.rs`（测试区）
- Modify: `crates/ralph-core/src/event_loop/tests.rs`（如有相关测试）

**Approach:**
- 在 `loop_runner.rs` 测试区新增 `test_hard_gate_triggers_on_missing_emit`
- 使用 mock backend 输出包含 `ralph emit` 但不写事件的场景
- 验证：
  1. `should_hard_gate` 返回 true
  2. guidance 事件被写入 events 文件
  3. `default_publishes` 未触发
  4. 下一轮 prompt 中包含 hard gate 消息
- 新增 `test_hard_gate_resets_on_successful_emit`
- 新增 `test_hard_gate_terminates_after_max_consecutive`
- 新增 `test_hard_gate_skips_hat_with_default_publishes`

**Test scenarios:**
- Integration: 模拟 ce-executor dimension-reviewer 口嗨场景，验证 Hard Gate 拦截
- Integration: 模拟 coordinator（有 default_publishes）口嗨场景，验证正常兜底

**Verification:**
- `cargo test -p ralph-cli`
- `cargo test -p ralph-core`

---

## System-Wide Impact

- **Interaction graph:** Hard Gate 新增的 `human.guidance` 事件会被所有 backend 正常消费，无特殊交互
- **Error propagation:** 连续 Hard Gate 超限终止时，返回 `TerminationReason::Stopped`，现有 termination hook 链不受影响
- **State lifecycle risks:** 连续拦截期间 iteration 计数器正常递增，不会阻塞 `max_iterations`
- **Unchanged invariants:**
  - `default_publishes` 机制对有兜底的 hat 完全保留
  - `recover_expected_emit_after_output` 轮询逻辑不变
  - `check_completion_event()` 和 `required_events` 验证不变

---

## Risks & Dependencies

| Risk | Mitigation |
|------|-----------|
| Hard Gate 误判：Agent 确实执行了 emit 但写入延迟导致被拦截 | `recover_expected_emit_after_output` 已轮询 20 次 × 250ms = 5 秒，足够覆盖正常写入延迟 |
| Agent 看到 guidance 后仍不理解如何正确 emit | 这是 Agent compliance 问题，非本机制缺陷；连续 3 次失败后终止避免无限消耗 |
| 某些 preset 的 hat 设计依赖"静默完成"（无 emit） | `should_hard_gate` 只在 `output_mentions_ralph_emit` 时触发，静默迭代不受影响 |
| 新增 guidance 事件导致 prompt 过长 | guidance 消息约 200 字，影响极小；且只在口嗨场景触发 |

---

## Documentation / Operational Notes

- 本机制无需用户配置，默认启用
- 若未来需要禁用，可在 `event_loop` 配置中增加 `hard_gate_enabled: bool` 开关
- 建议在 `docs/solutions/` 中记录此模式："Agent 口嗨 emit 的检测与拦截"

---

## Sources & References

- **Origin discussion:** 用户现场调试 ce-executor preset 死循环（2026-05-31）
- Related code: `crates/ralph-cli/src/loop_runner.rs:2671-2690`
- Related code: `crates/ralph-core/src/event_loop/mod.rs:1355-1395`
- Related preset: `crates/ralph-cli/presets/ce-executor.yml`
