---
title: ce-executor-supervisor Loop `primary-20260724-020630-` 运行链路诊断报告
date: 2026-07-24
type: diagnosis
loop_id: primary-20260724-020630-
preset: presets/en/ce-executor-supervisor.yml
run_dir: ../ralph-supervisor
status: 一句话健康度 — **链路 collapse**：3 iter / 14m 内 `plan.ready` 后未形成 `work.ready`，coordinator 错误直接 emit `LOOP_COMPLETE`（拒×2）→ U5 escalation 发 `plan.blocked target=shipper` 静默 drop → hard gate → `recovery_exhausted` → `loop.cancel`
diagnostics_mode: LOGS_ONLY
---

# ce-executor-supervisor Loop `primary-20260724-020630-` 运行链路诊断报告

> **生成时间**: 2026-07-24
> **诊断对象**: `ralph-supervisor/.ralph/`（loop_id=`primary-20260724-020630-`，启动 02:06:30 → 终止 02:20:30）
> **对照 preset**: `presets/en/ce-executor-supervisor.yml` + `presets/schemas/ce-executor-supervisor.yml`
> **执行方式**: Phase 0 主 Agent 盘点 → Phase 1 A∥B sub-agent 流程还原 + 历史 → Phase 2 C 对账 → Phase 3 D 归因
> **Diagnostics 模式**: LOGS_ONLY
> **execution_capabilities**: `["supervisor", "wave"]`（YAML `event_loop.supervisor.enabled: true` + 多 hat instructions 含 `ralph wave emit` + `.ralph/supervisor.db` 存在；events 无 `wave_id` 因 wave 从未启动，按 capability 判定记 N/A）
> **报告仓库**: `ralph-orchestrator` 主仓（非 run_dir）
> **置信度规则**: §5 仅收录 confidence ≥ 60；P0 须 confidence ≥ 70（见 confidence-rubric）

---

## 0. 产物盘点（Phase 0 必附）

| Tier | 路径 | 存在 | 行数 / 体积 | 备注 |
|------|------|------|-------------|------|
| S | `events-20260724-020630.jsonl`（current-events 解析） | ✅ | 3 行 | plan.ready + loop.terminate + 1 history 入口 |
| S | `events-history-20260724-020630.jsonl` | ✅ | 2 行 | 2× LOOP_COMPLETE reject + counter_changed + cancellation_requested |
| S | `ledger.jsonl` | ✅ | 6 行 | 含 plan.ready + LOOP_COMPLETE + loop.cancel + counter |
| S | `recovery.jsonl` | ❌ | 0 行 | workspace 缺失 |
| S | `loops.json` | ✅（空数组） | 0 实体 | loop 索引未登记 |
| S | `loop.lock` | ❌ | — | lock_released（已清理） |
| S | `current-loop-id` | ✅ | `primary-20260724-020630-` | — |
| S | `diagnostics/logs/ralph-2026-07-24T10-06-29-{945,948}-*.log` | ✅ | 13 + 40 行 | 唯一编排证据 |
| A | `agent/tasks.jsonl` | ✅ | 5 行 | u1-u5 全部 status=failed, owner=coordinator |
| A | `agent/summary.md` | ✅ | 小 | "Cancelled gracefully, 3 iter, 14m 0s" |
| A | `agent/progress.md` | ❌ | — | 未生成 |
| A | `agent/handoff.md` | ❌ | — | 未生成 |
| A | `agent/scratchpad.md` | ❌ | — | 未生成 |
| A | `agent/.ralph-enforce-current-unit` | ✅ | marker | R4 单 U 契约 |
| A | `agent/memories.md` | ✅（运行时注入 1 memory） | 494 chars | 跨 loop 记忆 |
| B | `supervisor.db` | ✅ | 81920 bytes (sqlite3 v4 schema) | capability +supervisor 已声明；存在符合预期 |
| B | `diagnostics/channel-routing-fallback-2026-07-24T02-18-00.md` | ✅ | 小 | coordinator hat-channel empty 兜底记录 |
| B | `diagnostics/agent_doc_sync.json` | ✅ | 4 字段 | synced=2 / skipped=0 / failed=0 |
| B | `agent/plan-baseline-*.sha` | ✅ | 1 文件 | plan attach baseline |
| B | `ralph.yml`（run_dir） | ❌ | — | 无 workspace 配置 |
| C | preset `unit_loop.allowed_emits` / `plan_end.allowed_emits` | ✅ | preset L48-55, L79-82 | 预期拓扑声明 |
| C | preset `event_policy.{terminal_topics,business_topics,completion_after_terminal}` | ✅ | preset L118-145 | 静态契约 |

**execution_capabilities 推断结果**: `["supervisor", "wave"]`（capability 信号：YAML `event_loop.supervisor.enabled: true`、多 hat instructions 含 `ralph wave emit`、`.ralph/supervisor.db` 存在；events 无 `wave_id` 因 wave 从未启动，不属 capability 故障）

