---
title: Preset Artifact-First Handoffs - Plan
type: feat
date: 2026-07-16
topic: preset-artifact-first-handoffs
artifact_contract: ce-unified-plan/v1
artifact_readiness: requirements-only
product_contract_source: ce-brainstorm
execution: code
---

# Preset Artifact-First Handoffs - Plan

## Goal Capsule

- **Objective:** 增强 `ralph-preset-author` 与 `ralph-preset-review`，使其默认要求 preset 中的 hat 及其 sub-agent 将重要状态和关键信息落盘，并以短消息或事件传递产物引用。
- **Product authority:** 本文的 Product Contract 约束本次 skill 行为；现有 Ralph runtime、isolated hat 可见性和内部 ledger 边界继续作为上游约束。
- **Open blockers:** 无。

---

## Product Contract

### Summary

`ralph-preset-author` 将引导作者设计 artifact-first 的 hat 执行与交接，`ralph-preset-review` 将其作为默认强制的 AAF 审核维度。完整内容、恢复状态与证据写入当前 workspace/worktree 的 `.ralph/`，消息和事件主要承担控制与导航。

### Problem Frame

当前 author/review 重点覆盖事件拓扑、payload 可构造性、状态投影和 handoff closure，但没有系统要求 hat 或其 sub-agent 把执行状态与关键产物落盘。sub-agent 的完整结果可能只通过长消息返回，跨 hat 的分析和证据也可能被塞入 event payload 或 prompt。

这种交接在上下文压缩、消息截断、hat 重启或后续复核时不稳定。下游可能被迫重复调查，主 hat 也难以确认 sub-agent 的结果是否已形成可恢复、可审计的产物。

### Key Decisions

- **文件是重要信息的事实源。** 若信息丢失会导致无法恢复、重新调查、证据不可审计或再次调用 sub-agent，该信息默认必须落盘。
- **事件承担控制面，文件承担数据面。** 事件和消息保留短状态、摘要、路径、必要身份与路由字段；完整结果、证据、进度和决策依据放在文件中。
- **默认强制但允许有理由的例外。** 短暂、短小且无需恢复的信息可以不落盘；author 必须能够说明理由，review 必须验证理由成立。
- **落盘位置限定在当前 `.ralph/`。** 实际执行的 hat 或 sub-agent 可以按任务创建专用文件或目录，但不得把 Ralph 内部 ledger 当作业务 artifact 接口。
- **不使用字符数作为主判据。** 是否必须落盘由恢复价值、审计价值和下游依赖决定。

### Actors

- A1. **Preset author：** 使用 `ralph-preset-author` 设计每个 hat 及其 sub-agent 的落盘点、引用方式和生命周期责任。
- A2. **Preset reviewer：** 使用 `ralph-preset-review` 验证这些设计在单 hat activation 视角下可执行，并形成完整交接。
- A3. **Producing hat：** 执行 preset 时创建或更新 `.ralph/` 下的 artifact，并通过短消息或事件发布引用。
- A4. **Hat-owned sub-agent：** 将完整结果、证据和未解决问题写入指定 artifact，只向调用 hat 返回短状态与路径。
- A5. **Consuming hat：** 从可见的交接字段取得路径，读取 artifact，并确认内容足以继续工作。

### Requirements

**Authoring guidance**

- R1. `ralph-preset-author` 必须把 artifact-first 交接加入 topology、单 hat AAF、Payload Contract 和 pre-review gate，而不是仅作为写作建议。
- R2. author 必须识别并要求落盘 sub-agent 完整结果、跨 hat 长内容、可恢复进度、关键决策依据、验证证据以及高成本重建的信息。
- R3. author 必须要求实际执行的 hat 或 sub-agent 在当前 workspace/worktree 的 `.ralph/` 下创建或更新业务 artifact；preset 本身不得被描述为文件创建者。
- R4. author 必须让消息或事件优先传递短状态、摘要、artifact 路径、必要身份和路由字段，并让下游 hat 明确读取路径指向的完整内容。
- R5. author 必须为每份重要 artifact 指定产出责任、消费责任以及最终保留、归档或清理责任。
- R6. author 必须阻止 hat 把 `.ralph/events.jsonl`、`.ralph/loops.json`、`.ralph/supervisor.db` 等 runtime 内部 ledger 当作自定义状态或交接文件。
- R7. author 允许短暂、短小且无需恢复的信息只存在于消息或事件中，但必须记录可审核的例外理由。

**Review guidance**

