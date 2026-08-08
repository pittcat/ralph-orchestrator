---
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
title: 修复 ce-executor-pipeline headless worktree 的子进程工作目录隔离
date: 2026-08-08
baseline: 691f98f6 (branch: pittcat-dev)
---

# 修复 ce-executor-pipeline headless worktree 的子进程工作目录隔离

## 0. 计划状态

**READY**：当前已调查的实施关键决策置信度均不低于 0.85。该结论只针对本计划的代码修复范围，不等同于已经修复代码。

- 当前基线：`691f98f6`，当前分支 `pittcat-dev`；调查开始时工作树干净。
- 调查范围：`ralph run --worktree` 的父/子进程路径、`ce-executor-pipeline` 的 headless 执行分支、`CliExecutor` 的 cwd/env 注入、PTY/RPC 对照路径、所有 `CliExecutor` 调用方、现有 worktree/adapter 测试、相关历史方案文档。
- 已执行验证：
  - `cargo nextest run -p ralph-cli --test integration_supervisor_runtime_p0 --no-fail-fast`：13/13 通过。
  - `cargo nextest run -p ralph-adapters --test cli_executor_integration --no-fail-fast`：6/6 通过。
  - `cargo nextest run -p ralph-adapters --lib -E 'test(test_execute_passes_ralph_reserved_env_vars)'`：1/1 通过。
  - `git rev-parse --short HEAD`、`git status --short`、`git branch --show-current`。
  - `rg`/`sed` 对调用链、配置、测试和历史文档的静态调查。
- 尚未执行：本计划对应的新回归测试、实现后的 build/lint/full regression；这些属于 Executor 执行阶段，不能在本计划调查阶段替代代码证据。
- 阻塞项：无。若 Unit 1 的真实 headless 验收 Red 不是“子进程写入主 checkout 而非 worktree”，必须按 Unit 停止条件暂停并重新调查，不得以调整断言消除失败。

## 1. 功能目标

### 业务目标与调用方

`ralph run --worktree --no-tui -H builtin:ce-executor-pipeline` 的实际 agent backend 必须在该 loop 的 worktree 中读写代码。调用方是 Ralph CLI 的 operator；直接受影响的是 pipeline 内的 executor/fixer/reviewer 等 agent；间接受影响的是所有使用 headless `CliExecutor` 的 preset 和 bench 路径。

### 当前行为

父进程创建 worktree 后，`run.rs` 将 `config.core.workspace_root` 更新为 worktree，但不会把整个 Ralph 进程的 cwd 改成该路径。headless loop 在 `inner.rs` 选择 `CliExecutor`，调用时只传 prompt/输出/timeout/verbose，没有传 `config.core.workspace_root`。`CliExecutor` 因而从继承的 `RALPH_WORKSPACE_ROOT` 或当前进程 cwd 选择子进程 cwd；在用户复现中该 cwd 是 `pittcat-dev` 或 `main`。

### 目标行为

1. headless `CliExecutor` 每次执行都使用调用方明确传入的 `config.core.workspace_root` 作为子进程 cwd。
2. 子进程的 `RALPH_WORKSPACE_ROOT` 与 `PWD` 与该显式 workspace 一致；backend 自带 env 不能覆盖这两个隔离控制变量。
3. `--worktree` 的真实 agent 写盘只出现在 worktree；主 checkout 不出现由该 agent 产生的代码改动。
4. 非 worktree headless 执行仍使用原有 workspace；bench 和现有 adapter 调用方继续可执行。
5. PTY/TUI/RPC 已有 workspace 传递逻辑保持不变，不能因修复 headless 造成路径分叉。

### 输入、输出与状态变化

- 输入：现有 loop config 的 `config.core.workspace_root`；没有新增 CLI 参数、配置字段或环境变量。
- 输出：backend 进程收到正确 cwd、`RALPH_WORKSPACE_ROOT`、`PWD`；其输出解析和事件处理语义不变。
- 状态变化：agent 对仓库文件的写入从错误的父 checkout 转移到 loop worktree；Ralph 控制面事件文件仍按现有运行时规则写入，不迁移控制面存储。
- 错误语义：workspace 不存在或不可作为 cwd 时沿用 `Command::spawn` 的现有 I/O 错误返回；不得静默 fallback 到父 checkout。若显式路径缺失，测试必须证明执行失败而不是回退。
- 副作用：只允许目标 workspace 内的 agent 工作副作用；主 checkout、其他 worktree 和控制面文件不得被该执行调用写入。

### 兼容、性能、安全与约束

- 兼容性：不改变 preset YAML、event topic、CLI 参数、事件 payload 或 completion 语义；保留 `RALPH_WORKSPACE_ROOT` 作为子进程可见变量，但其值由本次执行的显式 workspace 决定。
- 性能：每次执行只增加一次已有 `PathBuf` 传递和 env 设置，不引入扫描、复制或额外进程；不设新的时延目标。
- 安全/权限：worktree 路径是运行时已解析的 workspace；禁止把未验证的环境变量当作最终 cwd。主 checkout 写入隔离是本计划的安全不变量。
- 已知约束：`CliExecutor` 是跨 crate 的公开类型；`ralph-bench` 也直接调用它。测试必须用 `cargo nextest`，不得裸跑 `cargo test -p ralph-cli`。
- 非目标：不修改 worktree 创建/复用算法；不修改 `PtyExecutor` 的路径选择；不新增 agent branch/worktree 禁令文本；不实现 OS 级文件系统沙箱；不重构 event loop；不修改 preset/schema。
- 已确认假设：pipeline 的 `--no-tui` 路径会落入 `CliExecutor`；PTY/RPC 路径已有正确 workspace 参数；现有 custom backend 测试模式可用于写入 cwd marker。
- 待验证假设：无影响正式设计的待验证假设。Unit 1 必须用真实 CLI 子进程写 marker 重新验证上述事实；若失败形态不同，计划进入停止条件。

