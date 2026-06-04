---
title: "upstream: Ralph 原生状态机与 native pause"
type: upstream
status: proposed
date: 2026-05-20
origin:
  - docs/report/universal-autoresearch-future-optimization-directions-2026-05-20.md
  - docs/plans/2026-05-20-001-feat-contract-driven-governed-emit-plan.md
  - docs/plans/2026-05-20-002-feat-universal-runtime-state-machine-reconciler-plan.md
related:
  - docs/solutions/architecture-patterns/ralph-event-loop-execution-model-2026-05-11.md
  - docs/solutions/architecture-patterns/ralph-headless-claude-md-and-hat-skill-routing-2026-05-10.md
  - docs/solutions/architecture-patterns/ralph-autoresearch-single-vs-multi-iteration-modes-2026-05-15.md
---

# upstream: Ralph 原生状态机与 native pause

## Overview

本计划面向 Ralph 源码侧改造，目标是把 Universal AutoResearch sidecar 中验证过的运行安全能力逐步下沉到 Ralph runtime：

- 短期：在 Ralph 现有 `workflow_guards`、`required_events`、`EventReader`、`EventLogger`、hook suspend 基础上增加 opt-in typed event policy。
- 中期：在 Ralph 内部引入 loop state snapshot / trace replay / reconciler report。
- 长期：提供原生 state machine 与 native pause/hold/resume，使关键约束在 orchestrator 层绕不过。

核心约束：所有改造必须保持旧配置可运行。新增能力默认 opt-in，现有 `ralph run`、`ralph emit`、`workflow_guards`、`required_events` 和 hook suspend 行为不得被破坏。

## Problem Frame

Universal AutoResearch 可以用 `safe_emit.py` 在 sidecar 层减少坏事件，但 sidecar 无法彻底解决三类问题：

- Hat 或用户仍可绕过 `safe_emit.py` 调用 raw `ralph emit`。
- Ralph runtime 当前事件 payload 类型是字符串为主，缺少原生 typed event schema 和 policy enforcement。
- 暂停语义主要来自 hook suspend 和 stop/restart marker，缺少“事件策略违规导致 loop hold”的一等状态。

因此，长期需要 Ralph 自己理解事件协议、状态机和 pause/hold lifecycle。Universal sidecar 先作为原型层，Ralph upstream 以兼容方式吸收其中稳定的不变量。

## Requirements Trace

- R1. 新增 Ralph native 能力必须 opt-in；旧 YAML 不声明新字段时行为保持不变。
- R2. Ralph 必须继续支持当前 string payload 和 object payload JSONL 读取方式，不能破坏已有 `ralph emit --json` 和历史 events 文件。
- R3. 事件 policy 必须在事件进入 bus 前执行，位置应靠近现有 scope enforcement / workflow guard validation。
- R4. 违规处理必须可配置：`warn`、`reject_with_resume`、`hold`、`block`，第一版默认不对旧配置启用 hold。
- R5. Native hold 必须写结构化 artifact，并可通过现有 resume/continue 路径恢复。
- R6. Loop state snapshot 和 reconciler report 必须是派生物，不替代 events JSONL 事实源。
- R7. CLI 和 API/TUI 层必须能显示 paused/held 状态，但不能让旧 loop registry 语义漂移。
- R8. 所有新增 Rust 模块必须有单元测试和现有 behavior characterization 测试，防止回归。

## Scope Boundaries

- 本计划不要求一次 PR 完成全部长期目标；按兼容层逐步推进。
- 不移除现有 `workflow_guards`；新 state machine 应复用或扩展它，而不是另起平行链路。
- 不改变 `Event` 的公共结构为 breaking change；typed payload 可通过新增 wrapper/schema 逐步引入。
- 不改变 `.ralph/loops.json` 的 active-loop registry 定位；历史恢复仍依赖 `.ralph/current-loop-id`、`.ralph/current-events`、events JSONL、summary/scratchpad 等文件。
- 不把 Universal AutoResearch 特有事件硬编码进 Ralph core；Ralph 只提供通用 event policy/state machine 能力，Universal 通过配置声明 AutoResearch 协议。

