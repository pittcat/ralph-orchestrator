# U10 验证报告 — 统一编排状态重构（U0-U9）

**日期**: 2026-06-22
**执行 agent**: Ralph U10 单元执行 agent
**分支**: `pittcat-dev`
**范围**: 全 workspace nextest + doctest + BDD scenarios + smoke replay
**状态**: **PASS（默认状态）/ FAIL with known U6 gap（开启所有 feature flag）**

---

## TL;DR

- **默认状态（feature flag 全关）**：5075 测试全过（15 skipped，1 leaky 子进程清理延迟），doctest 18 passed，BDD 63 passed，smoke 57 passed。`RALPH_BASELINE_SERIAL=1` 兜底也通过。
- **开启所有 feature flag（`UNIFIED_STATE_LEDGER=1` + `UNIFIED_PROTOCOL_VIEW=1` + `UNIFIED_POLICY_CHECK=1` + `UNIFIED_DETERMINISTIC_CORRECTION=1`）**：5059 通过 + **16 失败** + 15 skipped。失败归为 2 类：
  - **14 条 U6 unified pipeline 已知语义 gap**（`commands::emit::tests::*` + `integration_emit_policy::*`）：unified pipeline 不读 `events.jsonl` 中已有的 terminal/business 状态，导致 `business_after_terminal`、`duplicate_terminal` 拒绝行为与 legacy path 不一致
  - **2 条测试设计缺陷**（`u3_feature_flag_default_off_explicit_on` + `pipeline_records_protocol_view_feature_flag`）：断言依赖 `UNIFIED_PROTOCOL_VIEW` 环境变量**未设置**的状态，无法在 env=1 下与共享进程 env 的其他测试共跑

这两个失败都属于 **U10 范围外**（U6 unified pipeline 是已知的 partial migration；测试隔离缺陷是 KTD-8 阶段遗留）。**没有新增 `#[ignore]`**（U7 引入的 2 条已在 commit `afdca02`/`104a296` 范围内）。

---

## 1. 验证矩阵

### 1.1 默认状态（无 env flag）

执行入口：每个包分别跑 nextest（默认 profile，ralph-cli 走 cli-serial 串行组，其他包并行），最后跑 doctest + BDD + smoke。

| 包 | 测试数 | Passed | Skipped | Leaky | Failed | 时间 |
|---|---|---|---|---|---|---|
| `ralph-proto` | 88 | 88 | 0 | 0 | 0 | 0.10s |
| `ralph-core` | 2655 | 2655 | 3 | 0 | 0 | 13.0s |
| `ralph-adapters` | 336 | 336 | 9 | 0 | 0 | 5.2s |
| `ralph-telegram` | 95 | 95 | 0 | 0 | 0 | 2.1s |
| `ralph-tui` | 259 | 259 | 0 | 0 | 0 | 0.4s |
| `ralph-api` | 77 | 77 | 0 | 0 | 0 | 2.4s |
| `ralph-bench` | 2 | 2 | 0 | 0 | 0 | 0.0s |
| `ralph-cli`（串行） | 1219 | 1219 | 3 | 1 | 0 | 112s |
| **合计** | **5075** | **5075** | **15** | **1** | **0** | **151s** |

#### 1.1.1 全 workspace 一次跑（`cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast`）

首次执行时 `ralph-core hooks::executor::tests::run_writes_json_payload_to_hook_stdin` 失败（nextest 并发 Mutex 中毒）。单跑该 crate（39 tests, 1.0s）全部通过；二次跑全 workspace 5075 全过。**这是 pre-existing flake，根因是 hooks executor 写 subprocess I/O 与并发进程的 CPU 抢占**（U1/U3/U4/U7 报告里都遇到过相同 flake）。

#### 1.1.2 Doctest（`cargo test --workspace --exclude ralph-e2e --doc`）

| Crate | Passed | Failed | Ignored |
|---|---|---|---|
| ralph-cli | 0 | 0 | 0 |
| ralph-core | 17 | 0 | 5 |
| ralph-proto | 0 | 0 | 0 |
| ralph-adapters | 0 | 0 | 0 |
| ralph-telegram | 0 | 0 | 0 |
| ralph-tui | 1 | 0 | 0 |
| **合计** | **18** | **0** | **5** |

5 个 ignored doctest 在源码中显式标注（不属于 fail 列表）。

#### 1.1.3 BDD scenarios（`cargo nextest run -p ralph-core --test scenarios`）

