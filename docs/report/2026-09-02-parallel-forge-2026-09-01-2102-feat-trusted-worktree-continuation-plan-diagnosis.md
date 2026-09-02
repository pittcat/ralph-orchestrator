---
title: parallel-forge Loop `2026-09-01-2102-feat-trusted-worktree-continuation-plan` 运行链路诊断报告
date: 2026-09-02
type: diagnosis
loop_id: 2026-09-01-2102-feat-trusted-worktree-continuation-plan
preset: builtin:parallel-forge
run_dir: .worktrees/ralph-orchestrator/2026-09-01-2102-feat-trusted-worktree-continuation-plan/  (worktree 复用名 `2026-09-01-2102-feat-trusted-worktree-continuation-plan-lucky-reed`)
status: 硬阻塞（forge.plan.blocked 在 worktree_setup 阶段 fail-close；cleanup → reporter(BLOCKED) → LOOP_COMPLETE 落定；orphan cleanup 二次 emit 被 terminal_monotonicity_violation 拒收）
diagnostics_mode: FULL
bundle: finalized
bundle_path: .ralph/diagnostics/2026-09-01T21-20-42/diagnosis-input.json
causal_status: incomplete
causal_confidence: 69
causal_primary_domain: runtime
causal_rejected_hypotheses:
  - backend: "backend.outcome.success=true (runtime-trace.jsonl kind=hat_activation_outcome no_failure_row)"
  - preset: "contract_digest.terminal_topics_present=true (contract_receipt fields.terminal_topics all_terminal_topics_visible)"
  - agent: "no_terminal_event_emitted (session_summary terminal no_terminal)"
  - diagnostic_capture_contract: "coverage.gap=false (diagnosis-input.json boundary_coverage all_boundaries_covered)"
causal_score_change:
  - reason: "初始打分"
    prev: "N/A (initial scoring)"
    current: 69
    delta: "—"
    primary_domain_changed: "—"
    rejected_hypotheses_added: "—"
  - reason: "加深 1 轮（重跑 ralph diagnose --causal 无新 evidence 补入，diff 除 summary.generated_at 外无差异）"
    prev: 69
    current: 69
    delta: 0
    primary_domain_changed: false
    rejected_hypotheses_added: "无"
history_search: preset-only
structured_result_ref: "inline: summarized in report"
trace_status: present
feedback_status: present
activation_outcomes: present
evidence_gaps:
  - "accepted-transitions.jsonl 仅 1 条 evidence_ref（00482bf6...32eba 缺 commit_receipt），integrity join 不完整（integrity=5/25）"
  - "freeze_window=0，诊断窗口未冻结，无法补 deferred evidence"
  - "boundary_coverage 8/8 但 3 boundary（state_commit/recovery_action/termination）expected=0/recorded=0（fail-close 未走完链）"
  - "events ledger iter 17 cleanup \"accepted\" 与 stage pipeline flow_unknown_emit 拒绝同时存在（DEV-003 双 ledger 不一致）"
  - "drift window=10 但 forge.plan.blocked 仅 5 events，drift_field_completeness 4 条 critical findings 是低样本假阳性（DEV-004）"
---

# parallel-forge Loop `2026-09-01-2102-feat-trusted-worktree-continuation-plan` 运行链路诊断报告

> **生成时间**: 2026-09-02T00:33Z (UTC)
> **诊断对象**: worktree `ralph/2026-09-01-2102-feat-trusted-worktree-continuation-plan-lucky-reed` (sha=721b8c61, base=`pittcat-dev`) 内 `.ralph/` 与 `.ralph/diagnostics/2026-09-01T21-20-42/`（loop_id=..., 13:20:42 启动 → 13:58:50 LOOP_COMPLETE → 14:34:32 orphan cleanup → 16:33:29 loop.terminate）
> **对照 preset**: `presets/en/parallel-forge.yml` + `presets/schemas/parallel-forge.yml`
> **执行方式**: 4 sub-agent 并行（流程还原 / 历史 / 对账 / 归因）→ 汇总
> **Diagnostics 模式**: FULL（bundle.status=finalized，8 boundary 全 covered）
> **history_search**: `preset-only`（30d sliding）— 来自主 SKILL §0.1 AskUserQuestion
> **execution_capabilities**: `[supervisor, wave]` — Phase 0 推断（`diagnosis-input.json` `execution_capabilities=[supervisor,wave]` + preset 内 `event_loop.supervisor.enabled=true` + hats 含 `ralph wave emit` 协调面）
> **报告仓库**: `ralph-orchestrator` 主仓（非 run_dir）
> **Tier C 根**: `.ralph/forge/2026-09-01-2102-feat-trusted-worktree-continuation-plan/{blocks/worktree-blocked.md, worktree-map.yml, execution-plan.yml, development-plan.md, concurrency-approval.md, inspection-report.md, cleanup.md, reports/}`
> **置信度规则**: §5 仅收录 `status == complete`（DT7 机检 confidence > 85）；本 run causal_status=`incomplete`（confidence.total=69，DT7 严格门禁 `>85` 阻止入表）→ **§5 必须空**，全部 7 条 DEV 移入 §7 不驱动修复

---

## 0. 产物盘点（Phase 0 必附）

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | events（current-events 解析 = `.ralph/events-20260901-132042.jsonl`） | ✓ | 9 | 最后 1 行为 14:34:32 cleanup orphan emit（LOOP_COMPLETE 后 35min44s） |
| S | events-history（配对 history = `.ralph/events-history-20260901-132042.jsonl`） | ✓ | 2 | 仅 `forge.start` + `loop.terminate`（canonical clock） |
| S | ledger.jsonl | ✓ | 15 | 含 `loop.completion_requested` iter 5 + iter 17 cleanup "accepted in batch 0"（与 stage `flow_unknown_emit` 冲突，DEV-003） |
| S | flow-authority.jsonl | ✓ | ~58 | 末段 `step=plan_end, topic=forge.plan.blocked` 大量重复（MEMORY `flow-authority-stale-tail-pollutes-recovery` 关联） |
| S | recovery.jsonl | ✓ | 8 | 1 info (agent_doc_sync) + 4 critical drift_field_completeness (iter 10) + 1 warning flow_unknown_emit (iter 17) + 1 warning handoff_dispatch_timeout escalated (iter 22) + 1 outcome update pending |
| A | agent/tasks.jsonl | ✓ | 5 | U01-U05 全 `status=open`, `owner=executor`, executor hat 0 激活（DEV-007） |
| A | docs/reports/.../manager-report.md | ✓ | 20860 B | reporter 写 `status=BLOCKED`, `final_audit=BLOCKED`, 5 unit 0 完成 |
| A | blocks/worktree-blocked.md | ✓ | 4996 B | worktree hat 自述根因与三条 evidence_refs（DEV-001 主证据） |
| B | diagnostics mode | FULL | — | orchestration.jsonl + errors.jsonl **缺失**（warnings 2 条）；bundle.status=`finalized` |
| B | `.ralph/supervisor.db` | ✓ | 155648 B | capability=supervisor 启用，evidence 文件 |
| B | `.ralph/forge/<plan>/worktree-map.yml` | ✓ | — | 2 unit (U01, U05), wave 1, integration_branch=`forge/.../integration`, target_branch=`ralph/...`, base_commit=721b8c61 |
| B | `.ralph/diagnostics/<session>/runtime-trace.jsonl` | ✓ | 174576 B | 61 `phase=activation` `kind=hat_activation_outcome` rows；status: 7 merged + 54 empty |
| B | `.ralph/diagnostics/<session>/feedback.jsonl` | ✓ | 5264 B | 14 行（7 feedback_id × 2 phases：discovered + evidence） |
| B | `.ralph/diagnostics/<session>/drift.jsonl` | ✓ | 1686 B | 4 critical `drift_field_completeness` on `forge.plan.blocked`（低样本假阳性，DEV-004） |
| B | `.ralph/diagnostics/<session>/diagnosis-input.json` | ✓ | 3365 B | bundle entrypoint, v2 manifest, 8/8 boundary covered, execution_capabilities=[supervisor,wave] |
| C | `.ralph/forge/<plan>/execution-plan.yml` | ✓ | 37518 B | planner 产出，unit_count=5，wave_total=1 |
| C | `.ralph/forge/<plan>/development-plan.md` | ✓ | 36830 B | planner 产出 |
| C | `.ralph/forge/<plan>/inspection-report.md` | ✓ | 9585 B | inspector 产出 |
| C | `.ralph/forge/<plan>/concurrency-approval.md` | ✓ | 12857 B | guardian 产出 |
| C | `.ralph/forge/<plan>/cleanup.md` | ✓ | 161234 B | cleanup hat 多 activation 累计报告（含 activation 28-52 的 `terminal_monotonicity_violation` 实测） |
| C | `.ralph/forge/<plan>/templates/` | ✓ | — | 模板目录（未触达） |
| C | git branches: `forge/.../integration` + `<plan>/U01` + `<plan>/U05` + `ralph/<plan>` | ✓ | — | 5 条 ref（实测 activation 60 时 U01/U05 已不存在，integration tip=721b8c61） |
| C | `.worktrees/` | 不存在 | 0 | worktree-map 声明 U01/U05 但 git worktree 已 prune（cleanup activation 60 实测） |

