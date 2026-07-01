---
date: 2026-07-01
topic: ce-executor-serial-fix-unit-terminal-guidance
---

# ce-executor-serial：不解析 plan markdown，引导 coordinator 正确发出最后一个 fix-unit 的 plan.complete

## Problem Frame

`ce-executor-serial` preset 在 isolated 模式下，最后一个 fix-unit 完成后，coordinator 需要从 `work.ready(fix-NN)` 切换到 `plan.complete`。之前的 U6 尝试让 base runtime 扫描 `plan.md` / `fix-plan.md` 的 `### U{N}.` 标题来缓存 plan 拓扑，从而算出 `expected_event` 注入 prompt。该方案因破坏历史 plan 格式而被回滚。

回滚后，coordinator 仍依赖 preset 指令去「数 fix-plan 里的 `### U{N}.` 标题」判断 total_fix_units，这与回滚 Lessons Learned 冲突（base runtime 不应解析业务 markdown，语义理解应交还给 LLM）。同时，isolated 模式每轮只能有一个业务事件进入总线，若 coordinator 先误发了一个 stray `work.ready`，真正的 `plan.complete` 会被静默丢弃，循环降级为 `plan.blocked`。

本需求重新设计：保留并强化已有的机制兜底（U1 终态事件预算优先、U3 `CoordinatorDecisionGateStage` topic 改写），通过**commit message 元数据 + tasks.jsonl 对账**给 coordinator 一个清晰、不依赖 markdown 解析的终态判断信号。

---

## Actors

- A1. **coordinator hat (LLM agent)**：在 `test.passed(fix-NN)` 后决定下一步发 `work.ready(fix-{NN+1})` 还是 `plan.complete`。需要读取任务状态与 commit 元数据做判断。
- A2. **executor hat (LLM agent)**：执行 fix-unit 工作并创建 commit。负责在 commit message footer 中附加 `[fix-unit: fix-NN]` 标记。
- A3. **base runtime (Rust)**：校验 commit 元数据是否匹配当前 step，缺失时发出诊断事件；保留 U1/U3 作为乱发兜底；不解析 plan/fix-plan 正文。

---

## Key Flows

- F1. **最后一个 fix-unit 完成后的正常终态**
  - **Trigger：** `test.passed(fix-02)` 被事件总线接纳，且 fix-02 是 fix-unit 链的最后一个。
  - **Actors：** A2 → A3 → A1
  - **Steps：**
    1. executor 在 fix-02 的工作 commit 中加入 `[fix-unit: fix-02]` footer。
    2. runtime 在 `work.done` / `test.passed` 的 execution contract 校验中扫描最近 commit，发现 footer 与当前 `step=fix-02` 匹配，通过；若不匹配，发出 `execution_contract.fix_unit_tag_missing` 诊断。
    3. coordinator 被唤醒后读取 `tasks.jsonl` 中的 fix-unit 任务总数与已完成数，结合 commit footer 确认 fix-02 是最后一个。
    4. coordinator 发射 `plan.complete`；U3 把任何 stray `work.ready(last_in_phase=true)` 改写为 `plan.complete`；U1 保证终端事件优先获得 isolated 预算槽位。
  - **Outcome：** 恰好一个 `plan.complete` 进入总线，后续进入 ship/reporter 阶段。
  - **Covered by：** R1, R2, R3, R5, R6

- F2. **coordinator 误判还有下一个 fix-unit**
  - **Trigger：** coordinator 在最后一个 fix-unit 后仍发射 `work.ready(fix-03)`。
  - **Actors：** A1 → A3
  - **Steps：**
    1. runtime 检查 `tasks.jsonl` 不存在 `fix-03` 任务（或该 step 不在预填的 fix-unit 集合中）。
    2. `event_policy` / emit gate 以 `invalid_step_target` 或类似 reason 拒绝该事件，并附带 `task.resume` 提示当前已是最后一个 fix-unit。
  - **Outcome：** 错误 emit 不占用 isolated 预算槽位，coordinator 重新尝试后发出 `plan.complete`。
  - **Covered by：** R4, R5

---

## Requirements

**Commit 元数据约定**

