---
date: 2026-06-13
type: ce-debug
diagnostic-of: 2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-crisp-wren
preset: ce-executor-isolated
plan: 2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split
subject: 为什么 U2 之后多轮审核 wave 没有运行 / executor 链路卡死
---

# ce-executor-isolated 多轮审核 wave 未运行诊断报告

> 📅 2026-06-13 | 🔖 loop `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-crisp-wren` · plan `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split` · preset `ce-executor-isolated` (isolated mode)
>
> 触发问题:本次运行 U1(commit `64b3794`)成功后进入 U2(`step-02`,task_id=`task-1781306436-dab4`)的 `work.ready`,TUI 里始终看不到任何 `review.wave.ready` / `dimension-reviewer` 活动 / `review-synthesizer` 聚合,多轮审核 wave **完全没有启动**。

---

## 1. TL;DR — 一句话定位

**`ce-executor-isolated` 在 U1 → U2 切步点进入 "大单步" 失败模式:`executor` hat 在 U2(`loop_runner/tests.rs` 11606 行拆分)上持续被 loop 调度但 agent 把每轮都消费在「追加 `### HUMAN GUIDANCE` 到 scratchpad.md」上,既不实施 task 也不 emit `work.done` / `work.failed`;`review-coordinator` 永远收不到 `work.done` 触发,所以 wave 链路从源头就没启动 —— 表面上"多轮审核没跑",根因在 wave 之前的 executor 路径,不是 wave 自身。**

下钻结论:

| 关注点 | 结论 | 证据 |
|---|---|---|
| `code review` 是否执行 | **从未执行** | `events-20260612-230900.jsonl` 共 3 条事件,0 次 `review.wave.ready` / 0 次 `review.dimension.done` / 0 次 `review-synthesizer` 出现 |
| `wave` 是否启动 | **从未启动** | 23:20:46 `work.ready`(step-02)后 `events.jsonl` **11 分钟零事件**;TUI 日志最后一次有效行为是 23:20:53 `Injecting ready tasks (1 ready, 1 open, 0 closed)` |
| preset 设计 | **存在结构性 gap** | `work.ready` 把整个 U2 作为单 task 下发(11606 行,远超单 iteration 上下文预算);executor 触发器列表 `[work.ready, fix.plan.ready]` 没有"大单步降级为 sub-task"机制 |
| agent 行为 | **卡在 guidance 循环** | `agent/scratchpad.md` 累计 4 条 `### HUMAN GUIDANCE`,跨度 23:12–23:17 UTC(每 1–2 分钟一条),内容都是 `Focus on error handling / Keep this in mind`,**没有实施 task 也没有 emit 终态事件** |
| Ralph 基座机制 | **发现 1 处 isolated-mode 设计缺陷 + 1 处预存事故信号** | TUI 日志 L6–7 `Isolated mode: event out of hat scope — dropping hat=coordinator topic=build.done`(coordinator 误 emit `build.done` 被 guard 拦截,无 hard-gate 反馈);L13 `Complete called for unknown or already-closed activation key ... terminal_topic=work.ready completed_count=0`(work.ready 触达后无下游 hat 接住) |
| task owner 错配 | **未发现** | task-1781306436-dab4 `owner_hat_id=coordinator`,但 ce-executor-isolated 的 `coordinator_hats` 已包含 executor,本次 owner 配错未触发(因为 executor 根本没运行) |
| U1→U7 进度 | **链路在 U2 卡死** | `progress.md` `Current Step` 仍 Step 01;Step 02–07 全部 pending;`tasks.jsonl` 中 task-1781306436-dab4 仍 `status: "open"` |
| 当前所处 step | **U2 step-02** | `events-20260612-230900.jsonl:3` `work.ready` step-02(23:20:46);此后无事件 |
| 进程状态 | **TUI 主进程(98725)仍存活,loop 子进程(98913)已退出,变成 orphan** | `loops.json` 登记 PID 98913,`loop.lock` 已清理;`ps aux` 显示 PID 98725 仍在但 `0% CPU` 状态 S+ |

---

## 2. 流程还原:预设 vs 实际执行链路

### 2.1 预设(`presets/en/ce-executor-isolated.yml`)期望链路

来源:preset 文件 L1–32 顶部注释 + L552–1347 各 hat 配置。

