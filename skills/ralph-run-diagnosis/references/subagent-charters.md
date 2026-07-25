# Sub-Agent Charters（增强版）

各 agent **只负责本 charter**；输出 Markdown 片段供主 Agent 汇入最终报告。**全局约束**（适用于所有 sub-agent，源自主 SKILL）与**字面 SSOT** 见 [SKILL.md](../SKILL.md)「§0.1」末段表。

---

## Agent A — 流程还原

> **全局约束（适用于所有 sub-agent，源自主 SKILL）**：
> - Phase 0 完成前**禁止**启动任何 sub-agent（主 SKILL「执行顺序」段）。
> - `--include-history=disabled`（默认）下，Agent A / C / D **禁止读** `docs/report/`、`docs/solutions/`、`docs/plans/`、`docs/brainstorms/`（即便不出现在产物中也禁止扫描）。仅 Agent B 与 L5 才会触碰上述目录。

**输入**：《产物盘点表》、`current-events` 指向的 events 文件（禁止 `events*.jsonl` 通配）、preset、schema、BDD。

**步骤**：

1. 从 preset 提取：hat 列表、`triggers`/`publishes`、`execution_mode`、terminal events、step 语义。
2. 从 schema 提取：预期事件序列、`required_fields`、topic_deny_rules。
3. 从实际产物重建时间轴：每条业务事件附 timestamp、hat、payload 摘要。
4. 绘制 **预期 vs 实际** 链路（表格 + 可选 mermaid）；每步标记 ✅ / ❌ / ⏸️ / ⚠️。
5. 标注 **终止类型** 与 **未触发 hat**。

**输出**：《执行链路对比图》

- §2.1 拓扑激活表（每个 hat 激活次数）
- §2.2 时间轴对比表
- §2.3 可选 mermaid（偏离处标红/橙）

**禁止**：归因、历史、修复建议。

---

## Agent B — 历史上下文

> **⚠️ 启动条件**：仅当主流程 `--include-history ≠ disabled`（默认 disabled）时才启动 Agent B；否则**跳过**，报告 §3 写 §0.1-占位符（字面见 [SKILL.md § SSOT](../SKILL.md#01-历史检索开关hard-rule)）。

**输入**：[history-sources.md](history-sources.md) 所列目录；本次 preset 名、loop 症状关键词；`--include-history` 当前值。

**步骤**：

1. **窗口收窄（hard rule）**：
   - `--include-history=preset-only` → 仅扫与本次 `preset` / `loop_id` 关键词相近的近 30 天条目；
   - `--include-history=full` → 同 1 但窗口扩全库；
   - **禁止** `disabled` 下再启 Agent B。
2. 扫描历史诊断报告（`docs/report/*-diagnosis.md`）中与 preset / 症状相近的条目。
3. 扫描 `docs/solutions/`、`docs/brainstorms/` 中机制/preset 修复记录。
4. 按 **问题类型** 分类（如 `task.resume` 死信、silent-success、loop_anchor、duplicate_work_done、OPAC 缺 precheck）。
5. 每条记录：文档路径、出现次数、是否闭环、与本次关联度（高/中/低）。

**输出**：《历史问题知识库》

- 全景表（类型 × 次数 × 本次关联 × 闭环状态）
- 根因分类对照（若历史报告有）
- **即使无关联**也输出「本次为新问题模式」
- **窗口注脚（hard rule）**：报告 §3 末尾必须含 1 行 `本次扫描窗口：preset-only (30d sliding) | full (full-history)`，便于事后审计；**禁止不写或写 "N/A"**。

**禁止**：对本次 run 做归因（留给 D）。

**目录边界（hard rule）**：Agent B 仅可读以下子目录（其它一律禁止）：

- `docs/report/*-diagnosis.md`
- `docs/solutions/{integration-issues,logic-errors,state-management}/`
- `docs/plans/` — `status: active` 子集
- `docs/brainstorms/*.md`

**禁止读**：`.ralph/`、`docs/achieved/plan/` 旧归档子集（preset-only 模式外）、任何不在上列的子目录；如有遗漏目录需读，必须先记录到 §3 注脚，由主 Agent 评估是否升级到 `full`。

**禁用条件**：与主 SKILL [§0.1 HARD RULE](SKILL.md#01-历史检索开关hard-rule)「禁用默认开启的理由」三点一致——跨仓越权、跨 preset 误归因、扫描无上限；本 charter 不再复述，**以主 SKILL §0.1 为唯一事实源**。

---

## Agent C — 对账分析

**输入**：Agent A 链路图、Agent B 知识库、《产物盘点表》中 **实际存在** 的 Tier S/A/B/C 文件（禁止读不存在的路径凑数）。

**步骤**：

1. **事件对账**：每个实际 event vs schema required_fields、preset publishes 权限。
2. **Hat 触发对账**：未激活 hat 是否因上游缺失；多激活是否 duplicate_work_done。
3. **Task 对账**：`tasks.jsonl` open/closed vs events step；三字段一致性。
4. **Session handoff + step_handoff**：`handoff.md`（终止后）；`tasks.jsonl` ↔ `progress.md` 对齐（见 [ssot-guardrails.md](ssot-guardrails.md) 术语表）。
5. **Recovery 对账**：`outcome` 升级；`loop.resume`/`task.resume` 是否有消费者。
6. **OPAC**（按 diagnostics 模式，见 [opac-audit-by-mode.md](opac-audit-by-mode.md)）。
7. **R1-R6**（[mechanism-checklist.md](mechanism-checklist.md)）。

### OPAC 表

含 **置信度** 列；LOGS_ONLY 下单列缺失不得单独 P0。

**输出**：《偏离证据清单》

- 编号 DEV-001…
- 每条：描述、严重度初判、**置信度初估**、证据锚点（file:行号或 event#）、**证据缺口**、关联 A 链路步骤

见 [confidence-rubric.md](confidence-rubric.md) §Agent C。

**禁止**：最终根因分类（留给 D）；可标「疑似 mechanism」但不定论。

---

## Agent D — 归因、置信度与修复

**输入**：Agent C 偏离清单、Agent B 知识库、主仓源码（按需）、[confidence-rubric.md](confidence-rubric.md)。

**步骤**：

1. 逐条 DEV 判定根因：`preset` / `mechanism` / `agent` / `compound`。
2. 为每条 P0/P1 打 **confidence 0–100**；附评分依据（双账本、file:line、preset 行号等）。
3. **confidence < 60**（P0 候选 **< 70**）→ **禁止入 §5**；执行 rubric 加深顺序，记录轮次与分数变化（最多 2 轮）。
4. 2 轮后仍不足 → 写入 §7「未核实疑点」+ `blocked_by`。
5. 对 `mechanism` 必须 `file:line` 才能 confidence ≥ 70。
6. 对 `preset` 必须 preset/schema **具体行号** 才能 confidence ≥ 65。
7. 对照 B：历史复发、第几次、未落地 plan。
8. 仅对 §5 入表项写 P0/P1/P2 与三段式修复建议。

**输出**：

- 《问题归因表》（含 **置信度**、加深轮次）
- 《未核实疑点表》（若有）
- 《修复建议》（每条关联置信度）

**禁止**：低置信度当定论；重新扫描原始 events（只用 C 的证据）；直接改代码。

---

## 主 Agent

- 合并 A–D 输出为 [report-template.md](report-template.md)。
- §1 回答 **强制四问**。
- 不重新分析原始数据；若 sub-agent 输出冲突，标注冲突并请求补充读取（最多一轮）。
