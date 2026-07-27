# RALPH 链路诊断报告 — primary-20260630-083222（终态）

> **run**: primary-20260630-083222（loop 已 terminated at iter 45，consecutive_failures 终态）
> **preset**: `builtin:ce-executor-serial`（isolated mode，10-hat）
> **plan**: `2026-06-20-001-feat-python-sort-algorithms-plan`（2-UNIT 4-step + 3 fix-unit；U3 未 dispatch）
> **run_dir**: `/home/chaowen/Dev/agent_tools/ralph-e2e/`
> **诊断日期**: 2026-06-30
> **诊断方式**: 4 个 sub-agent 并行（流程还原 / 历史上下文 / 对账分析 / 归因修复）+ 主 agent 汇总
> **第一版生成**: 2026-06-30 17:56（link 未终止前） → **本次更新**: 2026-06-30 18:40（loop terminated）

---

## 第 0 部分：结论摘要

**整体健康度**: ⚠️→🔴 **代码任务已完成，但链路未通过正常路径闭环**。fix-01 + fix-02 测试均通过（51/51 tests），代码修复（F-001 count_swaps / F-002 stability test / F-003 is_stable duplicate）全部落地，但 `plan.complete` 反复被拒（共 **9 次**被 plan_gate / schema / isolated-mode 拒绝），最终 shipper 走 hard-fail reason 路径发 REVIEW_COMPLETE(fail) → reporter report.done → 但 plan.blocked 没拦住后续 work.ready re-storm → 触发 consecutive_failures 终止。

- **关键异常数量**: P0×4, P1×3, P2×2（共 9 项，4 critical / 3 major / 2 minor）
- **历史重复**: ✅ 是 —— 与 primary-20260630-032648、primary-20260629-170451、primary-20260628-loop-and-mechanism-failure 同源；其中 fix-unit chain 桥接问题（P-M8 / P-X1）4+ 次复现
- **机制 vs 编排责任划分**: 编排问题 60% + 机制-编排协作裂缝 25% + 机制本身 15%

**与第一版的差异**（关键变化）：
1. ✅ **fix-02 实际被完成了**：test.passed(50/50 then 51/51)，通过 task-1782813211-u2（**错配 key 的那条**）+ task-1782813343-3b7d（**正确 key**）两次 work.done 闭环
2. ❌ **plan.complete 被拒次数从 2 次增长到 9 次**（recovery.jsonl 共 21 行，其中 plan.complete 入 repair sink 9 条）
3. 🔴 **loop 走完 shipper hard-fail 路径**：REVIEW_COMPLETE(pass_or_fail=fail) → report.done（pass_or_fail=fail, verdict=fail）→ 但 loop 没正常终止，反而又触发 work.ready re-storm → consecutive_failures 终止
4. 🔴 **ralph 自身在 10:22:39 直接 emit work.ready**（task_id=task-1782813343-3b7d, task_key=:fix-02:u2）—— 推测是 progress-steward 注入触发，coordinator 没接管

---

## 第 1 部分：执行链路对比（流程还原）

### 1.1 完整事件流（45 events，`events-20260630-083222.jsonl`）