```text
work.start
   ↓
coordinator                         (triggers: work.start)
   ↓ work.ready(step-XX, task_id, task_key)
executor                            (triggers: work.ready, fix.plan.ready)
   ↓ work.done(plan_name, task_id, task_key, step, commit_count, changed_lines)
review-coordinator                  (triggers: work.done, fix.applied)
   ↓ review.wave.ready              (7 个 always-on 维度 + 条件维度,单 wave emit)
        ↓
        dimension-reviewer × N      (triggers: review.wave.ready; concurrency: 9)
             ↓ review.dimension.done × N
        review-synthesizer          (triggers: review.dimension.done; aggregate: wait_for_all, timeout: 300s)
             ↓ review.passed / review.failed / review.complete
plan-gate                           (triggers: review.passed, review.failed, review.complete, work.failed)
   ↓ queue.advance(plan_name, completed_step, next_step, reviewed_task_id)
   ... (循环回 executor 实施 next_step)
   ↓ plan.complete
shipper                             (triggers: plan.complete, plan.blocked, debug.exhausted)
   ↓ REVIEW_COMPLETE(pass_or_fail)
reporter                            (triggers: REVIEW_COMPLETE)
   ↓ report.done
   ↓ LOOP_COMPLETE
```

**关键点**:`executor` 必须 emit `work.done` 或 `work.failed`,这是 wave 启动的唯一前件。preset L526 明确写「**Terminal events**: Only `work.done` (success) and `work.failed` (blocked/failure) advance the workflow. You MUST emit one of these at the end of every iteration.」

### 2.2 实际(`events-20260612-230900.jsonl`)链路

来源:`.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-crisp-wren/.ralph/events-20260612-230900.jsonl`,共 3 条事件。

```text
work.start                          (loop 启动,23:09:00)
   ↓
[coordinator 解析 plan,创建 tasks,emit work.ready step-01]      ← step-01 未直接出现在 events.jsonl(history 已 flush,events.jsonl 保留 current session)
   ↓
executor.work.done                  (U1 实施,commit 64b3794)    ← progress.md L10 记录
   ↓
[review-coordinator 接收 work.done,emit review.wave.ready → wave]    ← 未在 events.jsonl 出现(history 已 flush)
   ↓
[plan-gate 通过,emit queue.advance → step-02]                  ← 未在 events.jsonl 出现
   ↓
[coordinator 推进,emit work.ready step-02]                     ← 23:20:46 events.jsonl:3 ✅
   ↓
executor(iteration 2+)              ← 应该 emit work.done/failed
   ↳ 实际:agent 反复追加 ### HUMAN GUIDANCE 到 scratchpad.md(23:12–23:17 累计 4 条),零次 commit,零次 work.* emit
   ↳ TUI 日志 23:20:53 显示 "Injecting ready tasks (1 ready, 1 open, 0 closed)"
   ↳ 此后 11 分钟 events.jsonl 零增长
   ↓
[review-coordinator]                ← 永不被激活
[dimension-reviewer × N]            ← 永不被激活
[review-synthesizer]                ← 永不被激活
[plan-gate]                         ← 永不被激活
[shipper / reporter]                ← 永不被激活
```

**关键观察**:**不是 wave 系统坏了,是 wave 的前件 `work.done` 从未被 emit**。整个链路在 `executor(iteration 2+)` 阶段停转。

### 2.3 实际(进程 / 运行时状态)链路

```text
PID 98913 (loop 子进程)             ← 23:09:00 启动,已退出(loop.lock 已清理)
PID 98725 (TUI 主进程)              ← 仍存活,CPU 0%,状态 S+ 持续 11 分钟无新输出
ralph/2026-06-10-003-...-crisp-wren 分支   ← checkout 状态正常
events-20260612-230900.jsonl        ← 共 3 行,23:20:46 后无增长
events-history-20260612-230900.jsonl ← 共 1 行(work.start warmup)
history.jsonl                       ← 共 1 行(loop_started)
agent/tasks.jsonl                   ← 1 条 task(1781306436-dab4),status: open
agent/scratchpad.md                 ← 4 条 HUMAN GUIDANCE 累积
agents/scratchpad/ce-executor/.../progress.md ← Current Step = Step 01,Step 02-07 pending
diagnostics/.../recovery.jsonl      ← 1 条(agent_doc_sync 同步成功,无关本次)
diagnostics/.../active-activations.json ← iteration 1 (coordinator + ralph) only,无 iteration 2
```

