---
date: 2026-07-03
topic: supervisor-rusqlite-parallel-preset
type: requirements
supersedes_in_part:
  - docs/brainstorms/2026-06-18-supervisor-wave-protocol-upgrade-requirements.md
  - docs/brainstorms/2026-06-27-ralph-orchestrator-mechanism-foundation-requirements.md#4.2.5
related:
  - docs/brainstorms/2026-07-02-ce-executor-pipeline-preset-requirements.md
  - docs/achieved/report/2026-06-17-ce-executor-wave-abstraction-issues-diagnosis.md
  - specs/agent-waves/design.md
  - crates/ralph-core/src/worktree.rs
  - presets/en/ce-executor-pipeline.yml
---

# Supervisor + rusqlite 编排状态 + 并行 Preset 交付需求文档

> **定位**：本需求是 Ralph **微服务式并行编排**的单一事实源（SSOT）。在显式 `event_loop.supervisor.enabled: true` 时，用 **rusqlite**（`.ralph/supervisor.db`）管理 Supervisor 编排态（wave / slot / 幂等 / 反压 / 取消 / 补偿 / worktree 绑定）；**不取代**业务事件 JSONL、`plan.md`、`progress.md` 等现有文件链路。交付物包含机制实现 + builtin preset **`ce-executor-supervisor`**（参考 `ce-executor-pipeline` 一条龙，但执行 / 评审 / 修复三阶段改为 Supervisor 并行 + worktree 写隔离）。

---

## Summary

用户需要：给定开发计划 `plan.md`，Ralph 自动 **拆解任务 → 多 agent 在 worktree 中并行执行 → 集成合并 → 多维度并行 review → 汇总 fix plan → 拆解修复项 → 多 agent 并行修复 → 集成 → 对齐 → 报告**。

核心机制：

1. **Supervisor 模式**：Ralph loop_runner 作为唯一调度控制点，实现完整 6 件套（反压 / 取消 / 持久化 / 幂等 / 内容去重 / 补偿）。
2. **rusqlite 编排库**：`.ralph/supervisor.db` 持久化所有「调度员脑子里」的状态；进程崩溃可恢复。
3. **显式开关**：仅当 preset 配置 `event_loop.supervisor.enabled: true` 时启用；未配置或为 `false` 时行为与当前代码完全一致。
4. **worktree 绑定**：执行 wave 与修复 wave 的写代码 worker **必须**由 orchestrator 分配独立 worktree；review wave 默认 **shared_readonly**（不建 worktree）。
5. **交付 preset**：`builtin:ce-executor-supervisor`，端到端可跑 BDD scenario。

本需求 **部分取代** `2026-06-18-supervisor-wave-protocol-upgrade-requirements.md` 中「`.ralph/wave-state/*.json` 持久化」方案，改为 rusqlite；6 件套语义保持不变。**显式 override** `2026-06-27-ralph-orchestrator-mechanism-foundation-requirements.md` §4.2.5「不引入 SQLite / sled」——**仅限 Supervisor 编排路径**。

---

## Problem Frame

### 现状痛点

| 症状 | 根因 | 本需求如何治 |
|------|------|----------------|
| 并行 worker 共享 `events.jsonl` / git 工作区 | 无进程与文件系统隔离 | worktree per slot + 单写者 Supervisor merge |
| wave 状态进程挂后丢失 | `WaveTracker` 纯内存 | rusqlite 持久化 + 启动恢复 |
| 12s 内二次 `work.done` | 无 DB 级幂等 | `dispatch_records.idempotency_key UNIQUE` |
| 取消后 worker 仍烧 token | 无分布式取消 | `waves.cancel_requested` + kill worker |
| synthesizer 等不齐 N 维 | 无 slot 级可观测 | `wave_slots` 状态机 + fan-in SQL |
| `ce-executor-pipeline` 串行执行慢 | 单 executor / 串行 6 维 | 并行 execute + review + fix |

### 用户目标（与 `ce-executor-pipeline` 对比）

| 阶段 | `ce-executor-pipeline` | `ce-executor-supervisor`（本需求） |
|------|------------------------|-----------------------------------|
| 计划 | plan-reviewer 定稿 | 同左 |
| 执行 | 单 executor 整包 TDD | **task-planner 拆解 → exec wave → N × unit-executor（worktree）→ exec-integrator** |
| 评审 | 6 hat 串行链 | **review-coordinator → review wave × 6（shared_readonly）→ review-synthesizer** |
| 修复 | 单 fixer | **fix-planner 拆解 → fix wave → N × fix-worker（worktree）→ fix-integrator** |
| 收尾 | alignment → reporter | 同左 |

---

## Key Decisions

| ID | 决策 | 理由 |
|----|------|------|
| KD-1 | **rusqlite**（`bundled`）作为 v1 唯一 Supervisor 存储后端 | SQL 表达 fan-in / 幂等 / 查询；成熟、嵌入、无额外服务 |
| KD-2 | **显式开关** `supervisor.enabled: true`；不根据 wave 自动推断 | 避免 silent 行为变化；串行 preset 零成本 |
| KD-3 | **JSONL 保留**为业务事件审计总线；DB 不取代 EventBus | 兼容 TUI / diagnose / replay / 现有 hat 触发 |
| KD-4 | **单写者**：仅 loop_runner / Supervisor 写 `supervisor.db` | worker 通过 emit / 回调交结果，不直连 DB |
| KD-5 | **worktree 由 orchestrator 创建**，agent 禁止 `git worktree add` | 与现有 preset 安全规则一致 |
| KD-6 | **执行 / 修复 wave** 使用 `isolation_mode: worktree`；**review wave** 使用 `shared_readonly` | 写操作隔离；读评审共享集成后代码树 |
| KD-7 | **新 preset** `ce-executor-supervisor`，**不修改** `ce-executor-pipeline` / `ce-executor-serial` | 并行是可选升级路径 |
| KD-8 | **schema 一次设计全 6 件套**；实现可分 milestone | 避免表结构反复迁移 |
| KD-9 | **integrator hat** 负责 worktree 合并 / 冲突上报；Supervisor DB 记录 merge 状态 | 并行写的产物必须可集成 |
| KD-10 | v1 worker 由 **Ralph spawn 子进程**（`RALPH_WAVE_WORKER=1`），不依赖 backend 内置 Subagent | 可测、可控；backend Subagent 为后续适配 |

