# Payload Contracts

Payload contracts define what fields each event's payload **must** carry,
enforced both at preset-load time (static) and at runtime (dynamic).
They close the gap where a hat's `instructions` block reads `task_id` from
a `work.ready` event but the producer forgets to include the field.

Payload contracts are complementary to **execution contracts** (which
verify the agent's *completion declaration* — e.g. `work.done` carries
`task_id` and a closed runtime task). Payload contracts verify the
*event shape* that flows between hats.

---

## Why Two Layers?

| Layer | Catches | When |
|---|---|---|
| Execution contract | "agent claimed `work.done` but no closed task / no git change" | `ralph run` event policy, observe-mode (does not pause the loop) |
| Payload contract | "executor reads `plan_name` from `work.ready` but the topic's schema does not declare it" | `ralph run` startup hard gate + `ralph hats validate` + runtime `Enforce` mode |

The two layers are required and cannot replace each other:

- Execution contract without payload contract: a hat can still crash at
  runtime because the upstream topic lacks a field the hat depends on.
- Payload contract without execution contract: a hat can publish a
  topic with all required fields but lie about whether the work is
  actually done.

---

## Three Mechanisms

| Mechanism | When it runs | What it does |
|---|---|---|
| `ralph hats validate` (default mode) | `cargo run -- hats validate` | Reports payload contract warnings for any topic with payload refs but no schema. Does not fail. |
| `ralph hats validate --strict` | `cargo run -- hats validate --strict` | Reports payload contract errors and exits non-zero if any required topic has a missing field or missing schema. |
| `ralph run` startup hard gate | Always, before spawning any backend | Same as `--strict`; cannot be bypassed (no `--skip-payload-check` flag). |
| Runtime `Enforce` mode | When `event_policy.mode: enforce` and a real event arrives missing a required field | Loop pauses, diagnostic JSON written to `.ralph/diagnostics/payload-contract-error-{timestamp}.json`, terminal exit code 1. |

---

## Declaring a Schema

Schemas are declared under `event_policy.schemas.<topic>.required_fields`
inline, or in a separate YAML file referenced by `event_policy.schema_file`.

### Inline

```yaml
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    schemas:
      work.ready:
        required_fields: [plan_name, task_id, task_key, step]
      work.done:
        required_fields: [plan_name, task_id, task_key, step]
```

### External file (preferred for multi-topic presets)

Place the schema in a sibling file and reference it via `schema_file`.
The path is resolved relative to the preset's containing directory.

```yaml
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    schema_file: "schemas/ce-executor.yml"
```

Where `schemas/ce-executor.yml` lives next to the preset yml.

### Schema format

A schema file is a YAML map of topic name → schema:

```yaml
work.ready:
  required_fields:
    - plan_name
    - task_id
    - task_key
    - step
  payload: json_object   # optional; structural type check (default)

work.failed:
  required_fields:
    - reason
```

- `required_fields` — list of field names that must be present in the
  event payload.
- `payload` — optional structural type. `json_object` is the default
  and the only structural type currently enforced at runtime.
- Inline schemas take priority over file schemas when both define the
  same topic.

---

## Extractor Behaviour

The static validator (`validate_payload_contract`) uses a conservative
extractor (`extract_payload_field_refs`) to find payload field
references in hat instructions. Three patterns are matched:

| Pattern | Example line in instructions | Field extracted |
|---|---|---|
| `From event payload: <field1>, <field2>, ...` | `From event payload: task_id, plan_name, step` | `task_id`, `plan_name`, `step` |
| `payload MUST include: <field1>, <field2>, ...` | `payload MUST include: task_id, task_key` | `task_id`, `task_key` |
| Backtick field on line with explicit intent | `From event payload: read \`task_id\`, \`task_key\`` | `task_id`, `task_key` |

What the extractor does **not** match (to avoid false positives):

- Lines that merely contain the word "payload" without the intent
  prefixes above.
- Backticked values that are not bare identifiers (containing spaces,
  dots, slashes, hyphens, or non-alphanumeric chars). Topic names
  like `` `work.done` `` and file paths like `` `fix-log.md` `` are
  ignored.
- Hashed comments or code fences.

### Tuning per-hat

