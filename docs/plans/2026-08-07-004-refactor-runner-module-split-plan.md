# 将 loop_runner/runner.rs 拆分为可维护模块

## 0. 计划状态

- `READY`；共同基线 `87c88317c94ce5f15d3e17b74755ade3f3b56a47`；全量基线已验证通过。
- 已调查 5,590 行文件、`run_loop_impl`/`run_loop_impl_inner`、timeout 测试和 runner 调用方；`./scripts/run-tests.sh` 通过（Phase 1 7576/7576、Phase 2 23/23、doctest 19/19，4 ignored，退出码 0）。
- 阻塞：无；共享文档已由合并计划 002 独占处理。

### 0.1 0 回归硬门禁

本计划在计划集级阻塞解除前禁止执行。解除前必须在当前 HEAD 运行并保存结果：

- `./scripts/run-tests.sh`
- `cargo build --workspace`
- `just fmt-check`
- `just lint`
- `cargo nextest list --workspace`

每个 Unit 前后必须证明 targeted 命令实际命中了目标测试；ralph-cli 统一使用 `cargo nextest run -p ralph-cli --bin ralph -- <filter>`，不能凭模块名假设命中。纯重构验收还必须逐项核对生产 item、测试函数体、属性、字符串字面量、公开签名、方法数量和方法体 hash；不能只比较编译结果、测试名或测试数量。禁止空 stub、把全部测试塞进 `misc.rs`、遗漏生产区、拆解巨型函数、修改业务逻辑或通过削弱断言获得 Green。共享文档已由合并计划 002 独占处理；本计划不修改该文件。当前 HEAD 全量基线通过后方可执行。

## 1. 功能目标

将 `crates/ralph-cli/src/loop_runner/runner.rs` 拆分而保持 `ralph run` 的 loop 生命周期、超时、终止诊断、supervisor bridge、worktree/事件文件副作用及公共函数路径不变。最大单函数 `run_loop_impl_inner` 必须整体搬移，禁止函数体重构。非目标是修改运行语义、wave dispatcher 或 loop_runner 测试文件。

## 2. 代码库现状与证据

`runner.rs` 生产项从顶部到约 5,130，包含 `run_loop_impl`、约 4,300 行的 `run_loop_impl_inner`、sync timeout 族和 3 个 cfg(test) 区域；`loop_runner/mod.rs` 以 `mod runner` 接入，tests 通过公开/`pub(crate)` helper 消费。

| ID | 来源 | 事实 | 影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `wc -l`/源码 | 5,590 行，inner 单函数约 4,300 行 | 目标与禁拆边界 | 高 |
| E2 | 顶层符号/cfg | entry、run_impl、sync_timeout、测试族可分组 | 文件清单 | 高 |
| E3 | `loop_runner/mod.rs` 与 tests 引用 | runner helper 有 crate 内调用方 | re-export/可见性 | 高 |
| E4 | AGENTS 测试规则 | 时序测试必须 run-tests 两阶段 | 回归命令 | 高 |

## 3. 决策记录与置信度

| ID | 决策 | 选择 | 依据 | 置信度 |
|---|---|---|---|---|
| D1 | 目录模式 | 兄弟目录 | 仓库先例 | 0.96 |
| D2 | inner 处理 | 整体搬移 | 零行为变更，避免借用语义变化 | 0.99 |
| D3 | 测试处理 | timeout 两 mod 整块搬移 | 保持路径/断言 | 0.96 |

## 4. BDD 行为规格

```gherkin
Feature: loop runner 模块拆分
  Scenario: 正常 loop 行为不变
    Given 相同 preset、backend、loop 配置和事件输入
    When 调用拆分后的 runner
    Then 终止状态、事件和诊断不变
  Scenario: 超时行为不变
    Given sync/adapter timeout 边界输入
    When 运行 timeout 路径
    Then timeout envelope 和错误语义不变
  Scenario: 巨型 inner 函数完整搬移
    Given 拆分前方法计数与函数体
    When 完成搬移
    Then inner 函数体未被拆解或改写
```