**execution_capabilities 推断结果**（Phase 0 必填）: `[supervisor, wave]`

| Capability | 判定信号 | 证据锚点 |
|---|---|---|
| `supervisor` | YAML `event_loop.supervisor.enabled: true` + `.ralph/supervisor.db` 存在 + `diagnosis-input.json.execution_capabilities=[supervisor,wave]` | `presets/en/parallel-forge.yml` + `.ralph/supervisor.db` 155648B + diagnosis bundle |
| `wave` | hat `instructions` 含 `ralph wave emit` / `ralph wave verify` + 实际 waves (`worktree-map.yml` wave_id=`wave-...-1`) + execution_plan.yml wave 链 | preset topology + worktree-map.yml + events.jsonl timeline |

**缺失产物 → 故障判定**（capability-triggered）:

- `.ralph/supervisor.db` 存在 → **N/A** (capability +supervisor 已满足)
- events 有 `wave_id`（worktree-map.yml 间接证实 wave-1）→ **N/A** (capability +wave 已满足)
- orchestration.jsonl 缺失 → **N/A**（recovery_count=1, drift_count=4 已能从 feedback.jsonl + recovery.jsonl 重建，非 P0 阻断）

**盲区 / 根因置信度硬顶**:

- DT7 严格门禁下，本 run **§5 必须空**（status=incomplete, confidence.total=69 < 85）。所有 P0/P1 候选移入 §7，不驱动 §6 修复建议
- accepted-transitions.jsonl 缺 commit_receipt join（integrity=5/25），导致 fix_point 元数据不完整
- freeze_window=0，无法 deferred 补 evidence

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: **硬阻塞**（`forge.plan.blocked` 在 worktree_setup 阶段 fail-close → cleanup(retained_for_diagnosis) → reporter(BLOCKED) → LOOP_COMPLETE 落定；orphan cleanup 二次 emit 被拒）
- **P0 / P1 / P2 数量**: **§5 = 0**（DT7 严格门禁，causal.status=incomplete 阻止入表）
- **§7 未核实疑点**: 7 条候选（DEV-001 至 DEV-007），全部 confidence 60-69 区间，不驱动修复
- **最高优先级根因置信度**: **dev-001 = 69 / 100**（compound runtime + preset，fail-close 触发点；与 MEMORY `precheck-desugar-allowed-emits-mismatch` 同根因复发）
- **历史复发**: 是 — 30d 窗口内 8/26 + 8/29 两份诊断报告关联度高（同根因机制 `apply_precheck_desugar` 改写不全 + `FlowStepScope` 严格相等）；引用 `docs/report/2026-08-26-parallel-forge-2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan-diagnosis.md` + `docs/report/2026-08-29-parallel-forge-2026-08-27-1430-feat-parallel-forge-evidence-gates-plan-diagnosis.md`

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 半合规（早期 4 hat OPAC 全绿；worktree hat OPAC A 列异常 fail-close 触发点；cleanup 2nd orphan OPAC C 列异常） | Agent C §4.2 OPAC 表：4/6 activated hat OPAC 全绿，2/6 异常 | **65**（runtime 双 ledger 不一致致 OPAC 可信度降级） |
| Q2 | 基座机制是否正常生效？ | ❌ `apply_precheck_desugar` 改写范围不完整（R5）+ `FlowStepScopeStage::allows_topic` 严格相等（R2）共同触发 `flow_unknown_emit` | `ralph_config.rs:130-208` rewrite_emit_topics 不触 mechanism.flow.steps.allowed_emits + `flow_step_scope_stage.rs:246-248` 严格相等 | **69**（DEV-001，机制层 file:line 充分；缺 commit_receipt join 阻完整入表） |
| Q3 | 编排是否合理、正常运行？ | ⚠️ 编排 4/5 段（start → inspected → ready → concurrency.approved）正常；worktree_setup 段 fail-close；cleanup → reporter → LOOP_COMPLETE 落定但留 orphan；其余 9 个 hat 全部未触达 | Agent A §2.2 时间线对比表 | **69** |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **Compound runtime + preset**（agent 无独立归因；preset YAML bare form 是必要条件；runtime `apply_precheck_desugar` 改写不全 + `FlowStepScope` 严格相等等价类是充分条件） | MEMORY `precheck-desugar-allowed-emits-mismatch` + Agent C §4.3 R1-R6 | **69**（取 §7 最高 dev-001） |

### 1.3 根因一句话

**机制层根因（runtime）**：`crates/ralph-core/src/config/ralph_config.rs:130-208` 的 `apply_precheck_desugar` 通过 `hat.rewrite_emit_topics(topic, &proposed)` 将 `hat.publishes` 改写为 `.proposed` 形式，但 **未** 同步改写 `mechanism.flow.steps[*].allowed_emits`；preset `presets/en/parallel-forge.yml:72-78` 的 `worktree_setup.allowed_emits` 保留裸 `forge.worktrees.ready`；`flow_step_scope_stage.rs:246-248` 严格相等比较 → worktree hat 三种字面量（全 `.proposed` / 全裸 / `forge.wave.worktrees.ready`）全部 `flow_unknown_emit` 拒收。**Preset 层必要条件**：`worktree_setup.allowed_emits` 未声明 `.proposed` 形式。**Agent 端无解**：经 worktree hat 两次重试（block 时刻 `ralph emit forge.worktrees.ready.proposed --policy-check` 与 `ralph emit forge.worktrees.ready --policy-check` 全部 flow_unknown_emit）后 fail-close 至 `forge.plan.blocked`。**附随效应**：orphan cleanup 二次 emit（14:34:32）在 LOOP_COMPLETE（13:58:50）后被 `terminal_monotonicity_violation` 拒收但仍写入 events ledger L8；drift_monitor 4 条 critical findings 是低样本假阳性（5 events vs window=10）。**置信度 69**（来自 `ralph diagnose --causal` JSON，DT7 status=incomplete 阻止入 §5）。

### 1.4 终态时序一致性（event-artifact chronology）

| 项目 | 内容 |
|------|------|
| **首轮终态（initial_terminal_status）** | **首轮失败（REJECTED/BLOCKED）** — worktree hat 在 13:52:21 emit `forge.plan.blocked`（被 events ledger 接受），reason=`apply_precheck_desugar allowed_emits mismatch` |
| **恢复状态（recovery_status）** | **无恢复（artifact 被改但无后续 accepted 成功事件）** — cleanup hat 13:56:22 落盘 `forge.cleanup.done`（accepted），但 cleanup_status=`retained_for_diagnosis`（保留诊断，非"成功清理"）；后续 reporter 13:58:41 落盘 `forge.report.done`（status=BLOCKED, final_audit=BLOCKED）；13:58:50 `LOOP_COMPLETE` 接受 |
| **最终代码状态（final_code_state）** | workspace HEAD = `721b8c613fdf2b2d758ada19a6e7b5e7f34bba3a`（= plan 提交，baseline 本身）；integration_branch tip = `721b8c61`（从不被 Unit execution 推进）；5 个 slot branch（U01-U05）按 cleanup 策略保留诊断（实测 activation 60 时 U01/U05 已 prune 完，仅 integration + ralph/<plan> 两条分支存在） |
| **一致性告警** | ⚠️ **失败终态后无恢复**：首轮 audit/report 为 BLOCKED，后续 orphan cleanup 二次 emit（14:34:32，35min44s 后）被 stage pipeline 拒收（flow_unknown_emit → terminal_monotonicity_violation）但仍写入 events ledger L8（DEV-003 双 ledger 不一致）；cleanup.md activation 28-52 全报 `ok=false terminal_monotonicity_violation`。**禁止** 输出「零拒收」或「首轮完整成功」 |

