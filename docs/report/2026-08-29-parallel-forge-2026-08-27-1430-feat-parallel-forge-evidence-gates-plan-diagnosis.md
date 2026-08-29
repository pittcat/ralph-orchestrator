---
title: parallel-forge Loop `2026-08-27-1430-feat-parallel-forge-evidence-gates-plan` 运行链路诊断报告
date: 2026-08-29
type: diagnosis
loop_id: 2026-08-27-1430-feat-parallel-forge-evidence-gates-plan
preset: builtin:parallel-forge
run_dir: worktree/ralph-orchestrator/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan
status: 部分偏离 — Wave 1+2 完整（含 1 轮 correction）；Wave 3 dispatch 后 stall（5 slot 派发，3 succeeded/2 running，exec.unit.done 未落账，PID 外部 kill，reuse-history 捕获 salvage_write_count=0）
diagnostics_mode: MINIMAL
bundle: pending
bundle_path: .ralph/diagnostics/2026-08-29T13-55-44/diagnosis-input.json
causal_status: not_evaluable
causal_confidence: 0
causal_primary_domain: N/A
causal_rejected_hypotheses: []
causal_score_change: N/A (initial scoring; round 1 + round 2 均 not_evaluable)
history_search: preset-only
structured_result_ref: "inline: summarized in report"
trace_status: missing
feedback_status: missing
activation_outcomes: missing
evidence_gaps:
  - runtime-trace.jsonl empty (sidecar never written)
  - feedback.jsonl empty (sidecar never written)
  - orchestration.jsonl not found in session
  - errors.jsonl not found in session
  - bundle identity 全 null (loop_id/preset/config/plan/baseline 均为 null)
  - cap manifest execution_capabilities 空 (MINIMAL 模式未生成完整 cap 表)
---

# parallel-forge Loop `2026-08-27-1430-feat-parallel-forge-evidence-gates-plan` 运行链路诊断报告

> **生成时间**: 2026-08-29 (Asia/Shanghai)
> **诊断对象**: `worktree/ralph-orchestrator/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan/.ralph/`（loop_id=`2026-08-27-1430-feat-parallel-forge-evidence-gates-plan`，启动 2026-08-29 13:55:44 → 终止 stalled @ 15:34:18，PID 526040 外部 kill @ ~16:23）
> **对照 preset**: `presets/en/parallel-forge.yml` + `presets/schemas/parallel-forge.yml`
> **执行方式**: 4 sub-agent 并行（流程还原 / 历史 / 对账 / 归因）→ 汇总
> **Diagnostics 模式**: MINIMAL（bundle `manifest_status: pending` + 4 个 sidecar 缺失或空）
> **history_search**: `preset-only`（30 天滑动窗口；2026-07-30 → 2026-08-29）
> **execution_capabilities**: `["supervisor", "wave"]`（推断：forge.yml `event_loop.supervisor.enabled: true` + hat 含 `ralph wave emit` 拓扑 + `.ralph/supervisor.db` 存在 + events 含 `wave_id=w-18d0366e1ab3ce34-795108-0`）
> **报告仓库**: `ralph-orchestrator` 主仓（非 run_dir）
> **置信度规则**: §5 仅收录 `status == complete`（DT7 机检 confidence > 85）。`causal_status: not_evaluable` + `diagnostics_mode: MINIMAL`（硬顶 ≤ 75）共同决定 §5 入表门槛不可达。**§5 表为空；全部 P0 候选落入 §7**（per confidence-rubric DT7）。

---

## 0. 产物盘点（Phase 0）

| Tier | 路径 | 存在 | 行数 / 大小 | 备注 |
|------|------|------|-------------|------|
| **S** | `worktree/.../.ralph/reuse-history/20260829T082300.206561090Z/events-20260829-055544.jsonl`（current-events 指向） | ✅ | 37 行 / 92 KB | **唯一** events 文件（per HARD RULE "禁止 events*.jsonl 通配"）。覆盖 `forge.start` → 5× wave-3 `exec.unit.ready`，缺 w-3 任何 `exec.unit.done` |
| S | `worktree/.../.ralph/ledger.jsonl` | ✅ | 42 行 / 22 KB | supervisor ledger；iter 1–21，末行 `loop.observation.21` @ 2026-08-29T07:34:18Z（`forge.wave.worktrees.ready`） |
| S | `worktree/.../.ralph/agent/accepted-transitions.jsonl` | ✅ | 26 行 / 12 KB | accepted transitions；iter 1–21，末行 `worktree:21 forge.wave.worktrees.ready` |
| S | `worktree/.../.ralph/supervisor.db` (sqlite) | ✅ | 1.2 MB WAL | `waves`(3) / `wave_slots`(10) / `slot_resources`(10) / `slot_attempts`(10) / `dispatch_records`(10) / `worker_results`(0) / `wave_emissions`(0) / `redrive_requests`(0) / `compensation_jobs`(0)；w-3 `phase=collect, delivery_state=pending` |
| A | `worktree/.../.ralph/loops.json` | ✅ | 591 B | PID 526040, started 2026-08-29T05:55:44Z |
| A | `worktree/.../.ralph/agent/context.md` + `resume-context.md` | ✅ | 1.7 KB + 351 B | worktree setup context + reuse-history advisory |
| A | `worktree/.../.ralph/forge/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan/{inspection-report,development-plan,execution-plan,concurrency-approval,integration-log,worktree-map}.{md,yml}` | ✅ | 6 文件 / ~150 KB | forge artifacts；`units/` 含 7 个 completion.md（U01/U02/U03/U04/U06/U07/U10），U06/U07 在 15:57/16:01 落盘但无对应 exec.unit.done |
| A | `worktree/.../.ralph/reuse-history/20260829T082300.206561090Z/{events-...,flow-authority,parallel-forge-resume-manifest.v1,history}.jsonl/.json` | ✅ | 6 文件 / ~270 KB | reuse-history 归档，captured_at=2026-08-29T08:23:00Z（晚于最后事件 49 min） |
| B | `worktree/.../ralph.forge.yml` | ✅ | 4.6 KB | parallel-forge + supervisor.enabled=true + runtime_diagnosis.enabled=true |
| B | `worktree/.../docs/plans/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan.md` | ✅ | 67 KB | evidence-gates plan; status: active; origin: `docs/brainstorms/2026-08-12-003-...` |
| B | git: `.worktrees/U01`–`U10` + 12 个 slot worktrees | ✅ | 14 worktree HEADs | wave 3 slot 0/1/2/3/4 HEAD: e95698f1 / 8dfba463 / e2acf6a2 / e95698f1 / 838c4b66; slot 0/3 有未提交变更（4 / 9 uncommitted） |
| **C** | `.../diagnostics/2026-08-29T13-55-44/diagnosis-input.json` | ✅ | 602 B | `manifest_status: pending`, identity 全 null |
| C | `.../diagnostics/2026-08-29T13-55-44/runtime-trace.jsonl` | ✅ | **0 字节** | empty sidecar; runtime 视为 `Missing` |
| C | `.../diagnostics/2026-08-29T13-55-44/feedback.jsonl` | ✅ | **0 字节** | empty sidecar |
| C | `.../diagnostics/2026-08-29T13-55-44/recovery.jsonl` | ✅ | 0 字节 | empty |
| C | `.../diagnostics/2026-08-29T13-55-44/drift.jsonl` | ✅ | 0 字节 | empty |
| C | `.../diagnostics/2026-08-29T13-55-44/trace.jsonl` | ✅ | 7 行 / 2 KB | 仅初始 subprocess spawn（child PID 526040） |
| C | `.../diagnostics/2026-08-29T13-55-44/orchestration.jsonl` | ❌ | — | 不存在（warning） |
| C | `.../diagnostics/2026-08-29T13-55-44/errors.jsonl` | ❌ | — | 不存在（warning） |

**execution_capabilities 推断结果**: `["supervisor", "wave"]` — 判定信号：

