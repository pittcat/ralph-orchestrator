# Ralph Loop 诊断报告 — Review Wave 派发异常

- **报告路径**：`report/2026-06-13-review-wave-no-spawn.md`（主仓库 `pittcat-dev` 分支）
- **诊断人**：Ralph Loop / preset 运行链路诊断专家
- **诊断日期**：2026-06-13
- **诊断目标**：回答用户问题——"TUI 里没看到真的起多个 reviewer 去 review，是 ralph 机制问题还是编排问题？"
- **现场范围**：`pittcat-dev` 分支主仓库 + worktree `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-cheery-hawk`
- **分支**：`pittcat-dev`（主诊断分支） / `ralph/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-cheery-hawk`（被诊断 loop 运行分支）

---

## 结论摘要

**问题已定位：是 preset 编排与 Ralph loop `missing_event_gate` 兜底逻辑的复合问题，不是机制级 bug，也不是 preset schema 错误。**

具体链条：

1. **数据流正确**：`review-coordinator` 在 U1 commit 后，按 preset `ce-executor-isolated.yml:702-703` 的"ONE wave call, wave_total=N"规则，用 `ralph wave emit` 一次性向 `events-20260612-161708.jsonl` 写入了 **7 个** `review.wave.ready` 事件，**共享 wave_id `w-18b864225982d010-55383-0`**、`wave_index 0..6`、`wave_total=7`、同一时间戳——这是合规的 1-wave 7-worker 派发表述。
2. **派发链路畅通**：Ralph loop 的 `process_events_from_jsonl_with_waves` → `detect_all_wave_events_capped` → `enforce_wave_isolated_scope` → `handle_wave_events` → `execute_wave_structured` 整套路径**理论能识别**这 7 个事件为 1 个 wave，target_hat=`dimension-reviewer`（`concurrency=9`），应该 spawn 7 个 worker 并发执行，每个 worker 写自己的 `wave-{wave_id}-{index}.jsonl`，最终各 emit `review.dimension.done`。
3. **TUI 看不出 7 个 worker 的根因**：
   - 现场 7 个 worker 的 events file（`wave-w-18b864225982d010-55383-0-{0..6}.jsonl`）**从未在 worktree `.ralph/` 下出现**——worker 实际上**未被 spawn**。
   - 由此触发 `missing_event_gate`（`recovery.jsonl` 第二次 envelope），`review-coordinator` 被 hard-gate 第二次激活，要求其重发 publish obligation 事件。
   - 第二次激活时 `RALPH_CURRENT_HAT` 又错误地路由到 `executor`（`recovery.jsonl` 第三次 envelope：`payload_contract` violation on `work.failed`，`source_hat=[executor, coordinator]`，`target_hat=[plan-gate]`），executor 不在 `publishes` 里有 `review.*` 权，于是**整 loop 以 `payload_contract_violation` 终止**（`exit code=non-zero`，duration=49m46s，iterations=4）。
4. **最终归因**：
   - **P0**：preset 编排缺陷——`review-coordinator` 在 7 个 wave 事件 emit 之后，**没有 await 也没有任何回执**确认 dispatch 已就位，**写完 `events.jsonl` 就返回 0**；agent 视角是"已发布"，但 loop runner 在 iteration 结束前没把 7 个 worker 实际拉起。
   - **P0**：Ralph loop 的 `missing_event_gate`（`hard_gate.rs:422`）的**重置逻辑过激**——一旦 review-coordinator 没在一次 iteration 内同步产出 wave 派生事件，gate 立即把 review-coordinator 重新激活，**但同一 wave 不会重发**（`events.jsonl` 已被读走），导致 review-coordinator 第二次激活时无法"再补发"已经写出去的 7 个 `review.wave.ready`——event loop 期望 review-coordinator 在第二次激活时产出**新的 publish obligation 事件**，但 preset 视角下"已经发过了"，于是 hard gate 持续触发，loop 卡死。
   - **P1**：ralph 当前 hat-routing 错误地把 hard-gate 后的 `task.resume` 路由到 `executor`（`payload-contract-error-*.json` `source_hat=[executor, coordinator]`）——executor 不在 `publishes` 包含 `review.passed`/`review.wave.ready` 中，因此最终 `work.failed` payload contract 校验失败，**整 loop 终止**。

