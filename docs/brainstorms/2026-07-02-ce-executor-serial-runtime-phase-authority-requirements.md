---
date: 2026-07-02
topic: ce-executor-serial-runtime-phase-authority
status: draft
supersedes_in_spirit:
  - docs/brainstorms/2026-07-01-ce-executor-serial-fix-unit-terminal-guidance-requirements.md
related:
  - docs/brainstorms/2026-07-02-ce-executor-pipeline-preset-requirements.md
  - docs/brainstorms/2026-06-27-ralph-orchestrator-mechanism-foundation-requirements.md
  - docs/brainstorms/2026-06-16-ce-executor-loop-stability-requirements.md
  - presets/en/ce-executor-pipeline.yml
  - docs/report/
  - docs/achieved/report/
---

# ce-executor-serial 坐稳 — Runtime 阶段权威需求

## Summary

`ce-executor-serial` 半个多月来机制与报告持续增加，但金丝雀 plan 仍无法在**正规事件链**上稳定收尾。根因是阶段转换权威分裂：preset 用自然语言状态机指挥 coordinator，runtime 用另一套 gate 与多份磁盘/内存状态各自校验。本需求把**阶段转换权威收敛到 runtime 单点**：Rust 维护 workflow phase，只允许合法 topic 进入总线；preset 瘦身为 hat 角色与质量标准，不再承载 PHASE GATE / progress-steward 决策大表。验收：同一 plan（`docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`）连续 3 次 run 均走 `plan.complete → REVIEW_COMPLETE → report.done → LOOP_COMPLETE`，且不以 `plan.blocked` 或 shipper 叙事越界 pass 收尾。

---

## Problem Frame

### 谁在受影响

- **Operator**：固定命令 `ralph run -H builtin:ce-executor-serial` 跑 plan，期望一次 run 正规闭环，而非「代码绿了但事件链歪了」或 `consecutive_failures` 终止。
- **维护者**：每份 `docs/report/` 诊断催生一批 gate / HARD RULE，preset 膨胀至 3000+ 行，下一 run 在另一角落复发（`docs/achieved/report/2026-06-21-top-3-architectural-instability-factors.md` 三大因素至今同型）。

### 根因（对话 + 报告共识）

1. **双状态机**：coordinator prompt 里的 PHASE GATE 表与 runtime 的 `ReviewStepTracker` / `step_handoff` / `mechanism.flow` 语义不一致。
2. **多源状态**：`tasks.jsonl`、`progress.md`、内存 tracker 被不同 gate 读取，产生 phantom mismatch（140149 / 083222 同源）。
3. **症状驱动补丁**：近半月 commit（`step_handoff` U1–U6、`precheck`、`shipper` 白名单扩缩、`event_policy` dedup）各修单次 run 的 P0，未收口「fix-unit 或 pass_with_residuals 之后唯一合法 emit 是什么」。

### 既有机制怎么处理（删减 vs 保留）

本需求**不是**把近半月机制一锅端回滚，而是 **serial 专用减法 + 共享 runtime 保留/对齐**。处置分四类：

| 类别 | 机制（近半月 / 历史） | 处置 | 说明 |
|------|----------------------|------|------|
| **保留** | `execution_contracts`、`event_policy` schema、`topic_deny_rules`、isolated 单 emit | 不动 | 所有 preset 共用；pipeline 靠此跑通单链路 |
| **保留** | `precheck` / emit schema 硬校验、`completion_after_terminal` | 不动并统一热路径 | 6/28 报告：CLI 与 loop 必须同轨，禁止再旁路 |
| **保留** | `step_handoff` / `plan_gate` / `review_step_state` 的**校验能力** | 保留逻辑，改读 phase 快照 | 不删 gate，删与 prompt 重复的**第二套路由表** |
| **保留** | `event_policy` dedup（`review.start`、`review.dimension.ready`） | 不动 | 降噪，与 phase 不冲突 |
| **保留** | `repair_budget`、独立 repair topic 通道 | 不动（本轮不扩写） | 6/27 药方 4 已部分落地 |
| **删减** | `ce-executor-serial` 内 coordinator **PHASE GATE 大表** | **删除** | 由 runtime phase 白名单取代（R7） |
| **删减** | `progress-steward` 与 coordinator **重复的自愈决策表** | **删除或缩为 fallback 注释** | steward 只做 stall 升级，不再替 coordinator 选 review vs plan.complete |
| **删减** | preset 内与 R2 重复的 **HARD RULE 墙**（「DO NOT emit …」数十条） | **删除** | 保留少量质量标准；路由改由 runtime |
| **删减** | shipper **narrative 白名单**扩缩 ping-pong | **收敛为严格枚举**（R11） | 不再靠 prompt 模糊匹配 recoverable |
| **废止** | commit footer + 数 fix-plan 标题作**主路由**（7/1 guidance） | **不作终态权威** | 可作 execution 诊断，R4 取代 |
| **废止** | U6 PlanTopologyCache / runtime 解析 plan markdown | **不得再开** | 已回滚，写入 Scope |
| **暂缓** | `mechanism.flow` 在 serial 上的**执行层**双写 | **lint/文档保留，执行以 phase 为准** | 避免 flow + phase + prompt 三套并行 |
| **禁止全局默认** | 新建 **runtime phase authority** | **仅 opt-in preset 启用** | 见 R15；`ce-executor-pipeline` 等 hat-only 链**不得**被迫接入 |