---

## Actors

| ID | 角色 | 职责 |
|----|------|------|
| A0 | **Supervisor（loop_runner）** | 读写 `supervisor.db`；派发 / 取消 / fan-in；worker 结果 merge 到 `events.jsonl`；**注入 wave 协调事件**（`exec.wave.complete` 等）；创建 / 回收 worktree |
| A1 | `plan-reviewer` | 评审并改进计划文档；emit `plan.ready` / `plan.blocked` |
| A2 | `task-planner` | 读取定稿计划，拆解为可并行 implementation units；写 `plan_units.json`；emit `exec.batch.ready` |
| A3 | `exec-coordinator` | 将 units 组 batch wave；`ralph wave emit` → `exec.unit.ready` × N |
| A4 | `unit-executor`（wave worker） | 在 **分配的 worktree** 内对单个 unit TDD 实现；emit `exec.unit.done` / `exec.unit.failed` |
| A5 | `exec-integrator` | 收齐 exec wave；合并 worktree 改动到主工作区；跑集成验证；emit `work.done` / `work.failed` |
| A6 | `review-coordinator` | 发起 6 维 review wave；emit `review.wave.ready` × 6 |
| A7 | `dimension-reviewer`（wave worker） | 只读评审单维度；emit `review.dimension.done` |
| A8 | `review-synthesizer` | fan-in 6 维结果；写 `fix_plan_file`；emit `review.complete` |
| A9 | `fix-planner` | 将 fix plan 拆解为可并行 fix items；写 `fix_units.json`；emit `fix.batch.ready` |
| A10 | `fix-coordinator` | 组 fix wave；emit `fix.unit.ready` × M |
| A11 | `fix-worker`（wave worker） | 在 worktree 内修复单个 fix item；emit `fix.unit.done` / `fix.unit.failed` |
| A12 | `fix-integrator` | 收齐 fix wave；合并 worktree；验证；emit `fix.done` |
| A13 | `alignment` | 核对计划 / 修复执行度；emit `align.done` |
| A14 | `reporter` | 汇总报告；emit `report.done` + `LOOP_COMPLETE` |
| A15 | `progress-steward` | loop 级 stall 兜底（与 pipeline 同模式） |

---

## 存储边界（JSONL / 文件 vs SQLite）

### 原则

- **SQLite = 调度员账本**（可变、有事务、有约束）。
- **JSONL / 文件 = 业务世界官宣**（追加审计、人类可读、agent 技能文档已描述的路径）。

### 进 `supervisor.db` 的状态（必须）

| 类别 | 内容 |
|------|------|
| Wave 生命周期 | wave_id、类型、phase、status、expected_total、timeout、cancel |
| Slot | index、payload 摘要、assigned_hat、status、重试次数 |
| 资源绑定 | worktree_path、branch、isolation_mode、resource_status |
| 派发 / 幂等 | idempotency_key、dispatch 时间、worker_pid、dispatch_status |
| 结果元数据 | content_hash、result_event_id、dedup 标记 |
| 反压队列 | enqueued wave 顺序 |
| 补偿任务 | compensation job 状态 |

### 保留文件路径（必须不改语义）

| 路径 | 用途 |
|------|------|
| `.ralph/events.jsonl`（或 loop 标记路径） | merge 后的业务事件；hat 触发仍读此文件 |
| `.ralph/agent/tasks.jsonl` | v1 仍 JSONL；**并行 worker 禁止直接写** |
| `plan.md` / operator 传入的 plan | 计划真源 |
| `.ralph/agent/progress.md` | 进度叙事 |
| **`plan_units.json`**（新增，preset 产物） | task-planner 拆解结果（人类可读） |
| **`fix_units.json`**（新增） | fix-planner 拆解结果 |
| 各维度 review 产物文件 | 与 pipeline 同模式 |
| `fix_plan_file` | synthesizer 产出 |

### 显式不做（v1）

- 不把 `events.jsonl` 整体迁入 SQL 表作为主事件源。
- 不把 `tasks.jsonl` 整体迁入 SQL（aggregator / integrator 仍用 `TaskStore`）。
- 不使用 Turso / 远程 DB。

---

## 开关语义

### 配置

```yaml
event_loop:
  supervisor:
    enabled: true                    # 必须显式 true；缺省 = false
    db_path: ".ralph/supervisor.db"  # 可选；缺省为此路径
    max_concurrent_workers: 16       # 可选；反压上限
    aggregate_timeout_secs: 600      # 可选；wave 超时
```

### 行为矩阵

| `supervisor.enabled` | 有 wave dispatch | 行为 |
|----------------------|------------------|------|
| `false` / 未配置 | 任意 | **现状**：内存 `WaveTracker`；不创建 `.db` |
| `true` | 否 | 不创建 active wave 行；可惰性创建空库或首次 wave 时创建 |
| `true` | 是 | 打开 `supervisor.db`；所有 wave 编排走 `SupervisorStore` |

###  Lint

- **R-SW-1.** `preset_lint` 新增规则：preset 声明 `supervisor.enabled: true` 时，**必须** `execution_mode: isolated`，且至少一个 hat 声明 `concurrency > 1` 或文档化使用 `ralph wave emit`。
- **R-SW-2.** `supervisor.enabled: true` 且 hat 含 `concurrency > 1` 时，schema 必须定义对应 wave 事件的 `required_fields`（含 `wave_id` 等）。

---

## 数据库设计（`.ralph/supervisor.db`）

### 通用约定

- **R-DB-0.** 使用 `rusqlite` + `bundled`；`PRAGMA journal_mode=WAL`; `PRAGMA foreign_keys=ON`; `PRAGMA user_version=<schema_version>`。
- **R-DB-1.** 所有时间戳存 ISO-8601 UTC 文本或 Unix 毫秒（实现期二选一，全库统一）。
- **R-DB-2.** migration 在 loop 启动时自动执行；失败则 **拒绝启动 supervisor 模式**（fail closed），写 `recovery.jsonl`。
- **R-DB-3.** 仅 **Supervisor 线程 / `spawn_blocking` 池** 访问连接；禁止 worker 进程打开同一 DB 写连接（v1）。

