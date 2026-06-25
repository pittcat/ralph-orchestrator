---
title: 全量删除 hat_handoff 功能
type: refactor
status: completed
date: 2026-06-23
updated: 2026-06-25
origin: docs/brainstorms/2026-06-18-isolated-hat-handoff-requirements.md
related:
  - docs/plans/2026-06-24-001-refactor-ce-executor-serial-tdd-validator-plan.md
execution_order:
  - id: 2026-06-24-001
    reason: |
      2026-06-24-001 重写 ce-executor-serial preset (11→10-hat + TDD + validator + 总体 review)，
      会大幅修改本计划 U1/U5/U6/U7/U8 触及的下游文件：
      - presets/en/ce-executor-serial.yml、presets/schemas/ce-executor-serial.yml
      - crates/ralph-cli/src/presets.rs（PRESETS 数组 + 22+ 硬编码断言测试将合并到 5 个）
      - crates/ralph-core/tests/scenarios/step_handoff/（5 个 plan-gate scenario 将删除）
      - AGENTS.md / CLAUDE.md / .cursor/rules/multi-hat-isolation.mdc
      先合并 preset 架构再删 hat_handoff，可避免双重重构同一文件导致的中间态噪声。
  - id: 2026-06-23-006
    reason: 本计划
---

# 全量删除 hat_handoff 功能

## 概述

`hat_handoff` 是 isolated 模式下帽子间宏观边（macro-edge）的磁盘交接文件机制。该功能已在 `ce-executor-serial` 中显式关闭（`hat_handoff.enabled: false`），且是近期 30 天内 6 次 `hat_handoff_filename_mismatch` 死信复发的根源。本计划将其从代码、CLI、校验管线、状态账本、预设、文档中彻底移除，同时**不触碰 `step_handoff`**，确保 `ce-executor-serial` 的计划-进度对齐契约保持不变。

---

## 问题框定

- `hat_handoff` 引入了大量专用代码路径（gate、inject、emit instructions、allocator、validator、CLI 命令、诊断、恢复信封），但业务价值已被 task ownership 取代。
- 当前 active preset 已关闭该功能，代码处于“软废弃”状态，继续保留会增加维护成本和误触发风险。
- 删除范围必须严格限定为 `hat_handoff`，避免波及 `step_handoff`、`workflow_contract` 的通用 `HandoffIndex/HandoffTracker` 以及 session 结束时的 `.ralph/agent/handoff.md`。

---

## 对抗性审查 P0/P1 问题清单（2026-06-24 更新）

> 实施前必读。本节是对抗性审查后必须强制落地的 P0/P1 修复点；每条均给出**问题位置 → 处置方案 → 在 plan 哪个单元/段落实**。**不允许"按字面 plan 实施"——必须按本清单逐条核对**。

### P0 — 致命回归风险（4 项）

#### P0-1：`tests/scenarios/serial_lint/` 与 `correction_*.yml` 共 6 个 scenario 含 hat_handoff 触发器，删除 hat_handoff 后必然失败

| 文件 | 命中位置 | 触发机制 | 处置 |
|---|---|---|---|
| `serial_lint/serial_lint_7_handoff_seeds_coverage.yaml` | line 10, 12, 39, 68（5 处 hat_handoff 引用）+ `hat_handoff:` 配置块 | 2-hat 拓扑 + `hat_handoff_gate.from_hat` 契约 | **改写或删除整个文件**——plan U7 初稿误判"保留"是错误的 |
| `serial_lint/serial_lint_11_isolated_unaffected.yaml` | line 18, 22, 26, 52, 57（5 处 hat_handoff_gate 引用） | "isolated mode is unaffected" 测试目标依赖 hat_handoff_gate 被旁路 | **改用 origin guard 触发或删除整个文件** |
| `serial_lint/serial_lint_6_handoff_auto_prepare.yaml` | line 38 `hat_handoff: { enabled: true }` | linter auto_prepare 触发器 | **改用 event_policy 触发或删除** |
| `serial_lint/serial_lint_2_rejection_digest.yaml` | line 12-14, 18, 48, 53, 73-75 | 显式验证 `hat_handoff_gate.rejection_digest_contains` 调用点 | **改用 origin guard 触发或删除整个 scenario** |
| `tests/scenarios/cli_runtime_parity.yml` | line 9, 43, 50, 70, 71, 96（6 处 hat_handoff 引用） | "hat_handoff_gate 只写 recent_rejection_digest" 是核心测试目标 | **删除 hat_handoff 段或删整文件** |
| `tests/scenarios/correction_deterministic.yml` + `correction_three_escalation.yml` | line 44 / line 43 `hat_handoff:` 配置块 | 把 `hat_handoff.enabled: true` 作为配置噪音触发 escalation | **删除 `hat_handoff:` 配置块；如块是测试目标则改写或删 scenario** |

**落地位置**：U7 文件清单已显式列入 6 个文件路径 + 处置方案。

#### P0-2：`step_handoff/progress_task_mismatch.yml` 与 `step_advance_u1_to_u2.yml` 内嵌 `plan-gate` hat，与 `2026-06-24-001 U5` 直接冲突

- 这两个 scenario 在 `config.hats` 中显式声明 `plan-gate` subscribes `["work.done", "review.passed"]` 并 publishes `["queue.advance", "plan.complete"]`，新架构（11→10-hat）下 plan-gate 已删除，scenario 无法解析。
- `2026-06-24-001 U5` 已显式删除这 2 个 scenario。
- 本 plan U7 初稿**误判保留**——会与 2026-06-24-001 冲突。

**处置**：U7 改为"以 2026-06-24-001 U5 为准；待其落地后，按 scenario 主体是否引用 `plan-gate` / `debug-resolver` 分类删除"。**`progress_task_mismatch.yml` / `step_advance_u1_to_u2.yml` / `state_projection_work_done_updates_progress.yml` 必须删**。

**落地位置**：U7 文件清单「冲突 — 修正为以 2026-06-24-001 U5 为准」段。

#### P0-3：`state_projector/mod.rs` line 580, 595 含 `CommitDelta::HandoffAccepted/HandoffTrackerUpdated` match 分支，删除变体后未同步删除会编译失败

```rust
// state_projector/mod.rs:580
match delta { … :HandoffAccepted { .. } … }
// state_projector/mod.rs:595
match delta { … :HandoffTrackerUpdated { .. } … }
```

**处置**：U5 显式列入 `state_projector/mod.rs`，删除 line 580 与 line 595 的 match 分支。

**落地位置**：U5 文件清单已补 `state_projector/mod.rs`。

#### P0-4：`presets.rs` 误描述（0 命中但 U5 声称"删含 hat_handoff 的项"）+ `preflight.rs` 测试函数名漏列 + `runtime_state.rs` `hat_handoff_next_seq` 字段漏列

| 子问题 | 文件 | 实际状态 | 处置 |
|---|---|---|---|
| a. `presets.rs` 0 命中 | `crates/ralph-cli/src/presets.rs` | `rg hat_handoff` 返回 0 命中；所有 `test_ce_executor_*` 函数体均不引用 hat_handoff | U5 改为"跳过 `presets.rs`；该文件由 2026-06-24-001 U4 22→5 合并独立完成" |
| b. `preflight.rs` 两个测试函数名 | `crates/ralph-cli/src/preflight.rs:1521` / `:1569` | `merge_hats_overlay_preserves_preset_hat_handoff_when_operator_omits_it` / `merge_hats_overlay_lets_operator_override_preset_hat_handoff` | U5 列具体函数名，实施时按名删 |
| c. `runtime_state.rs` 字段漏列 | `crates/ralph-core/src/runtime_state.rs:55` | `hat_handoff_next_seq: Option<u32>` 字段（与 `hat_handoff_seq` / `hat_handoff_dir` 并列） | U8 显式补 `hat_handoff_next_seq` 字段删除要求 |

**落地位置**：U5 方法段（presets.rs 修正说明）+ U5 文件清单（preflight.rs）+ U8 文件清单（runtime_state 字段补全）。

### P1 — 严重遗漏（3 项）

#### P1-1：`presets/schemas/ce-executor-serial.yml` line 462-472 顶层 `hat_handoff:` 块，本 plan 与 2026-06-24-001 都不完整

```yaml
# presets/schemas/ce-executor-serial.yml:462-472
hat_handoff:
  enabled: false
  artifact:
    required_sections: 5
    require_next_marker: true
  linter:
    auto_prepare_on_macro_edge: false
  exempt_topics: [...]
```

- 本 plan U6 声称删该顶层块，但行号描述不准（`rs` 实测行号与初稿不同）。
- `2026-06-24-001 U2` 重写 `schemas` 段但**不删顶层块**——形成空白。

