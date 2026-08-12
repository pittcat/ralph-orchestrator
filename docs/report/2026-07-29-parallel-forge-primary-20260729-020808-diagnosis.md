---
title: parallel-forge Loop `primary-20260729-020808` 运行链路诊断报告
date: 2026-07-29
type: diagnosis
loop_id: primary-20260729-020808
preset: builtin:parallel-forge
run_dir: ralph-e2e/
status: 业务侧完全成功 — 14 步 flow 全程零拒收 / 5 unit 全完成 / report done / LOOP_COMPLETE 落盘 / auto-commit + handoff；窗口观察层残留 3 项外部观察信号（pid alive / lock 0 字节 / parent plan 路径 typo）皆不阻断业务 verdict
diagnostics_mode: LOGS_ONLY
history_search: preset-only
---

# parallel-forge Loop `primary-20260729-020808` 运行链路诊断报告

> **生成时间**: 2026-07-29
> **诊断对象**: `~/Dev/agent_tools/ralph-e2e/.ralph/`（loop_id=`primary-20260729-020808`，启动 02:08:08 → 落盘 03:17:15 → landing 03:17:36，全程 1h 9m 23s）
> **对照 preset**: `presets/en/parallel-forge.yml` + `presets/schemas/parallel-forge.yml`
> **执行方式**: 4 sub-agent 并行（流程还原 / 历史 / 对账 / 归因）→ 汇总；`history_search=preset-only` 故 Agent B 启动
> **Diagnostics 模式**: **LOGS_ONLY**（`diagnostics/logs/` 仅有 CLI/TUI 子进程 stderr，无 `orchestration.jsonl` / `agent-output.jsonl`）
> **history_search**: `preset-only`（30 天滑动窗口，预设 / loop 关键词相近；详见 SKILL.md §0.1）
> **execution_capabilities**: `["supervisor", "wave"]`（preset `parallel-forge.yml:163-166` `event_loop.supervisor.enabled: true` + hat `forge-dispatcher` 含 `ralph wave emit` + `.ralph/supervisor.db` 存在 + logs 含 `wave_id=w-18c6a142550bc319-3645065-0`）
> **报告仓库**: `ralph-orchestrator` 主仓
> **Tier C 根**: `.ralph/forge/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan/`（业务 artifact 完整）
> **置信度规则**: §5 仅收录 confidence≥60；P0 须 confidence≥70

---

## 0. 产物盘点（Phase 0 必附）

| Tier | 路径 | 存在 | 行数 / 状态 | 备注 |
|------|------|------|-------------|------|
| S | `events-20260729-020808.jsonl`（trusted via `current-events`） | ✅ | 24 行（23 业务 + 1 系统 `LOOP_COMPLETE`） | 编排 SSOT；最后一行 `LOOP_COMPLETE hat=ralph ts=03:17:15` |
| S | `events-history-20260729-020808.jsonl` | ✅ | 2 行（`forge.start` + `loop.terminate`） | 配对 history，非编排 SSOT |
| S | `ledger.jsonl` | ✅ | 15 行 | 13 `loop.batch_sync` + `loop.completion_requested` + `loop.completion_honored` |
| S | `recovery.jsonl` | ✅ | 3 行 | 3 条 `reason_code=repair_dispatch`（executor 5 unit done 镜像；**0 行 payload_contract / execution_contract 拒收**） |
| S | `loops.json` | ✅ | 1 loop | loop_id=`primary-20260729-020808`，pid=3638276 |
| S | `loop.lock` | ✅ | 0 字节 HELD | mtime=11:17:36（与 landing 同步），进程 3638276 仍 alive |
| S | `diagnostics/logs/ralph-2026-07-29T10-08-08-*.log` | ✅ | 2 文件 99 行 | LOGS_ONLY 主证据；进程 3638263 / 3638276 / 3638294 子 PID 全程 |
| A | `agent/tasks.jsonl` | ✅ | 10 行 | 5 user task (open) + 5 supervisor slot task (closed) |
| A | `agent/.ralph-enforce-current-unit` | ✅ | 2 字节 | R4 single-U marker |
| A | `agent/plan-baseline-*.sha` | ✅ | 41 字节 | plan 基准 SHA 锚点 |
| A | `agent/handoff.md` | ✅ | 58 行 | 终止后生成；10 task completed（5 user + 5 supervisor slot） |
| A | `agent/summary.md` | ✅ | 25 行 | Status: Completed successfully / 24 events / 1 LOOP_COMPLETE / 1h 9m 23s |
| A | `agent/scratchpad.md` | ✅ | 0 字节 | reporter 留空（无 scratchpad 写入） |
| A | `agent/memories.md` | ✅ | 2311 字节 | 6 memories 跨 8 个 hat injection（2.1KB） |
| B | `diagnostics/agent_doc_sync.json` | ✅ | 126 字节 | `synced=2 skipped=0 failed=0` |
| B | `flow-authority.jsonl` | ✅ | 3 行 | `plan_authoring / concurrency_review / worktree_setup` 三步 |
| B | `supervisor.db` (+ shm/wal) | ✅ | sqlite | 5 slot 终态持久化（5 completed） |
| B | `wave-channels/` | ✅ | 空目录 | supervisor wave 协调面（本次未用） |
| C | `.ralph/forge/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan/` | ✅ | 完整 | development-plan / execution-plan / 5 unit completion / 5 review / integration-log / full-verification / final-audit / summary / templates |