- R1. executor 必须在每个 fix-unit 工作流的最终 commit message footer 中加入 `[fix-unit: fix-NN]` 标记，其中 `NN` 与当前 `step` 字段中的 `fix-NN` 完全一致。
- R2. 该标记应放在 commit message 末尾的 footer 区域，不影响标题主体和 Conventional Commits 结构；允许同一 fix-unit 有多个 commit，但至少有一个 commit 包含该标记。
- R3. runtime 通过 execution contract / git evidence provider 在 `work.done` 或 `test.passed` 校验时扫描自 `last_reviewed_sha`（或本 step baseline）以来的 commit messages；若找不到与当前 `step` 匹配的 `[fix-unit: fix-NN]` footer，发出诊断事件，但不 hard-block 事件（避免误伤历史 plan 或人工 commit）。

**Coordinator 终态判断**

- R4. coordinator 在 `test.passed(fix-NN)` 后，**禁止**再通过数 fix-plan 的 `### U{N}.` 标题来判断 total_fix_units；改为读取 `tasks.jsonl` 中所有 `fix-*` 任务的数量与状态。
- R5. coordinator 判断「是否为最后一个 fix-unit」的规则：若 `tasks.jsonl` 中 `fix-*` 任务的总数等于已标记为完成（或已有 commit footer / `test.passed`）的 fix-unit 数量，且当前 `fix-NN` 是其中编号最大的一个，则必须发射 `plan.complete`；否则发射 `work.ready(fix-{NN+1})`。
- R6. coordinator 的 `plan.complete` payload 必须携带当前 `step`（`fix-NN` 或对象形式 `{"id":"fix-NN","last_in_phase":true}`），以便 U3 的 `CoordinatorDecisionGateStage` 识别并放行。

**机制兜底（保留并验证）**

- R7. isolated 模式业务事件预算必须保持「终端事件优先」策略（U1）：同一 activation 中若同时存在非终态业务事件与终态业务事件，终态事件获得唯一槽位。
- R8. `CoordinatorDecisionGateStage` 必须继续将 `work.ready` payload 中 `step.fix-NN.last_in_phase=true` 的事件改写为 `plan.complete`，并补齐 `plan.complete` 所需字段（U3）。
- R9. `LOOP_COMPLETE` 被 honor 后，跨 activation 的持久化 completion guard 必须继续拦截任何后续业务事件（U2，已在 2026-07-01-001 计划中定义，本需求只要求不破坏）。

**Preset 与文档**

- R10. `presets/en/ce-executor-serial.yml` 中 coordinator 的 fix-unit 推进指令必须删除「Count every `### U{N}.` heading」步骤，替换为「读取 `tasks.jsonl` 的 fix-unit 任务列表 + 读取最近 commit message 的 `[fix-unit: fix-NN]` footer」。
- R11. `crates/ralph-core/data/ralph-tools*.md` 中若涉及 fix-unit 工作流或 commit 约定，需同步更新。

---

## Acceptance Examples

- AE1. **Covers R1, R2, R3.** 给定 executor 正在执行 `step=fix-02`，当它创建 commit `fix(auth): handle edge case [fix-unit: fix-02]` 后，`test.passed(fix-02)` 的 execution contract 校验通过，不产生诊断事件。
- AE2. **Covers R3.** 给定 executor 执行 `step=fix-02` 但 commit message 只有 `fix(auth): handle edge case`，`test.passed(fix-02)` 仍被接纳，但 ledger 中写入 `execution_contract.fix_unit_tag_missing` 诊断，提示 coordinator 核对当前 step。
- AE3. **Covers R4, R5, R6.** 给定 `tasks.jsonl` 包含 `fix-01`、`fix-02` 两个任务，且 `git log` 显示两者都有 `[fix-unit: fix-NN]` footer，`test.passed(fix-02)` 后 coordinator 发射 `plan.complete` 且 payload 携带 `step=fix-02`。
- AE4. **Covers R7, R8.** 给定 coordinator 同一 activation 中先发射 `work.ready(fix-02, last_in_phase=true)` 又发射 `plan.complete(fix-02)`，isolated 预算保证 `plan.complete` 进入总线；U3 也会把前者改写为 `plan.complete`，最终 ledger 中只有一笔 `plan.complete`。
- AE5. **Covers R5 拒绝路径。** 给定 `tasks.jsonl` 只有 `fix-01`、`fix-02`，coordinator 发射 `work.ready(fix-03)`，runtime 以 `invalid_step_target` 拒绝并附带 `task.resume`。

