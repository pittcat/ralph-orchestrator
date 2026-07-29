# Wave Failure — <WAVE_ID> wave <WAVE_INDEX>

> **模板用途**：forge-failure-handler / integrator / verifier 在写
> `*.failed` 证据时使用，路径约定
> `.ralph/forge/<plan-key>/waves/<wave_id>/failure-<round>.md`。
>
> **必填字段**：wave_id / wave_index / failure_fingerprint /
> failure_topic / affected_task_ids / affected_unit_ids / plan_key /
> failure_observation_path。

## 元数据

| 字段 | 值 |
|---|---|
| plan_key | |
| wave_id | |
| wave_index | |
| failure_topic | (`forge.wave.review.failed` / `forge.verification.failed` / `exec.wave.failed` 等) |
| failure_fingerprint | |
| correction_round | |

## 失败摘要

<!-- 一句话：哪条 Unit 在哪条 gate 上失败，为何算失败 -->

## 影响范围

| affected_task_ids | affected_unit_ids |
|---|---|

## 已尝试的修复

| round | angle | evidence |
|---|---|---|

<!-- 0..3 round；耗尽即触发 work.failed / forge.plan.blocked -->

## 证据引用

| 类型 | 路径 |
|---|---|
| review diff | |
| verification log | |
| prior correction | |