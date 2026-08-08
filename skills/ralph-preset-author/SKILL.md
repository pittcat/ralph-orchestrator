---
name: ralph-preset-author
description: Discover and confirm user intent through interactive choices, then draft and validate Ralph preset YAML with per-hat Agent-Feasibility (AAF) checks. Use when creating or editing presets/en/, presets/schemas/, builtin presets, or .ralph/hats/ workflow files — including topology, state_projection, hat instructions, and handoff design.
---

# Ralph Preset Author

Use this skill to design and draft Ralph **presets** (builtin or local) with **Agent 视角可行性（AAF）** discipline.

**Boundary:** This skill covers the full preset chain including `presets/en/` and `presets/schemas/`. User-only `.ralph/hats/` collection authoring (create / inspect / validate user hat workflows) and topology-debug / validate-routing workflows are not owned by either preset skill. For review after drafting, hand off to `ralph-preset-review`.

## Use This Skill For

- Creating or editing builtin presets (`presets/en/`, `presets/schemas/`)
- Creating local presets from templates (`ralph preset new`)
- Designing event topology, `state_projection`, and handoff fields
- Designing event schema metadata (`field_docs` / `examples`) that lets agents repair policy-check failures
- **Declaring schema-backed `trigger_context`** (`summary_fields` + `routing_hints` + `known_fields`) for trigger-consuming hats — collapses duplicated payload if/else in `instructions` into a single injected `## TRIGGER CONTEXT` block
- Writing per-hat `instructions:` in isolated mode (one agent per activation)
- Producing `preset-author-notes.md` (AAF 五问表 per hat) before review
- **Per-key-hat scope and opt-in decision confidence gate (Author side)**: forming a Gate Scope table for hats that carry terminal authority, production mutation, phase branching, multi-hat aggregation, critical artifact production, or key handoff responsibility; asking the operator to choose hard/record/off before YAML drafting; recording the chosen metric matrix in `preset-author-notes.md`.
- **Artifact-First Handoff authoring** (R1–R7): declaring artifact 落盘点、消费方、生命周期责任，并在 Payload Contract 与 AAF 五问表中固化每一份重要信息的落盘判定
- **模板文件压缩 instructions（软性推荐）**: 当 hat `instructions:` 需要承载大段固定格式文档（报告模板、计划模板、验收清单、SOP 步骤等）时，推荐采用 `presets/templates/` + `ralph preset materialize-artifacts` 机制（参考 `parallel-forge`），把模板内容移出 prompt、改为运行时复制填写，从而压缩上下文。此推荐不强制，但 author 应在 Drafting phase 前通过菜单询问用户是否采用。

## Core Assumptions

- Each hat activation in `execution_mode: isolated` sees **only its own** `instructions` plus runtime injection — not other hats' instructions.
- **Prompt visibility evidence MUST come from `ralph inspect prompt`**, not from memory: see `references/prompt-visibility.md`. The shared command is the SSOT for `auto_inject` / `on_demand` and is enforced by `skills/tests/test_prompt_visibility_contract.py`.
- State passes via **emit → state_projection → task/progress → Observe**, not by reading internal ledgers.
- **Artifact-First Handoff assumption:**
  - **文件是重要信息的事实源**：完整结果、长内容、跨 hat 摘要、关键决策依据、验证证据、高成本重建信息默认必须落盘。
  - **事件承担控制面，文件承担数据面**：event payload 只携带短状态、摘要、路径、必要身份与路由字段。
  - **默认强制但允许有理由的例外**：只有短暂 + 短小 + 无需恢复的信息可以不落盘；例外必须在 Payload Contract 同行标注理由（恢复 / 审计 / 下游依赖），不能仅按字符数判断。
  - **落盘位置限定在当前 `.ralph/`**：业务 artifact 落在当前 workspace / worktree 的 `.ralph/<plan>/<unit>/<file>.md` 等业务子目录；不得把 `.ralph/events.jsonl`、`.ralph/loops.json`、`.ralph/supervisor.db` 等 runtime internal ledger 当作业务 artifact 接口。
- Command syntax lives in `crates/ralph-core/data/ralph-tools*.md` — **cite skill sections, do not copy parameter tables**.

## Workflow