## 2. 代码库现状与证据

### 2.1 当前实现入口

外部入口是 `crates/ralph-cli/src/commands/run.rs` 的 `run_command` 路径。`--worktree`/`--worktree-path` 先形成 `LoopContext`，并把 `config.core.workspace_root` 指向 worktree；headless 运行在 `crates/ralph-cli/src/loop_runner/inner.rs`。

调用链：

`ralph run --worktree --no-tui` → `run.rs` 创建/解析 worktree → `inner.rs` 计算 `use_pty = enable_tui || enable_rpc || user_interactive` → `CliExecutor::new(effective_backend)` → `CliExecutor::execute` → `std::process::Command::current_dir` → 外部 backend。

对照链：

`inner.rs` 的 PTY 分支构造 `PtyConfig { workspace_root: config.core.workspace_root.clone(), .. }` → `PtyExecutor`；`run.rs` 的 subprocess-TUI/RPC 子进程也显式设置 child cwd/PWD。因此本次缺口集中在 headless `CliExecutor` 调用链。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `crates/ralph-cli/src/commands/run.rs` worktree 分支，约 1062–1348 行 | 父进程创建/复用 worktree，并把 workspace 写入 loop context/config；没有证据表明同一进程 cwd 被切换到 worktree | 修复必须消费已有 `config.core.workspace_root`，不能假设进程 cwd 已正确 | 高 |
| E2 | `crates/ralph-cli/src/loop_runner/inner.rs` 约 431–448 行 | `--no-tui`、非 RPC、非真实 interactive 时 `use_pty` 为 false，走 headless `CliExecutor` | pipeline 的确定性 headless 复现必须覆盖该分支 | 高 |
| E3 | `crates/ralph-cli/src/loop_runner/inner.rs` 约 3182–3185 行 | headless 调用只传 prompt、stdout、timeout、verbosity，没有传 `config.core.workspace_root` | 直接调用缺少隔离路径，是主修复入口 | 高 |
| E4 | `crates/ralph-adapters/src/cli_executor.rs` 约 94–106 行 | `CliExecutor` 以 `RALPH_WORKSPACE_ROOT` 优先、否则 `current_dir()`，之后先注入 runtime env，再应用 `backend.env_vars` | cwd 必须改为显式参数；运行时隔离变量必须防止 backend env 覆盖 | 高 |
| E5 | `crates/ralph-adapters/src/cli_executor.rs` 约 1038–1065 行 | 既有测试只验证 backend env 透传，没有验证显式 worktree cwd 写盘，也明确记录了 runtime env 覆盖语义 | 必须新增真正观察 cwd/文件副作用的测试，而非只扩展 env 字符串断言 | 高 |
| E6 | `crates/ralph-cli/src/loop_runner/execution.rs` 约 177–184 行 | PTY `PtyConfig.workspace_root` 已取 `config.core.workspace_root` | 不应把 PTY 重做为本计划修复手段；需要 parity 回归 | 高 |
| E7 | `crates/ralph-cli/tests/integration_worktree_isolation.rs` | 已有真实 `ralph` binary、临时 git repo、`--worktree --no-tui`、worktree registry 测试和 custom backend fixture 附近的模式 | 新的 headless cwd/写盘验收应扩展该集成测试，而不是新建平行 harness | 高 |
| E8 | `crates/ralph-cli/tests/integration_run.rs` 约 199–215、310–313、383–386 行 | custom backend 通过脚本、stdin 和 `ralph run` 真实执行；脚本可成为 cwd marker | ATDD 可以使用现有 custom backend 机制验证子进程实际行为 | 高 |
| E9 | `crates/ralph-adapters/tests/cli_executor_integration.rs` | 已有 Unix `sh -c` headless adapter 集成测试和 `CliBackend` fixture | adapter 层可覆盖显式 cwd、PWD、环境优先级，不需真实 AI API | 高 |
| E10 | `crates/ralph-bench/src/main.rs` 约 376–489 | bench 直接构造 `CliExecutor`；执行前显式 `set_current_dir(workspace.path())` | API 变更必须同步 bench 调用，传入其已有 workspace；不得依赖环境 fallback | 高 |
| E11 | `crates/ralph-cli/tests/integration_supervisor_runtime_p0.rs` 约 434–467 | supervisor 测试区分 primary `RALPH_WORKSPACE_ROOT` 与 slot cwd，并验证二者不同 | 不能把“RALPH_WORKSPACE_ROOT 必须等于 primary root”误改成 slot root；本计划只约束 pipeline headless 的显式执行 workspace | 高 |
| E12 | `docs/solutions/developer-experience/cross-plan-mtime-correlation-false-alarm-2026-08-07.md` | 历史调查已区分 worktree 派生问题和运行进程写盘来源，提示不要把 `-c` 路径误当 worktree 选择器 | 范围限定为执行 cwd，不修改 worktree 命名/复用逻辑 | 中 |
| E13 | 已执行的 adapter/CLI nextest 命令 | 现有 headless、supervisor env/cwd 相关测试通过，但未覆盖 pipeline headless 真实写盘到 worktree | 现有绿灯不能证明本 bug 已覆盖；必须增加新验收 | 高 |

### 2.3 受影响范围

