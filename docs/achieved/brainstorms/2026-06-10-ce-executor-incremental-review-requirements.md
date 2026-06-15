---
date: 2026-06-10
topic: ce-executor-incremental-review
---

# ce-executor 增量 Review 改造

## Summary

让 `presets/en/ce-executor.yml` 的 code review 不再以 loop 启动那一刻的 SHA 为 diff 底；改为「上一次评过到现在的增量」，work.done 和 fix.applied 两条路径都用。fix.applied 进一步把维度集从 7 砍到 3（correctness + standards + requirements），work.done 仍走 7+ 维度全量。

---

## Problem Frame

当前 preset 的 `review-coordinator` 用 coordinator 在 work.start 一次性记的 `start_sha` 作为所有 wave 的 diff 底（参见 `presets/en/ce-executor.yml` line 519-531 现有 diff_base 推导逻辑）。这意味着一个多 step 计划跑完，step 2/3/4/5 的 reviewer 都要把前面所有 step 已经评过的代码再读一遍；fix.applied 复评时还要把原 work 再读一遍。

一个具体场景：executor 提交 step 1（包含若干 U-ID）→ work.done wave（reviewer 看整 step 全量）→ 第一次 review 失败 → fixer 修 3 个 P2 → fix.applied wave（按当前行为，reviewer 仍然看整 step + 3 行 fix）。第二次 review 的 reviewer 看到的 diff 跟第一次几乎一样，只是多了 3 行 fix 改动。

这违反了「增量 review」的直觉——既然上轮已经评过这批代码，这轮就该只看新增。Token 实际花在「重复读已评过代码」上的比例远高于花在「真正新内容」上的比例。

---

## Key Decisions

- **两条触发路径都切到增量基线**。work.done 和 fix.applied 各自只评「上次评过 → 现在」的增量；只在 loop 启动后第一次 wave 没有「上次」可读时才降级到 `start_sha` 全量。
- **fix.applied 阶段维度集从 7 砍到 3，且 conditionals 不跑**。修一轮只需要验证「修对了没」，testing / maintainability / agent-native / learnings 这 4 个首评已过，fix 阶段再跑属于重复投入；5 个 conditionals（security / performance / api-contract / reliability / adversarial）在 fix 阶段同样不跑。**接受 fix 触碰 auth / data layer 的 regression 风险，由 work.done 阶段首评覆盖**。
- **work.done 维持 7+ 维度，不按 diff 大小分档**。即使本 step diff 很小，work.done 仍是首评走全量；后续如要按大小分档再单独讨论。

---

## Requirements

### Diff base computation

- R1. work.done 触发的 `review-coordinator` 在推导 `diff_base` 时必须读「上次评过 SHA」（`last_reviewed_sha`）；找不到时降级到 `start_sha`，并在产物中标注 `diff_base_fallback: "no_prior_wave"`。**注**：找不到 `last_reviewed_sha` 在系统正常流里是异常——见 R3 / R6 的硬落点约束——但作为兜底保护仍然走全量。
- R2. fix.applied 触发的 `review-coordinator` 同样必须读 `last_reviewed_sha` 作为 `diff_base`；找不到时按 R1 同样的降级策略，但标注 `diff_base_fallback: "no_last_reviewed_sha"` 区分于首次 wave 的标记。
- R3. `last_reviewed_sha` 必须在每次 `review.wave.ready` 发出后立即更新到持久层（与该 plan 关联），保证下一次 wave 启动时能读到。

### Dimension set per trigger

- R4. fix.applied wave 的 `dimension` 集合必须**只**包含 `correctness` + `standards` + `requirements` 三项。必需 7 维里其他 4 个（testing / maintainability / agent-native / learnings）在 fix 阶段不跑；5 个 conditionals（security / performance / api-contract / reliability / adversarial）在 fix 阶段同样不跑（**与 work.done 阶段的 conditionals 触发逻辑无关**——fix 阶段无条件地只看 3 维）。
- R5. work.done wave 的 `dimension` 集合维持原 7 个必需 + 条件维度，**这次不做按 diff 大小分档**。当 fix 阶段后续要落地时，R4 的「3 维」是它的小集合基线。

