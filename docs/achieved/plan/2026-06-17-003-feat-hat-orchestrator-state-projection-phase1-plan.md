---
title: "feat: Hat orchestrator state projection — Phase 1"
type: feat
status: active
date: 2026-06-17
origin: docs/brainstorms/2026-06-17-hat-orchestrator-state-projection-requirements.md
---

# feat: Hat orchestrator state projection — Phase 1

## Overview

Phase 1 落地 **编排器状态投影** north star 的第一段：**orchestrator 成为 `tasks.jsonl` 与 `.ralph/agent/progress.md` 的唯一 writer**；hat 通过 **`## ORCHESTRATOR CONTEXT`** 只读快照理解运行态，不再被 instructions 要求手改 ledger 或 tail `events.jsonl`。

验收 preset：`ce-executor-isolated`、`ce-executor-serial`。Phase 2（events 收归、bash guard、plan status 投影、task CLI 同源）**不在本 plan**。

(see origin: `docs/brainstorms/2026-06-17-hat-orchestrator-state-projection-requirements.md`)

## Problem Frame

多 hat preset 中 **progress / tasks 双 ledger 由 agent 手维护**，与 `progress_task_gate`、operator 目视检查频繁不一致。近 2 周 gate/recovery 补丁未消除根因。本 plan 只解决 **线 A（账本漂移）**；review deadlock / wave spawn 等 **线 B** 仍由 flow-reliability 并行处理。

## Requirements Trace

- **SP-R1/R2/R3** — Phase 1 canonical artifacts + 单写者 + emit 驱动
- **SP-R4** — Task 投影（Phase 1 禁止 agent 调用 task 变更 CLI）
- **SP-R5/R7** — Progress 投影 + fail-closed
- **SP-R8** — 投影 **先于** `progress_task_gate`
- **SP-R9/R11/R12** — ORCHESTRATOR CONTEXT 注入 + instruction 读源约束
- **SP-R18/R19/R21** — 两 preset opt-in、instruction 删改、resume import
- **SC1–SC4** — Phase 1 成功标准

## Scope Boundaries

- events.jsonl 单写者、bash fail-closed、plan frontmatter 自动投影、per-hat 视图裁剪、preset lint、diagnose 对账 — **Deferred Phase 2**
- `memories.md` 写路径 — 不触碰
- 不替代 `ce-executor-flow-reliability` 工作

### Deferred to Separate Tasks

- **Phase 2 plan**（同 origin 文档 Phased Delivery 表）：SP-R6/13/14/16/17/20
- **CLI emit 预检接 progress_task_gate**（agent-recovery-gaps Plan B）：可与 Phase 2 并行，非 Phase 1 blocking

## Context & Research

### Relevant Code and Patterns

| 领域 | 路径 | 模式 |
|------|------|------|
| Emit 处理链 | `crates/ralph-core/src/event_loop/mod.rs` — `process_parse_result` | hook：SM 验证后、`apply_step_handoff_gate` 前 |
| Progress/task gate | `crates/ralph-core/src/step_handoff/progress_task_gate.rs` | 读 `.ralph/agent/progress.md` + `tasks.jsonl` |
| Wave 注入（镜像） | `crates/ralph-core/src/wave_context.rs`, `prepend_wave_context` | `to_prompt_block()` + 固定 heading |
| FR-3 注入 | `crates/ralph-core/src/state_file_injector.rs` | snapshot 文件 → prompt XML 块 |
| Task 存储 | `crates/ralph-core/src/task_store.rs`, `crates/ralph-cli/src/task_cli.rs` | Phase 1 仍直写 CLI，preset 禁 agent 调用 |
| Preset 源 | `presets/en/ce-executor-isolated.yml`, `presets/en/ce-executor-serial.yml` | embedded via `crates/ralph-cli/src/presets.rs` |
| 勿混用 | `crates/ralph-core/src/event_projection.rs` | FR-2 sidecar，≠ state projection |

### Institutional Learnings

- `docs/solutions/developer-experience/ce-executor-plan-gate-premature-completion-2026-06-02.md` — progress 滞后导致 plan-gate 误判
- `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-recovery-2026-06-17.md` — 三处状态漂移叠加 recovery 噪音
- `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` — steward 直读 ledger 与 north star 冲突（Phase 1 后 steward 改读注入块，本 plan U5 协调）

