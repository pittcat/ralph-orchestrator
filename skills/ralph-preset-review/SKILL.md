---
name: ralph-preset-review
description: Review Ralph preset YAML with agent-native AAF audit, mechanical lint, and structured preset-review-report.md. Use after drafting a preset or when validating builtin/local presets for per-hat feasibility, handoff closure, OPAC discipline, and P0/P1 issues. Produces findings with severity and confidence scores.
---

# Ralph Preset Review

Use this skill to **review** Ralph presets with **Agent 视角可行性（AAF）** — independent per-hat activation simulation, payload audit, and mechanical lint.

**Boundary:** Does not replace Rust `preset_lint` rules. Does not run full `./scripts/run-tests.sh` by default. For drafting, use `ralph-preset-author`.

**Deliverable:** Every review MUST write **`preset-review-report.md`** — not chat-only summaries.

## Use This Skill For

- Reviewing builtin or local presets before merge
- Finding P0/P1 issues: invisible inputs, broken handoffs, illegal commands, ledger reads, **invisible / fabricated / semantically unusable payload fields**
- Finding policy-check feedback adoption gaps: missing `field_docs`, unsafe `examples`, or emitter instructions that do not cite `ralph-tools-emit` Policy-Check feedback
- Running `ralph preset check --strict` and preset_lint nextest subsets
- Producing actionable remediation from AAF gaps + payload audit gaps

## Core Assumptions

- **Do not trust** `preset-author-notes.md` — rebuild AAF + payload audit independently; use notes only to flag author/review mismatches.
- **Simulate one hat activation at a time** — declare explicitly: "I am simulating hat X's activation."
- **Confidence ≥ 60** required for findings in the main table; below 60 → discard, re-investigate (max 2 rounds), or `Unverified Suspicions`.
- **Shape passing ≠ payload usable.** `ralph emit --schema` / `--policy-check` prove shape only. Field visibility, value source, identity, semantic sufficiency, downstream consumption are review's job.
- **Schema metadata is repair guidance, not truth.** `field_docs` / `examples` must match the payload audit, but they never replace visibility, value-source, or downstream semantic review.

## Workflow

1. Read **topology-only** fields from preset YAML: `event_loop`, `hats.*.triggers`, `hats.*.publishes`, `state_projection`, `event_policy` — **not** other hats' `instructions` yet.

2. Record `execution_mode`, hat count, preset path (`builtin:` vs file).

3. **Topology sketch** — event flow diagram (not prompt flow).

3a. **Single-chain-first audit (2026-07-07-006 Unit 6)** — mandatory:
   - Read `references/finding-rubric.md` 「Single-chain-first audit」段；按 `fallback_reaches_success_terminal` / `runtime_unit_loop_multiple_fact_sources` / `blocked_failed_promoted_to_pass` / `topic_multi_consumer` / `hidden_phase_decision` / `prompt_wall_serial_style` 六项逐项判定。
   - 任一命中 → 报告 P0（`fallback_reaches_success_terminal` / `runtime_unit_loop_multiple_fact_sources` / `blocked_failed_promoted_to_pass`）或 P1（其余）；confidence 起点 60。
   - 此审计**独立**于 mechanical lint 与 AAF 五问；可在 Per-Hat AAF Reviews 之前作为「Topology sketch 续」插入。

4. **Per-hat AAF review** (mandatory — one hat at a time, strict sequence per hat):
   - Declare: simulating hat `<id>` activation.
   - For the current hat, load **only** that hat's `instructions` (or `ralph hats show -H <path> <id>`). Do not use another hat's private instructions as evidence for this hat's visible context.
   - Run the **activation dry-run sequence** in order:
     1. **Trigger received** — what event triggered this hat? payload fields visible?
     2. **Visible context** — go through isolated prompt 栈 (`## HAT IDENTITY` / `## ORCHESTRATOR CONTEXT` / `instructions` / injected skills). What can the agent actually see?
     3. **Command plan** — which `ralph` commands in which order? OPAC 四阶段？
     4. **Payload construction** — for each emit topic, can every field be sourced from visible context?
     5. **Emit precheck** — `--policy-check` / `--triggered` ownership / policy-check feedback handling / single event budget / terminal ordering?
     6. **Handoff** — does any emitted field need to reach another hat? Does projection make it observable?
   - Fill AAF 五问表 + **Payload Audit 表** per emit topic (see `references/agent-native-model.md`).
   - For emitter hats, verify `instructions:` cites `ralph-tools-emit` Policy-Check feedback when it mentions payload construction, required fields, field shape, `ralph emit`, or `ralph wave emit`.
   - Compare to `instructions:` → candidate findings.
   - Optional: read `preset-author-notes.md` for that hat only after your table is drafted.

5. **Payload Audit table** (mandatory — aggregate it under the Per-Hat AAF Reviews section, one row per material emit field):
   - Columns: topic | field | value source | visibility evidence | identity check | semantic downstream use | schema metadata | policy-check repair surface | verdict | repair surface.
   - Cover every emit topic that drives a downstream hat decision or carries runtime identity.
   - For each required handoff / identity / verdict / count / path / reason field, inspect `event_policy.schemas.<topic>.field_docs.<field>`:
     - `meaning` must describe the field in agent-facing terms.
     - `source` must match a visible value source from the Payload Audit row.
     - `fill_rule` must tell the agent how to repair the field without inventing business facts.
   - Inspect `examples[]`: examples may show shape, but must not encode fake business conclusions that an agent could copy as facts.

