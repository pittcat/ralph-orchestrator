# Ralph Loop 链路诊断报告（对抗性审查修订版 v2）

> **Loop**：`primary-20260629-153653`（PID 44699，已结束、loop.lock 已释放）
> **Preset**：`presets/en/ce-executor-serial.yml`
> **时间窗**：2026-06-29 15:36:53Z → 16:31:44Z（LOOP_COMPLETE）
> **诊断时点**：2026-06-30 00:31（loop 终态后）
> **变更说明**：v1 在 loop 仍"安静未结束"时（iter=27、事件流 28 行）出具。loop 实际在 16:31:44 走 `plan.blocked → REVIEW_COMPLETE(fail) → report.done → human.guidance → LOOP_COMPLETE` 路径收尾。本修订根据终态产物（33 行 events、33 行 ledger、9 行 recovery、loop-termination-reason.json）修正 v1 的归因错误。

---

## 1. 结论摘要

**本次 run 完整走完了闭环（PHASE 1+2+3 全链路，事件流 33 行），但**最终 `verdict=fail`**——代码 100% 正确（23/23 测试通过、Hoare 分区修复递归深度问题），但编排层 2 个 bug 导致 `plan.complete` 被拒，coordinator 降级到 `plan.blocked`，shipper 给出 `verdict=fail`，reporter 写出 `awaiting_decision=true` 的报告，ralph hat 又违规 emit `human.guidance`（回归历史 H3）。最终 LOOP_COMPLETE 被显式发出，loop 退出。**

- **P0 偏离 × 4**（v1 是 3 条）：
  - **P0-1**：`plan.complete` payload **缺 `step` 字段**，`review_step_state.rs:314-318` 的 fix-* 豁免不生效，被 `plan_gate_review_not_terminal` 拒收 3 次 → coordinator 降级到 `plan.blocked`（v1 错误归因为"走 RepairStream"，实际是 plan_gate 拒收）
  - **P0-2**：fix-unit task_id 复用 step-02 的 `task-1782747751-e890`（v1 结论保留）
  - **P0-3**：`progress.md` 与 `tasks.jsonl` drift（v1 结论保留）
  - **P0-4【新增】**：`ralph` hat 违规 emit `human.guidance`（事件 31），违反 `preset_lint/hat_scope_invariant.rs:89` 的 `GLOBALLY_FORBIDDEN_PUBLISHES` 与 preset L215 `suppress_human_guidance: true`——**H3 历史修复完全回归**
- **P1 偏离 × 1**（v1 是 2 条，删除 P1-2 因事实错误）：
  - **P1-1**：recovery.jsonl 的 `repair_dispatch` 是**事件被拒记录**而非 fallback 路由（v1 误判归因）
- **P2 偏离 × 4**（保留 v1）
- **历史关联**：H3 回归（human.guidance）；其他 8 条历史问题（H1/H2/H4-H9）仍闭环

**根因分布**：**编排问题 4 条（P0-1 编排 + P0-2 编排 + P0-4 编排 + 全部 P2），基座机制问题 1 条（P0-3 projector + P1-1 plan_gate）**——编排问题占绝大多数，基座机制（事件循环、状态机、contract）本身按设计运行，问题集中在**编排层的 payload schema 设计与历史 lint 规则的执行**。

---

## 2. 执行链路对比图（v2 修订：补充完整闭环）