**原则**：减的是 **serial preset YAML 里与 runtime 重复的编排表** 和 **互相打架的补丁规则**；不减 **共享引擎能力**。任何共享引擎改动必须满足下文 **非回归约束（R15–R17、SC6）**。

### 本需求不重复做的事

- 不重开 `2026-06-27` 四药方全量落地（声明式 flow、幂等 JSONL、独立 repair 通道）作为**叠加**；若与阶段权威冲突，以本需求为准**替换** prompt 侧重复部分。
- 不把「坐稳」定义为仅代码/测试绿或 fallback 收口（用户已否决 032648 / 140433 式「某种成功」）。

---

## Actors

- **A1. Runtime phase authority**：维护当前 workflow phase 与允许的 emit 集合；拒绝或改写非法 topic。
- **A2. Coordinator hat**：解析 plan、生成 `work.ready` 文案与 task 元数据；**不再**自行决定 fix-unit 结束后发 `review.start` 还是 `plan.complete`。
- **A3. Executor / Validator / Review hats**：在 phase 允许范围内执行单元工作与 review；行为约束不变。
- **A4. Shipper / Reporter**：仅在 `plan.complete` 或 runtime 定义的合法 `plan.blocked` 之后激活；不得靠 narrative 把非常规 reason 升级为 pass。
- **A5. Operator / CI**：用金丝雀 plan 连跑 3 次验证正规链。

---

## Key Flows

- **F1. 正常 plan-unit 推进**
  - **Trigger：** `work.start`
  - **Actors：** A1 → A2 → A3
  - **Steps：** phase = `unit_loop`；coordinator 仅可 emit `work.ready`；executor `work.done` + validator `test.passed` 由 runtime 推进至下一 unit 或 `review` phase。
  - **Outcome：** 最后一个 plan-unit 的 `test.passed` 自动进入 `review` phase，coordinator 可 emit `review.start`（单次）。
  - **Covered by：** R1, R2, R3

- **F2. Review 结束 → fix-unit 或 plan 终态**
  - **Trigger：** `review.complete`
  - **Actors：** A1 → A2
  - **Steps：** runtime 根据 verdict 进入 `fix_units` 或 `plan_end`；fail 且 fix_plan 存在时 phase = `fix_units`；`pass_with_residuals` 且无需 fix 时 phase = `plan_end`。
  - **Outcome：** coordinator 不能在此分支自行 emit `review.start`。
  - **Covered by：** R2, R4, R5

- **F3. 最后一个 fix-unit 完成**
  - **Trigger：** `test.passed`，step 前缀 `fix-`，且 runtime 判定 fix-unit 队列已耗尽
  - **Actors：** A1 → A2
  - **Steps：** phase = `plan_end`；**仅** `plan.complete` 可进入总线；`review.start` 拒收。
  - **Outcome：** 恰好一次 `plan.complete`，payload 含合法 `step` 与 completed_steps。
  - **Covered by：** R4, R6

- **F4. 正规收口**
  - **Trigger：** `plan.complete` 被接纳
  - **Actors：** A4 → A1
  - **Steps：** shipper → `REVIEW_COMPLETE` → reporter → `report.done` → `LOOP_COMPLETE`；runtime 在 `LOOP_COMPLETE` honored 后禁止新业务事件与 stall_recovery 再激活。
  - **Outcome：** 进程退出，loops 状态写入 ended。
  - **Covered by：** R7, R8

