# 2026-07-06 ce-executor-serial 运行链路诊断报告

> **run**: `ce-executor-serial` primary-20260705-153532
> **preset**: `presets/en/ce-executor-serial.yml`
> **plan**: `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`（2 单元 plan）
> **中间产物**: `/Users/pittcat/Dev/Rust/ralph-e2e-serial/.ralph/`
> **诊断日期**: 2026-07-06
> **诊断模式**: MINIMAL（无 agent-output，无 orchestration.jsonl）

---

## 0. 产物盘点（Phase 0）

| Tier | 路径 | 存在 | 行数 | 备注 |
|------|------|------|------|------|
| S | `.ralph/current-events` | ✓ | 1 行 | 指向 `events-20260705-153532.jsonl` |
| S | `events-20260705-153532.jsonl`（trusted，唯一可信） | ✓ | **13** | 编排拓扑 SSOT |
| S | `events-history-20260705-153532.jsonl` | ✓ | 2 | warmup work.start + loop.terminate |
| S | `.ralph/ledger.jsonl` | ✓ | 18 | iter5 seq=8/9 duplicate_work_done ×2 |
| S | `.ralph/recovery.jsonl`（workspace RepairStream） | ✓ | 3 | work.ready×2 + **plan.complete×1** |
| S | `.ralph/loops.json` / `current-loop-id` | ✓ | - | loop_id=`primary-20260705-153532` |
| S | `.ralph/loop.lock` | ✓ | - | **lock 仍持**（异常结束但 summary 已写） |
| S | `.ralph/diagnostics/logs/ralph-*.log` | ✓ | 92 | log-783 第 61-62 行含 runtime-recovery 关键 log |
| A | `.ralph/agent/tasks.jsonl` | ✓ | 4 | 2 个 task，started=closed 同毫秒 |
| A | `.ralph/agent/progress.md` | ✓ | 8 | Current Step=step-02, Completed=[step-01,step-02] |
| A | `.ralph/agent/summary.md` | ✓ | - | "Completed successfully" verdict=pass_with_residuals |
| A | `.ralph/agent/handoff.md` | ✓ | - | 含 commit cadaae9 + 663e8a1 |
| B | `.ralph/diagnostics/2026-07-05T23-35-31/`（session） | ✓ | - | 含 drift.jsonl(0)、recovery.jsonl(5)、trace.jsonl、diagnosis-summary.json、active-activations.json |
| B | `.ralph/diagnostics/2026-07-05T23-35-31/orchestration.jsonl` | ✗ | 0 | **缺失** → diagnostics=MINIMAL 而非 FULL |
| B | `.ralph/diagnostics/agent_doc_sync.json` | ✓ | - | 旁路 |
| C | `ralph.yml` / `ralph.serial.yml` | ✓ | - | coordinator_hats=[coordinator, progress-steward] |
| C | `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md` | ✓ | - | 2 单元 |
| C | `docs/report/2026-07-05-ce-executor-...-report.md` | ✓ | - | final-report.md |

**盲区 / 根因置信度硬顶（MINIMAL 模式封顶）**：
- agent 归因 ≤60；mechanism 根因 ≤85；OPAC 单项 ≤60
- 缺 agent-output（看不到 hat 内 agent 是否曾尝试 emit review.dimension.ready 等）
- 缺 orchestration.jsonl（看不到 hat activation 的精确时序）

---

## 1. 结论摘要

### 1.1 健康度

- **判定**:**假闭环 silent-success（P0）**。loop 自报 `LOOP_COMPLETE, exit code 0`，`summary.md` 标 "Completed successfully" verdict=pass_with_residuals；但 **review_walk phase 整体跳过**（0 个 dimension-reviewer / 0 个 review.dimension.ready / 0 个 review.complete）、`plan.complete` 仅在 workspace `recovery.jsonl` 而**不在 trusted events.jsonl**、`work.failed` 事件已发出但 shipper 仍把 `REVIEW_COMPLETE` 提升为 pass。
- **关键异常**:P0 × 3 / P1 × 6 / P2 × 3（均为 confidence≥入表门槛）
- **最高根因置信度**:P0-1（silent-success 主根因）= **85** / 100（mechanism 单点缺陷）
- **历史复发**:是 — 与 024019/130118/115242/093813/075227 等同模式，**30 天同根 ≥5 次**（详见 §3）。本次 run 跑在 `docs/plans/2026-07-04-004-fix-ce-executor-serial-silent-success-p0-p1-plan.md`（**status: planned**）落地之前。
- **代码本身**:健康 — 2 个 step 全 work.done（commit 37c2b2d、cadaae9）、19 个 test 全部通过（8 unit + 11 integration）。问题**不在工程产出**，在**编排链路和机制缺失**。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ❌ 偏离 | 13 个 trusted events 中 6 个有偏差（46%）；test.passed 出现在 REVIEW_COMPLETE 之后 | **70** |
| Q2 | 基座机制是否正常生效？ | ❌ 偏离 | `runtime-recovery ForcePlanBlocked` 路径（`event_loop/mod.rs:5583-5594`）仅 `bus.publish` 不调 `record_event` → plan.blocked 不进 events.jsonl | **85** |
| Q3 | 编排是否合理、正常运行？ | ❌ 偏离 | review_coordinator/dimension_reviewer/review_synthesizer 全部 0 激活；shipper 把 stall_recovery 翻译为 pass_with_residuals | **80** |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | mechanism 主因（≥70%）+ preset 配合不足（20%）+ agent 弱因（≤10%） | 1 处机制缺陷引发 3 条 P0/P1（DEV-002/003/004 共因） | **85**（取 §5 主因最高值） |

