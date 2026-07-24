---
title: ce-executor-supervisor Loop `primary-20260724-121001` 运行链路诊断报告
date: 2026-07-24
type: diagnosis
loop_id: primary-20260724-121001
preset: builtin:ce-executor-supervisor
run_dir: ../ralph-e2e
status: exec wave 失败，未进入 review/fix/terminal 正常链路
diagnostics_mode: LOGS_ONLY
---

# ce-executor-supervisor Loop `primary-20260724-121001` 运行链路诊断报告

> **生成日期**：2026-07-24  
> **诊断对象**：`../ralph-e2e/.ralph/`  
> **对照配置**：`presets/en/ce-executor-supervisor.yml` + `presets/schemas/ce-executor-supervisor.yml`  
> **报告仓库**：`ralph-orchestrator` 主仓  
> **Diagnostics 模式**：`LOGS_ONLY`；没有 session `orchestration.jsonl` 与 agent-output，因此 OPAC/agent 归因降级。  
> **execution_capabilities**：`["supervisor", "wave"]`。判定信号：preset 的 `event_loop.supervisor.enabled: true`（preset:91-94）、存在 `.ralph/supervisor.db`，且可信 events 含 `wave_id`（events:4-13）。  
> **置信度规则**：§5 仅纳入 confidence≥60；P0 需≥70。

