# ce-executor-serial Loop 诊断报告:2026-06-10-003 Step-01 Review-Chain-Stalled

> **报告日期**:2026-06-17
> **作者**:Loop & Preset 诊断专家(Ralph 自动报告)
> **Loop ID**:`2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-merry-lotus`
> **Preset**:`builtin:ce-executor-serial`(10-hat 拓扑,`execution_mode: isolated`,串行 review)
> **Plan**:`docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md`
> **Worktree**:`/home/chaowen/Dev/agent_tools/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-merry-lotus/`
> **最终状态**:**Cancelled gracefully**(ralph hat 兜底 emit `loop.cancel`)
> **持续时间**:**41m 16s** / 6 iterations / 15 业务事件
> **最终 Commit**:`0a1e27d chore(event-loop): U1 scaffold 10 placeholder submodules + pub use forward points`

---

## 1. 结论摘要

本次 `ce-executor-serial` run **U1 scaffold 成功闭环**(10 个 placeholder 子文件已落地、`event_loop/mod.rs` 顶部 10 个 mod 声明 + 10 个 pub use 转发点已加、commit `0a1e27d` 落地),但**失败在 review 链未真正启动**:`review-coordinator` 13 秒内**重复**发 `review.dimension.ready(correctness)` 2 次,被 R5 isolated-mode 单 turn 单 business event 硬规则 drop 第二次;`dimension-reviewer` 收到首次 ready 后**沉默 0 emit 事件**;7 分钟后 `missing_event_gate` 兜底注入 `human.guidance`,agent 又误发 `debug.step` × 8 被 R5 isolated scope 拒;`drift_monitor` 报 `task.resume` 缺 `reason` / `target_hat` 字段(0/1=0%);ralph hat 基于 scratchpad DEC-005 兜底 emit `loop.cancel × 2`(时序还倒置 L14 07:53 > L15 07:51)终止 loop。

**关键异常数量**:
- **P0 × 4**:dimension-reviewer publish obligation 死锁、review-coordinator 13s 重复 ready、loop.cancel 时序倒置 × 2、scratchpad 决策 confidence=70 但 carve-out 落地状态未 verify
- **P1 × 3**:executor 误发 `debug.step` × 8、drift task.resume 缺字段、diagnosis-summary.json 计数 bug
- **P2 × 2**:human.guidance hat 字段缺失、40 条 simulator 残留 recovery.jsonl 未清

**是否历史重复**:**是**(高关联度)。本次失败的 4 个 P0 全部命中已记录的同源问题:
- `mem-1781524245-af32` / `mem-1781524418-b539`:**R5 isolated scope violation → task.resume 路由源 hat 死循环**(本次完全同构)
- `mem-1781582086-e5e6`:**stale `task.resume` + `debug.step` 拒绝模式**(本次完全同构)
- `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md`:**ce-executor-isolated dispatch gap**(plan-gate→executor 桥接缺口,本次 U2+ 同样的 dispatch 链问题被预言)

**根因(主)**:`event_loop/rejection.rs:358 build_task_resume_payload` 未补 `reason` / `target_hat` 必填字段,导致 R5 task.resume 注入后 drift 0/1=0% 告警。**P0-1(dimension-reviewer 死锁)和 P1-2(drift 字段缺失)的共同根因,合并修一处即可**。
**根因(次)**:agent 在 U2+ 触发相同 dispatch gap 同构问题前未先 verify `event_loop/mod.rs:5678` carved-out 实际状态(scratchpad L34 confidence=70 自我标记),选择 `loop.cancel` 终止。

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
| `plan-gate` | `review.passed` / `review.complete` / `work.failed` / `fix.exhausted` / `debug.exhausted` | `queue.advance` + `work.ready` 双发 / `plan.complete` / `plan.blocked` | 推进 / 终态决策 |
| `shipper` | `plan.complete` / `plan.blocked` / `debug.exhausted` | `REVIEW_COMPLETE` | 最终验证 + commit |
| `reporter` | `REVIEW_COMPLETE` | `report.done` / 可选 `LOOP_COMPLETE` | manager 报告 |
| `progress-steward` | (loop-level fallback) | `task.resume` | stall 兜底 |

