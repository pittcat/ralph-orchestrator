---
title: ce-executor-serial Loop `primary-20260705-224028` 运行链路诊断报告
date: 2026-07-06
type: diagnosis
loop_id: primary-20260705-224028
preset: presets/en/ce-executor-serial.yml
run_dir: /Users/pittcat/Dev/Rust/ralph-e2e-serial
status: silent-success 假闭环(P0)
diagnostics_mode: MINIMAL
---

# ce-executor-serial Loop `primary-20260705-224028` 运行链路诊断报告

> **生成时间**: 2026-07-06
> **诊断对象**: `/Users/pittcat/Dev/Rust/ralph-e2e-serial/.ralph/`（loop_id=primary-20260705-224028,启动 22:40:28Z → 终止 23:08:19Z,18 iter, 27m 51s）
> **对照 preset**: `presets/en/ce-executor-serial.yml` + `presets/schemas/ce-executor-serial.yml`
> **执行方式**: 4 sub-agent 并行（流程还原 / 历史 / 对账 / 归因）→ 汇总
> **Diagnostics 模式**: MINIMAL(session 缺 `orchestration.jsonl` / `errors.jsonl` / hat-prompt 现场数据)
> **报告仓库**: `ralph-orchestrator` 主仓（非 run_dir）
> **Tier C 根**: `.agents/scratchpad/ce-executor/2026-06-20-001-feat-python-sort-algorithms-plan/`
> **置信度规则**: §5 仅收录 confidence≥60；P0 须 confidence≥70（见 confidence-rubric）
> **关键事实**: 本次 run 起始 (22:40:28Z) 早于 fix commit `6c01bac8` (23:14:13Z) 约 **34 分钟**——本次 run 必然命中 pre-fix 代码路径

---

## 0. 产物盘点（Phase 0）

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | `.ralph/current-events` | ✓ | 1 行 | 指向 `events-20260705-224028.jsonl` |
| S | `events-20260705-224028.jsonl`(trusted,唯一可信) | ✓ | **20** | 编排拓扑 SSOT(经指针解析,未通配) |
| S | `events-history-20260705-224028.jsonl` | ✓ | 2 | warmup only |
| S | `.ralph/ledger.jsonl` | ✓ | 19 | iter 1→18 全覆盖,seq=17 `loop.completion_requested` |
| S | `.ralph/recovery.jsonl`(workspace RepairStream) | ✓ | 2 | 仅 Info 级 sink(`work.ready` ×1 + `plan.blocked` ×1) |
| S | `.ralph/loops.json` | ✓ | - | `{"loops": []}`(17 字节) |
| S | `.ralph/loop.lock` | ✓ | 0 字节 | lock 已释放(空文件) |
| S | `.ralph/diagnostics/logs/ralph-*.log` | ✓ | - | 2 个 log 文件 |
| A | `.ralph/agent/tasks.jsonl` | ✓ | 3 | 全部 closed;L1 与 L3 共用 `task_id=task-1783291303-a994` 但 task_key 不同 |
| A | `.ralph/agent/progress.md` | ✓ | 4 | 仅 `step-01`(events 实际跑 u1+u2 两 unit) |
| A | `.ralph/agent/summary.md` | ✓ | - | "Completed successfully",27m 51s,final commit 5f1c498 |
| A | `.ralph/agent/handoff.md` | ✓ | - | session 续跑上下文,无 pending |
| A | `.ralph/agent/scratchpad.md` | ✓ | **0 字节** | 已迁移到 `.agents/scratchpad/`(旧路径空文件) |
| B | `.ralph/diagnostics/2026-07-06T06-40-27/`(session) | ✓ | - | MINIMAL:drift=0、recovery=3、trace=5、diagnosis-summary 已写 |
| B | `.ralph/diagnostics/2026-07-06T06-40-27/orchestration.jsonl` | ✗ | 0 | **缺失** → diagnostics=MINIMAL 而非 FULL |
| C | `.ralph/agent/scratchpad.md` 0 字节 vs `.agents/scratchpad/ce-executor/{plan_name}/` 有内容 | 双路径模型并存 | - | preset L812 旧路径 vs L280-282 新路径 |
| C | `ralph.yml` | ✓ | - | `coordinator_hats=[coordinator, progress-steward]`、`telemetry.runtime_diagnosis.write_artifacts=true` |
| C | `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md` | ✓ | - | 2 unit: u1-skeleton / u2-polish |
| C | `.agents/scratchpad/ce-executor/2026-06-20-001-feat-python-sort-algorithms-plan/` | ✓ | - | 仅 3 个 `findings-*.json`(goal-alignment/correctness/testing)+ context.md/decisions.md/plan.md/progress.md/review-sequence.json/review-trace.json/review-diff.patch+meta+logs;**无 findings.md / 无 fix-plan.md / 3 维缺失** |
| C | `docs/report/2026-07-06-ce-executor-2026-06-20-001-feat-python-sort-algorithms-plan-report.md` | ✓ | - | reporter 出口报告 |

**盲区 / 根因置信度硬顶(MINIMAL 模式封顶)**:
- agent 归因 ≤60;mechanism 根因 ≤85;OPAC U2/U4/U13/U17-U26 ≤「低」置信度
- 缺 agent-output(看不到 hat 内 agent prompt 现场)
- 缺 orchestration.jsonl(看不到 hat activation 时序)
- 但本次 run 关键证据**全部为机制层**(events/recovery/ledger/shipper_reason.rs/preset/schema)——MINIMAL 限制**不削减**主因证据强度