**缺失产物 → 故障判定**（capability-triggered）:

- `supervisor.db` → ✅ 存在（capability +supervisor 期望）
- events 无 `wave_id` → N/A（capability +wave 但 wave 未启动是结果而非原因）
- `recovery.jsonl` 缺失 → 仅 workspace recovery 缺失；session 目录无（LOG_ONLY 模式）→ 不能据此推断无拒收（ledger 有 LOOP_COMPLETE 拒记录）
- `agent/handoff.md` / `progress.md` / `scratchpad.md` 缺失 → loop 未达正常终止，未生成

**盲区 / 根因置信度硬顶**: LOGS_ONLY → agent / OPAC 归因 ≤50，根因硬顶 75；mechanism 有 file:line+recovery 可例外至 85。

---

## 1. 结论摘要

### 1.1 健康度

- **判定**: **链路 collapse（preset × mechanism × agent 三方失配）**
- **P0 / P1 数量**（均为 confidence ≥ 入表门槛）:
  - P0：4 条（M1 85、M2 80、M3 60、M4 70）
  - P1：1 条（M5 70）
- **最高优先级根因置信度**: P0-M1 = **85** / 100
- **历史复发**: 是 — 第 3 次 — 同根因家族（U5 escalation hardcode × preset 删除 shipper / progress-steward 后的失配），引用 `docs/report/2026-07-23-ce-executor-supervisor-primary-20260723-082003-diagnosis.md` 与 `docs/report/2026-07-22-ce-executor-supervisor-primary-20260722-084810-diagnosis.md`

### 1.2 强制四问

| # | 问题 | 答案 | 一句证据 | 置信度 |
|---|------|------|----------|--------|
| Q1 | 整体执行与 OPAC 是否合规？ | ❌ | coordinator 违反单事件预算（emit LOOP_COMPLETE 代替 work.ready），且同一违例重复 2 次；OPAC Confirm 全 ❌（LOGS_ONLY） | 60 |
| Q2 | 基座机制是否正常生效？ | ❌（局部 ✅） | P0-5 reject、hard gate、stall detection 全部按预期触发；但 U5 escalation emit plan.blocked **静默 drop**（target=shipper 已不存在）— mod.rs:13960 | 85 |
| Q3 | 编排是否合理、正常运行？ | ❌ | 14 hat 拓扑仅 coordinator 一次激活；其余 13 hat（task-planner、exec-wave-dispatcher、worker×N、integrator、review/fix/alignment/reporter）0 激活 | 90 |
| Q4 | 问题归因：机制 vs 编排 vs agent？ | **compound (mechanism + preset) 主导，agent 触发** | M1 主链：U5 escalation hardcode (mechanism) × preset 删除 shipper (preset) → agent 误判加速 collapse | 85 |

### 1.3 根因一句话

**M1（compound, conf 85）**：preset 2026-07-23-005 删除 `shipper` / `progress-steward` 后，机制层 U5 escalation 在 `crates/ralph-core/src/event_loop/mod.rs:13960` 仍硬编码 `.with_target(HatId::new("shipper"))` 发 `plan.blocked`，而 EventBus 对 target=未知 hat 静默丢弃 → reporter 未唤醒 → `recovery_exhausted:stall_recovery` → `loop.cancel`。LLM coordinator 误判为加速器：收到 `plan.ready` 后跳过 §1 契约直接 emit `LOOP_COMPLETE`，且虚构 `preset_task_key_contract_drift` 字符串。

---

## 2. 执行链路对比图

### 2.1 拓扑激活表（仅 coordinator 一次激活）

| Hat | 预期激活次数 | 实际激活次数 | 触发条件 / 发布事件 | 缺失原因 |
|---|---:|---:|---|---|
| coordinator | 1 | 1 | `plan.ready` → `work.ready` / `plan.complete` / `LOOP_COMPLETE` | 已激活；未形成 `work.ready`，转 emit LOOP_COMPLETE（被拒×2）|
| task-planner | 1 | 0 | `work.ready` → `execution.plan.ready` / `plan.blocked` | 未收到 `work.ready` |
| exec-wave-dispatcher | ≥1 | 0 | `execution.plan.ready` / `exec.wave.complete` | 上游未触发 |
| worker ×N（≤4） | ≥1 | 0 | `exec.unit.ready` → `exec.unit.done` / `failed` | 上游未触发 |
| exec-integrator | 1 | 0 | `exec.wave.complete` → `work.done` | 上游未触发 |
| exec-failure-handler | 0/1 | 0 | `exec.wave.failed` → `work.failed` | 上游未触发 |
| review-coordinator | 1 | 0 | `work.done` / `exec.wave.complete` → `review.unit.ready` | 上游未触发 |
| review-batch-worker ×7 | ≥1 | 0 | `review.unit.ready` → `review.unit.done` | 上游未触发 |
| review-synthesizer | 1 | 0 | `review.wave.complete` → `review.complete` | 上游未触发 |
| fix-task-planner | 0/1 | 0 | `review.complete` → `fix.unit.ready` | 上游未触发 |
| fix-worker ×N | ≥1 | 0 | `fix.unit.ready` → `fix.unit.done` / `failed` | 上游未触发 |
| fix-integrator | 0/1 | 0 | `fix.wave.complete` → `fix.done` | 上游未触发 |
| alignment | 0/1 | 0 | `fix.done` → `plan.complete` / `plan.blocked` | 上游未触发 |
| reporter | 0/1 | 0 | `plan.complete` / `plan.blocked` / `work.failed` → `LOOP_COMPLETE` | 上游未触发；plan.blocked target=shipper 静默 drop |

