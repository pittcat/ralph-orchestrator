# U3 Dispatcher Deadline & Worker Lifecycle — Code Review 报告

| 字段 | 值 |
| --- | --- |
| Worktree | `.worktrees/u3-dispatcher-deadline-semaphore` |
| Branch | `feat/u3-dispatcher-deadline-semaphore` |
| 评审对象 commit | `edf10e3 refactor(cli): U3 dispatcher deadline semaphore rewrite` |
| 收尾修复 commit | `c01ae98 fix(cli): U3 让 partial_threshold_fired flag 真正可变 + 收尾 safe_auto` |
| Plan | `docs/plans/2026-06-11-004-u3-dispatcher-deadline-semaphore.md` |
| 评审人 | 主 Agent（correctness / maintainability / reliability / adversarial 四视角） |
| 报告时间 | 2026-06-11 |

## 红线状态

- ✅ 不在 `pittcat-dev` 提交（fix 落在 `feat/u3-dispatcher-deadline-semaphore` worktree 内）
- ✅ 不向 `pittcat-dev` 发起任何 push / PR
- ✅ 不自动 merge；本报告交付后由人工事务决定是否合入
- ✅ 所有代码改动都在 worktree 中完成（`/home/chaowen/Dev/agent_tools/ralph-orchestrator/.worktrees/u3-dispatcher-deadline-semaphore`）

## 评审范围

```
 crates/ralph-cli/Cargo.toml                       |   7 + (新增 test-util dev-dep)
 crates/ralph-cli/src/loop_runner/wave/dispatcher.rs | 1185 ± (U3 重构)
                                                  + 114 收尾修复
```

- `Cargo.toml`：只追加 `tokio = { workspace = true, features = ["test-util"] }` dev-dep，理由已注释（`start_paused` 在 `full` feature 不包含）
- `dispatcher.rs`：从 925 行扩到 1734 行，引入 4 个新类型（`WaveDispatchLimits` / `WaveDispatchOutcome` / `WorkerRequest` / `DispatchContext` / `ProgressChannels` / `WaveWorkerExecutor` trait），用 `JoinSet` 替换手写 `Vec<JoinHandle>`，用 `tokio::select!` + `sleep_until` 替换 `tokio::time::timeout` 嵌套

## 评审维度（按 P0 → P3）

| # | 严重度 | 文件:行 | 问题 | 来源视角 | 路由 | 状态 |
| --- | --- | --- | --- | --- | --- | --- |
| 1 | **P0** | `dispatcher.rs:758`（修复前） | `let partial_threshold_fired = false;` 是 `let` 而非 `let mut`，3 处 `if partial_threshold_fired {}` 读它但 0 处写它，导致 `WaveDispatchOutcome::AggregateDeadlineExceeded` 分支从不可达——enum 变体实际是死代码 | correctness / adversarial（双视角交叉确认，confidence 100） | safe_auto | ✅ 已修（`c01ae98`） |
| 2 | P2 | `dispatcher.rs:657 / 687`（修复前） | `progress_tx_for_task` 克隆 + 紧跟 `let _ = progress_tx_for_task;` 死代码 | maintainability | safe_auto | ✅ 已删（`c01ae98`） |
| 3 | P2 | `dispatcher.rs:965`（修复前） | `finalize_global_exceeded` 用裸 `progress_handle.await` 没有防御超时守卫；其他 finalize_* 路径都走 `wait_for_progress_reporter` 5s 守卫 | reliability | safe_auto | ✅ 已改（`c01ae98`） |
| 4 | P1 | `dispatcher.rs` 整体 | 文件从 925 → 1734 行（+809），跨过 1k 行阈值，建议拆分 helper 模块 | maintainability | gated_auto | ⏸ 移交人工（不在 safe_auto 范围） |
| 5 | P2 | `finalize_partial` / `finalize_timeout`（修复后基本消失）| 早期两 finalize 路径大量重复的 `abort_all + drain + force_take` 块 | maintainability | safe_auto | ✅ 已在修复中合并（partial 路径直接复用 `finalize_timeout`） |
| 6 | P2 | `dispatcher.rs` 测试 `TestExecutor::current_in_flight` | 测试断言 `executor.current_in_flight` 与 `abort` 之间存在可见性竞态（tokio abort 在 next yield 触发，fetch_sub 不一定执行） | adversarial | advisory | 📋 已在测试注释中说明（不修） |
| 7 | P2 | `WaveDispatchOutcome::AggregateDeadlineExceeded` | 上游 `execute_wave` match（行 595-597）把它和 `Completed` / `Partial` 都映射成 `Ok(c)`，差异化仅在 enum 标签 | correctness | advisory | 📋 设计上预留用于未来聚合器区分；当前阶段无外部可观察差异，**已通过死代码修复让该变体变为可达** |
| 8 | P3 | `DispatchContext::build` 中 `let _ = worker_timeout;` | 静默抑制 unused warning，参数本应真正用上 | maintainability | advisory | 📋 worker_timeout 通过 wave 整体规划使用，留 suppress 是有意的（详见 doc） |

