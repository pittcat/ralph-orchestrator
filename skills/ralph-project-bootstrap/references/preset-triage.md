# Preset Triage Contract (Phase 0)

This document is the implementation reference for Phase 0 of
`ralph-project-bootstrap`: turning an operator's task, file, or bare
goal into a confirmed preset choice. End agents do not see this file;
the skill owner does.

Phase 0 is **read-only**. It never writes the target project, never
runs `ralph run` / `ralph emit`, and never mutates `.ralph/`. Its only
output is a confirmed `--preset` token plus whatever launch inputs the
chosen preset's contract requires.

## Inventory collection

`scripts/preset_inventory.py` is the single deterministic collector:

```bash
python skills/ralph-project-bootstrap/scripts/preset_inventory.py \
  --cwd <root> \
  [--preset <file.yml> ...] [--presets-dir <dir> ...] \
  [--binary ralph] [--no-builtin] [--format json|text]
```

Facts are typed; ranking is agent judgement. The collector:

- drives `ralph preset builtin list --format json` then
  `ralph preset builtin show <id> --format yaml` per id — the same
  introspection contract the pipeline resolver consumes. The `id`
  field is the only match key; `source` is provenance data.
- reads file presets anchored on `--cwd`; every path token passes
  `_paths.is_safe_relative` before any filesystem read.
- never raises: missing binary → `binary_missing`, bad envelope →
  `builtin_list_unparseable`, per-show failure → `builtin_show_failed`,
  unreadable file → `input_missing_preset_file`, malformed YAML →
  `preset_yaml_invalid`. Issues are data; partial inventory is still a
  valid result.

## Candidate schema

Each `presets[]` row in the JSON envelope:

| field | meaning |
| --- | --- |
| `preset_id` / `source` / `origin` | builtin id or file stem; `builtin:<id>` or file path; `builtin` / `file` |
| `description` / `description_source` | envelope description for builtins; lifted header-comment block for file presets (`header_comment`), empty when absent |
| `hat_count` / `hat_ids` / `hat_summaries` | topology signal; `hat_summaries` joins per-hat `description` fields (the fallback semantic material for presets with no top-level description) |
| `backend` | `cli.backend`; `""` when the preset omits `cli` |
| `execution_mode` / `supervisor_enabled` | `event_loop.execution_mode`; `event_loop.supervisor.enabled` |
| `inline_prompt_present` / `prompt_excerpt` / `input_contract` | whether `event_loop.prompt` is non-empty; a ≤400-char excerpt exposing the operator-facing input contract; derived `self_prompted` / `needs_operator_input` |
| `completion_promise` / `required_events` | the terminal contract the loop must reach |
| `max_iterations` / `max_runtime_seconds` / `tasks_enabled` | weight / cost signal |
| `contract_complete` | `false` when `cli.backend` or a positive budget field is missing — the pipeline resolver (`preset_resolution` stage) will reject such presets with `preset_runtime_contract_missing`; recommend only with that risk flagged |
| `shadows_builtin` | file preset stem collides with a live builtin id — prefer the `builtin:` source; it is the vetted copy |
| `public` | builtin list-envelope flag; `null` for file presets |

## Recommendation rubric

Score candidates on **intent fit**, not keyword count. The operator's
verbs/objects are matched against `description` + `prompt_excerpt` +
`hat_summaries`; weight by input contract, backend readiness, and
topology weight (`hat_count`, `supervisor_enabled`, worktree
expectations).

The archetype table below is a **prior, not a lookup**: builtin
membership changes over time, so every row must be verified against the
live inventory before it is cited. An archetype whose preset id is
absent from the inventory does not exist for this run.

