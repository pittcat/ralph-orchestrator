---
date: 2026-07-05
topic: ralph-run-diagnosis-skill
---

# Ralph Run 事后诊断 Skill — 需求文档

> **方法**: ce-brainstorm（2026-07-05）
> **触发输入**: `debug.md` 四问 + 历史 `docs/report/*-diagnosis.md` 样板 + 用户对「比 4 sub-agent 更深」的要求
> **目标读者**: 跑完任意 preset / E2E 后的 operator、机制维护者、preset 作者

## Problem Frame

跑完 `ralph run` 或 `cargo run -p ralph-e2e` 后，中间产物散落在 `.ralph/`（events、tasks、recovery、diagnostics 等）与 preset/schema 定义处。用户需要回答四类问题：

1. 整体执行与 **OPAC**（Observe / Precheck / Apply / Confirm）是否每个 hat 都遵守？
2. 中间产物是否证明 **Ralph 基座机制**（event loop、recovery、单事件预算、origin guard 等）正常生效？
3. **编排**（preset 事件拓扑、hat 触发、step 闭环）是否合理、是否按预期走完？
4. 若有问题，根因是 **机制**、**编排（preset）**、**agent 执行/产物**，还是多因素叠加？

现有 `debug.md` 提供 4 sub-agent 并行骨架，但缺少：OPAC 逐 hat 审计清单、机制源码反查规程、`presets/schemas/` 对账、与 `docs/report/` 历史复发模式索引、以及强制落盘报告契约。`ce-debug` 偏通用 bug 修复，不覆盖编排/机制归因。`2026-06-13-run-diagnostics` 草案选择 preset 内嵌诊断、明确不加 skill——本需求**有意改为项目 skill**，供 loop 外 operator 显式调用。

## Requirements

**范围与触发**

- R1. Skill 名 `ralph-run-diagnosis`；存放于项目 `skills/` 与 `.claude/skills/`（与 `ralph-preset-review` 同级）。
- R2. 触发：任意 preset 跑完后；用户显式调用并传入 `run_dir` + `preset_file`（可选 `plan_file`、`history_root`）。E2E 场景为常见子集，非唯一入口。
- R3. **仅诊断 + 写报告**；不自动改 preset、不改基座源码、不复跑 loop（修复建议文字输出即可）。

**输入契约**

- R4. 必填：`run_dir`（含 `.ralph/` 或等价中间产物根）、`preset_file`（如 `presets/en/ce-executor-serial.yml`）。
- R5. 推荐只读补充：`ralph diagnose --session latest`（若 diagnostics 存在）、`presets/schemas/<preset>.yml`、对应 BDD 场景 YAML（`crates/ralph-core/tests/scenarios/`）。
- R6. 代码审查以**主仓源码**为准；`run_dir` 仅作运行时产物，用于反推机制/编排缺陷。

**诊断架构（增强版 4 sub-agent）**

- R7. 保留 4 路并行，职责不交叉；主 agent 只汇总，不重新分析原始数据（与 `debug.md` 一致）。
- R8. **Agent A — 流程还原**：从 preset + schema 提取预期事件流/hat 拓扑；从 `run_dir` 提取实际 events/tasks/diagnostics；输出实际 vs 预期链路图（✅/❌/⏸️/⚠️）。
- R9. **Agent B — 历史上下文**：扫描 `docs/report/`、`docs/brainstorms/`、`docs/plans/`、`docs/solutions/`、`docs/achieved/report/`；按问题类型建知识库；标注与本次 run 关联度（高/中/低）；**即使无直接关联也必须输出**。
- R10. **Agent C — 对账分析**（加深）：在 A+B 基础上逐项检查——payload vs schema、hat 触发、review/fix/ship 闭环、tasks/progress/findings 一致性；**新增 OPAC 四阶段逐 hat 审计**（对照 `ralph-tools-opac.md` + diagnostics agent-output）；**新增 R1-R6 治理项**（isolated 单事件预算、ledger 不可读、precheck 等）；列出全部偏离及证据（文件路径 + 行号/事件序号）。
- R11. **Agent D — 归因与修复**：对 C 的每条偏离分类为 `preset` / `mechanism` / `agent` / `compound`；P0/P1/P2；给出目标文件/机制与修复建议；**必须引用 B 判断是否为历史复发**。

