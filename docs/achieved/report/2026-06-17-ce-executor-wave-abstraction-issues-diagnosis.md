# ce-executor wave 抽象问题诊断报告

> **报告日期**:2026-06-17
> **作者**:Loop & Preset 诊断专家(Ralph 自动报告 + 人工对话总结)
> **覆盖范围**:`presets/en/ce-executor-isolated.yml` 的 `dimension-reviewer concurrency:9` + `review-synthesizer aggregate` 拓扑;`presets/en/ce-executor-wave.yml` 同类拓扑;`ralph wave` CLI 通用能力
> **触发原因**:4 份诊断文档(`2026-06-16-loop-diagnostic-report.md` / `2026-06-16-systematic-review-of-recent-fixes.md` / `2026-06-17-ce-executor-isolated-flow-reliability-plan-loop-synthesizer-stall-diagnosis.md` / `2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md`)反复指出同一类失败模式;8 份 `docs/brainstorms/` 下的 wave-* 需求文档累计 30+ R 编号机制未收口;用户判断"已经做了很多努力,但依然不稳定"
> **核心结论**:**`ce-executor-isolated` 的 `dimension-reviewer concurrency:9` 不是 Claude Code Subagent,而是"9 个并发 LLM 调用写共享状态"——这是该抽象反复失败的根因。**Wave/fan-out 抽象本身在 LLM agent 编排里有用(Claude Code / AutoGen / CrewAI / LangGraph 全部在用),但 Ralph 的"wave worker"只实现了 prompt 头部一行文字,没实现 Subagent 隔离。

---

## 1. 结论摘要

**Wave/fan-out 这个抽象本身没有错。** Claude Code 的 Task tool / Subagent、AutoGen 的 GroupChat、CrewAI 的 Crew、LangGraph 的 Send API 全部是 wave/fan-out 模式,业界用了 2 年稳定。**错的是 Ralph 的"wave worker"实现——它只做了 fan-out 的形,没做 Subagent 隔离的实。**

### 1.1 三个核心区分

| 维度 | Claude Code Subagent(标杆) | Ralph `WaveWorkerContext` (现状) | 影响 |
|---|---|---|---|
| 进程隔离 | 独立 claude 进程,OS 级隔离 | 同 1 个 `loop_runner` process,9 个 prompt 排队 | 9 worker 共享 LLM 连接,token 配额单点 |
| 上下文窗口 | 子 agent 独立 context,主 agent 看不到中间过程 | 9 worker 共享同一份 `events.jsonl` 写流 | 互相能读到对方的 emit,导致状态污染 |
| 工具池 + 工作目录 | 子 agent 独立 tool pool + worktree | 9 worker 工具池一致,写同 repo 根目录 | 9 worker 互相覆盖 git 状态 |

**`WaveWorkerContext`( `crates/ralph-core/src/wave_prompt.rs:9-20` )的完整定义**——就这 4 个字段:

```rust
pub struct WaveWorkerContext {
    pub wave_id: String,
    pub wave_index: u32,
    pub wave_total: u32,
    pub result_topics: Vec<String>,
}
```

**它的作用是给 prompt 头部加一行 "You are worker 4/9 in wave w-xxx"**。**仅此而已**。9 个 worker 实际共享:
- 同一个 `loop_runner` 进程(无 OS 隔离)
- 同一份 `events.jsonl` 写流(file lock 串行)
- 同一份 plan state(`tasks.jsonl` 写流)
- 同一份 git 工作目录(无 worktree 隔离)
- 同一份 prompt cache(这是唯一的好处,但被协调成本抵消)

**这不是 Subagent 模式。这是"9 个 LLM 调用写共享状态"。**

### 1.2 失败症状归因(5 条)