| Operator intent smells like | Candidate shape to look for | Typical live fit |
| --- | --- | --- |
| "implement / execute this plan", plan file supplied | plan-driven executor chain (plan-reviewer → executor → review → fix → report) | `builtin:ce-executor-pipeline` (serial, primary path); `builtin:ce-executor-pipeline-loop` when the task wants convergence-gated re-review rounds |
| "parallelise / split this plan across worktrees" | spec-first supervisor + DAG scheduler, worktree Units | `builtin:parallel-forge` |
| "merge my worktrees / branches" | git-first batch merge with verify-fix self-loop and merge report | `builtin:merge-batch` (needs target branch + ordered branch list in the prompt) |
| "stabilise / audit after merging" | post-merge system audit → findings → fix/regression → clean-env review | `builtin:post-merge-converge` |
| "review what was just implemented" (no fixes) | read-only multi-dimension review emitting a fix-plan artifact | `builtin:implementation-review` — note `contract_complete:false` on some binaries (no `cli.backend`); verify `preset check --strict` early |
| "bug / broken / root-cause this" | hypothesis-driven debug chain with adversarial fix verification | `builtin:debug` |
| "attack / pentest / adversarial proof" | experiment-driven red-team with dedicated worktree and evidence gating | `builtin:red-team-attack` |
| "try ideas, keep what works" (optimisation / research loop) | autonomous experiment loop | `builtin:autoresearch` |

Project-state signals adjust the prior: live side worktrees tilt toward
merge/parallel candidates; a supplied plan file tilts toward
plan-driven executors; a `.ralph/` directory with loop state **ends**
triage (hand off to `ralph-loop` / `ralph-run-diagnosis` — this is not
a bootstrap problem).

## Question protocol

Follows the same discipline as `ralph-task-discovery`:

- **One question at a time.** The preset-selection question is asked
  alone, with the recommended candidate as the leading option and the
  fit reasoning attached. A multi-question dump is a protocol violation.
- **Recommendation attached, never pre-applied.** The recommended
  option is presented first with its reason; the operator's explicit
  selection is the only confirmation. Silence, "looks good", or a
  rephrased echo of the recommendation all count; an unanswered
  question does not.
- **Contract questions follow selection.** Once a preset is confirmed,
  resolve its remaining inputs (target branch, ordered branch list,
  plan path, prompt fields the `prompt_excerpt` names) one question at
  a time, each with a recommendation where the environment supplies
  one (e.g. `git worktree list` output as the default branch order).
- **Facts never become questions.** Anything discoverable — stack,
  test entry points, worktree layout, plan-file existence, backend
  presence on `PATH` — is probed, not asked.

## Stop conditions

Stop triage and report instead of forcing a selection when:

- `.ralph/` loop state exists → hand off to `ralph-loop` /
  `ralph-run-diagnosis`.
- `root_ambiguous` from the audit helpers → reconcile the project root
  first (same blocker the pipeline would emit).
- The operator's intent is still ambiguous after **one** clarifying
  question per unclear dimension — do not interrogate; present the top
  candidates with their differentiating conditions and let the
  selection question carry the ambiguity.
- No candidate fits → route to `ralph-task-discovery` →
  `ralph-preset-author`; forcing the nearest preset is a failure mode,
  not a fallback.
- The only matching candidates carry `contract_complete: false` →
  surface the resolution risk explicitly in the selection question
  (the pipeline will emit `preset_runtime_contract_missing` if it
  applies) rather than hiding it.

## Handoff to Phase 1

Enter the Unified Entry Point only after explicit confirmation, with:

- `--preset <source>` — the confirmed `builtin:<id>` or file path.
- `--plan <file>` — when the confirmed preset is plan-driven and the
  operator supplied (or confirmed) a plan file.
- `--prompt-file <file>` — when the operator supplies an
  operator-owned prompt; this becomes the authoritative prompt source
  and suppresses the `--plan` prompt source per the pipeline's prompt
  ownership contract.
- `--binary <path>` — when introspection ran against a non-default
  binary.

A file the operator supplied is candidate input, not commitment: it is
passed through only when it matches the confirmed preset's contract
(plan-shaped file → `--plan`; prompt-shaped → `--prompt-file`).
