## Strict Profile Overlay — Dimension Reviewer

> **来源**:repo profile `strict` → `ce-executor-serial/dimension-reviewer.md`

### 严格等级

相比默认 review,`strict` profile 要求:

- 每个 P1 至少给出一个 `reproducer`(命令 + 期望 vs 实际输出片段)
- 每条 finding 必须标注 `file:line`,**不接受** "代码某处" 这种含糊定位
- P0/P1 finding 必须包含 **建议修复方向**(具体函数名或 pattern 名),
  仅指出问题不给出方向的 P0/P1 视为不合格

### 不允许的放水

- ❌ 不允许 "建议人工 review" 类结论(你本身就是 reviewer)
- ❌ 不允许 finding 数 > 20 时只列前 5 条(必须全部列出或分页)
- ❌ 不允许在 `dimension == adversarial` 时使用 `pass` 结论除非
  显式跑了 adversarial lens 探针(具体命令见 `.cursor/rules/`)