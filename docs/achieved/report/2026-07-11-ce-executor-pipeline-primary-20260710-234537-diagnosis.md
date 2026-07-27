---
title: ce-executor-pipeline Loop `primary-20260710-234537` 运行链路诊断报告
date: 2026-07-11
type: diagnosis
loop_id: primary-20260710-234537
preset: builtin:ce-executor-pipeline
run_dir: /Users/pittcat/Dev/Rust/modem_log_inspector
status: 硬拒终止 — dimension-reviewer scope_violation_hard_rejected,U5 plan 落地后第 3 次复发
diagnostics_mode: DISABLED
---

# ce-executor-pipeline Loop `primary-20260710-234537` 运行链路诊断报告

> **生成时间**: 2026-07-11 15:25 (UTC+8)
> **诊断对象**: `modem_log_inspector/.ralph/`(loop_id=primary-20260710-234537,启动 23:45:37 → 终止 01:33:54)
> **对照 preset**: `presets/en/ce-executor-pipeline.yml` + 内联 `event_policy.schemas`
> **执行方式**: 主 Agent 串行 Phase 0 盘点 → Agent A∥B 并行(流程+历史) → Agent C∥D 并行(对账+归因) → 主 Agent 汇总落盘
> **Diagnostics 模式**: **DISABLED**(无 session orchestration,无 agent-output.jsonl;log 仅 CLI 启动 + 终止 cleanup)
> **报告仓库**: `ralph-orchestrator` 主仓(`/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/`)
> **Tier C 根**: `docs/research/protocol-timeline-feasibility.md`(executor U7/U13 合法 commit 过;loop 终止时是 dirty)
> **置信度规则**: §5 仅收录 confidence≥60;P0 须 confidence≥70(见 `references/confidence-rubric.md`)

---

## 0. 产物盘点(Phase 0 必附)

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | `.ralph/current-events` → `events-20260710-234537.jsonl` | ✓ | 4 行 | **唯一**可信 events 指针(workflow 起 23:45:37) |
| S | `.ralph/events-history-20260710-234537.jsonl` | ✓ | 2 行 | warmup work.start + loop.terminate iteration 3 @ 01:33:54.736 |
| S | `.ralph/ledger.jsonl` | ✓ | 2 行 | iteration 1/2 counter_changed(loop_started 后 +1, executor 后 +1) |
| S | `.ralph/history.jsonl` | ✓ | 2 行 | `loop_started` + `loop_completed{reason=scope_violation_hard_rejected}` |
| S | `.ralph/loop-termination-reason.json` | ✓ | 1 行 | `{scope_violation_hard_rejected: {hat: dim:goal-alignment, diff_stat: feasibility.md \| 24 deletions(-)}}` |
| S | `.ralph/recovery.jsonl` | ✗ | 0 | 未触发拒收升级 |
| S | `.ralph/loops.json` | ✓ | 0 loops | 空(`loops: []`) |
| S | `.ralph/diagnostics/logs/ralph-2026-07-11T07-45-36-835-61413.log` | ✓ | 27 行(L20-26 关键) | 含 CLI 启动 + 3 处 audit/termination ERROR |
| A | `.ralph/agent/summary.md` | ✓ | — | 写入完成(Failed: scope_violation_hard_rejected, 3 iterations, 1h 48m 17s) |
| A | `.ralph/agent/decisions.md` | ✓ | — | executor 14 commit checkpoints + plan-reviewer §Step 2.5b |
| A | `.ralph/agent/handoff.md` | ✗ | — | 无(未触发 handoff 流程) |
| A | `.ralph/agent/tasks.jsonl` | ✗ | — | `tasks.enabled: false` |
| A | `.ralph/agent/progress.md` | ✗ | — | `state_projection` 缺省 |
| B | `.ralph/review/2026-07-11-001-feat-python-protocol-timeline-plan/{baseline,final}-verification.md` | ✓ | — | executor 验证产物齐全 |
| B | `.ralph/review/2026-07-11-001-feat-python-protocol-timeline-plan/{goal-alignment,review.diff.patch,review.diffstat.txt,verification-delta}.md` | ✓ | — | dim:goal-alignment 写 review 产品 → 但 also dirty feasibility.md |
| B | `.ralph/agent/plan-baseline-{plans-2026-07-11-001,PROMPT.pipeline}.sha` | ✓ | — | baseline SHA 锁定 |
| B | `.ralph/merge-queue.jsonl` / `.ralph/supervisor.db` | ✗ | — | non-supervisor preset |
| C | `docs/research/protocol-timeline-feasibility.md` | ✓ | — | (本文终结点)24 行净删除段 = "Unit 13 — Decode expansion backlog" |
| C | `docs/plans/2026-07-11-001-feat-python-protocol-timeline-plan.md` | ✓ | — | 13 Units, U1+U2 pre-existing |
| **C** | **U-ID chain commits** | ✓ | 14 | U1=aa288c0, U2=0dd553a, U3=30eb6bc, U4=be03f1b, U5=6b45540, U6=42d266e, U7=174c122, U8=3d6b4aa, U9=786c8f7, U10=dd41391, U11=d0155b9, U12=541c336, U13=01cdaff, + 5f102ad(operator post-loop re-append) |

