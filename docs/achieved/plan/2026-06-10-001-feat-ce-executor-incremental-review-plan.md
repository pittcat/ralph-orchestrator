---
title: "feat: ce-executor 增量 Review 改造"
type: feat
status: active
date: 2026-06-10
origin: docs/brainstorms/2026-06-10-ce-executor-incremental-review-requirements.md
---

# feat: ce-executor 增量 Review 改造

## Summary

改造 `presets/en/ce-executor.yml` 的 `review-coordinator`，将 diff base 从「loop 启动时的 `start_sha` 全量」切换为「上次 review 落点的 `last_reviewed_sha` 增量」；fix.applied 触发路径的维度集从 7+ 砍到 3（correctness + standards + requirements）；首次 wave 无 `last_reviewed_sha` 时降级到 `start_sha` 全量。同步改造 `ce-executor-wave.yml` 和 `ce-executor-zh.yml`。

---

## Problem Frame

当前 `review-coordinator` 用 coordinator 在 `work.start` 一次性记的 `start_sha` 作为所有 wave 的 diff 底。多 step 计划中，step 2/3/4/5 的 reviewer 都要把前面所有 step 已评过的代码再读一遍；fix.applied 复评时还要把原 work 再读一遍。Token 实际花在「重复读已评过代码」上的比例远高于花在「真正新内容」上的比例。（see origin: `docs/brainstorms/2026-06-10-ce-executor-incremental-review-requirements.md`）

---

## Requirements

- R1. work.done 触发的 `review-coordinator` 必须读 `last_reviewed_sha` 作为 `diff_base`；找不到时降级到 `start_sha`，产物标注 `diff_base_fallback: "no_prior_wave"`
- R2. fix.applied 触发的 `review-coordinator` 同样必须读 `last_reviewed_sha`；找不到时降级到 `start_sha`，标注 `diff_base_fallback: "no_last_reviewed_sha"`
- R3. `last_reviewed_sha` 必须在每次 `review.wave.ready` 或 `review.passed` 发出后立即更新到持久层（`review.passed` 路径同样需要更新——空 diff 跳过 review 时，当前 HEAD 仍是「已评过」的边界，否则下一次 wave 的增量基线会错误地指向更早的 SHA）
- R4. fix.applied wave 的维度集只包含 `correctness` + `standards` + `requirements`；5 个 conditionals 在 fix 阶段不跑
- R5. work.done wave 维持 7+ 维度全量，不做按 diff 大小分档
- R6. `last_reviewed_sha` 必须能跨 wave 读取——同一 plan 内下一次 wave 启动时能看到上一次 wave 落点的 SHA
- R7. plan-gate 的 `queue.advance` 不应清空上一 step 的 `last_reviewed_sha`

**Origin acceptance examples:** AE1（首次 wave 降级到 start_sha）, AE2（第二次 work.done 用上一轮 SHA）, AE3（fix.applied 用增量 + 3 维）, AE4（fix 多轮 SHA 串联）, AE5（plan-gate 不清空 last_reviewed_sha）, AE6（loop 重启后找不到 last_reviewed_sha 异常兜底）

---

## Scope Boundaries

### Deferred for later

- 极小 fix（< 5 行）跳过 wave 直接 `review.passed`
- work.done 阶段按 diff 大小分档
- per-commit 边界改造
- 给非 critical 维度换小模型（Haiku）

### Outside this product's identity

- 对其他 preset（code-assist / debug / merge-loop 等）做同样改造
- 改造 review-synthesizer 的合路逻辑

### Deferred to Follow-Up Work

- `fix-log.md` 的跨 step 污染问题（`current_fix_round` 在 step 推进时不重置）

---

## Context & Research

### Relevant Code and Patterns

