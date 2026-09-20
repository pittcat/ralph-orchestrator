---
title: "feat: parallel-forge 信号投递可靠性与正确归因（slot 事件持久化 + 崩溃恢复投递 + hard gate 归因）"
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
status: active
origin: docs/report/2026-09-01-parallel-forge-p0-gaps-adversarial-review.md
plan_depth: deep
plan_status: READY
baseline_commit: 21dfbab42680801601f8e9cec652905201e32c24
previous_baseline: 59c0dcf06634bd9fc2b8c5dd3495e24f317fe5d3
rebased: 2026-09-01
---

# feat: parallel-forge 信号投递可靠性与正确归因

> 目标 gap：诊断报告 §3 P0-1（信号投递链无可靠性保证）。本计划只修 runtime 投递/归因机制，不改 preset 拓扑、不改 payload 门禁（`2026-08-27-1430` 证据门禁已合入 `21dfbab`，正交且已落地）。
> Executor 硬性禁令：① 不得修改 `presets/**`、`crates/ralph-core/tests/scenarios.rs`、`crates/ralph-core/tests/scenarios/parallel_forge_*`（证据门禁契约已合入，回改会破坏已落地 handoff；E18）；② 不得改变健康路径 fan-in 的事件来源（仍读内存，D2）；③ 不得给 `exec.unit.done` 加任何 emit 侧门禁；④ 不得引入主动杀进程的 reaper（D6 非目标）；⑤ 测试入口必须 `cargo nextest run` 系列（AGENTS.md HARD RULE 1/2）；⑥ 集成测试 spawn ralph 必须用 `crates/ralph-cli/tests/common/mod.rs` 的 `common::ralph_bin()` / `scrub_agent_runtime_env`（HARD RULE 5）。

---

## 0. 计划状态

* **状态：`READY`** — 所有实施关键决策置信度 ≥ 0.85（§3）。2026-09-01 已按 `21dfbab` 重核行号与外部约束。
* **基线：** `21dfbab42680801601f8e9cec652905201e32c24`（`feat(parallel-forge): U11 verifier ingress parity + dynamic CLI scenario 验收`）。前基线 `59c0dcf0` 仍作调查起点：`59c0dcf0..21dfbab` 对本计划目标 runtime 文件几乎零 diff（仅 `diagnosis/reporter.rs` 1 行），证据门禁合入集中在 `presets/**` / scenarios / runtime-contract。所有行号以当前 HEAD 为准；Executor 进入每个 Unit 第一步用 `sed -n` 复核，漂移超过 ±20 行时以符号名定位，符号也找不到 → 触发该 Unit 停止条件。
* **调查范围：** 同前（loop_runner / supervisor / diagnostics / pty_executor / diagnose）。另核：`crates/ralph-core/src/event_origin.rs`（协调 topic 白名单）、`crates/ralph-cli/src/loop_runner/activation_outcome.rs`（outcome 行落盘入口）、`crates/ralph-core/src/config/telemetry.rs`（Minimal session 激活条件）。
* **已执行的验证：** 原调查 + 2026-09-01 对 HEAD 符号重核（`recover_active_waves_at_startup` / `run_supervisor_fan_in` / hard gate 文案 / `log_runtime_trace` / `SUPERVISOR_COORDINATION_TOPICS`）；`git status` 工作区干净；`git log 59c0dcf0..HEAD` 确认证据门禁已合入。
* **尚未执行（留给 Executor 的 Red/Green，不是阻塞）：** 新增 store API 的 round-trip、恢复投递集成测试等全部目标测试；最终全量 `./scripts/run-tests.sh`。
* **阻塞项：** 无。
* **外部约束：** `docs/plans/2026-08-27-1430-feat-parallel-forge-evidence-gates-plan.md` 的生产改动已合入（`3a80084a` / `bb699151` / `21dfbab4`）。本计划仍不碰 preset/scenario；目标 runtime 文件与那次合入无实质交叉。若执行中发现后续 commit 改动 `wave/dispatcher/*`、`supervisor/*`、`inner.rs`，触发受影响 Unit 停止条件并重核行号。
* **重定基线后的机制修正（必须读）：** ① U3 停止条件「`exec.wave.failed` 是否在 system_injected 白名单」已在 `crates/ralph-core/src/event_origin.rs:133-140` 确认**在列**，不再是未知项。② U4 的「既有 backend 失败分支 `inner.rs:3462-3484`」已漂移：该段现为 evidence-window flush；watchdog 在 `:3389-3437` 明确继续走 missing-event hard gate。③ U7 的「Minimal 诊断级别早退」描述不成立——不存在 DiagnosticLevel 枚举；真实拦截是 `session_dir`/`runtime_trace_logger` 为 None。详见 E13 / U7。

---

## 1. 功能目标

* **业务目标：** supervisor wave 中 slot 已完成的业务事件（`exec.unit.done` 等）在「worker 退出 → fan-in 合并」窗口内不再仅存于 dispatcher 进程内存；loop 进程在该窗口被杀并重启后，已完成 slot 的事件能补偿投递到主 events 账本，wave 能收敛（complete 或 failed）而不是拓扑死锁；isolated hat「无事件」时 runtime 能区分「没发 / 发了但被拒 / merge 失败 / backend 早死」，不再一律按「有发布义务但没发」定罪。
* **用户/调用方：** builtin `parallel-forge`（execution_model: supervisor）及一切走 supervisor wave 的 preset；`ralph diagnose` 使用者；wave 恢复路径（loop 重启）。
* **当前行为（基线，E1–E17）：**
  * slot 事件三段式：slot channel 文件 → worker 退出时 `read_worker_events`（`wave/io.rs:335`）读入内存 → 整波 JoinSet 汇合后 `run_supervisor_fan_in`（`dispatcher/fan_in.rs:97`）一次性合并入主账本；worker 退出后 channel 文件被无条件删除（`wave/worker.rs:642`）。`worker_results` 只存 `content_hash + event_count`（`supervisor/migrations/v1.sql:56-64`）。**窗口内进程死亡 = 事件永久丢失**。
  * 启动恢复 `recover_active_waves_at_startup`（`supervisor/recover.rs:65-139`）对超时 wave 只 `set_wave_phase(Failed)`（`:124-131`），**不注入 `exec.wave.failed`、不回捞已完成 slot 事件**；`restore_unmerged_completed_slot`（recover.rs:167）是 `#[allow(dead_code)]` 未接线。恢复成功后仅回放 task 投影（`recover_pending_projections`，inner.rs:1078/1171）。
  * hard gate 判定（`inner.rs:4712-4732`）只看 main/candidate 账本解析结果；`activation_outcome_close.rs:170-188` 已采集 channel_bytes/merge_succeeded/backend_success/output_bytes 等事实但**不参与判定**；merge 失败 / backend 早死 / 真空 activation 坍缩成同一 "hat has publish obligation but emitted no event"（inner.rs:4731），3 次连击 → fail-close（`event_loop/terminal_routing.rs:322`）。watchdog 超时路径（inner.rs:3389-3437）注释明确「若没 emit 则走 missing-event hard gate」——这是 U4 BackendDied 误定罪的现码锚点。
  * `record_slot_pid` API 存在（`supervisor/mod.rs:1437`）但全仓库无生产调用点；`dispatch_records` INSERT 不含 pid（rusqlite.rs:586）；pid 在 `pty_executor.rs:363,1813` 可得。`diagnose.rs:1059` 注释仍写「Until U7's `record_slot_pid` lands」——那是旧计划编号，本计划对应 **U5**。
  * worker channel 失败现场被删除（worker.rs:642）；isolated hat channel 有 quarantine 模式（`hat_channel.rs:287-313`）但 wave worker 侧没有。
  * `hat_activation_outcome` 行：`activation_outcome.rs:406-411` 在 `session_dir()` 为 None 时直接 return；`diagnostics/mod.rs:1662` 在 `runtime_trace_logger` 为 None 时 return。Minimal session（`telemetry.runtime_diagnosis.enabled && write_artifacts`）**会**创建 logger（mod.rs:927）。默认二者均为 false → Disabled collector → 行不落盘。08-29 DEV-8 的主叙述是 cap manifest 空，不是「DiagnosticLevel 早退」。
* **目标行为：** 见 §4 BDD。一句话：slot 事件本体在 worker 退出时即持久化到 supervisor store；崩溃恢复能补偿投递并注入收敛事件；silent activation 按事实分类处理；pid、channel 现场、outcome 行三类取证证据落盘。
* **行为差异：** 见 §4 每个 Scenario 的 Given/When/Then；健康路径（无崩溃、无 merge 失败）的可观察行为**不变**（D2 明确约束）。
* **本次范围：** R1–R7（§6）。**非目标：** ① preset/schema/payload 门禁（已由 2026-08-27-1430 合入，禁止回改）；② 主动死进程 reaper / 周期性 coordinator tick（需独立立项）；③ `record_slot_result` warn-only 可见性（需复跑取证后立项，见诊断报告 §4）；④ correction 轮次 runtime 锚点（P0-4，独立立项）；⑤ worktree/cleanup/预算调参（P2/P3）；⑥ per-hat isolated channel 的跨 activation 重试（本计划只改 gate 归因与 merge 单次重试）。
* **输入：** worker 退出时的 slot channel 事件集；supervisor store 中的 wave/slot 状态；activation outcome 事实。
* **输出：** 主 events 账本行（补偿投递 / `exec.wave.failed` 注入）；supervisor store 新表行；诊断文件与 runtime-trace 行；`ralph diagnose` 输出。
* **状态变化：** supervisor store 新增 `slot_event_payloads` 表；`dispatch_records.pid` 从恒 NULL 变为真实 pid；recovery 从「只标 phase」变为「投递 + 注入收敛事件」。
* **错误语义：** store 写入失败降级为 warn + 保留 channel 文件（不阻断 fan-in，S1.3）；恢复投递失败不阻断 loop 启动（warn + 下轮重试，与 recover 现有容错一致，inner.rs:1090 的 Err 分支模式）。
* **兼容性要求：** supervisor.db 走新 migration（旧库自动迁移）；无持久化负载的旧崩溃遗留 wave 恢复行为不 panic、维持现状（S2.4）；内存 store 与 rusqlite store 双实现语义一致（E14 模式）。
* **性能要求：** 持久化为每 slot 一次单行写入（事件集 JSON 序列化），相对 wave 3600s 级超时开销可忽略；恢复扫描只扫 active wave（`recover_active_waves` 既有查询，recover.rs:70）。
* **安全/权限要求：** 无新外部输入；事件负载来源是 runtime 自己读回的 slot channel（既有信任边界）。
* **已知约束：** 单事件预算、isolated 模式、hat env 注入语义全部不变；不得改 `ralph emit` / `ralph wave emit` worker 侧路由（emit_path.rs 不动）。
* **已确认假设：** A1 事件可通过 `Event::new(topic, payload).with_source(..).with_wave(..)` 重建（merge_sink.rs:205-207 测试实证）。A2 恢复路径可拿到 main events 文件路径与 store（inner.rs:1060-1100 实证，二者均已存在）。A3 fan-in 重读发布（dispatch.rs:1165-1177）对恢复追加的行同样生效（同一文件、同一 reader cursor 机制，merge_sink.rs:24-33 契约）。A4 `exec.wave.failed` 已在 `SUPERVISOR_COORDINATION_TOPICS`（event_origin.rs:133-140）。
* **待验证假设：** 无（全部已在调查期闭环；剩余的行号漂移属 Unit 入口常规复核，非决策级不确定性）。

---

## 2. 代码库现状与证据

### 2.1 当前实现入口与调用链

