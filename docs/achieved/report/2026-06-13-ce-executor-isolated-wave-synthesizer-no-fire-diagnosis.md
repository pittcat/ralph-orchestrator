---
date: 2026-06-13
type: ce-debug
diagnostic-of: 2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan
preset: ce-executor-isolated
plan: 2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan
subject: wave 8/8 完成后 review-synthesizer 不 fire + 多处编排/产物偏离
---

# ce-executor-isolated 链路诊断报告（loop 2026-06-10-003-...-neat-elm）

> 📅 2026-06-13 | 🔖 loop `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-neat-elm`（PID 48392, started 2026-06-13T01:22:30+08:00）| preset `ce-executor-isolated` | plan `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md`

---

## 1. TL;DR — 一句话定位

**wave `w-18b880c82d797fd8-33418-0` 完成 8/8 合并后,`wave::io` 把 8 个 `review.dimension.done` 投回 bus 时被 origin guard 标 `hat=review-coordinator`,preset `topic_deny_rules` 不允许 `review-coordinator → review.dimension.done`,8 次 dropped,synthesizer 的 `aggregate.wait_for_all` 永远凑不齐 8/8,永不 fire**。

叠加 4 项二级偏离(均在同一次 loop 内,且全部独立可证):

| 关注点 | 结论 | 证据(绝对路径) |
|---|---|---|
| **wave → synthesizer 链路** | **断** | `/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-neat-elm/.ralph/diagnostics/logs/ralph-2026-06-13T09-22-30-798-48392.log:38-45`(8 次 `Isolated mode: event out of hat scope — dropping hat=review-coordinator topic=review.dimension.done`)|
| **executor 越权 emit build.done** | **是** | 同 log L13-18(6 次 `hat=executor topic=build.done` dropped);preset 显式 deny `{hat_id: executor, topic: build.done}` |
| **handoff dispatch 两次 timeout** | **是** | log L12(`work.ready → executor`)、L25(`work.done → review-coordinator`) |
| **scratchpad human guidance 重复** | **是** | `/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-neat-elm/.ralph/agent/scratchpad.md` 同名重复 2-3 次 |
| **progress.md 状态错位** | **是** | `/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-neat-elm/.agents/scratchpad/ce-executor/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan/progress.md:9` 写 `Active Wave: 空` 但 wave 实际跑了 8 个 worker |
| **diagnostics U7 全空** | **是** | 主仓 `/Users/pittcat/Dev/Rust/ralph-orchestrator/.ralph/diagnostics/2026-06-13T09-22-30/{recovery,drift}.jsonl` 0 行;worktree 同 session 也 0 行 |
| **preset 编排 / Ralph 基座 / agent** | **三者叠加** | 见 §5 归因表 |

---

## 2. 流程还原:预设 vs 实际

### 2.1 预设(`presets/en/ce-executor-isolated.yml`)期望链路

```
work.start (loop 启动)
   ↓
coordinator                              (triggers: work.start)
   ↓ work.ready
executor                                 (triggers: work.ready, fix.plan.ready)
   ↓ work.done
review-coordinator                       (triggers: work.done, fix.applied)
   ↓ review.wave.ready (8 dims 一次性 wave emit)
        ↓
        dimension-reviewer × 8            (concurrency: 9; wave_id w-...)
             ↓ review.dimension.done × 8
        review-synthesizer               (aggregate: wait_for_all timeout 300s)
             ↓ review.passed / failed / complete / plan.blocked
plan-gate                                (triggers: review.passed, review.complete, work.failed, queue.advance, loop.cancel)
   ↓ queue.advance + work.ready (dual-publish, WAC-U4)
executor                                 (下一步)
   ... 循环
   ↓ plan.complete
shipper → REVIEW_COMPLETE → reporter → report.done → LOOP_COMPLETE
```