0. **Discovery and user-confirmation gate (MUST — before topology or YAML):**
   - **Task brief 前置输入（可选,来自 `ralph-task-discovery`）:** 调用方可选提供一个 repo-relative 的 `task_brief_path`。提供时,author 必须在 Discovery gate 内先复核并消费它:
     1. 按 `skills/ralph-task-discovery/references/author-handoff.md` 的顺序读取与校验:文件存在 → YAML 可解析 → `schema_version` 受支持 → `project_root` 与当前目标项目根一致 → 运行 `brief_validator.validate_brief_text` → 检查 `status` / `author_ready` → 字段消费。确定性参考实现:`skills/ralph-task-discovery/scripts/author_handoff.py` 的 `evaluate_task_brief(brief_path, target_project_root)`。不得信任 brief 的自我声明而跳过 validator 复核。
     2. brief invalid / blocked / 非 `author_ready` / stale(provenance 失效,判据见 author-handoff.md「stale brief 判据」段)→ **停在 Discovery gate**,输出 `task_brief_invalid` + 具体错误(validator code/path),**不生成任何 preset YAML**,也不消费 brief 的任何字段。
     3. valid 且 `author_ready=true` → brief 成为**已确认输入**:Preset Intent Confirmation 必须引用 brief 的 Goal、成功条件(acceptance)、阻塞条件(failure boundaries)、scope 与 Evidence refs;selected candidate 结论作为方案输入——仅消费 validator `candidate_gates` 结论为 `selected` 的候选,**被 rejected 的候选不得被当作 selected 使用**。
     4. brief 不豁免任何既有门禁:Discovery / Intent Confirmation / AAF / Payload Contract / prompt visibility / pre-review gate / review handoff 全部照常执行;brief 的 `author_ready` 标志不是跳过这些既有门禁的许可。
   - First inspect the user's request, the existing preset/schema, nearby documentation, and repository conventions. Build a provisional understanding before asking anything. **Do not ask the user for facts that can be discovered from the repository.**
   - Separate unresolved items into three groups:
     1. repository-verifiable facts — inspect and resolve them yourself;
     2. author-owned implementation choices — derive and recommend them yourself (for example hat count, topic names, or the smallest viable topology);
     3. user-owned intent choices — ask the user when they can change business outcome, acceptance criteria, authority/mutation boundaries, source of truth, artifact ownership, failure behavior, or required review independence.
   - Ask user-owned questions through an **interactive choice menu** (`AskUserQuestion` / `Question` when available; otherwise a numbered list in chat and wait for the reply). Each question must:
     - present 2–4 concrete, mutually exclusive choices;
     - put the recommended choice first and explain its consequence in one sentence;
     - allow the user to supply a custom answer when none fits;
     - avoid exposing internal topology jargon when a business-language choice is possible.
   - **Execution-model menu (MUST ask when material topology or parallelism is in scope)**: for a new preset or any material topology change that touches parallelism / multiple units / unclear orchestration, present the execution-model menu below. The vocabulary (single-chain | wave | supervisor | supervisor+wave) is frozen in `references/agent-native-model.md`「执行模型（Execution Model）」段; do not invent synonyms. Required behavior:
     1. **Recommended first**: `single-chain` — default path; lowest complexity; parallel work stays inside the executor hat's subagent boundary.
     2. Second: `wave` — main chain has one step that fans out across many workers on the same topic (uses `ralph wave emit` / `ralph wave verify`).
     3. Third: `supervisor` — runtime manages multiple slots / worktrees / queueing / fan-in (`event_loop.supervisor.enabled: true`).
     4. Fourth: `supervisor+wave` — supervisor where the dispatcher also fans out with wave.
     5. The first item is always `single-chain`; do **not** reorder or hide it.
     6. The user may supply a custom answer; if the answer is non-observable ("适当并行", "必要时用 supervisor", "由作者决定"), grill again with these same options before drafting.
   - **Deny → lock single-chain (hard rule)**: if the user denies wave / supervisor (or selects single-chain outright), the author must:
     1. Write `execution_model: single-chain` into the Intent Confirmation (field defined in `references/author-checklist.md`).
     2. **Not** introduce `event_loop.supervisor.enabled: true` in the YAML.
     3. **Not** introduce a dispatcher hat that calls `ralph wave emit` / `ralph wave verify`.
     4. **Not** silently upgrade to wave / supervisor under any pretext ("作者觉得这样更清晰" / "调度更顺手" / etc.).
   - **Narrow mechanical-edit exception**: when the change is a narrow mechanical edit (renames, doc-only changes, value tweaks) with no behavior ambiguity and no topology implication, the author may skip the execution-model menu. The author must still record the inferred execution-model in the Intent notes (or in commit message) so review can confirm consistency.
   - Ask in small rounds (prefer 1–3 related questions). Use each answer to inspect or infer the next real uncertainty; **do not dump a static questionnaire** and do not ask already-settled questions.
   - Grill further whenever the answer is vague, contradictory, non-observable, or shifts a critical choice elsewhere (for example「适当处理」「必要时修复」「由上游决定」). Turn it into selectable operational alternatives and ask again.
   - Before drafting, show a concise **Preset Intent Confirmation** containing: goal, operator journey, inputs/source of truth, success condition, blocked condition, allowed mutation scope, required independent review, important artifacts/consumers, non-goals, and author assumptions. Use a final choice menu: **确认并开始设计（recommended） / 返回修改 / 暂停**.
   - For a new preset or any material behavior change (topology, terminal semantics, mutation authority, handoff, recovery), explicit confirmation is mandatory. For a narrow mechanical edit with no behavioral ambiguity, state the inferred intent and proceed without forcing an interview.
   - **If a material ambiguity remains or the user has not confirmed: STOP.** Do not draft YAML/schema and do not present a topology as final.