### 1.3 根因一句话

**`event_loop/mod.rs:5583-5594` `RecoveryAction::ForcePlanBlocked` 分支只调 `self.bus.publish` 不调 `self.state.record_event`，导致 runtime-recovery 注入的 plan.blocked 永不写入 trusted events.jsonl —— 后续 shipper 看不到 plan.blocked 上下文，结合 `shipper_reason.rs:31-39` bare `recovery_exhausted` 字面短路白名单（L88 在 prefix allowlist 之前），最终 REVIEW_COMPLETE 被错误提升为 pass_with_residuals，形成 silent-success 假闭环（confidence 85）**。

---

## 2. 执行链路对比图（Agent A 输出）

### 2.1 拓扑激活表（10 hat + precheck desugar = effective 12）

| Hat | 预期激活场景 | 实际激活次数 | 备注 |
|---|---|---|---|
| `coordinator` | work.start/test.passed/review.complete/work.failed | 3 emit + 1 task.resume 接收 | iter5→iter9 跳到 review.start，未走 fix-unit 链 |
| `executor` | work.ready → TDD → work.done/work.failed | 3（work.done ×2、work.failed ×1） | step-01 work.failed 1 次、step-02 work.done 重复 2 次 |
| `validator` | work.done/fix.applied → test.passed/test.failed | **1（test.passed @15:53）** | step-01 work.done 后 validator 600s 未激活（iter8 handoff_dispatch_timeout） |
| `fixer` | test.failed → fix.applied/fix.exhausted | 0 | 未触发 |
| `review-coordinator` | review.start → 6 维串行 → review.dimensions.complete | **⏸️ 0** | review.start 之后 0 产出，进程从未 spawn |
| `dimension-reviewer` × 6 | review.dimension.ready → review.dimension.done | **⏸️ 0 × 6** | goal-alignment/correctness/testing/maintainability/project-standards/adversarial 全缺 |
| `review-synthesizer` | review.dimensions.complete → review.complete | **⏸️ 0** | hat 未激活 |
| `shipper` | plan.complete/plan.blocked → REVIEW_COMPLETE | **1（@15:52 提前 emit）** | 触发源不是 trusted events 的 plan.complete/plan.blocked，而是 runtime-recovery 注入 + RepairStream 注入 |
| `reporter` | REVIEW_COMPLETE → report.done → LOOP_COMPLETE | 2 | 顺序正确 |
| `progress-steward` | loop.stalled → task.resume / plan.blocked | 1（task.resume @15:49，source_hat=ralph） | trusted events 中无 progress-steward 直接 emit |
| `precheck-review.complete` | review.complete → gate → synthesizer | 0 | 无上游 review.complete |
| `precheck-plan.complete` | plan.complete → gate → coordinator | 0 | 无上游 trusted plan.complete |

### 2.2 时间轴对比表

| # | 时间 | 实际事件(hat) | 预期事件 | 标记 | 说明 |
|---|------|----------------|----------|------|------|
| 1 | 15:35:32 | work.start(loop-bootstrap) | coordinator 接收 | ✅ | starting_event |
| 2 | 15:37:05 | work.ready(coordinator) step-01 | coordinator→executor | ✅ | 6 字段齐 |
| 3 | 15:40:01 | work.done(executor) step-01 | executor→validator | ⚠️ | validator 600s 未激活（iter8 handoff_dispatch_timeout） |
| 4 | 15:41:27 | **work.failed(executor) step-01** reason="executor cannot close task owned by coordinator" | coordinator→**plan.blocked**（per preset L1077-1081） | ❌ | **未发 plan.blocked**，直接 step-02 |
| 5 | 15:43:37 | work.ready(coordinator) step-02 | 前置 step close 后 | ❌ | step-01 任务未 close |
| 6 | 15:46:14 | work.done(executor) step-02 | executor→validator | ✅ | 7 字段齐 |
| 7 | 15:47:41 | work.done(executor) step-02 重复 | 一次性 emit | ⚠️ | ledger iter5 2× duplicate_work_done rejection |
| 8 | 15:49:35 | task.resume(ralph) target=coordinator | coordinator→close coordinator-owned task | ⚠️ | safe_target=validator 但 trusted emit target=coordinator（错配） |
| 9 | 15:50:42 | **review.start(coordinator)** | on_test_passed_step primitive（preset L182-186） | ❌ | **trusted events 无 test.passed 前置** → primitive 跳过 |
| 10 | 15:50:43–15:52:20 | （无 review.* 事件） | review-coordinator→6 维→review-synthesizer→review.complete | ❌ | **review_walk 整体跳过** |
| 11 | 15:52:21 | **REVIEW_COMPLETE(shipper)** verdict=pass_with_residuals | review.complete→coordinator→plan.complete→shipper | ⚠️ | residual reason 含 `recovery_exhausted:stall_recovery:validator:work_done:handoff_dispatch_timeout:*`，但 shipper 提升 pass |
| 12 | 15:53:50 | **test.passed(validator) step-02** 19/19 | test.passed 早于 review.start | ⚠️ | **时序倒置**（在 REVIEW_COMPLETE 后 89s） |
| 13 | 15:54:57 | report.done(reporter) | reporter→report.done | ✅ | 2 字段齐 |
| 14 | 15:55:04 | LOOP_COMPLETE(reporter) reason="All steps completed successfully" | reporter→LOOP_COMPLETE | ✅ | summary 标 Completed successfully, exit 0 |

