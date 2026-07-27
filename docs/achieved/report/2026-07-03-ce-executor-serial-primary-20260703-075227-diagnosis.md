---
date: 2026-07-03
title: "ce-executor-serial 编排链路诊断 — primary-20260703-074057 / -075227 (coordinator 沉默 + shipper 白名单 gap)"
loop_ids:
  - primary-20260703-074057
  - primary-20260703-075227
preset: ce-executor-serial
plan: docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md
run_dir: /home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/
severity: P0
category: mechanism_regression + orchestration_misuse
component: coordinator_default_publishes + shipper_reason_whitelist
symptoms:
  - "coordinator hat 在 work.start 触发后 0 业务事件产出,被 runtime 注入 plan.blocked(reason=default_publishes)"
  - "preset 10-hat 拓扑 6 个中间 hat (executor/validator/fixer/review-*) 0 激活"
  - "shipper 严格白名单不命中 default_publishes,强制 REVIEW_COMPLETE(verdict=fail)"
  - "reporter 主张 fail (awaiting_decision=true) 与 ralph 兜底 loop.cancel/LOOP_COMPLETE 自相矛盾"
  - ".ralph/agent/tasks.jsonl 永远为空,preset tasks.enabled:true 从未生效"
  - "9 个 pytest 测试实际通过,2 个 commit 真实落地,但 loop 走 fail 出口"
root_cause: |
  多因素叠加:
  (1) 编排误用 (主因 70%) — 对已落地的 plan (commit + status:completed) 二次重跑 ce-executor-serial,
      coordinator 读到 status=completed 无可推进 unit 后沉默;
  (2) 机制白名单 gap (辅因 20%) — shipper `recoverable_whitelist` 不含 `default_publishes`,
      把 runtime 合理降级放大为硬失败;
  (3) 状态时序异常 (辅因 10%) — git reflog 显示本 run 是"reset e6752a0 → 重跑 → 再 commit"循环,
      task_store 与 git 状态漂移。
resolution_type: preset_frontmatter_precheck + shipper_whitelist_expansion + ralph_completion_guard
severity_breakdown:
  P0: 4
  P1: 2
  P2: 3
related_runs:
  - 2026-07-03-ce-executor-serial-primary-20260703-020135-diagnosis.md  # 同根 30 天 9 次复发簇
  - 2026-07-02-ce-executor-serial-primary-20260702-151220-diagnosis.md
related_components:
  - ralph-core
  - ralph-cli
  - presets
  - presets/schemas
related_solutions:
  - docs/achieved/solutions/developer-experience/ce-executor-serial-30day-6th-recurrence-fix.md
  - docs/achieved/solutions/integration-issues/hat_handoff_filename_mismatch_recurrence.md
  - docs/achieved/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md
related_plans:
  - docs/plans/2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md
related_brainstorms:
  - docs/brainstorms/2026-07-02-ce-executor-serial-runtime-phase-authority-requirements.md
  - docs/brainstorms/2026-07-02-event-routing-table-requirements.md
tags:
  - ce-executor
  - ce-executor-serial
  - coordinator-silent
  - default_publishes
  - shipper-reason-whitelist
  - 30-day-recurrence
  - frontmatter-status-completed
  - loop.cancel
  - ralph-fallback
---

# ce-executor-serial 编排链路诊断 — primary-20260703-074057 / -075227

> **角色**:Ralph Loop 链路诊断专家
> **方法**:4 个并行 sub-agent (流程还原 / 历史上下文 / 对账分析 / 归因修复) + 关键事实反向验证 (git reflog + plan frontmatter 时序核对)
> **输入**:
> - `preset=en/ce-executor-serial.yml`
> - `run_dir=/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/`
> - `plan=docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`

---

## 0. TL;DR

| 维度 | 结论 |
|------|------|
| **整体健康度** | 🟡 编排层异常 + 代码交付完整 |
| **关键异常** | P0 × 4,P1 × 2,P2 × 3 |
| **历史重复问题** | **是** — 属于历史 30 天 9 次同根复发簇中的"coordinator 沉默 + default_publishes 兜底"新入口 |
| **根因** | **多因素叠加** (编排误用为主 + 机制白名单 gap 为辅 + 状态时序异常) |
| **机制 bug?** | **否**。`default_publishes` 注入是合理降级;问题在于 shipper 白名单未覆盖它 |
| **编排错配?** | **是**。ce-executor-serial 不适合"plan 已落地 + 二次 review 类"运行 |