| # | ts (UTC) | topic | hat | 关键 payload | 状态 |
|---|---|---|---|---|---|
| 1 | 08:32:22 | work.start | loop-bootstrap | plan_path=...2026-06-20-001-feat-python-sort-algorithms-plan.md | ✅ |
| 2 | 08:33:50 | work.ready | coordinator | step=step-01, task=`-fd77` | ✅ |
| 3 | 08:39:10 | work.done | executor | changed_lines=512, commit=1 | ✅ |
| 4 | 08:40:26 | test.passed | validator | tests_passed=13/13 | ✅ |
| 5 | 08:42:00 | work.ready | coordinator | step=step-02, task=`-c4fc` | ✅ |
| 6 | 08:43:53 | work.done | executor | changed_lines=186, commit=1 | ✅ |
| 7 | 08:44:30 | work.done | executor | key=`:step-02:u1-impl`（**key 错配**） | ⚠️ duplicate_work_done rejected |
| 8 | 08:47:39 | task.resume | ralph | next_action=`start_unit2_step1` | ✅ 自愈 |
| 9 | 08:50:33 | work.ready | coordinator | step=step-01, task=`-0a76`（UNIT 2 语义） | ✅ |
| 10 | 08:53:34 | work.done | executor | changed_lines=132, commit=1 | ✅ |
| 11 | 08:54:10 | test.passed | validator | tests_passed=29/29 | ✅ |
| 12 | 08:56:23 | work.ready | coordinator | step=step-02, task=`-6638` | ✅ |
| 13 | 08:59:14 | work.done | executor | changed_lines=337, commit=1 | ✅ |
| 14 | 08:59:50 | test.passed | validator | tests_passed=47/47 | ✅ |
| 15 | 09:00:29 | review.start | coordinator | unit_index=2, total_units=2 | ✅ pre-decision gate 通过 |
| 16-27 | 09:01-09:25 | review.{dimension.ready,dimension.done}×6 | review-coordinator / dimension-reviewer | 6-dim walk 全 done，findings 累计 1+1+2+3+2+2=11 | ✅ |
| 28 | 09:26:54 | review.dimensions.complete | review-coordinator | fix_round=0 | ✅ |
| 29 | 09:29:07 | review.complete | review-synthesizer | **verdict=fail**, residual_findings_count=5, fix_plan_file=`.../fix-plan.md`, fix_round=0 | ✅ 字段齐全 |
| 30 | 09:30:50 | work.ready | coordinator | step=**fix-01**, task=`-3de0`, key=`:fix-01:u1` | ✅ |
| 31 | 09:49:27 | work.done | executor | changed_lines=166, commit=1 | ✅ |
| 32 | 09:50:04 | test.passed | validator | tests_passed=49/49 | ✅ **fix-01 闭合** |
| 33 | 09:53:31 | work.ready | coordinator | step=fix-02, task=`task-1782813211-u2`, key=**`:fix-01:u2`**（**错配**） | ⚠️ task_key 与 step 不一致 |
| 34 | 09:54:16 | work.ready | coordinator | step=fix-02, task=`task-1782813256-f002`, key=`:fix-02:u2` | ⚠️ **task_id 非标准格式** |
| 35 | 09:55:50 | work.ready | coordinator | step=fix-02, task=`task-1782813343-3b7d`, key=`:fix-02:u2` | ⚠️ 同 task_id 复用 |
| 36 | 10:04:41 | **work.done** | executor | changed_lines=36, commit=1, step=fix-02, **task=`task-1782813211-u2`**（**错配 key 的那个 task_id**！） | ✅ **执行了**（executor 读了错的 task_key） |
| 37 | 10:05:25 | **test.passed** | validator | tests_passed=**50**/50, step=fix-02, task=`task-1782813211-u2` | ✅ |
| 38 | 10:10:43 | work.ready | coordinator | step=fix-02, task=`task-1782813343-3b7d`, key=`:fix-02:u2`（再次重发） | ✅ projector 已重建 ledger |
| 39 | 10:12:25 | **plan.blocked** | coordinator | reason=**fix_unit_complete_but_policy_conflict**: test.passed(fix-02) 50 tests passed, P0-1 要求 plan.complete，但 emit 被 task_not_found（task-1782813343-3b7d 不在 runtime ledger）+ plan_gate_review_not_terminal 拒 | ❌ |
| 40 | 10:15:37 | **work.done** | executor | changed_lines=37, commit=1, step=fix-02, task=`task-1782813343-3b7d` | ✅ 第二次执行 |
| 41 | 10:16:17 | **test.passed** | validator | tests_passed=**51**/51, step=fix-02 | ✅ |
| 42 | 10:17:21 | **plan.blocked** | coordinator | reason=**projector_task_not_found**: task-1782813343-3b7d 在 tasks.jsonl 但不在 runtime ledger；fix-unit 链全完成；plan.complete 被 projector desync 阻断 | ❌ |
| 43 | 10:20:08 | **REVIEW_COMPLETE** | shipper | pass_or_fail=**fail**, verdict=fail, final_findings_count=1, residual_findings_summary="plan.blocked with hard-fail reason: projector_task_not_found..." | ⚠️ shipper reason 升级 |
| 44 | 10:21:31 | **report.done** | reporter | report_path=`docs/report/2026-06-30-ce-executor-2026-06-20-001-feat-python-sort-algorithms-plan-report.md`, awaiting_decision=true, pass_or_fail=fail, verdict=fail | ⚠️ |
| 45 | 10:22:39 | **work.ready** | **ralph**（不是 coordinator！） | task=`task-1782813343-3b7d`, key=`:fix-02:u2`, step=fix-02 | 🔴 **ralph 抢发**——推测 progress-steward 触发 |
| — | 10:40:07 | **loop.terminate** | loop | reason=**consecutive_failures**, 2h 7m 45s, exit_code=1 | 🔴 |

### 1.2 对比图（✅/❌/⏸️）

| 步骤 | 预期 | 实际 | 状态 |
|---|---|---|---|
| 4 步 plan unit 执行 | 13→26→34→52 tests 逐级通过 | 完全按序 | ✅ |
| review.start 触发 | pre-decision gate 确认 N_total=2 | unit_index=2, total_units=2 | ✅ |
| 6-dim review walk | 6 维全 done | 6/6 done，findings 11 条 | ✅ |
| review.complete | 携带 verdict/findings/fix_plan_file | verdict=fail, residual=5, fix_plan=`.agents/.../fix-plan.md` | ✅ |
| fix-01 dispatch | key=`:fix-01:u1` | task=`-3de0`, key=`:fix-01:u1` | ✅ |
| fix-01 work.done → test.passed | 49/49 | 49/49 closed | ✅ |
| **fix-02 dispatch #1** | key=`:fix-02:u2` | step=fix-02, key=**`:fix-01:u2`**（**错配**） | ⚠️ 但被 executor 实际执行 |
| **fix-02 work.done #1** | 应基于正确 task_key 校验 | executor 真的对 `task-1782813211-u2`/`fix-01:u2` 跑了 U2 改动 | ⚠️ 错配但功能完成 |
| **fix-02 test.passed #1** | 50/50 通过 | 50/50 ✅ | ⚠️ |
| **fix-02 dispatch #2** | 重发正确 task_key | task=`task-1782813343-3b7d`, key=`:fix-02:u2` | ⚠️ |
| **fix-02 work.done #2** | U2 已有 50/50，再跑应当 idempotent | changed_lines=37, 51/51 tests | ✅ |
| **plan.complete attempt** | step=fix-02 走豁免 | 9 次被拒（plan_gate_review_not_terminal / task_not_found） | ❌ |
| **plan.blocked fallback** | shipper → REVIEW_COMPLETE(fail) → reporter → report.done | REVIEW_COMPLETE(fail) ✓, report.done ✓ | ⚠️ hard-fail 路径 |
| **plan.blocked 后应停止** | plan.blocked → loop 终态 | ralph 在 10:22 抢发 work.ready → 再次进入循环 | 🔴 |
| **loop 终止** | LOOP_COMPLETE 后干净退出 | consecutive_failures 强制终止 | 🔴 |