---

## 1. 结论摘要

### 1.1 健康度

- **判定**:**假闭环 silent-success(P0)**。loop 自报 `LOOP_COMPLETE, exit code 0`,`summary.md` 标 "Completed successfully" verdict=pass;但 **review_walk phase 在 3/6 维后崩盘**(`maintainability` 维 ready 后无 done、`project-standards` / `adversarial` 未发出),`review-synthesizer` / `fixer` / 整条 fix_units 链 **完全未触发**,`findings.md` / `fix-plan.md` **不存在**,shipper 仍 emit `REVIEW_COMPLETE(pass_or_fail=pass, verdict=pass)`(无 `pass_with_residuals` 中间态,直接 pass)。
- **关键异常**:P0 × 4 / P1 × 1(均为 confidence≥入表门槛);P2 线索 2 项未入表
- **最高根因置信度**:P0-1(silent-success 主根因)= **85** / 100(mechanism 单点缺陷,MINIMAL 封顶)
- **历史复发**:是 —— 与 153532(昨日姊妹 run)/ 130118 / 115242 / 024019 / 075227 / 093813 / 130118 / 140149 / 140433 / 151220 / 170451 / 032648 / 175407 等同模式,**30 天 9 次同簇复发**。本次 run 跑在 `docs/plans/2026-07-04-004-fix-ce-executor-serial-silent-success-p0-p1-plan.md`(status: planned)落地**之前**;并且跑在 fix commit `6c01bac8` (2026-07-06 23:14:13Z)合并**之前 34 分钟**——本次 run 必然命中 pre-fix 代码路径。
- **代码本身**:健康 —— 2 个 step 全 work.done(commit b2da9f6、5f1c498)、22 个 test 全部通过(7 unit + 15 integration)。问题**不在工程产出**,在**编排链路和机制缺失**。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规? | ❌ 偏离 | 20 个 trusted events 中 9 个有偏差(45%);review_walk 整体在 3 维后卡死;shipper `REVIEW_COMPLETE(pass)` 越过 preset L2757「必读 findings.md」硬约束 | **78** |
| Q2 | 基座机制是否正常生效? | ❌ 偏离 | `runtime-recovery ForcePlanBlocked` 路径(`event_loop/mod.rs:5583-5594`)仅 `bus.publish` 不调 `record_event` → runtime 注入的 plan.blocked 不进 events.jsonl;shipper 看到的是 bus 内存另一份 | **85** |
| Q3 | 编排是否合理、正常运行? | ❌ 偏离 | review-coordinator 6 维串行 0/6 → synthesizer / fixer 全程未激活;session recovery `handoff_dispatch_timeout` 600s 悬空;`review_in_progress_findings_pending` 是 30 天全仓 0 命中的新 reason 变体 | **80** |
| Q4 | 问题归因:机制 vs 编排 vs agent? | mechanism 主因(≥70%)+ preset 配合不足(20%)+ agent 弱因(≤10%) | 1 处机制缺陷(M-3 `ForcePlanBlocked` 不调 `record_event`)引发 4 条 P0;preset O-1 6 维串行 + exempt_topics 协同 | **85**(取 §5 主因最高值) |

### 1.3 根因一句话

**`event_loop/mod.rs:5583-5594` `RecoveryAction::ForcePlanBlocked` 分支只调 `self.bus.publish` 不调 `self.state.record_event`,导致 runtime-recovery 注入的 plan.blocked 永不写入 trusted events.jsonl —— 后续 shipper 看到的 reason 是 recovery_runtime bus 内存另一份(reason 字面在 events L17 / workspace recovery L2 / shipper L18 三处不一致:`loop_stalled_max_iterations` / `review_in_progress_findings_pending:...` / `[recovery_exhausted:..., loop_stalled_max_iterations]`),结合 `shipper_reason.rs:64-77` 中 `recovery_exhausted:stall_recovery:coordinator:task_resume:handoff_dispatch_timeout` 在 prefix allowlist 内,最终 REVIEW_COMPLETE 被错误提升为 pass,叠加 review_walk 在 3/6 维后崩盘(`findings.md` / `fix-plan.md` 不存在),形成 silent-success 假闭环(confidence 85,主因 mechanism;shipper 词法独立可证 confidence 78;handoff_dispatch_timeout M-2 confidence 80)**。

---

## 2. 执行链路对比图(Agent A 输出,摘要)

完整图表见 `.ralph/diagnostics/2026-07-06T06-40-27/agent-brief/execution-chain-comparison.md`(mermaid 已 mcp 验证通过 valid:true / 0 issues)。

### 2.1 拓扑激活表(10 hat + precheck desugar = effective 12)