> **关键澄清**：用户问的"没真的起多个 reviewer 去 review"——**症状描述准确，但根本原因不在 review-coordinator 没尝试 fan-out**，而是 **wave dispatch 阶段在 isolated mode 下被静默 drop 了**（推测路径：`enforce_wave_isolated_scope` 第一次调用时 `current_isolated_hat` 还没及时更新为 review-coordinator，或者 wave detection 的 `concurrency > 1` 检查在 `dimension-reviewer` hat 注册时未通过）。详见"问题归因表"。

---

## 执行链路对比图

### 预期链路（按 preset `ce-executor-isolated.yml:23-31`）

```
work.done (16:57:14, executor)
  └→ review-coordinator (16:57:14, 激活)
       └→ 写入 7 个 review.wave.ready 事件 (17:02:31)  [✓ 实际发生]
            └→ dimension-reviewer × 7 (并发 7 workers)  [✗ 未实际发生]
                 └→ review.dimension.done × 7
                      └→ review-synthesizer (aggregate wait_for_all)
                           └→ review.passed / review.failed
                                └→ plan-gate (review verdict)
                                     ├→ queue.advance → 下一 step
                                     └→ plan.complete → shipper → reporter → LOOP_COMPLETE
```

### 实际链路

```
work.done (16:57:14, executor)
  └→ review-coordinator (16:57:14, 激活)
       └→ 写入 7 个 review.wave.ready 事件 (17:02:31)  [✓ 已落到 events.jsonl]
       └→ 【缺口】 wave 派发未生效：
            ├─ wave detection 应识别 1 wave total=7 → spawn 7 workers
            └─ 实际：0 个 worker 被 spawn，0 个 wave-{wave_id}-{idx}.jsonl 被创建
  └→ (16:58:09) Isolated mode: event out of hat scope — dropping
       hat=executor topic=build.done × 9  [executor 误发 build.done，被 isolated scope 拒]
  └→ (17:04:33) hard gate: review-coordinator "did not emit any event"
       [但 7 个 review.wave.ready 已写入！]  ← 异常点 1
  └→ (17:06:42) executor 发 work.failed
       payload="Hat-routing mismatch: task.resume at 2026-06-13T01:04Z
                activated executor but the hard gate at 17:04Z was for
                review-coordinator"
       [executor 越权，isolated scope 拒其发出 work.failed]  ← 异常点 2
  └→ (17:06:55) payload_contract_violation
       整 loop 终止, exit code 非零, iterations=4
```

### 关键差异

| 步骤 | 预期 | 实际 | 差距 |
|------|------|------|------|
| review-coordinator 写 7 个 review.wave.ready | ✓ | ✓ (17:02:31) | 一致 |
| wave detection → 1 wave total=7 | ✓ | ⚠️ 未观察到 | 现场无 `Wave detected` log |
| spawn 7 个 dimension-reviewer workers | ✓ | ✗ 0 workers | **致命缺口** |
| 7 个 worker 各写 `wave-{id}-{idx}.jsonl` | ✓ | ✗ 0 文件 | 文件未创建 |
| 7 个 `review.dimension.done` 事件 | ✓ | ✗ 0 事件 | 派生链断裂 |
| review-synthesizer 触发 | ✓ | ✗ 未触发 | 因 review.dimension.done 缺失 |
| plan-gate 评估 | ✓ | ✗ 跳过 | 因 review verdict 缺失 |
| loop 完成（LOOP_COMPLETE） | ✓ | ✗ payload_contract_violation | 提前终止 |

---

## 证据清单

### A. 现场文件路径（`pittcat-dev` 主分支 / worktree）