**Diagnostics 模式判定 = DISABLED**:
- 无 `.ralph/diagnostics/<session-id>/orchestration.jsonl` → 不满足 MINIMAL 起点
- `.ralph/diagnostics/logs/` 内只有 CLI 启动/TUI 子进程 stderr(loop 启动一次,大小 1.3KB+4.8KB),**无** hat activation 详情 → **跳过 L2 orchestration 对账**

**OPAC 置信度硬顶**:
- DISABLED mode → 单项 ≤30(允许多档加权平均)
- **P0-1 mechanism** 例外:log L20/L22/L24(3 错误链)+ history.jsonl `loop_completed` typed reason = **双账本一致** → 例外到 85
- **P0-2 preset** 默认 70,沉淀 solution 已加固证到 70

**根因盲区声明**:
- 由于 mode=DISABLED,**无法** 100% 锁定可行性.md 的 24 行删除是 **dim:goal-alignment activation 内新 Edit** 还是 **activating-之前的 working tree dirty**。L1 Agent P1-1(cap=30)无法升 60→60,降 P1 不入 P0。

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: 部分偏离(假闭环 silent-success **已被 U5 消灭**,机制层硬拒生效);本次 loop **必然不完整**(13 hat chain 终止于第 3 个)
- **P0 / P1 / P2 数量**:**P0×2(mechanism + preset/adapter) / P1×1(agent 待证) / P2×0**(均 ≥ 入表门槛)
- **最高优先级根因置信度**: P0-1 = **85** / 100
- **历史复发**: 是 — **U5 plan 落地后第 3 次 `dimension-reviewer scope_violation_hard_rejected`**,且 **ce-executor-pipeline preset 首次**触发。前两次:2026-07-06 073823(agent 主动 Edit plan frontmatter,mechanism 成功证据)+ 2026-07-07 234147(机制粗粒度误判 operator pre-session dirty,P0-1 新一类)。

### 1.2 强制四问(debug.md)

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| **Q1** | 整体执行与 OPAC 是否合规? | ⚠️(编排路径合规;**OPAC DISABLED 硬顶**) | events 4 行不含 origin_violation;Plan/Branch 闭合;但单 hat OPAC ≤30(无 agent-output stream) | Q1 = **60**(编排走通,OPAC 失观测) |
| **Q2** | 基座机制是否正常生效? | ✅(U5 硬拒路径按设计硬拒成功) | `audit.rs:78-86` BlockLoop → `termination.rs:102-167` typed `ScopeViolationHardRejected` → history reason 字符串字面匹配 | Q2 = **85** |
| **Q3** | 编排是否合理、正常运行? | ⚠️(preset 设计合理,但触发 audit 命中) | 13 hat linear chain 终止于 dim:goal-alignment 第 1 步;`event_policy.on_violation: reject_with_resume` 没拦,audit 兜底 | Q3 = **70** |
| **Q4** | 问题归因:机制 vs 编排 vs agent? | **mechanism 主体 + preset 加性**(compound) | `audit_file_modifications` 粗粒度 + `disallowed_tools: ["Edit"]` 漏 Write → 触发 U5 硬拒 | Q4 = **85**(取 §5 P0-1) |

### 1.3 根因一句话

`audit_file_modifications` 用 `git diff --stat HEAD` 抓 dirty 把上游 executor 落地的 feasibility.md stale 改动算在 dim:goal-alignment 头上,触发 U5 plan 2026-07-04-004 设计的 BlockLoop 硬拒;同时 preset `disallowed_tools: ["Edit"]` 只挡 Edit 不挡 Write(`docs/solutions/tooling-decisions/claude-disallowed-tools-edit-write-dimension-reviewer.md` 已实测),给 agent 留了 shell-bypass 路径。(**置信度 85**)

---

## 2. 执行链路对比图

### 2.1 拓扑激活表