## Ralph Source Evidence

以下为本计划依赖的 Ralph 源码事实，路径均为 Ralph 仓库 repo-relative 路径：

- `crates/ralph-core/src/config.rs`：
  - `EventLoopConfig` 已有 `completion_promise`、`persistent`、`required_events`、`cancellation_promise`、`enforce_hat_scope`、`workflow_guards`、`execution_mode`。
  - `WorkflowGuardsConfig` / `WorkflowChain` / `CorrelationConfig` 已支持 strict/advisory chain 和 payload correlation。
  - `HatExecutionMode` 默认 `coordinator`，显式支持 `isolated`。
- `crates/ralph-core/src/event_loop/mod.rs`：
  - `apply_workflow_guard_validation()` 在事件进入 bus 前执行，能拒绝乱序事件并发布 `task.resume`。
  - `check_completion_event()` 会检查 `required_events` 和 workflow guard completion，再决定是否接受 completion。
  - 事件处理顺序已经包含 scope enforcement、workflow guard validation、state.record_event、event_projection、bus.publish，是接入 policy 的合适位置。
- `crates/ralph-core/src/event_loop/loop_state.rs`：
  - `LoopState` 已记录 `seen_topics`、`completion_requested`、`workflow_progress`、stale loop signature。
  - `WorkflowProgress` 已支持按 chain/instance 记录 phase，且同 phase 重复是 idempotent。
- `crates/ralph-core/src/event_reader.rs`：
  - `EventReader` 读取 `.ralph/events.jsonl`，并接受 string payload 与 object payload；object payload 会被序列化为 JSON string。
  - malformed line 会进入 `ParseResult.malformed`，可用于 backpressure。
- `crates/ralph-core/src/event_logger.rs`：
  - `EventLogger` 以 O_APPEND 单行 JSON 写事件，`from_context()` 会读取 `.ralph/current-events`。
- `crates/ralph-cli/src/main.rs`：
  - `ralph emit` 支持 `--json`，会校验 JSON 并写入 `.ralph/current-events` 指向的 events 文件；没有 marker 时回退 `.ralph/events.jsonl`。
- `crates/ralph-cli/src/loop_runner.rs`：
  - `run_loop_impl()` 会写 `.ralph/current-loop-id` 和 `.ralph/current-events`。
  - `resolve_current_events_path()` 以 `.ralph/current-events` 为权威定位当前 events JSONL。
- `crates/ralph-core/src/hooks/suspend_state.rs`：
  - 已有 `.ralph/suspend-state.json` 和 `.ralph/resume-requested`，可作为 native hold/pause 的兼容基础，但现有 schema 只有 hook-driven suspended state。
- `crates/ralph-core/src/loop_registry.rs`：
  - `.ralph/loops.json` 是 active loop registry，register/deregister 管理运行中 loop，并有 PID stale detection；不应把它改造成历史状态事实源。

## Key Technical Decisions

- **新增 opt-in policy，不改变默认行为**：新字段如 `event_loop.event_policy` / `event_loop.state_machine` 默认 `None`，旧配置行为不变。
- **复用现有 workflow progress**：state machine v1 复用 `WorkflowProgress` 的 chain/instance phase 概念，避免平行状态结构。
- **typed payload 先做 schema wrapper**：不把 `ralph_proto::Event.payload` 立刻改成 enum，以免破坏大量调用点；先在 validation 层解析 payload 为 `serde_json::Value`。
- **hold 复用 suspend artifact 思路，但独立原因类型**：hook suspend 是 hook 失败导致的暂停；event policy hold 是事件安全策略导致的暂停。两者可共享 resume marker，但 artifact 要能区分来源。
- **先 observation，再 enforcement**：第一阶段可输出 policy diagnostics 和 snapshot；第二阶段再允许 `reject`/`hold`。
- **Universal 不进 Ralph core**：Ralph 只实现通用 schema/policy/state machine，AutoResearch 规则由 YAML 配置提供。

## Proposed Ralph Config Shape

