# CE executor pipeline review findings contract

This is the shared contract for all six dimension reviewer artifacts. The
dimension reviewer must read this file before writing its findings product.

## Required finding fields

Every `- id:` record under `## Findings` must contain all of these fields:

```yaml
- id: <dimension-prefix><number>
  dimension: <current-dimension>
  severity: P0 | P1 | P2 | P3
  confidence:
    overall: <number 0.0..1.0>
  evidence_score: <number 0.0..1.0>
  impact_score: <number 0.0..1.0>
  regression_probability: <number 0.0..1.0>
  risk_score: <number 0.0..1.0>
  merge_decision: block | warn | ignore
  owner: executor | plan-reviewer | manual | n/a
  description: <actionable description>
  evidence:
    - <repo-relative file:line, commit, command, or plan section>
  requires_verification: true | false
```

`dimension` is mandatory on every record and must exactly equal the current
hat's dimension value. The six allowed values are `goal-alignment`,
`correctness`, `testing`, `maintainability`, `project-standards`, and
`adversarial`. The event payload's `dimension` field does not replace this
per-record field.

## Required product checks before emit

After writing the product, reopen it and verify:

1. The `## Findings` section exists and every record has every required field.
2. Every record's `dimension` equals the current hat's fixed dimension.
3. `findings_count` equals the number of records.
4. `p0_count` and `p1_count` equal the counts derived from the records.
5. The product is non-empty and readable.

If any check fails, correct the product before running `ralph emit`. Never
emit a done event for a product that fails this contract. The downstream
review-synthesizer repeats these checks as a fail-close safety gate.

## No-finding product

Even when no issue is found, write one `G0`/`C0`/`T0`/`M0`/`S0`/`A0` record
with `severity: P3`, `dimension` set to the current dimension, and
`merge_decision: ignore`; set `findings_count: 1` and `p3_count: 1`.