---

## Requirements

**阶段模型与权威（仅 opt-in preset）**

- R1. Runtime phase authority **仅对显式启用的 preset 生效**（首轮：`builtin:ce-executor-serial`）。未启用的 preset（含 `builtin:ce-executor-pipeline`）**不得**进入 serial 的 phase 枚举或白名单校验。
- R1b. 启用后，runtime 必须维护单一 workflow phase 枚举，至少覆盖：`unit_loop`、`review`、`fix_units`、`plan_end`、`ship`、`terminal`。
- R2. 每个 phase 必须声明允许的 business topic 白名单；不在白名单的 emit 在进总线前拒收，并给源 hat 可操作的拒绝原因（非仅 `task.resume` 循环）。**仅**在 R1 启用的 preset 上执行。
- R3. Phase 转换必须由 runtime 根据**已接纳事件**推导，不得依赖 coordinator prompt 表格或 agent 自述「当前阶段」。

**终态边（历史最高频炸点）**

- R4. 在 `fix_units` phase 内，最后一个 fix-unit 的 `test.passed` 之后，runtime 必须转入 `plan_end`，且下一业务事件只允许 `plan.complete`（`review.start` 必须拒收）。
- R5. 在 `review.complete` 为 `pass_with_residuals` 且 fix_plan 为空（无需 fix-unit）时，runtime 必须转入 `plan_end`，coordinator 必须能 emit `plan.complete` 且不得被 `progress_missing_current_step` 类 gate 阻断。
- R6. `plan.complete` 的 schema 必填字段与 runtime phase 判断必须一致；`step` 在 fix-unit 终态不得缺失。

**Preset 减法**

- R7. 必须从 `presets/en/ce-executor-serial.yml` 删除或降级为「说明性注释」、不再作为行为依据的内容：coordinator PHASE GATE 决策大表、progress-steward 与 coordinator 语义重复的自愈决策表、与 R2 白名单重复的「DO NOT emit」HARD RULE 墙。
- R8. Coordinator instructions 保留：plan 解析、task 文案、`work.ready` payload 质量、与 executor/validator 的交接说明；**删除**「数 fix-plan 标题决定 total_units」「自行选择 Branch A/B」类路由逻辑。

**状态单源**

- R9. 所有 gate 对「当前 step / fix-unit 进度 / review 是否终态」的判定必须读取 runtime phase 与 projector 提交的**同一内存快照**，禁止 gate 绕过 projector 直读磁盘各读一份。
- R10. `progress.md` 的 `Current Step` 必须由 runtime 在 phase 转换时写入，不得留空导致 `plan.complete` 被 step_handoff 误拒（140149 复发点）。

**收口与非法成功**

- R11. Shipper 对 `plan.blocked` 的 recoverable reason 必须机器可解析的严格枚举；非常规 reason（如 `stall_no_events recovery` 子串匹配）不得升级为 pass。
- R12. `LOOP_COMPLETE` honored 后，progress-steward 不得再注入 `task.resume` 或 `work.ready`；loop 进程必须在有界时间内退出（SC-2）。

**可观测与回归**

- R13. 必须提供金丝雀 BDD 或 replay 场景，覆盖 F3、F4 两条终态边，且断言 events 序列含正规链四事件各恰好一次。
- R14. 诊断报告生成逻辑不得把「plan.complete 进 repair sink」自动等同于「未进总线」而不查 events（140433 v2/v3 教训）。

**非回归：hat-only 单链路 preset（硬约束）**

- R15. **禁止**为 serial 坐稳而引入**默认全局** phase gate、FlowStepScope 强制、或 coordinator 专用假设，导致未 opt-in 的 preset 事件被拒。`presets/en/ce-executor-pipeline.yml` 无 `mechanism.flow`、走 hat-only 管线（`StagePipeline::with_hat_only_stages_for_loop_config`）的行为 **必须保持不变**。
- R16. 任一 PR 若改动共享 `event_loop` / `stage_pipeline` / `event_policy` / `step_handoff`，**合并前**必须通过：`preset_lint`（含 `ce-executor-pipeline`）、`ce-executor-pipeline` 相关 BDD / `run_workflow_guard_scenario`（若已有）、以及 `./scripts/run-tests.sh` 全绿。任一失败 **禁止合并**。
- R17. `ce-executor-pipeline` 的线性主链事件拓扑不得被破坏：`plan.ready → work.done → review.*.done（6 维串行）→ review.complete → fix.done → align.done → report.done → LOOP_COMPLETE`；不得要求该 preset 引入 coordinator、`plan.complete`、shipper、或 `mechanism.flow` 块才能跑通。
- R18. 对共享引擎的「增强」须满足：**serial opt-in 行为新增，hat-only 默认路径零行为变更**（除非显式修复独立 bug 且附带 R16 回归证明）。

