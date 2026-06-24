---
title: Ralph 编排状态统一化重构
type: refactor
status: active
date: 2026-06-21
origin: docs/brainstorms/2026-06-21-unified-orchestrator-state-requirements.md
deepened: 2026-06-21
---

# Ralph 编排状态统一化重构

## Overview

把 Ralph orchestrator 从「多层 gate 各自维护状态 + `task.resume` 自指恢复」模型，重构为「单一 `StateLedger` + 单一 `ProtocolView` + deterministic correction」模型。

本次重构是 `docs/brainstorms/2026-06-21-unified-orchestrator-state-requirements.md` 的落地计划，吸收并替代 `docs/brainstorms/2026-06-21-serial-preset-root-cause-fix-requirements.md` 的战术修复包。

**重要调整（基于对抗性审查）**：原计划采用一次性 big-bang。审查发现 `task.resume` 调用面过大、`process_parse_result` 与状态写入深度耦合、测试迁移量被严重低估。因此本计划保留 big-bang 的**架构终态目标**，但引入**特性开关 + 绞杀者模式（strangler fig）**：新旧路径可在编译期/运行期切换，每一阶段都必须保持 `./scripts/run-tests.sh` 绿色（允许少量带 follow-up issue 的 `#[ignore]`）。

---

## Problem Frame

当前 `crates/ralph-core/src/event_loop/mod.rs` 的 `process_parse_result` 串联 10+ 层验证（origin guard、event policy、state machine、hat-handoff gate、state projection、step handoff gate、workflow guard、execution contract 等）。每层 gate 既做验证又写自己的内存状态，导致：

1. **状态源分散**：`WorkflowProgress`、`ReviewStepTracker`、`HandoffTracker`、`PolicyRuntimeState`、`tasks_cache`、`progress_cache` 等独立维护，没有统一提交点。
2. **恢复循环**：recoverable rejection 触发 `task.resume`，`task.resume` 重新进入 `.ralph/events.jsonl` 和完整验证链，agent 再次 emit 同类错误时可能换个 stage 重新开始计数。
3. **协议视图分裂**：CLI `--policy-check`、engine gate、runtime gate 对 macro-edge / required fields 的判断逻辑分叉（见 `crates/ralph-core/src/preset/engine/` vs `crates/ralph-core/src/hat_handoff/` vs `crates/ralph-cli/src/policy_check.rs`）。

重构后，所有状态变更走 `StateLedger::commit()`，所有验证走统一的 `validate_event(protocol_view, ledger_snapshot, event)`，recoverable rejection 直接变成 prompt 中的 deterministic correction，不再进入 EventBus 循环。

---

## Requirements Trace

- R1. 引入单一 `StateLedger` 结构，替代分散内存 tracker。
- R2. 所有状态变更在 `StateLedger::commit()` 中原子顺序提交。
- R3. `StateProjector::apply()` 从 ledger 派生，禁止并行状态。
- R4. 启动/恢复时从磁盘重建 `LedgerSnapshot`。
- R5. 定义单一 `ProtocolView`，lint/engine/runtime 共用。
- R6. `ProtocolView` 统一回答 topic 权限、macro-edge、handoff artifact、required fields。
- R7. 禁止 gate 绕过 `ProtocolView` / `LedgerSnapshot` 读私有状态或磁盘。
- R8. CLI lint 与 loop 验证使用同一 `validate_event` 流水线。
- R9. 删除 `publish_policy_rejection_resume` 及相关 `task.resume` 路径。
- R10. recoverable rejection 直接写入 prompt deterministic correction。
- R11. 同一 hat+reason_code 短窗口内 ≥ 3 次升级 `human.guidance` / `loop.suspend`。
- R12. `ralph diagnose` 从持久化 rejection log 输出结构化根因。
- R13. macro-edge handoff artifact 由 runtime 自动写出。
- R14. prompt `## ORCHESTRATOR CONTEXT` 读取 `LedgerSnapshot`。
- R15. `progress-steward` instructions 改为读 `## ORCHESTRATOR CONTEXT`。

**Origin actors:** A1 Operator, A2 Workflow hat, A3 Orchestrator runtime, A4 Preset maintainer
**Origin flows:** F1 事件从 emit 到提交, F2 hat 切换与 handoff artifact, F3 work.done 后状态不漂移, F4 Recovery 收敛为可观测信号
**Origin acceptance examples:** AE1 状态统一, AE2 gate 一致, AE3 无 task.resume 循环, AE4 deterministic correction, AE5 handoff 自动生成

---

## Scope Boundaries

- **In scope:** `crates/ralph-core/src/event_loop/`, `crates/ralph-core/src/state_projector/`, `crates/ralph-core/src/state/`, `crates/ralph-core/src/preset/engine/`, `crates/ralph-core/src/hat_handoff/`, `crates/ralph-core/src/step_handoff/`, `crates/ralph-core/src/diagnosis/`, `crates/ralph-core/src/event_origin.rs`, `crates/ralph-core/src/event_policy.rs`, `crates/ralph-core/src/execution_contract.rs`, `crates/ralph-cli/src/policy_check.rs`, `crates/ralph-cli/src/commands/emit.rs`, `crates/ralph-cli/src/loop_runner/`, 相关 BDD scenarios 与测试。
- **Out of scope:** wave supervisor 协议升级（新增语义）、`loop.cancel`/`loop.terminate` 语义统一、外部数据库存储。
- **Not out of scope（必须保持绿色）：** `ce-executor-isolated` 与 `ce-executor-wave` 作为共享 `process_parse_result` 的 preset，必须在 U9 验证中跑绿；本次不重构它们的 preset 拓扑，但要确保核心路径改动不破坏它们。

