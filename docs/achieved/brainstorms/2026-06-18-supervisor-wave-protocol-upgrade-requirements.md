---
superseded_by: docs/brainstorms/2026-07-03-supervisor-rusqlite-parallel-preset-requirements.md
date: 2026-06-18
topic: supervisor-wave-protocol-upgrade
type: requirements
related:
  - docs/achieved/brainstorms/2026-06-17-ce-executor-flow-reliability-requirements.md
  - docs/achieved/brainstorms/2026-06-17-wave-dimension-assignment-enforcement-requirements.md
  - docs/achieved/brainstorms/2026-06-17-ce-executor-step-handoff-requirements.md
  - docs/achieved/brainstorms/2026-06-18-recovery-escalation-routing-requirements.md
  - docs/report/2026-06-17-ce-executor-wave-abstraction-issues-diagnosis.md
---

# Ralph Supervisor + Wave 协议升级需求文档

> **定位**:Ralph Supervisor 协议层的"母舰"需求文档。覆盖 6 件套(backpressure / 分布式取消 / 状态持久化 / 幂等键 / 内容哈希去重 / 补偿路径)的完整需求。现有 4 份子需求文档(flow-reliability / step-handoff / wave-dimension-enforcement / recovery-escalation-routing)已归档至 `docs/achieved/brainstorms/`,本母舰文档为唯一权威来源。

## Summary

Ralph Supervisor 协议层升级到 2026 业界标配:在现有 `wave_tracker.rs` / `wave_detection.rs` 基础上实现 6 件套(backpressure / 分布式取消 / 状态持久化 / 幂等键 / 内容哈希去重 / 补偿路径),以轻量化协议层附加方式(方案 1)落地,全 6 件套由 Ralph 内部实现,backend 只管派 Subagent。为未来路径 B(真 Subagent 集成)铺路,同时解决 keen-fern 1h47m / zippy-sparrow 等 loop 失控问题。

---

## Problem Frame

Ralph 的 wave/fan-out 抽象目前只有"形"(wave_id + wave_index + prompt 头一行),没有"实"(进程隔离 + 上下文隔离 + mailbox)。keen-fern(1h47m52s)、zippy-sparrow(1h04m)等 loop 反复出现 5 类失败模式:

| 症状 | 根因 |
|---|---|
| 9 个 worker 共享 `events.jsonl` 写流互相穿插 | 无进程隔离 |
| synthesizer 等不齐 N 维,触发 R6 incomplete_wave_gate | 无 worker 间可见性 |
| 12s 内二次 `work.done` 同 payload | 无幂等键 |
| R6 触发后 worker 还在烧 token | 无分布式取消 |
| 进程挂后 wave 状态全丢 | 无状态持久化 |

业界 2026 共识:Supervisor 协议必须包含 6 件套(LangGraph Supervisor / AutoGen GroupChat v0.8.7 / CrewAI 2026 / SagaLLM 论文全部验证)。Ralph 现有代码有 4 个亮点(typed rejection / per-slot retry / dimension assignment / dual API),缺 3 件关键(backpressure / 取消 / 持久化)直接导致失控。

---

## Key Decisions

**架构:协议层附加(方案 1),不做 Supervisor 实体化(方案 2)**。在 `wave_tracker.rs` / `wave_detection.rs` 里加字段+方法,不新建 Supervisor 实体,不重写 `HatRegistry`,preset 完全不动。`WaveState` 预留"未来可升级 SupervisorState"的 hook(字段命名对齐,API 设计兼容)。

**实现:全 6 件套 Ralph 自做,backend 不管**。Ralph 是单一 Supervisor 控制点,所有协议能力内部实现。backend(Claude Code / Codex / Gemini)只管派 Subagent,不负责 backpressure / 取消 / 持久化 / 幂等 / 去重 / 补偿。

**演进:路径 C 串行先行,本需求为路径 B 铺路**。`ce-executor-isolated` 先走串行(路径 C)止血,本需求的 6 件套为未来真 Subagent 集成准备基础设施。

**子文档:归档不动**。4 份子需求文档归档 `docs/achieved/brainstorms/`,本母舰文档为唯一权威来源。机制/选型/验收标准以母舰为准。

---

## Requirements

### A. Backpressure(反压)

