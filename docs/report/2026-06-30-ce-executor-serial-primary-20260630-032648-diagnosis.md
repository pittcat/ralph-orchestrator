# RALPH 链路诊断报告 — primary-20260630-032648

> **run**: primary-20260630-032648（iter 41 completion_honored 已闭环）
> **preset**: ce-executor-serial（isolated mode，10-hat，2802 行）
> **plan**: `2026-06-20-001-feat-python-sort-algorithms`（2-UNIT 4-step + 2 fix-unit）
> **run_dir**: `/home/chaowen/Dev/agent_tools/ralph-e2e/`
> **诊断日期**: 2026-06-30

---

## 第 0 部分：结论摘要

**整体健康度**: ✅ **最终闭环成功，但执行链路在 fix-unit 终态处理路径上出现 4 条 critical + 4 条 major 偏离**。所有偏离被 runtime 兜底（stall_recovery / verdict_gate / completion_honored）+ shipper reason-based routing 自动升级为 pass 收口。可观测性与契约一致性被显著削弱。

- **最终交付**: ✅ commit `c6d67b5` 已落地，report.md / handoff.md / progress.md 三处一致显示 4 steps_closed + fix-01 + fix-02 closed + 52 tests_passed + P1 已修；loops.json 已清空
- **关键偏离数量**: P0×6, P1×3, P2×2（共 11 项, 4 critical / 4 major / 3 minor）
- **历史重复**: ✅ 是 — 模式 F (P-D1/P-M8/P-X1)、模式 C (P-M4)、模式 E (P-M7) 全部同源复现
- **闭环路径**: **非常规** — plan.blocked → shipper reason 白名单被 narrative 引导越界 → REVIEW_COMPLETE(pass)×2 → reporter report.done → LOOP_COMPLETE×3(completion_honored 仅 1 次) → commit c6d67b5 落地

---

## 第 1 部分：执行链路对比（流程还原）

### 1.1 Preset 预期

**预设来源**：`presets/en/ce-executor-serial.yml`(2802 行)

- **执行模式**：`execution_mode: isolated`(L171)，10-hat
- **完成承诺**：`completion_promise: LOOP_COMPLETE`，`required_events: ["report.done"]`，`max_iterations: 50`(L179-182)
- **核心机制**：`mechanism.flow.type: declared`，`terminal_emits: [LOOP_COMPLETE]`(L75-79)，4 段声明式步骤：`unit_loop → review_walk → plan_end → ship`
- **TDD 模式**：`complexity: small` 走 `test.passed(step-NN)` 路径；validator 是独立 hat

### 1.2 10-hat 拓扑 + 关键触发/发布

| Hat | triggers | publishes | 角色 |
|---|---|---|---|
| `coordinator` | `work.start, task.resume, test.passed, review.complete, work.failed` (L641) | `work.ready, review.start, plan.complete, plan.blocked, LOOP_COMPLETE` (L647) | 解析 plan、阶段门控、单元推进 |
| `executor` | `work.ready, fix.exhausted` (L1065) | `work.done, work.failed` (L1066) | TDD 实现 |
| `validator` | `work.done, fix.applied` (L1320) | `test.passed, test.failed` (L1321) | 全量测试 |
| `fixer` | `test.failed` (L2287) | `fix.applied, fix.exhausted` (L2288) | 诊断 + 修复（budget 10 轮） |
| `review-coordinator` | `review.start, review.dimension.done, review.dimension.failed` (L1402) | `review.dimension.ready, review.dimensions.complete` (L1403) | 6-dim 串行走读 |
| `dimension-reviewer` | `review.dimension.ready` (L1725) | `review.dimension.done, review.dimension.failed` (L1726) | 单维度评审 |
| `review-synthesizer` | `review.dimensions.complete` | `review.complete` | 聚合 findings |
| `shipper` | `plan.complete, plan.blocked` (L2403) | `REVIEW_COMPLETE` (L2404) | 最终校验 + plan 状态 + commit |
| `reporter` | `REVIEW_COMPLETE` (L2524) | `report.done, LOOP_COMPLETE` (L2525) | 输出 manager-facing 报告 |
| `progress-steward` | `loop.stalled` (L2716) | `work.ready, review.start, task.resume, plan.blocked` (L2718-2722) | loop-level 自愈兜底 |

### 1.3 预期终态事件链（plan 走通路径）

```
work.start
  └─ coordinator → work.ready(step-01)              [Unit Loop]
       └─ executor → work.done(step-01)
            └─ validator → test.passed(step-01)
                 └─ coordinator → work.ready(step-02) ... step-04
                      └─ coordinator → review.start   [PHASE GATE: last plan unit]
                           └─ review-coordinator → walk 6-dim
                                └─ review-synthesizer → review.complete (verdict=fail, fix_plan)
                                     └─ coordinator → work.ready(fix-01)  [PHASE GATE: now in fix-unit]
                                          └─ executor → work.done(fix-01)
                                               └─ validator → test.passed(fix-01)
                                                    └─ coordinator → work.ready(fix-02)
                                                         └─ coordinator → plan.complete (DO NOT emit review.start)  [Phase 2: fix-unit branch]
                                                              └─ shipper → REVIEW_COMPLETE
                                                                   └─ reporter → report.done
                                                                        └─ reporter → LOOP_COMPLETE
```

### 1.4 实际事件流（45 行 events.jsonl）

