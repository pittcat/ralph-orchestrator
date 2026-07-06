---
date: 2026-07-06
topic: ce-executor-serial-protocol-ssot-convergence
status: draft
supersedes_in_spirit: []
related:
  - docs/brainstorms/2026-07-02-ce-executor-serial-runtime-phase-authority-requirements.md
  - docs/brainstorms/2026-06-27-ralph-orchestrator-mechanism-foundation-requirements.md
  - presets/schemas/ce-executor-serial.yml
  - docs/handbook/serial-preset-development.md
  - docs/report/
origin: 对话收敛 — 机制过多不收敛；SSOT 从 payload schema 扩展到统一 emit 响应；删除 progress-steward
---

# ce-executor-serial 协议 SSOT 收敛 — 需求文档

## Summary

`ce-executor-serial` 半个多月反复跑不通，根因不是缺机制，而是 **机制叠层、对外不收敛**：agent 要面对 events、recovery、progress、prompt 路由表等多套状态，且失败时常被 shipper/recovery **假成功** 收尾。本需求在 **保留 `presets/schemas/ce-executor-serial.yml` 作为 payload SSOT** 的前提下，把 agent 可见面收敛为 **请求 JSON + 统一响应 JSON（EmitResult）**；Ralph 内部可保留 gate，但 **只通过标准响应告诉下一 hat 怎么办**。同时 **从 serial preset 删除 `progress-steward` hat**（及 `progress_steward` 启用），救场改由 runtime 确定性处理，不再 spawn 第二个 LLM coordinator。

---

## Problem Frame

### 谁在受影响

- **Operator**：`ralph run -H builtin:ce-executor-serial` 期望一次 run 正规闭环；现状是 unit/review/收尾多段可炸，且常 **exit 0 但链断**（silent-success）。
- **维护者**：每份诊断催生新 gate / HARD RULE，preset 3000+ 行；修 A 入口 B 再炸，**30 天内同簇复发 9+ 次**（见 `docs/report/*ce-executor-serial*`）。

### 根因（对话共识）

1. **多源状态、无单一对外协议**：schema 只管 payload 字段；被拒后 agent 有时拿 `validation_errors`，有时拿 `task.resume` 散文，有时读 progress.md / PHASE GATE — **入口不统一**。
2. **失败默认 fail-open**：`plan.blocked` + shipper recoverable 白名单 → pass / pass_with_residuals；链断仍可 `LOOP_COMPLETE`。
3. **救场叠 LLM 层**：`progress-steward` 在 `loop.stalled` 时再唤醒一个 hat 猜 recovery；报告里常 **发错 `task.resume`、触发 `loop_stalled_max_iterations` 假成功**，与机制层 `stall_recovery` / `handoff_dispatch_timeout` **重复且更不可控**。
4. **验证与实跑脱节**：BDD mock 绿 ≠ 金丝雀 plan 实跑绿；无 SC1×3 硬门禁。

### 本需求不重复做的事

- 不重写整个 Ralph 为全新 RPC 框架（方案 B 全量 Hat Completion API）— 首轮走 **强化现有 `ralph emit`**（方案 A）。
- 不改动 `builtin:ce-executor-supervisor` 的 progress-steward（另一 preset，本轮 out of scope）。
- 不在本轮删除 events.jsonl 审计账本、isolated 隔离、内部 validation pipeline — 它们退居 **响应生成逻辑**，不对 agent 各写一套说明。

---

## Key Decisions

| 决策 | 理由 |
|------|------|
| **收敛起点 = 统一 emit 响应 JSON（方案 A）** | 成本最低；与现有 `presets/schemas`、`--policy-check`、`--output json` 衔接；用户明确选择 |
| **删除 serial 的 `progress-steward` hat** | 救场 LLM 层鸡肋；与 SSOT 收敛冲突；报告多次帮倒忙或旁路 |
| **stall 改 runtime 确定性收尾** | 删 steward 后不得留真空：hard-fail 或结构化 `task.resume`，禁止再 spawn 救场 hat |
| **机制内化、prompt 减法** | coordinator PHASE GATE / 重复 HARD RULE 随 `allowed_next` + `handoff` 降级为引用 skill，不删质量标准 |
| **验收 = 正规链 + 响应可自洽** | 与 2026-07-02 phase authority 的 SC1 对齐；加「agent 仅凭 EmitResult 知下一步」 |

---

## Requirements

### 协议 SSOT（Agent 可见面）

