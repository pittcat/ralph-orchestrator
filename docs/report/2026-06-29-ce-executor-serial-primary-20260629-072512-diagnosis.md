# Ralph Loop 诊断报告 — `primary-20260629-072512` (ce-executor-serial)

> 角色:Ralph Loop / preset 运行链路诊断专家
> 中间产物:`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/`
> Preset:`presets/en/ce-executor-serial.yml`(10-hat,isolated)
> 参考代码:`pittcat-dev` 分支
> 运行时间:07:25:12 → 07:57:59(32m46s,13 iter,15 events)

---

## 1. 结论摘要

**本次 run 健康度:严重不健康(P0×3 + P1×2 + P2×3)**。4 个 unit 步骤技术上都 commit 落地(共 +1162 行,10/23/29 tests 通过),但整个 **review 链 + shipper/reporter 链 + LOOP_COMPLETE 链全部未启动**(grep 0 review events / 0 plan.complete),loop 终止于 `loop.terminate reason=stopped`(手动),不是 preset 设计的自然终点。

**核心定性**:不是"修复机制失效",也不是"agent 乱跑",而是 **coordinator 末步发空 `task_id` + state projector 静默回退 + hard gate 不区分 hat 角色** 三层叠加导致的硬门耗尽。**与 history `primary-20260629-032235` / `172725` / `115810` 三个最新 loop 复发同一模式群(4.3 task_id 形态漂移 + 2.4 stall_recovery 双轨 + 3.1 recovery_exhausted 不串联 plan.blocked)**。`docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` 是目前最完整的机制级闭环范例(3 层防御:lint + runtime gate + verdict_gate),本次 1.3/2.4/3.1 应按此范式重做。

---

## 2. 执行链路对比图(Agent A)

### 预期拓扑(preset 定义,10-hat)

```
work.start → coordinator → work.ready(step-NN) → executor → work.done → validator → test.passed
                                                       ↓
                                              coordinator: 推进 step-(N+1) 或 末步 → review.start
                                                                                          ↓
              review.start → review-coordinator → 6 × review.dimension.{ready,done} → review-synthesizer
                                                → review.complete(fix_plan_file) → coordinator → work.ready(fix-NN)
                                                                                          ↓
              test.passed(fix-NN 末) → coordinator → plan.complete → shipper → REVIEW_COMPLETE
                                                → reporter → report.done → LOOP_COMPLETE
```

### 实际事件流(15 events,13 iter)

| # | ts (UTC) | hat | topic | 关键 payload | 状态 |
|---|---|---|---|---|---|
| 1 | 07:25:12 | loop | `work.start` | 引导 prompt | OK |
| 2 | 07:26:02 | coordinator | `work.ready` | step-01, **task_id=""** | WARN |
| 3 | 07:34:23 | executor | `work.done` | step-01, task_id=`task-1782717995-2a71`, +368 lines | OK |
| 4 | 07:34:42 | system→validator | `task.resume` | **missing_event_gate** RD-EXECUTOR-RESEND-LIMIT | FAIL |
| 5 | 07:35:48 | validator | `test.passed` | step-01, 10 tests | OK |
| 6 | 07:38:46 | coordinator | `work.ready` | step-02, task_id=`task-1782718722-476c` | OK |
| 7 | 07:41:26 | executor | `work.done` | step-02, +236 lines | OK |
| 8 | 07:41:43 | system→validator | `task.resume` | **missing_event_gate** consecutive=2 | FAIL |
| 9 | 07:42:16 | validator | `test.passed` | step-02, 23/23 tests | OK |
| 10 | 07:44:14 | coordinator | `work.ready` | step-03, task_id=`task-1782718722-476c`(同 step-02 复用) | WARN |
| 11 | 07:49:18 | executor | `work.done` | step-03, +137 lines | OK |
| 12 | 07:51:35 | validator | `test.passed` | step-03, 29/29 tests | OK |
| 13 | 07:52:58 | coordinator | `work.ready` | step-04, **task_id=""** | FAIL |
| 14 | 07:57:50 | executor | `work.done` | step-04, task_id=`task-1782719850-0000`, +421 lines | FAIL projector 拒 |
| 15 | 07:57:59 | system→executor | `task.resume` | **missing_event_gate** consecutive=3, gate exhausted | FAIL |
| — | 07:57:59 | loop | `loop.terminate` | reason=stopped, manual | STOPPED |