- **R-A1.** `WaveTracker` 必须记录当前活跃 worker 数 `active_workers: usize`,`dispatch` 前检查 `active_workers >= max_concurrent_workers`(默认 16,可 preset 覆盖),超出时**不 spawn**而是 enqueue 并返回 `DispatchOutcome::BackpressureEnqueued`。
- **R-A2.** `WaveState` 记录 `enqueued_waves: Vec<DetectedWave>`,当活跃 worker 降至阈值以下时,按 FIFO 顺序调度 enqueued waves。
- **R-A3.** backpressure 触发时写入诊断事件 `wave.backpressure.paused(wave_id, active_workers, queue_depth)`,供 `ralph diagnose` 消费。
- **R-A4.** `ralph wave dispatch --force` 绕过 backpressure 检查,用于紧急场景;写入 `wave.backpressure.bypassed(wave_id, reason)` 审计。

### B. 分布式取消

- **R-B1.** `WaveState` 增加 `cancel_token: Arc<AtomicBool>`,`cancel_wave(wave_id, reason)` 设置 flag 并写 `wave.cancelled(wave_id, reason, cancelled_at)` 诊断事件。
- **R-B2.** dispatch loop 在每次 spawn worker 前检查 `cancel_token`,若为 true 则跳过该 worker 并标记为 `Cancelled` 而非 `Completed`。
- **R-B3.** `cancel_wave` 可被调用时,必须同时 kill 已在跑的 worker 进程(PID 通过 backend executor 暴露);若 backend 不支持 PID 暴露,则通过 backend cancel protocol(如 Claude Code Task tool 的 `termination_reason`)执行取消。
- **R-B4.** 取消完成后,若 wave 为 partial 状态,必须触发 `incomplete_wave_gate` 降级路径(复用 flow-reliability R-A5 降级出口),不得静默丢弃 partial 结果。

### C. 状态持久化

- **R-C1.** 每次 `WaveState` 字段变更后,同步写入 `.ralph/wave-state/{wave_id}.json`;文件名含 wave_id,不可预测路径。
- **R-C2.** 进程启动时,从 `.ralph/wave-state/` 恢复所有 active wave 状态;若 wave 超时(`started_at + timeout > now`),标记为 `StaleForRecovery` 并触发 R-B4 取消路径。
- **R-C3.** `CompletedWave` 写入 `.ralph/wave-state/completed/{wave_id}.json`,保留最近 20 个;旧文件由 `WaveStateStore` 清理策略管理(默认保留 7 天)。
- **R-C4.** 持久化失败时,**不阻塞内存状态**,只写 `wave.persistence.failed(wave_id, error)` 诊断事件并继续运行;进程重启后依赖内存状态恢复(如内存状态也丢失则进入 R-C2 StaleForRecovery 路径)。

### D. 幂等键

- **R-D1.** 每个 dispatch 事件必须携带 `idempotency_key: String`,格式为 `{wave_id}:{wave_index}:{payload_content_hash}`。
- **R-D2.** `WaveTracker` 维护 `dispatched_keys: HashMap<String, DispatchRecord>`,记录每个幂等键的 dispatch 时间戳和状态;收到同 key 的重复 dispatch 时,返回 `DispatchOutcome::DuplicateKey(existing_record)`,**不重新 spawn**。
- **R-D3.** `DispatchRecord` 包含 `dispatched_at: Instant`,`worker_pid: Option<u32>`,`status: DispatchStatus`(Pending / Running / Completed / Cancelled / Failed);状态为 Completed/Cancelled 时,key 在 `SLIDING_WINDOW`(默认 10 分钟)后过期并从 `dispatched_keys` 移除。
- **R-D4.** dedup 只在 worker spawn 前生效;若 worker 已 spawn(Running 状态)则允许同 key 重派(worker 内部自己处理幂等,backend 负责)。

### E. 内容哈希去重

- **R-E1.** worker 结果写入前,计算 payload 内容哈希 `content_hash: String`(SHA-256 前 16 位);`WaveResult` 增加 `content_hash` 字段。
- **R-E2.** `record_result` 前检查同 wave_id 同 `wave_index` 是否已有 `content_hash` 相同的记录,若有则写入 `wave.result.deduplicated(wave_id, wave_index, content_hash)` 并跳过写入,返回 `WaveProgress::Unchanged`。
- **R-E3.** 若 content_hash 不同但 wave_index 相同(同一 slot 的不同结果),写入 `wave.result.replaced(wave_id, wave_index, old_hash, new_hash)`,用新结果替换旧结果。
- **R-E4.** `ralph diagnose` 显示 deduplicated count 统计,用于识别"worker 重复 emit 同一结果"模式。

### F. 补偿路径

