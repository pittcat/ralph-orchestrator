---
title: Payload 契约强制校验与运行时诊断系统
type: feat
status: active
date: 2026-06-02
origin: docs/brainstorms/2026-06-02-payload-contract-validation-requirements.md
---

# Payload 契约强制校验与运行时诊断系统

## Overview

为 Ralph Orchestrator 引入双重防护机制：编排阶段强制校验 preset 的跨 hat payload 契约，运行时严格 enforce payload schema，出错时立即暂停 loop 并生成结构化诊断报告。解决当前 preset 中大量隐式 payload 约定无法被机器校验、运行时字段缺失只能黑盒排查的问题。

核心变更：
- 支持外部 schema 文件引用，避免 preset YAML 臃肿
- `ralph run` 启动前自动执行 payload 契约硬门槛，不提供 skip 参数
- 运行时 `event_policy` 默认启用且默认 enforce；payload schema 违规触发 Loop Pause
- 诊断报告提供 topic、字段、source/target hat、schema 来源和可操作修复提示；具备行号上下文时精确到文件行号

---

## Problem Frame

`ce-executor.yml` 等 preset 的 instructions 中存在大量隐式 payload 字段约定（如 `"From event payload: dimension, focus, depth, plan_name, task_id..."`），但系统缺乏三层能力：

1. **无编排期校验**：`ralph run` 启动前仅检查拓扑连通性（`preset_validator.rs`），不检查 hat A 发布的 payload 字段是否满足 hat B 的 instructions 依赖。
2. **Schema 能力闲置**：`event_policy.schemas` 和 `event_policy.rs` 的运行时校验已存在，但所有内置 preset 均未配置 schema，且默认 `mode: Observe`（只警告不阻止）。
3. **运行时错误模糊**：字段缺失时 agent 行为混乱，没有结构化错误报告指明"哪个 hat、哪个事件、缺哪个字段"。

(see origin: docs/brainstorms/2026-06-02-payload-contract-validation-requirements.md)

---

## Requirements Trace

| 需求 | 实现单元 |
|------|---------|
| R1 外部 Schema 文件支持 | U1 |
| R2 编排期强制校验（Preset Gate） | U3, U4 |
| R3 Instructions 字段引用静态分析 | U2, U3 |
| R4 运行时严格校验（Runtime Guard） | U5 |
| R5 Loop 暂停与诊断报告 | U5 |
| R6 Schema 完备性强制 | U3, U4 |

**Success criteria covered:**
- `ralph run` 启动前拦截 payload 契约问题 → U4
- `ce-executor.yml` schema 外置 → U1, U6
- 运行时缺字段 pause loop + JSON 诊断报告 → U5
- 诊断报告 1 分钟定位 → U5
- 强制开启但不引入回归：所有现有 preset 必须补齐 schema 或显式标注无 payload 依赖，升级后 `ralph run` 仍可正常启动 → U6, U7
- `ralph hats validate --strict` 非零退出 → U4
- 不引入回归：现有 preset、用户配置、hatless/solo 模式必须通过强制 gate 的适配路径继续可用 → U4, U7
- 不破坏现有功能：现有拓扑校验、origin guard、state machine、workflow guard、wave 路径和 completion 流程必须继续通过既有测试 → U5, U7
- preset 适配完整性：所有内置 public preset、中文镜像 preset、embedded preset 镜像和 zsh completion 影响必须被审计并记录 → U6, U7
- 文档更新跟随实现：用户指南、preset 作者指南、诊断报告格式、迁移说明和 embedded preset 同步说明必须随功能一起更新 → U8

---

## Scope Boundaries

- **In scope:** 外部 schema 文件加载、编排期 payload 契约静态校验、instructions 字段启发式提取、运行时 enforce 默认启用、Loop Pause 与诊断报告生成。
- **Out of scope:** 自动修复 preset、NLP 语义理解、payload 值业务含义校验、已有事件文件 retroactive 修复、wave 子事件特殊契约逻辑。

### Non-Regression Guardrails

- `ralph run` 的 payload contract gate 是强制开启的启动前检查。任何带 hat workflow 的运行都必须经过该 gate，不提供 `--skip-payload-check`、环境变量绕过或 fallback 到 warn-only。
- 强制开启不等于允许回归。所有现有 builtin/root preset 必须在本计划内完成适配：要么补齐 schema，要么由校验器证明该 preset 没有可提取的 payload 字段依赖且写入适配矩阵。不能用“未启用 event_policy”作为绕过理由。
- hatless/solo 模式必须定义明确行为：没有自定义 hats 或没有 topic/payload 契约时，gate 应返回“无可校验契约，pass”，而不是因为缺 schema 失败。
- `ralph hats validate` 默认执行拓扑校验 + payload contract 校验；`--strict` 额外强制 schema 完备性（所有被 triggers 引用的 topic 都必须有 schema）。默认 validate 若发现已提取 payload 字段但缺 schema，必须报 error；只有“无 payload 字段引用”的缺 schema 才可保持 warning。
- `EventPolicyConfig::default()` 必须改为 `enabled: true, mode: Enforce`，并新增测试证明默认 runtime guard 生效。显式 `event_policy.enabled: false` 是否允许作为用户配置逃生舱需在实现前确认；本计划默认不把它作为 builtin preset 的绕过路径。
- 运行时 payload contract violation 只能覆盖 schema 相关违规（missing required field、payload type mismatch、allowed value mismatch）。非 schema 的 completion-after-terminal、origin guard、state machine、workflow guard 违规继续走原有路径和 `on_violation` 行为。
- 诊断报告写入失败不得吞掉原始 contract violation；终端必须仍显示可操作错误摘要并以非零状态结束。
- 所有实现必须以“先适配 preset，再打开强制 gate”为顺序。不能先改默认强制开启再留下任何 builtin preset 无法运行。

### Deferred to Follow-Up Work

- `ralph preset fix` 自动修复命令：诊断报告格式已预留自动化消费接口，但人工修复是本次首选路径。
- Instructions 字段提取的 NLP 增强：当前采用启发式文本模式，后续可引入更智能的提取器。
- `ralph preset fix` 自动生成 schema：本次只要求手写 schema 和诊断报告，不实现自动修复。

(see origin: docs/brainstorms/2026-06-02-payload-contract-validation-requirements.md)

---

## Context & Research

### Relevant Code and Patterns

