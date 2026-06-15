---
date: 2026-06-02
type: ce-debug
diagnostic-of: ralph-log-monitor loop primary-20260602-070953
preset: ce-executor
plan: 2026-06-02-002-feat-merge-duplicate-cases
subject: 为什么 U2/U3/U4 没继续执行就 LOOP_COMPLETE 了
---

# ce-executor 提前退出诊断报告

> 📅 2026-06-02 | 🔖 loop primary-20260602-070953 · plan 2026-06-02-002-feat-merge-duplicate-cases

---

## 1. TL;DR — 一句话定位

**`ce-executor.yml` 预设只有"work.start → ... → LOOP_COMPLETE"的单程链路，没有"回到 coordinator 创建下一步 tasks"或"回到 executor 继续实施下一 step"的回路**。Coordinator 按"只创建当前 step 的 tasks"规则只建了 U1，U1 完成后任务池空、`verify_tasks_complete()` 返回 `Ok(true)`，`LOOP_COMPLETE` 被 ralph 事件循环正常接受——这一切**符合 ralph 机制的设计预期**，是 ce-executor 预设**自身缺少 loop-back 事件回路**的设计漏洞。

下钻结论：

| 关注点 | 结论 | 证据 |
|---|---|---|
| ralph 运行机制 | **本次跑中无未预期行为**；1 处设计契约 mismatch（任务门控 vs ce-executor 任务范围）、1 处潜在脆弱性（HashMap 序 topic 路由）均未触发 | 见第 6 节 6.1-6.7 逐项分析 |
| `ce-executor.yml` 预设 | **有结构性漏洞** | 8 个 hat 的事件流向是单程 DAG，无任何 hat 在 review/ship/reporter 后回环到 coordinator/executor |
| U2/U3/U4 没跑 | **预期内** | coordinator 严格按指令"only create current step's tasks"只创建了 U1，U2 任务从未被创建、从未进入任务池 |
| 完成 1 个子任务就 done | **预期内** | 1 个 U1 任务 closed 后，`tasks.jsonl` 中无 open task → `verify_tasks_complete()` 接受 LOOP_COMPLETE |
| verdict_gate | **未触发** | shipper 发布 `pass_or_fail=pass`、`verdict=pass_with_residuals`，verdict_gate 配的是 `pass_or_fail=="fail"`，未匹配 |
| Hat 切换本质 | Coordinator 模式下是**虚拟的 prompt 角色切换**（同一 ralph 后端进程） | `event_loop/mod.rs:1370-1402 next_hat()` 永远返回 "ralph"；`1600-1820 build_prompt()` 调 `determine_active_hat_ids` 决定 prompt 主体 |
| 状态机 | **未启用** | ce-executor.yml 无 `state_machine` 字段，默认 disabled；plan 状态无结构化追踪 |
| Topic 路由 | HashMap 序确定首个匹配 hat，**潜在非确定性**（本次唯一匹配，未踩到） | `hat_registry.rs:333-358 get_for_topic()` |

---

## 2. 事实链：从 work.start 到 LOOP_COMPLETE 的完整事件流

来源：`/home/chaowen/Dev/log_analyze/ralph-log-monitor/.ralph/events-20260602-070953.jsonl`（17 条事件）

```text
work.start                      (loop)
   ↓
coordinator                     (complexity=small, plan_name=..., task_id=task-1780384426-fec8)
   ↓ work.ready
executor                        (U1 实施 +81/-2, commit d83c36f8)
   ↓ work.done
review-coordinator              (wave w-...-0, 4 dimensions: correctness/testing/standards/requirements)
   ↓ review.wave.ready ×4
dimension-reviewer ×4           (4 个 review.dimension.done, 0~2 findings each)
   ↓
review-synthesizer              (P1 ×1, P3 ×3, safe_auto=1, gated_auto=2)
   ↓ review.failed
fixer                           (fix #3 safe_auto, 1 commit cc3690d2)
   ↓ fix.applied
review-coordinator              (deferred_findings #1/#2 to U2, emit review.passed)   ← 关键节点
   ↓ review.passed
shipper                         (verdict=pass_with_residuals, pass_or_fail=pass)
   ↓ review.complete
reporter                        (report.done: scope_pending=U2/U3/U4, scope_shipped=U1)
   ↓
reporter                        (LOOP_COMPLETE)                                       ← 退出点
```

**关键观察**：

1. **整个链路只有 1 个任务**：`task-1780384426-fec8`（U1），状态 `closed`，无 pending
2. **从未出现 `queue.advance` 事件**：8 次迭代、17 条事件，没有任何 `queue.advance`
3. **从未出现第二次 `work.ready`**：executor 只被激活 1 次
4. **review-coordinator 在 fix.applied 后发出 `review.passed`**：U1 范围内已 pass，**没有触发任何回到 executor 的事件**

---

## 3. 因果链：从根因到症状（无 gap）

### 3.1 设计层（root cause 层级 0）：ce-executor 预设事件图无回路

`presets/ce-executor.yml` 中各 hat 的 `triggers` 定义：

