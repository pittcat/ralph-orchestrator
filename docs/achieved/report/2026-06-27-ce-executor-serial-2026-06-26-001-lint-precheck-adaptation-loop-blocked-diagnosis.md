# Ralph Loop 运行链路诊断报告 — `2026-06-26-001-feat-ralph-lint-precheck-adaptation-plan`

> **生成时间**:2026-06-27 04:00 (UTC+8)
> **方法**:4 个并行 sub agent(流程还原 / 历史知识库 / 对账分析 / 归因修复),主 Agent 仅做汇总与校正
> **输入**:
> - preset:主仓库 `/Users/pittcat/Dev/Rust/ralph-orchestrator/presets/en/ce-executor-serial.yml`(10-hat isolated)
> - plan:`/Users/pittcat/Dev/agent_tools/universal-autoresearch/.worktrees/2026-06-26-001-feat-ralph-lint-precheck-adaptation-plan/docs/plans/2026-06-26-001-feat-ralph-lint-precheck-adaptation-plan.md`(U1-U8)
> - worktree `.ralph/` 中间产物:22 events + 28 recovery envelopes + 5 drift findings + 7 tasks + 9 状态文件
>
> **代码审查依据**:主仓库 `/Users/pittcat/Dev/Rust/ralph-orchestrator/`;worktree 仅作运行时产物参考。

---

## 1. 结论摘要

| 维度 | 结论 |
|---|---|
| **健康度一句话** | 编排机制本身 ✅ 正常(`coordinator → executor → validator → fixer` 主链路 22 events 完整跑通,U1-U4 真实实施质量高,全量回归回到 U1 baseline 16/238 fail)。失败原因是 **Ralph 基座 3 个 P0 级机制缺陷叠加**:`execution_contract` fail-closed 拒绝 legacy task(无 loop_id)→ stall_recovery 反复 retry 28 次→ `progress-steward` 没有"回填 loop_id"逃生通道→ coordinator 在 U5-U8 未实施时不知道该 `plan.complete` 还是 `plan.blocked`→ shipper 收到 `plan.blocked(reason="")` 走 hard-fail→ verdict_gate 终止语义错位(review_failed 包了 report.done)。**修复机制系统性失效** + **loop-termination 语义错位**。 |
| **P0 数量** | 6 条(TaskWrongLoop 反复触发 / progress-steward 无回填通道 / verdict_gate 终止语义错位 / completion_promise 契约冲突 / review pipeline 6-dim 完全缺失 / coordinator 早期 task 生成路径不写 loop_id) |
| **P1 数量** | 5 条(stall_recovery 死循环 / plan.blocked reason 必填校验缺失 / task.resume schema 不全 / drift_monitor outcome 反复翻转 / fix.applied 后立即又 stall) |
| **P2 数量** | 5 条(human.guidance.message 空 / loop-termination-reason.json 措辞不准 / progress 4/4 vs plan 4/8 命名粒度差 / drift_count vs recovery_count 计数口径不一 / final_commit 概念混淆) |
| **历史重复** | **95% 命中**——本次 run 几乎所有现象都是"30 天第 7+ 次复发的 ce-executor-serial 修复机制系统性失效"模式(见 6-26 报告),与 `primary-20260624-092856` / `primary-20260623-152241` / `keen-fern` / `nimble-teak` / `zippy-otter` 完全同型。**唯一半新特征**:`TaskWrongLoop { actual_loop: None }` 的字面 signature(根因是 worktree 复用时 task store loop_id 未迁移,见 P0-A)。 |
| **本质** | 编排机制能跑 4 个 unit,但跑不到第 5 个。失败的不是内容质量,而是 RALPH 自己不知道"4/8 完成时该怎么办"。 |

---

## 2. 执行链路对比图(Agent A 产出,主 Agent 校正)

### 2.1 preset 预期链路(主仓库 `presets/en/ce-executor-serial.yml`)

| 项目 | 实际状态 |
|---|---|
| **hats 数** | 10(coordinator / executor / validator / fixer / review-coordinator / dimension-reviewer / review-synthesizer / shipper / reporter / progress-steward) |
| **execution_mode** | `isolated` |
| **completion_promise** | `"LOOP_COMPLETE"` |
| **required_events** | `["report.done"]` |
| **starting_event** | `"work.start"` |
| **max_iterations** | 50 |
| **enforce_hat_scope** | true |
| **enforce_current_unit** | true |
| **suppress_human_guidance** | true |
| **max_residuals** | 8 |
| **预设事件流** | `work.start → coordinator(work.ready U1) → executor(work.done) → validator(test.passed) → coordinator(work.ready U2) → ... → coordinator(review.start) → review-coordinator(6 维 walk) → review-synthesizer(review.complete) → coordinator(plan.complete 或 work.ready fix-units) → shipper(REVIEW_COMPLETE) → reporter(report.done → LOOP_COMPLETE)` |

### 2.2 plan 预期单元(U1-U8)

| Unit | 标题 | 依赖 | 状态 |
|---|---|---|---|
| U1 | Contract 与 accessor:补充 hat-scope 元数据及 plan-baseline artifact 契约 | 无 | ✅ commit `0d81aab` 已合 |
| U2 | 生成器:为每个 Hat 产出 event_filter、exempt_topics 与 topic_deny_rules | U1 | ✅ |
| U3 | validate_config.py:放行 exempt_topics / topic_deny_rules + light scope 预检 | U2 | ✅ |
| U4 | audit_config.py:新增 check_hat_scope_invariant 审计 | U2 + U3 | ✅ |
| U5 | audit_config.py:接入 ralph preflight --strict 作为 hard gate | U4 | ❌ 未实施 |
| U6 | runtime_audit.py 与 hat-contracts.yml:识别 plan-baseline 产物 | U1 | ❌ 未实施 |
| U7 | 文档:ralph-cli-helper.md 新增 ORCHESTRATOR CONTEXT 小节 | 无 | ❌ 未实施 |
| U8 | 下游 skill 同步:review 与 report 识别新 finding 类型与产物路径 | U4 + U6 | ❌ 未实施 |

