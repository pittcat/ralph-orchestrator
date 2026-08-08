---
title: parallel-forge Loop `primary-20260808-021642` 运行链路诊断报告
date: 2026-08-08
type: diagnosis
loop_id: primary-20260808-021642
preset: builtin:parallel-forge
plan: docs/plans/2026-08-08-001-feat-nowledge-plugin-lifecycle-plan.md
run_dir: /Users/pittcat/Dev/Rust/ralph-orchestrator
status: 业务 4 波中 3 波 accepted 落地,U04 拒收后 correction round-0 完成;reviewer re-review 已启动但未产出业务事件,随后用户 RPC Abort 主动停止;终态未落 `forge.exec.development.done` / `forge.finalized` / `forge.report.done`
diagnostics_mode: MINIMAL
history_search: preset-only
execution_capabilities: [supervisor, wave]
---

# parallel-forge Loop `primary-20260808-021642` 运行链路诊断报告

> 生成时间:2026-08-08
> 诊断对象:`/Users/pittcat/Dev/Rust/ralph-orchestrator/.ralph/`。
> 对照 preset:`presets/en/parallel-forge.yml` + `presets/schemas/parallel-forge.yml`。
> 历史检索:`preset-only` —— 30 天滑动窗口,只看 `docs/{report,solutions,plans,brainstorms}/` 中与 `parallel-forge` / `knowledge` / `hat_channel_empty_after_activation` / `review failed` 相关条目(Phase 1B 报告)。
> 诊断模式:`MINIMAL`(可读取 orchestration events + flows + 业务 artifacts;缺 FULL agent-output,无法逐 tool-call 复演)。

## 0. 产物盘点(Phase 0)

`execution_capabilities: [supervisor, wave]`。