| Hat | triggers(preset) | publishes(preset) | 实际激活次数 | 实际触发 topic | 实际 publishes | 缺失上游 |
|---|---|---|---|---|---|---|
| **coordinator** | work.start / task.resume / test.passed / review.complete / work.failed | work.ready / review.start / plan.complete / plan.blocked | **3** | work.start(L2)→ test.passed(L4,L7)→ 内置 stall(L17) | work.ready(L2,L5) / review.start(L8) / plan.blocked(L17) | review.complete 未到达(维度卡死) |
| **executor** | work.ready / fix.exhausted | work.done / work.failed | **2** | work.ready(L2,L5) | work.done(L3,L6) | — |
| **validator** | work.done / fix.applied | test.passed / test.failed | **2** | work.done(L3,L6) | test.passed(L4,L7) | test.failed 未触发 |
| **fixer** | test.failed / fix.plan.ready | fix.applied / fix.exhausted / debug.exhausted | **0** | — | — | test.failed 未触发,hat 全程跳过 |
| **review-coordinator** | review.start / review.dimension.done | review.dimension.ready / review.dimensions.complete | **≥4** | review.start(L8) | review.dimension.ready(L9,L10,L12,L14,L16) | 6 维只发 5 次 ready(重复 goal-alignment 一次),3 次 done 后停摆 |
| **dimension-reviewer** | review.dimension.ready | review.dimension.done / review.dimension.failed | **3** | review.dimension.ready(L9/L10→L11;L12→L13;L14→L15) | review.dimension.done(L11,L13,L15) | maintainability / project-standards / adversarial 维度的 ready 没有 done |
| **review-synthesizer** | review.dimensions.complete | review.complete | **0** | — | — | **review.dimensions.complete 未到** → review.complete 未发 → 整条 fix_units 链缺失 |
| **shipper** | plan.complete / plan.blocked | REVIEW_COMPLETE | **1** | plan.blocked(L17) | REVIEW_COMPLETE(L18, pass) | 仅靠 plan.blocked 驱动,plan.complete 未到 |
| **reporter** | REVIEW_COMPLETE | report.done / LOOP_COMPLETE | **1** | REVIEW_COMPLETE(L18) | report.done(L19) + LOOP_COMPLETE(L20) | — |
| **progress-steward** | loop.stalled | task.resume / plan.blocked | **≥1(隐式)** | loop.stalled(loop 内部触发) | plan.blocked(L17, reason=loop_stalled_max_iterations) | recovery.jsonl 第 2 行 `stall_recovery:coordinator:task_resume:handoff_dispatch_timeout:*` 600s 无响应 |

**task 投影侧**:tasks.jsonl 3 行均 status=closed,`owner_hat_id` 仅 L2 有(`coordinator`),L1/L3 共用 task_id=`task-1783291303-a994` 但 task_key 不同(u1 vs u2)。

### 2.2 时间轴对比表(business events only)

| L# | ts(UTC) | topic | hat | 关键 payload | 期望(preset/schema) | 状态 |
|---|---|---|---|---|---|---|
| 1 | 22:40:28 | work.start | loop-bootstrap | `@docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md` | 起点 | ✅ |
| 2 | 22:42:09 | work.ready | coordinator | step=step-01 / task_id=task-1783291312-c3d6 / task_key=…:u1-… | PHASE 1 | ✅ |
| 3 | 22:44:59 | work.done | executor | commit_count=1 / changed_lines=263 | TDD | ✅ |
| 4 | 22:45:36 | test.passed | validator | tests_passed=7 / tests_run=7 | validator | ✅ |
| 5 | 22:46:44 | work.ready | coordinator | step=step-01 / task_id=task-1783291303-a994 / task_key=…:u2-… | PHASE 1 | ⚠️ step 仍 step-01(任务 key 切换 u1→u2) |
| 6 | 22:48:13 | work.done | executor | commit_count=1 / changed_lines=113 | TDD | ✅ |
| 7 | 22:48:54 | test.passed | validator | tests_passed=15 / tests_run=15 | validator | ✅ |
| 8 | 22:49:46 | review.start | coordinator | task_key=…:u2-… | 进入 review 阶段 | ✅ |
| 9 | 22:51:31 | review.dimension.ready | review-coordinator | dimension=goal-alignment | 第 1 维 | ✅ |
| 10 | 22:51:54 | review.dimension.ready | review-coordinator | dimension=goal-alignment(同 L9 重复) | 同上 | ⚠️ **重复 ready** |
| 11 | 22:54:52 | review.dimension.done | dimension-reviewer | goal-alignment / findings_count=0 | 期望 done | ✅ |
| 12 | 22:56:22 | review.dimension.ready | review-coordinator | dimension=correctness | 第 2 维 | ✅ |
| 13 | 22:58:34 | review.dimension.done | dimension-reviewer | correctness / findings_count=0 | 期望 done | ✅ |
| 14 | 22:59:24 | review.dimension.ready | review-coordinator | dimension=testing | 第 3 维 | ✅ |
| 15 | 23:01:03 | review.dimension.done | dimension-reviewer | testing / **findings_count=7**(p2=3 + p3=4) | 期望 done | ✅ |
| 16 | 23:02:07 | review.dimension.ready | review-coordinator | dimension=maintainability | 第 4 维 ready | ⏸️ **done 未到**;review-sequence.json status=pending |
| — | — | review.dimension.ready | review-coordinator | (project-standards 期望) | 第 5 维 | ❌ **未发出** |
| — | — | review.dimension.ready | review-coordinator | (adversarial 期望) | 第 6 维 | ❌ **未发出** |
| — | — | review.dimensions.complete | review-coordinator | — | 必需 | ❌ **未发出** |
| — | — | review.complete | review-synthesizer | — | 必经 | ❌ **未发出** |
| — | — | fix.applied / fix.exhausted | fixer | — | — | ❌ 链断 |
| — | — | plan.complete | coordinator | — | plan_end 出口 | ❌ **未发出** |
| 17 | 23:05:21 | plan.blocked | coordinator | reason=`loop_stalled_max_iterations` | 触发 shipper(recoverable) | ⚠️ **reason 字面 ≠ workspace recovery L2 ≠ shipper L18 三处不一致** |
| 18 | 23:06:48 | REVIEW_COMPLETE | shipper | verdict=**pass** / pass_or_fail=**pass** / final_findings_count=7 | shipper 出口 | ⚠️ **plan.blocked 走 recoverable → shipper 翻译为 pass,越过 preset L2757 必读 findings.md 硬约束** |
| 19 | 23:08:10 | report.done | reporter | report_path / awaiting_decision=false | reporter 出口 | ✅ |
| 20 | 23:08:12 | LOOP_COMPLETE | reporter | reason="All steps completed successfully" | 终态 | ⚠️ **reason 描述与事实不符** |

