---
title: Payload 契约强制校验与运行时诊断系统
type: feat
status: active
date: 2026-06-02
reviewed_at: 2026-06-03
origin: docs/brainstorms/2026-06-02-payload-contract-validation-requirements.md
related:
  - docs/plans/2026-06-03-001-feat-agent-execution-contract-gates-plan.md
---

# Payload 契约强制校验与运行时诊断系统

## Review 结论

2026-06-03 代码更新后，仓库已经落地了一条 **Agent 执行契约门控** 路线：missing-event hard gate、`event_loop.execution_contracts`、`execution_contract.rs`、ce-executor `work.done` contract 等。这些能力能防止 executor 忘记 emit、假 `work.done` 推进 review、task 未关闭仍完成等问题。

但这条路线**不能替代**本计划要求的 payload schema contract。当前代码仍缺少跨 hat payload 契约的编排期校验和运行时 schema pause 机制。因此，本计划继续保持 active，后续必须补齐原定主线：

- `schema_file` 外部 schema 文件支持。
- instructions payload 字段引用静态提取。
- `ralph hats validate --strict` payload contract gate。
- `ralph run` 启动前不可跳过的 payload contract hard gate。
- runtime payload schema violation 终止 loop，并生成结构化诊断报告。

`execution_contracts` 是互补能力：它验证 agent 完成声明是否真实；本计划验证事件 payload shape 是否满足上下游 hat 契约。两者必须共存，不能互相替代。

## 当前代码事实

### 已落地的互补能力

- `crates/ralph-core/src/config.rs` 已新增 `ExecutionContractsConfig`、`ExecutionContractRule`、`TaskCompletionRequirement`、`GitChangeRequirement`、`TestEvidenceRequirement` 和 `ContractRejectConfig`。
- `EventLoopConfig` 已支持 `execution_contracts: Option<ExecutionContractsConfig>`，默认 opt-in。
- `crates/ralph-core/src/execution_contract.rs` 已实现 `work.done` 等 topic 的执行契约验证：
  - required payload fields
  - task id / task key 字段验证
  - loop-scoped task 校验
  - task terminal status 校验
  - git diff 或 loop-start 之后 commit evidence 校验
  - optional / required payload field test evidence 校验
- `crates/ralph-core/src/event_loop/mod.rs` 已在事件 publish 到 bus 前调用 execution contract validator。被 execution contract 拒绝的原事件不会进入 bus。
- `crates/ralph-cli/src/loop_runner.rs` 已处理 execution contract rejection：记录诊断，但不终止 loop，依靠 `human.guidance` 驱动下一轮修复。
- missing-event hard gate 已升级：即使 agent 输出中没有提到 `ralph emit`，只要当前 hat 有 `publishes` 且没有 `default_publishes`，本轮无事件也会触发 gate。
- `presets/ce-executor.yml` 已移除 executor 的 `default_publishes`，并显式启用 `event_loop.execution_contracts.rules.work.done`。

### 仍未落地但必须完成

- `EventPolicyConfig` 没有 `schema_file` 字段。
- 没有 `RalphConfig::resolve_schema_files(base_path)`。
- 没有 `HatConfig.ignore_payload_fields`。
- 没有 `crates/ralph-core/src/payload_contract.rs`。
- 没有 instructions payload 字段提取器。
- 没有 payload contract validator。
- 没有 `ralph hats validate --strict`。
- `ralph hats validate` 默认仍只走拓扑校验，没有 payload contract 校验。
- `ralph run` 启动前没有不可跳过的 payload contract hard gate。
- runtime schema violation 仍走现有 event policy 路径，没有 `TerminationReason::PayloadContractViolation`。
- 没有 `.ralph/diagnostics/payload-contract-error-*.json` 结构化报告。
- `EventPolicyConfig::default()` 没有切换为 `enabled: true, mode: Enforce`。
- ce-executor / ce-executor-zh 还没有外部 schema 或等价 embedded schema 镜像。
- 全量 builtin/root/minimal/中文 preset 还没有 payload schema 适配矩阵。

## Problem Frame

