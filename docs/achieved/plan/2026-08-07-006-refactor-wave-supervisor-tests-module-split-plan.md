# 将 wave_supervisor 测试拆分为按行为族组织的目录模块

## 0. 计划状态

- 状态：`READY`；更新日期 2026-08-07；实施基线：`5791e21b`。
- 调查范围：`crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs`（**注意：是 `tests/` 目录下的扁平测试文件，不是 `wave/` 下的生产模块**），由 `crates/ralph-cli/src/loop_runner/tests/mod.rs:26` 的 `mod wave_supervisor;` 接入；顶部含 SpyBindingBridge / RecordingFactory / production_bridge_with_factory 等共享 fixture；后续按 U1/U2/U3/U4/U5/U6/U7/S1/S2 等命名空间覆盖 bind_slot、cap/barrier、ledger、fan-in、salvage、redrive、retry budget 等行为族。
- 当前验证：`cargo build --workspace`、`just fmt-check`、`just lint` 和全量回归 `./scripts/run-tests.sh` 均通过；Phase 1 7576/7576、Phase 2 23/23、doctest 19/19（4 ignored）。
- 已执行文件调查：当前 `wc -l` 9,423 行（验证）；245 个 `fn`（含测试与 helper）、108 个 `#[test] | #[tokio::test]` 属性、2 个 `const`、5 个 `use` 块；测试路径由 `tests/mod.rs` 的 `mod wave_supervisor;` 静态接入；通过 `super::super::*` 引用 `loop_runner` 的全部 `pub` 项，不直接依赖 `task_cli` 模块。
- 阻塞：无；共享文档 `.cursor/rules/state-management.mdc` 已由合并计划 002 独占处理，本计划**不修改**任何 `.cursor/rules/*.mdc`。
- 独立性确认：本计划与 `2026-08-07-001`（拆分 `task_cli.rs`）无编译/可见性依赖。`wave_supervisor.rs` 的依赖图是 `loop_runner`（父模块）+ `crate::loop_runner::wave::*`（生产 API）+ `ralph_core::supervisor::*`（共享类型），与 `task_cli` 路径完全不相交；可与 001 并行或独立串行执行。

### 0.1 0 回归硬门禁

本计划开始前必须在当前 HEAD 运行并保存结果：

- `./scripts/run-tests.sh`
- `cargo build --workspace`
- `just fmt-check`
- `just lint`
- `cargo nextest list --workspace`

每个 Unit 前后必须证明 targeted 命令实际命中了目标测试；ralph-cli 统一使用 `cargo nextest run -p ralph-cli --bin ralph -- <filter>`，不能凭模块名假设命中。纯重构验收必须保存：

1. **测试清单**：108 个 `#[test]/#[tokio::test]` 的多重集（含函数名、属性、字符串字面量）。
2. **测试函数体 hash**：每个 `#[test]/#[tokio::test]` 函数体的 SHA-256 前 16 字节。
3. **fixture 清单**：`SpyBindingBridge`、`RecordingFactory`、各 `setup_*`/`make_*` helper 的定义位置、可见性、引用计数。

禁止空 stub、把全部测试塞进 `misc.rs`、遗漏 fixture/测试、拆解巨型 helper、修改业务断言、通过削弱断言获得 Green。

## 1. 功能目标

将 `tests/wave_supervisor.rs`（9,423 行扁平测试文件）拆分为 `tests/wave_supervisor.rs + tests/wave_supervisor/` 目录模块，按行为族重组测试与共享 fixture，保持 `ralph-cli` 的 supervisor bridge、slot binding、U3 partial failure、U4 cap/barrier、U5 ledger/redrive、U6 production fan-in、U7 lazy bridge、U1 redrive payload、salvage、retry budget 等断言零变化。

调用方是 `crates/ralph-cli/src/loop_runner/tests/mod.rs`（仅一个 `mod wave_supervisor;` 声明）。非目标是修改 dispatcher、supervisor_bridge、production bridge、共享 fixture 行为、事件协议或 `super::super::*` 暴露的 loop_runner API。

## 2. 代码库现状与证据

文件由 `tests/mod.rs` 的 `mod wave_supervisor` 接入，顶部以 `use super::super::*` + `use crate::loop_runner::wave::{BridgeError, MockSupervisorBridge, SlotBinding, SupervisorBridge, WaveWorkerExecutor, is_supervisor_path_enabled}` + `use ralph_core::supervisor::{PhaseInputs, SlotResource, TerminalEvidence, WaveKind, WaveSnapshot}` 开头。共享 fixture `SpyBindingBridge`(行 52-167)、`RecordingFactory`(行 1118-1170)、`production_bridge_with_factory`(行 1200-1232) 与按 U3/U5/U6 分组的 `setup_*`/`make_*` helper 跨测试族复用。

