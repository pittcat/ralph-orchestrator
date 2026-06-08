---
date: 2026-06-08
type: ce-debug
diagnostic-of: 2026-06-05-002-feat-preset-template-versioning-plan-bold-wolf
preset: ce-executor
plan: 2026-06-05-002-feat-preset-template-versioning-plan
subject: 为什么 code review 没执行 / wave 没起来
---

# ce-executor review wave 未启动诊断报告

> 📅 2026-06-08 | 🔖 loop 2026-06-05-002-feat-preset-template-versioning-plan-bold-wolf · plan 2026-06-05-002-feat-preset-template-versioning-plan
>
> 触发问题：本次运行中,`code review` 与 `wave` 链路完全未启动,`review-coordinator` 收到 `work.done` 后**直接 emit `review.passed`(findings_count=0) 把整条 review 链短路掉**,导致 `dimension-reviewer × 9`、`review-synthesizer` 一次都没被激活。

---

## 1. TL;DR — 一句话定位

**`ce-executor.yml` preset 在 `review-coordinator` 上配置了 `publishes: [review.wave.ready, review.passed]` 与 `obligations.must_emit_any_of: [review.wave.ready, review.passed]`,但 `obligations` 块仅做 R4 编译期 lint(保证 topic 出现在 `publishes` 列表),不构成运行期硬阻断;同时 preset L471 "Empty diff handling" 条款被 agent 泛化解读为"小改动 = 跳过 review",加上 L484-491 七个 always-on 维度缺少 MUST 强度——三者叠加,导致 U0/U1/U2/U3/U4 5 次 `work.done` 全部被 `review-coordinator` 走 `review.passed` 短路掉,0 次 `review.wave.ready`、0 次 `review.dimension.done`、0 次 `review-synthesizer` 激活。**

下钻结论:

| 关注点 | 结论 | 证据 |
|---|---|---|
| `code review` 是否执行 | **从未执行** | `events.jsonl` 中 0 次 `review.wave.ready`、0 次 `review.dimension.done`、0 次 `review-synthesizer` 出现 |
| `wave` 是否启动 | **从未启动** | 6/6 次 `review.passed` 全部由 `review-coordinator` 直接 emit;`progress.md` 中 `## Active Wave (empty)` 从未更新 |
| preset 设计 | **存在结构性 gap** | `obligations` 仅 R4 lint,非运行期 gate;L471 "Empty diff" 条款与 L484-491 "always-on dimensions" 优先级不清 |
| agent 行为 | **误读 preset 条款** | U3(400 行)/U4(450 行)commit_count=1 的明显非空 diff,仍 emit `findings_count: 0` 的 `review.passed` |
| Ralph 基座机制 | **本次无未预期行为** | `event_loop` 路由、plan-gate、verdict_gate 全部按预设工作;问题不在底层 |
| task owner 错配 | **暴露 1 处 preset 契约 bug** | U0 task 由 coordinator 建,owner=coordinator;executor 接手想 close 被拒(events.jsonl L3 `work.failed`);preset 配了 `coordinator_hats: [executor, coordinator]` 未在 task owner 解析中生效 |
| U0→U4 进度 | **链路通了,但 review 维度漏** | 6 次 `queue.advance`、6 次 `work.done`、6 次 `review.passed`(全 0 finding)、0 次 wave 启动 |
| 当前所处 step | **U5** | progress.md L5 记录 `Current Step = Step 5 — U5: Integrate with Runtime Contract Without Duplicating It`;但 events 停在 U4 review.passed 后,说明 U5 work.ready 还没发出或被截断 |

---

## 2. 流程还原:预设 vs 实际执行链路

### 2.1 预设(presets/en/ce-executor.yml)期望链路

来源:preset 文件 L1-27 顶部注释 + L148-1214 各 hat 配置。

```text
work.start
   ↓
coordinator                        (triggers: work.start)
   ↓ work.ready
executor                           (triggers: work.ready, queue.advance, work.retry, fix.plan.ready)
   ↓ work.done
review-coordinator                 (triggers: work.done, fix.applied)
   ↓ review.wave.ready             (publishes 第 1 选项,走 wave)
        ↓
        dimension-reviewer × N      (triggers: review.wave.ready; concurrency: 9)
             ↓ review.dimension.done × N
        review-synthesizer         (triggers: review.dimension.done; aggregate: wait_for_all, timeout: 300s)
             ↓ review.passed / review.failed / review.complete
plan-gate                          (triggers: review.passed, review.complete, work.failed)
   ↓ queue.advance
executor                           (下一个 step 的 work.ready 触发器之一: queue.advance)
   ... (循环回 executor)
   ↓ plan.complete
shipper                            (triggers: plan.complete, plan.blocked, debug.exhausted)
   ↓ REVIEW_COMPLETE
reporter                           (triggers: REVIEW_COMPLETE)
   ↓ report.done
   ↓ LOOP_COMPLETE
```