**处置**：U6 待 2026-06-24-001 U2 落地后，按 `rg -n 'hat_handoff' presets/schemas/ce-executor-serial.yml` 实际命中位置删除顶层块；按内容删，不按行号。

**落地位置**：U6 文件清单「修改 `presets/schemas/ce-executor-serial.yml`」段已显式补顶层块位置 + 实施顺序。

#### P1-2：`event_loop/mod.rs` 38 处 hat_handoff 引用需二进制区分（专用删 vs 通用保留）

`event_loop/mod.rs` 是改动最复杂的文件，必须严格区分两类调用：

| 类型 | 位置（line） | 处置 |
|---|---|---|
| **通用 `HandoffTracker` 调用**（必须保留） | 350, 763, 783, 907, 928, 2230, 2594, 5762, 5771, 6459, 6461, 8380 | **不删**（WAC-U5 / WRC-U4 基础设施，与 hat_handoff 无关） |
| **专用 hat_handoff 调用**（必须删） | 3487, 3496, 3622, 3624, 3668, 4819, 4821-4828, 4951-4956, 5127, 5754, 7480-7727, 7832, 7909, 8401, 9703, 7596-7721 | **全删**（gate 调用、prompt 注入、emit 指令、recovery 派发、`prepend_hat_handoff_from_pending`、`process_parse_result` 分支） |

**处置**：U3 实施时按 `rg -n 'hat_handoff|HatHandoff' crates/ralph-core/src/event_loop/mod.rs` 输出逐行核对；通用行不动，专用行整段删除。

**落地位置**：U3 文件清单已用 `event_loop/mod.rs` 列表 + 风险表 U3 行明示。

#### P1-3：3 处源码注释引用被删类型/字样，删除后注释悬空

| 文件 | 位置 | 注释内容 | 处置 |
|---|---|---|---|
| `crates/ralph-core/src/correction/mod.rs` | line 69 | `/// Pipeline stage that rejected the event (\`origin\`, \`policy\`, \`hat_handoff\`, etc.)` | 删除 `hat_handoff` 字样 |
| `crates/ralph-core/src/step_handoff/progress_task_gate.rs` | line 262 | `crate::hat_handoff::gate::GateDecision`（引用被删类型） | 删除整行注释或改为通用名 |
| `crates/ralph-core/src/workflow_contract/handoff_tracker.rs` | line 164 | `Used by the hat_handoff gate to roll …` | 改写为通用名（如"by hat subscribers"） |

**落地位置**：U8 文件清单「源码注释清理」段已显式列入。

### 处置落地总表（与 plan 单元/段对齐）

| P0/P1 编号 | plan 单元/段 | 关键动作 |
|---|---|---|
| P0-1 | U7 文件清单 | 显式处理 6 个 scenario（serial_lint_2/6/7/11 + cli_runtime_parity + correction_2 个） |
| P0-2 | U7「冲突修正」段 | `progress_task_mismatch.yml` / `step_advance_u1_to_u2.yml` / `state_projection_work_done_updates_progress.yml` 必须删 |
| P0-3 | U5 文件清单 | 补 `state_projector/mod.rs` |
| P0-4a | U5「方法」段 | 跳过 `presets.rs` |
| P0-4b | U5 文件清单 | 列 `preflight.rs` 两个测试函数名 |
| P0-4c | U8 文件清单 | 补 `runtime_state.rs:55` `hat_handoff_next_seq` 字段 |
| P1-1 | U6 文件清单 | `presets/schemas/ce-executor-serial.yml` 顶层块删除 |
| P1-2 | U3 文件清单 + 风险表 | event_loop/mod.rs 38 处二进制区分 |
| P1-3 | U8 文件清单 | 3 处源码注释清理 |

**验证门**：U9 实施前必须用 `rg -n 'hat_handoff|HatHandoff|HAT_HANDOFF' crates/ralph-core/src/ crates/ralph-cli/src/ presets/` 全扫，结果中除 `step_handoff` / `HandoffTracker` / `HandoffIndex` / session `handoff_path` 合法命中外，其余全部为零才算 P0/P1 全部消化。

---

## 需求追溯

- R1. 删除 `hat_handoff` 运行时 gate、prompt 注入、emit 指令生成与 recovery 路径。
- R2. 保持 `ce-executor-serial` 的运行时行为不变（`hat_handoff.enabled: false` 已是现状）。
- R3. 保持 `step_handoff` 完整，不删除、不关闭、不改配置。
- R4. 删除所有仅服务于 `hat_handoff` 的单元/集成/BDD 测试。
- R5. 删除或更新 `hat_handoff` 相关的 skill 文档、运行手册、脚本、zsh 补全。
- R6. 全量回归通过 `./scripts/run-tests.sh`。

---

## 范围边界

- **在范围内**：`crates/ralph-core/src/hat_handoff/`、`ralph tools handoff`、`ralph audit hat-handoff`、校验管线中的 `HatHandoffRule`、event loop 中的 hat-handoff 分支、状态账本中的 hat-handoff delta、预设/模式中的 `hat_handoff` 配置块、相关测试与文档。
- **不在范围内**：
  - `step_handoff`（`crates/ralph-core/src/step_handoff/`、`validation/rules_step_handoff.rs`、相关测试与配置）。
  - `workflow_contract::HandoffIndex` 与 `HandoffTracker`（通用 WAC-U5 / WRC-U4 基础设施，本计划仅验证后保留）。
  - session 结束时由 `HandoffWriter` 生成的 `.ralph/agent/handoff.md`（属于 loop 会话交接，非 hat 间交接）。
  - `docs/report/`、`docs/achieved/` 中的历史报告与计划（仅作存档，不修改）。

### 延后工作

- 无。

### 与 2026-06-24-001 的下游文件重叠（执行顺序依赖）

2026-06-24-001 plan（ce-executor-serial preset 重写 11→10-hat）会在本计划实施前先行落地，二者会触及同一批下游文件。为避免双重重构同一文件造成的中间态噪声，本计划应**在 2026-06-24-001 完成后执行**。重叠清单：

| 文件 | 本计划涉及单元 | 2026-06-24-001 涉及单元 | 合并方式 |
|---|---|---|---|
| `presets/en/ce-executor-serial.yml` | U6（删 `hat_handoff` 块与 instructions 段落） | U1（11→10-hat 全量重写） | 等 2026-06-24-001 落地后，再扫一次 hat_handoff 残留并删段 |
| `presets/schemas/ce-executor-serial.yml` | U6（删 `hat_handoff` schema/合约字段） | U2（SSOT schema 全量重写） | 同上 |
| `crates/ralph-cli/src/presets.rs` | U5（删 ce_executor_serial 硬编码断言中含 hat_handoff 的项；动 `PRESETS` 数组） | U4（合并 22+ 硬编码断言到 5 个核心测试） | 必须串行：先 U4 收敛测试面，再 U5 删 hat_handoff 相关断言 |
| `crates/ralph-core/tests/scenarios/` | U7（删 `hat_handoff/` 目录、`correction_*.yml`、`serial_lint_*.yaml` 中 hat_handoff 触发器） | U5（删 5 个 plan-gate step_handoff scenario、调整 `ce_executor_*.yml`） | 合并后一次性扫：保留 `step_handoff/` 中非 plan-gate 用例 |
| `AGENTS.md` / `CLAUDE.md` | U8（同步 builtin preset 列表） | U6（11→10-hat 描述同步） | U8 复核时一并改 11→10-hat 描述 + 删 hat_handoff 引用 |
| `.cursor/rules/multi-hat-isolation.mdc` | U8 复核 | U6（ce-executor-serial hat 列表同步） | 同上 |
| `scripts/ralph-zsh-plugin.zsh` | U1 复核（hat_handoff 子命令） | U6 复核（preset 名字不变，无需动） | 仅作交叉复核，无主动改动 |

> **执行节奏建议**：先合 `2026-06-24-001` PR（保留 `hat_handoff.enabled: false` 与相关 schema 字段），再开 `2026-06-23-006` PR。这样每个 PR 改动面收敛，review 更易聚焦。

---

## 背景与研究

### 相关代码与模式

