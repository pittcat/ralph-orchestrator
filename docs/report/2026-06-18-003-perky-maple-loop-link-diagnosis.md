# ce-executor-serial Loop 诊断报告:2026-06-10-003 Step-01 Plan-Gate-Trigger-Gap & Isolated-Scope Noise

> **报告日期**:2026-06-18
> **作者**:Loop & Preset 诊断专家(Ralph 自动报告)
> **Loop ID**:`2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-perky-maple`
> **Preset**:`builtin:ce-executor-serial`(10-hat 拓扑,`execution_mode: isolated`,串行 review)
> **Plan**:`docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md`
> **Worktree**:`/home/chaowen/Dev/agent_tools/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-perky-maple/`
> **最终状态**:**链路 U1 业务闭环**但**fix 后 plan-gate 未推进 + fix→re-review 卡死**,loop 于 2026-06-18T06:49:33Z **用户主动 abort** 终止(PID 3590878 已退出;`loops.json` 仍残留 stale 条目)
> **持续时间**:**2h 6m 58s**(04:42:35 → 06:49:33) / ~7 iterations / **39** 条 events 文件记录(37 业务 + 2 `task.resume` 系统注入)
> **最终 Commit**:`5ded762e fix(event-loop+audit): U1 review-fix F1-F8 闭环`(06:41:25 UTC 落盘;但 `fix.applied` 于 06:15:51 报 `commit_count=0`,**emit 与 commit 时序错位 ~25min**)
> **基线 commit**:`aef7d515 docs(plan): 刷新 event_loop/loop_runner 拆分计划到 v11 baseline 并细化单元`
> **worktree commits**:`32555b75` U1 scaffold + `5ded762e` review-fix F1-F8(两次 commit 均在 worktree 内,未 merge 回 main)
> **增量复核**:2026-06-18 复核 `.worktrees/.../perky-maple/.ralph` 最新产物后更新本报告 §1/§2.2/§5/§7/§9

---

## 1. 结论摘要

本次 `ce-executor-serial` run **U1 scaffold 业务链路完整走通**:coordinator → executor(work.done 213 lines)→ review-coordinator(4 维串行)→ dimension-reviewer(4 维 done)→ review-synthesizer(review.failed 1×)→ fixer(fix.applied 8 项 96 行),所有事件均通过 `isolated_publish_allowed` / `DuplicateWorkDone` / `extra_business_event` / `hat_allowed_values` 四层 policy 校验。**没有发现 ralph 基座 bug**,也没有发现 preset 编排层流程设计 bug。

**真正的问题**集中在 3 个具体可修复点 + 5 个 agent 行为/preset 教学问题:

- **关键异常数量**:**P0 = 0**(无运行中断) / **P1 = 3** / **P2 = 6** / **信息 = 2**
- **P1-1**:`plan-gate.triggers` 缺失 `fix.applied` 和 `review.failed` → fixer.applied 后 plan-gate 未 dispatch,loop 卡在 step-01
- **P1-2**:executor 在 human guidance("Focus on error handling")注入后**反复探针式 emit**(6 轮 × 22+ 种变体),`recovery.jsonl` **135** 条 policy 拒绝把 noise 通道压爆
- **P1-3(增量)**:fix→re-review 被 policy dedup 永久阻断(`review.dimension.ready` key 不含 fix_round) → fix 后无法合法重走 review 序列
- **P2-1**:review-coordinator 重复 4× `review.dimensions.complete`(只有第 1 次有效)
- **P2-2**:dimension-reviewer 重复 2× 同 maintainability 维度 `review.dimension.done`
- **P2-3**:fixer.applied 报 `commit_count=0`(8 项,96 行未 commit)
- **P2-4**:06:19:03 / 06:25:22 review-coordinator 误以为 fix 后应再走完整 review,重发 `review.dimension.ready(correctness)` 被 `duplicate_work_done` 拒(**recovery 共 2 条**,非原报告 1 条)
- **P2-5**:fix.applied 后 review-coordinator 被 dispatch 但**静默无 emit** → 06:26:32 HARD GATE `task.resume`;随后误发第 5 次 `review.dimensions.complete`(06:35:16),review-synthesizer 再次静默 → 06:41:44 HARD GATE(consecutive=2);**fix→re-review 路径在 dedup policy 下不可行**(见 `mem-1781763958-323d`)
- **P2-6**:loop 于 06:49:33 **用户 abort** 终止,非自然 `LOOP_COMPLETE`;`loops.json` PID 3590878 条目未清理
- **信息-1**:`hat_lifecycle::complete` 在 04:48:29 启动 coordinator 时报 "Complete called for unknown or already-closed activation key" WARN
- **信息-2**:`events-hat-review-coordinator-*.jsonl` 0 bytes(serial preset 下 reviewer hat-channel 文件被创建但未写)

**是否历史重复**:**是**(中关联度)。本次观察到的 3 个核心问题**直接命中**已记录的历史坑:

- `memory/review-coordinator-aggregate-timeout-handling.md` + `memory/review-coordinator-isolated-scope-recovery.md`:**`aggregate_timeout` 是 review-synthesizer 专属,executor 写入端被 policy 拒**(本次 04:54 那 6 轮 `skip_reason=aggregate_timeout` 由 executor 试发,policy 正确拒绝,**与历史同构,行为符合预期**)
- `memory/ce-executor-isolated-dispatch-gap.md`:**plan-gate→executor 桥接缺口**(本次是 serial preset 而非 isolated,但 `plan-gate.triggers` 缺 `fix.applied` / `review.failed` 是**完全同根**:fixer 路径不在 plan-gate 触发列表)

