---
name: ralph-tools-tasks
description: Use when managing runtime tasks during Ralph orchestration runs
metadata:
  internal: true
---

# Ralph Tools — Tasks

## Two Task Systems

| System | Command | Purpose | Storage |
|--------|---------|---------|---------|
| **Runtime tasks** | `ralph tools task` | Track work items during runs | `.ralph/agent/tasks.jsonl` |
| **Code tasks** | `ralph task` | Implementation planning | `tasks/*.code-task.md` |

This skill covers **runtime tasks**. For code tasks, see `/code-task-generator`.

## Task Commands

```bash
ralph tools task add "Title" -p 2 -d "description" --blocked-by id1,id2
ralph tools task ensure --key spec:task-01 "Title" -p 2 -d "description" --blocked-by id1,id2
ralph tools task list [-s STATUS] [-d DAYS] [-l LIMIT] [-a] [--format table|json|quiet]
ralph tools task ready [-a] [--format table|json|quiet]
ralph tools task start <task-id>
ralph tools task close <task-id>
ralph tools task reopen <task-id>
ralph tools task fail <task-id>
ralph tools task show <task-id> [--format table|json|quiet]
```

**Task ID format:** `task-{timestamp}-{4hex}` (e.g., `task-1737372000-a1b2`)

**Task key:** optional stable key for idempotent orchestrator-managed tasks (for example `spec:task-01`)

**Priority:** 1-5 (1 = highest, default 3)

### Flags per command

| 子命令 | 可用 flags |
|--------|-----------|
| `add` / `ensure` | `-p/--priority`, `-d/--description`, `--blocked-by`, `--format`, `--root` |
| `list` | `-s/--status`, `-d/--days`, `-l/--limit`, `-a/--all`, `--format`, `--root` |
| `ready` | `-a/--all`, `--format`, `--root` |
| `start` / `close` / `fail` / `reopen` / `show` | `--format`（仅 `show`）, `--root` |

> `--root` 是 `ralph tools` 命名空间下所有子命令共享的工作目录选项；`-c/--config`、`-H/--hats`、`-v/--verbose`、`--color` 为全局选项，不在上表重复。

### Task Rules
- One task = one testable unit of work (completable in 1-2 iterations)
- Break large features into smaller tasks BEFORE starting implementation
- On your first iteration, check `ralph tools task ready` — prior iterations may have created tasks
- Use `task ensure` with a stable task `key` (concept) when a task has a stable identity and may be recreated across fresh-context iterations
- Use `task start` when you begin active work on a task
- ONLY close tasks after verification (tests pass, build succeeds)
- Use `task reopen` when more work remains after a failed review/finalization pass
- Use `task fail` when the task is blocked and cannot be completed in the current iteration
- **NEVER pass an empty `task_id`**: `ralph tools task start/close/fail/reopen/show` and any `ralph emit` payload containing `task_id` must use a real, non-empty id like `task-{timestamp}-{hex}`. Empty `task_id` is rejected by the CLI and will break step handoff.
- **`task_id` / `task_key` / `step` 必须同源**: 在 `ralph emit` payload 中，如果同时出现 `task_id`、`task_key` 和 `step`：
  - `task_id` 必须来自当前 loop 的 live record（`ralph tools task list` / `show` / `ensure` 返回），不要复用已 closed 的 id。
  - `task_key` 是稳定匹配键（`loop_id` + `task_key` + `step` 构成 live identity）；同一 identity 重复 ensure 返回同一 `task_id`，不得手写第二套 id。
  - `task_key` 中的 step 段（例如 `:fix-02:`）必须与 `step` 字段完全一致。
  - 不要手写 `task_id`；优先用 `ralph tools task add/ensure` 生成，或从 trigger payload / `## ORCHESTRATOR CONTEXT` 读取 projector 派生值。
  - **`work.done` 等 execution contract topic**：必须先 `ralph tools task close <task_id>`，再 emit（close-before-done 顺序固定）。

### Cross-Loop and Cross-Hat Authorization

`ralph tools task` 在 loop 中调用时，runner 已注入 `RALPH_CURRENT_HAT` 和 `RALPH_CURRENT_LOOP_ID`，同时 `.ralph/current-loop-id` marker 指向真实 loop。满足这些条件时适用以下规则：

- New tasks are stamped with the **current loop id** and the **current
  hat id** (`owner_hat_id`). They are not visible across loops without
  `task ready --all`.
- `start` / `close` / `fail` / `reopen` on a task in another loop is
  rejected outright.