| ID | 来源 | 观察 | 影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `wc -l` + 实际测量 | 9,423 行扁平测试 | 单切片边界确定 | 高 |
| E2 | `rg "^fn \|^#\[(tokio::)?test\]"` | 245 个 fn、108 个 test；按前缀 U1/U2/U3/U4/U5/U6/U7/S1/S2 等分布 | 分组依据 | 高 |
| E3 | `tests/mod.rs:26` | 单点 `mod wave_supervisor;` 接入，目录模块需保持入口名一致 | 入口声明 | 高 |
| E4 | 行 42-49 use 块 | `super::super::*` + `crate::loop_runner::wave::*` + `ralph_core::supervisor::*`；**无 task_cli 依赖** | 可见性边界 | 高 |
| E5 | `AGENTS.md` + `justfile` | 时序/并发测试须 nextest + 两阶段 `./scripts/run-tests.sh` | 质量门禁 | 高 |
| E6 | 最近 Git history | supervisor bridge / fan-in / redrive 多次回归提交；既有 `enabled_false_uses_wave_tracker` / `enabled_true_calls_bridge_bind_slot` / `bridge_off_no_feature_returns_error_path` 是 P0 锚点 | 必须保留回归 | 高 |
| E7 | fixture 行号定位 | `SpyBindingBridge`(52-167)、`RecordingFactory`(1118-1170)、`production_bridge_with_factory`(1200-1232)、`setup_u3_partial_failure_bridge`(2783)、`setup_u6_production_bridge`(5103)、`make_u3_wave`(2850-2906)、`make_test_cli_backend`(2399)、`make_test_hat_registry`(7832)、`redrive_test_registry`(9059)、`make_redrive_parent_with_descriptors`(9075) | fixture 所有权分配 | 高 |

### 2.1 当前文件结构（行号锚点）

| 区域 | 行号范围 | 主要内容 | 行数（约） |
|---|---|---|---|
| 头部 doc + use | 1-49 | 文件 doc 注释、5 个 use 块 | ~50 |
| SpyBindingBridge + 早期 slot binding 测试 | 50-1110 | `SpyBindingBridge`、U9 happy path、enabled/dispatcher cap、env override、build_supervisor_bridge_*、U8 worker_timeout_prefix | ~1,060 |
| RecordingFactory + bridge/worktree 测试 | 1111-2398 | `RecordingFactory`、exec/fix/review kind、`bind_slot_failure_fail_closed`、`dispatcher_fail_closed_for_exec_bind_failure`、`test_bind_slot_factory_failure_returns_err`、`test_dispatcher_fail_closed_on_exec_bind_none` | ~1,290 |
| `make_test_cli_backend` + 后端 + U3 partial failure | 2399-2908 | 后端构造、U3 partial failure bridge 与 helper、`make_u3_wave` 系列 | ~510 |
| U3/U4 cap/barrier + U5 ledger/dedup + U6 fan-in 锚点 | 2909-5095 | `test_u4_cap4_barrier_releases_fifth_fifo_slot`、`u5_failed_event`、`u5_event`、`exec_reported_failure`、`completed_wave_of`、`U2_FRESH_PROCESS_BACKEND`、`u2_read_attempts`、`captured_env` | ~2,190 |
| U6 production_fan_in + ledger + U2/U5/U7 lazy bridge | 5096-6066 | `setup_u6_production_bridge`、`make_u6_completed`、`read_u6_ledger`、`test_production_fan_in_*`、`test_production_bridge_writes_real_ledger_not_in_memory`、`u2_*` lazy bridge、`u5_register_wave_if_absent_*`、`u7_lazy_bridge_*` | ~970 |
| U3 resolve emit / U5 record outcome / U6 failed payload / salvage | 6067-7316 | `test_u3_resolve_emit_path_dispatcher_signed_carve_out`、`test_u5_record_outcome_*`、`test_u6_failed_payload_exposes_per_slot_reasons`、`exec_fix_partial_failure_does_not_salvage_completed_slot_events`、`exec_wave_with_zero_completed_slots_*`、`review_wave_with_zero_completed_slots_*`、`review_partial_failure_salvage_path_unaffected`、`build_wave_failed_payload_includes_salvaged_redrive_fields_on_exec_path` | ~1,250 |
| U1 redrive payload + supervisor | 7317-9058 | `test_u1_single_fail_only`、`test_u1_partial_failure_*`、`test_u1_zero_fail_happy_path_no_redrive_payload`、`test_u1_mixed_failure_reasons`、`task_close_then_next_ready_two_wave_supervisor_path`、`make_test_hat_registry`、U4/U5 retry budget | ~1,740 |
| Redrive + 尾部测试 | 9059-9423 | `redrive_test_registry`、`make_redrive_parent_with_descriptors`、尾段 `#[tokio::test]` 测试 | ~370 |

