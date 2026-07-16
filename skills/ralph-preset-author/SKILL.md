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
- **Declaring schema-backed `trigger_context`** (`summary_fields` + `routing_hints` + `known_fields`) for trigger-consuming hats — collapses duplicated payload if/else in `instructions` into a single injected `## TRIGGER CONTEXT` block
- Writing per-hat `instructions:` in isolated mode (one agent per activation)
- Producing `preset-author-notes.md` (AAF 五问表 per hat) before review
- **Artifact-First Handoff authoring** (R1–R7, per `docs/plans/2026-07-16-003-feat-preset-artifact-first-handoffs-plan.md`): declaring artifact 落盘点、消费方、生命周期责任，并在 Payload Contract 与 AAF 五问表中固化每一份重要信息的落盘判定

## Core Assumptions

- Each hat activation in `execution_mode: isolated` sees **only its own** `instructions` plus runtime injection — not other hats' instructions.
- State passes via **emit → state_projection → task/progress → Observe**, not by reading internal ledgers.
- **Artifact-First Handoff assumption (2026-07-16-003 plan):**
  - **文件是重要信息的事实源**：完整结果、长内容、跨 hat 摘要、关键决策依据、验证证据、高成本重建信息默认必须落盘。
  - **事件承担控制面，文件承担数据面**：event payload 只携带短状态、摘要、路径、必要身份与路由字段。
  - **默认强制但允许有理由的例外**：只有短暂 + 短小 + 无需恢复的信息可以不落盘；例外必须在 Payload Contract 同行标注理由（恢复 / 审计 / 下游依赖），不能仅按字符数判断。
  - **落盘位置限定在当前 `.ralph/`**：业务 artifact 落在当前 workspace / worktree 的 `.ralph/<plan>/<unit>/<file>.md` 等业务子目录；不得把 `.ralph/events.jsonl`、`.ralph/loops.json`、`.ralph/supervisor.db` 等 runtime internal ledger 当作业务 artifact 接口。
- Command syntax lives in `crates/ralph-core/data/ralph-tools*.md` — **cite skill sections, do not copy parameter tables**.

## Workflow

1. **Classify target:** local (`.ralph/hats/*.yml`) vs builtin (`presets/en/` + `presets/schemas/`). Note `execution_mode` and hat count (4+ → `isolated` mandatory).

2. **Topology phase (author brain):**
   - **Default execution model = single-chain** (`ce-executor-pipeline` 同型:linear hat chain + executor-owned subagent work)。仅当显式证明单链无法表达时,才允许引入多角色 runtime orchestration（如 `ce-executor-supervisor` 风格）。
   - Read schema SSOT for builtin presets.
   - Sketch event flow (topics, not prompts).
   - Align each handoff: upstream Q4 fields ↔ downstream Q2 Observe path.
   - **Artifact-First topic 判别**（每条 emit topic）：每条 emit 字段先判定是否属于「完整结果 / 长内容 / 跨 hat 摘要 / 关键决策依据 / 验证证据 / 高成本重建」。若是,完整内容必须由执行该 hat 或其 sub-agent 写到当前 `.ralph/` 下的业务 artifact;event payload 只携带路径 / 短摘要 / 必要身份 / 路由字段。判定三标准(恢复价值 / 审计价值 / 下游依赖)与术语定义见 `references/agent-native-model.md`「Artifact-First Handoff 模型」段。
   - For every agent-authored emit topic, decide whether `event_policy.schemas.<topic>.field_docs` and `examples` are needed. Any required handoff, identity, verdict, count, file path, or reason field needs field-level metadata unless the field is already self-evident from injected skill docs. **`field_docs.<path_field>.meaning` 必须明确「该路径是 artifact 落盘点」**,`source` 必须指向当前 hat 可见输入,`fill_rule` 不能诱导 agent 伪造路径,`examples[]` 用结构占位(`.ralph/<plan>/<unit>/<file>.md`)而非固定业务文件名(详见 `references/finding-rubric.md` 「Artifact-First Handoff `field_docs` 审核点」)。
   - See `references/patterns.md` for examples only at this stage.

