---
title: "fix: implementation-review default wave fan-in 与执行阶段共享同一有效 aggregate deadline"
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
product_contract_source: ce-plan-bootstrap
created: 2026-08-06
type: fix
---

# fix: implementation-review default wave fan-in 与执行阶段共享同一有效 aggregate deadline

## Summary

本次 `implementation-review` wave 的 6 个 reviewer 全部成功（耗时 726.156s，0 失败），但 fan-in 调用点（`crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`）从 `SupervisorConfig.aggregate_timeout_secs`（默认 600s）取得 deadline，而 dispatcher 执行阶段使用 worker/concurrency/batch 公式计算出的有效 deadline 约 930s，`phase.rs` 的 timeout 判断先于 complete 判断，因此 726 > 600 触发 `Failed(Timeout)`，最终 `review.wave.failed` 阻断 `review-synthesizer` 和 `fix-planner`。本计划把 fan-in 改为复用 dispatcher 已解析的同一有效 deadline，避免两条 deadline 链继续漂移，并在测试中固定“6 slots 726s 不应 timeout”的回归覆盖。

## Context

- 触发来源：`docs/report/2026-08-06-implementation-review-primary-20260806-090515-diagnosis.md` 的 P0（compound，根因置信度 95）。
- 失败现象：`.ralph/review/2026-08-05-001-refactor-large-file-module-split-plan/wave-blocked.md:1` 写入 `reason: wave_failed:timeout` 且 `missing_dimensions: []`；`.ralph/diagnostics/logs/ralph-2026-08-06T17-05-15-607-10178.log:31` 输出 `fan_in=InjectedFailed`。
- 机制证据：`crates/ralph-core/src/supervisor/phase.rs:133` 中 `elapsed_secs > aggregate_timeout_secs` 先于 fan-in 完成判断。
- 配置证据：`crates/ralph-core/src/config/loop_config.rs:1287` 中 `default_supervisor_aggregate_timeout_secs() = 600`；`presets/en/implementation-review.yml:101` 中 `supervisor.max_concurrent_workers: 6` 但未声明 aggregate timeout。
- 期望 outcome：fan-in 与 dispatcher 共享同一有效 aggregate deadline；6 slots 全部完成且 elapsed 落在合法区间内时必须返回 `Integrate`（产出 `review.wave.complete`），不再被误判 timeout。

## Scope Boundaries

### In Scope

- 改动 dispatcher terminal fan-in 调用点，复用 dispatcher 执行阶段使用的同一有效 aggregate deadline。
- 抽出 `effective_detected_aggregate_timeout_secs(&DetectedWave) -> u64`，让 fan-in 调用点和执行阶段共用同一计算公式。
- 在 dispatcher 测试中新增回归：6 slots、worker 900s、elapsed 726s 应走 `review.wave.complete`；同样配置下 elapsed 超过该 deadline 仍必须 timeout。

### Out of Scope

- 改 `crates/ralph-core/src/supervisor/phase.rs` 的判断顺序（语义保留）。
- 改 `SupervisorConfig.aggregate_timeout_secs` 的全局默认 600（该字段服务其它 supervisor wave，扩面）。
- 临时改 `presets/en/implementation-review.yml` 把 aggregate_timeout 设为 930 作为唯一修复（绕过，不治根）。
- 修复 `w-rs-1`/`w-2` public/store wave-id 映射产生的 orphan 告警，独立问题。
- 修复 `summary.md`/`handoff.md` 把 blocked 终态错写为成功，独立问题。

## Requirements

### R1 共享 deadline

fan-in 调用点必须把与 dispatcher 执行阶段完全一致的有效 aggregate deadline 写入 `PhaseInputs.aggregate_timeout_secs`；在 `hat.aggregate.timeout` 或 `consumer_aggregate_timeout` 显式配置时取该值，未配置时回退到 worker/batch 公式。deadline 数值不再来自 `SupervisorConfig.aggregate_timeout_secs`。

### R2 公共 helper

