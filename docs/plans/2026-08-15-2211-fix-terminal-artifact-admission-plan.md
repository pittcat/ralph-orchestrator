---
type: fix
title: "终止事件必须绑定真实可读产物"
date: 2026-08-15
origin: docs/brainstorms/2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-brainstorm
execution: code
---

# 终止事件必须绑定真实可读产物：开发计划

## Goal Capsule

### 0. 计划状态

- 状态：READY。
- 基线：`d0e53e75e0ea078ea9b43afdf8b16adeaee15d87`，分支 `pittcat-dev`，调查时工作树干净。
- 目标：阻止带有不存在、目录、工作区外或不可解析路径的终止产物声明继续被 `LOOP_COMPLETE`/同类 completion promise 接受。
- 调查范围：终止检查、required-field/schema 校验、事件接纳顺序、`ce-executor-pipeline` 及其 loop 变体、autoresearch/debug 的终止 schema、真实 BDD 场景 harness、相关 Git 提交。
- 已执行的验证命令：`git rev-parse HEAD`、`git status --short`、`rg`/`sed`/`nl` 检查上述源码、preset、scenario 和提交历史；检查了相关文件行数。
- 尚未执行的验证：本计划阶段不运行测试、不改生产代码；所有执行命令列在第 9 节，必须由 Executor 按 Unit 顺序执行。
- 阻塞项：无。实现关键决策均有直接源码/schema/测试入口证据，置信度不低于 0.90。

## Product Contract

### 1. 功能目标

- 业务目标：终止状态只能建立在真实最终产物之上，消除“字段存在但文件不存在”导致的假成功。
- 调用方：`EventLoop` 的 completion 检查、使用 `report_path`/`artifact_path` 的 builtin preset；CLI/agent 仍通过现有 emit 入口发送事件。
- 当前行为：schema 只检查字段存在；`check_completion_event` 检查 required topics、verdict 和字段值一致性，但不检查路径对应文件。prompt 只要求 agent 自行执行 `test -f`。
- 目标行为：当 completion promise 的 schema 要求 `report_path` 或 `artifact_path` 时，只有该字段为工作区相对路径、解析后位于工作区内且对应普通文件可读，终止才可被 honor。
- 行为差异：无产物字段的 completion promise 保持原行为；有产物字段但路径无效时，completion 被拒绝并进入既有 completion rejection/correction 路径，不设置 `completion_honored`。
- 本次范围：运行时终止门禁、最小路径校验 helper、真实 termination/BDD 覆盖、受影响 fixture、agent-facing 文档中“运行时会验证”的准确描述。
- 非目标：不修改 `required_fields` 的通用语义；不把所有中间 artifact 事件都改成文件存在校验；不修改 preset hat 拓扑、事件名、completion promise 默认值；不增加配置字段或外部依赖；不要求文件非空。
- 输入：已接纳 completion event 的 payload、当前 `config.core.workspace_root`、completion promise 对应 `EventSchema`。
- 输出：有效路径时维持原 `TerminationReason::CompletionPromise`；无效路径时返回 `None` 或现有 `CompletionStuck`，并记录稳定 rejection signature/纠正上下文。
- 状态变化：有效路径才允许后续既有逻辑清除 `completion_requested` 并设置 `completion_honored`；无效路径不得把 completion promise、required event 或 `report_done_seen` 当作成功终止证据。
- 错误语义：路径为空、绝对路径、解析失败、工作区外、目标不存在、目标为目录或不可读，均为终止契约拒绝；错误必须可诊断但不能泄露工作区外的敏感绝对路径。
- 兼容性要求：没有 completion schema、schema 没有 `report_path`/`artifact_path`、或 completion promise 不是终止产物契约的 preset 保持现有行为；旧 JSONL/旧 ledger 不迁移。
- 性能要求：只在实际 completion 检查时校验一次最终路径；不扫描事件历史、不读取文件内容，不增加每个普通事件的 I/O。
- 安全/权限要求：拒绝绝对路径和 canonical path 越出 workspace 的路径，避免通过 `..` 或工作区外 symlink 伪造产物；只需要普通文件 metadata/readability 检查。
- 已确认假设：当前所有终止路径都会经过 `EventLoop::check_completion_event`；completion schema 已在运行配置中可访问；工作区根目录来自 `config.core.workspace_root`。
- 待验证假设：场景 fixture 中是否已有 completion artifact 文件。进入 Unit 1 前必须用 `rg` 对所有实际命中 scenario 逐个确认；若缺失，只在对应 scenario 添加既有 `fixture_files`，不得降低运行时校验。

