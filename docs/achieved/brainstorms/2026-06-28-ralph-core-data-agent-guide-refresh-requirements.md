---
date: 2026-06-28
topic: ralph-core-data-agent-guide-refresh
title: "ralph-core/data AI 指南刷新：对齐代码更新 + 场景导航"
---

# ralph-core/data AI 指南刷新：对齐代码更新 + 场景导航

## Problem Frame

`crates/ralph-core/data/*.md` 是 loop 内 agent 看到的内置 skill 文档源：

- `ralph-tools.md` 在 `memories.enabled || tasks.enabled` 时每轮自动注入 prompt（`crates/ralph-core/src/event_loop/mod.rs:4479-4582`）。
- `ralph-tools-tasks.md` / `ralph-tools-memories.md` 分别随 task/memory 启用注入。
- `ralph-tools-emit.md` / `ralph-tools-wave.md` / `ralph-tools-cmdref.md` 注册为 built-in skill，按需 `ralph tools skill load`。

近两个月运行时能力已经多次迭代（U11 StateLedger、`ralph emit --schema`、worktree 显式复用、`--no-default-profiles` / `--no-sync-agent-docs`、task/memory flag 调整、policy-check unified pipeline 等），但 data 文档：

1. **参数/命令与当前 `--help` 不一致** — `check-cli-doc-drift.sh --strict` 会报出 task/memory/cmdref/emit/wave 多处 drift。
2. **新行为/概念讲解不足或缺失** — StateLedger 恢复、`ralph emit --schema` 用途、profile overlay、`--no-sync-agent-docs` 等没有出现在 AI 指南里。
3. **AI 不容易判断「什么时候该用哪个 skill」** — 命令按命名空间平铺，缺少以 agent 实际决策点为入口的导航。
4. **缺少典型场景的完整示例和决策链** — 例如从 `task.resume` 到选择修复命令、从 wave 派发到 dimension 评审、从 memory 搜索到 decision journal 记录。

结果：agent 在 loop 里要么用错命令/flag，要么在多个 skill 文件之间空转，增加了人工干预和迭代次数。

---

## Actors

- **A1 — Loop 内 agent（主要用户）**：每轮 prompt 自动看到 `ralph-tools.md`，需要靠它快速判断当前情况该读哪份 skill、执行哪条命令。
- **A2 — 维护者**：后续新增/修改 CLI 时，需要知道 data 文档应同步到何处、如何保持与代码一致。

---

## Key Flows

- **F1. Agent 收到 `task.resume` 后修复**
  - **Trigger:** policy / origin / payload_contract / execution_contract 拒收，loop 注入 `task.resume`。
  - **Actors:** A1
  - **Steps:**
    1. 读自动注入的 `ralph-tools.md` §「收到 `task.resume` 时」。
    2. 按字段（`stage`, `topic`, `violation`, `required_fields`, `allowed_topics`）定位原因。
    3. 选择修复命令：`ralph emit --policy-check` 补字段、按需 `ralph tools skill load ralph-tools-emit` 查详表、handoff 类问题 load handoff skill。
  - **Outcome:** 无需人类提示即可继续 loop，且不依赖 unsafe bypass。
  - **Covered by:** R1, R2, R3, R5

- **F2. Agent 需要发射事件但不确定 emit vs wave**
  - **Trigger:** 需要推进 step、触发 review wave、或返回 wave worker 结果。
  - **Steps:**
    1. 在 `ralph-tools.md` 场景导航表中找到对应场景（单事件 / wave 派发 / worker 返回）。
    2. 按链接 load `ralph-tools-emit.md` 或 `ralph-tools-wave.md`。
    3. 使用 `--policy-check` 预检后正式发出。
  - **Outcome:** 事件正确写入 allowlist 内的事件文件，且通过 policy 校验。
  - **Covered by:** R2, R4

- **F3. Agent 需要管理 task / memory / decision journal**
  - **Trigger:** 需要创建/关闭任务、记录决策或查询记忆。
  - **Steps:**
    1. 场景导航表提示 `ralph tools task ensure` 与 R4 Single-U 契约、`ralph tools memory prime` 等典型用法。
    2. 按需 load `ralph-tools-tasks.md` / `ralph-tools-memories.md` 查参数。
  - **Outcome:** 任务/记忆状态正确，不违反 R4 契约。
  - **Covered by:** R2, R6

- **F4. Agent 遇到崩溃/恢复/诊断**
  - **Trigger:** loop 异常退出、ledger 损坏、需要 post-mortem。
  - **Steps:**
    1. `ralph-tools.md` 场景导航指向 `RALPH_DIAGNOSTICS=1` + `ralph diagnose --session latest`。
    2. 按 `docs/guide/runtime-diagnosis.md` 决策树处理。
  - **Outcome:** 能独立收集诊断信息并按文档恢复。
  - **Covered by:** R7