### 2.3 实际事件流时间轴(22 个 events)

> 文件:`.ralph/events-20260626-160420.jsonl`(20 business + 2 loop-level boundary)

| # | ts (UTC) | iteration | hat | topic | payload 关键字段 | 状态 |
|---|---|---|---|---|---|---|
| 1 | 16:04:20 | 0 | loop | `work.start` | plan 路径 + worktree 复用 hint | ✅ |
| 2 | 16:10:15 | 1 | coordinator | `work.ready` | step-01, task_id=`1782490209-u001` | ✅ |
| 3 | 16:15:59 | 2 | executor | `work.done` | changed_lines=396, **task_id 无 loop_id** | ✅ → TaskWrongLoop reject |
| 4 | 16:19:37 | 2 | executor | `work.done` (retry) | 同 payload | ✅ retry |
| 5 | 16:50:17 | 3 | validator | `test.passed` | 222/238 pass | ✅ |
| 6 | 16:52:56 | 4 | coordinator | `work.ready` | step-02, task_id=`1782490209-u002` | ✅ |
| 7 | 16:53:00 | 4 | coordinator | `work.ready` (retry) | 同 | ✅ retry |
| 8 | 17:10:30 | 6 | executor | `work.done` | changed_lines=417, 14/14 u2 ok | ✅ |
| 9 | 17:15:47 | 6 | executor | `work.done` (retry, cf0f) | task_id=`1782492870-cf0f` | ✅ retry |
| 10 | 17:34:35 | 7 | validator | `test.passed` | 198/238 pass, 24 failures acknowledged | ✅ |
| 11 | 17:36:57 | 8 | coordinator | `work.ready` | step-03, **`task_id=""` 空字符串!** | ✅ retry |
| 12 | 18:03:43 | 10 | executor | `work.done` | changed_lines=364, task_id=`1782496128-eb32` | ✅ |
| 13 | 18:20:37 | 11 | validator | `test.failed` | 39 failures, U2 red_team bug | ✅ |
| 14 | 18:20:42 | 11 | validator | `test.failed` (retry) | 同 payload | ✅ retry |
| 15 | 18:30:20 | 12 | fixer | `fix.applied` | fix_round=1, commit_count=4 | ✅ |
| 16 | 18:43:27 | 14 | validator | `test.passed` | 222/238 pass(回 U1 baseline) | ✅ |
| 17 | 18:46:09 | 15 | coordinator | `work.ready` | step-04, **`task_id=""` 空字符串!** | ✅ retry |
| 18 | 18:46:16 | 15 | coordinator | `work.ready` (retry) | 同 | ✅ retry |
| 19 | 19:08:24 | 15 | executor | `work.done` | changed_lines=751, task_id=`1782499797-0ce8` | ✅ |
| 20 | 19:23:04 | 16 | validator | `test.passed` | 222/238 pass | ✅ |
| **21** | **19:41:41** | **17** | **shipper** | **`REVIEW_COMPLETE`** | **pass_or_fail=fail, verdict=fail** | ❌ **跳过 review-chain 3 hat** |
| 22 | 19:44:21 | 17 | reporter | `report.done` | awaiting_decision=true, pass_or_fail=fail | 🔁 |
| ⊥ | 19:44:39 | 19 | loop | `loop.terminate` | reason=`review_failed` | ❌ |

**Symbol**: ✅ 符合预期 / ❌ 偏离 / ⏸️ 被 stall_recovery 介入空转 / 🔁 走非主线路径

### 2.4 preset 预期 vs 实际(per-iteration 对比)

| Iter | 预期(ce-executor-serial 主线) | 实际 | 状态 |
|---|---|---|---|
| 0 | loop 启动,`work.start` 进入 coordinator | ✅ `work.start` 入站 | ✅ |
| 1 | coordinator 解析 plan + emit `work.ready` for U1 | ✅ emit `work.ready` step-01 | ✅ |
| 2 | executor 跑 U1 → `work.done` | ✅ emit `work.done`(2 次,TaskWrongLoop reject) | ✅ (retry) |
| 3 | validator 跑 test.passed → 触发 coordinator emit `work.ready` U2 | ✅ test.passed 222/238 | ✅ |
| 4 | coordinator emit `work.ready` U2 | ✅ emit(2 次,空 task_id 触发 retry) | ✅ (retry) |
| 5 | executor U2 implementation | ⏸️ 空转(stall_recovery 介入) | ⏸️ |
| 6 | executor emit `work.done` U2 | ✅ emit `work.done` step-02 | ✅ |
| 7 | validator emit `test.passed` U2 | ✅ 198/238(acknowledged 24 failures) | ✅ |
| 8 | coordinator emit `work.ready` U3 | ✅ emit(2 次) | ✅ |
| 9 | executor U3 implementation | ⏸️ 空转(stall_recovery) | ⏸️ |
| 10 | executor emit `work.done` U3 | ✅ emit | ✅ |
| 11 | validator emit `test.failed` U3(U2 generator bug) | ✅ emit `test.failed`(2 次) | ✅ |
| 12 | fixer 修复 → `fix.applied` | ✅ emit `fix.applied` Round 1 | ✅ |
| 13 | validator 重跑 → `test.passed` | ⏸️ 空转 | ⏸️ |
| 14 | validator emit `test.passed` | ✅ emit 222/238 | ✅ |
| 15 | coordinator emit `work.ready` U4 + executor work.done | ✅ emit + work.done 751 lines | ✅ |
| 16 | validator emit `test.passed` U4 | ✅ emit 222/238 | ✅ |
| **17** | **coordinator 应该 emit `plan.complete` 或 `review.start`(plan 4/8,需要推进 U5)** | ❌ **缺失 — 没 emit `review.start` / `plan.complete`,被 progress-steward → shipper 接管** | **❌ 关键偏离** |
| 17 | reviewer pipeline 6-dim walk + 6 dimension-reviewer 启动 | ❌ **完全缺失,review-* 系列 0 个** | ❌ |
| 17 | review-synthesizer `review.complete` | ❌ 缺失 | ❌ |
| 17 | coordinator emit `plan.complete` 或 `plan.blocked` | ❌ **缺失** | ❌ |
| 17 | shipper → `REVIEW_COMPLETE`(基于 plan.complete) | 🔁 shipper 直接 emit,但 **payload reason 字段为空** → hard-fail | 🔁 |
| 18 | reporter → `report.done` → `LOOP_COMPLETE` | 🔁 reporter emit `report.done(fail)`;**未 emit `LOOP_COMPLETE`**(因 fail 禁发) | 🔁 |
| 19 | LOOP_COMPLETE → loop 自然终止 | ❌ 由 `loop.terminate` 强行终止(reason=`review_failed`) | ❌ |

