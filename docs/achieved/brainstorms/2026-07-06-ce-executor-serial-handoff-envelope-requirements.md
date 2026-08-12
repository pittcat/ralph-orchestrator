---
date: 2026-07-06
topic: ce-executor-serial-handoff-envelope
status: draft
related:
  - docs/achieved/brainstorms/2026-07-06-ce-executor-serial-protocol-ssot-convergence-requirements.md
  - docs/brainstorms/2026-07-02-ce-executor-serial-runtime-phase-authority-requirements.md
  - docs/brainstorms/2026-06-17-hat-orchestrator-state-projection-requirements.md
  - docs/brainstorms/2026-06-18-isolated-hat-handoff-requirements.md
  - presets/en/ce-executor-serial.yml
  - presets/schemas/ce-executor-serial.yml
---

# ce-executor-serial Handoff Envelope 与状态交接协议

## Problem Frame

`ce-executor-serial` 是 Ralph 里最适合做复杂状态管理实验的 preset。它有 coordinator、executor、validator、fixer、review-coordinator、dimension-reviewer、review-synthesizer、shipper、reporter 等多个 isolated hat，真实运行时需要跨多轮传递目标、计划、当前状态、下一步动作、验证证据和完成信号。

当前事件 payload 已经有 `plan_name`、`plan_path`、`task_id`、`task_key`、`step` 等路由字段，但它还不是完整的工作交接协议。实际语义分散在四处：

- `OBJECTIVE`：保存原始用户目标，但不直接告诉当前 hat 本轮如何完成目标。
- `PENDING EVENTS`：承载触发事件，但多数 payload 只够路由和 gate 校验。
- `ORCHESTRATOR CONTEXT`：承载投影状态，如当前 step、open tasks、fix-unit 状态。
- preset `instructions` / scratchpad：承载大量“该做什么、做完发什么”的散文规则。

结果是 agent 每轮需要自己拼图：从原始目标、事件、投影状态、长 instructions、scratchpad 和 plan 文件里推断“我现在到底该做什么”。这让流程脆弱，也让事件账本难以 replay 出完整的工作交接链。

本需求把 `ce-executor-serial` 的事件 payload 升级为 **Handoff Envelope**：每次 hat 交接都携带明确、结构化、可审计的工作包。目标不是把所有上下文都塞进 payload，而是把 payload、runtime prompt 注入、state projection、memory 的职责分清楚，让 agent 收到 prompt 时直接看到本轮权威交接视图。

## Requirements

**Serial 实验边界**

- R1. 本轮仅面向 `builtin:ce-executor-serial` 启用 Handoff Envelope；不得要求其它 preset 同步迁移，也不得把旧 preset 兼容性作为约束。
- R2. `ce-executor-serial` 可以调整事件 schema、hat instructions、runtime prompt 注入、state projection 视图、EmitResult 字段和 BDD 场景；其它 preset 只允许被动保持不破。
- R3. 任何新机制必须保留 isolated 模式的单业务事件预算：一个 activation 内仍只能产生一个有效业务事件；Handoff Envelope 不得鼓励同轮补发多个信号。

**Handoff Envelope 三层模型**

- R4. 每个关键 handoff topic 的 payload 必须逐步收敛到三层结构：`identity`、`state`、`contract`。字段可在 JSON 中扁平化或嵌套，规划阶段定格式，但语义必须稳定。
- R5. `identity` 层描述“这是谁交给谁、属于哪条工作链”：至少覆盖 `plan_name`、`plan_path`、`task_id`、`task_key`、`step`、`phase`、`from_hat`、`to_hat`、`loop_id`。
- R6. `state` 层描述“为什么做、现在到哪”：至少覆盖 `root_goal`、`plan_summary`、`current_state`、`completed_so_far`、`remaining_scope`、`constraints`、`non_goals`。
- R7. `contract` 层描述“当前 hat 本轮要做什么、做完发什么”：至少覆盖 `next_action`、`expected_output`、`success_signal`、`failure_signal`、`evidence_required`、`context_refs`、`artifact_refs`。
- R8. `success_signal` 与 `failure_signal` 必须是机器可读对象，至少包含 `topic` 和 required field 摘要；agent 不应再从长 instructions 里猜合法完成信号。
- R9. Envelope 中的长上下文必须用 repo 相对路径引用，不把 plan 全文、diff 全文、findings 全文复制进事件 payload。