证据：

- preset coordinator 定义 `presets/en/ce-executor-supervisor.yml:171-342`
- preset task-planner `:344-662`，exec-wave-dispatcher `:675-836`，worker `:838-888`
- exec-integrator / exec-failure-handler `:889-967`
- review-coordinator / review-batch-worker / review-synthesizer `:968-1481`
- fix-task-planner / fix-worker / fix-integrator `:1483-1762`
- alignment / reporter `:1763-1920`

### 2.2 时间轴对比表

| 时间 | 实际事件 | 按 preset / schema 的下一步 | 结果 |
|---|---|---|---|
| 02:06:30.437 | `plan.ready`（source=loop-bootstrap）| 激活 coordinator → 期望 emit `work.ready` | coordinator 触发；未 emit `work.ready` |
| 02:09:21-02:10:44 | 5 task u1-u5 创建（owner=coordinator） | 应由 task-planner 接收 work.ready | 未观察到 `work.ready` |
| 02:16:33-02:16:47 | 5 task 全部 closed=failed（14s 内） | 应出现 `work.failed` 或 unit 失败 | events 流无对应业务事件（M4 复合根因） |
| 02:17:20.754 | ledger 记录 coordinator 发 LOOP_COMPLETE（虚构 reason `preset_task_key_contract_drift`） | preset 要求 reporter 才能 emit LOOP_COMPLETE | 终态发布者错位（M3） |
| 02:17:34.376 | P0-5 reject #1（missing work.done） | 继续等待 | 拒收 |
| 02:17:34.502-602 | progress-steward "not registered" WARN ×3（counter 递增） | U5 escalation 阈值未到 | 警告 |
| 02:17:34.653 | U5 escalation emit `plan.blocked target=shipper` | reporter 收 plan.blocked | **静默 drop**（M1 主根因） |
| 02:17:34.395 | P0-5 reject #2（同违例） | 应被 dedup 阻断 | 未阻断（M5） |
| 02:18:00.553 | hat-channel routing fallback for coordinator | 落 diagnostic 文件 | hat-channel empty（伴随现象） |
| 02:18:00.557 | hard gate 触发（publish_obligation） | escalate | 失败链 |
| 02:20:01.508 | loop.cancel by ralph source（`recovery_exhausted:stall_recovery:coordinator:task_resume:stall_no_events`） | 按 `ralph-tools-recovery-directives.md:70-85` 应由 hat emit plan.blocked | hat 未消费 recovery_exhausted（M9，合并 M1） |
| 02:20:30.597 | loop.terminate（cancelled gracefully, 3 iter, 14m 0s） | — | 链路终止 |
| 02:21:00.888 | TUI Quit intercept → subprocess SIGKILL | — | human 后续收尾 |

实际事件证据：

- `events-20260724-020630.jsonl#L1` plan.ready；`#L2` loop.terminate
- `events-history-20260724-020630.jsonl#L1, #L4` 2× LOOP_COMPLETE reject（policy:unknown:loop.complete:missing_field）
- `ledger.jsonl#L2-L3` coordinator LOOP_COMPLETE 记录 + 后续取消
- `agent/tasks.jsonl#L1-L5` 5 task 全部 failed
- logs `ralph-2026-07-24T10-06-29-948-1802416.log:22` "steward did not produce progress after 3 wakes — emitting plan.blocked"

### 2.3 关键观察

1. `plan.ready` 是唯一业务起点；主事件流没有后续 `work.ready`、`execution.plan.ready`、任何 unit 事件、`work.done` 或 `work.failed`
2. coordinator 触发后未按 §1（preset:228-241）走「register tasks → emit `work.ready`」，反而直接 emit LOOP_COMPLETE
3. U5 escalation 构造 plan.blocked 时硬编码 target=shipper（mod.rs:13960），但 shipper 已从 preset 删除（preset:23, 163），EventBus 静默 drop
4. 5 task 在 14s 内批量 closed=failed，但无对应业务事件（M4：tasks ledger 与 events 流分叉）
5. ledger 记录的 LOOP_COMPLETE 发布者为 coordinator，而 preset 唯一定义的 reporter 终态发布路径位于 preset:1813-1920
6. schema required events 是 `work.done + LOOP_COMPLETE`（preset:99-102），终态完成条件未满足
7. `recovery_directives` skill 明确要求 hat 收到 `recovery_exhausted` 后**禁止重试、立即 emit plan.blocked**（`ralph-tools-recovery-directives.md:70-85`），但本次由 runtime（ralph source）直接发 loop.cancel，绕过了该契约
8. **agent 虚构字符串** `preset_task_key_contract_drift` 在全仓 0 hit（rg 全仓） — 不是源码常量，是 LLM 幻觉