0d. **Key-hat scope and opt-in decision gate (capability-triggered; operator chooses the mode):**
   - Author builds a **preliminary scope (初评)** table from capability signals (one row per key-hat before drafting). The reviewer will independently re-derive its own scope later; author's preliminary scope is preserved as a comparison source, not as a binding scope.
   - Capability triggers that put a hat into Gate Scope (Chinese/English anchors both kept so review can grep either vocabulary):

     | Trigger signal | Plain-language description |
     | --- | --- |
     | terminal authority / 终态 authority / 终态 决策 | Hat can publish success, failure, or blocked terminal events. |
     | production mutation / production code / 修改 生产 | Hat can change production code, tests, or configuration. |
     | phase branching / 重试 决策 / 阶段 分支 | Hat decides stage transitions, retry, fix, rollback, or stop. |
     | multi-hat aggregation / 跨 hat 汇总 | Hat merges results from multiple hats into one downstream artifact. |
     | artifact producer / 关键 artifact | Hat writes a downstream-critical artifact. |
     | key handoff / 关键 handoff | Hat publishes a handoff whose downstream consumer must rely on the payload to decide. |

   - **Plain passthrough hats** (pure read-only reformatting that makes no decision, no authority, no mutation) are **out of scope**. They are not forced to fill any metric and are not blocked by gate failures.
   - **Ask the user once per authoring session, after Intent Confirmation and before topology/instructions drafting**:

     1. **启用硬门禁 (hard, recommended when the preset is material)** — apply the thresholds below and treat non-zero `Critical Ambiguities` / `Critical Unverified Assumptions` as a hard block.
     2. **仅记录不阻塞 (record)** — record the metrics and evidence in the notes; do not block the existing AAF / Payload workflow.
     3. **不启用 (off)** — do not run this gate at all; existing AAF, Payload Contract and mechanical-lint rules continue unchanged.

     Provide the choice via the same interactive choice menu used in Workflow 0. The user may supply a custom answer; non-observable answers must be grilled into the same three options before drafting.
   - **Metric applicability matrix (skill chooses; the user does NOT tick boxes per hat)**:

     | Role signals                                | Always-applied metrics                              |
     | -------------------------------------------- | --------------------------------------------------- |
     | terminal authority / phase branching / multi-hat aggregation | `Confidence`, `Evidence Coverage`, `Verifiability` |
     | production mutation (code / test / config)   | `Evidence Coverage`, `Verifiability`, `Impact Certainty` |
     | artifact producer / key handoff             | `Evidence Coverage`, `Impact Certainty`, `Verifiability` |
     | every hat that enters Gate Scope            | `Unverified Assumptions` (full list) **plus** the structured subsets `Critical Ambiguities` and `Critical Unverified Assumptions` |

   - **Thresholds (hard gate uses each value independently; no average-score escape hatch)**:

     | Metric                              | Hard-gate threshold                |
     | ----------------------------------- | ---------------------------------- |
     | `Confidence`                        | `>= 85`                            |
     | `Evidence Coverage`                 | `>= 80`                            |
     | `Verifiability`                     | `>= 80`                            |
     | `Impact Certainty`                  | `>= 75`                            |
     | `Critical Ambiguities`              | `= 0`  (structurally enforced: cannot be individually disabled when the gate is enabled) |
     | `Critical Unverified Assumptions`   | `= 0`  (structurally enforced: cannot be individually disabled when the gate is enabled) |
   - **Critical checks are structural, not opinion**: when the gate is enabled the two `Critical ...` counts must be filled in for every Gate Scope hat; the agent is not free to mark them N/A. The `off` choice is the only way to skip them entirely; that choice preserves all existing AAF/Payload/lint rules.
   - **What goes into `preset-author-notes.md`** — append a **Gate Scope** table after the Intent Confirmation:

     ```
     | Hat | Trigger reason | Applicable metrics | Evidence | Unverified assumptions | Critical ambiguities | Critical unverified assumptions | Mode | Decision |
     ```

     Evidence is one row per source (`ralph capability inventory --format json` excerpt, `ralph inspect prompt --hat <id> --format json` excerpt, schema field, doc reference, prior plan commit, etc.). `Decision` is `pass | re-confirm | block` per hat — for the `record` mode it must be set but does not block the workflow.
   - **Reconciliation before pre-review gate**: if topology drafting changed any hat's authority / mutation / handoff, recompute the Gate Scope table before Workflow 5. Notes and YAML scope must not silently diverge.
   - **Identity rule (capability-triggered, 不得 name-prefix)**: this section is a hard-rule, capability-triggered identification. Author must never identify a key hat by preset or hat name; equal capabilities yield equal rules regardless of preset or hat name.
   - **不与 0e 关键环节事件门禁字段复用**：本段 mode 字段 `hard/record/off` 与 0e 的 `guard_selection` / `precheck_guard` / `payload_consistency_guard` / 两个 retry budget 字段语义完全不同——前者是 Gate Scope 表的 metric 阈值模式，后者是各关键位置的 guard 类型与各自 budget。review 命中「字段复用」即 `preset.key_stage_event_gate_field_reuse` finding，参见 `finding-rubric.md`「Key-stage event gate」段。

