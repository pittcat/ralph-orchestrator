---
date: 2026-06-13
title: ralph-cli loop_runner 测试必须单线程跑——历史记录,2026-07-16 已修复
module: ralph-cli
tags: [loop-runner, fake-path-backend, mock-acp, process-global-mutex, nextest, cli-serial, test-flake, cargo-test, archived]
problem_type: testing-convention
status: superseded-by-2026-07-16-005
---

# ralph-cli loop_runner 测试必须单线程跑 — 历史记录(已修复)

> **状态(2026-07-16)**:**本解决方案已被 plan `2026-07-16-005-refactor-ralph-cli-parallel-tests-plan` 覆盖并修复**。
>
> - `ralph-cli` 现已不在 `cli-serial` 整包 override 下,走 nextest 默认并发。证据:
>   - `.config/nextest.toml` 删除了 `[[profile.default.overrides]]` 中 `package(ralph-cli)` → `cli-serial`
>   - `.ralph/review/2026-07-16-005-refactor-ralph-cli-parallel-tests-plan/scratch/u1-parallel-failure-characterization.md`(U1 + U6 验证)
> - `MOCK_ACP_EXECUTIONS` / `MOCK_ACP_EXECUTION_SERIAL` / `MockAcpExecution` / `AcpWaveExecutionResult` 已被确认是**死代码**并删除(`loop_runner/wave/acp_mock.rs` 文件删除,`wave/mod.rs` 移除 re-export,`tests/common.rs` 移除 helper,`tests/wave.rs` 移除唯一 install 调用方)。
> - `FAKE_PATH_BACKEND_*` 保留(2 个 static Mutex),但在 nextest 进程级隔离下,跨测试不构成实际共享 —— 不再需要整包串行闸。
>
> **本文件保留为历史知识**:不要按本文件的「正确入口」清单做配置变更 —— 现行入口仍是 `cargo nextest run`,但**所有 7 个包**(含 `ralph-cli`)均走默认并发。详见 `CLAUDE.md` / `AGENTS.md` 的 Build & Test 段 + HARD RULE 2。

## Context(历史)

`ralph-cli` 跑测试时,**默认**入口是 `./scripts/run-tests.sh` 或 `cargo nextest run -p ralph-cli --bin ralph`。**裸 `cargo test`**(包括 IDE 默认 runner、CI 偶尔绕开 nextest 的情况)会偶发 fail,症状两类:

- `PoisonError` 类的 lock 中毒报错,大量串在 `install_fake_path_backends(...)` 路径上
- 时间敏感型测试如 `test_process_pending_merges_redirects_subprocess_output_to_log_file` 偶发失败(测试用 500ms sleep 等子进程 flush,并行负载下超过 500ms)

单人本地跑经常看到、CI 用 nextest 路径又稳定,排查容易掉进"重启就好"的坑。

## Guidance(历史,已不再适用)

`crates/ralph-cli/src/loop_runner/tests.rs`(原单文件版本)头部 1-50 行注释把这事讲透了 —— **4 个** process-global 单例,设计故意保留,不能动:

| 全局变量 | 用途 | 现状(2026-07-16) |
|---|---|---|
| `MOCK_ACP_EXECUTIONS` | mock ACP backend 队列 | **已删除**(死代码) |
| `MOCK_ACP_EXECUTION_SERIAL` | mock ACP execution guard | **已删除**(死代码) |
| `FAKE_PATH_BACKEND_SERIAL` | fake-PATH 安装 guard | 保留;nextest 进程隔离下不构成实际共享 |
| `FAKE_PATH_BACKEND_BIN` | fake-PATH bin 目录(共享 TempDir) | 保留;nextest 进程隔离下不构成实际共享 |

**关键事实(历史)**:

- 同 binary 进程内,**所有** 5xx+ 测试共享这 4 个 Mutex + 共享 `FAKE_PATH_BACKEND_BIN` 的文件目录
- 任何测试 panic → `FAKE_PATH_BACKEND_SERIAL` 中毒 → 后续所有走 fake-PATH 的测试在 lock 上炸
- 跨测试的 fake-PATH bin 目录在并发 install/cleanup 下还会出现 fixture 串台
- 作者在 `tests.rs:46-48` 明确说**不要给这些测试加 `#[ignore]`** —— 它们是 production runner code path 的真回归,跳过 = 撤销保护

**承重墙位置(历史)**:

- `.config/nextest.toml`(原):`[[profile.default.overrides]]` 把 `package(ralph-cli)` 整个 binary 划到 `cli-serial` group,`max-threads = 1`
- `scripts/run-tests.sh` 走 nextest 路径
- `CLAUDE.md` / `AGENTS.md` "Build & Test" 段也明确写了 nextest 推荐入口

## Why This Matters(历史)

这条知识是**反着读出来的** —— 很多人(尤其是从其他 Rust 项目切过来的)看到 `cargo test` 默认行为就去裸跑,踩坑后第一反应是"加 `#[ignore]`"或"重构 fixture",但这里是设计决策:`loop_runner` 的 wave / FAKE_PATH 夹具**故意**在 binary 进程内共享,改成 per-test TempDir 会改 fixture 语义,改 `#[ignore]` 会放过真回归。

承重墙在执行层(`.config/nextest.toml` 的 `cli-serial` group),不在测试层。

## When to Apply(历史)

出现以下任一情况时,先核对入口再深挖:

- `ralph-cli` 跑测试时出现 `PoisonError` 或 thread 中毒类报错
- 跑全套时 loop_runner 几个测试 fail,单跑同一测试稳定 pass
- 新人在 IDE 里点 "Run Test" 后看到 loop_runner 偶发挂
- 跨测试访问同一文件路径冲突(共享 `FAKE_PATH_BACKEND_BIN` 目录)

## Examples(历史)

**错的入口**(会偶发 flake):
```bash
cargo test -p ralph-cli
cargo test -p ralph-cli -- --skip foo
```

**对的入口**(历史):
```bash
./scripts/run-tests.sh
cargo nextest run -p ralph-cli --bin ralph
cargo test -p ralph-cli --bin ralph -- --test-threads=1  # 兜底
```

## Related(历史)

- `crates/ralph-cli/src/loop_runner/tests.rs`(原文件)头部 1-50 行注释 — 4 个 process-global Mutex 的源头注释
- `.config/nextest.toml`(原)23-26 行 — `cli-serial` group 配置(**2026-07-16 已删除**)
- `scripts/run-tests.sh` — nextest wrapper
- `CLAUDE.md` / `AGENTS.md` "Build & Test" 段 — 推荐入口
- 同目录 `nextest-parallel-load-flaky-tests.md` — 另一个 nextest 调度下的 flaky 问题(已修)

## See Also(现行)

- `docs/plans/2026-07-16-005-refactor-ralph-cli-parallel-tests-plan.md` — 本修复的 plan
- `.ralph/review/2026-07-16-005-refactor-ralph-cli-parallel-tests-plan/scratch/u1-parallel-failure-characterization.md` — U1+U6 验证证据