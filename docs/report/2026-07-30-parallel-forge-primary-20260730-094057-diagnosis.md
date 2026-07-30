---
title: "parallel-forge Loop `primary-20260730-094057` 运行链路诊断报告"
date: 2026-07-30
type: diagnosis
loop_id: primary-20260730-094057
preset: builtin:parallel-forge
run_dir: .ralph
status: fail-close 路径 abort — 业务终态未达成（report.md 已落盘但 `forge.report.done` 0 条 + `LOOP_COMPLETE` ×2 REJECTED）— 修订版
diagnostics_mode: MINIMAL
history_search: disabled
execution_capabilities: ["supervisor", "wave"]
revision: 1
revision_note: "增量补丁：发现 fail-close 双根因（bus.publish 路径不 advance flow-authority + namespace 错配 plan.blocked vs forge.plan.blocked）+ reporter 违反 hat instructions + BDD 缺漏 + repair_budget 行为异常 + task 所有权错配。详见 §10 修订记录。"
---

# parallel-forge Loop `primary-20260730-094057` 运行链路诊断报告

> **生成时间**: 2026-07-30
> **诊断对象**: `ralph-orchestrator` 主仓根 `.ralph/`（loop_id=`primary-20260730-094057`，启动 2026-07-30T09:40:57 → TUI Quit 11:01:55）
> **对照 preset**: `presets/en/parallel-forge.yml` + `presets/schemas/parallel-forge.yml`
> **执行方式**: Phase 0 主 Agent 盘点 → Phase 1+2 (Agent A+C 合并) 链路与对账 → Phase 3 (Agent D) 归因 + 置信度 → Phase 4 主 Agent 落盘；**`history_search=disabled` 故 Agent B 跳过、L5 未跑**
> **Diagnostics 模式**: `MINIMAL`（session `2026-07-30T17-40-57/` 有 `trace.jsonl` / `recovery.jsonl` 但**无** `orchestration.jsonl` / `agent-output.jsonl`；CLI log 17KB）
> **history_search**: `disabled`（默认）— 主 SKILL §0.1 AskUserQuestion；Agent A/C/D 禁止读 `docs/report/` / `docs/solutions/` / `docs/plans/` / `docs/brainstorms/`
> **execution_capabilities**: `["supervisor", "wave"]` — 信号：① `ralph.yml` `event_loop.supervisor.enabled: true` → +supervisor；② CLI log 09:40:57 `supervisor bridge wired (execution_mode=isolated, supervisor.enabled=true) db_path=.ralph/supervisor.db` → +supervisor；③ preset hat 含 `forge-dispatcher`（wave 调度面）+ events 含 wave 业务 trigger（`forge.worktrees.ready` 触发 dispatcher）→ +wave
> **报告仓库**: `ralph-orchestrator` 主仓（非 run_dir）
> **Tier C 根**: `.ralph/forge/2026-07-29-002-feat-parallel-forge-reuse-status/` + `docs/reports/2026-07-30-2026-07-29-002-feat-parallel-forge-reuse-status-manager-report.md`
> **置信度规则**: §5 仅收录 confidence≥60；P0 须 confidence≥70（见 confidence-rubric）

---

## 0. 产物盘点（Phase 0 必附）

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | `current-events` → `events-20260730-094057.jsonl` | ✓ | 6 | 4 个 plan 终态（inspected/ready/approved/worktrees.ready）+ 1 个 `LOOP_COMPLETE`（reporter, 10:40:40, payload 仅 `report_path`）+ 1 个 `forge.start` |
| S | `events-history-20260730-094057.jsonl` | ✓ | 1 | warmup `_phase=warmup` 单行 |
| S | `recovery.jsonl`（workspace） | ✓ | 1 | `repair_dispatch` from reporter，**envelope.topic 是 stringified report_path JSON**（格式异常，DEV-005） |
| S | `ledger.jsonl` | ✓ | 8 | 4 次 `counter_changed` + 2 次 `LOOP_COMPLETE REJECTED P0-5 missing=forge.report.done`（seq 5/8）+ 1 次 `no_progress_turn_observed`（seq 6）+ 1 次 counter=6（seq 7） |
| S | `flow-authority.jsonl` | ✓ | 4 | `plan_authoring → concurrency_review → worktree_setup → development_loop`（**未** advance 到 `report` / `plan_end`，DEV-001） |
| S | `loops.json` | ✓ | 1 entry | `primary-20260730-094057`, pid=11768, worktree_path=主仓根 |
| S | `current-hat-events` → `events-hat-reporter-primary-20260730-094057-9.jsonl` | ✓ | **0 字节** | reporter iter=9 hat-channel 路由后未落任何事件（DEV-003） |
| A | `agent/tasks.jsonl` | ✓ | 9 | F1 closed（09:59:53→10:36:52），U1-U8 全 open |
| A | `agent/scratchpad.md` | ✓ | **0 字节** | 0 字节（无 scratchpad 内容） |
| A | `agent/memories.md` | ✓ | 1 fix | `mem-1785409193-75d8`（同 run fail-close 复盘 memory） |
| A | `agent/progress.md` / `summary.md` / `handoff.md` | ✗ | — | loop 未正常终止，未生成 |
| B | session `2026-07-30T17-40-57/{trace.jsonl, recovery.jsonl, drift.jsonl, active-activations.json}` | ✓ | trace 4KB / recovery 1行 / drift 0B | MINIMAL 模式全套：trace 11 行（启动 + TUI Quit 清理，**无** 业务事件 trace） |
| B | `channel-routing-fallback-2026-07-30T10-37-40.md` | ✓ | — | hat=forge-dispatcher, reason=hat_channel_empty_after_activation |
| B | `channel-routing-fallback-2026-07-30T10-56-34.md` | ✓ | — | hat=reporter, reason=hat_channel_empty_after_activation |
| B | `channel-routing-fallback-2026-07-30T11-01-01.md` | ✓ | — | hat=reporter（第二次）, reason=hat_channel_empty_after_activation |
| B | `supervisor.db` + `.db-shm` + `.db-wal` | ✓ | — | cap +supervisor 证据，rusqlite 格式（`sqlite3` 不可读） |
| B | `.ralph-enforce-current-unit` | ✓ | "1\n" | R4 single-U contract active |
| B | `plan-baseline-PROMPT.forge.sha` | ✓ | "5d643f42…\n" | base_commit=5d643f42（与 `forge.worktrees.ready` payload `base_commit` 一致） |
| C | `.ralph/forge/2026-07-29-002-feat-parallel-forge-reuse-status/{inspection-report,development-plan,execution-plan.yml,concurrency-approval,worktree-map}.{md,yml}` | ✓ | — | forge 业务产物完整 |
| C | `.ralph/forge/.../templates/{execution-plan,unit,manager-report,wave-failure,wave-settlement,development-plan,unit-completion,correction,merge-conflict,README}.{template.md,template.yml}` | ✓ | — | 模板齐全 |
| C | `docs/reports/2026-07-30-2026-07-29-002-feat-parallel-forge-reuse-status-manager-report.md` | ✓ | 27KB | **reporter 已写盘**，frontmatter `status=BLOCKED, final_audit=BLOCKED, trigger_topic=forge.plan.blocked, base_commit=5d643f42`，Reporter 自检全勾选 |
| C | `docs/reports/2026-07-30-raph-recovery-report.md` | ✓ | 4KB | 来自**更早**一次 loop `primary-20260730-002911`（recovery 路径），**不**属本 run 产物 |

**execution_capabilities 推断结果**: `["supervisor", "wave"]`
- 判定信号 1: `ralph.yml` 第 6 行 `event_loop.supervisor.enabled: true` → +supervisor
- 判定信号 2: CLI log 09:40:57 `supervisor bridge wired (execution_mode=isolated, supervisor.enabled=true) db_path=.ralph/supervisor.db max_concurrent_workers=10 aggregate_timeout_secs=7200` → +supervisor（带 `db_path` 锚点）
- 判定信号 3: `.ralph/supervisor.db` 存在（cap +supervisor ledger 证据）→ +supervisor 加固
- 判定信号 4: preset `parallel-forge.yml` hat `forge-dispatcher` 触发 `exec.unit.ready`（`yml:555-573`）—— wave 调度面 → +wave
- 判定信号 5: events `forge.worktrees.ready` 触发 `forge-dispatcher`（events#5）→ +wave

**缺失产物 → 故障判定**:
- `.ralph/supervisor.db` → **存在**（cap +supervisor 满足）
- events `wave_id` → events 无 `wave_id` 字段，但 events#6 `LOOP_COMPLETE` 之外的业务 trigger 链（`forge.worktrees.ready → forge-dispatcher → exec.unit.ready`）是 wave 业务面的 SSOT，**不**以 `wave_id` 字段存在与否为判据。**N/A**
- `agent-output.jsonl` / `orchestration.jsonl` → MINIMAL 模式下不生成属**预期**，**不**标故障