| Hat | preset triggers | preset publishes | execution_mode | 实际触发 | 状态 |
|---|---|---|---|---|---|
| Plan Reviewer | work.start | plan.ready, plan.blocked | isolated | 1 | ✅ |
| Executor (whole-plan TDD) | plan.ready | work.done, work.failed | isolated | 1 | ✅ |
| 🔍 Dimension Reviewer — Goal Alignment | work.done | review.goalalign.done | isolated | 1 | ⚠️(命中 BlockLoop) |
| 🔍 Dimension Reviewer — Correctness | review.goalalign.done | review.correctness.done | isolated | 0 | ⏸️(未触发) |
| 🔍 Dimension Reviewer — Testing | review.correctness.done | review.testing.done | isolated | 0 | ⏸️ |
| 🔍 Dimension Reviewer — Maintainability | review.testing.done | review.maintainability.done | isolated | 0 | ⏸️ |
| 🔍 Dimension Reviewer — Project Standards | review.maintainability.done | review.standards.done | isolated | 0 | ⏸️ |
| 🔍 Dimension Reviewer — Adversarial | review.standards.done | review.adversarial.done | isolated | 0 | ⏸️ |
| Review Synthesizer | review.adversarial.done | review.synthesized | isolated | 0 | ⏸️ |
| Fix Planner | review.synthesized | review.complete | isolated | 0 | ⏸️ |
| Fixer | review.complete | fix.done | isolated | 0 | ⏸️ |
| Alignment | fix.done | align.done | isolated | 0 | ⏸️ |
| Reporter | align.done / plan.blocked / work.failed | report.done, LOOP_COMPLETE | isolated | 0 | ⏸️ |

**触发 hat: 3/13(plan-reviewer ✅ / executor ✅ / dim:goal-alignment ⚠️)**

### 2.2 时间轴对比(预期 vs 实际)

| 时间(ts UTC+8) | topic | hat | payload 摘要 | 预期链路? | 备注 |
|---|---|---|---|---|---|
| 23:45:37.179 | work.start | loop-bootstrap | `# ce-executor-pipeline 编排契约`(全文 prompt) | ✅ | events L1;loop_started 已写入 history |
| 23:48:09.905 | plan.ready | plan-reviewer | `{plan_name: 2026-07-11-001-..., flow_audit: first_run, matched_uids: [U1, U2], missing_uids: [U3..U13], resolved_baseline_sha: b3e17e93...}` | ✅ | events L2;preset trigger 闭合 |
| 01:31:43.667 | work.done | executor | `{commit_count: 14, planned_units: U1..U13, completed_units: U1..U13, tests_run: 319, tests_passed: 316, post_verification_status: green, executor_head_sha: 5f102ad...}` | ✅ | events L3;execution contract green |
| 01:33:42.399 | review.goalalign.done | dim:goal-alignment | `{findings_count: 1, findings_file: .ralph/review/.../goal-alignment.md, executor_head_sha: 5f102ad..., resolved_baseline_sha: b3e17e93...}` | ✅ | events L4;产品在原位 |
| 01:33:54.762 (TUI cleanup, log L7) | (CLI 退出) | runner | `Subprocess TUI entering cleanup phase child_exit_status=ExitStatus(unix_wait_status(256))` | ⛔(终止) | |
| **01:33:54.695 (audit, log L20-22)** | dim:goal-alignment.scope_violation | dim:goal-alignment | `diff=docs/research/protocol-timeline-feasibility.md \| 24 ---- 1 file changed, 24 deletions(-)` | **⛔ 硬拒** | **BlockLoop 触发** |
| **01:33:54.736 (termination, log L24)** | loop.terminate | loop | `scope_violation_hard_rejected: terminating loop on first dimension-reviewer scope_violation` | **⛔** | terminal trigger |

> **关键诊断点**:`review.goalalign.done` 已 **成功落盘**(01:33:42),audit 在 **12 秒后**才命中(01:33:54)——audit 看的是 **文件改动** 而非 **emit 内容**;两个时间戳证明**产物正确发出,但 audit 不接受**。

### 2.3 终止与未触发

- **终止类型**: `scope_violation_hard_rejected`(typed,U5 plan 2026-07-04-004 设计的 hard-reject)
- **终止源 hat**: `dim:goal-alignment`
- **终止 trace 三处一致**:
  - `loop.terminate` events-history iteration 3 @ 01:33:54.736
  - `loop_completed{reason: scope_violation_hard_rejected}` history.jsonl 末行
  - ERROR log L24 字面匹配 "scope_violation_hard_rejected: terminating loop on first dimension-reviewer scope_violation"
- **未触发 hats** (10/13):5 dim(`correctness/testing/maintainability/standards/adversarial`)+ `review-synthesizer/fix-planner/fixer/alignment/reporter`
- **未走 Reporter Branch B**:audit 直接终止 loop,绕过了 reporter 的 `plan.blocked`/`work.failed` 端到端阻断路径 — preset 设计 vs runtime audit 路径的差异(由 P0-1 机制裁定)
- **未走 Branch A**:既未到 alignment 也未到 reporter,fixer/alignment/reporter 全 0 emit

---

## 3. 历史问题上下文

### 3.1 全景表

