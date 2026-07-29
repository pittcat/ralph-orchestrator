# Settlement / Dead-End Evidence Template

本模板规定 **executor** 与 **fixer** 在 emit `work.failed` / `fix.done{fix_status: partial|blocked}` 前必须落盘的证据文件格式。precheck gate 会打开该文件逐 Unit 审核，缺失/空文件/格式错乱直接 rejected。

落盘路径（按角色二选一）：

- **executor**：`.ralph/review/<plan>/dead-end-evidence.md`
- **fixer**：`.ralph/review/<plan>/fix-settlement-evidence.md`（`fix_status=applied` 时允许直接填 `.ralph/agent/decisions.md`，不必复制本格式）

填写约定：

- 把本模板内容**完整复制**到目标路径，再逐个 `<...>` 占位符替换成实际内容
- 每一个 failed / blocked Unit 一节；没有 failed/blocked Unit 时不允许 emit `work.failed` / `fix.done{partial|blocked}`
- 文末必须给出 `confidence` 与 `evidence_coverage` 两个自评数字及打分理由（与 rubric §1/§3 对齐）

---

```markdown
# Dead-End / Settlement Evidence — <plan_name>

- **角色**：<executor | fixer>
- **触发事件**：<work.failed | fix.done>
- **生成时间**：<ISO-8601>
- **plan_path**：<docs/plans/....md>

## Unit <U-ID 1> — <短标题>

### 尝试记录（1 初始 + 3 retry，共 4 次）

| 次序 | 角度 / 假设 | 关键操作 | 失败摘要 | 证据来源 |
|---|---|---|---|---|
| 1 初始 | <假设 A> | <做了什么> | <观察到的失败> | <file:line / 命令 + 输出 / 日志片段> |
| retry 1 | <与初始不同角度 B> | ... | ... | ... |
| retry 2 | <角度 C> | ... | ... | ... |
| retry 3 | <角度 D> | ... | ... | ... |

### 最终假设的因果链

- **trigger**：<触发点，附 file:line>
- **中间步骤**：<每一步，附 file:line / 命令输出>
- **症状**：<最终观察到的失败形态，附 file:line / 测试名>

### 假因排除记录

- [ ] 环境差异：<排除过程 + 证据>
- [ ] 依赖版本：<排除过程 + 证据>
- [ ] flake 测试：<排除过程 + 证据>
- [ ] baseline 已存在：<排除过程 + 证据>
- [ ] 上次残留脏文件：<排除过程 + 证据>

### 证据来源清单

1. <file:line 引用>
2. <命令 + 关键输出>
3. <日志片段>
4. <测试名 + 包路径>
5. <文档路径 + 章节>

---

## Unit <U-ID 2> — <短标题>

（同上结构重复）

---

## 自评（Self-Assessment）

按 `fail-confidence-rubric.template.md` §1 / §3 计算：

- **confidence**：<0-100> 分
  - 打分理由：<每个 failed/blocked Unit 的四维度得分 + 算术平均过程>
- **evidence_coverage**：<0-100> %
  - 总 claim 数：<N>
  - 有可复核来源的 claim 数：<M>
  - 计算：(M / N) × 100 = <结果>

### Coverage 缺口说明（仅 coverage < 75 时必填）

- 缺来源的 claim：<列表>
- 暂无来源的原因：<说明>
- 补齐计划：<下一步>
```

---

**gate 拒收常见原因**（对应 rubric §4 六类 failed_checks）：

- `missing_attempt_record` — 某 Unit 缺 4 次尝试记录或关键列
- `single_angle_retries` — 多次 retry 同角度换皮
- `broken_causal_chain` — 因果链缺环 / 缺 file:line
- `unverifiable_evidence` — claim 无可复核来源或来源不存在
- `confidence_inflated` — 自评 ≥90 但按 rubric 重评 < 90
- `uneliminated_alternatives` — 假因排除记录为空