- `presets/en/ce-executor.yml:276-277` — coordinator 在 `work.start` 记录 `start_sha` 到 `context.md`
- `presets/en/ce-executor.yml:519-531` — 当前 `review-coordinator` 的 diff_base 推导逻辑（改造核心）
- `presets/en/ce-executor.yml:534-549` — HARD RULE：wave vs pass 决策（不改）
- `presets/en/ce-executor.yml:482-506` — `obligations` 的 `conditional_must_emit`（不改）
- `presets/en/ce-executor.yml:562-591` — 维度选择逻辑（fix.applied 需改）
- `presets/en/ce-executor.yml:594-606` — 单一 wave emit HARD RULE（不改）
- `presets/en/ce-executor.yml:98-172` — `event_policy.schemas`（`review.wave.ready` schema 不需改——`diff_base_fallback` 是可选字段，不在 `required_fields` 中）
- `presets/en/ce-executor.yml:631-632` — `dimension-reviewer.concurrency: 9`（fix.applied 3 维时实际并发降为 3，无需改配置）
- `context.md` 的 `start_sha` key-value header 模式 — `last_reviewed_sha` 可复用同模式
- `fix-log.md` 的 `current_fix_round` key-value header — 参考 key-value header + structured body 的持久化模式

### Institutional Learnings

- `docs/solutions/integration-issues/ce-executor-wave-emission-must-batch-in-single-emit-2026-06-09.md` — wave 必须一次 emit 所有维度；增量改造不能破坏此 HARD RULE
- `docs/solutions/developer-experience/ce-executor-plan-gate-premature-completion-2026-06-02.md` — plan-gate 的 `queue.advance` 不清任何状态文件，`last_reviewed_sha` 天然跨 step 保留
- `docs/report/2026-06-08-ce-executor-review-wave-not-firing-diagnosis.md` — review-coordinator 曾短路过 review.passed；增量改造增加了 coordinator 决策复杂度，必须保留 HARD RULE + `conditional_must_emit` 保护

### External References

- 无需外部研究——代码库已有充分的本地模式参考

---

## Key Technical Decisions

- **`last_reviewed_sha` 存储在 `context.md`**：复用现有 key-value header 模式（与 `start_sha` 同文件），最小侵入。不新建 `review-state.md`——增加一个文件管理点但职责更清晰，不过当前 context.md 已承载多种状态（`start_sha`、`mode`、`complexity`），再加一个 key 不增加认知负担。`events.jsonl` 反推不可靠（缺少 git SHA 信息、有竞态问题）。
- **纯 preset YAML 指令层改造，不涉及 Rust 代码变更**：`runtime_contract.rs` 只检查 topic 名称不检查维度；`event_policy.rs` 只验证 `required_fields` 存在性不限制额外字段；`hat.rs` 的 `conditional_must_emit` 不涉及维度；`dispatcher.rs` 不关心 payload 语义；`ralph-proto` 的 `Event.payload` 是 `String` 类型。
- **`diff_base_fallback` 作为可选字段加入 wave payload**：不在 `event_policy.schemas.review.wave.ready.required_fields` 中声明——schema 不限制额外字段，可选字段天然兼容。降级时 agent 在 payload 中附带此字段供下游诊断。
- **`last_reviewed_sha` 的更新时机**：`review.wave.ready` 或 `review.passed` 发出后立即更新（R3）。具体实现为：review-coordinator 在 wave emit 或 review.passed 发射成功后执行 `git rev-parse HEAD` 并写入 `context.md` 的 `last_reviewed_sha` 字段。
- **ce-executor-wave.yml 和 ce-executor-zh.yml 同步改造**：wave 变体有完全对称的 review-coordinator 结构，不同步改造会导致行为不一致。zh 变体是 en 变体的中文翻译，必须同步。

---

## Open Questions

### Resolved During Planning

- **Q3（origin）`last_reviewed_sha` 持久层选型**：决定存储在 `context.md`，复用 key-value header 模式。理由：最小侵入、与 `start_sha` 同文件便于 agent 读取、不需要新文件管理、plan-gate 天然不清除此文件。
- **是否需要 Rust 代码变更**：不需要。所有变更在 preset YAML 指令层。

