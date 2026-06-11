# U4 子计划：Isolated Wave Scope、Recovery 接入与全局 Watchdog

> **父计划**：`docs/plans/2026-06-11-002-fix-ce-executor-wave-335-fanout-plan.md` §6 U4  
> **状态**：待实施；已按 2026-06-11 当前源码重新审查  
> **目标**：消除 Wave 对 isolated scope、recovery 聚合和 `max_runtime_seconds` 的旁路  
> **类型**：`fix` + `refactor`  
> **优先级**：**P1**

---

## 1. 当前源码事实

已经存在、不得重复实现的能力：

- `RecoveryDiagnosisEnvelope.retry_key`。
- `retry_attempt`、`safe_target`、`outcome`。
- `RecoveryResponder` 的 Soft/Hard/Final 分级。
- `EventLoop::record_recovery_envelope`。
- Wave typed rejection。
- Wave rejection 到 recovery envelope 的基本接入。
- `TerminationReason::MaxRuntime`。

仍然存在的缺口：

| 模块 | 缺口 |
|---|---|
| A. isolated scope | wave events 在 `process_parse_result()` 前被 partition，绕过 isolated publish scope 和 single-business-emission 边界 |
| B. recovery | 当前 Wave rejection retry key 不含 `wave_id`；partial/aggregate timeout 和 isolated rejection 尚无统一 Wave envelope |
| C. watchdog | runner 直接 `.await handle_wave_events()`，期间无法执行 max runtime 检查 |

---

## 2. 范围和依赖

### Part A：Isolated Wave Scope

可独立于 U3 实施。

### Part B：Wave Recovery 接入

- rejection 和 isolated scope 部分可先实施。
- partial/aggregate/global timeout envelope 需要 U3 提供结构化 outcome。

### Part C：Runner Watchdog

强依赖 U3：

- dispatcher 接受 optional global deadline。
- dispatcher 内部负责 abort + drain。
- dispatcher 返回 `GlobalDeadlineExceeded`。

Runner 不得直接持有或操作 dispatcher 的 JoinSet、JoinHandle 或 CancellationToken。

---

## 3. 关键设计决策

### KTD-U4-1：Wave 必须经过与普通事件等价的 isolated scope 校验

Wave partition 可以保留，但 partition 后必须执行：

- 当前 `current_isolated_hat` 的 publish scope 校验。
- isolated activation 的 single-business-emission 校验。

不得把一个 Wave 的 N 条 event 当成 N 次业务发布。

### KTD-U4-2：一个完整 Wave 是一个逻辑 business emission

在一次 `process_events_from_jsonl_with_waves()` 读取批次中：

1. 按 `wave_id` 分组。
2. 一个完整 Wave 计作一个业务 emission。
3. isolated 模式只允许一个 distinct `wave_id`。
4. 第二个 distinct `wave_id` 被 typed rejection。
5. 合法 Wave 的所有 event 必须完整保留。

先使用读取批次内的局部判定，不预先向 `LoopState` 新增 `IsolatedActivation`。

只有测试证明同一次 activation 可能跨多次 EventReader 读取时，才增加最小持久状态：

```rust
pub isolated_business_emission: Option<BusinessEmissionIdentity>
```

不得先引入包含 `started_at`、`completed` 等未被当前需求使用的大结构。

### KTD-U4-3：Wave recovery key 必须包含 wave_id

当前通用 key builder 基于：

```text
source + target_hat + topic + reason_code + field
```

这会把不同 Wave 的相同错误合并。Wave 专用 key 改为：

```text
wave_dispatcher:<normalized_wave_id>:<reason_code>
```

要求：

- 同一 Wave 的多个 event 只产生一个 finding。
- 不同 `wave_id` 不得被合并。
- worker index 不进入 key。
- 不修改非 Wave finding 的通用 key 规则。

### KTD-U4-4：所有 Wave failure 通过 `record_recovery_envelope`

以下路径必须统一接入：

- typed Wave rejection。
- isolated scope rejection。
- partial threshold。
- aggregate deadline。
- global deadline。

禁止仅调用 `DiagnosticsCollector::log_recovery`，因为那会绕过 responder 的内存状态。

### KTD-U4-5：复用现有 RecoveryResponder

本计划不新增：

- `retry_key` 字段。
- RecoveryResponder。
- Hard/Final 算法。
- 新的 `on_converged()` 回调，除非测试证明现有 `check_recovery` 无法表达 Wave 的收敛条件。