把 deadline 计算收敛到 `effective_detected_aggregate_timeout_secs(&DetectedWave) -> u64`；dispatcher 与 fan-in 调用点都通过该 helper 取值；helper 内部允许复用 `aggregate_timeout_for(per_worker, events.len(), concurrency)` 现有 batch 公式。

### R3 回归测试

新增回归测试覆盖：6 slots、worker 900s、elapsed 726s、fan-in 走 supervisor bridge 调用路径时必须返回 `Integrate`（不出现 `Failed(Timeout)`）；同一配置下 elapsed 大于该 deadline 时仍必须 `Failed(Timeout)`。

### R4 风格与门禁

- `cargo fmt --all -- --check` 必须为 0 diff。
- `cargo nextest run -p ralph-cli --bin ralph -- <substring>` 走 nextest，不裸 `cargo test`（项目硬规则）。
- `./scripts/run-tests.sh` 在落盘前最终门禁通过。

## Key Technical Decisions

### KTD-1 deadline SSOT 收敛到 helper

**决策**：在 `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` 抽 `fn effective_detected_aggregate_timeout_secs(&DetectedWave) -> u64`，统一显式 `hat.aggregate.timeout` → `consumer_aggregate_timeout` → `aggregate_timeout_for(per_worker, events.len(), concurrency)` 三层 fallback。fan-in 调用点与执行阶段共用该函数。

**理由**：当前两条 deadline 链在 dispatcher 内部已并行存在，仅 fan-in 调用点误用了 `SupervisorConfig`。收敛到 helper 后 future drift 风险最小，phase.rs 语义保持稳定。

**Alternatives considered**：

- 改 `SupervisorConfig.aggregate_timeout_secs` 默认值为 930：扩大影响面，破坏其它 supervisor wave 的契约。
- 在 preset 临时加 `event_loop.supervisor.aggregate_timeout_secs: 930`：仅绕过本次 preset，其它 default-wave preset 同类问题仍存。
- 在 `phase.rs` 调整判断顺序（先看 completed 再看 timeout）：篡改语义；正常 timeout wave 仍需 fast-fail。

## High-Level Technical Design

```
┌──────────────────────────────┐
│ DetectedWave (hat.aggregate? │
│  consumer_aggregate_timeout?  │
│  per_worker, events, concur.) │
└─────────────┬────────────────┘
              │ effective_detected_aggregate_timeout_secs()
              │
   ┌──────────┴──────────────────────────┐
   │                                      │
execute (spawn path)              fan-in (terminal)
  Duration::from_secs(...)            PhaseInputs {
  → workers                          aggregate_timeout_secs,
  → per-slot retries                 elapsed_secs,
                                     cancel_requested,
                                   }
                                         │
                                  evaluate_phase
                                         │
                         ┌───────────────┴───────────────┐
                         │                               │
            elapsed_secs > aggregate_timeout_secs    Integrate
                  → Failed(Timeout)              → review.wave.complete
```

- dispatch side (`dispatcher.rs:1385-1393` 与 `1743-1746`) 已经使用 `wave.aggregate_timeout_secs()` 或 `aggregate_timeout_for(...)`；fan-in 侧（约 919-939）误读了 `SupervisorConfig.aggregate_timeout_secs`。
- 修复后两路收敛到同一 helper，避免两条链漂移；`phase.rs` 与 `run_supervisor_fan_in` 不变。

## Implementation Units

### U1. 收敛 effective_detected_aggregate_timeout_secs helper

- **Goal**: 在 `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` 抽出公共 helper，让 dispatch 路径与 fan-in 调用点共用同一有效 aggregate deadline。
- **Requirements**: R1, R2
- **Dependencies**: 无
- **Files**:
  - `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`（抽出 helper，复用到两处）