**根因(主)**:`presets/en/ce-executor-serial.yml:1624` 的 `plan-gate.triggers` 列表未包含 `fix.applied` 和 `review.failed`。fixer.applied 后 EventBus 不 dispatch plan-gate,`queue.advance` 永不发出,loop 卡在 step-01 闭环而无法推进 step-02。**这是 preset 编排层 1 行可修的 bug**。
**根因(次-1)**:`executor.instructions` 缺 isolated-mode hard-rules 段(允许 publish 的 topic 白名单 / `aggregate_timeout` 单所有权说明),导致 executor 在 human guidance 注入后用 6×22+ 种变体"摸黑"试错,recovery.jsonl 噪声 135 条。
**根因(次-2,增量发现)**:即使补上 plan-gate triggers,**fix→re-review 仍被 policy dedup 阻断**——`review.dimension.ready` dedup key `{plan}::{step}::{task}::{dim}` 在 loop 生命周期内永久有效,fix_round≥1 时 review-coordinator 无法重发 readiness(06:19/06:25 CLI 拒 + 06:26 HARD GATE)。需 policy 层 fix_round-aware key 或 fix.applied 时 prune dedup set(见 `.ralph/agent/memories.md` mem-1781763958-323d)。

---

## 2. 执行链路对比图

### 2.1 Preset 预期事件流(`ce-executor-serial`)

`presets/en/ce-executor-serial.yml` 定义的 10 hat 串行 review 链路:

| Hat | Triggers | Publishes | 串行 review 角色 |
|---|---|---|---|
| `coordinator` | `work.start` | `work.ready` / `work.failed` | 解析 plan、复杂评估、创建 task |
| `executor` | `work.ready` / `fix.plan.ready` | `work.done` / `work.failed` | TDD 实施、每 U 一个 task |
| `review-coordinator` | `work.done` / `fix.applied` / `review.dimension.done` / `review.dimension.failed` | `review.dimension.ready` × 4 次串行 / `review.dimensions.complete` | 固定 4-dim 序列:correctness → testing → maintainability → requirements |
| `dimension-reviewer` | `review.dimension.ready` | `review.dimension.done` / `review.dimension.failed` | **无 concurrency、无 wave**、单次串行 |
| `review-synthesizer` | `review.dimensions.complete` | `review.passed` / `review.failed` / `review.complete` / `plan.blocked` | 读 4 维 findings + sequence 给出 verdict |
| `fixer` | `review.failed` | `fix.applied` / `fix.exhausted` | ≤3 轮 safe_auto |
| `debug-resolver` | `fix.exhausted` | `fix.plan.ready` / `debug.exhausted` / `plan.blocked` | Root-cause 诊断 |
| `plan-gate` | `review.passed` / `review.complete` / `work.failed` / `fix.exhausted` / `debug.exhausted` | `queue.advance` / `work.ready` / `plan.complete` / `plan.blocked` | 推进 / 终态决策 |
| `shipper` | `plan.complete` / `plan.blocked` / `debug.exhausted` | `REVIEW_COMPLETE` | 最终验证 + commit |
| `reporter` | `REVIEW_COMPLETE` | `report.done` / 可选 `LOOP_COMPLETE` | manager 报告 |

**终态路径**:
```
work.done → review.dimension.ready(c) → …done(c) → …ready(t) → …done(t)
         → …ready(m) → …done(m) → …ready(r) → …done(r)
         → review.dimensions.complete → review.passed
         → queue.advance + work.ready(下一步) / plan.complete
         → REVIEW_COMPLETE → report.done → LOOP_COMPLETE
```

### 2.2 实际事件序列(逐行对账,含异常标注)

来源:`.ralph/events-20260618-044235.jsonl` 36 条 + `.ralph/recovery.jsonl` 134 条拒绝 + `.ralph/diagnostics/logs/ralph-2026-06-18T12-42-34-448-3590828.log` 行为日志