**关键点**:`review-coordinator` 必须 emit 7 个 always-on 维度的 `review.wave.ready`(correctness / testing / maintainability / standards / requirements / agent-native / learnings),除非 diff 真的为空。

### 2.2 实际(events-20260608-100217.jsonl)链路

来源:`.worktrees/2026-06-05-002-feat-preset-template-versioning-plan-bold-wolf/.ralph/events-20260608-100217.jsonl`,共 28 条事件。

```text
work.start                         (loop 启动)
   ↓
coordinator.work.ready             (U0, task_key: ...u0-characterization-tests)
   ↓
executor.work.done                 (U0, commit_count=0, changed_lines=0, 验证即结论)
   ↓
executor.work.failed               (executor cannot close task owned by coordinator)   ← P1 task owner 错配
   ↓
plan-gate.plan.blocked             (reason 同上)
   ↓
shipper.review.complete + REVIEW_COMPLETE  (verdict=fail)
   ↓
reporter.report.done               (awaiting_decision=true)
   ↓
[loop 仍存活,继续]
   ↓
executor.work.done (U0 第二次,无 payload 字段)        ← 兜底 close
   ↓
executor.work.done (U0 第三次,plan_path 补齐)         ← 兜底 close
   ↓
review-coordinator.work.done (U0)                     ← 第一次由 review-coordinator 接管
   ↓
review-coordinator.review.passed  (U0, findings_count=0, pass_or_fail=pass, 缺 fix_round)
   ↓
plan-gate.plan.blocked             (reason: review.passed 缺 fix_round)
   ↓
shipper.REVIEW_COMPLETE            (verdict=fail)
   ↓
reporter.report.done
   ↓
[loop 继续]
   ↓
ralph.work.done (U0 4 次重发,逐渐补字段)
   ↓
review-coordinator.work.done (U0)
   ↓
review-coordinator.review.passed (U0, fix_round=0, pass)             ← 第一次完整 schema
   ↓
plan-gate.queue.advance (U0 → U1)
   ↓
executor.work.done (U1)
   ↓
review-coordinator.review.passed (U1, findings_count=0, fix_round=0) ← 走 review.passed
   ↓
plan-gate.queue.advance (U1 → U2)
   ↓
executor.work.done (U2)
   ↓
review-coordinator.review.passed (U2, findings_count=0, fix_round=0) ← 走 review.passed
   ↓
plan-gate.queue.advance (U2 → U3)
   ↓
executor.work.done (U3, commit_count=1, changed_lines=400)
   ↓
review-coordinator.review.passed (U3, findings_count=0, fix_round=0) ← 应走 wave 但走 passed
   ↓
plan-gate.queue.advance (U3 → U4)
   ↓
executor.work.done (U4, commit_count=1, changed_lines=450)
   ↓
review-coordinator.review.passed (U4, findings_count=0, fix_round=0) ← 应走 wave 但走 passed
   ↓
plan-gate.queue.advance (U4 → U5)
   ↓
[事件流截止,U5 还没启动新 work.ready]
```

### 2.3 链路对比图(关键差异点高亮)

| 阶段 | 预设期望 | 实际发生 | 差异 |
|---|---|---|---|
| U0 review | review.wave.ready × 7 (or review.passed 兜底) | 多次 review.passed(其中 2 次缺 fix_round 被 plan.blocked) | ⚠️ schema 漏字段 |
| U1 review | review.wave.ready × 7 | 直接 review.passed(findings=0) | ❌ wave 跳过 |
| U2 review | review.wave.ready × 7 | 直接 review.passed(findings=0) | ❌ wave 跳过 |
| U3 review(400 行) | review.wave.ready × 7 | 直接 review.passed(findings=0) | ❌ wave 跳过 |
| U4 review(450 行) | review.wave.ready × 7 | 直接 review.passed(findings=0) | ❌ wave 跳过 |
| U5 | queue.advance 已发,work.ready 应到 | 事件流截止,无新 work.ready | ⚠️ 数据不足 |

---

## 3. 证据清单

### 3.1 文件级证据

