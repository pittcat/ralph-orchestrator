---
title: 统一编排状态重构 — 遗留 Legacy 清理与 Production 接线计划
type: cleanup
status: active
date: 2026-06-23
origin: docs/handoff/260622-2046-handoff.md
related:
  - docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md
  - docs/plans/2026-06-22-003-unified-orchestrator-state-plan-review-report.md
  - docs/report/2026-06-21-top-3-architectural-instability-factors.md
---

# 统一编排状态重构 — 遗留 Legacy 清理与 Production 接线计划

> 本文档是 `docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md` 的 follow-up plan，用于清理 U11 commit 链完成后仍残留的 5 类核心遗留问题，并完成计划对抗性审查（`docs/plans/2026-06-21-002-adversarial-review.md`）中标记的 P0/P1 修复项。
> 
> **目标**：让 `StateLedger` / `ValidationPipeline` / `CorrectionContext` 在 production event loop 中真正生效，彻底切断 `task.resume` 循环，统一状态源，消除三层 gate 不一致。

---

## 1. 执行摘要

U0–U11 的 commit 链完成了统一编排状态重构的**代码模块实现**（`state/`、`validation/`、`correction/`），但 production `event_loop/mod.rs` 中仍有 5 类核心遗留问题：

1. **Post-commit pipeline 未接线**：`WorkflowGuardRule` 和 `ExecutionContractRule` 作为 PostCommit 规则，在 event loop 中从未被调用；`validate_with_preview` 的 speculative commit + rollback 机制是 dead code。
2. **`task.resume` 在 completion rejection 路径仍存活**：3 个生产注入点（`LOOP_COMPLETE` missing events / verdict fail / workflow guard incomplete）仍在发 `task.resume` 而非 `loop.resume` 或 deterministic correction。
3. **`OriginRule` 使用空 `HatRegistry`**：`ValidationPipeline::from_config` 传入 `None` registry，导致 `origin:unknown_hat` 检查完全失效。
4. **`HatHandoffRule` 透传 macro-edge**：有 `handoff_path` 字段即 `accept`，不验证文件内容，属于"命名做 A 实际做 B"的欺骗性代码。
5. **`ProjectionContext` deprecated cache 未清理**：`tasks_cache`/`progress_cache` 产生大量 deprecation warning，读侧仍未统一从 `LedgerSnapshot` 派生。

本计划分 3 个阶段，按依赖顺序执行，每阶段结束后 `cargo check` 并通过。

---

## 2. 问题全景与优先级

### 2.1 P0 — 阻塞问题（必须修复，否则新架构在 production 中不可用）

| # | 问题 | 文件/行号 | 根因 | 修复后预期效果 |
|---|---|---|---|---|
| P0-1 | Post-commit pipeline 未接线 | `validation/pipeline.rs:validate_with_preview` 定义存在但 `event_loop/mod.rs` 零调用 | `WorkflowGuardRule` 和 `ExecutionContractRule` 只在 CLI 路径生效，event loop 不走 | `workflow_guard` 和 `execution_contract` 校验统一走 `ValidationPipeline`；`apply_workflow_guard_validation` 调用点删除 |
| P0-2 | `task.resume` 在 completion rejection 路径存活 | `event_loop/mod.rs:2007`、`2062`、`2096`（3 个 `self.bus.publish(Event::new("task.resume", ...))`） | 3 种 `LOOP_COMPLETE` 拒绝路径仍走 legacy `task.resume` 恢复 | 替换为 `emit_correction_context` 或 `loop.resume`；completion rejection 不再进入 EventBus 循环 |
| P0-3 | `apply_workflow_guard_validation` 仍被调用 | `event_loop/mod.rs:8377`（`apply_workflow_guard_validation` 调用） | 注释声称已删除，实际仍有调用 | 删除调用点，由 `WorkflowGuardRule`（PostCommit）替代 |
| P0-4 | `publish_policy_rejection_resume` 仍被调用 | `event_loop/mod.rs:865`（被 `apply_workflow_guard_validation` 调用） | workflow guard 的 rejection 仍走 `task.resume` | 删除调用点，由 post-commit 的 correction path 替代 |

### 2.2 P1 — 严重问题（不阻塞主线，但会导致 gate 不一致或安全漏洞）