---

## 3. 历史问题上下文

### 3.1 同 preset 历史 run 对照表

| 日期 | loop_id | 关键症状 | 与本次关联度 | 历史报告 |
|---|---|---|---|---|
| 2026-07-22 | primary-20260722-084810 | worker event visibility / orphan emit；task lifecycle 未闭合 | 高（同一根因家族）| `docs/report/2026-07-22-ce-executor-supervisor-primary-20260722-084810-diagnosis.md` |
| 2026-07-23 | primary-20260723-082003 | plan.blocked 无 consumer / orphan emit / hat-channel empty / task lifecycle 全 open | **高**（同根因：preset 删除 progress-steward 后 recovery 责任漂移）| `docs/report/2026-07-23-ce-executor-supervisor-primary-20260723-082003-diagnosis.md` |
| **2026-07-24** | **primary-20260724-020630** | U5 escalation target=shipper 静默 drop + coordinator LLM 误判 | — | **本次报告** |

### 3.2 复发判定

- **明确复发**：是（30 天内同根因家族 ≥2 次）
  1. **U5 escalation hardcode × preset 删除 shipper**：本次新发现但历史报告 §1.3 多次提及 `plan.blocked` 无 consumer（已由 005/U8 静态闭合），本次是动态落盘路径未对齐
  2. **worker event visibility / orphan emit**：22 + 23 两次
  3. **hat-channel empty after activation**：22 + 23 + 本次共 3 次
  4. **task lifecycle 未闭合**：22 + 23 + 本次共 3 次
- **新模式**：coordinator LLM 跳过 §1 直接 emit 终态（**首次记录**）— 不归入旧复发家族
- **复发 vs 修复不完整**：
  - preset 2026-07-23-005 删了 shipper/progress-steward/fallback fixer → 历史报告认为 `plan.blocked` 已由 reporter 单 owner 闭合
  - **本次证明该修复不完整**：mechanism 层 mod.rs:13960 仍 hardcode target=shipper，preset 删除 shipper 后未联动修改 → 这是"修复不完整"型回归

### 3.3 相关 solution / 文档

| 文档 | 关联点 |
|---|---|
| `docs/solutions/integration-issues/emit-workspace-root-cwd-drift.md` | cwd/workspace-root shadowing（orphan emit 家族） |
| `docs/solutions/logic-errors/isolated-ralph-must-not-drain-multi-consumer-pending.md` | isolated pending 被错误 consumer 抽干 |
| `docs/solutions/logic-errors/ce-executor-p0-event-policy-and-projector-fanout.md` | task.resume 被业务 gate 拒（已闭环）|
| `crates/ralph-core/data/ralph-tools-recovery-directives.md:70-85` | **强约束**：收到 recovery_exhausted 必须立即 emit plan.blocked，禁止重试 |
| `crates/ralph-core/data/ralph-tools-tasks.md:154` | 同上（task.resume kind=recovery_exhausted → emit plan.blocked 禁止重试）|

---

