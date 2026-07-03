# ce-executor-serial Run 诊断报告 — primary-20260703-020135

> 跑于:2026-07-03 / 起始 02:01:35 UTC / 终态 02:34:53 UTC(loop pid 216945 仍存活,被 `plan.blocked` 强行终止)
> preset:`presets/en/ce-executor-serial.yml`(10-hat 串行 review 链)
> plan:`docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`
> 事件源:`.ralph/events-20260703-020135.jsonl`(22 行 = 1 启动 + 21 业务)
> 当前分支:`pittcat-dev` @ `3a50c2ab`

---

## 1. 结论摘要

**整体健康度:U1/U2 单元执行链 100% 合规,review 链完全失败,卡在 review-coordinator → dimension-reviewer 接力点。**

- **关键异常数量**:P0 ×3、P1 ×2、P2 ×2
- **是否涉及历史重复问题**:**是**——本次是 ce-executor-serial preset 在 30 天内第 9 次同根复发(170451 / 032648 / 083222 / 140433 / 175407 / 140149 / 112002 / 151220 + 本次 020135),核心活跃修复 plan 为 `docs/plans/2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md`(U1-U10+U12 active 待执行,仅 U11 已闭合)
- **一句话定性**:**这是机制问题(loop 基座),不是编排问题(preset 设计)**。preset 的 hat 拓扑、topic_deny_rules、6 维 walk 协议、empty-diff fast path 全部正确;真正缺失的是 `event_loop` 的 hat 路由器对 `dimension-reviewer` 的实际激活,以及 phase_authority 引擎对 review 链断裂的兜底

---

## 2. 执行链路对比图

### 实际事件流(events-20260703-020135.jsonl 22 行)

| # | 时间(UTC) | hat | topic | payload 关键字段 | 状态 |
|---|---|---|---|---|---|
| 1 | 02:01:35 | loop-bootstrap | work.start | PROMPT.md 内容 | ✅ 启动 |
| 2 | 02:04:47 | coordinator | work.ready | step-01 / task_id=task-1783044277-05ef / complexity=small | ✅ |
| 3 | 02:08:02 | executor | work.done | step-01 / commit_count=1 / changed_lines=197 | ✅ |
| 4 | 02:09:02 | validator | test.passed | step-01 / tests_run=5 / tests_passed=5 | ✅ |
| 5 | 02:10:34 | coordinator | work.ready | step-02 / task_id=task-1783044630-e2c7 | ✅ 首条 |
| 6 | 02:11:17 | coordinator | work.ready | step-02 重复(裸 emit) | ⚠️ 被 ledger `duplicate_work_done` 拒 |
| 7 | 02:11:59 | coordinator | work.ready | step-02 重复(裸 emit) | ⚠️ 被拒 |
| 8 | 02:13:09 | coordinator | work.ready | step-02 + preflight_checks | ⚠️ 被拒 |
| 9 | 02:13:29 | coordinator | work.ready | step-02 triggered=executor | ⚠️ 被拒 |
| 10 | 02:14:38 | coordinator | work.ready | step-02 triggered=executor | ⚠️ 被拒 |
| 11 | 02:17:04 | executor | work.done | step-02 / commit_count=1 / changed_lines=151 | ✅ |
| 12 | 02:18:01 | validator | test.passed | step-02 / tests_run=20 / tests_passed=20 | ✅ |
| 13 | 02:19:05 | coordinator | review.start | step-02 / task_id=task-1783044630-e2c7 | ⚠️ 第 1 次,被拒 |
| 14 | 02:19:29 | coordinator | review.start | step-02 | ⚠️ 第 2 次,被拒 |
| 15 | 02:22:50 | coordinator | review.start | step-02 triggered=review-coordinator | ✅ 第 3 次接受 |
| 16 | 02:23:13 | coordinator | review.start | step-02 triggered=review-coordinator | ⚠️ 第 4 次 |
| 17 | 02:28:16 | coordinator | plan.blocked | reason="review_failed" triggered=review-coordinator | ❌ 强行终止 |
| 18 | 02:30:30 | coordinator | review.start | step-02 triggered=review-coordinator | ⚠️ 第 5 次 |
| 19 | 02:32:09 | review-coordinator | review.dimension.ready | dimension=goal-alignment triggered=shipper | ❌ 路由错位 |
| 20 | 02:32:39 | review-coordinator | review.dimension.ready | dimension=goal-alignment / focus="test" / intent_summary="test" / changed_files=[] (stub payload) | ❌ |
| 21 | 02:32:53 | review-coordinator | review.dimension.ready | dimension=goal-alignment triggered=shipper | ❌ |
| 22 | 02:34:53 | review-coordinator | review.dimension.ready | dimension=goal-alignment triggered=shipper | ❌ |