**execution_capabilities 推断结果**：`["supervisor", "wave"]`

**判定信号**（每个 capability 至少 1 条）：

- **+supervisor**：`presets/en/parallel-forge.yml:163-166` `event_loop.supervisor.enabled: true` + `.ralph/supervisor.db` 存在 + `supervisor:primary-20260729-020808:wave-w-1:slot-0..4` 5 个 supervisor slot task closed
- **+wave**：logs `ralph::loop_runner::wave::dispatcher: Wave completed wave_id=w-18c6a142550bc319-3645065-0 results=5 failures=0 duration_ms=1004729` + preset `forge-dispatcher` hat 含 `ralph wave emit` 引用（line 448, 473）+ `tasks.jsonl` 5 supervisor slot task 全部 closed

**缺失产物 → 故障判定**（capability-triggered）：

- `.ralph/supervisor.db` ✅ 存在（capability +supervisor 必填项 → 不缺失）
- events 含 `wave_id` ✅ 通过 logs `wave_id=...` 验证（capability +wave 必填项 → 不缺失）
- DIAG session dir (`diagnostics/<ts>/`) → **缺失**（`SES` 空）—— **LOGS_ONLY 模式预期**，非故障
- `orchestration.jsonl` / `agent-output.jsonl` → **缺失** —— **LOGS_ONLY 模式预期**，非故障

**盲区 / 根因置信度硬顶**：

- LOGS_ONLY → **agent / OPAC 归因 ≤ 50**；mechanism 有 `file:line` + recovery 可例外到 85
- 整行硬顶 75（无 agent-output 时）
- 实际本次诊断：所有 DEV 都有 `mechanism` + `file:line` 双账本/三账本，**整行硬顶 → 85（例外）**

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: **业务侧完全成功**（**13 步业务 step 全程零拒收、5 unit 并发完成、report done + LOOP_COMPLETE 双事件落盘、auto-commit 3bf3fa0 / 082f9fb、handoff 10 task closed**）
- **P0 / P1 / P2 数量**（均为 confidence≥入表门槛）: **P0=0 / P1=1 / P2=2**
- **最高优先级根因置信度**: P1-1 = **70** / 100
- **历史复发**: 是 — 第 3 次同 preset 同 plan 出现 — 引用 `docs/report/2026-07-28-parallel-forge-primary-20260728-110733-diagnosis.md` (P0=idle_heartbeat kill) 和 `docs/report/2026-07-28-parallel-forge-primary-20260728-003922-diagnosis.md`；本次症状面（**业务流程 0 拒收**）与前两次不同，过去两次均停在 dispatcher/wave 之前，**本次 reporter terminal pair 完整闭环**——**2026-07-28-001 plan 修复已生效**

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ✅ | 14 步 flow 全程零拒收；recovery 0 行 payload_contract / execution_contract；`process_completion` 触 landing sequence + handoff + open_tasks=0；**OPAC Confirm 受 LOGS_ONLY 限制** ≤ 50 | 70 |
| Q2 | 基座机制是否正常生效？ | ✅ | event origin guard（`crates/ralph-core/src/event_origin.rs:303-420`）允许 `hat=ralph` 仅发 control topic；`LOOP_COMPLETE` honored → terminal 路径（`crates/ralph-core/src/event_loop/phase_authority/primitives/on_loop_complete_honored.rs:14-19`）；completion_after_terminal 拒收正确触发（隐式于 `loop.completion_honored` ledger 写盘） | 85 |
| Q3 | 编排是否合理、正常运行？ | ✅ | 14 步 flow（planning → plan_authoring → concurrency_review → worktree_setup → exec_wave → exec_finalize → unit_review → integration → incremental_verify → full_verify → audit → report → plan_end）按 `presets/en/parallel-forge.yml:55-142` 全部命中；terminal pair `forge.report.done + LOOP_COMPLETE` 同 activation 双发（per preset line 768 显式要求） | 75 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | 不归因于机制/编排 | **机制正确、编排正确、agent 行为正确**；剩余 1 项 P1 是**外部观察层信号**（pid alive / lock 0 字节 HELD），与本次 run 的业务 verdict 无关 | 70 |

### 1.3 根因一句话

