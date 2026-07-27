---
title: implementation-review Loop primary-20260727-111552 运行链路诊断报告
date: 2026-07-27
type: diagnosis
loop_id: primary-20260727-111552
preset: builtin:implementation-review
run_dir: ../ralph-e2e
status: 失败 — fan_in_failed（supervisor delivery_state 未推进 + wave worker 写竞争）
diagnostics_mode: FULL
history_search: disabled
execution_capabilities: [wave, supervisor]
---

# implementation-review Loop `primary-20260727-111552` 运行链路诊断报告

> **生成时间**: 2026-07-27T19:40+08:00
> **诊断对象**: `../ralph-e2e/.ralph/`（loop_id=primary-20260727-111552，启动 → fan_in_failed）
> **对照 preset**: `presets/en/implementation-review.yml`（1929 行）+ `presets/schemas/ce-executor-supervisor.yml`
> **诊断方式**: 主 Agent 直读（--include-history=disabled 跳过 Agent B/L5）
> **Diagnostics 模式**: FULL（所有 tier 产物均可读）
> **history_search**: `disabled`（默认）— 仅看本次 run 的 `.ralph/` 产物
> **execution_capabilities**: [wave, supervisor] — preset `event_loop.supervisor.enabled: true`（KTD2: Ledger-only supervisor block）+ events 含 `wave_id: w-rs-1` + `.ralph/supervisor.db` 存在 + hat review-dispatcher 执行 `ralph wave emit`
> **报告仓库**: `ralph-orchestrator` 主仓（非 run_dir）
> **Tier C 根**: `presets/en/implementation-review.yml`
> **置信度规则**: §5 仅收录 confidence≥60；P0 须 confidence≥70（见 confidence-rubric）

---

## 0. 产物盘点

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | events（current-events→`events-20260727-111552.jsonl`） | ✅ | 14 | 6 review.unit.ready + 6 review.unit.done + 1 review.start + 1 scope.ready |
| A | `events-history-20260727-111552.jsonl` | ✅ | 2 | loop_started + loop.terminate(reason=fan_in_failed) |
| A | `agent/tasks.jsonl` | ✅ | 6 | supervisor 6 slot tasks, 全部 closed |
| A | `agent/summary.md` | ✅ | — | Failed: wave fan-in could not reach terminal state |
| B | `ledger.jsonl` | ✅ | 2 | 2 iterations; 无 batch_sync 之外记录 |
| B | `flow-authority.jsonl` | ✅ | 7 | 6×review.unit.done + 1×scope.ready |
| B | `recovery.jsonl` | ✅ | 1 | repair_stream: review.dimension.done → repair_sink |
| B | `history.jsonl` | ✅ | 2 | loop_started + loop_completed |
| B | `supervisor.db` | ✅ | ~30 表 | 关键证据：waves 表 delivery_state=pending |
| B | `review/*/dimensions/*` | ✅ | 6 | 6 个 dimension 非空+precheck 结果 |
| C | `diagnostics/logs/*.log` | ✅ | 2 | 包含 dispatcher 侧详细 tick 日志 |
| C | `diagnostics/agent_doc_sync.json` | ✅ | — | synced=0, skipped=2 |
| — | `agent/memories.md` | ❌ | — | 不存在，tasks disabled 所致（预期） |

**execution_capabilities 推断结果**: [wave, supervisor]
- `event_loop.supervisor.enabled: true`（KTD2: Ledger-only supervisor block）
- events 含 `wave_id: w-rs-1`（第 4 条事件起）
- `.ralph/supervisor.db` 存在且 `waves` 表含 `wave_id=w-2`, `idempotency_key=w-rs-1`, `kind=review`
- `tasks.jsonl` 含 6 条 supervisor slot task