| 序 | 行 | topic | hat / source | 关键 payload | 备注 |
|---|---|---|---|---|---|
| 1 | 1 | `work.start` | loop-bootstrap | plan path payload | loop 启动 |
| 2 | 2 | `work.ready` | coordinator | step=step-01, task_id=`task-1782790083-0bfa` | U1 起步 |
| 3 | 3 | `work.done` | executor | step-01, commit=1, changed=578 | U1 完成 |
| 4 | 4 | `test.passed` | validator | step-01, 18/18 | |
| 5 | 5 | `work.ready` | coordinator | step-02 | U1 → U2 |
| 6 | 6 | `work.done` | executor | step-02, commit=1, 204 lines | |
| 7 | 7 | `test.passed` | validator | 26/26 | |
| 8 | 8 | `work.ready` | coordinator | step-03 | U2 → U3 |
| 9 | 9 | `work.done` | executor | step-03, 137 lines | |
| 10 | 10 | `test.passed` | validator | 34/34 | |
| 11 | 11 | `work.ready` | coordinator | step-04 | U3 → U4 |
| 12 | 12 | `work.done` | executor | step-04, 334 lines | |
| 13 | 13 | `test.passed` | validator | 52/52 | 全 52 测试通过 |
| 14 | 14 | `review.start` | coordinator | total_units=4, unit_index=4 | Branch B pre-decision gate |
| 15-26 | 15-26 | `review.dimension.{ready,done}` | review-coordinator + dimension-reviewer | 6-dim walk：goal-alignment / correctness / testing(P2×2 P3×2) / maintainability / project-standards(**P1×1**) / adversarial | P1=README 路径 |
| 27 | 27 | `review.dimensions.complete` | review-coordinator | fix_round=0, 6 dim done | 第一轮 walk 收口 |
| 28 | 28 | `review.complete` | review-synthesizer | verdict=fail, findings=2, residual=1, fix_plan_file=... | 触发 fix-unit |
| 29 | 29 | `work.ready` | coordinator | step=fix-01, fix_plan_file 携带 | 进入 fix-unit flow |
| 30 | 30 | `work.done` | executor | fix-01, commit=1, changed=11 | README pytest 路径已修 |
| 31 | 31 | `test.passed` | validator | 52/52 | |
| 32 | 32 | `work.ready` | coordinator | step=fix-02 | 推进到 fix-02 |
| 33 | 33 | `work.done` | executor | **fix-02, commit=0, changed=0** | **疑点：0 commit, 0 changed** |
| 34 | 34 | `test.passed` | validator | 52/52 | |
| 35 | 35 | **`review.start`** | coordinator, triggered=ralph | total_units=4, task_id=step-04 task_id | **疑点 A: fix-unit 完成后违规 review.start** |
| 36 | 36 | `review.dimension.ready` | review-coordinator, triggered=ralph | dim=goal-alignment | **疑点 B: 第二轮 review 启动** |
| 37 | 37 | `LOOP_COMPLETE` | ralph | reason="all steps completed..." | **疑点 C: ralph 抢发** |
| 38 | 38 | `task.resume` | progress-steward | kind=review_sequence_not_advanced, target=coordinator | stall 兜底 |
| 39 | 39 | `plan.blocked` | coordinator | reason="stall_no_events recovery ... step_handoff::task_not_found and plan_gate_review_not_terminal" | **疑点 G: plan.complete 降级为 plan.blocked** |
| 40 | 40 | `work.ready` | coordinator | step=fix-02, complexity=trivial | plan_path 回退到 plan.md |
| 41 | 41 | `REVIEW_COMPLETE` | shipper | pass_or_fail=pass, verdict=pass | shipper reason 升级 pass |
| 42 | 42 | `REVIEW_COMPLETE` | shipper | **与 L41 字节级相同** | **疑点 D: REVIEW_COMPLETE 重复** |
| 43 | 43 | `report.done` | reporter | report_path=...report.md | 报告输出 |
| 44 | 44 | `LOOP_COMPLETE` | reporter | all_steps_completed | reporter 自发 |
| 45 | 45 | `LOOP_COMPLETE` | ralph | commit=c6d67b5, status=completed | ralph 终态 record |

### 1.5 链路对比图（✅/❌/⏸️）

| 步 | 预期 | 实际 | 状态 |
|---|---|---|---|
| L1-L13 | 4-step work.ready/work.done/test.passed 串联 | 完全按序，commit_count/changed_lines 正常（578/204/137/334），测试 18→26→34→52 逐级过 | ✅ |
| L14 | coordinator → review.start (last plan unit) | 正常发出 | ✅ |
| L15-L27 | review-coordinator 6-dim walk | 6 维全收敛，aggregator 收到 `review.dimensions.complete(fix_round=0)`。P1×1 + P2 多条 | ✅ |
| L28 | review.complete(verdict=fail, fix_plan) | `verdict=fail, findings=2, residual=1` | ✅ |
| L29-L31 | fix-01 work.ready → work.done → test.passed | 全流程顺次 | ✅ |
| L32-L34 | fix-02 work.ready → work.done(commit=0, changed=0) → test.passed | ⚠️ work.done payload 异常（commit=0），但 validator 通过 |
| L35 | **plan.complete**（Branch A last fix-unit） | ❌ 第二次 `review.start`（coordinator triggered=ralph） | ❌ |
| L36 | 不应发生 | ❌ `review.dimension.ready(goal-alignment)`；ledger 同步 2 次 `duplicate_work_done` 拒 | ❌ |
| L37 | 不应发生 | ❌ ralph 直接发 LOOP_COMPLETE；ledger L40 拒（缺 report.done） | ❌ |
| L38 | 兜底 | progress-steward → task.resume(target=coordinator) | ⏸️ |
| L39 | coordinator → plan.complete | ❌ 改为发 **`plan.blocked`**（plan_gate_review_not_terminal + step_handoff::task_not_found 拦） | ❌ |
| L40 | 兜底 | coordinator → work.ready(fix-02, trivial) | ⏸️ |
| L41 | 应 hard-fail（reason 不在 shipper 白名单） | ❌ shipper 把 `stall_no_events recovery` 当 recoverable 升级 pass | ❌ |
| L42 | 不应发生 | ❌ **payload 与 L41 字节级相同** | ❌ |
| L43 | reporter → report.done | ✅ reporter verdict_gate dedup 接住 1 次 | ✅ |
| L44 | reporter → LOOP_COMPLETE | ✅ reporter 自发 | ✅ |
| L45 | ralph 写终态 LOOP_COMPLETE | ✅ ralph → LOOP_COMPLETE(commit=c6d67b5) | ✅ |

### 1.6 关键观察（局部异常征兆）

**闭环状态**：✅ iter 41 `completion_honored` 闭环（ledger.jsonl:47），最终 commit `c6d67b5` 落地，三处文档一致。

**终态事件次数**（与 preset single-emit 声明不符）：
- `plan.complete`: **0 次**（应为 1 次）
- `plan.blocked`: 1 次（替代 plan.complete）
- `REVIEW_COMPLETE`: **2 次**（应为 1 次，duplicate）
- `report.done`: 1 次
- `LOOP_COMPLETE`(events 流): **3 次**（L37 被拒 + L44 reporter + L45 ralph）
- `LOOP_COMPLETE`(honored): **1 次**（completion_honored）

**recovery.jsonl 关键记录**：28 行 envelope 中 `plan.complete` 修复记录 13 次（L5-21），系统在 fix-unit 完成后反复尝试 emit `plan.complete` 但落入 repair sink。

**孤儿任务**：tasks.jsonl L5 `task-1782792589-4705`（key=null, started_at=null, closed=04:20:35）—— 与 fix-01 内容相关但无 event 引用。

---

## 第 2 部分：历史上下文关联

扫描全部 docs/{plans, brainstorms, reviews, report, achieved, solutions}（35 份 report + 18 solutions + 60+ achieved）+ MCP memory 13 条。

### 2.1 历史问题清单（精选 38 条按类型）

**1.1 preset 设计与配置缺陷（13 条 P-D1~P-D13）**：
- 关键 4 条本次相关：
  - **P-D1** `plan.complete` payload 缺 `step` 字段（本次同源）
  - **P-D7** task placeholder 占位符污染（tasks.jsonl L5 孤儿任务同源）
  - **P-D9** scratchpad 路径互斥（summary_writer vs preset）
  - **P-D12** `test.passed` 缺 commit_count/changed_lines 字段