| # | 时刻(UTC) | 预期 | 实际 | 状态 |
|---|---|---|---|---|
| 1 | 04:42:35 | loop-bootstrap → `work.start` | ✅ `work.start`(payload: "Implement dev plan: docs/plans/2026-06-10-003-…") | ✅ |
| 2 | 04:48:07 | coordinator emit `work.ready(step-01, task-1781758078-3ef6, plan_name=2026-06-10-003-…)` | ✅ emit 成功;task `task-1781758078-3ef6` 创建并由 executor 接手 | ✅ |
| 3 | 04:48:29 | hat_lifecycle 启动 coordinator | ⚠️ WARN "Complete called for unknown or already-closed activation key" (`hat_lifecycle.rs` 状态机时序) | ⚠️ 信息-1 |
| 4 | 04:54:47 | Human guidance 注入:"Focus on error handling" | ✅ scratchpad 更新;agent 收到 136 char 注入 | ✅ |
| 5 | 04:54:47 ~ 05:05:45 | (无) | ⚠️ **134 条 cli_emit policy 拒绝**(详见 §2.3 探针序列) | 🔴 P1-2 |
| 6 | 05:13:23 | executor emit `work.done(step-01, 213 lines, plan_name 正确, 1 commit)` | ✅ emit 成功;task closed at 05:12:30 | ✅ |
| 7 | 05:13:44 | isolated mode drop 04:54 那批 6 轮 review.*(executor 越权) | ✅ 全部 drop,scope 警告;`event_loop/mod.rs:6833, 7135` log TTL=300s stale rejection | ✅(policy 工作) |
| 8 | 05:15:37 | review-coordinator emit `review.dimension.ready(correctness)` | ✅ emit 成功 | ✅ |
| 9 | 05:24:49 | dimension-reviewer emit `review.dimension.done(correctness, 6 findings, 0 P0, 2 P2, 4 P3)` | ✅ | ✅ |
| 10 | 05:26:26 | review-coordinator emit `review.dimension.ready(testing)` | ✅ | ✅ |
| 11 | 05:34:29 | dimension-reviewer emit `review.dimension.done(testing, 5 findings, 0 P0, 2 P2, 3 P3)` | ✅ | ✅ |
| 12 | 05:36:39 | review-coordinator emit `review.dimension.ready(maintainability)` | ✅ | ✅ |
| 13 | 05:42:33 | dimension-reviewer emit `review.dimension.done(maintainability, 6 findings, 0 P0, 2 P2, 4 P3)` | ✅ | ✅ |
| 14 | 05:43:33 | (单次 ready 对应一次 done) | ❌ **同 maintainability 维度第二次 done 重复 emit** → `Isolated mode: extra business event dropped — only one per turn`(`event_loop/mod.rs:7265`) | ⚠️ P2-2 |
| 15 | 05:46:08 | review-coordinator emit `review.dimension.ready(requirements)` | ✅ | ✅ |
| 16 | 05:51:51 | dimension-reviewer emit `review.dimension.done(requirements, 8 findings, **4 P0, 1 P1, 1 P2, 2 P3**)` | ✅ | ✅ |
| 17 | 05:53:37 | review-coordinator emit `review.dimensions.complete(1/4)` | ✅ | ✅ |
| 18 | 05:54:08 | (单次 complete) | ⚠️ review-coordinator 第二次重复 emit `review.dimensions.complete` | ⚠️ P2-1 |
| 19 | 05:58:33 | (单次 complete) | ⚠️ 第三次重复 → `extra business event dropped` | ⚠️ P2-1 |
| 20 | 05:59:14 | (单次 complete) | ⚠️ 第四次重复 → `extra business event dropped` | ⚠️ P2-1 |
| 21 | 06:01:51 | (drift 监测) | ⚠️ `coord_join_rate 1/4 = 25%` < 60% threshold → `diagnostics/2026-06-18T12-42-34/drift.jsonl:1` | ⚠️ P2-1 衍生 |
| 22 | 06:04:19 | review-synthesizer 收到 1× `review.dimensions.complete`,emit `review.failed(4 P0 触发, fix_round=0, gated_manual_count=5, safe_auto_count=8)` | ✅ 正确路径(`presets/schemas/ce-executor-serial.yml:157-168` 单所有权校验通过) | ✅ |
| 23 | 06:15:51 | fixer.applied(8 项,96 行,**commit_count=0**) | ✅ emit 成功;`commit_count: "0"` 报字段异常 | ⚠️ P2-3 |
| 24 | 06:19:03 | (fix 后) review-coordinator 再发 review.dimension.ready | ❌ **`duplicate_work_done`** 拒:`duplicate_dimension_ready: review.dimension.ready for key '…::correctness' was already accepted`(recovery.jsonl:134) | ⚠️ P2-4 |
| 25 | 06:25:22 | (fix 后) review-coordinator 再次重发 correctness ready | ❌ **第 2 次 `duplicate_work_done` 拒**(recovery.jsonl:135) | ⚠️ P2-4 |
| 26 | 06:26:32 | review-coordinator 被 fix.applied dispatch 后静默 | ❌ **HARD GATE** `task.resume`(missing_event);agent 上轮未 emit 任何业务事件 | 🔴 **P2-5** |
| 27 | 06:35:16 | (fix 后应 re-review 或 plan-gate 推进) | ⚠️ review-coordinator 误发**第 5 次** `review.dimensions.complete`(无新 dimension.done 前置) | ⚠️ P2-5 |
| 28 | 06:41:25 | (fix commit 应已落盘) | ✅ git `5ded762e` 18 files / +47 -49 落盘(与 fix.applied 报 `commit_count=0` **时序错位 ~25min**) | ⚠️ P2-3 衍生 |
| 29 | 06:41:44 | review-synthesizer 收到第 5 次 complete 后应 emit verdict | ❌ **HARD GATE** `task.resume`(consecutive=2);synthesizer 静默无 emit | 🔴 **P2-5** |
| 30 | **— 缺失 —** | plan-gate 收到 review.failed + fix.applied → emit `queue.advance(step-02)` | ❌ **没有发生**;`plan-gate.triggers` 缺 `fix.applied` 和 `review.failed`(`presets/en/ce-executor-serial.yml:1624`);EventBus 不 dispatch plan-gate | 🔴 **P1-1** |
| 31 | **— 缺失 —** | shipper → REVIEW_COMPLETE | ❌ 没有发生 | 🔴 **P1-1 衍生** |
| 32 | **— 缺失 —** | reporter → report.done / LOOP_COMPLETE | ❌ 没有发生 | 🔴 **P1-1 衍生** |
| 33 | 06:49:33 | (loop 应自然退出 / 兜底退出) | ❌ **用户主动 abort**(`RpcDispatcher Abort: User requested abort`);PID 3590878 SIGTERM→SIGKILL;`loops.json` 仍残留 stale 条目 | 🔴 **P2-6** |

