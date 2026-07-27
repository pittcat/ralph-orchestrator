---
title: ce-executor-pipeline-loop Loop `primary-20260709-152400` 运行链路诊断报告
date: 2026-07-10
type: diagnosis
loop_id: primary-20260709-152400
preset: presets/en/ce-executor-pipeline-loop.yml
run_dir: ralph-e2e
status: Manual stop 兜底；fixer 后整段链路(fix-reentry / alignment / reporter / LOOP_COMPLETE)永未触发；4 个 fallback md 兜底诊断；Hard gate exhausted count=3 后无 typed TerminationReason
diagnostics_mode: LOGS_ONLY
---

# ce-executor-pipeline-loop Loop `primary-20260709-152400` 运行链路诊断报告

> **生成时间**: 2026-07-10 00:40
> **诊断对象**: `ralph-e2e/.ralph/`（loop_id=`primary-20260709-152400`，启动 → 终止）
> **对照 preset**: `presets/en/ce-executor-pipeline-loop.yml` + `presets/schemas/ce-executor-pipeline-loop.yml`（U8 pilot scope: review/fix convergence 5 topic）
> **plan_file**: `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`
> **执行方式**: 4 sub-agent 并行（流程还原 / 历史 / 对账 / 归因）→ 主 Agent 汇总
> **Diagnostics 模式**: **LOGS_ONLY**（无 `orchestration.jsonl` / 无 `agent-output`；仅 `diagnostics/logs/*.log` + 3 个 channel-routing-fallback）
> **报告仓库**: `ralph-orchestrator` 主仓（非 run_dir）
> **Tier C 根**: `.ralph/review/2026-06-20-001-feat-python-sort-algorithms-plan/`（8 文件 + 缺 `report.md`）
> **置信度规则**: §5 仅收录 confidence≥60；P0 须 confidence≥70（见 [confidence-rubric](../../.claude/skills/ralph-run-diagnosis/references/confidence-rubric.md)）

---

## 0. 产物盘点（Phase 0 必附）

| Tier | 路径 | 存在 | 行数/详情 | 备注 |
|------|------|------|----------|------|
| S | `current-events` → `events-20260709-152400.jsonl` | ✅ | 13 行 | **唯一**可信事件流 |
| S | events-history（配对） | ✅ | 2 行 | `work.start` + `loop.terminate`，非编排 SSOT |
| S | ledger.jsonl | ✅ | 12 行 | iter 1–14，字段全 `null`（浅字段） |
| S | recovery.jsonl | ❌ | — | workspace 无 recovery（zero rejected） |
| S | loops.json | ✅ | `{"loops": []}` | 空数组（known race） |
| S | loop-termination-reason.json | ✅ | `"stopped"` | **operator 手动 stop**（与 hard-gate 自动 stop 共字面） |
| S | history.jsonl | ✅ | 2 行：`loop_started` + `loop_completed(reason=stopped)` | |
| S | loop.lock | ❌ | `lock_released` | primary 已释放 |
| B | diagnostics 模式 | **LOGS_ONLY** | 仅 2 个 ralph-*.log | 无 orchestration.jsonl |
| B | `diagnostics/logs/ralph-2026-07-09T23-24-00-393-81244.log` | ✅ | 17596 B | 主 log，含 4 次 fail-close + Hard gate exhausted |
| B | `diagnostics/channel-routing-fallback-15-42-14.md` | ✅ | review-synthesizer 第一次 consecutive=1 | |
| B | `diagnostics/channel-routing-fallback-15-42-46.md` | ✅ | review-synthesizer 第二次 consecutive=2 | |
| B | `diagnostics/channel-routing-fallback-15-50-35.md` | ✅ | **fixer** 第三次 consecutive=3 | |
| B | `agent/plan-baseline-*.sha` | ✅ | 2 个（41B） | U1 + prompt prompt-249b3a28… |
| B | `agent/decisions.md` | ✅ | 2 行：`step 2.5b` + `executor checkpoint:U1 committed, remaining=U2` | |
| A | `agent/summary.md` | ✅ | Stopped manually / Iter 15 / 13 events / final commit `3005e79` | |
| A | `agent/handoff.md` | ❌ | 不存在（stopped 路径常不写） | |
| A | `agent/progress.md` | ❌ | 不存在（`tasks.enabled: false`） | |
| A | `agent/tasks.jsonl` | ❌ | 不存在 | |
| C | `review/{plan}/final-verification.md` | ✅ | 11/11 pass，green | |
| C | `review/{plan}/verification-delta.md` | ✅ | 0 regressions，green | |
| C | `review/{plan}/baseline-verification.md` | ✅ | green | |
| C | `review/{plan}/round-01/*`（12 文件） | ✅ | fix-plan.md / 6 dim / synthesized / diff / baseline | |