### 闭环失败的因果链

```
step-04 coordinator work.ready(task_id="")                    # events:13
  → state_projector 静默回退 from_key:...                       # task.rs:75-84
  → executor 收不到合法 task_id → 自创 task-1782719850-0000        # events:14
  → state projection rejected: task_not_found                   # log:99
  → events.retain 移除该 event                                    # state_projector/mod.rs:813-820
  → agent_wrote_any_valid_or_rejected=false                      # loop_runner/runner.rs:4001
  → hard gate counter +1 (第 3 次,无 hat 角色区分)                # event_loop/mod.rs:1620
  → Hard gate exhausted, count=3                                 # log:101
  → loop.terminate reason=stopped (manual)                       # events-history:2
  → review.start / plan.complete / LOOP_COMPLETE                 # 全部未触发
```

---

## 3. 历史问题上下文(Agent B)

本次 run 复发了 `primary-20260629-032235` / `172725` / `115810` 三个最新 loop 的 **4 类历史模式群**(共 9 条),其中 **3 条 P0 未闭环**:

| 模式群 | 历史编号 | 文档 | 状态 |
|---|---|---|---|
| **编排卡点** | 1.3 `FlowStepScope` 误拒 | `docs/report/2026-06-29-ce-executor-serial-primary-20260629-032235-diagnosis.md` | **本次复发** |
|  | 5.1 coordinator 越权推 work.ready | 同上 | **本次复发** |
| **修复机制失能** | 2.4 `stall_recovery` 双轨 retry_key | `docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-diagnosis.md` | **本次复发** |
|  | 2.6 drift field_completeness 自观测 | 同上 | 本次相关 |
| **闭环断路** | 3.1 `recovery_exhausted` 不串联 shipper/reporter → `plan.blocked` | 同上 | **本次复发(P0-3 未修)** |
|  | 3.2 `human.guidance` 在 isolated 模式死信 | 同上 | **本次复发(P0-4/5 未修)** |
| **基座 bug 复发** | 4.3 `TaskWrongLoop` 反复拒(`""` × 5 / `from_key:` × 4) | `docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-diagnosis.md` | **本次复发(P0-2/3 未修)** |
|  | 4.4 hat_handoff 0 触发(30 天第 6+ 次) | `docs/achieved/solutions/developer-experience/ce-executor-serial-30day-6th-recurrence-fix.md` | 本次相关 |
|  | 4.6 `IdempotentLog::disabled` fallback | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md` | **本次复发(P0-10 未修)** |

**最高关联度**:`docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md`(3 层防御:lint + runtime gate + verdict_gate)是目前最完整的"机制级闭环"范例,本次 1.3/2.4/3.1 应按此范式重做。

---

## 4. 证据清单(Agent C,9 条偏离 D1-D9)

### 必修偏离(契约硬伤)

- **D1**:`events-20260629-072512.jsonl:2` `work.ready`(step-01)`task_id: ""` — 违反 schema required_fields
- **D2**:`events:13` `work.ready`(step-04)`task_id: ""` — 同样违反
- **D3**:loop 在 step-04 中途被手动停止,`review.start` / `REVIEW_COMPLETE` / `LOOP_COMPLETE` 全部未触发

### 高频偏离(恢复机制反复触发)

- **D4**:`events:4,8` validator 连续 2 次 `missing_event_gate`,drift 路径 `recovery.jsonl:2,5` outcome 在 Recovered / Repeated / Pending 间漂移 5 次
- **D5**:`events:15` executor 同样触发第 3 次 `missing_event_gate`(consecutive=3 → exhausted)
- **D6**:`ledger.jsonl:3,4,8,9` 出现 2 次 `topic_denied` 拒绝 `task.resume`(noise,无功能影响)

### 轻微偏离(不影响闭环)

- **D7**:`tasks.jsonl:3,4,5` step-02/03 共享 task_id `task-1782718722-476c`,违反 SSOT
- **D8**:`agent/progress.md:8-10` Completed Steps = `[step-01,02,03]`,与 events 不同步
- **D9**:`agent/summary.md:9` `_No scratchpad found._`,违反 coordinator 强制要求

---

## 5. 问题归因表(Agent D,P0 / P1 / P2)

| # | 偏离描述 | 根因分类 | 优先级 | 源码定位 | 历史关联 |
|---|---------|---------|--------|---------|---------|
| **R1** | step-04 work.ready `task_id=""` → executor 自创 ID | preset 设计 + Ralph 基座叠加 | **P0** | `presets/en/ce-executor-serial.yml:609-740` + `state_projector/task.rs:75-84` | 4.3 task_id 形态漂移 |
| **R2** | step-04 work.done 被 projector 拒收,`events.retain` 移除 → 触发 hard gate | Ralph 基座机制 | **P0** | `state_projector/mod.rs:813-820` + `loop_runner/runner.rs:4001-4254` | 新问题 |
| **R3** | hard gate 累计 3(validator×2 + executor×1)未区分 hat 角色即终止 | preset 设计(兜底过激) | **P1** | `event_loop/mod.rs:1236,1620` `HARD_GATE_MAX=3` | 已知模式 |
| **R4** | coordinator 指引未禁止空 task_id,projector 静默回退 from_key | preset 设计 | **P0** | `presets/en/ce-executor-serial.yml:609-740` + `state_projector/task.rs:75-84` | `ce-executor-task-ownership` memory |
| **R5** | executor 看到空 task_id 时回退方案不一致(step-01/02/03 能自创,step-04 失败) | agent 执行/产物问题 | **P1** | `task_cli.rs:607-689` + executor prompt line 1102 | `ce-executor-stale-activation-work-done-closure` |
| **R6** | 终态路径(`review.start` / `plan.complete` / `LOOP_COMPLETE`)全部未触发 | 多因素叠加(R1+R2→R6) | **P0** | `event_loop/mod.rs:1620-1625` + preset 终态分支未运行 | 3.1 `recovery_exhausted` 不串联 `plan.blocked` |
| **R7** | `.ralph/loops.json` 始终 `[]`(primary loop 不注册) | Ralph 基座 | **P2** | `run.rs:1366-1371` 仅 worktree 模式 register | 已知架构 |
| **R8** | step-04 executor 兜底失败(validator 兜底 OK) | preset 设计(兜底策略不一) | **P2** | `loop_runner/runner.rs:4254` `inject_missing_event_hard_gate_guidance_with_triggers` | 同 R3 |
| **R9** | `progress.md` 未记录 step-04 完成 | 多因素叠加(R1+R2+R6) | **P2** | `state_projector/progress.rs` 由 projector 推动,work.done 被拒后未跑 | 同 R6 |

---

## 6. 修复建议(按优先级)

### Fix-1 (P0,解 R1/R4/R6)— 强制非空 task_id

**目标**:`presets/en/ce-executor-serial.yml` coordinator 指引 + `state_projector/task.rs` 移除静默回退

**改动 A** — `presets/en/ce-executor-serial.yml:609-740` 在 coordinator `Runtime Task Creation` 段追加:

```
- **MANDATORY task_id derivation (HARD RULE)**:
  - The `task_id` field in `work.ready` payload MUST be a non-empty string.
  - Recommended: read it from `.ralph/agent/tasks.jsonl`
    (`ralph tools task show <task_key>`) BEFORE publishing work.ready.
  - DO NOT emit `task_id=""` — empty task_id → state projector
    fail-closes the matching work.done → hard gate exhaustion.
