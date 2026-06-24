---
title: Ralph 统一编排状态重构（U0–U10）执行 Review 报告
type: review
status: active
date: 2026-06-22
branch: pittcat-dev
reviewed_commits: ab72546..5e7dfcf
plan_ref: docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md
---

# Ralph 统一编排状态重构（U0–U10）执行 Review 报告

> **Review 范围**：`pittcat-dev` 分支 commits `ab72546`（U0 inventory）至 `5e7dfcf`（U10 verification），覆盖计划 U0–U10 全部实现单元。
>
> **Review 方法**：源码逐模块对比、feature flag 开关验证、production 调用链追踪、测试矩阵检查、文档同步审查。

---

## 执行摘要

本次 Review 对 "统一编排状态重构"计划（2026-06-21-002）的 U0–U10 执行结果进行了全维度检查。

**结论：阻塞（BLOCKED）**。

虽然 U1–U7 的新代码模块（`StateLedger`、`ProtocolView`、`ValidationPipeline`、`CorrectionContext`、`loop.resume`）全部实现了代码层面的结构和单元测试，但**核心架构目标在 production event loop 中完全未接入**。`event_loop/mod.rs` 的 `process_parse_result` 仍使用 legacy gate 栈，新代码是“已实现但不可用”的 dead code。此外，U10 验证报告包含与源码矛盾的虚假声明，2 条 BDD 因 production wire-up 缺失被 `#[ignore]`，多处计划要求的文档同步未执行。

---

## 1. 任务完成度：不通过

### 1.1 逐单元完成状态

| 单元 | 计划目标 | 代码实现 | Production 接入 | 状态 |
|---|---|---|---|---|
| **U0** 盘点 | 基线 fixtures + 调用点盘点 | ✅ 完成 | — | 通过 |
| **U1** StateLedger | 单一状态账本 + commit log | ✅ 完成 | ❌ **零调用** | **不通过** |
| **U2** StateProjector 迁移 | projector 从 ledger 派生 | ✅ 完成 | ❌ **零调用** | **不通过** |
| **U3** ProtocolView | 统一配置协议视图 | ✅ 扩展完成 | ❌ loop 未调用新方法 | **不通过** |
| **U4** ValidationPipeline | 统一验证流水线（pre/post commit） | ✅ 完成 | ❌ **零调用** | **不通过** |
| **U5** handoff auto-gen | macro-edge 自动写 artifact | ✅ 完成 | ❌ **零调用** | **不通过** |
| **U6** CLI policy-check | `--policy-check` 共享 `validate_event` | ⚠️ 部分 | CLI 已接入 Unified 路径 | 部分通过 |
| **U7a** task.resume → correction | 删除 `task.resume`，改 prompt correction | ⚠️ 部分 | ❌ rejection 路径仍发 `task.resume` | **不通过** |
| **U7b** loop.resume | `--continue` 替代 `task.resume` | ✅ 完成 | ✅ `initialize_resume` 已切换 | 通过 |
| **U8** diagnose/continue | `ralph diagnose` 读 ledger rejection log | ⚠️ 部分 | CLI 新增 `--from-ledger`/`--legacy` | 部分通过 |
| **U9** 测试迁移 | 更新测试 + 新增 BDD | ⚠️ 部分 | 2 条 BDD 被 `#[ignore]` | 部分通过 |
| **U10** 全量验证 | 两种 flag 状态全绿 | ❌ 未通过 | 开启全部 flag 后 16 条失败 | **不通过** |

### 1.2 关键证据

#### 证据 1：`ValidationPipeline` 在 event loop 中零引用

```bash
grep -c "ValidationPipeline\|validate_pre_commit\|validate_post_commit\|validate_with_preview" \
  crates/ralph-core/src/event_loop/mod.rs
# 输出: 0
```

`process_parse_result`（`event_loop/mod.rs:7330`）仍使用 legacy 路径：
- `apply_step_handoff_gate`（line ~8817）
- `apply_workflow_guard_validation`（line ~8850）
- `validate_execution_contract`（line ~8921）

没有任何调用点切换到 `ValidationPipeline::validate_with_preview`。

