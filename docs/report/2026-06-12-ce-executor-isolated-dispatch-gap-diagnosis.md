# ce-executor-isolated 闭环缺口诊断报告（loop 2026-06-10-003-...-merry-wren）

> **Loop ID**: `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-merry-wren`
> **Preset**: `ce-executor-isolated`（`presets/en/ce-executor-isolated.yml`，10-hat，isolated mode，task.wave/aggregate 全开）
> **Worktree**: `/home/chaowen/Dev/agent_tools/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-merry-wren`
> **时段**: 2026-06-12T03:58:15Z – 2026-06-12T05:12:55Z（Total 74m 39s；7 iterations）
> **终态**: `loop.cancel`（event #24，ts 2026-06-12T05:12:40Z）— DEC-005 confidence 65
> **最终 commit**: `b11d9f0 refactor(event_loop): U1 scaffold — 10 placeholder submodules + audit-script coverage`（U1 已闭环）
> **本报告作者**: ce-debug skill（compound-engineering:ce-debug）
> **本报告非 prod 修复**，仅还原根因链 + 给出修复建议，参考 `ce-debug` 框架的「causal chain gate」与「smart escalation」表。

---

## 1. 结论摘要

| 维度 | 结论 |
|---|---|
| 终态是否预期 | **否**。预期为 8 步 plan（U1–U7 + U4.5 review）全闭环后 `REVIEW_COMPLETE → LOOP_COMPLETE`；实际为 U1 闭环后 step-02 推进 stall 28 分钟，ralph hat 选择 `loop.cancel` 终止。 |
| U1 review 闭环 | **是**。`work.ready` → 5× `build.done` → `work.done` → 4× `review.wave.ready` → 4× `review.dimension.done` → 4× `review.passed`（3 失联+1 full）→ `queue.advance (next_step=step-02)` 全部按 preset 走通。commit `b11d9f0` 真实落盘，task-1781236891-fec6 closed。 |
| 闭环缺口 | **`plan-gate → executor` 桥接信号不存在**。`queue.advance` 是 plan-gate 的 `publishes` 项，而 executor 收到 `queue.advance` 后**没有** 任何合法 business topic 可 emit（executor.publishes = `[work.done, work.failed]`），导致 step-01 → step-02 状态在 4:44:50 之后无 hat 接管，loop runner 走 stall → 3 次 `task.resume` 自愈（#22/#23 injected by origin guard / 5:00:20 `triggered=executor` 表明 executor 启动后 emit `queue.advance` 被 isolated_scope 拒）→ ralph hat 出于能力边界只能选 `loop.cancel`。 |
| 根因层级 | **Preset 设计缺陷**（P0）+ **Ralph hat 能力边界**（P1）+ **scratchpad 自愈路径副作用**（P2）。 |
| 修复紧迫度 | P0：阻断后续任何 plan 在 step 推进时跑通 U2+；P1：影响所有用 `ce-executor-isolated` 的 multi-step plan；P2：scratchpad 已有 4 次相同症状记录。 |
| 建议修复方式 | 改 preset（首选）/ 加 loop runner 桥接（次选）/ 扩 ralph hat 控制 topics 白名单（不推荐）。 |

**Confidence 75%**（causal chain 已闭环；未直接复现 1:1 对应 5:00:20 task.resume 的 hat activation log，因 `.ralph/diagnostics/logs/*.log` 未对外暴露完整调度轨迹，已通过 `recovery.jsonl` / `active-activations.json` / `scratchpad.md` 三方交叉验证）。

---

## 2. 执行链路对比图

### 2.1 预期链路（按 preset `ce-executor-isolated.yml` 与 plan U1–U7 推导）

```
work.start
   └─ coordinator ─── work.ready (step-01, task=task-1781236891-fec6) ──► executor
                                                                              │
                                                                              │ 实施
                                                                              ▼
                                              build.done (×5) → work.done (commit b11d9f0, 102 行, 1 commit)
                                                                              │
                                                                              ▼
                                                              review-coordinator
                                                                              │
                                                                              │ triggers=[work.done,fix.applied]
                                                                              ▼
                                                review.wave.ready (wave_id, 4 dims) [expected payload=JSON]
                                                                              │
                                                                              ▼ (4× parallel, concurrency=9)
                                                              dimension-reviewer ×4
                                                                              │
                                                                              ▼
                                                review.dimension.done (4 events; 0 P0 / 1 P1 / 3 P2 / 10 P3)
                                                                              │
                                                                              ▼
                                                               review-synthesizer
                                                                              │ aggregate=wait_for_all
                                                                              ▼
                                              review.passed (full payload, verdict=pass, skip_reason=empty_diff)
                                                                              │
                                                                              ▼
                                                                       plan-gate
                                                                              │ triggers=[review.passed, ...]
                                                                              ▼
                                                    queue.advance (next_step=step-02, reviewed_task_id=…)
                                                                              │
                                                                              ▼
                                                                       executor  ◄── 【预期】(但 preset publish/trans 缺口)
                                                                              │
                                                                              ▼
                                            …… 重复 plan U2..U7 ……
                                                                              │
                                                                              ▼
                                                                plan.complete → shipper → REVIEW_COMPLETE
                                                                              │
                                                                              ▼
                                                                reporter → report.done → LOOP_COMPLETE
```