- `supervisor`: forge.yml `event_loop.supervisor.enabled: true` (L34-38) + `.ralph/supervisor.db` 存在且 `waves/wave_slots/slot_attempts/dispatch_records` 表可读
- `wave`: forge.yml `tasks.coordinator_hats: [forge-dispatcher]` + events 文件含 `wave_id=w-18d0366e1ab3ce34-795108-0`（5 次 `exec.unit.ready`）+ reuse-history `flow-authority.jsonl` step 序列含 `forge.wave.prepare` / `forge.wave.worktrees.ready`

**缺失产物 → 故障判定**（capability-triggered）:

- `.ralph/supervisor.db` 缺失 → **N/A**（capability +supervisor 且 db 存在 ✓）
- events 无 `wave_id` → **N/A**（capability +wave 且 events 含 wave_id ✓）
- `runtime-trace.jsonl` 缺失 → **P0 候选（DEV-8 共因）**：MINIMAL 模式未写 sidecar → cap manifest 空 + DT7 5 子项全 0 → causal not_evaluable

**盲区 / 根因置信度硬顶**：

- diagnostics_mode=**MINIMAL** → agent/OPAC 归因 ≤ 50，整行硬顶 ≤ 75
- bundle `manifest_status=pending` + identity 全 null → causal_status 兜底 not_evaluable（per confidence-rubric legacy/v1/无契约）
- **双重硬顶 → §5 入表门槛（confidence > 85）结构性不可达**

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: **部分偏离 — Wave 1+2 完整闭环，Wave 3 dispatch 后 stalled，3 slot 已 succeeded（commit 落盘）但 exec.unit.done 不到 main events，2 slot running 永不收敛；下游 8 顶 hat (reviewer/integrator/verifier/tester/auditor/finalizer/cleanup/reporter) 拓扑死锁**
- **P0 / P1 / P2 数量**（均为 confidence≥入表门槛）: **§5 表为空**（0 P0 / 0 P1 / 0 P2 入表）；§7 含 8 P0 候选 / 2 P1 / 2 P2（confidence 55-72，受 MINIMAL 硬顶 ≤ 75 压制）
- **最高优先级根因置信度**: §7 最高 = DEV-4 = **68-72**（file:line 强证据锁定 `parallel_forge_resume.rs:316-323`）；其次 DEV-9 = **65-72**（同族 3 次历史命中）
- **历史复发**: **是 — 旧问题复发**。本次 plan origin 指向 brainstorm `2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md`；同族根因在 30 天窗口命中 6+ 次（8/26、8/27、8/08、8/05、7/30-094057、7/30-002911）。详见 §3。

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 部分 | Wave 1+2 OPAC 全 ✅；Wave 3 5 个 executor 全 ✗（signal 丢失 + 2 永不收敛），8 顶下游 hat N/A | 55-65 |
| Q2 | 基座机制是否正常生效？ | ⚠️ 部分 | R1/R6 不可判定（缺 runtime-trace.jsonl）；R2/R4/R5 ✅；R3 OPAC `validate_bounded_path` 偏离（DEV-4 file:line 强证据） | 60-72 |
| Q3 | 编排是否合理、正常运行？ | ⚠️ 部分 | preset 编排本身合理（1 轮 correction 闭环）；但 Wave 3 后 fan-in 阻塞 + flow-authority 永久卡 `forge.wave.worktrees.ready` 步骤（DEV-12） | 65-70 |
| Q4 | 问题归因：runtime / preset / agent / backend / diagnostic_capture_contract？ | runtime (主导) + preset + diagnostic_capture_contract | §7 主因 DEV-1/2/7/9/11/12 = runtime（共因：hat-channel → main events 路由 + collect phase）；DEV-4 = runtime (parallel_forge_resume path validation)；DEV-6 = preset（命名漂移）；DEV-8 = diagnostic_capture_contract（MINIMAL mode cap 空） | **取 §7 最高 = 68-72**（DEV-4） |

### 1.3 根因一句话

**Wave 3 dispatch 后，5 个 executor slot 中 3 个已在 supervisor.db 标记 succeeded（end_head_sha 与 worktree commit SHA 一致）但 `exec.unit.done` 未落 main events.jsonl / accepted-transitions.jsonl / flow-authority.jsonl，2 个 slot running 永不收敛，导致下游 8 顶 hat (reviewer/integrator/verifier/tester/auditor/finalizer/cleanup/reporter) trigger 永不到达、整个 Wave 3 fan-in 拓扑死锁。PID 526040 在 49 分钟真空期后被外部 kill，reuse-history 捕获 salvage_write_count=0。本质上是 hat-channel → main events 路由在 collect phase 写入路径上未在 MINIMAL 模式验证（与 2026-08-26/27 `parallel-forge-plan-zippy-lark-executor-zero-emit` 主因同族复发）。**（**置信度 65-72**，per §7 DEV-9；DT7 confidence 受 `causal_status: not_evaluable` + MINIMAL ≤ 75 双重压制，故不入 §5）

### 1.4 终态时序一致性（event-artifact chronology）

> 强制分栏：先按 accepted event 确定首轮终态，再解释后续 artifact/commit 恢复。禁止用 mutable artifact 反向覆盖先前 accepted verdict。

| 项目 | 内容 |
|------|------|
| **首轮终态（initial_terminal_status）** | **首轮失败（stalled at Wave 3 fan-in）**：accepted events 序列末段止于 `forge.wave.worktrees.ready`（iter 21，2026-08-29T07:34:18Z）；Wave 3 任何 `exec.unit.done` 未 accepted → `exec.wave.complete` 未发 → reviewer/integrator/verifier/tester/auditor/finalizer/cleanup/reporter 全链未激活。supervisor.db `waves[w-3].phase='collect', delivery_state='pending'`。 |
| **恢复状态（recovery_status）** | **无 accepted 恢复事件**。artifact 层面：3 个 worktree commit 落盘（slot 1/2/4 HEAD 8dfba463/e2acf6a2/838c4b66）+ 2 个 completion.md（U06 @ 15:57、U07 @ 16:01）+ 1 个 reuse-history archive @ 16:23，但均**无对应 accepted business event**。属于"失败终态后工件被外部 operator / supervisor 落盘但无 accepted 成功事件"。 |
| **最终代码状态（final_code_state）** | wave 3 slot 1/2/4 各自的 `feat(parallel-forge): 启用 {forge.wave.reviewed / forge.wave.settled / forge.audit.done} 双 guard` commit 已落盘；slot 0 (`forge.worktrees.ready` 双 guard) commit 落盘于集成分支 e95698f1；slot 3 (U08) 有 9 个 uncommitted changes（未提交）。integration branch `forge/integration/...` @ e95698f1 = wave 3 slot 0 commit。 |
| **一致性告警** | ⚠️ **失败终态后恢复**：首轮 Wave 3 fan-in 为 stalled（5 slot 无 1 个 exec.unit.done accepted），后续 artifact 被落盘（3 commit + 2 completion.md + 1 reuse-history archive），但**无任何 accepted 成功事件**。禁止输出「零拒收」或「首轮完整成功」。**plan 目标 4 个核心 guard（U05 forge.worktrees.ready / U06 forge.wave.reviewed / U07 forge.wave.settled / U09 forge.audit.done）的代码改动事实上落盘**，但**该结论不能反写 event verdict**。 |

---

## 2. 执行链路对比图（Agent A）

### 2.1 拓扑激活表

> preset: `parallel-forge` (SSOT `presets/en/parallel-forge.yml`); execution_mode: `isolated`; execution_model: `supervisor`; repair_budget=3; max_concurrent_workers=8; slot_retry_budget=2。