- **`crates/ralph-core/src/config.rs`** — `EventPolicyConfig`（`enabled`, `mode`, `on_violation`, `schemas`），`EventSchema`（`payload`, `required_fields`, `allowed_values`），`EventPolicyMode`（`Observe`/`Enforce`）。当前 `EventPolicyConfig::default()` 为 `enabled: false, mode: Observe`。
- **`crates/ralph-core/src/preset_validator.rs`** — `validate_preset_topology` 构建 topic-hat 二分图，BFS 检查可达性。返回 `TopologyValidationResult`（`errors` + `warnings`）。现有 ~40 个内联测试覆盖拓扑场景。尚无 payload 校验逻辑。
- **`crates/ralph-core/src/event_policy.rs`** — `validate_event` 返回 `PolicyDecision`（`Accept`/`Warn`/`RejectWithResume`/`Hold`/`Block`/`Ignore`）。`Enforce` 模式下根据 `on_violation` 决定后续动作。现有 ~30 个内联测试。
- **`crates/ralph-core/src/event_loop/mod.rs`** — `process_parse_result` 是事件处理单一入口，依次执行 scope enforcement → origin guard → event policy validation → state machine → workflow guard。`ProcessedEvents` 记录处理结果。`TerminationReason` 枚举所有 loop 终止原因。
- **`crates/ralph-cli/src/loop_runner.rs`** — `run_loop_impl` 中的 `loop { }`（line 1114）是主 orchestration 循环。`event_loop.process_events_from_jsonl_with_waves()` 读取事件，`event_loop.process_output()` 处理 agent 输出。当前**启动前无任何 preset 校验**。
- **`crates/ralph-cli/src/hats.rs`** — `validate_hats` 调用 `preset_validator::validate_preset_topology`。`HatsCommands::Validate` 无参数。

### Institutional Learnings

- `docs/solutions/developer-experience/ce-executor-plan-gate-premature-completion-2026-06-02.md` — 讨论了 preset topology gate 的设计，与本次 payload gate 互补。
- `docs/solutions/tooling-decisions/ralph-preset-embedded-compilation-2026-05-26.md` — 内置 preset 通过 `include_str!` 编译，修改 preset 后需运行 `scripts/sync-embedded-files.sh`。

### External References

- 无外部依赖。所有模式均基于现有代码库内的 event policy、preset validator、origin guard 等成熟子系统。

---

## Key Technical Decisions

### 1. Loop Pause 机制：扩展 ProcessedEvents + TerminationReason

**决策：** 不新增 `PolicyDecision` variant，而是让 `apply_event_policy_validation` 在检测到 payload contract violation 时，通过 `PolicyValidationResult` 的扩展字段将 violation 信息传回 `process_parse_result`，再写入 `ProcessedEvents.contract_violation`。loop_runner 在每次事件读取后检查该字段，生成诊断报告并以 `TerminationReason::PayloadContractViolation` 终止 loop。

**理由：**
- `PolicyDecision` 是公共 API（被 `event_policy.rs` 导出），添加新 variant 会影响所有调用方。
- `ProcessedEvents` 已是事件处理的权威结果载体，扩展它比修改 policy 层契约更局部。
- loop_runner 层拥有完整的 hat 上下文（当前 active hat、registry），适合生成包含 source/target hat 的诊断报告。

**替代方案：** 在 `EventLoopState` 中添加持久性 pause 标志。被拒绝：增加不必要的跨迭代状态，且 ralph loop 通常是单次运行（非守护进程）。

### 2. 默认启用 + 默认 Enforce：修改 Default impl

**决策：** `EventPolicyConfig::default()` 的 `enabled` 从 `false` 改为 `true`，`mode` 从 `Observe` 改为 `Enforce`。`RalphConfig` 在未配置 `event_loop.event_policy` 时也必须构造有效的默认 policy，使 runtime guard 默认生效。

**理由：**
- 原始需求 R4.1 明确要求“若 preset 未显式配置，系统默认启用 enforce”。
- 强制 gate 的价值在于把 payload 契约变成系统不变量，而不是 preset 作者可选择开启的 lint。
- 不引入回归的方式不是关闭默认校验，而是把现有 preset 适配到强制 gate 可通过。

**迁移指导：** 若用户已有自定义 preset 依赖 warn-only 行为，需要显式添加 `mode: observe` 作为临时迁移策略；builtin preset 不允许用 observe/disabled 规避本功能，必须补齐 schema 或证明无 payload 字段引用。

### 3. Instructions 字段提取：保守启发式 + 可配置排除

**决策：** 采用基于文本模式的启发式提取，仅识别明确标记为 payload 引用的模式。在 `HatConfig` 中添加 `ignore_payload_fields: Vec<String>`，供 preset 作者排除误报。

**理由：**
- 完全准确的 NLP 提取超出范围且不可靠。
- 保守策略（低召回、低误报）优于宽松策略（高召回、高误报），因为 false positive 会直接阻止 `ralph run` 启动。
- `ignore_payload_fields` 为 preset 作者提供逃生舱，避免被不准确的提取器阻塞。

**提取模式：**
1. `payload[\s]*[:\-]?[\s]*` + 反引号字段（如 ``payload: `field_name` ``）
2. `From event payload[:\-]?` 后的逗号分隔列表
3. `payload MUST include[:\-]?` 后的字段
4. 仅在包含 "payload" 关键字的行/段落中，提取反引号包裹的标识符

### 4. Schema 文件路径解析：CLI 层驱动

**决策：** `RalphConfig::parse_yaml` 保持仅解析 YAML。新增 `RalphConfig::resolve_schema_files(base_path: &Path)` 方法，由 CLI 层在加载 preset 后调用。

**理由：**
- `parse_yaml` 接收纯字符串，无文件系统上下文，无法解析相对路径。
- CLI 层（`loop_runner.rs`、`hats.rs`）知道 preset 文件路径，适合作为解析起点。
- 分离关注点：config 模块负责数据结构，CLI 模块负责文件系统解析。

### 5. Agent 执行契约：先适配，再强制

**决策：** 实现顺序必须固定为：
1. 建立 schema 文件加载和静态校验能力。
2. 为所有 builtin/root preset 建立适配矩阵。
3. 至少让所有 builtin public preset 在默认 `ralph hats validate` 下通过。
4. 为含 payload 字段依赖的 preset 补 schema 或新增明确的 no-payload 契约证明。
5. 最后才切换默认 `event_policy.enabled = true` 和 `mode = Enforce`。

**理由：**
- 机制强制开启后，任何未适配 preset 都会直接变成用户可见回归。
- 计划是写给 agent 执行的，必须把顺序写死，避免 agent 只改核心代码而漏掉 preset 和文档。
- 这也是“不引入回归”和“不破坏现有功能”的主要实现手段。

---

## Implementation Units

### Agent Execution Checklist

后续执行 agent 必须按下面顺序推进，不能跳步：

1. **读取上下文**
   - 读取本计划和 origin brainstorm。
   - 读取 `crates/ralph-core/src/config.rs`、`preset_validator.rs`、`event_policy.rs`、`event_loop/mod.rs`。
   - 读取 `crates/ralph-cli/src/hats.rs`、`loop_runner.rs`、`presets.rs`、`scripts/sync-embedded-files.sh`。
   - 列出 `presets/*.yml`、`presets/*-zh.yml`、`presets/minimal/*.yml`、`crates/ralph-cli/presets/*.yml`。