### 表：`schema_migrations`

| 列 | 类型 | 说明 |
|----|------|------|
| `version` | INTEGER PK | |
| `applied_at` | TEXT NOT NULL | |

### 表：`waves`

| 列 | 类型 | 约束 | 说明 |
|----|------|------|------|
| `wave_id` | TEXT | PK | 与 event `wave_id` 一致 |
| `loop_id` | TEXT | NOT NULL | 当前 loop |
| `wave_kind` | TEXT | NOT NULL | `exec` \| `review` \| `fix` |
| `phase` | TEXT | NOT NULL | `dispatch` \| `collect` \| `integrate` \| `done` |
| `status` | TEXT | NOT NULL | `pending` \| `running` \| `completed` \| `partial` \| `failed` \| `cancelled` \| `timeout` |
| `target_hat_id` | TEXT | NOT NULL | worker hat |
| `expected_total` | INTEGER | NOT NULL | slot 数 |
| `completed_count` | INTEGER | DEFAULT 0 | 成功 slot |
| `failed_count` | INTEGER | DEFAULT 0 | |
| `isolation_mode` | TEXT | NOT NULL | `worktree` \| `shared_readonly` |
| `timeout_at` | TEXT | | |
| `cancel_requested` | INTEGER | DEFAULT 0 | 0/1 |
| `cancel_reason` | TEXT | | |
| `coordinator_hat_id` | TEXT | | 发起 wave 的 hat |
| `created_at` | TEXT | NOT NULL | |
| `updated_at` | TEXT | NOT NULL | |

索引：`idx_waves_loop_status (loop_id, status)`。

### 表：`wave_slots`

| 列 | 类型 | 约束 | 说明 |
|----|------|------|------|
| `wave_id` | TEXT | FK → waves | |
| `slot_index` | INTEGER | | |
| | | PK (wave_id, slot_index) | |
| `status` | TEXT | NOT NULL | `pending` \| `dispatched` \| `running` \| `completed` \| `failed` \| `cancelled` |
| `assigned_payload_json` | TEXT | NOT NULL | 完整 wave payload |
| `assigned_unit_id` | TEXT | | 如 `u1` / fix-item id |
| `assigned_dimension` | TEXT | | review 专用 |
| `retry_count` | INTEGER | DEFAULT 0 | |
| `max_retries` | INTEGER | DEFAULT 1 | |
| `worker_pid` | INTEGER | | |
| `idempotency_key` | TEXT | UNIQUE | `{wave_id}:{slot_index}:{payload_hash}` |
| `last_error` | TEXT | | |
| `created_at` | TEXT | NOT NULL | |
| `updated_at` | TEXT | NOT NULL | |

### 表：`slot_resources`（worktree 绑定）

| 列 | 类型 | 约束 | 说明 |
|----|------|------|------|
| `wave_id` | TEXT | FK | |
| `slot_index` | INTEGER | FK 复合 | |
| `resource_type` | TEXT | NOT NULL | v1 仅 `worktree` |
| `worktree_path` | TEXT | | 绝对或相对 repo_root |
| `branch_name` | TEXT | | 如 `ralph/{loop_id}-u1` |
| `resource_status` | TEXT | NOT NULL | `allocated` \| `active` \| `merged` \| `discarded` \| `failed` |
| `merge_commit_sha` | TEXT | | integrator 填入 |
| `created_at` | TEXT | NOT NULL | |
| `updated_at` | TEXT | NOT NULL | |

**R-WT-1.** `isolation_mode=worktree` 的 slot 在 `dispatched` 前 **必须** 有 `slot_resources` 行且 `worktree_path` 非空。  
**R-WT-2.** worktree 创建调用 `ralph_core::worktree::create_worktree`，命名 `{loop_id}-{wave_kind}-{slot_index}`。  
**R-WT-3.** integrator 成功后 `resource_status=merged`；失败 `failed` 并写 `last_error`。  
**R-WT-4.** wave 终止后 Supervisor 调度 worktree 清理（`remove_worktree`），失败写诊断不阻塞 loop。

### 表：`dispatch_records`

| 列 | 类型 | 说明 |
|----|------|------|
| `idempotency_key` | TEXT PK | |
| `wave_id` | TEXT NOT NULL | |
| `slot_index` | INTEGER NOT NULL | |
| `dispatch_status` | TEXT | `pending` \| `running` \| `completed` \| `cancelled` \| `failed` |
| `worker_pid` | INTEGER | |
| `dispatched_at` | TEXT NOT NULL | |
| `expires_at` | TEXT | 滑动窗口过期 |

### 表：`worker_results`

| 列 | 类型 | 说明 |
|----|------|------|
| `wave_id` | TEXT | |
| `slot_index` | INTEGER | PK 复合 |
| `content_hash` | TEXT NOT NULL | SHA-256 前 16 位 hex |
| `result_topic` | TEXT | |
| `result_payload_json` | TEXT | |
| `merged_to_events` | INTEGER DEFAULT 0 | 是否已 append 主 jsonl |
| `deduplicated` | INTEGER DEFAULT 0 | |
| `recorded_at` | TEXT NOT NULL | |

**R-DB-4.** 同 `(wave_id, slot_index)` 且 `content_hash` 相同 → 拒绝重复插入，记诊断 `wave.result.deduplicated`。

### 表：`wave_queue`（反压）

| 列 | 类型 | 说明 |
|----|------|------|
| `queue_id` | INTEGER PK AUTOINCREMENT | |
| `wave_id` | TEXT UNIQUE | |
| `enqueued_at` | TEXT NOT NULL | |
| `priority` | INTEGER DEFAULT 0 | FIFO = 按 queue_id |

### 表：`compensation_jobs`