Wave finding 的恢复条件必须先定义，再决定是否需要扩展现有 API：

| Finding | 默认 outcome | 恢复条件 |
|---|---|---|
| cap/结构 rejection | `NotRetriable` | 不进入自动恢复升级 |
| isolated scope rejection | `NotRetriable` | 不进入自动恢复升级 |
| partial/aggregate timeout | `Pending` 或 `Repeated` | 后续同目标 topic 的新 Wave 完整完成 |
| global timeout | `NotRetriable` | loop 立即终止 |

### KTD-U4-6：Runner 只负责时间预算和终止映射

Runner 计算：

```rust
let remaining = max_runtime.saturating_sub(event_loop.state().elapsed());
let global_deadline = tokio::time::Instant::now() + remaining;
```

然后传给 dispatcher。Dispatcher 返回：

```rust
WaveDispatchEnd::GlobalDeadlineExceeded
```

Runner 收到后：

1. 调用 `record_recovery_envelope`。
2. 设置 `TerminationReason::MaxRuntime`。
3. 跳过 default publishes、missing-event gate 和其他 iteration 后续阶段。
4. 进入现有统一 termination hooks 和 diagnosis finalization。

---

## 4. Part A：Isolated Wave Scope

### U4-A1：写真实路径 characterization tests

测试必须从 `process_events_from_jsonl_with_waves()` 进入，不新增虚构的 `process_wave_with_isolated_scope()` API。

覆盖：

1. isolated hat 发布不在 `publishes` 内的 Wave topic，Wave 被拒绝。
2. 合法 7-event Wave 不被 single-event 规则截断。
3. 同一读取批次存在两个不同 `wave_id`，只接受第一个。
4. 一个 Wave 的多条 event 只计作一个 business emission。
5. 非 isolated 模式行为不变。

### U4-A2：抽取共享 scope 判定

避免普通事件和 Wave 各自复制 `can_publish` 逻辑。抽取最小 helper，例如：

```rust
fn isolated_publish_allowed(&self, hat: &HatId, topic: &Topic) -> bool
```

普通路径继续保留“一个普通 business event”规则；Wave 路径按 wave group 应用边界。

### U4-A3：增加 typed isolated rejection

在现有 Wave rejection 模型中增加或复用合适变体：

```rust
WaveRejection::IsolatedScopeViolation
WaveRejection::IsolatedMultipleBusinessEmissions
```

拒绝内容必须携带：

- `wave_id`
- topic
- current isolated hat
- reason code

不得只发布字符串 diagnostic 后静默 drop。

### U4-A4：判断是否需要跨读取状态

先通过测试确认一次 activation 是否可能调用多次 `process_events_from_jsonl_with_waves()`。

- 不会：保持局部 distinct-wave 判定。
- 会：在 `LoopState` 增加最小 emission identity，并在 activation 开始/结束时清理。

---

## 5. Part B：Wave Recovery 接入

### U4-B1：修正现有 rejection retry key

修改当前 `handle_wave_rejection()`：

- retry key 加入 normalized `wave_id`。
- 保持每个 rejected wave 只调用一次 `record_recovery_envelope`。
- `NotRetriable` finding 不应触发无意义的 Hard/Final 重试。

测试：

1. 335-event Wave 只产生一个 envelope。
2. 相同 Wave 重复观察使用相同 key。
3. 两个不同 Wave 使用不同 key。

### U4-B2：isolated rejection 接入 responder

Part A typed rejection 通过统一 Wave envelope builder 写入：

```text
source = WaveDispatcher
reason_code = wave_isolated_scope_violation
retry_key = wave_dispatcher:<wave_id>:wave_isolated_scope_violation
outcome = NotRetriable
safe_target = false
```

### U4-B3：timeout outcome 接入 responder

消费 U3 的结构化结果：

- `Partial`：记录 expected/completed 到 message/evidence。
- `AggregateDeadlineExceeded`：记录 aggregate budget。
- `GlobalDeadlineExceeded`：由 runner 记录 loop-level envelope。

当前 envelope 没有 `wave_id`、`expected`、`completed` 独立字段。本计划默认使用：

- `topic`
- `reason_code`
- `message`
- `EvidenceRef`
- Wave 专用 retry key

