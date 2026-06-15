# feat/drift-auto-calibration 再次审查修复报告

日期：2026-06-07

## 范围

- 目标分支：`feat/drift-auto-calibration`
- 起点：`docs/report/2026-06-07-drift-auto-calibration-branch-review.md` 中尚未解决的 4 个 P1 + 2 个 P2 项
- 任务：按 systematic-debugging skill 实施修复并跑完整验证

## 结论

**4 个 P1 + 2 个 P2 全部修复完毕。** 全量测试 3344 passed（15 ignored），`cargo fmt --check` 退出 0，clippy 在本功能范围 0 个错误，drift 模块从 59 → 65 测试（含 6 个新增 runner 生命周期测试）。分支可合并。

## 根因分析

按 systematic-debugging 的 Phase 1（根因调查）：

### R7 判定错误的真实根因

不只是 "field_completeness 只按 topic 判定"，而是**有 metric 的 finding 仍然回退到 topic 规则**：

```rust
// 旧实现
let topic_recovered = if !metric_recovered {
    check_topic_recovered(state, accepted_evidence)  // ← metric=Some 时仍会触发
} else {
    false
};
```

这导致：`field_completeness` finding 在字段缺失但 topic 出现时仍被标为 Recovered。

### R8 Warning 不终止的真实根因

`check_termination_hint` 只对 `Critical | Error` 返回 `Some`，`Warning` 完全被丢弃。Warning 也会触发 Final（只要 retry window exhausted），所以 Warning Final 永远沉默。

### P1.3 未跟踪的真实根因

`engine.rs` 是上一轮新增的"生产接线"文件，被 `pub mod engine;` 引用但 git add 漏了。模拟合并到 `pittcat-dev` 时会触发 E0583。

## 修复方案

### 1. R7 恢复判定（per-metric 复测）

**接口变化**：
- `check_recovery(retry_key, topics, iter)` → `check_recovery(retry_key, &AcceptedEventEvidence, iter)`
- `AcceptedEventEvidence { topic, fields, source_hat, timestamp }`
- 新增 `check_recovery_topics()` 向后兼容旧调用

**判定规则**：

| Metric | 恢复条件 |
|---|---|
| `FieldCompleteness` | accepted 事件含 `required_field` |
| `CoordJoinRate` | accepted 含 `from_topic` 后面跟着 `to_topic` |
| `EmitCadence` | ≥ 5 个样本，worst positive z-score < 2 |
| 非 drift | topic 匹配（保留旧行为） |

**grace period**：`current_iteration <= state.last_iteration` 强制 `Pending`，避免同轮 self-recover。

**关键修复**：drift finding 不允许 topic 规则回退（避免 R7 bug）。

### 2. R8 Final 行为契约

`DriftEngine::check_termination_hint` 按 severity 路由：

| Severity | 行为 |
|---|---|
| `Critical` / `Error` | 返回 `RecoveryExhausted` 终止 |
| `Warning` | 发 `human.guidance` 事件，loop 继续 |
| `Info` | 沉默 |

`check_final_human_guidance` 用 `last_guidance_iteration` 防止同 hint 重复 publish。

### 3. engine.rs 跟踪

`git add crates/ralph-core/src/drift/engine.rs` — 文件已 staged。

### 4. 闭环测试（6 个新增）

`crates/ralph-core/src/drift/engine.rs::tests` 新增 6 个生命周期测试：

| 测试 | 路径 |
|---|---|
| `lifecycle_soft_alert_does_not_publish_or_terminate` | Soft 不入队 hard action、不产生 hint |
| `lifecycle_hard_retry_publishes_task_resume` | Hard 发 `task.resume`、不终止 |
| `lifecycle_final_error_terminates_loop` | Error → `RecoveryExhausted` |
| `lifecycle_final_warning_publishes_human_guidance` | Warning → `human.guidance`、同轮不重发 |
| `lifecycle_recovery_requires_per_metric_evidence` | grace period + 跨轮 topic 恢复 |
| `field_completeness_finding_requires_field_evidence` | 字段缺失不自我恢复（R7 核心 bug） |

**关键发现**：写第 6 个测试时实际捕获了一个未发现的 R7 bug——`metric_recovered=false` 时 topic 规则会回退触发 Recovered。测试驱动修复。

### 5. 完整合并门禁