| 文件 | 行数 | 关键内容 |
|------|------|---------|
| `presets/en/ce-executor-isolated.yml` | 1809 | preset 完整定义（10 hats / wave emit HARD RULE / event_policy / payload schemas） |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.ralph/events-20260612-161708.jsonl` | 20 | **20 个事件**：1 work.ready + 9 build.done + 1 work.done + 7 review.wave.ready + 1 human.guidance + 1 work.failed |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.ralph/loops.json` | 12 | loop 注册信息（worktree 模式，PID 18724） |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.ralph/agent/summary.md` | 35 | **Status: Failed: payload contract violation**, 20 events / 7 review.wave.ready / 1 work.failed |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.ralph/agent/tasks.jsonl` | 1 | task-1781281248-5e12 (U1 scaffold, closed) |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.ralph/agent/context.md` | 19 | worktree loop_id, branch, prompt |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.ralph/agent/scratchpad.md` | 72 | 18 条 HUMAN GUIDANCE 注入历史（每轮 2 条重复模式 "Focus on error handling" / "Keep this in mind"——存在 guidance 噪声） |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.ralph/history.jsonl` | 2 | loop_started (16:17:08) / loop_completed reason=payload_contract_violation (17:06:55) |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.ralph/diagnostics/2026-06-13T00-17-08/recovery.jsonl` | 3 | 3 envelopes：<br>① agent_doc_sync info `synced=0, skipped=2`<br>② **missing_event_gate** warning, source_hat=review-coordinator, target_hat=review-coordinator, topic=review.wave.ready, reason_code=missing_event, outcome=pending<br>③ **payload_contract** critical, topic=work.failed, reason_code=payload_contract_violation, outcome=**not_retriable** |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.ralph/diagnostics/2026-06-13T00-17-08/diagnosis-summary.json` | 13 | session_id=2026-06-13T00-17-08, total_iterations=4, recovery_count=0, drift_finding_count=0, loop_terminated_at=2026-06-12T17:06:55 |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.ralph/diagnostics/2026-06-13T00-17-08/active-activations.json` | 1 | 空 `[]` |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.ralph/diagnostics/payload-contract-error-2026-06-12T17-06-55-384619+00-00.json` | 18 | error_type=`payload_type_mismatch`, topic=work.failed, source_hat=[executor, coordinator], target_hat=[plan-gate], schema_defined_in=inline, fix_hint="Ensure the payload of 'work.failed' matches the schema's declared payload type." |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.ralph/diagnostics/logs/ralph-2026-06-13T00-17-08-559-18724.log` | 36 | 关键 log 行（按时间）：<br>① 16:22:41 Complete called for unknown or already-closed activation key coordinator (warmup 后协调器已关闭)<br>② 16:58:09 handoff dispatch timeout: routing task.resume to executor<br>③ 16:58:09 Isolated mode: event out of hat scope — dropping hat=executor topic=build.done × 9 （executor 9 次 build.done 全被 isolated scope 拒——agent 越权发 build.done）<br>④ 17:04:33 handoff dispatch timeout: routing task.resume to review-coordinator topic=work.done<br>⑤ 17:04:33 **Hard gate triggered: hat has publish obligation but emitted no event** hat=review-coordinator consecutive=1<br>⑥ 17:06:55 `[PAYLOAD CONTRACT VIOLATION] Loop paused.` |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.agents/scratchpad/ce-executor/2026-06-10-003-.../decisions.md` | DEC-001/002/004 | DEC-001: 任务拆分粒度; DEC-002: plan v2 baseline; **DEC-004: task.resume 越权（executor 拒绝代 review-coordinator 写 review 终态）**——confidence=75, 体现 executor 自查意识 |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.agents/scratchpad/ce-executor/2026-06-10-003-.../progress.md` | 41 | U1 step-01 DONE, commit e7cf782, 11 files +79 lines, 4074 passed / 39 failed（baseline 等同） |
| `.worktrees/2026-06-10-003-...-cheery-hawk/.agents/scratchpad/ce-executor/2026-06-10-003-.../logs/review-wave-payloads-step01.jsonl` | 7 JSONL | **7 个 review.wave.ready 完整 payload**（correctness/testing/maintainability/standards/requirements/agent-native/learnings）——与 events.jsonl 7 个事件完全对应 |

### B. 7 个 `review.wave.ready` 事件的关键属性（jq 抽取自 events.jsonl）

```
ts=2026-06-12T17:02:31.318444+00:00  [7 个完全相同]
wave_id=w-18b864225982d010-55383-0   [7 个完全相同 ← 一次性写入特征]
wave_index=0..6                       [0/1/2/3/4/5/6]
wave_total=7                          [7 个完全相同]
hat=review-coordinator                [7 个完全相同]
idempotency_key=ce-review:2026-06-10-003-...:task-1781281248-5e12:step-01:round-1  [7 个完全相同]
idempotency_hash=3e0e6369bdad7bd030988b756aacc5ef37e18a8e2bce9712ba5b02f87fe7c817  [7 个完全相同]
```