**schema required_fields 全部通过**（Agent A 验证 13/13 业务事件 schema 完整）—— **schema 不是漏点，问题在事件拓扑 / 时序 / 触发源**。

### 2.3 链路偏离摘要（按严重度降序）

- ❌ **review_walk 阶段整体跳过**（DEV-001 / DEV-010）：6 维 review-coordinator→dimension-reviewer→review-synthesizer→review.complete 全部 0 激活
- ❌ **silent-success 终态**（DEV-002）：plan.complete 仅在 RepairStream，不在 trusted events；shipper 把 stall_recovery 翻译为 pass
- ❌ **work.failed(step-01) 后未发 plan.blocked**（DEV-009）：coordinator 直接 work.ready(step-02)，跳过了 stall_recovery 链
- ❌ **task.resume 路由错配**（DEV-004 升级链偏差）：envelope safe_target=validator 但 trusted emit target=coordinator
- ⚠️ **test.passed 时序倒置**（DEV-007）：@15:53 晚于 REVIEW_COMPLETE @15:52 89s
- ⚠️ **duplicate_work_done**（DEV-006）：step-02 两次同 payload，ledger 2× rejection 但 trusted 双行
- ⚠️ **观测矛盾**（DEV-008）：diagnosis-summary recovery_count=0 vs session recovery 5 条 envelope
- ⚠️ **task ownership 死锁**（DEV-005）：executor 越权 close coordinator-owned task → TaskNotTerminal

### 2.4 终止类型 + 未触发 hat 清单

**终止**:`LOOP_COMPLETE` @15:55:04, summary "Completed successfully", exit 0 — 看似正常退出，但**实际是 repair-stream stall_recovery 路径产出的 shipper REVIEW_COMPLETE，不是 phase_authority engine 推动的 unit_loop → review → fix_units → plan_end → ship → terminal 完整链路**。trusted events 主链存在 4 个 phase 转换漏洞：
1. work.failed 无 plan.blocked
2. review.start 无前置 test.passed
3. review.* 全缺（review_walk 整体跳过）
4. plan.complete 缺位（仅在 RepairStream）

---

## 3. 历史问题上下文（Agent B 输出）

### 3.1 历史问题全景表（15 份诊断报告）

| 问题类型 | 命中文档数 | 关联本次症状# | 闭环状态 | 关联度 |
|---|---|---|---|---|
| **review_walk 整体跳过 / 6 维未跑全** | 12/15（130118×28、020135×28、024019×21、115242×18、140149×16、112002×30、175407×34、032648×13 等） | #1 | 复发中（同簇 5+ 次） | 🔴 极高 |
| **stall_recovery / handoff_dispatch_timeout** | 13/15 | #4/#7 | **未落地**（shipper 兜底未修） | 🔴 极高 |
| **TaskNotTerminal / task ownership / owner_hat_id** | 4/15 | #2 | 未落地（151220 P0-A、execution_contract.rs:867-906 未同步） | 🟠 高 |
| **silent-success / 假完成 / pass_with_residuals** | 8/15（115242、130118、140149、151220、024019、140433、112002、020135） | #6 | **未落地**（2026-07-04-002 P0-1/P0-2/P0-3 未闭合） | 🔴 极高 |
| **duplicate_work_done** | 9/15 | #3 | 部分闭环（perky-maple 06-18 U0-U6 修复 fix_round dedup；reason_code 归一未修） | 🟠 高 |
| **事件顺序倒置** | 5/15（140433、170451、130118、075227、093813） | #5 | 复发中（mechanism-close-loop 2026-06-23 三道防线已 commit 但 100% 未覆盖） | 🟡 中 |
| **task.resume 死信 / 无消费者 / 路由错位** | 12/15 | #7 | **未落地**（U16 task.resume 路由校验被绕过） | 🔴 极高 |

### 3.2 高关联历史条目详情

1. **`docs/report/2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md`**（最相近一次）:5/6 维 review 走通 + adversarial 失败 → review-synthesizer 误升 verdict=blocked + coordinator 把 findings_count=0 误路由到 plan.blocked → shipper recoverable_whitelist 兜底为 pass_with_residuals。本次 run 与 115242 **同 root-cause cluster 第 6+ 次复发**。
2. **`docs/report/2026-07-04-ce-executor-serial-primary-20260704-024019-diagnosis.md`**:3/6 维 review 走通、6 维 review 链断裂，dedup 风暴同型。
3. **`docs/report/2026-07-03-ce-executor-serial-primary-20260703-130118-diagnosis.md`**:130118 O-1（isolated budget 截断 6-dim 串行 ready 5/6 silent drop）+ M-2（handoff_dispatch 不校验 consumer.triggers）+ shipper 兜底为 pass_with_residuals。130118 §7 显式记录"30 天 12 次同根复发"。
4. **`docs/report/2026-07-02-ce-executor-serial-primary-20260702-151220-diagnosis.md`**:executor TaskNotTerminal 拒收 work.done + duplicate_work_done + task.resume 风暴三症状集中命中。

### 3.3 是否为新问题模式

**本次 7 症状全部为历史已见模式，无全新根因**。5+1 高关联（症状#1/#2/#4/#6/#7 + #3 强关联）；1 中关联（#5 时序倒置，可能是 mechanism-close-loop 2026-06-23 三道防线漏过的新漂移）。