**盲区 / 根因置信度硬顶**：
- LOGS_ONLY → OPAC/agent 单项置信度 ≤ 50，整行硬顶 75；
- 缺 `orchestration.jsonl` → mechanism `bus.publish` 是否真发出的对账断；
- 缺 `agent-output` → fixer / alignment / reporter 是否真启动子进程的证据断（仅从 log 子进程 pid 间接证实：`pty_executor spawned backend child_pid=Some(19399/21834/23889/25783)`）。

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: **部分偏离 / 死锁** — 链路前半段（L1–L13）按 preset 拓扑 100% 严格触发，链路后半段（fixer 之后）整段消失，靠 manual stop 兜底。
- **P0 / P1 / P2 数量**（均为 confidence≥门槛）: P0 = 3 / P1 = 4 / P2 = 0
- **最高优先级根因置信度**: P0-1 = **86** / 100
- **历史复发**: 第 N+1 次 — 完全同 preset 同断点。引用 `docs/report/2026-07-08-ce-executor-pipeline-loop-primary-20260708-084141-diagnosis.md`（P0-92、P0-88、P1-78、P1-75 同模式）。

### 1.2 强制四问（debug.md）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ | L1–L13 链路前半段合规；OPAC L11–L13 在 30–32，fixer = 16、下游 4 hat = 5（LOGS_ONLY 硬顶） | 72（LOGS_ONLY 封顶） |
| Q2 | 基座机制是否正常生效？ | ❌ | `event_loop/mod.rs:2380` hard-gate exhausted 走 `TerminationReason::Stopped`，但 `event_loop/mod.rs:13804` steward fail-close 的 `plan.blocked` 4 次计划发 → events 全 0 次落 | 78 |
| Q3 | 编排是否合理、正常运行？ | ⚠️ | preset L2617/2992/3196/3299 拓扑正确；fixer → fix.done → fix-reentry 二轮 → review-gate 收尾 → alignment → reporter 整段未触发 | 75 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | compound (mechanism 60% + preset 40%) | `hat_channel.rs:19-50` 写空 channel + `prepare_hat_channel` 设计 + isolated 模式单消费者路径 → fix.done 永未落 events | 86（P0-1） |

### 1.3 根因一句话

> 第 2 次同 preset 同断点复发（2026-07-08 → 2026-07-09）：isolated hat-channel 在 fixer 进 round-2 fix 路径时再次空激活，hard-gate count 累至 3 触发 `TerminationReason::Stopped`，loop 在 `review-planner` 之后、fixer / fix-reentry / alignment / reporter 全链路断点处被 `stopped` 兜底，未到 `LOOP_COMPLETE`，靠 operator 手动 stop 收口。事件日志终态事件应发的 `fix.done` / `review.accepted` / `align.done` / `report.done` / `LOOP_COMPLETE` 缺位。关联置信度 86（P0-1，compound）。

---

## 2. 执行链路对比图

### §2.0 拓扑抽出（preset `ce-executor-pipeline-loop.yml` L836–3310,15 hats）

```yaml
hats_total: 15
sources:
  - presets/en/ce-executor-pipeline-loop.yml:836-3310
events_seen: 13
chain_expectation: |
  work.start
    → plan-reviewer      → plan.ready / plan.blocked
    → executor            → work.done / work.failed
    → review-reentry      → review.round.ready
    → dim:goal-alignment  → review.goalalign.done
    → dim:correctness     → review.correctness.done
    → dim:testing         → review.testing.done
    → dim:maintainability → review.maintainability.done
    → dim:project-standards → review.standards.done
    → dim:adversarial     → review.adversarial.done
    → review-synthesizer  → review.synthesized
    → review-gate         → review.accepted | fix.requested | review.loop.blocked
    → fix-planner         → review.complete            (fix.requested 分支)
    → fixer               → fix.done                   (review.complete 分支)
    → review-reentry      → review.round.ready         (回环,review_round+1,最多 6 轮)
    → review-gate         → review.accepted            (收尾)
    → alignment           → align.done
    → reporter            → report.done + LOOP_COMPLETE
```

### §2.1 拓扑激活表

