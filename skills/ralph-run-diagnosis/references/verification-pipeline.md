# 七层校验流水线（强制）

诊断**必须按层顺序执行**。上一层未通过的门禁，不得在报告中写「健康」。每层产出写入工作笔记，最终汇入报告对应章节。

```
L0 盘点 → L1 拓扑 → L2 日志三联 → L3 产物五证 → L4 机制十二项 → L5 历史深挖 → L6 源码反查 → L7 归因落盘
```

---

## L0 — 运行盘点（Gate: 知道「评的是什么」）

**必须先做**：[artifact-discovery.md](artifact-discovery.md) → 《产物盘点表》。

**按 Tier 读**（详见 [artifact-manifest.md](artifact-manifest.md)）：

| Tier | 最少读什么 |
|------|------------|
| **S 基座** | `current-events` → events + events-history、`ledger`、`recovery`、`loops`、`logs/*.log` |
| **A 状态** | `tasks.jsonl`、`progress.md`、`summary.md` |
| **B 条件** | 盘点表勾选：session、hat-channel、`ralph.yml`… |
| **C preset** | 从 preset 解析 scratchpad/plan 路径，勾差预期文件 |

另：主仓 git sha、`preset_file`、`presets/schemas/`、diagnostics 模式 FULL/MINIMAL/LOGS_ONLY/DISABLED。

**产出**：《Run 元数据表》+ 《产物盘点表》+ diagnostics 盲区声明（若无 full session）。

**能力字段（强制）**：L0 必须带出 Phase 0 的 **`execution_capabilities`**（见 [artifact-discovery.md](artifact-discovery.md) Step 5b 与主 skill「Phase 0 能力推断」）。后续 L3/L4 对 `.ralph/supervisor.db` / `wave_id` 的缺失判定一律 capability-triggered；单链或缺能力信号时不得把缺 db / 缺 wave_id 标为故障。

**门禁**：Tier S 的 `current-events` 或指向的 events 缺失 → **停止**。未写出 `execution_capabilities` → **不得**进入把缺 supervisor.db / wave_id 当故障的归因。

---

## L1 — 编排拓扑（Gate: 预期链路已建模）

**步骤**

1. 从 preset 提取 hat DAG：`triggers` / `publishes` / `execution_mode` / `state_projection` / `mechanism.flow`（若有）。
2. 从 schema 提取：合法 topic、`required_fields`、`execution_contracts`、`topic_deny_rules`。
3. 若有 BDD：`crates/ralph-core/tests/scenarios/<preset>*.yml` 的 `expected.events` 作为第三份预期。
4. 从实际 events 重建**业务事件时间轴**（排除 `loop.*` / `agent.*` 系统事件或单独标注）。
5. 画 **预期 vs 实际** 表 + mermaid；每 hat 标注激活次数、未触发原因。

**产出**：Agent A《执行链路对比图》。

**门禁**：未列出「未触发 hat」及上游缺失事件 → 不得进入 L3。

---

## L2 — 日志三联对账（Gate: 模式适用）

见 [log-reconciliation.md](log-reconciliation.md)。**LOGS_ONLY 跳过 orchestration 行**；不得将「无 orchestration」标为机制 P0。

| 模式 | 必做 |
|------|------|
| FULL | events ↔ orchestration ↔ recovery |
| MINIMAL | events ↔ session recovery |
| LOGS_ONLY | events ↔ workspace recovery ↔ logs |

---

## L3 — 产物五证（Gate: 磁盘状态与事件一致）

五类产物必须与 events **逐项对齐**：

| 证 | 路径 | 对账要点 |
|----|------|----------|
| **Task** | `agent/tasks.jsonl` | open/closed、task_id 不复用、三字段与 payload、loop_id |
| **Handoff** | `agent/handoff.md`（终止）；tasks ↔ progress（step_handoff） | 续跑上下文 / step 对齐 |
| **Progress** | `agent/progress.md` | Current Step vs 最后 work.ready step |
| **Review/Fix** | findings*、fix-log、scratchpad | 与 review 事件 findings_count 一致 |
| **Terminal** | summary.md、report、plan frontmatter | status 与 task 闭合、终态语义 |

