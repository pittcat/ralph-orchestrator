---
title: "parallel-forge P0 Gap 对抗性审核（跨 16 次运行）"
date: 2026-09-01
type: adversarial-review
subject_preset: parallel-forge
evidence_window: 2026-07-28 .. 2026-08-29
baseline_commit: 59c0dcf06634bd9fc2b8c5dd3495e24f317fe5d3
related_active_plan: docs/plans/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan.md
follow_up_plan: docs/plans/2026-09-01-001-feat-forge-signal-delivery-reliability-plan.md
---

# parallel-forge P0 Gap 对抗性审核

> 角色：对抗性审核。样本：`docs/report/` 下 2026-07-28 → 2026-08-29 共 16 次有诊断记录的 parallel-forge 运行 + preset/schema 结构审计 + runtime 源码定位。目的：回答「为什么 parallel-forge 总是出问题，最大的 gap 在哪」。

## 0. 总体判断

16 次运行中业务成功仅 4 次（07-29-020808、08-01-003852、08-05-033341、08-27 重跑）。失败很少因为 agent 代码质量差——多次出现「commit 已正确落盘、验证 PASS，但系统判失败」。

**最大的单一 gap：「执行真相」与「账本真相」之间没有可靠投递与对账机制。** agent 的真实产出（磁盘 / PTY / worktree）和 runtime 账本（main events / supervisor.db）之间是一条无确认、无重投、无对账的单向投递链。信号丢了，runtime 一律按「hat 没干活」定罪，把可恢复故障升级成 fail-close 死锁。

当前 active 的 evidence-gates plan（2026-08-27-1430）修的是**下游**（payload 垃圾拦截），且其 §0 明确声明「stall 根因（hat-channel → main events 路由，诊断 DEV-1/9）不在本计划范围」。**上游信号丢失不在任何 active plan 的射程内**——这是修复策略的盲区：同族问题 30 天命中 3 次，一直在修症状。

## 1. 运行样本统计

| 日期 / 运行 | 结局 | 核心问题 |
|---|---|---|
| 07-28 primary-003922 | 死锁 | forge-dispatcher 从未被 spawn；worktree hat 重复 emit 被单事件预算吞掉 advance |
| 07-28 primary-110733 | 部分偏离 | PTY 零输出 120s 被 idle_heartbeat 误杀 → worker_timeout → 锁持有未达终态 |
| 07-29 primary-020808 | ✅ 成功 | 仅 P1：--rpc 进程不退出的 lock stale 假象 |
| 07-29 settlement 专项 | blocked 闭环 | work.failed 是终态死信非 retry 信号；retry_budget: 3 死代码 |
| 07-30 primary-002911 | 死锁 8h+ | planner 手工 payload `execution_wave` off-by-one 被 Rule 1 拒收，拒收无反馈无自愈 |
| 07-30 primary-094057 | fail-close 死锁 | plan.blocked 走 bus.publish 绕过 accept_event；命名空间错配（plan.blocked vs forge.plan.blocked）；LOOP_COMPLETE ×2 REJECTED |
| 08-01 primary-003852 | ✅ 健康 | P2：unit worktree 残留、inspector WARN（DISABLED 模式伪影） |
| 08-05 primary-033341 | ✅ 健康 | verifier 重复 emit ×3；final-audit.md 被引用但缺失（终态 artifact 契约缺口） |
| 08-05 primary-090210 | 成功但清理不完整 | reporter 绕过删 branch helper → 11 个 branch refs 残留 |
| 08-05 primary-133322 | 死锁 | integrator 把结算数组发成逗号分隔字符串 → CloseTaskBatch 拒收 → 拓扑死锁 |
| 08-08 primary-021642 | 用户 abort | reviewer re-review activation 空 channel（30 天命中 30 次的回归）+ 28 个 orphan-emit |
| 08-10 primary-152751 | BLOCKED | backend 失败（输出 81 bytes）被误记为「有发布义务但没发事件」——错误归因 |
| 08-26 evidence-loop plan | P0 fail-close | verifier 工作全部成功但 hat-channel merge 失败 → hard gate → fail-close；flow-authority 尾部 4 条 orphan stale |
| 08-27 evidence-loop 重跑 | ✅ 业务闭环 | 仍 2 次 channel merge 失败靠 fallback；73 个 orphan-emit 诊断文件 |
| 08-27 evidence-gates 第 1 跑 | 运行中 | 表观卡住实为 PTY 不回显 + 4 worktree 编译争 CPU（可观测性问题） |
| 08-29 evidence-gates 第 2 跑 | Wave 3 拓扑死锁 | 3/5 slot commit 落盘但 exec.unit.done 未落任何账本；2 slot running 无 failure_code；salvage_write_count=0；dispatch_records.pid 全 NULL |

