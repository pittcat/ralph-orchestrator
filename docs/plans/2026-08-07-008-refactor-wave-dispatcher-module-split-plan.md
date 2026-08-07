# 将 wave dispatcher 按职责拆分为可维护模块

## 0. 计划状态

- `READY`：计划基于当前 HEAD `181638ac349319551b7d8a6c627ea4ca026646b7`；相对原基线仅增加 event-policy 回归测试与计划文档更新，wave dispatcher 生产代码和拆分边界未变化。所有拆分边界均由当前符号、调用方和测试证据确认，未引入数字编号模块名。
- 调查范围：`crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`、`wave/mod.rs`、`worker.rs`、`supervisor_bridge.rs`、`io.rs`、`crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs`、相关 dispatcher 内联测试、最近 dispatcher Git 历史和仓库测试入口。
- 已执行验证：`git rev-parse HEAD`、`git log --oneline -12 -- crates/ralph-cli/src/loop_runner/wave`、`wc -l`、符号/调用关系搜索；未在本次计划更新中重新执行全量测试。
- 执行前硬门禁：必须在当前 HEAD 重新运行 `./scripts/run-tests.sh`、`cargo build --workspace`、`just fmt-check`、`just lint`、`cargo nextest list --workspace`；格式化问题先运行 `cargo fmt --all` 修复，再重新执行 `just fmt-check`。修复后仍失败才不得进入 Unit 1。

## 1. 功能目标

### 1.1 业务目标与调用方

调用方是 `wave/mod.rs` 暴露的 `handle_wave_events`、`execute_wave`、supervisor dispatch/fan-in 入口，以及 `loop_runner`。目标是只改变 Rust 文件组织和私有可见性，不改变任何外部可观察的 wave 行为：worker 启动数量、并发限制、retry、worker/aggregate/global deadline、fan-in、salvage、coordination、事件 payload、诊断和终态投影必须保持不变。

### 1.2 当前行为

所有生产实现和大部分测试集中在 `dispatcher.rs`；文件当前 13,543 行。`mod.rs` 仅声明单一 `mod dispatcher` 并从中 re-export 对外和测试需要的符号。最近提交已强化 aggregate deadline 与 fan-in 的共同 deadline 语义，因此这些函数必须整体搬移，不得顺手改写。

### 1.3 目标行为与边界

- 根文件保留稳定的公共/`pub(crate)` re-export 和职责入口。
- 生产文件按功能命名，不使用 `module-1`、`part_1`、`u1`、`helpers` 这类数字或无业务含义名称：
  - `dispatch.rs`：wave 入口、普通 dispatch、supervisor round dispatch、`WorkerRequest`/executor/dispatch context。
  - `worker_lifecycle.rs`：单 worker attempt 的执行、permit/release guard、超时/取消后的 worker 生命周期；仅承载已存在的生命周期函数。
  - `fan_in.rs`：`run_supervisor_fan_in` 及 fan-in 状态推进、coordination 前置调用。
  - `coordination.rs`：coordination payload、commit/failure coordination、摘要和指纹。
  - `salvage.rs`：exec/fix salvage merge、空 salvage、salvage receipt 与相关 JSON 构造。
  - `deadlines.rs`：aggregate/partial/global deadline 计算及已存在的 deadline 辅助函数。
  - `outcomes.rs`：slot result 分类、synthetic failure、最终 `WaveDispatchOutcome` 转换和相关序列化。
- 测试按可观察行为命名并进入 `dispatcher_tests/`：`dispatch.rs`、`worker_lifecycle.rs`、`fan_in.rs`、`coordination.rs`、`salvage.rs`、`deadlines.rs`。若某测试无法归入上述行为，才保留在 `dispatcher_tests/mod.rs`，且必须记录原因；不得以 `misc.rs` 吸收整批测试。
- 不拆 `dispatch_wave_inner_with_release` 的函数体，不改变算法，不新增接口，不修改 `wave_supervisor.rs` 的测试合同。

### 1.4 非目标

不修改 retry 语义、attempt receipt、Worktree reuse、supervisor store、event policy、CLI 参数、配置、公开 API、事件 schema、worker spawn 逻辑和测试断言。attempt reuse 计划只消费本计划完成后的符号定位结果，不在本计划提前接入。

## 2. 代码库现状与证据

### 2.1 当前实现入口