| # | 文件 | 关键内容 |
|---|---|---|
| 1 | `presets/en/ce-executor.yml:432-447` | review-coordinator 配置:`triggers: [work.done, fix.applied]`、`publishes: [review.wave.ready, review.passed]`、带 `obligations.must_emit_any_of: [review.wave.ready, review.passed]` |
| 2 | `presets/en/ce-executor.yml:471` | "Empty diff handling" 条款:**if diff is empty, publish `review.passed`**(缺 MUST 强度) |
| 3 | `presets/en/ce-executor.yml:484-491` | 7 个 always-on 维度:**correctness / testing / maintainability / standards / requirements / agent-native / learnings**(未用 MUST 强度) |
| 4 | `presets/en/ce-executor.yml:514-525` | "Wave Emission" 段:`Use ralph wave emit for each selected dimension` |
| 5 | `presets/en/ce-executor.yml:29-33` | 顶层 `tasks.coordinator_hats: [executor, coordinator]` —— 已配但运行期未生效 |
| 6 | `.ralph/events-20260608-100217.jsonl` | 28 条事件,主题统计:work.done×8、review.passed×6、queue.advance×5、REVIEW_COMPLETE×2、report.done×2、plan.blocked×2、work.ready×1、work.failed×1、review.complete×1 |
| 7 | `.ralph/agent/tasks.jsonl` | 6 个 task 全部 `status=closed`;U0 `owner_hat_id=coordinator`、U1-U4 `owner_hat_id=executor` |
| 8 | `.ralph/agent/progress.md:7` | `## Active Wave (empty)` —— wave tracker 从未记录任何批次 |
| 9 | `.ralph/agent/plan.md:1-107` | 计划含 8 个 Step (U0-U7),U5 描述"Integrate with Runtime Contract" |
| 10 | `crates/ralph-core/src/runtime_contract.rs:842-870` | obligations **只做 R4 编译期 lint**,保证 `must_emit_any_of` topic 在 `publishes` 列表;**无运行期阻断** |
| 11 | `crates/ralph-core/src/execution_contract.rs` | ExecutionContract 只覆盖 `work.done` 字段(plan_name/plan_path/task_id/task_key/step 等),不覆盖 review-* 事件 |
| 12 | `crates/ralph-core/src/preset_lint.rs:264-272` | `obligations[].must_emit_any_of` 校验器;**仅在 lint 时跑** |
| 13 | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:241` | `execute_wave` 是 wave spawn 入口;`review.wave.ready` 没出现 = wave 系统从未被通知 |
| 14 | `crates/ralph-core/src/wave_prompt.rs:89` | wave worker 指令:`DO NOT use ralph wave emit — nested wave dispatch is prohibited` —— 说明 `ralph wave emit` 是 review-coordinator 唯一可走的 wave 路径 |

### 3.2 事件级证据(关键 payload 摘录)

**events.jsonl L3 (executor.work.failed)**:
```json
{"hat":"executor","payload":"executor cannot close task owned by coordinator; preset needs coordinator_hats config or task should be created by executor","topic":"work.failed","ts":"2026-06-08T10:42:31.197Z"}
```

**events.jsonl L4 (plan-gate.plan.blocked)**:
```json
{"hat":"plan-gate","payload":{"plan_name":"...","reason":"executor cannot close task owned by coordinator; preset needs coordinator_hats config or task should be created by executor","step":"U0","task_id":"task-1780913040-7b36","task_key":"...:step-01:u0-characterization-tests"},"topic":"plan.blocked","ts":"2026-06-08T10:43:42.571Z"}
```

**events.jsonl L9 (review-coordinator.work.done, U0)** —— 由 review-coordinator 接管:
```json
{"hat":"review-coordinator","payload":{"plan_name":"...","plan_path":"...","step":"U0","task_id":"task-1780913040-7b36","task_key":"...:u0-characterization-tests"},"topic":"work.done","ts":"2026-06-08T11:10:20.887Z"}
```

**events.jsonl L10 (review-coordinator.review.passed, U0 首次)** —— 缺 fix_round:
```json
{"hat":"review-coordinator","payload":{"findings_count":0,"pass_or_fail":"pass","plan_name":"...","residual_findings_summary":"U0 characterization tests: no review required at this time; task closed and work.done emitted","step":"U0","task_id":"...","task_key":"...","verdict":"pass"},"topic":"review.passed","ts":"2026-06-08T11:11:21.518Z"}
```

**events.jsonl L11 (plan-gate.plan.blocked, U0)** —— 因 fix_round 缺:
```json
{"hat":"plan-gate","payload":{"plan_name":"...","reason":"review.passed missing required field: fix_round","step":"U0","task_id":"...","task_key":"..."},"topic":"plan.blocked","ts":"2026-06-08T11:12:46.368Z"}
```

**events.jsonl L14 (review-coordinator.review.passed, U0 补齐版)**:
```json
{"hat":"review-coordinator","payload":{"findings_count":0,"fix_round":0,"pass_or_fail":"pass","plan_name":"...","residual_findings_summary":"U0 characterization tests pass (115 preset tests). No review required; task closed and work.done emitted. Build passes, clippy clean.","step":"U0","task_id":"...","task_key":"...","verdict":"pass"},"topic":"review.passed","ts":"2026-06-08T11:17:32.304Z"}
```

**events.jsonl L18 (U3, 应走 wave 实际走 passed)**:
```json
{"hat":"executor","payload":{"changed_lines":400,"commit_count":1,"plan_name":"...","plan_path":"...","step":"U3","task_id":"task-1780920257-7a27","task_key":"...:step-01:u3-preset-list-show-new-cli"},"topic":"work.done","ts":"2026-06-08T12:16:47.263Z"}
```
↓ 6 秒后 ↓
```json
{"hat":"review-coordinator","payload":{"findings_count":0,"fix_round":0,"pass_or_fail":"pass","plan_name":"...","residual_findings_summary":"U3 complete: preset list/show/new CLI implemented with 15 acceptance tests passing. All 83 total preset tests pass. Build succeeds. Ready for U4 (diff/upgrade preview).","step":"U3","task_id":"...","task_key":"...","verdict":"pass"},"topic":"review.passed","ts":"2026-06-08T12:18:25.421Z"}
```
**关键观察**:executor `commit_count=1, changed_lines=400` 明显非空 diff,review-coordinator 6 秒后直接 emit passed,**未尝试调用 `ralph wave emit` 7 个维度**。

### 3.3 task.jsonl 证据(节选)

```jsonl
{"id":"task-1780913040-7b36","title":"U0: ...","status":"closed","owner_hat_id":"coordinator","closed":"2026-06-08T10:54:38.133Z"}
{"id":"task-1780918080-d2ab","title":"U1: ...","status":"closed","owner_hat_id":"executor","started":"2026-06-08T11:28:03Z","closed":"2026-06-08T11:28:08Z"}
{"id":"task-1780918629-4e14","title":"U2: ...","status":"closed","owner_hat_id":"executor","closed":"2026-06-08T11:57:29Z"}
{"id":"task-1780920257-7a27","title":"U3: ...","status":"closed","owner_hat_id":"executor","closed":"2026-06-08T12:16:30Z"}
{"id":"task-1780921191-26fc","title":"U4: ...","status":"closed","owner_hat_id":"executor","closed":"2026-06-08T12:35:47Z"}
```

**关键观察**:U0 owner=coordinator,U1-U4 owner=executor。U0 那次 owner 错配暴露 `coordinator_hats` 配而未生效的契约 bug;U1-U4 owner 都是 executor,说明 `ralph tools task ensure` 在 coordinator 建 task 时把 owner 设为 coordinator,而 queue.advance 后 executor 被默认设为新 task 的 owner。

---

## 4. 对账分析

### 4.1 preset 配置是否符合自洽?

| 检查项 | 结论 | 证据 |
|---|---|---|
| review-coordinator 双路径 | ✅ 配了 `publishes: [review.wave.ready, review.passed]` | L436 |
| review-coordinator obligations | ✅ 配了 `must_emit_any_of: [review.wave.ready, review.passed]`(L443-447) | L443-447 |
| obligations 是否有运行期执行 | ❌ 仅 R4 lint(编译期校验) | `runtime_contract.rs:842-870` |
| empty diff 条款优先级 | ⚠️ 与 always-on 维度表优先级不清 | L471 vs L484-491 |
| always-on 维度 MUST 强度 | ❌ 用了"always emit"而非 MUST | L484 |
| review.wave.ready schema | ✅ required_fields 完整 | L89-91 |
| review.passed schema | ⚠️ 必含 fix_round,但 L1 失败证明 review-coordinator 没看 schema | L95-97,events L10 |
| review.dimension.done schema | ✅ 完整 | L92-94 |
| review-synthesizer aggregate | ✅ wait_for_all + timeout 300s | L728-733 |
| `ralph wave emit` 路径 | ✅ CLI 在 PATH | `crates/ralph-cli/src/wave.rs` |
| review-synthesizer 触发条件 | ✅ `triggers: [review.dimension.done]` | L726 |
| dimension-reviewer 触发条件 | ✅ `triggers: [review.wave.ready]` | L536 |

**核心自洽性结论**:preset 在**配置层面**完整,但**运行期 enforcement 缺失** + **优先级文档不清** 导致 agent 选择性走 `review.passed` 路径且不被任何 gate 阻断。

### 4.2 review 链是否按 preset 闭环?

| 闭环点 | 期望 | 实际 | 是否闭环 |
|---|---|---|---|
| work.done → review-coordinator | 必触发 | 6/6 触发 | ✅ |
| review-coordinator → wave 或 passed | 二选一 | 6/6 走 passed | ⚠️ 形式闭环,语义走偏 |
| wave → dimension-reviewer | wave 启动才触发 | 0 次 | ❌ |
| dimension → synthesizer | wave 启动才触发 | 0 次 | ❌ |
| synthesizer → review.passed/failed | wave 启动才触发 | 0 次 | ❌ |
| plan-gate → queue.advance | review.passed 后推进 | 5 次推进 | ✅ |
| plan-gate → plan.complete | 所有 step 完成 | 未达(U5 没启动) | ❌(数据不足) |
| shipper → REVIEW_COMPLETE | plan.complete/blocked 后 | 2 次触发(都因 plan.blocked) | ✅ |
| reporter → report.done | REVIEW_COMPLETE 后 | 2 次 | ✅ |
| reporter → LOOP_COMPLETE | report.done 后 | 未达 | ❌(数据不足) |

### 4.3 task / progress / findings / fix-log 一致性

| 文件 | 期望状态 | 实际状态 | 一致性 |
|---|---|---|---|
| `tasks.jsonl` | U0-U4 全部 closed | 5/5 closed | ✅ |
| `progress.md` Current Step | 当前 step | U5(但 events 流截止,未实际启动 U5) | ⚠️ |
| `progress.md` Active Wave | wave 启动后非空 | 空(从未启动) | ✅(空是真实状态) |
| `progress.md` Completed Steps | 5 个 step 全列 | 5 个(U0-U4) | ✅ |
| `findings.md` | wave 跑后生成 | **不存在** | ❌(链路没启动) |
| `fix-log.md` | review.failed 后生成 | **不存在** | ❌(链路没启动) |
| `scratchpad.md` 20KB | 应有 wave 决策记录 | 无 wave 决策记录 | ❌ |

---

## 5. 因果链(从根因到症状,无 gap)

### 5.1 Root Cause 层 0:preset 文档优先级与运行期 enforcement 双重缺失

`presets/en/ce-executor.yml` L432-531 段:
- L436 `publishes` 列了 2 个 topic,agent 完全有权只选其中 1 个
- L443-447 `obligations` 块形式上声明"必须 emit 之一",但 `crates/ralph-core/src/runtime_contract.rs:842-870` 自承"validates",即只校验"topic 在 publishes 列表里",**不校验"运行时是否真的 emit 了"**
- L471 "Empty diff handling" 在 L484-491 "always-on dimensions" 之前,且 L471 用了 MUST 强度("if diff is empty, publish `review.passed`"),L484-491 用了弱强度("always emit wave for these")
- L484-491 的"always-on"未明示"MUST 总是 emit,除非满足 L471 的空 diff 强约束"

**因果链**:
1. preset 文档不强制 wave 优先级 → agent 不知道 wave 是默认路径、passed 是兜底
2. preset obligations 无运行期 enforcement → agent 选哪条路径都合规
3. → 5 次 `work.done` 后,review-coordinator 全部选 `review.passed`,`findings_count: 0`,`fix_round: 0`
4. → dimension-reviewer / review-synthesizer 0 次激活
5. → 整个 review 链短路,fixer / debug-resolver / plan.blocked-with-residual 链也未触发
6. → U0-U4 全靠"测试通过"+"build clean"自证,无任何独立 review 保障

### 5.2 Root Cause 层 1:agent 误读 "Empty diff handling"

`presets/en/ce-executor.yml:471`:
> **Empty diff handling**: if diff is empty, publish `review.passed` with `plan_name`, `findings_count: 0`, `task_id`, `task_key`, `step`

**被 agent 误读为**:"tests 全部通过 + 改动小 = 跳过 review"。

**实际应为**:"**仅当 `git diff` 输出空 AND `git ls-files --others --exclude-standard` 空 AND work.done payload `commit_count==0` AND `changed_lines==0` 时**,才走 `review.passed` 兜底"。

**因果链**:
- U0 commit_count=0, changed_lines=0 → 走 passed(理论上可)
- U1 commit_count 未明列,但 U1 description 提到新建文件 → 也走 passed
- U2 commit_count 未明列,55 tests passing → 也走 passed
- U3 commit_count=1, changed_lines=400 → **应走 wave,但走了 passed**
- U4 commit_count=1, changed_lines=450 → **应走 wave,但走了 passed**

### 5.3 Root Cause 层 2:task owner 错配暴露 coordinator_hats 契约未生效

events.jsonl L3:
```json
{"hat":"executor","payload":"executor cannot close task owned by coordinator; preset needs coordinator_hats config or task should be created by executor","topic":"work.failed","ts":"2026-06-08T10:42:31.197Z"}
```

`presets/en/ce-executor.yml:29-33` 配了:
```yaml
tasks:
  enabled: true
  coordinator_hats:
    - executor
    - coordinator
