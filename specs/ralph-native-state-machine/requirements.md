# 功能概述

Ralph 新增 opt-in `event_loop.state_machine` 配置，启用后每个带 instance key 的业务事件都按状态机转移处理。只有通过 scope、event_policy、state_machine 的事件才是 accepted event，才能写入主 `.ralph/events*.jsonl` 和发布到 EventBus。

# 范围内（In Scope）

1. 新增 `StateMachineConfig` 配置类型，支持 instance_key、transitions、terminal_guard
2. 实现纯 Rust 原生 state machine validator，不依赖 Java/TLA+/Alloy
3. 将 validator 集成到 event loop 的 `process_parse_result()` 管线中
4. 重构 accepted event logging 边界：主 events 文件只记录通过验证的 accepted events
5. 修复 duplicate terminal 诱发的无限 `task.resume`  retry loop
6. 新增 Rust unit/integration tests 和 Cucumber E2E tests
7. 所有改动仅限 `crates/ralph-core/` 和 `crates/ralph-cli/` 和 `crates/ralph-e2e/`

# 范围外（Out of Scope）

1. 不删除 `workflow_guards`，两种机制并存
2. 不实现 TLA+、Alloy、Java/TLC 调用
3. 不修改 Universal/AutoResearch 仓库
4. 不实现旧 events 文件的迁移工具
5. 不改动公共 preset（除非有独立计划）
6. 不在 `state_machine` 未启用时改变任何旧行为
7. 不实现 TUI state machine 可视化
8. 不实现 `ralph preflight` 静态可达性证明
9. 不实现 CLI `workflow_guards` 自动迁移工具

# 功能性需求（Functional Requirements）

## R1 — Config
- [ ] `EventLoopConfig` 增加 `state_machine: Option<StateMachineConfig>`
- [ ] `StateMachineConfig.enabled` 默认为 false
- [ ] 配置结构支持：`instance_key.from_payload`、`required_for`、`terminal_topics`、`business_topics`、`terminal_guard`、`transitions`
- [ ] Config 验证：重复 transition topic、empty `from`、同时 `opens_instance` 和 `closes_instance` 都要报错

## R2 — Instance Key
- [ ] `instance_key.from_payload` 从 event payload JSON 读取顶层字段
- [ ] `required_for` 列表中的 topic 必须携带有效 JSON object payload
- [ ] 缺失 payload、invalid JSON、missing key、非 string key 都要 reject

## R3 — State Transitions
- [ ] 支持 open instance：`opens_instance: true` 将 instance 插入 open map
- [ ] 支持 advance：`from` 匹配时更新 open map 中的 state
- [ ] 支持 close from multiple states：`experiment.blocked` 可从 `[planned, ready, measured, scored, attacked]` 任一状态关闭
- [ ] `closes_instance: true` 将 instance 从 open map 移除并写入 closed map
- [ ] 无 state 时默认 `idle`；已 closed 的 instance 再次 `planned` 必须 reject（v1 不允许 reopen）

## R4 — Terminal Guard
- [ ] `require_no_open_instances: true` 时，terminal accepted 前 open map 必须为空
- [ ] `duplicate_terminal: reject` 时，terminal honored 后重复 terminal 必须 reject/ignore，不发布 `task.resume`
- [ ] `business_after_terminal: reject` 时，terminal 后 business event 必须 reject

## R5 — Accepted Events Boundary
- [ ] 验证拒绝的事件不写入主 `.ralph/events*.jsonl`
- [ ] scope/policy/state-machine rejected 或 dropped 的事件不得进入主 events 文件
- [ ] 如果需要保留 LLM 原始候选事件，必须走单独 raw/diagnostic trace，不污染 accepted events

## R6 — Diagnostic
- [ ] violation 必须包含：topic、instance key、current state、expected states、reason
- [ ] Diagnostic payload 必须是结构化 JSON 或可读字符串

## R7 — Compatibility
- [ ] 缺失 `state_machine` 和 `enabled: false` 时，Ralph 行为完全不变
- [ ] `workflow_guards`、`event_policy`、`execution_mode` 等既有字段的 default 和序列化形状不能变化
- [ ] 所有公共 preset（`crates/ralph-cli/presets/*.yml`）不新增 `state_machine` 即可解析

# 非功能性需求（Non-Functional Requirements）

- **兼容性**：未启用 `state_machine` 的配置必须走原有 legacy runtime 路径
- **向后不破坏**：公共 preset、workflow_guards、event_policy、EventLogger、SessionRecorder、hooks/tasks/memories 行为不变
- **可测试性**：validator 可独立于 filesystem/EventBus/CLI 进行纯 unit 测试
- **最小改动**：共享路径（loop_runner/EventLogger/SessionRecorder）如需改动，必须先 characterization test 锁住 legacy 语义

# 验收标准（Acceptance Criteria）

- [ ] Config without `state_machine` deserializes and behaves as before
- [ ] Config with `enabled: true` and valid transitions parses without error
- [ ] `experiment.planned` opens instance `t1` from `idle` to `planned`
- [ ] `experiment.ready` advances `t1` from `planned` to `ready`
- [ ] `experiment.blocked` closes `t1` from `planned`（branch close）
- [ ] `experiment.blocked` closes `t1` from `scored`（branch close from later state）
- [ ] `experiment.ready` before `experiment.planned` is rejected
- [ ] `LOOP_COMPLETE` with open instance `t1` is rejected and reports `t1` as open
- [ ] `LOOP_COMPLETE` after all instances closed is accepted exactly once
- [ ] Second `LOOP_COMPLETE` after honored does not publish `task.resume`
- [ ] `experiment.planned` after terminal is rejected
- [ ] Out-of-scope `loop.noop` is absent from accepted events
- [ ] Rejected state-machine event is absent from `.ralph/events*.jsonl`
- [ ] State machine rejection includes structured diagnostic: topic, instance_key, current_state, expected_states, reason
- [ ] Public presets still parse without `state_machine`
- [ ] Existing `workflow_guards` tests pass without changed expectations
- [ ] Existing `event_policy` tests pass without changed expectations
- [ ] `cargo test` passes for all packages