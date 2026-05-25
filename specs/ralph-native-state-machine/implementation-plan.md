# 实施计划

## 阶段 1：准备

- [ ] 阅读 `crates/ralph-core/src/config.rs`，理解 `EventLoopConfig`、`EventPolicyConfig`、`WorkflowGuardsConfig` 的 serde/default 风格
- [ ] 阅读 `crates/ralph-core/src/event_policy.rs`，理解 `PolicyDecision` 返回风格
- [ ] 阅读 `crates/ralph-core/src/event_loop/mod.rs`，理解 `process_parse_result()` 验证管线顺序
- [ ] 阅读 `crates/ralph-core/src/event_loop/loop_state.rs`，理解 `LoopState` 结构
- [ ] 阅读 `crates/ralph-core/src/event_logger.rs`，理解 accepted events 写入时机
- [ ] 阅读 `crates/ralph-cli/src/loop_runner.rs`，理解 `log_events_from_output()` 位置和问题
- [ ] 确认现有测试名称和 fixture 位置，识别需要保留兼容性的测试套件

## 阶段 2：核心实现

### Unit 1：新增 state_machine 配置类型
- [ ] 在 `crates/ralph-core/src/config.rs` 新增 `StateMachineConfig` struct
- [ ] 新增 `InstanceKeyConfig`（`from_payload: String`, `required_for: Vec<String>`）
- [ ] 新增 `TransitionConfig`（`topic`, `from`, `to`, `opens_instance`, `closes_instance`）
- [ ] 新增 `TerminalGuardConfig`（`require_no_open_instances`, `duplicate_terminal`, `business_after_terminal`）
- [ ] 在 `EventLoopConfig` 增加 `state_machine: Option<StateMachineConfig>` 字段
- [ ] Config 验证：重复 transition topic、empty `from`、同时 `opens_instance` 和 `closes_instance`
- [ ] 确认旧配置（无 `state_machine` 字段）仍然 parse 和 behavior 不变
- [ ] 确认所有公共 preset（`crates/ralph-cli/presets/*.yml`）在无 `state_machine` 时仍能解析

### Unit 2：实现 state machine validator
- [ ] 创建 `crates/ralph-core/src/state_machine.rs`
- [ ] 新增 `StateMachineRuntimeState` struct（`open_instances`, `closed_instances`, `terminal_observed`, `terminal_honored`, `last_terminal_rejection`）
- [ ] 新增 `StateMachineFinding` struct（`topic`, `instance_key`, `current_state`, `expected_states`, `reason`）
- [ ] 实现 `StateMachineDecision` enum（`Accept`, `Reject`, `Ignore`, `DiagnosticOnly`）
- [ ] 实现 `validate_event()`: 解析 payload、提取 instance_key、检查 transition、terminal guard
- [ ] 实现 `apply_transition()`: open/advance/close 状态更新
- [ ] 纯 unit tests：open instance、advance、branch close、terminal guard、duplicate terminal、payload errors

### Unit 3：集成到 event loop
- [ ] 在 `LoopState` 增加 `state_machine_runtime_state: Option<StateMachineRuntimeState>`
- [ ] 在 `process_parse_result()` 中，scope 和 event_policy 之后、workflow_guards 之前插入 state_machine validation
- [ ] 当 `state_machine.enabled` 时：reject 则发布 diagnostic event，不调用 `state.record_event()` 或 `bus.publish()`
- [ ] `check_completion_event()` 在 state_machine enabled 时先检查 open instances
- [ ] 集成测试：valid chain events 发布到 EventBus、invalid out-of-order 不发布、`experiment.blocked` 关闭 instance、terminal with open instance 拒绝、diagnostic event 发布

### Unit 4：重构 accepted event logging
- [ ] 先写 characterization test 锁住 legacy config 下 event log 行为（event 数量、topic 顺序、marker）
- [ ] 移除 `process_output()` 前的 `log_events_from_output()` 主事件写入
- [ ] accepted logging 改为 event loop validation 之后执行
- [ ] `event.orphaned` 改为基于 accepted event 判断，不再从 raw topic 生成
- [ ] 如果重构会改变 legacy 配置的 event log 形状，则新 logging 仅在 `state_machine.enabled` 时启用，legacy 路径保持原样
- [ ] 测试：accepted event exactly-once 写入、rejected event 不写入、legacy config regression