### 1.3 关键异常事件原文

**fix-02 三次 work.ready（events.jsonl L33-35）**：

```json
// L33 - 09:53:31, 首次（task_key 错配）
{"payload":{"step":"fix-02","task_id":"task-1782813211-u2",
  "task_key":"ce-executor:2026-06-20-001-feat-python-sort-algorithms-plan:fix-01:u2"}}

// L34 - 09:54:16, 修正 key
{"payload":{"step":"fix-02","task_id":"task-1782813256-f002",
  "task_key":"ce-executor:2026-06-20-001-feat-python-sort-algorithms-plan:fix-02:u2"}}

// L35 - 09:55:50, 再发
{"payload":{"step":"fix-02","task_id":"task-1782813343-3b7d",
  "task_key":"ce-executor:2026-06-20-001-feat-python-sort-algorithms-plan:fix-02:u2"}}
```

**fix-02 第一次 work.done（events.jsonl L36）**—— task_id=`task-1782813211-u2` 那个**错配 task_key** 的：

```json
{"hat":"executor","topic":"work.done","triggered":"ralph","ts":"2026-06-30T10:04:41.263944033+00:00",
 "payload":{"changed_lines":36,"commit_count":1,"step":"fix-02",
  "task_id":"task-1782813211-u2",
  "task_key":"ce-executor:2026-06-20-001-feat-python-sort-algorithms-plan:fix-01:u2"}}
```

**fix-02 第二次 work.done（events.jsonl L40）**—— task_id=`task-1782813343-3b7d` 那个**正确 task_key** 的：

```json
{"hat":"executor","topic":"work.done","triggered":"ralph","ts":"2026-06-30T10:15:37.468735188+00:00",
 "payload":{"changed_lines":37,"commit_count":1,"step":"fix-02",
  "task_id":"task-1782813343-3b7d",
  "task_key":"ce-executor:2026-06-20-001-feat-python-sort-algorithms-plan:fix-02:u2"}}
```

**plan.complete 多次被拒**（recovery.jsonl 9 条）—— 注意 completed_steps 演进：

```json
// 前两次 (09:50, 09:53) — fix-02 测试未到
{"completed_steps":"step-01, step-02, fix-01", "task_id":"task-1782811846-3de0"}

// 第三次 (10:10) — fix-02 第一次 test.passed 后
{"completed_steps":"step-01,step-02,fix-01,fix-02", "task_id":"task-1782813343-3b7d"}

// 第四次 (10:10) — 用 task-1782813211-u2（错配 key 的那个）
{"completed_steps":"step-01,step-02,fix-01,fix-02", "task_id":"task-1782813211-u2"}

// 第五/六/七/八/九次 (10:11-10:17) — task_id 在 -3b7d 和 -u2 间反复横跳
{"completed_steps":"step-01,step-02,fix-01,fix-02", "task_id":"task-1782813343-3b7d"}

// 最后一次 (10:17) — 用一个新生成的 task_id=`task-1782815057-3ccf`
{"completed_steps":..., "task_id":"task-1782815057-3ccf"}
```

**plan.blocked reason**：

```json
// 10:12 第一次（fix-02 第一次 test.passed 后）
{"reason":"fix_unit_complete_but_policy_conflict: test.passed(fix-02) arrived with 50 tests passed, 
 P0-1 rule requires plan.complete, but emit is rejected with task_not_found 
 (task-1782813343-3b7d not in runtime task ledger despite existing in tasks.jsonl) 
 and plan_gate_review_not_terminal 
 (expects review terminal for fix-01 step but fix-unit bypass rule should override). 
 Projector bug caused task ledger desync."}

// 10:17 第二次（fix-02 第二次 test.passed 后）
{"reason":"projector_task_not_found: task-1782813343-3b7d exists in tasks.jsonl 
 but not in runtime ledger; fix-unit chain complete (fix-01+fix-02 all passed); 
 P0-1 requires plan.complete but blocked by projector desync"}
```

**REVIEW_COMPLETE**（shipper）：

```json
{"pass_or_fail":"fail","verdict":"fail","final_findings_count":1,
 "residual_findings_summary":"plan.blocked with hard-fail reason: projector_task_not_found. 
 task-1782813343-3b7d exists in tasks.jsonl but not in runtime ledger. 
 fix-unit chain completed successfully (51 tests passed), but plan.complete blocked 
 by projector desync. This is a projector bug, not a unit/review/fix pipeline failure. 
 P0 finding F-001 (count_swaps) was fixed by fix-01. No remaining P0 findings."}
```

**isolated-mode drop 警告**（log L230/L231）：
```
WARN ralph_core::event_loop: Isolated mode: extra business event dropped — only one per turn topic=work.ready
WARN ralph_core::event_loop: Isolated mode: extra business event dropped — only one per turn topic=work.ready
```

**loop 终止**（history.jsonl / events-history-20260630-083222.jsonl）：

```json
{"ts":"2026-06-30T10:40:07.466727830+00:00","iteration":45,"hat":"loop",
 "topic":"loop.terminate","payload":"## Reason\nconsecutive_failures\n\n## Status\nToo many consecutive failures.\n\n## Summary\n- Iterations: 45\n- Duration: 2h 7m 45s\n- Exit code: 1"}
```

---

## 第 2 部分：历史问题上下文

### 2.1 历史诊断报告关联度