**summary**:33 步中 23 步按设计走,6 步有可恢复重复/误 emit(被 policy 兜住),1 步(30)完全缺失 + 衍生 2 步(31/32)+ 2 步(26/29) hard-gate 卡死 + 1 步(33)用户 abort 终止。

### 2.3 探针序列详情(04:54–05:05,executor 6 轮变体)

executor 在收到 human guidance "Focus on error handling" 后,在 `executor` 通道反复试错 emit 各种变体。**135 条拒绝**按 reason_code 分布:

| reason_code | 次数 | 典型变体 | policy 来源 |
|---|---|---|---|
| `invalid_field_value` | 36 | `work.done plan_name mismatch: expected '2026-06-10-003-…', got "p"` / `got "x"` | `event_policy.rs` plan_name equality |
| `missing_required_field` | 30 | `Missing required field: plan_name` / `Missing required field: task_id` | `event_policy.rs` schema 校验 |
| `semantic_gate_violation` | 25 | `isolated scope violation: hat 'executor' is not allowed to publish topic 'debug.step'; allowed publishes: ["work.done", "work.failed"]` | `workflow_guard.rs` isolated scope |
| `topic_denied` | 24 | `Hat 'executor' is denied from publishing topic 'build.done'` | `topic_deny_rules` |
| `payload_contract_violation` | 18 | `Payload is not valid JSON: expected value at line 1 column 1` / `expected ident at line 1 column 2` | `event_policy.rs` JSON 解析 |
| `duplicate_work_done` | 2 | `duplicate_dimension_ready: review.dimension.ready for key '…::correctness' was already accepted`(06:19 + 06:25) | `event_policy.rs:127, 228, 729` |

**6 轮变体每轮都包含以下"必拒"组合**:
- `topic=build.done`(executor 不允许发)
- `topic=debug.step`(executor 不允许发,isolated scope)
- `topic=task.resume`(executor 不允许发,isolated scope)
- `topic=work.done` with `plan_name=p` / `x`(plan_name 不匹配)
- `topic=review.passed` with `skip_reason=aggregate_timeout`(review-synthesizer 专属,executor 写入端拒,见 `review_step_state.rs:213-222`)
- `topic=review.passed` with `skip_reason=dimensions_complete`(同上)
- `topic=work.done` with `payload="p"`(非 JSON 对象,ident 解析失败)
- `topic=work.ready` with `payload=""`(空 payload,非 JSON)

**policy 行为**:**完全符合预期**,所有拒绝理由清晰、可定位、可修复。**但 prompt/preset 未教 agent 该看哪里**,所以 agent 用 6 轮暴力试探才"摸到"正确 emit 形式(05:13:23 的 `work.done(plan_name=2026-06-10-003-…)`)。

---

## 3. 历史问题上下文(MEMORY + docs/solutions)

| 历史条目 | 关联度 | 本次复现情况 | 链接 |
|---|---|---|---|
| `review-coordinator-aggregate-timeout-handling` | **中** | 04:54 那 6 轮 `skip_reason=aggregate_timeout` 由 executor 试发,**本应是 review-synthesizer 专属**;policy 已正确拒绝(单所有权) | `memory/review-coordinator-aggregate-timeout-handling.md` |
| `review-coordinator-isolated-scope-recovery`(supersedes) | **高** | 04:54 那批 6 轮 executor 越权 emit `review.dimension.done` / `review.passed`,isolated mode 已 drop(scope 警告);同坑但执行端不同 | `memory/review-coordinator-isolated-scope-recovery.md` |
| `ce-executor-isolated-dispatch-gap`(commit 37bd281) | **高** | 本次是 `ce-executor-serial`(不是 isolated preset);但 plan-gate 未在 fixer.applied 后推进,与"plan-gate→executor 桥接缺口"同根:**fixer.applied 不是 plan-gate 的 trigger**;本 preset 的 `plan-gate.triggers` 在 `presets/en/ce-executor-serial.yml:1624` 同样缺 `fix.applied` 和 `review.failed` | `memory/ce-executor-isolated-dispatch-gap.md` + `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` |
| `ralph-emit-hat-channel-routing` | **低** | events 都正确写到 main `events-20260618-044235.jsonl`,hat-channel 文件 `events-hat-review-coordinator-…` 是空文件(0 bytes)→ reviewer hat-channel 路由**未生效**,但不影响主流程 | `memory/ralph-emit-hat-channel-routing.md` |
| `wave-emit-marker-fallback` / `ce-executor-wave-emit-policy` | 无关 | 本 preset 是 serial,无 wave | `memory/wave-emit-marker-fallback.md` |
| `agent-kill-self-parent-ralph` | 无关 | 004 不在本 worktree 范围 | `memory/agent-kill-self-parent-ralph.md` |
| `WAC rollout 003 baseline` | 无关 | WAC 不在本 worktree 范围 | `memory/wac-rollout-2026-06-12-baseline.md` |
| `ce-executor stale activation work.done closure` | **中** | 04:48:29 `hat_lifecycle` 启动 coordinator 时 WARN "Complete called for unknown or already-closed activation key" 与"stale activation"同源,需要关注 hat_lifecycle 状态机 | `memory/ce-executor-stale-activation-work-done-closure.md` |
| `payload contract preset baseline` | **中** | `payload_contract_violation` × 18 反映 preset strict validate 现在 8/8 builtin 通过,但 agent 没读 schema 直接试错(应考虑在 prompt 显式给 schema 摘要) | `memory/payload-contract-preset-baseline.md` |
| `merry-lotus review-chain-stalled diagnosis` | **高** | 2026-06-17 同 plan 003 merry-lotus worktree 报告已经预言本次问题:plan-gate triggers 缺 `fix.applied` / `review.failed` 的同根问题 | `docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md` |
| `noble-peacock review-chain-stalled diagnosis` | **高** | 2026-06-17 noble-peacock worktree 同样在 fixer.applied 后未推进,ralph hat 兜底 `loop.cancel` 终止;本次 perky-maple 同样卡在 step-01,最终由**用户 abort**(06:49:33)终止,非 `LOOP_COMPLETE` | `docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md` |

