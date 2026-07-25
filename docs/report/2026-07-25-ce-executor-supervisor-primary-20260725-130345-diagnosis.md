---
title: ce-executor-supervisor Loop `primary-20260725-130345` 运行链路诊断报告
date: 2026-07-25
type: diagnosis
loop_id: primary-20260725-130345
preset: builtin:ce-executor-supervisor
run_dir: ../ralph-e2e
status: 自动 supervisor exec wave 失败；operator 介入后业务产物完成
diagnostics_mode: LOGS_ONLY
history_search: disabled
---

# ce-executor-supervisor Loop `primary-20260725-130345` 运行链路诊断报告

> **生成时间**: 2026-07-25
> **诊断对象**: `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/`（loop_id=`primary-20260725-130345`）
> **对照 preset**: `presets/en/ce-executor-supervisor.yml` + `presets/schemas/ce-executor-supervisor.yml`
> **执行方式**: Phase 0 主 Agent 盘点；history disabled 后启动流程还原、对账、源码归因三个 sub-agent
> **Diagnostics 模式**: `LOGS_ONLY`
> **history_search**: `disabled`；未读取主仓 `docs/report/`、`docs/solutions/`、`docs/plans/`、`docs/brainstorms/`
> **execution_capabilities**: `[supervisor, wave]`
> **报告仓库**: `ralph-orchestrator` 主仓（非 run_dir）
> **Tier C 根**: 本次业务输入为 run workspace 的 `docs/plans/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md`；执行计划 artifact 为 `.ralph/review/PROMPT.ce-executor-supervisor/execution-plan.yml`
> **置信度规则**: §5 仅收录 confidence≥60；P0 须 confidence≥70；LOGS_ONLY 下纯 agent/OPAC 归因受置信度上限约束

---