### 2.5 拓扑断点分析(iter 17 处)

**预期应该出现但实际缺失的事件**:
- `coordinator review.start` — 缺失
- `review-coordinator review.dimension.ready`(6 次) — 缺失
- `dimension-reviewer review.dimension.done`(6 次) — 缺失
- `review-coordinator review.dimensions.complete` — 缺失
- `review-synthesizer review.complete` — 缺失
- `coordinator plan.complete` — 缺失(plan 4/8 完成不应 emit)
- `coordinator plan.blocked(reason=?)` — **reason="" 被 drift 报 critical**

**实际在 iter 17 出现的**:shipper 被 task.resume(progress-steward)强行激活 → 直接 emit `REVIEW_COMPLETE(pass_or_fail=fail, verdict=fail)` → reporter emit `report.done(pass_or_fail=fail, awaiting_decision=true)` → verdict gate 看到 last mirror 是 fail → 自动 terminate。

### 2.6 task 生命周期(`agent/tasks.jsonl` 7 行)

| id | key | status | loop_id | created → closed |
|---|---|---|---|---|
| `task-1782490209-u001` | step-01 contract-and-accessor | closed | **None**(legacy) | 16:12:42 → 16:20:20 |
| `task-1782490209-u002` | step-02 generator-scope-pinning | closed | **None**(legacy) | 16:53:34 → 17:10:45 |
| `task-1782492870-cf0f` | step-02(重生成) | closed | 2026-06-26-001-... | 16:54:30 → 17:16:12 |
| `""`(空 id!) | step-03 validate-scope-keys | **open** | None | 17:37:22 → 永远未 close |
| `task-1782496128-eb32` | step-03(重生成) | closed | 2026-06-26-001-... | 17:48:48 → 18:03:55 |
| `""`(空 id!) | step-04 audit-hat-scope-invariant | **open** | None | 18:46:55 → 永远未 close |
| `task-1782499797-0ce8` | step-04(重生成) | closed | 2026-06-26-001-... | 18:49:57 → 19:09:30 |

**关键观察**:
- 早期 task (u001/u002) **没有 loop_id** → 触发 execution_contract 的 `TaskWrongLoop` 错误
- step-03 和 step-04 出现 **空 task_id** 的占位任务(由 coordinator 第 1 次 emit `work.ready` 时生成),后由 executor 创建真实 task 覆盖
- 7 行任务对应 4 个 unit,但实际只完成 4/8

### 2.7 修复/恢复机制运转统计

**fix-log**(1 round):
- Round 1(step-03 red_team event_filter bug 修复):locate+fix 一次性成功,commit_count=4,changed_lines=1174
- **没有 exhausted**:fix budget 仅用 1/10
- `fix.applied` 后 validator 立刻 test.passed(回归 222/238,与 U1 baseline 对齐)

**recovery.jsonl**(28 条 envelope,outcome 分布):

| outcome | 数量 | 含义 |
|---|---|---|
| `recovered` | 7 | (sync_up_to_date 1 + TaskWrongLoop 6) |
| `pending` | 13 | 仍在等待重试 |
| `escalated` | 2 | validator 未在 30s 内激活 |
| `repeated` | 3 | stall_recovery 反复触发 |

按 source 分布:
- `agent_doc_sync`: 1 (info)
- `execution_contract` (TaskWrongLoop): 2 (init),后续 6 条为 outcome_updated 至 recovered/repeated
- `drift_monitor` (`drift_field_completeness`): 5 (kind/message 缺失为 critical;reason/target_hat 80% 为 warning)
- `stall_recovery` (`handoff_dispatch_timeout`): 2 (iter 3/iter 7 validator 30s 未激活),后续 outcome_updated

**drift.jsonl**(5 个 finding):
1. iter 2:`task.resume.kind` 0/1 (critical)
2. iter 2:`human.guidance.message` 0/1 (critical)
3. iter 7:`task.resume.reason` 4/5=80% < 85% (warning)
4. iter 7:`task.resume.target_hat` 4/5=80% < 85% (warning)
5. iter 17:`plan.blocked.reason` 0/1 (critical) — **本次终止的根因**:shipper 的 plan.blocked reason 字段为空,触发 drift

**diagnosis-summary**:`recovery_count=28`、`drift_finding_count=0`(计数口径与 drift.jsonl 实际 5 条不一致)

---

## 3. 历史问题上下文(Agent B 产出)

### 3.1 本次命中"已识别问题"的比例(类别 1-19)

