---
title: parallel-forge preset 集成缺口闭合 — schema pointer 未接导致 schedule 静默跳过
date: 2026-07-29
module: preset-integration
tags: parallel-forge, state-projection, ensure-task-batch, event-filter, close-task-batch, preset-lint, bdd
problem_type: integration-gap
plan: docs/plans/2026-07-29-005-fix-parallel-forge-preset-integration-gap-plan.md
---

# parallel-forge preset 集成缺口闭合

## 现象

plan 2026-07-29-001 落地了 `EnsureTaskBatch` 的 `execution_wave` /
`integration_order` / `execution_plan_digest` 三个 option pointer 与
`CloseTaskBatch` 原子批投影两条 runtime 机制。pointer 全部为
`None` 时投影走 legacy DAG-only 分支,`validate_wave_schedule`
不会激活;`close_task_batch` 则在 mid-loop close 失败时把
`store.tasks` 改了一部分但不写盘,留下内存与磁盘不一致。

merge 之后 `parallel-forge` 全量基线测试仍绿,因为 `parallel-forge`
的 `presets/schemas/parallel-forge.yml` 仍然把 3 个 pointer 留空,
preset 端从未接到新机制。其它 preset 不依赖这些 pointer,看不到差异。

## 根因

| 链路 | 缺口 |
|---|---|
| `presets/schemas/parallel-forge.yml::forge.plan.ready` | `required_fields` 缺 `execution_plan_digest` / `wave_total`;`unit_tasks` field_docs 未声明每项必含 `execution_wave` / `integration_order` |
| `presets/schemas/parallel-forge.yml::state_projection.actions.forge.plan.ready` | `EnsureTaskBatch` 三个 pointer 全为 None(执行序列化时 key 不出现) |
| `presets/en/parallel-forge.yml` reviewer/integrator/verifier/tester | `event_filter.events` 与 `triggers` 不一致(`forge.wave.*` / `forge.final.correction.settled` 入口被 filter 静默丢弃) |
| `presets/en/parallel-forge.yml` reviewer/integrator/verifier/tester instructions | 主路径教旧 topic(`forge.units.reviewed` / `forge.integration.done` / `forge.incremental.verified`),失败直接 `work.failed` 而非对应 `*.failed` observation |
| `presets/en/parallel-forge.yml` executor instructions | 「runtime 由事件投影原子关闭对应 task」是错的;实际 task 关闭发生在 `forge.wave.settled` |
| `presets/en/parallel-forge.yml` forge-failure-handler | Final correction 子节步骤编号与主步骤 1.–6. 重叠(4./5.) |
| `crates/ralph-core/src/state_projector/task.rs::project_close_task_batch` | 在 `store.tasks` 上原地 mutate;mid-loop 失败时 `store` 半改但 `persist` 未调 |
| `presets/templates/parallel-forge/` | 缺 4 个新模板(settlement / failure / conflict / correction) |
| `crates/ralph-core/tests/scenarios/` | 缺核心 BDD 覆盖 S4 / S5 / S6 / S7 / S10 / S11 / S13 / S14 |

## 修法

按 plan 005 U1–U8 顺序闭合,每 Unit 一个独立 commit:

- **U1**(schema pointer): `presets/schemas/parallel-forge.yml`
  state_projection.actions.forge.plan.ready 补三个 pointer;
  `forge.plan.ready.required_fields` 补 `execution_plan_digest` /
  `wave_total`;`unit_tasks` field_docs 声明每项必含
  `execution_wave` / `integration_order`。`crates/ralph-cli/src/
  presets.rs` 加 `test_builtin_state_projection_action_keys_migration
  _inventory` 内部断言三个 pointer 均 `Some(...)`(回退 schema
  时三条都报红)。
- **U2**(planner wave 算法): planner 步骤 4 写明
  `wave(unit) = 1 + max(wave(dep))`、wave 集合连续 1..W、
  `integration_order` 全局唯一 1..N、`execution_plan_digest` /
  `wave_total` 顶层必填。schema 字段作为行为 gate(不锁定
  instructions 字面,HARD RULE)。
- **U3**(event_filter ↔ triggers): reviewer / integrator / verifier /
  tester 四 hat 的 `event_filter.events` 与各自 `triggers` 业务入口
  对齐;tester triggers 同步覆盖
  `forge.exec.development.done` /
  `forge.final.correction.settled`。结构化断言在
  `test_parallel_forge_event_filter_covers_triggers`。
