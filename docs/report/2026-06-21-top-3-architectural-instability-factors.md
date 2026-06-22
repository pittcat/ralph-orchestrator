# Top 3 架构不稳定因素（设计层面）

> 分析范围：`crates/ralph-core/src/event_loop/mod.rs`、`event_origin.rs`、`rejection.rs`、`loop_state.rs`、`review_step_state.rs`、`state_projector/`、`hat_handoff/`、`step_handoff/progress_task_gate.rs`、`execution_contract.rs`、`hatless_ralph.rs`，以及 `presets/en/ce-executor-serial.yml`。所有行号与函数名均来自 2026-06-21 的 `main` 分支代码。

---

## 1. 验证链的「自指回环」—— `task.resume` 经 EventBus 驱动 agent 反复 emit 同类错误

### 一句话
系统用 `task.resume` 治疗 rejection，但 `task.resume` 只是被发到 EventBus 交给目标 hat；agent 读取 prompt 里的恢复指令后再次 emit 业务事件，该事件进入 `process_events_from_jsonl` → `process_parse_result` → 同一套或另一层 gate，再次被拒，再次触发 `task.resume`，形成跨 turn 的无限循环。

### 代码证据

1. **注入点**：`event_loop/mod.rs:398-496` 的 `publish_policy_rejection_resume` 构造结构化 payload（`reason` / `target_hat` / `rejected_topic` / `source_hat` / `message`），最终调用：
   ```rust
   let mut resume = Event::new("task.resume", structured_payload_str);
   ...
   bus.publish(resume);  // line 489-495
   ```
   这个事件**不**写回 `.ralph/events.jsonl`，而是进入 EventBus 的 `pending` 队列（`ralph-proto/src/event_bus.rs:94-131`）。

2. **消费点**：`process_output` 在 turn 边界把 `task.resume` 随同其他 pending 事件一起写入下一个 hat 的 prompt（`event_loop/mod.rs:5680-5737` 的 `prepend_hat_handoff_from_pending` 及周围逻辑）。agent 在下一轮根据 prompt 中的 `task.resume` 再次尝试 emit。

3. **重新进入验证链**：agent emit 的业务事件由 `event_reader.rs:1-1040` 从 `.ralph/events.jsonl` 读出，进入 `process_events_from_jsonl`（`event_loop/mod.rs:7160-7167`），再委托给 `process_parse_result`（`event_loop/mod.rs:7193`）。该函数依次经过：
   - engine required-field gate（`event_loop/mod.rs:7267`）
   - event origin guard（`event_origin.rs` 通过 `validate_event_origin`）
   - event policy validation：`apply_event_policy_validation`（`event_loop/mod.rs:1169-1700+`），内部调用 `check_completion_honored`（line 1263）、`check_topic_deny_rules`（line 1359）、`validate_event`（line ~1400）
   - state machine validation（line ~8420）
   - hat-handoff gate（line 8426）
   - state projection（line ~8578-8664）
   - step handoff gate（line 8666）
   - workflow guard validation：`apply_workflow_guard_validation`（line 8697-8728）
   - execution contract validation（line 8748-）

4. **retry key 的碎片化**：
   - `event_loop/rejection.rs:273` 的 `compute_retry_key` 格式为 `"{}:{}:{}:{}"`，即 `stage:source_hat:topic:violation_class`。
   - recoverable budget 计数器在 `event_loop/loop_state.rs:806-815` 记录为 `"policy:{hat}:{topic}:{reason_class}"`。
   - 结果：同一根因如果在 origin guard（stage=`origin`）、event policy（stage=`policy`）、hat-handoff gate（stage=`hat_handoff`，见 `event_loop/mod.rs:8538`）、workflow guard（stage 不同）或 execution contract 中被拒，会生成不同 key，预算和 responder escalation 都无法跨层累积。

5. **预算上限过低且无法跨层**：`event_loop/loop_state.rs:21` 定义 `pub const U2_REJECTION_RETRY_LIMIT: u32 = 3;`。当同一错误在不同 stage 间漂移时，每层都从零开始计数，`3` 次上限永远打不穿跨层循环。