- `crates/ralph-core/src/hat_handoff/` — gate、allocator、validator、inject、emit_instructions、macro_edges、payload。
- `crates/ralph-core/src/event_loop/mod.rs` — gate 调用、tracker 记录、`task.resume` / `plan.blocked` 派发、prompt 注入。
- `crates/ralph-core/src/event_loop/loop_state.rs` — `hat_handoff_seq`、`pending_handoff_artifacts`。
- `crates/ralph-core/src/event_loop/rejection.rs` — typed `RejectionKind` / `RejectionStage` handoff 变体、`RejectionEscalator`、`CoordinatorDispatcher`。
- `crates/ralph-core/src/runtime_state.rs` — `RuntimeStateSnapshot` handoff 字段与 `HAT_HANDOFF_DEFAULT_DIR`。
- `crates/ralph-core/src/validation/rules_hat_handoff.rs`、`validation/result.rs`、`validation/pipeline.rs`、`validation/rules_publisher.rs` — `HatHandoffRule`、校验阶段、stale 注释。
- `crates/ralph-core/src/state/ledger.rs`、`state/commit.rs`、`state/snapshot.rs`、`state/mod.rs` — `HandoffAccepted` delta、`CounterKind::HatHandoffSeq`、相关 re-export。
- `crates/ralph-core/src/preset/engine/{gates.rs,hint.rs,linter.rs,protocol.rs}` — `RejectionKind`、macro-edge handoff_path、`LintFailureClass::HandoffArtifact`、hash 输入。
- `crates/ralph-core/src/loop_context.rs` — `hat_handoff_dir()`。
- `crates/ralph-core/src/diagnosis/mod.rs`、`diagnosis/reporter.rs` — `hat_handoff` stage/source 映射与提示分支。
- `crates/ralph-core/src/summary_writer.rs` — 测试构造 `LoopState` 时使用 `hat_handoff_seq` / `pending_handoff_artifacts`。
- `crates/ralph-cli/src/{handoff_cli.rs,commands/audit_hat_handoff.rs,commands/mod.rs,policy_check.rs,main.rs,tools.rs,preflight.rs,config_resolution.rs}` — CLI 面与 preset opt-in 列表。
- `crates/ralph-cli/build.rs`、`src/preset_merge_table.rs` — 预设合并映射。
- `presets/schemas/ce-executor-serial.yml`、`presets/en/ce-executor-serial.yml` — 配置、注释与 hat instructions。
- `crates/ralph-core/data/ralph-tools-handoff.md`、`.claude/skills/ralph-tools/SKILL.md`、相关文档。

### 制度经验

- `docs/solutions/integration-issues/hat_handoff_filename_mismatch_recurrence.md` 记录了 30 天内 6 次复发的根因：SSOT 文件名与运行时文件名漂移。
- `docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md` 是原功能运行手册。
- `presets/en/ce-executor-serial.yml` 已在 2026-06-23 显式关闭 `hat_handoff`，说明产品决策已倾向移除。

### 外部参考

- 无。

---

## 关键技术决策

- **KTD1 — `HandoffIndex/HandoffTracker` 保留**：它们属于 `workflow_contract` 的通用调度/超时基础设施，服务于 WAC-U5 优先级抢占与 WRC-U4 超时恢复，不仅用于 `hat_handoff`。实施前用 `rg` 验证调用方；若确认无其他调用方再删除。
- **KTD2 — 直接删除而非弃用**：根据 `AGENTS.md`“Backwards compatibility doesn't matter”，直接移除 CLI 命令、校验阶段、配置块，不保留兼容层。
- **KTD3 — 先删代码再删测试**：按依赖顺序（core → cli → presets → docs/tests）逐步移除，每步后 `cargo build` 保证编译通过，最后统一清理测试。
- **KTD4 — `step_handoff` 隔离**：任何涉及 `step_handoff` 的模块只读不改；grep 时排除 `step_handoff` 路径，防止误删。
- **KTD5 — `macro_edges_resolved/is_macro_edge` 一并删除**：当前这些字段/方法仅被 hat_handoff 路径使用（`rules_hat_handoff`、linter auto_prepare、gates handoff_path 检查）。实施前再次用 `rg` 确认；确认后随 `hat_handoff` 删除，不保留死代码。
- **KTD6 — 文档只删活手册、不动历史**：`docs/solutions/` 中的运行手册与复发复盘需要清理或标注废弃；`docs/report/`/`docs/plans/` 历史文件保留。
- **KTD7 — 与 `2026-06-24-001` 串行执行，避免双重重构**：`2026-06-24-001` 重写 `ce-executor-serial` preset（11→10-hat，新增 `validator` hat，TDD executor，总体 review），会大幅修改本计划 U1/U5/U6/U7/U8 触及的同一批下游文件（详见「范围边界」段下游重叠表）。本计划应在 `2026-06-24-001` PR 合入并稳定后再开工；如确实需要并行，必须先与 `2026-06-24-001` 的 owner 协调文件级拆分边界，否则会出现重复 diff 与冲突。
- **KTD8 — U5/U6/U7/U8/U9 描述补全（对抗性审查 2026-06-24）**：本计划初稿存在 4 处"事实性漏列"（`presets.rs` 0 命中但 U5 声称"删含 hat_handoff 的项"；`serial_lint_7_handoff_seeds_coverage.yaml` 含 5 处 hat_handoff 但 U7 声称"保留"；`progress_task_mismatch.yml` 与 2026-06-24-001 U5 冲突；`cli_runtime_parity.yml` 漏列）。实施前必须按 U5/U6/U7/U8/U9「对抗性审查补全」段执行增补；不允许"按字面 plan 实施"——字面 plan 实施会导致 `cargo build` 失败、`./scripts/run-tests.sh` 红。
- **KTD9 — 残留扫描排除边界严格化**：U9 的 `rg` 排除规则必须扩展，初稿 `-g '!**/handoff.rs' -g '!**/landing.rs'` 不充分。还需排除：① `docs/achieved/`、`docs/report/`、`docs/brainstorms/`、`docs/superpowers/` 历史目录；② `ralph-loop-diagnosis-report.md`、`deviation-report.md` 仓库根未追踪文件；③ `crates/ralph-core/data/ralph-tools-handoff.md` 与 `.claude/skills/ralph-tools/SKILL.md`（已删文件不应再被扫描）。**反向验证**：实施前必须运行 `rg 'hat_handoff|HatHandoff' .`（无排除），人工核对所有命中确实属于历史/未追踪文件，否则还有遗漏。

---

## 待解决问题

### 规划中已解决

- **Q1. 是否删除 `step_handoff`？** 否。它在 `ce-executor-serial` 中启用，保护 `progress.md` ↔ `tasks.jsonl` 一致性，删除会导致行为回归。
- **Q2. `HandoffIndex/HandoffTracker` 是否随 `hat_handoff` 删除？** 否。它们是 `workflow_contract` 通用设施；本计划仅移除 `hat_handoff` 专用调用。
- **Q3. 是否保留 `ralph tools handoff` 命令？** 否。该命令仅用于生成 hat-handoff 文件，功能删除后无存在必要。
- **Q4. `macro_edges_resolved/is_macro_edge/HandoffArtifact` 是否保留？** 否。当前仅被 hat_handoff 路径使用，一并删除。

### 延后到实施

- **Q5. event loop 中 `process_parse_result` 内与 `hat_handoff` 交织的分支具体行号**：需要在实施时精读后删除。
- **Q6. 旧 `ledger.jsonl` / `recovery.jsonl` 中含已删除 `CommitDelta` / `RejectionKind` 变体时的错误处理**：实施时确认 `replay_from_disk` 不会 panic，并在 U4/U9 风险表中记录为可接受的数据废弃。
- **Q7. `serial_lint/*.yaml` 与 `correction_*.yml` 中把 `hat_handoff.enabled: true` 作为触发器的场景**：实施时分类处理，改用 event_policy 或 origin guard 触发，或直接删除配置噪音。
- **Q8. 与 `2026-06-24-001` 的执行节奏确认**：开工前确认该 PR 已合入并 CI 绿；若仍未合入，需与 owner 协调文件级拆分边界（如 `presets.rs` 的 22→5 测试合并 + hat_handoff 断言删除，可由同一 PR 一次性收尾，避免两次拆解）。

---

## 实施单元

> **全局前置依赖（2026-06-24 更新）**：本计划所有单元开工前，须确认 `2026-06-24-001-refactor-ce-executor-serial-tdd-validator-plan.md` 的 PR 已合入且 CI 绿，否则 U5/U6/U7/U8 会与该 PR 的 diff 严重冲突。详见 KTD7 与「范围边界」下游重叠表。

- [ ] U1. **清理 CLI 面与相关脚本**

**目标：** 删除所有用户可见的 `hat_handoff` CLI 命令与工具文档。

**需求：** R1、R5

**依赖：** 无