`crates/ralph-cli/src/loop_runner/wave/mod.rs` 通过 `dispatcher` re-export `handle_wave_events`、`execute_wave`、`WaveOutputs`、`WaveDispatchLimits`、`WaveDispatchOutcome`，并 re-export supervisor dispatch/fan-in 测试入口。`dispatcher.rs` 顶部定义这些类型和 `WorkerRequest`，中部包含普通 wave/supervisor dispatch 与 `run_supervisor_fan_in`，后部包含 coordination、salvage、deadline、outcome helper，文件末尾从约 6,779 行开始是内联 tests。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `git rev-parse HEAD` 与当前代码差异 | 当前基线为 `181638ac349319551b7d8a6c627ea4ca026646b7`；相对 82df 仅有 event-policy 测试和计划文档变更，dispatcher 代码未变 | 执行前仍需在实际 HEAD 重跑门禁；不得把文档/无关测试提交误当作 dispatcher 行为变化 | 高 |
| E2 | `wc -l crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | 文件为 13,543 行，生产代码与 tests 混合 | 必须拆生产职责与测试目录 | 高 |
| E3 | `dispatcher.rs` 顶层定义与 `wave/mod.rs` re-export | 公开入口、supervisor 入口、测试入口均来自 dispatcher | 根文件必须保留 re-export；拆分不得改变调用方路径 | 高 |
| E4 | `dispatcher.rs` 的符号扫描 | 存在明确的 dispatch、fan-in、coordination、salvage、deadline、outcome 函数群 | 文件名按功能命名；不得使用数字编号 | 高 |
| E5 | `dispatcher.rs::dispatch_wave_inner_with_release` | 该函数是完整的 dispatch 生命周期和 deadline/abort/drain 边界 | 整体搬移，不拆函数体、不改控制流 | 高 |
| E6 | `dispatcher.rs::run_supervisor_fan_in` | fan-in 包含 terminal salvage/coordination 顺序和 aggregate timeout 参数 | fan-in 文件必须保留现有顺序和调用关系 | 高 |
| E7 | `dispatcher.rs` 最近变更与 `git log` | 最近提交涉及 aggregate deadline、fan-in 和失败回归 | 必须保留这些回归测试并在每批搬移后验证 | 高 |
| E8 | `wave/mod.rs` | 已有 `channel_registry`、`io`、`worker`、`supervisor_bridge` 等职责模块 | 新模块遵循同级文件模式，不创建嵌套数字目录 | 高 |
| E9 | `AGENTS.md` | 测试必须使用 nextest/`./scripts/run-tests.sh`，ralph-cli 禁止裸 cargo test | 所有命令按仓库硬规则编写 | 高 |

### 2.3 受影响范围

- 生产：`crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`（改为 façade 或删除为同名模块目录入口）、计划新增的功能命名 dispatcher 子模块、`wave/mod.rs`（仅在 re-export 物理路径变化时调整）。
- 测试：`dispatcher.rs` 当前 `#[cfg(test)] mod tests` 搬入计划新增的 `crates/ralph-cli/src/loop_runner/wave/dispatcher_tests/` 文件；`wave_supervisor.rs` 只作回归，不重写。
- 调用方：`wave/mod.rs`、`loop_runner` 中引用 dispatcher re-export 的代码、测试中引用 `crate::loop_runner::wave` 的代码。
- 不受影响：`worker.rs`、`supervisor_bridge.rs`、`io.rs`、`channel_registry.rs`、core supervisor、配置、preset/schema、API/UI。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | 生产模块如何命名 | 数字分片；职责名文件；继续单文件 | 使用 `dispatch`、`worker_lifecycle`、`fan_in`、`coordination`、`salvage`、`deadlines`、`outcomes` 职责名 | E4,E8 | 数字名无法表达功能；单文件不解决维护边界 | 0.97 |
| D2 | `dispatch_wave_inner_with_release` 是否拆函数 | 重写拆解；整体搬移；复制兼容实现 | 整体搬移，保持函数体和调用顺序 | E5,E7 | 重写会把纯重构变成行为变更，难以证明 deadline/abort 等价 | 0.99 |
| D3 | 测试如何拆分 | 留在根文件；按数字分片；按行为目录模块 | `dispatcher_tests/` 下按 dispatch、worker_lifecycle、fan_in、coordination、salvage、deadlines 命名 | E2,E4,E7 | 根文件继续过大；数字名无语义；行为名可映射验收风险 | 0.95 |
| D4 | 跨模块可见性 | 全部 `pub`；全部复制 helper；最小 `pub(super)`/`pub(crate)` | 只为真实消费者提升最小可见性，公开表面由 `wave/mod.rs` 维持 | E3,E8 | 扩大公开 API 或复制实现都会产生新合同 | 0.96 |