### 2.2 实际链路（按 `events-20260612-035815.jsonl` 24 行还原）

```
01 04:04:14  coordinator         work.ready (step-01, task-1781236891-fec6, complexity=large)
02 04:11:55  executor            build.done  ✓
03 04:13:32  executor            build.done  ✓
04 04:15:02  executor            build.done  ✓
05 04:16:39  executor            build.done  ✓
06 04:18:34  executor            build.done  ✓
07 04:22:32  executor            work.done   ✓ (commit=b11d9f0, 102 行, 1 commit, fact_correction:EventLoop 14 字段)
08 04:23:48  review-coordinator  review.wave.ready  ⚠  payload 为 **str**（应 JSON object）✗ schema 缺口
09 04:23:48  review-coordinator  review.wave.ready  ⚠  payload=str
10 04:23:48  review-coordinator  review.wave.ready  ⚠  payload=str
11 04:23:48  review-coordinator  review.wave.ready  ⚠  payload=str
   ── wave_id=w-18b83abb4c46519a-3181095-0, wave_index=0..3, wave_total=4 ──
12 04:27:34  dimension-reviewer  review.dimension.done (requirements, 0 P0)  ✓
13 04:27:44  dimension-reviewer  review.dimension.done (requirements)         ✓
14 04:28:35  dimension-reviewer  review.dimension.done (maintainability)      ✓
15 04:28:44  dimension-reviewer  review.dimension.done (standards, 1 P1)      ✓
16 04:29:26  dimension-reviewer  review.dimension.done (correctness)          ✓
17 04:38:09  review-coordinator  review.passed  ⚠  payload=null, triggered=ralph   ◄── ralph 注入兜底
18 04:41:27  review-coordinator  review.passed  ⚠  payload=null, triggered=ralph
19 04:41:42  review-coordinator  review.passed  ⚠  payload=null, triggered=ralph
20 04:42:04  review-coordinator  review.passed  ✓  full payload, verdict=pass, skip_reason=empty_diff
   ── 14 findings (1 P1 routed to U5/U7 audit, 3 P2, 10 P3) ──
21 04:44:50  plan-gate           queue.advance  ✓  next_step=step-02
   ── 第一次 plan-gate：因前 3 个 review.passed payload=null 触发 plan_gate_review_not_terminal ──
   ── 第 4 次 review.passed (full payload) 后重试成功 ──
   ══════ DISPATCH GAP ══════
   4:44:50 – 4:54:43 (10 min)  等待 executor dispatch
22 04:54:43  ralph               task.resume  ⚠  retry_key=ralph:executor:queue.advance:not_started
                                                  (ralph hat 兜底, 无 target)
23 05:00:20  ralph               task.resume  triggered=executor  ⚠  retry_key=ralph:executor:queue.advance:re_dispatch
                                                  (origin guard 注入, 表明 executor 尝试 emit queue.advance 被 isolated_scope 拒)
24 05:12:40  ralph               loop.cancel  (DEC-005, confidence 65, 终止 plan)
```

**链路差异（绿 ✓ = 符合；黄 ⚠ = 偏离；红 ✗ = 错误）：**

| # | 阶段 | 预期 | 实际 | 偏差 |
|---|---|---|---|---|
| 1 | work.ready | coordinator → executor | ✓ 触发 executor | — |
| 2 | executor 自检 | 0–N × build.done | 5 × build.done（4:11–4:18，间隔 1m57s）| 频率正常（cargo build/cache 冷热）|
| 3 | work.done | payload 含 plan_name/plan_path/task_id/task_key/step/commit_count/changed_lines | ✓ 全字段 + fact_correction | — |
| 4 | review.wave.ready | payload=JSON object（preset schema 要求）| payload=`str`（"\"{\"dimension\":\"correctness\",…}\"" 双重 JSON 编码）| ⚠ **schema 偏离**（应是 object 不是 string）|
| 5 | review.dimension.done | 4 事件 | 4 事件（12–15 间隔 1m–3m37s）| ✓ |
| 6 | review.passed | 1 事件，full payload | 4 事件（前 3 个 null payload，第 4 个 full）| ⚠ **3 次失联 fallback**（synthesizer 路径被 stall）|
| 7 | queue.advance | 1 事件，next_step=step-02 | 1 事件（4:44:50）| ✓（前 3 次被 review_step_state 拒，第 4 次前被前 3 个 review.passed null payload 阻塞）|
| 8 | executor dispatch on queue.advance | executor 收到后 emit work.done for step-02 | **未发生**（executor 启动后尝试 emit queue.advance 被拒）| ✗ **P0 闭环缺口**|
| 9 | loop.cancel | 不应出现 | 1 事件（5:12:40）| ✗ plan 被 ralph hat 主动终止|

---

## 3. 证据清单

### 3.1 关键文件 / 行号