**Prompt 合成视图**

- R10. Isolated `build_prompt` 路径必须为 `ce-executor-serial` 注入统一的 `## HANDOFF ENVELOPE` 区块，位置应靠近 `## HAT IDENTITY` / `## ORCHESTRATOR CONTEXT`，早于 hat 长 instructions。
- R11. `## HANDOFF ENVELOPE` 是 agent 本轮行动的第一读取入口；它必须合成当前 pending event payload、原始 objective、runtime state snapshot、phase authority、EmitResult/allowed-next 能力中的相关信息。
- R12. `## HANDOFF ENVELOPE` 必须显式回答五个问题：最终目标是什么、当前状态是什么、当前 hat 要做什么、成功时发什么、失败时发什么。
- R13. 若 pending event 已含完整 Handoff Envelope，prompt 注入应直接渲染该 envelope；若旧 payload 缺字段，runtime 可以从 `OBJECTIVE`、`ORCHESTRATOR CONTEXT`、schema 和 phase authority 派生过渡版 envelope，但必须标注哪些字段是派生值。
- R14. 已有 `next_hint` 机制只能作为兼容输入或短提示来源，不能替代结构化 `next_action` / signal contract。

**关键 Topic 迁移**

- R15. 第一批必须迁移的 handoff topic 是：`work.ready`、`work.done`、`work.failed`、`test.passed`、`test.failed`、`review.start`、`review.dimension.ready`、`review.dimension.done`、`review.dimensions.complete`、`review.complete`、`fix.applied`、`fix.exhausted`、`plan.complete`、`plan.blocked`、`REVIEW_COMPLETE`、`report.done`。
- R16. `work.ready` 必须成为 coordinator 到 executor 的完整工作包：包含当前 unit 的目标、范围、相关 plan/context 文件、第一步动作、成功 `work.done` 合约和失败 `work.failed` 合约。
- R17. `work.done` 必须成为 executor 到 validator 的验证工作包：包含完成摘要、改动证据、测试证据、下一步验证动作和 validator 的成功/失败信号。
- R18. `test.passed` / `test.failed` 必须成为 validator 到 coordinator/fixer 的状态判定包：明确测试结论、失败证据、当前 step、下一阶段路由和允许的后续信号。
- R19. `review.dimension.ready` 必须成为 review-coordinator 到 dimension-reviewer 的审查工作包：明确只审哪个 dimension、审查 focus、diff/base、相关文件、禁止越权范围、成功/失败信号。
- R20. `review.complete`、`plan.complete`、`REVIEW_COMPLETE`、`report.done` 必须携带足够状态，防止链断、假成功或收尾阶段只靠 narrative 判断。

**状态、账本与 Memory 分工**

- R21. Event ledger 的职责是记录事实和 handoff envelope；它必须短、结构化、可 replay，不承担长文档存储职责。
- R22. State projection 的职责是从已接受事件推导当前状态，并生成 `ORCHESTRATOR CONTEXT`；agent 不应手写 `.ralph/agent/tasks.jsonl` 或 `.ralph/agent/progress.md` 来维护当前状态。
- R23. Memory 的职责是跨 loop 保存经验、决策和复用知识；不得把当前 step、当前 task open/closed、当前 phase 这类临时运行态放进 memory 作为权威来源。
- R24. Handoff Envelope 的职责是本次 hat-to-hat 交接工作包；它可以引用 memory 或上下文文件，但不能要求下游从 memory 推断当前运行态。
- R25. Scratchpad 可以保留为中间产物索引和本 loop 的辅助记录，但不能成为比 event ledger / state projection 更权威的状态源。