| 本次现象 | 关联历史类别 | 关联度 | 闭环状态 |
|---|---|---|---|
| `TaskWrongLoop { actual_loop: None }` | `execution_contract.rs:499-532` legacy task + loop_scoped=true 设计性 fail-closed | 极高 | **未闭环** — 没有"task store 自动回填 loop_id"路径 |
| `handoff_dispatch_timeout` consumer validator | `event_loop/mod.rs:5480-5560` U7 机制 | 高 | 机制已落但 consumer validator 缺 `kind` 字段 |
| `task.resume kind/reason/target_hat` 缺失 | `drift/engine.rs:1052-1120` 004 plan 半边修复 `enrich_task_resume_payload_with_stage` 缺 `kind` | 极高 | 004 修了 `build_task_resume_payload`,但对偶函数 `enrich_task_resume_payload_with_stage` 未加 |
| `plan.blocked reason=""` → shipper hard-fail | `presets/en/ce-executor-serial.yml:2117-2204` recoverable reason 白名单 + `mechanism-close-loop solution:178-185` P1-1 | 极高 | **P1-1 未闭环** — shipper 镜像 `pass_with_residuals → fail` 30 天反复 |
| `REVIEW_COMPLETE(pass_or_fail=fail)` | `mod.rs:1684-1725` verdict_gate 双层检测 + `loop_state.rs:1261-1285` last_upstream_verdict_payload | 极高 | **未闭环** — `pass_with_residuals` 判定在 prompt 而非 Rust,无 SSOT |
| `loop-termination-reason.json={review_failed:{topic:report.done}}` | `primary-20260624-092856` 报告 E-7 字面同型 | 极高 | **未闭环** — 2026-06-23 plan 加了 `schema_version`,但 verdict 命名仍是旧的 |
| recovery.jsonl oscillate Pending→Recovered→Repeated | `diagnosis/responder.rs:583-657` R11 tripwire | 极高 | **部分闭环** — R-C1 写了升级但 typed consumer 未接 |
| review-chain 完全没启动 | `preset_lint/review_terminal_coherence.rs` KTD-RTC 3 道防线 | 极高 | 防线已落但 4/8 完成的 plan 触发 shipper,绕过 review |

### 3.2 3 大架构不稳定因素(6-21 报告,与本次高度相关)

1. **`task.resume` 自指循环** — 直接命中(本次 28 条 recovery 绝大多数都是这个)
2. **软提示架构** — 直接命中(`reason` 字段空 = 软提示被丢)
3. **多状态源竞争** — 间接命中(worktree 复用导致 task store 状态分裂)

### 3.3 memory `plan-blocked-recovery-via-human-signoff` 关联

memory 中记录的"当 `report.done` with `awaiting_decision: true` 时,ralph hat 可以发布 `review.passed` with `human_signoff: true` 让 plan-gate 路由到 `queue.advance`"——这是当前**唯一绕路方案**。本次可以走 human_signoff 路径,但需要 hat 主动发 `review.passed`,目前没有机制强制 hat 这么做。

### 3.4 已识别修复包(主仓已有,未跑)

1. `ralph-orchestrator/docs/plans/2026-06-26-001-fix-ce-executor-serial-four-recurrences-plan.md` U1-U7(typed Verdict + 义务模型 + hat scope + 自愈边界)
2. `ralph-orchestrator/docs/achieved/report/2026-06-23-005-fix-ce-executor-serial-hard-gate-half-edge-recovery-plan.md` 004 plan typed kind wiring
3. `ralph-orchestrator/crates/ralph-core/src/event_loop/verdict.rs` typed `Verdict` 枚举(commit 7e3aa9ef, ea807f6f,Rust 端已闭环,但 shipper prompt 翻译未用)

### 3.5 30 天第 7+ 次复发(6-26 报告"修复机制系统性失效")

3 因素叠加,本次 run 命中 95%:
- 因素 1:`task.resume` 自指循环(**直接命中**)
- 因素 2:软提示架构(**直接命中**)
- 因素 3:多状态源竞争(**直接命中**)

---

## 4. 证据清单(Agent C 产出)

### 4.1 P0(直接导致终止)— 6 条

| # | 偏离描述 | preset/plan 期望 | 实际证据 | 历史关联 |
|---|---|---|---|---|
| **E5** | **`plan.blocked reason=""` 触发 shipper hard-fail** | shipper reason-based routing 要求 reason 非空 | `events-20260626-160420.jsonl` 第 21 行;`recovery.jsonl:28` iter=17 drift critical `reason present in 0/1 events`;`shipping.md:5` 自己承认"payload reason 为空" | 是(mechanism-close-loop P1-1) |
| **E6** | **review pipeline 6-dim walk 完全缺失** | preset 6-hat review-chain 应当产出 `review.start → review.dimension.* → review.complete` | 22 个 events 中 review.* 系列为 0;`progress.md:1-11` Current Step=(none);`recovery.jsonl` 0 条 review 相关的 envelope | 是(KTD-RTC 3 道防线已落但 4/8 完成的 plan 触发 shipper,绕过 review) |
| **E1** | **coordinator emit `work.ready` 缺 `task_id`** | preset `require_task.id_field=task_id`,但 coordinator 自身发的 task 给的 `task_id=""` | `events.jsonl:11` (step-03)、`events.jsonl:17` (step-04),`task_id:""`;`tasks.jsonl:4,6` id 空字符串 | 否(新特征) |
| **E2** | **Executor 发的 work.done 触发 execution_contract 拒绝(`actual_loop:None`)** | preset `require_task.loop_scoped=true` 要求 `task_id` 对应的 task 必须有 `loop_id` 字段 | `recovery.jsonl:2` iter=2 (`task-1782490209-u001`, `actual_loop: None`)、`recovery.jsonl:7` iter=6 (`task-1782490209-u002`);契约实现 `execution_contract.rs:518-532` | 否(本次新增字面 signature,根因已知) |
| **E13** | **tasks.jsonl 中 2 个 task 无 loop_id、1 个 id 空** | preset `loop_scoped=true` 要求 task 必须有 loop_id | `tasks.jsonl:1` u001 loop_id=null;`tasks.jsonl:2` u002 loop_id=null;`tasks.jsonl:4,6` id="" | 是(`execution_contract.rs:500-516` legacy 分支) |
| **E19** | **coordinator step-03/04 task_id 永远空,executor 重发带 loop_id 的 task** | preset `require_task.id_field=task_id` | `events.jsonl:11,17` coordinator 用 `task_id=""`;后续 executor 必须自创 task_id(`task-1782496128-eb32` / `task-1782499797-0ce8`) | 否 |

### 4.2 P1(异常行为,可能掩盖真问题)— 8 条

