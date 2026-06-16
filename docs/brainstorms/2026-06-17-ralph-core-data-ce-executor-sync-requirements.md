---
date: 2026-06-17
topic: ralph-core-data-ce-executor-sync
title: "ralph-core/data 文档同步：通用 emit 修复 + handoff skill + 诊断链"
---

## Summary

同步 `crates/ralph-core/data` 内置 agent skill 文档与当前 Ralph 运行时行为：通用层修正所有 preset 共用的 emit / policy / `task.resume` 参考（含行号反向验证），新增按需 `ralph-tools-handoff` skill 覆盖 ce-executor step handoff 机制，并在 `docs/guide/runtime-diagnosis.md` 补深度排查段、由 data skill 短链引用。

---

## Problem Frame

`crates/ralph-core/data/` 下的 7 个内置 skill 是 loop 内 agent 的**权威 CLI 参考**，通过 `skill_registry.rs` 编译进二进制并在 prompt 前缀注入（或按需 `ralph tools skill load`）。近期 Ralph 新增了 step handoff、null-payload 硬门（9 topic）、isolated 终态 authority、wave policy 预检等机制，但 data 目录仍停留在较早版本。

已核实的具体漂移与缺口：

- `ralph-tools.md` 引用 `event_loop/mod.rs:910-930`，实际注入逻辑在 `4855-4896`。
- data 目录内**零**提及 `queue.advance` / `plan.complete` / `plan.blocked` / `work.ready` / `progress_task_gate` / `trigger_multi_consumer_topics`。
- `docs/code-review-2026-06-17-002.md` finding #19：agent 收到 `task.resume` 后缺少 payload 修复指南，导致 emit/handoff 失败在 loop 内反复发生。
- `docs/guide/runtime-diagnosis.md` 已有 `handoff_dispatch_timeout` / `progress_task_mismatch` 段落（约 L579+），但 agent 在 loop 内不会主动读取 guide，且与 emit skill 之间缺少可执行指针。

用户痛点：运行时 emit 失败、`task.resume` 不知如何修；同时希望预防性同步，但**通用参考不应写成 ce-executor 特制文档**。

---

## Key Decisions

- **三层分工：通用（A）/ handoff 专用（B）/ 诊断深度（C）** — `ralph-tools-emit.md` 服务所有 preset 的共用 emit 规则与稳定 reason code；ce-executor 复杂 step 链、progress gate、multi-consumer 路由放进独立 `ralph-tools-handoff.md`；根因排查与 `ralph diagnose` 工作流放进 `runtime-diagnosis.md`，data 只保留一行链入。
- **A 的 ce-executor reason 采用「摘要 + 详见 handoff」** — 通用 `task.resume` 表写跨 preset 稳定的 reason（如 `payload_contract_violation`、isolated 越权、`missing_required_field`）；ce-executor 特有 reason（`progress_task_mismatch`、`handoff_dispatch_timeout`、`review_passed_while_wave_open` 等）在通用表中以一行摘要指向 handoff skill，避免 A 再次变成 preset 特制。
- **沿用 U5 按需 skill 拆分模式** — 与现有 `ralph-tools-emit` / `ralph-tools-wave` / `ralph-tools-cmdref` 一致；handoff skill 默认不注入 prompt，由 `ralph-tools.md` 速查表引导 `ralph tools skill load ralph-tools-handoff`。
- **反向验证为交付硬门** — 遵循 `CLAUDE.md` / `AGENTS.md`：修正所有 `xxx.rs:NN-MM` 引用、对齐 clap `--help`、对文档列出的命令做冒烟。

---

## Actors

- **A1 — Loop 内 agent（主要受益者）** — 在 emit 失败或收到 `task.resume` 时需要可执行修复步骤，而非翻源码。
- **A2 — 维护者 / 贡献者** — 改 CLI 或 event policy 时需知道更新哪份 data 文件；本轮不建 CI，但文档应自描述维护规则。
- **A3 — 人类运维** — 通过 `ralph diagnose` 与 `runtime-diagnosis.md` 做 post-mortem；C 层主要服务 A3，A1 通过短链间接受益。

