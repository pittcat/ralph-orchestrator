---
title: feat: Agent 操作防护
type: feat
status: active
date: 2026-05-31
origin: docs/brainstorms/2026-05-31-agent-operation-guard-requirements.md
depends: docs/plans/2026-05-31-003-fix-event-origin-guard-plan.md
---

# feat: Agent 操作防护

## 概述

在事件溯源防护基础稳定后，实现基于源验证的防护机制，保护 Agent 可调用的 Ralph 操作。本计划涵盖运行时任务、记忆、负载证据、人工介入完整性、Wave 分发、循环管理、emit 文件定位、事件历史清理、进度消息、技能加载、文档以及 CLI 补全适配。

本计划有意避免将 `RALPH_CURRENT_HAT` 视为可信凭证。它只是 Agent 提供的运行时声明。每个防护机制必须在允许状态变更操作之前，根据当前循环状态、注册表配置或所有者元数据验证该声明。

## 当前源码发现

| 领域 | 当前源码状态 | 计划响应 |
|---|---|---|
| 事件溯源防护 | 当前工作树已有 `event_origin.rs`、fail-closed 的 `can_publish()`、内部 `ralph emit` 时间戳以及 wave hat 标记。似乎还存在未解决的 `filter_wave_dispatch_by_origin` 导入/定义不匹配。 | 添加 P0 先决条件，先编译并稳定 003 再开始 004。不要将 R1-R12 重复实现为 004 的工作。 |
| 任务生命周期 | `Task` 有 `loop_id`，但没有所有者 hat；生命周期命令不检查 loop 或 hat。`ensure()` 以 key 为全局去重。 | 添加所有者元数据和共享授权辅助函数。 |
| 记忆 | Markdown 记忆条目没有所有者/可见性元数据；CLI 显示所有记忆。 | 扩展 markdown 元数据，同时保留传统共享记忆。 |
| 负载证据 | 现有解析器使用字符串包含检查来验证构建/审查证据。 | 强化现有解析器，而不是凭空发明不存在的函数。 |
| 人工响应 | Telegram 等待路径轮询 JSONL 中的 `human.response`。 | 用基于通道/令牌的源验证替换可信等待路径。 |
| Wave | 嵌套 wave 被阻止，wave 元数据已标记，但来源仅是环境派生的声明。 | 在摄取时验证声明的 hat 和发布范围，并添加格式错误的 wave 测试。 |
| 循环 | 破坏性和读取命令缺少 Agent 上下文授权。 | 添加循环所有者元数据，并区分 Agent 与人工 CLI 上下文。 |
| Emit 路径 | 最终路径可来自环境变量、标记或 `--file`，当前无循环限制。 | 保护最终解析路径，而不仅仅是 CLI 标志。 |
| 事件清理 | 当前 CLI 为 `ralph events --clear`；无确认提示。 | 添加 `--confirm <loop_id>` 和诊断信息。 |
| 进度消息 | 直接发送原始 Telegram 消息。 | 添加长度限制、频率限制、标记、诊断信息。 |
| 技能 CLI | 注册表可按 hats 过滤，但 CLI 传递 `None`。 | 首先复用现有的 `hats` 可见性过滤。 |

## 不可协商的回归约束

- 不要破坏无帽/单人模式。
- 不要破坏 ce-executor 链：`coordinator -> executor -> review-coordinator -> dimension-reviewer -> review-synthesizer -> fixer/shipper/reporter`。
- 不要破坏 wave-review。
- 不要使旧记忆不可见；无所有者的记忆保持共享。
- 不要仅因为 Agent 工作流受限就阻止人工操作员诊断工作流。
- 除非测试证明当前预设无法在没有更改的情况下工作，否则不要引入预设 YAML 更改。
- 不要重新引入 `ralph emit --ts`。

## 实施计划

### P0. 稳定事件溯源依赖

**目标：** 确保 003 在构建更高级的操作防护之前确实完成。

