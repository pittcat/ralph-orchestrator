---
title: fix: 适配当前代码的 preset loop termination 修复计划
type: fix
status: active
date: 2026-06-01
last_reviewed: 2026-06-02
origin: docs/brainstorms/2026-05-31-event-origin-guard-requirements.md
related:
  - docs/plans/2026-05-31-003-fix-event-origin-guard-plan.md
  - docs/plans/2026-06-02-002-fix-ralph-fallback-origin-contract-plan.md
---

# fix: 适配当前代码的 preset loop termination 修复计划

## 当前评估结论

原计划的方向仍然成立：要防止 preset 在实际工作完成后继续空转，必须同时处理 completion gate、拓扑校验、可信事件输入、来源校验和重复 completion rejection。

但当前代码已经发生较大变化，原计划不能按旧版本直接执行。需要适配的关键事实如下：

- `presets/ce-executor.yml` 和 `crates/ralph-cli/presets/ce-executor.yml` 已经把 `required_events` 改成 `["report.done"]`。
- `crates/ralph-cli/src/presets.rs` 已经有 `test_ce_executor_required_events_is_report_done`、root preset mirror drift guard、reporter publishes `report.done` 等回归测试。
- `crates/ralph-core/src/preset_validator.rs` 已经存在，`ralph hats validate` 也已经调用 `validate_preset_topology`。
- `preset_validator` 当前实现仍不够可靠：它没有真正从 `starting_event` 做路径遍历，`is_topic_reachable` 只检查 topic 是否出现在图里，`is_required_on_all_paths` 还把 completion 写死为 `LOOP_COMPLETE`。
- `PreflightRunner::default_checks_with_config` 还没有 topology check，所以 `ralph run` 和 `ralph preflight` 仍不会在调用后端前阻止坏 preset。
- `EventLogger::from_context` 仍会写 `.ralph/current-events` marker 指向的同一个 timestamped events 文件；legacy `log_events_from_output` 仍可能把 raw output 解析出的 demo/fake events 写进可信输入。
- completion rejection stale-breaker 已经部分落地，但进展判断只看 `seen_topics.len()`，还没有纳入 runtime task 状态、workflow progress 或 accepted business event 计数。
- `HatRegistry::from_runtime_config`、builtin runtime `ralph`、默认 `cancellation_promise: "loop.cancel"` 已经存在；本计划不再重复实现这些内容，只需要和相关行为保持一致。
- `presets/ce-executor-zh.yml` 仍然保留旧的 `required_events: ["review.passed", "review.complete"]`，这是当前计划必须补上的漏同步。

因此本计划从“新增所有机制”改为“补齐当前实现缺口并收敛回归覆盖”。

## 问题框架

真实故障表现仍是：

```text
完成实际任务 -> runtime tasks 关闭 -> report.done/LOOP_COMPLETE 已出现
随后 raw/demo/fake event 进入下一轮调度
completion 被重复拒绝或 workflow 被重新触发
loop 继续消耗迭代/API
```

当前代码下，主要剩余风险分成五类：

1. 中文版 `ce-executor-zh` 仍使用旧 completion gate，文件路径运行时可能复现旧死循环。
2. topology validator 已存在但判断过松，可能把不可达 topic、错误 completion path 或 runtime `ralph` fallback 误判成有效路径。
3. preflight/run 未复用 topology validator，坏 preset 仍可能开跑。
4. raw output logging 与 trusted events input 仍共用文件，模型输出里的 event mention 可能被 EventReader 再消费。
5. completion rejection 熔断已存在但进展 marker 太粗，容易被无关 topic 重置，也不能识别 task 状态变化带来的真实进展。

## Requirements Trace

- R1. `ce-executor` 和 `ce-executor-zh` 的 completion gate 必须统一使用 `report.done`。
- R2. `required_events` 继续保持 all-of 语义；validator 必须能发现 required event 不在所有 completion path 上。
- R3. `ralph hats validate`、`ralph preflight`、`ralph run` preflight 必须复用同一套 topology validator。
- R4. 模型 raw output 中的 demo/fake event 不得写入 trusted events input，也不得被下一轮 EventReader 消费。
- R5. 真实 `ralph emit`、wave merge、系统可信写入路径必须继续正常驱动 topology。
- R6. no-hat business event 在 hat-based preset 中不能绕过来源和 scope 校验；control topics 继续按白名单允许。
- R7. 重复 completion rejection 必须在无可信进展时熔断为 `LoopStale`，避免无限空转。
- R8. 修复不能破坏 ce-executor 正常主链：`work.start -> work.ready -> work.done -> review.* -> REVIEW_COMPLETE -> report.done -> LOOP_COMPLETE`。
- R9. 测试必须覆盖真实机制路径，不只做源代码字符串断言。

