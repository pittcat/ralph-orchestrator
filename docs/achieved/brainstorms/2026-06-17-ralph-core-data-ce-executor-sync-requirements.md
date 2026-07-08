---
date: 2026-06-17
topic: ralph-core-data-ce-executor-sync
title: "ralph-core/data 文档同步：loop 纠偏优先 + emit/handoff 参考 + 诊断链"
---

## Summary

以 **loop 内 agent 纠偏** 为首要目标，同步 `crates/ralph-core/data` 与**已落地的**运行时机制（2026-06 近两个月：`task.resume` 路由、step handoff gate、incomplete wave `plan.blocked`、`SemanticGateViolation` 等——见 `docs/report/2026-06-16-systematic-review-of-recent-fixes.md`）。在**每轮自动注入**的 `ralph-tools.md` 增加 `task.resume` 解码与修复要点（最高 ROI）；扩展按需 `ralph-tools-emit.md`；新增按需 `ralph-tools-handoff`；扩展 `runtime-diagnosis.md`。**本轮只补文档**，机制边角见姊妹计划 `docs/plans/2026-06-17-005-fix-agent-recovery-mechanism-gaps-plan.md`（分 PR）。

---

## Problem Frame

`crates/ralph-core/data/` 下的内置 skill 通过 `skill_registry.rs` 编译进二进制。loop 内 agent **每轮自动看到** `ralph-tools.md`（memories/tasks 至少一项启用时）；`ralph-tools-emit` / handoff 等**仅在被 `skill load` 时**才进入上下文。

**机制层**（与 data 无关）已在 payload 错误时注入 `task.resume`（含 `stage` / `topic` / `violation` / `required_fields` 等）并路由回源 hat，loop 多数情况下**会继续转**。但 **data 层未教 agent 如何读这些信号**，导致：

- emit/handoff 失败后 agent 空转或钻空子（empty_diff、越权 topic、progress 不对齐）；
- `docs/code-review-2026-06-17-002.md` finding #19：`task.resume` 后缺少可执行修复指南；
- `ralph-tools.md` 行号漂移（`event_loop/mod.rs:910-930` vs 实际 `4855-4896`）；
- 通用错误表将 `policy check failed` 导向 `--unsafe-no-policy-check`，有误导绕过风险；
- handoff 特有路径（`progress_task_mismatch` → `plan.blocked` 等）在 data 中几乎空白。

用户确认：**只关心 loop 行为**；`.claude/skills/` symlink 为 IDE 便利，**非 loop 必需**，本轮降为可选。

---

## Key Decisions

- **纠偏优先级：自动注入 > 按需深参考** — 跨 preset 的 `task.resume` 解码与「禁止 bypass」规则写在 **`ralph-tools.md`**（每轮可见）；详细 emit/null-payload 表在 **`ralph-tools-emit.md`**（按需）；ce-executor handoff 深表在 **`ralph-tools-handoff.md`**（按需）。handoff skill **不能替代**自动注入段。
- **机制 vs 文档边界** — 本轮只补文档，不改 `event_loop` 注入策略、不新增 gate。`ralph-tools-handoff` 不增加 runtime 能力，仅深参考。
- **三层分工：自动纠偏（A0）/ 通用 emit（A）/ handoff 深参考（B）/ 诊断（C）** — A0 在 `ralph-tools.md`；A 在 `ralph-tools-emit.md`；B 在 `ralph-tools-handoff.md`；C 在 `runtime-diagnosis.md` + data 短链。
- **A / A0 不写 ce-executor 特制长文** — ce-executor 特有 reason 在自动注入段**一行摘要** + 「`ralph tools skill load ralph-tools-handoff`」；完整步骤仅在 handoff skill。
- **禁止文档引导 bypass** — 移除或改写「policy 失败 → `--unsafe-no-policy-check`」类表述；统一强调 `ralph emit --policy-check` 预检与读 `required_fields`。
- **handoff 仅按需加载** — 不 patch preset instructions；不默认注入 prompt（token 预算）。
- **`.claude/skills` symlink 可选** — 对齐 emit/wave 模式可作维护便利，**不纳入 loop 交付硬门**。
- **反向验证为交付硬门** — `sed` 复核 `*.rs:NN` 引用 + 相关 `--help` 冒烟。

---

## Actors