| # | 偏离描述 | preset/plan 期望 | 实际证据 | 历史关联 |
|---|---|---|---|---|
| **E3** | `task.resume` payload 缺 `kind`/`reason`/`target_hat` | preset `event_filter.events` 内的消费契约要求 `task.resume` 含 kind+reason+target_hat | `drift.jsonl:1,3,4` — `kind` 0%(crit)、`reason` 80%(warn)、`target_hat` 80%(warn);`recovery.jsonl:3-4, 11-12` iter 2/7 多次报 | 是(004 plan 半边修复 `enrich_task_resume_payload_with_stage` 缺 `kind`) |
| **E7** | **同一 task-1782490209-u001 反复 stall_recovery escalate** | preset stall_recovery 应当自愈或 escalation 后真正修复 | `recovery.jsonl`:iter 2(pending)→ iter 3(escalate)→ iter 7(recovered)→ iter 8(repeated)→ iter 10(recovered)→ iter 11(repeated)→ iter 15(recovered)→ iter 16(repeated) | 是(R-C1 / 6-12 report P2-2 + 6-24 P2-5) |
| **E8** | **同一 task-1782490209-u002 同样 4 轮 stall 循环** | 同 E7 | `recovery.jsonl` iter 6/7/8/10/11/15/16 多个 envelope | 同 E7 |
| **E9** | **`recovery_count=28` 与 `drift_finding_count=0` 计数不一致** | `diagnosis-summary.json` 应一致报告 | `diagnosis-summary.json:11-12` 显示 recovery=28 drift=0,但 `drift.jsonl` 实际有 5 条 finding(4 critical + 2 warning) | 否 |
| **E11** | **fix.applied 后 iteration 15 验证通过但 iteration 16 立即又 stall** | preset fixer 上限 + 修复成功应推进 loop | `recovery.jsonl`:iter 15 fix.applied → test.passed(iter 15)→ iter 16 同一 task 又 repeated stall | 否 |
| **E14** | **`work.done` 同一 step 发了 2 次(executor 重发绕过 contract)** | preset `require_task` + `event_filter` 应去重 | `events.jsonl:3,4` 都是 step-01 work.done(ts 16:15:59 + 16:19:37,差 4 分钟);`events.jsonl:8,9` step-02 同样发两次 | 否 |
| **E16** | **drift.jsonl 5 条 finding 但 loop 没有 human_review_pending / block_drift 处置** | preset drift_monitor 应该把 critical finding 升级到 runtime guidance 或 alert | `drift.jsonl:1,2,5` 都是 critical;`recovery.jsonl:3,4,28` 只是记为 `outcome=pending`,没真正拦截 | 是(6-12 report P2-2) |
| **E18** | **validator 任务 ID 一致性:step-02 同 task 发过两次 work.done 用不同 task_id** | preset require_task 用 task_id 关联,不允许换 ID | `events.jsonl:8` 用 u002,`events.jsonl:9` 改用 cf0f | 否 |

### 4.3 P2(表象问题,无影响)— 6 条

| # | 偏离描述 | 实际证据 | 历史关联 |
|---|---|---|---|
| **E4** | `human.guidance` 缺 `message` 字段 | `drift.jsonl:2` — `message` 0%(critical),iter 2;scratchpad.md:4 实际内容是有意义的 | 否 |
| **E10** | `loop-termination-reason.json={review_failed:{topic:"report.done"}}` 语义错误 | `loop-termination-reason.json:1`;实际终止路径是 shipper `REVIEW_COMPLETE verdict=fail` → reporter `report.done` → 终止 | 是(primary-20260624-092856 报告 E-7 字面同型) |
| **E12** | progress.md 记 4/4 step done,但 plan 实际 4/8 unit | `progress.md:1-10` Completed Steps=step-01..04;plan 实际 U1-U8,只完成 U1-U4 | 否 |
| **E17** | report.done `awaiting_decision=true` 但 loop 已终止 | `events.jsonl:22` reporter `awaiting_decision=true`+`pass_or_fail=fail` | 否 |
| **E20** | final commit `0d81aab`(U4)已合,但 loop 终止时仍标"no final commit" | `summary.md:25` vs `shipping.md:108` 两个字段都存在但语义不同 | 否 |
| **E-fixcount** | `fix.applied.commit_count` 漂移风险(commit_only 合约) | `fix-log.md:24` 写 `commit_count=4`(累计),per-round delta 应为 1 | 否 |

### 4.4 修复机制完整运转链

```
iter 11: validator emit test.failed (U2 generator bug, 39/238 fail)
   ↓
iter 11: drift_monitor 检测 test.failed 重试 1 次 (event #14)
   ↓
iter 12: fixer 一次性诊断 + 修复
   - causal chain gate: U2 universal event_filter block overwrites red_team narrower allowlist
   - fix: generate_autoresearch.py:1175-1191 移除 `if isolation_mode != "isolated":` 门控
   - commit: 0d62499 (commit_count=4 cumulative, changed_lines=1174)
   ↓
iter 14: validator 重跑 → test.passed 222/238 (回 U1 baseline)
   ↓
iter 15-16: coordinator → executor → validator 完成 U4
   ↓
iter 17: plan.blocked 触发但 reason 字段空 → shipper fallback hard-fail
   ↓
iter 17: shipper → REVIEW_COMPLETE(fail)
   ↓
iter 18: reporter → report.done(awaiting_decision=true)
   ↓
iter 19: loop.terminate(reason="review_failed" — 命名错误,见 P0-C)
```

---

## 5. 问题归因表(P0/P1/P2)