## 4. BDD 行为规格

```gherkin
Feature: wave dispatcher 按职责拆分后保持既有行为

  Scenario: 普通 wave 仍按原入口完成派发与终态投影
    Given 调用方使用 wave/mod.rs 暴露的 dispatch 入口
    When 输入与拆分前相同的 wave、backend 和并发限制
    Then worker 数量、最终 outcome、事件写入和 CLI/RPC/TUI 投影与拆分前一致

  Scenario: deadline 与 abort/drain 顺序不变
    Given wave 触发 partial、aggregate 或 global deadline
    When 执行拆分后的 dispatcher
    Then 仍按原语义 abort、drain、生成 synthetic failure 并返回对应 outcome

  Scenario: supervisor fan-in、coordination 与 salvage 顺序不变
    Given supervisor wave 有完成、失败、部分失败或 salvage 输入
    When dispatcher 完成 fan-in
    Then coordination/salvage 事件、状态推进、指纹和失败语义与基线一致

  Scenario: 测试按行为文件拆分但覆盖集合不减少
    Given 拆分前 dispatcher 内联测试的测试名称集合
    When 测试迁移到 dispatcher_tests 行为文件
    Then所有测试仍被 nextest 发现，名称集合和断言保持不变
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 测试层级 | 风险补充测试 | E2E |
|---|---|---|---|---|---|
| 普通派发 | `wave/mod.rs` 原 re-export 可编译，相关 dispatcher/wave 测试通过 | `cargo nextest run -p ralph-cli --bin ralph -- <实际命中的过滤器>` | 集成/模块回归 | 保留现有 retry、spawn、progress 断言 | 否 |
| deadline | partial/aggregate/global 相关测试全部命中且通过 | `cargo nextest run -p ralph-cli --bin ralph -- <实际命中的过滤器>` | 集成 | 最近 aggregate deadline 回归测试 | 否 |
| fan-in/coordination/salvage | supervisor 终态和 payload 断言不变 | `cargo nextest run -p ralph-cli --bin ralph -- <实际命中的过滤器>` | 集成 | fan-in failure/salvage 边界 | 否 |
| 测试集合 | `cargo nextest list --workspace` 与迁移前保存的测试名多重集一致 | `cargo nextest list --workspace` | 结构化回归 | 对生产 item、属性和方法体做迁移前后清单/hash 对比 | 否 |

每个场景的 Red 必须来自目标拆分造成的编译/发现/行为差异；测试命令错误、fixture 缺失、环境损坏不算 Red。不得更新断言来适配搬移结果。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | 入口与 re-export 保持可用 | 普通派发 | 既有 wave dispatch 测试 | 原 dispatcher 测试 | `wave_supervisor.rs` | 否 | E3,E8 |
| R2 | deadline/abort/drain 行为不变 | deadline | 既有 deadline 测试 | 原 deadline 测试 | ralph-cli targeted | 否 | E5,E7 |
| R3 | fan-in/coordination/salvage 不变 | fan-in | 既有 supervisor 测试 | 原行为测试 | `wave_supervisor.rs` | 否 | E6,E7 |
| R4 | 测试覆盖集合不减少 | 测试集合 | nextest list 多重集比较 | 迁移后各行为文件 | workspace list | 否 | E2,E9 |

## 7. 严格串行开发单元

### Unit 1：按职责迁移 dispatcher 生产实现与测试集合

1. **Unit 目标**：完成一次纯物理拆分，使功能命名模块承载既有生产实现和测试，调用方行为、公共 re-export、测试名称和断言不变。
2. **对应需求与 Scenario**：R1–R4；Decision D1–D4；Evidence E1–E9。
3. **外部可观察结果**：`wave/mod.rs` 入口仍可被现有调用方使用；普通 dispatch、deadline、fan-in、coordination、salvage 和测试发现结果不变。
4. **当前行为基线**：`dispatcher.rs` 13,543 行；生产实现与 `#[cfg(test)] mod tests` 同文件；`dispatch_wave_inner_with_release`、`run_supervisor_fan_in` 和相关 deadline/outcome helper 均在该文件中（E2–E7）。
5. **输入/输出/错误/副作用**：输入、输出、错误、事件、文件写入、数据库调用和 spawn/abort 行为全部保持基线；唯一允许副作用是 Rust 文件路径和私有可见性变化。
6. **修改位置**：修改 `wave/dispatcher.rs`、必要时 `wave/mod.rs`；新增 `wave/dispatcher/` 目录下职责命名生产文件和 `wave/dispatcher_tests/` 行为命名测试文件。每个符号只能有一个物理定义；不得修改 `worker.rs`、`supervisor_bridge.rs`、`io.rs`、`wave_supervisor.rs`。
7. **可依赖能力**：现有 `wave/mod.rs` re-export、现有 dispatcher 内联 tests、当前 nextest 配置。
8. **禁止依赖的未来能力**：不得实现 attempt receipt、Recovery Context、Worktree reuse、store 新接口或新的业务行为。
9. **验收测试**：先保存当前 `cargo nextest list --workspace` 与 dispatcher 生产 item/测试函数清单；逐批搬移后运行命中的 targeted nextest；完成后比较符号、属性、字符串、方法体 hash，并运行 wave supervisor 回归。
10. **Acceptance Red**：执行前先建立清单，不人为制造失败；第一次迁移后，若目标测试未被发现、re-export 编译失败或 deadline/fan-in 测试失败，记录真实错误并停止。只因过滤器拼写错误或未执行目标测试不算 Red。
11. **单元测试拆分**：沿用现有测试，不新增行为断言；按测试实际覆盖的入口分别放入 `dispatcher_tests/dispatch.rs`、`worker_lifecycle.rs`、`fan_in.rs`、`coordination.rs`、`salvage.rs`、`deadlines.rs`。无法归类的单个测试放 `dispatcher_tests/mod.rs` 并注明职责。
12. **Red → Green → Refactor 顺序**：保存基线清单 → 建立模块声明与空的迁移壳（只允许用于编译组织，不得留生产 stub）→ 迁移 dispatch 入口和 `WorkerRequest`/executor → 迁移 worker lifecycle → 迁移 deadline/outcome → 迁移 fan-in → 迁移 coordination → 迁移 salvage → 迁移 tests → 恢复 re-export → targeted Green → 清理重复 import/最小可见性 → list/hash/full regression。
13. **最小实现范围**：仅移动已确认符号、更新 `use`/可见性和 re-export；不重写函数、不改变条件顺序、不合并/拆分业务函数、不更新快照、不新增依赖。
14. **集成验证**：必须真实联合 `wave/mod.rs`、dispatcher 新模块、`worker.rs`、`supervisor_bridge.rs` 和 `wave_supervisor.rs`；外部 backend 仍使用现有测试 fake；最终运行 workspace build 与 E2E mock 作为回归，不把 E2E 当作唯一证明。
15. **风险驱动测试**：Characterization（当前未拆代码的既有断言）；Differential（迁移前后 item/测试名称/方法体 hash）；Regression（最近 deadline/fan-in 提交对应测试）。不新增无风险的 mock 或文本测试。
16. **回归范围**：直接 dispatcher tests、`wave_supervisor.rs`、ralph-cli 全包 nextest、workspace build/lint/fmt、`cargo run -p ralph-e2e -- --mock`。这些范围覆盖入口 re-export、worker 生命周期、supervisor fan-in 和构建目标。
17. **预期文件变更**：

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | 修改或改为模块 façade | 保留稳定入口和 re-export | E3,E5 |
| `crates/ralph-cli/src/loop_runner/wave/mod.rs` | 修改现有生产文件 | 仅在物理路径变化时调整模块声明/re-export | E3,E8 |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher/` | 新增职责命名生产文件 | 按功能承载已存在实现 | E4 |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher_tests/` | 新增行为命名测试文件 | 拆分现有内联测试，保持覆盖集合 | E2,E7 |