- **U4**(instructions 新 topic): 四 hat + executor + failure-handler
  instructions 改写为主路径新 topic(`forge.wave.reviewed` /
  `forge.wave.integrated` / `forge.wave.verified` /
  `forge.wave.settled`),失败走对应 `*.failed` observation,**禁止**
  直接 `work.failed`。executor instructions 修正 task 关闭语义(本
  Unit task 由 `forge.wave.settled` CloseTaskBatch 投影原子批量关闭)。
  forge-failure-handler Final correction 步骤编号从 4./5. 重排为
  7./8.。结构化断言 `test_parallel_forge_failure_handler_step
  _numbering_is_consecutive`(重复编号报红)。
- **U5**(`project_close_task_batch` 原子性): 在
  `store.tasks()` 快照上完成全部 start+close;全部成功才调
  `replace_tasks_for_atomic_batch` + `persist` 一次。任一 id 失败
  立即返 Err,磁盘与 `ctx.tasks_cache` 保持调用前 byte-identical。
  `TaskStore` 新增 `pub(crate) tasks()` /
  `replace_tasks_for_atomic_batch(Vec<Task>)`。单测
  `settlement_partial_failure_leaves_ledger_unchanged` 锁定
  byte-identical 契约。
- **U6**(4 个模板): 新增 `wave-settlement.template.md` /
  `wave-failure.template.md` / `merge-conflict.template.md` /
  `correction.template.md`;`PARALLEL_FORGE_TEMPLATE_NAMES` /
  `PARALLEL_FORGE_TEMPLATES` / `build.rs::copy_artifact_templates` /
  `README.md` 同步注册。`materialize --plan-key` 实测产出 10 个
  文件。
- **U7**(BDD): 新增/改写 `parallel_forge_*.yml`(S6、S7 既有)+ 新增 `parallel_forge_round_exhaustion_gate_runtime.yml`(S11,#2 三轮终态门禁 e2e),均用 `run_workflow_guard_scenario` 真实 EventLoop 跑(禁止 `run_scenario` stub)。S11 锁 `forge.final.correction.settled` 只接受 `correction_round=3`(round=1 拒绝、round=3 接受,事件计数=1);S6 锁两波 settlement 各出现一次;S7 锁 `work.failed` absent + correction 路径走通。
- **U8**(文档 + skill): `AGENTS.md` ≡ `CLAUDE.md` 中 parallel-forge 描述从「旧 14-step」改为「静态 wave + per-wave settlement + development_loop」;`docs/solutions/` 新增本文档。通用 `ralph-tools-tasks.md` 新增「Projection-Owned Batch Close」段(对偶 Projection-Owned Task Creation),沉淀 fix-unit 豁免等通用规则。

## 复盘

- 「写 preset YAML 但不接 schema」是 001 / 003 之后半落地的稳定根
  因;U1 的 schema pointer 是唯一 SSOT,`presets/en/*.yml` 中独立
  `state_projection` 块不要另行声明。
- 「unit 测试锁定 instructions 字面」是反模式;锁结构化字段
  (`event_filter.events` ⊇ `triggers`、`publishes` 含新 topic、
  `required_fields` 含 wave/order/digest、step 编号无重复)即可。
- 「executor 关闭 task」是常见误解;task 关闭是 wave settlement
  的副作用,executor 只 emit `exec.unit.done`,从不调用
  `ralph tools task close`。executorreview/reviewer/integrator/
  verifier/tester 的 instructions 都按这个边界写。
- 「集成缺口闭合」类 plan 的 verdict 信号:`./scripts/run-tests.sh`
  全量绿 + `ralph preset check -H builtin:parallel-forge --strict`
  无 finding + `materialize-artifacts` 实测产出新模板 + BDD 真实
  EventLoop 跑通新 topic 链;任一项红即视为未闭合。

## 验证

```bash
# schema pointers + structural tests
cargo nextest run -p ralph-cli --bin ralph -- builtin_state_projection
cargo nextest run -p ralph-cli --bin ralph -- parallel_forge_event_filter
cargo nextest run -p ralph-cli --bin ralph -- parallel_forge_failure_handler_step_numbering
cargo nextest run -p ralph-cli --bin ralph -- planner_instructions
cargo nextest run -p ralph-cli --bin ralph -- builtin_artifact

# close_task_batch 原子性
cargo nextest run -p ralph-core -- close_task_batch

# BDD
cargo nextest run -p ralph-core --test scenarios -- parallel_forge

# 文档 byte-identical
diff -q AGENTS.md CLAUDE.md

# 模板 materialize
cargo run -p ralph-cli --bin ralph -- preset materialize-artifacts parallel-forge --plan-key test-u6
```