**文件：**
- 删除：`crates/ralph-cli/src/handoff_cli.rs`
- 删除：`crates/ralph-cli/src/commands/audit_hat_handoff.rs`
- 修改：`crates/ralph-cli/src/commands/mod.rs`（移除 `pub mod audit_hat_handoff;`）
- 修改：`crates/ralph-cli/src/main.rs`（移除 `Commands::AuditHatHandoff` 分支）
- 修改：`crates/ralph-cli/src/tools.rs`（移除 `ToolsCommands::Handoff`）
- 修改：`crates/ralph-cli/src/policy_check.rs`（移除 `check_hat_handoff_gate` 及相关调用）
- 修改：`crates/ralph-cli/src/commands/emit.rs`、`crates/ralph-cli/src/wave.rs`（移除 hat-handoff policy check 调用）
- 删除：`crates/ralph-core/data/ralph-tools-handoff.md`
- 修改：`crates/ralph-core/data/ralph-tools.md`、`ralph-tools-emit.md`（删除指向 `ralph tools handoff` 的段落）
- 修改：`crates/ralph-core/data/doppelganger-functions.md`（若含 handoff 命令示例则删除）
- 删除：`scripts/audit-hat-handoff-artifacts.sh`
- 复核：`scripts/ralph-zsh-plugin.zsh`（当前已无可命中项；如未来有则移除）

**方法：**
- 先删除独立的 subcommand 文件，再从 dispatch 表与 re-export 中移除。
- `policy_check.rs` 中仅删除 hat-handoff 相关函数，保留 `check_step_handoff_gate`。

**测试场景：**
- Happy path：`ralph tools --help` 不再列出 `handoff` 子命令。
- Happy path：`ralph audit --help` 不再列出 `hat-handoff` 子命令。
- Error path：尝试执行已删除命令时返回未知子命令错误（由 clap 自动保证）。
- Integration：`ralph emit --policy-check` 对非 handoff 事件仍正常工作。

**验证：**
- `cargo build -p ralph-cli` 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- policy_check` 中仅保留的测试通过（handoff 测试在 U7 删除）。

---

- [ ] U2. **移除校验管线与诊断中的 HatHandoffRule**

**目标：** 从统一校验管线、诊断映射和拒绝阶段中注销 hat-handoff。

**需求：** R1、R4、R5

**依赖：** 无（与 U1 可并行开始）

**文件：**
- 删除：`crates/ralph-core/src/validation/rules_hat_handoff.rs`
- 修改：`crates/ralph-core/src/validation/result.rs`（移除 `ValidationStage::HatHandoff`、`ReasonCode::HAT_HANDOFF_*`）
- 修改：`crates/ralph-core/src/validation/pipeline.rs`（从 `pre_commit_rules` 移除 `HatHandoffRule` 注册）
- 修改：`crates/ralph-core/src/validation/mod.rs`（如有 re-export 则移除）
- 修改：`crates/ralph-core/src/validation/rules_publisher.rs`（删除 stale 注释中关于 SSOT hat_handoff allow-list 的描述）
- 修改：`crates/ralph-core/src/validation/tests.rs`（删除 `HatHandoffRule` 相关测试）
- 修改：`crates/ralph-core/src/event_loop/rejection.rs`（移除 `RejectionStage::HatHandoff`、handoff 变体在 `RejectionEscalator::check` 与 `CoordinatorDispatcher::dispatch` 中的分支、相关测试）
- 修改：`crates/ralph-core/src/diagnosis/mod.rs`（移除 `"hat_handoff" => "hat_handoff_gate"` 映射）
- 修改：`crates/ralph-core/src/diagnosis/reporter.rs`（移除 `("hat_handoff", _)` 提示分支及对应测试）

**方法：**
- 若 `ValidationStage` / `RejectionStage` 变体被序列化后落盘，需要确认旧数据路径；根据 KTD2 直接删除，不保留兼容层。
- 保留 `ValidationStage::StepHandoff` 与对应原因码。

**测试场景：**
- Happy path：启用 `ce-executor-serial` preset 时，`ValidationPipeline` 不再包含 `HatHandoffRule`。
- Edge case：`ValidationStage` / `RejectionStage` 变体顺序改变后，相关单元测试仍通过。
- Error path：带 `handoff_path` 的宏观边事件不再因 hat-handoff 规则被拒收。

**验证：**
- `cargo build -p ralph-core` 通过。
- `cargo nextest run -p ralph-core -- validation` 通过。

---

- [ ] U3. **从 Event Loop 与相邻模块移除 hat-handoff 分支**

**目标：** 删除 event loop 中的 gate 调用、prompt 注入、emit 指令、recovery 派发与运行时状态字段，并清理相邻模块中的死代码。

**需求：** R1、R2、R4

**依赖：** U2（校验阶段与拒绝阶段已移除）

**文件：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`
  - 移除 `handoff_index` 字段（若确认仅 hat_handoff 使用；否则保留）。
  - 移除 `process_parse_result` 中的 `hat_handoff::gate::evaluate_event` 调用。
  - 移除 `hat_handoff_seq` 自增、`HandoffTracker::on_handoff_accepted` 的 hat-handoff 专用调用。
  - 移除 `prepend_hat_handoff_from_pending` 与 `build_prompt` 中的 `hat_handoff::emit_instructions` / `inject` 调用。
  - 移除 `extract_handoff_path_from_payload` 等专用 helper。
  - 移除 `process_output` 中仅因 hat-handoff 产生的 recovery 分支（保留通用 `HandoffTracker::expired()` 超时恢复）。
- 修改：`crates/ralph-core/src/event_loop/loop_state.rs`（移除 `hat_handoff_seq`、`pending_handoff_artifacts`、`register_pending_handoff`、`consume_pending_handoff`）
- 修改：`crates/ralph-core/src/runtime_state.rs`（移除 `RuntimeStateSnapshot` handoff 字段、`HandoffSnapshotState`、`HAT_HANDOFF_DEFAULT_DIR`）
- 修改：`crates/ralph-core/src/loop_context.rs`（移除 `hat_handoff_dir()`；保留 `handoff_path()` 等 session handoff 方法）
- 修改：`crates/ralph-cli/src/loop_runner/runner.rs`（移除 `RALPH_HAT_HANDOFF_SEQ` 环境变量注入）
- 修改：`crates/ralph-core/src/summary_writer.rs`（移除测试中构造 `LoopState` 时使用的 `hat_handoff_seq` / `pending_handoff_artifacts` 字段）

**方法：**
- 使用 `rg` 定位 `hat_handoff` 在 `event_loop/mod.rs` 中的所有调用点，逐个删除并兜底为“直接放行”。
- `HandoffIndex/HandoffTracker` 的通用调用保留；只删除 `hat_handoff_seq` 等专用状态。

**测试场景：**
- Happy path：isolated 模式下 macro-edge 事件（如 `work.ready`）不带 `handoff_path` 也能正常通过。
- Integration：`ce-executor-serial` BDD scenario 中不再出现 `## HAT HANDOFF` prompt 注入块。
- Error path：handoff 相关 rejection reason 不再出现在 `task.resume` payload。

**验证：**
- `cargo build -p ralph-core` 通过。
- `cargo nextest run -p ralph-core -- event_loop` 通过（handoff 专用测试在 U7 删除）。

---

- [ ] U4. **清理状态账本与恢复日志中的 handoff delta**

**目标：** 删除状态账本中仅服务于 hat-handoff 的 commit delta、ledger 方法、计数器与恢复日志工厂。

**需求：** R1、R4

**依赖：** U3（event loop 不再调用这些方法）

**文件：**
- 修改：`crates/ralph-core/src/state/ledger.rs`（删除 `commit_handoff_artifact` 函数、`HandoffAcceptedInputs`、`HandoffCommitOutcome`）
- 修改：`crates/ralph-core/src/state/commit.rs`（删除 `CommitDelta::HandoffAccepted`、`CommitDelta::HandoffTrackerUpdated`、`CounterKind::HatHandoffSeq`）
- 修改：`crates/ralph-core/src/state/snapshot.rs`（删除 `handoff_accepted_log`、`handoff_tracker_log`、`hat_handoff_seq`，仅当确认无其他调用方）
- 修改：`crates/ralph-core/src/state/recovery_log.rs`（删除 handoff rejection 的 typed/legacy 工厂函数）
- 修改：`crates/ralph-core/src/correction/mod.rs`（简化 typed/legacy 选择逻辑）
- 修改：`crates/ralph-core/src/state/mod.rs`（移除 `HandoffAcceptedInputs`、`HandoffCommitOutcome` re-export）
- 修改：`crates/ralph-core/src/state/tests.rs`（删除 `u5_commit_handoff_artifact_*` 测试族与 `HAT_HANDOFF_DIR` import）