### 为什么 30 天没根治
每次补丁都在加固某一层 gate：
- origin guard 加 `ralph_control_only`（`event_origin.rs:366-377`）
- policy 加 `required_fields` 和 `completion_honored`
- execution contract 加 `require_git_change`
- hat-handoff gate 在 isolated 模式下做 fail-closed 校验（`event_loop/mod.rs:8426-8538`）

但这些补丁都没有切断「`task.resume` → agent 重 emit → 验证链再次拒绝 → 再发 `task.resume`」的跨 turn 循环。只要 agent 对恢复提示的理解有偏差，换个 stage 或 violation class 就能重新开始计数。

### 修复方向
将 `task.resume` / `human.guidance` 这类系统内恢复事件从「agent 重 emit」路径中解耦：
- 当前 `inject_human_guidance`（`event_loop/mod.rs:3342`）已经是 in-memory 注入，不依赖 agent emit。
- 对 policy / contract / workflow guard / hat-handoff 的 recoverable 拒绝，应当把修复指令直接写入目标 hat 的下一次 prompt context（类似 `pending_lint_resume` 在 `loop_state.rs:455` 的机制），而不是发布一个让 agent 自己决定如何回应的 `task.resume`。
- 如果必须让 agent 重试，应把「上次拒绝的精确原因 + 期望 payload 模板」直接嵌入 prompt，并通过 engine gate 在 emit 落盘前 fail-closed 拦截，而不是等事件进入验证链后再循环拒绝。

---

## 2. 「软提示」架构 —— 关键动作仍依赖 agent 读取 prompt 后自觉执行

### 一句话
Ralph 协调、task 关闭、handoff 文件生成、progress.md 更新等核心动作，都是靠 prompt 里的文字约束 agent 行为；runtime 只在 emit 落盘后才做 gate 拒绝。agent 不读、不执行、执行错 = 系统断裂。

### 代码证据

1. **Ralph 被物理锁死在 control topics**：
   - `hatless_ralph.rs:660-668` 的 prompt 要求 Ralph 执行 `PLAN → DELEGATE`，并且 "MUST emit exactly ONE next event via `ralph emit`"。
   - 但 `event_origin.rs:366-377` 的 `ralph_control_only` 规则把 `ralph` hat 限制为只能发布 control topics；任何业务 topic（`work.start`、`review.complete` 等）都会被 origin guard 拒绝。这意味着 Ralph 的 "DELEGATE" 实际上只能发 `queue.advance` 等 control 事件，真正业务事件的正确性仍依赖下游 specialized hats 自觉执行 prompt。

2. **task 关闭依赖 agent 自觉**：
   - `presets/en/ce-executor-serial.yml:653` 的 prompt 明确说："Step 6 (`ralph tools task close <task_id>`) is REQUIRED before emitting `work.done`. The execution contract will REJECT `work.done` if the referenced task is not in a terminal state."
   - `execution_contract.rs:223-268` 的 `validate_execution_contract` 确实会 reject，但这仍是「先 emit 后 reject」；runtime 不会自动替 agent 调用 `ralph tools task close`。agent 不执行 = `work.done` 被拒 → 触发 `task.resume` → 回到因素 1 的循环。

3. **handoff 文件：已局部 fail-closed，但生成仍靠 agent**：
   - `event_loop/mod.rs:8426-8538` 的 hat-handoff gate 在 isolated 模式下会 fail-closed：如果 macro-edge emit 的 payload 没有 `handoff_path` 或文件内容不通过 `config.artifact.validate`，事件会被丢弃并发布 `diagnostic.hat_handoff.rejected` + `task.resume`。
   - 但 `handoff_path` 本身仍由 agent 在 emit 时填写（`hat_handoff/payload.rs:34-66` 的 `extract_handoff_path` 只是读取 agent 提供的值）。agent 不写或写错路径，gate 只能拒绝，不能自动补齐。

4. **progress.md / tasks.jsonl 更新依赖 agent emit 正确事件**：
   - `state_projector/mod.rs:250` 的 `apply()` 只在事件被所有 gate 接受后才写盘；如果 agent 不发 `work.done` / `queue.advance` / `plan.complete`，磁盘状态不会前进。
   - `step_handoff/progress_task_gate.rs:283-380` 的 `check_progress_task_alignment` 直接从磁盘读取 `progress.md` 和 `tasks.jsonl` 做校验。agent 不更新 progress.md = 后续 `queue.advance` 被 step handoff gate 拒（`event_loop/mod.rs:8666-8694`）。

