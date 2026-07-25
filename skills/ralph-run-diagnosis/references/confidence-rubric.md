# 根因置信度评分规程

与 `ralph-preset-review` 对齐：**每条 P0/P1 的根因结论必须有 0–100 置信度**；置信度太低不得当作定论，必须加深调查。

OPAC 单项置信度见 [opac-audit-by-mode.md](opac-audit-by-mode.md)。本文管 **DEV 偏离 → 根因归因** 的置信度。

---

## 入表门槛

| 区域 | 规则 |
|------|------|
| **§5 问题归因表** | 只收录 **confidence ≥ 60** 的 P0/P1/P2 |
| **P0 行** | confidence **< 70** 时不得标 P0，降为 P1 或继续深挖直至 ≥70 |
| **§4 证据清单** | 每条 DEV 附 **严重度初判 + 置信度初估**（C 产出） |
| **§7 未核实疑点** | confidence < 60 且已深挖 **2 轮**仍不足 → 单独列出，**不得驱动修复建议** |

---

## 评分锚点（0–100）

按证据叠加取**上限**，不要凭直觉给高分。

| 分数档 | 含义 | 典型证据组合 |
|--------|------|----------------|
| **90–100** | 可审计定论 | `file:line` + events/recovery **双账本一致** +（可选）历史报告同根因 + BDD/源码行为吻合 |
| **75–89** | 强推断 | 双账本一致 + preset/schema 具体行；或 `file:line` + 单一账本 |
| **60–74** | 可入表弱定论 | 单一强证据（如 recovery `reason_code` + 对应源码段）但缺双账本或缺行号 |
| **40–59** | **不可定论** | 仅 logs 弱信号、症状相似、或单一 event 推测 → **必须加深** |
| **0–39** | 猜测 | 仅拓扑「看起来不对」、无 DEV 锚点 → 禁止写入 §5 |

### 分类加减分

| 根因分类 | 升到 ≥70 还需 |
|----------|----------------|
| **mechanism** | Rust `file:line` + recovery/ledger 与源码语义一致 |
| **preset** | `preset_file` 或 `presets/schemas/` **具体行号** + 实际违规 event/log |
| **agent** | agent-output（FULL）或 logs 中明确违例 + schema 要求对照 |
| **compound** | 各成分分别打分后加权；**整行置信度 = min(成分置信度)** 或写明加权公式 |

### Diagnostics 模式封顶

在 L0 盲区声明的封顶之上，**根因置信度**再受模式约束：

| 模式 | 根因置信度硬顶（无 FULL agent-output 时） |
|------|------------------------------------------|
| FULL | 100 |
| MINIMAL | 85（缺 agent-output 的 agent 归因 ≤60） |
| LOGS_ONLY | 75（mechanism 有 file:line+recovery 可例外到 85；纯 OPAC/agent ≤50） |
| DISABLED | 70 |

---

## 低置信度 → 加深调查（强制）

当某条 DEV 或候选根因 **confidence < 60**（或 P0 候选 **< 70**）时，**禁止**写入 §5 定论表；按序加深，每轮须记录「做了什么 → 分数变化」：

### 加深顺序（至少做 2 项再重评）

1. **补读账本**：workspace + session `recovery.jsonl`、ledger 对应 iteration、hat-channel 文件
2. **源码反查**：[source-trace-guide.md](source-trace-guide.md) — `reason_code` → Rust 函数
3. **preset 行级**：`sed -n` 读 triggers/publishes/instructions 与违规 event 对照
4. **历史对照**：[history-sources.md](history-sources.md) — 同 preset 旧报告是否同根因；⚠️ **仅在 `--include-history ≠ disabled` 时允许**，disabled 下此步标记为不可用
5. **BDD**：`crates/ralph-core/tests/scenarios/<preset>*.yml` 预期 events
6. **Tier C 产物**：findings、fix-log、progress 与 event payload 交叉验证

### 轮次上限

- 每条候选根因最多 **2 轮**加深；仍 < 60 → 移入 **§7 未核实疑点**，标注 `blocked_by: <缺什么证据>`
- 主 Agent 可发起 **一轮**补充 sub-agent（仅针对未达标 DEV），不得无限循环

---

## Agent 分工

| Agent | 置信度职责 |
|-------|------------|
| **C** | DEV 表：`严重度初判` + `置信度初估` + `缺口说明` |
| **D** | 根因表：终评置信度；< 60 触发加深流程；compound 加权 |
| **主 Agent** | 汇总前审计：§5 无 < 60 行；P0 无 < 70 行；§7 与 §5 不重复 |

---

## 报告必填列（§4 / §5）

§4 证据：

```text
| ID | 描述 | 证据锚点 | 严重度 | 置信度初估 | 证据缺口 |
```

§5 归因（**P0/P1 必填置信度**）：

```text
| 优先级 | 问题 | 根因分类 | 置信度 | 证据 | 历史关联 | 加深轮次 |
```

§1.2 四问：Q4 格增加 **归因置信度**（取 §5 最高 P0 或 compound 整行置信度）。

§1.1 健康度：`P0 / P1 / P2 数量` 旁注明 **（均为 confidence≥门槛）**。

---

## 示例

```text
DEV-004 | review-synthesizer verdict=blocked 且 findings_count=0 | events:L24 | P0初判 | 45 | 缺 FULL agent-output 与 instructions 行号对照
→ 加深：读 preset L2374 + recovery → 置信度 78 → 入 §5 P0
```

```text
候选：mechanism silent-success | 仅见 LOOP_COMPLETE + summary 乐观 | 置信度 38
→ 加深：shipper_reason.rs + events 终态链 → 82 → 入 §5
```