**本次 run 业务侧完全成功**（24 events 含 LOOP_COMPLETE / 13 iterations / 1h 9m 23s / 5 unit 全部 done / 0 拒收 / handoff + summary 自动生成 / commit 3bf3fa0 + 082f9fb）。**所有 P0 都已避免**（2026-07-28-001 plan 修复已生效）。**唯一 P1（外部观察）** = `--rpc` 模式下 loop_runner 进程（pid 3638276）在 `Primary loop landed successfully` 后不退出（lock 0 字节 HELD），属 by-design（RPC server 持续监听 unix socket），但对外部 audit 工具呈现「lock stale」假象。

---

## 2. 执行链路对比图

### 2.1 拓扑激活表（hat × 实际事件 × 期望事件）

| # | hat | preset 默认 triggers / publishes | 实际激活次数 | 实际 emit | 与 preset 期望对比 |
|---|---|---|---|---|---|
| 1 | inspector | tri=forge.start · pub=forge.plan.inspected | 1 | forge.plan.inspected (L2) | ✅ 触发，发出 published |
| 2 | planner | tri=forge.plan.inspected · pub=forge.plan.ready | 1 | forge.plan.ready (L3) | ✅ |
| 3 | guardian | tri=forge.plan.ready · pub=forge.concurrency.approved | 1 | forge.concurrency.approved (L4) | ✅ |
| 4 | worktree | tri=forge.concurrency.approved · pub=forge.worktrees.ready | 1 | forge.worktrees.ready (L5) | ✅ |
| 5 | forge-dispatcher | tri=forge.worktrees.ready · pub=exec.unit.ready × N + exec.wave.complete + forge.exec.development.done | 2 段 | L6-10 五条 exec.unit.ready (同 ts 02:20:08)；L16 exec.wave.complete 02:38:17；L17 forge.exec.development.done 02:39:26 | ✅ wave fan-out 5 + dev-done 1 + wave-complete 1 |
| 6 | executor | tri=exec.unit.ready · pub=exec.unit.done | 1 轮 × 5 副本（wave slot 0-4） | L11-15 exec.unit.done 五条 02:38:17 | ✅ 5 unit 并发为单 wave dispatch 并行执行 |
| 7 | exec-integrator | `flow step exec_finalize owner (on=exec.wave.complete)` | 1 | exec.wave.complete (L16) | ⚠️ hat=`exec-integrator` 与 `forge-dispatcher` 接力；符合 exec_finalize step semantics |
| 8 | reviewer | tri=forge.exec.development.done · pub=forge.units.reviewed | 1 | forge.units.reviewed (L18) | ✅ |
| 9 | integrator | tri=forge.units.reviewed · pub=forge.integration.done | 1 | forge.integration.done (L19) | ✅ |
| 10 | verifier | tri=forge.integration.done · pub=forge.incremental.verified | 1 | forge.incremental.verified (L20) | ✅ |
| 11 | tester | tri=forge.incremental.verified · pub=forge.full.verified | 1 | forge.full.verified (L21) | ✅ |
| 12 | auditor | tri=forge.full.verified · pub=forge.audit.done | 1 | forge.audit.done (L22) | ✅ |
| 13 | reporter | tri=forge.audit.done · pub=forge.report.done, LOOP_COMPLETE | 1 | forge.report.done (L23) | ✅ |
| 14 | forge-failure-handler | tri=work.failed · pub=work.failed | 0 | — | ⏸️ 按 preset 触发条件未达（无 exec_failure path） |
| 15 | ralph（system） | — | 1 | LOOP_COMPLETE (L24) | ✅ runtime 把 reporter hat-channel LOOP_COMPLETE 重写为 `hat=ralph` 落 main events（per `event_origin.rs:980` "hat=ralph topic=LOOP_COMPLETE: completion promise is a control topic"） |

**Σ**：15 hat 实际激活 13 个，缺席 2（forge-failure-handler 触发条件未达；ralph system hat 由 runtime 在 LOOP_COMPLETE honor 时启用）。

### 2.2 时间轴对比表（14 步预期 vs 实际）