---

## Requirements

**一致性与准确性**

- R1. 审计并修正 `crates/ralph-core/data/*.md` 中所有 `*.rs:NN` 源码行号引用，确保与当前代码对齐（AGENTS.md 反向验证规则）。
- R2. 更新 `ralph-tools-tasks.md`、`ralph-tools-memories.md`、`ralph-tools-cmdref.md`、`ralph-tools-emit.md`、`ralph-tools-wave.md` 的参数表与示例，使其与当前 `ralph <cmd> --help` 一致；`scripts/check-cli-doc-drift.sh --strict` 对本次刷新覆盖的命令应无新漂移。
- R3. 在 `ralph-tools.md` 的「收到 `task.resume` 时」段补充当前 rejection payload 字段解释，并明确修复顺序：读 PENDING EVENTS payload → 对照 `required_fields` 补齐 → `ralph emit --policy-check` 预检 → 重试；继续禁止 unsafe bypass 和直写 `events.jsonl`。

**新行为/概念讲解**

- R4. 在 `ralph-tools.md` 或 `ralph-tools-emit.md` 中加入 `ralph emit --schema <TOPIC>` 的用途说明（查 embedded 协议、检测 preset schema drift、与 `protocol_hash` 配合）。
- R5. 在 `ralph-tools.md` 中加入 U11 StateLedger / `ralph loops clean --ledger` 的简要说明，让 agent 知道崩溃后 iteration/rejection digest 会从 ledger 重建。
- R6. 在 `ralph-tools.md` 中加入 `ralph run` 新增 flag 的说明：`--no-default-profiles`（跳过默认 profile overlay）、`--no-sync-agent-docs`（跳过 CLAUDE.md/AGENTS.md managed block 同步），以及 `ralph inspect profiles` 的用途。

**场景导航**

- R7. 在 `ralph-tools.md` 顶部新增 **「AI 决策速查 / 何时看哪份 skill」** 小节（控制在 25–40 行），按场景而非命令命名空间组织：
  - loop 拒绝 / `task.resume` → `ralph-tools.md` 本段 + 按需 `ralph-tools-emit.md` / handoff skill
  - 发射单个事件 / schema 预检 → `ralph-tools-emit.md`
  - 派发 review wave / 作为 wave worker 返回 → `ralph-tools-wave.md`
  - task 管理（含 R4 Single-U）→ `ralph-tools-tasks.md`
  - memory / decision journal → `ralph-tools-memories.md`
  - worktree 复用 / `ralph run` 参数 → `ralph-tools-cmdref.md`
  - 诊断 / ledger 恢复 → `docs/guide/runtime-diagnosis.md`
- R8. 保留现有命令速查表，但将其后置或并入场景导航，避免 AI 在「有命令无场景」的平铺列表中迷失。

**示例与决策链**

- R9. 为每个主要场景补充一条完整命令示例（例如 `ralph emit work.done --policy-check -j '{"plan_path":"...","task_id":"..."}'`），并说明「何时用、何时不用」。
- R10. 在 `ralph-tools.md` 通用错误恢复表中新增/刷新当前常见错误：
  - `events file not in allowlist`
  - `policy validation failed`
  - `progress_task_mismatch` / `plan.blocked`
  - `skill not found`（含 `RALPH_CURRENT_HAT` 检查）
  - ledger 损坏 / `cold_start` 降级提示

**内置 skill 注册与 CI**

- R11. 若新增 `.md` 文件，必须按 `crates/ralph-core/src/skill_registry.rs` 的 built-in skill 模式注册，并确保 `ralph tools skill list` 可见。
- R12. 刷新后必须执行：
  - `cargo nextest run -p ralph-core -- skill_registry`（或等价测试）验证 built-in skills 注册与加载。
  - `bash scripts/check-cli-doc-drift.sh --strict` 覆盖本次更新的命令。
  - `bash scripts/guard-prompt-size.sh` 保证 `ralph-tools.md` ≤ 200 行。

---

## Acceptance Examples