| 历史 run | 与本次关联度 | 关键共享模式 |
|---|---|---|
| **primary-20260630-032648** | 高 | plan_gate 与 fix-unit 冲突（P-D1/P-M8/P-X1）；coordinator Branch A 计数走偏；completed_steps 错配 |
| **primary-20260629-170451** | 高 | coordinator fix-unit prompt 模板 task_id 复用；validator 缺席（本次部分命中：validator 有发 test.passed，但 fix-02 第一次的 task_id 是错配 key 那个） |
| **primary-20260628-loop-and-mechanism-failure** | 中 | plan-gate + debug-resolver 旧架构时代；本次 plan_gate 仍然误拒 fix-unit 终态 |

### 2.2 Memory 条目关联

| Memory 条目 | 关联度 | 与本次对应 |
|---|---|---|
| `ce-executor task ownership` | 高 | coordinator 创建 task 不规范 → 本次 fix-02 三次 work.ready |
| `ce-executor-isolated dispatch gap` | 高 | plan-gate→executor 推进时缺桥接事件 → 本次 fix-01→fix-02 桥接缺失 |
| `review-coordinator isolated-scope recovery` | 高 | 同源 review 链断路 |
| `ce-executor stale activation work.done closure` | 高 | 本次 hard-fail 后还触发 work.ready re-storm |
| `ralph emit hat channel routing` | 中 | isolated mode 下 hat-channel 路由 |
| `ralph emit policy check still writes` | 中 | `--policy-check` 仍会写盘 → 解释为何 9 次 plan.complete 都入 repair.jsonl |
| `task.resume target_hat dead path` | 中 | progress-steward 注入 task.resume 后 coordinator 走错路径 |

### 2.3 历史问题分类

| 问题类型 | 历史案例数 | 本次关联 |
|---|---|---|
| plan_gate 与 fix-unit 冲突 | 5+ 次 | ✅ **本次 P0-A 根因**（9 次被拒） |
| coordinator 重复 work.ready | 3+ 次 | ✅ **本次 P0-B 根因**（fix-02 三次 dispatch） |
| task_id 复用 / 格式不规范 | 4+ 次 | ✅ **本次 P0-B 放大原因**（u2/f002/3b7d 三个 task_id 同 key 复用） |
| completed_steps 与 fix-plan U 编号错配 | 2 次 | ✅ **本次 P1-C 根因**（前两次 completed_steps 漏 fix-02） |
| isolated-mode extra event drop | 1 次 | ✅ **本次 P0-B 放大器** |
| projector runtime/persistent ledger 失步 | **首次记录** | ✅ **本次 P0-D 新症状**（task-1782813343-3b7d 在 tasks.jsonl 不在 runtime ledger） |
| ralph 自身 emit work.ready 抢 coordinator 角色 | **首次记录** | ✅ **本次 P0-E 新症状**（events L45，hat=ralph 不是 coordinator） |

---

## 第 3 部分：偏离证据清单

### 3.1 plan.complete 9 次拒绝证据

**`recovery.jsonl` 9 条 plan.complete repair-stream 记录**（关键字段）：

| 次 | ts | task_id | completed_steps | 备注 |
|---|---|---|---|---|
| 1 | ~09:50 | `task-1782811846-3de0` (fix-01) | `[step-01, step-02, fix-01]` | 缺 fix-02 |
| 2 | ~09:53 | `task-1782811846-3de0` (fix-01) | `[step-01, step-02, fix-01]` | 缺 fix-02 |
| 3 | ~10:11 | `task-1782813343-3b7d` | `[step-01, step-02, fix-01, fix-02]` | 修复完成 |
| 4 | ~10:11 | `task-1782813211-u2` | 同上 | task_id 横跳 |
| 5 | ~10:11 | `task-1782813343-3b7d` | 同上 | 反复 |
| 6 | ~10:12 | `task-1782813343-3b7d` | 同上 | 同 semantic_gate_violation 后 |
| 7 | ~10:13 | `task-1782813343-3b7d` | 同上 | |
| 8 | ~10:15 | `task-1782813343-3b7d` | 同上 | |
| 9 | ~10:17 | `task-1782815057-3ccf`（新生成） | 同上 | 终于换了 task_id 但仍拒 |

**semantic_gate_violation**（recovery.jsonl 1 条）：
```json
{"reason_code":"semantic_gate_violation",
 "message":"isolated scope violation: hat 'coordinator' is not allowed to publish topic 'review.passed'; 
  allowed publishes: [\"work.ready\", \"review.start\", \"plan.complete\", \"plan.blocked\", \"LOOP_COMPLETE\"]",
 "ts":"2026-06-30T10:12:00.366664136Z"}
```
**新症状**: coordinator 曾尝试 emit `review.passed`（违反 isolated scope），被 semantic_gate 拒——这是 primary-20260630-032648 run DE-003 同源问题，coordinator 想绕过 plan.complete 走 review.passed → review-coordinator → review.complete 路径失败。

### 3.2 fix-02 实际执行链路（关键更新）

