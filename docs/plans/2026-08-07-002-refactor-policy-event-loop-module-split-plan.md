# policy、event policy 与 event loop 模块拆分

## 0. 计划状态

- 状态：`READY`；共同基线：`87c88317c94ce5f15d3e17b74755ade3f3b56a47`。
- 合并原因：三个生产文件共同影响 `.cursor/rules/state-management.mdc`，必须由同一计划拥有文档更新，才能满足独立性与无所有权冲突。
- 调查范围：`policy_check.rs` 5,890 行、`event_policy.rs` 8,406 行、`event_loop/mod.rs` 17,682 行、共享活动规则文档、跨 crate 调用方和测试入口。
- 已执行验证：当前源码行数、顶层 item/`cfg(test)`/impl 方法扫描、调用方扫描、活动文档引用扫描、Git 状态/历史扫描。
- 已执行验证：当前 HEAD 的 `./scripts/run-tests.sh` 通过：Phase 1 为 7576/7576，Phase 2 为 23/23，doctest 为 19/19（4 ignored），退出码 0。
- 未执行验证：`cargo build --workspace`、`just fmt-check`、`just lint` 和 nextest 清单基线；这些仍是实施前的计划内门禁。
- 阻塞项：无；全量基线已满足进入 Unit 1 的条件。

### 0.1 0 回归硬门禁

实施前必须保存当前 HEAD 的全量证据（本次已保存结果；每个实施分支仍需重新执行）：

```bash
./scripts/run-tests.sh
cargo build --workspace
just fmt-check
just lint
cargo nextest list --workspace
```

每个 Unit 前后必须证明 targeted 命中真实测试；ralph-cli 使用 `cargo nextest run -p ralph-cli --bin ralph -- <filter>`。本计划只允许项级搬移、模块声明、精确导入、明确列出的最小可见性调整和 mdc 行号/路径更新。每个 Unit 必须逐项核对生产 item、测试函数体、属性、字符串字面量、公开签名、方法数量和方法体 hash；禁止空 stub、遗漏生产区、拆解巨型函数、修改业务逻辑、删除/削弱断言或通过更新 Snapshot 获得 Green。

## 1. 功能目标

在同一纵向切片中完成三个相互关联的运行时模块拆分，并保持外部行为零变化：

- `policy_check.rs`：CLI policy-check 的配置解析、校验、报告和 JSON 输出不变；
- `event_policy.rs`：事件 topic、deny rule、handoff、completion、dedup、projection 语义不变；
- `event_loop/mod.rs`：EventLoop 状态机、prompt、恢复、wave emit、step close、终态和公开 API 不变。

调用方包括 ralph-cli、ralph-core、ralph-tui、ralph-api、ralph-e2e、ralph-bench、BDD/scenario/smoke 及活动规则文档。最大 `process_parse_result`（约 3,777 行）和其他巨型函数只整体搬移，绝不拆体。非目标是修改事件协议、schema、错误语义、配置字段、业务规则或测试断言。

## 2. 代码库现状与证据

| Evidence ID | 来源 | 观察结果 | 对实施的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | 当前源码与 `wc -l` | 三文件分别为 5,890、8,406、17,682 行 | 确定合并切片 | 高 |
| E2 | 顶层 item/`cfg(test)` 扫描 | policy_check 有 5 个测试族；event_policy 有大型 tests；event_loop 有头部/尾部自由项、6 个内联测试 mod 和约 200 个 impl 方法 | 形成 U1/U2/U3 边界 | 高 |
| E3 | 跨 crate `rg` 调用扫描 | policy API、event policy API、EventLoop 公开项均有消费者 | 必须保留 root re-export，最终 workspace build | 高 |
| E4 | `.cursor/rules/state-management.mdc` | 同时引用 `policy_check.rs`、`event_policy.rs`、`event_loop/mod.rs` 行号 | 三文件不能分属独立计划；本计划独占该文档更新 | 高 |
| E5 | `event_loop/tests/`、`loop_runner/tests/mod.rs` | 仓库已有兄弟目录和路径式测试模块先例 | 选择 `foo.rs + foo/` | 高 |
| E6 | AGENTS/justfile/scripts | nextest、两阶段 run-tests、fmt/clippy 为硬门禁 | 固定验证入口 | 高 |
| E7 | 历史对抗性审查产物 | 曾出现生产区未拆、空测试 stub、EventLoop 整块集中、stale 行号引用 | 增加 item manifest、测试体 hash、文档闭环门禁 | 高 |