→ 这正是 `ralph wave emit` 一次性写入 N 个事件的指纹（见 `crates/ralph-cli/src/wave.rs:281-360` `write_wave_events` 函数：共享 `wave_id`、共享 `ts`、共享 `idempotency_hash`、递增 `wave_index`）。说明 **agent 这一步的 emit 操作是合规的**。

### C. 源码引用

| 位置 | 引用 |
|------|------|
| `crates/ralph-core/src/event_loop/mod.rs:6154-6320` | `process_events_from_jsonl_with_waves` 主流程：partition → origin guard → topic format → event policy → isolated scope → 委托给 `process_parse_result` |
| `crates/ralph-core/src/event_loop/mod.rs:6305-6312` | isolated mode 下 wave 事件需过 `enforce_wave_isolated_scope` 校验 |
| `crates/ralph-core/src/event_loop/mod.rs:1386-1460` | `enforce_wave_isolated_scope` 实现：按 wave_id 分组，第一个 distinct wave 必检 scope，第二 distinct wave 拒为 `IsolatedMultipleBusinessEmissions` |
| `crates/ralph-core/src/event_loop/mod.rs:4038-4043` | `current_isolated_hat` 在每次 hat 激活时被覆盖——本现场 review-coordinator 激活时 `current_isolated_hat = review-coordinator` |
| `crates/ralph-core/src/event_loop/wave_detection.rs:120-220` | `try_build_wave`：要求 wave_id 一致 + topic 一致 + wave_total 一致 + wave_index 在 [0, total) 范围内 |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:618-769` | `execute_wave_structured`：per-worker events_file 写到 `wave_dir/wave-{wave_id}-{index}.jsonl`（line 666） |
| `crates/ralph-cli/src/wave.rs:281-360` | `write_wave_events`：共享 wave_id + wave_index 0..N + wave_total=N + 同一 timestamp——与现场 7 个事件完全匹配 |
| `crates/ralph-cli/src/loop_runner/hard_gate.rs:422-460` | `inject_missing_event_hard_gate_guidance`：当 hat 有 publish obligation 但本轮未 emit 时触发 hard gate，注入硬门 guidance |
| `crates/ralph-cli/src/loop_runner/runner.rs:3465-3501` | runner 主循环中 `if !wave_events.is_empty() { handle_wave_events(...).await }` 派发路径 |
| `presets/en/ce-executor-isolated.yml:539-568` | review-coordinator 配置：`triggers=[work.done, fix.applied]`, `publishes=[review.wave.ready, review.passed]`, **obligations 强制 commit_count≥1 时必须 emit `review.wave.ready`**——本现场 commit_count=1, 7 个 review.wave.ready 已 emit，**obligation 已满足** |
| `presets/en/ce-executor-isolated.yml:813-820` | dimension-reviewer 配置：`triggers=[review.wave.ready]`, `publishes=[review.dimension.done]`, **`concurrency: 9`** ——支持最多 9 worker 并发 |
| `presets/en/ce-executor-isolated.yml:1004-1020` | review-synthesizer 配置：`triggers=[review.dimension.done]`, `aggregate.mode=wait_for_all`, `aggregate.timeout=300` |
| `presets/en/ce-executor-isolated.yml:153-156` | `work.failed` schema：`required_fields=[reason]`, `payload=json_object` |

---

## 问题归因表

| 等级 | 问题 | 证据 | 归因（机制 / preset / agent / 复合） |
|------|------|------|-------------------------------------|
| **P0** | 7 个 `review.wave.ready` 写入 events.jsonl 后，**wave dispatch 未触发**——0 个 `wave-{wave_id}-{idx}.jsonl` 文件被创建，0 个 dimension-reviewer worker 被 spawn，0 个 `review.dimension.done` 事件产生 | `ralph-2026-06-13T00-17-08-559-18724.log` 全程无 "Wave detected" / "executing parallel workers" 信息；`recovery.jsonl` envelope ② 显式标 `outcome=pending, safe_target=true` 但未触发；worktree `.ralph/` 下无任何 `wave-*.jsonl` 文件 | **机制 + 编排复合**：`process_events_from_jsonl_with_waves` 可能因 `current_isolated_hat` 时序、`enforce_wave_isolated_scope` 内部某判断分支、或 wave detection 失败（`max_wave_total` cap）导致 wave_events 被丢弃但**未落 recovery envelope**——这是静默 drop |
| **P0** | review-coordinator 被 hard-gate 重新激活（17:04:33），但**再次激活时** hat 视角下"已无 pending publish obligation"（7 个 review.wave.ready 已写过）——`missing_event_gate` 不应再触发 | `recovery.jsonl` envelope ② `reason_code=missing_event, safe_target=true, outcome=pending` | **机制**：`hard_gate.rs` 的 missing-event gate 计数逻辑未考虑"事件已写入但 worker 未 spawn"这一中间状态——只看到 hat 本轮"无新事件"就触发 gate，未检查 wave 派生事件是否在 historical events 中 |
| **P0** | hard-gate 后的 `task.resume` 路由到 executor 而非 review-coordinator，导致 executor 越权发 `work.failed` | `payload-contract-error-*.json` `source_hat=[executor, coordinator], target_hat=[plan-gate]`；`decisions.md DEC-004` 记录 executor 自查发现 routing 错配 | **机制**：hard-gate 后的 safe_target 路由逻辑（`hard_gate.rs:422` → 后续 routing）选错 hat。preset `tasks.coordinator_hats` 列表（line 39-46）含 executor 但不含 review-coordinator——可能影响 routing 表 |
| **P1** | scratchpad 中 18 条 `HUMAN GUIDANCE`（每轮 2 条重复 "Focus on error handling" / "Keep this in mind"）——TUI 中可能误显示为"reviewer 在工作"的提示，造成"已起多 worker"的视觉假象 | `.ralph/agent/scratchpad.md` 第 1-72 行 | **机制**：scratchpad 注入规则未去重——每个 build.done iteration（9 个）触发一次 guidance 注入，但内容相同 |
| **P1** | 9 个 `build.done` 事件被 isolated mode scope 全部 drop（`log:16:58:09 × 9`）——executor 误发 build.done，但 schema 明明在 line 121 声明 `topic_deny_rules: [{hat_id: executor, topic: build.done}]` | `ralph-2026-06-13T00-17-08-559-18724.log` 9 条 WARN | **preset 自身冗余 / agent 知识不足**：`topic_deny_rules` 已声明但 agent 仍 emit；说明 preset 硬规则足够，但 agent prompt 没有把这条规则钉死 |
| **P1** | `review-coordinator` 在硬门后 5 分钟（17:02 → 17:04 → 17:06）无新事件——其 "5-分钟内" 的沉默期与 `tasks.timeout`/iteration budget 配置不匹配 | log 时间窗 | **机制 / preset**：iteration budget 与 hard-gate reset 周期不匹配——hard-gate 触发后 5 分钟（17:04 → 17:06）才出 work.failed, 期间无任何 iteration 推进 |
| **P2** | `dimension-reviewer` hat 自身的 hard gate / fan-out 行为**未在 loop log 中留下任何 trace**——按预设它应该输出每个 worker 的 prompt 起始 banner | 全 log 36 行无 `dimension-reviewer` 启动信息 | **机制**：wave dispatch 阶段的 TUI/log 注入在 isolated mode + `concurrency>1` 路径上存在静默失败 |
| **P2** | 整 loop 失败 49m46s（16:17:08 → 17:06:55）但只跑了 4 个 iteration——**90% 时间用于 agent 内部推理**，真正 dispatch 步骤 0 次 | `summary.md` + history.jsonl | **复合**：单 iteration 处理 49 分钟（agent 推理 + emit 7 事件 + 沉默）说明 agent prompt 在 U1 第一次 emit 后没有明确的"等待 wave 派生"指令，导致 5 分钟空转 |

---

## 修复建议

### 针对 preset 问题（`presets/en/ce-executor-isolated.yml`）

| 优先级 | 建议 | 位置 |
|--------|------|------|
| **P0-1** | 在 `review-coordinator` instructions 末尾追加 "After emitting review.wave.ready, **MUST actively poll events.jsonl for 5 `review.dimension.done` events**（或 aggregate wait_for_all 触发 `review.passed`/`review.failed`）— do NOT consider review phase done until review-synthesizer emits a terminal review event. If 0 dimension.done events arrive within 5 minutes, re-emit review.wave.ready with fresh `--idempotency-key`." | preset line ~810（review-coordinator 段尾） |
| **P0-2** | `dimension-reviewer` hat 的 HARD RULE 增加 explicit 失败回退：当 worker 因任何原因（backend 错误 / context overflow / hat 不可达）未在 1800s 内 emit `review.dimension.done` 时，**必须**发一个 `review.failed`（而非沉默） | preset line ~880 (dimension-reviewer instructions 段) |
| **P1-1** | `dimension-reviewer` hat 的 `publishes: ["review.dimension.done"]` + `terminal_events: ["review.dimension.done"]` **改为同时 publish 一个 `review.failed`** 作为 worker 死掉的回退信号 | preset line ~817-818 |
| **P1-2** | preset 顶部注释（line 23-31）加一条 "If wave dispatch silently drops 0/N workers (verify via `ls .ralph/wave-*.jsonl`), emit `plan.blocked` with reason=`wave_dispatch_silent_drop`" | preset line 23 后 |
| **P1-3** | `tasks.coordinator_hats` 列表（line 39-46）加 `review-coordinator`——但要注意会改变 routing 表，需独立测试 | preset line 39-46 |
| **P2-1** | 在 preset 中明确"agent emit 完后必须验证 wave 派发已发生"：用 `ralph diagnose --session latest` 或 `jq '[.events[] \| select(.topic=="review.wave.ready") and (.wave_id == $wid)] \| length' .ralph/events.jsonl` 自检 | preset line ~780 附近 |

### 针对 Ralph loop 机制（`crates/ralph-core/src/event_loop/` + `crates/ralph-cli/src/loop_runner/`）

| 优先级 | 建议 | 位置 |
|--------|------|------|
| **P0-A** | `process_events_from_jsonl_with_waves` 在 `enforce_wave_isolated_scope` 之后追加**强制 wave-fan-out 验证**：当 wave_events 非空但 `handle_wave_events` 的 `outcome` 为空（line 289-291）时，落 recovery envelope `source=wave_silent_drop, severity=warning`，并把 `wave_id`/`total`/`target_hat` 写入 envelope evidence——避免 7 个事件被静默吞掉 | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:288-291` |
| **P0-B** | `missing_event_gate`（`hard_gate.rs:422`）的触发条件增加豁免：**当 hat 在最近 N (5) 秒内有过任何事件写入到 events.jsonl 时，不触发 missing_event_gate**（因为 hat 视角下"已发"了，gate 不应再算） | `crates/ralph-cli/src/loop_runner/hard_gate.rs:422` |
| **P0-C** | hard-gate 后的 `task.resume` safe_target 路由：当前选择 `current_isolated_hat` 但该 hat 不一定就是被 hard-gate 的 hat——应直接用 `original_hat` (即 hard-gate 触发的 hat) 而非 current_isolated_hat | `crates/ralph-core/src/event_loop/mod.rs:4034-4035`（`esc.safe_target` 计算处） |
| **P0-D** | `process_events_from_jsonl_with_waves` 中 `enforce_wave_isolated_scope` 调用前，确保 `current_isolated_hat` 已正确指向**本轮正在 dispatch 的 hat**——本现场可能因 hat lifecycle 完成时 `current_isolated_hat` 提前清空，导致 `isolated_publish_allowed` 检查用了错误 hat | `crates/ralph-core/src/event_loop/mod.rs:6305-6312` |
| **P1-A** | `dimension-reviewer` 的 `concurrency=9` 在 isolated mode 下应**显式 fan-out 到 max(7, dimension 实际数量)**——而不是把 7 个事件当成 1 wave 仍只 spawn 1 worker | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:638`（`concurrency = wave.hat_config.concurrency`） |
| **P1-B** | wave detection 失败时**必须落 recovery envelope**：`detect_all_wave_events_capped` 返回空 + 非空 wave_events 时，落 `wave_detection_empty_outcome` envelope，evidence 写入 wave_id / total / partition 前的事件数 | `crates/ralph-core/src/event_loop/mod.rs:6289-6294` |
| **P1-C** | `runner.rs:3465` 的 `if !wave_events.is_empty()` 检查应改为 `match wave_events { empty => warn + recovery envelope, non_empty => handle_wave_events }` —— 避免静默 skip | `crates/ralph-cli/src/loop_runner/runner.rs:3465` |
| **P2-A** | scratchpad 注入规则去重：相同 HUMAN GUIDANCE 5 分钟内不重复注入（避免本现场 18 条噪声） | `crates/ralph-core/src/event_loop/mod.rs:4840` 附近（scratchpad 注入逻辑） |
| **P2-B** | `dimension-reviewer` worker 启动时，**在 events.jsonl 写一条 `worker.started` 事件**（hat=dimension-reviewer, wave_id, wave_index）——这样 reviewer 真的"起"了的话 events.jsonl 立刻可观测，TUI 也能直接看到 7 个 worker 各自的进度 | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:678` 附近（worker spawn 前） |

