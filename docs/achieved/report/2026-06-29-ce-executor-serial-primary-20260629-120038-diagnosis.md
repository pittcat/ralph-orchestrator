# ce-executor-serial primary-20260629-120038 运行链路诊断报告(终版)

> 角色:Ralph Loop 与 ce-executor-serial preset 运行链路诊断专家
> 报告日期:2026-06-29
> Loop:`primary-20260629-120038`(12:00:38 → 13:08 UTC,68 分钟内执行)
> 主仓分支:pittcat-dev(`HEAD = 2ac23dea`)
> **本版相对前版的修订**:重写事件真相(基于 44 行而非 31 行过时快照)、修复 `RecoveryFinalizer` 虚指文件、重新平衡归因比例、补充已闭环 `2ac23dea` 修复

---

## 0. 修订说明(相对前置版本)

本报告前置版本基于 31 行事件快照,关键叙事错。**对抗性审查**发现 3 个致命错误、5 个归因偏差,本版全部修订:

| # | 前版错误 | 真事实 | 来源 |
|---|---|---|---|
| 1 | "loop 静默退出"`LOOP_COMPLETE` 永未发" | loop 实际进入 fix-01 修复轮(L41-L43),`review-synthesizer` L39 二次激活、`fix-01` tests 80→89 通过 | `events-20260629-120038.jsonl` 全文 44 行 |
| 2 | "P0-2 wiring 在 working tree 但未 commit" | commit **`2ac23dea`** (2026-06-29 21:01:56) 已合并,激活 `state_projector/task.rs:100-104` + `event_loop/mod.rs:8056` fallback | `git show 2ac23dea` |
| 3 | "新增 `recovery_runtime/finalizer.rs`" | **不存在**。已存在 `finalize_recovery_outcome.rs`(`mod.rs:18`) | `ls crates/ralph-core/src/recovery_runtime/` |

---

## 1. 结论摘要

**本次 run 健康度:中度异常** — 编排主体执行干净,但 review 闭环失败 + recovery 兜底路径混乱,需要 **人工介入**(`human.guidance`)才进入 fix 修复轮。
**一句话**:`review-synthesizer` 在 `review.dimensions.complete` 后第一次未激活(`FlowStepScope` `flow_unknown_emit` 拒收),`stall_recovery` `handoff_dispatch_timeout` 兜底超时 600s + ralph 主动 `human.guidance` 后才二次激活,fix-01 已落地(80→89 tests),fix-02 开启中。loop **未完全收敛**。

- **关键异常**:P0 = 1 条 / P1 = 3 条 / P2 = 3 条(共 7 条归因)
- **涉及历史重复问题**:5 条 / 7 条
- **归因比例**:编排 50% / 修复 30% / agent 行为 20%

---

## 2. 执行链路对比图

> 数据源对比:来自 preset / schema 的预期 vs `events-20260629-120038.jsonl` 44 行 + `diagnostics/recovery.jsonl` r1-r40 的实际