- **A1 — Loop 内 agent（主要受益者）** — 在 PENDING EVENTS 看到 `task.resume` 或 emit 失败时，**无需 skill load** 即可从自动注入的 `ralph-tools.md` 知道下一步；复杂 handoff 再 load 深参考。
- **A2 — 维护者** — 更新 data 时遵循 A0/A/B 分工与反向验证规则。
- **A3 — 人类运维** — `ralph diagnose` + `runtime-diagnosis.md` 做 post-mortem。

---

## Requirements

**自动纠偏层（A0）— 每轮注入的 `ralph-tools.md`**

- R0. 在 `ralph-tools.md` 新增 **「收到 `task.resume` 时」** 小节（控制在约 15–25 行内，保证文件仍 ≤200 行）：说明 payload 常见字段（`stage`、`topic`、`violation`、`required_fields`、`allowed_topics`）；修复顺序（读 violation → 对照 `required_fields` 补 payload → `ralph emit --policy-check` 预检 → 重试）；**明确禁止**用 `--unsafe-no-policy-check` 或直写 `events.jsonl` 绕过；ce-executor handoff 类 reason 一行指向 `ralph tools skill load ralph-tools-handoff`。
- R0b. 修正 `ralph-tools.md` 通用错误表中 `policy check failed` 等误导行：改为读 stderr / `validation_errors` / `--policy-check`，不得首选 unsafe bypass。
- R1. 审计并修正 `ralph-tools.md`（及 data 内其他文件）所有 `*.rs:NN` 行号引用（至少 `event_loop/mod.rs` 注入段、`hats.rs`、`skill_cli.rs`、`emit_path.rs`、`wave.rs`）。

**通用 emit 层（A）— 按需 `ralph-tools-emit.md`**

- R2. 列出 `NULL_PAYLOAD_REJECT_TOPICS` 当前 9 topic 及与 `--policy-check` 的关系（跨 preset）。
- R3. 扩展 **通用** `task.resume` / CLI 修复表（与 R0 互补，可更细）；ce-executor 特有 reason 一行摘要 → handoff skill。
- R4. 强化 isolated `publishes` 越权规则（跨 preset 通用表述）。
- R5. `ralph-tools-emit.md` 与 `ralph-tools.md` 所列 CLI 做 `--help` 冒烟对齐。

**Handoff 深参考层（B）— 按需 `ralph-tools-handoff.md`**

- R6. 新建 `ralph-tools-handoff.md` 并注册 builtin skill（`skill_registry.rs`，`metadata.internal: true`）。
- R7. 内容覆盖 ce-executor step 链可执行知识：topic 归属表、handoff payload 要点、`progress_task_gate` / `progress_task_mismatch` 修复、`trigger_multi_consumer_topics`、`plan.blocked` / `review_passed_while_wave_open` / `handoff_dispatch_timeout` 等；每节附校验命令。
- R8. 在 `ralph-tools.md` 速查表增加 handoff 一行 + R0 中的 load 指针。
- R9. 与 R0 交叉引用，避免三处文档重复长表。

**诊断层（C）**

- R10. 扩展 `runtime-diagnosis.md`：`emit rejection → task.resume → 修复` 决策树。
- R11. `ralph-tools-emit.md` / `ralph-tools-handoff.md` 末尾短链至 guide；R0 可一行提及 diagnose。
- R12. guide 内源码行号反向验证。

**注册与一致性**

- R13. **（可选）** `.claude/skills/ralph-tools-handoff/SKILL.md` symlink 指向 data 文件；**不阻塞** loop 交付。
- R14. 不修改 `ralph-tools-tasks.md` / `memories.md` / `cmdref.md`，除非 R1 审计发现明显冲突。

---

## Key Flows

- F1. Agent 在 loop 内收到 `task.resume`（**主路径**）
  - **Trigger:** policy/origin/contract 拒收后编排器注入 `task.resume`。
  - **Actors:** A1
  - **Steps:** 读**已自动注入**的 `ralph-tools.md` §收到 task.resume → 按 R0 顺序修 payload → `--policy-check` → 重试；若 violation 为 handoff 特有 → `skill load ralph-tools-handoff`；仍不明 → guide / diagnose。
  - **Outcome:** loop 继续且 agent 行为收敛，而非重复同类错误。