**1.2 Ralph Loop 机制缺陷（20 条 P-M1~P-M20）**：
- 关键 6 条本次相关：
  - **P-M1** `task.resume` 被 `topic_denied` 二次过滤
  - **P-M4** `TaskStore::close_by_key` 误关闭未 started 任务（本次直接复现）
  - **P-M5** `TaskStore::ensure_task` 未去重
  - **P-M7** `consecutive_failures` 双计数器（本次直接复现）
  - **P-M8** `plan_gate_review_not_terminal` 在 fix-unit 末段拦截 `plan.complete`（同源）
  - **P-M10** U9.5 verdict_gate 与 shipper 语义错位

**1.3 agent 输出问题（11 条 P-A1~P-A11）**：
- **P-A11** task closed 时间早于 work.done emit（fix-02 payload commit=0 与 c6d67b5 时序错位同类）

**1.4 多因素叠加（11 条 P-X1~P-X11）**：
- **P-X1** plan-gate → executor dispatch gap（本次主链路同源）
- **P-X5** stash/fix-log 路径互斥（本次未触发）
- **P-X7** event_loop step close（本次半完成状态同源）

### 2.2 与本次 run 高度相关的 8 条模式对账

| 模式 ID | 历史模式 | 本次复现 | 证据 |
|---|---|---|---|
| 🔴 **A** | task.resume topic_denied (P-M1) | ❌ 否 | events L38 成功写入；ledger 无 topic_denied |
| 🔴 **B** | fix-unit task 双条投影 + 21s 重发 (P-M5/P-M13) | 🟠 部分 | tasks.jsonl L5 占位符污染复现；双条投影未触发 |
| 🔴 **C** | `TaskStore::close_by_key` 误关闭未 started 任务 (P-M4) | 🔴 **是** | tasks.jsonl L5 `key=null, started_at=null, closed=04:20:35`；源码 `task_store.rs:452-459` 缺守卫 |
| 🔴 **D** | fix-plan 流向 vs summary 路径互斥 (P-D9/P-X5) | ❌ 否 | summary.md 正常生成，scratchpad 路径一致 |
| 🔴 **E** | `consecutive_failures` 双计数器 (P-M7) | 🔴 **是** | ledger seq-37 (no_progress, 04:27:42) 与 seq-39 (main, 04:30:06) 错位 |
| 🔴 **F** | plan.complete payload + plan_gate 误拒 (P-D1/P-M8/P-X1) | 🔴 **是（同源）** | events L39 字面 `plan_gate_review_not_terminal`；recovery.jsonl 13 次 plan.complete repair |
| 🟡 **G** | recovery.jsonl envelope 口径不一致 (P-M9/P-X11) | 🟡 是 | recovery.jsonl 全 `repair_dispatch`，未分 rejected vs routed |
| 🟢 **H** | human.guidance 自观测循环 (P-X8) | 🟢 否（已闭环） | events+recovery 全 grep 0 次 |

**结论**: 8 条中 5 条同源复现（B/F/E/C/G 中 4 critical + 1 minor），3 条未触发（A/D/H）。本次 run 与最近的 `170451` run 属同一链路断链，但失效点不同。

### 2.3 已加固但本次仍复现的薄弱点

- **P-M4** `close_by_key` 缺 started 守卫：`23dcfdaf` 加 `close_by_key` 但未加 `started:null` 守卫；本次复现
- **P-M7** 双计数器：部分加固（stall_recovery_counts 字段）；ledger 口径仍不一致
- **P-M8** plan_gate 豁免条件：U8 仅声明未接入；fix-* 路径被误拒
- **P-X1** dispatch gap 三件套（plan_gate + allowed_topics + recovery 终止）：单修任何一条都不够

---

## 第 3 部分：偏离证据清单

**来源**：events.jsonl + ledger.jsonl + recovery.jsonl + tasks.jsonl + preset + schema 全文交叉验证。

### 3.1 偏离统计

| 严重度 | 数量 | 是否影响最终闭环 |
|---|---|---|
| **critical** | 4 | 是（其中 1 个被 runtime 兜底、3 个被 shipper/reporter 升级为 pass） |
| **major** | 4 | 否（仅可观测性弱化） |
| **minor** | 3 | 否 |
| **总计** | **11** | — |

### 3.2 逐项偏离证据

#### DE-001: fix-02 后违规 emit 第二次 `review.start`（critical）

- **现象**：events L35（04:22:12）`review.start`，hat=coordinator，triggered=ralph，total_units=4, task_id 引用 step-04
- **预期**：preset L823-828 "DO NOT emit review.start when step starts with fix-"；L840-855 Branch A last fix-unit "Stop — do NOT emit review.start. A fix-unit's test.passed is the end-of-plan signal"
- **历史模式**：B-P-X1 + P-D1 + P-M8 复合
- **疑点归类**：coordinator 走错 Phase Gate 分支（应 Branch A last fix-unit → plan.complete，实际 Branch B last plan unit → review.start）；triggered=ralph 表明是 ralph 注入重试

#### DE-002: fix-02 work.done commit_count=0 / changed_lines=0（major）

- **现象**：events L33（04:18:01）`work.done(step=fix-02, commit_count=0, changed_lines=0)`
- **预期**：preset L1086-1095 PAYLOAD SCHEMA CHECKLIST（commit_count 必填 ≥0）
- **疑点归类**：executor 提交了 plan.md 但未在 work.done 前做 git commit（或 commit 后未重算 commit_count/changed_lines）；时序上 commit c6d67b5 在 events L33 后 22 分钟才出现
- **历史模式**：P-A11 同类

#### DE-003: plan.complete 全程未发出（critical）

- **现象**：events L39 改为发 `plan.blocked`，reason 字面包含"attempting to emit plan.complete but blocked by policy gates: step_handoff::task_not_found and plan_gate_review_not_terminal"；recovery.jsonl 13 次 repair-stream event 记录
- **预期**：preset L844-855 Branch A last fix-unit 应发 `plan.complete`（required_fields: plan_name, completed_steps, task_id, task_key, verdict, final_findings_count, fix_round）
- **疑点归类**：payload 缺字段 + plan_gate 拒绝（step_handoff::task_not_found 暗示 task 已不存在）+ 降级 plan.blocked
- **历史模式**：P-D1 + P-M8 + P-X1 同源

#### DE-004: LOOP_COMPLETE 在 review 起步阶段被抢发 3 次（critical）