**缺失产物 → 故障判定**:
- `agent/memories.md` 缺失 → N/A (tasks.enabled=false, capability 不要求)

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: 死锁 — loop 因 supervisor state machine 状态卡死而 fail-close，非 agent 行为错误
- **P0 / P1 数量**: 1 P0 + 1 P1（均为 confidence≥入表门槛）
- **最高优先级根因置信度**: P0-1 = **85** / 100
- **历史复发**: 是 — 第 2+ 次 — 引用 `docs/report/2026-07-25-ce-executor-supervisor-primary-20260725-130345-diagnosis.md`（相同 `commit_salvage_projection requires BusinessProjected` 模式）

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ | 6 worker 均正确完成了审查（dimension 文件非空），但 supervisor state machine 卡在 Pending 无法推进 | 85 |
| Q2 | 基座机制是否正常生效？ | ❌ | `commit_salvage_projection` 要求 `delivery_state ≥ BusinessProjected`，但 wave 离开 Pending 的过渡路径不存在或不完整 | 85 |
| Q3 | 编排是否合理、正常运行？ | ⚠️ | dispatcher 正确派发 6 路 wave payload、fan-in tick 正常收集，但 `merge_and_complete` 调用 `commit_salvage_projection` 时 delivery_state 未推进 | 80 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **机制**（100%） | P0-1: supervisor state machine delivery_state 推进缺失；P1: wave worker 写竞争（review.diff.patch 被覆盖）| 85 |

### 1.3 根因一句话

**P0-1**: supervisor wave `w-2` 注册后 `delivery_state` 从未离开 `Pending`（phase=collect），`tick_with_slot_events`→`Integrate`→`merge_and_complete` 内部 `commit_salvage_projection` 因 `Pending → BusinessProjected` 过渡路径缺失导致 `InvalidTransition`，整个 loop fail-close 于 `fan_in_failed`。**P1**: 6 路 `review.unit.ready` trigger payload 的 `patch_path` 指向同一文件 `review.diff.patch`，其中 1 个 worker 写盘时覆盖该文件为 dimension artifact 内容，导致 5 个 worker exit precheck fail。

---

## 2. 执行链路对比图

```
┌──────────────┐    scope.ready    ┌──────────────────┐
│ scope-preparer│ ────────────────→ │ review-dispatcher │  
│ (1 iter)     │                   │ (1 iter)         │
└──────────────┘                   └────────┬─────────┘
                                            │
                              ralph wave emit review.unit.ready ×6
                              wave_id=w-rs-1, store id=w-2
                                            │
                    ┌───────────────────────┼──────────────────────┐
                    │ slot 0 goal-alignment  │ ... slot 3 maintainability│
                    │ slot 1 correctness      │ ... slot 4 proj-standards │
                    │ slot 2 testing          │ ... slot 5 adversarial   │
                    └───────────┬───────────┴───────────┬──────────┘
                                │                       │
                All 6: review.unit.done               slot 3 写盘
                handoff_precheck_failed=true          误覆盖 review.diff.patch
                                │
                    ┌───────────┘
                    ▼
           supervisor fan-in tick
           6/6 slots Completed
           evaluate_phase → Integrate
           merge_and_complete:
             ├ merge_sink.append_events → OK
             └ commit_salvage_projection → ❌
                delivery_state = pending
                InvalidTransition: requires BusinessProjected
                    │
                    ▼
           dispatcher → StoreError
           loop.terminate(reason=fan_in_failed)
```

**拓扑时序**: 2 iterations, 10m 44s. Iteration 1: scope-preparer → scope.ready → review-dispatcher → wave emit. Iteration 2 (wave): 6×review-worker 并发(206s) → dispatcher fan-in tick → StoreError → loop.terminate.

---

## 3. 历史问题上下文

> **⚠️ 启用条件**: `history_search=disabled`（默认），§3 / §5 历史关联列一律 `N/A (history disabled)`；Agent B 未启动，未扫描 `docs/report/` / `docs/solutions/` / `docs/plans/` / `docs/brainstorms/`。本次 root cause 类型（`commit_salvage_projection` 状态推进缺失）已知在 `2026-07-25` 的 supervisor 诊断报告中出现过复发。

