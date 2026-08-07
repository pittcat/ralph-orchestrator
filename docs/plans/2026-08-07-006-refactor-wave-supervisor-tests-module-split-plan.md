# 将 wave_supervisor 测试拆分为模块

## 0. 计划状态

- `READY`；更新日期 2026-08-07；实施基线 `5791e21b`；当前尚未执行拆分。
- 调查：`wave_supervisor.rs` 当前 9,423 行、由 `tests/mod.rs` 接入，包含共享 fixture、slot binding、dispatch、timeout、coordination、salvage、redrive 和 supervisor bridge 测试族。
- 当前验证：`cargo build --workspace`、`just fmt-check`、`just lint` 和格式/warning 修复后的 `./scripts/run-tests.sh` 均通过；Phase 1 为 7576/7576、Phase 2 为 23/23、doctest 为 19/19（4 ignored）。
- 阻塞：无；共享文档由合并计划 002 独占处理，本计划不修改 `.cursor/rules/state-management.mdc`。

### 0.1 0 回归硬门禁

本计划开始前必须在当前 HEAD 运行并保存结果：

- `./scripts/run-tests.sh`
- `cargo build --workspace`
- `just fmt-check`
- `just lint`
- `cargo nextest list --workspace`

每个 Unit 前后必须证明 targeted 命令实际命中了目标测试；ralph-cli 统一使用 `cargo nextest run -p ralph-cli --bin ralph -- <filter>`，不能凭模块名假设命中。纯重构验收必须保存测试 ID 多重集、测试函数体 hash、属性和字符串字面量清单；不能只比较编译结果、测试名或测试数量。禁止空 stub、把全部测试塞进 `misc.rs`、遗漏 fixture/测试、拆解巨型 helper、修改业务逻辑或通过削弱断言获得 Green。

## 1. 功能目标

将 wave supervisor 的 9,423 行纯测试拆成按 slot binding、dispatch、timeout、coordination、salvage、supervisor 等行为族的测试模块，保持真实 worktree、bridge、fan-in、deadline、failure classification 和 retry 断言不变。非目标是修改 dispatcher、supervisor 生产代码、共享 fixture 或事件协议。

## 2. 代码库现状与证据

文件由 `tests/mod.rs` 的 `mod wave_supervisor` 接入，顶部含 SpyBindingBridge/RecordingFactory/U3/U5 测试基础设施，后续覆盖 slot、dispatch、barrier、fan-in、deadline、salvage。因是扁平纯测试文件，模块化后允许测试路径中段变化。

| ID | 来源 | 观察 | 影响 | 可靠性 |
|---|---|---|---|---|
| E1 | 当前 `wc -l` | 9,423 行纯测试 | 独立切片 | 高 |
| E2 | 顶层 fn/类型扫描 | 行为族和 test helper 可按前缀分组 | 分组依据 | 高 |
| E3 | tests/mod 与 wave imports | 依赖父层 glob、wave 生产 API | 声明/导入保留 | 高 |
| E4 | AGENTS/run-tests | 时序/并发测试需 nextest + 两阶段 | 门禁 | 高 |
| E5 | 最近 Git history | dispatcher 有 deadline 回归提交 | 必须保留既有回归 | 高 |

### 2.1 当前行为族与搬移边界

| 目标模块 | 主要内容 | 约束 |
|---|---|---|
| `slot_binding.rs` | bridge/worktree binding、环境变量、共享 readonly、失败闭环 | 保留真实 `SpyBindingBridge`/factory，不复制 fixture |
| `dispatch.rs` | U3/U4 dispatch approval、cap、FIFO、spawn 前失败 | 保留真实 dispatcher/bridge 交互 |
| `timeouts.rs` | timeout、retry、fresh process/cwd、aggregate deadline | 不修改 timeout 常量和时间断言语义 |
| `coordination.rs` | U5 slot outcome、retry budget、fan-in、ledger/dedup | 保留 success/failure/partial 三类路径 |
| `salvage_merge.rs` | salvage、redrive payload、zero-completed、failure classification | 不把 salvage 断言降级为“事件存在” |
| `supervisor.rs` | U2/U4/U5/U6/U7/S2/S3/S4 redrive、projection、resume boot | 保留 fail-closed 和 descriptor/digest 断言 |
| `misc.rs` | 仅收纳无法归类的小型稳定性/契约测试，目标 ≤600 行 | 禁止成为剩余大杂烩；超过 600 行必须继续按前缀拆 |

共享 fixture（`SpyBindingBridge`、`RecordingFactory`、wave builders、测试 executor）只放在首个消费者模块或独立 `fixtures.rs`，二选一，不能在多个子模块重复定义。

## 3. 决策记录与置信度

| ID | 决策 | 选择 | 依据 | 置信度 |
|---|---|---|---|---|
| D1 | 目录 | `wave_supervisor.rs + wave_supervisor/` | E3 | 0.96 |
| D2 | 分组 | 按测试前缀/行为族 | E2 | 0.94 |
| D3 | 不变量 | 测试名多重集+计数不变 | 扁平文件限制 | 0.97 |

