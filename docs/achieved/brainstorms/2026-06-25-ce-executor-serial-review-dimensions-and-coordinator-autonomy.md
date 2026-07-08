---
date: 2026-06-25
topic: ce-executor-serial-review-dimensions-and-coordinator-autonomy
---

# ce-executor-serial 扩展审查维度与 Coordinator 计划调整自主性

## Problem Frame

`presets/en/ce-executor-serial.yml` 当前的终审只走两个固定维度：`correctness` 与 `testing`。与 `ce-code-review` skill 的 reviewer persona 相比，缺少 `maintainability`、`project-standards` 等常规维度，也缺少一轮专门的对抗性审视，导致一些代码结构、命名、AGENTS.md/CLAUDE.md 合规问题、以及隐藏副作用/边界漏洞/安全路径容易被漏掉。

同时，Coordinator 目前严格按初始 plan 推进单元，发现计划需要拆/合单元或调整顺序时没有受控的修正机制，只能被动按原 plan 执行或阻塞。用户希望 Coordinator 对执行路径保留有限自主性：在不改变目标与验收标准的前提下，它能读取**同一个 plan 内**的历史记忆与上一轮输出（test failures、review findings、fix-log），在 scratchpad 中输出一份结构化修正计划（格式借鉴 `ce-plan` 的 Implementation Unit），并在后续执行中复用学到的模式。

---

## Actors

- A1. **Coordinator**：解析 plan、读取历史记忆与上一轮输出、创建/调整运行时任务、推进单元、触发终审，并在必要时生成结构化修正计划。
- A2. **Review Coordinator**：管理终审维度序列，一次只推进一个维度。
- A3. **Dimension Reviewer**：对单个维度做只读审查，输出 findings JSON。
- A4. **Review Synthesizer**：合并多维度 findings，生成 fix-plan。
- A5. **Operator / User**：提供初始 plan，监督循环，必要时介入决策。

---

## Key Flows

- F1. **扩展后的终审序列**
  - **Trigger:** 所有 plan unit 执行完毕，Coordinator 发布 `review.start`。
  - **Actors:** A2, A3, A4
  - **Steps:**
    1. Review Coordinator 初始化 5 维度序列：`correctness` → `testing` → `maintainability` → `project-standards` → `adversarial`。
    2. 依次对每个 pending 维度发布 `review.dimension.ready`。
    3. Dimension Reviewer 按对应维度的 checklist 审查，发布 `review.dimension.done` 或 `review.dimension.failed`。
    4. 全部维度完成后，Review Coordinator 发布 `review.dimensions.complete`。
    5. Review Synthesizer 合并 5 个维度的 findings，输出 `findings.md` 与 fix-plan（如需要）。
  - **Outcome:** 终审覆盖 correctness / testing / maintainability / project-standards / adversarial，P0/P1 进入 fix-plan。
  - **Covered by:** R1, R2, R3, R4, R9

- F2. **Coordinator 计划微调与学习**
  - **Trigger:** Coordinator 在推进过程中发现当前 unit 过大、过小、顺序不当，或从同一 plan 的上一轮输出中发现重复出现的问题模式。
  - **Actors:** A1
  - **Steps:**
    1. Coordinator 在每次激活时读取**同一 plan** 的相关历史：
       - `.ralph/agent/memories.md`
       - `decisions.md`、`fix-log.md`、`findings.md`、`plan-amendments.md`（如已存在）
       - 上一轮失败的 test errors 与 review findings（从上述文件或近期 events 摘要中读取）
    2. 评估当前 plan unit 与代码/测试现实之间的差距，并识别是否有历史模式可以复用。
    3. 若需要调整，按置信度协议决策：
       - 置信度 > 80：生成修正计划并直接执行。
       - 50–80：记录到 `decisions.md` 后继续。
       - < 50：发布 `work.failed`，说明原因，停止等待用户。
    4. 将修正计划写入 `.agents/scratchpad/ce-executor/{plan_name}/plan-amendments.md`，采用借鉴 `ce-plan` 的结构（Requirements、Implementation Units、Test scenarios、Verification），不修改原始 plan 文件。
    5. 若识别出可在同一 plan 内复用的模式，可选择用 `ralph tools memory add` 写入一条 lesson memory，但不得把跨 plan 记忆作为当前决策主因。
    6. 继续按修正后的单元推进。不引入新的 event topic，修正通过 `work.ready` payload 与 scratchpad 文件体现。
  - **Outcome:** 执行路径更贴合实际，Coordinator 能从同一 plan 的历史输出中学习并避免重复踩坑。
  - **Covered by:** R4, R5, R6, R8, R9, R10

