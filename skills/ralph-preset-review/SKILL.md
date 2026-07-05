---
name: ralph-preset-review
description: Review Ralph preset YAML with agent-native AAF audit, mechanical lint, and structured preset-review-report.md. Use after drafting a preset or when validating builtin/local presets for per-hat feasibility, handoff closure, OPAC discipline, and P0/P1 issues. Produces findings with severity and confidence scores.
---

# Ralph Preset Review

Use this skill to **review** Ralph presets with **Agent 视角可行性（AAF）** — independent per-hat activation simulation plus mechanical lint.

**Boundary:** Does not replace Rust `preset_lint` rules. Does not run full `./scripts/run-tests.sh` by default. For drafting, use `ralph-preset-author`.

**Deliverable:** Every review MUST write **`preset-review-report.md`** — not chat-only summaries.

## Use This Skill For

- Reviewing builtin or local presets before merge
- Finding P0/P1 issues: invisible inputs, broken handoffs, illegal commands, ledger reads
- Running `ralph preset check --strict` and preset_lint nextest subsets
- Producing actionable remediation from AAF gaps

## Core Assumptions

- **Do not trust** `preset-author-notes.md` — redo AAF independently; use notes only to flag author/review mismatches.
- **Simulate one hat activation at a time** — declare explicitly: "I am simulating hat X's activation."
- **confidence ≥ 60** required for findings in the main table; below 60 → discard, re-investigate (max 2 rounds), or `Unverified Suspicions`.

## Workflow

1. Read **topology-only** fields from preset YAML: `event_loop`, `hats.*.triggers`, `hats.*.publishes`, `state_projection`, `event_policy` — **not** other hats' `instructions` yet.

2. Record `execution_mode`, hat count, preset path (`builtin:` vs file).

3. **Topology sketch** — event flow diagram (not prompt flow).

4. **Per-hat AAF review** (mandatory — one hat at a time):
   - Declare: simulating hat `<id>` activation.
   - Load **only** that hat's `instructions` (or `ralph hats show -H <path> <id>`). **Do not read other hats' instructions** until all per-hat AAF tables are done.
   - Fill AAF 五问表 (see `references/agent-native-model.md`).
   - Compare to `instructions:` → candidate findings.
   - Optional: read `preset-author-notes.md` for that hat only after your table is drafted.

5. **Handoff Audit table:** for each edge A→B: A.Q4 fields | B.Q2 Observe | projection | finding id.

6. **Mechanical lint** — run commands in `references/commands.md`. Map JSON `id` values (`lint.preset.*`) via `references/finding-rubric.md`. Continue AAF even if lint fails; note failure in Executive Summary.

7. **Confidence calibration** (`references/finding-rubric.md`):
   - Lint Error → 95; Warn → 85
   - Soft AAF: start ≤ 50; verify before ≥ 60
   - P0 with confidence < 60 cannot be reported as P0 until verified

8. **Write report** — all eight sections below.

## Report Path

- Default: `.ralph/reviews/<preset-basename>-<YYYY-MM-DD>.md` (gitignored)
- Optional PR copy: `<preset-dir>/preset-review-report.md`

## Report Structure (fixed)

1. **Executive Summary** — mode, hat count, P0/P1/P2 counts (confidence ≥ 60 only), lint pass/fail
2. **Findings Table** — sort P0 → P1 → P2; columns: id, severity, confidence, category, aaf_question, hat, problem one-liner
3. **Topology** — event flow
4. **Per-Hat AAF Reviews** — full五问表 per hat + deltas vs instructions
5. **Handoff Audit Table**
6. **Mechanical Lint Results** — commands + output excerpt
7. **Remediation Plan** — P0 first, with fix summary
8. **Unverified Suspicions** (optional) — confidence < 60 after 2 rounds; does not drive edits

## Finding Schema (each row)

| Field | Required |
|---|---|
| `id` | e.g. F-001 |
| `severity` | P0 / P1 / P2 |
| `confidence` | 0–100 |
| `category` | feasibility / visibility / handoff / state / opac / topology / lint / style |
| `aaf_question` | Q1–Q5 for feasibility findings |
| `hat` | hat id or `A→B` |
| `location` | YAML path or finding_id |
| `evidence` | command output / test name / schema trace; tag **hat-X view** vs **topology view** |
| `problem` | one sentence, agent-native |
| `fix` | actionable edit |

Example row:

```markdown
| F-003 | P0 | 92 | feasibility | Q2 | executor | hats.executor.instructions | Q2 requires plan path with no Observe command | Add `ralph tools task list`; remove events.jsonl |
```

## P0 / P1 Quick Map

See `references/finding-rubric.md` for `finding_id` defaults. AAF gaps take priority over style.

## Guardrails (review skill itself)

- Never pass/fail based on "agent read the whole preset."
- Isolated P0 evidence must be **hat-visible** unless proving runtime injection.
- Reject user request for chat-only review — write the report file.

## Optional Verification

- `ralph hats show -H <path> <hat_id>` — hat config snapshot (not full prompt)
- `ralph emit --schema <topic>` — payload field SSOT

## Pre-merge Upgrade (not default)

```bash
./scripts/run-tests.sh
cargo nextest run -p ralph-core --test scenarios
```

## Read These References When Needed

- AAF model: `references/agent-native-model.md`
- Commands: `references/commands.md`
- Severity / confidence / finding_id: `references/finding-rubric.md`
- Topology context: `references/patterns.md`
