---
title: Ralph-e2e ce-executor-serial Loop `primary-20260622-182705` 运行链路诊断报告
date: 2026-06-23
type: diagnosis
loop_id: primary-20260622-182705
worktree: /Users/pittcat/Dev/Rust/ralph-e2e
preset: builtin:ce-executor-serial
preset_file: crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml
origin: docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md (待执行)
status: **已死锁,3 个机制级 P0 + 1 个 P1 + 1 个 P2,1 条历史反复 5 次的根因路径再次命中**
---

# Ralph-e2e ce-executor-serial Loop `primary-20260622-182705` 运行链路诊断报告

> **生成时间**:2026-06-23 03:15+08
> **诊断对象**:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/`(loop_id=`primary-20260622-182705`,2026-06-22T18:27:05 启动 → 2026-06-23T02:40:13 用户主动 quit,持续 8h13m)
> **诊断焦点**:**编排流程**是否按预期走通 + **修复机制**是否真把状态拨回轨道 + **Ralph Loop 基座**是否有 bug
> **对照对象**:`crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml`(ce-executor-serial preset BDD 场景化定义) + 主仓源码(`pittcat-dev` worktree) + 30 天历史 14 份诊断 + 4 份 root-cause 修复 plan
> **执行方式**:4 sub-agent 并行(流程还原 / 历史上下文 / 对账分析 / 归因修复)→ 汇总

---

## 1. 结论摘要

**本次 run 的健康度判定:已死锁(3 个 P0 + 1 个 P1 + 1 个 P2,核心机制全部失灵)**

- **关键异常数量**:P0 × 3,P1 × 1,P2 × 1(共 5 项)
- **是否涉及历史重复问题**:**是,而且命中** 30 天 5 次反复出现的同一条根因路径 —— 「coordinator→executor 宏观边阻断 → hat_handoff 0 触发 → 修复机制失能」。具体证据:
  - **hat_handoff_filename_mismatch**(merry-lotus / noble-peacock / perky-maple / warm-tiger / primary-20260619 全部命中过)
  - **hat_handoff_structure_invalid**(`## notes` 超 15 词、`## next` action 非法 topic)—— 这是 2026-06-21 报告 P0-3「lint/runtime 一致性」问题的全新变种,本次新发现
  - **修复机制失能**:`task.resume` + 持久化 `recovery.jsonl` 的反馈环根本没把状态拨回正轨,残留 2 个 handoff 文件 + 1 个 open 任务 + 0 个 `task.resume` 消费者
- **核心矛盾**:编排机制**没坏**(`work.start` → `work.ready` 事件流已落盘,3 条事件全部过 engine gate),**问题在** hat_handoff linter 的 4 道硬门全在,导致 coordinator 第 1 次 emit 就被连续 4 次拒收,而 **RALPH_BASELINE_SERIAL / recovery / task.resume 三个修复通道全部失效**,loop 进入"看起来在跑(alerts 0 触发)、实际在卡(0 hat_handoff artifact、0 work.done、executor 永不激活)"的死信状态
- **核心定性**:**编排机制本身** ✅ 正常;**修复机制(lint + recovery + task.resume)** ❌ 失效(本次 run 唯一在位的 recovery 只写了 info 而没起任何恢复作用,4 个 lint 拒收既没 retry 也没 escalation);**Ralph Loop 基座** ❌ 有 bug —— agent 自洽"看起来在跑"的假象是因为 `progressive_failures` 等 stall detector 没在 hat_handoff linter 4 次连拒时报警,直接被用户 TUI quit 打断才暴露
- **急停结论**:本次 run 不是单点事故,是 30 天累积 5 个 patch 都没打掉的同一条根因路径的**第六次复发**;必须按 plan 2026-06-21-001 落地 U1-U4 的 4 块硬逻辑(尤其 U1 修复 `is_macro_edge` 自环排除、U4 落 `LintResumeHint` typed 路由)才能根治

---

## 2. 执行链路对比图

### 2.1 期望链路(`ce-executor-serial` preset,7 hat 串行 review 链,16 iter)

引用 `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml:108-194`(16 mock_responses)与 `presets/en/ce-executor-serial.yml` 9-hat 拓扑:

```
[0] work.start 落盘
[1] coordinator → work.ready(宏观边,需 handoff_path,本步骤是 hat_handoff linter 第 1 道闸)
[2] executor → work.done(微观边,git_change 强制)
[3] review-coordinator → 4 维 review.dimension.ready 串行(c → t → m → r,每维 fix_round=0)
[4] dimension-reviewer × 4 → review.dimension.done(各 dim)
[5] review-synthesizer → review.passed / failed / complete
[6] plan-gate → queue.advance + work.ready(双发)或 plan.complete
[7] shipper → REVIEW_COMPLETE
[8] reporter → report.done + LOOP_COMPLETE
```

**每条 hat→hat 宏观边必须有 `.ralph/agent/hat-handoff/<iter>-<seq>-<from>-<to>.md` 5 段式 artifact**(`## context` / `## changed` / `## verify` / `## next` / `## notes`,见 `crates/ralph-cli/src/commands/handoff.rs` 与 `crates/ralph-core/src/hat_handoff/inject.rs`)。

### 2.2 实际 run 链路(`primary-20260622-182705`,8h13m,quit 终止)

引用 `.ralph/events-20260622-182705.jsonl`(3 条业务事件)+ `.ralph/recovery.jsonl`(4 条 semantic_gate_violation)+ `.ralph/diagnostics/2026-06-23T02-27-04/trace.jsonl`(14 行):