合计 ~9,360 行（含测试体之间的间隔行）；与 `wc -l` 9,423 一致。

### 2.2 当前行为族与搬移边界

| 目标模块 | 包含 fn/test 前缀 | 行数估算 | 依赖的 fixture | 约束 |
|---|---|---|---|---|
| `fixtures.rs` | `SpyBindingBridge`、`RecordingFactory`、`production_bridge_with_factory`、`make_test_cli_backend`、`captured_env`、`make_test_hat_registry`、`redrive_test_registry`、`make_redrive_parent_with_descriptors`、`u5_failed_event`、`u5_event`、`exec_reported_failure`、`completed_wave_of`、`u2_read_attempts`、`make_u3_wave`/`make_u3_wave_with_concurrency`/`make_u3_wave_with_topic`、`setup_u3_partial_failure_bridge`、`setup_u6_production_bridge`、`make_u6_completed`、`read_u6_ledger`、`U5_RETRYABLE_REASON`、`U2_FRESH_PROCESS_BACKEND` | ~900 | 无外部依赖 | 唯一持有处；不得跨模块重复 |
| `slot_binding.rs` | `enabled_false_uses_wave_tracker`、`enabled_true_calls_bridge_bind_slot`、`bridge_off_no_feature_returns_error_path`、`build_supervisor_bridge_*`(5 个)、`slot_binding_env_overrides_worker_backend_env_keys`、`review_kind_bind_slot_returns_none_for_shared_readonly`、`recover_active_waves_at_startup_returns_report_on_empty_store`、`recover_pending_projections_closes_stale_task_and_is_idempotent`、`u8_bind_slot_env_does_not_contain_ralph_wave_id`、`s1_same_loop_different_waves_get_distinct_branches`、`u8_worker_timeout_prefix_constant_is_shared` | ~1,050 | `SpyBindingBridge` | 真实 bridge trait，**不**替换为 mock |
| `dispatch.rs` | `supervisor_capability_gate_truth_table`、`supervisor_disabled_does_not_call_bridge_builder`、`supervisor_enabled_isolated_invokes_bridge_builder_once`、`pipeline_disabled_workspace_has_no_supervisor_artifacts`、`exec_kind_produces_unique_branch_path_cwd`、`fix_kind_produces_unique_branch_path_cwd`、`review_kind_returns_shared_readonly_none`、`bind_slot_failure_fail_closed_no_main_workspace_write`、`production_bridge_default_factory_is_default_worktree_factory`、`dispatcher_fail_closed_for_exec_bind_failure`、`production_bridge_only_returns_none_for_review`、`test_build_supervisor_bridge_provides_context_for_exec`/`for_fix`/`review_returns_none`、`test_legacy_from_store_returns_none_for_exec`、`test_bind_slot_factory_failure_returns_err`、`test_dispatcher_fail_closed_on_exec_bind_none`、`test_u4_cap4_barrier_releases_fifth_fifo_slot`、`u4_production_bridge_forwards_slot_retry_budget_through_constructor`、`u4_register_wave_if_absent_call_sites_use_same_bridge_budget`、`u4_runner_rejects_out_of_range_slot_retry_budget`、`u5_worker_request_implements_clone`、`u5_slot_retry_budget_zero_closes_auto_retry_at_bridge_accessor`、`u5_slot_retry_budget_two_propagates_to_accessor`、`test_u3_resolve_emit_path_dispatcher_signed_carve_out` | ~2,500 | `SpyBindingBridge`、`RecordingFactory`、`production_bridge_with_factory`、`setup_u3_partial_failure_bridge`、`make_u3_wave*` | 保留真实 dispatcher / bridge 交互 |
| `timeouts.rs` | 含 U2 fresh process / U8 worker_timeout_prefix 锚点的 timeout、retry、aggregate deadline 测试；以 `U2_FRESH_PROCESS_BACKEND`、`u2_read_attempts`、captured_env 为核心；以 `s1_same_loop_different_waves_get_distinct_branches` 为隔离基线 | ~600 | `captured_env`、`u2_read_attempts`、`U2_FRESH_PROCESS_BACKEND` | 不修改 timeout 常量与时间断言语义 |
| `coordination.rs` | U5 ledger / dedup / register_wave_if_absent / content_hash / cap / lazy bridge default cap：`u5_register_wave_if_absent_is_idempotent_sso_t`、`u5_content_hash_is_part_of_record_slot_result_signature`、`u5_lazy_bridge_default_cap_is_unlimited`、`u2_no_phantom_bridge_when_no_detected_wave`、`u2_legacy_wave_tracker_surface_still_reachable`、`u2_register_failure_fails_closed`、`u2_lazy_bridge_uses_in_memory_store_trait_surface`、`test_u5_record_outcome_empty_success_is_failure`、`test_u5_record_outcome_partial_timeout_stays_result`、`exec_reported_failure` 周边 ledger 测试 | ~700 | `exec_reported_failure`、`completed_wave_of` | 保留 success/failure/partial 三类路径 |
| `salvage_merge.rs` | `exec_fix_partial_failure_does_not_salvage_completed_slot_events`、`exec_wave_with_zero_completed_slots_injects_failed_without_precommitted_salvage`、`review_wave_with_zero_completed_slots_injects_failed_without_precommitted_salvage`、`review_partial_failure_salvage_path_unaffected`、`build_wave_failed_payload_includes_salvaged_redrive_fields_on_exec_path`、`test_u6_failed_payload_exposes_per_slot_reasons` | ~700 | `make_u6_completed`、`read_u6_ledger` | **不**把 salvage 断言降级为"事件存在" |
| `supervisor.rs` | U7 lazy bridge：`u7_lazy_bridge_bind_slot_routes_to_production_trait_method`、`u7_review_default_is_shared_readonly`；U6 production fan-in：`test_production_fan_in_writes_ledger_and_injects_complete_once`、`test_production_fan_in_partial_failure_injects_failed`、`test_production_fan_in_sink_failure_defers_complete`、`test_production_bridge_writes_real_ledger_not_in_memory`、`test_production_fan_in_dedups_identical_business_events`；U3 G3 slot closure：`g3_record_never_started_marks_pending_slots_in_store`、`g3_cancel_closure_cancelled_slot_has_never_started_reason`；U1 redrive payload（不含 U1 happy path，放到 `redrive_payload.rs`）；`task_close_then_next_ready_two_wave_supervisor_path` | ~1,400 | `setup_u6_production_bridge`、`make_u6_completed`、`read_u6_ledger` | 保留 fail-closed 和 ledger/digest 断言 |
| `redrive_payload.rs` | U1 redrive payload 4 个测试：`test_u1_single_fail_only`、`test_u1_partial_failure_one_complete_one_fail`、`test_u1_zero_fail_happy_path_no_redrive_payload`、`test_u1_mixed_failure_reasons`；尾部 redrive 段以 `redrive_test_registry`、`make_redrive_parent_with_descriptors` 为核心 | ~900 | `redrive_test_registry`、`make_redrive_parent_with_descriptors` | payload 字段断言不削弱 |
| `misc.rs` | 仅收纳无法归类的小型稳定性/契约测试，**目标 ≤600 行** | 待统计 | 必要时复用 `fixtures.rs` | 禁止成为剩余大杂烩；超过 600 行必须继续按前缀拆 |