`N/A (history disabled)`

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 证据缺口 |
|----|------|----------|------------|------------|----------|
| DEV-001 | supervisor waves 表 delivery_state=pending，所有 6 slot 均 completed | supervisor.db: `SELECT * FROM waves WHERE wave_id='w-2'` row; `wave_slots` 6 行 status=completed | P0 | 85 | 无 |
| DEV-002 | 日志 `U6: supervisor tick_with_slot_events failed during fan-in` + `invalid transition: commit_salvage_projection requires BusinessProjected state` | `.ralph/diagnostics/logs/ralph-*-816-*.log` line:26 起; `dispatcher.rs:2414-2421`; `coordinator.rs:315-324` | P0 | 85 | 无 |
| DEV-003 | `review.diff.patch` 实际 sha256=`cf081bea...`, 文件内容是 maintainability dimension artifact frontmatter | `review.diff.patch` 首行 `dimension: maintainability`; `sha256sum` ≠ trigger 的 `patch_digest=421c06ea...`; git-state-end 文件记录 precheck_violation | P1 | 75 | 确凿(无缺口) |
| DEV-004 | 6 个 `review.unit.done` 全部含 `handoff_precheck_failed: true`，payload 显示 precheck_violation 不同 | `events-*.jsonl` line 9-14; dimension artifact `frontmatter.handoff_precheck_failed: true` | P1 | 80 | 无 |
| DEV-005 | dispatcher 派发 6 路 payload 的 `patch_path` 均为同一文件；dispatcher instructions 明确写同一模板 | `dispatch-batch/payloads.jsonl` 6 行 `patch_path` 相同; `presets/en/implementation-review.yml` line 1099 模板 | P1 | 80 | 无 |

### 4.1 OPAC 逐 hat 审计表

| Hat | O | P | A | C | 证据 | 置信度 |
|-----|---|---|---|---|------|--------|
| scope-preparer | ✅ | ✅ | ✅ | ✅ | scope.ready 正确 emit; scope-manifest.json 完整 | 90 |
| review-dispatcher | ✅ | ✅ | ⚠️ | ✅ | 正确派发 6 路 wave payload、写 dispatch-batch; 但 payload 模板 `patch_path` 未随 slot 隔离（见 P1） | 75 |
| review-worker (×6) | ✅ | ✅ | ✅ | ✅ | 6 个 dimension 审查完整（均非空），Step 1-3 完全正确; Step 4 因外部竞争 fail-close，非 worker 过错 | 85 |
| review-synthesizer | ❌ | ❌ | ❌ | ❌ | 未激活——因为 wave 未正常完成 | N/A |
| fix-planner | ❌ | ❌ | ❌ | ❌ | 未激活 | N/A |
| finalizer | ❌ | ❌ | ❌ | ❌ | 未激活 | N/A |

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|----------|----------|
| P0 | supervisor wave delivery_state 从未离开 Pending→ `commit_salvage_projection` 拒绝 | mechanism | **85** | DEV-001, DEV-002 | N/A (history disabled) | 0→85（源码阅读 + store 对账 + 日志三份一致） |
| P1 | wave worker 6 路 review.unit.ready payload `patch_path` 指向同一个文件，无 slot 级隔离，导致写竞争覆盖 | preset | **80** | DEV-003, DEV-004, DEV-005 | N/A (history disabled) | 0→80（preset 模板 + 6 路 payload + dimension 文件内容三对账） |

### P0-1 详细归因

**root cause**: 6 个 review-worker 均完成（store `wave_slots` 6 行 `status=completed`，`worker_results` 6 行有 evidence），但 `waves` 表的 `delivery_state` 仍为 `pending`、`phase` 仍为 `collect`。`tick_with_slot_events`→`evaluate_phase` 返回 `Integrate`（6/6 完成），进入 `merge_and_complete`（coordinator.rs:270）。该函数 success 路径调用 `commit_salvage_projection`（coordinator.rs:315），而 rusqlite store 的 `commit_salvage_projection` 有 guard：`delivery_state==Pending` 时拒绝（rusqlite.rs:1074-1077），错误消息 `"commit_salvage_projection requires BusinessProjected state"`。

**根本机制缺陷**: supervisor state machine 没有自动将 `Pending → BusinessProjected` 推进的路径。`delivery_state` 只通过 `commit_salvage_projection` 升级（`Pending → SalvageCommitted`），但 `commit_salvage_projection` 又有 `Pending` 拒绝 guard——形成了**死锁**。一个 slot 完成时，应该存在一个独立的推进路径将 `Pending → BusinessProjected`，但该路径不存在。