| # | 问题 | 文件/行号 | 根因 | 修复后预期效果 |
|---|---|---|---|---|
| P1-1 | `OriginRule` 空 `HatRegistry` | `validation/pipeline.rs:157`（`from_registry(protocol_view, None)`） | `from_config` 不传入 registry，OriginRule 使用空注册表，所有事件被接受 | `from_config` 或 event loop 调用方传入 `HatRegistry::from_config(&ralph_config)`；未知 hat 事件被拒绝 |
| P1-2 | `HatHandoffRule` 透传 macro-edge | `validation/rules_hat_handoff.rs:91-97`（`handoff_path` 存在即 `accept`） | 不验证文件内容/结构，agent 写错文件也能通过 | 接入 `hat_handoff::validator::validate_artifact` 或 `hat_handoff::gate::evaluate_event`；验证失败返回 `HAT_HANDOFF_STRUCTURE_INVALID` 等 reason code |
| P1-3 | `ProjectionContext` cache 未清理 | `state_projector/mod.rs:158-169`（`tasks_cache`、`progress_cache` 已标记 `#[deprecated]`） | 仍被 legacy 路径和测试广泛使用，产生 30+ deprecation warning | 读侧统一从 `LedgerSnapshot` 派生；删除 deprecated 字段（或保留但让编译器 silence warning） |
| P1-4 | `persist_commit` 非原子写入 | `state/ledger.rs:485-491`（`write_all` + `sync_all().ok()`） | crash 后 commit log 尾部可能损坏，replay 失败 | 采用 `temp-file + rename` 原子写模式，或将 `sync_all` 错误向上传播 |
| P1-5 | `replay_from_disk` 多 loop iteration 错误恢复 | `state/ledger.rs:292-347`（`snapshot.iteration = iterations.max().unwrap_or(0)`） | 同一工作空间多次 loop 后 resume，iteration 取旧 loop 的最大值 | 在 loop 边界写入 `SnapshotReset` delta 或截断 ledger；replay 只读最后一次 reset 后的记录 |
| P1-6 | `CorrectionContext::render_block` prompt injection 风险 | `correction/mod.rs:193-233`（`last_message`/`topic` 直接拼接） | 拒绝信息含 `<!--` 或指令分隔符可导致 prompt injection | 对 `last_message` 和 `topic` 做 HTML 实体转义（`&lt;`、`&gt;`）或过滤 |

### 2.3 P2 — 优化项（可选，但影响性能和可维护性）

| # | 问题 | 文件/行号 | 根因 | 修复建议 |
|---|---|---|---|---|
| P2-1 | `StateLedger::commit` 全量 clone | `state/ledger.rs:245`（`self.snapshot.clone()`） | 每次 commit 深拷贝整个 `LedgerSnapshot`（含多个 HashMap/Vec） | 按受影响子结构选择性 clone，或引入 `im::HashMap` 持久化数据结构 |
| P2-2 | `apply_counter_change` 字符串匹配 | `state/snapshot.rs:569-597`（`match counter { "iteration" => ... }`） | 字符串拼写错误导致 silent no-op | 用 `CounterKind` enum 替代字符串（`CommitDelta::CounterChanged` 已用 enum，但 `apply_counter_change` 内部仍是 match 字符串） |
| P2-3 | 测试 env 隔离缺陷 | `preset/engine/protocol.rs` 等 | `std::env::set_var` 是进程级，并发测试互相污染 | 用 `serial_test` 或显式参数化 API 替代 env var 读取 |
| P2-4 | `event_loop/mod.rs` 膨胀至 10k+ 行 | `event_loop/mod.rs` | 文件过大，任何修改都容易冲突 | 将 `process_parse_result` 拆分为 `pre_validate`/`commit`/`post_validate`/`project` 子函数，或提取到 `event_loop/batch_processor.rs` |
| P2-5 | `CommitDelta::SnapshotReset` 未使用 | `state/commit.rs:184` | 标记为 "reserved for U3" 但 U3 已合入且未使用 | 删除或标记 `#[deprecated]` |

---

## 3. 阶段划分与执行顺序

```
Stage 1（可并行）
├── Agent_1: 接线 post-commit pipeline + 删除 legacy gate 调用
│   └── 依赖：P0-1, P0-3, P0-4
└── Agent_2: 修复 `OriginRule` + `HatHandoffRule`
    └── 依赖：P1-1, P1-2

Stage 2（依赖 Stage 1 完成）
└── Agent_3: 删除 `task.resume` 注入点 + 接入 correction
    └── 依赖：P0-2（需 Stage 1 的 post-commit pipeline 就绪）

Stage 3（可并行，依赖 Stage 2）
├── Agent_4: 清理 `ProjectionContext` cache + 统一读侧
│   └── 依赖：P1-3
└── Agent_5: 对抗性审查 P1/P2 修复 + 全量验证
    └── 依赖：P1-4, P1-5, P1-6, P2-1, P2-2
```