**强制四问（报告摘要层）**

- R12. 最终报告必须在「结论摘要」中**逐条回答** `debug.md` 四问（执行+OPAC、机制生效、编排合理、机制 vs 编排归因），不允许合并成模糊段落。

**输出契约**

- R13. 落盘路径：`docs/report/YYYY-MM-DD-<preset-basename>-<loop_id>-diagnosis.md`（与现有 15+ 份历史报告一致）。
- R14. 报告固定章节：①结论摘要 ②执行链路对比图 ③历史问题上下文 ④证据清单 ⑤问题归因表 ⑥修复建议（短期/中期/长期）；模板见 skill `references/report-template.md`。
- R15. 所有证据须可点击定位：禁止「某处有问题」式描述。

**与现有能力边界**

- R16. 不替代 `ralph preset check` / `preset_lint`（启动前）；不替代 `ralph-preset-review`（静态 AAF）；本 skill 是**跑完后**复盘。
- R17. 可引用 `ralph diagnose` JSON/Markdown 作为证据，但不以其为唯一输入。

## Success Criteria

- 对 `debug.md` 示例输入（`ralph-e2e-serial/.ralph` + `ce-executor-serial`），skill 产出报告且四问均有明确是/否/部分 + 证据指针。
- 报告格式与 `docs/report/2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md` 同级深度（链路图、历史关联表、P0 机制归因）。
- Operator 无需记忆产物路径：skill 内 `artifact-manifest` 列出完整读取清单。
- 与 `ce-debug` 边界清晰：本 skill 名称/描述不含「自动修复」。

## Scope Boundaries

- 不做实时 loop 内诊断（跑的过程中不介入）。
- 不做自动修复或自动复跑 E2E。
- 不新增 CLI subcommand（纯 skill 工作流）。
- 不替代 LLM-as-judge 主观质量打分（判断基于可验证文件/事件）。

## Key Decisions

- **任意 preset 跑完后可用**（非仅 E2E）：用户明确选择；输入契约沿用 `debug.md`。
- **增强 4 sub-agent**（非 5 agent、非单 agent 深潜）：在 Agent C 嵌入 OPAC/R1-R6，避免协调开销。
- **报告写 `docs/report/`**：与历史诊断语料同目录，便于 Agent B 索引。
- **仅报告不修**：与 `2026-06-13-run-diagnostics` 原 N2 一致；修复走后续 plan/ce-debug。

## Dependencies / Assumptions

- 主仓为 `ralph-orchestrator`；`run_dir` 可在 sibling worktree（如 `ralph-e2e-serial`）。
- OPAC 定义以 `crates/ralph-core/data/ralph-tools-opac.md` 为 SSoT。
- 机制规则以 `.cursor/rules/observability.mdc`、`multi-hat-isolation.mdc` 及 `docs/brainstorms/2026-06-27-ralph-orchestrator-mechanism-foundation-requirements.md` 为深读参考。

## Outstanding Questions

### Resolve Before Planning

（无——用户已在 brainstorm 中确认范围、行为、位置、架构、报告路径。）

### Deferred to Planning

- [Affects R7][Technical] sub-agent 在 Cursor 中用 `Task` 工具还是 sequential 深读——skill 写推荐并行，实现时按平台能力降级。
- [Affects R13][Needs research] 是否增加 frontmatter 字段（`loop_id`、`preset`、`recurrence_count`）统一历史检索——plan 阶段定。

## Next Steps

-> 实现 `skills/ralph-run-diagnosis/SKILL.md` + references
-> `/ce:plan` 若需将 skill 与 E2E CI 钩子集成（可选）
-> 用最近一次 E2E 产物试跑验收
