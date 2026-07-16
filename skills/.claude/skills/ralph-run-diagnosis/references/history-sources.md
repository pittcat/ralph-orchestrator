# 历史文档扫描范围（Agent B）

按 preset 名、症状关键词、loop_id 检索。优先近 30 天，全库扫描复发模式。

## 目录

| 路径 | 内容 |
|------|------|
| `docs/report/*-diagnosis.md` | **主索引** — 历次跑后诊断 |
| `docs/achieved/report/` | 已归档诊断 |
| `docs/brainstorms/` | 机制/preset 需求与根因讨论 |
| `docs/plans/` | 修复 plan（status、U1-Ux 任务） |
| `docs/solutions/` | 已沉淀 solution 文档 |
| `docs/achieved/plan/` | 已完成 plan |

## 检索策略

1. **Preset 级**：文件名或正文含 `ce-executor-serial`、`ce-executor-pipeline` 等。
2. **症状级**：`task.resume`、`silent-success`、`loop_anchor`、`duplicate_work_done`、`OPAC`、`recovery.jsonl`、`semantic_gate_violation`。

> 历史报告若出现 [ssot-guardrails.md](ssot-guardrails.md) 禁止项，标注「旧报告/已删除机制」，**不纳入本次对账**。
3. **Loop 级**：若历史报告引用同一 `plan_file` 或 worktree 名，标高关联。

## 输出字段（每条历史项）

- `doc_path`
- `problem_type`（分类标签）
- `occurrence_count`（全库约数）
- `closed?`（plan merged / solution 存在）
- `relevance`：高 / 中 / 低
- `one_line_summary`

## 复发判定

满足任一即标「复发」：

- 同一 `problem_type` + 同一根因分类在 30 天内 ≥2 次
- 本次 DEV 证据与历史报告 §4 引用同一源码路径/同一 recovery reason
- 历史 plan 标 `active` 且本次仍命中其描述的 symptom

## 与修复 plan 对照

对每条 P0，在 `docs/plans/` 搜索是否已有 plan：

- 有 plan 未合 → 标注「待落地」
- 有 plan 已合但复发 → 标注「修复不完整」→ 倾向 mechanism 回归