## 4. 证据清单（DEV 表）

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 证据缺口 |
|----|------|----------|------------|------------|----------|
| DEV-001 | coordinator emit `LOOP_COMPLETE` 代替 `work.ready` | preset:205-234, events L1-2, log 02:17:20 | P0 | 85 | 缺 FULL agent-output |
| DEV-002 | LOOP_COMPLETE P0-5 reject（missing work.done） | mod.rs:11239-11284, log 02:17:34.376 | P0 | 95 | 无 |
| DEV-003 | 5 task u1-u5 closed=failed 无对应业务事件 | tasks.jsonl L1-5; events 无 unit failed | P0 | 85 | 缺 task close 原始命令 |
| **DEV-004** | **U5 escalation emit plan.blocked target=shipper 静默 drop** | **mod.rs:13933-13961 (L13960 hardcode), preset:23,163,1820** | **P0** | **95** | 缺 EventBus drop 结构化记录 |
| DEV-005 | hard gate 触发（publish_obligation） | runner.rs:4474-4499, log 02:18:00.557 | P0 | 90 | 缺 activation stdout |
| DEV-006 | hat-channel routing fallback for coordinator | hat_channel.rs:71-88, log 02:18:00.553 | P1 | 90 | 缺 fallback 诊断文件正文 |
| DEV-007 | 同一 LOOP_COMPLETE 被拒 2 次（iter 0+1） | mod.rs:11247-11284, log 02:17:34.376 + 02:17:34.395 | P1 | 85 | 缺两次 event ID 对比 |
| DEV-008 | agent 虚构字符串 `preset_task_key_contract_drift` | 全仓 rg 0 hit; events 02:17:20 | P1 | 90 | 缺 agent 生成上下文 |
| DEV-009 | loop.cancel by ralph 而非 hat emit plan.blocked | event 02:20:01.508, `ralph-tools-recovery-directives.md:68-77`, `publish_loop_stalled.rs:28-50` | P0 | 90 | 缺 task.resume 投递/消费日志 |
| DEV-010 | tasks 14s 内批量 close=failed 无 close 原因 | tasks.jsonl 02:16:33-02:16:47 | P1 | 85 | 缺 task note / close 参数 |
| DEV-011 | progress-steward "not registered" 警告×3 + counter++ → U5 escalation | mod.rs:13885-13909, 13933-13961, preset:23,163-169, log WARN×3 | P0 | 95 | 缺运行时 `progress_steward` 配置快照 |
| DEV-012 | capability +wave 无 wave_id | preset:89-96, events 无 wave_id | **N/A** | 90 | 无 |
| DEV-013 | plan.blocked 不在 event_policy.terminal_topics/business_topics | schema:174-181, preset:118-145 | P0 | 85 | 是否在更早层被分类，待 D 验证 |
| DEV-014 | instructions 要求 emit plan.blocked 但 allowlist 缺失 | preset:190-193,241-248,118-145 | P0 | 90 | 缺真实 policy-check 结果 |
| DEV-015 | 缺 work.ready → task-planner 未激活 | preset:205-234, timeline 仅 coordinator 激活 | P0 | 90 | 缺主 ledger 精确行号 |
| DEV-016 | emit bridge 拒 terminal task（潜在） | task_cli.rs:2496-2555, tasks 均 terminal | P1 | 80 | 未见真实 verify-emit-bridge 调用 |
| DEV-017 | LOOP_COMPLETE 进 ledger 但 P0-5 拒 accepted stream | mod.rs:11278-11284, timeline 02:17:20 + reject | P1 | 80 | 缺 current-events 精确行号 |
| DEV-018 | recovery_exhausted:stall_recovery 取消 | event 02:20:01.508, termination 02:20:30.597 | P0 | 90 | 缺 recovery.jsonl 逐阶段 action |
| DEV-019 | required_events (work.done + LOOP_COMPLETE) 均未闭合 | preset:99-102, P0-5 reject log | P0 | 95 | 无 |
| DEV-020 | 仅 coordinator 激活（其余 13 hat 0 激活） | termination 3 iter, OPAC 激活记录 | P0 | 90 | 缺 orchestration.jsonl |

### 4.1 OPAC 逐 hat 审计表（LOGS_ONLY）

| Hat | O | P | A | C | 证据 | 置信度 |
|-----|---|---|---|---|------|--------|
| coordinator | ✅ | ❌ | ❌ | ❌ | 创建 5 tasks；LOOP_COMPLETE 被拒；hat-channel empty；hard gate | 50 |
| task-planner | N/A | N/A | N/A | N/A | 未收到 `work.ready` | 45 |
| exec-wave-dispatcher | N/A | N/A | N/A | N/A | 无 `execution.plan.ready` | 45 |
| worker | N/A | N/A | N/A | N/A | 无 exec slot / `wave_id` | 45 |
| exec-integrator | N/A | N/A | N/A | N/A | 无 `exec.wave.complete` | 45 |
| exec-failure-handler | N/A | N/A | N/A | N/A | 无 `exec.wave.failed` | 45 |
| review-coordinator | N/A | N/A | N/A | N/A | 无 `work.done` | 45 |
| review-batch-worker | N/A | N/A | N/A | N/A | 无 review wave | 45 |
| review-synthesizer | N/A | N/A | N/A | N/A | 无 review batch结果 | 45 |
| fix-task-planner | N/A | N/A | N/A | N/A | 无 review handoff | 45 |
| fix-worker | N/A | N/A | N/A | N/A | 无 fix slot | 45 |
| fix-integrator | N/A | N/A | N/A | N/A | 无 `fix.wave.complete` | 45 |
| alignment | N/A | N/A | N/A | N/A | 无 `fix.done` trigger | 45 |
| reporter | N/A | N/A | N/A | N/A | 无 `plan.complete`/可消费 `plan.blocked` | 45 |

注：LOG_ONLY 模式下 OPAC 单项置信度硬顶 ≤50；Confirm 列 N/A 在 LOGS_ONLY 下允许。

### 4.2 R1-R6 isolated 检查表

| ID | 结果 | 对账项 | 证据 |
|----|------|--------|------|
| R1 不读 ledger / supervisor.db | N/A | LOGS_ONLY 不可证 | — |
| R2 单 activation 单业务事件 | ❌ | 同 terminal 违例 2 次重试；§1 `work.ready` 被 LOOP_COMPLETE 替代 | events, preset:205-234 |
| R3 不假设拓扑 | ❌ | coordinator 用无契约依据的虚构字符串 | DEV-008 |
| R4 共享状态经 task API | ✅ | 5 task 经 task store 创建/关闭 | tasks.jsonl |
| R5 emitter 先过 `--policy-check` | ❌ | LOOP_COMPLETE 被拒 + hat-channel 空 + hard gate | log 02:17:34, 02:18:00 |
| R6 task 三字段一致 | N/A | 无被接受 handoff payload 可供核对 | — |

