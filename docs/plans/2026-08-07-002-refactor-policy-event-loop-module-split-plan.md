# policy、event policy 与 event loop 模块拆分

## 0. 计划状态

- 状态：`READY`；本次更新日期：2026-08-07；实施基线：`5791e21b`。
- 合并原因：三个生产文件共同影响 `.cursor/rules/state-management.mdc`，必须由同一计划拥有文档更新，才能满足独立性与无所有权冲突。
- 调查范围：`policy_check.rs` 5,890 行、`event_policy.rs` 8,406 行、`event_loop/mod.rs` 17,682 行、共享活动规则文档、跨 crate 调用方和测试入口。
- 已执行验证：当前源码行数、顶层 item/`cfg(test)`/impl 方法扫描、调用方扫描、活动文档引用扫描、Git 状态/历史扫描。
- 已执行验证：当前 HEAD 的 `./scripts/run-tests.sh` 通过：Phase 1 为 7576/7576，Phase 2 为 23/23，doctest 为 19/19（4 ignored），退出码 0。
- 已执行验证：`cargo build --workspace` 通过；`just lint`（`cargo clippy --all-targets --all-features -- -D warnings`）通过；`just fmt-check` 初始失败，已用标准 `cargo fmt --all` 修复并再次通过。
- 本次格式修复涉及 8 个已有文件：`crates/ralph-cli/src/presets.rs`、`crates/ralph-cli/src/task_verify_gate.rs`、`crates/ralph-cli/tests/integration_emit_policy.rs`、`crates/ralph-cli/tests/integration_tasks.rs`、`crates/ralph-core/src/correction/mod.rs`、`crates/ralph-core/src/event_loop/mod.rs`、`crates/ralph-core/src/event_loop/precheck_gate_runner.rs`、`crates/ralph-core/src/task.rs`；仅为 rustfmt 输出，无业务逻辑修改。
- 全量测试首次编译又暴露了 `task_cli` 拆分后的 unused-import warning；已收窄 `crates/ralph-cli/src/task_cli.rs` 的生产/测试 re-export 作用域并保留测试可见性。修复后 `cargo check -p ralph-cli --bin ralph --tests` 无 warning/error。
- 格式与 warning 修复后的 `./scripts/run-tests.sh` 已重新通过：Phase 1 为 7576/7576，Phase 2 为 23/23，doctest 为 19/19（4 ignored），退出码 0。
- 尚未执行验证：本次计划实施前的 `cargo nextest list --workspace` 独立清单快照；实施分支必须重新执行，不能复用旧基线数字。
- 阻塞项：无；warning、build、fmt 门禁已清零，可以进入 Unit 1。格式修复应先作为独立 housekeeping 提交，避免与模块搬移混在同一结构 diff 中。

### 0.1 0 回归硬门禁

实施前必须保存当前 HEAD 的全量证据（每个实施分支仍需重新执行）：

```bash
./scripts/run-tests.sh
cargo build --workspace
just fmt-check
just lint
cargo nextest list --workspace
```

此外必须保存三类可回溯清单：

1. **文件清单**：根文件、每个新模块文件、文件行数、模块声明和 re-export。
2. **符号清单**：每个生产 `fn`/`struct`/`enum`/`trait`/`impl`/常量及其原始范围；`impl EventLoop` 还要记录方法多重集。
3. **测试清单**：`cargo nextest list --workspace` 的测试 ID 多重集、测试函数体 hash 和 `#[cfg(test)]`/属性位置。

清单必须保存在实施分支的可审查提交中，但不得写入 `.ralph/review/**` 残留目录；如需过程证据，完成后应合并到计划的实施记录或删除。

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

### 2.1 当前格式与 warning 基线

| 检查 | 结果 | 结论 |
|---|---|---|
| `cargo build --workspace` | 通过 | 当前代码可编译 |
| `just lint` | 通过，`-D warnings` | 当前没有待处理 compiler/clippy warning |
| `just fmt-check`（首次） | 失败，8 个文件存在 rustfmt 漂移 | 不是模块拆分缺陷，先做独立格式修复 |
| `cargo fmt --all` 后 `just fmt-check` | 通过 | 实施前格式门禁已恢复 |
| `./scripts/run-tests.sh` | 旧基线通过 | 格式修复后仍需重新跑一次，作为本次实施的可信基线 |
| `cargo check -p ralph-cli --bin ralph --tests` | 修复后通过且无 warning/error | 覆盖 cargo test 编译路径中 task_cli 拆分产生的 unused imports |
| 格式修复后 `./scripts/run-tests.sh` | 7576/7576、23/23、19/19 | 当前实施前可信回归基线 |

