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

> **Supervisor-spawned rows** — when a preset enables `supervisor`, every slot's
> lifecycle is projected onto a stable row in `.ralph/agent/tasks.jsonl` under
> key `supervisor:<loop_id>:wave-<wave_id>:slot-<index>`. The dispatcher is the
> SOLE writer of these rows; workers must NEVER touch `tasks.jsonl` directly.
> Repeated projections (re-reports, restart replay) are idempotent — the same
> task_key always resolves to the same row.

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
ralph tools task confirm <task-id> --reference <ref> --digest <digest> [--format table|json|quiet]
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
| `confirm` | `--reference`（必填）, `--digest`（必填）, `--format`, `--root` |

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
  - 不要手写 `task_id`；从 trigger payload、`ralph tools task list` / `show`，或 prompt 里的 `## ORCHESTRATOR CONTEXT` 取得 live id。
  - **`work.done` 等 execution contract topic**：必须先 `ralph tools task close <task_id>`，再 emit（close-before-done 顺序固定）。
  - **例外：emit 即自动关闭 task**：当 hat instructions 明确说明「emit 该事件即原子关闭 task、禁止手工 close」时，跳过 `ralph tools task close`，直接 emit；`task_id`/`task_key` 仍需与 trigger payload 同源。

### Cross-Loop and Cross-Hat Authorization

`ralph tools task` 在 loop 中调用时，runner 已注入 `RALPH_CURRENT_HAT` 和 `RALPH_CURRENT_LOOP_ID`，同时 `.ralph/current-loop-id` marker 指向真实 loop。满足这些条件时适用以下规则：

- New tasks are stamped with the **current loop id** and the **current
  hat id** (`owner_hat_id`). They are not visible across loops without
  `task ready --all`.
- `start` / `close` / `fail` / `reopen` on a task in another loop is
  rejected outright.
- Within the same loop, the task owner may execute and administer its task.
  A hat listed in `tasks.coordinator_hats` may administer lifecycle state
  (`close` / `fail` / `reopen`) for coordination, but **cannot `start` or
  implement another hat's task**. `start` requires execution ownership and an
  open, unblocked task.
- The prompt's `[read-only]` marker uses the same execution-ownership decision
  as `task start`; coordinator administration never removes that marker.
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

### Projection-Owned Task Creation

Builtin presets may declare a **single declarative handoff** that gives a
batch of tasks to the runtime projector. When your hat instructions identify
such a handoff, the projector is the single task writer and you **must not**
also call `task add` / `task ensure`.

Two input forms exist; follow the form named by your hat instructions/schema:

- **Payload-backed batch**: the event carries an items array with stable task
  keys, titles, dependency keys, and the declared count. Missing items,
  duplicate keys, cycles, self-edges, unknown dependencies, or count mismatch
  reject the entire batch before any task is written.
- **Artifact-backed batch**: the event carries only the repo-relative artifact
  path and identity fields. Write and validate the artifact first, then run the
  required `ralph emit ... --policy-check`; do not copy derived task, wave, or
  ordering arrays into the payload. The runtime reads the bounded artifact,
  verifies its digest and path boundary, and derives the task DAG atomically.
  If precheck reports an artifact error, fix the artifact and stop until the
  same payload passes precheck.

For either form:

- Live task IDs are minted by the runtime. Read them with
  `ralph tools task list` (or the prompt task block); never hand-write one.
- A task-mutation-authority lint finding is fatal: remove the CLI mutation and
  use the declared handoff rather than weakening the preset.
- Do not emit the handoff after any task mutation in the same activation.

> **Single writer**: while a projector action is configured for a topic, the
> projector owns task creation for that topic. Pick one creation path per
> handoff; mixing CLI mutation and projection can leave task state inconsistent.

### Projection-Owned Batch Close（结算事件关 task）

> **对偶于上一节**：当 preset 的 `state_projection.actions` 为某结算事件声明了 `close_task_batch`（典型形态：wave settle / fix settled / final correction），**该 hat 不调用 `ralph tools task close`**。runtime 投影器在结算事件 accept 后原子批量关闭；该 hat 与下游任何 hat 都不再手工关 task。

- **何时走这条路径**：你的 hat instructions 声明「该事件由 runtime 原子批量关闭 task，禁止手工 close」时。
- **agent 动作**：不调用 `ralph tools task close`。结算事件 payload 必须携带 live task IDs（`ralph tools task list` / trigger payload / `## ORCHESTRATOR CONTEXT`），runtime 接到结算事件后一次性投影；任何前置 close 都是冗余且可能与投影结果冲突。
- **停止条件**（任一即停 emit 并报告）：
  - IDs 缺失 / 为空 → runtime 整批拒绝。
  - IDs 含重复 / 未知 id → runtime 整批零写拒绝（拒绝先于任何持久化）。
  - IDs 混合 open + closed → runtime 整批拒绝（identity drift）。
  - ID 与实际 ledger row 不一致（跨 hat 互相覆盖）→ 整批拒绝。
- **fix-unit 例外**：结算里包含 fix-unit id（`is_fix_unit_id`）时，runtime 跳过 defensive-start、把 fix-unit row 直接 close 且保留 `started` 为 `None`（与单条 close 路径一致）。不要为了「补 started」去手工 start fix-unit——会与单条 close 路径产生 ledger 分叉。
- **OPAC Precheck (zero-write)**：结算事件 emit 前先 `ralph emit ... --policy-check`（同源校验，零写盘），同 always-injected `ralph-tools-opac` Precheck 段。