| Hat | Triggers | Publishes | 在链路上的位置 |
|---|---|---|---|
| coordinator | `["work.start"]` | `["work.ready", "work.failed"]` | 入口 |
| executor | `["work.ready", "queue.advance", "work.retry"]` | `["work.done", "work.failed", "queue.advance"]` | 实施 |
| review-coordinator | `["work.done", "fix.applied"]` | `["review.wave.ready", "review.passed"]` | 评审发起 |
| dimension-reviewer | `["review.wave.ready"]` | `["review.dimension.done"]` | 并行评审 |
| review-synthesizer | `["review.dimension.done"]` | `["review.passed", "review.failed", "review.complete"]` | 评审汇总 |
| fixer | `["review.failed"]` | `["fix.applied", "fix.exhausted"]` | 自动修复 |
| shipper | `["review.passed", "review.complete", "fix.exhausted"]` | `["REVIEW_COMPLETE"]` | 发货 |
| reporter | `["REVIEW_COMPLETE"]` | `["report.done", "LOOP_COMPLETE"]` | 汇报 |

**事件图 DAG**（实际激活路径）：

```text
work.start ──→ coordinator ──work.ready──→ executor ──work.done──┐
                                                                  ↓
                                                       review-coordinator
                                                                  ↓ review.wave.ready
                                                       dimension-reviewer ×4
                                                                  ↓ review.dimension.done ×4
                                                       review-synthesizer
                                                                  ↓ review.failed
                                                              fixer
                                                                  ↓ fix.applied
                                                       review-coordinator
                                                                  ↓ review.passed  ★ 第 2 圈 review-coordinator
                                                              shipper
                                                                  ↓ review.complete
                                                             reporter
                                                                  ↓ report.done
                                                                  ↓ LOOP_COMPLETE  ★ 终止
```

**唯一被二次激活的是 `review-coordinator`（fix.applied 后）**，但它只发 `review.passed`，**没有回环到 coordinator 或 executor**。

### 3.2 实现层（root cause 层级 1）：executor 知道要做 queue.advance，但没机会执行

`ce-executor.yml:246`（executor 指令最后一段）：

> "If more steps remain in plan.md → create next step's runtime tasks, publish `queue.advance`"

这条指令**写在了 executor 自己的 instructions 里**，但 executor 的 triggers `["work.ready", "queue.advance", "work.retry"]` **没有任何一个事件会在 review/fix/ship/reporter 链路上产生**：

- `work.ready` 只在 coordinator 处理 `work.start` 时发，**coordinator 不会再次激活**
- `queue.advance` 没有任何下游 hat 会发布
- `work.retry` 不会自然产生

**等价地说**：executor 写了"我应该 publish queue.advance"，但没有任何上游 hat 会触发它再次激活。指令和事件图是矛盾的。

### 3.3 实现层（root cause 层级 2）：review-coordinator 不会在 U1 完成后驱动 U2

`ce-executor.yml:298` 规定 review-coordinator 在 diff 为空时直接发 `review.passed`；在 351-353 段规定要发 `ralph wave emit`——但**这些事件全部终止于 shipper**。

review-coordinator 没有指令要"检查 plan 进度"、"还有 step 时触发 executor 继续"——它的整个 mental model 是"对当前 diff 做评审"，**不是驱动 plan 推进**。

### 3.4 实现层（root cause 层级 3）：reporter 直接发 LOOP_COMPLETE

`ce-executor.yml:1005-1006`：

> "If `pass_or_fail` from the `REVIEW_COMPLETE` payload is `'fail'`, the reporter MUST NOT publish `LOOP_COMPLETE`. ... Otherwise (pass / pass_with_residuals), publish `LOOP_COMPLETE` after `report.done`"

shipper 给的 `pass_or_fail=pass` → reporter 直接发 `LOOP_COMPLETE`。reporter 也**没有任何指令要"检查 plan 是否还有 pending step"**。

### 3.5 校验层（ralph 事件循环的行为）：所有 gate 都通过

`event_loop/mod.rs:1021-1232` `check_completion_event()` 的串行检查：

| 检查 | 结果 | 证据 |
|---|---|---|
| 1. `required_events` 全部发出 | ✓ | `report.done` 已在事件流中（events[16]） |
| 2. `verdict_gate` (review.complete.pass_or_fail != "fail") | ✓ | shipper 发 `pass_or_fail=pass`（events[15]） |
| 3. `workflow_guards` 完整性 | ✓ | 状态机非启用，无 workflow_guards 链 |
| 4. `verify_tasks_complete()` 是否有 open tasks | ✓ 通过 | `tasks.jsonl` 中只有 1 个 task `task-1780384426-fec8`，已 `closed`，0 open |

`verify_tasks_complete()` 源码（`event_loop/mod.rs:2806-2820`）：

```rust
fn verify_tasks_complete(&self) -> Result<bool, std::io::Error> {
    use crate::task_store::TaskStore;
    let tasks_path = self.tasks_path();
    if !tasks_path.exists() { return Ok(true); }    // ★ 没文件 = 完成
    let store = TaskStore::load(&tasks_path)?;
    let current_loop_id = self.current_loop_id();
    let open = Self::filter_tasks_by_loop(store.open(), current_loop_id.as_deref());
    Ok(open.is_empty())                              // ★ open=空 = 完成
}
```

