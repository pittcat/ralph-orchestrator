---
name: ralph-preset-review
description: Review Ralph preset YAML with agent-native AAF audit, mechanical lint, and structured preset-review-report.md. Use after drafting a preset or when validating builtin/local presets for per-hat feasibility, handoff closure, OPAC discipline, and P0/P1 issues. Produces findings with severity and confidence scores.
---

# Ralph Preset Review

Use this skill to **review** Ralph presets with **Agent 视角可行性（AAF）** — independent per-hat activation simulation, payload audit, and mechanical lint.

**Boundary:** Does not replace Rust `preset_lint` rules. Does not run full `./scripts/run-tests.sh` by default. User-only `.ralph/hats/` collection authoring (create / inspect / validate user hat workflows) and topology-debug / validate-routing workflows are not owned by either preset skill. For drafting, use `ralph-preset-author`.

**Deliverable:** Every review MUST write **`preset-review-report.md`** — not chat-only summaries.

## Use This Skill For

- Reviewing builtin or local presets before merge
- Finding P0/P1 issues: invisible inputs, broken handoffs, illegal commands, ledger reads, **invisible / fabricated / semantically unusable payload fields**
- Finding policy-check feedback adoption gaps: missing `field_docs`, unsafe `examples`, or emitter instructions that do not cite `ralph-tools-emit` Policy-Check feedback
- **Auditing `trigger_context` declarations** in `event_policy.schemas.<topic>`: hint conditions, label uniqueness, topology consumers, and the `instructions` ↔ `## TRIGGER CONTEXT` boundary (no duplicated hint conditions in prose)
- Running `ralph preset check --strict` and preset_lint nextest subsets
- Producing actionable remediation from AAF gaps + payload audit gaps
- **Key-hat scope + opt-in decision confidence gate (Reviewer side, capability-triggered, independently rebuilt)**: detecting hats with terminal authority, production mutation, phase branching, multi-hat aggregation, critical artifact production, or key handoff responsibility from the real topology; asking the operator separately to choose hard/record/off; recording the gate decision, scope delta vs author, metric evidence, and critical counts in the review report.
- **Artifact-First Handoff audit (R8–R12)**: 逐项验证重要信息是否落盘、路径对消费 hat 可见、消费动作闭环、生命周期责任完整、`field_docs` 与 `examples` 不诱导伪造;命中即按 `references/finding-rubric.md` 「Artifact-First Handoff finding_id」 表入主表(review-only,不进 `ralph preset check` JSON)

## Core Assumptions

- **Do not trust** `preset-author-notes.md` — rebuild AAF + payload audit independently; use notes only to flag author/review mismatches.
- **Simulate one hat activation at a time** — declare explicitly: "I am simulating hat X's activation."
- **Confidence ≥ 60** required for findings in the main table; below 60 → discard, re-investigate (max 2 rounds), or `Unverified Suspicions`.
- **Shape passing ≠ payload usable.** `ralph emit --schema` / `--policy-check` prove shape only. Field visibility, value source, identity, semantic sufficiency, downstream consumption are review's job.
- **Schema metadata is repair guidance, not truth.** `field_docs` / `examples` must match the payload audit, but they never replace visibility, value-source, or downstream semantic review.
- **Artifact-First Handoff 不可被 lint 直接验证**:`ralph preset check --strict` 与 `ralph emit --policy-check` 只能验 shape 与 topology ownership;路径可见性、消费动作闭环、生命周期责任、`field_docs` 是否诱导伪造——这些是 review-only 缺口,按 `references/finding-rubric.md`「Artifact-First Handoff finding_id」 表入主表,**不进** `ralph preset check` JSON。

## Workflow

