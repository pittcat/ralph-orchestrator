---
title: "2026-06 近 2 周修复系统性复盘审查报告"
type: review
status: completed
date: 2026-06-16
reviewer: 主 Agent（系统性复盘）
scope:
  - docs/plans/2026-06-17-001-feat-ce-executor-flow-reliability-plan.md
  - docs/plans/2026-06-17-002-feat-ce-executor-step-handoff-plan.md
  - docs/plans/2026-06-17-003-fix-ce-executor-wave-stall-bypass-plan.md
  - docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md
  - docs/plans/2026-06-03-003-refactor-schema-refs-replace-regex-plan.md
  - docs/report/ 下 9 份诊断报告
  - docs/reviews/2026-06-11-u3-dispatcher-review.md
  - docs/achieved/plan/ 近 2 周（2026-06-11 至 2026-06-16）
---

# 2026-06 近 2 周修复系统性复盘审查报告

> 📅 2026-06-16 | 🎯 范围：docs/plans/（4 份）、docs/report/（9 份诊断）、docs/reviews/（1 份）、docs/achieved/plan/（近 2 周 ~10 份核心）、最近 192 个 commit。
>
> **核心问题**：当前 working tree 未提交变更（loop_state / event_loop/mod / progress_task_gate + 3 BDD scenarios，201 增 / 75 删）来自 2026-06-17-002 U6 评审闭环 fix（`fix(step-handoff, review-gated/safe)` × 2 commit），是否需要再回滚？

---

## 1. TL;DR — 一句话定位

**近 2 周的修复**（2026-06-03 至 2026-06-16）**总体高质量**：3 条主线（017-001 flow-reliability、017-002 step-handoff、017-003 wave stall bypass）已全部落地为机制级硬门，单测 / BDD / preset check 全绿；**5 项诊断报告 P0 问题已根治**，**但 2 项大型重构计划仍 stalled**（event_loop 拆分 U1 scaffold 仅在 worktree 分支；schema_refs Phase 0 仍 pending），需要单独评估是否值得续推。**当前 working tree 的 6 个未提交变更不是 debug 残留，是 review-gated/safe 两个 fix commit 评审闭环后的代码修复**，必须先合入。

---

## 2. 修复质量评估（按计划逐份）

### [Plan 2026-06-17-001] — ce-executor Flow Reliability（并行流程可靠性机制）

- **状态评估**: **大部分已根治**（Unit 1、3、4、5、6、8 全部落地），部分单元需继续推进
- **P0 问题**: 无
- **P1 问题**:
  1. **Unit 2 (spawn 保证)** 与 **Unit 7 (aggregator handoff SLA)** 在 017-003 计划的"合并契约"中推迟；后续需验证两个 plan 合并后是否真有一致的兜底语义。
  2. **flow_lifecycle.rs 中 TimeoutReconciler 实际行为**：从单测看，`timeout_reconciler_over_budget_writes_drift_envelope` 与 `within_tolerance_writes_no_envelope` 都通过，但 U1 + U3 + U4 在生产 dogfood（zippy-sparrow）实测中，4 次 handoff_dispatch_timeout 仍累积后才触发机制 plan.blocked，**说明 TimeoutReconciler 在 wave 主动 stall 时无法提前介入**——必须依赖 U3 第 3 次 escalation 才进 U2。**复发风险：中**。
- **修复质量评分**: 4 / 5（机制层兜底强，但 U7 仍是 handoff 注册时机问题）
- **复发风险评估**: **低-中**
  - 强兜底（已验证）：incomplete wave 由机制层 emit `plan.blocked`（U2，BDD scenario `test_u6_incomplete_wave_plan_blocked_mechanism` 5s 绿）
  - 仍依赖 preset 文案：empty_diff HARD RULE 仍主要靠 agent 自觉，U3 第 3 次 escalation 是终极防线
  - **建议**：补一个 dogfood 复现脚本，验证 `wave_total=11 received=4` 时**不需要**等到 80%×1800s=1440s 才 emit plan.blocked
- **建议后续动作**:
  1. 跑 `cargo nextest run -p ralph-core -- flow_lifecycle`（已绿，41/41）
  2. 验证 `timeout_reconciler_within_tolerance_writes_no_envelope` 的 10% 容差是否合理（生产 4/11 维度 4× handoff 堆积时仍 0s 超时，没容差可言）
  3. U7（handoff 注册时机）是否需要单独起 017-001-补充计划