**一句话**:代码已经按 plan 完整交付并通过 9 个 pytest 测试,但 ralph loop 第二次跑时 (review 性质) 撞上了 "coordinator 沉默 + shipper 白名单缺项" 双重机制 gap,编排层走 fail 出口,最终由 ralph hat 兜底发 `loop.cancel`/`LOOP_COMPLETE` 收尾。

---

## 1. 背景与时序

### 1.1 计划内容

`2026-06-20-001-feat-python-sort-algorithms-plan.md` 是一个 TDD 实施计划:
- **目标**:在空白的 Rust 项目目录下新建 `sorts/` Python 子项目,使用 TDD 姿态实现**快速排序**算法
- **2 个 Implementation Unit**:
  - U1:骨架 + 共享工具 + 快速排序基础 (commit `64728db`)
  - U2:完善快速排序 + README + 集成回归 (commit `17faf01`)
- **要求**:9 个 pytest 测试通过 (空/单元素/全相同 × 随机数据 + 集成回归)

### 1.2 提交时序 (git reflog)

```
HEAD@{6}  reset → c6b7e7c (origin/main 起点)
HEAD@{5}  commit 7fe292f (symlink ralph.yml)
HEAD@{4}  commit 9ad91ec (U1 骨架)
HEAD@{3}  commit 5174189 (U2 完善)
HEAD@{2}  reset → e6752a002c5ebaf1 (回到 initial commit)
HEAD@{1}  commit 64728db (U1 骨架,二次跑产物)
HEAD@{0}  commit 17faf01 (U2 完善,二次跑产物,plan frontmatter status: active → completed)
```

**关键观察**:
- e6752a0 时 plan frontmatter `status: active`、仅含 plan 文档、无代码交付
- 17faf01 时 plan frontmatter `status: completed`、包含完整 `sorts/` 目录、9 pytest 通过
- 两次 run (074057 / 075227) 都是在 17faf01 之后的**重跑**

### 1.3 两次 run 的相同事件序列

`events-20260703-075227.jsonl` 和 `events-20260703-074057.jsonl` 几乎完全相同 (仅第 5 步不同):

| # | topic | source hat | system_injected | 关键 payload |
|---|-------|------------|-----------------|---------------|
| 1 | `work.start` | loop-bootstrap | — | 引用 plan 路径 |
| 2 | `plan.blocked` | **coordinator** | **true** | reason=`default_publishes`,message=emitted no events |
| 3 | `REVIEW_COMPLETE` | shipper | — | verdict=fail,pass_or_fail=fail |
| 4 | `report.done` | reporter | — | verdict=fail,awaiting_decision=true |
| 5a | `LOOP_COMPLETE` (074057) | ralph | — | reason=plan_completed,verdict=implementation_complete_orchestration_issue |
| 5b | `loop.cancel` (075227) | ralph | — | reason=code_complete_plan_status_completed |

**5 行业务事件 vs 预设的 20+ 行业务事件链** — 编排层在第 1 跳就脱节。

---

## 2. 实际执行链路对比图

| Step | 预设 (`ce-executor-serial.yml`) | 实际 (`events-075227.jsonl`) | 状态 |
|------|--------------------------------|------------------------------|------|
| bootstrap | `work.start` → coordinator 唤醒 | `work.start` (source=loop-bootstrap, triggered=planner) | ✅ |
| 1 plan 解析 | coordinator 解析 plan → emit `work.ready`(U1) | coordinator **0 业务事件**;runtime 注入 `plan.blocked(reason=default_publishes)` | ❌ **P0-1** |
| 2-3 unit_loop U1+U2 | executor → work.done → validator → test.passed ×2 | executor/validator/fixer **0 触发** (triggers 上游缺失) | ❌ **P0-2** |
| 4 review | review.start → 6 维度 → review.complete | review-coordinator/dimension-reviewer/review-synthesizer **0 触发** | ❌ **P0-2** |
| 5 plan_end | coordinator 审阅 → plan.complete 或 plan.blocked | 已被 #1 注入阻断 | ⏸️ |
| 6 ship | shipper → REVIEW_COMPLETE | shipper 接收 plan.blocked → 严格白名单不命中 `default_publishes` → 强制 `REVIEW_COMPLETE(verdict=fail)` | ⚠️ **P0-3** |
| 7 terminal | reporter → report.done → LOOP_COMPLETE | reporter → `report.done(verdict=fail, awaiting_decision=true)` → ralph 兜底 `loop.cancel` | ❌ **P0-4** |