---

## 4. Stage 1 — 接线与 Rule 修复

### 4.1 Agent_1：接线 Post-Commit Pipeline + 删除 Legacy Gate 调用

#### 目标
让 `ValidationPipeline::validate_with_preview` 在 `process_parse_result` 的 per-event 循环中被调用，从而激活 `WorkflowGuardRule`（PostCommit）和 `ExecutionContractRule`（PostCommit），并删除 legacy `apply_workflow_guard_validation` 和 `publish_policy_rejection_resume` 的调用点。

#### 前提条件
- 已确认 `ValidationPipeline` 在 `event_loop/mod.rs` 中的构造位置（`build_unified_pipeline` 或 `process_parse_result` 内）
- 已确认 `validate_pre_commit_with_view` 的调用位置（U11-T2，约 line 8052）

#### 具体步骤

**Step 1.1：读取 `validate_with_preview` 完整实现**

文件：`crates/ralph-core/src/validation/pipeline.rs`

读取该方法的完整签名和实现，确认：
- 输入参数：`&self, ctx: &mut ValidationContext<'_>, event: &Event`
- 内部行为：是否已包含 `speculative_commit` + `validate_post_commit` + `rollback` 逻辑
- 返回值：是否包含 `ValidationReport` 或聚合后的 `ValidationResult`

**Step 1.2：在 `process_parse_result` 中接入 `validate_with_preview`**

文件：`crates/ralph-core/src/event_loop/mod.rs`

在 `validate_pre_commit_with_view` 全部通过的事件上，添加 post-commit 调用：

```rust
// 在 validate_pre_commit_with_view 通过之后、accepted_events.push 之前
for evt in &events {
    // 1. 保存 snapshot rollback 点
    let snapshot_backup = if let Some(ref mut ledger) = state_ledger {
        Some(ledger.snapshot().clone())
    } else { None };
    
    // 2. 构建 ValidationContext（复用 pre-commit 的 ctx）
    let mut ctx = ValidationContext::new(ledger_snapshot)
        .with_workflow_progress(&mut workflow_progress)
        .with_policy_rejections(&mut policy_rejections);
    
    // 3. 调用 validate_with_preview
    let post_results = pipeline.validate_with_preview(&mut ctx, evt);
    
    // 4. 如果有 post-commit rejection，rollback 并注入 correction
    if let Some(rej) = post_results.iter().find(|r| !r.accepted) {
        if let Some(ref backup) = snapshot_backup {
            // rollback: 恢复 snapshot
            *ledger_snapshot = backup.clone();
        }
        // 注入 correction context
        crate::correction::publish_correction_via_context(
            &mut self.state, Some(ledger), evt, rej,
        );
        // 标记该事件为拒绝，不进入后续处理
        rejected_topics.push(evt.topic.clone());
        continue;
    }
    
    // 5. 通过：snapshot 保持变更（已 speculative committed）
    accepted_events.push(evt.clone());
}
```

> **注意**：`validate_with_preview` 的具体签名可能不同（可能返回单个 `ValidationResult` 或 `ValidationReport`）。**必须先读取 `pipeline.rs` 的实际实现**，再确定调用方式。如果 `validate_with_preview` 的签名与上述不同，请调整。

**Step 1.3：删除 `apply_workflow_guard_validation` 调用**

文件：`crates/ralph-core/src/event_loop/mod.rs`

搜索 `apply_workflow_guard_validation` 的所有调用点（约 line 8377）：
- 删除该调用
- 保留函数定义（供测试使用），添加 `#[deprecated = "replaced by WorkflowGuardRule (PostCommit)"]`
- 删除相关局部变量（如 `workflow_guard_rejection`）

**Step 1.4：删除 `publish_policy_rejection_resume` 的调用点**

文件：`crates/ralph-core/src/event_loop/mod.rs`

搜索 `publish_policy_rejection_resume`：
- 删除 `event_loop/mod.rs:865` 的调用点（被 `apply_workflow_guard_validation` 调用）
- 保留函数定义，添加 `#[deprecated]`
- 如果 Step 1.3 已删除 `apply_workflow_guard_validation`，该调用点自然消失，但仍需检查是否有其他直接调用

**Step 1.5：验证 `workflow_guard_details` 的 drain 逻辑**