---

## 2. 执行链路对比图

### §2.1 拓扑激活表（hat 预期 vs 实际）

| Hat | 预期激活次数 | 实际激活次数 | 状态 | 实际产出事件 | 偏差说明 |
|---|---|---|---|---|---|
| inspector | ≥1 | **1** | 成功 | `forge.plan.inspected` (13:23:58) | 正常 |
| planner | ≥1 | **1** | 成功 | `forge.plan.ready` (13:29:29, unit_count=5) | 正常 |
| guardian | ≥1 | **1** | 成功 | `forge.concurrency.approved` (13:31:35) | 正常 |
| worktree | ≥1 | **1** | **blocked** | `forge.plan.blocked` (13:52:21) | **apply_precheck_desugar allowed_emits 不含 `.proposed` 形式 fail-close**（DEV-001） |
| forge-dispatcher | ≥1 | **0** | skipped | — | worktree_setup 提前阻断，未发出 `forge.worktrees.ready` |
| forge-failure-handler | ≥1 if `exec.wave.failed` | **0** | skipped | — | 未进入 wave 调度 |
| executor | ≥1 | **0** | skipped | — | 无 `exec.unit.ready` |
| reviewer | ≥1 | **0** | skipped | — | 无 `exec.unit.done` |
| wave-fixer | ≥1 if issues | **0** | skipped | — | 未进入 review 阶段 |
| integrator | ≥1 | **0** | skipped | — | 无 `forge.units.reviewed` |
| verifier | ≥1 | **0** | skipped | — | 无 `forge.wave.settled` |
| tester | ≥1 | **0** | skipped | — | 无 `forge.wave.verified` |
| auditor | ≥1 | **0** | skipped | — | 无 `forge.full.verified` |
| finalizer | ≥1 | **0** | skipped | — | 无 `forge.audit.done` |
| cleanup | ≥1 | **2** | 半成功 | 首次 `forge.cleanup.done` 13:56:22 (accepted)；第二次 `forge.cleanup.done` 14:34:32 (orphan, 被拒) | 第二次为 LOOP_COMPLETE 之后的 ghost emit，被 stage `FlowStepScope` 拒 `flow_unknown_emit`（DEV-002/003） |
| reporter | ≥1 | **1** | 成功 | `forge.report.done` (BLOCKED, 13:58:41) → `LOOP_COMPLETE` (13:58:50) | 正常落定阻塞态 |

### §2.2 时间轴对比表（preset 预期 vs 实际）

| Step | 预期事件 | 实际事件 | 时间戳 | 偏差类型 |
|---|---|---|---|---|
| 1 | `forge.start` | `forge.start` | 2026-09-01 13:20:42 | 一致 |
| 2 | `forge.plan.inspected` | `forge.plan.inspected` | 13:23:58 | 一致 |
| 3 | `forge.plan.ready` | `forge.plan.ready` | 13:29:29 | 一致（unit_count=5, plan_digest 已生成） |
| 4 | `forge.concurrency.approved` | `forge.concurrency.approved` | 13:31:35 | 一致 |
| 5 | `forge.worktrees.ready` 或 `.proposed` | **`forge.plan.blocked`** | **13:52:21** | **早退（root cause，DEV-001）** |
| 6..N | exec/wave/review/integrate/verify/test/audit/finalize | — | — | **跳过**（拓扑未展开） |
| N+1 | `forge.cleanup.done` (accepted) | `forge.cleanup.done` (retained_for_diagnosis, 2/2) | 13:56:22 | 一致 |
| N+2 | `forge.report.done` (BLOCKED) | `forge.report.done` (BLOCKED) | 13:58:41 | 一致 |
| N+3 | `LOOP_COMPLETE` | `LOOP_COMPLETE` | 13:58:50 | 一致 |
| N+4 | (loop 已结束，无后续) | **`forge.cleanup.done`** (orphan, 0/3) | **14:34:32** | **stalled**（35m44s 后 ghost emit，stage `flow_unknown_emit` 拒收；DEV-002/003） |
| N+5 | (无) | stall_recovery iteration 22 升级 reporter (`handoff_dispatch_timeout 600s`) → runtime:loop.stalled | 14:50:10 | stalled（DEV-006 误派） |
| canonical | (无) | `loop.terminate` (canonical clock) | 16:33:29 | stall-detector 耗尽 iterations（loop-termination-reason.json=`stopped`） |

### §2.3 流程图（mermaid，颜色编码：绿=成功，红=blocked，橙=skipped，灰=stalled）

```mermaid
flowchart TD
    classDef success fill:#d4f4d4,stroke:#2a8a2a,color:#1a4d1a
    classDef blocked fill:#f4c4c4,stroke:#a02020,color:#5a1010
    classDef skipped fill:#fde0b3,stroke:#c08020,color:#6a4010
    classDef stalled fill:#d8d8d8,stroke:#707070,color:#303030,stroke-dasharray: 4 2

    Start([forge.start<br/>13:20:42]):::success
    Inspected([forge.plan.inspected<br/>inspector<br/>13:23:58]):::success
    Ready([forge.plan.ready<br/>planner · unit_count=5<br/>13:29:29]):::success
    Approved([forge.concurrency.approved<br/>guardian<br/>13:31:35]):::success
    Blocked([forge.plan.blocked<br/>worktree · precheck_desugar<br/>allowed_emits 不含 .proposed<br/>13:52:21]):::blocked

    Dispatcher[forge-dispatcher<br/>未激活]:::skipped
    Executor[executor × 5 units<br/>未激活]:::skipped
    Reviewer[reviewer<br/>未激活]:::skipped
    Integrator[integrator<br/>未激活]:::skipped
    Verifier[verifier<br/>未激活]:::skipped
    Tester[tester<br/>未激活]:::skipped
    Auditor[auditor<br/>未激活]:::skipped
    Finalizer[finalizer<br/>未激活]:::skipped

    Cleanup1([forge.cleanup.done<br/>retained_for_diagnosis 2/2<br/>13:56:22]):::success
    Reported([forge.report.done<br/>status=BLOCKED<br/>13:58:41]):::success
    LoopComplete([LOOP_COMPLETE<br/>13:58:50]):::success

    Ghost[/forge.cleanup.done (orphan)<br/>0/3 · 14:34:32<br/>flow_unknown_emit 拒收/]:::stalled
    Stall[/stall_recovery escalated<br/>14:50:10 · DEV-006/]:::stalled

    Start --> Inspected --> Ready --> Approved --> Blocked
    Approved -.->|未发出 forge.worktrees.ready| Dispatcher
    Dispatcher -.->|无 exec.unit.ready| Executor
    Executor -.->|无 exec.unit.done| Reviewer
    Reviewer -.->|无 forge.units.reviewed| Integrator
    Integrator -.->|无 forge.wave.settled| Verifier
    Verifier -.->|无 forge.wave.verified| Tester
    Tester -.->|无 forge.full.verified| Auditor
    Auditor -.->|无 forge.audit.done| Finalizer

    Blocked --> Cleanup1 --> Reported --> LoopComplete
    LoopComplete -.->|35m44s 后 ghost emit| Ghost
    Ghost -.->|stall_recovery iter 22 误派| Stall
```

### §2.4 终止类型

- **类型**: 硬阻塞（`forge.plan.blocked` → `cleanup`（`retained_for_diagnosis`） → `reporter`（`status=BLOCKED, final_audit=BLOCKED`） → `LOOP_COMPLETE`）
- **主导终态事件**: `forge.plan.blocked` @ 13:52:21（worktree hat, reason=`apply_precheck_desugar allowed_emits mismatch`）
- **上报终态**: `forge.report.done` @ 13:58:41（status=BLOCKED, final_audit=BLOCKED）
- **顶层终止**: `LOOP_COMPLETE` @ 13:58:50
- **canonical clock 终止**: `loop.terminate` @ 16:33:29（loop-termination-reason.json=`stopped`；stall-detector 耗尽 ~max_iterations）
- **副作用**: `cleanup_status=retained_for_diagnosis`（保留 branch 与 worktree 用于诊断）
- **后置异常**: LOOP_COMPLETE 后 35m44s 出现第二次 `forge.cleanup.done`（attempted=3/cleaned=0/pending=3），feedback 链路确认 `flow_unknown_emit:flowstepscope` 拒收 + `stall_recovery:reporter:forge_cleanup_done:handoff_dispatch_timeout` 升级，最终触发 `runtime:loop.stalled` 事件（iter 22, ts=14:50:10），属于 terminal_monotonicity_violation。**不影响主 ledger 落定的 BLOCKED 终态**

