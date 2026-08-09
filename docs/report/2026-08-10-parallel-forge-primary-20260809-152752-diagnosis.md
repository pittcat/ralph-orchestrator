---
title: parallel-forge Loop `primary-20260809-152752` 运行链路诊断报告
date: 2026-08-10
type: diagnosis
loop_id: primary-20260809-152751
preset: builtin:parallel-forge
plan: docs/plans/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md
run_dir: /Users/pittcat/Dev/Rust/ralph-e2e
status: backend 执行失败导致 worktree 未完成；阻塞终态被接受，但随后 tasks/handoff 产物出现终态后修改，业务交付不可据此判定成功
diagnostics_mode: LOGS_ONLY
history_search: preset-only
execution_capabilities: [supervisor, wave]
---

# parallel-forge Loop `primary-20260809-152751` 运行链路诊断报告

> 生成时间：2026-08-10
> 诊断对象：`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/`
> 对照：`presets/en/parallel-forge.yml`、`presets/schemas/parallel-forge.yml`
> 历史范围：`preset-only`，近 30 天内与 `parallel-forge`、该 plan、hat-channel 空路由及 open-task completion 相关的记录。
> 报告仓库：`ralph-orchestrator` 主仓。

## 0. 产物盘点（Phase 0）

`execution_capabilities: [supervisor, wave]`。