### State persistence

- R6. `last_reviewed_sha` 必须能跨 wave 读到——同一 plan 内的 wave 序列里，下一次 wave 启动时能看到上一次 wave 落点的 SHA。
- R7. `last_reviewed_sha` 的持久层必须正确处理同一 plan 多个 step 推进的边界——plan-gate 的 `queue.advance` 不应清空上一 step 的 `last_reviewed_sha`，否则下一个 step 的 work.done 会拿不到基线。

---

## Acceptance Examples

- AE1. **首次 wave 降级到 start_sha**
  - **Given:** loop 刚启动，coordinator 记录了 `start_sha = A`；持久层里没有 `last_reviewed_sha`。
  - **When:** step 1 executor 提交完，触发 `work.done`。
  - **Then:** `review-coordinator` 找不到 `last_reviewed_sha`，降级使用 `start_sha = A`；wave 发出的 `diff_base` 字段为 `A`；产物中标注 `diff_base_fallback: "no_prior_wave"`；维度集合为 7+ 维度全量。
- AE2. **第二次 work.done 用上一轮 SHA**
  - **Given:** step 1 的 `last_reviewed_sha` 已落点为 `B`（B 是 step 1 末尾的 commit）；plan-gate 已发 `queue.advance`；step 2 executor 提交完，触发 `work.done`。
  - **When:** `review-coordinator` 启动。
  - **Then:** 读到 `last_reviewed_sha = B`，`diff_base = B`；维度集合仍为 7+ 维度全量。
- AE3. **fix.applied 用增量 + 3 维**
  - **Given:** step 1 首次 review 已结束，`last_reviewed_sha = B`；fixer 提交一轮 fix，HEAD 走到 `B'`。
  - **When:** `fix.applied` wave 触发。
  - **Then:** `diff_base = B`，`diff_base_fallback` 字段不存在（或显式为空）；`dimension` 集合为 `correctness` + `standards` + `requirements`。
- AE4. **fix 多轮 SHA 串联**
  - **Given:** step 1 已完成 round 1 fix，`last_reviewed_sha = B1`（fix round 1 末）；fixer 提交 round 2 fix，HEAD 走到 `B2`。
  - **When:** `fix.applied` wave 触发（round 2）。
  - **Then:** `diff_base = B1`（不是 step 1 末的 `B`），`dimension` 集合仍为 3 维。
- AE5. **plan-gate 不清空 last_reviewed_sha**
  - **Given:** step 1 末 `last_reviewed_sha = B`；plan-gate 已发 `queue.advance`。
  - **When:** step 2 executor 提交完，触发 `work.done`。
  - **Then:** 读到的 `last_reviewed_sha` 仍是 `B`，不是空也不是被覆盖为 step 1 起点。
- AE6. **loop 重启后找不到 last_reviewed_sha（异常兜底）**
  - **Given:** loop 重启后第一次 fix.applied 触发；持久层里 `last_reviewed_sha` 缺失（异常状态）。
  - **When:** `review-coordinator` 启动。
  - **Then:** 走降级路径：`diff_base = start_sha`，产物标注 `diff_base_fallback: "no_last_reviewed_sha"`；维度集合与该触发路径默认一致（fix.applied 为 3 维）。

---

## Scope Boundaries

### Deferred for later

- **极小 fix（< 5 行）跳过 wave 直接 `review.passed`**：条件细化收益有限，先把「增量 + 减维度」跑稳再说。
- **work.done 阶段按 diff 大小分档**：step 2/3/4 即使本 step diff 很小，work.done 仍走 7+ 维度全量。这次不做。
- **per-commit 边界改造**：executor 每 commit 都触发小 review，结构性变动大；这次不做。
- **给非 critical 维度换小模型（Haiku）**：和这个改动正交，留作单独讨论。

### Outside this product's identity

- **对其他 preset（code-assist / debug / merge-loop 等）做同样改造**：本需求只针对 ce-executor；其他 preset 走各自的 review 流程。
- **改造 review-synthesizer 的合路逻辑**：fix.applied 走 3 维产出的 findings 文件格式不变，synthesizer 继续按现有逻辑合并。

