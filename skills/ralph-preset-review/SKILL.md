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
- **Auditing `trigger_context` declarations** in `event_policy.schemas.<topic>`: hint conditions, label uniqueness, topology consumers, and the `instructions` ↔ `## TRIGGER CONTEXT` boundary (no duplicated hint conditions in prose)
- Running `ralph preset check --strict` and preset_lint nextest subsets
- Producing actionable remediation from AAF gaps + payload audit gaps
- **Artifact-First Handoff audit (R8–R12, per `docs/plans/2026-07-16-003-feat-preset-artifact-first-handoffs-plan.md`)**: 逐项验证重要信息是否落盘、路径对消费 hat 可见、消费动作闭环、生命周期责任完整、`field_docs` 与 `examples` 不诱导伪造;命中即按 `references/finding-rubric.md` 「Artifact-First Handoff finding_id」 表入主表(review-only,不进 `ralph preset check` JSON)

## Core Assumptions

- **Do not trust** `preset-author-notes.md` — rebuild AAF + payload audit independently; use notes only to flag author/review mismatches.
- **Simulate one hat activation at a time** — declare explicitly: "I am simulating hat X's activation."
- **Confidence ≥ 60** required for findings in the main table; below 60 → discard, re-investigate (max 2 rounds), or `Unverified Suspicions`.
- **Shape passing ≠ payload usable.** `ralph emit --schema` / `--policy-check` prove shape only. Field visibility, value source, identity, semantic sufficiency, downstream consumption are review's job.
- **Schema metadata is repair guidance, not truth.** `field_docs` / `examples` must match the payload audit, but they never replace visibility, value-source, or downstream semantic review.
- **Artifact-First Handoff 不可被 lint 直接验证**:`ralph preset check --strict` 与 `ralph emit --policy-check` 只能验 shape 与 topology ownership;路径可见性、消费动作闭环、生命周期责任、`field_docs` 是否诱导伪造——这些是 review-only 缺口,按 `references/finding-rubric.md`「Artifact-First Handoff finding_id」 表入主表,**不进** `ralph preset check` JSON。

## Workflow

1. Read **topology-only** fields from preset YAML: `event_loop`, `hats.*.triggers`, `hats.*.publishes`, `state_projection`, `event_policy` — **not** other hats' `instructions` yet.

2. Record `execution_mode`, hat count, preset path (`builtin:` vs file).

3. **Topology sketch** — event flow diagram (not prompt flow).

3a. **Single-chain-first audit (2026-07-07-006 Unit 6)** — mandatory:
   - Read `references/finding-rubric.md` 「Single-chain-first audit」段；按 `fallback_reaches_success_terminal` / `runtime_unit_loop_multiple_fact_sources` / `blocked_failed_promoted_to_pass` / `topic_multi_consumer` / `hidden_phase_decision` / `prompt_wall_serial_style` 六项逐项判定。
   - 任一命中 → 报告 P0（`fallback_reaches_success_terminal` / `runtime_unit_loop_multiple_fact_sources` / `blocked_failed_promoted_to_pass`）或 P1（其余）；confidence 起点 60。
   - 此审计**独立**于 mechanical lint 与 AAF 五问；可在 Per-Hat AAF Reviews 之前作为「Topology sketch 续」插入。

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
   - **Artifact-First 单 hat 审核(逐 hat 必做)**:在 AAF 五问表中加一列「Artifact 落盘 / 消费」或单列附注。
     - Q2 / Q3: 验证 consumer hat instructions 是否要求「从当前 hat 可见输入取得路径并读取 artifact」;producer hat instructions 是否要求「先写 artifact 再 emit」。
     - Q4: 验证 `artifact 落盘` 列已填(必填 / 可选 / 不需要 / 不落盘+理由);不落盘例外必须说明恢复 / 审计 / 下游依赖。
     - Q5: 验证 artifact 路径形成完整链路(emit 字段 → projection → 下游 Q2 Observe 可见路径)。
     - 不落盘例外无理由 / 仅有「字符很少」 → `preset.artifact_first_exemption_unjustified` finding。
     - producer / consumer 缺顺序约束 → 严重度按 `references/finding-rubric.md`「Artifact-First Handoff → Severity」表入栏。
   - Compare to `instructions:` → candidate findings.
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

5a. **Artifact-First 跨 hat 独立审核(独立于第 4 / 5 步 AAF,2026-07-16-003 必做)**:
   - **路径可见性闭环**：每条 emit topic 携带的 path 字段,沿「emit → projection → 下游 hat 可见输入」逐跳检查;任一节点路径不可见 → `preset.artifact_path_not_in_visible_context`(P0)。
   - **消费动作闭环**：下游 consumer hat 的 instructions 必须**显式要求**读路径(若仅说「看 payload 中的摘要」,路径存在但未消费 → `preset.artifact_first_passed_on_path_presence` P1 / `preset.artifact_no_consumer_declared` P1)。
   - **生命周期闭环**：跨多 hat 的中间 / 汇总文件必须显式声明 owner / reader / retention / cleanup 任一方;缺失 → `preset.artifact_no_lifecycle_owner`(P1,blast radius 大则 P0)。
   - **payload 内容闭环**：payload 携带完整结果 / 长内容 / 跨 hat 摘要(>200 字符或可恢复) → `preset.payload_carries_full_content`(P0);sub-agent 完整结果通过 hat 长消息返回(未落盘) → `preset.subagent_result_returned_only_in_message`(P0)。
   - **internal-ledger-as-artifact**:任何 hat instructions 要求把 `.ralph/events.jsonl` / `.ralph/loops.json` / `.ralph/supervisor.db` 当业务 artifact 接口(写或读) → `preset.artifact_uses_internal_ledger`(P0)。
   - **preset 自身 ownership**:preset 文本被描述为 artifact 创建者(实际由 hat / sub-agent 在 activation 创建) → `preset.artifact_described_as_preset_owned`(P0)。
   - 详细 default severity / confidence / aaf_question / category 见 `references/finding-rubric.md`「Artifact-First Handoff finding_id」表。