**文件：**
- `crates/ralph-core/src/event_origin.rs`
- `crates/ralph-core/src/event_loop/mod.rs`
- `crates/ralph-core/src/hat_registry.rs`
- `crates/ralph-cli/src/main.rs`
- `crates/ralph-cli/src/wave.rs`
- `scripts/ralph-zsh-plugin.zsh`
- `docs/guide/cli-reference.md`

**工作：**
1. 运行 `cargo check` 或针对性测试来暴露当前事件溯源的编译问题。
2. 添加或修正 `filter_wave_dispatch_by_origin()` 使 `event_loop` 能编译通过。
3. 修复 `validate_event_origin()` 的排序，使控制话题在注册 hat 的 `can_publish()` 之前被检查。
4. 确认 `can_publish()` 对未知 hat 的行为保持 fail-closed。
5. 确认 `EmitArgs` 没有 `ts` 字段，zsh 和文档没有宣传 `--ts`。
6. 确认 wave 分发验证使用分发 hat 的 `publishes`，而不是目标工作者的 `triggers`。

**测试：**
- `test_event_origin_registered_hat_control_topic_accepted`
- `test_event_origin_registered_hat_out_of_scope_business_rejected`
- `test_event_origin_unknown_hat_rejected`
- `test_event_origin_no_hat_business_rejected_in_hat_mode`
- `test_wave_dispatch_origin_registered_publisher_accepted`
- `test_wave_dispatch_origin_unknown_hat_rejected`
- `test_emit_args_has_no_ts_completion`

**验证：**
- `cargo test -p ralph-core event_origin`
- `cargo test -p ralph-cli wave`
- 如果补全有变化，zsh 补全在脚本更新后仍能加载。

### P1. 共享操作上下文和授权辅助函数

**目标：** 避免在 task/memory/loop/skill/emit 命令中出现分散不一致的授权检查。

**文件：**
- 新增：`crates/ralph-cli/src/operation_guard.rs` 或等价的文件
- `crates/ralph-cli/src/main.rs`
- `crates/ralph-cli/src/task_cli.rs`
- `crates/ralph-cli/src/memory.rs`
- `crates/ralph-cli/src/loops.rs`
- `crates/ralph-cli/src/skill_cli.rs`

**设计：**
```rust
struct OperationContext {
    workspace_root: PathBuf,
    current_loop_id: Option<String>,
    current_hat_id: Option<String>,
    is_agent_context: bool,
}
```

当运行时环境指示属于 Agent 拥有的执行时（例如设置了 `RALPH_CURRENT_HAT`、`RALPH_LOOP_ID`、`RALPH_EVENTS_FILE` 或 `RALPH_WAVE_WORKER`），`is_agent_context` 应为 true。没有这些环境变量的人工 CLI 应保留诊断访问权限，除非命令是破坏性的且缺少确认。

**工作：**
1. 添加读取 `.ralph/current-loop-id` 的辅助函数。
2. 添加读取 `RALPH_CURRENT_HAT` 的辅助函数。
3. 添加当前 accepted/candidate 事件标记的路径解析器。
4. 添加常见错误：`cross_loop_denied`、`cross_hat_denied`、`agent_context_missing_hat`、`path_outside_current_loop`。
5. 添加辅助函数，判断命令应 fail closed 还是要求确认。

**测试：**
- `test_operation_context_reads_current_loop_id`
- `test_operation_context_empty_loop_marker_is_none`
- `test_operation_context_agent_when_current_hat_set`
- `test_operation_context_human_when_no_runtime_env`
- `test_operation_context_wave_worker_is_agent`
- `test_operation_context_resolves_candidate_events_marker`
- `test_operation_context_resolves_accepted_events_marker`
- `test_operation_context_missing_markers_defaults_events_jsonl`

### P2. 任务操作防护

**目标：** 保护运行时任务生命周期操作免受跨循环和跨 hat 的篡改。