额外（路径须存在于盘点表）：

- `.ralph/agent/.ralph-enforce-current-unit`（`enforce_current_unit`）
- hat-channel：`events-hat-{hat}-{loop_id}-{iter}.jsonl`
- Tier C：`review-sequence.json` 等在 `.agents/...`（非 `.ralph/`）
- `run_dir/ralph.yml`、`git log` / `git diff`

**capability 条件对账（L3）**：

| 条件 | 动作 |
|------|------|
| `execution_capabilities` 含 supervisor | 勾差 `.ralph/supervisor.db`；缺 → 记缺失（runtime） |
| 不含 supervisor | 缺 `supervisor.db` → **N/A**，非故障 |
| `execution_capabilities` 含 wave，或 events 已见 `wave_id` | worker/dispatcher Confirm 走 `ralph events --events-source main`（main ledger）；**禁止**用 hat-channel 做 wave Confirm |
| 不含 wave | events 无 `wave_id` → **N/A**，非故障 |

**产出**：Agent C《偏离证据清单》DEV-xxx + §3.4 OPAC/机制行为表。

---

## L4 — 机制十二项（Gate: 基座逐项验收）

对 [mechanism-checklist.md](mechanism-checklist.md) **每一项**填 ✅/❌/N/A + 证据。至少覆盖：

1. Event origin guard / hat scope
2. Payload contract（schema）
3. Execution contract（git_change、task 绑定）
4. Workflow guard / phase
5. Isolated 单事件预算
6. step_handoff（tasks ↔ progress）+ semantic_gate_violation
7. Recovery 升级（Soft→Hard→Final）
8. loop.resume / task.resume 消费者
9. Stall / progressive_failure / loop_stale
10. Drift monitor（三指标）
11. Dedup / duplicate_work_done
12. Terminal / completion_after_terminal / silent-success 检测
13. Event-artifact temporal consistency（终态时序一致性）：accepted event chronology 是终态事实源；artifact 被后续修改但无对应 accepted 成功事件时，必须标记「失败终态后恢复」，不得输出「零拒收」

**产出**：《机制生效矩阵》— 生效 / 失效 / 未触发 + 文件:行号。

**门禁**：§1 问题 2（机制是否生效）必须引用本矩阵，禁止只写「大部分正常」。

---

## L5 — 历史深挖（Gate: 仅在 --include-history ≠ disabled 时执行）