3. **Drafting phase (single-hat agent brain):**
   - For each hat, **pretend other hats do not exist**.
   - Write only that hat's `instructions:`.
   - Fill one AAF 五问表 per hat (template in `references/author-checklist.md`).
   - For every emit topic, fill the **Payload Contract 表** (per-topic rows: field, value source, visibility evidence, identity check, downstream use, **artifact 落盘**).
     - `artifact 落盘` 列必须填「必填 / 可选 / 不需要」与路径格式约定;不落盘例外必须写「不落盘 + 理由」(恢复 / 审计 / 下游依赖三标准)。
     - 每条写入型 hat(executor / fixer / sub-agent 拥有方)必须显式声明本 activation 会写的 artifact 路径集合。
     - 每条消费型 hat 必须显式声明从哪个可见路径读取 artifact;不得依赖 prompt 中的长文本。
   - For every field that appears in Payload Contract, add or verify schema metadata:
     - `field_docs.<field>.meaning`: what the field means to the emitting agent.
     - `field_docs.<field>.source`: where the emitting agent obtains the value.
     - `field_docs.<field>.fill_rule`: how to fill or repair the value after policy-check rejects it.
     - `examples[]`: topic-level example payloads only when they do not invent business facts.
   - In `instructions:`, cite `ralph-tools-emit` Policy-Check feedback instead of copying field tables. The prompt builder supplies the per-topic schema-aware publish section.
   - **Artifact-First handoff closure (单 hat 视角)**：每条 hat 的 instructions 必须明确产出顺序——「实际执行的 hat 或其 sub-agent 先写 artifact → hat 验收文件 → `ralph emit --policy-check` → 真实 emit」。消费型 hat 的 instructions 必须明确「从路径读完整内容后再决策」。
   - **Trigger Context 收敛**：trigger-consuming hats 的分支判定（accept / fix-now / blocked、residual 处理边界）若用 payload if/else 表达，必须先收敛到 `event_policy.schemas.<topic>.trigger_context.routing_hints`，再用 `summary_fields` 暴露关键计数；`instructions` 只引用 `## TRIGGER CONTEXT` 区块，不复制 hint 条件值。详情见 `references/author-checklist.md`「Trigger Context 审核项」。

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
   - **Artifact-First Handoff 5 问全 ✓**: 填 `references/author-checklist.md` 的「Hard questions — Artifact-First Handoff」段；任一 ✗ 必须改写或显式 justify。
   - For builtin edits, list the 7-point sync checklist (do not auto-apply).
   - **If any check fails: STOP.** Do not recommend review or deliver YAML as complete.

6. **Hand off to `ralph-preset-review`** only after step 5 passes — does not replace `ralph preset check`.

## Guardrails