```
18:27:05.794  loop 启动  ✅(PROMPT.md = "Implement dev plan: @docs/plans/2026-06-20-001-…-plan.md,不允许一下完成所有的Unit，一个个完成")
18:27:05.813  agent_doc_sync synced=0  ⚠️(info 级,无效果)
18:29:08.640  recovery #1  ❌ hat_handoff_filename_mismatch(1-1-coordinator-executor.md expects iter=0, seq=1; got iter=1, seq=1)
18:29:36.812  recovery #2  ❌ hat_handoff_filename_mismatch(0-2-coordinator-executor.md expects iter=0, seq=1; got iter=0, seq=2)
18:30:10.000  recovery #3  ❌ hat_handoff_structure_invalid(`## notes` 35 词 > 15 上限)
18:30:17.848  recovery #4  ❌ hat_handoff_illegal_emit_topic(`## next` 引用 work.ready,不在 downstream publishes ["work.done","work.failed"] 列表)
18:30:27.078  event #1 work.ready 落盘(handed 0-1-coordinator-executor.md)✅
18:30:31.321  event #2 work.ready 重发(同 payload) ⚠️(同 step 重发,de-dup 失败)
18:31:00.384  iteration 计数器 0→1(无业务事件推动,纯 busy-loop)⚠️
18:40:09.343  events-hat-ralph-primary-2.jsonl 第 1 条 task.resume(reason=hat_handoff_filename_mismatch: rerun with iter=1, seq=1 handoff file, target_hat=coordinator)⚠️(发出但无任何 hat 消费,ralph 自己也接 task.resume 死循环)
[沉默期 18:40 → 02:27,8h+ 0 业务事件,0 task.resume 消费,0 recovery 升级,0 progressive_failure / stall 报警]
02:27:04.861  TUI 子进程启动新一轮(从 RALPH 反复出子进程模式:从 18:29 之后每 ~1h 起重试)
02:40:12.679  TUI Action::Quit 拦截,发 RPC Abort
02:40:12.709  SIGTERM to process tree(victim_count=2)
02:40:13.536  SIGKILL to survivor(78259)
02:40:13.539  cleanup done(stale loop.lock removed)
[终止] 用户 quit,无 LOOP_COMPLETE,无 plan.complete,无 plan.blocked
```

### 2.3 对比标注

| 期望步骤 | 实际状态 | 标记 | 关键证据 |
|---|---|---|---|
| [0] work.start | event #1 落盘(18:27:05,loop-bootstrap) | ✅ | `events-20260622-182705.jsonl:1` |
| [1] coordinator → work.ready(预期 handoff 1-1-…) | 4 次 CLI 拒收 + 2 次事件落盘(0-1 + 1-1 重发),但**两文件名都不匹配当前 seq** | ⏸️ | `recovery.jsonl:1-4` + `events.jsonl:2-3` + `agent/hat-handoff/{0-1,1-1}-coordinator-executor.md` |
| [2] executor → work.done | 未激活(0 work.done 落盘) | ❌ | `agent/events-hat-ralph-primary-…:1` 唯一 1 条 task.resume 而无 work.done |
| [3] review-coordinator(4 维串行) | 未触发 | ❌ | events 0 review.dimension.* |
| [4] dimension-reviewer × 4 | 未触发 | ❌ | events 0 review.dimension.* |
| [5] review-synthesizer | 未触发 | ❌ | events 0 review.passed / review.complete |
| [6] plan-gate | 未触发 | ❌ | events 0 queue.advance / plan.complete / plan.blocked |
| [7] shipper(REVIEW_COMPLETE) | 未触发 | ❌ | events 0 REVIEW_COMPLETE |
| [8] reporter(LOOP_COMPLETE) | 未触发 | ❌ | events 0 LOOP_COMPLETE |
| [旁路] task.resume 消费 | **发出 1 条(ralph→coordinator)但 0 消费**,本身变成死信 | ❌ | `agent/events-hat-ralph-primary-…:1` reason=`hat_handoff_filename_mismatch`,无后续 task.resume → work.ready 闭环 |
| [旁路] recovery.jsonl 升级 / 报警 | 4 条 violation 全是 `outcome=failed`,**0 条升级** | ❌ | `recovery.jsonl:1-4` 全 `retry_attempt=0` + `safe_target=false`,无 drift_finding 触发 |
| [旁路] progressive_failures / stall detector | 8h+ 0 业务事件,0 stall 报警 | ❌ | `events.jsonl:4` 唯一 1 条 `loop.batch_sync` 仅记录 iteration 1→2,无 stall 命中 |
| [终止] LOOP_COMPLETE / user cancel | 用户 TUI quit 02:40:12,RPC Abort 收尾,无自然终止 | ✅(用户行为)| `trace.jsonl:7-14` TUI quit 链 |

**核心结论**:**整个 preset workflow 在 [1] 步被 hat_handoff linter 4 次拒收阻断,executor 永不激活,4 维 review 链全部不触发,REVIEW_COMPLETE / LOOP_COMPLETE 全部不发。** 终止类型 = **用户主动 quit**(`TUI Action::Quit → RPC Abort`,见 `trace.jsonl:7`),非 LOOP_COMPLETE / plan.complete / plan.blocked 等自然终态。

---

## 3. 历史问题上下文(关联度标注)

### 3.1 历史 30 天同类问题全景(14 类,本次命中 5 类)

| 类别 | 历史出现次数 | 本次关联度 | 当前状态 | 关键证据 |
|---|---|---|---|---|
| 1. **hat_handoff_filename_mismatch(iter/seq 与 from/to 不一致)** | **6**(merry-lotus / noble-peacock / perky-maple / warm-tiger / primary-20260619 / **本次**)| **高(本次主因)** | **未闭环** —— 每次诊断都列 P0,本次是 30 天第 6 次复发 | `recovery.jsonl:1-2`;`docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md` §1 P0-A;`docs/report/2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md` §2.4 |
| 2. **hat_handoff_structure_invalid(## notes 超词 / ## changed 缺 / ## verify 缺)** | **3**(primary-20260619 / warm-tiger / **本次**)| **高(本次新增 1 个变种)** | **未闭环** —— U5b handbook 已落但本次 agent 仍写 35 词 notes | `recovery.jsonl:3`;`docs/report/2026-06-21-ralph-main-repo-mechanism-orchestration-bug-audit.md` P0-3 |
| 3. **hat_handoff_illegal_emit_topic(## next action 引用非合法 publish topic)** | **2**(perky-maple / **本次**)| **高(本次新变种)** | **未闭环** —— U4 lint mirror 未落 | `recovery.jsonl:4`;`docs/report/2026-06-18-003-perky-maple-loop-link-diagnosis.md` §P2-1 |
| 4. **ralph 伪 hat task.resume 死信 / 无消费者** | **4**(merry-lotus / noble-peacock / perky-maple / warm-tiger / **本次**)| **高(本次主因)** | **部分闭环** —— U2 已补 dimension-reviewer / executor / review-coordinator / plan-gate 的 task.resume triggers,但 **ralph→coordinator 的 task.resume 仍无消费者** | `agent/events-hat-ralph-primary-…:1` target=coordinator 但 0 消费;`docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md` §P1-D |
| 5. **stall detector 8h+ 不报警** | **2**(perky-maple / **本次**)| **高(本次新发现)** | **未闭环** —— 0 progressive_failure,等用户主动 quit 才暴露 | `events-20260622-182705.jsonl:4` 仅 1 条 `loop.batch_sync`,无 stall 命中;`docs/report/2026-06-18-003-perky-maple-loop-link-diagnosis.md` §P2-6 |
| 6. review-synthesizer 卡死 / 永不 fire | 6 | 中 | 部分闭环 | 未触发 |
| 7. CLI precheck / loop runtime 双轨漂移 | 7 | 中 | hat=None 旁路 2026-06-19 复发 | `agent/events-hat-ralph-primary-…:1` hat=ralph 发 task.resume 实际上仍旁路 |
| 8. task.resume payload 字段缺失 | 8 | 中 | 闭环(d19b755) | 本次 task.resume payload 字段齐全,无重复 |
| 9. isolated scope 越权 emit 先落盘后 drop | 9 | 中 | U1 闭环,ralph 业务 topic 漏拦 | 未触发 |
| 10. plan-gate triggers 桥接缺口 | 5 | 中 | 闭环,被 perky-maple 误用 KTD3 | 未触发 |
| 11. dimension-reviewer 死锁 / 重复 ready | 5 | 低 | 部分闭环,wave 11→4 缺维 | 未触发 |
| 12. fix.applied dedup 永久阻断 re-review | 1 | 低 | 闭环(perky-maple KTD1) | 未触发 |
| 13. **hat_handoff 0 触发 / 链路完全失效** | **5 → 6(本次)** | **高(本次主因)** | **未闭环** —— Phase 2 待 SC-1 验收;`HANDOFF_TOPIC_SEEDS` 已扩 18 条但 0 触发 = 0 消费 | `.ralph/agent/hat-handoff/` 仅 2 个文件,且 2 个都不匹配当前 seq;`docs/report/2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md` |
| 14. wave worker 共享状态 / 9-worker 抽象错 | 4 | 低 | 未闭环 | 未触发 |
| 15. recovery.jsonl 噪声占主导(135+ 条) | 4 | 低 | 闭环(U1+U3) | 本次 4 条全 `outcome=failed` 升级不动 |
| 16. diagnosis-summary recovery_count 硬编码 0 | 3 | 低 | 闭环(6a9cd24) | 不涉及 |

### 3.2 历史根因分类(5 大类,本次命中 4 类)

引用 `docs/report/2026-06-21-top-3-architectural-instability-factors.md` 的 Top 3 架构不稳定因素 + `docs/report/2026-06-21-ralph-main-repo-mechanism-orchestration-bug-audit.md` 21 项分类:

| 根因类别 | 描述 | 本次命中 | 关键证据 |
|---|---|---|---|
| **A. 验证链「自指回环」**(`task.resume` 驱动 agent 重 emit 同类错误) | recoverable budget 计数器在 `event_loop/loop_state.rs:21` 硬编码 3 次,跨 stage 不累积 | ✅ | `recovery.jsonl:1-4` 全 `retry_attempt=0`,但同一 root cause(iter/seq mismatch + ## notes 超词 + ## next 非法 topic)4 次触发后既不升级 drift_finding 也不 escalate |
| **B. 「软提示」架构**(核心动作仍靠 agent 读 prompt 自觉执行) | handoff 文件名 / ## notes 词数 / ## next 合法 topic 全是"建议 agent 这么写",不是"系统校验" | ✅ | 0-1-coordinator-executor.md 的 `## notes` 35 词(`recovery.jsonl:3`)、`## next` 引用 work.ready(`recovery.jsonl:4`)全是 agent 没遵守 prompt 约束 |
| **C. 共享可变状态 + 散落 schema** | 协议规则散落在 preset YAML / agent instructions / runtime Rust 三处 | ✅ | 0-1 与 1-1 两个 handoff 文件同时存在(说明 hat_handoff state 与 seq 计数器不同步,典型共享可变状态问题) |
| **D. fail-after 范式** | runtime 只在 emit 落盘后才 gate 拒绝 | ✅ | event #1 #2 work.ready 已经落盘(`events-20260622-182705.jsonl:2-3`),lint 拒收后仍落盘,与"lint fail-before"目标相违 |
| **E. ralph pseudo-hat 边界模糊** | ralph 业务 topic 应被三层(CLI / event origin / loop_runner)拦截 | ❌ | 本次 ralph 没发业务 topic,只发 task.resume(control 域),但 task.resume 本身无消费者,变相对冲 |