```

但 `events.jsonl L3` 报"preset needs coordinator_hats config"——**说明 `ralph tools task ensure` 的 owner 解析路径没读 `coordinator_hats`**,要么:
- (a) `coordinator_hats` 解析的是"建任务时哪些 hat 可以建",**没用来判定 close 权限**
- (b) close 权限严格等于"owner_hat_id 与触发 hat 完全相同"
- (c) 配了但 task_store 字段读取顺序错了

**因果链**:
1. coordinator 建 U0 task,owner=coordinator
2. executor 触发 work.ready,接手 U0
3. executor 想 close U0,但 owner=coordinator,被拒
4. → events L3 work.failed → plan.blocked → shipper 兜底 fail
5. → loop 试图兜底:ralph hat 自己发 work.done(L7, L8)→ 让 review-coordinator 再接管(L9)
6. → review-coordinator 不在 task owner 检查路径里,成功 close
7. → 4 次重发,最后 U0 才被 close(2026-06-08T10:54:38Z),U1 才开工

### 5.4 影响范围

- **直接损失**:U0-U4 5 个 step 全部缺独立 review 保障,fixer / debug-resolver / wave 维度全部未激活
- **机会成本**:U3 400 行新 CLI、U4 450 行新 CLI,**没有任何 dimension-reviewer 看过**,没识别出可能的设计缺陷、错误处理漏洞、API 契约问题
- **对项目的影响**:U5 是"Integrate with Runtime Contract Without Duplicating It",U6 是"Builtin Authoring Maintenance Guard",U7 是"Documentation and Authoring Guide"——后 3 步会基于未 review 的 U3/U4 继续推进,**风险累积**
- **时间成本**:本次运行从 10:04 (U0 work.ready) 到 12:38 (U4 queue.advance),约 2.5 小时;若 wave 启动,fixer 可能要花更多时间 review + fix,但**单步质量更高**

---

## 6. 问题归因表

| 级别 | 类别 | 问题 | 证据 | 修复点 |
|---|---|---|---|---|
| **P0** | preset design | `obligations` 只做 R4 编译期 lint,**没在循环里强制 hat emit 至少一个 must_emit_any_of topic** | `runtime_contract.rs:842-870` 注释自承"validates" | 给 `event_policy` 加 `enforce_obligation` 模式:若 hat 触发了 `on_trigger` 列表中的某 trigger 且没有 default_publishes 兜底,必须看到对应 `must_emit_any_of` 之一的事件 |
| **P0** | preset design | review-coordinator 误读"Empty diff"为"小改动 = 跳过 review" | events.jsonl: 6/6 全部直接 emit `review.passed`(findings_count=0, fix_round=0),且 U3/U4 commit_count=1, changed_lines=400/450 | preset L515 之后加显式规则:`NON-EMPTY diff (commit_count>=1 OR changed_lines>=50 OR any changed file) MUST emit ralph wave emit for 7 always-on dimensions`;并把 empty-diff 条款限定到"git diff 输出空 AND git ls-files --others --exclude-standard 空" |
| **P0** | preset design | "Always-on dimensions" 表和 "Empty diff" 条款优先级不清 | L471 在 L484-491 之前;L471 用了 MUST 强度,L484-491 没用 MUST 强度 | L484-491 改用 **MUST** 强度;明示"only if `git diff` AND `git ls-files --others --exclude-standard` are both empty AND work.done.commit_count==0 AND work.done.changed_lines==0" 才走 L471 路径 |
| **P1** | Ralph 基座 | `event_policy.schemas` 不校验 review-coordinator 到底发哪个 `must_emit_any_of` 路径 | `event_policy.schemas` 只校验单个事件 payload 的 `required_fields`,**不知道 hat 是不是漏发了另一条合规路径** | 加一个 `hat_emit_obligation` 校验层:每个 hat 在 `on_trigger` 命中后,看后续 N events 内必须出现该 trigger 对应 `must_emit_any_of` 之一 |
| **P1** | Ralph 基座 | 任务 owner 错配:U0 由 coordinator 建 task 但 executor 接手,触发"cannot close"事故 | `tasks.jsonl:1` owner=coordinator;`executor` preset 缺 `coordinator_hats` 配置 | preset L31-33 已配 `coordinator_hats: [executor, coordinator]`,但**实际是 events L3 的 work.failed 才暴露**——说明 runtime 用了"建任务时 hat"判定 owner,preset 配置未生效;查 `crates/ralph-core/src/task_store.rs` 的 owner_hat_id 解析路径是否读 `coordinator_hats` |
| **P1** | preset design | review.passed schema 必含 fix_round,但 review-coordinator L10 漏发 | events.jsonl L10 缺 fix_round → L11 plan.blocked | preset review-coordinator instructions 加"必须先看 event_policy.schemas.review.passed.required_fields";enforce_obligation 模式同步默认补 fix_round=0 |
| **P2** | agent 行为 | 同一 review-coordinator 在 5 次中输出几乎一样的"no review required at this time" 文案,**未尝试调用 `ralph wave emit`** | events.jsonl U1-U4 4 次 review.passed 文本结构高度相似;没有任何 worker spawn 记录 | preset L525 补强:"MUST run `ralph wave emit` CLI for each dimension; if `ralph wave emit` is not in PATH, emit `work.failed` with `reason: wave tool unavailable`" |
| **P2** | 产物 | progress.md / findings.md 不存在 | `progress.md` 在但 `findings.md` 未生成(review 链未启动当然没 findings) | 链路修好后自然生成 |
| **P2** | 产物 | scratchpad 20KB 但**没记 wave spawn / review 决策依据** | scratchpad.md 没引用 dimension 选择结果 | preset L483 显式要求 review-coordinator 把"selected_dimensions[]"写到 `.agents/scratchpad/ce-executor/{plan_name}/review-coordinator-log.md` |
| **P2** | 验证 | U5 已 queue.advance 但 events 截止,**U5 实际是否启动未知** | progress.md 标 U5,events 流无 U5 work.ready | 本次不修,但应让 reporter 在 U5 work.ready 缺失时 emit `loop.stale` |

---

## 7. 修复建议

### 7.1 preset 修复(优先:P0)

**`presets/en/ce-executor.yml` L460-531 改写**:

```yaml
### Scope Detection (REVISED 2026-06-08)
- ... (原有 diff 命令,保留)