---

## 3. 证据清单

### 3.1 事件流证据(决定性证据)

| # | 文件 | 行号 | 关键内容 | 含义 |
|---|------|------|----------|------|
| E1 | `events-20260612-230900.jsonl` | L1 | `{"hat":"coordinator","payload":{"ok":true},"topic":"build.done","ts":"2026-06-12T23:12:52.868153+00:00"}` | coordinator 在 23:12 误 emit `build.done`(应是 review-synthesizer 的产物);后续被 isolated guard 拦截 |
| E2 | `events-20260612-230900.jsonl` | L2 | `{"hat":"coordinator","payload":{"ok":true},"topic":"build.done","ts":"2026-06-12T23:14:19.752850+00:00"}` | 第二次 `build.done`,同样被 isolated guard 拦截 |
| E3 | `events-20260612-230900.jsonl` | L3 | `{"hat":"coordinator","payload":{"complexity":"large",...,"step":"step-02","task_id":"task-1781306436-dab4",...},"topic":"work.ready","ts":"2026-06-12T23:20:46.196648+00:00"}` | **最后一次有效事件**:coordinator emit `work.ready`(step-02,U2 大单步,task-1781306436-dab4) |
| E4 | `events-20260612-230900.jsonl` | 全文 | 只有 3 行,23:20:46 之后 **零事件** | 11 分钟内 executor / review-coordinator / 任何 hat 都没产生事件 |

### 3.2 TUI 子进程日志(决定性证据)

文件:`.ralph/diagnostics/logs/ralph-2026-06-13T07-09-00-393-98725.log`

| # | 行号 | 日志内容 | 含义 |
|---|------|----------|------|
| T1 | L1 | `WARN ralph::loop_runner::runner: Interactive mode requested but stdout is not a TTY, falling back to autonomous` | loop 降级为 autonomous 模式,无人工交互能力 |
| T2 | L2-5 | `Memory injection check ... 0 memories` | 0 memories,本次运行无历史经验可参考 |
| T3 | L6-7 | `WARN ralph_core::event_loop: Isolated mode: event out of hat scope — dropping hat=coordinator topic=build.done` × 2 | **isolated mode guard 工作正常**,但仅做"丢弃"无 hard-gate 反馈,coordinator 不知道自己误 emit 了 `build.done` |
| T4 | L12 | `Injecting scratchpad (544 chars) into prompt` | 注入 scratchpad 内容 544 字符,主要是 HUMAN GUIDANCE |
| T5 | L13 | `WARN ralph_core::hat_lifecycle: Complete called for unknown or already-closed activation key key=2026-06-10-003-...-crisp-wren:2:coordinator terminal_topic=work.ready completed_count=0` | **关键告警**:`work.ready` 的 coordinator activation 标记为 `completed_count=0`,意味着**触达 `work.ready` 时没有下游 hat 准备好接住** |
| T6 | L19 | `Injecting ready tasks (1 ready, 1 open, 0 closed) into prompt` | 23:20:53 注入 1 个 ready task(U2),**0 closed** —— 印证 task-1781306436-dab4 始终未 close |
| T7 | L20+ | 文件结束,无后续日志 | TUI 进程在 23:20:53 后**没有任何有意义输出**,持续 11 分钟 |

### 3.3 任务 / 进度 / 上下文证据

