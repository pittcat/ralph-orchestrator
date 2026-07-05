---
name: ralph-preset-author
description: Draft and validate Ralph preset YAML with per-hat Agent-Feasibility (AAF) checks. Use when creating or editing presets/en/, presets/schemas/, builtin presets, or .ralph/hats/ workflow files — including topology, state_projection, hat instructions, and handoff design. Not for user-only hat collections (use ralph-hats).
---

# Ralph Preset Author

Use this skill to design and draft Ralph **presets** (builtin or local) with **Agent 视角可行性（AAF）** discipline.

**Boundary:** `ralph-hats` manages user `.ralph/hats/` collections only. This skill covers the **full preset chain** including `presets/en/` and `presets/schemas/`. For review after drafting, hand off to `ralph-preset-review`.

## Use This Skill For

- Creating or editing builtin presets (`presets/en/`, `presets/schemas/`)
- Creating local presets from templates (`ralph preset new`)
- Designing event topology, `state_projection`, and handoff fields
- Writing per-hat `instructions:` in isolated mode (one agent per activation)
- Producing `preset-author-notes.md` (AAF 五问表 per hat) before review

## Core Assumptions

- Each hat activation in `execution_mode: isolated` sees **only its own** `instructions` plus runtime injection — not other hats' instructions.
- State passes via **emit → state_projection → task/progress → Observe**, not by reading internal ledgers.
- Command syntax lives in `crates/ralph-core/data/ralph-tools*.md` — **cite skill sections, do not copy parameter tables**.

## Workflow

1. **Classify target:** local (`.ralph/hats/*.yml`) vs builtin (`presets/en/` + `presets/schemas/`). Note `execution_mode` and hat count (4+ → `isolated` mandatory).

2. **Topology phase (author brain):**
   - Read schema SSOT for builtin presets.
   - Sketch event flow (topics, not prompts).
   - Align each handoff: upstream Q4 fields ↔ downstream Q2 Observe path.
   - See `references/patterns.md` for examples only at this stage.

3. **Drafting phase (single-hat agent brain):**
   - For each hat, **pretend other hats do not exist**.
   - Write only that hat's `instructions:`.
   - Fill one AAF 五问表 per hat (template in `references/author-checklist.md`).

4. **Assemble `preset-author-notes.md`** next to the preset YAML (all AAF tables).

5. **Pre-review gate (MUST — do not skip):**
   - Every hat has a complete AAF table in `preset-author-notes.md`.
   - Hat count in notes **equals** hat count in YAML.
   - No empty cells; no「待定」「同上」「上游会处理」.
   - Ask: "If I only received this hat's instructions + injection, can I complete Q1?"
   - For builtin edits, list the 7-point sync checklist (do not auto-apply).
   - **If any check fails: STOP.** Do not recommend review or deliver YAML as complete.

6. **Hand off to `ralph-preset-review`** only after step 5 passes — does not replace `ralph preset check`.

## Guardrails

- **No whole-file agent perspective** in `instructions:` — no "the reviewer will…", no topology position.
- **No internal ledger reads** — no `.ralph/events.jsonl`, `.ralph/supervisor.db`, `.ralph/loops.json`.
- **Emitter hats:** require `--policy-check` before emit; cite `ralph-tools-opac` and `ralph-tools-emit` §5.
- **`--triggered`:** only use hat ids declared in preset `hats[]`; verify with `ralph emit --policy-check --triggered <hat> …` (see `references/commands.md`).
- **`task_id` / `task_key` / `step`:** cite `ralph-tools-tasks` red box; never hand-write `task_id`.
- **Single business event budget** per isolated activation; no business events before terminal emit.
- **Do not duplicate** `ralph-tools*.md` content into instructions.

## Output Expectations

- Updated preset YAML
- `preset-author-notes.md` with one complete AAF table per hat
- Explicit handoff message: preset path + notes path → invoke `ralph-preset-review`

## Read These References When Needed

- AAF model and prompt stack: `references/agent-native-model.md`
- Checklist and AAF template: `references/author-checklist.md`
- Validation commands: `references/commands.md`
- Topology patterns (topology phase only): `references/patterns.md`