**slot 事件投递链（本计划主战场）：**
```
forge-dispatcher `ralph wave emit exec.unit.ready`（wave.rs:805，写主账本）
  → loop runner 检出 wave → handle_wave_events（dispatch.rs:530）
  → execute_wave_via_supervisor_with_executor（dispatch.rs:1695）
  → per-slot channel .ralph/wave-<wave>-<slot>.jsonl（dispatch.rs:1899）+ env 注入（dispatch.rs:1919-1927）
  → JoinSet spawn（dispatch.rs:2531）→ run_wave_worker（worker.rs:64）→ PTY（pty_executor.rs）
  → worker 内 `ralph emit` 写 slot channel（路由 emit_path.rs:414-456，写盘零门禁）
  → worker 退出 → read_worker_events（io.rs:335）→ 【删除 channel：worker.rs:642】
  → slot 终态落 store：begin/finish_slot_attempt（dispatch.rs:2739/2809）、
    classify（outcomes.rs:181）、record_slot_result（dispatch.rs:3009 → rusqlite.rs:711）、
    record_slot_terminal_evidence（dispatch.rs:3041 → rusqlite.rs:903，只存指纹）
  → 整波 JoinSet 汇合 → run_supervisor_fan_in（fan_in.rs:97；调用点 dispatch.rs:928）
  → coordinator.tick_with_slot_events（coordinator.rs:158）→ merge_and_complete（coordinator.rs:270）
  → FileEventMergeSink.append_events（merge_sink.rs:137，唯一业务事件写主账本点）
  → dispatch.rs:1165-1177 重读主账本发布到 bus（政策校验在重读侧 event_origin.rs:341）
```

**恢复链：**
```
loop 启动 → build_supervisor_bridge → recover_active_waves_at_startup（recover.rs:65；
调用点 inner.rs:1078 与 :1171 两处）→ 成功后 recover_pending_projections（task 投影回放）
```

**hard gate 链：**
```
activation 结束 → prepare_normal_merge（activation_outcome_close.rs:61-136，采集 outcome 事实 :170-188）
→ merge_hat_channel_at_path（hat_channel.rs:87 → impl :104-264；空 channel :161-177 quarantine :287-313）
→ gate 判定（inner.rs:4712-4732）→ hard gate 计数（event_loop/terminal_routing.rs:246）→ 3 次 fail-close（:322）
watchdog 超时：inner.rs:3389-3437（记 trace 后继续常规处理，无事件则落入上一行 gate）
backend termination 早退：inner.rs:3486-3520（写 outcome 后终止，不到 gate）
```

**现有测试基座（E15）：** `crates/ralph-cli/tests/integration_wave_channel_convergence.rs`（真实子进程 10 场景矩阵，含 crash window s_08）；`crates/ralph-cli/src/loop_runner/tests/wave_supervisor/*` + `dispatcher_tests/mod.rs`（fake executor 直驱 `execute_wave_via_supervisor_with_executor`）；`crates/ralph-core/src/supervisor/recover.rs:209-436` 内嵌测试（InMemorySupervisorStore 闭环）；`crates/ralph-cli/src/loop_runner/tests/legacy/activation_outcome.rs`（T5/T6 merge 失败/成功、只读目标 :699-721）；`crates/ralph-core/tests/hat_activation_outcome_contract.rs`。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `wave/worker.rs:642`、`wave/io.rs:335` | worker 退出读回事件后 channel 文件被无条件删除；事件唯一副本进 dispatcher 内存 | U1 必须 persist-before-delete；U6 现场保留改此处 | 高 |
| E2 | `dispatcher/fan_in.rs:97`、`merge_sink.rs:137-190`、`dispatch.rs:1165-1177` | fan-in 是唯一业务事件写主账本点；sink 原子批写、失败 → MergeFailed 下 tick 重试（KTD-7）；写后重读发布 | 恢复投递复用同一 sink/重读模式，不发明第二写路径 | 高 |
| E3 | `migrations/v1.sql:56-64`、`dispatch.rs:3041` → `rusqlite.rs:903` | worker_results 只存 hash+event_count；terminal evidence 只存指纹 | 事件本体无持久化 = 内存空洞根因；U1 新增表 | 高 |
| E4 | `dispatcher/salvage.rs:99-147,165-257` | salvage 从内存 `completed.results[].events` 重建 canonical JSONL 行（topic/payload/hat/source/wave_id/wave_index，去 ts 保指纹稳定）；`commit_salvage_batch` 先 append 后 stamp，空批也 stamp | U1 持久化格式 = 可重建 canonical 行的字段集；U2/U3 恢复复用该 seam | 高（Planner 亲读全文） |
| E5 | `supervisor/recover.rs:65-139,149-207` | 恢复只标超时 Failed，不注入 `exec.wave.failed`、不回捞；`restore_unmerged_completed_slot` 死代码 | U2/U3 在恢复路径新增投递与注入；死代码由 U2 接线或删除（实现期定，非决策项） | 高（Planner 亲读全文） |
| E6 | `inner.rs:1078`、`:1171` | 两处启动恢复调用点；recover 成功后 `recover_pending_projections` 模式（传 store + 路径，幂等回放） | U2 新恢复步骤照此模式接线，两处调用点都接 | 高（Planner 亲读） |
| E7 | `merge_sink.rs:24-33,104-117` | sink 契约：成功 = 落盘且过 reader cursor；dispatcher fan-in 侧负责去重（`already_present_count`） | 重复追加窗口与现状一致（见 D3 残余风险），不新增去重机制 | 高（Planner 亲读） |
| E8 | `inner.rs:4712-4732,73-81`、`activation_outcome_close.rs:61-136,170-188`、`activation_outcome.rs:47-67,399` | gate 判定不消费已采集的 outcome 事实；outcome 状态枚举已有 merged/empty/missing/unreadable/merge_failed/interrupted | U4 分类所需事实同生命周期内可得；watchdog 路径 inner.rs:3389-3437 是 BackendDied 误入 gate 的现码 | 高 |
| E9 | `hard_gate.rs:53-189`、`event_loop/terminal_routing.rs:246,322`、`event_loop/event_processing.rs:573,631-659`、`event_loop/rejection.rs:44-65`、`diagnosis/reporter.rs:740-746` | hard gate 计数与 fail-close 路径；missing-terminal 特例恢复；诊断文案与 RejectionStage 位置 | U4 的分类结果接入点清单；`terminal_routing` / `event_processing` / `rejection` 均在 `ralph-core` 的 `event_loop/` | 中高 |
| E10 | `supervisor/mod.rs:1437`、`rusqlite.rs:1564`、`memory.rs:2142`、`pty_executor.rs:363,1813`、`commands/diagnose.rs:1059` | record_slot_pid 三处实现齐、零生产调用；pid 在 PTY 层可得；diagnose 自证近似 | U5 接线即可，无新机制 | 高 |
| E11 | `rusqlite.rs:586`（INSERT 无 pid）、`:694-705,759-787,857` | dispatch_records 行创建即 pid=NULL；outcome 后续回填 | U5 在 spawn 后回填 pid | 高 |
| E12 | `hat_channel.rs:287-313,466-492` | isolated channel quarantine 到 `.ralph/diagnostics/failed-activations/` + routing-fallback 诊断文件既有模式 | U6 复用同一目录与命名模式 | 高 |
| E13 | `diagnostics/mod.rs:927,1648-1664`、`activation_outcome.rs:406-421`、`tests/hat_activation_outcome_contract.rs`、`config/telemetry.rs:109,155,253` | logger 在 full 或 runtime-diagnosis 会话时创建；`log_runtime_trace` 在 logger None 时 return；outcome 入口在 `session_dir()` None 时 return。默认 `write_artifacts=false`。Minimal session 已有 sidecar。08-29 DEV-8 是 cap manifest 空，不是 DiagnosticLevel 早退 | U7 第一步必须 Characterization：Minimal+write_artifacts 下 outcome 是否已落盘；禁止把默认 full diagnostics 打开 | 高（2026-09-01 重核） |
| E14 | `supervisor/migrations/`（v1…v11+，v8.sql:25 为 salvage_write_count）、`memory.rs` | migration 文件按版本号递增；store 双实现（rusqlite + InMemory）+ trait 在 mod.rs | 新表走下一版本 migration，双实现同步 | 高 |
| E15 | 见 §2.1 测试基段落 | crash window、fake executor、InMemory store、merge 失败 fixture 全部已有 | 本计划零新测试基建 | 高 |
| E16 | `docs/report/2026-08-29-…evidence-gates-plan-diagnosis.md`、`2026-08-26-…`、`2026-09-01-parallel-forge-p0-gaps-adversarial-review.md` | 08-29：3 slot succeeded 但事件零落账、pid 全 NULL、纠偏账本全空；08-26：verifier 成功但 merge 失败 → fail-close | 验收场景的真实事故原型 | 高 |
| E17 | `cli/emit_path.rs:414-456`、`commands/emit/command_impl.rs:572,1924-1944`、`event_origin.rs:341` | worker 写盘零门禁，政策校验在主账本重读侧；协调 topic 白名单在 `event_origin.rs:133-140`（含 `exec.wave.failed`） | 恢复投递的行必须经重读发布；U3 注入 topic 已在白名单 | 高 |
| E18 | `git status` + `git log 59c0dcf0..21dfbab` | 工作区干净。证据门禁已合入 presets/scenarios/runtime-contract；`59c0dcf0..HEAD` 对本计划目标 runtime 文件几乎零 diff | 本计划仍零改 preset/scenario；不再存在「对方脏文件」并发约束 | 高（2026-09-01 亲验） |

### 2.3 受影响范围（均经证据确认）