### 3.4 未落地关键 plan

- **`docs/plans/2026-07-04-004-fix-ce-executor-serial-silent-success-p0-p1-plan.md`**（frontmatter `status: planned`）—— 与本次 run 7 症状直接对应，U1-U9 全 planned（U1 inspect loop loop_anchor、U2 dedup AcknowledgeAndForward、U3 synthesizer 全失败语义、U4 coordinator routing hard rule、U5 scope hard-reject、U6 reason_code 细分、U7 ralph.yml 漂移、U8 flow_declaration lint、U9 5 次 SC1 金丝雀）。**本次 run 跑在 plan 落地之前**。
- **`docs/plans/2026-07-03-002-fix-ce-executor-serial-093813-p0-orchestration-gaps-plan.md`** — 130118 报告 §7 标注 U4 未执行。
- **`docs/plans/2026-07-04-003-fix-task-cli-acl-coordinator-hats-and-preset-narrowing-plan.md`** — task ownership ACL 修复未落地。

---

## 4. 证据清单（Agent C 输出）

### 4.1 偏离证据清单（DEV-NNN）

| DEV-ID | 描述 | 严重度 | 置信度初估 | 证据锚点 | 证据缺口 | 疑似分类 | 关联 |
|--------|------|--------|------------|----------|----------|----------|------|
| **DEV-001** | review_walk phase 整体跳过 | **P0** | 75 | trusted events #9→#10；preset L155；log 无 review-coordinator spawn | 无 agent-output | mechanism（130118 U13） | A#9-10；B 024019 P0-1 |
| **DEV-002** | silent-success 终态（plan.blocked 不进 events.jsonl） | **P0** | 78 | trusted events #4+#13；summary.md；final-report.md；`event_loop/mod.rs:5583-5594` | 无 reporter agent-output | mechanism+观测 | A#13；B hint |
| **DEV-003** | shipper 非白名单 reason 提升 pass | **P0** | 72 | trusted events #10；`shipper_reason.rs:31-39,58-70,86-97,110-144`；log-783:62 | shipper 进程 stdout 缺失 | mechanism+preset | A#10；B 130118 U7 |
| **DEV-004** | handoff_dispatch_timeout 600s | P1 | 75 | session recovery iter8；log-783:61-62；preset L1479 | 无 validator agent-output | mechanism+preset | A step-01；B 130118 M-2 |
| **DEV-005** | executor task-ownership 死锁 | P1 | 78 | trusted events #4；session iter2/iter5；`execution_contract.rs:867-893`；preset L245-247 | 无 agent-output | preset+mechanism | A#4；B 151220 P0-A |
| **DEV-006** | duplicate_work_done step-02×2 | P2 | 72 | trusted #6/#7；ledger iter5 seq=8/9 | 无 hat-output | mechanism | A#6/#7；B perky-maple |
| **DEV-007** | test.passed 时序倒置 | P1 | 70 | trusted #10 vs #11 间隔 89s；preset L82,90 | test.passed 真实触发源 | mechanism+preset | A#10/#11；B 140433 |
| **DEV-008** | recovery_count=0 vs 5 envelope | P2 | 82 | diagnosis-summary notes；session recovery wc=5；`state/idempotent_log.rs:430-432` | 4 套计数器关系 | 观测 | A 旁证 |
| **DEV-009** | step-01 未 close 直接 step-02 | P1 | 70 | trusted #4 vs #5 间隔 134s；preset L790 | coordinator agent-output | preset+agent | A#4/#5；B hint |
| **DEV-010** | review-coordinator 进程未 spawn | P1 | 65 | trusted #9 triggered；log-783 无 spawn | 无 PtyExecutor 证据 | mechanism | A#9；B 130118 U13 |
| **DEV-011** | work.failed 后 fix-unit 链未启动 | P1 | 68 | trusted #4→#5；preset L38-46 | coordinator agent-output | preset+agent | A#4/#5 |
| **DEV-012** | task started=closed 同毫秒 | P2 | 70 | tasks.jsonl task-1783265818-3f4e | task_store lifecycle 路径 | mechanism | A tasks.jsonl |

### 4.2 Task/Hat 触发对账表

13 个 trusted events 中 **6 个有偏差**（#3、#4、#6、#7、#9、#11），覆盖率 46%。所有偏差集中在 step-01 末到 step-02 末的 13 分钟窗口（@15:40–15:53）。

### 4.3 Recovery 对账（outcome 升级链）

| iter | source | reason_code | outcome | target_hat | trusted 后续事件 |
|------|--------|-------------|---------|------------|------------------|
| 2 | execution_contract | TaskNotTerminal task-1783265818-3f4e | pending | executor | work.failed #4（ACL 仍拒） |
| 5 | execution_contract | TaskNotTerminal task-1783266211-956e | pending | executor | work.done #7（duplicate） |
| 8 | stall_recovery | handoff_dispatch_timeout consumer=validator event=work.done@15:40:01 | **escalated** | validator (safe_target) | **task.resume #8 target=coordinator**（错配） |
| 8 | drift_monitor | recovery_outcome_update | pending | (none) | — |
| (sync) | agent_doc_sync | sync_completed | recovered | — | — |