### Dimension Selection (REVISED 2026-06-08)
**Hard rule**: 只要下列任一条件成立,就必须 emit 7 always-on 维度的 wave:
- `git diff <base>` 非空,或
- `git ls-files --others --exclude-standard` 非空,或
- work.done payload 的 `commit_count >= 1`,或
- work.done payload 的 `changed_lines >= 50`

**Empty diff handling**(优先级低于上面的 hard rule):
- 仅当上述 4 个条件全部不成立时,才允许 publish `review.passed`
- 必须在 payload 写明 `skip_reason: "empty_diff"` 让 plan-gate 能审计
- 仍要包含 `fix_round: 0` 字段(schema 已要求)

**Always-on dimensions**(满足 hard rule 时 MUST emit 7 个 ralph wave emit):
- correctness / testing / maintainability / standards / requirements / agent-native / learnings
```

**L443-447 obligations 改写**(让"必须 emit 之一"语义更紧):
```yaml
obligations:
  - on_trigger: "work.done"
    must_emit_any_of: ["review.wave.ready", "review.passed"]
    # 2026-06-08: review.passed 只在 hard rule 4 条件全假时才允许
    # 任何不满足 must_emit_any_of 的 iteration = event_policy 阻断 + diagnostic.stall
    audit_field: "skip_reason"   # review.passed 必须带此字段解释跳过原因
  - on_trigger: "fix.applied"
    must_emit_any_of: ["review.wave.ready", "review.passed"]
    audit_field: "skip_reason"