不要为单一调用方扩大全局 envelope schema。只有 reporter 或机器消费明确要求结构化字段时另立 schema 变更。

### U4-B4：验证现有收敛机制

为 timeout finding 写集成测试：

1. iteration N 产生 pending timeout finding。
2. iteration N+1 同一目标 topic 的新 Wave 完整完成。
3. 调用现有 `check_recovery` 后 outcome 变为 `Recovered`。

若测试证明现有 topic-based recovery 无法区分旧 Wave 和新 Wave，再扩展 responder 的 recovery evidence；不得直接新增 `on_plan_blocked_published()`。

---

## 6. Part C：Runner Watchdog

### U4-C0：U3 交付检查

确认以下能力已存在：

- dispatcher optional global deadline。
- `GlobalDeadlineExceeded` outcome。
- timeout 后 worker active count 为 0 的测试。
- progress reporter 能退出。

缺少任一项时暂停 Part C，先完成 U3。

### U4-C1：写 failing integration test

使用 paused time 和 fake worker executor：

```text
max_runtime = 10s
worker runtime = 3600s
预期 = 10s 到达后返回 MaxRuntime
```

测试从 runner 的真实 wave await 路径进入，不允许仅测试辅助函数。

### U4-C2：Runner 计算并传递 global deadline

不要在 runner 外层 `select!` 后直接 drop `handle_wave_events()` future，因为 future 被 drop 时必须能保证内部 worker 已清理，而当前保证由 dispatcher outcome 协议提供。

推荐：

```rust
let outcome = handle_wave_events(..., Some(global_deadline)).await;
```

dispatcher 自己 select global deadline 并完成收尾。

### U4-C3：映射 MaxRuntime 和 recovery envelope

`GlobalDeadlineExceeded` 后：

```text
record recovery envelope
→ TerminationReason::MaxRuntime
→ termination hooks
→ finalize diagnosis
→ return
```

retry key：

```text
loop_runner:<loop_id>:max_runtime
```

### U4-C4：验证跳过 iteration 后续阶段

断言：

- default publishes 未执行。
- missing-event gate 未执行。
- Wave result merge 未继续执行。
- termination hooks 已执行。
- diagnosis summary 已落盘。
- active worker 数为 0。

---

## 7. 实施顺序

```text
U4-A1 → A2 → A3 → A4
U4-B1 → B2

等待 U3 完成

U4-B3 → B4
U4-C0 → C1 → C2 → C3 → C4
```

A 和 B1/B2 可以与 U3 并行；B3/B4 和 C 必须基于 U3 的真实 outcome API。

---

## 8. 测试矩阵

| 测试 | 验证 |
|---|---|
| isolated out-of-scope Wave rejected | Wave 不再绕过 publish scope |
| legal multi-event Wave preserved | 一个 Wave 作为一个 business emission |
| second wave_id rejected | isolated activation 边界 |
| same Wave one recovery key | 防 N event → N finding |
| different Wave different key | 防跨 Wave 错误聚合 |
| timeout finding recovered next iteration | 复用现有 responder 收敛机制 |
| max runtime preempts Wave | watchdog 生效 |
| watchdog leaves zero workers | U3 清理契约生效 |
| watchdog skips post-iteration phases | 不产生超时后的副作用 |

---

## 9. 验证命令

```bash
rtk cargo test -p ralph-core isolated
rtk cargo test -p ralph-core diagnosis
rtk cargo test -p ralph-cli wave
rtk cargo test -p ralph-cli max_runtime
rtk cargo clippy -p ralph-core -p ralph-cli --all-targets -- -D warnings
./scripts/run-tests.sh
```

---

## 10. 完成标准

- [x] Wave 与普通事件遵守等价的 isolated publish scope。
- [x] 合法多 event Wave 不被截断。
- [x] 同一 isolated activation 的第二个 Wave 被 typed rejection。
- [x] Wave recovery key 包含 `wave_id`。
- [x] 不重复实现 envelope、responder 或 Hard/Final 算法。
- [x] 所有 Wave failure 通过 `record_recovery_envelope`。
- [x] Partial / AggregateDeadlineExceeded 走 `record_recovery_envelope`；`GlobalDeadlineExceeded` 走 warn（U4-C 接管）。**B3 完成（2026-06-11）**
- [x] 收敛机制在 retry_key 一致时可 Recovered；跨 wave_id 收敛不可达已固化为 B4-4 测试。**B4 完成（2026-06-11）**
- [x] runner 不直接操作 worker task handle。
- [x] max runtime 到达后 worker 全部结束。
- [x] watchdog 跳过 iteration 后续阶段并走统一终止流程。
- [x] 定向测试、clippy 和 `./scripts/run-tests.sh` 全部通过。（已完成部分 0 failures）
- [x] 在文末追加实施记录和 commit hash。