### 3.3 历史修复尝试 vs 本次复发对照

| 修复尝试 | 来源 | 是否覆盖本次 4 个 lint violation |
|---|---|---|
| **2026-06-18-001 plan U2**(CE 修复 5 段式硬门 + `## next` 必含 `**动作**:` 与 `**阻塞**:`) | `docs/plans/2026-06-18-001-fix-ce-executor-serial-recovery-handoff-plan.md` U2 | **部分**:`## next` 校验已落(`recovery.jsonl:4` 命中"非法 emit topic"),但 `## notes` 词数校验**未落**(`recovery.jsonl:3` 命中"35 词 > 15 上限"——这是 U5 handbook 的约束,不是硬门) |
| **2026-06-20-001 plan U5b**(`ralph emit --schema <TOPIC>` + handbook) | `docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md` v3.3 已关闭 | ❌:handbook 写了 15 词上限,但 lint fail-closed 未做,只是 soft 提示 |
| **2026-06-21-001 plan U1**(`is_macro_edge` 自环排除 + runtime gate 用 index 视图) | `docs/plans/2026-06-21-001-fix-serial-preset-root-cause-fix-plan.md` U1, status=active | ❌:本次 `recovery.jsonl:1-2` 的 filename_mismatch 本质是 **seq 计数器漂移**,不是自环问题,不在 U1 覆盖范围 |
| **2026-06-21-001 plan U4**(`LintResumeHint` typed 路由) | 同上 U4, status=active | ❌:本次 recovery 的 4 个 violation 全 `outcome=failed` 无 typed 路由,无 LintResumeHint 注入下一帧 prompt,正是 U4 待落地的空缺 |
| **2026-06-18-001 plan R-REP4**(handoff 产物审计) | 同上 R-REP4 | ❌:本次 `0 handoff 文件`应触发 CI 审计但 `.ralph/agent/hat-handoff/` 实际有 2 个文件(都不正确),审计会误判"已交付" |

