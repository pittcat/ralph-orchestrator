# AAF Review Fixtures

**Not registered in `presets/manifest.yml`.** For skill acceptance and manual regression only.

## Files

| File | Purpose |
|---|---|
| `aaf-review-negative-fixture.yml` | AAF + invisible-input violations (Q2 missing / ledger read / unprojected handoff) |
| `payload-audit-negative-fixture.yml` | Mechanically valid topology with payload-content violations (fabricated identity, vague decision, missing upstream field) |

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

## Report output

Default: `.ralph/reviews/<fixture-basename>-<date>.md`