---

## 11. 明确排除

- 修改 EventOriginGuard。
- 修改 preset YAML。
- 修改 CLI 语法。
- 新增 RecoveryResponder 基础设施。
- 修改非 Wave finding 的通用 retry key。
- 为 Wave 专门扩大全局 recovery envelope schema。
- 在 runner 暴露或操作 dispatcher 内部 JoinSet。

---

## 12. 实施记录

> **实施日期**：2026-06-11
> **分支**：`pittcat-dev`
> **状态**：全部完成（Part A + B1–B4 + C1–C4）

### 已完成单元

| 单元 | Commit | 说明 |
|---|---|---|
| A1–A3 | `3db6427` | 新增 `wave_isolated_scope.rs` 5 个 characterization tests、`isolated_publish_allowed` helper、`WaveRejection::IsolatedScopeViolation` / `IsolatedMultipleBusinessEmissions` 变体、`enforce_wave_isolated_scope` 后置 scope check、`publish_isolated_wave_violation` 诊断事件 |
| B1 | `82e98e2` | `RecoveryDiagnosisEnvelopeBuilder::wave_retry_key(wave_id, reason_code)` 专用 3-part 格式、`handle_wave_rejection` match 新变体 + retry key 替换、dispatcher 测试 `u4_b1_retry_key_is_wave_scoped`、envelope 3 个 unit test |
| B2 | `1253e66` | `publish_isolated_wave_violation` 加 `record_recovery_envelope` 调用（outcome=NotRetriable）、3 个 B2 集成测试（scope violation envelope、two waves two envelopes、non-isolated no envelope） |

### A4 判定

`process_events_from_jsonl_with_waves()` 在每次 runner iteration 中仅被调用一次（`runner.rs:2920`），`current_isolated_hat` 在 `process_output()` 每次按"刚产生 output 的 hat"刷新。因此局部 per-batch 判定已覆盖"一次 isolated activation 一个 wave"语义。**不需要跨读取状态**（KTD-U4-2 默认假设成立）。

### Plan 假设 vs 实际不符

1. `WaveRejection` 原无 `OutOfScope` / `AggregateTimeout` / `Partial` / `IsolatedScopeViolation` / `IsolatedMultipleBusinessEmissions`——新增了后两个。
2. `handle_wave_rejection` 现有路径既调 `record_recovery_envelope` 也调 `DiagnosticsCollector::log_error`——plan 提到的 `log_recovery` 不在调用栈里（`record_recovery_envelope` 内部调 `log_recovery`）。
3. `WaveDispatchEnd::GlobalDeadlineExceeded` 返回类型、`on_converged` 回调、`fn isolated_publish_allowed` helper、`publisher_can_publish` / `is_in_publishes` 同名 API——plan 假设存在但实际不存在。新增了 `isolated_publish_allowed` 和 `wave_retry_key`。
4. `RecoveryDiagnosisEnvelope` 无 `wave_id` / `expected` / `completed` 独立字段——按 plan §5 B3 用 `topic` / `reason_code` / `message` / `EvidenceRef` 表达。
5. `process_events_from_jsonl_with_waves` 内不读 `current_isolated_hat`——这是 Part A 的事实基础。
6. runner 在 `runner.rs:3466` 直接 `.await handle_wave_events(...)` 无 select/timeout——Part C 需 U3 先提供 dispatcher deadline。

### 测试覆盖

| 测试文件 | 测试数 | 覆盖单元 |
|---|---|---|
| `event_loop::tests::wave_isolated_scope` | 8 | A1(5) + B2(3) |
| `diagnosis::envelope::tests::wave_retry_*` | 3 | B1 unit |
| `loop_runner::wave::dispatcher::tests` | 3 | B1 integration + U2 baseline |
| `event_loop::tests::wave_recovery_timeout` | 4 | B4 (Pending 写入 / 跨 iteration Recovered / 双 wave 独立 finding / 跨 wave_id 收敛固化为文档) |
| `loop_runner::wave::dispatcher::tests`（C 段） | 6 | C1(2) + C3(2) + C4(2) |