| Hat | 关键 Triggers | 计划阶段 | 实际 activation 次数 | 状态 |
|---|---|---|---|---|
| inspector | `forge.start` | planning | 1（:1） | ✅ |
| planner | `forge.plan.inspected` | plan_authoring | 1（:2） | ✅ |
| guardian | `forge.plan.ready` | concurrency_review | 1（:3） | ✅ |
| worktree | `forge.concurrency.approved`, `forge.wave.prepare` | worktree_setup | 3（:4/:14/:21） | ✅ |
| forge-dispatcher | `forge.wave.worktrees.ready`, `forge.wave.settled` | development_loop | 3（:13/:20 准备 + fan-out 派发归 supervisor） | ✅（dispatch 已派发 5+1+4=10 个 exec.unit.ready） |
| exec-integrator (supervisor) | （slot 完成计数） | development_loop | 2（:5/:15） | ✅（w-1/w-2）；⏸ w-3 未达 |
| executor | `exec.unit.ready` | development_loop | 2 hat-id（:5/:15）共 5 条 `exec.unit.done` | ✅（w-1:4 + w-2:1）；⏸ w-3:5 dispatched, 3 succeeded, 2 running, 0 accepted |
| reviewer | `exec.wave.complete`, `forge.correction.done` | development_loop | 3（:6/:9/:16） | ✅ |
| forge-failure-handler | `exec.wave.failed`, `forge.wave.review.failed`, … | development_loop | 1（:7） | ✅（wave 1 1 轮 correction） |
| wave-fixer | `forge.correction.requested` | development_loop | 1（:8） | ✅（round 1） |
| integrator | `forge.wave.reviewed`, `forge.wave.verified` | development_loop | 3（:10/:12/:17/:19） | ✅（w-1 + w-2 settle） |
| verifier | `forge.wave.integrated`, `forge.integration.done` | development_loop | 2（:11/:18） | ✅ |
| tester | `forge.exec.development.done`, `forge.final.correction.settled` | full_verify | 0 | ⏸ 未触发（依赖 w-3 settled → exec.development.done） |
| auditor | `forge.full.verified` | audit | 0 | ⏸ 未触发 |
| finalizer | `forge.audit.done` | finalize | 0 | ⏸ 未触发 |
| cleanup | `forge.finalized`, `forge.plan.blocked`, `work.failed`, `forge.units.reviewed` | cleanup | 0 | ⏸ 未触发 |
| reporter | `forge.cleanup.done` | report | 0 | ⏸ 未触发（LOOP_COMPLETE 未发） |

**来源统计**: `accepted-transitions.jsonl` 共 26 行；`exec.unit.done`=5（4+1）、`exec.wave.complete`=2、`forge.wave.prepare`=2、`forge.wave.worktrees.ready`=2、`forge.worktrees.ready`=1、`forge.wave.integrated`=2、`forge.wave.settled`=2、`forge.wave.reviewed`=2、`forge.wave.verified`=2、`forge.wave.review.failed`=1、`forge.correction.requested`=1、`forge.correction.done`=1。

### 2.2 时间轴对比表（预期 vs 实际）

| Iter | Hat（:序号） | 预期事件 | 实际事件 | 时间 (+08:00) | 状态 | 偏差 |
|---:|---|---|---|---|---|---|
| 1 | inspector:1 | `forge.plan.inspected` | `forge.plan.inspected` | 13:57:28 | ✅ | — |
| 2 | planner:2 | `forge.plan.ready` | `forge.plan.ready` | 14:01:43 | ✅ | — |
| 3 | guardian:3 | `forge.concurrency.approved` | `forge.concurrency.approved` | 14:03:41 | ✅ | — |
| 4 | worktree:4 | `forge.worktrees.ready`（初扇 w-1, 4 slots） | `forge.worktrees.ready` | 14:08:50 | ✅ | — |
| 5a | executor:5 (w-1 s0-s3) | 4× `exec.unit.done` | 4× `exec.unit.done` | 14:26:13 | ✅ | slots s0-s3 均 succeeded, commit HEAD 落盘 |
| 5b | exec-integrator:5 | `exec.wave.complete`（w-1） | `exec.wave.complete` | 14:26:13 | ✅ | wave 1 fan-in 完成 |
| 6 | reviewer:6 | `forge.wave.reviewed` | **`forge.wave.review.failed`** | 14:29:29 | ⚠️ | wave 1 评审失败, failure-handler 接管 |
| 7 | failure-handler:7 | `forge.correction.requested`（round 1） | `forge.correction.requested` | 14:31:54 | ✅ | correction budget 1/3 |
| 8 | wave-fixer:8 | `forge.correction.done` | `forge.correction.done` | 14:40:02 | ✅ | round 1 修复落地 |
| 9 | reviewer:9 | `forge.wave.reviewed`（ACCEPTED） | `forge.wave.reviewed` | 14:43:47 | ✅ | 复审通过 |
| 10 | integrator:10 | `forge.wave.integrated` | `forge.wave.integrated` | 14:47:55 | ✅ | w-1 合并 |
| 11 | verifier:11 | `forge.wave.verified` | `forge.wave.verified` | 14:55:13 | ✅ | w-1 验证 |
| 12 | integrator:12 | `forge.wave.settled`（CloseTaskBatch） | `forge.wave.settled` | 14:57:25 | ✅ | w-1 close task batch |
| 13 | dispatcher:13 | `forge.wave.prepare`（w-2） | `forge.wave.prepare` | 14:58:30 | ✅ | w-2 进入 lazy fan-out |
| 14 | worktree:14 | `forge.wave.worktrees.ready`（w-2, 1 slot） | `forge.wave.worktrees.ready` | 14:59:22 | ✅ | — |
| 15a | executor:15 (w-2 s0) | `exec.unit.done` | `exec.unit.done` | 15:22:01 | ✅ | single slot succeeded |
| 15b | exec-integrator:15 | `exec.wave.complete`（w-2） | `exec.wave.complete` | 15:22:01 | ✅ | — |
| 16 | reviewer:16 | `forge.wave.reviewed` | `forge.wave.reviewed` | 15:23:14 | ✅ | w-2 直通 |
| 17 | integrator:17 | `forge.wave.integrated` | `forge.wave.integrated` | 15:25:44 | ✅ | — |
| 18 | verifier:18 | `forge.wave.verified` | `forge.wave.verified` | 15:29:42 | ✅ | — |
| 19 | integrator:19 | `forge.wave.settled` | `forge.wave.settled` | 15:30:39 | ✅ | w-2 close |
| 20 | dispatcher:20 | `forge.wave.prepare`（w-3） | `forge.wave.prepare` | 15:32:20 | ✅ | w-3 lazy fan-out |
| 21 | worktree:21 | `forge.wave.worktrees.ready`（w-3, 5 slots） | `forge.wave.worktrees.ready` | **15:34:18** | ✅ | **最后一条 accepted transition** |
| 22 | executor slot-attempts | 5× `exec.unit.done`（w-3 s0-s4） | **缺失**: 5 dispatched, 3 succeeded, 2 running, **0 accepted** | 15:34:18→ | ❌ | `slot_attempts`: s1/s2/s4 已 succeeded（commit HEAD 落盘）；s0/s3 仍 running；supervisor.waves[2].phase='collect', delivery_state='pending' |
| 23 | exec-integrator (w-3) | `exec.wave.complete`（w-3） | **缺失** | — | ❌ | w-3 fan-in 阻塞 |
| 24..∞ | reviewer/integrator/verifier/... tester/auditor/finalizer/cleanup/reporter | w-3 后续收尾链 | **缺失** | — | ⏸ | 链路从未启动 |
| — | supervisor.salvage | （无） | loop 在 49 分钟后被 reuse-history 捕获（16:23:00）；`salvage_write_count=0`、coordination_topic 空 → runtime 状态机停在 `collect/pending` | 16:23:00 | ⚠️ | reuse-history 触发后再无任何 transition 落盘 |

### 2.3 终止类型 + 未触发 hat

- **终止类型: `stalled`**（w-3 exec.unit.done 未落盘 → w-3 fan-in 永不到达 → tester/auditor/finalizer/cleanup/reporter 全链未启动）
  - 既不是 `plan.blocked` / `forge.plan.blocked`（无任何 blocked transition）
  - 也非 `work.failed`（forge-failure-handler 仅 round 1 触发后未再激活）
  - 也非 `LOOP_COMPLETE` / `forge.report.done`（reporter 0 激活）
  - supervisor.db `waves[2]`: `phase=collect, delivery_state=pending, coordination_topic=''`（coordination_committed 永远到不了）；5 个 slot 中 3 个 succeeded（commit 落盘）但 `exec.unit.done` 未 accepted；2 个仍 running（slot_attempts.status='running'）。