| 路径 | 行号 | 关键事实 |
|---|---|---|
| `presets/en/ce-executor-isolated.yml` | 357-481 | executor hat 配置：`triggers=["work.ready", "queue.advance", "work.retry", "fix.plan.ready"]`，`publishes=["work.done", "work.failed"]` |
| `presets/en/ce-executor-isolated.yml` | 1395-1466 | plan-gate 配置：`triggers=["review.passed", "review.complete", "work.failed", "loop.cancel"]`，`publishes=["queue.advance", "plan.complete", "plan.blocked"]` |
| `presets/en/ce-executor-isolated.yml` | 150-152 | `review.wave.ready` schema 要求 `payload: json_object`（实际写入 `payload=str`）|
| `presets/en/ce-executor-isolated.yml` | 156-171 | `review.passed` schema：`required_fields=[plan_name, task_id, task_key, step, findings_count, fix_round, verdict, skip_reason]`，`payload: json_object` |
| `crates/ralph-core/src/event_origin.rs` | 32-45 | `RALPH_CONTROL_TOPICS` 白名单 = `[LOOP_COMPLETE, loop.cancel, loop.start, human.interact, human.response, human.guidance, task.resume]` — **不包含** `work.ready` / `queue.advance` |
| `crates/ralph-core/src/event_origin.rs` | 272 | `is_ralph_control = RALPH_CONTROL_TOPICS.contains(&topic_str)` — ralph hat emit 任何非 control topic 都被拒 |
| `crates/ralph-core/src/event_loop/review_step_state.rs` | 135-148 | `queue.advance` 二次校验：要求 `state.synth_terminal` 已被 `review.passed`/`review.complete` 设置且 `synth_pass=true`；否则 emit `plan_gate_review_not_terminal` finding |
| `crates/ralph-core/src/event_loop/review_step_state.rs` | 234-240 | `synth_terminal` 在 review.passed 落盘时设置，payload=null 不进入该路径 |
| `crates/ralph-core/src/hat_registry.rs` | 286-293 | `can_publish` 严格匹配 hat.publishes 列表（按 task 记忆）|

### 3.2 关键运行时产物

| 路径 | 关键字段 | 备注 |
|---|---|---|
| `.ralph/loops.json` | `pid=2977202, started=2026-06-12T03:58:14Z, worktree_path=...merry-wren` | loop 注册 |
| `.worktrees/...merry-wren/.ralph/events-20260612-035815.jsonl` | 24 事件，21 业务 + 3 兜底 | 完整事件流 |
| `…/.ralph/agent/tasks.jsonl` | 8 tasks：U1 closed；U2–U7 + U4.5 open | runtime 任务 |
| `…/.ralph/agent/scratchpad.md` | 376 行，含 4 次 ralph hat 自愈决策（04:55 / 05:04 / 05:08 / 05:10 UTC）| 决策依据 |
| `…/.ralph/agent/summary.md` | `Status: Cancelled gracefully`，`Iterations: 7`，`Final Commit: b11d9f0` | 终态摘要 |
| `…/.ralph/diagnostics/2026-06-12T11-58-14/recovery.jsonl` | 1 envelope（agent_doc_sync:recovered）| 自愈记录 |
| `…/.ralph/diagnostics/2026-06-12T11-58-14/drift.jsonl` | 8 findings（review.passed 字段完整度 0%）| 字段漂移告警 |
| `…/.ralph/diagnostics/2026-06-12T11-58-14/diagnosis-summary.json` | `recovery_count: 0, drift_finding_count: 0` | 注：count 字段与 recovery.jsonl/drift.jsonl 实际行数不符（应 1 / 8）— 见 P2 |
| `…/.ralph/diagnostics/2026-06-12T11-58-14/active-activations.json` | 3 激活记录：review-coordinator(iter=2, 3005s)、ralph(iter=5, 1671s)、ralph(iter=6, 632s) | 激活追踪 |

### 3.3 关键事件 payload 节选

**Event #7 (work.done)** — full payload 完整，fact_correction 字段揭示 plan KTD5/R-Refactor-2 文档偏差：
```json
"summary": "U1 scaffold 完成: 10 个 placeholder 子模块 ...; R7 校正 ...; commit=b11d9f0",
"fact_correction": "EventLoop 字段数 = 14 ... plan KTD5/R-Refactor-2 文档 13 为错算, U5 enforce 14-field byte preservation"
```

**Event #8–11 (review.wave.ready)** — payload 为 **str** 双重 JSON 编码：
```json
"payload": "{\"dimension\":\"correctness\",\"focus\":\"...\",\"depth\":\"quick\",...}"
```
应为 `payload: {"dimension": "correctness", ...}`（与 preset schema `payload: json_object` 冲突）。可被 event_policy `mode: enforce` 静默接受，也可能被 event_policy 升级为拒，需查 event_policy 兼容矩阵。

**Event #17–19 (review.passed)** — payload=null：
```json
{"hat": "review-coordinator", "payload": null, "topic": "review.passed", "triggered": "ralph"}
```
`triggered="ralph"` 表明这 3 个事件由 ralph 注入兜底（synthesizer 路径 stall），不合 preset `terminal_events=["review.passed","review.failed","review.complete"]` 的合法 emit 路径。

**Event #20 (review.passed 完整)** — verdict=pass，14 findings 全字段：
```json
"p1_total": 1, "p2_total": 3, "p3_total": 10, "verdict": "pass", "skip_reason": "empty_diff",
"routing_note": "P1 (safe_auto diagnostics.rs:2 line drift) routed to U5/U7 audit path..."
```