---

## Dependencies / Assumptions

- 假设 coordinator 已经在 `context.md` 记 `start_sha`（现状已成立）。
- 假设当前 7 个必需维度（correctness / testing / maintainability / standards / requirements / agent-native / learnings）的必跑语义不变。
- 假设 `event_policy.schemas.review.wave.ready` 当前只要求 `dimension, focus, depth, diff_base, intent_summary, changed_files, plan_name, task_id, task_key, step` 字段；不要求新增 `dimension_set` 显式标记「这是 fix 用的 3 维」——可通过「trigger 是 fix.applied」隐含表达。
- 假设 `last_reviewed_sha` 的更新是 `review-coordinator` 自身的职责（不需要新设一个 hat）。这与现有「review-coordinator 是 diff 分析的单一 owner」语义一致。
- 假设 `event_policy` 不会因为 `diff_base` 取不同值而拒绝——schema 不关心 diff_base 是不是 `start_sha`。

---

## Outstanding Questions

### Resolve Before Planning

- **Q1. ✅ 已解决 — conditionals（security / performance / api-contract / reliability / adversarial）在 fix.applied 阶段不跑。** 决策：fix 阶段无条件只看 3 维（correctness + standards + requirements），与 work.done 阶段的 conditionals 触发逻辑解耦。已知代价：fix 触碰 auth / data layer 的回归可能在 fix 阶段漏掉，由 work.done 首评承担覆盖。已写入 R4。
- **Q2. ✅ 已解决 — 找不到 `last_reviewed_sha` 时降级到 `start_sha` 全量评。** 决策：`last_reviewed_sha` 在系统正常流里**必须**能被读到（这是 R3 / R6 的硬约束）；万一找不到是异常状态，按 R1 同样的降级路径走 `start_sha` 全量评，并标注 `diff_base_fallback: "no_last_reviewed_sha"` 区分于首次 wave 的 `diff_base_fallback: "no_prior_wave"`。不放过任何 review。已写入 R1 / R2。

### Deferred to Planning

- **Q3. `last_reviewed_sha` 的持久层选型**：`context.md` 加字段 vs 新建 `review-state.md` vs 从 `events.jsonl` 反推。三个方案在 worktree 隔离、loop 重启、跨 worktree merge 场景下的可靠性不同；由 ce-plan 阶段根据具体 storage 实现评估。

---

## Sources / Research

- `presets/en/ce-executor.yml` line 276 — coordinator 在 work.start 一次性记 `start_sha` 的当前实现位置。
- `presets/en/ce-executor.yml` line 519-531 — 现有 `review-coordinator` 的 diff_base 推导逻辑（要改造的核心）。
- `presets/en/ce-executor.yml` line 537-541 — HARD RULE「wave vs pass 决策」——本次改造不改变这条规则，wave 仍然必发。
- `presets/en/ce-executor.yml` line 482-506 — `review-coordinator` 的 `obligations`（fix.applied 触发的 wave 必发语义）—— 本次改造复用这套 obligation 机制。
- `presets/en/ce-executor.yml` line 631-632 — `dimension-reviewer.concurrency: 9` —— 减维度后实际并发数会从 7-9 降到 3。
- `presets/en/ce-executor.yml` line 596-606 — 单一 wave emit HARD RULE（所有选中维度必须一次 emit）—— 本次改造保留这条 HARD RULE 不变。
- `presets/en/ce-executor.yml` line 98-172 — 现有 `event_policy.schemas` —— 验证 `review.wave.ready` 不需要 schema 改动。
- 现有持久化模式参考：`fix-log.md`（fix 轮次跟踪）、`context.md`（plan 级状态）、`progress.md`（step 跟踪）—— `last_reviewed_sha` 的新位置可借鉴这套模式。
- 现有 4 篇 brainstorm 文档（`docs/brainstorms/2026-06-08-*` / `2026-06-09-*`）—— 验证 ce-executor 周边需求文档的写法惯例。