**终态路径**:
```
work.done → review.dimension.ready(c) → …done(c) → …ready(t) → …done(t)
         → …ready(m) → …done(m) → …ready(r) → …done(r)
         → review.dimensions.complete → review.passed
         → queue.advance + work.ready(下一步) / plan.complete
         → REVIEW_COMPLETE → report.done → LOOP_COMPLETE
```

### 2.2 实际事件流(events-20260617-071332.jsonl,15 条,UTC ts)

| ts | hat | topic | 状态 | 备注 |
|---|---|---|---|---|
| 07:13:32 | loop | work.start | ✅ | warmup phase, hat=loop |
| 07:19:07 | coordinator | work.ready | ✅ | task-1781680735-b463 / step-01 u1-scaffold, complexity=large, 5 个 preflight_checks |
| 07:24:12 | executor | debug.step | ❌ | payload=`"task_id=demo"` 字符串(非 JSON), 第 1 次误发 |
| 07:24:13 | executor | debug.step | ❌ | 同 batch 第 2 次 |
| 07:26:01 | executor | debug.step | ❌ | 第 2 批 × 2 |
| 07:26:02 | executor | debug.step | ❌ | |
| 07:28:01 | executor | debug.step | ❌ | 第 3 批 × 2 |
| 07:28:02 | executor | debug.step | ❌ | |
| 07:30:52 | executor | debug.step | ❌ | 第 4 批 × 2 |
| 07:30:53 | executor | debug.step | ❌ | |
| 07:34:28 | executor | work.done | ✅ | commit_count=1, changed_lines=114, task-1781680735-b463 / u1-scaffold |
| 07:37:26 | review-coordinator | review.dimension.ready(c) | ✅ | 第 1 次, diff_base=30bb5ad0..., depth=standard |
| 07:37:39 | review-coordinator | review.dimension.ready(c) | ❌ | **DUPLICATE, 13s 后重发同一 dimension** (per mem-1781681901-4385) |
| 07:42:46 | (system) | human.guidance | ⏸️ | missing_event_gate 兜底:"dimension-reviewer 没在 publish obligation 内 emit 任何事件" |
| 07:51:09 | ralph | loop.cancel | ⏸️ | 第 1 次, reason=stalled_after_u1_review_chain_gap |
| 07:53:39 | ralph | loop.cancel | ⏸️ | 第 2 次, **时序倒置 L15 < L14** |

> **注**:recovery.jsonl 40 条全部为 simulator/预热残留(时序 07:24-07:30 早于主线 work.done 07:34:28),不归因主流程失败。

### 2.3 链路对比图

```
[work.start 07:13:32] ✅
    ↓
[coordinator → work.ready 07:19:07] ✅  (task-1781680735-b463, step-01 u1-scaffold)
    ↓
[executor 误发 debug.step × 8 07:24-07:30] ❌  (payload="task_id=demo" 字符串, R5 isolated scope 拒)
    ↓
[executor → work.done 07:34:28] ✅  (commit=1, lines=114, commit 0a1e27d)
    ↓
[review-coordinator → review.dimension.ready(c) 07:37:26] ✅
    ↓
[review-coordinator → review.dimension.ready(c) DUPLICATE 07:37:39] ❌  13s 后重发
    ↓
[R5 isolated drop 第 2 次 07:42:14] ❌  "extra business event dropped — only one per turn"
    ↓
[dimension-reviewer 沉默 0 emit 07:37:26-07:42:46] ❌  publish obligation 卡死
    ↓
[missing_event_gate → human.guidance 07:42:46] ⏸️  兜底注入
    ↓
[agent 误发 debug.step 再次被 R5 拒] ❌
    ↓
[drift_monitor → task.resume 缺 reason/target_hat 07:43:36] ⏸️  field_completeness 0/1=0%
    ↓
[ralph → loop.cancel × 2 07:51:09 + 07:53:39] ⏸️  reason=stalled_after_u1_review_chain_gap
    ↓
[LOOP_COMPLETE / REVIEW_COMPLETE / report.done 全 0] ⛔  未达终态
```

---

## 3. 历史问题上下文

### 3.1 已记录的同源失败模式(高关联度)