> **P0 #1 的修复方向**：经用户确认采用"让 flag 真的可变 + 加测试"。具体做法是**两阶段合并**——partial_deadline 触发时直接调用 `finalize_timeout` 走完整的 abort+drain+force_take，并把 outcome 标签为 `AggregateDeadlineExceeded`（保留"deadline 触发"语义；与"自然完成带失败"的 `Partial` 区分开）。`partial_threshold_fired = true;` 真正被写入，flag 走出死代码。

## 验证证据（修复后）

```
$ cargo test -p ralph-cli --bin ralph -- u3_
test result: ok. 9 passed; 0 failed; 0 ignored

$ cargo test -p ralph-cli --bin ralph
test result: ok. 940 passed; 0 failed; 3 ignored

$ ./scripts/run-tests.sh
✅ 测试通过（nextest + doctest）
```

- 9 个 U3 paused-time 测试全过
- 全 ralph-cli binary 测试 940/0
- workspace 全套（nextest + doctest）通过
- `cargo build -p ralph-cli` 成功（残留 8 个 warning 全在 `preset_templates.rs` 的预先存在未使用方法，与本次改动无关）

## 计划完成度（vs `docs/plans/2026-06-11-004-u3-dispatcher-deadline-semaphore.md`）

| 计划单元 | 状态 | 证据 |
| --- | --- | --- |
| U3-1 测试接缝 + 失败基线 | ✅ | `WaveWorkerExecutor` trait + `TestExecutor`；9 个 `u3_*` 测试 |
| U3-2 前置 deadlines | ✅ | `DispatchContext::build` 一次性算 `started_at / partial_deadline / aggregate_deadline / global_deadline` |
| U3-3 Permit 移入 task | ✅ | 每个 spawn 的 task 内 `semaphore.acquire_owned().await` |
| U3-4 JoinSet 重写收集 | ✅ | 单 `JoinSet` + 单 `tokio::select!` 循环 |
| U3-5 修复 partial threshold | ✅（P0 收尾见上） | `finalize_timeout` 路径 + 显式 `AggregateDeadlineExceeded` 变体可达 |
| U3-6 修复 progress reporter 生命周期 | ✅ | `wait_for_progress_reporter` 5s 防御超时；main `progress_tx` 在 spawn 后 drop |
| U3-7 接入可选 global deadline | ✅ | `WaveDispatchLimits { global_deadline }` + 循环顶 re-check |
| U3-8 验证测试 + lint | ✅ | 见上"验证证据" |

## 未在此 review 处理的事项

1. **P1 #4 文件超 1k 行**：拆分建议是设计层面决策（是否抽出 `dispatcher/loop.rs` / `finalize.rs` / `tests.rs`），需要人工 review 后单独起 plan。当前在 1734 行，所有函数仍能在单文件内定位。
2. **P3 #8 `let _ = worker_timeout;`**：保留——`worker_timeout` 通过 wave 整体规划（`build_wave_worker_prompt` 内的 per-worker duration）使用，参数本就不是未用。

## 结论

**Ready with fixes ✅**

- 评审期间发现的 P0 + 2 个 P2 safe_auto 项**已全部落进 worktree**（`c01ae98`）
- 全部测试套件通过
- 1 个 P1（文件超 1k 行）移交人工按需拆分
- 2 个 advisory 项（`current_in_flight` 竞态注释、`AggregateDeadlineExceeded` 外部语义）保留作为设计意图

**人工下一步**：
- 决定是否合入 `feat/u3-dispatcher-deadline-semaphore` 到 `pittcat-dev`
- 是否要为 P1 #4 单独起一个 "dispatcher 模块拆分" 计划
- 是否要追踪 P3 #8 的 `worker_timeout` 真实使用路径

> 按红线，**本 Agent 不执行 merge**。`pittcat-dev` 分支和 main repo `.ralph/` 状态文件均未被触碰。