| # | 症状 | 实证 loop | 根因 |
|---|---|---|---|
| 1 | 11→4 维 review stall,synthesizer 永不 fire | `zippy-sparrow` 1h04m | 9 worker 共享 `events.jsonl`,synthesizer 等待 wave_id=N 全员到齐,但 N 个 worker 互相看不到对方完成状态 |
| 2 | `work.done` 12s 内二次发,同 payload | `keen-fern` 1h47m | 2 个 executor worker 各自看 task 状态 open,各自独立 emit 完成事件。**单 executor 串行根本不会发生** |
| 3 | `review.passed(skip_reason=empty_diff)` 在 wave 还 open 时被允许发出 | `zippy-sparrow` | `check_semantic_gates` 把 `review_passed_while_wave_open` 误归 `InvalidFieldValue` 字段错误,绕过 R6 收摊 |
| 4 | `incomplete_wave_gate` 触发后 fixer / debug-resolver / plan-gate 4 个 fix 路径结构性失效 | `keen-fern` | preset 设计的 4 个 fix 恢复 hat 被 R6 收摊截断,plan.blocked schema 过松无法与 plan-gate reconcile |
| 5 | `stall_recovery` 4 次 escalation 后 plan-gate 未启动 | `zippy-sparrow` | stall detector 把 rejection noise 当作 event 流,识别不出"rejection stall"模式 |

**所有 5 条症状的共同根因**:**9 个 LLM worker 共享状态,不是 9 个独立 Subagent**。**5 条症状指向同一个抽象错误**,不是 5 个独立的工程 bug。

### 1.3 业界对比

**Claude Code Subagent 文档(6 篇 CSDN 源码分析 + Claude Code 官方文档交叉验证)**:

> "**子代理是一个微型会话,执行完毕后中间过程全都消失,主对话只看结论。**"——knqiufan 博客园 2026-04-09
>
> "**worktree isolation for parallel execution + team coordination with async mailboxes**"——Claude Code 核心公式 2026-05-21
>
> "**Tool 是原子能力,Agent 是隔离的执行上下文。Agent 本质上是独立 Claude 实例。**"——wlu 博客园 2026-03-13

**LangGraph / CrewAI / AutoGen(2026-04 至 06 多篇综述)**:

> "**Supervisor 模式:Supervisor 节点是单一控制点,子 agent 是 Supervisor 状态机的一部分。**"——拓业智询实战 2026-06-06
>
> "**Datadog 2026 报告:5% 的 LLM 调用报错,60% 是 rate limit,重试风暴引发系统雪崩。一个 AI Agent 接到需求,并发启动 20 个子 Agent,前 3 秒看起来很快,第 4 秒 API 开始 5xx。**"——CSDN 6 月 13 日