## 4. BDD 行为规格

```gherkin
Feature: wave supervisor 回归测试模块化
  Scenario: slot/dispatch/supervisor 回归完整
    Given 现有真实 bridge、worktree 和 executor fixture
    When 测试拆入行为族模块
    Then 所有断言和测试通过
  Scenario: deadline 与 fan-in 边界不丢失
    Given 既有 aggregate deadline 和 salvage 测试
    When 完成拆分
    Then 测试名、计数和边界覆盖保持完整
  Scenario: 并发测试仍按既有隔离运行
    Given nextest 进程隔离与 run-tests 两阶段
    When 执行全量回归
    Then 无新增竞态失败
```

## 5. 验收与测试策略

| 场景 | 条件 | 入口 | 层级 | 补充 | E2E |
|---|---|---|---|---|---|
| supervisor | targeted 通过 | `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` | 集成 | slot/fan-in | 否 |
| 完整性 | 名多重集/计数一致 | `cargo nextest list --workspace` | 清单 | 分组路径允许变 | 否 |
| 竞态 | 全量两阶段通过 | `./scripts/run-tests.sh` | 回归 | deadline slow path | 否 |

## 6. 需求—测试追踪矩阵

| ID | 需求 | Scenario | 验收 | 单元 | 集成 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | supervisor 测试按行为族拆分 | supervisor 回归 | targeted/full | 原测试 | wave/loop | 否 | E1-E3 |
| R2 | deadline/fan-in 覆盖保留 | 边界完整 | list/full | 原边界测试 | dispatcher 链路 | 否 | E4,E5 |

## 7. 严格串行开发单元

### Unit 1：wave_supervisor 测试目录模块化

1. 目标：根 ≤300 行；新增 `fixtures.rs`（若依赖分析证明需要）、`slot_binding.rs`、`dispatch.rs`、`timeouts.rs`、`coordination.rs`、`salvage_merge.rs`、`supervisor.rs`、`misc.rs` 等实际行为族文件；所有子文件 <5,000 行，`misc.rs` ≤600 行。
2. 对应 R1/R2、BDD、D1–D3、E1–E3。
3. 结果：最大子文件约 1,100 行，全部 <5,000；测试名多重集/计数不变。
4. 基线：当前文件包含 Spy/Recording fixture、多个按 U/S 编号命名的 helper 和测试族，均属本文件所有权；搬移前先生成函数/fixture/测试 ID manifest。
5. 输入/输出/副作用：fixture 与断言不变。
6. 修改边界：仅该文件和子目录；不动 dispatcher/bridge 生产文件。
7. 可依赖：现有 wave API；无其他计划依赖。
8. 禁止：改并发/timeout 常量、删测试、mock 掉真实 bridge。
9. 验收：list、targeted、full。
10. Red：编译、测试失败、计数不一致即红。
11. 单测：不新增；只做物理搬移。fixture 随首个使用族整体搬移，若跨 3 个以上行为族共享则抽 `fixtures.rs`，仍不得复制。
12. 顺序：快照 → fixture 依赖图 → slot_binding → dispatch → timeouts → coordination → salvage_merge → supervisor → misc → root 声明/re-export → targeted/full。
13. 最小实现：路径式 mod、导入、最小 `pub(super)`。
14. 集成：wave_supervisor、wave/loop_runner、workspace。
15. 风险：fixture 重复定义和慢测试误跑；用 build、ID、run-tests 检测。
16. 回归：ralph-cli、workspace、lint、fmt。
17. 文件：根+行为族子模块（E1–E3）。
18. 完成：既有测试全集和边界覆盖通过，行数合规。
19. 停止：需修改生产逻辑或共享 fixture 时停止。
20. 缓解：保持原 `super::super::*` 语义，逐族编译。

## 8. Unit 串行依赖图

唯一 Unit 内按 fixture/slot → dispatch → timeout → coordination/salvage → supervisor → 清单执行，后组可能依赖前组 fixture，不可交错。

## 9. 执行命令清单

`cargo nextest list --workspace`；`cargo build --workspace`；`cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`；`cargo nextest run -p ralph-cli --bin ralph -- wave`；`just fmt-check`；`just lint`；`./scripts/run-tests.sh`。

## 10. 最终质量门禁

所有 wave supervisor/全量测试、build、fmt、clippy 通过；测试名多重集/计数不变；无断言削弱；并发边界覆盖保留；决策 ≥0.85；无 BLOCKED。

## 11. 最终计划自检

| 检查项 | 结果 |
|---|---|
| 独立行为切片 | 是，wave supervisor 回归 |
| 修改所有权无冲突 | 是；本计划只拥有 `wave_supervisor.rs` 及其子目录，不修改生产代码和共享 mdc |
| 当前是否可执行 | 是；基线、fmt、lint、warning 和全量回归均通过 |
| 独立性置信度 | 0.94 |