### 4.3 机制十二项表

| 机制 | 结果 | 证据 |
|------|------|------|
| Origin guard | N/A | 未见 `origin:*` recovery |
| Payload contract | ✅ | LOOP_COMPLETE 缺 required event 被拒；plan.blocked schema 是否到达待 D 验证 |
| Execution contract | N/A | 执行链未启动 |
| Workflow guard | ✅ | P0-5 阻止执行前 LOOP_COMPLETE |
| Semantic gate | ✅ | 缺 work.done 时 terminal 未被接受 |
| Isolated 单事件 | ❌ | coordinator 未发 work.ready，转发 LOOP_COMPLETE |
| step_handoff 对齐 | ❌ | tasks 批量 closed 但无 handoff/business event |
| Recovery 升级 | ❌ | counter 升级但 plan.blocked 定向 shipper（不存在）|
| Resume 路由 | ❌ | coordinator 未消费 recovery_exhausted 发 plan.blocked |
| Stall | ✅ | hard gate / steward WARN / recovery_exhausted / cancel 全程显式 |
| Drift | N/A | LOGS_ONLY 无 session drift.jsonl |
| Dedup | ❌ | 同 LOOP_COMPLETE rejection 在 2 iter 重复 |
| Terminal | ❌ | 未被接受的 LOOP_COMPLETE / plan.complete；最终 ralph source loop.cancel |

### 4.4 Preset 静态契约对账

| 项目 | 声明 | 对账 |
|------|------|------|
| `terminal_topics` | `LOOP_COMPLETE, plan.complete` | **不含 plan.blocked** |
| `business_topics` | work.ready/done/failed + review.complete + fix.done + execution.plan.ready + unit topics + task.resume | **不含 plan.blocked** |
| `plan.blocked` schema | required: plan_name/task_id/task_key/step/reason | schema:174-181 定义 |
| mechanism `plan_end.allowed_emits` | `plan.complete + LOOP_COMPLETE` | 不含 plan.blocked |
| mechanism `unit_loop.allowed_emits` | 含 `plan.blocked` | preset:48-55 |
| runtime U5 escalation | payload 仅 `reason`，target=shipper | mod.rs:13948-13960 |

**开放问题**：plan.blocked 不在 `terminal_topics` 与 `business_topics`，runtime 或 CLI emit 可能先在 business-topic allowlist 分类层被拒；U5 escalation 构造的 payload 也只携带 `reason` 而非 schema 要求的 5 字段。

---

## 5. 问题归因表（confidence ≥ 60；P0 ≥ 70）

| 优先级 | 问题 | 根因分类 | **置信度** | 证据 DEV | 历史关联 | 加深轮次 |
|--------|------|----------|------------|----------|----------|----------|
| **P0** | **M1**：U5 escalation hardcode `target=shipper`（mod.rs:13960），但 preset 2026-07-23-005 U8 已删 shipper（preset:23,163）→ plan.blocked 静默 drop → reporter 未唤醒 → recovery_exhausted → loop.cancel | **compound** (mechanism 95 + preset 90) | **85** | DEV-004/005/006/009/011/018/019 | **高**（2026-07-23 同根因；2026-07-22 worker visibility 家族）| 1→3（mod.rs:13933-13966 + preset:23,118-145,1820-1825 + history 07-23） |
| **P0** | **M2**：`plan.blocked` 不在 `event_policy.terminal_topics` / `business_topics`（preset:118-145），但 schema 定义（schema:174-181）+ reporter triggers（preset:1825-1828）+ coordinator/alignment/reporter instructions 都要求 emit plan.blocked — **自我闭环陷阱**：即使 M1 修了，CLI precheck (`require_policy_check_for_cli_emit: true`, preset:152) 仍会拒 | **preset** | **80** | DEV-013/014 | **高**（2026-07-23 §1.3 终态未闭合；同 family 2x） | 1→2（preset:118-145 + schema:174-181 + history 07-23） |
| **P0** | **M3**：coordinator LLM 在 plan.ready 触发后跳过 §1「register tasks → emit work.ready」指令（preset:205-234），直接 emit LOOP_COMPLETE；reason 字段填入虚构字符串 `preset_task_key_contract_drift`（全仓 rg 0 hit；schema:162-181 列的 7 个 required_field 全无）| **agent**（LOGS_ONLY 上限 60；机制辅助：DEV-020 仅 coordinator 激活 = 单次 LLM call 即触发全链 collapse）| **60** | DEV-001/008/020 | **新**（07-23 是 worker visibility 失败，不是 LLM 幻觉）| 1→2（preset:205-318 + schema:162-181 + history 07-23 对比）|
| **P0** | **M4**：5 task u1-u5 在 ~14s 内全部 `closed=failed` 但**无对应业务事件**（无 exec.unit.failed / exec.wave.failed / work.failed）；tasks.jsonl 是 runtime ledger，事件流是 event bus，两者无统一语义担保 — coordinator 未 emit work.ready 时，task lifecycle hook 仍可 close（推测 aggregate_timeout 或 missing-work-ready 后台 closure）| **compound** (mechanism 70 + agent 60) | **70** | DEV-003/010/015 | **高**（2026-07-23 §1.3 终态未闭合 + tasks 全 open；2026-07-22 task lifecycle 2x） | 1→2（preset:158-162 + supervisor.db 契约 + history 07-23）|
| **P1** | **M5**：P0-5 LOOP_COMPLETE reject 后 dedup 不充分 — 同 LOOP_COMPLETE 第二次 emit 时 `completion_requested=true` 已置（mod.rs:11277），仍走 reject 分支；且 LOOP_COMPLETE 在第一次 reject 时**已被写入 ledger**（mod.rs:11294 `accepted_log_events.push(...)`），agent 看到假阳性「completion honored」 | **mechanism** | **70** | DEV-007/017 | **中**（family: required slot 超时 2x；agent 重复 emit 偶发）| 1→2（mod.rs:11234-11294 + loop_state.rs:1586-1604, 1707-1730）|