## 2. 跨报告重复问题模式（按频次）

1. **hat-channel 空 / merge 失败 → hard gate 错误归因（7+ 次，最高频）**：07-28-003922、07-30-094057、08-05-133322、08-08、08-10、08-26、08-27×2。runtime 无法区分「未 emit / emit 被拒 / merge 失败 / backend 早死」。
2. **worker 完成但 exec.unit.done 不到主账本 / slot 永不收敛（3+ 次）**：07-28-110733、08-26、08-29。30 天内第 3 次同族，**仍未根治**。
3. **flow-authority stale-tail / step 不前进（4 次）**：07-30-094057、08-08、08-26、08-29。
4. **终态/失败事件契约不自洽（3+ 次）**：命名空间错配、raw LOOP_COMPLETE 被拒、loop.cancel 被 flow_unknown_emit 拒。
5. **settlement/task 关闭链断裂 → 拓扑死锁（2+ 次）**：08-05-133322、08-29。
6. **agent 手工构造 payload 值错误（3+ 次）**：planner off-by-one、integrator 字符串数组、reviewer 误填 commit。
7. **重复 emit**：verifier ×3、`exec.unit.done` ×3。
8. **cleanup 不完整**：branch refs 残留 11 个、cleanup 被 terminal_monotonicity_violation 拒收。
9. **诊断证据缺口放大一切（元问题）**：几乎全部报告是 LOGS_ONLY/MINIMAL，OPAC 置信度 ≤50~75；08-29 bundle sidecar 全空导致因果 not_evaluable。

## 3. P0 — 直接致死、反复发生、未根治

### P0-1 信号投递链无可靠性保证（最高优先级）

代码级根因（基线 59c0dcf0，行号以该 commit 为准）：

- **fan-in 前内存空洞（supervisor wave 路径）**：slot 业务事件走「slot channel 文件 → worker 退出时读入 dispatcher 内存 → 整波 JoinSet 汇合后 fan-in 一次性合并入主账本」三段式。worker 退出读回事件后 channel 文件被无条件删除（`crates/ralph-cli/src/loop_runner/wave/worker.rs:642`），此后事件唯一副本在 dispatcher 进程内存；`worker_results` 只存 `content_hash + event_count`（`crates/ralph-core/src/supervisor/migrations/v1.sql:56-64`），`record_slot_terminal_evidence` 只存指纹（`dispatch.rs:3041`）。**任一 slot 不收敛或进程被杀 → 整波已完成 slot 的事件随内存蒸发，不可重建。** 唯一写主账本的点是 `run_supervisor_fan_in`（`fan_in.rs:97` → `coordinator.rs:270` → `merge_sink.rs:137`）。
- **per-hat isolated channel 无 salvage**：空 channel → quarantine + Err（`hat_channel.rs:161-177`）；非空 channel merge IO 失败 → 只写诊断 + `error!`，channel 文件残留但永不再被消费（下一轮用新 iteration 文件名，`paths.rs:74-75`）。
- **hard gate 归因坍缩**：判定点 `crates/ralph-cli/src/loop_runner/inner.rs:4712-4716` 的 `agent_wrote_any_valid_or_rejected` 只看 main/candidate 文件解析结果；channel 字节数、merge 是否成功、backend exit code、output 非空——这些事实已被 `activation_outcome_close.rs:170-188` 采集到 outcome row，但**不参与 gate 判定**。merge 失败 / emit 被拒 / backend 早死全部坍缩成同一个 "hat has publish obligation but emitted no event"（inner.rs:4728）→ 3 次连击 → fail-close（terminal_routing.rs:322，HARD_GATE_MAX=3）。
- **`record_slot_pid` 从未接线**：API 存在（`supervisor/mod.rs:1437`、`rusqlite.rs:1564`、`memory.rs:2142`）但全仓库无生产调用点；PTY pid 在 `ralph-adapters/src/pty_executor.rs:363` 可得却未回传。`dispatch_records.pid` 因此全 NULL，`commands/diagnose.rs:1059` 注释自证「Until U7's record_slot_pid lands we approximate」。无 pid → 无死进程检测 → running slot 只能靠 per-worker 3600s / aggregate 7200s 到时收敛（08-29 外部 kill 发生在 49 分钟，远早于任何超时，slot 停 running 无 failure_code 是设计内行为）。
- **重启恢复不回捞**：`recover.rs:94-104` 对未超时 wave 直接跳过；超时只做 `set_wave_phase(Failed)`（recover.rs:124-131），**不注入 exec.wave.failed、不回捞已完成 slot 事件**；`restore_unmerged_completed_slot`（recover.rs:167）是 `#[allow(dead_code)]` 未接线。