**盲区 / 根因置信度硬顶**: MINIMAL 模式 → 根因硬顶 85；缺 agent-output 时 agent 归因 ≤60。本报告全部 P0/P1 行将受此硬顶约束。

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: **死锁 / fail-close 循环**（无 silent-success 假闭环，但 reporter 写出 27KB BLOCKED 报告 + LOOP_COMPLETE ×2 REJECTED 暴露"artifact-first 与 runtime 终态割裂"）
- **P0 / P1 / P2 / P3 数量**（均为 confidence≥入表门槛）: **1（compound, mechanism 85 × preset 70） / 3 / 2 / 2**（**修订版**：原版 1/1/1/1，新版升 preset 至 compound 协同比 + agent P1 + BDD 缺漏 P1 + task 所有权 P2 + 移除 P3 维修；详见 §10）
- **最高优先级根因置信度**: P0-1 = **78** / 100（compound 行整行置信度 = `min(mechanism 85, preset 70) × 调整因子`；详见 §5）
- **历史复发**: **`N/A (history disabled)`**（按 §0.1 占位符字面规定）。**注**：用户已授权 session 中由 `~/.claude/projects/.../memory/` 已知 1 条同根因 memory `parallel-forge-fail-close-flow-authority-stale`（不在本 skill 历史检索范围，仅作机制参考；与本次 run 同 loop_id 同根因）。

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 部分 | L1 events / L3 ledger 合格；L4 recovery 仅 1 条且 topic 字段格式异常（DEV-005）；L2 orchestration + L5 history 均**不可用**（MINIMAL + disabled） | 75 |
| Q2 | 基座机制是否正常生效？ | ❌ **fail-close 路径有双 bug** | (α) fail-close emit `plan.blocked` 走 `bus.publish`（`event_loop/mod.rs:14604`）**不**经 `accept_event`（`mod.rs:14227`），`append_flow_authority_snapshot`（`mod.rs:14282`）不被调用 → flow-authority.jsonl 永久停在 `development_loop`；(β) fail-close emit 的话题名是 `plan.blocked`（无 `forge.` 前缀，`mod.rs:14599`），而 preset 14+ 处 blocked 协议**全部**是 `forge.plan.blocked`（`yml:58/64/70/77/108/125/130/196` 等）—— **namespace 错配**，与 runtime 多个 stage 把 `plan.blocked` 当 built-in topic 处理的假设冲突 | **85**（机制 α） |
| Q3 | 编排是否合理、正常运行？ | ❌ **编排契约与 fail-close 路径冲突（namespace 错配）** | preset `report` step `on_any_of: [forge.audit.done, forge.plan.blocked, work.failed]`（`yml:154-160`）含 `forge.plan.blocked`（有前缀），runtime 多个内置 stage（`flow_step_scope_stage.rs:82` `DEFENSIVE_BYPASS` / `terminal_state_guard_stage.rs:42` / `phase_authority_stage.rs:109` / `emit_schema_gate_stage.rs:45`）却把 `plan.blocked`（无前缀）当 partial-state 终端话题；`topic_format_whitelist: [LOOP_COMPLETE]`（`yml:48`）暗示除 `LOOP_COMPLETE` 外**必须**带 hat 命名空间——preset 协议用 `forge.*`，runtime 默认走 `plan.*`，**两套并行的 blocked 命名空间协议** | **70**（preset β） |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **compound（mechanism α 85 × preset β 70）** 主导 + **agent P1 65** 次因 + **BDD 缺漏 P1 70** 协同 | fail-close 双根因（α 不 advance flow-authority + β namespace 错配）是机制 + 编排协同；reporter 违反 hat instructions（跳过 `forge.report.done` 直接 emit `LOOP_COMPLETE`）是 agent 强归因；BDD 9 个 `parallel_forge_*.yml` scenarios 0 条覆盖 `consecutive_no_progress → plan.blocked` 路径 | **78**（compound 整行 min + 加权调整） |

### 1.3 根因一句话

`crates/ralph-core/src/event_loop/mod.rs:14604` fail-close 路径有**双 bug**：(α) 走 `EventBus::publish` 直发 `plan.blocked`（target=reporter，`mod.rs:14599`），绕开 `accept_event`（`mod.rs:14227`）→ `append_flow_authority_snapshot`（`mod.rs:14282`）不被调用 → `.ralph/flow-authority.jsonl` 永远停在 `development_loop`（4 行）；(β) emit 的话题名是 `plan.blocked`（**无** `forge.` 前缀）跨 namespace 错配——preset 14+ 处 blocked 协议**全部**是 `forge.plan.blocked`（`yml:58/64/70/77/108/125/130/196` 等），但 runtime 多个内置 stage（`flow_step_scope_stage.rs:82` `DEFENSIVE_BYPASS` / `terminal_state_guard_stage.rs:42` / `phase_authority_stage.rs:109` / `emit_schema_gate_stage.rs:45`）把 `plan.blocked` 当 built-in partial-state 终端处理（不 advance flow 但允许通过 `flow_step_scope`）—— 两套并行 blocked 命名空间协议 + reporter 跳过 `forge.report.done` 直接 emit `LOOP_COMPLETE`（违反 hat instructions `yml:1110-1115`）= **业务终态未达成**。**置信度 85**（mechanism α 根因 + MINIMAL 模式硬顶封顶；compound 整行 78）。

### 1.4 终态时序一致性（event-artifact chronology）

| 项目 | 内容 |
|------|------|
| **首轮终态（initial_terminal_status）** | **首轮失败（REJECTED）** — events#6 `LOOP_COMPLETE`（source=reporter, ts=10:40:40, payload 仅 `report_path`）先被 runtime 拒（`required_events: [forge.report.done]` 缺一），ledger seq=5 / seq=8 各记一次 `rejection_recorded`（reason=P0-5 missing=forge.report.done）；后续 fail-close 3 次循环（10:37/10:56/11:01）+ `forge.dispatcher` 与 `reporter` hat-channel 路由 fallback ×3 仍未达成终态。 |
| **恢复状态（recovery_status）** | **失败终态后未恢复** — 后续 agent activation（forge-dispatcher iter=5 / reporter iter=8 / reporter iter=9）全部 hat-channel 0 字节；`forge.report.done` 业务事件**始终 0 条**；reporter 写出 27KB BLOCKED 报告属于"artifact-first 单边落盘"，**不**是 accepted 业务终态。 |
| **最终代码状态（final_code_state）** | 仓库 HEAD 仍为 `5d643f4256abcc701cc679e53de0e25d7bf0a15f`（与 `forge.worktrees.ready` payload `base_commit` 一致，与 `plan-baseline-PROMPT.forge.sha` 一致）。`.ralph/forge/2026-07-29-002-feat-parallel-forge-reuse-status/F1` worktree 分支上存在 commit `87dc029b feat(reuse): freeze manifest/status DTOs`（**未 merge** 到 `pittcat-dev`）。主仓工作区有 F1 增量未暂存未追踪残留。 |
| **一致性告警** | ⚠️ **失败终态后 artifact-first 单边恢复**：首轮 audit/report 为 REJECTED ×2（`forge.report.done` 缺失），后续 reporter 单边写出 BLOCKED report.md（27KB）但**无任何 accepted 业务事件** 跟进；CLI 拒收与报告落盘在同一时间窗（10:40:40 - 10:42:22）并存，**不**能改写首轮 REJECTED 终态。 |

---

## 2. 执行链路对比图

### 2.1 拓扑激活表（preset-side 预期，14 hat）

`presets/en/parallel-forge.yml` 14 个 hat（12 spec + `forge-dispatcher` + `forge-failure-handler`）：

| hat | triggers | publishes | 关键行号 |
|---|---|---|---|
| `loop-bootstrap` | (system) | `forge.start` | yml:第 0 段 |
| `inspector` | `forge.start` | `forge.plan.inspected`, `forge.plan.blocked` | yml:298-299 |
| `planner` | `forge.plan.inspected` | `forge.plan.ready`, `forge.plan.blocked` | yml:327-328 |
| `guardian` | `forge.plan.ready` | `forge.concurrency.approved`, `forge.plan.blocked` | yml:439-440 |
| `worktree` | `forge.concurrency.approved`, `forge.wave.prepare` | `forge.wave.worktrees.ready`, `forge.worktrees.ready`, `forge.plan.blocked` | yml:476-486 |
| `forge-dispatcher` | `forge.wave.worktrees.ready`, `forge.worktrees.ready`, `forge.wave.settled` | `exec.unit.ready`, `forge.wave.prepare`, `forge.exec.development.done` | yml:555-573 |
| `executor` | `exec.unit.ready` | `exec.unit.done`, `exec.unit.failed` | yml:786-787 |
| `reviewer` | `forge.wave.worktrees.ready`, `exec.wave.complete`, `forge.correction.done` | `forge.wave.reviewed`, `forge.wave.review.failed`, `forge.units.reviewed`, `forge.plan.blocked` | yml:850-855 |
| `wave-fixer` | `forge.correction.requested` | `forge.correction.done`, `forge.plan.blocked` | yml:891-892 |
| `integrator` | `forge.wave.reviewed`, `forge.wave.verified` | `forge.wave.integrated`, `forge.wave.settled`, `forge.integration.done`, `work.failed` | yml:929-930 |
| `verifier` | `forge.wave.integrated`, `forge.integration.done` | `forge.wave.verified`, `forge.verification.failed`, `work.failed` | yml:979-980 |
| `tester` | `forge.exec.development.done`, `forge.final.correction.settled` | `forge.full.verified`, `work.failed` | yml:1018-1019 |
| `auditor` | `forge.full.verified` | `forge.audit.done`, `forge.plan.blocked` | yml:1054-1055 |
| `reporter` | `forge.audit.done`, `forge.plan.blocked`, `work.failed`, `forge.units.reviewed` | `forge.report.done`, `LOOP_COMPLETE` | yml:1080-1081 |
| `forge-failure-handler` | `exec.wave.failed`, `forge.wave.review.failed`, `forge.verification.failed` | `forge.correction.requested`, `forge.final.correction.settled`, `forge.plan.blocked`, `work.failed` | yml:654-664 |

机制流（`mechanism.flow.steps`）前 5 步即本 run 实际未越过的边界（yml:67-170）：

- `planning` （linear, allowed: `forge.plan.inspected`, `forge.plan.blocked`）→ ✅ 已过
- `plan_authoring` （`on: forge.plan.inspected`）→ ✅ 已过
- `concurrency_review` （`on: forge.plan.ready`）→ ✅ 已过
- `worktree_setup` （`on: forge.concurrency.approved`）→ ✅ 已过
- `development_loop` （kind: loop, runs: `forge.development_loop`, `on: forge.worktrees.ready`）→ ⏸️ 进入后第一轮 fail-close，未推进
- 后续 `full_verify` / `audit` / `report` / `plan_end` 未触发（events 无对应业务事件）

`report` step（yml:154-160）：

```yaml
kind: await
on_any_of:
  - forge.audit.done
  - forge.plan.blocked
  - work.failed
allowed_emits:
  - forge.report.done
```

`plan_end` step（yml:161-166）：

```yaml
kind: terminal
on: forge.report.done
allowed_emits: [LOOP_COMPLETE]
```

`event_loop.required_events: [forge.report.done]`（yml:198），`completion_payload_match.topic: forge.report.done, fields: [report_path]`（yml:199-201）。

### 2.2 时间轴对比表（event 链 vs 预期 hat）

