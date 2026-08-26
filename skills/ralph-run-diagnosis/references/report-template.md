# 诊断报告模板

落盘：`docs/report/YYYY-MM-DD-<preset-basename>-<loop_id>-diagnosis.md`

参考样板：`docs/report/2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md`

置信度规程：[confidence-rubric.md](confidence-rubric.md)

---

```markdown
---
title: <preset> Loop `<loop_id>` 运行链路诊断报告
date: YYYY-MM-DD
type: diagnosis
loop_id: <loop_id>
preset: builtin:<name> 或 <preset_file>
run_dir: <repo-relative run_dir>
status: <一句话健康度>
diagnostics_mode: FULL | MINIMAL | LOGS_ONLY | DISABLED
bundle: present | finalized | degraded | legacy | missing   # 来自 §0.2 bundle-first 读取
bundle_path: <rel-path-to-diagnosis-input.json>             # 同源；缺失时省略
# U10 plan: causal 归因（DT7 机检，>85 严格门禁）
# 来自 `ralph diagnose --causal` JSON；非 session 视图下省略
causal_status: complete | incomplete | not_evaluable
causal_confidence: <0-100, ralph diagnose --causal.confidence>
causal_primary_domain: runtime | preset | agent | backend | diagnostic_capture_contract
causal_rejected_hypotheses: <list, 4 落选域各自 ≥1 条反驳证据>
causal_score_change: <list of {prev, current, delta, reason}；首次 N/A>
history_search: disabled | preset-only | full   # 来自主 SKILL §0.1 的 AskUserQuestion；默认 disabled
# plan 2026-08-12-001 fix-plan U3: bundle-first 报告配套 frontmatter 字段
# 全部 required: true，缺一即视为模板漂移（grep 校验）
structured_result_ref: "inline: summarized in report"  # JSON 只存在 DIAG_WORKDIR，清理后不在 target branch 留副本
trace_status: present | missing | degraded | legacy        # runtime-trace.jsonl 读取状态
feedback_status: present | missing | degraded | legacy     # feedback.jsonl 读取状态
# plan 2026-08-15-1823 (U3): activation outcome 报告 frontmatter 字段
# 仅记录 presence / classification 状态；具体行表在报告 §4.2 中展示
activation_outcomes: present | missing | degraded | legacy  # runtime-trace.jsonl 中 phase=activation/kind=hat_activation_outcome 行集状态
evidence_gaps: <list of strings>                          # bundle reader / trace reader / feedback reader / activation outcome reader / causal attribution reader 上报的证据缺口
---

# <preset> Loop `<loop_id>` 运行链路诊断报告

> **生成时间**: ...
> **诊断对象**: `<run_dir>/.ralph/`（loop_id=..., 启动 → 终止）
> **对照 preset**: `<preset_file>` + `presets/schemas/<name>.yml`
> **执行方式**: 4 sub-agent 并行（流程还原 / 历史 / 对账 / 归因）→ 汇总；**`history_search=disabled` 时仅 3 个 sub-agent**（Agent B 跳过）
> **Diagnostics 模式**: FULL | MINIMAL | LOGS_ONLY | DISABLED
> **history_search**: `disabled` | `preset-only` | `full`（默认 `disabled`）— 来自主 SKILL §0.1 AskUserQuestion
> **execution_capabilities**: [single-chain | wave | supervisor | supervisor+wave 的子集]（Phase 0 推断结果; 由 `event_loop.supervisor.enabled` / hat `ralph wave emit` / `.ralph/supervisor.db` 存在 / events 含 `wave_id` 等 capability 信号决定; **`ralph inspect loop` 的 `supervisor` 键**在 enabled **或** 盘上已有可打开 wave 账本时出现，先 `has("supervisor")`；**禁止**按 builtin preset 名称点名; 详见 `SKILL.md`「Phase 0 能力推断」段）
> **报告仓库**: `ralph-orchestrator` 主仓（非 run_dir）
> **Tier C 根**: （从 preset+schema 解析）
> **置信度规则**: §5 仅收录 `status == complete`（DT7 机检 confidence > 85）；P0 同样要求 confidence > 85；`status == incomplete` / `not_evaluable` 移入 §7（见 confidence-rubric DT7）

---

## 0. 产物盘点（Phase 0 必附）

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | events（current-events 解析） | | | |
| … | | | | |

**execution_capabilities 推断结果**（Phase 0 必填）: [single-chain / wave / supervisor / supervisor+wave 的子集] — 给出每个 capability 的判定信号 + 证据锚点（如 `event_loop.supervisor.enabled=true` / events 中第一条 `wave_id` 行号 / `.ralph/supervisor.db` 文件 stat）。

**缺失产物 → 故障判定**（capability-triggered）:

- `.ralph/supervisor.db` 缺失 → **仅当** execution_capabilities 含 supervisor 时记为 P0；否则记「N/A (capability 不要求)」。
- events 无 `wave_id` → **仅当** execution_capabilities 含 wave 时记为缺失；否则记「N/A (capability 不要求)」。
- 其它 Tier B 文件缺失按 manifest 既有规则判定（不因本 plan 改变）。

**盲区 / 根因置信度硬顶**：（如 LOGS_ONLY → agent/OPAC 归因 ≤50，整行硬顶 75）

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: （健康 / 部分偏离 / 死锁 / 假闭环 silent-success / …）
- **P0 / P1 / P2 数量**（均为 confidence≥入表门槛）:
- **最高优先级根因置信度**: P0-1 = **NN** / 100
- **历史复发**: 是/否 — 第 N 次 — 引用 `docs/report/...`

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ✅/❌/⚠️ | ... | NN |
| Q2 | 基座机制是否正常生效？ | ✅/❌/⚠️ | ... | NN |
| Q3 | 编排是否合理、正常运行？ | ✅/❌/⚠️ | ... | NN |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | ... | ... | NN（取 §5 主因） |

### 1.3 根因一句话

...（附 **置信度 NN**）

### 1.4 终态时序一致性（event-artifact chronology）

> 强制分栏：先按 accepted event 确定首轮终态，再解释后续 artifact/commit 恢复。禁止用 mutable artifact 反向覆盖先前 accepted verdict。

| 项目 | 内容 |
|------|------|
| **首轮终态（initial_terminal_status）** | 按 accepted audit/report 事件序列判定：首轮成功 / 首轮失败（REJECTED/FAILED/BLOCKED） / 证据不足 |
| **恢复状态（recovery_status）** | 无恢复 / 失败终态后恢复（artifact 被改但无后续 accepted 成功事件） / 恢复后成功（有后续 accepted 成功事件） |
| **最终代码状态（final_code_state）** | 按最终 Git diff / artifact 内容描述（仅陈述事实，不反写 event verdict） |
| **一致性告警** | 若存在失败终态后恢复：输出「⚠️ 失败终态后恢复：首轮 audit/report 为 REJECTED/FAILED，后续 artifact 被修改但无对应 accepted 成功事件」；**禁止**输出「零拒收」或「首轮完整成功」 |

---

## 2. 执行链路对比图

（粘贴 Agent A 输出：拓扑表 + 时间轴 + mermaid）

---

## 3. 历史问题上下文

> **⚠️ 启用条件**：`history_search=disabled`（默认）下，**不启动 Agent B**，由主 Agent 在合成阶段直接写入 §0.1-占位符（字面见 [SKILL.md § SSOT](SKILL.md#01-历史检索开关hard-rule)）；`preset-only` / `full` 才走下文 schema，且 §3 末尾必须含一行 `本次扫描窗口：<preset-only (30d sliding) | full (full-history)>`（Agent B 自填；disabled 模式不写）。

（粘贴 Agent B：关联度表 + 复发对照）

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度 | DT7 分项来源 | 缺口 |
|----|------|----------|--------|--------------|------|
| DEV-001 | ... | file:line / event#L | P0 | coverage(+30) / integrity(+25) / correlation(+15) | freeze_window 缺 |

### 4.1 OPAC 逐 hat 审计表

| Hat | O | P | A | C | 证据 | 置信度 |
|-----|---|---|---|---|------|--------|

### 4.2 Activation outcome 表（plan 2026-08-15-1823）

> **⚠️ 启动条件**：仅当 frontmatter `activation_outcomes: present` 时填写本节。`missing` / `degraded` / `legacy` 时整节写 `N/A (activation outcomes unavailable)`，并把缺失原因写进 `evidence_gaps`。

| sequence | hat | status | backend_exit_code | watchdog | merge_succeeded | channel_bytes | terminal_obligation | classification | confidence | evidence_refs | notes |
|----------|-----|--------|-------------------|----------|-----------------|---------------|---------------------|----------------|------------|---------------|-------|

**列含义**：

- `sequence`：`runtime-trace.jsonl` 内的单调序号。
- `status`：`merged` / `empty` / `missing` / `unreadable` / `merge_failed` / `interrupted`。
- `classification`：`timeout_or_termination` / `backend_failure` / `channel_routing_failure` / `attempted_but_rejected` / `successful_no_terminal_emit` / `unknown`。
- `confidence`：计分卡打分；`unknown` 一律 confidence<60。
- `evidence_refs`：与第二账本交叉验证的 `file:line` 或 `recovery.jsonl:L<N>` 或 `events.jsonl:L<N>`。
- `notes`：evidence gap 或 raw facts 摘录（不复制完整 output / prompt）。

**禁止**：

- 凭 `status=empty` 单值写 agent 根因；必须满足 `terminal_obligation + 无 accepted/rejected candidate + recovery 一致` 三条同时成立。
- 凭 activation outcome row 跳过 L6 源码反查。
- 在 `activation_outcomes: missing` / `legacy` 时仍填写本节——直接 N/A。

### 4.3 Causal Attribution（plan 2026-08-26-1104, U10）

> **⚠️ 启动条件**：仅当 frontmatter `causal_status` ∈ {`complete`, `incomplete`} 时填写本节。`not_evaluable`（legacy / v1 / 无契约）时整节写 `N/A (causal attribution unavailable)`，并把缺失原因写进 `evidence_gaps`。

`ralph diagnose --causal` 是归因事实唯一来源；agent 不另行打分。

#### 4.3.1 DT7 分项 + 总置信度

| DT7 项 | 分值 | 实测值（来自 `--causal`） | 来源 |
|--------|------|---------------------------|------|
| coverage | +30 | `<covered 边界数 / 8>` | `diagnosis-input.json` `boundary_coverage[]` |
| integrity | +25 | `<join 一致计数 / 应一致计数>` | 三类收据 + ledger |
| refutation | +20 | `<4 落选域反驳条数 / 4>` | `rejected_hypotheses[]` |
| correlation | +15 | `<contract_digest 一致 / sequence 单调 / retry_key 对账 三项布尔>` | `runtime-trace.jsonl` |
| freeze_window | +10 | `<evidence-window.jsonl 是否存在 + 首行 anomaly>` | `<session>/evidence-window.jsonl` |
| **总置信度** | **max 100** | **`<--causal JSON .confidence>`** | `ralph diagnose --causal` |

#### 4.3.2 被否决假设（rejected_hypotheses）

| 落选域 | 反驳证据类型 | 反驳证据引用 |
|--------|----------------|----------------|
| runtime | ... | `event:L<N>` / `recovery.jsonl:L<N>` |
| agent | ... | ... |
| backend | ... | ... |
| diagnostic_capture_contract | ... | ... |

> 仅记录 `primary_domain` 之外 4 个落选域；域枚举固定为 `runtime / preset / agent / backend / diagnostic_capture_contract`，**禁止**扩展。证据引用仅用 `file:line` 或 `*.jsonl:L<N>`，不复述具体 payload（per HARD RULE 4.8）。

#### 4.3.3 分数变化（causal_score_change）

| 重新打分原因 | 上次 total | 本次 total | Δ | primary_domain 是否变化 | 落选域反驳新增 |
|--------------|------------|------------|---|--------------------------|------------------|
| 加深：补 read 第二账本 | 70 | 95 | +25 | 否 | 无 |
| 首次打分 | N/A (initial scoring) | 100 | — | — | — |

> 禁止在分数变化小节捏造上次分数；首次写 `N/A (initial scoring)`。

---

## 5. 问题归因表（DT7 机检，confidence > 85）

| 优先级 | 问题 | primary_domain | status | confidence | 证据 DEV | DT7 分项来源 | rejected_hypotheses | 历史关联 | 加深轮次 |
|--------|------|----------------|--------|------------|----------|--------------|---------------------|----------|----------|
| P0 | ... | preset | complete | 95 | DEV-00x | coverage / integrity / correlation / refutation / freeze_window | 4 落选域 | 高 | 1→95 |
| P1 | ... | runtime | complete | 88 | DEV-00y | coverage / integrity / correlation / refutation | 4 落选域 | N/A (history disabled) | 0 |

> **历史关联列规则**：`history_search=disabled`（默认）一律 `N/A (history disabled)`；`preset-only` / `full` 才填高/中/低/新。
>
> **status 列规则**（U10 DT7）：仅 `status == complete`（confidence > 85）入表；`status == incomplete` / `not_evaluable` 移入 §7，不得入 §5 / §6。`primary_domain` 枚举固定 `runtime / preset / agent / backend / diagnostic_capture_contract`，**禁止**扩展。`rejected_hypotheses` 列每行固定 4 条（4 落选域各自 ≥1 条反驳），不全填视为工具漂移。

---

## 6. 修复建议

（仅针对 §5 已入表项；§7 疑点不得写修复）

### 6.1 短期（operator workaround）

### 6.2 中期（preset / schema / instructions）

### 6.3 长期（机制 / 底座）

每条：目标 | 改动 | 预期效果 | **关联置信度**

---

## 7. 未核实疑点（可选）

confidence < 60 且已加深 2 轮仍不足；**不驱动修复**。

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|----------|------------|------------|----------|
| ... | 48 | 缺 agent-output | recovery+logs 已查 |
```

---

## 质量门槛

- §1 四问 **不可省略**；Q1–Q4 均有 **置信度** 列。
- §5 **每条 P0/P1 必有 `status` + `confidence`**；无 `status == incomplete` / `not_evaluable` 行；P0 / P1 confidence 均 > 85。
- 每条 P0 至少一条 DEV +（DT7 任一分项来源 + 反驳证据）。
- `primary_domain` 枚举固定 `runtime / preset / agent / backend / diagnostic_capture_contract`，**禁止**扩展。
- `rejected_hypotheses` 每行固定 4 条（4 落选域各自 ≥1 条反驳），不全填视为漂移。
- 低置信度由 [confidence-rubric.md](confidence-rubric.md) DT7 重打分流程处理（不允许手算补分）。
- 路径一律 **repo-relative**。
- frontmatter 必含 `history_search: <disabled | preset-only | full>`（默认 `disabled`，与执行实际一致）。
- frontmatter 必含 `causal_status` / `causal_confidence` / `causal_primary_domain` / `causal_rejected_hypotheses` / `causal_score_change`（session 视图）。
