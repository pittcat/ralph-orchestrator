# 2026-06-29-ce-executor-serial-primary-20260629-032235 链路诊断报告

> 范围:`presets/en/ce-executor-serial.yml` + `ralph-e2e/.ralph/` 中间产物 + 主仓源码
> Loop ID:`primary-20260629-032235`
> 计划:`2026-06-20-001-feat-python-sort-algorithms`(4 个 step)
> 重审版本:第 2 稿(已纠正首次审查的归因错误)

---

## 1. 结论摘要

**本次 run 的健康度**:**结构性 broken,但 orchestrator 的兜底机制勉强把 plan 推到了 hard-fail 收尾**。4/4 单元闭环、53/53 测试通过,但 review 链被基座机制打断,coordinator 主动发 `plan.blocked`,shipper 收 hard-fail verdict,reporter 写完报告 — 然而 **run 并没有真正停下**,shipper fail 之后 #40/#41/#42 又开了新一轮 review。

**关键异常数量**:**5 个 P0、2 个 P1**。其中 3 个 P0 属基座机制问题,1 个属编排契约漂移,1 个属 scope_violation 报警非拦截。

**是否涉及历史重复问题**:**是**。P0-1 / P0-2 在 2026-06-17 merry-lotus、2026-06-23 ralph-e2e、2026-06-28-115810 / 2026-06-28-172725 多份诊断报告中已识别为相同根因,30 天内 11 次复发。P0-3 / P1-1 / P1-2 属本次新发现。

---

## 2. 执行链路时间线(40 条事件,4 个拐点)

> 数据源:`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/events-20260629-032235.jsonl`(40 行)+ `diagnostics/2026-06-29T11-22-34/recovery.jsonl`(20 条 recovery)+ `ledger.jsonl`(39 条 batch_sync)

| # | ts (UTC) | hat | topic | recovery.iter | 关键说明 |
|---|---|---|---|---|---|
| 1-24 | 03:22:35~04:19:06 | 8 hats 协作 | work.start→work.ready×3→executor×3→validator×3→review.start→6 dim→review.dimensions.complete | iter 3/4 | step-01/02/03 闭环 ✅,step-03 review 6 维全 done |
| **25** | **04:19:47** | review-coordinator | **review.dimensions.complete** | **iter=24** stage=FlowStepScope reason=`flow_unknown_emit` | **🔴 拐点①:被 FlowStepScope 拒收,review-synthesizer 永不激活** |
| 26 | 04:20:44 | progress-steward | task.resume(plan_complete_not_emitted) | iter=25 stall_recovery 注入 | **🔴 拐点②:600s 无事件兜底,触发 feedback 环** |
| 27 | 04:20:44 | progress-steward | task.resume(同上,12ms 重复) | iter=26 outcome=Recovered | **第 2 次重发,未走 preset 注释规定的 2-retry 升级** |
| **28** | **04:22:14** | coordinator | **work.ready(step-04)** | iter=27 | **🔴 拐点③:跳过 review-synthesizer/fix-unit 链,直接进 step-04** |
| 29 | 04:25:25 | executor | work.done(step-04) | – | |
| 30 | 04:25:43 | validator | task.resume(missing_event_gate) | iter=29 | `original_trigger_payload` 引用 `dimension-reviewer.scope_violation` |
| 31 | 04:26:35 | validator | test.passed(53/53) | iter=30 Recovered | |
| 32 | 04:27:23 | coordinator | review.start(step-04) | iter=31 | |
| 33 | 04:28:45 | review-coordinator | review.dimension.ready(goal-alignment) | – | |
| 34 | 04:30:24 | dimension-reviewer | review.dimension.done(goal-alignment) | – | |
| 35 | 04:30:42 | progress-steward | task.resume(missing_event_gate)**→ self** | iter=34 | **preset 注释明确禁止 target_hat=progress-steward(self)** |
| 36 | 04:31:14 | progress-steward | task.resume(plan_complete_not_emitted) | iter=35 | **第 4+ 次重发,仍未升级** |
| **37** | **04:32:53** | **coordinator** | **plan.blocked(reason=review_never_completed_scope_violation_blocked_review_coordinator)** | – | **🔴 拐点④:主动 block 收尾** |
| 38 | 04:34:06 | shipper | REVIEW_COMPLETE(verdict=fail) | – | shipper fail 收尾 |
| 39 | 04:35:22 | reporter | report.done(verdict=fail) | – | reporter 写报告 |
| **40** | **04:36:09** | **review-coordinator** | **review.dimension.ready(correctness)** | iter=39 | **🔴 拐点⑤:shipper fail 后 run 仍推进新 review** |
| 41-42 | 04:37:47/04:38:25 | dimension-reviewer/review-coordinator | done/ready(testing) | iter=40/41 | run 未停,新维度继续推进 |

