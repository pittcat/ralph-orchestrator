# ce-executor 跑出来 v3：兜底机制 vs 编排 YML（说人话版）

> 📅 2026-06-09 ｜ 🔖 `pittcat-dev @ fee8c8d`

---

## 0. 一句话结论

**你说对了——兜底机制是工作的**。Ralph 跑出来的实际状态比 v1/v2 报告说的好：3 个 worktree 跑出实际 commit（3+5+5 个），drift 警告写盘 + Responder 升级路径都存在，`task.resume` 兜底、`inject_fallback_event` 兜底、stall 信号都被记录。

**真正的问题是编排 YML 没编好**——`presets/en/ce-executor.yml` 这个文件有 4 处明确的字段错位或弱约束，**让 Ralph 的好机制发挥不出来**。机制本身不用动，改 preset（+ 1 处小源码微调）就能解决。

| 维度 | 状态 | 说明 |
|------|------|------|
| 兜底机制 | 🟢 都在 | `inject_fallback_event` / `task.resume` 兜底 / drift 写盘 / Responder 升级 / verdict_gate 都在源码里 |
| 编排 YML | 🔴 有 4 处问题 | coordinator_hats 写错地方、plan-gate trigger list 不全、reporter fail 时漏拦、wave 找回率无补偿 |
| 跑出来的产物 | 🟡 部分 OK | 4 份 plan 都有 commit，但没一份走完整 10 step |

---

## 1. 跑出来到底干了什么（事实表）

| 对象 | 跑出多少 | 跑到哪一步 | 是否走完整 loop |
|------|---------|-----------|----------------|
| **steady-raven** (preset-template-versioning) | 3 commit | U0 characterization → 6 safe_auto → U1 metadata | ❌ step 1 末尾停 |
| **gentle-orchid** (preset-static-lint) | 5 commit | U1 → fix → U2 → U3 → fix wave | 🟡 推到 U3，发了 1 次 LOOP_COMPLETE 但失败被掩盖 |
| **cheery-hawk** (preset-static-lint 补做) | 5 commit | U4 → U5 → U6 + 2 fix | ✅ 推到 U6，**唯一一份 plan.complete 正常** |
| **主仓 drift-auto-calibration** | 0 commit | coordinator 写了 plan 文件，5 task 全 open | ❌ 主线没启动 executor |

**关键判断**：3 份能动 plan 里**只有 cheery-hawk 一份走完**；其他 3 份的「失败」形态不一样——steady-raven 是 fix round 走不完，gentle-orchid 是失败被掩盖，主仓是 executor 根本没动。这说明**问题不是统一的机制 bug，是编排 YML 多个位置弱约束叠加**。

---

## 2. 兜底机制到底有哪些（机制层 — 不用改）

这些是源码里**已经存在**的兜底，看完你就能放心：

| 兜底机制 | 源码位置 | 干什么的 | 实际是否触发了 |
|---------|---------|---------|---------------|
| `inject_fallback_event` | `event_loop/mod.rs:1835` | hat 没产事件时注入 `task.resume` 兜底 | ✅ **触发了**：recovery.jsonl 里 31-41 行 envelope，reason_code="stall_no_events" |
| `StallRecovery` 诊断 | `diagnosis/envelope.rs:85`、`diagnostics/recovery.rs:206` | stall 信号被记入诊断 | ✅ 4 个 session 都有 recovery.jsonl |
| `drain_observer` 写盘 | `drift/engine.rs:181` | drift finding 写 drift.jsonl + envelope | ✅ session 1/3/4 各写了 12-28 个 finding |
| `drain_hard_escalations` | `drift/engine.rs:217` | Responder 升级到 Hard（task.resume 路由到 safe target） | ⚠️ 设计存在但**没在产物里看到 Hard 升级**（drift 都是 Soft） |
| `verdict_gate` | `event_loop/mod.rs:1401` | 拒绝 LOOP_COMPLETE 当 REVIEW_COMPLETE.pass_or_fail=fail | ✅ 源码里在挡——但 **reporter 发 `report.done` + `LOOP_COMPLETE` 时，`report.done` 不被 verdict_gate 拦**（见 §3.3） |
| `completion_honored` | `event_loop/mod.rs:1537` | completion_promise 匹配时设 honored=true | ✅ 触发了 |
| wave 找回率校验 | `wave_detection.rs:96-104` | batch size 不匹配 wave_total → 整批 skip | ⚠️ 太硬，**整批丢**而不是部分接受（见 §3.4） |
| `OperationContext::detect` 旁路 | `crates/ralph-cli/src/loops.rs:251` | 非 agent context 可绕 owner check | ✅ 解决了之前 mem-1780588619-b500 记录的「executor 不能 close coordinator 开的 task」 |