受影响文件只包括三根文件、各自新子模块和 `.cursor/rules/state-management.mdc`。不修改其他 6 份计划拥有的生产/测试文件、`crates/ralph-core/data/*.md`、presets、schema、`.ralph` 运行时状态。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选 | 最终选择 | 证据 | 置信度 |
|---|---|---|---|---|---|
| D1 | 目录结构 | `foo.rs + foo/`；`foo/mod.rs` | 前者 | E5，减少根文件重命名 | 0.96 |
| D2 | 三文件边界 | 三份独立计划；一份合并计划 | 合并为一个计划 | E4，消除共享文档冲突 | 0.98 |
| D3 | 测试拆分 | 整块搬移；重写 | 整块搬移，超过 5,000 行才按既有前缀分组 | E2/E7 | 0.96 |
| D4 | 巨型函数 | 整体搬移；拆 helper | 整体搬移 | 零逻辑变更约束 | 0.99 |
| D5 | EventLoop impl | 10 个连续方法区域 | 按方法起始位置机械分区 | E2/E7，避免整块集中 | 0.93 |
| D6 | 文档所有权 | 各计划自行改；合并切片独占 | 本计划独占 mdc，三文件同一 Unit 体系串行 | E4 | 0.98 |

## 4. BDD 行为规格

