# HTML Deliverables — Entry Binding Inventory

Single maintained source: `skills/ralph-html/` (plan
`docs/plans/2026-10-29-1053-feat-html-deliverables-plan.html`; vendored
Effective HTML provenance in `skills/ralph-html/VENDOR.md`).

Every preset entry and operator skill that ships a human-readable final
document binds the guidance **explicitly** — agents never search for
same-named skills. Loop hats load the namespaced built-ins below via
`ralph tools skill load <name>`; operator skills read the same content
from the installed `skills/ralph-html/` directory. The two distributions
are byte-identical to the vendored source (loop side rewrites vendored
sibling/reference links to the explicit load names below at
registration time; see `crates/ralph-core/src/html_skills.rs`).

## Binding profiles

**Report profile** (final reports, findings docs, root-cause notes,
blocked artifacts):

1. `ralph-html` — Ralph entry: scope, offline contract, checks, failure
   handling. Always loaded first.
2. `ralph-html-artifact` — report authoring workflow.
3. `ralph-html-design` — visual hierarchy, typography, accessibility.
4. `ralph-html-ref-documents-and-presentations` — report/document
   patterns.

Optional, load only when the condition holds:

- `ralph-html-ref-charts-and-data` — the document carries quantitative
  comparison, metrics tables, or data stories.
- `ralph-html-ref-diagrams` — the document must explain topology,
  sequence, state, or causality.
- `ralph-html-diagram` — a rendered relationship/state diagram is
  material to understanding.
- `ralph-html-big-pics` — an intuitive illustration genuinely helps;
  never replaces required evidence.
- `ralph-html-ref-interfaces`, `ralph-html-wireframe`,
  `ralph-html-prototype` — only when the deliverable itself explores an
  interface or interaction flow.

**Plan profile** (fix plans, repair plans, author notes, discovery
reading views):

1. `ralph-html` — always first.
2. `ralph-html-plan` — preserves decisions, dependencies, order,
   ownership, and completion conditions.
3. `ralph-html-design` — same as report profile.

Optional: `ralph-html-ref-diagrams` / `ralph-html-diagram` when
dependencies or topology need a diagram.

**Shared delivery contract** (every entry): write the HTML file first —
self-contained, offline-openable, all content in semantic HTML, HTML
characters escaped — then emit the path field, then print
`DELIVERABLE_PATH: <path>` in the visible reply. A missing,
inconsistent, or unwritten file keeps the original failure/blocked
path; guidance load or content-check failure is reported through the
original stop flow, never claimed as done.

## presets/en/ (builtin)

| Entry | Terminal event | Path field | Deliverable | Producer hat | Binding |
|---|---|---|---|---|---|
| `autoresearch` | `LOOP_COMPLETE` | `report_path` | `autoresearch.html` | `strategist` (primary) / `evaluator` (stuck fallback) — emitter writes the final HTML from the session document `autoresearch.md`, which stays a working Markdown file | report |
| `ce-executor-pipeline` | `report.done` → `LOOP_COMPLETE` | `report_path` | `.ralph/review/<plan>/report.html` (also `precheck-exhausted-<ts>/report.html`) | `reporter` | report |
| `debug` | `DEBUG_COMPLETE` | `report_path` | prompt-specified path, default `.eval-sandbox/debug/counter.html` | `fixer` writes the note; `investigator` emits | report |
| `hatless-baseline` | — | — | **Not applicable** — single-hat baseline with no file deliverable and no path field | — | — |
| `implementation-review` | `LOOP_COMPLETE` | `artifact_path` | success/`residual_only`/`fixes_required`: `.ralph/review/<plan>/fix-plan.html`; blocked: `scope-blocked.html` / `dispatch-blocked.html` / `review-blocked.html` / `wave-blocked.html` | `fix-planner` (fix plan), `scope-preparer`, `review-dispatcher`, `review-synthesizer`, `finalizer` | fix plan → plan profile; blocked artifacts → report profile |
| `merge-batch` | `merge.batch.complete` | `report_path` | `.ralph/merge/REPORT.html` (optional stamped copy `.ralph/merge/reports/<ts>.html`) | `reporter` | report |
| `merge-loop` | `merge.handled` | — | **Not applicable** — text-path completion promise; terminal event carries no path field | — | — |
| `parallel-forge` | `forge.report.done` → `LOOP_COMPLETE` | `report_path` | `docs/reports/<YYYY-MM-DD>-<task>-manager-report.html` | `reporter` | report; content checklist `presets/templates/parallel-forge/manager-report-checklist.md` |
| `post-merge-converge` | `postmerge.complete` | `report_path` | `.ralph/post-merge/REPORT.html` (optional stamped copy `.ralph/post-merge/reports/<ts>.html`) | `reporter` | report |
| `red-team-attack` | `redteam.complete` | `report_path` / `plan_path` / `questions_path` | `.ralph/red-team/REPORT.html`, `PLAN.html`, `QUESTIONS.html` | `reporter` (REPORT + QUESTIONS); `impact-boundary` (PLAN, verified by reporter) | REPORT/QUESTIONS → report; PLAN → plan |

## presets/zh/ (mirror variants)

