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
REPO=<ralph-orchestrator 主仓根>  # 仅最终报告写 REPO/docs/report/
DIAG_WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/ralph-diagnosis.XXXXXX")"  # 所有中间产物
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

FULL/MINIMAL 时列出 session 内文件存在性（orchestration、agent-output、recovery、drift、diagnosis-summary、**diagnosis-input.json**、**runtime-trace.jsonl**、**feedback.jsonl**、**evidence-window.jsonl**）。其中 `diagnosis-input.json` 的 v2 manifest 是 causal 诊断的入口；只有 manifest 的 8 个 boundary 均可验证时，才允许进入可归因评分。

> **Bundle-first（plan 2026-08-12-001）**：`diagnosis-input.json` 是新 bundle 的入口；`runtime-trace.jsonl` 与 `feedback.jsonl` 是它的 sidecar。三者均按 §0.2 顺序读取；缺失则回退 legacy Tier 路径。

> **Activation outcome（plan 2026-08-15-1823）**：`runtime-trace.jsonl` 内 `phase=activation` / `kind=hat_activation_outcome` 行按 session 对应盘点；缺行集时报告 §0 标 `activation_outcomes: missing`（FULL/MINIMAL 时）或 `legacy`（缺 bundle 时）；**不**列为 P0——activation outcome 是 additive sidecar，老 session / 非 isolated run 自然缺。盘点时一并统计 `empty` / `merged` / `merge_failed` / `interrupted` / `missing` / `unreadable` 计数与首个非 merged 行（若有）的 `hat` / `status` / `source_ref`。

> **Causal evidence**：异常运行还应检查 `evidence-window.jsonl`。该文件只包含 anomaly 首行和受容量限制的上下文；缺失或无法按当前 `loop_id` 关联的 runtime receipt 必须降低 causal confidence，不得作为当前运行的证据。

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

## Step 5b：执行能力推断（execution_capabilities）

在写《产物盘点表》之前，按主 skill「Phase 0 能力推断」段产出 `execution_capabilities`（字符串数组）。检测信号冻结在 [`../../ralph-preset-review/references/agent-native-model.md`](../../ralph-preset-review/references/agent-native-model.md)「执行模型（Execution Model）」段：

| 信号 | → capability / Observe |
|------|----------------|
| `event_loop.supervisor.enabled: true` | +supervisor |
| hat `instructions` 含 `ralph wave emit` / `ralph wave verify`，或 `## WAVE CONTEXT` | +wave |
| events 含 `wave_id` | +wave（产物侧） |
| `.ralph/supervisor.db` 存在 | ledger 证据：YAML 已 enabled 时加固 +supervisor；enabled=false 时仍可能让 `ralph inspect loop` JSON 出现 `supervisor` 键（default-wave）——先 `has("supervisor")`，**不要**用 enabled=false 断言无键 |

**硬规则**：

- **禁止**用 `exec.wave.*` / `slot.*` 推断 +wave（协调 topic ≠ wave fan-out）。
- 能力集合为单链（或不含 supervisor/wave）时：缺 `.ralph/supervisor.db`、events 无 `wave_id` 均为**预期**，**不**标故障。
- `inspect` 的 `supervisor` 块门控 = **enabled 或盘上已有可打开 wave 账本**（与注入 skill `ralph-tools-opac` 一致）；诊断时以 JSON 是否含键为准，禁止手读 ledger 文件内容。
- 将 `execution_capabilities` 写入报告 §0（见 [report-template.md](report-template.md)）。

---

## Step 6：《产物盘点表》

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | events（指针解析后） | | | 扫 `wave_id` 作 capability 信号 |
| S | recovery.jsonl | | 0行=无拒收 | |
| A | tasks.jsonl | | | tasks.enabled? |
| B | diagnostics mode | | | FULL/MINIMAL/LOGS_ONLY |
| B | `.ralph/supervisor.db` | | | **仅** capability +supervisor 时缺则记缺失；否则 N/A |
| C | （逐条） | | | |

盘点表上方必填一行：`execution_capabilities: [...]`（Phase 0 / Step 5b 结果）。

**门禁**：

- 缺 `current-events` 或指向文件 → **停止**
- LOGS_ONLY → 报告必须含 OPAC 降级声明（见 [opac-audit-by-mode.md](opac-audit-by-mode.md)）
- 最终报告写入 **`$REPO/docs/report/`**（非 run_dir）；JSON、stderr、工作笔记和临时清单必须写入 `$DIAG_WORKDIR`，不得写入 target branch
- 结束时清理 `$DIAG_WORKDIR`；清理失败必须报告残留路径
- 未声明 `execution_capabilities` → **不得**把缺 `supervisor.db` / 缺 `wave_id` 写成故障