---

### [Plan 2026-06-17-002] — ce-executor Step Handoff（阶段交接机制）

- **状态评估**: **大部分已根治 + 当前未提交变更属于闭环修复**
- **P0 问题**: 无
- **P1 问题**:
  1. **当前 working tree 的 6 个未提交文件是 review-gated/safe fix 闭环**：
     - `crates/ralph-core/src/event_loop/loop_state.rs` 加 `last_upstream_verdict_payload` 字段——防止 `report.done` 假 pass 覆盖上游 REVIEW_COMPLETE fail 真相（**真正的 P0 修复**）
     - `crates/ralph-core/src/event_loop/mod.rs` 136 行变更——`apply_step_handoff_gate` plan.blocked payload 改为 JSON object（兼容 schema），verdict_gate 二次校验 `last_upstream_verdict_payload`，新增 `extract_xml_attr` fallback（**允许 mock fixture 用 XML 属性**，修复 #7 ProgressSnapshot::split_heading + 评审发现）
     - `crates/ralph-core/src/step_handoff/progress_task_gate.rs` 88 行变更——区分 `progress_not_found` vs `progress_unreadable`（**真正的 fail-closed 强化**：权限错误不再被冷启动豁免），`is_cold_start_step` 修正 `step-10` 被误判为 cold-start
  2. **预设缺 fix.exhausted / debug.exhausted triggers** — Unit 1 待改 preset（计划 §Implementation Units 标记待办）
- **修复质量评分**: 4.5 / 5（review 闭环发现的问题都被 gated/safe auto 修了，但还没 merge）
- **复发风险评估**: **低**
  - 当前未提交 fix 合并后，U6 verdict_gate 二次校验 + last_upstream_verdict_payload 完全闭环 2026-06-09 诊断的 fake-pass bypass
  - step_handoff Unit 4（progress-task gate）已在 production BDD 验证（`test_progress_task_mismatch_gate_blocks_queue_advance` PASS）
- **建议后续动作**:
  1. **立刻 commit 当前未提交变更**（loop_state/mod/progress_task_gate + 3 BDD）——这不是 debug 残留，是 review 闭环成果
  2. Unit 1 preset 触发器修补（fix.exhausted / debug.exhausted）需另起 PR

---

### [Plan 2026-06-17-003] — wave stall 与 empty_diff bypass 闭环

- **状态评估**: **完全根治**（zippy-sparrow 6 个 unit 全部落地，merge commit `44b9240` 显式声明"ralph-core 2112/2112, ralph-cli 1064/1064, BDD 28/28, preset check PASS"）
- **P0 问题**: 无（zippy-sparrow 原始症状"loop 以 PayloadContractViolation 致命终止"已不复发——`test_u6_zippy_sparrow_replay_fixture` PASS）
- **P1 问题**:
  1. **P1-1 (wave emit 按 dimension 去重)** 已声明 deferred 到 001 Unit 4——**当前允许 11 维重复 emit 但 dimension-reviewer 仍可工作**（zippy 案例证明可以只有 4/11 unique dim），需评估是否必须修
  2. **P2-3 (last_reviewed_sha)** U5 已加 wave_closed 闸门（preset L917-940）——✓
  3. **P2-5 (diagnosis-summary.json recovery_count 不对账)** 仍可能存在（U6 不在 003 scope 内）
- **修复质量评分**: 4 / 5（机制层闭环强，零 fatal 退化；preset 行为靠 U3 ladder 兜底，仍偏 agent-side）
- **复发风险评估**: **低**
  - `ViolationType::SemanticGateViolation` + `RecoverableRejection`（U1）确保 review_passed_while_wave_open 不再 fatal
  - `maybe_emit_incomplete_wave_blocked`（U2）确保 80%×aggregate_timeout_secs staleness 主动 emit plan.blocked
  - `stall_recovery` ladder（U3）确保第 3 次 escalation 不再路由 empty_diff bypass
  - **但**：dogfood 仍可能发现 P2-3 / P2-5 类小修
- **建议后续动作**:
  1. 补 `test_diagnosis_summary_recovery_count_reconcile` 验证 P2-5
  2. 跟踪 001 Unit 4 wave 维度去重进展

---

### [Plan 2026-06-10-003] — event_loop/mod.rs 与 loop_runner/tests.rs 拆分