字段命名可在实现时调整，但建议语义如下：

```yaml
event_loop:
  event_policy:
    enabled: true
    mode: observe # observe | enforce
    on_violation: reject_with_resume # warn | reject_with_resume | hold
    schemas:
      experiment.planned:
        payload: json_object
        required_fields:
          - task_key
        field_types:
          task_key: string
      experiment.evaluated:
        payload: json_object
        required_fields:
          - task_key
          - evaluation.decision
        allowed_values:
          evaluation.decision: [keep, discard, blocked]
    terminal_topics:
      - LOOP_COMPLETE
    business_topics:
      - experiment.planned
      - experiment.ready
      - experiment.measured
      - experiment.scored
      - experiment.attacked
      - experiment.evaluated
      - experiment.blocked
```

长期可合并或关联到现有：

```yaml
event_loop:
  workflow_guards:
    chains:
      - name: experiment
        topics: [...]
        mode: strict
        correlation:
          from_payload: task_key
  state_machine:
    enabled: true
    terminal_monotonicity: true
    duplicate_open_instance: reject
    hold_on_policy_violation: true
```

## Implementation Units

- [ ] **Unit 1: Characterization tests for current event behavior**

  **Goal:** 在新增功能前锁住现有行为，确保旧配置不回归。

  **Files:**
  - Modify: `crates/ralph-core/src/event_loop/tests.rs`
  - Modify: `crates/ralph-core/src/config.rs`
  - Modify: `crates/ralph-cli/src/main.rs`

  **Approach:**
  - 增加或确认测试覆盖：旧配置无 `event_policy` 时，string payload、object payload、plain `ralph emit` 均保持现状。
  - 覆盖 `workflow_guards` absent parses as disabled、empty required_events allows completion、completion_promise 行为不变。
  - 覆盖 `ralph emit --json` 仍写 object payload，reader 仍转成 string payload 给 bus。

  **Tests:**
  - `cargo test -p ralph-core event_loop::tests::test_chain_validation_empty_required_events_allows_completion`
  - `cargo test -p ralph-core event_reader`
  - `cargo test -p ralph-cli test_emit_command_resolves_marker_relative_to_workspace_root_from_nested_dir`

- [ ] **Unit 2: Config 增加 opt-in EventPolicyConfig**

  **Goal:** 在不影响旧 YAML 的前提下，为 typed event policy 提供配置入口。

  **Files:**
  - Modify: `crates/ralph-core/src/config.rs`
  - Modify: `docs/guide/configuration.md`
  - Modify: `docs/api/config.md`

  **Approach:**
  - 在 `EventLoopConfig` 增加 `event_policy: Option<EventPolicyConfig>`。
  - `EventPolicyConfig` 第一版只支持 `enabled`、`mode`、`on_violation`、`schemas`、`terminal_topics`、`business_topics`。
  - `mode` 默认 `observe`，`on_violation` 默认 `warn`。
  - schema 第一版支持 JSON object、required field、allowed values、field type string/number/bool/object/array。
  - `validate()` 只校验配置自洽，不把 topic 必须存在于 hats/events 作为硬要求，避免通用性下降。

  **Tests:**
  - 旧 YAML 缺字段可解析，默认 `event_policy == None`。
  - 新 YAML 可解析 observe/enforce。
  - 非法 `mode`、空 schema topic、非法 field path 报 config error。
  - docs 示例能被配置测试 fixture 解析。