- **AE1. Covers R1, R2.** 给定 `ralph tools task add --help` 输出包含 `--description`、`--priority`、`--root`，则 `ralph-tools-tasks.md` 的 Task Commands 段也列出这些 flag，且不再列出 `--help` 已移除的 `--all`、`--key`。
- **AE2. Covers R3, R9.** 给定 loop 注入 `task.resume`，payload 含 `required_fields: ["task_id"]`，agent 读 `ralph-tools.md` 后执行 `ralph emit <topic> --policy-check -j '{"task_id":"..."}'`，不再先尝试 `--unsafe-no-policy-check`。
- **AE3. Covers R4, R5, R6.** 给定 agent 想确认当前 preset 的 `work.done` schema，文档说明使用 `ralph emit --schema work.done | jq -r .protocol_hash`；给定 loop 崩溃后重启，文档说明 iteration 从 StateLedger 重建，若 ledger 损坏可用 `ralph loops clean --ledger`。
- **AE4. Covers R7.** 给定 agent 不确定 emit 与 wave 的区别，读 `ralph-tools.md` 顶部场景导航表后，能在 1 步内决定 load `ralph-tools-emit.md` 还是 `ralph-tools-wave.md`。
- **AE5. Covers R12.** 刷新后的 PR 中，`check-cli-doc-drift.sh --strict` 对 task/memory/emit/wave/cmdref 覆盖的命令无新增漂移；`guard-prompt-size.sh` 通过；`cargo nextest run -p ralph-core -- skill_registry` 通过。

---

## Success Criteria

- SC1. AI 在常见 loop 拒绝场景下，仅依靠自动注入的 `ralph-tools.md` 就能判断下一步动作，无需人类提示。
- SC2. `ralph-tools.md` 中的场景导航让 AI 在 1 步内定位到应 load 的 skill 或应执行的命令。
- SC3. data 文档中源码行号引用与当前代码一致；`*.rs:NN` 无漂移。
- SC4. 本次刷新覆盖的 CLI 命令在 `check-cli-doc-drift.sh --strict` 下无新漂移。
- SC5. `ralph-tools.md` 保持 ≤ 200 行；其他 skill 文件无硬性膨胀。
- SC6. 新增/修改的 built-in skill 能通过 `ralph-core` 的 skill registry 测试。

---

## Scope Boundaries

**Deferred for later**

- 全量自动化的 CI doc ↔ `--help` 漂移门禁（本轮只保证覆盖命令无漂移，不重构脚本）。
- 将 `ralph-tools-handoff.md` 的 handoff 深参考补全（若尚未落地）。
- 把 scenario navigation 做成独立的 auto-inject skill（本轮先放在 `ralph-tools.md` 内，验证效果后再决定是否拆出）。
- 重写 preset instructions 中的 skill 引用（本轮只更新 data 文件本身）。

**Outside this product's identity**

- 修改 CLI 行为或运行时注入策略（data 文档只跟随代码，不驱动代码）。
- 把完整 JSON Schema 或 preset YAML 复制进 data。
- 为 IDE/Claude Code 单独维护一套与 data 不同的 skill 文档（`.claude/skills/ralph-tools/SKILL.md` 保持 symlink）。

---

## Key Decisions

- **保留现有 skill 文件拆分，新增场景导航层**：不改 `ralph-tools-*.md` 的边界，只在 `ralph-tools.md` 顶部加场景索引，降低维护成本。
- **ralph-tools.md 作为唯一自动注入的入口**：所有新概念的入口指针放在这里；深参考仍按需 load，避免 token 膨胀。
- **先修正 drift，再补概念和示例**：不先重构结构，而是确保现有命令表准确后，再叠加场景导航和示例。
- **不独立创建新 skill 文件**：场景导航先内嵌在 `ralph-tools.md`，待使用后再评估是否拆成 `ralph-tools-scenarios.md` 并注册为 built-in skill。

---

## Dependencies / Assumptions

- 依赖当前 CLI 已实现的功能：`ralph emit --schema`、`--no-default-profiles`、`--no-sync-agent-docs`、`ralph inspect profiles`、StateLedger、task/memory 当前 flag 集合。
- 依赖现有测试与门禁：`scripts/check-cli-doc-drift.sh`、`scripts/guard-prompt-size.sh`、`cargo nextest run -p ralph-core -- skill_registry`。
- 假设用户希望本轮以 data 文档刷新为主，不引入新运行时代码。

---

## Outstanding Questions

### Resolve Before Planning

- 无。

### Deferred to Implementation

- R7 场景导航小节放在 `ralph-tools.md` 顶部还是「收到 `task.resume` 时」之后？实施时根据可读性选择。
- R6 中 `--no-default-profiles` / `--no-sync-agent-docs` 等 flag 是放在 `ralph-tools.md` 顶部导航表里简要说明，还是只在 cmdref skill 里详细说明？
- R2 覆盖的命令集合：是覆盖本次 `check-cli-doc-drift.sh` 已映射的全部命令，还是优先修复 agent 高频使用的 emit/task/memory/wave/run？

---

## Next Steps

-> `/ce-plan` for structured implementation planning, or proceed directly to edit `crates/ralph-core/data/*.md` if scope is clear.