- **关键缺口（Agent A 视角）**:
  1. **w-3 exec.unit.done 静默丢失**: supervisor.db 确认 w-3 s1/s2/s4 已 succeeded（end_head_sha:8dfba463…/e2acf6a2…/838c4b66…），但 `events-20260829-055544.jsonl` / `accepted-transitions.jsonl` 中**无任何 w-3 的 `exec.unit.done`**。worker PID 退出 0 ≠ emit accepted；runtime 与 supervisor ledger 之间出现 gap。
  2. **w-3 s0/s3 永不收敛**: slot_attempts `end_head_sha/end_dirty=null`，仍 running，无失败码 → 既不 succeeded 也不 failed，supervisor 既不发 `exec.wave.complete` 也不发 `exec.wave.failed`。
  3. **failure-handler 未接到 w-3 fail 信号**: 预设中 failure-handler 订阅 `exec.wave.failed`/`forge.wave.review.failed`/`forge.verification.failed` 等，w-3 既无 `exec.wave.complete` 也无 `exec.wave.failed` → failure-handler 0 次再激活 → correction budget 不会被消费。
  4. **reuse-history 49 min 真空**: 15:34:18 → 16:23:00（约 49 min）无任何 transition；loop 在 16:23:00 被 reuse-history 捕获但 `salvage_write_count=0`，意味着 stale 状态被原样封存；运行时既无恢复也无阻断。

- **未触发 hat 清单**: `tester` (0) → `auditor` (0) → `finalizer` (0) → `cleanup` (0) → `reporter` (0)。
- **完全未执行的终态事件**: `forge.exec.development.done`、`forge.full.verified`、`forge.full.verification.failed`、`forge.audit.done`、`forge.finalized`、`forge.cleanup.done`、`forge.report.done`、`work.failed`、`LOOP_COMPLETE`、`plan.blocked`、`forge.plan.blocked`。

---

## 3. 历史问题上下文（Agent B）

> **⚠️ 启用条件**: 本报告 `history_search=preset-only`（30 天滑动窗口；2026-07-30 → 2026-08-29）。Agent B 启动并扫白名单目录（`docs/report/*-diagnosis.md` / `docs/solutions/{integration-issues,logic-errors,state-management,workflow-orchestration}/` / `docs/plans/` status:active / `docs/brainstorms/*.md`）。`disabled` 模式不写本节。

### 3.1 全景表

| 类型 | 文档路径 | 出现次数 | 闭环 | 本次关联度 | 关键症状 |
|---|---|---:|---|---|---|
| diagnosis | `docs/report/2026-08-26-parallel-forge-2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan-diagnosis.md` | 1 | ✗ | **高** | U01 已 merged, 但 verifier hat-channel merge 失败（output=8509/channels=0）→ hard gate `consecutive=1` 触发 → isolated loop no-progress 3 turns → fail-close `forge.plan.blocked`；cleanup hat 不订 `forge.wave.settled` 致兜底链缺失；flow-authority.jsonl 末尾 4 条 orphan 污染。**与本次 evidence-gates plan symptom 几乎同构** |
| diagnosis | `docs/report/2026-08-27-parallel-forge-2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan-diagnosis.md` | 1 | ✗ | **高** | 同 8/26 plan 重跑：isolated channel merge 失败两次（integrator seq 366 + forge-dispatcher seq 386），73 个 `orphan-emit-*.md` 反复出现，bundle capability 错误写成 runner（应 supervisor+wave），capability manifest 推断未含 wave |
| diagnosis | `docs/report/2026-08-10-parallel-forge-primary-20260809-152752-diagnosis.md` | 1 | ✗ | 中 | approval 后 worktree channel 空、未产 `forge.worktrees.ready`、连续无进展后 BLOCKED |
| diagnosis | `docs/report/2026-08-08-parallel-forge-primary-20260808-021642-diagnosis.md` | 1 | ✗ | 中 | flow-authority 停在 `development_loop` 末 topic=`forge.correction.done`；reviewer publish obligation 空 → hard gate 命中；本 run 同 preset 内 stale-tail → flow 锁死先例 |
| diagnosis | `docs/report/2026-08-05-parallel-forge-primary-20260805-090210-diagnosis.md` | 1 | ✗ | 低 | LOGS_ONLY 模式封顶 75；forge-dispatcher / executor 主 events 可见但 limited evidence |
| diagnosis | `docs/report/2026-08-05-parallel-forge-primary-20260805-133322-diagnosis.md` | 1 | ✗ | 中 | `settled_task_ids` 类型错误 → CloseTaskBatch 未关 Unit task → forge-dispatcher 不能有效推进 `forge.exec.development.done` |
| diagnosis | `docs/report/2026-07-30-parallel-forge-primary-20260730-094057-diagnosis.md` | 1 | ✗ | 中 | 68 KB, 最大型；preset 内 stale-tail → flow 锁死先例；review-worker 6 emit 全被 FlowStepScope 拒 (`flow_unknown_emit`) |
| diagnosis | `docs/report/2026-07-30-parallel-forge-primary-20260730-002911-diagnosis.md` | 1 | ✗ | 低 | 同 family, dispatcher stage 内 wave 调度路径 |
| solution | `docs/solutions/workflow-orchestration/parallel-forge-preset-integration-gap.md` | 1 | ✓（plan 2026-07-29-005 已闭环） | 参考 | preset pointer 接通、`close_task_batch` mid-loop 持久化、`event_filter` 与 `triggers` 对齐 |
| brainstorm | `docs/brainstorms/2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md` | 1 | ✗（GAP-03 已关；GAP-01/02/04-16 仍 active） | **高** | **本次 plan 2026-08-27-1430 的 origin**（plan frontmatter 写明）；GAP-03 终态证据 + GAP-04 Decision Contract + GAP-12 隔离硬边界正是 evidence-gates plan 要落地的语义 |
| plan | `docs/plans/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan.md` | — | active | 本次 | evidence-gates plan 自身（frontmatter `status: active`、`origin: docs/brainstorms/2026-08-12-003-...`） |

注: `docs/solutions/{integration-issues,logic-errors,state-management}/` 在本窗口对关键词（`reuse-history` / `resume-manifest` / `path-escape` / `incomplete_reasons` / `evidence-gates`）零命中。

### 3.2 根因分类对照

| 根因模式 | 历史命中文档 | 与本次关联 |
|---|---|---|
| **forge-dispatcher empty / isolated channel merge 失败** | 8/26、8/27（重跑）、8/08、8/05-133322 | **直接同构**——本次 run 如出现"executor `exec.unit.done` 不推进 + forge-dispatcher 不发 `forge.exec.development.done`"，属此族 |
| **flow-authority.jsonl stale-tail 污染** | 8/26（主因 R-1 / 置信度 88）、8/08、7/30-094057 | 8/26 已给出机制层修复点（`event_loop/mod.rs:1156` legacy blind read + `completion_and_termination.rs:861-863` append 契约）；本次是否仍命中需 §7 对账 |
| **cleanup hat 缺 `forge.wave.settled` 订阅** | 8/26（主因 R-3 / 置信度 78）、8/05-133322 | preset-topology gap |
| **hard gate `consecutive=1` + `max_iter=3` 过快 fail-close** | 8/26（主因 R-4 / 置信度 75） | 同 policy 命中可能 |
| **verifier candidate=0 / output_mentions_emit=true** | 8/26 候选 R-7（置信度 50） | 候选同族——若 verifier/reviewer/executor hat 出现"输出含 `emit` 字面提及但 channel 字节=0"，属此模式 |
| **`loop.cancel` 与 `forge.plan.blocked` FlowStepScope 双锁死** | 8/26（R-5 / 置信度 72） | 同 preset 内 coexistence bug 先例 |

### 3.3 本次为新问题模式 / 旧问题复发判定

**结论: 旧问题复发 + 体系化修法，非全新模式**。本次 plan `2026-08-27-1430-feat-parallel-forge-evidence-gates-plan` 的 frontmatter `origin` 明确指向 brainstorm `2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md`，属于把"分散于 8/05 / 8/08 / 8/10 / 8/26 等多份诊断的同族根因（hat-channel merge 失败 + flow-authority stale-tail + cleanup 缺 `forge.wave.settled` 订阅 + hard gate 过快 fail-close）"收敛到统一 evidence-gates 机制层面的体系化修法。`reuse-history` / `resume-manifest` / `path-escape` / `incomplete_reasons` 等具体新词在 30 天 preset-only 窗口内未在白名单目录命中历史记录（这些是本次 plan 内部命名）。