6. **Handoff Audit table:** for each edge A→B: A.Q4 emit fields | projection action | downstream Q2 Observe | verdict | finding id.
   - Closed handoff = upstream emit field is in projection → downstream Q2 Observe command sees it.
   - Open handoff = P0 unless runtime evidence proves otherwise.
   - **Artifact-First handoff 子集**：对每条 artifact handoff 边额外列「artifact 路径 → projection → 下游 Q2 Observe 实际命令 → 消费确认」,open 即 `preset.artifact_path_not_in_visible_context`(P0) 或 `preset.artifact_no_consumer_declared`(P1)。

7. **Mechanical lint** — run commands in `references/commands.md`. Map JSON `id` values (`lint.preset.*`) via `references/finding-rubric.md`. **Continue AAF and payload audit even if lint fails**; note failure in Executive Summary.
   - If `preset.instructions_emit_feedback_skill_reference_missing` appears, treat it as a real adoption gap unless the hat does not construct payloads. The repair surface is the relevant hat `instructions:` plus, if needed, `event_policy.schemas.<topic>.field_docs`.
   - **Trigger Context lint IDs** (`preset.trigger_context_unknown_field` / `preset.trigger_context_unsupported_predicate` / `preset.trigger_context_value_shape` / `preset.trigger_context_duplicate_label` / `preset.trigger_context_no_consumer`) 是机械 lint 抓得到的 shape / 拓扑错误。命中即按 `references/finding-rubric.md` 默认 severity 与 confidence 入表，**不要重写为软性 AAF**。
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

1. **Executive Summary** — mode, hat count, P0/P1/P2 counts (confidence ≥ 60 only), lint pass/fail, **payload audit pass/fail**, policy-check feedback adoption pass/fail, **artifact-first handoff audit pass/fail** (含 review-only finding 计数,例如 `preset.artifact_path_not_in_visible_context` / `preset.artifact_no_lifecycle_owner` / `preset.payload_carries_full_content` 等)
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
|| F-101 | P0 | 90 | payload-content | Q4 | executor | hats.executor.publishes[work.done].payload.report_body | sub-agent 完整结果通过 `report_body` 内联在 payload,未在 `.ralph/reports/<unit>.md` 落盘 | emitter instructions 改为「先写 `.ralph/reports/<unit>.md`」,emit 只携带 `report_path` + 短摘要 |

|| F-102 | P0 | 90 | visibility | Q2 | reviewer | hats.reviewer.instructions | 上游 emit 携带 `report_path`,但 reviewer instructions 未要求读文件 | instructions 显式 `cat <report_path>` 后再决策,移除 `report_body` 内联依赖;命中 `preset.artifact_path_not_in_visible_context` |

|| F-103 | P0 | 95 | visibility | Q3 | planner | hats.planner.instructions | planner 把阶段状态写到 `.ralph/supervisor.db`(runtime internal ledger) | 改写为 `.ralph/plans/<plan>.md`,emit 只携带路径与摘要;命中 `preset.artifact_uses_internal_ledger` |

|| F-104 | P1 | 80 | state | Q5 | orchestrator | hats.orchestrator.publishes[loop.summary].payload.intermediate_metrics_path | 中间文件由多 hat 持续追加,但 preset 未声明消费方 / 保留 / 清理责任 | 在 preset 顶部或 orchestrator instructions 声明 owner + retention + cleanup;命中 `preset.artifact_no_lifecycle_owner` |
```

## Guardrails (review skill itself)

- Never pass/fail based on "agent read the whole preset."
- Isolated P0 evidence must be **hat-visible** unless proving runtime injection.
- Reject user request for chat-only review — write the report file.
- Reject "handoff unclear" / "payload looks weak" findings that don't name field + source + fix — rewrite with evidence or move to `Unverified Suspicions`.
- **Artifact-First 拒绝放行**：禁止因「payload 含路径字段」就判定 handoff 闭环。必须同时验证 ① 路径在 consumer hat 可见输入中、② consumer instructions 显式要求读文件、③ `field_docs.<path_field>.meaning` 明确落盘点语义、④ 生命周期责任(consume / retain / cleanup)有声明。任一缺失 → 按 `preset.artifact_path_not_in_visible_context` / `preset.artifact_no_consumer_declared` / `preset.artifact_first_passed_on_path_presence` / `preset.artifact_no_lifecycle_owner` 入主表。
- **Artifact-First 不接受「字符很少」作为唯一例外理由**：例外必须满足短暂 + 短小 + 无需恢复,并标注恢复 / 审计 / 下游依赖依据。理由不充分 → `preset.artifact_first_exemption_unjustified` finding。

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
- 验收 fixture: `fixtures/aaf-artifact-first-negative-fixture.yml`(覆盖 AE1-AE5,可用于 review 流程自检)
