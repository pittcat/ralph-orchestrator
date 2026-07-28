# 根因置信度评分规程

与 `ralph-preset-review` 对齐：**每条 P0/P1 的根因结论必须有 0–100 置信度**；置信度太低不得当作定论，必须加深调查。

OPAC 单项置信度见 [opac-audit-by-mode.md](opac-audit-by-mode.md)。本文管 **DEV 偏离 → 根因归因** 的置信度。

---

## 入表门槛

| 区域 | 规则 |
|------|------|
| **§5 问题归因表** | 只收录 **confidence ≥ 60** 的 P0/P1/P2 |
| **P0 行** | confidence **< 70** 时不得标 P0，降为 P1 或继续深挖直至 ≥70 |
| **§4 证据清单** | 每条 DEV 附 **严重度初判 + 置信度初估**（C 产出，用计分卡） |
| **§7 未核实疑点** | confidence < 60 且已深挖 **2 轮**仍不足 → 单独列出，**不得驱动修复建议** |

---

## 证据向量计分卡（可计算，替代文字锚点）

每条 DEV 的根因置信度 = **基础分 40** + **证据向量加分**（逐项累加，上限 100）。**禁止**凭直觉给「感觉像 80 分」。

| 证据项 | 加分 | 判定标准（必须可审计） |
|--------|------|------------------------|
| `file:line` 源码锚点 | +25 | 有具体 Rust 文件 + 行号，且 `sed -n` 读过该分支语义 |
| 双账本一致 | +20 | events + recovery（或 ledger / agent-output / orchestration）**两条独立记录**指向同一结论 |
| preset/schema 行号 | +15 | `preset_file` 或 `presets/schemas/` 具体行号 + 实际违规 event/log 对照 |
| BDD 预期吻合 | +10 | `crates/ralph-core/tests/scenarios/<preset>*.yml` 的 `expected.events` 与本次实际偏离一致 |
| 历史同根因 | +10 | Agent B 找到 30 天内同 `problem_type` + 同根因分类的历史报告（仅 `--include-history ≠ disabled` 时可用） |
| Tier C 产物交叉验证 | +10 | findings / fix-log / progress / scratchpad 与 event payload 交叉印证 |
| agent-output 明确违例 | +15 | FULL 模式下 `agent-output.jsonl` 中 tool_call 序列明确违例（仅 agent 归因可用） |
| prompt visibility 矛盾 | +10 | `inspect prompt` JSON 显示 auto_inject 与 instructions 声明不一致（仅 agent/preset 归因可用） |

**计分示例**：

- `file:line` (+25) + 双账本 (+20) + BDD (+10) = **95**（可审计定论）
- `file:line` (+25) + 单账本 (+0) + preset 行号 (+15) = **80**（强推断）
- 仅 recovery `reason_code` + 源码段，无 file:line，无第二账本 = **40 + 0 = 40**（不可定论，必须加深）

### 根因分类加分门槛

| 根因分类 | 升到 ≥70 的最低证据组合（必须同时满足） |
|----------|----------------------------------------|
| **mechanism** | `file:line` (+25) + 双账本 (+20) = 85；或 `file:line` (+25) + 单账本 + BDD (+10) = 75 |
| **preset** | preset/schema 行号 (+15) + 实际违规 event/log (+0，但必须有) + 双账本 (+20) = 75；或 preset 行号 (+15) + prompt visibility 矛盾 (+10) + 单账本 = 65（P1 可入，P0 需再加深） |
| **agent** | agent-output 违例 (+15) + schema 要求对照 (+0，但必须有) + 双账本 (+20) = 75；或 logs 明确违例 (+0) + prompt visibility 矛盾 (+10) + 单账本 = 50（LOGS_ONLY 下常见，须加深） |
| **compound** | 各成分分别按上表计分；**整行置信度 = min(成分置信度)** 或写明加权公式（如 `0.6×mechanism + 0.4×preset`） |

### Diagnostics 模式封顶（硬顶，不可突破）

| 模式 | 根因置信度硬顶（无 FULL agent-output 时） | 例外 |
|------|------------------------------------------|------|
| FULL | 100 | — |
| MINIMAL | 85 | 缺 agent-output 的 agent 归因 ≤60 |
| LOGS_ONLY | 75 | mechanism 有 `file:line` + recovery 可例外到 85；纯 OPAC/agent ≤50 |
| DISABLED | 70 | — |

**封顶规则**：计分卡算出分数后，**再受模式硬顶约束**。例如 LOGS_ONLY 下 mechanism 算出 80，硬顶 75 → 最终 75；若机制有 `file:line` + recovery，例外到 85。

### 置信度传播规则（C → D）

Agent C 的「置信度初估」与 Agent D 的「终评置信度」必须满足：