**文件：**
- `crates/ralph-core/src/task.rs`
- `crates/ralph-core/src/task_store.rs`
- `crates/ralph-cli/src/task_cli.rs`
- `crates/ralph-core/src/config.rs`（如果协调器授权需要配置）
- `crates/ralph-core/data/ralph-tools-tasks.md`

**数据模型：**
- 向 `Task` 添加 `owner_hat_id: Option<String>`。
- 通过 `#[serde(default)]` 保持与旧 JSONL 任务的 serde 兼容性。
- 任务创建时从 `OperationContext.current_hat_id` 标记 `owner_hat_id`。

**授权规则：**
- Agent 上下文：
  - 生命周期操作需要当前循环标记。
  - 要求 `task.loop_id == current_loop_id`。
  - 要求 `task.owner_hat_id == current_hat_id`，除非当前 hat 被配置为任务协调器。
- 人工 CLI 上下文：
  - 允许读取操作。
  - 生命周期操作暂时保持当前行为，但如果检测到不匹配，破坏性跨循环命令应打印显式警告。

**协调器授权：**
使用配置，而非命名约定。添加以下之一：
- `tasks.coordinator_hats: Vec<String>` 放在现有任务配置下（如果任务权限是全局的，首选此方式）。
- 或 `hats.<id>.task_permissions: coordinator`（如果权限是按 hat 设置的，首选此方式）。

在实施过程中检查现有任务配置形状后选择较小的 schema。如果添加了字段，更新 `docs/guide/configuration.md`。

**工作：**
1. 添加所有者字段和构造辅助函数。
2. 用上下文感知的标记替换 `add_common_task_fields()`。
3. 添加 `TaskStore::get_by_key_in_loop(key, loop_id)`。
4. 当 loop_id 存在时，`ensure()` 改为按 `(loop_id, key)` 去重。
5. 在 add/ensure 期间验证阻塞器。
6. 在变异之前使用授权包装 `start/close/fail/reopen`。
7. 保持 `ready` 循环过滤，并为 `--all` 添加测试。

**测试：**
- `test_task_add_stamps_loop_and_owner_hat`
- `test_task_add_without_hat_keeps_owner_none_for_human`
- `test_task_start_rejects_other_loop_task`
- `test_task_close_rejects_other_loop_task`
- `test_task_fail_rejects_other_loop_task`
- `test_task_reopen_rejects_other_loop_task`
- `test_task_start_rejects_other_hat_task`
- `test_task_close_allows_owner_hat`
- `test_task_operation_allows_configured_task_coordinator`
- `test_task_operation_rejects_missing_current_hat_in_agent_context`
- `test_task_ensure_key_scoped_by_loop`
- `test_task_ensure_same_key_same_loop_reuses`
- `test_task_ensure_same_key_different_loop_creates_new`
- `test_task_add_rejects_blocker_from_other_loop`
- `test_task_add_rejects_missing_blocker`
- `test_task_ready_defaults_to_current_loop`
- `test_task_ready_all_includes_other_loops`
- `test_legacy_task_without_loop_not_mutable_by_agent`

### P3. 记忆操作防护

**目标：** 支持共享的机构记忆，同时隔离私有的 hat 记忆，防止不相关 hat 的删除或投毒。

**文件：**
- `crates/ralph-core/src/memory.rs`
- `crates/ralph-core/src/memory_store.rs`
- `crates/ralph-core/src/memory_parser.rs`
- `crates/ralph-cli/src/memory.rs`
- `crates/ralph-core/data/ralph-tools-memories.md`
- `crates/ralph-core/data/ralph-tools.md`
- `docs/guide/configuration.md`

**数据模型：**
- 添加 `owner_hat_id: Option<String>`。
- 添加 `visibility: MemoryVisibility`，默认值为 `Shared`。
- 用 `owner_hat_id` 和 `visibility` 扩展 markdown 注释元数据。
- 缺少这些字段的旧条目解析为 `visibility=Shared`、`owner_hat_id=None`。