## Scope Boundaries

- 不新增 `required_events_any_of` 或 completion DSL。
- 不把 `max_iterations` 当作主要修复手段。
- 不重写 event bus、state machine 或整个 event policy。
- 不重复实现 `HatRegistry::from_runtime_config` 或默认 `loop.cancel`，这些当前代码已经具备。
- 不把 builtin `ralph` 设计成无限权限来源；它仍必须受当前 topology 派生 scope 约束。
- 不清理历史 worktree 事件文件。若原始 worktree 不存在，使用最小化 fixture 固化故障形状。

## 已完成但必须保留的内容

这些不再作为待办实现，但必须通过测试守住：

- 英文 `ce-executor` root preset 与 embedded mirror 的 `required_events == ["report.done"]`。
- reporter 声明 `publishes: ["report.done", "LOOP_COMPLETE"]` 或等价能力。
- `presets.rs` 中 ce-executor completion gate 与 publish chain 的回归测试。
- runtime-aware `HatRegistry::from_runtime_config` 和 builtin `ralph` 的有限 publish scope。
- 默认 `cancellation_promise == "loop.cancel"`。
- completion rejection signature 与连续拒绝计数的基础实现。

## Implementation Units

### Unit 1: 同步修正 `ce-executor-zh` completion gate

**Goal:** 补齐当前实际遗漏，避免通过文件路径使用中文 preset 时复现旧死循环。

**Requirements:** R1, R8

**Files:**
- Modify: `presets/ce-executor-zh.yml`
- Test: `crates/ralph-cli/src/presets.rs` 或新增针对 root-only preset 的测试辅助

**Approach:**
- 将 `presets/ce-executor-zh.yml` 的 `event_loop.required_events` 改为 `["report.done"]`。
- 确认中文 preset 的 reporter 仍发布 `report.done` 和 `LOOP_COMPLETE`。
- 因 `ce-executor-zh` 不在 embedded builtin 列表中，测试应直接读取 root 文件，而不是通过 `get_preset`。

**Test scenarios:**
- Happy path: 解析 `presets/ce-executor-zh.yml` 后，`required_events == ["report.done"]`。
- Regression: 中文 preset reporter 声明可发布 `report.done` 和 `LOOP_COMPLETE`。
- Regression: 英文 root preset、embedded mirror、中文 root preset 的 completion gate 一致。

### Unit 2: 修正现有 `preset_validator` 的真实拓扑分析

**Goal:** 把当前占位式 validator 升级成能从 start 到 completion 做真实路径判断的共享校验器。

**Requirements:** R2, R3, R8

**Files:**
- Modify: `crates/ralph-core/src/preset_validator.rs`
- Modify: `crates/ralph-cli/src/hats.rs`
- Test: `crates/ralph-core/src/preset_validator.rs`
- Test: `crates/ralph-cli/src/presets.rs`

**Current problems to fix:**
- `is_topic_reachable` 不能只判断 `all_topics.contains(target)`，必须从 `event_loop.starting_event` 出发遍历。
- `is_required_on_all_paths` 不能写死 `LOOP_COMPLETE`，必须使用 `config.event_loop.completion_promise`。
- validator 不能让 runtime `ralph` 的 `*` 订阅或派生 publishes 把所有 path 都误判为可达。
- wildcard trigger 要复用 `Topic::matches` / `HatRegistry` 的匹配语义。
- 环形 topology 必须有 visited/path limit，避免递归或路径爆炸。

**Design:**
- 以 `config.hats` 构建主图：`topic -> hat -> published topic`。
- `default_publishes` 与 `publishes` 都纳入 hat 可发布 topic。
- builtin `ralph` 只作为特殊 fallback/coordination actor 处理，不作为“任意业务 path”的普通节点。
- completion path 定义为从 starting event 可达的 hat 最终发布 `completion_promise`。
- required event 必须满足：
  - 从 starting event 可达；
  - 至少一个 reachable hat 可发布；
  - 每条 reachable completion path 都经过该 topic。