**关键约束**(preset 内联规则):
- `event_policy.topic_deny_rules` 显式 deny:`{executor, build.done}` 与 `{ralph, review.wave.ready/review.passed/work.ready/queue.advance/plan.complete/plan.blocked}`
- wave 必须一次性 emit 全部 8 维度(否则 wave_total=1,N 个独立 wave 串行)
- `idempotency_key = ce-review:{plan_name}:{task_id}:{step}:round-{N}`(必填)
- `synthesizer` 必须先做 U6 Completeness Check:对比 `wave_id` 的 `received` vs `wave_total`,不足时 emit `plan.blocked` 而非伪造 verdict
- `review.passed` 必须在 `aggregator.timeout` 之前不能由 `synthesizer` emit

### 2.2 实际(`events-20260613-012231.jsonl`,24 条)链路

```text
work.start (loop_started)                            01:22:31
   ↓
coordinator → work.ready                             01:27:05  (R6, R8; step-01; task-1781313966-6e84)
   ↓ handoff timeout (17m 7s)                        01:44:12  [log L12] task.resume → executor
   ↓
executor × 6 build.done dropped                      01:33:23~01:38:06  [log L13-18] out of hat scope
   ↓
executor → work.done                                 01:43:41  (commit 848043a, 81 lines, 9 files)
   ↓ handoff timeout (4m 20s)                        01:48:01  [log L25] task.resume → review-coordinator
   ↓
review-coordinator → review.wave.ready × 8           01:47:29  (wave_id w-18b880c82d797fd8-33418-0, wave_total=8)
   ↓ wave dispatch 8 workers (concurrency=9)         01:48:01~01:56:00  [log L26-37]
   ↓
dimension-reviewer × 8 → review.dimension.done × 8   01:50:22~01:55:50  (events.jsonl L17-24;全部 hat=dimension-reviewer,全部落 events.jsonl)
   ↓
   同步:同 8 个 done 走 bus re-publish 时             01:56:00  [log L38-45]
        8 次 hat=review-coordinator dropped
   ↓
   aggregator 永远 0/8 (8 个 done 在 bus 上被吞,aggregator 看不见)
   ↓
review-synthesizer 永不 fire
   ↓
plan-gate 永不 fire
   ↓
loop 死锁在等待 synthesizer (current-events 未推进;current-loop-id 仍指向 active loop)
```

**关键偏离**:events.jsonl 显示 `hat=dimension-reviewer` 的 8 个 done **已落盘**(说明 agent emit 路径成功),但 log L38-45 显示**回灌 bus 时 hat 被改成 review-coordinator**(8 次 dropped)。一个事件两个 hat —— origin guard 用 bus 端,JSONL 写入用 emit 端,两者不冲突。

---

## 3. 证据清单(全部绝对路径)

### 3.1 主仓 `/Users/pittcat/Dev/Rust/ralph-orchestrator/.ralph/`

| 文件 | 内容 | 状态 |
|---|---|---|
| `loops.json` | 仅 1 个 active loop 注册(`2026-06-10-003-...-neat-elm`, PID 48392, started 2026-06-13T01:22:30, worktree_path + workspace 指向 worktree 绝对路径) | OK |
| `agent/` | 空目录 | **异常**:agent memory/tasks 应在主仓还是 worktree?worktree 才有内容 |
| `diagnostics/2026-06-13T{08-25-17,08-26-11,08-53-42,08-54-37,09-12-01,09-15-59,09-22-30}/` | 7 个 session,每个都只有 trace/recovery/drift 三个 0 行 jsonl(0 / 0 / 4-5 行) | **异常** U7 runtime diagnosis 全空 |
| `diagnostics/logs/ralph-*.log` | 5 个 log 各 4 行(只记录 loop spawn 信息,无 hat 激活/事件流) | **异常** log 内容与 worktree 不同 |

### 3.2 worktree `/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-neat-elm/.ralph/`