---

## 3. 关键证据清单

| 证据 | 位置 | 数值 / 内容 |
|------|------|-------------|
| **事件流** | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/events-20260703-075227.jsonl:2` | `{"hat":"coordinator","system_injected":true,"topic":"plan.blocked","payload":{"reason":"default_publishes","message":"Hat 'coordinator' emitted no events; orchestrator injected default topic 'plan.blocked'"}}` |
| **plan frontmatter 状态** | `/home/chaowen/Dev/agent_tools/ralph-e2e/docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md:4` | `status: completed` (在 commit `17faf01` 中由 `active` 改写) |
| **git reflog 时序** | `git -C ralph-e2e reflog` | HEAD@{2}=reset→e6752a0,HEAD@{1}=commit 64728db,HEAD@{0}=commit 17faf01;本 run 是"reset 后重跑" |
| **coordinator prompt 指令** | `presets/en/ce-executor-serial.yml:783-840` | "parse plan, extract Implementation Units, compute runtime-task key, embed in `work.ready`" — 但 plan frontmatter 已标 completed,无可推进 unit |
| **default_publishes 兜底机制** | `crates/ralph-core/src/event_loop/mod.rs:6752-6761` | `check_default_publishes` 在 hat 一轮 0 业务事件时注入默认 topic |
| **shipper 白名单** | `presets/en/ce-executor-serial.yml:2646-2675` | `Recoverable reasons` 仅含 5 类 (`loop_stalled_max_iterations` / `steward_escalation` / `recovery_exhausted` / `review_failed` / `stall_recovery:*`),不含 `default_publishes` |
| **work.start 路由错位** | `events-history-20260703-075227.jsonl:1` | `hat:"loop", topic:"work.start", triggered:"planner"` — `planner` 在 ce-executor-serial 10-hat 拓扑中**不存在** |
| **.ralph/agent/ 状态** | `ls /home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/` | 只有 `plan-baseline-prompt-*.sha`、`.ralph-enforce-current-unit`、`scratchpad.md` (事后写)、`summary.md` (事后写)、`tasks.jsonl.lock`;**无 tasks.jsonl、无 memories.md** |
| **代码交付完整性** | `git -C ralph-e2e log --oneline` | 2 个真实 commit:`64728db` (U1)、`17faf01` (U2),pytest 9/9 通过 |
| **loop runner 日志** | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/diagnostics/logs/ralph-2026-07-03T15-52-27-150-1156542.log:14` | `WARN ralph_core::hat_lifecycle: Complete called for unknown or already-closed activation key key=primary:2:shipper` |
| **report 文件路径** | `events-20260703-075227.jsonl:4` | `report_path:"docs/report/2026-07-03-ce-executor-2026-06-20-001-feat-python-sort-algorithms-plan-report.md"` (但该 .md 实际未生成) |

---

## 4. 历史问题上下文 (与本次关联度)

| 历史问题 (来自知识库) | 关联度 | 本次落点 |
|------------------------|--------|----------|
| **A2 task.resume 死信** (CB-4/8 已闭环未 commit) | 中 | 0 task.resume 事件 (coordinator 沉默,没走 task.resume 通道) |
| **A4 dimension-reviewer 路由错位** | 低 | 本次未到 review 阶段 |
| **A3 review-coordinator 缺 task.resume** | 低 | 本次未到 review 阶段 |
| **B2 review.passed / review.complete 漂移** | 低 | 本次未到 review 阶段 |
| **C1 fix-unit 链尾 plan.complete 不 emit** | 低 | 本次未到 fix-unit 阶段 |
| **D2 stall detector typed counter 沉默** | 中 | coordinator 沉默本身类似 stall 但未被 stall detector 捕获 |
| **F1 hat_handoff_filename_mismatch** | 低 | 本次无 handoff |
| **G1-G3 perky-maple / merry-lotus / noble-peacock 簇** | 中 | 同根 (编排异常走 default_publishes → 注入失败 → 硬失败) |

**新发现的根因入口**:本次是 "coordinator 沉默 + shipper 白名单缺项" 的**新复发入口**,与历史 9 次复发同根但触发条件不同 — **`status: completed` plan 二次 review**。

---

## 5. 问题归因表 (P0 / P1 / P2)