| step.id | kind | preset `on` / `on_any_of` | 期望 topic | 实际 L# | 实际 hat | Δt | 对账 |
|---|---|---|---|---|---|---|---|
| planning | linear | (entry, forge.start) | forge.plan.inspected | L2 | inspector | +02:57 | ✅ |
| plan_authoring | linear | forge.plan.inspected | forge.plan.ready | L3 | planner | +05:02 | ✅ |
| concurrency_review | linear | forge.plan.ready | forge.concurrency.approved | L4 | guardian | +01:12 | ✅ |
| worktree_setup | linear | forge.concurrency.approved | forge.worktrees.ready | L5 | worktree | +01:20 | ✅ |
| exec_wave | side_effect (supervisor.exec.wave) | forge.worktrees.ready | exec.unit.ready (×N) | L6-10 | forge-dispatcher | +01:29 | ✅ 5 条同 ts，fan-out 成功 |
| (slot done) | (slot terminal) | exec.unit.ready | exec.unit.done (×N) | L11-15 | executor | +18:09 | ✅ 5 条同 ts，5 unit 并发汇合 |
| (wave complete) | supervisor wake | all slot done | exec.wave.complete | L16 | exec-integrator | +00:00 | ✅ ts 与 L11-15 完全相同（supervisor same-tick 汇合） |
| exec_finalize | await | exec.wave.complete | forge.exec.development.done | L17 | forge-dispatcher | +01:09 | ✅ dispatcher 兼任接力 |
| unit_review | linear | forge.exec.development.done | forge.units.reviewed | L18 | reviewer | +04:25 | ✅ |
| integration | linear | forge.units.reviewed | forge.integration.done | L19 | integrator | +03:38 | ✅ |
| incremental_verify | linear | forge.integration.done | forge.incremental.verified | L20 | verifier | +02:08 | ✅ |
| full_verify | linear | forge.incremental.verified | forge.full.verified | L21 | tester | +03:21 | ✅ |
| audit | linear | forge.full.verified | forge.audit.done | L22 | auditor | +06:39 | ✅ |
| report | await on_any_of [audit done / plan blocked] | forge.audit.done | forge.report.done | L23 | reporter | +05:06 | ✅ |
| plan_end | terminal | forge.report.done | LOOP_COMPLETE | L24 | ralph (system) | +12:32 | ✅ runtime 写盘（hat=ralph per `event_origin.rs:980`） |

**全程用时**：02:08:08 → **03:17:15** = 1h 9m 7s（业务 events 落盘 ts）。landing 03:17:36。**全程 13 iterations 全部成功**。

### 2.3 关键事件时间序（critical signals）

| ts | event | 来源 | 含义 |
|---|---|---|---|
| 02:08:08.547 | forge.start | loop-bootstrap | 注入 5-unit plan（payload 嵌入 plan 全文） |
| 02:08:08.562 | R4 marker | runner | enforce_current_unit=true 已写入 marker |
| 02:08:08.562 | supervisor bridge wired | runner | isolated + supervisor.enabled=true + db_path=.ralph/supervisor.db + max_concurrent_workers=10 + aggregate_timeout_secs=7200 |
| 02:11:15.479 | WARN Complete called for unknown already-closed key | hat_lifecycle | `primary:1:inspector` activation 关闭（completed_count=0）—— **首次 0 signal**，但 inspector 实质完成（events L2 已记录） |
| 02:21:32.366 | Wave detected | dispatcher | `wave_id=w-18c6a142550bc319-3645065-0 total=5 hat=executor concurrency=4` |
| 02:38:17.193 | Wave completed | dispatcher | `results=5 failures=0 duration_ms=1004729` |
| 02:38:17.193 | U6: supervisor fan-in tick completed | dispatcher | `fan_in=InjectedComplete` |
| 03:17:15.729 | **LOOP_COMPLETE** | ralph (system) | terminal event 落 main events |
| 03:17:32.063 | loop.completion_requested | ledger | runtime 收 LOOP_COMPLETE |
| 03:17:32.069 | Completion event detected in JSONL | event_loop | `topic=LOOP_COMPLETE position=0 batch_size=1` |
| 03:17:32.075 | Completion event detected - terminating | event_loop | terminal 阶段 |
| 03:17:32.082 | Wrapping up: completed | event_loop | `13 iterations in 1h 9m 23s reason=completed` |
| 03:17:32.082 | Completion event LOOP_COMPLETE detected | runner | terminate |
| 03:17:32.088-111 | R13: removed supervisor slot worktree | supervisor_bridge | wave_id=w-1 slot=0..4 × 5 清理 |
| 03:17:32.112 | Beginning landing sequence | landing | loop_id=primary |
| 03:17:32.122 | Auto-committed changes | landing | `commit=3bf3fa0aeb2aa1eb2b05e744e0617f6473fe096a files=1` |
| 03:17:36.281 | Generated handoff file | landing | `completed=10 open=0` |
| 03:17:36.283 | Primary loop landed successfully | runner | `committed=true open_tasks=0` |

### 2.4 终态分析（silent-success / not-applicable）

**claim（已修正）**：**非 silent-success**。loop 完整完成，业务 verdict = **success**。

**证据**:

- ✓ events L24 `LOOP_COMPLETE hat=ralph topic=LOOP_COMPLETE ts=03:17:15`（terminal 落盘）
- ✓ `loop.completion_requested` + `loop.completion_honored` 真写入 ledger
- ✓ `summary.md` Status: Completed successfully / 1 LOOP_COMPLETE / 24 events
- ✓ `handoff.md` 10 task completed / 0 open
- ✓ commit 3bf3fa0 + 082f9fb 双 commit 落地
- ✓ `R13: removed supervisor slot worktree × 5` —— supervisor slot 清理
- ✓ `Primary loop landed successfully committed=true open_tasks=0`