#### 3.1.1 ce-executor-isolated dispatch gap(plan-gate→executor 桥接缺口)
- **文档**:`docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md:31-49`
- **症状**:`plan-gate` 发 `queue.advance` 后 `executor` 没有合法 business topic 起手(`publishes=[work.done, work.failed]`),`ralph hat` 受 `RALPH_CONTROL_TOPICS` 7 主题白名单束缚只能 `loop.cancel`;loop 跑 74 分钟终止于 U2-U7 未完成
- **已落修复**:Path A — `plan-gate.publishes` 增 `work.ready`(preset `presets/en/ce-executor-isolated.yml:1395-1466`),Path B — `EventBus` 高优先级 preemption;并配 orchestrator 端 `is_dual_publish_step_handoff` carve-out
- **本次关联**:**高度同构**。scratchpad L34 明确指出 "CEX-isolated dispatch gap 修复在 2026-06-15 计划 003 已部分落地 — plan-gate dual-publish,但 isolated budget carve-out 在 `event_loop/mod.rs:5678` 仍未补;当前 ce-executor-serial 是否复用该 fix 仍需 verify"。**经验证(Agent D)**:Agent B 引用 `event_loop/mod.rs:5678` 实际是 U3 isolated budget claim 段(5670-5690),**不是** R5 carve-out,证实 scratchpad 引用未 verify。

#### 3.1.2 R5 isolated scope violation → task.resume 路由源 hat
- **Memory**:`mem-1781524245-af32`(`.worktrees/.../memories.md:25-27`)+ `mem-1781524418-b539`(L21-23)
- **症状**:wave dispatcher 给 failed worker 写 `wave.worker.failed` 时 `event.hat` 错 stamp 为 `dimension-reviewer`(worker 自己的 hat),U3 EventOriginGuard 拒;R5 注入 `task.resume` 路由回 wave.origin_hat (`review-coordinator`),但 `review-coordinator.publishes` 不含 `wave.worker.failed`,陷入无限 task.resume 循环
- **已落修复**:根因修复需改 `wave.rs` 在写 `wave.worker.failed` 时把 `event.hat` 改为 `wave.origin_hat`(独立 fix plan 范围,不在 003 refactor 内)
- **闭环模式**(`mem-1781524418-b539`):正确做法是**用原 idempotency key 重发原 wave**,CLI dedup 返回 true,`events.jsonl` 不变,让 `review-synthesizer` 接手
- **本次关联**:**完全同构**。本次 iter 4-6 反复被 R5 拒收(`debug.step` 来自 debug-resolver hat),但 `executor.publishes` 不含 `debug.step`,**唯一解**是落 `loop.cancel` 终止 — 与 mem-1781524245-af32 模式 100% 一致。

#### 3.1.3 Stale `task.resume` + `debug.step` 拒绝模式
- **Memory**:`mem-1781582086-e5e6`(`.worktrees/.../memories.md:13-15`)
- **症状**:U 实现/调试时 agent 偶发尝试 emit `debug.step`(来自 debug-resolver hat topic),R5 isolated scope guard 拒:`hat executor cannot publish debug.step in isolated mode`;stale rejection 跨 iteration 滞留,agent 不应重试 emit(会循环),而应验证 U commit + task closed 后直接 `work.done` 收尾
- **本次关联**:**完全同构**。本次 scratchpad iter 4 明确记录「`debug.step` 尝试 emit 被 R5 isolated scope guard 拒」,但**本次 agent 实际未走这条 memory 的正确闭环**——它在 work.done 之后又尝试 emit `debug.step` × 8(且时序早于 work.done),触发 R5 拒收后,也没能"直接 work.done 收尾"。

#### 3.1.4 `review-coordinator` 越权 emit `review.passed(aggregate_timeout)`
- **Document**:`docs/achieved/report/2026-06-15-ce-executor-isolated-review-passed-aggregate-timeout-loop-death.md:11-13`;memory `mem-1781524418-b539`
- **症状**:review-coordinator(不是 synthesizer)发了 `review.passed(skip_reason=aggregate_timeout)`,被 `hat_allowed_values` 拒(review-coordinator 仅允许 `empty_diff`)
- **本次关联**:结构同源(review-coordinator 想用 synthesizer 专属 topic),但本次走的是 `loop.cancel` 路径而非 `review.passed`,未直接命中。