按当前 loop_id 过滤，1 个 task 已 closed → `open.is_empty()=true` → `Ok(true)` → 接受 LOOP_COMPLETE。

**ralph 事件循环的所有 gate 都正常工作**，是 ce-executor 没把 U2 的 task 喂给 ralph——**不是 ralph 的锅**。

---

## 4. 反事实（counter-factual）分析

### 4.1 假如 shipper 给 `pass_or_fail=fail`，会怎样？

verdict_gate 拒绝 LOOP_COMPLETE，注入 `task.resume` 事件（events 1058-1062、1083-1097）→ 循环继续 → 但**还是不会有 U2 任务**。最终会进入"stale-breaker"机制（3 次连续同 signature 拒绝 → `TerminationReason::LoopStale`）→ 还是退出，但理由变成 "LoopStale" 而非 "CompletionPromise"。

### 4.2 假如 coordinator 一次性创建了 U1+U2+U3+U4 所有 task，会怎样？

会发生：
- U1 task closed 后 executor 不需要 publish queue.advance（因为没有 step advancement 概念）
- U2 task 进入 open 池
- LOOP_COMPLETE 被 `verify_tasks_complete()` 拒绝（仍有 3 个 open task）
- 注入 `task.resume`，循环继续
- 触发到下一个 open task 的 hat（但 ce-executor 没有 hat 监听 "open task" 这一信号）→ 实际上不会自然推进
- 最终也是 stale-breaker 退出

**这条路是错的**：违反 coordinator 现有指令"only create current step's tasks"，且没有 hat 会按 task 自动推进。

### 4.3 假如有 hat 在 review/fix/ship 链路上 publish queue.advance，会怎样？

参考 `presets/pdd-to-code-assist.yml:736` 的 `finalizer` 模式：

```yaml
finalizer:
    triggers: ["review.passed"]
    publishes: ["queue.advance", "implementation.ready", "finalization.failed"]
    default_publishes: "finalization.failed"
    instructions: |
      ## FINALIZER MODE — Step-Wave Queue Exhaustion And Implementation Readiness
      ...
      6. Decide one of:
         - 还有 pending work → publish queue.advance
         - 全部完成 → publish implementation.ready
```

**ce-executor.yml 缺少一个等价于 finalizer 的 gate hat**。这是真正的设计缺口。

---

## 5. 流程有没有问题？—— 问题清单

| 编号 | 位置 | 问题 | 严重度 |
|---|---|---|---|
| **P-1** | `ce-executor.yml:23-29` 整体事件图 | 8 hat DAG 是单程，无回路到 coordinator/executor | **P0（结构性漏洞）** |
| **P-2** | `ce-executor.yml:144-262` executor instructions | "If more steps remain → publish queue.advance" 写在 executor 自己里，但 executor 没有被回环激活 | **P0** |
| **P-3** | `ce-executor.yml:271-356` review-coordinator | 不知道 plan 进度；review.passed 是无条件的"本 step pass"信号，不是"全 plan pass"信号 | **P1** |
| **P-4** | `ce-executor.yml:771-846` shipper | 不知道 plan 进度；直接 publish REVIEW_COMPLETE | **P1** |
| **P-5** | `ce-executor.yml:848-1013` reporter | 不知道 plan 进度；pass_or_fail=pass 就 publish LOOP_COMPLETE | **P1** |
| **P-6** | `ce-executor.yml:56-141` coordinator | 严格"只创建当前 step tasks" → 当前 step 完成后任务池空 → ralph 任务门控失效 | **P1** |
| **P-7** | `event_loop/mod.rs:2806-2820` `verify_tasks_complete()` | 设计假设 "tasks.jsonl 是当前活跃 step 的 source of truth"——在 ce-executor 的"逐步推进"模式下，coordinator 还没建下一步 task 时，这个判断会把"未实施"误判为"已完成" | **P0（与 P-1 联动）** |
| **P-8** | `event_loop/mod.rs:1154-1190` `check_completion_event()` 任务门控 | 同 P-7：从 task 池角度判断"完成"，与"plan 推进"语义错位 | **P0（与 P-1 联动）** |

**P-1 ~ P-6 在 ce-executor.yml 内；P-7 ~ P-8 在 ralph 事件循环源码中**。两者形成"互锁"：

- ce-executor 的 P-6（coordinator 只建当前 step tasks）使 ralph 的任务门控（P-7、P-8）失效
- ralph 的 P-7、P-8 设计假设"task 池是 plan 进度的真相"在 ce-executor 这种"逐步推进"模式下不成立

---

## 6. ralph 运行机制有没有问题？—— 源码层深挖（hat 切换与状态转换）

> 上一版报告的"ralph 机制正确"结论过于草率。本节重新逐项审视：hat 切换、状态机、事件路由、状态追踪。

### 6.1 Hat 切换：Coordinator 模式下是"虚拟"的，不存在真正的进程级状态机切换

`config.rs:1259-1270` 定义了 `HatExecutionMode`：

```rust
pub enum HatExecutionMode {
    Coordinator,  // 默认
    Isolated,
}
```

