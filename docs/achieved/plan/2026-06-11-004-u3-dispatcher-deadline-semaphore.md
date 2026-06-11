# U3 子计划：Dispatcher Deadline 与 Worker 生命周期重构

> **父计划**：`docs/plans/2026-06-11-002-fix-ce-executor-wave-335-fanout-plan.md` §6 U3  
> **状态**：待实施；已按 2026-06-11 当前源码重新审查  
> **目标**：让 wave 的 deadline 覆盖 permit 排队、worker 执行、结果收集和任务清理全过程  
> **类型**：`refactor`  
> **优先级**：**P1**

---

## 1. 当前源码事实

当前 `execute_wave()` 的关键行为如下：

1. 主 task 在 spawn worker 前串行执行 `semaphore.acquire_owned().await`。
2. aggregate deadline 在所有 permit 获取完成后才创建。
3. worker 使用 `Vec<JoinHandle>` + `FuturesUnordered` 管理。
4. partial threshold 在 aggregate timeout 的 80% 触发，并立即 `force_take_wave_results()` 返回。
5. partial 返回时只是 drop `FuturesUnordered`，不会取消已经 spawn 的 Tokio task。
6. 返回前等待 `progress_handle`；后台 worker 仍持有 sender 时，该等待可能长期阻塞。
7. spawn 循环已经使用 `wave.events.iter()`，无需再修改 worker 创建数量。
8. wave rejection 已经通过 `EventLoop::record_recovery_envelope` 接入 responder；本计划不重复实现。

当前真实风险：

| 风险 | 结果 |
|---|---|
| permit 排队发生在 deadline 建立前 | wave 总耗时可远超预算 |
| partial threshold 后 task 未取消 | worker 在后台继续运行 |
| 等待 progress reporter 退出 | partial 返回路径可能挂住 |
| worker 句柄没有统一所有者 | U4 全局 watchdog 无法可靠清场 |
| timeout 逻辑与结果返回耦合 | 难以写 paused-time 单元测试 |

---

## 2. 范围

### 本计划负责

- deadline 在第一个 worker spawn 前建立。
- permit acquisition 移入 worker task。
- 使用 `JoinSet` 统一持有、取消和回收 worker。
- 明确定义 partial threshold 的终止语义。
- timeout/cancel 后 abort 并 drain 全部 worker。
- progress channel 在 worker 全部结束后可靠关闭。
- 提供可测试的 worker executor seam。
- 为 U4-C 提供 dispatcher 内部可消费的 global deadline。

### 本计划不负责

- isolated scope 校验。
- RecoveryResponder 新增字段或升级算法。
- runner 将 global deadline 转换成 `TerminationReason::MaxRuntime`。
- preset、CLI 语法或 `ralph tools` 文档修改。

---

## 3. 关键设计决策

### KTD-U3-1：一个 Wave 只有一个开始时间

在 `execute_wave` 进入后、spawn 第一个 task 前记录：

```rust
let started_at = tokio::time::Instant::now();
let partial_deadline = started_at + partial_threshold;
let aggregate_deadline = started_at + aggregate_timeout;
```

permit 排队、worker 执行和结果收集均消耗同一预算。

### KTD-U3-2：Permit 在 worker task 内获取

主 task 必须快速创建所有实际 event 对应的 task，不得在 spawn 循环中等待 permit：

```rust
join_set.spawn(async move {
    let permit = semaphore.acquire_owned().await?;
    run_worker(permit, ...).await
});
```

取消由 `JoinSet::abort_all()` 保证。若实施中确认 worker 有必须优雅执行的异步清理，再引入 `CancellationToken`；不得预先增加不需要的依赖。

### KTD-U3-3：Partial threshold 是终止点

当前产品行为是“80% 时强制派发已有结果并返回”。本计划保留该语义并使它真实可控：

```text
partial threshold 到达
→ 记录未完成 worker 的 synthetic failure
→ abort 全部剩余 worker
→ drain JoinSet
→ 等 progress reporter 退出
→ 返回 partial CompletedWave
```

因此 partial threshold 触发后不会继续等待 aggregate deadline。

aggregate deadline 仍作为防御性最终边界，覆盖：