| 列 | 类型 | 说明 |
|----|------|------|
| `job_id` | INTEGER PK AUTOINCREMENT | |
| `wave_id` | TEXT NOT NULL | |
| `trigger` | TEXT | `on_failure` \| `on_timeout` \| `on_partial` |
| `action_kind` | TEXT | `emit_event` \| `call_hook` \| `noop` |
| `action_payload_json` | TEXT | |
| `status` | TEXT | `pending` \| `executed` \| `failed` |
| `executed_at` | TEXT | |
| `error` | TEXT | |

---

## Supervisor 六件套需求

> 编号延续 `2026-06-18` 母舰文档语义；持久化介质改为 rusqlite（取代 `.ralph/wave-state/*.json`）。

### A. Backpressure（反压）

- **R-A1.** `active_workers` = `COUNT(*)` FROM `wave_slots` WHERE `status IN ('dispatched','running')`；若 `>= max_concurrent_workers`（默认 16，preset 可覆盖），新 wave **入队** `wave_queue`，返回 `DispatchOutcome::BackpressureEnqueued`。
- **R-A2.** worker 完成导致 active 下降时，按 FIFO 从 `wave_queue` 取出下一 `wave_id` 继续 dispatch。
- **R-A3.** 反压时写诊断 topic `wave.backpressure.paused`（payload 含 `wave_id`, `active_workers`, `queue_depth`）到 `recovery.jsonl` 或 events 诊断流。
- **R-A4.** CLI `ralph wave dispatch --force`（若保留）绕过反压并记 `wave.backpressure.bypassed`。

### B. 分布式取消

- **R-B1.** `cancel_wave(wave_id, reason)` 置 `waves.cancel_requested=1`，写 `cancel_reason`。
- **R-B2.** dispatch 前检查 `cancel_requested`；已取消则 slot 标 `cancelled`，不 spawn。
- **R-B3.** 对已 spawn worker，Supervisor 通过 PID kill 或 backend cancel protocol 终止；无 PID 时标 `cancelled` 并记限制。
- **R-B4.** partial / cancelled wave 走 `incomplete_wave_gate` 降级；integrator 不得假装成功。

### C. 状态持久化

- **R-C1.** 每次 wave / slot / resource 变更在 **同一 SQL 事务** 内提交。
- **R-C2.** loop 启动时 `SupervisorStore::recover_active_waves(loop_id)`；`timeout_at < now` 标 `timeout` 并走 R-B4。
- **R-C3.** `status IN ('completed','cancelled','failed')` 且超过 7 天的 wave 可归档删除（保留 `worker_results` 摘要或整 wave 删除——实现期选「整 wave 删除」并记 diagnose 统计）。
- **R-C4.** DB 写入失败：内存态可继续但写 `wave.persistence.failed`；**supervisor.enabled=true 时若无法打开 DB 则拒绝启动 wave**（fail closed）。

### D. 幂等键

- **R-D1.** 格式：`{wave_id}:{slot_index}:{sha256(payload)[0:16]}`。
- **R-D2.** dispatch 前 `INSERT INTO dispatch_records`；冲突则 `DuplicateKey`，不 spawn。
- **R-D3.** `dispatch_status` 生命周期完整记录；`completed|cancelled` 后 `expires_at` 过期可清理。
- **R-D4.** 仅 spawn 前 dedup；Running 状态允许 supervisor 判定为 stall 后重试（新 idempotency 代或 retry_count）。

### E. 内容哈希去重

- **R-E1.** 结果入库前计算 `content_hash`。
- **R-E2.** 同 slot 同 hash → `deduplicated=1`，不重复 merge 到 events.jsonl。
- **R-E3.** 同 slot 不同 hash → 替换 `worker_results` 行，记 `wave.result.replaced` 诊断。
- **R-E4.** `ralph diagnose` 可统计 deduplicated / replaced 次数。

### F. 补偿路径

- **R-F1.** preset 可选 `hat.wave.compensation` 配置；映射到 `compensation_jobs` 模板。
- **R-F2.** `action_kind`：`emit_event` | `call_hook` | `noop`。
- **R-F3.** wave 进入 `failed|timeout|partial` 时执行对应 jobs；结果写 `compensation_jobs.status`。
- **R-F4.** 补偿失败不阻塞 wave 终态。
- **R-F5.** 缺省 compensation = noop。

---

## Worktree 编排需求

- **R-WT-5.** 仅 `exec` / `fix` wave 使用 worktree；`review` wave `isolation_mode=shared_readonly`，`slot_resources` 可为空。
- **R-WT-6.** worker 进程环境变量必须注入：`RALPH_WAVE_WORKER=1`、`RALPH_WAVE_WORKTREE_PATH`、`RALPH_WAVE_ID`、`RALPH_WAVE_INDEX`、`RALPH_WAVE_UNIT_ID`（如有）。
- **R-WT-7.** worker **禁止** `git worktree add` / `git checkout -b`（preset instructions + origin guard 双保险）。
- **R-WT-8.** 每个 worktree 使用独立 per-worker events 文件（现有 `wave-{id}-{index}.jsonl` 模式保留）；merge 由 Supervisor 事务完成后 append 主 `events.jsonl`。
- **R-WT-9.** integrator 合并策略 v1：**顺序 merge**（按 `slot_index` 升序 cherry-pick 或 patch apply）；冲突则 `work.failed` / `fix.failed`，`reason=integrate_merge_conflict`，附冲突文件列表。
- **R-WT-10.** 合并成功后跑 **全量测试**（与 pipeline executor DoD 一致）作为 integrator 门控。

---

## 代码模块（实现指引）

| 模块 | 路径 | 职责 |
|------|------|------|
| `SupervisorStore` trait | `crates/ralph-core/src/supervisor/store.rs` | 抽象存储；测试用内存实现 |
| `RusqliteSupervisorStore` | `crates/ralph-core/src/supervisor/rusqlite.rs` | SQL + migration |
| `SupervisorCoordinator` | `crates/ralph-core/src/supervisor/coordinator.rs` | 6 件套编排；接 wave_detection |
| 接入点 | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | `supervisor.enabled` 分支 |
| 配置 | `crates/ralph-core/src/config/loop_config.rs` | `SupervisorConfig` 字段 |

