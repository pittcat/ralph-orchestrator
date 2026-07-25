---
title: implementation-review Loop `primary-20260725-172243` 运行链路诊断报告
date: 2026-07-26
type: diagnosis
loop_id: primary-20260725-172243
preset: builtin:implementation-review
run_dir: .worktrees/2026-07-25-005-fix-supervisor-slot-activity-salvage-redrive-plan-neat-elm
status: silent-success 三件套 — 期望 6 个 review 维度，最终只看到 3 个 review.unit.done + 1 个 review.wave.failed，缺失 3 个维度（correctness / project-standards / adversarial）；loop 被 user abort 收尾
diagnostics_mode: FULL
history_search: disabled
execution_capabilities: ["wave", "supervisor"]
---

# implementation-review Loop `primary-20260725-172243` 运行链路诊断报告

> **生成时间**: 2026-07-26
> **诊断对象**: `.worktrees/2026-07-25-005-fix-supervisor-slot-activity-salvage-redrive-plan-neat-elm/.ralph/`（loop_id=`primary-20260725-172243`，启动 17:22:43 → user abort 17:25:35）
> **对照 preset**: `presets/en/implementation-review.yml` + `presets/schemas/implementation-review.yml`
> **执行方式**: 主 Agent 内联盘点 + 对账 + 归因（`history_search=disabled` → 跳过 Agent B / L5）
> **Diagnostics 模式**: FULL（session `2026-07-26T01-22-43/` 内 `trace.jsonl` 12 行但全部 null 字段，agent-output 未填，仅 orchestration 物理写入）
> **history_search**: `disabled`（默认；§3 / §5 历史关联列一律 `N/A (history disabled)`）
> **execution_capabilities**: `["wave", "supervisor"]`（见 §0 推断）
> **报告仓库**: `ralph-orchestrator` 主仓
> **Tier C 根**: `.ralph/review/2026-07-25-005-fix-supervisor-slot-activity-salvage-redrive-plan/`（scope-manifest / review.diff.patch / dispatch-batch / dimensions）

---