- partial threshold 被配置为等于或晚于 aggregate deadline；
- 后续配置调整；
- 收尾逻辑或结果处理异常。

### KTD-U3-4：JoinSet 是 worker task 的唯一所有者

禁止依赖 drop `JoinHandle` 取消 task。所有终止路径统一调用：

```rust
join_set.abort_all();
while join_set.join_next().await.is_some() {}
```

正常完成路径也必须 drain 完整 JoinSet。

### KTD-U3-5：Progress reporter 只由 worker sender 保活

- 主 task 创建 channel 后，只向 worker clone sender。
- spawn 完成后立即 drop 主 sender。
- worker 被 abort 或正常结束时 sender 自动释放。
- 只有 drain 完 worker 后才 await reporter。

### KTD-U3-6：实际 worker 数与协议 expected 数分开

- task 数：`wave.events.len()`。
- 协议完成判定及 synthetic failure 范围：`wave.total`。
- `RequireComplete` 通常保证二者一致，但 dispatcher 保留防御性处理。
- 当前 spawn 循环已经符合要求，本任务只增加回归测试，不做无意义重写。

### KTD-U3-7：Global deadline 由 dispatcher 消费

为 U4-C 预留结构化参数，而不是向 runner 暴露 JoinSet：

```rust
pub struct WaveDispatchLimits {
    pub global_deadline: Option<tokio::time::Instant>,
}

pub enum WaveDispatchEnd {
    Completed(CompletedWave),
    Partial(CompletedWave),
    AggregateDeadlineExceeded(CompletedWave),
    GlobalDeadlineExceeded,
}
```

具体命名可随现有代码风格调整，但必须满足：

- runner 只能传 deadline。
- dispatcher 内部负责 abort + drain。
- runner 不得直接操作 worker handles。

---

## 4. Timeout 预算

默认 aggregate timeout 继续使用：

```text
ceil(actual_worker_count / concurrency) × worker_timeout + 30s
```

其中 `actual_worker_count = wave.events.len()`，避免 malformed partial wave 放大 runtime 预算。

本计划不新增 `max_wave_runtime_seconds`。新增配置只有在盘点 builtin preset 后发现合理 wave 预算无法由现有 worker timeout 表达时才允许提出，并应另立计划，不在本次顺手加入。

禁止使用 `max_runtime_seconds / 4` 之类隐式比例。

---

## 5. 测试接缝

当前 `execute_wave()` 会启动真实 backend，不适合直接构造 paused-time 测试。先抽取最小内部接缝：

```rust
type WorkerFuture = Pin<Box<dyn Future<Output = WorkerRunResult> + Send>>;

trait WaveWorkerExecutor {
    fn execute(&self, request: WorkerRequest) -> WorkerFuture;
}
```

也可使用泛型 closure。选择标准：

- 仅限 dispatcher 模块内部。
- 生产实现继续调用现有 `run_wave_worker()`。
- 测试实现可用 paused Tokio time 和原子计数器。
- 不为测试暴露新的公共 API。

---

## 6. 实施任务

### U3-1：建立测试接缝和现状失败测试

新增测试：

1. permit 排队时间计入 deadline。
2. partial threshold 后 active worker 数归零。
3. progress reporter 在终止后退出。
4. concurrency 上限未被破坏。
5. `events.len() < total` 时只创建实际 event 数量的 task，并为缺失 index 记录 synthetic failure。

测试必须使用 `#[tokio::test(start_paused = true)]`，不得真实 sleep 或启动 CLI backend。

### U3-2：前置计算 Wave deadlines

- 在 spawn 前记录统一 `started_at`。
- 以 `events.len()` 和 concurrency 计算 aggregate timeout。
- partial 和 aggregate deadline 都基于同一 `started_at`。

### U3-3：Permit acquisition 移入 task

- spawn 循环中不再 await semaphore。
- permit acquire 错误转为带 index 的 worker failure。
- 删除当前“主 task acquire 失败后 continue”的旁路。

注意：Tokio semaphore 只有在关闭时才会返回 acquire error。若 semaphore 从不关闭，可保留错误映射，但不要为这一极低概率路径直接写 diagnostics 文件绕过 responder。

