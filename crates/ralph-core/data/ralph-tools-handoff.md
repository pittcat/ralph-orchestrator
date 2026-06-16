---
name: ralph-tools-handoff
description: ce-executor step handoff 深参考 — `task.resume` 复杂 violation 的归属、progress 修复、wave 收摊路径（按需加载）
metadata:
  internal: true
---

# ralph-tools-handoff — Step handoff 深参考

> **先读自动注入 R0**：loop 内 agent 收到 `task.resume` 时，**第一手修复路径**在每轮自动注入的 `ralph-tools.md`「收到 `task.resume` 时」段；本文档供按需 `ralph tools skill load ralph-tools-handoff` 后**深查**复杂 violation（progress / handoff dispatch / plan.blocked / wave 收摊等）。
>
> **不注入**：本 skill 不在 auto-inject 白名单中（plan 004 KTD3）；按需 load 节省 token。

## 1. Step handoff topic 归属（`ce-executor-isolated` preset）

| Topic | 发布 hat | 消费者 hat | 拒收 reason_code 常见值 |
|-------|---------|----------|------------------------|
| `work.ready` | `plan-gate` | `executor`（唯一） | `payload_contract_violation` / `MissingPayloadField` |
| `plan.complete` | `executor` | `plan-gate` | `progress_task_mismatch` |
| `plan.blocked` | `plan-gate` | `shipper` | `payload_contract_violation`（见 §5 provenance 约束） |
| `queue.advance` | `executor` | `plan-gate` | `progress_task_mismatch` |
| `review.wave.ready` | `plan-gate` | `dimension-reviewer` | `MissingPayloadField`（`depth`） |
| `review.dimension.done` | `dimension-reviewer` | `review-synthesizer` | wave 收摊后被 `review_passed_while_wave_open` 拒（独立 bucket） |
| `review.complete` | `review-synthesizer` | `plan-gate` | `MissingPayloadField` |
| `LOOP_COMPLETE` | `executor` / `plan-gate` | ralph | 终态，须在 hat `publishes` 显式声明 |

`trigger_multi_consumer_topics`：上表中**唯一消费者**的 topic（`work.ready` / `queue.advance` / `plan.complete` / `plan.blocked` / `review.complete`）走 `HandoffTracker` 30s SLA（`event_loop.workflow_contract.handoff_dispatch_timeout_seconds`，上限 120s）；多消费者 topic（`review.wave.ready` / `review.dimension.done`）走 wave 收摊而非 handoff。

## 2. `progress_task_gate` / `progress_task_mismatch` 修复

`queue.advance` 或 `plan.complete` 在 step handoff 时必须满足 `progress.md` ↔ `tasks.jsonl` 对齐，否则 `progress_task_gate` 拒收并触发 `task.resume`（payload 含 `reason_code: progress_task_mismatch`）。

**修复顺序**（agent 视角）：

1. 读 `.ralph/agent/progress.md` 顶部 `## Completed Steps` 列表
2. `ralph tools task list --status closed` 拿到本 step 内已 `closed` 的 task
3. 对齐：所有「当前 step 已完成」的 task 必须 `closed`；所有 `closed` 的 task 必须在 `## Completed Steps` 里
4. 缺一则先补 `ralph tools task close <task-id>` 或在 `progress.md` 加记录
5. 重发原 topic（不要绕过 gate）

**校验命令**：

```bash
# 1. 列出当前 step 的 closed task
ralph tools task list --status closed --format json | jq -r '.[] | .id'

# 2. 对比 progress.md 已记录的 step
grep -A 5 '## Completed Steps' .ralph/agent/progress.md

# 3. 看 recovery.jsonl 历史 progress_task_mismatch
jq 'select(.reason_code == "progress_task_mismatch")' \
   .ralph/diagnostics/latest/recovery.jsonl
```

CLI 入口预检（`--policy-check` 接 `progress_task_gate`）见计划 `docs/plans/2026-06-17-005-fix-agent-recovery-mechanism-gaps-plan.md` U1（机制在本计划不落地）。

## 3. `handoff_dispatch_timeout` 修复

`work.ready` 等唯一消费者 handoff 在 `handoff_dispatch_timeout_seconds`（默认 30s，上限 120s）内未被激活，触发 `recovery.jsonl` `reason_code: handoff_dispatch_timeout` + `task.resume to plan-gate`（Hard 升级）。

**排查**：

- 消费者 hat（`executor`）是否在 budget 内被 backpressure 阻塞
- 后端是否已返回但事件未 flush（看 `.ralph/agent/events.jsonl` 末尾）
- 是否多个 worktree 同时持有 executor 上下文导致隔离预算耗尽

**不要**自重发 `work.ready` — `task.resume` 已经由机制重派回源 hat；先解决消费者未激活的根因。

## 4. Wave 收摊：缺维度 / 超时

review wave `received_count < expected_dimensions` 时的两条路径：

- **等待中**（`now - last_dimension_at < 0.8 * aggregate_timeout_secs`）：继续等 worker 收尾，**不要**自补 `review.dimension.done`。
- **超时**（`now - last_dimension_at >= 0.8 * aggregate_timeout_secs`）：**机制层**自动 emit `plan.blocked(reason=dimension_reviewers_failed_to_converge)`，路由 `review-synthesizer` → `shipper`（不要等 plan-gate 自消费）；详见 `docs/plans/2026-06-17-003-fix-ce-executor-wave-stall-bypass-plan.md` U1+U2 与 `crates/ralph-core/src/flow_lifecycle/incomplete_wave_gate.rs`。

`review_passed_while_wave_open`（U1）改为 `ViolationType::SemanticGateViolation`，独立 recoverable bucket，**不**计入 `U2_REJECTION_RETRY_LIMIT`，不发 fatal `PayloadContractViolation`；`task.resume` hint 显式禁止 empty_diff，要求等待 `plan.blocked` 或补全维度。

## 5. `plan.blocked` provenance 约束

`plan.blocked` 在 isolated 模式下**只能**由 `plan-gate` hat 发布（preset 唯一合法 publisher）。其他 hat 自发 `plan.blocked` 会被 `EventOriginGuard` 拒收并触发 `task.resume`（`reason_code: out_of_scope_topic`）。

不要绕：若需表达「我无法推进」，发 `human.guidance`（等人类决策）而非 `plan.blocked`。

## 6. 校验命令速查

```bash
# 当前 hat 可发 topic（与 isolated 越权判定对齐）
ralph hats list --format json | jq -r '.[] | select(.id == "'"$RALPH_CURRENT_HAT"'") | .publishes[]'

# 看最近一轮 task.resume 来源
jq 'select(.type == "task.resume")' .ralph/events.jsonl | tail -1

# 看 recovery.jsonl 全部 envelope
jq '.' .ralph/diagnostics/latest/recovery.jsonl

# 出报告（CI / post-mortem）
ralph diagnose --session latest
```

## 7. 相关文档

- `docs/plans/2026-06-17-002-feat-ce-executor-step-handoff-plan.md` — step handoff 机制完整设计
- `docs/plans/2026-06-17-003-fix-ce-executor-wave-stall-bypass-plan.md` — wave 收摊 / R6 机制
- `docs/plans/2026-06-17-005-fix-agent-recovery-mechanism-gaps-plan.md` — CLI 预检对齐（姊妹 PR）
- `docs/guide/runtime-diagnosis.md` §10 / §12.1 — 诊断决策树
- `crates/ralph-core/data/ralph-tools.md` — 每轮自动注入的修复段（速查）
- `crates/ralph-core/data/ralph-tools-emit.md` — emit 详表（schema / null-payload / isolated）