**共享 fixture 所有权规则**：`fixtures.rs` 是**唯一**持有 `SpyBindingBridge`、`RecordingFactory`、`make_test_cli_backend`、`captured_env`、`make_test_hat_registry`、`redrive_test_registry`、`make_redrive_parent_with_descriptors`、各 `make_u*`/`setup_u*` 的文件；其它模块通过 `super::fixtures::*` 引用，**不得**复制或重定义。

## 3. 决策记录与置信度

| ID | 决策问题 | 候选 | 选择 | 依据 | 置信度 |
|---|---|---|---|---|---|
| D1 | 目录结构 | `foo.rs + foo/`；`foo/mod.rs` | 前者（`wave_supervisor.rs` 保持文件名 + `wave_supervisor/` 兄弟目录） | E3；与 `tests/wave.rs + tests/wave/...` 先例一致 | 0.96 |
| D2 | 入口声明 | `tests/mod.rs` 改 `mod wave_supervisor;` 为声明目录；根 `wave_supervisor.rs` 仅 `mod xxx;` | 后者 | `loop_runner/tests/wave_supervisor.rs` + `wave_supervisor/*.rs` 目录模式 | 0.95 |
| D3 | 测试分组依据 | 按测试函数名前缀；按 E2 锚点行号；按 fixture 依赖 | 前缀 + 锚点 | E2/E7；U* 编号语义清晰 | 0.94 |
| D4 | fixture 所有权 | 首个消费者模块；独立 `fixtures.rs`；多处复制 | 独立 `fixtures.rs` | 跨 6 个以上行为族共享，必须集中 | 0.95 |
| D5 | 不变量 | 测试名多重集不变；测试函数体 hash 不变 | 二者 | 0 行为变更 + 0 漂移 | 0.97 |
| D6 | 与 001 关系 | 串行依赖；并行；冲突 | **并行可执行** | E4；`wave_supervisor.rs` 与 `task_cli.rs` 无可见性交叉 | 0.93 |
| D7 | 共享 mdc 所有权 | 本计划处理；合并计划 002 处理 | 不处理（002 独占） | CLAUDE.md「preset/schema 改动后的下游同步清单」HARD RULE；本计划无 mdc 引用 | 0.99 |