`ce-executor.yml` 等 preset 的 instructions 中存在大量隐式 payload 字段约定，例如从上游事件 payload 读取 `dimension`、`focus`、`depth`、`plan_name`、`task_id`、`task_key`、`step` 等字段。当前 execution contract 只覆盖少量完成声明主题，无法证明任意发布者的 payload 满足任意订阅者的字段依赖。

系统仍缺三层能力：

1. **编排期校验缺失**：`ralph run` 启动前必须检查 hat A 发布的 payload schema 是否满足 hat B instructions 中声明的 payload 字段依赖。
2. **Schema 配置缺失**：现有 `event_policy.schemas` 能力没有外部 schema 文件支持，也没有在 builtin presets 中系统启用。
3. **运行时诊断缺失**：payload 字段缺失或类型错误时，必须明确指出 topic、字段、source hat、target hat、schema 来源和修复建议，而不是让 agent 黑盒失败。

## Requirements Trace

| 需求 | 实现单元 |
|---|---|
| R1 外部 schema 文件支持 | U1 |
| R2 instructions payload 字段静态提取 | U2 |
| R3 编排期 payload contract validator | U3 |
| R4 `ralph hats validate --strict` 与默认 validate 集成 | U4 |
| R5 `ralph run` 启动前不可跳过 hard gate | U5 |
| R6 runtime payload schema violation 终止 loop + 诊断报告 | U6 |
| R7 ce-executor / ce-executor-zh schema 适配 | U7 |
| R8 全量 preset 兼容性矩阵 | U8 |
| R9 用户与 preset 作者文档 | U9 |

## Non-Regression Guardrails

- payload contract gate 是必做主线，不得被 execution contract 替代。
- `execution_contracts` 继续保留，用于验证 `work.done` 完成真实性；payload contract 用于验证事件字段 shape 和上下游契约。
- `ralph run` 启动前 payload contract gate 必须不可跳过。不提供 `--skip-payload-check`、环境变量绕过或 fallback 到 warn-only。
- `ralph hats validate` 默认执行拓扑校验 + payload contract 校验。`--strict` 额外强制 schema 完备性。
- 默认 validate 若发现 instructions 提取到 payload 字段但缺 schema，必须报 error。只有无 payload 字段引用的缺 schema 才能是 warning。
- hatless/solo 模式必须定义明确 pass 条件：没有自定义 hats 或没有 topic/payload 契约时，返回“无可校验契约，pass”。
- `EventPolicyConfig::default()` 最终必须切换为 `enabled: true, mode: Enforce`。切换前必须完成 preset 适配矩阵。
- runtime payload schema violation 必须终止 loop，并生成 payload contract 专用诊断报告。execution contract rejection 当前不终止 loop，这两类行为不能混淆。
- 诊断报告写入失败不能吞掉原始 violation；终端仍必须输出摘要并以非零状态结束。
- root preset、embedded preset、中文 preset 必须同步，不能只修 root 文件。

## Implementation Units

- [ ] U1. **外部 Schema 文件加载与配置扩展**

  **Goal:** 让 `EventPolicyConfig` 支持引用外部 schema 文件，并在加载 preset 时合并到 `schemas`。

  **Files:**
  - Modify: `crates/ralph-core/src/config.rs`
  - Modify if needed: `crates/ralph-cli/src/presets.rs`
  - Test: `crates/ralph-core/src/config.rs`

  **Approach:**
  - `EventPolicyConfig` 新增 `schema_file: Option<String>`，serde default。
  - 新增 `RalphConfig::resolve_schema_files(&mut self, base_path: &Path)`。
  - 相对路径基于 preset 文件所在目录解析。
  - 合并策略固定为内联 schema 优先于文件 schema。
  - 文件不存在、格式非法、schema 不是 map 时必须返回清晰错误。
  - embedded preset 不能依赖 root repo 路径。必须选择以下之一：
    - 将 schema inline 镜像进 embedded preset。
    - 或新增 embedded schema registry，并同步 `crates/ralph-cli/presets/...` schema 文件。
  - 未解决 embedded schema 解析前，不得在 public builtin preset 上启用 `schema_file`。

  **Verification:**
  - `rtk cargo test -p ralph-core config::tests::test_schema_file -- --nocapture`
  - `rtk cargo test -p ralph-cli presets::tests -- --nocapture`