**CLI 变更：**
- `memory add`：添加 `--private` 标志。默认保持共享，除非配置更改。
- `memory list/show/search/prime`：在 Agent 上下文中，返回共享 + 当前 hat 私有的记忆。
- `memory delete`：在 Agent 上下文中，仅允许删除当前 hat 的私有记忆；共享记忆的删除需要人工上下文或显式的协调器权限。

**验证：**
- 拒绝空内容。
- 默认拒绝超过 10000 字符的内容。
- 按所有者 hat 计数私有记忆，拒绝超过 1000 条。
- 如果配置了共享上限，单独计数共享记忆。

**测试：**
- `test_memory_add_stamps_owner_hat`
- `test_memory_add_private_sets_visibility_private`
- `test_memory_add_default_visibility_shared`
- `test_memory_legacy_metadata_parses_as_shared`
- `test_memory_format_round_trips_owner_and_visibility`
- `test_memory_search_agent_sees_shared_and_own_private`
- `test_memory_search_agent_hides_other_hat_private`
- `test_memory_show_rejects_other_hat_private`
- `test_memory_prime_hides_other_hat_private`
- `test_memory_delete_rejects_other_hat_private`
- `test_memory_delete_rejects_shared_from_agent_context`
- `test_memory_delete_allows_human_cli_shared`
- `test_memory_add_rejects_empty`
- `test_memory_add_rejects_oversized`
- `test_memory_add_private_threshold_per_hat`
- `test_memory_list_json_includes_visibility`
- `test_memory_markdown_output_includes_owner_when_json_only_or_detail`

### P4. 负载证据防护

**目标：** 强化现有的构建/审查证据验证，同时不破坏当前的文本负载工作流。

**文件：**
- `crates/ralph-core/src/event_parser.rs`
- `crates/ralph-core/src/event_loop/mod.rs`
- `crates/ralph-core/src/event_policy.rs`（仅在验证后续移至策略时才需要）

**方法：**
1. 保留当前文本解析器路径，用于处理如 `tests: pass, lint: pass` 之类的负载。
2. 添加结构化 JSON 解析器，支持：
   - `checks.tests`、`checks.lint`、`checks.typecheck`、`checks.audit`、`checks.coverage`、`checks.duplication`
   - `complexity.score`
   - 可选的 `performance.regression`
   - 可选的 `specs.verified`
   - 可选的 `evidence_files: []`
3. 验证状态值：`pass`、`fail`、`skipped`、boolean。除非显式可选，否则将 `skipped` 视为必需检查的失败。
4. 验证数值范围：
   - complexity >= 0 且有限
   - coverage 0..=100（如果表示为百分比）
   - mutation 0..=100
5. 验证证据文件：
   - 非空相对路径
   - 规范化后在工作空间内
   - 处理时文件实际存在
6. 继续生成 `build.blocked` 和 `review.blocked`，除非所有预设和文档一起更新，否则不引入新话题。

**测试：**
- `test_parse_backpressure_text_valid_all_pass`
- `test_parse_backpressure_json_valid_all_pass`
- `test_parse_backpressure_json_missing_required_check`
- `test_parse_backpressure_json_invalid_status`
- `test_parse_backpressure_json_complexity_out_of_range`
- `test_parse_backpressure_json_coverage_out_of_range`
- `test_parse_backpressure_json_evidence_file_exists`
- `test_parse_backpressure_json_evidence_file_missing`
- `test_parse_backpressure_json_evidence_path_outside_workspace`
- `test_build_done_invalid_json_evidence_emits_build_blocked`
- `test_build_done_missing_evidence_emits_build_blocked`
- `test_parse_review_text_valid`
- `test_parse_review_json_valid`
- `test_parse_review_json_missing_dimensions`
- `test_parse_review_json_missing_conclusion`
- `test_review_done_non_wave_missing_evidence_emits_review_blocked`
- `test_review_done_wave_result_skips_build_review_evidence_gate`

### P5. 人工交互完整性

**目标：** 防止 Agent 编写的 JSONL 满足人工响应等待。