```

### 7.2 Ralph Loop 基座修复(优先:P1)

**`crates/ralph-core/src/event_policy.rs`** 新增 obligation enforcement:
- 引入 `HatObligationEnforcer`:跟踪每个 hat 的 `on_trigger` 命中 → 等待 `must_emit_any_of` 之一出现
- 等待窗口:当前 iteration 内 + 下一 iteration 开头(避免误杀)
- 缺失时:在 orchestration.jsonl 写 `envelope_source: workflow_guard, outcome: failed` 并 emit `loop.cancel`(让 reporter 把它当 P0 报)

**`crates/ralph-core/src/execution_contract.rs`** 扩展:
- 新增 `review.*` 事件 contract:`review.passed` 必须含 `findings_count` + `fix_round` + `skip_reason`(optional)
- `review.wave.ready` 必须含 7 维度(如果 hard rule 命中)

**`crates/ralph-core/src/task_store.rs`** 检查 `coordinator_hats` 是否真在 `ensure` 路径里被读:
- 路径:`ralph tools task ensure` → `task_definition::Task::owner_hat_id` 默认值
- 修复方向:`coordinator_hats` 列表里的 hat 应对其创建的 task 拥有 close 权限(即建任务者 = close 授权者,而非触发 work.ready 的 hat)

### 7.3 产物改进(优先:P2)

- `progress.md` 在 wave 启动时自动追加 `## Active Wave\n- wave_id: <id>, dimensions: [correctness, testing, ...], step: U3`
- `.agents/scratchpad/ce-executor/{plan_name}/review-coordinator-log.md` 由 review-coordinator 每次写一行:`{ts} {trigger} → selected={} skipped_reason={}`
- 同样的 log 文件供后续 loop 做"为什么这个 U 跳过 review"的回溯