`config.rs:5366-5411` 确认默认是 `Coordinator`，且 ce-executor.yml 没有覆盖此字段 → 本次运行就是 **Coordinator 模式**。

**Coordinator 模式下，hat 切换的本质**（`event_loop/mod.rs:1370-1402` `next_hat()` + `1600-1820` `build_prompt()`）：

```rust
pub fn next_hat(&self) -> Option<&HatId> {
    let next = self.bus.next_hat_with_pending();
    match self.config.event_loop.execution_mode {
        HatExecutionMode::Coordinator => {
            if self.config.hats.is_empty() {
                next  // solo 模式：返回真实 hat
            } else {
                // ★ 多 hat 模式：永远返回 "ralph"
                self.bus.hat_ids().find(|id| id.as_str() == "ralph")
            }
        }
        ...
    }
}
```

**关键事实**：

1. `next_hat()` 在 Coordinator + 多 hat 模式下**永远返回 `"ralph"`**（不是任何具体的 custom hat）
2. `build_prompt()` 在 1612-1681 行的多 hat 分支里：
   - **取所有 hat 的 pending events**（`for id in &all_hat_ids { pending = take_pending(id); ... }`）
   - 调 `determine_active_hat_ids(&regular_events)`（2378-2412）来**决定"当前应当以哪个 hat 的视角行动"**
   - 用 `determine_active_hat_ids` 返回的第一个 hat 的 `instructions` 作为 prompt 主体
3. "active hat" 是**提示层面的角色扮演**，不是 OS 进程级切换

**这意味着什么**：

- 8 个 hat（coordinator / executor / review-coordinator / dimension-reviewer / review-synthesizer / fixer / shipper / reporter）**共享同一个 ralph 后端进程**（claude CLI 单进程）
- 同一份对话上下文，所有 hat 的指令都在 prompt 里以"## HATS"章节出现
- "hat 切换"是把 `active_hat_ids.first()` 切到下一个角色，把对应指令注入 prompt，然后让同一个 LLM 继续生成
- 4 个 dimension-reviewer 的 wave 行为（`concurrency: 4`）—— 这是 ralph 在单进程内**串行**触发的（看 wave_tracker.rs 与 loop_runner.rs），**不是真的 4 个并行进程**

**正确性的边界**：

- ✅ **正确**：topic 路由（`get_for_topic`）正确把 `work.done` 路由到 `review-coordinator`，`review.failed` 路由到 `fixer` 等等
- ✅ **正确**：prompts 的拼接顺序、active hat 的 instructions 注入符合设计
- ⚠️ **未达预期但非 bug**："hat 切换"在 Coordinator 模式下没有真正的进程隔离、context 隔离、并发隔离
- ❌ **不构成问题**：ce-executor.yml 的 8 hat 设计在 Coordinator 模式下只是"角色脚本"——prompt 看起来是 8 个独立 hat，实际是 1 个 LLM 串行扮演 8 个角色

**ralph 机制本身在这一项上没有 bug**，但其"hat 切换"的语义和 preset 设计者/用户心智模型（"8 个独立 agent"）有落差。

### 6.2 Topic 路由：`get_for_topic` 用 HashMap 迭代顺序，是潜在的非确定性源

`hat_registry.rs:333-358`：

```rust
pub fn get_for_topic(&self, topic: &str) -> Option<&Hat> {
    ...
    // First pass: prefer hats with specific (non-wildcard) subscriptions
    if let Some(hat) = self
        .hats
        .values()                              // ★ HashMap iteration
        .find(|hat| !hat.is_fallback_only() && self.hat_is_subscribed_in_phase(&hat.id, topic))
    {
        return Some(hat);
    }
    ...
}
```

**问题**：

- `hats.values()` 是 `HashMap` 的迭代，**迭代顺序由 HashMap 内部实现决定**，不保证稳定
- 当多个 hat **同时**订阅同一 topic 时，**返回哪个 hat 是不确定的**
- ce-executor.yml 严格做了"一 topic 一订阅者"（coordinator: `work.start` / executor: `work.ready+queue.advance+work.retry` / review-coordinator: `work.done+fix.applied` / ...），所以**这次跑没踩到**

**但**：

- 如果未来 ce-executor 改 1 个 hat 多订阅、或加新 hat 撞了 topic，行为会变成"有时候路由到 A，有时候路由到 B"
- `event_bus.rs:123-138` 的 `publish()` 也用 `for (id, hat) in &self.hats` —— **同一次 publish 内多 hat 收到事件时，处理顺序也是 HashMap 序**——不保证一致

**这是一处真实的设计脆弱性**：hash 序依赖会影响同一 topic 多个订阅者场景的确定性。

**修法（如果你想修）**：

- 在 `from_config()` 时按 `hat_id` 排序，把 hats 放进 `BTreeMap<HatId, Hat>`，或在 `values().collect()` 后排序再迭代
- 或在 `get_for_topic` 里改成"返回所有匹配的 hat 的 sorted list，让调用方决定优先级"

**本次跑是否触发了这个非确定性**：**没有**。`get_for_topic` 在 ce-executor 跑中每个 topic 都唯一匹配 1 个 hat。事件全部正确路由。

### 6.3 状态机（State Machine）：**未启用**——没有"instance lifecycle"强制