* **生产模块：** `crates/ralph-core/src/supervisor/{mod.rs,rusqlite.rs,memory.rs,recover.rs,migrations/<next>.sql}`；`crates/ralph-cli/src/loop_runner/wave/{worker.rs,dispatcher/dispatch.rs,dispatcher/fan_in.rs}`；`crates/ralph-cli/src/loop_runner/{inner.rs,activation_outcome_close.rs}`；`crates/ralph-core/src/diagnostics/mod.rs`；`crates/ralph-core/src/diagnosis/reporter.rs`；`crates/ralph-cli/src/commands/diagnose.rs`；`crates/ralph-adapters/src/pty_executor.rs`（仅暴露 pid，不改执行语义）。
* **测试模块：** §2.1 测试基座 + 新增 `crates/ralph-cli/tests/integration_wave_recovery_redelivery.rs`（计划新增）。
* **配置 / CLI / API：** 无新增配置项；`ralph diagnose` 输出多一个真实 pid 字段来源（U5）。
* **不受影响（明确排除）：** `presets/**`、`event_policy/**`、`emit_path.rs`、`scenarios.rs` 及 scenario yml、merge sink 契约、worker 侧 emit 路由。

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | slot 事件本体持久化到哪里 | (a) supervisor store 新表；(b) worker 退出即直接 append 主账本；(c) 不删 channel 文件靠文件恢复 | **(a)** | E3（现存储只有 hash/指纹）；E14（migration 机制成熟） | (b) 破坏「wave settle 前业务事件不上 bus」语义，且绕过主账本重读侧政策校验（E17）；(c) channel 路径按 iteration 生成无恢复消费方（paths.rs:74-75），且文件无事务语义 | 0.90 |
| D2 | 持久化时机与健康路径是否改 fan-in | (a) worker 退出后 write-through 落 store、健康 fan-in 仍读内存；(b) fan-in 改从 store 读 | **(a)** | E1/E2/E8；fan-in 健康路径不动 = 零回归面 | (b) 改动健康路径数据源，回归面大且无收益（内存已在手） | 0.85 |
| D3 | 崩溃后补偿投递走哪条路径 | (a) 复用 salvage seam（`merge_completed_*_slots_to_main` → `commit_salvage_batch`）由恢复侧重建 Event 输入；(b) 新写一条恢复专用 merge 路径 | **(a)** | E4（salvage seam 先 append 后 stamp、指纹幂等、空批合法）；E6（恢复接线模式已有 `recover_pending_projections` 先例） | (b) 发明第二写路径违反「同一账本单一写入语义」，且要重建 stamp/指纹逻辑 | 0.85 |
| D4 | 恢复把超时 wave 标 Failed 后是否注入 `exec.wave.failed` | (a) 注入 system_injected `exec.wave.failed`（先 salvage 已完成 slot）；(b) 维持只标 phase | **(a)** | E5（现状不注入 → 下游 failure-handler 永不激活 → stall）；E4（KTD2：先 salvage 再 fail）；salvage.rs:311 `build_wave_failed_slots_json` 有现成 payload 构造 | (b) 即 08-29 事故的「收敛了但无人知道」状态 | 0.85 |
| D5 | silent activation 如何分类处置 | (a) 纯函数分类器 + 分类处置：MergeFailed→立即重试一次 merge；BackendDied→不计 publish hard gate、走既有 backend 失败路径；NeverEmitted→现状；(b) 只改诊断文案不改处置 | **(a)** | E8（事实已采集、判定点同生命周期）；E16（08-26/08-10 两次误定罪实跑事故） | (b) 不解决 fail-close 误杀，只改文案 | 0.85 |
| D6 | pid 接线范围 | (a) 只接线 `record_slot_pid` + diagnose 展示；(b) 附带主动死进程 reaper | **(a)** | E10/E11（API 与 pid 来源均已存在，纯接线） | (b) 需要周期性 tick 锚点（现状没有），属新机制，独立立项 | 0.85 |
| D7 | worker channel 失败现场 | (a) 非干净结局移动 quarantine；(b) 保留原路径不删；(c) 维持删除 | **(a)** | E12（hat channel 侧既有 quarantine 模式）；E1 | (b) 残留文件干扰后续运行与磁盘；(c) 即现状（08-29 无现场可查） | 0.90 |
| D8 | Minimal 模式 outcome 行 | (a) `hat_activation_outcome` 行豁免 Minimal 早退；(b) 提升默认诊断级别 | **(a)** | E13；outcome 行每 activation 一行，体量可忽略 | (b) 改全局默认影响所有 loop 的 IO 量级 | 0.85 |
| D9 | 与 2026-08-27-1430 证据门禁的边界 | (a) 本计划零 preset/scenario 文件改动，验收走新增 ralph-cli 集成测试；(b) 等待对方合入后再启动 | **(a)**（对方已合入，边界仍成立） | E18（合入后 runtime 目标文件零实质 diff） | (b) 已无必要等待；仍禁止回改已落地 preset 契约 | 0.95 |

**D3 残余风险（已接受）：** 「append 主账本成功、stamp 前崩溃」→ 恢复重投产生重复行。该窗口与现有 salvage re-tick 行为完全一致（`commit_salvage_batch` 注释自承，salvage.rs:220-247），下游重读侧对重复 `exec.unit.done` 的容忍度与现状相同；本计划不引入新去重机制（E7）。

**D6 备选细节（不构成分支决策）：** pid 取自 `pty_executor.rs:363,1813` 的 PTY 子进程；若该处句柄类型不直接暴露 pid，则在 `run_wave_worker` spawn 点经 `std::process::Child::id()` 取。二者都在 U5 第一步验证，取可达者；两条路径产出相同语义（dispatch_records.pid = worker 进程 pid）。

---

## 4. BDD 行为规格

### Feature F1: slot 业务事件本体持久化（R1）

```gherkin
Feature: slot 业务事件在 fan-in 前持久化

  Background:
    Given supervisor 模式 loop 且 wave 已 dispatch

  Scenario S1.1: worker 退出后事件本体落入 supervisor store
    Given slot 0 的 worker 已写出 exec.unit.done 到 slot channel 并退出
    When dispatcher 读回 slot 事件并完成 slot 终态落账
    Then supervisor store 的 slot_event_payloads 表存在 (wave, slot 0) 行
    And 行内容可重建为与 channel 中相同的 Event 列表（topic/payload/source/wave 包络）
    And slot channel 文件在持久化成功后才被删除

  Scenario S1.2: 健康 fan-in 完成后清理持久化负载
    Given 所有 slot 已完成且事件已持久化
    When fan-in 合并成功且 wave 收敛
    Then 主 events 账本包含全部 slot 业务事件（与基线行为一致）
    And 该 wave 的 slot_event_payloads 行被删除

  Scenario S1.3: store 写入失败不阻断 fan-in
    Given slot 0 的 worker 已退出且事件读回成功
    When 持久化到 store 失败（注入 store 错误）
    Then dispatcher 记录 warn 与诊断
    And slot channel 文件保留不删
    And fan-in 仍按内存事件正常合并（健康路径不回退）
```

### Feature F2: 崩溃恢复补偿投递（R2）

```gherkin
Feature: loop 重启后补偿投递 fan-in 前丢失的 slot 事件

  Background:
    Given loop 在「slot 已完成、fan-in 未执行」窗口被杀

  Scenario S2.1: 全部 slot 完成的 wave 重启后补投并收敛
    Given wave w 的 2 个 slot 均 Completed 且 payload 已持久化
    And 主 events 账本中没有这些 slot 的业务事件
    When loop 重启并完成启动恢复
    Then 持久化的 slot 业务事件被追加到主 events 账本
    And wave 按既有收敛语义注入 exec.wave.complete
    And 重读发布使下游 hat 正常激活

  Scenario S2.2: 部分 slot 完成的 wave 重启后先 salvage 再判失败
    Given wave w 的 slot 0 Completed（payload 已持久化）、slot 1 未完成
    When loop 重启恢复且 wave 按既有规则判失败
    Then slot 0 的业务事件先被 salvage 追加到主账本
    And 然后注入 exec.wave.failed
    And 主账本中不出现 slot 1 的事件

  Scenario S2.3: 已完成投递的 wave 重启不重复投递
    Given wave w 的 delivery_state 已达 BusinessProjected 或更高
    When loop 重启恢复
    Then 不对 w 执行任何补偿投递（幂等跳过）

  Scenario S2.4: 旧版崩溃遗留（无持久化负载）不 panic
    Given wave w 有 Completed slot 但 slot_event_payloads 无对应行（升级前崩溃）
    When loop 重启恢复
    Then 恢复不 panic、不阻断 loop 启动
    And 该 wave 按现状语义处理（记 warn 诊断说明事件本体不可恢复）
```

### Feature F3: 恢复时超时 wave 注入收敛事件（R3）

```gherkin
Feature: 启动恢复对超时 wave 注入 exec.wave.failed

  Scenario S3.1: 超时 wave 恢复时 salvage + 注入 failed
    Given wave w 有 in-flight slot 且 started_at 距今超过 aggregate_timeout_secs
    When loop 重启恢复
    Then w 被标记 phase=Failed（现状保持）
    And w 的 Completed slot 已持久化事件先被 salvage 投递
    And 主 events 账本出现 system_injected 的 exec.wave.failed（含 wave_id 与 slot 明细）
    And 下游 forge-failure-handler 可据此激活

  Scenario S3.2: 未超时 wave 不受恢复影响
    Given wave w 有 in-flight slot 但未超 aggregate_timeout_secs
    When loop 重启恢复
    Then w 的 phase 不被改为 Failed（现状保持）
    And 不注入任何 wave 级事件
```

### Feature F4: silent activation 归因分类（R4）

```gherkin
Feature: isolated activation 无事件时按事实分类而非一律定罪

  Background:
    Given isolated 模式某 hat 的 activation 结束且 main/candidate 账本无新事件

  Scenario S4.1: merge 失败重试成功则正常推进
    Given channel 文件非空且首次 merge 因 IO 错误失败
    When runtime 执行一次 merge 重试并成功
    Then 事件落入 main/candidate 账本
    And 不触发 hard gate 计数
    And 诊断记录 reason=merge_failed_retried

  Scenario S4.2: merge 重试仍失败则正确归因
    Given channel 文件非空且 merge 重试仍失败
    When gate 判定执行
    Then 诊断与 runtime-trace 记录 reason=merge_failed（区别于 never_emitted）
    And channel 文件被 quarantine 保留现场
    And hard gate 计数 +1（确实无进展，但归因正确）

  Scenario S4.3: backend 早死不计入 publish-obligation hard gate
    Given backend_success=false 或 watchdog_timeout=true 且 channel 为空
    When gate 判定执行
    Then 不按 "hat has publish obligation but emitted no event" 计数
    And 走既有 backend 失败处理路径并记 reason=backend_died

  Scenario S4.4: 真空 activation 维持现状
    Given backend 正常退出、channel 为空、output 无 emit 痕迹
    When gate 判定执行
    Then 维持现状：计入 publish-obligation hard gate（reason=never_emitted）
```

### Feature F5: dispatch_records.pid 接线（R5）

```gherkin
Feature: slot spawn 后记录真实进程 pid

  Scenario S5.1: spawn 成功后 pid 落 store
    Given supervisor wave 正在 dispatch
    When slot worker 进程成功 spawn
    Then dispatch_records 对应行的 pid 被回填为该进程 pid（非 NULL）

  Scenario S5.2: diagnose 展示真实 pid
    Given dispatch_records.pid 已写入
    When 运行 ralph diagnose
    Then 输出中 slot 的 pid 来自 store 真实值而非近似估计
```

### Feature F6: worker channel 失败现场保留（R6）

```gherkin
Feature: 失败 slot 的 channel 文件移入 quarantine

  Scenario S6.1: 非干净结局保留现场
    Given slot worker 退出且结局为 empty/failed/timeout 之一
    When dispatcher 完成 slot 终态处理
    Then slot channel 文件被移动到 .ralph/diagnostics/failed-activations/（命名含 wave/slot/attempt）
    And 原路径不再存在该文件

  Scenario S6.2: 干净成功仍删除
    Given slot worker 干净成功且事件已持久化
    When dispatcher 完成 slot 终态处理
    Then slot channel 文件被删除（磁盘不累积）
```

### Feature F7: Minimal 会话下 outcome 行落盘（R7）

```gherkin
Feature: hat_activation_outcome 行在 Minimal runtime-diagnosis 会话下落盘

  Scenario S7.1: Minimal 会话 outcome 行落盘
    Given telemetry.runtime_diagnosis.enabled 与 write_artifacts 均为 true（Minimal session）
    When 一次 isolated activation 结束
    Then runtime-trace sidecar 中存在该 activation 的 hat_activation_outcome 行

  Scenario S7.2: Disabled / full 行为边界
    Given 诊断 collector 为 Disabled（默认）或 Full
    When 一次 isolated activation 结束
    Then Disabled：无 sidecar、不 panic；Full：行为与基线一致
```

---

## 5. 验收与测试策略