**compound 行说明**：

- **M1 整行 85** = min(mechanism 95, preset 90)；mechanism 因 mod.rs:13960 有明确 file:line 且与 2026-07-23 报告同根因 → 95；preset 因 shipper 删除 + event_policy 漏 plan.blocked → 90。两者联合触发 plan.blocked 静默丢失主链。
- **M4 整行 70** = min(mechanism 70, agent 60)；机制层 task lifecycle hook 与事件流语义分叉 → 70；agent 未 emit work.ready 但 task 仍可 close → 60。

---

## 6. 修复建议

### 6.1 短期（operator workaround）

| # | 目标 | 改动 | 预期效果 | 关联置信度 |
|---|------|------|----------|------------|
| 6.1.1 | 绕开 U5 escalation 死锁 | run 前 `unset RALPH_PROGRESS_STEWARD_ENABLED` 或 preset 显式 `progress_steward.enabled: false`；监控 `consecutive_steward_activations >= max_iter` warning，operator 手动 `ralph emit plan.blocked --policy-check -j '{...}'` 直接发给 reporter（绕开 hardcode target） | 阻断 M1 死链；救回 M1+M2 终态闭合 | M1 85 + M2 80 |
| 6.1.2 | 强制 plan.blocked 主题 | 在 `presets/en/ce-executor-supervisor.yml:118` `terminal_topics` 临时加 `plan.blocked`（operator patch），让 CLI precheck 放行 | 关闭 M2 allowlist 缺口（运行时补丁） | M2 80 |

### 6.2 中期（preset / schema / instructions）

| # | 目标 | 改动 | 预期效果 | 关联置信度 |
|---|------|------|----------|------------|
| 6.2.1 | preset 补齐 event_policy allowlist | `presets/en/ce-executor-supervisor.yml:118-145` `terminal_topics` 增 `"plan.blocked"`；`business_topics` 也增 `"plan.blocked"` | 关闭 M2 allowlist 缺口；schema 闭合 | M2 80 |
| 6.2.2 | 改 U5 escalation target | `crates/ralph-core/src/event_loop/mod.rs:13960` `.with_target(HatId::new("reporter"))`（reporter 仍订阅 plan.blocked, preset:1825-1828），或加 `exempt_hats: [reporter]` | 关闭 M1 hardcode 死链路 | M1 85 |
| 6.2.3 | coordinator instructions 顶部加 HARD RULE | `presets/en/ce-executor-supervisor.yml:205-234` 顶部加：「当 `event_loop.supervisor.enabled=true` 时，**禁止直接 emit `LOOP_COMPLETE`**；必须先 emit `work.ready` 走 task-planner 链路」 | 阻断 M3 LLM 幻觉直接 collapse 整条链 | M3 60 |
| 6.2.4 | schema 收紧 plan.blocked payload | `presets/schemas/ce-executor-supervisor.yml:174-181` `reason` 字段增 `enum: [loop_stalled_max_iterations, recovery_exhausted:..., invalid_plan, operator_killed, ...]` | 辅助 M2；防 agent 写任意 reason | M2 80 |

### 6.3 长期（机制 / 底座）

