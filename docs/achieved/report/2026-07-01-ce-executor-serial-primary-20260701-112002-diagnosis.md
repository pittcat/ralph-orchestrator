# RALPH 链路诊断报告 — primary-20260701-112002 (v2 · review 阶段已正常闭环)

> **run**: `primary-20260701-112002`
> **preset**: `ce-executor-serial`(isolated mode, 10-hat)
> **plan**: `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`(2 plan-unit)
> **run_dir**: `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/`
> **loop 状态**: `2026-07-01 11:20:02` 启动 → 仍存活 → **review 阶段已完整跑完 6 维并到达 `review.complete(pass_with_residuals)`**
> **诊断日期**: 2026-07-01
> **报告版本**: v2（勘误 v1 的“review 挂死”误判）

---

## 第 0 部分:结论摘要（v2 更新）

**整体健康度**: 🟡 **unit 阶段 100% 闭环,review 阶段 6/6 维闭环,但 review 启动段有明显噪音。**

- 2 个 plan-unit 全部跑完（16/16 pytest 绿、commit 落盘）。
- review 阶段 **完整走完全部 6 个维度**：`goal-alignment → correctness → testing → maintainability → project-standards → adversarial`。
- `review.dimensions.complete` 已 emit（event #26），`review-synthesizer` 已 emit `review.complete`（event #27），verdict 为 `pass_with_residuals`。
- **问题不是“review 挂死”，而是 review 启动阶段 review-coordinator 重复 emit `review.dimension.ready(goal-alignment)` 4 次，并触发 coordinator 错误地第二次 emit `review.start`**。

**关键异常数量**:
- **P0**: 0 个（6 维 review 已闭环，无挂死）
- **P1**: 2 个（review 启动段 `review.dimension.ready` 重复 emit；coordinator 第二次 emit `review.start` 违反单次契约）
- **P2**: 1 个（事件 #12 为 `intent_summary="Test emit"` 的探测 emit，污染事件流）

**是否涉及历史重复问题**:
- **否**。v1 报告将本 run 与 `perky-maple`、`warm-tiger`、`merry-lotus` 关联为“review 挂死复发”，但事实是本 run 的 review 主路径已正常走完。启动段噪音与历史 dedup 风暴形态相似，但结果不同。

**主因判定更新**:
- **不是编排问题**（preset 的 6 维 walk 回路正常工作，最终走完）。
- **是机制层噪音**：`review.dimension.ready` 的 dedup 在跨 activation 场景下失效，导致 review-coordinator 重复发同一维度；`review.start` 被 coordinator 发了第二次，说明触发链路存在污染。

---

## 第 1 部分:实测事件流(27 行 events.jsonl)

| # | 时刻 (UTC+8) | topic | hat | triggered | 关键 payload | 状态 |
|---|------|-------|-----|-----------|---|---|
| 1 | 11:20:02 | `work.start` | loop-bootstrap | - | PROMPT 引用 plan | ✅ |
| 2 | 11:21:04 | `work.ready` (step-01) | coordinator | - | task_id=task-1782904861-f6de | ✅ |
| 3 | 11:23:07 | `work.done` (step-01) | executor | - | commit_count=1, changed_lines=254 | ✅ |
| 4 | 11:24:02 | `test.passed` (step-01) | validator | - | 6/6 | ✅ |
| 5 | 11:25:11 | `work.ready` (step-02) | coordinator | - | task_id=task-1782905106-9cfb | ✅ |
| 6 | 11:27:26 | `work.done` (step-02) | executor | - | commit_count=1, changed_lines=166 | ✅ |
| 7 | 11:28:03 | `test.passed` (step-02) | validator | - | 10/10 | ✅ |
| 8 | 11:28:34 | `review.start` | coordinator | - | total_units=2, unit_index=2 | ✅ 首次 |
| 9 | 11:29:32 | `review.dimension.ready` | review-coordinator | 无 | dimension=goal-alignment | ✅ 首次 |
| 10 | 11:29:55 | `review.dimension.ready` | review-coordinator | 无 | 同 #9 同 payload | ❌ 重复 |
| 11 | 11:30:04 | `review.dimension.ready` | review-coordinator | 无 | 同 #9 同 payload | ❌ 重复 |
| 12 | 11:30:12 | `review.dimension.ready` | review-coordinator | 无 | `intent_summary="Test emit"`, `changed_files=[]` | ❌ 探测 emit 污染 |
| 13 | 11:31:31 | `review.start` | coordinator | `review-coordinator` | 同 #8 payload | ❌ 违反单次契约 |
| 14 | 11:32:32 | `review.dimension.ready` | review-coordinator | `dimension-reviewer` | dimension=goal-alignment, intent 改英文 | ⚠️ triggered 异常 |
| 15 | 11:33:35 | `review.dimension.done` | dimension-reviewer | `ralph` | goal-alignment, 0 findings | ✅ |
| 16 | 11:34:21 | `review.dimension.ready` | review-coordinator | `ralph` | dimension=correctness | ✅ |
| 17 | 11:36:20 | `review.dimension.done` | dimension-reviewer | `ralph` | correctness, 0 findings | ✅ |
| 18 | 11:37:21 | `review.dimension.ready` | review-coordinator | `ralph` | dimension=testing | ✅ |
| 19 | 11:38:49 | `review.dimension.done` | dimension-reviewer | `ralph` | testing, 0 findings | ✅ |
| 20 | 11:39:30 | `review.dimension.ready` | review-coordinator | `ralph` | dimension=maintainability | ✅ |
| 21 | 11:41:31 | `review.dimension.done` | dimension-reviewer | `ralph` | maintainability, 0 findings | ✅ |
| 22 | 11:42:35 | `review.dimension.ready` | review-coordinator | `ralph` | dimension=project-standards | ✅ |
| 23 | 11:43:51 | `review.dimension.done` | dimension-reviewer | `ralph` | project-standards, 0 findings | ✅ |
| 24 | 11:44:41 | `review.dimension.ready` | review-coordinator | `ralph` | dimension=adversarial | ✅ |
| 25 | 11:47:00 | `review.dimension.done` | dimension-reviewer | `ralph` | adversarial, 1 P2 finding | ✅ |
| 26 | 11:47:37 | `review.dimensions.complete` | review-coordinator | `ralph` | 6 维全 done | ✅ |
| 27 | 11:49:20 | `review.complete` | review-synthesizer | `ralph` | verdict=pass_with_residuals, fix_plan_file=null | ✅ |