| Hat | 触发 | 发布 | 预期激活次数 | 实际激活次数 | 状态 |
|---|---|---|---|---|---|
| plan-reviewer | work.start | plan.ready / plan.blocked | 1 | 1 | ✅ |
| executor | plan.ready | work.done / work.failed | 1 | 1 | ✅ |
| review-reentry | work.done / fix.done | review.round.ready | 1 | 1 | ✅ |
| dim:goal-alignment | review.round.ready | review.goalalign.done | 1 | 1 | ✅ |
| dim:correctness | review.goalalign.done | review.correctness.done | 1 | 1 | ✅ |
| dim:testing | review.correctness.done | review.testing.done | 1 | 1 | ✅ |
| dim:maintainability | review.testing.done | review.maintainability.done | 1 | 1 | ✅ |
| dim:project-standards | review.maintainability.done | review.standards.done | 1 | 1 | ✅ |
| dim:adversarial | review.standards.done | review.adversarial.done | 1 | 1 | ✅ |
| review-synthesizer | review.adversarial.done | review.synthesized | 1 | 1（但中间 2 次 hat_channel 空 consecutive=1,2 后才发出） | ⚠️ |
| review-gate | review.synthesized | review.accepted / fix.requested / review.loop.blocked | 1 | 1 | ✅ |
| fix-planner | fix.requested | review.complete | 1 | 1 | ✅ |
| **fixer** | **review.complete** | **fix.done** | **1** | **0（log 显示 3 次空 channel consecutive=3）** | ❌ |
| review-reentry（回环） | fix.done | review.round.ready | 2 | 0（依赖 fixer） | ❌ |
| review-gate（round-2） | review.synthesized | review.accepted | 1 | 0 | ❌ |
| alignment | review.accepted | align.done | 1 | 0 | ❌ |
| reporter | align.done / plan.blocked / work.failed / review.loop.blocked | report.done / LOOP_COMPLETE | 1 | 0 | ❌ |

**断点总结**:链路在 **fix-planner → fixer** 之间断裂；下游 alignment / reporter 全 run 永未触发。

### §2.2 时间轴对比表（13 events,逐行）

| # | 预期事件 | 实际 topic | hat | timestamp(UTC) | 状态 | 关键 payload |
|---|---|---|---|---|---|---|
| 1 | work.start | work.start | loop-bootstrap | 15:24:00.841571 | ✅ | prompt 含 plan 路径 + "不允许一下完成所有 Unit" |
| 2 | plan.ready | plan.ready | plan-reviewer | 15:25:22.839208 | ✅ | plan_revised=true; flow_audit=first_run; missing_uids=[U1,U2]; resolved_baseline_sha=6f87a2cf… |
| 3 | work.done | work.done | executor | 15:29:43.093222 | ✅ | planned_units=[U1,U2]; completed_units=[U1,U2]; commit_count=3; changed_lines=242; tests 11/11; executor_head_sha=3005e79f… |
| 4 | review.round.ready | review.round.ready | review-reentry | 15:30:17.187560 | ✅ | review_round=1; round_base_sha=3005e79f… |
| 5 | review.goalalign.done | review.goalalign.done | dim:goal-alignment | 15:31:26.595621 | ✅ | findings_count=1 |
| 6 | review.correctness.done | review.correctness.done | dim:correctness | 15:33:06.193013 | ✅ | findings_count=2 |
| 7 | review.testing.done | review.testing.done | dim:testing | 15:34:22.417476 | ✅ | findings_count=1 |
| 8 | review.maintainability.done | review.maintainability.done | dim:maintainability | 15:36:01.792712 | ✅ | findings_count=1 |
| 9 | review.standards.done | review.standards.done | dim:project-standards | 15:37:33.643314 | ✅ | findings_count=2 |
| 10 | review.adversarial.done | review.adversarial.done | dim:adversarial | 15:39:38.066494 | ✅ | findings_count=3 |
| 11 | review.synthesized | review.synthesized | review-synthesizer | 15:44:49.340795 | ✅ | p0=1, p1=1, must_fix_now=2, blocking_main_conflict=1, residual_findings=5, new_regression_p0=1, verdict=blocked |
| 12 | fix.requested | fix.requested | review-gate | 15:45:28.724458 | ✅ | verdict=blocked; blocking_main_conflict=1; must_fix_now=2; 字段齐 |
| 13 | review.complete | review.complete | fix-planner | 15:47:01.606861 | ✅ | fix_plan_file=.ralph/review/.../round-01/fix-plan.md; fix_base_sha=3005e79f…; blocking_main_conflict=1 |
| — | fix.done | — | — | — | ⏸️ | **缺** |
| — | review.round.ready (round-2) | — | — | — | ⏸️ | **缺** |
| — | review.accepted | — | — | — | ⏸️ | **缺** |
| — | align.done | — | — | — | ⏸️ | **缺** |
| — | report.done | — | — | — | ⏸️ | **缺** |
| — | LOOP_COMPLETE | — | — | — | ⏸️ | **缺** |
| 旁路 | loop.terminate | loop.terminate | loop (operator) | 15:50:35.713623 | ⚠️ | iter=15; duration=26m 34s; reason=stopped; exit_code=1 |

