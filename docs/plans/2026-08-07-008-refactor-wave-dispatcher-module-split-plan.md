# 将 wave dispatcher 拆分为可维护模块

## 0. 计划状态

- `READY`；共同基线 `87c88317c94ce5f15d3e17b74755ade3f3b56a47`；全量基线已验证通过。
- 调查：`crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` 当前 13,543 行、dispatch/worker/fan-in/coordination/helpers 和大型 tests；`./scripts/run-tests.sh` 通过（Phase 1 7576/7576、Phase 2 23/23、doctest 19/19，4 ignored，退出码 0）。
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

保持 wave 派发、slot 并发、worker timeout、supervisor fan-in、completion/failure coordination、salvage merge、aggregate deadline 与诊断事件完全不变，将 dispatcher 拆成生产子模块和测试目录。调用方是 wave runner、supervisor bridge、CLI loop。非目标是调度算法、deadline、事件 payload 或 supervisor 生产逻辑改造。

## 2. 代码库现状与证据

当前文件 13,543 行：顶部为 WaveOutputs/limits/outcome、worker/dispatch context；约 489 起 handle/execute；约 2,466 起 fan-in；约 4,080 起协调/helper；约 4,921 起 `dispatch_wave_inner_with_release`；tests 从约 6,758 起且超过 5,000。已有 `wave/` 兄弟模块模式。

| ID | 来源 | 事实 | 影响 | 可靠性 |
|---|---|---|---|---|
| E1 | 当前源码/`wc -l` | 13,543 行，生产+tests | 目标确认 | 高 |
| E2 | 顶层符号扫描 | slot/execute/coordination/helpers/inner 可分组 | 生产边界 | 高 |
| E3 | tests 区扫描 | tests 约 5,694 行，按 slot/timeout/coord/salvage/supervisor 前缀可分组 | tests 目录必需 | 高 |
| E4 | wave 目录 | 已有兄弟模块先例 | 结构决策 | 高 |
| E5 | 最近 dispatcher history | 有 aggregate deadline/fan-in 回归提交 | 必须保留边界测试 | 高 |

## 3. 决策记录与置信度

| ID | 决策 | 选择 | 依据 | 置信度 |
|---|---|---|---|---|
| D1 | 生产结构 | slot/execute/inner/coordination/helpers/dispatch_inner | E2/E4 | 0.93 |
| D2 | tests 结构 | tests/mod.rs + 6 个行为族 | E3 | 0.92 |
| D3 | 单函数 | `dispatch_wave_inner_with_release` 整体搬移 | 零行为变更 | 0.99 |
| D4 | 跨模块私有 helper | 最小 `pub(super)` | 编译错误驱动 | 0.91 |

## 4. BDD 行为规格

```gherkin
Feature: wave dispatcher 模块拆分
  Scenario: wave 派发与 worker 结果保持不变
    Given 相同 wave、slot、backend 和并发限制
    When 执行拆分后的 dispatcher
    Then worker 启动、结果和终态事件不变
  Scenario: deadline/fan-in/salvage 保持不变
    Given timeout、部分失败、重试和 salvage 输入
    When 执行 dispatcher
    Then aggregate deadline、fan-in 和诊断 payload 不变
  Scenario: 大型测试全集保持完整
    Given 原 tests 名多重集
    When tests 转为目录模块
    Then 名称多重集和计数一致
```

## 5. 验收与测试策略

| 场景 | 条件 | 入口 | 层级 | 补充 | E2E |
|---|---|---|---|---|---|
| dispatch | targeted 通过 | `cargo nextest run -p ralph-cli --bin ralph -- dispatcher` | 集成 | wave | 否 |
| deadline/fan-in | 既有边界通过 | `cargo nextest run -p ralph-cli --bin ralph -- wave` | 集成 | slow path | 否 |
| 全量 | run-tests 通过 | `./scripts/run-tests.sh` | 回归 | 并发隔离 | 否 |

## 6. 需求—测试追踪矩阵

| ID | 需求 | Scenario | 验收 | 单元 | 集成 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | dispatcher 生产模块化 | 派发行为 | targeted/full | 原 tests | wave/loop | 否 | E1,E2 |
| R2 | fan-in/deadline/salvage 不变 | 边界 | wave/full | 原边界 tests | supervisor | 否 | E3,E5 |

## 7. 严格串行开发单元

### Unit 1：dispatcher 生产与测试目录拆分

1. 目标：根 ≤300 行；新增 6 个生产文件和 tests/mod + 6 个测试族文件。
2. 对应 R1/R2、BDD、D1–D4、E1–E5。
3. 结果：最大生产约 2,010 行、测试 misc 约 1,100 行；全部 <5,000。
4. 基线：生产区间与锚点见 E2；tests 从约 6,758 起且超过 5,000。
5. 输入/输出/错误/副作用：调度和事件完全不变。
6. 修改边界：仅 dispatcher 根及 `dispatcher/`；不动 wave_supervisor 测试文件、channel/worker 生产文件。
7. 可依赖：现有 wave API；无其他计划依赖。
8. 禁止：改 timeout、fan-in、failure classification、payload 或测试断言。
9. 验收：list、targeted dispatch/wave、full。
10. Red：编译、测试、清单、deadline 回归失败即红。
11. 单测：原 tests 先整体至 tests/mod，再按前缀拆分；不新增。
12. 顺序：快照 → tests 目录/分组 → slot/execute → inner → coordination → helpers → dispatch_inner → re-export → full。
13. 最小实现：项级搬移、最小可见性。
14. 集成：dispatcher/wave、workspace、E2E mock（最终）。
15. 风险：私有 helper 跨区、并发边界；编译/全量/边界测试检测。
16. 回归：ralph-cli、workspace、lint、fmt、E2E mock。
17. 文件：根、6 个生产子模块、tests 目录（E1–E4）。
18. 完成：行为/边界/清单/行数门禁全绿。
19. 停止：需改调度语义或跨计划测试文件时停止。
20. 缓解：按区域逐批 build+targeted，`dispatch_wave_inner_with_release` 不拆体。

## 8. Unit 串行依赖图

单一 Unit 依次 tests → slot/execute → inner → coordination/helpers → dispatch_inner；后者依赖前面 helper 的稳定可见性，不得交错。

## 9. 执行命令清单

`cargo nextest list --workspace`；`cargo build --workspace`；`cargo nextest run -p ralph-cli --bin ralph -- dispatcher`；`cargo nextest run -p ralph-cli --bin ralph -- wave`；`just fmt-check`；`just lint`；`./scripts/run-tests.sh`；最终 `cargo run -p ralph-e2e -- --mock`。

## 10. 最终质量门禁

dispatch/wave/full/E2E mock、build、fmt、clippy 通过；deadline/fan-in/salvage 断言未削弱；测试全集保留；所有文件 <5,000；决策 ≥0.85；无 BLOCKED。

## 11. 最终计划自检

| 检查项 | 结果 |
|---|---|
| 独立业务能力 | 是，wave dispatch |
| 文件所有权无重叠 | 代码文件是；计划集共享 mdc 未解决 |
| 独立性置信度 | 未达 0.90，保持 BLOCKED |