0e. **Key-stage event gate (capability-triggered; per-location guard selection)**:
   - **触发条件**：完成 Workflow 0d Gate Scope 表之后、起草阶段 2 拓扑之前，对每个被 Gate Scope 列入的关键 hat 的关键 handoff / 阶段分支逐位置识别。**逐位置识别**的输入信号与 0d Gate Scope 同源（terminal authority / production mutation / phase branching / multi-hat aggregation / artifact producer / key handoff），但**维度不同**：0d 决定 hat 是否进入 Gate Scope，0e 决定该 hat 内哪些事件位置需要 guard。**禁止**用一个 preset 全局选择替代逐位置确认。
   - **与 Gate Scope `hard/record/off` 的关系**：0d 选 mode 决定 Gate Scope 表内 metric 阈值是否阻塞；0e 选 guard 类型决定在该 hat 的哪个事件位置增加 `precheck guard` / `payload consistency` 拦截。两者**不是同一字段、不是同一问题、不可互相复用**。0d 选 `off` 不豁免 0e 的逐位置询问；0e 选 `neither` 也不豁免 0d 的 metric 评估。
   - **逐位置询问菜单**：对每个被识别为关键位置的 handoff / 阶段分支，author 必须用 `AskUserQuestion` 给出 4 选 1:
     1. **加入 `precheck guard`**（推荐当该位置需要主观质量判断）——在 `event_loop.precheck.rules.<topic>` 声明事件级 LLM gate；producer 先进入 `<topic>.proposed`，合成 gate hat 通过后才发 `<topic>`，拒绝时按现有 `on_fail` / bounded retry / exhaustion 语义恢复。它不是 `ralph emit --policy-check`：后者仍是独立的确定性 schema/ownership 预检，两者都必须遵守且不能互相替代。
     2. **加入 `payload consistency`**（推荐当该位置涉及 payload 内字段一致性 / 跨字段约束）——runtime 走 `event_loop.event_policy.payload_consistency.rules` + `payload_consistency:<rule_id>` gate 拦截，与现有 `event_policy.payload_consistency` 规则共用空间。
     3. **加入 both**（当两类都成立）——两类 guard 共存；各自独立 retry budget（见下）。
     4. **暂不加入（neither）**——本位置不新增上述 guard；既有 AAF / Payload Contract / mechanical lint / 0d Gate Scope 全部不受影响。
   - **每类 guard 独立 retry budget**：对每个用户选定的 guard 类型，author **必须**再问一次 3 / 2 / 1 中的一个；默认 3；不可共享总预算。两类 guard **不**共享总预算、计数器或 exhaustion state；其 retry / correction / blocked 语义继续引用现有 runtime（`crates/ralph-core/data/ralph-tools-emit.md` §5 与 `ralph-tools-recovery-directives.md`）。
   - **降级 / 关闭必须记录原因**：用户选择 `neither`、或选择低于推荐覆盖范围 / 默认 budget（3）时，author 必须在 notes 中记录简短理由（≤80 字），不得用「用户偏好」「后续再说」「先这样」等空话。理由必须指向具体风险（恢复 / 审计 / 下游依赖等）而非字符数。
   - **未确认停止**：任一关键位置未拿到 guard 选择 + 各自 budget + 确认状态前，author **不得**把该选择当作已确认事实，**不得**继续生成依赖它的最终 YAML / schema 设计。Workflow 5 pre-review gate 会逐位置复审。
   - **notes contract 字段固定**（按关键位置各填一行）：
     - `key_stage`：关键 handoff / 阶段分支的人类可读标识（例如 `executor → fixer main handoff`、`work.done terminal`）。
     - `guard_selection`：`precheck` / `payload_consistency` / `both` / `neither` 四选一。
     - `precheck_guard`：布尔（`true` 当且仅当 `guard_selection ∈ {precheck, both}`）。
     - `precheck_retry_budget`：整数 3 / 2 / 1，`precheck_guard=false` 时填 `null`。
     - `payload_consistency_guard`：布尔（`true` 当且仅当 `guard_selection ∈ {payload_consistency, both}`）。
     - `payload_consistency_retry_budget`：整数 3 / 2 / 1，`payload_consistency_guard=false` 时填 `null`。
     - `reason`：≤80 字理由，包含关闭 / 降级 / 选择覆盖范围的根据。
     - `confirmation_status`：`confirmed` / `pending` / `rejected` 三选一；非 `confirmed` 即视为未确认。
   - **身份规则（capability-triggered）**：author 必须按 hat 能力信号识别关键位置，不得按 preset / hat 名称套用；同一能力信号在不同 hat 上得到同一组 guard 选项。
   - **不替 runtime 决策**：作者不得借 0e 段落新增 runtime 配置、计数器、恢复路径或绕过 guard 的替代行为；所有 retry / correction / blocked 描述只引用现有 `ralph-tools-emit.md` / `ralph-tools-recovery-directives.md` 等已注入 skill，禁止凭印象写出新 runtime 规则。