---

## Requirements

**通用层（A）— 所有 preset 共用**

- R1. 审计并修正 `crates/ralph-core/data/ralph-tools.md` 中所有源码行号引用，确保 `sed -n` 复核后指向正确代码范围（至少包含已漂移的 `event_loop/mod.rs` 注入段、`hats.rs`、`skill_cli.rs`、`emit_path.rs`、`wave.rs`）。
- R2. 扩展 `crates/ralph-core/data/ralph-tools-emit.md`：**跨 preset** 的 null-payload 硬门说明——列出 `NULL_PAYLOAD_REJECT_TOPICS` 当前 9 个 topic（`review.passed`、`review.failed`、`review.complete`、`work.done`、`queue.advance`、`review.wave.ready`、`work.ready`、`plan.complete`、`plan.blocked`），说明空 payload 会被拒收及与 `--policy-check` 的关系。
- R3. 在 `ralph-tools-emit.md` 增加**通用** `task.resume` 修复表：覆盖稳定 reason / violation 类型（如 `missing_required_field`、`payload_contract_violation`、isolated `event.isolation.boundary_violation`、policy check 失败），每行给出「检查什么 → 怎么修 → 验证命令」。ce-executor 特有 reason 仅一行摘要 + 「详见 `ralph-tools-handoff`」。
- R4. 在 `ralph-tools-emit.md` 保留并强化 isolated 模式 hat 作用域规则（`publishes` 越权 → `task.resume`），表述为**所有 isolated preset** 通用，不绑定 ce-executor 名称。
- R5. 对 `ralph-tools-emit.md` 和 `ralph-tools.md` 中列出的 CLI 命令执行 `--help` 冒烟，参数表与 clap 定义一致。

**Handoff 层（B）— ce-executor step handoff**

- R6. 新建 `crates/ralph-core/data/ralph-tools-handoff.md`，注册为 builtin skill `ralph-tools-handoff`（`skill_registry.rs`），`metadata.internal: true`，遵循现有 frontmatter 约定。
- R7. Handoff skill 内容须覆盖 ce-executor step 链 agent 可执行知识：
  - step handoff topic 归属表（谁可 emit `queue.advance` / `work.ready` / `plan.complete` / `plan.blocked`；executor **不可** emit `queue.advance` 等）
  - `NULL_PAYLOAD_REJECT_TOPICS` 在 handoff 链上的 payload 要点（哪些字段常见缺失：`plan_name`、`step`、`task_id` 等——以 preset schema / policy 为准，不写死错误 JSON）
  - `progress_task_gate`：`progress.md` 与 `tasks.jsonl` 对齐要求；`progress_task_mismatch` 时的修复步骤（关 task、回写 progress、再 emit）
  - `trigger_multi_consumer_topics` 概念：哪些 topic 允许多消费者、emit 前如何确认路由
  - 常见 `plan.blocked` reason（如 `dimension_reviewers_failed_to_converge`）与 agent 应做/不应做
  - `review_passed_while_wave_open` 语义：recoverable、禁止 empty_diff 投机、等待机制 `plan.blocked`
- R8. 在 `ralph-tools.md` 速查表增加一行：handoff 参考 → `ralph tools skill load ralph-tools-handoff`（标注 ce-executor / step-handoff preset 场景）。
- R9. Handoff skill 内每个修复步骤附带校验命令（`tail events.jsonl | jq`、`ralph tools task list`、`ralph emit --policy-check` 等），与 emit skill 风格一致。

**诊断层（C）— runtime-diagnosis 与短链**

- R10. 扩展 `docs/guide/runtime-diagnosis.md`：在现有 handoff 排查段（`handoff_dispatch_timeout`、`progress_task_mismatch`）基础上，增加「emit rejection → task.resume → 修复」决策树，串联 `recovery.jsonl` source（`payload_contract`、`workflow_guard`、`execution_contract`）与对应修复动作。
- R11. 在 `ralph-tools-emit.md` 和 `ralph-tools-handoff.md` 末尾增加短指针：`docs/guide/runtime-diagnosis.md` 对应章节（repo-relative 路径），说明「loop 内速查用本 skill；根因排查用 diagnose + guide」。
- R12. 若 `runtime-diagnosis.md` 引用源码行号，同步做反向验证（与 R1 同标准）。