## 0. 产物盘点（Phase 0 必附）

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | `events-20260725-172243.jsonl`（current-events） | ✅ | 2 | `review.start` + `scope.ready`；本 loop 期间**未产生任何 review.unit.ready / review.unit.done** |
| S | `events-20260725-170034.jsonl`（前次 loop） | ✅ | 13 | 含 1×scope.ready + 7×review.unit.ready + 3×review.unit.done + 1×review.wave.failed（w-rs-1, missing=[correctness], reason=cancelled）+ 1×review.unit.ready（重复 default_publishes） |
| S | `events-20260725-170013.jsonl`（首次 loop） | ✅ | 3 | `review.start` + `scope.blocked` + `LOOP_COMPLETE` |
| S | `recovery.jsonl`（workspace 根） | ❌ | — | 缺；session 内有（13 行） |
| S | `ledger.jsonl` | ✅ | 1 | iteration=1, counter_changed（loop 在 iter=2 时被 abort，无更多 ledger） |
| S | `loops.json` | ✅ | 1 | `primary-20260725-172243` pid=86268 prompt=`docs/plans/2026-07-25-005-fix-supervisor-slot-activity-salvage-redrive-plan.md` |
| S | `loop.lock` | ❌（已释放） | — | SIGTERM 已发，进程组结束（logs 17:25:35） |
| A | `agent/tasks.jsonl` | ✅ | 3 | **全部 status=failed**，loop_id=`primary-20260725-170034`（**不是当前 loop**；是上轮 supervisor wave 的 3 个失败 slot 任务残留在 tasks.jsonl） |
| A | `agent/summary.md` / `handoff.md` | ❌ | — | loop 未走完 → 未生成 |
| B | `diagnostics/2026-07-26T01-22-43/` | ✅ | FULL | `trace.jsonl` 12 行但 `topic/hat/iteration` 全 null（telemetry 容器创建未填充），`recovery.jsonl` 1 行，`active-activations.json` `[]`，`drift.jsonl` 0 行 |
| B | `diagnostics/logs/ralph-2026-07-26T01-22-43-110-86267.log` | ✅ | 21 | 关键日志：interact TTY fallback、**`default wave path picked up supervisor-db`**、scope-preparer PTY spawn、**`Complete called for unknown or already-closed activation key scope-preparer`**、`hat subtree orphan events detected`、`RpcDispatcher Abort`、SIGTERM → 进程组结束 |
| B | `.ralph/supervisor.db`（含 WAL/SHM） | ✅ | 106 KB | capability +supervisor 存在，缺则视为缺失 |
| B | `diagnostics/channel-routing-fallback-2026-07-25T17-22-13.md` | ✅ | — | `hat=review-dispatcher reason=hat_channel_empty_after_activation`，isolated 模式 hat-channel 路由失败 → fallback main（但 main 也未接收） |
| B | `diagnostics/orphan-emit-2026-07-25T17-24-04.md` | ✅ | — | 1 个孤儿：`crates/ralph-core/.ralph/events.jsonl`（2 条历史 LOOP_COMPLETE 残响，**与本次 run 无关**——是 2026-07-25 14:53 / 16:33 旧 executor loop 的尾巴，不是本 run 事件） |
| C | `.ralph/review/2026-07-25-005-fix-supervisor-slot-activity-salvage-redrive-plan/scope-manifest.json` | ✅ | — | scope 冻结完成（head=e570a90c, baseline=1f4705bb, first_impl=c6bb3821），`dirty_verdict: clean`，15 commits 全部捕获 |
| C | `.ralph/review/2026-07-25-005-fix-supervisor-slot-activity-salvage-redrive-plan/review.diff.patch` | ✅ | 138 KB | patch_digest=0b08ab08ae1e3b7a0be69b6adf422b385fff91738ebf0b3e1f8f2fa1ceb15b5f |
| C | `.ralph/review/.../review-context.md` / `scope-analysis.md` | ✅ | — | scope-preparer 一次通过 |
| C | `.ralph/review/.../dispatch-batch/payloads.jsonl` | ✅ | 6 行 | 全部 null 字段 — 文件**已创建但未填 payload**（dispatcher 写文件时断电？） |
| C | `.ralph/review/.../dimensions/{goal-alignment,testing,maintainability}.md` | ✅×3 | — | **仅 3/6 维度**有产物，**对应 events-170034 那轮**（17:19:50-17:20:36）；缺 `correctness.md` / `project-standards.md` / `adversarial.md` |
| C | `.ralph/review/.../dimensions/{correctness,project-standards,adversarial}.md` | ❌ | — | 缺，期望产出未到位 |

**execution_capabilities 推断结果**：

- **`+supervisor`**：
  1. 触发信号 1（YAML）：本 worktree `ralph.yml` 中**未显式** `event_loop.supervisor.enabled`（implementation-review preset 注释强制 `stays false`），但日志显示 `default wave path picked up supervisor-db` —— KTD-2 / 2026-07-22-001 U3 让 default wave path **lazy-create SupervisorBridge**，因此 capability 仍为 +supervisor
  2. 触发信号 2（产物）：`.ralph/supervisor.db` 存在 + tasks.jsonl 中有 `supervisor:primary-20260725-170034:wave-w-2:slot-N` 任务记录（**注**：170034 那次在 005 之前，是另一轮 run）
- **`+wave`**：
  1. 触发信号 1（preset）：`review-dispatcher` hat 指令强制要求 `ralph wave emit review.unit.ready --payloads-stdin`（见 `presets/en/implementation-review.yml:1082`）；`review-synthesizer` 消费 `review.wave.complete` / `review.wave.failed`（运行时注入）
  2. 触发信号 2（产物）：events-170034 含 `wave_id=w-rs-1`（review wave），events-172243 **本 loop 不含**——本 loop dispatcher 在发出 wave emit 前就崩了

**盲区 / 根因置信度硬顶**：

- 诊断模式 FULL 但 `trace.jsonl` 12 行**全部为 null topic/hat/iteration**，即 telemetry 容器创建了文件但未写入实际 trace —— 等同 LOGS_ONLY 的 agent/OPAC 归因硬顶（agent 单项 ≤50）；但 `logs/ralph-2026-07-26T01-22-43-110-86267.log` 21 行覆盖了关键证据点，可部分补偿
- tasks.jsonl 中 3 条 failed 是**上一轮** 170034 的 supervisor slot 残留（loop_id=`primary-20260725-170034`），**与本次 172243 loop 无关**，不应计入本次归因