- **现象**：events L37（ralph, 04:29:56，review 仅走 1 个 dim 后抢发），L44（reporter, 04:42:57），L45（ralph, 04:43:32，commit=c6d67b5）
- **预期**：preset L79 `terminal_emits: [LOOP_COMPLETE]` 单数；L180 `required_events: ["report.done"]` 顺序；L350-352 "after LOOP_COMPLETE is honored, the loop MUST stay quiet"
- **runtime 拒绝**：ledger L40（iter 36）`LOOP_COMPLETE rejected: missing required events: ["report.done"]`；最终 ledger L45-47（iter 41）`completion_honored` 仅 1 次
- **疑点归类**：L37 抢发违契约；L44 与 L45 重复，单 producer 声明违规
- **历史模式**：P-M10 + P-X7

#### DE-005: tasks.jsonl L5 孤儿任务（critical）

- **现象**：tasks.jsonl L5 `task-1782792589-4705`, title="Fix README pytest command paths", key=null, owner=coordinator, created=04:09:49, closed=04:20:35
- **预期**：preset L259-279 `execution_contracts.work.done.require_task`：必须有 task_id + task_key
- **事件流对照**：events 全 45 行 grep `1782792589` 无任何 work.ready/work.done/test.passed 引用
- **疑点归类**：coordinator 在 fix-01 投递期间手工创建占位任务，未被任何 event 引用——与历史 placeholder 污染同源
- **历史模式**：P-D7 占位符 + P-M4 close_by_key 缺守卫

#### DE-006: REVIEW_COMPLETE 重复 2 次（major）

- **现象**：events L41 / L42（时间差 29s，payload 字节级相同）
- **预期**：preset L2404 shipper `publishes: ["REVIEW_COMPLETE"]` 单次；L350-352 `duplicate_terminal: reject`
- **疑点归类**：reporter verdict_gate 兜住 1 次但 events 流污染；shipper 的 `triggered=ralph` 表明是 ralph 二次注入
- **历史模式**：P-X7 同源

#### DE-007: progress.md 未记录 plan.complete 终态（minor）

- **现象**：progress.md L4 `Current Step: fix-02`，L6-12 `Completed Steps` 含 step-01..step-04 + fix-01 + fix-02，但**没有 plan.complete 终态记录**
- **疑点归类**：projector 写 progress.md 但 `plan.blocked` 未走 projector 路径更新 progress.md 的 current_step
- **历史模式**：P-A9 口径不一致同类

#### DE-008: report.done 缺 verdict 字段（minor）

- **现象**：events L43 payload 含 `awaiting_decision: false, pass_or_fail: "pass", report_path: ...` —— **缺 `verdict`**
- **预期**：preset L2673 HARD RULE "Every report.done payload MUST mirror pass_or_fail (and verdict when present) from the upstream REVIEW_COMPLETE"
- **疑点归类**：reporter 漏掉 verdict 镜像——runtime 未拦截（schema required_fields 只硬要求 2 个字段）
- **历史模式**：—

#### DE-009: ledger iter 35 no_progress 与 events 流不一致（minor）

- **现象**：ledger.jsonl L37（iter 35）`loop.batch_sync.no_progress` counter=35；与 summary.md 显示 41 iter 错位
- **疑点归类**：`consecutive_failures` 与 `consecutive_no_progress_turns` 双轨计数器独立路径
- **历史模式**：P-M7 已识别未闭环

#### DE-010: coordinator 越权 emit loop.stalled 被 isolated scope 拒绝（major）

- **现象**：recovery.jsonl L23 envelope 含 `isolated scope violation: hat 'coordinator' is not allowed to publish topic 'loop.stalled'`
- **预期**：preset L647 coordinator `publishes: [work.ready, review.start, plan.complete, plan.blocked, LOOP_COMPLETE]` —— **不含 `loop.stalled`**
- **疑点归类**：coordinator 走 plan-gate 兜底时尝试发 `loop.stalled` 触发 progress-steward，但被 isolated scope 拒绝
- **历史模式**：P-M19 + P-X1

#### DE-011: 5 个 critical 偏离同源根因（critical）

- **现象**：DE-001/002/003/004/005 在 fix-02 完成（L34, 04:19:45）之后涌现
- **共同特征**：fix-02 worker.done commit_count=0/changed_lines=0 → test.passed → 违反 Phase Gate 走 review.start 而非 plan.complete
- **历史模式**：B-P-X1（plan-gate → executor dispatch gap），本次 fix-02 闭链断链与 noble-peacock / 153653 / 170451 同源

### 3.3 偏离分类（preset / loop / agent / 叠加）

**preset 缺陷导致**：DE-003（plan.complete payload Rust enforcement 缺）, DE-008（report.done schema 与 preset 不对齐）, DE-010（coordinator publishes 不含 loop.stalled）

**loop 机制导致**：DE-001（Phase Gate 无 Rust 强）, DE-002（trivial step 豁免被绕过）, DE-004（terminal_emits 单数未生效）, DE-006（duplicate_terminal policy 未生效）, DE-009（双计数器）

**agent 输出导致**：DE-001（coordinator 走错分支）, DE-002（executor commit 后未重算）, DE-005（coordinator 创建占位任务）, DE-008（reporter 漏 verdict）, DE-011（5 critical 复合触发源）

**多因素叠加**：DE-003（preset 缺 enforcement + agent 漏填 + runtime 降级）, DE-004（preset 单数 + policy 未生效 + agent 抢发）, DE-007（projector 写空 + plan.md 未更新）, DE-011（同上）

### 3.4 Agent B 8 条模式映射

| 模式 | 复现 | 偏离 |
|---|---|---|
| A task.resume topic_denied | ❌ 否 | — |
| B fix-unit 双条投影 + 21s 重发 | 🟠 部分 | DE-005 |
| C close_by_key 误关闭未 started | 🔴 是 | DE-005 |
| D fix-plan/summary 路径互斥 | ❌ 否 | — |
| E 双计数器口径不一致 | 🔴 是 | DE-009 |
| F plan.complete payload + plan_gate | 🔴 是 | DE-003 |
| G recovery.jsonl 口径不一致 | 🟡 是 | DE-010 |
| H human.guidance 自观测循环 | 🟢 否 | 已闭环 |

---

## 第 4 部分：偏离归因 + P0/P1/P2 表

### 4.1 归因表