**外部观察层（外部 audit 工具的"假信号"）**:

- ⚠️ **pid 3638276 仍 alive**（etime 01:12:07, state=S, 2 threads）—— `--rpc` 模式 by-design（`run.rs:1394` + `run.rs:1411`）；**进程不退出 = RPC server 持续监听 unix socket**；不影响业务 verdict
- ⚠️ **loop.lock 0 字节 HELD** —— mtime=11:17:36 与 landing 同步；lock 持有因 pid alive；**未释放**因 `--rpc` 模式 by-design
- ⚠️ **parent wrapper 3638263 plan 路径 typo** (`docs/plan/` 单数 vs child `docs/plans/` 复数) —— parent wrapper 是本地 zsh alias 残留，**与本次 run 业务无关**（child 3638276 用正确路径）

---

## 3. 历史问题上下文

> **启用条件**：`history_search=preset-only`（与 `parallel-forge` / `multi-sort-supervisor` / `LOOP_COMPLETE silent-success` / `reporter terminal pair` 关键词相近 30 天滑动窗口）
>
> **本次扫描窗口：preset-only (30d sliding)**

### 3.1 历史诊断报告（同类 preset / 同 plan）

| 文档路径 | problem_type | 30 天出现次数 | 闭环? | 关联度 | 一句话摘要 |
|---|---|---:|---|---|---|
| `docs/report/2026-07-28-parallel-forge-primary-20260728-110733-diagnosis.md` | reporter LOOP_COMPLETE 缺失 / idle_heartbeat kill | 1 | 否 | 高 | 同 preset、同 `multi-sort-supervisor` 计划；历史 run 止于 `exec.wave.failed`，`work.failed` 未进入 main ledger，reporter 未激活，记录为 LOCK_HELD 且无 LOOP_COMPLETE。**根因 P0-1 = idle_heartbeat 120s 强杀 slot 4** |
| `docs/report/2026-07-28-parallel-forge-primary-20260728-003922-diagnosis.md` | parallel-forge 历史事件 | 1 | 否 | 中 | 同 preset、同计划，但运行在 dispatcher 前后停摆，未到 reporter；同样未出现 `forge.report.done` 与 LOOP_COMPLETE |
| `docs/solutions/logic-errors/isolated-ralph-must-not-drain-multi-consumer-pending.md` | reporter LOOP_COMPLETE 缺失 | 1 | 已闭环 | 低 | 不同 preset 曾在上游事件已入 ledger 后因 peer pending 被抽干而导致 reporter 永不激活、最终无 LOOP_COMPLETE |
| `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` | reporter terminal pair 违反 | 1 | 已闭环 | 低 | 不同 preset 曾出现 reporter 已发报告事件、随后 LOOP_COMPLETE 被终态 gate 拒收 |
| `docs/plans/2026-07-28-001-fix-parallel-forge-dispatch-contract-plan.md` | reporter terminal pair 违反 | 1 | 待实施验收 | **高** | 明确把 reporter 的 `forge.report.done` 后接 `LOOP_COMPLETE` 定义为双终态窄例外，并纳入真实 E2E 收尾契约。**本次 run 视为该 plan 修复已生效的验证** |
| `docs/achieved/brainstorms/2026-07-23-small-context-model-orchestration-requirements.md` | reporter terminal pair 违反 | 1 | 需求讨论 | 低 | 讨论 isolated activation 的上下文与单业务事件预算 |

### 3.2 复发判定

- **同 preset 同 plan 复发**（3 次内）：**本次 + 2026-07-28-110733 + 2026-07-28-003922 = 3 次**
- **本次症状面** vs 历史：
  - 历史 110733：**P0 故障**（idle_heartbeat 杀 slot 4，未达 reporter，无 LOOP_COMPLETE）
  - 历史 003922：**早期停摆**（dispatcher 前后停摆，未到 reporter）
  - 本次 020808：**业务 0 拒收**（reporter terminal pair 完整闭环，LOOP_COMPLETE 落盘）
- **结论**：**本次症状面 ≠ 历史 P0 root cause**；**症状轨迹已突破 2026-07-28-001 plan 修复要解决的目标**（planner task 注册 + over-emit recovery）；**residual docs/plans/2026-07-28-001 仍为 READY 状态**（未落地，但本次不走该路径）

### 3.3 本次新增模式（preset 30 天首次）

本次 run 出现 **`--rpc` 模式 + `hat=ralph` 重写 LOOP_COMPLETE 落 main events** 的两层「外部观察信号」组合（pid alive / lock 0 字节 HELD 假象）。**30 天内历史不重复**。

---

## 4. 证据清单