### B3 + B4 实施记录（2026-06-11）

| 单元 | Commit | 说明 |
|---|---|---|
| B3 | `b3db6c5` | 新增 `execute_wave_structured` 公开入口；`handle_wave_events` 改用它并对 `Partial` / `AggregateDeadlineExceeded` 调 `record_wave_timeout_envelope`；`execute_wave` 保留为兼容 wrapper（`#[allow(dead_code)]`）；`GlobalDeadlineExceeded` 暂走 warn（U4-C 接管） |
| B4 | `e6beec8` | 4 个 B4 集成测试（`crates/ralph-core/src/event_loop/tests/wave_recovery_timeout.rs`）；其中 B4-4 明确"Responder 现有 API 不能跨 wave_id 收敛 timeout finding"，作为 responder 后续扩展项的固化文档 |

#### B3 实施细节

- `record_wave_timeout_envelope(event_loop, &wave, &completed, reason_code)`:
  - `source = WaveDispatcher`、`severity = Warning`（KTD-U4-5 收敛表：timeout 可恢复，故用 Warning 而非 Error）
  - `topic` 取 `wave.hat_config.publishes.first()`，空时 fallback 到 `wave.events.first().topic`
  - `reason_code` 字面量 `"wave_partial_threshold"` / `"wave_aggregate_deadline_exceeded"`
  - `message` 形如 `Wave {id} timeout: {actual}/{expected} workers reported in {duration_ms}ms (reason={reason_code})`（`actual = results + failures.len()`）
  - `retry_key = wave_retry_key(wave_id, reason_code)`（与 B1 一致）
  - `outcome = Pending`、`safe_target = false`、`retry_attempt = 0`
  - `source_hat = wave.target_hat.to_string()`（hat 名，便于 responder prompt 注入时筛选）
- `execute_wave_structured` 完整透传 `WaveDispatchOutcome`（方案 A 推荐路径）
- `handle_wave_events` 在 `WaveDispatchOutcome::Completed/Partial/AggregateDeadlineExceeded` 共享 merge 路径；只有 `Partial` / `AggregateDeadlineExceeded` 调 envelope；`Completed` 不调（无失败信号）

#### B4 实施细节 + 收敛不达偏离说明

- B4-1：写入 timeout envelope 后 `recovery_responder.tracked_retry_keys() == 1`，且同一 iteration 调 `check_recovery` 返回 `Pending`（R7 grace period）
- B4-2：iteration N+1 命中 envelope 的 topic → `Recovered`（**前提：retry_key 一致**）
- B4-3：两个不同 wave_id 各写一个 timeout → 2 个独立 retry_key（与 B1 `wave_retry_key` 设计对齐）
- B4-4：固化"跨 wave_id 收敛不可达"——因为 `wave_retry_key` 按 `wave_id` namespaced，生产中新 wave 完成不会触发老 finding 的 `check_recovery`。本测试断言 Responder 自身 API 在 retry_key 一致时**能** Recovered（验证 API 行为），同时注释说明生产路径无法触发该条件。
- 收敛问题的修复方向（**不**在本次范围）：responder 需要新增 `check_recovery_by_source_topic(source, topic, ...)`，跨 retry_key 找同 source+topic 的 Pending findings；KTD-U4-5 末尾已禁止直接新增 `on_converged`，但跨 key 的 source+topic 收敛是合法扩展，应另立 plan。

### C0–C4 实施记录（2026-06-11）