### P0-2 失败/终态路径事件契约不自洽

- 历史 3 次死锁源于命名空间错配与终态事件被拒（部分已修：commit ba6753fa、717705f3）。
- **仍存活的缺口**：
  - `forge.audit.done` 的 `verdict` 无 `allowed_values`（schema 只有 required_fields），finalizer `triggers: [forge.audit.done]` 无 verdict 过滤——审计 REJECTED/BLOCKED 也会激活 finalizer 的真实 `git merge --ff-only`。runtime 证据：hat triggers 是纯 topic 字符串匹配（`config/hat.rs:353-356`、`event_processing.rs:97`），**不支持 payload 字段过滤**；唯一的 payload 谓词 `TriggerPredicate.payload_field_equals` 只服务激活后的 emit 义务，不做路由过滤。
  - `business_topics` 漏掉 `forge.wave.reviewed/integrated/verified/settled`、`forge.correction.*` 等全部 wave 内事件 → 绕过 completion guard（`validation.rs:25-53`）与 terminal-closed guard（`terminal_closed_guard.rs:77-100`）的业务校验。
  - `work.failed` 的 90/75 死胡同置信度门禁只在 instructions，不在 schema required_fields —— 纯 prompt 级。
- 注：verdict 过滤与 payload 证据门禁已在 active plan 2026-08-27-1430 射程内（其 S/E 系列单元），本项的 runtime 侧结论（triggers 不支持字段过滤）是该 plan 的边界约束。

### P0-3 wave 内环 9 事件链全靠 agent 手工透传

- 每波 prepare→worktrees→dispatch→fan-in→review→integrate→verify→settle 约 9 跳，wave_id/wave_index/plan_key 逐跳手工透传；`forge.correction.requested` 12 个 required 字段、`exec.unit.ready` 8 个。
- 实证两次死锁：planner `execution_wave` off-by-one（07-30 死锁 8h）、integrator 字符串数组（08-05 死锁）。
- HARD RULE 4 违反实证：reviewer instructions 要求「从 trigger 读 wave_index 和 plan_key」，但 `exec.wave.complete` schema 只有 `wave_id/completed_slots/merge_root_event_id`——trigger 里根本没有这两字段。
- 注：payload_consistency / precheck 门禁属 active plan 射程；「由 runtime 注入 wave 上下文取代手工透传」不在，列为后续方向。

### P0-4 correction 收敛没有 runtime 锚点

- 轮次计数靠 agent 自维护磁盘文件 `corrections/counter`（failure-handler instructions），runtime 无强制、无读取。
- `correction_round` 的 0..3 只在 field_docs；唯一 runtime 锚点是 `forge.final.correction.settled` 的 `allowed_values: [3]`（实现：`validation.rs:1611-1631`，enforce + reject_with_resume 下拒收并注入 correction）。**allowed_values 只能校验单事件字段值，无跨事件计数能力**——counter 文件丢失/错读 → 超轮空转，或 off-by-one 后 final settled 被永久拒绝 + resume 空转。
- `failure_fingerprint` 需 agent 按固定字符串格式手工合成，错一字节破坏重复轮次检测。
- 现有可复用机制调查结论：跨事件计数无声明式机制，red-team 队列式有状态门禁是硬编码 Rust（`validation.rs:124-437`）；`ralph tools` 只有 memory/task/skill 三个子命令，无 counter/kv API。

