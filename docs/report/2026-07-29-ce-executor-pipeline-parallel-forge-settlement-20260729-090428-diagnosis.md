---
preset: builtin:ce-executor-pipeline
loop_id: 2026-07-29-001-fix-parallel-forge-static-wave-settlement-plan
diagnosis_date: 2026-07-29
run_dir: /home/chaowen/Dev/agent_tools/ralph-orchestrator/.worktrees/2026-07-29-001-fix-parallel-forge-static-wave-settlement-plan
final_commit: 33e4532b
loop_status: completed-with-blocked-verdict
total_iterations: 3
duration: 1h 1m 46s
events_count: 5
execution_capabilities: [single-chain]
diagnostics_mode: FULL
history_search: disabled
preset_kind: linear-pipeline
---

# 2026-07-29-ce-executor-pipeline-parallel-forge-settlement-20260729-090428-diagnosis

## §0 强制四问摘要(执行层)

| # | 问题 | 答 |
|---|------|-----|
| 1 | 执行与 OPAC(诊断模式 + 置信度) | diagnostics=FULL,OPAC 置信度 = 高。完整 5 事件 + recovery.jsonl + ralph log + supervisor.db schema 都到位 |
| 2 | 基座机制是否生效 | **机制全部按设计生效**,但**设计选择与用户预期相反**:`work.failed` 是终态死信,不是 retry 信号 |
| 3 | 编排是否合理 | 编排本身闭环(plan-reviewer→executor→reporter),但**没有"重派 executor"的拓扑边**;reporter 直接 short-circuit 到 blocked |
| 4 | 归因(preset / mechanism / agent / compound) | **preset 100%**(根因置信度 90):`work.failed` 在 flow_declaration 上没有定义前进路径,`retry_budget` 配置项未被 runtime 消费;**agent 10%**:executor 0 业务事件本身(plan 内 U1-U3 已 commit,但 executor 那一轮没发任何 `work.*` 事件) |

## §1 用户问的两件事(精确回答)

### 1.1 为什么 executor 发 work.failed 没有拦住?

**答**:`work.failed` 在 `ce-executor-pipeline` preset 里**不是可拦截信号,而是 reporter 终态收集信号**。

**证据链**(preset → runtime 双重账本一致,confidence 90):

- **preset 声明**(来源:`presets/en/ce-executor-pipeline.yml:5214`):
  ```yaml
  reporter:
    triggers: ["align.done", "plan.blocked", "work.failed", ...]
    publishes: ["report.done", "LOOP_COMPLETE"]
    terminal_events: ["report.done", "LOOP_COMPLETE"]
  ```
  reporter 的 `triggers` 显式列 `work.failed`,description 写明 "**Sole consumer of plan.blocked / work.failed / stabilization.blocked**"。preset 作者把 work.failed 设计成"reporter 收口的合法输入",**不是"拦下来重做"**。

- **preset 注释**(同上文件第 17 行):
  > `work.failed` is reserved for true dead-ends (zero deliverable commits, ...)
  > 注释明确说 work.failed 是"真死信"——不是 retry trigger。

- **实际事件流**(来源:`.ralph/events-20260729-080237.jsonl` line 3):
  ```json
  {"hat":"executor","source":"executor","system_injected":true,
   "topic":"work.failed","ts":"2026-07-29T09:02:09.522626537+00:00",
   "payload":{"hat":"executor","message":"Hat 'executor' emitted no events; orchestrator injected default topic 'work.failed'","reason":"default_publishes","topic":"work.failed"}}
  ```
  `system_injected: true` + `reason: default_publishes` —— 这条 work.failed **不是 executor 主动发的**,而是** executor 那一轮 0 业务事件,orchestrator 按 `default_publishes` 兜底注入的**。

- **runtime 兜底逻辑**(来源:`crates/ralph-cli/src/loop_runner/runner.rs:4686-4702`):
  ```rust
  // Inject default_publishes for active hats only when agent wrote no events.
  // Skip default_publishes when hard gate triggered — the agent explicitly
  // claimed to emit and we want it to learn to do so, not be bailed out.
  ```
  这是 **hat-level 兜底**,executor 的 `default_publishes: "work.failed"`(`ce-executor-pipeline.yml:2095`)被消费,符合 preset 语义。