- **Approach**: 在文件内新增 `fn effective_detected_aggregate_timeout_secs(wave: &ralph_core::DetectedWave) -> u64`，逻辑为：若 `wave.has_explicit_aggregate_timeout()` 或 `wave.consumer_aggregate_timeout.is_some()` 则返回 `wave.aggregate_timeout_secs()`；否则返回 `aggregate_timeout_for(wave_timeout, wave.events.len(), wave.hat_config.concurrency as usize)`（`wave_timeout` 由 `Duration::from_secs(wave.per_worker_timeout_secs())` 得到）。把 dispatch 路径（约 1385-1393 与约 1743-1746）的内联表达式替换为 helper 调用。
- **Patterns to follow**: 现有 `aggregate_timeout_for`（约 4800 行）与 `effective_wave_deadlines`（`crates/ralph-core/src/flow_lifecycle.rs:735`）的双 helper 风格；保持参数语义、避免传 `Duration`/整数混用。
- **Test scenarios**:
  - Happy path: 6 slots、worker `timeout=900`、无显式 `hat.aggregate`，helper 返回约 930（与现有 `aggregate_timeout_for` 公式一致）。
  - Edge case: `hat.aggregate.timeout=300` 时 helper 优先返回 300。
  - Edge case: `consumer_aggregate_timeout=Some(450)` 但 `hat.aggregate` 未设，helper 返回 450。
  - Error path: events 长度为 0、concurrency 为 0 时回退到 `per_worker_timeout_secs()`，不 panic（参考 `wave_detection.rs` 默认 300s 行为）。
  - Integration: helper 在 dispatcher spawn 路径与 terminal fan-in 路径调用同一函数得到的值完全相等（写一个内部断言，编译期保证）。
- **Verification**: `cargo nextest run -p ralph-cli --bin ralph -- effective_detected_aggregate_timeout_secs`；通过现有 dispatcher deadline 测试不回归。

### U2. fan-in 调用点复用 helper

- **Goal**: 把 `run_supervisor_fan_in` 之前的 `aggregate_timeout_secs` 取值从 `event_loop.config().event_loop.supervisor.aggregate_timeout_secs` 改为 helper。
- **Requirements**: R1
- **Dependencies**: U1
- **Files**:
  - `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`（修改约 919-938 的 terminal fan-in 调用）
- **Approach**: 删除局部 `let aggregate_timeout_secs = event_loop.config().event_loop.supervisor.aggregate_timeout_secs;`，直接调用 `effective_detected_aggregate_timeout_secs(detected)` 并传给 `run_supervisor_fan_in`。保留 `run_supervisor_fan_in` 的签名不变，避免破坏现有 `terminal_context_preserves_elapsed_timeout_relation` 测试。
- **Patterns to follow**: 现有 helper 调用约定；不破坏 `TerminalFanInContext` 的 `cancel_requested` 与 `elapsed` 透传。
- **Test scenarios**:
  - Happy path: 在 dispatcher 测试中构造 supervisor-enabled `EventLoopConfig`、`DetectedWave`（6 events，concurrency=6，per_worker=900），调用 `effective_detected_aggregate_timeout_secs(detected)` 验证结果约 930（不取 600）。
  - Edge case: 同步覆盖 `phase.rs` 边界（`elapsed_secs == aggregate_timeout_secs` 不应 timeout），参考 `terminal_context_preserves_elapsed_timeout_relation` 既有断言。
  - Integration: 模拟 6-slot wave、elapsed=726s、bridge 返回 `ContinueCollect` + `Integrate`，断言 `SupervisorFanInOutcome` 走 `InjectedComplete` 而不是 `InjectedFailed`。
- **Verification**: `cargo nextest run -p ralph-cli --bin ralph -- terminal_context_preserves_elapsed_timeout_relation`；新增的 6-slot/726s 测试通过。

### U3. 6-slot/726s 不应 timeout 回归覆盖

- **Goal**: 在 `dispatcher/tests/misc.rs` 新增回归测试，固定本次失败模式不再发生。
- **Requirements**: R3
- **Dependencies**: U1, U2
- **Files**:
  - `crates/ralph-cli/src/loop_runner/wave/dispatcher/tests/misc.rs`（在 `terminal_context_preserves_elapsed_timeout_relation` 附近新增）