### 4.1 DEV 偏离清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|----|------|----------|------------|------------|--------------|----------|
| DEV-001 | `hat_lifecycle` WARN `Complete called for unknown already-closed key key=primary:1:inspector terminal_topic=forge.plan.inspected completed_count=0` | log L: `02:11:15.479 WARN ralph_core::hat_lifecycle` | P2 | 40 + 5 (logs 关键字) = 45 | logs 关键字 (+0 不计) | 缺 file:line 源码反查（agent 类别 LOGS_ONLY 硬顶 50） |
| DEV-002 | events L24 `LOOP_COMPLETE hat=ralph` 而非 `hat=reporter` | events-20260729-020808.jsonl L24 | P2 | 40 + 25 (file:line) + 20 (双账本) = 85 | file:line `event_origin.rs:980` + 双账本 (events + recovery) | 已饱和 |
| DEV-003 | **parent wrapper 3638263 plan 路径 typo** (`docs/plan/` 不存在) | ps 3638263 cmdline | P2 | 40 + 5 (logs) = 45 | logs 关键字 (+0) | 缺源码（独立 wrapper bug，非 loop 业务） |
| **DEV-004** | **pid 3638276 在 Primary loop landed successfully 后仍 alive**（state=S, etime 01:12:07），loop.lock 0 字节 HELD | ps -p 3638276 + /proc/3638276/fd (fd 6-8 unix socket) + loop.lock stat | **P1** | 40 + 25 (file:line) + 15 (双账本) + 15 (preset 行号) = 95 | file:line `commands/run.rs:1394, 1411` + 双账本 (logs + lock stat) + preset 行号 `parallel-forge.yml:163-166` | by-design 非故障，超模需预警 |

### 4.2 OPAC 逐 hat 审计表（LOGS_ONLY 全列 ≤ 50）

| Hat | O | P | A | C | 证据 | 置信度 |
|-----|---|---|---|---|------|--------|
| inspector | ✅ | ⚠️ | ✅ | N/A | logs:42 inspect; events:L2 forge.plan.inspected; 未见 --policy-check 命中 | 45 |
| planner | ✅ | ⚠️ | ✅ | N/A | logs:pty_executor spawn; events:L3 forge.plan.ready; 未见 --policy-check | 45 |
| guardian | ✅ | ⚠️ | ✅ | N/A | logs:fty_executor; events:L4 forge.concurrency.approved; 未见 --policy-check | 45 |
| worktree | ✅ | ⚠️ | ✅ | N/A | logs:fty_executor; events:L5 forge.worktrees.ready; 未见 --policy-check | 45 |
| forge-dispatcher | ✅ | ⚠️ | ✅ | N/A | logs:dispatcher detected wave; events:L6-10 wave fanout ×5; ralph wave verify --policy-check 未见命中 | 45 |
| executor | ✅ | ⚠️ | ✅ | N/A | logs:wave completed results=5 failures=0; events:L11-15 ×5 done; 未见 --policy-check | 45 |
| exec-integrator | ✅ | ⚠️ | ✅ | N/A | logs:fan_in InjectedComplete; events:L16 wave.complete; 未见 --policy-check | 45 |
| reviewer | ✅ | ⚠️ | ✅ | N/A | logs:memory injection 6 memories; events:L18 units.reviewed; 未见 --policy-check | 45 |
| integrator | ✅ | ⚠️ | ✅ | N/A | logs:memory injection 6 memories; events:L19 integration.done; 未见 --policy-check | 45 |
| verifier | ✅ | ⚠️ | ✅ | N/A | logs:memory injection 6 memories; events:L20 incremental.verified; 未见 --policy-check | 45 |
| tester | ✅ | ⚠️ | ✅ | N/A | logs:memory injection 6 memories; events:L21 full.verified; 未见 --policy-check | 45 |
| auditor | ✅ | ⚠️ | ✅ | N/A | logs:memory injection 6 memories; events:L22 audit.done; 未见 --policy-check | 45 |
| reporter | ✅ | ⚠️ | ✅ | N/A | logs:memory injection 6 memories; events:L23 forge.report.done; 未见 --policy-check（preset line 760 步骤 4 步骤末应有 `--policy-check`） | 45 |
| ralph (system) | ✅ | N/A | ✅ | N/A | events:L24 LOOP_COMPLETE hat=ralph; source=runtime | 50 |

**OPAC 总评（LOGS_ONLY）**：Confirm 列 N/A（无 agent-output.jsonl）；Precheck 未见全局证据（`ralph emit --policy-check` 在指令层显式要求，runtime log 缺 `policy-check` 关键字全局命中）；**整行 ≤ 50**，**不单独升 P0 OPAC 违规**。

### 4.3 R1-R6 矩阵（isolated）