## 0. 产物盘点（Phase 0）

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` | 是 | 指向 `events-20260724-121001.jsonl` | 唯一可信 events 指针 |
| S | 指针指向的 events | 是 | 13 行 | 仅读取该文件，含 `wave_id` |
| S | 配对 `events-history-20260724-121001.jsonl` | 是 | 2 行 | 历史旁路，不作拓扑 SSOT |
| S | `.ralph/ledger.jsonl` | 是 | 3 行 | 状态提交证据 |
| S | `.ralph/recovery.jsonl` | 是 | 5 行 | 含 RepairStream 记录及 recovery exhausted |
| S | `.ralph/loops.json` | 是 | `loops: []` | 终止后无活动 loop 登记 |
| S | `.ralph/loop-termination-reason.json` | 是 | recovery exhausted | `cli_emit:*:exec_unit_done:flow_unknown_emit:flowstepscope` |
| S | `.ralph/loop.lock` | 否 | 已释放 | 非 stale lock |
| A | `.ralph/agent/tasks.jsonl` | 是 | 10 行 | 5 个 plan task 仍 open；5 个 supervisor slot failed |
| A | `.ralph/agent/summary.md` | 是 | 终止摘要 | 3 iterations，15m19s，未发现 scratchpad |
| A | `.ralph/agent/progress.md` | 否 | 条件未满足 | 不将缺失视为机制故障 |
| A | `.ralph/agent/handoff.md` | 否 | 本次无可用终止 handoff | 不将缺失单独视为机制故障 |
| B | `.ralph/diagnostics/logs/` | 是 | 2 个日志 | LOGS_ONLY 的主机制证据 |
| B | `.ralph/diagnostics/<session>/` | 否 | 无 session 目录 | 无 orchestration/agent-output，OPAC 降级 |
| B | `.ralph/supervisor.db` | 是 | 文件存在 | supervisor capability 的必要产物已生成 |
| B | `.ralph/agent/.ralph-enforce-current-unit` | 是 | 标记文件 | 对应 preset `enforce_current_unit: true` |
| B | `.ralph/review/.../execution-plan.yml` | 是 | 5 nodes、0 edges | task-planner 产物，静态计划已写入 |
| C | `sorts/` 业务产物 | 未从本次 `.ralph/` 盘点确认 | — | 不能以缺失推断 worker 未写入；事件只提供 payload 摘要 |

**Tier C 预期**：由 plan 的 5 个 unit 指定 `sorts/` 下骨架、4 个排序实现、测试和 README；execution-plan 已包含 U1–U5 及对应 `files_written`。由于本次在 exec wave 失败，review/fix 阶段产物属于未触发，不标为丢失。

**Diagnostics 盲区声明**：`LOGS_ONLY` 只支持 events ↔ workspace recovery ↔ logs 三联对账；没有 orchestration 与 agent-output，无法逐 activation 证明 agent 是否执行了 precheck、实际命令输出、或完整 OPAC。纯 OPAC/agent 结论置信度上限为50；带 recovery + 源码行号的机制/配置结论可使用较高但仍受整体日志盲区约束。

## 1. 结论摘要

### 1.1 健康度

- **判定**：exec wave 失败，属于真实失败而非正常完成；主链停在 `exec.wave.failed`，没有 `work.done`、review、fix、`plan.complete` 或 `LOOP_COMPLETE`。
- **P0 / P1 / P2 数量**：P0 2 项、P1 1 项（均满足 §5 置信度门槛）。
- **最高根因置信度**：P0-1 = **85/100**。
- **历史复发**：是。`exec.unit.done` 可见性/闭环问题至少连续出现在 2026-07-22、07-23、07-24 三份 supervisor 诊断中；相关机制残留未闭环。

### 1.2 强制四问（逐条）

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 整体执行与 OPAC 是否合规？ | ⚠️ 部分可见、无法完全证明 | plan/task-planner/dispatcher 阶段按事件顺序推进；但 LOGS_ONLY 无 agent-output，且 recovery 记录了 CLI emit 的 flow scope 拒收 | 55 |
| Q2 | 基座机制是否正常生效？ | ❌ 未完全生效 | supervisor.db、recovery、lock release 和失败终止均工作；但完成状态与 wave failure 状态矛盾，且 recovery 重试耗尽 | 78 |
| Q3 | 编排是否合理、正常运行？ | ❌ 不正常 | 5 个 `exec.unit.ready` 后仅 4 个 `exec.unit.done`，随后 `exec.wave.failed`；exec-integrator 及后续 hats 均未触发 | 90 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **以 preset 编排契约为主，叠加机制状态传播问题** | `unit_loop.allowed_emits` 未覆盖 `exec.unit.done`，同时 supervisor 状态将已出现的完成结果判为全部 failed | 78 |

### 1.3 根因一句话

`exec.unit.done` 同时被声明为业务/schema 事件，却未被 `mechanism.flow` 当前步骤允许；该 preset 契约冲突触发 `flow_unknown_emit`，又与 supervisor fan-in 的 slot 状态传播不一致共同导致 exec wave fail（根因置信度 **85/100**）。

## 2. 执行链路对比

### 2.1 实际激活表

| Hat | 次数 | 结果 |
|---|---:|---|
| coordinator | 1 | `plan.ready` → `work.ready` |
| task-planner | 1 | `execution.plan.ready` |
| exec-wave-dispatcher | 1 | 发出 5 个 `exec.unit.ready` payload |
| worker | 4 个可见完成事件 | U1、U3、U4、U5 有 `exec.unit.done`；U2 无对应主 events |
| exec-failure-handler | 1 | system-injected `exec.wave.failed` |
| exec-integrator | 0 | 未收到 `exec.wave.complete` |
| review-coordinator / review-batch-worker / review-synthesizer | 0 | `work.done` 未产生 |
| fix-task-planner / fix-worker / fix-integrator | 0 | review 链未启动 |
| alignment / reporter | 0 | 没有终态 handoff |

### 2.2 预期与实际时间轴

| 时间（UTC） | 实际事件 | 预期/偏离 |
|---|---|---|
| 12:10:01 | `plan.ready` | ✅ 启动 |
| 12:14:58 | coordinator → `work.ready` | ✅ |
| 12:17:36 | task-planner → `execution.plan.ready` | ✅ |
| 12:18:41 | dispatcher → `exec.unit.ready` ×5 | ✅ 五 slot fan-out |
| 12:21:21–12:25:10 | `exec.unit.done` ×4（U3/U4/U1/U5） | ⚠️ U2/slot1 缺失；完成 payload 结构也不一致 |
| 12:25:19 | `exec.wave.failed`，`blocking_slots:[0,1,2,3,4]` | ❌ 与四个完成事件及 recovery 记录矛盾 |
| 之后 | 无 `work.done` / review / fix / terminal | ⏸️ 上游失败导致后续未触发 |

## 3. 历史问题上下文

| 问题类型 | 历史次数 | 代表路径 | 闭环状态 | 本次关联 |
|---|---:|---|---|---|
| supervisor `exec.unit.done` 可见性/闭环断裂 | 3 | `docs/report/2026-07-22-ce-executor-supervisor-primary-20260722-084810-diagnosis.md` 等 | 未闭环；M4 deferred | 极高 |
| `flow_unknown_emit` 误拒业务事件 | 6+ | 2026-06-28～06-30 serial 诊断/评审 | serial 已闭环，supervisor 未验证 | 中（本次直接命中同类 reason） |
| recovery retry exhausted | 多次 | `docs/achieved/report/2026-06-28...`、`docs/achieved/plan/2026-06-29-006...` | serial 部分闭环，supervisor 仍复现 | 高 |
| escalation 指向已删除 shipper | 3 | 2026-07-23/24 supervisor 诊断 | 已知 residual，未闭环 | 高；日志出现 unregistered `shipper` 警告 |

本次不是全新问题模式，而是 supervisor 上下文中既有的 `exec.unit.done` 状态/事件闭环问题与 `flow_unknown_emit` 恢复路径叠加。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 证据缺口 |
|---|---|---|---|---:|---|
| DEV-001 | `exec.unit.done` 在当前 flow 声明中无允许步骤，recovery 明确记录 flow scope 拒绝 | `.ralph/recovery.jsonl:1-5`；`presets/en/ce-executor-supervisor.yml:48-55`；`flow_step_scope_stage.rs:191` | P0 | 85 | 需确认 supervisor bridge 应归属哪个 flow step |
| DEV-002 | wave failure 将 `[0,1,2,3,4]` 全列为 blocking，但可信 events 有 4 个完成事件，tasks 也显示 5 slot failed | `events-20260724-121001.jsonl:9-13`；`tasks.jsonl:6-10` | P0 | 78 | 需读取 supervisor store 的 fan-in 时刻状态（当前报告不直接读取内部 DB 内容） |
| DEV-003 | 已声明业务/schema 事件的 payload 结构不一致，至少 U1/U3/U4/U5 与 schema required fields 不一致 | `presets/schemas/ce-executor-supervisor.yml:57-65`；events:9-12 | P1 | 72 | 无 agent-output，不能确认实际 CLI precheck/bridge 豁免路径 |
| DEV-004 | recovery 重试键为 CLI emit + flow_unknown_emit，最终没有安全 retry target | `.ralph/loop-termination-reason.json:1`；`.ralph/agent/summary.md:25-33` | P1 | 72 | 需补读 recovery routing 的完整 source/outcome 链 |
| DEV-005 | disabled steward 仍有指向未注册 shipper 的残留警告 | `.ralph/diagnostics/logs/ralph-...856-78837.log:36`；preset:98-110；`shipper_reason.rs:103-104` | P1 | 72 | 需确认本次 warning 是否直接阻断 wave，还是独立收尾噪声 |

### 4.1 OPAC 逐 hat 审计（LOGS_ONLY 降级）

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| coordinator | ✅ | ⚠️ 无 agent-output | ✅ | ✅ 事件已落盘 | `plan.ready`→`work.ready` | 45 |
| task-planner | ✅ | ⚠️ 无 agent-output | ✅ | ✅ artifact + handoff | `execution.plan.ready`、execution-plan.yml | 45 |
| exec-wave-dispatcher | ✅ | ⚠️ 无 agent-output | ✅ | ⚠️ 结果未完整闭环 | 5 个 ready payload | 45 |
| worker | ⚠️ 4 个结果可见、1 个缺失 | ⚠️ | ⚠️ | ⚠️ | 4 个 done + recovery RepairStream | 45 |
| exec-failure-handler | ✅（system injected） | N/A | ✅ | ✅ failed 事件已落盘 | `exec.wave.failed` | 45 |
| 后续 hats | N/A | N/A | N/A | N/A | 前置事件未产生 | 40 |

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---:|
| P0 | `exec.unit.done` 在 `unit_loop` 被 FlowStepScope 拒收；业务 topics/schema 与 flow allowed_emits 冲突 | **preset** | **85** | DEV-001；`presets/en/ce-executor-supervisor.yml:48-55,149`；`flow_step_scope_stage.rs:191`；recovery retry_key | 高，supervisor 连续 3 次 | 1→85 |
| P0 | 已完成 unit 未形成一致的 supervisor fan-in 状态，failure 侧将全部 slot 标为 failed，导致 wave 不收敛 | **compound（preset 60% + mechanism 40%）** | **78** | DEV-002；events:9-13；tasks:6-10；`workflow_activation.rs:106-109` | 高，与可见性断裂复发模式一致 | 1→78 |
| P1 | RepairStream/CLI recovery 重试沿用被 flow scope 拒绝的事件路径，最终 retry exhausted | **mechanism** | **72** | DEV-004；`loop-termination-reason.json:1`；`summary.md:25-33` | 中高，recovery exhausted 多次复现 | 1→72 |
| P1 | U5 escalation 仍可能硬编码目标 `shipper`，目标 hat 已不存在，EventBus 警告后无法形成 blocked handoff | **mechanism** | **72** | DEV-005；logs:36；`shipper_reason.rs:103-104`；preset:98-110 | 高，已知 residual | 1→72 |
| P1 | worker done payload 没有统一满足 `wave_id`、`slot_index`、`content_hash` required_fields | **compound（agent 30% + preset/mechanism 70%）** | **72** | DEV-003；schema:57-65；events:9-12 | 中 | 1→72 |

## 6. 修复建议

### 6.1 短期（operator workaround）

- **目标**：避免把当前 run 误判为成功；**动作**：在修复前不要重试同一 recovery key，先保留 `.ralph/` 产物并重新运行 supervisor E2E；**效果**：区分新 run 是否仍在 U2/`exec.unit.done` 阶段失败；**置信度**：85。
- **目标**：降低 5 slot 压力变量；**动作**：临时将测试 plan 拆成不超过 `max_concurrent_workers` 的独立批次，或明确接受排队；**效果**：减少队列因素，但不能替代契约修复；**置信度**：65。

### 6.2 中期（preset/schema/流程契约）

- **目标**：消除事件分类冲突；**动作**：明确 `exec.unit.done` 的真实 flow 所属：若它是 supervisor bridge 内部事件，就在正确的 side-effect/bridge flow 声明中允许；若它是 worker activation 事件，则将 worker step 与 `allowed_emits` 对齐；同步 schema、BDD 和 `publishes/triggers`，不要仅把它加入 `business_topics`；**效果**：避免 schema gate 通过而 FlowStepScope 拒收；**置信度**：85。
- **目标**：统一 payload 契约；**动作**：为所有 worker done 路径强制填充 `wave_id`、`slot_index`、`content_hash`，并增加真实 runtime BDD fan-out/fan-in 验收；**效果**：拒收应在正确边界发生，不再让结构不完整事件旁路进入主账本；**置信度**：72。

### 6.3 长期（机制/底座）

- **目标**：恢复 supervisor 状态与 main events 的一致性；**动作**：审计 fan-in 在 U2 缺失/RepairStream 重放时的 slot 状态写回、`blocking_slot_indices()` 与 wave failure 注入逻辑，保证已确认完成的 slot 不会同时进入 blocking list；**效果**：消除“4 个 done + 5 个 failed”的状态矛盾；**置信度**：78。
- **目标**：让 recovery 具备可达的失败终态；**动作**：为 RepairStream 的 flow scope 失败选择正确的恢复/升级路由，清理已删除 `shipper` 目标的 dead path，确保无法恢复时产生合法 `plan.blocked` 或等价当前终态；**效果**：避免 `recovery_exhausted` 成为无消费者的硬退出；**置信度**：72。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| `exec.unit.done` 应声明在 `unit_loop` 还是 `exec_wave` side-effect step | 55 | 缺 supervisor fan-in 内部状态及完整 bridge trace | 已做 preset/schema + flow 源码反查；未将其作为独立定论 |
| `exec.wave.failed` 的 blocking list 是否由全部 slot failed 状态直接计算 | 58 | 未读取 supervisor DB 内容，且 LOGS_ONLY 无 orchestration | 已做 events/tasks 对照；保留为 DEV-002 的待核实机制细节 |
| shipper warning 是否直接导致本次 wave 失败 | 50 | 缺完整 recovery routing/orchestration | 已读日志与 preset 注释；未单独升级 P0 |

## 8. 关键主仓代码与配置引用

- `presets/en/ce-executor-supervisor.yml:48-55`：`unit_loop.allowed_emits` 未声明 `exec.unit.done`。
- `presets/en/ce-executor-supervisor.yml:91-94`：supervisor enabled、数据库路径和并发上限。
- `presets/en/ce-executor-supervisor.yml:145-153`：`exec.unit.done` 位于 business topics，形成 schema/business 与 flow scope 的不一致表面。
- `presets/schemas/ce-executor-supervisor.yml:57-65`：`exec.unit.done` required fields 为 `wave_id`、`slot_index`、`content_hash`。
- `crates/ralph-core/src/preset/engine/flow_step_scope_stage.rs:191`：按当前 flow step 的 `allowed_emits` 判断事件是否允许。
- `crates/ralph-core/src/preset/engine/flow_declaration.rs:59-60`：`allowed_emits` 的声明模型。
- `crates/ralph-core/src/preset_lint/workflow_activation.rs:106-109`：supervisor fan-in 的 `exec.unit.done → exec.wave.complete` 虚拟边。
- `crates/ralph-core/src/supervisor/rusqlite.rs:158-172`：slot 状态/失败状态相关实现入口（本次未直接读取 DB 内容）。
- `crates/ralph-core/src/shipper_reason.rs:103-104`：残留的 `target=shipper` 路径说明。
- `.ralph/recovery.jsonl:1-5`：recovery 与 RepairStream 证据。
- `.ralph/diagnostics/logs/ralph-2026-07-24T20-10-00-856-78837.log:36,41`：未注册 shipper 警告与 recovery exhausted 收尾。

## 9. 盲区与边界

- 本报告未修改代码、preset 或 run 目录运行时状态。
- 未将不存在的 `orchestration.jsonl`、`agent-output`、`progress.md` 凑作证据。
- 未直接读取或推断 supervisor 数据库内部内容；关于 slot fan-in 的状态矛盾按 events/tasks/recovery 交叉证据列出，并保留待核实疑点。
- 历史报告中若含已删除概念，不作为本次机制对账依据；本报告只采用当前 skill 护栏允许的事件、recovery、tasks、flow 和源码路径。