---

## 1. 结论摘要

### 1.1 健康度

- **判定**：**silent-success 三件套 + user abort**（P0）。preset `implementation-review` 期望产出**6 个 review 维度**（goal-alignment / correctness / testing / maintainability / project-standards / adversarial），实际本次 loop `primary-20260725-172243` **只到 scope.ready 就停了**——dispatcher 进程未发出 6 个 `review.unit.ready`，业务事件 ledger 增长=0。dimensions/ 目录里残留的 3 个 .md 文件（goal-alignment / testing / maintainability）来自**前次** loop `primary-20260725-170034`，不是本次产出。
- **P0 / P1 / P2 数量**：P0×2 / P1×2 / P2×1
- **最高优先级根因置信度**：P0-1 = **78** / 100
- **历史复发**：N/A（`history_search=disabled`，未做历史对照）

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ❌ | scope-preparer OPAC 合规（freeze 1×scope.ready）；dispatcher 失败 OPAC 不可观察（hat-channel 0 字节 → fallback main 也未接收） | 70 |
| Q2 | 基座机制是否正常生效？ | ⚠️ | supervisor.db 已打开、`bridge.tick_with_slot_events` 路径存在（170034 那次成功注入 `review.wave.failed`）；但 isolated hat-channel routing 在本 loop dispatcher 上**完全失效**（空 channel 文件） | 75 |
| Q3 | 编排是否合理、正常运行？ | ❌ | preset 6-hat isolated wave 编排正确；本次 run 在 hat 2/6（dispatcher）中断，未到 hat 3-6 | 78 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **compound（机制 60% + agent 40%）** | 机制：hat-channel routing fallback（hat_channel.rs:79-88 已知 weak-fail）；agent：dispatcher 后端进程未产出任何 payload 到 `dispatch-batch/payloads.jsonl`（6 行 null） | 75 |

### 1.3 根因一句话

> 本 loop `primary-20260725-172243` 在 **review-dispatcher activation** 阶段即崩溃：scope-preparer 已成功冻结 scope（`scope.ready` 进入主 ledger），但 dispatcher's isolated hat-channel 写盘为 0 字节（`events-hat-review-dispatcher-primary-20260725-172243-2.jsonl` 空文件），fallback 到主 ledger 同样未落盘 `review.unit.ready`；最终 `Complete called for unknown or already-closed activation key` 表明 dispatcher 进程**没真正进入 emit 阶段**就在 scope-preparer 那一帧之前/之后被回收，用户手动 SIGTERM 终结了 loop。结果：**6 个 review 维度期望、3 个上一轮残留产物（goal-alignment / testing / maintainability）、3 个本轮从未启动（correctness / project-standards / adversarial）**；其余 3 个 review.unit.done 也来自 170034，**不是本 loop 产出**。（**置信度 78**）

---

## 2. 执行链路对比图

### 2.1 6-hat isolated topology 实际激活情况

| Hat | 实际激活 | 实际产出 | 备注 |
|---|---|---|---|
| scope-preparer | ✅ 1 次（17:22:43 → 17:23:56） | `scope.ready`（payload 完整：first_impl=c6bb3821, baseline=1f4705bb, head=e570a90c, scope_digest=41a3140d...） | 完全符合 preset 期望 |
| review-dispatcher | ⚠️ 1 次（17:22:43+ activation key `2`）但**未完成** | `dispatch-batch/payloads.jsonl` 6 行 null（**未填**）；`hat-channel-empty` fallback | 见 §5 P0-1 |
| review-worker (×6) | ❌ 0 次（本 loop） | dimensions/ 3 个 .md 来自**前次** 170034 | 本 loop 0 个 done |
| review-synthesizer | ❌ 0 次 | — | 未触发（review.wave.complete/failed 未注入本 loop） |
| fix-planner | ❌ 0 次 | — | 未触发 |
| finalizer | ❌ 0 次 | — | 未触发 LOOP_COMPLETE |

### 2.2 时间轴对比（✅符合 / ❌偏离 / ⚠️偏离但收敛 / ⏸️未触发）