## 0. 产物盘点（Phase 0）

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` | ✅ | 指向 `events-20260725-130345.jsonl` | 唯一可信 events 指针 |
| S | `.ralph/events-20260725-130345.jsonl` | ✅ | 14 行 | 本报告唯一读取的主 events |
| S | `.ralph/events-history-20260725-130345.jsonl` | ✅ | 2 行 | 配对旁路文件；未作为编排 SSOT |
| S | `.ralph/ledger.jsonl` | ✅ | 8 行 | 迭代、completion_requested、completion_honored |
| S | `.ralph/recovery.jsonl` | ✅ | 2 行 | 两条 `repair_dispatch`，主题均为 `exec.unit.done` |
| S | `.ralph/loops.json` | ✅ | `{"loops":[]}` | 终止后无 loop 注册项 |
| S | `.ralph/current-loop-id` | ✅ | `primary-20260725-130345` | loop_id 来源 |
| A | `.ralph/agent/tasks.jsonl` | ✅ | 10 行 | 5 个业务 task closed；5 个 supervisor slot task failed |
| A | `.ralph/agent/progress.md` | ❌ | 条件未满足 | preset 未产生 state projection progress artifact，不判故障 |
| A | `.ralph/agent/summary.md` | ✅ | 24 行 | 最终摘要声称 106 passed |
| A | `.ralph/agent/handoff.md` | ✅ | 58 行 | 仍列 5 个 supervisor slot 为 Remaining，和 tasks.jsonl 冲突 |
| B | `.ralph/diagnostics/logs/ralph-*.log` | ✅ | 2 个日志 | LOGS_ONLY 主诊断证据 |
| B | `.ralph/diagnostics/agent_doc_sync.json` | ✅ | JSON | 文档同步结果；无 timestamp session/orchestration.jsonl |
| B | `.ralph/diagnostics/<timestamp>/orchestration.jsonl` | ❌ | 条件未满足 | Diagnostics 模式为 LOGS_ONLY，跳过 L2 orchestration |
| B | `.ralph/supervisor.db` | ✅ | 106496 bytes | `supervisor` capability 的预期 ledger |
| B | `.ralph/review/PROMPT.ce-executor-supervisor/execution-plan.yml` | ✅ | 92 行 | 5 个 execution nodes，执行计划已生成 |
| C | `.ralph/diagnostics/reporter-primary-20260725-130345.md` | ✅ | 64 行 | reporter 失败终态及 operator 收尾记录 |
| C | `.ralph/agent/memories.md` | ✅ | 存在 | 本次 run 的 workspace 记忆文件 |
| C | `.worktrees/primary-exec-{0..4}` | 终态前存在 | 终态日志记录已移除 | 不作为业务 artifact |

**execution_capabilities 推断**：

- `supervisor`：`ralph-e2e/ralph.supervisor.yml:46-47` 配置 `supervisor.enabled: true`；启动日志还明确记录 `supervisor bridge wired`、数据库路径与并发上限。
- `wave`：主 events 的 `exec.unit.ready` envelope 含 `wave_id=w-rs-1`，日志记录 `Wave detected, executing parallel workers`。
- 因此缺 `.ralph/supervisor.db` 或缺 `wave_id` 均不适用；两者均存在。

**Diagnostics 盲区声明**：本次为 `LOGS_ONLY`，没有 `orchestration.jsonl` 和完整 agent-output；OPAC 单项及仅凭 agent 行为的归因不单独升 P0，源码归因以 events、recovery、日志和 preset 行号交叉支持。

---

## 1. 结论摘要

### 1.1 健康度

- **判定**：自动 supervisor exec wave **部分偏离并失败**，随后由 operator 手动完成 worktree commit/merge 与测试，因此业务结果达成但自动编排路径未闭合。
- **P0 / P1 数量**（均为 confidence≥门槛）：P0 1；P1 2。
- **最高优先级根因置信度**：P0-1 = **82 / 100**。
- **历史复发**：`N/A (history disabled)`。

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 部分合规 | bridge 已 wired，dispatcher 完成 fan-in；但一个 worker 超时、一个/多个 slot 无可确认 result，自动路径转入 `exec.wave.failed`；LOGS_ONLY 无法完整审计每个 hat 的 OPAC | 65 |
| Q2 | 基座机制是否正常生效？ | ✅ 生效但暴露了失败 | supervisor fan-in 返回并注入 `InjectedFailed`；`FlowStepScopeStage` 对非法 `exec.unit.done` 返回 `flow_unknown_emit`（`crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs:169-193`） | 82 |
| Q3 | 编排是否合理、正常运行？ | ❌ 自动路径未闭合 | preset 的 `exec_wave` 期望 `exec.wave.complete`/`exec.wave.failed`，本次只出现失败终态，没有 `exec.wave.complete`；worker 事件的 envelope/payload wave 标识不一致且至少一个 slot 超时 | 82 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **compound：执行波结果/标识传播与 worker 超时导致自动 wave 失败；后续手动 emit 被正确的 FlowStepScope 拒收** | recovery 的 `source=RepairStream`、`repair_dispatch` 和 `flow_unknown_emit` 证据与源码一致；bridge 未初始化假设被启动日志排除 | **82** |

### 1.3 根因一句话

本次不是 supervisor bridge 未初始化：bridge 已成功接线并执行 fan-in；自动 exec wave 中 worker/波次结果的标识与状态没有形成可闭合的 5-slot 结果集合（同时存在 `wave_id` 分层不一致和 worker timeout），于是 supervisor 正确注入 `exec.wave.failed`；operator 后续尝试补发 `exec.unit.done` 时又被当前 flow step 的作用域门禁拒收，最终只能手动整合 worktree 完成业务目标。**根因置信度：82 / 100。**

---

## 2. 执行链路对账

| 顺序 | 时间（UTC） | 来源/hat | topic | 关键事实 |
|---:|---|---|---|---|
| 1 | 13:03:45 | loop-bootstrap | `plan.ready` | 开始本次 5-unit workload |
| 2 | 13:04:52 | coordinator | `work.ready` | 注册 U1–U5 |
| 3 | 13:06:08 | task-planner | `execution.plan.ready` | `.ralph/review/.../execution-plan.yml` 已写入 |
| 4–8 | 13:11:33 | exec-wave-dispatcher | `exec.unit.ready` ×5 | envelope `wave_id=w-rs-1`；payload 内业务 `wave_id=w-246cb4afef33` |
| 9 | 13:14:07 | worker | `exec.unit.done` U3 | envelope `wave_id=w-2`；payload `wave_id=w-246cb4afef33`；16/16 passed |
| 10 | 13:15:34 | worker | `exec.unit.done` U4 | envelope `wave_id=w-2`；payload `wave_id=w-246cb4afef33`；28/28 passed |
| 11 | 13:19:06 | worker | `exec.unit.done` U5 | envelope `wave_id=w-2`；payload `wave_id=w-rs-1`；22/22 passed |
| 12 | 13:19:32 | exec-failure-handler | `exec.wave.failed` | `reason=required_slot_failure`、`blocking_slots=[0,1,2,3,4]` |
| 13 | 13:22:33 | reporter | `LOOP_COMPLETE` | 自动路径 verdict=failed，main 尚无 sorts artifact |
| 14 | 13:27:47 | ralph | `LOOP_COMPLETE` | operator-driven 收尾后声称 106 passed、5 runtime tasks closed |

### 2.1 终态语义

- 自动链路只到 `exec.wave.failed`，没有 `exec.wave.complete`、`work.done`、`plan.complete` 或 review/fix 链路事件。
- reporter 的 `LOOP_COMPLETE` 是自动失败终态；后一个 `ralph` `LOOP_COMPLETE` 是 operator 介入后的人工终态，不能把后者当成自动 supervisor 成功证据。
- `ledger.jsonl` 在 iteration 5 记录 `completion_honored`，证明最终 completion 请求被接受，不证明 supervisor success spine 曾闭合。

---

## 3. 历史问题上下文

> **⚠️ 启动条件**：`history_search=disabled`；未启动历史 Agent，不扫描主仓历史目录。

- 历史关联：`N/A (history disabled)`。
- 本节不对历史复发或跨 run 同构性下结论。

---

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 证据缺口 |
|---|---|---|---|---:|---|
| DEV-001 | 自动 exec wave 因 required slot failure 进入失败终态 | `.ralph/events-20260725-130345.jsonl:11`；`.ralph/diagnostics/logs/ralph-2026-07-25T21-03-45-516-37104.log:31-35` | P0 | 86 | 缺 supervisor DB 的逐 slot 结构化查询锚点 |
| DEV-002 | worker 完成事件的 envelope wave 标识与 dispatcher/业务 payload 不一致 | `.ralph/events-20260725-130345.jsonl:3-10` | P0 | 78 | 无 FULL orchestration artifact，无法进一步还原每层 ID 的生成调用链 |
| DEV-003 | 至少一个 worker 超时，导致结果集合无法满足 5-slot 成功条件 | `.ralph/diagnostics/logs/ralph-2026-07-25T21-03-45-516-37104.log:31-33`；events:11 | P0 | 84 | 缺每个 slot 的 stdout/agent-output |
| DEV-004 | recovery 记录了两次 `exec.unit.done` repair dispatch，且最终 scope reason 为 `flow_unknown_emit` | `.ralph/recovery.jsonl:1-2`；reporter event `.ralph/events-20260725-130345.jsonl:12` | P1 | 74 | recovery 文件为 repair summary，未保留完整 rejected payload |
| DEV-005 | 自动失败后 operator 手动合并并跑出 106 passed | `.ralph/diagnostics/reporter-primary-20260725-130345.md:17-21,39-47`；events:13 | P1 | 95 | 无独立 CI 重跑证据；但收尾报告与 summary 一致 |
| DEV-006 | handoff 仍把 5 个 supervisor slot 列为 Remaining，而 tasks.jsonl 已将其标为 failed | `.ralph/agent/handoff.md:19-25`；`.ralph/agent/tasks.jsonl:6-10` | P1 | 93 | 无后续 resume 读取行为证据 |

### 4.1 OPAC 逐 hat 审计表

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| coordinator/task-planner | ✅ | ? | ✅ | ✅ | `work.ready` → `execution.plan.ready`；未有 FULL agent-output | 55 |
| exec-wave-dispatcher | ✅ | ? | ✅ | ✅ | 5 个 `exec.unit.ready` 同批发出；日志确认 wave detected | 60 |
| worker | ✅ | ? | ⚠️ | ⚠️ | U3/U4/U5 business events 记录测试通过；U1/U2 无对应主 events 完成证据，且 worker 0 timeout | 55 |
| supervisor bridge/fan-in | ✅ | N/A | ✅ | ✅ | 启动 wired；日志 `U6 ... InjectedFailed`；源码 fan-in 分支 | 82 |
| reporter | ✅ | ? | ✅ | ✅ | 失败报告写入并发出 reporter `LOOP_COMPLETE`；完整 OPAC 无法从 LOGS_ONLY 复核 | 60 |
| ralph/operator | ✅ | ? | ✅ | ✅ | 后续 task close、merge、106 passed 见收尾 artifact；不是自动 preset 路径 | 70 |

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---:|
| P0 | supervisor exec wave 未形成可闭合的 5-slot 成功结果集合；worker completion 的 envelope/payload wave 标识不一致，且至少一个 slot 超时，最终注入 `exec.wave.failed` | **compound（结果标识传播/监督器状态 + worker timeout）** | **82** | DEV-001, DEV-002, DEV-003；`dispatcher.rs:781-802`、`phase.rs:130-143` | `N/A (history disabled)` | 2→82 |
| P1 | operator 后续补发 `exec.unit.done` 时被当前 flow step 作用域拒收，恢复链因 `flow_unknown_emit` 结束 | **mechanism（正确门禁）+ agent/operator recovery mismatch** | **78** | DEV-004；`crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs:169-193`；`presets/en/ce-executor-supervisor.yml:56-74` | `N/A (history disabled)` | 2→78 |
| P1 | 自动失败后仍保留相互矛盾的终态/续跑状态：reporter 已失败，ralph 后续成功，handoff 仍列 slot Remaining | **compound（终态叠加 + handoff projection freshness）** | **72** | DEV-005, DEV-006；events:12-13；handoff:19-25 | `N/A (history disabled)` | 1→72 |

> **复合归因说明**：P0 由两部分构成：结果标识/slot 状态未闭合（贡献约 60%，置信度 82）与 worker timeout/缺失结果（贡献约 40%，置信度 84）；整行按较低及交叉证据取 82。`supervisor bridge 未初始化`未列为根因：启动日志明确记录 bridge wired，且 dispatcher 日志出现 `U6: supervisor fan-in tick completed`。

---

## 6. 修复建议

### 6.1 短期（operator workaround）

- **目标**：避免用普通业务 `ralph emit exec.unit.done` 试图补写 supervisor slot 结果。**改动**：遇到 worker timeout 或 slot 状态不完整时，先保留 failure terminal event，让 `exec-failure-handler`/reporter 完成失败闭环；如需重跑，使用新的 loop/worktree，而不是在旧 loop 中伪造 worker completion。**预期效果**：不再触发 `flow_unknown_emit` recovery，避免把自动失败和人工成功叠加在同一 loop。**关联置信度：78**。
- **目标**：收尾前核对主 events 的自动终态与业务测试结果。**改动**：将 operator merge/test 结果作为独立人工收尾记录，不再发第二个与 reporter 同名但语义相反的 loop completion，除非 runtime 明确支持该续跑语义。**预期效果**：终态不会被误读为自动链路成功。**关联置信度：72**。

### 6.2 中期（preset / schema / instructions）

- **目标**：让 dispatcher、worker、supervisor 使用同一个可验证的 wave identity。**改动**：在 `exec.unit.ready` 到 `exec.unit.done` 的契约中明确区分并校验 public wave identity、internal supervisor wave identity 与业务 payload identity；fan-in 认领前拒绝或显式转换不一致值，并把转换结果写入结构化诊断。**预期效果**：完成事件只能归属其实际 dispatch wave，避免已完成 slot 被视为无结果。**关联置信度：82**。
- **目标**：让失败路径的可观察状态与 handoff 一致。**改动**：preset/schema 或终态 writer 在 supervisor `exec.wave.failed` 后生成与 slot failure 状态一致的 handoff projection，明确标注“失败待重跑”而不是 Remaining。**预期效果**：下次 activation 不会把 failed slot 误判成可继续完成的任务。**关联置信度：72**。

### 6.3 长期（机制 / 底座）

- **目标**：保证 supervisor fan-in 对每个 slot 的状态、结果和 public/internal wave ID 原子对账。**改动**：在 `run_supervisor_fan_in` 与 supervisor coordinator 状态转换边界增加结构化一致性检查和可审计 diagnostics artifact；对 `Completed` slot 不得进入 `blocking_slots`，对 timeout/empty result 给出 slot-specific reason。**预期效果**：在注入 `exec.wave.failed` 前即可定位具体未完成 slot，避免 `[0,1,2,3,4]` 这种无法区分已完成与阻塞项的宽泛列表。**关联置信度：82**。
- **目标**：保留自动 wave 失败后的可恢复边界。**改动**：让 recovery/resume 入口使用 supervisor 专属 slot 状态 API，而不是普通 agent `exec.unit.done` emit；普通 flow step scope 继续拒绝非当前 step 允许的业务 emit。**预期效果**：门禁维持 fail-closed，同时恢复动作走正确的协调面。**关联置信度：78**。

---

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| `w-rs-1`、`w-2`、`w-246cb4afef33` 三层 wave 标识不一致的精确生成/转换点 | 58 | 本次无 FULL orchestration artifact，且未读取 supervisor DB 内部 ledger 作为业务接口 | 已查 events、recovery、日志、dispatcher 与 fan-in 源码；未写入 §5 的精确函数根因 |
| U1/U2 worker 是否完成后仅因事件写回失败而缺主 events | 52 | 缺 worker stdout/agent-output 与 slot-level result artifact | 已查主 events、summary、reporter、日志；未将“代码是否已在 slot worktree 完成”当作自动链路证据 |
| `blocking_slots` 全量 `[0,1,2,3,4]` 是否来自故障前快照或 DB slot 状态映射错误 | 58 | 本报告不把 supervisor.db 当业务接口，缺公开 inspect JSON 的 slot detail | 已查 `phase.rs:130-143` 的 `failed_count`/blocking 语义与事件 payload |

---

## 提交前检查

- [x] Phase 0 产物盘点表已写入。
- [x] 仅读取 `current-events` 指向的一个 events 文件作为编排 SSOT。
- [x] Diagnostics 模式按实际产物判定为 `LOGS_ONLY`，已声明 OPAC 降级。
- [x] 缺 supervisor.db / wave_id 未误判为故障；两者均符合 `[supervisor,wave]` capability。
- [x] 四问均有答案和置信度。
- [x] §5 每条 P0/P1 均有置信度；P0≥70，未把低于 60 的候选写入 §5。
- [x] disabled 模式下 §3 / §5 历史关联均为 `N/A (history disabled)`。
- [x] 未引用 `hat_handoff`、`loop_state_snapshot.json` 或其他 SSOT 禁止概念。
- [x] 报告落在主仓 `docs/report/`。