`ValidationContext` 有 `workflow_guard_details` 字段，用于累积 `WorkflowGuardRule` 的 rejection details。确认 event loop 在 post-commit 后正确 drain 该向量：
- 如果 `validate_with_preview` 内部已处理，则无需额外操作
- 如果未处理，在 post-commit 循环后添加 drain 逻辑，写入 recovery envelope

#### 验收标准
- [ ] `event_loop/mod.rs` 中出现 `validate_with_preview` 的调用
- [ ] `apply_workflow_guard_validation` 的调用点数为 0（保留定义供测试）
- [ ] `publish_policy_rejection_resume` 的调用点数为 0（保留定义供测试）
- [ ] `cargo check -p ralph-core` 通过
- [ ] `cargo nextest run -p ralph-core -- validation` 通过
- [ ] `cargo nextest run -p ralph-core -- event_loop` 通过

---

### 4.2 Agent_2：修复 `OriginRule` 和 `HatHandoffRule`

#### 4.2.1 修复 `OriginRule`：传入真实 `HatRegistry`

**目标**：让 `ValidationPipeline::from_config` 或 event loop 的调用方传入 `HatRegistry::from_config(&ralph_config)` 构建的真实注册表，使 `origin:unknown_hat` 检查生效。

**具体步骤**：

1. **读取 `HatRegistry::from_config` 的签名**：`crates/ralph-core/src/hat_registry.rs`
2. **确认 `RalphConfig` 在 event loop 中的访问路径**：
   - `EventLoop` 结构体通常持有 `RalphConfig` 或 `EventLoopConfig`
   - 搜索 `self.config` 的类型，确认是否为 `RalphConfig`
   - 如果不是，确认如何从 `EventLoop` 到达 `RalphConfig`（可能通过 `ralph` 字段或 `hat_registry` 字段）
3. **修改 `ValidationPipeline` 的构造**：
   - 在 `event_loop/mod.rs` 中搜索 `ValidationPipeline::from_config`
   - 替换为：
     ```rust
     let registry = Arc::new(HatRegistry::from_config(&self.config));
     let pipeline = ValidationPipeline::from_registry(&view, Some(registry));
     ```
   - 如果 `self.config` 是 `EventLoopConfig` 而非 `RalphConfig`，需要传入 `&self.ralph.config` 或类似路径
4. **验证**：确认 `OriginRule::with_registry` 被调用，且 `registry` 非空

#### 4.2.2 修复 `HatHandoffRule`：接入 Artifact 验证

**目标**：在 `HatHandoffRule` 中，对 `handoff_path` 存在的事件，执行文件内容验证，而不是直接 `accept`。

**具体步骤**：

1. **读取参考文件**：
   - `crates/ralph-core/src/hat_handoff/gate.rs`：找到 `evaluate_event` 或 `validate_artifact` 的实现
   - `crates/ralph-core/src/hat_handoff/validator.rs`：如果有独立 validator，读取接口
   - `crates/ralph-core/src/hat_handoff/mod.rs`：了解 `HandoffIndex` 的获取方式
2. **确定验证接口**：
   - 如果 `hat_handoff::gate` 有 `evaluate_event` 函数，检查其签名（是否接受 `FileContent`、`HandoffIndex`、`HatHandoffConfig` 等）
   - 如果签名太复杂（需要 `workspace: &Path`、`HandoffIndex` 等），考虑在 `event_loop` 中预读取文件内容并传入 `ValidationContext`，rule 只检查内容
3. **修改 `HatHandoffRule::validate`**：
   - 当前逻辑（保留）：
     * 非 macro-edge → `NotRequired`
     * macro-edge 且 `handoff_path` 缺失 → `reject`（`HAT_HANDOFF_MISSING_PATH`）
   - 新逻辑（修改）：
     * macro-edge 且 `handoff_path` 存在 → 读取文件内容
     * 调用 artifact 验证（如 `hat_handoff::validator::validate_artifact` 或自定义检查）
     * 验证失败 → `reject`（具体 reason code 如 `HAT_HANDOFF_STRUCTURE_INVALID`、`HAT_HANDOFF_MISSING_SECTION`）
     * 验证通过 → `accept`
4. **如果文件读取在 rule 中不合适**：
   - 在 `event_loop/mod.rs` 的 pre-commit 循环中，先读取 `handoff_path` 对应的文件内容
   - 将内容存入 `ValidationContext`（扩展 `context.rs` 添加 `handoff_content` 字段）
   - `HatHandoffRule` 从 `ctx` 读取内容验证，不做 IO