- **下家消费**(来源:同 line 5214 reporter triggers,加 `flow-authority.jsonl` 顺序):
  - executor 发 work.failed → reporter 被触发(reporter 的 trigger 命中)
  - reporter 发 `report.done`(`verdict: blocked`)+ `LOOP_COMPLETE`
  - loop 在 5 秒内收口

**结论**:`work.failed` **没有被拦下**是**按设计** —— preset 作者把它当 reporter 终态信号,不是 retry trigger。所谓"拦住"在 ce-executor-pipeline 拓扑下不存在该边。

---

### 1.2 为什么没有 retry?

**答**:`work.failed` 在 runtime flow_declaration 里被显式列入 `NON_TRANSITION_TOPICS` 白名单,**任何 work.failed 都不会前进步骤**;preset 里 executor 配的 `retry_budget: 3` 是**死代码**。

**证据链**(preset → runtime 双重账本一致,confidence 90):

- **preset 配置**(来源:`presets/en/ce-executor-pipeline.yml`,在 `work.failed` 状态机的 allow_rules 段):
  ```yaml
  retry_budget: 3
  retry_budget: 3   # 行 102/113 重复两次
  ```
  preset 作者**的确**为 work.failed 路径配了 retry_budget(注释写明 "Executor owns work.done / work.failed retry path")。

- **runtime 消费侧**(来源:`crates/ralph-core/src/event_loop/mod.rs:14880-14918`):
  ```rust
  const NON_TRANSITION_TOPICS: &[&str] = &[
      "work.done",
      "work.failed",       // <-- 在白名单内
      "work.ready",
      ...
  ];
  if NON_TRANSITION_TOPICS.contains(&accepted_topic) {
      return None;        // <-- 直接返回,不前进步骤
  }
  ```
  `advance_plan_step` 收到 work.failed **直接 return None** —— 当前 plan 步骤(本 run 中是 executor step)既不前进也不后退,loop 也不重派。

- **没有 plan.blocked 兜底**:`work.failed` 不在 preset `recover_current_plan_step`(`crates/ralph-core/src/event_loop/mod.rs:14957`)的任何合法转移目标上 —— flow_declaration 没声明"work.failed → back to plan-reviewer"或"work.failed → retry executor"这样的边。

- **executor 0 业务事件的根因**(来源:`diagnostics/channel-routing-fallback-2026-07-29T09-02-09.md` + `ralph.log` ERROR 行 09:02:09):
  ```
  hat-channel routing fallback ... hat=executor
  reason=hat_channel_empty_after_activation
  ```
  isolated 模式下 executor 的 hat-channel(`.ralph/agent/events-hat-executor-*.jsonl`)是空文件 —— executor 的 backend child_pid 4123216 在 spawn 后**没有任何业务事件落盘到 hat-channel**。原因未知(可能:backend 在 plan 大文档上下文下提前终止 / token 用尽 / 工具调用失败但未上报),但 **default_publishes 兜底按设计接住了**,所以 loop 没崩。

- **git log 对账**(来源:`git log --oneline -20`):
  ```
  33e4532b feat(core): U2 EnsureTaskBatch 静态 schedule 校验   ← 本 plan 自身 U2
  54951cc4 fix(test): U1 补 5 处 FlowStepConfig literal transition_emits 字段
  20191cb3 feat(core): U1 step-local transition_emits 支撑重复 wave 生命周期
  d737b9b7 docs(plan): 新增 parallel-forge 静态 wave 结算与失败恢复计划
  ```
  本 plan 的 U1-U2 实际**已 commit**,但 commit message 显示这是 "parallel-forge 静态 wave 结算" 计划 —— **此 run 的 plan 标题和 commit 标题都聚焦在 parallel-forge,而 ce-executor-pipeline 的 retry 拓扑是另一回事**。executor 死信是上层 runtime 行为,与本次 U1-U2 的 commit 内容(EnsureTaskBatch 静态 schedule 校验)无直接因果。