#### 证据 2：`StateLedger::commit` 在 production 中零调用

```bash
grep -rn "\.commit(" crates/ralph-core/src/ | grep -v test | grep -v "ledger.rs:"
# 输出: 仅 diagnosis/reporter.rs 的 git commit，与 ledger 无关
```

`StateLedger` 构造于 `event_loop/mod.rs:1977`（`build_state_ledger_from_env`），但构造后无任何 `commit()` 或 `commit_handoff_artifact()` 调用。

#### 证据 3：`emit_correction_context` 在 production 中零调用

```bash
grep -rn "emit_correction_context" crates/ | grep -v test | grep -v "mod.rs:"
# 输出: 0
```

`correction/mod.rs:310` 的 `emit_correction_context` 仅在单元测试和 `u7_correction.rs` 测试中被调用。`process_parse_result` 中的 10 余处 `publish_policy_rejection_resume` 调用（line 786, 1311, 1334, 1420, 1442, 1492, 1530, 1632, 1666）全部走 legacy `task.resume` 路径。

#### 证据 4：U10 验证报告存在虚假声明

`docs/plans/2026-06-21-002-unified-state-u10-verification.md` 第 181 行：
> "U4 validation pipeline 已替换所有 runtime 路径"

**与源码矛盾**。`event_loop/mod.rs` 未使用 `ValidationPipeline`。同文档第 180 行声称 `UNIFIED_VALIDATION` "等价于默认启用"——该 env var 在 event loop 中不存在，且 pipeline 未接入 runtime。

### 1.3 问题汇总（任务完成度）

| 优先级 | 问题 | 文件/行号 | 说明 |
|---|---|---|---|
| **P0** | `StateLedger` 构造后未接入 `process_parse_result` | `event_loop/mod.rs:1977` | 新 ledger 是 dead code，无 commit 调用点 |
| **P0** | `ValidationPipeline` 未接入 event loop | `event_loop/mod.rs:7330+` | 统一验证流水线未替换 legacy gate 栈 |
| **P0** | `emit_correction_context` 未接入 rejection 路径 | `correction/mod.rs:310` | U7a 核心目标未在生产中生效 |
| **P0** | `commit_handoff_artifact` 未接入 macro-edge 路径 | `state/ledger.rs:376` | U5 自动 handoff 未在生产中触发 |
| **P0** | U10 验证报告声明不实 | `docs/plans/2026-06-21-002-unified-state-u10-verification.md:180-181` | 声称已替换 runtime，实际未接入 |
| **P0** | 开启全部 flag 后 16 条测试失败 | U10 验证矩阵 | 5059/5075 通过，说明新路径未真正可用 |
| **P1** | 2 条 BDD `#[ignore]` | `tests/scenarios.rs:1728,1747` | 因 production wire-up 缺失被跳过 |

---

## 2. 回归风险：通过（有条件）

### 2.1 默认状态（flags 全关）

- 旧路径完整保留，无接口签名变更。
- `LoopState` 新增 `state_ledger: Option<StateLedger>` 和 `prompt_context: PromptContext` 字段，但默认均为 `None`/空，不触发行为变化。
- 5075 测试全绿，BDD 63/63 通过，smoke 57/57 通过。

**结论：无回归。**

### 2.2 风险点

| 优先级 | 风险 | 说明 |
|---|---|---|
| **P2** | 长期维护两套 gate 栈 | legacy gate + `ValidationPipeline` 并存，增加后续 merge 冲突和维护负担 |
| **P2** | `#[allow(dead_code)]` 字段积累 | `CommitDeltaView`（`pipeline.rs:85`）、`truncate_after`（`ledger.rs:504`）等标记为允许 dead code，说明接口与调用方存在 gap |

---

## 3. Bug 隐患：不通过

### 3.1 逐条隐患

#### P0-1：`apply_delta` 中 4 个变体为 no-op，replay 后状态丢失

**文件**: `crates/ralph-core/src/state/snapshot.rs:368-431`  
**问题**：
- `CommitDelta::HandoffAccepted` → no-op（注释说 "U2 will wire"）
- `CommitDelta::ReviewStepUpdated` → no-op
- `CommitDelta::HandoffTrackerUpdated` → no-op
- `CommitDelta::FlowLifecycleUpdated` → no-op