#### 验收标准
- [ ] `OriginRule` 使用非空 `HatRegistry`；`origin:unknown_hat` 测试通过
- [ ] `HatHandoffRule` 对 macro-edge 做文件内容验证；`HAT_HANDOFF_STRUCTURE_INVALID` 测试通过
- [ ] `cargo check -p ralph-core` 通过
- [ ] `cargo nextest run -p ralph-core -- origin` 通过（或相关测试）
- [ ] `cargo nextest run -p ralph-core -- hat_handoff` 通过

---

## 5. Stage 2 — 删除 `task.resume` 注入点

### 5.1 Agent_3：替换 Completion Rejection 的 `task.resume` 为 Deterministic Correction

#### 目标
删除 `event_loop/mod.rs` 中 3 个 `task.resume` 的生产注入点，替换为 `emit_correction_context` 或 `loop.resume`，彻底切断 completion rejection 的 `task.resume` 循环。

#### 具体步骤

**Step 2.1：定位 3 个注入点**

文件：`crates/ralph-core/src/event_loop/mod.rs`

搜索 `self.bus.publish(Event::new("task.resume"`：
- 约 line 2007：`LOOP_COMPLETE` missing required events
- 约 line 2062：verdict gate fail
- 约 line 2096：workflow guard incomplete

**Step 2.2：分析每个注入点的上下文**

每个注入点都在 `check_completion_event` 函数内，触发条件：
1. `LOOP_COMPLETE` 时仍有未完成的事件链
2. `LOOP_COMPLETE` 时 verdict gate 观察到 failing verdict
3. `LOOP_COMPLETE` 时 workflow guard chain 仍有 open instance

这些拒绝的共同特征：
- 事件是 `LOOP_COMPLETE`（控制 topic）
- 拒绝发生在 completion 阶段，不是普通业务事件
- 旧逻辑：发 `task.resume` 让 loop 继续，而不是终止

**Step 2.3：替换策略**

**方案 A（推荐）**：不发任何事件，直接注入 deterministic correction
```rust
// 删除：self.bus.publish(Event::new("task.resume", resume_payload));
// 替换为：
if let Some(ref mut ledger) = self.state.state_ledger {
    let _ = crate::correction::emit_correction_context(
        &mut self.state.prompt_context,
        ledger,
        "LOOP_COMPLETE",
        "contract:missing_required_events",
        &free_form,
    );
}
self.state.completion_requested = false; // 保持 completion 未处理，等 agent 修正
return None; // 不返回 TerminationReason，让 loop 继续
```

**方案 B**：如果 loop 必须继续，发 `loop.resume` 而非 `task.resume`
```rust
let topic = ralph_proto::LOOP_RESUME;
let payload = crate::correction::ResumeContext::default().to_payload();
self.bus.publish(Event::new(topic, payload));
```

> **注意**：先读取 `correction/mod.rs` 中 `emit_correction_context` 和 `publish_correction_via_context` 的签名，确认如何调用。`emit_correction_context` 需要 `&mut PromptContext` 和 `Option<&mut StateLedger>`，确认 `self.state` 中这些字段可访问。

**Step 2.4：更新 `correction/mod.rs` 的接口（如需要）**

如果 `emit_correction_context` 的签名不适合 completion rejection 场景，考虑：
- 新增一个 `emit_completion_correction` 辅助函数
- 或修改 `emit_correction_context` 接受更多参数

**Step 2.5：更新相关测试**

搜索引用这 3 个 `task.resume` 注入点的测试：
- `event_loop/tests/termination.rs`：检查 `task.resume` 出现次数的断言
- `event_loop/tests/state_machine.rs`：completion 拒绝注入 `task.resume` 的断言
- `event_loop/tests/text_fallback.rs`：completion 拒绝注入 `task.resume` 的断言
- 将断言改为 `correction_context` 或 `loop.resume`

#### 验收标准
- [ ] `event_loop/mod.rs` 中 `self.bus.publish(Event::new("task.resume"` 的调用点数为 0
- [ ] `cargo check -p ralph-core` 通过
- [ ] `cargo nextest run -p ralph-core -- termination` 通过（可能需要更新断言）
- [ ] `cargo nextest run -p ralph-core -- state_machine` 通过
- [ ] `cargo nextest run -p ralph-core -- text_fallback` 通过
- [ ] `cargo nextest run -p ralph-core -- correction` 通过

---

## 6. Stage 3 — 清理与验证

### 6.1 Agent_4：清理 `ProjectionContext` Deprecated Cache