### 4. BDD 行为规格

```gherkin
Feature: 终止 promise 的最终产物证据门禁

  Background:
    Given EventLoop 的 completion schema 要求 `report_path`
    And workspace_root 是当前场景临时工作区

  Scenario: 真实可读报告允许终止
    Given workspace 下存在普通文件 `docs/report.md`
    And completion payload 的 `report_path` 是 `docs/report.md`
    When EventLoop 接收 completion promise 并运行 completion 检查
    Then 返回 CompletionPromise
    And completion_honored 为 true
    And 不产生产物缺失 rejection

  Scenario: 不存在的报告阻止终止
    Given workspace 下不存在 `docs/missing.md`
    And completion payload 的 `report_path` 是 `docs/missing.md`
    When EventLoop 运行 completion 检查
    Then 不返回 CompletionPromise
    And completion_honored 仍为 false
    And rejection 说明最终产物不可读或不存在

  Scenario: 目录不能冒充报告
    Given workspace 下存在目录 `docs/report`
    When completion payload 的 `report_path` 为 `docs/report`
    Then completion 不被 honor

  Scenario: 工作区外路径和 symlink escape 不能终止
    Given completion payload 使用绝对路径或 `../outside.md`
    Or payload 路径经过 symlink 解析后位于 workspace 外
    When EventLoop 运行 completion 检查
    Then completion 不被 honor

  Scenario: 没有产物契约的旧 completion 行为不变
    Given completion schema 不要求 `report_path` 或 `artifact_path`
    When EventLoop 接收原有合法 completion payload
    Then 仍按原规则返回 CompletionPromise
```

## Planning Contract

### 2. 代码库现状与证据

#### 2.1 当前实现入口

- 外部事件进入 `crates/ralph-core/src/event_loop/parse_and_emit/legacy.rs::process_parse_result`；当前代码先把接纳事件记录进 `LoopState`，再将业务事件通过 `disposition::publish_synthetic_with_state_machine_projection` 或旧 direct channel 发布。
- completion 终止入口是 `crates/ralph-core/src/event_loop/wave_scope.rs::check_completion_event`。它现有顺序为 required-events、verdict、completion payload match、workflow guard、persistent mode，最后才完成终止状态变化。
- schema 字段存在检查在 `crates/ralph-core/src/validation/rules_required_fields.rs::RequiredFieldsRule`，只调用 `map.contains_key`，不做 filesystem 检查。
- agent 提示在 `crates/ralph-core/src/event_loop/event_processing.rs::append_terminal_deliverable_contract`，目前仅把 `test -f` 写进 prompt，不能约束恶意/错误 emit。
- `presets/en/ce-executor-pipeline.yml` 和 `presets/en/ce-executor-pipeline-loop.yml` 的 `LOOP_COMPLETE` schema 要求 `report_path`，且要求与 `report.done.report_path` 相同；`autoresearch`/`debug` 也有 completion schema 的 `report_path`。
- 真实 BDD harness 是 `crates/ralph-core/tests/scenarios.rs::run_workflow_guard_scenario`/`run_scenario_with_snapshots`；scenario YAML 支持 `fixture_files`，临时 workspace 会在运行前写入 fixture。

#### 2.2 Evidence Ledger

| ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `docs/brainstorms/2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md` GAP-13、总结 | 文档将 artifact existence/orphan 归入 P2，但当前 adversarial 审查发现它仍能穿过 completion 造成 P0 级假成功 | 本计划只修复“终止时产物证据”这一窄边界，不重开全部 GAP-03 | 高 |
| E2 | `presets/en/ce-executor-pipeline.yml` `event_loop` 与 `report.done`/`LOOP_COMPLETE` schema | completion 需要 `report.done`，两类 payload 都携带 `report_path`；字段文档明确要求 readable，但只是声明 | 运行时必须把文档契约变成硬门禁 | 高 |
| E3 | `crates/ralph-core/src/validation/rules_required_fields.rs:38-105` | 规则只判断 JSON object 是否含字段 | 不能在该通用 rule 中加入 filesystem 语义，否则会改变所有 required field 调用方 | 高 |
| E4 | `crates/ralph-core/src/event_loop/wave_scope.rs:619-835` | completion 检查已有稳定 rejection、correction、verdict、payload-match 流程，最终状态变化集中在此入口 | 最小修改点是 completion honor 之前的专用终止产物检查 | 高 |
| E5 | `crates/ralph-core/src/event_loop/event_processing.rs:1034-1083` | `test -f` 只存在于 prompt 文本 | prompt 不能作为安全边界；保留提示，但不能依赖它 | 高 |
| E6 | `crates/ralph-core/src/event_loop/tests/termination.rs:1083-1259`、`event_policy.rs`、`chain_validation.rs` | 已有 completion payload match、无 policy/无 required event 的 characterization 测试 | 新行为应扩展 termination 测试，并保留未配置路径测试 | 高 |
| E7 | `crates/ralph-core/tests/scenarios.rs:1141-1195` | scenario 运行在真实 EventLoop，支持临时 workspace fixture | BDD 不得用文本/source assertion；需要真实文件 fixture | 高 |
| E8 | `presets/en/autoresearch.yml:87-94`、`presets/en/debug.yml:89-96`、`implementation-review.yml:677-704` | 还有其他 completion schema 使用 `report_path`/`artifact_path` | helper 不能写死 ce-executor；应按 completion schema 的字段发现规则工作 | 高 |
| E9 | Git `d68b5f98`、`2ac70963`、`a38b0218`、`7391a438`、`f57ac2b0` | 历史修复不断加强 artifact handoff、precheck 和终止契约，但没有把最终路径解析提升为 runtime admission | 说明这是现有契约的闭环缺口，不应新建第二套 artifact workflow | 中高 |
| E10 | `wc -l` 当前文件检查 | `wave_scope.rs` 1190 行、`event_processing.rs` 2282 行，均低于 5000 行硬上限 | 可在既有模块增加窄 helper；不得把逻辑堆进已接近上限文件 | 高 |

#### 2.3 受影响范围

- 生产：`crates/ralph-core/src/event_loop/wave_scope.rs`；如为保持职责清晰需要抽取，只能新增由 `event_loop/mod.rs` 明确挂载的窄模块，并先验证模块边界。
- 测试：`crates/ralph-core/src/event_loop/tests/termination.rs`、必要时 `event_policy.rs`/`chain_validation.rs`；真实场景位于 `crates/ralph-core/tests/scenarios/` 与 `scenarios.rs`。
- 配置/schema：不改字段、不改事件拓扑；仅读取当前 `event_policy.schemas[completion_promise]`。
- 数据：不改 ledger/outbox/JSONL wire format；只读取 workspace filesystem。
- CLI/API/UI：无直接接口变更；CLI emit 的既有 payload 经过 core completion gate。
- 外部服务：无。
- 调用方：所有实际使用产物字段的 completion promise，尤其 ce-executor pipeline、autoresearch、debug、implementation-review。
- 构建目标：`ralph-core`、依赖其行为的 `ralph-cli`，以及全 workspace。

### 3. 决策记录与置信度

| ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | 在哪一层阻止假终止？ | 修改通用 required-fields；修改 emit policy；在 `check_completion_event` honor 前检查 | 在 `check_completion_event` 中、清除 completion 状态前检查 completion schema 的产物字段 | E3、E4、E5 | 通用 rule 不知道 workspace；prompt 可被绕过；emit acceptance 早于最终语义且会误伤中间 artifact | 0.97 |
| D2 | 哪些字段触发 filesystem 校验？ | 只写死 `report_path`；所有 `*_path`；仅 completion schema 中的 `report_path`/`artifact_path` | 仅 completion promise schema 的 `report_path` 或 `artifact_path` | E2、E8 | 所有 `*_path` 会把中间报告变成同步 filesystem 契约；写死 topic 会误伤其他 preset | 0.95 |
| D3 | 路径有效性如何定义？ | 仅 `exists()`；仅 `is_file()`；工作区相对 + canonical 后在 workspace 内 + regular file 可读 | 最后一项；不要求非空 | E2 的 repo-relative/readable 文档、E7 临时 workspace、用户的 no-regression 约束 | 仅 exists 接受目录/escape；非空没有仓库证据且会增加回归 | 0.93 |
| D4 | 是否新增配置/依赖？ | 新增开关；增加 crate；复用 std path 与现有 config | 不新增配置、不新增依赖，复用现有 workspace/config/schema | E2、E4、E10、Cargo workspace 现状 | 新开关会允许再次产生假成功；新依赖无必要 | 0.96 |
| D5 | 测试层级？ | 只写 parser 单测；只写 CLI E2E；core termination 单测 + 真实 BDD + 全量回归 | 纯路径规则单测、completion 集成/termination 测试、至少一个真实 scenario、现有 preset lint/全量门禁 | E6、E7、AGENTS.md 测试规则 | 只测 parser 无法证明终止状态；只 E2E 成本高且难覆盖边界 | 0.94 |

没有低于 0.85 的关键决策。唯一待验证事项是已有 scenario 是否需要补 `fixture_files`；它不改变设计，进入 U1 先逐项确认。

### 8. Unit 串行依赖图

```text
Unit 1：终止产物校验规则与拒绝行为
  ↓
Unit 2：有效产物接入 completion honor，并覆盖 report/artifact 两种字段
  ↓
Unit 3：真实 preset/scenario 与旧路径回归
```

- U2 使用 U1 已验证的路径判定和拒绝语义；不能交换，因为没有先固定拒绝边界就无法判断有效 completion 的接入点。
- U3 使用 U2 已验证的 runtime gate；不能提前，因为 fixture 修改必须以真实失败结果为依据。
- 不允许 U1/U2 修改 preset 拓扑或提前放宽无 schema 路径；这些是 U3 的回归对象。

## Implementation Units

### 7. Unit 1：拒绝没有真实可读最终产物的 completion

#### 1. Unit 目标

让 completion schema 中的 `report_path`/`artifact_path` 具备可执行的 filesystem 证据语义：无效路径不能成为终止证据。

#### 2. 对应需求与 Scenario

- Requirement：R1 终止必须绑定真实产物。
- Scenario：S1-正常、S2-不存在、S3-目录、S4-工作区外/symlink escape。
- Decision：D1、D2、D3。
- Evidence：E2、E3、E4、E5。

#### 3. 外部可观察结果

completion payload 含无效产物路径时，`check_completion_event()` 不返回 `CompletionPromise`，`completion_honored` 不变为 true，并沿用既有 rejection/correction 机制。

#### 4. 当前行为基线

当前 `RequiredFieldsRule` 只验证字段存在；`check_completion_event` 没有文件检查。因此一个 JSON payload 只要含 `report_path` 且字段匹配，就可能进入终止尾部。先在 `termination.rs` 增加 characterization/acceptance Red，确认失败来自“当前实现错误地返回 CompletionPromise”，不是 fixture 或命令错误。

#### 5. 输入与输出

- 输入：completion topic、JSON payload、completion schema、workspace root。
- 输出：有效路径 `Ok/CompletionPromise`；无效路径返回既有拒绝结果。
- 错误：空、绝对、`..` escape、canonical escape、不存在、目录、不可读普通文件均拒绝。
- 状态：无效路径不得设置 `completion_honored`。
- 副作用：不得发布新的成功终止事件；允许既有 rejection/correction 状态更新。
- 不变量：无产物字段 schema 不走新检查；不改 required-field 通用 rule。