### 3.4 窗口注脚（hard rule）

本次扫描窗口: `preset-only (30d sliding; 2026-07-30 → 2026-08-29)`

---

## 4. 证据清单（Agent C）

### 4.0 偏离清单（DEV）— 按 DT7 维度组织

| # | DT7 | 现象 | 证据锚点（`file:line` / `events:L<N>`） | 状态 |
|---|---|---|---|---|
| DEV-1 | coverage | Wave 3 三个 slot（s1/s2/s4）的 `exec.unit.done` **未出现在任何业务事件账本**（主 events / accepted-transitions / ledger / flow-authority 全部缺失），但 supervisor.db `slot_attempts` 标记 succeeded 且有真实 `end_head_sha` 与 worktree commit 一致 | `events-20260829-055544.jsonl:L28-32`（w-3 仅有 `forge.wave.worktrees.ready` + 5× `exec.unit.ready`，**无** `exec.unit.done`）；`.ralph/supervisor.db slot_attempts wave_id=w-18d0366e1ab3ce34-795108-0`: s1 finished 15:59:18 / `end_head_sha=8dfba463…`、s2 finished 16:01:41 / `e2acf6a2…`、s4 finished 16:00:09 / `838c4b66…`；`.ralph/reuse-history/.../flow-authority.jsonl` 末段仅 `…forge.wave.worktrees.ready`，**无** w-3 的 `exec.unit.done` 任何 attempt | **P0 候选（信号丢失）** |
| DEV-2 | coverage | Wave 3 s0/s3 永不收敛: `slot_attempts.status='running'`、`finished_at=0`、`failure_code=NULL`，既未 succeeded 也未 failed；supervisor 没有 fan-in 机制触发 complete | `supervisor.db slot_attempts`: `slot_index=0`（U05）started 15:35:36 finished_at=0 end_head_sha=NULL；`slot_index=3`（U08）started 15:35:36 finished_at=0 end_head_sha=NULL；`dispatch_records wave_id=w-3 outcome=NULL`（s0/s3）；`.ralph/wave-channels/wave-w-18d0366e1ab3ce34-795108-0-{0,3}.jsonl` 两文件均为 **0 字节** | **P0 候选（永不收敛）** |
| DEV-3 | coverage | reuse-history 真空期 49 分钟: 15:34:18 (`forge.wave.worktrees.ready` 写入) → 16:23:00（reuse-history `captured_at` + `.ralph/agent/resume-context.md` mtime） | `.ralph/agent/accepted-transitions.jsonl:L26` committed_at=2026-08-29T07:34:18Z；`.ralph/ledger.jsonl` 末行 timestamp=2026-08-29T07:34:18Z；`.ralph/forge/.../units/` mtime: U01/U02/U03 14:21-22, U04 15:21, **U06 15:57, U07 16:01**；`U05/U08/U09` 在集成侧不存在, 只有 `exec-w-3-4/.ralph/forge/.../units/U09-completion.md`（slot 工作树内） | **P0 候选（真空→非业务写盘）** |
| DEV-4 | integrity | `resume-manifest.incomplete_reasons` 唯一一条将 U10 误判为路径逃逸（绝对路径实际就在 worktree 内） | `.ralph/reuse-history/.../parallel-forge-resume-manifest.v1.json` `incomplete_reasons=["declared artifact path escapes worktree: resume manifest path escapes the worktree: …/.ralph/forge/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan/units/U10-completion.md"]`；`crates/ralph-core/src/parallel_forge_resume.rs:316-323` `validate_bounded_path` 仅检查"非空 + 非绝对 + 无 `..`", 未做 worktree-prefix 归一化 | **P1（校验器假阳性）** |
| DEV-5 | integrity | supervisor 调度台账 `dispatch_records.pid` 对 **所有 w-3 slot**（含 succeeded）均为 `NULL`，机制上无法用 PID 反查 worker 是否仍存活 | `supervisor.db dispatch_records wave_id=w-3`: 5 行 `pid=NULL`；对比 `slot_attempts.started_at_unix_ms`（07:35:36 = 1756410936Z）与 PID 526040 `ps` 输出已无该 PID | **P1（机制盲点）** |
| DEV-6 | correlation | Wave 1 使用 topic `forge.worktrees.ready`, Wave 2/3 改用 `forge.wave.worktrees.ready` — 同名语义阶段名漂移，导致下游订阅与 reconciliation 工具可能只匹配其一 | `events-20260829-055544.jsonl:L8`（w-1 `forge.worktrees.ready`） vs `L23`（w-2 `forge.wave.worktrees.ready`） | **P2（命名漂移）** |
| DEV-7 | correlation | U06/U07 completion 文件（15:57/16:01）与 w-3 slot finish 时间（15:59:18/16:01:41）相差 ~8 小时（注: 实际是同日 15:34 → 16:23 跨段, U06/U07 file mtime 16:01 仍在 stall 真空内），且无对应 `exec.unit.done` accepted — 文件落盘与业务事件流完全脱钩 | `.ralph/forge/.../units/U06-completion.md` mtime 15:57 vs slot 1 finished 15:59:18；`.ralph/forge/.../units/U07-completion.md` mtime 16:01 vs slot 2 finished 16:01:41；events 流无对应 `exec.unit.done` | **P0 候选（事件流与工件流分裂）** |
| DEV-8 | correlation | cap manifest `execution_capabilities: []` 空数组，但 `diagnosis_input.execution_capability = ["supervisor", "wave"]` — cap manifest 漂移，无法反映 worker 真实能力面 | `.ralph/diagnostics/2026-08-29T13-55-44/diagnosis-input.json` `execution_capability=["supervisor","wave"]` vs 同目录 cap manifest 缺字段 / `[]`（MINIMAL 模式未生成完整 cap 表） | **P2（diagnostics 降级）** |
| DEV-9 | refutation | Wave 3 仅 `exec.unit.ready` 在 events 流出现（5 次），WAVE 内的扇出已写盘，但下游 reviewer/integrator/verifier/tester/auditor/finalizer/cleanup/reporter hat 的 `exec.unit.done` trigger 永远不满足 → 8 顶 hat 不可能被激活 | `events-20260829-055544.jsonl:L26-32` 5× `exec.unit.ready` for w-3；preset `presets/en/parallel-forge.yml` reviewer/integrator/verifier/tester/auditor/finalizer/cleanup/reporter 各自 `triggers:` 显式订阅 `exec.unit.done`（见 §4.3 矩阵） | **P0 候选（拓扑死锁）** |
| DEV-10 | refutation | `worker_results` 表为空、`wave_emissions` 表为空、`redrive_requests` 表为空、`compensation_jobs` 表为空 — supervisor 自身定义的 4 张纠偏/审计账本全部为空，纠偏通道从未启动 | `supervisor.db` 4 张表 SELECT 返回 `[]`；时间范围 `reserved_at >= 1756410936`（w-3 起点）至 16:30 全程无记录 | **P0 候选（纠偏沉默）** |
| DEV-11 | freeze_window | 事件账本在 15:34:18 后冻结约 49 分钟，直到 16:23 才出现 `resume-context.md`（advisory, 非业务事件）；PID 526040 在 13:55:44 启动 → 死之前整个 freeze window 内无任何业务 emit | `.ralph/loops.json` started=13:55:44, PID=526040；`ps -p 526040` 返 `process not found`（外部 kill 已被外部观察者确认） | **P0 候选（runtime freeze）** |
| DEV-12 | freeze_window | `flow-authority.jsonl` 末段停在 w-3 的 `forge.wave.worktrees.ready`，无后续 `exec.unit.done` 接受记录也无 `flow.step.advance` — flow 调度永久卡在"准备 exec 槽"阶段 | `.ralph/reuse-history/.../flow-authority.jsonl` 末行 `step=forge.wave.worktrees.ready loop_id=2026-08-27-1430-feat-parallel-forge-evidence-gates-plan` | **P0 候选（flow 卡死）** |