**方法：**
- 删除前用 `rg 'handoff_accepted_log\|handoff_tracker_log\|HandoffAccepted\|HandoffCommitOutcome\|HatHandoffSeq\|hat_handoff_seq'` 确认无其他调用方。
- 若 `LedgerSnapshot.handoff_tracker` 被通用 `HandoffTracker` 重放使用，则保留；否则删除。
- 旧 `ledger.jsonl` 中若包含已删除的 delta 变体，反序列化会报解析错误；按可接受数据废弃处理，必要时引导用户用 `ralph loops clean --ledger` 截断。

**测试场景：**
- Happy path：`state` 模块编译与单元测试通过。
- Edge case：旧 ledger 中 `"kind":"handoff_accepted"` 行被记录为 corrupt line，不阻塞启动。
- Error path：运行时不再生成 `CommitDelta::HandoffAccepted`。

**验证：**
- `cargo build -p ralph-core` 通过。
- `cargo nextest run -p ralph-core -- state` 通过。

---

- [ ] U5. **删除 hat_handoff 模块与核心配置**

**目标：** 删除 `hat_handoff` 模块目录及 `EventLoopConfig` 中的配置字段，并清理 preset engine 中的专用逻辑。

**需求：** R1、R4

**依赖：** U3、U4（所有调用方已清理）

**文件：**
- 删除：`crates/ralph-core/src/hat_handoff/` 整个目录
- 修改：`crates/ralph-core/src/lib.rs`（移除 `pub mod hat_handoff`）
- 修改：`crates/ralph-core/src/config/loop_config.rs`（移除 `EventLoopConfig.hat_handoff` 字段与 `HatHandoffConfig` 解析）
- 修改：`crates/ralph-core/src/config/mod.rs`（移除相关 re-export）
- 修改：`crates/ralph-core/src/preset/engine/protocol.rs`（移除 `ProtocolView.hat_handoff`、从 hash 输入删除 `hat_handoff`、删除 `macro_edges_resolved` / `macro_edge_consumers` / `is_macro_edge*` / `resolve_macro_edges` 等仅 hat_handoff 使用的方法、更新/删除相关测试）
- 修改：`crates/ralph-core/src/preset/engine/gates.rs`（移除 `RejectionKind` handoff 变体、macro-edge `handoff_path` 检查、`has_handoff_path`、kind 与 lint class 映射、更新 `empty_view()` 等 test helper）
- 修改：`crates/ralph-core/src/preset/engine/hint.rs`（移除 `LintFailureClass::HandoffArtifact`、对应 `from_reason` 字符串匹配路径与测试）
- 修改：`crates/ralph-core/src/preset/engine/linter.rs`（移除 hat-handoff precheck、auto-prepare、`RALPH_HAT_HANDOFF_SEQ` 环境变量读取、`LintPaths` 中对 `HAT_HANDOFF_DIR` 的引用）
- 修改：`crates/ralph-core/src/preset/engine/mod.rs`（如有 re-export 则移除）
- 修改：`crates/ralph-cli/src/preflight.rs`（从 `PRESET_OPT_IN_WHEN_OPERATOR_OMITS` / `PRESET_OPT_IN_KEYS` 移除 `"hat_handoff"`，删除以下两个测试函数：`merge_hats_overlay_preserves_preset_hat_handoff_when_operator_omits_it` 与 `merge_hats_overlay_lets_operator_override_preset_hat_handoff`）
- 修改：`crates/ralph-cli/src/config_resolution.rs`（从 preset opt-in 列表移除 `"hat_handoff"`）
- 修改：`crates/ralph-core/src/state_projector/mod.rs`（删除 line 580 `CommitDelta::HandoffAccepted` 与 line 595 `CommitDelta::HandoffTrackerUpdated` match 分支；删除 delta 变体后必须同步删除这两处，否则编译失败）

**方法：**
- 删除模块目录前确认 `src/lib.rs` 中无其他内部路径引用。
- 用 `rg` 确认 `macro_edges_resolved` / `is_macro_edge` / `HandoffArtifact` / `LintFailureClass::HandoffArtifact` 的所有调用方；若仅 hat_handoff 使用则一并删除。
- `preflight.rs` 中的 opt-in 列表删除 `"hat_handoff"` 后，对应测试会编译失败，同步删除或改写。
- **`presets.rs` 修正说明（对抗性审查补全）**：`rg hat_handoff crates/ralph-cli/src/presets.rs` 当前 **0 命中**。所有 `test_ce_executor_*` 测试函数体均不引用 hat_handoff 字符串，删除 hat_handoff 后这 5 个保留测试不需要改动。本计划初稿"删 ce_executor_serial 硬编码断言中含 hat_handoff 的项"为描述失实，实施时**跳过 `presets.rs`**；该文件的工作由 `2026-06-24-001 U4`（22+ → 5 个合并）独立完成。

**测试场景：**
- Happy path：`cargo build` 时找不到 `hat_handoff` 模块的错误消失。
- Edge case：`EventLoopConfig` 反序列化旧 YAML 中残留的 `hat_handoff:` 块时，未知字段按 serde 默认行为忽略；仓库中所有测试用 YAML 需同步清理。
- Error path：lint 阶段不再因 hat-handoff 产生 `HandoffArtifact` / `HandoffFilenameMismatch` rejection。

**验证：**
- `cargo build -p ralph-core` 通过。
- `cargo nextest run -p ralph-core -- preset` 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- preflight` 通过。

---

- [ ] U6. **清理预设、Schema 与构建期合并映射**

**目标：** 从预设 SSOT、内联 preset 和构建脚本中移除 `hat_handoff` 配置块与合并映射。

**需求：** R1、R2、R5

**依赖：** U5（配置类型已删除）

**文件：**
- 修改：`presets/schemas/ce-executor-serial.yml`（删除 `hat_handoff:` 顶层块 line 462-472 全段：`hat_handoff: { enabled: false, artifact: {...}, linter: {...}, exempt_topics: [...] }`）
- 修改：`presets/en/ce-executor-serial.yml`（删除 line 106-112 注释段、line 346-348 instructions 段落、coordinator instructions 中的 `handoff_path` 指引；**行号以实施时实际扫描为准，初稿 91-97 行号已漂移**）
- 修改：`crates/ralph-cli/build.rs`（删除 `hat_handoff` 合并映射条目）
- 修改：`crates/ralph-cli/src/preset_merge_table.rs`（删除 `("hat_handoff", ...)` 条目）
- 复核：`crates/ralph-cli/src/presets.rs` 中的 `PRESETS` 数组使用 `include_str!(concat!(env!("OUT_DIR"), "/presets/ce-executor-serial.yml"))`，因此**无需手动修改 `content` 字符串**；只需保证 YAML 与 schema 一致并在 build 后重新生成。

**方法：**
- 编辑 YAML 文件后运行 `cargo build -p ralph-cli`，让 `build.rs` 报错提示任何不一致，再修正。
- 用 `rg -i 'hat_handoff|handoff_path' presets/en/ce-executor-serial.yml presets/schemas/ce-executor-serial.yml` 确保无残留。
- **实施顺序（与 2026-06-24-001 协同）**：待 `2026-06-24-001` U1/U2 落地后，再扫描 `presets/schemas/ce-executor-serial.yml` 顶层 `hat_handoff:` 块与 `presets/en/ce-executor-serial.yml` 中残余 hat_handoff 段落；按实际内容删，不按行号。

**测试场景：**
- Happy path：`cargo build -p ralph-cli` 不再因 `hat_handoff` 合并映射报错。
- Happy path：`ralph run -H builtin:ce-executor-serial` 能正常初始化 event loop。
- Edge case：用户项目 `ralph.yml` 中残留的 `event_loop.hat_handoff.enabled: false` 被忽略，不影响启动。

**验证：**
- `cargo build -p ralph-cli` 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- presets` 通过。

---

- [ ] U7. **删除 hat_handoff 专用测试与 BDD 场景**

**目标：** 删除所有仅验证 hat-handoff 行为的测试与场景文件，避免测试失败后需要维护废弃代码。

**需求：** R4

**依赖：** U1–U6（被测代码已删除）

