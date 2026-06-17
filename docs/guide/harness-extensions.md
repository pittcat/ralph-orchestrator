# Harness Extensions

Ralph's **Harness Extensions** are four opt-in mechanisms that turn "soft conventions"
into "hard guarantees" — without changing any existing behavior when disabled.

They are designed for methodology-heavy workflows (such as Universal AutoResearch)
where consistency across agents and sessions is critical.

---

## Overview

| Extension | Problem Solved | What It Does |
|-----------|---------------|--------------|
| **Event Filtering** (FR-1) | Red-team anchoring bias | Each Hat sees only events it explicitly allows |
| **Event Projection** (FR-2) | Ledger gaps | Matching events are auto-copied to sidecar JSONL files |
| **State File Injection** (FR-3) | Unreliable scratchpad state | External JSON/JSONL files are injected into prompts as structured XML blocks |
| **Preflight Hooks** (FR-4) | Skipped validation steps | External commands run as part of `ralph preflight` |

**Design principles:**
- **Default off** — `enabled: false` (or absent) means zero behavioral change
- **Zero regression** — `serde(default)` guarantees old configs load unchanged
- **Fail-soft** — any extension failure prints a `stderr` warning and continues; the main loop is never blocked

---

## Event Filtering (FR-1)

### The Problem

All Hats share the full event history. A red-team Hat that is supposed to review
decisions independently may be anchored by seeing the evaluator's scores in its
context window.

### The Solution

Per-Hat **allowlist filtering**. When every active Hat has an enabled filter,
Ralph computes the **union** of their allowed events (plus their trigger topics)
and shows only those events in the prompt.

Events are still published to the bus and persisted — filtering affects only
**what enters the prompt**.

### Configuration

```yaml
hats:
  red_team:
    name: "Red Team"
    triggers: ["experiment.propose"]
    publishes: ["redteam.review"]
    event_filter:
      enabled: true
      mode: allowlist          # Only allowlist is supported today
      events:
        - "experiment.propose"
        - "experiment.design"
        # Scores and evaluations are NOT listed → red team never sees them

  evaluator:
    name: "Evaluator"
    triggers: ["experiment.run"]
    publishes: ["eval.score"]
    event_filter:
      enabled: true
      mode: allowlist
      events:
        - "experiment.run"
        - "experiment.result"
        - "redteam.review"
```

**Union semantics:** If `red_team` allows `{experiment.propose, experiment.design}`
and `evaluator` allows `{experiment.run, experiment.result, redteam.review}`,
the prompt contains events whose topic is in the union of both sets (plus their
triggers). Neither Hat sees the other's private scoring events.

**Fallback:** If even one active Hat lacks `event_filter.enabled: true`, filtering
is disabled for that iteration — Ralph shows the full event history. This prevents
accidental information starvation.

**Isolated mode difference:** In `execution_mode: isolated`, each Hat runs in its own
backend process and sees only its own allowed events. The union semantics above apply
to `coordinator` mode where multiple Hats share a single prompt. In isolated mode,
each Hat's filter is applied independently, so a Hat never sees events that another
Hat's filter would have added to the union.

