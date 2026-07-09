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
- Designing event schema metadata (`field_docs` / `examples`) that lets agents repair policy-check failures
- Writing per-hat `instructions:` in isolated mode (one agent per activation)
- Producing `preset-author-notes.md` (AAF 五问表 per hat) before review

## Core Assumptions

- Each hat activation in `execution_mode: isolated` sees **only its own** `instructions` plus runtime injection — not other hats' instructions.
- State passes via **emit → state_projection → task/progress → Observe**, not by reading internal ledgers.
- Command syntax lives in `crates/ralph-core/data/ralph-tools*.md` — **cite skill sections, do not copy parameter tables**.

## Workflow

1. **Classify target:** local (`.ralph/hats/*.yml`) vs builtin (`presets/en/` + `presets/schemas/`). Note `execution_mode` and hat count (4+ → `isolated` mandatory).

2. **Topology phase (author brain):**
   - **Default execution model = single-chain** (`ce-executor-pipeline` 同型:linear hat chain + executor-owned subagent work)。仅当显式证明单链无法表达时,才允许引入多角色 runtime orchestration（如 `ce-executor-supervisor` 风格）。
   - Read schema SSOT for builtin presets.
   - Sketch event flow (topics, not prompts).
   - Align each handoff: upstream Q4 fields ↔ downstream Q2 Observe path.
   - For every agent-authored emit topic, decide whether `event_policy.schemas.<topic>.field_docs` and `examples` are needed. Any required handoff, identity, verdict, count, file path, or reason field needs field-level metadata unless the field is already self-evident from injected skill docs.
   - See `references/patterns.md` for examples only at this stage.

3. **Drafting phase (single-hat agent brain):**
   - For each hat, **pretend other hats do not exist**.
   - Write only that hat's `instructions:`.
   - Fill one AAF 五问表 per hat (template in `references/author-checklist.md`).
   - For every emit topic, fill the **Payload Contract 表** (per-topic rows: field, value source, visibility evidence, identity check, downstream use).
   - For every field that appears in Payload Contract, add or verify schema metadata:
     - `field_docs.<field>.meaning`: what the field means to the emitting agent.
     - `field_docs.<field>.source`: where the emitting agent obtains the value.
     - `field_docs.<field>.fill_rule`: how to fill or repair the value after policy-check rejects it.
     - `examples[]`: topic-level example payloads only when they do not invent business facts.
   - In `instructions:`, cite `ralph-tools-emit` Policy-Check feedback instead of copying field tables. The prompt builder supplies the per-topic schema-aware publish section.

4. **Assemble `preset-author-notes.md`** next to the preset YAML (all AAF tables + Payload Contract tables).

5. **Pre-review gate (MUST — do not skip):**
   - Every hat has a complete AAF table **and** a complete Payload Contract table in `preset-author-notes.md`.
   - Hat count in notes **equals** hat count in YAML; per-emit-topic row count covers every material `publishes` entry.
   - No empty cells; no「待定」「同上」「上游会处理」「约定俗成」.
   - Every `task_id` / `task_key` / `step` row is marked `live required` with a concrete observation command.
   - Multi-trigger hats split Payload Contract by trigger, not collapsed into one row.
   - Required handoff / identity / decision fields have `field_docs` metadata or a documented reason why existing injected docs already explain the field.
   - Emitter instructions reference `ralph-tools-emit` Policy-Check feedback when they mention payload construction, `ralph emit`, `ralph wave emit`, required fields, or field shape.
   - Ask: "If I only received this hat's instructions + injection, can I complete Q1? Can I construct every Q4 field from visible sources?"
   - **Single-chain-first 5 问全 ✓**: 填 `references/author-checklist.md` 的「Hard questions — single-chain-first」段；任一 ✗ 必须改写或显式 justify。
   - For builtin edits, list the 7-point sync checklist (do not auto-apply).
   - **If any check fails: STOP.** Do not recommend review or deliver YAML as complete.

6. **Hand off to `ralph-preset-review`** only after step 5 passes — does not replace `ralph preset check`.

## Guardrails

- **No whole-file agent perspective** in `instructions:` — no "the reviewer will…", no topology position.
- **No internal ledger reads** — no `.ralph/events.jsonl`, `.ralph/supervisor.db`, `.ralph/loops.json`.
- **Emitter hats:** require `--policy-check` before emit; cite `ralph-tools-opac` and `ralph-tools-emit` §5.
- **Policy-check feedback:** emitter instructions must cite `ralph-tools-emit` Policy-Check feedback; schema metadata belongs in `event_policy.schemas`, not in prose instructions.
- **Schema metadata:** `field_docs` and `examples` are agent-facing repair guidance only. They must not be treated as validation authority; machine acceptance still comes from `required_fields`, `allowed_values`, `hat_allowed_values`, and element constraints.
- **`--triggered`:** only use hat ids declared in preset `hats[]`; verify with `ralph emit --policy-check --triggered <hat> …` (see `references/commands.md`).
- **`task_id` / `task_key` / `step`:** cite `ralph-tools-tasks` red box; never hand-write `task_id`.
- **Single business event budget** per isolated activation; no business events before terminal emit.
- **Do not duplicate** `ralph-tools*.md` content into instructions.

## Output Expectations

- Updated preset YAML
- `preset-author-notes.md` with one complete AAF table **+ one Payload Contract table** per hat
- Updated schema metadata for agent-authored emit topics (`field_docs` / safe `examples`) or an explicit no-op rationale
- Explicit handoff message: preset path + notes path → invoke `ralph-preset-review`

## Read These References When Needed

- AAF model and prompt stack: `references/agent-native-model.md`
- Checklist and AAF template: `references/author-checklist.md`
- Validation commands: `references/commands.md`
- Topology patterns (topology phase only): `references/patterns.md`