### Deferred to Follow-Up Work

- `ralph-tools*.md` 文档批量同步：在 U9 绿后由单独 PR 处理（按 AGENTS.md 反向验证规则）。
- TUI/RPC 对 ledger rejection log 的实时展示：保持现有 EventBus observer 模式。
- `ce-executor-isolated` / `ce-executor-wave` 利用新架构做深度优化：先保证兼容，再后续迭代。

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/state_projector/mod.rs` — `StateProjector::apply()` 是当前集中写盘入口，已有 `ProjectionContext` 缓存。
- `crates/ralph-core/src/event_loop/mod.rs` — `process_parse_result` 是验证链主函数，串联所有 gate。
- `crates/ralph-core/src/event_loop/loop_state.rs` — `LoopState` 承载 `WorkflowProgress`、`ReviewStepTracker`、`HandoffTracker`、`PolicyRuntimeState` 等分散状态，以及 `rejection_retry_counts`、`recent_rejection_digest`、`consecutive_same_signature` 等辅助状态。
- `crates/ralph-core/src/event_policy.rs` — `PolicyRuntimeState::from_events()` 从 `.ralph/events.jsonl` 重建 dedup set、terminal observed、completion 状态等。
- `crates/ralph-core/src/preset/engine/protocol.rs` — `ProtocolView` 已提供索引化视图，但 engine gate 只覆盖 required fields + 浅层 handoff 检查。
- `crates/ralph-core/src/hat_handoff/gate.rs` — runtime handoff gate 做完整文件/结构/R15 校验，但与 CLI/engine 不共享。
- `crates/ralph-core/src/diagnosis/responder.rs` — `RecoveryResponder` 负责 U6/U7/U8 恢复响应，是 `task.resume` 主要触发点之一。
- `crates/ralph-core/src/step_handoff/progress_task_gate.rs` — 直接读 `tasks.jsonl` / `progress.md` 做校验。
- `crates/ralph-cli/src/policy_check.rs` — CLI `--policy-check` 走 legacy 路径，与 engine linter 不一致。

### Institutional Learnings

- `docs/report/2026-06-21-top-3-architectural-instability-factors.md` 指出：补丁式加固 gate 无法根治循环，必须拆掉状态源分散和恢复循环。
- `docs/brainstorms/2026-06-21-serial-preset-root-cause-fix-requirements.md` 已识别 lint/runtime 三层视图不一致、ralph 伪 hat 越权、`task.resume` 无法唤醒 reviewer/fixer/steward 等具体问题。

### External References

- 无外部依赖；本次为内部 Rust 架构重构。

---

## Key Technical Decisions

- **KTD-1. StateLedger = mutable snapshot + persisted commit log。** `.ralph/ledger.jsonl` 是第一级持久事实源；`tasks.jsonl` / `progress.md` 降级为只读派生视图。启动时通过 replay ledger 重建 snapshot，而不是读投影文件。理由：当前 `tasks.jsonl` / `progress.md` 是破坏性重写，且无法恢复 dedup set、rejection history、handoff seq 等内存状态。
- **KTD-2. ProtocolView 是只读配置视图，不含动态状态。** `phase_allowed_topics` 等动态规则从 `LedgerSnapshot` 实时读取；`ProtocolView` 只封装 preset 拓扑、schema、macro-edge 判定、required fields、静态 publisher 权限。每 batch 重新生成一次（成本低），batch 内事件顺序应用。
- **KTD-3. 校验拆成两段：pre-commit + post-commit preview。** 依赖投影后状态的规则（execution contract、step handoff）对 speculative commit 后的 snapshot 做校验，失败则 rollback。这样 `work.done` 等事件先关闭 task 再被 execution contract 校验的语义得以保留。
- **KTD-4. 保留验证阶段名用于诊断，但阶段不再持有私有状态。** `origin`、`policy`、`hat_handoff`、`step_handoff`、`workflow_guard`、`execution_contract` 等作为 `ValidationRule` 名称存在；它们都是纯函数，输入为 `ProtocolView + LedgerSnapshot + Event`。
- **KTD-5. `human.guidance` 仍作为每次拒绝的可见性事件，但只在达到阈值时触发 escalation。** 这兼容现有 `human.guidance` 测试，同时满足 R11 的升级语义。
- **KTD-6. Handoff artifact 由 runtime 在 macro-edge accept 后自动生成。** 校验阶段 `handoff_path` 可选；accept 后 `StateLedger.commit` 调用 allocator 生成唯一文件，写回事件元数据再 publish。
- **KTD-7. CLI `--policy-check` 必须等统一 `validate_event` 覆盖全部规则后再迁移。** 在此之前保留 legacy 路径作为 fallback，但通过一致性测试逐步收紧。
- **KTD-8. 新旧路径通过特性开关共存。** `UNIFIED_STATE_LEDGER=1`（环境变量或编译 cfg）启用新模型；默认关闭直到 U9 全部绿色。这允许绞杀者式迁移，避免长期 broken 主线。

---

## Open Questions

### Resolved During Planning

- **StateLedger 形式：** persisted commit log + mutable snapshot（KTD-1）。
- **ProtocolView 刷新频率：** 每 batch 重新生成，动态状态从 snapshot 读取（KTD-2）。
- **execution contract 顺序冲突：** pre-commit + post-commit preview（KTD-3）。
- **`process_parse_result` 是否保留内部阶段：** 保留阶段名作为诊断标签，阶段无状态（KTD-4）。
- **`continue` 模式如何替代 `task.resume`：** 新增 `loop.resume` 控制事件，在首个 hat prompt 中注入 resume context（U6b）。
- **`pending_lint_resume` 命运：** 并入 `CorrectionContext`，`## LINT RESUME REQUIRED` 区块由统一 correction injection 生成（U6a）。
- **rejection budget 计数键：** 保持 `hat+topic+reason_class` 粒度，与现有 `RecoveryResponder` 的 `retry_key` 兼容；持久化到 `recovery.jsonl`（U6a）。