2. **先建能力，不打开默认强制**
   - 实现 schema_file 解析、payload 字段提取、payload contract validator。
   - 默认值切换前，先用单元测试覆盖 parser/validator/runtime violation 结构。

3. **全量 preset 适配**
   - 生成 U7 适配矩阵。
   - 对所有含 payload 字段引用的 preset 补 schema。
   - 对所有无 schema 的 preset 写明 no-payload 证明。
   - 同步 root preset 与 embedded preset 镜像。

4. **打开强制默认**
   - 将默认 runtime policy 改为 enabled/enforce。
   - 将 `ralph run` 启动前 gate 接入为不可跳过 hard gate。
   - 验证所有 builtin public preset 默认 validate 和 run preflight 不回归。

5. **补文档与反向验证**
   - 更新 U8 文档。
   - 若触及 `crates/ralph-core/data/*.md`，按 AGENTS.md 做源码行号反向验证。

6. **最终验证**
   - `cargo fmt`
   - `cargo test -p ralph-core payload_contract`
   - `cargo test -p ralph-core preset_validator`
   - `cargo test -p ralph-core event_policy`
   - `cargo test -p ralph-core scenarios`
   - `cargo test -p ralph-cli hats`
   - `cargo test -p ralph-cli presets`
   - 对每个 public builtin preset 跑默认 `ralph hats validate`
   - 对每个启用 schema 或含 payload 字段引用的 preset 跑 `ralph hats validate --strict`
   - 最后跑 `./scripts/run-tests.sh`

如果任一 builtin preset 在强制 gate 下失败，任务不得标记完成；必须修 preset/schema 或调整提取器误报。

- [ ] U1. **外部 Schema 文件加载与配置扩展**

**Goal:** 让 `EventPolicyConfig` 支持引用外部 schema 文件，并在加载时自动合并。

**Requirements:** R1.1–R1.4

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/config.rs`
- Test: `crates/ralph-core/src/config.rs` (inline `#[cfg(test)]`)

**Approach:**
- `EventPolicyConfig` 新增 `schema_file: Option<String>` 字段（serde `default`）。
- 新增 `RalphConfig::resolve_schema_files(&mut self, base_path: &Path)` 方法：若 `event_loop.event_policy.schema_file` 存在，读取该 YAML 文件，解析为 `HashMap<String, EventSchema>`，合并到 `event_policy.schemas` 中。
- 合并策略必须固定为“内联定义优先于文件定义”。理由：外部 schema 是共享默认值，preset 文件内联 schema 应能做局部覆写，且不会因为 schema 文件变更意外覆盖调用方显式配置。
- 路径解析规则：若 `schema_file` 为绝对路径则直接使用；若为相对路径，则相对于 `base_path`（preset 文件所在目录）。
- Embedded preset 路径规则：`builtin:*` preset 没有稳定的源文件路径。首期必须支持两种方案之一，并在实现前选定：
  - 把 schema 作为 inline `schemas` 镜像进 embedded preset，避免运行时读文件。
  - 或把 schema 文件同步到 `crates/ralph-cli/presets/...` 并通过新的 embedded schema registry 暴露给 `resolve_schema_files`。
  未解决 embedded 路径前，不得在 public builtin preset 上启用 `schema_file`。
- `HatConfig` 新增 `ignore_payload_fields: Vec<String>`（serde `default`），供 U2 使用。

**Patterns to follow:**
- `ScratchpadConfig` 的自定义 deserializer 模式（若需要支持字符串简写形式）。本次不需要，schema_file 为纯字符串即可。
- 现有 `RalphConfig::validate()` 中的 schema 校验逻辑（检查空 topic、非法路径等）。

**Test scenarios:**
- Happy path: `schema_file` 指向存在的 YAML，正确加载并合并 schema
- Edge case: `schema_file` 为相对路径，基于 `base_path` 正确解析
- Error path: `schema_file` 不存在时必须返回清晰错误。不能静默回退，因为这会让已声明的强 schema 误以为生效。
- Edge case: 文件和内联同时定义同一 topic，合并策略正确
- Error path: schema 文件格式非法（非 YAML、非 Map），返回清晰错误
- Compatibility: 无 `schema_file`、无 `event_policy` 的配置解析后会得到默认 enabled/enforce policy；没有 payload 字段引用的 preset 可通过 gate
- Compatibility: 显式 `mode: observe` 可解析并保留 warn-only 语义，但 builtin preset 不用它规避强校验
- Embedded: 若选择 embedded schema registry，`builtin:ce-executor` 能解析 schema；若选择 inline 镜像，embedded preset 内容包含完整 schema

**Verification:**
- `cargo test -p ralph-core config::tests::test_schema_file_*` 全部通过
- `ralph hats validate` 能正确加载含 `schema_file` 的 preset
- `cargo test -p ralph-cli presets::tests::*embedded*` 或新增等价测试证明 embedded preset schema 不漂移

---

- [ ] U2. **Instructions 字段引用启发式提取器**

**Goal:** 实现从 hat instructions 文本中提取 payload 字段引用的模块。

**Requirements:** R3.1–R3.3

**Dependencies:** U1（需要 `HatConfig.ignore_payload_fields`）

**Files:**
- Create: `crates/ralph-core/src/payload_contract.rs`
- Modify: `crates/ralph-core/src/lib.rs`（导出模块）
- Test: `crates/ralph-core/src/payload_contract.rs` (inline tests)

**Approach:**
- 新建模块 `payload_contract.rs`，包含：
  - `extract_payload_field_references(instructions: &str) -> Vec<String>`：基于正则/文本扫描提取字段名
  - `is_likely_payload_field(word: &str) -> bool`：排除常见非字段词（`true`, `false`, `null`, `ralph`, `emit`, `git`, `code`, 等）
- 提取规则（按优先级）：
  1. 行内匹配 `(?i)payload\s*[:\-]?\s*` 后紧跟 `` `([a-zA-Z_][a-zA-Z0-9_]*)` ``
  2. 行内匹配 `(?i)from event payload\s*[:\-]?\s*([^\n]+)`，从中提取逗号分隔的标识符（支持反引号包裹或裸词）
  3. 行内匹配 `(?i)payload must include\s*[:\-]?\s*([^\n]+)`，同上提取
  4. 对包含 `(?i)payload` 的行，提取所有 `` `([a-zA-Z_][a-zA-Z0-9_]*)` ``，经 `is_likely_payload_field` 过滤
- 去重并返回排序后的字段列表。
- 提取器必须输出“字段来源上下文”（至少包含 hat id、匹配行号、匹配模式、原始行片段），供 U3/U5 诊断使用。单纯 `Vec<String>` 不足以支撑“1 分钟定位”和误报排查。
- 提取器只作为 preset 静态校验输入，不参与 runtime event policy。runtime 必须只信任显式 schema，避免 instructions 文案变化影响运行时行为。