**Runtime 与 Schema 约束**

- R26. `presets/schemas/ce-executor-serial.yml` 必须成为 Handoff Envelope payload contract 的 single source of truth；新增 required fields、allowed values、element constraints 时必须保持 preset lint 和 drift check 可验证。
- R27. `ralph emit --policy-check --output json` 必须能返回 envelope 相关错误，例如缺少 `next_action`、signal topic 不在当前 hat publishes、`context_refs` 不是 repo 相对路径。
- R28. EmitResult 应与 Handoff Envelope 对齐：`phase`、`allowed_next`、`activate_next`、`errors[]` 应能解释为什么某个 envelope 合法或非法。
- R29. Runtime 必须尽量从 `HandoffIndex` / phase authority / hat publishes / event schema 派生 `success_signal` 与 `failure_signal` 的可选值，避免在 YAML instructions 里维护第二套路由表。
- R30. 当 envelope 与 runtime state 冲突时，runtime state 优先。例如 payload 声称 `step=step-03`，但 `ORCHESTRATOR CONTEXT` / phase authority 显示当前只能处理 `step-02`，必须拒收或注入明确 correction。

**Preset Instructions 减法**

- R31. Hat instructions 应从“长篇流程路由说明”逐步降级为角色职责、质量标准、领域审查标准和必要约束；重复 `success_signal` / `failure_signal` / phase routing 的内容应迁移到 Handoff Envelope 和 EmitResult。
- R32. Instructions 涉及命令语法、policy-check、single-event budget、task_id/task_key/step 三字段约束时，必须引用 `crates/ralph-core/data/*.md` 对应 skill 文档，不复制长段规则。
- R33. 对 agent 直接有用的 envelope 行为必须同步到 `crates/ralph-core/data/ralph-tools-emit.md` 或新增 `ralph-tools-handoff-envelope.md`，让 agent 能按需加载深参考。

**可观测性与 Replay**

- R34. 一次 serial run 的 events JSONL 必须能 replay 出：root goal、plan identity、每个 step 当前状态、每次 hat 交接的 next action、成功/失败信号、最终完成或阻塞原因。
- R35. `ralph diagnose` 或后续诊断能力应能从 Handoff Envelope 中指出链断点：哪个 hat 收到什么 envelope、缺哪个字段、发错哪个 signal、状态与 phase 哪里冲突。
- R36. 运行产物中不得出现“事件看起来成功但无法解释下游为什么被激活”的黑箱交接；每个关键 handoff 都必须有 envelope 或 runtime 派生说明。

## Success Criteria

- SC1. `ce-executor-serial` 任一 isolated hat 激活时，prompt 顶部都能看到 `## HANDOFF ENVELOPE`，且该区块能直接回答“我是谁、目标是什么、当前状态是什么、我本轮做什么、做完发什么”。
- SC2. 金丝雀 plan `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md` 的事件账本能 replay 出完整交接链：`work.start` → `work.ready` → `work.done` → `test.passed` → review → ship → report。
- SC3. executor 不需要通过 tail events、猜 scratchpad、重读整份 preset instructions 来判断当前 unit 的成功/失败信号。
- SC4. dimension-reviewer 不需要通过拓扑知识推断自己审什么维度；`review.dimension.ready` envelope 已明确 dimension、focus、scope、context refs 和 signal contract。
- SC5. 当 agent 发出缺字段、错 phase、错 signal topic 的事件时，`--policy-check --output json` 和 loop runtime 给出同源错误，并能指向 envelope 中的具体字段。
- SC6. Memory 中不再承载当前 step/phase/open task 这类临时运行态；运行态以 event ledger + state projection 为准。
- SC7. `presets/en/ce-executor-serial.yml` 中与路由重复的 instructions 明显减少，角色质量标准保留，路由/信号契约转入 schema、Envelope 和 EmitResult。
- SC8. 相关校验通过：`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`、`cargo nextest run -p ralph-core -- preset_lint`、`cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`，最终全量 `./scripts/run-tests.sh` 绿。