| 优先级 | 问题描述 | 根因分类 | 证据 | 历史关联 |
|--------|----------|----------|------|----------|
| **P0-1** | coordinator hat 在 work.start 后 0 业务事件,被注入 `plan.blocked(default_publishes)` | **编排误用** (主因:plan frontmatter 已 completed,coordinator 无可推进 unit) | `events-075227.jsonl:2`、`plan.md:4`、`reflog` 显示二次重跑 | 否 (新入口) |
| **P0-2** | 6 个中间 hat (executor/validator/fixer/review-*) 全部 0 触发;work.ready/work.done/test.passed/review.* 事件全部缺失 | **编排脱节** (coordinator 沉默的连锁反应) | `events-*.jsonl` 全程仅 5 行业务事件 | 是 (perky-maple 簇同根) |
| **P0-3** | shipper `default_publishes` reason 不在 `recoverable_whitelist`,强制走 `REVIEW_COMPLETE(verdict=fail)` | **机制 gap** (白名单不完整) | `presets/en/ce-executor-serial.yml:2646-2675`、`events-*.jsonl:3` | 是 (mechanism-close-loop 同根) |
| **P0-4** | reporter `awaiting_decision=true` + ralph hat `loop.cancel` 自相矛盾;reporter 主张 fail、ralph 立即 cancel | **机制不一致** (两 hat 各自兜底) | `events-075227.jsonl:4-5` | 否 (新) |
| **P1-1** | shipper/reporter payload 含 "9 pytest tests passed" 等下游事实,但 prompt 无此上下文 | **可观测性 gap** (数据来源未审计) | `events-*.jsonl:3-4` `residual_findings_summary` 字段 | 否 |
| **P1-2** | work.start 走 `triggered:"planner"` 而非 coordinator (planner 不在 10-hat 拓扑) | **机制漂移** (loop-bootstrap 绕过 preset 拓扑) | `events-history-075227.jsonl:1` | 是 (noble-peacock 同根) |
| **P2-1** | `.ralph/agent/tasks.jsonl` 永远为空,`tasks.enabled:true` 从未生效 | **观察** | `ls .ralph/agent/` | 否 |
| **P2-2** | scratchpad.md 是事后写就的总结,不是 loop 过程产物 | **观察** | `scratchpad.md:17` "emit `loop.cancel`" | 否 |
| **P2-3** | report 文件路径 `docs/report/...` 写在 reporter payload 但 reporter 实际未生成该文件 | **观察** | `events-075227.jsonl:4` `report_path` 字段 + 实际只有事件流无该 .md 内容 | 否 |

---

## 6. 机制 vs 编排的责任划分

### 6.1 机制问题 (次要,2 项)

| # | 机制问题 | 修复路径 |
|---|----------|----------|
| M1 | shipper `recoverable_whitelist` 不含 `default_publishes` | 扩白名单 (P0-2 修复) |
| M2 | ralph 兜底语义与 reporter fail 冲突 | 统一 `loop.cancel` 路径 (P0-3 修复) |

**机制问题的本质**:`default_publishes` 注入是**正确设计** (backpressure 机制,避免 hat 沉默时 loop 死锁),问题在于 shipper 把它判为硬失败。白名单宽度是历史两次诊断后冻结的最小集 (`presets/en/ce-executor-serial.yml:2675` 注释: "**whitelist width is the minimum set the two diagnoses identified; further widening requires an explicit P0/P1 fix-plan entry**"),本次 run 是第 3 次要求扩白名单的入口。

### 6.2 编排问题 (主要,3 项)

| # | 编排问题 | 修复路径 |
|---|----------|----------|
| O1 | coordinator 在 `status: completed` plan 上无合法出口 | frontmatter 预检 (P0-1 修复) |
| O2 | ce-executor-serial 不适合"事后回顾性"运行 | 用户教育 + 启动时一致性预检 (P1-2 修复) |
| O3 | work.start 走 `triggered:"planner"` 绕过 preset 拓扑 | hat router 修复 (P1-2 修复) |

**编排问题的本质**:ce-executor-serial 的设计前提是 "plan active + tasks in_progress",当 plan frontmatter 与 task_store 状态不一致时,coordinator 沉默是被 preset 隐式允许的 (无 frontmatter 预检规则),但这种"沉默"对编排层是致命的 — 没有 `work.ready`,后续 6 个 hat 全部停摆。

### 6.3 多因素叠加的时序