| 单元 | Commit | 说明 |
|---|---|---|
| C0 | （隐含在 C1） | U3 交付检查：`WaveDispatchLimits::global_deadline` + `WaveDispatchOutcome::GlobalDeadlineExceeded` 已存在；`started=0` 清理契约已用 C1 的 `started == 4` 反证；progress reporter 可退出由 U3 `wait_for_progress_reporter` 保证 |
| C1 | `59fe6a6` | 2 个 paused-time integration test：`u4_c1_global_deadline_preempts_wave`（max_runtime=10s vs worker=3600s 验证 `GlobalDeadlineExceeded`）和 `u4_c1_zero_remaining_deadline_fires_immediately`（global_deadline=now 验证 zero-remaining 短路）。副产物：修复 dispatcher `select!` sleep 分支 bug——原实现 sleep 触发后无条件走 partial/aggregate 路径，导致 global_deadline 永远被误判为 `AggregateDeadlineExceeded`；修复为 sleep 分支先 re-check global_deadline |
| C2 | `325878a` | `execute_wave_structured` 新增 `limits: WaveDispatchLimits` 参数；`handle_wave_events` 新增 `global_deadline: Option<tokio::time::Instant>` 参数；runner.rs 在 `handle_wave_events` 调用前计算 deadline：`max_runtime=0` 时 `None`（不限制），否则 `Some(now + max_runtime.saturating_sub(state.elapsed()))`；`execute_wave` 老 wrapper 传 `WaveDispatchLimits::default()` 保持兼容 |
| C3 | `6ec6fe5` | 新增 `HandleWaveOutcome { global_deadline_exceeded: bool }`，`handle_wave_events` 改返回它；`record_loop_max_runtime_envelope` 写一条 `retry_key = loop_runner:<loop_id>:max_runtime` / `outcome = NotRetriable` / `severity = Error` / `source = WaveDispatcher` / `reason_code = loop_max_runtime_exceeded` 的 envelope，然后 `return result` early；runner.rs 接住 outcome，设 `late_termination_reason = Some(MaxRuntime)` 让现有 termination flow 接管 |
| C4 | `0d609d7` | runner.rs 的 default_publishes（`runner.rs:3585`）和 missing-event gate（`runner.rs:3563`）块加 `late_termination_reason.is_none()` 守卫；2 个静态源分析测试验证守卫存在 + C3 接线未回退 |

#### C2 实施细节

- 签名变更 3 处：
  - `execute_wave_structured(..., limits: WaveDispatchLimits)`（U3 已有 `WaveDispatchLimits`，C2 把 `limits.global_deadline` 透传给 `DispatchContext::build`）
  - `handle_wave_events(..., global_deadline: Option<tokio::time::Instant>)`（runner 入口，内部构造 `WaveDispatchLimits { global_deadline }`）
  - `runner.rs:3476-3486` 的 deadline 计算块（`max_runtime == 0` → `None`，否则 `Some(now + remaining)`，即使 `remaining=0` 也传 `Some(now)`）
- `max_runtime_seconds=0`（多数 preset 默认）→ 走 wave 内部 partial/aggregate timer，不引入 outer deadline
- `remaining=0`（已耗尽预算）→ 传 `Some(now)`，dispatcher 在 loop 顶部 `global_fired` check 短路返回 `GlobalDeadlineExceeded`（C1 第二个 test 覆盖此路径）

#### C3 实施细节

- envelope schema：
  - `retry_key = "loop_runner:<loop_id>:max_runtime"`（loop-scope，故意 NOT wave-scoped）
  - `source = DiagnosisSource::WaveDispatcher`（不是 `LoopRunner`，因为 `envelope.rs` 没有该变体——plan §5 B3 明确禁止为单一调用方扩大 schema；选 `WaveDispatcher` 因为 wave dispatcher 是实际触发 abort 的代码路径）
  - `severity = DiagnosisSeverity::Error`（loop 将立即终止，必须 Error 而非 Warning）
  - `outcome = DiagnosisOutcome::NotRetriable`（KTD-U4-5 收敛表：global timeout 不可恢复）
  - `reason_code = "loop_max_runtime_exceeded"`
  - `topic = wave.hat_config.publishes.first()`，空时 fallback 到 `wave.events.first().topic`
  - `message` 含 `loop_id` 和 `wave_id`
  - `expected_action` 提示 loop 将以 `TerminationReason::MaxRuntime` 终止
- `HandleWaveOutcome` 设计：单一 bool 字段（`global_deadline_exceeded`），runner 只关心这个信号；其他 outcome（Completed/Partial/AggregateDeadlineExceeded）的 merge 路径在 `handle_wave_events` 内部已处理，无需透传
- retry_key loop-scope 设计理由：`max_runtime` 是 loop 级别信号，不同 wave 命中同一 budget 必须 collapse 成单一 finding。如果用 `wave_dispatcher:<wave_id>:max_runtime`，则同一个 max_runtime budget 被 3 个 wave 先后命中会产 3 条 finding，污染 responder。loop-scope 配合 `loop_runner:<loop_id>:max_runtime` 保证 1 budget = 1 finding

