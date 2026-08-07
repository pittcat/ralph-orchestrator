# 将 task_cli.rs 拆分为可维护模块

## 0. 计划状态

- 状态：`READY`；当前 HEAD 全量基线已验证通过。
- 共同基线：`87c88317c94ce5f15d3e17b74755ade3f3b56a47`（当前 `HEAD`）
- 调查范围：`task_cli.rs` 的生产项、内联测试模块、CLI 调用方、模块先例和测试入口。
- 已执行验证：`wc -l`、顶层符号/`cfg(test)` 扫描、`git status`、历史扫描；确认文件为 5,042 行。
- 已执行验证：`./scripts/run-tests.sh` 通过：Phase 1 为 7576/7576，Phase 2 为 23/23，doctest 为 19/19（4 ignored），退出码 0；规划阶段不修改代码。
- 阻塞项：无；共享文档已由合并计划 002 独占处理。

### 0.1 0 回归硬门禁

本计划在计划集级阻塞解除前禁止执行。解除前必须在当前 HEAD 运行并保存结果：

- `./scripts/run-tests.sh`
- `cargo build --workspace`
- `just fmt-check`
- `just lint`
- `cargo nextest list --workspace`

每个 Unit 前后必须证明 targeted 命令实际命中了目标测试；ralph-cli 统一使用 `cargo nextest run -p ralph-cli --bin ralph -- <filter>`，不能凭模块名假设命中。纯重构验收还必须逐项核对生产 item、测试函数体、属性、字符串字面量、公开签名、方法数量和方法体 hash；不能只比较编译结果、测试名或测试数量。禁止空 stub、把全部测试塞进 `misc.rs`、遗漏生产区、拆解巨型函数、修改业务逻辑或通过削弱断言获得 Green。共享文档已由合并计划 002 独占处理；本计划不修改该文件。当前 HEAD 全量基线通过后方可执行。

## 1. 功能目标

将 `crates/ralph-cli/src/task_cli.rs` 从 5,042 行拆为模块根和兄弟子模块，保持 `ralph task` 的参数、输出、错误、权限、任务状态和测试 ID 不变。调用方是 `crates/ralph-cli/src/main.rs` 及 task 集成测试。范围仅限项级搬移、模块声明、必要可见性/导入调整；不拆函数体、不改 CLI 契约、不新增业务能力。

## 2. 代码库现状与证据

入口链为 CLI `task` 命令 → `task_cli::execute` → add/ensure/list/ready/start/close/fail/show/reopen/verify 命令族；`crates/ralph-cli/src/loop_runner/tests/mod.rs` 的测试模块模式提供目录模块先例。生产区约 1–3,090 行，测试模块为 `tests`、`load_coordinator_hats_tests`、`ensure_for_fix_unit_clap_tests`、`task_verify_gate_wiring_tests`。

| Evidence ID | 来源 | 观察结果 | 影响 | 可靠性 |
|---|---|---|---|---|
| E1 | 当前源码与 `wc -l` | 文件 5,042 行，包含 clap 类型、命令执行族和 4 个测试 mod | 确定单文件切片 | 高 |
| E2 | `rg` 顶层项及 `cfg(test)` | 生产区与 4 个测试 mod 边界可由符号定位 | 搬移清单可执行 | 高 |
| E3 | `crates/ralph-cli/src/loop_runner/tests/mod.rs` | 仓库使用 `foo.rs + foo/` 与路径式 `mod` | 选兄弟目录模式 | 高 |
| E4 | `AGENTS.md` 测试规则、`justfile` | 测试入口必须是 `./scripts/run-tests.sh`/nextest | 质量门禁 | 高 |
| E5 | 当前 Git history | HEAD 为最近文档/回归提交，目标文件无本次工作区改动 | 共同基线有效 | 高 |

受影响文件仅为 `task_cli.rs`、新增 `task_cli/*.rs`；测试资产随原模块搬移，不新建共享 fixture。公开类型和 `execute` 路径保持原路径并通过 re-export 兼容。

## 3. 决策记录与置信度

| Decision ID | 问题 | 候选 | 选择 | 证据/排除原因 | 置信度 |
|---|---|---|---|---|---|
| D1 | 目录结构 | `foo.rs + foo/`；`foo/mod.rs` | 前者 | E3；避免重命名根文件 | 0.96 |
| D2 | 测试处理 | 整块搬移；重写测试 | 整块搬移 | E2；零行为变更 | 0.97 |
| D3 | 根文件职责 | 保留全部；模块根+re-export | 后者 | 减少入口认知负荷且保留公开路径 | 0.94 |
| D4 | 不变量 | 测试 ID 逐字节不变 | 是 | task 测试不是扁平跨模块分组 | 0.93 |

## 4. BDD 行为规格

