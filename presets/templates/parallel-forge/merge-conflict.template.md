# Merge Conflict — <WAVE_ID> wave <WAVE_INDEX>

> **模板用途**：integrator 在按 integration_order 顺序 FF 时遇到冲突
> 后写出本证据，路径约定
> `.ralph/forge/<plan-key>/waves/<wave_id>/merge-conflicts/<unit-id>.md`。
>
> **必填字段**：wave_id / wave_index / conflicting_unit_ids /
> conflict_fingerprint / resolution_strategy / plan_key /
> merge_conflict_path。

## 元数据

| 字段 | 值 |
|---|---|
| plan_key | |
| wave_id | |
| wave_index | |
| conflict_fingerprint | |

## 冲突单元

| unit_id | task_id | candidate_sha | base_sha | conflict_paths |
|---|---|---|---|---|

## 冲突分类

<!-- text / rename / semantic / dep-version -->

## 解决策略

| 选项 | 描述 | 风险 |
|---|---|---|

<!-- 优先级：依赖边 < 冲突路径 < union of allowed_paths -->

## 证据引用

| 类型 | 路径 |
|---|---|
| conflict diff | |
| resolution diff | |
| verifier re-run | |