# AAF Review Fixtures

**Not registered in `presets/manifest.yml`.** For skill acceptance and manual regression only.

## Files

| File | Purpose |
|---|---|
| `aaf-review-negative-fixture.yml` | AAF + invisible-input violations (Q2 missing / ledger read / unprojected handoff) |
| `payload-audit-negative-fixture.yml` | Mechanically valid topology with payload-content violations (fabricated identity, vague decision, missing upstream field) |
| `trigger-context-negative-fixture.yml` | Trigger Context anti-pattern index (2026-07-09-003 plan U8 step 11): unknown summary field, duplicate hint label, unsupported predicate, value-shape mismatch, no consumer, and instructions duplicating hint conditions |
| `aaf-wave-capability-negative-fixture.yml` | Wave capability audit axes (2026-07-22-002 plan U6): worker `ralph wave emit`, missing `wave verify`, hat-channel Confirm, hat publishes coordination topic |
| `aaf-supervisor-capability-negative-fixture.yml` | Supervisor capability audit axes (2026-07-22-002 plan U6): supervisor with non-isolated mode, hat publishes `exec.wave.*`, dispatcher reads `supervisor.db`, execution_model Intent mismatch |
| `reentry-dirty-worktree-positive-fixture.yml` | Step 1.25 re-entry dirty-worktree reconciliation (2026-07-28-002 plan R-F13): writable executor hat with full attributable/unattributable protocol — zero reentry findings expected |
| `reentry-dirty-worktree-negative-fixture.yml` | Step 1.25 anti-patterns: missing reconciliation (P1), unconditional `git checkout . && git clean -fd` on unattributable dirt (P0), no fail-closed terminal (P0) |

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

### 7. Wave / Supervisor capability negative fixtures (2026-07-22-002 plan U6)

`aaf-wave-capability-negative-fixture.yml` and
`aaf-supervisor-capability-negative-fixture.yml` carry four / five
anti-pattern axes each, covering the new `preset.wave_*` and
`preset.supervisor_*` review-only findings in
`references/finding-rubric.md`「Wave capability audit」 /
「Supervisor capability audit」 段. Each axis is tagged in an inline
comment so review-skill trainees can read a single YAML and locate every
shape error the soft AAF path is supposed to catch.

| Axis | What the YAML contains | Expected finding | Source |
|---|---|---|---|
| Wave (a) | `worker_alpha` instructions require `ralph wave emit` | `preset.wave_worker_calls_wave_emit` | U4 finding |
| Wave (b) | `dispatcher` skips `ralph wave verify` before real emit | `preset.wave_missing_verify_before_emit` | U4 finding |
| Wave (c) | `worker_alpha` Confirm path reads hat-channel | `preset.wave_confirm_uses_hat_channel` | U4 finding |
| Wave (d) | `coordinator` publishes `exec.wave.complete` (coordination topic) | `preset.wave_agent_emits_coordination_topic` | U4 finding |
| Supervisor (a) | `execution_mode: coordinator` + `supervisor.enabled: true`（非 isolated） | `preset.supervisor_requires_isolated`（软性 AAF / review 读 YAML 形状命中；当前 `ralph preset check` CLI 路径可能不吐出该 lint id） | existing lint id / U4 finding |
| Supervisor (b) | `worker` publishes `exec.wave.failed` | `preset.supervisor_hat_publishes_coord_topic` | U4 finding |
| Supervisor (c) | `dispatcher` reads `.ralph/supervisor.db` as unit state | `preset.supervisor_unit_state_not_via_task_api` | U4 finding |
| Supervisor (d) | `dispatcher` reads `supervisor.db` as business artifact | `preset.artifact_uses_internal_ledger` | existing review-only |
| Supervisor (e) | `execution_model: single-chain` Intent vs `supervisor.enabled: true` YAML | `preset.execution_model_intent_mismatch` | U4 finding |

**Acceptance gates:**

```bash
# CLI 冒烟 — 不要求 strict 模式吐出新 finding (无新增 lint); 仅验证
# fixture 可被加载 + 结构化 lint 不崩.
ralph preset check -H skills/ralph-preset-common/fixtures/aaf-wave-capability-negative-fixture.yml --strict --format json
ralph preset check -H skills/ralph-preset-common/fixtures/aaf-supervisor-capability-negative-fixture.yml --strict --format json

# 软性 AAF (Wave/Supervisor capability audit) 是 review-only, 通过阅读
# fixture 配合 references/finding-rubric.md 新增段对照命中。
```

**Capability-triggered invariant**: 两个 fixture **不**复制 `ce-executor-supervisor`
或任何 builtin preset 的全拓扑; 触发条件写在 fixture 顶部注释里, 与 builtin
preset 名称完全无关。 任何 review 脚本不得按 `name starts with ce-executor-supervisor`
对这两个 fixture 做特殊分支。

## Report output

Default: `.ralph/reviews/<fixture-basename>-<date>.md`
