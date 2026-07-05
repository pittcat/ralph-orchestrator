# 日志三联对账手册

让 **可信 events**、`orchestration`（若有）、`recovery`（workspace + session）互相印证。

> `<RUN>` = run_dir；先解析 `EVENTS=$(cat $RUN/.ralph/current-events)` 为绝对路径（见 [artifact-discovery.md](artifact-discovery.md)）。**禁止** `events*.jsonl` 通配。

---

## 0. Diagnostics 模式与 L2 适用性

| 模式 | L2 orchestration 行 | L2 recovery 行 |
|------|---------------------|----------------|
| FULL | **必做** | 必做 |
| MINIMAL | 跳过；用 session recovery | 必做 |
| LOGS_ONLY | **跳过**（无 orchestration 是预期） | workspace recovery + logs |
| DISABLED | 跳过 | 仅 workspace recovery |

LOGS_ONLY 下「events 有、orchestration 无」**不是**机制 bug，不得标 P0。

---

## 1. 业务事件清单（账本 A）

```bash
EVENTS=<绝对路径，来自 current-events>
jq -r 'select(.topic != null) | "\(input_line_number) \(.topic) hat=\(.hat // .source_hat // "?")"' "$EVENTS" | head -80
rg -n 'LOOP_COMPLETE|plan\.complete|plan\.blocked|REVIEW_COMPLETE' "$EVENTS" | tail -10
```

---

## 2. Orchestration（仅 FULL）

```bash
SES=<timestamp session id>
jq -r 'select(.type == "hat_selected" or .type == "dispatch") | "\(.type) hat=\(.hat // .hat_id // "?")"' \
  "$RUN/.ralph/diagnostics/$SES/orchestration.jsonl" | head -60
```

---

## 3. Recovery（workspace + session）

```bash
wc -l "$RUN/.ralph/recovery.jsonl" 2>/dev/null
test -n "$SES" && wc -l "$RUN/.ralph/diagnostics/$SES/recovery.jsonl" 2>/dev/null
jq -r '.source // .envelope.source // "?"' "$RUN/.ralph/recovery.jsonl" 2>/dev/null | sort | uniq -c
```

**0 行 recovery**：可能全程无拒收 — 写 N/A，勿当异常。

---

## 4. OPAC 证据（按模式）

| 模式 | 命令 |
|------|------|
| FULL | `jq` filter `agent-output.jsonl` 的 tool_call |
| LOGS_ONLY | `rg -n 'policy-check|scope violation|inspect loop' "$RUN/.ralph/diagnostics/logs/"` |
| MINIMAL | recovery `payload_contract` + events |

见 [opac-audit-by-mode.md](opac-audit-by-mode.md)。

---

## 5. Phantom 检测

```bash
comm -23 \
  <(jq -r '.topic // empty' "$RUN/.ralph/recovery.jsonl" 2>/dev/null | sort -u) \
  <(jq -r '.topic // empty' "$EVENTS" | sort -u)
```

recovery 有 topic、events 无 → phantom / repair-stream 候选。

---

## 6. 终止与假闭环

| 终态 | 证据 |
|------|------|
| 自然完成 | LOOP_COMPLETE / plan.complete |
| 假闭环 | LOOP_COMPLETE 但 review/fix 未走完（对照 Tier C + events） |
| user quit | `trace.jsonl`（FULL）或 logs 中 Quit/Abort |
| 无终态 | 无 terminal topic + lock/stale |

---

## 7. 三联对账表（报告）

| # | 检查 | events | 第二账本 | 一致 | 备注 |
|---|------|--------|----------|------|------|
| 1 | 首行 work.start | L1 | — | | |
| 2 | 拒收有 recovery | | recovery | | 0行=N/A |
| 3 | 终态 | 末段 | summary | | |

FULL 时增加 orchestration 行；LOGS_ONLY 时第二账本填 `logs` 或 `N/A`。