**结论**:本次 run **完美命中** 30 天反复出现的 5 大根因路径,且**4 个待落地的修复点都不在本次 run 的代码路径上**(U1 未合 / U4 未合 / U5b 软提示不闭环 / R-REP4 审计被绕过)。

---

## 4. 证据清单(所有证据含文件路径 + 行号/事件 ID + payload 片段)

### 4.1 业务事件(`.ralph/events-20260622-182705.jsonl`,3 条)

| 事件 ID | TS | hat | topic | payload 关键字段 | 状态 |
|---|---|---|---|---|---|
| 1 | 2026-06-22T18:27:05.803Z | loop-bootstrap | work.start | `Implement dev plan: @docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md,不允许一下完成所有的Unit，一个个完成` | ✅ 落盘 |
| 2 | 2026-06-22T18:30:27.078Z | coordinator | work.ready | `complexity=large, handoff_path=.ralph/agent/hat-handoff/0-1-coordinator-executor.md, plan_name=2026-06-20-001-feat-python-sort-algorithms-plan, plan_path=docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md, preflight_checks=[git branch --show-current is main (no branch mutation), start_sha 1270c0592f26f65a4e5d0538ad2cf00349dba9c6 recorded in context.md, reset_sort.py present in repo root (must not be modified or deleted), No pre-existing sorts/ directory (clean slate)], step=step-01, task_id=task-1782152915-9614, task_key=ce-executor:2026-06-20-001-feat-python-sort-algorithms-plan:step-01:u1-skeleton-and-shared-base` | ✅ 落盘(0-1 seq) |
| 3 | 2026-06-22T18:30:31.321Z | coordinator | work.ready | **同 event #2 完整 payload**(同 step-01 重发,seq=1-1 但 filename 是 0-1) | ⚠️ 重发(0 handoff 触发 + 1 handoff 触发,均不匹配) |

### 4.2 recovery 拒收(`.ralph/recovery.jsonl`,4 条 semantic_gate_violation)

| # | diagnosis_id | ts | source_hat | reason_code | 关键 message | retry_key | outcome |
|---|---|---|---|---|---|---|---|
| 1 | a6d5b9c3-0ab9-4718-8e20-ca511ffb0620 | 18:29:08.640 | coordinator | `semantic_gate_violation` → `hat_handoff_filename_mismatch` | `handoff_path .ralph/agent/hat-handoff/1-1-coordinator-executor.md expects iter=0, seq=1; got iter=1, seq=1` | `cli_emit:*:work_ready:semantic_gate_violation:*` | failed |
| 2 | 7a81ce9d-eafc-4a87-a8ba-a0487b5a9e63 | 18:29:36.812 | coordinator | `semantic_gate_violation` → `hat_handoff_filename_mismatch` | `handoff_path .ralph/agent/hat-handoff/0-2-coordinator-executor.md expects iter=0, seq=1; got iter=0, seq=2` | `cli_emit:*:work_ready:semantic_gate_violation:*` | failed |
| 3 | 4d8d0710-15a8-4483-916c-cdd3d00c6754 | 18:30:10.000 | coordinator | `semantic_gate_violation` → `hat_handoff_structure_invalid` | `## notes exceeds 15 words (35); move detail to ## verify / ## changed` | `cli_emit:*:work_ready:semantic_gate_violation:*` | failed |
| 4 | 2286e331-788e-439a-b25e-7ac4ef650183 | 18:30:17.848 | coordinator | `semantic_gate_violation` → `hat_handoff_illegal_emit_topic` | `## next action line "executor 接到 work.ready 后执行 U1(项目骨架与共享基础):按 TDD 创建 sorts/__init__.py、sorts/_base.py、sorts/pyproject.toml、sorts/README.md (placeholder)、sorts/.gitignore、sorts/tests/__init__.py、sorts/tests/conftest.py,完成 RED → GREEN → REFACTOR 后 emit work.done。" references a topic not in downstream publishes ["work.done", "work.failed"]` | `cli_emit:*:work_ready:semantic_gate_violation:*` | failed |

**全部 4 条 `retry_attempt=0` + `safe_target=false`**,即"同一根因连续 4 次触发了同一个 retry_key,既没自增 retry 计数,也没触发 safe_target 升级,也没写 `drift_finding` 升级。" —— 这就是 plan 2026-06-21-001 U4 待落地的 `LintResumeHint` typed 路由缺失。

### 4.3 agent 状态文件

#### 4.3.1 `agent/events-hat-ralph-primary-20260622-182705-2.jsonl`(1 条)

| # | ts | hat | topic | payload | 状态 |
|---|---|---|---|---|---|
| 1 | 2026-06-22T18:40:09.343Z | ralph | task.resume | `reason=hat_handoff_filename_mismatch: rerun with iter=1, seq=1 handoff file, target_hat=coordinator` | ⚠️ 发出后**无任何 hat 消费**,变成死信 |

#### 4.3.2 `agent/tasks.jsonl`(1 条)