这些变体在 `replay_from_disk` 时被直接跳过，导致 `HandoffTracker`、`ReviewStepTracker`、`FlowLifecycleRegistry` 的持久化状态在冷启动后**无法重建**。如果进程重启，这些 tracker 的状态将丢失。

**建议**：要么在 `apply_delta` 中实现这些变体的具体逻辑，要么将 `todo!()` 替代 no-op 以在开发阶段暴露缺失，而非在生产中静默丢失数据。

#### P1-1：`validate_pre_commit` 默认路径使用空 `ProtocolView`

**文件**: `crates/ralph-core/src/validation/pipeline.rs:185-208`  
**问题**：`validate_pre_commit` 方法构造 `ProtocolView::default()` 作为空视图传入规则。虽然 `OriginRule` 和 `PublisherRule` 在设计上可以处理空视图，但 `RequiredFieldsRule` 依赖 `effective_required_fields`（来自 `ProtocolView`），空视图下该规则失效。

**建议**：删除 `validate_pre_commit`（仅保留 `validate_pre_commit_with_view`），或在文档中明确标注该方法为 "仅用于测试，不用于生产"。

#### P1-2：`CorrectionContext::render_block` 直接拼接用户输入到 prompt，存在注入风险

**文件**: `crates/ralph-core/src/correction/mod.rs:193-233`  
**问题**：`last_message` 和 `topic` 直接写入 prompt 字符串，未做 HTML/JSON 转义。如果拒绝信息包含 `<!--` 或指令分隔符（如 `\n\n### `），可能导致 prompt injection。

**建议**：对 `last_message` 和 `topic` 进行简单的 HTML 实体转义（`&lt;`、`&gt;`、`&amp;`）或至少过滤掉 `<!--` 和 `-->` 序列。

#### P2-1：`StateLedger::commit` 全量 clone `LedgerSnapshot`

**文件**: `crates/ralph-core/src/state/ledger.rs:245`  
**问题**：每次 commit 调用 `self.snapshot.clone()`，而 `LedgerSnapshot` 包含约 50 个字段、多个 `HashMap`/`HashSet`/`Vec`。高频 commit 场景下（如每 turn 多次事件），clone 开销可能显著。

**建议**：U2 注释已承认此问题（"U2 may introduce a narrower affected sub-state snapshot"），但尚未实现。建议在后续迭代中引入按 delta 类型选择性 clone 的机制。

#### P0-2（相对 U5 目标）：`handoff_path` 缺失时 gate 不触发 auto-generate

**文件**: `crates/ralph-core/src/event_loop/mod.rs:8627`  
**问题**：`FileContent::Missing` 直接传入 `evaluate_event`，返回 `Reject` 或 `NotRequired`。但 `StateLedger::commit_handoff_artifact`（U5 设计）在 `handoff_path` 缺失时应自动调用 `prepare_with_dedup` 生成 skeleton。当前 event loop 中无任何调用 `commit_handoff_artifact` 的代码，因此 auto-generate 逻辑从未触发。

---

## 4. 代码质量：通过（有优化项）

### 4.1 好的方面

- **模块结构清晰**：`state/` 下 `ledger.rs` + `snapshot.rs` + `commit.rs` + `recovery_log.rs` 职责分离；`validation/` 按 rule 拆分为 7 个独立文件；`correction/mod.rs` 单一职责（prompt injection）。
- **特性开关设计合理**：`UNIFIED_STATE_LEDGER`、`UNIFIED_PROTOCOL_VIEW`、`UNIFIED_DETERMINISTIC_CORRECTION` 的 env var 读取集中，不分散在业务逻辑中。
- **文档注释详尽**：每个新模块顶部引用 plan 文档，关键设计决策（KTD-1~KTD-8）均有注释说明。
- **测试结构良好**：`state::tests`（897 行）、`validation::tests`（505 行）、`correction::tests`（约 40 个 case）覆盖全面。

### 4.2 需改进