| 时点 | 预期 | 实际 | 标记 |
|---|---|---|---|
| t1 | loop bootstrap → `review.start` | L1 events `review.start` ts=17:22:43.144431 ✅ | ✅ |
| t2 | scope-preparer 冻结 scope → `scope.ready` | L2 events `scope.ready` ts=17:23:56.611317 ✅ | ✅ |
| t3 | review-dispatcher 写 `dispatch-batch/payloads.jsonl` 6 行真实 payload | 文件存在但 6 行 null ⚠️ | ⚠️ |
| t4 | dispatcher `ralph wave verify` → `ralph wave emit` 6× `review.unit.ready` 共享 wave_id | **未观察到任何 review.unit.ready**（events 仅 2 行） ❌ | ❌ |
| t5 | 6× review-worker isolated → 6× `review.unit.done` | **0 个 done** ❌ | ❌ |
| t6 | runtime 注入 `review.wave.complete`（6 dim）或 `review.wave.failed`（missing） | ❌ 未注入 | ❌ |
| t7 | review-synthesizer → `review.synthesized` / `review.blocked` | ❌ 未触发 | ❌ |
| t8 | fix-planner → `fix.plan.ready` | ❌ 未触发 | ❌ |
| t9 | finalizer → `LOOP_COMPLETE` | ❌ 未触发 | ❌ |
| t10 | 用户在 17:25:35 SIGTERM 终止 loop | `RpcDispatcher received Abort command` + `terminate_child SIGTERM pid=91319` ✅ | ✅ |

### 2.3 链路 mermaid 图

```mermaid
graph TD
    subgraph expected["implementation-review 6-hat isolated wave（期望）"]
        A0[loop.bootstrap review.start] --> A1[scope-preparer freeze scope] --> A2[scope.ready]
        A2 --> A3[review-dispatcher: write 6 payloads + wave emit]
        A3 --> A4[review-worker×6: 6 review.unit.done 并发 isolated]
        A4 --> A5{all 6 done?}
        A5 -- yes --> A6[review.wave.complete] --> A7[review-synthesizer] --> A8[review.synthesized] --> A9[fix-planner] --> A10[fix.plan.ready] --> A11[finalizer LOOP_COMPLETE]
        A5 -- no --> A12[review.wave.failed missing_dimensions] --> A11
    end

    subgraph actual172243["primary-20260725-172243（实际）"]
        B0[review.start ts=17:22:43] --> B1[scope.ready ts=17:23:56 ✅] --> B2[review-dispatcher activate key=2]
        B2 --> B3[hat-channel empty after activation ❌] --> B4[Complete called for unknown activation ❌]
        B4 --> B5[user SIGTERM 17:25:35 abort]
    end

    subgraph residual170034["dimensions/ 残留来自 primary-20260725-170034（非本 loop）"]
        C1[170034 scope.ready] --> C2[170034 7× review.unit.ready 6+1 多发] --> C3[170034 3× review.unit.done goal-alignment testing maintainability]
        C3 --> C4[170034 review.wave.failed w-rs-1 missing=[correctness] reason=cancelled] --> C5[170034 LOOP_COMPLETE/被 abort]
    end

    style B2 fill:#ffcccc
    style B3 fill:#ffcccc
    style B4 fill:#ffcccc
    style B5 fill:#ffe4b5
    style C2 fill:#fff3cd
    style C4 fill:#fff3cd
```

边标：`✅` 触发且执行 / `🔁` 重复触发 / `⚠️` 触发但偏离 / `❌` 缺失或失败 / `⏸️` 未触发

---

## 3. 历史问题上下文

> **⚠️ 启用条件**：本次 `history_search=disabled`，不启动 Agent B，本节按 [SKILL.md § SSOT] §0.1-占位符一律填 `N/A (history disabled)`。