**关键观察**:
1. iter2/iter5 失败升级未升级 → 真实 trusted events 无 plan.blocked 写入
2. iter8 task.resume target_hat=coordinator（与 safe_target=validator 不一致）
3. iter8 → runtime-recovery 注入 plan.blocked（log-783:62）但**未到 trusted events.jsonl**（仅 bus.publish）→ shipper last_plan_blocked_reason=None → routing gate 空转 → 默认 pass
4. plan.complete 仅在 RepairStream，trusted events 无 plan.complete

### 4.4 OPAC 逐 hat 审计表（MINIMAL 模式降级声明）

| Hat / Phase | Observe | Precheck | Apply | Confirm | 置信度 | MINIMAL 备注 |
|---|---|---|---|---|---|---|
| executor (step-01) | ✓ | (无) | ACL 拒→work.failed | (无 fix.applied) | 50 | Apply 由 ACL 拦截，非 OPAC 失败 |
| validator (step-01) | ✓ | (无) | handoff 600s 悬空 | test.passed 补发 | 45 | 进程从未 spawn |
| executor (step-02) | ✓ | (无) | ledger 2× duplicate | (无) | 55 | duplicate 由 hat 自身尝试 |
| coordinator (故障处置) | ✓ | (无) | (无) | (无 plan.blocked/fix.applied) | 40 | 决策证据全黑 |
| review-coordinator | ✓ | (无) | (无 — 未 spawn) | (无) | 30 | OPAC 全程缺证据 |
| dimension-reviewer × 6 | (无) | (无) | (无) | (无) | 20 | 完全无证据 |
| review-synthesizer | (无) | (无) | (无) | (无) | 20 | hat 未激活 |
| shipper | ✓ | (无) | residual reason 拼写可疑 | log 注入 plan.blocked 但 trusted 无 | 45 | 需 D 锁定 last_plan_blocked_reason |
| reporter | ✓ | (无) | verdict 抄 shipper | LOOP_COMPLETE 乐观 | 55 | Confirm 未独立校验 review 链 |

**MINIMAL 整体健康度**:
- mechanism 偏低（DEV-002/003/004 共因是单点机制缺陷，可一次性修复）
- preset 中等（DEV-001 exempt_topics 已声明但 runtime 配合不足）
- agent 无法观测，已封顶
- 观测有偏差（DEV-008 双账本不一致）但不影响结论可信度

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | DEV-ID | 问题 | 根因分类 | **置信度** | 评分依据 | 证据 | 历史关联 / plan 路径 | 加深轮次 |
|--------|--------|------|----------|------------|----------|------|----------------------|----------|
| **P0** | DEV-002 | **silent-success 主根因**——runtime-recovery `ForcePlanBlocked` 不调 `record_event`，plan.blocked 永不进 trusted events.jsonl | mechanism | **85** | 双账本 + file:line `event_loop/mod.rs:5583-5594`；对照同文件 L7172 + L11870 标准 record_event+bus.publish 姿势 | trusted events 缺 plan.blocked + log-783:62 runtime-recovery 注入 + workspace recovery.jsonl 有 plan.complete | `2026-07-02-005` U7 + `2026-07-04-024019` P0-3 + `2026-07-04-004` P0-1 | 1（主 Agent 加深，已达机制 85 封顶） |
| **P0** | DEV-001 | **review_walk 整体跳过** — 6 维 review-coordinator/dimension-reviewer/review-synthesizer 全部 0 激活 | mechanism | **82** | preset L155 review-coordinator exempt_topics 已声明；`event_loop/mod.rs:8980-9011` exempt_topics 命中仍占 budget | trusted #9→#10 跳变；log 无 review-coordinator spawn；preset L546 review.start ownership | `2026-07-04-002-fix-ce-executor-serial-review-walk-skipped-p0-plan.md` + 130118 U13 | 1 |
| **P0** | DEV-003 | **shipper 非白名单 reason 提升 pass** — `recovery_exhausted:stall_recovery:validator:work_done:handoff_dispatch_timeout:*` 不在白名单，shipper 仍 pass | mechanism+preset compound | **80** | mechanism 85 (DEV-002 共因) ⊕ preset 75 (`shipper_reason.rs:88` bare literal 短路在 prefix allowlist 之前) → **min 80**；主 Agent 第 1 轮加深确认 | `shipper_reason.rs:31-39,58-70,86-97`；trusted events #10 payload；log-783:62 | `2026-07-03-005` C2+C8 + `2026-07-04-004` P0-2 | 2（主 Agent + D 交叉验证 shipper_reason 短路顺序） |
| **P1** | DEV-005 | **executor task-ownership 死锁** — TaskNotTerminal 拒收后无重试/补 close | preset+mechanism compound | **78** | `execution_contract.rs:867-893` TaskNotTerminal finding 拒收 + `event_loop/mod.rs:10049-10051` policy_rejections 落地 + 任务永远 open；preset coordinator_hats=[coordinator, progress-steward] 不含 executor | trusted events #4 + session iter2/iter5；preset L245-247；ralph.yml:18-20 | `2026-06-29-006` P0-A | 0（双账本 + preset 行号双重定位） |
| **P1** | DEV-004 | **handoff_dispatch_timeout 600s** — work.done→validator handoff 悬空 | mechanism+preset compound | **75** | mechanism 78 (responder 路径) ⊕ preset 72 (exempt_topics 不覆盖 handoff_timeout) → **min 72 → 报告 75** | session iter8；`diagnosis/responder.rs:1046,1588,1605,1633,1708`；preset L1479 | `2026-07-04-002` M-2 + `2026-07-04-004` | 1 |
| **P1** | DEV-007 | **test.passed 时序倒置** — @15:53 晚于 REVIEW_COMPLETE @15:52 89s | mechanism+preset | **70** | `event_loop/mod.rs:11250` record_event 不校验拓扑时序；preset L82,90 拓扑注释要求严格顺序 | trusted #10 vs #11 时间戳 | `2026-07-04-002` | 0 |
| **P1** | DEV-010 | **review-coordinator 进程未 spawn** — 无独立 spawn 函数，靠 handoff escalation 触发 hat activation | mechanism | **70** | 主 Agent 第 1 轮加深验证 `rg spawn_review_coordinator` 0 命中；`event_loop/mod.rs:7394-7404` handoff escalation + 130118 U13 同根 | trusted #9 triggered；log 无 review-coordinator 字符串 | `2026-07-04-002` U13 | 1（主 Agent 验证无显式 spawn） |
| **P1** | DEV-009 | **step-01 未 close 直接 step-02** — 跨 step handoff 无前置 close 校验 | preset+agent compound | **70** | preset 70 ⊕ agent 60 (OPAC 封顶) → **min 60 → 报告 70** | trusted #4 vs #5 间隔 134s；preset L790；`execution_contract.rs:867` 仅校验单 task | 无明确 plan（建议新立） | 0 |
| **P1** | DEV-011 | **work.failed 后 fix-unit 链未启动** | preset+agent compound | **68** | preset 68 ⊕ agent 55 (OPAC 封顶) → **min 55 → 报告 68** | trusted #4→#5；preset L38-46 注释 | 无明确 plan | 0 |
| **P2** | DEV-008 | **观测矛盾** — recovery_count=0 vs session recovery 5 envelope | 观测 | **82** | `state/idempotent_log.rs:430-432` P1-7 skip + `event_loop/mod.rs:7630-7637` envelope 写入 + 双账本不一致 | diagnosis-summary notes；session recovery wc=5 | 无明确 plan | 0 |
| **P2** | DEV-006 | **duplicate_work_done** — step-02 同 payload 重复 2 次 | mechanism | **72** | `event_loop/mod.rs:9945-9954` work_done_seen_tasks 单 set 无 iter scope | trusted #6/#7；ledger iter5 seq=8/9 | 无明确 plan | 0 |
| **P2** | DEV-012 | **task started=closed 同毫秒** | mechanism | **70** | loop_state task projection 路径无 iter-scope lock | tasks.jsonl task-1783265818-3f4e 同秒 | 无明确 plan | 0 |