- **R1.** Agent 与 Ralph 的 **唯一写盘动作** 仍为 `ralph emit <topic>`（及既有 `ralph wave emit`）；请求 body 为 JSON payload，字段约束以 `presets/schemas/ce-executor-serial.yml` 为 SSOT（延续 handbook 约定）。
- **R2.** 定义并强制 **`EmitResult` 响应 SSOT**（`ralph emit` 与 `--policy-check` 均返回同一形状；dry-run 时 `recorded: false`）：
  - `ok: bool`
  - `recorded: bool` — 是否写入 trusted events
  - `topic: string`
  - `phase: string` — 当前 workflow phase（与 `mechanism.phase_authority` 一致，如 `unit_loop` / `review` / `fix_units` / `plan_end` / `ship` / `terminal`）
  - `errors: [{ code, field?, message, suggested_command? }]`
  - `allowed_next: string[]` — 当前 hat 在当 phase 下 **允许发送的 topic 列表**
  - `handoff: object` — 下一激活所需快照（至少可含 `task_id`, `task_key`, `step`, `plan_name`, `plan_path`, `loop_anchor` 等；字段集由 schema 附录定义，单源维护）
  - `activate_next: string | null` — 可选，runtime 建议下一 hat id
- **R3.** 所有现有机制（execution_contract、dedup、phase_authority、step_handoff、enforce_hat_scope 等）**不得再向 prompt 注入与 EmitResult 字段语义重复的散文 recovery 说明**；拒收原因必须可映射到 `errors[]`，修复指引映射到 `suggested_command`。
- **R4.** `--policy-check` 与正式 emit 的校验 pipeline **同源**；差异仅 `recorded`。agent 预检通过后去掉 `--policy-check` 再落盘的行为不变（见 `ralph-tools-emit`）。

### 删除 progress-steward（serial 专用）

- **R5.** 从 `presets/en/ce-executor-serial.yml` **完全删除** `progress-steward` hat 定义（`triggers` / `publishes` / `instructions` 及拓扑注释中的 10-hat 表述改为 **9 hat**）。
- **R6.** `event_loop.progress_steward.enabled` 在 serial preset 上设为 **`false`**（或等效：不配置 steward_hat）；`loop.stalled` **不得**再用于唤醒 LLM 救场 hat。
- **R7.** `tasks.coordinator_hats` 收窄为 **仅 `[coordinator]`**；删除 `progress-steward` 条目（与 2026-07-04-003 plan U7 意图一致，但不再保留 steward 例外）。
- **R8.** `presets/schemas/ce-executor-serial.yml` 中 progress-steward 的 topic 归属注释删除或改注「serial 无此 hat」；`build.rs` merge 后 preset_lint / SSOT byte-equality 仍绿。
- **R9.** stall / no-progress 路径在删 steward 后必须由 **runtime 确定性逻辑** 收尾，二选一（plan 阶段定实现，需求层禁止真空）：
  - **fail-close**：emit 结构化 `plan.blocked` + `EmitResult.ok=false`，且 **禁止** shipper 将此类 reason 提升为 pass；或
  - **deterministic resume**：runtime 直接注入带 `target_hat` + `suggested_command` 的 `task.resume`（不经 LLM hat）。
- **R10.** `LOOP_COMPLETE` honored 后，禁止任何 recovery 注入（延续 R12 / `serial_phase_post_loop_steward_silent` 语义）；删 steward 后由 runtime guard 保证，不依赖 hat instructions。

### Preset 减法（与 EmitResult 配合）

- **R11.** 删除或降级 coordinator **PHASE GATE 决策大表** 及与 `allowed_next` 重复的 **DO NOT emit 墙**（保留质量标准、TDD、review 维度说明）；路由权威在 runtime phase + EmitResult。
- **R12.** Hat `instructions` 涉及 emit 语法 / 拒收处理时 **引用** `crates/ralph-core/data/ralph-tools-emit.md` 的 EmitResult 字段说明，**不复述**响应形状（防漂移）。

### 非回归与范围

- **R13.** 改动不得破坏 `builtin:ce-executor-pipeline` 及未 opt-in phase authority 的 preset（R15–R17 同 2026-07-02 phase authority 需求）。
- **R14.** 本轮 **不**删除 runtime 内 `ProgressStewardConfig` 类型及其它 preset 对 `progress_steward` 的可选配置 — 仅 serial **停用**。

### 文档与下游同步

