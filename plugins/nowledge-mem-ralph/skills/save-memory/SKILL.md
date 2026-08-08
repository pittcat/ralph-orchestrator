---
name: save-memory
description: Persist a durable Nowledge Mem Memory candidate after schema, hard-gate, and quality-threshold validation. Use only when a stable, reusable conclusion has emerged; do not save progress, logs, command transcripts, or one-shot workarounds.
---

# Save Memory (Ralph) — 0.2.0 lifecycle

> **0.2.0 lifecycle context.** Save-memory is the **only** path
> that writes a Memory record. The plugin owns the lifecycle:
> `validate schema → hard gate → quality thresholds → dedupe signal`,
> then hands an ACCEPTED record to the plugin writer. The command
> boundary performs the bounded write and reports its status; this skill
> does **not** call ``nmem`` directly and does **not** read transcripts.

Save durable knowledge. Not progress. Not logs. Not command
transcripts. Not one-shot workarounds. Memory is what a future
Ralph session would thank you for finding — a stable,
reusable conclusion with concrete evidence.

## When to save

**Strong signals (do save):**

- You have just resolved a root cause that another hat might hit
- You have a concrete, reusable decision backed by evidence
  (test path, doc reference, reproducer steps)
- You have identified a procedural convention that future work
  must follow
- You have just produced a hard-won constraint ("never do X
  because Y") with verification

**Skip when:**

- The "conclusion" is "we shipped the feature" — that is
  progress, hard-gate rejected
- The candidate is a transcript excerpt or command log
- The candidate is a workaround for a known bug that will be
  fixed upstream
- The candidate duplicates a memory already in the loop cache
  (the dedupe signal will short-circuit it anyway, but do not
  waste a turn)

## Fixed schema

The Memory schema is fixed; renaming a field is a breaking change.
All fields are required and must be non-empty strings (or empty
lists for the two critical-list fields):

```text
memory_type           durable_decision | durable_procedure
                       | durable_root_cause | durable_constraint
title                 ≥ 8 chars, must convey the claim
claim                 one-sentence assertion
why_it_matters        consequence of getting this wrong
evidence              concrete references / reproducer / test paths
applies_when          the triggering condition for this memory
scope                 plugin/loop/file scope the memory applies to
verification          how a future agent can confirm the claim
critical_assumptions  list, may be empty
critical_ambiguities  list, may be empty
metrics               seven-metric block, each value in [0, 100]
```

## Seven quality metrics

| Metric | Floor | What it measures |
|---|---|---|
| `confidence` | 80 | the agent's own confidence in the claim |
| `evidence_coverage` | 70 | how much of the claim is backed by evidence |
| `reusability` | 50 | how likely to apply to a future session |
| `stability` | 60 | durability across code changes |
| `scope_clarity` | 70 | how narrowly `scope` resolves ambiguity |
| `verifiability` | 50 | ease of re-checking the claim |
| `novelty` | 20 | new information (dedupe catches near-zero) |

## Hard gates

`memory_type` ∈ {`progress`, `log`, `command`, `transcript`} →
`REJECTED`. There is no override; raw process state does not
belong in Memory.

## Anti-hallucination rule

`confidence >= 90` and `evidence_coverage < 70` →
`REJECTED`. The rule is symmetric: high confidence must be
backed by evidence; otherwise it is a hallucination.

## Critical assumption / ambiguity

Non-empty `critical_assumptions` or `critical_ambiguities` →
`NEEDS_REWRITE`. Resolve the assumption inline (or rewrite the
claim so it does not depend on it) before saving.

## Submit the candidate

The skill should invoke the plugin's `memory.py` entry with the
candidate JSON on stdin. The plugin returns a JSON verdict on
stdout:

```bash
python3 "${CLAUDE_PLUGIN_ROOT}/scripts/memory.py" <<'JSON'
{
  "memory_type": "durable_decision",
  "title": "...",
  "claim": "...",
  "why_it_matters": "...",
  "evidence": "...",
  "applies_when": "...",
  "scope": "...",
  "verification": "...",
  "critical_assumptions": [],
  "critical_ambiguities": [],
  "semantic_review": false,
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

## Verdict handling

- `ACCEPTED` — the command boundary attempts the bounded writer call;
  you do not call nmem. The verdict carries `memory_digest` (64-char
  SHA-256), the structured `record`, and a nested `write` result.
  `write.result` is `SAVED`, `ALREADY_SAVED`, `FAILED_OPEN`, or
  `UNKNOWN`; `UNKNOWN` means the remote outcome must be reconciled
  before retrying the same digest.
- Set `semantic_review` to `true` when reuse, stability, scope, or novelty
  needs semantic judgment beyond the deterministic thresholds. The plugin
  then calls the configured structured evaluator; missing, invalid, timed-out,
  or side-effecting evaluator output is rejected for this candidate.
- `REJECTED` — surface `reason` and `missing_fields`; fix only if
  the user wants to refine. Do not retry with the same payload.
- `NEEDS_REWRITE` — structural rework required (assumptions or
  ambiguities). Do not retry until the rewrite is complete.
- `OBSERVATION` — the digest was already accepted in this scope;
  treat as success.

## What the skill does NOT do

- It does not call `nmem --json m add` directly. The plugin writer is
  the sole owner of that call; the command returns its bounded outcome
  alongside the policy verdict.
- It does not read transcripts or `last_assistant_message`. The
  candidate must come from the agent's own conclusion.
- It does not save progress, logs, command transcripts, or
  one-shot workarounds — the hard gate rejects them.
- It does not relax thresholds. The deterministic gates run in
  Python and cannot be lowered.