- `supervisor`：preset 的 `event_loop.supervisor.enabled: true`（`presets/en/parallel-forge.yml:162-164`），且 `.ralph/supervisor.db` 存在。
- `wave`：preset instructions 含 `ralph wave emit` / `ralph wave verify`（`presets/en/parallel-forge.yml:643-697`）；本次 tasks 与 supervisor 账本均存在。

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` | 是 | 1 个指针 | 唯一可信 events 为 `.ralph/events-20260809-152752.jsonl` |
| S | 指向 events | 是 | 8 行 | `forge.start` → plan/approval → cleanup → `forge.report.done(BLOCKED)` → 两个 `LOOP_COMPLETE` |
| S | `.ralph/ledger.jsonl` | 是 | 9 行 | 含 completion request ×2 与最终 completion honored |
| S | `.ralph/recovery.jsonl` | 否 | 0 | 无 workspace recovery 记录 |
| A | `.ralph/agent/tasks.jsonl` | 是 | 5 行 | 初始均 open，之后全部 closed |
| A | `.ralph/agent/summary.md` / `handoff.md` | 是 | 终止后生成 | 与主 events 的 BLOCKED 业务 verdict 不一致地写成成功 |
| B | `.ralph/supervisor.db` | 是 | 存在 | supervisor capability 要求满足 |
| B | `.ralph/diagnostics/logs/` | 是 | 5 个日志 | 无 orchestration/agent-output，模式为 LOGS_ONLY |
| B | channel fallback diagnostic | 是 | 1 份 | `worktree` 的 `hat_channel_empty_after_activation` |
| C | `.ralph/forge/<plan-key>/` | 是 | 5 份核心 artifact | inspection、development/execution plan、approval、cleanup 均已落盘 |

诊断盲区：LOGS_ONLY 不能逐 tool-call 证明 agent 的 `policy-check`、wave verify、emit 顺序；OPAC 单项置信度上限为 50，缺 orchestration 不单独构成 P0。

## 1. 结论摘要

### 1.0 重新核查后的根因结论

此前将本次现象概括为“worktree hat 忘记 emit”证据不足，现已根据 runner 原始日志、worktree 清理报告、accepted-transitions 与目标仓库 Git 状态修正结论：

> 本次 `worktree` activation 的 backend 没有成功结束，worktree provisioning 没有完成，因此没有产生 `forge.worktrees.ready`。空 channel 是 backend 失败后的观测症状，不足以证明 agent 已完成 worktree 后忘记 emit。runtime 随后把“backend 失败且无事件”压进了“hat 没有 emit”的通用 hard-gate 日志，造成了错误归因。

当前可以高置信度确认“worktree 未完成”和“不是已成功后单纯忘记 emit”；具体 backend 失败原因仍无法从当前 LOGS_ONLY 产物确定，因为 runner 只保留了 `backend_success=false`，没有把 exit code 和完整 backend output 纳入本次诊断产物。

### 1.1 健康度

- 判定：**部分偏离，阻塞终态与后置成功形态不一致**。
- P0：0；P1：2（均达到 confidence ≥ 60 门槛）。
- 最高优先级：P1-1，置信度 **85/100**。
- 历史复发：是；`hat_channel_empty_after_activation` 与 parallel-forge 的阻塞/终态时序问题均有近期同类记录。

### 1.2 强制四问

| # | 问题 | 答案 | 证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 执行与 OPAC 是否合规？ | ⚠️ | LOGS_ONLY；events 拓扑可还原，但 `worktree` channel 为空并发生 fallback，Precheck/Confirm 无法完整验证 | 50 |
| Q2 | 基座机制是否生效？ | ⚠️ | completion 首次遇到 5 个 open tasks 时被拒收；随后最终 completion honored，但 tasks 是终态后才关闭，时序保护未形成可审计的成功链 | 75 |
| Q3 | 编排是否合理？ | ❌ | approval 后没有 `forge.worktrees.ready`、`exec.unit.ready`、`exec.wave.complete` 或业务执行/审计事件；worktree backend 以失败状态结束后进入无进展/cleanup/report 阻塞路径 | 90 |
| Q4 | 归因是什么？ | **backend activation failure 为主，runtime failure classification 为次；不能归因成“成功后忘记 emit”** | runner 原始日志明确 `backend_success=false`、`output_bytes=81`、`output_mentions_emit=false`；cleanup 明确无 worktree/branch/map；具体 backend 失败原因仍因 LOGS_ONLY 不可证实 | 90 |

### 1.3 根因一句话

本次运行在 `forge.concurrency.approved` 后启动了 `worktree` activation，但 runner 记录该 backend `success=false`，且只得到 81 bytes 输出；随后 isolated channel 为空，因此没有 worktree provisioning、`forge.worktrees.ready` 或 execution wave 事件。runtime 的通用 hard gate 将“backend 失败且无事件”记录成“hat has publish obligation but emitted no event”，连续 3 个无进展 turn 后进入阻塞报告。随后 completion retry 看到 5 个 open tasks 并被 runtime 拒收，但 tasks 在 `forge.report.done(BLOCKED)`/`LOOP_COMPLETE` 之后才全部 closed，导致 summary/handoff 的“成功”描述不能代表首轮 accepted verdict。（backend activation failure 主因置信度 **90/100**；具体 backend failure subtype 未确认）

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| 首轮终态 | `forge.report.done` accepted，payload `final_audit=BLOCKED`、`status=BLOCKED`（events 第 6 行） |
| 恢复状态 | **失败/阻塞终态后恢复形态**：tasks 在 15:46:58–15:47:01 才 closed；此前 15:45:07 已写入 BLOCKED report，且没有后续 accepted 的执行成功事件 |
| 最终代码状态 | handoff/summary 记录 auto-commit `3fa6a67`，但这不能覆盖 BLOCKED 的业务终态；本次没有 `exec.unit.done`、integrated、verified 或 report-success 事件 |
| 一致性告警 | ⚠️ 失败终态后恢复：后置 artifact 被改成成功形态，但无对应 accepted 成功业务事件。 |

## 2. 执行链路对比

| 预期阶段 | 实际证据 | 结果 |
|---|---|---|
| `forge.start` → inspector | events #1–2 | ✅ |
| planner → `forge.plan.ready` | events #3，5 units / 1 wave | ✅ |
| guardian → `forge.concurrency.approved` | events #4 | ✅ |
| worktree → `forge.worktrees.ready` | 无；日志显示 channel 空 | ❌ |
| dispatcher → `exec.unit.ready` | 无 | ❌，executor 未激活 |
| executor/integrator/verifier/tester/auditor | 无业务事件 | ⏸️ 上游缺失 |
| cleanup → reporter | events #5–6 | ⚠️ cleanup 落地，reporter 只产出 BLOCKED |
| terminal | events #7–8；ledger 最终 honored | ⚠️ 终态控制事件存在，但业务报告为 BLOCKED |

未触发 hat：`forge-dispatcher`、`executor`、`exec-integrator`、`verifier`、`tester`、`auditor`、`forge-failure-handler`；直接上游缺失是 `forge.worktrees.ready`。

## 3. 历史问题上下文

| 文档 | 问题类型 | 关联度 | 是否闭环 | 一行摘要 |
|---|---|---|---|---|
| `docs/report/2026-08-08-parallel-forge-primary-20260808-021642-diagnosis.md` | `hat_channel_empty_after_activation` / reviewer 无事件 | 高 | 否，报告标记本次为回归 | reviewer re-review channel 为空，hard gate 后被 Abort；同样缺 FULL output |
| `docs/report/2026-08-05-parallel-forge-primary-20260805-133322-diagnosis.md` | settlement 后 open tasks / terminal 未接受 | 高 | 未证明彻底闭环 | 业务 artifact 完成但 task projection 拒收，后续 report 链断裂 |
| `docs/report/2026-07-30-parallel-forge-primary-20260730-094057-diagnosis.md` | fail-close / `forge.plan.blocked` / reporter terminal mismatch | 高 | 部分修复，仍有相关残留 | 多次阻塞后 reporter 产物与 accepted event 不一致 |
| `docs/report/2026-07-29-parallel-forge-primary-20260729-020808-diagnosis.md` | 成功基线 | 中 | 是业务成功基线 | 同一类 plan 曾完整走完 wave、report done、LOOP_COMPLETE；说明本次不是 plan 天然不可执行 |

本次扫描窗口：preset-only (30d sliding)

## 4. 证据清单

| ID | 描述 | 证据锚点 | 初判 | 初估 | 已计分证据项 | 缺口 |
|---|---|---|---|---:|---|---|
| DEV-001 | approval 后 worktree channel 为空，未产生 `forge.worktrees.ready`，连续无进展后进入阻塞路径 | `.ralph/diagnostics/channel-routing-fallback-2026-08-09T15-39-31.md`；logs `ralph-2026-08-09T23-27-51-897-95135.log:30-33`；events #4–6 | P1 | 75 | file:line(+25)、双账本(log + diagnostic/effects)(+20)、preset 行号(+15) | 缺 FULL agent-output/backend exit reason |
| DEV-002 | `forge.report.done` 为 BLOCKED 后，tasks 才全部 closed，summary/handoff 改写为成功 | events #6–8；ledger #5–9；tasks #1–5；`wave_scope.rs:863-938` | P1 | 85 | file:line(+25)、双账本(events + ledger/tasks)(+20)、Tier C/A 交叉验证(+10) | 缺完整 orchestration accepted-transition 链 |

### 4.1 Prompt visibility 对账

已运行 `ralph -c presets/en/parallel-forge.yml inspect prompt --hat worktree --format json`。`auto_inject` 包含 `ralph-tools`、`ralph-tools-tasks`、`ralph-tools-memories`、`ralph-tools-opac`；`on_demand` 包含 `ralph-tools-cmdref`、`ralph-tools-emit`、`ralph-tools-precheck`、`ralph-tools-recovery-directives`、`ralph-tools-wave`。未见 auto/on-demand 名称矛盾；但 LOGS_ONLY 无法证明 activation 实际加载并执行了 on-demand skill。

### 4.2 OPAC 逐 hat 审计（LOGS_ONLY）

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| inspector/planner/guardian | ✅ | ⚠️ | ✅ | N/A | events #2–4；无完整 tool-call | 45 |
| worktree | ⚠️ | ⚠️ | ❌/未知 | N/A | empty channel fallback；无 `forge.worktrees.ready` | 50 |
| dispatcher/executor | ⏸️ | ⏸️ | ⏸️ | N/A | 未激活 | 30 |
| cleanup/reporter | ✅ | ⚠️ | ✅ | ⚠️ | events #5–8；report final_audit=BLOCKED | 45 |

Confirm 在 LOGS_ONLY 下不可完整验证；未把“未见 policy-check”单独升为 P0。

## 5. 问题归因表

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 | 已计分证据项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| P1 | worktree backend activation 失败，导致 worktree provisioning 未完成、`forge.worktrees.ready` 缺失、后续 wave 未启动 | backend execution failure + mechanism observability | **90** | DEV-001 | runner 原始日志 `backend_success=false`(+25) + cleanup 无 worktree/branch/map(+25) + events/accepted-transitions 缺失下游事件(+20) + preset 触发链(+15) | 高，2026-08-08 同类空 channel 复现 | 第1轮源码+日志；第2轮补查 backend success、Git/worktree 与 cleanup 证据 |
| P1 | BLOCKED accepted 后 tasks/handoff/summary 形成成功形态，终态时序不一致 | mechanism + compound artifact lifecycle | **85** | DEV-002 | `wave_scope.rs:863-938`(+25) + events/ledger/tasks 双账本(+20) + Tier A 交叉(+10) | 高，2026-08-05/07-30 有相近终态断裂 | 第1轮源码+三账本；第2轮终态 chronology 对账 |

## 6. 修复建议

### 6.1 短期

- 将本次 run 标记为业务未完成；不要把 `summary.md`、`handoff.md` 或关闭后的 tasks 当成成功验收证据。
- 对空 channel 的 `worktree` activation 保留原始 backend 输出、退出码、termination 类型和 channel 文件，便于区分 backend 失败、agent 未 emit、emit 被拒收、崩溃与 runner 竞态。

### 6.2 中期

- 在 parallel-forge 的 worktree provisioning 后增加真实 runtime-path BDD：要求 worktree 真实资源未创建时不能接受 `forge.worktrees.ready`，也不能进入成功 reporter 链，并验证 backend failure、missing emit、channel routing failure 三种路径分别分类。
- 将 `forge.report.done` 的 `status/final_audit=BLOCKED` 与后续 summary/handoff 状态绑定；后置 landing 不得把业务 BLOCKED 改写为 completed。

### 6.3 长期

- 强化 accepted event chronology 与 artifact mutation 的一致性门禁：终态 accepted 后若无新的 accepted 成功业务事件，不得关闭业务 tasks 或生成成功 handoff。
- 为 isolated hat-channel fallback 增加可审计的失败分类和 FULL diagnostics 采样，避免每次只能停在“空 channel 的具体原因不可知”。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| worktree backend 的具体失败类型是命令失败、agent 进程异常退出、Claude backend 错误还是 runner race | 35 | 缺 exit code、完整 backend output、orchestration/agent-output | 已确认 `backend_success=false`、`watchdog_timeout=false`、`backend_termination=None`、81 bytes output；不能进一步归因 |
| tasks 在 terminal 后关闭的确切调用路径及是否属于 supervisor ledger 投影延迟 | 55 | 缺 accepted-transition/orchestration 细节 | 已对账 events、ledger、tasks、landing 源码；不作为 §5 主因之外的新增结论 |

## 8. 主仓关键代码引用

- `crates/ralph-cli/src/loop_runner/hat_channel.rs:75-87`：空 hat-channel 产生 fallback diagnostic，但不 fail-close。
- `crates/ralph-cli/src/loop_runner/inner.rs:3482-3500`：空 channel 诊断同时记录 `backend_success=false`、watchdog、termination、output bytes 和是否出现 `ralph emit`；本次日志中 backend_success 为 false。
- `crates/ralph-cli/src/loop_runner/inner.rs:4446-4512`：即时 missing-terminal recovery 需要 `success=true`，但 generic hard gate 只检查“无有效/拒绝事件 + obligation”，因此 backend failure 会被记录成“没有 emit”。
- `crates/ralph-cli/src/loop_runner/execution.rs:276-292`：runner 将 adapter 结果转换为 `ExecutionOutcome` 时保留 success/termination，但没有继续暴露 exit code。
- `crates/ralph-core/src/event_loop/wave_scope.rs:863-898`：completion promise 在 open tasks 存在时拒收并发布恢复信号。
- `crates/ralph-core/src/event_loop/wave_scope.rs:927-938`：只有 completion 被接受后才标记 `completion_honored`。
- `presets/en/parallel-forge.yml:600-697`：dispatcher 依赖 worktree/wave 事件，且要求 wave verify → emit。

---

诊断结论：本次 loop 的控制面最终结束，但业务面没有进入 execution wave；`BLOCKED` 是首轮 accepted 业务 verdict。高置信度根因是 worktree backend activation 失败，空 channel 只是其结果；后续 task closure 与成功 handoff 属于终态后的产物变化，不能证明多排序算法已经交付。当前证据不足以认定 agent 已完成 worktree 后忘记 emit，也不足以确定 backend 失败的具体 subtype。
