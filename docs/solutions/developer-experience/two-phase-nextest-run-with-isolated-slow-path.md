---
title: "nextest 两阶段跑：默认并发 + race-sensitive 测试 -j 1 串行隔离"
date: 2026-07-25
category: developer-experience
module: testing-framework
problem_type: developer_experience
component: development_workflow
severity: medium
applies_when:
  - "全 workspace 并发跑测试时，少量测试有硬编码 wall-clock 阈值（≤1.1s）"
  - "这些测试在 num-cpus 并发下偶现 flake，但隔离 / 单线程跑稳绿"
  - "全 workspace 串行兜底（如 RALPH_BASELINE_SERIAL=1）太慢，开发者不愿等"
tags: nextest, test-flake, ci, test-infrastructure, partial_timeout, two-phase
---

# nextest 两阶段跑：默认并发 + race-sensitive 测试 -j 1 串行隔离

## Context

`ralph-cli::loop_runner::tests::wave` 里 3 个 `*_partial_timeout_events_visible` 测试**硬编码 1 秒** worker timeout。隔离跑（其它测试不并发抢 CPU）实测耗时 1.06-1.09s，留给 1s 阈值的余量只有 ~80ms。

在全 workspace 6541 测试 `cargo nextest run --workspace` 并发跑时：
- num_cpus slots 同时跑其它测试，CPU 时间片被分走
- 部分 race-sensitive 测试越过 1s 阈值 → "Worker timed out after 1s without emitting events"
- **同时**这些测试每次单独跑都 PASS — 是经典 nextest 并发 flake

CLAUDE.md 已有 `hooks-executor-test-flake` 同类记录，根因一致：硬 wall-clock 阈值在并发压力下被打破。

**问题**：要保证全量绿，要么走 `RALPH_BASELINE_SERIAL=1` 强制全串行（太慢，~5+ 分钟），要么让这些 flake 继续反复失败。**两种都不能接受**。

## Guidance

### 方案：两阶段跑

`scripts/run-tests.sh` 在 nextest 分支拆成两阶段：

```bash
# Phase 1：默认 num-cpus 并发（快路径），但排除 race-sensitive trio
echo "📦 Phase 1: full workspace at default num-cpus concurrency..."
cargo nextest run \
  --workspace \
  --exclude ralph-e2e \
  -E 'not test(/partial_timeout_events_visible/)'

# Phase 2：trio 单独 -j 1（隔离慢路径）
echo "🐢 Phase 2: race-sensitive trio at -j 1 (3 tests)..."
cargo nextest run \
  --workspace \
  --exclude ralph-e2e \
  -j 1 \
  -E 'test(/partial_timeout_events_visible/)'
```

性能实测（本地 macOS）：
- Phase 1: 6538 tests, ~55s, num-cpus 并发
- Phase 2: 3 tests, ~3s, 1 thread
- **合计 ~58s**，比 `RALPH_BASELINE_SERIAL=1` 全串行 ~5+ 分钟快 5x+。

### 为什么不用其它机制

| 机制 | 问题 |
|---|---|
| `[[profile.X.overrides]]` + `threads-required = 1` | `threads-required` 是**预留** 1 个 thread pool slot，不是独占 CPU；3 个 partial_timeout 各占 1 slot 但剩余 slots 仍跑其它测试，CPU 仍被分时抢占 — 实际验证无效 |
| `#[nextest::test(group = "...")]` + group() filterset | nextest 0.9.105+ 才有 `group()` 表达式；0.9.100 报 parse error。要么升级 nextest 到 ≥0.9.105，要么用 regex filter |
| 升级 `cargo install cargo-nextest --locked` → 0.9.140 | 是的，可行；升级后用 `group()` filterset 替代 regex filter 更整洁。但 phase 1/2 拆分才是关键 — 升级本身只是语法糖 |
| `#[ignore]` 测试 + 手动跑 | 体验差，开发者忘了跑就漏检 |
| `RALPH_BASELINE_SERIAL=1` 全串行 | 太慢，~5x 时间成本 |

### 为什么 phase 1/2 拆分真解决问题