| Step | 预期 | 实际 | 状态 |
|---|---|---|---|
| 1 | `work.start`(loop-bootstrap) | 12:00:38 | ✅ |
| 2 | coordinator `work.ready(step-01)` | 12:02:12,task_id=task-1782734528-0001 | ✅ |
| 3 | executor `work.done(step-01)` | 12:06:07,被 `execution_contract TaskWrongLoop` 拒(`actual_loop=None`) | 🔁 |
| 4 | executor 重发 `work.done` | 12:07:37,task_id 换为 task-1782734843-e50b,**重发成功** | ✅ |
| 5 | validator `test.passed(step-01)` | 12:08:47,12/12 tests | ✅ |
| 6-8 | step-02 → executor → validator | 12:10–12:13,26 tests | ✅ |
| 9-11 | step-03 → executor → validator | 12:15–12:18,36 tests | ✅ |
| 12-14 | coordinator 重复 `work.ready(step-04)` × 3 | 12:20 / 12:21 / 12:22 (差 90s) | 🔁 |
| 15-16 | step-04 → executor → validator | 12:26 / 12:27,80 tests | ✅ |
| 17 | coordinator `review.start` | 12:28:00,unit_index=2,total_units=2 | ✅ |
| 18-29 | 6 维 review.dimension.ready/done(goal-alignment/correctness/testing/maintainability/project-standards/adversarial) | 12:29–12:44 全部完成,17 findings(3 P1 + 12 P2/P3 + 0 P0) | ✅ |
| 30 | `review.dimensions.complete` | 12:45:34 emit | ⏸️ |
| **31** | **FlowStepScope 接受 → review-synthesizer 激活** | **FlowStepScope `flow_unknown_emit` 拒绝**(12:46:09,diagnostics/recovery.jsonl r12) | **❌ 关键偏离** |
| 32 | review-synthesizer `review.complete` | **未在 trigger 后激活**,stall_recovery 兜底超时 600s | ❌ |
| 33 | progress-steward `task.resume(plan_complete_not_emitted, target=coordinator)` | 12:48:46 | 🔁 |
| 34 | coordinator `plan.blocked(review_synthesizer_stuck)` | 12:51:25(走 hat-channel),同时 `human.guidance` 被 `semantic_gate_violation` 拒(12:51:16) | 🔁 |
| 35 | shipper `REVIEW_COMPLETE(fail)` | 12:53:26,verdict=fail,3 P1 + 12 P2/P3 | ✅(失败分支可走通) |
| 36 | reporter `report.done(fail)` | 12:54:50,report_path=`docs/report/2026-06-29-ce-executor-...-report.md` | ✅ |
| 37 | ralph `human.guidance`(决策请求) | 12:56:58,显式列出 3 P1 bugs(sort_in_place / count_swaps / assertion-less) | ✅(ralph 主动求决) |
| 38 | progress-steward `plan.blocked` | 12:59:01(review_complete_not_emitted_after_task_resume_exhausted) | 🔁 |
| 39 | shipper `REVIEW_COMPLETE(fail)` 二次 | 12:59:58,verdict=fail(同样 fail) | ✅ |
| 40 | reporter `report.done(fail)` 二次 | 13:00:43,awaiting_decision=true | ✅ |
| **41** | **handoff_dispatch_timeout 后 review-synthesizer 二次激活** | **12:57:19 stall_recovery 自动 `task.resume(target=review-synthesizer)`(r33)** | ✅(timeout 兜底成功) |
| 42 | **review-synthesizer emit `review.complete`(× 2 次,L39+L40 重复 emit)** | **13:02:32 + 13:02:36,差 4 秒,verdict=fail,findings_count=3** | **⚠️ agent 重复 emit** |
| 43 | coordinator `work.ready(fix-01, complexity=small)` | 13:04:21,`fix_plan_file` 指向 `.agents/scratchpad/.../fix-plan.md` | ✅ |
| 44 | executor `work.done(fix-01)` + validator `test.passed(89/89)` | 13:06 / 13:07,tests 80→89,新增 9 | ✅(**修复成功**) |
| 45 | coordinator `work.ready(fix-02, count_swaps)` | 13:08:05,正在执行 | 🔁(loop 未完成,仍在跑) |

**说明**:
- ✅ 顺利完成的步骤
- 🔁 修复/重试/兜底路径
- ⏸️ 发出但被拒收
- ❌ 完全未推进
- ⚠️ agent 行为异常

---

## 3. 历史问题上下文

### 历史高频模式 30 天内复发次数 ≥ 6 的顽固问题

| 问题 | 历史关联 | 本次是否复发 |
|---|---|---|
| `flow_unknown_emit`(`FlowStepScope` 误拒 `review.dimensions.complete`) | 多次在 `docs/report/2026-06-17-ce-executor-serial-merry-lotus-*.md` 等历史报告出现,**memory 中无独立 slug**,数据来自 history.jsonl 多 run 累计 | **本次复发**(诊断 r12,12:46:09) |
| `recovery_outcome_update` flip storm(同 retry_key 不同 outcome 反复 flip) | `docs/report/2026-06-29-ce-executor-serial-primary-20260629-032235-diagnosis.md` P1-2 | **本次复发**(r2-r14 共 14 次 flip) |
| coordinator / ralph 越权 emit | `recovery.jsonl:5` 字面 `semantic_gate_violation: hat 'coordinator' is not allowed to publish topic 'loop.stalled'` | **本次复发**(events L32 同时越权发 human.guidance,r3 拒收) |
| `TaskWrongLoop` 拒收(`actual_loop: None` 缺 loop_id) | `crates/ralph-core/src/event_loop/mod.rs:8056` P0-2 wiring 在 commit `2ac23dea` 已修 | **本次是修复前最后一次复现**(loop 启动在 `2ac23dea` 之前) |
| `handoff_dispatch_timeout` 兜底 | `recovery_runtime/stall_recovery.rs` 默认 600s handoff timeout | **本次触发**(r33,12:57:19) |