### Deferred to Implementation

- `ledger.jsonl` 具体 schema 与 compaction 策略：由 U1 设计文档确定。
- `correction_context` prompt 模板精确格式：由 U6a 设计文档确定，并在实现前锁定。
- `ProtocolView` 与 `HandoffIndex` 缓存策略：U3 实现后 benchmark，若退化 >5% 则加缓存。

---

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

### 事件处理新流程

```text
Agent emits JSONL event
  │
  ▼
EventReader parses → Event
  │
  ▼
process_batch(events)
  │
  ├─ Build LedgerSnapshot from StateLedger (replay ledger.jsonl if needed)
  ├─ Build ProtocolView from EventLoopConfig + HandoffIndex (config-only)
  │
  ▼
for each event in batch:
  ├─ pre_commit_validate(protocol_view, ledger_snapshot, event)
  │     └─ origin, publisher, required fields, topic format, macro-edge
  ├─ if pre-commit rejected:
  │     └─ record_rejection → correction_context → human.guidance (visibility)
  │     └─ if count >= threshold: emit human.guidance escalation / loop.suspend
  ├─ if pre-commit accepted:
  │     └─ speculative_commit(event) → LedgerSnapshot'
  │     └─ post_commit_validate(protocol_view, LedgerSnapshot', event)
  │           └─ execution contract, step handoff, workflow guard
  │     └─ if post-commit rejected:
  │           └─ rollback to LedgerSnapshot
  │           └─ record_rejection → correction_context
  │     └─ if post-commit accepted:
  │           └─ finalize commit
  │           └─ StateProjector.apply(commit) writes tasks.jsonl + progress.md
  │           └─ if macro-edge: auto_handoff_prepare → write handoff_path back
  │           └─ EventBus.publish(event) for downstream hat
  │
  ▼
Next turn: build prompt from LedgerSnapshot + correction_context
```

### StateLedger 结构

```text
StateLedger
  ├─ snapshot: LedgerSnapshot
  │     ├─ tasks: Vec<Task>
  │     ├─ progress: ProgressSnapshot
  │     ├─ workflow_phases: Map<instance_id, WorkflowPhase>
  │     ├─ review_steps: ReviewStepState
  │     ├─ handoff_deadlines: HandoffDeadlineState
  │     ├─ policy_runtime: PolicyRuntimeState      (dedup, terminal, completion)
  │     ├─ flow_lifecycle: FlowLifecycleState
  │     ├─ rejection_counts: Map<retry_key, u32>
  │     └─ ... (完整 LoopState 字段盘点后映射)
  ├─ commit_log: Vec<Commit>  → persisted to .ralph/ledger.jsonl
  └─ rejection_log: Vec<RejectionRecord>  → persisted to .ralph/recovery.jsonl
```

### ProtocolView 职责（只读配置）

```text
ProtocolView
  ├─ publishers: Map<hat, Set<topic>>
  ├─ macro_edges: Set<topic>        (with self-loop exclusion)
  ├─ required_fields: Map<topic, Set<field>>
  ├─ handoff_artifact_required: Map<topic, ArtifactSpec>
  └─ static_rules: ...
```

---

## Implementation Units

### Phase 0: 基线与盘点

- [ ] U0. **Characterization and inventory**

**Goal:** 在改动任何行为前，记录当前基线并完整盘点将被替换的状态/概念。

**Requirements:** R1, R4, R9

**Dependencies:** None

**Files:**
- Create: `docs/plans/2026-06-21-002-unified-state-inventory.md`（盘点文档，非持久计划）
- Read-only: `crates/ralph-core/src/event_loop/loop_state.rs`, `crates/ralph-core/src/event_loop/mod.rs`, `crates/ralph-core/src/diagnosis/responder.rs`, `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`, `crates/ralph-cli/src/loop_runner/hard_gate.rs`
- Test: `crates/ralph-core/tests/fixtures/unified-state-baseline/`（新建基线目录）

**Approach:**
- 运行 `ce-executor-serial`、`ce-executor-isolated`、`ce-executor-wave` 的 BDD/scenarios，捕获 `recovery.jsonl`、`tasks.jsonl`、`progress.md`、`events.jsonl`、prompt snapshot 作为 golden fixtures。
- 用 grep 全量盘点 `task.resume`、`publish_policy_rejection_resume`、`pending_lint_resume`、`recent_rejection_digest`、`WorkflowProgress`、`ReviewStepTracker`、`PolicyRuntimeState`、`handoff_tracker`、`contract_rejections` 的所有生产/测试引用。
- 输出 `LoopState` 字段到 `LedgerSnapshot` 子结构的映射表：每个字段是进入 ledger、作为外部只读参数、还是删除。

**Patterns to follow:**
- 现有 smoke replay fixture 的录制方式。
- 现有 BDD scenario runner 的 `assert_state` 机制。