### 1.1 关键结论

- **review 主路径已完整闭环**：event #8 到 #27，6 维全部 review 完成，最终 `review.complete`  verdict=`pass_with_residuals`。
- **噪音集中在启动段**：#9-#12 是同一维度的重复/探测 emit，#13 是第二次 `review.start`。
- **噪音之后主路径恢复**：从 #14 开始，review-coordinator 按正确顺序推进 6 维，无重复。

### 1.2 磁盘状态验证

- `review-sequence.json` 6 维全 `done` ✅
- `findings-*.json` 6 个文件全部存在 ✅
- `findings-adversarial-task-1782905106-9cfb.json` 包含 1 个 P2 advisory（集成测试缺少 float 覆盖）和 1 个 P3 residual risk ✅

---

## 第 2 部分:v1 误判原因分析

v1 报告写于 loop 运行中途（约 iter 11 时），当时事件流只到 `review.dimension.done(goal-alignment)`（event #15），后续 5 维尚未发生。因此 v1 报告基于不完整快照得出“review 挂死在 1/6”的结论。

实际运行后续证明：
- review-coordinator 在噪音之后恢复了正常 walk；
- 6 维全部完成；
- `review.dimensions.complete` 和 `review.complete` 均正常 emit。

**教训**：诊断报告必须等待 loop 进入终态或至少确认无后续事件后再下结论，否则会把“运行中噪音”误判为“挂死”。

---

## 第 3 部分:仍然存在的真实问题（噪音）

虽然 review 已闭环，但启动段噪音是真实缺陷，需要修复：

### P1-1: `review.dimension.ready(goal-alignment)` 重复 emit

- **证据**：events #9、#10、#11、#12 在 43 秒内（11:29:32 → 11:30:12）发了 4 次同一维度。
- **根因**：`review.dimension.ready` 的 dedup 只在**同一 batch 内**生效（`event_policy.rs:1070-1076` 明确说明 in-batch set 会被 drain），跨 activation 不保留。review-coordinator 在多个 turn 里看不到之前的 emit 是否成功，于是重复发。
- **修复方向**：让 dedup 在跨 activation 时仍然有效，或让 review-coordinator 在 emit 前读取 `review-sequence.json` 校验当前维度状态。

### P1-2: coordinator 第二次 emit `review.start`

- **证据**：event #13 由 coordinator 发出，`triggered="review-coordinator"`，payload 与 #8 相同。
- **违反契约**：preset line 992-999 与 schema 约定 `review.start` 应由 coordinator 在 `test.passed` 后单次 emit。
- **根因**：review-coordinator 的下游行为反推了 coordinator，触发 review.start 重发。这是事件路由/触发链路的机制污染。
- **修复方向**：禁止 coordinator 被 review-coordinator 触发 emit `review.start`；明确 `review.start` 的唯一触发源是 `test.passed`（最后一个 unit 完成时）。