#### 目标
删除 `ProjectionContext` 的 `tasks_cache` 和 `progress_cache` deprecated 字段，让 `state_projector` 的读侧完全从 `LedgerSnapshot` 派生，消除 deprecation warning。

#### 具体步骤

**Step 3.1：统计 `tasks_cache`/`progress_cache` 的引用点**

```bash
grep -rn "tasks_cache\|progress_cache" crates/ralph-core/src/state_projector/ crates/ralph-core/src/runtime_state.rs
```

确认所有引用点：
- 读引用（从 `ctx.tasks_cache` 读取）：需要替换为 `ledger_snapshot.tasks()` 或 `snapshot.tasks()`
- 写引用（向 `ctx.tasks_cache` 写入）：需要改为写 `StateLedger` 或 `LedgerSnapshot`

**Step 3.2：替换读引用**

文件：`crates/ralph-core/src/state_projector/mod.rs`、`progress.rs`、`task.rs`、`runtime_state.rs`

- 将 `ctx.tasks_cache` 替换为 `snapshot.tasks()`（或 `ledger_snapshot.tasks()`）
- 将 `ctx.progress_cache` 替换为 `snapshot.progress()`（或 `ledger_snapshot.progress()`）
- 如果函数签名中没有 `snapshot` 参数，需要添加

**Step 3.3：替换写引用**

- `persist` 函数：写 `tasks.jsonl` 的逻辑不变，但写完后更新 `LedgerSnapshot` 而非 `ctx.tasks_cache`
- `write_progress` 函数：写 `progress.md` 的逻辑不变，但写完后更新 `LedgerSnapshot` 而非 `ctx.progress_cache`

**Step 3.4：删除 deprecated 字段**

文件：`crates/ralph-core/src/state_projector/mod.rs`

```rust
// 删除以下字段：
#[deprecated = "Use LedgerSnapshot::tasks() instead"]
pub tasks_cache: Vec<crate::task::Task>,
#[deprecated = "Use LedgerSnapshot::progress() instead"]
pub progress_cache: ProgressSnapshot,
```

同时删除 `ProjectionContext::new()` 中对这些字段的初始化，以及所有 `#[allow(deprecated)]` 标记。

**Step 3.5：更新测试**

- `state_projector/tests.rs` 中引用 `tasks_cache`/`progress_cache` 的断言：替换为 `snapshot.tasks()` 或 `snapshot.progress()`
- `state_projector/u2_tests.rs` 中的 legacy 测试：如果已标记 deprecated，确认是否还需要

#### 验收标准
- [ ] `cargo check -p ralph-core` 0 warning（或 deprecation warning 数量显著减少）
- [ ] `cargo nextest run -p ralph-core -- state_projector` 通过
- [ ] `cargo nextest run -p ralph-core -- runtime_state` 通过

---

### 6.2 Agent_5：对抗性审查 P1/P2 修复 + 全量验证

#### 6.2.1 修复 `persist_commit` 非原子写入

文件：`crates/ralph-core/src/state/ledger.rs:485-491`

当前逻辑：
```rust
let mut f = OpenOptions::new().append(true).open(&path)?;
f.write_all(line.as_bytes())?;
f.sync_all().ok(); // 错误被静默丢弃
```

修复方案：
```rust
// 方案 A：将 sync_all 错误向上传播
f.sync_all().map_err(|e| LedgerError::Io(e.to_string()))?;

// 方案 B：temp-file + rename（更安全但更重）
let temp_path = path.with_extension("tmp");
let mut f = OpenOptions::new().write(true).create(true).truncate(false).open(&temp_path)?;
f.write_all(line.as_bytes())?;
f.sync_all().map_err(|e| LedgerError::Io(e.to_string()))?;
drop(f);
std::fs::rename(&temp_path, &path)?;
```

> **建议**：先采用方案 A（最小改动），因为方案 B 改变写入模式可能影响性能。如果 crash 安全是硬性需求，再用方案 B。

#### 6.2.2 修复 `replay_from_disk` 多 loop iteration 问题

文件：`crates/ralph-core/src/state/ledger.rs:292-347`

当前逻辑：
```rust
snapshot.iteration = iterations.iter().copied().max().unwrap_or(0);
```

修复方案：
1. 在 `CommitDelta` 中新增 `SnapshotReset` 的使用（或新增 `LoopStarted` delta）
2. 在 `event_loop/mod.rs` 的 loop 启动时写入该 delta
3. `replay_from_disk` 时只读取最后一次 `SnapshotReset` / `LoopStarted` 之后的记录