| 优先级 | 问题描述 | 根因分类 | 证据 | 历史关联 |
|---|---|---|---|---|
| **P0-A** | **TaskWrongLoop 反复触发**:`task-1782490209-u001/u002` 在 7 个 iteration 反复被 `execution_contract` 拒为 `actual_loop: None`,drift_monitor 把它们标 Pending → Recovered → Repeated → Pending,28 条 recovery 绝大多数都是这条 | **loop(契约) + agent(执行) 叠加** | `recovery.jsonl:2,7,9,10,13,15,16,19,22,25-27`;契约实现 `crates/ralph-core/src/execution_contract.rs:518-532`(legacy task + loop_scoped=true 直接拒绝,无回填路径) | 否(新增字面 signature,根因已知) |
| **P0-B** | **`progress-steward` 没有"修正 task"的逃生通道** | **loop 机制缺失 + preset 设计** | preset 在 `presets/en/ce-executor-serial.yml:2408-2478` 只允许 steward 选 5 种 emit:`work.ready`/`review.start`/`task.resume`/`plan.blocked`,**没有任何路径回填 legacy task 的 loop_id**;`process_output` 的 stall_recovery 路径 `crates/ralph-core/src/event_loop/mod.rs:5448-5531` 只合成 `task.resume`、不修改 tasks.jsonl | 否 |
| **P0-C** | **shipper hard-fail vs `loop-termination-reason.json` 错位**:`plan.blocked` 走 shipper hard-fail → `REVIEW_COMPLETE{verdict:"fail"}`,但 runtime 走的是 verdict gate "last mirror is report.done" 路径(`mod.rs:1163-1201`),把 topic 写成 `report.done` 而非 `REVIEW_COMPLETE`,**写入的是 verdict mirror 而不是 shipper emit 的话题** | **loop(verdict gate 设计) + 编排数据漂移** | `loop-termination-reason.json:1` 写 `{"review_failed":{"topic":"report.done"}}`;`mod.rs:1190-1200` `expected_last = gate.additional_topics.last()`,so 终止时 `topic` = mirror 末端(report.done),不是 shipper emit 的话题(REVIEW_COMPLETE)。这正是 2026-06-10 P0-C 自动终止设计的预期行为,但**与 shipper 路由无关**——两个机制并存 | 是(`mod.rs:1167` 注释本身就是 P0-C,`mechanism-close-loop` solution:178-185) |
| **P0-D** | **`completion_promise: "LOOP_COMPLETE"` 与 reporter `pass_or_fail="fail"` 禁止 emit LOOP_COMPLETE 的契约冲突** | **preset 设计(契约不全)** | preset `presets/en/ce-executor-serial.yml:80-81` 写 `completion_promise: "LOOP_COMPLETE"` + `required_events: ["report.done"]`;reporter `obligations.conditional_forbid_topics`(`presets/en/ce-executor-serial.yml:2235-2239`)在 `pass_or_fail="fail"` 时**禁止** emit `LOOP_COMPLETE`。两条规约在同一份 preset 内**互斥** | 否 |
| **P0-E** | **review pipeline 6-dim walk 完全缺失**:22 events 0 个 review-*;review-coordinator / dimension-reviewer / review-synthesizer 3 hat 从未激活 | **preset 拓扑 + 内容(plan 4/8)叠加** | `events-20260626-160420.jsonl` 0 个 review.*;`progress.md:1-11` Current Step=(none);coordinator iter 17 未 emit `review.start` / `plan.complete` | 是(KTD-RTC 3 道防线已落但 4/8 完成的 plan 触发 shipper 绕过 review) |
| **P0-F** | **coordinator 早期 task 生成路径不写 loop_id**:task-1782490209-u001/u002 无 loop_id,触发 TaskWrongLoop | **agent 执行漂移(legacy schema)+ preset 契约过严** | `tasks.jsonl:1,2` loop_id=null;`recovery.jsonl:2,7` TaskWrongLoop | 是(`execution_contract.rs:500-516` legacy 分支 + 45d 适配 plan N1 未闭环) |
| **P1-A** | **stall_recovery "escalate → task.resume" 死循环**:handoff_dispatch_timeout → task.resume(target=validator) → validator 不激活 → 再 timeout → 再 resume(28 次) | **loop(stall_recovery 缺降级) + preset(progress-steward 缺降级)** | `recovery.jsonl:5,8` 显示 validator handoff 30s 超时;`progress-steward` 步 2 表(`presets/en/ce-executor-serial.yml:2440-2472`)对"validator handoff timeout"没有降级路径,只能重发 task.resume | 是(同主题 P1,R3 残留) |
| **P1-B** | **`plan.blocked` reason 字段缺必填校验**:drift_monitor 报 `field reason on topic plan.blocked present in 0/1 events (0.0%)` | **preset schema 漂移 + agent 执行漂移** | `recovery.jsonl:28`(iteration 17);schema `presets/schemas/ce-executor-serial.yml:279-294` 已声明 `required_fields: [reason]`,但 shipper 实际拒收"空 reason"的方式是 hard-fail → REVIEW_COMPLETE fail,**Drift Monitor 报的是 schema_completeness,不是 runtime rejection** | 否 |
| **P1-C** | **`task.resume` schema 不全**(缺 `kind`/`reason`/`target_hat`):drift_monitor 报 80% 完整性,5 条事件中 4 条全字段,1 条缺字段 | **loop(orchestrator emit 路径缺字段) + preset(schema 必填) 叠加** | `recovery.jsonl:3-4`(iteration 2 kind=0%) + `recovery.jsonl:11-12`(iteration 7 reason=80%, target_hat=80%);`presets/schemas/ce-executor-serial.yml:340-345` 要求 `reason+target_hat+kind`;`mod.rs:5508-5516` stall_recovery 实际 emit 的 JSON **没有 `kind` 字段** | 是(同类 P2 漂移,004 plan 半边修复) |
| **P1-D** | **drift_monitor 反复 Pending/Repeated/Recovered 状态翻转**:同一条 retry_key 在 iteration 7/10/11/15/16 反复变化 | **loop(drift_monitor state 写入策略) 设计选择** | `recovery.jsonl:8-27` 同一 `retry_key` 在多个 iteration 反复出现,outcome 字段反复切;但 `mod.rs:8588-8607` 中 `batch_sync_source` 不会改 outcome;**drift_monitor 内部显式更新上一条 envelope 的 outcome**,这是诊断放大器 | 否 |
| **P1-E** | **fix.applied 后立即又 stall(修复成功 ≠ loop 推进)**:fixer 修了真 bug,但 task_resume 路由又指向同一 task,触发 stall_recovery 重试 counter 不清零 | **preset design gap(fixer output 未清 stall_recovery retry counter)** | `recovery.jsonl` iter 15 fix.applied → iter 16 同一 task repeated | 否 |
| **P2-A** | **`human.guidance` field 完整性 0%** | **loop(human.guidance 几乎没真实发) + preset(schema 必填) 叠加** | `recovery.jsonl:4` iteration 2 报 `human.guidance.message present in 0/1 events (0.0%)`;`presets/en/ce-executor-serial.yml:115-116` 显式 `suppress_human_guidance: true`,说明该事件**几乎被禁用** | 否 |
| **P2-B** | **`recovery_count=28` 与 `drift_finding_count=0` 计数不一致** | **diagnose summary 算法选择** | `diagnosis-summary.json:11-12` 显示 recovery=28 drift=0,但 `drift.jsonl` 实际有 5 条 finding | 否 |
| **P2-C** | **`loop-termination-reason.json` 措辞不准**:`review_failed.topic="report.done"` 与 shipper 实际 verdict=fail 触发终止不一致 | **serde enum 序列化 + verdict gate 命名** | `loop-termination-reason.json:1` 写 `{"review_failed":{"topic":"report.done"}}` | 是(primary-20260624-092856 报告 E-7 字面同型) |
| **P2-D** | **progress.md 4/4 step vs plan 4/8 unit 命名粒度差** | **agent 输出粒度选择** | `progress.md:1-10` Completed Steps=step-01..04;plan 实际 U1-U8,只完成 U1-U4 | 否 |
| **P2-E** | **summary.md final_commit vs shipping.md no final commit 概念混淆** | **文档语义** | `summary.md:25` 写"0d81aab: ..." 是"Final Commit";`shipping.md:108` 写"按 shipper 约束:不在 plan.blocked 上做 final commit" | 否 |

