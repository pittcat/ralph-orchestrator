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

### Cross-Loop and Cross-Hat Authorization

When `ralph tools task` is invoked from inside a loop (`RALPH_CURRENT_HAT`
or `RALPH_CURRENT_LOOP_ID` is set in the env, and `.ralph/current-loop-id`
points to a real loop), the following rules apply:

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

### Single-U Contract（2026-06-14 计划 003 R4 — `ce-executor-serial` only）

**默认关闭**。当 `ce-executor-serial` preset 启动后，`ralph run` 写 `.ralph/agent/.ralph-enforce-current-unit` marker，子进程 `ralph tools task ensure` 检测后激活契约。standalone CLI 用户可设环境变量 `RALPH_ENFORCE_CURRENT_UNIT=1`。

**契约规则**：

- key 形如 `ce-executor:{plan}:step-XX:uN-impl`（N 是数字）才被 gate。`u1a-impl` / `u1b-impl` 塌缩到 `u1`，允许并存。
- 同一 `(loop_id, plan_name, step)` 下已 open U1 task，再 ensure `u2-impl` 时：
  - CLI 退出非零，stderr 输出 `rejected by R4 single-U contract: ...`。
  - ensure 返回已存在的 U1 task（id 与 requested key 不一致）。
- 同 key 重复 ensure 是幂等的 — 返回同一 task。
- 旧 key / 非 `uN-` 形状（`step-99-impl`、`review-bug-impl` 等）**不被 gate** — 这是已知边界，**不要**依赖 R4 保护非 canonical keys。
- 失败时不要重试同一 key — 切换到下一 U 或关闭冲突 task。

Configure coordinator hats globally:

```yaml
tasks:
  enabled: true
  coordinator_hats:
    - coordinator
    - executor
```

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

