# Agent Handoff：统一编排状态清理（2026-06-22 20:46）

## 当前目标
把 `docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md` 完成后残留的 legacy 代码和重复测试清理掉：
- 所有 `UNIFIED_*` feature flag 锁死常开，删除 env / CLI 逃生门。
- 把 `apply_event_policy_validation`、`apply_step_handoff_gate`、`apply_workflow_guard_validation` 等 legacy gate 函数迁移进 `ValidationPipeline` 后删除。
- 把 `task.resume` 注入点替换为 `loop.resume` / deterministic correction。
- 删除只测 legacy 路径的测试和 BDD scenario。
- 更新 `ralph-tools*.md`、`runtime-diagnosis.md`、`AGENTS.md`/`CLAUDE.md`。

完成标准：`cargo check` 干净、`./scripts/run-tests.sh` 全绿、无 `task.resume` 运行时注入、无 `UNIFIED_*` 开关。

## 已完成内容
1. **特性开关锁死**
   - `UNIFIED_STATE_LEDGER`、`UNIFIED_VALIDATION`、`UNIFIED_DETERMINISTIC_CORRECTION`、`UNIFIED_PROTOCOL_VIEW`、`UNIFIED_POLICY_CHECK` 全部改为默认常开，删除 env 读取分支。
   - `ralph-cli/src/policy_check.rs` 删除 `PolicyCheckPath::Compat`，保留但未清理 `--policy-check-unified`/`--policy-check-compat` 两个废弃 flag 字段（当前未读）。
2. **ValidationRule trait 扩展为可变 context**
   - 新增 `crates/ralph-core/src/validation/context.rs`：`ValidationContext` 包装 `&mut LedgerSnapshot`。
   - 所有 rule 签名改为 `validate(&self, &ProtocolView, &mut ValidationContext, &Event)`。
3. **event policy 迁移进 pipeline**
   - 新增 `crates/ralph-core/src/validation/rules_event_policy.rs`。
   - `event_loop/mod.rs` 中删除 `apply_event_policy_validation` 批量调用，改走 unified pre-commit 循环。
4. **step handoff 迁移进 pipeline**
   - 删除 `apply_step_handoff_gate` 函数。
   - 在统一拒绝处理器中保留 `plan.blocked` 注入和 `RecoveryDiagnosisEnvelope` 持久化。
5. **workflow guard rule 实现（未接入）**
   - `crates/ralph-core/src/validation/rules_workflow_guard.rs` 已实现 strict-chain / out-of-order 检查，但 event loop **尚未执行 post-commit rules**。
6. **测试清理**
   - 删除：
     - `crates/ralph-core/src/event_loop/tests/u7_correction.rs`
     - `crates/ralph-core/src/event_loop/tests/u9_correction_assertions.rs`
     - `crates/ralph-core/src/event_loop/tests/topic_format_recovery.rs`
     - `crates/ralph-core/src/event_loop/tests/unified_short_circuits_legacy.rs`
     - `crates/ralph-core/src/event_loop/tests/task_resume_ttl.rs`（已删空）
   - `event_policy.rs` / `workflow_guard.rs` 中 legacy 断言已删。
7. **文档同步**
   - `crates/ralph-core/data/ralph-tools-emit.md`、`ralph-tools-wave.md`、`ralph-tools.md` 移除 `UNIFIED_*` 开关表。
   - `docs/guide/runtime-diagnosis.md` 顶部增加 `task.resume` 已废弃说明。

## 尝试过什么
- **方案 A（trait 可变 context）**：已采用。能自然表达 event policy 状态更新，但导致所有 rule 机械改签名。
- **纯删除 `apply_workflow_guard_validation`**：不可行。`WorkflowGuardRule` 原先是 stub，event loop 不跑 post-commit，直接删除会丢失 out-of-order 检查。
- **并发 subagent 清理**：因文件重叠和未完成的 workflow guard 依赖而中断，改为串行 subagent 推进。

## 关键决策和理由
- `ValidationRule` 用可变 context：event policy 需要更新 `PolicyRuntimeState`（同 batch `work.done` 去重等），纯函数无法实现。
- `plan.blocked` / recovery envelope 保留：这些 operator-facing side effect 由统一拒绝处理器触发，而不是 rule 直接写 `EventBus`。
- Workflow guard 先实现 rule 再接线：post-commit 阶段未在 event loop 运行，必须等接线后才能删除 legacy 函数。