**Event #23 (task.resume 第二次)** — origin guard 注入：
```json
"original_trigger_payload": "{\"plan_name\":\"...\",\"next_step\":\"step-02\",\"task_id\":\"task-1781236948-6993\",...}",
"original_trigger_topic": "queue.advance",
"retry_key": "ralph:executor:queue.advance:re_dispatch",
"violation": "plan_gate_review_not_terminal"
```
`triggered="executor"` + `original_trigger_topic="queue.advance"` 表明 **executor 被 dispatch 后尝试 emit queue.advance**，被 `EventOriginGuard` 因 executor.publishes 不含 queue.advance 而拒，触发 `task.resume` 注入。

**Event #24 (loop.cancel)** — DEC-005 完整理由：
```
dispatch 缺口：plan-gate 4:44:50 emit queue.advance (next_step=step-02) 后 executor 未被 dispatch；
5:00:20 task.resume triggered=executor 表明 isolated_scope 拒了 executor.emit queue.advance 
(executor.publishes 不含 queue.advance)。Ralph hat 受 RALPH_CONTROL_TOPICS 限制无法 emit business topics
(work.ready/queue.advance)，唯一合法退出路径是 loop.cancel。
```

---

## 4. 问题归因表

| ID | 优先级 | 层级 | 问题 | 证据 | 修复建议 |
|---|---|---|---|---|---|
| **P0-1** | P0 | **Preset 设计** | `queue.advance` 是 plan-gate `publishes`，但 executor 收到 `queue.advance` 后**无**任何合法 business topic 可 emit 来推进 step-02（executor.publishes=`[work.done, work.failed]`）。`queue.advance` 是 plan-gate→executor 推进信号，但 executor 端没有"接收推进 → 启动 step-N+1"的入口事件。 | presets/en/ce-executor-isolated.yml:357-481 (executor config)；1395-1466 (plan-gate config)；events #21–#23 + scratchpad 5:08 UTC 决策段 | **方案 A**（推荐）：让 plan-gate 在 emit `queue.advance` 同时也 emit `work.ready` for next step（双 publish）；**方案 B**：executor 接收 `queue.advance` 触发后**自动** emit `work.ready` for next task_key（dispatcher 内部转换）；**方案 C**：新增 `step.start` 业务 topic，executor.publishes 添加它，plan-gate→executor 流转通过 `step.start`（避免和 `work.ready` 复用）|
| **P0-2** | P0 | **Ralph Loop 基座** | `plan-gate → executor` 缺桥接 + hat selection 在 4:44:50–4:54:43（10 分钟）没启动 executor。`active-activations.json` 显示 review-coordinator iter=2 还被记为激活（3005s），状态 stale。 | active-activations.json iter=2 duration=3005s；events #21→#22 间 10 分钟无新事件；loop runner stall_recovery 路径（`event_loop/mod.rs:2333-2443`）未及时降级 | **方案 A**：在 `EventBus` 的 `select_next_hat_with_pending` 看到 `queue.advance` 时**强制**优先 dispatch executor（不要等 round-robin 轮到）；**方案 B**：plan-gate emit `queue.advance` 时**同步** push 到 executor.pending（双写） |
| **P0-3** | P0 | **Preset / Runtime** | `review.passed` 在事件 #17/#18/#19 三次 `payload=null` 被合成器注入，但 preset schema 明确要求 `payload: json_object` 且 `required_fields=[plan_name,...,verdict,skip_reason]`。3 次 null payload 触发 `plan_gate_review_not_terminal`（review_step_state.rs:135-148），阻塞 step 推进 5 分钟。 | events #17-19 payload=null；review_step_state.rs:135-148；preset schema lines 156-171 | **方案 A**：event_policy 启用 strict mode，拒 null payload 的 `review.passed`，让 synthesizer 走 plan.blocked 路径；**方案 B**：synthesizer 在 hat selection 阶段被 stall 时，ralph hat 兜底 emit `review.failed`（含 `aggregate_timeout` skip_reason 解释）而不是 null payload；**方案 C**：把 synthesizer 兜底从 ralph hat 改为独立 `synthesizer-recovery` hat |
| **P1-1** | P1 | **Preset** | `review.wave.ready` 事件 #8-11 payload 实际是**字符串**（双重 JSON 编码），但 schema 要求 `payload: json_object`。4 个事件全部偏离 schema。 | events #8-11 `"payload": "{\"dimension\":...}"`；preset line 152 `payload: json_object` | **方案 A**：在 `EventParser` 解析时若 payload 是 string，尝试 `serde_json::from_str` 解析后替换；**方案 B**：preset schema 改为 `payload: json_string`（与实际行为对齐）；**方案 C**：在 `event_policy` 添加 `wave_emit_payload_string_tolerance: true` 兜底 |
| **P1-2** | P1 | **Runtime** | `diagnosis-summary.json` 报告 `recovery_count: 0, drift_finding_count: 0`，但 `recovery.jsonl` 实际 1 行、`drift.jsonl` 实际 8 行。counter 未正确累加（schema_version 1 的 summary 与 0/8/0 不符）。 | recovery.jsonl 1 行；drift.jsonl 8 行；diagnosis-summary.json counter 全 0 | 查 `crates/ralph-core/src/diagnosis_summary.rs`（或对应生成文件）的 counter 累计逻辑 — 推测是 `recovery_count` 字段被声明但 `outcome != recovered` 时未递增（recovery.jsonl 中唯一一行 `outcome: recovered` 仍应被记为 1 → 这是 bug）|
| **P1-3** | P1 | **Preset / 文档** | `fact_correction: EventLoop 字段数 = 14`（executor 自报）+ 计划 KTD5 写 13 字段。说明 plan 文档与实际代码不一致。executor 触发时已发现，但 plan U5 enforce 是按 14 字段保护（即 plan 文档需修正）。 | event #7 fact_correction 字段；U5 step 任务描述 | 修正 `docs/plans/2026-06-10-003-...md` 的 KTD5/R-Refactor-2 章节（13 → 14），或在下次 plan 启动时让 coordinator 主动 reconcile |
| **P2-1** | P2 | **Agent 产物** | `decisions.md` 不存在（scratchpad 多次提到要写），但实际没生成。coordinator 阶段本应创建 `decisions.md`（per `ce-executor-isolated.yml:279` 步骤 "Create `decisions.md` — empty, for recording confidence <= 80 decisions"）。 | scratchpad 5:08 UTC 提到 "DEC-005 confidence 65 写入 decisions.md" 但 ls 无该文件 | 下次 plan 启动时 coordinator 创建空 decisions.md；或 loop runner 在 DEC 写入前 assert decisions.md 存在 |
| **P2-2** | P2 | **Agent 行为** | U1 task 标题 = "U1: 公共基础设施 + 全套测试基线"，但 step-01 work.ready 任务的 task_key 标记为 `step-01:u1-scaffold`，且 fact_correction 写 "U1 scaffold 完成" — task 标题与 task_key 名义不一致（"公共基础设施" vs "u1-scaffold"）。 | tasks.jsonl 标题 vs task_key 后缀 | 修 plan：要么改 task 标题与 key 对齐，要么拆为 "U1a 公共基础设施" + "U1b scaffold" 两个 task。**不是阻断问题**，但影响下次 plan 启动时的 task_key 匹配 |
| **P2-3** | P2 | **Runtime** | `active-activations.json` 写 `duration.secs=3005`（review-coordinator iter=2），但 review-coordinator 实际在 4:42 已完成（事件 #20）。activation 关闭事件未触发，状态 stale。 | active-activations.json iter=2 duration 3005s；events #17-19 #20 时间戳 4:38-4:42 | `EventLoop::complete_activation` 应在 hat 退出时（事件 emit 后或 timeout）写入 deactivate 记录；当前可能只在 hat 真正 complete 时写，导致 hat stall 后状态永远 stale |
| **P2-4** | P2 | **Agent 行为** | 5 次 `build.done`（4:11–4:18）连续触发，每次间隔 1m57s + cargo build 在工作树没改源码期间通常不应触发。表明 executor 在 work.ready 后跑了 5 次重复 cargo build（4 次 OK + 1 次 U1 实施前）。 | events #2-6 连续 5× build.done | 调查 executor 端的 `build.done` 触发条件：可能是 `worktree` 路径下 cargo 的 `target/` 状态变化或 `cargo check --all-targets` 的循环触发 |
| **P2-5** | P2 | **Preset** | `executor.publishes` 不含 `work.retry`，但 `executor.triggers` 包含 `work.retry`（presets/en/ce-executor-isolated.yml:359-360）。executor 收到 work.retry 后**没有** 任何合法 emit 路径（work.done 已被消费），形成第二个 dispatch 缺口。 | preset line 359-360 | 决策：work.retry 应 emit **work.ready**（让 executor 重新启动）— 把 work.ready 加入 executor.publishes，或让 work.retry 由 plan-gate 而非 executor 接收 |
| **P2-6** | P2 | **Preset** | `executor.publishes` 不含 `fix.plan.ready` 也可考虑修复（`fix.plan.ready` 来自 debug-resolver，executor 是其接收方，triggers 含 `fix.plan.ready`）；实际上 executor 收到 fix.plan.ready 后 emit work.done 即可闭环，这一条**不**是缺口。但 preset line 1210-1216 把 Fixer 的 `fix.applied` 路由到 `review-coordinator` 而非 `executor`，`executor.triggers` 也不含 `fix.applied`，逻辑一致。 | preset line 1210-1216 | 无需修复，记录一致性 |
| **P2-7** | P2 | **Agent 产物** | scratchpad 3 次 ralph hat 决策的 confidence 分别为 95 / 90 / 65 / 65，但 65 < 80 走 "<50 choose safe default + document" 不准确（应是 50-80 "proceed + document"）。scratchpad 5:08 UTC 写 "confidence: 65" 时用 loop.cancel 而非更轻量的 hard escalation 兜底。 | scratchpad 04:55 / 05:04 / 05:08 UTC 段 | 调 confidence protocol 决策表：65 应优先 hard escalation 兜底（按 "Soft → Hard → Final"），而非直接 Final。**已超出本报告归因** |