**Test scenarios:**
- Happy path: 基线 fixtures 成功录制并可通过现有 scenario runner 回放。
- Integration: 盘点文档列出所有 `task.resume` 生产调用点及替代策略。

**Verification:**
- 盘点文档通过内部 review。
- 基线 fixtures 可成功回放一次。

---

### Phase 1: StateLedger 基础

- [ ] U1. **Design and implement StateLedger with persisted commit log**

**Goal:** 建立单一状态账本，把 `tasks.jsonl` / `progress.md` 降为派生视图，commit log 作为第一级持久源。

**Requirements:** R1, R2, R4

**Dependencies:** U0

**Files:**
- Create: `crates/ralph-core/src/state/ledger.rs`
- Create: `crates/ralph-core/src/state/snapshot.rs`
- Create: `crates/ralph-core/src/state/commit.rs`
- Modify: `crates/ralph-core/src/event_loop/loop_state.rs`
- Test: `crates/ralph-core/src/state/tests.rs`

**Approach:**
- 定义 `LedgerSnapshot` 统一承载 task、progress、workflow phase、review step、handoff deadline、`policy_runtime`、`flow_lifecycle`、rejection counts 等字段（基于 U0 映射表）。
- 定义 `Commit` / `CommitDelta` 与 `.ralph/ledger.jsonl` 持久化格式。
- 实现 `StateLedger::commit(event) -> Commit`（含 in-memory rollback）、`StateLedger::snapshot()`、`StateLedger::replay_from_disk()`。
- 启动/恢复时 replay `.ralph/ledger.jsonl` 重建 snapshot，而不是读 `tasks.jsonl` / `progress.md`。

**Patterns to follow:**
- 现有 `TaskStore` 的 JSONL 写模式。
- 现有 `ProjectionContext` 的缓存模式。

**Test scenarios:**
- Happy path: 提交 `work.done` 后 snapshot 中 task closed、step completed、phase advanced 同时更新。
- Edge case: commit 失败时 snapshot 回滚到提交前状态。
- Edge case: 进程重启后 replay ledger.jsonl 重建 snapshot 与重启前一致。
- Error path: ledger.jsonl 损坏时能最佳 effort 恢复并记录错误。

**Verification:**
- `cargo nextest run -p ralph-core -- state::ledger` 通过。
- `StateLedger` snapshot 覆盖 U0 映射表中标记为 "进入 ledger" 的所有字段。

---

- [ ] U2. **Migrate StateProjector to derive from StateLedger**

**Goal:** 让 `StateProjector` 只从 `StateLedger` commit log 派生写盘，不再独立维护缓存。

**Requirements:** R3

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/src/state_projector/mod.rs`
- Modify: `crates/ralph-core/src/state_projector/task.rs`
- Modify: `crates/ralph-core/src/state_projector/progress.rs`
- Test: `crates/ralph-core/src/state_projector/tests.rs`

**Approach:**
- `StateProjector` 接收 `&StateLedger` 或 `&LedgerSnapshot`。
- `StateProjector::apply(commit_log)` 把 commit 中的 delta 写入 `tasks.jsonl` 和 `progress.md`。
- `## ORCHESTRATOR CONTEXT` 注入从 `StateLedger.snapshot()` 读取。
- 删除 `ProjectionContext` 中的独立 `tasks_cache` / `progress_cache`（或将其作为写盘缓存，读侧统一走 ledger）。

**Patterns to follow:**
- 现有 `write_progress()` 的原子 temp-file + rename 写盘模式。

**Test scenarios:**
- Happy path: `StateLedger::commit(work.done)` + `StateProjector::apply(commit)` 后磁盘状态与 snapshot 一致。
- Edge case: 多个 commit 批量 apply 时磁盘只写一次。
- Error path: 写盘失败时 ledger snapshot 不回调，但记录持久化失败事件；gate 读取 snapshot 而非磁盘。
- Integration: `## ORCHESTRATOR CONTEXT` 注入内容来自 ledger snapshot。

**Verification:**
- 原 `state_projector` 单元测试全部通过。
- `crates/ralph-core/src/event_loop/tests/runtime_state_injection.rs` 通过。

---

### Phase 2: ProtocolView 与验证统一

- [ ] U3. **Create unified ProtocolView layer**

**Goal:** 建立 lint、engine gate、runtime gate 共用的只读配置协议视图。