#### P2-1：`event_loop/mod.rs` 已膨胀至 10689 行

`process_parse_result` 超过 3300 行（从 line 7330 到 line ~10650），违反单一职责。即使 legacy 代码已存在，U4 的 `ValidationPipeline` 接入本应提供拆分机会，但未能实现。

#### P2-2：`#[allow(dead_code)]` 出现频率较高

| 位置 | 说明 |
|---|---|
| `validation/pipeline.rs:85` | `CommitDeltaView` 结构体 |
| `validation/pipeline.rs:95` | `CommitDeltaView::from_event` |
| `state/ledger.rs:504` | `truncate_after` 函数 |
| `state/ledger.rs:111` | `HandoffAcceptedInputs` 结构体（注释标记 U5） |

这些标记说明新代码的接口设计先于调用方，存在设计与实现的脱节。

#### P2-3：`CommitDelta` 中 4 个变体为 no-op

见维度 3 的 P0-1。设计与实现脱节的典型表现。

---

## 5. 测试覆盖：不通过

### 5.1 通过的方面

- `state::tests`（897 行）：覆盖 `LedgerSnapshot::apply_delta` 全部变体、`StateLedger::commit` 回滚、`replay_from_disk` 损坏恢复。
- `validation::tests`（505 行）：覆盖全部 7 个 rule 的接受/拒绝路径、`ValidationPipeline` 的 pre/post commit 阶段。
- `correction::tests`（约 40 个 case）：覆盖 `CorrectionContext` 构造、渲染、escalation 阈值、`PromptContext` 聚合。
- 默认状态下全 workspace 5075 测试通过。

### 5.2 未通过的方面

#### P1-1：2 条 BDD 被 `#[ignore]`

**文件**: `crates/ralph-core/tests/scenarios.rs:1728,1747`  
**原因**：`test_u9_correction_deterministic_scenario` 和 `test_u9_correction_three_escalation_scenario` 因 "production wire-up of `emit_correction_context`" 缺失被 `#[ignore]`。  
**影响**：U7a 的集成行为（deterministic correction 实际注入 prompt）在端到端测试中未验证。

#### P1-2：feature flags 全开时 16 条失败

详见 U10 验证报告 §1.2.1：
- **14 条 U6 unified pipeline 已知 gap**：`run_policy_check_unified` 使用 `LedgerSnapshot::cold_start()`（无历史事件上下文），导致 `business_after_terminal` 和 `duplicate_terminal` 语义与 legacy 路径不一致。
- **2 条测试隔离缺陷**：`u3_feature_flag_default_off_explicit_on` 和 `pipeline_records_protocol_view_feature_flag` 依赖 "env 未设置" 的状态，在进程级 env 共享下失败。

#### P1-3：无 production 集成测试

新路径（`StateLedger` → `ValidationPipeline` → `CorrectionContext`）在 event loop 中的实际行为**未在任何测试中验证**，因为 loop 未接入这些组件。这是 "代码存在但无行为验证" 的测试盲区。

---

## 6. 文档同步：不通过

### 6.1 计划要求的文档 vs 实际状态

| 计划要求 | 目标文档 | 实际状态 | 优先级 |
|---|---|---|---|
| "更新 `docs/report/2026-06-21-top-3-architectural-instability-factors.md` 的「修复方向」为「已实现」" | `docs/report/2026-06-21-top-3-architectural-instability-factors.md` | ❌ 未更新，仍描述旧架构问题（task.resume 循环、状态分散等） | **P1** |
| "在 `docs/brainstorms/2026-06-21-unified-orchestrator-state-requirements.md` 中记录最终架构决策" | `docs/brainstorms/2026-06-21-unified-orchestrator-state-requirements.md` | ❌ 未检查到更新 | **P1** |
| "更新 `docs/guide/runtime-diagnosis.md`，说明 `ralph diagnose` 现在从 ledger rejection log 读取根因" | `docs/guide/runtime-diagnosis.md` | ❌ 未提及 `ledger.jsonl`/`recovery.jsonl` 新路径；仍描述旧 session-scoped 模式 | **P1** |
| "更新 `crates/ralph-core/data/ralph-tools*.md` 等 skill 文档（按 AGENTS.md 反向验证规则）" | `crates/ralph-core/data/ralph-tools.md` 等 | ❌ 未提及 `correction`/`loop.resume`/`StateLedger` 等新概念 | **P1** |
| "在 PR 描述中清楚列出：被废弃/替换的测试、`#[ignore]` 列表及 follow-up issues、reason_code 命名空间变更、特性开关使用方法" | PR 描述 | ❌ 无 PR 描述文档 | **P2** |
| "同步更新本文件「Presets & Hats System」段的 builtin preset 列表" | `AGENTS.md` / `CLAUDE.md` | ⚠️ 已更新（`ce-executor-lite` 说明等） | 通过 |