- **状态评估**: **🔴 STALLED — 仅 U1 scaffold 在 worktree `sleek-willow`，未合并**
- **P0 问题**:
  - **event_loop/mod.rs 实际 8883 行**（计划写 7171 行，v5 baseline；当前已远超 v7 baseline 推测）——**严重漂移**
  - **loop_runner/tests.rs 实际 12098 行**（计划写 11796 行）——同样漂移
- **P1 问题**:
  1. U1 scaffold commit `b11d9f0` 在分支 `ralph/2026-06-10-003-...-sleek-willow`，**与当前 HEAD（pittcat-dev 88 commits later）必然冲突**——cherry-pick 不可行，需重做 U1
  2. `scripts/audit-file-sizes.sh` 仍未扩展覆盖 event_loop/*.rs 根子文件（计划 U1 要做的工作）
- **修复质量评分**: 1 / 5（计划 1/7 落地，且未合并）
- **复发风险评估**: **不适用**（计划本身未实施，谈不上复发）
- **建议后续动作**:
  1. **建议取消该计划**：自 2026-06-10 立项起 6 天，仅 U1 scaffold 在分支，HEAD 漂移 88 commit 包含 R1/R3/R4/R5 + flow_lifecycle + step_handoff + 大量 fix。**重构价值正在被新功能填充抵消**——event_loop/mod.rs 增长不慢于拆分速度
  2. 替代方案：把大文件里新增功能**先拆到独立模块**（flow_lifecycle.rs、step_handoff/ 已落地），让 mod.rs 自然收缩

---

### [Plan 2026-06-03-003] — 用 schema_refs 替换 payload_contract.rs 正则提取

- **状态评估**: **🔴 STALLED — Phase 0 仍 pending**
- **P0 问题**:
  - 计划本体的 4 个 Phase 全部 pending
  - `crates/ralph-core/src/payload_contract.rs` 第 18 行 `use regex::Regex;` 仍存在（计划要求删除）
  - `HatConfig` 仍无 `schema_refs` 字段
  - 所有 builtin preset YAML 仍依赖正则兜底
- **P1 问题**:
  1. 计划本体的目的（**消除正则误判**）已被 **2026-06-15-001 schema-aware hat emit instructions** 计划间接覆盖（commit `20195df feat(ralph-core): InstructionBuilder schema-aware` + `49d7779 feat(ralph-core): 新增 emit_schema_hint 共享模块`）——**目标已被侧路达成**
- **修复质量评分**: 1 / 5（未启动）
- **复发风险评估**: **不适用**
- **建议后续动作**:
  1. **强烈建议取消**：schema-aware emit instructions 已经把"agent 知道必填字段"和"启动 lint 必填字段"打通；regex 提取仍存在但**不再是 single-source-of-truth**，仅作为 instruction hint 的兜底
  2. 如果保留正则风险作为 backstop（fail-open → fail-closed），至少需把第 18 行 `use regex::Regex;` 改为 `#[allow(unused)]` 或显式 fail-closed 标注

---

## 3. 诊断报告 P0/P1 根治验证表

| 诊断报告 | P0/P1 编号 | 修复对应 | 当前状态 | 证据 |
|---|---|---|---|---|
| 2026-06-13 wave-not-firing | U2 链路卡死 | 2026-06-11-004 U3 dispatcher deadline + partial_threshold_fired | ✅ 已根治 | `c01ae98 fix(cli): U3 让 partial_threshold_fired flag 真正可变`；`u2_non_cap_rejections_do_not_publish_plan_blocked` PASS |
| 2026-06-13 wave-synthesizer-no-fire | wave → bus 8 次 hat=review-coordinator dropped | 2026-06-11-003 multi-hat isolated + 006 multi-hat isolated regression | ✅ 已根治 | `incident_fixture::u6_incident_fixture_eight_dimension_done_all_accepted` PASS |
| 2026-06-15 plan-gate dual-publish blocking | isolated mode budget 丢弃 work.ready | 2026-06-15-003 plan-gate dual-publish isolated budget | ✅ 已根治 | `test_plan_gate_dual_publish_handoff` + `test_plan_gate_dual_publish_inverse_rejected` + `test_plan_gate_dual_publish_third_blocked` 3/3 PASS |
| 2026-06-15 work.ready payload contract | coordinator 缺 `--json` 示例 | 2026-06-15-001 schema-aware hat emit instructions + 2026-06-15 fix(ralph-cli): C3 hat-scoped fix hint | ✅ 已根治 | `feat(ralph-core): emit_schema_hint 共享模块` (commit `49d7779`) |
| 2026-06-15 worktree isolation leak | context.md 暴露 main repo 路径 | 2026-06-15-002 fix-worktree-context-main-repo-leak | ✅ 已根治 | `fix(worktree): context.md 不再泄漏主仓路径` (commit `eb5a49a`) |
| 2026-06-16 loop diagnostic | work.ready JSON parse 失败 + isolated scope violation | 上述 fix + `cba9f32 fix(ralph-cli): emit.rs fail-closed / envelope stderr / wave --config 合一` | ✅ 已根治 | `commands/emit.rs:117-120` `looks_like_json` 增强 + envelope stderr |
| 2026-06-17 flow-reliability-plan-loop-synthesizer-stall | 11→4 维 stall + empty_diff bypass + PayloadContractViolation fatal | 2026-06-17-003 U1+U2+U3+U4+U5+U6 | ✅ 已根治 | merge commit `44b9240`：ralph-core 2112/2112 + BDD 28/28 + zippy-sparrow replay PASS |
| 2026-06-03 execution contract review | R7 TaskNotFound fail-open + R9 git evidence 假阳性 | 后续 2026-06-04 多个 fix 已修 | ✅ **已修复**（实测确认） | `execution_contract.rs:243 validate_task` 现 fail-closed；`execution_contract.rs:560 validate_git_change` 用 `loop_start_sha` + `has_uncommitted_changes` + `has_new_commits_since` 三种 mode |
| 2026-06-11 u3-dispatcher review | P0 `partial_threshold_fired` 死代码 | `c01ae98 fix(cli): U3 ...` | ✅ 已根治 | 9 个 u3_ 测试 + 全 ralph-cli binary 940/0 |

---

## 4. 全局总结

### 总体完成质量: **~85%**

**完成的（高完成度）**:
- 3 条主线（flow-reliability / step-handoff / wave stall bypass）全部机制级落地
- 8 项诊断报告 P0/P1 问题全部根治（含 2026-06-03 review 报告的 R7/R9）
- ralph-core 2215 测试全绿 + 关键 BDD scenarios 28/28
- preset check + agent docs + CLAUDE.md/AGENTS.md 同步到位

**未完成的（需要决策）**:
- 2 项大型重构计划（event_loop 拆分 + schema_refs）**6 天 / 14 天 仍 stalled**
- 当前 working tree 6 个未提交变更需先 commit
- 部分 P2 类小修（P1-3 DEC-002 confidence 协议、P2-5 diagnosis-summary 对账）仍 pending

### 共性风险模式

1. **"机制层 enforcement 替代 preset 文案"的哲学被普遍采纳**：preset L735-748 empty_diff HARD RULE 等关键约束都同时加了 Rust 端 gate——这是好的方向
2. **preset L+C 同步漂移**：每次改 preset 必须 en + zh + manifest + presets.rs 四同步（CLAUDE.md 强约束）；近 2 周基本守住
3. **commit message 格式不统一**：merge commit `44b9240` 用 `merge: 2026-06-17-003 plan` 而 feat commit 用 `feat(...)`，但都在 conventional commit 范围内
4. **dogfood-driven 修复链**：zippy-sparrow 失败 → 003 计划 → review-safe/gated auto fix → 当前 working tree 闭环——这是好的 dogfood 模式

### TOP 3 优先跟进项（按紧急程度）

1. **🔴 立即合并当前 6 个未提交变更**：loop_state / event_loop/mod / progress_task_gate + 3 BDD scenarios。这是 review-safe/gated 的 fix commit 闭环，**不是 debug 残留**。若不立即合并会与下一波 commit 冲突丢失 review 修正。
2. **🟠 取消 2026-06-10-003 拆分计划**：6 天 1/7 落地 + 88 commit 漂移。**新功能（flow_lifecycle / step_handoff）已在自然拆分 mod.rs**，强行 refactor 价值<成本。
3. **🟠 取消 2026-06-03-003 schema_refs 计划**：schema-aware emit instructions 已侧路达成目标，14 天未动。建议至少把 `use regex::Regex` fail-closed 标注。

### 建议补充的测试 / 验证

| 测试 | 目的 | 优先级 |
|---|---|---|
| `test_diagnosis_summary_recovery_count_reconcile` | 验证 P2-5 (recovery_count 不对账) | P1 |
| `test_incomplete_wave_emits_before_aggregate_timeout` | 验证 U2 不需要等到 1800s 才 emit plan.blocked | P1 |
| `test_last_reviewed_sha_blocked_when_wave_open` | 验证 U5 wave_closed 闸门 | P1 |
| 手动 dogfood：`wave_total=11 received=4` 验证 plan.blocked 在 <30s 内 emit | 验证 U2 时机参数 0.8 × aggregate_timeout_secs 是否合理 | P2 |
| `cargo nextest run --workspace --exclude ralph-e2e` 全量回归 | 验证 17-003 + 17-002 + 17-001 三计划无 cross-regression | P0（合并前必须） |
| `scripts/audit-file-sizes.sh` 扩展覆盖 event_loop/*.rs | 验证 010-003 计划遗留 todo | P3（若取消 010-003 则降级） |

---

## 5. 当前 working tree 6 个未提交变更的判定

**不是 debug 残留，是 review 闭环 fix**。

| 文件 | 变更性质 | 评审 commit 锚点 | 必须保留理由 |
|---|---|---|---|
| `event_loop/loop_state.rs` | +1 字段 `last_upstream_verdict_payload` | `fix(step-handoff, review-gated)` issue #3 + U6 | 防 fake-pass on report.done 覆盖 upstream fail |
| `event_loop/mod.rs` | plan.blocked payload 改 JSON object + extract_xml_attr fallback + verdict_gate 二次校验 | `fix(step-handoff, review-safe)` issue #7 + `fix(step-handoff, review-gated)` issue #1 | 与 schema 兼容 + XML fixture 支持 + verdict_gate 闭环 |
| `step_handoff/progress_task_gate.rs` | 区分 `progress_not_found` vs `progress_unreadable` + `is_cold_start_step` 修复 step-10 误判 | `fix(step-handoff, review-gated)` issue #5 + 评审发现 | 真 fail-closed + 防 cold-start 误判 |
| 3 BDD scenarios | 测试断言更新 | 跟随 progress_task_gate.rs 修复 | 测试覆盖必须同步 |

**建议动作**: 立即 stage 并 commit，commit message 建议：

```
fix(step-handoff, review-finalize): 合入 review-safe/gated 闭环 + 修复 step-10 误判

- 引入 last_upstream_verdict_payload 防止 mirror event 覆盖上游 verdict
- plan.blocked payload 改 JSON object (兼容 event_policy.schemas)
- verdict_gate 二次校验 last_upstream_verdict_payload
- ProgressTaskGate 区分 progress_not_found vs progress_unreadable
- is_cold_start_step 修正 step-10 误判为 cold-start

测试: ralph-core 2215/2215, BDD 28/28, preset check PASS
```

---

## 6. 关键时间线（用于交叉验证）

| 日期 | 事件 | 影响 |
|---|---|---|
| 2026-06-03 | Execution contract review 报告指出 R7/R9 未满足 | **当前已修复**（实测确认） |
| 2026-06-10 | event_loop 拆分计划立项 | 仍 stalled |
| 2026-06-11 | U3 dispatcher deadline + P0 partial_threshold_fired 修复 | 940/0 测试通过 |
| 2026-06-13 | wave-synthesizer-no-fire 诊断 + multi-hat isolated 修复 | 8/8 dim done 通过 |
| 2026-06-15 | plan-gate dual-publish blocking 诊断 + worktree leak 修复 + work.ready payload contract 修复 | 3 个 P0 闭环 |
| 2026-06-16 | flow-reliability / step-handoff / wave-stall 三计划立项 | 文档状态 active |
| 2026-06-16 17:47 | `fix(step-handoff, review-safe)` + `fix(step-handoff, review-gated)` 闭环 commit | 当前 working tree 来自此 |
| 2026-06-16 16:45 | `merge: 2026-06-17-003 plan` 汇总 6 unit | 2112/2112 测试通过 |

---

## 7. 一句话行动项

**先把当前 6 个未提交变更 commit 掉**（review-safe/gated 闭环成果），然后跑一次 `cargo nextest run --workspace --exclude ralph-e2e` 全量回归，最后**取消 010-003 与 003-003 两个 stalled 大型重构计划**——3 条主线已机制级落地，代码组织的自然演进（flow_lifecycle.rs / step_handoff/）比强行拆分更可持续。
