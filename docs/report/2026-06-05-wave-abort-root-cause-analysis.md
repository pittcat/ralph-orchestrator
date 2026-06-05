# Wave 评审中途停止根因分析报告

## 事件摘要

- **时间**：2026-06-04 14:07 → 18:18（UTC+8）
- **Loop**：`implement-refactor-split-dev-plan-warm-tiger`（worktree）
- **触发场景**：U4 step-01 执行完成后，Review Coordinator 发射 8 维 Wave 评审，6 个 Worker 成功、2 个 Worker 超时后，整个 Loop 被强制终止，未进入 review-synthesizer / fixer / plan-gate 后续阶段。
- **用户疑问**：这是 Ralph 机制问题，还是 `presets/en/ce-executor.yml` 编排问题？

---

## 结论（前置）

**这不是 preset 编排问题，也不是 wave 超时问题，而是 Ralph 配置安全机制与用户预期之间的设计冲突 + worktree `ralph.yml` 缺失关键配置项导致的资源预算耗尽。**

具体：
- `ce-executor.yml` 中写了 `max_runtime_seconds: 28800`（8h），但 Ralph 的安全机制**禁止** hat collection preset 覆盖资源预算类字段；
- worktree 的 `ralph.yml` **没有**写 `max_runtime_seconds`，系统回退到硬编码默认值 **14400s（4h）**；
- Loop 实际运行到 **4h 10m 51s** 时，在 Wave 刚完成的瞬间触发 `max_runtime` 终止条件，强制退出。

---

## 证据链

### 1. 终止原因直接证据

`.ralph/events-history-20260604-140753.jsonl`（已归档到 `events-history-loop-terminate.jsonl`）：

```json
{
  "ts": "2026-06-04T18:18:45.059153+00:00",
  "iteration": 11,
  "hat": "loop",
  "topic": "loop.terminate",
  "payload": "## Reason\nmax_runtime\n\n## Status\nStopped at runtime limit.\n\n## Summary\n- Iterations: 11\n- Duration: 4h 10m 51s\n- Exit code: 2",
  "_phase": "warmup"
}
```

诊断日志 `.ralph/diagnostics/logs/ralph-2026-06-04T22-07-52-502-53314.log`（已归档到 `diagnostics-log-max-runtime.log`）最后一条：

```
2026-06-04T18:18:45.059111Z INFO ralph_core::event_loop: Wrapping up: max_runtime. 11 iterations in 4h 10m 51s. reason=max_runtime iterations=11 duration=4h 10m 51s
```

### 2. 硬编码默认值证据

`crates/ralph-core/src/config/loop_config.rs:48-50`：

```rust
fn default_max_runtime() -> u64 {
    14400 // 4 hours
}
```

此默认值在 `EventLoopConfig` 的 `serde(default = "default_max_runtime")` 上生效。

### 3. Preset 配置被安全过滤证据

`presets/en/ce-executor.yml:35` 确实写了：

```yaml
event_loop:
  max_runtime_seconds: 28800  # 8 hours
```

但 `crates/ralph-cli/src/preflight.rs:493-502` 的常量定义明确将其排除：

```rust
// Note: resource budgets (`max_iterations`, `max_runtime_seconds`,
// `checkpoint_interval`) and `enforce_hat_scope` are intentionally
// NOT in this list. They are operator-controlled, not hat-controlled,
// so a hat collection must not be able to widen the loop budget or
// disable scope enforcement behind the user's back.
const ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS: &[&str] = &[
    "completion_promise",
    "starting_event",
    "cancellation_promise",
    "required_events",
    "event_policy",
    "verdict_gate",
    "execution_contracts",
];
```

`merge_hats_overlay` 函数（`preflight.rs:548`）在合并 preset 时，只遍历 `ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS` 白名单内的键。`max_runtime_seconds` 不在白名单，因此 **被静默丢弃**。

### 4. worktree ralph.yml 缺失配置证据

`.worktrees/implement-refactor-split-dev-plan-warm-tiger/ralph.yml`（已归档到 `ralph-worktree.yml`）的 `event_loop` 部分：

```yaml
event_loop:
  completion_promise: LOOP_COMPLETE
  max_iterations: 500
  prompt_file: PROMPT.md
```

**缺少 `max_runtime_seconds`**。由于 preset 的覆盖被过滤，系统只能使用默认值 14400s。