`config.rs` 里的 `StateMachineConfig` 默认是 `enabled: false`。ce-executor.yml 没有 `state_machine` 字段，所以本次跑**没启用状态机**。

**状态机本来要做什么**（`state_machine.rs:16-73`）：

- 给每个 event 定义 `business_topics` / `terminal_topics` / `instance_key`
- 每个 instance 有 `open_instances` 和 `closed_instances` 两套状态
- 拒绝违反 lifecycle 的事件（如：closed instance 又收到 business event）
- 提供 `accepted_transition_count` 给 progress fingerprint（用于 stale-breaker）

**为什么不启用**：ce-executor 把"状态推进"完全交给 agent 自觉（agent 写 progress.md、task 系统记录状态）。这与预设设计哲学一致——让 agent 用 markdown 做 soft state。

**这种选择的风险**：

- agent 可能误报"完成"（在 progress.md 里写"Step 1 done"但其实没做完）
- agent 可能漏报（已完成但不更新 progress.md）
- ralph 没办法通过 instance lifecycle 自动发现"实际状态 vs 声明状态"的差异
- 唯一的兜底是 `verify_tasks_complete()`——但这只能看到 `tasks.jsonl`，看不到 agent 自己的 markdown 状态

**本次跑没踩到**：executor 自己 close 了 task，progress.md 被 agent 写为"Current Step: Step 1 — U1"，但这没被 ralph 校验。

### 6.4 状态追踪：loop_state 里的字段

`loop_state.rs:75-138` 列出了所有持久化在 `LoopState` 里的字段：

| 字段 | 用途 | 本次跑的值（推断） |
|---|---|---|
| `iteration` | 循环计数 | 8（0~7） |
| `last_hat` | 上一个被 process_output 调用的 hat_id | "ralph"（Coordinator 模式）|
| `consecutive_failures` | 连续失败次数 | 0（U1 success）|
| `consecutive_blocked` | 连续 blocked 次数 | 0 |
| `task_block_counts` | task 阻塞计数 | 空 |
| `abandoned_tasks` | 放弃的任务列表 | 空 |
| `completion_requested` | 是否收到完成信号 | true（reporter 发 LOOP_COMPLETE）|
| `completion_honored` | 完成是否被接受 | true |
| `hat_activation_counts` | 每个 hat 激活次数 | {coordinator: 1, executor: 1, review-coordinator: 2（work.done + fix.applied 各 1 次）, dimension-reviewer: 4, review-synthesizer: 1, fixer: 1, shipper: 1, reporter: 2（REVIEW_COMPLETE → report.done + LOOP_COMPLETE）} |
| `exhausted_hats` | 达到 max_activations 的 hat 集合 | 空（没设 max_activations）|
| `last_active_hat_ids` | 上一次激活的 hat 列表 | 切换的 |
| `seen_topics` | 整个 loop 见过的话题 | {work.start, work.ready, work.done, review.wave.ready, review.dimension.done, review.failed, fix.applied, review.passed, review.complete, report.done, LOOP_COMPLETE} |
| `last_verdict_payload` | verdict_gate topic 最后一次的 payload | shipper 的 review.complete（pass_or_fail=pass）|
| `completion_rejection_signature` | 最后一次被拒的 signature | 未被拒 |
| `consecutive_completion_rejections` | 连续被拒计数 | 0 |
| `state_machine_runtime_state` | 状态机运行时态 | **None**（未启用） |
| `policy_runtime_state` | event_policy 运行时态 | **None**（未启用） |
| `workflow_progress` | workflow_guards 进度 | 空（未启用） |

**关键观察**：

- `state_machine_runtime_state: None` —— 没状态机，**没有"plan 在哪个 step"这种结构化状态**
- `last_active_hat_ids` —— 仅用于 `default_publishes` 注入的兜底，**不用作完成判定**
- `seen_topics` —— 用于 `required_events` 检查，**不与 task 系统联动**

**结论**：loop_state 提供的状态字段**足以驱动 8 hat 单程链路**，但**没有"plan 进度"这种维度的状态**——这正是 ce-executor 缺的东西。

### 6.5 安全网：`default_publishes` 和 `hat_exhaustion`

**`default_publishes` 注入**（`event_loop/mod.rs:2547-...`）：

如果一个 hat 激活后，agent 没写任何事件，ralph 会按 hat 配置的 `default_publishes` 自动注入一条。

ce-executor 各 hat 的 `default_publishes`：

| Hat | default_publishes | 含义 |
|---|---|---|
| coordinator | `work.failed` | 解析失败时 |
| executor | `work.done` | 实施完成时 |
| review-synthesizer | `review.complete` | 评审失败但 fix 完了 |
| fixer | `fix.exhausted` | 修不动了 |
| shipper | `REVIEW_COMPLETE` | 最终通过 |
| reporter | `report.done` | 报告已写 |
| review-coordinator | （未设） | 缺省行为：什么都不发（agent 必须显式 emit）|
| dimension-reviewer | （未设） | 同上 |

**`hat_exhaustion`**（`event_loop/mod.rs:2465-2511`）：