#### 6. 修改位置

- `crates/ralph-core/src/event_loop/wave_scope.rs::check_completion_event`：当前终止状态检查入口；增加 completion artifact gate，边界止于 honor 之前。
- `crates/ralph-core/src/event_loop/tests/termination.rs`：当前 completion payload/termination 测试位置；新增无效路径 Red/Green 与合法文件测试。
- 如 `wave_scope.rs` 内 helper 会造成职责或行数风险，才新增窄模块并在 `event_loop/mod.rs` 挂载；Executor 必须先确认新模块名称、可见性和行数，不能自行扩展到通用 validation crate。
- 明确不改：`rules_required_fields.rs`、preset YAML/schema、EventBus、outbox、`report_done_seen` 的通用记录逻辑。

#### 7. 可依赖能力

现有 `RalphConfig`、`EventLoop`、`CompletionPayloadMatchConfig`、`tempfile` workspace fixture、既有 rejection/correction 逻辑。

#### 8. 禁止依赖的未来能力

不得依赖 U2 的 preset fixture 调整；不得先改变 reporter prompt 或新增 CLI 校验；不得把中间 `report.done` admission 改成全局文件校验。

#### 9. 验收测试

- 前置：构造要求 `report_path` 的 completion schema，workspace 为 `TempDir`。
- 动作：分别提交不存在文件、目录、绝对路径、`../` 路径、指向 workspace 外的 symlink、workspace 内真实普通文件。
- 断言：前五项不返回 `CompletionPromise` 且 `completion_honored == false`；真实文件返回原有 completion reason。
- 副作用：无效路径不产生成功终止；schema 未含产物字段的旧测试仍通过。
- 运行：`cargo nextest run -p ralph-core -- termination`。

#### 10. Acceptance Red

先运行新增 `completion_artifact_*` acceptance tests。预期 Red 是：不存在路径/目录/escape case 当前错误返回 `Some(TerminationReason::CompletionPromise)`，或在调用链上暴露为 `completion_honored=true`。如果失败是 YAML parse、fixture 不存在、测试未执行或 unrelated compile error，不是有效 Red，必须停下修正测试。

#### 11. 单元测试拆分

1. `completion_artifact_accepts_workspace_relative_regular_file`：输入相对路径和真实文件，期望通过。
2. `completion_artifact_rejects_missing_file`：输入不存在路径，期望明确拒绝。
3. `completion_artifact_rejects_directory`：输入目录，期望拒绝。
4. `completion_artifact_rejects_absolute_and_parent_escape`：输入绝对/`..`，期望拒绝。
5. `completion_artifact_rejects_symlink_outside_workspace`：输入 workspace 内 symlink 指向外部文件，期望拒绝；若平台不支持 symlink，必须使用现有平台条件方式，不得跳过整个行为。
6. 不允许 mock `Path`/filesystem 判定；只 mock 不相关 EventBus observer。

#### 12. Red → Green → Refactor 顺序

`completion_artifact_rejects_missing_file` Red → 最小路径检查 Green → `completion_artifact_rejects_directory` Red → 补 regular-file 判定 Green → escape/symlink Red → canonical/workspace containment Green → valid-file Red/Green → 将错误映射接入 completion gate → Refactor helper/错误信息并重复全组测试。

#### 13. 最小实现范围

必须实现：从 completion schema 发现两个允许的产物字段之一；以 workspace 为根做相对路径、canonical containment、regular file/readability 判断；把失败接入既有 completion rejection。

不实现：内容解析、非空检查、文件 hash、所有中间事件校验、配置开关、CLI 重复实现。

#### 14. 集成验证

真实联合 `EventLoop::check_completion_event`、`LoopState` 和 config schema；filesystem 用 TempDir 真实目录；EventBus 只作观察器。命令：`cargo nextest run -p ralph-core -- termination event_policy chain_validation`。所有失败都阻止进入 U2。

#### 15. 风险驱动测试

