---
superseded_by: docs/brainstorms/2026-06-18-supervisor-wave-protocol-upgrade-requirements.md
date: 2026-06-17
topic: wave-dimension-assignment-enforcement
related:
  - docs/report/2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md
  - docs/brainstorms/2026-06-13-wave-dispatch-policy-gate-requirements.md
  - docs/brainstorms/2026-06-17-ce-executor-flow-reliability-requirements.md
  - presets/en/ce-executor-isolated.yml
---


# Wave Dimension Assignment Enforcement — 需求文档

## Problem Frame

Operator 用 `ce-executor-isolated` 跑多步 plan 时，review wave 的 worker 经常**不审自己被分配的维度**：要么返回错误维度，要么直接不返回。结果 review-synthesizer 收到的 dimension.done 集合不完整/不一致，触发 `plan.blocked(reason=dimension_reviewers_failed_to_converge)`，loop 以 `REVIEW_COMPLETE(fail)` 终止。

2026-06-17 keen-fern worktree 的 4 维 wave 是典型现场：

- review-coordinator 发了 correctness / testing / maintainability / requirements 四个维度。
- 实际返回 3 条 `review.dimension.done`：wave_index 0 → correctness，wave_index 1 → requirements，wave_index 3 → correctness。
- testing 与 maintainability 完全没有返回；correctness 被重复审了两次。
- R6 `incomplete_wave_gate` 在 04:06:40 触发 `plan.blocked`，但这是**症状拦截**，不是根因治愈。

现有机制只校验 `dimension` 字段存在，不校验它是否与 worker 被分配的维度一致；也不把「worker 审错维度」识别为可重试的 worker failure。agent 提示词虽然写了 "Your dimension is from the event payload's `dimension` field"，但在并发 wave 压力下不可靠。

本需求在 **wave 派发、worker 输入、merge 回写** 三个卡点加硬绑定，让「审错维度」和「缺维度」变成可观测、可恢复的运行时事件，而不是让 synthesizer 拿到残缺信号后一枪 fail。

---
---

## Actors

- A1. **review-coordinator agent**：根据 diff 内容选择维度，emit `review.wave.ready` wave，每个 payload 含 `dimension` 字段。
- A2. **loop runner / wave dispatcher**：把 wave 的每个 payload 绑定到一个 worker slot，决定 spawn 数、超时、merge 策略。
- A3. **dimension-reviewer worker agent**：wave 中单个 worker，应按分配维度审代码并 emit `review.dimension.done`。
- A4. **review-synthesizer agent**：聚合全部维度结果，emit 终态 verdict 或 `plan.blocked`。

---
---

## Key Flows

- F1. **正常 4 维 wave（回归路径）**
  - **Trigger:** review-coordinator emit 4 个 `review.wave.ready` payload，维度互不相同。
  - **Actors:** A1, A2, A3 × 4, A4
  - **Steps:** dispatcher 绑定 slot → 4 worker → 各审各维度 → merge 校验全过 → synthesizer 收到 4 条 → 正常 verdict
  - **Outcome:** wave 关闭，无 `dimension_reviewers_failed_to_converge` 路径。
  - **Covered by:** R1, R2, R4

- F2. **Worker 审错维度（新治理路径）**
  - **Trigger:** wave_index 1 的 worker 被分配 `testing`，但 emit `review.dimension.done` 时 `dimension="correctness"`。
  - **Actors:** A2, A3
  - **Steps:** merge 时发现 mismatch → 拒绝该事件 → 写入 `wave.worker.failed` + targeted `task.resume` 到对应 worker slot → 重跑该 slot
  - **Outcome:** 错误结果不进主事件流；同一 wave 其他 3 个正确结果保留。
  - **Covered by:** R3, R4, R5

- F3. **Worker 超时/未返回（partial wave 路径）**
  - **Trigger:** wave_index 2 的 worker 在 aggregate window 内未产出任何事件。
  - **Actors:** A2, A4
  - **Steps:** dispatcher 超时后标记该 slot failed → synthesizer 拿到 3/4 结果 + `missing_dimensions=[testing]`
  - **Outcome:** synthesizer 可基于 partial 结果做 verdict 或按 preset 协议 emit `plan.blocked`，但机制层不再把它伪装成完整 wave。
  - **Covered by:** R5, R6