- [ ] U2. **Instructions Payload 字段引用提取器**

  **Goal:** 从 hat instructions 中保守提取 payload 字段依赖，并提供行号上下文。

  **Files:**
  - Create: `crates/ralph-core/src/payload_contract.rs`
  - Modify: `crates/ralph-core/src/lib.rs`
  - Modify: `crates/ralph-core/src/config.rs`
  - Test: `crates/ralph-core/src/payload_contract.rs`

  **Approach:**
  - 新增 `HatConfig.ignore_payload_fields: Vec<String>`，serde default。
  - 新增提取结果结构，至少包含：
    - `hat_id`
    - `field`
    - `line`
    - `pattern`
    - `source_excerpt`
  - 提取规则必须保守，只处理明确提到 payload 的文本：
    - `From event payload: task_id, plan_name`
    - `payload MUST include: task_id, task_key`
    - 含 payload 的行内反引号字段
  - 去重并保持稳定排序。
  - `ignore_payload_fields` 只用于静态 validator 排除误报，不影响 runtime event policy。

  **Verification:**
  - `rtk cargo test -p ralph-core payload_contract::tests -- --nocapture`

- [ ] U3. **Payload Contract Validator**

  **Goal:** 校验每个 target hat 的 payload 字段依赖是否被上游 topic schema 覆盖。

  **Files:**
  - Modify: `crates/ralph-core/src/payload_contract.rs`
  - Modify: `crates/ralph-core/src/preset_validator.rs`
  - Test: `crates/ralph-core/src/payload_contract.rs`

  **Approach:**
  - 输入 `RalphConfig`、hat registry/topology、schema map。
  - 对每个 hat trigger topic：
    - 提取该 hat instructions 中依赖的 payload fields。
    - 找到 trigger topic 的 schema。
    - 若字段被 instructions 依赖但 schema 未声明 required field，报 error。
    - 若字段类型 schema 缺失，在 strict 模式报 error，默认模式可 warning。
  - 对无 payload field 引用的 topic，不强制 schema。
  - source hat 如果多个发布同一 topic，诊断必须列出全部候选，不得猜一个。

  **Verification:**
  - `rtk cargo test -p ralph-core payload_contract::tests -- --nocapture`
  - `rtk cargo test -p ralph-core preset_validator::tests -- --nocapture`

- [ ] U4. **`ralph hats validate --strict` 集成**

  **Goal:** CLI 层暴露 payload contract validation。

  **Files:**
  - Modify: `crates/ralph-cli/src/hats.rs`
  - Modify: `crates/ralph-core/src/preset_validator.rs`
  - Test: `crates/ralph-cli/src/hats.rs`

  **Approach:**
  - `ralph hats validate` 默认执行拓扑 + payload contract 校验。
  - 新增 `--strict`：
    - 所有被 trigger 引用且有 payload 字段依赖的 topic 必须有 schema。
    - 所有 schema required fields 必须可用于 runtime event policy。
  - 输出必须包含 preset 路径、hat id、topic、field、schema 来源、instructions 行号。

  **Verification:**
  - `rtk cargo test -p ralph-cli hats -- --nocapture`
  - `rtk cargo run -p ralph-cli -- hats validate --help`

- [ ] U5. **`ralph run` 启动前 Hard Gate**

  **Goal:** 任何带 hat workflow 的 `ralph run` 在启动 agent 前必须通过 payload contract gate。

  **Files:**
  - Modify: `crates/ralph-cli/src/loop_runner.rs`
  - Modify if needed: `crates/ralph-cli/src/main.rs`
  - Test: `crates/ralph-cli/src/loop_runner.rs`

  **Approach:**
  - preset/config 加载后、agent 启动前调用 payload contract validator。
  - validator 有 error 时：
    - 不启动 backend。
    - 终端输出可操作错误。
    - 返回非零退出。
  - 不提供 skip 参数。
  - hatless/solo 无契约时 pass。

  **Verification:**
  - 构造缺 schema fixture，确认 `ralph run --dry-run` 或等价路径失败且不启动 agent。
  - `rtk cargo test -p ralph-cli payload_contract -- --nocapture`