6. **Handoff Audit table:** for each edge A→B: A.Q4 emit fields | projection action | downstream Q2 Observe | verdict | finding id.
   - Closed handoff = upstream emit field is in projection → downstream Q2 Observe command sees it.
   - Open handoff = P0 unless runtime evidence proves otherwise.

7. **Mechanical lint** — run commands in `references/commands.md`. Map JSON `id` values (`lint.preset.*`) via `references/finding-rubric.md`. **Continue AAF and payload audit even if lint fails**; note failure in Executive Summary.
   - If `preset.instructions_emit_feedback_skill_reference_missing` appears, treat it as a real adoption gap unless the hat does not construct payloads. The repair surface is the relevant hat `instructions:` plus, if needed, `event_policy.schemas.<topic>.field_docs`.

8. **Confidence calibration** (`references/finding-rubric.md`):
   - Lint Error → 95; Warn → 85
   - Soft AAF / payload audit: start ≤ 50; verify before ≥ 60
   - P0 with confidence < 60 cannot be reported as P0 until verified

9. **Write report** — all eight sections below.

## Report Path

- Default: `.ralph/reviews/<preset-basename>-<YYYY-MM-DD>.md` (gitignored)
- Optional PR copy: `<preset-dir>/preset-review-report.md`

## Report Structure (fixed)

1. **Executive Summary** — mode, hat count, P0/P1/P2 counts (confidence ≥ 60 only), lint pass/fail, **payload audit pass/fail**, policy-check feedback adoption pass/fail
2. **Findings Table** — sort P0 → P1 → P2; columns: id, severity, confidence, category, aaf_question, hat, problem one-liner
3. **Topology** — event flow
4. **Per-Hat AAF Reviews** — full 五问表 per hat + deltas vs instructions + mandatory Payload Audit Table rows (topic / field / source / visibility / identity / downstream / schema metadata / policy-check repair / verdict / fix)
5. **Handoff Audit Table** — closed / open per edge
6. **Mechanical Lint Results** — commands + output excerpt
7. **Remediation Plan** — ordered by **runtime unblock order**, not by file or discovery order; each item names repair surface (instructions / publishes / payload schema / state_projection / event_policy / author notes / fixture)
8. **Unverified Suspicions** (optional) — confidence < 60 after 2 rounds; does not drive edits; **must include the repair surface they would target once verified**

## Finding Schema (each row)

| Field | Required |
|---|---|
| `id` | e.g. F-001 |
| `severity` | P0 / P1 / P2 |
| `confidence` | 0–100 |
| `category` | feasibility / visibility / handoff / state / opac / topology / **payload-content** / policy-feedback / lint / style |
| `aaf_question` | Q1–Q5 for feasibility / payload-content findings |
| `hat` | hat id or `A→B` |
| `location` | YAML path or finding_id |
| `evidence` | command output / test name / schema trace; tag **hat-X view** vs **topology view** |
| `problem` | one sentence, agent-native |
| `fix` | actionable edit naming the repair surface |

**Gate:** A P0/P1 main-table finding without a concrete repair surface (field name + source + fix target) is rejected from the main table and demoted to `Unverified Suspicions`.

Example rows:

```markdown
| F-003 | P0 | 92 | feasibility | Q2 | executor | hats.executor.instructions | Q2 requires plan path with no Observe command | Add `ralph tools task list`; remove events.jsonl |

| F-014 | P0 | 88 | payload-content | Q4 | reviewer | hats.reviewer.publishes[work.done].payload.secret_handoff_token | Field is referenced downstream but never emitted / projected — hat-X view shows no observable source | Add `secret_handoff_token` to worker's emit payload + state_projection action; or remove downstream reference |

| F-021 | P1 | 78 | payload-content | Q4/Q5 | coordinator | hats.coordinator.publishes[work.start].payload.task_id | Live task_id required; no live observation path cited in instructions | Add `ralph tools task list` reference; cite ralph-tools-tasks red box |

| F-027 | P1 | 80 | policy-feedback | Q3/Q4 | reviewer | event_policy.schemas.review.synthesized.field_docs.must_fix_now_count | Required count field has no field_docs, so policy-check can reject but cannot tell the agent how to repair safely | Add meaning/source/fill_rule matching the Payload Audit row; keep instructions as a skill citation |
```

## P0 / P1 Quick Map

See `references/finding-rubric.md` for `finding_id` defaults and the new **Payload Audit → Severity** table. **Payload-content and invisible-input findings outrank style.**

## Guardrails (review skill itself)

- Never pass/fail based on "agent read the whole preset."
- Isolated P0 evidence must be **hat-visible** unless proving runtime injection.
- Reject user request for chat-only review — write the report file.
- Reject "handoff unclear" / "payload looks weak" findings that don't name field + source + fix — rewrite with evidence or move to `Unverified Suspicions`.

## Optional Verification

- `ralph hats show -H <path> <hat_id>` — hat config snapshot (not full prompt)
- `ralph emit --schema <topic>` — payload field SSOT (shape only)
- `ralph emit --policy-check --triggered <hat-id> <topic> '<payload>' -H <path>` — envelope `triggered` 须在 `hats[]`（与 payload schema 分开校验）

## Pre-merge Upgrade (not default)

```bash
./scripts/run-tests.sh
cargo nextest run -p ralph-core --test scenarios
```

## Read These References When Needed

- AAF model + payload audit: `references/agent-native-model.md`
- Commands: `references/commands.md`
- Severity / confidence / finding_id (including payload-content): `references/finding-rubric.md`
- Author checklist + Payload Contract template: `references/author-checklist.md`
- Topology context: `references/patterns.md`