## 4. BDD 行为规格

```gherkin
Feature: wave_supervisor 测试目录模块化
  Scenario: supervisor bridge hot-path 回归完整
    Given 现有 SpyBindingBridge / RecordingFactory / production_bridge_with_factory fixture
    When 测试按行为族拆入 slot_binding / dispatch / timeouts / coordination / salvage_merge / supervisor / redrive_payload / misc
    Then `enabled_false_uses_wave_tracker` / `enabled_true_calls_bridge_bind_slot` / `bridge_off_no_feature_returns_error_path` 三个 U9 锚点全部通过
  Scenario: U3 partial failure / U4 cap/barrier / U5 ledger/dedup / U6 production fan-in / U7 lazy bridge / U1 redrive payload / salvage 边界不丢失
    Given 既有 aggregate deadline / ledger dedup / zero-completed / failure classification / retry budget 测试
    When 完成拆分
    Then 测试名多重集、计数、函数体 hash 与边界覆盖保持完整
  Scenario: 共享 fixture 唯一持有
    Given SpyBindingBridge / RecordingFactory / 各 setup_* / make_* helper
    When 拆分到 fixtures.rs
    Then 其它行为族模块只能 `use super::fixtures::*`，不得复制定义
  Scenario: 并发测试仍按既有隔离运行
    Given nextest 进程隔离与 run-tests 两阶段
    When 执行全量回归
    Then 无新增竞态失败；Phase 1 7576/7576、Phase 2 23/23、doctest 19/19（4 ignored）
  Scenario: 与 plan 001（task_cli 拆分）独立可并行
    Given task_cli 模块与 wave_supervisor 测试无可见性交叉
    When 任一计划先行完成
    Then 另一计划无新增冲突
```

## 5. 验收与测试策略

| 场景 | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
|---|---|---|---|---|---|
| 行为族测试通过 | slot_binding / dispatch / timeouts / coordination / salvage_merge / supervisor / redrive_payload / misc 各自绿 | `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor::slot_binding` 等 8 次 | 集成 | 时序测试需 phase 2 | 否 |
| 测试 ID 多重集一致 | 108 个 `#[test]/#[tokio::test]` 函数名多重集与拆分前一致 | `cargo nextest list --workspace` diff | 清单 | 多重集需保留 `wave_supervisor::xxx::test_name` 前缀 | 否 |
| 测试函数体 hash 一致 | 每个测试函数体的 SHA-256 前 16 字节与拆分前一致 | `git diff --stat` + 自定义 hash 清单 | 结构 | 任何 hash 漂移即停止 | 否 |
| 共享 fixture 唯一 | `rg "struct SpyBindingBridge\|struct RecordingFactory"` 仅命中 `fixtures.rs` | rg 静态扫描 | 结构 | 重复定义即停止 | 否 |
| 全量回归 | 两阶段脚本通过 | `./scripts/run-tests.sh` | 回归 | env scrub；时序隔离 | 否 |
| 编译完整 | workspace build 通过且无 warning | `cargo build --workspace` | 构建 | duplicate/missing item | 否 |
| 静态门禁 | fmt 与 clippy 通过 | `just fmt-check`、`just lint` | 静态 | warning 即停 | 否 |

## 6. 需求—测试追踪矩阵

| ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | wave_supervisor 测试按行为族拆分 | 行为族测试通过 | U1.1-U1.8 targeted | 原 108 测试 | wave/loop | 否 | E1-E3 |
| R2 | 测试 ID 多重集与函数体 hash 不变 | 测试 ID 多重集一致 | nextest list diff | 原 108 测试 | targeted | 否 | E2/E5 |
| R3 | 共享 fixture 唯一持有 | 共享 fixture 唯一 | rg 静态扫描 | 原 6+ 个 fixture | build | 否 | E4/E7 |
| R4 | deadline / ledger / fan-in / salvage 边界覆盖保留 | 边界不丢失 | 8 次 targeted + 全量 | 原边界测试 | wave/loop | 否 | E5/E6 |
| R5 | 与 plan 001 独立可并行 | 独立性 | 两计划分别绿 | 任一 plan | 任一 plan | 否 | D6/E4 |
| R6 | 0 运行时回归 | 全量门禁 | full/build/fmt/lint | 全部既有测试 | workspace | mock | E5 |