- `supervisor`:`presets/en/parallel-forge.yml` 启用了 `event_loop.supervisor.enabled: true`,且 `.ralph/supervisor.db` 落盘(4096 B 头 + 964 KB WAL)。
- `wave`:events 含 `exec.unit.ready` / `exec.unit.done` / `exec.wave.complete` / `forge.wave.{prepare,worktrees.ready,reviewed,integrated,verified,settled,review.failed}`,且 `wave_id` 字段在 4 波内一致使用。

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` | 是 | 1 个指针 | 唯一主事件文件 `.ralph/events-20260808-021642.jsonl` |
| S | 指向的 events | 是 | 39 行 | 含 forge.start → forge.correction.done;无 `forge.exec.development.done` / `forge.finalized` / `forge.report.done` / `LOOP_COMPLETE` |
| S | `.ralph/recovery.jsonl` | 是 | 2 行 | `repair_dispatch` envelope:exec.unit.done(U01)、exec.unit.done(U04) |
| S | `.ralph/ledger.jsonl` | 是 | 29 行 | `loop.batch_sync` ×29;iteration 1→29 |
| S | `.ralph/agent/tasks.jsonl` | 是 | 8 行 | U01/U02/U03 closed,U04/U05/U06 open;4 个 wave slot task closed |
| S | `.ralph/flow-authority.jsonl` | 是 | 29 行 | 末尾 step=`development_loop`,topic=`forge.correction.done` |
| S | `.ralph/loops.json` | 是 | 1 loop | pid 88523(worktree-path = 主仓根) |
| A | `.ralph/forge/.../inspection-report.md` | 是 | plan_usable_with_findings,F1-F4 amendment list | 含 3 个 P0 + 1 个 P2 |
| A | `.ralph/forge/.../execution-plan.yml` | 是 | 6 units × 4 waves(U01..U06),base = `6b8e5150` | unit U01/U02/U03 各 wave 单独执行 |
| A | `.ralph/forge/.../reviews/summary.md` | 是 | wave1/2/3 ACCEPTED;wave4 REJECTED(U04 P0 race fix landed + 新发现 P1 `memory.py:205 record_save`) | 阻断 integration FF |
| A | `.ralph/forge/.../units/U0{1..4}-completion.md` | 是 | 4 份 | U04 内容哈希含 `memory_writer.py:db8a24...` 等 |
| B | `.ralph/diagnostics/logs/ralph-...log` | 是 | 2 个 log(401/397),401 包含 28+ orphan-emit 与最后一次 channel-routing-fallback | TUI 子进程(401)+ 后端(397) |
| B | `.ralph/diagnostics/orphan-emit-*.md` | 是 | 28 份 | 02:47 首次出现,05:02:57 最后一次;orphans 落在 `forge-u01` 与 `primary-exec-w-3-0` 两棵 worktree |
| B | `.ralph/diagnostics/channel-routing-fallback-*.md` | 是 | 1 份(05:02:57,hat=reviewer) | reviewer hat-channel 0 字节 |
| B | `.ralph/diagnostics/2026-08-08T10-16-42/` | 是 | recovery.jsonl 1 行(agent_doc_sync synced=0) + trace.jsonl 13 行 + active-activations=[] | diagnostics session dir,内层本子层有内容但未持续刷新 |
| B | `.ralph/agent/.ralph-enforce-current-unit` | 是 | 2 字节 | enforcer 文件 |
| B | `.ralph/agent/accepted-transitions.jsonl` | 是 | 14249 字节 | runtime 投影 transitions |
| B | `.ralph/agent/plan-baseline-...sha` | 是 | plan baseline 锁定 | |
| C | `.ralph/supervisor.db` | 是 | supervisor capability ledger | 已使用 |
| C | `.ralph/wave-channels/` | 是 | 空目录 | dispatcher 未在主仓落 hat-channel |
| C | `agent/memories.md` | 是 | 4 条 shared memories(双进程 race / .venv / --schema / pytest abs path) | 全部为 fix-pattern 类 |
| C | `agent_doc_sync.json` | 是 | `synced=0,skipped=2,last_success_at=null` | Nowledge 未挂上,详见 §5 |
| C | `.worktrees/forge-...-u0{1..4}` | 是 | 4 个 worktree,已 detached | 4 个对应 unit 分支 |
| C | `.worktrees/primary-exec-w-{1..4}-0` | 是 | 4 个空 worktree(branch 同 `ralph/primary-exec-w-*-0`,HEAD 仍 `6b8e5150`) | 未实际写盘 |
| C | `.worktrees/forge-2026-08-08-001-feat-nowledge-plugin-lifecycle-plan` | 否 | 缺位 | integration branch 未在工作区出现 |

### 0.1 历史检索开关(per SKILL §0.1)

启用 `preset-only`。已扫描 `docs/report/`、`docs/solutions/`、`docs/plans/`、`docs/brainstorms/` 近 30 天与 parallel-forge / knowledge / hat_channel_empty_after_activation / review-failure 相关的条目。结果详见 §3 与 §5。

### 0.2 历史检索 vs ssot-guardrails

未引用 `hat_handoff` / `loop_state_snapshot.json` / 错误 CLI 路径;未把"历史里说修过"当作"已经修过"的免证。`compound_patterns[*].fix_landed=false` 仅用于 `channel routing fallback`(详见 §3),其它三种 pattern fix_landed=true 的依据是 Phase 1B 在最近 30 天 parallel-forge 报告中未再观察到同样信号,作为"近期未复发"记录,不作为"已修"的强证明。

## 1. 结论摘要

### 1.1 健康度

- 判定:**业务进度正常(U01-U03 accepted 集成),U04 correction round-0 已完成;随后 reviewer re-review activation 结束时没有业务事件,hard gate 重试后被用户主动 abort**;终态未达,无 plan.blocked / work.failed。
- P0:2(置信度 ≥ 70);P1:2(置信度 ≥ 60)。
- 最高优先级根因置信度:**P0-1 = 80/100**。
- 历史复发:`hat_channel_empty_after_activation` 30 天内共 30 次报告命中,自 2026-08-01 起 parallel-forge 报告未再观察到,本次是 2026-08-08 首次回归;`channel routing fallback` 自 2026-07-22 起持续 12 份报告出现,本次也复现。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规? | ⚠️ | MINIMAL 模式可读 orchestration events 与 flow-authority;OPAC U0-U8 四个 gate(emit-precheck / wave-verify / required-fields / hard-gate)可见 1 次 reviewer `Hard gate triggered: hat has publish obligation but emitted no event` 命中;5 次 reviewer activation 中 4 次正常 emit、1 次 channel 空触发 fallback | 65 |
| Q2 | 基座机制是否正常生效? | ✅ | supervisor fan-out 4 个 wave slot 全部 closed;`forge.wave.{reviewed,integrated,verified,settled}` 3 轮完整落地;correction round-0 的 `forge.correction.done` 路径闭合 | 80 |
| Q3 | 编排是否合理、正常运行? | ✅ | preset 已声明 `forge.correction.done` 触发 reviewer;日志显示 reviewer backend 已在 04:27:18 启动。失败发生在 activation 无业务事件之后,不是 re-review 未调度 | 90 |
| Q4 | 问题归因:机制 vs 编排 vs agent? | **主要是 reviewer agent/backend activation 未产出事件;机制正确拦截但诊断不足** | U04 初始 writer race 已由 `53801c23` 修复;当前阻断由 reviewer 空 channel + hard gate 触发;cwd-drift orphan 是独立 P1 信号,非本次终止的已证实主因 | 90 |

### 1.3 根因一句话

U04 初始 writer 的跨进程去重竞态触发 `forge.wave.review.failed`;`53801c23` 完成 round-0 修复后,reviewer re-review 已被启动,但该 activation 结束时 isolated event channel 为空,未产生 `forge.wave.reviewed` 或 `forge.wave.review.failed`;runtime 触发 hard gate 并重试,随后用户主动 Abort。reviewer 空 channel 的更底层原因(未 emit、emit 拒绝、backend 结束或 timeout)缺少 FULL activation 输出,不能继续臆测。(主链置信度 **92/100**,底层 agent 原因 **75/100**)

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| 首轮终态 | 4 wave 全部 settle;最后状态 `forge.correction.done`(correction round-0);无 `forge.exec.development.done` / `forge.finalized` / `forge.report.done` / `LOOP_COMPLETE` |
| 恢复状态 | `.ralph/recovery.jsonl` 仅 2 行 `repair_dispatch`(exec.unit.done U01/U04);无失败恢复;recovery 通道未触发阻塞 |
| 最终代码状态 | integration_branch HEAD = `3630cdba`(wave-3 U03 commit,U04 review-failed 后 Integrator 暂禁 FF per `reviews/summary.md`);4 个 forge-...-u0{1..4} worktree 已 detached,commits 仍在分支 |
| 一致性告警 | ⚠️ `.ralph/agent/tasks.jsonl` 中 U04/U05/U06 仍 open,但 U04 实际提交在 forge-u04 worktree 上,代码 commit ≠ task ledger 状态;`reviews/summary.md` 标记"已审 Units:4, 已接 ACCEPTED:3, 已排 REJECTED:1" 与 task ledger 状态一致(U04 未 close) |
| 用户主动停止 | `.ralph/diagnostics/logs/...401.log` 05:03:20 显示 `RpcDispatcher received Abort` → `Runtime interrupt received, sending SIGTERM to process group` → `process_tree Sending SIGKILL to survivor`;**非故障 abort** |

### 1.5 Prompt visibility 对账

`ralph -c builtin:parallel-forge inspect prompt --hat reviewer --format json`:

- `auto_inject`:`ralph-tools-opac`
- `on_demand`:`ralph-tools-emit`(emit precheck 用)

无 auto/on-demand 矛盾,无 skill 内容泄漏内部实现。本次不把"agent 看不到 emit skill"列为根因。MINIMAL 模式下无法证明 reviewer 实际 activation 是否按要求执行 `ralph emit --policy-check`。

## 2. 执行链路对比

实际主账本链路(events-20260808-021642.jsonl):

```text
forge.start
  → forge.plan.inspected
  → forge.plan.ready
  → forge.concurrency.approved
  → forge.worktrees.ready
  → exec.unit.ready
  → exec.unit.done
  → exec.wave.complete
  → forge.wave.reviewed
  → forge.wave.integrated
  → forge.wave.verified
  → forge.wave.settled
  → forge.wave.prepare
  → forge.wave.worktrees.ready
  → exec.unit.ready
  → exec.unit.done
  → exec.unit.done
  → exec.wave.complete
  → forge.wave.reviewed
  → forge.wave.integrated
  → forge.wave.verified
  → forge.wave.settled
  → forge.wave.prepare
  → forge.wave.worktrees.ready
  → exec.unit.ready
  → exec.unit.done
  → exec.wave.complete
  → forge.wave.reviewed
  → forge.wave.integrated
  → forge.wave.verified
  → forge.wave.settled
  → forge.wave.prepare
  → forge.wave.worktrees.ready
  → exec.unit.ready
  → exec.unit.done
  → exec.wave.complete
  → forge.wave.review.failed
  → forge.correction.requested
  → forge.correction.done
  → reviewer activation 结束但 channel 为空
  → hard gate 重试 reviewer
  → 用户 Abort