**文件：**
- 删除：`crates/ralph-core/tests/scenarios/hat_handoff/` 整个目录（5 文件：`disabled_passthrough.yml`、`dual_publish_work_ready_only.yml`、`macro_handoff_inject.yml`、`micro_edge_exempt.yml`、`next_rejected.yml`、`work_done_rejected_blocks_projection.yml`）
- 删除：`crates/ralph-core/tests/scenarios/handoff_auto_generate.yml`（hat_handoff 专用）
- 删除：`crates/ralph-core/tests/scenarios/cli_runtime_parity.yml`（含 6 处 hat_handoff 引用 line 9, 43, 50, 70, 71, 96；删除 hat_handoff 段后该 scenario 不再可解析）
- 删除：`crates/ralph-core/tests/scenarios/correction_deterministic.yml` line 44 `hat_handoff:` 配置块（删该块，或删整文件）
- 删除：`crates/ralph-core/tests/scenarios/correction_three_escalation.yml` line 43 `hat_handoff:` 配置块
- 保留：`crates/ralph-core/tests/scenarios/plan_gate_dual_publish_handoff.yml`（测试双发 carve-out，与 hat_handoff 无关）
- **冲突 — 修正为"以 2026-06-24-001 U5 为准"**：本计划初稿列保留 `progress_task_mismatch.yml` / `step_advance_u1_to_u2.yml`，但这 2 个 scenario 内嵌 `plan-gate` hat 拓扑（line 10, 24, 5, 9 等显式 `plan-gate` subscribes/publishes），与 2026-06-24-001 U5 已删除的 plan-gate 冲突。**正确处理**：待 2026-06-24-001 落地后复核 `crates/ralph-core/tests/scenarios/step_handoff/` 全部 5 文件，按 scenario 主体是否引用 plan-gate / debug-resolver 分类删除。
- 修改：`crates/ralph-core/tests/scenarios.rs`（移除已删除场景的 `#[test]` 函数：`test_hat_handoff_*` 6 个 + `test_handoff_auto_generate_*`）
- 修改/保留分类处理：`crates/ralph-core/tests/scenarios/serial_lint/*.yaml` 与 `correction_*.yml`
  - **初稿误判修正**：`serial_lint_7_handoff_seeds_coverage.yaml` line 10, 12, 39, 68 含 5 处 hat_handoff 引用 + `hat_handoff:` 配置块——**必须改写或删除**，不能保留。
  - `serial_lint_11_isolated_unaffected.yaml` line 18, 22, 26, 52, 57 含 5 处 hat_handoff_gate 引用——**改用 origin guard 触发或删除整个文件**。
  - `serial_lint_6_handoff_auto_prepare.yaml` line 38 `hat_handoff: { enabled: true }`——**改用 event_policy 触发或删除**。
  - `serial_lint_2_rejection_digest.yaml` line 12-14, 18, 48, 53, 73-75 显式验证 `hat_handoff_gate.rejection_digest_contains`——**改用 origin guard 触发或删除整个 scenario**。
  - 仅把 `hat_handoff.enabled: true` 作为配置噪音的场景：删除该配置块。
  - 依赖 hat_handoff gate 触发 rejection 的场景：改用 event_policy 或 origin guard 触发，或删除该场景。
  - `assert_state_harness_smoke.yaml` line 34, 36 含 hat_handoff 引用——改写为非 hat_handoff 触发。
- 保留：`crates/ralph-core/src/event_loop/tests/handoff_dispatch.rs`（通用 `HandoffTracker` WRC-U4 测试，**反向验证 0 个 hat_handoff 命中**）
- 保留：`crates/ralph-core/src/event_loop/tests/recovery_envelope_u7_u8.rs`（通用 HandoffTracker recovery envelope 测试，**反向验证 0 个 hat_handoff 命中**）
- 保留：`crates/ralph-core/src/event_loop/tests/enrich_kind_wiring.rs`（通用 `kind` 接线测试；实施前用 grep 复核无 handoff 引用）
- 修改：`crates/ralph-core/src/event_loop/tests/serial_lint.rs`（移除 hat-handoff 专用用例与 line 36 fixture + line 416 注释）
- 删除：`crates/ralph-core/src/event_loop/tests/coordinator_dispatch_coverage.rs`（初稿误描述"修改"——文件不存在；如确需删除该覆盖测试则需进一步调查）
- **路径修正**：本计划初稿"删除 `crates/ralph-cli/tests/policy_check_handoff.rs`"——该文件**实际不存在**。CLI 端 hat_handoff 测试全部内联在 `crates/ralph-cli/src/policy_check.rs::hat_handoff_tests` 模块（line 1175 起，9 个 `#[test]`）。正确处理：U1 中删除整个 `hat_handoff_tests` mod 子模块。
- 跳过：`crates/ralph-cli/tests/integration_emit_policy.rs`（初稿误描述"修改"——0 命中）
- 修改：`crates/ralph-core/benches/protocol_view_bench.rs`（line 48 `hat_handoff:` YAML fixture；若 benchmark 仅覆盖 hat_handoff 则删除对应 case）

**方法：**
- 删除前用 `rg` 确认每个场景/测试是否也覆盖 `step_handoff` 或通用逻辑；若混合覆盖，仅删除 handoff 分支。
- **保留**（通用 WAC-U5 / WRC-U4 设施，与 hat_handoff 无关）：
  - `crates/ralph-core/tests/scenarios/step_handoff/` 中**不含** `plan-gate` / `debug-resolver` 拓扑的 scenario（实施时 `rg -l 'plan-gate\|debug-resolver' tests/scenarios/step_handoff/` 排除这些）
  - `crates/ralph-core/src/event_loop/tests/handoff_dispatch.rs`（HandoffTracker 通用）
  - `crates/ralph-core/src/event_loop/tests/recovery_envelope_u7_u8.rs`（HandoffTracker 通用）
  - `crates/ralph-core/src/event_loop/tests/enrich_kind_wiring.rs`（kind 接线）
  - `crates/ralph-core/tests/scenarios/plan_gate_dual_publish_handoff.yml`（双发 carve-out，与 hat_handoff 无关；反向验证 0 命中）
- **删除/改写**（按 P0/P1 清单）：
  - `serial_lint/serial_lint_7_handoff_seeds_coverage.yaml`（P0-1：line 10/12/39/68 含 5 处 hat_handoff，必须删/改写）
  - `serial_lint/serial_lint_11_isolated_unaffected.yaml`（P0-1：line 18/22/26/52/57 含 hat_handoff_gate）
  - `serial_lint/serial_lint_6_handoff_auto_prepare.yaml`（P0-1：line 38 `hat_handoff: { enabled: true }`）
  - `serial_lint/serial_lint_2_rejection_digest.yaml`（P0-1：line 12-14 等显式验证 `hat_handoff_gate.rejection_digest_contains`）
  - `tests/scenarios/cli_runtime_parity.yml`（P0-1：line 9/43/50/70/71/96 共 6 处）
  - `tests/scenarios/correction_deterministic.yml` + `tests/scenarios/correction_three_escalation.yml`（P0-1：`hat_handoff:` 配置块）
- **以 2026-06-24-001 U5 为准删除**（P0-2 冲突修正）：
  - `tests/scenarios/step_handoff/progress_task_mismatch.yml`
  - `tests/scenarios/step_handoff/step_advance_u1_to_u2.yml`
  - `tests/scenarios/step_handoff/state_projection_work_done_updates_progress.yml`
  - `tests/scenarios/step_handoff/fix_exhausted_reaches_plan_gate.yml`
  - `tests/scenarios/step_handoff/debug_exhausted_reaches_plan_gate.yml`

**测试场景：**
- Happy path：删除后 `./scripts/run-tests.sh` 不再执行已删除的 handoff 测试。
- Edge case：保留的 step_handoff scenario 仅含纯 step_handoff 拓扑（不含 plan-gate / debug-resolver），通过 `rg -l 'plan-gate\|debug-resolver' tests/scenarios/step_handoff/` 反向核验为 0 命中。
- Integration：`HandoffTracker` 通用超时与 recovery envelope 测试仍通过（保留文件未被本次删除影响）。

**验证：**
- `./scripts/run-tests.sh` 通过。
- 对抗性审查 P0/P1 清单全部消化（详见顶部「P0/P1 问题清单」段与「验证门」）。

---

- [ ] U8. **更新文档、运行手册与运营脚本**

**目标：** 删除或标注废弃所有仍在维护的 `hat_handoff` 文档，避免用户读到已删除功能的说明。

**需求：** R5

**依赖：** U1、U6