1. **Classify target:** local (`.ralph/hats/*.yml`) vs builtin (`presets/en/` + `presets/schemas/`). Note `execution_mode` and hat count (4+ → `isolated` mandatory). This step begins only after the Discovery gate passes.

2. **Topology phase (author brain):**
   - **Default execution model = single-chain**. The single-chain default is **only** relaxed when the user's Intent Confirmation has `execution_model ∈ {wave, supervisor, supervisor+wave}` **and** the corresponding hard-question section in `references/author-checklist.md` is fully ✓ with evidence. Do **not** upgrade the model on author preference or to "match a builtin preset name"; the choice is capability-triggered and user-owned.
   - Read schema SSOT for builtin presets.
   - Sketch event flow (topics, not prompts).
   - Align each handoff: upstream Q4 fields ↔ downstream Q2 Observe path.
   - **Artifact-First topic 判别**（每条 emit topic）：每条 emit 字段先判定是否属于「完整结果 / 长内容 / 跨 hat 摘要 / 关键决策依据 / 验证证据 / 高成本重建」。若是,完整内容必须由执行该 hat 或其 sub-agent 写到当前 `.ralph/` 下的业务 artifact;event payload 只携带路径 / 短摘要 / 必要身份 / 路由字段。判定三标准(恢复价值 / 审计价值 / 下游依赖)与术语定义见 `references/agent-native-model.md`「Artifact-First Handoff 模型」段。
   - For every agent-authored emit topic, decide whether `event_policy.schemas.<topic>.field_docs` and `examples` are needed. Any required handoff, identity, verdict, count, file path, or reason field needs field-level metadata unless the field is already self-evident from injected skill docs. **`field_docs.<path_field>.meaning` 必须明确「该路径是 artifact 落盘点」**,`source` 必须指向当前 hat 可见输入,`fill_rule` 不能诱导 agent 伪造路径,`examples[]` 用结构占位(`.ralph/<plan>/<unit>/<file>.md`)而非固定业务文件名(详见 `references/finding-rubric.md` 「Artifact-First Handoff `field_docs` 审核点」)。
   - See `references/patterns.md` for examples only at this stage.
   - **Opt-in same-payload consistency rules**: when a preset needs an inter-field invariant on a single emit payload (e.g. two fields that must agree), declare rules under `event_loop.event_policy.payload_consistency.rules` (lint covers `rule.id` / `rule.topic` / `when` field references); see `references/author-checklist.md`「Payload Consistency 审核项」for the audit items.