**关键观察**:**merry-lotus / noble-peacock 报告均已发现 plan-gate 触发桥接缺口是同根 bug**,本次 perky-maple 再次落入同坑。说明**该 P1-1 修复在 3 次 worktree run 中均未落地**,需要在本 worktree 提交前完成。

---

## 4. 证据清单(带文件路径 + 行号 + 事件时间)

| ID | 类别 | 证据 | 位置 |
|---|---|---|---|
| E1 | policy 拒绝 | `topic_denied` × 24(6 轮 × 4 hat 试发 build.done / debug.step) | `.ralph/recovery.jsonl:1-3, 16, 27, …` 时间 04:54:47–05:05:45 |
| E2 | policy 拒绝 | `skip_reason=aggregate_timeout` × 6 被拒(allowed=[trivial_step, dimensions_complete]) | `.ralph/recovery.jsonl:8, 17, 26, 35, 44, …` 时间 04:54:47.955–05:05:41.496 |
| E3 | policy 拒绝 | `invalid_field_value plan_name mismatch` × 6(executor 写 `plan_name=p/x`) | `.ralph/recovery.jsonl:4, 7, 12, 13, 18, 19` |
| E4 | policy 拒绝 | `payload_contract_violation` × 18(JSON 非对象/ident 错误) | `.ralph/recovery.jsonl:6, 14, 15, 23, 24, 25, …` |
| E5 | isolated drop | 04:54 6 轮 review.* 在 05:13:44 被 isolated mode drop(TTL 300s 失效) | `crates/ralph-core/src/event_loop/mod.rs:6833, 7135`;log `2026-06-18T05:13:44.366911Z`–`05:13:44.369638Z` |
| E6 | 重复 emit | review-coordinator 在 05:53:37 / 05:54:08 / 05:58:33 / 05:59:14 重复 4× review.dimensions.complete | `.ralph/events-20260618-044235.jsonl` 行 28/30/31/32;log `2026-06-18T06:01:51.636014Z` 起连续 drop |
| E7 | 重复 emit | dimension-reviewer 在 05:42:33 / 05:43:33 重复 2× maintainability done | `.ralph/events-20260618-044235.jsonl` 行 27/28;log `2026-06-18T05:44:10.705881Z` drop |
| E8 | duplicate dedup | 06:19:03 + 06:25:22 review-coordinator 重发 correctness ready 各被 duplicate_work_done 拒(**2 条**) | `.ralph/recovery.jsonl:134-135`;`event_policy.rs:127, 228, 729, 2459-2592` |
| E9 | 缺失步骤 | fix.applied 后**无任何** queue.advance / plan.complete / LOOP_COMPLETE | `.ralph/events-20260618-044235.jsonl` 第 36 行之后无终态事件;最后记录 06:41:44 `task.resume` |
| E9b | hard-gate 卡死 | 06:26:32 review-coordinator + 06:41:44 review-synthesizer HARD GATE `task.resume` | log `2026-06-18T06:26:32.351266Z`, `06:41:44.127610Z`(consecutive=2) |
| E9c | 用户 abort | 06:49:33 `User requested abort` → SIGTERM/SIGKILL PID 3590878 | log `2026-06-18T06:49:33.814048Z`;`ps -p 3590878` 已不存在 |
| E9d | stale loops.json | abort 后 `loops.json` 仍登记 PID 3590878 | `.ralph/loops.json:5` |
| E10 | plan-gate 触发列表 | `plan-gate.triggers` **不包含** `review.failed` 和 `fix.applied` | `presets/en/ce-executor-serial.yml:1624` |
| E11 | 4 P0 来源 | requirements 维度 4 P0 / 1 P1 / 1 P2 / 2 P3(其余 3 维均 0 P0) | `.ralph/events-20260618-044235.jsonl` 行 25;review-synthesizer 总结:行 33 |
| E12 | drift 信号 | `coord_join_rate 1/4 = 25%` < 60% threshold,`review.dimension.done → review.dimensions.complete` 转换率不达标 | `.ralph/diagnostics/2026-06-18T12-42-34/drift.jsonl:1` 时间 06:01:51(与 E6 同一时刻) |
| E13 | commit 时序错位 | fix.applied 06:15:51 报 `commit_count=0`;git `5ded762e` 于 06:41:25 UTC 落盘 | `.ralph/events-20260618-044235.jsonl` 行 36;`git log -1 5ded762e` |
| E13b | dedup 阻断 re-review | mem-1781763958-323d: fix_round≥1 时 review.dimension.ready dedup key 永久有效,无法 reset | `.ralph/agent/memories.md:9-11` |
| E14 | hat-channel 空文件 | reviewer hat-channel jsonl 文件创建但 0 bytes | `ls -la .ralph/agent/events-hat-review-coordinator-…-14.jsonl` = 0 bytes |
| E15 | task 闭环 | task `task-1781758078-3ef6` closed at 05:12:30,work.done 05:13:23 emit | `.ralph/agent/tasks.jsonl:1` `status: closed` |
| E16 | memory 注入 | run 启动时 Memory injection check: enabled=true, inject=Auto, 0 memories loaded | log `2026-06-18T04:42:35.076009Z` |
| E17 | human guidance 注入 | "Keep this in mind" + "Focus on error handling" 在 04:54:47 UTC 注入 | `.ralph/agent/context.md` HUMAN GUIDANCE 段 |
| E18 | scratchpad | 136 chars 在每次 PTY spawn 时注入 | log `2026-06-18T05:13:44.380821Z` 等多次 |
| E19 | hat_lifecycle WARN | "Complete called for unknown or already-closed activation key" | log `2026-06-18T04:48:29.788051Z` |
| E20 | worktree commits | U1 scaffold `32555b75` + review-fix `5ded762e`(18 files,+47/-49) | `git log --oneline main..HEAD` 在 worktree 内 |
| E21 | preset schema | `aggregate_timeout` / `dimensions_complete` 是 `review-synthesizer` 单所有权,policy 已测试覆盖 | `presets/schemas/ce-executor-serial.yml:157-168`;`review_step_state.rs:213-222`;`event_policy.rs:2459-2592` |
| E22 | isolated 1 per turn 警告 | `event_loop/mod.rs:7265` "Isolated mode: extra business event dropped — only one per turn" | `.ralph/diagnostics/logs/ralph-2026-06-18T12-42-34-448-3590828.log:05:44:10.705881Z, 06:01:51.636014-218Z` |
| E23 | fix.applied 字段 | `applied_count: "8", changed_lines: "96", commit_count: "0", failed_count: "0", fix_round: "1"` | `.ralph/events-20260618-044235.jsonl` 行 36 |