---

## Success Criteria

- SC1. **正规链 × 3**：金丝雀 plan 连续 3 次 run，每次 events 均含且仅含一次 `plan.complete`、`REVIEW_COMPLETE`、`report.done`、`LOOP_COMPLETE`（honored），且顺序合法。
- SC2. **无非法成功**：3 次 run 均不得主要依赖 `plan.blocked` + shipper 越界 pass 收尾；不得出现 `LOOP_COMPLETE` 后进程常驻（lock 永占）。
- SC3. **Preset 瘦身**：`presets/en/ce-executor-serial.yml` 行数相对当前基线减少 ≥ 25%，且删除 R7 所列决策表后 preset_lint 与 SSOT byte-equality 仍通过。
- SC4. **复发簇封口**：同一金丝雀 plan 的 3 次 run 中，`plan_gate_review_not_terminal` 与 `progress_missing_current_step` 对 `plan.complete` 的拒收次数均为 0。
- SC5. **测试基线**：`./scripts/run-tests.sh` 全绿；新增场景走 `run_workflow_guard_scenario`。
- SC6. **Pipeline 非回归（阻断项）**：`builtin:ce-executor-pipeline` 在改动前后均能完成 preset_lint + 其 workflow guard / BDD 场景（与 `presets/schemas/ce-executor-pipeline.yml` 一致）；**不允许**出现「单链路 preset 跑不了」的回归。
- SC7. **共享引擎零默认破坏**：未声明 phase authority 的 preset，其 `plan.ready` / `review.*.done` / `fix.done` 等 emit **接纳率**相对改动前基线不下降（以 CI replay/BDD 为准）。

---

## Scope Boundaries

### 本次覆盖

- `builtin:ce-executor-serial` 的 runtime phase authority（**opt-in**）与 preset 减法。
- 既有机制处置（上表）：serial YAML 删减 + 共享 runtime 保留对齐。
- 金丝雀 plan 正规链验收与 BDD。
- 与 phase 冲突的 step_handoff / plan_gate 规则对齐（**仅 serial**）。
- **`builtin:ce-executor-pipeline` 非回归验收（SC6）** — 不迁移其拓扑，但必须保证改动后仍可跑通。

### 本次不覆盖

- 其他 builtin preset（`autoresearch`、`merge-loop` 等）的 phase 迁移 — 设计可复用，本轮不强制落地。
- **`ce-executor-pipeline` 的 phase 化或机制.flow 引入** — 该 preset 刻意 hat-only 线性链，见 R17。
- 后端模型 / adapter 行为。
- TUI / Web Dashboard。
- 新增 repair topic 或幂等 JSONL 全量迁移（`2026-06-27` 药方 2/4 整体重做）。
- 用 commit message footer（`2026-07-01` fix-unit guidance）作为**主要**终态判断手段 — 可作为辅助诊断，不得替代 R4 runtime 转换。

### 明确废弃的方向

- 继续向 preset 叠加 HARD RULE 与 progress-steward 分桶行以「修最后一次 run 的 P0」。
- Shipper 白名单靠 narrative「recoverable reason」模糊匹配。
- Base runtime 解析 plan/fix-plan markdown 正文做拓扑缓存（U6 PlanTopologyCache 已回滚，不得再开）。

---

## Key Decisions

| 决策 | 理由 |
|------|------|
| 验收 = 正规事件链 × 3，非仅交付绿 | 用户明确；与 032648/140433 分歧划界 |
| Runtime 阶段机接管转换，非 coordinator 自觉 | 6/21 三大因素 + 半月报告同一根因 |
| 先 serial preset，不先做全 preset 底座 | 范围可控；坐稳后再抽象到 `mechanism.flow` 的执行层 |
| 减法 preset 与加 runtime 同期 | 否则双状态机仍在；R7 与 R1–R2 同一 PR 系列 |
| 保留 10-hat 拓扑与 review 6 维 | 报告多次确认主路径编排合理，炸点在终态边 |
| Phase authority **opt-in**，非全局默认 | 用户硬性要求：不能搞挂 `ce-executor-pipeline` 等单链路 preset |
| 共享机制保留、serial prompt 删减 | 近半月 commit 不整体回滚；去重的是 prompt 侧第二套状态机 |