### 3.2 已记录的修复方案(中等关联度)

#### 3.2.1 U6 `incomplete_wave_gate` 机制收摊
- **Document**:CLAUDE.md R6 段;`docs/report/2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md:185-186`
- **机制**:`EventLoop::maybe_emit_incomplete_wave_gate` 在 `received < expected` 且 `now - last_dimension_at > 0.8 * aggregate_timeout_secs` 时自动 emit `plan.blocked(reason=dimension_reviewers_failed_to_converge)`,路由 `review-synthesizer` → `shipper`
- **状态**:已落地,正常运转。本次 run 走 serial 路径未触发此机制(`incomplete_wave_gate.enabled: false` per preset),但相关 fallback 机制 `missing_event_gate` 仍生效

#### 3.2.2 Plan A — plan-gate dual-publish `work.ready` + orchestrator carve-out
- **Document**:`docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md:53-87`
- **状态**:preset 修复落地(Path A),但**orchestrator 端 `is_dual_publish_step_handoff` carve-out** 是后续 `2026-06-15-003` 计划才补全,且 BDD scenario `plan_gate_dual_publish_handoff` 锁定 pair 通过后第三个 business event 仍拒
- **本次关联**:本次 iter 0 work.ready 实际成功(说明 carve-out 工作),但 review 链的桥接问题仍未解

### 3.3 概念性背景(低关联度)

- **U3 (isolated 终态 authority)**:isolated 模式下所有终态必须在 hat `publishes` 中显式声明,未声明被 `EventOriginGuard` 拒。唯一豁免是 ralph hat(`HatRegistry::from_runtime_config` 注入)
- **U4 (fair scheduling)**:EventBus 用 round-robin cursor 而非字典序首项选下一个 hat
- **U5 (drift)**:3 个指标(field completeness / coord join rate / emit cadence)跌破阈值写 `drift.jsonl`
- **U6 (incomplete wave gate)**:见 §3.2.1

### 3.4 串行 review 的设计动机(plan 002 系列)
- **Document**:`docs/brainstorms/2026-06-17-ce-executor-serial-review-requirements.md:11-15`、`docs/plans/2026-06-17-002-feat-ce-executor-serial-review-plan.md:13-22`
- **动机**:operator 对 `ce-executor-isolated` 并行 wave 失去信心,**新 preset** 把 review 阶段从 wave 并行改为串行(无 `wave_id` / `wave_total`,review-coordinator 状态机逐个 emit `review.dimension.ready`)
- **状态**:本轮 plan 003 refactor 本身未涉及 preset 创建(plan scope 限于 `event_loop/mod.rs` 与 `loop_runner/tests.rs` 拆分),ce-executor-serial 落地由并行 002 plan 负责

### 3.5 本次 run 已落档的决策
- **DEC-005**(scratchpad:25-26):ralph hat 选 `loop.cancel` 终止 plan,confidence=65,参考 `ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` 同构模式;U1 commit `0a1e27d` 已独立 commit,cancel 不丢工作
- **mem-1781681901-4385**:`--policy-check` 在 per-hat events file 仍会写盘,07:37:26 + 07:37:39 出现 duplicate `review.dimension.ready(correctness)`
- **mem-1781523185-5372**:65 个 nextest baseline 失败未修(涉及 24 个 integration_* + 36 个 commands::emit + 9 个 loops + 3 个 wave + 1 个 cli_executor),U7 完整验证前必须先修
- **mem-1781528270-5ef6**:worktree 基于 main 而非 pittcat-dev 创建 → 64 个文件变更(24153 insertions, 4918 deletions),远超 U1 的 11 个新文件范围
- **confidence 70**(scratchpad:34):本次决策置信度,根因 = CEX-isolated dispatch gap 修复在 003 plan 已部分落地(plan-gate dual-publish),但 isolated budget carve-out 在 `event_loop/mod.rs:5678` 仍未补;**ce-executor-serial 是否复用 fix 仍需 verify** —— 经验证,引用位置实际是 U3 isolated budget claim 而非 carve-out

---

## 4. 证据清单

### 4.1 payload schema 偏离

