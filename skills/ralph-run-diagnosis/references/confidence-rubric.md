# 根因置信度评分规程

> 本规程配套 `ralph diagnose --causal` 输出（U08/U09）为归因事实来源；agent 不另行打分。所有「置信度」数字直接来自 `ralph diagnose --causal` 的 `confidence` 字段及其分项 breakdown。

OPAC 单项置信度见 [opac-audit-by-mode.md](opac-audit-by-mode.md)。本文管 **归因完成度** 的判定。

---

## 入表门槛（DT7 机检，>85 严格）

| 区域 | 规则 |
|------|------|
| **§5 问题归因表** | 仅收录 **`status == complete`**（`confidence > 85`，DT7 机检） |
| **P0 行** | 同样 `confidence > 85`；`status == incomplete` 不得标 P0，降为 P1 或落入 §7 |
| **§4 证据清单** | 每条 DEV 列「DT7 分项加分」与「来源锚点」（来自 `--causal` JSON） |
| **§7 未核实疑点** | `status == incomplete`（`confidence ≤ 85` 或 `not_evaluable`）单独列出，**不得驱动修复建议** |
| **legacy / v1 / 无契约** | `status == not_evaluable`，按 bundle 既有规则兜底（见 [report-template.md](report-template.md) §0.2） |

> **DT7 严格门禁（hard rule）**：`> 85` 必须**严格**大于 85（不接受 `≥ 85`）。边界判定：85 → `incomplete`；86 → `complete`。该边界由 U08 的 `causal_attribution` 单测钉住。

---

## DT7 机检计分卡（5 项，max 100）

`ralph diagnose --causal` 输出 `confidence_breakdown` 字段即下表五项分值的逐项明细。**禁止**按本表手动加总；与 JSON 字段不一致视为工具漂移，按 HARD RULE 上报。

| DT7 项 | 分值 | 机检判定 | 来源 sidecar / 收据 |
|--------|------|----------|---------------------|
| **coverage**（覆盖完整） | +30 | 8 边界（contract / activation / backend_outcome / event_candidate / policy_decision / state_commit / recovery_action / termination）全部 `covered` | `diagnosis-input.json` `boundary_coverage[]`（U7 v2 manifest） |
| **integrity**（收据/账本一致） | +25 | `outbox ↔ commit_receipt` join 一致；`policy_receipt` accept/reject 计数匹配；`recovery_receipt.action ∈ {resume, exhausted, correction}` 与 `plan.blocked` payload `retry_key` 对账 | `runtime-trace.jsonl` 三类收据（U03-U05）+ `.ralph/ledger.jsonl` |
| **refutation**（落选域反驳） | +20 | 4 个落选域（除 `primary_domain` 外的 4 个）各自提供 ≥1 条反驳证据，列入 `rejected_hypotheses[]` | `CausalAttributionReport.rejected_hypotheses[]` |
| **correlation**（关联链完整） | +15 | `contract_digest` + `sequence` 严格单调 + `retry_key` 与 `plan.blocked{kind=precheck_exhausted}` payload 一致 | `runtime-trace.jsonl` `phase=decision` `kind=contract_receipt` 行（U02） |
| **freeze_window**（异常冻结窗口） | +10 | 5 类异常（watchdog timeout / 非零退出 / precheck 耗尽 / recovery 耗尽 / 异常 activation outcome）之一触发时 `evidence-window.jsonl` 存在且首行 anomaly 描述 | `<session>/evidence-window.jsonl`（U6） |

**总置信度** = 上述 5 项逐项累加（**机器读 `--causal` JSON，不手算**）。

**门禁**：`total > 85` ⇒ `status == complete`；否则 `status == incomplete`。

### 计分示例（来自 `--causal` JSON）

- 全 8 边界 covered（+30）+ 三类收据 join 一致（+25）+ 4 落选域各 1 条反驳（+20）+ contract_digest 一致（+15）+ watchdog 触发有 evidence-window（+10）= **100** → `complete`
- 7 边界 covered（boundary_coverage 缺 1）+ 三类收据 join（+25）+ 4 落选域各 1 条（+20）+ contract_digest 一致（+15）+ 无异常触发（+0）= **70** → `incomplete`

---

## 落选域与被否决假设

`CausalAttributionReport.rejected_hypotheses[]` 列出 `primary_domain` 之外 4 个落选域的反驳证据。报告 §4.x 与 §6 引用时**只引用域名 + 反驳证据类型**，不复述具体 evidence_refs（per HARD RULE 4.8）。