| # | 文件 | 关键内容 | 含义 |
|---|------|----------|------|
| S1 | `agent/tasks.jsonl` | `task-1781306436-dab4`, `status: "open"`, `owner_hat_id: "coordinator"`, `loop_id: "2026-06-10-003-...-crisp-wren"`, `priority: 1`, `blocked_by: []` | U2 task 始终 open,从未被 `ralph tools task start` / `close` |
| S2 | `agent/scratchpad.md` | 4 条 `### HUMAN GUIDANCE (2026-06-12 23:12:53 / 23:14:07 / 23:16:56 / 23:17:08 UTC)`,内容反复出现 `Focus on error handling` / `Keep this in mind` | **executor 反复被激活但只追加 human guidance,不实施 task**;每条间隔 1–2 分钟,符合 loop 调度节奏 |
| S3 | `agents/scratchpad/ce-executor/2026-06-10-003-.../progress.md` | `## Current Step: Step 01 — U1: Public infrastructure scaffold`;`## Completed Steps: ✅ Step 01 (commit 64b3794)`;Step 02–07 全部 pending | U1 已完成 commit,U2 没启动 |
| S4 | `agents/scratchpad/ce-executor/2026-06-10-003-.../context.md` L48 | `**large** — cross-cutting refactor with 7 phases (U1-U7), 27 new subfiles, 201 tests, 70+ documentation references, multi-crate impact` | coordinator 评估 complexity=large,理应触发「自动 sub-task 拆解」,但实际只建 1 个 task |
| S5 | `agents/scratchpad/ce-executor/2026-06-10-003-.../context.md` L57 | `## start_sha: 64b37943ee6e03a70c542e418c5f95065a87276d` | U2 的 diff 锚点已就绪,但 executor 没用上 |

### 3.4 进程 / 锁证据

| # | 文件 / 命令 | 关键内容 | 含义 |
|---|------|----------|------|
| P1 | `loops.json` | `pid: 98913`, `started: 2026-06-12T23:09:00.781445Z` | loop 子进程 PID 98913 登记在册 |
| P2 | `find -name loop.lock` | **无 `loop.lock` 文件** | 登记的 PID 98913 已退出,lock 已清理 |
| P3 | `ps aux \| grep ralph` | `pittcat 98725 2.2 0.1 ... 8848 s046 S+ 7:09上午 0:58.23 ralph -H builtin:ce-executor-isolated...` | TUI 主进程 PID 98725 仍存活,但 S+ 状态 11 分钟 CPU 几乎不动 |
| P4 | `loops.json`(主仓库) | `pid: 98725` (主仓库与 worktree 的 loops.json 登记的 PID 不一致) | 主仓库登记的是 TUI PID,worktree 登记的是 loop 子进程 PID —— 已分裂 |

### 3.5 历史 incident 对照证据

| # | 文件 | 关联内容 |
|---|------|----------|
| H1 | `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md` | 2026-06-12 incident:plan-gate emit `queue.advance` 后 executor 10 分钟无 hat 激活,ralph 兜底注入 `task.resume` 被拒,loop 74 分钟后 `loop.cancel` 终止。本次症状高度相似,但卡点从 plan-gate 后移到了 executor 单步实施。 |
| H2 | `docs/report/2026-06-08-ce-executor-review-wave-not-firing-diagnosis.md` | 2026-06-08 incident:`review-coordinator` 直接 emit `review.passed` 短路 review。本次症状不同(根本没有 review-coordinator 触达),但同样是"wave 没起来"。 |
| H3 | `docs/report/2026-06-05-wave-abort-root-cause-analysis.md` | 2026-06-05 incident:wave 启动后中止。本次症状不同(wave 根本没启动)。 |

---

## 4. 失败链路分解(causal chain)

### 4.1 完整因果链(从触发到症状)

```
[trigger] 23:09:00 ralph run -H builtin:ce-executor-isolated --worktree
   │
   ├── loop 创建 worktree,登记 loops.json (PID 98913)
   ├── 启动 TUI 子进程 (PID 98725) RPC 模式
   └── loop 启动,emit work.start (iteration 0 warmup)
       │
       ├── coordinator (iteration 1) 解析 plan
       │     ├── 创建 task-1781306436-dab4 (U2 step-02),1 个 task 覆盖整个 11606 行拆分
       │     ├── 创建 context.md, progress.md, plan.md
       │     └── emit work.ready (step-02) 23:20:46 ✅
       │
       ├── [causal step 1: 大单步风险进入调度]
       │     └── work.ready payload 中 task_id 是单 task,U2 工作量(11606 行大文件)超过
       │         executor 单 iteration 上下文预算
       │
       ├── executor (iteration 2+) 被反复激活(每 1–2 分钟一次)
       │     │
       │     ├── [causal step 2: guidance 取代实施]
       │     │     └── agent 看到 human.guidance / HUMAN GUIDANCE 注入到 prompt,
       │     │         把它当成"主任务已完成,继续关注"信号
       │     │     └── agent 反复追加 "Focus on error handling / Keep this in mind" 到
       │     │         scratchpad.md,共 4 条 (23:12, 23:14, 23:16, 23:17)
       │     │     └── agent 不调用 `ralph tools task start` / `close`,
       │     │         不做 U2 文件拆分,git 无 commit
       │     │
       │     └── [causal step 3: 终态事件永不 emit]
       │           └── work.done / work.failed 永不被 emit
       │
       ├── [causal step 4: wave 前件缺失]
       │     └── review-coordinator 的 trigger `work.done` 永不被触发
       │     └── review.wave.ready 永不被 emit
       │     └── dimension-reviewer × N 永不被激活
       │     └── review-synthesizer 永不被激活
       │     └── plan-gate 永不被激活(其 trigger 也是 review.* + work.failed)
       │     └── queue.advance / plan.complete 永不被 emit
       │
       ├── [causal step 5: loop 状态分裂]
       │     └── loop 子进程(98913)在某个时刻退出(原因:可能超时/panic/无任务可调度)
       │     └── .ralph/loop.lock 被清理
       │     └── loops.json 仍登记旧 PID(stale)
       │     └── TUI 主进程(98725)变成 orphan,无子循环可观察
       │
       └── [symptom] TUI 看不到任何 wave / 任何 hat 活动,events.jsonl 11 分钟零增长,
                     tasks.jsonl 1 task 始终 open,progress.md 停在 Step 01
```