| step | row | 预期 hat | 预期事件 | 实际 hat | 实际事件 | 状态 |
|------|-----|----------|----------|----------|----------|------|
| 启动 | 1 | loop | `work.start` | loop-bootstrap | `work.start` | ✅ |
| Unit 1 dispatch | 2 | coordinator | `work.ready(step-01)` | coordinator | `work.ready` step-01 | ✅ |
| Unit 1 done | 3 | executor | `work.done(step-01)` | executor | `work.done` commit=1, 218行 | ✅ |
| Unit 1 test | 4 | validator | `test.passed(step-01)` | validator | `test.passed` 5/5 | ✅ |
| Unit 2 dispatch | 5 | coordinator | `work.ready(step-02)` | coordinator | `work.ready` step-02 | ✅ |
| Unit 2 done | 6 | executor | `work.done(step-02)` | executor | `work.done` commit=1, 272行 | ✅ |
| Unit 2 test | 7 | validator | `test.passed(step-02)` | validator | `test.passed` 17/17 | ✅ |
| PHASE 1 → Review | 8 | coordinator | `review.start(unit=2,total=2)` | coordinator | `review.start` | ✅ |
| Review 维度 1-6 | 9-20 | review-coordinator → dimension-reviewer | ready/done ×6 | review-coordinator → dimension-reviewer | ready/done ×6（7 raw findings） | ✅ |
| 6 维收束 | 21 | review-coordinator | `review.dimensions.complete` | review-coordinator | `review.dimensions.complete` fix_round=0 | ✅ |
| 总体 review | 22 | review-synthesizer | `review.complete` | review-synthesizer | `review.complete` verdict=`pass_with_residuals` | ✅ |
| PHASE 2 Fix-1 dispatch | 23 | coordinator | `work.ready(fix-01)` | coordinator | `work.ready` step=`fix-01` | ✅ |
| Fix-1 done | 24 | executor | `work.done(fix-01)` | executor | `work.done` commit=1, 89行 | ✅ |
| Fix-1 test | 25 | validator | `test.passed(fix-01)` | validator | `test.passed` 23/23 | ✅ |
| Fix-2 dispatch | 26 | coordinator | `work.ready(fix-02)` | coordinator | `work.ready` step=`fix-02` | ✅ |
| Fix-2 done | 27 | executor | `work.done(fix-02)` | executor | `work.done` commit=1, 6行 | ✅ |
| Fix-2 test | 28 | validator | `test.passed(fix-02)` | validator | `test.passed` 23/23 | ✅ |
| **PHASE 2 收束（被拒）** | — | coordinator | `plan.complete(verdict=pass_with_residuals, step=fix-02, ...)` | coordinator | **`plan.complete` 3 次尝试被 plan_gate 拒收**（payload 缺 `step` 字段） | **❌ P0-1** |
| **降级到 plan.blocked** | 29 | coordinator | `plan.blocked` | coordinator | `plan.blocked(reason=semantic_gate_bug+projector_bug)` ts=16:23:46 | ⚠️ 降级 |
| **Shipper verdict** | 30 | shipper | `REVIEW_COMPLETE` | shipper | `REVIEW_COMPLETE(verdict=fail, pass_or_fail=fail, final_findings_count=0)` ts=16:26:14 | ⚠️ verdict=fail |
| **Reporter 写报告** | 31 | reporter | `report.done` | reporter | `report.done(verdict=fail, awaiting_decision=true, report_path=docs/report/2026-06-30-...-report.md)` ts=16:27:27 | ⚠️ 报告生成 |
| **【P0-4 回归】human.guidance** | 32 | （无） | — | ralph | **`human.guidance(message="...awaiting decision on how to proceed")` ts=16:29:42** | **❌ P0-4 违规 emit** |
| **Loop 结束** | 33 | loop | `LOOP_COMPLETE` | ralph | `LOOP_COMPLETE(reason="...blocked by orchestration system bugs")` ts=16:31:19 | ✅ LOOP_COMPLETE 已发 |
| **终止原因** | — | loop | (terminal reason) | — | `loop-termination-reason.json`: `{"completion_stuck":{"source":"structural_rejection","retry_key":"verdict_fail:REVIEW_COMPLETE","attempts":1,"last_reason":"verdict fail on REVIEW_COMPLETE (pass_or_fail=fail)"}}` | ⚠️ completion_stuck |

**v2 链路图标注**：1-28 行 ✅；29-33 行为**非预期但合理的降级路径**（v1 的"❌ 缺失"全部补齐，但其中 P0-4 `human.guidance` 是 H3 回归）。

---

## 3. 历史问题上下文（v2 修订：H3 标记为回归）