`threads-required = 1` 让测试在 nextest pool 占 1 slot，但 nextest 仍把 3 个 partial_timeout 与其它测试放进**同一个全局 thread pool**，OS 调度器看不到 nextest 内部边界 — 3 个 partial_timeout 各自跑时，剩余 ~num_cpus-3 个测试进程同时抢 CPU，wall-clock 1.08s+ 仍可能。

**两阶段 = 两个独立 nextest 进程**：phase 2 进程独占 OS 视角下的 1 thread（`-j 1`），不与任何其它测试进程共享 CPU。硬 1s 阈值得到 ~99ms 余量，跨机器稳定。

### 必须更新的文档位置

- **`.config/nextest.toml`**：不加 override — phase 2 用 CLI flag 显式触发，不需要静态配置（避免下次升级 nextest 改 group 语义时静默失效）
- **`scripts/run-tests.sh`**：nextest 分支插两阶段
- **`CLAUDE.md` 和 `AGENTS.md`**：在 nextest/version 章节明确"最低 0.9.100；推荐 0.9.140+（拿到 `group()` filterset 表达式 + 性能改进）"

## Why This Matters

1. **速度 vs 正确性的 trade-off 双方都不愿让**：开发者不接受 5 分钟串行（`RALPH_BASELINE_SERIAL=1`），CI 也不接受偶发 flake。两阶段跑同时满足两条。
2. **测试与生产对齐**：partial_timeout trio 的硬 1s 阈值来自真实业务（worker timeout），不是测试 bug；业务上不能放宽阈值来迁就并发。生产中 FileLock 串行 wave emit 不会有这个 race，但 supervisor store 跨进程并发是真实场景 — 必须让测试既保留 1s 阈值、又在 CI 稳定。
3. **可推广**：未来若再有硬 wall-clock 阈值测试（如 backend timeout、IPC deadline），同样追加到 phase 2 filter。regex filter 改成 `test(/partial_timeout|backend_timeout|ipc_deadline/)` 即可，无需改 nextest 配置。
4. **CI 友好**：phase 1/2 都在同一脚本，CI 直接 `./scripts/run-tests.sh` 即可，无需额外参数。

## When to Apply

- 添加新测试时若硬编码了 wall-clock 阈值（`< 1.5s` 实际耗时），考虑加入 phase 2 filter
- phase 1 总耗时超过本地 5 分钟（说明有非 race 测试在做长跑）— 这时考虑是否也需要分流
- **不要**把这条思路套用到"测试逻辑 bug"上 — 真 bug 应该修测试，不是让它们慢跑

## Examples

### ❌ Before: full serial fallback

```bash
# 用户每次跑全量都被吓到（5+ 分钟）
RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh
```

### ❌ Before: 让 flake 反复失败

```bash
# 默认跑 ~58s 但 partial_timeout 三件套偶现失败
./scripts/run-tests.sh
# 每次都得 CI 重跑或本地手工 `cargo nextest run -j 1 -E '...'`
```

### ✅ After: 两阶段跑

```bash
# 默认就稳，~58s
./scripts/run-tests.sh

# 仍然想兜底全串行（CI 极端资源争用时）
RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh
```

### 输出示例

```
🚀 使用 cargo-nextest 并行运行测试...
📦 Phase 1: full workspace at default num-cpus concurrency...
     Summary [  54.903s] 6538 tests run: 6538 passed, 14 skipped
🐢 Phase 2: race-sensitive trio at -j 1 (3 tests)...
        PASS [   1.073s] (1/3) ...partial_timeout_events_visible
        PASS [   1.076s] (2/3) ...partial_timeout_events_visible
        PASS [   1.075s] (3/3) ...partial_timeout_events_visible
     Summary [   3.228s] 3 tests run: 3 passed, 6549 skipped
✅ 测试通过(nextest + doctest)
```

## Related

- `docs/solutions/database-issues/emission-store-concurrent-open-race.md` — 姊妹文档（同 PR 的 store 修复）
- `CLAUDE.md` "hooks-executor-test-flake" — 已知 nextest 并发 flake 的同类记录
- `scripts/run-tests.sh` — 两阶段实现位置
- 升级 nextest：`cargo install cargo-nextest --locked`（→ 0.9.140）；本方案不需要升级，但升级后可改用 `group()` filterset 替代 regex