- [ ] **Unit 3: EventPolicyValidator 纯函数模块**

  **Goal:** 提供不依赖 EventLoop 的事件策略校验，可单测，可被 CLI/API/runtime 复用。

  **Files:**
  - Create: `crates/ralph-core/src/event_policy.rs`
  - Modify: `crates/ralph-core/src/lib.rs`
  - Test: `crates/ralph-core/src/event_policy.rs`

  **Approach:**
  - 输入：`event_reader::Event` 或轻量 `EventEnvelope`、`EventPolicyConfig`、当前 `PolicyRuntimeState`。
  - 输出：`PolicyDecision`：
    - `Accept`
    - `Warn(Vec<PolicyFinding>)`
    - `RejectWithResume(PolicyFinding)`
    - `Hold(PolicyFinding)`
  - 第一版不做 repair，避免 Ralph core 猜业务语义。
  - 支持 terminal monotonicity：terminal 后业务事件产生 finding。
  - 支持 schema validation：payload 必须是 JSON object、字段存在、枚举合法。

  **Tests:**
  - 合法 JSON object accepted。
  - string payload 在 schema 要求 JSON object 时产生 violation。
  - missing `task_key` 产生 violation。
  - invalid allowed value 产生 violation。
  - terminal 后 business event 产生 violation。
  - `mode=observe` 不 reject。

- [ ] **Unit 4: EventLoop 接入 policy decision**

  **Goal:** 在事件进入 bus 前执行 policy，且不破坏现有 scope/workflow guard 顺序。

  **Files:**
  - Modify: `crates/ralph-core/src/event_loop/mod.rs`
  - Modify: `crates/ralph-core/src/event_loop/loop_state.rs`
  - Test: `crates/ralph-core/src/event_loop/tests.rs`

  **Approach:**
  - 接入点放在 `process_events_from_jsonl()` 内，靠近现有 scope enforcement 和 `apply_workflow_guard_validation()`。
  - 推荐顺序：
    - read JSONL
    - malformed backpressure
    - scope enforcement
    - event policy observe/enforce
    - workflow guard validation
    - state.record_event
    - event_projection
    - bus.publish
  - `observe` 模式只写 diagnostics，不改变事件流。
  - `reject_with_resume` 模式与 workflow guard 类似：不发布坏业务事件，发布 `task.resume` 带可操作原因。
  - `hold` 模式写 hold artifact 并返回新的 termination/suspend reason，具体在 Unit 6 完成。

  **Tests:**
  - 无 `event_policy` 时现有 workflow guard 测试全部不变。
  - `observe` 模式下坏事件仍进入 bus，但 diagnostics 有 finding。
  - `reject_with_resume` 模式下坏事件不进入 bus，`task.resume` 被发布。
  - policy reject 不污染 `seen_topics` 和 `workflow_progress`。

- [ ] **Unit 5: Loop state snapshot / trace replay**

  **Goal:** 在 Ralph core 中提供通用 trace replay 和 loop snapshot，支持 API/TUI/CLI 查询。

  **Files:**
  - Create: `crates/ralph-core/src/loop_state_snapshot.rs`
  - Modify: `crates/ralph-core/src/lib.rs`
  - Modify: `crates/ralph-cli/src/loops.rs`
  - Modify: `crates/ralph-api/src/loop_domain.rs`
  - Test: `crates/ralph-core/tests/event_loop_ralph.rs` or module tests

  **Approach:**
  - 从 `.ralph/current-events` 或指定 events path 读取 JSONL。
  - 基于 `workflow_guards`、`event_policy.business_topics`、`terminal_topics` 重建通用 loop state。
  - 输出 `LoopStateSnapshot`：loop id、events path、last index、terminal、open instances、closed instances、findings。
  - 第一版不把 Universal-specific fields 写进 core；task_key 只是配置中的 correlation field。
  - CLI 可增加 `ralph loops inspect --json` 或扩展现有 loops 命令展示 snapshot。

  **Tests:**
  - 读取 no-policy events 仍能给出基础 last topic/terminal。
  - 有 workflow guard correlation 时能按 instance key 重建 open/closed。
  - malformed events 不 panic，进入 findings。
  - snapshot 不修改 events 文件。