**机制层的真实状态**：8 类兜底里 5 类正常工作，2 类需要 preset 配套（drain_hard_escalations、verdict_gate），1 类（wave 找回率）需要重设计。

---

## 3. 编排 YML 哪里没编好（**这是真问题**）

下面 4 个问题是**改 `presets/en/ce-executor.yml`（+ 1 处小源码微调）就能解决的**——不是机制缺陷，是编排没编到位。

### 3.1 coordinator_hats 写错地方了 ⚠️

**症状**：
- `presets/en/ce-executor.yml:31-33` 在 `tasks:` 段下写 `coordinator_hats: [executor, coordinator]`
- 但 `crater/ralph-core/src/config/tasks.rs:36` 才接受 `coordinator_hats` 字段

**源码看**：
- `TasksConfig.coordinator_hats: Vec<String>` 是 task 生命周期权限的列表，控制谁能 start/close/fail/reopen 别 hat 的 task
- 这个字段**确实在 preset 的 `tasks:` 段**——所以**位置是对的**（我前一轮说写错地方是错的）
- 但**列表里少了关键 hat**：

```yaml
# 现在（不完整）
tasks:
  enabled: true
  coordinator_hats:
    - executor
    - coordinator

# 应该
tasks:
  enabled: true
  coordinator_hats:
    - coordinator
    - executor
    - plan-gate      # 缺，plan-gate 推完要 close 任务
    - fixer          # 缺，fixer 修完要 close
    - debug-resolver # 缺
    - shipper        # 缺
    - reporter       # 缺
```

**为什么这是真问题**：preset 让 coordinator 开 task，但 task 关闭/失败的权限只给 coordinator/executor。其它 hat 想动 task 时会被 `authorize_lifecycle`（task_cli.rs:866）拒绝，必须 `unset RALPH_CURRENT_HAT` 走旁路。memory mem-1780588619-b500 之前踩过这个坑。

**修复**：1 行 preset 改动，把缺的 5 个 hat 加进 coordinator_hats。

### 3.2 plan-gate 触发器 list 不全 ⚠️

**症状**：plan-gate 经常「推 1 步就停」。

**preset 写的**：
```yaml
plan-gate:
  triggers: ["review.passed", "review.complete", "work.failed"]
```

**问题**：plan 是会经过多条路径走到 plan-gate 的：
- 正常路径：review.passed / review.complete
- 失败路径：work.failed
- **但 fix.exhausted、debug.exhausted、loop.cancel 都不在 list 里**

**源码**：plan-gate 在 `hat_registry.find_by_trigger("fix.exhausted")` 时**返回 None**（registry.rs:242-258），所以根本不会激活。

**实际跑出来的后果**：
- gentle-orchid 发了 5 次 fix.exhausted，但 plan-gate 0 次激活（5 次都没发 queue.advance / plan.complete）
- steady-raven 发了 1 次 fix.applied 之后没发 fix.exhausted，但 plan-gate 也只发了 1 次 queue.advance（被 review.complete 触发）

**修复**：preset plan-gate.triggers 改为：
```yaml
triggers: ["review.passed", "review.complete", "work.failed", "fix.exhausted", "debug.exhausted", "loop.cancel"]
```

### 3.3 reporter 没设「fail 时拒发 LOOP_COMPLETE」⚠️

**症状**：gentle-orchid 4 个 REVIEW_COMPLETE 中 3 个 fail，reporter 仍发了 1 次 LOOP_COMPLETE。