> **DEV 编号注**: DEV-1/2/3/7/9/10/11/12 共 8 条 P0 候选，**最终归并到 §7**（per DT7 strict: causal_status=not_evaluable + MINIMAL ≤ 75 双重硬顶）；DEV-4/5 P1，DEV-6/8 P2。此处只列证据，不做归因。

### 4.1 OPAC 表（按 hat 维度对账，diagnostics_mode=MINIMAL）

| Hat | Ownership | Permission | Authority | Contract | 评估 |
|---|---|---|---|---|---|
| inspector | plan inspect artifact | default | bootstrap→planner | 必有 `execution_wave`/`integration_order`/`wave_total` | ✅ events 流 L1-3 正常 accepted |
| planner | plan emission | default | inspector | `forge.plan.ready` schema 校验通过 | ✅ events L4 正常 |
| guardian | plan approval | default | planner | `forge.concurrency.approved` | ✅ events L5 正常 |
| worktree | worktree provisioning | default | guardian | `forge.wave.worktrees.ready` 含 `verified_base_commit` | ✅ events L8（w-1）/ L23（w-2）/ L26（w-3）accepted, 末次 L26 |
| forge-dispatcher | wave emit | supervisor-cmd | worktree | `ralph wave emit exec.unit.ready` 5 次 for w-3 | ✅ events L27-L31 |
| executor (slot 0 U05) | per-slot worktree exec | wave-worker | forge-dispatcher | `exec.unit.done` 必须带 `commit_sha`+`content_hash` | **✗ 无任何 accept 痕迹, slot 仍 `running`** |
| executor (slot 1 U06) | 同上 | 同上 | 同上 | 同上 | **✗ slot `succeeded` 有 `end_head_sha=8dfba463…` 但无 business emit** |
| executor (slot 2 U07) | 同上 | 同上 | 同上 | 同上 | **✗ slot `succeeded` 有 `end_head_sha=e2acf6a2…` 但无 business emit** |
| executor (slot 3 U08) | 同上 | 同上 | 同上 | 同上 | **✗ 与 s0 同状态, `running`/`finished_at=0`** |
| executor (slot 4 U09) | 同上 | 同上 | 同上 | 同上 | **✗ slot `succeeded` 有 `end_head_sha=838c4b66…`, 工件 U09-completion.md 落在 slot worktree, 集成侧无** |
| reviewer | wave review | default | forge-dispatcher | `forge.wave.reviewed`/`forge.wave.review.failed` | **N/A — trigger 未满足** |
| integrator | FF integration | default | reviewer | `forge.wave.integrated` 带 `candidate_commit_sha` | **N/A** |
| verifier | full gate | default | integrator | `forge.wave.verified` | **N/A** |
| tester | full regression | default | verifier | `forge.full.verified`（3 轮 correction 预算） | **N/A** |
| auditor | gate audit | default | tester | （preset schema） | **N/A** |
| finalizer | final commit | default | auditor | （preset schema） | **N/A** |
| cleanup | worktree cleanup | default | finalizer | `forge.cleanup.done` | **N/A**（与 MEMORY `parallel-forge-cleanup-after-loop-complete.md` 关联: LOOP_COMPLETE 后 cleanup 会被 `terminal_monotonicity_violation` 拒收） |
| reporter | final report | default | cleanup | `forge.report.done` | **N/A** |

> **OPAC 备注**: Activation outcome 因 `runtime-trace` 缺失（`causal.status: not_evaluable`, diagnostics MINIMAL）整体 **N/A** — 见 §4.2。

### 4.2 Activation outcome 表

**N/A (activation outcomes unavailable)** — `runtime-trace.jsonl` 0 字节被 runtime 视为 `Missing`；`feedback.jsonl` 同。`causal_status: not_evaluable`, `diagnostics_mode: MINIMAL` 下, Agent C 无法复原"哪条 policy 把 emit 拒了"。Agent D 重跑 causal 时同样 not_evaluable（见 §D.1），无法升 §5。activation_outcomes frontmatter = `missing`。

### 4.3 Causal Attribution（plan 2026-08-26-1104, U10）

**N/A (causal attribution unavailable)** — `ralph diagnose --causal` round 1 + round 2 均返回 `status: not_evaluable`, `confidence.total: 0`, 5 子项全 0, `coverage_gaps: []`, `rejected_hypotheses: []`。bundle identity 全 null + 4 个 sidecar 缺失/空 = bundle 结构性缺失根因。per confidence-rubric DT7: legacy/v1/无契约 → §5/§6 不得入表, §7 完整列候选。原因写入 `evidence_gaps`。

#### 4.3.1 DT7 分项 + 总置信度

| DT7 项 | 分值 | 实测值（来自 `--causal`） | 来源 |
|--------|------|---------------------------|------|
| coverage | +30 | **0** | bundle `execution_capabilities: []`、`artifacts: []`、`boundary_coverage: []` |
| integrity | +25 | **0** | outbox ↔ commit_receipt 不可 join（events.jsonl 缺 terminal event 计数） |
| refutation | +20 | **0** | `rejected_hypotheses: []`（4 落选域均无反驳证据） |
| correlation | +15 | **0** | contract_digest 缺, sequence 不单调 |
| freeze_window | +10 | **0** | `evidence-window.jsonl` 不存在 |
| **总置信度** | max 100 | **0** | `ralph diagnose --causal` round 1+2 |

#### 4.3.2 被否决假设（rejected_hypotheses）

**空**（per causal_status=not_evaluable, no hypotheses to refute）。

#### 4.3.3 分数变化（causal_score_change）

**N/A (initial scoring; round 1 + round 2 均 not_evaluable, total=0)**。

### 4.4 R1-R6 机制级偏离矩阵

| 机制层 | 文件 / 锚点 | 状态 | 偏离描述 |
|---|---|---|---|
| R1 单业务事件预算 | `crates/ralph-core/data/ralph-tools.md` §6 | **不可判定** | 5 个 executor activation 都看不到 emit 是否曾"想发但被单事件预算吞掉"，trace 缺失 |
| R2 终态事件隔离 | 同上 §6 | ✓ 通过 | 末段无 `plan.complete`/`LOOP_COMPLETE`/`plan.blocked` 误夹带 |
| R3 OPAC per-hat | `crates/ralph-core/src/parallel_forge_resume.rs:316-323` (`validate_bounded_path`) | **偏离** | 绝对路径不归一化就拒，U10-completion.md 误判（DEV-4 file:line 强证据） |
| R4 Isolated 进程隔离 | `event_loop/mod.rs` step-close | ✓ 通过 | events 流无 hat 间互见 |
| R5 OPAC Confirm 不重发 | `crates/ralph-core/data/ralph-tools-opac.md` | ✓ 通过 | accepted-transitions 无同一 transition_id 重复 |
| R6 payload_consistency | `crates/ralph-core/src/event_policy/payload_consistency.rs` | **不可判定** | 无法判断 emit 是否"已写但被规则拒"，因为根本无 accepted 事件 |
| 终态时序一致性 | event-artifact chronology | ⚠️ **失败终态后恢复** | 详见 §1.4 |

### 4.5 Agent B 知识提示（供下游引用，非归因）

1. **w-3 s1/s2/s4 工件散落但事件丢失** — 与 MEMORY `parallel-forge-slot-worktree-lock-cherry-pick-fallback.md` 的"slot branch parent 常是 workspace HEAD 而非 integration HEAD"机制相关。Agent B 提示 D: 修复路径需区分"slot 内 commit 已落盘" vs "事件流未 capture" 两层。
2. **reuse-history 49 分钟真空** — 跨 w-3（15:34:18）→ 16:23，与 MEMORY `parallel-forge-plan-zippy-lark-executor-zero-emit.md`（2026-07-28 plan executor 0 业务事件 → runtime 注入 work.failed）属同一谱系: executor 进程内 emit 通道未 flush 即被外力终止时，主 events 流永久丢失。
3. **flow-authority 末段 `forge.wave.worktrees.ready`** — 与 MEMORY `flow-authority-stale-tail-pollutes-recovery.md` 的反向证据: **此处无 stale tail 污染**，只是单纯的 step 永远不前进（区别于 zippy-lark 的污染性残留）。
4. **cleanup hat 与 LOOP_COMPLETE 抢跑** — 见 MEMORY `parallel-forge-cleanup-after-loop-complete.md`: 本 run 永远到不了 cleanup, 但若未来续作触发 LOOP_COMPLETE, cleanup hat 的 `forge.cleanup.done` 仍会被 `terminal_monotonicity_violation` 拒收。
5. **dispatch_records.pid 全 NULL** — 与 MEMORY `agent-kill-self-parent-ralph.md` 反向印证: 本 run 内无任何自相杀 PID 事件（ps 已确认 PID 526040 来自外部 kill）。
6. **Wave 1 vs Wave 2/3 topic 命名漂移** — 见 DEV-6, Agent B 提示: preset `presets/en/parallel-forge.yml` 内 `forge.wave.worktrees.ready` 与历史 `forge.worktrees.ready`（w-1）共存, reviewer hat 的 triggers 是否对二者兼容需 §7 复核。

