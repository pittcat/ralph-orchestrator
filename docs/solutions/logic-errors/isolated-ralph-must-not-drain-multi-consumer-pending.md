---
date: 2026-07-23
title: Isolated 模式下 ralph 不得抽干 multi-consumer peer 的 pending
module: ralph-core/event_loop
tags: [isolated, multi-consumer, pending, plan.complete, shipper, reporter, stall-recovery, supervisor]
problem_type: logic-error
status: resolved
---

# Isolated 模式下 ralph 不得抽干 multi-consumer peer 的 pending

## Context

`ce-executor-supervisor` 的 blocking full-chain 集成测在 `plan.complete` 之后卡住：ledger 有 `plan.complete`，但 `shipper`/`reporter` 从不激活，最终 hard gate 三次后 `TerminationReason::Stopped`，无 `LOOP_COMPLETE`。

表面症状像「bus 没路由」：`plan.complete` 的 ledger 行没有 `triggered` 字段。这是**误导**——multi-consumer topic 故意不填 `triggered`（见 `derive_triggered_for_topic`）。

## Root Cause

Isolated 模式下 `build_prompt("ralph")` 误走了 coordinator 路径的 `take_pending`，把**所有 hat** 的 pending 事件抽干。`reporter`/`shipper` 订阅的 `plan.complete` 被 ralph 偷走后，这两个 hat 永远不会被调度激活。

随后 stall recovery 给 pass-through 的 `shipper` 注入 targeted `task.resume`，shipper 霸占 round-robin；hard gate 满 3 次 → 停止。

## Fix

1. **Isolated ralph 只消费自己的 pending**（`event_loop/mod.rs`）：`build_prompt` 在 isolated 下不再 drain 全员 pending。
2. **Stall recovery 对 multi-consumer peer 让路**：`should_skip_stall_recovery_for_multi_consumer_peers()`——当 pass-through hat（如 shipper）仍有 peer 持有同 topic pending 时，不注入 targeted `task.resume`。
3. 回归：`event_loop/tests/build_prompt.rs::isolated_ralph_build_prompt_does_not_drain_multi_consumer_peer_pending`。

## Companion closures（同次 U7）

- Wave phase：`merge_and_complete` 在 `mark_merge_to_events` 后 `set_wave_phase(Done)`（对称 `fail_wave` → Failed）。
- R13 终态 worktree cleanup：`SupervisorBridge::finalize_terminal_cleanup` 在 `handle_termination` 调用。
- Fault path：PTY `Ok((_, _, false))` 且**无事件**时在 `record_outcome` / supervisor `record_slot_*` 记为 failure；timeout-with-events（有事件）仍记入 results，避免破坏 partial-timeout 可见性合同。
- U8 对账：`docs/solutions/architecture-patterns/2026-07-23-002-u8-closure-reconciliation.md`。

## Symptoms → Look For

| 症状 | 实际含义 |
|---|---|
| `plan.complete` 无 `triggered` | multi-consumer 正常；别当成路由失败 |
| shipper/reporter 从不激活 | 查 isolated `build_prompt` 是否 drain 了 peer pending |
| shipper 反复 `task.resume` + hard gate | stall recovery 在 peer 仍有 pending 时误注入 |

## Prevention

- Isolated 路径下，任何「取 pending」必须按 **当前 hat id** 限定，禁止 coordinator 式全员 drain。
- Multi-consumer 主题的 ledger 观测用 `recipients` / hat marker，不要依赖 `triggered`。
- 改 `build_prompt` / stall recovery 时跑：`cargo nextest run -p ralph-cli --test integration_supervisor_primary -- supervisor_full_chain_blocking`。