- 生产模块：`crates/ralph-adapters/src/cli_executor.rs`；`crates/ralph-cli/src/loop_runner/inner.rs`；必要时 `crates/ralph-bench/src/main.rs` 的调用签名适配。
- 测试模块：`crates/ralph-adapters/tests/cli_executor_integration.rs`；`crates/ralph-adapters/src/cli_executor.rs` 内现有单测；`crates/ralph-cli/tests/integration_worktree_isolation.rs`；必要的现有 `integration_run.rs` 回归。
- 配置/数据：不修改 preset、schema、event payload 或数据库；只读取已有 `config.core.workspace_root`。
- API：改变 `CliExecutor::execute` 的 Rust 调用签名；所有已确认消费者必须同步编译通过。无用户 CLI API 变化。
- UI/外部服务：无；backend 作为真实外部子进程边界，用 shell fixture 代替网络 API。
- 构建目标：`ralph-adapters`、`ralph-cli`、`ralph-bench` 及 workspace 全量测试目标。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | 如何让 headless backend 得到正确 workspace？ | A. 继续依赖 `RALPH_WORKSPACE_ROOT`；B. 在 `CliExecutor` 内重新猜 cwd；C. 由 loop 调用方把 `config.core.workspace_root` 作为显式参数传入 | 选择 C：显式传入执行 workspace，并用它同时设置 `Command::current_dir` 与运行时 workspace env | E1、E3、E4、E6、E13 | A 依赖 ambient env，无法区分主 checkout/loop workspace；B 仍把 adapter 置于错误的配置边界；PTY 已证明配置 workspace 是正确来源 | 0.95 |
| D2 | backend env 与运行时隔离变量冲突时谁优先？ | A. backend env 覆盖；B. runtime workspace 覆盖；C. 清空所有 env | 选择 B：先应用 backend env，再最后写入本次执行的 workspace/PWD/runtime 保留变量；不清空无关 env | E4、E5、E9、E11 | A 可被 stale `RALPH_WORKSPACE_ROOT` 劫持；C 会破坏 agent/runtime 所需 env，超出范围 | 0.92 |
| D3 | 是否新增配置/CLI flag 或改 preset？ | A. 新 flag/config；B. 修改 preset instructions；C. 复用现有 `config.core.workspace_root` | 选择 C，不新增配置或 preset 分叉 | E1、E6、E7、E12 | 真实缺口是执行层丢弃已解析值；prompt 约束不能保证 cwd，新增 operator 参数会重复现有输入 | 0.97 |
| D4 | 是否改 PTY/RPC 作为统一修复？ | A. 改 PTY/RPC；B. 只修 headless，增加 PTY/RPC parity 回归 | 选择 B | E2、E6、E7 | PTY/RPC 已有 workspace 传递，重做会扩大回归面并可能破坏 supervisor 的 primary/slot 语义 | 0.94 |
| D5 | API 如何兼容现有调用方？ | A. 隐式从 env fallback；B. `execute` 增加必需 workspace 参数，调用方显式传值 | 选择 B；adapter 测试和 bench 传临时/bench workspace，CLI 传 `config.core.workspace_root` | E3、E9、E10、`ralph_adapters` 公共导出 | A 正是当前缺陷来源；必需参数让编译器暴露漏改调用方 | 0.90 |

上述决策均已达到 0.85；不存在可进入执行计划的低置信度关键决策。

## 4. BDD 行为规格

