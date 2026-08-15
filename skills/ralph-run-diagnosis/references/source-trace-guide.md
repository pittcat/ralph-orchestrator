# 源码反查规程（L6）

产物证明「发生了什么」；**mechanism** 归因须 **file:line**。

## 1. recovery → 源码

| `source` | 读 |
|----------|-----|
| `payload_contract` | `event_policy.rs` |
| `execution_contract` | `execution_contract.rs` |
| `workflow_guard` | `event_loop/mod.rs` |
| `stall_recovery` / `loop_stale` | event_loop stall 分支 |
| `topic_format` | event_policy topic 白名单 |
| `hat_channel_empty_after_activation` (plan 2026-08-15-1823) | `crates/ralph-cli/src/loop_runner/hat_channel.rs`（merge_hat_channel 空文件路径）；是 evidence anchor，不是 root cause |
| `merge_hat_channel_failed` | `crates/ralph-cli/src/loop_runner/inner.rs`（normal merge close 失败分支）；evidence anchor |
| `merge_hat_channel_failed_on_interrupt` | `crates/ralph-cli/src/loop_runner/entry.rs::merge_isolated_channel_on_interrupt`；evidence anchor |

`jq` 取 `reason_code` → `rg` 定位 → `sed -n` 读分支。

## 2. 症状 → 入口

| 症状 | 入口 |
|------|------|
| duplicate_work_done | `event_policy.rs` dedup |
| loop_anchor null | `inspect.rs` `LoopInspectView` / `build_loop_anchor_summary` |
| resume 死信 | preset `triggers` + hat 选择 |
| silent-success | shipper recoverable、`shipper_reason.rs`、`max_residuals` |
| scope 只 warn | `enforce_hat_scope` 实现 |
| `runtime-trace.jsonl` 中 `phase=activation` / `kind=hat_activation_outcome` 行（plan 2026-08-15-1823） | `crates/ralph-cli/src/loop_runner/activation_outcome.rs`（snapshot_channel / refine_after_merge / log_activation_outcome）；evidence anchor；状态值见 §1 速查表 |

## 3. preset / schema

- hat `instructions` **仅模拟该 hat**（AAF）
- `presets/schemas/<name>.yml` 的 `state_projection.actions_chain`
- 可选：`cargo nextest run -p ralph-core -- preset_lint`

## 4. 工作区漂移

必读 `run_dir/ralph.yml`：`coordinator_hats`、`telemetry` 覆盖。

## 5. 报告 §7 格式

每条 P0 ≥2 引用（机制 file:line + preset/schema 行 或 产物行）。

## 6. 深读文档

`docs/guide/runtime-diagnosis.md`、`.cursor/rules/observability.mdc`、`docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md`
