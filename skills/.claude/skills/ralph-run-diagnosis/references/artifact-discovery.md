# 产物发现规程（Phase 0 强制）

Sub-agent **之前**完成；输出《产物盘点表》+ Diagnostics 模式 + Tier C 预期清单。

---

## Step 0：解析 preset 路径

```bash
# 若用户给 builtin:ce-executor-serial
ARG="builtin:ce-executor-serial"
case "$ARG" in
  builtin:*) PRESET="presets/en/${ARG#builtin:}.yml" ;;
  *)         PRESET="$ARG" ;;
esac
SCHEMA="presets/schemas/$(basename "$PRESET" .yml).yml"
RUN=<run_dir>   # 含 .ralph/ 的 workspace 根
REPO=<ralph-orchestrator 主仓根>  # 报告写 REPO/docs/report/
```

`preset_file` / schema 路径相对 **主仓**；产物路径相对 **RUN**。

---

## Step 1：物理盘点

```bash
tree -a "$RUN/.ralph" 2>/dev/null | head -100
tree "$RUN/.agents" 2>/dev/null | head -40
test -f "$RUN/ralph.yml" && echo "HAS ralph.yml" || echo "NO ralph.yml"
```

---

## Step 2：可信 events 指针链（禁止 events*.jsonl 通配）

```bash
MARKER="$RUN/.ralph/current-events"
EVENTS_REL=$(tr -d '\n' < "$MARKER")
case "$EVENTS_REL" in
  /*) EVENTS="$EVENTS_REL" ;;
  *)  EVENTS="$RUN/$EVENTS_REL" ;;
esac
echo "trusted: $EVENTS"
wc -l "$EVENTS"
# 配对 history：events-XXX.jsonl → events-history-XXX.jsonl
HIST="${EVENTS/events-/events-history-}"
test -f "$HIST" && wc -l "$HIST" || echo "no paired events-history"

head -1 "$EVENTS" | jq '{topic, source}'
tail -3 "$EVENTS" | jq '{topic, hat: (.hat // .source_hat // null)}'
```

`loop_id`：文件名 / `current-loop-id` / `loops.json`。

---

## Step 3：Diagnostics 四档

```bash
SES=$(ls -1t "$RUN/.ralph/diagnostics/" 2>/dev/null | grep -E '^[0-9]{4}-' | head -1)
if test -n "$SES" && test -f "$RUN/.ralph/diagnostics/$SES/orchestration.jsonl"; then
  MODE=FULL
elif test -n "$SES"; then
  MODE=MINIMAL
elif test -d "$RUN/.ralph/diagnostics/logs" && ls "$RUN/.ralph/diagnostics/logs/" 2>/dev/null | grep -q .; then
  MODE=LOGS_ONLY
else
  MODE=DISABLED
fi
echo "diagnostics_mode=$MODE"
```

FULL/MINIMAL 时列出 session 内文件存在性（orchestration、agent-output、recovery、drift、diagnosis-summary）。

---

## Step 4：Tier C 预期（从 preset + schema）

```bash
rg -n 'specs_dir|scratchpad|\.agents/|docs/plans/' "$REPO/$PRESET" "$REPO/$SCHEMA" 2>/dev/null | head -20
# 列出 preset 引用的文件名关键词
rg -o '[a-z_-]+\.(json|md|patch)' "$REPO/$PRESET" | sort -u | head -30
```

在 `$RUN` 下勾差实际存在文件；未跑到的阶段标「未触发」勿标「丢失」。

---

## Step 5：状态快照

```bash
wc -l "$RUN/.ralph/ledger.jsonl" "$RUN/.ralph/recovery.jsonl" "$RUN/.ralph/agent/tasks.jsonl" 2>/dev/null
test -f "$RUN/.ralph/loop.lock" && echo "LOCK_HELD" || echo "lock_released"
```

---

## Step 6：《产物盘点表》

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | events（指针解析后） | | | |
| S | recovery.jsonl | | 0行=无拒收 | |
| A | tasks.jsonl | | | tasks.enabled? |
| B | diagnostics mode | | | FULL/MINIMAL/LOGS_ONLY |
| C | （逐条） | | | |

**门禁**：

- 缺 `current-events` 或指向文件 → **停止**
- LOGS_ONLY → 报告必须含 OPAC 降级声明（见 [opac-audit-by-mode.md](opac-audit-by-mode.md)）
- 报告写入 **`$REPO/docs/report/`**（非 run_dir）
