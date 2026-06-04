---
title: "upstream: Ralph terminal monotonicity、completion idempotency 与 lock lifecycle"
type: upstream
status: proposed
date: 2026-05-21
origin:
  - docs/plans/2026-05-20-003-upstream-ralph-native-state-machine-pause-plan.md
related:
  - docs/plans/2026-05-20-003-upstream-ralph-native-state-machine-pause-plan.md
---

# upstream: Ralph terminal monotonicity、completion idempotency 与 lock lifecycle

## Summary

本计划只覆盖 Ralph 上游编排层改造，目标是在 Ralph runtime 原生提供三类保护：

- terminal monotonicity：loop terminal 后不再允许业务事件进入 event log。
- completion idempotency：同一 loop 的 completion side effect 只能执行一次。
- lock lifecycle：正常结束后 `.ralph/loop.lock` 不再残留为 stale active signal。

这是增量保护功能，不是行为重写。默认必须兼容旧配置：未启用 `event_policy` 的项目继续保持现有行为；启用策略的项目获得更严格的事件门禁。外部项目的后验审计继续保留为第二道防线。

## Problem Frame

真实长循环运行暴露出 Ralph 编排层仍依赖 agent prompt 自律的问题：同一 events 文件可出现重复 terminal、terminal 后业务事件和 terminal 后 stale lock。外部 runtime audit 可以发现这些问题，但后验发现不能替代 Ralph 原生保护。

源码事实：

- `crates/ralph-core/src/event_policy.rs` 已有 `PolicyRuntimeState.terminal_observed`，但状态是否持久取决于调用方。
- `crates/ralph-cli/src/main.rs` 的 `ralph emit --policy-check` 当前用 fresh `PolicyRuntimeState::default()` 校验单次事件，无法根据已有 events 判断 terminal 后写入。
- `crates/ralph-core/src/event_loop/mod.rs` 已在 EventLoop 内部维护 policy runtime state，但 completion 检测和 fallback 路径需要明确幂等边界。
- `crates/ralph-core/src/loop_lock.rs` 的 `LockGuard::drop()` 释放 flock，但不清理 lock file metadata，正常退出后仍可能留下 `.ralph/loop.lock` 文件。

## Requirements

- R1. 新增保护默认 opt-in；旧 YAML 不声明 `event_policy` 时不得改变既有 `ralph run` / `ralph emit` 行为。
- R2. 启用 `event_policy` 且配置 terminal/business topics 后，terminal topic 出现后必须拒绝后续 business topic 写入。
- R3. 重复 terminal 必须被拒绝或幂等去重，不能追加多个等价 completion event。
- R4. `ralph emit --policy-check` 必须基于当前 loop events replay 形成 runtime state，不能只校验当前单个事件。
- R5. loop runner 的 event parser、completion promise、text fallback、late completion check 不得重复触发 completion side effect。
- R6. `loop.terminate` 等 Ralph 系统收尾事件不得被误判为业务事件；业务 topic 由配置显式声明。
- R7. 正常 loop 结束后，`.ralph/loop.lock` 不得继续表现为 active lock；crash stale lock 必须可识别、可诊断、可恢复。
- R8. 所有改动必须有 characterization tests，先锁定当前默认行为，再验证 opt-in 新行为，避免引入回归。

## Implementation Changes

### Event policy replay 与 terminal monotonicity

- 在 `ralph-core` 增加从 events JSONL 初始化 `PolicyRuntimeState` 的 helper，例如 `PolicyRuntimeState::from_events(events_path, policy)`。
- 该 helper 复用现有 `EventReader` / flexible payload 读取逻辑，保持 string payload、object payload、null payload 的兼容语义。
- 修改 `crates/ralph-cli/src/main.rs` 的 `emit_command_with_root()`：当 `--policy-check` 且配置启用 event policy 时，先 replay `.ralph/current-events` 指向的 event log，再校验新事件。
- 扩展 `event_policy.rs` violation 类型，至少区分 `business_event_after_terminal` 与 `duplicate_terminal_event`。
- 拒绝的事件不得写入 JSONL；warn/observe 模式仅输出诊断，不改变旧兼容路径。