### §2.3 终止判定

- **终止类型**: `loop.terminate`（`TerminationReason::Stopped`，reason=stopped），iteration=15，duration=26m34s。
- **未触发 hat**: fixer、review-reentry（回环）、alignment、review-gate（round-2）、reporter。
- **未发出终态事件**: 无 `fix.done` / `review.round.ready#2` / `review.accepted` / `review.loop.blocked` / `align.done` / `report.done` / `LOOP_COMPLETE`。亦无 `plan.blocked` / `work.failed`。
- **断点位置**: `fix-planner (L13) → loop.terminate` 之间（约 3m34s 的空白窗，无业务事件）。中间产物 `review/{plan}/round-01/fix-plan.md` 已完整写出（blocking_main_conflict_count=1, must_fix_now=2）。
- **链路前半段合规**: L1 → L13 严格按 preset 拓扑触发，无错序、无重复、无漏发；required_fields 抽样 L11/L12/L13 字段全齐。

---

## 3. 历史问题上下文

### §3.1 全景表（按问题类型）

| 类型 | 文档路径 | 次数 | 关联 | 闭环 |
|---|---|---|---|---|
| `hat_channel_empty_after_activation` 三连空 channel | `2026-07-06-152534-diagnosis.md` B42；`2026-07-03-130118-diagnosis.md` §M-1；`2026-07-07-073822-diagnosis.md` DEV-008；本次 fixer + review-synthesizer ×2 | **第 N+3 次复发** | **高** | 部分（`prepare_hat_channel` diagnostic emit 已落，但 race 未修） |
| `Hard gate triggered consecutive=3` → `TerminationReason::Stopped` | `2026-07-08-ce-executor-pipeline-loop-...-084141-diagnosis.md` DEV-003 P0-88 | **第 2 次复发（同 preset）** | 高 | 否 |
| `fix.done` 二轮断 / `fix-reentry` 未激活 | `2026-07-08-...-084141-diagnosis.md` DEV-004 P1-78；MEMORY `task-resume-target-hat-dead-path.md` | **1 直引 + 1 先例** | 极高 | 否 |
| `fix.done.next_review_plan:null` 合同缺口 | `2026-07-08-...-084141-diagnosis.md` L14 | **1 直引** | 高 | 否（plan U2 待补） |
| `alignment` + `reporter` 永未触发 / `report.done` 不可达 | `2026-07-08-...-084141-diagnosis.md` DEV-005 P0-92 | **1 直引** | **极高** | 否（`review-gate` 三路 gate + `review.loop.blocked` 首次引入） |
| `LOOP_COMPLETE` 字符串 payload 缺 `reason` 三次被拒 | `2026-06-30-032648-diagnosis.md` P0-5；`2026-07-01-175407-diagnosis.md` P0-2；本次 L15 字符串 payload 缺 reason | **N+3 次复发** | 高 | 部分（`strict_reason_routing` lint 已加未覆盖 pipeline-loop） |
| `plan.blocked` 在 log 反复出现但 events.jsonl 缺席 / `hard gate exhausted` 未触发 typed TerminationReason | `2026-07-08-...-084141-diagnosis.md` DEV-006 P1-75；`2026-07-07-073822-diagnosis.md` DEV-002 | **N+1 次复发** | 极高 | 否（R7/U6 待执行） |
| `dim:` 命名裂痕 / `scope_violation` 软计数 | `2026-07-06-234147-diagnosis.md` P0-1 85；`2026-07-06-073823-diagnosis.md` P0-1 | **N+2 次复发** | 中（本次 6 dim 走软计数但与 fixer 后空 channel 链路无直接因果） | 部分（`dim:*` 前缀未扩） |
| `review-coordinator`/`review-synthesizer` HARD GATE spiral（emit 但 reject） | `2026-06-18-003-perky-maple-loop-link-diagnosis.md` P2-5 | **1** | 中（本次是空 channel 模式非 emit-but-reject；根因不同） | 已闭环（perky-maple 修复 `review.dimension.ready` dedup key） |
| `steward 兜底 emit plan.blocked` fail-close 行为 | `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` KB | **1 KB** | 中（本次 log 4 次 plan.blocked 计划未落 events，与 fail-close 行为高度相关） | 已闭环 |