| 文件 | 行/大小 | 关键发现 |
|---|---|---|
| `events-20260613-012231.jsonl` | 24 行 | 1 个 work.ready(01:27:05)+ 6 个 build.done(全被丢)+ 1 个 work.done(01:43:41)+ 8 个 review.wave.ready + 8 个 review.dimension.done(01:50~01:55)+ **0 个 synthesizer/plan-gate/shipper/reporter 输出** |
| `events-history-20260613-012231.jsonl` | (空或小) | 无下游事件 |
| `history.jsonl` | 1 行 | 仅 `loop_started` 标记,无 hat 激活历史 |
| `.events-20260613-012231.jsonl.idempotency.jsonl` | 1 行 | 8 个 review.wave.ready 共享 `idempotency_key` `ce-review:...:step-01:round-0`,wave_id `w-18b880c82d797fd8-33418-0` |
| `current-events` | 1 行 | 指向 `events-20260613-012231.jsonl` |
| `current-loop-id` | 1 行 | `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-neat-elm` |
| `loops.json` | 1 个 entry | 字段完整(worktree_path = workspace 绝对路径 = worktree 路径) |
| `agent/tasks.jsonl` | 1 条 task | task-1781313966-6e84 status=closed,owner=coordinator,started/closed 时间正确 |
| `agent/scratchpad.md` | 11 行 | **5 条 HUMAN GUIDANCE 重复**:`Focus on error handling` 重复 2 次,`Keep this in mind` 重复 3 次,无去重 |
| `agent/context.md` | (11604 B) | 计划上下文(尚未精读) |
| `diagnostics/2026-06-13T09-22-30/active-activations.json` | `[]` | **无 active hat 激活记录** —— 印证 synthesizer 死锁 |
| `diagnostics/2026-06-13T09-22-30/{drift,recovery}.jsonl` | 0 / 0 行 | U7 envelope 全空(preset 的 8 个 source 都没产出) |
| `diagnostics/2026-06-13T09-22-30/trace.jsonl` | 4 行 | 仅 4 行(已读) |
| `diagnostics/agent_doc_sync.json` | 1 条 info | `synced=0, skipped=2, failed=0`,outcome=recovered |
| `diagnostics/logs/ralph-2026-06-13T09-22-30-798-48392.log` | 53 行 | **核心证据**:L12-18 两次 handoff timeout + 6 次 build.done dropped;L25 work.done handoff timeout;L26-37 wave 启动/完成;L38-45 8 次 review-coordinator dropped;L46-52 aggregator 收 0 done |

### 3.3 worktree `/Users/pittcat/Dev/Rust/ralph-orchestrator/.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-neat-elm/.agents/scratchpad/ce-executor/2026-06-10-003-.../`

| 文件 | 大小 | 关键发现 |
|---|---|---|
| `context.md` | 11604 B | (尚未精读,已通过 context_path 解析) |
| `plan.md` | 2469 B | 7 步 U1-U7,U1 当前 |
| `progress.md` | 5894 B | L9 `Active Wave: 空` —— **错位**:实际 wave 跑过 8 worker;L20 `[ ] 提交 commit` 未勾选但 work.done 01:43:41 已发 —— **task close 顺序倒置** |
| `decisions.md` | 4241 B | 5 条 DEC:001-005,合理 |
| `findings-{correctness,testing,maintainability,standards,requirements,agent-native,learnings,api-contract}-task-1781313966-6e84.json` | 共 8 个文件 | 8 维度全产出;synthesizer 永远不读它们 |
| `wave-diff.patch` | 6702 B | review-coordinator 01:46~01:47 写 |

### 3.4 preset `/Users/pittcat/Dev/Rust/ralph-orchestrator/presets/en/ce-executor-isolated.yml`

