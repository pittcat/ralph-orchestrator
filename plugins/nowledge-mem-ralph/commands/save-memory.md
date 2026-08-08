---
description: Save a durable Memory candidate to Nowledge Mem after schema, hard-gate, and quality-threshold validation
argument-hint: <memory-json>
---

# Save Memory (Ralph) — 0.2.0 lifecycle

> **0.2.0 lifecycle context.** This command is the **only** path that
> writes a Memory record. The plugin owns the lifecycle:
> `validate schema → hard gate → quality thresholds → dedupe signal`.
> It invokes the plugin-owned writer only after policy returns ``ACCEPTED``.
> The writer performs a bounded argv-safe nmem write and durable digest
> dedupe; rejected candidates never reach nmem.
>
> **Do not** run this command on every iteration. Save-memory is
> for durable knowledge only — a stable, reusable conclusion you
> would want a future Ralph session to recall. Progress, logs,
> command transcripts, and one-shot workarounds do not belong in
> Memory and will be hard-gate rejected.

Submit a single Memory candidate as JSON on stdin. The schema is
fixed (see `MemorySchema.REQUIRED_FIELDS`) and the seven quality
metrics are mandatory.

## Fixed schema (MemorySchema.REQUIRED_FIELDS)

```text
memory_type           string, one of:
                        durable_decision | durable_procedure
                        durable_root_cause | durable_constraint
title                 string, ≥ 8 chars
claim                 string, non-empty
why_it_matters        string, non-empty
evidence              string, non-empty
applies_when          string, non-empty
scope                 string, non-empty
verification          string, non-empty
critical_assumptions  string list (may be empty)
critical_ambiguities  string list (may be empty)
metrics               object, every required metric in [0, 100]
```

## Seven quality metrics (MemorySchema.REQUIRED_METRICS)

| Metric | Floor | Purpose |
|---|---|---|
| `confidence` | 80 | the agent's own confidence in the claim |
| `evidence_coverage` | 70 | how much of the claim is backed by concrete evidence |
| `reusability` | 50 | how likely the knowledge applies to a future session |
| `stability` | 60 | how durable the conclusion is across code/system changes |
| `scope_clarity` | 70 | how narrowly the scope field resolves ambiguity |
| `verifiability` | 50 | how easy it is to reproduce / re-check |
| `novelty` | 20 | how much new information this carries (dedupe catches near-zero) |

## Hard gates (always reject)

`memory_type` ∈ {`progress`, `log`, `command`, `transcript`} →
`REJECTED`, regardless of metrics. Raw process state does not
belong in Memory.

## Anti-hallucination rule

`confidence >= 90` **and** `evidence_coverage < 70` → `REJECTED`.
This is the canonical hallucination shape (high confidence with
thin evidence); the rule fires before the per-metric floors so it
shows up clearly in the audit trail.

## Critical assumption / ambiguity

`critical_assumptions` non-empty **or** `critical_ambiguities`
non-empty → `NEEDS_REWRITE`. The candidate is recorded locally but
never reaches the writer.

## Command

Pipe the JSON candidate through the plugin's `memory.py` entry:

```bash
python3 "${CLAUDE_PLUGIN_ROOT}/scripts/memory.py" <<'JSON'
{
  "memory_type": "durable_decision",
  "title": "Use atomic os.replace for state.json writes",
  "claim": "All plugin state writes go through temp file + os.replace.",
  "why_it_matters": "Claude Code reads state.json on each SessionStart; a half-written file would cause env-detection branches to fire incorrectly.",
  "evidence": "hooks/hooks.json declares timeout 5; concurrent writer smoke test in test_hook_runtime.py::test_session_start_writes_state_marker.",
  "applies_when": "any new script in plugins/nowledge-mem-ralph/scripts that writes state.json or recall.json",
  "scope": "plugin:knowledge-mem-ralph",
  "verification": "pytest plugins/knowledge-mem-ralph/tests/ shows no torn writes across 3 concurrent SessionStart processes.",
  "critical_assumptions": [],
  "critical_ambiguities": [],
  "metrics": {
    "confidence": 95,
    "evidence_coverage": 88,
    "reusability": 90,
    "stability": 92,
    "scope_clarity": 96,
    "verifiability": 90,
    "novelty": 40
  }
}
JSON
```

The plugin prints a JSON verdict to stdout. The verdict carries:

- `result`: `ACCEPTED` | `REJECTED` | `NEEDS_REWRITE` | `OBSERVATION`
- `reason`: human-readable
- `missing_fields`: list of field names that drove the verdict
- `rewrite_suggestion`: hint to the agent
- `memory_digest`: 64-char SHA-256 hex when `ACCEPTED`
- `record`: the structured ACCEPTED record used by the plugin writer

## Auto-finalization marker (Stop / SubagentStop)

除了显式调用本命令,Claude 在 `Stop` 或 `SubagentStop` 的最终消息
中嵌入**一个** bounded marker 也可以触发自动 save-memory 写入。
marker 格式固定(plugin 不接受变体):

```text
<!-- nowledge-memory-finalize
{"finalize":true,"memory_type":"durable_decision","title":"...","claim":"...","why_it_matters":"...","evidence":"...","applies_when":"...","scope":"...","verification":"...","critical_assumptions":[],"critical_ambiguities":[],"metrics":{"confidence":95,"evidence_coverage":88,"reusability":90,"stability":92,"scope_clarity":96,"verifiability":90,"novelty":40}}
-->
```

规则(违反任何一条即跳过保存):

- 标记名必须精确为 `nowledge-memory-finalize`,且全文只能出现一次。
- 中间必须是合法 UTF-8 JSON object(单 marker ≤ 16 KiB)。
- 对象必须包含字段 `finalize: true`(JSON 布尔字面值)。
- 其余字段必须满足上面的固定 schema 与七项质量指标。
- 出现多个 marker 时拒绝解析,plugin 不会自行择一。
- plugin 不读取 `transcript_path`,也不会把 Thread / transcript 当
  作 Memory。
- 自动保存走现有 writer,nmem 失败时 Stop 仍 exit 0。

## Verdict handling

- `ACCEPTED` — the plugin writer attempts the bounded nmem write; do not
  call nmem yourself. Inspect the nested `write` result for `SAVED`,
  `ALREADY_SAVED`, `FAILED_OPEN`, or `UNKNOWN`.
- `REJECTED` — surface the `reason` and `missing_fields` to the
  user; fix and retry only if the user wants to refine the
  candidate.
- `NEEDS_REWRITE` — the candidate needs structural rework
  (unverified assumptions or scope ambiguity). Do not retry until
  the rewrite is done.
- `OBSERVATION` — the digest was already accepted in this scope.
  Treat as success: the knowledge is already in Memory.

## Empty / malformed input

If the JSON payload cannot be parsed, the plugin prints the error
to stderr and exits 1. Do not treat this as a Memory verdict —
fix the JSON and resubmit.