## 4. P1 — 不直接致死，但放大故障率/阻碍修复

- **可观测性缺口（元问题）**：LOGS_ONLY/MINIMAL 压垮 OPAC 置信度；`log_runtime_trace` 在 Minimal 模式早退（`crates/ralph-core/src/diagnostics/mod.rs:1662`），`hat_activation_outcome` 行根本不落盘——08-29「runtime-trace.jsonl 缺失」的机制根因。「查不清根因」是同族问题 30 天复发 3 次的直接推手。
- **并发波次 vs 单例 projection 键冲突**：最多 3 波并发，但 `forge.wave.current_wave_id` 是单例键；failure-handler 的 fail-closed 校验（current_wave_id != trigger.wave_id → blocked）在并发波次下会误伤。且顶层 `state_projection:` pin 表（preset yml:1657-1779）**无 runtime 消费者**（RalphConfig 无该字段，serde 静默丢弃；pin 键全仓库 Rust 零读取方）——failure-handler「从全局 projection 恢复 wave 上下文」永远读到空。
- **flow-authority stale-tail 复发（4 次）**：已有 solution 文档仍复发，修复没落到机制。
- **重复 emit / 缺幂等**：verifier ×3、exec.unit.done ×3。
- **`record_slot_result` warn-only 失败的不可见性**：失败不影响收敛（Drop guard 兜底置 completed，rusqlite.rs:694-705 不写 worker_results）但 worker_results 静默缺失——08-29 的 worker_results=0 疑似此路径，需复跑取证确认。

## 5. P2 — 效率与清理

- cleanup 链不完整：branch refs 残留 11 个；LOOP_COMPLETE 后 cleanup 被 terminal_monotonicity_violation 拒收。
- 预算调参：`max_iterations: 60` 对 16 hat × 多波 × 9 事件/波偏紧；`slot_retry_budget: 2` × executor timeout 3600s 串行可达 3h，会先撞 `aggregate_timeout_secs: 7200`，把慢 slot 放大成 wave failed。

## 6. P3 — 配置卫生

- 遗留 alias 三件套（`forge.units.reviewed` / `forge.integration.done` / `forge.incremental.verified`）仍挂 triggers/publishes/business_topics；cleanup 订阅 alias 被触发即空转。
- `business_topics` 里躺着无 schema 无 publisher 的死 topic `forge.incremental.verified`。
- `retry_budget: 3` 死代码（work.failed 是 NON_TRANSITION 终态死信）。
- 顶层 `state_projection:` pin 表死配置（见 P1）。

## 7. 处理建议（已落地的部分）

1. **P0-1 优先于一切** → 已出实施计划：`docs/plans/2026-09-01-001-feat-forge-signal-delivery-reliability-plan.md`（slot 事件本体持久化 + 重启补偿投递 + hard gate 归因分类 + pid 接线 + 现场保留 + MINIMAL 诊断修复）。修好这条，历史 7 次投递类死锁里至少 5 次不会发生。
2. **P0-2 / P0-3** → 继续执行 active plan 2026-08-27-1430（payload_consistency + precheck 门禁），不另起计划；runtime 侧「triggers 不支持 payload 过滤」的边界约束已写入本报告 §3。
3. **P0-4** → 单独立项：correction 轮次需要 runtime 跨事件计数锚点（硬编码门禁或通用 counter API），设计候选见本报告 §3，需独立计划，不阻塞 P0-1。
4. **P1 可观测性** → 部分并入 P0-1 计划（U4 pid、U5 现场保留、U6 MINIMAL 修复）；record_slot_result warn-only 可见性留待复跑取证后立项。
5. **P2/P3** → 随 active plan 收尾或 hygiene 批次处理，不单独占用计划。