## 5. 验收与测试策略

| 场景 | 验收 | 入口 | 层级 | 补充 | E2E |
|---|---|---|---|---|---|
| runner 主链 | targeted/full 通过 | `cargo nextest run -p ralph-cli --bin ralph -- runner` | 集成 | run_loop | 否 |
| timeout | timeout tests 通过 | `cargo nextest run -p ralph-cli --bin ralph -- timeout` | 单元/集成 | phase 2 | 否 |
| 全量 | 两阶段脚本 | `./scripts/run-tests.sh` | 回归 | 时序隔离 | 否 |

## 6. 需求—测试追踪矩阵

| ID | 需求 | Scenario | 验收 | 单元 | 集成 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | runner 模块化 | 正常 loop | runner/full | 既有 runner tests | loop integration | 否 | E1-E3 |
| R2 | inner/timeout 不变 | timeout/完整搬移 | targeted/full | timeout tests | `ralph run` | 否 | E1,E4 |

## 7. 严格串行开发单元

### Unit 1：runner 入口、inner 与 timeout 模块拆分

1. 目标：根 ≤150 行；新增 `entry.rs`、`run_impl.rs`、`inner.rs`、`sync_timeout.rs`、`sync_timeout_tests.rs`。
2. 对应 R1/R2、BDD、D1–D3、E1–E3。
3. 结果：inner 约 4,305 行但单函数整体保留；所有新文件 <5,000 行。
4. 基线：`run_loop_impl` 约 591，inner 约 888 起，sync timeout 约 5,216 起；cfg(test) 随宿主项搬移。
5. 输入/输出/错误/副作用：完全不变。
6. 修改边界：仅 runner 根和 runner 子目录；不动 tests/wave。
7. 可依赖：现有 runner API；无其他计划依赖。
8. 禁止：拆 inner、改 async 签名/错误/超时常量。
9. 验收：nextest list、runner/timeout targeted、full。
10. Red：借用/生命周期/时序测试失败即红；不得顺手修逻辑。
11. 单测：原 timeout 与 wiring mod 整块搬移。
12. 顺序：快照 → timeout → run_impl → inner → entry/re-export → targeted → full → close。
13. 最小实现：项级搬移、`use super::*`/精确 use、最小可见性。
14. 集成：runner targeted、`integration_resume`/相关 loop tests、workspace build。
15. 风险：inner 体内 cfg(test) 和 process-global fixture；用 nextest/run-tests 检测。
16. 回归：ralph-cli、workspace、lint、fmt。
17. 文件：根+5 个子模块（E1–E3）。
18. 完成：方法/测试清单、时序测试、全量和行数门禁通过。
19. 停止：发现 inner 语义差异或需改 loop API 时停止。
20. 缓解：函数整体复制/移动，编译器驱动可见性；不编辑函数体。

## 8. Unit 串行依赖图

唯一 Unit 内必须先建立 timeout/entry namespace，再搬 run_impl，最后搬 inner；否则无法区分模块声明错误与 inner 迁移错误。

## 9. 执行命令清单

`cargo nextest list --workspace`；`cargo build --workspace`；`cargo nextest run -p ralph-cli --bin ralph -- runner`；`cargo nextest run -p ralph-cli --bin ralph -- timeout`；`just fmt-check`；`just lint`；`./scripts/run-tests.sh`。失败停止。

## 10. 最终质量门禁

loop/timeout/full、build、fmt、clippy 通过；inner 函数完整且未改逻辑；无测试削弱；根/子文件行数合规；决策 ≥0.85、无 BLOCKED。

## 11. 最终计划自检

| 检查项 | 结果 |
|---|---|
| 独立业务能力 | 是，runner 执行行为 |
| 无未来计划依赖 | 是；共享文档由合并计划 002 独占处理 |
| 证据充分 | 是 |
| 独立性置信度 | 0.95 |