| 行号 | 关键约束 |
|---|---|
| L48-56 | `execution_mode: isolated`, `verdict_gate` 配 `REVIEW_COMPLETE` + `report.done` |
| L103-112 | `event_policy.mode: enforce`, `on_violation: reject_with_resume` |
| L122-124 | `require_policy_check_for_cli_emit: true`, `allow_unsafe_cli_emit: false`(U5 强制) |
| L133-140 | `topic_deny_rules`:`{executor, build.done}` + 6 项 `{ralph, *}` —— **不包含 `{review-coordinator, review.dimension.done}`** |
| L154-236 | 16 个 schema,其中 `review.dimension.done` required_fields: dimension/findings_count/findings_file/plan_name/task_id/task_key/step |
| L266-374 | coordinator instructions 显式禁止创建/切换/重命名分支,允许 `work.ready` / `work.failed` |
| L383-385 | executor `publishes: [work.done, work.failed]`,**无 build.done** —— 印证 build.done 应被拒 |
| L554-557 | review-coordinator `publishes: [review.wave.ready, review.passed]`,**无 review.dimension.done** —— 与 8 次 dropped 一致(但 dropped 的不是 agent emit,是 wave re-publish) |
| L846-852 | dimension-reviewer `publishes: [review.dimension.done]`, `concurrency: 9` |
| L1040-1046 | synthesizer `triggers: [review.dimension.done]`, `aggregate.wait_for_all`, `timeout: 300` |
| L1196-1231 | synthesizer U6 Completeness Check 强制要求 `received == expected` 才能 emit verdict;不足时 emit `plan.blocked` |

### 3.5 根因唯一性证伪表

| 假设 | 是否成立 | 证据 |
|---|---|---|
| A. synthesizer 没收到 trigger 事件 | **不成立** | events.jsonl 8 个 `review.dimension.done` 都成功落盘;**是 bus 端 origin guard 拒**,不是 trigger 路由问题 |
| B. preset `topic_deny_rules` 显式 deny `review-coordinator → review.dimension.done` | **不成立** | preset L133-140 列表里**没有这条**;`review-coordinator` 的 `publishes` 是 `[review.wave.ready, review.passed]`,但 `topic_deny_rules` 是另一组 deny,这条 deny 不在 deny list |
| C. wave dispatcher 写 events.jsonl 时 hat=dimension-reviewer,但 re-publish 到 bus 时 hat 改成 review-coordinator | **成立** | events.jsonl L17-24 hat=dimension-reviewer 8 行;log L38-45 hat=review-coordinator 8 行;同一 8 个事件两个 hat |
| D. 落盘后由 wave::io::merge 触发 re-publish,re-publish 时使用"当前激活的 hat"作为 source | **机制猜测**(与 log 现象一致,无源码) |
| E. agent 越权 emit 失败不算根因(只是表层 6 次 build.done dropped) | **次要** | preset 配了 `topic_deny_rules: {executor, build.done}`,dropped 是预期行为,executor agent prompt 未充分教育自己不要发 build.done |

---

## 4. 执行链路对比图

```
                          预设期望                              实际(events.jsonl + log)
                          ─────                              ───────────────────────
work.start                触发 coordinator                    ✅ 01:22:31 loop_started
   ↓
coordinator.work.ready    emit                                ✅ 01:27:05 (ts+payload 完整)
   ↓ handoff              instant                             ⚠️ 17m 7s timeout → task.resume [log L12]
executor.build.done × 6   (禁止,应被拒)                       ⚠️ 6 次 dropped [log L13-18](agent prompt 漏)
   ↓
executor.work.done        emit                                ✅ 01:43:41
   ↓ handoff              instant                             ⚠️ 4m 20s timeout → task.resume [log L25]
review-coordinator        接收 work.done                      ✅ hat scope OK,正常
   ↓
review-coordinator        emit 8 个 review.wave.ready          ✅ 01:47:29 (wave_id 一致)
.wave.ready × 8           (1 次 wave 全部维度)
   ↓
wave::dispatcher          启动 8 workers (concurrency=9)      ✅ 01:48:01~01:56:00 (8m 19s)
   ↓
dimension-reviewer × 8    各自 emit review.dimension.done     ✅ 01:50:22~01:55:50 (8 个全落 events.jsonl)
                          (events.jsonl hat=dimension-reviewer)
   ↓
wave::io::merge           合并 + re-publish 到 bus             ⚠️ re-publish 时 hat=review-coordinator [log L38-45]
                          (期望 hat=dimension-reviewer)        8/8 dropped
   ↓
aggregator                等待 8/8                            ❌ 永远 0/8(收不到 done,bus 被 origin guard 吞)
   ↓ timeout 300s         emit plan.blocked                   ❌ 永远不 fire(没有 timeout 触发,聚合器停等)
review-synthesizer        emit review.passed/failed           ❌ 永不 fire
                          /complete
plan-gate                 接收 verdict                        ❌ 永不 fire
shipper                   REVIEW_COMPLETE                     ❌ 永不 fire
reporter                  report.done + LOOP_COMPLETE         ❌ 永不 fire
```