格式修复不属于 U1/U2/U3 的功能范围；若实施分支未包含对应格式提交，必须先 cherry-pick 或重新执行 `cargo fmt --all`，否则禁止开始模块搬移。

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

1. 目标：根 ≤450 行；新增 `gates.rs`、`unified.rs`、5 个测试文件，所有新文件均小于 5,000 行。
2. 对应 R1、policy Scenario、D1/D3/D4、E1-E4。
3. 边界：生产类型/解析器、gates、统一运行器及 5 个测试 mod 按当前 item manifest 搬移；`run_policy_check_unified` 的 cfg(test) 属性随项搬移。
4. 禁止：修改校验逻辑、错误字符串、JSON、schema、emit 或 event_policy。
5. 具体拆分边界：根文件保留公共配置类型、解析入口和模块声明；`gates.rs` 承载 `build_policy_state`、各类 scope/wave/step gate、`PolicyCheckReport` 及其实现；`unified.rs` 承载 unified runner 与 recovery/read-ledger helper；5 个测试族按原命名整体搬入 `policy_check/`，不重写测试体。
6. Red→Green→Refactor：先保存 item/test/hash manifest；先搬测试族，再搬生产族；每次搬移后 build、nextest 命中核对和 root re-export 检查；最后跑 CLI consumer 与全量回归。
7. 验收：policy_check targeted、`integration_emit_policy`、workspace build、fmt、lint、nextest 清单。
8. 风险：跨模块私有 helper 和测试 namespace；以编译、manifest、测试体 hash 检测。
9. 文件所有权：仅 policy_check 根/子模块；mdc 暂不改，留给 U3 统一闭环。

### Unit 2：event_policy.rs 拆分

1. 目标：根 ≤550 行；新增类型/runtime/validation/projection 子模块和 `tests/` 目录；测试文件不得通过空 stub 或单一 misc 重新聚集；所有新文件均小于 5,000 行。
2. 对应 R2、event policy Scenario、D1/D3/D4、E1-E5。
3. 边界：类型/运行时状态/topic validation/projection 按 item manifest 搬移；超过 5,000 行的原 tests 必须按已确认前缀分组。
4. 禁止：改变 reason、topic、deny rule、序列化、公开 API 或 event_loop。
5. 具体拆分边界：根文件只保留公共类型、协议常量、模块声明和 re-export；按现有顶层 item 连续区间拆出 `types.rs`、`runtime.rs`、`validation.rs`、`projection.rs`；大型 `mod tests` 先转为 `event_policy/tests/mod.rs`，再按现有函数名前缀拆成多个测试文件，禁止复制 helper 产生第二份行为。
6. Red→Green→Refactor：manifest/hash → 测试目录实际搬移 → 生产 item 搬移 → 最小可见性 → core targeted → CLI consumer → BDD/scenario/smoke → full。
7. 验收：core policy targeted、event_loop targeted、CLI policy consumer、BDD/scenario/smoke、全量。
8. 风险：公开类型 re-export、私有 validator helper、测试遗漏；以跨 crate build、hash 和全量检测。
9. 文件所有权：仅 event_policy 根/子模块；mdc 暂不改，留给 U3。

### Unit 3：event_loop/mod.rs 与共享文档闭环