### Completion idempotency

- 在 `EventLoop` 或 `loop_runner` 层增加单一 completion latch，确保同一 loop 只执行一次 completion handling。
- 收敛以下路径的 side effect：parsed event、`check_completion_event()`、completion promise、text fallback、late termination check。
- `check_completion_event()` 必须幂等：重复调用只返回已有 terminal 结论，不重复写 terminal、不重复 summary/history/completion handler。
- `log_events_from_output()` 解析出的事件不得绕过 policy-aware append/ingest 路径；如果输出同时包含 terminal event 和 completion 文本，只保留一次 terminal 语义。
- `EventLogger` 保持底层 append 组件定位，不在内部隐式读取 config；策略校验放在调用层或新增的 policy-aware append wrapper 中，避免破坏现有低层 API。

### Lock lifecycle

- 修改 `crates/ralph-core/src/loop_lock.rs`：`LockGuard::drop()` 在仍持有当前进程 flock 时清理或截断 `.ralph/loop.lock` metadata。
- 增加 verified lock inspection API，例如 `LoopLock::inspect()`，返回 `Active`、`Stale` 或 `None`，避免 `read_existing()` 把无 flock 的旧 metadata 误认为 active lock。
- CLI 启动路径遇到 stale lock 时清理并继续；遇到 active lock 时保持现有阻塞、worktree 或 parallel loop 行为。
- 保留 crash 诊断信息：metadata 无效、PID 不存在、文件可重新 acquire 时，报告 stale 来源并恢复，而不是静默当作 active lock。

## Regression Safety

- 新增能力只在 `event_policy.enabled: true` 或显式 `--policy-check` 路径生效；旧配置默认不受影响。
- 不修改 `.ralph/loops.json` 的定位，它仍是 active loop registry，不变成历史账本。
- 不改变 `ralph_proto::Event` 的 payload 公共形状，不把 string payload 迁移为 breaking enum。
- 不把外部项目的 topic 名硬编码进 Ralph core；Ralph 只提供通用 terminal/business policy。
- 不把 `loop.terminate` 默认归入 business topic，避免 runtime 收尾事件被误拒。

## Test Plan

- `ralph-core` event policy tests：
  - replay 已有 terminal 后拒绝 business event。
  - 重复 terminal 产生 duplicate terminal violation。
  - terminal 后允许非 business 系统事件。
  - string/object/null payload replay 保持现有兼容语义。
- `ralph-cli` emit tests：
  - `ralph emit --policy-check` 在已有 terminal 后拒绝 `experiment.*`。
  - 被拒绝事件不追加到 `.ralph/current-events`。
  - 未启用 event policy 的 emit 行为保持不变。
- loop runner tests：
  - backend 输出同时包含 terminal event 和 completion 文本时，只触发一次 completion handling。
  - event parser、completion promise、text fallback、late completion check 不重复写 terminal。
- loop lock tests：
  - 正常 drop 后 lock file 不再表现为 active。
  - stale lock 文件可被识别并清理。
  - active flock 仍被正确识别，防止并发 loop 误启动。
- smoke/E2E：
  - 临时 workspace 启用 event policy，跑最小 loop 到 completion。
  - 校验 terminal event 唯一。
  - terminal 后手动 emit business event 失败。
  - loop 结束后 lock inspection 不返回 active/stale failure。

## Acceptance Criteria

- 启用 event policy 的 loop 无法写入 terminal 后业务事件。
- 同一 loop completion side effect 最多执行一次。
- 正常结束后 `.ralph/loop.lock` 不再导致 runtime audit stale lock failure。
- Ralph 默认旧配置、旧 events JSONL、旧 `ralph emit` 主路径通过现有测试。
- `cargo test -- --test-threads=1` 通过；至少补充覆盖 `event_policy`、`loop_runner`、`loop_lock` 的针对性测试。