### P2-1: `ralph emit` 探测污染事件流

- **证据**：event #12 `intent_summary="Test emit"`、`changed_files=[]`。
- **根因**：agent 用 `ralph emit` 做探测，未隔离到 dry-run 模式，落盘污染事件流。
- **修复方向**：探测 emit 必须走 `--dry-run` 或不落盘的 policy-check 模式。

---

## 第 4 部分:编排 vs 机制归因更新

| 问题 | 原 v1 归因 | v2 更正归因 | 理由 |
|---|---|---|---|
| 6 维 review 是否挂死 | 编排缺回路（P0） | **无此问题** | review 已走完 6 维 |
| review 启动段重复 ready | 基座 dedup 放大器（P0-2） | **机制问题（P1）** | dedup 跨 activation drain，无法阻止重复 |
| coordinator 第二次 review.start | 编排 contract violation（P0） | **机制问题（P1）** | 触发链路被 review-coordinator 反推 |
| 探测 emit 污染 | 基座 emit 隔离缺失（P2） | **机制/工具问题（P2）** | `ralph emit` 未提供可靠的 dry-run |

**关键结论更新**：
- **编排（preset）本身没有问题**：`review-coordinator` 的 instructions、triggers、publishes、obligations 设计完整，最终成功驱动 6 维 walk。
- **机制层需要加固**：dedup 跨 activation 失效、触发链路污染、探测 emit 未隔离。

---

## 第 5 部分:修复建议（按优先级）

### P1-1: 让 `review.dimension.ready` dedup 跨 activation 生效

**目标文件**：`crates/ralph-core/src/event_policy.rs:1044-1076`、`crates/ralph-core/src/event_policy.rs:455-477`

**方案 A（推荐，机制层）**：
- 将 `review_dimension_ready_seen_keys` 的 in-batch 记录升级为**per-loop lifetime set**，不要每次 batch 后 drain。
- 同时保留 `fix.applied` 时的 prune 逻辑（`prune_review_dimension_ready_bucket`），确保新 fix round 可以重发。

**方案 B（preset 层 self-guard）**：
- 在 `review-coordinator` instructions 的 "Walk the sequence" 段加 Pre-emission Hard Gate：
  1. emit 前读取 `review-sequence.json`；
  2. 若目标维度 status ∈ {done, in_progress}，则不发 `review.dimension.ready`，改为继续找下一个 pending 维度；
  3. 若无 pending 维度，发 `review.dimensions.complete`。

**建议**：A 和 B 都做。A 是机制层兜底，B 是让 agent 自觉不重复。

### P1-2: 禁止 coordinator 被 review-coordinator 触发 emit `review.start`

**目标文件**：`presets/en/ce-executor-serial.yml:638-646`、`crates/ralph-core/src/event_loop/mod.rs`（触发路由）

**修复方向**：
- preset 中明确 `coordinator.triggers` 不含 `review.dimension.ready` / `review.dimensions.complete` / `review.complete`；
- 机制层加 `topic_deny_rules` 或触发源校验，确保 `review.start` 只能由 `test.passed`（最后一个 unit）触发。

### P2-1: `ralph emit` 探测隔离

**目标文件**：`crates/ralph-cli/src/commands/emit.rs` 或相关 CLI

**修复方向**：
- 增加 `--dry-run` / `--policy-check` 模式，只输出校验结果，不落盘；
- 或提供 `ralph emit --probe` 专门用于探测，不写 `events.jsonl`。

---

## 第 6 部分:验证基线

```bash
# preset_lint + SSOT byte-equality
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded

# 新增/更新 BDD scenarios
cargo nextest run -p ralph-core --test scenarios -- \
  review_dimension_ready_cross_activation_dedup \
  review_start_single_emission_guard

# 全 workspace 基线
./scripts/run-tests.sh
```

---

## 第 7 部分:约束性总结（v2）

本次 run 的 **review 阶段没有挂死，6 维已正常闭环**。v1 报告基于不完整快照误判为“review 挂死在 1/6”。

真实问题是 **review 启动段的机制噪音**：
1. `review.dimension.ready` dedup 跨 activation 失效，导致同一维度重复 emit；
2. coordinator 被异常触发第二次 emit `review.start`；
3. `ralph emit` 探测未隔离，污染事件流。

这些问题**不是编排缺回路**，而是机制层需要加固。修复优先级：P1-1 > P1-2 > P2-1。

---

**报告完结** · v2 · 2026-07-01