### 2.3 mermaid 流程对比图

> 完整 mermaid 见 `.ralph/diagnostics/2026-07-06T06-40-27/agent-brief/execution-chain-comparison.md`(已通过 mcp `mermaid_validator validateMermaidPreview` 校验,valid:true / 0 issues / 1 block)。核心对比:**预设拓扑 12 hat 闭合链路 vs 实际链路在 review_synthesizer 之前断链 → shipper 在 plan.blocked 上越权翻译为 pass → 假闭环**。

---

## 3. 历史问题上下文(Agent B 输出,摘要)

完整知识库见 `/tmp/agent-b-historical-context.md`。

### 3.1 全景表(症状 × 出现次数 × 本次关联 × 闭环状态)

| # | 症状类型 | 历史命中次数 | 本次关联度 | 闭环状态 |
|---|---------|------------|----------|---------|
| 1 | silent-success / 假闭环 / `pass_with_residuals` 误提升 | **8/15** | 🔴 极高 | 未落地(2026-07-04-002 P0-1/P0-2/P0-3、2026-07-04-004 U1-U9 全 planned) |
| 2 | `stall_recovery` + `handoff_dispatch_timeout` 升级到 `recovery_exhausted` | **13/15** | 🔴 极高 | 未落地(2026-07-03-005 C2+C8 已修 preset 白名单;**drift-engine 重写路径仍漏过**) |
| 3 | `recovery_exhausted:stall_recovery:coordinator:task_resume:handoff_dispatch_timeout` 走 shipper `pass` 路径 | **3+** | 🔴 极高 | 部分闭环(shipper_reason.rs:64-77 prefix allowlist 已列入该 prefix;**但 shipper 读不到 plan.blocked 上下文时 routing gate 空转仍默认 pass**) |
| 4 | `plan.blocked(reason=review_in_progress_findings_pending)` 走 shipper `pass` 路径 | **0/15**(30 天全仓 0 命中) | 🟢 新 reason | 新症状模式,但症状同源(M-3 + shipper routing gate 空转) |
| 5 | silent-success 缺 findings.md / fix-plan.md 但 shipper 仍 pass | **2/15**(115242、153532 显式) | 🔴 极高 | 未落地(mechanism 主因:event_loop/mod.rs:5583-5594) |
| 6 | 仅 3 维 findings(缺 maintainability/project-standards/adversarial)但 shipper 仍 pass | **3/15** | 🔴 极高 | 未落地(2026-07-04-004 U1 + U3 + U5 planned) |
| 7 | handoff_dispatch_timeout 触发 escalation 链 | **11/15** | 🔴 极高 | 未落地(2026-07-04-002 R5 U16 active) |
| 8 | duplicate_work_done 风暴 | 9/15 | 🟠 高 | 部分闭环 |
| 9 | `task.resume` 路由错配(safe_target vs trusted emit target) | 12/15 | 🔴 极高 | 未落地(2026-07-04-002 R5 U16 active) |

**闭环状态分布**:🔴 极高关联且未落地 = 6 项;🟠 高关联且部分闭环 = 2 项;🟢 新症状 = 1 项。

### 3.2 关键历史诊断报告(按关联度排序)

| # | 报告路径 | 关联度 | 关键交叉点 |
|---|---------|--------|-----------|
| 1 | `docs/report/2026-07-06-ce-executor-serial-primary-20260705-153532-diagnosis.md` | 🔴 **极高**(昨日姊妹 run) | DEV-002 silent-success 主根因(conf=85);DEV-003 shipper 非白名单提升(conf=80);DEV-004 handoff 600s |
| 2 | `docs/report/2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md` | 🔴 **极高** | 5/6 维 review 走通 + adversarial 失败 → shipper 走 recoverable → pass_with_residuals |
| 3 | `docs/report/2026-07-04-ce-executor-serial-primary-20260704-024019-diagnosis.md` | 🔴 **极高** | 半假闭环 3/6 review;dedup 风暴同型;M-2 同根 |
| 4 | `docs/report/2026-07-03-ce-executor-serial-primary-20260703-130118-diagnosis.md` | 🔴 **极高** | O-1 isolated budget 截断 6 维串行 ready 5/6 silent drop + M-2 handoff dispatch + shipper pass_with_residuals;显式记录"30 天 12 次同根复发" |

### 3.3 根因分类对照(历史已固化)

**机制层**:

| 根因 ID | 文件:行 | 历史命中 | 本次是否相关 |
|---------|---------|----------|--------------|
| **M-1** isolated budget 对 `declared_serial_walk` hat 不让步 | event_loop/mod.rs:7857, 8520, 8537, 8542;8980-9011 | 130118 O-1、024019、153532 DEV-001 | **是**(本次 review_walk 整体跳过) |
| **M-2** handoff_dispatch 不校验 `consumer.triggers.contains(topic)` | event_loop/mod.rs:7029-7132;diagnosis/responder.rs:1046, 1588, 1605, 1633, 1708 | 130118、151220、115242 等 11 份 | **是**(本次 600s handoff 悬空) |
| **M-3** `RecoveryAction::ForcePlanBlocked` 不调 `record_event` | event_loop/mod.rs:5583-5594 | 153532 DEV-002 主根因(conf=85) | **是**(本次 run 几乎肯定命中同一单点) |
| **M-4** shipper `is_recoverable_plan_blocked_reason` `starts_with("recovery_exhausted:")` 通道漏过 | shipper_reason.rs:52-58, 86-97;65-77 prefix allowlist | 075227-M-2、130118-M-4、024019 P0-3 | **是**(本次 run 该 reason 在 prefix allowlist 内,但若 shipper 读不到 events.jsonl 上下文,routing gate 空转仍默认 pass) |

**编排层**:

| 根因 ID | 来源 | 历史命中 | 本次是否相关 |
|---------|------|----------|--------------|
| **O-1** review-coordinator "one dimension per turn" 串行约定 vs 机制 budget 冲突 | presets/en/ce-executor-serial.yml:16, 99-114, 152-162 | 130118 O-1 | **是** |
| **O-2** review-synthesizer 1/6 失败 → verdict=blocked 误升 | preset review-synthesizer 段 | 130118、115242 | (潜在) |
| **O-3** review-coordinator `review.dimensions.complete` 把 4 维伪造成 `status: done, findings_file: null` | review-coordinator hat | 130118 O-3 | (潜在) |
| **O-5** shipper 把 `recovery_exhausted:stall_recovery:*` 译为 `pass_with_residuals` | shipper_reason.rs + presets/schemas/ce-executor-serial.yml:351-353 | 130118 O-5 | **是** |

### 3.4 shipper recoverable whitelist 起源

- 2026-07-04 P0-3 落地 8 项 prefix allowlist(shipper_reason.rs:65-77),把 `recovery_exhausted:stall_recovery:coordinator:task_resume:handoff_dispatch_timeout` 显式列入
- 2026-07-06 U3 (commit 6c01bac8) 移除 RECOVERABLE_REASONS 末项 **bare `recovery_exhausted`** 字面短路,改为 fail-close 默认
- 本次 run 命中 reason 在 prefix allowlist 内 → 走 recoverable → pass 翻译

### 3.5 已有修复计划落地状态

| 计划 | 状态 | 与本次 run 关联 |
|------|------|----------------|
| `docs/plans/2026-07-04-004-fix-ce-executor-serial-silent-success-p0-p1-plan.md` | **planned**(U1-U9 全 planned,**未合并**) | 🔴 本次 run 跑在该 plan 落地之前 |
| `docs/plans/2026-07-04-002-fix-opac-p0-p1-p2-issues-plan.md` | active | U7 / U13 / U16 对应本次 run handoff 悬空 + review_walk 跳过 |
| `docs/plans/2026-07-04-001-feat-opac-isolated-agent-discipline-plan.md` | active | U1-U26 已 commit |
| `docs/plans/2026-07-04-003-fix-task-cli-acl-coordinator-hats-and-preset-narrowing-plan.md` | planned | task ownership ACL 修复;U7 强依赖 002 plan U7 |
| `docs/achieved/plan/2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md` | achieved | U7 shipper whitelist 起源 |
| `docs/achieved/plan/2026-06-29-007-fix-ce-executor-serial-mechanism-p0-p1-plan.md` | achieved | 早一批 P0/P1 机制修复 |

### 3.6 一句话历史结论

**本次 run `primary-20260705-224028` 是 silent-success 主根因(M-3 `ForcePlanBlocked` 不调 `record_event`)的 9 次同簇复发之一;新 reason 关键词 `review_in_progress_findings_pending` 是该机制缺陷在不同 review 阶段的另一 reason 变体——症状模式历史 30 天 8 次复发同根**。

---

## 4. 证据清单(Agent C 输出,摘要)

完整清单见 `.ralph/diagnostics/2026-07-06T06-40-27/agent-brief/deviation-evidence.md`。

### 4.1 DEV 偏离证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 证据缺口 |
|----|------|----------|------------|------------|----------|
| **DEV-001** | `plan.blocked` reason 三处字面不一致(events L17 / workspace recovery L2 / shipper L18) | events L17 / workspace recovery L2 / shipper_reason.rs:64-77 / preset L2822-2862 | C1(机制层疑似 bug) | H | 缺 shipper hat prompt 现场(MINIMAL 无 orchestration) |
| **DEV-002** | `review_in_progress_findings_pending` 路由(30 天全仓 0 命中新 reason) | workspace recovery L2 payload_preview | C1(同 DEV-001,疑似同一静默重写通道) | M | 同上 |
| **DEV-003** | Tier C 实际仅 3 维 findings(缺 maintainability/project-standards/adversarial)且无 findings.md / fix-plan.md,shipper 仍 emit REVIEW_COMPLETE(pass, 7) | Tier C 文件系统硬证据 + review-sequence.json L11-13 + events L18 + preset L2757 | C0(架构性) | H | shipper hat prompt 原文(MINIMAL) |
| **DEV-004** | `.ralph/loops.json` 终止时 `{"loops": []}` 是否 by-design 不可定论 | loops.json 17 字节 | C3(工件缺陷) | M | 缺 last-modified 时间戳 |
| **DEV-005** | workspace recovery 仅 2 条 Info;session recovery 三联字面不全 | workspace recovery L1-L2 / session recovery L1-L3 / events L17 | C1 | H | workspace 端缺 iter 字段,无法直接 join |
| **DEV-006** | tasks.jsonl 3 行:L1/L3 共用 task_id 但 task_key 不同;L2/L3 缺 owner_hat_id | tasks.jsonl L1-L3 / events L2/L5 / HARD RULE 10 | C2(契约漂移) | H | 缺 schema parity check |
| **DEV-007** | `.ralph/agent/progress.md` 仅 1 step 但 events 跑 2 unit;shipper L2785 解析冲突 | progress.md / events L2/L5 / preset L2785 | C3(工件缺陷) | H | shipper 实际是否读 progress.md(MINIMAL) |
| **DEV-008** | `.ralph/agent/scratchpad.md` 0 字节 vs preset L812 旧路径,契约漂移 | scratchpad.md / preset L280-282 / L812 | C2 | M | coordinator hat prompt 注入现场 |