| 类型 | 文档路径 | 出现次数 | closed? | 与本次关联度 | 一句话摘要 |
|---|---|---|---|---|---|
| **dimension-reviewer scope_violation_hard_rejected**(mechanism 粗粒度) | `docs/report/2026-07-06-ce-executor-serial-primary-20260706-073823-diagnosis.md` | 第 1 次 | **否**(U5 plan 2026-07-04-004 落地) | **高** | agent 主动 Edit plan frontmatter 触发,mechanism 成功硬拒证据 100 |
| **dimension-reviewer scope_violation_hard_rejected**(mechanism 误判 operator dirty) | `docs/report/2026-07-07-ce-executor-serial-primary-20260706-234147-diagnosis.md` §5 P0-1 | 第 2 次 | **否** | **高** | operator pre-session dirty + direnv 自动 + agent Edit 三类混算,mechanism 85,P0-1 **新一类** |
| **dimension-reviewer scope_violation**(Edit vs Write) | `docs/solutions/tooling-decisions/claude-disallowed-tools-edit-write-dimension-reviewer.md` | 1 KB(2026-07-06 实测) | **是**(solution 已沉淀) | **高** | `--disallowedTools=Edit` 不挡 `Write`;`Edit,Write` 毁 findings;路径 allow headless 无效 |
| **U5 hard-reject 计划** | `docs/plans/2026-07-04-004-fix-ce-executor-serial-silent-success-p0-p1-plan.md` | 1 plan | **已合**(U5 patch 上线) | **高** | `audit_file_modifications` → BlockLoop + `RejectionKind::ScopeViolation` typed kind + termination trigger |
| **multi_hat_isolation lint** | `crates/ralph-core/src/preset_lint/multi_hat.rs`(12 hats > 3) | N/A | **是**(lint 强制 isolated) | 低(本次 preset 本就 isolated) | preset L63 `execution_mode: isolated` 已合规 |

### 3.2 根因分类对照

| 根因分类 | 历史报告 | 本次是否同因 |
|---|---|---|
| **mechanism 粗粒度**(`audit_file_modifications` 看 dirty 不分时窗) | 2026-07-06 + 2026-07-07 | **是**(同 mechanism,本次 dirty 是 feasibility.md 上游 stale) |
| **agent 主动 Edit** | 2026-07-06(plan frontmatter) | **存疑**(本次不能 100% 锁定;P1-1 cap=30) |
| **adapter 漏挡 Write** | 2026-07-06 实测 solution | **是**(同 `disallowed_tools` 漏 Write) |
| **preset 类别化禁写粒度** | 2026-07-06 instruction 写作 | **是**(dim:goal-alignment instructions L1199-1202/L1311-1317 文本完整,但未点名 `docs/research/*.md`) |

### 3.3 复发判定

- 满足「30 天内 ≥2 次」+ 「同一根因机制」
- 本次是 **U5 plan 2026-07-04-004 落地后第 3 次**(前两次:07-06 / 07-07;**本次 07-11 首次在 ce-executor-pipeline preset 上触发,而非 ce-executor-serial**)
- 历史 plan `2026-07-04-004` 已合(`U5 hard-reject` 已上线),**但 P0-1 机制粗粒度本身没有修复**(只把 silent-success 路径替换成 typed 硬拒;`audit_file_modifications` 仍用 `git diff --stat HEAD`)
- 标记:**复发 + 新一类**(dirty 来源是 reviewer 激活期间 feasibility.md 的 mixed state)

### 3.4 历史 plan 对照