3. **Drafting phase (single-hat agent brain):**
   - **Prompt Visibility 必查（每条 hat）**：起草或修改某 hat 的 `instructions:` 之前，**先跑** `ralph -c <preset>.yml inspect prompt --hat <hat_id> --format json`，把返回的 `auto_inject` / `on_demand` / `block_titles` 作为该 hat 真实可见性证据。详细规程与字段约定见 `references/prompt-visibility.md`。
     - **禁止**把 on-demand skill（如 `ralph-tools-emit` / `ralph-tools-wave` / `ralph-tools-cmdref`）写成「已自动注入」。
     - **禁止**引用 auto-inject skill 时让 agent 再 `ralph tools skill load <name>`（已注入就不必再 load）。
     - **禁止**复制 `ralph-tools*.md` 命令表到 `instructions:`——按 `crates/ralph-core/data/*.md` 注入，**只引用章节名**。
     - 外仓（无 `crates/ralph-core/data/`）时仍可用 `inspect prompt`：内容来自当前 ralph 二进制内嵌；报告与 review 标注须注明来源。
     - **场景化激活预览**：
       ```bash
       # 静态预览（无场景参数）
       ralph -c <preset>.yml inspect prompt --hat <hat_id> --format json

       # 场景化预览（指定 trigger / payload / source-hat / iteration / wave-context 等）
       ralph -c <preset>.yml inspect prompt --hat <hat_id> --format json \
           --trigger build.task --source-hat planner --payload '{"task":"refactor-x"}'
       ```
       输出含 `trigger_context_injected`、`wave_context_injected`、`orchestrator_context_injected`、`correction_injected`、`skill_gates`、`evidence_level` 字段；`evidence_level` 标识证据等级（`static` / `runtime` / `unverified`，默认静态预览可省略该字段）。

2.5. **Capability discovery (mandatory for new presets / material changes)**：
   - Run `ralph capability inventory --format json` and walk each capability's `applies_when` field.
   - For each capability the preset legitimately uses, ensure the corresponding review evidence path in `references/finding-rubric.md` / `agent-native-model.md` is cited in `preset-author-notes.md`.
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
   - **Instructions ↔ schema required-fields SSOT 对账（强制）**：每写一个 emitter hat 的 `ralph emit <topic>` 示例，立即提取示例 payload 的实际字段集合，与同一 preset 的 `event_policy.schemas.<topic>.required_fields` 做集合对账；逐字段确认占位值能从当前 trigger、注入上下文或本 hat 产物取得。缺字段、字段名漂移、错误上游引用或无法取得值时，必须先修正 schema / instructions / Payload Contract，不能交给 review 或让 agent 自己猜字段。
   - In `instructions:`, cite `ralph-tools-emit` Policy-Check feedback instead of copying field tables. The prompt builder supplies the per-topic schema-aware publish section.
   - **Artifact-First handoff closure (单 hat 视角)**：每条 hat 的 instructions 必须明确产出顺序——「实际执行的 hat 或其 sub-agent 先写 artifact → hat 验收文件 → `ralph emit --policy-check` → 真实 emit」。消费型 hat 的 instructions 必须明确「从路径读完整内容后再决策」。
   - **Trigger Context 收敛**：trigger-consuming hats 的分支判定（accept / fix-now / blocked、residual 处理边界）若用 payload if/else 表达，必须先收敛到 `event_policy.schemas.<topic>.trigger_context.routing_hints`，再用 `summary_fields` 暴露关键计数；`instructions` 只引用 `## TRIGGER CONTEXT` 区块，不复制 hint 条件值。详情见 `references/author-checklist.md`「Trigger Context 审核项」。