---

## 5. 问题归因表

| 优先级 | 问题描述 | 根因分类 | 证据 | 历史关联 |
|---|---|---|---|---|
| **P1** | fixer.applied 后 loop 停滞,plan-gate 未触发,无 queue.advance / LOOP_COMPLETE | **preset 设计**:`plan-gate.triggers` 缺 `fix.applied` 和 `review.failed` | E9, E10;§2.2 步 30–32 | 与 `ce-executor-isolated-dispatch-gap` / merry-lotus / noble-peacock **同根** |
| **P1** | `recovery.jsonl` 135 条 policy 拒绝(6 轮 × 22+ 种变体)压满 noise 通道 | **agent 执行 + preset 教学不充分**:executor 在 04:54 human guidance 后**反复试错 emit**;preset / prompt 没明确 isolated 模式 publish 白名单 | E1–E4(133 条探针期 + 2 条 fix 后 dedup);§2.3 | 同前 |
| **P2** | review-coordinator 重复 5× `review.dimensions.complete`(第 1 次有效,第 5 次在 fix 后误发) | **agent 执行** + **preset 教学**:fix 后误走 complete 捷径而非 dimension.ready 序列 | E6, E12;events 行 38(06:35:16 第 5 次) | 全新 |
| **P2** | dimension-reviewer 重复 2× 同一维度 done | **agent 执行**;**基座**:`extra business event dropped` 兜住 | E7;E22 | 全新 |
| **P2** | fix.applied 报 `commit_count=0` 但 git 已有 `5ded762e`(**emit 与 commit 时序错位 ~25min**) | **agent 执行**:fixer 先 emit 后 commit 或 payload 字段未刷新;**preset**:缺 `fix.applied` require_git_change | E13, E23;git `5ded762e` @ 06:41:25 | 全新 |
| **P2** | fix 后 review-coordinator 重发 readiness 被 duplicate 拒(2 次) | **policy 设计**:dedup key 不含 fix_round,loop 生命周期内永久有效 | E8, E13b | mem-1781763958-323d |
| **P2** | fix.applied 后 review-coordinator / review-synthesizer HARD GATE 卡死 spiral | **preset + policy 复合**:plan-gate 未 dispatch + dedup 阻断 re-review → agent 静默 → hard gate → 误 emit complete → synthesizer 静默 | E9b;events 行 37–39 | 全新 |
| **P2** | loop 用户 abort 终止,`loops.json` stale | **运维**:用户 06:49:33 手动 abort;未跑 `ralph loops clean` | E9c, E9d | 同 noble-peacock(终止方式不同) |
| **P2** | 04:48:29 hat_lifecycle WARN "unknown or already-closed activation key" | **基座**时序;不致命 | E19 | `ce-executor-stale-activation-work-done-closure` |
| **信息** | `events-hat-review-coordinator-*.jsonl` 0 bytes | serial preset hat-channel 未写;不影响主流程 | E14 | `ralph-emit-hat-channel-routing` |

---

## 6. 修复建议(按优先级)

### P1-1:补 plan-gate triggers 包含 fix.applied + review.failed

- **目标文件**:
  - `presets/en/ce-executor-serial.yml:1624`
  - `presets/zh/ce-executor-serial-zh.yml`(如存在,需同步)
  - `presets/schemas/ce-executor-serial.yml`(如存在 plan-gate trigger 约束)
- **具体修改**:
  ```yaml
  plan-gate:
    # U1 (2026-06-18-003 perky-maple): 补充 fix.applied / review.failed
    # 触发,确保 fixer 闭环后 plan-gate 立即 dispatch 推进 queue.advance。
    # 历史同根:ce-executor-isolated-dispatch-gap / merry-lotus / noble-peacock。
    triggers: ["review.passed", "review.complete", "work.failed", "loop.cancel", "queue.advance", "fix.exhausted", "debug.exhausted", "fix.applied", "review.failed"]
  ```