```gherkin
Feature: policy、event policy 与 event loop 的纯结构拆分
  Scenario: policy-check 外部结果不变
    Given 相同配置、topic、payload、状态和 CLI 参数
    When 执行拆分后的 policy-check
    Then decision、report、错误和 JSON 输出完全一致
  Scenario: event policy 规则不变
    Given 相同 hat、topic、payload 和 policy state
    When 执行 policy validation
    Then finding、reason、decision、dedup 和 projection 完全一致
  Scenario: EventLoop 主链路不变
    Given 相同事件流、配置、恢复状态和 backend
    When 执行拆分后的 EventLoop
    Then 状态、事件、prompt、终态和错误完全一致
  Scenario: 结构搬移完整
    Given 三个 Unit 的搬移前 item manifest 和 hash
    When 完成拆分
    Then item、函数体、属性、字面量和测试函数体逐项一致
  Scenario: 活动文档引用闭环
    Given state-management 中的三类源码引用
    When 三个文件完成拆分
    Then 每条引用都指向实际存在的符号/新路径，不保留 stale 行号
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
|---|---|---|---|---|---|
| policy 行为 | policy_check 与 emit consumer 通过 | `cargo nextest run -p ralph-cli --bin ralph -- policy_check` | 集成 | `integration_emit_policy` | 否 |
| event policy 行为 | core policy/event_loop 测试通过 | `cargo nextest run -p ralph-core -- event_policy` | 集成 | BDD/scenario/smoke | 否 |
| EventLoop 主链 | event_loop 与 workspace 通过 | `cargo nextest run -p ralph-core -- event_loop` | 集成 | `event_loop_ralph` | 否 |
| 结构零差异 | manifest/hash/方法计数全等 | 每个 Unit 的结构核对 | 结构 | diff 审查 | 否 |
| 最终真实链路 | mock E2E 通过 | `cargo run -p ralph-e2e -- --mock` | E2E | 最终一次 | 是 |
| 全量 | 两阶段脚本通过 | `./scripts/run-tests.sh` | 回归 | env scrub/时序隔离 | 否 |

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | policy_check 生产与测试拆分 | policy 行为/结构完整 | U1 targeted/full | 原 policy tests | emit policy | 否 | E1-E3 |
| R2 | event_policy 生产与测试拆分 | event policy/结构完整 | U2 targeted/full | 原 policy tests | EventLoop/CLI | 否 | E1-E3 |
| R3 | EventLoop 头尾、测试和 impl 拆分 | EventLoop/结构完整 | U3 targeted/full | 原 event_loop tests | scenarios/smoke | mock | E1-E5 |
| R4 | 活动文档引用闭环 | 文档闭环 | U3 文档核对 | 符号/路径扫描 | drift review | 否 | E4 |
| R5 | 0 运行时回归 | 全部 Scenario | full/build/fmt/lint/E2E | 全部既有测试 | workspace | 是 | E6/E7 |

## 7. 严格串行开发单元

### Unit 1：policy_check.rs 拆分

1. 目标：根 ≤450 行；新增 `gates.rs`、`unified.rs`、5 个测试文件。
2. 对应 R1、policy Scenario、D1/D3/D4、E1-E4。
3. 边界：生产类型/解析器、gates、统一运行器及 5 个测试 mod 按当前 item manifest 搬移；`run_policy_check_unified` 的 cfg(test) 属性随项搬移。
4. 禁止：修改校验逻辑、错误字符串、JSON、schema、emit 或 event_policy。
5. Red→Green→Refactor：先保存 item/test/hash manifest；逐测试族/生产族搬移并 build；targeted 命中核对；re-export；全量回归；Close。
6. 验收：policy_check targeted、`integration_emit_policy`、workspace build、fmt、lint、nextest 清单。
7. 风险：跨模块私有 helper 和测试 namespace；以编译、manifest、测试体 hash 检测。
8. 文件所有权：仅 policy_check 根/子模块；mdc 暂不改，留给 U3 统一闭环。

### Unit 2：event_policy.rs 拆分

1. 目标：根 ≤550 行；新增类型/runtime/validation/projection 子模块和 `tests/` 目录；测试文件不得通过空 stub或单一 misc 重新聚集。
2. 对应 R2、event policy Scenario、D1/D3/D4、E1-E5。
3. 边界：类型/运行时状态/topic validation/projection 按 item manifest 搬移；超过 5,000 行的原 tests 必须按已确认前缀分组。
4. 禁止：改变 reason、topic、deny rule、序列化、公开 API 或 event_loop。
5. Red→Green→Refactor：manifest/hash → tests 目录实际搬移 → 生产 item 搬移 → 最小可见性 → core targeted → CLI consumer → full。
6. 验收：core policy targeted、event_loop targeted、CLI policy consumer、BDD/scenario/smoke、全量。
7. 风险：公开类型 re-export、私有 validator helper、测试遗漏；以跨 crate build、hash 和全量检测。
8. 文件所有权：仅 event_policy 根/子模块；mdc 暂不改，留给 U3。

### Unit 3：event_loop/mod.rs 与共享文档闭环

1. 目标：根 ≤1,500 行；先拆头部/尾部自由项和 6 个内联测试 mod，再按 10 个连续方法区域拆 `impl EventLoop`。
2. 对应 R3/R4/R5、EventLoop/结构/文档 Scenario、D1/D4-D6、E1-E7。
3. 边界：新增 `prompt_types.rs`、`flow_wiring.rs`、`resume_types.rs`、`stall_recovery.rs`、6 个测试文件和 10 个 impl 区域文件；`process_parse_result` 必须整体搬入 `parse_result.rs`，不得拆解。
4. 禁止：整块搬成单一 `impl_event_loop.rs`、修改 prompt/事件/状态逻辑、遗漏方法或用大量无理由 `pub(super)` 掩盖错误边界。
5. Red→Green→Refactor：Unit 2 绿后保存 EventLoop item/method/function-body hash；U1 头尾测试；U2 头部 wiring；U3 每个 impl 区域按 R1→R10 逐区 build+targeted+方法计数；最后更新 mdc 三条源码引用并用 `rg`/`sed` 逐条确认。
6. 验收：core event_loop、`event_loop_ralph`、BDD/scenario/smoke、workspace、`cargo doc --no-deps`、mock E2E、全量。
7. 风险：兄弟 impl 模块的私有方法访问、方法重复/遗漏、文档 stale reference；以可见性白名单、方法 hash、路径扫描和 full gate 检测。
8. 文件所有权：本计划独占 event_loop 三个 Unit 及 `.cursor/rules/state-management.mdc`，不与其他 6 份计划冲突。

## 8. Unit 串行依赖图

`Unit 1 policy_check → Unit 2 event_policy → Unit 3 event_loop + mdc`。

U2 不能提前，因为 policy/event policy 的公开 API 和测试清单必须先稳定；U3 不能提前，因为 EventLoop 同时消费二者且文档引用必须最后以真实路径更新。每个 Unit 内部也只能按其第 7 节列出的 Red→Green→Refactor→Integration→Regression→Close 顺序执行。

## 9. 执行命令清单

每个 Unit 开始/结束：`cargo nextest list --workspace`。快环：`cargo build --workspace` 与已先验证命中的 targeted nextest。静态：`just fmt-check`、`just lint`。U1：`cargo nextest run -p ralph-cli --bin ralph -- policy_check` 与 `integration_emit_policy`。U2：`cargo nextest run -p ralph-core -- event_policy`。U3：`cargo nextest run -p ralph-core -- event_loop`、`cargo doc --no-deps` 和文档 `rg`/`sed` 复核。最终：`./scripts/run-tests.sh`、`cargo run -p ralph-e2e -- --mock`。任一失败停止当前 Unit。

## 10. 最终质量门禁

当前 HEAD 基线与最终结果均有记录；三 Unit 的 item/test/body hash 校验通过；所有生产区实际拆出；无 stub、遗漏、整块集中或逻辑 diff；公开 API、CLI、事件、policy、prompt、状态和错误语义不变；BDD、scenario、smoke、集成、workspace、build、fmt、lint、doc、mock E2E 全绿；mdc 三条引用均有效；没有 skipped、`.only`、弱化断言或未处理 BLOCKED。

## 11. 最终计划自检

| 检查项 | 结果 |
|---|---|
| 独立业务切片而非技术阶段 | 是：同一运行时 policy/EventLoop 模块拆分能力 |
| 三个共享大文件已合并 | 是 |
| 共享文档只有一个 owner | 是，本计划 |
| 内部 Unit 严格线性 | 是，U1→U2→U3 |
| Executor 无关键设计决策 | 是，需先补齐 item manifest 后执行 |
| 当前是否可执行 | 是；当前 HEAD 全量基线已通过，实施分支仍必须按门禁重新验证 |
| 独立性置信度 | 0.96（解除基线门禁后） |