**Requirements:** R5, R6

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/src/preset/engine/protocol.rs`
- Modify: `crates/ralph-core/src/preset/engine/gates.rs`
- Test: `crates/ralph-core/src/preset/engine/tests.rs`

**Approach:**
- 扩展 `ProtocolView`：topic 发布权限、macro-edge（含自环排除）、handoff artifact 要求、required fields。
- **不**在 `ProtocolView` 中放入 `phase_allowed_topics` 等动态状态；动态规则从 `LedgerSnapshot` 读取。
- 移除 `hat_handoff::macro_edges::requires_handoff` 中的重复逻辑，runtime 统一调用 `ProtocolView::is_macro_edge`。
- 添加 benchmark：对比新旧路径在 `ce-executor-serial` BDD fixture 上的 `process_batch` 延迟。

**Patterns to follow:**
- 现有 `ProtocolView::from_event_loop_with_index` 构造方式。
- 现有 `engine_and_runtime_agree_on_macro_set_for_isolated` 测试。

**Test scenarios:**
- Happy path: CLI lint、engine gate、runtime gate 对 `review.dimension.ready` 的 macro-edge 结论一致。
- Edge case: `queue.advance` coordinator 自环不被误判为 macro-edge。
- Performance: per-batch `ProtocolView` 生成开销 < 5% 基线。

**Verification:**
- `cargo nextest run -p ralph-core -- protocol_view` 通过。
- benchmark 结果写入 `docs/plans/2026-06-21-002-unified-state-benchmark.md`。

---

- [ ] U4a. **Unify origin, publisher, and required-fields validation**

**Goal:** 把 event origin guard、topic format、engine required-field gate 合并为第一批无状态 `ValidationRule`。

**Requirements:** R5, R7

**Dependencies:** U3

**Files:**
- Create: `crates/ralph-core/src/validation/mod.rs`
- Modify: `crates/ralph-core/src/event_origin.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/validation/tests.rs`

**Approach:**
- 新建 `validation` 模块，定义 `ValidationRule` trait / pipeline。
- 把 origin guard、topic format、required fields 实现为纯函数规则。
- 保留阶段名 `origin` / `topic_format` / `engine_required` 用于 reason_code。
- 默认走旧路径；通过特性开关 `UNIFIED_STATE_LEDGER=1` 启用新路径。

**Test scenarios:**
- Happy path: 合法事件通过所有 pre-commit 规则。
- Error path: `ralph` hat 发业务 topic 返回 `origin:ralph_control_only`。
- Error path: 缺失 required field 返回 `engine_rejected:required_field`。
- Integration: 特性开关关闭时旧路径仍绿；开启时新路径绿。

**Verification:**
- `cargo nextest run -p ralph-core -- validation` 通过。
- 旧 `event_loop/tests/origin_guard.rs` 通过。

---

- [ ] U4b. **Unify execution contract and workflow guard validation**

**Goal:** 把 execution contract 和 workflow guard 接入统一验证流水线，通过 post-commit preview 机制保留语义。

**Requirements:** R5, R7

**Dependencies:** U4a

**Files:**
- Modify: `crates/ralph-core/src/execution_contract.rs`
- Modify: `crates/ralph-core/src/event_loop/workflow_guard.rs`（若存在）
- Modify: `crates/ralph-core/src/validation/mod.rs`
- Test: `crates/ralph-core/src/validation/tests.rs`

**Approach:**
- 把 execution contract、workflow guard 实现为 `ValidationRule`，但标记为 `post_commit`。
- pipeline 先 speculative commit，再运行 post-commit 规则，失败则 rollback。
- 保留阶段名 `execution_contract` / `workflow_guard`。

**Test scenarios:**
- Happy path: `work.done` 在 task 关闭后通过 execution contract。
- Error path: `work.done` 缺少 `task_id` 返回 `contract:missing_task_id`。
- Edge case: post-commit 失败后 snapshot rollback 干净。

**Verification:**
- 原 `event_loop/tests/execution_contract.rs` 通过。
- BDD `step_handoff/state_projection_work_done_updates_progress.yml` 通过。

---

- [ ] U4c. **Unify step-handoff and hat-handoff validation**

**Goal:** 把 step handoff gate 和 hat-handoff gate 接入统一验证，并消除 step handoff 的直接磁盘读取。

**Requirements:** R5, R7

**Dependencies:** U4b

**Files:**
- Modify: `crates/ralph-core/src/step_handoff/progress_task_gate.rs`
- Modify: `crates/ralph-core/src/hat_handoff/gate.rs`
- Modify: `crates/ralph-core/src/validation/mod.rs`
- Test: `crates/ralph-core/src/validation/tests.rs`, `crates/ralph-core/src/step_handoff/tests.rs`

**Approach:**
- 重写 `progress_task_gate`：入参改为 `&ProgressSnapshot` + `&[Task]`（来自 `LedgerSnapshot`），不再接受 `workspace: &Path`。
- 把 hat-handoff 的文件/结构/R15 校验提取为可复用函数，接入 `ValidationRule`。
- 保留阶段名 `step_handoff` / `hat_handoff`。

**Test scenarios:**
- Happy path: `queue.advance` 在 progress/task 对齐时通过。
- Error path: `progress_task_mismatch` 返回结构化 reason_code。
- Error path: hat-handoff section 缺失返回 `hat_handoff:missing_section`。
- Lint: gate 代码不再调用 `std::fs::*` 读磁盘。

**Verification:**
- BDD `step_handoff/progress_task_mismatch.yml` 通过。
- BDD `hat_handoff/macro_handoff_inject.yml`、`next_rejected.yml` 通过。

---

- [ ] U5. **Integrate handoff artifact auto-generation**

**Goal:** macro-edge 事件 accept 后，runtime 自动生成通过 validator 的 handoff artifact。

**Requirements:** R6, R13

**Dependencies:** U4c

**Files:**
- Modify: `crates/ralph-core/src/hat_handoff/allocator.rs`
- Modify: `crates/ralph-core/src/hat_handoff/validator.rs`
- Modify: `crates/ralph-core/src/preset/engine/linter.rs`
- Modify: `crates/ralph-core/src/state/ledger.rs`
- Test: `crates/ralph-core/src/hat_handoff/tests.rs`

**Approach:**
- macro-edge 校验阶段 `handoff_path` 可选；accept 后 `StateLedger.commit` 调用 allocator 按 `(iteration, seq, from, to)` 生成唯一文件。
- 生成的文件必须通过 `hat_handoff::validator::validate`。
- 统一使用 `HAT_HANDOFF_DIR` (`.ralph/agent/hat-handoff`)。
- 把生成的 `handoff_path` 写回事件元数据再 publish 到 EventBus。
- 用 `(iteration, seq, topic)` 去重，避免 retry 重复生成。

**Test scenarios:**
- Happy path: macro-edge accept 后 artifact 自动生成并写入 `HAT_HANDOFF_DIR`。
- Error path: agent 提供错误 `handoff_path` 时 runtime 拒绝或覆盖（策略在 U5 设计文档中锁定）。
- Integration: BDD `hat_handoff/macro_handoff_inject.yml` 通过。

**Verification:**
- `cargo nextest run -p ralph-core -- hat_handoff` 通过。
- engine gate 与 runtime gate 对同一 handoff 文件结论一致。

---

### Phase 3: CLI 对齐

- [ ] U6. **Migrate CLI --policy-check to unified validate_event**

**Goal:** CLI `--policy-check` 与 loop 验证使用同一 `validate_event` 流水线。

**Requirements:** R8

**Dependencies:** U4c, U5

**Files:**
- Modify: `crates/ralph-cli/src/policy_check.rs`
- Modify: `crates/ralph-cli/src/commands/emit.rs`
- Test: `crates/ralph-cli/tests/policy_check_handoff.rs`, `crates/ralph-cli/tests/integration_emit_policy.rs`

**Approach:**
- 只有当 `validate_event` 已覆盖 origin、publisher、required fields、macro-edge、handoff 文件/结构、step handoff、execution contract 后，才把 `--policy-check` 切到统一路径。
- 切换前保留 `--policy-check-compat` 模式，用于对比新旧路径差异。
- 统一后错误输出结构化 `reason_code`。

**Test scenarios:**
- Happy path: `--policy-check` 与 loop 对合法事件结论一致。
- Error path: `--policy-check` 对 misaligned `queue.advance` 返回与 loop 一致的 reason_code。
- Integration: `presets/schemas/ce-executor-serial.yml` 修改后 `cargo build` 通过一致性校验。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- policy_check` 串行通过。

