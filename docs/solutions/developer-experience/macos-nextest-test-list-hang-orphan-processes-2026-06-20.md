---
title: macOS 上 cargo nextest 全量测试在 test-list 阶段挂起（XProtect + 孤儿 --list 进程）
date: 2026-06-20
category: developer-experience
module: scripts/run-tests.sh
problem_type: developer_experience
component: testing_framework
severity: high
symptoms:
  - "./scripts/run-tests.sh 或 cargo nextest run --workspace 编译完成后长时间无输出，像死机"
  - "RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh 可正常跑完并实时输出 case"
  - "ps 可见大量 target/debug/deps/* --list 进程堵在 _dyld_start，0% CPU"
root_cause: thread_violation
resolution_type: environment_setup
tags:
  - nextest
  - macos
  - xprotect
  - test-list
  - run-tests
  - orphan-process
  - developer-experience
---

# macOS 上 cargo nextest 全量测试在 test-list 阶段挂起

## Problem

在 macOS 本地跑全 workspace nextest（`./scripts/run-tests.sh` 或 `cargo nextest run --workspace --exclude ralph-e2e`）时，编译结束后进入 **test-list** 阶段会长时间无输出、看似卡死。同一机器上用 `RALPH_BASELINE_SERIAL=1` 走单线程 `cargo test` 则正常。这不是某条 git commit 的功能回归，而是 **nextest 并行 `--list` + macOS XProtect 单线程扫描 + `run-tests.sh` 误杀留下的孤儿进程** 叠加恶化。

## Symptoms

- `Finished test profile` 之后数分钟无任何 `PASS`/`START` 输出
- `pgrep -fl 'target/debug/deps.*--list'` 返回多个进程（曾实测一次 16 个）
- 进程栈停在 `_dyld_start`，CPU 接近 0%
- 清场后裸跑 nextest 可恢复（实测 65s 内跑完 4090/4862 tests）

## What Didn't Work

- **开 Ghostty / Terminal Developer Tools**：只加速单次二进制扫描，不能消除 nextest 再次 8 路并行 `--list` 与 XProtect 的争用
- **逐文件 `codesign` 预热**：199 个二进制太慢，且仍可能在下一轮 test-list 挂起
- **`cargo nextest run --no-build`**：nextest 0.9.100 不支持该 flag（exit 2）
- **归因于 6/20 BDD/scenarios commit**：`fd77546` 仅在同一 `scenarios` 二进制内增测，不显著增加 test-list 并行度；清场后 nextest 本身可用

## Solution

### 1. 立即恢复（清场 + 可靠路径）

```bash
# 杀掉本仓库孤儿 --list / nextest 进程
pkill -9 -f "ralph-orchestrator/target/debug/deps" 2>/dev/null || true
pkill -9 -f cargo-nextest 2>/dev/null || true
rm -rf target/.run-tests.lock.d

# 可靠全量基线（已验证）
RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh
```

### 2. 根因之一：`run-tests.sh` 全局 pgrep 误杀

已提交版 `scripts/run-tests.sh` 在启动时用 `pgrep -f` **全局**匹配 `cargo`/`rustc`/`cargo-nextest`，会误杀其他终端或 agent 的构建，并留下 `target/debug/deps/* --list` 孤儿：

```64:67:scripts/run-tests.sh
      # 兜底:任何 cargo/rustc/nextest 进程(孤儿)
      pgrep -f "cargo (nextest|test|build|doc)" 2>/dev/null || true
      pgrep -f "rustc" 2>/dev/null || true
      pgrep -f "cargo-nextest" 2>/dev/null || true
```

**建议修复方向**（待提交）：将清场范围收窄到本仓库 `target/debug/deps/* --list` 孤儿 + 本 repo 内 nextest 进程树 BFS；禁止并行 `./scripts/run-tests.sh`；macOS 本地默认可回退 serial 或分包 `cargo test -p` 避开 `--list`。

### 3. 诊断 nextest 是否真的坏了

清场后限时跑 nextest，若能看到 `PASS` 行则 nextest 可用、问题是环境脏：

```bash
timeout 120s cargo nextest run --workspace --exclude ralph-e2e 2>&1 | tail -20
```

## Why This Works

1. **nextest test-list**：编译后对每个测试二进制并行执行 `--list --format terse`（`list_threads = num_cpus()`，0.9.100 不可配置）
2. **macOS XProtect**：新二进制首次执行需 Gatekeeper 扫描，实质单线程；多路并行 `--list` → 进程堵在 dyld 加载
3. **孤儿累积**：`run-tests.sh` 杀父留子、多 agent 并行 nextest、debug 会话中断 → `--list` 孤儿占满 XProtect 锁，后续任何 nextest 一进 test-list 即挂
4. **serial 路径绕过**：`cargo test --test-threads=1` 不经过 nextest 的并行 test-list 阶段

## Prevention

- 跑全量前检查孤儿：`pgrep -fl 'target/debug/deps.*--list' | wc -l` 应为 0
- macOS 本地优先 `RALPH_BASELINE_SERIAL=1` 或 targeted `cargo nextest run -p <pkg> -- <subset>`，避免频繁全 workspace test-list
- 不要并行开多个 `./scripts/run-tests.sh` 或多个 agent 全量 nextest
- 修复 `kill_stale_test_processes` 前，避免在有多路 cargo 构建时跑 `run-tests.sh`
- CI 用 `ubuntu-latest` 不受此问题影响

## Related

- `docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md` — ralph-cli 包内 Mutex 串行（不同根因，但同属 nextest 测试入口）
- `docs/solutions/test-failures/nextest-parallel-load-flaky-tests.md` — nextest 并行下的源码 race flaky（非挂起）
- `CLAUDE.md` Build & Test — `RALPH_BASELINE_SERIAL=1` 兜底说明