**R-IMPL-1.** `supervisor.enabled=false` 时 **不得** 链接初始化 rusqlite（可用 feature `supervisor-db` 或运行时惰性加载；实现期二选一，优先 feature 减小默认二进制）。

**R-IMPL-2.** 所有 DB 操作在 `tokio::task::spawn_blocking` 内执行。

---

## 交付 Preset：`ce-executor-supervisor`

### 概述

- **名称**：`ce-executor-supervisor`
- **定位**：`ce-executor-pipeline` 的 **Supervisor 并行版**；计划评审 + 对齐 + 报告语义与 pipeline 对齐；执行 / 评审 / 修复三阶段并行化。
- **启动示例**：
  ```bash
  ralph run -H builtin:ce-executor-supervisor -p docs/plans/my-plan.md
  ```

### 必选配置片段

```yaml
event_loop:
  execution_mode: isolated
  supervisor:
    enabled: true
    db_path: ".ralph/supervisor.db"
    max_concurrent_workers: 8
    aggregate_timeout_secs: 600
```

### Hat 列表（功能 hat 16 + progress-steward）

| hat_id | concurrency | 说明 |
|--------|-------------|------|
| `plan-reviewer` | 1 | 同 pipeline |
| `task-planner` | 1 | 拆解 plan → `plan_units.json` |
| `exec-coordinator` | 1 | 发 exec wave |
| `unit-executor` | ≥4（preset 默认 4） | wave worker，worktree |
| `exec-integrator` | 1 | triggers: `exec.wave.complete`；合并 + 集成测试 |
| `review-coordinator` | 1 | 发 review wave |
| `dimension-reviewer` | ≥6 | wave worker，只读 |
| `review-synthesizer` | 1 | triggers: `review.wave.complete`；aggregate 与 DB fan-in 双一致 |
| `fix-planner` | 1 | 拆解 → `fix_units.json` |
| `fix-coordinator` | 1 | 发 fix wave |
| `fix-worker` | ≥4 | wave worker，worktree |
| `fix-integrator` | 1 | triggers: `fix.wave.complete`；合并 + 验证 |
| `alignment` | 1 | 同 pipeline |
| `reporter` | 1 | 同 pipeline |
| `progress-steward` | 1 | stall 兜底 |

### 主流程（Key Flow F-MAIN）

```
work.start
  → plan-reviewer           → plan.ready | plan.blocked
  → task-planner            → exec.batch.ready
  → exec-coordinator        → ralph wave emit exec.unit.ready × N
       [Supervisor DB: exec wave, worktree × N, unit-executor workers]
       [各 worker 在独立 worktree 内 TDD，emit exec.unit.done / failed]
       [Supervisor fan-in 收齐 → merge worker 事件 → inject exec.wave.complete]
  → exec-integrator         → 【合并 N 个 worktree → 主工作区 → 跑全量测试】
                            → work.done | work.failed
  → review-coordinator      → ralph wave emit review.wave.ready × 6
       [Supervisor DB: review wave, shared_readonly；审的是合并+测试通过后的代码树]
       [Supervisor fan-in 收齐 → inject review.wave.complete]
  → review-synthesizer      → review.complete
  → fix-planner             → fix.batch.ready
  → fix-coordinator         → ralph wave emit fix.unit.ready × M
       [Supervisor DB: fix wave, worktree × M, fix-worker workers]
       [Supervisor fan-in 收齐 → inject fix.wave.complete]
  → fix-integrator          → 【合并 M 个 worktree → 主工作区 → 跑全量测试】
                            → fix.done
  → alignment               → align.done
  → reporter                → report.done → LOOP_COMPLETE
```

> **硬门槛**：`work.done` / `fix.done` **只能在**「所有 worktree 已合并进主工作区 **且** 集成测试通过」之后 emit。并行 worker 的 `exec.unit.done` **不等于** 整阶段完成；它只是 slot 级「本 unit 在独立 worktree 内自测通过」的信号。

### 业务事件 Topic（新增 + 沿用）

#### 协调事件（Supervisor runtime 注入 — wave 收齐 → 下一阶段桥接）

> **语义**：agent worker 只 emit slot 级结果（`*.unit.done` / `review.dimension.done`）。当 Supervisor 在 DB 中确认 fan-in 完成、且 worker 结果已 merge 进主 `events.jsonl` 后，由 **loop_runner 注入**协调事件（非 agent hat emit）。integrator / synthesizer **只认协调事件**，不认单个 slot 事件。

| Topic | Publisher | Consumer | 触发条件 |
|-------|-----------|----------|----------|
| `exec.wave.complete` | **Supervisor（runtime）** | `exec-integrator` | exec wave 全部 required slot `completed`（或 preset 允许的 partial 策略满足） |
| `exec.wave.failed` | **Supervisor（runtime）** | `reporter`（逃逸） | exec wave `timeout` / `cancelled` / 不可恢复 partial |
| `review.wave.complete` | **Supervisor（runtime）** | `review-synthesizer` | review wave 6 slot 全部 `review.dimension.done` 已 merge |
| `review.wave.failed` | **Supervisor（runtime）** | `reporter`（逃逸） | review wave incomplete / timeout |
| `fix.wave.complete` | **Supervisor（runtime）** | `fix-integrator` | fix wave 全部 required slot `completed` |
| `fix.wave.failed` | **Supervisor（runtime）** | `reporter`（逃逸） | fix wave `timeout` / `cancelled` / 不可恢复 partial |

**R-COORD-1.** 协调事件 **只能**由 Supervisor 注入；任何 agent hat 尝试 `ralph emit exec.wave.complete` 等 topic 必须被 `event_policy` + origin guard **拒绝**（`source` 非 supervisor / 无 runtime 标记）。

**R-COORD-2.** 注入顺序（与 R-MRG-1 一致）：DB fan-in 提交 → merge slot 结果到 `events.jsonl` → **再** inject `*.wave.complete`；禁止在 merge 完成前 inject complete。