**注册与一致性**

- R13. 若 `.claude/skills/ralph-tools-emit/SKILL.md` 等 symlink 指向 data 文件，新增 handoff 后评估是否需要在 `.claude/skills/` 增加对应 symlink（与现有 emit/wave 模式一致）；`ralph-tools.md` 的 symlink 保持为 base entry。
- R14. 不修改 `ralph-tools-tasks.md` / `ralph-tools-memories.md` / `ralph-tools-cmdref.md` 正文，除非 R1 审计发现与当前 CLI 明显冲突的行号或命令描述。

---

## Key Flows

- F1. Agent emit 被拒
  - **Trigger:** `ralph emit` 返回 policy check 失败或 loop 注入 `task.resume`。
  - **Actors:** A1
  - **Steps:** 读已注入的 `ralph-tools-emit` → 查 reason 表 → 按步骤修 payload → `ralph emit --policy-check` 预检 → 重试；若 reason 为 handoff 特有 → `ralph tools skill load ralph-tools-handoff` → 执行 handoff 修复表 → 验证。
  - **Outcome:** 事件被接受或 agent 明确知道等待机制收摊（如 incomplete wave `plan.blocked`）。

- F2. Agent 主动加载 handoff 参考
  - **Trigger:** ce-executor preset、`queue.advance` / `plan.complete` / progress 相关任务。
  - **Actors:** A1
  - **Steps:** 从 `ralph-tools.md` 速查表 → `ralph tools skill load ralph-tools-handoff`（需 `RALPH_CURRENT_HAT`）→ 按 topic 归属表 emit。
  - **Outcome:** 不越权 emit、payload 含必需字段。

- F3. 人类 post-mortem
  - **Trigger:** loop 异常终止或反复 `task.resume`。
  - **Actors:** A3
  - **Steps:** `ralph diagnose --session latest` → 按 `runtime-diagnosis.md` 决策树查 `recovery.jsonl` → 对照 handoff / emit skill 修复项。
  - **Outcome:** 定位是 payload、gate、还是 handoff SLA 问题。

---

## Acceptance Examples

- AE1. **Covers R1, R5**
  - **Given:** `ralph-tools.md` 引用 `event_loop/mod.rs:910-930`
  - **When:** 维护者 `sed -n '4855,4896p' crates/ralph-core/src/event_loop/mod.rs`
  - **Then:** 文档行号更新为覆盖 `inject_memories_and_tools_skill` 中 ralph-tools 注入逻辑的实际范围；`cargo nextest run -p ralph-core -- skill_registry` 或 smoke 相关子集仍通过。

- AE2. **Covers R2, R3, R4**
  - **Given:** 任意 isolated preset 下 agent 对 `work.done` emit 空 payload
  - **When:** agent 阅读 `ralph-tools-emit.md` 的 null-payload 表与 `task.resume` 修复表
  - **Then:** 文档明确该 topic 在 `NULL_PAYLOAD_REJECT_TOPICS` 中、需非空 JSON payload，并给出 `--policy-check` 预检命令；**不出现**「仅 ce-executor」限定语。

- AE3. **Covers R6, R7, R8**
  - **Given:** ce-executor-isolated preset、agent 为 executor hat
  - **When:** agent 加载 `ralph-tools-handoff` 并尝试 emit `queue.advance`
  - **Then:** skill 明确 executor 不可 emit 该 topic，并指向 plan-gate 职责；文档含 `progress_task_mismatch` 修复步骤。

- AE4. **Covers R10, R11**
  - **Given:** `recovery.jsonl` 出现 `reason_code=progress_task_mismatch`
  - **When:** agent 读 `ralph-tools-handoff` 末尾指针
  - **Then:** 可跳转到 `docs/guide/runtime-diagnosis.md` 对应段落的 jq 示例与排查清单。