**Technical design:**
> *Directional guidance for review, not implementation specification.*
> 使用 `regex` crate（项目已有依赖）编译一次静态正则，在 `once_cell::Lazy` 中缓存。提取函数为纯函数，不依赖外部状态，便于单元测试。

**Patterns to follow:**
- 项目已有 `regex` 依赖（`event_reader.rs`、`claude_stream.rs` 等大量使用）。
- `preflight.rs` 中的文本解析模式（acceptance criteria 提取）。

**Test scenarios:**
- Happy path: `` "From event payload: `task_id`, `plan_name`" `` → `["plan_name", "task_id"]`
- Happy path: `` "payload: `dimension`" `` → `["dimension"]`
- Happy path: 多行 instructions，混合模式，正确去重
- Edge case: 包含 `(?i)payload` 的行中有 `` `git` ``、`` `ralph` `` 等，被过滤
- Edge case: 无 payload 关键字的行中的反引号字段，不被提取
- Edge case: 空 instructions → 空列表
- Error path: 字段名包含非法字符（如 `-`），保留或过滤？决策：仅提取合法标识符 `[a-zA-Z_][a-zA-Z0-9_]*`
- Regression: 对所有 public builtin preset 的 instructions 运行提取器并 snapshot/断言关键字段，确认不会把命令名、文件名、section 标题误识别为 payload 字段

**Verification:**
- `cargo test -p ralph-core payload_contract::tests` 全部通过
- 用 `ce-executor.yml` 的实际 instructions 做抽样验证，提取结果与人工检查一致率 > 80%
- 用 `code-assist`、`debug`、`review`、`research`、`pdd-to-code-assist`、`autoresearch` 做误报抽样；若误报会影响 `--strict`，必须在 preset 中加入 `ignore_payload_fields` 或调整提取规则

---

- [ ] U3. **编排期 Payload 契约校验器**

**Goal:** 在 `preset_validator.rs` 中新增跨 hat payload 契约校验，复用拓扑图做上下游字段匹配。

**Requirements:** R2.2–R2.5, R3.2–R3.3, R6.1–R6.2

**Dependencies:** U1, U2

**Files:**
- Modify: `crates/ralph-core/src/preset_validator.rs`
- Create: `crates/ralph-core/src/payload_contract.rs`（U2 已创建，本次添加校验逻辑）
- Test: `crates/ralph-core/src/preset_validator.rs` (inline tests)

**Approach:**
- 在 `payload_contract.rs` 中新增：
  - `PayloadContractError` struct：包含 `error_type`, `topic`, `field`, `source_hat`, `target_hat`, `message`, `details`（schema 位置、上下游引用位置、fix_hint）
  - `PayloadContractValidationResult`：类似 `TopologyValidationResult`，`errors` + `warnings`
  - `validate_payload_contracts(config: &RalphConfig, registry: &HatRegistry) -> PayloadContractValidationResult`
- 建议实际 API 使用显式模式参数，避免 `hats validate`、`hats validate --strict`、`ralph run` 三种语义混淆：
  ```rust
  pub enum PayloadContractCheckMode {
      DefaultValidate,
      StrictValidate,
      RunHardGate,
  }

  pub fn validate_payload_contracts(
      config: &RalphConfig,
      registry: &HatRegistry,
      mode: PayloadContractCheckMode,
      source: PayloadContractSourceContext,
  ) -> PayloadContractValidationResult
  ```
- `PayloadContractSourceContext` 至少包含：
  - `preset_path: Option<PathBuf>`
  - `schema_path: Option<PathBuf>`
  - `raw_preset_yaml: Option<String>`（用于行号定位）
  - `embedded_preset_name: Option<String>`
- 校验逻辑：
  1. 构建 `topic → Vec<&Hat>` 订阅映射（复用 `TopologyGraph` 的构建逻辑，或提取公共函数）
  2. 对每个 hat（排除 fallback），调用 `extract_payload_field_references(&hat.instructions)`
  3. 对该 hat 的每个 trigger topic，检查 schema 是否存在：
     - 若该 hat 的 instructions 提取到 payload 字段，trigger topic 缺 schema → error（默认 validate、strict validate、run hard gate 都失败）
     - 若没有提取到 payload 字段，trigger topic 缺 schema → 默认 validate warning，`--strict` error
     - 若存在，检查提取的字段是否在 schema `required_fields` 中 → 不在则错误（R3.2）
  4. 对每个 topic，检查所有订阅者的字段需求是否被所有发布者的 schema 覆盖（R2.3）：
     - 若订阅者需要字段 `X`，但某个发布者的 schema 未声明 `X` → 错误
     - 这里需要明确：是检查"所有发布者都提供"还是"至少一个发布者提供"？
     - 决策：检查"所有发布者都提供"更严格。因为不确定运行时哪个发布者会触发，所有路径都必须安全。
- 错误信息包含：topic、字段名、下游 hat 及 instructions 行号引用、上游 hat 及 instructions 行号引用、schema 定义位置。
- `ignore_payload_fields` 必须在校验阶段按 hat 生效，并在 warning/error 中说明字段被忽略时不再作为缺失 schema 处理。
- wildcard trigger（如 `review.*`）不能简单字符串相等；必须复用现有 topic/subscription matching 规则，或明确提取公共 matcher，避免与 runtime 路由不一致。
- `PayloadContractValidationResult` 必须提供：
  - `is_valid_for_run() -> bool`：仅当 `errors.is_empty()` 时 true
  - `error_count()`
  - `warning_count()`
  - `render_human_summary()` 或等价 CLI helper，保证 `hats validate` 与 `ralph run` 的错误摘要一致

**Technical design:**
> *Directional guidance for review, not implementation specification.*
> 行号引用通过扫描原始 YAML 内容获取。`RalphConfig` 解析时不保留行号，但可通过在 CLI 层传入原始 YAML 文本 + `serde_yaml::Value` 的映射来实现。为简化，首期版本使用文件路径（不含行号），后续迭代补充行号。若项目已使用 `serde_yaml` 的 `with_span` 等扩展则优先利用。

**Patterns to follow:**
- `TopologyGraph::build` 的 hat/topic 索引逻辑。
- `TopologyValidationResult` / `TopologyError` / `TopologyErrorKind` 的错误分类模式。

**Test scenarios:**
- Happy path: 上下游字段完全匹配，校验通过
- Happy path: 无 schema 配置且 hat instructions 无 payload 引用，校验通过
- Error path: 下游 instructions 引用 `task_id`，但 trigger topic 的 schema 未定义该字段
- Error path: 下游 instructions 引用字段，但 trigger topic 无 schema 定义
- Error path: topic 有多个发布者，其中一个未提供下游需要的字段
- Edge case: wildcard trigger（如 `review.*`），需匹配所有对应 schema
- Integration: `validate_preset_topology` + `validate_payload_contracts` 联合调用时互不干扰
- Compatibility: `strict = false` 下，只有“无 payload 字段引用”的缺 schema 产生 warning；带 payload 字段引用的缺 schema 必须 error
- Regression: 所有 public builtin preset 在默认 validate 模式下返回 0，前提是 U7 已完成 schema/no-payload 适配