### 为什么 30 天没根治
补丁一直在增加 prompt 段落和后置 gate：
- handoff 5 段式模板（`## context / ## changed / ## verify / ## next / ## notes`，见 `hat_handoff/inject.rs:97-105` 的测试样例）
- emit 示例和 schema hint（`emit_schema_hint.rs`）
- execution contract 的 required fields
- hat-handoff gate 的 artifact validation

但 runtime 仍然是「agent Emit → 系统 Check → 拒绝 → 提示 agent 再试」。没有机制把「必须完成的动作」变成系统原子操作：关闭 task、写 handoff、更新 progress 都应由 runtime 在识别到正确语义后自动完成，而不是等 agent 自觉调用工具。

### 修复方向
- **把关键副作用从 agent 侧移到 runtime 侧**：例如 `work.done` 被接受后，runtime 自动调用 `task_store::TaskStore::close`（而不是等 agent 先 `ralph tools task close`）；`queue.advance` 被接受后，runtime 自动重写 `progress.md` 的 Current Step。
- **handoff_path 必填且由 runtime 分配**：macro-edge emit 必须携带 runtime 生成的 handoff 文件句柄；没有句柄的事件在 engine gate 阶段直接拒绝，拒绝信息直接写回 prompt，不进入 EventBus 循环。
- **Ralph 的 DELEGATE 指令与 runtime 能力对齐**：既然 `ralph_control_only` 已经限制 Ralph 只能发 control topics，prompt 中不应再要求 Ralph "emit business event"，而应明确 "你只能 emit control topics；具体业务动作由 runtime 在下游 hat 返回后自动推进"。

---

## 3. 多状态源的「竞争写入」—— 没有单一权威状态根

### 一句话
同一 workflow 的进度同时写入 6+ 个独立状态源，各源之间没有事务同步；不同 gate 读取不同源的 snapshot，产生 phantom mismatch，把合法事件误判为拒绝。

### 代码证据

1. **状态源清单**：
   - **磁盘**：`.ralph/agent/tasks.jsonl` 和 `.ralph/agent/progress.md`，由 `state_projector/mod.rs:250` 的 `apply()` 写入（`state_projector/task.rs:131-140` 的 `persist` 和 `state_projector/progress.rs:138-173` 的 `write_progress`）。
   - **内存 `WorkflowProgress`**：`event_loop/loop_state.rs:601-605` 定义 `HashMap<String, HashMap<Option<String>, WorkflowInstanceProgress>>`，由 `apply_workflow_guard_validation` 在 `event_loop/mod.rs:8716` 维护。
   - **内存 `ReviewStepTracker`**：`event_loop/review_step_state.rs:57-60` 定义，在 `event_loop/mod.rs:6660`（`maybe_emit_incomplete_wave_blocked`）和 `event_loop/mod.rs:8153`（policy validation）中被更新/传递。
   - **内存 `FlowLifecycleRegistry`**：`event_loop/loop_state.rs:547` 的 `flow_lifecycle`，由 `process_output` 在 handoff escalation 时读取（`event_loop/mod.rs:6572-6603`）。
   - **内存 `HandoffTracker`**：`event_loop/loop_state.rs:546`，在 policy accept 时记录 handoff deadline（`event_loop/mod.rs:8231-8236`），在 `process_output` 中检查超时（line 6512）。
   - **内存 `ActivationLifecycleTracker`**：`hat_lifecycle.rs:289`，追踪 hat 激活生命周期。
   - **内存 `PolicyRuntimeState`**：`event_loop/mod.rs:8137-8153`，记录 terminal_observed、observed_topics、completion 承诺等。

2. **更新顺序依赖调用链，而非事务**：在 `process_parse_result` 中，gate 和 projector 的调用顺序是固定的（state machine → hat-handoff gate → state projection → step handoff gate → workflow guard → execution contract），但每个阶段只更新自己的状态源：
   - state projection 写磁盘（tasks.jsonl / progress.md）
   - workflow guard 写内存 `WorkflowProgress`
   - event policy 写内存 `PolicyRuntimeState` / `ReviewStepTracker`
   - execution contract 读磁盘 + git
   - step handoff gate 读磁盘