| 优先级 | 编号 | 问题 | 根因分类 | 证据 | 历史关联 |
|---|---|---|---|---|---|
| **P0-1** | C-A + B-A | fix-unit `test.passed` 后被注入 `review.start`(events L35)，违反 preset L826-828 硬规则 | **preset 设计 + Ralph Loop 机制叠加** | events L35；preset L826-828, L2758-2764；recovery.jsonl L11/L24 | 模式 A(P-M1)+ 模式 F(P-D1/P-M8/P-X1)+ P-X7 |
| **P0-2** | C-G | shipper 把 `stall_no_events recovery` 判定为 `pass_or_fail: pass, verdict: pass`(events L41 字面 `"with recoverable reason stall_no_events recovery"`)，违反 preset L2494-2497 严格白名单 | **preset 设计 vs agent 执行叠加** | events L41 payload 字面；preset L2494-2509；recovery.jsonl L23 | 模式 F(P-D1)+ 模式 G(P-M9) |
| **P0-3** | C-D | `REVIEW_COMPLETE` 同 payload 发 2 次(events L41 / L42 payload diff = 0)，reporter 内部 dedup 1 次但 events 流污染 | **Ralph Loop 机制**(无 topic-uniqueness 兜底)+ **agent 执行**(shipper 二次跑) | events L41 / L42；presets/schemas/ce-executor-serial.yml:306-312 仅声明 required_fields；verdict_gate_stage.rs:30 仅 terminal_emits=[LOOP_COMPLETE] | 模式 A 连锁 |
| **P0-4** | C-E | `TaskStore::close_by_key` 在 `started: null` 时直接 closed，tasks.jsonl L5 孤儿任务(`key=null, owner_hat_id=coordinator`) | **Ralph Loop 机制**(task_store 守卫缺失) | tasks.jsonl L5；源码 `task_store.rs:452-459`；recovery.jsonl L5-9 显示 coordinator 一连 emit 13 次 `plan.complete` | **P-M4 部分加固未闭环**(23dcfdaf 加 `close_by_key` 未加 `started:null` 守卫) |
| **P0-5** | C-F | `ralph` 在 iter 36 (4:29:56) 抢先发 LOOP_COMPLETE（review chain 起步阶段，缺 report.done），ledger L40 拒(`missing required events: ["report.done"]`)，但 events.jsonl:37 已写盘 | **Ralph Loop 机制**(completion_requested 无 guard) + **preset 设计**(coordinator L647 publishes 含 LOOP_COMPLETE + L649 exempt_topics) | events L37 hat=ralph source=ralph；ledger L40；recovery.jsonl L24 `coordinator → LOOP_COMPLETE` repair_dispatch；preset L647 / L649 / L651 | 模式 E(P-M7)+ 模式 F(P-X1) |
| **P0-6** | C-C | `consecutive_failures` 与 `consecutive_no_progress_turns` 双计数器错位。ledger iter 35 sequence 37 出现 `loop.batch_sync.no_progress`(counter=35)，iter 36 sequence 39 出现 `loop.batch_sync`(counter=36)，35→36 跨双计数器 iter 计数错位 | **Ralph Loop 机制**(loop_state 双轨) | ledger L37 seq-37 vs L39 seq-39；源码 `loop_state.rs:417` `stall_recovery_counts: HashMap<String,u32>` | 模式 E(P-M7) 已识别未闭环 |
| **P1-1** | C-G2 | plan.blocked payload `reason: "stall_no_events recovery: ..."` 走兜底而非真实 plan.complete payload | **preset 设计** | events L39；presets/schemas L264-277 required_fields | 模式 F(P-D1 / P-X1) |
| **P1-2** | C-F2 | fix-02 `executor → work.done(commit_count=0, changed_lines=0)` 与后续 commit `c6d67b5` 时序错位：L33 04:18:01(commit=0) vs c6d67b5 出现在 report.md 与 events L41+04:40+ | **agent 执行** | events L33；report.md:44；events L41 payload 提及 `c6d67b5` | (无直接历史) |
| **P1-3** | C-H | tasks.jsonl L5 孤儿 task 没有 task_key，coordinator 在 fix-01 投递期间创建 task 又不通过 work.ready 引用 | **Ralph Loop 机制**+ **agent 执行** | tasks.jsonl L5 全字段；preset L715-731；`task_store.rs:498-589` ensure 未去 key=null | P0-4 同源 |
| **P2-1** | C-诊断 | recovery.jsonl 与 ledger 口径不一致：同 sequence 段(recovery L11 / L17 / L24 / L27-28 共 5 次 LOOP_COMPLETE repair-stream 但 ledger L40 仅记 1 次) | **Ralph Loop 机制**(repair_sink 没分桶) | recovery.jsonl L11/L17/L24/L27-28；ledger L40 | 模式 G(P-M9 / P-X11) |
| **P2-2** | C-诊断 | ralph 'triggered=ralph' 语义被稀释，几乎所有 hat emit 都带该字段 | **Ralph Loop 机制** | events L2-L45 大量 `"triggered":"ralph"` | (无直接历史) |

### 4.2 关键 P0 深度归因

#### P0-1: fix-unit test.passed 后 progress-steward 注入 review.start

**根因（机制级）**: **Ralph Loop 机制**（progress-steward 状态机 + recovery routing）+ **preset 设计**（coordinator PHASE 2 决策表与 progress-steward 自愈表语义错位）叠加。

- progress-steward 把"fix-unit 完成 + coordinator 没发 plan.complete"和"plan-unit 完成 + review sequence 没 close"用同一逻辑分支 `task.resume(target_hat=coordinator)` 处理（preset:2761-2764）
- coordinator 的 phase gate 决策（L815-829）依赖 `step` 字段前缀；progress-steward 注入 task.resume 时**不携带原始 fix-unit 的 step 上下文**
- L35 payload 的 `task_id=task-1782790995-b649`（step-04 的 task_id，不是 fix-02）— coordinator 从 scratchpad/progress.md 重读时误判 phase
- 源码：`review_step_state.rs:329-331` 修复了 fix-* 时 plan_gate 放行 `plan.complete`，但 L35 走的是 `review.start` 不是 `plan.complete`
- 这一类判定错配历史上 2026-06-24 P0-A 回归、2026-06-29 153653 P0-1 都出现过

#### P0-2: shipper reason 越界升级为 pass

**根因**: **preset 设计**（shipper prompt narrative 描述 + reason 白名单）+ **agent 执行**（shipper 不严格白名单检查）叠加。

- preset L2494-2498 字面要求 recoverable reasons 必须是 `[loop_stalled_max_iterations, steward_escalation, review_terminal_drift]` 三个之一
- events L39 reason 字面带 `stall_no_events recovery`，shipper 看到 "recoverable reason" 字面前缀就 routing 为 pass
- L2508 "any other reason not listed as recoverable → hard-fail" 规则未执行

#### P0-3: REVIEW_COMPLETE 同 payload 发 2 次

**根因**: **Ralph Loop 机制**（top-level `completion_honored` 仅覆盖 loop.complete）+ **agent 执行**（shipper prompt 没有强制 "emit REVIEW_COMPLETE exactly once"）。

- preset `schemas/ce-executor-serial.yml:306-312` 仅声明 `required_fields`，无 uniqueness
- `verdict_gate_stage.rs:30` `DEFAULT_TERMINAL_EMITS = &["LOOP_COMPLETE"]`，REVIEW_COMPLETE 不是 terminal
- reporter 通过 `verdict_gate` 内部 dedup 接住 1 次，但 events 流层面 2 次都被记录

#### P0-4: close_by_key 误关闭未 started 任务

**根因**: **Ralph Loop 机制**（task_store 守卫缺失）+ **agent 执行**（coordinator 临时创建占位任务）。