**文件：**
- 删除或归档：`docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md`
- 删除或归档：`docs/solutions/integration-issues/hat_handoff_filename_mismatch_recurrence.md`
- **初稿漏列**：删除或归档 `docs/solutions/developer-experience/ce-executor-serial-30day-6th-recurrence-fix.md`（含 ~10 处 hat_handoff 引用 line 7, 9, 19, 42, 52, 56, 58, 68, 76, 118；归档到 `docs/achieved/` 或重命名为 `*-deprecated.md`）
- 修改：`docs/guide/runtime-diagnosis.md`（**初稿误描述**——实际 0 命中；改为"删除前再 grep 一次，确认无 hat_handoff 行后再跳过"）
- 修改：`docs/handbook/serial-preset-development.md`（删除 `hat_handoff.macro_topics` 配置说明段 line 11, 38, 81, 88, 101, 107, 148）
- 复核：`HANDOFF.md`（当前无 hat_handoff 引用；如有则删除）
- 修改：`crates/ralph-core/data/ralph-tools.md`（**初稿误描述**——实际 0 命中；改为"删除前再 grep 一次，确认无 handoff 子命令引用后再跳过"）
- 修改：`crates/ralph-core/data/ralph-tools-emit.md` line 40（删除 `hat_handoff.macro_topics` 段落）
- 修改：`crates/ralph-core/data/doppelganger-functions.md` line 330（删除对 `hat_handoff_filename_mismatch_recurrence.md` 的引用）
- 修改：`crates/ralph-core/data/ralph-tools-handoff.md`（U1 已删，整文件删除）
- 修改：`crates/ralph-core/src/validation/rules_publisher.rs`（删除 stale SSOT allow-list 注释 line 40）
- 修改：`crates/ralph-core/src/diagnosis/reporter.rs`（在 U2 中已处理；此处复核文档/提示字符串 line 3050/3051 `validation_stage_to_source("hat_handoff")`）
- 修改：`scripts/ralph-zsh-plugin.zsh`（在 U1 中已复核，0 命中）
- 修改：`AGENTS.md` / `CLAUDE.md`（同步更新 builtin preset 列表、命令列表；当前未发现直接引用，但需复核）
- **源码注释清理**（plan 初稿漏列）：删除 `crates/ralph-core/src/step_handoff/progress_task_gate.rs:262` 注释中 `crate::hat_handoff::gate::GateDecision` 悬空引用
- **源码注释清理**（plan 初稿漏列）：删除 `crates/ralph-core/src/workflow_contract/handoff_tracker.rs:164` 注释中 `Used by the hat_handoff gate to roll …` 字样
- **源码注释清理**（plan 初稿漏列）：删除 `crates/ralph-core/src/correction/mod.rs:69` 注释中 `hat_handoff` 字符串
- **运行时状态字段补全（plan 初稿漏列）**：`crates/ralph-core/src/runtime_state.rs` 除 `hat_handoff_seq` / `hat_handoff_dir` 外，line 55 还含 `hat_handoff_next_seq: Option<u32>` 字段——U3 删除时必须同步删除此字段，否则编译失败

**方法：**
- 历史报告 `docs/report/` 与已完成计划 `docs/achieved/` 不动。
- 若选择归档而非删除，将文件移入 `docs/achieved/solutions/` 或重命名为 `*-deprecated.md`。

**测试场景：**
- Happy path：`rg -i 'hat_handoff|## HAT HANDOFF|ralph tools handoff|ralph audit hat-handoff' docs/solutions/ docs/guide/ docs/handbook/ crates/ralph-core/data/` 无命中（排除历史报告）。
- Error path：文档中不再引导用户使用已删除命令。

**验证：**
- `cargo test --doc` 通过（文档示例中无已删除命令）。
- 手动检查 `ralph --help`、`ralph tools --help`、`ralph audit --help` 无 handoff 子命令。

---

- [ ] U9. **全量回归验证与残留扫描**

**目标：** 确保删除后无编译错误、无测试失败、无残留引用、无行为回归。

**需求：** R6

**依赖：** U1–U8

**文件：**
- 全仓库（无新增文件）

**方法：**
- 运行 `cargo fmt` 与 `cargo clippy`。
- 运行 `./scripts/run-tests.sh`。
- 使用精确模式扫描残留（执行顺序：先 `rm` U1-U8 已删文件，再做以下扫描）：
  - `rg 'hat_handoff|HatHandoff|HAT_HANDOFF' crates/ralph-core/src/ crates/ralph-cli/src/ presets/`
  - `rg 'HatHandoffSeq|hat_handoff_seq|hat_handoff_next_seq|pending_handoff_artifacts' crates/ralph-core/src/`
  - `rg 'HandoffAccepted|HandoffCommitOutcome|HandoffAcceptedInputs|HandoffTrackerUpdated' crates/ralph-core/src/`
  - `rg 'handoff_path' crates/ralph-core/src/ crates/ralph-cli/src/ presets/ -g '!**/handoff.rs' -g '!**/landing.rs'`（过滤 session handoff 路径）
  - `rg -i '## HAT HANDOFF|HAT HANDOFF EMIT|hat-handoff' crates/ralph-core/src/ crates/ralph-cli/src/`
  - `rg -i 'ralph tools handoff|ralph audit hat-handoff' scripts/ docs/guide/ docs/handbook/ docs/solutions/ docs/solutions/developer-experience/ crates/ralph-core/data/`
  - `rg 'macro_edges_resolved|is_macro_edge|macro_edge_consumers|resolve_macro_edges' crates/ralph-core/src/`
  - `rg 'RejectionKind::Handoff|ValidationStage::HatHandoff|LintFailureClass::HandoffArtifact' crates/ralph-core/src/`
- 扫描结果中排除 `step_handoff`、`HandoffTracker`、`HandoffIndex`、`.ralph/agent/handoff.md`、session `handoff_path` 等合法命中。
- **残留扫描结果交叉验证（plan 初稿漏列）**：扫描结果中必须额外排除 `docs/achieved/`、`docs/report/`、`docs/brainstorms/`、`docs/superpowers/`、`ralph-loop-diagnosis-report.md`、`deviation-report.md`（仓库根）——这些是历史/运行态/未追踪文件，不应被修改。

**测试场景：**
- Integration：`ce-executor-serial` 的 BDD scenarios（不含 hat_handoff）全部通过。
- Happy path：`ralph run -H builtin:ce-executor-serial` 能正常初始化 event loop。
- Regression：与删除前基线相比，`ce-executor-serial` 场景成功率不下降。
- Edge case：旧 ledger 中残留的 `handoff_accepted` / `handoff_tracker_updated` / `hat_handoff_seq` 行被识别为 corrupt line，不 panic。

**验证：**
- `./scripts/run-tests.sh` 全绿。
- `cargo clippy` 无新警告。
- 残留扫描无 hat_handoff 专用命中。

---

## 系统级影响

- **交互图**：`event_loop` 与 `validation`、`state`、`preset/engine`、`CLI` 的交互均减少一个分支；`step_handoff` 分支保持原样。
- **错误传播**：`RejectionKind` 减少 4 个 handoff 变体，`task.resume` 的 `kind` 字段不再携带这些值；`CoordinatorDispatcher` 的 typed dispatch 同步删除对应分支。
- **状态生命周期**：`LoopState.hat_handoff_seq` 与 `pending_handoff_artifacts` 删除后，event loop 不再维护 hat-handoff 专用中间状态；通用 `HandoffTracker` 的超时恢复路径保留。
- **API 表面**：CLI 删除 2 个子命令；配置文件中的 `hat_handoff` 块变为未知字段（serde 默认忽略）。
- **集成覆盖**：`ce-executor-serial` 的端到端场景是主要回归风险点；需确保 `queue.advance`/`plan.complete` 仍被 `step_handoff` 正确 gate。
- **数据兼容**：旧 `ledger.jsonl` 中可能残留的 `handoff_accepted` / `handoff_tracker_updated` / `hat_handoff_seq` 行会变为 corrupt line；按可接受数据废弃处理。
- **不变性保证**：
  - `step_handoff` 的 `progress_task_gate` 保持启用。
  - `workflow_contract.handoff_topic_seeds`、`HandoffIndex`、`HandoffTracker` 保持运行。
  - session 结束 `.ralph/agent/handoff.md` 保持生成。

---

## 风险与依赖