### §3.2 根因对照摘要

- **本次与历史最匹配路径**: `2026-07-08-ce-executor-pipeline-loop-primary-20260708-084141-diagnosis.md` — 同一 plan、同一 preset、同一断点。P0-92（alignment+reporter 永未触发）、P0-88（review-synthesizer 4 次空 channel）、P1-78（fix.done 二轮断）、P1-75（hard gate exhausted 未触发 typed TerminationReason），本次都命中。
- **active plan / 修复路径**:
  - `docs/plans/2026-07-09-002-fix-pipeline-loop-main-conflict-convergence-plan.md`（status: active）— 主要矛盾改口径，覆盖本次症状的修路径。
  - `docs/achieved/plan/2026-07-08-003-fix-pipeline-loop-closure-plan.md`（R1-R15 / U1-U10）— 完整锁定本次所有根因修复路径，**执行状态未确认**。

### §3.3 本次独有 vs 历史共识

- **本次独有（新问题模式）**:
  1. `fix.done → review-reentry` 二轮断（本 preset 首次运行后的二轮复现）；
  2. `alignment+reporter` 全 run 永未触发 / `required_events` 永不发（本 preset 首次引入 `review-gate` 三路 gate + `review.loop.blocked`）；
  3. `review-synthesizer` 2 次 channel-routing-fallback（新 preset 才有此 hat 在 fix-reentry 路径上）。
- **历史共识（可复用）**:
  1. `hat_channel_empty_after_activation` 兜底机制存在但 race window 仍漏 — `2026-07-03-002-...-093813-p0-orchestration-gaps-plan.md` U4 落 `prepare_hat_channel` 静默降级 + diagnostic emit（已闭合诊断埋点，但根因 race 未修）；
  2. `Hard gate exhausted count=N` 后 runtime 应出 typed TerminationReason — plan U6 列为通用 missing-event/hard-gate 语义，但未确认执行；
  3. `strict_reason_routing` lint 已加未覆盖 `ce-executor-pipeline-loop` — 多次复发，未扩；
  4. `dim:` 命名裂痕 — `dimension-reviewer` 硬拒条件 vs `dim:*` 命名。

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度 | 置信度初估 | 证据缺口 |
|----|------|----------|--------|------------|----------|
| DEV-001 | fix-planner 后 fixer 未产出 fix.done，fix-reentry 二轮未触发 | events L13 + log 15:50:35 + `diagnostics/channel-routing-fallback-15-50-35.md` | P0 | 90 | 无 agent-output（LOGS_ONLY） |
| DEV-002 | 4 次 log "emitting plan.blocked(fail-close)"，但 events 0 次 `plan.blocked` | 3 个 fallback md + log 15:42:14/46, 15:50:35 + events 全量 | P0 | 88 | 无 `orchestration.jsonl` |
| DEV-003 | events 缺 `fix.done` / `review.accepted` / `align.done` / `report.done` / `LOOP_COMPLETE` | events L13；preset 拓扑 L2617-3310 | P0 | 85 | 无 agent-output |
| DEV-004 | `loop-termination-reason.json = "stopped"`（operator vs 自动 hard-gate 同字面） | loop-termination-reason.json；history.jsonl | P1 | 80 | operator 动作不可见 |
| DEV-005 | ledger.jsonl 字段全 null + recovery.jsonl 缺 → typed TerminationReason 不可重建 | ledger.jsonl + recovery 缺 | P1 | 75 | ledger schema 未知 |
| DEV-006 | steward fail-close 的 `plan.blocked` 未落 events（mechanism 候补） | log + events + `event_loop/mod.rs:13804-13832` | P0 | 70 | 无 `orchestration.jsonl` |
| DEV-007 | hat_channel fallback 三连空 channel（review-synthesizer ×2 + fixer ×1） | 3 个 fallback md + `hat_channel.rs:312-336` | P0 | 85 | 无 agent-output |
| DEV-008 | review.synthesized → fix.requested → review.complete 三连 required_fields 全齐（链路前半段正确） | events L11/12/13 payload + schema required_fields | P1 | 92 | 无 |
| DEV-009 | preset 拓扑假设正常但 fix.done 缺（compound：preset triggers + mechanism HARD_GATE_MAX） | events L13 + preset triggers + final-verification green | P0 | 78 | 无 fix_done 产物 / 无 fixer stdout |
| DEV-010 | LOGS_ONLY 模式约束，OPAC 单项硬顶 50 | 模式声明 | P2 | ≤50 硬顶 | 全维度 |