- **R15.** 更新 `crates/ralph-core/data/ralph-tools-emit.md`：新增 EmitResult 字段表与示例；`scripts/check-cli-doc-drift.sh` 通过。
- **R16.** 同步 `CLAUDE.md` / `AGENTS.md` builtin preset 描述（9-hat）；`scripts/ralph-zsh-plugin.zsh` 若列 hat 则更新。
- **R17.** BDD：调整或替换依赖 `progress-steward` 的场景（如 `serial_phase_post_loop_steward_silent.yml`）；新增场景断言 **EmitResult 含 `phase` + `allowed_next`**（可用 mock）。

---

## Success Criteria

- **SC1.** 金丝雀 plan（`docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`）连续 **3 次** run：events 含且仅含一次 `plan.complete` → `REVIEW_COMPLETE` → `report.done` → `LOOP_COMPLETE`（honored），且 **不以** shipper 越界 pass 或 `plan.blocked` 假成功收尾。
- **SC2.** 删 steward 后，同一金丝雀 plan 的 3 次 run 中 **无** `progress-steward` hat activation；`loop.stalled` 不触发 LLM 救场进程。
- **SC3.** 任意一次 emit 拒收后，agent **仅凭 `EmitResult` JSON**（不需读 progress.md / PHASE GATE）可获知：`errors[].code`、`allowed_next`、`suggested_command`（人工或 scripted 走查验收）。
- **SC4.** `presets/en/ce-executor-serial.yml` 相对当前基线 **减少** progress-steward 相关行 + 与 `allowed_next` 重复的路由 HARD RULE；`preset_lint` + schema parity + `./scripts/run-tests.sh` 全绿。
- **SC5.** stall 触发时 **无 silent-success**：`ok=false` 或确定性 resume，且 shipper **不得**将 steward 删除前的 `loop_stalled_max_iterations` 类 reason 翻成 pass（与 fail-close 策略一致）。

---

## Scope Boundaries

### 本次覆盖

- `builtin:ce-executor-serial`：EmitResult SSOT、删 progress-steward、stall 确定性收尾、preset 路由减法。
- `ralph emit` CLI 响应形状、phase_authority 快照写入响应字段。
- skill 文档与 serial 相关 BDD 调整。

### 本次不覆盖

- 全量 Hat Completion API（方案 B）— 后续若 EmitResult 不够再评估。
- Handoff Envelope 单文件 SSOT（方案 C）— 可在 EmitResult.`handoff` 稳定后第二轮做。
- `ce-executor-supervisor` / `merge-loop` 等其它 preset 的 steward 或 phase 迁移。
- 消灭全部 mechanism 内 gate（contract、dedup 等保留，仅对外收敛）。

### 明确废弃的方向

- 继续为 serial 叠加 progress-steward 式 **LLM 救场 hat**。
- 在 preset instructions 里为每次诊断新增 **平行 recovery 散文**（应进 EmitResult / runtime）。
- shipper narrative 白名单 **对 agent 可见** 的模糊 recoverable 匹配 — 收敛为机器可解析 `errors[].code` + 严格终态枚举。

---

## Outstanding Questions

| ID | 问题 | 默认倾向 |
|----|------|----------|
| **Q1** | stall 默认 fail-close 还是 deterministic `task.resume`？ | **fail-close 优先**（更符合「不假成功」）；resume 仅用于可确定 `target_hat` 的机制路径（如 handoff_dispatch_timeout 且 consumer 明确） |
| **Q2** | `EmitResult` schema 版本化（`emit_result.v1`）是否进 `ralph emit --schema`？ | 是，与 `loop_inspect.v2` 同模式 |
| **Q3** | SC1×3 是否进 CI（非仅文档注记）？ | 应进；具体 job 形态由 plan 定 |

---

## 附录：progress-steward 删除理由（报告摘要）

| 症状 | 报告证据 |
|------|----------|
| 发错 `task.resume` | 153532：safe_target=validator，实际 target=coordinator |
| 假成功收尾 | 224028：`plan.blocked(loop_stalled_max_iterations)` → shipper pass |
| 与机制 recovery 重复 | handoff 600s、`stall_recovery`、`ForcePlanBlocked` 已覆盖；steward 为第三层 LLM 决策 |
| 常未激活或旁路 | 115242、024019：review 链断时 steward 非主因修复点 |

删除后救场责任：**runtime + EmitResult**，不再 spawn「🛟 Progress Steward」hat。

---

**文档路径**：`docs/brainstorms/2026-07-06-ce-executor-serial-protocol-ssot-convergence-requirements.md`

**建议下游**：`ce-plan` 拆为至少两轨 — (1) EmitResult + CLI 响应；(2) serial 删 progress-steward + stall 收尾 + preset 减法；合并前 SC1 子集验证。
