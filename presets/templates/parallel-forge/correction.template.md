# Correction — <WAVE_ID> round <ROUND>

> **模板用途**：wave-fixer 收到 `forge.correction.requested` 后产出本
> 证据再 emit `forge.correction.done`；同时是 final correction 后
> `forge.final.correction.settled` 的引用对象。路径约定
> `.ralph/forge/<plan-key>/waves/<wave_id>/corrections/round-<n>/report.md`。
>
> **必填字段**：wave_id / wave_index / correction_round /
> commit_sha / affected_unit_ids / plan_key / correction_report_path。

## 元数据

| 字段 | 值 |
|---|---|
| plan_key | |
| wave_id | |
| wave_index | |
| correction_round | |
| trigger_topic | (`forge.wave.review.failed` / `forge.verification.failed`) |
| trigger_fingerprint | |

## 修复摘要

<!-- 一句话：针对哪个 fingerprint 修了哪些路径，为何这样修 -->

## 改动文件

| 路径 | 变更类型 | 来源 unit_id |
|---|---|---|

<!-- 仅 allowed_paths 内；forbidden_paths 一律不动 -->

## 回归证据

| 命令 | 退出码 | 关键输出 |
|---|---|---|

## 与上轮差异

<!-- 标 fingerprint-diff，避免机械重复旧修法 -->

## 证据引用

| 类型 | 路径 |
|---|---|
| trigger failure | |
| correction diff | |
| verifier re-run | |