源码 `task_store.rs:452-459`:
```rust
pub fn close_by_key(&mut self, key: &str) -> Option<&Task> {
    if let Some(task) = self.get_by_key_mut(key) {
        task.status = TaskStatus::Closed;
        task.closed = Some(chrono::Utc::now().to_rfc3339());
        return self.get_by_key(key);
    }
    None
}
```
**无 `started: null` 守卫**，直接覆盖 status。Agent B 模式 C（P-M4）的部分加固 `23dcfdaf` 加 `close_by_key` 但**未**加 `started:null` 守卫。

推断 close 触发：`state_projector/progress.rs:117-128` 的 `project_plan_complete` 函数在 "close every still-open task" 时调 `store.close(&task.id)`，task L5 `started_at=null` 直接被 close。

#### P0-5: ralph 提前发 LOOP_COMPLETE 在 report.done 之前

**根因**: **Ralph Loop 机制**（`completion_requested` 在 review chain 未收敛时被置位）+ **preset 设计**（coordinator L647 publishes 含 LOOP_COMPLETE + L649 exempt_topics 让其绕 schema）+ **agent 执行**（coordinator prompt narrative "P1 已修, plan.complete policy 拦了" 引导）。

- events L37 hat=ralph source=ralph（loop runner 自身发的）
- ledger seq 38: `loop.completion_requested` 在 iter 36 04:30:06（早于 events L37）
- preset L647-651: coordinator 既允许 `plan.complete` 又允许 `LOOP_COMPLETE`，exempt_topics 让其绕过 schema
- recovery.jsonl L24 字面 "All plan units and fix-units completed. P1 findings resolved. Plan cannot emit plan.complete due to policy gates." —— coordinator 实际 prompt 给出的 LOOP_COMPLETE 路径

#### P0-6: consecutive_failures / consecutive_no_progress_turns 双计数器错位

**根因**: **Ralph Loop 机制**（loop_state.rs 双轨）。

- 源码：`loop_state.rs:334` `consecutive_failures` 与 `:698` `consecutive_no_progress_turns` 独立
- `:417` `stall_recovery_counts: HashMap<String, u32>` 仅记 stall counter
- agent summary.md 显示 41 iter 与 ledger no_progress 28 与 main 41 错位

---

## 第 5 部分：修复建议

### 5.1 P0 修复（必修，按依赖排序）

#### P0-4：task_store 守卫加 `started:null` 检查

**目标文件**：`crates/ralph-core/src/task_store.rs:452-459`

```rust
pub fn close_by_key(&mut self, key: &str) -> Option<&Task> {
    if let Some(task) = self.get_by_key_mut(key) {
        // 2026-06-30 P0-4 fix: do not close a task that
        // was never started (`started_at == None`).
        if task.started_at.is_none() {
            tracing::warn!(
                task_id = %task.id, task_key = %key,
                "P0-4: refusing to close_by_key on a started=null task; \
                 projector / recovery path should call ensure_task deletion instead"
            );
            return None;
        }
        task.status = TaskStatus::Closed;
        task.closed = Some(chrono::Utc::now().to_rfc3339());
        return self.get_by_key(key);
    }
    None
}
```

**同步改**：`crates/ralph-core/src/state_projector/progress.rs:117-128` 的 `project_plan_complete` close 循环加 `task.started_at.is_some()`

**预期效果**：tasks.jsonl L5 不再出现 `started=null, closed` 孤儿任务；validator `open_tasks` view 一致

**风险**：`started_at=None` 的合法用法（coordinator 在 work.start 路径上预创建 task）—— 修复点只 close_by_key 加 null check，不动 close(id)

**历史参考**：Agent B 模式 C（P-M4）已加固未闭环——`23dcfdaf` 加 `close_by_key` 但**未**加 started 守卫；plan `2026-06-30-002` P0-1 已规划未上线

#### P0-6：双计数器合并为 `consecutive_stall_turns`

**目标文件**：`crates/ralph-core/src/event_loop/loop_state.rs:334, :698`

合并到同一 `consecutive_stall_turns: HashMap<String, u32>`，no_progress 与 failures 都 self-bump；在 ledger 输出加 `counter_kind` 字段

**预期效果**：ledger 末条与 summary 显示 iter 一致；不再出现 seq-37 (no_progress) vs seq-39 (main) 错位

**风险**：合并影响 `consecutive_failures` 终止门（默认 5 触发 loop fail），需 regression test

#### P0-5：completion_requested 加 report.done guard

**目标文件**：`crates/ralph-core/src/event_loop/loop_state.rs:213-214`

```rust
pub fn mark_completion_requested(&mut self) -> Result<(), String> {
    if !self.report_done_seen {
        return Err(
            "completion_requested rejected: report.done has not been \
             observed yet; ralph runner must wait for the reporter".into()
        );
    }
    self.completion_requested = true;
    Ok(())
}
```

**同步加** `report_done_seen: bool` 字段，在 observe_accepted 接到 `report.done` 事件时置 true

**预期效果**：ralph runner 在 fix-02 → plan.blocked → stall recovery 路径不再抢先发 LOOP_COMPLETE；ledger seq-38 不应再出现 `loop.completion_requested` 在 iter 36

**风险**：`completion_requested` 是 loop 内部状态，UI/CLI 探针可能依赖；需在 `ralph diagnose --loop-state` 显式说明 guard 触发原因

#### P0-1：progress-steward 状态机分桶（fix-unit 完成独立行）

**目标文件 1**：`presets/en/ce-executor-serial.yml:2758-2764`（progress-steward 表）

新增独立行（放在 `review_sequence_not_advanced` 行之前）：
```yaml
- kind: fix_unit_complete_plan_complete_pending
  when: "all fix-units closed in tasks.jsonl AND coordinator never emitted plan.complete"
  emit: "task.resume(target_hat=coordinator, reason=fix_unit_complete_plan_complete_pending)"
  note: "this nudges coordinator to re-emit plan.complete, NOT review.start"
```

**目标文件 2**：`presets/en/ce-executor-serial.yml:815-829`（coordinator PHASE GATE 表）

在 fix-NN 分支追加："If completed_steps contains fix-* AND plan.complete not yet emitted, MUST emit plan.complete, NOT review.start"

**预期效果**：fix-02 test.passed 后 coordinator 不再被 progress-steward 误注入 review.start；events L35 不再出现；plan.complete 在 L34 后直接发

**风险**：新增行与 `review_sequence_not_advanced` 路径可能冲突，需反 set 测

**历史重复**：🔴 模式 F（P-D1/P-M8/P-X1），`docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md` U3.4/U3.5 + mechanism-foundation U9.5 已规划但**未落地**

#### P1-1：plan.complete step 字段 backfill（强依赖 P0-1）

**目标文件**：`crates/ralph-core/src/event_loop/review_step_state.rs:305-353`