| Scenario | 验收条件（关键断言） | 测试入口 | 层级 | 风险补充测试 | 需要 E2E |
|---|---|---|---|---|---|
| S1.1 | store 行存在且可逐字段重建 Event；channel 删除发生在持久化之后（顺序由注入故障验证） | 新 store 单测 + dispatcher 测试（E15 fake executor 直驱） | 单元 + 集成 |  round-trip（序列化↔重建 byte 等价于 canonical 行） | 否 |
| S1.2 | 主账本事件与基线一致；wave 收敛后 slot_event_payloads 无该 wave 行 | 现有 `integration_wave_channel_convergence.rs` 全绿 + 新增清理断言 | 集成 | — | 否 |
| S1.3 | store 故障注入下 fan-in 成功、channel 文件保留、warn 落日志 | dispatcher 测试（fault injection store） | 集成 | Fault Injection（依据：store 是唯一新 IO 依赖） | 否 |
| S2.1 | 重启后主账本含 slot 事件且含 exec.wave.complete；hat 激活链可续 | 新增 `integration_wave_recovery_redelivery.rs` | 集成 | — | 否 |
| S2.2 | salvage 行先于 exec.wave.failed；无 slot 1 事件 | 同上 | 集成 | State-Machine（delivery_state 迁移断言） | 否 |
| S2.3 | delivery_state ≥ BusinessProjected 时零追加（账本 diff 为空） | 同上 + store 单测 | 单元 + 集成 | Idempotency（恢复函数连跑两次账本不变） | 否 |
| S2.4 | 无 payload 行时恢复返回正常、warn 诊断存在 | store/recover 单测 | 单元 | Characterization（固定升级前行为） | 否 |
| S3.1 | phase=Failed；salvage 行在前；exec.wave.failed 为 system_injected 且含 slot 明细 | 恢复集成测试（backdate_wave_for_test 已有，recover.rs:389 实证） | 集成 | — | 否 |
| S3.2 | 未超时 wave 相位与账本零变化 | recover 单测（recover.rs:397 既有模式扩展） | 单元 | — | 否 |
| S4.1 | 重试成功后账本有事件、hard gate 计数不变 | `loop_runner/tests/legacy/activation_outcome.rs` 扩展（只读目标 fixture :699-721 复用） | 单元 | — | 否 |
| S4.2 | reason=merge_failed；quarantine 文件存在；计数 +1 | 同上 | 单元 | — | 否 |
| S4.3 | backend_died 不触发 publish-obligation 计数；走 backend 失败路径 | hard gate 单测 + inner 判定测试 | 单元 | — | 否 |
| S4.4 | never_emitted 维持计数与文案 | 现有 replay 测试（`ralph-core` `replay_light_integration.rs:1449`）保持绿 | 回归 | — | 否 |
| S5.1 | dispatch_records.pid = 实际 spawn pid | dispatcher 测试断言 store；pty 层单测 | 单元 + 集成 | — | 否 |
| S5.2 | diagnose 输出含真实 pid | diagnose 命令测试（commands/diagnose.rs 既有测试模式） | 单元 | — | 否 |
| S6.1 | quarantine 文件存在且命名含 wave/slot/attempt；原路径删除 | dispatcher/worker 测试 | 单元 | — | 否 |
| S6.2 | 成功路径文件删除 | 同上 | 单元 | — | 否 |
| S7.1 | Minimal 下 outcome 行存在于 sidecar | hat_activation_outcome_contract.rs 新增 Minimal 用例 | 单元 | — | 否 |
| S7.2 | Disabled 无 sidecar/不 panic；Full 与基线一致 | hat_activation_outcome_contract 边界用例 | 单元 | — | 否 |

测试层级选择理由：所有行为均在单进程内可观察（store 行 / 账本行 / 文件存在性），无需跨进程 E2E；crash 窗口用「直接构造崩溃后状态 + 调恢复入口」模拟（与 recover.rs:209-436 内嵌测试同法），不起真实子进程杀进程——该取舍与 s_08 crash window 测试同法（E15）。

---

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试（集成/契约） | 单元测试 | E2E | Evidence |
|---|---|---|---|---|---|---|
| R1 | slot 事件本体持久化 + persist-before-delete | S1.1/S1.2/S1.3 | integration_wave_channel_convergence 扩展断言 | store round-trip、dispatcher 持久化调用、故障注入 | 否 | E1/E3/E14 |
| R2 | 崩溃恢复补偿投递 | S2.1–S2.4 | integration_wave_recovery_redelivery（新增文件） | recover/redelivery 纯函数、幂等连跑 | 否 | E4/E5/E6/E7 |
| R3 | 超时 wave 恢复注入 exec.wave.failed | S3.1/S3.2 | 恢复集成测试 | recover 单测扩展 | 否 | E5/E4 |
| R4 | silent activation 归因分类 | S4.1–S4.4 | activation_outcome 既有集成模式扩展 | classify_silent_activation 纯函数、gate 判定 | 否 | E8/E9/E16 |
| R5 | pid 接线与 diagnose 展示 | S5.1/S5.2 | dispatcher spawn 断言 | store record_slot_pid round-trip、diagnose 输出 | 否 | E10/E11 |
| R6 | channel 失败现场 quarantine | S6.1/S6.2 | — | worker/dispatcher 文件断言 | 否 | E1/E12 |
| R7 | Minimal 会话 outcome 行落盘 | S7.1/S7.2 | hat_activation_outcome_contract 扩展 | collector 模式判定（先 Characterization） | 否 | E13 |

---

## 7. 严格串行开发单元

执行顺序：U1 → U2 → U3 → U4 → U5 → U6 → U7。禁止并行/交替。

---

### Unit 1：slot 业务事件本体 write-through 持久化（persist-before-delete）

**1. Unit 目标：** worker 退出、slot 事件读回内存后、fan-in 之前，事件本体持久化到 supervisor store 新表 `slot_event_payloads`；channel 文件删除推迟到持久化成功之后；健康 fan-in 行为完全不变。覆盖 S1.1/S1.2/S1.3。

**2. 对应需求与 Scenario：** R1；S1.1/S1.2/S1.3；D1/D2；E1/E2/E3/E14。

**3. 外部可观察结果：** wave 运行后 supervisor.db 的 `slot_event_payloads` 表在 fan-in 前含 slot 事件行、fan-in 收敛后被清理；主 events 账本内容与基线逐行一致（健康路径零变化）。

**4. 当前行为基线：** 事件仅存 dispatcher 内存，channel 删除于 `worker.rs:642`，store 无事件本体（E1/E3）。现有覆盖：`integration_wave_channel_convergence.rs` 全绿即健康基线；无持久化断言（缺口即本 Unit）。

**5. 输入与输出：** 输入 = `read_worker_events` 读回的 Event 列表 + (wave_id, slot_index, attempt)。输出 = store 行。错误：store 写失败 → warn + 保留 channel + 不阻断（S1.3）。副作用：新表 I/O。不变量：主账本行与基线一致；健康 fan-in 数据源仍是内存。

**6. 修改位置：**
- `crates/ralph-core/src/supervisor/migrations/<next>.sql`（计划新增；进入 Unit 时 `ls crates/ralph-core/src/supervisor/migrations/` 取最大版本号 +1）— 新表 `slot_event_payloads(wave_id TEXT, slot_index INTEGER, attempt INTEGER, event_seq INTEGER, topic TEXT, payload TEXT, source TEXT, wave_index INTEGER, wave_total INTEGER, system_injected INTEGER, PRIMARY KEY(wave_id, slot_index, attempt, event_seq))`（每事件一行，event_seq 保序；字段集 = 重建 Event 所需的 canonical 字段，对应 E4 的 salvage 渲染字段 + wave_total/system_injected 以匹配 merge_sink.rs:165-172 的行形）。
- `crates/ralph-core/src/supervisor/migrations.rs`（修改）— 注册新 migration（照 v8 既有注册模式）。
- `crates/ralph-core/src/supervisor/mod.rs`（修改）— trait `SupervisorStore` 新增 3 方法：`record_slot_event_payloads` / `load_slot_event_payloads(wave_id) -> 按 slot 分组的 Event 列表` / `delete_slot_event_payloads(wave_id)`。
- `crates/ralph-core/src/supervisor/rusqlite.rs`（修改）— 上述 3 方法的 rusqlite 实现。
- `crates/ralph-core/src/supervisor/memory.rs`（修改）— InMemory 实现（E14 双实现模式）。
- `crates/ralph-cli/src/loop_runner/wave/dispatcher/dispatch.rs:2995-3086`（修改）— slot 终态落账段：读回事件后先调 `record_slot_event_payloads`，成功后才允许删除 channel 文件。
- `crates/ralph-cli/src/loop_runner/wave/worker.rs:642`（修改）— 无条件删除改为「持久化确认后删除」；删除动作移到 dispatcher 侧（worker 返回事件 + channel 路径，由 dispatch.rs 统一决策删除时机）。
- `crates/ralph-cli/src/loop_runner/wave/dispatcher/fan_in.rs`（修改，仅清理钩子）— wave 收敛（`merge_and_complete` 成功 / salvage committed）后调 `delete_slot_event_payloads`。
- 明确不修改：`merge_sink.rs`、coordinator、emit 路由、`read_worker_events` 解析逻辑。

**7. 可依赖能力：** store trait/双实现/migration 机制（E14）；Event builder（`Event::new/with_source/with_wave`，merge_sink.rs:205-207 实证）；fake executor 测试基座（E15）。

**8. 禁止依赖的未来能力：** 恢复投递（U2）、超时注入（U3）——本 Unit 不消费持久化数据，只写和清理。

**9. 验收测试：**
- 集成：`integration_wave_channel_convergence.rs` 新增场景行「全成功后 `slot_event_payloads` 无该 wave 行；fan-in 前（测试内 hook 点）行存在且内容可重建」。运行：`cargo nextest run -p ralph-cli --test integration_wave_channel_convergence`。
- 断言：主账本事件与基线一致（回归）；store 行内容逐字段等于 channel 事件。
- 运行命令见 §9。

**10. Acceptance Red：** 先写 S1.1 集成断言（fan-in 前 store 行存在）。Red 表现：查询返回 0 行 / 编译错误 `no such table: slot_event_payloads` 或方法不存在。无效 Red：fixture 路径错、nextest 过滤词不匹配。

**11. 单元测试拆分：**
- T1.1 `slot_event_payloads_round_trip`（rusqlite + memory 各一）：写入 2 slot × 2 事件 → load 逐字段相等（含 event_seq 顺序）。
- T1.2 `delete_slot_event_payloads_only_target_wave`：两 wave 数据，删 w1 不影响 w2。
- T1.3 `record_slot_event_payloads_overwrites_same_attempt`：同 (wave, slot, attempt) 重复写幂等（先删后插或 UPSERT，实现期二选一并在测试钉住）。
- T1.4 dispatcher 测试（wave_supervisor 基座）：fake executor 产出事件 → 断言 dispatch 过程中 store 被写入且 channel 文件在写入后删除。
- T1.5 store 故障注入（stub store 返回 Err）：dispatcher warn + fan-in 仍成功 + channel 文件保留。
- 不允许 mock 的行为：Event 序列化/重建必须走真实 serde。

**12. Red → Green → Refactor 顺序：** T1.1 Red（表/方法不存在）→ migration + trait + 双实现 → Green → T1.2 Red → delete 实现 → Green → T1.3 Red → 幂等语义 → Green → T1.4 Red（dispatcher 未调用新 API）→ dispatch.rs/worker.rs 接线 → Green → T1.5 Red → 错误降级路径 → Green → fan-in 清理钩子（S1.2 断言 Red → Green）→ Refactor（仅提取重复序列化代码）→ 全量 Unit 测试。

**13. 最小实现范围：** 新表 + 3 个 store 方法 ×2 实现 + dispatcher 写/删时机调整 + fan-in 清理钩子。必须保持的不变量：健康路径主账本字节级不变。明确不实现：恢复消费（U2）、任何 gate 改动。

**14. 集成验证：** `integration_wave_channel_convergence.rs` 全部既有场景 + 新断言；`cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`。预期：全绿。真实验证：事件从 fake executor → channel → store → fan-in 全链路真实文件 I/O。

**15. 风险驱动测试：** Fault Injection（T1.5，依据：store 是新 IO 依赖，08-29 证明 store 失败曾静默）；round-trip（T1.1，依据：序列化是重建正确性根基）。