3. **模板文件机制询问（软性推荐，不强制）**：
   - **触发条件**：当某 hat 的 `instructions:` 需要承载大段固定格式文档（报告模板、计划模板、验收清单、SOP 步骤、审计表等），或单 hat `instructions:` 长度预计超过 ~80 行时，author 应通过交互菜单询问用户是否采用模板文件机制。
   - **菜单示例**：
     ```text
     该 hat 的 instructions 需要包含固定格式文档（如报告模板 / 计划模板 / 验收清单）。
     推荐做法：把模板内容抽到 presets/templates/<preset>/ 并在编译期内嵌，
     hat 运行时通过 `ralph preset materialize-artifacts` 复制填写，从而压缩上下文。
     是否采用模板文件机制？
       1) 采用（推荐）— instructions 只写「materialize → 复制 → 填模板」三步
       2) 不采用 — 模板内容直接写在 instructions 里
       3) 部分采用 — 仅对最长的 1–2 个模板使用文件
     ```
   - **若用户选择采用**：在 preset 同级目录创建 `presets/templates/<preset>/`，把固定格式文档写成模板文件；在 `instructions:` 中只保留「先 materialize 再复制填写」的简短指引（参考 `presets/en/parallel-forge.yml` 的 planner / executor / reporter 写法）。**注意**：当前 runtime 仅内嵌 `parallel-forge` 的模板目录；若为新 preset 采用模板机制，需同步扩展 `crates/ralph-cli/src/builtin_artifact_templates.rs` 的 `templates_for_preset` 匹配分支（或改用本地文件路径方案），并更新 `crates/ralph-cli/build.rs` 的模板拷贝逻辑。
   - **若用户选择不采用**：继续把模板内容写在 `instructions:` 内，author 需在 `preset-author-notes.md` 中记录「未采用模板文件机制」及理由，供 review 参考。
   - **记录**：无论是否采用，都应在 `preset-author-notes.md` 的 Intent Confirmation 或对应 hat AAF 表旁注明该决定。

4. **Assemble `preset-author-notes.md`** next to the preset YAML (all AAF tables + Payload Contract tables).
   - Put the confirmed `Preset Intent Confirmation` at the beginning so authoring and review share the same business baseline.

5. **Pre-review gate (MUST — do not skip):**
   - Every hat has a complete AAF table **and** a complete Payload Contract table in `preset-author-notes.md`.
   - Hat count in notes **equals** hat count in YAML; per-emit-topic row count covers every material `publishes` entry.
   - No empty cells; no「待定」「同上」「上游会处理」「约定俗成」.
   - Every `task_id` / `task_key` / `step` row is marked `live required` with a concrete observation command.
   - Multi-trigger hats split Payload Contract by trigger, not collapsed into one row.
   - Required handoff / identity / decision fields have `field_docs` metadata or a documented reason why existing injected docs already explain the field.
   - Emitter instructions reference `ralph-tools-emit` Policy-Check feedback when they mention payload construction, `ralph emit`, `ralph wave emit`, required fields, or field shape.
   - **Instructions ↔ schema required-fields SSOT 对账已完成**：每个 emitter 示例字段集合等于对应 schema 的 `required_fields`，并且每个字段都有可验证值源；在 `preset-author-notes.md` 记录 schema 行与 instructions 行证据。
   - Ask: "If I only received this hat's instructions + injection, can I complete Q1? Can I construct every Q4 field from visible sources?"
   - **Single-chain-first 5 问全 ✓**: 填 `references/author-checklist.md` 的「Hard questions — single-chain-first」段；任一 ✗ 必须改写或显式 justify。
   - **执行模型分支 Hard questions 全 ✓（按 model 分支强制 / N/A）**: 按 `references/author-checklist.md`「Hard questions — N/A 规则」段的模型-分支矩阵填：
     - `single-chain` → wave / supervisor 两段标 N/A（不得留空 / 不得引入 `event_loop.supervisor.enabled` / 不得写 dispatcher `wave emit`）。
     - `wave` → 「Hard questions — wave fan-out」7 问全 ✓ + 证据；supervisor 段标 N/A。
     - `supervisor` → 「Hard questions — supervisor orchestration」6 问全 ✓ + 证据；wave 段标 N/A。
     - `supervisor+wave` → wave 7 问与 supervisor 6 问同时全 ✓，与 Intent.execution_model 一致。
     - 不一致（YAML 与 Intent）按 `finding-rubric.md`「Wave / Supervisor capability audit」段 `preset.execution_model_intent_mismatch` 入 review 主表。
   - **Artifact-First Handoff 5 问全 ✓**: 填 `references/author-checklist.md` 的「Hard questions — Artifact-First Handoff」段；任一 ✗ 必须改写或显式 justify。
   - **Key-stage event gate (0e) 复查**：每个被 Gate Scope 列入的关键 hat 必须有 `Key-stage event gate` 表（按 notes 字段固定 8 列：`key_stage` / `guard_selection` / `precheck_guard` / `precheck_retry_budget` / `payload_consistency_guard` / `payload_consistency_retry_budget` / `reason` / `confirmation_status`）；所有行的 `confirmation_status` 必须为 `confirmed`；`guard_selection` ∈ {`precheck`, `payload_consistency`, `both`, `neither`}；`precheck_guard=true` ⇔ `guard_selection ∈ {precheck, both}`；`payload_consistency_guard=true` ⇔ `guard_selection ∈ {payload_consistency, both}`；`precheck_guard=true` ⇒ `precheck_retry_budget ∈ {3, 2, 1}`；`payload_consistency_guard=true` ⇒ `payload_consistency_retry_budget ∈ {3, 2, 1}`；`precheck_guard=false` ⇒ `precheck_retry_budget` 为 `null`；`payload_consistency_guard=false` ⇒ `payload_consistency_retry_budget` 为 `null`；两 budget 不共享；选择 `neither` 或 budget 低于 3 必须有 ≤80 字 `reason`，不得为空。任一不满足 → STOP 不得交付。
   - **Key-stage 与 Gate Scope 字段隔离**：0d 的 `hard/record/off` 与 0e 的 `guard_selection` / `precheck_guard` / `payload_consistency_guard` / `precheck_retry_budget` / `payload_consistency_retry_budget` 字段语义不混；不得把 Gate Scope `off` 字段当作 0e 的关键位置选择。违规 → `preset.key_stage_event_gate_field_reuse` finding（参见 `finding-rubric.md`「Key-stage event gate」段，review-only）。
   - For builtin edits, list the 7-point sync checklist (do not auto-apply).
   - **If any check fails: STOP.** Do not recommend review or deliver YAML as complete.