### 4.2 OPAC 表(MINIMAL 模式)

| OPAC ID | 名称 | 置信度 | 备注 |
|---|---|---|---|
| OPAC-U0 | CLI session ownership wiring | 高 | session_id=`2026-07-06T06-40-27` |
| OPAC-U1 | Step-deny / step-gate 校验 | 高 | events 三字段齐全;tasks.jsonl 触发 step-gate 漂移 |
| OPAC-U2 | Hat 隔离 prompt 注入 | 低 | MINIMAL 不可判 |
| OPAC-U3 | isolated 终态事件无 prefix | 高 | L17→L18→L19→L20 链无业务事件夹带 |
| OPAC-U4 | Fair scheduling | 低 | MINIMAL |
| OPAC-U5 | Drift engine emit cadence | 高 | history.jsonl 覆盖区间完整 |
| OPAC-U6 | Step-handoff 推进 | 高 | events 投影侧正常 |
| OPAC-U7 | EventOriginGuard 命中 | 高 | session recovery L2 reason_code 不在 events 出现(phantom) |
| OPAC-U8 | IdempotentLog final_records | 中 | recovery_count=0, drift_finding_count=0 |
| OPAC-U9 | Lock release 顺序 | 高 | OK;loop.lock 已释放 |
| OPAC-U10 | event_policy schema 拒收 | 中 | events L1-L20 全部成功落盘;shipper payload 无拒收 |
| OPAC-U11 | exempt_topics 命中 | 高 | coordinator.exempt_topics 含 `plan.blocked` |
| OPAC-U12 | state_projection 推进 | 高 | iter 1→18 与 events 同步 |
| OPAC-U13 | preset_lint finding_id 一致性 | 低 | 待主线核查 |
| OPAC-U14 | repair budget escalation | 高 | session L2 outcome=escalated |
| OPAC-U15 | topic_deny_rules | 高 | 无 deny 触发 |
| OPAC-U16 | handoff_dispatch_timeout 注入 payload_contract | 高 | payload_contract 字段齐全 |
| OPAC-U17-U26 | 全部禁用 | 低 | MINIMAL |

### 4.3 R1-R6 Agent Output Governance 矩阵

| 规则 | 名称 | 判定 | 证据 | 关联 DEV |
|---|---|---|---|---|
| R1 | 终态事件前不夹带业务事件 | **PASS** | events L17→L18→L19→L20 链无业务事件夹带 | DEV-003(辅助) |
| R2 | hat 不读 ledger | **INDETERMINATE** | MINIMAL 无 prompt 现场 | DEV-008 |
| R3 | emitter hat 必须 `--policy-check` 强预检 | **INDETERMINATE** | MINIMAL | DEV-001(辅助) |
| **R4** | **task 三字段(task_id/task_key/step)必须同源** | **FAIL** | tasks.jsonl L3 缺 owner_hat_id;L1 与 L3 共用 task_id 但 task_key 不同 | **DEV-006** |
| R5 | isolated 单事件预算 | **PASS** | events 无相邻两次 emit 在同一 hat 内 | DEV-001 |
| **R6** | **final_findings_count 与 findings.md 必读对齐** | **FAIL** | shipper L2757 必读 findings.md 但 findings.md 不存在 | **DEV-003** |

### 4.4 三联对账表(关键 5 行)

| # | topic | events 字面 | workspace recovery 字面 | session recovery 字面 | 一致性 |
|---|-------|------------|-----------------------|----------------------|---------|
| 7 | task.resume | events 全文无(phantom) | 无 | L2 evidence 字段字面含 `topic=task.resume` | **字面不一致 / phantom** |
| 8 | plan.blocked | L17 `reason=loop_stalled_max_iterations` | L2 `payload_preview.reason=review_in_progress_findings_pending:...` | 无 | **字面不一致** |
| 12 | handoff_dispatch_timeout | events 无(phantom) | 无 | L2 reason_code=`handoff_dispatch_timeout`, outcome=escalated | **phantom** |
| 13 | recovery_outcome_update | events 无 | 无 | L3 outcome=pending, 无闭合 | **缺闭合** |

---