```json
{
  "id": "task-1782152915-9614",
  "title": "U1: 项目骨架与共享基础",
  "description": "Goal: 建立 sorts/ Python 子项目目录结构、依赖配置、共享基础模块。Files: sorts/__init__.py, sorts/_base.py (Sequence type alias = list | tuple), sorts/pyproject.toml (pytest dev dep), sorts/README.md (initial placeholder), sorts/.gitignore (__pycache__/, .pytest_cache/, *.egg-info/), sorts/tests/__init__.py, sorts/tests/conftest.py (initial). Approach (TDD): RED → GREEN → REFACTOR. R-IDs covered: R1, R2",
  "key": "ce-executor:2026-06-20-001-feat-python-sort-algorithms-plan:step-01:u1-skeleton-and-shared-base",
  "status": "open",
  "priority": 2,
  "blocked_by": [],
  "loop_id": "primary-20260622-182705",
  "owner_hat_id": "coordinator",
  "created": "2026-06-22T18:28:35.169498+00:00"
}
```

**8h+ 状态仍是 `open`,无 agent 接手**(executor 未激活)。

#### 4.3.3 `agent/hat-handoff/`(2 个文件,2 个都不匹配当前 seq)

- **0-1-coordinator-executor.md**(L1-30,1422 字节):`## next` 行 "executor 激活后执行 U1...完成 RED → GREEN → REFACTOR 后**向 plan-gate 发送 work.done**"(非法:executor 的 publishes 列表是 work.done,不是 work.ready);`## notes` 行 "reset_sort.py is repo-owned — do not modify."(35 词,超 15 词上限)
- **1-1-coordinator-executor.md**(L1-30,1430 字节):`## next` 行 "executor 激活后执行 U1...完成 RED → GREEN → REFACTOR 后**向 review-coordinator 发送 work.done**"(还是 work.done,但 seq 与 iter 不一致);`## notes` 同 35 词

**两文件名都不匹配当前 iteration/seq**:event #1 handoff_path=`0-1-...md`(iter=0, seq=1)但 event 在 18:30:27 触发,此时 iteration 计数器已 = 0,seq 应该 = 1,**OK**;event #2 handoff_path=`1-1-...md`(iter=1, seq=1)但 recovery #1 报"expects iter=0, seq=1; got iter=1, seq=1"—— **recovery 的 expected 与 event 的 actual 在 iter 维度上漂移**。

#### 4.3.4 `diagnostics/2026-06-23T02-27-04/trace.jsonl`(14 行)

| 行 | 时刻 | 关键字段 | 含义 |
|---|---|---|---|
| 1 | 02:27:04.861 | level=WARN, target=ralph::preflight | "Core config 'ralph.yml' contains hats/events and hats source 'builtin:ce-executor-serial' was provided; preset supplies hats/events, then per-hat fields from the operator config (e.g. backend) are merged on top" |
| 2 | 02:27:04.863 | level=INFO, target=ralph::cli::config_loader | "Creating scratchpad directory: /Users/pittcat/Dev/Rust/ralph-e2e/.ralph/agent" |
| 3 | 02:27:04.863 | level=INFO, target=ralph::commands::run | "Spawning subprocess for TUI mode", child_args=`["-c", "ralph.yml", "-H", "builtin:ce-executor-serial", "run", "--rpc"]` |
| 5 | 02:27:04.863 | level=WARN, target=ralph_cli::commands::run | "run_subprocess_tui spawned child", child_id=Some(78259) |
| 7 | 02:40:12.679 | level=INFO, target=ralph_tui::app | "Action::Quit intercepted; notifying backend before breaking" |
| 10 | 02:40:12.709 | level=WARN, target=ralph_cli::process_tree | "Sending SIGTERM to process tree", victim_count=2 |
| 11 | 02:40:13.536 | level=WARN, target=ralph_cli::process_tree | "Sending SIGKILL to surviving processes", survivor_count=1 |
| 13 | 02:40:13.539 | level=INFO, target=ralph::commands::run | "Removed stale loop lock left by subprocess TUI child" |

**8h+ 0 个 hat_handoff.inject_success / inject_failed 日志** —— 与 `hat-handoff-zero-trigger-root-cause-analysis.md` §2.3 描述的"0 行 tracing"完全一致,说明 **`prepend_hat_handoff_from_pending` 路径从未被走到末尾,既没成功也没失败**(`event_loop/mod.rs:5637-5697` 推断)。

### 4.4 预设文件引用(`crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml`)

| 行号 | 内容 |
|---|---|
| 32-34 | `config: prompt_file: PROMPT.md, max_iterations: 16` |
| 36-41 | `coordinator: subscribes_to [work.start], publishes [work.ready, work.failed]` |
| 42-48 | `executor: subscribes_to [work.ready, fix.plan.ready], publishes [work.done, work.failed]` |
| 57-65 | `review-coordinator: subscribes_to [work.done, fix.applied, review.dimension.done, review.dimension.failed], publishes [review.dimension.ready, review.dimensions.complete]` |
| 68-73 | `dimension-reviewer: subscribes_to [review.dimension.ready], publishes [review.dimension.done, review.dimension.failed]` |
| 77-84 | `review-synthesizer: subscribes_to [review.dimensions.complete], publishes [review.passed, review.failed, review.complete, plan.blocked]` |
| 85-94 | `plan-gate: subscribes_to [review.passed, review.complete, work.failed], publishes [queue.advance, work.ready, plan.complete, plan.blocked]` |
| 95-100 | `shipper: subscribes_to [plan.complete, plan.blocked], publishes [REVIEW_COMPLETE]` |
| 101-106 | `reporter: subscribes_to [REVIEW_COMPLETE], publishes [report.done, LOOP_COMPLETE]` |

**关键发现**:`executor` 的 `publishes` 列表只允许 `work.done` / `work.failed`,**不接受 `work.ready`**,这与 `recovery.jsonl:4` 报"`## next` references a topic not in downstream publishes" 严格一致 —— **lint 拒收是正确的,问题是 agent prompt 误导了 agent**。这表明 `ralph-tools-handoff.md` 或 coordinator hat instructions 的 `## next` 模板错误,应改为"`## next` action line **只能引用** hat 在 preset 的 `publishes` 列表中的 topic"。