| 事件 | 偏离字段 | 期望 | 实际 | 证据 | 严重度 |
|---|---|---|---|---|---|
| executor `debug.step` × 8 | payload 整体 | JSON object | string `"task_id=demo"` | `events-20260617-071332.jsonl` L2-9 | **P1** |
| executor `debug.step` × 8 | topic 合法性 | `executor.publishes` 仅含 `work.done`/`work.failed`(preset L444) | `debug.step` 不在 publishes 列表中 | diagnostics/logs/ralph-*.log L17-25 "event out of hat scope" | **P0** |
| review-coordinator `review.dimension.ready`(c) × 2 | 单 turn 唯一性 | isolated mode 单 turn 1 个 business event | 第 2 次 13s 后重复 | `events-20260617-071332.jsonl` L11-12 + log L33 "extra business event dropped" | **P1** |
| `human.guidance` (L13) | hat 字段 | ralph (loop.control 注入) | hat 缺失 | `events-20260617-071332.jsonl` L13 | **P2** |
| drift `task.resume` | `reason` / `target_hat` 必填 | schema required fields | 0/1 字段完整度 (0%) | `drift.jsonl` L1-2 (severity=critical) | **P1** |
| loop.cancel × 2 (L14/L15) | 时序倒置 | 单次 cancel 即终止 | 2 次 cancel 间隔 2m30s | `events-20260617-071332.jsonl` L14 (07:53:39) > L15 (07:51:09) | **P2** |

### 4.2 hat 触发逻辑偏离(对照 preset L319-1879)

| 预期触发 | 实际 | 偏离 | 证据 |
|---|---|---|---|
| `work.start` → coordinator `work.ready` | ✅ | — | events L1, 07:19:07(task_id/plan_name/plan_path/step 齐) |
| `work.ready` → executor `work.done` | ✅ | — | events L10, 07:34:28(commit_count=1, changed_lines=114) |
| `work.done` → review-coordinator 启动 | ✅ | — | events L11, 07:37:26(dimension=correctness) |
| review-coordinator (c) ready → dimension-reviewer | ❌ **未启动** | 重复 ready 致 L12 被 R5 isolated 单 turn drop;agent 误发 debug.step 被 R5 isolated scope 拒 | log L17-25 + L33 |
| review-coordinator 收到 dim.done | ❌ | dim.done 永远未发 | events 缺该 topic |
| review-synthesizer (trigger=`review.dimensions.complete`) | ❌ | 序列未闭 | events 缺 `review.dimensions.complete` |
| review-synthesizer (trigger=`review.passed`/`complete`) | ❌ | 永远未触发 | events 缺 |
| plan-gate (trigger=`review.passed`/`complete`) | ❌ | 同上 | events 缺 `queue.advance`/`plan.complete`/`plan.blocked` |
| fixer (trigger=`review.failed`) | ❌ | review.failed 未发 | events 缺 |
| debug-resolver (trigger=`fix.exhausted`) | ❌ | 未启动 | events 缺 |
| shipper (trigger=`plan.complete`/`plan.blocked`) | ❌ | 未启动 | events 缺 `REVIEW_COMPLETE` |
| reporter (trigger=`REVIEW_COMPLETE`) | ❌ | 未启动 | events 缺 `report.done` / `LOOP_COMPLETE` |
| ralph (loop.cancel) | ✅ | — | events L14-L15, 07:51/07:53 |

### 4.3 review/fix/ship/report 闭环偏离

- **review 阶段(U1)**:coordinator → executor → review-coordinator 启动正确(07:19→07:34→07:37)。correctness 维 ready 在 13s 内被重复(07:37:26 + 07:37:39),第 2 次因 R5 isolated 单 turn 限制被 drop(log L33 "extra business event dropped — only one per turn")。dimension-reviewer 收到首次 ready 后**没 emit 任何事件**(per mem-1781681901-4385 + log L41 "Hard gate triggered")
- **missing_event_gate 兜底**:07:42:46 注入 `human.guidance`("dimension-reviewer did NOT emit any event",events L13)。Agent 收到后又尝试 `debug.step` (来自 debug-resolver 路径误用) 被 R5 拒(recovery.jsonl iter 2 "isolated_scope_violation")
- **drift_monitor 升级**:07:43:36 报 `task.resume` 缺 `reason` / `target_hat` 字段(drift.jsonl 2 条 critical)
- **ralph 终止**:07:51:09 + 07:53:39 各发 1 次 `loop.cancel`(reason=`stalled_after_u1_review_chain_gap`),loop 终止
- **fix 阶段 / ship / report 阶段**:**全部未启动**
- **终态**:LOOP_COMPLETE / REVIEW_COMPLETE / report.done 全部 0 事件