- AE5. **Covers R3（摘要边界）**
  - **Given:** `task.resume` payload 含 `progress_task_mismatch`
  - **When:** agent 只读了 `ralph-tools-emit.md` 通用表
  - **Then:** 通用表有一行指向 handoff skill；完整修复步骤仅在 `ralph-tools-handoff.md`。

---

## Success Criteria

- SC1. ce-executor loop 内 agent 在 emit/handoff 失败时，**无需读源码**即可从 data skill 找到下一步修复动作（可由场景测试或 dogfood 记录验证）。
- SC2. `ralph-tools.md` / `ralph-tools-emit.md` 中所有 `*.rs:NN` 引用经 `sed` 复核零漂移。
- SC3. `ralph emit --help`、`ralph wave emit --help`、`ralph tools skill load --help` 与文档参数表一致。
- SC4. 新增 `ralph-tools-handoff` 出现在 `ralph tools skill list` 且可通过 `skill load` 加载（smoke_runner 或等效测试覆盖注册）。

---

## Scope Boundaries

**Deferred for later**

- CI / 集成测试自动检测 data 文档与 `--help` 漂移（如 `integration_emit_policy.rs` 模式扩展到 handoff topic）。
- 全量审计 `ralph-tools-tasks.md`、`ralph-tools-memories.md`、`ralph-tools-cmdref.md` 正文（本轮仅 R14 冲突时触达）。
- `ralph hats show` 输出 `trigger_multi_consumer_topics`（CLI 增强，非 data 范围）。
- `ralph emit --policy-check` 接入 U4 step_handoff gate 预检（code review #21）。

**Outside this product's identity**

- 把 ce-executor preset 的完整 hat instructions 复制进 data skill（preset YAML 仍是 hat 行为 SSOT；skill 只写 agent 可执行 CLI + payload 修复）。
- 在 data skill 中维护完整 JSON Schema 副本（以 runtime policy + `--policy-check` 为准，文档写要点与验证命令）。

---

## Dependencies / Assumptions

- Step handoff 机制以 `docs/plans/2026-06-17-002-feat-ce-executor-step-handoff-plan.md` 及已合并代码为 SSOT；`NULL_PAYLOAD_REJECT_TOPICS` 列表以 `crates/ralph-core/src/event_policy.rs` 为准。
- `ralph tools skill load` 继续要求 `RALPH_CURRENT_HAT`（agent 上下文）；文档须保留该前提。
- `docs/guide/runtime-diagnosis.md` 为诊断 guide SSOT；C 层是扩展而非另起文档。
- 假设本轮**不**改 event loop 注入策略（handoff skill 保持按需加载，不默认塞进 prompt 前缀以控制 token）。

---

## Sources / Research

- `crates/ralph-core/data/` — 当前 7 个内置 skill 源文件
- `crates/ralph-core/src/skill_registry.rs` — builtin 注册与 include_str 入口
- `crates/ralph-core/src/event_policy.rs:502-512` — `NULL_PAYLOAD_REJECT_TOPICS`
- `crates/ralph-core/src/event_loop/mod.rs:4855-4896` — ralph-tools 注入实际位置
- `docs/code-review-2026-06-17-002.md` finding #19 — 文档缺口证据
- `docs/guide/runtime-diagnosis.md` — 已有 handoff 排查段（~L579+）
- `docs/brainstorms/2026-06-17-ce-executor-step-handoff-requirements.md` — handoff 产品需求上游
- `docs/achieved/brainstorms/2026-06-01-ralph-cli-agent-reference-requirements.md` — U5 按需 skill 拆分先例

---

## Outstanding Questions

**Deferred to Planning**

- OQ1. `ralph-tools-handoff` 是否需要在 ce-executor preset 的某 hat `instructions` 中显式提示加载（preset 补丁 vs 纯 skill 速查表）——规划阶段根据 token 预算决定。
- OQ2. `.claude/skills/` 是否为 handoff 新建 symlink，或与 emit 一样仅 runtime `skill load` —— 对齐现有 `.claude/skills/ralph-tools-emit` 模式后定。