**结论**:整条链在 wave→synthesizer 这一段断裂;前面 6 步虽有几处次要偏离(timeout / executor.build.done 越权 / scratchpad 重复 / progress 错位),但**前 6 步都成功推进**;第 7 步(synthesizer)开始全断。

---

## 5. 问题归因表(P0/P1/P2)

| 严重度 | 编号 | 问题 | 归因 | 证据(绝对路径) |
|---|---|---|---|---|
| **P0** | P0-1 | wave merge re-publish 时 8 个 `review.dimension.done` 被 origin guard 全部 drop,synthesizer 永远不 fire,loop 死锁 | **Ralph 基座机制**(wave::io re-publish 时 hat source 标记错位) **+ preset 不防御**(topic_deny_rules 未配 `{review-coordinator, review.dimension.done}` 安全网) | log L38-45;events.jsonl L17-24 vs log L38-45 hat 标不一致 |
| **P0** | P0-2 | 7 个 U7 diagnostics envelope 全部 0 行(recovery/drift 缺失,events.jsonl L38-45 同样不在 recovery.jsonl 落) | **Ralph 基座机制** + **preset 编排** | 主仓 `diagnostics/2026-06-13T09-22-30/recovery.jsonl` 0 行;worktree 同;preset 期望 `event.isolation.boundary_violation` 落 recovery.jsonl |
| **P1** | P1-1 | executor agent 误 emit 6 次 `build.done`(preset `topic_deny_rules` 显式 deny,理应 100% 拒,但落 events.jsonl) | **agent 行为**(executor prompt 没强约束"绝不 emit build.done") | events.jsonl L2-7;preset L133-140 |
| **P1** | P1-2 | handoff dispatch 两次 timeout(`work.ready → executor` 17m / `work.done → review-coordinator` 4m) | **Ralph 基座机制** | log L12、L25 |
| **P1** | P1-3 | `progress.md` 状态错位:写 `Active Wave: 空` 但 wave 跑了 8 worker;`[ ] 提交 commit` 未勾但 work.done 已发 | **agent 行为**(plan-gate 未跑,progress.md 未刷新) | `progress.md:9`;events.jsonl L8 vs L17-24 |
| **P1** | P1-4 | scratchpad human guidance 重复 2-3 次,无去重 | **Ralph 基座机制**(`ralph tools interact progress` 或类似 squash 逻辑缺失) | `agent/scratchpad.md:1-11` |
| **P2** | P2-1 | 主仓 `.ralph/agent/` 空,但 worktree `.ralph/agent/` 有内容;loops.json 在 worktree,diagnostics 共享根路径解析在主仓 | **Ralph 基座机制**(`diagnostics-root` 解析逻辑不一致) | 主仓 `agent/`;worktree `agent/`;preset 注释 + `loops.json` 设计 |
| **P2** | P2-2 | U1 内 `[ ] 提交 commit` checkbox 未勾选,plan 写"独立 U1 commit"但 progress.md 写"已 commit 848043a" | **agent 行为** | `progress.md:19`;worktree `git log` 需复核 |
| **P2** | P2-3 | DEC-004 标 ralph-cli pre-existing flake,但 U1 任务工作目录在 worktree(不是 clean HEAD a7918fc) | **agent 行为**(根因分析可能不准确) | `decisions.md:36-50`;`progress.md:31-40` |
| **P2** | P2-4 | `task.resume` 路由由 handoff timeout 触发,落到 `safe_target`,但 user prompt 上看不到;RALPH_DIAGNOSTICS=1 未设(主仓 session 全 0 行) | **Ralph 基座机制** | log L12、L25;`diagnostics/*/recovery.jsonl` 全 0 行 |

---