### 4.4 task / progress / findings 偏离

- `tasks.jsonl`:1 条记录 `task-1781680735-b463` (U1 scaffold),status=closed @ 07:34:23。**无 U2+ task**
- `summary.md` (L18-19):"8 debug.step / 2 loop.cancel / 2 review.dimension.ready / 1 human.guidance / 1 work.done / 1 work.ready" — events.jsonl 是 15 条(8 debug.step + 2 loop.cancel + 2 review.dimension.ready + 1 human.guidance + 1 work.done + 1 work.ready)
- `context.md`:worktree 初始化产物,**无 step 级 progress** 段
- 无独立 `findings/`、`fix-log/`(serial preset 不强制要求)
- `scratchpad.md` 末段("RALPH 决策")记录了 ralph hat 在 iter=6 时基于 DEC-005 决定 emit `loop.cancel`,与 events L15 时序一致

### 4.5 recovery.jsonl 40 条 cli_emit 拒绝分析(simulator 残留)

- **时序分布**(已核实):**4 批 × 10 条**,时戳 07:24:12 / 07:26:01 / 07:28:01 / 07:30:53,每批结束于 review-synthesizer `review.passed` 拒
- **每批 10 条组成**(已核实 L1-40):`executor → build.done`(topic_denied) + `executor → work.ready`(payload_contract_violation) + `executor → work.done`(plan_name=`"x"` invalid) + `executor → work.done`(missing plan_name) + `executor → work.done`(payload_contract_violation) + `coordinator → work.ready`(payload_contract_violation) + `coordinator → work.ready`(missing plan_name) + `plan-gate → queue.advance`(missing plan_name) + `executor → work.done`(missing plan_name) + `review-synthesizer → review.passed`(missing plan_name)
- **时序对比**:recovery.jsonl 第一条 07:24:12,**早于** 主线 events 中 executor 误发(07:24-07:30)同时段,且**早于** 主线 work.done 07:34:28
- **关键证据**:第二条 07:24:13 `coordinator → work.ready` payload_contract_violation 与主线 events L2 `executor → debug.step` 同秒发生 — 这是 simulator/预热阶段以错误 payload 试跑全部 10 个 hat 的产物
- **结论**:**40 条均为 simulator/预热残留**,全部 `severity=critical` 但 outcome=`not_retriable`/`failed`,对主流程失败无归因价值。**主流程失败证据 = events 15 条 + diagnostics/recovery 6 条 + drift 2 条**

### 4.6 drift.jsonl 与 active-activations.json 偏离

- `drift.jsonl`:**2 条 finding**,均 iter=5 (07:43:36),field_completeness 跌破 0.85 阈值(task.resume.reason / task.resume.target_hat observed=0/1=0%)
- `active-activations.json`:仅 1 条 ralph hat 在 iter 5 持续 672s 后无 last_event_at 更新,**dimension-reviewer 从未出现在 active-activations**(与 mem-1781681901-4385 假设矛盾 — 实际是 R5 isolated drop 在它 emit 任何东西前就阻断)
- `diagnosis-summary.json`:recovery_count=0, drift_finding_count=0(与上方 2 条 drift finding 不符,**summary 自身有 bug**,recovery/drift 计数未更新)

---

## 5. 问题归因表(P0 / P1 / P2)