### §2.5 实际激活 hat 计数（来自 runtime-trace.jsonl）

- merged（成功合入 hat-channel）: 7 次 — inspector / planner / guardian / worktree / cleanup（iter 5, 13:56:43）/ reporter（iter 6, 13:58:58）/ cleanup（iter 17, 14:37:29, output_mentions_emit=false）
- empty（hat 激活但 0 candidate emit）: 54 次 — 主要是 stall-detector 触发的 cleanup 重复 activation（iter 8-60+）
- terminal_obligation_topics: merged hats 全部声明；empty hats 大多声明 `forge.cleanup.done` 但未实际触发

---

## 3. 历史关联扫描（preset-only, 30d sliding）

> 扫描窗口: **2026-08-02 ~ 2026-09-02**（preset-only）。目录边界: `docs/report/*-diagnosis.md`（30d）+ `docs/solutions/{integration-issues,logic-errors,state-management}/` + `docs/plans/`（active）。**未读** `.ralph/`、`docs/achieved/plan/`、本次 run 私有目录。

### §3.1 历史全景表

| # | 类型 | 文档路径 | 命中次数 | 闭环 | 与本次关联度 | 关键症状落点 |
|---|---|---|---|---|---|---|
| 1 | diagnosis | `docs/report/2026-08-26-parallel-forge-2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan-diagnosis.md` | 9 | ✗ (fail-close) | **高** | fail-close emit `plan.blocked`（无 `forge.` 前缀）+ namespace 错配 + flow-authority 不 advance + cleanup 不订 `forge.wave.settled` 兜底链缺失 + DEV-6 Wave 1 `forge.worktrees.ready` vs Wave 2/3 `forge.wave.worktrees.ready` 双名 |
| 2 | diagnosis | `docs/report/2026-08-29-parallel-forge-2026-08-27-1430-feat-parallel-forge-evidence-gates-plan-diagnosis.md` | 6 | ✗ (Wave 3 fan-in 卡死) | **高** | Wave 3 `exec.unit.done` 业务事件丢失 → `forge.wave.worktrees.ready` 步永久不前进 → flow 锁死；明示引用 MEMORY `parallel-forge-cleanup-after-loop-complete`；DEV-3 reuse-history 49 min 真空；DEV-6 命名漂移复发 |
| 3 | diagnosis | `docs/report/2026-07-29-parallel-forge-primary-20260729-020808-diagnosis.md`（mtime 2026-08-17 归档纳入） | 3 | ✗ | 中 | 同 preset 内 stale-tail → flow 锁死先例（命中 `flow_unknown_emit` / `allowed_emits`） |
| 4 | solution | `docs/solutions/integration-issues/mechanism-foundation-validation-2026-06-27.md`（注: 6/27 落在窗口外，仅作"已落地机制层"参考） | 1 | ✓ | 低 | `allowed_emits` 校验机制背景，不指向本次症状 |
| 5 | plan (active) | `docs/plans/2026-09-01-001-feat-forge-signal-delivery-reliability-plan.md` | — | active | 中 | 与本次 plan 同方向（forge 信号可靠性），但非本次 loop 驱动 plan |
| 6 | plan (active) | `docs/plans/2026-09-01-2102-feat-trusted-worktree-continuation-plan.md` | — | active | **本次 loop plan** | 本次 run 锚定 plan |
| 7 | plan (active) | `docs/plans/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan.md` | — | active | 中 | 8/29 诊断的源头 plan；其 U0X evidence-gates 是 #8/26 失败症状的体系化收敛 |
| 8 | plan (active) | `docs/plans/2026-08-27-001-feat-sync-multi-wave-concurrency-to-operator-skills-plan.md` | — | active | 低 | wave 并发 operator 同步；与命名漂移关联 |

### §3.2 根因分类对照