**R-COORD-3.** `exec-integrator` 的 `triggers` **必须**仅包含 `exec.wave.complete`（不含 `exec.unit.done`）。`fix-integrator` 同理。`review-synthesizer` **必须**包含 `review.wave.complete`（可保留对最后一维 `review.adversarial.done` 的兼容 trigger 仅用于 serial 回退 preset，本 preset 不使用）。

**R-COORD-4.** `*.wave.failed` payload 必须含：`wave_id`, `wave_kind`, `reason_code`, `completed_slots`, `expected_slots`, `missing_slots[]`。

#### Slot / 阶段事件（agent + coordinator）

| Topic | Publisher | Consumer | 备注 |
|-------|-----------|----------|------|
| `plan.ready` / `plan.blocked` | plan-reviewer | task-planner / reporter | 沿用 |
| `exec.batch.ready` | task-planner | exec-coordinator | 含 `unit_count`, `plan_units_file` |
| `exec.unit.ready` | exec-coordinator（wave） | unit-executor | wave 事件 |
| `exec.unit.done` / `exec.unit.failed` | unit-executor | **Supervisor**（merge only） | **不**触发 integrator |
| `work.done` / `work.failed` | exec-integrator | review-coordinator / reporter | 合并+集成测试后 |
| `review.wave.ready` | review-coordinator | dimension-reviewer | wave；必填 `dimension`, `depth` |
| `review.dimension.done` | dimension-reviewer | **Supervisor**（merge only） | **不**直接触发 synthesizer（本 preset） |
| `review.complete` | review-synthesizer | fix-planner | |
| `fix.batch.ready` | fix-planner | fix-coordinator | |
| `fix.unit.ready` | fix-coordinator（wave） | fix-worker | |
| `fix.unit.done` / `fix.unit.failed` | fix-worker | **Supervisor**（merge only） | **不**触发 integrator |
| `fix.done` | fix-integrator | alignment | 合并+集成测试后 |
| `align.done` | alignment | reporter | |
| `report.done` | reporter | — | + `LOOP_COMPLETE` |

#### 协调事件 Payload Schema（`event_policy.schemas` 必填）

**`exec.wave.complete`**

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `wave_id` | string | ✓ | |
| `wave_kind` | string | ✓ | 固定 `exec` |
| `loop_id` | string | ✓ | |
| `expected_slots` | number | ✓ | |
| `completed_slots` | number | ✓ | |
| `failed_slots` | number | ✓ | 可为 0 |
| `plan_units_file` | string | ✓ | 回指拆解文件 |
| `worktree_paths` | array[string] | ✓ | 按 slot_index 升序 |

**`fix.wave.complete`** — 同构；`wave_kind=fix`；`fix_units_file` 替代 `plan_units_file`。

**`review.wave.complete`**

| 字段 | 类型 | 必填 | 说明 |
|------|------|------|------|
| `wave_id` | string | ✓ | |
| `wave_kind` | string | ✓ | 固定 `review` |
| `dimensions_received` | array[string] | ✓ | 6 维名称列表 |
| `merge_commit_sha` | string | ✓ | integrator 产出，review 所审代码版本 |

**`*.wave.failed`** — 共有：`wave_id`, `wave_kind`, `reason_code`, `expected_slots`, `completed_slots`, `missing_slots`；`reason_code` 枚举：`wave_timeout` \| `wave_cancelled` \| `incomplete_wave` \| `slot_failures_exceeded`。

**R-PST-1.** 上表所有 topic 须在 `presets/schemas/ce-executor-supervisor.yml` 的 `event_policy.schemas` 定义 `required_fields`；wave 派发事件须通过 `ralph wave emit` 预检（同 `2026-06-13-wave-dispatch-policy-gate`）。

**R-PST-2.** `topic_deny_rules` 单 owner；`*.wave.complete` / `*.wave.failed` 的 owner 为 **runtime supervisor 注入路径**（无 hat `publishes` 声明；schema 与 lint 须登记为 control/coordination topic）。`preset_lint` + BDD 全绿。

**R-PST-2b.** `event_loop.event_policy.business_topics`（或等价列表）**必须**包含：`exec.wave.complete`, `exec.wave.failed`, `review.wave.complete`, `review.wave.failed`, `fix.wave.complete`, `fix.wave.failed`。

**R-PST-3.** 同步交付：`presets/en/ce-executor-supervisor.yml`、`presets/schemas/ce-executor-supervisor.yml`、`presets/manifest.yml`、`crates/ralph-cli/src/presets.rs`、`presets/index.json`、`scripts/ralph-zsh-plugin.zsh`、`CLAUDE.md` / `AGENTS.md` preset 列表。

### Planner 拆解规则

**R-PLN-1.** `task-planner` 输出 `plan_units.json`（数组），每项至少含：`unit_id`, `title`, `files_hint[]`, `depends_on[]`, `verification`。

**R-PLN-2.** 有 `depends_on` 交叉或 `files_hint` 重叠的 units **不得**同一 wave 并发；exec-coordinator 须分 batch 或降级串行（同 wave 内仅无依赖、无文件重叠的 units）。

**R-PLN-3.** `fix-planner` 对 `fix_plan_file` 做同样拆解 → `fix_units.json`；规则同 R-PLN-2。

### Integrator 规则（worktree 合并 + 集成测试 — 预设编排核心）

并行 wave **结束之后**，preset **必须**经过独立的 **integrator hat** 做「代码合并 + 全量验证」，再进入下一阶段。integrator 在 **主 loop 工作区**（非 worker worktree）运行；review 审的是合并后的树；fix 阶段若再次并行，修完仍须 fix-integrator 再合并一次。

#### 子流程 F-EXEC-INTEGRATE（执行阶段集成）