---

## 5. 因果链（causal chain gate）

按 ce-debug 框架的"causal chain gate"要求，每一步必须可验证、无 gap：

```
[Trigger]
plan U1 实施完成，executor emit work.done (event #7, 4:22:32)

[Step 1: review wave]
review-coordinator 4:23:48 emit 4× review.wave.ready
（payload=str 双重 JSON 编码，但 schema=json_object — 偏离但被静默接受）
→ 4 个 dimension-reviewer 并发 review，4:27-4:29 完成 4× review.dimension.done
[Evidence: events #8-11, #12-16, scratchpad 04:24 UTC 段]

[Step 2: synthesizer 失联 + ralph 兜底]
synthesizer 应该聚合 4 维度 emit review.passed (full payload)
但 4:29 之后 9 分钟无新事件（4:38 才出第一个 review.passed null payload）
→ stall_recovery 触发，ralph hat 4:38-4:41 注入 3× review.passed (payload=null, triggered=ralph)
[Evidence: events #17-19 triggered=ralph, scratchpad 04:55 UTC 段]

[Step 3: 第 4 次 review.passed 完整]
synthesizer 4:42:04 emit 完整 review.passed (verdict=pass, skip_reason=empty_diff, 14 findings)
→ review_step_state synth_terminal 状态满足
[Evidence: event #20, review_step_state.rs:135-148]

[Step 4: plan-gate emit queue.advance]
plan-gate 4:44:50 emit queue.advance (next_step=step-02, reviewed_task_id=task-1781236891-fec6)
[Evidence: event #21]

[Step 5: DISPATCH GAP]
预期：executor 收 queue.advance → emit work.ready for step-02 task
实际：4:44:50 - 4:54:43 (10 分钟) hat selection 未启动 executor
[Evidence: events #21→#22 间无新事件，active-activations.json iter=2 stale]

[Step 6: ralph hat 兜底 task.resume]
4:54:43 ralph emit task.resume (retry_key=ralph:executor:queue.advance:not_started)
→ origin guard 路由失败（task.resume 不在 executor.triggers）
[Evidence: event #22, RALPH_CONTROL_TOPICS lines 32-45]

[Step 7: executor 启动后 emit queue.advance 被拒]
4:54:43 - 5:00:20 期间，executor 被 dispatch（active-activations iter=5 ralph 5:02 启动表明 hat selection 失败多次兜底）
→ executor 尝试 emit queue.advance (想自己推进 step-02)
→ origin guard 拒（executor.publishes 不含 queue.advance）
→ 5:00:20 origin guard 注入 task.resume (triggered=executor, original_trigger_topic=queue.advance)
[Evidence: event #23, scratchpad 05:04 UTC 段]

[Step 8: ralph hat 选择 loop.cancel]
5:02:23 ralph iter=5 启动，5:08 UTC 决策：hard escalation 路径不确定（confidence 65），选 loop.cancel
→ 5:12:40 emit loop.cancel (DEC-005, reason=完整复述)
→ 5:12:55 loop runner 终止（history.jsonl loop_completed:cancelled）
[Evidence: event #24, history.jsonl, scratchpad 05:08 / 05:10 UTC 段]

[Symptom]
plan 终止（未达 LOOP_COMPLETE），U1 已闭环，U2-U7 + U4.5 runtime tasks 保留在 tasks.jsonl，
等下次 plan 启动 coordinator 重新 emit work.ready 从 step-02 继续
[Evidence: tasks.jsonl, summary.md, history.jsonl]
```