**任务状态**(`agent/tasks.jsonl`):4/4 step task 全部 `closed`(`task-1782703477-c875` / `task-1782704376-69dd` / `task-1782704774-92fd` / `task-1782707109-fd84`),`from_key:` 占位 task 残留 `open` 未 close。

**进度文件**(`agent/progress.md`):`Completed Steps = [step-01, step-02, step-03, step-04]`,`Current Step = (none)`。

---

## 3. 历史问题上下文

> 数据源:`docs/report/` 30 份诊断(2026-06-03~2026-06-29)

### 3.1 历史已识别的同根问题

| 历史案例 | 报告路径 | 与本次关联度 |
|---|---|---|
| `2026-06-28-172725` 早班 — review 链全程未启动,`recovery_exhausted` 杀 loop 不走 plan.blocked | `docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-diagnosis.md` | **高**(同样落入 `flow_unknown_emit` 拒绝 + stall_recovery 升级路径) |
| `2026-06-28-115810` 早班 — dimension-reviewer 改 plan.md scope_violation | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md` | **高**(本次 #30 `original_trigger_payload` 同源 scope_violation) |
| `2026-06-17-merry-lotus` — review-coordinator 13s 重复 ready,R5 isolated-mode drop 第二次 | `docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md` | **中**(本次是基座 stage reject 而非 isolated-mode) |
| `2026-06-17-noble-peacock` — task.resume 路由 dead branch,coordinator 死循环 | `docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md` | **中**(本次 #35 progress-steward→self 死路径同源) |
| `2026-06-23-mechanism-review-layer3` — 反模式 2 累计 5+ 次:hard gate / responder 双轨 | `docs/report/2026-06-23-mechanism-review-layer3-history-patterns.md` | **高**(本次 stall_recovery + missing_event_gate 双轨同时跑) |

### 3.2 本次新增(无历史 case)

- `flow_lifecycle.current_step_id()` 字段语义错位(返回 `record.source_topic` 而非 step id)→ **拐点①根因**
- `stall_recovery` 注入端 retry_attempt 计数未落地,2-retry cap 升级门缺失 → **拐点②④根因**
- `plan.blocked` 后终态机未拦截 review-coordinator 继续推进 → **拐点⑤根因**
- `dimension-reviewer` 写 plan.md 触发 scope_violation 报警后未拒绝事件,事件仍落盘 → 拐点④ reason 含 scope_violation 前缀

---

## 4. 证据清单(具体文件 + 行号 + 事件 ID)

| 证据 ID | 路径:行号 / iter | 内容 | 关键字段 |
|---|---|---|---|
| E-1 | `events-20260629-032235.jsonl:25` | `review.dimensions.complete` payload 含 6 dim status=done, fix_round=0 | `source="review-coordinator"` `topic="review.dimensions.complete"` `triggered="ralph"` |
| E-2 | `diagnostics/2026-06-29T11-22-34/recovery.jsonl:iter=24` | `stage=FlowStepScope reason=flow_unknown_emit topic=review.dimensions.complete` | `retry_key=cli_emit:*:review_dimensions_complete:flow_unknown_emit:flowstepscope` |
| E-3 | `presets/en/ce-executor-serial.yml:75-130` | `mechanism.flow.steps[0].unit_loop.allowed_emits = [work.ready, work.done, work.failed, test.passed, test.failed, fix.applied, fix.exhausted]` | **无 `review.dimensions.complete`** |
| E-4 | `crates/ralph-core/src/flow_lifecycle.rs:453` | `current_step_id()` 返回 `record.source_topic` 而非 step id,unit_loop 永远非终态 | line 453-460 |
| E-5 | `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs:40-58` | `DEFENSIVE_BYPASS` 列表含 `("review-coordinator", "review.dimensions.complete")` | line 54-55 |
| E-6 | `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs:124-132` | bypass 匹配 `if let Some(source) = event.source.as_ref() { ... }` | line 124-132 |
| E-7 | `events-20260629-032235.jsonl:26,27` | progress-steward `task.resume(plan_complete_not_emitted)` 同 ts 12ms 间隔双发 | `target=coordinator` |
| E-8 | `presets/en/ce-executor-serial.yml:2697` | `progress-steward.triggers = ["loop.stalled"]` | **不包含 `task.resume`** |
| E-9 | `presets/en/ce-executor-serial.yml:2710-2770` | progress-steward instruction 规定 2-retry cap 升级 `plan.blocked(reason=xxx_unrecoverable_after_<N>_retries)` | 2-retry cap 硬约束 |
| E-10 | `events-20260629-032235.jsonl:30` | validator `task.resume(missing_event_gate)` `original_trigger_payload` 引用 `dimension-reviewer.scope_violation` 改 plan.md | 1 file, 1 insertion(+), 1 deletion(-) |
| E-11 | `events-20260629-032235.jsonl:35` | progress-steward `task.resume(missing_event_gate)→self` | `target=progress-steward` |
| E-12 | `events-20260629-032235.jsonl:37` | coordinator `plan.blocked(reason=review_never_completed_scope_violation_blocked_review_coordinator)` | reason 整段含 scope_violation |
| E-13 | `events-20260629-032235.jsonl:40,41,42` | shipper fail verdict (#38) 后 review-coordinator 仍推 correctness / testing 维度 | seq 41 还在 +1 |
| E-14 | `presets/en/ce-executor-serial.yml:638` | `coordinator.triggers = [work.start, task.resume, test.passed, review.complete, work.failed]` | **不包含 `review.dimensions.complete`** |
| E-15 | `presets/en/ce-executor-serial.yml:2000` | `review-synthesizer.triggers = [review.dimensions.complete]` | review-synthesizer 失活后无其他激活路径 |
| E-16 | `crates/ralph-core/src/event_loop/mod.rs:9788` | 每步读 `current_step_id()` 全程 fallback `"unit_loop"` | 隐式 unit_loop 卡死 |

---

## 5. 问题归因表(P0 / P1 / P2)

| 优先级 | 问题描述 | 根因分类 | 证据 | 历史关联 |
|---|---|---|---|---|
| **P0-1** | review-coordinator #25 发的 `review.dimensions.complete` 被 FlowStepScope stage 拒绝(reason=`flow_unknown_emit`),DEFENSIVE_BYPASS 未生效,导致 review-synthesizer 永不激活 | **B Ralph 基座** | E-1 / E-2 / E-3 / E-4 / E-5 / E-6 / E-16 | 是(`2026-06-28-172725-diagnosis.md` 第 11 次复发) |
| **P0-2** | progress-steward 重发 task.resume 4+ 次(#26/#27/#35/#36),未按 preset 注释规定走 2-retry 升级路径 | **B 基座 + D 行为契约漂移** | E-7 / E-8 / E-9 | 是(`review-coordinator-isolated-scope-recovery.md`) |
| **P0-3** | `dimension-reviewer` 改 plan.md 触发 `scope_violation` 报警后,事件仍落盘并最终污染 `plan.blocked` reason 整段(`scope_violation_blocked_review_coordinator`) | **A preset 内容 + C 编排** | E-10 / E-12 | 是(`2026-06-28-115810-diagnosis.md` 同 mode 复发) |
| **P0-4** | progress-steward #35 把 `task.resume` 发给自己(target=progress-steward),preset 注释明确禁止此 dead path | **D 行为契约漂移** | E-8 / E-11 | 是(`task-resume-target-hat-dead-path.md` 死路径同源) |
| **P0-5** | step-03 review-synthesizer 失活后,coordinator #28 推 `work.ready(step-04)` 跳过 fix-unit 链,review-synthesizer 缺席被无视 | **C 编排 + A preset** | E-1 / E-14 / E-15 | 否(本次新发现) |
| **P1-1** | shipper fail verdict (#38) + reporter done (#39) 之后,#40/#41/#42 又开新一轮 review,run 未真正停下 | **B Ralph 基座** | E-13 | 否(本次新发现) |
| **P1-2** | validator hard gate(#30)与 progress-steward hard gate(#35)双轨同时跑,target_hat 互相冲突(target=validator vs target=progress-steward) | **B 基座** | E-10 / E-11 | 是(`2026-06-23-mechanism-review-layer3` 反模式 2) |

---

## 6. 修复建议(按 P0 → P1 排序)

### 修复 1(P0-1)`flow_lifecycle.current_step_id()` 字段语义错位

- **目标**:`crates/ralph-core/src/flow_lifecycle.rs:453`
- **根因**:`current_step_id()` 返回 `record.source_topic`(事件 topic 名)而非 step id(`unit_loop` / `review_walk` / `plan_end` / `ship`),unit_loop record 始终非终态,导致 step 推进从未注册,`current_step.id` 永远 fallback 到 `"unit_loop"`。
- **修改**:
  - 引入独立 `current_step_id` 字段,从 `flow_transition()` 时显式更新;
  - 或在 `event_loop/mod.rs:9788` 调用处补 `if current_step == "unit_loop" { try advance to review_walk }` 显式推进;
  - 加 BDD scenario 验证:在 step-03 review.dimensions.complete 落盘后,`current_step_id` 应返回 `review_walk`。
- **预期**:`DEFENSIVE_BYPASS` 不再被卡;bypass 列表可在后续 commit 清理(`flow_step_scope_stage.rs:40-58` 注释 line 31-37 提到"U4 will replace most of these naturally")。

### 修复 2(P0-2)`stall_recovery` retry cap 升级门缺失

- **目标**:`crates/ralph-core/src/event_loop/stages/stall_recovery_stage.rs`(待定位)+ `presets/en/ce-executor-serial.yml:2710-2770`
- **根因**:stall_recovery 注入 `task.resume` 后,progress-steward 重发 4+ 次仍未升级 `plan.blocked(reason=xxx_unrecoverable_after_2_retries)`。`recovery.jsonl` 仅 outcome Recovered/Pending 翻转,从无升级判定。
- **修改**:
  - stall_recovery 注入端加 `retry_attempt` 计数(per-retry_key 维度);
  - 当 `retry_attempt > 2` 时,直接 emit `plan.blocked(reason=<hat>_unrecoverable_after_2_retries)`,**不再**让 progress-steward 介入;
  - 同步更新 `recovery.jsonl` 字段 schema(已有 `retry_attempt` 字段)。
- **预期**:#35/#36 不再发生;#37 由 stall 层而非 coordinator 主动推断触发(reason 来源可追溯)。

### 修复 3(P0-3)`dimension-reviewer` 写 plan.md 硬拒绝

- **目标**:`crates/ralph-core/src/preset_lint/scope_violation.rs`(待定位)
- **根因**:scope_violation 拦截器只 emit `dimension-reviewer.scope_violation` 事件,事件**仍落盘**;下游 #30/#35 引用此事件 original_trigger_payload,污染 #37 plan.blocked reason。
- **修改**:
  - `scope_violation` 拦截器对 `dimension-reviewer` 写 `docs/plans/*.md` 改为 `bail` 而非 `record WARN`;
  - 在 preset_lint 阶段就把 `dimension-reviewer.allowed_write_paths` 强制收窄为 `[]`(或仅 review 输出目录)。
- **预期**:#30/#35 original_trigger_payload 不会再出现 `dimension-reviewer.scope_violation`;#37 reason 不会带 scope_violation 前缀。

### 修复 4(P0-4)progress-steward target_hat self-loop 死路径防御

- **目标**:`crates/ralph-core/src/event_loop/stages/stall_recovery_stage.rs` + `presets/en/ce-executor-serial.yml:2697`
- **根因**:progress-steward 在 missing_event_gate 兜底中把 `task.resume` 发给自己(target=progress-steward),preset 注释明确禁止此 dead path。但**该 hat 的 `triggers = ["loop.stalled"]` 不包含 `task.resume`**(line 2697),所以 task.resume 根本不应激活 progress-steward。
- **修改**:
  - `task.resume` 注入端加 guard:`target_hat` 不能等于 `source_hat` 或 `missing_event_gate` 的 `source_hat`;
  - 或在 `EventOriginGuard` / `EventPolicy` 阶段对 `target == source` 拒绝。
- **预期**:#35 task.resume(missing_event_gate)→self 不会再发生。

### 修复 5(P0-5)coordinator 越级推 work.ready(step-N+1) 拦截

- **目标**:`presets/en/ce-executor-serial.yml:638` 或 `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs:48-69`
- **根因**:coordinator `triggers` 不包含 `review.dimensions.complete`(E-14),review-synthesizer 失活时 coordinator 无视上游 broken 继续推 `work.ready(step-04)`。
- **修改**:
  - 方案 A:coordinator `triggers` 增 `review.dimensions.complete`(作为禁用信号,emit 自身 work.ready 前检查);
  - 方案 B:`DEFENSIVE_BYPASS` 把 `("coordinator", "work.ready")` 限定 `current_step ∈ {unit_loop, plan_end}`。
- **预期**:review-synthesizer 失活时 coordinator 不会越级发 step-N+1 的 work.ready。

### 修复 6(P1-1)shipper fail 后 review 链仍推进拦截

- **目标**:`crates/ralph-core/src/event_loop/mod.rs`(终态机 `drive_step_close_progress`)+ `presets/en/ce-executor-serial.yml`
- **根因**:`plan.blocked` + `REVIEW_COMPLETE(verdict=fail)` + `report.done` 之后,`flow_lifecycle` 全部 record 已终态,但 review-coordinator 仍被 residual 任务调度推进。`DEFENSIVE_BYPASS` 不应豁免终态后的 review 链。
- **修改**:
  - `drive_step_close_progress` 加 guard:若 `flow_lifecycle.phase == Closed/Failed`,跳过 review-coordinator 推进;
  - `EventPolicy` 增 `flow_state.closed` 后 reject `review.dimension.*` 事件。
- **预期**:seq=40/41/42 不再出现;run 在 #39 reporter 之后真正停。

### 修复 7(P1-2)hard gate 双轨 target_hat 冲突

- **目标**:`crates/ralph-core/src/event_loop/stages/missing_event_gate_stage.rs`(待定位)
- **根因**:validator hard gate (#30) 与 progress-steward hard gate (#35) 同一时间触发,target_hat 互相冲突(target=validator vs target=progress-steward),retry_key 互不感知。
- **修改**:
  - hard gate 注入端用 typed `RejectionKind` + `LintResumeHint::from_typed_rejection`(`gate.rs:509-531`)替代字符串匹配;
  - `compute_retry_key(kind)` 签名同步更新;
  - `event_loop/mod.rs:6074` 的 `from_reason` 字符串路径迁到 typed enum。
- **预期**:两个 hard gate 共用 retry_key 计数,不会重发覆盖。

---

## 7. 长期架构建议

1. **step 推进状态机显式化**:本次 P0-1 / P0-5 共同根因是 `flow_lifecycle` 的 step 推进没显式。建议把 `current_step` 改为受 type-state 约束的状态机:`unit_loop → review_walk → plan_end → ship`,每次 transition 必须 emit 显式 `flow.transition` 事件,BDD scenario 覆盖。

2. **DEFENSIVE_BYPASS 临时白名单收敛**:`flow_step_scope_stage.rs:40-58` 的 bypass 列表本应是"U4 落地前的过渡"(`crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs:23-39` 注释明确说"U4 will replace most of these"),但 U4 未落地 → bypass 长期化。**建议把 U4 拆分独立 plan 落地,移除临时白名单**。

3. **recovery 注入端与 agent 行为分离**:`stall_recovery` / `missing_event_gate` / `progress-steward` 三套兜底机制(分属 recovery_runtime / event_loop / progress-steward hat)目前共享兜底意图但**互相覆盖**(本次 #26 重发 + #35 self-loop)。建议引入"recovery-injection decision table"做单一职责。

---

## 8. 约束与开放问题

- `recovery.jsonl iter=40/41` 提示 run 还在推进(可能 event #40 后又开新 review),本次诊断截止 04:38:25。**若 run 未主动停,实际产物污染会持续**,建议先 `ralph loops clean` 再排错。
- `progress.md` 残留 4 条 `from_key:` 占位 task 未 close(`agent/tasks.jsonl` line 1/3/5/7),需在 `task` CLI 增加 from_key 清理命令。
- `loops.json` 显示 `loop.id=primary-20260629-032235` 但 ledger iter 已到 39,`recovery.jsonl` 还在写,说明子进程未退出,本报告只能基于 04:38 之前的事件做归因。