---

## 5. 问题归因表（DT7 机检，confidence > 85）

**空表**。

理由（结构性事实）:

- `causal_status: not_evaluable`（round 1 + round 2 双确认，见 §D.1）
- `causal.confidence.total = 0`，5 个子项（integrity / freeze_window / correlation / coverage / refutation）均为 0
- 输入 bundle 结构性缺失: `diagnosis-input.run.{loop_id, preset_label, config_path, plan_path, baseline_sha}` 全 null；`runtime-trace.jsonl` 空；`feedback.jsonl` 空；`orchestration.jsonl` / `errors.jsonl` 不存在
- `diagnostics_mode: MINIMAL` → confidence 硬顶 ≤ 75
- 上述两个原因（not_evaluable + MINIMAL 上限 75）共同决定 §5 入表门槛（>85）**结构性不可达**
- per `confidence-rubric.md` legacy/v1/无契约 兜底规则：所有 P0 候选一律落 §7

| 优先级 | 问题 | primary_domain | status | confidence | 证据 DEV | DT7 分项来源 | rejected_hypotheses | 历史关联 | 加深轮次 |
|--------|------|----------------|--------|------------|----------|--------------|---------------------|----------|----------|
| — | — | — | — | — | — | — | — | — | — |

---

## 6. 修复建议（仅针对 §5 已入表项）

**空表**（per confidence-rubric §6.1/6.3：§5 未入表时 §6 不得驱动修复）。

> **advisory only** — 来自 §7 高置信度候选，非 §5 已入表项；operator 可酌情采纳。下次 run 仍命中的同族根因首推 DEV-1/7/9 共因（executor 完成但 exec.unit.done 不到 main events → 8 顶下游 hat 拓扑死锁）。

| 候选 | 目标 | 改动 | 预期效果 | 关联置信度（§7） |
|---|---|---|---|---|
| DEV-1/7/9 共因 | 让 `exec.unit.done` 跨 hat-channel 落账 | `crates/ralph-core/src/supervisor/coordinator.rs:528` 确认 `record_slot_terminal_evidence` 写入 flow + events 双账本；`event_loop/disposition.rs:376-377` 已声明 `forge.wave.settled` 与 `exec.unit.done` 的同源关系，需补一轮以 hat-channel → main events 路由强制合并的集成测试 | 下次 executor 完成时 main events.jsonl 必现 `exec.unit.done`，下游 reviewer/integrator/verifier/tester/auditor/finalizer/cleanup/reporter 8 顶 hat 不再拓扑死锁 | 65-72 |
| DEV-4 | 修 `validate_bounded_path` 误判绝对路径 | `crates/ralph-core/src/parallel_forge_resume.rs:316-323`：先 `std::path::absolute()` 归一再做 `is_absolute()` 与 `..` 检查 | U10 `incomplete_reasons` 不再因 manifest 路径绝对化 false-positive | 68-72 |
| DEV-6 | 收敛 worktree_setup dual emit 名 | `presets/en/parallel-forge.yml:73-79` 与 `L92-99` 同时列出 `forge.worktrees.ready` 与 `forge.wave.worktrees.ready`，将 Wave 1 改用 `forge.wave.worktrees.ready`（与 Wave 2/3 一致），并同步修 `L76-77` allowed_emits 与 `L533-538` 顺序 | 单名化降低 preset lint / dispatch 误路由 | 60-65 |
| DEV-12 | 解 flow-authority stale-tail 永久阻塞 | `.ralph/flow-authority.jsonl` trim 在 loop_id 边界前收口（已有 solution: `flow-authority-stale-tail-pollutes-recovery.md`），并在 `resume_routing.rs` 启动时强制 `load_flow_authority_current_step` 取首个有 `loop_id` 字段的 entry 而非末段 | `forge.wave.worktrees.ready` 之后 flow 调度能继续推进 | 65-70 |

---

## 7. 未核实疑点

> per `confidence-rubric.md` DT7: `status == incomplete` / `not_evaluable`（legacy / v1 / 无契约）→ 落 §7 不入 §5/§6。`diagnostics_mode: MINIMAL` → 整行硬顶 ≤ 75。下表按置信度从高到低排，每条均含 `blocked_by`。