### External References

- `docs/guide/harness-extensions.md` — FR-1 / FR-3
- 无额外外部 research（本地模式充分）

## Key Technical Decisions

- **Hook 顺序**：`state machine OK` → **`StateProjector::apply(events)` 写 disk** → `apply_step_handoff_gate` → workflow guard / execution contract → bus。保证 gate 校验投影后 ledger。(see origin SP-R8)
- **Canonical progress 路径**：**仅** `.ralph/agent/progress.md`；废弃 preset 中 `.agents/scratchpad/ce-executor/{plan_name}/progress.md` 写入义务。(see origin Key Decisions)
- **新模块命名**：`crates/ralph-core/src/state_projector/`（或 `runtime_ledger/`），**不复用** `event_projection.rs` 语义。
- **映射表**：step/U → progress 字段变更由 **preset YAML 配置**（如 `state_projection.progress_mapping`）声明；投影引擎 generic，不硬编码 plan 正文。
- **Opt-in**：`event_loop.state_projection.enabled: true`（或 `workflow_contract` 子字段）；两 ce-executor preset 显式开启；默认 off。
- **WAVE 子段**：GOV-R1 `## WAVE CONTEXT` 在 Phase 1 **保留**；ORCHESTRATOR CONTEXT 的 `wave` 子段可 duplicate 摘要或 planning 时评估合并（U4 spike）。

## Open Questions

### Resolved During Planning

- **Projector hook 点**：`process_parse_result` 内 SM 后、gate 前（repo 已验证调用序）。
- **Progress 路径**：`.ralph/agent/progress.md` only。

### Deferred to Implementation

- **Preset mapping 表精确字段**：implement 时对照 preset step 命名与现有 `ProgressSnapshot` parser 对齐。
- **Resume import 边界字段**：closed task 但 progress 缺 Completed Steps 的 repair 策略（fail-closed vs 一次性 reconcile）。
- **progress-steward instructions**：U5 最小改（读 ORCHESTRATOR CONTEXT）还是 Phase 2；默认 U5 改一句读注入块。

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

```mermaid
sequenceDiagram
  participant Agent
  participant JSONL as events JSONL
  participant Loop as EventLoop
  participant Proj as StateProjector
  participant Disk as tasks.jsonl + progress.md
  participant Gate as progress_task_gate
  participant Bus as EventBus

  Agent->>JSONL: ralph emit work.done
  Loop->>JSONL: process_parse_result
  Loop->>Loop: policy + state machine
  Loop->>Proj: apply(validated batch)
  Proj->>Disk: atomic write ledger
  Loop->>Gate: check queue.advance / plan.complete
  Gate->>Disk: read canonical
  alt aligned
    Loop->>Bus: publish
  else mismatch
    Loop->>Loop: plan.blocked / recovery
  end
  Note over Loop,Agent: next iteration prepend ORCHESTRATOR CONTEXT from Proj snapshot
```

**投影输入（Phase 1 最小 topic 集）**

| Topic | Task 投影 | Progress 投影 |
|-------|-----------|---------------|
| `work.ready` | ensure/open task | set in_progress step |
| `work.done` | close task | mark step checkbox / U complete |
| `queue.advance` | — | advance Current Step |
| `plan.complete` | close remaining? | finalize Completed Steps |
| review terminal (`review.passed` / `plan.blocked` 等) | 按 mapping | Active Wave / Sequence 字段 |

## Implementation Units

- [ ] **Unit 1: State projector scaffold + config + hook**

**Goal:** 建立投影模块、配置开关、emit 链 hook（SM 后、gate 前），默认 no-op。

**Requirements:** SP-R1, SP-R2, SP-R3, SP-R8

**Dependencies:** None

**Files:**
- Create: `crates/ralph-core/src/state_projector/mod.rs`（+ `task.rs`, `progress.rs`, `mapping.rs` 子模块按需）
- Modify: `crates/ralph-core/src/lib.rs`, `crates/ralph-core/src/config/event_loop.rs`（或 `workflow_contract.rs`）— `StateProjectionConfig`
- Modify: `crates/ralph-core/src/event_loop/mod.rs` — hook 调用点
- Test: `crates/ralph-core/src/state_projector/tests.rs` 或 `crates/ralph-core/tests/state_projector.rs`