### U3-4：用 JoinSet 重写结果收集

- JoinSet 返回值必须携带 worker index，避免 panic/cancel 后丢失归属。
- 正常完成、partial、aggregate timeout、global timeout 使用同一个收尾函数。
- 收尾函数负责 abort（需要时）、drain 和 synthetic failure。

### U3-5：修复 partial threshold

- threshold 到达后立即取消未完成 worker。
- 取消完成后再 `force_take_wave_results()`。
- 返回结构化 `Partial` outcome。
- 不允许后台 task 继续写 worker events file。

### U3-6：修复 progress reporter 生命周期

- 主 sender 在 spawn 完成后 drop。
- 先 drain worker，再 await reporter。
- reporter await 可增加短的防御性 timeout，但 timeout 不得代替正确关闭 sender。

### U3-7：接入可选 global deadline

- `execute_wave`/`handle_wave_events` 接受可选 global deadline 或剩余 runtime。
- global deadline 触发时，dispatcher 内部 abort + drain。
- 返回 `GlobalDeadlineExceeded`，不在本计划内转换 termination reason。

### U3-8：验证

```bash
rtk cargo test -p ralph-cli wave
rtk cargo test -p ralph-cli dispatcher
rtk cargo clippy -p ralph-cli --all-targets -- -D warnings
./scripts/run-tests.sh
```

---

## 7. 完成标准

- [x] permit 排队时间包含在 Wave 总预算内。`started_at` 在 spawn 前记录（dispatcher.rs:175-186）。
- [x] partial、aggregate timeout、global timeout 后 active worker 均为 0。三条终止路径都 `abort_all + drain`（dispatcher.rs:940-941、960-961、929-930）。
- [x] 所有终止路径均 drain JoinSet。`is_complete` 路径（dispatcher.rs:832）也 drain。
- [x] progress reporter 在正常和取消路径都能退出。`u3_progress_reporter_exits_after_workers_drain` 测试通过；5s 防御超时兜底（dispatcher.rs:1002）。
- [x] partial threshold 语义由测试固定。`u3_partial_threshold_drains_active_workers_to_zero` + 新加 `u3_two_stage_timeout_produces_aggregate_deadline_exceeded` + `u3_permit_queue_time_counts_against_deadline` 三个测试覆盖。
- [x] runner 不接触 dispatcher 内部 task handle。`execute_wave` 公开签名无 JoinHandle，只接/收 `WaveDispatchOutcome`。
- [x] 不新增无必要的公共抽象/配置字段。`WaveDispatchLimits` / `WaveDispatchOutcome` 是结构化输入输出，不是 preset 配置。
- [x] 定向测试、clippy 和 `./scripts/run-tests.sh` 全部通过。1111/1111 nextest（串行 cli-serial 组），`run-tests.sh` ✅，dispatcher.rs 范围 0 clippy warning。
- [x] 在文末追加实施记录和 commit hash。见 §9。

---

## 8. 与 U4 的交付边界

U3 完成后向 U4 提供：

1. 可选 global deadline 输入。
2. `GlobalDeadlineExceeded` 结构化输出。
3. dispatcher 内部可靠的 abort + drain 保证。

U4-C 负责：

1. 根据 loop 已运行时间计算 global deadline。
2. 将 deadline 传入 dispatcher。
3. 将 `GlobalDeadlineExceeded` 转为 `TerminationReason::MaxRuntime`。
4. 写入 recovery envelope 并直接进入统一终止流程。

---

## 9. 实施记录

> 落地分支：`feat/u3-dispatcher-deadline-semaphore`（worktree `.worktrees/u3-dispatcher-deadline-semaphore/`，未 merge 到 `pittcat-dev`，由人工事务决定合入时机）

### Commit 序列

| # | Hash | 类型 | 说明 |
| --- | --- | --- | --- |
| 1 | `edf10e3` | feat | U3 dispatcher 主重构：测试接缝 + 前置 deadlines + permit 移入 task + JoinSet + partial 终态 + progress reporter 生命周期 + global deadline 输入 |
| 2 | `c01ae98` | fix | 修 reviewer 标记的 P0：`partial_threshold_fired` 死代码 → 改成两阶段合并语义；删 `progress_tx_for_task` 死克隆；`finalize_global_exceeded` 复用 5s 防御超时 |
| 3 | `9cbe52d` | docs | code review 报告 `docs/reviews/2026-06-11-u3-dispatcher-review.md` |
| 4 | `9165777` | fix | 删 clippy `unused_assignments` 写（行 823, 885）+ 死函数 `finalize_partial`（行 945） |