---

## Success Criteria

- `ce-executor-serial` 跑完最后一个 fix-unit 后稳定进入 `plan.complete`，不再出现 `plan.blocked(reason=progress_md_validation_stale)` 或重复的 `LOOP_COMPLETE`。
- 使用 `### UNIT 1:`、`## Step 1`、`### 1. 项目骨架` 等非 `### U{N}.` 格式的历史 plan 仍能正常走完 review/fix/ship 流程。
- coordinator prompt 不再包含「数 plan/fix-plan 标题」的指令。
- commit message 中缺失 `[fix-unit: fix-NN]` 时，ledger 中出现可观测诊断，但流程不因此 hard-fail。
- `./scripts/run-tests.sh` 全量通过，包括新增/更新的 BDD 场景与 preset_lint。

---

## Scope Boundaries

- 在范围内：commit footer 约定、runtime 软校验与诊断、coordinator prompt 改写、U1/U3 机制保持与验证、`tasks.jsonl` 读取逻辑、相关 BDD 场景与 preset_lint。
- 不在范围内：重新引入任何 base runtime 解析 plan/fix-plan markdown 正文的逻辑；修改 `review_step_state::prefill_fix_steps_from_plan` 的现有行为（它仍按 fix-plan 标题预填 tracker，但不作为 coordinator 的判断来源）。
- 不在范围内：把 fix-unit tag 变成 hard gate（execution contract 拒绝事件）。
- 不在范围内：修改 commit message 标题格式或引入新的 Conventional Commits 类型。

---

## Key Decisions

- **Commit footer 而非 plan 解析：** 用 `[fix-unit: fix-NN]` 这种工作产物（commit）上的元数据替代 base runtime 扫描 plan markdown，既保留人类可读性，又让 coordinator 与 runtime 有独立、可审计的信号源。
- **Runtime 只校验不阻塞：** 缺失 tag 时发诊断但不拒绝事件，避免对非 ce-executor-serial 场景或人工干预造成误伤。
- **Coordinator 以 tasks.jsonl 为 total 权威：** `tasks.jsonl` 在 `review.complete(fix_plan_file)` 时已预填所有 fix-unit 任务，总数可靠；commit footer 提供完成进度交叉验证，两者对照即可判断终态。
- **保留 U1/U3 作为乱发兜底：** prompt 指引是第一道防线，机制改写/预算优先级是第二、三道防线，不依赖单一路径。

---

## Dependencies / Assumptions

- 依赖：`review_step_state::prefill_fix_steps_from_plan` 在 `review.complete(fix_plan_file)` 时已经为所有 fix-unit 创建了 `StepKey` 条目（当前行为已满足）。
- 依赖：`tasks.jsonl` 中 fix-unit task 的创建与 `prefill_fix_steps_from_plan` 同步（当前行为已满足）。
- 依赖：executor hat 能遵守 preset 指令在 commit footer 中加入 tag（通过 prompt 约束，无强制 hard gate）。
- 假设：fix-unit 编号在单次 loop 内是连续且唯一的（`fix-01`, `fix-02`, …），符合 `ce-executor-serial` 的当前约定。

---

## Outstanding Questions

### Resolve Before Planning

- 无。

### Deferred to Planning

- [Needs research] execution contract 中扫描 commit messages 的最佳位置：扩展 `GitEvidenceProvider` trait 新增 `recent_commit_messages` 方法，还是在 `validate_execution_contract` 完成后单独做一次 soft check？
- [Technical] coordinator prompt 中注入 fix-unit 任务列表与 commit footer 示例的具体格式（直接读 `tasks.jsonl` 原始 JSON，还是 runtime 生成一段人类可读的 summary 注入 `context.md`？）
- [Technical] `invalid_step_target` 拒绝的具体 reason code 与 `task.resume` payload 内容。

---

## Next Steps

-> `/ce-plan` 或直接进入实现规划：在 `docs/plans/` 下生成 `2026-07-01-002-ce-executor-serial-fix-unit-terminal-guidance-plan.md`，细化 U1–U3 保持验证、commit footer 约定、runtime 诊断、coordinator prompt 改写四个实现单元。