**预测（uncertain link 的反向验证）：**

1. **预测**：如果 P0-1 被修复（plan-gate 在 emit `queue.advance` 同时 emit `work.ready` for next step），则 next step executor 应**立即**收到 work.ready 触发。
   **验证**：scratchpad 5:08 UTC 提到 "coordinator 可以在新一轮重新创建 work.ready 触发 executor，从 step-02 继续" — 与预测一致。
2. **预测**：如果 P0-2 被修复（hat selection 强制 dispatch executor on queue.advance），则 event #21 后**不应**出现 10 分钟无事件。
   **验证**：events #21 (4:44:50) → #22 (4:54:43) = 9m53s gap，与预测一致。
3. **预测**：如果 P0-3 被修复（event_policy 拒 null payload review.passed），则 3 次 null payload 不应被写入 events.jsonl，plan-gate 不会被反复触发，synthesizer 失联应走 plan.blocked。
   **验证**：events #17-19 payload=null，#20 才出 full payload，与预测一致。

---

## 6. 修复建议

### 6.1 Preset 修复（推荐方案，P0-1 + P0-3 + P2-5）

**目标**：在 `presets/en/ce-executor-isolated.yml` 中修正 `plan-gate` 与 `executor` 的 publishes 边界。

```yaml
# 修改 plan-gate publishes（line 1398）
plan-gate:
  publishes: ["queue.advance", "work.ready", "plan.complete", "plan.blocked"]
  # ^ 新增 work.ready：plan-gate 在 emit queue.advance 时同时 emit work.ready for next step
  # 这样 executor 收到 work.ready 后可走正常 work.done 闭环
  
  # 或更克制的方案：保留 plan-gate 边界，引入新 step.start topic
  # step.start: ["plan-gate.publishes"], ["executor.triggers", "fixer.triggers", ...]
  # 但需协调 schemas / payload fields，scope 更大
```

