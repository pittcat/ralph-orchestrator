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
ralph tools task ensure "Title" --key spec:task-01 -p 2 -d "description" --blocked-by id1,id2
ralph tools task list [--status open|in_progress|closed] [--format table|json|quiet]
ralph tools task ready                    # Show unblocked tasks
ralph tools task start <task-id>
ralph tools task close <task-id>
ralph tools task reopen <task-id>
ralph tools task fail <task-id>
ralph tools task show <task-id>
```

**Task ID format:** `task-{timestamp}-{4hex}` (e.g., `task-1737372000-a1b2`)

**Task key:** optional stable key for idempotent orchestrator-managed tasks (for example `spec:task-01`)

**Priority:** 1-5 (1 = highest, default 3)

### Task Rules
- One task = one testable unit of work (completable in 1-2 iterations)
- Break large features into smaller tasks BEFORE starting implementation
- On your first iteration, check `ralph tools task ready` — prior iterations may have created tasks
- Use `task ensure --key ...` when a task has a stable identity and may be recreated across fresh-context iterations
- Use `task start` when you begin active work on a task
- ONLY close tasks after verification (tests pass, build succeeds)
- Use `task reopen` when more work remains after a failed review/finalization pass
- Use `task fail` when the task is blocked and cannot be completed in the current iteration

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
ralph tools task ensure "Fix: <short description>" --key fix:<short-key> -p 2
```

## Common Workflows

### Track dependent work
```bash
ralph tools task ensure "Setup auth" --key auth:setup -p 1
# Returns: task-1737372000-a1b2

ralph tools task ensure "Add user routes" --key auth:routes --blocked-by task-1737372000-a1b2
ralph tools task ready  # Only shows unblocked tasks
```