## 5. 问题归因表(confidence ≥ 60;P0 ≥ 70)

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|----------|----------|
| **P0-1** | runtime-recovery `ForcePlanBlocked` 不调 `record_event` 导致 silent-success 假闭环(M-3,主根因) | mechanism | **85** | DEV-001+002+005 复合 | 极高(153532/130118/115242/024019 等 9 次同根) | 1→85 |
| **P0-2** | review_walk 跳过(只走 3/6 维),`findings.md`/`fix-plan.md` 不存在,shipper 仍 emit REVIEW_COMPLETE(pass)(M-1 主因 + O-1 编排协同) | compound(M-1 70% + O-1 30%) | **82**(min) | DEV-003 | 极高(024019/115242/153532) | 1→82 |
| **P0-3** | `recovery_exhausted:stall_recovery:coordinator:task_resume:handoff_dispatch_timeout` 在 prefix allowlist 但 shipper 实际接收 reason 字面 ≠ events 字面(M-3 衍生,词法独立可证) | mechanism | **78** | DEV-001 子链 | 极高(075227-M-2/130118-M-4/024019 P0-3) | 1→78 |
| **P0-4** | handoff_dispatch_timeout phantom topic,workspace/session ledger 写入路径不收敛(M-2 主因) | mechanism | **80** | DEV-005 | 极高(11/15 历史报告) | 1→80 |
| **P1-1** | tasks.jsonl 三字段自相矛盾(task_id 复用 + owner_hat_id 缺失),schema 未在 allowed_values 强制三字段一致(R4 FAIL + 契约漂移) | compound(preset + schema) | **60**(MINIMAL 上限) | DEV-006 | 中 | 1→60 |

**compound P0-2 成分明细**:M-1 mechanism = **82**(`event_loop/mod.rs:8980-9011` exempt_topics 仍占 non_wave_business_event_accepted slot;fix commit 6c01bac8 DEV-001 描述精确吻合);O-1 preset = **78**(`presets/en/ce-executor-serial.yml:16, 99-114, 152-162` 6 维串行声明);整行 = **min(82, 78) = 82**;贡献比例 M-1 70% + O-1 30%(exempt_topics/budget 卡死是机制必要条件,preset 6 维串行是触发条件)。

**P0-1 加深轮次记录**:
- 第 1 轮:补读 workspace+session recovery.jsonl、ledger.jsonl、preset L2822-2862 whitelist 注释、shipper_reason.rs:64-77 prefix allowlist → 置信度从初估 78 → 85(双账本 + file:line + 历史同根 + fix commit 双重锁定)。

---

## 6. 修复建议(仅针对 §5 入表项)

### 6.1 短期(operator workaround)

无新增 operator workaround。本次 run 已自然终止(`LOOP_COMPLETE`),且 fix commit `6c01bac8` 已合并(2026-07-06 23:14:13Z),后续 runs 应自然受益。

### 6.2 中期(preset / schema / instructions)

**修复 #1:补 shipper 「必读 findings.md」 schema 层硬约束**(覆盖 §5.P0-2)

- **目标**:阻止 shipper 在 `findings.md` 不存在时仍 emit REVIEW_COMPLETE(pass)
- **改动**:在 `presets/schemas/ce-executor-serial.yml:325-359` (REVIEW_COMPLETE schema 防线) 加 required_field `findings_file_read: ".agents/scratchpad/ce-executor/{plan_name}/findings.md"`;shipper 路径(`crates/ralph-core/src/shipper_reason.rs`)在 emit REVIEW_COMPLETE 前先 stat 该文件,缺失时 emit `REVIEW_COMPLETE(fail, reason=findings_md_missing)` 或 throw
- **预期效果**:review_walk 跳过 + 缺 findings.md → 不再走 silent pass,而是显式 fail
- **关联置信度**:**82**(P0-2)
- **owner**:operator

**修复 #2:task 三字段 parity check 入 schema**(覆盖 §5.P1-1)

- **目标**:tasks.jsonl `task_id` 唯一性 + task_key 中 `:step-XX:` 段与 `step` 字段一致 + closed task 上 `owner_hat_id` 必填
- **改动**:`docs/plans/2026-07-04-003-fix-task-cli-acl-coordinator-hats-and-preset-narrowing-plan.md` 已在 planned 状态,U1-U5 实施时同时落实此 parity check
- **预期效果**:R4 FAIL 不再发生;task_id 复用会直接被 CLI 拒收
- **关联置信度**:**60**(P1-1)
- **owner**:operator

### 6.3 长期(机制 / 底座)

**修复 #3:验证 M-3 fix `6c01bac8` 真正在 2026-07-06 之后的 run 路径生效**(覆盖 §5.P0-1 + §5.P0-3)

- **目标**:确认 fix commit `6c01bac8` 在后续 `primary-*` runs 中真正生效(force_plan_blocked 现在调 `record_event`,plan.blocked 进 events.jsonl)
- **改动**:
  1. 复核 2026-07-06 23:14:13Z 之后的所有 `primary-*` runs 的 events.jsonl:若有 `plan.blocked(reason=recovery_exhausted:*)` 则必须有字面记录
  2. 若仍缺失,fix 未生效,需补 derived U2 标识(在 event_loop/mod.rs:5583-5594 加 force_record_event guard)
  3. **警惕**:该 fix 仅解决"不写盘",并未解决"stale plan.blocked reason 仍走 shipper pass 翻译"——需配合 shipper fail-close(已在 6c01bac8 DEV-003 移除 bare `recovery_exhausted` 字面短路)。两条 fix 在同一 commit,shipper fail-close 的 prefix allowlist 是否过紧(2026-07-04-024019 落地 8 项)仍需观察后续 runs