| 落选域 | 含义（domain 枚举固定 5 项） |
|--------|------------------------------|
| `runtime` | runtime 行为/契约偏离 |
| `preset` | preset / schema 配置问题 |
| `agent` | agent 在自己 activation 内可见的行为 |
| `backend` | 后端适配器 / 模型输出 |
| `diagnostic_capture_contract` | 诊断捕获契约缺口（coverage 决定） |

> **禁止** 维护平行根因分类：`mechanism / preset / agent / compound` 等旧分类不再独立打分；归因落点由 `--causal` 的 `primary_domain` 字段给定。Agent C/D 不再按旧分类加深决策树。

---

## 分数变化记录（DT7 重新打分）

每次 Agent D 重新触发 `ralph diagnose --causal` 后，`confidence` / `confidence_breakdown` / `primary_domain` 可能变化。报告 §4 必须记录每次「分数变化」：

| 重新打分原因 | 上次 total | 本次 total | Δ | primary_domain 是否变化 | 落选域反驳新增 |
|--------------|------------|------------|---|--------------------------|------------------|
| 加深：补 read 第二账本 | 70 | 95 | +25 | 否（仍为 preset） | 无 |
| 加深：refutation 增 2 条 | 95 | 100 | +5 | 否 | +2 条 |

> **禁止** 在分数变化小节捏造上次分数；如属首次打分，写 `N/A (initial scoring)`。

---

## Agent 分工（DT7 重新对齐）

| Agent | 职责 |
|-------|------|
| **C** | 触发 `ralph diagnose --causal` 第一次，把 JSON 五项分项与 `rejected_hypotheses` 抄录到 §4 证据清单；标注每条 DEV 的「DT7 分项来源」。 |
| **D** | 必要时重跑 `--causal`（重新打分），记录「分数变化」；不再用 legacy 计分卡加深。 |
| **主 Agent** | 汇总前审计：§5 行 `status == complete` 才入表；`status == incomplete` / `not_evaluable` 不得入 §5 / §6。 |

> **回退禁止**：DT7 严格门禁下，**禁止** legacy "low-confidence → 加深调查" 决策树作为补分手段。加深调查可以补 **新证据**，但补证据后必须重跑 `--causal`，由机检重新打分；不允许 Agent 自行加分。

---

## 报告必填列（§4 / §5）

§4 证据：

```text
| ID | 描述 | 证据锚点 | 严重度 | DT7 分项来源 | 缺口 |
|----|------|----------|--------|--------------|------|
| DEV-001 | ... | file:line / event#L / event_candidate:L<N> | P0 | coverage(+30) / integrity(+25) / correlation(+15) | freeze_window 缺 |
```

§5 归因（**P0/P1 必填 status + confidence**）：

```text
| 优先级 | 问题 | primary_domain | status | confidence | 证据 DEV | DT7 分项来源 | rejected_hypotheses | 历史关联 | 加深轮次 |
|--------|------|----------------|--------|------------|----------|--------------|---------------------|----------|----------|
| P0 | ... | preset | complete | 95 | DEV-00x | coverage / integrity / correlation / refutation / freeze_window | 4 落选域 | 高 | 1→95 |
```

§1.2 四问：Q4 格增加 **归因置信度**（取 §5 最高 P0 的 `confidence` 字段；`status == incomplete` 时附 `not_evaluable` 备注）。

§1.1 健康度：`P0 / P1 / P2 数量` 旁注明 **（均为 status=complete）**。

---

## 示例（按 DT7 + `--causal`）

```text
DEV-004 | review-synthesizer verdict=blocked 且 findings_count=0 | events:L24 | P0初判
--causal confidence=95, status=complete, primary_domain=preset
DT7 分项：coverage(+30) + integrity(+25) + refutation(+20) + correlation(+15) + freeze_window(+5, partial)
rejected_hypotheses：runtime(recovery:1) / agent(empty_channel:1) / backend(timeout_audit:1) / capture_contract(no)
→ §5 P0 入表
```

```text
候选：mechanism silent-success | 仅见 LOOP_COMPLETE + summary 乐观
--causal confidence=70, status=incomplete, primary_domain=runtime
DT7 分项：coverage(+30, 缺 backend_outcome) + integrity(+25) + refutation(+0, 落选域未反驳) + correlation(+15) + freeze_window(+0)
→ §7 未核实疑点；blocked_by: coverage.backend_outcome 缺 / refutation 未给出
```

```text
候选：agent 未做 policy-check | LOGS_ONLY 模式
--causal confidence=20, status=incomplete, primary_domain=capture_contract
DT7 分项：coverage(+0, 4 边界 covered 缺失) + integrity(+10, 仅 ledger 单账本) + correlation(+10) + freeze_window(+0)
rejected_hypotheses：runtime(/health 拒收缺) / preset(policy_check OK) / agent(无 agent-output) / backend(no_logs)
→ §7；不驱动修复
```