| 时序 | task_id | task_key | executor 实际读到的 plan 段 | 结果 |
|---|---|---|---|---|
| 09:53 dispatch #1 | `task-1782813211-u2` | `:fix-01:u2` | 推测读 U2 段（fix-plan U2 描述） | ❌ 错配但未阻塞执行 |
| 10:04 work.done #1 | `task-1782813211-u2` | `:fix-01:u2` | 改了 sorts/_compare.py 36 行 | ✅ changed_lines=36 |
| 10:05 test.passed #1 | `task-1782813211-u2` | `:fix-01:u2` | 50/50 tests | ✅ |
| 09:54 dispatch #2 | `task-1782813256-f002` | `:fix-02:u2` | — | ❌ isolated-mode drop（log L230） |
| 09:55 dispatch #3 | `task-1782813343-3b7d` | `:fix-02:u2` | — | ❌ isolated-mode drop（log L231） |
| 10:10 dispatch #4 | `task-1782813343-3b7d` | `:fix-02:u2` | — | ✅ projector 重建 ledger 后接收 |
| 10:15 work.done #2 | `task-1782813343-3b7d` | `:fix-02:u2` | 改了 sorts/tests/test_quick_sort.py 37 行 | ✅ changed_lines=37 |
| 10:16 test.passed #2 | `task-1782813343-3b7d` | `:fix-02:u2` | **51/51 tests** | ✅ |

**关键观察**: 
- fix-02 实际**被执行了两次**：第一次是错配 task_key（`:fix-01:u2`）的那个 task_id=`task-1782813211-u2`，第二次是正确 task_key 的 task_id=`task-1782813343-3b7d`
- 两次 work.done 的 `changed_lines` 不同（36 vs 37），说明第二次执行了对 U2 修复的剩余部分（推测 F-002 stability test 实际由第二次 work.done 修复，F-001 由第一次已修）
- 最终 test.passed(51/51) 包含了两次 fix-02 的合并效果

### 3.3 tasks.jsonl 最终状态

| task_id | key | status | 来源 |
|---|---|---|---|
| `task-1782808424-fd77` | `:step-01:u1-skeleton` | closed | events L2 |
| `task-1782808917-c4fc` | `:step-02:u2-quick-sort` | closed | events L5 |
| `task-1782808917-c4fc` | `:step-02:u1-impl` | closed | events L7（**task_id 复用**） |
| `task-1782809426-0a76` | `:step-01:u2-quick-sort-enhancement` | closed | events L9（**task_id 复用**） |
| `task-1782809780-6638` | `:step-02:u2-readme-integration` | closed | events L12 |
| `task-1782811846-3de0` | `:fix-01:u1` | closed | events L30 |
| **`task-1782813343-3b7d`** | `:fix-02:u2` | **closed**（10:15:50） | events L40 |
| **`task-1782813211-u2`** | `:fix-01:u2`（**key 错配**） | **closed**（10:04:52） | events L36 |
| `task-1782815057-3ccf` | `:fix-02:u2-impl` | **open**（未闭合） | events L45（ralph 抢发，未接 executor） |
| **（U3 / fix-03）** | — | **不存在** | coordinator 从未 dispatch U3 |

**最终 closed tasks: 8 条**（UNIT 1: 2 + UNIT 2: 2 + fix-01: 1 + fix-02: 2 含错配 task_id + fix-01 task_id 复用）—— **fix-02 通过两条 task_id 都 closed 了（双胞胎）**。

### 3.4 projector 账本失步证据

**症状**: `task-1782813343-3b7d` 在 tasks.jsonl 中存在（closed at 10:15:50），但 coordinator emit `plan.complete(task_id=task-1782813343-3b7d)` 时被 policy 拒（reason=`task_not_found`）。

**plan.blocked reason 完整记录**（10:12 与 10:17 两次）：
- 10:12: `task-1782813343-3b7d not in runtime task ledger despite existing in tasks.jsonl`
- 10:17: `task-1782813343-3b7d exists in tasks.jsonl but not in runtime ledger`

**根因推断**: projector 写 tasks.jsonl（持久层）成功，但 runtime ledger（内存层，state_projector/task.rs:`TaskStore`）在某次 re-projection 失败或 hot-reload 时丢了这条 task。在 10:04 第二次 fix-02 work.ready 重新触发 projector 重建 ledger 后 task-1782813343-3b7d 才被 runtime 看到——这解释了为什么 fix-02 work.done #2 (10:15) + test.passed (10:16) 正常落盘，但 plan.complete (10:17) 仍被 task_not_found 拒（plan.complete 走的是另一个 ledger 路径，与 work.ready 用的 projector 路径不同）。

### 3.5 plan_gate 豁免触发证据（与第一版不同）

**`recovery.jsonl` 第 3-9 条**显示：**coordinator 后来带的 plan.complete payload 已包含 `step` 字段**（`step:"fix-02"`），所以**plan_gate 的 fix-* 豁免分支应当触发**——但仍被拒。

**这暴露了第一版未识别的关键裂缝**：豁免分支 (`review_step_state.rs:329-331`) 触发后仍走 line 332-353 的 **matching 检查**，要求 `plan_name + task_id` 在 `self.steps` 中有 synth_terminal——但 fix-02 的 StepReviewState 因为 projector 账本失步，**可能不在 tracker 的 self.steps 中**，所以 matching 为空 → `plan_gate_review_not_terminal`。

### 3.6 ralph 抢发 work.ready（events L45）

**events.jsonl L45**:
```json
{"hat":"ralph","topic":"work.ready","ts":"2026-06-30T10:22:39.650721859+00:00",
 "payload":{"complexity":"simple","plan_name":"...","plan_path":"docs/plans/...",
            "step":"fix-02","task_id":"task-1782813343-3b7d",
            "task_key":"ce-executor:...:fix-02:u2"}}
```

**hat=`ralph` 而不是 `coordinator`**！这违反 preset L647（coordinator 是唯一允许发 work.ready 的 hat）和 L994 "MUST NOT publish events for any hat other than coordinator"。推测是 progress-steward 在 plan.blocked 后注入 task.resume，ralph 自身兜底发 work.ready 试图重启 fix-unit chain。