```gherkin
Feature: pipeline headless agent 的 worktree 工作目录隔离

  Background:
    Given 一个包含主 checkout 与独立 worktree 的临时 git 仓库
    And Ralph loop 的 workspace 已解析为该 worktree 的绝对路径
    And custom backend 会在收到 prompt 后写入当前目录的 marker 文件并输出当前目录

  Scenario: headless pipeline backend 在 worktree 中写入
    Given Ralph 从主 checkout 启动并使用 --worktree --no-tui
    When loop 通过 headless CliExecutor 执行 custom backend
    Then backend 的当前目录等于 worktree 绝对路径
    And marker 文件存在于 worktree 内
    And marker 文件不存在于主 checkout
    And loop 的 workspace registry 仍指向该 worktree

  Scenario: backend 提供冲突 workspace 环境变量时不能劫持执行目录
    Given backend env_vars 中提供了指向主 checkout 的 RALPH_WORKSPACE_ROOT 与 PWD
    When headless CliExecutor 执行 backend
    Then backend 的当前目录仍等于显式 worktree
    And backend 看到的 RALPH_WORKSPACE_ROOT 与 PWD 都等于显式 worktree
    And主 checkout 没有 marker 写入

  Scenario: 非 worktree headless 执行保持原有 workspace
    Given loop workspace 是当前临时仓库且没有创建 worktree
    When headless CliExecutor 执行 custom backend
    Then backend 在该 workspace 中写入 marker
    And 执行成功、输出解析和 loop completion 语义不变

  Scenario: workspace 不存在时不回退到主 checkout
    Given显式 workspace 路径不存在
    When headless CliExecutor 尝试启动 backend
    Then执行返回 spawn/cwd 错误
    And主 checkout 不产生 marker
    And 不读取当前进程 cwd 作为替代 workspace

  Scenario: PTY/RPC 已有 workspace 传递保持不变
    Given 使用现有 TUI/RPC workspace forwarding 路径
    When backend 被启动
    Then 它仍收到原有 `PtyConfig.workspace_root` 或 child current_dir
    And 不出现 primary workspace 被错误改成 slot worktree 的回归
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
|---|---|---|---|---|---|
| S1 | 断言 backend cwd、worktree marker 存在、主 checkout marker 不存在、执行完成 | `crates/ralph-cli/tests/integration_worktree_isolation.rs` 新增测试 | CLI 集成 | Characterization + 真实子进程写盘；不 mock `Command` | 否，CLI integration 已覆盖关键边界 |
| S2 | 断言冲突 backend env 无法覆盖显式 workspace/PWD | `crates/ralph-adapters/tests/cli_executor_integration.rs` 新增测试 | adapter 集成 | env precedence fault injection | 否 |
| S3 | 断言非 worktree 的 cwd、成功结果、输出解析不变 | 现有 adapter headless tests + 新参数适配 | adapter 单元/集成 | 回归 `test_execute_passes_ralph_reserved_env_vars` | 否 |
| S4 | 断言不存在 workspace 返回错误且主 checkout无副作用 | adapter integration 新测试 | adapter 集成 | 失败恢复/无 fallback | 否 |
| S5 | 断言 PTY workspace 和 supervisor primary/slot 语义不变 | `pty_executor_integration.rs`、`integration_supervisor_runtime_p0.rs` 现有测试 | 集成回归 | 不新增网络/真实 AI E2E | 否 |

所有验收测试必须断言副作用和不变量，不得只检查 `RALPH_WORKSPACE_ROOT` 字符串；S1 必须真正让外部脚本创建文件，以证明 agent 可能写入的位置。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | headless pipeline 使用显式 worktree cwd | S1 | 新增 `headless_worktree_backend_writes_only_to_worktree` | `CliExecutor` 显式 cwd | `integration_worktree_isolation` | 否 | E1–E4、E7 |
| R2 | runtime workspace/PWD 不可被 backend env 覆盖 | S2 | 新增 env precedence 测试 | runtime env helper 断言 | `cli_executor_integration` | 否 | E4、E5、E9 |
| R3 | 非 worktree 与 backend 输出语义不变 | S3 | 现有 headless suite 全绿 | 现有 dispatch/result tests | adapter integration + `integration_run` | 否 | E9、E10 |
| R4 | workspace 不存在 fail-close，不回退主 checkout | S4 | 新增 nonexistent workspace 测试 | error propagation test | adapter integration | 否 | E4、D1 |
| R5 | PTY/RPC/supervisor workspace 语义不回归 | S5 | 现有测试保持通过 | `PtyConfig` 相关测试 | PTY + supervisor runtime | 否 | E6、E11 |
| R6 | 所有 `CliExecutor` 调用方显式完成迁移 | S3 | workspace build/test 编译通过 | 公开 API call-site compile | `ralph-bench` build 与 workspace build | 否 | E10、D5 |

## 7. 严格串行开发单元

Unit 1 → 完成 Acceptance Red、Characterization、回归后 → Unit 2 → 完成全部测试、重构和回归后 → Unit 3 → 完成全部测试、重构和回归后进入最终质量门禁。

### Unit 1：建立 headless pipeline 写盘隔离的真实失败验收

#### 1. Unit 目标

新增一个真实 CLI 集成验收，证明当前实现会把 headless backend 的 marker 写到主 checkout，而不是 worktree；该测试必须在当前实现下以目标行为失败，形成可复现 Red。

#### 2. 对应需求与 Scenario

- Requirement：R1
- Scenario：S1
- Decision：D1、D3
- Evidence：E1–E4、E7–E8、E13

#### 3. 外部可观察结果

测试能区分主 checkout 与 worktree 的文件副作用：目标实现要求 worktree 有 marker、主 checkout 无 marker；当前实现应观察到相反结果或 worktree marker 缺失。

#### 4. 当前行为基线

`run.rs` 已解析 worktree workspace，但 `inner.rs` 的 headless调用未传该值，`CliExecutor`使用进程 cwd/env。现有 worktree 测试只断言 worktree 创建与 registry，不断言 backend 写盘位置（E7、E13），因此必须增加 Characterization/ATDD 测试。

#### 5. 输入与输出

- 输入：临时主 repo、由 `--worktree` 创建的 worktree、custom backend 脚本、`--no-tui`。
- 输出：backend cwd 输出和 marker 文件。
- 错误：测试在修复前必须因 marker 位置错误失败，不得因 backend 未启动、preset lint、fixture 语法错误失败。
- 状态/副作用：只允许测试 fixture 写 marker；测试结束清理临时 repo。
- 不变量：测试进程不修改当前工作树；主 repo 与 worktree 的初始 commit 相同。

#### 6. 修改位置

- `crates/ralph-cli/tests/integration_worktree_isolation.rs`：当前已有 worktree CLI integration、git fixture、`common::ralph_bin()`；新增 custom backend marker fixture 和单个 S1 测试。只增加测试行为，不改变现有 worktree 创建逻辑。
- 必要时复用该文件已存在的 custom backend 配置形态（约 840–936 行附近）；若引用点实际位置不同，以源码为准并只调整测试 fixture，不能修改生产代码。

#### 7. 可依赖能力

现有 `setup_git_repo`、`write_minimal_config`、`common::ralph_bin()`、worktree 创建路径和 custom backend 配置能力。

#### 8. 禁止依赖的未来能力

不得在本 Unit 修改 `CliExecutor`、增加 fallback、修改运行时 env precedence，或把测试断言改成当前错误行为；生产修复属于 Unit 2。

#### 9. 验收测试

- 测试名：`headless_worktree_backend_writes_only_to_worktree`（计划新增）。
- 前置：临时 repo 有初始 commit；主 checkout 预先没有 marker；配置使用 custom backend，backend 脚本写 `pwd` 和 marker。
- 动作：从主 checkout 启动 `ralph run --worktree --no-tui`，让 loop 至少执行一次 custom backend。
- 断言：目标行为为 worktree marker 存在、内容 cwd 等于 worktree；主 checkout marker 不存在；命令成功或达到现有测试允许的终止状态。
- 运行：`cargo nextest run -p ralph-cli --test integration_worktree_isolation --no-fail-fast`。

#### 10. Acceptance Red

先运行上述测试，不改生产代码。有效 Red 是测试进程成功启动 backend，但 `worktree/marker` 不存在且 `main/marker` 存在，或输出 cwd 等于主 checkout；这正好证明 E3/E4 的缺失调用参数。

以下不算有效 Red：custom backend 未找到、配置解析失败、preset lint 在 backend 启动前失败、测试没有执行到 marker 写入、临时 git 初始化失败、测试断言自身 panic。

#### 11. 单元测试拆分

本 Unit 只有一个可观察行为，不拆生产单元测试；只增加一个真实 CLI acceptance/characterization。若 fixture helper 需要独立验证，只验证“脚本可执行并写 marker”，不得 mock 目标 `Command`。

#### 12. Red → Green → Refactor 顺序

1. 写 S1 测试和 custom backend fixture。
2. 运行 `cargo nextest run -p ralph-cli --test integration_worktree_isolation --no-fail-fast`，记录真实失败输出。
3. 仅修正 fixture 使其确认 backend 已执行；不得触碰生产代码。
4. 再次运行确认失败原因稳定地是 cwd/marker 位置。
5. 清理 helper 命名和临时文件断言；再次运行 Unit 1 测试。

#### 13. 最小实现范围

本 Unit 无生产实现；必须留下一个能在修复前 Red、修复后 Green 的真实验收测试。明确不实现 API、env precedence 或 PTY 改动。

#### 14. 集成验证

真实联合 `ralph` binary、`run.rs` worktree 创建、loop runner、custom backend 子进程；不 mock 这些边界。只可 mock/fixture 外部 AI backend 本身。

#### 15. 风险驱动测试

Characterization：现有代码没有真实写盘位置断言，这是本 bug 的直接证据缺口。Fault injection 不在本 Unit 增加；不存在外部网络依赖。

#### 16. 回归范围

运行该 integration 文件全量测试，原因是新增 fixture 会共享 worktree/git helper；不得只运行新测试后关闭 Unit。暂不运行 full workspace，但必须确认没有修改生产代码和没有影响现有测试。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/tests/integration_worktree_isolation.rs` | 新增测试/Fixture | 建立真实 headless worktree 写盘 Red | E7、E8、E13 |