### 针对产物问题（agent prompt / scratchpad / decisions）

| 优先级 | 建议 |
|--------|------|
| **P0-1** | `dimension-reviewer` agent prompt 增加显式 telemetry 指令："**At the start of your response, write a marker line to events.jsonl** via `echo '{"hat":"dimension-reviewer","topic":"worker.started","wave_id":"...","wave_index":N,"ts":"..."}' >> .ralph/events.jsonl` (read RALPH_WAVE_ID / RALPH_WAVE_INDEX env vars)——TUI 依赖此 marker 确认 worker 真的 spawn 了" |
| **P0-2** | `review-coordinator` agent prompt 在 emit `review.wave.ready` 之后强制要求："**Wait for `review.dimension.done` events OR 5 minutes, whichever first**——do NOT emit `work.failed` (it is not your publish authority); if 0 dimension.done arrive in 5 min, re-emit `review.wave.ready` with new `--idempotency-key`" |
| **P1-1** | scratchpad 注入逻辑去重 + `decisions.md` 模板**禁止**重复 DEC 编号——本现场 DEC-003 缺失（直接跳到 DEC-004），说明 decisions 编号管理失控 |
| **P1-2** | 每个 hat 完成后，**必须在 decisions.md 记录 wave_id 实际处理结果**（worker 数 / dimension 数 / 实际 emit 的 review.* 事件）——而非只记录"发了 7 个 review.wave.ready"这种"我以为我发了"的描述 |