| 历史问题 | 关联度 | v2 修订 |
|----------|--------|---------|
| **H1** FlowStepScope 误拒 `review.dimensions.complete` | 高 | ✅ 闭环 |
| **H2** drift_monitor flip storm | 高 | ✅ 闭环 |
| **H3** coordinator 越权 `human.guidance`/`loop.stalled` | **🔴 回归** | ❌ **未闭环**：`hat_scope_invariant.rs:89` 的 `GLOBALLY_FORBIDDEN_PUBLISHES` 与 preset L215 `suppress_human_guidance: true` 均被绕过，本次 run 第 32 行 `ralph → human.guidance` 实际发出 |
| **H4** `TaskWrongLoop` `actual_loop=None` 缺 loop_id | 中 | ✅ 已修，但**新形态**：fix-unit task_id 复用 step-02（P0-2） |
| **H5** dimension-reviewer 写 plan.md scope_violation | 中 | ✅ 闭环 |
| **H6** phase 终态后 review 链仍推进 | 中 | ⚠️ 本次未触发（因为走了 plan.blocked 降级路径而非正常 plan.complete） |
| **H7** stall_recovery/missing_event_gate 双轨 retry_key | 中 | ✅ 闭环 |
| **H8** progress-steward self-loop 死路径 | 中 | ✅ 闭环 |
| **H9** dimension-reviewer Bash 越权测试 | 高（未闭环） | ⚠️ plan `2026-06-29-001` 仍 active，本次 fix-plan 链路干净仅因静态结论 |

**v2 历史风险画像**：本次 run **命中 H3 回归**——这是 v1 未发现的关键问题，因为 v1 在 loop 未结束时已出具，错过了 `human.guidance` 事件。其他 8 条历史问题保持 v1 结论。

---

## 4. 证据清单（v2 修订：补全 P0-4 + P0-1 payload 证据）

| 证据 | 文件:行号 | 实际值 | 预期值 |
|------|-----------|--------|--------|
| events 完整行数 | `events-20260629-153653.jsonl:33` | 33 行（含 28→33 终态链） | — |
| plan.complete payload 缺 step | `recovery.jsonl:3-5` 三条 payload_preview | `{"plan_name":"...","completed_steps":"step-01,step-02,fix-01,fix-02","task_id":"...","task_key":"..."}` | 应包含 `step="fix-02"` 以触发 `review_step_state.rs:314-318` 豁免 |
| plan_gate 拒收机制 | `crates/ralph-core/src/event_loop/review_step_state.rs:305-318` | 当 `plan.complete` 的 `step.starts_with("fix-")` 时直接 `return None`（放行） | — |
| plan_gate 实际拒收 | `crates/ralph-core/src/event_loop/review_step_state.rs:138` | `state.synth_terminal` 非 `review.passed/review.complete` 或 `state.synth_pass=false` → 返回 `plan_gate_review_not_terminal` finding | — |
| 拒收次数 | `recovery.jsonl:3-5` + 注释 `repair_sink: topic=plan.complete` | 3 次重试全部被拒（reason_code=`repair_dispatch`） | 至少 1 次成功入 events.jsonl |
| coordinator 降级 | `events-20260629-153653.jsonl:29` | `coordinator → plan.blocked(reason="semantic_gate_bug: fix-unit test.passed rejected by plan_gate_review_not_terminal...")` ts=16:23:46 | 应走 plan.complete 而非 plan.blocked |
| shipper 接受降级 | `events-20260629-153653.jsonl:30` | `shipper → REVIEW_COMPLETE(verdict=fail, pass_or_fail=fail, residual_findings_summary="plan.blocked due to semantic_gate_bug...")` | 正常路径应是 `verdict=pass` |
| reporter 写报告 | `events-20260629-153653.jsonl:31` | `reporter → report.done(verdict=fail, awaiting_decision=true, report_path="docs/report/2026-06-30-ce-executor-2026-06-20-001-feat-python-sort-algorithms-report.md")` | 报告已写，路径如预期 |
| **P0-4 回归** | `events-20260629-153653.jsonl:32` | `ralph → human.guidance(message="plan implementation complete but blocked by orchestration bugs...Awaiting decision on how to proceed.")` ts=16:29:42 | **不应出现**：`hat_scope_invariant.rs:89` 明文禁止、preset L215 `suppress_human_guidance: true` |
| LOOP_COMPLETE 显式 emit | `events-20260629-153653.jsonl:33` | `ralph → LOOP_COMPLETE(reason="All code work complete...Blocked by orchestration system bugs...")` ts=16:31:19 | ✅ preset L179 `completion_promise` 兑现 |
| loop 终止原因 | `loop-termination-reason.json:1` | `{"completion_stuck":{"source":"structural_rejection","retry_key":"verdict_fail:REVIEW_COMPLETE","attempts":1,"last_reason":"verdict fail on REVIEW_COMPLETE (pass_or_fail=fail)"}}` | — |
| loop.lock 已释放 | `loops.json` | `{"loops": []}`（空数组） | v1 时 `pid=44699`，v2 已退出 |
| task_id 复用 | `events-20260629-153653.jsonl:22-27` + `agent/tasks.jsonl:4-5` | fix-01/fix-02 全部携带 `task_id=task-1782747751-e890`，tasks.jsonl 中 fix-01/fix-02 status=`open` | 每个 fix-unit 应有独立 task_id + closed |
| ledger | `ledger.jsonl:33` | iter 1→32，sequence 33（含 `completion_requested` 与最终 `counter_changed`） | — |
| diagnostic recovery | `recovery.jsonl:1-9` | 9 条 repair_dispatch：3 条 plan.complete 拒收 + 3 条 plan.blocked 拒收 + 2 条 work.ready + 1 条 task.resume | — |