0a. **Agent-skill audit gate（强制弹窗，默认跳过）** — 在 Workflow 第 1 步前必须弹出交互选择菜单（`AskUserQuestion` / `Question` 当平台支持；否则在 chat 里给编号选项并等回复），让 reviewer 决定是否同时审「注入给 agent 的 skill 文档」（`crates/ralph-core/data/*.md` / 外仓等价来源）：

   1. **仅审查 preset YAML（推荐，默认）** — 不审注入 skill；运行时间更短；适合 preset 形态稳定、只想确认 hat instructions + topology 的场景。
   2. **同时审查注入 skill 文档** — 怀疑 data 被改坏 / agent 看不懂 / 可读性下降时再选；跑 `references/agent-skill-audit.md` 规程。

   **默认行为**：未选 / 选推荐 → **不**审 data/*.md；报告「Executive Summary」必须写 `agent_skill_audit: skipped`。
   **选审后**：报告写 `agent_skill_audit: performed`，并按 `references/agent-skill-audit.md` 输出审计结论（含外仓「二进制内嵌」来源说明）；命中按 `references/finding-rubric.md`「Agent skill audit」段 `agent_skill.*` finding_id 入主表。

0b. **Discovery / 重审区分**：复跑已有 preset-review-report.md 时，先看上一份报告「Workflow 0a」行记录的 `agent_skill_audit` 字段；若上一份是 `performed`，本次默认仍是 `performed`（不要无声地降级回 skipped）。若选择降级，必须在本次报告显式说明「由 performed 降级到 skipped」。

0z. **Key-stage event gate discovery (capability-triggered, independent rebuild)**:
   - **Workflow ordering**: this step sits between Workflow 0a/0b and the topology-only discovery (Step 1). Per-Hat AAF may not run before both Workflow 0a/0b and 0z are settled.
   - **Independent scope (hard rule)**: rebuild the key-stage scope from real topology signals (`hats.*.triggers`, `hats.*.publishes`, `state_projection`, `event_policy`, `instructions` permissions). **Hard rule: 不得 继承 author notes / preset-author-notes.md 的 scope as evidence**. Author scope may appear in the report's `author_vs_reviewer_key_stage_delta` row as comparison data only — never as the reviewer's own scope.
   - **Capability triggers (same vocabulary as Author side, capability-triggered)**: terminal authority / production mutation / phase branching / multi-hat aggregation / artifact producer / key handoff. Plain passthrough / pure read-only hats are out of scope. Hard rule: 禁止 identifying key hats by preset-name or hat-name prefix; equal capability signals yield equal rules.
   - **Author notes are consumption evidence, not source of truth**: read `preset-author-notes.md` to extract the user-confirmed `key_stage` / `guard_selection` / `precheck_guard` / `precheck_retry_budget` / `payload_consistency_guard` / `payload_consistency_retry_budget` / `reason` / `confirmation_status` per key stage. The notes serve as the audit trail of the operator's choices; do not let them replace the reviewer's own capability rebuild.
   - **Skip-mode note**: when the author side skips Workflow 0e (e.g. narrow mechanical edit, no key stages identified), the reviewer records `key_stage_event_gate_audit: skipped` with a brief reason. No finding is raised solely due to skip-mode, but transparent skip-record is required.

1. Read **topology-only** fields from preset YAML: `event_loop`, `hats.*.triggers`, `hats.*.publishes`, `state_projection`, `event_policy` — **not** other hats' `instructions` yet.

2. Record `execution_mode`, hat count, preset path (`builtin:` vs file).

3. **Topology sketch** — event flow diagram (not prompt flow).

3a. **Single-chain-first audit** — mandatory:
   - Read `references/finding-rubric.md` 「Single-chain-first audit」段；按 `fallback_reaches_success_terminal` / `runtime_unit_loop_multiple_fact_sources` / `blocked_failed_promoted_to_pass` / `topic_multi_consumer` / `hidden_phase_decision` / `prompt_wall_serial_style` 六项逐项判定。
   - 任一命中 → 报告 P0（`fallback_reaches_success_terminal` / `runtime_unit_loop_multiple_fact_sources` / `blocked_failed_promoted_to_pass`）或 P1（其余）；confidence 起点 60。
   - 此审计**独立**于 mechanical lint 与 AAF 五问；可在 Per-Hat AAF Reviews 之前作为「Topology sketch 续」插入。

3a.5. **Capability-triggered audit (mandatory for capability-triggering presets)**：
   - Independently run `ralph capability inventory --format json`. For each capability whose `applies_when` matches this preset, verify the corresponding rule in `references/finding-rubric.md` / `agent-native-model.md` applies.
   - This audit is **capability-triggered**, not preset-name gated.

3a.6. **Key-hat scope and opt-in decision gate (Reviewer side, capability-triggered, independent from author)**:
   - **Workflow ordering**: this step sits between topology-only discovery (Step 1) and Per-Hat AAF (Step 4). Per-Hat AAF may not run before the Gate Scope table is drafted.
   - **Independent scope (hard rule)**: rebuild the Gate Scope from real topology signals (`hats.*.triggers`, `hats.*.publishes`, `state_projection`, `event_policy`, `instructions` permissions). **Hard rule: 不得 继承 author notes / preset-author-notes.md 的 scope as evidence**. Author scope may appear in the report's `author_vs_reviewer_scope_delta` row as comparison data only — never as the reviewer's own scope.
   - **Capability triggers (same vocabulary as Author side, capability-triggered)**: terminal authority / 终态 authority / 终态 决策 (success, failure, blocked terminal), production mutation / production code / 修改 生产 (code / test / config), phase branching / 重试 决策 / 阶段 分支 (stage transitions, retry, fix, rollback, stop), multi-hat aggregation / 跨 hat 汇总, artifact producer / 关键 artifact, key handoff / 关键 handoff. Plain passthrough / pure read-only hats are out of scope. Hard rule: 禁止 identifying key hats by preset-name or hat-name prefix; equal capability signals yield equal rules.
   - **Separate opt-in question (再次询问 hard rule)**: after the scope is drafted and **before** running Per-Hat AAF, ask the operator again with the same three-mode menu: 启用硬门禁 (hard) / 仅记录不阻塞 (record) / 不启用 (off). The reviewer must use the operator's answer for the reviewer side; it does not have to match the author's prior choice. Reviewer never inherits author's gate mode.
   - **Three-mode semantics**:
     - **hard** — each Gate Scope hat must satisfy `Confidence >= 85`, `Evidence Coverage >= 80`, `Verifiability >= 80`, `Impact Certainty >= 75`, AND `Critical Ambiguities = 0` AND `Critical Unverified Assumptions = 0`. Non-zero Critical counts or any sub-threshold metric blocks the report. The block is recorded as a P0 finding with confidence = max(60, score_used).
     - **record** — write the metrics, evidence, and Critical counts to the report; do not downgrade existing P0/P1 findings; never use the new metrics to claim pass on their own.
     - **off** — do not run this gate; existing AAF / Payload Audit / mechanical lint and finding confidence calibration remain unchanged.
   - **Critical checks are structural**: under hard or record, every Gate Scope hat must carry `Critical Ambiguities` and `Critical Unverified Assumptions` rows. The agent is not allowed to mark them N/A while the gate is enabled; `off` is the only way to skip them entirely.
   - **Author/reviewer scope delta**: if the reviewer identifies a hat that the author notes omitted from scope (for example, a terminal-authority hat that author skipped), record a `scope_gap` entry in `preset-review-report.md` Executive Summary and an additional P0 finding keyed on the missing hat's capability class.
   - **Identity rule (capability-triggered, 不得 name-prefix)**: this section is a hard rule, capability-triggered identification. Reviewer must never identify a key hat by preset-name or hat-name prefix; equal capabilities must yield equal rules regardless of preset or hat name.

3a.7. **Key-stage event gate audit (capability-triggered, independent from author)**:
   - **Workflow ordering**: this step sits between Workflow 3a.6 (Decision-gate) and the per-hat AAF (Step 4). Per-Hat AAF may not run before both 3a.6 and 3a.7 are settled.
   - **Independent scope (hard rule)**: rebuild the key stages from real topology signals (`hats.*.triggers`, `hats.*.publishes`, `state_projection`, `event_policy`, `instructions` permissions). **Hard rule: 不得 继承 author notes / preset-author-notes.md 的 scope as evidence**. Author scope may appear in the report's `author_vs_reviewer_key_stage_delta` row as comparison data only — never as the reviewer's own scope.
   - **Author notes as audit trail**: read `preset-author-notes.md` to extract the user-confirmed `key_stage` / `guard_selection` / `precheck_guard` / `precheck_retry_budget` / `payload_consistency_guard` / `payload_consistency_retry_budget` / `reason` / `confirmation_status` per key stage. Notes serve as evidence of the operator's choices; do not let them replace the reviewer's capability rebuild.
   - **Per-stage audit items**:
     - `key_stage` 字段是否齐全（每个 reviewer 独立识别的关键位置都有 name + 上下文）。
     - `guard_selection` ∈ {`precheck`, `payload_consistency`, `both`, `neither`}；选择 `neither` 必须有 ≤80 字 `reason` 且理由指向恢复 / 审计 / 下游依赖等具体风险。
     - `precheck_guard=true` ⇔ `guard_selection ∈ {precheck, both}`；`payload_consistency_guard=true` ⇔ `guard_selection ∈ {payload_consistency, both}`；任一字段语义不一致 → `preset.key_stage_event_gate_field_reuse`。
     - `precheck_guard=true` ⇒ `precheck_retry_budget ∈ {3, 2, 1}`；`precheck_guard=false` ⇒ `precheck_retry_budget` 为 `null`；同理 `payload_consistency_*`。
     - 两 budget 不共享总预算 / 计数器 / exhaustion state；合并为一个 `retry_budget` 或共享 → `preset.key_stage_event_gate_shared_budget`。
     - `confirmation_status` 必须为 `confirmed`；`pending` / `rejected` 但 author 已继续生成依赖 YAML / schema → `preset.key_stage_event_gate_pending_status`。
     - notes 记录的 guard 选择与 YAML 实际 `event_loop.precheck.rules` / `event_policy.payload_consistency.rules` / `event_policy.schemas.<topic>.field_docs` 实际声明是否一致；不一致 → `preset.key_stage_event_gate_notes_preset_diverge`。
     - author 是否用单个 preset 全局开关替代 per-position 询问 → `preset.key_stage_event_gate_single_combined_choice`。
     - author 是否借 0e 段落新增 runtime 配置 / 计数器 / 恢复路径 / 绕过 guard 替代行为 → `preset.key_stage_event_gate_unsupported_runtime_rule`。
   - **Capability-triggered invariant**: 同 0d 的 hard rule，禁止按 preset-name / hat-name prefix 识别关键位置；equal capability signals yield equal rules.
   - **`neither` 不削弱既有 AAF / Payload Contract / mechanical lint**：选择 `neither` 仅表示该位置不新增本次 guard；既有的 AAF 五问、Payload Contract 落盘判断、mechanical lint 仍生效；reviewer 不得因 `neither` 而跳过既有审查。

3b. **CE pipeline 评审附加检查** — 当且仅当被审 preset 名称以 `ce-executor-pipeline` 开头时执行（其它 preset 不强制）：
   1. **mandatory review artifact fail-close** — reviewer-synthesizer hat 的 instructions 必须定义 6 个 mandatory dimension finding products 的完整性校验（存在 / 可读 / 格式合法 / dimension 字段匹配 / count 一致），任一失败时禁止 synthesized verdict、必须发 `review.artifact.blocked` 并写阻塞 audit block；topic ownership / deny rules / required fields 由 preset 显式声明。缺任一项 → P0。
   2. **reporter 是否从 trigger / bundle 获取跨 hat 状态** — reporter hat 的 instructions 必须禁止 `ralph events --events-source main` 用作业务输入；6 个 reporter trigger topic 的 schema 必须包含 `report_input_file` required field + field_docs 三段。违反 → P0。
   3. **required-event-to-completion 窄例外** — 若 hat 是 preset 的 sole 收尾 hat（典型 reporter / alignment），preset `event_loop.required_events[]` 配对声明、`event_loop.completion_promise` 非空、该 hat `publishes` 同时包含二者且 `terminal_events` 匹配，**不**报 multi-emit P0。其它任何 hat 一律不享受该例外。判定按 `finding-rubric.md`「required-event-to-completion 窄例外」复核段六条全检。
   4. **CE 新增合同的 field_docs 三段** — reporter 各入口的 `report_input_file` 与 stabilization inline schema 必须具备完整 meaning / source / fill_rule。`crates/ralph-cli/src/presets.rs::test_ce_*` 结构化测试是机械落地；review 端只需复跑该测试，不得借本检查强制迁移无关历史字段。

3c. **operator fixture 自检** — 对以下 fixtures 各跑一遍 mechanical lint 与 AAF，验证 `finding-rubric.md` 的窄例外与新 finding 能区分正负：
   - `fixtures/aaf-review-required-event-completion-fixture.yml`（正）— 期望：multi-emit P0 不出现。
   - `fixtures/aaf-review-negative-fixture.yml`（负）— 期望：multi-emit P0 + artifact-read-ledger P0 同时出现。
   - `fixtures/aaf-artifact-first-negative-fixture.yml`（负）— 期望：artifact-first 系列 P0 出现。
   - `fixtures/aaf-runtime-unit-loop.yml`（负）— 期望：unit-loop 相关 P0 出现。
   - `fixtures/aaf-fallback-success-terminal.yml`（负）— 期望：fallback-reaches-success P0 出现。
   - `fixtures/payload-audit-negative-fixture.yml`（负）— 期望：payload 系列 P0 / P1 出现。
   - `fixtures/trigger-context-negative-fixture.yml`（负）— 期望：trigger-context 系列 P0 出现。
   - `fixtures/aaf-wave-capability-negative-fixture.yml`（负）— 期望：Wave capability audit 系列 P0 出现。
   - `fixtures/aaf-supervisor-capability-negative-fixture.yml`（负）— 期望：Supervisor capability audit 系列 P0 出现。

3d. **Wave capability audit** — **capability-triggered**, **不**按 preset 名称点名门控：
   1. **检测顺序**：先读 `references/author-checklist.md` Intent Confirmation 的 `execution_model` 字段 → 再扫 YAML `event_loop.supervisor.enabled` 与 hat `instructions` / `publishes` 中是否出现 `ralph wave emit` / `ralph wave verify` / `## WAVE CONTEXT` 字样。
   2. **触发条件**：`execution_model ∈ {wave, supervisor+wave}` **或** 上述命令字样出现在非 builtin-cli 位置。**未触发**：review 把本步记为 `N/A`（不假装已审）。
   3. **判定**：按 `references/finding-rubric.md`「Wave capability audit」段逐项查 `preset.wave_worker_calls_wave_emit` / `preset.wave_missing_verify_before_emit` / `preset.wave_confirm_uses_hat_channel` / `preset.wave_agent_emits_coordination_topic`；命中 → 主表，默认 P0。
   4. **触发条件不得按 preset 名称**：检测顺序中**禁止**写「`name starts with ...`」之类的名缀门控；详见 `references/agent-native-model.md`「执行模型」段的硬约束。

3e. **Supervisor capability audit** — **capability-triggered**, **不**按 preset 名称点名门控：
   1. **检测顺序**：先读 Intent.execution_model → 再扫 YAML `event_loop.supervisor.enabled` 与 hat `instructions` 是否引用 `.ralph/supervisor.db` / 协调 topic。
   2. **触发条件**：`execution_model ∈ {supervisor, supervisor+wave}` **或** `event_loop.supervisor.enabled: true` **或** 上述命令 / 路径字样出现。**未触发**：N/A。
   3. **判定**：按 `references/finding-rubric.md`「Supervisor capability audit」段逐项查 `preset.supervisor_requires_isolated` / `preset.supervisor_hat_publishes_coord_topic` / `preset.supervisor_unit_state_not_via_task_api` / `preset.artifact_uses_internal_ledger` / `preset.execution_model_intent_mismatch`；命中 → 主表，默认 P0。
   4. **与 3b 既有 CE pipeline 检查的关系**：3e **不**修改 3b 的「仅 `ce-executor-pipeline*` preset 触发」语义；3e 是 capability-triggered 的新通用审计，**新增** 5 条 finding_id（含 review-only 软性），3b 既有 4 条 review-only 软性 finding 保留不动。

4. **Per-hat AAF review** (mandatory — one hat at a time, strict sequence per hat):
   - Declare: simulating hat `<id>` activation.
   - For the current hat, load **only** that hat's `instructions` (or `ralph hats show -H <path> <id>`). Do not use another hat's private instructions as evidence for this hat's visible context.
   - Run the **activation dry-run sequence** in order:
     1. **Trigger received** — what event triggered this hat? payload fields visible?
     2. **Visible context** — go through isolated prompt 栈 (`## HAT IDENTITY` / `## ORCHESTRATOR CONTEXT` / `instructions` / injected skills). What can the agent actually see? **Visible context MUST be backed by `ralph -c <preset> inspect prompt --hat <id> --format json`**（共享规程 `references/prompt-visibility.md`）的 `auto_inject` / `on_demand` / `block_titles` 输出；禁止凭记忆说「该 skill 一定会注入」。命中 on-demand skill 被 instructions 当成 auto-inject 时入 `agent_skill.inject_claim_false` finding。
     3. **Command plan** — which `ralph` commands in which order? OPAC 四阶段？
     4. **Payload construction** — for each emit topic, can every field be sourced from visible context?
     5. **Emit precheck** — `--policy-check` / `--triggered` ownership / policy-check feedback handling / single event budget / terminal ordering?
     6. **Handoff** — does any emitted field need to reach another hat? Does projection make it observable?
   - Fill AAF 五问表 + **Payload Audit 表** per emit topic (see `references/agent-native-model.md`).
   - For emitter hats, verify the instructions explicitly require `ralph tools skill load ralph-tools-emit` in the activation and stop on load failure, then cite `ralph-tools-emit` Policy-Check feedback when they mention payload construction, required fields, field shape, `ralph emit`, or `ralph wave emit`. Missing the explicit load is review-only `preset.instructions_emit_skill_load_missing` (P1, confidence 85).
   - **Artifact-First 单 hat 审核(逐 hat 必做)**:在 AAF 五问表中加一列「Artifact 落盘 / 消费」或单列附注。
     - Q2 / Q3: 验证 consumer hat instructions 是否要求「从当前 hat 可见输入取得路径并读取 artifact」;producer hat instructions 是否要求「先写 artifact 再 emit」。
     - Q4: 验证 `artifact 落盘` 列已填(必填 / 可选 / 不需要 / 不落盘+理由);不落盘例外必须说明恢复 / 审计 / 下游依赖。
     - Q5: 验证 artifact 路径形成完整链路(emit 字段 → projection → 下游 Q2 Observe 可见路径)。
     - 不落盘例外无理由 / 仅有「字符很少」 → `preset.artifact_first_exemption_unjustified` finding。
     - producer / consumer 缺顺序约束 → 严重度按 `references/finding-rubric.md`「Artifact-First Handoff → Severity」表入栏。
   - Compare to `instructions:` → candidate findings.
   - **模板文件机制轻量检查（review-only，不新增 finding_id）**：若某 hat 的 `instructions:` 内联了大段固定格式文档（报告模板、计划模板、验收清单、SOP 步骤等，通常 >80 行或占 instructions 一半以上），review 应在 Per-Hat AAF Reviews 的「deltas vs instructions」或 Executive Summary 中备注「该 hat 可考虑模板文件机制（`presets/templates/` + `ralph preset materialize-artifacts`）压缩上下文」。验证方式：用 `ralph -c <preset>.yml inspect prompt --hat <hat_id> --format json --full` 查看 `prompt_body` 中 `### 1. EXECUTE` 到 `### 2. VERIFY` 之间的 instructions 实际长度；若 instructions 引用了 `materialize-artifacts` 且长度 <80 行，说明模板机制已生效。此检查不阻塞 review，仅作为优化建议记录；若 author notes 中已说明「未采用模板文件机制」及理由，则跳过。
   - Optional: read `preset-author-notes.md` for that hat only after your table is drafted.

5. **Payload Audit table** (mandatory — aggregate it under the Per-Hat AAF Reviews section, one row per material emit field):
   - Columns: topic | field | value source | visibility evidence | identity check | semantic downstream use | schema metadata | policy-check repair surface | verdict | repair surface.
   - Cover every emit topic that drives a downstream hat decision or carries runtime identity.
   - For each required handoff / identity / verdict / count / path / reason field, inspect `event_policy.schemas.<topic>.field_docs.<field>`:
     - `meaning` must describe the field in agent-facing terms.
     - `source` must match a visible value source from the Payload Audit row.
     - `fill_rule` must tell the agent how to repair the field without inventing business facts.
   - **Artifact-First `field_docs` 审核点(每 path 字段必查)**:
     - `meaning` 必须明确「该路径是 artifact 落盘点,值为相对 `.ralph/` 的路径」,不是笼统的「handoff path」。
     - `source` 必须指向当前 hat 可见输入(trigger payload 字段 / `state_projection.actions` 投影后的字段 / 本 hat work 输出);不得指向其它 hat 内部状态 / runtime internal ledger。
     - `fill_rule` 必须说明如何在路径未提供时计算或拒绝,不得诱导 agent 伪造路径。
     - `examples[]` 用结构占位(`.ralph/<plan>/<unit>/<file>.md`),不固定具体业务文件名。
     - 任一项缺失或不一致 → `preset.artifact_first_field_docs_missing`(P1);若诱导伪造 → 升 P0。
   - Inspect `examples[]`: examples may show shape, but must not encode fake business conclusions that an agent could copy as facts.

   - **Instructions ↔ schema required-fields SSOT 对账（强制）**：对每个 emitter hat 的每个 `ralph emit <topic>` 示例，提取示例 payload 中实际出现的字段集合，与同一 preset 的 `event_policy.schemas.<topic>.required_fields` 做集合对账；再检查示例中的每个占位值是否能从当前 trigger / 本 hat 产物获得。缺字段、字段名漂移或示例指向错误上游 topic 时，必须入主表 `preset.instructions_schema_required_fields_drift`（P0，confidence 95，Q4，category policy-feedback），不得以“agent 可以自己补字段”降级。对账证据必须同时引用 schema 行和 instructions 行。该检查是本 skill 的 SSOT 审计，不要求锁定完整 prompt 文案。

5a. **Artifact-First 跨 hat 独立审核(独立于第 4 / 5 步 AAF,必做)**:
   - **路径可见性闭环**：每条 emit topic 携带的 path 字段,沿「emit → projection → 下游 hat 可见输入」逐跳检查;任一节点路径不可见 → `preset.artifact_path_not_in_visible_context`(P0)。
   - **消费动作闭环**：下游 consumer hat 的 instructions 必须**显式要求**读路径,且读盘后做验收 / 确认(文件存在、可解析、足以支撑本 hat Q1)。判定规程:
     1. instructions 未要求读路径 → `preset.artifact_no_consumer_declared`(P1)。
     2. 仅说「看 payload 摘要 / 有 path 即可」而无读盘命令 → `preset.artifact_first_passed_on_path_presence`(P1)。
     3. 要求读盘但无「验收 / 确认内容可用」语句 → 仍按 `preset.artifact_no_consumer_declared`(P1);R10 消费确认未闭环。
   - **内容充分性闭环(R8)**：假设 consumer 已按路径读盘,检查 artifact 约定内容是否足以支撑该 hat Q1(完整结果 / 证据 / 未解决问题 / 可恢复进度)。路径存在但内容设计不足以恢复或继续决策 → `preset.artifact_content_insufficient_for_decision`(P0 阻塞下游 / 否则 P1)。**不得**只凭「payload 有 path」或「instructions 写了 cat」放行。
   - **生命周期闭环**：跨多 hat 的中间 / 汇总文件必须显式声明 owner / reader / retention / cleanup 任一方;缺失 → `preset.artifact_no_lifecycle_owner`(P1,blast radius 大则 P0)。
   - **payload 内容闭环**：payload 携带完整结果 / 长内容 / 跨 hat 摘要(>200 字符或可恢复) → `preset.payload_carries_full_content`(P0);sub-agent 完整结果通过 hat 长消息返回(未落盘) → `preset.subagent_result_returned_only_in_message`(P0)。
   - **internal-ledger-as-artifact**:任何 hat instructions 要求把 `.ralph/events.jsonl` / `.ralph/loops.json` / `.ralph/supervisor.db` 当业务 artifact 接口(写或读) → `preset.artifact_uses_internal_ledger`(P0)。
   - **preset 自身 ownership**:preset 文本被描述为 artifact 创建者(实际由 hat / sub-agent 在 activation 创建) → `preset.artifact_described_as_preset_owned`(P0)。
   - 详细 default severity / confidence / aaf_question / category 见 `references/finding-rubric.md`「Artifact-First Handoff finding_id」表。

6. **Handoff Audit table:** for each edge A→B: A.Q4 emit fields | projection action | downstream Q2 Observe | verdict | finding id.
   - Closed handoff = upstream emit field is in projection → downstream Q2 Observe command sees it.
   - Open handoff = P0 unless runtime evidence proves otherwise.
   - **Artifact-First handoff 子集**：对每条 artifact handoff 边额外列「artifact 路径 → projection → 下游 Q2 Observe 实际命令 → 消费确认 → 内容充分性」。
     - **消费确认(R10)判定**：consumer instructions 须同时具备 (a) 从可见输入取路径、(b) `cat`/`Read` 读盘、(c) 读后验收(存在/可解析/足以决策)。缺 (a)(b) → `preset.artifact_no_consumer_declared`(P1);有路径无读盘 → `preset.artifact_first_passed_on_path_presence`(P1);路径对 consumer 不可见 → `preset.artifact_path_not_in_visible_context`(P0)。
     - **内容充分性(R8)判定**：在消费确认通过后,检查 artifact 约定是否覆盖 consumer Q1 所需事实;不足 → `preset.artifact_content_insufficient_for_decision`。

7. **Mechanical lint** — run commands in `references/commands.md`. Map JSON `id` values (`lint.preset.*`) via `references/finding-rubric.md`. **Continue AAF and payload audit even if lint fails**; note failure in Executive Summary.
   - If `preset.instructions_emit_feedback_skill_reference_missing` appears, treat it as a real adoption gap unless the hat does not construct payloads. The repair surface is the relevant hat `instructions:` plus, if needed, `event_policy.schemas.<topic>.field_docs`.
   - **Trigger Context lint IDs** (`preset.trigger_context_unknown_field` / `preset.trigger_context_unsupported_predicate` / `preset.trigger_context_value_shape` / `preset.trigger_context_duplicate_label` / `preset.trigger_context_no_consumer`) 是机械 lint 抓得到的 shape / 拓扑错误。命中即按 `references/finding-rubric.md` 默认 severity 与 confidence 入表，**不要重写为软性 AAF**。
   - **Payload Consistency lint IDs** (`preset.payload_consistency_duplicate_id` / `preset.payload_consistency_unknown_topic` / `preset.payload_consistency_unknown_field`) 来自 `event_loop.event_policy.payload_consistency.rules` 的静态校验：id 唯一性 / topic 在 schemas 内 / `when` 引用的 field 在该 topic schema 字段并集内。命中即按 `references/finding-rubric.md` 默认 severity 与 confidence 入表，与 `trigger_context_*` 同样按机械 lint 看待。
   - **lint 不抓的 review-only 项**：hint `guidance` 是否与下游 hat 实际决策分支语义一致、`instructions` 是否仍在复制 hint 条件值（与 `## TRIGGER CONTEXT` 双写漂移）、`summary_fields` 引用字段是否在 trigger payload 中真实可见——这些由本 skill 第 4 / 5 步的 AAF 与 Payload Audit 独立审。
   - **Artifact-First 机械 lint 范围**：当前 `ralph preset check --strict` 不直接产出 artifact-first ID（详见 `references/finding-rubric.md`「Artifact-First Handoff finding_id」表 + 「备注：lint vs review-only」段）。Mechanical Lint Results 段需显式注明「artifact-first 项：review-only，不进 lint JSON」；命中按 review-only ID + default severity + default confidence 入主表。

8. **Confidence calibration** (`references/finding-rubric.md`):
   - Lint Error → 95; Warn → 85
   - Soft AAF / payload audit: start ≤ 50; verify before ≥ 60
   - P0 with confidence < 60 cannot be reported as P0 until verified

9. **Write report** — all eight sections below.

## Report Path

- Default: `.ralph/reviews/<preset-basename>-<YYYY-MM-DD>.md` (gitignored)
- Optional PR copy: `<preset-dir>/preset-review-report.md`

## Report Structure (fixed)

1. **Executive Summary** — mode, hat count, P0/P1/P2 counts (confidence ≥ 60 only), lint pass/fail, **payload audit pass/fail**, policy-check feedback adoption pass/fail, **artifact-first handoff audit pass/fail** (含 review-only finding 计数,例如 `preset.artifact_path_not_in_visible_context` / `preset.artifact_no_lifecycle_owner` / `preset.payload_carries_full_content` 等), **`agent_skill_audit: skipped|performed`**（Workflow 0a 选定值；若 `performed` 则附「来源: 本仓 crates/.../data/*.md / 外仓二进制内嵌」），**`prompt_visibility_evidence: <hat_id>=inspect-prompt-json`**（Per-Hat AAF Visible context 引用证据）, **`decision_gate: off|record|hard`**（Workflow 3a.6 选定值；独立于 author 的 scope 与 metric 评估——`off` 表示 reviewer 选择了不启用此 gate,`record`/`hard` 表示 operator 选定的运行模式;`decision_gate` 字段缺失表示 Workflow 3a.6 未触发,与 `agent_skill_audit` 是字段词汇完全独立的两个字段：`decision_gate: off|record|hard`（Workflow 3a.6，三字面量与 `KEY_HAT_GATE_MODES = ("hard", "record", "off")` 一致),`agent_skill_audit: skipped|performed`（Workflow 0a）。两者值集合 disjoint,不得复用 `skipped` 作为 `decision_gate` 的同义词）, **`decision_gate_scope: <hat_id>=capability_trigger`**（per-hat trigger 理由;author/reviewer scope delta 与 scope_gap 单独记录）, **`decision_gate_critical_counts: <hat_id>={critical_ambiguities:int,critical_unverified_assumptions:int}`**（当 mode=record 时仍必须存在;mode=hard 时必须均为 0,否则该 hat P0 blocking）,
**`key_stage_event_gate_audit: skipped|performed`**（Workflow 0z 选定值；字段词汇与 `agent_skill_audit` / `decision_gate` disjoint；`performed` 模式下还要附 `key_stage_event_gate_scope: <key_stage>=capability_trigger` 与 `key_stage_event_gate_per_stage: <key_stage>={precheck_guard:bool,precheck_retry_budget:int|null,payload_consistency_guard:bool,payload_consistency_retry_budget:int|null,confirmation_status:confirmed|pending|rejected,reason:<≤80 字>}`）,
**`author_vs_reviewer_key_stage_delta`**：reviewer 独立识别的关键位置与 author notes 记录的关键位置比对差异（命中 `preset.key_stage_event_gate_notes_preset_diverge` / `preset.key_stage_event_gate_missing_selection` 等 review-only finding 时必须列出）
2. **Findings Table** — sort P0 → P1 → P2; columns: id, severity, confidence, category, aaf_question, hat, problem one-liner
3. **Topology** — event flow
4. **Per-Hat AAF Reviews** — full 五问表 per hat + deltas vs instructions + mandatory Payload Audit Table rows (topic / field / source / visibility / identity / downstream / schema metadata / policy-check repair / **artifact 落盘(必填 / 可选 / 不落盘+理由)** / verdict / fix)
5. **Handoff Audit Table** — closed / open per edge; artifact handoff 子集额外列「artifact 路径 → projection → 下游 Q2 Observe 实际命令 → 消费确认」
6. **Mechanical Lint Results** — commands + output excerpt + 「artifact-first 项：review-only，不进 lint JSON」 显式说明
7. **Remediation Plan** — ordered by **runtime unblock order**, not by file or discovery order; each item names repair surface (instructions / publishes / payload schema / state_projection / event_policy / author notes / fixture / **业务 artifact 路径声明 / `field_docs.<path_field>` / lifecycle owner**)
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

See `references/finding-rubric.md` for `finding_id` defaults and the new **Payload Audit → Severity** table. **Payload-content and invisible-input findings outrank style.** Artifact-First Handoff finding_id 默认见 `references/finding-rubric.md`「Artifact-First Handoff finding_id」表(全部 review-only,不进 `ralph preset check` JSON)。

**Artifact-First handoff example rows**(命中按 review-only ID 入主表,confidence 起点见 `references/finding-rubric.md`):

```markdown
| F-101 | P0 | 90 | payload-content | Q4 | executor | hats.executor.publishes[work.done].payload.report_body | sub-agent 完整结果通过 `report_body` 内联在 payload,未在 `.ralph/reports/<unit>.md` 落盘 | emitter instructions 改为「先写 `.ralph/reports/<unit>.md`」,emit 只携带 `report_path` + 短摘要;命中 `preset.payload_carries_full_content` / `preset.subagent_result_returned_only_in_message` |

| F-102 | P1 | 80 | payload-content | Q5 | reviewer | hats.reviewer.instructions | 上游 emit 携带 `executor_head_report_path` 且对 reviewer 可见,但 instructions 明确忽略读盘、只看 inline `report_body` | instructions 显式 `cat <executor_head_report_path>` 并验收后再决策;命中 `preset.artifact_no_consumer_declared` + `preset.artifact_first_passed_on_path_presence` |

| F-103 | P0 | 95 | visibility | Q3 | planner | hats.planner.instructions | planner 把阶段状态写到 `.ralph/supervisor.db`(runtime internal ledger) | 改写为 `.ralph/plans/<plan>.md`,emit 只携带路径与摘要;命中 `preset.artifact_uses_internal_ledger` |

| F-104 | P1 | 80 | state | Q5 | orchestrator | hats.orchestrator.publishes[loop.summary].payload.intermediate_metrics_path | 中间文件由多 hat 持续追加,但 preset 未声明消费方 / 保留 / 清理责任 | 在 preset 顶部或 orchestrator instructions 声明 owner + retention + cleanup;命中 `preset.artifact_no_lifecycle_owner` |

| F-105 | P0 | 90 | visibility | Q2 | summarizer | hats.summarizer.instructions | instructions 要求读 `canonical_bundle_path`,但 trigger / projection 从未提供该字段 | 上游 emit + projection 暴露路径,或去掉对该不可见字段的依赖;命中 `preset.artifact_path_not_in_visible_context` |
```

## Guardrails (review skill itself)

- Never pass/fail based on "agent read the whole preset."
- Isolated P0 evidence must be **hat-visible** unless proving runtime injection.
- Reject user request for chat-only review — write the report file.
- Reject "handoff unclear" / "payload looks weak" findings that don't name field + source + fix — rewrite with evidence or move to `Unverified Suspicions`.
- **Artifact-First 拒绝放行**：禁止因「payload 含路径字段」就判定 handoff 闭环。必须同时验证 ① 路径在 consumer hat 可见输入中、② consumer instructions 显式要求读文件并验收、③ artifact 内容足以支撑 consumer Q1、④ `field_docs.<path_field>.meaning` 明确落盘点语义、⑤ 生命周期责任(consume / retain / cleanup)有声明。任一缺失 → 按 `preset.artifact_path_not_in_visible_context` / `preset.artifact_no_consumer_declared` / `preset.artifact_first_passed_on_path_presence` / `preset.artifact_content_insufficient_for_decision` / `preset.artifact_no_lifecycle_owner` 入主表。
- **Artifact-First 不接受「字符很少」作为唯一例外理由**：例外必须满足短暂 + 短小 + 无需恢复,并标注恢复 / 审计 / 下游依赖依据。理由不充分 → `preset.artifact_first_exemption_unjustified` finding。
- **Decision-gate scope is capability-triggered, reviewer-independence is a hard rule.** Workflow 3a.6 forbids inheriting or trusting author scope; equal capabilities yield equal rules regardless of preset or hat name. Hard rule: 不得 key-hat identification by preset-name or hat-name prefix.
- **Decision-gate critical counts are structural.** When the gate is enabled, every Gate Scope hat carries `Critical Ambiguities` and `Critical Unverified Assumptions` rows; the agent must not mark either as N/A while enabled. `off` mode is the only way to skip them entirely, and even then it does not weaken existing AAF / Payload / lint.
- **Decision-gate does not downgrade prior P0/P1.** Under `record`, metrics and evidence are written but never used to claim pass on their own; existing P0/P1 findings keep their severity and confidence.
- **Key-stage event gate scope is capability-triggered, reviewer-independence is a hard rule.** Workflow 3a.7 forbids inheriting or trusting author scope; equal capabilities yield equal rules regardless of preset or hat name. Hard rule: 不得 key-stage identification by preset-name or hat-name prefix.
- **Key-stage event gate fields are disjoint from Gate Scope.** The 0e `guard_selection` / `precheck_guard` / `payload_consistency_guard` / `precheck_retry_budget` / `payload_consistency_retry_budget` fields are not interchangeable with the 0d `hard/record/off` mode and the Gate Scope metric rows; reviewer must not merge them. Violation → `preset.key_stage_event_gate_field_reuse`.
- **Key-stage event gate does not downgrade prior P0/P1.** Selection `neither` for a key stage does not weaken existing AAF / Payload / mechanical lint findings; reviewer must still surface unrelated issues.
- **Key-stage event gate review-only findings** are not part of `ralph preset check` JSON; they appear in the report's main table under the default severity / confidence in `references/finding-rubric.md` 「Key-stage event gate」段. The Mechanical Lint Results section must explicitly note:「key-stage event gate 项：review-only，不进 lint JSON」.

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

- AAF model + payload audit(含「Artifact-First Handoff 模型 / 状态传递 / 边界 / 知识分层 / Review 必须独立重做的 artifact-first 检查」): `references/agent-native-model.md`
- Commands: `references/commands.md`
- Severity / confidence / finding_id(含 Payload Audit → Severity、Artifact-First Handoff → Severity、Artifact-First Handoff finding_id、Artifact-First Handoff `field_docs` 审核点): `references/finding-rubric.md`
- Author checklist + Payload Contract template(含 Artifact-First topic 判别、Artifact-First 单 hat 视角审核项、Hard questions — Artifact-First Handoff): `references/author-checklist.md`
- Topology context: `references/patterns.md`
- 验收 fixture: `fixtures/aaf-artifact-first-negative-fixture.yml`(覆盖 AE1-AE5 + field_docs / ownership / path-invisible 分支,可用于 review 流程自检)
