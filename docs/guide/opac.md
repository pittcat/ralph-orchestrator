# OPAC Agent Discipline

> **OPAC** = **O**bserve → **P**recheck → **A**pply → **C**onfirm
>
> A four-phase workflow that every hat must follow for any state-changing operation in `isolated` mode. It is the primary defense against silent drops, permission leaks, and stalled loops.

## When you need this guide

Use this guide when:

- You are writing or editing a preset that runs in `event_loop.execution_mode: isolated`.
- An agent in your loop was denied, emitted an event that disappeared, or left a task closed without a completion event.
- You want to understand why `ralph emit` refuses to write in agent context unless `--policy-check` passes first.
- You are debugging a `ce-executor-*` preset and see `close_without_completion_emit` warnings or `task.resume` events.

## The four phases

| Phase | Question the agent must answer | Typical commands |
|-------|--------------------------------|------------------|
| **O — Observe** | "Who am I? What is the loop state?" | `ralph inspect loop`, `ralph tools task list`, `ralph events --events-source hat-channel` |
| **P — Precheck** | "Will this operation succeed without side effects?" | `ralph tools task verify ...`, `ralph emit <topic> --policy-check ...`, `ralph wave verify ...` |
| **A — Apply** | "Execute the operation now." | `ralph tools task ...`, `ralph emit ...`, `ralph wave emit ...` |
| **C — Confirm** | “预期状态真的产生了吗？下一步是什么？” | 优先检查操作通过公开接口给出的成功反馈；反馈不足时，再使用该操作文档规定的只读查询接口 |

**规则：**每个 `Apply` 之前必须有 `Precheck`，每个 `Apply` 之后必须完成 `Confirm`；Confirm 不可省略。Apply 后必须找到当前操作对应的专项 skill，并取得该 skill 规定的有效证据。未找到 skill、未取得证据或证据不一致时，必须停止，不得继续下一次状态变更。Confirm 不要求所有操作机械地执行同一条查询命令；具体证据与查询方式由专项 skill 定义。

## Where the rules come from

OPAC is enforced at three layers, all derived from the resolved preset (`RalphConfig`) rather than hard-coded hat names:

1. **L1 Prompt layer** — the runtime injects a `## HAT IDENTITY` block into every activation. It lists the current `hat_id`, the topics this hat may publish, the topics it may trigger, and the task command permissions it has.
2. **L2 CLI ACL layer** — `HatCommandPolicy` blocks agent-context calls that violate the hat's role. For example, a non-coordinator hat cannot call `ralph tools task add` or `ralph tools task ensure`.
3. **L3 Runtime layer** — existing gates (`check_isolated_scope`, `ValidationPipeline`, completion-emit warnings) reject or warn about malformed events.

The L1 source of truth is `HatRegistry::can_publish(hat, topic)` plus `tasks.coordinator_hats`. The L2 source of truth is the same, plus optional `tasks.command_rules`.

## Observe

Before any state-changing command, the agent should know:

1. **Its own identity** — read the `## HAT IDENTITY` block in the prompt, or run:
   ```bash
   ralph inspect loop --format json
   ```
2. **Loop phase** — read the `## ORCHESTRATOR CONTEXT` block, or inspect the same JSON output.
3. **Task state** — list live tasks and their statuses:
   ```bash
   ralph tools task list
   ralph tools task ready
   ```
4. **Where recent events landed** — single emits are merged into the hat-channel after the backend exits:
   ```bash
   ralph events --events-source hat-channel
   ```
   For cross-hat debugging, read the main ledger:
   ```bash
   ralph events --events-source main
   ```
5. **Supervisor state (when enabled)** — if the preset declares `event_loop.supervisor.enabled: true`, `ralph inspect loop --format json` includes an agent-safe `supervisor` summary (`active_waves`, `queue_depth`, `slot_summary`, `last_coordination_topics`). It never exposes the database path or raw ledger contents.

## Precheck

Precheck runs the same authorization and schema logic as the real command, but writes nothing.

### Task changes

```bash
# Verify a task operation before running it
ralph tools task verify add --task-key plan:my-plan:step-01:unit
ralph tools task verify ensure --task-key plan:my-plan:step-01:unit
ralph tools task verify start --task-id <live-id>
ralph tools task verify close --task-id <live-id>
```

Use the dedicated bridge command when you need to cross-check emit payload fields against the task store:

```bash
ralph tools task verify-emit-bridge --task-id <live-id> --task-key <task-key> --step <step>
```