---

### Phase 4: Recovery 与 Prompt

- [ ] U7a. **Replace task.resume with deterministic correction (policy rejection path)**

**Goal:** 删除 policy rejection 触发的 `task.resume`，改为 prompt 内 deterministic correction。

**Requirements:** R9, R10

**Dependencies:** U4c

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-core/src/diagnosis/responder.rs`
- Modify: `crates/ralph-core/src/event_loop/rejection.rs`
- Test: `crates/ralph-core/src/event_loop/tests/rejection.rs`（新建或修改）

**Approach:**
- 删除 `publish_policy_rejection_resume`。
- `RecoveryResponder` 不再 emit `task.resume`；维护 rejection counter 和 escalation decision。
- 构造 `CorrectionContext { reason_code, stage, expected_payload_template }`，写入当前 hat 下一次 prompt 的 `## ORCHESTRATOR CORRECTION` 区块。
- `pending_lint_resume` 并入 `CorrectionContext`。
- rejection log 持久化到 `.ralph/recovery.jsonl`，保持 `retry_key = hat+topic+reason_class` 粒度。

**Test scenarios:**
- Happy path: recoverable rejection 后 prompt 包含 correction 区块，不生成 `task.resume`。
- Error path: 连续 3 次同 reason_class 拒绝后升级 `human.guidance`。
- Error path: origin 等非 recoverable 拒绝不注入 correction。
- Integration: `ralph` 伪 hat 越权业务事件被拒后不触发 `task.resume`。

**Verification:**
- `cargo nextest run -p ralph-core -- rejection` 通过。

---

- [ ] U7b. **Migrate remaining task.resume call sites**

**Goal:** 处理 policy rejection 之外的 `task.resume` 调用点：`--continue`、missing-event fallback、wave dispatcher、drift escalation 等。

**Requirements:** R9

**Dependencies:** U7a

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs` (`initialize_resume`)
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`
- Modify: `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`
- Modify: `crates/ralph-cli/src/loop_runner/hard_gate.rs`
- Test: `crates/ralph-cli/tests/integration_resume.rs`, `crates/ralph-core/src/event_loop/tests/recovery_envelope_u7_u8.rs`

**Approach:**
- 新增 `loop.resume` 控制事件用于 `--continue`；首个 hat prompt 注入 resume context（读取 loop_id、已关闭 tasks、当前 progress 等）。
- missing-event fallback、wave dimension retry、drift hard escalation 改为直接注入 `correction_context` 或 `human.guidance`，不再写 `task.resume` 到 events.jsonl。
- 特性开关关闭时保留旧行为，开关开启时走新路径。

**Test scenarios:**
- Happy path: `--continue` 后首个 hat prompt 包含 resume context，不依赖 `task.resume`。
- Happy path: wave dimension mismatch retry 通过 `correction_context` 触发下游 hat 重试。
- Error path: drift escalation 达到阈值后发布 `human.guidance`。
- Integration: `integration_resume.rs` 通过。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- resume` 串行通过。
- `cargo nextest run -p ralph-core -- recovery_envelope` 通过。

---

- [ ] U8. **Update continue mode and diagnosis**

**Goal:** 让 `ralph run --continue` 和 `ralph diagnose` 适应无 `task.resume` 的新模型。

**Requirements:** R11, R12

**Dependencies:** U7b

**Files:**
- Modify: `crates/ralph-core/src/diagnosis/reporter.rs`
- Modify: `crates/ralph-cli/src/commands/diagnose.rs`
- Test: `crates/ralph-cli/tests/diagnose.rs`, `crates/ralph-cli/tests/ce_executor_recovery.rs`

**Approach:**
- `ralph diagnose` 优先读取持久化 rejection log（`.ralph/recovery.jsonl` + ledger commit log），聚合为结构化根因。
- 定义 source 命名空间映射表，对齐统一后的 validation stage。
- 旧会话没有 ledger rejection log 时，降级为读取 legacy recovery.jsonl。

**Test scenarios:**
- Happy path: `ralph diagnose` 对失败 run 输出单一结构化根因。
- Edge case: rejection log 为空时不 panic。
- Integration: `ce_executor_recovery.rs` 的 recovery fixture 分类测试适配新 reason_code。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- diagnose` 串行通过。