## 7. 严格串行开发单元

### Unit 1：wave_supervisor 测试目录模块化

1. **目标**：根 `wave_supervisor.rs` ≤200 行（仅 `mod xxx;` 声明与可能的 `pub(super)` re-export）；新增 `fixtures.rs`、`slot_binding.rs`、`dispatch.rs`、`timeouts.rs`、`coordination.rs`、`salvage_merge.rs`、`supervisor.rs`、`redrive_payload.rs`、`misc.rs` 9 个行为族文件；所有子文件 <5,000 行，`misc.rs` ≤600 行。
2. **对应**：R1/R2/R3/R4/R5/R6、全部 BDD Scenario、D1-D7、E1-E7。
3. **结果**：最大子文件 ≤2,500 行（dispatch），全部 <5,000；测试名多重集与函数体 hash 不变。
4. **基线**：当前 `wave_supervisor.rs` 包含 108 个 `#[test]/#[tokio::test]`、约 137 个 helper `fn`、2 个 const、5 个 use 块、6+ 个共享 fixture；均属本文件所有权；搬移前先生成：
   - 函数/常量/属性 manifest（每项含行号、签名、属性、字符串字面量）
   - 测试函数体 SHA-256 前 16 字节清单
   - fixture 引用计数（每个 fixture 被多少 `#[test]` 引用）
5. **输入/输出/副作用**：fixture 与断言不变；仅 `mod` 声明、`use super::fixtures::*`、`pub(super)` 可见性、`pub use fixtures::*` re-export。
6. **修改边界**：仅 `tests/wave_supervisor.rs`（根退化为目录入口）与新建 `tests/wave_supervisor/*.rs`；不动 `tests/mod.rs` 之外的任何 loop_runner / wave / supervisor_bridge 生产代码、不动 `.cursor/rules/*.mdc`、不动 `tests/wave.rs` 或 `tests/legacy.rs`。
7. **可依赖**：现有 `loop_runner::wave` 公共 API；`ralph_core::supervisor::*`；无其他计划变更。
8. **禁止**：
   - 修改 `SpyBindingBridge` / `RecordingFactory` / `production_bridge_with_factory` / 任何 `setup_*` / `make_*` helper 的字段、trait 实现、断言、字符串字面量；
   - 删除或合并任何 `#[test]/#[tokio::test]`；
   - 把全部测试塞进 `misc.rs`（超过 600 行必须按前缀继续拆）；
   - 拆解 `make_test_cli_backend` / `production_bridge_with_factory` 等长函数（保持单一完整函数）；
   - 修改 `super::super::*` 暴露的 loop_runner API；
   - 修改并发/超时常量、aggregate deadline、failure classification、payload schema。
9. **验收**：`cargo nextest list --workspace`（108 个 `wave_supervisor::*` 测试 ID）、8 次 targeted nextest（每个行为族模块）、全量回归、build、fmt、lint、fixture 唯一性扫描。
10. **Acceptance Red**：编译失败、测试函数体 hash 变化、测试 ID 多重集变化、fixture 重复定义、行数越界、公开签名变化任一即红；正确原因是搬移/声明不完整。
11. **单测（characterization-first）**：不新增；只做物理搬移。fixture 随首个使用族整体搬移，因跨 3+ 行为族共享，**统一抽到 `fixtures.rs`**，不得复制。
12. **顺序**（严格串行；后组可能依赖前组的 fixture re-export）：
    1. 快照：行号、测试函数体 hash、fixture 引用计数、use 块；
    2. 新建 `tests/wave_supervisor/fixtures.rs` 并整块搬入全部共享 fixture；
    3. 根 `wave_supervisor.rs` 退化为 `mod fixtures; mod slot_binding; ... mod misc;` 与必要的 `pub(super) use fixtures::*;`；
    4. 按 §2.2 表格 7 个行为族 + misc 逐个搬迁测试（slot_binding → dispatch → timeouts → coordination → salvage_merge → supervisor → redrive_payload → misc）；
    5. 每搬一个行为族立即 `cargo build --workspace` + 对应 targeted nextest；
    6. 全部搬完后跑 ID 多重集 diff + 函数体 hash diff + 全量回归；
    7. 静态门禁 `just fmt-check`、`just lint`。