---

## 5. 问题归因表（v2 修订：补 P0-4 + 重写 P0-1/P1-2）

| 优先级 | 问题描述 | **根因分类** | 证据 | 历史关联 |
|--------|----------|------|------|----------|
| **P0-1【v2 重写】** | coordinator emit 的 `plan.complete` payload **缺 `step` 字段**，导致 `review_step_state.rs:314-318` 的 fix-* 豁免逻辑不生效，3 次重试全部被 `plan_gate_review_not_terminal` 拒收，coordinator 降级到 `plan.blocked`，shipper 给出 `verdict=fail` | **preset 设计 + coordinator 编排**（编排问题） | `recovery.jsonl:3-5` payload_preview（无 step 字段） + `events:29` plan.blocked + `crates/ralph-core/src/event_loop/review_step_state.rs:305-318` | H6（未在本 run 验证） |
| **P0-2** | fix-unit task_id 复用 step-02 的 `task-1782747751-e890`，tasks.jsonl 中 fix-01/fix-02 状态始终为 `open` | **coordinator 编排**（编排问题） | `events-20260629-153653.jsonl:22-27` + `agent/tasks.jsonl:4-5` | H4（同类已修） |
| **P0-3** | `progress.md` 显示 step-01/step-02/fix-01/fix-02 完成，但 `tasks.jsonl` 中 step-01/fix-01/fix-02 仍为 `open` | **loop 基座（projector 写路径）**（基座机制问题） | `agent/tasks.jsonl:2,4,5` | H3（同模式） |
| **P0-4【v2 新增】** | `ralph` hat 违规 emit `human.guidance`（事件 32），违反 `preset_lint/hat_scope_invariant.rs:89` `GLOBALLY_FORBIDDEN_PUBLISHES` 与 preset L215 `suppress_human_guidance: true`，**H3 历史修复完全回归** | **loop 基座（preset_lint 未拦截）+ ralph hat 编排**（编排 + 基座叠加） | `events-20260629-153653.jsonl:32` + `crates/ralph-core/src/preset_lint/hat_scope_invariant.rs:89` | **H3 回归** |
| **P1-1【v2 重写】** | RecoveryStream 9 条 `repair_dispatch` 实际是**事件被 plan_gate 拒收的记录**（v1 误以为是 fallback 路由）。3 次 `plan.complete` + 3 次 `plan.blocked` 全部被拒后系统无法再 emit，最终通过 `LOOP_COMPLETE` 显式跳出 | **loop 基座（plan_gate 与 recovery_sink 语义不对齐）**（基座机制问题） | `recovery.jsonl:1-9` + `events:29-33` 完整链 | H4（同类） |
| **P1-2【v2 删除】** | ~~loop 在 iter=27 后空转~~ | — | 实际：loop 在 iter=31 后正常走 plan.blocked→LOOP_COMPLETE 路径（loop-termination-reason.json: `completion_stuck`），并非空转 | — |
| **P2-1** | fix-unit `work.ready.plan_path` 指向 `fix-plan.md` 而非原始 plan，与 `plan_name_equality_required: true` 语义有歧义 | preset 设计（编排问题） | `events:22` + `ce-executor-serial.yml:574, 1169` | 无 |
| **P2-2** | preflight 启动 3 条 WARN（debug-resolver/plan-gate hat overlay 被忽略） | 配置残留（编排问题） | `diagnostics/.../trace.jsonl:2-4` | H6（同类） |
| **P2-3** | `test.passed` payload 未携带 commit_count/changed_lines 字段，可观测性弱化 | preset schema 设计（编排问题） | `events:4,7,25,28` | 无 |
| **P2-4** | `fix-plan.md` 的 final_findings_count 字段语义模糊 | preset schema 设计（编排问题） | `events:22` + fix-plan.md | 无 |