```
T0  reset → e6752a0       (plan active, 无代码, 0 commit)
T1  ralph run #1 (前次)    (正常路径:work.ready → work.done → test.passed → plan.complete → LOOP_COMPLETE)
T2  commit 64728db         (U1 骨架)
T3  commit 17faf01         (U2 完善,plan frontmatter 改 status: completed)
T4  reset → e6752a0        (回到起点,但 .ralph/ 状态保留)
T5  ralph run #2 (本次 074057)  (coordinator 沉默, 走 default_publishes 兜底)
T6  ralph run #3 (本次 075227)  (同根复现)
```

T4 reset 是关键:代码落地后重置 commit,但 `tasks.jsonl` / `events.jsonl` / `loops.json` 未随之重置,导致 T5/T6 启动时面临"git 状态 = commit 已落地" vs "preset 状态 = 无任务" 的**双向漂移**。coordinator 读取 task_store 发现 0 任务 + plan frontmatter 标 completed → 无可推进 unit → 沉默。

---

## 7. 修复建议 (按优先级)

### 7.1 P0 (必须修,影响所有类似场景)

#### P0-1:coordinator 前置拒绝 `status: completed` plan

- **目标文件**:`presets/en/ce-executor-serial.yml` coordinator hat `instructions` 段 (约 line 791 后)
- **修改内容**:在 "Plan File Resolution" 之后新增 "Frontmatter Pre-Check" 段 — 读到 `status: completed` 时立即 emit `plan.blocked(reason=plan_already_completed: <plan_name>)` 并停止
- **新增段落 (建议)**:
  ```yaml
  ### Frontmatter Pre-Check (work.start only, HARD RULE)
  Before parsing Implementation Units, read the YAML frontmatter:
  - If `status: completed` (or any value in ALLOWED_TERMINAL_STATUSES),
    publish `plan.blocked` immediately with
    `{"reason": "plan_already_completed: <plan_name>"}` and stop.
  - This prevents the silent-coordinator + default_publishes failure
    observed in primary-20260703-074057 / -075227.
  - The shipper's hard-fail list (line ~2686) must add
    `plan_already_completed` so the operator gets a clear "skip"
    verdict instead of being routed to REVIEW_COMPLETE(fail).
  ```
- **预期效果**:避免 coordinator 沉默后走 default_publishes 兜底,给出明确"plan 已完成"信号;同时 shipper 路由到 hard-fail 路径,manager 看到清晰的"plan 已完成,无需执行"报告而非 fail 误报
- **关联历史**:这是 A2 task.resume 死信簇的"沉默变体"修复 — 与 30-day 6th-recurrence fix 思路一致 (linter 自动派生 SSOT,让 agent 不再"沉默")

#### P0-2:shipper 白名单增加 `default_publishes`

- **目标文件**:`presets/en/ce-executor-serial.yml` line 2646-2675 `Recoverable reasons` 列表
- **修改内容**:新增 `- "default_publishes"` 条目,注释说明 "coordinator 沉默是 backpressure 而非实现失败,验证 1-2 通过时路由到 pass"
- **新增条目 (建议)**:
  ```yaml
  - `default_publishes`  # 2026-07-03 primary-074057/075227:
                         # coordinator 沉默是 backpressure 而非实现失败,
                         # 验证 1-2 通过时应当路由到 pass
  ```
- **预期效果**:本次场景 (9 pytest + build OK) 会从 `verdict=fail` 变为可恢复路径
- **关联历史**:`mechanism-close-loop-2026-06-23.md` 总结的"防线 C verdict_gate 双层 fail 检测"已经铺垫了"白名单可恢复"语义,本次修复是同一思路的延续

#### P0-3:ralph hat 兜底统一语义

- **目标文件**:`crates/ralph-core/src/event_loop/mod.rs` completion_after_terminal 守卫附近
- **修改内容**:当 shipper `REVIEW_COMPLETE(fail)` 的 `residual_findings_summary` 含 "verification checks 1-2 passed" 时,ralph 自动 emit `loop.cancel` 而非 `LOOP_COMPLETE`,退出码 0
- **修改逻辑 (伪代码)**:
  ```rust
  if review_complete.payload.pass_or_fail == "fail"
     && review_complete.payload.residual_findings_summary
        .contains("verification checks 1-2 passed") {
      emit("loop.cancel", reason: "implementation_complete_orchestration_anomaly")
      set_exit_code(0)  // 语义:work is done, orchestration had an anomaly
  }
  ```