```

preset 期望的成功链路:

```text
forge.correction.done
  → forge.wave.prepare (correction re-review)
  → forge.wave.worktrees.ready
  → exec.unit.ready (corrected U04)
  → exec.unit.done
  → exec.wave.complete
  → forge.wave.reviewed         ← 期望 reviewer re-review
  → forge.wave.integrated       ← Integrator FF correction candidate
  → forge.wave.verified
  → forge.wave.settled
  → forge.exec.development.done
  → forge.full.verified
  → forge.audit.done
  → forge.finalized
  → forge.cleanup.done
  → forge.report.done
  → LOOP_COMPLETE
```

关键差异:`forge.correction.done` 之后 reviewer 已被 dispatch,但 activation 没有产生业务事件。runtime 因空 channel 触发 hard gate 并重新启动 reviewer;用户在重试 activation 约 22 秒后 Abort,因此没有足够证据判断 agent/backend 未 emit 的最后原因。

## 3. 历史关联(preset-only,30 天窗口)

Phase 1B 扫描结果(经过去重与加权):

| 模式 | 报告数 | 首次 | 末次 | 是否仍在复发 |
|---|---:|---|---|---|
| `hat_channel_empty_after_activation` | 10 | 2026-07-22 | 2026-08-04 | **本次 2026-08-08 再次出现(reviewer 1 次)**;parallel-forge 自 2026-08-01 起 4 份报告未观察到 |
| `orphan-emit` (cwd-drift) | 4 | 2026-07-23 | 2026-07-26 | 本次 28 份 orphan-emit 集中爆发,是单 run 内首次记录密集度 |
| `U4/U5 review-failure correction cycles` | 7 | 2026-07-22 | 2026-07-27 | 本次(2026-08-08)同模式:U04 race-fix round-0 → re-review 排队,但本次未发 re-review 即被 abort |
| `channel routing fallback` | 12 | 2026-07-22 | 2026-08-07 | 本次 1 份(reviewer @ 05:02:57) |

Knowledge plugin 系列 plans(`docs/plans/2026-08-07-010` / `011` / `2026-08-08-001`)在历史 solutions 中未出现;`docs/solutions/database-issues/emission-store-concurrent-open-race.md` 与 `docs/solutions/state-management/proposal-state-projection-design-walkthrough-v3.md` 涉及 `nmem` 字面,但与本次 U04 race 不是同源(本次是 `memory.py:205` record_save 早于 writer,而 emission-store race 是 DB open 并发)。

无历史关联:`memory.py:205`、`RALPH_NOWLEDGE_*` env 变量、`reviewer.no-event` 精确短语 —— 三个信号在 30 天报告窗口内从未出现。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|---|---|---|---|---:|---|---|
| DEV-001 | U04 `memory.py:205 record_save` 在 writer 失败前抢占落盘,违反 FAILED_OPEN 重试契约 | `reviews/summary.md` wave-4 段落 P1;`agent/memories.md` mem-1786162506-aed8;`units/U04-completion.md` 内容哈希 | P0 | **80** | file:line(+25)、双账本(events + tasks)+15、preset 行号 +15 | 无 FULL agent-output,不能确认具体 tool-call |
| DEV-002 | reviewer re-review activation 已启动,但结束时 channel 为空,随后 hard gate 与用户 Abort | `parallel-forge.yml:895-911`;`401.log` 04:27:18 reviewer backend spawn、05:02:57 empty channel/hard gate、05:03:20 RPC Abort | P0 | 92 | preset trigger + backend spawn + hard gate + Abort 时间线 | 缺 FULL agent-output、backend exit code、stderr,无法确认空 channel 的最后原因 |
| DEV-003 | hat subprocess cwd-drift 致 28 份 orphan-emit + 1 份 reviewer hat-channel 0 字节 fallback;reviewer 触发 `Hard gate triggered: hat has publish obligation but emitted no event` | `hat_channel.rs:79-88`;`execution.rs:66-115`(无 cwd reset,env 仅 RALPH_EVENTS_FILE path);28 份 `orphan-emit-*.md` 02:47 → 05:02:57;`channel-routing-fallback-2026-08-08T05-02-57.md`;`401.log` 05:02:57 hard gate 行 | P1 | 75 | file:line(+25)、双账本(+20)、preset 行号 +15 | orphan-emit 历史"近期未复发"基于最近 4 份 parallel-forge 报告,**不**作为修复保证 |
| DEV-004 | Nowledge bridge env contract(`RALPH_NOWLEDGE_*` 8 项)不存在 → Nowledge plugin 未挂载 | `execution.rs:66-115` env list 实际只注入 6 个;`agent_doc_sync.json` synced=0 skipped=2;plan §5.3 F2 finding;`inspect prompt reviewer` 不含 knowledge skill | P1 | 65 | file:line(+25)、双账本(agent_doc_sync + memory ledger)+15 | Plan 自身已标 F2;本次不要求落地,只确认"未启动 = 未产生故障" |

### 4.1 OPAC 逐 hat 审计(MINIMAL)

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| reviewer | ✅ | ⚠️ | ✅ | ⚠️ | 4 wave review + 1 correction review(预)事件齐全;最后 1 次 reviewer 命中 hard gate(publish-obligation-no-event) | 65 |
| wave-fixer | ✅ | ⚠️ | ✅ | ✅ | correction.requested → correction.done 单 cycle 完整;不允许_paths 收紧 | 65 |
| exec-integrator | ✅ | ⚠️ | ✅ | ✅ | 4 wave 的 integrate / verify / settle 全部完成 | 60 |
| worktree | ✅ | ⚠️ | ✅ | ✅ | 4 wave 的 worktrees.ready + 4 forge-u0{1..4} 真实分支 | 65 |
| inspector / planner / guardian | ✅ | N/A | N/A | ✅ | forge.start 链路 4 个事件齐全 | 70 |
| forge-dispatcher | ✅ | ⚠️ | ✅ | ⚠️ | correction.done 后 reviewer backend 确实启动;但 reviewer activation 无业务事件,触发 hard gate | 90 |

MINIMAL 上限说明:未观察到 `policy-check` 记录不能单独证明 agent 违规;只有 runtime 的明确拒收证据(hard gate log)用于归因。

### 4.2 终态时序补遗

| # | 事件 | 时间 | 主账本序号 |
|---|---|---|---:|
| 1 | loop start | 02:16:42Z | — |
| 2 | plan inspect/ready/concurrency/approved | 02:19 → 02:24Z | 1-4 |
| 3 | wave 1 (U01) settle | 02:25Z | 5-11 |
| 4 | wave 2 (U02) settle | 02:47Z | 12-18 |
| 5 | wave 3 (U03) settle | 02:56Z | 19-25 |
| 6 | wave 4 (U04) review.failed → correction.requested → correction.done | 04:08 → 04:16 → 04:27Z | 26-29 |
| 7 | reviewer hat-channel 0 字节 fallback + orphan-emit(05:02:57) | 05:02:57Z | (post 29;runtime diagnostic) |
| 8 | user RPC Abort | 05:03:20Z | (kill loop) |

事件序号与 `loop.batch_sync` ledger 一致,所有业务事件 timestamp 在 5 分钟以内合理分布;无 evidence of lost/dropped events in main ledger。

## 5. 问题归因表

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| P0 | U04 producer payload race:U04 `memory.py:205` 的 `record_save` 早于 writer 失败回调,违反 `FAILED_OPEN` / `UNKNOWN` 重试契约,reviewer 拒收;correction round-0 已落 commit `53801c23` 但 re-review 未发 | preset + agent(agent 主体) | **80** | DEV-001 | file:line(+25)+15+15 | `U4 review-failure correction cycles` 30 天 7 例,本次同模式 | 第 1 轮:reviewer review + U04 completion + memory ledger 三方交叉;第 2 轮:`reviews/summary.md` correction-round 摘要确认 |
| P0 | reviewer re-review 已派发但 activation 无业务事件,hard gate 重试后用户 Abort 阻断后续 4-6 业务事件 | agent/backend activation + mechanism observability | 92 | DEV-002 | preset trigger + spawn + empty channel + hard gate + Abort 时间线 | 30 天 `hat_channel_empty_after_activation` 有历史复发;本次缺 FULL output |
| P1 | hat subprocess cwd-drift:`forge-u01` 与 `primary-exec-w-3-0` 子目录写 `.ralph/events.jsonl` 致 28 份 orphan-emit;reviewer hat 在 05:02:57 channel 空 → `Hard gate triggered` | mechanism(已落)+ agent(不遵守) | 75 | DEV-003 | file:line(+25)+双账本(+20)+preset +15 | `hat_channel_empty_after_activation` 30 天 30 例,parallel-forge 报告自 2026-08-01 起未观察,本次回归 | 第 1 轮:`hat_channel.rs:79-88` + `execution.rs:66-115` 源码;第 2 轮:orphan-emit 路径均落在 worktree 子目录,确认 cwd 漂移 |
| P1 | Nowledge bridge env 不存在 → plugin 未挂载(`agent_doc_sync` synced=0) | preset / plan 自身 | 65 | DEV-004 | file:line(+25)+双账本 +15 | `RALPH_NOWLEDGE_*` 在 30 天报告窗口零命中,本次系计划 F2 确认 | 第 1 轮:plan §5.3 + F2 finding + execution.rs 源码 |

### 5.1 归因边界

- DEV-001 主因归 agent(producer payload 选择 / record_save 调用顺序),但其**根因契约**(FAILED_OPEN 必须可重试)在 U04 completion 与 reviewer review 中均被显式列出;若重新 dispatch 一个 producer 在 retry 时同样命中,根因将向"preset contract 边界"迁移。
- DEV-002 主因不归 preset orchestration:源码和日志已确认 reviewer 被重新调度。当前只能归因到 reviewer activation 未产出事件;更底层 agent/backend 原因需新增诊断字段后复现确认。
- DEV-003 已确认机制(hat_channel.rs:79-88 fallback + execution.rs:66-115 cwd 由 hat 自管)层面已落,但 hat agent 不遵守 cwd 规则 → 归类为 mechanism+agent 复合根因;**不可单归 preset**,因为 preset 没有对 cwd 的硬约束。
- DEV-004 是本次 run 未启动 plugin 的事实陈述,不归为故障;保留为"knowledge plugin lifecycle 计划在本次 run 中未触发"的提示。

## 6. 修复建议

### 6.1 短期(operator workaround)

- 不要再手工 `git rm` / `git worktree remove` 已 detach 的 4 个 forge-u0{1..4} 工作区;保留 commit 证据(尤其 `53801c23` U04 correction round-0 与 `3630cdba` wave-3 U03 integrate 候选)。
- 4 个 `primary-exec-w-*-0` 空 worktree(HEAD 仍 `6b8e5150`)可直接 `git worktree remove` 清理;不携带业务内容。
- 不要把 manager report 内的 "wave-1/2/3 ACCEPTED" 解读为"loop 完成";task ledger U04/U05/U06 仍 open 是唯一可信源。
- 若需继续推进,**不要**直接补发 `forge.wave.reviewed`;re-ralph 该 plan 走 `task.resume` 通道(参见 plan F1-F4 amendment:Inspector 已声明 plan_usable_with_findings,F1 设计文档未 commit、F2 env contract 待修、F3 contract test 修订)。

### 6.2 中期(preset / schema / instructions)

- 不修改 `parallel-forge` 的 correction/re-review 拓扑: `reviewer.triggers` 已包含 `forge.correction.done`,BDD 场景也已覆盖 re-review 链。
- 修正 integrator 的 `settled_task_ids` / `settled_unit_ids` 类型(参考 2026-08-05-133322 报告 DEV-001 的同源问题;本次未观察到相同信号,但形态相同)。
- 在 hat instructions(`forge-u01..u04` 路径相关的 executor / wave-fixer / reviewer)加入"`ralph emit` 前先 `cd $RALPH_WORKSPACE_ROOT` 或显式 unset 其它 cwd";`inspect prompt` 已确认 hat 看不到 `RALPH_WORKSPACE_ROOT`(execution.rs:180 只把 `workspace_root` 注入 `PtyConfig`,不注入 env),这是本轮 hat-channel cwd-drift 的根本来源。

### 6.3 长期(机制 / 底座)

- `crates/ralph-cli/src/loop_runner/hat_channel.rs` 当前对 `hat_channel_empty_after_activation` 只能记录空 channel;本次已在 `inner.rs` 增加 backend termination、watchdog、stdout 长度和 `ralph emit` 痕迹诊断,用于区分 agent 未 emit、emit 被拒绝与 backend 结束。保留现有 hard gate 重试,不伪造 review 结果。
- `crates/ralph-cli/src/loop_runner/execution.rs:66` 注入 env 时,把 `RALPH_WORKSPACE_ROOT` 也注入 hat subprocess,并在 hat_channel.rs 的 `scan_orphan_subtree_events` 中以 `RALPH_WORKSPACE_ROOT` 为白名单锚(而不是 depth-bounded 扫描);消除 worktree 子目录写 `.ralph/events.jsonl` 的根因。
- 在 preset_lint 增加"hat instructions 显式声明 env var 引用 + skill name"的可见性约束(参考 `.cursor/rules/multi-hat-isolation.mdc` 的"hat 视角"硬规则),把 `RALPH_WORKSPACE_ROOT` / `RALPH_EVENTS_FILE` 这类全局 env 显式列入 hat `instructions:`。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| reviewer 空 channel 的最后原因是未 emit、emit 拒绝、backend 结束还是 timeout? | 75 | 本次缺 FULL agent-output、backend exit code、stderr | 已确认 reviewer spawn、35 分钟后 empty channel/hard gate、重试后用户 Abort |
| 28 份 orphan-emit 中 4 个不同路径(`forge-u01` 重复 + `primary-exec-w-3-0` 出现 1 次)是否对应不同 hat 的不同 cwd 漂移场景 | 55 | 仅看过 `orphan-emit-2026-08-08T05-02-57.md` 的路径;其它 27 份未逐文件读 | 已对照源 timestamps 与 wave 节律;可推断为 4 个 wave 期间 hat 反复进入 worktree 子目录 |
| reviewer hat-channel 0 字节的真正原因是 backend 退场前没写 channel、channel marker race,还是 hat 自己放弃 emit | 50 | 缺 FULL activation 输出 | 已读 `hat_channel.rs:79-88` 与 `401.log` 05:02:57 hard gate 行,无法再下沉 |
| `agent_doc_sync.json` `synced=0, skipped=2` 是否代表 plugin 评估因 cwd-drift 漏发,还是 plugin 完全未启动 | 60 | 已确认 `execution.rs` 不注入 `RALPH_NOWLEDGE_*`,倾向后者,但未直接读 `agent_doc_sync` 实现 | 已读 plan F2 + execution.rs |

## 附录 A. 报告/解决方案/计划参考

来自 Phase 1B 历史扫描的强相关条目(用于 §3 / §5 / §6 引用,不重复列出):

- `docs/report/2026-07-30-parallel-forge-primary-20260730-094057-diagnosis.md` —— 30 天内 `hat_channel_empty_after_activation` 最高密度(9 例),形态相似。
- `docs/report/2026-08-04-merge-batch-primary-20260804-053651-diagnosis.md` —— 最近一次 `hat_channel_empty_after_activation`(2026-08-04,merge-batch preset)。
- `docs/report/2026-08-05-parallel-forge-primary-20260805-133322-diagnosis.md` —— settlement payload 类型错,CloseTaskBatch 拒收,5 个 Unit task 保持 open。本次未观察到同源信号,但同一类编排风险。
- `docs/solutions/database-issues/emission-store-concurrent-open-race.md`、`docs/solutions/state-management/proposal-state-projection-design-walkthrough-v3.md` —— 涉及 `nmem` 关键字,但与本次 U04 race 不是同源。
- `docs/plans/2026-08-07-010-feat-nowledge-ralph-plugin-plan.md`、`docs/plans/2026-08-07-011-feat-ralph-nowledge-runtime-adapter-plan.md` —— knowledge plugin 上游与并行计划。

## 附录 B. 本报告未触及的 ssot 禁止项

- 未引用 `hat_handoff`、`loop_state_snapshot.json`、错误 CLI 路径。
- 未把"agent 看不到某 skill"作为独立根因(已对账 reviewer `inspect prompt`)。
- 未把"`agent_doc_sync` skipped=2"等同于"plugin 失败"——前者是"plugin 未挂载",后者是"plugin 挂载但失败"。