- **预期效果**:events.jsonl 字面 reason 与 shipper reason 输入对齐,M-3 主根因消除;P0-1/P0-3 自动消失
- **关联置信度**:**85**(P0-1)+ **78**(P0-3)
- **owner**:机制主线

**修复 #4:推动 U16 `validate_resume_routing` 决策落地**(覆盖 §5.P0-4)

- **目标**:handoff_dispatch 不再 600s 悬空
- **改动**:
  1. 在 `crates/ralph-core/src/event_loop/mod.rs:7029-7132` 中加 `consumer.triggers.contains(topic)` 校验;不匹配时**不静默 handoff**,而是 emit `task.resume.misrouted` 警告 + bail(而不是 600s 阻塞)
  2. 落地后**必须**补 BDD 场景覆盖(避免静默覆盖):在 `crates/ralph-core/tests/scenarios/` 增加 `ce_executor_serial_handoff_misrouted.yml`,断言:`handoff 到不含 target topic 的 hat → emit misrouted 警告 + 不调用 publish`(非 600s 阻塞)
- **预期效果**:workspace/session ledger 写入路径收敛;M-2 600s 悬空不再发生
- **关联置信度**:**80**(P0-4)
- **owner**:机制主线(`docs/plans/2026-07-04-002-fix-opac-p0-p1-p2-issues-plan.md` U16)

---

## 7. 未核实疑点(confidence < 60 或 MINIMAL 不可判,无法驱动修复建议)

| # | 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---------|------------|------------|----------|
| **U1 / DEV-001 子链** | shipper 实际接收的 reason 字面值与 events 字面值不一致;shipper prompt 输入是否经过 drift-engine 二次重写? | — | shipper hat prompt injection 现场数据 + orchestration.jsonl 投影 | 走 FULL 模式可验 |
| **U2 / DEV-002 子链** | `review_in_progress_findings_pending` reason 由哪条 code path 产生?(grep 全仓 0 命中) | — | workspace recovery L2 payload_preview 是 Info sink 副本,非 policy 层拒收证据 | grep `recovery_runtime` / `runtime-recovery / dispatch / policy.findings_pending` 找 emit 路径;若仍 0 命中 → 推断为 M-3 同症状的 reason 变体(同根) |
| **U3 / DEV-008** | coordinator hat 实际读哪条 scratchpad 路径(`.ralph/agent/scratchpad.md` 0 字节 vs `.agents/scratchpad/...`)? | — | hat prompt injection 现场 + RALPH_CURRENT_LOOP_ID 环境变量观测 | 补 hat prompt 文本 |
| **U4 / DEV-007** | shipper 是否真读 progress.md 计 total_units? | — | shipper hat prompt 原文 + L18 payload 是否含 total_units 字段(本次不含) | 补 shipper prompt 注入 |
| **U5 / DEV-004** | `loops.json` 终止时清空是否 by-design? | 50 | `loops.json` last-modified 时间戳 + git history | 查 git history |
| **U6** | 本次 run 23:08:19Z 终止 vs fix commit 6c01bac8 23:14:13Z(+34 min 时间窗)是否在生产环境做了 hot-patch 重启? | — | 部署日志 | 运维侧日志(超出源码追溯范围) |
| **U7 / events L17 字面 reason** | events L17 字面是 `loop_stalled_max_iterations`,而非 `recovery_exhausted:stall_recovery:coordinator:task_resume:handoff_dispatch_timeout`;是否说明"实际触发 plan.blocked 的不是 stall_recovery 升级,而是另一种 loop_stalled_max_iterations 内层路径"? | — | 需读 `loop_runner` 的 stall 升级触发条件源码 + recovery_runtime `dispatch` 实现 | grep `loop_stalled_max_iterations` emit site + `recovery_runtime::dispatch` 实现 |

---

## 提交前 checklist 校对

- [x] Phase 0 盘点表在报告中(§0)
- [x] 只读了 `current-events` 指向的 events(`events-20260705-224028.jsonl`,未通配)
- [x] LOGS_ONLY 未因缺 orchestration 标 P0(本次是 MINIMAL,类似处理)
- [x] 每条 P0/P1 在 §5 有 **置信度**;P0≥70、入表≥60(详见 §5)
- [x] confidence<60 的候选已加深或落入 §7(P2-1 conf=42 / P2-2 conf=50 已落入 §7)
- [x] 未引用 ssot-guardrails 禁止项(未引用 hat_handoff / loop_state_snapshot.json / human.guidance 等)
- [x] 报告在主仓 `docs/report/`(`docs/report/2026-07-06-ce-executor-serial-primary-20260705-224028-diagnosis.md`)
- [x] mermaid 已通过 mcp `mermaid_validator validateMermaidPreview` 校验(详见 §2.3 标注)

---

**报告路径**:`docs/report/2026-07-06-ce-executor-serial-primary-20260705-224028-diagnosis.md`
**对照姊妹 run**:[`docs/report/2026-07-06-ce-executor-serial-primary-20260705-153532-diagnosis.md`](2026-07-06-ce-executor-serial-primary-20260705-153532-diagnosis.md)(昨日同一 workspace/plan/preset 姊妹 run,本报告判定为同根 9 次复发中的第 8 次)
**修复 commit 锚点**:`6c01bac8 fix(orchestrator): 修复 silent-success 假闭环机制层缺陷`(本次 run 之后 34 min 合并)