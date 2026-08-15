# 机制与 OPAC 检查清单

归因须回读源码（[source-trace-guide.md](source-trace-guide.md)）。**禁止项**见 [ssot-guardrails.md](ssot-guardrails.md)。

## OPAC（`ralph-tools-opac.md`）

按 [opac-audit-by-mode.md](opac-audit-by-mode.md)；LOGS_ONLY 下 OPAC 单项置信度 ≤50，不得单独升 P0。

## R1–R6（isolated）

| ID | 检查 | 失败信号 |
|----|------|----------|
| R1 | 不读 ledger / supervisor.db | logs 出现 tail events |
| R2 | 单事件预算 | 同 activation 多业务 topic |
| R3 | 不假设拓扑 | projection 缺字段 |
| R4 | 共享状态经 task API | 手改 tasks.jsonl |
| R5 | emitter 先 `--policy-check` | FULL agent-output 或 recovery |
| R6 | task 三字段 | payload vs tasks.jsonl |

## 机制十二项

| 机制 | 读什么 | 异常信号 |
|------|--------|----------|
| Origin guard | recovery `reason_code`（`origin:*` 等） | 越权仍落盘 |
| Payload contract | recovery `source=payload_contract` | 缺字段仍 accept |
| Execution contract | recovery `execution_contract` | work.done 无 git |
| Workflow guard | recovery / ledger | 乱序 phase |
| Semantic gate | recovery `semantic_gate_violation` | 非法 review/plan 组合 |
| Isolated 单事件 | events + hat-channel | 重复 business emit |
| step_handoff 对齐 | tasks.jsonl + progress.md | step 漂移 |
| Recovery 升级 | recovery `outcome` | 连 failed 无 escalate |
| Resume 路由 | hat-channel、`loop.resume`/`task.resume` | 发出无消费者 |
| Stall | recovery `stall_recovery`/`loop_stale` | 长沉默无记录 |
| Drift | session `drift.jsonl` | 坏流无 drift |
| Dedup | ledger / recovery | duplicate 仍推进 |
| Terminal | events 终态 | silent-success |
| Activation outcome row | runtime-trace.jsonl `phase=activation` / `kind=hat_activation_outcome`（plan 2026-08-15-1823） | raw facts 与 recovery / events 不一致 |

> 新 recovery 常走 **prompt correction**（`docs/guide/runtime-diagnosis.md`），不一定有 bus 上的 resume 事件。

## 源码索引

| 主题 | 入口 |
|------|------|
| Event loop | `crates/ralph-core/src/event_loop/mod.rs` |
| Event policy | `crates/ralph-core/src/event_policy.rs` |
| shipper 兜底 | `crates/ralph-core/src/shipper_reason.rs` |
| Recovery | `crates/ralph-core/src/state/recovery_log.rs` |
| Ledger | `crates/ralph-core/src/state/ledger.rs` |
| Session handoff | `crates/ralph-core/src/handoff.rs` |
| step_handoff | `crates/ralph-core/src/step_handoff/` |
| Inspect / loop_anchor | `crates/ralph-cli/src/commands/inspect.rs` |
| Loop snapshot（内存） | `crates/ralph-core/src/loop_state_snapshot.rs` |
| Hat-channel 路径 | `crates/ralph-cli/src/loop_runner/paths.rs` |
| Preset lint | `crates/ralph-core/src/preset_lint/` |

## 编排层

- triggers/publishes 闭合；schema `required_fields`、`state_projection.actions_chain`
- 终态期望用 schema 声明的 topic（**非** `review.passed`）
- BDD：`crates/ralph-core/tests/scenarios/`

## 机制 vs 编排

```
lint/schema 已警告但 runtime 放行 → preset 或 lint 缺口
runtime 拒收/路由错误 → mechanism
正确拒收 + agent 反复违例 → compound
instructions 诱导非法 emit → preset + agent
```

## 能力感知对账（capability-aware reconciliation）

按 `SKILL.md`「Phase 0 能力推断」段结果，对账 Tier B 条件产物与 events / log：

| capability 信号 | 期望产物 / 路径 | Confirm 路径 | 缺失判定 |
|---|---|---|---|
| `event_loop.supervisor.enabled: true` **或** `.ralph/supervisor.db` 存在 | wave/supervisor ledger（enabled 时通常有 db；default-wave 可能仅有 db） | `ralph inspect loop --format json`：先 `has("supervisor")`，为 true 再读 `supervisor` 块 | enabled=true 且缺 db → +supervisor 记 P0；enabled=false 且无 db → 无 `supervisor` 键是预期；enabled=false 但有 db → **期望**可能有 `supervisor` 键，勿当故障省略 |
| events 含 `wave_id` | hat-channel（dispatcher）+ main ledger（worker Confirm） | worker 完成态走 `ralph events --events-source main`（main ledger） | events 无 wave_id → capability +wave 必需时记缺失；否则 N/A |
| 既无 supervisor.enabled、又无盘上 ledger、又无 `ralph wave emit` | 仅 main events + tasks + Tier C | `ralph events --events-source main` + Tier C artifact | 缺 supervisor.db / wave_id / inspect `supervisor` 键是预期，**不**列为故障 |

报告 §0 与 §5 引用 capability 推断结果而非 preset 名称；详见 `SKILL.md`「Phase 0 能力推断」段。