### Deferred to Implementation

- **`last_reviewed_sha` 写入的精确格式**：key-value header 格式（`last_reviewed_sha: <sha>`）还是 markdown 段落格式——由实施者参照 `start_sha` 和 `current_fix_round` 的现有写法确定
- **wave emit 后更新 `last_reviewed_sha` 的具体 shell 命令序列**：依赖实施时 review-coordinator instructions 的具体写法

---

## Implementation Units

### U1. 在 context.md 添加 `last_reviewed_sha` 持久层

**Goal:** 为 `last_reviewed_sha` 建立持久化存储，使 review-coordinator 能跨 wave 读取上次 review 落点的 SHA。

**Requirements:** R3, R6, R7

**Dependencies:** None

**Files:**
- Modify: `presets/en/ce-executor.yml` — review-coordinator 的 instructions（wave emit 和 review.passed 后更新 `last_reviewed_sha`）
- Modify: `presets/en/ce-executor-wave.yml` — 同上
- Modify: `presets/zh/ce-executor-zh.yml` — 同上

**Approach:**
- 在 coordinator 的 `work.start` instructions 中，`start_sha` 记录之后，不需要显式初始化 `last_reviewed_sha`——它的「不存在」就是首次 wave 的降级信号
- 在 review-coordinator 的 instructions 中，wave emit 成功后增加一步：`git rev-parse HEAD` → 写入 `context.md` 的 `last_reviewed_sha: <sha>` 字段
- 同样，在 `review.passed` 发射后也执行 `git rev-parse HEAD` → 写入 `context.md` 的 `last_reviewed_sha: <sha>`（R3 扩展：空 diff 跳过 review 时，当前 HEAD 仍是「已评过」的边界）
- 写入格式参照 `start_sha` 的 key-value header 模式
- 如果 `context.md` 中已有 `last_reviewed_sha`，覆写（不是追加）——保证始终是最近一次 review 的 SHA

**Patterns to follow:**
- `start_sha` 的 key-value header 写入模式（`presets/en/ce-executor.yml:276-277`）
- `current_fix_round` 的覆写模式（`fix-log.md` 的文件值优先于 payload 值）

**Test scenarios:**
- Happy path: review-coordinator 发出 `review.wave.ready` 后，`context.md` 中出现 `last_reviewed_sha: <当前 HEAD SHA>`
- Edge case: 连续两次 wave emit，`last_reviewed_sha` 被第二次覆写为最新 SHA
- Edge case: `context.md` 中已有旧的 `last_reviewed_sha`，新值正确覆写
- Integration: plan-gate 发出 `queue.advance` 后，`context.md` 中的 `last_reviewed_sha` 仍保留（不被清除）

**Verification:**
- 手动构造 ce-executor 场景：loop 启动 → step 1 work.done → review wave → 检查 `context.md` 中 `last_reviewed_sha` 存在且为正确 SHA
- plan-gate `queue.advance` 后检查 `last_reviewed_sha` 未被清除

---

### U2. 改造 review-coordinator 的 diff_base 推导逻辑

**Goal:** 将 review-coordinator 的 diff_base 从始终使用 `start_sha` 改为优先使用 `last_reviewed_sha`，找不到时降级到 `start_sha`。

**Requirements:** R1, R2, R6

**Dependencies:** U1

**Files:**
- Modify: `presets/en/ce-executor.yml` — review-coordinator 的 Scope Detection 段（line 519-531）
- Modify: `presets/en/ce-executor-wave.yml` — review-coordinator 的 Scope Detection 段（line 650-660；注意：wave 变体的 Scope Detection 比 en 变体简单，缺少 commit_count/changed_lines 读取命令，增量改造时只需替换 diff_base 推导优先链，不需要添加 en 变体中不存在的 git 命令）
- Modify: `presets/zh/ce-executor-zh.yml` — review-coordinator 的 Scope Detection 段