---

## Requirements

**审查维度扩展**

- R1. `review-coordinator` 的固定维度序列从 2 个扩展到 5 个，顺序为：`correctness` → `testing` → `maintainability` → `project-standards` → `adversarial`。
- R2. `dimension-reviewer` 必须为 `maintainability` 提供专门的 checklist，与 `ce-code-review` skill 中 `ce-maintainability-reviewer` 的关注点一致：耦合、复杂度、命名、死代码、抽象债务。
- R3. `dimension-reviewer` 必须为 `project-standards` 提供专门的 checklist，与 `ce-code-review` skill 中 `ce-project-standards-reviewer` 的关注点一致：CLAUDE.md / AGENTS.md 合规、frontmatter、引用、可移植性。
- R4. `dimension-reviewer` 必须为 `adversarial` 提供对抗性审查 checklist。该维度作为最后一轮 Red-Team 审视，关注隐藏副作用、兼容性破坏、边界与并发、安全路径、命名误导、测试充分性、维护成本与回滚安全；允许借鉴用户提供的对抗性审查模板并做适当适配，不要求照搬。
- R5. `review-synthesizer` 必须能合并 5 个维度的 findings，并在 Coverage 中按维度统计；fix-plan 的生成逻辑保持不变。
- R9. 所有事件 schema、topic_deny_rules、`review-sequence.json` 结构必须与 5 维度序列保持一致，并通过 preset_lint 与 SSOT byte-equality 校验。

**Coordinator 计划调整自主性**

- R6. Coordinator 在执行过程中被允许识别并自主调整以下执行路径问题：
  - 一个 Implementation Unit 过大，需要拆成多个子单元（U1a / U1b / U1c）。
  - 多个小单元可以安全合并为一个单元。
  - 单元执行顺序需要调整以消除依赖或提前验证。
- R7. Coordinator **不得**直接修改原始 plan 文件；所有调整必须写入 `.agents/scratchpad/ce-executor/{plan_name}/plan-amendments.md`，并注明调整原因、触发时机、对原始 U-ID 的映射。
- R8. Coordinator 对计划调整继续使用全局置信度协议（>80 自主执行、50–80 记录并继续、<50 停止并请求用户）。
- R10. Coordinator 在每次激活时必须读取**同一 plan** 的历史记忆与上一轮输出（`memories.md`、`decisions.md`、`fix-log.md`、`findings.md`、上一轮 `test.failed` / `review.dimension.done` / `review.complete` 的关键字段），作为决策输入。
- R11. Coordinator 生成的修正计划必须写入 `.agents/scratchpad/ce-executor/{plan_name}/plan-amendments.md`，采用结构化格式，至少包含 Requirements、Implementation Units（U-ID、Goal、Files、Approach、Test scenarios、Verification）和变更映射；不调用 `ce-plan` skill，也不修改原始 plan 文件。
- R12. Coordinator 的计划修正**不引入新的 event topic**；调整通过现有 `work.ready` payload 与 scratchpad 文件体现，review-coordinator / shipper / reporter 等下游 hat 的事件契约保持不变。

---

## Acceptance Examples

- AE1. **Covers R1、R2、R3、R4.** 给定一个 plan 执行完毕，当 Coordinator 发布 `review.start`，Review Coordinator 依次触发 `correctness`、`testing`、`maintainability`、`project-standards`、`adversarial` 五个维度审查，最终生成包含全部五个维度 Coverage 的 `findings.md`。
- AE2. **Covers R6、R7.** 给定 U3 在实现时发现需要拆成 U3a（接口）和 U3b（实现），Coordinator 将拆分决定写入 `plan-amendments.md`，并继续创建 `ce-executor:{plan_name}:step-03a:u3a-interface` 与 `ce-executor:{plan_name}:step-03b:u3b-impl` 任务，原始 `docs/plans/my-plan.md` 不被修改。
- AE3. **Covers R8.** 给定 Coordinator 对一次顺序调整只有 45 分置信度，它应发布 `work.failed`，payload 包含 `reason: "plan amendment confidence too low: reorder step-02 and step-03 requires user confirmation"`，而不是擅自调整。
- AE4. **Covers R10、R11、R12.** 给定 U2 在第一轮实现后因 `test.failed` 暴露出接口设计问题，Coordinator 在第二轮激活时读取 `fix-log.md` 与 `findings.md`，生成 `plan-amendments.md` 将 U2 拆为 U2a（接口调整）和 U2b（实现适配），并继续按修正后的单元推进，整个过程不引入新的 event topic。