#### 18. 完成标准

新测试在当前代码下以正确 cwd 断言失败；失败原因已记录；现有 `integration_worktree_isolation` 其余测试通过；无生产代码修改、无跳过、无弱化断言；Evidence ledger 更新。

#### 19. 停止条件

若 backend 未执行、无法使用现有 custom backend、失败不是 cwd/marker 位置、或实际路径显示 `run.rs` 已把进程 cwd 切换到 worktree，则停止，补充 Evidence 并重新评估 D1；不得进入 Unit 2。

#### 20. 风险与注意事项

风险：测试可能被 loop 的 completion/preset gate 提前结束。检测：日志/输出显示 backend 是否启动。缓解：复用 `integration_run.rs` 已验证的 custom backend 配置和最小 completion 配置。剩余风险：该测试验证单进程 headless pipeline，不覆盖真实 Claude/Codex API；外部 API 不应进入 CI。

### Unit 2：让 CliExecutor 使用显式 workspace 并锁定 runtime env 优先级

#### 1. Unit 目标

让 headless `CliExecutor` 接收必需 workspace 参数，并保证子进程 cwd、`RALPH_WORKSPACE_ROOT`、`PWD` 均由该参数决定且最终覆盖冲突 backend env。

#### 2. 对应需求与 Scenario

- Requirement：R1、R2、R4、R6
- Scenario：S1、S2、S4
- Decision：D1、D2、D5
- Evidence：E3–E5、E9–E10

#### 3. 外部可观察结果

adapter 直接执行时，backend 在显式 workspace 中运行；错误 workspace 立即返回 spawn/cwd 错误；冲突 env 不改变目录；原有输出解析结果不变。

#### 4. 当前行为基线

当前 `CliExecutor::execute` 没有 workspace 参数，以 `RALPH_WORKSPACE_ROOT`/process cwd 推导 cwd，并且 backend env 应用在 runtime env 注入之后（E4）。Unit 1 已提供 CLI Red。

#### 5. 输入与输出

- 输入：`&Path` 或项目当前采用的等价路径类型；backend；prompt；writer；timeout；verbose。
- 输出：既有 `ExecutionResult` 与输出文本。
- 错误：`Command::spawn` 对无效 cwd 的原始 I/O error 向上返回；不 fallback。
- 不变量：stdout/stderr、timeout、process group、event parsing、backend-specific 非隔离 env 语义不变。
- 副作用：子进程只在显式 workspace 创建 fixture 文件。

#### 6. 修改位置

- `crates/ralph-adapters/src/cli_executor.rs`：把 cwd 解析从 ambient env/current_dir 改为 execute 调用的显式 workspace；调整 env 应用顺序，使 runtime workspace 保留变量最后生效；保留其他运行逻辑。
- `crates/ralph-adapters/tests/cli_executor_integration.rs`：新增显式 cwd、冲突 env、无效 workspace 测试；适配现有 execute 调用。
- `crates/ralph-adapters/src/cli_executor.rs` 现有单测：适配 execute 参数并保留 reserved env 断言；不改 dispatch/result 规则。
- `crates/ralph-bench/src/main.rs`：把已有 `workspace.path()` 传入 execute；不改 bench 的 loop、recording 或 current_dir 生命周期。

#### 7. 可依赖能力

Unit 1 的真实 Red；已有 `CliBackend`、`Command`、`inject_ralph_runtime_env`、Unix shell fixture、bench workspace。

#### 8. 禁止依赖的未来能力

不得修改 `run.rs` worktree 创建、`CoreConfig::default`、PTY executor、preset instructions、event schemas；不得用 `env::set_current_dir` 作为修复。

#### 9. 验收测试