或者更简单：在 `replay_from_disk` 中，如果检测到 commit 的 `iteration` 不单调递增（出现下降），则认为是新 loop 开始，截断后续读取。

#### 6.2.3 修复 `CorrectionContext::render_block` prompt injection

文件：`crates/ralph-core/src/correction/mod.rs:193-233`

在 `render_block` 中，对 `last_message` 和 `topic` 进行转义：
```rust
fn escape_for_prompt(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace("<!--", "&lt;!--")
     .replace("-->", "--&gt;")
}
```

#### 6.2.4 修复 `apply_counter_change` 字符串匹配

文件：`crates/ralph-core/src/state/snapshot.rs:695-739`

当前 `CounterKind` 是 enum，但 `apply_counter_change` 内部仍用 `match counter_name` 字符串匹配。确认 `CounterKind` 是否已定义，如果已定义，修改 `apply_counter_change` 接受 `CounterKind` 而非 `&str`。

#### 6.2.5 全量验证

运行完整测试矩阵：
```bash
# 1. ralph-core 单元测试
cargo nextest run -p ralph-core --no-fail-fast

# 2. ralph-cli 集成测试（串行）
cargo nextest run -p ralph-cli --bin ralph --no-fail-fast

# 3. BDD scenarios
cargo nextest run -p ralph-core --test scenarios

# 4. Smoke replay
cargo nextest run -p ralph-core --features recording --test smoke_runner

# 5. Doctest
cargo test --workspace --exclude ralph-e2e --doc

# 6. 全 workspace
cargo nextest run --workspace --exclude ralph-e2e --no-fail-fast
```

#### 验收标准
- [ ] 全 workspace nextest 通过（5075+ 测试）
- [ ] BDD 63/63 通过
- [ ] Smoke 57/57 通过
- [ ] Doctest 18/18 通过
- [ ] 0 个 `task.resume` 生产注入点
- [ ] 0 个 `apply_workflow_guard_validation` 调用
- [ ] 0 个 `publish_policy_rejection_resume` 调用
- [ ] `tasks_cache`/`progress_cache` deprecation warning 归零

---

## 7. 风险与回滚方案

| 风险 | 可能性 | 影响 | 缓解措施 |
|---|---|---|---|
| Post-commit 接线后 workflow guard 行为与 legacy 不一致 | 高 | 高 | 保留 legacy 函数定义（仅删除调用），必要时用 `#[cfg(feature = "legacy_workflow_guard")]` 快速回退；新增集成测试 `event_loop/tests/u12_post_commit_workflow_guard.rs` 覆盖所有 legacy 拒绝路径 |
| `task.resume` 替换后某些测试断言失效 | 高 | 中 | 预先搜索所有引用 `task.resume` 的测试，在 Agent_3 执行前生成迁移清单；保留旧 JSONL fixture 的兼容性（replay 时把 `task.resume` 当 `loop.resume` 别名） |
| `ProjectionContext` cache 删除导致 `state_projector` 测试大面积失败 | 中 | 高 | 不一次性删除所有引用，先替换为 `LedgerSnapshot` 读路径，确认测试通过后再删除字段；使用 `#[deprecated]` 的 `since` 属性控制编译期警告 |
| `HatRegistry` 传入后导致 `OriginRule` 拒绝过多（误杀合法事件） | 中 | 高 | 先在 `tests/` 中跑 `origin_guard` 测试确认；如果误杀，检查 `HatRegistry::from_config` 是否遗漏了某些 hat 的注册（如 `ralph` 或 progress-steward） |
| `persist_commit` 原子写改动引入性能退化 | 低 | 中 | 先采用最小改动（`sync_all` 错误传播），不做 `temp-file + rename`；若必须做，benchmark 后决定 |
| 多 Agent 并行导致文件冲突 | 中 | 高 | 本计划中 Stage 1 的两个 Agent 修改不同文件（`event_loop/mod.rs` vs `validation/rules_*.rs`），无冲突；Stage 2 和 Stage 3 顺序执行，无冲突 |

---

## 8. 依赖关系图