> **⚠️ 启动条件**：与 Agent B 同步；`--include-history=disabled`（默认）下**跳过** L5，报告 §3 与 §5"历史关联"列一律写 §0.1-占位符（字面见 [SKILL.md § SSOT](SKILL.md#01-历史检索开关hard-rule)）。详见主 SKILL §0.1。

**执行的检索**（见 [history-sources.md](history-sources.md)）：

1. `docs/report/*<preset>*diagnosis*.md` — 30 天窗口（preset-only）/ 全库（full）
2. `docs/solutions/integration-issues/` + `logic-errors/` + `state-management/` — grep 症状关键词
3. `docs/plans/` — `status: active` 且与 symptom 相关的 plan
4. `docs/brainstorms/*.md`（最近 N 篇根因讨论） — Top 根因对照
5. 对每条 P0：在 solutions 搜是否已有 fix；plan 是否 merged

> **文件存在性提醒**：第 2-4 项所列目录中真实存在哪些文档，因时间推移会变动；执行前先 `ls` 确认目标文件存在，**禁止假设路径有效**。

**产出**：Agent B《历史知识库》+ §8 历史 run 对照表 + 「第 N 次复发」判定。

**门禁**：仅在 `--include-history ≠ disabled` 时才检查；P0 未标注历史关联度（高/中/低/新）→ 报告不合格。`disabled` 模式下不检查此项。

---

## L6 — 源码反查（Gate: 机制归因已落到行号）

凡 DEV 标为 `mechanism` 或 `compound` 含机制成分，**必须**走 [source-trace-guide.md](source-trace-guide.md)：

1. 从 `reason_code` / recovery `source` 定位 Rust 入口
2. `sed -n` 读相关函数，确认是 bug 还是 by-design
3. preset 归因：读 `preset_file` 具体行 + 对应 `presets/schemas` 规则
4. 产出 §7《关键主仓代码引用清单》

**禁止**：只写「event_policy 有问题」不写 `file:line`。

---

## L7 — 归因、置信度与四问落盘

Agent D 输出 P0/P1/P2 + **每条置信度** + 修复依赖序；低分须已完成 [confidence-rubric.md](confidence-rubric.md) 加深或落入 §7。

主 Agent 写 §1《用户四问明确回答》— 逐问含 **置信度**、逐 hat OPAC、机制矩阵、编排判定、归因分类。

**最终门禁**（全部满足才可提交）：

- [ ] L0–L6 每层有产出（可合并进报告章节；`--include-history=disabled` 时 L5 标 `N/A`）
- [ ] §5 每条 P0/P1 有 **置信度**；P0≥70、入表≥60
- [ ] confidence<60 项仅在 §7 或已加深达标，未混入 §5/§6
- [ ] P0 每条有 DEV + 源码或 preset 行号
- [ ] 日志三联至少 5 行对账
- [ ] 历史表 ≥3 行（仅在 `--include-history ≠ disabled`）/ `disabled` 时显式 `N/A (history disabled)`
- [ ] 报告路径 `docs/report/...-diagnosis.md` 已写入
- [ ] frontmatter 含 `history_search: <disabled | preset-only | full>`
- [ ] 已执行下方"frontmatter 对账"机器校验脚本并通过

### frontmatter 对账（机器校验，hard rule）

下列 jq 命令读报告文件 frontmatter，与本次执行参数 `$RALPH_INCLUDE_HISTORY`（或 `--include-history` 入参）比对。**未通过则报告不合格**（即使其它项已 ✓）。

```bash
# 历史检索开关必须可在执行环境复现（如未设 RALPH_INCLUDE_HISTORY 则按 disabled 兜底）
: "${RALPH_INCLUDE_HISTORY:=disabled}"

REPORT="${REPORT:-docs/report/$(date +%Y-%m-%d)-<preset>-<loop_id>-diagnosis.md}"

# 提取 frontmatter 中 history_search 的值（解析 ---…--- 包围的 YAML 块）
HS=$(awk 'BEGIN{f=0} /^---$/{n++; next} n==1 && /^history_search:/{print $2; exit}' "$REPORT")
HS="${HS:-missing}"

# 1) 与执行参数一致
if [ "$HS" != "$RALPH_INCLUDE_HISTORY" ]; then
  echo "FAIL: --include-history=$RALPH_INCLUDE_HISTORY 与 report frontmatter history_search=$HS 不一致" >&2
  exit 1
fi

# 2) disabled 模式下 §5 历史关联列必须含 §0.1-占位符
if [ "$RALPH_INCLUDE_HISTORY" = "disabled" ]; then
  NA_COUNT=$(grep -cE '\| N/A \(history disabled\) \|' "$REPORT")
  if [ "$NA_COUNT" -eq 0 ]; then
    echo "FAIL: disabled 模式下报告 §5 应含 'N/A (history disabled)' 占位行" >&2
    exit 1
  fi
fi

echo "OK: history_search=$HS, 占位符=$NA_COUNT"
```

> **何时必须跑**：① L7 最终门禁前；② `git commit` 诊断报告时。**禁止只跑人工走读不跑此命令**（这是 P2-TEST-001 的合规底线）。

### §3 历史关联一致性（P3-TEST-002 落地）

Agent B 在 `[full | preset-only]` 模式下，**报告 §3 末尾必须含一行**：`本次扫描窗口：<preset-only (30d sliding) | full (full-history)>`。这条作为事后 audit trail；缺失则 Agent B 未完成。