**文件：**
- `crates/ralph-telegram/src/service.rs`
- `crates/ralph-telegram/src/handler.rs`
- `crates/ralph-core/src/event_loop/mod.rs`
- `crates/ralph-core/src/event_origin.rs`
- `docs/guide/telegram.md`

**方法：**
1. 引入可信响应路径：
   - 首选：`TelegramService` 拥有一个内存通道，从轮询处理程序接收响应文本。
   - 备选：处理程序写入一个带有进程私有 nonce/源标记的 `human.response` 事件，该标记仅当前服务知道。`wait_for_response()` 只接受匹配的标记。
2. 如果 `human.guidance` 文件行为是有意主动且非阻塞的，则保留它。
3. 发送前验证 `human.interact` 负载：
   - 纯字符串：修剪后非空。
   - JSON 对象：`question` 字符串修剪后非空。
   - 拒绝空/非字符串。
4. 在 Telegram 激活模式下，忽略没有可信源的 JSONL `human.response`。
5. 如果退化的 JSONL 轮询仍用于测试，将其放在显式的测试/模拟配置之后，并记录警告。

**测试：**
- `test_human_interact_plain_question_valid`
- `test_human_interact_json_question_valid`
- `test_human_interact_empty_string_rejected`
- `test_human_interact_json_missing_question_rejected`
- `test_human_response_forged_jsonl_ignored_when_telegram_active`
- `test_human_response_from_trusted_channel_accepted`
- `test_human_response_wrong_nonce_rejected`
- `test_human_response_timeout_still_injects_timeout`
- `test_human_guidance_still_appends_from_telegram`
- `test_degraded_jsonl_polling_only_enabled_in_mock_mode`
- `test_degraded_jsonl_polling_logs_warning`

### P6. Wave 防护和 Emit 文件防护

**目标：** 验证 wave 分发来源，防止跨循环事件注入。

**文件：**
- `crates/ralph-cli/src/wave.rs`
- `crates/ralph-cli/src/main.rs`
- `crates/ralph-core/src/event_origin.rs`
- `crates/ralph-core/src/event_loop/mod.rs`
- `crates/ralph-cli/src/loop_runner.rs`
- `crates/ralph-core/data/ralph-tools.md`
- `docs/guide/cli-reference.md`

**Wave 工作：**
1. 保留嵌套 wave 工作者拒绝。
2. 在工作者执行前验证 wave 记录形状：
   - `wave_id` 存在
   - `wave_total > 0`
   - `wave_index < wave_total`
   - 同一 wave 中的所有事件在 topic 和 total 上一致
3. 使用事件溯源防护验证分发的 `hat`。
4. 不要声称 `RALPH_CURRENT_HAT` 是可信的。将其视为事件来源声明。

**Emit 路径工作：**
1. 在考虑 `RALPH_EVENTS_FILE`、candidate 标记、accepted 标记和 `--file` 后解析最终事件路径。
2. 规范化目标路径。
3. 构建允许集合：
   - `.ralph/current-candidate-events` 目标（如果存在）
   - `.ralph/current-events` 目标（如果存在）
   - 仅在不存在标记时的 `.ralph/events.jsonl`
4. 拒绝允许集合之外的任何最终路径。
5. 确保内部循环运行器仍一致地设置标记/环境变量。

**测试：**
- `test_wave_emit_rejects_nested_worker`
- `test_wave_record_rejects_missing_wave_total`
- `test_wave_record_rejects_index_equal_total`
- `test_wave_record_rejects_inconsistent_total`
- `test_wave_dispatch_unknown_hat_rejected`
- `test_wave_dispatch_out_of_scope_hat_rejected`
- `test_wave_dispatch_registered_publisher_accepted`
- `test_wave_emit_solo_mode_without_hat_still_allowed`
- `test_emit_default_uses_current_candidate_marker`
- `test_emit_default_uses_current_events_marker`
- `test_emit_no_marker_allows_default_events_jsonl`
- `test_emit_file_explicit_current_marker_allowed`
- `test_emit_file_other_loop_rejected`
- `test_emit_env_events_file_other_loop_rejected`
- `test_emit_path_traversal_rejected`
- `test_emit_symlink_to_other_loop_rejected`
- `test_emit_wave_worker_metadata_preserved`