**16. 回归范围：** `cargo nextest run -p ralph-core -- supervisor`（store schema/双实现）；`cargo nextest run -p ralph-cli --bin ralph -- wave`（dispatcher/worker/fan-in 全部相邻测试）；`cargo nextest run -p ralph-cli --test integration_wave_channel_convergence`；`cargo nextest run -p ralph-cli --test u7_real_ralph_emit`。理由：改动点在 wave 投递链主干，所有 wave 消费者皆相邻。

**17. 预期文件变更：**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `supervisor/migrations/<next>.sql` | 新增 | 新表 | E14 |
| `supervisor/migrations.rs` | 修改 | 注册 migration | E14 |
| `supervisor/mod.rs` | 修改 | trait 新方法 | E14 |
| `supervisor/rusqlite.rs` / `memory.rs` | 修改 | 双实现 | E14 |
| `dispatcher/dispatch.rs` | 修改 | persist-before-delete 接线 | E1/E3 |
| `wave/worker.rs` | 修改 | 删除时机移交 dispatcher | E1 |
| `dispatcher/fan_in.rs` | 修改 | 收敛后清理钩子 | D2 |
| 上述各文件 `#[cfg(test)]` 或 wave_supervisor 测试目录 | 新增测试 | T1.1–T1.5 | E15 |

**18. 完成标准：** S1.1/S1.2/S1.3 验收全绿；回归范围全绿；`cargo clippy` 与 `cargo build` 通过；无跳过/削弱测试；健康路径主账本 diff 为空（对照基线场景输出）；Evidence 表行号复核完成。

**19. 停止条件：** migration 机制与 E14 描述不符（如无版本注册点）；`read_worker_events` 返回类型不含重建 Event 所需字段（如丢 wave_total）；worker.rs 与 dispatch.rs 的删除/读回归属与 E1 不符；store trait 改动牵连出未预见的第四处实现。

**20. 风险与注意事项：** ① 事件量大时单行 TEXT 体积——wave slot 事件量级为个位数（E16 实跑样本），无分页需求；若 load 时发现 >1MB 行，停止并回报。② channel 删除移交 dispatcher 后，worker 崩溃路径的文件残留由 U6 收尾，本 Unit 只保证「持久化成功才删」。

---

### Unit 2：崩溃恢复补偿投递（fan-in 前窗口）

**1. Unit 目标：** loop 启动恢复时，对「有 Completed slot 且 delivery_state 未达 BusinessProjected」的 wave，从 `slot_event_payloads` 重建事件并经既有 salvage seam 补投主账本；幂等。覆盖 S2.1–S2.4。

**2. 对应需求与 Scenario：** R2；S2.1/S2.2/S2.3/S2.4；D1/D3；E4/E5/E6/E7。

**3. 外部可观察结果：** 杀掉 loop（窗口期）→ 重启 → 主 events 账本出现已完成 slot 的业务事件，wave 随后按既有语义收敛；无持久化负载的旧遗留 wave 不 panic。

**4. 当前行为基线：** 恢复只标超时 phase、不回捞（E5）；重启后已完成 slot 事件永久丢失（E16 的 08-29）。Characterization：S2.4 测试先固定「无负载时恢复不投递不 panic」这一现状语义。

**5. 输入与输出：** 输入 = store 中 active wave + 持久化负载 + main events 路径。输出 = 追加到主账本的 canonical 行 + store 投影 stamp（`commit_salvage_batch` 语义）。错误：append 失败 → 不 stamp、warn、下轮恢复重试（E4 seam 自带语义）。不变量：delivery_state ≥ BusinessProjected 的 wave 零追加（S2.3）。

**6. 修改位置：**
- `crates/ralph-cli/src/loop_runner/wave/` 新增模块 `recovery_redelivery.rs`（计划新增；职责：从 store load 负载 → 重建 `CompletedWave` 形状的输入 → 调 `salvage::merge_completed_exec_fix_slots_to_main` / `merge_completed_review_slots_to_main`）。
- `crates/ralph-cli/src/loop_runner/inner.rs:1078 与 :1171`（修改，两处调用点）— `recover_active_waves_at_startup` 成功后、`recover_pending_projections` 之前，调用新模块（E6 模式）。
- `crates/ralph-core/src/supervisor/recover.rs`（修改）— `RecoveryReport` 增加「待补偿 wave」清单字段（含 wave kind，供调用侧选 exec/review seam）；`restore_unmerged_completed_slot`（recover.rs:167 死代码）接线或删除——实现期二选一：若新模块覆盖了它的语义则删除该函数并在提交信息说明。
- `crates/ralph-cli/src/loop_runner/wave/dispatcher/salvage.rs`（不改逻辑；如函数可见性不足则调整 `pub(crate)` 暴露，允许的最小改动）。
- 明确不修改：coordinator、merge_sink、fan-in 健康路径。

**7. 可依赖能力：** U1 的 `load_slot_event_payloads`；salvage seam 双函数 + `commit_salvage_batch`（E4）；`SupervisorBridge`（build_supervisor_bridge 两处已有，E6）；恢复幂等先例 `recover_pending_projections`（E6）。

**8. 禁止依赖的未来能力：** 超时注入（U3，本 Unit 只处理「有负载可投递」）；pid（U5）。

**9. 验收测试：** 新增 `crates/ralph-cli/tests/integration_wave_recovery_redelivery.rs`（计划新增，HARD RULE 5：用 `common::ralph_bin()` 或纯库级驱动，不裸 spawn）。前置：构造 store（Completed slot + 负载行）+ 空主账本。动作：调恢复入口。断言：S2.1 账本含事件；S2.3 二次调用账本 diff 为空（幂等）；S2.4 无负载时 warn 且不 panic。运行：`cargo nextest run -p ralph-cli --test integration_wave_recovery_redelivery`。

**10. Acceptance Red：** S2.1 集成测试先跑——恢复后账本不含 slot 事件（机制不存在）。无效 Red：store 构造错（负载行未写入就断言）。

**11. 单元测试拆分：**
- T2.1 `redelivery_builds_completed_wave_from_store`：负载行 → seam 输入结构，slot 分组与 failures 过滤正确。
- T2.2 `redelivery_skips_business_projected_wave`（S2.3）。
- T2.3 `redelivery_empty_payload_store_is_noop_with_warn`（S2.4）。
- T2.4 `redelivery_idempotent_double_run`：连跑两次账本内容一致（配合 seam 指纹幂等，E4）。
- T2.5 review 类 wave 选 review seam（topic 过滤 `review.unit.done`，E4 salvage.rs:29）。
- 不允许 mock：seam 调用必须真实走 `commit_salvage_batch` 写临时账本文件。

**12. Red → Green → Refactor 顺序：** T2.3 Red（函数不存在）→ 空载骨架 → Green → T2.1 Red → 重建逻辑 → Green → T2.2 Red → delivery_state 门槛 → Green → T2.5 Red → seam 选择 → Green → T2.4 Red → 幂等修正 → Green → inner.rs 两处接线（集成 Red：S2.1 端到端 → Green）→ Refactor。

**13. 最小实现范围：** 重建 + seam 调用 + 两处接线 + RecoveryReport 字段。必须处理：store load 失败（warn + 跳过该 wave 不阻断启动）。明确不实现：超时注入（U3）、in-loop 周期恢复。

**14. 集成验证：** 新集成测试文件全绿；`cargo nextest run -p ralph-core -- recover`；`cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`。真实验证：恢复写真实临时账本文件并经 seam stamp。

**15. 风险驱动测试：** Idempotency（T2.4，依据：崩溃可发生在 stamp 前，D3 残余风险窗口）；Characterization（S2.4，依据：升级前遗留状态必须安全）。

**16. 回归范围：** U1 全部测试；`cargo nextest run -p ralph-core -- supervisor`；`integration_wave_channel_convergence`（crash window s_08 必须保持绿——恢复语义变更的高危相邻）；`cargo nextest run -p ralph-cli --bin ralph -- task_projection`（恢复段相邻，E6）。

**17. 预期文件变更：**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `loop_runner/wave/recovery_redelivery.rs` | 新增生产文件 | 补偿投递 | D3/E4 |
| `inner.rs` | 修改 | 两处接线 | E6 |
| `supervisor/recover.rs` | 修改 | report 字段 + 死代码处置 | E5 |
| `dispatcher/salvage.rs` | 修改（仅可见性，如需要） | seam 复用 | E4 |
| `crates/ralph-cli/tests/integration_wave_recovery_redelivery.rs` | 新增测试 | S2.1–S2.4 | E15 |

**18. 完成标准：** S2.1–S2.4 全绿；回归全绿；clippy/build 通过；恢复两次连跑账本 diff 为空；Evidence 行号复核完成。

**19. 停止条件：** `CompletedWave` 结构无法从负载重建（字段缺 wave_total 等 → 回 U1 补字段，U1 已含）；salvage seam 签名与 E4 不符；两处 inner.rs 调用点上下文无法同时拿到 store 与主账本路径。

**20. 风险与注意事项：** ① 重复行窗口（D3 已接受）——检测：S2.3/T2.4 幂等断言。② 恢复在主 loop 接受事件前执行（inner.rs 注释自证顺序），不得把投递挪到 loop 运行中。③ wave kind 判别错误会选错 seam——T2.5 钉住。

---

### Unit 3：恢复时超时 wave 注入 exec.wave.failed（先 salvage 后宣告）

**1. Unit 目标：** 启动恢复将 wave 标记超时 Failed 时，先 salvage 其 Completed slot 的持久化事件，再向主账本注入 system_injected `exec.wave.failed`（含 slot 明细）。覆盖 S3.1/S3.2。

**2. 对应需求与 Scenario：** R3；S3.1/S3.2；D4；E5/E4/E16。

**3. 外部可观察结果：** 超时遗留 wave 重启后，主账本出现该 wave 的 salvage 行 + `exec.wave.failed`；parallel-forge 拓扑中 forge-failure-handler 可激活（不再 stall）。

**4. 当前行为基线：** 只 `set_wave_phase(Failed)`，无事件注入（recover.rs:124-131，E5）。Characterization：S3.2（未超时不动）与「无负载 wave 超时」的现状相位行为先钉住。

**5. 输入与输出：** 输入 = 超时判定结果 + store 负载 + 主账本路径。输出 = salvage 行（可空）+ 一行 system_injected `exec.wave.failed`。错误：注入失败 → warn + phase 已 Failed 保留（不再重试注入，诊断可查）。不变量：salvage 严格先于 failed 注入（KTD2，E4）；未超时 wave 零影响（S3.2）。

**6. 修改位置：**
- `crates/ralph-core/src/supervisor/recover.rs:124-131`（修改）— 超时分支除标 phase 外，把 wave 加入 `RecoveryReport` 新字段 `timed_out_pending_injection`（recover.rs 保持纯 store 语义，不写账本——账本写在调用侧）。
- `crates/ralph-cli/src/loop_runner/wave/recovery_redelivery.rs`（修改，U2 新增文件扩展）— 对 `timed_out_pending_injection` 的 wave：salvage 投递 + 经 `FileEventMergeSink`（merge_sink.rs:119）追加 system_injected `exec.wave.failed`（payload 用 `salvage::build_wave_failed_slots_json`，salvage.rs:311）。
- `crates/ralph-cli/src/loop_runner/inner.rs`（修改）— 随 U2 接线点自然串联（同一块内先 U2 投递后 U3 注入）。
- 明确不修改：`evaluate_phase`（phase.rs:115 纯函数不动）、fan-in 失败路径（`fan_in.rs` 的 `emit_injected_failed_coord` / dispatch JoinSet 循环 `dispatch.rs:3238` 起）既有 in-loop 超时注入不动——那是 in-loop 路径。