**Approach:**
- `StateProjector::apply(&self, events: &[Event], ctx: &ProjectionContext) -> Result<ApplyReport>`
- `ProjectionContext` 含 workspace、`tasks_path`, `progress_path`, mapping config
- Hook 仅在 `state_projection.enabled` 时运行；失败 → fail-closed（SP-R7），不 publish 该批或走现有 rejection 路径（与 planning 实现时二选一，文档化于 PR）

**Execution note:** 先写 hook 集成测试（enabled flag off/on），再写投影逻辑。

**Patterns to follow:**
- `step_handoff/progress_task_gate.rs` — ledger 路径常量
- `event_loop/mod.rs` — `apply_step_handoff_gate` 调用位置

**Test scenarios:**
- **Happy path:** enabled=false 时 hook 不触碰 disk，行为与现网一致
- **Happy path:** enabled=true 空 batch → 无写盘
- **Integration:** hook 在 SM 后、gate 前被调用（可用 mock projector 计数器）

**Verification:**
- nextest 新模块测试绿；现有 `ralph-core` 测试无回归

---

- [ ] **Unit 2: Task ledger projection**

**Goal:** `work.ready` / `work.done`（及 mapping 声明的其它 topic）驱动 `tasks.jsonl` 变更。

**Requirements:** SP-R4, SP-R7, SC1

**Dependencies:** Unit 1

**Files:**
- Modify: `crates/ralph-core/src/state_projector/task.rs`
- Modify: `crates/ralph-core/src/task_store.rs`（如需 package-private 写 API 供 projector 专用）
- Test: `crates/ralph-core/src/state_projector/task.rs` tests + 扩展 `task_store` 测试

**Approach:**
- 从 event payload 提取 `task_key` / `task_id` / `step` / `plan_name`（与 execution contract 字段对齐）
- `work.ready` → ensure + start；`work.done` → close（status + timestamp）
- 使用 `with_exclusive_lock` 与 TaskStore 同源路径；**禁止**与 agent CLI 并发假设——projector 在 loop 单线程批处理内运行
- payload 缺必填 → `ApplyReport::Rejected` + reason_code

**Patterns to follow:**
- `crates/ralph-core/src/event_loop/mod.rs` — `validate_execution_contract` 字段要求
- `crates/ralph-core/src/task_store.rs` — `ensure`, `close`

**Test scenarios:**
- **Happy path:** work.ready → tasks.jsonl 新增 open task
- **Happy path:** work.done → 对应 task closed
- **Edge case:** duplicate work.ready 同 task_key → idempotent 或 reject（实现时选一并测试）
- **Error path:** work.done 缺 task_id → 不 silent write，返回 reject

**Verification:**
- 单元测试覆盖 task 投影；SC1 子集可测

---

- [ ] **Unit 3: Progress ledger projection + preset mapping**

**Goal:** 同一批 emit 更新 `.ralph/agent/progress.md`（Current Step、Completed Steps、Active Wave/Sequence）。

**Requirements:** SP-R5, SP-R7, SC1, SC3

**Dependencies:** Unit 2

**Files:**
- Modify: `crates/ralph-core/src/state_projector/progress.rs`
- Modify: `crates/ralph-core/src/step_handoff/progress_task_gate.rs`（如需导出 `ProgressSnapshot` 写 API 或共用 parser）
- Modify: `presets/en/ce-executor-isolated.yml`, `presets/en/ce-executor-serial.yml` — `state_projection.progress_mapping` 段
- Modify: `presets/zh/*` 镜像（若存在对应字段）
- Test: `crates/ralph-core/src/state_projector/progress.rs` tests

**Approach:**
- 复用 `ProgressSnapshot::parse` / 格式化写回（避免第二套 markdown 方言）
- mapping 配置示例（directional）：
  ```yaml
  state_projection:
    enabled: true
    progress_mapping:
      work_done: { update: completed_step_from_payload }
      queue_advance: { update: advance_current_step }
  ```
- `queue.advance` / review terminal 更新 Active Wave/Sequence 字段（serial vs isolated 差异在 mapping 表，非 Rust 分支）