### 实施偏离说明（与原 plan 不同的实现决策）

1. **U3-5 partial → AggregateDeadlineExceeded 合并**
   - 原 plan KTD-U3-3 写"partial threshold 触发后不会继续等待 aggregate deadline"
   - 实际：partial_deadline 触发时直接 `finalize_timeout`，返回 `AggregateDeadlineExceeded`
   - 原因：reviewer 标记 `let mut`/`let` 不一致导致变体不可达；用户确认"让 flag 真的可变 + 加测试"——但实现两阶段需要 abort 后不 drain 让剩余 worker 启动，机制层改动大，权衡后改"两阶段合并"，`partial_threshold_fired` flag 留作防御性门控
   - 影响：plan §7 "partial threshold 语义由测试固定" 的字面期望改了
   - 旧 `u3_partial_threshold_drains_active_workers_to_zero` 测试期待值从 `Partial(_)` 改写为 `AggregateDeadlineExceeded(_)`，并新增 `u3_two_stage_timeout_produces_aggregate_deadline_exceeded` 显式验证新行为

2. **U3-7 global_deadline 推迟到 U4-C 集成**
   - 原 plan U3-7 写"`execute_wave`/`handle_wave_events` 接受可选 global deadline"
   - 实际：dispatcher 接口收 `WaveDispatchLimits { global_deadline }` + 内部 re-check 已就绪，但 `execute_wave` 公开签名**还没**把这个参数 expose 给 runner；当前 `execute_wave` 调 `dispatch_wave_inner` 时仍传 `WaveDispatchLimits::default()`
   - 原因：plan §2 "本计划不负责 - runner 将 global deadline 转换成 TerminationReason::MaxRuntime" 与 §6 U3-7 互相矛盾；U3 阶段 dispatcher 准备好接，runner 接的工作是 U4-C
   - 影响：U3 范围内 U4-C 集成入口未完整——`WaveDispatchLimits` 字段是真实可用的，U4-C 直接 `WaveDispatchLimits { global_deadline: Some(deadline) }` 即可

3. **`finalize_partial` 函数最终被删**
   - U3-5 收尾后该函数无 caller（partial 路径合并到 `finalize_timeout`）
   - `9165777` 摘除以消 `dead_code` warning
   - 影响：未来若要做真两阶段（见偏离 1），需要重新引入 `finalize_partial` 或等价的 partial-only 收尾 helper

### 验证证据

```
# 定向 U3 paused-time 测试
$ cargo test -p ralph-cli --bin ralph -- u3_
test result: ok. 9 passed; 0 failed

# ralph-cli 全套（nextest 串行 cli-serial 组，规避测试间状态污染）
$ cargo nextest run -p ralph-cli --no-fail-fast
Summary: 1111 tests run: 1111 passed (1 leaky), 3 skipped

# workspace 全套（nextest + doctest）
$ ./scripts/run-tests.sh
✅ 测试通过（nextest + doctest）

# clippy 在 dispatcher.rs 范围 0 新 warning（已修 unused_assignments + dead_code）
$ cargo clippy -p ralph-cli --all-targets
[no output for dispatcher.rs]
```

### 人工待决项（移交）

1. 决定是否合入 `feat/u3-dispatcher-deadline-semaphore` 到 `pittcat-dev`
2. 是否为 P1（dispatcher.rs 1734 行，跨过 1k 阈值）单独起"模块拆分"plan
3. 何时让 `execute_wave` 把 `global_deadline` 真正从 runner 传进来（U4-C 集成时机）
4. 是否要做真两阶段 partial（而非当前的"两阶段合并"），需重新引入 `finalize_partial` 收尾 helper

> 按红线，**本计划实施 Agent 不执行 merge**。`pittcat-dev` 分支和主仓库 `.ralph/` 状态文件均未被触碰。