**7. 可依赖能力：** U1 负载、U2 投递模块与 seam；`Event` system_injected 行形（merge_sink.rs:170-172）；`backdate_wave_for_test`（recover.rs:389 实证可构造超时）。

**8. 禁止依赖的未来能力：** 无（本 Unit 是恢复链终点）。不得实现 in-loop 看门狗。

**9. 验收测试：** 扩展 `integration_wave_recovery_redelivery.rs`：构造超时 wave（backdate）+ Completed slot 负载 → 恢复 → 断言账本行序：salvage 行在前、`exec.wave.failed` 在后且 `system_injected=true`、`payload.wave_id` 正确、含 slot 明细。S3.2：未超时 wave 账本零追加。运行命令同 U2。

**10. Acceptance Red：** 断言 `exec.wave.failed` 行存在 → Red：账本只有 salvage 行或全空（注入逻辑不存在）。无效 Red：backdate 未生效（wave 未判超时——先查 `aggregate_timeout_secs` 传参）。

**11. 单元测试拆分：**
- T3.1 `timeout_wave_reported_for_injection`：recover 报告含超时 wave（纯 store）。
- T3.2 `failed_injection_payload_contains_slot_details`：payload 构造正确（复用 build_wave_failed_slots_json）。
- T3.3 `injected_failed_event_is_system_injected`：行形含 system_injected/wave 包络。
- T3.4 `non_timeout_wave_zero_mutation`（S3.2）。
- 不允许 mock：账本写走真实 FileEventMergeSink 临时文件。

**12. Red → Green → Refactor 顺序：** T3.1 Red → report 字段 → Green → T3.2/T3.3 Red → 注入实现 → Green → T3.4 Red（保护性）→ Green → inner.rs 串联（集成 Red → Green）→ Refactor。

**13. 最小实现范围：** report 字段 + 注入函数 + 串联。明确不实现：失败重试注入（一次性 best-effort + warn，与 recover 现有容错同级）；对 in-loop 超时的任何改动。

**14. 集成验证：** U2 集成文件全绿（含 S3.1/S3.2）；`cargo nextest run -p ralph-core -- recover`。

**15. 风险驱动测试：** State-Machine（断言 phase=Failed 且 delivery_state 达 SalvageCommitted 序，依据：KTD2 顺序违规即 silent partial complete）。

**16. 回归范围：** U1/U2 全部；`cargo nextest run -p ralph-core -- supervisor`；`integration_wave_channel_convergence`；`cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`。

**17. 预期文件变更：**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `supervisor/recover.rs` | 修改 | report 新字段 | E5 |
| `loop_runner/wave/recovery_redelivery.rs` | 修改 | 注入逻辑 | D4 |
| `inner.rs` | 修改 | 串联 | E6 |
| `tests/integration_wave_recovery_redelivery.rs` | 修改（新增用例） | S3.1/S3.2 | — |

**18. 完成标准：** S3.1/S3.2 全绿；回归全绿；clippy/build 通过；行序断言（salvage → failed）在测试中显式存在。

**19. 停止条件：** payload 构造函数签名与 E4 不符。`exec.wave.failed` 已在 `SUPERVISOR_COORDINATION_TOPICS`（E17/A4）——若执行期发现该常量被删或注入被拒，停止并回报，不得擅自扩白名单。

**20. 风险与注意事项：** ① 白名单已确认含 `exec.wave.failed`（A4）；注入后重读发布会激活 failure-handler 的 correction 流程——这是设计意图，但测试环境需确认不触发真实 hat spawn（集成测试只断言账本行，不跑 loop 主体）。

---

### Unit 4：silent activation 归因分类（merge_failed / backend_died / never_emitted）

**1. Unit 目标：** isolated activation 结束且账本无新事件时，runtime 依据已采集的 outcome 事实分类处置：merge 失败先重试一次；backend 早死不计入 publish-obligation hard gate；真空 activation 维持现状。覆盖 S4.1–S4.4。

**2. 对应需求与 Scenario：** R4；S4.1–S4.4；D5；E8/E9/E16。

**3. 外部可观察结果：** 08-26 同类事故（merge 失败）下 loop 不再 3 轮 no-progress 后 fail-close，而是重试成功即正常推进；诊断/runtime-trace 的 reason 字段可区分 `merge_failed` / `backend_died` / `never_emitted`；08-10 同类事故（backend 早死）不再被记为「有发布义务但没发」。

**4. 当前行为基线：** gate 判定只看账本解析结果（inner.rs:4712-4732），全部坍缩为同一文案（inner.rs:4731；diagnosis/reporter.rs:740-746），3 次 fail-close（event_loop/terminal_routing.rs:322）。watchdog 超时（inner.rs:3389-3437）注释写明无 emit 时落入 missing-event gate。现有回归锚点：`crates/ralph-core/src/event_loop/tests/replay_light_integration.rs:1449`（missing_event reason_code）、`activation_outcome.rs` T5/T6（merge 失败/成功 fixture，只读目标 :699-721）。

**5. 输入与输出：** 输入 = activation outcome 事实集（channel_bytes / merge 结果 / backend_success / exit_code / watchdog / output_bytes，E8）+ 账本解析结果。输出 = 分类 → 处置（重试 merge / backend 失败路径 / 现状计数）+ 诊断 reason。错误：重试 merge 仍失败 → quarantine channel + reason=merge_failed + 计数。不变量：never_emitted 的计数与文案与基线一致（S4.4）。

**6. 修改位置：**
- `crates/ralph-cli/src/loop_runner/` 新增小模块或加入既有模块（候选：`activation_outcome_close.rs` 同文件，职责最近）— 纯函数 `classify_silent_activation(facts) -> SilentActivationClass { NeverEmitted, MergeFailed, BackendDied }`。优先级规则：merge 结果失败 → MergeFailed；否则 backend_success=false 或 watchdog → BackendDied；否则 NeverEmitted。（Emit 被拒类已由 `agent_wrote_any_valid_or_rejected` 覆盖，到不了 gate，E8。）
- `crates/ralph-cli/src/loop_runner/activation_outcome_close.rs`（修改）— 非空 channel merge IO 失败（现仅诊断 + error!，:91-104）改为：返回可重试的失败信号给调用侧，并保留 channel 文件路径。
- `crates/ralph-cli/src/loop_runner/inner.rs:4712-4732`（修改）— gate 判定前取分类：MergeFailed → 调一次 `merge_hat_channel_at_path` 重试（hat_channel.rs:87 既有入口），成功则正常返回（事件落账）；BackendDied → 跳过 publish-obligation 计数（含 watchdog_timeout 与 `backend_success=false` 且未走 termination 早退的路径），记 reason 后继续既有非 publish-gate 处理；NeverEmitted → 现状。**不要**把 BackendDied 接到 inner.rs:3462 的 evidence-window flush（那不是失败处置分支）。
- `crates/ralph-core/src/event_loop/rejection.rs:44-65`（修改）— `RejectionStage` 增加 `MergeFailed` / `BackendDied` 变体（诊断枚举扩展）。
- `crates/ralph-core/src/diagnosis/reporter.rs:740-746`（修改）— 文案按新变体分支。
- 明确不修改：`hard_gate.rs` 的 grace/overdue 逻辑、`should_gate_missing_events` 的义务判定（E9）——本 Unit 只改「判定为 silent 之后」的分类与处置。

**7. 可依赖能力：** outcome 事实已采集（E8）；merge 入口可重入（hat_channel.rs:87 纯文件操作，幂等 stamp）；quarantine 模式（E12）。

**8. 禁止依赖的未来能力：** 无。不得在本 Unit 改 supervisor wave 侧（worker 的 empty_worker_result 分类在 outcomes.rs:42-48，不属于 isolated hat-channel 路径，不动）。

**9. 验收测试：** `crates/ralph-cli/src/loop_runner/tests/legacy/activation_outcome.rs` 扩展：① 只读目标 fixture（:699-721 模式）首次 merge 失败 + 第二次放行 → 事件落账、gate 计数不变（S4.1）；② 两次都失败 → reason=merge_failed、quarantine 存在、计数 +1（S4.2）；③ backend_success=false + 空 channel → 不计 publish-obligation（S4.3）；④ 干净空 activation → 现状计数（S4.4 回归）。运行：`cargo nextest run -p ralph-cli --bin ralph -- activation_outcome`。

**10. Acceptance Red：** S4.1 测试先跑——merge 失败一次即被计 gate 或无重试逻辑（断言重试后账本有事件，当前实现无重试 → 账本空 → Red）。无效 Red：fixture 权限模拟失效（只读目录在 root/某些 FS 上不生效应改用注入错误，实现期验证 fixture 有效性）。

**11. 单元测试拆分：**
- T4.1 `classify_merge_failed_when_merge_err`；T4.2 `classify_backend_died_when_backend_failed`；T4.3 `classify_never_emitted_when_clean_empty`；T4.4 `merge_failed_precedes_backend_died`（两事实同存时 merge 优先——merge 失败时 backend 状态不可信）。
- T4.5 重试成功路径（mock merge 首次 Err 二次 Ok——注意：只允许 stub 文件系统错误注入，不得 mock 掉账本写入本身）。
- T4.6 RejectionStage 新变体的诊断文案渲染。
- 不允许 mock：真实 merge 重试必须走 `merge_hat_channel_at_path` 真实文件操作。

**12. Red → Green → Refactor 顺序：** T4.1–T4.4 Red（分类器不存在）→ 纯函数 → Green → T4.5 Red（无重试）→ activation_outcome_close/inner 重试接线 → Green → T4.6 Red → 诊断变体+文案 → Green → S4.3 backend 分支接线（Red → Green）→ S4.4 回归确认 → Refactor。

**13. 最小实现范围：** 分类器 + 单次 merge 重试 + backend_died 绕行 + 诊断变体。明确不实现：多次重试/指数退避（单次即够，merge 失败是瞬时 IO 错误的假设由 S4.1 验证）；跨 activation 重试（残留 channel 下轮消费——非目标，见 §1）。

**14. 集成验证：** activation_outcome 测试文件全绿 + `cargo nextest run -p ralph-cli --bin ralph -- hard_gate`。真实验证：merge 走真实文件 I/O。

**15. 风险驱动测试：** 故障注入贯穿（T4.5）；S4.4 即回归保护。

**16. 回归范围：** `cargo nextest run -p ralph-cli --bin ralph -- activation_outcome`；`-- hard_gate`；`cargo nextest run -p ralph-core -- replay_light`（missing_event 消息断言，E9）；`cargo nextest run -p ralph-core -- rejection`；`-- diagnosis`。理由：gate 判定是所有 isolated preset 的共用路径。

**17. 预期文件变更：**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `loop_runner/activation_outcome_close.rs` | 修改 | 分类器 + 可重试信号 | E8 |
| `loop_runner/inner.rs` | 修改 | gate 判定接入分类处置 | E8/E9 |
| `event_loop/rejection.rs` | 修改 | 新 RejectionStage 变体 | E9 |
| `diagnosis/reporter.rs` | 修改 | 文案分支 | E9 |
| `loop_runner/tests/legacy/activation_outcome.rs` | 修改（新增用例） | S4.1–S4.4 | E15 |

**18. 完成标准：** S4.1–S4.4 全绿；回归范围全绿；`replay_light_integration.rs:1449` 的 missing_event 断言未削弱（如文案调整必须同步更新断言并在提交说明原因）；clippy/build 通过。