| 风险 | 可能性 | 影响 | 缓解措施 |
|---|---|---|---|
| 误删 `HandoffIndex/HandoffTracker` 导致 workflow_contract 超时/优先级失效 | 中 | 高 | U3/U5 实施前用 `rg` 验证所有调用方；只删除 `hat_handoff` 专用调用，保留通用设施。 |
| 误删 `step_handoff` 相关测试或配置 | 中 | 高 | U7 删除测试时排除 `step_handoff/` 目录与 `serial_lint_7_handoff_seeds_coverage.yaml`；U6 不修改 `workflow_contract.step_handoff`。 |
| 遗漏 `commands/mod.rs`、`loop_context.rs`、`diagnosis/*`、`preflight.rs`、`config_resolution.rs`、`summary_writer.rs` 等文件导致编译失败 | 高 | 高 | 本计划已将这些文件明确列入各单元；U9 残留扫描再次兜底。 |
| 预设合并映射删除后 `build.rs` panic | 中 | 中 | U6 同步更新 `build.rs`、`preset_merge_table.rs`，并通过 `cargo build -p ralph-cli` 验证。 |
| `presets/en/ce-executor-serial.yml` 中 hat_handoff 相关 instructions/注释残留 | 中 | 低 | U6 用 `rg -i 'hat_handoff\|handoff_path' presets/en/ce-executor-serial.yml` 兜底扫描。 |
| 旧 `ledger.jsonl` 含已删除的 `CommitDelta` 变体，反序列化报错 | 低 | 中 | 按可接受数据废弃处理；必要时引导用户用 `ralph loops clean --ledger` 截断。 |
| 文档中仍有指向已删除命令的说明 | 中 | 低 | U8 用 `rg` 扫描 `docs/solutions/`、`docs/guide/`、`docs/handbook/`、`crates/ralph-core/data/`。 |
| 残留 `handoff_path` 字段在事件 schema 或 prompt 中引发用户困惑 | 低 | 低 | U9 精确扫描 `handoff_path`，排除 session handoff 路径。 |
| **U7 — `serial_lint_7_handoff_seeds_coverage.yaml` line 10, 12, 39, 68 含 5 处 hat_handoff 引用 + `hat_handoff:` 配置块；plan 初稿 U7 误判"保留"，删除 hat_handoff 后 scenario 必失败** | **高** | **高** | **对抗性审查补全**：U7 显式处理 `serial_lint_7_*` / `serial_lint_11_*` / `cli_runtime_parity.yml` / `correction_deterministic.yml` / `correction_three_escalation.yml` 6 文件——改写或删除，不允许"按字面 plan 实施"。 |
| **U7 — `progress_task_mismatch.yml` / `step_advance_u1_to_u2.yml` 内嵌 plan-gate hat，与 2026-06-24-001 U5 删 plan-gate 冲突** | **高** | **高** | **对抗性审查补全**：U7 改为"待 2026-06-24-001 落地后，按 scenario 主体是否引用 plan-gate / debug-resolver 分类删除"；删除决策以 2026-06-24-001 U5 为准。 |
| **U5 — `state_projector/mod.rs` line 580, 595 含 `CommitDelta::HandoffAccepted` / `HandoffTrackerUpdated` match 分支，删除变体后未同步删除分支会编译失败** | **高** | **高** | **对抗性审查补全**：U5 显式列入 `state_projector/mod.rs`，删除两处 match 分支。 |
| **U5 — `preflight.rs` line 1521 / 1569 两个 `hat_handoff` merge 测试函数（`merge_hats_overlay_preserves_preset_hat_handoff_when_operator_omits_it` / `merge_hats_overlay_lets_operator_override_preset_hat_handoff`），plan U5 仅描述"删除相关 merge 测试"未给测试函数名** | 中 | 中 | **对抗性审查补全**：U5 显式列两个测试函数名，实施时按名删除。 |
| **U3 — `runtime_state.rs` line 55 `hat_handoff_next_seq: Option<u32>` 字段，plan U3 仅列 `hat_handoff_seq` / `pending_handoff_artifacts`，漏此字段** | 中 | 高 | **对抗性审查补全**：U8 显式补 `hat_handoff_next_seq` 字段删除要求。 |
| **U5 — `presets.rs` 中实际 0 个 hat_handoff 命中，plan U5 声称"删 ce_executor_serial 硬编码断言中含 hat_handoff 的项"为描述失实** | 中 | 中 | **对抗性审查补全**：U5 改为"跳过 `presets.rs`；该文件由 2026-06-24-001 U4 22→5 合并独立完成"。 |
| **U3 — `event_loop/mod.rs` 38 处 hat_handoff 引用（line 3487-9703 散布），plan U3 仅粗列"逐个删除"** | 中 | 高 | 实施时按 U3 文件清单的 `方法` 段执行"使用 `rg` 定位 `hat_handoff` 在 `event_loop/mod.rs` 中的所有调用点"；line 350, 763, 783, 907, 928, 2230, 2594, 5762, 5771, 6459, 6461, 8380 是通用 `HandoffTracker` 调用，**不删**；line 3624, 3668, 4821-4828, 4951-4956, 5127, 5754, 7480-7727, 7909, 8401, 9703, 7596-7721 等专用调用**全删**。 |
| **U3 — `step_handoff/progress_task_gate.rs:262` 与 `workflow_contract/handoff_tracker.rs:164` 注释引用 `hat_handoff_gate` 字样，删除后注释悬空** | 低 | 低 | U8 显式清理这两处源码注释。 |
| **U6 — `presets/en/ce-executor-serial.yml` hat_handoff 引用实际在 line 106-112 注释、line 346-348 instructions；plan U6 声称"包括但不限于第 91-97 行"行号已漂移** | 中 | 低 | U6 实施时按 `rg -n 'hat_handoff\|handoff_path' presets/en/ce-executor-serial.yml` 实际命中位置删除。 |
| **U6 — `presets/schemas/ce-executor-serial.yml` line 462-472 顶层 `hat_handoff:` 块，本计划与 2026-06-24-001 都未完整处理** | 中 | 中 | U6 待 2026-06-24-001 U2 落地后删除该顶层块（按实际内容删，不按行号）。 |
| **U4 — `replay_from_disk` 失败 fallback 到 `cold_start()` 会让 `HandoffTracker::on_handoff_accepted` 记录的事件全部丢失；`hat_handoff_seq` counter 重启后从 0 开始漂移；plan 风险表仅说"按可接受数据废弃处理"，未明确 counter 漂移** | 低 | 中 | 风险表中明示：counter 漂移是接受的数据废弃；新事件从 0 开始计数。 |
| **U9 — `rg` 排除规则 `!handoff.rs / !landing.rs` 不充分；需额外排除 `ralph-tools-handoff.md`、`.claude/skills/ralph-tools/SKILL.md`** | 中 | 中 | U9 显式扩展排除规则（已写入 U9 方法段）。 |

---

## 文档 / 运营说明

- 删除 `ralph tools handoff` 与 `ralph audit hat-handoff` 后，更新 `scripts/ralph-zsh-plugin.zsh` 并重新安装到 `~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`（按 `AGENTS.md` 要求）。
- `docs/solutions/` 中相关运行手册归档或删除；`docs/report/` 历史复盘保留。
- 若后续用户项目 `ralph.yml` 仍包含 `hat_handoff:` 块，serde 默认会忽略未知字段，不会阻塞启动。
- **与 `2026-06-24-001` 协同**：本计划 U8 同步 `AGENTS.md`/`CLAUDE.md`/`.cursor/rules/multi-hat-isolation.mdc` 时，须把 2026-06-24-001 落地的 11→10-hat 描述、validator hat 与 TDD executor instructions 一并写入，避免「文档落后于代码」违规。建议两 PR 合入前各 owner 互发一份 cross-review 清单。

---

## 来源与参考

- **原始需求文档：** `docs/brainstorms/2026-06-18-isolated-hat-handoff-requirements.md`
- **复发复盘：** `docs/solutions/integration-issues/hat_handoff_filename_mismatch_recurrence.md`
- **运行手册：** `docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md`
- **当前 preset 配置：** `presets/schemas/ce-executor-serial.yml`、`presets/en/ce-executor-serial.yml`
- **相关代码模块：** `crates/ralph-core/src/hat_handoff/`、`crates/ralph-core/src/event_loop/`、`crates/ralph-core/src/validation/`、`crates/ralph-core/src/state/`、`crates/ralph-cli/src/`
- **关联 plan（须先完成）：** `docs/plans/2026-06-24-001-refactor-ce-executor-serial-tdd-validator-plan.md`（ce-executor-serial preset 重写 11→10-hat + TDD + validator + 总体 review；本计划与它在多个下游文件上重叠，详见 KTD7 与「范围边界」下游重叠表）

---

## 完成记录

- **完成日期：** 2026-06-25
- **最终验证：** `./scripts/run-tests.sh` 全绿、`cargo clippy` 无新增错误
- **主要收尾工作：**
  - 删除 `crates/ralph-cli/src/policy_check.rs` 中未删净的 `hat_handoff_tests` 模块
  - 清理 `crates/ralph-cli/src/commands/emit.rs` 中 dead lint / hat-handoff 相关代码
  - 删除 `crates/ralph-core/tests/scenarios/` 下 hat_handoff 专用 BDD 场景与 plan-gate 冲突场景
  - 将 `RejectionKind::Handoff*` 测试引用迁移到现有 variant（MissingField / TopicOwnership / ContractViolation）
  - 修复 `RejectionKind::from_reason_code` 遗漏新 variant 的映射
  - 删除 `ralph-tools-handoff` 相关 skill 测试断言
  - 执行 U9 残留扫描，确认 `step_handoff` / session `handoff_path` / `HandoffTracker` 等合法命中外无 hat_handoff 专用残留