- R8. `ralph-preset-review` 必须新增 artifact-first 审核，逐项验证重要信息是否落盘、路径是否来自当前 hat 可见来源、下游是否明确读取，以及内容是否足以恢复或继续决策。
- R9. review 必须检查 sub-agent 的完整结果是否在消息返回前已经落盘；只有长消息而没有 artifact 的设计不得通过。
- R10. review 必须检查每条 artifact handoff 的闭环：产出动作、路径传递、下游读取、消费确认和生命周期责任均有可执行依据。
- R11. review 必须把无合理例外的重要信息未落盘视为正式 finding，并按其对恢复、审计或下游执行的影响确定严重度。
- R12. review 不得因为 payload 含有文件路径就判定通过；还必须验证路径可见性、文件语义、消费动作和责任归属。

### Key Flows

- F1. **Hat 将工作委派给 sub-agent**
  - **Actors:** A3、A4
  - **Steps:** hat 指定 `.ralph/` 下的产物位置；sub-agent 执行任务并写入完整结果、证据和未解决问题；sub-agent 只返回完成状态、短摘要和路径；hat 读取并验收文件。
  - **Covered by:** R2、R3、R5、R9
- F2. **Hat 向下游交接重要信息**
  - **Actors:** A3、A5
  - **Steps:** 产出 hat 写入 artifact；事件携带路径与短控制信息；消费 hat 从可见字段取得路径并读取完整内容；消费 hat 依据文件继续决策。
  - **Covered by:** R3、R4、R5、R8、R10、R12
- F3. **Author 与 reviewer 处理不落盘例外**
  - **Actors:** A1、A2
  - **Steps:** author 说明信息为何短暂、短小且无需恢复；review 从单 activation 视角验证该判断；理由不足时形成 finding。
  - **Covered by:** R7、R11

### Acceptance Examples

- AE1. **Covers R2, R4, R9.** 给定一个 hat 要求 sub-agent 生成完整审查报告，当 sub-agent 仅在返回消息中粘贴报告时，author gate 或 review 必须拒绝该设计；当报告先写入 `.ralph/` 且消息只返回状态、摘要和路径时，该项可以通过后续审核。
- AE2. **Covers R4, R8, R10, R12.** 给定上游事件包含 artifact 路径，当下游 hat 的可见上下文无法取得该路径或 instructions 未要求读取文件时，review 必须把 handoff 判为未闭环。
- AE3. **Covers R3, R6.** 给定 hat 需要保存阶段状态，当其 instructions 要求写入专用 `.ralph/` 业务 artifact 时允许继续；当其要求直接复用 runtime 内部 ledger 时必须拒绝。
- AE4. **Covers R5, R10.** 给定多个阶段持续产生中间文件，当 preset 没有明确消费方或最终保留、归档、清理责任时，review 必须指出生命周期缺口。
- AE5. **Covers R7, R11.** 给定事件只携带一个可立即重算的短计数，当 author 说明它无需恢复且下游不依赖历史证据时，可以作为不落盘例外；不能仅以“字符很短”作为理由。

### Success Criteria

- author 能在交付 preset 前发现“sub-agent 只回长消息”“跨 hat 搬运完整正文”和“重要状态仅在上下文中存在”等设计。
- reviewer 能以字段来源、路径可见性、消费动作和生命周期责任为证据，给出可修复的 artifact-first finding。
- 合规 preset 的事件与消息保持短小，但下游仍能从 `.ralph/` artifact 恢复完整上下文并继续执行。
- 新规则保持轻量，不要求统一目录命名、不引入 runtime 改造，也不强迫临时思考全部落盘。

### Scope Boundaries

- 不修改 Ralph runtime、event loop、state projection 或内部 ledger。
- 不为所有 preset 规定统一的 `.ralph/` 子目录结构或文件格式。
- 不优化 `ralph-preset-author` 与 `ralph-preset-review` 自身运行时的多 agent 状态管理。
- 不要求所有短消息、临时思考或可低成本重算的信息落盘。
- 不改变现有 AAF、Payload Contract、policy-check 与 mechanical lint 的职责，只扩展其审核维度。

### Dependencies / Assumptions

- hat 与其 sub-agent 对当前 workspace/worktree 的 `.ralph/` 业务 artifact 路径具有读写能力。
- preset 仍需遵守 isolated activation 的可见性边界；artifact 路径必须通过当前 hat 可见的输入获得。
- `.ralph/` 中由 hat 创建的业务 artifact 与 Ralph 内部 ledger 必须保持概念和操作边界。

### Sources / Research

- `skills/ralph-preset-author/SKILL.md`：现有 author AAF、Payload Contract、handoff 与 pre-review gate。
- `skills/ralph-preset-review/SKILL.md`：现有 per-hat AAF、Payload Audit、Handoff Audit 与 finding 规则。
- `crates/ralph-core/data/ralph-tools-emit.md`：事件落盘、单事件预算和 agent 可见事件文件约束。
- `docs/plans/2026-07-16-001-refactor-ce-unified-plan-pipeline-plan.md`：现有 pipeline 中以 artifact 路径替代长内容交接的相邻方案。