**修改**：coordinator emit plan.complete 时若 step 字段为 null，先从 progress.md current_step 读取 `fix-02` 再 emit；plan_gate fail-close 返回友好 hint

**预期效果**：修了 P0-1 后本条自动消失；单独加 lint 检测 step=null emit plan.complete

#### P0-2：shipper reason 严格白名单

**目标文件**：`presets/en/ce-executor-serial.yml:2491-2498`

```yaml
# On plan.blocked: (2026-06-30 P0-2 STRICT-MATCH)
#   STRICT-MATCH whitelist (exact string equality):
#     ["loop_stalled_max_iterations", "steward_escalation", "review_terminal_drift"]
#   For ANY other reason (string match ANYWHERE, regex, prefix, suffix) MUST hard-fail.
#   Do NOT treat progress-steward's payload phrase "with recoverable reason X" as authoritative.
```

**同步加 lint**：`crates/ralph-core/src/preset_lint/strict_reason_routing.rs`，检测 shipper prompt 里是否含 "strict"/"exact match"，缺失告警

**预期效果**：shipper 对 plan.blocked(reason=stall_no_events recovery) 走 hard-fail；REVIEW_COMPLETE pass_or_fail=fail；reporter L43 不会 emit LOOP_COMPLETE

**历史重复**：🔴 模式 F+G，`docs/report/2026-06-30-...-170451-diagnosis.md` P2-3 标 P2，本次升 P0

**风险**：严格匹配拦截原本走 narrative 模糊匹配的合法路径；需 plan.blocked schema normalize（`lower().trim()` 后比较或 `reason_normalize` 字段）

### 5.2 P1 / P2 修复（精简）

**P0-3 → P1**：REVIEW_COMPLETE 唯一性
- **目标**：`loop_state.rs:213-214` 加 `review_complete_emitted_at: Option<Instant>`；observe_accepted 接到 REVIEW_COMPLETE 时算 SHA1 hash；若同 hash 则 `tracing::warn!` 写 `delta.kind=duplicate_review_complete` 但**仍写盘**
- **同步**：`presets/en/ce-executor-serial.yml:2398-2419` shipper constraints "MUST emit REVIEW_COMPLETE exactly once per loop"

**P1-2**：fix-02 commit 边界
- **目标**：`presets/en/ce-executor-serial.yml:2467-2473` shipper Commit 段加"On plan.blocked: do NOT create a final commit. The final commit MUST be emitted by the executor on the last work.done step"
- **同步**：executor hat prompt 加"If your work.done is the last fix-unit, the resulting commit's hash MUST be carried into plan.complete.final_commit_hash"

**P1-3**：task_store ensure_task 强约束
- **目标**：`task_store.rs:498` `ensure`:`if task.key.is_none() { return Err(...); }` 改 fail-close
- **同步**：`state_projector/task.rs` 必须从 work.ready 的 task_key 派生 key 字段

**P2-1**：recovery.jsonl envelope 分桶
- **目标**：`crates/ralph-core/src/state/recovery_log.rs` reason_code 新增 `event_rejected_by_gate` vs `event_routed_to_repair` 两个 bucket

**P2-2**：`triggered=ralph` 语义重定义
- **目标**：`ralph_proto::event.rs` `triggered` 表示"哪个上游事件触发"，自身发起时记 `triggered=agent` 或省略（跨包改动，等下个主版本）

### 5.3 推荐修复 plan 排序

1. **plan 2026-07-XX-001**（本周，1-2 天）：闭合 P0-4 + P0-6 + P0-5 + P0-1 + P1-1（5 个 P0/P1 集中在 fix-unit 终态处理，task_store + loop_state + progress-steward）
2. **plan 2026-07-XX-002**（下周，1 天）：闭合 P0-2（shipper reason 白名单严格化 + 新 lint `strict_reason_routing`）
3. **plan 2026-07-XX-003**（下周晚，2-3 天）：闭合 P0-3 + P1-2 + P1-3 + P2-1（边缘路径硬化）
4. **plan 2026-07-XX-004**（可选）：P2-2 triggered 语义（观测性，跨包）

**关键合并建议**：P0-1 + P0-2 + P0-5 + P1-1 + P1-3 几乎都是 **fix-unit 终态处理**问题，对应 Agent B §3.2 P-X1 的 dispatch gap 三件套同一修复点；若并入一个新 plan `docs/plans/2026-07-XX-005-fix-fix-unit-terminal-handling-plan.md`，可一次收口。

---

## 第 6 部分：用户四个问题的回答

### Q1：整体执行过程有没有问题？

**最终交付 ✅ 成功，但执行链路有 4 个 critical 偏离**：

1. fix-02 后违规触发第二轮 review（events L35，违反 preset L826-828 "DO NOT emit review.start when step starts with fix-" 硬规则）—— 系统走了 7 分钟断链（04:18:01 fix-02 完成 → 04:25 之后才被 stall recovery 拉回）。
2. plan.complete 全程未发，被 `plan.blocked(reason: "stall_no_events recovery: ...blocked by policy gates: step_handoff::task_not_found and plan_gate_review_not_terminal")` 替代——终态信号错位。
3. shipper 把非常规 reason 升级为 pass（违反 preset L2494-2497 严格白名单，绕过了 hard-fail 兜底）。
4. REVIEW_COMPLETE 同 payload 发 2 次（时间差 29s，字节级相同）；reporter verdict_gate 兜住 1 次但 events 流污染。
5. ralph runner 抢发 LOOP_COMPLETE 3 次（L37 / L44 / L45，其中 L37 被 ledger 拒）。
6. tasks.jsonl L5 孤儿任务（`key=null, started_at=null, closed`，task_store close_by_key 守卫缺失 + projector 强制 close 同源）。

**但最终结果正确**：completion_honored 在 iter 41 收口，commit `c6d67b5` 已落地，52 tests passed，P1 已修。

### Q2：RALPH 机制是否正常生效？

**OK 的部分**：
- ✅ 单 step 推进严格走 plan-unit phase gate（work.ready → work.done → test.passed → next-unit），4 个 step 顺次推进无乱序
- ✅ review sequence 6 维 walk 完全正常：L15-L27 收敛 `review.dimensions.complete`，聚合到 `review.complete(verdict=fail)`，fix plan 触发正常
- ✅ fix-unit flow 启动正常：fix-01 → test.passed → fix-02 → test.passed
- ✅ completion_honored 在 completion_requested 之后仍能正确反转（reporter L43 触发 → L44 LOOP_COMPLETE → L45 ralph runner 写终态 record）

