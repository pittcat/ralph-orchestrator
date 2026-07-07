# AAF Review Fixtures

**Not registered in `presets/manifest.yml`.** For skill acceptance and manual regression only.

## Files

| File | Purpose |
|---|---|
| `aaf-review-negative-fixture.yml` | Intentional AAF violations for review skill |

## Acceptance Checklist

### 1. Author gate on negative fixture

Run `ralph-preset-author` mindset on `aaf-review-negative-fixture.yml`:

- `worker` hat Q2 should be **unfillable** (「等 reviewer 通过」with no Observe path)
- `worker` instructions reference `.ralph/events.jsonl` → Q3 P0
- `reviewer` expects `secret_handoff_token` but `worker` Q4 does not emit it → Q5 P0

Author should **not** deliver notes with「待定」on Q2 for worker.

### 2. Review negative fixture

```bash
ralph preset check -H skills/ralph-preset-common/fixtures/aaf-review-negative-fixture.yml --strict --format json
```

**机械 lint（preset check）：** 期望 **4 条 Error**，均为 `lint.preset.hat_scope_*`（`event_filter` 未启用、`topic_deny` 不完整）。`instructions_opac`（如 `instructions_read_internal_ledger`）**不会**在此路径触发——`ralph preset check` 不传 raw YAML 给 opac lint。

**软性 AAF（review skill 必查）：** 仍须产出 ≥2 条 P0（confidence ≥ 60），例如：

- worker Q2：「等 reviewer 通过」无 Observe 路径
- worker Q3：读 `.ralph/events.jsonl`
- reviewer Q2/Q5：`secret_handoff_token` 上游未 emit

### 3. Review clean builtin

```bash
ralph preset check -H builtin:debug --strict
```

Mechanical lint should pass. AAF review: 4 hats → 4 Per-Hat AAF sections + Handoff Audit rows.

### 4. Three-hat minimum

Any preset with ≥3 hats → report Per-Hat AAF 表数量 = hat 数；Handoff Audit 覆盖相邻业务边。

## Report output

Default: `.ralph/reviews/aaf-review-negative-fixture-<date>.md`