---

## Success Criteria

- 终审从 2 维度扩展到 5 维度后，preset_lint、SSOT byte-equality、BDD scenarios 全部通过。
- `maintainability`、`project-standards` 与 `adversarial` 的 checklist 能复用既有 reviewer 关注点或用户提供的对抗性审查模板，避免发明新的审查语言。
- Coordinator 在遇到可自主拆/合/ reorder 的场景时，能够记录 amendment 并继续执行，而不是阻塞或违规修改 plan 文件。
- Coordinator 能从同一 plan 的上一轮输出中读取关键信息，并在修正计划中体现 reasoning。
- 计划修正不引入新的 event topic，下游 hat 的事件契约保持不变。
- 原始 plan 文件保持只读，amendment 文件成为可审计的执行路径补充。

---

## Scope Boundaries

- 不引入动态/条件维度选择；5 个维度固定顺序执行。
- 不允许 Coordinator 修改目标、验收标准、范围边界（scope boundaries）或 Requirements Trace。
- Coordinator 的学习范围仅限**同一个 plan 的 earlier iterations**，不把跨 plan 记忆作为当前决策主因。
- 不引入新的 event topic 或 hat；计划修正通过现有 `work.ready` 与 scratchpad 文件体现。
- 不改动 isolated execution mode、topic 所有权、review-passed/review-complete 的终态语义。
- 不引入新的 hat，也不合并现有 hat 的职责。
- 不改动 Shipper / Reporter 的输出结构。

---

## Key Decisions

- **固定 5 维度而非动态维度**：动态选择需要 review-coordinator 根据 changed_files 决策，会显著增加复杂度和测试负担；固定 5 维度能直接复用现有 serial sequence 机制。
- **新增 maintainability + project-standards + adversarial**：前两者补齐通用工程标准，adversarial 作为最后一轮 Red-Team 审视，专门捕捉隐藏副作用、边界漏洞与回滚风险；security / performance / api-contract 等跨领域条件维度仍作为未来可选增强。
- **Amendment 文件隔离原始 plan**：保持原始 plan 的不可变性，避免 plan 文件漂移和 `ralph doctor plan-sync` 冲突。
- **修正计划写内部 scratchpad，不调用 ce-plan skill**：避免 skill 递归和权限边界问题，格式仅借鉴 ce-plan 的 Implementation Unit 结构。
- **学习范围仅限同 plan**：跨 plan 记忆匹配容易误用上下文，先让 Coordinator 在同一 plan 的历史输出中学习。
- **不新增 event topic**：降低对 review-coordinator / shipper / reporter 等下游 hat 的侵入，修正通过现有事件与文件体现。
- **置信度协议复用**：不新建决策规则，沿用现有 `decisions.md` 协议降低心智负担。

---

## Dependencies / Assumptions

- 必须同步更新 `presets/schemas/ce-executor-serial.yml` 中的 event schema、required_fields、execution_contracts 等，以保持 SSOT 一致。
- 必须跑以下校验：`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`、`cargo nextest run -p ralph-core -- preset_lint`、`cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`。
- 如果 BDD scenarios 中硬编码了 `correctness`/`testing` 两个维度，需要同步更新为 5 维度的 mock responses 与 expected events。
- 假设 `dimension-reviewer` 的只读约束和 `disallowed_tools: ["Edit"]` 不变；新增维度 checklist 不得诱导 reviewer 修改源码。

---

## Outstanding Questions

### Resolve Before Planning

- 无。用户已确认维度组合与 Coordinator 自主性范围。

### Deferred to Planning

- [R9][Needs research] BDD scenarios 中是否有对 2 维度序列的硬编码断言，需要逐条核对。
- [R3][Needs research] `project-standards` checklist 中对 AGENTS.md/CLAUDE.md 的引用检查是否需要在无相关文件时降级，避免误报。
- [R11][Technical] plan-amendments.md 的具体 frontmatter 与 U-ID 映射格式需要在实现时与现有 `plan.md` 解析逻辑对齐。

---

## Next Steps

- 进入实现阶段：修改 `presets/en/ce-executor-serial.yml`、同步 `presets/schemas/ce-executor-serial.yml`、更新相关 BDD scenarios，并跑完整校验。