## 修改过的文件及改动说明
| 文件 | 改动 |
|------|------|
| `crates/ralph-core/src/event_loop/mod.rs` | 删除 `apply_event_policy_validation`、`apply_step_handoff_gate` 调用；增加统一 pre-commit 循环；仍保留 `apply_workflow_guard_validation`、`publish_policy_rejection_resume`；仍有大量 `task.resume` 注入点。 |
| `crates/ralph-core/src/validation/context.rs`（新增） | `ValidationContext` 包装可变 `LedgerSnapshot`。 |
| `crates/ralph-core/src/validation/rules_event_policy.rs`（新增） | event policy 统一规则。 |
| `crates/ralph-core/src/validation/rules_workflow_guard.rs` | 实现 strict-chain 检查（未接入运行）。 |
| `crates/ralph-core/src/validation/pipeline.rs` | trait 签名改 mutable context；pipeline 已注册 `EventPolicyRule`。 |
| `crates/ralph-core/src/validation/result.rs` | 新增 `ValidationStage::EventPolicy`、`WorkflowGuardRejectionDetail`、相关 reason code。 |
| `crates/ralph-core/src/validation/rules_*.rs` | 机械改签名。 |
| `crates/ralph-core/src/correction/mod.rs` | `is_correction_enabled()` 默认 true。 |
| `crates/ralph-core/src/preset/engine/protocol.rs` | `UNIFIED_PROTOCOL_VIEW` 常开。 |
| `crates/ralph-cli/src/policy_check.rs` | 默认永远走 Unified 路径；Compat 路径删除。 |
| `crates/ralph-core/data/ralph-tools*.md` | 移除 feature flag 表。 |
| `docs/guide/runtime-diagnosis.md` | 增加 deprecated 说明。 |
| 多个测试文件 | 删除 legacy 测试，见上方列表。 |

未验证/仍有联动改动：`AGENTS.md`/`CLAUDE.md` 需要再次检查同步；`state_projector` deprecated cache 未删；`loop_state` legacy 字段未删。

## 障碍 / 待决问题
1. **Workflow guard 未真正运行**：`WorkflowGuardRule` 是 `PostCommit`，但 `process_parse_result` 只调用 `validate_pre_commit_with_view`。`apply_workflow_guard_validation` 仍是唯一生效路径。
2. **`publish_policy_rejection_resume` 仍存在**：被 `apply_workflow_guard_validation` 调用，未删除。
3. **`task.resume` 注入点大量残留**：`event_loop/mod.rs` 中 10+ 处，`ralph-cli/src/loop_runner/{hard_gate,runner,wave}` 也有。
4. **`state_projector` deprecated cache 未清理**：`tasks_cache`/`progress_cache` 仍被广泛使用，产生 30+ 条 deprecation warning。
5. **`loop_state` legacy 字段未清理**：`recent_rejection_digest`、`consecutive_same_signature` 等可能已死。
6. **`ralph-cli` policy_check 仍有废弃 flag 字段**：`UnifiedPolicyCheckFlags::policy_check_unified`/`policy_check_compat` 未读，但 struct 仍存在。
7. **大量 deprecation warning**：来自未清理的 `ProjectionContext` cache 字段。

## 下一步计划（按优先级）

### 1. 接线 post-commit pipeline（ blocker #1 ）
- 在 `event_loop/mod.rs` 的 per-event 统一循环中，对通过 pre-commit 的事件调用 `pipeline.validate_with_preview(ctx, event)`。
- 该函数内部会做 speculative commit + post-commit rules；返回 `ValidationReport`。
- 如果 post-commit 拒绝：rollback snapshot，调用 `publish_correction_via_context`，丢弃事件。
- 如果全部通过：保持 snapshot 变更（即视为已 commit），继续后续处理。
- 需要确认 `ValidationContext` 是否支持 snapshot 回滚；如不支持，在调用 `validate_with_preview` 前手动 clone snapshot，拒绝后恢复。

### 2. 删除 `apply_workflow_guard_validation` 和 `publish_policy_rejection_resume`
- 在 post-commit 接线后，`apply_workflow_guard_validation` 失效，删除该函数。
- 检查 `publish_policy_rejection_resume` 是否还有其它调用；无则删除及其 helper（`enrich_payload_with_wave_open_hint`、`task_resume_payload_has_required_fields` 等）。