---

### Phase 5: 测试迁移与验收

- [ ] U9. **Migrate tests and BDD scenarios**

**Goal:** 更新依赖 `task.resume`、独立 gate 阶段名、旧 recovery 路径的测试和 BDD。

**Requirements:** SC1, SC2, SC3, SC4

**Dependencies:** U1-U8

**Files:**
- Modify: `crates/ralph-core/src/event_loop/tests/task_resume_ttl.rs`
- Modify: `crates/ralph-core/src/event_loop/tests/serial_lint.rs`
- Modify: `crates/ralph-core/src/event_loop/tests/execution_contract.rs`
- Modify: `crates/ralph-core/src/event_loop/tests/recovery_envelope_u7_u8.rs`
- Modify: `crates/ralph-core/src/event_loop/tests/origin_guard.rs`
- Modify: `crates/ralph-core/src/event_loop/tests/topic_format_recovery.rs`
- Modify: `crates/ralph-core/src/event_loop/tests/guidance_dedup.rs`
- Modify: `crates/ralph-core/src/event_loop/tests/handoff_dispatch.rs`
- Modify: `crates/ralph-core/src/event_loop/tests/review_step_gate.rs`
- Modify: `crates/ralph-core/src/event_loop/tests/wave_*.rs`
- Modify: `crates/ralph-core/src/event_loop/tests/loop_state.rs`
- Modify: `crates/ralph-core/tests/scenarios.rs` 及相关 YAML
- Modify: `crates/ralph-core/tests/smoke_runner.rs` 及 fixtures
- Modify: `crates/ralph-cli/tests/integration_resume.rs`
- Modify: `crates/ralph-cli/tests/ce_executor_recovery.rs`
- Modify: `crates/ralph-cli/tests/diagnose.rs`
- Modify: `crates/ralph-cli/tests/policy_check_handoff.rs`
- Modify: `crates/ralph-cli/src/loop_runner/tests.rs`

**Approach:**
- 先机械盘点所有引用（U0 输出），生成迁移矩阵。
- 把断言 `task.resume` 被注入/消费的测试改为断言 `correction_context` / `loop.resume`。
- 把断言特定 gate stage 独立分类的测试改为断言统一 validation 的 `reason_code`。
- 更新 BDD YAML 中的 `absent_events` / `events` 预期。
- 对无法直接迁移的测试，标记 `#[ignore]` 并创建 follow-up issue（需审批）。
- 新增 BDD scenario 覆盖：deterministic correction、三次拒绝升级、auto handoff、diagnose from ledger、CLI/runtime parity。

**Patterns to follow:**
- 现有 BDD scenario 的 `run_workflow_guard_scenario` 驱动方式。
- 现有 nextest 串行/并行分组（`.config/nextest.toml`）。

**Test scenarios:**
- Happy path: `ce_executor_serial` 全链路 BDD 通过。
- Happy path: `ce_executor_isolated`、`ce_executor_wave` 相关 BDD 通过。
- Error path: `progress_task_mismatch.yml` 仍触发一致拒绝。
- Error path: `hat_handoff/next_rejected.yml` 仍触发一致拒绝。
- Integration: smoke replay 不再断言 `task.resume` 出现顺序。

**Verification:**
- `cargo nextest run -p ralph-core` 通过。
- `cargo nextest run -p ralph-core --test scenarios` 通过。
- `cargo nextest run -p ralph-core --features recording --test smoke_runner` 通过。
- `cargo nextest run -p ralph-cli --bin ralph` 串行通过。

---

- [ ] U10. **Run full verification matrix and document results**

**Goal:** 全 workspace 验证，确保重构不破坏其他 preset/功能。

**Requirements:** SC5

**Dependencies:** U1-U9

**Files:**
- 无新增文件。
- 可能需要更新：`.config/nextest.toml`（若测试分组变化）。

**Approach:**
- 默认关闭特性开关时跑 `./scripts/run-tests.sh` 绿色（证明旧路径未被破坏）。
- 开启特性开关时跑 `./scripts/run-tests.sh` 绿色（证明新路径可用）。
- 跑 `cargo test --workspace --exclude ralph-e2e --doc`。
- 跑 BDD scenarios 和 smoke replay。
- 若出现 flake，用 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底确认是否为真失败。
- 记录失败项、修复动作、所有 `#[ignore]` 及其 follow-up issue。

**Test scenarios:**
- Integration: 全 workspace nextest 绿色（特性开关两种状态）。
- Integration: doctest 绿色。
- Integration: BDD scenarios 绿色。
- Integration: smoke replay 绿色。

**Verification:**
- `./scripts/run-tests.sh` 退出码 0（两种特性开关状态）。
- `cargo test --workspace --exclude ralph-e2e --doc` 退出码 0。
- 特性开关默认开启，旧路径代码在 U10 后标记为 deprecated 并在后续版本中移除。