- [ ] **Unit 6: Native hold/pause lifecycle**

  **Goal:** 将 event policy 的严重违规升级为 Ralph 原生 hold 状态，可恢复、可展示、可审计。

  **Files:**
  - Modify: `crates/ralph-core/src/hooks/suspend_state.rs`
  - Modify: `crates/ralph-cli/src/loop_runner.rs`
  - Modify: `crates/ralph-cli/src/loops.rs`
  - Modify: `crates/ralph-api/src/loop_domain.rs`
  - Modify: `crates/ralph-tui/src/state.rs`
  - Test: `crates/ralph-cli/src/loop_runner.rs`

  **Approach:**
  - 不直接复用 `SuspendLifecycleState::Suspended` 表达所有 hold，避免 hook suspend 与 policy hold 混淆。
  - 方案 A：扩展 suspend schema，增加 `source: hook | event_policy`、`policy_finding`、`event_summary`。
  - 方案 B：新增 `.ralph/hold-state.json`，resume marker 继续复用 `.ralph/resume-requested`。
  - 推荐先做方案 A 的兼容扩展时必须 bump schema version；若担心破坏旧 parser，则用方案 B。
  - `ralph run --continue` 或现有 resume 逻辑遇到 hold state 时，要求 resume marker 或显式用户动作。
  - TUI/API 显示状态为 `held`/`paused`，并展示 reason。

  **Tests:**
  - policy hold 写结构化 artifact。
  - 没有 resume signal 时 loop 不继续 dispatch 下一 hat。
  - resume signal 优先级仍低于 stop/restart，符合现有 suspend tests。
  - hook suspend 旧测试继续通过。

- [ ] **Unit 7: Transactional emit / policy-aware `ralph emit`**

  **Goal:** 让 CLI 写事件时也能 opt-in 使用 policy，减少绕过 EventLoop 的窗口。

  **Files:**
  - Modify: `crates/ralph-cli/src/main.rs`
  - Modify: `crates/ralph-core/src/event_logger.rs`
  - Modify: `crates/ralph-core/src/event_policy.rs`
  - Test: `crates/ralph-cli/src/main.rs`

  **Approach:**
  - 第一版不改变默认 `ralph emit`，只新增可选参数或通过 config/env 启用 policy-aware emit。
  - 可选 CLI：
    - `ralph emit --policy-check`
    - `ralph emit --config ralph.yml`
  - Policy-aware emit 读取当前 config、current-events、已有 state snapshot，然后校验候选事件。
  - 写入保持单行 append，避免破坏 EventLogger/O_APPEND 假设。
  - 如果 policy reject/hold，不写业务事件，写 policy artifact 或返回非零。

  **Tests:**
  - 默认 `ralph emit` 行为不变。
  - `--policy-check` 合法事件写入。
  - `--policy-check` 缺 required field 返回非零且不写 events。
  - `RALPH_EVENTS_FILE` 和 `.ralph/current-events` 优先级不变。

- [ ] **Unit 8: API/TUI 状态展示与 reconciler report**

  **Goal:** 让用户和上层工具能看到 loop held/snapshot/reconciler 状态。

  **Files:**
  - Modify: `crates/ralph-api/src/loop_domain.rs`
  - Modify: `crates/ralph-api/src/stream_domain/mod.rs`
  - Modify: `crates/ralph-tui/src/state.rs`
  - Modify: `crates/ralph-tui/src/widgets/content.rs`
  - Test: `crates/ralph-api/tests/rpc_v1_loop_parity_regressions.rs`
  - Test: `crates/ralph-tui/tests/integration_snapshots.rs`

  **Approach:**
  - API loop detail 增加 optional `runtime_state`，旧字段不删除。
  - Stream event 可发送 `loop.held` / `loop.policy_violation` 类型的 envelope，但旧订阅不要求处理。
  - TUI 只显示简短 held reason 和恢复提示，不把完整 JSON 直接塞进主界面。

  **Tests:**
  - API 旧 contract tests 不变。
  - 有 hold artifact 时 API 返回 paused/held。
  - TUI snapshot 不出现布局崩坏。

## Regression Protection Strategy