- F2. Agent emit 在 CLI 层失败（**次路径**）
  - **Trigger:** `ralph emit` 非零退出。
  - **Steps:** 读 R0 要点 → 按需 `skill load ralph-tools-emit` 查详表 → 预检重试。

- F3. progress_task_gate 拒收 → `plan.blocked`（**handoff 路径**）
  - **Trigger:** `queue.advance` / `plan.complete` 与 progress/tasks 不一致。
  - **Steps:** R0 一行摘要 → load handoff skill → 对齐 progress/tasks；等待 plan-gate 消费 `plan.blocked`（非 executor 自救 emit）。

- F4. 人类 post-mortem（A3）— 同前：`ralph diagnose` + guide。

---

## Acceptance Examples

- AE0. **Covers R0, SC1**
  - **Given:** loop 注入 `task.resume`，payload 含 `required_fields: ["plan_name"]`
  - **When:** agent **未** load 任何按需 skill，仅读自动注入的 `ralph-tools.md`
  - **Then:** 文档说明读 `required_fields`、用 `--policy-check` 补字段后重 emit；**不**建议 unsafe bypass。

- AE1. **Covers R1, R5** — 行号 sed 复核（同前）。

- AE2. **Covers R2, R3, R4** — isolated 空 payload / emit 详表（同前，在 emit skill）。

- AE3. **Covers R6, R7, R8** — handoff skill load 后见 topic 归属与 progress 修复（同前）。

- AE4. **Covers R10, R11** — guide 短链（同前）。

- AE5. **Covers R0 + R3 边界** — 自动注入段仅摘要 handoff reason；完整表在 handoff skill。

---

## Success Criteria

- SC1. **Loop 纠偏（主）**：常见 `task.resume`（缺字段、越权 topic、policy 失败）下，agent **在不 load 按需 skill 时** 能从自动注入的 `ralph-tools.md` 执行下一步修复；handoff 复杂场景 load handoff 后可修。
- SC2. 行号 `sed` 零漂移。
- SC3. `--help` 与文档参数表一致。
- SC4. `ralph-tools-handoff` 可 list/load（registry 测试）。
- SC5. `ralph-tools.md` 仍 ≤200 行（CI guard）。

---

## Scope Boundaries

**Deferred for later**

- CI 自动 doc ↔ `--help` 漂移门禁。
- 全量 audit tasks/memories/cmdref。
- `ralph hats show` 暴露 multi-consumer。
- `ralph emit --policy-check` 接入 step_handoff gate 预检。
- preset instructions 内嵌 load handoff 提示（dogfood 后若发现率不足再议）。

**Outside this product's identity**

- 复制完整 preset instructions 或 JSON Schema 进 data。
- 修改编排机制（新 gate、默认注入 handoff 全文）。
- **强制** `.claude/skills` symlink（IDE 层，非 loop）。

---

## Dependencies / Assumptions

- 纠偏依赖既有机制：`task.resume` payload、`append_fix_hint_if_recoverable`、hat §3 schema 示例、可选 `Runtime Diagnosis Alert`（`RALPH_DIAGNOSTICS=1`）。data 文档与之对齐，不重复发明机制。
- Step handoff SSOT：`2026-06-17-002` plan + `event_policy.rs` `NULL_PAYLOAD_REJECT_TOPICS`。
- `ralph tools skill load` 仍需 `RALPH_CURRENT_HAT`。
- handoff skill 保持按需，不进入 `event_loop` 注入白名单。

---

## Sources / Research

- `crates/ralph-core/src/event_loop/mod.rs` — `task.resume` 发布、`publish_policy_rejection_resume`、prompt 注入链
- `crates/ralph-core/src/event_loop/rejection.rs` — `build_task_resume_payload`
- `crates/ralph-core/data/ralph-tools.md` — 当前自动注入内容与 Agent Output Governance 段
- `docs/code-review-2026-06-17-002.md` finding #19
- `docs/guide/runtime-diagnosis.md` §12

---

## Outstanding Questions

**Resolved**

- OQ1. **不** patch ce-executor preset instructions；靠 R0 自动注入 + R8 速查表。
- OQ2. `.claude/skills` symlink **可选**（R13），非 loop 交付硬门。

**Deferred to Implementation**

- R0 与现有「Agent Output Governance」段合并 vs 独立新节 — 实施时选更短、更不重复的结构。