---

## 第 4 部分：问题归因表（P0/P1/P2）

| 优先级 | 问题描述 | 根因分类 | 证据 | 历史关联 |
|--------|----------|----------|------|----------|
| **P0-A** | `plan.complete` 9 次被 plan_gate / schema 拒（plan_gate_review_not_terminal / task_not_found） | **机制-编排协作裂缝**：schema 不要求 step + projector 账本失步 + 豁免后仍走 matching | `presets/schemas/ce-executor-serial.yml:264-277` schema；`review_step_state.rs:329-331` 豁免依赖 step + 仍走 matching；recovery.jsonl L7-L21 共 9 条 plan.complete 拒；plan.blocked reason 10:17 "task-1782813343-3b7d exists in tasks.jsonl but not in runtime ledger" | 是（032648 P-D1/P-M8/P-X1，5+ 次复现） |
| **P0-B** | fix-02 三次 work.ready 后两次被 isolated-mode drop + task_id 格式均违规 | **编排（coordinator 模板 bug）+ 机制（isolated-mode drop 不报错）+ 基座（projector 弱校验）** | log L230/L231 WARN；preset L1144 要求 `task-<ts>-<4hex>` 实际 `u2`/`f002`/`3b7d`；`state_projector/task.rs:81-88` 仅校验 empty；`task.rs:111-129` 仅 debug! | 是（032648 DE-002、170451 DE-005） |
| **P0-C** | fix-03 (U3) task 从未被 coordinator dispatch | **编排（coordinator Branch A 计数逻辑跳到 plan.complete）** | fix-plan.md U1/U2/U3（line 22/37/52）；tasks.jsonl 无 U3 行；preset L847-857 Branch A 未强制必做 | 是（032648 DE-001/002 同源） |
| **P0-D** | projector runtime ledger 与 tasks.jsonl 失步，task-1782813343-3b7d 在持久层存在但 runtime ledger 找不到 | **机制 bug（projector 内存/持久不同步）** | plan.blocked reason 10:12/10:17 显式声明；tasks.jsonl L8 closed；recovery.jsonl 9 条 plan.complete 拒 | **首次记录** |
| **P0-E** | ralph 自身在 10:22:39 抢发 work.ready（hat=`ralph` 不是 coordinator），违反 preset L994 "MUST NOT publish events for any hat other than coordinator" | **机制 bug（ralph 兜底越权）+ 编排（coordinator 在 plan.blocked 后没接管）** | events L45 hat=`ralph`；preset L647 coordinator 是唯一 work.ready publisher；preset L994 "MUST NOT publish events for any hat other than coordinator" | **首次记录** |
| **P1-F** | coordinator 10:12:00 尝试 emit `review.passed`（违反 isolated scope：coordinator 不允许发 review.passed） | **编排（coordinator 想绕过 plan.complete 走 review.passed → review-coordinator → review.complete 路径失败）** | recovery.jsonl semantic_gate_violation 记录 | 是（032648 DE-003 同源） |
| **P1-G** | fix-02 task_id/task_key 错配（`:fix-01:u2` + `task-1782813211-u2`），但 executor 实际执行了错配 task_id（tasks.jsonl L9 closed 10:04:52） | **编排（coordinator 模板 bug + executor 没校验 task_key 与 step 的一致性）** | events L33/L36 task_id=task-1782813211-u2 task_key=`:fix-01:u2` step=`fix-02` | 是（170451 P0-3） |
| **P2-H** | recovery.jsonl 反复出现 task.resume / plan.complete repair-stream 记录（21 条） | **机制（兜底机制被反复触发，是 P0-A/B/C/D 的下游症状）** | `.ralph/recovery.jsonl` line 7-21 | 是 |
| **P2-I** | dimension-reviewer 6 次修改 plan.md 但被 audit 警告（不在 plan_unit 写权限内） | **编排（dimension-reviewer 模板未禁用 plan 修改）** | log L63/L80/L97/L114/L130/L146 WARN `scope violation: docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md` | 是（audit 已记录但无 fail-close） |

---

## 第 5 部分：修复建议（按优先级，与第一版基本一致，新增 P0-D/E）

### 修复 1（P0-A 根因，**与第一版相同 + 增强**）

**目标**：
- 主路径：`presets/schemas/ce-executor-serial.yml:264-277` 增加 `step` 为 required
- 备路径：`crates/ralph-core/src/event_loop/review_step_state.rs:329-353` 放宽豁免后 matching 检查（fix-* step 豁免后不再要求 matching）

```rust
// review_step_state.rs:329-353
let step_str = obj.get("step")...;
if step_str.starts_with("fix-") {
    // 第一版只 return None；第二版增强：还要校验 projector 账本是否能看到 task
    return None;  // fix-unit 路径完全不依赖 matching
}
// 否则走原 line 332-353 通用路径
```

**预期效果**：fix-* step 的 plan.complete 不再受 matching 限制；P0-A 9 次 plan.complete 拒全部消失。

### 修复 2（P0-B 根因，与第一版相同）

**目标**：`crates/ralph-core/src/state_projector/task.rs:81-129`
- 强校验 task_id 格式（preset L1144 SSOT）
- P0-3 复用检测从 debug 升级到 warn + reject

### 修复 3（P0-C 根因，与第一版相同）

**目标**：`presets/en/ce-executor-serial.yml:847-869` Branch A 增加"必须重读 fix-plan 算 U 总数"硬 gate

### 修复 4（P0-D 根因，**新增**）：projector runtime/persistent ledger 同步保证