**Patterns to follow:**
- `progress_task_gate.rs` — `ProgressSnapshot`, step 解析
- `docs/solutions/developer-experience/ce-executor-plan-gate-premature-completion-2026-06-02.md` — 字段语义

**Test scenarios:**
- **Happy path:** work.done(step-01) → Completed Steps 含 step-01，Current Step 推进
- **Happy path:** 投影后 `check_progress_task_alignment` 返回 Allow
- **Edge case:** progress 文件不存在 → 从 template 创建（cold start）
- **Error path:** payload step 与 open task 不一致 → reject

**Verification:**
- 投影 + gate 联调测试绿；BDD `progress_task_mismatch` 仍绿（Unit 6）

---

- [ ] **Unit 4: ORCHESTRATOR CONTEXT injection**

**Goal:** hat 激活时注入 `## ORCHESTRATOR CONTEXT`（runtime / wave / ephemeral 子段），替代 instructions 要求读 ledger。

**Requirements:** SP-R9, SP-R11, SP-R12, SC2

**Dependencies:** Unit 3

**Files:**
- Create: `crates/ralph-core/src/runtime_state.rs`（或 `state_projector/snapshot.rs`）
- Modify: `crates/ralph-core/src/event_loop/mod.rs` — `prepend_orchestrator_context`（插入点：`prepend_wave_context` 之后）
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs` — 可选 `RALPH_RUNTIME_STATE` env（mirror GOV-R1）
- Test: `crates/ralph-core/src/event_loop/tests/runtime_state_injection.rs`

**Approach:**
- `RuntimeStateSnapshot::build(ctx)` 读 **projector 内存缓存或 canonical 文件**（优先缓存，避免 agent 竞态）
- `to_prompt_block()` → markdown + 可选 JSON fence
- 子段：`runtime`（plan/step/open tasks/progress 摘要）、`wave`（委托现有 wave_context 或摘要）、`ephemeral`（GOV-R3 relocated 列表可选并入）
- Token 预算：优先 plan_name、current_step、open_task_ids、wave received/total；defer 全量 tasks 列表

**Patterns to follow:**
- `wave_context.rs` — `to_prompt_block`, heading 常量
- `state_file_injector.rs` — 结构化块格式

**Test scenarios:**
- **Happy path:** isolated hat prompt 含 `## ORCHESTRATOR CONTEXT` 与 current_step
- **Happy path:** review-synthesizer 仍含 GOV-R1 wave 块（不破坏 SC2）
- **Edge case:** projection disabled → 不注入（或注入 stub 说明 disabled）

**Verification:**
- 新 injection 测试绿；`wave_context_injection` 现有测试无回归

---

- [ ] **Unit 5: Preset instruction cleanup (isolated + serial)**

**Goal:** 删除手改 ledger 义务；统一 progress 路径；启用 `state_projection.enabled`。

**Requirements:** SP-R18, SP-R19, SP-R11

**Dependencies:** Unit 4（注入可用后再改 instructions）

**Files:**
- Modify: `presets/en/ce-executor-isolated.yml`, `presets/en/ce-executor-serial.yml`
- Modify: `presets/zh/ce-executor-isolated-zh.yml`, `presets/zh/ce-executor-serial-zh.yml`（若 embedded）
- Modify: `crates/ralph-cli/src/presets.rs` — 更新契约测试（progress 路径、禁止 tail events）
- Modify: `scripts/ralph-zsh-plugin.zsh` — 无 preset 名变更则跳过

**Approach:**
- 删除/替换所有「Update progress.md (path: `.agents/scratchpad/...`)」HARD RULE → 「emit 后系统更新；以 ## ORCHESTRATOR CONTEXT 为准」
- 删除「tail events.jsonl 数 wave / 读 tasks.jsonl 推导下一步」
- 禁止 agent 调用 `ralph tools task ensure|start|close|fail`（保留 operator 人工 CLI）
- `progress-steward`：改为读 ORCHESTRATOR CONTEXT，不直读四文件决策树
- 开启 `state_projection.enabled: true`

**Patterns to follow:**
- GOV 脑暴删 prompt patch 的同样语气（机制优先）

