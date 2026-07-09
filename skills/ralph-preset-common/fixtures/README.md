# AAF Review Fixtures

**Not registered in `presets/manifest.yml`.** For skill acceptance and manual regression only.

## Files

| File | Purpose |
|---|---|
| `aaf-review-negative-fixture.yml` | AAF + invisible-input violations (Q2 missing / ledger read / unprojected handoff) |
| `payload-audit-negative-fixture.yml` | Mechanically valid topology with payload-content violations (fabricated identity, vague decision, missing upstream field) |
| `trigger-context-negative-fixture.yml` | Trigger Context anti-pattern index (2026-07-09-003 plan U8 step 11): unknown summary field, duplicate hint label, unsupported predicate, value-shape mismatch, no consumer, and instructions duplicating hint conditions |

## Acceptance Checklist

### 1. Author gate on negative fixtures

Run `ralph-preset-author` mindset on each negative fixture:

- AAF negative fixture:
  - `worker` hat Q2 should be **unfillable**（「等 reviewer 通过」with no Observe path）
  - `worker` instructions reference `.ralph/events.jsonl` → Q3 P0
  - `reviewer` expects `secret_handoff_token` but `worker` Q4 does not emit it → Q5 P0
- Payload-audit negative fixture:
  - `dimension_reviewer` Q4 hard-codes `task_id: task-123` instead of reading live runtime identity → P0 payload-content
  - `summary: "done"` must be flagged as P1 payload-content (decision field with insufficient semantic content)
  - `reviewer_consumer` references `correction_signal` which is not emitted anywhere → P0 payload-content / Q5

Author should **not** deliver notes with「待定」「上游会处理」「约定俗成」on any of these.

### 2. Review negative fixtures

```bash
ralph preset check -H skills/ralph-preset-common/fixtures/aaf-review-negative-fixture.yml --strict --format json
ralph preset check -H skills/ralph-preset-common/fixtures/payload-audit-negative-fixture.yml --strict --format json
```

**机械 lint（preset check）：**

- AAF negative fixture: 期望 **≥4 条 Error**，多为 `lint.preset.hat_scope_*`（`event_filter` 未启用、`topic_deny` 不完整）。`instructions_opac` 类不会在此路径触发——`ralph preset check` 不传 raw YAML 给 opac lint。
- Payload-audit negative fixture: 期望 mechanical lint 安静或只出现与 payload-content 无关的环境性告警。该 fixture 已声明基础 ownership / filter 结构；`task_id: task-123`、`summary: "done"`、`correction_signal` 上游缺失这三类问题必须由软性 audit 发现，不能依赖 `ralph preset check`。

**软性 AAF + Payload Audit（review skill 必查）：**

AAF negative fixture 期望 ≥2 条 P0（confidence ≥ 60）：

- worker Q2：「等 reviewer 通过」无 Observe 路径
- worker Q3：读 `.ralph/events.jsonl`
- reviewer Q2/Q5：`secret_handoff_token` 上游未 emit

Payload-audit negative fixture 期望 ≥2 条 P0 + ≥1 条 P1（confidence ≥ 60）：

- P0：`task_id` 在 dimension_reviewer payload 中被写成 `task-123`，不是从 `ralph tools task list` 或等价 live runtime view 取得
- P0：`correction_signal` 被下游引用但上游不 emit；schema 未声明 → payload-content / handoff 双重违规
- P1：`summary: "done"` 决策字段无法支撑下游 fix/block/complete 分支

### 3. Report quality gate

无论机械 lint 通过 / 失败，review report 必须包含：

- [ ] Executive Summary 标明 payload audit pass/fail
- [ ] Findings Table 每行带 `category`（feasibility / handoff / **payload-content** …）+ `aaf_question` + `hat` + repair surface
- [ ] Per-Hat AAF Reviews：每个 hat 一段，按 activation sequence（trigger → context → command → payload → emit → handoff）
- [ ] Payload Audit Table：每个 material emit topic 至少一行（topic / field / source / visibility / identity / downstream / verdict / fix）
- [ ] Handoff Audit Table：closed / open / finding id per 业务边
- [ ] Remediation Plan：按 **runtime unblock order** 排序，不按 file / discovery
- [ ] Unverified Suspicions：必须有可疑的具体 repair surface（不允许只写 "handoff unclear"）

**`handoff unclear` / `payload weak` 等无 repair surface 的 finding 必须被拒入主表**，移到 Unverified Suspicions 或重写。

### 4. Review clean builtin

```bash
ralph preset check -H builtin:debug --strict
```

Mechanical lint should pass. AAF review: 4 hats → 4 Per-Hat AAF sections + Payload Audit 表覆盖 material emit + Handoff Audit rows for adjacent 业务边。**不能为了凑数而虚构 payload P0**——clean preset 应几乎全是 Pass。

### 5. Three-hat minimum

Any preset with ≥3 hats → report Per-Hat AAF 表数量 = hat 数；Payload Audit 表覆盖每个 material emit topic；Handoff Audit 覆盖相邻业务边。

### 6. Trigger Context negative fixture (2026-07-09-003 plan U8)

`trigger-context-negative-fixture.yml` carries six anti-pattern axes covering the
schema-backed trigger context contract. Each axis is tagged in an inline comment
so review-skill trainees can read a single YAML and locate every shape error
the lint family and soft AAF path are supposed to catch.

| Axis | What the YAML contains | Expected finding | Source |
|---|---|---|---|
| (a) summary field not in known set | `summary_fields: [ghost_count, ...]` | `preset.trigger_context_unknown_field` | U4 lint |
| (b) unsupported predicate op | hint condition with `op: regex` | `preset.trigger_context_unsupported_predicate` | U4 lint |
| (c) numeric op with non-number value | `op: gt, value: "not-a-number"` | `preset.trigger_context_value_shape` | U4 lint |
| (d) duplicate hint label | two hints labelled `accept_when_clean` | `preset.trigger_context_duplicate_label` | U4 lint |
| (e) no hat subscribes to topic with context | `review.synthesized` declares context, no `triggers:` includes it | `preset.trigger_context_no_consumer` | U5 topology lint |
| (f) instructions copy hint conditions | `gate` instructions re-state `accept_when_clean` conditions in prose | soft AAF P1 (`style / Q3`) | review-only |

**Acceptance gates:**

```bash
# Lint family: 验证 U4/U5 typed-config lint 实现。
# 这一步是 5 类 trigger_context lint finding 的真正来源。
cargo nextest run -p ralph-core -- preset_lint::trigger_context

# Soft AAF (axis f) is review-only and is verified by reading the fixture
# alongside `references/finding-rubric.md` 「Trigger Context 软性 AAF 缺口」
# table.
```

**CLI 验收的当前限制（2026-07-09）**：`ralph preset check -H trigger-context-negative-fixture.yml --strict`
在该 fixture 上**不会**直接吐出 5 条 `preset.trigger_context_*` finding —— Step-2
`RuntimeContractAggregator` 在 strict 模式下确实调用 `run_preset_lint(Strict)`，
但 typed-config 链路在本 CLI 表面上对这 5 类 ID 的暴露路径当前与 unit-test
路径不一致。这是已知 follow-up，本 fixture 的 lint 验收以 nextest 子集为准，
**不要把 CLI 输出当作这 5 类 finding 的判 fail 依据**。

## Report output

Default: `.ralph/reviews/<fixture-basename>-<date>.md`