---

## 因果链还原

```
14:07:53  Loop 启动（iteration 0）
         ↓
14:13    Coordinator 创建 U3 step-01 任务
         ↓
15:36-15:48  Executor 完成 U3 config.rs 拆分，emit work.done
         ↓
16:01    Review Coordinator 发射 wave w-18b5ec2f55465570-72307-0（9 workers）
         ↓
16:08-16:20  7 workers 完成，Worker 5 (agent-native) 超时 1800s
         ↓
16:32    Wave 完成（8 done, 1 failed）→ Ralph 自动汇总 → review.failed → Fixer Round 1
         ↓
16:59    Fixer 完成 7 safe_auto → fix.applied
         ↓
17:11    Review Coordinator re-review → review.passed
         ↓
17:14    Plan Gate → queue.advance → U4 step-01
         ↓
17:29-17:42  Executor 完成 U4 main.rs 拆分，emit work.done
         ↓
17:47    Review Coordinator 发射 wave w-18b5f1eb4aeaa740-95895-0（8 workers）
         ↓
17:57-18:15  6 workers 完成
         ↓
18:18:43  Worker 4 (requirements) + Worker 5 (api-contract) 超时 1800s，被 kill
         ↓
18:18:45  Wave 完成（6 done, 2 failed），发布 wave result events
         ↓
18:18:45  EventLoop::check_termination() 发现 elapsed = 15052s ≥ 14400s
         ↓
18:18:45  触发 TerminationReason::MaxRuntime，Loop 强制终止
         ↓
18:18:45  **review-synthesizer / fixer / plan-gate 均未能执行**
```

---

## 为什么 2 个 Worker 会超时

Worker 4 (`requirements`) 和 Worker 5 (`api-contract`) 的 focus 涉及大量逐项验证：
- requirements：R1-R7 逐项 diff 核对、测试计数独立验证、plan 文件列表比对；
- api-contract：13 项 pub(crate) use 列表逐项核对、loop_runner.rs import 路径迁移验证、外部消费者兼容性推演。

这两个维度的 prompt 工作量天然大于 correctness / testing 等维度，在 1800s（30min）内未能完成即被 wave timeout 机制 kill。**这是正常的 wave 超时行为，不是 bug。**

真正导致「中途停止」的，是 wave 完成后 Loop 立刻因 `max_runtime` 终止，使得 6 个已成功 worker 的评审结果没有被 synthesizer 消费，也没有进入 fix → plan-gate → 下一步的正常流程。

---

## 定性：机制问题 vs 编排问题

| 维度 | 判定 | 说明 |
|------|------|------|
| `ce-executor.yml` 编排 | ❌ 无问题 | preset 中写 `max_runtime_seconds: 28800` 是合理预期，但被框架安全策略过滤 |
| Ralph 安全机制 | ⚠️ 设计如此，但 UX 缺陷 | 资源预算类字段不允许 hat collection 覆盖，这是安全设计；但「静默丢弃」导致用户无法察觉 |
| worktree `ralph.yml` | ⚠️ 配置缺失 | 作为 operator 配置文件，应显式声明资源预算；缺失导致回退到 4h 默认值 |
| Wave 超时 2/8 | ❌ 非根因 | 正常行为；即使 8/8 全部在 10min 内完成，Loop 仍会在 4h 时被 terminate |

**根因归类**：Ralph 配置分层模型的 UX 缺陷（operator-config vs hat-collection-config 的边界未向用户暴露）+ worktree 配置文件漏配。

---

## 已执行的修复

### 1. worktree `ralph.yml` 增加 `max_runtime_seconds`

```yaml
event_loop:
  completion_promise: LOOP_COMPLETE
  max_iterations: 500
  max_runtime_seconds: 28800   # <-- 已添加
  prompt_file: PROMPT.md
```

这样 operator 配置直接提供 `max_runtime_seconds`，无需经过 preset 合并，可生效。

### 2. `ce-executor.yml` executor 增加 `build.done` 防御性兼容

将 executor 的 `publishes` 从 `["work.done", "work.failed"]` 扩展为 `["work.done", "work.failed", "build.done"]`，并增加注释说明：

> `build.done` is defense-in-depth: agent may spontaneously emit it during verification steps (build/lint/typecheck). Accept it to avoid origin-guard noise, but do NOT treat it as a terminal event.