- `explicit_workspace_controls_cli_executor_cwd`：两个临时目录，backend 写 marker；传入 target 后只 target 有 marker。
- `runtime_workspace_overrides_backend_workspace_env`：backend env 提供 source 路径；断言 cwd、`RALPH_WORKSPACE_ROOT`、`PWD` 全是 target。
- `missing_explicit_workspace_returns_spawn_error`：target 不存在；断言 Err，source 无 marker。
- 运行：`cargo nextest run -p ralph-adapters --test cli_executor_integration --no-fail-fast`；src unit 使用 `cargo nextest run -p ralph-adapters --lib --no-fail-fast`；bench compile 在 Unit 2 集成验证执行。

#### 10. Acceptance Red

先运行 Unit 1 的 CLI Red，再添加 adapter 目标测试。目标测试初始可能因新参数未编译而失败；这不是有效行为 Red，必须先使测试 harness 编译到目标逻辑，再记录“当前实现使用 wrong cwd/env precedence”的行为失败。任何只因签名迁移漏改 call site 的编译失败，都必须先修正为测试真实失败。

#### 11. 单元测试拆分

1. cwd 选择：输入 source/target、backend `pwd`/marker；期望 target。
2. runtime env precedence：输入 backend 冲突 `RALPH_WORKSPACE_ROOT`/`PWD`；期望 target；不得 mock env helper 外的真实 `Command`。
3. invalid cwd：输入不存在 target；期望 I/O Err，不允许 fallback。
4. output regression：复用现有 AgentStreamJson success/error/delta 测试，确认 execute 参数迁移未改变 `ExecutionResult`。

#### 12. Red → Green → Refactor 顺序

1. Test 1 Red：显式 target API/断言，确认当前实现无法满足。
2. 最小实现：在 `CliExecutor::execute` 使用显式 target 设置 `current_dir`。
3. Test 1 Green。
4. Test 2 Red：冲突 backend env 覆盖检查。
5. 最小实现：先应用 backend env，再最后注入 runtime workspace 保留变量；保持无关 env 透传。
6. Test 2 Green。
7. Test 3 Red：不存在 target 不得 fallback。
8. 最小实现：移除该路径上的 ambient fallback，保留 spawn error。
9. Test 3 Green；再适配所有 execute 调用和 bench。
10. Refactor：只整理 workspace/env helper，保证所有 runtime 控制变量的来源和顺序可读；运行 adapter 全套测试。

#### 13. 最小实现范围

必须修改 execute 接口和 cwd/env 注入顺序；必须迁移所有编译器发现的消费者；必须保留输出、超时、stderr、process group 和 env 透传。明确不新增 workspace resolver、不改全局 cwd、不改 preset。

#### 14. 集成验证

真实联合 `CliExecutor`、`std::process::Command`、shell backend；backend 可 fake，cwd/env/文件系统行为必须真实。运行 adapter integration、adapter lib、`cargo check -p ralph-bench`（若仓库现有命令允许；最终命令以第 9 节为准）。

#### 15. 风险驱动测试

Fault injection：不存在 workspace，验证 fail-close。Contract：backend 可见 cwd/PWD/RALPH_WORKSPACE_ROOT 的一致性。无需 property/fuzz/mutation；问题是确定性的进程边界，不是解析空间。

#### 16. 回归范围

