# Preset Topology Patterns

> **仅拓扑阶段参考。** 起草 `instructions:` 时不得把下列拓扑描述抄进 hat 文案。

## debug（4 hat，isolated）

适合学习 AAF handoff 与 OPAC。Builtin：`builtin:debug`。

```
debug.start
  → investigator → hypothesis.test
  → tester → hypothesis.confirmed | hypothesis.rejected
  → fixer → fix.propose → fix.applied
  → verifier → fix.verified | fix.failed
  → investigator (fix.verified) → DEBUG_COMPLETE
```

| Hat | 典型 Q4 emit | 下游 Q2 |
|---|---|---|
| investigator | `hypothesis.test`, `fix.propose`, `DEBUG_COMPLETE` | tester/fix 从 trigger payload + orchestrator context |
| tester | `hypothesis.confirmed`, `hypothesis.rejected` | fixer 读 `fix.propose` payload |
| fixer | `fix.applied`, `fix.blocked` | verifier 读 `fix.applied` |
| verifier | `fix.verified`, `fix.failed` | investigator 读 `fix.verified` |

参考：`presets/en/debug.yml`。

## ce-executor-serial（10 hat，isolated）

Plan-driven 执行 + 串行多维 review。Builtin：`builtin:ce-executor-serial`。

高层事件流（简化）：

```
plan-gate / work.start
  → coordinator（拆 task、发 work.start）
  → executor（TDD 实现，work.done）
  → validator
  → review-coordinator（review.start，批量 wave）
  → 6× review dimension hats（review.dimension.done）
  → review-synthesizer → fixer → alignment → reporter → plan.complete
```

- Schema SSOT：`presets/schemas/ce-executor-serial.yml`
- 改 topic / `required_fields` / `state_projection` 须同步 schema 与 7 点清单

参考：`presets/en/ce-executor-serial.yml`、`docs/handbook/serial-preset-development.md`。

## 起草反模式（禁止抄进 instructions）

| 反模式 | 应改为 |
|---|---|
| 「reviewer 通过后你会收到…」 | Q2：`ralph tools task list` / trigger payload 字段名 |
| 「上一步 executor 已提交代码」 | Q2：Observe `work.done` 投影字段 |
| 「读 events.jsonl 末尾」 | `ralph events --events-source hat-channel` |
| 「整个 pipeline 有 12 个 hat」 | 删除；该 hat 不知拓扑 |