| 步骤 | 执行者 | 动作 | 失败时 |
|------|--------|------|--------|
| 1 | Supervisor | 确认 exec wave 在 DB 中 `status=completed`；`waves.phase` 置 `integrate`；向 `events.jsonl` **注入 `exec.wave.complete`**（见 R-COORD-*） | 不可恢复 partial → 注入 `exec.wave.failed` → reporter |
| 2 | `exec-integrator` | **由 `exec.wave.complete` 触发激活**（hat iteration） | — |
| 3 | Supervisor | 按 `slot_index` 升序读取 `slot_resources`（`resource_status=active`）；路径来自 `exec.wave.complete.worktree_paths` | 缺 worktree 路径 → `work.failed` |
| 4 | `exec-integrator` | **合并代码**：将每个 worktree 的 commit 按序合入主工作区（v1：按 slot 升序 cherry-pick；见 OQ2） | 冲突 → `work.failed` `reason=integrate_merge_conflict`，payload 含 `conflict_files[]` |
| 5 | `exec-integrator` | 更新 DB：`slot_resources.resource_status=merged`，写入 `merge_commit_sha` | 记诊断，不 emit 成功 |
| 6 | `exec-integrator` | **跑集成测试**（主工作区）：执行 preset 声明的 `integration_test_command`（默认全 workspace `./scripts/run-tests.sh` 或 `cargo nextest run`，与 `ce-executor-pipeline` executor DoD 同级） | 失败 → `work.failed` `reason=integrate_test_failed`，附测试摘要 |
| 7 | `exec-integrator` | 测试通过后 emit **唯一** `work.done`（含 `merge_commit_sha`, `units_merged`, `test_evidence`） | — |
| 8 | Supervisor | 调度 worktree 清理（`remove_worktree`）；失败写诊断不阻塞 | — |
| 9 | 事件路由 | `review-coordinator` 仅在 `work.done` 后触发 | `work.failed` → `reporter` 逃逸（同 pipeline） |

#### 子流程 F-FIX-INTEGRATE（修复阶段集成）

与 F-EXEC-INTEGRATE **同构**，差异：

| 项 | 执行阶段 | 修复阶段 |
|----|----------|----------|
| 前置协调事件 | `exec.wave.complete` | `fix.wave.complete` |
| integrator hat | `exec-integrator` | `fix-integrator` |
| 成功事件 | `work.done` | `fix.done` |
| 下游 | `review-coordinator` | `alignment` |
| 输入拆解文件 | `plan_units.json` | `fix_units.json` |

**R-INT-1.** `exec-integrator` / `fix-integrator` **仅**由 `exec.wave.complete` / `fix.wave.complete` 触发（`triggers` 单消费者）。Supervisor 注入 complete 事件前，DB 中对应 wave 必须 `status=completed` 且 slot fan-in 满足 schema。

**R-INT-2.** **集成测试是 integrator 的硬门控**，不是可选项。命令由 preset `execution_contracts.integration_test` 或 hat `instructions` 声明；未声明时默认与 `ce-executor-pipeline` 一致：**全量测试绿** 才可 emit `work.done` / `fix.done`。

**R-INT-3.** git merge / cherry-pick **冲突**不得 emit 成功事件；必须 `work.failed` / 等效失败 topic + `reason=integrate_merge_conflict`。

**R-INT-4.** **集成测试失败**不得 emit `work.done` / `fix.done`；必须 `reason=integrate_test_failed`，并附 `stdout_tail` 或结构化 `failing_tests[]`（schema 在 preset 定义）。

**R-INT-5.** worker 级 `exec.unit.done` **不得**触发 review；仅 integrator 的 `work.done` 可触发 `review-coordinator`。preset `event_policy` 与 `topic_deny_rules` 须 enforce 该顺序。

**R-INT-6.** unit worker 在 worktree 内跑的 `verification`（`plan_units.json`  per-unit 命令）是 **slot 内自测**；integrator 跑的是 **合并后全量集成测试** — 两层门控，缺一不可。

**R-INT-7.** `work.done` / `fix.done` payload **必须**包含：`merge_commit_sha`, `worktrees_merged_count`, `integration_test_command`, `integration_test_passed: true`（及可选 `duration_secs`）。

#### Preset 编排中的 integrator 位置（事件链）

```
exec-coordinator
  → [wave] unit-executor × N  （各 worktree 并行）
  → Supervisor：DB fan-in + merge exec.unit.* → inject exec.wave.complete
  → exec-integrator             （triggers: exec.wave.complete；合并 + 全量测试）
  → work.done
  → review-coordinator
  → [wave] dimension-reviewer × 6
  → Supervisor：inject review.wave.complete
  → review-synthesizer          （triggers: review.wave.complete）
  …
fix-coordinator
  → [wave] fix-worker × M
  → Supervisor：inject fix.wave.complete
  → fix-integrator              （triggers: fix.wave.complete；合并 + 全量测试）
  → fix.done
  → alignment
```

**R-INT-8.** `exec-integrator` / `fix-integrator` / `review-synthesizer` 在 preset YAML 中 **必须**为独立 hat；integrator 的 `triggers` **分别且仅**为 `exec.wave.complete` / `fix.wave.complete`；synthesizer 为 `review.wave.complete`。**禁止**由 coordinator 或 worker 兼任合并与全量测试职责；**禁止** integrator 订阅 `*.unit.done`。

---

## Merge 到 events.jsonl 的顺序（铁律）

**R-MRG-1.** 顺序：`(1) DB 事务提交 slot=completed` → `(2) 写 worker_results` → `(3) 检查 fan-in` → `(4) 批量 append 主 events.jsonl（slot 级 `*.unit.done` / `review.dimension.done`）` → `(5) 标记 worker_results.merged_to_events=1` → `(6) Supervisor 注入 *.wave.complete`（协调事件，见 R-COORD-2）。

**R-MRG-1b.** `exec.wave.complete` / `fix.wave.complete` / `review.wave.complete` **不属于**步骤 (4) 的 worker batch；它们在 (6) 单独 inject，且 `source` 标记为 supervisor runtime。

**R-MRG-2.** 若 `(4)` 失败，DB 回滚 `(5)` 标记，wave 保持 `running` 并注入 `task.resume` 指向 coordinator / integrator。

**R-MRG-3.** `supervisor.enabled=false` 时保持现有 merge 逻辑，不读 DB。

---

## 实现分期（Milestone）