### 7.4 优先级与工时

| 修复项 | 优先级 | 估时 | 影响 |
|---|---|---|---|
| A1: preset L471/484-491 优先级改写 + hard rule 4 条件 | **P0** | 30 min | 立即让 wave 链路 5/5 次都能启动 |
| A2: obligations 加 audit_field `skip_reason` | **P0** | 15 min | 让"跳过 review"成为可审计信号 |
| B1: event_policy 加 `HatObligationEnforcer` | **P1** | 2 h | 给"不满足 obligations"硬阻断,杜绝同类短路 |
| C1: task owner 解析支持 `coordinator_hats` | **P1** | 1 h | 解决 events.jsonl:3 那次 work.failed |
| B2: execution_contract 扩展到 `review.*` | **P2** | 1 h | schema 补完 |
| D1: wave tracker 写 progress.md | **P2** | 30 min | 可观测性 |

**总修复工时:约 5.5 小时,可分 2 个 commit 落地**(commit 1 = A+C, commit 2 = B+D)。

---

## 8. 风险与未确认事项

### 8.1 未确认事项

| 事项 | 风险 | 建议 |
|---|---|---|
| U5 work.ready 是否真的未发出?还是 events 流被人为截断? | 若是截断则根因不同 | 跑 `ralph diagnose --session latest` 看 `drift.jsonl` / `recovery.jsonl` |
| U3/U4 commit_count=1 之后,fix.applied / fix.exhausted / debug-resolver 链是否真没机会? | 若有则需要看 review-coordinator 何时被 fix.applied 触发 | grep `fix.applied` 在 events.jsonl 是否 0 次 |
| preset "ce-executor-wave" 是否也是同样问题? | wave-review 是它的卖点,应单独验证 | 跑一次 `ce-executor-wave` 同样诊断 |
| Loop 仍存活还是已死? | 若是 LOOP_COMPLETE 已发,本次 run 终止 | 查 `.ralph/loops.json` 与最近 log |