### 6.2 新增文档问题

`docs/plans/2026-06-21-002-unified-state-u10-verification.md` 包含**与源码矛盾的虚假声明**（第 180-181 行），会误导后续维护者认为新路径已替换 runtime。这是计划文档自身的质量问题，需要修正。

---

## 7. 安全合规：通过（有关注点）

### 7.1 检查项

| 检查项 | 结果 | 说明 |
|---|---|---|
| 硬编码密钥/Token | ✅ 通过 | 新代码中无硬编码凭证 |
| 注入风险 | ⚠️ 需关注 | `correction/mod.rs:193-233` 的 `render_block` 直接拼接 `last_message`/`topic` 到 prompt，未做转义 |
| 敏感信息落入日志 | ✅ 通过 | `RejectionRecord` 写入 `.ralph/recovery.jsonl` 仅包含 hat/topic/reason_code，无敏感信息 |

### 7.2 具体关注点

**P1：prompt injection 风险**  
**文件**: `crates/ralph-core/src/correction/mod.rs:193-233`  
`CorrectionContext::render_block` 将 `last_message`（拒绝信息）和 `topic` 直接写入 prompt 字符串。如果拒绝信息包含恶意内容（如 `<!-- 忽略上述指令 -->` 或 `\n\n### 新的指令`），存在 prompt injection 风险。

**建议**：对 `last_message` 和 `topic` 进行简单的 HTML 实体转义或过滤（至少替换 `<` → `&lt;`、`>` → `&gt;`）。当前来源是系统内部校验输出，风险可控，但建议防御性编程。

---

## 优先级问题汇总

### P0（阻塞问题，必须修复）

| # | 问题 | 文件/行号 | 修改建议 |
|---|---|---|---|
| P0-1 | `StateLedger` 构造后无任何 `commit()` 调用点 | `event_loop/mod.rs:1977` | 在 `process_parse_result` 的事件接受路径中调用 `state_ledger.commit()`，将 `TaskLifecycle`/`ProgressUpdate` 等 delta 写入 ledger |
| P0-2 | `ValidationPipeline` 未接入 event loop | `event_loop/mod.rs:7330+` | 在 `process_parse_result` 中替换或包装 legacy gate 栈，调用 `pipeline.validate_with_preview()` |
| P0-3 | `emit_correction_context` 未接入 rejection 路径 | `correction/mod.rs:310` | 在 `publish_policy_rejection_resume` 的调用点（或替代该函数）调用 `emit_correction_context`，将 `CorrectionContext` 写入 `state.prompt_context` |
| P0-4 | `commit_handoff_artifact` 未接入 macro-edge 路径 | `state/ledger.rs:376` | 在 `process_parse_result` 的 `handoff_path` 缺失时，调用 `ledger.commit_handoff_artifact()` 自动生成 artifact |
| P0-5 | `apply_delta` 中 4 个变体为 no-op，replay 丢失状态 | `state/snapshot.rs:368-431` | 实现 `HandoffAccepted`/`ReviewStepUpdated`/`HandoffTrackerUpdated`/`FlowLifecycleUpdated` 的具体 delta 应用逻辑，或用 `todo!()` 暴露缺失 |
| P0-6 | U10 验证报告声明不实 | `docs/plans/2026-06-21-002-unified-state-u10-verification.md:180-181` | 修正为 "U4 validation pipeline 已实现在 `validation/` 模块中，但尚未接入 `event_loop/mod.rs` 的 production 路径" |
| P0-7 | 开启全部 flag 后 16 条测试失败 | U10 验证矩阵 | 修复 `run_policy_check_unified` 的 `LedgerSnapshot::cold_start()` 历史上下文缺失，以及 2 条测试的 env 隔离问题 |