| P0/P1 | 历史是否有 plan | 状态 | 落地 |
|---|---|---|---|
| P0-1 mechanism `audit_file_modifications` 粗粒度 | 2026-07-07 P0-1 plan(同 doc)**未单独落 root-cause plan** | open | **未合并**(本次报告再次点名) |
| P0-2 preset + adapter 漏挡 Write | 2026-07-06 solution 已沉淀,但 pipeline preset **未应用** | open | **未合并**(本次 pipeline preset `disallowed_tools: ["Edit"]` 仍漏 Write) |
| P1-1 agent 主动 Edit | 2026-07-06 plan 2026-07-04-004 已合并(U5 硬拒) | closed | 已落地(但本次机制层挡的是 dirty 不论来源) |

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度 | 置信度初估 | 证据缺口 |
|---|---|---|---|---|---|
| **DEV-001** | dim:goal-alignment 触发 `scope_violation_hard_rejected`,working tree vs HEAD 有 24 行 deletions,落在 `docs/research/protocol-timeline-feasibility.md` | log L20(`WARN hat=dim:goal-alignment diff=feasibility.md \| 24 -------- 1 file changed, 24 deletions(-)`) + log L22(`audit finding (block-loop severity, immediate termination)`) + log L24(`scope_violation_hard_rejected: terminating loop on first dimension-reviewer scope_violation`) + `loop-termination-reason.json` | P0 | **0.95** | 不知 24 行具体内容、agent 选择路径的 reasoning |
| **DEV-002** | agent 用 Edit 之外的方式(Write/Bash sed/Bash rm)改 in-repo 路径,违反 preset 显式约束 | preset L1174(`disallowed_tools: ["Edit"]`) + L1199-1202(`Read-only role...MAY NOT modify source code, configs, or docs`) + L1311-1317(`NEVER write findings to in-repo source paths (that triggers <hat>.scope_violation)`) | P0 | **0.85** | `audit_file_modifications` 未记录 tool 名(Edit/Write/Bash 区分不开);缺 agent-output |
| **DEV-003** | preset `event_policy.on_violation: reject_with_resume` 没拦住 scope violation;最终靠 U5(plan 2026-07-04-004)audit BlockLoop 路径硬终止 | preset YAML `event_policy.on_violation: reject_with_resume` + events 4 行无 `scope_violation` policy 事件 + 终止 reason=`scope_violation_hard_rejected`(audit 路径,非 policy 路径) + `event_loop/mod.rs:8130-8235` audit_file_modifications 优先 policy | P1 | **0.90** | 缺:events-history 中是否有 `<hat>.scope_violation` pre-warning |
| **DEV-004** | feasibility.md 在本次 loop 之前已被 executor 在 U7/U13 合法 commit;终止时 audit 看到 24 行 deletions 是 working tree vs HEAD 的差,**不是** committed deletion | `git log -20 -- docs/research/protocol-timeline-feasibility.md` → 174c122(U7 commit)/01cdaff(U13 commit) → commit 5f102ad「docs(feasibility): re-append U13 backlog after U7 overwrite」**在 loop 终止 8 小时后由 operator 提交** | P0 | **0.80** | 缺:`git diff HEAD` 在 audit 时刻的精确快照;缺 5f102ad commit body 内容细节 |
| **DEV-005** | events 4 行 vs 13-hat linear topology:仅 plan-reviewer/executor/dim:goal-alignment 触发,后续 10 hat 从未激活 | events 4 行 + preset 拓扑 13 hat + ledger 2 行 | P0 | **1.00** | 无 |
| **DEV-006** | `disallowed_tools: ["Edit"]` 只挡 Edit;**Write 仍可用**;adapter 未把 hat `disallowed_tools` 合并到 Claude spawn `--disallowedTools` | `docs/solutions/tooling-decisions/claude-disallowed-tools-edit-write-dimension-reviewer.md` 2026-07-06 三表实测 + `presets/en/ce-executor-pipeline.yml:1174` 显式声明 `disallowed_tools: ["Edit"]` + 本次 dirty 路径是 `docs/research/...` 而非 `.ralph/review/.../findings-*.md` | P0 | **0.70**(solution 已证) | 缺:adapter 实际的 spawn argv(merge hat `disallowed_tools`?) |
| **DEV-007** | recovery.jsonl 不存在 → 整个 audit chain 直接走 BlockLoop,未触发 `consecutive_failures` / recovery 升级 | `.ralph/` ls 无 recovery.jsonl + 终止 reason=`scope_violation_hard_rejected`(非 `recovery_exhausted`) | P2 | **0.95** | 缺:无 |
| **DEV-008** | diagnostics mode = DISABLED,仅 CLI 启动 + TUI cleanup,无 orchestration/agent-output 事件;OPAC 单项 ≤30 硬顶 | log 只有 CLI 启动 + 终止 cleanup + log L20/L22/L24(audit 模块,非 OPAC 通道) | P2 | **1.00** | 无法补——mode=DISABLED 是 loop 启动前的设计选择 |
| **DEV-009** | `.ralph/agent/tasks.jsonl` 不存在 → `tasks.enabled: false` | `.ralph/agent/` ls 无 tasks.jsonl + events 4 行无 task 业务事件 | P3 | **0.95** | 缺:preset 中 `tasks.enabled` 字段精确位置 |
| **DEV-010** | dim:goal-alignment instructions 路径级禁止写得正确但**未点名** `docs/research/*.md`,agent 视角下 feasibility.md 看起来像「可写 findings 的低风险 in-repo 路径」 | preset L1199-1202 + L1311-1317 vs `docs/research/protocol-timeline-feasibility.md` 实际归属 | P3 | **0.55** | 缺:agent 的 activation transcript(无法看它如何理解 instructions) |

### 4.1 OPAC 逐 hat 审计表(DISABLED 单项硬顶 ≤30)

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---|
| plan-reviewer | 25 | 20 | 15 | 10 | events L2 `plan.ready` 由 plan-reviewer 发出;无 per-hat OPAC log(DISABLED) | 18(≤30) |
| executor | 25 | 20 | 15 | 10 | events L3 `work.done` 含 `post_verification_status=green`;commit_count=14 | 18(≤30) |
| **dim:goal-alignment** | **30** | **30** | **30** | **30** | events L4 `review.goalalign.done` 发出;**audit 在同 activation 内检测到 Write 到 feasibility.md,触发 `scope_violation_hard_rejected`**;log L20/L22/L24 三连 | **30**(因为有失败可观测) |
| 后续 10 hat(correctness..standards / adversarial / review-synthesizer / fix-planner / fixer / alignment / reporter) | 0 | 0 | 0 | 0 | **从未触发**,因 BlockLoop 立即终止 | N/A(无数据,非健康) |