13. **最小实现**：
    - M1：仅项级搬移（`fn` 整体、`#[test]` 整体、`const` 整体、`use` 整体）；
    - M2：精确导入（`use super::fixtures::*;` 与必要的 `use crate::loop_runner::wave::*;`）；
    - M3：路径式 `mod` 声明（`mod fixtures;` 等 9 个）；
    - M4：最小 `pub(super)`（仅当编译错误明确指向时）。
14. **集成**：`cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor::slot_binding`、`wave_supervisor::dispatch`、`wave_supervisor::timeouts`、`wave_supervisor::coordination`、`wave_supervisor::salvage_merge`、`wave_supervisor::supervisor`、`wave_supervisor::redrive_payload`、`wave_supervisor::misc` 共 8 次；`cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` 全量；`cargo build --workspace`；`cargo run -p ralph-e2e -- --mock`（最终）。
15. **风险测试**：
    - 共享 fixture 跨模块重复定义（rg 扫描）；
    - `super::super::*` 路径在新目录层级下被破坏（编译报错驱动）；
    - 测试函数体 hash 漂移（自定义 hash 清单比对）；
    - 时序 / 并发测试在并发 nextest 下退化（`./scripts/run-tests.sh` phase 1+2）；
    - 带 `RALPH_CURRENT_HAT` 等污染环境复跑（按 CLAUDE.md HARD RULE 5）。
16. **回归**：ralph-cli（`wave_supervisor`、`wave`、`runner`、`task`、`integration_supervisor_runtime_p0`、`integration_wave_channel_convergence`）、workspace、lint、fmt。
17. **文件**：`tests/wave_supervisor.rs` 根 + `tests/wave_supervisor/{fixtures,slot_binding,dispatch,timeouts,coordination,salvage_merge,supervisor,redrive_payload,misc}.rs` 9 个新文件。
18. **完成**：V2 全绿、ID 多重集 diff 空、函数体 hash diff 空、root 与子文件行数达标、diff 仅限计划文件、无 `.only` / `#[ignore]` / 削弱断言。
19. **停止**：发现业务断言差异、测试 ID 丢失、共享 fixture 行为差异、需改公共接口、需动 `.cursor/rules/*.mdc` 时停止并重新决策。
20. **缓解**：fixture 整体复制/移动，编译器驱动可见性；不编辑函数体；行号漂移由"测试函数体 hash 不变"保证。

### 计划审查补充：必须明确的执行证据

- **当前基线前置门禁**：执行开始前必须在当前 HEAD 运行并记录 `./scripts/run-tests.sh`、`cargo build --workspace`、`just fmt-check`、`just lint`、`cargo nextest list --workspace` 的退出码与命中数（Phase 1 = 7576/7576、Phase 2 = 23/23、doctest = 19/19、4 ignored）；任一失败即停止，不得进入拆分。
- **快照证据**：在首次移动前保存：
  1. 108 个测试函数名多重集 + 函数体 SHA-256 前 16 字节清单；
  2. 6+ 个 fixture（`SpyBindingBridge`、`RecordingFactory`、`production_bridge_with_factory`、`setup_u3_partial_failure_bridge`、`setup_u6_production_bridge`、`make_u3_wave`、`make_u6_completed`、`captured_env`、`make_test_cli_backend`、`make_test_hat_registry`、`redrive_test_registry`、`make_redrive_parent_with_descriptors`）的定义行号与引用计数；
  3. 2 个 const（`U5_RETRYABLE_REASON`、`U2_FRESH_PROCESS_BACKEND`）的字符串字面量。
  快照必须能在拆分后逐项复核，不能仅比较测试名称或数量。
- **单元顺序与中间门禁**：每个行为族模块搬完后先 `cargo build -p ralph-cli --tests` + 对应 targeted nextest；编译失败、公开签名变化、函数体 hash 变化、方法/测试项缺失、出现空 stub、`misc.rs` 超过 600 行时停止并恢复，不得继续下一族。
- **模块边界**：
  - `wave_supervisor.rs` 仅保留 9 个 `mod` 声明与必要的 `pub(super) use fixtures::*;` re-export；
  - `fixtures.rs` 仅承载全部共享 fixture 与 const，不得包含 `#[test]`；
  - 7 个行为族模块各自按 §2.2 表格承载对应测试，**不**混入未在表内的测试；
  - `misc.rs` 仅收纳未归类测试，超过 600 行必须按前缀继续拆。