1. 目标：根 ≤1,500 行；先拆头部/尾部自由项和 6 个内联测试 mod，再按 10 个连续方法区域拆 `impl EventLoop`；每个新文件小于 5,000 行，`process_parse_result` 保持单一完整方法但不再位于根文件。
2. 对应 R3/R4/R5、EventLoop/结构/文档 Scenario、D1/D4-D6、E1-E7。
3. 具体拆分边界：头部自由项按现有依赖拆入 `prompt_types.rs`、`flow_wiring.rs`、`resume_types.rs`、`stall_recovery.rs` 等语义模块；尾部 6 个 `cfg(test)` 模块搬入 `event_loop/tests/` 并沿用当前模块名；`impl EventLoop` 按连续方法区间拆入 10 个 sibling 文件，首个区域单独承载 `process_parse_result` 的完整方法体，其他区域按当前方法起始顺序搬移，不按“看起来相关”重新排序。
4. 禁止：整块搬成单一 `impl_event_loop.rs`、修改 prompt/事件/状态逻辑、遗漏方法或用大量无理由 `pub(super)` 掩盖错误边界。
5. 先后顺序：A. 测试区与尾部自由项；B. 头部类型/ wiring；C. `impl EventLoop` 的 10 个连续区域；D. 只在真实编译错误驱动下增加 `pub(super)`；E. 更新 `.cursor/rules/state-management.mdc` 的 3 条引用；F. 运行文档 drift、doc、BDD、smoke、mock E2E 和全量门禁。
6. 每个区域的完成条件：原始方法集合与 hash 完全一致、无重复/遗漏、root re-export 可解析、`cargo nextest list` 测试 ID 多重集一致、targeted nextest 通过；任何一个条件失败都停止后续区域。
7. 验收：core event_loop、`event_loop_ralph`、BDD/scenario/smoke、workspace、`cargo doc --no-deps`、mock E2E、全量。
8. 风险：兄弟 impl 模块的私有方法访问、方法重复/遗漏、文档 stale reference；以可见性白名单、方法 hash、路径扫描和 full gate 检测。
9. 文件所有权：本计划独占 event_loop 三个 Unit 及 `.cursor/rules/state-management.mdc`，不与其他 6 份计划冲突。

## 8. Unit 串行依赖图

`Unit 1 policy_check → Unit 2 event_policy → Unit 3 event_loop + mdc`。

U2 不能提前，因为 policy/event policy 的公开 API 和测试清单必须先稳定；U3 不能提前，因为 EventLoop 同时消费二者且文档引用必须最后以真实路径更新。每个 Unit 内部也只能按其第 7 节列出的 Red→Green→Refactor→Integration→Regression→Close 顺序执行。

## 9. 执行命令清单

每个 Unit 开始/结束：`cargo nextest list --workspace`。快环：`cargo build --workspace` 与已先验证命中的 targeted nextest。静态：`just fmt-check`、`just lint`。U1：`cargo nextest run -p ralph-cli --bin ralph -- policy_check` 与 `cargo nextest run -p ralph-cli --test integration_emit_policy`。U2：`cargo nextest run -p ralph-core -- event_policy`，再跑受 event policy 消费影响的 `event_loop` 子集。U3：`cargo nextest run -p ralph-core -- event_loop`、`cargo nextest run -p ralph-core --test scenarios`、`cargo nextest run -p ralph-core --features recording --test smoke_runner`、`cargo doc --no-deps` 和文档 `rg`/`sed` 复核。最终：`./scripts/run-tests.sh`、`cargo run -p ralph-e2e -- --mock`。禁止裸跑 `cargo test`；任一失败停止当前 Unit。

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
| 当前是否可执行 | 是；build/lint/fmt 已通过，实施分支仍必须重新跑 nextest 与全量基线 |
| 独立性置信度 | 0.96（解除基线门禁后） |

## 12. 实施记录（2026-08-07，本计划复用 worktree 阶段）

本计划在复用 worktree `ralph/2026-08-07-002-refactor-policy-event-loop-module-split-plan` 下由 executor + fixer 双阶段落地。U1 已完成；U2 已完成（生产区拆分，namespace 收敛）；U3 部分完成（mdc 文档闭环已应用，EventLoop mod.rs 拆分未完成——见 §12.3 residual）。完整证据在 `.ralph/review/2026-08-07-002-refactor-policy-event-loop-module-split-plan/`。

### 12.1 已完成

