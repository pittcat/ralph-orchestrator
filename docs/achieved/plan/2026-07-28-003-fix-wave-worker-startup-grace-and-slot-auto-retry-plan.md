---
title: Wave Worker 启动宽限期与 Slot 自动重试接线修复计划
type: fix
date: 2026-07-28
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan
execution: code
---

# Wave Worker 启动宽限期（startup grace）与 Slot 自动重试接线修复计划

## 0. 计划状态

**READY，首版。**

- **代码基线：** `0eb29e01`（`fix(preset): parallel-forge 放宽 executor 并发与 idle 心跳窗口`）。
- **上游诊断：** `docs/report/2026-07-28-parallel-forge-primary-20260728-110733-diagnosis.md`（P0-1 置信度 85 / P1-1 置信度 75）。
- **调查范围：** `crates/ralph-cli/src/loop_runner/wave/{worker,heartbeat,dispatcher,supervisor_bridge}.rs`、`crates/ralph-cli/src/loop_runner/{runner.rs,tests/wave.rs,tests/wave_supervisor.rs}`、`crates/ralph-core/src/{config/hat.rs,config/loop_config.rs,wave_detection.rs,supervisor/{worker_outcome,memory,bridge,mod}.rs}`、`presets/en/{parallel-forge,ce-executor-supervisor}.yml`、`crates/ralph-core/data/ralph-tools-wave.md`、`CONCEPTS.md`、`skills/ralph-preset-common/references/patterns.md`、`docs/achieved/plan/2026-07-25-005-fix-supervisor-slot-activity-salvage-redrive-plan.md`、`docs/achieved/plan/2026-07-25-006-feat-wave-worker-idle-heartbeat-lease-plan.md`。
- **已执行验证：** 全部关键调用链源码勘察；`classify_failure_reason` 全仓调用点 grep（生产零调用）;`WorkerRequest` 字段逐项 Clone 性确认；preset schema 不含 hat 字段钉值（grep `presets/schemas/parallel-forge.yml`）。
- **尚未执行验证：** 本计划各 Unit 的 Acceptance Red/Green、build、clippy、全量测试，由对应 Unit 执行。
- **阻塞项：** 无。

---

## 1. 功能目标

### 业务目标

消除诊断 P0-1 的机制根：`claude` headless backend 冷启动期零输出被 idle 心跳误杀；并让「已设计未接线」的 slot 自动重试真正生效——误杀类可重试失败在 wave 内自动重派，不再一步到位整波失败。

### 用户或调用方

- `parallel-forge` / `ce-executor-supervisor` 等 supervisor preset 的 operator：backend 启动慢不再被误杀；可重试的 slot 失败自动自愈一次。
- preset 作者：获得 `startup_grace_secs` hat 字段与 `slot_retry_budget` supervisor 字段两个显式旋钮。

### 当前行为（均有证据）

1. **idle 窗口自 spawn 起算**：`worker.rs:285` `LeaseState::fresh(0)` —— spawn 即第 0 号信号；backend 在「spawn → 第一行输出」之间没有任何保护窗，`idle_heartbeat_secs` 到期即杀（诊断 P0-1：slot 4 PTY 零输出 120s 被杀）。
2. **kill 归因只有两种**：`worker.rs:546-558` 仅区分 hard timeout 与 idle_kill（weak_count>0 / =0）。
3. **自动重试是半成品**：分类器 `classify_failure_reason`（`worker_outcome.rs:97`）与可重试白名单（`worker_outcome.rs:53-58`，含 `worker_timeout`）已存在且有表驱动测试，但**生产路径零调用**（仅 `is_retryable_slot_reason` 在 `dispatcher.rs:3409` 用于 `redrive_slots` payload）；`SupervisorConfig`（`loop_config.rs:1121-1178`）**没有** `slot_retry_budget` 字段；dispatcher 注册 wave 时预算**硬编码 1**（`dispatcher.rs:1660`、`2351`）。CONCEPTS.md:71-73 宣称的「自动 redispatch 默认 1 次」并未发生。
4. **slot 失败即终态**：每个 worker task 内 `classify_slot_result`（`dispatcher.rs:5308`）→ `record_slot_failure`（`dispatcher.rs:4835`）→ drop guard `release_slot_dispatch(Failed)`（`dispatcher.rs:226-240` → `memory.rs:560-596`），无任何重派分支。

### 目标行为与差异

- 新增 hat 级 `startup_grace_secs`：idle 模式启用时，**首个合格信号（Strong/Weak 行或 events-file 增长）之前**的静默窗口用 `startup_grace_secs` 取代 `idle_heartbeat_secs`；首个合格信号后回到现有 idle 语义。未配置 / `0` = 现状。
- startup 超时杀产生**独立归因字符串**（`startup_kill`），仍归 `worker_timeout` family（可重试、可 redrive）。
- 新增 `event_loop.supervisor.slot_retry_budget`（默认 1，允许 0..=2，>2 启动期拒绝）；dispatcher 两处注册点改为读配置值；worker task 内在「分类为可重试失败且预算未尽」时**同 task 内重试**，不先进 store Failed 中间态。

### 输入 / 输出 / 状态变化 / 错误语义

- **输入：** preset YAML 的 `hats.<id>.startup_grace_secs`、`event_loop.supervisor.slot_retry_budget`。
- **输出：** startup 超时 worker 的 Err reason 以 `Worker timed out after` 前缀携带 `startup_kill` 标记；自动重试在日志可见（warn 携带 slot_index / attempt / budget）。
- **状态变化：** 可重试失败在预算内不再使 slot 进入 Failed；预算耗尽后与现状完全一致（record_slot_failure + wave failed）。
- **错误语义：** `slot_retry_budget > 2` → loop 启动期拒绝 supervisor 启动（fail-closed）；未知 / 非白名单 reason 永不重试（fail-closed）。

### 兼容、性能、安全与约束

- **兼容：** `startup_grace_secs` 缺省 = 行为逐位不变；`slot_retry_budget` 缺省 1 = 与 DB 列默认值（`migrations.rs:220`）及 CONCEPTS.md 宣称语义一致。旧 YAML 两字段缺省均可加载（`HatConfig` 无 deny_unknown_fields，`hat.rs:344-345`；`SupervisorConfig` 有 serde default）。
- **性能：** 纯函数分支 + task 内一次循环，零新增 I/O；重试消耗 wave 既有 aggregate/global deadline，不新增时间预算。
- **安全：** 不放宽 EventOriginGuard / HatCommandPolicy / topic_deny_rules 任何边界；重试复用同一 slot 身份与同一 worktree 绑定。
- **测试入口：** 严禁裸跑 `cargo test -p ralph-cli`；一律 nextest（`AGENTS.md` HARD RULE 1/2）；涉及 spawn `ralph` 的测试遵守 HARD RULE 5（`common::ralph_bin()` / `scrub_agent_runtime_env`）。

### 本次范围

- `startup_grace_secs` 全链路（hat 配置 → DetectedWave → dispatcher 两路径 → WorkerRequest → worker 双时钟 → Lease 纯函数）。
- `slot_retry_budget` 配置 + bridge 访问器 + dispatcher 两注册点替换 + worker task 内自动重试闭环。
- 上述的单元 / 集成测试、preset opt-in、文档同步。

### 非目标

- 不改 legacy（非 supervisor）WaveTracker 路径的重试语义（无 store、无预算概念）。
- 不改 idle 模式的运行期语义（weak_cap、Strong/Weak 分类表、events-file ticker 判定）。
- 不改 `create_redrive_wave` / `ralph wave redrive` operator 路径（那是 2026-07-28-002 计划的范围）。
- 不改 `parallel-forge.yml`（idle 已 600，0eb29e01 已覆盖其 P0 场景）。
- 不引入 startup grace 的「standalone」模式（idle 禁用时单独生效）——留作后续候选。
- 不新增 crate；不改 DB schema（`slot_retry_budget` 列已存在）。

### 已确认假设 / 待验证假设

- **已确认：** `WorkerRequest` 全部字段可 Clone（`CliBackend` derive Clone，`cli_backend.rs:49-50`；sender/Arc/PathBuf/Duration/String 均 Clone）；kill reason 前缀 `Worker timed out after` 会被 `classify_slot_result` 第一臂归类 `Static("worker_timeout")`（`dispatcher.rs:20`、`5359-5366`），因此 startup_kill 归因字符串只需复用同一前缀即可免费获得「可重试 + 入 redrive_slots」语义。
- **待验证（Unit 内验证）：** U5 任务体代码形状与证据锚点（`SupervisorSlotRelease` 创建点、`executor.execute(request)` 调用点、`classify_slot_result` 调用点）一致；若不符触发停止条件。

