# ce-executor 现场塌缩归因修正（Errata）

> 📅 2026-06-04 | 🔖 关联报告：`docs/report/2026-06-04-ce-executor-worktree-prod-audit.md`（存在于 worktree 分支）
> 🔖 修正来源：`docs/plans/2026-06-04-001-fix-contract-rejection-hat-retry-plan.md`

---

## 0. 一句话修正

原 audit 报告把"coordinator → executor → ralph"塌缩的根因归到 **Ralph registry fallback shadowing downstream hats**。
修正后的根因是 **execution contract rejection 的恢复路径缺乏 targeted retry**：被拒绝的业务事件被丢弃，剩余的 `human.guidance` 被 `human.guidance` 隔离层从 active hat 选择中排除，下一轮由 Ralph coordinator 拿到恢复信号并越权补发业务事件。

`d7ef7cc test(hat): 为 ce-executor 预设固化 hat 路由契约测试` **只是 registry 层的回归固化测试**，不是该塌缩的修复。

---

## 1. 旧归因 vs 新归因

| 维度 | 旧归因（audit 报告 v0） | 修正后归因（v1） |
|------|------------------------|----------------|
| 出错层 | `HatRegistry::get_for_topic` 把 work.done 路由到 Ralph | `process_parse_result` 在 contract rejection 分支只发 `human.guidance`，没发 targeted retry |
| 触发条件 | Ralph 的 `*` 兜底订阅 | work.done 在 `TaskNotTerminal` 或 `MissingPayloadField` 等 contract 违规时 |
| d7ef7cc 的作用 | "修了 registry bug" | "把 registry 行为固化为测试，**未触及** rejection recovery 路径" |
| Ralph 越权补发 work.done 的原因 | Ralph 截胡 | Ralph 在 recovery 时是"唯一能 publish 的 hat"（因为 guidance 不参与 hat 选择） |

---

## 2. 修复对象

- `crates/ralph-core/src/event_loop/mod.rs`：在 `ExecutionContractDecision::Reject` 分支额外发布 `task.resume` 事件，`target=原发 hat`，payload 携带 `rejected_topic` / `reason` / `required_action` / `original_payload` / `retry_publish_topics` / `contract_finding`
- `crates/ralph-core/src/event_loop/mod.rs`：`determine_active_hat_ids` 与 `effective_regular_events` 跳过 `event.*` 系统事件，避免 diagnostic 激活 Ralph fallback
- `presets/en/ce-executor.yml` 与 `presets/zh/ce-executor-zh.yml`：补齐 `work.done` 字段一致性（5 个必需字段），把 `work.failed` 加入 `plan-gate.triggers` 避免孤儿化
- `crates/ralph-cli/src/loop_runner.rs`：rejection 时记录 `OrchestrationEvent::ContractRecoveryRouted`，区分"已 routed 到 source hat"与"no safe retry target"两种状态

---

## 3. 验证证据

新增/修改的测试覆盖：

- `crates/ralph-core/src/event_loop/tests.rs`：`test_contract_rejection_publishes_targeted_retry_to_source_hat`、`test_contract_rejection_activates_source_hat_for_next_prompt`、`test_contract_rejection_does_not_activate_reviewer`、`test_valid_work_done_directly_published_activates_reviewer`、`test_accepted_work_done_routes_to_reviewer`、`test_rejected_open_task_routes_retry_to_executor_not_reviewer`、`test_rejected_missing_plan_path_names_finding_and_routes_retry`、`test_retry_path_corrected_work_done_activates_reviewer`、`test_forged_ralph_work_done_does_not_create_retry_to_ralph`
- `crates/ralph-cli/src/presets.rs`：`test_ce_executor_work_done_field_consistency`、`test_ce_executor_zh_work_done_field_consistency`
- `crates/ralph-core/tests/hat_explicit_routing.rs`：`work_failed_routes_to_plan_gate_in_ce_executor_topology`（替换旧 `work_failed_is_orphan_in_ce_executor_topology`）
- `crates/ralph-cli/src/loop_runner.rs`：`test_compute_recovery_status_returns_target_when_targeted_retry_published`、`test_compute_recovery_status_returns_none_when_no_targeted_retry`

---

## 4. 教训

- 症状（"Ralph 截胡"）和机制（"rejection recovery 缺 targeted retry"）之间的桥需要源码级验证，不能仅凭 LLM 推断
- `d7ef7cc` 这类 "test" commit 不应被解读为 bug fix；commit message 类型前缀（`test:` / `fix:` / `feat:`）是首要信号
- 任何"hat 路径上有 N 步但实际只跑 M 步"的报告，都应先在源码中定位 skip 发生的事件层（registry / event_loop / prompt_build / agent_process），再下结论