### P1（重要问题，建议修复）

| # | 问题 | 文件/行号 | 修改建议 |
|---|---|---|---|
| P1-1 | `validate_pre_commit` 使用空 `ProtocolView` | `validation/pipeline.rs:185-208` | 删除 `validate_pre_commit` 或标注为 "测试专用，不用于生产"；生产入口强制使用 `validate_pre_commit_with_view` |
| P1-2 | `CorrectionContext::render_block` prompt injection 风险 | `correction/mod.rs:193-233` | 对 `last_message`/`topic` 做 HTML 实体转义（`&lt;`, `&gt;`, `&amp;`） |
| P1-3 | 2 条 BDD `#[ignore]` | `tests/scenarios.rs:1728,1747` | 接入 `emit_correction_context` 后移除 `#[ignore]` |
| P1-4 | 无 production 集成测试 | `event_loop/` 测试目录 | 新增 `event_loop/tests/u10_unified_path.rs`，在 `UNIFIED_STATE_LEDGER=1` 环境下验证 `process_parse_result` 走新路径 |
| P1-5 | `docs/report/...` 未更新 | `docs/report/2026-06-21-top-3-architectural-instability-factors.md` | 在报告末尾添加 "修复状态" 章节，标注已实现的模块和待接入的调用点 |
| P1-6 | `docs/guide/runtime-diagnosis.md` 未更新 | `docs/guide/runtime-diagnosis.md` | 添加 `ralph diagnose --from-ledger` 的使用说明，说明 ledger-based rejection log 的读取路径 |
| P1-7 | `ralph-tools*.md` 未更新 | `crates/ralph-core/data/ralph-tools*.md` | 按 AGENTS.md 反向验证规则，补充 `correction`/`loop.resume`/`StateLedger` 的说明 |

### P2（优化项，可选处理）

| # | 问题 | 文件/行号 | 修改建议 |
|---|---|---|---|
| P2-1 | `event_loop/mod.rs` 10689 行，`process_parse_result` 3300+ 行 | `event_loop/mod.rs` | 将 `process_parse_result` 拆分为 `pre_validate`/`commit`/`post_validate`/`project` 四个子函数，或提取到 `event_loop/batch_processor.rs` |
| P2-2 | `StateLedger::commit` 全量 clone snapshot | `state/ledger.rs:245` | 引入按 delta 类型选择性 clone 的机制（如 `LedgerSnapshot::clone_affected`），减少高频 commit 开销 |
| P2-3 | `#[allow(dead_code)]` 字段过多 | 多处 | 清理无调用方的接口，或标记明确的 TODO issue 号 |
| P2-4 | 长期维护两套 gate 栈 | 全项目 | 在 issue 中明确 legacy gate 的淘汰时间表（建议 1 个 minor version） |
| P2-5 | PR 描述缺失 | — | 补充 PR 描述，列明废弃测试、`#[ignore]` 列表、follow-up issues、reason_code 变更、flag 使用方法 |

---

## 修复计划与验收标准

### 阶段一：Production 接入（解除阻塞）

**目标**：让新路径在 event loop 中真正生效。

1. **接入 `StateLedger`**：
   - 在 `process_parse_result` 的事件接受路径中，将 `task`/`progress`/`workflow`/`handoff` 等状态变更转换为 `CommitDelta`，调用 `state_ledger.commit()`。
   - 在 `StateProjector::apply` 的调用点，增加 `apply_from_ledger` 分支（当 `state_ledger.is_some()` 时）。

2. **接入 `ValidationPipeline`**：
   - 在 `process_parse_result` 中，构造 `ValidationPipeline` 并调用 `validate_with_preview()`，将结果映射到现有的 `ProcessedEvents` 结构。
   - 保留 legacy 路径作为 fallback（当 `UNIFIED_STATE_LEDGER=0` 时）。