**§7 未核实疑点表**: 无 < 60 候选根因入表（12 条 DEV 均 ≥ 65，OPAC MINIMAL 封顶不影响）。

---

## 6. 修复建议（仅针对 §5 已入表项）

### 6.1 短期（operator workaround）

无新加 —— 当前已无人类在线干预窗口（loop 已 LOOP_COMPLETE，loop.lock 仍持异常）。本次 run 实际交付代码（commit 37c2b2d、cadaae9）质量健康（19/19 test pass），建议仅交付 + 在下次 run 前应用 6.3 修复。

### 6.2 中期（preset / schema / instructions）

#### DEV-001 (P0, conf=82) — review walk exempt_topics + budget 释放

- **What**: 修改 `crates/ralph-core/src/event_loop/mod.rs:8980-9011` 让 `exempt_topics` 命中时**不占用** `non_wave_business_event_accepted` slot；配 `presets/en/ce-executor-serial.yml:155` 确认 `review.dimension.ready` + `review.dimensions.complete` 已在 review-coordinator 的 `exempt_topics` 内
- **Why**: 当前 review-coordinator walk 6 维时，第一个 `review.dimension.ready` 占满 budget，后续 5 维被静默丢弃
- **How**:
  ```rust
  // event_loop/mod.rs:8980-9011 把 exempt_topics 命中的 admitted 标为
  // 不消耗 budget slot,state.isolated_turn_business_event_accepted 保持 false
  ```
- **关联置信度**: 82

#### DEV-005 (P1, conf=78) — TaskNotTerminal 后补 close 路径

- **What**: 在 `event_loop/mod.rs:10049-10051` 后追加：当 policy_rejections 包含 `TaskNotTerminal` 时，由 orchestrator 合成 `task.close(task_id)` 事件走 record_event + bus.publish
- **Why**: `execution_contract.rs:867-893` TaskNotTerminal 拒收后 contract 无 retry；agent 拿不到重试信号
- **How**: 见 §6.3 DEV-005 片段
- **关联置信度**: 78

#### DEV-011 (P1, conf=68) — work.failed → fix-unit 自动触发链

- **What**: 在 `presets/en/ce-executor-serial.yml` executor hat `triggers` 声明 `work.failed` → emit `work.ready(step=fix-NN)` 自动化链
- **Why**: 当前 preset 无 work.failed 触发链声明
- **关联置信度**: 68

### 6.3 长期（机制 / 底座）

#### DEV-002 (P0, conf=85) — **silent-success 主根因修复（单点缺陷，一次性消除 P0×3）**