**Verification:**
- `cargo test -p ralph-core preset_validator::tests::test_payload_contract_*` 全部通过
- 现有拓扑校验测试不因新增代码而失败

---

- [ ] U4. **CLI 集成：Hard Gate 与 `ralph hats validate --strict`**

**Goal:** 将 payload 契约校验集成到 CLI：`ralph run` 启动前自动校验，`ralph hats validate --strict` 启用 schema 完备性检查。

**Requirements:** R2.1, R2.4–R2.5, R6.1

**Dependencies:** U1, U3

**Files:**
- Modify: `crates/ralph-cli/src/hats.rs`
- Modify: `crates/ralph-cli/src/loop_runner.rs`
- Modify: `crates/ralph-cli/src/preflight.rs`（若 preset 加载逻辑集中在此处）
- Test: `crates/ralph-cli/src/hats.rs` (inline tests)、BDD scenarios

**Approach:**
- **`hats.rs`：**
  - `HatsCommands::Validate` 新增 `#[arg(long)] strict: bool`
  - `validate_hats` 函数签名添加 `strict: bool` 参数
  - 调用 `preset_validator::validate_preset_topology` 后，若拓扑通过，继续调用 `validate_payload_contracts`
  - 若 `strict` 为 true，额外检查：所有有订阅者的 topic 必须有 schema 定义（R6）
  - 若 `strict` 为 false，payload 字段引用缺 schema、字段不在 schema `required_fields`、发布者 schema 不满足订阅者字段需求均为 error；只有无 payload 字段引用的 topic 缺 schema 可作为 warning
  - 输出格式：沿用现有的 `[ok]`/`[warn]`/`[err]` 前缀，新增 payload contract 错误段落
- **`loop_runner.rs`：**
  - 在 `run_loop_impl` 中，`EventLoop::with_context` 创建之前，调用 `validate_payload_contracts`
  - payload 校验失败永远作为 hard gate：直接输出错误、非零退出、不启动 loop、不调用 agent
  - 不允许新增 `--skip-payload-check`、`RALPH_SKIP_PAYLOAD_CHECK` 等绕过开关
  - 若校验失败，直接输出错误摘要到终端（使用 `print_loop_banner` 或类似的错误输出 helper），然后 `return Err(...)`（exit code 非零）
  - 不启动 event loop，不创建任何事件文件
  - 需确保 `resolve_schema_files(base_path)` 在校验前已被调用
- **CLI 帮助与工具文档：**
  - `ralph hats validate --help` 必须展示 `--strict`
  - 若 `ralph tools` 文档引用 hats validate 或 event policy 行为，必须同步更新 `crates/ralph-core/data/*.md` 并按 AGENTS.md 的反向验证规则复核源码行号

**Patterns to follow:**
- `hats.rs` 中现有的拓扑校验输出格式（`[ok]`/`[warn]`/`[err]` + 彩色输出）。
- `loop_runner.rs` 中已有的错误返回模式（`anyhow::Result`）。

**Test scenarios:**
- Happy path: `ralph run` 启动，payload 契约通过，loop 正常启动
- Error path: `ralph run` 启动，payload 契约失败，exit code 1，终端显示错误摘要
- Happy path: `ralph hats validate`（无 `--strict`），拓扑通过即返回 0，不检查 schema 完备性
- Error path: `ralph hats validate --strict`，schema 缺失，exit code 非零
- Error path: `ralph hats validate --strict`，payload 契约违规，exit code 非零
- Compatibility: `ralph hats validate` 不带 `--strict` 时，现有 orphan warning 用例仍返回 0
- Compatibility: `ralph run -H builtin:code-assist` 在强制 gate 下可启动，原因必须是 code-assist 已补 schema 或 U7 证明没有未声明 payload 字段依赖，不能依赖未启用 event_policy

**Verification:**
- `cargo test -p ralph-cli hats::tests` 通过
- BDD scenario `payload_contract_gate.yml`（新建）通过
- `cargo run -p ralph-cli -- hats validate --help` 或等价二进制命令显示 `--strict`

---

- [ ] U5. **运行时 Enforce 默认与 Loop Pause 机制**

**Goal:** 运行时 event policy 默认 enforce，payload contract violation 触发 Loop Pause 并生成结构化诊断报告。

**Requirements:** R4.1–R4.3, R5.1–R5.5

**Dependencies:** U1, U4

**Files:**
- Modify: `crates/ralph-core/src/config.rs`
- Modify: `crates/ralph-core/src/event_policy.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-cli/src/loop_runner.rs`
- Test: `crates/ralph-core/src/event_loop/tests.rs`、内联 tests

**Approach:**
1. **默认 mode 变更：**
   - `EventPolicyConfig::default()` 的 `enabled` 改为 `true`
   - `EventPolicyConfig::default()` 的 `mode` 改为 `EventPolicyMode::Enforce`
   - `RalphConfig` 未显式配置 `event_loop.event_policy` 时，事件循环必须使用默认 enabled/enforce policy，而不是 `None` 等价于禁用
   - 若保留 `event_loop.event_policy: null` 或 `enabled: false` 的解析能力，必须只作为用户显式迁移逃生舱；builtin preset 和测试夹具不得依赖它绕过 schema

2. **运行时 violation 识别：**
   - `event_policy.rs`：`validate_event` 仍返回现有 `PolicyDecision`，不新增 variant。
   - `apply_event_policy_validation` 负责识别 schema 相关 finding（`MissingRequiredField`, `PayloadTypeMismatch`, `InvalidFieldValue`），并在 `Enforce` 模式下把它们提升为 **payload contract violation**。
   - completion-after-terminal、duplicate terminal、business-after-completion 等非 schema finding 必须继续遵守现有 `on_violation`/completion action 逻辑，避免运行时策略回归。
   - **决策：** 采用扩展 `PolicyValidationResult` 的方案（见 Key Technical Decision 1），最小化公共 API 变更。

3. **事件处理层传递：**
   - `event_loop/mod.rs`：`PolicyValidationResult` 新增 `contract_violation: Option<PayloadContractError>`
   - `apply_event_policy_validation` 中，当 `Enforce` 模式下出现 schema violation 时，填充 `contract_violation`（不再对该事件执行 `RejectWithResume`/`Hold`/`Block`）
   - `process_parse_result` 将 `contract_violation` 传递到 `ProcessedEvents`