- **预期效果**:fixer.applied 后 plan-gate 立即被 dispatch,可 emit `queue.advance(step-02)` 推进 plan,loop 不再卡在 step-01 闭环;同时 `loop.cancel` / 兜底退出路径多 1 个 hook
- **验证命令**:
  ```bash
  cargo nextest run -p ralph-cli --bin ralph -- preset_lint::test_plan_gate_triggers
  cargo nextest run -p ralph-core --test scenarios serial_plan_gate_fix_applied_triggers
  ```

### P1-2:在 prompt / preset instructions 里加 explicit "isolated mode hat scope 硬规则"段

- **目标文件**:`presets/en/ce-executor-serial.yml` 的 `executor.instructions` 头部(约 L528)
- **具体修改**(在 EXECUTOR MODE 段后插入):
  ```text
  ## ISOLATED MODE HARD RULES (ce-executor-serial)
  - You CAN publish: work.done, work.failed ONLY
  - You CANNOT publish: review.*, debug.*, build.*, task.resume, queue.advance, plan.*
  - For "skip_reason=aggregate_timeout" / "skip_reason=dimensions_complete": FORBIDDEN in executor; only review-synthesizer may emit. If you see these in recovered events, those are diagnostic noise — DO NOT retry.
  - For 0 finding review: do NOT call `ralph emit review.passed`; emit work.done and let review-synthesizer synthesize.
  - For plan_name: MUST be the full plan name from the work.ready payload (e.g. "2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan"). Use `plan_name: $WORK_READY_PLAN_NAME` from ORCHESTRATOR CONTEXT block, never abbreviate to "p" / "x" / short.
  ```
- **预期效果**:`recovery.jsonl` 噪声从 135 条降到个位数;executor 一次 emit 成功率从 ~8%(1/12) 提升到 100%
- **验证方法**:下次 run 启动时观察 04:54 时段后是否有重复 `topic_denied` / `semantic_gate_violation` 噪声

### P1-3:fix_round-aware dedup 或 fix.applied 时 prune dedup set(阻断 re-review 的 policy 缺口)

- **目标文件**:`crates/ralph-core/src/event_policy.rs`(U5 `review.dimension.ready` dedup,commit 6c7f3a4e 引入)
- **具体修改**(三选一,见 mem-1781763958-323d):
  1. dedup key 加 `fix_round` 后缀:`{plan}::{step}::{task}::{dim}::{fix_round}`
  2. `fix.applied` 处理时 prune 该 task 的 readiness dedup set
  3. 改为 per-batch dedup(仅阻止同 turn 重复)
- **预期效果**:fix.applied 后 review-coordinator 可合法重发 `review.dimension.ready`,不再 06:19/06:25 duplicate 拒 + 06:26 HARD GATE
- **验证命令**:
  ```bash
  cargo nextest run -p ralph-core -- event_policy::tests::duplicate
  cargo nextest run -p ralph-core --test scenarios serial_fix_applied_rereview
  ```

### P2-1:review.dimensions.complete 用 idempotency key 去重

- **目标文件**:`crates/ralph-core/src/event_policy.rs` `DuplicateWorkDone` 逻辑(参考已有 `duplicate_work_done` 框架)
- **具体修改**:为 `review.dimensions.complete` 加 dedup key = `{plan_name}::{task_id}::{step}::{fix_round}`,重复 emit 走 reject 不写盘,不触发 extra business event dropped 警告
- **预期效果**:减少无意义 IO + 提升 drift 监测准确性(避免 coord_join_rate 因重复 emit 被错算)

### P2-2:fixer 强制 git commit 钩子

- **目标文件**:`crates/ralph-core/src/event_loop/execution_contracts.rs`(参考 `work.done` 的 `require_git_change` 模式)
- **具体修改**:给 `fix.applied` 加 `require_git_change.mode = strict`,commit_count=0 走 reject_with_resume
- **预期效果**:fix 闭环时必须有真实 commit,避免 "fixed but uncommitted" 状态;同时 shipper 不会因 commit_count=0 报 stagger

### P2-3:fixer → review-coordinator 路径在 preset 加 explicit routing

- **目标文件**:`presets/en/ce-executor-serial.yml` `review-coordinator.conditional_must_emit` 段
- **具体修改**:fix 后如果有新 fix_round,应发 `review.dimension.ready(0 P0 维度)`;加 `when: fix_round > 0` 分支;**须与 P1-3 policy 修复配套**,否则仍被 dedup 拒
- **预期效果**:fix→re-review 路径在 preset 层可观测;不再误发裸 `review.dimensions.complete`

### P2-4:`hat_lifecycle::complete` 时序 bug 调查

- **目标文件**:`crates/ralph-core/src/hat_lifecycle.rs`(定位 activation key 状态机)
- **具体修改**:在 "unknown or already-closed" 路径加 debug log,确认是并发启动竞态还是 cleanup 顺序问题
- **预期效果**:消除 04:48:29 的 WARN,提升可观测性;与 `ce-executor-stale-activation-work-done-closure` 历史知识合并

### P3(信息,不阻塞):hat-channel 路由 serial preset 失效

