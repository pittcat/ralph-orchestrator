# Wave Settlement — <WAVE_ID> wave <WAVE_INDEX>

> **模板用途**：integrator 在收到 `forge.wave.verified` 后写出本证据并
> emit `forge.wave.settled`。路径约定
> `.ralph/forge/<plan-key>/waves/<wave_id>/settlement.md`。
>
> **必填字段**：wave_id / wave_index / settled_task_ids /
> settled_unit_ids / verified_base_commit / integration_branch /
> reviewer_verdict / verifier_verdict / correction_rounds /
> settlement_log_path。

## 元数据

| 字段 | 值 |
|---|---|
| plan_key | |
| wave_id | |
| wave_index | |
| integration_branch | |
| verified_base_commit | |
| settlement_log_path | |

## Unit settlement bill

| unit_id | task_id | task_key | commit_sha | reviewer_verdict | verifier_verdict |
|---|---|---|---|---|---|

<!-- task_id 必须是 ralph tools task list 中的 live id（不要手写 closed id） -->

## Correction history

| round | fingerprint | files_changed | outcome |
|---|---|---|---|

<!-- 0..3 round；耗尽走 work.failed / forge.plan.blocked -->

## 证据引用

| 类型 | 路径 |
|---|---|
| review report | |
| verification log | |
| merge log | |
| correction log | |