### 预期 vs 实际对比

| 阶段 | 预期 | 实际 | 状态 |
|---|---|---|---|
| U1 单元执行(coordinator→executor→validator) | work.ready → work.done → test.passed | 全程通畅 | ✅ |
| U2 单元执行(step-02) | work.ready → work.done → test.passed | 通畅,中间 5 次 work.ready 重复被 dedup gate 拦截 | ✅ |
| Phase 1 → Review 过渡 | coordinator emit 1 次 review.start | 5 次重试,前 2 次被拒,后 3 次接受但触发链路错 | ⚠️ |
| review-coordinator 接管 | walk 6 维序列(goal-alignment → correctness → ...) | 永远卡在 goal-alignment 第 1 维 | ❌ 卡死 |
| dimension-reviewer 激活 | 收到 ready → emit done/failed | 从未激活,4 次 ready 全无对应 done | ❌ 卡死 |
| review-synthesizer 收尾 | 收到 review.dimensions.complete → emit review.complete | 未触发(无 dimensions.complete) | ❌ 未到 |
| plan.complete | gate 放行后 emit | 被 `review_not_terminal` gate 拒 | ❌ 卡死 |
| 兜底路径 | coordinator emit plan.blocked(reason=review_wave_stuck) | coordinator 误发 plan.blocked(reason="review_failed")(语义错误) | ❌ 越权 |

### ledger.jsonl 健康度指标

- iteration 1-12,共 5 次 `rejection_recorded`(全部 `duplicate_work_done`)
- 2 次 `no_progress_turn_observed`(iter 6, iter 11)
- 1 次 `repair_dispatch`(review.complete 越权 emit,被 topic_deny_rules 拒)

### recovery.jsonl 关键记录

- line 6:coordinator 违规 emit `review.complete(verdict=pass, fix_plan_file=null)`,被 `topic_deny_rules` (preset line 617) 拒
- line 8:`plan.blocked(reason="review_wave_stuck: review.start emitted but no dimension workers ran, review.complete cannot be emitted by coordinator, plan.complete blocked by review_not_terminal gate")` —— **recovery stream 自述根因**

---

## 3. 历史问题上下文(关联度标注)

来源:9 份历史诊断报告(`docs/report/2026-06-30` ~ `2026-07-03`)+ 5 份 solutions 根因记录 + 5 份 memory。

### 高关联度(P0-A / P0-B / P0-D / P0-F)
- **P0-A `plan_gate_review_not_terminal` 阻塞**:历史 140149 / 175407 / 112002 多次复现,`plan_gate_should_skip_review_not_terminal` (phase_authority/plan_gate_helper.rs:25) 已实现但 BDD 验证缺失
- **P0-B review 启动段 dimension-reviewer 不响应**:140149 / 112002 / 151220 同根,核心是 `review-coordinator.triggers` 缺 `task.resume` (preset line 1459),`task.resume(target_hat=review-coordinator)` 走 review-coordinator 是死路径
- **P0-D fix-unit 链尾 `plan.complete` 不 emit**:2026-07-02-005 plan U1-U10+U12 全部 active 待执行,**本次失败的核心修复 plan**
- **P0-F `coordinator` 不能 emit `review.complete`**:preset line 513 严格限制,`recovery.jsonl line 6` 显示 coordinator 仍尝试越权 → 被拒

### 中关联度(P0-C / 2.7 task.resume 频次熔断)
- **P0-C review-synthesizer 漂移**:`review.complete` 替代 `review.passed`(KTD-RTC 2026-06-24 已闭环),本次症状不直接命中
- **task.resume 同 reason_code 反复触发无熔断**:event_policy.rs:1185-1217 仅判单次 dedup,4 次 task.resume 风暴可能是 dimension-reviewer 失活的诱因

### 低关联度(P0-E `completion_after_terminal`)
- `LOOP_COMPLETE` 跨 batch 二次风暴,本次未到 LOOP_COMPLETE 阶段,无关

### 历史方案落地状态