## 6. 修复建议(分级)

### 6.1 P0-1 修复(链路核心)—— 3 选 1

| 方案 | 改哪里 | 工作量 | 推荐度 |
|---|---|---|---|
| **A. 源头修:Ralph 基座** 让 wave::io re-publish 时使用原始 emit hat(从 events.jsonl L17-24 读到 hat=dimension-reviewer,re-publish 保留该 hat) | `crates/ralph-cli/src/loop_runner/wave/io.rs`(或 wave::dispatcher 合并段) | 5-10 行 | **推荐** —— 直击根因,所有 preset 受益 |
| **B. 防御性 preset 加 deny** 在 `topic_deny_rules` 加 `{review-coordinator, review.dimension.done}` —— 但这是用错的方法堵对的漏洞,re-publish hat 错位问题仍在(其他 hat 走 re-publish 仍可能撞) | `presets/en/ce-executor-isolated.yml:133-140` 加 1 行 | 1 行 | **不推荐** —— 治标,且可能误伤其他 re-publish 路径 |
| **C. 改 agent 路径:让 wave worker 自己 emit 不走 re-publish** | wave::dispatcher 改 | 大改 | 不推荐 |

### 6.2 P0-2 修复(diagnostics 真空)

- 让 `event.isolation.boundary_violation` 真在 origin guard drop 时落到 `recovery.jsonl`(目前 log L38-45 是 stderr,没进 recovery.jsonl envelope)
- 启用 `RALPH_DIAGNOSTICS=1` + 配 `telemetry.runtime_diagnosis: { enabled: true, write_artifacts: true }` 写到主仓 `.ralph/`

### 6.3 P1 系列

- **P1-1**:executor agent prompt 加硬约束 "NEVER emit build.done/test.done/lint.done — verification result goes in `work.done` payload only"(preset L528-530 已有指令,但 6 次 dropped 表明 agent 仍尝试,需要更显式 / 加 schema precheck)
- **P1-2**:handoff timeout 调查为何 17m(默认应是秒级),配置或 bug
- **P1-3**:plan-gate 触发条件应包含"review-synthesizer 路径走完后",目前 events 流里 plan-gate 完全没机会 fire
- **P1-4**:`ralph tools interact progress` 落 scratchpad.md 时加去重 key

### 6.4 P2 系列

- **P2-1**:`diagnostics-root` 解析统一:以 `.ralph/loops.json` 的 `workspace.workspace` 字段为准,而不是路径启发式
- **P2-2**:executor prompt 加"close task 之前先 git commit"硬约束
- **P2-3**:DEC-004 重审,worktree 上跑测试 ≠ clean HEAD a7918fc,需要二次验证
- **P2-4**:task.resume 路由成功后,应把 resume 事件写到 `recovery.jsonl` 的 `not_retriable` outcome,让用户可见

### 6.5 防御性测试建议

| 测试位置 | 内容 |
|---|---|
| `crates/ralph-cli/src/loop_runner/wave/io.rs` (单测) | wave 8 维度合并 re-publish 后,bus 上收到的 8 个 `review.dimension.done` 仍应 hat=dimension-reviewer |
| `crates/ralph-core/src/event_origin.rs` (单测) | `topic_deny_rules` 拒绝时必须 emit `event.isolation.boundary_violation` envelope 到 `recovery.jsonl`,不能只 stderr |
| `crates/ralph-cli/src/loop_runner/tests.rs` (集成,需 cli-serial) | wave 完成后 30s 内 synthesizer 必须 fire,否则 fail |
| preset `ce-executor-isolated.yml` (yaml lint) | `topic_deny_rules` 应覆盖 wave re-publish 路径上所有可能的 hat 错位组合 |

---

## 7. 一句话行动项

**先修 P0-1**:让 `wave::io::merge` re-publish 时保留原始 emit hat 标(从 events.jsonl 读 hat=dimension-reviewer,不要用当前激活 hat=review-coordinator)—— 8 个 done 才能落 bus,synthesizer 才能 fire,整条链路才能推进到 plan-gate / shipper / reporter。