直接相关：`cli_executor_integration`、`cli_executor.rs` 全部 unit、`pty_executor_integration`（确认 adapter 改动未影响共享 env helper）。公开消费者：`ralph-cli`、`ralph-bench` compile。旧行为：无 worktree headless 输出/timeout/error/delta 测试。默认关闭路径：非 pipeline custom backend。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-adapters/src/cli_executor.rs` | 修改生产文件/新增单测 | 显式 cwd 与 runtime env precedence | E4、D1、D2 |
| `crates/ralph-adapters/tests/cli_executor_integration.rs` | 新增测试/适配调用 | 真实子进程 cwd/env/fail-close | E9 |
| `crates/ralph-bench/src/main.rs` | 修改调用 | 迁移公开 execute 参数 | E10、D5 |

#### 18. 完成标准

S2/S4 和 adapter 单元/集成通过；Unit 1 仍保持 Red 直到 Unit 3 接入 CLI；所有 adapter execute call site 编译通过；无新增依赖、skip、弱断言；D1/D2/D5 置信度不下降。

#### 19. 停止条件

若 `CliExecutor` 的公开接口还有未发现消费者、runtime env 语义与 E5 矛盾、invalid cwd 被其他层吞掉、或必须修改 `CoreConfig` 才能通过，则停止并新增 Evidence/Decision；不得在 Unit 2 临时加入隐式 fallback。

#### 20. 风险与注意事项

风险：改变 env 注入顺序可能影响事件文件变量。检测：运行现有 reserved env test，并检查实际 child env。缓解：只让本次执行的 workspace/PWD/runtime 保留变量最终生效，保留无关 env；明确审查 `RALPH_EVENTS_FILE` 的现有来源。剩余风险：backend 自身可能把 cwd 再次切换，这不属于 Ralph 可控制范围，测试只证明 spawn 初始 cwd。

### Unit 3：把 loop workspace 显式接入 pipeline headless 调用

#### 1. Unit 目标

在 `inner.rs` 的 headless pipeline 调用点把 `config.core.workspace_root` 传给已修复的 `CliExecutor`，使 Unit 1 的真实 CLI 验收转 Green。

#### 2. 对应需求与 Scenario

- Requirement：R1、R3、R5
- Scenario：S1、S3、S5
- Decision：D1、D4
- Evidence：E2、E3、E6、E7、E11

#### 3. 外部可观察结果

从主 checkout 启动 `--worktree --no-tui` 时，pipeline headless backend 只在 worktree 写入；普通 headless run 仍在其 config workspace；PTY/RPC 继续使用原有路径。

#### 4. 当前行为基线

Unit 1 已建立 wrong cwd Red；Unit 2 已使 adapter 能接收 explicit workspace，但 loop 尚未传值时，pipeline 仍不能隔离。

#### 5. 输入与输出

- 输入：`inner.rs` 已有 `config.core.workspace_root`、effective backend、prompt。
- 输出：调用 `CliExecutor::execute` 时传入该 workspace；ExecutionResult 处理不变。
- 错误：adapter 的 explicit cwd error 原样进入现有 loop error/termination path；不增加 fallback。
- 不变量：`use_pty` 判定不变；PTY/RPC 分支不调用新 headless 参数；event loop、hat prompt、completion 不变。

#### 6. 修改位置

- `crates/ralph-cli/src/loop_runner/inner.rs` 约 3182–3185：只修改 headless execute 调用参数，来源固定为 `config.core.workspace_root`；不修改执行模式判定或结果处理。
- `crates/ralph-cli/tests/integration_worktree_isolation.rs`：运行 Unit 1 测试并必要时补充断言，不能重写为 env-only 检查。
- `crates/ralph-cli/tests/integration_run.rs`：只在已有 custom backend/headless 回归因签名适配需要时更新测试配置；无行为扩展。

#### 7. 可依赖能力

Unit 2 已验证的 explicit workspace `CliExecutor`；现有 config workspace、worktree context、CLI integration helpers。

#### 8. 禁止依赖的未来能力

不得在本 Unit 改 `run.rs` worktree 创建、CoreConfig 默认值、PTY/RPC、preset YAML 或在 agent prompt 中增加“不要改主分支”文字作为替代实现。

#### 9. 验收测试

- S1：重新运行 `headless_worktree_backend_writes_only_to_worktree`，应从 Unit 1 Red 变 Green。
- S3：运行现有 `integration_run` custom backend headless 场景，断言 completion/output 原有行为。
- S5：运行已有 supervisor/PTY 相关集成，断言 primary root 与 slot cwd 的原有不变量。

#### 10. Acceptance Red

先在 Unit 2 Green 后运行 S1；预期仍 Red，因为 loop 调用点尚未传 `config.core.workspace_root`。有效失败必须仍是 marker 落在主 checkout/不在 worktree；若已经 Green，说明 Unit 2 意外修改了 loop 行为，必须检查并将实现边界恢复为本 Unit。

#### 11. 单元测试拆分

本 Unit 只需 loop 调用点的编译/集成行为，不新增 mock loop unit。测试必须真实走 `ralph` binary 和 custom backend；不可把 `config.core.workspace_root` mock 成常量而绕过 worktree 创建。

#### 12. Red → Green → Refactor 顺序

1. Test 1 Red：运行 Unit 1 S1，记录 adapter 已修复但 CLI 仍传错/缺失 workspace。
2. 最小实现：在唯一 headless execute 调用点传 `config.core.workspace_root`。
3. Test 1 Green：运行 worktree isolation 集成测试。
4. Test 2 Green：运行已有 non-worktree custom backend 回归，确认 output/completion 不变。
5. Refactor：仅清理调用点注释/参数格式，不能抽出新的 workspace resolver。

#### 13. 最小实现范围

只接线，不重新设计 loop。必须保证 worktree config 是唯一传入值；不改变 PTY/RPC 和普通 loop 的其它行为。

#### 14. 集成验证

真实联合 `run.rs` worktree lifecycle、`inner.rs` headless branch、`CliExecutor`、custom backend 文件写盘。命令：`cargo nextest run -p ralph-cli --test integration_worktree_isolation --no-fail-fast`；随后 `cargo nextest run -p ralph-cli --test integration_run --no-fail-fast`。

#### 15. 风险驱动测试

Characterization：S1 从 Red 到 Green。Contract/parity：S3、S5 防止只修 pipeline 而破坏普通/PTY 路径。无需 E2E 真实模型。

#### 16. 回归范围

直接：worktree isolation、run custom backend、`integration_config_precedence`（workspace/config precedence 可能受调用链影响）。相邻：PTY executor integration、supervisor runtime P0。构建目标：`ralph-cli` binary 与 `ralph-bench`。旧配置：没有新字段，旧 `ralph.yml` 必须继续启动。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/loop_runner/inner.rs` | 修改生产文件 | 将现有 config workspace 接入 headless execute | E3、D1 |
| `crates/ralph-cli/tests/integration_worktree_isolation.rs` | 测试适配/断言 | 使真实 worktree 写盘验收 Green | E7 |
| `crates/ralph-cli/tests/integration_run.rs` | 仅必要时适配测试调用 | 保护普通 custom backend 行为 | E8 |

#### 18. 完成标准

S1/S3/S5 通过；S1 的主 checkout 无 marker、worktree 有 marker；adapter tests 和 CLI integration 通过；无 preset/schema/doc skill 变更（因为 CLI 用户可见能力没有变化）；D1/D4 仍 ≥0.85；Unit 可独立提交。

#### 19. 停止条件

若传入 config workspace 后仍写主 checkout，停止检查 `run.rs` 的 workspace 解析和 `RALPH_WORKSPACE_ROOT` 注入，不得继续添加 agent prompt 约束；若 PTY/supervisor 回归失败，停止并恢复 Unit 3 仅改 headless 调用点的边界。

#### 20. 风险与注意事项

风险：普通 run 与 worktree run 可能共享进程级 env。检测：S1 设置 source/target 对照并断言 marker；S3 使用无 worktree。缓解：不使用全局 `set_current_dir`，每次 Command 显式 current_dir。剩余风险：agent 自行执行 `git -C` 或绝对路径修改主 checkout，这是执行器之外的行为；本计划保证默认 cwd 和 Ralph runtime control path 正确。

## 8. Unit 串行依赖图