| 候选问题 | 当前置信度 | primary_domain | blocked_by | 已做加深 | 历史关联 | 备注 |
|----------|------------|----------------|------------|----------|----------|------|
| **DEV-9**: w-3 `exec.unit.done` 永久不到达 → 8 顶下游 hat (reviewer/integrator/verifier/tester/auditor/finalizer/cleanup/reporter) 拓扑死锁 | **65-72** | runtime | DT7 `integrity` + `correlation` 双账本缺失（runtime-trace.jsonl 空 + orchestration.jsonl 不存在） | 第 1 轮：源码定位 `supervisor/coordinator.rs:528` `record_slot_terminal_evidence` + `event_loop/disposition.rs:376-377` 同源声明 + `event_loop/wave_branch_tests.rs:45-57` R3 单测契约 | **高**（与 `parallel-forge-plan-zippy-lark-executor-zero-emit` 同族；30d 命中 3 次: 8/26 / 8/27 / 8/05） | hat-channel → main events 路由未在 MINIMAL 模式下被验证；cap manifest 空（DEV-8）使该假设无法证伪 |
| **DEV-4**: U10 incomplete_reasons 路径越界 false-positive（`validate_bounded_path` 未做绝对路径归一化） | **68-72** | runtime | DT7 `integrity`（缺 U10 manifest 校验失败的 stderr/log 实证）+ `coverage`（缺 path 校验 round-trip 轨迹） | 第 1 轮：**file:line 锁定** `crates/ralph-core/src/parallel_forge_resume.rs:316-323` `validate_bounded_path`: `path.is_absolute()` 直接判，未做 `std::path::absolute()` 归一；`Component::ParentDir | RootDir | Prefix(_)` 检查存在 | 中（path-escape 是新词，30d 窗口未命中） | high-confidence DEV-4（命中 file:line 证据），是 §7 中置信度最高的一条；operator 可优先采纳 |
| **DEV-12**: flow-authority.jsonl 末段卡在 `forge.wave.worktrees.ready`，flow 调度永久不前进 | **65-70** | runtime | DT7 `correlation`（缺 orchestration.jsonl 时间序列）+ `coverage`（缺 flow-authority step 切换轨迹） | 第 1 轮: `event_loop/wave_branch_tests.rs:45-57` R3 + `L93-106` S3 单测已定义 exec.unit.done 不 advance `development_loop`；说明 flow-authority 推进靠 `forge.wave.settled` 而非 `exec.unit.done` 单事件 | **高**（30d 主因: 2026-08-26 置信度 88，solution `flow-authority-stale-tail-pollutes-recovery.md`；2026-08-27 同根因二次命中） | 与 DEV-9/DEV-1 共因；但 isolated hat-channel 路由也是嫌疑链 |
| **DEV-1**: w-3 s1/s2/s4 已 succeeded（end_head_sha 与 commit 一致），但 `exec.unit.done` 未出现在任何业务账本 | **62-68** | runtime | DT7 `integrity`（events.jsonl 缺 terminal event 计数）+ `coverage`（orchestration.jsonl 缺 fan-in 状态轨迹） | 第 1 轮：`supervisor/coordinator.rs:528` 看到 `record_slot_terminal_evidence` 写入机制存在；`supervisor/bridge.rs:808` 也声明 `exec.unit.done` topic | **高**（与 DEV-9 同族） | end_head_sha 与 commit 一致 → 确认完成是 ground truth；缺的是 signal 投递 |
| **DEV-7**: U06/U07 `completion.md` 落盘（15:57/16:01）与 w-3 slot finish（15:59:18/16:01:41）跨 stall 真空，无对应 `exec.unit.done` | **60-67** | runtime | DT7 `integrity`（事件账本缺 stall 跨度）+ `correlation`（缺投递时序对账） | 第 1 轮: `supervisor/coordinator.rs:528` + `event_loop/disposition.rs:376-377`；第 2 轮: 补 `presets/schemas/parallel-forge.yml:965-1018` `exec.unit.done` `required_fields` 8 项确认 schema 完备，问题在投递不在 schema | 中 | completion.md 落盘是 agent 自陈行为，不是 supervisor terminal evidence |
| **DEV-2**: w-3 s0/s3 永不收敛（`slot_attempts.running, finished_at=0, no failure_code`） | **58-65** | runtime | DT7 `freeze_window`（事件账本冻结 49min 无法判定是否在 running 中）+ `integrity`（缺 dispatch_records.pid 实际值，因 DEV-5） | 第 1 轮: `supervisor/migrations.rs:426-454` v11 加 `slot_attempts` + `dispatch_records` 表结构；`supervisor/memory_protocol_tests.rs:62` 之后 `fan_in_status` 可读 `in_flight_count` | **高**（与 2026-08-26 forge-dispatcher empty 主因同族） | running slot 没有 `failure_code` → supervisor 不会强制 timeout |
| **DEV-11**: 事件账本 15:34:18 后冻结 49 min，runtime freeze | **60-67** | runtime | DT7 `freeze_window`（缺 49min 跨度 events）+ `refutation`（缺 refutation 候选列表） | 第 1 轮: `event_loop/event_processing.rs:68-185` `terminal_event_emitted` flag + `L622-669` expected_terminal_events 检查；说明 freeze 时可能在等 terminal_event 但 dispatch_records.pid=NULL（DEV-5）使 worker 死亡盲点 | **高**（2026-08-26 R-5 置信度 72: loop.cancel 与 forge.plan.blocked FlowStepScope 双锁死） | 与 DEV-3 同一段真空 |
| **DEV-10**: worker_results / wave_emissions / redrive_requests / compensation_jobs 4 张纠偏账本全空 | **58-65** | runtime | DT7 `coverage`（4 张账本缺任意一行样本）+ `correlation`（缺账本 ↔ events.jsonl 时间相关） | 第 1 轮: `supervisor/migrations.rs:426-454` 仅声明 `dispatch_records` + `slot_attempts` 表；其余 4 张账本在 cap manifest 中也未声明（与 DEV-8 共因） | **高**（2026-08-26 redrive 链置信度 75） | MINIMAL 模式下未生成 4 张纠偏账本，evidence_gap |
| **DEV-3**: 49 min 真空（15:34→16:23）无 transition；reuse-history 捕获时 PID 526040 已死 | **55-62** | runtime | DT7 `freeze_window`（49min 跨度 events 缺失）+ `coverage`（缺 orchestration.jsonl 时间序列） | 第 1 轮：locate `crates/ralph-core/src/event_loop/mod.rs`（loop runner + collect phase）入口；PID 526040 死后 reuse-history 是新机制但 30d 窗口未命中历史同款 | **高**（reuse-history 是新机制 + 30d 命中 8/26 PID 死亡盲点 1 次） | reuse-history 已捕获但 salvage_write_count=0；无法判定是否落到 disk |
| **DEV-5**: dispatch_records.pid 全 NULL（worker 死亡后机制盲点） | **55-62** | runtime | DT7 `integrity`（pid 缺 NULL/非 NULL 二元对比）+ `coverage`（缺 dispatch_records 实际列值） | 第 1 轮：`supervisor/migrations.rs:426` dispatch_records 表结构存在；`supervisor/bridge.rs:72-102` fan_in_lock + `bridge.rs:191` `fan_in_status` 抽象存在；推测 pid 在 worker 死亡时未被回填 | 中（2026-08-26 dispatch_records pattern 同款但弱） | 与 DEV-2 同根（s0/s3 running 永不收敛因 pid NULL 不知 worker 已死） |
| **DEV-8**: cap manifest `execution_capabilities` 空（MINIMAL 模式未生成完整 cap 表） | **65-70** | diagnostic_capture_contract | DT7 5 项均涉及；MINIMAL 模式 sidecar 不写 cap manifest 是 root bundle 缺失根因 | 第 1 轮：locate `crates/ralph-cli/src/diagnostics/`（cap manifest 生成入口，未读源码）；MINIMAL mode flag 决定是否写 cap 表 | **新**（MINIMAL mode cap 空未在历史命中） | 与 §D.1 round 2 evidence_gaps 中 `runtime-trace.jsonl empty` + `feedback.jsonl empty` 同根 |
| **DEV-6**: Wave 1 用 `forge.worktrees.ready` vs Wave 2/3 `forge.wave.worktrees.ready` 命名漂移 | **60-65** | preset | DT7 `coverage`（缺 preset_lint 对单名/复名一致性的 finding）+ `refutation`（缺两 topic 在 runtime 路由的实际差异化） | 第 1 轮：**preset 行级锁定** `presets/en/parallel-forge.yml:73-79` worktree_setup.allowed_emits 双列名 + `L92-99` development_loop.allowed_emits 双列名 + `L76-77` Wave 1 explicit + `L533-538` 重复出现 | 低（30d 窗口未命中同款命名漂移） | operator 视角命名漂移；runtime 视角两 topic 在 main events.jsonl 中已被 supervisor 路由到不同 hat-channel |

**置信度说明**：MINIMAL 模式硬顶 ≤ 75；§7 全部候选均 ≤ 72。DEV-4 最高（file:line 强证据 + 行号具体到 `parallel_forge_resume.rs:316-323`），DEV-9 次之（同族 3 次命中 + 已有 solution 同款）。P0 候选按 P0 阈 < 70 应拦在 §7；其中 DEV-2/3/5/7/11 略低于 70 阈值，DEV-1/9/12 在 65-70 区间，DEV-4 突破 70 但仍受 MINIMAL 上限压制。

---

## 质量门槛

- [x] L0 产物盘点表（§0）
- [x] L1 拓扑表 + 时间轴对比（§2.1, §2.2）
- [x] L2 模式适用：MINIMAL 跳过 orchestration 行（已在 §4.1/4.2 标 N/A）
- [x] L3 产物五证（§4.0 DEV 12 条 + §4.1 OPAC 表）
- [x] L4 机制 R1-R6（§4.4）
- [x] L5 历史深挖（preset-only，§3.1-§3.4）
- [x] L6 源码反查（DEV-4/DEV-6/DEV-12 命中 file:line；其余走双账本）
- [x] L7 归因落盘（§5 空 + §6 advisory only + §7 完整 12 条）
- [x] §5 status 规则：仅 complete 入表；本报告 §5 为空（causal_status=not_evaluable 结构决定）
- [x] 每条 P0 含 DEV 编号 + 证据锚点（§7 表）
- [x] `primary_domain` 枚举严格 5 项（runtime / preset / agent / backend / diagnostic_capture_contract）
- [x] frontmatter 含 `history_search: preset-only`
- [x] frontmatter 含 `causal_status` / `causal_confidence` / `causal_primary_domain` / `causal_rejected_hypotheses` / `causal_score_change`
- [x] frontmatter 含 `bundle` / `bundle_path` / `structured_result_ref` / `trace_status` / `feedback_status` / `activation_outcomes` / `evidence_gaps`
- [x] §3 末尾含一行窗口注脚: `本次扫描窗口: preset-only (30d sliding; 2026-07-30 → 2026-08-29)`
- [x] 路径全部 repo-relative
- [x] `docs/report/` 仅含本最终报告；DIAG_WORKDIR 临时 JSON / stderr 已清理（见 §D.1 末尾说明）