6. **Hand off to `ralph-preset-review`** only after step 5 passes — does not replace `ralph preset check`.

## Guardrails

- **No whole-file agent perspective** in `instructions:` — no "the reviewer will…", no topology position.
- **No internal ledger reads** — no `.ralph/events.jsonl`, `.ralph/supervisor.db`, `.ralph/loops.json`.
- **No internal-ledger-as-artifact** — hat instructions 不得要求把 `.ralph/events.jsonl` / `.ralph/loops.json` / `.ralph/supervisor.db` 当业务 artifact 接口(写或读)。业务 artifact 必须落在 `.ralph/<plan>/<unit>/...` 等业务子目录。
- **Emitter hats:** in the activation, first run `ralph tools skill load ralph-tools-emit`; if loading fails, stop without emitting. Then require `--policy-check` before emit and cite `ralph-tools-opac` and `ralph-tools-emit` §5. Missing the explicit load is review-only `preset.instructions_emit_skill_load_missing` (P1, confidence 85).
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
- **Decision-gate scope is capability-triggered, not name-prefixed.** The Gate Scope table is built from authority / mutation / branching / aggregation / artifact / handoff signals. Two hats with the same capability must receive the same rule regardless of preset or hat name. Hard rule: 禁止 identifying a key hat by preset or hat name.
- **Decision-gate off mode preserves existing AAF / Payload / lint.** The `off` choice is the only way to opt out entirely; it must not be described as "all review disabled".
- **Decision-gate record mode does not invent approval.** Under `record`, metrics and evidence are written but never used to claim pass. The real pass criterion remains the existing AAF + Payload + mechanical lint.
- **Decision-gate hard mode keeps Critical checks structural.** Under `hard`, `Critical Ambiguities = 0` and `Critical Unverified Assumptions = 0` are independently enforced per Gate Scope hat. The agent must not mark either as N/A while the gate is enabled.
- **Key-stage event gate (0e) 不退化为 preset 全局选择**：author 不得用一个 preset 全局开关替代 per-position 询问；不得把 0d 的 `hard/record/off` 字段复用为 0e 的 guard 选择；不得把 0e 的 `precheck_retry_budget` 与 `payload_consistency_retry_budget` 合并为一个 `retry_budget` 字段。任一违反 → review-only finding。
- **Key-stage event gate 不替 runtime 决策**：author 不得借 0e 段落新增 runtime 配置、计数器、恢复路径或绕过 guard 的替代行为；任何 retry / correction / blocked 描述只引用现有 `ralph-tools-emit.md` / `ralph-tools-recovery-directives.md` 等已注入 skill；不得写新 runtime 规则。
- **Key-stage event gate `neither` 不削弱既有 AAF / Payload Contract / mechanical lint**：选择 `neither` 只表示该位置不新增本次 guard；既有的 AAF 五问、Payload Contract 落盘判断、mechanical lint 仍然对该位置生效；亦不豁免 0d Gate Scope 的 metric 评估。
- **Do not duplicate** `ralph-tools*.md` content into instructions.

## Output Expectations

- Confirmed `Preset Intent Confirmation` (embedded at the beginning of `preset-author-notes.md` for new presets or material behavior changes)
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