The three correlation fields (`task_id`, `task_key`, `step`) must be internally consistent. `task_id` is a live id returned by `ralph tools task list`; `task_key` is the stable registration key; `step` must match the `:step-<n>:` segment of `task_key`.

**Task confirmation（gate 开启时）**：当 `tasks.require_verify_for_cli_mutate: true` 时，一次成功的 protected Apply（`task add` / `task ensure`）会在写入的 task 行上附带一条状态为 `pending` 的 confirmation 记录；Apply 的 `--format json` 输出含 `confirmation.reference`（唯一确认凭证）与 `confirmation.digest`（该 mutation 的指纹）。在同一 loop + 同一 hat 发起**下一次** protected mutation 之前，必须先执行：

```bash
ralph tools task confirm <task_id> --reference <reference> --digest <digest>
```

两个参数值直接取自上一步 Apply 的 JSON 输出，不要手工构造，也不要复用其它 task 的值。若产生 pending 记录的 Apply 输出已不在当前上下文（例如新一轮 iteration），执行 `ralph tools task show <task_id> --format json`，行内 `confirmation.reference` / `confirmation.digest` 即所需值，同样不要手工构造。未 confirm 时，下一次 protected mutation 会被 `task_verify_gate denied ... confirmation_required` 拒收且不写盘；confirm 成功后，同 scope 的下一次 protected mutation 放行。重复 confirm（相同 reference + digest）幂等：exit 0，不重复写盘。人类 CLI、gate 关闭、`tasks.allow_unsafe_task_mutate: true` 三条 bypass 路径不受该门禁影响。

### Single event emit

```bash
ralph emit work.ready --policy-check -j '{"plan_name":"my-plan", ...}'
```

In agent context, `ralph emit` without `--policy-check` is rejected by default. The rejection message tells the agent exactly which required fields are missing or which values are illegal.

### Wave emit (concurrent presets)

```bash
ralph wave verify --payloads-stdin < payloads.jsonl
```

Only dispatcher hats may wave-emit; worker hats are prohibited by `HatCommandPolicy`.

## Apply

Run the real command only after Precheck succeeds.

```bash
ralph tools task add --task-key plan:my-plan:step-01:unit --title "Implement unit 1"
ralph emit work.ready -j '{"plan_name":"my-plan", ...}'
ralph wave emit --payloads-stdin < payloads.jsonl
```

### Apply-time red lines

- Do **not** call `ralph tools task create` or `ralph task create` — these commands do not exist. Use `add` or `ensure`.
- Do **not** call `ralph tools task add` from a hat that is not in `tasks.coordinator_hats`.
- Do **not** emit more than one business event in the same activation. In isolated mode only the first business event is kept; subsequent events, and any terminal event placed after them, are silently dropped.
- Do **not** emit a terminal event (`plan.complete`, `LOOP_COMPLETE`, etc.) immediately after another business event — the terminal event will be dropped.
- Do **not** write directly to `.ralph/events.jsonl`, `.ralph/agent/tasks.jsonl`, or `.ralph/supervisor.db`.

## Confirm

通过该操作的公开契约确认预期效果：

1. 先按下表找到并加载当前操作对应的专项 skill，不得凭经验猜测什么证据有效。
2. 按专项 skill 检查公开成功反馈或只读查询结果；仅有命令退出成功不算完成 Confirm。
3. 区分**操作成功**与**流程推进**。状态变更成功不代表下游 hat 已经处理，也不代表工作流已经推进。只有当前任务要求这类更强证据时，才查询下游可见状态。
4. 状态不明确时停止。未找到专项 skill、缺少有效证据、只读查询结果不一致，或 warning 指出仍有必需动作时，应先解决问题，再执行下一次状态变更。

| 当前操作 | Confirm 规则来源 | Agent 动作 |
|---------|------------------|------------|
| `ralph emit` | `ralph-tools-emit` | `ralph tools skill load ralph-tools-emit` |
| `ralph tools task` | `ralph-tools-tasks` | 使用 prompt 中已注入的 task skill；若不可见则停止 |
| `ralph tools memory` | `ralph-tools-memories` | 使用 prompt 中已注入的 memory skill；若不可见则停止 |
| `ralph wave` | `ralph-tools-wave` | `ralph tools skill load ralph-tools-wave` |
| 无法判断操作类别 | 当前 hat 的可见 skill 列表 | `ralph tools skill list --format json`；仍找不到就停止，不得猜测 |