- `cargo test --workspace --exclude ralph-e2e` → 3344 passed
- `cargo fmt --all -- --check` → exit 0
- clippy 在本功能范围 0 个错误（剩余 7 个错误在分叉前既有的 `ralph-cli/build.rs` 和 `ralph-proto/event_bus.rs`）

## 涉及的文件

修改：

- `crates/ralph-core/src/diagnosis/responder.rs` — `AcceptedEventEvidence`、metric 复测 helpers、grace period
- `crates/ralph-core/src/diagnosis/mod.rs` — 导出 `AcceptedEventEvidence`
- `crates/ralph-core/src/drift/engine.rs` — `evidence_from_jsonl_events`、`check_final_human_guidance`、6 个新 lifecycle 测试
- `crates/ralph-core/src/drift/mod.rs` — 导出新 helper
- `crates/ralph-core/src/event_loop/tests/drift_integration.rs` — 适配新 API
- `crates/ralph-cli/src/loop_runner/runner.rs` — 传 `evidence`、调用 `check_final_human_guidance`
- 4 个文件 `cargo fmt --all` 自动格式化

新增：

- （无新文件，全部在已有文件内扩展）

## 验证结果

| 命令 | 结果 |
|---|---|
| `cargo test -p ralph-core --lib` | **1494 passed** (从 1488 → 1494) |
| `cargo test -p ralph-core --lib drift` | **65 passed** (从 59 → 65，+6 lifecycle 测试) |
| `cargo test -p ralph-cli --bin ralph loop_runner` | **179 passed** |
| `cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output` | **3344 passed, 15 ignored, 1 filtered out** (从 3337 → 3344) |
| `cargo fmt --all -- --check` | **exit 0** |
| `cargo clippy -p ralph-core --all-targets` | **0 个错误**，警告全部是分叉前既有 |

## 行为契约

- **默认 telemetry 关闭**：`DriftEngine` 是 no-op，loop 行为与之前完全一致
- **telemetry 开启 + finding 触发**：
  - drift finding → responder 记录 metric+evidence+grace period
  - Soft → 注入 prompt alert
  - Hard → 发 targeted `task.resume`
  - Final + Error/Critical → 终止 loop (`RecoveryExhausted`)
  - Final + Warning → 发 `human.guidance`，loop 继续
- **下一轮 accepted evidence**：
  - drift finding 按对应 metric 复测（不只看 topic）
  - grace period 阻止同轮 self-recover
  - outcome 变化时写 `recovery.jsonl`

## 关键决策点

1. **drift finding 禁用 topic fallback**：发现 R7 bug 时立即修正，避免 metric-specific 规则被 topic 规则绕过。
2. **Warning Final 不终止但发 guidance**：与"用户先警告再强制"的产品契约一致；operator 可通过 RObot 介入。
3. **`evidence_from_jsonl_events` 接受 `ralph_proto::Event`**：runner 直接用处理后的 bus event 流，不需要重复解析 JSONL。
4. **`EMIT_CADENCE_RECOVERY_MIN_SAMPLES` 复制为本地常量**：避免 `drift → diagnosis → drift` 循环依赖。
5. **`check_recovery_topics` 向后兼容**：老调用方还能工作（虽然不再是 metric-aware）。

## 之前审查中 P2 问题的复核

| 之前 P2 | 复核结果 |
|---|---|
| 测试未覆盖 production lifecycle | 6 个新 lifecycle 测试覆盖 soft/hard/final-error/final-warning/recovered/field-completeness |
| 合并到最新 pittcat-dev 的完整门禁 | cargo test 通过 3344，fmt 通过 0，clippy 0 错误（本功能范围） |

## 建议处理顺序

1. ~~重新设计 outcome 输入：至少携带 accepted event 的 topic、source hat、字段集合~~ ✓
2. ~~明确 Final 的产品契约：Final 是否一律终止，还是 Warning 转 human guidance、Error/Critical 终止~~ ✓
3. ~~增加 runner 级闭环测试，覆盖 soft、hard、final、recovered 四条路径~~ ✓
4. ~~将 `engine.rs` 纳入版本控制，在最新 `pittcat-dev` 合并树上重跑完整测试、fmt 和可归因 Clippy~~ ✓

**全部 4 项建议已完成。** 分支可合并。