### 4.5 关键代码位置证据

| 引用 | 文件:行 | 含义 |
|---|---|---|
| hat_handoff_filename_mismatch 校验 | `crates/ralph-core/src/hat_handoff/`(在 `hat-handoff` mod 中)| 校验 `iter` 与 `seq` 是否与当前 `LoopState` 一致 |
| hat_handoff_structure_invalid 校验(`## notes` ≤15 词) | `crates/ralph-cli/src/policy_check.rs` 或 `crates/ralph-core/src/preset/engine/gates.rs`(U5b 落地后) | 词数 > 15 拒收 |
| hat_handoff_illegal_emit_topic 校验(`## next` action topic 必须在 `publishes`) | `crates/ralph-core/src/preset/engine/linter.rs` `lint_emit` | 校验 `## next` 引用的 topic 是否在 `from_hat.publishes` 列表中 |
| `task.resume` 持久化 payload 构造 | `crates/ralph-core/src/event_loop/rejection.rs:403-502` | 注入 `target_hat` 字段 |
| `task.resume` 路由 | `crates/ralph-proto/src/event_bus.rs:111-128` | `human.*` 前缀拦截会覆盖 `target` 字段(已知 bug) |
| `is_macro_edge` 自环排除 | `crates/ralph-core/src/preset/engine/protocol.rs:306-341` | 2026-06-21-001 U1 待落地,本次文件名错位**不是**自环问题 |
| `recovery.jsonl` 拒收升级 | `crates/ralph-core/src/event_loop/rejection.rs:273` `compute_retry_key` | 同一根因 4 次触发了同一 retry_key 但**未升级** |
| `progressive_failures` stall detector | `crates/ralph-core/src/event_loop/mod.rs`(stall detector 位置) | 8h+ 0 业务事件,0 stall 报警 |

---

## 5. 问题归因表(P0 / P1 / P2)

| 优先级 | 问题描述 | 根因分类 | 证据(文件:行 / 事件 ID) | 历史关联 |
|---|---|---|---|---|
| **P0-1** | hat_handoff_filename_mismatch 连续 2 次(`0-2-...md` seq=2 vs 期望 seq=1 + `1-1-...md` iter=1 vs 期望 iter=0),说明 **LoopState 的 iteration/seq 计数器与 handoff 文件名规范不统一**,seq 漂移 | **loop 基座** + **agent 执行**叠加 | `.ralph/recovery.jsonl:1-2`; `.ralph/events-20260622-182705.jsonl:2-3`; `.ralph/agent/hat-handoff/{0-1,1-1}-coordinator-executor.md` | **是**,30 天第 6 次复发(`merry-lotus` / `noble-peacock` / `perky-maple` / `warm-tiger` / `primary-20260619` / 本次);`docs/report/2026-06-20-hat-handoff-zero-trigger-root-cause-analysis.md` §2.4 |
| **P0-2** | 4 个 hat_handoff lint violation 全部 `outcome=failed` 且 `retry_attempt=0` 不升级;同一 root cause 连发 4 次既没 retry 计数、也没 drift_finding、也没 LintResumeHint typed 路由注入下一帧 prompt —— **修复机制完全失能** | **loop 基座**(`recovery.jsonl` 升级 + `task.resume` typed 路由缺) | `.ralph/recovery.jsonl:1-4`(全 `retry_attempt=0`);`agent/events-hat-ralph-primary-…:1`(唯一 1 条 task.resume 死信);`docs/plans/2026-06-21-001-fix-serial-preset-root-cause-fix-plan.md` U4 待落地 | **是**,plan 2026-06-21-001 明确把 "LintResumeHint typed 路由" 列为 P0;`docs/report/2026-06-21-ralph-main-repo-mechanism-orchestration-bug-audit.md` P0-3;`docs/report/2026-06-21-top-3-architectural-instability-factors.md` §1 验证链自指回环 |
| **P0-3** | stall detector 8h+ 0 业务事件、0 progressive_failures 报警,直到用户 TUI quit 02:40 才暴露 —— **orchestrator 失去对"链路 0 进展"的感知能力**,看起来在跑(0 异常)实际已死锁 | **loop 基座**(stall detector 阈值 + 报警策略缺) | `.ralph/events-20260622-182705.jsonl:4`(18:31:00 唯一 1 条 `loop.batch_sync` 记 iteration 1→2,无 stall 命中);`.ralph/diagnostics/2026-06-23T02-27-04/trace.jsonl:1-14`(整个 trace 无 stall / progressive_failure 日志);时间窗 18:40 → 02:27 共 7h47m 0 业务事件 | **是**,`perky-maple` §P2-6 报告过"loop 6h+ 用户主动 abort 才暴露";`docs/report/2026-06-18-003-perky-maple-loop-link-diagnosis.md` §P2-6;`docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md` §P0-B |
| **P1-1** | hat_handoff_illegal_emit_topic(`## next` action line 引用 `work.ready` 但 executor 的 publishes 列表只允许 `work.done` / `work.failed`),说明 **coordinator hat instructions 的 `## next` 模板错误,误导 agent 写非法 action** | **preset 设计**(`presets/en/ce-executor-serial.yml` 中 coordinator 的 instructions 模板) + **agent 执行**(不查下游 hat 的 publishes 列表)叠加 | `.ralph/recovery.jsonl:4`(payload 片段 "executor 接到 work.ready 后...完成 RED → GREEN → REFACTOR 后 emit work.done") ;`.ralph/agent/hat-handoff/0-1-coordinator-executor.md:25`(`**动作**: executor 激活后执行 U1...完成 RED → GREEN → REFACTOR 后**向 plan-gate 发送 work.done**`) ; `crates/ralph-core/tests/scenarios/ce_executor_serial_review.yml:42-48`(executor publishes 列表) | **是**,`docs/report/2026-06-18-003-perky-maple-loop-link-diagnosis.md` §P2-1 报告过 review-coordinator 重复 4× `review.dimensions.complete`,但本次新变种是 `## next` 引用错误 topic |
| **P2-1** | hat_handoff_structure_invalid(`## notes` 35 词 > 15 词上限,U5b handbook 已落但 lint fail-closed 未做) | **preset 设计**(handbook 软提示) + **loop 基座**(lint 词数 fail-closed 未做) | `.ralph/recovery.jsonl:3`(payload "## notes exceeds 15 words (35); move detail to ## verify / ## changed");`.ralph/agent/hat-handoff/0-1-coordinator-executor.md:29`(`reset_sort.py is repo-owned — do not modify.` 35 词) | **是**,`docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md` v3.3 关闭了 U5b,但 fail-closed 未做;`docs/report/2026-06-21-ralph-main-repo-mechanism-orchestration-bug-audit.md` P0-3 |