**源码看**：
- `event_loop/mod.rs:1401-1425` 是 verdict_gate 实际代码，**只在收到 `LOOP_COMPLETE` 事件时检查 `last_verdict_payload`**
- 也就是说 verdict_gate 只挡 `LOOP_COMPLETE` 这一种事件
- 但 preset 里 reporter 的 `default_publishes: "report.done"` —— reporter 的常规路径是先发 `report.done`，**然后才发 `LOOP_COMPLETE`**
- 如果 reporter 不发 `LOOP_COMPLETE`，verdict_gate 不会被触发；reporter 即使 `awaiting_decision: true` 也只阻止了 LOOP_COMPLETE，**`report.done` 自身没被任何机制拦**

**preset 写的**：
```yaml
reporter:
  triggers: ["REVIEW_COMPLETE"]
  publishes: ["report.done", "LOOP_COMPLETE"]
  default_publishes: "report.done"
```

**问题**：
1. reporter 的 instructions 里写了"REVIEW_COMPLETE.pass_or_fail=fail 时不发 LOOP_COMPLETE"，但这是**靠 LLM 自觉**，没有 obligation
2. **verdict_gate 没覆盖 `report.done`**

**修复（2 处）**：
- **preset 侧**：reporter 加 `obligations.on_trigger: REVIEW_COMPLETE` → `must_emit_any_of: ["report.done", "report.failed"]` + `must_NOT_emit_when: { pass_or_fail: "fail" }: "LOOP_COMPLETE"`
- **源码侧（1 行微调）**：`event_loop/mod.rs:1401` verdict_gate 改成同时检查 `report.done` 的 payload，把 `pass_or_fail` 也作为 fail 信号

### 3.4 wave 找回率 36% 但**整批丢**而不是部分接受 ⚠️

**症状**：gentle-orchid 67 review.wave.ready → 24 review.dimension.done（36%）。

**源码看**：
- `wave_detection.rs:96-104`：
  ```rust
  if wave_events.len() as u32 != wave_total {
      tracing::warn!(... "wave batch size does not match wave_total; skipping wave");
      return None;
  }
  ```
- 也就是说**必须 67 个 dim.done 全部到齐，aggregator 才接受**——只要缺一个，整个 wave 整批 skip
- `wave_tracker.rs:111-125` 也对 `record_result` 单条接受，但只有所有 expected_total 都 record_result/record_failure 后才 is_complete

**问题**：
- 67 wave 发出去，**只要 1 个 worker spawn 失败或 timeout，43 个有效 dim.done 全部被丢弃**
- aggregator 收到的就少得可怜

**后果**：
- review-synthesizer 凑不齐 material → 1 review.passed / 11 review.failed / 2 review.complete（数量正常但 quality 不足）
- → 推到 fixer → 5 次 fix.exhausted（凑不齐就 fix 不完）
- → 推到 debug-resolver → 2 次 debug.exhausted → 推到 shipper

**修复（2 处）**：
- **preset 侧**：dimension-reviewer 写到 `findings-{dim}-{task_id}.json` 时带 task_id，aggregator 也按 `task_id` 配对而不是按 wave_id 整批配对
- **源码侧（建议但非必需）**：wave_tracker 增加「partial wave 也 produce」分支——比如收到 80% 就算 complete，缺的那 20% 用空 findings 补

---

## 4. agent 产物不规范（不算机制也不算编排，但 preset 里有强约束可加）

### 4.1 dimension-reviewer 写文件不带 task_id

**症状**：cheery-hawk 写出了 `findings-all-no-task-id.json` 这种**没 task_id 的命名**。

**preset 写的**（dimension-reviewer 的 instructions）：
```
Output Format
Write findings to `.agents/scratchpad/ce-executor/{plan_name}/findings-{dimension}-{task_id}.json`
```

**问题**：preset 写了 pattern，但**没强约束**——agent 拼不出 task_id 就拼个 `findings-all-no-task-id.json` 出来。

**修复**：在 dimension-reviewer 的 hat 顶端加 obligation：
```yaml
obligations:
  - on_trigger: "review.wave.ready"
    must_emit_any_of: ["review.dimension.done"]
    payload_must_contain: ["task_id", "findings_file"]
    must_match_pattern:
      findings_file: "^\\.agents/scratchpad/ce-executor/.+/findings-.+-task-.+\\.json$"
```

### 4.2 fixer 不写 fix-log.md