- **目标文件**:`crates/ralph-cli/src/loop_runner/hat_channel.rs` 或 `crates/ralph-core/src/event_loop/hat_channel.rs`
- **具体修改**:serial preset 下 reviewer hat-channel 文件被创建但 0 bytes 是因为 hat-channel 路由只对 wave/isolated 模式生效;需要在 `current-hat-events` 写入路径加 hat_id 解析
- **预期效果**:`events-hat-review-coordinator-…` 不再 0 bytes;但**不影响主事件流**,可后续修

---

## 7. 关键结论

- **ralph 基座**:**没有发现误拒 bug**。policy 层 135 条拒绝全部符合设计;但 **dedup key 不含 fix_round 是 re-review 路径的设计缺口**(P1-3),不是误拒而是**过严**。
- **preset 编排**:**1 个真实 bug**(P1-1 plan-gate triggers 缺 fix.applied/review.failed) + **1 个与 policy 交叉的缺口**(fix→re-review 需 P1-3 配套) + 教学/contract 改进项。
- **agent 执行**:executor 探针噪声占 recovery 133/135;fix 后 review-coordinator 进入 **HARD GATE spiral**(06:26 → 06:35 误 complete → 06:41 synthesizer HARD GATE → 06:49 用户 abort)。
- **loop 状态**:进程已于 **06:49:33 用户 abort** 退出(PID 3590878 不存在);`loops.json` **仍残留 stale 条目**,需 `ralph loops clean`。
- **业务产出**:worktree 内 **2 个 commit** — `32555b75` U1 scaffold + `5ded762e` review-fix F1-F8。U1 代码已落盘,但 **orchestration 未闭环**(无 queue.advance / LOOP_COMPLETE)。

---

## 8. 元信息

- **报告路径**:`docs/report/2026-06-18-003-perky-maple-loop-link-diagnosis.md`
- **worktree 路径**:`/home/chaowen/Dev/agent_tools/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-perky-maple/`
- **数据源**:
  - `.ralph/events-20260618-044235.jsonl`(**39** 条,含 2 `task.resume`)
  - `.ralph/recovery.jsonl`(**135** 条)
  - `.ralph/diagnostics/logs/ralph-2026-06-18T12-42-34-448-3590828.log`
  - `.ralph/diagnostics/2026-06-18T12-42-34/drift.jsonl`
  - `.ralph/loops.json`(PID 3590878 stale,进程已退出)
  - `.ralph/agent/tasks.jsonl`(1 task,closed)
  - `.ralph/agent/memories.md`(mem-1781763958-323d,06:26 后注入)
  - `.ralph/agent/context.md`(含 human guidance)
  - `.ralph/agent/scratchpad.md`(136 chars)
  - worktree `git log`: `32555b75`, `5ded762e`
- **参考报告**:
  - `docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md`
  - `docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md`
  - `docs/report/2026-06-17-ce-executor-isolated-flow-reliability-plan-loop-synthesizer-stall-diagnosis.md`
- **引用源码**:
  - `presets/en/ce-executor-serial.yml:522-748, 1621-1767`(executor + plan-gate 定义)
  - `crates/ralph-core/src/event_loop/mod.rs:6833, 7135, 7265`(isolated drop + extra-business 警告)
  - `crates/ralph-core/src/event_policy.rs:127, 228, 729, 2459-2592`(duplicate dedup + 测试)
  - `crates/ralph-core/src/event_loop/review_step_state.rs:213-222, 305`(`aggregate_timeout` 限制为 review-synthesizer 专属)
  - `crates/ralph-core/src/hat_lifecycle.rs`(WARN 04:48:29 来源)
- **关联 MEMORY**:
  - `memory/review-coordinator-aggregate-timeout-handling.md`
  - `memory/review-coordinator-isolated-scope-recovery.md`
  - `memory/ce-executor-isolated-dispatch-gap.md`
  - `memory/ce-executor-stale-activation-work-done-closure.md`
  - `memory/payload-contract-preset-baseline.md`
  - `memory/ralph-emit-hat-channel-routing.md`

---

## 9. 增量更新记录(2026-06-18 复核 `.ralph` 产物)

本次对照 worktree `.ralph/` 最新产物,相对初版报告修正/补充如下:

| 维度 | 初版报告 | 复核后 |
|---|---|---|
| loop 终态 | in-flight,8h+ 无活动 | **06:49:33 用户 abort** 终止;PID 已退出 |
| 持续时间 | 1h 33m(至 fix.applied) | **2h 7m**(04:42 → 06:49) |
| events 条数 | 36 | **39**(含 2 `task.resume` 系统注入) |
| recovery 条数 | 134 | **135**(+1 duplicate_work_done @ 06:25) |
| commit 状态 | 无 commit / commit_count=0 | **已有 `5ded762e`**(06:41:25),但 fix.applied emit 仍报 0 |
| fix 后行为 | 仅 duplicate 拒 1 次 | **HARD GATE spiral**:06:26 review-coordinator → 06:35 第 5 次 complete → 06:41 review-synthesizer → 06:49 abort |
| 新发现 | — | **P1-3**: dedup key 不含 fix_round,fix→re-review 路径 policy 层阻断(mem-1781763958-323d) |

**建议下一步**(按阻塞优先级):

1. `ralph loops clean`(清理 stale `loops.json`)
2. 修 P1-1 plan-gate triggers + P1-3 dedup policy(两项需配套,否则 fix 后仍卡)
3. 修 P1-2 executor isolated 硬规则(降噪)
4. 在 preset 修好后重新跑 step-01→step-02 推进验证
