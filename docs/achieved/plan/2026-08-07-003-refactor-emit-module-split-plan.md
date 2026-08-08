# 将 emit.rs 拆分为可维护模块

## 0. 计划状态

- 状态：`BLOCKED（实施前置基线失败）`；当前基线为 branch `pittcat-dev` @ `1f765a2d457fa07052a7125fe6582f39ea121c0a`（`fix(loop-runner): capture empty isolated channel context`，2026-08-08 17:00 +0800）。原共同基线 `87c88317c94ce5f15d3e17b74755ade3f3b56a47` 已过期。
- 调查范围：`commands/emit.rs` 生产入口、`schema_view`、6 个测试模块、`policy_check` 调用关系；当前文件为 6,652 行。
- 已执行验证（当前 HEAD）：`cargo build --workspace` 通过；`just fmt-check` 通过；`just lint` 通过，`cargo clippy --all-targets --all-features -- -D warnings` 无 warning；`cargo nextest --version` 为 0.9.140；`cargo nextest list --workspace` 生成 8,060 行清单；`cargo nextest run -p ralph-cli --bin ralph -- emit` 通过（137/137）。
- 全量基线结果：`./scripts/run-tests.sh` 未通过。Phase 1 运行 1,557/7,594，1 个既有失败：`loop_runner::tests::wave_supervisor::dispatch::test_dispatcher_spawns_only_approved_slot`（`dispatch.rs:1042`，实际 spawn 0、预期 1）；Phase 2 为 23/23；doctest 为 19/23（4 ignored）。该失败不属于 emit 拆分范围，但按本计划的零回归硬门禁，在修复或明确解除阻塞前不得开始 Unit 1。
- 阻塞：上述 wave supervisor 基线失败；共享文档仍由合并计划 002 独占处理。阻塞解除后必须重新执行全量基线，不得复用本记录。

### 0.1 0 回归硬门禁

本计划在计划集级阻塞解除前禁止执行。解除前必须在当前 HEAD 运行并保存结果：

- `./scripts/run-tests.sh`（当前失败，见 §0）
- `cargo build --workspace`（当前通过）
- `just fmt-check`（当前通过）
- `just lint`（当前通过，无 warning）
- `cargo nextest list --workspace`（当前清单 8,060 行）

每个 Unit 前后必须证明 targeted 命令实际命中了目标测试；ralph-cli 统一使用 `cargo nextest run -p ralph-cli --bin ralph -- <filter>`，不能凭模块名假设命中。纯重构验收还必须逐项核对生产 item、测试函数体、属性、字符串字面量、公开签名、方法数量和方法体 hash；不能只比较编译结果、测试名或测试数量。禁止空 stub、把全部测试塞进 `misc.rs`、遗漏生产区、拆解巨型函数、修改业务逻辑或通过削弱断言获得 Green。共享文档已由合并计划 002 独占处理；本计划不修改该文件。当前 HEAD 全量基线通过后方可执行。

## 1. 功能目标

保持 `ralph emit` 的参数、文件输入、policy gate、recovery envelope、JSON/文本输出和 `commands::emit::schema_view` 路径不变，将当前 6,652 行的 `crates/ralph-cli/src/commands/emit.rs` 拆成根模块、生产实现、schema_view 和测试模块。非目标是改变事件协议、policy 规则、输出字段或权限。

## 2. 代码库现状与证据

生产入口约 1–792 与 845–2,204；测试包含 `emit_reject_summary_tests`、大型 `tests`、4 个 JSON/recorded 测试族，共 6 个 `#[cfg(test)]` 模块；尾部为 `pub mod schema_view`。`emit.rs` 直接 import `policy_check`，但本计划不修改 policy_check。

| ID | 来源 | 观察 | 影响 | 可靠性 |
|---|---|---|---|---|
| E1 | 当前源码/`wc -l` | 6,652 行，生产、测试、schema_view 混合 | 目标确认 | 高 |
| E2 | 顶层 `rg` | 生产入口和 6 个 `#[cfg(test)]` 模块边界可定位 | 搬移表 | 高 |
| E3 | `rg 'commands::emit::schema_view'`/import | schema_view 是路径契约 | 必须路径式声明+re-export | 高 |
| E4 | AGENTS/justfile | 只能使用 nextest/run-tests | 门禁 | 高 |
| E5 | 当前 HEAD 与门禁命令 | HEAD=`1f765a2d`；build、fmt、lint 通过；lint 为 `-D warnings` 且无 warning；emit targeted 137/137 通过 | 更新实施前基线 | 高 |
| E6 | `./scripts/run-tests.sh` 当前运行 | Phase 1 的 wave supervisor 测试 `test_dispatcher_spawns_only_approved_slot` 失败；Phase 2 23/23、doctest 19/23（4 ignored） | 实施前阻塞；不得把全量基线写成 Green | 高 |

## 3. 决策记录与置信度

