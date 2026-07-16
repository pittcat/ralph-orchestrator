# OPAC 审计：按 Diagnostics 模式降级

SSoT：`crates/ralph-core/data/ralph-tools-opac.md`

| 模式 | 判定 | Observe | Precheck | Apply | Confirm | 最高置信度 |
|------|------|---------|----------|-------|---------|------------|
| **FULL** | 有 `orchestration.jsonl` + `agent-output.jsonl` | tool_call 逐条 | `--policy-check` 在 emit 前 | 单业务事件 / hat-channel | `ralph events` | 90+ |
| **MINIMAL** | 有 `diagnostics/<ts>/` 但无 `agent-output.jsonl` | session recovery + events | recovery `payload_contract` | events 重复 topic | ledger 拒收行 | 70 |
| **LOGS_ONLY** | 仅 `diagnostics/logs/`（多数默认 run） | logs 中 inspect/task | logs 中 policy-check | events 同 iter 多业务 topic | 通常**不可验证** | **≤50** |
| **DISABLED** | 连 logs 都无（极罕见） | 仅 events 推断 | 不可验证 | 不可验证 | 不可验证 | **≤30** |

## LOGS_ONLY 下的 OPAC 表写法

每 hat 一行，证据列必须写来源：

```markdown
| Hat | O | P | A | C | 证据 | 置信度 |
| coordinator | ✅ | ⚠️ | ✅ | N/A | logs:42 inspect; events:L8 work.ready; 未见 policy-check | 45 |
```

规则：

- **Confirm 列 N/A** 在 LOGS_ONLY 下允许，须在表下注脚说明
- **不得**因未见 precheck 就标 P0 OPAC 违规——标 ⚠️ + 置信度 ≤50，除非 recovery/logs 有明确 `payload_contract` 拒收且 agent 未先 precheck
- FULL 模式下 Confirm 缺失可标 P1

## 与四问 Q1 的关系

Q1 须分两段：

1. **编排执行**（events 拓扑）— 可不依赖 diagnostics
2. **OPAC 合规**— 必须标注 **审计模式 + 置信度上限**

示例：「OPAC：LOGS_ONLY 下仅能以 logs+events 弱推断；Precheck 未见全局证据，置信度 45，不作 P0 OPAC 违规定论。」