| Entry | Terminal event | Path field | Deliverable | Producer hat | Binding |
|---|---|---|---|---|---|
| `autoresearch-zh` | `LOOP_COMPLETE` | `report_path` | `autoresearch.html` | `strategist` / `evaluator` | report |
| `debug-zh` | `DEBUG_COMPLETE` | `report_path` | default `.eval-sandbox/debug/counter.html` | `fixer` writes; `investigator` emits | report |
| `hatless-baseline-zh` | — | — | **Not applicable** — same as en baseline | — | — |
| `wave-review-zh` | `review.complete` | — | `.ralph/agent/review-report.html` | `synthesizer` | report |

## presets/extras/ (reference variants)

| Entry | Terminal event | Path field | Deliverable | Producer hat | Binding |
|---|---|---|---|---|---|
| `wave-review` | `review.complete` | — | `.ralph/agent/review-report.html` | `synthesizer` | report |
| `ralph.reviewer` | `review.done` | — | `.ralph/REVIEW-REPORT.html` | `reviewer` | report |
| `harness-demo` | — | — | **Not applicable** — hooks scratchpad demo; no file deliverable | — | — |

## presets/minimal/ (starter variants)

| Entry | Terminal event | Path field | Deliverable | Producer hat | Binding |
|---|---|---|---|---|---|
| `builder`, `claude`, `codex`, `gemini`, `opencode`, `code-assist`, `smoke`, `test` | — | — | **Not applicable** — single-hat minimal presets without a file deliverable | — | — |
| `preset-evaluator` | `evaluation.complete` | — | `tools/preset-evaluation-findings.html` | `evaluator` | report |

## presets/monitor/, presets/schemas/, presets/scenarios/

- `presets/monitor/` — **Not applicable / no change**: descriptors only
  reference runtime artifacts (`.ralph/agent/summary.md`,
  `.ralph/agent/tasks.jsonl`, `.ralph/plans/*.verified.md`) — none name a
  terminal deliverable.
- `presets/schemas/` — field_docs/fill_rule/examples updated wherever
  they name a `.md` deliverable path; field names and required-field
  sets unchanged.
- `presets/scenarios/` — payload path strings for terminal deliverables
  use `.html`; internal evidence paths keep `.md`.

## Final-deliverable templates (presets/templates/)

Templates that feed final deliverables are migrated to self-contained
HTML — same sections and stable metadata (frontmatter fields become
`<meta name="ralph:...">` elements), still materialized through
`ralph preset materialize-artifacts`:

| Preset | Template | Feeds |
|---|---|---|
| `parallel-forge` | `presets/templates/parallel-forge/manager-report.template.html` | `docs/reports/<date>-<task>-manager-report.html` |
| `red-team-attack` | `presets/templates/red-team-attack/report.template.html` | `.ralph/red-team/REPORT.html` |
| `red-team-attack` | `presets/templates/red-team-attack/plan.template.html` | `.ralph/red-team/PLAN.html` |

Internal handoff/evidence templates (`development-plan.template.md`,
`unit*.template.*`, `wave-*.template.md`, `correction.template.md`,
`merge-conflict.template.md`, `fail-confidence-rubric.template.md`,
`review-findings-contract.md`, `settlement-evidence.template.md`,
`experiment.template.yml`, `finding.template.yml`) stay Markdown/YAML —
they are machine-consumed mid-chain artifacts, not final deliverables.

## Operator skills

| Skill | Deliverable | Binding | Machine contract preserved |
|---|---|---|---|
| `ralph-run-diagnosis` | `docs/report/<YYYY-MM-DD>-<preset>-<loop_id>-diagnosis.html` | report (+ diagram / big-pics only when warranted) | frontmatter-equivalent fields live in stable HTML metadata (`<meta name="ralph:...">`) + semantic blocks |
| `ralph-preset-author` | `<preset>-preset-author-notes.html` | plan | AAF, Payload Contract, Gate Scope, Key-stage fields readable from stable HTML blocks for `ralph-preset-review` |
| `ralph-task-discovery` | validated YAML brief (unchanged machine handoff) **plus** `<brief>.html` reading view | plan | YAML brief remains the only handoff artifact; the HTML view mirrors it, pending/blocked shown honestly |
| `ralph-preset-review` | `preset-review-report.html` | report | fixed 8-section structure + `agent_skill_audit` fields readable from stable HTML blocks for the next review run |

## Files that stay Markdown

Mid-chain handoff/evidence artifacts are not final deliverables and do
not migrate: `.ralph/review/<plan>/{scope-analysis,review-context,
synthesized-review,dimensions/*}.md`, `.ralph/forge/<plan>/**` (except
the manager report), `.ralph/merge/{review,integration,stabilize-log,
report-input}.md`, `.ralph/post-merge/{01-15}-*.md` and
`findings/PMI-*.md`, `.ralph/red-team/{01-10}-*.md` / `experiments/` /
`findings/` / `failures/`, `autoresearch.md` working document,
`.ralph/agent/{summary,decisions}.md`, plans under `docs/plans/` unless
an entry's own contract says otherwise. Historical Markdown reports are
not back-migrated.

## Visibility

The built-in guidance registers under the controlled `ralph-html*` names
with a default hat list covering the deliverable writers above
(`reporter`, `finalizer`, `fix-planner`, `scope-preparer`,
`review-dispatcher`, `review-synthesizer`, `evaluator`, `strategist`,
`fixer`, `reviewer`, `impact-boundary`, plus generic file-preset role
names). A filesystem skill claiming a `ralph-html*` name is refused —
the embedded vendored source always wins. A file preset whose
deliverable hat uses another name grants access via
`skills.overrides."<name>".hats` in `ralph.yml`.