```gherkin
Feature: task CLI 模块拆分
  Scenario: 任务命令行为保持不变
    Given 当前 HEAD 的 task CLI 行为基线
    When 将生产项和测试 mod 搬入 task_cli 子模块
    Then `ralph task` 的参数、输出、错误和状态变化保持不变
  Scenario: 测试清单保持不变
    Given 拆分前保存 ralph-cli nextest ID 清单
    When 完成模块拆分
    Then task 相关测试 ID 集合逐字节一致
  Scenario: 搬移遗漏导致验收失败
    Given 任一项或 cfg(test) helper 未被声明/导出
    When 编译或 targeted nextest
    Then Unit 失败且不得通过删除断言修复
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
|---|---|---|---|---|---|
| 行为不变 | task 测试与集成测试通过 | `cargo nextest run -p ralph-cli --bin ralph -- task_cli` | 集成 | `integration_tasks` | 否 |
| 清单不变 | 前后 nextest list 相同 | `cargo nextest list --workspace` | 清单 | ID diff | 否 |
| 编译完整 | workspace build 通过 | `cargo build --workspace` | 构建 | duplicate/missing item | 否 |
| 全量回归 | 两阶段 nextest 与 doctest 通过 | `./scripts/run-tests.sh` | 回归 | 环境变量污染 | 否 |

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | task_cli 根文件模块化 | 行为不变 | targeted + full | 既有 task tests | `integration_tasks` | 不需要 | E1,E2 |
| R2 | 测试与公开路径不漂移 | 清单不变 | nextest list diff | 既有测试 | CLI build | 不需要 | E2,E3 |

## 7. 严格串行开发单元

### Unit 1：task_cli 生产区与测试区拆分

1. 目标：建立 `task_cli/`，根文件只保留声明、re-export 和 `execute` 分发。
2. 对应：R1/R2、全部 BDD、D1–D4、E1–E3。
3. 观察结果：根文件 ≤250 行；所有新文件 <5,000 行；运行时行为不变。
4. 基线：`CoordinatorHatsError`/clap Args 在 1–564；验证与命令族在 565–3,090；4 个测试 mod 从 3,091 起。
5. 输入/输出：输入现有文件；输出模块树；错误仅允许编译/测试错误；无运行时副作用。
6. 修改边界：新增 `args.rs`、`validation.rs`、`cmd_add_ensure.rs`、`cmd_list_close.rs`、`cmd_fail_verify.rs` 及 4 个测试文件；保留原公开路径。
7. 可依赖：现有代码和 E3 目录模式；不得依赖其他计划变更。
8. 禁止：改函数体、字符串、clap 属性、共享 fixture 或其他大文件。
9. 验收：保存 `cargo nextest list --workspace`；targeted task；全量脚本。
10. Acceptance Red：编译、targeted、清单任一失败即红；正确原因是搬移/声明不完整。
11. 单测：不新增；原 4 个测试 mod 原样搬移。
12. TDD：清单 Red → 搬一个模块/编译 Green → 搬测试/targeted Green → re-export Refactor → integration → regression → close。
13. 最小实现：仅 M1 项级搬移、M2 精确导入、M3 路径式 mod、M4 最小 `pub(super)`。
14. 集成：`cargo nextest run -p ralph-cli --bin ralph -- task_cli`、`cargo build --workspace`。
15. 风险测试：测试 ID diff、`integration_tasks`、带 `RALPH_CURRENT_HAT` 的 env scrub 回归。
16. 回归：ralph-cli 全包、workspace、fmt、clippy。
17. 文件：上述根文件与 9 个新子模块（E1–E3）。
18. 完成：V2 全绿、ID diff 空、根/子文件行数达标、diff 仅限计划文件。
19. 停止：发现逻辑差异、测试 ID 丢失或需改公共接口时停止并重新决策。
20. 风险：cfg(test) helper 归属错误；以编译器和清单检测，随唯一使用者搬移。

## 8. Unit 串行依赖图

`Unit 1` 是本计划唯一 Unit；内部顺序为“测试模块声明 → 生产项 → re-export → 全量回归”，不可交换，因为测试路径和生产可见性必须在同一模块树中验证。

## 9. 执行命令清单

- 开始与结束：`cargo nextest list --workspace`，核对测试 ID。
- 快环：`cargo build --workspace`；`cargo nextest run -p ralph-cli --bin ralph -- task_cli`。
- 静态：`just fmt-check`、`just lint`。
- 最终：`./scripts/run-tests.sh`；仅通过后继续，无需 E2E。

## 10. 最终质量门禁

所有 BDD/既有测试、build、fmt、clippy 通过；无 skipped/`.only`/弱化断言；根 ≤250 行、子文件 <5,000 行；决策均 ≥0.85；无 BLOCKED；Unit 完成 Red→Green→Refactor→Integration→Regression。

## 11. 最终计划自检

| 检查项 | 结果 |
|---|---|
| 实施计划而非 Roadmap | 是 |
| Executor 无关键设计决策 | 是 |
| 文件/接口/命令有证据 | 是 |
| 每 Unit 一个可观察行为 | 是 |
| 不依赖其他计划 | 是，0.95 |
| 无文件/语义所有权冲突 | 未满足：共享 mdc owner 未解决 |
| Scenario 可追踪 | 是 |