| 阶段 | 交付 | 验收 |
|------|------|------|
| **M1** | `SupervisorStore` + schema migration + 开关 + C/D/E | 单元测试；开关 off 无回归 |
| **M2** | 接入 wave dispatcher；review wave only BDD | 4 维并行 review 收敛 |
| **M3** | worktree 绑定 + exec wave + exec-integrator | 2 unit 并行写不同文件 |
| **M4** | B/A/F 全套 + cancel/backpressure/compensation | F1–F5 诊断场景 |
| **M5** | fix wave + fix-integrator | 2 fix item 并行 |
| **M6** | 完整 preset + schema + manifest + 文档 + 全量 BDD | `ralph run -H builtin:ce-executor-supervisor` 冒烟 |

---

## 成功标准（Success Criteria）

- **SC1.** `supervisor.enabled: false` 时，`cargo nextest run` 全量基线与现网一致。
- **SC2.** `supervisor.enabled: true` 时，keen-fern / zippy-sparrow 类失败模式不再出现：无重复 `work.done`、无取消后空跑、无 wave 丢状态。
- **SC3.** 进程 kill 重启后，未超时 wave 从 `supervisor.db` 恢复并可继续或安全降级。
- **SC4.** BDD：`ce-executor-supervisor` 最小 plan（2 units, 2 fix items, 6 review 维）事件链完整断言；**必须**断言 `exec.wave.complete` → `exec-integrator` → `work.done` 顺序及 `fix.wave.complete` → `fix-integrator` → `fix.done` 顺序。
- **SC5.** `preset_lint` + SSOT byte test + zsh 补全通过。
- **SC6.** `ralph diagnose` 可展示 active waves、queue depth、dedup 统计（至少读 DB 或镜像诊断事件）。

---

## 范围边界

### In Scope

- rusqlite `supervisor.db` 与 6 件套
- worktree 编排（exec / fix）
- `ce-executor-supervisor` preset 全链路
- `SupervisorStore` trait + 测试
- BDD scenarios under `crates/ralph-core/tests/scenarios/supervisor/`
- 更新 `crates/ralph-core/data/ralph-tools-wave.md`（若 wave CLI 行为有变）

### Out of Scope（v1）

- Turso / 远程 DB / 多 loop 共享一库
- worker 进程直连 DB 写
- `tasks.jsonl` / `events.jsonl` 全量迁 SQL
- backend 内置 Subagent 适配（仅预留 PID / cancel hook）
- 修改 `ce-executor-pipeline` / `ce-executor-serial` 默认行为
- 跨 step 预执行（仅当前 plan 批内并行）
- 自动依赖图推断（planner 文本解析 + 保守降级即可）

### Deferred

- 中文 preset `ce-executor-supervisor-zh.yml`
- review wave 可选 worktree 只读快照
- 全局 worker pool 跨 wave 共享反压

---

## 依赖与假设

- **D1.** 仓库为 git repo；`create_worktree` 可用。
- **D2.** 并行 unit 的文件范围由 planner 声明；声明不准时 integrator 测试失败是预期 backpressure。
- **D3.** `ralph wave emit` 预检已落地（`2026-06-13`）；本需求依赖其不回归。
- **D4.** isolated 模式下 hat 单 emit 规则不变。
- **A1.** operator 接受并行执行占用更多磁盘（每 slot 一 worktree）与 merge 冲突需人工介入的风险。

---

## 待规划阶段闭合的问题（Outstanding Questions）

- **OQ1.** `supervisor-db` feature 是否默认开启编译（推荐 **默认 off**，CI 两矩阵都跑）。
- **OQ2.** integrator v1 merge 算法：cherry-pick 顺序 vs `git merge` 临时分支（规划期 PoC 二选一）。
- **OQ3.** `plan_units.json` / `fix_units.json` 默认路径：`.ralph/agent/plan_units.json` 是否 OK。
- **OQ4.** `max_concurrent_workers` 与 hat `concurrency` 关系：取 **min(全局, hat.concurrency)**。

---

## 文档与下游同步清单（实现完成后）

1. `docs/plans/2026-07-03-001-feat-supervisor-rusqlite-parallel-preset-plan.md`（由本需求衍生）
2. `CLAUDE.md` / `AGENTS.md` builtin preset 列表
3. `.cursor/rules/multi-hat-isolation.mdc`（若新增 16+ hat 拓扑说明）
4. `crates/ralph-core/data/ralph-tools-wave.md` + `ralph-tools.md`
5. 将 `2026-06-18-supervisor-wave-protocol-upgrade-requirements.md` 文首加 `superseded_by` 指向本文（持久化章节）

---

## 附录 A：`plan_units.json` 示例

```json
{
  "plan_name": "my-plan",
  "units": [
    {
      "unit_id": "u1",
      "title": "Add User model and migration",
      "files_hint": ["src/models/user.rs", "migrations/001_users.sql"],
      "depends_on": [],
      "verification": "cargo nextest run -p app -- user_model"
    },
    {
      "unit_id": "u2",
      "title": "Add User API routes",
      "files_hint": ["src/routes/user.rs"],
      "depends_on": ["u1"],
      "verification": "cargo nextest run -p app -- user_api"
    }
  ]
}
```

## 附录 B：配置类型（Rust 草案）

```rust
/// crates/ralph-core/src/config/loop_config.rs
pub struct SupervisorConfig {
    /// 缺省 false
    pub enabled: bool,
    /// 缺省 ".ralph/supervisor.db"
    pub db_path: PathBuf,
    /// 缺省 16
    pub max_concurrent_workers: u32,
    /// 缺省 600
    pub aggregate_timeout_secs: u64,
}
```

## 附录 C：与旧 Supervisor 母舰文档的差异

| 条目 | 2026-06-18 母舰 | 本文 |
|------|-----------------|------|
| 持久化 | `.ralph/wave-state/*.json` | `.ralph/supervisor.db` |
| preset | 不改 preset | 交付 `ce-executor-supervisor` |
| worktree | 未规范 | exec/fix 强制绑定 |
| 开关 | 隐含 wave | 显式 `supervisor.enabled: true` |
| 存储后端 | 未指定 | rusqlite |

---

*文档版本：2026-07-03 v1.0 — 待用户确认后进入 plan 阶段。*