---
---

## Requirements

**Wave 派发绑定**

- R1. dispatcher 在构建每个 worker 时，必须从对应 `review.wave.ready` payload 中解析 `dimension` 字段，并将其与 `wave_id` + `wave_index` 一起作为该 worker 的 **assigned dimension** 持久化到本次 dispatch 上下文。
- R2. assigned dimension 必须通过两个独立通道传给 worker：
  - prompt 顶部固定格式块（例如 `## ASSIGNED DIMENSION: testing`），位于 `# Your Task` 之前；
  - 环境变量 `RALPH_WAVE_DIMENSION`，供 worker 的 `ralph emit` 预检查询。
- R3. `ralph emit review.dimension.done` 的 CLI policy check 在 `RALPH_WAVE_DIMENSION` 存在时，必须校验 payload 的 `dimension` 字段与其一致；不一致时拒绝写入并给出结构化错误（含 `expected_dimension` / `actual_dimension`）。

**Merge 回写校验**

- R4. dispatcher 在把 worker 结果 merge 回主 `events.jsonl` 前，必须比对返回事件的 `dimension` 与 R1 中绑定的 assigned dimension；不一致时**不得**把该事件写入主事件流。
- R5. 对 R4 中 rejected 的 mismatch 事件，dispatcher 必须：
  - 记录 `wave.worker.failed`（reason=`dimension_mismatch`，含 expected / actual / wave_id / wave_index）；
  - 向源 worker slot 注入 targeted `task.resume`，提示 "You were assigned `<expected>` but emitted `<actual>`；re-emit `review.dimension.done` with the correct dimension"。
- R6. 对超时/未返回的 worker slot，dispatcher 必须在 merge 时生成 synthetic `review.dimension.done` 占位或等价 `missing_dimensions` 信号，让 synthesizer 明确知道哪些维度缺失，而不是依赖 agent 自己数。

**Preset 与提示词对齐**

- R7. `presets/en/ce-executor-isolated.yml` 的 `dimension-reviewer` instructions 必须显式引用 `## ASSIGNED DIMENSION` 块和 `RALPH_WAVE_DIMENSION`，并加 HARD RULE：emit 的 `dimension` 必须与 assigned 完全一致，否则事件会被拒绝。
- R8. `review.dimension.done` 的 schema 允许保留 `dimension` 为自由字符串（以支持条件/扩展维度），但 runtime/CLI 必须执行 R3/R4 的动态一致性检查。

**可观测性与回归**

- R9. mismatch / missing dimension 事件必须写入 `recovery.jsonl`，`source` 为 `wave_dimension_guard`，`reason_code` 为 `dimension_mismatch` 或 `dimension_missing`，并携带 `wave_id` / `wave_index` / `expected_dimension`。
- R10. 新增 BDD scenario 或 replay fixture：4 维 wave 中 1 个 worker 返回错误维度，验证 loop 最终仍能收到 4 个正确维度（通过 retry）而不是直接 fail。
- R11. `cargo nextest run --workspace --exclude ralph-e2e` 通过；现有 wave 相关 scenario 不得回归。

---
---

## Acceptance Examples

- AE1. **Covers R1, R2, F1**
  - **Given:** review-coordinator emit 4 维 wave，wave_index 1 的 payload 为 `{"dimension":"testing",...}`。
  - **When:** dispatcher 构建 worker 1。
  - **Then:** worker 1 prompt 顶部含 `## ASSIGNED DIMENSION: testing`；环境变量 `RALPH_WAVE_DIMENSION=testing`。

- AE2. **Covers R3, R4, R5, F2**
  - **Given:** worker 1 的 `RALPH_WAVE_DIMENSION=testing`。
  - **When:** agent 执行 `ralph emit review.dimension.done --json '{"dimension":"correctness",...}'`。
  - **Then:** CLI 非零退出，stderr 含 `expected_dimension=testing actual_dimension=correctness`；主 `events.jsonl` 无此事件；`recovery.jsonl` 写入 `wave_dimension_guard` envelope。