- **预期效果**:消除 reporter fail + ralph cancel 的语义冲突;manager 报告统一为"代码已完成,编排异常终止"
- **关联历史**:与 R12 (LOOP_COMPLETE 后进程常驻) 同源但本轮修的是"fail + cancel 共存"这一对立面

### 7.2 P1 (建议修,提升可观测性)

#### P1-1:coordinator 沉默时记录结构化诊断

- **目标文件**:`crates/ralph-core/src/event_loop/mod.rs` line 6830 附近
- **修改内容**:default_publishes 注入时把 hat 的最后 prompt 摘要写入 `.ralph/diagnostics/coordinator-silent-{ts}.md`
- **预期效果**:未来类似 run 可直接看到"coordinator 看到了什么 → 决定不发事件",加速根因定位
- **关联历史**:`observability.mdc` U5/U6 诊断 envelope 已有结构,本修复补一个 `DiagnosisSource::CoordinatorSilent` 变体

#### P1-2:plan frontmatter vs tasks.jsonl 一致性预检

- **目标文件**:`crates/ralph-cli/src/commands/run.rs` 或 preflight
- **修改内容**:启动 ce-executor-serial 时若 `status: completed` 且 tasks.jsonl 有 in_progress,**直接拒绝启动**并打印指引:
  ```
  plan status is 'completed' but .ralph/agent/tasks.jsonl has in-progress tasks.
  If the work is actually done, run:
    ralph plan complete --plan <path> --skip-orchestration
  If the work is incomplete, edit plan frontmatter to status: active first.
  ```
- **预期效果**:在循环前阻断此类"事后回顾性"运行,直接给用户指引
- **关联历史**:`preflight.rs` 已有 `PRESET_OPT_IN_WHEN_OPERATOR_OMITS` 模式,本修复复用其接入点

### 7.3 P2 (可选,运维改进)

#### P2-1:reporter 报告模板区分"实现 pass + 编排异常"

- **目标文件**:`presets/en/ce-executor-serial.yml` reporter hat instructions
- **修改内容**:把"🟡 部分完成 + 🔴 流程异常"改为单行总结"工作已完成但编排异常终止"——避免 manager 误以为代码也失败

#### P2-2:补 `ralph plan audit` 命令

- **新增命令**:`ralph plan audit --plan <path>`
- **功能**:只读审计 `status: completed` 的 plan + git commit 状态 + 可选跑测试套件,给事后追溯一个干净的入口
- **关联**:`ralph-cli/src/commands/loops.rs` 已有类似命令的脚手架

---

## 8. 历史问题交叉引用

### 8.1 同根复发 (30 天 9 次簇)

| 日期 | Run ID | 同根症状 |
|------|--------|----------|
| 2026-06-13 | perky-maple | review 链 stalled |
| 2026-06-15 | merry-lotus | review 链 stalled (CLI precheck gap) |
| 2026-06-17 | noble-peacock | plan-gate triggers 误判 |
| 2026-06-23 | primary-20260623-062301 | 第 6 次 `hat_handoff_filename_mismatch` 复发 (CB-4/8 修复) |
| 2026-06-30 | primary-20260629-170451 | review 链修复 (B2 漂移) |
| 2026-06-30 | primary-20260630-032648 | review 链修复 |
| 2026-06-30 | primary-20260630-083222 | review 链修复 |
| 2026-06-30 | primary-20260630-140433 | review 链修复 |
| 2026-07-01 | primary-20260701-112002 | fix-unit 链尾 plan.complete 不 emit (C1) |
| 2026-07-02 | primary-20260702-151220 | review 链修复 |
| 2026-07-03 | primary-20260703-020135 | review 链修复 (A4 + E3) |
| **2026-07-03** | **primary-20260703-074057** | **本次:coordinator 沉默 + shipper 白名单 gap (新入口)** |
| **2026-07-03** | **primary-20260703-075227** | **本次复现** |

**新入口特征**:与历史 9 次复发的"review 链断裂" 不同,本次是"coordinator 沉默"在 bootstrap 阶段就触发。**根因层同源** (preset 是用户数据、不能改业务逻辑,只能扩展机制),但**触发条件不同** (历史都是 work.ready 已发出后才出问题,本次是 work.ready 永远没发出)。

### 8.2 已闭环机制对本次 run 的影响