18. **完成标准**：所有目标测试命中且通过；测试名称多重集、生产 item、属性、字符串和方法体 hash 与基线一致；`cargo build --workspace`、`just fmt-check`、`just lint`、相关 nextest、`cargo run -p ralph-e2e -- --mock` 和 `./scripts/run-tests.sh` 通过；无 stub、跳过、断言削弱或数字模块名。
19. **停止条件**：发现需要改业务逻辑、调整 deadline/fan-in/salvage、修改公开 API、测试集合减少、现有符号找不到、模块名无法按职责表达、或必须修改本计划非目标文件时停止；记录新 Evidence，重新做 Decision，不得继续搬移。
20. **风险与注意事项**：私有 helper 跨模块可见性变化可能造成编译失败；通过最小 `pub(super)` 解决。模块声明错误可能导致重复定义；通过单一物理定义和 list 检查解决。测试漏迁移可能只在全量时暴露；通过迁移前后测试名多重集和全量 nextest解决。剩余风险是未来 dispatcher 行为变更需要同步多个职责文件，但这是本拆分的预期维护边界。

## 8. Unit 串行依赖图

只有一个实施 Unit：先完成基线清单，再按职责顺序迁移并在同一 Unit 内逐批验证。attempt reuse 计划必须等待本 Unit 完成，因为其 dispatcher 入口在物理路径稳定前不应写入固定文件位置；本计划不得提前实现 attempt 行为。