- **No whole-file agent perspective** in `instructions:` — no "the reviewer will…", no topology position.
- **No internal ledger reads** — no `.ralph/events.jsonl`, `.ralph/supervisor.db`, `.ralph/loops.json`.
- **No internal-ledger-as-artifact** — hat instructions 不得要求把 `.ralph/events.jsonl` / `.ralph/loops.json` / `.ralph/supervisor.db` 当业务 artifact 接口(写或读)。业务 artifact 必须落在 `.ralph/<plan>/<unit>/...` 等业务子目录。
- **Emitter hats:** require `--policy-check` before emit; cite `ralph-tools-opac` and `ralph-tools-emit` §5.
- **Artifact-First emitter order:** emitter hat instructions 必须明确「先写业务 artifact,再 `ralph emit --policy-check`,再真实 emit 携带路径的事件」。不得先 emit 再补文件,不得把完整结果内联到 event payload。
- **Artifact-First consumer order:** consumer hat instructions 必须明确「从当前 hat 可见输入(trigger payload / projection / task view / `ralph tools task list`)取得路径,再 `cat` / `Read` 完整内容后再决策」。不得只看 payload 中的摘要或长字段。
- **Artifact-First lifecycle:** 每份重要 artifact 必须在 Payload Contract 同行写明消费方与最终保留 / 归档 / 清理责任;不允许「一直追加、永不清理」或「无消费者」的中间文件。
- **Artifact-First 不落盘例外:** 只有短暂 + 短小 + 无需恢复的信息可以不落盘;必须在 Payload Contract 同行写「不落盘 + 理由」(恢复 / 审计 / 下游依赖),不能仅按字符数判断。详细判定三标准见 `references/agent-native-model.md`「Artifact-First Handoff 模型」段。
- **Artifact-First preset ownership:** preset 自身不得被描述为 artifact 创建者;实际由 hat 或其 sub-agent 在 activation 创建/更新。禁止在 preset 注释或 instructions 中写「preset creates X.md」类语句。
- **Policy-check feedback:** emitter instructions must cite `ralph-tools-emit` Policy-Check feedback; schema metadata belongs in `event_policy.schemas`, not in prose instructions.
- **Schema metadata:** `field_docs` and `examples` are agent-facing repair guidance only. They must not be treated as validation authority; machine acceptance still comes from `required_fields`, `allowed_values`, `hat_allowed_values`, and element constraints. **`field_docs.<path_field>.meaning / source / fill_rule / examples[]` 必须满足 `references/finding-rubric.md`「Artifact-First Handoff `field_docs` 审核点」**。
- **`--triggered`:** only use hat ids declared in preset `hats[]`; verify with `ralph emit --policy-check --triggered <hat> …` (see `references/commands.md`).
- **`task_id` / `task_key` / `step`:** cite `ralph-tools-tasks` red box; never hand-write `task_id`.
- **Single business event budget** per isolated activation; no business events before terminal emit.
- **Do not duplicate** `ralph-tools*.md` content into instructions.

## Output Expectations

- Updated preset YAML
- `preset-author-notes.md` with one complete AAF table **+ one Payload Contract table** per hat
- Updated schema metadata for agent-authored emit topics (`field_docs` / safe `examples`) or an explicit no-op rationale
- Explicit handoff message: preset path + notes path → invoke `ralph-preset-review`

## Artifact-First Handoff Acceptance (loop 外)

交 review 前自检必须全部满足；任一未满足即按 `references/finding-rubric.md` 「Artifact-First Handoff finding_id」 表预演 finding(可与 author notes 同目录附 `/tmp/author-artifact-first-self-check.md` 自检快照):

1. **每条写入型 hat 都声明了当前 `.ralph/` 下的 artifact 路径集合**;preset 文本不把自己描述为文件创建者。
2. **每条 consumer hat 的 instructions 显式要求从路径读完整内容**;不依赖 prompt 中的长文本。
3. **每个被传递的完整结果 / 长内容 / 跨 hat 摘要都已落盘**,event / message 只保留短状态、摘要、路径、必要身份与路由字段。
4. **没有任何 hat 把 `.ralph/events.jsonl` / `.ralph/loops.json` / `.ralph/supervisor.db` 当业务 artifact 接口**。
5. **每条「不落盘」例外都在 Payload Contract 同行写明可审核理由**(恢复 / 审计 / 下游依赖);不出现「仅字符数少」式理由。
6. **每份重要 artifact 都有消费方以及最终保留 / 归档 / 清理责任**;中间文件不能无限增长或无人认领。
7. **Payload Contract 每行的 `artifact 落盘` 列已填**(必填 / 可选 / 不需要 / 不落盘+理由);没有空白行。

## Read These References When Needed

- AAF model and prompt stack: `references/agent-native-model.md`(含「Artifact-First Handoff 模型 / 状态传递 / 边界 / 知识分层 / Review 必须独立重做的 artifact-first 检查」)
- Checklist and AAF template: `references/author-checklist.md`(含「Artifact-First topic 判别」「Artifact-First 单 hat 视角审核项」「Hard questions — Artifact-First Handoff」)
- Validation commands: `references/commands.md`
- Topology patterns (topology phase only): `references/patterns.md`
- Finding 命名与 severity: `references/finding-rubric.md`(「Artifact-First Handoff → Severity」「Artifact-First Handoff finding_id」「Artifact-First Handoff `field_docs` 审核点」三段)