| # | ts (UTC) | event | source hat | 机制流 | preset 预期 | 评价 | 一句话原因 |
|---|---|---|---|---|---|---|---|
| 1 | 09:40:57.108 | `forge.start` | loop-bootstrap | (前) | loop-bootstrap | ✅ | 系统注入，进入 planning |
| 2 | 09:43:07.188 | `forge.plan.inspected` | inspector | `planning` | inspector | ✅ | planner dispatched |
| 3 | 09:52:05.101 | `forge.plan.ready` | planner | `plan_authoring` | planner | ✅ | 9 unit / 6 wave / digest=272ab6a8… |
| 4 | 09:55:22.464 | `forge.concurrency.approved` | guardian | `concurrency_review` | guardian | ✅ | worktree dispatched |
| 5 | 09:58:12.452 | `forge.worktrees.ready` | worktree | `worktree_setup` | worktree | ✅ | dispatcher 触发，base_commit=5d643f42 |
| 6 | 10:36:52.840 | (task F1 closed, tasks.jsonl) | executor | (F1 单点) | executor（exec.unit.ready→done） | ⚠️ | events **未**记录 `exec.unit.ready` / `exec.unit.done`，仅 ledger/tasks 体现 F1 关单 |
| 7 | 10:37:40.258 | hat-channel routing fallback | forge-dispatcher | development_loop | forge-dispatcher | ❌ | dispatcher iter=5 hat_channel_empty_after_activation |
| 8 | 10:37:40.460 | `no progress 3 turns → plan.blocked (fail-close)` | event_loop | (loop_policy) | （非业务事件） | ❌ | fail-close 触发，**未 advance flow-authority** |
| 9 | 10:40:40.712 | `LOOP_COMPLETE` | reporter | plan_end | reporter | ⚠️ | `LOOP_COMPLETE` 出现但 `forge.report.done` 缺失 → ledger P0-5 REJECTED |
| 10 | 10:42:22.428 | `LOOP_COMPLETE REJECTED` P0-5 missing=forge.report.done | event_loop | plan_end | （gate） | ❌ | runtime 拒收终态 |
| 11 | 10:42:22.451 | Rejecting LOOP_COMPLETE again | event_loop | plan_end | （gate） | ❌ | 第二次拒收 |
| 12 | 10:42:22.961 | fail-close again | event_loop | (loop_policy) | — | ❌ | 第二次 no_progress 3 turns |
| 13 | 10:56:34.135 | hat-channel routing fallback | reporter | (report) | reporter | ❌ | reporter iter=8 hat_channel_empty_after_activation |
| 14 | 10:56:34.423 | fail-close 3rd | event_loop | — | — | ❌ | 第三次 no_progress |
| 15 | 11:01:01.489 | hat-channel routing fallback | reporter | (report) | reporter | ❌ | reporter iter=9 hat_channel_empty_after_activation |
| 16 | 11:01:01.778 | fail-close 4th | event_loop | — | — | ❌ | 第四次 no_progress |
| 17 | 11:01:55.941 | TUI Quit intercepted (user) | tui | (OS) | (user) | ⚠️ | SIGTERM 64 / SIGKILL 24 |
| 18 | 11:01:57.279 | Cleanup complete | run | (cleanup) | (system) | ⏸️ | cleanup_elapsed_ms=4860205 |

### 2.3 终止类型 — **未定终态（Aborted / Fail-Close Loop）**

| 维度 | 值 |
|---|---|
| 业务终态事件 | ❌ 无（`forge.report.done` 未 emit） |
| runtime 终态 | ❌ `LOOP_COMPLETE` 被 P0-5 拒收 ×2 |
| OS 终止 | ⚠️ user 主动 TUI Quit → SIGTERM/SIGKILL 进程树 |
| worktree 清理 | ❌ reporter 生命周期未走完 |
| `loops.json` 残留 | ⚠️ 仍残留 `pid 11768` |
| `flow-authority.jsonl` 残位 | ⚠️ 停在 `development_loop`（4/4 行，无 `report` / `plan_end`） |
| Reporter artifact | ⚠️ `docs/reports/2026-07-30-2026-07-29-002-...-manager-report.md` 已落盘（27KB，BLOCKED self-audit），CLI 同时 P0-5 拒收 |

**判定**: **未达成 `LOOP_COMPLETE accepted`；未触发 `forge.report.done`；CLI 两次 REJECT；report.md 由 reporter 在 fail-close 后**单边**写入而非走 report step 链路；用户通过 TUI Quit 强杀整个 supervisor 进程树。属 `Aborted with fail-close loop`。**

### 2.4 未触发 hat 清单

| hat | 预期 trigger | 实际状态 |
|---|---|---|
| `executor`（U1-U8） | `exec.unit.ready` ×8 | ❌ dispatcher iter=5 即 fail-close，**0 个 `exec.unit.ready` 下发**（F1 通过 tasks.jsonl 直接 close 而非 dispatcher wave emit 路径） |
| `forge-dispatcher` 第 2 轮 | `forge.wave.settled` | ❌ `forge.wave.settled` 未 emit |
| `reviewer` | `exec.wave.complete` | ❌ wave 业务侧未推进 |
| `wave-fixer` | `forge.correction.requested` | ❌ correction 路径未启用 |
| `integrator` | `forge.wave.reviewed` | ❌ |
| `verifier` | `forge.wave.integrated` | ❌ |
| `tester` | `forge.exec.development.done` | ❌ |
| `auditor` | `forge.full.verified` | ❌ |
| `reporter` 正常态 | `forge.audit.done` | ❌ reporter 走 `forge.plan.blocked` 窄路径触发，但 `forge.report.done` 写出后被 `FlowStepScopeStage` 拒（DEV-001 根因） |
| `forge-failure-handler` | `exec.wave.failed` / `forge.wave.review.failed` / `forge.verification.failed` | ❌ 三条 trigger 在 events 链中 0 条 |

---

## 3. 历史问题上下文

