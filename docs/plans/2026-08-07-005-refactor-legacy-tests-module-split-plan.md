# 将 loop_runner legacy 测试拆分为模块

## 0. 计划状态

- `READY`；共同基线 `87c88317c94ce5f15d3e17b74755ade3f3b56a47`；全量基线已验证通过。
- 调查范围：`crates/ralph-cli/src/loop_runner/tests/legacy.rs`（当前 5,521 行）、tests/mod 声明、common/fake_path 依赖和 nextest 规则。
- 已执行：行数、测试函数/模块扫描、Git 状态/历史扫描；`./scripts/run-tests.sh` 通过（Phase 1 7576/7576、Phase 2 23/23、doctest 19/19，4 ignored，退出码 0）。
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

把 legacy 测试从单一 5,521 行文件拆成按行为族组织的 `legacy/` 子模块，保持测试名语义、断言、fixture、进程环境隔离和 loop_runner 测试入口不变。该切片的可观察能力是既有 loop-runner legacy 回归继续覆盖真实 runner 行为；非目标是修复或重写测试及生产逻辑。

## 2. 代码库现状与证据

`tests/mod.rs` 以 `mod legacy;` 接入，legacy 文件使用 `super::super::*`、`common::*`、`fake_path::*`，包含 termination、PTY、recovery、event processing、diagnosis、timeout 等大量扁平测试。纯测试文件拆分会改变模块路径中段，因此测试名多重集和计数是契约，不能要求路径逐字节一致。

| ID | 来源 | 观察 | 影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `wc -l`/当前源码 | 5,521 行，纯测试文件 | 单独业务切片 | 高 |
| E2 | `rg '^fn test_|^async fn test_'` | 测试按前缀可分组 | 机械拆分 | 高 |
| E3 | `tests/mod.rs`、common/fake_path | 依赖父模块 glob 与共享 fixture | 保留声明链/导入 | 高 |
| E4 | AGENTS | nextest 进程隔离是硬门禁 | 命令 | 高 |

## 3. 决策记录与置信度

| ID | 问题 | 选择 | 依据 | 置信度 |
|---|---|---|---|---|
| D1 | 测试模块结构 | `legacy.rs + legacy/`，按测试前缀分组 | E2/E3 | 0.94 |
| D2 | ID 不变量 | 测试名多重集和计数不变，允许模块中段变化 | 扁平测试拆分物理必需 | 0.96 |
| D3 | 测试内容 | 整函数/辅助项搬移 | 不改断言 | 0.98 |

## 4. BDD 行为规格

```gherkin
Feature: legacy loop-runner 回归测试模块化
  Scenario: 既有 legacy 行为回归保持绿
    Given 原有 legacy 测试和真实 runner fixture
    When 测试按前缀拆入子模块
    Then 所有测试通过且断言未削弱
  Scenario: 测试全集保持完整
    Given 拆分前测试名多重集和计数
    When 完成拆分
    Then 名称多重集和计数一致
  Scenario: 环境污染仍被隔离
    Given 外层存在 RALPH_CURRENT_HAT 等环境变量
    When 运行 legacy 测试
    Then fixture 仍按既有 scrub/nextest 语义运行
```

## 5. 验收与测试策略

| 场景 | 条件 | 入口 | 层级 | 补充 | E2E |
|---|---|---|---|---|---|
| legacy 回归 | 通过 | `cargo nextest run -p ralph-cli --bin ralph -- legacy` | 集成 | timeout/recovery | 否 |
| 清单 | 名多重集/计数相同 | `cargo nextest list --workspace` | 清单 | 路径变化允许 | 否 |
| 全量 | 通过 | `./scripts/run-tests.sh` | 回归 | 两阶段 | 否 |

## 6. 需求—测试追踪矩阵

| ID | 需求 | Scenario | 验收 | 单元 | 集成 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | legacy 测试按行为族拆分 | 回归绿 | targeted/full | 原测试 | loop_runner | 否 | E1-E3 |
| R2 | 测试全集不丢失 | 完整性 | list 多重集 | 计数脚本 | nextest | 否 | E2 |

## 7. 严格串行开发单元

### Unit 1：legacy 测试目录模块化

1. 目标：把 `legacy.rs` 转为根声明，新增按前缀的 `termination.rs`、`pty.rs`、`recovery.rs`、`event_processing.rs`、`diagnosis.rs`、`misc.rs` 等实际分组文件。
2. 对应 R1/R2、全部 Scenario、D1–D3、E1–E3。
3. 结果：根 ≤100 行；子文件均 <5,000 行；测试名多重集/计数不变。
4. 基线：现有文件全部是纯测试与 test helper，不引入生产入口。
5. 输入/输出/错误/副作用：测试行为、fixture 和环境语义保持不变。
6. 修改边界：仅 `tests/legacy.rs` 和 `tests/legacy/*.rs`；不得改 common/fake_path。
7. 可依赖：现有 `tests/mod.rs` 声明链；无其他计划依赖。
8. 禁止：删除测试、改断言、`.only`、mock 真实 runner、改共享 helper。
9. 验收：快照、legacy targeted、workspace full。
10. Red：编译/测试名缺失/测试失败即红。
11. 单测：不新增，函数与 helper 整体搬移。
12. 顺序：快照 → 建目录/声明 → 逐前缀搬移并 targeted → Refactor imports → full → close。
13. 最小实现：仅路径式 mod、导入和必要 `pub(super)`。
14. 集成：legacy、loop_runner 全包、workspace。
15. 风险：`super::super::*` 层级变化、重复 helper、时序 flake；以编译、清单、两阶段脚本检测。
16. 回归：ralph-cli、workspace、lint、fmt。
17. 文件：legacy 根及其测试子模块（E1–E3）。
18. 完成：多重集/计数一致、全量绿、无断言变化。
19. 停止：发现需修改共享 fixture 或生产语义时停止。
20. 缓解：每组搬移后立即 build + targeted，保持原 helper 归属。

## 8. Unit 串行依赖图

单一 Unit 内按声明、共享导入、测试族、清单核对顺序执行；清单核对必须最后，因为目录层级只有在全部搬移后才稳定。

## 9. 执行命令清单

`cargo nextest list --workspace`；`cargo build --workspace`；`cargo nextest run -p ralph-cli --bin ralph -- legacy`；`cargo nextest run -p ralph-cli --bin ralph -- loop_runner`；`just fmt-check`；`just lint`；`./scripts/run-tests.sh`。任何失败停止。

## 10. 最终质量门禁

所有 legacy/loop_runner/workspace 测试、build、fmt、clippy 通过；测试名多重集与计数一致；无删除/跳过/弱化断言；子文件合规；决策 ≥0.85；无 BLOCKED。

## 11. 最终计划自检

| 检查项 | 结果 |
|---|---|
| 业务切片而非技术层拆分 | 是，legacy 回归能力 |
| 文件所有权无冲突 | 是 |
| 独立可验证 | 未满足：计划集级 BLOCKED |
| 独立性置信度 | 0.96 |