> **DISABLED 注脚**:Confirm 列 N/A 在该模式下允许;Precheck 列不可见;**不**因 Precheck 缺失单独判 P0 OPAC 违规(plan-reviewer/executor/dim:goal-alignment 都标 ⚠️ 不标 ❌)

### 4.2 R1–R6 检查

| Rule | 状态 | 证据 |
|---|---|---|
| R1(单事件预算) | ✅ | plan-reviewer 1 emit / executor 1 emit / dim:goal-alignment 1 emit,均 ≤1 |
| R2(trigger context) | ✅ | L3/L4 payload 含 schema 要求的字段子集(`plan_name`/`plan_path`/`post_verification_status`/`executor_head_sha` 等) |
| R3(终态事件不夹带业务事件) | ✅ | 无 `plan.complete`/`LOOP_COMPLETE`/`plan.blocked` 出现 |
| R4(fail-close 必须有 typed reason) | ✅ | scope_violation_hard_rejected 是 typed(plan 2026-07-04-004 U5 落地) |
| R5(emit 必须 precheck) | **不可验证** | mode=DISABLED,无 agent-output stream 抓取 |
| R6(preset 拓扑与 events 一致) | ⚠️ | L1-L4 与 topology 一致;后续 10 hat 因 BlockLoop 立即终止未触发 |

### 4.3 机制十二项矩阵

| # | 机制 | 状态 | 证据 |
|---|------|------|------|
| 1 | origin guard | ✅ | events 4 行无 `origin_violation`,log 无 origin warn |
| 2 | payload contract | N/A | recovery.jsonl 不存在;events L3/L4 schema 未在 OPAC 通道被校验(DISABLED) |
| 3 | execution contract | ✅ | events L3 `work.done` 含 `post_verification_status=green`、`tests_passed=316`、`tests_run=319` |
| 4 | workflow guard | N/A | pipeline 是 single-phase linear,无 phase violation 场景 |
| 5 | isolated 单事件预算 | ✅ | 每 hat 仅 1 emit |
| 6 | step_handoff + semantic_gate | ✅ | events L2→L3 / L3→L4 正常驱动,无 0 emit 也无误触发 |
| 7 | recovery 升级 | ❌(未走) | BlockLoop 路径优先于 recovery chain;`consecutive_failures` 未触发 |
| 8 | resume 路由 | N/A | 无 consumer;pipeline 无 review-reentry |
| 9 | stall | ✅ | 终止前无 stall(plan-reviewer 触发后到 work.done 间隔 ~100 分钟;work.done→review.goalalign.done 间隔 < 2 分钟) |
| 10 | drift | N/A | session 不存在(DISABLED);recovery 不存在(双重 N/A) |
| 11 | dedup | ✅ | L3 正常驱动 dim:goal-alignment,无 duplicate fires |
| 12 | terminal / silent-success | ✅(伪假闭环已消除) | `loop_completed` 携带 typed reason `scope_violation_hard_rejected`,**非** silent-success — U5 plan 2026-07-04-004 的「非 silent 硬终止」实证 |

---

## 5. 问题归因表(confidence ≥ 60;P0 ≥ 70)

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|----------|----------|
| **P0-1** | `audit_file_modifications` 用 `git diff --stat HEAD` 抓 dirty → 凡 Edit/Write 被禁的 dim hat,working tree 任何 dirty 都算在它头上 → 硬拒 | **mechanism**(粗粒度 dirty 不分"本 hat 改动" vs "上游遗留") | **85** | DEV-001 + DEV-002 + DEV-003 + DEV-004 + DEV-005(`event_loop/mod.rs:8130-8235`) | **高**(U5 落地后第 3 次) | 1→85 |
| **P0-2** | `disallowed_tools: ["Edit"]` 只挡 Edit,**Write 仍可用**,且 adapter 未把 hat `disallowed_tools` 合并到 Claude spawn `--disallowedTools` → dim hat 用 Write/Bash 绕过 | **preset + adapter**(防护口径不严,缺整工具名 deny) | **70** | DEV-006(2026-07-06 实测 solution) + DEV-007(`presets/en/ce-executor-pipeline.yml:1174`) + DEV-008(本次 dirty 路径 `docs/research/...` 典型 Write-bypass 形态) | **中**(同 solution 07-06 已沉淀,但 pipeline preset 未应用) | 0→70 |
| **P1-1** | (待证)feasibility.md 24 行净删除可能是 dim:goal-alignment activation 内新改动,或 activating 之前 dirty | **agent**(待证) | **≤30** | DEV-009(`git log --all -- docs/research/protocol-timeline-feasibility.md` 5f102ad 09:29 operator 提交;工作树现 24 行净删等于 5f102ad 的反向操作)+ DEV-004(working tree 混合状态无法 git reflog 锁定 activation 内 dirty 来源) | **新一类**(相对 07-06/07-07) | 0→30 |