- **行为冻结**：允许调整的内容仅为模块路径、`use super::fixtures::*`、精确 `mod` 声明、`pub(super)` 可见性、`pub use` re-export；禁止改变业务分支、trait impl、并发/超时常量、错误文本、事件 topic/payload、ledger 字段、failure classification、retry budget、salvage 断言。
- **结束证据**：拆分后复核快照，确认 108 个 `#[test]/#[tokio::test]` 函数名多重集与函数体 hash 不变；确认所有目标文件行数 `<5,000` 且根文件 `≤200`、`misc.rs ≤600`；确认 8 次 targeted 命中目标测试；最后按本计划命令清单完成全量门禁。

## 8. Unit 串行依赖图

本计划唯一 Unit 内部按 7 个行为族 + 1 个 misc + 1 个 fixtures 顺序搬迁。**fixtures 必须最先完成**（被所有行为族依赖）；行为族之间无相互依赖，但需保证 `mod` 声明顺序与编译顺序一致。后搬入模块可以引用先搬入模块的 fixture re-export，不可交错。

```
fixtures (1)
  ↓
slot_binding (2) → dispatch (3) → timeouts (4) → coordination (5) → salvage_merge (6) → supervisor (7) → redrive_payload (8) → misc (9)
                                                                                                          ↑
                                                                                            所有收尾后跑全量门禁
```

## 9. 执行命令清单

开始与结束：

- `cargo nextest list --workspace 2>&1 | tee /tmp/ws_baseline.txt`（前后 diff）

快环（每个行为族搬完后跑一次）：

- `cargo build -p ralph-cli --tests`
- `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor::slot_binding`（按族递增）
- `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`
- `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor::dispatch`
- `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor::timeouts`
- `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor::coordination`
- `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor::salvage_merge`
- `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor::supervisor`
- `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor::redrive_payload`
- `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor::misc`

静态：

- `just fmt-check`
- `just lint`

fixture 唯一性扫描：

- `rg -n "struct SpyBindingBridge" crates/ralph-cli/src/loop_runner/tests/`
- `rg -n "struct RecordingFactory" crates/ralph-cli/src/loop_runner/tests/`
- `rg -n "fn production_bridge_with_factory" crates/ralph-cli/src/loop_runner/tests/`

最终：

- `./scripts/run-tests.sh`（phase 1 + phase 2 + doctest）
- `cargo run -p ralph-e2e -- --mock`（mock E2E 最终一次）

禁止裸跑 `cargo test -p ralph-cli`；任一命令失败立即停止当前 Unit。

## 10. 最终质量门禁

- 108 个 `#[test]/#[tokio::test]` 函数名多重集与函数体 hash 不变；
- 共享 fixture（SpyBindingBridge / RecordingFactory / 各 setup_*/make_*）唯一持有，rg 扫描无重复；
- 9 个新文件行数 <5,000，根 ≤200，`misc.rs` ≤600；
- 8 次 targeted nextest 全部命中目标测试且全绿；
- `./scripts/run-tests.sh` phase 1 = 7576/7576、phase 2 = 23/23、doctest = 19/19（4 ignored）；
- `cargo build --workspace` 无 warning；
- `just fmt-check`、`just lint` 通过；
- mock E2E 通过；
- 不修改任何 `.cursor/rules/*.mdc`；
- 不修改 `tests/mod.rs` 之外的任何生产/测试文件；
- 与 plan 001 并行/独立可执行确认；
- 所有决策置信度 ≥0.85，无 BLOCKED；
- Unit 完成 Red → Green → Refactor → Integration → Regression → Close。

## 11. 最终计划自检

| 检查项 | 结果 |
|---|---|
| 实施计划而非 Roadmap | 是 |
| Executor 无关键设计决策 | 是，需先补齐测试函数体 hash 与 fixture manifest |
| 文件/接口/命令有证据 | 是，§2.1 行号锚点 + §7 manifest 要求 |
| 每 Unit 一个可观察行为 | 是：1 个 Unit = 1 个可观察的目录模块化结果 |
| 不依赖其他计划 | 是（D6），仅与 plan 002 在 mdc 所有权上有 002 独占约定，与本计划正交 |
| 无文件/语义所有权冲突 | 是：仅拥有 `tests/wave_supervisor.rs` 与新建 `tests/wave_supervisor/*.rs`；不动 wave/、loop_runner/、supervisor_bridge、`.cursor/rules/*` |
| Scenario 可追踪 | 是，§6 R1-R6 |
| 当前是否可执行 | 是；build/lint/fmt/全量基线均通过；实施分支仍必须重新跑 nextest 与全量基线 |
| 独立性置信度 | 0.95（基线门禁通过后） |