### Unit 5：修复 completion idempotency 和 retry-loop prevention
- [ ] terminal honored 后：duplicate terminal 不发布 `task.resume`，按 terminal_guard action 处理
- [ ] terminal rejected 因 open instances 存在：发布一次 diagnostic/recovery，记录 state 避免相同 rejection 重复注入
- [ ] `check_completion_event()` 避免同一 completion rejection 原因反复触发 resume
- [ ] 测试：first valid terminal 返回 `CompletionPromise` 一次、duplicate terminal 不发布 `task.resume`、repeated rejected terminal 无无限恢复循环、persistent mode 仍抑制 completion

### Unit 6：Snapshot 和 diagnostics 集成
- [ ] Snapshot 增加 state machine open/closed instance summary
- [ ] Summary writer 在 loop 终止时输出是否有 open instances
- [ ] Diagnostics 记录 state machine rejection finding
- [ ] 测试：snapshot 包含 open instance key 和 state、snapshot 包含 closed instance key 和 close topic、rejection 出现在 diagnostics

### Unit 7：CLI 和 E2E tests
- [ ] 创建 `crates/ralph-cli/tests/loop_runner_state_machine_tests.rs`
- [ ] 创建 `crates/ralph-e2e/features/state-machine/accepted-events.feature`
- [ ] 创建 `crates/ralph-e2e/features/state-machine/branch-close.feature`
- [ ] 创建 `crates/ralph-e2e/features/state-machine/completion-guard.feature`
- [ ] 创建或修改 `crates/ralph-e2e/src/state_machine_bdd.rs`
- [ ] 修改 `crates/ralph-e2e/src/main.rs` 注册新 feature
- [ ] CLI tests 用 temporary workspace 和 fake backend output
- [ ] E2E tests 检查 accepted events file，不只检查 exit status

### Unit 8：Documentation 和 preset compatibility
- [ ] 更新 README 或相关 docs 说明 `state_machine` 是 opt-in
- [ ] 说明 `workflow_guards`（linear ordering）、`event_policy`（schema/terminal policy）、`state_machine`（instance lifecycle）的边界
- [ ] 所有公共 preset 继续解析，completion path 不变

## 阶段 3：验证与测试

- [ ] `cargo build` 通过
- [ ] `cargo test -- --test-threads=1` 通过（ralph-cli tests 需要单线程）
- [ ] Config tests：old config without `state_machine` deserializes、valid state machine YAML deserializes、invalid transition rejected、`opens_instance` 和 `closes_instance` 同时 true rejected
- [ ] Validator unit tests：所有 Unit 2 测试场景通过
- [ ] Event loop integration tests：rejection before bus publish、completion gate、diagnostics
- [ ] CLI tests：valid chain writes accepted events in order、blocked branch terminal accepted、terminal with open instance rejected、out-of-scope absent from accepted events、duplicate terminal 不 append duplicate accepted terminal
- [ ] Cucumber E2E：branch close scenario pass、completion guard scenario pass、accepted events trace excludes rejected/dropped
- [ ] Legacy regression：public presets parse、`workflow_guards` tests unchanged、`event_policy` tests unchanged、EventLogger legacy tests unchanged

## 阶段 4：完成条件

- [ ] `cargo test` 所有测试通过
- [ ] Config without `state_machine` behaves as before
- [ ] Branch close works：`experiment.blocked` 可从多个 open states 关闭 instance
- [ ] Terminal with open instances rejected before accepted logging
- [ ] Duplicate terminal after honored 不创建 `task.resume` loop
- [ ] Out-of-scope isolated events 不在 accepted events 中
- [ ] Rejected state-machine events 不在 `.ralph/events*.jsonl` 中
- [ ] State machine rejection 包含 structured diagnostic
- [ ] Existing `workflow_guards` tests pass without changed expectations
- [ ] Existing `event_policy` tests pass without changed expectations
- [ ] `crates/ralph-cli/presets/code-assist.yml` 默认行为不变
- [ ] 无 broad refactors 发生在文件列表以外