- Within the same loop, only the task's owner hat (or any hat listed in
  `tasks.coordinator_hats` in `ralph.yml`) may mutate it. An executor
  hat cannot start a reviewer hat's task.
- Legacy tasks with no `loop_id` and no `owner_hat_id` are **not
  mutable** from an agent context. Recreate them via `task add` or
  `task ensure` so they pick up the current loop/owner.
- `blocker` IDs must exist in the current loop's task list. Cross-loop
  blockers and missing blockers are rejected at `add` / `ensure` time.
- A human CLI invocation (no runtime env) may still mutate any task
  for diagnostics; a warning is printed when the target task's
  `loop_id` differs from the current marker.

Configure coordinator hats globally:

```yaml
tasks:
  enabled: true
  coordinator_hats:
    - orchestrator
    - worker
```

> **OPAC Precheck (zero-write)**: 任何 `task add` / `ensure` / `start` / `close` / `fail` / `reopen` 前先跑 `ralph tools task verify <verb>`（共享同一 `authorize_lifecycle` / `validate_owner_hat_id` / `HatCommandPolicy` 内核，零写盘）。三字段一致性走 `ralph tools task verify-emit-bridge --task-id ID --task-key KEY --step STEP`，详见 always-injected `ralph-tools-opac` Precheck 段。
>
> **OPAC Confirm (close 后)**: agent context 下 `task close` 成功后若 hat-channel 无 completion topic，CLI 会 stderr 输出 `close_without_completion_emit` warning，含 `expected_topics` + `next_step`——**忽略它等于进入 stall 30s 等待 rescue**。详见 `ralph-tools-opac` Confirm 段。

> **U7 两步式 task verify gate（agent 强制）**: 当 preset 启用 `tasks.require_verify_for_cli_mutate: true` 时，agent 调 `task add` / `task ensure` **必须**先 `ralph tools task verify <verb> <args…>`（Allow 后 runtime 自动写 ticket）→ 再用**完全相同**参数调 `ralph tools task <verb>`。漂移（参数变了 / 跨 hat / 没先 verify）会被 `task_verify_gate denied` 拒收，**不写盘**。人类 CLI 永远 bypass；`tasks.allow_unsafe_task_mutate: true` 是 escape hatch（recovery 专用）。详见 `ralph-tools-opac` "Apply 阶段两步式 task verify gate" 段。

### First thing every iteration
```bash
ralph tools task ready    # What's open? Pick one. Don't create duplicates.
```

### Failure Capture — Task Half

If any command fails (non-zero exit), or you hit a missing dependency/skill, or you are blocked:
- **Open or reopen a task** if it won't be resolved in the same iteration.

```bash
ralph tools task ensure --key fix:short-key "Fix: <short description>" -p 2
```

## Common Workflows

### Track dependent work
```bash
ralph tools task ensure --key auth:setup "Setup auth" -p 1
# Returns: task-1737372000-a1b2

ralph tools task ensure --key auth:routes "Add user routes" --blocked-by task-1737372000-a1b2
ralph tools task ready  # Only shows unblocked tasks
```

---

## 运行时行为规范

以下规范在 loop 遇到 `task.resume` 时由 runner 自动注入（对应 `ralph-tools-recovery-directives` skill）。task 管理**必须**遵守：

- **收到 `task.resume(kind=recovery_exhausted)` 后**：**禁止**再重试；立即 emit `plan.blocked(reason="recovery_exhausted:<retry_key>")` 并把阻塞原因写入当前 task note。
- **收到 `task.resume(kind=execution_contract:TaskWrongLoop)` 后**：重新 emit 前**必须**确认 `task_id` 属于当前 loop；跨 loop task 只能读、不能改。
- **任何 emit 的 `task_id` 必须真实且非空**：在 emit 任何带 `task_id` 字段的事件前，必须先从 `.ralph/agent/tasks.jsonl`（`ralph tools task list` / `ralph tools task show`）取得当前 loop 的真实 id 填入 payload。`task_id=""`、`null` 或 `from_key:...` 形态都会被拒绝，并破坏 step handoff / state projection。
- **task 反复失败时**：不要无限 reopen 同一 task；评估是否需要拆分为更小任务或提升到 `plan.blocked`。
- 更多细节见自动注入的 `## RECOVERY DIRECTIVES` 块（ID：`RD-PLAN-BLOCKED-ON-RECOVERY-EXHAUSTED`、`RD-TASK-ID-MUST-BE-LOOP-SCOPED`）。