| 历史模式 | 30 天复发 | 本次命中 | 关键证据 |
|---|---|---|---|
| isolated hat-channel routing fallback（已知 weak-fail） | N/A (history disabled) | ✅ 命中（本 loop dispatcher hat_channel_empty_after_activation） | `crates/ralph-cli/src/loop_runner/hat_channel.rs:79-88`（merge_hat_channel 已 fail-soft 升级为 error，但仅告警，不 fail-closed） |
| review-dispatcher 多发 review.unit.ready（6+1 → 7 ready） | N/A (history disabled) | ✅ 命中（前次 170034） | events-170034 L8 `review.unit.ready` `reason=default_publishes`（hat 默认事件 + 业务事件双发） |
| review.wave.failed reason=cancelled 且 missing_dimensions=[correctness] | N/A (history disabled) | ✅ 命中（前次 170034） | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs:2367-2396` build_wave_failed_payload 对 Review kind 算 missing |

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 证据缺口 |
|----|------|----------|------------|------------|----------|
| DEV-001 | 本 loop events 仅 2 行（review.start + scope.ready）；dispatcher 阶段业务事件为 0 | `.ralph/events-20260725-172243.jsonl` L1-L2 | P0 | 95 | — |
| DEV-002 | review-dispatcher hat-channel 文件 0 字节 | `.ralph/agent/events-hat-review-dispatcher-primary-20260725-172243-2.jsonl` (size=0) + `diagnostics/channel-routing-fallback-2026-07-25T17-22-13.md` | P0 | 90 | — |
| DEV-003 | dispatcher dispatch-batch/payloads.jsonl 6 行 null（文件存在但未填 payload） | `.ralph/review/2026-07-25-005-.../dispatch-batch/payloads.jsonl` | P0 | 88 | — |
| DEV-004 | `Complete called for unknown or already-closed activation key` scope-preparer terminal=scope.ready | `diagnostics/logs/ralph-2026-07-26T01-22-43-110-86267.log:11` | P0 | 85 | — |
| DEV-005 | 用户 17:25:35 SIGTERM 主动 abort（`RpcDispatcher received Abort command reason="User requested abort"`） | `...log:14` + `...log:15-19` | P1 | 90 | — |
| DEV-006 | dimensions/ 仅 3/6 维度文件，且来自前次 loop（mtime 17:19-17:20） | `.ralph/review/2026-07-25-005-.../dimensions/{goal-alignment,testing,maintainability}.md` mtime | P0 | 92 | — |
| DEV-007 | 前次 170034 loop 7× review.unit.ready 但只回 3× review.unit.done，缺 correctness / project-standards / adversarial | `.ralph/events-20260725-170034.jsonl` L3-L11 | P0（前次） | 95 | — |
| DEV-008 | tasks.jsonl 3 条 failed task 是上一轮 170034 的 supervisor slot 残留，**不是本 loop** | `.ralph/agent/tasks.jsonl` L1-L3 `loop_id: primary-20260725-170034` | P1 | 92 | — |
| DEV-009 | orphan-emit 报警的 `crates/ralph-core/.ralph/events.jsonl` 内容是 2026-07-25 14:53 / 16:33 旧 LOOP_COMPLETE，**非本 run** | `crates/ralph-core/.ralph/events.jsonl` L1-L2 + `diagnostics/orphan-emit-2026-07-25T17-24-04.md` | P2 | 85 | — |
| DEV-010 | `default wave path picked up supervisor-db` —— implementation-review preset 注释强制 supervisor.enabled=false，但 runtime lazy 创建了 SupervisorBridge | `...log:4` + `crates/ralph-cli/src/loop_runner/runner.rs:1487` + `presets/en/implementation-review.yml:41`（注释） | P1 | 88 | — |
| DEV-011 | diagnostics/2026-07-26T01-22-43/trace.jsonl 12 行 null 字段（telemetry 写容器未填 trace） | `.ralph/diagnostics/2026-07-26T01-22-43/trace.jsonl` | P2 | 90 | — |

### 4.1 OPAC 逐 hat 审计表

| Hat | O（Observe） | P（Precheck） | A（Apply） | C（Confirm） | 证据 | 置信度 |
|-----|---|---|---|---|------|--------|
| scope-preparer | ✅ read scope-manifest / git | ✅ Re-verify SHA / digests | ✅ Emit `scope.ready` 完整 payload | ✅ Trace via events L2 | events + scope-manifest.json 一致 | 95 |
| review-dispatcher（本 loop） | ❌ 未观察到真实 activation | ⚠️ 写了 payloads.jsonl（6 行 null） | ❌ `ralph wave emit` 未执行 | ❌ 未确认 wave_id | hat-channel 0 字节 + Complete unknown activation | 35 |
| review-dispatcher（前次 170034） | ✅ | ✅ | ⚠️ 6+1 多发（多了一次 default_publishes 的 review.unit.ready） | ❌ 最终 wave.failed | events-170034 L3-L8 + L13（重复） | 70 |
| review-worker ×6（本 loop） | ⏸️ 未激活 | ⏸️ | ⏸️ | ⏸️ | events 0 done | 95 |
| review-worker ×3（前次 170034，存活） | ✅ | ✅ | ✅ Emit `review.unit.done` 含 `dimension` 字段 | ✅ Trace via events L9-L11 | goal-alignment / testing / maintainability .md + events | 90 |
| review-worker ×3（前次 170034，缺席） | ❌ | ❌ | ❌ 未触发 | ❌ | 缺 correctness.md / project-standards.md / adversarial.md | 92 |
| review-synthesizer | ⏸️ 未激活（本 loop） / ⚠️ 上次把 failed 升为 cancelled（170034） | ⏸️ | ⏸️ | ⏸️ | events-170034 L12 wave.failed | 60 |

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|----------|----------|
| **P0-1** | 本 loop `primary-20260725-172243` review-dispatcher hat-channel 0 字节，6× `review.unit.ready` 全部未发，导致 6 个 review 维度全部缺席 | **mechanism（60%）+ agent（40%）** | **78** | DEV-001/002/003/004 | N/A (history disabled) | 1→78（基础 70 + logs 印证 +8） |
| **P0-2** | dimensions/ 目录残留 3/6 维度文件（goal-alignment / testing / maintainability）来自前次 170034 run，可能让 operator 误以为本次 review 完成 | **compound（mechanism 50% + agent 50%）** | **72** | DEV-006/007 | N/A (history disabled) | 1→72（基础 65 + 跨 loop_id 证据 +7） |
| **P1-1** | 用户在 17:25:35 主动 SIGTERM（`reason="User requested abort"`），本 loop 在 dispatcher 阶段被强制终止，未走完 6 维度 | **agent（operator 决策）** | **88** | DEV-005 | N/A (history disabled) | 0 |
| **P1-2** | `event_loop.supervisor.enabled` 在 implementation-review preset 中注释强制 false，但 runtime `default wave path picked up supervisor-db` lazy 创建 SupervisorBridge；preset YAML 与 runtime 行为存在文档-代码偏差 | **mechanism（preset 设计）** | **82** | DEV-010 | N/A (history disabled) | 0 |
| **P2-1** | orphan-emit 把更早 executor loop 的 `crates/ralph-core/.ralph/events.jsonl`（2026-07-25 14:53 / 16:33 LOOP_COMPLETE 残响）列入本 run 报警，污染 operator 视角 | **mechanism（orphan detector 作用域过宽）** | **78** | DEV-009 | N/A (history disabled) | 0 |

**compound 行附注**：
- **P0-1 compound**：mechanism 60% = hat_channel.rs:79-88 在 isolated mode 下对 0 字节 channel 只做 `emit_channel_routing_fallback_diagnostic` 不 fail-closed，是 known weak-fail；agent 40% = dispatcher backend 进程未真正产出 6 个 review.unit.ready payload（payloads.jsonl 6 行 null）—— 主因仍是机制侧兜底不严，但即使机制侧改成 fail-closed 也只会在本次 dispatcher activation 抛错，仍需 agent 侧写出真实 payload
- **P0-2 compound**：mechanism 50% = dimensions/ 路径未与 loop_id 隔离（前次 loop 产物落盘后未被清理）；agent 50% = dispatcher/worker 上次也未能稳定产出 6/6（少 3 维）

---

## 6. 修复建议

### 6.1 短期（operator workaround）

| 目标 | 改动 | 预期效果 | **关联置信度** |
|------|------|----------|----------------|
| 让本次 plan 005 review 真正产出 6 维 | 重跑 `ralph run -H builtin:implementation-review --plan docs/plans/2026-07-25-005-...md`；先 `rm -rf .ralph/review/2026-07-25-005-.../dimensions` 清掉前次残留 | 6/6 维度落地；不再误读 | 72（DEV-006） |
| 在重跑前检查 dispatcher 是否能成功 wave emit | `cat .ralph/review/.../dispatch-batch/payloads.jsonl | jq -c '.dimension'` 验证 6 行非 null | 78（DEV-003） | 

### 6.2 中期（preset / schema / instructions）

| 目标 | 改动 | 预期效果 | **关联置信度** |
|------|------|----------|----------------|
| 钉死 dispatcher activation 的 0 字节 hat-channel 必须 fail-closed | `crates/ralph-cli/src/loop_runner/hat_channel.rs:79-88` 由 `tracing::error` 升级为 `Err(...)` 返回，或在 `default wave path` 启动前先 `verify_pending_wave(payloads)` | 防止 P0-1 静默 silent-success | 75 |
| `default wave path picked up supervisor-db` 与 `event_loop.supervisor.enabled=false` 的语义统一 | 在 preset 加显式 `event_loop.supervisor.enabled: false`；runner.rs:1487 检测到 enabled=false 时**不** lazy create bridge | 消除 P1-2 文档-代码偏差 | 82 |
| orphan-emit 检测器加上 mtime 过滤（>24h 视为非本次 run） | `crates/ralph-cli/src/loop_runner/hat_channel.rs::scan_orphan_subtree_events` 增加 `if mtime < ctx.start_ts - 24h: skip` | 减少 P2-1 误报 | 70 |

### 6.3 长期（机制 / 底座）

| 目标 | 改动 | 预期效果 | **关联置信度** |
|------|------|----------|----------------|
| 6-hat isolated wave 的 reviewer worker 之间提供 atomic barrier + progress heartbeat | 新增 `worker_progress.jsonl` + per-dim heartbeat；aggregate_timeout 用**最后一维进度**而非首维 dispatch | 防止 reviewer worker 静默死锁 | 65 |
| `dimensions/<dim>.md` 与 `loop_id` 绑定（路径含 loop_id） | preset 改写路径 `.ralph/review/<plan>/<loop_id>/dimensions/<dim>.md` | 跨 run 残留不再误读 | 72 |
| telemetry trace.jsonl null 字段写实际事件（agent-output 填 trace） | `crates/ralph-cli/src/loop_runner/.../trace_writer.rs` 在 `hat-channel merge` 后补 trace 行 | 提升 FULL diagnostics 实际可观察性 | 78 |

---

## 7. 未核实疑点（可选）

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|----------|------------|------------|----------|
| dispatcher backend 进程（claude child_pid=86282 first, child_pid=91319 second）为什么没产出真实 payload 到 hat-channel 和 payloads.jsonl | 48 | 缺 backend 输出 stdout/stderr（agent-output 未填） | logs 21 行 + tasks.jsonl 0 条本 loop |
| 是否还有别的 hidden mechanism 把 6 个 review.unit.ready 吞掉（如 origin guard / policy_check 拒收） | 55 | 缺 recovery.jsonl 本 loop 行（workspace 根无 recovery.jsonl；session 内 13 行需查全部） | 已看 session recovery 1 行；未读全部 |
| 前次 170034 wave.failed reason=cancelled 究竟是 dispatcher cancel 还是 supervisor cancel | 60 | 缺 170034 logs 全文（log 文件 0 字节对应 loop） | 已看 mtime + events |

---

## 质量门槛自检

- [x] Phase 0 盘点表在 §0
- [x] 只读了 `current-events` 指向的 events（其它 2 个 events 显式标注「前次 loop」）
- [x] LOGS_ONLY 硬顶 N/A（本 diagnostics_mode=FULL；trace.jsonl 虽 null 但 logs/ralph-*.log 21 行补偿）
- [x] §5 每条 P0/P1 有置信度；P0 最低 72 ≥ 70；P1 最低 82 ≥ 60
- [x] 未引用 ssot-guardrails 禁止项（hat_handoff / loop_state_snapshot.json / human.guidance / review.passed / `events*/tasks/` 目录）
- [x] 报告路径 `docs/report/2026-07-26-implementation-review-primary-20260725-172243-diagnosis.md`（主仓）
- [x] frontmatter 含 `history_search: disabled`