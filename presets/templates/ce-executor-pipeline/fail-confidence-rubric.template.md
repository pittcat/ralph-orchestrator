# Fail-Confidence Rubric (executor / fixer 共用评分对照表)

本模板是 `ce-executor-pipeline` 中 executor 与 fixer 在声明 **fail / partial / blocked** 结算前唯一的自评 + 他评标准。precheck gate 也按本文件审核——**自评与他评使用同一份 rubric**，消除两处标准漂移。

适用字段命名：

- executor 在 `work.failed` 上填：`dead_end_confidence` / `dead_end_evidence_coverage` / `dead_end_evidence_file`
- fixer 在 `fix.done` 上填：`settlement_confidence` / `settlement_evidence_coverage` / `settlement_evidence_file`

下面四节是评分与判定规则；§4 是 gate 拒收时使用的 failed_checks 命名清单。

---

## §1. 四维度评分对照表

对每一个宣告 failed / blocked 的 Unit，按下列四个维度打分（每个维度独立打分，取最低值作为该 Unit 的置信度上限）：

### 1.1 尝试充分性（Attempt Sufficiency）

- 每 Unit 标准：1 次初始尝试 + 3 次 retry = 共 **4 次尝试**
- 每少 1 次尝试：**−15 分**（基础 100 分起）
- 示例：只尝试了 2 次 → 上限 100 − 2×15 = **70 分**

### 1.2 假设多样性（Hypothesis Diversity）

- 每次 retry 必须采用 **不同角度**（不同根因假设 / 不同切入点 / 不同工具组合）
- 标准要求：**≥3 个真正不同的角度**
- 每发现一对"机械重复"（同角度换皮重试）：**−10 分**
- 全部 4 次都是同一角度：最高只能给 60 分

### 1.3 因果链完整性（Causal Chain Completeness)

- 必须在证据文件里写清完整因果链：`trigger → 中间步骤 → 症状`
- 每个环节必须给出 **`file:line` 或 命令输出摘录**
- 缺任意一环、或缺少 file:line 支撑：置信度**上限 70 分**

### 1.4 假因排除（Alternative Elimination)

- 必须显式排除常见假因：环境差异 / 依赖版本 / flake 测试 / baseline 已存在 / 上次残留脏文件
- 每少排除一类：**−5 分**
- 完全未排除任何假因：置信度**上限 80 分**

最终单 Unit 置信度 = min(100, 1.1, 1.2, 1.3, 1.4 各维度得分)。

整体 `confidence` 字段 = 所有 failed/blocked Unit 单 Unit 置信度的**算术平均**，向下取整。

---

## §2. 阈值表

| 置信度区间 | 允许动作 |
|---|---|
| **≥ 90** | 允许 emit `work.failed` / `fix.done{fix_status: partial\|blocked}` |
| **75 – 89** | 必须先验证 1-2 个关键假设，再重评；仍 < 90 则继续 retry |
| **60 – 74** | 禁止 fail；继续 retry 并换角度 |
| **< 60** | 禁止 fail；视为严重证据不足，必须按 corrected course 继续干活 |

低于 90 而强行 emit，gate 会以 `confidence_inflated` 拒收。

---

## §3. Coverage 计算规则

`evidence_coverage` 衡量证据文件中"可复核"内容的比例。

- 一条 claim 算"有可复核来源"，当它附带下列任一：
  - `file:line` 引用（指向真实存在的源码或文档行）
  - 命令输出摘录（含命令 + 关键输出片段）
  - 日志片段（含时间戳 / 关键字）
  - 测试名 + 包路径
  - 文档路径 + 章节
- 计算：`(有可复核来源的 claim 数 ÷ 总 claim 数) × 100`，向下取整
- **≥ 75 合格**；< 75 必须在证据文件末尾追加"Coverage 缺口说明"章节，列出哪些 claim 暂无来源、为什么、计划如何补

虚报 coverage（claim 实际无来源但计入分子）会被 gate 判 `unverifiable_evidence`。

---

## §4. 六类 failed_checks 命名清单

precheck gate 拒收时，`failed_checks` 字段必须使用下列命名（每类附含义）：

| failed_check | 含义 |
|---|---|
| `missing_attempt_record` | 某 failed/blocked Unit 在证据文件里找不到 4 次尝试记录（1 初始 + 3 retry），或记录缺关键字段（角度 / 失败摘要） |
| `single_angle_retries` | 多次 retry 实为同一角度换皮，违反 §1.2 假设多样性 |
| `broken_causal_chain` | 因果链缺环（无 trigger / 缺中间步骤 / 缺症状），或缺 file:line 支撑，违反 §1.3 |
| `unverifiable_evidence` | claim 无可复核来源（违反 §3），或声称的来源经 spot-check 不存在 / 不支撑该 claim |
| `confidence_inflated` | 自报 confidence ≥90 但按 §1 四维度独立重评应 < 90 |
| `uneliminated_alternatives` | 未排除常见假因（环境 / 依赖 / flake / baseline / 残留），违反 §1.4 |

gate 在 `failed_checks` 中列出触发的命名（可多个），并在 `reason` 字段补一句可执行的整改指引（"U2 缺第 3 次 retry 记录"这种粒度）。

---

## §5. 使用流程

1. **materialize**：在 activation 内执行
   `ralph preset materialize-artifacts ce-executor-pipeline --plan-key <plan_name>`
   模板会落盘到 `.ralph/forge/<plan_name>/templates/`。
2. **读模板**：从落盘位置读 `fail-confidence-rubric.template.md` 与 `settlement-evidence.template.md`。
3. **写证据文件**：按 `settlement-evidence.template.md` 复制填写，落盘到：
   - executor：`.ralph/review/<plan>/dead-end-evidence.md`
   - fixer：`.ralph/review/<plan>/fix-settlement-evidence.md`（`applied` 时可填 `.ralph/agent/decisions.md`）
4. **自评**：按 §1 给每个 failed/blocked Unit 打分，按 §3 算 coverage，按 §2 决定是否允许 emit。
5. **填字段**：把分数与证据文件路径填到 `work.failed` / `fix.done` payload 的对应字段。
6. **emit**：触发 precheck gate。gate 用同一份 rubric 重评，通过则原样转发，不通过则按 §4 命名拒收。