**63 passed, 2 skipped, 0 failed**（5.1s）。

#### 1.1.4 Smoke replay（`cargo nextest run -p ralph-core --features recording --test smoke_runner`）

**57 passed, 0 skipped, 0 failed**（0.1s）。

#### 1.1.5 RALPH_BASELINE_SERIAL 兜底

`RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` → `✅ 测试通过(serial fallback)`。默认状态下无真失败。

---

### 1.2 开启所有 feature flag

环境变量：`UNIFIED_STATE_LEDGER=1` + `UNIFIED_PROTOCOL_VIEW=1` + `UNIFIED_POLICY_CHECK=1` + `UNIFIED_DETERMINISTIC_CORRECTION=1`

注：`UNIFIED_VALIDATION` 和 `UNIFIED_HANDOFF_AUTO` 在源码中**未实际读取**（`from_config` 直接构造，文档注释过时），故等价于默认启用——无需 env var。

执行：`cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast`

**Summary**: 5075 tests run: **5059 passed (1 leaky), 16 failed, 15 skipped**（149.98s）。

#### 1.2.1 失败清单（16 条）

**类别 A — U6 unified pipeline 已知 gap（14 条）**

`run_policy_check_unified` 在 `crates/ralph-cli/src/policy_check.rs:740` 使用 `LedgerSnapshot::cold_start()`，**不读** `events.jsonl` 中已有的 `LOOP_COMPLETE` / `business_topics` / `terminal_topics` 历史状态。因此开启 `UNIFIED_POLICY_CHECK=1` 后：

- **CLI emit 不再触发 business-after-terminal 拒绝**（`run_policy_check_unified` 不知道 terminal 已观察到）
- **CLI emit 不再触发 duplicate-terminal 拒绝**（同样原因）
- 这些测试的预期是"emit 在 strict config + 已经 LOOP_COMPLETE → 试图再发 experiment.planned 时应该失败"，但 unified path 直接接受

```text
ralph-cli::integration_emit_policy
  test_ce_executor_serial_coordinator_review_passed_rejected_dimensions_complete
  test_emit_isolated_mode_rejects_coordinator_aggregate_timeout
  test_emit_with_builtin_preset_rejects_string_payload
  test_emit_with_env_hats_source_rejects_string_payload_for_work_ready

ralph-cli::bin/ralph commands::emit::tests
  test_emit_explicit_policy_check_behavior_preserved
  test_emit_policy_check_default_keeps_legacy_path
  test_emit_policy_check_fallback_to_args_file_when_marker_missing
  test_emit_policy_check_rejects_business_after_terminal_with_marker
  test_emit_strict_config_rejects_duplicate_terminal_without_policy_check_flag
  test_emit_strict_config_rejects_missing_required_field_without_policy_check_flag
  test_emit_unsafe_bypass_rejected_when_config_denies
  test_fixture_cli_business_after_terminal_rejected
  test_fixture_cli_duplicate_terminal_rejected
  test_fixture_cross_cutting_cli_and_event_loop_agree
```

**根因**：`crates/ralph-cli/src/policy_check.rs:740` 把 `LedgerSnapshot::cold_start()` 传给 `pipeline.validate_with_preview`，导致 unified pipeline 无历史事件上下文。

**修复路径**（U10 范围外）：在 `run_policy_check_unified` 中加载 `events.jsonl` 到 `LedgerSnapshot`，或新增一个 `policy_event_history` 规则读取 terminal/business 状态。属于 U6 的 migration 已知 gap（详见 U6 commit `c900cc4` 注释"legacy path remains the default to avoid breaking existing scripts"）。

**类别 B — 测试隔离缺陷（2 条）**

```text
ralph-core preset::engine::protocol::tests::u3_feature_flag_default_off_explicit_on
ralph-core validation::tests::pipeline_records_protocol_view_feature_flag
```

两条测试都断言"**默认（env 未设置）**时 `feature_enabled == false`"。但在 nextest 默认并发下，多个测试共享同一进程的 env（`std::env::set_var` 是进程级），`ProtocolView::from_event_loop` 在 env=1 下读到的就是 `true`，断言失败。

**单跑通过**（`env -u UNIFIED_PROTOCOL_VIEW` 单独跑就过）。这是 KTD-8 env-var-based feature flag 的固有问题——单元测试必须用 `env::set_var` / `env::remove_var` 在测试体内管理 env，而不是依赖外部 env。