### 验证清单（修改后跑一遍）

1. **重新启动同一个 worktree loop**（`worktree_path=/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-cheery-hawk`），跑 `ralph run -c ralph.yml -H builtin:ce-executor-isolated -p docs/plans/2026-06-10-003-...-plan.md`
2. **TUI 端验证**：在 review 阶段确认 TUI 显示 7 个 worker 各自的进度行（`Worker N/7: launching...` / `Worker N/7: ...`），TUI 状态 `wave_active.iterations[last].worker_buffers` 7 个 buffer 都有内容
3. **磁盘验证**：
   - `ls .ralph/wave-*.jsonl | wc -l` 应 ≥ 7（每个 worker 独立 events file）
   - `jq 'select(.topic=="review.dimension.done") | length' .ralph/events.jsonl` 应 = 7
   - `jq 'select(.topic=="worker.started") | length' .ralph/events.jsonl` 应 = 7（如果 P2-B 实施）
4. **recovery envelope 验证**：`recovery.jsonl` 中应**无** `reason_code=missing_event` 且 `source_hat=review-coordinator` 的 envelope
5. **terminal event 验证**：在 review 阶段结束时应 emit `review.passed` 或 `review.failed` 或 `review.complete`——而不是 `work.failed`

---

## 附：分支与现场路径速查

| 项 | 路径 / 标识 |
|----|-------------|
| 当前诊断分支 | `pittcat-dev`（主仓库） |
| 被诊断 loop worktree | `.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-cheery-hawk` |
| Loop ID | `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-cheery-hawk` |
| Worktree branch | `ralph/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-cheery-hawk` |
| Loop PID | 18724 |
| Start time | 2026-06-12T16:17:08Z |
| End time | 2026-06-12T17:06:55Z (49m 46s) |
| Iterations | 4 |
| Final commit | e7cf782 (`feat(ralph-core): U1 scaffold 10 submodules for event_loop split`) |
| Preset | `ce-executor-isolated` (10 hats: coordinator/executor/review-coordinator/dimension-reviewer/review-synthesizer/fixer/debug-resolver/plan-gate/shipper/reporter) |
| Plan | `docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md` (786 行, v2 baseline @ 918192a) |
| Wave ID | `w-18b864225982d010-55383-0` (total=7, indices 0..6) |
| 报告自身路径 | `report/2026-06-13-review-wave-no-spawn.md`（`pittcat-dev` 分支下） |