- **F1.** `WaveState` 增加 `compensation_plan: Option<CompensationPlan>`;`CompensationPlan` 包含 `on_failure: Vec<CompensationAction>`,`on_timeout: Vec<CompensationAction>`,`on_partial: Vec<CompensationAction>`。
- **F2.** `CompensationAction` 为 tagged union:`{ kind: "emit_event", topic, payload }` 或 `{ kind: "call_hook", hook_id }` 或 `{ kind: "noop" }`。
- **F3.** 当 wave 进入 Failed / Timeout / Partial 状态时,`WaveTracker` 同步执行对应 `CompensationPlan` 中的 actions;执行结果写入 `wave.compensation.executed(wave_id, action_kind, action_index, result)`。
- **F4.** 补偿执行失败时,**不阻塞 wave 完成**,只写 `wave.compensation.failed(wave_id, action_kind, error)` 并继续;compensation 是"尽力而为"而非"强保证"。
- **F5.** preset 通过 `hat.wave.compensation` 配置块声明补偿计划;无配置时使用默认 no-op。

---

## Key Flows

- F1. **正常 fan-out wave(R-A1 + R-B2 + R-D1 + R-E1 + R-C1)**
  - **Trigger:** review-coordinator emit `review.wave.ready` × 4,dimension 互不相同。
  - **Actors:** dispatcher(A2),dimension-reviewer workers(A3 × 4),synthesizer(A4)
  - **Steps:** dispatcher 收到 wave → 检查 backpressure(R-A1) → 计算 idempotency_key(R-D1) → spawn workers → 各 worker 写 results → 内容哈希去重(R-E1) → 持久化 WaveState(R-C1) → synthesizer 聚合
  - **Outcome:** 4 维全齐,wave 关闭,compensation 不触发。
  - **Covered by:** R-A1,R-B2,R-D1,R-E1,R-C1

- F2. **Backpressure 暂停 wave(R-A1 + R-A2)**
  - **Trigger:** 活跃 worker 数已达 max_concurrent_workers(16),新 wave 到达。
  - **Actors:** dispatcher
  - **Steps:** dispatch 前检查 active_workers >= 16 → 返回 DispatchOutcome::BackpressureEnqueued → enqueue 到 WaveState.enqueued_waves → 写 wave.backpressure.paused 诊断 → worker 完成腾出槽位 → FIFO 调度 enqueued wave
  - **Outcome:** wave 不丢失,只是延迟调度,无 token 浪费。
  - **Covered by:** R-A1,R-A2,R-A3

- F3. **分布式取消 wave(R-B1 + R-B2 + R-B3 + R-B4)**
  - **Trigger:** supervisor 决定取消 wave(用户 interrupt 或 stall_recovery escalation)。
  - **Actors:** supervisor,dispatcher,backend
  - **Steps:** cancel_wave(wave_id, reason) → set cancel_token=true → 写 wave.cancelled 诊断 → dispatch loop 检测 cancel_token 跳过未 spawn worker → kill 已在跑 worker(PID 或 backend cancel protocol) → partial 状态触发 incomplete_wave_gate 降级
  - **Outcome:** wave 取消,partial 结果走降级路径,无 worker 空跑 token。
  - **Covered by:** R-B1,R-B2,R-B3,R-B4

- F4. **进程重启恢复 wave(R-C2)**
  - **Trigger:** loop runner 进程重启。
  - **Actors:** dispatcher
  - **Steps:** 进程启动 → 扫描 .ralph/wave-state/ 恢复 active waves → 检查每个 wave 的 timeout → 超时 wave 标记 StaleForRecovery → cancel_wave 走 R-B4 降级路径;未超时 wave 继续等待 worker 结果
  - **Outcome:** 进程挂后 wave 状态不丢失,超时 wave 有明确恢复路径。
  - **Covered by:** R-C2,R-B4

- F5. **Worker 重复 emit 同结果(R-D1 + R-E2)**
  - **Trigger:** 同 worker 因 backend 重试发了两次 `review.dimension.done` 同 payload。
  - **Actors:** dispatcher,worker
  - **Steps:** 收到同 idempotency_key 的第二次 dispatch → 返回 DispatchOutcome::DuplicateKey 跳过 spawn;若同 wave_index 不同 content_hash → 走 R-E3 replace 路径
  - **Outcome:** 避免 worker 重复工作导致的资源浪费和状态不一致。
  - **Covered by:** R-D2,R-E2,R-E3

---

## Scope Boundaries

### In Scope