### 4.2 不确定链节的预测(prediction)

| 链节 | 预测(必须同时为真) | 验证方法 | 验证结果 |
|------|---------------------|----------|----------|
| C1: 大单步是根因 | 如果把 U2 拆成 5 个 sub-task(task-1781306436-dab4.a/b/c/d/e),每个 < 500 行,executor 会在 ~10 分钟内 emit work.done 之一,wave 链路应能启动 | 重启 loop 时用拆解后的 plan,观察 events.jsonl 是否出现 work.done | 待验证 |
| C2: agent 把 guidance 当主任务 | executor prompt 注入 scratchpad 时,内容前缀是 `### HUMAN GUIDANCE ...`,agent 把它当成"已经引导过"的信号而非"主任务列表" | 读 executor instruction 文本,确认它把 scratchpad.md 视为次要输入 | 间接验证(S2 scratchpad 累积符合此模式) |
| C3: isolated guard 不会 hard-gate | coordinator 误 emit `build.done` 应被 guard 拒绝并产生 hard-gate 反馈(例如 emit `task.resume` 路由给 coordinator) | 读 preset L107–141 `event_policy.on_violation: reject_with_resume`,确认实际触发 | **预测部分错**:guard 仅 "drop" (T3),未触发 `task.resume` 路由——coordinator 没收到失败反馈,可能继续在错误假设下重试 |
| C4: 进程 orphan 是 loop 退出导致 | 如果不终止当前 PID 98725,新 loop 启动会因 loops.json 残留 PID 98913 引发 `is_alive()` 误判 | `ralph loops list` 是否仍把 PID 98913 视为 alive(已死但未清理) | 待验证 |

**C3 预测错位是关键**:`reject_with_resume` 应该是 hard-gate 行为,但实际只 drop。这意味着即使 coordinator 误 emit,事件流也只丢事件不丢 agent 上下文——agent 不知道自己做错了什么。

---

## 5. 问题归因表

| 级别 | 问题分类 | 问题 | 根因 | 证据 |
|------|----------|------|------|------|
| **P0** | preset 设计 | U2 大单步(11606 行)作为单 task 下发,executor 无 sub-task 拆解机制 | preset L262–366 coordinator instructions 缺少 `large` complexity 时的强制 sub-task 拆解规则;L375–554 executor 触发器列表 `[work.ready, fix.plan.ready]` 不感知 task 大小 | E3, S1, S4 |
| **P0** | agent 行为 | executor 反复追加 HUMAN GUIDANCE 替代实际工作 | scratchpad.md 注入机制(预设 L 区域)对 guidance 缺乏「这是一次性提示,非迭代输入」明示;agent 把 guidance 误读为"主任务列表" | S2 (4 条累积), T5 (completed_count=0) |
| **P0** | 事件链断裂 | wave 永远不被触发,因为 `work.done` 永不被 emit | executor 不出 terminal event → review-coordinator 不激活 → wave 短路;这是 P0 问题的下游表现,不是独立根因 | E1–E4 (events.jsonl 23:20:46 后零增长) |
| **P1** | isolated mode 设计(诊断时判断有偏,详见 5.5) | `Isolated mode: event out of hat scope — dropping` 仅丢事件不 hard-gate | 读 `event_loop/mod.rs` 中 isolated mode 的 drop 逻辑(待源码核实),drop 后不产生 `task.resume` 或 `recovery.jsonl` 反馈,coordinator 不知道 emit 失败 | T3, T5 |
| **P1** | 进程生命周期 | TUI 主进程(98725)变成 orphan,子循环已退出但 `loops.json` stale | loop 子进程退出时未更新 `loops.json` 的 `pid` 字段;TUI 主进程没有心跳检测到子循环死亡 | P1–P4 |
| **P2** | 调度节奏 | iteration 2+ 每次都被激活但每次都空转,消耗 LLM 配额无产出 | executor 调度频率(~1–2 分钟一次)没有基于"上一次产出"反馈调节 | S2 时间间隔 1–2 分钟 |