**漂移的部分**：
- ⚠️ completion_requested 在 fix-unit 完成但 review chain 未终止时被 ralph runner 提前置位（缺 report.done guard）—— P0-5
- ⚠️ tasks.jsonl 写入路径未严格保持一致（孤儿 L5）—— P0-4
- ⚠️ recovery.jsonl 写入路径未做去重 / 未做 bucket —— P2-1
- ⚠️ ledger 双计数器错位（no_progress vs main iter 序列值不一致）—— P0-6
- ⚠️ progress-steward 状态机分桶错误 —— P0-1
- ⚠️ plan_gate 漏掉 fix-unit step 字段豁免 —— P1-1（P-D1/P-M8）

**简短回答**：✅ 主路径机制正常生效（work.ready → work.done → test.passed → review → fix → ship → report → loop 闭环），但 **6 条 P0 都在机制边界或机制↔preset 耦合面角落路径 fail-safe 缺失**。

### Q3：编排是否合理？

**编排侧（10-hat serial isolated 6-dim review_walk）**：✅ 合理
- 10-hat 拓扑 OK，固定 6 维 review_walk 是合理设计
- fix-unit flow 起点（fix-01 → fix-02 串联）符合 P0/P1 修复排序
- shipper / reporter / progress-steward 三方分离 OK
- execution_contracts / topic_deny_rules / isolated scope check 全部按 P-M10 升级成功

**机制侧**：⚠️ 角落路径 fail-safe 不足
- ⚠️ progress-steward 自愈表 + coordinator PHASE GATE 间存在两套判断路径（语义错位）
- ⚠️ shipper reason 路由与 hard-fail 不是严格状态机（narrative + 严格白名单共存，narrative 引导越界）
- ⚠️ completion_requested 在 review chain 未终止时被 ralph runner 提前置位（缺 guard）
- ⚠️ dual counter(no_progress / failures)未合并
- ⚠️ task_store close_by_key 缺 started 守卫
- ⚠️ REVIEW_COMPLETE 没去重（verdict_gate 只覆盖 LOOP_COMPLETE）

**简短回答**：编排（10-hat isolated serial）是合理的；问题全部出在**机制层角落路径 fail-safe 缺失**或**机制↔preset 耦合面**。

### Q4：真有问题，是机制还是编排？

**分类结论**：
- **6/6 P0 都是机制问题（不是编排问题）**：
  - P0-1（progress-steward 状态机混淆）= 机制 + preset 双重
  - P0-2（shipper 白名单 narrative 漂移）= preset 可机器检查但机制未强制
  - P0-3（REVIEW_COMPLETE 唯一性）= 机制缺 dedup
  - P0-4（task_store 守卫缺失）= 机制 bug
  - P0-5（completion_requested guard 缺失）= 机制 bug
  - P0-6（双计数器错位）= 机制 bug
- **3 P1 分别是**: P1-1 preset 字段 backfill（preset + 机制）, P1-2 shipper final-commit 边界（agent 执行）, P1-3 task_store ensure 强约束（机制 + agent）
- **2 P2 都是机制观测性弱化**：P2-1 recovery bucket 缺失，P2-2 triggered 语义稀释
- **编排选错参数导致的问题**: 0 条 —— 编排选 10-hat serial 6 维 review 是 OK 的

**为什么是机制而不是编排**：

1. **编排的"剧本"是合理的** —— B 报告历史 38 条问题中没有 1 条质疑编排选型；本次 plan 还是按编排正常走完 4-step + 2-fix-unit + 1-轮完整 6-dim review
2. **真正出问题的是"剧本之外的角落路径"**：
   - fix-unit 完成后 progress-steward 自愈表的设计选择（机制层的 prompt-as-state-machine）
   - shipper reason 路由的 narrative vs strict 白名单漂移（preset↔机制耦合面的"应该机器检查却仍依赖 prompt 自检"）
   - completion_requested 在 review chain 未终止时被 runner 提前置位（loop_state.rs 的逻辑漏洞）
   - task_store close_by_key 守卫缺失（task_store.rs:452 的代码 bug）
3. **历史 8 条高度相关模式全部是机制类**（P-D1/P-D7/P-M1/P-M4/P-M5/P-M7/P-M8/P-X1）—— 编排从未被指为根因
4. **本次的次级 P0 也都是机制类**：recovery.jsonl 与 events 流口径不一致（recovery_log.rs 没分桶）、ledger 双计数器错位（loop_state.rs 双轨）

**简短回答**：**6/6 P0 + 3 P1 + 2 P2 全部是机制问题**（确切地说是 Ralph Loop 基座机制在角落路径的 fail-safe 缺失 + preset↔机制耦合面的"prompt-as-state-machine"层未机器化），编排本身（10-hat isolated serial 6 维 review_walk）合理且未做错选择。

---

## 第 7 部分：附录

### 7.1 主数据源

- **主 preset**：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/en/ce-executor-serial.yml`（2802 行）
- **主 schema**：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/schemas/ce-executor-serial.yml`
- **主 plan**：`/home/chaowen/Dev/agent_tools/ralph-e2e/docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`
- **主 report**：`/home/chaowen/Dev/agent_tools/ralph-e2e/docs/report/2026-06-30-ce-executor-2026-06-20-001-feat-python-sort-algorithms-report.md`

### 7.2 运行时数据

- 事件流：`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/events-20260630-032648.jsonl`（45 行）
- 任务账本：`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/tasks.jsonl`（7 行）
- runtime ledger：`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/ledger.jsonl`（47 行）
- 修复流：`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/recovery.jsonl`（28 行）
- 诊断 trace：`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/diagnostics/2026-06-30T11-26-48/trace.jsonl`
- 诊断 summary：`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/diagnostics/2026-06-30T11-26-48/diagnosis-summary.json`

### 7.3 Agent 文档

- Agent progress：`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/progress.md`
- Agent summary：`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/summary.md`
- Agent handoff：`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/handoff.md`

### 7.4 关键源码 file:行号（已 grep 验证）

- `task_store.rs:452-459` — `close_by_key` 缺 `started:null` 守卫
- `task_store.rs:498-589` — `ensure` 未去 `(loop_id, key)` 强约束
- `review_step_state.rs:305-353` — `plan_gate_review_not_terminal` 在 fix-unit 末段拦截 `plan.complete`
- `verdict_gate_stage.rs:30` — `DEFAULT_TERMINAL_EMITS = &["LOOP_COMPLETE"]`，REVIEW_COMPLETE 不是 terminal
- `loop_state.rs:213-214` — `completion_requested / completion_honored` 仅覆盖 loop.complete
- `loop_state.rs:334` vs `:698` — 双轨计数器
- `loop_state.rs:417` — `stall_recovery_counts: HashMap<String, u32>`
- `rejection_kind.rs:45-64` — `StallNoEvents` 拒绝原因定义

### 7.5 报告版本

**报告版本**: v1（2026-06-30，合并单份版）
**任务来源**: `/home/chaowen/Dev/agent_tools/ralph-orchestrator/task.md`
**诊断角色**: 4 个 sub-agent 并行：A 流程还原 + B 历史关联 + C 对账分析 + D 归因修复，由主 Agent 汇总合并