**v2 P0 根因分布**：编排 2 条（P0-1、P0-2）+ 基座 1 条（P0-3）+ 叠加 1 条（P0-4 编排 + 基座 lint）——**编排仍占主导，但基座的 plan_gate/lint 拦截缺失也是关键**。

---

## 6. 修复建议（v2 修订：聚焦 P0-1 payload + P0-4 lint 拦截）

### P0-1：plan.complete payload 强制携带 step 字段（编排 + 源码）
- **目标**：`crates/ralph-core/src/coordinator.rs`（emit `plan.complete` 处） + `presets/schemas/ce-executor-serial.yml`（required_fields）
- **根因**：coordinator 在 fix-unit 末端的 `plan.complete` emit **payload 缺 `step` 字段**（只有 plan_name/completed_steps/task_id/task_key），源码 `review_step_state.rs:314-318` 的 `if step.starts_with("fix-")` 豁免**条件不满足**，于是被 `plan_gate_step_gate` 判定 `synth_terminal` 非 `review.passed/review.complete` → 返回 `plan_gate_review_not_terminal` finding，事件被拒。
- **修复**：
  1. **`presets/schemas/ce-executor-serial.yml`**：将 `plan.complete` 的 `required_fields` 强制加入 `step: string`（fix-unit 路径）或基于上下文自动注入
  2. **`crates/ralph-core/src/coordinator.rs`**：emit `plan.complete` 前自动从 fix_unit_context 注入 `step` 字段（fallback）
  3. **更稳健**：将 `review_step_state.rs:305-318` 的豁免条件改为"检查 `completed_steps` 是否含 `fix-*`"（payload 已有 `completed_steps` 字段），不再依赖 `step` 单字段
  4. 加 BDD 场景 `last-fix-unit-completion-emits-plan-complete.yml`（**必须用 `run_workflow_guard_scenario` 断言 events**），覆盖 fix-unit 末端 `plan.complete` 的 payload schema
- **预期效果**：fix-NN 完成后 `plan.complete` payload 自动含 step 字段，触发 `review_step_state.rs:314-318` 豁免，主事件流落 `plan.complete` → shipper → reviewer `verdict=pass` → LOOP_COMPLETE clean

### P0-2：fix-unit task_id 独立化（编排）
- **目标**：`crates/ralph-core/src/coordinator.rs` + `presets/en/ce-executor-serial.yml` Fix-3 硬规则
- **修复**：见 v1（保留原建议）