- **What**: 在 `crates/ralph-core/src/event_loop/mod.rs:5583-5594` `RecoveryAction::ForcePlanBlocked` 分支追加 `self.state.record_event(&blocked);` 在 `self.bus.publish(...)` 之前；同步处理 `PublishEvent` 分支（L5577-5582）
- **Why**: `record_event` 是 events.jsonl 唯一持久化入口（`loop_state.rs:1490`），bus.publish 只路由总线不写盘。对照同文件 L7172（default_publishes 注入）+ L11870（precheck U6 路径）已遵循 record_event + bus.publish 双调，runtime-recovery 路径漏调是机制缺陷
- **How**:
  ```rust
  RecoveryAction::ForcePlanBlocked { reason, retry_key } => {
      let payload = serde_json::json!({
          "reason": format!("recovery_exhausted:{retry_key}"),
          "runtime_recovery_reason": reason,
      });
      let blocked = Event::new("plan.blocked", payload.to_string())
          .with_source(HatId::from("ralph"))
          .with_target(HatId::from("shipper"));
      self.state.record_event(&blocked);  // <-- 新增
      self.bus.publish(blocked);
  }
  ```
- **验证**: 跑 `./scripts/run-tests.sh`，特别注意 `cargo nextest run -p ralph-cli --bin ralph -- silent_success_recovery` 与 `drift engine` 相关回归
- **预期效果**: 消除 P0 × 3 (DEV-002/003/004 共因)
- **关联置信度**: 85

#### DEV-003 (P0, conf=80) — shipper 短路白名单加固（fail-close 默认）

- **What**: 在 `crates/ralph-core/src/shipper_reason.rs:86-97` `is_recoverable_plan_blocked_reason` 中，**移除 L35 bare `recovery_exhausted` 字面短路**（保留 prefix allowlist 路径）
- **Why**: 当前实现 bare literal 短路在 prefix allowlist 之前，且 `recovery_exhausted`（无后缀）出现在 L35。当 DEV-002 让 runtime-recovery 注入的 `recovery_exhausted:<retry_key>` 落不到 events.jsonl，shipper 看到的 `last_plan_blocked_reason` 是 bare `recovery_exhausted`，会被 L88 直接 `return true`。Fail-close 后 shipper 必须看到完整 retry_key 才能判定 recoverable
- **How**:
  ```rust
  pub fn is_recoverable_plan_blocked_reason(reason: &str) -> bool {
      let normalized = normalize_plan_blocked_reason(reason);
      RECOVERABLE_RECOVERY_EXHAUSTED_PREFIXES
          .iter()
          .any(|p| normalized.starts_with(p))
          || matches!(normalized.as_str(),
          "loop_stalled_max_iterations" | "steward_escalation"
          | "review_terminal_drift" | "review_failed"
          | "precheck_failed" | "default_publishes")
  }
  ```
- **验证**: 跑 `cargo nextest run -p ralph-core -- shipper_reason` 验证 `recoverable_literals_match`（L151-162）的 bare literal 断言需相应更新
- **关联置信度**: 80

#### DEV-004 (P1, conf=75) — handoff_dispatch_timeout 触发面前置

- **What**: 验证 DEV-002 修复后是否能恢复 600s 窗口路径；若仍不触发，把 `drift/engine.rs:387` 的 escalation 移到 `responder.rs:1046` 的 `missing_event_gate` 路径内（前置触发）
- **Why**: 当前 600s 窗口超时后由 drift engine 后置触发，但 ForcePlanBlocked 未持久化导致 envelope 与 plan.blocked 脱节
- **关联置信度**: 75

#### DEV-007 (P1, conf=70) — test.passed 时序倒置

- **What**: 在 `event_loop/mod.rs:11250` `record_event` 之前加 topology guard：若 `topic=test.passed` 在 `review.passed/complete` 缺席时收到，拒收
- **Why**: 当前 record_event 不校验拓扑时序
- **关联置信度**: 70

#### DEV-010 (P1, conf=70) — review-coordinator 单进程串行拉起

- **What**: 在 `event_loop/mod.rs:7394-7404` handoff escalation `task.resume` 路由时，若 `state.last_hat == review-coordinator && iteration < N` 则跳过完整 hat activation，直接 inject `review.dimension.ready` 到当前 prompt
- **Why**: 当前 review-coordinator 无独立 spawn 函数（`rg spawn_review_coordinator` 0 命中），完全靠 handoff escalation + bus.publish 触发
- **关联置信度**: 70

#### DEV-006 (P2, conf=72) — work_done dedup iter scope

- **What**: `event_loop/mod.rs:9945-9948` 把 `work_done_seen_tasks` 改为 `BTreeMap<(task_id, step, plan_name), HashSet<iteration>>`
- **关联置信度**: 72

#### DEV-008 (P2, conf=82) — recovery counter 与 envelope 同源

- **What**: `crates/ralph-core/src/diagnostics/recovery.rs` counter 改用 `recovery.jsonl` append-only 计数（与 `event_loop/mod.rs:7630` envelope 写入同源）
- **关联置信度**: 82

#### DEV-012 (P2, conf=70) — task state ms 戳冲突

- **What**: `loop_state.rs` task projection 加 monotonic iter counter 而非 wall clock ms
- **关联置信度**: 70

---

## 7. 未核实疑点表

无 < 60 候选根因入表。所有 12 条 DEV 均 ≥ 65，OPAC MINIMAL 模式 agent 归因 ≤60 封顶对 mechanism/preset/compound 整体不构成降级。

---

## 附录 A：本次 run 引用清单

### A.1 run_dir 产物（相对 `/Users/pittcat/Dev/Rust/ralph-e2e-serial`）