## 9. 执行命令清单

| 时机 | 命令 | 目的 | 失败处理 |
|---|---|---|---|
| 执行前 | `./scripts/run-tests.sh` | 当前全量基线 | 失败即停止 |
| 执行前 | `cargo build --workspace` | 编译基线 | 失败即停止 |
| 执行前 | `cargo fmt --all && just fmt-check` | 修复并确认格式基线 | fmt warning 可修复；修复后检查仍失败才停止 |
| 执行前 | `just lint` | lint 基线 | 失败即停止 |
| 每批迁移后 | `cargo nextest list --workspace` | 确认测试仍被发现 | 未命中即停止 |
| 每批迁移后 | `cargo nextest run -p ralph-cli --bin ralph -- <已验证命中的过滤器>` | 验证当前行为族 | 失败不得进入下一批 |
| Unit 完成 | `cargo nextest run -p ralph-cli --bin ralph -- <wave_supervisor/dispatcher 已验证过滤器>` | 入口与 supervisor 回归 | 失败不得关闭 Unit |
| Unit 完成 | `cargo build --workspace && just fmt-check && just lint` | 构建与静态门禁 | 任一失败不得关闭 Unit |
| 最终 | `cargo run -p ralph-e2e -- --mock` | 真实 CLI E2E smoke | 失败不得声明完成 |
| 最终 | `./scripts/run-tests.sh` | 仓库规定的完整回归 | 失败不得声明完成 |

## 10. 最终质量门禁

所有 Scenario 可追踪到 R、Evidence 和 Unit；生产模块文件名全部按功能命名且无数字编号；测试发现集合不减少；deadline、fan-in、coordination、salvage、retry 和终态投影无行为差异；全量 nextest、build、fmt、lint、E2E mock 通过；无新增跳过测试、无削弱断言、无未来行为提前实现。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap | 是 | 明确到符号职责、文件名、测试、Red 和命令 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D4 已冻结命名、拆分边界、可见性与搬移策略 |
| 所有文件和接口是否有代码库证据 | 是 | E2–E8；计划新增路径已明确标记为新增 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1–D4 为 0.95–0.99 |
| 是否存在未处理的低置信度假设 | 否 | 没有低于阈值的实施决策 |
| 每个 Unit 是否只有一个可观察行为 | 是 | 只有一次纯重构行为闭环 |
| 每个 Unit 是否可以独立验证 | 是 | 有 list/hash、targeted、workspace、E2E 和全量门禁 |
| 每个 Unit 是否有真实 Red | 是 | 迁移后编译、发现或既有回归失败才算有效 Red |
| 每个 Unit 是否包含回归范围 | 是 | §7.16 和 §9 |
| 是否存在未来 Unit 依赖 | 否 | 只有单一 Unit；attempt reuse 是外部后续计划，不被提前实现 |
| 是否存在泛化任务描述 | 否 | 每项对应真实文件、符号、行为和命令 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §5、§6、Unit 1 |
| 所有关键决策是否有 Evidence | 是 | D1–D4 均引用 E |
| 计划是否可以严格串行执行 | 是 | 单一 Unit 内按职责顺序执行 |