- **默认关闭**：`event_policy` / `state_machine` 未配置时，Rust runtime 行为必须与当前一致。
- **observe-first rollout**：新配置第一阶段推荐 `mode: observe`，先收集 diagnostics，再打开 enforce/hold。
- **公共类型保守**：不在第一阶段修改 `ralph_proto::Event.payload` 类型，避免跨 crate breaking change。
- **现有测试先跑**：每个 implementation unit 都先补 characterization tests，再改 runtime。
- **Artifact schema versioning**：hold/snapshot/reconciler artifact 必须有 `schema_version`。
- **No hardcoded AutoResearch**：Ralph core 不出现 `experiment.planned` 这类业务常量；这些只能出现在测试 fixture 或文档示例。
- **回滚路径明确**：如果 policy 引发问题，删除/关闭 `event_policy` 配置即可恢复旧行为。

## Test Plan

建议按单元逐步执行：

- `cargo test -p ralph-core config`
- `cargo test -p ralph-core event_reader`
- `cargo test -p ralph-core event_policy`
- `cargo test -p ralph-core event_loop`
- `cargo test -p ralph-cli emit`
- `cargo test -p ralph-cli loop_runner`
- `cargo test -p ralph-api rpc_v1_loop_parity_regressions`
- `cargo test -p ralph-tui integration_snapshots`

集成验证：

- 旧配置运行：不含 `event_policy` 的简单 ralph.yml 仍可 `ralph run`、`ralph emit`、completion。
- Observe 模式运行：坏 payload 进入 diagnostics 但不改变 dispatch。
- Enforce 模式运行：坏 payload 被 reject，`task.resume` 进入 bus。
- Hold 模式运行：坏 payload 写 hold artifact，loop 暂停，resume 后可继续。
- Universal AutoResearch 生成配置开启 policy 后，`safe_emit.py` 与 Ralph native policy 对同一 fixture 给出一致结论。

## Acceptance Criteria

- 旧 YAML 不声明新字段时，现有 config、emit、event loop、workflow guard、hook suspend 测试全部通过。
- Ralph 支持 opt-in event policy，至少能校验 JSON object payload、required field、allowed value、terminal monotonicity。
- EventLoop 能在事件进入 bus 前执行 policy，并按 observe/reject/hold 模式处理。
- Native hold 写结构化 artifact，CLI/API/TUI 能显示 held 状态，并能通过 resume/continue 路径恢复。
- Loop state snapshot 能从 events JSONL 派生，不修改事件日志。
- Ralph core 不硬编码 Universal AutoResearch 业务事件。

## Sequencing

1. Unit 1：锁住旧行为，建立回归保护。
2. Unit 2：增加配置入口，默认关闭。
3. Unit 3：实现纯函数 validator。
4. Unit 4：EventLoop observe/reject 接入。
5. Unit 5：snapshot/replay。
6. Unit 6：native hold/pause。
7. Unit 7：policy-aware `ralph emit`。
8. Unit 8：API/TUI 展示。

推荐前 4 个 unit 组成第一批 PR；Unit 5-6 组成第二批 PR；Unit 7-8 作为第三批 PR。这样可以在每批后运行完整 regression，避免一次性触碰 CLI、core、API、TUI 导致定位困难。

## Risks

| 风险 | 影响 | 缓解 |
|------|------|------|
| event policy 与 workflow_guards 重叠 | 用户不清楚谁拒绝事件 | policy 负责 payload/schema/terminal，workflow_guards 负责 chain order；diagnostics 写清 source |
| 修改 suspend schema 破坏 hook suspend | pause/resume 回归 | 优先考虑独立 hold artifact，或 schema version + 旧字段兼容 |
| typed payload 改动过大 | 跨 crate breaking change | 第一阶段不改 `Event.payload` 类型，只在 validator 内解析 |
| CLI emit 默认行为变化 | 老脚本失败 | policy-aware emit 必须 opt-in，默认路径不变 |
| native hold 导致 loop 卡住 | 自动化体验下降 | observe-first rollout，hold reason 必须可恢复，stop/restart 优先级保持 |

## Deferred Work

- 将 event schema 拆为独立 registry 或 Protobuf。
- Transactional append with compare-and-swap snapshot。
- Idempotency key 原生去重。
- Cross-loop reconciler。
- Formal model checking。
- 将 Universal `hat-contracts.yml` 自动编译为 Ralph `event_policy` config。