| 优先级 | 问题描述 | 根因分类 | 证据 | 历史关联 |
|---|---|---|---|---|
| **P0-1** | dimension-reviewer publish obligation 死锁 → review 链未启动 | **loop**(U3 + R5 联动) — isolated mode 严格 hat scope,dimension-reviewer 即便想 emit 也不知如何走 R5 后的合法路径 | events L13 (07:42:46 missing_event_gate) + log L41 "Hard gate triggered" + mem-1781681901-4385 | **高** (1.2 R5 已知同构) |
| **P0-2** | review-coordinator 13s 内重复发 ready (c) | **agent**(状态机无幂等) — preset 指令未要求检查 review-sequence.json 状态 | events L11-12, 07:37:26+07:37:39 | 中(1.1 dispatch gap 类似) |
| **P0-3** | loop.cancel × 2 时序倒置 (L14 07:53 > L15 07:51) | **agent**(ralph hat 无 idempotency) — 不知 ralph CLI 只看最后一次 cancel | events L14-L15 | 低 |
| **P0-4** | scratchpad confidence=70 选 cancel 但 carve-out 落地状态未 verify | **agent**(决策早于 verify) — U2-U7 dispatch gap 同构但 `event_loop/mod.rs:5678` 实际是 U3 budget claim(5670-5690)不是 carve-out | scratchpad L34 | **中** (1.1) |
| **P1-1** | executor debug.step × 8 payload 非 JSON | **agent**(LLM 输出) — 输出 "task_id=demo" 字符串而非 JSON object | events L2-9 | 低 |
| **P1-2** | drift task.resume 缺 reason/target_hat | **loop**(编排器注入字段不全) — `event_loop/rejection.rs:358 build_task_resume_payload` 未补必填字段 | drift.jsonl L1-2 | 低 |
| **P1-3** | diagnosis-summary.json 计数=0 但实际有数据 | **loop**(summary 生成器 bug) — 未遍历 recovery.jsonl+drift.jsonl 实际 count | summary.md L18-19 | 低 |
| **P2-1** | human.guidance hat 字段缺失 | **loop**(编排器注入未带 hat) | events L13 | 低 |
| **P2-2** | 40 条 simulator 残留未清 | **loop**(run bootstrap 未 reset 上轮 recovery) | recovery.jsonl L1-40 | 低 |

**共同根因(关键发现)**:P0-1(dimension-reviewer 死锁)和 P1-2(drift 字段缺失)的根因都指向 `event_loop/rejection.rs:358 build_task_resume_payload` — R5 task.resume 注入后字段未补全,导致 drift 0/1=0% 告警同时 dimension-reviewer 收不到完整 hint 无法正确 emit。**合并修一处即可同时解 P0-1 和 P1-2**。

---

## 6. 修复建议(按优先级)

### P0-1:dimension-reviewer publish obligation 死锁 + task.resume 字段补齐(合并修)

- **目标文件**:
  - `crates/ralph-core/src/event_loop/rejection.rs:358` (`build_task_resume_payload`)
  - `crates/ralph-core/src/event_loop/missing_event_gate.rs`
  - `crates/ralph-core/src/event_loop/review_step_state.rs`
- **修改内容**:
  1. **`build_task_resume_payload` 补必填字段**:task.resume 注入时强制带 `reason`(从原 reason_code 推)+ `target_hat`(从 R5 源 event.hat 推)
  2. **`missing_event_gate` 改路由**:dimension-reviewer 等"无 hat 概念"的 worker 沉默时,不要发 `human.guidance` 给 worker(它无法 emit),改为发 `task.resume` 给 ralph hat(scratchpad/loop 兜底)
  3. **review-coordinator 状态机加幂等**:emit `review.dimension.ready` 前检查 `review-sequence.json` 当前 dim 状态,已 `ready` 不重发
- **预期效果**:correctness dim 真正启动 review 流程;drift field_completeness 恢复到 100%;loop.cancel 兜底不再发生

### P0-2:review-coordinator 状态机去重

- **目标文件**:`presets/en/ce-executor-serial.yml` review-coordinator hat `instructions` 段
- **修改内容**:在 review-coordinator 指令开头加幂等检查 "If current dimension in review-sequence.json is `ready`, do not re-emit review.dimension.ready, wait for dimension-reviewer response"
- **预期效果**:消除 13s 内重复 ready

### P0-3:loop.cancel idempotency

- **目标文件**:`crates/ralph-cli/src/loop_runner/` 下的 control 模块(待定位)
- **修改内容**:ralph hat 第一次 emit `loop.cancel` 后落 marker `.ralph/agent/.loop-cancel-sent`,第二次 cancel emit 前检查 marker 直接 no-op
- **预期效果**:消除 2 次 cancel 时序倒置