**MAST 论文("Why Do Multi-Agent LLM Systems Fail?", BuildingTrust '25)**:

> "**多智能体系统的失败是结构性的,不是工程 bug。**"——对 5 个 MAS 的 150+ Trace 做 GT 实证研究,首篇多 agent 失败分类法

**业界共识**:multi-agent LLM 编排不稳是结构性的,正确做法是 **Supervisor 单点控制 + 受控调度**,**不是 fan-out 自由竞争**。Ralph `ce-executor-isolated` 里的 `dimension-reviewer concurrency:9` + `review-synthesizer aggregate` **是反业界共识的"fan-out 自由竞争"**。

### 1.4 关键澄清:Wave 这个抽象本身没错

**直接证据**:Claude Code Task tool / Subagent、AutoGen GroupChat、CrewAI Crew、LangGraph Send API 全部是 wave/fan-out 模式,2024-2026 两年稳定运行。

**真正错的是 Ralph 现状的"wave worker"**——**它把 fan-out 抽象的实现做了 1/4**(只有 fan-out prompt 头,没有 Subagent 隔离)。**`ce-executor-serial.yml` 走的"单 LLM 串行 N 维"不是"反 wave"**,**它是 wave 抽象在"任务有上下文耦合"场景下的合理替代**(和 Supervisor 模式并列)。

### 1.5 5 份历史诊断的二次审视

`docs/brainstorms/` 下 8 份 wave-* 需求文档(`flow-reliability` / `step-handoff` / `wave-dimension-enforcement` / `loop-stability` 等)累计 30+ R 编号机制——

**它们都是治"9 worker 共享状态"的补丁,不是治 wave 抽象本身。** 包括但不限于:
- `incomplete_wave_gate` R6 → 治"9 worker 收不齐"
- `wave.worker.failed(dimension_mismatch)` → 治"9 worker 互相撞维度"
- `is_dual_publish_step_handoff` R-B3 → 治"12s 内二次 work.done"
- `check_semantic_gates` 改 `ViolationType::SemanticGateViolation` → 治"wave open 时 review.passed 被允许"
- 通用 `missing_event_gate` → 治"前置事件没到齐,后置事件许 fire"

**补丁越叠越多,根因没动**——**根因是"9 worker 共享状态",不是"机制不够多"**。

---

## 2. 执行链路对比图

### 2.1 Preset 预期事件流(`ce-executor-isolated`)

```
work.start
  └─ coordinator (work.start → work.ready)
       └─ executor (work.ready → work.done)
            └─ review-coordinator (work.done → review.wave.ready ×N)
                 └─ dimension-reviewer ×N (concurrency:9, review.wave.ready → review.dimension.done ×N)
                      └─ review-synthesizer (aggregate: wait_for_all → review.passed|review.failed)
                           └─ plan-gate → queue.advance | plan.complete
                                └─ shipper → REVIEW_COMPLETE → reporter → report.done → LOOP_COMPLETE
       └─ (fix path): Fixer ≤3 → debug-resolver → Executor (fix.plan.ready)
       └─ (block path): review-synthesizer → plan.blocked → shipper → REVIEW_COMPLETE → reporter → report.done
```

### 2.2 实际执行链路(loop `keen-fern` 1h47m52s)

```
[iter 0] work.start                              02:33:24
[iter 0] work.ready (coordinator, valid)         02:47:43
[iter 1] work.done (executor #1, valid)          03:30:02
[iter 1] work.done (executor #2, ⚠️ 12s 内同 payload 二次) 03:30:14   ← 共享状态
[iter 4] review.wave.ready (review-coordinator, 4 维)  03:30:25
[iter 4] review.dimension.done (correctness)     03:30:35
[iter 4] review.dimension.done (ux)              03:31:12
[iter 4] review.dimension.done (security)        03:32:48
   ↓ testing + maintainability 2 维 stall
[iter 5] review.dimension.done (maintainability) 04:06:10   ← 0.8×aggregate_timeout 边界
[iter 5] review.dimension.failed (testing)       04:06:40   ← aggregate_timeout
   ↓ 触发 R6 incomplete_wave_gate
[iter 5] plan.blocked(reason=dimension_reviewers_failed_to_converge)  04:06:40   ← R6 收摊
[iter 6] REVIEW_COMPLETE(verdict=fail)           04:18:45   ← shipper
[iter 6] verdict_gate 拦截                       04:21:16   ← 终止
```

### 2.3 实际执行链路(loop `zippy-sparrow` 1h04m)

```
[iter 0] work.start                              03:01:30
[iter 0] work.ready                              03:01:45
[iter 1] work.done (executor)                    03:08:12
[iter 1] review.wave.ready (11 维)               03:08:20
[iter 1-2] review.dimension.done (4 维齐)        03:09:00 - 03:54:50
   ↓ 7 维 stall
[iter 2] work.done (executor 二次, ⚠️)            03:54:50
[iter 3] review.passed(skip_reason=empty_diff)   04:06:52   ← wave 还 open!
[iter 3] task.resume × 5                         04:07:00
[iter 3] PayloadContractViolation                04:08:04   ← 终止
```

### 2.4 链路关键差异

**预期 vs 实际的核心差异**:
- **预期**:9 worker 独立 → 各自 emit review.dimension.done → synthesizer aggregate 等齐 → 一次性合并
- **实际**:9 worker 共享 `events.jsonl` 写流 → 互相覆盖 task 状态 → synthesizer 等不齐(因为 N worker 互相看不到)→ 触发 R6 收摊 → fix 路径被截断

---

## 3. 失败模式分类

按共享状态冲突类型分 3 类。

### 3.1 类型 1:`events.jsonl` 写流冲突(file-level 共享)

**症状**:12s 内二次 `work.done`(keen-fern)、5 次 `task.resume` 循环(zippy-sparrow)

**根因**:9 worker 都通过 `ralph emit` 写 `events.jsonl`——file lock 串行,9 worker 看到的事件流**互相穿插**。executor A 看到 task 还 open,executor B 看到 task 还 open,**两个独立判断 + 两次 emit**。

**对比**:Claude Code Subagent 每个子 agent 独立 mailbox,主 agent 单点 merge——**主 agent 拿到的 N 份结论是"快照"不是"穿插流"**。

### 3.2 类型 2:`tasks.jsonl` 写流冲突(应用层共享)

**症状**:dimension-reviewer 跑 4 维,但只有 2 unique dimension 被审(zippy-sparrow)

**根因**:`dimension-reviewer concurrency:9` 启动 9 个 worker,但 `wave_id` 分配是 review-coordinator 单次 emit `review.wave.ready` 9 次——**9 次 emit 在 `events.jsonl` 里是连续 9 行,review-coordinator 没有持久化"哪个 worker 该审哪个 dimension"**。9 worker **自己**从 payload 推 dimension,重复推 + 漏推。

**对比**:Claude Code Subagent 主 agent 派任务时**显式指定子 agent 职责**(如 `Task(subagent_type="Explore", prompt="find X")`)——**子 agent 不需要自己推**。

### 3.3 类型 3:状态机死锁(事件流不闭合)

**症状**:synthesizer 永不 fire、stall_recovery 4 次后 plan-gate 未启动

**根因**:`review-synthesizer aggregate: wait_for_all` 等 N 维齐全才 fire——但 N 维是 wave_id 关联,**9 worker 互相读不到对方的完成事件**(被 file lock 延迟 / 事件被中途错误标记)。synthesizer 永远等不齐,触发 `incomplete_wave_gate` 收摊,触发 `plan.blocked`,plan-gate 走 dead branch。

**对比**:Claude Code Supervisor 模式下,主 agent 是**单一控制点**,子 agent mailbox 投递完成信号,主 agent 计数器 +1——**单点状态,不死锁**。

---

## 4. 业界对比表

### 4.1 Subagent 模式四要素(Claude Code 标杆)

| 要素 | 作用 | Ralph 现状 | 缺口 |
|---|---|---|---|
| 进程隔离 | OS 级隔离,worker 死不影响主 | ❌ 同 1 个 process | 全模块重写 |
| 上下文隔离 | 子 agent 独立 context,主 agent 看不到中间过程 | ❌ 共享 `events.jsonl` | 重写事件流 |
| Worktree 隔离 | 子 agent 在 worktree 改文件 | ❌ 写同 repo 根 | 改 worktree 集成 |
| Mailbox 通信 | 子 agent 异步投递,主 agent 单点 merge | ❌ 共享 stream 写 | 加 mailbox 层 |

### 4.2 4 种 wave 模式横向对比

| 模式 | 适用场景 | 协调成本 | Ralph 对应 | 稳定性 |
|---|---|---|---|---|
| **单 LLM 串行 N turn** | 任务有上下文耦合(N 维 review、连续分析) | 0(无协调) | `ce-executor-serial.yml` | ✅ 稳定 |
| **Supervisor 集中调度** | 子任务独立 + Supervisor 单点 merge | 中(单点状态) | ❌ 没实现 | ✅ 业界标准 |
| **Subagent 隔离模式** | 子任务独立 + 输出是结构化 | 高(进程隔离) | ❌ "wave worker" 名义上是,实际不是 | ❌ Ralph 现状 |
| **Fan-out 自由竞争** | 永不推荐 | 失控 | `dimension-reviewer concurrency:9` | ❌ keen-fern / zippy-sparrow |

**`ce-executor-isolated` 现状的 wave worker 落在第 4 行"Fan-out 自由竞争"——这是最差的一档**。

### 4.3 业界对 wave/fan-out 的实际态度

| 框架 | 模式 | 关键设计 |
|---|---|---|
| **Claude Code** | Subagent | Task tool + 进程隔离 + mailbox + worktree |
| **LangGraph** | Supervisor | StateGraph 单点 + Send API 派发 + checkpoint 持久化 |
| **CrewAI** | Crew | Manager agent 单点 + 任务队列 + LLM-as-judge 可选 |
| **AutoGen** | GroupChat | Manager 单点轮询 + speaker selection |
| **拓业智询实战** | Supervisor | LangGraph StateGraph + Supervisor 节点 + 5 维 analysis |

**5 个框架全部是 Supervisor / Subagent 模式——没有一家用 fan-out 自由竞争**。**`ce-executor-isolated` 的 `dimension-reviewer concurrency:9` + `aggregate` 是反业界共识的设计**。

---

## 5. 5 份历史诊断的二次审视

### 5.1 已落地机制(2026-06-16-001 U1-U6)

| U | 机制 | 治的失败模式 | 根因是否触动 |
|---|---|---|---|
| U1 | per-turn budget 拆分 | executor 烧 token | ❌ token 控制,非状态共享 |
| U2 | synthetic `wave.worker.failed` provenance | 发件人冒充 | ❌ 安全,非状态共享 |
| U3 | `task.resume` freshness TTL | 旧 resume 误激活 | ❌ 调度,非状态共享 |
| U4 | dimension 7 缩到 4 | 复杂度爆炸 | ⚠️ 部分(降低 worker 数量) |
| U5 | progress-steward hat | loop 失联 | ❌ 用户沟通,非状态共享 |
| U6 | ce-code-review findings | review 质量 | ❌ review 质量,非状态共享 |

**6 个 U 中 5 个治的是 token / 安全 / 调度 / 用户沟通 / review 质量,1 个(U4)间接降低 worker 数量。** **没有任何一个 U 直接治"9 worker 共享状态"的根因**。

### 5.2 草稿 / 计划未开工(8 份需求 + 5 份计划)

| 需求 | 治的失败模式 | 根因是否触动 |
|---|---|---|
| `flow-reliability` R-A5 降级出口 | 收不齐时怎么走 | ❌ 治症状,治不了 worker 互相看不到 |
| `flow-reliability` R-B1 aggregator SLA | aggregator 等多久 | ❌ 治超时,治不了共享 stream |
| `step-handoff` R-A3 re-emit trap | 同一 task 二次发 | ❌ 治 dedup,治不了根因 |
| `step-handoff` R-B3 `is_dual_publish_step_handoff` | 二次 `work.done` 拒收 | ❌ 治 dedup |
| `step-handoff` R-C1 progress/tasks 硬门 | 状态不一致 | ❌ 治一致性检查,治不了共享 stream |
| `wave-dimension-enforcement` R1-R11 | dimension 分配 | ❌ 治维度分配,治不了 worker 看不到 |
| `incomplete_wave_gate` R6 | 收不齐时收摊 | ❌ 治收摊,治不了收不齐的根因 |
| `loop-stability` R-A1 SSOT + R-B1-B4 payload 恢复 | payload 校验 | ❌ 治 payload,治不了 stream |

**8 份需求累计 30+ R 编号,全部治"症状",全部不直接治根因"9 worker 共享 stream"**。**补丁越叠越多,新 bug 比旧 bug 多**。

### 5.3 5 份诊断对账

| 诊断报告 | 报告作者归因 | 本报告归因 |
|---|---|---|
| `2026-06-16-loop-diagnostic-report.md` | human guidance 诱导 agent 发调试事件 + payload 校验 | 同意 payload 校验 + 同意 schema 缺失,但 652s stall 的根因是 worker 共享 stream |
| `2026-06-17-ce-executor-isolated-flow-reliability-plan-loop-synthesizer-stall-diagnosis.md` | `check_semantic_gates` 误分类 + U6 completeness check 缺 enforcement + stall_recovery 截断 | 同意这 3 条都是 bug,但更深根因是"9 worker 共享 stream + aggregate 等不齐"——修了 3 个 bug 之后还会有第 4 个 |
| `2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md` | R6 收摊按设计触发 + U1 留 2 个 P1 residual | 同意 R6 设计正确 + 同意 P1 residual,但更深的疑问是"为什么 4 维 review 跑了 1h47m 没收齐"——**2 维 stall 1h36m 是 fan-out 共享 stream 的副作用,不是 review 慢** |
| `2026-06-16-systematic-review-of-recent-fixes.md` | event_loop 拆分 U1 scaffold 漂移 + schema_refs 目标已侧路达成 | 同意拆分计划可取消 + schema_refs 可废弃,但"修了 6 个 U 之后还剩 5 个新症状"的根因没分析 |
| `2026-06-11-u3-dispatcher-review.md` | U3 dispatcher 边界 | 同意,本报告没新结论 |

**5 份诊断作者都从"机制层 bug"角度找根因——本报告补充第 6 维度"抽象层错配":`dimension-reviewer concurrency:9` 不是 Subagent 模式,注定不稳**。

---

## 6. 3 选 1 路径

### 6.1 路径 A:**保持 `dimension-reviewer concurrency:9` 现状,继续打补丁**

**做法**:完成 8 份需求文档里的 30+ R 编号机制(`incomplete_wave_gate` R6、`wave-dimension-enforcement` R1-R11、`step-handoff` R-B3 二次拒收等)

**预期成本**:2-3 个月纯机制开发,30+ R 编号逐个实施

**预期结果**:
- 短期:失败模式从 5 类减到 2-3 类
- 中期:每加一个补丁冒 1-2 个新 bug(补丁本身是协调机制,协调机制本身又有协调成本)
- 长期:稳定不下来——**根因没动,补丁治不完**

**风险**:已验证(2026-06-13、2026-06-17 两个 loop 实证)——**机制层修不完**。

### 6.2 路径 B:**重写 wave worker 成真 Subagent 模式**

**做法**:
- `loop_runner` 加 worker process 隔离(每个 worker 独立 OS 进程)
- `events.jsonl` 拆主/子流(worker 写自己子流,完事 merge 到主 stream 单点)
- 加 mailbox 通信层(子 agent 异步投递结论,主 agent 单点收口)
- 9 worker 改成 9 个独立 Subagent,每个独立 context + 独立 worktree + 独立工具池

**预期成本**:1-2 个月纯基础设施重写

**预期结果**:
- 短期:`dimension-reviewer concurrency:9` 真正变成 9 个独立 LLM 调用,共享状态问题解决
- 中期:Subagent 本身有新的失败模式(进程通信失败、mailbox 投递失败、merge 冲突)
- 长期:需要继续做 Supervisor 单点控制才能稳定

**风险**:**Subagent 模式不是银弹,业界 Subagent 也有自己的失败模式**(MAST 论文实证)。做了 Subagent 之后还需要做 Supervisor。

### 6.3 路径 C:**`ce-executor-isolated` 改用 `ce-executor-serial` 模式(单 LLM 串行 N 维 review)**

**做法**:
- `dimension-reviewer` 改无 `concurrency`,单 LLM 串行 4 turn 审 4 维
- 砍 `review-synthesizer aggregate: wait_for_all`
- `review-coordinator` 改 4 维顺序 emit `review.dimension.ready`(c) → (s) → (p) → (u) → 最后一维后 emit `review.dimensions.complete`
- `review-synthesizer` 改 trigger on `review.dimensions.complete`,单 LLM 调,读 N 份 findings file 合成
- `ce-executor-serial.yml` 14:42 已落地,直接套拓扑
- 5 份 wave-* 需求文档归档(标 `[ARCHIVED 2026-06-17]`),不生成新 plan
- 8 份 wave-* 文档的 wave 模块代码保留(wave_tracker / wave_detection / wave_prompt / wave dispatcher)作为**通用 fan-out 基础设施**,但不挂到任何 preset

**预期成本**:半天改 `ce-executor-isolated.yml` + 1-2 天补 BDD 测试

**预期结果**:
- 短期:fail 模式从 5 类直接降到 0 类(单 LLM 串行无共享状态冲突)
- 中期:`progress-steward` + `plan-gate` + `fixer` + `debug-resolver` 流水线已经能稳定
- 长期:`ce-executor-isolated` 和 `ce-executor-serial` 合并或保留两套(场景 B 串行 review + 场景 A supervisor 调度,用户选)

**风险**:**4 维 review 串行 4 turn 比 fan-out 慢 wall time 3-4 分钟**——但 `keen-fern` 1h47m / `zippy-sparrow` 1h04m 的 wall time 是 fan-out 失败导致的"卡死",不是 fan-out 成功导致的"快"。

### 6.4 推荐:路径 C

**理由**:
1. **`ce-executor-serial.yml` 14:42 已落地,直接套用**——成本最低
2. **5 份失败模式全部归零**——单 LLM 串行无共享状态冲突
3. **符合业界 Supervisor 模式共识**——Supervisor 集中调度,不是 fan-out 自由竞争
4. **`progress-steward` + `plan-gate` + `fixer` + `debug-resolver` 流水线已经稳定**——单 LLM 串行 review 套在流水线上是自然延伸
5. **可逆**——`ce-executor-wave.yml` 保留(标 [DEPRECATED]),将来要做真 Subagent 模式可以重启路径 B

**实施时间表(估算)**:
- 改 `ce-executor-isolated.yml`:0.5 天
- 补 BDD scenarios(串行 4 维 review 跑通):1-2 天
- 归档 8 份 wave-* 文档:0.5 天
- **总计 2-3 天**,vs 路径 A 的 2-3 个月

---

## 7. 关键决策点

| 问题 | 推荐答案 | 理由 |
|---|---|---|
| 砍 `ce-executor-wave.yml` 吗? | ✅ 砍(标 [DEPRECATED]) | 从来没跑过,拓扑和 isolated 重复 |
| 砍 `dimension-reviewer concurrency:9` 吗? | ✅ 改 `concurrency: 1`(单 LLM 串行) | 走路径 C |
| 砍 `review-synthesizer aggregate` 吗? | ✅ 砍 | 走路径 C |
| 砍 wave_tracker / wave_detection / wave_prompt / wave dispatcher 模块吗? | ❌ 不砍 | 作为通用 fan-out 基础设施保留,不挂 preset |
| 砍 `ralph wave` CLI 吗? | ❌ 不砍 | 通用 fan-out 工具,供非 LLM 场景备用 |
| 砍 8 份 wave-* 需求文档吗? | ❌ 不砍,归档 | 标 `[ARCHIVED 2026-06-17]`,加决策记录 |
| `ce-executor-isolated` 和 `ce-executor-serial` 合并吗? | ⚠️ 暂不合并 | 两个 preset 并存,用户按场景选,半年后再根据使用率决定 |
| 8 份需求里 5 条可挪用机制怎么办? | ✅ 挪到 event_policy / event_loop 通用层 | 通用 gate / hat_idle_alert / 同 task 二次拒收 / 降级出口思路 / 进度落后阈值 |

---

## 8. 附录

### 8.1 关键代码引用

| 文件 | 行 | 内容 |
|---|---|---|
| `crates/ralph-core/src/wave_prompt.rs` | 9-20 | `WaveWorkerContext` 定义,4 字段 |
| `crates/ralph-core/src/wave_prompt.rs` | 30-95 | `build_wave_worker_prompt` 函数,加 5 段 prompt 头(Instructions / Wave Context / Your Task / Publishing / Constraints) |
| `presets/en/ce-executor-isolated.yml` | 1052-1123 | `dimension-reviewer` hat 配置,`concurrency: 9` |
| `presets/en/ce-executor-isolated.yml` | 1285-1293 | `review-synthesizer` aggregate 配置 |
| `presets/en/ce-executor-serial.yml` | 944-1090 | `dimension-reviewer` 串行版配置,无 concurrency |
| `presets/en/ce-executor-serial.yml` | 7-15 | "**no `concurrency`, no wave dispatcher, no `wave_id`**" 注释 |

### 8.2 关键诊断 / 需求 / 计划文件

| 路径 | 类型 | 关键内容 |
|---|---|---|
| `docs/report/2026-06-16-loop-diagnostic-report.md` | 诊断 | 652s stall 实证 |
| `docs/report/2026-06-17-ce-executor-isolated-flow-reliability-plan-loop-synthesizer-stall-diagnosis.md` | 诊断 | 11→4 维 stall 实证 |
| `docs/report/2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md` | 诊断 | 1h47m 12s 二次 work.done 实证 |
| `docs/brainstorms/2026-06-17-ce-executor-flow-reliability-requirements.md` | 需求 | 5 R-A/B 机制,治共享状态补丁 |
| `docs/brainstorms/2026-06-17-ce-executor-step-handoff-requirements.md` | 需求 | 4 R 机制,治 re-emit 漏洞 |
| `docs/brainstorms/2026-06-17-wave-dimension-assignment-enforcement-requirements.md` | 需求 | 6 R 机制,治 dimension 分配 |
| `docs/plans/2026-06-17-002-feat-ce-executor-serial-review-plan.md` | 计划 | `ce-executor-serial` 实现 |

### 8.3 业界参考资料

| 来源 | 关键结论 |
|---|---|
| knqiufan 博客园 2026-04-09《拆解 Claude Code SubAgent》 | "**子代理是独立 Claude 实例,执行完毕中间过程消失**" |
| CSDN 2026-05-21《Claude Code 源码架构拆解》 | "**worktree isolation for parallel execution + team coordination with async mailboxes**" |
| CSDN 2026-06-13《AI Agent 并发调度工程实战》 | "**Datadog 2026 报告:5% LLM 调用报错,60% 是 rate limit 重试风暴**" |
| CSDN 2026-06-06《字节面试 Multi-Agent 教程》 | "**三个 Agent 同时分析同一家公司,WriterAgent 写报告怎么合并?沉默。**" |
| 拓业智询实战 2026-06-06 CSDN | "**Supervisor 节点是单一控制点,子 agent 是 Supervisor 状态机的一部分**" |
| MAST 论文 BuildingTrust '25 | "**multi-agent LLM 系统失败是结构性的,不是工程 bug**" |

### 8.4 待办事项(本报告产出后)

- [ ] 用户决策:走路径 A / B / C
- [ ] 如走 C:改 `ce-executor-isolated.yml` 让 `dimension-reviewer` 串行 4 维
- [ ] 如走 C:补 BDD scenarios 覆盖新拓扑
- [ ] 8 份 wave-* 文档归档(标 `[ARCHIVED 2026-06-17]`)
- [ ] `ce-executor-wave.yml` 标 `[DEPRECATED]`
- [ ] 5 条可挪用机制挪到 event_policy / event_loop 通用层(路径 A/B/C 共用)

---

**报告结束**