If your hat instructions include a payload field that you want the
extractor to skip (e.g., a field name that happens to collide with a
real identifier but isn't a contract field), use
`HatConfig.ignore_payload_fields`:

```yaml
hats:
  executor:
    ignore_payload_fields: [legacy_field, debug_only]
```

`ignore_payload_fields` is consumed only by the static validator. The
runtime event policy still enforces whatever the schema declares.

---

## Runtime Violation Diagnostic

When a real event violates a schema in `Enforce` mode, the loop pauses
and writes a JSON file under
`.ralph/diagnostics/payload-contract-error-{timestamp}.json`.

The file's contents:

```json
{
  "error_type": "missing_required_field",
  "timestamp": "2026-06-03T12:34:56.789Z",
  "topic": "work.ready",
  "field": "plan_name",
  "source_hat": ["coordinator"],
  "target_hat": ["executor"],
  "schema_defined_in": "inline + file:schemas/ce-executor.yml",
  "downstream_reference": null,
  "upstream_reference": null,
  "fix_hint": "Add the missing field to the payload of the 'work.ready' event. ...",
  "payload_excerpt": "{\"task_id\": \"t-1\"}"
}
```

| Field | Meaning |
|---|---|
| `error_type` | `missing_required_field` / `payload_type_mismatch` / `allowed_value_mismatch` / `schema_missing_for_required_topic` |
| `source_hat` | Hat ID(s) that published the topic (the producer side). Multiple values listed if several hats can publish. |
| `target_hat` | Hat ID(s) subscribed to the topic (the consumer side). |
| `schema_defined_in` | `inline`, `file:<path>`, or `inline + file:<path>` when both define the same topic. |
| `fix_hint` | Actionable hint for the operator or the producing hat. |
| `payload_excerpt` | First 240 chars of the offending payload (truncated for safety). |

The terminal also prints
`[PAYLOAD CONTRACT VIOLATION] Loop paused. Diagnostic written to <path>`.
If the diagnostic write fails (e.g., disk full, permission denied), the
violation summary is still printed on stderr and the loop still
terminates with non-zero exit code — the diagnostic file is
informational, not the source of truth for the violation.

---

## Builtin Schema Library

As of 2026-06-03 the builtin presets inline their payload schemas
directly under `event_policy.schemas` in `presets/en/<name>.yml`. The
previous `event_policy.schema_file: "../schemas/<name>.yml"` form
was broken for builtin presets — `resolve_schema_files` has no
on-disk anchor to resolve the relative path against, so the schemas
silently went unloaded and the payload contract hard gate failed
with `SchemaMissingForRequiredTopic` for every topic the preset
declared. (Symptom: `ralph run -H builtin:ce-executor-pipeline -p "..."` reports
"Subprocess exited before starting the orchestration loop" with the
real cause buried in `.ralph/diagnostics/logs/`.)

For file-based hat collections (e.g. `ralph run -H .ralph/hats/my.yml`)
the relative `schema_file` form still works, but the inline form is
preferred for consistency and for portability across install
locations.

The `presets/schemas/` directory is the authoring SSOT for builtin
preset schemas. It is merged into the embedded preset at compile time
by `crates/ralph-cli/build.rs`.

| Schema file | Preset that owns the schemas | Status |
|---|---|---|
| `presets/schemas/ce-executor-pipeline.yml` | `ce-executor-pipeline` | Authoring SSOT (merged into `presets/en/ce-executor-pipeline.yml` at build time) |

When adding a new schema to a builtin preset:

1. Edit `presets/schemas/<name>.yml` for the schema SSOT, or add an
   `event_policy.schemas.<topic>` entry directly in `presets/en/<name>.yml`
   as an override layer.
2. Keep `presets/en/<name>.yml` and `presets/schemas/<name>.yml` in sync;
   run `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`
   for SSOT byte-equality where applicable.
3. Run `ralph hats validate --strict -H builtin:<name>` to confirm
   no schema warnings or errors.

---

## CLI Reference

```bash
# Default mode: report warnings; do not fail.
ralph hats validate -c ralph.yml -H .ralph/hats/my-flow.yml

# Strict mode: report errors; exit non-zero on any payload contract violation.
ralph hats validate --strict -c ralph.yml -H .ralph/hats/my-flow.yml

# `ralph run` automatically runs the strict hard gate. There is no skip flag.
ralph run -c ralph.yml -H builtin:ce-executor-pipeline -p "..."
```

---

## Boundary With Execution Contracts

| Question | Payload contract | Execution contract |
|---|---|---|
| "Did the agent actually finish the work?" | — | yes (`work.done` requires task closed, git diff or commit, optional test evidence) |
| "Did the agent emit an event with the right shape?" | yes (required fields per topic) | partial (`work.done` only; the `work.done` rule declares `require_payload_fields`) |
| "Where in the loop does it fire?" | Static: at preset load. Runtime: on every JSONL event with `Enforce`. | Static: at preset load. Runtime: on `work.done` events only. |
| "What happens on violation?" | Static: non-zero exit. Runtime: pause + diagnostic JSON. | Runtime: rejection with `task.resume` recovery (does not pause the loop). |
| "Can the agent bypass it?" | No. | No. |

The two contracts are required and complementary; disabling either
opens a regression.

---

## Schema Metadata: Authoring Hint

payload contract 不只是机器校验——schema 作者还可以为 preset/schema reader
（review agent、emit agent、AAF 评审员）声明**字段含义与取值来源**。这些 metadata
不参与 runtime accept/reject 决策，只让 agent 知道字段怎么填、错在哪里。

声明位置在 `event_policy.schemas.<topic>` 下。

| 字段 | 是否参与机器校验 | 用途 | 读者 |
|---|---|---|---|
| `required_fields` | 是 | runtime 拒收缺字段 | runtime |
| `payload` | 是 | 结构类型检查 | runtime |
| `allowed_values` | 是 | enum 白名单 | runtime |
| `hat_allowed_values` | 是 | 按 hat 收紧的 enum 白名单 | runtime |
| `element_constraints` | 是 | array/object 元素检查 | runtime |
| `field_docs` | 否 | 字段含义 + 来源 + 填法 | agent / AAF reviewer |
| `examples` | 否 | topic 级示例 payload | agent / prompt builder |
| `known_fields` | 否（lint 校验） | schema 知道存在但非必填的字段，供 trigger_context 引用 | runtime + lint |
| `trigger_context` | 否（lint 校验） | 下游 hat prompt 注入 `## TRIGGER CONTEXT` 区块 | runtime prompt builder |

### `field_docs` 形状

```yaml
event_loop:
  event_policy:
    schemas:
      work.done:
        required_fields: [plan_name, task_id, task_key, step]
        field_docs:
          task_id:
            meaning: "当前 loop 的 live task id"
            source: "ralph tools task list → 当前 active task"
            fill_rule: "禁止手写，必须从 task list 取得 live id"
          verdict:
            meaning: "本 hat work 的完成判定"
            source: "本 hat work 输出"
            fill_rule: "只能填 pass / blocked / partial 之一"
```

每个字段有三个独立可选子键：

- `meaning`:字段语义（一句话解释，让 agent 不再从字段名猜）。
- `source`:值从哪儿来（CLI 命令、其它 topic、当前 hat work 输出）。**不允许
  写"由 agent 自行判断"——`fill_rule` 禁止 agent 凭直觉填业务事实。**
- `fill_rule`:填法（必须 live / 必须 enum / 必须严格匹配 schema prompt section
  等）。

没有声明 `field_docs` 的字段、`field_docs` 没出现的字段——policy-check 拒收时
仍然只给机器校验字段（`field` / `reason_code` / `expected` / `actual` / `message`）。

### `examples` 形状

```yaml
work.done:
  required_fields: [plan_name, task_id, task_key, step, verdict, reason]
  examples:
    - plan_name: "2026-07-09-001-feat-policy-check-agent-feedback-plan"
      task_id: "T-007"
      task_key: "t-007:step-3:"
      step: 3
      verdict: "pass"
      reason: "policy-check enrichment 已通过 strict lint"
```

`examples` 用于 prompt builder 渲染 schema-aware publish section（agent 在 prompt
里看到的 `<topic>` 示例 payload）。**policy-check 报错时给的
`suggested_payload_shape` 永远是占位符骨架，不会从这里取业务值填充**——
参见下一节。

### `known_fields` 形状

```yaml
review.synthesized:
  required_fields: [plan_name, task_path, review_round, ...]
  known_fields:
    - "round_base_sha"
    - "diff_patch_file"
    - "synthesized_review_file"
```

- `known_fields` 中的字段是 schema 作者**明确知道存在**的 optional / pass-through
  字段，不在 `required_fields` 中但也允许 trigger_context 引用。
- runtime 不接受/拒绝它们；strict lint 检查 `summary_fields` / hint condition
  引用的字段必须在 `required_fields ∪ known_fields ∪ field_docs.keys() ∪
  allowed_values.keys()` 之内，未声明的引用会触发
  `trigger_context_unknown_field` Error。
- 试用例：见 `presets/schemas/ce-executor-pipeline-loop.yml` 中
  `review.synthesized.known_fields`（`round_base_sha` / `diff_patch_file` /
  `synthesized_review_file` 都是 pass-through 字段，不作 gate 决策但供下游 hint
  引用）。

---

## Policy-Check 反馈（读懂 `--policy-check` 拒收）

> 来自 plan `2026-07-09-001-feat-policy-check-agent-feedback-plan`（U1-U9）。
> 实现已经落地，字段集以本节为准。

`ralph emit --policy-check` / `ralph wave emit --policy-check` 拒收时，错误
响应里**每条** `validation_errors[]` 现在带一组可机读、可修复的字段。修 payload
时**优先读这些字段**，不要凭"error message"猜字段含义。

### 单事件：JSON 拒收示例

```bash
ralph emit work.done --policy-check -j '{"verdict": "bogus"}'
```

```json
{
  "ok": false,
  "recorded": false,
  "topic": "work.done",
  "errors": [
    {
      "field": "task_id",
      "reason_code": "missing_required_field",
      "message": "required field missing",
      "expected": ["task_id"],
      "actual": null,
      "field_description": "当前 loop 的 live task id; 必须从 ralph tools task list 取得",
      "suggested_payload_shape": {
        "plan_name": "<plan_name>",
        "task_id": "<task_id>",
        "task_key": "<task_key>",
        "step": "<step>",
        "verdict": "bogus"
      },
      "suggested_command": "ralph emit work.done --policy-check -j '{\"plan_name\":\"<plan_name>\",\"task_id\":\"<task_id>\",\"task_key\":\"<task_key>\",\"step\":\"<step>\",\"verdict\":\"<verdict>\"}'"
    },
    {
      "field": "verdict",
      "reason_code": "invalid_field_value",
      "message": "verdict not in allowed_values",
      "expected": ["pass", "blocked", "partial"],
      "actual": "bogus"
    }
  ]
}
```

### 字段含义速查表

| 字段 | 你从这里得到什么 | 下一步动作 |
|---|---|---|
| `field` | 触发错误的 payload 字段名；空字符串表示错误在 payload / topic 层级 | 定位 payload 中要改的字段 |
| `reason_code` | 稳定错误码（`missing_required_field` / `invalid_field_value` / `payload_type_mismatch` / `terminal_monotonicity_violation` 等） | 判断错误类别，决定补字段还是改值 |
| `expected` | schema 期望的形态：required field 名 / allowed_values 列表 / payload 类型 | 知道应该填什么 |
| `actual` | 实际触发的 payload 值；缺字段时为 `null` | 知道当前 payload 哪里不对 |
| `field_description` | `field_docs.<f>.meaning`（schema 声明时才有） | 读懂字段语义后再改 |
| `suggested_payload_shape` | 已存在字段保留原值，缺失字段用 `<field>` 占位符的 JSON 骨架 | 把占位符替换成实际值，整骨架原样保留 |
| `suggested_command` | 修完后可直接重跑的 `ralph emit <topic> --policy-check -j '<shape>'` 命令 | 复制粘贴重跑 |

### 修复流程（agent 必读）

1. 跑 `ralph emit <topic> --policy-check -j '<payload>'` 预检。
2. 拒收时读第一条 error 的 `field` ——**只**改 `field` 提示的字段。
3. 读 `expected` 与 `actual` 知道"原来填错了什么 / 应该填什么"。
4. 读 `field_description` 理解字段含义（schema 没声明时此字段缺失，靠
   `meaning` 在 prompt 解析）。
5. 把 `suggested_payload_shape` 中所有 `<field>` 占位符替换成实际值，已存在
   字段保留原值。**不要**填业务结论（如 `0` / `pass`）；占位符是真实未填字段
   的标记，不是默认填法。
6. 跑 `suggested_command` 重试；通过后**去掉 `--policy-check`** 正式 emit。

### Wave batch 特殊处理

`ralph wave emit --policy-check` 的 `validation_errors[]` 每条带
`payload_index`，对应原始 batch 的索引：

```json
{
  "ok": false,
  "validation_errors": [
    {
      "payload_index": 3,
      "field": "depth",
      "reason_code": "missing_required_field",
      "field_description": "...",
      "suggested_payload_shape": {"path": "/.../L2:file.rs", "depth": "<depth>"}
    },
    {
      "payload_index": 7,
      "field": "depth",
      "reason_code": "missing_required_field"
    }
  ]
}
```

整个 batch 仍 atomic reject（任何一个失败 = events.jsonl 一行都不写）。修整批
后一次性重发，而不是分多次 patch。

### 与 runtime diagnostic 的边界

- `--policy-check`（dry-run，CLI 层）：JSON 直接打印到 stdout / stderr，本节
  描述的就是它。
- runtime payload contract violation（`event_policy.mode: enforce`，loop 跑
  起来后）：见上文「Runtime Violation Diagnostic」节，写到
  `.ralph/diagnostics/payload-contract-error-{timestamp}.json`。
- 两者都用同一份 machine authority（`required_fields` /
  `allowed_values` / `element_constraints`），但 agent 视角下看到的是
  `--policy-check` JSON —— 出错就重跑、不要绕过去写 diagnostic 文件。

---

## Trigger Context（读懂 `## TRIGGER CONTEXT`）

> 来自 plan `2026-07-09-003-feat-schema-backed-trigger-context-plan`（U1-U9；
> Implementation Correction 在 plan 末尾）。

当下游 hat 被激活时，runtime 会注入一个**短、结构化**的 `## TRIGGER CONTEXT`
区块到 prompt 顶部，列出上一轮 trigger payload 的字段摘要与命中的 routing hint。
这与 Policy-Check 是两条独立通道：

- **Policy-Check** = 你下一条 emit 的形状怎么填。
- **Trigger Context** = 上一条 emit（激活你的那条事件）的字段怎么解读。

### 区块位置与阅读顺序

isolated prompt 顶部的相对位置：

```
## ACTIVE TRIGGER       ← 上游 hat 触发的描述
## TRIGGER CONTEXT      ← 新注入：当前 trigger payload 摘要 + matched hints
## NEXT ACTION          ← hat 行动指引
## HAT INSTRUCTIONS     ← 本 hat 自己的 instructions
```

**先**读 `## TRIGGER CONTEXT`，**再**读 hat instructions。Trigger Context 已经
把"看到 payload X 应该走哪条分支"说清楚了；不要从完整 payload 重新推断，也不要
读 `.ralph/events.jsonl` / `.ralph/supervisor.db` 还原该信息。

### 注入条件（runtime 决定要不要注入）

| 条件 | 行为 |
|---|---|
| 当前 trigger topic 的 schema 有 `trigger_context` 声明 | 注入 |
| 当前 hat 在 `triggers` / `subscribes_to` 中包含该 source topic | 该 hat 才看到，其它 hat 不泄漏 |
| 当前 hat 不订阅 source topic | 不注入（即使 preset 其它地方声明了 trigger_context） |
| preset / schema 没声明 trigger_context | 不注入（与旧行为 byte-equivalent） |

### 真实示例（来自 `ce-executor-pipeline-loop`）

`review-gate` hat 订阅 `review.synthesized`，payload 为

```json
{
  "plan_name": "...",
  "review_round": 3,
  "blocking_main_conflict_count": 0,
  "must_fix_now_count": 0,
  "residual_findings_count": 1,
  "verdict": "pass"
}
```

注入到 `review-gate` prompt 的 `## TRIGGER CONTEXT`：

```text
## TRIGGER CONTEXT
source topic: review.synthesized
source hat: review-synthesizer
summary fields:
  review_round: 3
  must_fix_now_count: 0
  blocking_main_conflict_count: 0
  residual_findings_count: 1
  loop_decision_basis: "review round accepted"
  verdict: "pass"
  synthesized_review_file: ".ralph/runtime/ce-executor-pipeline-loop/review-synthesized-3.md"
matched routing hints:
  - [accept_or_residual_report_only] If blocking_main_conflict_count == 0:
    emit only review.accepted. Any residual P0/P1 findings are report-only; do not
    open another fix round for them.
```

`review-gate` hat instructions 应**只引用** `## TRIGGER CONTEXT` 区块，不再
复述条件值。例如：

```yaml
- hat_id: review-gate
  subscribes_to: ["review.synthesized"]
  publishes: ["review.accepted", "review.loop.blocked"]
  instructions: |
    ## TRIGGER CONTEXT
    阅读 prompt 顶部 ## TRIGGER CONTEXT 区块（schema 声明的 summary fields +
    matched routing hints）。
    - 取 hint 中的"本轮你应该如何处理"作为本 hat 行动。
    - 决定 accept / fix / block；构造 emit payload 时再次走 schema-aware
      publish section + `ralph emit --policy-check`。
    - 不要从完整 payload 推断本节信息，不要读 runtime ledger 还原。
```

禁止：把 `must_fix_now_count == 0` / `> 0` 这种条件复制到 instructions；
这样写会与 schema hints 双写漂移。

### 缺失字段的渲染规则（`<missing>` 语义）

schema 在 `summary_fields` 中声明但 payload 没有该字段时，Trigger Context 区块
渲染为 `<missing>`：

```text
summary fields:
  review_round: 3
  blocking_main_conflict_count: 0
  residual_findings_count: <missing>
```

- **不**推断为 `0` / `false` / 空字符串 / `null`。
- **不**省略该行（agent 一眼看出"这一项声明了但缺数据"）。
- 若 hat 决策必须用该字段且显示 `<missing>`，先 `ralph inspect loop --format json`
  或 `ralph tools task list` 复核当前任务状态，再决定继续、阻塞或报告——不要
  自填默认。

`routing_hints` 中数值比较的条件如果读取到了 `<missing>`，**该 hint 不命中**。
`exists` 命中 / `missing` 命中按预设。

### 与 Policy-Check 的关系

| 维度 | Policy-Check | Trigger Context |
|---|---|---|
| 触发时机 | agent 调 `ralph emit` / `ralph wave emit` 前 | 下游 hat 被激活时 |
| 输入 | agent 构造的 payload | 已 accepted 的 trigger event payload |
| 输出 | JSON/text 拒收反馈 | prompt 顶部区块 |
| 责任 | 决定下一条 emit 是否写盘 | 决定本轮 hat 如何理解上一条事件 |
| 不可替代 | runtime schema gate 仍然在 policy-check 之后独立校验 | Trigger Context 不替代 emit 形状验证 |

发事件仍然必须先 `--policy-check` 通过；Trigger Context 只告诉你上一条事件
**怎么理解**，不替你验证下一条 emit 的形状。

### 试点与扩展

当前只有 `ce-executor-pipeline-loop` 的 `review.synthesized` / `review.accepted`
/ `fix.requested` 配置了 `trigger_context`；其它 preset 未声明时不注入——与
未启用 `--policy-check feedback` 等价的向后兼容。

要为自己的 preset 加 trigger context，参见
[Preset Authoring](preset-authoring.md) 与 `skills/ralph-preset-author`。

---

## See also

- `harness-extensions.md` — the four opt-in Harness 4 mechanisms
- `execution-contracts.md` — the `work.done` completion gate
- `docs/plans/2026-06-02-005-feat-payload-contract-validation-plan.md` —
  the design and implementation plan
- `docs/plans/2026-07-09-001-feat-policy-check-agent-feedback-plan.md` —
  policy-check 反可修复反馈的可机读字段（U1-U9 已落地）
- `docs/plans/2026-07-09-003-feat-schema-backed-trigger-context-plan.md` —
  trigger context 区块的设计与实现