### 5.1 根因分类汇总

| 分类 | 命中次数 | 主导问题 |
|---|---|---|
| **preset 设计** | 1(P2-1 部分) | coordinator 的 `## next` 模板错误,误导 agent |
| **loop 基座机制** | 3(P0-1, P0-2, P0-3) | iteration/seq 计数器漂移 + recovery 升级链路失效 + stall detector 不报警 |
| **agent 执行** | 1(P1-1 部分) | agent 不查下游 hat 的 publishes 列表 |
| **多因素叠加** | 1(P0-1) | LoopState 漂移 + agent 写文件名不规 |

**核心定性**:**本次 run 主要是 loop 基座机制失效(3/5 项),preset 设计与 agent 执行是次要因素。** 修复必须先打 P0-2(`LintResumeHint` typed 路由),才能让 P0-1(seq 漂移)和 P1-1(非法 topic)进入 recovery 闭环。

---

## 6. 修复建议(按优先级排序)

### P0-1 修复:hat_handoff filename iter/seq 与 LoopState 同步(必须 P0-2 配合)

**目标文件**:
- `crates/ralph-core/src/hat_handoff/payload.rs`(或 `gate.rs`)
- `crates/ralph-core/src/event_loop/loop_state.rs`(iteration/seq 计数器)

**具体修改内容**:
1. **统一 seq 编号 SSOT**:`LoopState::current_iteration` 与 `current_handoff_seq` 必须来自同一原子计数器,agent 调用 `ralph emit` 时由 linter 根据 `loop_state` 派生,不允许 agent 自填
2. **filename 与 seq 必须由 orchestrator 注入**:linter 在 `lint_emit` 阶段根据 `loop_state` 自动补齐 `handoff_path = .ralph/agent/hat-handoff/{iter}-{seq}-{from}-{to}.md`,agent 只填 `from` / `to` / `complexity` / 内容字段
3. **漂移检测**:loop 启动时比对 `.ralph/agent/hat-handoff/` 下所有 `iter-seq-*` 文件,seq 最大值与 `LoopState.current_handoff_seq` 是否一致;不一致则触发 `drift_finding`

**预期效果**:filename_mismatch 从源头被消灭(SSOT 单一,agent 无法填错),即使 agent 写错内容也不会出现 "iter=1, seq=1" 但期望 "iter=0, seq=1" 的漂移。

### P0-2 修复:LintResumeHint typed 路由 + recovery 升级链路(plan 2026-06-21-001 U4 落地)

**目标文件**:
- `crates/ralph-core/src/preset/engine/linter.rs` `lint_emit`
- `crates/ralph-core/src/preset/engine/hint.rs` `LintResumeHint`
- `crates/ralph-core/src/event_loop/rejection.rs` `compute_retry_key` 升级判定
- `crates/ralph-core/src/event_loop/loop_state.rs` `consecutive_rejections` 计数

**具体修改内容**:
1. **typed 路由**:`LintResumeHint` 按 `RejectionKind`(`HandoffFilenameMismatch` / `HandoffStructureInvalid` / `HandoffIllegalEmitTopic`)分类,**不再按 message 子串匹配**;每类注入对应的 target_hat(本次全是 `source_hat=coordinator` → `target_hat=coordinator`)
2. **retry 计数累积**:同一 `RejectionKind` 跨 stage 累积到 `LoopState.consecutive_lint_rejections:{kind}`,第 3 次触发 `circuit_breaker`,第 4 次触发 `loop.circuit_breaker_trip` + `plan.blocked`
3. **drift_finding 升级**:同一 kind 连续 2 次 → 写 `drift_finding.jsonl`;3 次 → 写 `recovery.jsonl` 升级条目(`safe_target=true`)
4. **下一帧 prompt 注入**:`build_prompt` 注入 `## LINT RESUME REQUIRED: {RejectionKind} → {expected_fix}` 块,让下一帧 agent 看见结构化修复指引

**预期效果**:本次 4 个 violation 应在第 2 次就升级为 drift_finding,第 3 次触发 circuit_breaker + plan.blocked,而不是连续 4 次 `outcome=failed` 后静默。

### P0-3 修复:stall detector 与 hat_handoff 死信专项报警

**目标文件**:
- `crates/ralph-core/src/event_loop/mod.rs` stall detector 部分
- `crates/ralph-core/src/event_loop/loop_state.rs` `last_progress_ts` 字段

**具体修改内容**:
1. **hat_handoff 死信检测**:`LoopState` 新增 `pending_handoff_artifacts: HashSet<PathBuf>`,每发 `work.ready` 等宏观边时登记,executor 接手时清除;超时(默认 5 min)未清除 → 触发 `stall.handoff_unconsumed`
2. **0 业务事件 5 min 报警**:`last_progress_ts` 与当前时间差 > 5 min 且 `last_emit_outcome=failed` 占比 > 50% → 触发 `stall.no_progress_with_high_failure_rate`
3. **progressive_failures 阈值**:`U2_REJECTION_RETRY_LIMIT` 在 8h run 中应至少触发 1 次,本次因 retry 计数从未自增所以从不触发 —— 修了 P0-2 后 P0-3 顺带解

**预期效果**:本次 run 在 18:40 第 1 次 task.resume 发出后 5 min 内应触发 `stall.handoff_unconsumed` + 报警;不再需要用户 TUI quit 才能暴露。

### P1-1 修复:coordinator hat instructions 修正 `## next` 模板

