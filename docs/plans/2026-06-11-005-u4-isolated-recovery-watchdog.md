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

- [ ] Wave 与普通事件遵守等价的 isolated publish scope。
- [ ] 合法多 event Wave 不被截断。
- [ ] 同一 isolated activation 的第二个 Wave 被 typed rejection。
- [ ] Wave recovery key 包含 `wave_id`。
- [ ] 不重复实现 envelope、responder 或 Hard/Final 算法。
- [ ] 所有 Wave failure 通过 `record_recovery_envelope`。
- [ ] runner 不直接操作 worker task handle。
- [ ] max runtime 到达后 worker 全部结束。
- [ ] watchdog 跳过 iteration 后续阶段并走统一终止流程。
- [ ] 定向测试、clippy 和 `./scripts/run-tests.sh` 全部通过。
- [ ] 在文末追加实施记录和 commit hash。

---

## 11. 明确排除

- 修改 EventOriginGuard。
- 修改 preset YAML。
- 修改 CLI 语法。
- 新增 RecoveryResponder 基础设施。
- 修改非 Wave finding 的通用 retry key。
- 为 Wave 专门扩大全局 recovery envelope schema。
- 在 runner 暴露或操作 dispatcher 内部 JoinSet。