3. **跨源 mismatch 示例**：
   - `work.done` 经 state projection 把 tasks.jsonl 中对应 task 标记为 closed（`state_projector/task.rs:97-129`）。
   - 但 `WorkflowProgress` 要等到 `apply_workflow_guard_validation`（`event_loop/mod.rs:8716`）处理 `work.done` 时才 advance phase；如果 `work.done` 不在任何 strict chain 的 topics 列表里，phase 不会前进。
   - 下一轮 plan-gate emit `queue.advance` 时，step handoff gate 读取 `tasks.jsonl` 发现 task 已 closed，但 `progress.md` 的 `Current Step` 可能还没更新（例如 agent 没有正确 emit 带 `step` 的 `queue.advance` 让 `project_advance_step` 执行），于是 `check_progress_task_alignment`（`step_handoff/progress_task_gate.rs:283`）报 `task_closed_but_progress_missing` 或 `step_mismatch`。
   - 这种拒绝的根因不是业务逻辑错误，而是两个状态源（tasks.jsonl 与 progress.md）由不同事件、在不同阶段、通过不同 writer 更新，没有统一事务保证。

4. **step handoff gate 直接从磁盘读**：`step_handoff/progress_task_gate.rs:293-294`：
   ```rust
   let progress_path = workspace.join(".ralph").join("agent").join("progress.md");
   let tasks_path = workspace.join(".ralph").join("agent").join("tasks.jsonl");
   ```
   这意味着它读取的是最新磁盘状态，而不是 `StateProjector` 的内存 cache；虽然当前实现是同步写盘，但设计上是两个独立读取路径，任何未来对 projector 写盘逻辑的改动都会破坏这里的假设。

### 为什么 30 天没根治
过去 30 天的补丁链（missing_event_gate、dedup、handoff 0 触发等）本质上都是在加固某一层的 gate 判断，例如：
- 加 `progress_task_gate` 的 cold-start 豁免（`step_handoff/progress_task_gate.rs:325-328`）
- 加 `plan.complete` 时自动关闭未关闭 task（`state_projector/progress.rs:94-126`）
- 加 `work.done` dedup（`event_loop/mod.rs:8252-8263`）

但这些补丁都是在「状态源已经不一致」之后做修复，而不是把多个状态源合并到一个单一、顺序、原子的 state ledger 中。只要状态写入分散在 6+ 个位置，换个执行路径就会重新出现 mismatch。

### 修复方向
建立单线程、顺序写入的 **state ledger**：
1. 所有状态变更（task 状态、progress 步骤、workflow phase、review step、flow lifecycle）必须在同一个 `StateProjector::apply()` 事务中按顺序提交。
2. 任何 gate 的验证只能基于 `ProjectionContext` 的 `tasks_cache` + `progress_cache` 内存快照（`state_projector/mod.rs:228-230`），禁止 gate 绕过 projector 直接读取磁盘或维护自己的并行状态。
3. `WorkflowProgress`、`ReviewStepTracker`、`FlowLifecycleRegistry` 等内存结构应从 projector 的提交日志中派生，而不是由 event loop 在验证阶段直接修改。

---

## 三因素的耦合关系

这三个缺陷不是独立的 bug，而是相互耦合的系统性脆弱：

```
┌─────────────────────────────────────────────────────────────────┐
│ 软提示架构 (Top 2)                                              │
│ agent 不执行 task close / 不写 handoff / 不更新 progress        │
│ ↓                                                               │
│ 验证链拒绝 → publish task.resume (Top 1)                        │
│ ↓                                                               │
│ task.resume 进入 target_hat 的 prompt                           │
│ agent 再次 emit 业务事件 → 进入 process_events_from_jsonl       │
│ 再次被某层 gate 拒绝 → 再发 task.resume … 无限循环              │
│ ↓                                                               │
│ 无正确事件 → state projector 不写盘 → 状态源停滞 (Top 3)        │
│ ↓                                                               │
│ step_handoff / workflow guard 读取旧版本或跨源不一致            │
│ → 判定 mismatch → 更多 rejection → 更多 recovery                │
│ ↓                                                               │
│ 回到 Ralph fallback → Ralph 被 prompt 教着发 control topics    │
│ 下游 specialized hats 再次 emit 错误事件 → 回到 Top 1           │
└─────────────────────────────────────────────────────────────────┘
```

