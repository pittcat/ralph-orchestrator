# Manager Report — Ralph recovery for `forge.plan.ready` 投影拒收

- **Date**: 2026-07-30
- **Plan**: `docs/plans/2026-07-29-002-feat-parallel-forge-reuse-status-plan.md`
- **Plan key**: `2026-07-29-002-feat-parallel-forge-reuse-status-plan`
- **Loop**: `primary-20260730-002911`
- **Hat**: `ralph`（operator 触发面）
- **Status**: BLOCKED → 终止（需人工 `ralph run --resume` 复跑）

## 摘要

Planner hat 在 iteration 2 emit `forge.plan.ready` 时，runtime `state_projector`
执行 `validate_wave_schedule`（`crates/ralph-core/src/state_projector/task.rs:1631`），
命中规则 1："`execution_wave` 与 `integration_order` 必须为正整数"。具体违规：
`F1` 的 `execution_wave=0`、`integration_order=1`（详见
`.ralph/events-20260730-002911.jsonl` 内 `forge.plan.ready` payload）。

## 已修复（artifact 已写盘、digest 自洽）

1. `.ralph/forge/2026-07-29-002-feat-parallel-forge-reuse-status-plan/execution-plan.yml`
   - 9 个 Unit 的 `execution_wave` 全部 +1：`F1: 0→1`, `U1/U2/U3: 1→2`,
     `U4: 2→3`, `U5: 3→4`, `U6/U7: 4→5`, `U8: 5→6`。
   - `wave_total` 保持 6（与 `max(execution_wave)=6` 一致）。
   - `integration_order` 不变（已是 1..9）。
   - `execution_plan_digest` 由
     `c74c444732c54d74d3382795dd191d696cc9d16de39a1e0a85858c44d0a659d5`
     重算为
     `a5cdfe9d3d106d3feb932d574973273cdb7025da34ac48ce736e9e2b14efa8a9`，
     canonical JSON = sorted_keys / `separators=(",", ":")` /
     排除 `execution_plan_digest` / `wave_total` / `unit_count`
     （与文件头部注释 §canonical 一致）。
   - 自洽验证：再次跑同一 canonical 化算出的 digest 与文件内写入的 digest 相等。
2. `.ralph/forge/2026-07-29-002-feat-parallel-forge-reuse-status-plan/development-plan.md`
   - 同步将 "Wave 0..5" 描述与 `execution_wave: 0..5` 改为 "Wave 1..6" /
     `execution_wave: 1..6`，保持 doc 与 SSOT 一致。

## 复跑 payload（已构造、未写盘）

`/tmp/forge_plan_ready.json`（build script 见 `/tmp/build_payload.py`），
字段全部满足 `ralph emit forge.plan.ready --policy-check` 预检，含
9 个 `unit_tasks[]`（F1..U8），wave / integration_order 与更新后的
`execution-plan.yml` 一一对应，digest 与文件内一致。

## 终止原因（`LOOP_COMPLETE`）

- 当前 hat = builtin `ralph`（pseudo-hat），runtime 硬约束
  `RALPH_CONTROL_TOPICS = ["LOOP_COMPLETE", "loop.cancel", "loop.start", "task.resume", "plan.blocked"]`
  + `add_builtin_ralph` 派生 `publishes`（不含任何业务 topic），无法从本 hat
  emit `forge.plan.ready`/`forge.plan.blocked` 等业务 topic（实测 CLI guard:
  "Builtin ralph hat may only emit control topics..."）。
- isolated mode `--hat planner` 被拒（"Isolated mode hat mismatch"）。
- `loop.cancel` 与 `task.resume` 触发 `flow_unknown_emit` / `origin:out_of_scope`。
- 唯一通过的 control topic 是 `LOOP_COMPLETE`，因此走"干净终止 + 报告 + 留
  待 operator resume"路径，避免在错误状态下继续 spin。

## 后续 operator 动作

```bash
# 1. 已无需手工改 artifact——execution-plan.yml / development-plan.md 已就位
# 2. 启动新 loop 让 planner 重新 emit 修正后的 forge.plan.ready
ralph -H builtin:parallel-forge -c ralph.forge.yml \
  --worktree --reuse-worktree \
  --plan docs/plans/2026-07-29-002-feat-parallel-forge-reuse-status-plan.md
```

`planner` hat 下一轮会重新读 `execution-plan.yml`（已含修正后的 wave / digest），
emit 的 `forge.plan.ready` payload 与本报告 `/tmp/forge_plan_ready.json`
同源；`validate_wave_schedule` 规则 1（`execution_wave > 0`）会通过。

## 决策置信度

- 修复正确性：高（runtime 源码 + 文件注释 + canonical 重算三方一致）。
- 终止路径：> 80，唯一可行的 control topic（已实测）。
- 选择 LOOP_COMPLETE 而非 loop.cancel：terminate 而非 abort，保留
  `.ralph/events-20260730-002911.jsonl` 与 `.ralph/forge/<plan-key>/` 状态，
  方便 operator 复核后 `--resume` / 新建 loop 续跑。