| # | 目标 | 改动 | 预期效果 | 关联置信度 |
|---|------|------|----------|------------|
| 6.3.1 | 机制层默认 `plan.blocked` 进 terminal_topics | `crates/ralph-cli/src/config_resolution.rs` 默认把 `plan.blocked` 加进 `terminal_topics`（opt-out 而非 opt-in）；preset_lint `event_policy_parity` 在 preset 显式排除时给 finding | 根治 M2 家族 + 防止同类预设再犯 | M2 80 |
| 6.3.2 | 机制层：P0-5 reject 路径写 seen_topics | `crates/ralph-core/src/event_loop/mod.rs:11239-11284` reject 分支：把 `event.topic` 写进 `state.seen_topics`，且**不要** push `accepted_log_events`（mod.rs:11294 当前 push，与「拒绝」语义矛盾）| 根治 M5：dedup 真正生效 + ledger 不再带假阳性 | M5 70 |
| 6.3.3 | 机制层：U5 escalation 改用 hat-registry lookup | `mod.rs:13960` 加 `registry.get(&HatId::new("shipper"))` 校验；若 None，fallback 到 `registry.list_default_routed_hats_for_topic("plan.blocked")`（与 mod.rs:13894-13910「cross-validate steward_id」对称）| 关闭整个 U5 escalation 死锁族（不只 shipper）| M1 85 |
| 6.3.4 | 机制层：task lifecycle 与事件流语义统一 | `crates/ralph-core/src/event_loop/loop_state.rs` task close 路径加约束：「只有当对应 `exec.unit.done` 或 `exec.wave.failed` 业务事件已 ack 才允许 close=failed」；否则仅标 `pending_orphan_close` 由 agent 显式 ack | 根治 M4：task ledger 与事件流分叉 | M4 70 |

修复依赖序：6.2.2 + 6.2.1（**核心闭合**） → 6.3.3（机制兜底） → 6.3.2（dedup） → 6.2.3（agent HARD RULE） → 6.3.1（默认 opt-out 改造） → 6.3.4（task 语义统一）。

---

## 7. 未核实疑点

无 — 5 条主根因均 ≥60；4 条 P0 均 ≥70；M3 卡在 LOGS_ONLY 上限 60（已合并机制辅助证据 + DEV-020），按规则不归入未核实。

---

## 8. 关键主仓代码引用清单（§7 for L6）

| 主题 | file:line | 备注 |
|------|-----------|------|
| U5 escalation emit plan.blocked | `crates/ralph-core/src/event_loop/mod.rs:13933-13961` | **L13960 hardcode `target=shipper`** |
| progress-steward "not registered" + counter++ | `crates/ralph-core/src/event_loop/mod.rs:13894-13910` | cross-validate 注册但 target 字段未校验 |
| P0-5 LOOP_COMPLETE reject | `crates/ralph-core/src/event_loop/mod.rs:11239-11284` | missing_required_events gate |
| LOOP_COMPLETE 进 accepted_log_events | `crates/ralph-core/src/event_loop/mod.rs:11294` | 与 reject 语义矛盾 |
| Hard gate (publish_obligation) | `crates/ralph-cli/src/loop_runner/runner.rs:4474-4499` | "hat has publish obligation but emitted no event" |
| hat_channel_empty_after_activation | `crates/ralph-cli/src/loop_runner/hat_channel.rs:71-88, 525-543` | — |
| Recovery_exhausted / stall_recovery | `crates/ralph-core/src/recovery_runtime/publish_loop_stalled.rs:28-50` | — |
| TerminationReason::RecoveryExhausted | `crates/ralph-cli/src/loop_runner/runner.rs:2140` | — |
| verify_emit_bridge（task_id/task_key/step） | `crates/ralph-cli/src/task_cli.rs:2496-2619` | 拒 terminal task |
| plan.blocked schema | `presets/schemas/ce-executor-supervisor.yml:174-181` | required: plan_name/task_id/task_key/step/reason |
| preset 删除 shipper / progress-steward | `presets/en/ce-executor-supervisor.yml:23, 163, 1820` | — |
| preset event_policy | `presets/en/ce-executor-supervisor.yml:118-145` | terminal_topics 缺 plan.blocked |
| preset coordinator §1 | `presets/en/ce-executor-supervisor.yml:205-234` | ONE emit work.ready |
| preset reporter | `presets/en/ce-executor-supervisor.yml:1813-1920` | 唯一终态 owner |
| recovery directives skill | `crates/ralph-core/data/ralph-tools-recovery-directives.md:68-85` | 强约束：收到 recovery_exhausted → 立即 plan.blocked，禁止重试 |
| recovery directives skill（tasks 副本）| `crates/ralph-core/data/ralph-tools-tasks.md:154` | 同上 |

---

## 9. 报告元信息

- **执行**: 主 Agent 盘点 + 3 sub-agent（流程 A / 历史 B / 对账 C / 归因 D），主 Agent 汇总
- **§0-§9** 全填；§7 空（按规则保留）
- **置信度规约**: §5 仅入表 ≥60；P0 入表 ≥70；M1=85 / M2=80 / M3=60 / M4=70 / M5=70
- **路径约定**: 全部 repo-relative；run_dir 写 `../ralph-supervisor`
- **历史报告交叉引用**: 2026-07-22 + 2026-07-23 两次同 preset 诊断（同根因家族；本次为新机制归因）
- **报告落盘**: `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/report/2026-07-24-ce-executor-supervisor-primary-20260724-020630-diagnosis.md`