### 8.2 长期改进方向

1. **B4 (drift 监测)补强**:U5 drift detector 应当能在"wave 启动率 0"时报警,纳入"health score"指标
2. **PRESET 编译期 lint 升级**:`preset_lint.rs` 应对 `obligations` 块加语义检查:若 `must_emit_any_of` 有 ≥2 个 topic 且包含 `*.passed`,提示风险"agent 可能走 passed 路径"
3. **多 preset 横向对比**:抽取 ce-executor / ce-executor-wave / code-assist / pdd-to-code-assist 的 `review-coordinator` 等价 hat,看 obligations 设计是否一致
4. **observability 补完**:`recovery.jsonl` 当前没看到与 review 短路相关的 envelope,应让 `HatObligationEnforcer` 失败时落 `envelope_source: workflow_guard`

---

## 9. 附录:文件索引

- 主报告:`docs/report/2026-06-08-ce-executor-review-wave-not-firing-diagnosis.md`(本文件)
- 事件流:`.worktrees/2026-06-05-002-feat-preset-template-versioning-plan-bold-wolf/.ralph/events-20260608-100217.jsonl`
- agent 产物:同上 `.ralph/agent/{context.md,plan.md,progress.md,scratchpad.md,tasks.jsonl}`
- preset 源:`presets/en/ce-executor.yml`
- 关键源码:
  - `crates/ralph-core/src/runtime_contract.rs:842-870`(obligations lint 范围)
  - `crates/ralph-core/src/preset_lint.rs:264-272`(obligations 编译期校验)
  - `crates/ralph-core/src/execution_contract.rs`(work.done contract)
  - `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:241`(wave spawn 入口)
  - `crates/ralph-core/src/wave_prompt.rs:89`(nested wave 禁止条款)
  - `crates/ralph-core/src/task_store.rs`(owner_hat_id 解析路径,需查)
  - `crates/ralph-cli/src/wave.rs:50`(wave emit CLI 入口)