**Test scenarios:**
- **Integration:** `presets.rs` 断言无 scratchpad progress 写入路径、含 ORCHESTRATOR CONTEXT 说明
- **Integration:** `ralph preset check --strict -H builtin:ce-executor-isolated` 绿

**Verification:**
- preset 嵌入测试绿；manifest 无变更则无需 rebuild  panic 检查

---

- [ ] **Unit 6: Resume bootstrap + BDD / regression**

**Goal:** resume 一次性 import；扩展 BDD；满足 SC1–SC4。

**Requirements:** SP-R21, SC1–SC4

**Dependencies:** Units 1–5

**Files:**
- Modify: `crates/ralph-core/src/state_projector/mod.rs` — `bootstrap_from_disk`
- Modify: `crates/ralph-core/tests/scenarios/step_handoff/progress_task_mismatch.yml`（如需 fixture 调整）
- Create: `crates/ralph-core/tests/scenarios/state_projection/work_done_updates_progress.yml`（或扩展现有 scenario）
- Modify: `crates/ralph-core/tests/scenarios.rs` — 注册新 scenario

**Approach:**
- Loop resume：若 enabled 且 projector 空 → 从 tasks.jsonl + progress.md import 内存状态
- BDD：emit work.done → 投影更新 progress → queue.advance gate Allow
- Replay：noble-peacock 类 fixture 断言投影后 progress/tasks 一致（SC3 最小 fixture）

**Test scenarios:**
- **Integration:** bootstrap 后 emit 仅增量变更
- **Integration:** BDD work_done_updates_progress PASS
- **Regression:** `cargo nextest run -p ralph-core --test scenarios` 绿
- **Regression:** `./scripts/run-tests.sh` 全 workspace（exclude e2e）绿

**Verification:**
- SC1–SC4 满足；document 更新 CLAUDE.md/AGENTS.md **仅当**新增 config 字段需文档化（minimal）

---

## System-Wide Impact

- **Interaction graph:** `process_parse_result` 新增写盘步骤；`build_prompt` 新增 prepend；preset YAML 新 config 段；**不**改 EventBus 路由
- **Error propagation:** 投影 fail-closed → 事件批不进入 gate/bus（或单事件 reject，实现时统一策略并测试）
- **State lifecycle risks:** projector 与 task_cli 直写竞态 — Phase 1 靠 preset 禁 agent task CLI + loop 单线程批处理；Phase 2 CLI 同源
- **API surface parity:** `ralph emit` 行为不变；`ralph tools task` 对 agent 仅 instruction 层禁止
- **Integration coverage:** BDD scenario 证明 emit → 投影 → gate 链；unit 测试不足处靠 scenario 补
- **Unchanged invariants:** FR-2 event_projection、hat_channel merge、flow_lifecycle、memories 路径

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| 投影规则与 preset step 命名漂移 | mapping 表 + preset 契约测试 |
| ProgressSnapshot 写回破坏 hand-parse | 复用同一 parser 往返测试 |
| Token 膨胀（ORCHESTRATOR CONTEXT） | 字段优先级 + 摘要而非全文 |
| progress-steward 与投影语义冲突 | U5 改 instructions 读注入块 |
| Phase 1 未禁 bash 写盘，agent 仍可破坏 ledger | 接受；SC5 属 Phase 2；靠 instruction + 投影后立即 gate 检测 |

## Documentation / Operational Notes

- 本 plan **不**强制更新 `ralph-tools.md`（task CLI 语义 Phase 1 不变）
- Operator：`RALPH_DIAGNOSTICS=1` 下可在 orchestration.jsonl 观察投影 apply 记录（若 U1 加 diagnostic event，optional）

## Phased Delivery

本文件 **即 Phase 1**。Phase 2 另起 plan（origin 文档 Phased Delivery 表）。

## Sources & References

- **Origin document:** [docs/brainstorms/2026-06-17-hat-orchestrator-state-projection-requirements.md](../brainstorms/2026-06-17-hat-orchestrator-state-projection-requirements.md)
- Code: `crates/ralph-core/src/event_loop/mod.rs`, `step_handoff/progress_task_gate.rs`, `wave_context.rs`
- Learnings: `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-recovery-2026-06-17.md`