```text
Stage 1
├── Agent_1: 接线 post-commit pipeline
│   ├── 读取 pipeline.rs validate_with_preview
│   ├── 修改 event_loop/mod.rs（接入 post-commit）
│   ├── 删除 apply_workflow_guard_validation 调用
│   ├── 删除 publish_policy_rejection_resume 调用
│   └── 验收: cargo check + nextest
└── Agent_2: 修复 OriginRule + HatHandoffRule
    ├── 读取 hat_registry.rs, hat_handoff/gate.rs
    ├── 修改 validation/pipeline.rs（from_registry 调用）
    ├── 修改 event_loop/mod.rs（传入 HatRegistry）
    ├── 修改 validation/rules_hat_handoff.rs（artifact 验证）
    └── 验收: cargo check + nextest

Stage 2（依赖 Stage 1）
└── Agent_3: 删除 task.resume 注入点
    ├── 定位 event_loop/mod.rs 3 个注入点
    ├── 替换为 emit_correction_context / loop.resume
    ├── 更新 correction/mod.rs 接口（如需要）
    ├── 更新测试断言（termination, state_machine, text_fallback）
    └── 验收: cargo check + nextest

Stage 3（依赖 Stage 2，可并行）
├── Agent_4: 清理 ProjectionContext cache
│   ├── 统计 tasks_cache/progress_cache 引用点
│   ├── 替换为 LedgerSnapshot 读路径
│   ├── 删除 deprecated 字段
│   └── 验收: cargo check + nextest
└── Agent_5: 对抗性审查修复 + 全量验证
    ├── 修复 persist_commit 原子写
    ├── 修复 replay_from_disk iteration
    ├── 修复 CorrectionContext render_block 转义
    ├── 修复 apply_counter_change 字符串匹配
    └── 全量验证: cargo nextest run --workspace
```

---

## 9. 文件修改清单（预估）

| 文件 | 修改类型 | 预估变更行数 | 负责 Agent |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/mod.rs` | 修改（接入 post-commit + 删除 legacy 调用 + 替换 task.resume） | +200 / -300 | Agent_1 + Agent_3 |
| `crates/ralph-core/src/validation/pipeline.rs` | 修改（确认 validate_with_preview 签名） | +10 / -5 | Agent_1（只读确认） |
| `crates/ralph-core/src/validation/rules_hat_handoff.rs` | 修改（artifact 内容验证） | +50 / -10 | Agent_2 |
| `crates/ralph-core/src/validation/rules_origin.rs` | 修改（无改动，但 pipeline.rs 调用方式变） | 0 | Agent_2（只读确认） |
| `crates/ralph-core/src/state/ledger.rs` | 修改（persist_commit 原子写 + replay iteration） | +30 / -10 | Agent_5 |
| `crates/ralph-core/src/state/snapshot.rs` | 修改（apply_counter_change enum） | +20 / -20 | Agent_5 |
| `crates/ralph-core/src/correction/mod.rs` | 修改（render_block 转义 + 可能新增接口） | +20 / -5 | Agent_3 + Agent_5 |
| `crates/ralph-core/src/state_projector/mod.rs` | 修改（删除 deprecated 字段） | -20 / +5 | Agent_4 |
| `crates/ralph-core/src/state_projector/progress.rs` | 修改（替换 progress_cache 读/写） | +30 / -30 | Agent_4 |
| `crates/ralph-core/src/state_projector/task.rs` | 修改（替换 tasks_cache 读/写） | +20 / -20 | Agent_4 |
| `crates/ralph-core/src/runtime_state.rs` | 修改（替换 cache 读路径） | +10 / -10 | Agent_4 |
| `crates/ralph-core/src/event_loop/tests/termination.rs` | 修改（断言 task.resume → correction） | +10 / -10 | Agent_3 |
| `crates/ralph-core/src/event_loop/tests/state_machine.rs` | 修改（断言 task.resume → correction） | +10 / -10 | Agent_3 |
| `crates/ralph-core/src/event_loop/tests/text_fallback.rs` | 修改（断言 task.resume → correction） | +10 / -10 | Agent_3 |
| `crates/ralph-core/src/state/tests.rs` | 修改（新增 persist crash 测试） | +40 / 0 | Agent_5 |

---

## 10. 参考文档

- 主计划：`docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md`
- 对抗性审查：`docs/plans/2026-06-21-002-adversarial-review.md`
- U11 handoff 文档：`docs/handoff/260622-2046-handoff.md`
- 架构不稳定因素报告：`docs/report/2026-06-21-top-3-architectural-instability-factors.md`
- U10 验证报告：`docs/plans/2026-06-21-002-unified-state-u10-verification.md`
- U11 review 报告：`docs/plans/2026-06-22-003-unified-orchestrator-state-plan-review-report.md`

---

*计划生成日期：2026-06-23*
*基于代码版本：pittcat-dev 分支 commit `80f36e2` 及之前*