同时在 executor instructions 中新增 **Events You MUST and MUST NOT Emit** 章节，明确约束：
- 只有 `work.done` / `work.failed` 是终端事件
- `build.done` 是可选内部进度事件，不能替代 `work.done`
- 禁止 emit 其他 hat 的事件（`queue.advance`、`REVIEW_COMPLETE` 等）

**修复文件**：`presets/en/ce-executor.yml`

### 3. `ce-executor.yml` dimension-reviewer 加固 hat ID 约束

在 Event Publishing 章节增加 **HARD RULE — Hat Identity**：

> The JSON payload MUST include `"hat": "dimension-reviewer"`. Do NOT change your hat name to `reviewer-standards`, `standards-reviewer`, or any variant. Origin guard rejects unknown hats, which voids the entire review result.

**修复文件**：`presets/en/ce-executor.yml`

---

## 其他发现的问题

### 1. executor `publishes` 列表与 agent 行为不匹配（build.done）

executor 在实际运行中 emit 了 `build.done`（6次），被 origin guard 拒绝。

**根因**：legacy/minimal presets（`builder.yml`、`roo.yml`、`kiro.yml`、`code-assist.yml`）中普遍存在独立的 `builder` hat 和 `build.done` 事件；`ralph-tools-emit.md` 技能文档也用 `build.done` 作为 emit 示例。ce-executor 取消了 `builder` hat，但未在 instructions 中清理 agent 的"肌肉记忆"。

**修复**：已在 executor `publishes` 中防御性增加 `build.done`，并在 instructions 中增加事件约束章节。

### 2. wave worker hat ID 漂移（reviewer-standards）

U4 wave 的 standards 维度 worker 错误声明 `hat="reviewer-standards"`（不在 preset 注册表中），被 origin guard 拒绝。

**根因**：backend agent（LLM）在长时间运行后出现"身份混淆"，输出了错误的 hat 字段。

**修复**：已在 dimension-reviewer instructions 中增加 HARD RULE 约束 hat ID。

### 3. U3 work.done 重复发射 + contract 时序异常

15:48:39 和 15:56:50 两次 work.done，第二次被 execution contract 拒绝（TaskNotTerminal）。

**疑点**：第一次 work.done 被接受，但 `tasks.jsonl` 显示 task `closed` 时间是 15:56:35——比第一次 work.done 晚了约 8 分钟。这意味着 task 可能在中途被重新打开，或存在 race condition。由于 JSONL 只保留最终状态，无法从现有证据 100% 确认。

### 4. U4 work.done 非 JSON payload

17:40:08 的 work.done payload 是纯字符串，被 execution contract 拒绝（InvalidPayload），2 分钟后重新发射正确 JSON。

**定性**：backend agent 行为问题，contract gate 正确拦截。

---

## 证据文件清单

| 文件 | 来源 | 说明 |
|------|------|------|
| `events-history-loop-terminate.jsonl` | `.ralph/events-history-*.jsonl` | Loop 终止事件，明确记录 `max_runtime` 原因 |
| `events-full-wave-failure.jsonl` | `.ralph/events-*.jsonl` | 完整事件流，含 wave 发射、worker 超时、wave 完成、loop 终止 |
| `diagnostics-log-max-runtime.log` | `.ralph/diagnostics/logs/ralph-*.log` | 运行时诊断日志，含 wave timeout kill 和 max_runtime 终止 |
| `ralph-worktree.yml` | `.worktrees/.../ralph.yml` | worktree 配置文件，证实缺失 `max_runtime_seconds` |
| `preset-ce-executor.yml` | `presets/en/ce-executor.yml` | 预设原文件，证实写了 `max_runtime_seconds: 28800` |

---

## 引用代码位置

- `crates/ralph-core/src/config/loop_config.rs:48-50` — `default_max_runtime() = 14400`
- `crates/ralph-cli/src/preflight.rs:493-502` — `ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS` 白名单
- `crates/ralph-cli/src/preflight.rs:548-590` — `merge_hats_overlay` 合并逻辑
- `crates/ralph-core/src/event_loop/mod.rs:1054` — `check_termination` 中 `max_runtime` 判定
- `presets/en/ce-executor.yml:35` — preset 中声明的 `max_runtime_seconds: 28800`
