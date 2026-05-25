# 设计概述

新增 opt-in `event_loop.state_machine` 配置，在 scope enforcement 和 event_policy validation 之后、EventBus publish 之前插入状态机验证层。State machine validator 独立于 event loop 单元测试即可验证，纯 Rust 原生实现，不依赖外部 model checker。

# 现有系统上下文

| 模块 | 文件 | 作用 |
|------|------|------|
| Config | `crates/ralph-core/src/config.rs` | YAML schema，`EventLoopConfig` 增加 `state_machine` 字段 |
| Event Policy | `crates/ralph-core/src/event_policy.rs` | payload/schema validation，`PolicyDecision` 返回风格参考 |
| Event Loop | `crates/ralph-core/src/event_loop/mod.rs` | `process_parse_result()` 验证管线 |
| Loop State | `crates/ralph-core/src/event_loop/loop_state.rs` | `LoopState` 保存 per-loop runtime state |
| EventLogger | `crates/ralph-core/src/event_logger.rs` | accepted events JSONL 写入 |
| LoopRunner | `crates/ralph-cli/src/loop_runner.rs` | `log_events_from_output()` 是当前 raw/accepted 混淆的主要位置 |
| E2E | `crates/ralph-e2e/features/hooks/*.feature` | Cucumber feature 风格参考 |

# 方案设计

## Config Types

```
StateMachineConfig
  enabled: bool (default false)
  instance_key: InstanceKeyConfig
    from_payload: String  (顶层字段名，如 "task_key")
    required_for: Vec<String>  (topic 列表)
  terminal_topics: Vec<String>
  business_topics: Vec<String>
  terminal_guard: TerminalGuardConfig
    require_no_open_instances: bool
    duplicate_terminal: DuplicateTerminalAction (reject/ignore)
    business_after_terminal: BusinessAfterTerminalAction (reject/ignore)
  transitions: Vec<TransitionConfig>
    topic: String
    from: Vec<String>  (状态列表)
    to: String
    opens_instance: bool
    closes_instance: bool
```

## Validator Return Type

```rust
enum StateMachineDecision {
    Accept { instance_key: Option<String>, new_state: String },
    Reject { finding: StateMachineFinding },
    Ignore { finding: StateMachineFinding },  // duplicate noise
    DiagnosticOnly { finding: StateMachineFinding },
}

struct StateMachineFinding {
    topic: String,
    instance_key: Option<String>,
    current_state: String,
    expected_states: Vec<String>,
    reason: String,
}
```

## Runtime State

```rust
struct StateMachineRuntimeState {
    open_instances: HashMap<String, InstanceState>,  // key -> (state, last_topic)
    closed_instances: HashMap<String, InstanceState>,  // key -> (final_state, close_topic)
    terminal_observed: bool,
    terminal_honored: bool,
    last_terminal_rejection: Option<TerminalRejectionFingerprint>,
}
```

# 数据流 / 控制流

```
Backend output
  -> EventParser.parse_candidates()
  -> Scope enforcement (isolated/coordinator)
  -> event_policy validation (payload/schema/terminal policy)
  -> state_machine validation (NEW)
      - extract instance_key from payload if topic in required_for
      - check transition: current_state -> target_state
      - opens_instance / closes_instance / advance
      - terminal_guard check (open instances, duplicate terminal, business after terminal)
  -> Decision: Accept / Reject / Ignore / DiagnosticOnly
  -> If Accept: record_event() + bus.publish() + EventLogger.write()
  -> If Reject/Ignore: publish diagnostic event (no record_event, no accepted logging)
```

# 文件 / 模块改动

| 操作 | 文件 |
|------|------|
| Modify | `crates/ralph-core/src/config.rs` — 新增 `StateMachineConfig` 等类型 |
| Create | `crates/ralph-core/src/state_machine.rs` — validator 实现 |
| Modify | `crates/ralph-core/src/lib.rs` — 导出 state_machine 模块 |
| Modify | `crates/ralph-core/src/event_loop/loop_state.rs` — 增加 `state_machine_runtime_state` |
| Modify | `crates/ralph-core/src/event_loop/mod.rs` — 插入 state_machine validation step |
| Modify | `crates/ralph-core/src/event_logger.rs` — 重构 accepted logging 边界 |
| Modify | `crates/ralph-cli/src/loop_runner.rs` — 移除 `process_output()` 前的 `log_events_from_output()` 主事件写入 |
| Create | `crates/ralph-cli/tests/loop_runner_state_machine_tests.rs` |
| Create | `crates/ralph-core/src/event_loop/tests.rs` — 集成测试 |
| Create | `crates/ralph-e2e/features/state-machine/accepted-events.feature` |
| Create | `crates/ralph-e2e/features/state-machine/branch-close.feature` |
| Create | `crates/ralph-e2e/features/state-machine/completion-guard.feature` |
| Create | `crates/ralph-e2e/src/state_machine_bdd.rs` |

# 边界情况

1. **Missing payload for required_for topic**: reject with finding
2. **Non-string instance key**: reject with finding
3. **Out-of-order transition**: reject, report expected states and current state
4. **Reopen closed instance**: reject by default (v1 不允许)
5. **Terminal with open instances**: reject, include open instance list in diagnostic
6. **Duplicate terminal after honored**: ignore, no `task.resume`
7. **Business event after terminal**: reject/ignore per terminal_guard config
8. **Legacy config (no state_machine)**: 完全不走 state machine path

# 风险与权衡

| 风险 | 缓解 |
|------|------|
| Accepted logging 重构可能引入 duplicate/missing records | 先写 characterization test 锁住 legacy 行为；新语义仅在 `state_machine.enabled` 分支启用 |
| state_machine 与 workflow_guards 概念重叠 | 代码和文档明确区分：lifecycle vs linear ordering |
| Rejection diagnostics 误触发 hats | 使用 diagnostic topic family，确保 diagnostics 不推进业务状态 |

# 测试策略

1. **Config tests**: serde defaults, old config compatibility, invalid transition shape
2. **Validator unit tests**: open/advance/branch close/terminal guard/payload errors — 可独立于 filesystem 测试
3. **Event loop integration tests**: rejection before bus publish, completion gate, diagnostics
4. **CLI tests**: accepted events file inspection, not just exit status
5. **Cucumber E2E**: branch close, completion guard, accepted events 三个 feature
6. **Legacy regression**: public presets parse、workflow_guards/event_policy tests pass unchanged