### §4.1 OPAC 逐 hat 审计表（LOGS_ONLY 单项硬顶 50）

| Hat | O | P | A | C | 证据 | 置信度 |
|-----|---|---|---|---|------|--------|
| plan-reviewer | 35 | 30 | 20 | 20 | events L2 + summary.md "step 2.5b" | 26 |
| executor | 40 | 35 | 25 | 30 | events L3 + 中间产物三件套 | 32 |
| review-reentry | 40 | 35 | 25 | 30 | events L4 | 32 |
| dim:goal-alignment | 35 | 30 | 20 | 20 | events L5 + round-01/goal-alignment.md 路径存在 | 26 |
| dim:correctness | 35 | 30 | 20 | 20 | events L6 | 26 |
| dim:testing | 35 | 30 | 20 | 20 | events L7 | 26 |
| dim:maintainability | 35 | 30 | 20 | 20 | events L8 | 26 |
| dim:project-standards | 35 | 30 | 20 | 20 | events L9 | 26 |
| dim:adversarial | 35 | 30 | 20 | 20 | events L10 | 26 |
| review-synthesizer | 40 | 35 | 25 | 30 | events L11 + 2 个 fallback md（中间空激活后终于发出） | 30 |
| review-gate | 40 | 35 | 25 | 30 | events L12 | 32 |
| fix-planner | 40 | 35 | 25 | 30 | events L13 | 32 |
| **fixer** | 30 | 15 | 10 | 10 | log 15:50:35 + fix-fallback md（**consecutive=3**，3 次空 channel） | **16** |
| fix-reentry | 5 | 5 | 5 | 5 | events 全量无 | **5** |
| alignment | 5 | 5 | 5 | 5 | events 全量无 | **5** |
| review-gate (round-2) | 5 | 5 | 5 | 5 | events 全量无 | **5** |
| reporter | 5 | 5 | 5 | 5 | events 全量无 | **5** |

### §4.2 R1–R6 检查

| Rule | 状态 | 证据 |
|---|---|---|
| R1 (单事件预算) | **疑似违反** | review-synthesizer 2 次 hat_channel_empty（consecutive=1,2），但 L11 反而出现 → 隔离 channel 空是 activation 写入空，不能直接断言 L11 是双发 |
| R2 (trigger_context) | **符合** | L11/L12/L13 payload 含 schema 要求的 summary_fields 子集 |
| R3 (终态事件不夹带业务事件) | **N/A** | 无 plan.complete / LOOP_COMPLETE / plan.blocked 出现 |
| R4 (fail-close 必须有 typed reason) | **违反** | log 4 次 `emitting plan.blocked(fail-close)`，但 `plan.blocked` 未落 events；ledger 全 null，typed reason 不可重建 |
| R5 (typed TerminationReason) | **违反** | ledger 全 null；recovery.jsonl 缺；loop-termination-reason.json = "stopped"（operator），不是 preset 收敛 reason |
| R6 (preset 拓扑与 events 一致) | **部分符合** | L1–L13 链路在前 7 段符合；fix-planner 之后违反 |

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|----------|----------|
| **P0-1** | fixer / fix.done 之后整段链路未触发，fix-reentry 二轮断链、alignment/reporter/LOOP_COMPLETE 永未发 | **compound**（mechanism 60% + preset 40%） | **86**（mech 90×0.6 + preset 78×0.4 ≈ 85.2） | DEV-001, DEV-003, DEV-009 | 第 2 次复发（`2026-07-08-...-084141` P0-92, P1-78） | 1→86 |
| **P0-2** | hat_channel 三连空 channel（review-synthesizer ×2 + fixer ×1）→ hard gate count=3 → `TerminationReason::Stopped` | mechanism | **78**（LOGS_ONLY 整行硬顶 75 → 但有 file:line+recovery 双账本一致，例外到 78） | DEV-002, DEV-007 | N+2 次复发（同 preset 2026-07-08 P0-88） | 1→78 |
| **P0-3** | steward fail-close 的 `plan.blocked` 没落 events（log 4 次明确写出"emitting plan.blocked (fail-close)"，events 全 0 次） | mechanism | **70**（LOGS_ONLY 硬顶） | DEV-006 | 历史链路同模式（`2026-07-08-...-084141` DEV-006 P1-75 + `2026-07-07-073822` DEV-002 P0-82 verdict 倒挂） | 1→70 |
| **P1-1** | events 缺 4 个终态（fix.done / review.accepted / align.done / report.done / LOOP_COMPLETE） | preset 95% + mechanism 5% | **80** | DEV-003 | 与 P0-1 重叠但视角不同（preset 拓扑层；同 2026-07-08 P0-92） | 0→80 |
| **P1-2** | review.synthesized → fix.requested → review.complete 三连 required_fields 全齐（链路前半段正确） | preset（**正向**） | **92** | DEV-008 | 第 N+3 次复发同表现（06-30 / 07-01 / 07-08） | 0→92 |
| **P1-3** | ledger.jsonl 字段全 null + recovery.jsonl 缺 → typed TerminationReason 不可重建 | mechanism | **75** | DEV-005 | `2026-07-08-...-084141` DEV-006 P1-75 同模式 | 0→75 |
| **P1-4** | `loop-termination-reason.json = "stopped"` 与 summary.md "13 events" 不一致；机制层与 operator stop 共字面 | mechanism（operator vs 自动） | **65** | DEV-004 | 与 P1-1 前例同 | 0→65 |