- `crates/ralph-core/src/wave_tracker.rs` — R-A1~A4, R-B1~B4, R-C1~C4, R-D1~D4, R-E1~E4, F1~F5
- `crates/ralph-core/src/wave_detection.rs` — backpressure gate,dispatch with idempotency check
- `crates/ralph-core/src/event_loop/` — cancel_token 传播,compensation hook 调用
- `.ralph/wave-state/` — 持久化文件管理,清理策略
- `crates/ralph-core/src/wave_context.rs` — Supervisor wave context 注入(R1 / R3 / R5 现有能力,本需求不做新改动但需兼容)
- 验收测试:wave/tracker + backpressure + cancel + persistence + idempotency + dedup + compensation 的单元测试
- BDD scenario:覆盖 F1~F5 所有关键路径

### Out of Scope

- Supervisor 实体化(方案 2 预留,不在本需求范围)
- `HatRegistry` 重构或新增 Supervisor 角色
- `ce-executor-isolated.yml` / `ce-executor-wave.yml` preset 修改(preset 不动,协议层兼容现有拓扑)
- backend executor 修改(backend 只管派 Subagent,不做适配)
- 非 wave 的普通 hat lifecycle 取消/持久化(本需求只覆盖 wave 相关路径)

### Deferred for Later

- Supervisor 实体化(方案 2):当路径 B 真 Subagent 集成启动时,评估是否需要将 WaveState 升级为 SupervisorState
- 跨-wave 的全局 backpressure(当前 per-wave 调度,未来可能需要全局 worker pool)
- 补偿路径的"回滚文件系统改动"能力(当前只支持 emit_event / call_hook,文件系统回滚由 backend 负责)

### Outside This Product's Identity

- 在 wave 外部实现通用 hat lifecycle 取消(不在 wave 路径里,不做)
- 实现"全局事务"(跨多个 wave 的原子性保证):SagaLLM 论文的完整 Saga 模式超出本 Ralph 版本范围

---

## Success Criteria

- **SC1:** `keen-fern` / `zippy-sparrow` 同类失败模式在升级后不再出现。具体:无 12s 内二次 `work.done`、无 wave 等不齐触发 R6 stall、无 worker 在 wave 取消后继续烧 token。
- **SC2:** `cargo nextest run -p ralph-core -- wave` 全通过;BDD scenario 覆盖 F1~F5 全部关键路径。
- **SC3:** 进程挂后重启,未超时的 wave 能继续运行(`R-C2` StaleForRecovery 路径验证)。
- **SC4:** backpressure 触发时,`ralph diagnose` 显示 `wave.backpressure.paused` 事件,`ralph wave status` 显示 enqueued count。
- **SC5:** 幂等键 dedup 生效时,同一 idempotency_key 重复 dispatch 不产生新 worker 进程,日志有 `DuplicateKey` 记录。
- **SC6:** 6 件套的实现不影响当前 `ce-executor-serial` 串行 preset 的行为(backward compat)。

---

## Dependencies / Assumptions

- D1:backend executor 暴露 worker PID 或支持 cancel protocol;若不支持,`R-B3` 的 kill 步骤为 no-op(保留 cancel_token 标记但不实际 kill)。此为已知限制,不影响其他 5 件套验收。
- D2:`.ralph/wave-state/` 目录由 `ralph init` 或首次 wave 触发时创建,持久化写入失败不阻塞内存状态(R-C4 尽力而为原则)。
- D3:本需求的 6 件套与已有 4 份子需求文档(flow-reliability / step-handoff / wave-dimension-enforcement / recovery-escalation-routing)存在能力交叉(如 flow-reliability R-A4 partial wave 与 R-B4 取消后 partial 降级)。交叉点以本母舰文档为准,子文档已归档。
- A1:Claude Code / Codex / Gemini 等 backend 在真 Subagent 模式下能自行处理"派了 Subagent 但 Subagent 失败"的场景,Ralph 不负责 Subagent 内部失败恢复。

---

## Outstanding Questions

- **OQ1.(Resolve Before Planning)** `R-B3` 的 backend cancel protocol 兼容性:Claude Code Task tool 是否支持 cancel?Codex Agent Mode 的 cancel 语义是什么?需要在规划阶段验证,否则 `R-B3` kill 步骤可能为 no-op。
- **OQ2.(Deferred to Planning)** `max_concurrent_workers` 的默认值(16)是否需要 preset 覆盖机制?还是固定 16?
- **OQ3.(Deferred to Planning)** compensation plan 的 YAML DSL 设计是否需要在 preset 中可声明?还是纯 Rust struct 配置?