**结论**:`retry_budget: 3` 在 preset 里是**装饰性配置**,**没有任何 runtime 代码路径消费它**。work.failed 触发后,plan 步骤被 `NON_TRANSITION_TOPICS` 锁死,reporter 接管发出 LOOP_COMPLETE(reason: blocked)。**没有"重派 executor"的拓扑边** —— 这不是 bug,是 preset + runtime 联合契约的现状。

---

## §2 用户问的两件事(回答完之后的延伸)

### 2.1 这次 run 的最终定性

`summary.md` 写 `Status: Completed successfully`,**但 verdict 是 `blocked`**(来源:`report.done` payload `verdict: blocked`)。这看似矛盾,实际是 ralph 的语义:

- `Status: Completed successfully` = loop 收口了(LOOP_COMPLETE 被消费)
- `verdict: blocked` = 业务结论是 blocked(因为 executor 0 业务事件)

**真实定性**:**计划目标未达成,loop 干净收口**。U1-U2 的代码 commit 是上次 run(2026-07-28 `parallel-forge-primary-20260728-110733-diagnosis`)的产物,本次 run 没有产出新 commit(`33e4532b` 是上次 run 末尾的 commit,本次 run 期间 working tree 无新 commit)。

### 2.2 派生结论(用户没问但有信号)

1. **executor 0 业务事件 + hat-channel 空** = 双重证据指向"executor backend 这一轮根本没产出"。建议下一步:在 supervisor 视角下,把 `hat_channel_empty_after_activation` 与 work.failed default_publishes 关联成同一个 failure_kind(recovery.jsonl 已记到 envelope 但未归一),这样后续能按"silent backend death"聚合。

2. **retry_budget 是死配置** = preset schema 缺少 `runtime_consumed: true` 标记。下一轮 preset_lint 可加 finding_id: `preset.retry_budget_unused`(参见 `parallel-forge` 的 `coordinator work.failed 死信` 教训已落 `docs/solutions/`,见历史 —— **本报告 history=disabled,不引历史**)。

3. **本次 run 的 plan 真正目标**(EnsureTaskBatch 静态 schedule 校验,U1-U2)在 `33e4532b` 之前**已完成** —— 跑这次 loop 时 executor 的 work 上下文没东西可做,产出空缺是预期,**但 default_publishes 兜底+reporter 终态仍按机制跑完**。

---

## §3 一句话总结

**`work.failed` 没被拦** = reporter 的 trigger,preset 设计如此。**没有 retry** = `work.failed` 在 `advance_plan_step` 的 `NON_TRANSITION_TOPICS` 白名单里,preset 的 `retry_budget: 3` 没人消费。两件事都是**按设计**,不是 bug。Loop 干净收口(reporter 5 秒内出 report.done + LOOP_COMPLETE),verdict=blocked,工作流自洽。

---

## §4 产物清单(本次 run 范围内)

- `events-20260729-080237.jsonl` — 5 事件(work.start / plan.ready / work.failed / report.done / LOOP_COMPLETE)
- `recovery.jsonl` — 2 envelope(agent_doc_sync.sync_up_to_date + missing_event_gate.default_publishes_injected)
- `channel-routing-fallback-2026-07-29T09-02-09.md` — executor hat-channel 空
- `flow-authority.jsonl` — 7 行,plan.ready×2 / queue.advance×2 / work.ready×2 / report.done×1
- `ledger.jsonl` — 4 行,iteration 1→3 + completion_requested/honored
- `supervisor.db` — 表结构完整,0 行(本 run 走 linear pipeline,未走 wave)
- `agent/{summary,handoff,decisions,context,resume-context}.md` — 全部到位
- `loops.json` — `loops: []`(loop 已结束)
- `git log` — HEAD=`33e4532b`,本 run 无新 commit

## §5 历史关联

`N/A (history disabled)`(用户明确不要历史)
