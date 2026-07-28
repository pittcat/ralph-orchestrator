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
history_search: disabled | preset-only | full   # 来自主 SKILL §0.1 的 AskUserQuestion；默认 disabled
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
> **置信度规则**: §5 仅收录 confidence≥60；P0 须 confidence≥70（见 confidence-rubric）

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

---

## 2. 执行链路对比图

（粘贴 Agent A 输出：拓扑表 + 时间轴 + mermaid）

---

## 3. 历史问题上下文

> **⚠️ 启用条件**：`history_search=disabled`（默认）下，**不启动 Agent B**，由主 Agent 在合成阶段直接写入 §0.1-占位符（字面见 [SKILL.md § SSOT](SKILL.md#01-历史检索开关hard-rule)）；`preset-only` / `full` 才走下文 schema，且 §3 末尾必须含一行 `本次扫描窗口：<preset-only (30d sliding) | full (full-history)>`（Agent B 自填；disabled 模式不写）。

（粘贴 Agent B：关联度表 + 复发对照）

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|----|------|----------|------------|------------|--------------|----------|
| DEV-001 | ... | file:line / event#L | P0 | 40 | （无） | 缺 file:line、缺双账本 |

### 4.1 OPAC 逐 hat 审计表

| Hat | O | P | A | C | 证据 | 置信度 |
|-----|---|---|---|---|------|--------|

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|--------------|----------|----------|
| P0 | ... | mechanism / preset / agent / compound | **82** | DEV-00x | file:line(+25) + 双账本(+20) + preset行号(+15) + BDD(+10) | 高 | 1→82 |
| P1 | ... | preset | **65** | DEV-00y | preset行号(+15) + 单账本 | N/A (history disabled) | 0 |

> **历史关联列规则**：`history_search=disabled`（默认）一律 `N/A (history disabled)`；`preset-only` / `full` 才填高/中/低/新。

**compound 行须附**：成分 A(conf%) + 成分 B(conf%) → 整行置信度 = min 或加权公式。

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
- §5 **每条 P0/P1 必有置信度**；无 < 60 行；P0 无 < 70 行。
- 每条 P0 至少一条 DEV +（mechanism）源码行号。
- `compound` 须写贡献比例 + 各成分置信度。
- 低置信度须走 [confidence-rubric.md](confidence-rubric.md) 加深流程并记录轮次。
- 路径一律 **repo-relative**。
- frontmatter 必含 `history_search: <disabled | preset-only | full>`（默认 `disabled`，与执行实际一致）。