| 历史方案 | 状态 | 与本次关联 |
|---|---|---|
| `2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md` (U1-U12) | U11 已闭合,其余 active 待执行 | **本次失败的核心修复 plan** |
| `2026-07-02-006-feat-ce-executor-serial-runtime-phase-authority-plan.md` (U16) | 已实现 plan_gate_helper.rs:25 | `plan_gate_should_skip_review_not_terminal` BDD 验证缺失 |
| `2026-06-17-004-fix-ce-executor-serial-noble-peacock-review-chain-plan.md` (U1-U6) | U1-U3 已落地(c4d1811),U4-U6 follow-up | review 链 clock + context replay 部分止血 |
| `2026-06-23-004-fix-ce-executor-serial-review-terminal-coherence-plan.md` (KTD-RTC) | 已闭环 | review.passed / review.complete 漂移已修 |
| `2026-06-24` KTD-Drift 二次闭环 | 已闭环 | coord_join_mode 4 件套修复 |

---

## 4. 证据清单

### 4.1 运行时证据

| 证据 | 路径 / 事件 ID | 关键值 |
|---|---|---|
| **事件流主文件** | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/events-20260703-020135.jsonl` | 22 行 |
| **任务账本** | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/tasks.jsonl` | 4 行(step-01 ×2 + step-02 ×2,每 step 2 实体) |
| **进度投影** | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/progress.md` | "Completed Steps: step-01, step-02" |
| **agent 故障记忆** | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/memories.md` | 3 条 review 阻塞 memory(mem-1783045640-647e / mem-1783045715-782c / mem-1783046856-c969) |
| **ledger 拒绝日志** | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/ledger.jsonl` | 22 行,5 次 rejection_recorded(duplicate_work_done) |
| **recovery 修复流** | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/recovery.jsonl` | 9 行,line 6 记录 coordinator 越权 review.complete,line 8 自述根因 |
| **loop 注册** | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/loops.json` | pid=216945,started=02:01:35 |
| **hat-channel 路由** | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/current-hat-events` | 指向 review-coordinator 的 hat-channel |

### 4.2 关键事件 ID 与字段

| 事件 ID | 主题 | 关键 payload 字段 | 偏离 |
|---|---|---|---|
| #13-#16 | review.start (4 次重复) | `task_id`, `plan_name`, `task_key`(缺 `step` 字段) | review-synthesizer 即便被激活也无法产出合规 payload |
| #17 | plan.blocked | `reason="review_failed"` triggered=review-coordinator | 语义错误:真正根因是 dimension-reviewer 跑不起来,而非 review 真 failed |
| #19-#22 | review.dimension.ready (4 次) | `dimension=goal-alignment`, `triggered=shipper`(应为 dimension-reviewer) | 路由错位 |
| #20 | review.dimension.ready (stub) | `focus="test"`, `intent_summary="test"`, `changed_files=[]` | stub payload,review-coordinator 在重发循环中触发了空 payload 兜底 |
| (无) | review.dimension.done/failed | 缺失 | 核心 blocker 证据 |

### 4.3 preset 关键定义

| 项 | 值 | 来源 |
|---|---|---|
| coordinator.triggers | [work.start, task.resume, test.passed, review.complete, work.failed] | `presets/en/ce-executor-serial.yml` |
| coordinator.publishes | [work.ready, review.start, plan.complete, plan.blocked, LOOP_COMPLETE] | 同上 |
| review-coordinator.triggers | [review.start, review.dimension.done, review.dimension.failed] | preset line 1459 |
| review-coordinator.publishes | [review.dimension.ready, review.dimensions.complete] | preset line 1460 |
| dimension-reviewer.triggers | [review.dimension.ready] | preset line 1876 |
| dimension-reviewer.publishes | [review.dimension.done, review.dimension.failed] | preset line 1877 |
| topic_deny_rules | {hat_id: dimension-reviewer, topic: review.dimension.ready} (line 506) 等 | preset line 504-543 |

---

## 5. 问题归因表(P0 / P1 / P2)