### 历史修复闭环状态

- **已闭环**:
  - **commit `2ac23dea`** (loop 启动后 commit):`state_projector/task.rs:100-104` + `event_loop/mod.rs:8056` 修 P0-2 wiring
  - **commit `c327d295`**:`LOOP_COMPLETE` 被拒时不污染 `terminal_observed`
  - **commit `76123d49`**:`task_id` 空串 fail-closed
  - **commit `245fcc35`**:`dimension-reviewer` 写 plans/* 改为 hard reject
- **部分闭环**:`compute_retry_key` 跨 stage 共享(`UNIFIED_DETERMINISTIC_CORRECTION` 默认未反转,本次 r2-r14 仍 flip)
- **本次未触发**:hat_handoff / progress-steward retry_cap(plan 007 U3 与本 loop 启动时间相关)

### 完整历史知识库(8 类 26 条)

详见 `docs/report/` 同类历史报告与 `.cursor/rules/multi-hat-isolation.mdc` / `feature-flags.mdc`。本表已收敛最关键 5 条与本次 run 相关的顽固模式。

---

## 4. 证据清单(可定位到文件路径 + 行号 + 事件 ID)

| # | 证据 | 路径:行号 / 事件 ID |
|---|---|---|
| E-1 | `FlowStepScope` 拒收 `review.dimensions.complete` | `.ralph/diagnostics/2026-06-29T20-00-37/recovery.jsonl:r12`(reason=flow_unknown_emit topic=review.dimensions.complete,12:46:09) |
| E-2 | review-synthesizer 第一次未激活 | `.ralph/events-20260629-120038.jsonl:L30` 之后只有 L31 task.resume(L32 coordinator plan.blocked 是 hat-channel) |
| E-3 | 6 维 review 全部 done,17 findings | `.ralph/events-20260629-120038.jsonl:L19,21,23,25,27,29` (6 个 dimension.done,P0=0,P1=3,P2=10,P3=4) |
| E-4 | stall_recovery `handoff_dispatch_timeout` 兜底 | `.ralph/diagnostics/2026-06-29T20-00-37/recovery.jsonl:r33`(12:57:19,timeout 600s,target=review-synthesizer) |
| E-5 | progress-steward `task.resume(plan_complete_not_emitted, target=coordinator)` | `.ralph/events-20260629-120038.jsonl:L31`(12:48:46) |
| E-6 | coordinator `plan.blocked(review_synthesizer_stuck)`(落 hat-channel) | `.ralph/agent/events-hat-coordinator-primary-20260629-120038-30.jsonl:1`(12:51:25) |
| E-7 | coordinator 越权发 `human.guidance` 被 `semantic_gate_violation` 拒 | `.ralph/recovery.jsonl:L4`(12:51:16,allowed publishes: work.ready/review.start/plan.complete/plan.blocked/LOOP_COMPLETE) |
| E-8 | coordinator 越权发 `loop.stalled` 被 `semantic_gate_violation` 拒 | `.ralph/recovery.jsonl:L5`(12:51:21) |
| E-9 | shipper + reporter + ralph 兜底链路 | `.ralph/events-20260629-120038.jsonl:L33-L35`(12:53:26 / 12:54:50 / 12:56:58) |
| E-10 | **review-synthesizer 二次激活(13:02:32)+ 重复 emit review.complete × 2 次** | `.ralph/events-20260629-120038.jsonl:L39,L40`(差 4 秒,verdict=fail,findings_count=3) |
| E-11 | fix-01 修复成功(80→89 tests) | `.ralph/events-20260629-120038.jsonl:L42,L43`(13:06:00 changed_lines=96,13:07:00 tests_passed=89/89) |
| E-12 | fix-02 正在执行 | `.ralph/events-20260629-120038.jsonl:L44`(13:08:05,fix-02 count_swaps) |
| E-13 | `TaskWrongLoop` 拒收(loop 启动在 commit `2ac23dea` 前) | `.ralph/diagnostics/2026-06-29T20-00-37/recovery.jsonl:r2`(12:06:15,actual_loop=None) |
| E-14 | drift_monitor 同 retry_key flip storm × 14 次 | `.ralph/diagnostics/2026-06-29T20-00-37/recovery.jsonl:r2-r40`(Recovered/Pending 交替) |
| E-15 | work.ready step-04 重复 emit × 3 | `.ralph/events-20260629-120038.jsonl:L12-L14`(12:20/12:21/12:22) |
| E-16 | **loop 在 `2ac23dea` 之前启动**(`HEAD=2ac23dea` 已含 P0-2 fix,但 loop 已先开工) | `git log --oneline`:`2ac23dea` 21:01:56,`loops.json:started` 12:00:38 |
| E-17 | `finalize_recovery_outcome.rs` 已存在 | `crates/ralph-core/src/recovery_runtime/finalize_recovery_outcome.rs:L15`(`pub fn finalize_recovery_outcome_on_flapping`) |
| E-18 | 关键 commit hash 验证 | `git show --stat 2ac23dea` 完整可见(commit 21:01:56,P0-2 wiring + 新增 TDD 测试) |
| E-19 | `loops.json` SSOT | `.ralph/loops.json:1-9`(loop_id=primary-20260629-120038,pid=1473441) |
| E-20 | `loop.lock` SSOT | `.ralph/loop.lock`(pid=1473441,started=12:00:37) |

---

## 5. 问题归因表

| 优先级 | 问题描述 | 根因分类 | 证据 | 历史关联 |
|---|---|---|---|---|
| **P0-1** | `FlowStepScope` `flow_unknown_emit` 拒收 `review.dimensions.complete`,review-synthesizer 第一次未激活,触发 `handoff_dispatch_timeout` 600s 后才二次激活(本应在 12:45:34 完成,实际拖到 13:02:32) | 编排(preset 拓扑) | E-1 / E-2 / E-4 / E-10 | 多次历史报告同模式,无独立 memory slug |
| **P1-1** | drift_monitor 同 retry_key flip Recovered/Pending 共 ≥ 14 次,无 outcome_history_length 阈值熔断 | 修复机制(recovery_runtime) | E-14 | `docs/report/...-20260629-032235-diagnosis.md` P1-2 同模式 |
| **P1-2** | coordinator 越权发 `human.guidance` 和 `loop.stalled` 各被 `semantic_gate_violation` 拒;`progress-steward` 才能发 `loop.stalled`,但本 loop 派发不及时 | 编排(preset hat_scope) | E-7 / E-8 | memory `ralph-emit-hat-channel-routing.md` 同模式 |
| **P1-3** | `recovery_outcome_update` flip storm + `handoff_dispatch_timeout` 600s 之间无前置熔断,等 600s 才触发 task.resume(target=review-synthesizer) | 修复机制 | E-4 / E-14 | `docs/report/...-20260629-072512-diagnosis.md` 同模式 |
| **P2-1** | review-synthesizer 在 L31 已收 task.resume(target=coordinator),但实际激活是 L39 的 stall_recovery 注入的 task.resume(target=review-synthesizer),**review-synthesizer.triggers = `[review.dimensions.complete]`**(schema 207L),对 `task.resume` 不敏感 | 编排(preset)+ agent | E-2 / E-4 / E-10 | `task-resume-target-hat-dead-path.md` 同模式 |
| **P2-2** | **review-synthesizer L39 + L40 4 秒间隔重复 emit review.complete(无新 trigger)** | **agent 行为** | E-10 | 无历史关联 |
| **P2-3** | coordinator 重发 work.ready(step-04) × 3 次,trigger 没及时接 | 编排 + 修复 | E-15 | 无历史关联 |

---

## 6. 修复建议(按优先级)

### 修复 #1(P0-1):`FlowStepScope` 接纳 `review.dimensions.complete` 不再误拒

**目标**:避免 review-synthesizer 第一次激活被 `flow_unknown_emit` 阻断,消除 600s `handoff_dispatch_timeout` 等待
**目标位置**:`crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs`
**当前行为**:`FlowStepScope` 在 iteration 27 把 `review.dimensions.complete` 标为 `flow_unknown_emit` 拒收(`safe_target=false`)
**具体改动**:
1. 把 `review.dimensions.complete` 加入 `FlowStepScope` 的 `flow_known_emits` 白名单(在 `unit_loop → review_walk` 切换时显式声明)
2. `FlowStepScope` 在 `current_step_id() == "review_walk"` + topic=`review.dimensions.complete` 时放行
3. 同步更新 `.cursor/rules/multi-hat-isolation.mdc` 与 `presets/schemas/ce-executor-serial.yml` `mechanism.flow` 段

**预期效果**:`review-synthesizer` 在 12:45:34 立刻激活(而非 600s 后),`review.complete` 在 12:45:50 内发出,fix-01 提前 17 分钟进入
**验证**:
```bash
cargo nextest run -p ralph-core -- flow_step_scope_review_walk
cargo nextest run -p ralph-core -- test_review_synthesizer_activates_after_dimensions_complete
./scripts/run-tests.sh
```

### 修复 #2(P1-1):`drift_monitor` 加 outcome_history_length 熔断阈值

**目标**:同 retry_key flip ≥ 3 次直接升级 `plan.blocked(reason=drift_monitor_flip_storm)`,不依赖 `finalize_recovery_outcome.rs` 全量变更
**目标位置**:`crates/ralph-core/src/recovery_runtime/`(已有 `dedupe_stall_recovery.rs`,在同模块加 `outcome_history` 跟踪)
**具体改动**:在 `drift_monitor.rs` 加 `per_retry_key_outcome_history: HashMap<String, Vec<RecoveryOutcome>>`,阈值 ≥ 3 时触发升级门
**预期效果**:r2-r14 的 14 次 flip 中,flip ≥ 3 时即停止反复 dispatch,改为单次 plan.blocked,recovery 不再空转
**验证**:`cargo nextest run -p ralph-core -- drift_monitor_flip_threshold`

### 修复 #3(P1-2):preset lint 强制 `coordinator.publishes` 不含 `human.guidance` / `loop.stalled`

**目标**:避免 coordinator 误发事件被 semantic_gate 拒
**目标位置**:`crates/ralph-core/src/preset_lint/hat_scope_invariant.rs`(已存在,需扩 allowlist)
**具体改动**:在 lint 规则中加 explicit-error:`coordinator.publishes` 含 `human.guidance` 或 `loop.stalled` → fail preset_lint
**预期效果**:preset authors 一开始就不能误声明,不再在 runtime 才被发现
**验证**:`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`

### 修复 #4(P1-3):增强已存在的 `finalize_recovery_outcome.rs`

**目标**:扩展已有 `finalize_recovery_outcome_on_flapping`(L15)以兜底 stall flip storm
**目标位置**:`crates/ralph-core/src/recovery_runtime/finalize_recovery_outcome.rs`(已存在,非"新增")
**具体改动**:在 `finalize_recovery_outcome_on_flapping` 内部加 `handoff_dispatch_timeout` 路径触发升级 `plan.blocked(reason=handoff_timeout_recovery_finalized)`,而不是让 600s timeout 一过 task.resume 又重发
**预期效果**:600s 后不再 task.resume(target=review-synthesizer),而是 plan.blocked 让 shipper + reporter 走正常失败链路
**验证**:`cargo nextest run -p ralph-core -- finalize_recovery_outcome_handoff_timeout`

### 修复 #5(P2-1):`review-synthesizer.triggers` 增 `task.resume`(替代 U6a 被 revert 的方案)

**目标**:让 review-synthesizer 对 task.resume 敏感,progress-steward 注入 task.resume(target=review-synthesizer) 可立刻激活
**目标位置**:`presets/en/ce-executor-serial.yml` review-synthesizer.triggers
**具体改动**:triggers 显式加 `task.resume`(已知 U6a 路径被 revert,这是替代方案)
**预期效果**:兜底延时不依赖 stall_recovery 600s,progress-steward 一发即激活

### 修复 #6(P2-2):synthesizer hat prompt 加重复 emit 防护

**目标**:避免 review-synthesizer L39+L40 4 秒内重复 emit review.complete
**目标位置**:`presets/en/ce-executor-serial.yml` `review-synthesizer.instructions` 段
**具体改动**:instructions 加 "在 emit review.complete 前,先检查 `RALPH_LAST_REVIEW_COMPLETE` env 或 `.ralph/scratchpad/synthesizer-last-emit.md` 是否 1 分钟内,若是则 skip"
**预期效果**:防止 synthesizer 二次无触发 emit

### 修复 #7(P2-3):`progress-steward`/`coordinator` 加去重 emit 防护

**目标**:避免 coordinator L12-L14 work.ready(step-04) 重复 emit
**目标位置**:`presets/en/ce-executor-serial.yml` `progress-steward.instructions` + `coordinator.instructions`
**具体改动**:emitter 加 idempotency_key 机制,1 分钟内同 step 的 work.ready 不重复

---

## 7. 直接回答用户问题(修订版)

> "编排机制有问题?修复机制失效?ralph 自身 bug?"

**三者皆有,比例约为 5 : 3 : 2**:

- **编排(50%)**:`FlowStepScope` 误拒 + coordinator 越权 + review-synthesizer triggers 不全 + coordinator 重复 emit — 都是 preset 拓扑缺陷
- **修复(30%)**:`drift_monitor` 无 flip 阈值 + `handoff_dispatch_timeout` 600s 太长 + 已有 `finalize_recovery_outcome.rs` 触达不到 handoff_timeout 路径
- **agent 行为(20%)**:`review-synthesizer` 二次 emit review.complete(主动发起,无 trigger) — 这是 LLM agent 收到 stall_recovery 注入的 task.resume 后**自发额外 emit** 一次,4 秒后再次 emit(总 × 2 次)

**关键修订**:前版把 P0-2 wiring 归为"未 commit"是错。**实际**:loop 在 commit `2ac23dea` 之前启动,这次 TaskWrongLoop 是修复前最后一次复现。`2ac23dea` 已合并,后续 loop 不再触发。前版"基座 10%"的归因需压缩到 ~0%(基座层面 wiring 已闭环)。

---

## 8. 推荐的下一步

1. **修复 #1(P0)** — 90 行 `flow_step_scope_stage.rs` 改动 + 1 个 BDD(`test_review_synthesizer_activates_after_dimensions_complete`,**必须 `run_workflow_guard_scenario`**),让 review-synthesizer 不再 600s 等待
2. **修复 #4(P1-3)** — 增强 `finalize_recovery_outcome.rs` 让 handoff_timeout 走终结
3. **修复 #2 / #3 / #5 / #6 / #7** — 5 个小补丁收敛 flip storm / 越权 / 重复 emit / triggers 缺失

**预计下一轮 loop 可以从 12:45:34 到 fix-01 仅用 5 分钟**(而非 13:08),把 60+ 分钟链路收敛到 20 分钟内。

---

## 9. 本次 run 实际成效(相对完整链路)

**好消息**(前版没强调):
- 4 个 step 全跑通,80 个 tests 全过
- 6 维 review 全过,findings 全部产出
- shipper 失败分支 + reporter + ralph 决策请求全链路打通(L33-L37)
- **`handoff_dispatch_timeout` 兜底机制有效**(r33 注入 task.resume 后 review-synthesizer 二次激活,fix-01 已落地,tests 80→89)

**坏消息**:
- `FlowStepScope` 误拒让 review-synthesizer 第一次等 17 分钟
- coordinator 越权发 `human.guidance` + `loop.stalled` 各被拒一次
- review-synthesizer 重复 emit review.complete × 2 次,触发 shipper 二次 fail verdict
- 最终 loop 未完全收敛(还在 fix-02 中),需等下一轮结果

---

**附录**:
- 关键文件:`presets/en/ce-executor-serial.yml`(L75-L2806)、`presets/schemas/ce-executor-serial.yml`(L59-L377)
- 关键源码:`crates/ralph-core/src/event_loop/{mod.rs, flow_step_scope_stage.rs, rejection.rs}`、`crates/ralph-core/src/event_loop/stages/coordinator_decision_gate.rs`、`crates/ralph-core/src/recovery_runtime/{mod.rs, finalize_recovery_outcome.rs, dedupe_stall_recovery.rs, retry_cap.rs}`、`crates/ralph-core/src/state_projector/task.rs:100-104`
- 关键 commit:`2ac23dea`(P0-2 wiring)、`c327d295`(LOOP_COMPLETE 不污染 terminal)、`76123d49`(task_id 空串 fail-closed)、`245fcc35`(dimension-reviewer hard reject)
- 历史方案:`docs/plans/2026-06-28-004`、`2026-06-28-005`、`2026-06-29-006`(recovery exhausted)、`2026-06-29-007`(mechanism p0/p1)
- Memory:`MEMORY.md` 中 `task-resume-target-hat-dead-path.md`、`ralph-emit-hat-channel-routing.md`、`payload-contract-preset-baseline.md`、`ce-executor-task-ownership.md`、`ce-executor-stale-activation-work-done-closure.md`