### P0-3：projector 写 tasks.jsonl 与 progress.md 同步（基座）
- **目标**：`crates/ralph-core/src/state_projector/task.rs` 和 `progress.rs`
- **修复**：见 v1（保留原建议）

### P0-4【v2 新增】：lint 规则升级为硬拦截（基座）
- **目标**：`crates/ralph-core/src/preset_lint/hat_scope_invariant.rs:89` + `crates/ralph-core/src/event_loop/mod.rs`（emit gate）
- **根因**：`GLOBALLY_FORBIDDEN_PUBLISHES` 规则存在但**未被运行时执行**——`ralph` hat 实际 emit 了 `human.guidance`，lint 仅在 preset 加载时校验，不在运行时拦截
- **修复**：
  1. 在 `crates/ralph-core/src/event_loop/mod.rs` 的 emit gate 中加入 `GLOBALLY_FORBIDDEN_PUBLISHES` 运行时检查（参考 `topic_deny_rules`）
  2. preset L215 `suppress_human_guidance: true` 应作为硬约束——若 hat 仍尝试 emit，loop 应直接报错而非写入 recovery
  3. 加测试 `human_guidance_runtime_blocked`（在 `hat_scope_invariant.rs:638` 单测基础上加运行时拦截）
- **预期效果**：H3 历史修复真正闭环，未来 `human.guidance` 在任何 hat 上 emit 都会被拒

### P1-1：plan_gate 与 recovery_sink 语义对齐（基座）
- **目标**：`crates/ralph-core/src/event_loop/mod.rs`（repair_dispatch 分支）
- **修复**：在 `recovery.jsonl` 中区分 `repair_dispatch` 的语义——`event_rejected_by_gate` 与 `event_routed_to_repair` 应分开记录，便于诊断（当前都是 `repair_dispatch`，无法区分）

### P2 修复（清理级，保留 v1）
- **P2-1**：`review.start` payload schema 加 `plan_path` 必填
- **P2-2**：清理 `ralph.yml` 残留 hat overlay
- **P2-3**：`test.passed` payload 加可选 commit 字段
- **P2-4**：preset instructions 显式 `final_findings_count` 字段语义

---

## 7. 直接回答用户的 4 个问题（v2 修订）

### Q1：整体执行过程有没有问题？
**主体执行没问题（28 行事件流、PHASE 1+2 全链路、6 维 review、5 个降级事件全部按既有路径走完），但**最终 verdict=fail**——代码 100% 正确（23/23 测试通过），但编排层 2 个 bug 导致 `plan.complete` 被拒，最终以 `verdict=fail` 收尾（虽然代码本身应 pass）。**

### Q2：中间产物是否符合 RALPH 基座机制、是否正常生效？
**部分生效**：
- ✅ 事件流（33 行）、ledger（iter 1→32）、state machine、projection（progress.md）、execution_contracts（work.done 7 字段）、topic_deny_rules（除 P0-4 外）、plan_name_equality 正常
- ⚠️ **task_id 隔离失败**（P0-2）
- ⚠️ **tasks.jsonl 与 progress.md drift**（P0-3）
- ❌ **`plan.complete` payload schema 与源码豁免条件不匹配**（P0-1，源码 `review_step_state.rs:314` 要求 `step.starts_with("fix-")`，但 coordinator emit 的 `plan.complete` payload 不带 step）
- ❌ **`GLOBALLY_FORBIDDEN_PUBLISHES` 规则未运行时执行**（P0-4，H3 回归）
- ✅ `completion_promise LOOP_COMPLETE` 已兑现（事件 33）

### Q3：编排是否合理、是否正常运行？
**编排合理（10-hat 拓扑、PHASE 1/2/3 分阶段、6 维 review、fix_plan 收口），但运行走的是降级路径而非正常路径**。preset L45-46 期望的路径：

```
fix-XX → coordinator → plan.complete → shipper → reporter → LOOP_COMPLETE
```