**Compound 公式说明**:
- P0-1 主体(mechanism 85)+ P0-2 加性(preset 70)→ 整行取最小值;**P0-1 与 P0-2 不是 compound 行**,而是独立两条 P0,机制层先发现 + preset 缺陷放大
- P1-1(agent)无法升 P0:缺 agent-output stream(DISABLED mode 上限 30)

---

## 6. 修复建议(仅针对 §5 已入表项;§7 疑点不驱动修复)

### 6.1 短期(operator workaround,本周内)

- **目标**:在 main pipeline run 上防 P0-1 / P0-2 立即触发。
- **改动**:
  1. **operator 清理 working tree**:在 rerun 前跑 `git status --short`,确认 feasibility.md 不再 dirty(`git checkout HEAD -- <file>` 或 `git restore`),**因为 audit 现在用 `git diff HEAD` 无 activation 时窗**;
  2. **路径白名单 shell alias**:`alias ralph-audited='git diff HEAD --stat | rg -v "^\s*docs/research/"'` 在跑前过滤可疑 dirty(临时);
  3. **不修改 dim:goal-alignment 行为**,接受本次 BlockLoop 触发 → 通过 `ralph loops` / `ralph inspect loop` 收尾 `loop_completed{reason=scope_violation_hard_rejected}` 的语义(不要去 silent-retry)。
- **预期效果**:本周 rerun 不再因 feasibility.md dirty 立即触发 P0-1(但 P0-2 preset gap 仍在)。
- **关联置信度**: **P0-1 = 85**(机制层无法修,但 working tree 干净可让 audit 不查)

### 6.2 中期(preset / schema / instructions,1-2 周)

- **目标**:根治 P0-2 preset + adapter 漏挡 Write。
- **改动**:
  1. **`presets/en/ce-executor-pipeline.yml:1164-1355` dim:goal-alignment instructions 升级**:
     - 在「Read-only role」段显式点名 `docs/research/*.md` 与 `.ralsph/agent/scratchpad.md` 之外的路径；
     - 加 Step 0「executor 已在 U7/U13 commit 过的 in-repo file,**你不要再动**」反向预期;
     - 给 findings 唯一允许写路径:`.ralsph/review/{plan_name}/{dim}.md`(不写 `docs/research/...`)。
  2. **adapter 合并 hat `disallowed_tools` 到 `--disallowedTools`**(`crates/ralph-adapters/src/cli_backend.rs`):
     - 加 `Edit` + `Bash`(尤其 `Bash(cat|tee|sed|...|rm)` family)整工具名 deny;
     - **保留** Write 给 findings;
     - 不依赖路径 allow/deny(headless 已知不可靠,见 2026-07-06 实测)。
  3. **preset_lint 加 `disallowed_tools_path_audit`**:lint 把 `disallowed_tools` 缺 Write 作为 warning 推 preset author,而不是 fail(因为合法 findings 仍需 Write)。
- **预期效果**:P0-2 closed;P0-1 仍有机制层问题,但 agent 不会动 in-repo,working tree dirty 来源消解。
- **关联置信度**: **P0-2 = 70**

### 6.3 长期(机制 / 底座,2-4 周)