---

## System-Wide Impact

- **Interaction graph:**
  - `EventLoop::process_parse_result` 内部调用链简化为 `pre_commit_validate → speculative_commit → post_commit_validate → finalize_commit`。
  - `RecoveryResponder` 不再 emit `task.resume`；只维护 rejection counter 和 escalation decision。
  - `EventBus` 不再承载 recovery 事件（`task.resume` 移除），只承载 agent 业务事件、系统 escalation 事件（`human.guidance`、`loop.suspend`）和新的 `loop.resume`。
  - `StateProjector` 从 `LoopState` 解耦为 ledger 的落盘层。
  - CLI `emit --policy-check` 最终与 loop 共享 `validate_event`。

- **Error propagation:**
  - 统一 validation 返回 `ValidationResult { accepted: bool, reason_code, stage, correction_hint, retry_eligible }`。
  - 拒绝事件不再 drop 后通过 `task.resume` 恢复，而是记录到持久化 rejection log 并同步注入 prompt。
  - 写盘失败与验证失败分离：commit log 写入是同步阻塞的；`tasks.jsonl` / `progress.md` 是后台派生视图，允许短暂滞后但 gate 不直接读取。

- **State lifecycle risks:**
  - `StateLedger` 是唯一可变状态源；`commit()` 必须保证 in-memory snapshot 与 `ledger.jsonl` 持久化一致。
  - 跨进程写盘现在必须通过写入 `.ralph/ledger.jsonl` 完成，或触发 ledger reload；外部工具直接改 `tasks.jsonl` / `progress.md` 会被视为只读派生视图的污染。

- **API surface parity:**
  - CLI `ralph emit --policy-check` 的行为会变得更严格（与 runtime 一致），迁移期间提供 `--policy-check-compat`。
  - `ralph diagnose` 的输出格式保持兼容，source 命名空间对齐统一 validation stage。

- **Integration coverage:**
  - BDD scenarios、smoke replay、CLI integration tests 是验证关键。
  - `ce-executor-isolated` 和 `ce-executor-wave` 的相关 scenario 必须在 U9/U10 中跑绿。

- **Unchanged invariants:**
  - `EventBus` 的 pub/sub 机制不变，只是不再 publish `task.resume`。
  - 多 hat 隔离规则（3-hat 上限、isolated 强制）不变。
  - `presets/` 的拓扑配置和 schema 格式不变。

---

## Risks & Dependencies

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| 测试改动量巨大，迁移周期超过预期 | 高 | 高 | U0 先盘点；U9 分系统迁移；允许带 follow-up issue 的 `#[ignore]`；特性开关保证旧路径可回退。 |
| `StateLedger` 成为性能瓶颈 | 中 | 中 | U3 benchmark；必要时缓存 `ProtocolView` / `HandoffIndex`；commit log 批量写盘。 |
| CLI `--policy-check` 行为变更破坏用户脚本 | 中 | 中 | 提供 `--policy-check-compat`；在变更日志中明确标注。 |
| 统一 gate 后某些边缘 case 的 reason_code 丢失 | 中 | 高 | 保留 validation stage 名作为诊断标签；U4a-U4c 测试覆盖所有原 gate 拒绝路径。 |
| `continue` 模式无法完全替代 `task.resume` | 中 | 高 | U7b 专门设计 `loop.resume`；若发现缺失场景，回退到 `human.guidance`。 |
| Handoff artifact 自动生成与现有 agent 行为冲突 | 中 | 中 | 保持 agent 可显式提供 `handoff_path`，runtime 只在缺失/无效时生成；U5 设计文档锁定策略。 |
| ledger.jsonl 持久化引入写放大 | 中 | 中 | U1 设计 compaction 策略；批量 commit 写入。 |
| isolated/wave preset 在核心路径改动后回归 | 高 | 高 | U0 录制基线；U9/U10 强制包含 isolated/wave BDD scenarios。 |

---

## Documentation / Operational Notes

- 更新 `docs/report/2026-06-21-top-3-architectural-instability-factors.md` 的「修复方向」为「已实现」。
- 在 `docs/brainstorms/2026-06-21-unified-orchestrator-state-requirements.md` 中记录最终架构决策。
- 更新 `docs/guide/runtime-diagnosis.md`，说明 `ralph diagnose` 现在从 ledger rejection log 读取根因。
- 更新 `crates/ralph-core/data/ralph-tools.md` 等 skill 文档（按 AGENTS.md 反向验证规则）。
- 在 PR 描述中清楚列出：被废弃/替换的测试、`#[ignore]` 列表及 follow-up issues、reason_code 命名空间变更、特性开关使用方法。

---

## Sources & References

- **Origin document:** `docs/brainstorms/2026-06-21-unified-orchestrator-state-requirements.md`
- **Related report:** `docs/report/2026-06-21-top-3-architectural-instability-factors.md`
- **Related plan (superseded):** `docs/plans/2026-06-21-001-fix-serial-preset-root-cause-fix-plan.md`
- **Related brainstorm (absorbed):** `docs/brainstorms/2026-06-21-serial-preset-root-cause-fix-requirements.md`
- **Key code:** `crates/ralph-core/src/event_loop/mod.rs`, `crates/ralph-core/src/state_projector/mod.rs`, `crates/ralph-core/src/preset/engine/`, `crates/ralph-core/src/hat_handoff/`, `crates/ralph-core/src/diagnosis/`, `crates/ralph-cli/src/policy_check.rs`, `crates/ralph-cli/src/loop_runner/`