### 3. 替换 `task.resume` 注入点
- `event_loop/mod.rs`：isolated-scope rejection、execution contract fallback、wave dispatcher、handoff dispatch timeout、persistent mode idle resume、`initialize_resume` 等。
- `ralph-cli/src/loop_runner/hard_gate.rs`、`runner.rs`、`wave/dispatcher.rs`。
- 策略：recoverable fallback 改发 `loop.resume`；agent-facing correction 改用 `publish_correction_via_context` / `CorrectionContext`。
- 保留旧 JSONL fixture 的最小兼容层（如 replay 时把 `task.resume` 当 `loop.resume` 别名）。

### 4. 清理 `state_projector` / `loop_state`
- `ProjectionContext` 删除 `tasks_cache`/`progress_cache`/`new_legacy`；读侧统一走 `LedgerSnapshot`。
- `LoopState` 删除 `recent_rejection_digest`、`consecutive_same_signature` 等 legacy 字段。
- 解决所有 deprecation warning。

### 5. 清理剩余 legacy 测试和 BDD
- 继续审查仍含 `task.resume` 的测试文件（见下方 gotchas）。
- 删除或迁移 `ralph-cli/tests/integration_resume.rs`、`ce_executor_recovery.rs` 中的 legacy 断言。
- 清理 `tests/scenarios/serial_lint/` 中仍只测 task.resume 的 YAML。

### 6. 文档收尾
- 同步 `AGENTS.md` 与 `CLAUDE.md`。
- 清理 `ralph-tools-handoff.md`、`ralph-tools-tasks.md`、`ralph-tools-memories.md` 中残留开关说明。
- 跑 `./scripts/run-tests.sh` 最终验证。

## 风险与注意事项
- `event_loop/mod.rs` 当前 11k+ 行，任何大段删除都要边删边跑测试，不要一次删太多。
- `ralph-cli` 测试必须走 `cargo nextest run -p ralph-cli --bin ralph`（串行），禁止裸跑 `cargo test -p ralph-cli`。
- Post-commit 接线会改变状态提交时机：之前是 pre-commit 后直接处理，现在 accepted 事件会先进入 speculative snapshot。注意 `StateLedger` 持久化时机不要遗漏。
- `task.resume` 在大量 smoke replay fixture 中可能仍作为旧 JSONL 出现；删除 topic 识别前要先更新 fixture 或 replay 驱动。
- 当前基线 `cargo check` 通过，但仍有 41 warnings，不要误以为失败。

## 重要注意事项（gotchas、约束、环境变量等）
- 测试入口：`cargo nextest run` 系列；`./scripts/run-tests.sh` 是 CI 推荐入口。
- `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 仅用于 flake 兜底。
- `AGENTS.md` 与 `CLAUDE.md` 必须保持内容一致；修改其中一个后建议 `cp CLAUDE.md AGENTS.md`。
- 当前工作区未提交改动约 `-4273 +1283` 行；提交前确认没有误删的源文件。
- 不要手动编辑 `.ralph/` 下的运行时状态文件。
- 中文输出规则：所有面向人类的文档/总结用中文；文件名、代码字符串、技术标识符保持原样。

## 总结 bullet points
- `UNIFIED_*` 开关已全部锁死常开。
- `ValidationRule` trait 已支持可变 `ValidationContext`。
- `event_policy`、`step_handoff` 已接入统一 pipeline 并删除 legacy gate 函数。
- `workflow_guard` rule 已实现，但 event loop 还没跑 post-commit，legacy 函数仍保留。
- `task.resume` 注入点和相关测试尚未清理完毕。
- `ProjectionContext` / `LoopState` 的 legacy 字段尚未删除，产生大量 deprecation warning。
- `cargo check -p ralph-core -p ralph-cli` 已通过。
- 当前 diff 约 3k 行删除，尚未提交。
- 下一位 Agent 应优先接线 post-commit pipeline，再删 `apply_workflow_guard_validation`。

## 下一位 Agent 的第一步建议
**接线 post-commit pipeline**：
1. 阅读 `crates/ralph-core/src/validation/pipeline.rs` 的 `validate_with_preview` 实现。
2. 在 `crates/ralph-core/src/event_loop/mod.rs` 找到 per-event unified pre-commit 循环（搜索 `validate_pre_commit_with_view`）。
3. 把 pre-commit 全部通过的事件改用 `validate_with_preview`，处理 rollback + finalize 逻辑。
4. 跑 `cargo nextest run -p ralph-core --no-fail-fast`；预期有少量测试需要更新拒绝阶段名称。
5. 通过后，删除 `apply_workflow_guard_validation` 和 `publish_policy_rejection_resume`。