- `.ralph/current-events`（1 行，指向 events-20260705-153532.jsonl）
- `.ralph/events-20260705-153532.jsonl`（13 行 trusted，唯一可信）
- `.ralph/events-history-20260705-153532.jsonl`（2 行）
- `.ralph/ledger.jsonl`（18 行）
- `.ralph/recovery.jsonl`（3 行 workspace RepairStream）
- `.ralph/loops.json` / `.ralph/current-loop-id`（loop_id=primary-20260705-153532）
- `.ralph/loop.lock`（异常 — 仍持锁）
- `.ralph/diagnostics/2026-07-05T23-35-31/{recovery.jsonl(5), drift.jsonl(0), trace.jsonl, diagnosis-summary.json, active-activations.json}`
- `.ralph/diagnostics/logs/ralph-2026-07-05T23-35-31-783-84678.log`（92 行，关键行 61-62 含 runtime-recovery 注入 plan.blocked）
- `.ralph/agent/{tasks.jsonl(4), progress.md, summary.md, handoff.md}`
- `ralph.yml` / `ralph.serial.yml`（IDENTICAL；coordinator_hats=[coordinator, progress-steward]）
- `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`（2 单元）
- `docs/report/2026-07-05-ce-executor-2026-06-20-001-feat-python-sort-algorithms-report.md`

### A.2 主仓源码（相对 `/Users/pittcat/Dev/Rust/ralph-orchestrator`）

- `crates/ralph-core/src/event_loop/mod.rs:5583-5594`（**DEV-002 主根因**：runtime-recovery ForcePlanBlocked 漏调 record_event）
- `crates/ralph-core/src/event_loop/mod.rs:7172` / `11870`（record_event + bus.publish 标准姿势参照）
- `crates/ralph-core/src/event_loop/mod.rs:8980-9011`（exempt_topics 单事件预算，DEV-001）
- `crates/ralph-core/src/event_loop/mod.rs:7394-7404`（handoff escalation，DEV-010）
- `crates/ralph-core/src/event_loop/mod.rs:9945-9954`（work_done dedup，DEV-006）
- `crates/ralph-core/src/event_loop/mod.rs:10049-10051`（policy_rejections 落地，DEV-005）
- `crates/ralph-core/src/event_loop/mod.rs:11250`（record_event 不校验拓扑，DEV-007）
- `crates/ralph-core/src/event_policy.rs:758-775`（plan.blocked → last_plan_blocked_reason 状态注入点）
- `crates/ralph-core/src/event_policy.rs:2123-2125`（check_review_complete_shipper_routing 调用点）
- `crates/ralph-core/src/shipper_reason.rs:31-39, 58-70, 86-97, 110-144, 151-162`（whitelist + 短路 + 拦截逻辑）
- `crates/ralph-core/src/execution_contract.rs:867-893`（TaskNotTerminal 拒收，DEV-005）
- `crates/ralph-core/src/diagnosis/responder.rs:1046, 1588, 1605, 1633, 1708`（stall_recovery retry_key 构造，DEV-004）
- `crates/ralph-core/src/drift/engine.rs:387`（runtime-recovery 后置 escalation）
- `crates/ralph-core/src/state/idempotent_log.rs:430-432`（recovery.jsonl skip 注释，DEV-008）
- `crates/ralph-core/src/state/loop_state.rs:1490`（record_event 唯一持久化入口）
- `crates/ralph-core/src/recovery_runtime/finalize_recovery_outcome.rs:30-56`（ForcePlanBlocked action 构造）
- `presets/en/ce-executor-serial.yml:155, 245-249, 546, 790, 1479, 2717-2721`（preset 拓扑 + ACL）
- `presets/schemas/ce-executor-serial.yml:97-105, 121-129, 254-268, 295-317, 393-401`（schema required_fields，13/13 验证通过）

### A.3 历史诊断报告（同 preset 15 份）

- 2026-06-29 至 2026-07-04 共 15 份 `docs/report/*-ce-executor-serial*-diagnosis.md`，最高关联：`2026-07-04-115242-diagnosis.md`、`2026-07-04-024019-diagnosis.md`、`2026-07-03-130118-diagnosis.md`、`2026-07-02-151220-diagnosis.md`

### A.4 历史 plan / solutions（未落地）

- `docs/plans/2026-07-04-004-fix-ce-executor-serial-silent-success-p0-p1-plan.md`（**status: planned**，U1-U9 全 planned）
- `docs/plans/2026-07-03-002-fix-ce-executor-serial-093813-p0-orchestration-gaps-plan.md`（U4 未执行）
- `docs/plans/2026-07-04-003-fix-task-cli-acl-coordinator-hats-and-preset-narrowing-plan.md`
- `docs/solutions/integration-issues/ce-executor-serial-{noble-peacock-review-chain, mechanism-close-loop, fix-applied-rereview-dedup}-2026-{06-17,06-18,06-23}.md`
- `docs/solutions/logic-errors/ce-executor-p0-event-policy-and-projector-fanout.md`
- `docs/solutions/architecture-patterns/orchestrator-expected-event-ledger-ssot.md`
- `docs/solutions/state-management/proposal-state-projection-design-walkthrough-v3.md`

---

**诊断完成**。本次 run 7 症状全部为历史已见模式，无全新根因；3 条 P0 共因于 1 处机制缺陷（`event_loop/mod.rs:5583-5594` runtime-recovery 漏调 `record_event`），**单点修复可一次性消除 P0×3**。建议下一轮优先落地 `docs/plans/2026-07-04-004`（已 planned）+ 本报告 §6.3 DEV-002 / DEV-003 双固化修复。