**Test scenarios:**
- Happy path: 线性图 `start -> A(mid) -> B(done) -> LOOP_COMPLETE`，required `done`，通过。
- Happy path: `ce-executor` 和 `ce-executor-zh` 中 `report.done` 在 completion path 上，通过。
- Error path: completion promise 只出现在无关 hat 或不可达分支，失败。
- Error path: required event 不可达，失败。
- Error path: required event 只在一条 completion branch 上出现，另一条 branch 可直接 completion，失败。
- Regression: 旧 `review.passed + review.complete` 作为互斥分支同时 required 时失败。
- Edge case: `default_publishes` 指向 completion promise 时可识别。
- Edge case: `review.*` 等 wildcard trigger 可识别。
- Edge case: 图中有 retry 环时不爆栈。
- Regression: public builtin presets 通过 validator；失败消息包含 preset 名称和具体 topic。

### Unit 3: 将 topology validator 接入 preflight/run

**Goal:** 坏 preset 在 `ralph preflight` 或 `ralph run` 调用后端前失败，而不是跑到运行时死循环。

**Requirements:** R2, R3

**Files:**
- Modify: `crates/ralph-core/src/preflight.rs`
- Modify: `crates/ralph-cli/src/preflight.rs`
- Modify: `crates/ralph-cli/src/main.rs` 如需调整输出
- Test: `crates/ralph-core/src/preflight.rs`
- Test: `crates/ralph-cli/tests/integration_preflight.rs`
- Test: `crates/ralph-cli/tests/integration_run_presets.rs`

**Approach:**
- 在 `PreflightRunner::default_checks_with_config` 中加入 `PresetTopologyCheck`。
- check 内部调用 `validate_preset_topology`，让 CLI preflight、run auto-preflight、doctor 可共享。
- warning 不阻塞运行；completion path / required-events 错误必须是 failure。
- `ralph hats validate` 继续输出更完整的拓扑报告；`ralph run` 只需要打印 fatal summary 和具体 topic。
- 保留 `features.preflight.skip` 的行为，允许用户显式跳过该 check，但默认启用。

**Test scenarios:**
- Happy path: `builtin:ce-executor` preflight 通过。
- Happy path: public builtin presets 全部通过 topology preflight。
- Error path: 临时 hats file 的 completion promise 不可达，`ralph preflight -H file` 非零退出。
- Error path: branch-only required event，`ralph run --dry-run -H file` 在后端调用前失败。
- Regression: `-c builtin:{name}` 旧用法仍给迁移提示，不被 topology check 覆盖成模糊错误。

### Unit 4: 分离 raw output history 与 trusted events input

**Goal:** 阻止模型输出文本里的 demo/fake events 被写回 `.ralph/current-events` 指向的可信输入文件。