---

## 6. 修复建议(按优先级)

### 6.1 P0 修复(直击根因)

#### P0-1: progress-steward 增加"回填 legacy task 的 loop_id"逃生通道
- **目标**:`presets/en/ce-executor-serial.yml:2408-2478`(progress-steward instructions)+ `crates/ralph-core/src/event_loop/mod.rs:5448-5531`
- **修改**:在 progress-steward Step 2 决策表新增一行"同 loop 内存在 legacy task(loop_id 为 None 且 task_key 以 `ce-executor:{plan_name}:` 开头),且已 escalate 1 次未恢复 → emit `work.ready(re_emigrate_legacy_task=true, task_id, task_key, step)`,或更安全:新增一条 hat emit 主题 `task.relocate_legacy(loop_id=<current>)` 触发 `EventLoop` 在 `mod.rs` 新增的 helper `relocate_legacy_tasks(tasks_path, current_loop_id)`,对所有 `loop_id=None` 且 key 匹配的 task **写回 loop_id**(单次幂等操作,只能在 progress-steward hat 的高权限路径下使用)
- **预期**:TaskWrongLoop 错误一次后,progress-steward 在下次 loop.stalled 触发时自动把 legacy task loop_id 补齐,executor 再发 work.done 不再被拒
- **验证**:`cargo nextest run -p ralph-core -- progress_steward_legacy_task_relocate`(新增 unit test)+ 在 worktree 中跑实际 run,观察 recovery.jsonl 中 `TaskWrongLoop` retry_key 的 outcome 变为 `recovered` 后不再 Pending 翻转

#### P0-2: 补齐 `task.resume` schema 必填字段 `kind`(stall_recovery emit 路径)
- **目标**:`crates/ralph-core/src/event_loop/mod.rs:5508-5516`(handoff escalation JSON payload)
- **修改**:在 `serde_json::json!` 块中新增 `"kind": "handoff_dispatch_timeout"`(或者读 `ReasonClass`);同时检查 `build_task_resume_payload`(`rejection.rs:503-519`)的 emit 路径,确认所有路径都加 `kind`
- **预期**:drift_monitor 报 `task.resume.kind` 完整性从 80%(4/5)升到 100%
- **验证**:`cargo nextest run -p ralph-core -- drift_monitor_task_resume_kind`(新增 BDD)+ `cargo nextest run -p ralph-core --test scenarios -- workflow_guard_emit_completeness`

#### P0-3: 解除 `completion_promise: "LOOP_COMPLETE"` 与 reporter fail 路径的语义冲突
- **目标**:`presets/en/ce-executor-serial.yml:80-81` + `2230-2239`
- **修改**:把 `completion_promise: "LOOP_COMPLETE"` 改为 `completion_promise: "report.done"`(因为 reporter 在 fail 路径只能 emit report.done,不能 emit LOOP_COMPLETE);或者把 `required_events` 改为 `["REVIEW_COMPLETE", "report.done"]`(双 sentinel,verdict gate 走 last mirror);同时把 verdict gate 的 last mirror 显式标注:`gate.additional_topics.last() == "report.done"` 时,终止原因应清晰区分 shipper 翻译失败 vs 真正的 plan-end
- **预期**:fail 路径以单一信号(report.done)终止,loop-termination-reason.json 含义清晰,避免 verdict gate 自动接管时与 shipper 路由重叠
- **验证**:`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`(SSOT 校验)+ `cargo nextest run -p ralph-core --test scenarios -- ce_executor_serial_fail_path_terminates_on_report_done`

#### P0-4: 文档化"P0-C verdict gate 自动终止 ≠ shipper routing"
- **目标**:新增 `ralph-orchestrator/docs/solutions/integration-issues/ce-executor-serial-fail-path-verdict-gate-vs-shipper.md`
- **修改**:文档化"P0-C verdict gate 自动终止 ≠ shipper routing"——当 shipper hard-fail + reporter 收到 fail + reporter emit report.done(下游 mirror),verdict gate 观察到 fail verdict 在 last mirror 上,自动以 `ReviewFailed{topic: "report.done"}` 终止。这是设计预期,但和 shipper 路径视觉上重叠,需要明确"双重保险"
- **预期**:未来诊断时不再把 `review_failed.topic=report.done` 误判为 shipper 路由 bug
- **验证**:审阅 docs 后,在 plan-follow-up 注释里明确指向该 solutions 文档