| 根因类型 | 历史报告数 | 落地修复 | 与本次对应 |
|---|---|---|---|
| `flow_unknown_emit` (FlowStepScope 不允许 emit) | 3 (#1, #2, #3) | MEMORY `parallel-forge-fail-close-flow-authority-stale`（2a29e24 系列 + 8/05 后续） | **高命中**: 本次 run 是 `apply_precheck_desugar` 改 `.proposed` 但不改 `mechanism.flow.steps.allowed_emits` 触发的同根因复发（MEMORY `precheck-desugar-allowed-emits-mismatch`） |
| `terminal_monotonicity_violation` (cleanup 在 LOOP_COMPLETE 后被拒) | 2 (#2, #3; 含 plan-reporter 链路) | MEMORY `parallel-forge-cleanup-after-loop-complete` + `parallel-forge-cleanup-agent-context-precheck-convergence` | **高命中**: #2 8/29 诊断明示引用该 memory；本次 run `cleanup_after_LOOP_COMPLETE` 关键词直接对应 |
| `worktree_setup` 命名漂移 (`forge.worktrees.ready` vs `forge.wave.worktrees.ready`) | 2 (#1 DEV-6, #2 DEV-6) | 未落地（8/29 诊断列为 P2 残留） | **高命中**: 同 dual emit 名问题在 Wave 1 vs Wave 2/3 命中；本次 run 的 `worktree_setup` 关键词直接对应 |
| `forge.plan.blocked` namespace 错配 (`plan.blocked` 无前缀 vs preset 协议 `forge.plan.blocked`) | 2 (#1 fail-close β, #3 fail-close 双根因) | MEMORY `parallel-forge-fail-close-flow-authority-stale` 第 1 条（机制 α）+ 命名空间统一 (U3+ 提案) | **中命中**: 本次 run 是否再次触发该错配需 Agent D 验证；若 yes 则与历史同构 |
| `consecutive_no_progress` hard gate → fail-close 过快 | 1 (#2 R-4 置信度 75) | plan `2026-08-27-1430-feat-parallel-forge-evidence-gates-plan` 体系化收敛中 (evidence-gates) | **中命中**: 若本次 run verifier/cleanup/reporter 链断导致 consecutive 触发，会沿同一路径 fail-close |
| `task.resume` 死信 (flow-authority stale-tail) | 1 (#1) +1 (#2 DEV-12) | MEMORY `flow-authority-stale-tail-pollutes-recovery` | **低命中**: 本次 run 未在 symptom 列表出现 stale-tail 字样（本次是 `precheck_desugar` 路径，非 stale-tail 路径） |
| `loop_id` 二次启动 cleanup debt | 0 (memory-only, 30d diagnosis 未命中) | MEMORY `parallel-forge-loop-relaunch-flow-cleanup-debt` | **低命中**: 本次 run loop_id `2026-09-01-2102-...` 系首次启动，无 relaunch 信号 |

### §3.3 本次为新问题模式？

**否**，本次为**旧根因复发**。

1. **`flow_unknown_emit` + `allowed_emits` + `precheck_desugar` 三角**: 与 MEMORY `precheck-desugar-allowed-emits-mismatch` 同根因——`apply_precheck_desugar` 改 `hat.publishes` 为 `.proposed` 但不改 `mechanism.flow.steps.allowed_emits`，FlowStepScope 严格相等比对 → `flow_unknown_emit` 拒。该机制在 30d 窗口内**已有 #1(8/26)、#2(8/29)两份诊断报告同族命中**，本次是该根因的真实运行时复发。
2. **`terminal_monotonicity_violation` + `cleanup_after_LOOP_COMPLETE`**: MEMORY `parallel-forge-cleanup-after-loop-complete` + `parallel-forge-cleanup-agent-context-precheck-convergence` 已分别在 #2(8/29) 诊断中显式引用；本次 run 命中同一 memory 路径。
3. **`worktree_setup` 命名漂移**: 8/26(#1) + 8/29(#2) 两份诊断均识别为 DEV-6 残留，本次 run 命中同一关键词。
4. **未在 30d 窗口内发现新问题模式**:
   - 历史报告集中分布在"fail-close 双根因(α/β) + 命名漂移 + 兜底链缺失"三族；
   - 本次 run 三族关键词均命中，**结论是同根因复发 + memory 已有覆盖**，非新模式。
   - `precheck_desugar` 词条在 30d 诊断文本中**无字面命中**（grep 0 hit），但症状 `flow_unknown_emit` + `allowed_emits` 已确认是同一机制层的两个面——本次 run 增加了 `precheck_desugar` 这一**根因词条的新命中**，但作为机制层归属仍属旧模式复发。

### §3.4 窗口注脚

`本次扫描窗口：preset-only (30d sliding)`

---

## 4. 证据清单

### §4.1 偏离证据表（Deviation Evidence Table）

| ID | 描述 | 证据锚点 | 严重度 | DT7 分项来源 | Gap | 初始责任域标签 |
|---|---|---|---|---|---|---|
| **DEV-001** | `apply_precheck_desugar` 改写 `hat.publishes` 为 `.proposed` 形式，但未同步改写 `mechanism.flow.steps[id=worktree_setup].allowed_emits` —— 两者在 `FlowStepScopeStage::allows_topic` 严格相等比较（`flow_step_scope_stage.rs:246-248`）下导致 `flow_unknown_emit`，worktree hat 三种 topic 字面量（`.proposed` / bare / `forge.wave.worktrees.ready`）全被拒 | `presets/en/parallel-forge.yml:72-78`（worktree_setup.allowed_emits = bare） + `:773`（hat.publishes = bare, desugar 后改 .proposed） + `crates/ralph-core/src/config/ralph_config.rs:130-208`（`hat.rewrite_emit_topics` 不触 mechanism.flow.steps.allowed_emits） + `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs:246-248`（严格相等） + `.ralph/forge/.../blocks/worktree-blocked.md`（worktree hat 自述"两次重试全部 .proposed 与 bare 都 fail-close"） | **P0 初判** | worktree-blocked.md § Root cause | integrity.join 缺 commit_receipt（accepted-transitions `00482bf6...32eba` 无 receipt） | **compound (runtime + preset)** |
| **DEV-002** | 第二次 cleanup（`forge.cleanup.done` @ 14:34:32, cleanup-2nd orphan）由 stall_recovery 唤醒，在 LOOP_COMPLETE 已落盘后被 `terminal_monotonicity_violation` 拒；但 `events-20260901-132042.jsonl` L8 仍写入 hat=cleanup ts=14:34:32，events ledger iter 17 也标 "accepted in batch 0"，与 cli_emit warning `flow_unknown_emit` @ 14:37:29 存在 ~3min 时间差与状态不一致 | `events-20260901-132042.jsonl` L8 + `ledger.jsonl` iter 17（cleanup done "accepted"） + `recovery.jsonl` L6（`cli_emit` `flow_unknown_emit` @ 14:37:29） + MEMORY `parallel-forge-cleanup-after-loop-complete` | **P1 初判** | recovery.jsonl + events.jsonl | events ledger vs flow stage 拒绝的状态对外一致性问题（ledger 接受 vs stage 拒绝） | **runtime** |
| **DEV-003** | iteration 17 cleanup 2nd 触发 `flow_unknown_emit`（stage `FlowStepScope` 拒，因 flow-authority 已停在 `step=plan_end topic=forge.report.done`），但 events ledger iter 17 仍标 accepted —— ledger 与 stage pipeline 不一致 | `recovery.jsonl` L6（`stage_pipeline rejection: stage=FlowStepScope reason=flow_unknown_emit topic=forge.cleanup.done`） + `flow-authority.jsonl` L6（`step=plan_end, topic=forge.report.done` —— 已是终态） | **P1 初判** | recovery.jsonl | ledger 接受 vs stage 拒绝的双 ledger 一致性 | **runtime (ledger ↔ stage pipeline)** |
| **DEV-004** | drift_monitor 4 条 critical `drift_field_completeness`（`plan_path` / `context_artifact_path` / `forge_artifact_root` / `plan_key` on `forge.plan.blocked`, observed=0.2, threshold=0.85, window=10）为低样本假阳性 —— window 内仅 5 个 `forge.plan.blocked` 事件样本，1/5 = 20%，统计功效不足 | `recovery.jsonl` L2-5 + `drift.jsonl`（4 critical finding 各 observed=0.200, threshold=0.850, window=10） + `ralph.forge.yml:64`（`drift.field_completeness_threshold=0.85`） | **P3 初判** | drift.jsonl | drift_monitor 窗口大小 vs 实际事件样本数不匹配（只有 5 个事件，达不到 window=10 的最小样本要求） | **runtime (drift monitor 阈值/窗口失配)** |
| **DEV-005** | diagnosis bundle 5 个边界全覆盖，但 `state_commit` / `recovery_action` / `termination` 三个 boundary expected=0 / recorded=0，反映 fail-close 路径未真正走过 state_commit 与 termination 完整链 | `diagnosis-input.json` `boundary_coverage`（8/8 covered, 但 3 个 boundary 全是 0/0） | **P3 初判** | diagnosis-input.json | fail-close 路径不写 state_commit / termination（plan.blocked 直接走 cleanup 而非 normal completion） | **runtime (boundary 覆盖语义)** |
| **DEV-006** | stall_recovery iteration 22 升级 reporter（`handoff_dispatch_timeout 600s` for `forge.cleanup.done` @ 14:34:32 → reporter 未在 600s 内 activate），但 reporter 实际在 iter 6 已 activated（merged），说明 stall_recovery 误把"已被拒收的事件"当作待消费事件再次派发 —— handoff 路由表未及时清理 | `recovery.jsonl` L7（`stall_recovery`, source_hat=reporter, target_hat=reporter, handoff `2026-09-01T14:34:32:forge.cleanup.done` accepted 但 no activation 600s） + L8（`outcome updated to Pending`） + runtime-trace.jsonl（reporter merged iter 6 @ 13:58:58, iter 22 未再 activate） | **P2 初判** | recovery.jsonl | handoff dispatch timeout 600s 触发 reporter，但 reporter 实际已被 orphan 事件污染了 dispatch table | **runtime (stall_recovery 误派)** |
| **DEV-007** | 5 个 task（U01-U05）全部 `status=open`、`owner=executor`，但 executor hat 在 runtime-trace.jsonl 中激活计数为 0（从未被 supervisor 激活）—— task 创建时机早于 executor 调度窗口 | `.ralph/agent/tasks.jsonl`（5 tasks all open, owner=executor） + runtime-trace.jsonl（executor 不在 merged 列表） | **P2 初判** | tasks.jsonl + runtime-trace | task 生命周期未与 wave dispatch 实际状态对齐（plan.blocked 之后 task 应被 close 或标记 blocked） | **runtime (task lifecycle)** |

### §4.1.1 初始根因域分布（仅供 Agent D 参考，不做终判）

- **Runtime 单独**: DEV-002, DEV-003, DEV-005, DEV-006, DEV-007 (共 5 条)
- **Preset 单独**: 0 条
- **Compound (runtime + preset)**: DEV-001 (核心 fail-close 触发点)
- **Compound (drift 阈值)**: DEV-004 (单独看是 runtime, 但与 DEV-001 强相关 —— fail-close 后样本不足放大了 drift critical 噪音)
- **Agent 单独**: 0 条（无 agent 越权证据）

### §4.2 OPAC 逐 Hat 审计表（Observability / Policy precheck / Allowlist / Commit）

> 列定义: **O** = 运行时是否观察到该 hat activation（merged 计数）； **P** = hat 写盘前是否走 `--policy-check`； **A** = `hat.publishes` 是否在该 hat 允许清单内； **C** = 该 hat 实际 emit 是否真正落盘到 ledger 且不被 stage pipeline 拒。

| Hat | O (激活/merged) | P (policy-check) | A (hat.publishes vs 实际 emit topic) | C (落盘成功?) | 证据 | 置信度 |
|---|---|---|---|---|---|---|
| **inspector** | O = 1/1 merged (iter 1 @ 13:24:06) | P = N/A (不需 precheck, 初始 fan-out 入口) | A = ✅ `inspector.publishes=[forge.plan.inspected, forge.plan.blocked]`, emit `forge.plan.inspected` 一致 | C = ✅ accepted to events (L1) + ledger (iter 1) + flow-authority step=plan_authoring | `presets/en/parallel-forge.yml:567` + events.jsonl L1 + ledger.jsonl iter 1 | **高** |
| **planner** | O = 1/1 merged (iter 2 @ 13:29:46) | P = N/A | A = ✅ `planner.publishes=[forge.plan.ready, forge.plan.blocked]`, emit `forge.plan.ready` 一致 | C = ✅ accepted (L2 + iter 2 + flow step=concurrency_review) | `:596` + events.jsonl L2 | **高** |
| **guardian** | O = 1/1 merged (iter 3 @ 13:31:55) | P = N/A | A = ✅ `guardian.publishes=[forge.concurrency.approved, forge.plan.blocked]`, emit `forge.concurrency.approved` 一致 | C = ✅ accepted (L3 + iter 3 + flow step=worktree_setup) | `:713` + events.jsonl L3 | **高** |
| **worktree** | O = 1/1 merged (iter 4 @ 13:53:56) | P = ⚠️ 未观察到 agent 侧 `--policy-check` 调用（仅 worktree-blocked.md 自述"两次重试"） | A = ⚠️ 静态定义 `:773 worktree.publishes=[forge.wave.worktrees.ready, forge.worktrees.ready, forge.plan.blocked]` bare, 但 runtime `apply_precheck_desugar` 应改 `.proposed`, hat 实际直接试 bare → **DEV-001 触发点** | C = ❌ 三种字面量全部 `flow_unknown_emit`, 最终降级 emit `forge.plan.blocked` 被接受 (4th/L4) | `:773` + worktree-blocked.md §"尝试1/2/3" + recovery.jsonl 无 cli_emit warning（说明降级到 plan.blocked 走通了） | **中-高** (P 列置信度因缺 agent prompt 实证下降) |
| **cleanup** (1st, iter 5) | O = 1/1 merged (iter 5 @ 13:56:43) | P = N/A (allowed_emits 阶段已校验) | A = ✅ `:1640 cleanup.publishes=[forge.cleanup.done]`, emit `forge.cleanup.done` 一致 | C = ✅ accepted (L5 + iter 5 + flow step=cleanup, then plan_end) | `:1640` + events.jsonl L5 | **高** |
| **reporter** | O = 1/1 merged (iter 6 @ 13:58:58) | P = N/A | A = ✅ `:1705 reporter.publishes=[forge.report.done, LOOP_COMPLETE]`, emit 两者均一致 | C = ✅ accepted (L6+L7 + iter 6 + flow step=report → plan_end) | `:1705` + events.jsonl L6, L7 | **高** |
| **cleanup** (2nd, iter 17 orphan) | O = 1/1 merged (iter 17 @ 14:37:29) | P = N/A | A = ⚠️ hat.publishes 仍含 `forge.cleanup.done`, 但 stage pipeline flow_step 此时停在 `plan_end topic=forge.report.done`, 不允许新事件 → **DEV-002/006 触发点** | C = ❌ events.jsonl L8 写了 hat=cleanup ts=14:34:32, 但 stage `FlowStepScope` @ 14:37:29 拒（`flow_unknown_emit`） → ledger iter 17 标 "accepted" 是错误的（双 ledger 一致性问题，**DEV-003**） | recovery.jsonl L6 + events.jsonl L8 + ledger.jsonl iter 17 | **中** (C 列 ledger 状态可信度受 DEV-003 影响) |
| **未激活 hats**（executor, reviewer, integrator, verifier, tester, auditor, finalizer, forge-dispatcher, forge-failure-handler） | O = 0 (未在 runtime-trace.jsonl merged 列表) | N/A | N/A | N/A (loop 终结于 worktree_setup 之前) | runtime-trace.jsonl (merged 仅 6 个 hat) | **N/A** |

**OPAC 整体观察**:

- 6 个 activated hats 中，4 个（inspector / planner / guardian / reporter）OPAC 全绿；
- 1 个（cleanup 1st）OPAC 全绿；
- 1 个（worktree）OPAC 唯一异常：静态 `hat.publishes` 与 runtime desugar 行为不一致，**核心 fail-close 根因**；
- 1 个（cleanup 2nd orphan）OPAC 异常但属于 terminal_monotonicity_violation / flow_unknown_emit 派生，**非独立 root cause**。

### §4.3 R1-R6 机制逐项 Checklist

| 机制 | 启用状态 | 本次失败是否触发该机制 | 证据 | 评估 |
|---|---|---|---|---|
| **R1 ephemeral_isolation** | ✅ yml line 184 `ephemeral_isolation: true` | ❌ 未触发（fail-close 在 worktree_setup 阶段，wave dispatch 未启动） | `presets/en/parallel-forge.yml:184` | 不适用 |
| **R2 enforce_hat_scope** | ✅ yml line 186 `enforce_hat_scope: true` | ⚠️ **触发**：worktree hat emit `forge.worktrees.ready` 时，stage `FlowStepScope` 用严格相等（`flow_step_scope_stage.rs:246-248`）校验 `step.allowed_emits`, 但 `apply_precheck_desugar` 仅改 `hat.publishes`, 未改 step.allowed_emits → `flow_unknown_emit` | `presets/en/parallel-forge.yml:186` + `flow_step_scope_stage.rs:246-248` + worktree-blocked.md §Root cause | **核心失败路径** (R2 + R5 共同触发) |
| **R3 enforce_current_unit** | ✅ yml line 185 `enforce_current_unit: true` | ❌ 未触发（未进入 wave dispatch, unit 还未被选中） | `presets/en/parallel-forge.yml:185` | 不适用 |
| **R4 payload_consistency** | ✅ (preset 内置 payload_consistency rule 框架) | ❌ 未触发（未进入 forge.wave.* 阶段） | 无 payload_consistency violation 记录 | 不适用 |
| **R5 precheck_desugar** | ✅ `apply_precheck_desugar` (ralph_config.rs:130-208) | ⚠️ **触发但实现不完整**: desugar 改写 `hat.publishes` 到 `.proposed`, 但 **不**改写 `mechanism.flow.steps[*].allowed_emits`, 导致 R2 + R5 同步漏洞 | `crates/ralph-core/src/config/ralph_config.rs:130-208` + yml line 201-207 注释（明确指出 desugar + precheck-forge.worktrees.ready gate） | **核心失败路径** (与 R2 共同) |
| **R6 concurrent_workers / supervisor** | ✅ `execution_capability: supervisor` (diagnosis-input.json L11) | ⚠️ 部分触发: supervisor 模式启用 stall_recovery, iteration 22 把 orphan cleanup 2nd 派给 reporter, 但 reporter 已完成, 触发误派（**DEV-006**） | `diagnosis-input.json` `execution_capability=supervisor` + recovery.jsonl L7-8 | **次级触发** (误派, 非主因) |

**R1-R6 责任域汇总**:

| 机制 | 是否 root cause 直接贡献 | 责任域 | 置信度 |
|---|---|---|---|
| R1 ephemeral_isolation | 否 | — | — |
| R2 enforce_hat_scope | **是** (与 R5 同步漏洞) | runtime (`flow_step_scope_stage.rs`) + preset (yml line 72-78 bare form) | **高** |
| R3 enforce_current_unit | 否 | — | — |
| R4 payload_consistency | 否 | — | — |
| R5 precheck_desugar | **是** (改写范围不完整) | runtime (`ralph_config.rs:130-208`) | **高** |
| R6 supervisor | 否（仅次级 DEV-006 误派） | runtime (stall_recovery) | **中** |

### §4.4 Activation outcome 表（plan 2026-08-15-1823）

> `runtime-trace.jsonl` 中 `phase=activation` / `kind=hat_activation_outcome` 行集状态：**present**（61 rows；status 分布: 7 merged + 54 empty）

| 关键 sequence | hat | status | merge_succeeded | output_mentions_emit | accepted_event_count | classification | confidence | evidence_refs | notes |
|---|---|---|---|---|---|---|---|---|---|
| 7 | inspector | merged | true | true | 1 | successful_no_terminal_emit | 95 | events.jsonl#L2 (forge.plan.inspected) | 正常 fan-out |
| 13 | planner | merged | true | true | 1 | successful_no_terminal_emit | 95 | events.jsonl#L3 (forge.plan.ready) | unit_count=5 |
| 19 | guardian | merged | true | true | 1 | successful_no_terminal_emit | 95 | events.jsonl#L4 (forge.concurrency.approved) | 正常 |
| 25 | worktree | merged | true | true | 1 | attempted_but_rejected | 69 | events.jsonl#L5 (forge.plan.blocked) + worktree-blocked.md | **DEV-001 触发**; emit `forge.plan.blocked` 成功，但 worktrees.ready 三种字面量全 flow_unknown_emit |
| 31 | cleanup | merged | true | true | 1 | successful_no_terminal_emit | 95 | events.jsonl#L6 (forge.cleanup.done @ 13:56:22) | 首次 cleanup 正常 |
| 38 | reporter | merged | true | true | 1 | successful_no_terminal_emit | 95 | events.jsonl#L7+L8 (forge.report.done BLOCKED + LOOP_COMPLETE) | 正常落定阻塞态 |
| 42, 50, 58, 66, 74, 82, 90, 98, 106, 114, 134, 142, 150, 158, 166, 178, 188, 194, 202, 210, 218 | ralph / cleanup | empty | false | true | 0 | attempted_but_rejected | 60 | events.jsonl 末行未新增；flow-authority 末段 plan_end 重复 | stall-detector 重复 activation；agent 输出提及 emit 但 0 candidate |
| 124 | cleanup | merged | true | **false** | 1 | attempted_but_rejected | 65 | recovery.jsonl#L6 (flow_unknown_emit @ 14:37:29) + events.jsonl#L9 (cleanup @ 14:34:32) | **DEV-003 双 ledger 异常**; events 接受但 stage 拒 |
| 172 | reporter | empty | false | true | 0 | successful_no_terminal_emit | 50 | recovery.jsonl#L7 (stall_recovery 升级) | DEV-006 误派 |

**列含义**：

- `classification`: `successful_no_terminal_emit` / `attempted_but_rejected` / `backend_failure` / `channel_routing_failure` / `timeout_or_termination` / `unknown`
- `confidence`: 计分卡打分；`unknown` 一律 confidence<60

**禁止**：

- 凭 `status=empty` 单值写 agent 根因（已遵守：54 个 empty 全标 attempted_but_rejected，归因为 stall-detector）
- 凭 activation outcome row 跳过 L6 源码反查（已遵守：DEV-001 已附 file:line）

### §4.5 Causal Attribution（plan 2026-08-26-1104, U10）

`ralph diagnose --causal` 是归因事实唯一来源；agent 不另行打分。

#### §4.5.1 DT7 分项 + 总置信度

| DT7 项 | 分值 | 实测值（来自 `--causal`） | 来源 |
|--------|------|---------------------------|------|
| coverage | +30 | **30**（boundary_coverage 8/8 全 covered, 但 3 boundary `expected=0/recorded=0` 反映 fail-close 未走完链，证据完整性折半） | `diagnosis-input.json` `boundary_coverage[]` |
| integrity | +25 | **5**（accepted-transitions `00482bf6...32eba` 缺 commit_receipt，ledger join 不完整） | `runtime-trace.jsonl` 三类收据 + `.ralph/ledger.jsonl` |
| refutation | +20 | **20**（4 落选域各 1 条反驳：backend `success=true` / preset `terminal_topics_visible` / agent `no_terminal_event_emitted` / diag `coverage.gap=false`） | `CausalAttributionReport.rejected_hypotheses[]` |
| correlation | +15 | **14**（`contract_digest` + `sequence` 单调 + `retry_key` 与 `plan.blocked{kind=precheck_exhausted}` payload 不一致 — 本次 run 未走 precheck_exhausted, 所以 correlation 部分扣分） | `runtime-trace.jsonl` `phase=decision` `kind=contract_receipt` 行 |
| freeze_window | +10 | **0**（诊断窗口未冻结，无 deferred evidence） | `<session>/evidence-window.jsonl` (缺失) |
| **总置信度** | **max 100** | **69**（`--causal` JSON `.confidence.total`） | `ralph diagnose --causal` |

#### §4.5.2 被否决假设（rejected_hypotheses）

| 落选域 | 反驳证据类型 | 反驳证据引用 |
|--------|----------------|----------------|
| runtime | （primary_domain） | — |
| preset | `contract_receipt.fields.terminal_topics` all_terminal_topics_visible | contract_digest 字段一致 |
| agent | `session_summary.terminal.no_terminal_event_emitted` | 规则匹配后落选域 |
| backend | `runtime-trace.jsonl kind=hat_activation_outcome no_failure_row` | backend.outcome.success=true |
| diagnostic_capture_contract | `diagnosis-input.json.boundary_coverage all_boundaries_covered` | coverage.gap=false |

> 仅记录 `primary_domain` 之外 4 个落选域；域枚举固定为 `runtime / preset / agent / backend / diagnostic_capture_contract`。

#### §4.5.3 分数变化（causal_score_change）

| 重新打分原因 | 上次 total | 本次 total | Δ | primary_domain 是否变化 | 落选域反驳新增 |
|--------------|------------|------------|---|--------------------------|------------------|
| 初始打分 | N/A (initial scoring) | 69 | — | — | — |
| 加深 1 轮（重跑 `ralph diagnose --causal`，无新 evidence 补入；diff 除 `summary.generated_at` 外无差异） | 69 | 69 | 0 | 否（runtime → runtime） | 无新增 |

> 禁止在分数变化小节捏造上次分数；首次打分已写 `N/A (initial scoring)`。

---

## 5. 问题归因表（DT7 机检，confidence > 85）

| 优先级 | 问题 | primary_domain | status | confidence | 证据 DEV | DT7 分项来源 | rejected_hypotheses | 历史关联 | 加深轮次 |
|--------|------|----------------|--------|------------|----------|--------------|---------------------|----------|----------|
| (空) | 本 run `causal.status = incomplete`，`confidence.total = 69`，DT7 严格门禁 (`status == complete AND confidence > 85`) 阻止入表 | — | — | — | — | — | — | — | — |

> **DT7 严格门禁说明**：本次机检 `status == incomplete`，按 `confidence-rubric.md §入表门槛`，§5 必须空。下方 §4.5.3 / §7 / §6 仅作未核实疑点参考，不驱动修复。

---

## 6. 修复建议

**§5 必须空（`status == incomplete`）→ §6 不提供修复建议。**

以下三栏为「若 §5 入表时的候选动作」（每条均标 **关联置信度 = N/A (`status=incomplete`)**，**不构成修复建议**）：

### §6.1 短期（operator workaround）

| 候选动作（仅描述, 不构成建议） | 关联 DEV | 关联置信度 |
|-------------------------------|----------|------------|
| 在 `apply_precheck_desugar` 改写后，同步将 `mechanism.flow.steps[*].allowed_emits` 也追加 `.proposed` 形式（or 在 desugar 时 keep raw 形式），让 `FlowStepScopeStage::allows_topic` 严格相等通过 | DEV-001 | N/A (status=incomplete) |
| 重启该 loop 时设置 `RALPH_PRECHECK_MODE=off`（旁路 precheck desugar, hat.publishes 保留裸形式, CLI 写盘也是裸形式, flow_step_scope 接受） | DEV-001 | N/A (status=incomplete) |
| 在 events ledger 落盘前加 `terminal_monotonicity_violation` 短路：若 `LOOP_COMPLETE` 已 accepted, 后续 emit 写 `feedback.jsonl` 而非 events ledger | DEV-002 | N/A (status=incomplete) |

### §6.2 中期（preset / schema / instructions）

| 候选动作（仅描述, 不构成建议） | 关联 DEV | 关联置信度 |
|-------------------------------|----------|------------|
| 修正 `apply_precheck_desugar` 的改写范围：让 `hat.rewrite_emit_topics` 同时改写 `mechanism.flow.steps[*].allowed_emits`（或在 desugar 时保留原 raw 形式 + 兼容两种) | DEV-001 | N/A (status=incomplete) |
| 调整 `FlowStepScopeStage::allows_topic` 为前缀匹配（`topic.starts_with(t)` 或 `topic == t || topic == format!("{t}.proposed")`），避免未来 desugar 引入再发生同根因 | DEV-001 | N/A (status=incomplete) |
| 让 cleanup hat 在 `LOOP_COMPLETE` 后直接跳过 emit（不再走 stall_recovery handoff dispatch） | DEV-002 / DEV-006 | N/A (status=incomplete) |

### §6.3 长期（机制 / 底座）

| 候选动作（仅描述, 不构成建议） | 关联 DEV | 关联置信度 |
|-------------------------------|----------|------------|
| 引入 `freeze_window > 0` 的诊断补证据机制（replay events + apply new evidence），使 `coverage=30` / `correlation=14` 可提升至 85+ 入表 | DEV-001..007 整体 | N/A (status=incomplete) |
| stall_recovery 的 handoff dispatch table 在 `LOOP_COMPLETE` 后必须显式 purge（避免 orphan event 误派 reporter 等已 merge hat） | DEV-006 | N/A (status=incomplete) |
| drift_monitor 的 `field_completeness` 阈值应改为 `min_samples` 元约束（window=10 但只有 5 个 events 时不报警） | DEV-004 | N/A (status=incomplete) |
| task lifecycle 与 wave dispatch 状态同步：plan.blocked 之后 task 应自动 close 或标记 blocked（避免 5 task 全 open 虚占资源） | DEV-007 | N/A (status=incomplete) |

> **§6 结论**：DT7 严格门禁下未入表，**无可驱动修复建议**。§7 仅作未核实疑点参考，不驱动修复。

---

## 7. 未核实疑点

> 本表 7 条 DEV **仅为未核实疑点参考**（status=incomplete, confidence ≤ 85），不构成修复依据；按 `confidence-rubric.md §入表门槛` 全部移入 §7。

| 候选问题 | 当前置信度 | blocked_by | 已做加深 | primary_domain |
|----------|------------|------------|---------|----------------|
| **DEV-001** `apply_precheck_desugar` 改 `hat.publishes → .proposed` 但不改 `mechanism.flow.steps[id=worktree_setup].allowed_emits` → `FlowStepScopeStage::allows_topic` 严格相等 → worktree hat 三种字面量全 `flow_unknown_emit` | **69**（来自 `--causal`） | integrity.join 缺 commit_receipt（accepted-transitions `00482bf6...32eba` 无 receipt） | 1 轮（重跑 `--causal`） | **compound (runtime + preset)**（fix_point=runtime, YAML bare=preset） |
| **DEV-002** 14:34:32 cleanup 第二次 emit 在 LOOP_COMPLETE (13:58:50) 后被 `terminal_monotonicity_violation` 拒, events ledger L8 仍写入（与 cli_emit warning 时间差 35min） | 65 | evidence_gaps 空，但 accepted-transitions 仅 1 条无法交叉验证 | 0 | runtime |
| **DEV-003** iter 17 cleanup 第二次 `flow_unknown_emit` 但 events ledger iter 17 标 accepted → 双 ledger 不一致 | 65 | 同 DEV-002 | 0 | runtime (ledger ↔ stage pipeline) |
| **DEV-004** drift 4 critical findings（`plan_path` / `context_artifact_path` / `forge_artifact_root` / `plan_key` 各 1/5 events = 20%）是低样本假阳性（window=10 / events=5） | 60 | freeze_window=0 不允许重新采样 | 0 | runtime (drift monitor 阈值/窗口失配) |
| **DEV-005** boundary coverage 8/8 但 3 boundary（`state_commit` / `recovery_action` / `termination`）`expected=0/recorded=0`（fail-close 未走完链） | 62 | coverage=30 但折半（3 boundary 0/0） | 0 | runtime (boundary 覆盖语义) |
| **DEV-006** stall_recovery iter 22 升级 reporter，但 reporter 实际 13:58:41 已 merge 完毕, handoff dispatch table 未清理 → 误派 | 63 | correlation=14 不足以 cross-verify | 0 | runtime (stall_recovery 误派) |
| **DEV-007** 5 个 task 全 open `owner=executor`，但 executor iter 17 后未激活 → task lifecycle 与 wave dispatch 失同步 | 63 | agent domain 在 refutation 中被排除（`no_terminal_event_emitted`），无法直接归因 | 0 | runtime (task lifecycle) |

> **§7 限定说明**：本表 7 条 DEV **仅为未核实疑点参考**，不构成修复依据。

### §7.x 历史关联备注（旁注，不驱动修复）

- MEMORY `precheck-desugar-allowed-emits-mismatch` 直接对应 DEV-001 同根因复发
- MEMORY `parallel-forge-cleanup-after-loop-complete` 对应 DEV-002/003（cleanup 在 LOOP_COMPLETE 后被拒）
- MEMORY `flow-authority-stale-tail-pollutes-recovery` 对应 flow-authority.jsonl 尾部 `step=plan_end, topic=forge.plan.blocked` 全是 stale entry
- 30d 窗口内 3 份诊断报告（8/26、8/29、7/29），8/26 + 8/29 与本次关联度高；**本次 = 旧根因复发（非新问题模式）**

---

## 附录 A: 关键文件路径（仓库相对）

- **Preset YAML**: `presets/en/parallel-forge.yml`（worktree_setup.allowed_emits `:72-78`, worktree.publishes `:773`）
- **Preset schema**: `presets/schemas/parallel-forge.yml`
- **Runtime source**:
  - `crates/ralph-core/src/config/ralph_config.rs:130-208`（`apply_precheck_desugar`）
  - `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs:246-248`（`allows_topic` 严格相等）
- **本 run 工作树配置**: `ralph.forge.yml`（drift.field_completeness_threshold=0.85）
- **Plan 文件**: `docs/plans/2026-09-01-2102-feat-trusted-worktree-continuation-plan.md`
- **本 run 产物**:
  - `.ralph/events-20260901-132042.jsonl`（9 行，current-events 唯一可信源）
  - `.ralph/flow-authority.jsonl`（末段 plan_end stale entries）
  - `.ralph/ledger.jsonl`（15 行）
  - `.ralph/diagnostics/2026-09-01T21-20-42/diagnosis-input.json`（bundle 入口）
  - `.ralph/diagnostics/2026-09-01T21-20-42/runtime-trace.jsonl`（174576 B, 61 activation_outcome rows）
  - `.ralph/diagnostics/2026-09-01T21-20-42/recovery.jsonl`（8 entries）
  - `.ralph/diagnostics/2026-09-01T21-20-42/feedback.jsonl`（14 rows, 7 feedback_id × 2 phases）
  - `.ralph/diagnostics/2026-09-01T21-20-42/drift.jsonl`（4 critical field_completeness findings）
  - `.ralph/diagnostics/2026-09-01T21-20-42/diagnosis-summary.json`
  - `.ralph/diagnostics/2026-09-01T21-20-42/active-activations.json`（空）
  - `.ralph/diagnostics/2026-09-01T21-20-42/channel-routing-fallback-*.md`（15 个 hat-channel 文件）
  - `.ralph/forge/2026-09-01-2102-feat-trusted-worktree-continuation-plan/blocks/worktree-blocked.md`（核心 fail-close 自述）
  - `.ralph/forge/2026-09-01-2102-feat-trusted-worktree-continuation-plan/worktree-map.yml`（U01 + U05, wave 1）
  - `.ralph/forge/2026-09-01-2102-feat-trusted-worktree-continuation-plan/cleanup.md`（cleanup activation 28-52 累计报告）
  - `.ralph/forge/2026-09-01-2102-feat-trusted-worktree-continuation-plan/{inspection-report.md, development-plan.md, execution-plan.yml, concurrency-approval.md}`
  - `docs/reports/2026-09-01-2026-09-01-2102-feat-trusted-worktree-continuation-plan-manager-report.md`（reporter 写 BLOCKED final report）

## 附录 B: 本诊断过程清理记录

- **DIAG_WORKDIR**: `/tmp/ralph-diagnosis.Ww28Ci/`（bundle-first structured result + causal + causal_deepen + diagnose stderr）
- 报告 frontmatter `structured_result_ref: "inline: summarized in report"`（JSON 仅存在 DIAG_WORKDIR，清理后不在 target branch 留副本）
- 清理时间: 报告落盘后立即执行；残留路径将于最终回复中明示