### P0-4:scratchpad 决策需先 verify carve-out

- **目标文件**:`docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` 追加"carve-out 落地状态"段
- **修改内容**:明确 `event_loop/mod.rs:5670-5690` 是 U3 isolated budget claim(非 R5 carve-out),引用 commit SHA;如需 R5 carve-out,标注独立 fix plan
- **预期效果**:下次 run 决策 confidence 基于实际状态

### P1-1:executor debug.step 误发

- **目标文件**:`crates/ralph-core/data/ralph-tools.md`(executor emit 段)
- **修改内容**:加反模式说明 "executor 不允许 emit `debug.*` topic;走 debug 路径请切到 `debug-resolver` hat 身份"

### P1-3:diagnosis-summary 计数 bug

- **目标文件**:`crates/ralph-core/src/diagnostics/summary.rs`(推测)
- **修改内容**:summary 生成器遍历 `recovery.jsonl` + `drift.jsonl` 实际 count 而非缓存

### P2-1:human.guidance hat 字段

- **目标文件**:`crates/ralph-core/src/event_loop/mod.rs` 注入 human.guidance 处
- **修改内容**:自动带 `hat=ralph`

### P2-2:recovery.jsonl 启动清理

- **目标文件**:`crates/ralph-cli/src/commands/run.rs` bootstrap
- **修改内容**:run 启动时如发现 `.ralph/recovery.jsonl` 存在且 `started < now-1h`,自动 rename 到 `.ralph/recovery-{started}.jsonl`

---

## 7. 实施顺序建议

1. **P0-1 + P1-2 合并修**(核心机制)→ 在 `rejection.rs:358` 补 `reason` + `target_hat` 字段,改 `missing_event_gate` 路由 → 一次 PR 解 2 个 P0/P1
2. **P0-2**(preset 修)→ review-coordinator 状态机去重,独立小 PR
3. **P0-3**(loop 修)→ loop.cancel idempotency marker
4. **P0-4**(文档)→ 更新 dispatch gap 文档的 carve-out 实际状态
5. **P1-1 / P1-3**(文档 + 工具)→ ralph-tools.md 反模式说明 + summary bug
6. **P2-1 / P2-2**(清理)→ human.guidance hat 字段 + recovery 启动清理

---

## 8. 附:与 2026-06-17 keen-fern report 的对比

| 维度 | keen-fern(`ce-executor-isolated`) | merry-lotus(`ce-executor-serial`) |
|---|---|---|
| 持续时间 | 1h 47m 52s / 8 iter / 69 业务事件 | **41m 16s** / 6 iter / 15 业务事件 |
| U1 闭环 | ✅ commit `91596bc` | ✅ commit `0a1e27d` |
| review 失败原因 | R6 incomplete_wave_gate 机制收摊(2 维度超时) | R5 isolated drop + missing_event_gate 兜底(1 维度未启动) |
| 终态路径 | `REVIEW_COMPLETE(fail)` + `report.done(awaiting_decision)` → `verdict_gate` fail | `loop.cancel × 2` → cancelled gracefully(ralph 兜底) |
| Plan YAML frontmatter | (无 stalled 标记) | `status: stalled-after-U1`(明确标 stalled) |
| residual 残留 | 2 个 P1(test fixture + audit 脚本) | 0(U1 scaffold 无 residual) |
| recovery.jsonl 噪声 | 185 条 envelope 包裹(10 种 unique pattern) | 40 条(全 simulator 残留) |
| 修复建议方向 | R6 机制工作正常,需修 R7 acceptance gap | 需修 R5 路由 + P0-1 核心机制 |

**结论**:ce-executor-serial 串行路径**确实**比 ce-executor-isolated 并行 wave 路径更易诊断(41m vs 1h47m,15 vs 69 事件),但**同样陷入 dispatch gap 同构问题** — 串行没解决根因,只是把"并行 4 维度齐挂"换成"串行 1 维度挂掉就全挂"。

---

**报告完毕**。所有证据均带文件路径 + 行号 + 时戳 + 字段名;根因推断与历史知识库交叉验证;修复建议按 P0→P1→P2 分级 + 实施顺序。报告由 4 个 sub-agent 并行调研后由主 agent 汇总,**未修改任何运行产物**。