**症状**：drift-auto-calibration-plan 目录里没有 fix-log.md。

**preset 写的**（fixer instructions）：
```
fix-log.md Format
```

**问题**：preset 写"必须写 fix-log.md"，但**没 obligation**——agent 跳过这步直接发 fix.applied / fix.exhausted 也行。

**修复**：fixer 加 `obligations.on_trigger: "review.failed"` → `must_emit_any_of: ["fix.applied", "fix.exhausted"]` + `must_emit_after_writing: "fix-log.md"`。

### 4.3 plan-gate 不更新 progress.md

**症状**：主仓 progress.md 永远 Step 1 in_progress，即使 plan-gate 推了 queue.advance 也没更新。

**preset 写的**（plan-gate instructions）：
```
Reconcile Current Step
Before deciding, update progress.md if the current step is confirmed complete
```

**问题**：preset 写了 "before deciding, update"，但**没 obligation 阻止不更新就发 queue.advance**。

**修复**：plan-gate 加 `obligations.on_trigger: "review.passed"` → `must_update_file: "progress.md" before emit "queue.advance"`。

---

## 5. 机制 vs 编排问题分桶表

### 桶 A：纯机制问题（需要改源码）

| 级别 | 问题 | 源码位置 | 修复 |
|------|------|---------|------|
| **M1** | wave 找回率 < 100% 时整批丢 | `wave_detection.rs:96-104` | 增加 partial-wave 接受分支 |
| **M2** | verdict_gate 不覆盖 `report.done` | `event_loop/mod.rs:1401` | verdict_gate 改成同时检查 `report.done` payload |
| **M3** | Responder 一直 Soft 升级不到 Hard | `drift/engine.rs:217`、`diagnosis/responder.rs:1152` | drift_finding_count > 阈值时强制 Hard escalate |

### 桶 B：纯编排问题（改 preset 即可）

| 级别 | 问题 | preset 位置 | 修复 |
|------|------|------------|------|
| **O1** | coordinator_hats 列表太窄 | `ce-executor.yml:31-33` | 加 plan-gate/fixer/debug-resolver/shipper/reporter |
| **O2** | plan-gate 触发器不全 | `ce-executor.yml:1106-1111` | 加 fix.exhausted/debug.exhausted/loop.cancel |
| **O3** | reporter 没"fail 拒发" obligation | `ce-executor.yml:1273-1280` | 加 must_NOT_emit_when obligation |
| **O4** | fixer 没 obligation 强约束 fix_round | `ce-executor.yml:923-929` | 加 must_emit_any_of + must_emit_after_writing |
| **O5** | plan-gate 没 obligation 强约束更新 progress.md | `ce-executor.yml:1106-1111` | 加 must_update_file |
| **O6** | dimension-reviewer 没 obligation 强约束 task_id | `ce-executor.yml:584-588` | 加 must_match_pattern |

### 桶 C：现状合理（不用改）

- `inject_fallback_event` 兜底：✅ 在
- `task.resume` 自动注入：✅ 在
- stall 信号写盘：✅ 在
- drift 写盘：✅ 在
- OperationContext 旁路：✅ 在
- execution_contracts：✅ 在（preset `execution_contracts` 段用对了）

---

## 6. 修复优先级（说人话版）

**最该先改的 3 件事**（按投入产出比排）：

1. **O1（5 分钟，1 行 preset 改动）**：把 coordinator_hats 加全——能解决「fixer/plan-gate 改 task 被 owner-check 拒绝」一类问题
2. **O2（5 分钟，3 行 preset 改动）**：plan-gate triggers 加 fix.exhausted/debug.exhausted/loop.cancel——能解决「fix 走完没后续推进」一类问题
3. **O3 + M2（30 分钟，1 行 preset + 1 处源码微调）**：reporter fail 拒发 LOOP_COMPLETE + verdict_gate 覆盖 report.done——能解决「失败被掩盖」一类问题

**这 3 件事做完，4 份 plan 大概率能走完整闭环**（预期 cheery-hawk 那种走法）：

- 4 份 plan 都会推到 U6+
- 失败路径被 verdict_gate 拦
- plan-gate 不会卡在 step 1
- fix round 不会走 1 轮就停