**Isolated terminal authority (U3)**: In isolated mode, every agent-emitted terminal
topic — `LOOP_COMPLETE`, `review.complete`, `report.done`, `plan.blocked`, and any
other end-of-flow topic — must be declared in the current Hat's `publishes` list.
The `EventOriginGuard` rejects undeclared terminal publications with an
`event.isolation.boundary_violation` diagnostic and injects a `task.resume` so the
agent sees the failure reason. `default_publishes` fallbacks follow the same rule —
the default topic must also appear in `publishes`, otherwise the original guard
still rejects. The only exception is the runtime-injected `hat=ralph` pseudo-hat,
which is granted `LOOP_COMPLETE` / `work.start` / `loop.cancel` by
`HatRegistry::from_runtime_config()`. See the [Multi-Hat Isolation Policy section
in `configuration.md`](./configuration.md#multi-hat-isolation-policy-mandatory) for
the full rule.

**Isolated fair scheduling (U4)**: `EventBus` selects the next isolated Hat with
pending events using a stable round-robin cursor instead of the old
"BTreeMap-first-key" order. After a Hat is selected, the cursor advances past it;
on the next scheduling decision, the search resumes from that position. The
historical "always pick the alphabetically first pending Hat" behavior is removed —
do not rely on it in tests, prompts, or skill docs. The waiting bound for any Hat
sharing a pending pool is `N - 1` other selections, where `N` is the number of
Hats with non-empty pending queues.

### Validation

`ralph preflight` checks that:
- No event name is empty
- No event name contains spaces

Violations fail the preflight with a descriptive message.

---

## Event Projection (FR-2)

### The Problem

Downstream tools (dashboards, analyzers, audit scripts) must parse the entire
events JSONL file to find specific event types. If an evaluator Hat forgets to
append to the experiment ledger, the audit trail has gaps.

### The Solution

**Automatic projection**: When an event matches a configured topic, Ralph extracts
specified fields and appends a JSON line to a target file.

### Configuration

```yaml
core:
  event_projection:
    enabled: true
    rules:
      - name: experiment-ledger
        trigger_events:
          - "experiment.run"
          - "experiment.complete"
        fields:
          - "topic"
          - "payload"
        target_file: .ralph/experiments.jsonl
        mode: append
```

### Field Extraction

| Field | Source |
|-------|--------|
| `topic` | Event topic string |
| `payload` | Raw event payload string |
| `wave_id` | Wave ID if present |
| any other | Key lookup in payload JSON (falls back to `null`) |

### Output Format

Each matching event produces one JSONL line:

```jsonl
{"topic":"experiment.run","payload":"{\"id\":\"exp-42\"}"}
{"topic":"experiment.complete","payload":"{\"id\":\"exp-42\",\"score\":0.91}"}
```

The target directory is created automatically. Projection happens **after**
successful `EventBus::publish()`, so an event that fails to persist is never
projected.

---

## State File Injection (FR-3)

### The Problem

Bayesian beliefs, UCB scores, and other structured state are often written to
scratchpad free text. Context truncation and LLM "rot" make these values
unreliable — you are asking an LLM to do arithmetic on text it may have
partially forgotten.

### The Solution

Inject external **structured state files** (JSON/JSONL) directly into the prompt,
wrapped in typed XML blocks. The agent receives the raw data, not a paraphrase.

### Configuration

```yaml
core:
  state_files:
    enabled: true
    inject_preamble: |
      ## Strategy State
      The following files contain the current Bayesian beliefs and UCB scores.
      Trust these values over any stale scratchpad references.
    files:
      - path: .ralph/agent/strategy_state.json
        format: json
        char_budget: 4000       # Keep last 4000 chars if file is large
      - path: .ralph/agent/experiment_log.jsonl
        format: jsonl
        tail_lines: 50          # Keep last 50 lines
```

### Injected Format

```xml
## Strategy State
The following files contain the current Bayesian beliefs and UCB scores.
Trust these values over any stale scratchpad references.

<state-file name=".ralph/agent/strategy_state.json" format="json">
{
  "beliefs": { "hypothesis_a": 0.73, "hypothesis_b": 0.27 },
  "ucb_scores": { "arm_1": 1.42, "arm_2": 0.98 }
}
</state-file>

<state-file name=".ralph/agent/experiment_log.jsonl" format="jsonl">
<!-- earlier content truncated (1247 chars omitted) -->
{"experiment":"exp-99","score":0.88,"arm":"arm_2"}
{"experiment":"exp-100","score":0.91,"arm":"arm_1"}
</state-file>
```

State files are injected **after scratchpad, before ready tasks**, so they sit
near the top of the prompt where attention is strongest.

### Truncation Strategies

| Strategy | Behavior |
|----------|----------|
| `char_budget` | Keep the tail (most recent chars); prepend truncation notice |
| `tail_lines` | Keep the last N lines; no truncation notice needed |
| both | `tail_lines` applied first, then `char_budget` on the result |

If a file is missing or unreadable, an empty `<state-file>` block is injected
with a `stderr` warning — the loop continues.

---

## Preflight Hooks (FR-4)

### The Problem

Validation and audit steps written in Skill documents can be skipped by different
Agent platforms or when agents deviate from instructions.

### The Solution

Make external validation commands a **native preflight check**. Hooks run as
part of `ralph preflight` (and `ralph run` when `features.preflight.enabled: true`),
producing pass/warn/fail results just like built-in checks.

### Configuration

```yaml
core:
  preflight_extensions:
    enabled: true
    hooks:
      - name: validate-specs
        command: "python scripts/validate_specs.py {{config_dir}}/specs"
        stage: before_native
        fail_on_error: true

      - name: audit-budget
        command: "jq '.total_cost' {{project_root}}/.ralph/budget.json"
        stage: after_native
        fail_on_error: false
```

### Hook Stages

| Stage | When It Runs | Typical Use |
|-------|--------------|-------------|
| `before_native` | Before built-in checks (config, git, paths, etc.) | Environment guards, dependency checks |
| `after_native` | After built-in checks | Audit, reporting, budget checks |

### Template Variables

| Variable | Value |
|----------|-------|
| `{{config_path}}` | Absolute path to the loaded config file |
| `{{config_dir}}` | Directory containing the config file |
| `{{project_root}}` | Workspace root (`core.workspace_root`) |

Variables are substituted before the shell executes the command.

### Failure Behavior

- `fail_on_error: true` + non-zero exit → preflight **fails** (blocks `ralph run`)
- `fail_on_error: false` + non-zero exit → preflight **warns** (logs, does not block)
- Execution failure (spawn error) → treated according to `fail_on_error`

Stdout is ignored; stderr is captured and shown in the check message.

---

## Putting It All Together: AutoResearch Example

```yaml
# ralph.autoresearch.yml
core:
  event_projection:
    enabled: true
    rules:
      - name: experiment-ledger
        trigger_events: ["experiment.run", "experiment.complete", "eval.score"]
        fields: ["topic", "payload"]
        target_file: .ralph/autoresearch.jsonl

  state_files:
    enabled: true
    inject_preamble: "## Current Strategy State"
    files:
      - path: .ralph/agent/beliefs.json
        format: json
        char_budget: 2000

  preflight_extensions:
    enabled: true
    hooks:
      - name: check-ledger-schema
        command: "jq 'has(\"topic\") and has(\"payload\")' .ralph/autoresearch.jsonl"
        stage: before_native
        fail_on_error: false

hats:
  strategist:
    name: "Strategist"
    triggers: ["loop.start", "eval.score"]
    publishes: ["experiment.propose"]
    event_filter:
      enabled: true
      events:
        - "loop.start"
        - "eval.score"
        - "experiment.result"

  red_team:
    name: "Red Team"
    triggers: ["experiment.propose"]
    publishes: ["redteam.review"]
    event_filter:
      enabled: true
      events:
        - "experiment.propose"
        - "experiment.design"

  evaluator:
    name: "Evaluator"
    triggers: ["experiment.run"]
    publishes: ["eval.score"]
    event_filter:
      enabled: true
      events:
        - "experiment.run"
        - "experiment.result"
        - "redteam.review"
```

---

## Isolated Mode with Extensions

When `execution_mode: isolated` is enabled, each Hat runs in its own backend process
and the four extensions compose as follows:

| Extension | Behavior in isolated mode |
|-----------|--------------------------|
| Event Filtering | Applied per-Hat independently (no union). Each Hat sees only its own allowlisted events. |
| Event Projection | Runs after accepted events are published, accumulating across all Hat turns. |
| State File Injection | Injected into every isolated Hat prompt at the same prepend position. |
| Preflight Hooks | Run before each isolated Hat backend execution, not just once per Ralph iteration. |

### AutoResearch Example

A complete configuration combining isolated execution, workflow guards, event filtering,
and event projection for an AutoResearch pipeline:

```yaml
event_loop:
  execution_mode: isolated
  starting_event: "experiment.start"
  # U3: 过滤 stale rejection。默认 300s，0 表示关闭。
  task_resume_ttl_seconds: 300
  # U5: progress-steward 配置。默认关闭（enabled: false），
  # 需在 preset 中显式开启。仅在 stall/recovery 路径激活，
  # 不订阅正常业务事件。
  progress_steward:
    enabled: false
    steward_hat_id: "progress-steward"
    max_steward_iterations: 3
  workflow_guards:
    chains:
      - name: experiment
        topics:
          - experiment.planned
          - experiment.ready
          - experiment.measured
          - experiment.scored
          - experiment.evaluated
        mode: strict
        correlation:
          from_payload: experiment_id

core:
  event_projection:
    enabled: true
    rules:
      - name: experiment-ledger
        trigger_events:
          - "experiment.planned"
          - "experiment.ready"
          - "experiment.measured"
          - "experiment.scored"
          - "experiment.evaluated"
        fields:
          - "topic"
          - "payload"
        target_file: .ralph/experiments.jsonl
        mode: append

hats:
  planner:
    name: "Planner"
    triggers: ["experiment.start"]
    publishes: ["experiment.planned"]
    event_filter:
      enabled: true
      events:
        - "experiment.start"
    instructions: |
      Plan the experiment.

  implementer:
    name: "Implementer"
    triggers: ["experiment.planned"]
    publishes: ["experiment.ready"]
    event_filter:
      enabled: true
      events:
        - "experiment.planned"
    instructions: |
      Implement the plan.

  evaluator:
    name: "Evaluator"
    triggers: ["experiment.ready"]
    publishes: ["experiment.evaluated"]
    event_filter:
      enabled: true
      events:
        - "experiment.ready"
        - "experiment.measured"
        - "experiment.scored"
    instructions: |
      Evaluate the experiment results.
```

In this setup:
- Each phase runs in isolation — the evaluator never sees the planner's instructions.
- Workflow guards enforce that `experiment.scored` must precede `experiment.evaluated`.
- Event projection accumulates every phase into `.ralph/experiments.jsonl` for audit.
- Event filtering ensures each Hat's prompt contains only the events it needs.

---

## Compatibility & Migration

- All extension fields are `Option<T>` with `serde(default)` — adding them to an
  existing config is safe and does not change behavior unless `enabled: true` is set.
- Removing an extension section (or setting `enabled: false`) restores pre-existing
  behavior immediately.
- Preflight hooks are validated by name uniqueness; duplicate names cause a config
  validation warning.

---

## Runtime Ralph Hat

Ralph registers a **builtin** `ralph` hat in the runtime hat registry for every run.
This hat:

- **Does NOT require manual declaration** in preset YAML. It is automatically present
  as the universal fallback coordinator.
- **Has a derived publish scope** based on the configured topology (starting event,
  completion promise, cancellation promise, and all configured hats' triggers and
  publishes). This means `hat=ralph` can emit `work.start`, `LOOP_COMPLETE`, and
  `loop.cancel` without additional configuration.
- **Cannot publish off-graph topics.** A `hat=ralph topic=totally.fake` event is
  still rejected by the event origin guard.
- **Is registered via `HatRegistry::from_runtime_config()`**, which combines user-
  configured hats with the builtin runtime hat. The event loop and all origin guard
  checks use this unified registry.

### Cancellation Default

`loop.cancel` is now **enabled by default** as the cancellation promise topic.
To disable:

```yaml
event_loop:
  cancellation_promise: ""    # empty string disables loop.cancel
```

With the default enabled, agents can emit `loop.cancel` to trigger graceful early
termination without chain validation. The loop exits with code 0 (success) and
`TerminationReason::Cancelled`. This is intended for human rejection paths, timeout
escalation, or any scenario requiring an abort while keeping the workspace intact.

---

## Troubleshooting

| Symptom | Cause | Fix |
|---------|-------|-----|
| Events still visible despite filter | Not all active Hats have `event_filter.enabled: true` | Enable filtering on every Hat, or accept full-history fallback |
| Projection file empty | `trigger_events` mismatch or `event_projection.enabled: false` | Verify event topics match exactly (no wildcards) |
| State file not in prompt | `state_files.enabled: false` or path resolution failure | Check path is relative to workspace root; look for `stderr` warnings |
| Preflight hook "Failed to execute" | Shell or command not found | Ensure the command works standalone; hooks run via `sh -c "..."` |
| `{{config_path}}` resolves to empty string | Config loaded from stdin or memory | Use `{{project_root}}` or absolute paths instead |

---

## See also: Payload Contracts

Beyond the four mechanisms above, Ralph also enforces **payload
contracts** between hats. Payload contracts check that every event
carries the fields its downstream consumers expect. They run both at
preset-load time (`ralph hats validate --strict` and the `ralph run`
startup hard gate) and at runtime (when `event_policy.mode: enforce`).

See `docs/guide/payload-contracts.md` for the schema format,
extractor behaviour, runtime diagnostic fields, and the boundary with
execution contracts.