- **C 用计分卡初估**：C 在 §4 证据清单中对每条 DEV 按上述计分卡打分，标注「已计分证据项」。
- **D 在 C 基础上加深**：D 的终评 = C 初估 + 加深轮次新增证据项加分（每轮最多 +25，即一个 `file:line` 或双账本）。
- **D 不得无理由低于 C**：若 D 终评 < C 初估，必须在 §5 该行「加深轮次」列写明「D 认为 C 的某证据项无效，理由：...」。
- **D 不得突破模式硬顶**：无论加深多少轮，终评 ≤ 模式硬顶。

---

## 低置信度 → 加深调查（强制）

当某条 DEV 或候选根因 **confidence < 60**（或 P0 候选 **< 70**）时，**禁止**写入 §5 定论表；按根因分类走加深决策树，每轮须记录「做了什么 → 新增证据项 → 分数变化」。

### 加深决策树（按根因分类，每轮最多 2 项，有信息增量阈值）

**信息增量阈值（hard rule）**：每轮加深必须新增至少 **一个可计分证据项**（见计分卡），否则视为无效轮次，直接入 §7。

#### mechanism 归因加深路径

```
第 1 轮（必做）：源码反查 — reason_code → Rust 函数 → file:line（+25）
    ↓ 若仍 <70
第 2 轮（二选一）：
    a) 补读第二账本 — workspace recovery ↔ session recovery ↔ ledger（+20）
    b) BDD 对照 — scenarios/<preset>*.yml expected.events 是否覆盖此偏离（+10）
```

#### preset 归因加深路径

```
第 1 轮（必做）：preset/schema 行级 — sed -n 读 triggers/publishes/instructions 具体行号（+15）
    ↓ 若仍 <70
第 2 轮（二选一）：
    a) 双账本 — events + recovery 同时指向 preset 违规（+20）
    b) prompt visibility 对账 — inspect prompt JSON 与 instructions 声明矛盾（+10）
```

#### agent 归因加深路径

```
第 1 轮（FULL 模式必做）：agent-output 审计 — tool_call 序列明确违例（+15）
    ↓ 若仍 <70（或 LOGS_ONLY 模式）
第 2 轮（二选一）：
    a) logs 关键词 — rg 'policy-check|scope violation' diagnostics/logs/（+0，但可补双账本）
    b) prompt visibility 对账 — inspect prompt JSON 显示 agent 看不到某 skill（+10）
```

#### compound 归因加深路径

```
各成分并行第 1 轮：分别按 mechanism/preset/agent 路径第 1 轮加深
    ↓ 若整行 <70
第 2 轮：对最低分成分补第 2 轮（或补历史对照 +10，仅 --include-history ≠ disabled）
```

### 轮次上限

- 每条候选根因最多 **2 轮**加深；仍 < 60 → 移入 **§7 未核实疑点**，标注 `blocked_by: <缺什么证据项>`（必须是计分卡中的具体项，如「缺 file:line」「缺双账本」）
- 主 Agent 可发起 **一轮**补充 sub-agent（仅针对未达标 DEV），不得无限循环

---

## Agent 分工

| Agent | 置信度职责 |
|-------|------------|
| **C** | DEV 表：`严重度初判` + `置信度初估`（按计分卡，列出已计分证据项） + `缺口说明`（缺哪些证据项） |
| **D** | 根因表：终评置信度（C 初估 + 加深新增，≤ 模式硬顶）；< 60 触发按分类决策树加深；compound 加权 |
| **主 Agent** | 汇总前审计：§5 无 < 60 行；P0 无 < 70 行；§7 与 §5 不重复；**校验每行是否有计分卡证据项列出** |

---

## 报告必填列（§4 / §5）

§4 证据：

```text
| ID | 描述 | 证据锚点 | 严重度 | 置信度初估 | 已计分证据项 | 证据缺口 |
```

§5 归因（**P0/P1 必填置信度**）：

```text
| 优先级 | 问题 | 根因分类 | 置信度 | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
```

§1.2 四问：Q4 格增加 **归因置信度**（取 §5 最高 P0 或 compound 整行置信度）。

§1.1 健康度：`P0 / P1 / P2 数量` 旁注明 **（均为 confidence≥门槛）**。

---

## 示例（按计分卡）

```text
DEV-004 | review-synthesizer verdict=blocked 且 findings_count=0 | events:L24 | P0初判
C 初估：40（无计分证据项，仅 events 单点）
→ D 加深第 1 轮：读 preset L2374（preset 行号 +15）+ recovery 拒收记录（双账本 +20）→ 75 → 入 §5 P0
```

```text
候选：mechanism silent-success | 仅见 LOOP_COMPLETE + summary 乐观
C 初估：40
→ D 加深第 1 轮：shipper_reason.rs file:line（+25）→ 65
→ D 加深第 2 轮：events 终态链 + recovery 双账本（+20）→ 85 → 入 §5
```

```text
候选：agent 未做 policy-check | LOGS_ONLY 模式
C 初估：40（仅 logs 弱信号）
→ D 加深第 1 轮：logs 中 rg 'policy-check' 无结果（+0，不算新增证据项，无效轮次）
→ 直接入 §7，blocked_by: 缺 agent-output（FULL 模式才可用）
```