**Approach:**
- Scope Detection 段改造为三级优先链：
  1. 读 `last_reviewed_sha` from `context.md` → 如果存在且有效，用作 `diff_base`
  2. 读 `start_sha` from `context.md` → 如果存在且有效，用作 `diff_base`（降级路径）
  3. 回退到 base detection chain（`origin/main` → … → `HEAD~1`）
- 降级时在 wave payload 中附带 `diff_base_fallback` 字段（agent 通过检查触发事件的 topic 字段区分 work.done 和 fix.applied）：
  - 从 `start_sha` 降级时：`diff_base_fallback: "no_prior_wave"`（work.done 首次 wave，无 prior review）
  - 从 `start_sha` 降级时：`diff_base_fallback: "no_last_reviewed_sha"`（fix.applied 异常状态，按 R2 区分于首次 wave）
  - 从 base detection chain 降级时：`diff_base_fallback: "no_start_sha"`（极端异常）
- `diff_base_fallback` 不加入 `event_policy.schemas.review.wave.ready.required_fields`——它是可选诊断字段
- 保留现有的 `git diff`/`git log`/`git rev-list`/`git diff --shortstat` 命令结构，只替换 base 参数

**Technical design:**

> *This illustrates the intended approach and is directional guidance for review, not implementation specification.*

```
diff_base 推导优先链：
1. last_reviewed_sha (from context.md)
   → diff_base = last_reviewed_sha
   → diff_base_fallback = 不设置

2. start_sha (from context.md) — 降级
   → diff_base = start_sha
   → diff_base_fallback = "no_prior_wave" (work.done 首次)
                        或 "no_last_reviewed_sha" (fix.applied 异常)

3. base detection chain — 最终兜底
   → diff_base = merge-base result
   → diff_base_fallback = "no_start_sha"
```

**Patterns to follow:**
- 现有 Scope Detection 的三级 fallback 模式（`start_sha` → base detection chain → `HEAD~1`）

**Test scenarios:**
- Covers AE1. 首次 wave 降级到 start_sha：`context.md` 无 `last_reviewed_sha`，有 `start_sha = A` → `diff_base = A`，payload 含 `diff_base_fallback: "no_prior_wave"`
- Covers AE2. 第二次 work.done 用上一轮 SHA：`context.md` 有 `last_reviewed_sha = B` → `diff_base = B`，无 `diff_base_fallback`
- Covers AE4. fix 多轮 SHA 串联：round 1 fix 后 `last_reviewed_sha = B1`，round 2 fix 触发 → `diff_base = B1`
- Covers AE6. loop 重启后找不到 last_reviewed_sha（fix.applied 触发）：`context.md` 无 `last_reviewed_sha`，有 `start_sha` → 降级到 `start_sha`，`diff_base_fallback: "no_last_reviewed_sha"`（若为 work.done 触发则用 `"no_prior_wave"`）
- Edge case: `last_reviewed_sha` 存在但指向已不存在（rebase/amend 后）的 commit → git diff 失败 → agent 应识别错误并降级到 `start_sha`

**Verification:**
- 手动构造多 step 场景：step 1 review 完成 → step 2 work.done → 检查 diff_base 是 step 1 末尾 SHA 而非 loop 启动 SHA
- 手动构造 fix 场景：首次 review → fix.applied → 检查 diff_base 是首次 review 末尾 SHA

---

### U3. fix.applied 触发路径维度集从 7+ 砍到 3

**Goal:** fix.applied 触发的 review-coordinator 只选 3 个维度（correctness + standards + requirements），不跑 conditionals。

**Requirements:** R4

**Dependencies:** U2

**Files:**
- Modify: `presets/en/ce-executor.yml` — review-coordinator 的 Dimension Selection 段（line 562-591）
- Modify: `presets/en/ce-executor-wave.yml` — review-coordinator 的 Dimension Selection 段（line 673-701）
- Modify: `presets/zh/ce-executor-zh.yml` — review-coordinator 的 Dimension Selection 段