4. **Loop Pause 触发：**
   - `ProcessedEvents` 新增 `contract_violation: Option<PayloadContractError>`
   - `loop_runner.rs`：在 `event_loop.process_events_from_jsonl_with_waves()` 返回后，检查 `processed.contract_violation`
   - 若存在：
     a. 生成诊断报告 JSON → `.ralph/diagnostics/payload-contract-error-{timestamp}.json`
     b. 终端输出 `[PAYLOAD CONTRACT VIOLATION] Loop paused.` 摘要
     c. 以 `TerminationReason::PayloadContractViolation` 终止 loop（调用 `handle_termination` 等现有 hook 流程）
   - 若不存在：正常继续
   - 诊断报告写入失败时，仍必须以 `PayloadContractViolation` 终止，并在终端输出报告写入失败原因和原始 violation 摘要。

5. **诊断报告内容：**
   - 报告结构遵循需求文档 R5.3 的字段列表
   - `source_hat`：从 `registry` 查找能发布该 topic 的 hat（若有多个，列出全部）
   - `target_hat`：当前 active hat（`event_loop.state().last_active_hat_ids`）
   - `details.schema_defined_in`：schema 定义文件路径
   - `details.downstream_reference` / `details.upstream_reference`：基于 `HatConfig` 和 `registry` 的引用（首期版本可仅包含 hat ID 和 topic，不含精确行号）
   - JSON struct 建议固定为：
     ```json
     {
       "error_type": "missing_required_field",
       "timestamp": "2026-06-02T12:34:56Z",
       "event": {
         "topic": "review.wave.ready",
         "source_hat": ["review-coordinator"],
         "target_hat": "dimension-reviewer"
       },
       "field": "task_id",
       "severity": "error",
       "message": "Field `task_id` missing in event `review.wave.ready`.",
       "details": {
         "schema_defined_in": "presets/ce-executor/schemas.yml:45",
         "downstream_reference": "presets/ce-executor.yml:390",
         "upstream_reference": "presets/ce-executor.yml:184"
       },
       "fix_hint": "Add `task_id` to the publisher payload and to the topic schema required_fields."
     }
     ```
   - 若 source hat 无法唯一确定，`source_hat` 必须是数组；不能猜一个 source 并丢掉其他候选。

**Technical design:**
> *Directional guidance for review, not implementation specification.*
> `TerminationReason` 新增 `PayloadContractViolation { report_path: String }`。`termination_status_text` 返回人类可读描述，`exit_code` 返回 1。loop_runner 中的终止处理流程（pre/post termination hooks、terminate event 发布）对新 reason 透明工作。

**Patterns to follow:**
- `event_loop/mod.rs` 中 `apply_event_policy_validation` 对 `RejectWithResume`/`Hold` 的处理模式。
- `TerminationReason` 的扩展模式（参考 `RestartRequested`、`WorkspaceGone` 等新增 reason 的历史）。
- `.ralph/diagnostics/` 下的日志写入模式（参考 `DiagnosticsCollector`）。

**Test scenarios:**
- Happy path: `Enforce` 模式下，事件 payload 完全符合 schema，正常通过
- Happy path: `Observe` 模式下，payload 违规仅触发 `Warn`，不 pause loop
- Error path: `Enforce` 模式下，缺失 required field → `ProcessedEvents.contract_violation` 被填充
- Error path: loop_runner 检测到 contract violation → 生成 JSON 报告、终端输出、以 `PayloadContractViolation` 终止
- Edge case: 多个事件同时违规，仅报告第一个（或全部？决策：报告第一个，避免信息过载）
- Integration: event_loop integration tests 中验证新的 terminate reason
- Regression: `on_violation: hold` 的非 schema violation 仍生成原有 hold 行为，不被误判为 contract violation
- Regression: origin guard 拒绝、state machine 拒绝、workflow guard 拒绝仍走原有终止/处理路径
- Regression: 未显式配置 `event_policy` 时，默认 enabled/enforce 生效，缺字段会触发 contract violation
- Migration: 显式 `mode: observe` 时，payload 违规仅 warn；若显式 `enabled: false` 被保留，需有测试证明这是显式配置行为而非默认行为

**Verification:**
- `cargo test -p ralph-core event_loop::tests::test_event_policy_*` 全部通过
- 新增 integration test：构造 enforce + missing field 场景，验证 loop 以 `PayloadContractViolation` 终止
- 诊断报告 JSON 结构通过 schema 校验（可用 `serde_json::from_str` 反序列化验证）

---

- [ ] U6. **ce-executor.yml Schema 定义与端到端测试**

**Goal:** 为 `ce-executor.yml` 和 `ce-executor-zh.yml` 编写完整 schema，验证整个系统工作流，并处理 embedded preset 镜像。

**Requirements:** 所有 R1–R6 的端到端验证

**Dependencies:** U1–U5

**Files:**
- Create: `presets/ce-executor/schemas.yml`
- Create or mirror: `crates/ralph-cli/presets/ce-executor/schemas.yml`（若选择 embedded schema registry）
- Modify: `presets/ce-executor.yml`（添加 `event_policy.schema_file` 引用）
- Modify: `presets/ce-executor-zh.yml`（添加同等 `event_policy` 配置，必须启用并与英文 preset 使用同一契约）
- Modify: `crates/ralph-cli/presets/ce-executor.yml` / `ce-executor-zh.yml`（通过同步脚本生成，不手工漂移）
- Modify: `scripts/sync-embedded-files.sh`（若新增 schema 镜像文件）
- Test: `crates/ralph-core/tests/scenarios/payload_contract*.yml`、BDD tests

**Approach:**
1. **Schema 文件编写：**
   - 分析 `ce-executor.yml` 中所有 hat 的 triggers 和 instructions，提取所有 payload 字段引用
   - 为每个 topic 定义 `EventSchema`，包含 `payload: json_object` 和 `required_fields`
   - 参考需求文档附录中的 schema 示例作为起点，补全所有缺失 topic（如 `work.start`, `work.ready`, `review.wave.ready`, `review.dimension.done`, `review.wave.done`, `plan.ready`, `plan.gate.passed`, `fix.ready`, `ship.ready`, `report.ready`, `LOOP_COMPLETE` 等）
   - 若某些 topic 的 payload 结构不确定，先运行一次 `ce-executor` 并观察实际 emit 的事件

2. **Preset 修改：**
   - `ce-executor.yml` 的 `event_loop` 段添加：
     ```yaml
     event_policy:
       enabled: true
       schema_file: "ce-executor/schemas.yml"
     ```
   - `ce-executor-zh.yml` 必须与英文 preset 的事件契约保持一致。中文说明可以不同，但 schema、triggers、publishes、default_publishes 的 payload contract 必须等价。
   - `ce-executor.yml` 和 `ce-executor-zh.yml` 必须显式写出 `event_policy.enabled: true` / `mode: enforce`，即使 defaults 已经强制开启，也要作为 preset 作者示范。
   - 运行 `scripts/sync-embedded-files.sh`，确保 `crates/ralph-cli/presets/` 镜像与 root `presets/` 一致。若 schema 文件需要嵌入，脚本必须同步 schema 文件并增加漂移检查。