实际走的路径：

```
fix-XX → coordinator → plan.complete (3次被拒) → coordinator → plan.blocked → shipper(verdict=fail) → reporter(verdict=fail) → ralph → human.guidance (违规) → ralph → LOOP_COMPLETE
```

**编排的语义漏洞**：当 `plan.complete` 被拒时，coordinator 正确降级到 `plan.blocked`（避免死锁），但 shipper/reporter 的 `verdict=fail` 与"代码实际 pass"的语义冲突——`verdict=fail` 应表示"代码不达标"，但本次报告里 shipper 自己说"Implementation is correct: Hoare partition implemented, all 23 tests pass, P0 recursion depth issue resolved."。**这是 shipper 把"编排被拒"等同于"代码失败"的语义误判**。

### Q4：如果真有问题，是机制问题还是编排问题？
**主要是编排问题（含 preset schema 设计 + coordinator payload 构造 + lint 规则未运行时执行），机制问题占小部分**：

| 偏离 | 根因分类 | 备注 |
|------|----------|------|
| P0-1 `plan.complete` payload 缺 `step` | **编排**（coordinator emit payload schema）+ **基座源码**（`review_step_state.rs:314-318` 豁免条件苛刻） | 编排 + 基座叠加 |
| P0-2 fix-unit task_id 复用 | **编排**（coordinator 未遵守 preset Fix-3 硬规则） | 编排 |
| P0-3 tasks.jsonl/project.md drift | **基座机制**（projector 写路径覆盖不全） | 基座 |
| P0-4 `human.guidance` 回归 | **基座机制**（lint 规则未运行时拦截） | 基座 |
| P1-1 recovery_sink 语义不对齐 | **基座机制**（plan_gate 与 recovery_sink） | 基座 |
| P2-1 ~ P2-4 | 编排（preset/schema/配置残留） | 编排 |

**结论**：
- **编排问题 3 条（P0-1 编排部分、P0-2、全部 P2）**——preset schema 设计不充分（缺 step 必填）、coordinator payload 构造不一致、未清理 ralph.yml 残留 hat
- **基座问题 4 条（P0-1 源码豁免条件部分、P0-3、P0-4 lint 未运行时、P1-1 recovery 语义）**——源码 `review_step_state.rs` 豁免条件仅依赖 `step` 单字段、projector 写路径不全、`GLOBALLY_FORBIDDEN_PUBLISHES` 仅 lint 时校验、recovery.jsonl 不区分 reject vs route

**v2 与 v1 归因反转**：v1 说"编排问题占 2/3、基座占 1/3"；v2 修正为**编排 3 条 + 基座 4 条**——基座机制（plan_gate 源码豁免条件 + lint 运行时拦截缺失）的责任比 v1 估计的更大。

**v2 编排问题的本质**：preset schema 设计时未将 `step` 字段作为 `plan.complete` 必填项——这与"fix-unit 是 plan.complete 的合法调用路径"的事实不符。**coordinator 的 payload 构造与 schema 契约脱钩**，是导致 plan_gate 拒收的根本原因。

**v2 基座问题的本质**：`review_step_state.rs` 的 fix-* 豁免条件设计过于脆弱（依赖 `step` 单字段），`GLOBALLY_FORBIDDEN_PUBLISHES` lint 规则仅在 preset 加载时生效，运行时无任何拦截——历史 H3 fix 只修了"lint 阶段"未修"runtime 阶段"，是不完整闭环。

---

## 8. 关键源码与文档锚点（v2 修订：补 plan_gate 与 lint 拦截证据）