- **U1 policy_check.rs 拆分**：commit `23c6b24a`。`policy_check.rs` 由 5,890 行压至 213 行；`policy_check/{gates,unified}.rs` + 5 个测试文件全部 < 5,000 行。
- **U2 event_policy.rs 拆分**：commit `20627e00`（executor）。`event_policy.rs` 由 8,406 行压至 180 行 facade；`event_policy/{types,runtime,validation,projection}.rs` + `tests/{mod,helpers,tests_part1,tests_part2}.rs` 全部 < 5,000 行；fixer 在 `a80a77a0` 进一步用 `include!` 收敛 `tests_part{1,2}::tests` 命名空间使测试 ID 还原 baseline `event_policy::tests::<fn>`。
- **U3 mdc 文档闭环**：commit `32704916`（fixer）。`.cursor/rules/state-management.mdc` 的 4 处漂移引用全部更新到真实路径；glob 改为 `event_loop/**/*.rs` + `event_policy/**/*.rs`；`scripts/check-cli-doc-drift.sh` 通过。
- **U4 fixture namespace 还原**：commit `a80a77a0`（fixer）。
- **U6 删除 7 个 dead helpers**：commit `8bb5a9f1`（fixer）。`event_policy/tests/helpers.rs` 删除 `review_passed_allowlist_config`、`work_done_payload`、`review_dimensions_complete_payload`、`insert_review_dimensions_schema`、`test_config_with_enforce_and_resume`、`work_ready_payload`、`test_result_payload`；移除文件级 `#![cfg_attr(test, allow(dead_code))]`。

### 12.2 全量门禁（fixer 完成阶段后）

- `cargo build --workspace`：PASS
- `RUSTFLAGS='-D warnings' cargo check --workspace --all-targets`：PASS
- `just fmt-check`：PASS
- `just lint`（clippy -D warnings）：PASS
- `cargo nextest run -p ralph-cli --bin ralph -- policy_check`：126/126 PASS
- `cargo nextest run -p ralph-cli --test integration_emit_policy`：13/13 PASS
- `cargo nextest run -p ralph-core --no-fail-fast -- event_policy`：202/202 PASS
- `cargo nextest run -p ralph-core --no-fail-fast -- event_loop`：1228/1228 PASS
- `scripts/check-cli-doc-drift.sh`：PASS

### 12.3 Residual — 后续独立计划应处理的 P2 事项

1. **EventLoop `mod.rs` 拆分为 10 个连续 impl 区域**（fix-plan §7 U2 / 本计划 §7 Unit 3）。本次 fixer 激活尝试用 `impl_region_01.rs..10.rs` 拆分 `impl EventLoop` 的 287 个方法，但因（a）每个方法都有前置 `///` doc comment 导致边界级联修复无界；（b）3,761 行的 `process_parse_result` 与首个区域绑定；（c）尾部嵌套测试模块 `flow_authority_pf_declared_14step_tests` 包裹 impl 闭合大括号——三个原因叠加，简单 subagent 在本 activation 50 分钟内未能收敛。mod.rs 维持 17,691 行（基线状态）。后续计划应：（i）允许单 impl region 文件超出 5,000 行（如把 `process_parse_result` 单独放进 `impl_region_process_parse_result.rs` 而不嵌入区域 06a）；（ii）使用 `#[cfg(test)] mod flow_authority_pf_declared_14step_tests_tests { ... }` 的命名重构把尾部嵌套测试模块先移出 `impl EventLoop`；（iii）按"包含 fn + 完整 doc comment"成对切分避免 orphan doc。
2. **`validate_event_with_options` 内部关切拆分**：当前 validator 仍保留单一大函数（与 `process_parse_result` 同类巨型结构）；下一轮 refactor 应按 facade 模式（顶层 facade + `validate_event_with_options::internal_{payload_consistency,schema,envelope,topic_format,completion}`）拆分。
3. **`event_policy` 模块 facade 的 `pub(crate)` re-export 与历史 `#[allow(...)]` 收敛**：executor U2 commit 引入了若干 `pub(crate)` 用于跨模块访问；review 中记录的 G3 / M3 / C3(b)/(c)/(d) / M1 / M5 / S4 / A2 等待独立 plan 处理。
4. **fixture helper `StubHandoff` 与 `HITTING_PAYLOAD` 在 `tests/helpers.rs` 的去留**：目前这两个 item 仍 active 使用，不在 U6 删除范围；下一轮如不需要应单独清理。

### 12.4 不在本计划范围

- 修改事件协议、schema、错误语义、配置字段、preset、agent-facing skill 行为或业务逻辑。
- 修改或弱化既有测试断言、更新 snapshot 以取得 Green。
- 修改 operator-facing CLI 参数或启动流程。