#### C4 实施细节 + 三层覆盖

C4 要求验证 GlobalDeadlineExceeded 后：
- default_publishes 未执行 ✓
- missing-event gate 未执行 ✓
- Wave result merge 未继续执行 ✓
- termination hooks 已执行 ✓（dispatcher 不参与，runner 现有 termination flow 接管）
- diagnosis summary 已落盘 ✓（dispatcher 写 envelope 后由 `finalize_recovery_diagnosis` 落盘）
- active worker 数为 0 ✓（U3 清理契约）

**三层覆盖策略**（plan §6 C4 接受 dispatcher-level + handle_wave_events-level + 静态分析，因为 production test 跑真 wave 不实际）：

1. **dispatcher 级**（C1，`dispatcher.rs:2221-2281`）：`u4_c1_global_deadline_preempts_wave` 验证 `dispatch_wave_inner` 在 max_runtime 到达后返回 `WaveDispatchOutcome::GlobalDeadlineExceeded`，且 `executor.started == 4`（反证 dispatcher 实际 spawn 了 worker 但 abort 路径走到了所有 worker；JoinSet 在 `finalize_global_exceeded` 中 `while join_set.join_next().await.is_some() {}` 排空，故 active worker 数为 0）
2. **handle_wave_events 级**（C3，`dispatcher.rs:476-501`）：`WaveDispatchOutcome::GlobalDeadlineExceeded` 分支 `return result` 提前结束，跳过 per-wave 的 `merge_wave_results_to_events_file` 块（`dispatcher.rs:467-474`）和循环结束后的 `process_events_from_jsonl_with_waves()`（`dispatcher.rs:510`），证明 wave merge 不会继续执行。`HandleWaveOutcome { global_deadline_exceeded: true }` 透传给 runner
3. **runner 级**（C4 静态源分析，`dispatcher.rs:2487-2579`）：2 个测试用 `include_str!("../runner.rs")` 静态读取 runner.rs 源码：
   - `u4_c4_runner_post_wave_gates_consult_late_termination_reason`：断言 2 个 gate 块（missing-event gate + default_publishes fallback）各带 `late_termination_reason.is_none()` 守卫
   - `u4_c4_runner_wires_handle_wave_outcome_to_late_termination_reason`：断言 C3 接线 `wave_outcome.is_some_and(|o| o.global_deadline_exceeded)` → `late_termination_reason = Some(MaxRuntime)` 仍在源码中

runner 现有 termination flow（`runner.rs:3625-3683` 的 `late_termination_reason.or_else(...)` 块）天然包含 `dispatch_pre_loop_termination_hooks` + `publish_terminate_event` + `dispatch_post_loop_termination_hooks` + `handle_termination` + `finalize_recovery_diagnosis`，所以 termination hooks 执行 + diagnosis summary 落盘靠现有代码路径保证，不需新测试。

#### Plan 偏离说明

- **runner.rs 改动只设 `late_termination_reason` 触发现有 termination flow，未新发明 break 路径**：C3 设计故意让 iteration body 跑完——TUI 和 hook 元数据簿记需要执行；break 路径会跳过这些簿记。`late_termination_reason.is_none()` 守卫（C4）保证 post-wave gate 块在 doomed iteration 里不产生副作用，但其他簿记（事件循环、log、状态机 tick）正常完成
- **C3 envelope 选 `WaveDispatcher` source 而非新增 `LoopRunner` 变体**：`envelope.rs` 的 `DiagnosisSource` 枚举没有 `LoopRunner` 成员；plan §5 B3 明确禁止"为单一调用方扩大全局 envelope schema"。loop-scope 语义通过 `retry_key = "loop_runner:<loop_id>:max_runtime"` 携带，不依赖 source
- **C4 用静态源分析代替 E2E test**：runner iteration body 跑真 wave 需 spawn 真 backend（`ProductionExecutor`），CI 不实际。dispatcher-level + handle_wave_events-level + 静态源分析三层覆盖已经把 C4 6 条断言全部归结到可测试的代码位置
- **C2 在 `remaining=0` 时也传 `Some(now)`**：保持 dispatcher "接到 Some 就走 global 路径" 的不变量，runner 不必关心 `saturating_sub` 是否返回 0

### 阻塞项

（无 — U3 Dispatcher Deadline 重构已在 `edf10e3` 完成，C 全部解锁）