- **Approach**: 使用现有 `make_wave` / `CapturingBridge` helper，构造 6 个 event、`DetectedWave.timeout=900`、`detected.events.len()=6`、`concurrency=6`，调用 `effective_detected_aggregate_timeout_secs(detected)` 得约 930，再用 `PhaseInputs { aggregate_timeout_secs, elapsed_secs: 726, cancel_requested: false }` 调 `evaluate_phase`，断言返回 `Integrate`（不应为 `Failed(Timeout)`）。再构造 `elapsed_secs = 931` 断言仍返回 `Failed(Timeout)`。
- **Patterns to follow**: 既有 `terminal_context_preserves_elapsed_timeout_relation`（约 1576 行）使用 `CapturingBridge` + `tick_with_slot_events` 的写法；保证测试是 process-per-test 隔离。
- **Test scenarios**:
  - Happy path: 6 slots / 726s elapsed → `Integrate`，与本次诊断报告中“6 reviewers all completed”一致。
  - Boundary: elapsed = deadline → `Integrate`（不超时）。
  - Error path: elapsed = deadline + 1 → `Failed(Timeout)`，确认 timeout 路径未丢失。
  - Integration: helper 调用两路径结果一致（编译期）。
- **Verification**: `cargo nextest run -p ralph-cli --bin ralph -- terminal_context_preserves_elapsed_timeout_relation` 及新增测试名通过。

### U4. 风格与门禁收尾

- **Goal**: 满足 `cargo fmt --all -- --check` 与 nextest 收尾验证。
- **Requirements**: R4
- **Dependencies**: U1, U2, U3
- **Files**:
  - 当前修改文件本身
- **Approach**: 跑 `cargo fmt --all`，跑 `cargo nextest run -p ralph-cli --bin ralph -- <substring>` 收尾。若 `./scripts/run-tests.sh` 在受限环境运行，优先定向回归到本计划相关的子集。
- **Test scenarios**:
  - `cargo fmt --all -- --check` 0 diff。
  - `cargo nextest run -p ralph-cli --bin ralph -- terminal_context_preserves_elapsed_timeout_relation` 通过。
  - `cargo nextest run -p ralph-core -- wave_detection` 通过。
- **Verification**: 全部 0 失败。

## Risks & Dependencies

- **风险 R-1**：把 helper 抽到 dispatcher.rs 内部后，未来 fan-in 与执行阶段仍可能因为新调用点绕开 helper 而再次漂移。**缓解**：在 helper 上加编译期断言或 doc-comment，明确要求所有 fan-in 调用点必须走 helper；U3 测试同时覆盖两个调用点数值相等。
- **风险 R-2**：`SupervisorConfig.aggregate_timeout_secs` 在其它 supervisor wave 仍是有效字段。**缓解**：本次不动默认 600；仅让 fan-in 调用点优先走 wave-derived deadline。
- **风险 R-3**：现有 `terminal_context_preserves_elapsed_timeout_relation` 测试用 `aggregate_timeout_secs=60`，未来若有人改动 `PhaseInputs` 默认值会回归。**缓解**：保留测试现场数值，不动 U3 测试外的现有常量。
- **依赖**：项目必须使用 nextest（HARD RULE 1），本计划已遵守。

## Definition of Done

- [ ] U1-U3 实现完成，`cargo fmt --all -- --check` 0 diff。
- [ ] `cargo nextest run -p ralph-cli --bin ralph -- terminal_context_preserves_elapsed_timeout_relation` 与新增 6-slot/726s 测试通过。
- [ ] `cargo nextest run -p ralph-core -- wave_detection` 通过。
- [ ] `./scripts/run-tests.sh`（或定向 nextest 子集）通过；本计划不涉及 preset/lint 改动，不需单独跑 `preset_lint`。
- [ ] 同一 wave 不再因本次根因被误判 timeout；若该 wave 在新版本下复跑，`fan_in` 走 `InjectedComplete` 而非 `InjectedFailed`。

## Open Questions

- 无阻塞；若实现后发现 helper 命名/位置需要拆分到 `crates/ralph-core`，回到计划重排而非自由变更。

## Deferred to Follow-Up Work

- `w-rs-1` / `w-2` public/store wave-id 映射产生的 orphan 告警；见 `docs/report/2026-08-06-implementation-review-primary-20260806-090515-diagnosis.md` P2。
- `summary.md` / `handoff.md` 把 blocked 终态写为成功的回归；见同一报告 P1。