- [ ] U6. **Runtime Payload Violation Loop Pause 与诊断报告**

  **Goal:** event policy schema violation 在 enforce 模式下终止 loop，并生成结构化诊断报告。

  **Files:**
  - Modify: `crates/ralph-core/src/event_policy.rs`
  - Modify: `crates/ralph-core/src/event_loop/mod.rs`
  - Modify: `crates/ralph-cli/src/loop_runner.rs`
  - Test: `crates/ralph-core/src/event_loop/tests.rs`

  **Approach:**
  - 新增 payload schema violation 结构，区分：
    - missing required field
    - payload type mismatch
    - allowed value mismatch
  - `ProcessedEvents` 新增 `payload_contract_violation: Option<...>`。
  - `TerminationReason` 新增 `PayloadContractViolation`。
  - loop runner 检测到 violation 后：
    - 写入 `.ralph/diagnostics/payload-contract-error-{timestamp}.json`
    - 终端输出 `[PAYLOAD CONTRACT VIOLATION] Loop paused.`
    - 通过正常 termination hook 流程终止 loop。
  - 诊断报告至少包含：
    - `error_type`
    - `timestamp`
    - `topic`
    - `field`
    - `source_hat[]`
    - `target_hat`
    - `schema_defined_in`
    - `downstream_reference`
    - `upstream_reference`
    - `fix_hint`

  **Verification:**
  - `rtk cargo test -p ralph-core event_policy -- --nocapture`
  - `rtk cargo test -p ralph-core event_loop::tests::test_payload_contract -- --nocapture`

- [ ] U7. **ce-executor / ce-executor-zh Schema 适配**

  **Goal:** 为 ce-executor 全链路补齐 payload schema，并处理 embedded 镜像。

  **Files:**
  - Create: `presets/ce-executor/schemas.yml`
  - Modify: `presets/ce-executor.yml`
  - Modify: `presets/ce-executor-zh.yml`
  - Modify if embedded registry: `crates/ralph-cli/presets/ce-executor/schemas.yml`
  - Modify if needed: `scripts/sync-embedded-files.sh`

  **Approach:**
  - 分析 ce-executor 所有 triggers、publishes、instructions payload 引用。
  - 为 `work.ready`、`queue.advance`、`work.done`、`review.wave.ready`、`review.dimension.done`、`review.wave.done`、`review.passed`、`review.failed`、`plan.complete`、`report.done` 等 topic 建 schema。
  - 英文和中文 preset 使用同一契约。
  - 同步 root 与 embedded preset。

  **Verification:**
  - `rtk proxy ./scripts/sync-embedded-files.sh check`
  - `rtk cargo run -p ralph-cli -- hats validate --strict -p presets/ce-executor.yml`
  - `rtk cargo run -p ralph-cli -- hats validate --strict -p presets/ce-executor-zh.yml`

- [ ] U8. **全量 Preset 兼容性审计矩阵**

  **Goal:** 强制开启前审计所有 builtin/root/minimal/中文 preset。

  **Files:**
  - Modify: 本计划文档或新增实施报告
  - Modify if needed: `presets/*.yml`
  - Modify if needed: `presets/*-zh.yml`
  - Modify if needed: `presets/minimal/*.yml`

  **Matrix columns:**

  | Preset | Embedded | Payload refs | Trigger topics needing schema | Schema source | Strategy | Validation command |
  |---|---|---|---|---|---|---|

  **Required coverage:**
  - public builtin: `autoresearch`、`ce-executor`、`code-assist`、`debug`、`pdd-to-code-assist`、`research`、`review`
  - hidden builtin: `hatless-baseline`、`merge-loop`
  - root-only / non-embedded: `harness-demo`、`wave-review`、`ralph.reviewer`、`minimal/*`
  - all `*-zh.yml`

  **Verification:**
  - 对所有 public builtin preset 跑默认 validate。
  - 对所有启用 schema 或含 payload 字段引用的 preset 跑 strict validate。