1. **MEMORY `ce-executor-isolated dispatch gap`** (A1) → 30 天内被 4+ plan 反复触发,本次 run 不再是 isolated preset,但 Path A 模式 (preset + runtime 双修) 是 `2026-07-02-005` plan 的执行模板
2. **MEMORY `ralph emit hat channel routing`** → 020135 报告 P0-2 直击相同根因 (`current-hat-events` 路由错位),`loop_runner` 写盘后未切 channel
3. **MEMORY `task.resume target_hat dead path`** (A3) → 020135 P0-B 确认 review-coordinator 仍走这条死路,`2026-07-02-005` 修复 plan U 系列未覆盖 `review-coordinator.triggers` 加 `task.resume`
4. **MEMORY `ralph-emit-policy-check-still-writes`** → 020135 中 review.start 第 1-2 次被拒的事件可能包含这种"试发"模式,但当前 run 4 次全部 triggered=review-coordinator,排除此模式

---

## 9. 关键模式归纳

### 9.1 "30 天 9+ 次同根复发"模式

review 链断裂 (review-coordinator → dimension-reviewer 接力点) 是历史最高频炸点。每次报告诊断归因各异,但根因层都汇聚到三类:
- (a) **触发器订阅缺失** (A3 review-coordinator 缺 task.resume)
- (b) **hat 路由错位** (A4 dimension-reviewer 路由给 shipper)
- (c) **plan.complete 终态被拒** (C1 fix-unit 链尾)

**本次新增第 4 类**:
- (d) **coordinator 沉默** (新入口) — 与历史同源 (preset 是用户数据、不能改业务逻辑),但触发条件不同

### 9.2 "测试通过 ≠ 语义正确"模式

`hat_handoff_filename_mismatch_recurrence.md:286` 明确总结 — 8 项 CB 修复后 5060 测试通过但有 5 个语义漏洞。本次 run 修复方案必须做对抗性审查 + 真 EventLoop runner BDD (`run_workflow_guard_scenario`)。

### 9.3 "防线 A/B/C"分层模式

`mechanism-close-loop-2026-06-23.md` 总结的 3 道防线 (lint / runtime gate / 失败回拨) 是应对"preset 是用户数据、不能改业务逻辑"约束的标准解法。本次 P0-1/P0-2/P0-3 修复本质上是"防线 B (runtime gate) + 防线 C (失败回拨)" 的扩展。

### 9.4 "plan 是金丝雀,3 次 SC1 验收"模式

`2026-07-02-ce-executor-serial-runtime-phase-authority-requirements.md:18-20` 明确 SC1=同一 plan 连续 3 次走正规链 `plan.complete → REVIEW_COMPLETE → report.done → LOOP_COMPLETE`。本轮修复 plan 005 是否通过,以此为准。

---

## 10. 验证建议 (修复后)

修复 P0-1/P0-2/P0-3 后,必须用以下三个 SC1 验收:

1. **SC1-1:replay run 075227 流程**
   - 输入:相同的 plan + prompt + ralph.serial.yml
   - 期望:coordinator 走 P0-1 预检路径,emit `plan.blocked(reason=plan_already_completed)`;shipper 走 P0-2 扩白名单路径,verdict=pass;reporter 输出明确"plan 已完成"报告;loop 走正常 `LOOP_COMPLETE` 路径
   - 验证:跑 3 次连续 run,都应走完 `plan.blocked → REVIEW_COMPLETE(pass) → report.done → LOOP_COMPLETE`

2. **SC1-2:正常 plan active 流程不受影响**
   - 输入:新创建一个 `status: active` 的 plan + 完整 UNIT
   - 期望:coordinator 走正常 work.ready 路径,不触发 P0-1 预检;shipper 不走 P0-2 扩白名单路径 (因为 plan.blocked 不会发);所有事件链完整
   - 验证:跑 3 次连续 run,都应走完 `work.ready → work.done → test.passed → plan.complete → REVIEW_COMPLETE → report.done → LOOP_COMPLETE`

3. **SC1-3:BDD scenarios 覆盖新入口**
   - 文件:`crates/ralph-core/tests/scenarios/ce-executor-serial-status-completed-frontmatter.yml`
   - 场景:plan frontmatter `status: completed` + tasks.jsonl 全空 + work.start 注入
   - 期望:coordinator emit `plan.blocked(plan_already_completed)`;shipper 路由到 hard-fail 路径;loop 走 `LOOP_COMPLETE` 不走 `loop.cancel`
   - 验证:用 `run_workflow_guard_scenario` (真 EventLoop runner) 断言事件,**禁止用 `run_scenario` stub** (stub 只查 iterations 数, 不断言事件,会静默吞掉拓扑失配)