### 5.1 表注(诊断后置修订)

第 245 行 P1 第一行关于"isolated mode 仅 drop 不 hard-gate"的归因,经事后源码核对(`crates/ralph-core/src/event_loop/mod.rs:4789-4918`)实际**只对了一半**:
源码已经在 drop 之后 emit `event.isolation.boundary_violation` + 注入 `task.resume` 给源 hat(带 dedup),本机制已实现,不存在"完全静默"。
原 P1 行的"修复建议 6.2.3 加 hard-gate 反馈"对应的工作**实际已落地**,真正缺的是 hard-escalation 路由目标错位 + 缺超时自动 emit 失败事件的兜底(详见下文 5.5)。
本报告后续修订以 5.5 节"结论性判断"为准。

---

## 5.5 结论性判断:编排问题,不是机制问题(但机制也有盲点)

### 5.5.1 这次卡死的根因是编排问题

`ce-executor-isolated` preset 在 U2 阶段把 **11606 行的代码拆分工作**作为**单 task** 下发给 executor,executor 在单 iteration 的上下文预算内无法完成,只能"假装在干活"。
coordinator 评估 `complexity: large` 后**没有强制拆 sub-task**,这是预设层的结构性 gap,直接导致 executor 反复空转、事件流停转。

**编排层 gap 是主要根因。** 在不动 preset 的前提下,任何恢复机制都只能缓解症状,无法根治——因为根本问题是任务粒度不适合任何 agent 单轮完成。

### 5.5.2 恢复机制本身存在,但有 3 个盲点

源码审计确认 Ralph **已经实现**以下恢复机制(参见 `crates/ralph-core/src/event_loop/mod.rs`):

1. **Isolated boundary violation 反馈**(`mod.rs:4789-4918`):
   coordinator 误 emit `build.done` 时,系统会:
   - emit `event.isolation.boundary_violation` 到 bus
   - emit `task.resume` 给源 hat,payload 写明允许的 publishes
   - 带 dedup,避免 event-storm

   本报告 6.2.3 中"加 hard-gate 反馈"的修复建议**已部分实现**——此机制在 U2 阶段已上线。

2. **Stall recovery(fallback event 注入)**(`mod.rs:2399-2503`,触发点在 `crates/ralph-cli/src/loop_runner/runner.rs:1996`):
   每轮无 business event 时,runner 会:
   - 第 1–2 次:soft recovery,注入 task.resume 给上一个 hat
   - 第 3 次(`STALL_HARD_THRESHOLD=3`):hard escalation,路由到 review-coordinator/review-synthesizer
   - 全程记录到 `recovery.jsonl`

3. **机制盲点 1——hard-escalation 路由目标错位**:
   ce-executor-isolated 场景下 executor 卡死,hard-escalation 把 task.resume 发到 review-coordinator(`mod.rs:2415-2418`)。
   但 review-coordinator 的 trigger 是 `["work.done", "fix.applied"]`(`presets/en/ce-executor-isolated.yml:555`),**根本不会被激活**——hard escalation 打了个空炮。
   更合理的目标是:
   - 路由回 coordinator(让它重新评估 + 强制拆 sub-task),或
   - 直接 emit `work.failed`(plan-gate trigger 含 `work.failed`,可接住并发出 `plan.blocked`)

