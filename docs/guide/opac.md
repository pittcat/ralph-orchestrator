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
| **C — Confirm** | "Did it land? What is the next expected event?" | `ralph events --events-source hat-channel` (single emit) or `--events-source main` (wave emit) |

**Rule:** every `Apply` must be preceded by a `Precheck`, and every `Apply` must be followed by a `Confirm`. Skipping Precheck bypasses the schema gate; skipping Confirm risks silent drops.

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

After Apply, read back the correct ledger.

| Write path | Confirm command | File |
|------------|-----------------|------|
| `ralph emit` (single event) | `ralph events --events-source hat-channel` | `.ralph/current-hat-events` |
| `ralph wave emit` (batch) | `ralph events --events-source main` | `.ralph/events.jsonl` |

`--events-source auto` prefers the hat-channel when running inside an activation, so single emits are confirmed automatically. Wave emits always write the main ledger, so wave Confirm must use `--events-source main`.

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
| Event emitted but next hat never triggered | Confirmed on wrong ledger, or event was silently dropped after a second business event | Use `ralph events --events-source hat-channel` and emit only one business event per activation |
| `task.resume` injected after `task close` | Closed task without emitting a completion topic | Emit `work.done` / `test.passed` / etc. before closing, or before activation ends |
| Worker hat denied `task add` | Hat is not in `tasks.coordinator_hats` | Only coordinator-style hats may create tasks; worker hats should receive tasks via events |
| `review.wave.complete` rejected | Supervisor-only coordination topic emitted by an agent | Let the runtime inject coordination topics; agents emit only their own completion topics |

## Relationship to other guides

- [Payload Contracts](payload-contracts.md) — schema enforcement between hats.
- [Execution Contracts](execution-contracts.md) — `work.done` completion gate.
- [Precheck Gates](precheck-gates.md) — optional LLM-as-judge gate before key topics.
- [Runtime Diagnosis](runtime-diagnosis.md) — offline recovery reports.

For the agent-facing reference (injected into prompts as a skill), see [`crates/ralph-core/data/ralph-tools-opac.md`](../../crates/ralph-core/data/ralph-tools-opac.md).