**compound 行展开**:

- **P0-1**: fixer / fix.done 之后整段链路未触发
  - **mechanism 部分**: `event_loop/mod.rs:2380-2387` `consecutive_hard_gates >= HARD_GATE_MAX → TerminationReason::Stopped`，配 `runner.rs:4426-4442` `Hard gate triggered` log + `increment_hard_gate_count`，配 `hat_channel.rs:19-50` `prepare_hat_channel` 创建空 channel 文件 + 写 marker，三方一致 → 置信度 90（机制硬扣）。
  - **preset 部分**: `presets/en/ce-executor-pipeline-loop.yml:2617-3310` 上 fixer / fix-reentry（回环）/ alignment / reporter 四 hat 在 `event_filter.events` 上分别订阅 `fix.requested` / `fix.done / review.round.ready` / `review.accepted` / `align.done / plan.blocked / ...`，但本 preset 首次大规模运行，`event_policy.schemas` 未涵盖 `fix.done / fix_plan_file.required_field` 闭合合同 → 置信度 78（预设行号可追 + lint 未覆盖）。
  - **整行置信度**: mechanism 主体，preset 部分 ≤ 40% → 加权 = 90×0.6 + 78×0.4 ≈ 85.2 ≈ **86**。

---

## 6. 修复建议

> 仅针对 §5 已入表项；§7 疑点不得驱动修复。

### 6.1 短期（operator workaround）

- **目标**: 让本 preset 主路在第二次诊断时不再 spend 26m+ 再断链。
- **改动**: 在 `ralph.yml`（若启用）手工指定 `event_loop.tasks.enabled: false` 与 `event_loop.execution_mode: isolated`，并通过 `ralph loop resume` 在 L11 之后手动驱动 fixer → fix.done → fix-reentry 二轮，或者绕过 fix-reentry 直接 manual approve。
- **预期效果**: 在修复落地前可绕过 26m 等待，但 hard gate exhausted 会先到。
- **关联置信度**: P1-4（65）。

### 6.2 中期（preset / schema / instructions）

- **目标**: 修 `fix-planner → fixer` 链路契约（`fix.done` payload schema + `next_review_plan` 必填非空结构）。
- **改动**:
  1. `presets/schemas/ce-executor-pipeline-loop.yml` 增加 `fix.done` topic 的 `required_fields`（含 `next_review_plan.focus_areas[] / fixed_findings[] / verification_performed[] / residual_risks[] / diff_ranges[]` 全数组字段，非空 + 元素约束）；
  2. `presets/en/ce-executor-pipeline-loop.yml` fixer hat 的 `instructions:` 在 Step 1 强约束 `next_review_plan` 非 `null`，并在 PRESET_OPT_IN_KEYS 把 `next_review_plan` 从"可空"提升为"必填"；
  3. `strict_reason_routing` lint 扩展到 `ce-executor-pipeline-loop` preset 的 L13+ 末态事件（`alignment.verified / report.done / LOOP_COMPLETE` 字符串 payload 必带 `reason`）；
  4. 落实 `docs/plans/2026-07-09-002-fix-pipeline-loop-main-conflict-convergence-plan.md` U2 review-synthesizer / U3 fix-planner 改写。