> **OPAC Precheck (zero-write)**: 任何 `task add` / `ensure` / `start` / `close` / `fail` / `reopen` 前先跑 `ralph tools task verify <verb>`（与正式写盘同源校验，零写盘）。三字段一致性走 `ralph tools task verify-emit-bridge --task-id ID --task-key KEY --step STEP`，详见 always-injected `ralph-tools-opac` Precheck 段。
>
> **OPAC Confirm (close 后)**: agent context 下 `task close` 成功后若 hat-channel 无 completion topic，CLI 会 stderr 输出 `close_without_completion_emit` warning，含 `expected_topics` + `next_step`——**忽略它等于进入 stall 30s 等待 rescue**。详见 `ralph-tools-opac` Confirm 段。

> **两步式 task verify gate（agent 强制）**: 当 preset 启用 `tasks.require_verify_for_cli_mutate: true` 时，agent 调 `task add` / `task ensure` **必须**先 `ralph tools task verify <verb> <args…>`（Allow 后 runtime 自动写 ticket）→ 再用**完全相同**参数调 `ralph tools task <verb>`。漂移（参数变了 / 跨 hat / 没先 verify）会被 `task_verify_gate denied` 拒收，**不写盘**。人类 CLI 永远 bypass；`tasks.allow_unsafe_task_mutate: true` 是 escape hatch（recovery 专用）。详见 `ralph-tools-opac` "Apply 阶段两步式 task verify gate" 段。
>
> **Task Confirmation（gate 内 Apply 成功后强制）**: gate 生效时，一次成功的 protected Apply（`task add` / `task ensure`）会在写入的 task 行上附带一条 confirmation 记录（状态 `pending`）。Apply 的 `--format json` 输出里有它的 `reference`（唯一确认凭证）和 `digest`（该 mutation 的指纹）。**在同一 loop + 同一 hat 发起下一次 protected mutation 之前**，必须先执行 `ralph tools task confirm <task_id> --reference <reference> --digest <digest>`（两个字段值直接取自 Apply 的 JSON 输出，不要手工构造）。未 Confirm 时，下一次 protected mutation 会被 `task_verify_gate denied ... confirmation_required` 拒收且不写盘——此时按 stderr 指引先 confirm 再重试，已 verify 的 ticket 仍有效。若产生 pending 记录的 Apply 输出已不在当前上下文（例如新一轮 iteration），执行 `ralph tools task show <task-id> --format json`，行内 `confirmation.reference` / `confirmation.digest` 即所需值，同样不要手工构造。重复 confirm（相同 reference + digest）幂等，exit 0。人类 CLI / gate 关闭 / unsafe hatch 三条 bypass 路径不产生 confirmation，也不受该门禁影响。

### First thing every iteration
```bash
ralph tools task ready    # What's open? Pick one. Don't create duplicates.
```

### When `task add` / `task ensure` is denied

Loop 里若 `ralph tools task add` 或 `task ensure` 返回 `hat_command_policy denied`，看 stderr 的 `[reason]` 和 hint：

| 现象 | 你该做什么 |
|------|-----------|
| hint 提到 **emit `work.ready`**（或你的 hat Trigger State Table 写「任务由事件创建」） | **停止** CLI 建 task。按 hat instructions：发 handoff 事件（常见字段 `task_key` + `step`），下一轮从 **trigger payload** 或 prompt 里的 **`## ORCHESTRATOR CONTEXT`** 读取 `task_id`。 |
| `[non_coordinator_owner]` | 只有 `tasks.coordinator_hats` 里的 hat 能 `add`/`ensure`；worker hat 用 `task list` 只读，等 coordinator 派 task。 |
| `task close` 报 **owned by hat** / **cannot close** | 不要 emit 业务失败事件来「兜底」。写 memory，等 `task.resume` 纠正；implementation 已完成 ≠ workflow 失败。 |

**不要**在 `task add` 被拒后换参数重试建第二套 id，也不要对同一 `task_id` 既 CLI 建 task 又往 handoff 事件里塞同一个 id（会产生重复行，`work.done` 可能对不上 live row）。

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
- **收到 `task.resume` 且其 `required_action` 指向「修复 task 记录」而非「重做实现」时**（典型措辞：删除/修正冲突行、对齐 key、让 projector 重新派生）：这是 ledger 修复指令，**不要**重写代码。应：① `ralph tools task list` / `show` 找到与目标 `task_key` 不一致的行；② 删除或修正该行；③ 重新触发 handoff 事件或 `task ensure` 让记录恢复一致。实现代码本轮已完成，无需再改。
- **任何 emit 的 `task_id` 必须真实且非空**：emit 前用 `ralph tools task list` / `show` 确认当前 loop 的 live id。`task_id=""`、`null` 或 `from_key:...` 会被拒收并破坏 step handoff。
- **task 反复失败时**：不要无限 reopen 同一 task；评估是否需要拆分为更小任务或提升到 `plan.blocked`。
- 更多细节见自动注入的 `## RECOVERY DIRECTIVES` 块（ID：`RD-PLAN-BLOCKED-ON-RECOVERY-EXHAUSTED`、`RD-TASK-ID-MUST-BE-LOOP-SCOPED`）。