3. **接入 `emit_correction_context`**：
   - 替换 `publish_policy_rejection_resume` 的调用点（或在该函数内部增加分支），当 `is_correction_enabled()` 时调用 `emit_correction_context`。
   - 确保 `state.prompt_context` 在 `build_prompt` 时被正确读取。

4. **接入 `commit_handoff_artifact`**：
   - 在 `handoff_path` 缺失的 macro-edge 接受路径中，调用 `ledger.commit_handoff_artifact()`。

### 阶段二：补齐 delta 实现

- 实现 `snapshot.rs` 中 4 个 no-op `CommitDelta` 变体的具体逻辑，确保 `replay_from_disk` 能完整重建状态。

### 阶段三：测试修复

- 修复 `run_policy_check_unified` 的 `LedgerSnapshot::cold_start()` 历史上下文缺失（加载 `events.jsonl` 或 `ledger.jsonl` 的历史状态）。
- 修复 2 条 env 隔离测试（用 `serial_test` 或显式参数替代进程级 env）。
- 移除 2 条 BDD 的 `#[ignore]`。

### 阶段四：文档同步

- 更新 `docs/report/...` 的修复状态。
- 更新 `docs/guide/runtime-diagnosis.md` 的 ledger 路径说明。
- 更新 `ralph-tools*.md` 的新概念说明。
- 修正 U10 验证报告的虚假声明。

### 验收标准

- [ ] `event_loop/mod.rs` 中出现 `ValidationPipeline` 的调用。
- [ ] `StateLedger::commit` 在 `process_parse_result` 或 `StateProjector` 中被调用。
- [ ] `emit_correction_context` 在 rejection 路径中被调用。
- [ ] `commit_handoff_artifact` 在 macro-edge 路径中被调用。
- [ ] `snapshot.rs` 的 4 个 no-op `CommitDelta` 变体实现具体逻辑。
- [ ] 所有 feature flags 开启后 `./scripts/run-tests.sh` 全绿（0 失败）。
- [ ] `correction_deterministic` 和 `correction_three_escalation` BDD 的 `#[ignore]` 被移除并通过。
- [ ] `docs/report/...`、`docs/guide/runtime-diagnosis.md`、`ralph-tools*.md` 已更新。

---

## 总体评估

| 维度 | 结论 | 优先级 |
|---|---|---|
| **任务完成度** | 不通过 | 新模块代码完成但均未接入 production |
| **回归风险** | 通过（有条件） | 旧路径完整保留，无回归；但两套路径并存增加维护风险 |
| **Bug 隐患** | 不通过 | 4 个 CommitDelta no-op 导致 replay 状态丢失；prompt injection 风险 |
| **代码质量** | 通过（有优化项） | 结构清晰、文档详尽，但文件过大、dead code 过多 |
| **测试覆盖** | 不通过 | 2 条 BDD 被 ignore、16 条 flag-on 失败、无 production 集成测试 |
| **文档同步** | 不通过 | 4 处计划要求的文档更新未执行；U10 报告存在虚假声明 |
| **安全合规** | 通过（有关注点） | 无硬编码密钥；prompt 拼接存在注入风险 |

### 最终结论：阻塞（BLOCKED）

本次重构在**代码层面**（数据结构、模块设计、单元测试）上质量较高，但**在架构层面**（production 接入、端到端验证、文档同步）上未达到计划目标。核心问题可以归结为一句话：**"新代码实现了，但 event loop 没有使用它。"**

在以下问题修复之前，不建议合并到主线：
1. `ValidationPipeline`/`StateLedger`/`emit_correction_context` 必须接入 `process_parse_result`。
2. `snapshot.rs` 的 4 个 no-op `CommitDelta` 变体必须实现。
3. 全部 feature flags 开启后测试必须全绿。
4. U10 验证报告的虚假声明必须修正。
5. 计划要求的文档同步必须完成。

---

*报告生成日期：2026-06-22*  
*Reviewer：基于源码审查与测试矩阵分析*  
*计划参考：docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md*