如果 hat 设了 `max_activations`，激活次数超限后：
1. 该 hat 的 pending events 被丢弃
2. 注入 `<hat_id>.exhausted` 事件（一次性）

ce-executor 没设 `max_activations`（默认 None）→ 不限制。所以 review-coordinator 可以被无限次激活（理论上 work.done → fix.applied → review.passed 之后没人再 emit fix.applied 触发，但兜底机制存在）。

**这两个安全网在本次跑中没起作用**，但属于"机制正常"的一部分。

### 6.6 任务系统：`tasks.jsonl` 的真相

`task_store.rs` / `task.rs` 实现了 runtime task 系统。关键点：

- task 有 `status`: `pending` / `started` / `closed`（failed 在某些 flow 里也走 closed）
- task 有 `loop_id`，可以被 `verify_tasks_complete` 过滤到当前 loop
- `open()` 返回所有非 closed 的 task

**关键设计假设**（这点我之前没看清）：

- ralph 假设 `tasks.jsonl` 是"当前活跃工作"的真相
- ce-executor 让 coordinator **只创建当前 step 的 task** → `tasks.jsonl` 永远只反映"当前 step"
- 当 U1 完成后 U2 还没创建 → 任务池空 → `verify_tasks_complete` 通过 → LOOP_COMPLETE 接受

**这是设计 mismatch**：
- ralph 的契约："`tasks.jsonl` 反映工作池，池空 = 没事可做"
- ce-executor 的契约："coordinator 按需扩展工作池"

**修法（如果你想修 ralph）**：让 `verify_tasks_complete()` 支持"plan manifest 兜底"——如果 hat registry 里有声明"plan-driven"模式，并且有 plan 文件路径，去查 plan.md 的 `## Implementation Units` 列表，对比已 closed 的 task key 集合，看是否还有未实施的 unit。这是新功能，不是修 bug。

### 6.7 综合结论

| 关注点 | 状态 | 备注 |
|---|---|---|
| Hat 切换机制 | 正确（Coordinator 模式是虚拟切换，符合设计） | 用户心智模型可能与实现有落差，但非 bug |
| Topic 路由 | 正确（本次跑每个 topic 唯一匹配） | **潜在非确定性**：HashMap 序；本次未触发 |
| 状态机 | 未启用 | 风险：plan 状态无结构化追踪 |
| 状态追踪 | 字段齐全但缺"plan 进度"维度 | 设计取舍 |
| `default_publishes` | 正常 | 安全网 |
| `hat_exhaustion` | 正常 | 没设 `max_activations` 故未触发 |
| 任务门控 | 正确（按设计） | 设计契约与 ce-executor 不匹配 |
| `required_events` | 正确 | `report.done` 已发 |
| `verdict_gate` | 正确 | `pass_or_fail=pass` 未触发 |
| `workflow_guards` | 正确 | 未启用，无 N/A |

**ralph 机制本身在本次跑中**：
- **没有"未预期行为"**（no surprise）
- **有 1 处设计契约与 ce-executor 不匹配**：任务门控假设 tasks.jsonl 是 plan-wide，ce-executor 把 tasks.jsonl 当 step-scoped
- **有 1 处潜在脆弱性**：HashMap 序 topic 路由——本次未触发，但 preset 改动时可能踩到

**判定**：ralph 机制**没有导致** U2/U3/U4 没跑；它是按 ce-executor 的事件图正确执行。**真正的问题是 ce-executor 自身的事件图缺少回路**（P-1）。

---

## 7. presets/ce-executor.yml 怎么改？

### 7.1 推荐方案 A：插入一个"plan gate" hat（参考 pdd finalizer）

参考 `pdd-to-code-assist.yml:736` 的 `finalizer`，在 ce-executor.yml 的 shipper **之前**插入一个 plan-gate hat：