### P7. 循环操作防护

**目标：** 防止 Agent 上下文操作不相关的循环，同时保留人工诊断能力。

**文件：**
- `crates/ralph-core/src/loop_registry.rs`
- `crates/ralph-cli/src/loops.rs`
- `docs/guide/cli-reference.md`

**数据模型：**
- 向 `LoopEntry` 添加可选的 `owner_hat_id`。
- 保留现有 `.ralph/loops.json` 的 serde 默认值。
- 在注册时从 `OperationContext.current_hat_id` 标记所有者。

**授权规则：**
- Agent 上下文：
  - `logs/history/diff`：仅当前循环，除非配置授予共享访问权限。
  - `stop`：仅当前循环。
  - `discard`：仅所有者。
  - `attach`：拒绝。
  - `merge`：仅当队列状态允许且调用者是所有者/协调器时才允许。
- 人工 CLI：
  - 允许读取命令。
  - 破坏性命令需要现有确认或新的显式确认。
  - 允许 attach，但记录诊断信息。

**测试：**
- `test_loop_entry_stamps_owner_hat`
- `test_loop_entry_legacy_owner_none_deserializes`
- `test_loop_logs_agent_rejects_other_loop`
- `test_loop_logs_human_allows_other_loop`
- `test_loop_history_agent_rejects_other_loop`
- `test_loop_diff_agent_rejects_other_loop`
- `test_loop_stop_agent_rejects_other_loop`
- `test_loop_stop_agent_allows_current_loop`
- `test_loop_discard_rejects_non_owner_agent`
- `test_loop_discard_allows_owner_agent`
- `test_loop_discard_human_requires_confirmation`
- `test_loop_attach_agent_rejected`
- `test_loop_attach_human_logs_diagnostics`
- `test_loop_merge_rejects_running_loop`
- `test_loop_merge_rejects_merged_loop`
- `test_loop_merge_rejects_discarded_loop`
- `test_loop_merge_allows_queued_owner`

### P8. 事件清理防护

**目标：** 防止意外或 Agent 触发的事件删除。

**文件：**
- `crates/ralph-cli/src/main.rs`
- `docs/guide/cli-reference.md`
- `scripts/ralph-zsh-plugin.zsh`（如果补全包含 `events` 标志）

**CLI 形状：**
当前命令是 `ralph events --clear`。添加：
```text
ralph events --clear --confirm <loop_id>
```

如果 `--confirm` 存在但为空或不匹配，拒绝。

**工作：**
1. 向 `EventsArgs` 添加 `confirm: Option<String>`。
2. 像现在一样解析目标事件路径。
3. 从 `.ralph/current-loop-id` 或目标工作树元数据解析预期的 loop id。
4. 拒绝没有匹配 confirm 的 clear。
5. 在 `history.clear()` 之前写入诊断/追踪信息。

**测试：**
- `test_events_clear_without_confirm_rejected`
- `test_events_clear_empty_confirm_rejected`
- `test_events_clear_wrong_loop_confirm_rejected`
- `test_events_clear_matching_loop_confirm_succeeds`
- `test_events_clear_no_loop_marker_requires_literal_confirm_current_or_default`
- `test_events_clear_logs_diagnostics_before_delete`
- `test_events_clear_with_file_still_requires_confirm`

### P9. 交互进度防护

**目标：** 使进度消息可识别且滥用防护，同时不假装 Ralph 可以验证真实性。

**文件：**
- `crates/ralph-cli/src/interact.rs`
- `crates/ralph-core/data/ralph-tools.md`
- `docs/guide/telegram.md`