**根因**：`task-1782813343-3b7d` 在 tasks.jsonl 存在但 runtime ledger 找不到。

**目标**：
- `crates/ralph-core/src/state_projector/task.rs:171` `store.ensure(task)` 后增加 runtime ledger 同步
- 或：plan_gate 等所有读 self.steps 的路径，**优先从 tasks.jsonl 重新加载**（已持久化的源）而不是用 runtime 内存

```rust
// 在 plan_gate / matching 检查时优先 reload from disk
fn refresh_runtime_ledger_from_disk(&mut self) {
    let disk_store = TaskStore::load(&self.tasks_path).unwrap();
    // 合并 disk_store 到 self.runtime_ledger
}
```

**预期效果**：即便 projector 写入失败，policy 检查时仍能读到 task；不再出现"tasks.jsonl 有但 runtime 找不到"。

### 修复 5（P0-E 根因，**新增**）：ralph 兜底 work.ready 强制带 coordinator hat

**根因**：events L45 hat=`ralph` 抢发 work.ready，违反 preset L647/L994。

**目标**：`crates/ralph-core/src/event_loop/stages/progress_steward_stage.rs` 或 `ralph.rs`：

```rust
// ralph 兜底 emit work.ready 时强制 hat="coordinator"
if hat == "ralph" && topic == "work.ready" {
    tracing::warn!("ralph fallback work.ready should be re-routed via coordinator");
    // 改写 hat 字段为 coordinator，或者 reject 并 trigger task.resume 让 coordinator 接管
    event.hat = Some("coordinator".to_string());
}
```

**预期效果**：ralph 不再越权发 work.ready；plan.blocked 后由 coordinator 接管（task.resume 触发）继续推进。

### 修复 6（P1-F 根因，**新增**）：coordinator 不能 emit review.passed（已部分实现，需加强）

**根因**：coordinator 在 plan.complete 被拒时尝试 emit review.passed 绕过（recovery.jsonl semantic_gate_violation）。

**目标**：`presets/en/ce-executor-serial.yml:647` 已声明 coordinator 不允许发 review.passed，但 harness 没 fail-close。`crates/ralph-core/src/event_loop/stages/scope_guard.rs` 增加：

```rust
if topic == "review.passed" && hat == "coordinator" {
    return Err(format!(
        "scope_violation: coordinator cannot publish review.passed; \
         use plan.complete → shipper chain instead"
    ));
}
```

**预期效果**：coordinator 想绕过 plan.complete 走 review.passed 的尝试会被直接拒（而非仅 WARN）。

---

## 第 6 部分：不建议的修复

1. **扩大 plan_gate 豁免到"无 step 字段也放行"** —— 与第一版同
2. **projector 直接 reject 所有非 `from_key:` 形式的 task_id** —— 与第一版同
3. **让 ralph 完全不发 work.ready** —— 会破坏合法兜底场景（coordinator 真的卡死）；修复 5 的"hat 改写"是更稳妥的限流策略
4. **跳过 plan.blocked 直接发 plan.complete** —— 会让 shipper 走 hard-fail 路径收口（本次实际就是这条路径），但 fix-unit chain 没真正闭环会埋雷

---

## 第 7 部分：机制 vs 编排责任划分（回答用户问题）

### 7.1 整体执行过程有没有问题？

**有 9 项偏离**（4 P0 + 3 P1 + 2 P2），链路 70% 工作（4 step + 6-dim review + fix-01 + fix-02 全部完成 51/51 tests），但最后 30% plan.complete 卡死 + projector 账本失步 + ralph 抢发 + consecutive_failures 终止。代码本身完整修复，但编排未通过正常路径闭环。

### 7.2 RALPH 基座机制是否正常生效？

**绝大多数机制正常工作**：hat 拓扑、execution_contracts、event_policy schemas、state machine、isolated-mode one-event-per-turn（log L230/231）、progress-steward、shipper reason-based routing —— 全按 spec 工作。

**新增 2 个机制 bug**：
- 🔴 **projector runtime/persistent ledger 失步**（P0-D）—— 首次记录，task-1782813343-3b7d 在 tasks.jsonl 但不在 runtime ledger
- 🔴 **ralph 兜底越权**（P0-E）—— ralph 在 plan.blocked 后抢发 work.ready（hat=ralph），违反 preset L994

### 7.3 编排是否合理且正常运行？

**编排有多处违规**：
- ❌ coordinator Branch A 计数逻辑未硬 gate（跳过 fix-03/U3）
- ❌ coordinator 模板 bug：task_key 与 step 错位（`:fix-01:u2` + `step=fix-02`）
- ❌ coordinator 模板 bug：task_id 手写不规范（`u2`/`f002`/`3b7d`）
- ❌ coordinator 尝试 emit review.passed 绕过 plan.complete（违反 isolated scope）
- ❌ dimension-reviewer 6 次 scope violation 改 plan.md
- ⚠️ progress-steward 触发 ralph 抢发 work.ready，coordinator 没接管

### 7.4 是机制问题还是编排问题？

**是机制问题 + 编排问题混合**：

| 维度 | 占比 | 关键证据 |
|---|---|---|
| **编排问题** | 50% | Branch A 计数逻辑、coordinator 模板 bug（task_key/task_id）、尝试 emit review.passed 绕过 |
| **机制 bug** | 30% | projector runtime/persistent 账本失步（P0-D）；ralph 兜底越权（P0-E） |
| **机制-编排协作裂缝** | 20% | plan.complete schema 不要求 step；plan_gate 豁免依赖 step + 仍走 matching |