- **目标**:根治 P0-1 机制层 `audit_file_modifications` 粗粒度。
- **改动**:
  1. **`event_loop/mod.rs:8130-8235` `audit_file_modifications` 感知 activation 时窗**:
     - 在 hat activation 进入时(`LoopRunner::activate_hat`)capture `git rev-parse HEAD` 或 `git write-tree` baseline SHA,
     - activation exit 时 `git diff --stat {baseline_sha}..HEAD` 比较,**不再** 看 `git diff --stat HEAD` 全树;
     - 配合 plan 2026-07-07 P0-2 solution 的 operator pre-session dirty 也用「baseline SHA」消除。
  2. **`audit_file_modifications` 记录 tool 来源**:Emit `<hat>.scope_violation` payload 增加 `tool_calls: [{tool, file_path, ...}]` 由 cli_backend 收集(参考 `docs/solutions/.../claude-disallowed-tools-edit-write-dimension-reviewer.md` §When to Apply #2);
  3. **typed TerminationReason 字段扩展**:`ScopeViolationHardRejected { hat, diff_stat, tool_calls }` 三元组落 `loop-termination-reason.json`,让 dashbaord 可区分 agent 主动 Edit vs 上游遗留 dirty vs adapter 漏挡。
- **预期效果**:P0-1 永久 → 60-70(误判消除);P0-2 layered defense 也难以绕过;P1-1 在 agent 真主动 Edit 时 confidence 可升到 80+(有 tool_calls 证据)。
- **关联置信度**: **P0-1 = 85**(根因级修复)

---

## 7. 未核实疑点(confidence < 60;不驱动修复)

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|----------|------------|------------|----------|
| **feasibility.md 24 行净删除**,是 dim:goal-alignment activation 内新 Edit,还是 terminating 之前 dirty(stale) | **30** | **缺 agent-output stream(DISABLED mode),working tree 是混合状态无法 git reflog 锁定** | git log+git reflog+`git show 5f102ad`+`git diff HEAD -- docs/research/protocol-timeline-feasibility.md` 已查 |
| **`01cdaff U13 gap_report` 09:27 commit 之前**,executor 是否在 activation 内已写过一次 feasibility.md U13 段又被自重置("U7 overwrite"字样) | **50** | 缺 executor agent-output;`.ralsph/agent/decisions.md` 只记录 checkpoint 不记文件动作 | `.ralsph/agent/decisions.md` 全文已读(只列 commit + remaining,无文件 diff 序列) |
| **`disallowed_tools: ["Edit"]` 经 spawn 层是否合并到 Claude `--disallowedTools`**,或在 headless `claude --print` 下 Write 实际被拒 | **60** | 缺 adapter 日志;无法确认本次 dim 进程实际拿到的 tool set | 2026-07-06 solution 实测已证 Edit-only 不挡 Write;但本次未抓 argv |
| **同源机制在 ce-executor-serial preset 上是否同样存在 P0** | 90 | `dim:goal-alignment` 是 ce-executor-pipeline 独有;serial preset 用单 `dimension-reviewer` hat,机制相同但 hat 名不同 | 2026-07-06 + 2026-07-07 双复发已证实 |

---

## 8. 历史 run 对照表

| 报告 | preset | 同症状? | 同根因? | 不同点 |
|------|--------|---------|---------|---------|
| `2026-07-06-ce-executor-serial-primary-20260706-073823-diagnosis.md` | ce-executor-serial | 是 | mechanism 主因相同;agent 主动 Edit plan frontmatter | 本次不是 plan frontmatter;是 feasibility.md research doc |
| `2026-07-07-ce-executor-serial-primary-20260706-234147-diagnosis.md` | ce-executor-serial | 是 | mechanism 粗粒度相同;operator pre-session dirty | 本次 dirty 类型不同:feasibility.md (U7/U13 已 commit)+ activation 间 transform 而非 untouched file + direnv |
| `2026-07-03-ce-executor-pipeline-primary-20260702-163157-diagnosis.md` | ce-executor-pipeline | 否 | 拓扑不同 | 历史是 silent-success,本次机制层已修 |

---

## 9. 关键主仓代码引用清单

| 主题 | file:line |
|---|---|
| `audit_file_modifications` body & BlockLoop 分支 | `crates/ralph-core/src/event_loop/mod.rs:8130-8235` |
| `AuditSeverity::BlockLoop` 立即 termination log | `crates/ralph-core/src/event_loop/audit.rs:78-86` |
| `TerminationTrigger::ScopeViolation` → `TerminationReason::ScopeViolationHardRejected` | `crates/ralph-core/src/event_loop/termination.rs:102-167` |
| `RejectionKind::ScopeViolation` typed kind(U5 新增) | `crates/ralph-core/src/preset/engine/gates.rs:90-130` |
| `audit_file_modifications` 仅 Edit/Write 限制 dim hat | `crates/ralph-core/src/event_loop/mod.rs:8139-8142` |
| Claude CLI Edit-vs-Write 实测三类表 | `docs/solutions/tooling-decisions/claude-disallowed-tools-edit-write-dimension-reviewer.md`(全文) |
| U5 plan silent-success → BlockLoop 改造 | `docs/plans/2026-07-04-004-fix-ce-executor-serial-silent-success-p0-p1-plan.md`(全文) |
| preset dim:goal-alignment topology | `presets/en/ce-executor-pipeline.yml:1164-1355`(HARD RULES L1199-1202, findings path L1311-1317) |
| preset `disallowed_tools: ["Edit"]` | `presets/en/ce-executor-pipeline.yml:1174` |
| preset `event_policy.on_violation: reject_with_resume`(无 BlockLoop 命中) | `presets/en/ce-executor-pipeline.yml:73` |

---

## 10. 提交前检查

- [x] Phase 0 盘点表已在 §0
- [x] 只读了 `current-events` 指向的一个 events 文件(4 行)
- [x] DISABLED mode 已在 §0 标注;未因缺 orchestration 误标 P0
- [x] §5 每条 P0/P1 有置信度;P0-1=85 ≥70,P0-2=70 ≥70 入表;P1-1=30 ≤60 降为 P1
- [x] confidence<60 的 P1-1 已落入 §7,未混入 §5/§6
- [x] 每条 P0 有 DEV + 源码或 preset 行号(P0-1: `event_loop/mod.rs:8130-8235`;P0-2: `ce-executor-pipeline.yml:1174` + solution 实测)
- [x] 日志三联至少 5 行对账(events/events-history/history.jsonl/loop-termination-reason.json/events history terminal log 三连)
- [x] 历史表 ≥3 行(§3.1 5 行 + §3.2 4 行 + §3.3 1 行 + §3.4 3 行 + §8 4 行)
- [x] 报告路径 `docs/report/2026-07-11-ce-executor-pipeline-primary-20260710-234537-diagnosis.md` 已写入