**修复路径**（U10 范围外）：用 `serial_test` crate 或 `std::sync::Mutex` 串行化这两条测试 + 用 `from_event_loop_with_index_and_feature` 这种带显式参数的入口替代读 env 的入口。

#### 1.2.2 Doctest + BDD + Smoke（flags=on）

| 项目 | 结果 |
|---|---|
| `cargo test --workspace --exclude ralph-e2e --doc` | 18 passed, 0 failed, 5 ignored（与默认状态一致） |
| `cargo nextest run -p ralph-core --test scenarios` | 63 passed, 2 skipped, 0 failed |
| `cargo nextest run -p ralph-core --features recording --test smoke_runner` | 57 passed, 0 skipped, 0 failed |

BDD scenarios 和 smoke replay 在 flags=on 下也全过，证明 U1-U9 的新代码路径（StateLedger / ProtocolView / ValidationPipeline / CorrectionContext / loop.resume）对**集成路径**无回归。CLI emit 单元测试的失败是 U6 policy pipeline 的局部 gap，不影响 loop runner 主链路。

---

## 2. Pre-existing flake 列表

| Flake 测试 | 触发场景 | 单跑结果 | 根因 |
|---|---|---|---|
| `ralph-core hooks::executor::tests::run_writes_json_payload_to_hook_stdin` | 全 workspace nextest 并发首跑 | PASS（39/39 hooks tests） | subprocess I/O 与并发 CPU 抢占 |
| `ralph-core hooks::executor::tests::run_truncates_stdout_and_stderr_at_max_output_bytes` | 全 workspace + `UNIFIED_STATE_LEDGER=1` | PASS | 同上 |
| `ralph-cli loop_runner::tests::test_process_pending_merges_redirects_subprocess_output_to_log_file` | 全 workspace + `UNIFIED_STATE_LEDGER=1` | PASS | loop_runner 的 4 个 process-global Mutex（`MOCK_ACP_EXECUTIONS` 等）+ sleep CPU 抢占 |

3 条 flake 全部在单跑时通过，根因已知（CLAUDE.md HARD RULE 1+2 注释）。**RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh** 兜底后无 flake。

---

## 3. `#[ignore]` 测试清单

| 文件 | 行 | 原因 | Follow-up |
|---|---|---|---|
| `crates/ralph-cli/src/display.rs` | 616 | visual demo；手工跑 `--ignored --nocapture` | N/A（demo 用） |
| `crates/ralph-cli/src/hats.rs` | 1232, 1255 | requires live AI backend | N/A（live-only） |
| `crates/ralph-core/tests/scenarios.rs` | 1728 | U7a follow-up: `correction::emit_correction_context` production wire-up on policy rejection path | U7a migration（已知遗留：legacy `publish_policy_rejection_resume` 仍发 `task.resume`，未切到 deterministic-correction path） |
| `crates/ralph-core/tests/scenarios.rs` | 1747 | 同上 | 同上 |
| `crates/ralph-core/src/event_loop/tests/wave_recovery_timeout.rs` | 233 | 跨 wave_id 收敛 responder 暂不支持 | KTD-U4-5 已知 follow-up（详见 plan §12） |
| `crates/ralph-adapters/tests/acp_executor_integration.rs` | 48, 82, 115, 173, 260 | requires live kiro-cli | N/A（live-only） |
| `crates/ralph-adapters/tests/pty_executor_integration.rs` | 324, 405, 487 | Requires pi CLI + API credentials | N/A（live-only） |
| `crates/ralph-e2e/src/...` | 多处 | requires live backend / requires ralph binary | N/A（e2e 范围外，U10 已 `--exclude ralph-e2e`） |

**U0-U9 范围新增 `#[ignore]`**：
- `crates/ralph-core/tests/scenarios.rs:1728, 1747`（commit `afdca02` U7 引入）：2 条，标记为 U7a follow-up

无其他新增 `#[ignore]`。

---

## 4. Feature Flag 默认开启 vs 旧路径建议

按计划 U10 的最后一行：

> "特性开关默认开启，旧路径代码在 U10 后标记为 deprecated 并在后续版本中移除。"