```

**改动 B** — `crates/ralph-core/src/state_projector/task.rs:75-84` 静默回退改为 fail-closed:

```rust
if json_pointer(payload, "task_id").map(str::is_empty).unwrap_or(true) {
    return Err("empty_task_id_in_work_ready: coordinator must embed the projector-derived id".to_string());
}
```

**预期**:step-N coordinator 发 work.ready 携带从 `tasks.jsonl` 反查的真实 id;work.done task_id 匹配 → projector 接受 → hard gate 不触发。

### Fix-2 (P0,解 R2)— 区分"agent 没 emit"与"agent emit 但被 projector 拒"

**目标**:`loop_runner/runner.rs:4001` 把 state projection 拒绝的事件也视为"agent 写过"

**改动** — `crates/ralph-cli/src/loop_runner/runner.rs:4179-4253` 区分两类失败:

- `agent_wrote_any_valid_or_rejected=false` 且 `projection_rejections.is_empty()` → 真没 emit → hard gate fires
- `projection_rejections` 非空 → 走 schema-level guidance(executor 重发 work.done 时按正确 task_id)

**预期**:step-04 executor work.done 被 projector 拒后,runner 注入 schema guidance,executor 重发 → 不再走 hard gate。

### Fix-3 (P1,解 R3)— HARD_GATE_MAX 按 hat 角色分级

**目标**:`event_loop/mod.rs:1315` 区分 executor/validator/coordinator 阈值

**改动** — `crates/ralph-core/src/event_loop/loop_state.rs:22`:

```rust
pub const HARD_GATE_MAX_PER_HAT: &[(&str, u32)] = &[
    ("executor", 5),    // executor 经常是 work.done,projector 拒收概率高
    ("validator", 3),
    ("coordinator", 3),
];
```

`event_loop/mod.rs:1620` 改为按当前 hat 查表。

**预期**:executor 错 5 次才停,validator 仍 3 次。R3 类 flake 不再单点自杀。

### Fix-4 (P2,解 R5/R7/R9)— 兜底与观测

- **R5**:`presets/en/ce-executor-serial.yml:1102` executor 指引补 "if `task_id == ""`,call `ralph tools task show --key <task_key>`"
- **R7**:`loop_registry.rs:8-12` 注释 + `run.rs:1368-1371` 增加 primary 模式可选用 `history.jsonl` 形式记录
- **R9**:`state_projector/progress.rs` 在 `project_close_task` 失败时也写 `progress.md` 的 `Attempted Steps` 段(只做观测,不阻塞 ledger)

### 验证标准

1. 跑同一 plan,Fix-1+2 后 step-04 work.done 应被 projector 接受 → coordinator 收到 test.passed → 发 `review.start` → 走 review 链 → `LOOP_COMPLETE`
2. `cargo nextest run -p ralph-core -- state_projector` 加新单测:empty task_id 必 reject
3. `cargo nextest run -p ralph-cli --bin ralph -- hard_gate` 加新单测:projection reject 不增 hard gate counter
4. `./scripts/run-tests.sh` 全基线

---

## 7. 终止根因一句话总结

本次 run 不是"编排机制有问题"也不是"修复机制失效",而是 **coordinator 末步发空 `task_id` + state projector 静默回退 + hard gate 不区分 hat 角色** 三层叠加,导致 loop 在 4/4 step 全部 commit 通过、tests 全绿的情况下,因硬门耗尽而停在 step-04 末,review / ship / report 链全未启动。**这是历史 `primary-20260629-032235` / `172725` / `115810` 三个 loop 同一模式群的第 4 次复发**,3 个 P0 都未闭环,根因层在 `state_projector/task.rs` 的静默回退 + `loop_runner/runner.rs:4001` 的 hard gate 触发逻辑 + preset coordinator 指引的契约缺位。修复按 `ce-executor-serial-mechanism-close-loop-2026-06-23.md` 范式重做(3 层防御:lint + runtime gate + verdict_gate)。