- **预期效果**: 把"能跑但必然 86% 复现"的链路契约上移到 schema，让 `cef 084141` 这一类问题在 lint 阶段就能拒收。
- **关联置信度**: P0-1 (86) + P1-2 (92)。

### 6.3 长期（机制 / 底座）

- **目标**: 修两处机制层 race。
- **改动**:
  1. **`hat_channel.rs:19-50` `prepare_hat_channel` race**: 在 `prepare_hat_channel` 返回 `Result<PathBuf>` 之前，把 channel 文件以 `O_EXCL`（独占创建）打开，且写入的 marker 加上 `fsync`，避免 isolated 模式下 hat activation race 时 channel 已是 0 字节但 marker 已写 → 修 `2026-07-03-002-...-093813-p0-orchestration-gaps-plan.md` U4 已埋点但未修根因的部分；
  2. **`event_loop/mod.rs:2380-2387` 硬改 typed TerminationReason**: `consecutive_hard_gates >= HARD_GATE_MAX` 不再返 `TerminationReason::Stopped`（同 operator stop 字面），改返 `TerminationReason::HardGateExhausted { count }`，并把 `as_str()` 映射 `"hard_gate_exhausted"`，history.jsonl / `loop-termination-reason.json` 同步迁移脚本；同步把 `plan.blocked {reason: loop_stalled_max_iterations}` 在 `event_loop/mod.rs:13804-13832` 路径上的"实际未落 events"事件追加到 orchestration ledger。
- **预期效果**: 让所有 hard-gate / fail-close 路径都能从 `loop-termination-reason.json` 字面就能区分 operator 与机制，让 `docs/report/...` 这类 P1-4 (65) 永久消失。
- **关联置信度**: P0-2 (78) + P0-3 (70) + P1-3 (75)。

---

## 7. 未核实疑点（置信度 < 60 或显著证据缺口）

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|----------|------------|------------|----------|
| **fixer 在 isolated 下是否子进程根本没启动**（vs 启动但没产 fix.done） | 45 | 缺 agent-output | 已读 `hat_channel.rs:19-50` 锚点 + log 4 次子进程 pid（19399/21834/23889/25783）显示 backend 在跑，但每次都没产 fix.done —— 子进程启动 vs 子进程产出是两件事 |
| **`plan.blocked` 是否在 bus publish 路径被 EventOriginGuard 拦截**（vs 真发了但未持久化） | 50 | 缺 `orchestration.jsonl` | 已读 `event_loop/mod.rs:13804-13832`，`target=shipper` 是合法 hat，理论上不应被拦；但 bus 层是否被 origin guard 或 terminal-priority 优先级吞掉无可见证据 |
| **`stopped` 字面是 operator Ctrl-C 还是 hard-gate 自动** | 50 | 缺 external input log | 已读 `runner.rs:1856, 2627, 2754` 三处 `"stopped"` 写点同字面，且都共用 `TerminationReason::Stopped` enum；无法区分 |
| **events.jsonl L11 review.synthesized 实际来源**（是空 channel 后 fallback 合并主 events 的产物 vs L11 是 fix-planner 又一次重发的残留） | 48 | 缺 `orchestration.jsonl` | 已读 events 全量，L11 ts=15:44:49.340795，与 L10 15:39:38.066494 间隔 5m11s；与 log 15:42:14/46 两次 fix_fallback 间相差很小；可推断 review-synthesizer 在 fallback 之后再次成功激活并发出 L11，但空 channel 后产物如何合并到主 events 不可证 |

---

## 质量门槛复核

- [x] §1 四问 **不可省略**；Q1–Q4 均有 **置信度** 列。
- [x] §5 **每条 P0/P1 必有置信度**；无 < 60 行；P0 无 < 70 行。
- [x] P0-1 有 `compound` 拆分（mechanism 90% / preset 78%）+ 加权 → 86。
- [x] §4 R1–R6 不可省；4 处违反 (R1 疑似 / R4 / R5 / R6 部分)。
- [x] §1.2 / §3 / §7 盲区声明 + LOGS_ONLY 封顶均明示。
- [x] 路径一律 **repo-relative**（`crash-derivation/ralph-orchestrator/...` 形式已规避）。
- [x] 报告在主仓 `docs/report/2026-07-10-ce-executor-pipeline-loop-primary-20260709-152400-diagnosis.md`。