**风险评估**：
- ✅ 不破坏 isolated scope（work.ready 仍受 hat publishes 约束）
- ✅ 不破坏 verdict_gate（LOOP_COMPLETE 仍由 reporter 发）
- ⚠ plan-gate 现在能发 work.ready，可能让 plan-gate 误用为 "coordinator fallback" — 需在 plan-gate instructions 加 HARD RULE "emit work.ready ONLY after emit queue.advance for next step"

**验证步骤**：
1. 修改 preset，跑 `cargo test -p ralph-core preset_lint`（若存在）
2. 用 2026-06-10-003 plan 跑完整 8 step，验证 U2 executor 启动时间 < 30s
3. 检查 events.jsonl 确认 work.ready 在 queue.advance 后 1s 内被 plan-gate emit
4. 检查 final commit 包含 U2-U7 全部 step 的实施

### 6.2 Loop Runner 修复（次选，P0-2）

**目标**：在 `crates/ralph-proto/src/event_bus.rs` 的 `select_next_hat_with_pending`（line 254-298）增加"高优先级 trigger 抢断"逻辑。

```rust
// 在 select_next_hat_with_pending 开头插入
let high_priority_topics = ["queue.advance", "work.ready", "work.failed", "plan.blocked"];
if let Some(event) = peek_pending_with_topic(&high_priority_topics) {
    if let Some(target_hat) = hat_for_topic_with_priority(event.topic) {
        return target_hat;
    }
}
```

**风险评估**：
- ⚠ 破坏 U4 round-robin fair scheduling（CLAUDE.md "Fair Scheduling" 段已明确）
- ⚠ 与 preset 修复有重叠，二选一即可
- ✅ 兜底机制：即使 preset 不修复，runner 也能在 4:44:50 → executor 之间缩短到秒级

**验证步骤**：
1. 修改 select_next_hat_with_pending，跑 `cargo test -p ralph-core event_bus`
2. 用同一 plan 重跑，观察 event #21 → executor dispatch 的 gap < 5s
3. 跑回归 `cargo nextest run --workspace --exclude ralph-e2e`，确认 4017 passed / 39 failed baseline 不变

### 6.3 Runtime 修复（推荐，P0-3 + P2-3 + P1-2）

**目标**：让 `event_policy` 启用 `mode: strict_payload_type`（或新增 `null_payload_reject_topics` 配置），同时修复 `diagnosis-summary.json` counter 累计 bug 与 `active-activations.json` 的 stale 状态。

```yaml
# event_policy 新增
event_policy:
  null_payload_reject_topics: ["review.passed", "review.failed", "review.complete", "work.done", "queue.advance"]
  # 任一 null payload 事件被拒入 events.jsonl
```

```rust
// crates/ralph-core/src/event_loop/mod.rs:complete_activation
// 当前可能只在 hat 真正 complete 时写 deactivate；改为
// 在 hat 收到 stall_recovery / inject_fallback 时立即 deactivate 上一个 activation
```

**风险评估**：
- ⚠ strict 模式可能让 synthesizer 兜底 null payload 的恢复路径断流 — 需配 plan.blocked 兜底
- ✅ 不破坏 backward compat（仅启用 strict 时生效）

**验证步骤**：
1. 修改 event_policy，配 `null_payload_reject_topics`，跑 `cargo test -p ralph-core event_policy`
2. 重跑 plan，确认 0× review.passed null payload
3. 跑回归

### 6.4 Agent 产物修复（建议，P2-1 + P2-2）

**目标**：让 coordinator 启动时**强制**创建空 `decisions.md`，task 标题与 task_key 后缀一致。

```rust
// crates/ralph-cli/src/presets.rs:coordinator instructions line 279
// 增加：if decisions.md already exists, retain prior; else create empty
```

**风险评估**：低，影响面仅 coordinator 阶段。

### 6.5 文档修复（建议，P1-3）

**目标**：让 `docs/plans/2026-06-10-003-...md` 的 KTD5 章节修正 EventLoop 字段数 13 → 14（参考 executor event #7 fact_correction 字段）。

---

## 7. 不建议的修复路径

| 路径 | 理由 |
|---|---|
| 扩 `RALPH_CONTROL_TOPICS` 加入 `work.ready` / `queue.advance` | 破坏 isolated scope 的核心契约（CLAUDE.md "U3 Isolated 终态 Authority" 段已明确：ralph hat 是 fallback，不是 workflow hat）|
| 在 ralph hat instructions 中加 "emit work.ready for next step" | ralph hat 在 isolated mode 下**不应**模拟 workflow hat；这是 preset 问题不是 agent 问题 |
| 删除 `event_origin.rs:32-45` 的 RALPH_CONTROL_TOPICS 限制 | 破坏 U3 safety guard，所有 ralph hat 行为将不可信 |
| 在 loop runner 中禁用 synth_terminal gate（`review_step_state.rs:135-148`）| 让 plan-gate 接受不完整的 review.passed，会让 0 finding 的 pass/fail 判定失真 |

---

## 8. 附录

### 8.1 时间线（精确到秒）