- Characterization：无产物 schema 的既有 completion 测试，防止新 gate 变成全局 gate。
- Fault/permission：若 Unix 测试环境可稳定设置不可读文件，增加不可读 case；否则记录平台限制，不把 `exists()` 当成可读性证明。
- Security boundary：symlink escape 必测，因为只检查字符串前缀会绕过 workspace boundary。

#### 16. 回归范围

直接：termination、event_policy、chain_validation、completion payload match。

相邻：`crates/ralph-core/src/event_loop/event_processing.rs` prompt contract 测试；旧无 policy/空 required_events 路径。

公开消费者：`ralph-cli` emit/integration tests；ce-executor pipeline、autoresearch、debug、implementation-review 的 preset lint。

旧配置/默认关闭：没有 event policy 或没有产物字段的 completion 必须保持原行为；StateMachine、wave、merge 等无关功能不得被触碰。

构建/Lint：`cargo check --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`，最终 `./scripts/run-tests.sh`。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/wave_scope.rs` | 修改现有生产文件 | completion honor 前执行产物 gate | E4 |
| `crates/ralph-core/src/event_loop/tests/termination.rs` | 新增测试 | 覆盖路径边界与无副作用 | E6 |
| 可能的 `crates/ralph-core/src/event_loop/<窄 helper>.rs` | 仅在行数/职责检查要求时新增 | 从 wave_scope 抽取纯路径判定 | E10 |

#### 18. 完成标准

当前 Scenario、Unit 单测、termination 集成测试、相关回归、build/lint/typecheck 全通过；无 skip/only、无削弱断言；不改未来 Unit 行为；证据/决策更新；可以独立提交。

#### 19. 停止条件

若 completion 不是所有路径都经过 `check_completion_event`、schema 结构实际不同、Red 不是目标失败、需要新增依赖、无产物旧测试被意外拒绝、或决定置信度低于 0.85，停止并更新 Evidence/Decision，不得绕过。

#### 20. 风险与注意事项

- symlink canonicalization 在不同平台行为不同；检测：Unix/CI targeted tests；缓解：用标准 Path canonicalization，不做字符串前缀判断；剩余风险是 Windows symlink 权限差异。
- 场景报告路径可能只是 mock payload；检测：逐文件核对 `fixture_files`；缓解：补真实 fixture，不放宽 gate。
- `completion_requested` 可能在 gate 前已由事件处理设置；检测：断言最终 `completion_honored` 和 termination reason，而不是只断言 seen topic；缓解：拒绝路径只进入既有 correction。

### Unit 2：有效产物接入 completion honor，并覆盖两种字段

1. Unit 目标：当 schema 使用 `report_path` 或 `artifact_path` 且目标文件真实可读时，completion 保持成功。
2. 对应：R1、S1、Decision D2/D3、Evidence E2/E8。
3. 外部结果：ce-executor、autoresearch、debug、implementation-review 的合法终止不再被误拒绝。
4. 基线：当前无 filesystem 检查，合法 payload 会完成；U1 后必须证明仍完成。
5. 输入输出：workspace 内真实文件 + completion payload；输出原 CompletionPromise；无新增 outbox/preset 状态。
6. 修改位置：`crates/ralph-core/src/event_loop/tests/termination.rs`；若 BDD payload 需要真实文件，修改对应 `crates/ralph-core/tests/scenarios/*.yml` 的 `fixture_files`。不改 schema 字段。
7. 依赖：U1 已验证 helper/gate；现有 scenario fixture harness。
8. 禁止：不得把“文件不存在”的 scenario 改成 expected success；不得把 artifact path 映射成 report path。
9. 测试：分别以两个字段构造合法 completion；断言 reason、honored、payload match、不产生 rejection；运行 `cargo nextest run -p ralph-core -- termination` 与对应 scenario 测试。
10. Red：先运行合法 completion；若 U1 改动错误，会看到合法文件被拒绝；该 Red 才能证明兼容行为覆盖。fixture/compile 失败无效。
11. 单测：`report_path` 合法、`artifact_path` 合法、相对路径规范化后仍在 workspace、completion payload match 与真实文件同时满足。
12. 顺序：report 合法 Red→Green；artifact 合法 Red→Green；合法路径+payload match Red→Green；Refactor 后全组。
13. 最小实现：只修复 U1 接入时的 schema field selection/错误映射；不扩展 `*_path`。
14. 集成：真实 EventLoop + TempDir；不 mock completion check；命令 `cargo nextest run -p ralph-core -- termination chain_validation`。
15. 风险测试：BDD fixture 是必要风险测试，因为现有 scenario runner 会把 workspace_root 重设到 TempDir。
16. 回归：ce-executor pipeline/loop、autoresearch/debug completion；无产物 schema 继续由 U3 负责全面回归。
17. 预期文件：termination tests；实际缺 fixture 的 scenario YAML；必要时 scenario expected 只补文件副作用，不改事件期望。
18. 完成：所有合法路径通过且无行为拓扑变化。
19. 停止：发现某 preset 的 completion promise 不是当前 schema 结构或 fixture 依赖未确认，先记录证据。
20. 风险：若 scenario 使用相对路径但文件由 mock agent 本应运行时创建，不能静态 fixture 掩盖真实流程；应改用真实 runtime producer 或把场景标为非 completion-artifact 场景并排除。

### Unit 3：其他 preset 与无产物 completion 的回归闭环

1. Unit 目标：证明新 gate 只影响有终止产物契约的 completion，不破坏其他 preset/功能。
2. 对应：R2 兼容性、S5、Decision D1/D2/D4、Evidence E7/E8/E10。
3. 外部结果：hatless、merge、post-merge、red-team、parallel-forge 等不要求 completion 产物的路径继续按原契约完成。
4. 基线：`event_policy`/`chain_validation` 已有无 policy/空 required-events characterization。
5. 输入输出：旧配置、无产物 schema、StateMachine disabled/projection none；输出与 baseline 相同。
6. 修改位置：只在受影响 scenario fixture 与必要测试文件；不修改 `presets/en/*.yml`、schema、manifest、zsh completion。
7. 依赖：U1/U2 已通过。
8. 禁止：不得通过删除已有断言、跳过场景、改 completion promise 来绿测试。
9. 验收：`cargo nextest run -p ralph-core -- event_policy chain_validation payload_types`；`cargo nextest run -p ralph-cli --bin ralph -- preset_lint presets`；全量 scenario 通过。
10. Red：在 U1/U2 后先跑旧测试；任何无产物 completion 被拒绝都是有效回归 Red，必须修复 gate 条件而不是更新 expected。
11. 单测：无 schema path passthrough、旧 `LOOP_COMPLETE` string payload、persistent mode、completion mismatch、required event rejection。
12. 顺序：无产物 passthrough Red/Green→旧 completion rejection Red/Green→preset scenario Red/Green→Refactor/全量。
13. 最小实现：只允许调整 gate 条件/错误映射；不得新增长期兼容分支。
14. 集成：真实 core scenario、CLI preset lint、CLI integration emit。
15. 风险：跨 preset schema parity；用真实 preset lint 与 BDD，而非字符串测试。
16. 回归：所有 builtin preset lint、`ralph-cli` preset/emit tests、`ralph-core` full nextest、build/clippy/docs drift。
17. 预期文件：仅必要 fixture/test；若 skill 文档描述“runtime 会验证”与现状不同，更新 `crates/ralph-core/data/*.md`，否则不改文档。
18. 完成：全量质量门禁通过，无其他 preset 失败。
19. 停止：任何 unrelated preset 失败、schema/preset parity drift、或需要修改 preset topology，停止并重新评估范围。
20. 风险：历史 scenario 依赖不存在的 artifact 只是当前 mock 缺陷；修复 fixture必须保持事件断言和生产路径真实。

## Verification Contract

### 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
|---|---|---|---|---|---|
| S1 | 真实 workspace 普通文件允许 CompletionPromise，honored=true | `event_loop/tests/termination.rs` | 集成/单元组合 | report/artifact 双字段 | 否 |
| S2-S4 | 无效路径不 honor、不产生成功副作用 | 同上 | 集成 + filesystem boundary | symlink/escape | 否 |
| S5 | 无产物 schema 旧 completion 仍通过 | `event_policy.rs`、`chain_validation.rs` | characterization | 默认关闭路径 | 否 |
| S1-S5 真实 preset | scenario 事件、终止 reason、fixture 副作用一致 | `tests/scenarios.rs` + scenario YAML | BDD integration | fixture workspace | 否，现有 scenario 足够 |

所有测试都必须断言：返回 reason/None、`completion_honored`、rejection/correction 必要状态、无新成功发布；不能只断言字符串。

### 6. 需求—测试追踪矩阵

| Requirement | 需求 | Scenario | 验收测试 | 单元 | 集成/契约 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | 有产物契约的终止必须有真实可读文件 | S1-S4 | `completion_artifact_*` | 路径规则测试 | `termination.rs` | 否 | E2-E5 |
| R2 | 无产物旧路径不变 | S5 | existing completion characterization | passthrough | `event_policy`/`chain_validation` | 否 | E6-E8 |
| R3 | 其他 preset 不被误伤 | S1-S5 | scenario expected events/termination | schema selection | preset lint + full scenario | 否 | E7-E10 |

## Definition of Done

### 9. 执行命令清单

- Unit 1 Red/Green：`cargo nextest run -p ralph-core -- termination`；失败不得进入下一步。
- Unit 1 相邻回归：`cargo nextest run -p ralph-core -- termination event_policy chain_validation`。
- Unit 2 BDD：先确认 `crates/ralph-core/tests/scenarios.rs` 的过滤方式，再按仓库实际支持的 scenario 过滤命令执行；不得改用 stub runner。
- CLI contract：`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`、`cargo nextest run -p ralph-cli --bin ralph -- presets`。
- Build：`cargo build --workspace`；失败不得继续。
- Typecheck：本 Rust workspace 以 `cargo check --workspace` 为类型检查入口。
- Lint：`cargo clippy --workspace --all-targets --all-features -- -D warnings`。
- 文档 drift：若修改 `crates/ralph-core/data/*.md`，运行 `scripts/check-cli-doc-drift.sh`。
- 最终全量：`./scripts/run-tests.sh`；禁止裸跑 `cargo test`。

### 10. 最终质量门禁

所有 S1-S5 通过；无 skip/only、无削弱断言、无无解释 snapshot/golden；所有相关 builtin preset lint、scenario、CLI integration、build、check、clippy、全量 nextest 通过；没有引入新配置/依赖；没有修改无产物 completion 的行为；实际变更不超出本计划；所有决策置信度仍 ≥0.85。

### 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap | 是 | 有真实入口、Red、最小边界和命令 |
| Executor 是否仍需做关键设计决策 | 否 | D1-D5 已定；fixture 只需按 U1 证据核对 |
| 所有文件和接口是否有代码库证据 | 是 | E2-E10；候选 helper 明确标为条件新增 |
| 所有关键决策置信度是否 ≥0.85 | 是 | D1-D5 为 0.93-0.97 |
| 是否存在未处理的低置信度假设 | 否 | fixture 假设已设为 U1 前置调查，不依赖猜测 |
| 每个 Unit 是否只有一个可观察行为 | 是 | 拒绝、允许、兼容分别拆开 |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 有 Red/命令/回归 |
| 每个 Unit 是否有真实 Red | 是 | 明确预期错误行为与无效 Red |
| 每个 Unit 是否包含回归范围 | 是 | 第 16 节逐 Unit 列出 |
| 是否存在未来 Unit 依赖 | 否 | 仅线性依赖已验证能力 |
| 是否存在泛化任务描述 | 否 | 每项绑定文件、函数、断言和命令 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 第 5、6 节 |
| 所有关键决策是否有 Evidence | 是 | 第 2.2、3 节 |
| 计划是否可以严格串行执行 | 是 | 第 8 节 |