| 优先级 | 问题描述 | 根因分类 | 证据 | 历史关联 |
|--------|----------|----------|------|----------|
| **P0-1** | **`dimension-reviewer` 从未被激活,review 链断在第一步 goal-alignment** | **loop 基座机制问题**(hat 调度) | 事件 #19-#22:4 次 `review.dimension.ready` 全部 `triggered: shipper`(应为 dimension-reviewer),且无任何 `review.dimension.done/failed` 发出;preset line 1876 明确声明 `dimension-reviewer.triggers=["review.dimension.ready"]` | 是——mem-1783046856-c969 "no dimension workers run" |
| **P0-2** | **`current-hat-events` hat-channel 路由错位指向 review-coordinator,dimension-reviewer 的 hat-channel 永远收不到 ready 事件** | **loop 基座机制问题**(isolated mode channel routing) | `.ralph/current-hat-events:1` → `.ralph/agent/events-hat-review-coordinator-primary-20260703-020135-13.jsonl`;loop_runner 写盘后未把 current-hat-events 切到 dimension-reviewer 的 hat-channel | 是——memory `ralph-emit-hat-channel-routing.md` 提示 isolated mode 下 ralph emit 落盘目标是 hat-channel |
| **P0-3** | **coordinator 误发 `plan.blocked(reason="review_failed")`,越权且语义错误** | **agent 执行问题**(coordinator 行为违背预设) | 事件 #17:`hat: coordinator`, `source: coordinator`, `triggered: review-coordinator`, `reason: review_failed`;真实根因是 dimension-reviewer 跑不起来,不是 review 真的 failed | 是——mem-1783045715-782c "emitted plan.blocked with reason=review_failed because: review wave didn't run" |
| **P1-1** | **review-coordinator 4 次重复 emit `review.dimension.ready`(goal-alignment 维度),触发 Single-emit guard 但 guard 未生效** | **loop 基座机制问题** + preset 软契约 | 事件 #19-#22:同一 `(plan_name, task_id, step, dimension)` 4 次 ready;preset 明确声明"single-emit guard for review.dimension.ready"(line 1751),但 runtime 未 enforce | 是——140149 / 112002 历史 review 启动段同根 |
| **P1-2** | **`review.start` payload 缺 `step` 字段** | **preset 设计缺陷**(contract 字段不全) | 事件 #13-#16:5 次 review.start payload 仅 `plan_name/task_id/task_key`,无 `step`;schema SSOT (`presets/schemas/ce-executor-serial.yml:121-126`) 也未声明 `step` 为 required,但 review-synthesizer 需要 step 字段参与 plan_end phase 判定 | 中——属于 schema 与 plan 协调器指令不一致 |
| **P2-1** | **task 实体在 tasks.jsonl 重复登记(每 step 2 行同 task_id)** | **loop 基座机制问题**(state projector 重复写入) | `tasks.jsonl:1-4`:step-01 task_id 出现 2 条,step-02 task_id 出现 2 条;不影响流程但违背 U5 SSOT | 低——历史 175407 / 140149 偶发 |
| **P2-2** | **`triggered: shipper` 字段错位出现在 review-coordinator 事件上** | **loop 基座机制问题**(triggered 字段来源 bug) | 事件 #19-#22:`review.dimension.ready.triggered=shipper`,语义应为 dimension-reviewer | 否——未见历史 record |

---

## 6. 修复建议(按优先级)

### P0-1:修复 `event_loop` 的 hat 路由器,让 `dimension-reviewer` 真正被 `review.dimension.ready` 激活

- **目标文件**:
  - `crates/ralph-core/src/event_loop/mod.rs`(HatSelector / hat 路由表)
  - `crates/ralph-core/src/hat_registry.rs`(SSOT 注册)
- **具体修改**:
  1. 验证 `HatSelector::select_for_emit(trigger_topic)` 在收到 `review.dimension.ready` 时是否正确查找 `triggers: ["review.dimension.ready"]` 的 hat —— 应返回 `dimension-reviewer` 而非 `shipper`
  2. 检查 hat 选择器是否读取 `hats.dimension-reviewer.triggers` 字段,可能存在 SSOT 漂移(preset 写了但运行时读 `mechanism.phase_authority` 而非 `hats.*.triggers`)
  3. 重点检查 `triggered` 字段来源 —— 是 hat 路由结果还是 emit 时的 `--source` 透传
- **预期效果**:`review.dimension.ready` 真正激活 `dimension-reviewer`,事件流 19 之后出现 `review.dimension.done(failures=...)`,6 维走完进入 review-synthesizer

### P0-2:phase_authority 引擎补强 missing-dimension-worker 兜底诊断

- **目标文件**:
  - `crates/ralph-core/src/event_loop/phase_authority.rs`
  - `crates/ralph-core/src/event_loop/phase_authority/plan_gate_helper.rs`