**当前状态**：
- `UNIFIED_VALIDATION` / `UNIFIED_HANDOFF_AUTO`：源码里没有 env var 读取（注释过时），等价于**默认启用**。U4 validation pipeline 已替换所有 runtime 路径。
- `UNIFIED_STATE_LEDGER`：env-var opt-in；U1 实现已 commit，但**默认关闭**（runtime production wiring 仍走 StateProjector + tasks_cache / progress_cache，已 deprecated 但保留兼容）。
- `UNIFIED_PROTOCOL_VIEW`：env-var opt-in；U3 实现已 commit，但**默认关闭**（preset engine 仍用 `from_event_loop` 内部默认行为）。
- `UNIFIED_POLICY_CHECK`：env-var opt-in；U6 实现已 commit，但**默认关闭**（CLI emit 仍走 legacy `validate_event_with_hat`）。
- `UNIFIED_DETERMINISTIC_CORRECTION`：env-var opt-in；U7 实现已 commit，但**默认关闭**（publish_policy_rejection_resume 仍发 `task.resume`）。

**建议（U10 范围内不动源码，仅记录）**：

1. **下个 release（v0.5+）将 4 个 env-var flag 的默认值反转为开启**：
   - `UNIFIED_STATE_LEDGER` → 默认开（StateProjector 内的 deprecated field 可移除）
   - `UNIFIED_PROTOCOL_VIEW` → 默认开
   - `UNIFIED_POLICY_CHECK` → 默认开（**前置条件**：先修 §1.2.1 类别 A 的 14 条测试 gap）
   - `UNIFIED_DETERMINISTIC_CORRECTION` → 默认开（**前置条件**：U7a follow-up，wire-up `emit_correction_context` 到 policy rejection 路径）
2. **flag 保留 1 个 minor version**（约 6 个月）作为 escape hatch
3. **flag 移除后**：删除 `crates/ralph-core/src/state/ledger.rs:22` 等位置的 env var 读取、`U*_FEATURE_FLAG_DOC` 注释、`docs/solutions` 中的 KTD-8 文档
4. **旧路径代码清理顺序**：
   - U10 后立刻：给 `state_projector::ProjectionContext::tasks_cache` / `progress_cache` 加 `#[deprecated]`（已有 deprecation warning）
   - U11+ 移除 `legacy_validate_event_with_hat` 函数（`crates/ralph-cli/src/policy_check.rs:572` 周边）
   - U12+ 移除 `MOCK_ACP_EXECUTIONS` / `FAKE_PATH_BACKEND_*` 等 process-global Mutex（前提是 HARD RULE 1 也移除——根因修复后）

---

## 5. 整体 PASS/FAIL 状态

| 验证项 | 默认状态 | 开启所有 flag |
|---|---|---|
| 全 workspace nextest | **PASS**（5075/5075，15 skipped） | **FAIL**（5059/5075，16 failed） |
| Doctest | **PASS**（18/18，5 ignored） | **PASS**（18/18，5 ignored） |
| BDD scenarios | **PASS**（63/63，2 skipped） | **PASS**（63/63，2 skipped） |
| Smoke replay | **PASS**（57/57） | **PASS**（57/57） |
| RALPH_BASELINE_SERIAL 兜底 | **PASS** | N/A（不在兜底范围） |

**整体状态**：**PASS（生产路径无回归）/ 已知 U6 gap（16 条单元测试需要 U6 follow-up）**

U0-U9 重构对**生产 runtime 路径**（loop runner 主链路、BDD scenarios、smoke replay、doctest）零回归。`UNIFIED_POLICY_CHECK=1` 路径下的 14 条 emit 单元测试失败是 U6 迁移的已知 gap，**不是 U10 的真失败**，应在后续 U6a follow-up 中修复（统一 `LedgerSnapshot` 与 `events.jsonl` 历史状态的接入路径）。

---

## 6. 参考

- 计划：`docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md`（U10: line 654-685）
- U0-U9 commits（已合入 `pittcat-dev`）：
  - `ab72546` U0 inventory
  - `6dbbe94` U1 StateLedger
  - `6f79eeb` U2 StateProjector derives
  - `47ca011` U3 ProtocolView
  - `b1c8f5a` U4 unified validation pipeline
  - `1fb86b6` U5 handoff artifact auto-gen
  - `c900cc4` U6 migrate --policy-check
  - `afdca02` U7 deterministic correction + loop.resume
  - `4dd2913` U8 diagnose + continue adapted
  - `104a296` U9 migrate tests + BDD scenarios
- 项目硬规则：CLAUDE.md HARD RULE 1+2（nextest 入口 + ralph-cli 串行）
- 测试入口：`scripts/run-tests.sh`（默认 nextest，兜底回退单线程 cargo test）