**Approach:**
- 在 Dimension Selection 段增加触发路径分支（agent 通过检查触发事件的 topic 字段区分 work.done 和 fix.applied）：
  - **work.done 触发**：维持 7 个 always-on + 5 个 conditional（现有逻辑不变，满足 R5）
  - **fix.applied 触发**：只选 `correctness` + `standards` + `requirements`；不评估、不选择 5 个 conditional dimensions
- 在 instructions 中明确说明：fix 阶段的 3 维是**无条件**的——即使 fix 触碰 auth / data layer，也不跑 security / reliability 等 conditional。已知代价由 work.done 首评覆盖（see origin Key Decisions）
- wave emit 仍遵循单一 emit HARD RULE——3 个维度一次 emit
- `dimension-reviewer.concurrency: 9` 不改——3 维时实际并发为 3，无需调整配置

**Patterns to follow:**
- 现有 Dimension Selection 的 always-on / conditional 分层模式
- 单一 wave emit HARD RULE（`presets/en/ce-executor.yml:594-606`）

**Test scenarios:**
- Covers AE3. fix.applied 用增量 + 3 维：fix.applied 触发 → dimension 集合为 `correctness` + `standards` + `requirements`，无 conditional
- Happy path: work.done 触发 → dimension 集合仍为 7+ conditional（不受影响）
- Edge case: fix 触碰 auth 相关代码 → 仍不选 `security` 维度（3 维无条件）
- Integration: 3 维 wave emit 仍是一次 `ralph wave emit` 调用（3 个 payloads），不是 3 次独立 emit

**Verification:**
- 手动构造 fix 场景：work.done review 失败 → fixer 修复 → fix.applied → 检查 wave payload 只有 3 个 dimension
- 检查 wave emit 是一次调用（`wave_total=3`），不是 3 次独立调用

---

### U4. review-coordinator instructions 整合与 HARD RULE 保护

**Goal:** 将 U1-U3 的改造整合到 review-coordinator 的完整 instructions 中，确保 HARD RULE 和 obligations 不被削弱。

**Requirements:** R1, R2, R3, R4, R5, R6, R7

**Dependencies:** U1, U2, U3

**Files:**
- Modify: `presets/en/ce-executor.yml` — review-coordinator 完整 instructions
- Modify: `presets/en/ce-executor-wave.yml` — review-coordinator 完整 instructions
- Modify: `presets/zh/ce-executor-zh.yml` — review-coordinator 完整 instructions

**Approach:**
- 整合改造后的 Scope Detection（U2）和 Dimension Selection（U3）到完整 instructions
- 在 Scope Detection 段末尾增加 `last_reviewed_sha` 更新步骤（U1）
- 验证以下 HARD RULE 不被破坏：
  - Wave vs Pass Decision（line 534-549）：纯 diff 状态判断，不受 diff_base 来源影响
  - 单一 wave emit（line 594-606）：3 维也是一次 emit
  - `conditional_must_emit` obligations（line 482-506）：topic 名称不变，维度数不影响
- 在 Scope Detection 段增加降级路径的审计字段说明：`diff_base_fallback` 的含义和取值
- 确保 `review.passed` 事件也更新 `last_reviewed_sha`（空 diff 跳过 review 时，当前 HEAD 仍是「已评过」的边界）。插入位置：Wave vs Pass Decision 段的 `review.passed` 发射指令之后，与 wave emit 路径的更新步骤对称

**Patterns to follow:**
- 现有 HARD RULE 的不可违反语义
- `conditional_must_emit` 的双保险机制（instructions + runtime contract）

**Test scenarios:**
- Integration: 完整 flow — loop 启动 → step 1 work.done → 首次 review（降级到 start_sha）→ step 2 work.done → 增量 review（用 last_reviewed_sha）→ fix.applied → 3 维增量 review
- Integration: review.passed 场景（空 diff）→ `last_reviewed_sha` 仍被更新
- Integration: 多 step + 多轮 fix 的 SHA 串联正确性
- Covers AE5. plan-gate 不清空 last_reviewed_sha：step 1 完成 → queue.advance → step 2 work.done → 读到 step 1 末尾的 `last_reviewed_sha`