- `presets/en/ce-executor-serial.yml:45-46, 79, 102-131, 179, 215, 269-276, 638, 736-755, 921-928, 1169, 2099-2180`
- `crates/ralph-core/src/event_loop/review_step_state.rs:126-142, 305-318`（**P0-1 关键**：plan_gate 豁免条件 + 拒收逻辑）
- `crates/ralph-core/src/event_loop/mod.rs:8056`（commit `2ac23dea` 已接 current_loop_id）
- `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs:30-86`（H1 fix 已激活）
- `crates/ralph-core/src/event_loop/stages/coordinator_decision_gate_stage.rs`（P0-1 修复目标）
- `crates/ralph-core/src/event_loop/tests/review_step_gate.rs:232`（已有 "plan.complete with step=fix-* must skip plan_gate_review_not_terminal" 单测）
- `crates/ralph-core/src/state_projector/task.rs:100-104`（P0-3 修复目标）
- `crates/ralph-core/src/recovery_runtime/finalize_recovery_outcome.rs:24-216`（H2 fix 已激活）
- `crates/ralph-core/src/preset_lint/hat_scope_invariant.rs:74-89, 278-346, 638-717`（**P0-4 关键**：GLOBALLY_FORBIDDEN_PUBLISHES + 单测）
- `crates/ralph-core/src/drift/engine.rs:1282`（plan_gate_review_not_terminal retry_key 注册）
- `docs/plans/2026-06-29-001-fix-dimension-reviewer-bash-hard-block-plan.md`（H9 active）
- `docs/report/2026-06-29-ce-executor-serial-primary-20260629-120038-diagnosis.md`（同型历史诊断）
- `docs/report/2026-06-30-ce-executor-2026-06-20-001-feat-python-sort-algorithms-report.md`（**reporter 写出的最终报告**——本次 run 闭环证据）

**v2 修订建议的下一步**：
1. 先修 P0-1（coordinator emit `plan.complete` 自动注入 `step` 字段），跑 BDD `last-fix-unit-completion-emits-plan-complete.yml` 验证主事件流确实落 `plan.complete`
2. 再修 P0-2（task_id 独立化）和 P0-3（projector 同步）
3. 修 P0-4 时同时把 `GLOBALLY_FORBIDDEN_PUBLISHES` 升级为运行时拦截，并加测试 `human_guidance_runtime_blocked`
4. 跑 `./scripts/run-tests.sh` 全量回归，特别关注 `preset_lint` + BDD scenarios + SSOT byte-equality

---

## 附录：v1 → v2 主要修订清单

| # | 修订点 | v1 结论 | v2 结论 | 证据 |
|---|--------|---------|---------|------|
| 1 | 链路结尾状态 | "未观察到 LOOP_COMPLETE，loop 空转" | "33 行事件流、loop 正常结束" | `events:29-33` + `loop-termination-reason.json` |
| 2 | P0-1 根因 | "coordinator emit fallback 把 plan.complete 路由到 RepairStream" | "payload 缺 step 字段，plan_gate 拒收 3 次后降级到 plan.blocked" | `recovery.jsonl:3-5` payload_preview（无 step）+ `review_step_state.rs:305-318` |
| 3 | P1-2 删除 | "loop 在等不到的事件上空转" | 删除（事实错误） | loop 实际走 plan.blocked 路径收尾 |
| 4 | P1-1 重写 | "修复流与事件流水分离" | "repair_dispatch 是事件被拒的记录而非 fallback 路由" | `recovery.jsonl` + `events:29-33` |
| 5 | P0-4 新增 | 无 | "`human.guidance` 回归 H3" | `events:32` + `hat_scope_invariant.rs:89` |
| 6 | H3 状态 | "✅ 闭环" | "❌ 回归" | 实际 emit 出现 |
| 7 | 根因分布 | 编排 4 + 基座 2 | 编排 3 + 基座 4 + 叠加 1 | 重新分类 |
| 8 | 修复优先级 | P0-1 编排 fallback | P0-1 coordinator emit payload | 源码 `review_step_state.rs:314` |
| 9 | Agent D 判定 | "无致命偏离" | 与 v1 一致但需在终态产物下重新审视 | v1 文档已记录 |
| 10 | Q3 答案 | "coordinator emit 路径 fallback 误触发" | "shipper 把编排被拒等同于代码失败" | `events:30` `pass_or_fail=fail` 但 payload 承认代码正确 |