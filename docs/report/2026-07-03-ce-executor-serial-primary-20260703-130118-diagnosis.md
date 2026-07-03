# ce-executor-serial 运行链路诊断报告 — primary-20260703-130118

> **诊断对象**:`primary-20260703-130118`(`pid` 终态已结束,21 iter / 56m51s / verdict=pass_with_residuals / commit `995098f`)
> **preset**:`presets/en/ce-executor-serial.yml`(10-hat isolated 模式 + progress-steward,共 12 hat)
> **plan**:`docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`
> **诊断时间**:2026-07-03T22:00Z
> **诊断依据**:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/events-20260703-130118.jsonl`(33 行业务事件)+ `ledger.jsonl`(25 行)/ `recovery.jsonl`(6 envelope)+ 12 份历史诊断 + 4 份 active plan + 主仓源码

---

## 1. 结论摘要

**整体健康度:实现层 ✅ 100%(R1-R5 满足,17 测试通过,commit `995098f` 落地),review 编排层 ❌ 严重偏离(6 维 review.dimension.ready 串行 walk 合法,但 isolated mode 单 business event/turn budget 把 5 条 ready 全部 silent drop,导致 testing 永远收不到 done / `review-synthesizer` 整链路未触发 / `review.complete` 终态 0 次 / 3 次 `hat_channel_empty_after_activation`),shipper 兜底层 ⚠️ 用 `pass_with_residuals` 把残缺链路包装成 pass**。

- **关键异常**:**P0 × 4、P1 × 4、P2 × 2**
- **P0 阻断点**:
  - **M-1**:`event_loop/mod.rs:7857, 8520, 8537, 8542` 的 isolated mode **"one business event per turn"** budget 不让步给 review-coordinator 的 6 维串行 walk(`presets/en/ce-executor-serial.yml:16` 明确约定),把 6 维 ready 中 5 条 silent drop;agent 看到被 drop 后反复重试,testing 维度累计 8 次
  - **C2**:3 次 `recovery_exhausted:stall_recovery:dimension_reviewer:review_dimension_ready:handoff_dispatch_timeout:*` 升级到 `repair_unrecoverable_after_3_retries`,handoff 路由错把 task.resume 投到不监听该 topic 的 hat
  - **C7**:`review.dimensions.complete` 把 maintainability / project-standards / adversarial / testing 4 维度伪造为 `findings_file: null, status: done`,schema 显式声明"element shape 由 agent prompt 守,不由 EventSchema 校验"——这是 silent drop findings 的设计洞
- **历史重复**:**是**——本次是 ce-executor-serial preset 在 30 天内第 12 次同根复发,核心活跃修复 plan `2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md` 的 U1-U10+U12 全部 active 待执行(仅 U11 已闭合 commit `5a58b8ac`),叠加新 plan `2026-07-03-002-fix-ce-executor-serial-093813-p0-orchestration-gaps-plan.md` U4 也未执行
- **机制 vs 编排一句话定性**:**机制 bug 为主(机制 isolated budget 不让步给 declared_serial_walk hat、handoff dispatch 路由不校验 consumer.triggers、schema element shape 不硬校验)+ preset 没标记 declared_serial_walk 让机制知道 review-coordinator 是合法串行方**。preset 的 hat 拓扑、topic_deny_rules、`emit_sequence.kind=sequence` 的 6 维 body 描述本身没错;但 shipper 白名单 `RECOVERABLE_REASONS` 把 stall 翻译成 pass_with_residuals,让"机制 budget 不让步"被自我包装成了 pass

---

## 2. 整体执行过程评估

### Phase 1:unit_loop(✅ 100%)

| # | 时间(UTC) | 事件 | payload 关键 | 状态 | 证据 |
|---|---|---|---|---|---|
| 1 | 13:01:18 | `work.start`(loop-bootstrap) | PROMPT.md 内容 | ✅ | events:1 |
| 2 | 13:03:49 | `work.ready(step-01)` | task_id=task-1783083814-a1b2 | ✅ | events:2, tasks.jsonl:1 |
| 3 | 13:05:30 | `work.done(step-01)` | commit=1 / changed_lines=237 | ✅ | events:3, ledger seq=2 |
| 4 | 13:06:14 | `test.passed(step-01)` | tests_run=5 / passed=5 | ✅ | events:4, ledger seq=3 |
| 5 | 13:06:14 | `work.ready(step-02)` | task_id=task-1783084327-a1b2 | ✅ | events:5, tasks.jsonl:2 |
| 6 | 13:08:11 | `plan.blocked(reason=work_failed)` ×2 | **被 isolated mode 单 event/turn 截断** | ❌ silent drop | events:6-7, log:30-31 |
| 7 | 13:11:30 | `work.ready(step-02)` retry | dedup SSOT 第二次命中 | ✅ | events:8, ledger seq=5-6 拒 |
| 8 | 13:14:23 | `work.done(step-02)` | commit=1 / changed_lines=181 | ✅ | events:9 |
| 9 | 13:17:30 | `test.passed(step-02)` | tests_run=17 / passed=17 | ✅ | events:10, ledger seq=9-10 |
| 10 | 13:18:50 | `loop.batch_sync` | iter=10, no progress turn 触发 ledger seq=7 | ⚠️ | ledger seq=7 |

### Phase 2:review_walk(❌ 严重偏离——**isolated budget 截断 6 维串行 ready**)

| # | 时间 | 事件 | 实际 vs 期望 | 状态 | 证据 |
|---|---|---|---|---|---|
| 11 | 13:19:03 | `review.start` | ✅ 正常 | ✅ | events:11 |
| 12 | 13:20:50 | `review.dimension.ready(goal-alignment)` | ✅ 第 1 维串行,被接受 | ✅ | events:12 |
| 13 | 13:22:35 | `review.dimension.ready(goal-alignment)` 第 2 次 | 期望 1 次即发下一维,实际 2 次 | ❌ | events:13, log:64-65 |
| 14 | 13:22:35 | `review.dimension.ready(goal-alignment)` 第 3 次 | ❌ | events:14 |
| 15 | 13:23:29 | `review.dimension.done(goal-alignment)` | findings=0 | ✅ | events:15 |
| 16-18 | 13:24:30-13:25:50 | `review.dimension.ready(correctness)` ×3 | 期望 1 次,实际 3 次(第 1 次被 budget drop 后 agent 串行 retry 2 次) | ❌ | events:16-18, log:86-87 |
| 19 | 13:25:15 | `review.dimension.done(correctness)` | findings=0 | ✅ | events:19 |
| 20-26 | 13:26:18-13:35:30 | `review.dimension.ready(testing)` ×7 + 第 8 次 13:39:09 | 期望 1 次,实际 8 次(每被 budget drop 1 次,agent 串行 retry 1 次) | ❌ | events:20-26,28, log:72-73 / 94-95 / 138-141 |
| 27 | 13:41:21 | `plan.blocked(reason=review_failed)` | 测试维度卡住后由 coordinator 误发 | ❌ | events:27, ledger seq=17 |
| 28 | 13:46:27 | `review.dimension.ready(testing)` retry(被 ledger 拒,dedup) | 第 8 次仍走通(命中降级路径) | ❌ | events:28 |
| 29 | 13:47:27 | `review.dimensions.complete` | **6 维中 4 维伪造 `findings_file: null, status: done`**(testing / maintainability / project-standards / adversarial) | ❌ | events:29 |
| 30 | 13:54:31 | `review.dimension.done(testing)` | P1=1, P2=2 | ✅(迟到 22 分钟) | events:30 |

### Phase 3:recovery / handoff(❌ 多次 stall 升级)

| # | 时间 | 事件 | 状态 | 证据 |
|---|---|---|---|---|
| 31-33 | 13:25 / 13:28 / 13:53 | `hat_channel_empty_after_activation` ×3 → 落盘 `diagnostics/channel-routing-fallback-{ts}.md` | ❌ | diagnostics 3 份文件 |
| - | 13:39:09 | `recovery_exhausted:stall_recovery:dimension_reviewer:review_dimension_ready:handoff_dispatch_timeout:*` 升级 | ❌ | recovery.jsonl:5, log:116 |
| - | 13:42:30 | `runtime-recovery: forcing plan.blocked reason=handoff_timeout_recovery_finalized` | ❌ | log:108-109, log:142-143 |
| - | 13:46-13:55 | `stall_recovery_counts: {coordinator: 1, shipper: 1, dimension_reviewer: 1}` | ❌ | recovery.jsonl:1-6 |

### Phase 4:ship / report / terminal(✅ 兜底闭环)

| # | 时间 | 事件 | 状态 | 证据 |
|---|---|---|---|---|
| 31 | 13:55:15 | `REVIEW_COMPLETE(verdict=pass_with_residuals, pass_or_fail=pass, residual_findings_summary="isolated mode violation")` | ⚠️ shipper 把 plan.blocked(review_failed) 翻译为 pass | events:31 |
| 32 | 13:57:05 | `report.done(report_path=docs/.../report.md)` | ✅ | events:32 |
| 33 | 13:58:09 | `LOOP_COMPLETE(reason=plan_complete_all_steps_pass)` + `loop.completion_requested` + `loop.completion_honored` | ✅ | events:33, ledger seq=23-25 |

**链路完整性核算**:

| 节点 | 期望 | 实际 | 差距 |
|---|---|---|---|
| `review.dimension.ready` | 6(每维 1 条,共 6 维) | **14**(goal-alignment 1 / correctness 1 / testing 1 + 5 条 budget drop 后 agent 串行 retry) | 多 8 条全部被 isolated mode budget silent drop 后 agent 串行重试;实际只有 3 条被接受(goal-alignment + correctness + testing 各 1),其余 5 维(goal-alignment 之后 + correctness 之后 + testing 之后 + maintainability + project-standards + adversarial)从未 emit ready |
| `review.dimension.done` | 6 | **3**(goal-alignment + correctness + testing) | 缺 3 维: maintainability / project-standards / adversarial |
| `review.dimensions.complete` | 1 | 1 | OK 但 payload 伪造 4 维为 done |
| `review-synthesizer` 触发 | 1 | **0** | 整链路未触发 |
| `review.complete`(终态) | 1 | **0** | 完全缺失,fix-unit 回路被旁路 |
| `fix.applied` | 0(无 P0/P1 走修复回路即通过) | 0 | OK(因 review.coordinator 错把 review_failed 走 shipper 兜底) |
| `REVIEW_COMPLETE` | 1 | 1 | OK(但 shipper 硬发) |
| `report.done` | 1 | 1 | OK |
| `LOOP_COMPLETE` | 1 | 1 | OK |
| `plan.complete` | 1 | **0** | 走 `plan.blocked` 替代(同根 4.1) |
| `duplicate_work_done` 拒绝 | 0(理想) | **4**(2×work.ready + 2×review.dimension.ready) | ledger seq=5-6, 18-19 |
| `hat_channel_empty_after_activation` | 0 | **3** | diagnostics 3 份 |
| `plan.blocked` | 0(理想) | **3**(events:6-7, 27) | 全部合法 reason,但反映编排偏离 |

---

## 3. RALPH 基座机制评估

**机制本身的设计原则是对的**,但**有三处缺位防线**共同让 preset 编排的偏离被自我包装成 pass。

| ID | 机制层问题 | 证据 | 严重性 | 历史关联 |
|---|---|---|---|---|
| **M-1** | **isolated mode "one business event per turn" budget 不让步给 review-coordinator 的 6 维串行 walk**——`event_loop/mod.rs:7857, 8520, 8537, 8542` 注释字面 `Isolated mode: hard-enforce current hat scope + single business event boundary` 和 `Isolated mode: dropped extra event '{}' — only one business event per turn allowed`,把 preset `presets/en/ce-executor-serial.yml:16` 明确约定的 `Walks fixed 6-dimension sequence ... one dimension per turn` 中的第 2-6 条 ready 全部 silent drop;review-coordinator 必须连发 6 条 `review.dimension.ready` 是 preset 契约,机制 budget 没给这种"必须连发 N 条 ready 的 hat"留例外 | `crates/ralph-core/src/event_loop/mod.rs:7857, 8520, 8537, 8542` + `:1700-1800` `enforce_wave_isolated_scope` 段;`presets/en/ce-executor-serial.yml:16, 99-114, 152-162`;`memories.md:9-11`;`shipping.md:53-55` | **P0** | `ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md` 同因;`151220 §P0-A` 也有同形(本报告初稿把根因错写成"dedup 缺 per-dim prune",实际是 isolated budget 不让步) |
| **M-2** | `handoff_dispatch` 路由前**不校验 `consumer_hat.triggers.contains(topic)`**,把 `task.resume` 投到 validator(validator 只监听 `work.done/fix.applied`) | `crates/ralph-core/src/event_loop/mod.rs:7029-7132` handoff 注入段;shipping.md 4 类 stall reason 全部走 handoff_dispatch_timeout | **P0** | `ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` v2 variant;`151220 §P1-B` 同根 |
| **M-3** | `presets/schemas/ce-executor-serial.yml:200-202` 显式声明 `dimensions` 数组 element shape "由 agent prompt 守,不由 EventSchema 校验"——**C7 静默 drop 4 维 findings 的设计洞** | `presets/schemas/ce-executor-serial.yml:200-216` | **P0** | `ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md` P1-3 根因延伸 |
| M-4 | `shipper_reason.rs:19-29` `RECOVERABLE_REASONS` 把 `recovery_exhausted:stall_recovery:dimension_reviewer:*` 与 `recovery_exhausted:stall_recovery:coordinator:*` 列入 recoverable,**翻译为 `pass_with_residuals` 把真因盖住** | `crates/ralph-core/src/shipper_reason.rs:19-49`;`presets/schemas/ce-executor-serial.yml:351-353` 允许 `recovery_exhausted` 通过 schema | P1 | U11 commit `5a58b8ac` 部分闭环,但 shipper 翻译策略过宽 |
| M-5 | `event_loop/mod.rs:8194-8290` isolated_scope skip 区把 `state_projector` 调用跳过,导致 `progress.md` 在 isolated 模式下不投影 | `crates/ralph-core/src/event_loop/mod.rs:8194-8290`;`progress.md` 字段空 | P2 | 未直接触发,但本 run 偶发 |
| M-6 | `summary_writer.rs:567` `stall_recovery_counts` 写入但 dashboard/shipper 不读,6 条 `repair envelope` 不可观测 | `crates/ralph-core/src/summary_writer.rs:567`;`recovery.jsonl:1-6` | P2 | 无历史关联 |
| ✅ 正常 | `default_publishes` 机制侧白名单(U11 闭合) | `shipper_reason.rs:23, 47`;`presets/schemas/ce-executor-serial.yml:361-367` | - | 075227 run 验证通过 |
| ✅ 正常 | multi-hat isolation 硬规则(4+ hat 强制 isolated)| `preset_lint/multi_hat.rs:51-92` | - | 已闭环 |

---

## 4. 编排合理性评估

**编排层的问题集中在 review-coordinator 和 shipper 两端**。

| ID | 编排问题 | 证据 | 严重性 |
|---|---|---|---|
| **O-1** | `review-coordinator` 严格按 preset `presets/en/ce-executor-serial.yml:16` 的 **"one dimension per turn"** 串行约定发 6 维 `review.dimension.ready`(goal-alignment → correctness → testing → maintainability → project-standards → adversarial);但 isolated mode **"one business event per turn"** budget(`event_loop/mod.rs:7857, 8520, 8537, 8542`)把首维之后的每条 ready 全部 silent drop。这是**机制对预设契约不兼容**:preset 串行约定合法,机制单 event/turn 把合法串行打成 silent drop。**注意:不是 O-1 写了"并发 emit"——本报告初稿此处措辞错了,agent 实际是串行发的,根因是机制 budget 对 review-coordinator 这种"必须连发 6 条 ready"的 hat 不让步** | `presets/en/ce-executor-serial.yml:16, 99-114, 152-162`;`event_loop/mod.rs:7857, 8520, 8537, 8542`;`events:12-14, 16-18, 20-26, 28`;`memories.md:9-11`;`shipping.md:53-55` | **P0** |
| **O-2** | `coordinator` 在 testing 维度卡住 22 分钟后**误发 `plan.blocked(reason=review_failed)` 而非 `review_wave_stuck`**,reason 在 preset schema 白名单中,误导 shipper 走 recoverable | `events:27`;`presets/schemas/ce-executor-serial.yml:328-367` allowed_values | **P0** |
| **O-3** | `review-coordinator` 发出 `review.dimensions.complete` 时把 testing / maintainability / project-standards / adversarial 4 维度记为 `status: done, findings_file: null`,**显式数据造假**——synthesizer 全部需要这些 findings 走 `review.complete` | `events:29` dimensions 数组;`presets/schemas/ce-executor-serial.yml:202-216` 注释 | **P0** |
| O-4 | `review-synthesizer` hat **整链路未触发**——既未消费 `review.dimensions.complete` 也未 emit `review.complete` 终态 | `events:29` 之后无 synthesizer 事件;`grep review-synthesizer events.jsonl` 仅 1 命中(line 29 source=review-coordinator) | P1 |
| O-5 | shipper 把 `plan.blocked(reason=recovery_exhausted:stall_recovery:*)` 译为 `REVIEW_COMPLETE(pass_with_residuals)`,**通过 schema 防线 + 白名单防线**,把"机制 isolated budget 不让步"包装成 pass | `events:31`;`shipper_reason.rs:19-49`;`presets/schemas/ce-executor-serial.yml:351-353` | P1 |
| O-6 | 4 类 `stall_recovery` reason 全部含 `handoff_dispatch_timeout:*` 后缀,recovery 路径相同,reason_code 频次熔断缺失 | `shipping.md:54-58`;`recovery.jsonl:1-6` | P1 |
| O-7 | `handoff.md` "Recently modified" 列了 10 个文件,但 commit `995098f` 只触及 2 个(混入了前序 commit `0eb4166` 的文件);harness 在 isolated mode 下生成 handoff 时未对齐最新 commit | `handoff.md:20-31`;`git show 995098f --stat` | P2 |
| O-8 | `progress.md` `Current Step=(none)` 与 `Completed Steps=[step-01, step-02]` 错位(projector 在 isolated 下未投影) | `progress.md:4-8`;`state_projection.rs` 静态 | P2 |

---

## 5. 中间产物与机制一致性

| 产物 | 与事件流一致性 | 备注 |
|---|---|---|
| `events-20260703-130118.jsonl` | ✅ | 33 行,无跳号,iter 推进可重建 |
| `ledger.jsonl` | ✅ | 25 行,4 条 `rejection_recorded`(seq 5-6, 18-19)+ 1 条 `no_progress_turn_observed`(seq 7) |
| `recovery.jsonl` | ✅ | 6 envelope(2× plan.blocked + 4× info),stall counter 真实 |
| `tasks.jsonl` | ✅ | 2 行(步骤对应 task_id),均 closed |
| `progress.md` | ⚠️ | `Current Step=(none)` 与 `Completed Steps=[step-01, step-02]` 错位 |
| `summary.md` | ✅ | 21 iter / 33 events / 14 review.dimension.ready 数字准确 |
| `handoff.md` | ⚠️ | 列的 10 个"Recently modified" 中只有 2 个属于 commit `995098f` |
| `memories.md` | ✅ | 4 条 mem 字面命中 review-coordinator violation + plan.blocked review_failed |
| `diagnostics/channel-routing-fallback-*.md` | ✅ | 3 份 423-byte 报告,hat=review-coordinator 三连发 |
| `diagnostics/logs/ralph-*.log` | ✅ | 33K 字节,含 4 类 stall 注入痕迹 + handoff 路由错日志 |
| `diagnostics/agent_doc_sync.json` | ✅ | synced=2 / failed=0,doc 同步无问题 |
| `loops.json` | ✅ | `{"loops":[]}`(loop 终态已清空) |
| `commit 995098f` | ✅ | 真实存在,改动 `sorts/README.md` + `sorts/tests/test_integration.py` |
| `shipping.md` | ⚠️ | 自我声明 "pass_with_residuals",残差清单中写明"review infrastructure failures known Ralph bugs"——本质是 shipping 自报残差 |

---

## 6. 核心问题归因(机制 vs 编排)

**直接结论**:**机制 bug 为主(70%——M-1 isolated mode 单 business event/turn budget 不让步给 review-coordinator 6 维串行 walk + M-2 handoff 路由错 + M-3 schema element shape 不硬校验)+ preset 设计是触发因(20%——O-3 维度数据造假 + O-5 shipper 白名单过宽)+ 产物可观测性不足(10%——M-5/M-6 + O-7/O-8)**。**注:O-1 已重写为"agent 串行 walk 是合法契约",根因不在编排层。**

### 6.1 机制层主导(P0-1, P0-2, P0-3)

**根因**:`event_loop/mod.rs:7857, 8520, 8537, 8542` 的 isolated mode **"one business event per turn"** budget 在 `enforce_wave_isolated_scope` 段(1700-1800 行)对所有非 wave 业务事件硬截断,只允许每 turn 一条非 wave 业务事件。preset `presets/en/ce-executor-serial.yml:16` 明确约定 `review-coordinator: Walks fixed 6-dimension sequence ... one dimension per turn`——**每条 ready 单独占一 turn 是契约**;但机制 budget 把"6 turn 内连发 6 条不同 ready"打成"6 turn 内只发 1 条 ready,其他 5 条 silent drop",**对这种"必须连发 N 条 ready 的 hat"没留例外**。本次 run 测试维度 8 次连发(goal-alignment ×3 / correctness ×3 / testing ×8)正是机制 budget 一次次 silent drop 后 agent 反复重试的痕迹——agent 看到 ready 被 drop 后会按 preset 契约再发一次,但每次发都再被 drop,直到 stall counter 升级到 recovery_exhausted。叠加 M-2 `handoff_dispatch` 路由不校验 `consumer_hat.triggers.contains(topic)`,task.resume 投到不订阅的 hat,触发 30 秒超时→inject recovery→stall counter 累加→再升级 recovery_exhausted。

**这是机制问题,不是编排问题**:
- 拒绝行为本身**完全正确**(single-business-event budget 是设计意图,防 hat 沉默时 loop 死锁)
- 拒绝消息**完全准确**(silent drop + ledger rejection_recorded + diagnostic `channel-routing-fallback-{ts}.md` 落盘留痕)
- 错的是 budget 没对 review-coordinator 这种 preset 串行契约的 hat 让步 + handoff 路由不校验 + recovery 升级后 shipper 又把它当 recoverable 包装成 pass

### 6.2 编排层 trigger(P0-3 + P1-1)

**根因**:`review-coordinator` 串行走 6 维是 preset 契约,合法且正确;**唯一**编排问题是 review-coordinator 在 testing 维度卡住 22 分钟后(因为前 5 维 ready 全部被 isolated budget silent drop)由 coordinator 误发 `plan.blocked(reason=review_failed)`,且 O-3 把未走的 4 维伪造为 `status: done, findings_file: null` 走 `review.dimensions.complete`,把 4 维 findings 静默吞掉。

**判断**:preset 设计本身没问题,串行契约清楚;错的是 agent 在 testing 卡住后**应等待事件 ack 而非伪造完整**,以及 coordinator 误发 `review_failed` reason。

### 6.3 shipper 兜底掩盖(机制 + 编排双向强化)

**根因**:`shipper_reason.rs:23, 47` 把 `recovery_exhausted:stall_recovery:dimension_reviewer:*` 与 `...:coordinator:*` 列成 RECOVERABLE_REASONS,`presets/schemas/ce-executor-serial.yml:351-353` 又把 `recovery_exhausted` 加进 allowed_values,**两处联合让"机制 isolated budget silent drop → review-coordinator 反复重试 → stall counter 升级 → shipper 翻译为 pass_with_residuals → 闭环"**。本次 run 走通就是这条路径。

**判断**:**这是机制 + preset 互相强化的循环 bug**。shipper 把 stall 翻译成 pass_with_residuals 是对的(默认 lint 不变),但 residual_findings_summary 应显式包含 `stall_recovery_observed: true`,让 reporter / dashboard 把这条值班算进"task 真正完成的 P0 残留"。

---

## 7. 与历史 30 天 11 次复发的关系

**这是同根簇的第 12 次复发**(`perky-maple / noble-peacock / merry-lotus` 簇),但**机制翻面**了:

| Run | 起始 UTC | 卡点 | 现象 | 与本次 run 共享症状 |
|---|---|---|---|---|
| 170451 | 2026-06-30 17:04:51 | fix-02 断链 | plan.complete 0 次 / plan.blocked 0 次 / LOOP_COMPLETE 0 次 | (本 run plan.complete 0 次) |
| 032648 | 2026-06-30 03:26:48 | DE-003 链路断 | plan_gate_review_not_terminal 拦 plan.complete | 同根 plan.complete 被拦 |
| 083222 | 2026-06-30 08:32:22 | plan.complete 9 次被拒 → ralph 抢发 work.ready | hat=ralph 越权 | 同根 plan.blocked 替代 |
| 140433 | 2026-06-30 14:04:33 | LOOP_COMPLETE 后进程不退出 | 进程级退出缺陷 | (本次正常) |
| 175407 | 2026-07-01 17:54:07 | 2x REVIEW_COMPLETE + 3x report.done + 2x LOOP_COMPLETE | 二次风暴 | (本次单次,但同根 shipper 白名单) |
| 112002 | 2026-07-01 11:20:02 | review 启动段 4 次重复 | review.dimension.ready 多发 | **同根 isolated mode budget 截断 6 维 ready** |
| 140149 | 2026-07-01 14:01:49 | progress.md Current Step=(none) + shipper 3 次 REVIEW_COMPLETE fail | progress-steward 注入 | **同根 plan.blocked 替代 plan.complete** |
| 151220 | 2026-07-02 15:12:20 | task.resume 4 次风暴 + ralph 抢发 LOOP_COMPLETE | handoff 路由错 | **同根 handoff_dispatch_timeout*** |
| 020135 | 2026-07-03 02:01:35 | review 链 0% + current-hat-events 路由错位 | review 链完全断 | **同根 hat_channel_empty_after_activation** |
| 075227 | 2026-07-03 07:52:27 | coordinator 沉默 + default_publishes 注入 | 0 业务事件 | (本次 shipper 走得通,U11 已闭合) |
| 093813 | 2026-07-03 09:38:13 | fix-01 task_id 复用 + shipper 白名单缺 default_publishes | fix 链 0% | (本次 fix 链被 review 错位挡掉) |
| **130118(本次)** | **2026-07-03 13:01:18** | **isolated mode budget 不让步给 review-coordinator 6 维串行 walk → 5 维 ready silent drop → agent 反复重试 → hat_channel 路由错 → shipper 白名单掩盖** | **review 链 50% + shipper 兜底闭环** | **5 类高关联度全中** |

**共同根因**:
- `2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md` U1-U10+U12 全部 active 待执行(仅 U11 commit `5a58b8ac` 已闭合)
- `2026-07-03-002-fix-ce-executor-serial-093813-p0-orchestration-gaps-plan.md` U1-U4 active(U4 hat_channel 修复正是本次 P0-2 的根因修复)
- `2026-07-01-001-refactor-event-loop-mod-split-plan.md` U1-U6 待办(`event_loop/mod.rs` 11000+ 行杂糅仍是根因分布底层)

**本次新发现(未在 active plan 中)**:
- **C7 schema element shape 不硬校验**:`presets/schemas/ce-executor-serial.yml:200-202` 显式声明"由 agent prompt 守,不由 EventSchema 校验"——是 silent drop 4 维 findings 的设计洞
- **C6 task.resume 同 reason_code 频次熔断缺失**:4 类 stall_recovery reason 都走 `handoff_dispatch_timeout:*`,无 reason_code 频次熔断
- **C8 shipper residual_findings_summary 缺 `stall_recovery_observed` 标记**:让"机制 isolated budget 不让步"被自我包装成 pass

---

## 8. 修复建议(按优先级)

### Fix 1(M-1 / P0)—— isolated mode single-business-event budget 给 review-coordinator 6 维串行 walk 让步

- **目标**:机制(让 review-coordinator 6 维串行 walk 在 isolated mode 下不被 budget silent drop)
- **修改文件**:
  - `crates/ralph-core/src/event_loop/mod.rs:7857, 8520, 8537, 8542`(isolated budget 段)
  - `crates/ralph-core/src/event_loop/mod.rs:1700-1800`(`enforce_wave_isolated_scope` 段)
  - `presets/en/ce-executor-serial.yml:99-114, 152-162`(`phase_authority.phases[1].review`)
- **具体内容**:
  ```rust
  // event_loop/mod.rs:7857 旧注释:
  // "Isolated mode: hard-enforce current hat scope + single business event boundary"
  // 改为:对 declared_serial_walk hats(在 phase_authority.phases[].review 中标记
  //      emit_sequence.kind=sequence 且 emit_sequence.body 含 N>1 条 review.*.ready
  //      事件)放宽 budget:N 条同 topic-prefix 事件算 1 个 business emission
  if hat.is_declared_serial_walk() && self.phase_authority.matches(emitted) {
      // 不计入 non_wave_business_event_accepted,允许 N 条 ready 串行
      accepted.push(event);
  } else {
      // 维持原 single-event budget
  }
  ```
  ```yaml
  # presets/en/ce-executor-serial.yml:99-114 phase_authority.phases[1].review 改:
  # 在 review 阶段给 review-coordinator 加 declared_serial_walk: true 标记
  - id: review
    declared_serial_walk: review-coordinator  # 新增,声明此 hat 在 review 阶段走串行 6 维
    allowed_emits:
      ...
  ```
- **预期效果**:M-1 silent drop 链打破;review-coordinator 6 维 ready 全部被接受,dimension-reviewer 正常对 6 维 emit done,review-synthesizer 正常 emit `review.complete`,shipper 走正规 `REVIEW_COMPLETE` 而非 `pass_with_residuals`
- **回归测试**:
  ```bash
  cargo nextest run -p ralph-core --event_loop/tests/isolated_wave_budget -- test_declared_serial_walk_exempt_from_budget
  cargo nextest run -p ralph-core --test scenarios -- ce_executor_serial_review_chain_6_dims_no_drop
  ```

### Fix 2(C2 + C8 / P0)—— shipper 白名单减负 + 把 "pipeline 自救" 从 "pass_with_residuals" 剥离

- **目标**:preset + 机制
- **修改文件**:
  - `presets/en/ce-executor-serial.yml:480-500`(review-coordinator.instructions)
  - `crates/ralph-core/src/shipper_reason.rs:19-49`(`RECOVERABLE_REASONS` 段)
- **具体内容**:
  ```rust
  // shipper_reason.rs:27-28 当前把 stall 翻译为 recoverable
  // 改为:把 recovery_exhausted:stall_recovery:dimension_reviewer:* 与
  //      recovery_exhausted:stall_recovery:coordinator:* 抽出 RECOVERABLE_REASONS,
  //      单列为 fail-with-residual;
  //      但 pass_with_residuals 路径必须输出
  //      residual_findings_summary 必须显式包含 stall_recovery_observed: true,
  //      让 dashboard 能直接看到 review-coordinator 反复 ready 的形态
  ```
- **预期效果**:C8 不再"成功掩盖 C1/C2 真因",下次 run 看 shipping.md 就能直接发现 review-coordinator 反复 ready 的形态
- **回归测试**:
  ```bash
  cargo nextest run -p ralph-core -- test_shipper_routing_residual_summary_carries_stall_recovery_marker
  cargo nextest run -p ralph-core --test scenarios -- review_chain_with_dup_dispatch
  ```

### Fix 3(C7 / P0 silent drop)—— review.dimensions.complete 的 element shape 校验

- **目标**:preset schema
- **修改文件**:`presets/schemas/ce-executor-serial.yml:200-216`
- **具体内容**:
  ```yaml
  # 旧注释:
  # "the dimensions array element shape ... is enforced by agent prompt discipline"
  # 改为 Rust 侧硬校验:
  review.dimensions.complete:
    required_fields:
      - plan_name
      - task_id
      - task_key
      - step
      - dimensions
      - fix_round
    element_constraints:
      dimensions:
        - {field: dimension, type: string, allowed_values: [goal-alignment, correctness, testing, maintainability, project-standards, adversarial]}
        - {field: status, type: string, allowed_values: [done, skipped, failed]}
        - {field: findings_file, required_when: {status: done}, type: string_or_null_for_skipped}
  ```
  在 `crates/ralph-core/src/event_policy.rs` 的 `validate_event_with_hat` 中加 nested array 校验
- **预期效果**:本次 run `events:29` 中 4 个 `findings_file: null, status:"done"` 不会再 silent drop;会被 schema 拒
- **回归测试**:
  ```bash
  cargo nextest run -p ralph-core -- test_review_dimensions_complete_element_shape_validation
  ```

### Fix 4(M-2 / P0)—— handoff_dispatch 路由前校验 consumer.triggers

- **目标**:机制
- **修改文件**:`crates/ralph-core/src/event_loop/mod.rs:7029-7132`(handoff 注入段)
- **具体内容**:
  ```rust
  // 在 handoff_dispatch 前增加校验:
  if !consumer_hat.triggers.contains(&topic) {
      warn!("handoff_dispatch routing {} to {} but consumer doesn't subscribe; \
             falling back to ralph hat which always subscribes", topic, consumer_hat.id);
      // 改为走 ralph 兜底(ralph hat triggers=["*"])
      consumer_hat = ralph_hat_lookup();
  }
  ```
- **预期效果**:`recovery_exhausted:stall_recovery:dimension_reviewer:review_dimension_ready:handoff_dispatch_timeout:*` 升级路径上,`task.resume` 不再投到 validator;stall 计数收敛
- **回归测试**:
  ```bash
  cargo nextest run -p ralph-core -- test_handoff_dispatch_consumer_trigger_check
  cargo nextest run -p ralph-core --test scenarios -- stall_recovery_with_misrouted_resume
  ```

### Fix 5(C4 + M-6 / P2 可观测)—— repair envelope 回流 summary

- **目标**:机制
- **修改文件**:`crates/ralph-core/src/summary_writer.rs:567`(`stall_recovery_counts` 段)
- **具体内容**:`recovery.jsonl` ≥ 1 条 envelope 时,summary 必须列出 `repair_envelope_count` + breakdown by `source_hat` + envelope reason_code 直方图
- **预期效果**:shipping.md 不再依赖手写总结,可直接 `cat .ralph/agent/summary.md` 看到 stall 真因
- **回归测试**:
  ```bash
  cargo nextest run -p ralph-core -- test_summary_writer_includes_repair_envelope_section
  ```

### Fix 6(M-5 / P2 产物完整性)—— isolated 模式下 projector 仍工作

- **目标**:机制
- **修改文件**:`crates/ralph-core/src/event_loop/mod.rs:8194-8290`(isolated_scope skip 区)
- **具体内容**:把 `state_projector` 调用从 isolated_scope skip 区移出,确保 `progress.md` 在 isolated 模式下也由 projector 写入(避免 agent 双写冲突)
- **预期效果**:`.ralph/agent/progress.md` 不再 `Current Step=(none)`
- **回归测试**:
  ```bash
  cargo nextest run -p ralph-core -- test_state_projector_runs_under_isolated_scope
  ```

---

## 9. 验收路径(必须)

修复后必须用 `run_workflow_guard_scenario`(真 EventLoop runner,**禁止用 `run_scenario` stub**——stub 只查 iterations 数,会静默吞掉拓扑失配),按以下三组场景验收:

| 场景 | 验证目标 | 期望事件链 |
|---|---|---|
| SC-1 | 同 plan 走正规链 | `work.start → work.ready → work.done → test.passed → review.start → 6×review.dimension.ready → 6×review.dimension.done → review.dimensions.complete(6 维全 findings_file) → review.complete → REVIEW_COMPLETE → report.done → LOOP_COMPLETE` |
| SC-2 | review-coordinator 误发第 2 条同 dim ready(本次症状) | 第 2 条被 silent drop 拒,ledger rejection_recorded,dimension-reviewer 仍正常 emit done |
| SC-3 | handoff_dispatch 投到不订阅 hat(本次 M-2 症状) | fallback 到 ralph 兜底,不再升级到 recovery_exhausted;stall counter 不累加 |

---

## 10. 关键证据索引

| 类别 | 路径 |
|------|------|
| preset | `/Users/pittcat/Dev/Rust/ralph-orchestrator/presets/en/ce-executor-serial.yml`(2962 行;phase_authority 行 138-222, hats 行 713-1312, work_contract / event_policy 行 248-695, review-coordinator.instructions 行 480-500) |
| preset schema SSOT | `/Users/pittcat/Dev/Rust/ralph-orchestrator/presets/schemas/ce-executor-serial.yml`(schemas 行 59-461;dimensions element shape 注释 行 200-202, allowed_values 行 328-367, recovery_exhausted 白名单 行 351-353) |
| isolated budget 截断 | `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/src/event_loop/mod.rs:7857, 8520, 8537, 8542`(budget 段)+ `:1700-1800` `enforce_wave_isolated_scope` 段 |
| handoff 路由错 | `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/src/event_loop/mod.rs:7029-7132` |
| shipper 翻译 | `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/src/shipper_reason.rs:19-49` |
| multi-hat isolation lint | `/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/ralph-core/src/preset_lint/multi_hat.rs:51-92` |
| 事件流(本次) | `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/events-20260703-130118.jsonl`(33 行) |
| ledger(本次) | `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/ledger.jsonl`(25 行) |
| recovery envelope(本次) | `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/recovery.jsonl`(6 envelope) |
| diagnostics(本次) | `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/diagnostics/channel-routing-fallback-{2026-07-03T13-25-13, 2026-07-03T13-28-42, 2026-07-03T13-53-03}.md` |
| 日志(本次) | `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/diagnostics/logs/ralph-2026-07-03T21-01-17-400-28913.log`(33KB) |
| shipping | `/Users/pittcat/Dev/Rust/ralph-e2e/shipping.md`(verdict=pass_with_residuals, 4 类 stall reason) |
| handoff | `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/agent/handoff.md`(loop primary-20260703-130118 终态) |
| 业务 plan | `/Users/pittcat/Dev/Rust/ralph-e2e/docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md` |
| 业务 commit | `995098f: feat(sorts): 快速排序完善 + README + 集成回归`(改动 `sorts/README.md` + `sorts/tests/test_integration.py`,174 insertions / 7 deletions) |
| 历史同根 11 次 | `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-{06-30, 07-01, 07-02, 07-03}-ce-executor-serial-*.md`(12 份) |
| 核心修复 plan(未完成) | `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/plans/2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md`(U1-U10+U12 active,U11 `5a58b8ac` 已闭合) |
| 关联修复 plan(未完成) | `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/plans/2026-07-03-002-fix-ce-executor-serial-093813-p0-orchestration-gaps-plan.md`(U1-U4 active,U4 hat_channel 修复是本次 P0-2 根因) |
| mod.rs 拆分 plan | `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/plans/2026-07-01-001-refactor-event-loop-mod-split-plan.md`(12669 行 mod.rs 杂糅) |
| 历史 KB(perky-maple 同因) | `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/solutions/integration-issues/ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md` |
| 历史 KB(noble-peacock 静默 DR) | `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/solutions/integration-issues/ce-executor-serial-noble-peacock-review-chain-2026-06-17.md` |
| 历史 KB(stall 兜底机制) | `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` |
| 历史 KB(10-hat 架构背景) | `/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/solutions/integration-issues/mechanism-foundation-validation-2026-06-27.md` |

---

## 11. 总结

本次 run `primary-20260703-130118` 的核心问题是**机制 bug 为主**(`event_loop/mod.rs:7857, 8520, 8537, 8542` isolated mode "one business event per turn" budget 不让步给 review-coordinator 6 维串行 walk + `event_loop/mod.rs:7029-7132` handoff 路由不校验 consumer.triggers + `presets/schemas/ce-executor-serial.yml:200-202` 显式声明 element shape 不硬校验)被**preset 没声明 declared_serial_walk**(`presets/en/ce-executor-serial.yml:99-114, 152-162` phase_authority.phases[1].review 没标 `declared_serial_walk: review-coordinator` 让机制知道此 hat 必须连发 6 条 ready)激活,又由 shipper 的 `pass_with_residuals` 兜底翻译自我包装成 pass,共同让 30 天 12 次同根复簇继续累积。这是 isolated budget 截断 6 维 ready → agent 反复重试 → hat_channel 路由错位 → stall_recovery 升级 → shipper 白名单掩盖 的完整链路。

**修复责任分配**:
- 70% 在机制层(必须改 `event_loop/mod.rs:7857, 8520, 8537, 8542` isolated budget 对 declared_serial_walk hat 让步 + `:7029-7132` handoff 校验段 + `shipper_reason.rs` 翻译策略)
- 20% 在 preset schema 层(`presets/schemas/ce-executor-serial.yml:200-216` element shape 硬校验 + `:351-353` recovery_exhausted 白名单收窄)
- 10% 在 preset phase_authority 层(`presets/en/ce-executor-serial.yml:99-114, 152-162` review 阶段给 review-coordinator 加 `declared_serial_walk: true` 标记 + `:8194-8290` state_projector 不被 isolated_scope 跳过)

**RALPH 机制本身没有重大设计问题**——它的拒绝行为、backpressure、白名单最小集、recovery 升级都是对的。错的是 **isolated budget 没给 declared_serial_walk hat 让步**、**handoff 路由不校验**、**schema element shape 不硬校验**这三道防线存在;以及 preset 没声明 declared_serial_walk 让机制知道 review-coordinator 是合法串行方;以及 shipper 兜底策略对"recovery_exhausted"过宽。这三个问题都在 `2026-07-02-005` + `2026-07-03-002` plan 范围内,但执行进度滞后——U1-U10+U12 全部 active 待执行(仅 U11 `5a58b8ac` 已闭合)。**建议把本报告的 Fix 1 / Fix 2 / Fix 3 / Fix 4 修复追加到 `2026-07-02-005` plan 的 U 列表中,优先 Fix 1(isolated budget 对 declared_serial_walk 让步)+ Fix 4(handoff 路由校验)+ Fix 3(schema 硬校验)——这三者闭环后,本次 run 的 silent drop 链将被打破**。