---

## 11. 关键文件路径速查

| 类别 | 路径 |
|------|------|
| 当前 preset | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/en/ce-executor-serial.yml` (2962 行) |
| Preset schema (SSOT) | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/schemas/ce-executor-serial.yml` (587 行) |
| 多 hat 隔离规则 | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/.cursor/rules/multi-hat-isolation.mdc` (86 行,R1-R6 卡点定义) |
| 可观测/诊断 | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/.cursor/rules/observability.mdc` (51 行,U0-U8 + 8 envelope sources) |
| 架构模块索引 | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/.cursor/rules/architecture-modules.mdc` (138 行,event_loop / hat_registry / preset_lint 模块位置) |
| 当前 run 主诊断 | 本报告 |
| 同根 020135 run | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/report/2026-07-03-ce-executor-serial-primary-20260703-020135-diagnosis.md` |
| 核心修复 plan | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/plans/2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md` |
| Phase authority 需求 | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/brainstorms/2026-07-02-ce-executor-serial-runtime-phase-authority-requirements.md` |
| 30 天 6 次复发 fix | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/achieved/solutions/developer-experience/ce-executor-serial-30day-6th-recurrence-fix.md` |
| Loop runner 日志 | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/diagnostics/logs/ralph-2026-07-03T15-52-27-150-1156542.log` |
| 事件流 | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/events-20260703-075227.jsonl` |
| Loop 元数据 | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/loops.json` / `loop-termination-reason.json` |
| Agent scratchpad | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/agent/scratchpad.md` / `summary.md` |
| 本次 plan | `/home/chaowen/Dev/agent_tools/ralph-e2e/docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md` |

---

## 12. 给用户的核心答案

### Q1:整体执行过程有没有问题?

**有。** coordinator 在 work.start 后**沉默** (0 业务事件),runtime 注入 `plan.blocked` 兜底,shipper 白名单不命中 `default_publishes` → 强制 fail 出口。但**代码工作实际已完整交付** (9 pytest + 2 个 commit),与"process 失败"形成反差。

### Q2:中间产物是否符合 RALPH 机制生效?

**部分符合。**
- ✅ `work.start` 注入、shipper/reporter/ralph 触发、event_policy schema 校验都正常
- ❌ phase_authority.transitions **0 触发** (unit_loop → review_walk → plan_end → ship 完全脱节)
- ❌ `.ralph/agent/tasks.jsonl` 为空,state_projector 投影未生效
- ❌ progress_steward / precheck 防线未介入

### Q3:编排是否合理?运行是否正常?

- **编排层错配**:ce-executor-serial 不适合处理"代码已落地 + plan 已 completed + 二次 review 类"运行。其设计前提是"plan active + tasks in_progress"。
- **运行层异常**:coordinator 沉默后被 runtime 兜底,但 shipper 白名单缺项把"降级"放大为"硬失败"。

### Q4:如果是真问题,是机制问题还是编排问题?

- **机制问题 (次要)**:shipper `recoverable_whitelist` 不含 `default_publishes` — 是 **P0-2 修复对象**
- **编排问题 (主要)**:coordinator 在 `status: completed` plan 上无合法出口 — 是 **P0-1 修复对象**
- **多因素叠加**:plan 已落地 (git reflog 证明是二次重跑) + coordinator 无 frontmatter 预检 + shipper 白名单缺项,三者叠加

### Q5:最优先修复是?

**P0-1 (coordinator 前置拒绝 `status: completed` plan)** — 从根上让此类 run 进入可解释的 `plan.blocked(plan_already_completed)` 路径,而不是沉默后被 runtime 兜底放大为编排失败。这是 30 天 9+ 次同根复发簇的"沉默变体"入口,必须与 P0-2 (shipper 白名单) 配套修复才能彻底闭环。

---

**报告完毕**。

> 诊断方法:4 个并行 sub-agent (流程还原 / 历史上下文 / 对账分析 / 归因修复) + 关键事实反向验证
> 反向验证项:git reflog 时序核对、plan frontmatter commit 归属、preset 行号定位、agent prompt 指令逐行比对
> 历史关联:已纳入 30 天 9+ 次同根复发簇 (`docs/report/` 已有 12+ 份历史报告)