**19. 停止条件：** outcome 事实在 gate 判定点不可得（生命周期与 E8 描述不符）；merge 重试会重复 stamp `triggered` 回填导致重复事件（hat_channel.rs:104-264 语义若为非幂等 → 停止，改为不自动重试只改归因分类，并回报决策变更）。

**20. 风险与注意事项：** ① merge 重试的幂等性（见停止条件 ②，U4 第一步验证 hat_channel merge 是否幂等）。② backend_died 绕行后若该 hat 事件本就必需，wave 仍会 stall——这是正确行为（backend 死了重试 activation 才有意义），诊断 reason 使归因可查。③ 不得把 BackendDied 也做成「合成 default_publishes 兜底」（inner.rs:4733-4753 现状对 never_emitted 的兜底），那会伪造业务事件。

---

### Unit 5：record_slot_pid 接线 + diagnose 展示真实 pid

**1. Unit 目标：** slot worker spawn 成功后把真实 pid 写入 `dispatch_records.pid`；`ralph diagnose` 读取真实值。覆盖 S5.1/S5.2。

**2. 对应需求与 Scenario：** R5；S5.1/S5.2；D6；E10/E11。

**3. 外部可观察结果：** wave 运行期间 `dispatch_records.pid` 非 NULL；`ralph diagnose` 输出 slot pid 为真实值。

**4. 当前行为基线：** INSERT 不含 pid（rusqlite.rs:586）；API 存在无调用（E10）；diagnose 近似（diagnose.rs:1059 注释自证）。

**5. 输入与输出：** 输入 = spawn 后的进程 pid。输出 = store 行更新。错误：pid 不可得（如 executor 未暴露）→ 保持 NULL + warn（不阻断 dispatch）。不变量：dispatch 流程不因 pid 写入失败而失败。

**6. 修改位置：**
- `crates/ralph-adapters/src/pty_executor.rs:363,1813`（修改，最小）— 暴露 pid（若已暴露则零改动；U5 第一步验证）。
- `crates/ralph-cli/src/loop_runner/wave/worker.rs`（修改）— `run_wave_worker` 结果/回传结构携带 pid。
- `crates/ralph-cli/src/loop_runner/wave/dispatcher/dispatch.rs:2531` 附近（修改）— JoinSet spawn 后调 `record_slot_pid(wave_id, slot_index, pid)`（rusqlite.rs:1564 既有）。
- `crates/ralph-cli/src/commands/diagnose.rs:1059`（修改）— 删除「Until U7's `record_slot_pid`」近似逻辑，读真实 pid；注释改为本计划 U5 已接线。
- 明确不修改：`try_dispatch_next` 的 INSERT（pid 回填走 UPDATE 语义，既有 API）；不实现 reaper。

**7. 可依赖能力：** record_slot_pid 三实现（E10）；fake executor 基座（E15，需确认 fake 也可注入假 pid——若不能则 store 层单测覆盖 + 真实路径以 warn 降级）。

**8. 禁止依赖的未来能力：** 无（reaper 是显式非目标）。

**9. 验收测试：** dispatcher 测试：spawn（fake/真实 PTY 视基座能力）→ 断言 store `dispatch_records.pid` 等于回传值。diagnose 测试：构造 store 行 → 输出含 pid。运行：`cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` + `cargo nextest run -p ralph-cli --bin ralph -- diagnose`。

**10. Acceptance Red：** 断言 dispatch_records.pid 非 NULL → Red：恒 NULL（E11）。无效 Red：测试没真正走到 spawn（slot 未 dispatch）。

**11. 单元测试拆分：** T5.1 `record_slot_pid_round_trip`（rusqlite+memory）；T5.2 `pid_write_failure_does_not_block_dispatch`（stub store Err）；T5.3 diagnose 输出渲染。

**12. Red → Green → Refactor：** T5.1 Red（API 已有，可能直接 Green——若直接 Green 则作为 characterization 保留，在提交说明）→ T5.2 Red → 错误降级 → Green → dispatcher 接线（验收 Red → Green）→ T5.3 → Refactor。

**13. 最小实现范围：** pid 回传链路 + 一次 store 调用 + diagnose 改读。明确不实现：进程存活检测、kill、任何周期任务。

**14. 集成验证：** wave_supervisor 测试套件 + diagnose 测试。

**15. 风险驱动测试：** 无额外（接线类改动，故障注入已由 T5.2 覆盖）。

**16. 回归范围：** `cargo nextest run -p ralph-core -- supervisor`；`cargo nextest run -p ralph-cli --bin ralph -- wave`；`-- diagnose`；`integration_wave_channel_convergence`。

**17. 预期文件变更：**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `ralph-adapters/src/pty_executor.rs` | 修改（如需） | 暴露 pid | E10 |
| `wave/worker.rs` | 修改 | pid 回传 | E10 |
| `dispatcher/dispatch.rs` | 修改 | record_slot_pid 调用 | E10/E11 |
| `commands/diagnose.rs` | 修改 | 真实 pid 展示 | E10 |
| 测试（wave_supervisor / diagnose / supervisor store） | 新增用例 | S5.1/S5.2 | E15 |

**18. 完成标准：** S5.1/S5.2 全绿；回归全绿；clippy/build 通过；pid 失败降级 warn 路径有测试。

**19. 停止条件：** pty_executor 与子进程句柄之间无 pid 通道且 `run_wave_worker` 拿不到 `Child`（两处都不可达 → 停止，回报重新决策 D6）；record_slot_pid 的 rusqlite 实现与 trait 签名不匹配。

**20. 风险与注意事项：** ① PTY 场景 pid 是会话首进程，子进程组不在本 Unit 范围（非目标）。② fake executor 若无 pid 概念，用约定值（如 0 或 fixture pid）并注明，不得伪造「真实 pid」断言。

---

### Unit 6：worker channel 失败现场 quarantine

**1. Unit 目标：** slot 非干净结局（empty/failed/timeout）时 channel 文件移入 `.ralph/diagnostics/failed-activations/`；干净成功仍删除。覆盖 S6.1/S6.2。

**2. 对应需求与 Scenario：** R6；S6.1/S6.2；D7；E1/E12。

**3. 外部可观察结果：** 失败 wave 运行后 diagnostics 目录可找到 slot channel 原文件（含 wave/slot/attempt 命名）；成功运行无残留。

**4. 当前行为基线：** worker.rs:642 无条件删除（U1 后改为「持久化成功后删除」——本 Unit 在 U1 的新删除点上加分支）。quarantine 模式证据：hat_channel.rs:287-313。

**5. 输入与输出：** 输入 = slot outcome 分类（outcomes.rs:181 classify 结果）+ channel 路径。输出 = 文件移动或删除。错误：移动失败 → warn + 退化为删除（不阻断）。不变量：channel 文件最终必然离开原路径（不干扰后续 wave）。

**6. 修改位置：** `crates/ralph-cli/src/loop_runner/wave/dispatcher/dispatch.rs`（U1 移交后的删除决策点）— 按 outcome 分类分支：clean success → 删除；其余 → 移动。quarantine 目录与命名模式复用 E12。明确不修改：outcomes.rs 分类逻辑本身。

**7. 可依赖能力：** U1 的删除时机移交；hat_channel quarantine 实现模式（E12）。

**8. 禁止依赖的未来能力：** 无。

**9. 验收测试：** dispatcher 测试：构造 empty/failed 结局 → 断言 quarantine 文件存在且原名路径不存在；clean success → 文件删除。运行：`cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`。

**10. Acceptance Red：** failed 结局后断言 quarantine 文件存在 → Red：文件被删（现状）。无效 Red：quarantine 目录权限（用 tempdir）。

**11. 单元测试拆分：** T6.1 failed→quarantine；T6.2 empty→quarantine；T6.3 success→删除；T6.4 移动 IO 失败→warn+删除降级。

**12. Red → Green → Refactor：** T6.1 Red → 移动逻辑 → Green → T6.2/T6.3 Red（分支覆盖）→ Green → T6.4 → Green → Refactor。

**13. 最小实现范围：** 删除点分支 + 移动函数。明确不实现：quarantine 清理策略（保留策略随诊断目录既有惯例）；wave 结束后的批量清理。

**14. 集成验证：** wave_supervisor 套件 + `integration_wave_channel_convergence`（timeout/cancel 场景会经过新分支）。

**15. 风险驱动测试：** T6.4 故障注入（依据：诊断目录写失败不得连锁阻断 wave 处理——E15 已有 diagnostics 写失败场景先例）。

**16. 回归范围：** `cargo nextest run -p ralph-cli --bin ralph -- wave`；`integration_wave_channel_convergence`。

**17. 预期文件变更：**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `dispatcher/dispatch.rs` | 修改 | quarantine 分支 | E1/E12 |
| wave_supervisor 测试目录 | 新增用例 | S6.1/S6.2 | E15 |

**18. 完成标准：** S6.1/S6.2 全绿；回归全绿；clippy/build 通过。

**19. 停止条件：** U1 的删除移交实际未落在 dispatch.rs（与 U1 实际实现不符 → 以 U1 落点为准调整本 Unit 位置并记录）；outcome 分类在删除决策点不可得。

**20. 风险与注意事项：** quarantine 目录体积增长——wave channel 为 KB 级且仅失败路径写入，可接受；如诊断目录已有清理策略则遵循，没有则不新增（非目标）。

---

### Unit 7：Minimal 诊断会话下 hat_activation_outcome 行落盘

**1. Unit 目标：** 在 Minimal runtime-diagnosis 会话（`enabled && write_artifacts`）中，每次 isolated activation 的 `hat_activation_outcome` 行必须写入 `runtime-trace.jsonl`。覆盖 S7.1/S7.2。**不是**「给 DiagnosticLevel 加豁免」——该枚举不存在。

**2. 对应需求与 Scenario：** R7；S7.1/S7.2；D8；E13。

**3. 外部可观察结果：** Minimal 会话的 runtime-trace sidecar 含每次 isolated activation 的 outcome 行。默认 Disabled collector（`write_artifacts=false`）行为不变（S7.2 的「非 Minimal」在此解读为 full session 与 Disabled：full 与基线一致；Disabled 仍无 sidecar）。

**4. 当前行为基线（2026-09-01 重核）：** 两处 None 早退——`activation_outcome.rs:406`（无 session_dir）与 `diagnostics/mod.rs:1662`（无 logger）。Minimal 会话**会**创建 logger（mod.rs:927）。**先 Characterization：** 用 `DiagnosticsCollector` 的 minimal 选项调用 `log_activation_outcome_with_diagnostics`。若 sidecar 已有 outcome 行 → 本 Unit 收缩为契约测试钉住该行为，**禁止**再改生产代码去「豁免早退」。若仍无行 → 沿调用链定位真实拦截点（可能是 kind 未传到 logger、或 session 创建失败）。

**5. 输入与输出：** 输入 = outcome 行 + collector 模式。输出 = sidecar 行或明确的 Disabled no-op。不变量：Disabled collector 零 I/O；full session 其它 kind 行为不变；不得把默认 `write_artifacts` 改为 true（D8）。

**6. 修改位置：** 仅在 Characterization 证明 Minimal 仍丢行时修改。候选（按优先级，实现期只动被证伪的那一处）：`activation_outcome.rs:406` 的 session_dir 守卫；`diagnostics/mod.rs:1648` 的 logger None 路径（只允许为 outcome kind **lazy 打开** runtime-trace，且仅当 collector 已 enabled）。`crates/ralph-core/tests/hat_activation_outcome_contract.rs`（新增 Minimal 用例，无论生产是否改动）。明确不修改：默认 telemetry、其它 sidecar、cap manifest（DEV-8 非本 Unit）。

**7. 可依赖能力：** outcome contract 测试基座（E13）；`TelemetryConfig::to_diagnostics_options`（telemetry.rs）。