## Scope Boundaries

**本次覆盖**

- `builtin:ce-executor-serial` 的 Handoff Envelope 协议。
- Isolated `build_prompt` 的 `## HANDOFF ENVELOPE` 注入。
- `presets/schemas/ce-executor-serial.yml` 的关键 topic payload contract。
- `presets/en/ce-executor-serial.yml` 的 instructions 减法。
- EmitResult / phase authority / state projection 与 envelope 的对齐。
- `crates/ralph-core/data/*.md` 中 agent 需要知道的 emit/handoff 文档同步。
- BDD / replay / preset_lint / doc drift 校验。

**本次不覆盖**

- 不要求 `ce-executor-pipeline`、`ce-executor-supervisor`、`merge-loop` 等 preset 迁移。
- 不重写 Ralph 为全新 workflow DSL。
- 不把长 plan、diff、findings 全文塞进 event payload。
- 不把 memory 改成当前运行态数据库。
- 不要求一次提交内删除所有旧 recovery / task.resume 路径；可以在 serial envelope 稳定后继续收敛。

## Key Decisions

- **D1. serial 作为实验场，允许大胆改。** `ce-executor-serial` 本来就是复杂状态管理实验 preset，不让其它 preset 兼容性拖慢这个方向。
- **D2. Envelope 是交接协议，不是全文教材。** payload 放结构化摘要和路径指针，长内容仍在 plan/context/findings/report 文件里。
- **D3. prompt 合成视图先行。** 先让 agent 在 prompt 中看到统一 envelope，再逐步收紧 schema；这样可以边跑边发现哪些字段真正有用。
- **D4. runtime state 优先于 agent narrative。** envelope 中任何状态字段都要能被 state projection / phase authority 校验或纠偏。
- **D5. memory 退出当前状态管理。** memory 只保存跨 loop 经验，不承担当前 step/phase 的权威职责。

## Dependencies / Assumptions

- 现有 `OBJECTIVE` 注入已能保存原始用户目标。
- 现有 `ORCHESTRATOR CONTEXT` 已能提供 plan/current step/open tasks/fix-unit/review summary 等状态视图。
- 现有 `phase_authority` 和 EmitResult 能提供 `phase`、`allowed_next` 等路由信号，可作为 envelope 合法性来源。
- 现有 `presets/schemas/ce-executor-serial.yml` 是 serial payload contract 的 authoring SSOT。

## Outstanding Questions

### Resolve Before Planning

- （无。用户已确认三层模型、prompt 合成、状态/memory 分工都要做，且范围限定为 `ce-executor-serial`。）

### Deferred to Planning

- [Affects R4-R9][Technical] Handoff Envelope 在 JSON 中采用嵌套结构还是扁平字段，以兼容现有 schema gate 和 CLI 输出。
- [Affects R10-R14][Technical] `## HANDOFF ENVELOPE` 的精确 prompt 位置、最大 token 预算和缺字段时的派生/标注格式。
- [Affects R15-R20][Technical] 第一批 topic 是一次性全部改 schema，还是按 `work.ready` → `work.done/test.*` → review → terminal 分阶段落地。
- [Affects R26-R30][Technical] Envelope contract 应在哪一层执行校验：event_policy schema、validation pipeline、EmitResult routing，或三者组合。
- [Affects R33][Technical] 新增 `ralph-tools-handoff-envelope.md` 还是扩展现有 `ralph-tools-emit.md`。

## Next Steps

-> `/ce:plan docs/brainstorms/2026-07-06-ce-executor-serial-handoff-envelope-requirements.md` for structured implementation planning.