- AE3. **Covers R4, R5, F2（merge 兜底）**
  - **Given:** worker 绕过 CLI 预检，直接在 per-worker events file 写了 `dimension=correctness`。
  - **When:** dispatcher merge 回主文件。
  - **Then:** 该事件被丢弃；生成 `wave.worker.failed`；注入 targeted `task.resume` 要求重跑 wave_index 1。

- AE4. **Covers R6, F3**
  - **Given:** 4 维 wave 中 wave_index 2 超时未返回。
  - **When:** aggregate deadline 到达，dispatcher 结束 wave。
  - **Then:** synthesizer 收到的 wave context 含 `missing_dimensions=["testing"]`（或等价信号），不假装 wave 完整。

---
---

## Success Criteria

- SC1：keen-fern 类 4 维 wave 在相同 agent/backend 下能完成 4 个不同维度的 review，不再出现同一维度重复、其他维度缺失。
- SC2：维度错误的事件**不进**主事件流，也不会触发 `plan.blocked(dimension_reviewers_failed_to_converge)` 误判为 wave 失败。
- SC3：单一 worker slot 的维度错误只导致该 slot 重试，不影响同 wave 其他 worker 结果。
- SC4：测试与 lint 全绿，preset 文档与示例同步更新。

---
---

## Scope Boundaries

- **本次覆盖**：wave worker 维度绑定、CLI/merge 校验、错误恢复、prompt 与 preset 对齐、回归测试。
- **本次不覆盖**：
  - 移除 wave 机制或改为串行 review；
  - 修改 review-coordinator 的维度选择策略；
  - 修复 keen-fern 报告中提到的 U1 残留（`audit-file-sizes.sh` 扩展、`test_u2_*` fixture schema）；
  - 改动 R6 `incomplete_wave_gate` 的触发阈值；
  - 引入新的 agent backend 或模型能力。

---
---

## Key Decisions

- **机制绑定优于提示词修补**：agent 在并发 wave 下不可靠，runtime 必须知道每个 slot 该审什么维度，并在写盘前/后双重校验。
- **拒绝但不 fatal**：审错维度的事件被拒绝并触发单 slot 重试，而不是让整个 wave 直接 fail。
- **不静默重写 dimension**：即使返回的 `dimension` 内容有价值，也不自动改成 expected，避免掩盖 agent 错误并把错误归因到别的维度。
- **复用现有 wave worker 失败路径**：`wave.worker.failed` 与 `task.resume` 已存在，新需求只增加 `dimension_mismatch` reason，不新增事件拓扑。

---
---

## Dependencies / Assumptions

- 假设 `review.wave.ready` payload 已包含 `dimension` 字段（preset schema 已要求）。
- 假设 dispatcher 已有 per-worker `wave_index` 与 per-worker events file 机制。
- 假设 `task.resume` 路由已支持 targeted hat（R5 hard-gate routing 已落地）。
- 假设 `ralph emit` CLI policy check 可访问 active preset 与 `RALPH_WAVE_DIMENSION`。

---
---

## Outstanding Questions

### Resolve Before Planning

（无 — 方向已确定为 B：runtime binding + validation。）

### Deferred to Planning

- **[Technical]** assigned dimension 的上下文存哪：`WorkerRequest` 新增字段、还是 dispatcher 内 HashMap、还是利用 per-worker events file 名推导？
- **[Technical]** CLI 预检如何实现 `RALPH_WAVE_DIMENSION` 校验：扩展 `policy_check.rs` 的 schema 校验，还是新增专用 guard？
- **[Technical]** merge 时 rejected mismatch 的 retry 策略：立即重跑同一 slot、还是等下一轮 loop 由 `task.resume` 驱动？是否需要重试上限？
- **[Technical]** 占位/缺失维度信号的具体形态：synthetic `review.dimension.done`（`findings_count=0` 带 `missing=true`）还是扩展 `WaveContext` 的 `missing_dimensions`？

---
---

## Next Steps

→ `/ce-plan` 生成实施计划（建议文件 `docs/plans/2026-06-17-001-feat-wave-dimension-assignment-enforcement-plan.md`）。

→ 可与 `2026-06-17-ce-executor-flow-reliability` 并行，但两者在 partial wave / degraded completion 语义上有交叉，集成测试需一起跑。