---

## Dependencies / Assumptions

- 金丝雀 plan 固定为 `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`（2 unit + 可变 fix-unit），直至 SC1 通过再换 plan 泛化。
- isolated mode、单 turn 单 business event 约束保持。
- `mechanism.flow` metadata 在 **serial** 可保留为 lint/文档；热路径以 opt-in phase 为准。 **Pipeline 无 `mechanism.flow`**，继续 hat-only 路由。
- 假设：agent 仍可能 emit 非法 topic；在 serial 上 runtime 拒收 + 短路径纠正即可，不要求 agent 100% 合规才跑通。
- **硬假设（用户确认）**：任何增强或删减 **不得** 导致 `ce-executor-pipeline` 单链路跑不通；CI 以 SC6 阻断。

---

## Outstanding Questions

### Deferred to Planning

- Phase 枚举是否暴露给 `ralph diagnose` / TUI（可观测性）。
- 非法 emit 拒收后走 deterministic prompt 纠正还是单次 `task.resume`（需避免 6/21 因素 1 自循环）。
- `mechanism.phase_authority`（或等价）配置字段的 YAML 形状与 preset_lint 规则。
- Preset 瘦身是否分两个 PR（先 runtime 接线再删表）以降低回归风险。

### Resolved（本轮对话）

- `ce-executor-pipeline` **不复用** serial phase 引擎；**必须**保持 hat-only 非回归（R15–R17、SC6）。
- 近半月机制 **选择性删减**（见「既有机制怎么处理」表），非整体回滚。

---

## Relationship to Prior Docs

| 文档 | 关系 |
|------|------|
| `docs/brainstorms/2026-07-01-ce-executor-serial-fix-unit-terminal-guidance-requirements.md` | **精神废止**：commit footer + tasks.jsonl 数 fix-unit 不能作为终态权威；R4 取代 |
| `docs/brainstorms/2026-06-27-ralph-orchestrator-mechanism-foundation-requirements.md` | 保留硬契约/声明式 flow **思想**；本需求用 phase authority **实现** flow 的执行语义，不另加第四层 prompt 表 |
| `docs/brainstorms/2026-06-16-ce-executor-loop-stability-requirements.md` | SSOT schema 与 payload 恢复仍有效；与 phase gate 互补 |
| `docs/brainstorms/2026-07-02-ce-executor-pipeline-preset-requirements.md` | 单链路 preset 拓扑 SSOT；本需求 **不得破坏** 其事件链（R17） |

---

## Acceptance Examples

- AE1. **Covers R4, F3.** 给定 fix-01、fix-02 均已 `test.passed`，runtime phase 为 `plan_end`；coordinator emit `review.start` → 拒收；coordinator emit `plan.complete` → 接纳。
- AE2. **Covers R5, F2.** 给定 `review.complete(verdict=pass_with_residuals, fix_plan_file=null)`，`progress.md` 无 current fix step；coordinator emit `plan.complete` → 接纳，不被 `progress_missing_current_step` 拒。
- AE3. **Covers SC1.** 金丝雀 plan 单次 run 的 events 序列中，`plan.complete`、`REVIEW_COMPLETE`、`report.done`、`LOOP_COMPLETE` 各出现 1 次且顺序递增。
- AE4. **Covers R11.** 给定 `plan.blocked(reason=stall_no_events recovery…)`，shipper emit `REVIEW_COMPLETE(pass)` → 拒收或 hard-fail，不得升级为 pass。
- AE5. **Covers R15–R17, SC6.** 给定 `builtin:ce-executor-pipeline` 配置（无 `mechanism.flow`），executor emit `work.done`、dim hat emit `review.goalalign.done` 等 —— 与改动前相同，**不被** serial phase 白名单拒收。
- AE6. **Covers 机制处置表「删减」行。** 给定 `ce-executor-serial` preset 删除 PHASE GATE 表后，runtime phase 在 `fix_units` 终态拒收 `review.start`、接纳 `plan.complete`；**不**要求恢复已删的 prompt 表才能跑通。