- [ ] U9. **文档、迁移说明与工具说明更新**

  **Goal:** 让用户和 preset 作者知道如何写 schema、如何 validate、如何排障。

  **Files:**
  - Modify: `docs/guide/harness-extensions.md`
  - Create or modify: `docs/guide/payload-contracts.md`
  - Modify: `presets/COLLECTION.md`
  - Modify if applicable: `crates/ralph-core/data/ralph-tools.md`

  **Required content:**
  - `event_policy.schema_file` 示例。
  - inline schema 与 external schema 合并策略。
  - `ralph hats validate` 和 `--strict` 的区别。
  - `ralph run` hard gate 不可跳过。
  - payload violation 诊断报告字段说明。
  - execution contract 与 payload contract 的边界。
  - 中文 preset 与英文 preset 的契约同步要求。
  - embedded preset/schema 同步要求。

  **Verification:**
  - `rtk cargo run -p ralph-cli -- hats validate --help`
  - 若修改 `crates/ralph-core/data/*.md`，按 AGENTS.md 做源码行号反向验证。

## Sequencing

1. **先建 schema 加载能力：** U1。
2. **建立静态提取和 validator：** U2 + U3。
3. **接入 CLI validate 和 run hard gate：** U4 + U5。
4. **接入 runtime pause 和诊断报告：** U6。
5. **适配 ce-executor 和 embedded 镜像：** U7。
6. **审计所有 preset：** U8。
7. **补齐文档与迁移说明：** U9。
8. **最后才切换默认 enforce：** 在 U7/U8 验证通过前，不得把默认强制开启推给所有 preset。

## Test Matrix

| Area | Command |
|---|---|
| Schema file parsing | `rtk cargo test -p ralph-core config::tests::test_schema_file -- --nocapture` |
| Payload extraction | `rtk cargo test -p ralph-core payload_contract::tests -- --nocapture` |
| Preset validator | `rtk cargo test -p ralph-core preset_validator::tests -- --nocapture` |
| CLI hats validate | `rtk cargo test -p ralph-cli hats -- --nocapture` |
| Runtime event policy | `rtk cargo test -p ralph-core event_policy -- --nocapture` |
| Runtime loop pause | `rtk cargo test -p ralph-core event_loop::tests::test_payload_contract -- --nocapture` |
| ce-executor strict validate | `rtk cargo run -p ralph-cli -- hats validate --strict -p presets/ce-executor.yml` |
| zh strict validate | `rtk cargo run -p ralph-cli -- hats validate --strict -p presets/ce-executor-zh.yml` |
| Embedded drift | `rtk proxy ./scripts/sync-embedded-files.sh check` |
| Full gate | `rtk proxy ./scripts/run-tests.sh` |

## Acceptance Criteria

- `schema_file` 能加载并合并 schema，内联 schema 优先。
- instructions payload 字段提取器能给出字段、hat、行号和原始片段。
- `ralph hats validate` 默认执行 payload contract 校验。
- `ralph hats validate --strict` 对缺 schema / 缺 required field 非零退出。
- `ralph run` 启动前发现 payload contract error 时，不启动 backend。
- runtime payload schema violation 会终止 loop，并写入 `.ralph/diagnostics/payload-contract-error-*.json`。
- ce-executor / ce-executor-zh strict validate 通过。
- 所有 builtin/root/minimal/中文 preset 都有适配矩阵结论。
- execution contract 继续保留，并与 payload contract 边界清晰。

## Sources & References

- Origin document: [docs/brainstorms/2026-06-02-payload-contract-validation-requirements.md](docs/brainstorms/2026-06-02-payload-contract-validation-requirements.md)
- Related execution contract plan: [docs/plans/2026-06-03-001-feat-agent-execution-contract-gates-plan.md](docs/plans/2026-06-03-001-feat-agent-execution-contract-gates-plan.md)
- Current related implementation:
  - `crates/ralph-core/src/config.rs`
  - `crates/ralph-core/src/event_policy.rs`
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/execution_contract.rs`
  - `crates/ralph-core/src/preset_validator.rs`
  - `crates/ralph-cli/src/hats.rs`
  - `crates/ralph-cli/src/loop_runner.rs`
  - `presets/ce-executor.yml`