**8. 禁止依赖的未来能力：** 无。禁止开启默认 full diagnostics。禁止修 cap manifest / diagnosis-input 空数组（属 DEV-8，非本计划）。

**9. 验收测试：** contract 测试新增：Minimal 会话记录一次 outcome → sidecar 含该行（S7.1）；Disabled collector → 无文件/无 panic（S7.2 边界）。运行：`cargo nextest run -p ralph-core --test hat_activation_outcome_contract`。

**10. Acceptance Red：** 仅当 Characterization 显示 Minimal 丢行时，S7.1 才构成生产代码 Red。若 Characterization 已 Green，Red 阶段就是「合同测试尚未提交」——补测试即可，不要为了制造 Red 去改生产。

**11. 单元测试拆分：** T7.1 `minimal_session_writes_outcome_row`；T7.2 `disabled_collector_skips_all_rows`（Characterization，当前应为 Green）；T7.3 `full_session_unchanged`。

**12. Red → Green → Refactor：** T7.1/T7.2 先跑 Characterization → 若 T7.1 已 Green 则只留测试 → 若 Red 再按 §6 最小修补 → T7.3 → Refactor。

**13. 最小实现范围：** 契约测试必做；生产代码仅在 Minimal 确证丢行时改。明确不实现：诊断级别体系、其它 kind 强制落盘、默认打开 write_artifacts。

**14. 集成验证：** contract 测试 + `cargo nextest run -p ralph-core -- diagnostics`。

**15. 风险驱动测试：** Characterization（T7.2，依据：不得把 Disabled 变成写盘）。

**16. 回归范围：** `cargo nextest run -p ralph-core -- diagnostics`；`cargo nextest run -p ralph-core --test hat_activation_outcome_contract`；`cargo nextest run -p ralph-cli --bin ralph -- activation_outcome`。

**17. 预期文件变更：**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `tests/hat_activation_outcome_contract.rs` | 修改（新增用例） | S7.1/S7.2 | E13 |
| `diagnostics/mod.rs` 或 `activation_outcome.rs` | 仅当 Characterization Red | 真正拦截点 | E13 |

**18. 完成标准：** S7.1/S7.2 全绿；回归全绿；clippy/build 通过；若未改生产代码，提交信息写明 Characterization 已 Green 的证据。

**19. 停止条件：** 真实缺口是 Disabled 默认路径（无 session）而产品要求「所有 loop 都有 outcome 行」→ 停止回报，不得擅自把 `write_artifacts` 默认改 true（与 D8 冲突，需新决策）。改动面扩大到诊断初始化/cap manifest → 停止回报。

**20. 风险与注意事项：** 08-29 DEV-8 主因是 cap 表空，与 outcome 行只部分重叠；本 Unit 不修 cap。wave worker 路径可能根本不调用 `log_activation_outcome_with_diagnostics`——那是 isolated hat 范围外，发现后记入 follow-up 不要扩 scope。

---

## 8. Unit 串行依赖图

```
U1（slot 事件持久化 + persist-before-delete）
  ↓ U2 消费 U1 的 load_slot_event_payloads 与新表语义；且 U1 把 channel 删除决策移交 dispatcher，U2/U6 的分支都挂在该决策点上
U2（崩溃恢复补偿投递）
  ↓ U3 的「先 salvage 再注入 failed」复用 U2 的 recovery_redelivery 模块与接线点
U3（超时 wave 注入 exec.wave.failed）
  ↓ 无能力依赖；排在 U4 前是因为同属恢复/投递主线，先闭合 wave 侧再回 hat 侧
U4（silent activation 归因分类）
  ↓ 无能力依赖；U5/U6 同属 wave worker 取证，U4 完成 hat 侧后回到 wave 侧收口
U5（pid 接线）
  ↓ 无能力依赖；与 U6 同在 worker/dispatch 区域，相邻执行降低上下文切换
U6（channel 失败现场 quarantine）
  ↓ 依赖 U1 移交后的删除决策点（U1 必须先完成）
U7（Minimal outcome 行落盘）
  ↓ 无能力依赖；先 Characterization 再决定是否改生产；最小改动放最后
```

- 为什么不可交换：U2/U3 依赖 U1 的存储能力（硬依赖）；U6 依赖 U1 的删除点移交（硬依赖）；U3 依赖 U2 的恢复模块（硬依赖）。U4/U5 无硬依赖，固定在现位置仅为降低风险（先主线后旁支），不与任何 Unit 并行。
- 如何避免提前实现：U1 明确「不写恢复消费」；U2 明确「不做超时注入」；各 Unit §8 均已列出禁令。

---

## 9. 执行命令清单

> 全部遵守 AGENTS.md HARD RULE 1/2/5：测试入口只走 `cargo nextest run` 系列；spawn ralph 的集成测试必须用 `tests/common/mod.rs` 的 scrub helper；最终全量走 `./scripts/run-tests.sh`（不要在开发中途跑全量代替 targeted）。

| 命令 | 运行时机 | 验证目的 | 预期结果 | 失败能否继续 |
|---|---|---|---|---|
| `cargo nextest --version` | 开工前 | 版本 = 0.9.140（mise.toml SSOT） | `0.9.140` | 否（先 `mise install`） |
| `sed -n '…p' <file>` 复核 E1–E17 行号 | 每个 Unit 第一步 | 证据与基线一致 | 符号在 ±20 行内 | 否（触发停止条件） |
| `cargo nextest run -p ralph-core -- supervisor` | U1/U2/U3/U5 的 Red 与 Green | store 双实现、recover、coordinator | 目标测试先 Red 后 Green，其余全绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` | U1/U2/U3/U5/U6 | dispatcher/worker/fan-in | 同上 | 否 |
| `cargo nextest run -p ralph-cli --test integration_wave_channel_convergence` | U1/U2/U3/U6 | wave 投递全链路回归 + 新断言 | 全绿（含 s_08 crash window） | 否 |
| `cargo nextest run -p ralph-cli --test integration_wave_recovery_redelivery` | U2/U3 | S2.1–S2.4/S3.1/S3.2 | 先 Red 后 Green | 否 |
| `cargo nextest run -p ralph-cli --test u7_real_ralph_emit` | U1/U2 | 真实 emit 链路回归 | 全绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- activation_outcome` | U4/U7 | S4.1–S4.4 | 先 Red 后 Green | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- hard_gate` | U4 | gate 相邻回归 | 全绿 | 否 |
| `cargo nextest run -p ralph-core -- replay_light` | U4 | missing_event 消息回归（E9；测试在 ralph-core） | 全绿 | 否 |
| `cargo nextest run -p ralph-core -- rejection` / `-- diagnosis` | U4 | 诊断枚举/文案相邻 | 全绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- diagnose` | U5 | S5.2 | 先 Red 后 Green | 否 |
| `cargo nextest run -p ralph-core --test hat_activation_outcome_contract` | U7 | S7.1/S7.2 | Characterization 先跑；仅丢行时才有生产 Red | 否 |
| `cargo nextest run -p ralph-core -- diagnostics` | U7 | 诊断级别相邻回归 | 全绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- task_projection` | U2 | 恢复段相邻（E6） | 全绿 | 否 |
| `cargo clippy --workspace` | 每个 Unit 收尾 | Lint（pedantic） | 零警告（既有基线水平） | 否 |
| `cargo build` | 每个 Unit 收尾 | 编译 | 通过 | 否 |
| `cargo fmt --check` | 每个 Unit 收尾 | 格式 | 通过 | 否 |
| `./scripts/run-tests.sh` | **仅 U7 完成后一次**（nextest 两阶段 + doctest） | 最终全量门禁 | 全绿 | 否 |
| `RALPH_CURRENT_HAT=executor RALPH_EVENTS_FILE=/tmp/x.jsonl cargo nextest run -p ralph-cli --test integration_wave_recovery_redelivery` | U2/U3 收尾 | HARD RULE 5 污染验收 | 全绿 | 否 |

---

## 10. 最终质量门禁

* S1.1–S7.2 全部 Scenario 验收通过；R1–R7 每需求至少一个可执行测试（§6 矩阵逐行核对）。
* §9 所有 Unit 级命令在最后状态全绿；`./scripts/run-tests.sh` 全绿（含 phase 2 串行隔离与 doctest）。
* 无新增失败/跳过测试；无 `.only`；无 Snapshot/Golden 更新（本计划无 snapshot 类测试，出现即违规）；无削弱断言（`replay_light` 的 missing_event 断言若因文案分支调整，必须在提交信息中说明新旧映射）。
* 健康路径行为不变证明：`integration_wave_channel_convergence` 的账本输出与基线一致（s_01–s_10 全绿且未改断言）。
* D1–D9 置信度在执行中未跌破 0.85；跌破过的 Unit 必须有停止→补证据→复评记录。
* 实际变更文件集合 ⊆ §7 各 Unit §17 的并集；任一越界文件需在提交信息中说明归属哪个 Unit 的哪个停止条件处理后修订。
* 每个 Unit 形成完整 TDD 闭环（Red 记录 → Green → 回归 → 关闭）；Unit 顺序 = U1→…→U7，无交叉提交。
* 本计划不改 `presets/**` 与 `parallel_forge_*` scenario（E18；证据门禁已合入，禁止回改）。

---

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每 Unit 有入口文件/行号、Red 失败原因、完成标准 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D9 全部定案；仅余行号复核与二个实现期微选择（T1.3 幂等语义二选一、restore_unmerged 接线或删除），均已给出判定标准且不影响行为契约 |
| 所有文件和接口是否有代码库证据 | 是 | §2.2 E1–E18；新增文件均标注「计划新增」 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | §3 最低 0.85（D2–D6、D8），D1/D7 0.90，D9 0.95 |
| 是否存在未处理的低置信度假设 | 否 | §1 已确认假设 A1–A3 均有代码证据；待验证假设清零 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 持久化、U2 补偿投递、U3 超时注入、U4 归因分类、U5 pid、U6 quarantine、U7 诊断豁免 |
| 每个 Unit 是否可以独立验证 | 是 | 各有独立测试命令与完成标准 |
| 每个 Unit 是否有真实 Red | 是 | 各 Unit §10 写明 Red 表现与无效 Red 排除 |
| 每个 Unit 是否包含回归范围 | 是 | 各 Unit §16 基于调用链列出 |
| 是否存在未来 Unit 依赖 | 否 | §8 依赖图每条边注明已验证能力来源 |
| 是否存在泛化任务描述 | 否 | 无「完善/优化/相应」类表述 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §5/§6 矩阵 |
| 所有关键决策是否有 Evidence | 是 | §3 每行引用 E 编号 |
| 计划是否可以严格串行执行 | 是 | §8 线性图，无并行边 |

---

## 附：明确的非目标与后续立项建议（不属本计划执行范围）

1. **主动死进程 reaper / 周期性 coordinator tick**：需新机制（in-loop tick 锚点），待 U5 pid 数据落地后立独立计划。
2. **P0-4 correction 轮次 runtime 锚点**：跨事件计数无声明式机制（诊断报告 §3），候选为硬编码门禁（red-team 队列模式，validation.rs:124-437）或通用 counter API，需独立计划。
3. **P0-2/P0-3 残余**（audit.done verdict 分流、wave 上下文 runtime 注入）：证据门禁计划已合入；若仍有残余，以 `21dfbab` 上 preset/schema 为准另立计划，不在本计划改 preset。
4. **record_slot_result warn-only 可见性**（08-29 worker_results=0 疑点）：需复跑取证确认后立项。
5. **P2/P3**（预算调参、死配置清理、alias 移除）：hygiene 批次，不在本计划。
