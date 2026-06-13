---
date: 2026-06-13
title: ralph-cli loop_runner 测试必须单线程跑——裸 cargo test 会因 process-global Mutex 中毒而 fail
module: ralph-cli
tags: [loop-runner, fake-path-backend, mock-acp, process-global-mutex, nextest, cli-serial, test-flake, cargo-test]
problem_type: testing-convention
---

# ralph-cli loop_runner 测试必须单线程跑

## Context

`ralph-cli` 跑测试时,**默认**入口是 `./scripts/run-tests.sh` 或 `cargo nextest run -p ralph-cli --bin ralph`。**裸 `cargo test`**(包括 IDE 默认 runner、CI 偶尔绕开 nextest 的情况)会偶发 fail,症状两类:

- `PoisonError` 类的 lock 中毒报错,大量串在 `install_fake_path_backends(...)` 路径上
- 时间敏感型测试如 `test_process_pending_merges_redirects_subprocess_output_to_log_file` 偶发失败(测试用 500ms sleep 等子进程 flush,并行负载下超过 500ms)

单人本地跑经常看到、CI 用 nextest 路径又稳定,排查容易掉进"重启就好"的坑。

## Guidance

`crates/ralph-cli/src/loop_runner/tests.rs:14-49` 注释把这事讲透了——4 个 **process-global** 单例,设计故意保留,不能动:

| 全局变量 | 用途 |
|---|---|
| `MOCK_ACP_EXECUTIONS` | mock ACP backend 队列 |
| `MOCK_ACP_EXECUTION_SERIAL` | mock ACP execution guard |
| `FAKE_PATH_BACKEND_SERIAL` | fake-PATH 安装 guard |
| `FAKE_PATH_BACKEND_BIN` | fake-PATH bin 目录(共享 TempDir) |

**关键事实**:
- 同 binary 进程内,**所有** 5xx+ 测试共享这 4 个 Mutex + 共享 `FAKE_PATH_BACKEND_BIN` 的文件目录
- 任何测试 panic → `FAKE_PATH_BACKEND_SERIAL` 中毒 → 后续所有走 fake-PATH 的测试在 lock 上炸
- 跨测试的 fake-PATH bin 目录在并发 install/cleanup 下还会出现 fixture 串台
- 作者在 `tests.rs:46-48` 明确说**不要给这些测试加 `#[ignore]`**——它们是 production runner code path 的真回归,跳过 = 撤销保护

**正确入口**(按优先级):
1. `./scripts/run-tests.sh` — 内部走 nextest,自动用 `cli-serial` group
2. `cargo nextest run -p ralph-cli --bin ralph` — 显式 nextest
3. `cargo test -p ralph-cli --bin ralph -- --test-threads=1` — 裸 cargo test 时的兜底

**承重墙位置**:
- `.config/nextest.toml` 23-26 行:`[[profile.default.overrides]]` 把 `package(ralph-cli)` 整个 binary 划到 `cli-serial` group,`max-threads = 1`
- `scripts/run-tests.sh` 走 nextest 路径
- `CLAUDE.md` "Build & Test" 段也明确写了 nextest 推荐入口

## Why This Matters

这条知识是**反着读出来的**——很多人(尤其是从其他 Rust 项目切过来的)看到 `cargo test` 默认行为就去裸跑,踩坑后第一反应是"加 `#[ignore]`"或"重构 fixture",但这里是设计决策:`loop_runner` 的 wave / FAKE_PATH 夹具**故意**在 binary 进程内共享,改成 per-test TempDir 会改 fixture 语义,改 `#[ignore]` 会放过真回归。

承重墙在执行层(`.config/nextest.toml` 的 `cli-serial` group),不在测试层。**理解这点后,所有"loop_runner 测试 flake"的报告/修复尝试都应该先问"跑的入口对了吗"**——这能省掉未来几小时/天的误调试。

## When to Apply

出现以下任一情况时,先核对入口再深挖:

- `ralph-cli` 跑测试时出现 `PoisonError` 或 thread 中毒类报错
- 跑全套时 loop_runner 几个测试 fail,单跑同一测试稳定 pass
- 新人在 IDE 里点 "Run Test" 后看到 loop_runner 偶发挂
- 跨测试访问同一文件路径冲突(共享 `FAKE_PATH_BACKEND_BIN` 目录)

## Examples

**错的入口**(会偶发 flake):
```bash
cargo test -p ralph-cli
cargo test -p ralph-cli -- --skip foo
```

**对的入口**:
```bash
# 最佳
./scripts/run-tests.sh
# 或
cargo nextest run -p ralph-cli --bin ralph
# 裸 cargo test 时的兜底
cargo test -p ralph-cli --bin ralph -- --test-threads=1
```

如果确认已经在用 nextest 还看到 flake,那才是真 bug——参考同目录的 `nextest-parallel-load-flaky-tests.md`(commit `a38ea8d`,修的是 hook executor race + bench 阈值,不是同问题)。

## Related

- `crates/ralph-cli/src/loop_runner/tests.rs:14-49` — 4 个 process-global Mutex 的源头注释
- `.config/nextest.toml:23-26` — `cli-serial` group 配置
- `scripts/run-tests.sh` — nextest wrapper
- `CLAUDE.md` "Build & Test" 段 — 推荐入口
- 同目录 `nextest-parallel-load-flaky-tests.md` — 另一个 nextest 调度下的 flaky 问题(已修)