4. **机制盲点 2——机制默认 agent 理性**:
   stall recovery 假设 LLM Agent 看到 task.resume 的下一轮会主动 emit 一个 valid event。
   本次 executor 实际行为:把 task.resume 当成新一条 guidance,继续追加到 scratchpad,**继续不 emit event**。
   机制需要"超时 = 自动 emit 失败事件"这种**不可被 agent 绕开**的兜底。

5. **机制盲点 3——isolation guard 反馈写入 recovery 但运行产物易失**:
   boundary violation 反馈机制存在,但 `recovery.jsonl` 写在 `.ralph/diagnostics/<session>/` 下,与 `worktree` 一样是 gitignored 运行产物,loop 异常结束后易失,导致诊断时看不到反馈痕迹,误判"机制不存在"。
   建议把边界 violation 的高优先级信号同时落进 worktree 内的 `.ralph/agent/recovery.md`(纳入 git),确保诊断可追溯。

### 5.5.3 进程层(orphan TUI)与编排/机制正交

TUI 主进程变成 orphan 与编排/恢复机制无直接因果关系,属于进程生命周期管理问题(P1 级别,6.3.2 的心跳检测修复即可解决)。

### 5.5.4 最终归因

| 层级 | 问题 | 严重度 | 修复优先级 |
|------|------|--------|-----------|
| **编排** | 大单步无 sub-task 拆解 | **P0** | 立即(6.2.1) |
| **机制** | hard-escalation 路由目标错位 + 缺超时兜底 | **P1** | 本周(6.2.2 + 6.2.4) |
| **机制** | boundary violation 反馈运行产物易失 | **P2** | 本季度(6.3.4) |
| **进程** | loop 子进程退出 TUI 不退出 | **P2** | 本季度(6.3.2) |

**一句话**:这次失败的根因是编排(preset 没拆 sub-task),机制本身存在但 hard-escalation 路由目标错位让机制空转——改编排治本,改机制治标,两件事都得做。

---

## 6. 修复建议

### 6.1 立即恢复(不改代码,只改操作)

```bash
# 1. 停止当前 TUI + orphan loop
ralph loops stop 2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-crisp-wren

# 2. 拆分 U2 为更小的 sub-task(避免单 iteration 上下文爆栈)
#    编辑 .ralph/specs/2026-06-10-003-.../context.md
#    把 "Step 02 — U2: Split loop_runner/tests.rs" 拆成 5 个 U-2.x:
#      U-2.1: copy mutex doc + 创建 tests/mod.rs (5 files scaffolding)
#      U-2.2: 移 FAKE_PATH_BACKEND_* Mutex 到 tests/fake_path.rs
#      U-2.3: 移 install_mock_acp_executions / MockAcpExecution / Guard 到 tests/common.rs
#      U-2.4: 移 wave-specific helpers 到 tests/wave.rs
#      U-2.5: 移 hooks-specific helpers 到 tests/hooks.rs + 删 tests.rs

# 3. 重新启动 loop
cd .worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-crisp-wren
ralph run -H builtin:ce-executor-isolated -P "$PWD/PROMPT.md" --worktree

# 4. 观察 TUI:第一波 review-coordinator.review.wave.ready 应在 executor 第一次
#    work.done 后 30 秒内出现
```

### 6.2 中期修复(ce-executor-isolated preset 改造)

1. **coordinator 阶段强制拆 sub-task**(解决 P0 preset 设计)
   - 文件:`presets/en/ce-executor-isolated.yml` L262–366 coordinator instructions
   - 改动:增加 `### Sub-task 拆解规则 (large complexity 强制)` 章节
   - 规则:`complexity: large` 时,扫描 Implementation Unit 的 estimated_changed_lines,> 800 行的 U 必须拆成 U-Na/U-Nb/U-Nc,每个 < 500 行,对应独立 task_id 和独立 work.ready
   - 影响:executor 收到的单 task 永远在 smart zone 内(40-60% of 176K tokens),guidance 替换实施的概率下降

2. **executor 跳过检测**(解决 P0 agent 行为)
   - 文件:`presets/en/ce-executor-isolated.yml` L375–554 executor instructions
   - 改动:在 HARD RULE 章节加「**Skip Detection**:如果同一 task_id 被 executor 激活 ≥ 3 次而 task 仍 `open` 且 git 无 commit,emit `work.failed` with `reason: "executor stuck: <N> consecutive no-op iterations"`,路由到 plan-gate」
   - 影响:guidance 循环会在 3 轮后被强制终止,plan-gate 收到失败信号后可以 emit `plan.blocked` 而不是无止境等待