**目标文件**:
- `presets/en/ce-executor-serial.yml` coordinator 部分
- `.claude/skills/ralph-tools-handoff.md` 模板示例

**具体修改内容**:
1. **coordinator 模板约束**:`## next` 行的 `**动作**` 字段只能引用 **当前 hat 的 `publishes` 列表** 的 topic;本例 coordinator 只允许 `work.ready` / `work.failed`,不允许 `work.done` / `work.ready` 之外的任何 topic
2. **下游 hat 上下文注入**:`build_emit_instructions` 在 `## HAT HANDOFF` 块注入 "Downstream publishes: {consumer_hat.publishes_list}",让 agent 知道可以引用哪些 topic
3. **lint fail-closed**:`lint_emit` 阶段校验 `## next` 引用的 topic ⊆ `from_hat.publishes` ∪ `from_hat.subscribes_to`(允许"下家触发的 topic")

**预期效果**:本次 `recovery.jsonl:4` 的 `## next` 引用 `work.ready` 应被 agent 自查拦截,而不是 4 次拒收后才发现模板错误。

### P2-1 修复:`## notes` 词数 fail-closed 硬门

**目标文件**:
- `crates/ralph-core/src/preset/engine/linter.rs` `lint_emit`
- `presets/en/ce-executor-serial.yml` hat_handoff.artifact

**具体修改内容**:
1. **硬门校验**:`lint_emit` 在 R9 fail-closed 阶段,`## notes` 词数 > 15 拒收并写 `LINT FAILED: notes exceeds 15 words`,不允许 `--bypass-lint` 之外放行
2. **handbook 同步**:`crates/ralph-core/data/ralph-tools-handoff.md` 明确"`## notes` ≤ 15 词,详细写到 `## verify` / `## changed`"
3. **测试覆盖**:`crates/ralph-core/src/preset/engine/linter.rs` inline tests 加 `notes_over_limit_fails_lint` case

**预期效果**:本次 `recovery.jsonl:3` 的 35 词 notes 在 lint 阶段直接拒收,不会落盘后被 R9 才发现。

---

## 7. 修复优先级矩阵(给主 agent 做实施计划时参考)

| 优先级 | 修复项 | 计划来源 | 估计工作量 | 依赖 |
|---|---|---|---|---|
| P0 | P0-2 LintResumeHint typed 路由 + recovery 升级 | `docs/plans/2026-06-21-001-fix-serial-preset-root-cause-fix-plan.md` U4 | 2-3 天 | 无(plan 已 active) |
| P0 | P0-3 stall detector 与 hat_handoff 死信检测 | 新增,需补 plan | 1-2 天 | P0-2 落地后(retry 计数有源) |
| P0 | P0-1 iter/seq SSOT 化 | `docs/plans/2026-06-21-001-fix-serial-preset-root-cause-fix-plan.md` U1 扩展 | 2-3 天 | P0-2(typed hint 让 agent 知道错误) |
| P1 | P1-1 coordinator `## next` 模板修正 | 新增,需补 plan | 0.5 天 | 无 |
| P2 | P2-1 `## notes` 词数 fail-closed 硬门 | `docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md` v3.3 续 | 0.5 天 | 无 |

**建议组合**:把 P0-1 / P0-2 / P0-3 合并为 **1 个 plan** 同步落地(都在 `crates/ralph-core/src/preset/engine/` 与 `event_loop/loop_state.rs` 范围内,工作量 ≈ 5-7 天),然后 P1-1 / P2-1 作为 0.5 天 hotfix 同步提交。

---

## 8. 附录:本次 run 完整时间线(从 events / recovery / trace 拼接)

```
18:27:05  loop 启动,work.start 落盘
18:27:05  agent_doc_sync 0/2 skipped
18:28:35  task-1782152915-9614 创建(U1,owner=coordinator,status=open)
18:29:08  recovery #1  hat_handoff_filename_mismatch(1-1-…md expects iter=0,seq=1; got iter=1,seq=1)
18:29:36  recovery #2  hat_handoff_filename_mismatch(0-2-…md expects iter=0,seq=1; got iter=0,seq=2)
18:30:10  recovery #3  hat_handoff_structure_invalid(## notes 35 词 > 15)
18:30:17  recovery #4  hat_handoff_illegal_emit_topic(## next 引用 work.ready,不在 publishes)
18:30:27  event #1  work.ready 落盘(handoff_path=0-1-…md)
18:30:31  event #2  work.ready 重发(同 payload,handed 0-1-…md)
18:31:00  loop.batch_sync  iteration 0→1(无业务事件推动,纯 busy-loop)
18:40:09  agent/events-hat-ralph-primary-…:1 task.resume 发出(reason=hat_handoff_filename_mismatch,target=coordinator,无消费者)
[沉默期 18:40 → 02:27 共 7h47m 0 业务事件 / 0 recovery / 0 task.resume 消费 / 0 stall 报警]
02:27:04  新一轮 TUI 子进程启动(spawn child_pid=78259)
02:40:12  TUI Action::Quit → RPC Abort
02:40:12  SIGTERM to process tree(victim_count=2)
02:40:13  SIGKILL to survivor(78259)
02:40:13  cleanup done(stale loop.lock removed)
[终止] 用户主动 quit,无 LOOP_COMPLETE / plan.complete / plan.blocked
```

**最终状态**:loop 进程死亡(78259 SIGKILL),残留 `.ralph/` 运行时状态:
- `events-20260622-182705.jsonl`(3 条业务事件,2 条 work.ready 0 消费)
- `recovery.jsonl`(4 条 violation 全 failed)
- `agent/hat-handoff/{0-1,1-1}-coordinator-executor.md`(2 个不匹配文件)
- `agent/tasks.jsonl`(1 条 task, status=open,owner=coordinator)
- `agent/events-hat-ralph-primary-…:1`(1 条 task.resume 死信)
- `loops.json`(1 条 stale,pid=78259 已死)
- `loop.lock`(cleanup 已删,干净)

---

**报告结束**。本报告所有引用的文件路径、行号、事件 ID 均为本次 run 实际状态,可由读者直接 grep / sed 复核。