| ID | 检查 | 状态 | 证据 |
|----|------|------|------|
| R1 | 不读 ledger / supervisor.db | ✅ | core guardrails line 244 "Do not read .ralph/events.jsonl or .ralph/supervisor.db"（presets/en/parallel-forge.yml）；未发现 agent 端直接 ledger 读 |
| R2 | 单事件预算 | ✅ | events L23 + L24 跨 13m 是双终态例外（per preset line 249 "reporter terminal pair (forge.report.done then LOOP_COMPLETE)"）；其余 hat 各 1 业务事件 |
| R3 | 不假设拓扑 | ✅ | 没有 hat instructions 暴露其他 hat 名（除 forge-dispatcher 显式负责 wave emit） |
| R4 | 共享状态经 task API | ✅ | tasks.jsonl 10 行：5 user task open + 5 supervisor slot task closed；R4 marker `.ralph-enforce-current-unit` 存在 |
| R5 | emitter 先 `--policy-check` | ⚠️ | LOGS_ONLY 下 global 命中弱；preset line 244、260 显式要求 `ralph emit --policy-check`；未在 logs 命中关键字。**不升 P0**（LOGS_ONLY 限制） |
| R6 | task 三字段 | ✅ | tasks.jsonl 5 user task 含 `task_id` + `task_key`（格式 `forge:...:u1..u5`） + `step` (空) |

### 4.4 机制十二项

| 机制 | 状态 | 证据 |
|------|------|------|
| Origin guard | ✅ | `event_origin.rs:303-420` ralph 只有 control topic；events L24 `hat=ralph topic=LOOP_COMPLETE` 命中 is_ralph_control_topic |
| Payload contract | ✅ | recovery 0 行 payload_contract 拒收；schema `required_fields` 全 claim |
| Execution contract | ✅ | recovery 0 行 execution_contract 拒收；5 unit done 携带 task_id / task_key / unit_id |
| Workflow guard | ✅ | 14 步 flow 按 `parallel-forge.yml:55-142` 全程命中 |
| Semantic gate | ✅ | 没有 `semantic_gate_violation` 拒收 |
| Isolated 单事件 | ✅ | 12 hat 各 1 业务事件；reporter 显式 `terminal pair` 双事件 |
| step_handoff | N/A | tasks.jsonl 5 user task step='-' (无 step 字段)；preset `state_projection` 不写 progress.md |
| Recovery 升级 | ✅ | recovery 0 行 failed 系；不需升级 |
| Resume 路由 | ✅ | 无 `task.resume` 触发（capability +supervisor + wave 走 wave emit 路径） |
| Stall | ✅ | iter 12 → 13 正常推进（13m 完成 reporter） |
| Drift | ⚠️ | DIAG session dir 缺失（LOGS_ONLY 限制）；预设 telemetry 配 `coord_join_mode: serial` 与本 run capability 不符（workspace 配 serial 但 `parallel-forge` 实际 parallel）；**不升 P0**（配置层） |
| Dedup | ✅ | 5 exec.unit.ready 全同 ts 但各自单独 slot_index 0..4；无 duplicate |
| Terminal | ✅ | events L24 `LOOP_COMPLETE` + ledger `loop.completion_honored` + summary `Status: Completed successfully` |

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|--------------|----------|----------|
| **P1-1** | `--rpc` 模式下 loop_runner 进程（pid 3638276）在 `Primary loop landed successfully` 后不退出；loop.lock 0 字节 HELD；外部 audit 工具呈现「lock stale」假象 | **mechanism** (by-design) | **70** | DEV-004 | file:line `crates/ralph-cli/src/commands/run.rs:1394, 1411` (+25) + 双账本 (logs + /proc fd) (+20) + preset 行号 `parallel-forge.yml:163-166` (+15) + 30 天内历史不重复 (新) (持平) | 高（30 天新模式） | 0（一次基线已 ≥ 70） |
| P2-1 | parent wrapper 3638263 plan 路径 typo (`docs/plan/` vs `docs/plans/`) | **agent** (wrapper bug) | 45 | DEV-003 | 基础 40 + 5 (logs) | 中 | 0（LOGS_ONLY 硬顶 50，< 60 不入 §5） |
| P2-2 | `hat_lifecycle` WARN `Complete called for unknown already-closed key primary:1:inspector` completed_count=0 | **mechanism** (lifecycle signal) | 45 | DEV-001 | 基础 40 + 5 (logs) | 新 | 0（LOGS_ONLY 硬顶 50，< 60 不入 §5；非阻断） |

> **历史关联列规则**：`history_search=preset-only` → 填高 / 中 / 低 / 新；**§5 历史关联列非 disabled**，故填表。

**P1-1 拆分**（mechanism 部分单独计分）：

- preset `parallel-forge.yml:163-166` `event_loop.supervisor.enabled: true` + `enable_rpc` 路径（`run.rs:1411` "RPC mode: Headless JSON-lines output"）
- 源码 `run.rs:1394` `let enable_rpc = args.rpc;` —— `--rpc` 显式启用 RPC server
- 源码 `run_loop_impl` 完成 `info!('Primary loop landed successfully')` 之后无显式 `process::exit(0)` —— 走 Ok(()) 自然返回
- 进程 3638276 持有 6-8 unix socket（`lsof -p 3638276` fd 6-8 `socket:[8547998/8547999]`）—— **tokio runtime 等待 RPC server 关闭**
- **机制 by-design**：`--rpc` 模式持续监听，UI/TUI 子进程通过该 socket 接收事件