```text
Unit 1
  ↓ 真实 CLI Red：证明 bug 在 headless 子进程 cwd/写盘位置
Unit 2
  ↓ adapter 已验证显式 workspace、env precedence、fail-close contract
Unit 3
  ↓ loop 已验证把 config workspace 传到 headless pipeline，S1 Green；随后进入最终质量门禁
```

- Unit 2 不能先于 Unit 1：没有真实 Red，Executor 可能只修 env 字符串而未证明文件副作用。
- Unit 3 不能先于 Unit 2：loop 传参必须依赖已验证的 adapter contract，不能自行决定 fallback。
- 最终质量门禁不能先于 Unit 3：只有 headless 目标行为 Green 后，PTY/RPC/bench parity 才有稳定对照。
- 后续行为不得提前实现：Unit 1 不改生产；Unit 2 不接 loop；Unit 3 不改 worktree 创建；最终门禁不新增行为。

## 9. 执行命令清单

所有命令必须在对应 Unit 的 Red/Green/Regression 时执行；失败不得进入下一步。

| 命令 | 时机 | 目的 | 预期 | 失败处理 |
|---|---|---|---|---|
| `cargo nextest run -p ralph-cli --test integration_worktree_isolation --no-fail-fast` | Unit 1/3 | 真实 worktree 创建和 headless marker | Unit 1 Red，Unit 3 Green | 记录失败；非预期 Red 停止 |
| `cargo nextest run -p ralph-adapters --test cli_executor_integration --no-fail-fast` | Unit 2/4 | explicit cwd/env/error contract | 全部通过 | 返回 Unit 2 修复 |
| `cargo nextest run -p ralph-adapters --lib --no-fail-fast` | Unit 2/4 | adapter unit/output regression | 全部通过 | 不得跳过 |
| `cargo nextest run -p ralph-cli --test integration_run --no-fail-fast` | Unit 3/4 | 普通 custom backend/headless 回归 | 全部通过 | 回到 Unit 3 |
| `cargo nextest run -p ralph-adapters --test pty_executor_integration --no-fail-fast` | Unit 3 完成前 | PTY path parity | 全部通过 | 检查未改 PTY 语义 |
| `cargo nextest run -p ralph-cli --test integration_supervisor_runtime_p0 --no-fail-fast` | Unit 3 完成前 | primary/slot workspace contract | 全部通过 | 检查 D4/E11 |
| `cargo check -p ralph-bench` | Unit 2/3 | 公开 adapter 调用方迁移 | 通过 | 修复遗漏调用方后重跑 |
| `cargo fmt --check` | Unit 3 完成后 | 格式 | 通过 | 仅格式化计划范围 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Unit 3 完成后 | lint/type correctness | 通过 | 修复真实 lint，不降级规则 |
| `cargo build --workspace` | Unit 3 完成后 | workspace build | 通过 | 不进入最终门禁 |
| `./scripts/run-tests.sh` | Unit 3 完成后最终 | 仓库规定的全量 nextest/文档测试入口 | 通过 | 如遇已知 race 按 AGENTS 规定记录并使用 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 作为最后兜底；仍失败则未完成 |

不新增 E2E 真实模型命令：仓库要求 replay/smoke 优先，本问题的关键边界由真实 CLI + fake backend 已足够证明。

## 10. 最终质量门禁

- S1–S5 均有可执行测试且通过；S1 明确证明 worktree 有 marker、主 checkout 无 marker。
- R1–R6 均能回溯到 Scenario、测试、Unit 和 Evidence。
- adapter、CLI、PTY、supervisor、bench 相关目标通过；旧配置和默认关闭路径通过。
- `cargo fmt --check`、clippy、build、`./scripts/run-tests.sh` 通过；不使用裸 `cargo test -p ralph-cli`。
- 无新增 skip/only、无削弱断言、无无解释 snapshot/golden、无临时 debug 文件。
- 不修改 preset/schema/agent skill 文档；若实现阶段意外改变 agent 可见能力或 CLI 行为，必须停止并重新评估文档同步清单，不能把变更偷偷纳入本计划。
- 无未处理 BLOCKED 决策；所有实施关键决策 ≥0.85；剩余风险仅限 agent 自行使用绝对路径或 OS 级权限边界。
- Unit 严格按 1→2→3→4 完成，每个 Unit 有 Acceptance Red、Unit Red/Green、Refactor、Integration、Regression、Close 记录。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 有真实入口、调用链、Red、文件边界、命令和 DoD |
| Executor 是否仍需做关键设计决策 | 否 | D1–D5 已固定显式参数、优先级、范围和错误语义 |
| 所有文件和接口是否有代码库证据 | 是 | E1–E11；新增测试位置明确标注为新增 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1 0.95、D2 0.92、D3 0.97、D4 0.94、D5 0.90 |
| 是否存在未处理的低置信度假设 | 否 | Unit 1 只做已定义的真实复验，不承载未决架构选择 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 建 Red、U2 adapter contract、U3 loop 接线；最终回归不作为 Unit |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 有命令、失败语义、回归和完成标准 |
| 每个 Unit 是否有真实 Red | 是 | U1/2/3 均明确真实 Red；最终质量门禁只验证既有回归，不冒充开发 Unit |
| 每个 Unit 是否包含回归范围 | 是 | 每个 Unit 第 16 节列出直接/相邻/公开消费者 |
| 是否存在未来 Unit 依赖 | 否 | 依赖严格线性且后续能力未提前实现 |
| 是否存在泛化任务描述 | 否 | 每项绑定到文件、符号、输入、输出、断言和命令 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | S1–S5 → R → 测试入口 → Unit |
| 所有关键决策是否有 Evidence | 是 | D1–D5 均引用 E 编号 |
| 计划是否可以严格串行执行 | 是 | Unit 1→2→3→4 及停止条件完整 |