---

## 2. 代码库现状与证据

### 2.1 当前实现入口

- **idle 双时钟：** `worker.rs:242-330`（LeaseConfig 构建 / `compute_next_deadline`）→ `worker.rs:332-466`（select 循环：timer tick / PTY 行 / events-file ticker）→ kill 归因 `worker.rs:546-558`。
- **Lease 纯函数：** `heartbeat.rs:82-94`（`LeaseConfig`）、`165-214`（`decide_lease`）、`222-298`（`LeaseState::tick`）；表驱动单测 `heartbeat.rs:737-1063`。
- **hat 配置链：** `hat.rs:450/469/480`（`timeout` / `idle_heartbeat_secs` / `idle_weak_signal_cap`）→ `wave_detection.rs:108-122`（`DetectedWave::idle_heartbeat_secs()`，`Some(0)`→`None`）→ `dispatcher.rs:1291-1294`（legacy 路径）与 `dispatcher.rs:1611-1614`（supervisor 路径）→ `WorkerRequest.idle_heartbeat`（`dispatcher.rs:172-176`）→ `run_wave_worker`（`dispatcher.rs:248-255`）→ `run_wave_worker_pty`（测试直调，如 `tests/wave.rs:2002-2015`）。
- **slot 失败出口（supervisor）：** worker task 内 `classify_slot_result`（`dispatcher.rs:5308-5390`）→ 成功 `record_slot_result`/`record_slot_terminal_evidence`（`dispatcher.rs:4783-4827`）、失败 `record_slot_failure`（`dispatcher.rs:4830-4847`）→ guard.outcome（`dispatcher.rs:4870-4875`）→ Drop `release_slot_dispatch`（`dispatcher.rs:219-240` → `memory.rs:560-596`）→ harvest `record_outcome`（`dispatcher.rs:5083-5096`、`5196-5247`）。
- **wave 注册（预算硬编码点）：** `dispatcher.rs:1656-1661`（dispatch 路径）、`dispatcher.rs:2347-2351`（fan-in 路径），均 `register_wave_if_absent(..., 1)`。
- **bridge 构造与配置消费：** `runner.rs:764-771`（`cfg.max_concurrent_workers` 传入 `CoordinatorSupervisorBridge::with_context_and_factory_with_cap`）；`runner.rs:1399` 处 `supervisor_cfg` 在作用域内。
- **重试分类器（已存在未接线）：** `worker_outcome.rs:53-58`（`RETRYABLE_REASONS = [worker_timeout, empty_worker_result, missing_worker_terminal, slot_never_started]`）、`61-70`（非重试名单）、`88-94`（`is_retryable_slot_reason`）、`97-109`（`classify_failure_reason`）；测试 `retry_classifier_tests.rs`。
- **store 预算校验：** `memory.rs:313-315`（`register_wave` 拒绝 >2）、`memory.rs:388-400`（`register_wave_if_absent` 对 kind/total/**retry_budget 不一致**报错——两注册点必须传同值）。
- **历史计划：** 2026-07-25-005 U3（分类器，已落地）/ U4（配置字段，**未落地**）/ U5（dispatcher 接线，**未落地**）；2026-07-25-006（idle lease，已落地）。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `worker.rs:285` | `LeaseState::fresh(0)`：idle 窗口自 spawn 起算 | startup grace 必须改变「首个合格信号前」的窗口来源 | 高 |
| E2 | `heartbeat.rs:165-214, 222-298` | `decide_lease`/`LeaseState::tick` 是纯函数，表驱动测试齐全 | F1 语义扩展落在纯函数层，TDD 成本最低 | 高 |
| E3 | `worker.rs:546-558` + `dispatcher.rs:20, 5359-5366` | kill reason 以 `Worker timed out after` 前缀生成；dispatcher 按前缀归类 `Static("worker_timeout")` | startup_kill 归因字符串复用同一前缀即可免费进入 retryable family，**无需改分类器** | 高 |
| E4 | `hat.rs:344-345, 450-480` | `HatConfig` 无 deny_unknown_fields；三字段均 `Option<u32>` serde default | 新 hat 字段同构扩展；结构体字面量站点需编译器驱动补齐（`doctor.rs:1063`、`hat.rs:671,1007`、`legacy.rs:3640`、`tests/wave.rs:209`） | 高 |
| E5 | `wave_detection.rs:108-122` | `DetectedWave` 访问器模式（`Some(0)`→`None`） | `startup_grace_secs()` 访问器同构 | 高 |
| E6 | `dispatcher.rs:1291-1294, 1611-1614` | 两条 dispatch 路径各自解析 idle 配置为 `Option<Duration>` | 两路径都要加 `startup_grace` 解析 | 高 |
| E7 | `dispatcher.rs:141-177, 248-255` | `WorkerRequest` 携带 idle 参数进 `run_wave_worker` | 新参数同一通道 | 高 |
| E8 | `worker_outcome.rs:53-109` + 全仓 grep | `classify_failure_reason` 生产零调用；`is_retryable_slot_reason` 仅用于 `redrive_slots` payload | 自动重试决策直接复用现成分类器，不重写 | 高 |
| E9 | `loop_config.rs:1121-1178` | `SupervisorConfig` deny_unknown_fields + 4 字段 + Default impl，无 `slot_retry_budget` | 新增字段必须同步 Default impl 与全部结构体字面量（`wave_supervisor.rs:297,326` 等） | 高 |
| E10 | `dispatcher.rs:1656-1661, 2347-2351` | 两处注册预算硬编码 `1` | 替换为配置值；两处必须同值（E12） | 高 |
| E11 | `runner.rs:764-771` | bridge 构造点消费 `cfg.max_concurrent_workers` | `slot_retry_budget` 同点进入 bridge；启动期校验同点 fail-closed（先例：db_path 拒绝启动，`loop_config.rs:1133-1134`） | 高 |
| E12 | `memory.rs:313-315, 388-400` | store 拒绝 budget>2；重复注册 budget 不一致报错 | 两注册点同值由同一 bridge 访问器保证 | 高 |
| E13 | `dispatcher.rs:4830-4847, 4870-4875, 219-240` | 失败记录 + guard outcome + Drop 释放都在 worker task 内 | 重试闭环可以完整收进 task 内部，无需动 harvest/store | 高 |
| E14 | `dispatcher.rs:5083-5096, 5164-5189, 5196-5247` | harvest/sweep/tracker 只消费 task 最终产物 | task 内重试对 harvest 透明（中间失败 tracker 不可见） | 高 |
| E15 | `cli_backend.rs:49-50` | `CliBackend` derive Clone | `WorkerRequest` 可整体 derive Clone（重试重入 execute 的前提） | 高 |
| E16 | `tests/wave.rs:1986-2027` | idle 集成测试模式：fake executable + `run_wave_worker_pty` 直调 + `#[cfg(unix)]` | F1 集成测试同构复用 | 高 |
| E17 | `tests/wave.rs:2568-2602` | KTD7 钉值测试只钉 `ce-executor-supervisor` 的 timeout/idle/weak_cap | 给该 preset 加 `startup_grace_secs` 不破此测试 | 高 |
| E18 | `docs/achieved/plan/2026-07-25-005`（U3/U4/U5） | 自动重试三段设计存在，U4/U5 未落地 | 本计划是其补完，完成标准沿用 U5 原文（同 wave 同 slot 身份、无 Failed 中间态回滚） | 高 |
| E19 | `dispatcher.rs:1714` + `tests/wave_supervisor.rs:1915-1930` | bridge 已有 `max_concurrent_workers()` 访问器模式（含测试 stub 成对实现） | `slot_retry_budget()` 走同模式，编译器驱动补全 impl | 高 |
| E20 | `ralph-tools-wave.md:326-334` + `CONCEPTS.md:71-85` + `patterns.md:312-326` | 三处文档描述双时钟 / slot_retry_budget 语义 | 行为变更后必须同步（HARD RULE） | 高 |

### 2.3 受影响范围

- **生产：** `worker.rs`、`heartbeat.rs`、`dispatcher.rs`、`supervisor_bridge.rs`（bridge impl）、`runner.rs`、`hat.rs`、`loop_config.rs`、`wave_detection.rs`、`ralph-core/src/supervisor/bridge.rs`（trait 定义与 impl）。
- **测试：** `heartbeat.rs` 内联测试、`tests/wave.rs`、`tests/wave_supervisor.rs`、`hat.rs` 内联测试、`wave_detection.rs` 内联测试、`loop_config.rs` 内联测试、`doctor.rs:1063`（HatConfig 字面量）。
- **配置 / preset：** `presets/en/ce-executor-supervisor.yml`（仅 U6）。
- **文档：** `crates/ralph-core/data/ralph-tools-wave.md`、`CONCEPTS.md`、`skills/ralph-preset-common/references/patterns.md`。
- **不受影响：** DB schema、preset 拓扑与 hat instructions、`presets/schemas/*`（不钉 hat 字段，已 grep 确认）、CLI 子命令表面、operator redrive 路径、legacy 非 supervisor dispatch。

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| KTD1 | startup grace 语义挂在哪 | idle 模式内的「首信号前窗口」；独立 standalone  stall 检测 | idle 模式内的首信号前窗口（idle 禁用时该字段无效） | E1, E2；诊断场景就是 idle 误杀 | standalone 需要改 legacy 单时钟路径（`worker.rs:473-523` 无 lease 循环），范围扩大且无事故驱动 | 0.90 |
| KTD2 | 何种信号结束 startup grace | 任意首行（含 None 行）；仅合格信号（Strong/Weak/events-file 增长） | 仅合格信号 | E2 + `heartbeat.rs:354`（claude `system init` 归 None）：banner 后长思考仍受 grace 保护 | 「任意首行」会让 None 行提前结束 grace 且 `last_hb` 未移动，语义不自洽 | 0.88 |
| KTD3 | startup_kill 归因与 family | 新 reason code + 改分类器；复用 `WORKER_TIMEOUT_ERR_PREFIX` 前缀字符串 | 复用前缀：`"Worker timed out after {n}s of startup grace (worker_timeout/startup_kill, no first signal)"` | E3 | 改分类器引入新 reason code 会连带 retry/redrive/failure_class 全链路改动，收益为零 | 0.92 |
| KTD4 | Lease 扩展方式 | `LeaseState.last_hb_ms: Option<u64>`；新增 `seen_first_signal: bool` | 新增 `seen_first_signal: bool` + `LeaseDecision::StartupKill` 新变体 | E2（纯函数 + 表驱动测试） | Option 化 `last_hb_ms` 改动面波及全部既有断言；新变体显式可测且 Display 可 grep | 0.90 |
| KTD5 | `slot_retry_budget` 配置位置与默认值 | `SupervisorConfig` 默认 1；hat 级；默认 0 | `SupervisorConfig.slot_retry_budget` 默认 1，允许 0..=2 | E9, E18 + CONCEPTS.md:71-73 + `migrations.rs:220`（DB 默认 1） | hat 级违背 005 计划 KTD11；默认 0 与既有文档/DB 默认值矛盾 | 0.92 |
| KTD6 | 预算如何进 dispatcher | 调用链加参数透传；bridge trait 新增 `slot_retry_budget()` 访问器 | bridge trait 访问器（对称 `max_concurrent_workers()`） | E10, E11, E19 | 参数透传要改 `execute_wave_via_supervisor(_with_executor)` 签名与全部测试调用点；trait 方法由编译器驱动补全，无遗漏面 | 0.88 |
| KTD7 | 自动重试执行形态 | harvest 重 spawn（JoinSet 重入）；task 内 attempt 循环 | **task 内 attempt 循环**：`execute → classify → 可重试且预算未尽 → 同 task 再 execute` | E13, E14, E15, E18-U5（"不要先进 Failed 中间态再回滚"） | harvest 重 spawn 需要新 `DispatchOutcome` 变体 + store slot 状态回退 + tracker 回滚，三层改动；task 内循环对 harvest/store/tracker 全透明 | 0.87 |
| KTD8 | 重试决策用哪个 reason | 动态原文；冻结静态码（`SlotOutcome::Failed.reason` / `ClassifiedReason::Static`） | 冻结静态码 | E8 + `dispatcher.rs:5330-5333`（Static 即冻结码）；动态原文不在白名单永不重试（fail-closed） | 用动态原文会把 operator 文案参与逻辑判定，脆弱且违背白名单设计 | 0.90 |
| KTD9 | 重试尝试的事件批次处置 | salvage 中间批次；只取最终批次 | 只取最终批次进 `record_slot_result`/fingerprint | E13（record 在循环外只跑一次）；中间批次本就不入 main ledger（只在 outcome vec 内） | salvage 中间批次会改变 dedup/fingerprint 语义且无下游消费者 | 0.86 |
| KTD10 | budget>2 校验位置 | serde 自定义 deserializer；runner bridge 构造点拒绝 | runner bridge 构造点（`runner.rs:764-771`）启动期 fail-closed | E11 + E12（store 二次拦截仍在） | serde 层无既有模式（`repair_budget` 等均无范围校验）；runner 点有 fail-closed 先例 | 0.85 |
| KTD11 | preset opt-in 范围 | 不改 preset；ce-executor-supervisor worker/fix-worker 加 `startup_grace_secs: 300` | 只改 ce-executor-supervisor 两个 hat | E17 + 该 preset idle=120（KTD7）正是裸奔区最大者；parallel-forge 已被 0eb29e01 覆盖 | 给 parallel-forge 再加 startup_grace<600 反而缩短其容忍窗，是回退 | 0.85 |

---

## 4. BDD 行为规格

```gherkin
Feature: Wave worker startup grace（F1）

  Background:
    Given hat 配置 `idle_heartbeat_secs: 120`（idle 模式启用）
    And wave worker 走 PTY 双时钟路径

  Scenario S1: 首信号前静默未超 startup_grace 不杀
    Given hat 配置 `startup_grace_secs: 600`
    When worker spawn 后 150 秒无任何 stdout 行
    Then worker 仍然存活
    And 未发生任何 kill

  Scenario S2: 首信号前静默超过 startup_grace 触发 startup_kill
    Given hat 配置 `startup_grace_secs: 2`
    When worker spawn 后一直无 stdout 行
    Then worker 在约 2 秒后被杀
    And Err reason 以 "Worker timed out after" 开头且含 "startup_kill"
    And 下游 classify_slot_result 归类冻结码 "worker_timeout"

  Scenario S3: 首个合格信号后恢复 idle 语义
    Given hat 配置 `startup_grace_secs: 600`
    When worker 在 spawn 后 150 秒产出第一条 Weak 行
    And 此后静默超过 `idle_heartbeat_secs`
    Then worker 被 idle_kill（非 startup_kill）

  Scenario S4: 未配置 startup_grace_secs 行为与现状逐位一致
    Given hat 未配置 `startup_grace_secs`
    When worker spawn 后静默超过 `idle_heartbeat_secs`
    Then 按现状 idle_kill（窗口自 spawn 起算，reason 含 "idle_kill"）

  Scenario S5: None-classified 行不结束 startup grace
    Given hat 配置 `startup_grace_secs: 600` 且 backend 为 StreamJson
    When worker 先输出一行 `{"type":"system","subtype":"init"}`（None 行）
    And 此后持续静默
    Then 静默窗口仍按 startup grace 计（首合格信号未到）

  Scenario S6: events-file 增长作为 Strong 结束 startup grace
    Given hat 配置 `startup_grace_secs: 600`
    When worker 无 stdout 行但 `RALPH_EVENTS_FILE` 在 grace 内增长
    Then startup grace 结束，进入 idle 语义

Feature: Slot 自动重试（F2）

  Background:
    Given supervisor 启用且 wave 已注册
    And bridge 的 `slot_retry_budget()` 为 1

  Scenario S7: 可重试失败自动重派一次并成功
    Given slot 0 第一次 attempt 以 worker_timeout 失败
    When task 内重试决策执行
    Then 不调用 record_slot_failure
    And 同 task 内以同一 slot 身份第二次 execute
    And 第二次成功后走 record_slot_result，slot 为 Completed

  Scenario S8: 预算耗尽后永久失败
    Given slot 0 连续两次 attempt 均以 worker_timeout 失败
    When 第二次分类完成
    Then 调用 record_slot_failure("worker_timeout")
    And wave 失败路径与现状一致（含 redrive_slots 含 slot 0）

  Scenario S9: budget=0 关闭自动重试
    Given bridge 的 `slot_retry_budget()` 为 0
    When slot 0 以 worker_timeout 失败
    Then 直接 record_slot_failure，不发生第二次 execute

  Scenario S10: 非可重试失败不重派
    When slot 0 以 worker_cancelled（或未知动态 reason）失败
    Then 直接 record_slot_failure，不发生第二次 execute

  Scenario S11: budget 配置越界启动期拒绝
    Given YAML `event_loop.supervisor.slot_retry_budget: 3`
    When loop 启动构造 supervisor bridge
    Then 启动失败且错误信息指明 budget 允许范围 0..=2

  Scenario S12: 重试尝试的事件批次不进入 record
    Given slot 0 第一次 attempt 产出部分事件但以 missing_worker_terminal 失败
    When 第二次 attempt 成功
    Then record_slot_result 的 fingerprint 只覆盖第二次事件批次

  Scenario S13: 两次注册点预算一致
    Given bridge 的 `slot_retry_budget()` 为 2
    When dispatch 路径与 fan-in 路径分别 register_wave_if_absent
    Then 两次注册携带相同 budget，不触发 store 不一致错误
```

---

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐层级 | 风险补充 | E2E |
|---|---|---|---|---|---|
| S1 | 150s 静默存活（grace 600） | `tests/wave.rs` 新增 PTY 集成测试（fake executable sleep） | 集成 | 时序边界（grace±1s） | 否 |
| S2 | grace=2 零输出被杀 + reason 前缀 + startup_kill 标记 + 归类 worker_timeout | `tests/wave.rs` 集成 + `heartbeat.rs` 纯函数表 | 集成+单元 | 归因字符串断言 | 否 |
| S3 | 首 Weak 后按 idle 杀 | `tests/wave.rs` 集成（先 echo 一行 text 再 sleep） | 集成 | — | 否 |
| S4 | 未配置 = 现状 | 既有 `test_run_wave_worker_pty_idle_kill_on_silence` 等全部旧测试不改即绿 | 回归 | Characterization | 否 |
| S5 | None 行不结束 grace | `heartbeat.rs` 纯函数表（`classify_heartbeat_line` None + tick 序列） | 单元 | — | 否 |
| S6 | events-file 增长结束 grace | `tests/wave.rs` 集成（复用既有 events-ticker fixture 模式） | 集成 | — | 否 |
| S7 | 失败一次→自动重派→成功 | `tests/wave_supervisor.rs` 注入式 executor（第一次 timeout Err、第二次 Ok(done)） | 集成 | 状态机 | 否 |
| S8 | 两次失败→record_slot_failure + redrive_slots 含该 slot | `tests/wave_supervisor.rs` | 集成 | — | 否 |
| S9 | budget=0 不重派 | `tests/wave_supervisor.rs` | 集成 | — | 否 |
| S10 | cancelled/未知不重派 | `tests/wave_supervisor.rs` + `worker_outcome.rs` 表（已有，补缺失行） | 集成+单元 | Mutation（删掉白名单行必失败） | 否 |
| S11 | budget=3 启动拒绝 | `runner.rs` 邻近测试或 `loop_config.rs`/bridge 构造测试 | 单元 | Fault injection | 否 |
| S12 | fingerprint 只算最终批次 | `tests/wave_supervisor.rs`（断言 record_slot_result 的 content_hash/event_count） | 集成 | — | 否 |
| S13 | 两注册点同值 | `dispatcher.rs` 内联测试或 `wave_supervisor.rs`（stub bridge 记录两次注册参数） | 集成 | — | 否 |

层级选择理由：F1 语义核心是纯函数（单元），端到端时序用已有 fake-executable PTY 模式（集成，非 E2E）；F2 决策核心已被 `retry_classifier_tests.rs` 覆盖（单元），闭环用注入 executor 的既有 supervisor 集成模式，无需 E2E。

---

## 6. 需求—测试追踪矩阵

| Req | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约 | Evidence | Unit |
|---|---|---|---|---|---|---|---|
| R1 | `startup_grace_secs` hat 字段，None/0=关闭 | S4 | 旧测试全绿 | hat.rs 解析 + wave_detection 访问器 | — | E4, E5 | U3 |
| R2 | 首信号前窗口=startup_grace | S1, S2 | PTY 集成 ×2 | heartbeat 表（StartupKill 分支） | — | E1, E2 | U1, U2 |
| R3 | 首信号后恢复 idle 语义 | S3 | PTY 集成 | heartbeat 表（seen_first_signal 迁移） | — | E2 | U1, U2 |
| R4 | None 行不结束 grace | S5 | — | heartbeat 表 | — | E2 | U1 |
| R5 | events-file 增长=Strong 结束 grace | S6 | PTY 集成 | — | — | E2 | U2 |
| R6 | startup_kill 归因入 worker_timeout family | S2 | PTY 集成 reason 断言 + classify 断言 | heartbeat Display 表 | — | E3 | U2 |
| R7 | idle 禁用则 startup_grace 无效 | S4 变体 | PTY 集成（idle=None + grace=2 → 仅硬顶生效） | — | — | E1 | U2 |
| R8 | `slot_retry_budget` 默认 1 / 0..=2 / >2 拒绝 | S11 | 构造拒绝测试 | loop_config 解析 + 默认值 | — | E9, E11, E12 | U4 |
| R9 | 可重试失败自动重派（同 wave 同 slot） | S7 | wave_supervisor 集成 | — | — | E8, E13, E15 | U5 |
| R10 | 预算耗尽=现状失败路径 | S8 | wave_supervisor 集成（含 redrive_slots） | — | — | E13 | U5 |
| R11 | budget=0 关闭 | S9 | wave_supervisor 集成 | — | — | E12 | U5 |
| R12 | 非白名单/未知不重试 | S10 | wave_supervisor 集成 + 分类器表 | retry_classifier_tests 补行 | — | E8 | U5 |
| R13 | 无 Failed 中间态回滚 | S7, S12 | 集成（stub store 断言无 record_slot_failure 前置调用） | — | — | E13, E18 | U5 |
| R14 | 两注册点预算同值 | S13 | stub bridge 参数记录断言 | — | — | E10, E12 | U4 |
| R15 | preset opt-in 生效 | — | strict-lint 三件套 | — | — | E17 | U6 |
| R16 | 文档同步 | — | check-cli-doc-drift.sh | — | — | E20 | U7 |

---

## 7. 严格串行开发单元

```text
U1（Lease 纯函数扩展）
  ↓
U2（worker 双时钟接线 + PTY 集成测试）
  ↓
U3（hat 配置链：HatConfig → DetectedWave → dispatcher → WorkerRequest）
  ↓
U4（SupervisorConfig.slot_retry_budget + bridge 访问器 + 两注册点替换）
  ↓
U5（dispatcher task 内自动重试闭环）
  ↓
U6（ce-executor-supervisor preset opt-in）
  ↓
U7（文档同步 + 全量回归）
```

### U1：Lease 纯函数扩展（startup grace 决策核心）

1. **Unit 目标：** `decide_lease` 在「未见过首个合格信号且配置了 startup_grace」时使用独立窗口并产出 `StartupKill` 决策。
2. **对应：** R2/R3/R4；S1/S2/S3/S5；KTD1/KTD2/KTD4；E1/E2。
3. **外部可观察结果：** 暂无（纯内部能力，U2 接线后可观察）。
4. **当前行为基线：** `decide_lease` 只有 HardKill/IdleKill/Continue（`heartbeat.rs:118-131`）；`LeaseState` 无首信号标记（`heartbeat.rs:222-298`）。
5. **输入与输出：** `LeaseConfig` 增 `startup_grace_ms: Option<u64>`；`LeaseSnapshot` 增 `seen_first_signal: bool`；`LeaseState` 增 `seen_first_signal: bool`（`fresh` 为 false）；输出新增 `LeaseDecision::StartupKill`。
6. **修改位置：** `crates/ralph-cli/src/loop_runner/wave/heartbeat.rs`（结构、决策函数、tick、Display、内联表驱动测试）。不改任何其他文件。
7. **可依赖能力：** 现有纯函数与表驱动测试框架（`heartbeat.rs:737-1063`）。
8. **禁止依赖的未来能力：** 不接 worker.rs、不接配置（U2/U3 的事）；`LeaseConfig` 新字段先由测试直接构造。
9. **验收测试（表驱动新增行）：**
   - 未见首信号 + startup_grace 未到期 → Continue（即使已超过 idle_window）。
   - 未见首信号 + 到达 startup_grace 边界 → StartupKill（`>=` 边界，对齐 idle 的边界语义）。
   - 未见首信号 + `startup_grace_ms=None` → 现状 idle 语义（回归行）。
   - Strong/Weak tick 后 `seen_first_signal=true` → 后续按 idle_window 判定（IdleKill 恢复）。
   - None tick 不置位 `seen_first_signal`。
   - 未见首信号但 `now_ms >= hard_cap_ms` → HardKill（硬顶仍最高优先）。
   - `Display`：`startup_kill` 稳定小写串。
   - 运行命令：`cargo nextest run -p ralph-cli --bin ralph -- heartbeat`。
10. **Acceptance Red：** 新增测试引用 `LeaseDecision::StartupKill` / `startup_grace_ms` —— 编译失败（字段与变体不存在），即本仓 Rust TDD 约定的有效 Red（同 006 计划「Red 预期：函数不存在」）。无效 Red：fixture 路径错误、无关测试失败。
11. **单元测试拆分：** 即第 9 项七组行；每组独立 #[test] 或表行。
12. **Red → Green → Refactor 顺序：** StartupKill 变体+字段 Red → 编译 Green（空分支）→ 逐行决策表 Red → decide_lease 分支实现 Green → tick 置位迁移 Red → Green → Refactor（去重 deadline 计算）。
13. **最小实现范围：** 仅上述结构与分支；保持既有断言逐位不变（旧表全绿是回归门）。不实现：worker 接线、配置解析、reason 字符串。
14. **集成验证：** 本 Unit 无跨模块集成；`cargo nextest run -p ralph-cli --bin ralph -- heartbeat` 全绿。
15. **风险驱动测试：** 边界 Mutation（`>` vs `>=` 与 idle 边界一致性，对照 `heartbeat.rs:822-826` 既有边界行）。
16. **回归范围：** `cargo nextest run -p ralph-cli --bin ralph -- heartbeat`（纯函数全部旧行）。
17. **预期文件变更：** `heartbeat.rs`（修改+内联测试）。Evidence：E2。
18. **完成标准：** 新表行全绿 + 旧表行全绿 + build/clippy 绿 + 可独立提交。
19. **停止条件：** 发现 `decide_lease` 已有其他调用方对新变体不兼容（grep `LeaseDecision` 全用点确认 worker.rs 是唯一 match 消费方，新增变体需其配套处理——若存在第二消费方，停并重评 KTD4）。
20. **风险与注意：** 新变体使 worker.rs 既有 `match decision` 出现非穷尽——这正是 U2 要处理的接线点；本 Unit 允许 worker.rs 暂时编译失败？**不允许**。本 Unit 必须同时在 worker.rs 的三个 match 臂加 `StartupKill =>` 占位（与 IdleKill 同臂处理，行为等价 idle，保证编译与旧测试绿），U2 再把该臂改为独立归因。

### U2：worker 双时钟接线 + startup_kill 归因

1. **Unit 目标：** 双时钟循环把 `startup_grace_ms` 接入 LeaseConfig；StartupKill 产生独立归因字符串并复用 `worker_timeout` family 前缀。
2. **对应：** R2/R3/R5/R6/R7；S1/S2/S3/S4/S6/S7(无)；KTD1/KTD3；E1/E3/E16。
3. **外部可观察结果：** 配置 startup_grace 的 worker（U3 之前由测试直接传参）零输出静默在 grace 内不被杀、超 grace 被杀且 reason 含 `startup_kill`。
4. **当前行为基线：** `worker.rs:245-254` LeaseConfig 无 startup 字段；kill 分支只有 Hard/Idle（`worker.rs:337-362, 386-403`）；归因仅 hard/idle 两族（`worker.rs:546-558`）。
5. **输入与输出：** `run_wave_worker` / `run_wave_worker_pty` 新增参数 `startup_grace: Option<Duration>`（跟在 `idle_heartbeat` 后）；输出 Err reason 新族 `"{WORKER_TIMEOUT_ERR_PREFIX} {n}s of startup grace (worker_timeout/startup_kill, no first signal)"`。
6. **修改位置：** `crates/ralph-cli/src/loop_runner/wave/worker.rs`（lease_cfg 构建、compute_next_deadline、三个 match 臂、kill-kind 带出、归因字符串构建）；`crates/ralph-cli/src/loop_runner/tests/wave.rs`（集成测试）；`dispatcher.rs` 两处 `run_wave_worker` 调用点（暂时传 `None`，U3 再接真值——保证本 Unit 编译闭合）。
7. **可依赖能力：** U1 的 `StartupKill` 与 `seen_first_signal`；events-file ticker 已 tick Strong（`worker.rs:441`），自动置位首信号。
8. **禁止依赖的未来能力：** 不读 hat 配置、不动 `DetectedWave`（U3）；不改 dispatcher 注册逻辑（U4/U5）。
9. **验收测试（`tests/wave.rs`，`#[cfg(unix)]` + fake executable，模式对齐 `test_run_wave_worker_pty_idle_kill_on_silence`）：**
   - `test_run_wave_worker_pty_startup_grace_survives_idle_window`：grace=8s、idle=2s、backend 静默 4s 后输出一行 Text 再正常退出 → 不被杀，成功返回。
   - `test_run_wave_worker_pty_startup_grace_exceeded_kills`：grace=2s、idle=60s、backend 永久静默 → 约 2s 被杀；reason 前缀 `Worker timed out after` 且含 `startup_kill`。
   - `test_run_wave_worker_pty_startup_grace_then_idle_semantics`：grace=8s、idle=2s、backend 1s 时输出一行 Text 后静默 → 约 3s 被 idle_kill（reason 含 `idle_kill` 而非 `startup_kill`）。
   - `test_run_wave_worker_pty_startup_grace_disabled_legacy`：startup_grace=None（现状调用形参）→ 旧行为（复用既有测试即绿，无需新增）。
   - `test_run_wave_worker_pty_startup_grace_ignored_when_idle_disabled`：idle=None + grace=2s + 静默 → 只受硬顶约束（KTD1）。
   - 运行命令：`cargo nextest run -p ralph-cli --bin ralph -- startup_grace`。
10. **Acceptance Red：** 新测试编译失败（`run_wave_worker_pty` 参数不存在 / `startup_grace_ms` 未接线）；或接线前语义失败（worker 在 idle=2s 被杀而非活到 4s）。无效 Red：PTY fixture 本身错误。
11. **单元测试拆分：** U1 已覆盖决策层；本 Unit 只需对「kill-kind 归因映射」加一个纯映射测试（Hard/Idle/Startup → reason 子串）。
12. **Red → Green → Refactor 顺序：** S2 Red → lease_cfg/臂接线 Green → S1 Red → deadline 计算修正 Green → S3 Red → 首信号置位确认 Green → S7(idle 禁用) Red → 条件短路 Green → Refactor（三个相同 kill 臂去重为单函数）。
13. **最小实现范围：** 仅上述；保留 U1 占位臂的编译闭合；归因字符串按 KTD3 定稿。不实现：配置链、dispatcher 真值传递、重试。
14. **集成验证：** `cargo nextest run -p ralph-cli --bin ralph -- wave`（wave worker 全部新旧集成测试）。
15. **风险驱动测试：** 时序边界（grace±0.5s 容差断言，对齐既有 `duration <= 8s` 风格）；Differential（startup_grace=None 时与基线行为一致——旧测试即差分）。
16. **回归范围：** `cargo nextest run -p ralph-cli --bin ralph -- wave`（含 006 计划全部 idle 测试）；`cargo nextest run -p ralph-cli --bin ralph -- heartbeat`。
17. **预期文件变更：** `worker.rs`（修改）、`tests/wave.rs`（新增测试）、`dispatcher.rs`（两个调用点传 `None` 占位）。Evidence：E1/E3/E6/E7/E16。
18. **完成标准：** 新增集成全绿 + wave/heartbeat 回归全绿 + build/clippy 绿 + 可独立提交。
19. **停止条件：** events-file ticker 的 Strong tick 不能置位 `seen_first_signal`（说明 ticker 绕过了 `LeaseState::tick`——停并回 U1 修订）；`run_wave_worker` 存在第三个未盘点调用方（grep 后超出预期，停并补调查）。
20. **风险与注意：** PTY 时序 flake——fake backend 用短秒数 + 断言容差，遵守既有测试的 `#[cfg(unix)]` 门；`partial_timeout_events_visible` 相关两阶段隔离不受影响（不改其路径）。

### U3：hat 配置链（`startup_grace_secs` 全链路）

1. **Unit 目标：** preset YAML 的 `hats.<id>.startup_grace_secs` 能进入运行时 worker（两条 dispatch 路径）。
2. **对应：** R1；S4；KTD1；E4/E5/E6/E7。
3. **外部可观察结果：** preset 配置后 worker 按 grace 行为（U2 集成测试改为经由 WorkerRequest 传参即证明链路通）。
4. **当前行为基线：** `HatConfig` 三字段（`hat.rs:450/469/480`）；`DetectedWave::idle_heartbeat_secs()`（`wave_detection.rs:108-122`）；两路径解析（`dispatcher.rs:1291-1294, 1611-1614`）；`WorkerRequest.idle_heartbeat`（`dispatcher.rs:172`）；U2 占位传 `None`。
5. **输入与输出：** YAML `startup_grace_secs: <u32>`；`DetectedWave::startup_grace_secs() -> Option<u32>`（`Some(0)`→`None`）；`WorkerRequest.startup_grace: Option<Duration>`。
6. **修改位置：** `hat.rs`（字段+文档注释+解析测试）、`wave_detection.rs`（访问器+表测试）、`dispatcher.rs`（两路径解析 + WorkerRequest 字段 + 两 executor 调用点传真值，替换 U2 占位）、`doctor.rs:1063` / `hat.rs:671,1007` / `tests/legacy.rs:3640` / `tests/wave.rs:209`（HatConfig 字面量补字段，编译器驱动）。
7. **可依赖能力：** U2 的 worker 参数。
8. **禁止依赖的未来能力：** 不改任何 preset 文件（U6）；不接 supervisor budget（U4）。
9. **验收测试：**
   - `hat.rs`：`startup_grace_secs: 300` 解析往返；缺省 None；`0` 保留为 Some(0)（与 idle 字段同构，对照 `hat.rs:924-967` 既有模式）。
   - `wave_detection.rs`：访问器表（None→None、Some(0)→None、Some(300)→Some(300)）。
   - 链路：把一个 U2 集成测试改为构造带 `startup_grace` 的 `WorkerRequest`（而非直接调 `run_wave_worker_pty`），证明字段到达 worker。
   - 命令：`cargo nextest run -p ralph-core -- hat` + `cargo nextest run -p ralph-core -- wave_detection` + `cargo nextest run -p ralph-cli --bin ralph -- startup_grace`。
10. **Acceptance Red：** `DetectedWave::startup_grace_secs()` 引用编译失败；YAML 字段解析测试断言失败（unknown field 被忽略 → None）。
11. **单元测试拆分：** 解析往返 ×3、访问器表 ×3、WorkerRequest 字段传递 ×1。
12. **Red → Green → Refactor 顺序：** hat 字段 Red→Green → 访问器 Red→Green → dispatcher 两路径 Red→Green → 字面量站点编译修复 → Refactor（两路径解析去重为一个 helper）。
13. **最小实现范围：** 仅字段与传递；不改 preset；不动 clap/CLI。
14. **集成验证：** `cargo nextest run -p ralph-cli --bin ralph -- wave` + `cargo nextest run -p ralph-core -- wave_detection`。
15. **风险驱动测试：** 无新增（配置解析属低风险的既定模式）。
16. **回归范围：** `cargo nextest run -p ralph-core -- config` + `cargo nextest run -p ralph-cli --bin ralph -- doctor`（doctor.rs 字面量）+ `cargo nextest run -p ralph-cli --bin ralph -- presets`（结构化 parity 不应受影响）。
17. **预期文件变更：** `hat.rs`、`wave_detection.rs`、`dispatcher.rs`、`doctor.rs`、`tests/legacy.rs`、`tests/wave.rs`。Evidence：E4/E5/E6/E7。
18. **完成标准：** 上述测试全绿 + `cargo build --workspace` 绿 + clippy 绿 + 可独立提交。
19. **停止条件：** `HatConfig` 存在未知 deny 语义或 preset_lint 对 hat 新字段报警（停并查 `preset_lint` hat 检查面）。
20. **风险与注意：** 字面量站点漏改会编译失败——这是期望的编译器驱动行为，逐一补 `startup_grace_secs: None`（测试 fixture 不涉及语义）。

### U4：`slot_retry_budget` 配置 + bridge 访问器 + 注册点替换

1. **Unit 目标：** `event_loop.supervisor.slot_retry_budget`（默认 1，0..=2）经 bridge 访问器进入两处 wave 注册点，替换硬编码 `1`。
2. **对应：** R8/R14；S11/S13；KTD5/KTD6/KTD10；E8-E12/E19。
3. **外部可观察结果：** YAML 配置 2 时 store 中 wave 行 budget=2；配置 3 时 loop 启动失败并报范围错误。
4. **当前行为基线：** `SupervisorConfig` 无字段（E9）；硬编码 `1`（E10）；store 校验 0..=2 与不一致报错（E12）；bridge `max_concurrent_workers()` 访问器模式（E19）。
5. **输入与输出：** YAML 字段；`SupervisorBridge::slot_retry_budget() -> u32`；两注册点传该值；启动期越界错误。
6. **修改位置：** `loop_config.rs`（字段+default+Default impl+解析测试）、`ralph-core/src/supervisor/bridge.rs`（trait 方法+impl）、`ralph-cli/src/loop_runner/wave/supervisor_bridge.rs`（impl）、`runner.rs`（构造传参+越界拒绝）、`dispatcher.rs`（两注册点替换）、测试 stub（`dispatcher.rs` 内联 stub、`tests/wave_supervisor.rs` stub，编译器驱动补 `slot_retry_budget()`）、`tests/wave_supervisor.rs:297,326` 等 `SupervisorConfig` 字面量补字段。
7. **可依赖能力：** store 既有校验（E12）作为第二道闸。
8. **禁止依赖的未来能力：** 不做任何重试行为（U5）；不改 preset YAML。
9. **验收测试：**
   - `loop_config.rs`：缺省=1、0、2 解析；结构体字面量同步。
   - bridge：`slot_retry_budget()` 返回构造值（memory/rusqlite bridge + stub）。
   - 注册一致性（S13）：stub bridge 记录两次 `register_wave_if_absent` 的 budget 实参，断言相同且等于配置值。
   - 越界（S11）：构造 budget=3 的配置 → runner bridge 构造点返回 Err 且信息含 `0..=2`。
   - 命令：`cargo nextest run -p ralph-core -- supervisor` + `cargo nextest run -p ralph-cli --bin ralph -- supervisor`。
10. **Acceptance Red：** `slot_retry_budget()` 方法不存在编译失败；S11 测试在拒绝逻辑前表现为「构造成功」断言失败。
11. **单元测试拆分：** 默认值 ×1、合法边界 ×2、访问器 ×1、注册同值 ×1、越界拒绝 ×1。
12. **Red → Green → Refactor 顺序：** 配置字段 Red→Green → trait+impl Red→Green → runner 传参+越界 Red→Green → 两注册点替换 Red(S13)→Green → Refactor。
13. **最小实现范围：** 仅字段/访问器/替换/拒绝；不动任何 dispatch 行为语义（budget 此时仍无人消费，行为=现状）。
14. **集成验证：** `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`（stub 全绿即证明 trait 扩展无行为回归）。
15. **风险驱动测试：** Contract（两注册点同值——防 E12 的不一致报错在生产炸开）。
16. **回归范围：** `cargo nextest run -p ralph-core -- supervisor` + `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` + `cargo nextest run -p ralph-cli --bin ralph -- supervisor`。
17. **预期文件变更：** `loop_config.rs`、`supervisor/bridge.rs`（core）、`supervisor_bridge.rs`（cli）、`runner.rs`、`dispatcher.rs`、`tests/wave_supervisor.rs`。Evidence：E9-E12/E19。
18. **完成标准：** 全绿 + build/clippy 绿 + 可独立提交；行为与基线一致（差分：budget 值仍不被消费）。
19. **停止条件：** `SupervisorBridge` 存在第四处未盘点 impl（编译器暴露）；runner 构造点无法在不改公开签名下拿 budget（停并重评 KTD6）。
20. **风险与注意：** trait 新方法无默认实现——刻意让编译器列出全部 impl 站点，禁止图省事给默认 `1`（那会静默漏掉真实 impl 的接线）。

### U5：dispatcher task 内自动重试闭环

1. **Unit 目标：** supervisor worker task 在「可重试冻结码失败 + 预算未尽」时同 task 内重试，预算耗尽后走现状失败路径。
2. **对应：** R9/R10/R11/R12/R13；S7/S8/S9/S10/S12；KTD7/KTD8/KTD9；E8/E13/E14/E15/E18。
3. **外部可观察结果：** 日志可见第二次 worker spawn（warn 携带 slot_index/attempt/budget）；可重试失败自愈后 wave 正常收敛；耗尽后 `*.wave.failed` 的 `redrive_slots` 仍含该 slot。
4. **当前行为基线：** task 单次 execute → classify → record（E13）；无重试分支；`WorkerRequest` 无 Clone（待加 derive）。
5. **输入与输出：** 输入=每次 attempt 的 `WaveWorkerOutcome` + `bridge.slot_retry_budget()`；输出=最终 attempt 的分类结果（与现状同型）；副作用=重试 warn 日志。
6. **修改位置：** `dispatcher.rs`：WorkerRequest 加 `#[derive(Clone)]`（或并入既有 derive 行）；supervisor per-worker task 体（锚点：`SupervisorSlotRelease` 创建、`executor.execute(request)`、`classify_slot_result` 调用、`record_slot_failure` 于 `4830-4847`）改为 attempt 循环：「execute → classify → Failed 且 Static 冻结码在 `RETRYABLE_REASONS` 且 `attempts < budget` → attempts+=1 + warn + continue；否则出循环走现有 record/guard/projection」；`tests/wave_supervisor.rs` 集成测试。
7. **可依赖能力：** U4 的 `bridge.slot_retry_budget()`；`worker_outcome::RETRYABLE_REASONS`/`is_retryable_slot_reason`（E8）；`WorkerRequest: Clone`（E15）。
8. **禁止依赖的未来能力：** 不改 harvest/sweep/tracker（E14 保证透明）；不改 store API；不动 legacy 路径。
9. **验收测试（`tests/wave_supervisor.rs`，注入式 executor 模式对齐既有 U3CountingExecutor 风格）：**
   - S7：`timeout → done`：executor 第一次返回 Err(timeout 前缀串)、第二次返回 Ok(done 事件)；断言 spawn 计数=2、无 `record_slot_failure` 调用、`record_slot_result` 一次、slot Completed。
   - S8：`timeout → timeout`（budget=1）：spawn=2、`record_slot_failure("worker_timeout")` 一次、`build_wave_failed_payload` 的 `redrive_slots` 含该 slot。
   - S9：budget=0：spawn=1、直接 record failure。
   - S10：Err(非超时动态串)/cancelled：spawn=1。
   - S12：第一次 Ok(部分事件无 terminal)→ 第二次 Ok(done)：断言 `record_slot_result` 的 `content_hash`/`event_count` 只对应第二批次。
   - 命令：`cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`。
10. **Acceptance Red：** S7 在接线前 spawn 计数=1 且无第二次 execute（断言失败）；`bridge.slot_retry_budget()` 不存在编译失败（若 U4 未完成——顺序保证不会发生）。
11. **单元测试拆分：** 重试判定小函数（冻结码 × attempts × budget → Retry/Permanent）表驱动 ×6 行（复用/扩展 `retry_classifier_tests.rs` 的表，补 budget 维度）。
12. **Red → Green → Refactor 顺序：** 判定函数 Red→Green → S9(budget=0) Red→Green → S10 Red→Green → S7 Red→Green → S8 Red→Green → S12 Red→Green → Refactor（循环体与 record 段的边界整理）。
13. **最小实现范围：** 仅 task 内循环 + derive(Clone) + 判定函数 + warn 日志；progress_tx 只在最终 outcome 发送一次（保持 reporter 语义）；不实现跨 attempt 的事件合并（KTD9）。
14. **集成验证：** `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` 全量；`cargo nextest run -p ralph-core -- supervisor`。
15. **风险驱动测试：** 状态机（attempts 计数与 budget 边界：0/1/2）；Mutation（删 `RETRYABLE_REASONS` 任一行必有测试红）；幂等（成功后重复 record 不发生——store first-terminal-wins 已保证，`memory.rs:617-619`）。
16. **回归范围：** `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` + `cargo nextest run -p ralph-cli --bin ralph -- supervisor` + `cargo nextest run -p ralph-cli --bin ralph -- wave`（dispatch 全族）；HARD RULE 5：若新增测试 spawn `ralph`，用污染环境复跑一次。
17. **预期文件变更：** `dispatcher.rs`、`tests/wave_supervisor.rs`、`worker_outcome.rs`（仅测试表补行）。Evidence：E8/E13/E14/E15。
18. **完成标准：** S7-S12 全绿 + 回归全绿 + build/clippy 绿 + 可独立提交。
19. **停止条件：** task 体代码形状与 E13 锚点不符（找不到单一 classify→record 段）；executor trait 语义不允许同一 request 重入（如发现 execute 消费了不可 Clone 的内部状态——停并重评 KTD7）。
20. **风险与注意：** 重试占用 semaphore permit 时长翻倍——这是刻意语义（slot 仍在工作），由既有 aggregate/global deadline 兜底；测试 executor 必须按 attempt 序号返回不同结果（用 `AtomicU32` 计数）。

### U6：preset opt-in（ce-executor-supervisor）

1. **Unit 目标：** `ce-executor-supervisor` 的 `worker` / `fix-worker` 两个 hat 启用 `startup_grace_secs: 300`。
2. **对应：** R15；KTD11；E17。
3. **外部可观察结果：** 该 preset 的 wave worker 获得「120s idle 灵敏监控 + 300s 冷启动容忍」组合。
4. **当前行为基线：** `presets/en/ce-executor-supervisor.yml:911-917`（worker 120/8）、`:1335`（review-batch-worker 90）；KTD7 钉值测试（E17）。
5. **输入与输出：** 两处 YAML 行新增；无运行时新行为之外的变化。
6. **修改位置：** `presets/en/ce-executor-supervisor.yml`（worker、fix-worker 两 hat 各加一行 `startup_grace_secs: 300`）。
7. **可依赖能力：** U3 的配置链。
8. **禁止依赖的未来能力：** 不动 parallel-forge；不动 review-batch-worker；不改任何 instructions 文本。
9. **验收测试：** preset 校验三件套：`cargo nextest run -p ralph-cli --bin ralph -- preset_lint` + `cargo nextest run -p ralph-core -- preset_lint` + `cargo nextest run -p ralph-cli --bin ralph -- presets`（含 strict-lint 与 parity）。
10. **Acceptance Red：** 若 U3 未落地，YAML 新字段在 strict-lint 报 unknown field——顺序保证不会发生；本 Unit 的 Red 是「KTD7 钉值测试语义检查」：确认钉值测试不需要钉 startup 字段（若团队决定钉，则同步加断言——两者择一，Executor 不得自行决定：选择**不钉**，因为 KTD7 测试注释声明其范围是 006 计划的三个字段）。
11. **单元测试拆分：** 无新增（结构化 lint 即覆盖）。
12. **Red → Green → Refactor 顺序：** 直接 Green + 三件套验证。
13. **最小实现范围：** 两行 YAML。
14. **集成验证：** 三件套全绿。
15. **风险驱动测试：** 无。
16. **回归范围：** 三件套 + `cargo nextest run -p ralph-cli --bin ralph -- presets`。
17. **预期文件变更：** `presets/en/ce-executor-supervisor.yml`。Evidence：E17。
18. **完成标准：** 三件套全绿 + 可独立提交。
19. **停止条件：** strict-lint 对 `startup_grace_secs` 报 unknown/unexpected（说明 preset_lint 有 hat 字段白名单——停并回 U3 补 lint 白名单）。
20. **风险与注意：** schema 目录不含 hat 字段（已 grep 确认），无需同步 `presets/schemas/ce-executor-supervisor.yml`；若 lint 结果推翻此结论，触发停止条件。

### U7：文档同步 + 全量回归

1. **Unit 目标：** agent 可见文档与概念表反映两个新行为；全量测试收口。
2. **对应：** R16；KTD3/KTD5/KTD7 的对外语义；E20。
3. **外部可观察结果：** `ralph-tools-wave.md` 的「Worker 终止语义」新增 `startup_grace_secs` 段落与「可重试失败自动重派一次」说明；CONCEPTS.md 两节更新；patterns.md 双时钟段落更新。
4. **当前行为基线：** `ralph-tools-wave.md:326-334`、`CONCEPTS.md:71-85`、`patterns.md:312-326`。
5. **输入与输出：** 文档 diff；`scripts/check-cli-doc-drift.sh` 绿。
6. **修改位置：** 上述三文件。
7. **可依赖能力：** U1-U6 全部落地行为。
8. **禁止依赖的未来能力：** 无。
9. **验收测试：** `bash scripts/check-cli-doc-drift.sh`；文档行文遵守「agent 下一步能执行什么」可读性 HARD RULE（触发条件 / 字段来源 / 失败停止条件）；不得出现 plan 编号、事故路径、内部函数名（去计划化 HARD RULE）。
10. **Acceptance Red：** drift 脚本或人工对照发现文档缺新字段语义。
11. **单元测试拆分：** 无。
12. **Red → Green → Refactor 顺序：** 直接编写 + drift 验证。
13. **最小实现范围：** 三文件；`ralph-tools-wave.md` 要点：`startup_grace_secs` 仅在 idle 模式启用时生效、首个合格进度信号前生效、超时归因属 `worker_timeout` family；自动重试：worker_timeout 等可重试失败最多自动重派 `slot_retry_budget` 次、agent 应保证工作幂等可重入。
14. **集成验证：** `./scripts/run-tests.sh`（两阶段 nextest + doctest）。
15. **风险驱动测试：** 无。
16. **回归范围：** 全量 `./scripts/run-tests.sh`；flake 时按基线策略 `RALPH_BASELINE_SERIAL=1` 兜底一次，仍红即真失败。
17. **预期文件变更：** `crates/ralph-core/data/ralph-tools-wave.md`、`CONCEPTS.md`、`skills/ralph-preset-common/references/patterns.md`。Evidence：E20。
18. **完成标准：** drift 脚本绿 + 全量绿 + 可独立提交。
19. **停止条件：** drift 脚本暴露与本计划无关的既有漂移——只修本计划引入的段落，其余记录并上报，不顺手改。
20. **风险与注意：** 文档禁止泄漏内部实现细节（`decide_lease`、`worker.rs` 行号等），只写 hat 可见语义。

---

## 8. Unit 串行依赖图

```text
U1 → U2 → U3 → U4 → U5 → U6 → U7
```

- **U2 依赖 U1：** worker 的 kill 分支消费 `LeaseDecision::StartupKill`（U1 产物）；不可交换（U1 是纯函数前提）。U1 内已含 worker 占位臂，保证任何中间态编译闭合。
- **U3 依赖 U2：** dispatcher 传真值需要 worker 参数已存在（U2 产物）；U2 的 `None` 占位保证 U3 前编译闭合。
- **U4 与 U1-U3 无代码依赖，但严格串行：** 按顺序执行避免 dispatcher.rs 双特性并行改动互相干扰回归归因。
- **U5 依赖 U4：** 重试预算来源是 `bridge.slot_retry_budget()`（U4 产物）。
- **U6 依赖 U3：** preset 字段需要配置链已生效（否则 strict-lint/运行时无意义）。
- **U7 依赖全部：** 文档描述最终行为。

---

## 9. 执行命令清单

| 时机 | 命令 | 目的 | 失败是否放行 |
|---|---|---|---|
| U1 | `cargo nextest run -p ralph-cli --bin ralph -- heartbeat` | Lease 纯函数 | 不放行 |
| U2 | `cargo nextest run -p ralph-cli --bin ralph -- startup_grace` | F1 新集成 | 不放行 |
| U2 | `cargo nextest run -p ralph-cli --bin ralph -- wave` | wave worker 回归 | 不放行 |
| U3 | `cargo nextest run -p ralph-core -- hat` + `cargo nextest run -p ralph-core -- wave_detection` | 配置解析 | 不放行 |
| U3 | `cargo nextest run -p ralph-cli --bin ralph -- doctor` + `cargo nextest run -p ralph-cli --bin ralph -- presets` | 字面量/parity 回归 | 不放行 |
| U4 | `cargo nextest run -p ralph-core -- supervisor` + `cargo nextest run -p ralph-cli --bin ralph -- supervisor` | budget 配置与注册 | 不放行 |
| U5 | `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` | 重试闭环 | 不放行 |
| U6 | preset 三件套（`preset_lint`×2 + `presets`×1） | strict-lint/parity | 不放行 |
| 每 Unit | `cargo build --workspace` + `cargo clippy --workspace --all-targets` + `cargo fmt --all -- --check` | 构建/lint/格式 | 不放行 |
| U7 | `bash scripts/check-cli-doc-drift.sh` | 文档漂移 | 不放行 |
| 最终 | `./scripts/run-tests.sh` | 两阶段全量 + doctest | 不放行 |
| flake 兜底 | `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | 仅竞态 flake | serial 仍红=真失败 |

HARD RULE 5：新增会 spawn `ralph` 二进制的测试时，用 `common::ralph_bin()` / `scrub_agent_runtime_env`，并以污染环境复跑（例：`RALPH_CURRENT_HAT=executor RALPH_EVENTS_FILE=/tmp/x.jsonl cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`）。

---

## 10. 最终质量门禁

- S1-S13 全部通过；R1-R16 均可追踪到可执行测试。
- F1 关闭时（未配置 `startup_grace_secs`）行为与基线逐位一致（006 计划全部既有 idle 测试未改即绿）。
- F2 budget=0 时行为与基线一致；默认 1 时仅多一次自动重派。
- `classify_failure_reason` 结束「生产零调用」状态；两注册点预算同值。
- 无新增 skipped/ignored/`.only`；无削弱断言；无 snapshot 无解释更新。
- `cargo fmt --check`、build、clippy、targeted nextest、`./scripts/run-tests.sh` 全绿。
- 实际变更不超出第 2.3 节范围；不改 DB schema / preset 拓扑 / CLI 表面 / operator redrive 路径 / legacy 路径语义。
- 文档三处同步完成且无内部实现细节泄漏。
- 每个 Unit 形成完整 TDD 闭环并独立提交；所有 KTD 置信度 ≥0.85，无 BLOCKED。

---

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每 Unit 绑定符号级入口、Red/Green、命令与完成门 |
| Executor 是否仍需做关键设计决策 | 否 | KTD1-KTD11 锁定语义/归因/配置/执行形态；U6 钉值选择也已注明 |
| 所有文件和接口是否有代码库证据 | 是 | E1-E20 全部 file:line |
| 所有关键决策置信度是否 ≥0.85 | 是 | 最低 KTD10/KTD11=0.85 |
| 是否存在未处理的低置信度假设 | 否 | U5 task 体形状以锚点+停止条件兜底（已在 U5-19 声明） |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 纯决策 / U2 接线归因 / U3 配置链 / U4 预算通路 / U5 重试闭环 / U6 preset / U7 文档 |
| 每个 Unit 是否可以独立验证 | 是 | 各自 targeted nextest + 完成门 |
| 每个 Unit 是否有真实 Red | 是 | 编译 Red 与语义 Red 均注明；无效 Red 已排除 |
| 每个 Unit 是否包含回归范围 | 是 | 各 Unit 第 16 项 |
| 是否存在未来 Unit 依赖 | 否 | 依赖图单向；U2/U3 占位设计保证中间态编译闭合 |
| 是否存在泛化任务描述 | 否 | 全部动作绑定文件、符号、断言、命令 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 追踪矩阵 |
| 所有关键决策是否有 Evidence | 是 | KTD 表逐行引用 E 编号 |
| 计划是否可以严格串行执行 | 是 | 单链 7 Unit |