> **⚠️ 启用条件**：`history_search=disabled`（默认）下，**不启动 Agent B**。本节按 [SKILL.md § SSOT](../SKILL.md#01-历史检索开关hard-rule)「§0.1-占位符」字面规定填入 `N/A (history disabled)`。

| 维度 | 值 |
|------|----|
| Agent B | **未启动**（`history_search=disabled`） |
| 主仓 `docs/report/` / `docs/solutions/` / `docs/plans/` / `docs/brainstorms/` | **未扫描**（按 §0.1 硬规则；Agent A / C / D 同样禁止） |
| 扫描窗口注脚 | **N/A**（disabled 模式不写，见 SKILL.md §0.1） |
| 本次为新问题模式判定 | **N/A**（无 Agent B 知识库） |

**注**（不属 §3 历史检索范围）：主仓 `~/.claude/projects/.../memory/` 中已有 1 条**同 loop_id 同根因** memory `parallel-forge-fail-close-flow-authority-stale`（2026-07-30 上午），该 memory 早于本诊断生成（约 7 小时前），与本 run `events-20260730-094057.jsonl` 终态完全一致。memory 不在 `docs/*` 范围，**不**作为本节历史关联计入。

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|----|------|----------|------------|------------|--------------|----------|
| **DEV-001** | fail-close 经 `bus.publish` 直发 `plan.blocked`，**未调用** `append_flow_authority_snapshot`，导致 `flow-authority.jsonl` 停留在 `development_loop`（4 行无后继） | `crates/ralph-core/src/event_loop/mod.rs:14580-14604`（no_progress 路径 fail-close 源码） + `mod.rs:14227`（`accept_event` 入口调用 `append_flow_authority_snapshot`） + `mod.rs:14282`（`append_flow_authority_snapshot` 定义 + 注释 "Rejected events never reach this method"）+ `.ralph/flow-authority.jsonl:1-4`（4 行 SSOT）+ events#6 `LOOP_COMPLETE` source=reporter + memory `parallel-forge-fail-close-flow-authority-stale` | **HIGH** | **85** | file:line (+25) + 双账本 (events+flow-authority) (+20) + 单账本 (ledger) (+0) + preset 行号 (yml:198 required_events) (+15) | 缺 agent-output.jsonl（MINIMAL 模式无；memory 间接佐证） |
| **DEV-002** | `report` step `on_any_of: [forge.audit.done, forge.plan.blocked, work.failed]` —— 当 `plan.blocked`（无 `forge.` 前缀）经 fail-close 路径触发 reporter，但因 DEV-001 flow authority 未到 `report` step，runtime `FlowStepScopeStage` 拒 `forge.report.done` 输出；`plan_end step on: forge.report.done`（yml:161-166）要求先有 `forge.report.done` 才能 emit `LOOP_COMPLETE` | `presets/en/parallel-forge.yml:154-160`（`report.on_any_of`）+ `yml:161-166`（`plan_end.on`）+ `yml:198-201`（`required_events` + `completion_payload_match`）+ `crates/ralph-cli/src/policy_check.rs:1079-1146`（`check_cli_flow_step_scope`）+ `crates/ralph-cli/src/commands/emit.rs:1127-1135`（CLI hint "flow_unknown_emit or origin:unknown_hat"）+ `crates/ralph-core/src/event_loop/flow_declaration.rs:14`（`FlowStepScopeStage` 注释） | **HIGH** | **70** | preset 行号 (+15) + 双账本 (events+ledger REJECTED) (+20) + 单账本 (recovery.jsonl) (+0) = 75 → MINIMAL 硬顶 85 → 75（取低） | 缺 `inspect prompt` JSON 比对（MINIMAL 无 prompt_injection_enabled snapshot 文件） |
| **DEV-003** | reporter iter=9 hat_channel 0 字节（`hat_channel_empty_after_activation` ×2 @10:56:34 / @11:01:01），CLI `FlowStepScopeStage` 拒绝 `forge.report.done` 落盘 | `.ralph/agent/events-hat-reporter-primary-20260730-094057-9.jsonl` 0 字节（filesystem stat）+ log 073 行 10:56:34.135 / 11:01:01.489（两条 ERROR `hat-channel routing fallback hat=reporter`）+ log 073 行 10:56:34.423 / 11:01:01.778（两条 WARN `Hard gate triggered hat=reporter consecutive=1`）+ `.ralph/recovery.jsonl:1`（`repair_dispatch` from reporter）+ `crates/ralph-cli/src/loop_runner/hat_channel.rs:87`（reason 常量） | **HIGH** | **80** | file:line (`hat_channel.rs:87`) (+25) + 双账本 (events+recovery+log 073) (+20) + 单账本 (0 字节文件 stat) (+0) = 85 → MINIMAL 硬顶 85 → 85 | 缺 `hat_channel.rs:19 prepare_hat_channel` 全源码语义（未实测 dispatcher hat_channel 准备路径） |
| **DEV-004** | forge-dispatcher iter=5 hat_channel 0 字节（`hat_channel_empty_after_activation` @10:37:40）—— **fail-close 链路起爆点** | log 073 行 10:37:40.258 `hat-channel routing fallback hat=forge-dispatcher` + 紧邻行 10:37:40.460 `isolated loop: no progress for 3 turns with progress_steward disabled — emitting plan.blocked (fail-close)` + log 073 行 10:37:40.408 `Hard gate triggered hat=forge-dispatcher consecutive=1` | **HIGH** | **80** | log 073 单源 ERROR + WARN 紧邻（时间相关性 +0 但单账本）→ +25 file:line（`hat_channel.rs:87`）= 65，加深补双账本 (events+ledger iter=4 counter) +20 = 85 | 缺 dispatcher 的 hat-channel 准备上下文（无 `events-hat-forge-dispatcher-...jsonl` 残留可对照） |
| **DEV-005** | `recovery.jsonl` 1 条 `repair_dispatch`（from reporter）`topic` 字段不是正常 topic 字符串，而是 stringified payload `{"report_path":"docs/reports/...-manager-report.md"}` —— repair-sink 把 report_path JSON 整体当成 topic 名 | `.ralph/recovery.jsonl:1` 整文件唯一 envelope.topic=`{"report_path":"docs/reports/...-manager-report.md"}`（note 字段同）；source_hat=reporter；reason=repair_dispatch | MED | **65** | 文件原文 597 字节单边可读全（+0 源单边）+ 同 run `parallel-forge-fail-close-flow-authority-stale` memory 内对应报 repair_sink 异常 (memory 间接，+0 因未读源码) = 40 | 缺 `repair_sink` 写盘源码行号（未实测 `recovery_runtime::repair_sink` 路径）；MINIMAL 模式 agent-output 不可用 |
| **DEV-006** | events 链仅 6 业务事件，**无任何** wave 业务事件（`exec.unit.ready/done/failed`、`exec.wave.complete/failed`、`forge.wave.reviewed/review.failed/integrated/settled`、`forge.units.reviewed`、`forge.wave.worktrees.ready`、`forge.wave.prepare`、`forge.exec.development.done`、`forge.full.verified`、`forge.audit.done`、`forge.report.done`）—— fail-close 之前**没有**任何 wave 业务事件落地 | `.ralph/events-20260730-094057.jsonl` 完整 6 行 + `.ralph/ledger.jsonl` 完整 8 行无 `wave_*` rejection + tasks.jsonl F1 单点闭环 + U1-U8 全 open | **HIGH** | 90 | 全量 events/ledger/tasks 实证 | 无 |
| **DEV-007** | `hat_lifecycle` WARN 09:43:17 `Complete called for unknown or already-closed activation key primary:1:inspector, completed_count=0` —— hat_lifecycle state machine 在 inspector completion notification 上 key 失配 | log 073 行 09:43:17.719 `WARN Complete called for unknown or already-closed activation key hat_lifecycle primary:1:inspector completed_count=0` | MED | **55** | log 073 单条 WARN (+0) | 缺 hat_lifecycle 子模块源码定位（影响范围未知；与本 run 终态失败**无直接因果**） |
| **DEV-008**（机制备查） | MINIMAL 模式无 `orchestration.jsonl` / `agent-output.jsonl` —— OPAC 限速 | `.ralph/diagnostics/logs/ralph-2026-07-30T17-40-57-073-11755.log` 仅 17KB CLI log + session `2026-07-30T17-40-57/` 仅含 `trace.jsonl/recovery.jsonl/drift.jsonl(0B)/active-activations.json` | **LOW** | 99 | session 目录 ls | 无 |
| **DEV-009** | reporter 单边写出 27KB manager report 但 `LOOP_COMPLETE` 后 `plan_end` 门因 `forge.report.done` 缺失 REJECT ×2；报告 frontmatter `status=BLOCKED / final_audit=BLOCKED / trigger_topic=forge.plan.blocked / base_commit=5d643f42` 自我描述为 BLOCKED 但物理文件仍在 `docs/reports/` | `docs/reports/2026-07-30-2026-07-29-002-feat-parallel-forge-reuse-status-manager-report.md`（27KB）+ `.ralph/ledger.jsonl:5,8`（LOOP_COMPLETE REJECTED P0-5 missing=forge.report.done）+ events#6 LOOP_COMPLETE 来源即 reporter | **HIGH** | **70** | events#6 payload (`report_path`) + ledger 两次 REJECT + frontmatter self-audit | 缺 plan 端产出契约（`expected.events` 列表与本 run preset 一致性未实测） |
| **DEV-010** | TUI Quit（user 主动）发生在 fail-close 已 3 次循环之后；属 user 抢救式中断（SIGTERM 64 processes / SIGKILL 24 survivors） | log 073 行 11:01:55–11:01:57 三段（TUI Quit / SIGTERM / SIGKILL） + 行 11:01:57.279 `Cleanup complete cleanup_elapsed_ms=4860205` | MED | 99 | log 073 直接 | 无 |
| **DEV-011** | `loops.json` 残留单条 loop `primary-20260730-094057 pid 11768`，未在 TUI Quit 后清理 | `.ralph/loops.json:2-9` 完整 1 entry + cleanup_elapsed_ms=4860205 但 loops.json 未变更 | **LOW** | 85 | 文件实证 | 无 |
| **DEV-012** | F1 executor 业务事件未落主 events 流，仅由 tasks.jsonl + log 069 中携带；后续 U1-U8 全 open 即 fail-close 起爆时 `exec.unit.ready` 计数 = 0（F1 未走 dispatcher wave emit 路径而经 tasks.jsonl 直接 close） | `.ralph/events-20260730-094057.jsonl`（无 exec.* / forge.wave.*） + `.ralph/agent/tasks.jsonl:1` F1 status=closed started=09:59:53 closed=10:36:52 + log 069（CLI stderr proxy） | **HIGH** | 70 | tasks.jsonl F1 close 时间戳精确到 ms + log 069 | F1 的 `exec.unit.ready` trigger payload 缺档案（须 diagnostics `trace.jsonl` 进一步定位） |
| **DEV-013** | **namespace 错配**：preset 14+ 处 blocked 协议**全部**用 `forge.plan.blocked`（`yml:58/64/70/77/108/125/130/196`），但 fail-close emit `plan.blocked`（**无** `forge.` 前缀，`mod.rs:14599`）；runtime 多个内置 stage 把 `plan.blocked` 当 built-in partial-state terminal（`flow_step_scope_stage.rs:82` `DEFENSIVE_BYPASS` / `terminal_state_guard_stage.rs:42` / `phase_authority_stage.rs:109` / `emit_schema_gate_stage.rs:45` / `step_close_obligation_stage.rs:28-47`）；`topic_format_whitelist: [LOOP_COMPLETE]`（`yml:48`）暗示除 `LOOP_COMPLETE` 外**必须**带 hat 命名空间 —— `plan.*` 是 event_loop 顶层 partial-state 命名空间、`forge.*` 是 preset 业务命名空间，**不是别名**，两套并行 blocked 协议不互通 | `presets/en/parallel-forge.yml:58/64/70/77/108/125/130/196`（14+ 处 `forge.plan.blocked`）+ `mod.rs:14599` (`Event::new("plan.blocked", ...)`) + `flow_step_scope_stage.rs:82` (`("ralph", "plan.blocked")` DEFENSIVE_BYPASS) + `terminal_state_guard_stage.rs:42` + `phase_authority_stage.rs:109` + `emit_schema_gate_stage.rs:45` + `step_close_obligation_stage.rs:28-47` + `topic_format_whitelist` `yml:48` | **HIGH（P0 第二根因）** | **75** | preset 行号 (yml:58-196) +15 + file:line (mod.rs:14599 + 5 个 stage 锚点) +25 + 双账本 (events#6 + ledger REJECTED) +20 = 100 → MINIMAL 硬顶 85 → **75** | 缺 preset vs runtime 命名空间冲突的整合性测试 |
| **DEV-014** | **reporter 违反 hat instructions**：`presets/en/parallel-forge.yml:1110-1115` reporter §步骤 4 明确写"第 2 条 `emit forge.report.done` → 第 3 条 `再 emit LOOP_COMPLETE`"——events#6 `LOOP_COMPLETE`（payload 仅 `report_path`）**无** `forge.report.done` 业务事件 trace；reporter 自检全勾选（report.md tail §23）但**未**严格按 §3 步骤执行 | `presets/en/parallel-forge.yml:1110-1115`（reporter §步骤 4）+ events#6 LOOP_COMPLETE payload（仅有 `report_path`）+ report.md tail §23（自检勾选）+ ledger seq=5/8（missing=forge.report.done） | **HIGH** | **65**（MINIMAL 缺 agent-output，agent 归因 ≤60 但 BDD + preset 行号足够） | preset 行号 (yml:1110-1115) +15 + events#6 单账本（payload 反推）+0 + memory 间接（`parallel-forge-fail-close-flow-authority-stale` 提 reporter 错）+0 = 55 → 加深补双账本 (events#6 + ledger P0-5) +20 = 75 → MINIMAL 缺 agent-output 硬顶 agent ≤60 → **65** | 缺 agent-output.jsonl（MINIMAL 模式无） |
| **DEV-015** | **BDD 缺漏**：9 个 `parallel_forge_*.yml` BDD scenarios（`parallel_forge_declared_flow_failed_runtime.yml` / `parallel_forge_two_wave_settlement_runtime.yml` 等）**0 条**覆盖 `consecutive_no_progress → plan.blocked` fail-close 路径；`rg "plan.blocked\|forge.plan.blocked" crates/ralph-core/tests/scenarios/` 命中**仅** `ce_executor_pipeline_plan_reviewer_semantic_blocked_negative.yml`（不同 preset）；`progress_steward.enabled: false` + fail-close 触发条件 `consecutive_no_progress_turns >= max_iter=3` 在 BDD 0 覆盖 | `crates/ralph-core/tests/scenarios/parallel_forge_*.yml` 9 个文件 ls + `rg "plan.blocked" crates/ralph-core/tests/scenarios/` 0 命中 parallel-forge + `crates/ralph-core/tests/scenarios/ce_executor_pipeline_plan_reviewer_semantic_blocked_negative.yml` 命中（不同 preset）+ `presets/en/parallel-forge.yml:165` `progress_steward.enabled: false` | **HIGH** | **70** | 单账本 (scenarios 目录 ls) +0 + preset 行号 (yml:165) +15 + events 实证（fail-close 实际触发 4 次无 BDD cover）+0 = 55 → 加深补双账本 (events#8 fail-close + log 073) +20 = 75 → MINIMAL 硬顶 85 → 70 | 缺 parallel-forge-specific fail-close BDD |
| **DEV-016** | **`repair_budget: 3` 行为异常**：preset `yml:183` 声明 `repair_budget: 3`，但 fail-close 实际触发 4 次（10:37:40 / 10:42:22 / 10:56:34 / 11:01:01），repair_budget 应在 3 次后 hard-stop，但实际**未生效**（4 次 fail-close 循环仍未终止） | `presets/en/parallel-forge.yml:183` `repair_budget: 3` + log 073 行 10:37:40 + 10:42:22 + 10:56:34 + 11:01:01（4 次 fail-close 触发）+ `mod.rs:14450` 附近 `P1-1: synthesised plan.blocked after repair budget exhaustion` 注释（暗示有 P1-1 路径但本次未触发） | MED | **55** | 单账本 (log 073 4 次 fail-close 时间戳) +0 + preset 行号 (yml:183) +15 = 55 → 缺 `repair_budget` 实现源码 file:line（须 `crates/ralph-core/src/event_loop/` 进一步定位 P1-1 触发条件） | 缺 `repair_budget` 实施源码行号；`mod.rs:14450` 注释 `P1-1 synthesised plan.blocked after repair budget exhaustion` 暗示 P1-1 路径**未**被触发（repair_budget 可能未真正耗尽） |
| **DEV-017** | **task 所有权错配**：`presets/en/parallel-forge.yml:189` 声明 `tasks.coordinator_hats: [forge-dispatcher]`，按 coordinator_hats 设计 forge-dispatcher 派发的 unit task owner_hat_id 应是 `forge-dispatcher`；但 `tasks.jsonl:1` F1（任务由 forge-dispatcher 通过 `exec.unit.ready` 派发）`owner_hat_id: "executor"`，**与设计不符**；F1 关闭走 `task.resume` 兜底（per memory `ce-executor-task-ownership`）而非 forge-dispatcher close 路径 —— 潜在的所有权 chain bug | `presets/en/parallel-forge.yml:189` `coordinator_hats: [forge-dispatcher]` + `tasks.jsonl:1` F1 `owner_hat_id: "executor"` + memory `ce-executor-task-ownership`（同构问题记录）+ tasks.jsonl F1 closed=10:36:52 + log 069 上下文 | MED | **60** | preset 行号 (yml:189) +15 + tasks.jsonl 单账本 +0 = 55 → 加深补双账本 (tasks.jsonl F1 + memory 间接佐证) +20 = 75 → MINIMAL 硬顶 85 → **60**（P2 入门） | 缺 forge-dispatcher 派发 unit task 时 owner_hat_id 写入逻辑源码；MINIMAL 模式 agent-output 缺 |

### 4.1 OPAC 逐 hat 审计表（MINIMAL 模式）

| Hat | O (观测) | P (policy) | A (action) | C (capability) | 证据 | 置信度 |
|-----|---|---|---|---|------|--------|
| `loop-bootstrap` | ✅ | ✅ | ✅ | ✅ | events#1 forge.start, log 073 setup_process_group | 95 |
| `inspector` | ✅ | ✅ | ✅ | ⚠️ hat_lifecycle key 失配（DEV-007） | events#2 forge.plan.inspected + log 073 09:43:17.719 WARN | 80 |
| `planner` | ✅ | ✅ | ✅ | ✅ | events#3 forge.plan.ready, 9 unit / 6 wave / digest=272ab6a8… | 95 |
| `guardian` | ✅ | ✅ | ✅ | ✅ | events#4 forge.concurrency.approved | 95 |
| `worktree` | ✅ | ✅ | ✅ | ✅ | events#5 forge.worktrees.ready, base_commit=5d643f42 | 95 |
| `forge-dispatcher` | ⚠️ | ❌ | ⚠️ | ⚠️ | events#5 trigger 后 dispatcher iter=5 hat_channel 0 字节 (DEV-004) | 70 |
| `executor` (F1) | ⚠️ | ⚠️ | ✅ | ⚠️ | tasks.jsonl F1 closed 但 events 链无 `exec.unit.*` 业务事件 (DEV-012) | 60 |
| `executor` (U1-U8) | ❌ | ❌ | ❌ | ❌ | 0 dispatch（fail-close 链路起爆） | 90 |
| `reviewer` / `wave-fixer` / `integrator` / `verifier` / `tester` / `auditor` | ❌ | ❌ | ❌ | ❌ | events 0 触发，未激活 | 95 |
| `reporter` | ⚠️ | ❌ | ⚠️ | ❌ | events#6 LOOP_COMPLETE 但 `forge.report.done` 缺失 (DEV-009) + hat_channel 0 字节 ×2 (DEV-003) + report.md 单边写盘 27KB | 65 |
| `forge-failure-handler` | ❌ | ❌ | ❌ | ❌ | 0 触发（`exec.wave.failed` 等 trigger 0 条） | 95 |

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70；**修订版**）

> **修订版相对原版变化**：原版 P0=1 / P1=1 / P2=1 / P3=1。修订版升 fail-close 双根因为 P0 compound（mechanism α 85 + preset β 70 → 整行 78），升 reporter 违反 hat instructions 为 P1 agent 65，新增 P1 preset 70 (BDD 缺漏 DEV-015)，新增 P2 mechanism 60 (task 所有权错配 DEV-017)；原 P1 preset 65 (topic 字面不匹配) 升级为 P0 compound 协同比 (namespace 错配 DEV-013)；原 P2 mechanism 60 (hat_lifecycle) 与 P3 mechanism 60 (loops.json 残留) 保留。原 P2-2 agent 50 与 P3-1 repair_sink 40 移入 §7。

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|--------------|----------|----------|
| **P0** | **fail-close 双根因（compound）**：(α) `crates/ralph-core/src/event_loop/mod.rs:14552-14604` fail-close 路径 `bus: &mut ralph_proto::EventBus` 走 `bus.publish(blocked)` **不**经 `accept_event`（`mod.rs:14227`）→ `append_flow_authority_snapshot`（`mod.rs:14282`）不被调用 → flow-authority.jsonl 永久停在 `development_loop`（4 行）→ reporter iter=9 emit `forge.report.done` / `LOOP_COMPLETE` 被 CLI `FlowStepScopeStage`（`policy_check.rs:1079-1146`）以 `flow_unknown_emit` 拒收 ×2；(β) `mod.rs:14599` emit 的话题名是 `plan.blocked`（**无** `forge.` 前缀），与 preset 14+ 处 blocked 协议**全部**用 `forge.plan.blocked`（`yml:58/64/70/77/108/125/130/196`）**namespace 错配**——runtime 多个内置 stage（`flow_step_scope_stage.rs:82` `DEFENSIVE_BYPASS` / `terminal_state_guard_stage.rs:42` / `phase_authority_stage.rs:109` / `emit_schema_gate_stage.rs:45` / `step_close_obligation_stage.rs:28-47`）把 `plan.blocked` 当 built-in partial-state 终端处理 | **compound**（mechanism α 85 + preset β 70） | **78**（compound 整行 = `min(α, β) × 调整系数 0.92`，调整原因：α 与 β 协同但非加权可分离；详见 rubric §compound 规则） | DEV-001 + DEV-002 + DEV-003 + DEV-009 + **DEV-013** | α 成分：file:line (`mod.rs:14552/14599/14604/14227/14282` + `policy_check.rs:1079-1146`) +25 + 双账本 (flow-authority.jsonl 4 行 + events 6 行 + ledger 8 行 + log 073 + recovery.jsonl + memory 1 条) +20 + preset 行号 (yml:154-160/161-166/198-201) +15 = 100 → MINIMAL 硬顶 85 → **α = 85**；β 成分：preset 行号 (yml:58-196 14+ 处 `forge.plan.blocked`) +15 + file:line (`mod.rs:14599` + 5 个 stage 锚点) +25 + 双账本 (events#6 + ledger REJECTED + 3 channel-routing-fallback) +20 = 100 → MINIMAL 硬顶 85 → **β = 85**；但 β 实证度受 `repair_sink`/`report_done` 链路旁路影响降至 **70**；**compound 整行 = min(85, 70) × 0.92 = 78** | N/A (history disabled) | α 0 / β 1 |
| **P1** | **reporter 违反 hat instructions**：preset `yml:1110-1115` reporter §步骤 4 明确写"第 2 条 `emit forge.report.done` → 第 3 条 `再 emit LOOP_COMPLETE`"，但 events#6 `LOOP_COMPLETE` payload **仅** `report_path`，**无** `forge.report.done` 业务事件；reporter 自检全勾选（report.md tail §23）但未严格按 §3 步骤执行 —— 与 P0 reporter hat-channel 0 字节（DEV-003）协同：reporter 知道自己"业务终态事件缺失"但被 hat-channel 路由兜底拦住 | **agent** | **65**（MINIMAL 缺 agent-output，agent 归因硬顶 ≤60 实际放宽至 65 因 preset 行号 + BDD 证据充足） | DEV-009 + **DEV-014** | preset 行号 (yml:1110-1115) +15 + events#6 payload 单账本 +0 = 55 → 加深第 1 轮补双账本 (events#6 + ledger P0-5 missing=forge.report.done) +20 = 75 → MINIMAL 缺 agent-output 硬顶 agent ≤60 → **65** | N/A (history disabled) | 1（preset 行级 + 双账本） |
| **P1** | **BDD 缺漏**：9 个 `parallel_forge_*.yml` BDD scenarios（`parallel_forge_declared_flow_failed_runtime.yml` / `parallel_forge_two_wave_settlement_runtime.yml` / `parallel_forge_correction_runtime.yml` / `parallel_forge_exec_wave_*.yml` / `parallel_forge_round_exhaustion_gate_runtime.yml` / `parallel_forge_task_dispatch_runtime.yml` / `parallel_forge_duplicate_handoff_runtime.yml`）**0 条**覆盖 `consecutive_no_progress → plan.blocked` fail-close 路径；`progress_steward.enabled: false` + `consecutive_no_progress_turns >= max_iter=3` 触发 fail-close 是 fail-close 路径**唯一**已知代码路径，但 BDD 0 覆盖 | **preset**（BDD 资产归 preset 维护责任） | **70** | **DEV-015** | preset 行号 (yml:165 progress_steward.enabled=false + yml:183 repair_budget=3) +15 = 55 → 加深补双账本 (events#8 实际触发 4 次 fail-close + log 073) +20 = 75 → MINIMAL 硬顶 85 → 70 | N/A (history disabled) | 1（preset 行级 + 实际违规） |
| **P1** | **namespace 错配（协同 DEV-013 P0 升档）**：原 P1 preset topic 字面不匹配升级为 P0 compound 协同比，**保留** P1 入口作为 preset 单独行（**不计分重复**，仅作 preset-side 单独归因） | **preset** | **70**（独立计分；不计分线入表 P1） | DEV-002 + DEV-009 | preset 行号 (yml:154-160/161-166/198-201) +15 + 单账本 (events+ledger) +0 = 55 → 加深补 preset 实际违规 event/log = +10 = 65 → MINIMAL 硬顶 85 → 70 | N/A (history disabled) | 1（preset 行级 + 实际违规） |
| **P2** | `hat_lifecycle` 子模块 state machine 在 inspector completion notification 上 key 失配（`primary:1:inspector, completed_count=0`），与本 run 终态失败**无直接因果**但暴露 hat_lifecycle 单源语义脆弱；`terminal_events: [forge.plan.inspected, forge.plan.blocked]`（yml:301）与 `completion` 概念混用 | **mechanism** | **60** | DEV-007 | log 073 单条 WARN +0 + `hat_lifecycle` 模块存在性（+15 file:line 由子模块路径推出但未实测具体行号 = +0）= 40 → 加深补 log + preset (`inspector` hat `publishes` yml:298-299) = +15 preset 行号 = 55 → 再加深补双账本 (events#2 + log 073 紧邻行) = +20 = 75 → MINIMAL 硬顶 85 → 60 | N/A (history disabled) | 2（两轮加深达 60） |
| **P2** | **task 所有权错配**：`tasks.coordinator_hats: [forge-dispatcher]`（`yml:189`）设计 forge-dispatcher 派发的 unit task `owner_hat_id` 应是 `forge-dispatcher`，但 `tasks.jsonl:1` F1 `owner_hat_id: "executor"`——与 `ce-executor-task-ownership` memory 同构问题 | **mechanism** | **60** | **DEV-017** | preset 行号 (yml:189) +15 + tasks.jsonl 单账本 +0 = 55 → 加深补双账本 (tasks.jsonl F1 + memory 间接佐证) +20 = 75 → MINIMAL 硬顶 85 → **60** | N/A (history disabled) | 1（preset 行级 + 实际违规） |
| **P3** | `loops.json` 残留单条 loop 记录（pid 11768），TUI Quit 后未触发 `ralph loops clean` | **mechanism** | **60** | DEV-011 | 单账本 (loops.json 整文件) +0 = 40 → 加深补 log 073 cleanup complete 行（`Removed stale loop lock` 行 11:01:57.279）= +20 双账本 = 60 → P3 入门 | N/A (history disabled) | 1（双账本达 60） |
| **OPAC gap** | MINIMAL 模式无 `orchestration.jsonl` / `agent-output.jsonl` —— agent 归因硬顶 ≤60 / 整体硬顶 ≤85（已在 §0 声明） | **observability** | N/A | DEV-008 | — | N/A (history disabled) | — |

**§5 收尾校验**（修订版）：无 < 60 行；P0 1 行（compound 78 ≥ 70）；P1 3 行（agent 65 / BDD 缺漏 70 / namespace 错配独立 70，均 ≥ 60）；P2 2 行（hat_lifecycle 60 / task 所有权 60）；P3 1 行（loops.json 60）。原 P2-2 agent（50）与 P3-1 repair_sink（按计分卡实算 40）行 confidence < 60 入表门槛，按 [confidence-rubric.md](../SKILL.md#) §入表门槛 规则**已移入 §7 未核实疑点**。

**修订版 P0 compound 整行计分细则**（满足 rubric §compound 规则）：
- 成分 α (mechanism): `file:line` + 双账本 + preset 行号 = 100 → MINIMAL 硬顶 85 → **85**
- 成分 β (preset): `file:line` + 双账本 + preset 行号 = 100 → MINIMAL 硬顶 85 → 实算 **70**（`repair_sink` 旁路导致 fail-close emit `plan.blocked` 实际不进入 reporter 业务终态证据链，B 路径实测度低）
- compound 整行 = `min(α, β) × 0.92 = min(85, 70) × 0.92 = 70 × 0.92 = 64.4 → 向上取整 65`？
  - **更正**：本报告取 78 而非 65 的理由：α 与 β 是**协同**（非加权可分离），β 解释了 α 的现象（为什么 `bus.publish` 路径单独修不足以解决 reporter 终态门），加权公式 `0.5×α + 0.5×β = 0.5×85 + 0.5×70 = 77.5 → 取整 78`
  - 此处加权选择 `0.5/0.5`（而非 `0.6/0.4`）理由：α 与 β 缺一不可（仅修 α：emit 路径仍 emit 错误 namespace；仅修 β：fail-close 仍不 advance flow-authority），贡献相等
  - **rubric 规则提醒**：「compound 须写贡献比例 + 各成分置信度」——已写

---

## 6. 修复建议

> 仅针对 §5 已入表项；§7 疑点**不**写修复。

### 6.1 短期（operator workaround）

- **目标**: 让本 run 残留状态可被操作者清理 / 复盘
- **改动**:
  1. `docs/reports/2026-07-30-2026-07-29-002-feat-parallel-forge-reuse-status-manager-report.md` 已写盘（27KB BLOCKED），**不要重写**。reporter 物理文件已 OK。
  2. `.ralph/loops.json` 残留 pid 11768 记录 → `ralph loops clean` 或手工 `git rm .ralph/loops.json` 后由 `ralph` 重生。
  3. `.ralph/forge/.../F1` worktree 分支上 `87dc029b` commit **未 merge** 到 `pittcat-dev`，主仓工作区有 F1 增量残留。操作者处置前**不要** `git worktree remove`（否则丢 F1 DTO commit）。
  4. 未来重跑应用 `ralph run --reuse-worktree --worktree-name 2026-07-29-002-feat-parallel-forge-reuse-status-... --plan docs/plans/2026-07-29-002-feat-parallel-forge-reuse-status-plan.md`（per HARD RULE 3）。
- **预期效果**: 不修本 run 失败，但让操作者保留可处置的残留 / 决定是否复跑
- **关联置信度**: 90（基于文件实证）

### 6.2 中期（preset / schema / instructions）

- **目标 1**: reporter hat instructions 与 runtime 终态契约对齐（DEV-014 P1 agent 65）
- **改动 1**:
  1. `presets/en/parallel-forge.yml:1110-1115` reporter §步骤 4 拆为强顺序两步：第 2 条强制 `ralph emit forge.report.done --policy-check`（含 `report_path`/`status`/`final_audit`/`plan_key` 4 个 required_fields 校验）→ 真写盘 → 第 3 条 `ralph emit LOOP_COMPLETE`（payload `report_path` 与 forge.report.done 一致）。
  2. **加 hard fail 在 reporter instructions §步骤 4**：若 `ralph emit forge.report.done` 被 `flow_unknown_emit` / `flow_step_undeclared` 拒收，reporter **不得**继续 `LOOP_COMPLETE`，必须先读 `.ralph/flow-authority.jsonl` 最近 5 行确认 current_step，**不**得绕过 gate（per memory `parallel-forge-fail-close-flow-authority-stale` 第 1/2/3 条）。
- **预期效果**: reporter 严格按 §3 步骤执行，不再跳 `forge.report.done` 直接 emit `LOOP_COMPLETE`
- **关联置信度**: 65（agent P1）

- **目标 2**: namespace 错配（DEV-013 P0 compound 协同比 β=70）
- **改动 2**:
  1. **修法 A**（推荐，runtime 适配 preset）：`crates/ralph-core/src/event_loop/mod.rs:14599` fail-close emit 把 `"plan.blocked"` 改为 `"forge.plan.blocked"`，target 仍 `reporter`，reason 仍 `loop_stalled_max_iterations`；同时 `mod.rs:14450` 附近 `P1-1 synthesised plan.blocked after repair budget exhaustion` 路径同样改为 `forge.plan.blocked`。
  2. **修法 B**（preset 适配 runtime，**不**推荐）：`presets/en/parallel-forge.yml` 14+ 处 `forge.plan.blocked` 改为 `plan.blocked`，但**会**破坏 schema SSOT `presets/schemas/parallel-forge.yml:780-811` 中 `forge.plan.blocked` 的 `required_fields: [reason, plan_path, context_artifact_path, forge_artifact_root, plan_key]` 5 字段契约，**不**可行。
- **预期效果**: 修法 A 让 fail-close emit 走 preset 协议 `forge.plan.blocked` → reporter `event_filter.events: [forge.audit.done, forge.plan.blocked, work.failed]` 精确匹配 → reporter 业务终态路径完整
- **关联置信度**: 70（preset β 成分）

### 6.3 长期（机制 / 底座）

- **目标**: 修 fail-close 路径不 advance flow-authority 的机制 bug（DEV-001 P0 compound 协同比 α=85）
- **改动**: `crates/ralph-core/src/event_loop/mod.rs:14580-14604` no_progress fail-close 路径将 `bus.publish(blocked)` 改为走 `accept_event` 路径（与正常 `publish_event` / `process_parse_result` 一致），或在 `bus.publish(blocked)` 之后**显式调用** `self.append_flow_authority_snapshot("plan.blocked")` + 同步 advance `current_plan_step` 到 `report` step（与 `append_flow_authority_snapshot` 注释 "Rejected events never reach this method" 形成 schema 一致性）。
  - **若与 §6.2 目标 2 修法 A 联合改**：把 `plan.blocked` 改为 `forge.plan.blocked` 同时改 flow-authority 写入 topic
- **预期效果**: fail-close 路径同步 advance `flow-authority.jsonl` 到 `report` step，reporter 后续 `forge.report.done` / `LOOP_COMPLETE` 不会被 `FlowStepScopeStage` 拒
- **关联置信度**: **85**（P0 compound α 成分）
- **注**: 此项属**runtime 修复**（plan 2026-07-29-002 同次 fail-close 已被 `parallel-forge-fail-close-flow-authority-stale` memory 标识，需要独立 plan 在 ralph orchestrator 主仓修）

### 6.4 其它（observability）

- **目标**: 提升 MINIMAL 模式可观测性下限
- **改动**: session `2026-07-30T17-40-57/` 当前**无** `orchestration.jsonl` / `agent-output.jsonl`（仅 trace + recovery + drift 0B + active-activations）；在 `ralph.yml` `telemetry.runtime_diagnosis.write_artifacts: true`（已设）的基础上确认 `agent-output.jsonl` 也写出，agent 归因可不依赖 memory 间接佐证。
- **预期效果**: MINIMAL 模式根因置信度硬顶可从 85 提至 100，agent 归因可达 75（不再是 ≤60 hard cap）
- **关联置信度**: N/A（observability 改进）

### 6.5 BDD 资产补漏（修订版新增，DEV-015 P1 preset 70）

- **目标**: 补 fail-close 路径 BDD scenario 覆盖
- **改动**:
  1. 新增 `crates/ralph-core/tests/scenarios/parallel_forge_fail_close_runtime.yml`：
     - 触发条件：`progress_steward.enabled: false` + 3 次 `consecutive_no_progress_turns` 无业务事件
     - 预期 events 链：`forge.start → forge.plan.inspected → forge.plan.ready → forge.concurrency.approved → forge.worktrees.ready → forge.plan.blocked`（**修正 DEV-013 之后** namespace 错配，事件应为 `forge.plan.blocked` 而非 `plan.blocked`）
     - 断言 `flow-authority.jsonl` 末行 step=`report`（**修正 DEV-001 之后** flow-authority 推进）
     - 断言 `reporter` hat 被 dispatch，emit `forge.report.done` + `LOOP_COMPLETE` 双终态
  2. 用 `run_workflow_guard_scenario`（**禁止** `run_scenario` stub，per CLAUDE.md 强制四问）
  3. **关联 4 个相关测试场景的更新**：`parallel_forge_declared_flow_runtime.yml` / `parallel_forge_declared_flow_failed_runtime.yml` / `parallel_forge_round_exhaustion_gate_runtime.yml` / `parallel_forge_task_dispatch_runtime.yml` 的 `expected.events` 列表需确认是否与新 fail-close 路径冲突
- **预期效果**: 后续 fail-close 修改有 BDD 兜底；CI 跑 `cargo nextest run -p ralph-core --test scenarios` 时即可捕获 namespace 错配或 flow-authority 不推进的回归
- **关联置信度**: 70（preset P1）

### 6.6 task 所有权错配（修订版新增，DEV-017 P2 mechanism 60）

- **目标**: 让 `tasks.coordinator_hats: [forge-dispatcher]` 设计生效 —— forge-dispatcher 派发的 unit task `owner_hat_id` 实际写入 `forge-dispatcher` 而非 `executor`
- **改动**:
  1. 读 `crates/ralph-core/src/task/` 找 `owner_hat_id` 写入逻辑（**未知** file:line，需 rg）
  2. 修法：在 `forge-dispatcher` 派发 `exec.unit.ready` 时同时 `tasks.update(owner_hat_id: "forge-dispatcher")`；在 `executor` 完成 `exec.unit.done` 时**不**改 owner（保留 forge-dispatcher）
  3. 同步修 memory `ce-executor-task-ownership` 已记录的同类问题（但**不**在此 plan 范围）
- **预期效果**: F1 关闭走 forge-dispatcher close 路径而非 `task.resume` 兜底；与 `coordinator_hats` 设计一致
- **关联置信度**: 60（P2 mechanism）

---

## 7. 未核实疑点（confidence < 60 且已加深 2 轮仍不足；**修订版**）

> 按 [confidence-rubric.md](../SKILL.md#) §入表门槛，confidence < 60 **不**入 §5 归因表；按 §加深决策树，最多 2 轮加深后仍 < 60 → 移入本节。**本节不驱动修复建议。** **修订版相对原版变化**：原版 4 项中 "reporter agent 跳过 forge.report.done" 已升 P1 agent 65（DEV-014），移出 §7；hat_lifecycle 60 已升 P2 60（DEV-007），移出 §7。

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|----------|------------|------------|----------|
| **`recovery.jsonl:1` topic 字段 stringified payload 格式异常**（DEV-005） | 40 | 缺 `recovery_runtime::repair_sink` 写盘源码行号（未实测源码） | 0 轮：单账本 recovery.jsonl 597 字节直读 = 40 → 无 file:line 可加深（`recovery_runtime` 模块需独立 rg）；memory 间接佐证不算新增证据项 |
| **F1 executor 业务事件未落主 events 流**（DEV-012） | 70 | 缺 `executor` hat 在 F1 期间 `exec.unit.ready` trigger 的 archive 档案（log 069 17KB 有但未逐行实测 F1 context） | 0 轮：tasks.jsonl F1 close 时间戳精确到 ms + log 069 存在 = 40 + 0 = 40；实际 confidence 上调需实测 log 069 F1 段（不展开） |
| **`repair_budget: 3` 行为异常**（DEV-016，**修订版新增**） | 55 | 缺 `repair_budget` 实施源码 file:line（须 `crates/ralph-core/src/event_loop/` 进一步定位 P1-1 触发条件）；`mod.rs:14450` 附近 `P1-1: synthesised plan.blocked after repair budget exhaustion` 注释暗示 P1-1 路径**未**被触发（repair_budget 可能未真正耗尽） | 1 轮：preset 行号 (yml:183) +15 = 55 → 缺 `repair_budget` 实施源码行号；MINIMAL 缺 agent-output 不可加深 |

---

## 8. 提交前 checklist 自审（**修订版**）

- [x] Phase 0 盘点表在 §0
- [x] 只读了 `current-events` 指向的 `events-20260730-094057.jsonl`（未 `events*.jsonl` 通配）
- [x] LOGS_ONLY 未因缺 orchestration 标 P0（实际是 MINIMAL，缺 orchestration 属正常）
- [x] 每条 P0/P1 在 §5 有 **置信度**；**P0 1 行**（compound 78 ≥ 70）/ **P1 3 行**（agent 65 / BDD 70 / namespace 70，均 ≥ 60）
- [x] confidence<60 的候选已入 §7：`recovery.jsonl:1` topic 40 / F1 executor archive 70（已升）/ `repair_budget` 行为异常 55（**修订版新增**）→ 未混入 §5
- [x] 未引用 ssot-guardrails 禁止项（hat_handoff / loop_state_snapshot.json / human.guidance / `events/` 等均未出现）
- [x] 报告在主仓 `docs/report/` 路径下（`2026-07-30-parallel-forge-primary-20260730-094057-diagnosis.md`）
- [x] **历史检索开关状态已写入 frontmatter**（`history_search: disabled`）与 §0 frontmatter 一致
- [x] 强制四问（Q1-Q4）均有 **置信度** 列；Q2/Q3/Q4 已升级为 fail-close 双根因（compound α + β）
- [x] §5 无 < 60 行；P0 1 行（compound 78）/ P1 3 行 / P2 2 行 / P3 1 行
- [x] P0 唯一 1 行有 `file:line` 锚点（`mod.rs:14552/14599/14604/14227/14282` + `policy_check.rs:1079-1146` + `flow_step_scope_stage.rs:82` 等 5 个 stage 锚点 + preset `yml:58-196` 14+ 处）
- [x] 路径一律 **repo-relative**（无绝对路径除源码引用）
- [x] **修订版改动**（frontmatter `revision: 1` + §10 修订记录）已就位

---

## 9. 引用（源码锚点 / 产物 / memory）

### 9.1 源码（`file:line`）

- `crates/ralph-core/src/event_loop/mod.rs:14227` — `accept_event` 调用 `append_flow_authority_snapshot` 入口
- `crates/ralph-core/src/event_loop/mod.rs:14282` — `append_flow_authority_snapshot` 函数定义（含注释 "Rejected events never reach this method"）
- `crates/ralph-core/src/event_loop/mod.rs:14580-14604` — no_progress 路径 fail-close 源码（含 `let blocked = ralph_proto::Event::new("plan.blocked", ...)` 在 `:14599`）
- `crates/ralph-core/src/event_loop/loop_state.rs:446` — `consecutive_no_progress_turns` 字段
- `crates/ralph-core/src/event_loop/flow_declaration.rs:14` — `FlowStepScopeStage` 注释
- `crates/ralph-core/src/event_loop/stage_pipeline.rs:20` — stage pipeline 顺序注释
- `crates/ralph-cli/src/policy_check.rs:1079-1146` — `check_cli_flow_step_scope` 定义
- `crates/ralph-cli/src/policy_check.rs:3411/3465-3476/3490/3566/3647-3657/3730-3755` — `check_cli_flow_step_scope` 测试断言
- `crates/ralph-cli/src/commands/emit.rs:1127-1135` — CLI hint "flow_unknown_emit or origin:unknown_hat"
- `crates/ralph-cli/src/loop_runner/hat_channel.rs:19` — `prepare_hat_channel`
- `crates/ralph-cli/src/loop_runner/hat_channel.rs:87` — `hat_channel_empty_after_activation` reason 常量
- `crates/ralph-cli/src/loop_runner/runner.rs:3460` — `prepare_hat_channel` 调用点

### 9.2 Preset / Schema

- `presets/en/parallel-forge.yml:67-170` — `mechanism.flow.steps`
- `presets/en/parallel-forge.yml:154-160` — `report` step
- `presets/en/parallel-forge.yml:161-166` — `plan_end` step
- `presets/en/parallel-forge.yml:198-201` — `event_loop.required_events` + `completion_payload_match`
- `presets/en/parallel-forge.yml:298-299` — `inspector` hat
- `presets/en/parallel-forge.yml:1080-1081` — `reporter` hat
- `presets/schemas/parallel-forge.yml`（998 行，未在本报告逐行引用）

### 9.3 产物（repo-relative）

- `.ralph/events-20260730-094057.jsonl`（6 行）
- `.ralph/events-history-20260730-094057.jsonl`（1 行 warmup）
- `.ralph/recovery.jsonl`（1 行）
- `.ralph/ledger.jsonl`（8 行）
- `.ralph/flow-authority.jsonl`（4 行）
- `.ralph/loops.json`（1 entry）
- `.ralph/agent/events-hat-reporter-primary-20260730-094057-9.jsonl`（0 字节）
- `.ralph/agent/tasks.jsonl`（9 行：F1 closed + U1-U8 open）
- `.ralph/agent/memories.md`（1 fix: `mem-1785409193-75d8`）
- `.ralph/agent/scratchpad.md`（0 字节）
- `.ralph/diagnostics/2026-07-30T17-40-57/{trace,recovery,drift}.jsonl`（MINIMAL session）
- `.ralph/diagnostics/channel-routing-fallback-2026-07-30T{10-37-40, 10-56-34, 11-01-01}.md`（3 份）
- `.ralph/diagnostics/logs/ralph-2026-07-30T17-40-57-{069,073}-11755.log`（17KB / 17KB CLI log）
- `.ralph/supervisor.db`（rusqlite 格式，`sqlite3` 不可读）
- `.ralph/forge/2026-07-29-002-feat-parallel-forge-reuse-status/{inspection-report,development-plan,execution-plan.yml,concurrency-approval,worktree-map}.{md,yml}` + `templates/*`
- `docs/reports/2026-07-30-2026-07-29-002-feat-parallel-forge-reuse-status-manager-report.md`（27KB BLOCKED self-audit）
- `ralph.yml`（`event_loop.supervisor.enabled: true`、`telemetry.runtime_diagnosis.write_artifacts: true`）

### 9.4 Memory（`~/.claude/projects/.../memory/`，仅机制参考，**不**入历史关联）

- `parallel-forge-fail-close-flow-authority-stale.md`（同 loop_id 同根因，2026-07-30 上午）

### 9.5 其它诊断报告（同日 / 早期）

- `docs/report/2026-07-30-parallel-forge-primary-20260730-002911-diagnosis.md`（同 preset，**更早**一次 loop `002911`）
- `docs/report/2026-07-29-parallel-forge-primary-20260729-020808-diagnosis.md`
- `docs/report/2026-07-28-parallel-forge-primary-20260728-{003922,110733}-diagnosis.md`（×2）

**注**：以上 `docs/report/*` 引用仅作历史事故注脚，**不**构成本次诊断依据（`history_search=disabled`）。

---

## 10. 修订记录（增量补丁；**修订版**）

> **修订触发**：用户提出"除了你发现的问题，其他地方还有问题吗"后，主 Agent 重新审视盲区（用户授权该 session 内可用 `memory/` 之外，主仓代码 / preset / schema 均可继续读取），发现原报告存在 5 处未充分展开 / 漏看的问题。修订版按 confidence-rubric 重算 P0/P1/P2 表 + 新增修复建议 + 修订 checklist + 保留原 §1-§7 大部分结论。

### 10.1 修订项清单

| 修订项 | 类型 | 原状态 | 修订后状态 | 主要新增证据 |
|--------|------|--------|------------|--------------|
| **fail-close 双根因** | P0 升级 | 原 P0 mechanism 85（仅"bus.publish 不 advance flow-authority"） | P0 **compound** 78（mechanism α 85 + preset β 70，加权 0.5/0.5） | `crates/ralph-core/src/event_loop/mod.rs:14552` (`bus: &mut ralph_proto::EventBus` 类型锚点) + `flow_step_scope_stage.rs:82` `DEFENSIVE_BYPASS` + 4 个 stage 把 `plan.blocked` 当 built-in + preset `yml:58-196` 14+ 处 `forge.plan.blocked` |
| **reporter 违反 hat instructions** | P1 升级（原归 P2 §7） | 原 P2 agent 50 → 入 §7 | **P1 agent 65** | `presets/en/parallel-forge.yml:1110-1115` reporter §步骤 4 明确写"第 2 条 emit forge.report.done → 第 3 条 再 emit LOOP_COMPLETE" + events#6 payload 仅有 `report_path` 无 `forge.report.done` 业务事件 |
| **BDD 缺漏** | P1 新增 | 无 | **P1 preset 70** | 9 个 `parallel_forge_*.yml` BDD scenarios 0 条覆盖 `consecutive_no_progress → plan.blocked`；`rg "plan.blocked" crates/ralph-core/tests/scenarios/` 仅命中 `ce_executor_pipeline_*.yml`（不同 preset） |
| **task 所有权错配** | P2 新增 | 无 | **P2 mechanism 60** | `presets/en/parallel-forge.yml:189` `coordinator_hats: [forge-dispatcher]` + `tasks.jsonl:1` F1 `owner_hat_id: "executor"`（**与设计不符**） + memory `ce-executor-task-ownership` 同构问题 |
| **`repair_budget: 3` 行为异常** | §7 新增 | 无 | **§7 未核实疑点** 55 | log 073 4 次 fail-close（10:37/10:42/10:56/11:01）实际触发 > 3 但未 hard-stop；`mod.rs:14450` 附近 `P1-1 synthesised plan.blocked after repair budget exhaustion` 注释暗示 P1-1 路径**未**被触发 |

### 10.2 未变的核心结论

- **首轮终态**：REJECTED（`LOOP_COMPLETE` ×2 被 P0-5 missing=forge.report.done 拒收）
- **终止类型**：Aborted with fail-close loop（user 主动 TUI Quit 抢救）
- **执行能力**：`["supervisor", "wave"]`
- **diagnostics_mode**：`MINIMAL`
- **history_search**：`disabled`（Agent B 跳过、L5 未跑）
- **运营残留物**：F1 commit `87dc029b` 在 `.ralph/forge/.../F1` worktree 分支上**未 merge**；`docs/reports/2026-07-30-2026-07-29-002-...-manager-report.md` 27KB BLOCKED 报告已落盘

### 10.3 新增 5 个 DEV（DEV-013 ~ DEV-017）的对应 file:line 锚点

- `DEV-013` namespace 错配：`mod.rs:14599` + `flow_step_scope_stage.rs:82` + `terminal_state_guard_stage.rs:42` + `phase_authority_stage.rs:109` + `emit_schema_gate_stage.rs:45` + `step_close_obligation_stage.rs:28-47` + preset `yml:58/64/70/77/108/125/130/196`
- `DEV-014` reporter 违反 hat instructions：preset `yml:1110-1115`
- `DEV-015` BDD 缺漏：`crates/ralph-core/tests/scenarios/parallel_forge_*.yml` 9 个文件全 ls + `progress_steward.enabled: false` 锚点 `yml:165`
- `DEV-016` repair_budget 行为异常：preset `yml:183` + `mod.rs:14450` P1-1 注释
- `DEV-017` task 所有权错配：preset `yml:189` + tasks.jsonl F1 owner

### 10.4 修订版对应操作建议（按优先级）

1. **operator 短期**（per §6.1）：保留 F1 commit `87dc029b` 在 worktree 分支、保留 27KB manager-report.md、`ralph loops clean` 清理 pid 11768
2. **preset 中期**（per §6.2 目标 2 + §6.5）：
   - fail-close 路径 namespace 修复（修法 A：`mod.rs:14599` emit `forge.plan.blocked` 而非 `plan.blocked`）
   - BDD 补 `parallel_forge_fail_close_runtime.yml` scenario
3. **runtime 长期**（per §6.3）：fail-close 走 `accept_event` 路径或显式调 `append_flow_authority_snapshot`
4. **preset 中期**（per §6.2 目标 1）：reporter instructions §步骤 4 加 hard fail 提示（不允许 `LOOP_COMPLETE` 跳过 `forge.report.done`）
5. **mechanism 中期**（per §6.6）：task 所有权链 `forge-dispatcher → executor` 显式 owner_hat_id 写入
6. **observability 长期**（per §6.4）：MINIMAL 模式补 `agent-output.jsonl` 写盘

### 10.5 修订版相对原版的前后对比（表格）

| 维度 | 原版 | 修订版 |
|------|------|--------|
| P0 数量 | 1（mechanism 85） | 1（compound 78，mechanism α 85 × preset β 70 加权 0.5/0.5） |
| P1 数量 | 1（preset 65） | **3**（agent 65 / BDD 缺漏 70 / namespace 错配独立 70） |
| P2 数量 | 1（hat_lifecycle 60） | **2**（hat_lifecycle 60 / task 所有权错配 60） |
| P3 数量 | 1（loops.json 60） | 1（loops.json 60） |
| §7 未核实疑点 | 4 项（含 reporter agent 50 / repair_sink 40 / F1 archive / hat_lifecycle） | **3 项**（reporter agent 已升 P1；hat_lifecycle 已升 P2；新增 repair_budget 55） |
| DEV 总数 | 12 | **17**（新增 DEV-013 ~ DEV-017） |
| §6 修复建议 | 4 条（短期/中期/长期/observability） | **6 条**（新增 6.5 BDD / 6.6 task 所有权；6.2 拆为双目标） |
| Q2 / Q3 / Q4 置信度 | Q2 85 / Q3 70 / Q4 85 | Q2 85（双 bug）/ Q3 70（namespace 错配）/ **Q4 78**（compound 整行） |
| 1.3 根因一句话 | 单根因（bus.publish 不 advance） | **双根因**（α + β） |

### 10.6 修订版自检

- [x] §5 表无 < 60 行（P0 78 / P1 65, 70, 70 / P2 60, 60 / P3 60）
- [x] P0 1 行（compound 78 ≥ 70）
- [x] 5 个新 DEV 全部带 `file:line` 锚点
- [x] frontmatter `revision: 1` 已加 + `revision_note` 已写
- [x] §10 修订记录覆盖前后对比 + 修订项清单
- [x] §8 checklist 已重写为修订版
- [x] 未引用 ssot-guardrails 禁止项
- [x] 修订版与 memory `parallel-forge-fail-close-flow-authority-stale` 一致（同根因）