```yaml
plan-gate:
  name: "🚦 Plan Gate"
  description: "Decides queue advancement vs final shipping based on plan progress."
  triggers: ["review.passed", "fix.applied"]
  publishes: ["queue.advance", "plan.complete"]
  default_publishes: "plan.complete"
  instructions: |
    ## PLAN GATE MODE — Step Exhaustion Check
    - Read `progress.md`, `plan.md` in `.agents/scratchpad/ce-executor/{plan_name}/`
    - If `## Completed Steps` covers all numbered steps in `plan.md`:
        - Publish `plan.complete` (triggers shipper)
    - Else:
        - Publish `queue.advance` (triggers executor to create next step's tasks)
    - Stop — do not review or implement
```

并调整 shipper 的 trigger 改为 `["plan.complete"]`。

### 7.2 推荐方案 B：让 review-coordinator 在 review.passed 前自检 plan 进度

更小的改动：把 plan 进度检查内化到 review-coordinator 指令里：

```text
### Plan Progress Check (NEW)
- Read `progress.md` and `plan.md` in `.agents/scratchpad/ce-executor/{plan_name}/`
- If `## Completed Steps` covers all numbered steps:
    - Proceed to publish `review.passed` as before (triggers shipper → LOOP_COMPLETE)
- Else:
    - DO NOT publish `review.passed`
    - Instead publish `queue.advance` (triggers executor to create + run next step's tasks)
```

**优点**：只动 1 个 hat 的 instructions，不用新增 hat 节点；
**缺点**：review-coordinator 的职责被扩张（"评审 + 调度"），与"只评审"的设计哲学略有冲突。

### 7.3 备选方案 C：让 reporter 在 LOOP_COMPLETE 前自检 plan 进度

最小改动：在 reporter 的 instructions 末尾加一段：

```text
### Plan Completion Check (NEW)
- Read `progress.md` in `.agents/scratchpad/ce-executor/{plan_name}/`
- If `## Completed Steps` does NOT cover all numbered steps in `plan.md`:
    - DO NOT publish LOOP_COMPLETE
    - Publish `queue.advance` instead (triggers executor to continue)
- Else:
    - Publish LOOP_COMPLETE as before
```

**优点**：1 个 hat 改 1 段；
**缺点**：语义上"reporter"和"queue advance"职责错位；reporter 不应该决定要不要继续干活。

### 7.4 方案对比

| 方案 | 改动量 | 语义清晰度 | 与 pdd 预设一致性 | 推荐度 |
|---|---|---|---|---|
| **A** | +1 个 hat | 高（职责单一） | 高 | **⭐⭐⭐** |
| **B** | 改 1 个 hat 指令 | 中（review-coordinator 兼任调度） | 中 | **⭐⭐** |
| **C** | 改 1 个 hat 指令 | 低（reporter 越权） | 低 | ⭐ |

**推荐 A**。改动需要同步更新 `ce-executor-zh.yml`（项目 CLAUDE.md 要求 EN/ZH 同步）。

---

## 8. 推荐的回归测试

在 `crates/ralph-core/tests/scenarios/` 或 `crates/ralph-e2e/scenarios/` 加一个 BDD 场景：

**场景：multi-step plan advances correctly**

```yaml
scenario: "ce-executor multi-step plan should not terminate after step 1"
preset: "ce-executor"
plan: |
  ## Step 1 — U1. Add config flag
  ## Step 2 — U2. Implement core function
  ## Step 3 — U3. Wire into caller
expected_events_in_order:
  - work.start
  - work.ready         # U1
  - work.done          # U1
  - review.wave.ready
  - review.dimension.done
  - review.complete
  - queue.advance      # ★ NEW: plan gate decides to advance
  - work.ready         # ★ U2
  - work.done          # U2
  ...
  - LOOP_COMPLETE      # ★ only after all steps
assertions:
  - events.count("LOOP_COMPLETE") == 1
  - events.find("LOOP_COMPLETE").index > events.count("work.done")  # last work.done before LOOP_COMPLETE
  - tasks.jsonl has 0 open tasks at LOOP_COMPLETE time
```

这条测试在当前 ce-executor.yml 下会失败（只有 U1 work.done 就 LOOP_COMPLETE），加完 plan-gate hat 后会通过——可以作为该修复的 acceptance test。

---

## 9. 关键文件与行号索引

| 主题 | 文件 | 行号 |
|---|---|---|
| 事件循环 LOOP_COMPLETE 接受检查 | `crates/ralph-core/src/event_loop/mod.rs` | 1021-1232 |
| 任务完整性检查 | `crates/ralph-core/src/event_loop/mod.rs` | 2806-2820 |
| verdict_gate 检查 | `crates/ralph-core/src/event_loop/mod.rs` | 1075-1099 |
| required_events 检查 | `crates/ralph-core/src/event_loop/mod.rs` | 1032-1063 |
| next_hat() Coordinator 模式总返回 "ralph" | `crates/ralph-core/src/event_loop/mod.rs` | 1370-1402 |
| build_prompt() 多 hat 模式事件合并 + active hat 决定 | `crates/ralph-core/src/event_loop/mod.rs` | 1600-1820 |
| determine_active_hat_ids() | `crates/ralph-core/src/event_loop/mod.rs` | 2378-2412 |
| check_hat_exhaustion() | `crates/ralph-core/src/event_loop/mod.rs` | 2465-2511 |
| record_hat_activations() | `crates/ralph-core/src/event_loop/mod.rs` | 2513-2521 |
| check_default_publishes() | `crates/ralph-core/src/event_loop/mod.rs` | 2547-... |
| process_output() 设置 last_hat | `crates/ralph-core/src/event_loop/mod.rs` | 2598-2605 |
| inject_fallback_event() | `crates/ralph-core/src/event_loop/mod.rs` | 1509-... |
| 缺漏事件追踪 | `crates/ralph-core/src/event_loop/loop_state.rs` | 340-345 |
| last_verdict_payload 记录 | `crates/ralph-core/src/event_loop/loop_state.rs` | 353-359 |
| 完整 LoopState 字段 | `crates/ralph-core/src/event_loop/loop_state.rs` | 75-138 |
| HatExecutionMode 定义 | `crates/ralph-core/src/config.rs` | 1259-1270 |
| HatExecutionMode 默认值 | `crates/ralph-core/src/config.rs` | 1601, 5366-5411 |
| 状态机 | `crates/ralph-core/src/state_machine.rs` | 全文件 832 行 |
| 状态机运行时态 | `crates/ralph-core/src/state_machine.rs` | 60-73 |
| Event origin 验证 | `crates/ralph-core/src/event_origin.rs` | 135-196 |
| Event bus 路由（specific vs fallback） | `crates/ralph-proto/src/event_bus.rs` | 79-149 |
| Event bus next_hat_with_pending (BTreeMap 序) | `crates/ralph-proto/src/event_bus.rs` | 183-188 |
| Hat subscriptions matching | `crates/ralph-proto/src/hat.rs` | 154-182 |
| Hat registry get_for_topic (HashMap 序) | `crates/ralph-core/src/hat_registry.rs` | 333-358 |
| Hat registry can_publish | `crates/ralph-core/src/hat_registry.rs` | 280-288 |
| Per-hat scratchpad | `crates/ralph-core/src/hatless_ralph.rs` | 264-275 |
| 预设 coordinator 指令 | `presets/ce-executor.yml` | 56-141 |
| 预设 executor 指令（含 "publish queue.advance"） | `presets/ce-executor.yml` | 144-262（关键 246） |
| 预设 review-coordinator 指令 | `presets/ce-executor.yml` | 271-356 |
| 预设 review-synthesizer 指令 | `presets/ce-executor.yml` | 545-694 |
| 预设 fixer 指令 | `presets/ce-executor.yml` | 695-769 |
| 预设 shipper 指令 | `presets/ce-executor.yml` | 771-846 |
| 预设 reporter 指令 | `presets/ce-executor.yml` | 848-1013 |
| pdd finalizer（参考实现） | `presets/pdd-to-code-assist.yml` | 736-769 |
| 实际事件流 | `log_analyze/ralph-log-monitor/.ralph/events-20260602-070953.jsonl` | 17 行 |
| 实际任务池 | `log_analyze/ralph-log-monitor/.ralph/agent/tasks.jsonl` | 1 行（U1 closed） |
| Plan 计划文件 | `log_analyze/ralph-log-monitor/.agents/scratchpad/ce-executor/.../plan.md` | Step 1-4 |
| Shipping 记录 | `log_analyze/ralph-log-monitor/.agents/scratchpad/ce-executor/.../shipping.md` | 全文 |
| Report 输出 | `log_analyze/ralph-log-monitor/docs/report/2026-06-02-ce-executor-...-report.md` | 全文 |

---

## 10. 行动项建议

按优先级：

1. **(P0) 修复 ce-executor.yml** — 采用方案 A：插入 `plan-gate` hat；同步更新 `ce-executor-zh.yml`
2. **(P0) 写回归 BDD 场景** — multi-step plan advancement，参考第 8 节
3. **(P1) 给 agent prompt 加显式提示** — 在 ce-executor executor/reviewer/shipper/reporter 指令里加粗提示"检查 `progress.md` 再决定是否继续"
4. **(P1) 写 learning doc** — 在 `docs/solutions/` 下加一条记录"ce-executor 缺少 plan-gate 导致单 step 提前 LOOP_COMPLETE"，便于后续 preset 设计参考
5. **(可选) 增强 ralph 任务门控** — 考虑让 `verify_tasks_complete()` 支持可选的"plan manifest 校验"，让 ralph 也能兜住"task 池空但 plan 未完成"这种场景。这是新功能不是 bug fix，需要先和团队对齐再决定

---

## 附录 A：本报告涉及的所有"facts"溯源

| 报告中的事实 | 直接证据 |
|---|---|
| Loop 跑了 8 次迭代、17 个事件 | `.ralph/events-20260602-070953.jsonl`（17 行）+ `events-history-20260602-070953.jsonl`（iteration=0..8）|
| 8 个 hat 全部按设计执行 | events 中各 `hat` 字段值与 ce-executor.yml 中 hat 名称一一对应 |
| tasks.jsonl 只有 1 个 task | `.ralph/agent/tasks.jsonl`（1 行，唯一 key `ce-executor:...:step-01:config-and-extract-helper`）|
| Task 已 closed | 该 task 的 `status: "closed"`, `closed: "2026-06-02T07:20:48..."` |
| 没有 queue.advance 事件 | grep `queue.advance` events 文件返回 0 行 |
| 没有第二次 work.ready | events 中 `topic:"work.ready"` 仅出现 1 次（第 1 行）|
| shipper 发 pass_or_fail=pass | events[15] `payload.pass_or_fail: "pass"`, `verdict: "pass_with_residuals"` |
| verdict_gate 配置是 "pass_or_fail==fail" | `presets/ce-executor.yml:32-38` |
| ralph 任务门控实现 | `event_loop/mod.rs:2806-2820` (verify_tasks_complete) + 1154-1190 (调用) |
| pdd finalizer 模式 | `presets/pdd-to-code-assist.yml:736-769` (finalizer 触发 review.passed，发布 queue.advance) |

## 附录 B：anti-pattern 检查

按 ce-debug 框架 anti-pattern 自查：

- ❌ "Quick fix for now, investigate later" — 已完成完整因果链，方案 A 是结构性修复
- ❌ "This should work"（无预测）— 方案 A 的预测写在第 8 节 BDD 场景断言里
- ❌ "Let me just try..."（无假设）— 报告基于源码 + 事件数据 + cross-preset 模式对比

无 anti-pattern 违反。