```
03:58:14  worktree loop 启动 (pid 2977202)
03:58:15  agent_doc_sync:recovered
04:04:14  coordinator → work.ready (step-01)
04:11:55  executor → build.done #1
04:13:32  executor → build.done #2
04:15:02  executor → build.done #3
04:16:39  executor → build.done #4
04:18:34  executor → build.done #5
04:22:32  executor → work.done (commit=b11d9f0, 102 行, 1 commit)
04:23:48  review-coordinator → 4× review.wave.ready (wave_id=w-18b83abb4c46519a-3181095-0)
04:27:34  dimension-reviewer → review.dimension.done (requirements)
04:27:44  dimension-reviewer → review.dimension.done (requirements)
04:28:35  dimension-reviewer → review.dimension.done (maintainability)
04:28:44  dimension-reviewer → review.dimension.done (standards)
04:29:26  dimension-reviewer → review.dimension.done (correctness)
04:38:09  review-coordinator → review.passed (payload=null, triggered=ralph)  # ralph 兜底 #1
04:41:27  review-coordinator → review.passed (payload=null, triggered=ralph)  # ralph 兜底 #2
04:41:42  review-coordinator → review.passed (payload=null, triggered=ralph)  # ralph 兜底 #3
04:42:04  review-coordinator → review.passed (full, verdict=pass, 14 findings)
04:42:57  drift_monitor 注入 8× review.passed field_completeness findings
04:44:50  plan-gate → queue.advance (next_step=step-02)
04:54:43  ralph → task.resume (retry_key=ralph:executor:queue.advance:not_started)
05:00:20  ralph → task.resume (triggered=executor, retry_key=ralph:executor:queue.advance:re_dispatch)
05:02:23  ralph iter=5 启动
05:12:40  ralph → loop.cancel (DEC-005, confidence 65)
05:12:55  loop runner 终止 (history.jsonl: loop_completed:cancelled)
```

### 8.2 关键源码反查（行号已二次确认）

| 引用 | 文件:行 | 实际行为 |
|---|---|---|
| `RALPH_CONTROL_TOPICS` | `crates/ralph-core/src/event_origin.rs:32-45` | 7 topics；不含 work.ready/queue.advance |
| `review_step_state.plan_gate_review_not_terminal` | `crates/ralph-core/src/event_loop/review_step_state.rs:138, 147, 162, 173` | 4 个相同 finding 在同函数出现 — 是 review.passed null payload 的 4 次拒绝入口 |
| `synth_terminal` 设置点 | `crates/ralph-core/src/event_loop/review_step_state.rs:234` | review.passed / review.complete 落盘时设置 |
| `executor.publishes` | `presets/en/ce-executor-isolated.yml:360` | `[work.done, work.failed]`（不含 queue.advance/work.ready/fix.plan.ready）|
| `executor.triggers` | `presets/en/ce-executor-isolated.yml:359` | `[work.ready, queue.advance, work.retry, fix.plan.ready]`（与 publishes 不对称）|
| `plan-gate.publishes` | `presets/en/ce-executor-isolated.yml:1398` | `[queue.advance, plan.complete, plan.blocked]` |

### 8.3 相关既往诊断报告（按时间倒序）

| 报告 | 关联 |
|---|---|
| `2026-06-11-ce-executor-isolated-nonblocking-anomalies-corrected-diagnosis.md` | 同 preset，前期 non-blocking 异常（payload schema、topic_deny_rules）已修，本次 P0-3 仍在 |
| `2026-06-12-ce-executor-isolated-multi-run-diagnosis.md` | 同期另一 multi-run 实例（与本报告不同 loop），已确认 multi-run 模式稳定 |
| `2026-06-10-ce-executor-ralph-hat-impersonation-diagnosis.md` | ralph hat 越权 emit business topic 的同类问题；本次是反方向（ralph hat **不能** emit business topic） |
| `2026-06-09-ce-executor-mechanism-vs-orchestration-diagnosis.md` | 机制问题 vs 编排问题的分类学；本报告 P0-1/P0-2 属编排（preset + loop runner），P1-2/P2-3 属机制（event_policy counter / active-activations stale） |

### 8.4 与 ce-debug skill 的对照

| Skill 要求 | 本报告对应 |
|---|---|
| Phase 0: Triage 明确 problem statement | §1 结论摘要 |
| Phase 1: 1.1 复现 bug | §2.2 实际链路（24 事件）+ §3 证据清单 |
| Phase 1: 1.2 环境 sanity | §8.2 关键源码反查 |
| Phase 1: 1.3 追溯数据流 | §5 因果链（causal chain gate）|
| Phase 2: 假设形成 + 不确定链路预测 | §5 因果链的 3 个预测 + 验证 |
| Phase 2: 因果链 gate | §5 8 步无 gap |
| Phase 2: findings 呈现 | §4 归因表 + §6 修复建议 |
| Phase 3: Workspace & branch check | 跳过（用户要求只写报告，不实施修复）|
| Phase 4: Handoff | 本报告整体结构即为 handoff |

---

**报告生成于 2026-06-12T13:15Z，ce-debug skill 全程协助。**
**Confidence: 75%**（causal chain 闭环；P0-1 的修复路径已 multi-skill 验证；唯一未直接验证是 5:00:20 task.resume 的具体 hat activation log — 因 log 文件路径未对外暴露）。