**工作：**
1. 拒绝空或仅空白字符的消息。
2. 拒绝超过 2000 字符的消息。
3. 添加 `[via Ralph agent]` 前缀或后缀。
4. 实现基于进程和标记文件的频率限制。
5. 记录被拒绝和接受的进度尝试。

**测试：**
- `test_progress_empty_rejected`
- `test_progress_whitespace_rejected`
- `test_progress_oversized_rejected`
- `test_progress_appends_agent_marker`
- `test_progress_rate_limited_same_process`
- `test_progress_rate_limited_marker_file`
- `test_progress_after_interval_succeeds`
- `test_progress_missing_bot_token_error_unchanged`
- `test_progress_missing_chat_id_error_unchanged`

### P10. 技能访问防护

**目标：** 将现有的技能 hat 可见性应用于 CLI list/load。

**文件：**
- `crates/ralph-cli/src/skill_cli.rs`
- `crates/ralph-core/src/skill_registry.rs`
- `crates/ralph-core/src/skill.rs`（仅当前置元数据需要扩展时）
- `crates/ralph-core/data/ralph-tools.md`
- `docs/guide/configuration.md`

**工作：**
1. 在 Agent 上下文中，将 `Some(current_hat)` 传入 `skills_for_hat`。
2. 对于 load，如果请求的技能不在当前 hat 的可见集合中，则拒绝。
3. 对于错误建议，只打印可见的技能名称。
4. 在人工 CLI 上下文中，保持 `None` 行为以支持诊断。
5. 除非现有的 `hats` 白名单无法表达该策略，否则不添加 `restricted`。

**测试：**
- `test_skill_list_agent_filters_by_hat`
- `test_skill_list_human_shows_all`
- `test_skill_load_agent_allows_visible_skill`
- `test_skill_load_agent_rejects_hidden_skill`
- `test_skill_load_error_does_not_reveal_hidden_skill_name_in_available_list`
- `test_skill_load_agent_missing_hat_fails_closed`
- `test_skill_load_human_missing_hat_allows`
- `test_skill_backend_filter_still_applies`
- `test_skill_auto_inject_filter_unchanged`

### P11. 文档、CLI 参考和补全适配

**目标：** 使 Agent 面向的指令和人工文档与行为保持同步。

**需要更新的文件（视具体情况而定）：**
- `crates/ralph-core/data/ralph-tools.md`
- `crates/ralph-core/data/ralph-tools-tasks.md`
- `crates/ralph-core/data/ralph-tools-memories.md`
- `docs/guide/cli-reference.md`
- `docs/guide/configuration.md`
- `docs/guide/telegram.md`
- `docs/guide/agents.md`（如果示例提到旧的 emit 或 review 证据）
- `scripts/ralph-zsh-plugin.zsh`
- `~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`（脚本变更后）

**必需的文档更新：**
1. 运行时任务文档提到循环/hat 所有权失败。
2. 记忆文档提到共享 vs 私有以及删除限制。
3. 共享工具文档提到进度频率限制和技能 hat 可见性。
4. CLI 参考记录 `events --clear --confirm <loop_id>`。
5. CLI 参考记录 emit 路径限制。
6. Telegram 指南记录可信响应路径和退化测试模式。
7. 配置指南记录任何新的任务协调器或记忆默认值。
8. zsh 补全反映新标志，且继续不包含 `--ts`。

**测试/检查：**
- `rg -- '--ts' docs scripts crates/ralph-core/data`
- 验证复制后 zsh 补全脚本能加载。
- 如果 Markdown 有 Mermaid 更改，使用 Mermaid 工具验证。本计划不需要 Mermaid 更改。

## 集成测试矩阵

运行或添加以下流程的集成测试：