### 6.2 P1 修复(机制增强)

#### P1-1: progress-steward 增加"validator handoff 超时 N 次 unrecoverable"分支
- **目标**:`presets/en/ce-executor-serial.yml:2440-2472`(Step 2 决策表)
- **修改**:在表中增加新行:`| Validator handoff 连续 3 次 timeout(同 loop 内) | plan.blocked(reason=validator_handoff_unrecoverable_after_<N>_retries) | shipper → reporter(hard-fail)`;同样理由:防止 stall_recovery 反复 task.resume → validator 不激活的死循环
- **预期**:stall_recovery 链最多 3 次 resume 后强制 unrecoverable,进入 hard-fail 终止路径,不再消耗 28+ 条 recovery 记录
- **验证**:新增 BDD scenario `cargo nextest run -p ralph-core --test scenarios -- validator_handoff_timeout_3x_emits_unrecoverable`

#### P1-2: 校验 `plan.blocked` 必带 `reason` 在 shipper dispatch 入口
- **目标**:`crates/ralph-core/src/event_loop/policy.rs`(event_policy check) 或 shipper hat prompt
- **修改**:在 event_policy 的 `on_violation: reject_with_resume` 路径中,显式对 `plan.blocked` 主题 payload 做 `reason ∈ required_fields` 校验,缺字段直接 reject + emit `task.resume(target=coordinator, reason=missing_plan_blocked_reason)`;同时考虑在 `crates/ralph-cli/src/preflight.rs` 加 `plan.blocked reason` 必填的 schema 校验
- **预期**:drift_monitor 的 `plan.blocked.reason present in 0/1 events` 警告消失
- **验证**:`cargo nextest run -p ralph-core -- policy_enforces_plan_blocked_reason_field` + `cargo nextest run -p ralph-cli --bin ralph -- emit_plan_blocked_without_reason_rejected`

#### P1-3: 抑制 drift_monitor 对同 retry_key 的 outcome 反复更新
- **目标**:drift_monitor 内部实现(grep `recovery_outcome_update` 找到处理 envelope 更新 outcome 的位置)
- **修改**:当同 `retry_key` 在 N iterations 内反复出现时,只在**最后一次迭代**写 final outcome,中间 iteration 不再写 `recovery_outcome_update` envelope,避免噪声污染 recovery.jsonl
- **预期**:recovery.jsonl 中 `recovery_outcome_update` 反复出现的"Pending ↔ Recovered ↔ Repeated"循环消失,真正不可恢复的 issue 更容易被识别
- **验证**:`cargo nextest run -p ralph-core -- drift_monitor_no_outcome_update_loop`

### 6.3 P2 修复(噪声治理)

#### P2-1: 删除 `human.guidance` 主题或迁移到低噪声通道
- **目标**:`presets/en/ce-executor-serial.yml:115-116`(`suppress_human_guidance: true`)+ `presets/schemas/ce-executor-serial.yml:365-368`(schema)
- **修改**:既然 `suppress_human_guidance: true`,drift_monitor 还把 `human.guidance` 列入完整性检查就产生永久 0% 警告。要么:① 把 `human.guidance` 从 schema 删除(因为它已被抑制,几乎不存在);② 或者把 drift_field_completeness 阈值在 0 样本时改为"skip"而非"fail"
- **预期**:drift 噪声减少,真正的 schema 漂移更醒目
- **验证**:BDD 跑一次 plan 后,grep `drift.jsonl` 中 `human.guidance` 警告不再出现

### 6.4 短期绕路方案(立即可走)

依据 memory `plan-blocked-recovery-via-human-signoff`:
- 当 reporter emit `report.done(pass_or_fail=fail, awaiting_decision=true)` 时,操作者可手动发布 `review.passed(human_signoff=true)`,让 plan-gate 路由到 `queue.advance`,再走 coordinator 推进 U5-U8 计划。
- 这是当前唯一"绕路"方案,本次可走(已在 `presets/en/ce-executor-serial.yml` 中具备必需的事件支持)。

---

## 7. 总结

本次 run 的根因是**三因素叠加**:
1. (1) worktree 之外的 universal-autoresearch 共享 tasks.jsonl 存在 legacy task(无 loop_id),
2. (2) ce-executor-serial 的 `require_task.loop_scoped: true` 严格契约拒绝这些 task,
3. (3) **progress-steward 没有"自动回填 loop_id"的逃生通道**,只能反复 task.resume(executor 不接 task.resume 走 stall_recovery 链),最终 19 个 iteration 中 15 个在 stall/recovery/drift 循环里空转。

终止路径 verdict gate(2026-06-10 P0-C)的设计是预期行为——但与 shipper 路由在 fail 路径上语义重叠,造成"loop-termination-reason.json 写 `review_failed.topic=report.done`"的表观错位。

**修复优先级**:
- **P0-1(progress-steward 回填能力)**是直击根因
- **P0-2/P0-3** 是表观契约修复
- **P1-1/P1-2** 是降级与硬校验
- **短期绕路**:走 memory 中的 human_signoff 路径救 dead-letter

---

**报告完成时间**:2026-06-27
**报告范围**:仅基于 `.ralph/` 下 1 个 loop session (2026-06-27T00-04-19) 的 22 events + 28 recovery envelope + 5 drift findings + 7 tasks + 9 状态文件
**未触达**:plan U5-U8(audit integration / runtime_audit / doc / report skill sync);preset L3 维度 vs 6 维度的最终裁决
**方法**:4 个并行 sub agent(流程还原 / 历史知识库 / 对账分析 / 归因修复)+ 主 Agent 汇总