**Requirements:** R4, R5

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner.rs`
- Modify: `crates/ralph-core/src/event_logger.rs`
- Test: `crates/ralph-cli/src/loop_runner.rs`
- Test: `crates/ralph-cli/tests/integration_events_isolation.rs`

**Approach:**
- 将 loop runner 中的 `event_logger` 从 trusted events file 切换到独立 history/observability 文件，例如 `.ralph/history.jsonl` 或明确命名的 `.ralph/events-history-*.jsonl`。
- `EventLogger::from_context` 不应默认读取 `current-events` marker；若保留旧函数，新增更明确 API：
  - `EventLogger::trusted_events_from_context` 仅用于必须写 trusted input 的少数路径；
  - `EventLogger::history_from_context` 用于 raw output、accepted events、orphan diagnostics、terminate events。
- `log_events_from_output` 只能写 history/diagnostics，绝不能写 EventReader 消费的文件。
- `event.orphaned` 作为诊断保留，但只进入 history/diagnostics，不触发下一轮 fallback 调度。
- `log_accepted_events` 可以保留 accepted event history，但不得造成同一 event 被下一轮重复读取。

**Test scenarios:**
- Happy path: agent 真正执行 `ralph emit work.done` 后，trusted events file 包含并驱动 `work.done`。
- Regression: raw output 包含 `<event topic="debug.step">demo</event>`，history 中可见，trusted events file 不包含。
- Regression: raw output 包含 JSON 行 `{"topic":"experiment.planned","payload":{"task_key":"x"}}`，不会进入 trusted events file。
- Regression: raw output 包含 `LOOP_COMPLETE` 字样但没有真实 emit 时，只走 text fallback completion 检测，不生成 fake JSONL business event。
- Regression: `event.orphaned` 只在 history/diagnostics 中，不被 EventReader 消费。
- Edge case: state-machine candidate events 文件不受 raw output logger 污染。
- Edge case: wave worker output 不污染 main trusted events file。

### Unit 5: 收紧 hat-based mode 的 no-hat business event 策略

**Goal:** 在 raw output 污染被切断后，进一步减少无 provenance 业务事件进入可信管道的空间。

**Requirements:** R4, R5, R6, R8

**Files:**
- Modify: `crates/ralph-core/src/event_origin.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/event_origin.rs`
- Test: `crates/ralph-core/src/event_loop/tests.rs`

**Approach:**
- 保留 solo/empty config 兼容行为。
- 在 hat-based mode 中：
  - control topics 继续允许 no-hat，例如 `human.interact`、`human.guidance`、`task.resume`、`loop.cancel`。
  - completion promise 可以 no-hat 进入 completion safety checks，但不能绕过 required events、runtime tasks、state machine。
  - 普通 business topics 如果没有 `hat`，必须能被当前 active hat scope 授权；否则拒绝。
- 注意当前 coordinator mode 下 `active_hats.is_empty()` 会直接放行，需要改成“无 active hat 时只允许 control/completion，不允许 arbitrary business”。
- 对 `hat=ralph` 继续依赖 runtime registry 的有限 publish scope，不做无限特判。

**Test scenarios:**
- Happy path: registered hat 发布声明内 topic，被接受。
- Happy path: builtin runtime `ralph` 发布 topology 内 topic，被接受。
- Happy path: no-hat `task.resume`、`human.interact`、`loop.cancel` 被接受。
- Error path: unknown hat `strategist` 发布 `experiment.planned` 被拒绝。
- Error path: registered hat 发布未声明 topic 被拒绝。
- Error path: hat-based preset 中 no-hat `debug.step` 在无 active hat 授权时被拒绝。
- Error path: active hat 不可发布 `build.done` 时 no-hat `build.done` 被拒绝。
- Edge case: solo mode registry empty 时 no-hat business event 继续兼容。
- Regression: ce-executor 正常链中合法事件全部通过。

### Unit 6: 补齐 completion rejection stale-breaker 的进展判定

**Goal:** 已有 stale-breaker 能真正识别“同一拒绝原因 + 无可信进展”，同时不误杀有进展的长流程。

**Requirements:** R7, R8

**Files:**
- Modify: `crates/ralph-core/src/event_loop/loop_state.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-core/src/summary_writer.rs` 如需展示摘要
- Test: `crates/ralph-core/src/event_loop/tests.rs`

**Current gap:**
- 当前 `handle_completion_rejection` 只比较 `seen_topics.len()`。
- 无关新 topic 可能被误认为进展。
- runtime task close/open、workflow phase advancement 等真实进展没有纳入 signature reset。

**Approach:**
- 引入明确的 progress marker，例如：
  - accepted business event monotonic count；
  - runtime task store fingerprint 或 open/closed task version；
  - workflow progress instance/phase count；
  - state machine accepted transition count。
- completion rejection signature 至少包含 kind 和关键参数：
  - `missing_required:<sorted topics>`
  - `open_tasks:<stable task ids or count+ids hash>`
  - `workflow_guard:<chain/instance summary>`
  - `verdict_fail:<topic>`
- 相同 signature 连续 3 次且 progress marker 未变化，返回 `TerminationReason::LoopStale`。
- 任何 accepted business progress、task 状态变化、workflow/state-machine progress 都重置计数。
- diagnostics/summary 中包含最近拒绝原因和建议。

**Test scenarios:**
- 第一次和第二次相同 missing required events 只注入 `task.resume`，不终止。
- 第三次相同 missing required events 且无进展时返回 `LoopStale`。
- open runtime tasks 导致 completion rejection 连续 3 次后返回 `LoopStale`。
- workflow guard incomplete 连续 3 次后返回 `LoopStale`。
- 两次 rejection 中间有 accepted business event，计数重置。
- 两次 rejection 中间有 runtime task 状态变化，计数重置。
- 两次 rejection 中间只有 raw/history event，不重置。
- 正常 accepted completion 仍返回 `CompletionPromise`。

### Unit 7: 加入最小化 replay 回归

**Goal:** 用真实故障形状证明修复覆盖 completion gate、raw contamination 和 stale breaker。

**Requirements:** R1, R4, R7, R9

**Files:**
- Create/Modify: `crates/ralph-core/tests/fixtures/`
- Modify: `crates/ralph-core/tests/smoke_runner.rs` 或 `crates/ralph-core/src/event_loop/tests.rs`

**Approach:**
- 原计划引用的 `.worktrees/...` 事件文件当前不一定存在；不要让测试依赖本地 worktree。
- 从文档中的故障形状提取最小 fixture：
  - tasks closed；
  - `review.complete` / `report.done` / `LOOP_COMPLETE`；
  - 后续 raw/fake `debug.step` / `build.done` / `experiment.planned(task_key=x)` / `LOOP_COMPLETE(reason=retry)`。
- 测试断言：
  - `report.done + LOOP_COMPLETE` 可以终止；
  - fake tail 不进入 trusted events input；
  - 旧 bad required-events 配置会在重复 rejection 后 stale-break，而不是无限 retry。

**Test scenarios:**
- Regression: `report.done -> LOOP_COMPLETE` 终止为 `CompletionPromise`。
- Regression: post-completion fake `experiment.planned(task_key=x)` 不触发下游 hat。
- Regression: fake `debug.step` 和 `build.done` 不计入 trusted progress marker。
- Regression: 旧 bad config 缺失 `review.passed` 连续 rejection 后返回 `LoopStale`。
- Integration: runtime tasks 全 closed 时 completion 被接受；有 open tasks 时 completion 被拒绝并可 stale-break。

### Unit 8: 更新文档和诊断说明

**Goal:** 让 preset 作者知道 all-of completion gate、拓扑校验和完成后重试的诊断方式。

**Requirements:** R2, R3, R7, R9

**Files:**
- Modify: `docs/guide/presets.md`
- Modify: `docs/concepts/hats-and-events.md`
- Modify: `docs/reference/troubleshooting.md`
- Modify: `docs/advanced/loop-detection.md`

**Approach:**
- 明确 `required_events` 是 all-of，不是 any-of。
- required event 应选择所有成功 completion path 都会经过的汇聚 topic，例如 `report.done`。
- 说明 `ralph hats validate` 和 preflight 会检查 completion path / required-events。
- troubleshooting 新增“完成后反复 `LOOP_COMPLETE` / retry / task.resume”的诊断条目。
- loop detection 文档补充 completion rejection stale-breaker 的触发条件。

## System-Wide Impact

- **Preset 行为:** 英文/中文 ce-executor completion gate 一致，避免旧分支事件 AND 条件。
- **Validation:** topology 错误从运行时空转提前到 validate/preflight/run 阶段。
- **Event IO:** trusted events input 只承载真实 emit/wave/system writes；raw output 只进入 history/diagnostics。
- **Security:** no-hat business event 在 hat-based preset 中不再无条件放行。
- **Termination:** 相同 completion rejection 且无可信进展时终止为 `LoopStale`。

## Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
| --- | --- | --- | --- |
| validator 因 runtime `ralph` fallback 产生假阳性 | Medium | High | 主图基于 config hats，Ralph 只作特殊 fallback |
| validator 对 wildcard/循环误报 | Medium | High | 复用 Topic match，增加 wildcard/cycle 测试 |
| raw output 不写 current-events 后调试信息减少 | Low | Medium | 写入独立 history/diagnostics |
| no-hat 策略收紧破坏 solo mode | Medium | Medium | solo/empty config 保持兼容，hat-based mode 单独收紧 |
| stale-breaker 误杀长流程 | Low | High | 只有同 signature 且 progress marker 不变才熔断 |
| 中文 preset 再次漂移 | Medium | Medium | root-only test 直接读取 `presets/ce-executor-zh.yml` |

## Detailed Test Plan

### Unit Tests

- `crates/ralph-cli/src/presets.rs`
  - `test_ce_executor_required_events_is_report_done`
  - `test_ce_executor_required_events_is_report_done_for_root_preset`
  - `test_ce_executor_zh_required_events_is_report_done`
  - `test_ce_executor_reporter_publishes_report_done`
  - `test_public_presets_pass_completion_topology_validation`

- `crates/ralph-core/src/preset_validator.rs`
  - accepts linear topology with required event on all completion paths
  - rejects unreachable completion promise
  - rejects unreachable required event
  - rejects required event only present on one branch
  - handles wildcard trigger
  - handles graph cycles without recursion overflow
  - uses configured completion promise, not hardcoded `LOOP_COMPLETE`

- `crates/ralph-core/src/preflight.rs`
  - includes topology check in `default_checks_with_config`
  - topology failure produces `CheckStatus::Fail`
  - warning-only topology diagnostics do not fail non-strict preflight

- `crates/ralph-cli/src/loop_runner.rs`
  - raw output events are written only to history/diagnostics
  - trusted events file is untouched by `log_events_from_output`
  - accepted event history does not get re-consumed
  - text fallback completion still works with bare `LOOP_COMPLETE`

- `crates/ralph-core/src/event_origin.rs`
  - unknown hat rejected
  - registered hat out-of-scope topic rejected
  - no-hat control topic accepted
  - no-hat business topic rejected in hat-based mode when not authorized
  - solo mode compatibility preserved

- `crates/ralph-core/src/event_loop/tests.rs`
  - `report.done` satisfies ce-executor completion gate
  - same completion rejection 3 times returns `LoopStale`
  - trusted progress resets rejection counter
  - runtime task status change resets rejection counter
  - raw/history-only event does not reset rejection counter
  - accepted completion returns `CompletionPromise`

### Integration Tests

- `crates/ralph-cli/tests/integration_preflight.rs`
  - valid builtin preset passes topology preflight
  - invalid hats file fails preflight with required topic details

- `crates/ralph-cli/tests/integration_run_presets.rs`
  - `ralph run --dry-run -H builtin:ce-executor` passes preflight
  - bad temporary preset fails before backend execution

- `crates/ralph-cli/tests/integration_events_isolation.rs`
  - raw output logging does not append to trusted current-events target
  - `ralph emit` still writes to current-events target

- `crates/ralph-core` replay/smoke tests
  - minimal ce-executor completion fixture terminates after `report.done + LOOP_COMPLETE`
  - fake event tail does not route to hats
  - old bad required-events config stale-breaks

## Full Verification Commands

```bash
cargo fmt
cargo test -p ralph-cli presets
cargo test -p ralph-cli hats
cargo test -p ralph-cli --test integration_preflight
cargo test -p ralph-cli --test integration_run_presets
cargo test -p ralph-cli --test integration_events_isolation
cargo test -p ralph-core preset_validator
cargo test -p ralph-core event_origin
cargo test -p ralph-core event_loop
cargo test -p ralph-core smoke_runner
./scripts/run-tests.sh
```

项目要求完成前必须跑完整测试；如果 `cargo-nextest` 不可用，`./scripts/run-tests.sh` 会走 fallback。

## Sources & References

- `docs/brainstorms/2026-05-31-event-origin-guard-requirements.md`
- `docs/plans/2026-05-31-003-fix-event-origin-guard-plan.md`
- `docs/plans/2026-06-02-002-fix-ralph-fallback-origin-contract-plan.md`
- `presets/ce-executor.yml`
- `presets/ce-executor-zh.yml`
- `crates/ralph-cli/presets/ce-executor.yml`
- `crates/ralph-cli/src/presets.rs`
- `crates/ralph-cli/src/hats.rs`
- `crates/ralph-cli/src/preflight.rs`
- `crates/ralph-cli/src/loop_runner.rs`
- `crates/ralph-core/src/preset_validator.rs`
- `crates/ralph-core/src/preflight.rs`
- `crates/ralph-core/src/event_logger.rs`
- `crates/ralph-core/src/event_origin.rs`
- `crates/ralph-core/src/event_loop/mod.rs`
- `crates/ralph-core/src/event_loop/loop_state.rs`