### 为什么每次补丁只打到症状
因为补丁都在加固某一层的 gate（加字段、加校验、加 retry 逻辑、加 artifact validation），但没有拆掉 gate 之间的循环依赖：
- **因素 1** 的循环依赖：验证链 → `task.resume` → agent → 验证链。
- **因素 2** 的软提示：系统只检查 agent 的 emit 结果，不自动完成关键副作用。
- **因素 3** 的多状态源：每个 gate 维护自己的状态，缺乏单一提交点。

只要这个循环还在，换个错误名字、换个 stage、换个 violation class，就会再次出现。

---

## 修复状态(2026-06-22,U11 commit 链)

| 不稳定因素 | 修复单元 | 状态(2026-06-22) |
|---|---|---|
| **因素 1** `task.resume` 自指循环 | U7a deterministic correction + U7b `loop.resume` | ✅ 代码完成:`publish_correction_via_context` 接入真实 `LoopState::prompt_context` + `StateLedger::commit`(U11-T3, commit `d568437e`)。1 条 BDD `correction_deterministic_scenario` 已 un-ignore 并通过。`correction_three_escalation_scenario` 因 `LINT_CIRCUIT_BREAKER_LIMIT=2` 限制保留 `#[ignore]`(已知 follow-up)。⚠️ `UNIFIED_DETERMINISTIC_CORRECTION` 默认值尚未反转(T7.1 sub-task),需先迁移 10+ 旧 `task.resume` wire-format 测试。 |
| **因素 2** 状态源分散 + agent 跳过副作用 | U1 StateLedger + U2 StateProjector migrate | ✅ 代码完成:`StateLedger::new` 接入 `replay_from_disk` 自动 cold-start 恢复(U11-T1, commit `df7dcae3`);`process_parse_result` 末尾 commit scalars(A1 hook);macro-edge auto-gen artifact(U11-T4, commit `63af596c`)。⚠️ `UNIFIED_STATE_LEDGER` 已默认 ON(T7);full per-event delta commit 仍待 P2 follow-up(原 plan §阶段二)。 |
| **因素 3** 协议视图分裂(CLI / engine / runtime 三层不一致) | U3 ProtocolView + U4 ValidationPipeline + U6 CLI 迁移 | ✅ 代码完成:per-event `ValidationPipeline::validate_pre_commit_with_view` 接入 `process_parse_result` step handoff gate 之前(U11-T2, commit `76e409db`);`run_policy_check_unified` 加载 events.jsonl 重建 LedgerSnapshot(U11-T7)。⚠️ 3 个 event_loop 测试 + 16 个 ralph-cli policy_check 测试因 wire-format 差异需迁移(T7 follow-up);`review_step_gate::legal_synth_passed_then_plan_complete_accepted` 揭示 unified rule 集合与 legacy 不完全对齐,需规则审计。 |

### 未完成项

1. **`UNIFIED_DETERMINISTIC_CORRECTION` 默认反转**:T7 因 10+ 旧 `task.resume` wire-format 测试 pin 旧路径,保留 off。需把这些测试迁移到新 surface(correction blocks in prompt)后才能翻转默认。
2. **Per-event delta commit**:当前 A1 hook 只 commit scalars(iteration + completion/cancellation flags)。每个 task/progress/handoff event 仍走 legacy StateProjector 写盘路径。
3. **Unified rule 集合与 legacy 对齐**:U11-T2 接入 unified pipeline 后,3 个 event_loop 测试失败,说明 RequiredFieldsRule / StepHandoffRule 等规则与 legacy 不完全等价,需审计具体行为差异。
4. **CLI policy_check wire-format 迁移**:16 个 ralph-cli 测试 pin 旧 compat path 错误消息文本,unified pipeline 输出不同格式。
5. **`correction_three_escalation_scenario` BDD**:fixture 假设 3 次连续 rejection,但 `LINT_CIRCUIT_BREAKER_LIMIT=2` 在第 2 次就 trip。需要 fixture 调整或 breaker limit 提高。