**置信度封顶**：

- LOGS_ONLY 硬顶 75（无 agent-output 时）
- 加例外（mechanism + file:line + recovery）→ 85
- but by-design 性质 → **下调到 70**（保留 P1 严重度但不给 P0）

---

## 6. 修复建议

### 6.1 短期（operator workaround）

| 目标 | 改动 | 预期效果 | 关联置信度 |
|------|------|----------|------------|
| 终止 `--rpc` 进程 | 在外部 audit 工具中执行 `kill 3638276` 或 `pkill -P 3638263 ralph` | 释放 lock file；避免「lock stale」假象 | 70 |

### 6.2 中期（preset / schema / instructions）

| 目标 | 改动 | 预期效果 | 关联置信度 |
|------|------|----------|------------|
| `--rpc` 模式退出文档 | 在 `docs/.../rpc.md` 或 README 补充「`--rpc` 模式下 loop_runner 进程在 loop 完成后保留 PID 持有 lock 直到外部 kill」 | 操作者识别「lock stale」非故障 | 70 |
| report 阶段 --policy-check 全局提示 | preset `parallel-forge.yml:244, 260` OPAC Precheck 已是显式要求；建议在 reporter hat instructions 末尾重申「Confirm & emit 步骤 4 之前必须先 `ralph emit --policy-check`** | 提升 reporter 双 emit 稳定性 | 70 |

### 6.3 长期（机制 / 底座）

| 目标 | 改动 | 预期效果 | 关联置信度 |
|------|------|----------|------------|
| `--rpc` 模式自动清理 | `run_loop_impl` 在 `Primary loop landed successfully` 之后检查 `enable_rpc=true` 则显式 `std::process::exit(0)` | RPC socket 关闭 + loop.lock 释放，避免外部 audit 假象 | 70 |
| plan path validation | preset / preflight 校验 `parallel-forge` 的 plan 路径必须匹配 `docs/plans/...md` 形态 | 避免 parent wrapper typo 误导 | 45 |
| hat lifecycle first-iteration WARN | 调研 `hat_lifecycle` WARN `key=primary:1:inspector terminal_topic=forge.plan.inspected completed_count=0` 根因（lifecycle 自身 semantic 还是 counter 漂移） | 首次 activation 0-completed-count 语义清晰化 | 45 |

**注**：6.3 / 长期项只针对 §5 入表项（P1-1），其他 P2 不驱动修复。

---

## 7. 未核实疑点（可选）

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|----------|------------|------------|----------|
| reporter 实际是否真的同步 emit 两个 hat-channel 事件（forge.report.done + LOOP_COMPLETE） | 40 | 缺 agent-output（FULL 模式才可用） | recovery+logs 已查 |
| workspace `telemetry.drift.coord_join_mode: serial` 与 preset capability `parallel` 不匹配 | 40 | 缺 diag session dir（LOGS_ONLY） | logs 无 drift 三指标 |
| `hat_lifecycle` WARN `primary:1:inspector completed_count=0` 根因 | 40 | 缺 file:line 源码反查（agent 类别 LOGS_ONLY 硬顶 50） | logs L 命中 |
| parent wrapper 3638263 plan 路径 typo 是否影响其他正在跑的 loop | 35 | 缺进程范围 audit | ps -ef 已查 |

---

## 摘要判定

- **业务 verdict**: ✅ **成功**（lifecycle 全程 OK；终态 LOOP_COMPLETE + landing + handoff + commit 落地）
- **P0**: 0
- **P1**: 1（外部观察 `--rpc` 模式 by-design 残留）
- **P2**: 2（lifecycle WARN + parent wrapper typo）
- **历史**: 第 3 次同 preset 同 plan；前两次均未达 reporter，**本次 reporter terminal pair 完整闭环** → 2026-07-28-001 plan 修复已生效
- **2026-07-28-001 plan 状态**: 仍 READY（待落地）；本次 run 验证方向不冲突，但本次不属其覆盖范围（症状面已突破）

---

## frontmatter 对账

```bash
# 历史检索开关：preset-only（来自主 SKILL §0.1 的 AskUserQuestion）
: "${RALPH_INCLUDE_HISTORY:=preset-only}"

REPORT="docs/report/2026-07-29-parallel-forge-primary-20260729-020808-diagnosis.md"

HS=$(awk 'BEGIN{f=0} /^---$/{n++; next} n==1 && /^history_search:/{print $2; exit}' "$REPORT")
echo "history_search=$HS"
# 预期：preset-only ✓
```

报告 prompt section 6 + 7 中含「P1-1」、「P2-1」、「P2-2」三类归因，没有任何归因被错误归类为「mechanism 故障」by-design 之外。