**Verification:**
- 端到端手动测试：启动 ce-executor loop，执行 2 step 计划，验证 step 2 的 diff_base 是 step 1 末尾 SHA
- 端到端手动测试：首次 review 失败 → fix → fix.applied → 验证维度只有 3 个且 diff_base 是首次 review 末尾 SHA

---

## System-Wide Impact

- **Interaction graph:** review-coordinator 的触发/发布 topic 不变（`work.done`/`fix.applied` → `review.wave.ready`/`review.passed`）。下游 hat（dimension-reviewer、review-synthesizer、plan-gate）不受影响——它们不读 `diff_base` 值也不关心维度数量。
- **Error propagation:** `last_reviewed_sha` 缺失时降级到 `start_sha` 全量评——不放过任何 review。降级通过 `diff_base_fallback` 可观测。
- **State lifecycle risks:** `context.md` 是 agent 管理的文件，不是 orchestrator runtime 管理的。如果 agent 在 wave emit 后崩溃（未写 `last_reviewed_sha`），下一次 wave 会降级到 `start_sha` 全量——安全但冗余。部分写入风险极低但非零（agent 是唯一写入者、文件小、key-value 覆写是单次写入操作）。
- **API surface parity:** ce-executor-wave 和 ce-executor-zh 必须同步改造，否则行为不一致。
- **Integration coverage:** `review.passed` 也必须更新 `last_reviewed_sha`——否则空 diff 跳过 review 后，下一次 wave 的增量基线会错误地指向更早的 SHA。
- **Unchanged invariants:** `conditional_must_emit` obligations 不变；wave vs pass HARD RULE 不变；单一 wave emit HARD RULE 不变；`event_policy.schemas` 不变；Rust 运行时不变。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| agent 在 wave emit 后崩溃，未写 `last_reviewed_sha` | 降级到 `start_sha` 全量评，安全但冗余；`diff_base_fallback` 字段可观测 |
| `last_reviewed_sha` 指向已不存在的 commit（rebase/amend 后） | instructions 中指导 agent 识别 git diff 错误并降级到 `start_sha` |
| fix 阶段 3 维漏掉 auth/data layer regression | 已知代价，由 work.done 首评覆盖（see origin Key Decisions） |
| ce-executor-wave 未同步改造导致行为不一致 | 本计划将 wave 变体纳入同步改造范围 |
| review-coordinator 决策复杂度增加导致短路 | HARD RULE + `conditional_must_emit` 双保险不变；降级路径始终走向更多 review 而非更少 |

---

## Documentation / Operational Notes

- 改造完成后，`context.md` 会新增 `last_reviewed_sha` 字段——这是 agent 管理的状态，不需要文档更新
- `diff_base_fallback` 是可选的 wave payload 字段，不需要 schema 改动
- 如果后续需要观测降级频率，可通过 `events.jsonl` 中 `review.wave.ready` 事件的 `diff_base_fallback` 字段统计

---

## Sources & References

- **Origin document:** [docs/brainstorms/2026-06-10-ce-executor-incremental-review-requirements.md](docs/brainstorms/2026-06-10-ce-executor-incremental-review-requirements.md)
- Related code: `presets/en/ce-executor.yml` (review-coordinator), `presets/en/ce-executor-wave.yml` (wave variant), `presets/zh/ce-executor-zh.yml` (zh variant)
- Related learnings: `docs/solutions/integration-issues/ce-executor-wave-emission-must-batch-in-single-emit-2026-06-09.md`, `docs/solutions/developer-experience/ce-executor-plan-gate-premature-completion-2026-06-02.md`
- Related diagnosis: `docs/report/2026-06-08-ce-executor-review-wave-not-firing-diagnosis.md`