- **具体修改**:
  1. review 阶段进入后若 ≥ 540s(`dimension-reviewer.missing_event_grace_secs`)无任何 `review.dimension.done/failed`,自动 emit `review.dimensions.complete` 触发 review-synthesizer 兜底(synthesizer 处理"全 dim failed"路径在 preset line 2223-2226 已写好 → emit `plan.blocked(reason="all_dimensions_failed")`)
  2. 或:超时未触发时主动 `task.resume(target=dimension-reviewer, reason="missing_terminal_emit")` 而不是让 loop 死锁
  3. 在 `plan_gate_should_skip_review_not_terminal` (line 25) 增加 BDD scenario 覆盖 review 链断裂路径
- **预期效果**:dimension-reviewer 失活不再导致死锁,review 链有可恢复出口

### P0-3:coordinator prompt 强化 review 失败语义识别

- **目标文件**:`presets/en/ce-executor-serial.yml`(coordinator instructions,line 972-1010 区域)
- **具体修改**:
  1. 在 `Fix Plan Handling` 段开头加 HARD RULE:"If `review.dimension.ready` 已被 emit 但 ≥ N 分钟无 `review.dimension.done/failed` 跟进,DO NOT 自己 emit `plan.blocked(reason='review_failed')` —— coordinator 不掌握 review 真相。应先等 540s missing-event gate,再 emit `plan.blocked(reason='review_wave_stuck: no dimension workers activated')` 让 shipper 走 reason-based gate"
  2. 删除 coordinator 现有"re-emit review.start 直到成功"的隐性行为(事件 #13-#16 表明 coordinator 至少重试了 3 次)
- **预期效果**:coordinator 不再误判 review 失败,review 链断裂时走 shipper 兜底而非自残

### P1-1:加 `review.start` 的 single-emit 守卫对等物

- **目标文件**:
  - `presets/en/ce-executor-serial.yml`(coordinator instructions line 950-970 区域)
  - `crates/ralph-core/src/event_loop/mod.rs`(completion_after_terminal 守卫,line 463)
- **具体修改**:
  1. 现有守卫覆盖"重复 emit review.start"的检查逻辑需要运行时层 enforce,而非只靠 prompt 描述
  2. `completion_after_terminal` 守卫应加对 `review.start` 的对等 `duplicate_topic_within_session: reject` 配置
- **预期效果**:coordinator 不会重发 review.start,避免 4 次重复触发(事件 #13-#16 浪费 ~10 分钟)

### P1-2:BDD 场景补充 review 链断点测试

- **目标文件**:`crates/ralph-core/tests/scenarios/ce-executor-serial-review-stuck.yml`(新增)
- **具体修改**:构造"review.dimension.ready emit 但 dimension-reviewer 不响应"的 mock scenario,**必须用 `run_workflow_guard_scenario`(真 EventLoop runner 断言 events)**,禁止用 `run_scenario` stub(2026-06-24 P0-2/P0-3 根因),断言:
  1. 540s 后 `task.resume(target=dimension-reviewer)` 触发
  2. 再 540s 后 phase_authority 强制 `review.dimensions.complete` 走 synthesizer 兜底
  3. synthesizer 收到"全 dim failed"后 emit `plan.blocked(reason="all_dimensions_failed")`
  4. shipper 将该 reason 路由到 `REVIEW_COMPLETE(fail)`
- **预期效果**:本次 bug 被永久覆盖,未来回归自动捕获

### P2-1:preset lint 加 "orphan dimension-reviewer" 静态检查

- **目标文件**:`crates/ralph-core/src/preset_lint/`(新增模块 `orphan_review_chain.rs`)
- **具体修改**:检查 `review-coordinator.publishes: [review.dimension.ready]` 时,是否存在 hat `triggers: [review.dimension.ready]` 且 `publishes: [review.dimension.done/failed]` —— dimension-reviewer 必须存在
- **预期效果**:preset 误删 dimension-reviewer 时 preset_lint 立即 fail,防止静默退化

---

## 7. 关键文件路径速查

| 类别 | 路径 |
|---|---|
| preset 文件 | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/en/ce-executor-serial.yml` |
| preset schema(SSOT) | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/schemas/ce-executor-serial.yml` |
| 运行中间产物根 | `/home/chaowen/Dev/agent_tools/ralph-e2e/` |
| 运行时事件 | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/events-20260703-020135.jsonl` |
| 任务账本 | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/tasks.jsonl` |
| 故障记忆 | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/memories.md` |
| 修复流 | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/recovery.jsonl` |
| ledger | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/ledger.jsonl` |
| plan 文件 | `/home/chaowen/Dev/agent_tools/ralph-e2e/docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md` |
| 核心修复 plan | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/plans/2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md` |
| 历史诊断报告 | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/report/`(9 份) |

---

## 8. 最终回答用户的 4 个问题

### Q1.整体执行过程有没有问题?

**有严重问题。** U1/U2 单元执行链正常(step-01/02 完成,test.passed 5/5 + 20/20),但 review 链完全失败。loop 在 review-coordinator → dimension-reviewer 接力点死锁,最终 coordinator 越权 emit `plan.blocked(reason="review_failed")` 强行终止。**loop pid 216945 仍在运行但已卡死**。

### Q2.中间产物是否符合 RALPH 基座机制是否正常的生效?

**部分符合,部分偏离。** 中间产物(`events-*.jsonl` / `tasks.jsonl` / `progress.md` / `ledger.jsonl`)在 U1/U2 阶段完全符合 RALPH 机制(event schema 全合规、task lifecycle 正常、dedup gate 在工作);但 review 链出现 3 类机制偏离:① hat 路由器错把 `review.dimension.ready` 路由给 shipper 而非 dimension-reviewer;② `current-hat-events` hat-channel 路由错位,dimension-reviewer 收不到 ready 事件;③ `triggered` 字段语义错位(应为 dimension-reviewer,实际写 shipper)。**RALPH 基座机制本身在 review 链这一段未生效**,但 U1/U2 段完全正常。

### Q3.编排是否合理,是否正常运行?

**编排设计合理但未正常运行。** preset `ce-executor-serial.yml` 的 hat 拓扑、topic_deny_rules、6 维 walk 协议、empty-diff fast path 全部设计正确 —— Agent A/B/C/D 一致判断 preset 编排无问题。**真正问题在 runtime(event_loop 的 hat 路由器 + isolated mode channel routing),不在 preset 设计**。本次失败的"review.dimension.ready 被发出但没被消费"现象属机制缺陷,30 天 9 次同根复发(`docs/report/` 下 9 份诊断均显示相同症状)。

### Q4.是机制问题还是编排问题?

**这是 100% 的机制(loop 基座)问题,叠加约 30% 的 agent 行为失当,不是编排问题。** 具体分账:

- **Loop 基座机制问题(70% 责任)**:
  1. `event_loop` 的 hat 路由器在收到 `review.dimension.ready` 时错误地命中 shipper(`triggered: shipper`),导致 dimension-reviewer 从未激活(P0-1)
  2. `current-hat-events` hat-channel 路由错位,dimension-reviewer 永远收不到 ready 事件(P0-2)
  3. phase_authority 引擎未能在 dimension-reviewer 失活时主动诊断(应有 missing-event gate 540s 宽限期但未触发)(P0-2 兜底)

- **Agent 行为问题(30% 责任)**:
  - coordinator 在 review 链断裂后未按预设"再等一轮 / emit plan.blocked with reason=review_wave_stuck"路径处理,而是错误地直接 `plan.blocked(reason="review_failed")`,越过了 review-synthesizer 兜底通道(P0-3)

- **Preset 编排本身无问题**:
  - `topic_deny_rules` 设计正确(`dimension-reviewer` 是 `review.dimension.done/failed` 的唯一发布者)
  - `phase_authority` 转换规则正确
  - 6 维序列契约、walk vs pass 决策、empty-diff 快速路径等所有编排逻辑自洽
  - **真正缺失的是 hat 调度器对 `dimension-reviewer` 的实际激活**

---

**总结**:本次 run 健康度 = 单元执行 100% 合规 / review 链 0% 工作。**这是机制问题,不是编排问题**。修复路径清晰:P0-1(hat 路由)+ P0-2(phase_authority 兜底)+ P0-3(coordinator prompt 强化)。核心修复 plan `2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md` 应优先推进落地,并在 `crates/ralph-core/tests/scenarios/` 新增 `review_coordinator_triggers_must_include_task_resume.yml` + `plan_complete_with_phase_authority_enabled.yml` + `ce-executor-serial-review-stuck.yml` 三个 BDD scenario(必须 `run_workflow_guard_scenario` 真 EventLoop runner 断言事件)。