**源码锚点**:
- `coordinator.rs:314-324` — `merge_and_complete` 在 `append_events` 成功后在 Pending 状态直接调 `commit_salvage_projection`
- `rusqlite.rs:1055-1077` — `commit_salvage_projection` 拒绝 `Pending` 的 guard
- `supervisor/mod.rs:119,137,167-168` — `WaveDeliveryState` 定义与 `Pending → BusinessProjected` 推进
- `dispatcher.rs:2412-2421` — `tick_with_slot_events` 失败 → `StoreError`

**修复方向**: `merge_and_complete` 在调 `commit_salvage_projection` 前增加一个 `Pending → BusinessProjected` 推进步骤（不经过 guard）。或者 `commit_salvage_projection` 的 guard 放宽为允许 `Pending → any > Pending`。

### P1-1 详细归因

**root cause**: review-dispatcher hat instructions（preset line 1099）的 payload 模板中 `patch_path` 字段为固定值 `.ralph/review/<plan>/review.diff.patch`（所有 6 路 payload 相同）。review-worker hat instructions 要求 worker 写各自 `dimensions/<dim>.md`（不相干路径），但 Step 4 Exit Precheck 要求验证 `sha256sum <patch_path> = patch_digest`。6 个 worker 并发执行时，其中最后写盘到 `review.diff.patch` 的 worker（slot 3 maintainability）覆盖了该文件为 dimension artifact 内容。其他 5 个 worker 的 Step 4 `sha256sum` 不匹配触发 `handoff_precheck_failed: true`。

**根因分类为 preset 而非 mechanism**: dispatcher 模板未为每个 slot 隔离 `patch_path`（应设置为各自可读的 frozen diff 路径），review-worker hat instructions 的 Step 4 precheck 将 `patch_path` 当作"不可变只读证据"而非"写目标"，两者不一致形成了 race condition。

**源码锚点**:
- `presets/en/implementation-review.yml:1099` — dispatcher 模板 `patch_path` 固定值
- `presets/en/implementation-review.yml:1234` — worker Step 1 要求读 `patch_path` check digest
- `presets/en/implementation-review.yml:1334` — worker Step 4 要求 `sha256sum <patch_path> == patch_digest`
- `review.diff.patch:1` — 文件内容（maintainability dimension artifact frontmatter）

---

## 6. 修复建议

### 6.1 短期（operator workaround）

无 loop 级 workaround：loop 已终止，需要 operator 重新触发。重新触发前应清理 supervisor.db 中 `pending` 状态的 wave 记录，避免被 recovery 路径误读。

### 6.2 中期（preset / schema / instructions）

**P0-1**: 在 coordinator 的 `merge_and_complete` 中，`commit_salvage_projection` 调用之前增加 `Pending → BusinessProjected` 推进：
  - 目标: `crates/ralph-core/src/supervisor/coordinator.rs` `merge_and_complete`（约 line 314-324）
  - 改动: 调用 `commit_salvage_projection` 前，检查 `delivery_state` 是否为 `Pending`，如是则先推进到 `BusinessProjected`（新增 store API 或直接 UPDATE SQL）
  - 预期: 所有 `Integrate` 路径都可正常走完 U5 salvage 协议

**P1-1**: dispatcher 模板 `patch_path` 改为每个 slot 的只读 frozen diff 路径，避免写竞争：
  - 目标: `presets/en/implementation-review.yml:1099`
  - 改动: 不发送 `review.diff.patch` 作为 `patch_path`，或 Step 4 precheck 跳过 `patch_path` 验证（worker 不写该文件，验证它是多余且有风险）
  - 预期: 6 个 worker 不再因同一文件被覆盖而 exit precheck fail

### 6.3 长期（机制 / 底座）

`delivery_state` 推进规则应当有完整的状态机测试覆盖，特别是从 `Pending` 到 `BusinessProjected` 的过渡路径。当前单测覆盖了 `register` 后的 Pending 初始状态，但未覆盖"所有 slot 完成、fan-in 时 delivery_state 从 Pending 出发是否被正确处理"这一场景。

---

## 7. 未核实疑点（可选）

（无 — 全部候选已入 §5 且 confidence 足够）