不要为了完成 Confirm 而读取或修改 runtime 内部文件。使用 `ralph-tools-emit`、`ralph-tools-tasks` 和 `ralph-tools-wave` 记录的公开入口。

### Task close warning

After `ralph tools task close`, the runtime checks whether a completion topic (from `event_policy.terminal_topics` / `event_policy.business_topics` intersected with the hat's `publishes`) has been written to the hat-channel. If not, it prints a structured stderr warning:

```json
{"warning":"close_without_completion_emit","expected_topics":["work.done"],"next_step":"emit completion topic before this activation ends"}
```

The close itself is **not** blocked, but ignoring the warning often causes a 30-second stall followed by a recovery `task.resume`.

## Configuration and overrides

### Opt out of agent-context policy-check enforcement

By default, agent context enforces `ralph emit --policy-check`. Presets can opt out by setting:

```yaml
event_loop:
  allow_unsafe_cli_emit: true
```

This is strongly discouraged for production presets; it exists mainly for local debugging and legacy migration.

### Per-hat command rules

Advanced presets can add `tasks.command_rules` to extend or restrict the default command matrix derived from `coordinator_hats`. See [`ralph-tools-tasks.md`](../../crates/ralph-core/data/ralph-tools-tasks.md) for the red-box rules on `task_id`, `task_key`, and `step`.

## Common failure modes

| Symptom | Likely cause | Fix |
|---------|--------------|-----|
| `ralph emit` rejected with missing required field | Skipped Precheck or wrong payload | Run `ralph emit <topic> --schema <TOPIC>` to see required fields |
| event 操作成功但下一个 hat 未触发 | 把操作成功误当成下游流程已经推进，或额外业务事件被丢弃 | 使用 event 指南支持的诊断查询；每次 activation 只发送一条真正需要的业务事件 |
| `task.resume` injected after `task close` | Closed task without emitting a completion topic | Emit `work.done` / `test.passed` / etc. before closing, or before activation ends |
| Worker hat denied `task add` | Hat is not in `tasks.coordinator_hats` | Only coordinator-style hats may create tasks; worker hats should receive tasks via events |
| `review.wave.complete` rejected | Supervisor-only coordination topic emitted by an agent | Let the runtime inject coordination topics; agents emit only their own completion topics |
| `task_verify_gate denied ... confirmation_required` | 同一 loop + hat 上一条 protected Apply 留下的 confirmation 仍为 `pending` | 先 `ralph tools task confirm <task_id> --reference ... --digest ...` 再重试该 mutation；prepared verify ticket 保留，同一参数无需重新 verify |
| `task confirm denied: confirmation_mismatch` / `confirmation_unavailable` | reference / digest / loop+hat scope 不符，或 task / confirmation 记录不存在 | 重新读取产生该 confirmation 记录的 Apply JSON 输出，取 `confirmation.reference` 与 `confirmation.digest` 后重试；不要靠猜参数重试 |
| `task confirm denied: confirmation_scope_conflict` | 目标 task 行已有其它 loop/hat 记录的 pending confirmation | 由记录方（该 loop/hat 的 agent）先执行 confirm；可用 `ralph tools task show <task_id> --format json` 查看记录的 `loop_id` / `hat_id` 归属；不要试图用其它 scope 的凭证覆盖 |

**预期行为（gate bypass 与遗留 pending）**：gate 关闭（`tasks.require_verify_for_cli_mutate: false`）或 `tasks.allow_unsafe_task_mutate: true` 期间，runtime 不会清除已存在的 pending confirmation——bypass 路径只是不再拦截新的 mutation。gate 重新开启后，旧的 pending 记录仍会让下一次 protected mutation 收到 `confirmation_required` 拒收，按正常 `ralph tools task confirm` 流程解除。

## Relationship to other guides

OPAC 是状态变更的操作纪律框架；`event_policy.payload_consistency` 是可选的同 payload 验收硬闸，`execution_contracts` 则负责 PostCommit 证据义务，三者互补而不互相替代。

- [Payload Contracts](payload-contracts.md) — schema enforcement between hats.
- [Execution Contracts](execution-contracts.md) — `work.done` completion gate.
- [Precheck Gates](precheck-gates.md) — optional LLM-as-judge gate before key topics.
- [Runtime Diagnosis](runtime-diagnosis.md) — offline recovery reports.

For the agent-facing reference (injected into prompts as a skill), see [`crates/ralph-core/data/ralph-tools-opac.md`](../../crates/ralph-core/data/ralph-tools-opac.md).