1. **ce-executor 模拟后端快乐路径：** wave 分发仍能到达维度审查者和聚合器。
2. **wave-review 快乐路径：** wave 工作者产生带有元数据的结果，且没有嵌套分发。
3. **任务受限循环：** loop-a 和 loop-b 共享 `.ralph/agent/tasks.jsonl`；loop-b 不能修改 loop-a 的任务。
4. **记忆可见性：** 执行者可以 prime 共享 + 自己的私有记忆，但不能 prime 审查者的私有记忆。
5. **人工响应伪造：** 在 Telegram 等待超时时，伪造的 JSONL 响应被忽略。
6. **emit 路径注入：** Agent 不能通过 `--file` 或环境变量写入另一个工作树的事件文件。
7. **人工 CLI 诊断：** 人工仍能检查另一个循环的日志/历史。
8. **技能过滤：** 未授权的 hat 不能加载隐藏的技能内容。
9. **事件清理安全：** 在提供正确的 confirm 值之前，clear 拒绝执行。
10. **负载证据：** 格式错误的 JSON 证据生成带有可操作原因的 blocked 事件。

## 验证命令

声明完成前的最低要求：

```bash
cargo test -- --test-threads=1
cargo test -p ralph-core smoke_runner
cargo run -p ralph-e2e -- --mock
```

开发期间的针对性测试：

```bash
cargo test -p ralph-core event_origin
cargo test -p ralph-core event_parser
cargo test -p ralph-core memory_store
cargo test -p ralph-core task_store
cargo test -p ralph-cli task_cli
cargo test -p ralph-cli wave
cargo test -p ralph-cli loops
cargo test -p ralph-telegram human_response
```

文档和补全检查：

```bash
rg -- '--ts' docs scripts crates/ralph-core/data
cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh
zsh -ic 'autoload -Uz compinit && compinit && source ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh'
```

## 发布顺序

1. 先做 P0。当前事件溯源代码可能无法编译时，不要开始更高级的防护。
2. 接下来做 P1。后面所有单元都使用共享上下文辅助函数。
3. P2 和 P3 在 P1 之后可以独立进行。
4. P4 和 P5 是风险更高的核心行为变更；编辑前添加特征化测试。
5. P6 和 P8 应一起完成，因为两者都涉及事件文件语义。
6. P7 在 P1 之后可以进行，且应保留人工 CLI 诊断能力。
7. P9 和 P10 较小，但在同一变更中更新工具文档。
8. P11 放在最后，在 CLI/配置形状确定之后。

## 风险分析

| 风险 | 可能性 | 影响 | 缓解措施 |
|---|---|---|---:|---:|
| 当前事件溯源脏状态编译失败 | 高 | 高 | P0 编译门禁，在 004 工作前完成 |
| 任务所有者防护阻止合法的协调器工作 | 中 | 高 | 基于配置的协调器授权和 ce-executor 集成测试 |
| 记忆隔离隐藏了有用的机构记忆 | 中 | 高 | 默认旧/共享可见性，仅过滤私有 |
| 人工响应通道重构破坏 Telegram | 中 | 高 | 特征化测试、模拟 Telegram 测试、仅用于测试的退化模式 |
| Emit 路径防护阻止循环运行器内部写入 | 中 | 高 | 根据当前标记集保护最终路径，而非原始环境变量存在性 |
| 循环授权阻止人工操作者 | 中 | 中 | 将 Agent 上下文与人工 CLI 上下文分开 |
| 负载验证对当前文本证据过于严格 | 中 | 中 | 保留文本解析器，添加 JSON 解析器作为更严格的路径 |
| 技能过滤在错误中泄露隐藏的技能名称 | 低 | 中 | 仅从可见集合构建建议 |
| zsh/文档与 CLI 产生偏差 | 中 | 低 | 必需的 P11 检查和脚本安装 |

## 实施过程中的待定决策

- 任务协调器 hat 的确切配置位置。
- 可信人工响应是仅使用通道方式，还是使用 nonce 标记的 JSONL 加通道方式。
- Agent 新记忆的默认值应保持全局共享，还是按运行可配置。
- Agent 上下文的循环读取访问是否需要立即可配置的共享循环白名单，还是 v1 版本仅限当前循环。

这些决策必须在实施过程中通过源码检查来确定，然后反映在 `docs/guide/configuration.md` 中。