| ID | 问题 | 选择 | 依据 | 置信度 |
|---|---|---|---|---|
| D1 | 结构 | `emit.rs + emit/` | 仓库先例 | 0.96 |
| D2 | schema_view | 原路径式子模块 | E3，保持公开路径 | 0.97 |
| D3 | 测试 | 每个 mod 整块搬移 | 零行为变更 | 0.97 |

## 4. BDD 行为规格

```gherkin
Feature: emit CLI 模块拆分
  Scenario: emit 成功事件不变
    Given 同一 payload、配置和输出模式
    When 执行拆分后的 emit 命令
    Then 事件、输出和文件副作用不变
  Scenario: emit 拒绝路径不变
    Given 非法 topic/payload 或 policy rejection
    When 执行 emit
    Then 错误码、summary 和 recovery envelope 不变
  Scenario: schema_view 路径兼容
    Given 外部调用 `commands::emit::schema_view`
    When 编译并运行
    Then 路径和结果保持兼容
```

## 5. 验收与测试策略

| 场景 | 条件 | 入口 | 层级 | 补充 | E2E |
|---|---|---|---|---|---|
| 成功/拒绝 | emit 测试通过 | `cargo nextest run -p ralph-cli --bin ralph -- emit` | 集成 | JSON families | 否 |
| 清单 | ID 不变 | `cargo nextest list --workspace` | 清单 | diff | 否 |
| 编译 | workspace build | `cargo build --workspace` | 构建 | path resolution | 否 |
| 回归 | 全量脚本 | `./scripts/run-tests.sh` | 回归 | env scrub | 否 |

## 6. 需求—测试追踪矩阵

| ID | 需求 | Scenario | 验收 | 单元 | 集成 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | emit 生产模块化 | 成功/拒绝 | emit/full | 原 tests | CLI | 否 | E1,E2 |
| R2 | schema_view 路径不变 | 路径兼容 | build/emit | 既有 schema tests | CLI | 否 | E3 |

## 7. 严格串行开发单元

### Unit 1：emit 生产、schema_view 与测试区拆分

1. 目标：根 ≤850 行；新增 `command_impl.rs`、`test_support.rs`、`schema_view.rs` 与 5 个测试文件。
2. 对应 R1/R2、BDD、D1–D3、E1–E3。
3. 结果：所有新文件 <5,000 行，公开路径、输出、事件不变。
4. 基线：生产 1–792/845–2,204；大型 tests 约 3,713 行；schema_view 在尾部。
5. 输入/输出/错误/副作用：保持完全相同。
6. 修改边界：仅 `commands/emit.rs` 与 `commands/emit/*.rs`；不动 policy_check、schema、fixture。
7. 可依赖：现有 emit/policy API；不得依赖其他计划。
8. 禁止：重写 gate、输出格式、事件 payload、测试断言。
9. 验收：nextest list、emit targeted、workspace build、全量。
10. Red：编译/emit/ID 任一失败即红；不得 mock 掉真实 emit。
11. 单测：原测试 mod 整块搬移，不新增。
12. 顺序：快照 → 测试族 → command_impl → schema_view → re-export Refactor → Integration → Regression → Close。
13. 最小实现：项级搬移与最小可见性。
14. 集成：emit targeted、`integration_*`、workspace。
15. 风险：schema_view 路径与 cfg(test) helper；用 build、ID、JSON 测试检测。
16. 回归：ralph-cli、workspace、lint、fmt。
17. 文件：根+`command_impl`/`test_support`/`schema_view`+5 测试文件（E1–E3）。
18. 完成：全绿、路径仍可用、行数/diff 合规。
19. 停止：需改变 policy 或事件协议时重新切分。
20. 缓解：保留 `pub mod schema_view;`，按编译错误最小放宽可见性。

## 8. Unit 串行依赖图

唯一 Unit 内按“测试 → 生产 → schema_view → re-export → 全量”执行；schema_view 需在根声明稳定后验证，不能提前完成。

## 9. 执行命令清单

`cargo nextest list --workspace`；`cargo build --workspace`；`cargo nextest run -p ralph-cli --bin ralph -- emit`；`just fmt-check`；`just lint`；`./scripts/run-tests.sh`。当前 emit targeted、build、fmt、lint 已通过；完整基线因 E6 失败，修复或明确解除阻塞后必须从头重新执行全部命令。失败均停止。

## 10. 最终质量门禁

emit 全部既有测试、workspace、build、fmt、clippy 通过；JSON/错误断言未削弱；路径兼容；根/子文件合规；决策 ≥0.85；无 BLOCKED。

## 11. 最终计划自检

| 检查项 | 结果 |
|---|---|
| 独立业务切片 | 是 |
| 修改所有权无重叠 | 是：共享 mdc 由合并计划 002 独占 |
| 真实代码/测试/命令证据 | 是 |
| 当前基线 | 未满足：E6 的既有 wave supervisor 测试失败 |
| 独立性置信度 | 0.95 |