3. **测试：**
   - BDD scenario：构造一个故意缺少 `task_id` 字段的 preset，验证 `ralph hats validate --strict` 捕获错误
   - BDD scenario：构造运行时 payload 缺失场景，验证 loop pause 和诊断报告
   - 冒烟测试：`ce-executor.yml` + schemas.yml 能通过 `ralph hats validate --strict`

**Patterns to follow:**
- `presets/ce-executor.yml` 的现有结构。
- `crates/ralph-core/tests/scenarios/` 中的 YAML scenario 格式（参考 `autoresearch_guard.yml` 或 `isolated_boundary_violation.yml`）。

**Test scenarios:**
- Happy path: `ralph hats validate --strict -p presets/ce-executor.yml` 通过
- Happy path: `ralph hats validate --strict -p presets/ce-executor-zh.yml` 通过
- Error path: schema 文件中删除一个 required field，`ralph hats validate --strict` 报错
- Error path: 运行时 agent emit 缺少字段的事件，loop pause，诊断报告生成
- Happy path: 修复 preset 后重新 `ralph run`，loop 正常执行
- Embedded: `builtin:ce-executor` 的 schema 与 root preset schema 一致，不因 `include_str!` 路径缺失失败

**Verification:**
- `cargo test -p ralph-core scenarios` 通过（含新增 scenarios）
- `ralph hats validate --strict -p presets/ce-executor.yml` 返回 0
- `ralph hats validate --strict -p presets/ce-executor-zh.yml` 返回 0（若启用）
- `scripts/sync-embedded-files.sh --check` 或脚本等价检查通过
- `scripts/run-tests.sh` 全量通过

---

- [ ] U7. **全量 Preset 兼容性审计与适配矩阵**

**Goal:** 防止新契约系统只适配 `ce-executor`，同时确保强制开启后其他 preset 不被默认行为破坏。

**Requirements:** 非回归、现有功能不破坏、preset 适配完整性

**Dependencies:** U2, U3, U4, U6

**Files:**
- Modify: `docs/plans/2026-06-02-005-feat-payload-contract-validation-plan.md`（实施时更新审计结果）
- Modify if needed: `presets/*.yml`, `presets/*-zh.yml`, `presets/minimal/*.yml`
- Modify if needed: `crates/ralph-cli/src/presets.rs` tests
- Modify if needed: `scripts/ralph-zsh-plugin.zsh`（仅当 builtin preset 名称新增/删除/重命名时）

**Approach:**
- 建立适配矩阵，至少包含：
  - public builtin: `autoresearch`, `ce-executor`, `code-assist`, `debug`, `pdd-to-code-assist`, `research`, `review`
  - hidden builtin: `hatless-baseline`, `merge-loop`
  - root-only / non-embedded: `harness-demo`, `wave-review`, `ralph.reviewer`, `minimal/*`
  - 中文镜像：所有 `*-zh.yml`
- 每个 preset 记录六列：preset 路径、embedded 状态、是否含 payload instructions、覆盖的 trigger topics、schema 状态、当前策略（补 schema / no-payload 证明 / 需要立即修复）。
- 适配矩阵必须写入本计划或后续实现报告，格式固定为：
  | Preset | Embedded | Payload refs | Trigger topics needing schema | Schema source | Strategy | Validation command |
  |--------|----------|--------------|-------------------------------|---------------|----------|--------------------|
  | `presets/ce-executor.yml` | yes | yes | `work.ready`, ... | `presets/ce-executor/schemas.yml` | 补 schema | `ralph hats validate --strict -p presets/ce-executor.yml` |
  | `presets/research.yml` | yes | TBD | TBD | TBD | TBD | TBD |
- 对未补 schema 的 preset，必须证明提取器没有发现 payload 字段引用；否则必须补 schema，不能标记为“暂不启用但兼容”。
- 对启用 schema 的 preset，必须验证 `--strict` 通过，且 embedded/mirrored 版本可解析。
- 若 builtin preset 名称或 public 列表变化，按 AGENTS.md 更新 `scripts/ralph-zsh-plugin.zsh`、安装到 `~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh` 并验证 zsh completion loads。若名称不变，记录“不需要 zsh completion 更新”。

**Test scenarios:**
- 所有 public builtin preset YAML parse 通过
- 所有 public builtin preset 默认 `hats validate` 在强制 payload 校验下返回 0
- `ce-executor`/`ce-executor-zh` strict validate 通过
- 无 schema 的 preset 只有在 no-payload 证明成立时才能通过；若 instructions 中出现 payload 字段引用，`ralph run` 启动前必须失败
- Embedded preset drift guard 覆盖新增 schema 镜像或 inline schema 内容

**Verification:**
- `cargo test -p ralph-cli presets::tests` 通过
- `cargo test -p ralph-core preset_validator::tests` 通过
- 对每个 public builtin preset 运行一次默认 validate 冒烟
- 对所有启用 schema 的 preset 运行一次 `--strict` validate 冒烟

---

- [ ] U8. **文档、迁移说明与工具说明更新**

**Goal:** 确保功能上线后用户、preset 作者和支持人员知道如何启用、验证、排障和迁移。

**Requirements:** 文档更新计划完整性

**Dependencies:** U1-U7

**Files:**
- Modify: `docs/guide/harness-extensions.md`（event_policy、schema_file、enforce 默认、diagnostics）
- Modify or create: `docs/guide/payload-contracts.md`（若现有 guide 不适合承载完整说明）
- Modify: `presets/COLLECTION.md`（preset 作者如何声明 payload schema、何时使用 external schema）
- Modify if applicable: `crates/ralph-core/data/ralph-tools.md` / `ralph-tools-tasks.md` / `ralph-tools-memories.md`
- Modify: `README.md` 或 CLI guide（若已有 hats validate 文档）

**Approach:**
- 用户文档必须包含：
  - `event_policy.schema_file` 示例
  - inline schema 与 external schema 合并策略（内联优先）
  - `ralph hats validate` vs `ralph hats validate --strict` 的区别
  - `ralph run` hard gate 强制生效、不可跳过，以及 no-payload preset 如何通过 gate
  - Loop pause 后如何读取 `.ralph/diagnostics/payload-contract-error-*.json`
  - `mode: observe` 若被保留，仅作为用户迁移用法；builtin preset 不使用 observe 规避强校验
- preset 作者文档必须包含：
  - 如何在 instructions 中写 payload 字段，避免提取器误判
  - `ignore_payload_fields` 的适用场景和风险
  - 中文 preset 与英文 preset 的契约同步要求
  - embedded preset/schema 同步要求：修改 root preset 后必须运行 `scripts/sync-embedded-files.sh`
- 如果修改 `crates/ralph-core/data/*.md` 中的 `ralph tools` 命令说明，必须按 AGENTS.md 的反向验证要求，用 `sed -n 'NN,MMp' <file>` 复核所有源码行号引用，并跑对应 `ralph <cmd> --help` 冒烟。