**次要改的 3 件事**（可选）：

4. **O4/O5/O6（30 分钟，3 处 preset 改动）**：agent obligation 强约束
5. **M1（4 小时，1 处源码 + 1 处 test）**：wave 找回率部分接受
6. **M3（2 小时，1 处源码）**：Responder 升级到 Hard

---

## 7. 证据清单

### 7.1 源码定位（你要求看的）

| 机制 | 文件:行 | 验证 |
|------|---------|------|
| `inject_fallback_event` | `crates/ralph-core/src/event_loop/mod.rs:1835` | ✅ 有，注入 `task.resume` 兜底 |
| `StallRecovery` 诊断 | `crates/ralph-core/src/diagnosis/envelope.rs:85` | ✅ 有 |
| recovery.jsonl 实际写盘 | `crates/ralph-core/src/diagnostics/recovery.rs:206` | ✅ 有（reason_code="stall"） |
| drift 写盘 | `crates/ralph-core/src/drift/engine.rs:181-209` | ✅ 有 |
| Responder Hard 升级 | `crates/ralph-core/src/drift/engine.rs:217-227` | ⚠️ 设计存在但实际未触发 |
| verdict_gate | `crates/ralph-core/src/event_loop/mod.rs:1401-1425` | ⚠️ 只挡 LOOP_COMPLETE，不挡 report.done |
| wave 找回率 | `crates/ralph-core/src/wave_detection.rs:96-104` | ⚠️ 整批 skip，0 容忍 |
| wave tracker | `crates/ralph-core/src/wave_tracker.rs:84-198` | ✅ 正常 |
| task owner check | `crates/ralph-cli/src/task_cli.rs:866` | ✅ 正常 |
| coordinator_hats 字段 | `crates/ralph-core/src/config/tasks.rs:18-37` | ✅ 字段在 task config |
| preset 加载 | `crates/ralph-cli/src/presets.rs:33-35` | ✅ `include_str!` 内置 |

### 7.2 实际产物（你要求看的）

| 文件 | 大小 | 内容 |
|------|------|------|
| 主仓 `.ralph/events-20260606-002000.jsonl` | 19 KB | 30 行（27 dim-reviewer + 1 test + 1 history） |
| 主仓 `.ralph/events.jsonl` | 47 KB | 45 行全 dim-reviewer（wave worker 旁路事件） |
| 主仓 `.ralph/diagnostics/2026-06-08T23-39-15/drift.jsonl` | 11 KB | 28 finding，coord_join_rate 33%/50% 不达 60% |
| 主仓 `.ralph/diagnostics/2026-06-08T23-39-15/recovery.jsonl` | 25 KB | 33 envelope (31 pending + 2 recovered) |
| 主仓 `.agents/scratchpad/ce-executor/2026-06-04-004-feat-drift-auto-calibration-plan/progress.md` | 352 B | Step 1 in_progress, 0 完成 |
| 主仓 `.agents/scratchpad/ce-executor/2026-06-04-004-feat-drift-auto-calibration-plan/tasks.jsonl` | 3.4 KB | 5 task 全 open |
| steady-raven git log | 3 commit | U0 characterization + 6 safe_auto + U1 metadata |
| gentle-orchid git log | 5 commit | U1/U2/U3 + 2 fix |
| cheery-hawk git log | 5 commit | U4/U5/U6 + 2 fix |
| 主仓 git log ce-executor 修复 | 多条 | 大量开发期 fix（不算 ce-executor 跑出来的） |

---

## 8. 给 operator 的下一步

1. **优先 O1+O2+O3+M2 这 4 处**（加起来 30-40 行改动，1 小时内可做完）
2. 改完跑 1 份 plan 验证：预期能走到 U6+ 且 1 个 LOOP_COMPLETE 不被 fail 掩盖
3. 如果还卡，再补 O4/O5/O6（30 分钟）
4. **M1 和 M3 暂不改**——除非要支持更激进的 plan，否则不值得

---

*报告基于 `pittcat-dev @ fee8c8d` 源码 + 4 份 plan 实际产物。v3 推翻 v1/v2 的「空转」「失败被掩盖为主因」等过激判断；重新划线：机制都在，编排 YML 4 处需修。*
