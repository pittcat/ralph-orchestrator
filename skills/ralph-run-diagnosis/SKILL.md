---
name: ralph-run-diagnosis
description: >-
  Post-run deep diagnosis for any Ralph preset loop. Inventories .ralph artifacts
  by tier (S/A/B/C), reconciles events/ledger/recovery/logs against preset schema,
  audits OPAC with mode-aware confidence, traces mechanism bugs to source lines,
  checks docs/report recurrence, writes docs/report/*-diagnosis.md with per-finding
  root-cause confidence scores and low-confidence re-investigation. Use after
  ralph run, ralph-e2e, debug.md, loop diagnosis, or orchestration vs mechanism.
argument-hint: "[run_dir] [preset_file_or_builtin] [optional: plan_file]"
---

# Ralph Run Diagnosis

跑后诊断：**先盘点产物 → 按 Tier 对账 → 历史 → 源码归因 → 落盘报告**。不修代码。

**写任何机制/路径前必读**：[ssot-guardrails.md](references/ssot-guardrails.md)（禁止 hat_handoff、loop_state_snapshot.json、错误 CLI 等）。

**交付物**：**主仓** `docs/report/YYYY-MM-DD-<preset>-<loop_id>-diagnosis.md`。

## 输入

| 参数 | 必填 | 说明 |
|------|------|------|
| `run_dir` | 是 | 含 `.ralph/` 的 workspace（可 sibling worktree） |
| `preset` | 是 | `presets/en/foo.yml` 或 `builtin:foo` → 解析为 `presets/en/foo.yml` |
| `plan_file` | 否 | plan frontmatter 对账 |
| `repo` | 否 | 默认当前 `ralph-orchestrator` 主仓（报告路径） |

## 强制四问（§1 逐条，禁止合并）

1. 执行与 OPAC（须标 **diagnostics 模式 + OPAC 置信度**）
2. 基座机制是否生效
3. 编排是否合理
4. 归因：preset / mechanism / agent / compound（附 **根因置信度**）

## 执行顺序（硬约束）

```
Phase 0 盘点（串行，主 Agent）
    → 产出《产物盘点表》+ diagnostics 四档
    → 仅 then ↓
Phase 1  A∥B（流程 + 历史）
Phase 2  C（对账，吃 A+B+盘点表）
Phase 3  D（归因 + 置信度评分，吃 C+B+源码；低分加深）
Phase 4  主 Agent 汇总落盘
```

**禁止**在 Phase 0 完成前启动 sub-agent。

## Phase 0

[artifact-discovery.md](references/artifact-discovery.md) 六步 + [artifact-manifest.md](references/artifact-manifest.md) 分层读：

- **Tier S**：`current-events` → **唯一** events 文件（禁止 `events*.jsonl` 通配）
- **Tier A**：tasks/progress/summary/handoff（后两者仅终止后）
- **Tier B/C**：按盘点表 + preset/schema 解析

Diagnostics 四档：`FULL` | `MINIMAL` | `LOGS_ONLY` | `DISABLED` — 决定 L2/L OPAC 深度。

### Phase 0 能力推断（execution capabilities）

> **目的**：在写报告 §0 与 §1 之前，先声明这次 run 的 capability 集合，便于后续对账（supervisor.db 是否存在、wave_id Confirm 走哪条路径）有锚点。**禁止**按 builtin preset 名称点名门控；一律 capability-triggered（Intent.execution_model + YAML 信号 + 产物信号）。

**推断步骤（顺序固定）**：

1. 读 [`../ralph-preset-common/references/agent-native-model.md`](../ralph-preset-common/references/agent-native-model.md)「执行模型（Execution Model）」段确认枚举与检测信号；该节是 frozen vocabulary，本 plan 不再扩展。
2. 解析 preset：
   - `event_loop.supervisor.enabled: true` → capability +supervisor
   - hat `instructions` 含 `ralph wave emit` / `ralph wave verify`，或 hat 依赖 `## WAVE CONTEXT` → capability +wave
   - **禁止**用 `exec.wave.*` / `slot.*` 等协调 topic 推断 +wave（那些是 supervisor 协调面，走 supervisor audit，不是 wave fan-out 信号）
3. 解析 Intent（如有作者 notes）：`execution_model: wave | supervisor | supervisor+wave` → 与上面 capability 一致则 OK；不一致 → 主表 P0（详见 [`../ralph-preset-common/references/finding-rubric.md`](../ralph-preset-common/references/finding-rubric.md)「Supervisor capability audit」段 `preset.execution_model_intent_mismatch`）。
4. 扫描产物：
   - `.ralph/supervisor.db` 存在 → capability +supervisor（**仅**当上一步 YAML 也声明；产物不应推翻配置）
   - events 含 `wave_id` → capability +wave
5. 输出到报告 §0 的 **`execution_capabilities`** 字段（字符串数组），例如 `["single-chain"]` / `["wave"]` / `["supervisor", "wave"]`。

**缺 db / 缺 wave_id 不算故障（hard rule）**：在 capability 推断结果为单链时，缺 `.ralph/supervisor.db` 是**预期**，**不**是异常；events 无 `wave_id` 也是**预期**，**不**是异常。**仅**当 capability +supervisor 时缺 db 才列为缺失（runtime 异常）；**仅**当 capability +wave 时缺 wave_id 对账才列为缺失。

**wave Confirm 路径**：capability +wave 时，worker / dispatcher 完成态由 `ralph events --events-source main`（main ledger）对账；hat-channel 是 dispatcher 自己 private 落盘点，**不**用作 wave Confirm。L3 / L4 验证按 `references/mechanism-checklist.md`（如有 wave Confirm 源行则引用）。

## Phase 1–3 Sub-Agent

见 [subagent-charters.md](references/subagent-charters.md)、[verification-pipeline.md](references/verification-pipeline.md)。

**根因置信度**（详见 [confidence-rubric.md](references/confidence-rubric.md)）：

- **§5 入表门槛**：confidence ≥ 60；**P0 须 ≥ 70**，否则继续深挖或降为 P1
- **低分强制加深**：< 60 不得写入 §5 定论；按 rubric 补读 recovery/源码/preset 行号/历史，最多 2 轮
- **仍不足**：移入 §7「未核实疑点」，不得写修复建议
- 有 `file:line` + 双账本一致 → 可 ≥85；LOGS_ONLY 下 OPAC/agent 单项 ≤50
- compound 须写贡献比例 + 各成分置信度

**OPAC 置信度**：[opac-audit-by-mode.md](references/opac-audit-by-mode.md)

## Phase 4 落盘

[report-template.md](references/report-template.md)；§0 产物盘点 + §1 四问 + 盲区声明。

## 提交前检查

- [ ] Phase 0 盘点表在报告中
- [ ] 只读了 `current-events` 指向的 events
- [ ] LOGS_ONLY 未因缺 orchestration 标 P0
- [ ] 每条 P0/P1 在 §5 有 **置信度**；P0≥70、入表≥60
- [ ] confidence<60 的候选已加深或落入 §7，未混入 §5/§6
- [ ] 未引用 ssot-guardrails 禁止项
- [ ] 报告在主仓 `docs/report/`

## 参考

- [ssot-guardrails.md](references/ssot-guardrails.md) — **过时概念/错误路径禁止清单**
- [artifact-manifest.md](references/artifact-manifest.md) — Tier S/A/B/C
- [artifact-discovery.md](references/artifact-discovery.md) — Phase 0
- [confidence-rubric.md](references/confidence-rubric.md) — **根因置信度评分 + 低分加深**
- [log-reconciliation.md](references/log-reconciliation.md)
- [mechanism-checklist.md](references/mechanism-checklist.md)
- [source-trace-guide.md](references/source-trace-guide.md)
- [history-sources.md](references/history-sources.md)
- [examples/minimal-diagnostics-layout.md](references/examples/minimal-diagnostics-layout.md)
- 样板：`docs/report/2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md`