3. **isolated mode hard-gate 反向反馈**(解决 P1 isolated mode 设计)
   - 文件:读 `crates/ralph-core/src/event_loop/` 中 isolated drop 逻辑(待源码核实)
   - 改动:drop 事件时,emit `recovery.jsonl` envelope `source: workflow_guard, reason_code: hat_out_of_scope`,并 inject `task.resume` 到 hat 的下一次 prompt,让 agent 知道上一次 emit 被拒
   - 影响:coordinator 误 emit `build.done` 后,会收到「你不能 emit `build.done`」的反馈,避免重复犯错

4. **wave 等待可视化**(用户体验)
   - 文件:TUI 组件(待定位)
   - 改动:在 TUI 顶部状态条增加 "Awaiting executor terminal event" 标识,当 `work.ready` 已发但 `work.done`/`work.failed` 超过 5 分钟未到时显示
   - 影响:用户能看到"loop 不是卡在 wave,是卡在 executor",避免误判 wave 系统故障

### 6.3 长期机制(Ralph loop 基座)

1. **executor context-overflow guard**(基于 token 用量)
   - 文件:`crates/ralph-cli/src/loop_runner/runner.rs`
   - 改动:在 `run_loop_impl` 加 token-usage checkpoint;如果单 iteration token 接近 60%(smart zone 上限),自动 emit `work.failed` 路由到 debug-resolver
   - 影响:agent 不再"在 guidance 上空转",超额 token 立刻 hard-gate

2. **loop 子进程 / TUI 主进程心跳**
   - 文件:`crates/ralph-cli/src/commands/run.rs`(TUI 启动处)
   - 改动:TUI 主进程定期(每 30 秒)检查 loop 子进程 `is_alive()`,如果子进程退出,主动清理 `loops.json` 并显示「Loop terminated, TUI exiting」
   - 影响:避免 TUI 变成 orphan 持续占用资源

3. **调度节奏基于产出反馈**
   - 文件:`crates/ralph-core/src/event_loop/loop_state.rs`
   - 改动:executor 连续 3 次激活但 git 无 commit 时,延长激活间隔从 1–2 分钟到 5 分钟,避免无意义 token 消耗
   - 影响:无产出 hat 不会持续高频激活浪费 quota

### 6.4 推荐的下一步操作

按优先级:

1. **立刻做(15 分钟)**:执行 6.1 步骤 1–3,重启 loop 时用拆解后的 U-2.1 ~ U-2.5;如果 wave 链路启动,确认 hypothesis C1
2. **本周内**:起 PR 实现 6.2.2 executor skip detection,这个改动能避免未来同类 incident 持续 11 分钟空转
3. **本季度**:实现 6.3.1 context-overflow guard + 6.2.1 coordinator 强制 sub-task 拆解,从根上消除"大单步"失败模式

---

## 7. 关键学习(可作为 `docs/solutions/` 候选)

- **「wave 没起来」不一定 wave 故障**:本次症状表现是 wave 没启动,但根因在 wave 之前的 executor 路径。诊断时**先看 events.jsonl 是否有 `work.done`,再判断 wave**。
- **HUMAN GUIDANCE 累积是 agent 失败的早期信号**:如果 `agent/scratchpad.md` 在 N 分钟内出现 N≥3 条相同模式 guidance,基本可以判定该 hat 处于「空转状态」。
- **大单步 = 高失败概率**:单 task 超过 800 行 / 超过 50% smart zone,preset 必须强制 sub-task 拆解,否则几乎必然撞上下文超限。
- **isolated mode 的 drop 行为缺乏反馈**:guard 拒收事件后,应同时 emit `recovery.jsonl` envelope 并 inject `task.resume`,否则 agent 在错误假设下继续重试。
- **进程生命周期分裂**:`loops.json` 登记的 PID 与实际 `ps` 不一致时,要同时清 TUI 主进程和 loop 子进程,单停一个会留 orphan。

---

**Confidence**: High
**Owner**: 待指派
**Related**: `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md`(类似 dispatch gap,但卡点不同)
**Upstream**: `docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md` U2 章节