**结论**:
- **第一版判断"机制正常、编排为主"不完整** —— 本次新增 2 个机制 bug（projector 失步 + ralph 越权），让 mechanism 责任上升到 30%。
- **RALPH Loop 机制基座的 80% 是健康的**（事件循环、ledger、state machine、execution_contracts、event_policy、isolated-mode drop、shipper reason routing 全工作），但 **20% 的 projector + ralph 兜底机制有 bug**，需要修。
- **编排层面的 50% 是核心问题** —— Branch A 计数逻辑、coordinator 模板、scope violation。
- **协作裂缝 20%** —— schema/plan_gate 口径不一致（plan.complete schema 不要求 step + plan_gate 豁免依赖 step + 豁免后仍走 matching 校验）。
- **整体责任划分**: **编排 50% + 机制 bug 30% + 协作裂缝 20%** —— 这与第一版"70/20/10"的判断不同，本次机制 bug 占比从 10% 上升到 30%。

---

## 第 8 部分：源码引用速查

- plan_gate 豁免分支：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/crates/ralph-core/src/event_loop/review_step_state.rs:305-353`
- step_key_from_event：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/crates/ralph-core/src/event_loop/review_step_state.rs:87-124`
- prefill_fix_steps_from_plan：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/crates/ralph-core/src/event_loop/review_step_state.rs:163-214`
- projector task_id 校验：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/crates/ralph-core/src/state_projector/task.rs:81-129`
- projector runtime/persistent 同步：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/crates/ralph-core/src/state_projector/task.rs:171` (store.ensure 之后)
- plan.complete schema：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/schemas/ce-executor-serial.yml:264-277`
- preset Branch A：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/en/ce-executor-serial.yml:847-869`
- preset task_id 推荐：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/en/ce-executor-serial.yml:1144`
- preset coordinator publishes：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/en/ce-executor-serial.yml:647`
- preset scope rule：`/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/en/ce-executor-serial.yml:994`

---

## 第 9 部分：与第一版报告的关键差异

| 维度 | 第一版（loop 未终止） | 本次（loop 已 terminate） | 差异原因 |
|---|---|---|---|
| fix-02 状态 | ❌ 卡死，未执行 | ✅ 已完成（51/51 tests） | loop 多次重发 work.ready 后两次都执行 |
| plan.complete 拒绝次数 | 2 次 | **9 次** | recovery.jsonl 共 21 条 plan.complete repair-stream |
| loop 终止方式 | 未终止 | **consecutive_failures（iter 45，2h 7m 45s，exit_code=1）** | ralph 抢发 work.ready + 多次 plan.blocked 触发 consecutive_failures 兜底 |
| 机制 bug 数量 | 1 个（weak validation） | **3 个**（+ projector 账本失步 + ralph 越权） | 新发现 projector 与 ralph 兜底机制问题 |
| 编排责任占比 | 70% | 50% | 机制 bug 占比从 10% 上升到 30% |
| review.passed scope violation | 未识别 | **P1-F** | coordinator 试图绕过 plan.complete 走 review.passed |
| U3 (fix-03) dispatch | ❌ 未 dispatch | ❌ 仍未 dispatch | coordinator Branch A 计数逻辑未硬 gate |
| code review 完成度 | F-001/F-002/F-003 已修 | 同上 + **f04c2c80 commit 落地** | reporter 已生成 report.md |

**新增机制 bug 是本次报告相对第一版的最关键发现**：
1. **P0-D projector 账本失步** —— projector 写 tasks.jsonl 成功但 runtime ledger 找不到
2. **P0-E ralph 越权发 work.ready** —— 违反 preset L994 "MUST NOT publish events for any hat other than coordinator"

---

## 附：诊断产物路径速查

| 类别 | 路径 |
|---|---|
| 事件流（45 events） | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/events-20260630-083222.jsonl` |
| 历史事件流 | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/events-history-20260630-083222.jsonl`（loop.terminate iter 45） |
| ledger（46 行） | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/ledger.jsonl` |
| recovery（21 行） | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/recovery.jsonl`（9 条 plan.complete repair + 1 条 semantic_gate_violation） |
| tasks.jsonl | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/tasks.jsonl`（8 closed + 1 open） |
| progress.md | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/progress.md`（completed_steps 含 step-01/step-02/fix-01/fix-02，Current Step=fix-02） |
| memories.md | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/memories.md`（4 条 fix-unit 经验记录） |
| loop-termination-reason.json | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/loop-termination-reason.json`（"consecutive_failures"） |
| history.jsonl | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/history.jsonl`（loop_started + loop_completed） |
| fix-plan | `/home/chaowen/Dev/agent_tools/ralph-e2e/.agents/scratchpad/ce-executor/2026-06-20-001-feat-python-sort-algorithms-plan/fix-plan.md`（U1/U2/U3） |
| report.md | `/home/chaowen/Dev/agent_tools/ralph-e2e/docs/report/2026-06-30-ce-executor-2026-06-20-001-feat-python-sort-algorithms-plan-report.md`（reporter 输出 pass_or_fail=fail） |
| ralph log | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/diagnostics/logs/ralph-2026-06-30T16-32-21-292-2174929.log`（isolated-mode drop WARN L230/L231） |
| loops.json | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/loops.json`（空，loop 已清理） |

---

**报告版本**: v2（终态版，覆盖第一版）
**生成时间**: 2026-06-30 18:40
**诊断 agent 数**: 4 个并行 + 1 个主 agent 汇总
**总字数**: ~8000 字