**Test scenarios:**
- 文档中的 `schema_file` 示例能被 `RalphConfig::parse_yaml` + `resolve_schema_files` 解析
- CLI help 与文档中的 `--strict` 描述一致
- 诊断报告示例字段与实际 JSON struct 可反序列化结构一致

**Verification:**
- `cargo run -p ralph-cli -- hats validate --help` 输出与文档一致
- 若更新 `ralph-core/data/*.md`，完成 AGENTS.md 要求的源码行号反向验证
- 文档链接和文件路径存在

---

## System-Wide Impact

- **Interaction graph:**
  - `event_policy.rs` 的 `validate_event` 行为变更：`Enforce` 模式下 schema violation 不再走 `on_violation` 分支，而是走新的 contract violation 路径。
  - `loop_runner.rs` 的主循环新增 `contract_violation` 检查点，位于每次 `process_events_from_jsonl_with_waves()` 之后。
  - `hats.rs` 的 `validate_hats` 新增 `--strict` 分支和 payload 契约校验调用。
- **Compatibility boundary:**
  - hard gate 对 `ralph run` 强制生效；默认 preset 和用户旧配置不得因为缺 schema 回归，解决方式是 preset/schema 适配或 no-payload 证明，不是关闭 gate。
  - `ralph hats validate` 不带 `--strict` 也必须执行 payload contract 校验；带 payload 字段引用的 schema 缺失是 error，无 payload 字段引用的 schema 缺失可为 warning。
  - `schema_file` 不能在 embedded preset 场景下依赖不存在的 root repo 路径；必须通过 inline schema 或 embedded schema registry 解决。
- **Error propagation:**
  - 编排期错误：从 `preset_validator` → `validate_hats` / `run_loop_impl` → 终端输出 → 非零退出。
  - 运行时错误：从 `event_policy` → `apply_event_policy_validation` → `ProcessedEvents` → `loop_runner` → 诊断报告 + `TerminationReason::PayloadContractViolation`。
- **State lifecycle risks:**
  - 运行时 pause 不写入 hold artifact（与 `PolicyDecision::Hold` 不同），不注入 `task.resume`。loop 完全终止，开发者需手动修复后重新 `ralph run`。
  - 已产生的有效事件和文件修改保留在 workspace 中（与正常 loop 终止一致）。
- **API surface parity:**
  - `PolicyDecision` 枚举**不新增 variant**（遵循 Key Technical Decision 1），保持下游代码兼容。
  - `TerminationReason` 新增 variant，但 `exit_code` 和 `as_str` 的 match  arms 必须完整覆盖，编译器会强制检查。
- **Integration coverage:**
  - Wave 事件处理路径（`process_events_from_jsonl_with_waves`）与常规事件共用 `process_parse_result`，因此自动继承 contract violation 检测。
  - 但 wave worker 的 payload 聚合逻辑不受本次变更影响（见 Scope Boundaries）。
  - Embedded preset 镜像路径必须纳入测试，否则 `presets/ce-executor.yml` 本地通过但 `builtin:ce-executor` 发布后二进制失败。
- **Unchanged invariants:**
  - `ralph run` 启动前必须校验 payload contract，且不可跳过。
  - `Observe` 模式若作为用户显式迁移配置保留，其行为仍是仅警告、不 pause；builtin preset 不依赖 observe 规避强校验。
  - 拓扑校验逻辑不变，现有 `TopologyErrorKind` 不变。

---

## Risks & Dependencies

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| `EventPolicyConfig::default()` enabled/enforce 变更导致现有 preset 行为变化 | High | High | U7 必须先完成所有 builtin/root preset 适配矩阵和 schema/no-payload 证明，再切换默认强制开启。 |
| Instructions 字段提取误报过高，阻塞正常 preset | Med | High | 保守提取策略 + `ignore_payload_fields` 逃生舱。U2 测试覆盖率要求 > 80% 一致率。 |
| 运行时 Loop Pause 与现有 hook/terminate 流程冲突 | Low | High | 复用现有 `TerminationReason` 和 `handle_termination` 路径，不新增特殊流程。U5 集成测试覆盖。 |
| ce-executor.yml schema 定义不完整，导致自身无法通过校验 | Med | Med | U6 中通过实际运行收集字段，先完成 schema 再启用 enforce。 |
| Schema 文件路径解析在 embedded preset 场景下失效 | Low | Med | Embedded preset（`include_str!`）无文件系统路径，需回退到内联 schema 或预设已知路径。U1 中处理。 |
| 默认 `ralph hats validate` 退出码变化破坏用户脚本 | Med | High | 这是强制机制的有意行为范围：只有真实 payload 契约错误改变退出码；无 payload 字段引用的 schema 缺失保持 warning。文档需明确。 |
| 只适配 ce-executor，其他 preset 被强制 gate 卡住 | Med | High | U7 建立全量 preset 适配矩阵，所有 public builtin、hidden builtin、root-only、minimal 和中文 preset 都必须审计。 |
| schema 文件与 embedded preset 镜像漂移 | Med | High | U6/U7 更新 `scripts/sync-embedded-files.sh` 和 drift guard 测试，发布前跑同步检查。 |
| 文档落后导致 preset 作者误用 `schema_file` 或 `ignore_payload_fields` | Med | Med | U8 把用户指南、preset 作者指南、诊断报告格式和迁移说明列为交付项，而不是 PR 描述附带说明。 |

---

## Documentation / Operational Notes

- 文档更新不是可选项，纳入 U8 交付。至少更新 `docs/guide/harness-extensions.md`，必要时新增 `docs/guide/payload-contracts.md`。
- `ce-executor.yml` / `ce-executor-zh.yml` 的修改需在文档和 PR 描述中标注迁移步骤，供其他 preset 作者参考。
- 新增的诊断报告格式必须有字段说明和示例，便于支持人员解读用户提供的 `.ralph/diagnostics/payload-contract-error-*.json`。
- `ralph hats validate --strict` 的 CLI help、用户文档和测试断言必须一致。
- 若触及 `ralph tools` 文档，必须执行 AGENTS.md 规定的源码行号反向验证和命令冒烟。

---

## Sources & References

- **Origin document:** [docs/brainstorms/2026-06-02-payload-contract-validation-requirements.md](docs/brainstorms/2026-06-02-payload-contract-validation-requirements.md)
- Related code:
  - `crates/ralph-core/src/config.rs`
  - `crates/ralph-core/src/preset_validator.rs`
  - `crates/ralph-core/src/event_policy.rs`
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-cli/src/loop_runner.rs`
  - `crates/ralph-cli/src/hats.rs`
- Related docs:
  - `docs/solutions/developer-experience/ce-executor-plan-gate-premature-completion-2026-06-02.md`
  - `docs/solutions/tooling-decisions/ralph-preset-embedded-compilation-2026-05-26.md`
