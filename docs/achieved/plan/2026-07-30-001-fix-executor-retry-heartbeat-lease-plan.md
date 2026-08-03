---
title: "fix: Executor 经验继承重试与 Heartbeat 租约收敛"
date: 2026-07-30
type: fix
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan
execution: code
---

# Executor 经验继承重试与 Heartbeat 租约收敛 - Plan

## 0. 计划状态

- **状态：READY。** 所有实施关键决策置信度均达到 `0.85`；没有把 backend 静默退出的进程层根因伪装成已确认事实，也没有让后续 Unit 依赖该未知根因。
- **代码库基线：** 分支 `pittcat-dev`，提交 `a957ab29`（`fix(supervisor,parallel-forge): 空 salvage 提交 delivery 相位并订阅 exec.wave.failed`）。
- **调查范围：**
  - supervisor wave worker 请求构造、PTY 执行、stdout heartbeat 分类、双时钟租约；
  - dispatcher slot attempt loop、slot outcome 分类、partial/aggregate/global deadline；
  - supervisor retry budget、失败 reason、fan-in 与 `exec.wave.failed`；
  - `parallel-forge` executor/failure-handler preset、event schema、BDD scenario；
  - agent 注入指南与 preset operator references；
  - P0 事故日志、F1 completion artifact、遗留 task 状态及相关提交历史。
- **已执行的验证命令（只读调查）：**
  - `git status --short`
  - `git log -8 --oneline --decorate`
  - `rg` / `sed` 对本计划 Evidence Ledger 所列文件、符号、scenario 和日志进行静态核对；
  - 检查 `.ralph/diagnostics/logs/ralph-2026-07-30T12-32-32-599-447560.log`、F1 completion artifact 与当前 task ledger 的只读内容；
  - 检查现有 heartbeat、retry budget、fan-in、BDD 测试入口及项目规定的 nextest 命令。
- **尚未执行的验证：** 本计划阶段未运行测试、build、clippy、CLI smoke 或全量门禁；这些属于实施阶段，已固定到各 Unit 与第 9、10 节。未执行 live backend 调用，因为本功能可由 fake backend、暂停时钟和真实 runtime path 验证。
- **阻塞项：** 无。
- **工作区说明：** 调查时 `git status --short` 为空；本次只新增此计划文件，不修改生产代码或 `.ralph/` runtime 状态。
- **未知但不阻塞的事实：** P0 中 Claude PTY 子进程为什么未完成，现有证据仍无法在 token/context、backend 异常和指令理解失败之间定责。计划通过失败归一化、fresh-process retry 和原 worktree 经验继承，使这三类原因都进入同一可恢复路径；进程层诊断日志持久化不在本次范围。

---

## 1. 功能目标

### 1.1 业务目标

当 `parallel-forge` 的 executor 因 idle/hard timeout、空结果、缺失终态或主动发出 `exec.unit.failed` 而未完成 Unit 时，supervisor 在同一 slot 上最多启动两个新的 executor，使总尝试次数为三次。每个新 executor 必须继承同一 worktree 中已经留下的代码、commit、completion report 和测试证据，并收到全部既往 attempts 的有界失败摘要，避免从空白状态重复踩坑。

### 1.2 用户或调用方

- 运行 `parallel-forge` 的操作者；
- supervisor dispatcher 与 slot store；
- wave executor backend；
- `parallel-forge` 下游 reviewer、integrator 和 failure handler；
- 读取 `ralph-tools-wave` 的 loop 内 agent，以及编写/评审 preset 的操作者。

### 1.3 当前行为

1. worker PTY 已把 headless stdout 行分类为 Strong/Weak/None heartbeat；Strong 和未超 cap 的 Weak 会刷新 idle lease，hard cap 始终优先。
2. dispatcher 已有同一 task 内的 slot retry loop；`WorkerRequest::Clone` 会让 `ProductionExecutor::execute` 再次调用 `run_wave_worker`，从而启动新的 backend 进程。
3. retry 只接受四个静态 reason：`worker_timeout`、`empty_worker_result`、`missing_worker_terminal`、`slot_never_started`。
4. `exec.unit.failed` 被 `classify_worker_outcome` 归类为 `SlotOutcome::Completed(WorkerTerminalKind::Failed)`，因此不会进入 retry，并会走 `record_slot_result`。
5. retry 复用完全相同的 prompt；没有 attempt 编号、失败 reason 或强制检查现有 worktree 证据的恢复上下文。
6. retry loop 在 partial deadline（aggregate 的 80%）或 aggregate deadline 到达时停止重试；dispatcher 当前又在 partial deadline 直接 abort 并返回 `AggregateDeadlineExceeded`。
7. `parallel-forge` 未显式配置 `slot_retry_budget`，因此使用默认值 `1`，总计最多两次尝试。
8. `a957ab29` 已使最终 `exec.wave.failed` 能进入 failure handler，但 handler 看到的是 fan-in 终态；它不能替代 slot 内 fresh executor retry。

### 1.4 目标行为

1. 仅对 supervisor 的 `WaveKind::Exec`，`exec.unit.failed` 视为一次失败尝试，而不是成功完成的 slot。
2. `slot_retry_budget: 2` 表示初始 attempt + 两次 redispatch，总计最多三次。
3. 每次 redispatch 都调用新的 backend 进程，但保留同一 `cwd`、slot binding、branch、worker identity 和事件权限边界。
4. attempt 2/3 的 prompt 包含：
   - 当前 attempt / 最大 attempts；
   - 按 attempt 顺序排列的全部既往失败摘要（最多两项），每项含稳定失败码；
   - 若存在，该 attempt 的 `exec.unit.failed.payload.reason` 非空、限长文本；
   - 同一 worktree 续做声明；
   - 先检查 `git status`、最近 commit、既有 completion report 和既有测试证据，再决定最小增量动作；
   - 禁止 reset/clean/覆盖既有成果，禁止假装重新从零执行。
5. 中间 attempt 的业务事件不得合并进主 ledger、不得更新 task、不得触发 reviewer/integrator；只有最终成功 attempt 的 `exec.unit.done` 可进入现有 fan-in。
6. 三次都失败后，attempt loop 必须先把最终主动失败 event batch 规范化为稳定 Failed outcome 并丢弃该 batch，再让 slot 进入 Failed；fan-in 随后只生成一次 `exec.wave.failed`。payload 保留最终稳定 reason，`redrive_slots` 仅描述已耗尽自动重试后的 operator recovery 候选。
7. stdout heartbeat 保持现有定义：Strong 和 cap 内 Weak 刷新 idle lease，None 不刷新，hard cap不刷新。aggregate 预算必须按并发批次数与最大 attempts 提供足够下限，避免 wave partial deadline 在合法 attempt hard cap 之前杀掉 lease-healthy worker。
8. runner `max_runtime_seconds` 仍是绝对全局上限；到达全局上限时允许中断当前 wave，不为完成第三次尝试突破操作者设置的全局边界。

### 1.5 行为差异

| 条件 | 当前行为 | 目标行为 |
|---|---|---|
| `exec.unit.failed` | slot 被记为 Completed，不自动重试 | 记为失败 attempt；预算内启动 fresh executor |
| timeout / empty / missing terminal | 最多默认重试 1 次 | `parallel-forge` 最多重试 2 次，总 attempts=3 |
| retry prompt | 与第一次完全相同 | 带 attempt、失败原因和原 worktree 续做协议 |
| retry 进程 | 代码上会再次执行 backend，但无进程级验收 | fake backend 验收三个不同 PID、同一 cwd |
| heartbeat | 已刷新 idle lease | 保持；增加跨 dispatcher aggregate 的验收保护 |
| wave deadline | partial=80% 时直接 abort | supervisor effective aggregate 取配置值与安全下限的较大值，使 partial 晚于合法 attempts 的最坏执行预算 |
| 最终失败 | 可产生 `exec.wave.failed` | 仅三次耗尽后产生一次；中间失败不可泄漏 |

### 1.6 本次范围

- supervisor Exec slot 的主动失败自动重试；
- retry context 的结构、构造和 prompt 注入；
- retry attempt/final outcome 的分类与持久化边界；
- aggregate timeout 安全下限计算；
- `parallel-forge` retry budget、instructions/schema 语义同步；
- 单元、集成、真实 EventLoop BDD、agent guide 和 operator skill 同步。

### 1.7 非目标

- 不判定 P0 Claude 子进程的唯一进程层 root cause；
- 不持久化完整 PTY stdout，不新增 hat stdout diagnostics 文件；
- 不自动 cherry-pick F1 commit，不清理当前 `.ralph/` tasks/worktrees；
- 不改 Claude/Gemini/Codex adapter 的协议或 token/context 策略；
- 不新增独立 retry service、后台队列或新 CLI；
- 不允许无限重试；
- 不把“worktree 有 commit/completion”自动提升为 `exec.unit.done`；
- 不改变 Review/Fix wave 的 failed-terminal 语义；
- 不改变 runner 全局 `max_runtime_seconds` 的最高优先级。

### 1.8 输入

- `exec.unit.ready` wave event 和原 slot binding；
- worker outcome：accepted events、exit/timed-out 状态、duration；
- `exec.unit.failed` JSON payload 中的 `reason`；
- `SupervisorConfig.slot_retry_budget`；
- hat `timeout`、`idle_heartbeat_secs`、`idle_weak_signal_cap`、`startup_grace_secs`；
- wave event 数、有效 concurrency、配置的 aggregate timeout；
- runner 传入的 global deadline。

### 1.9 输出

- 成功：仅最终 attempt 的 `exec.unit.done` event batch，沿用现有 `exec.wave.complete`；
- 耗尽：一次 slot failure 和一次 fan-in `exec.wave.failed`；
- retry prompt 中的结构化 `Retry Context`；
- tracing 中可辨识的 attempt、max attempts、稳定 failure code；不得记录未脱敏的完整 stdout。

### 1.10 状态变化

- attempt 1/2 失败但预算未耗尽：supervisor slot 保持 in-flight，不写 Completed/Failed 终态；
- attempt 成功：slot 写 Completed 与 terminal evidence；
- attempt 3 失败：slot 写 Failed，release guard 以 Failed 释放；
- 中间 worker event file 由现有 worker 读取和删除，不合并主 ledger；
- task 状态仍只由后续 `forge.wave.settled` 的 `CloseTaskBatch` 改变。

### 1.11 错误语义

- 新增计划常量 `executor_reported_failure`：仅表示 Exec worker 已接受并发出 `exec.unit.failed`；它是自动重试允许项，也是耗尽后 `slot_failures[].reason` 的稳定码。
- `exec.unit.failed.payload.reason` 是诊断详情，只进入下一 attempt prompt 和 bounded tracing；不得替代稳定分类码、不得直接作为 store reason。
- payload 非 JSON、缺少 reason 或 reason 为空时，仍以 `executor_reported_failure` 重试，但 retry context 的 detail 使用固定 `unavailable`，不得猜测。
- timeout/idle/startup 继续归入 `worker_timeout` family。
- 非白名单 failure、取消、identity mismatch、control-plane path 错误不因本计划变成可重试。

### 1.12 兼容性要求

- `slot_retry_budget=0` 保持零重试；
- 默认值仍为 `1`，仅 `parallel-forge` 显式设为 `2`；
- legacy 非-supervisor dispatcher 不启用新 Exec failed-terminal 重试；
- Review/Fix wave 继续使用现有 truth table；
- `exec.unit.done/failed` schema 的既有 required fields 不删除；
- `exec.wave.failed` 现有 consumer 保持可用，只澄清它已代表自动 retry 耗尽。

### 1.13 性能要求

- retry context 为有界列表：最多保存前两个 attempts，每项 failure detail 去首尾空白后最多保留 1 KiB；超出时按 UTF-8 字符边界截断并标注；
- 不复制完整 stdout，不引入数据库新表或 migration；
- effective aggregate 计算为 O(1)：先计算完整工作预算，再反推能让 80% partial threshold 不早于该预算的 aggregate 下限；
- worker 并发上限不变，retry 在原 slot permit 内串行执行，不增加并发峰值；
- global runtime 仍限制最坏总耗时。

### 1.14 安全与权限要求

- 新进程继承现有 runtime 注入的 `RALPH_WAVE_*`、`RALPH_EVENTS_FILE`、hat ACL 和 worktree cwd；
- retry context 不包含凭证、环境变量值或完整 stdout；
- 不允许 agent 改写 worker channel；
- 不执行自动 reset/clean/cherry-pick；
- 中间 attempt event 不得越过 supervisor merge 边界。

### 1.15 已知约束

- worker stdout 当前只驱动 RPC/TUI/heartbeat，没有被 `WaveWorkerOutcome` 返回，因此本次无法把“上次完整对话”传给新进程；
- 可继承经验的权威来源是稳定 failure code、主动失败 reason 和同一 worktree 内的 Git/artifact/test 状态；
- global deadline 可能使三次尝试无法全部执行，这是操作者设置的绝对上限，不属于 slot retry bug；
- `parallel-forge` executor concurrency 为 4，静态 wave 可能大于 4，因此 aggregate 安全下限必须包含批次数。

### 1.16 已确认假设

- A1：再次调用 `ProductionExecutor::execute` 会启动新 PTY/backend 进程；由 E7/E8 直接支持，实施时以 PID 集成测试锁定。
- A2：同一 retry task 持有同一 `WorkerRequest.cwd` 和 supervisor slot binding；由 E6/E8/E12 支持。
- A3：主动失败 reason 可从返回的 accepted `exec.unit.failed` event payload 读取；由 E4/E13 支持。
- A4：heartbeat 已从 headless stdout 分类并刷新 idle lease；由 E9/E10/E11 支持。
- A5：真正导致 P0 的进程层原因未知；本计划不依赖该原因。

### 1.17 待验证假设

无实施关键待验证假设。以下是 Unit 内必须用测试继续证明、但已经有足够代码证据支撑决策的实现断言：

| 假设 | 为什么需要确认 | 验证方法 | 预期证据 | 失败影响 |
|---|---|---|---|---|
| V1 fake backend 可记录 PID、cwd、prompt 并按 attempt 输出不同终态 | 证明“新 executor”不是同一进程复用 | 在现有 wave integration fixture 中新增可执行 fake backend | 三个不同 PID、同 cwd、attempt prompt 递增 | 若 fixture 能力不足，停在 Unit 2，先扩展测试 adapter；不改生产设计 |
| V2 `exec.unit.failed` 中间 event 未进入主 ledger | 防止 reviewer 提前激活 | dispatcher/supervisor integration 断言主 events 文件与 store | attempt 1/2 无业务 event；final only | 若泄漏，Unit 1 不得关闭 |
| V3 effective aggregate floor 在暂停时钟下晚于所有合法 attempt hard caps | 防止 outer timeout 抢先 | helper 单测 + dispatcher paused-time integration | partial/aggregate 未提前 abort；global 仍可 preempt | 若不成立，停在 Unit 3 重算公式 |

---

## 2. 代码库现状与证据

### 2.1 当前实现入口

#### 外部入口与调用链

```text
exec.unit.ready
→ runner.rs 识别 wave events 并计算 global_deadline
→ handle_wave_events
→ execute_wave_structured
→ execute_wave_via_supervisor_with_executor
→ 构造 WorkerRequest（含 cwd / prompt / timeout / heartbeat）
→ dispatch_wave_inner_with_release
→ 同一 slot task 内 attempt loop
→ ProductionExecutor::execute
→ run_wave_worker
→ run_wave_worker_pty
→ stdout heartbeat / worker events
→ classify_slot_result
→ record_slot_result 或 record_slot_failure
→ supervisor fan-in
→ exec.wave.complete 或 exec.wave.failed
```

#### 核心模块

- `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`
  - supervisor dispatch、`WorkerRequest`、attempt loop、deadline、slot store、fan-in。
- `crates/ralph-cli/src/loop_runner/wave/worker.rs`
  - PTY spawn、stdout 读取、heartbeat lease、worker event 收集。
- `crates/ralph-cli/src/loop_runner/wave/heartbeat.rs`
  - headless stdout → Strong/Weak/None 与纯租约状态机。
- `crates/ralph-core/src/supervisor/worker_outcome.rs`
  - terminal/exit truth table、稳定 reason、retryability/failure class。
- `crates/ralph-core/src/wave_prompt.rs`
  - wave worker prompt 的单一构造入口。
- `crates/ralph-core/src/config/loop_config.rs`
  - supervisor aggregate timeout 与 slot retry budget。
- `presets/en/parallel-forge.yml`
  - runtime supervisor 配置、executor/failure-handler agent 合同。
- `presets/schemas/parallel-forge.yml`
  - `exec.unit.failed` 与 `exec.wave.failed` 公开字段语义。

#### 数据边界

- supervisor store：slot phase、result fingerprint、failure reason、terminal evidence；
- per-slot worker events JSONL：worker 进程的私有 event channel，worker 完成后读取并删除；
- unit worktree：跨 attempts 复用的 Git/文件状态；
- main events ledger：仅 fan-in 接受的最终业务事件；
- `.ralph/forge/<plan_key>/units/<unit-id>-completion.md`：executor 已有 completion report 合同。

#### 外部依赖

- `portable_pty`/backend adapter 所驱动的 headless CLI；
- Tokio task/semaphore/time；
- Git worktree；
- 本计划不新增第三方依赖。

#### 现有测试

- heartbeat 纯单测：`crates/ralph-cli/src/loop_runner/wave/heartbeat.rs`；
- PTY/heartbeat 集成测试：`crates/ralph-cli/src/loop_runner/tests/wave.rs`；
- supervisor、retry budget、fan-in 测试：`crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` 与 dispatcher 内部 tests；
- outcome truth table：`crates/ralph-core/src/supervisor/worker_outcome.rs`；
- prompt tests：`crates/ralph-core/src/wave_prompt.rs`；
- real EventLoop BDD：`crates/ralph-core/tests/scenarios.rs` + `crates/ralph-core/tests/scenarios/*.yml`。

#### 构建和验证方式

- targeted：`cargo nextest run -p <crate> -- <substring>`；
- BDD：`cargo nextest run -p ralph-core --test scenarios -- <substring>`；
- preset：项目规定的三条 nextest preset 校验；
- docs drift：`scripts/check-cli-doc-drift.sh`；
- build/lint：`cargo build`、`cargo clippy`、`cargo fmt --check`；
- 最终：`./scripts/run-tests.sh`。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `.ralph/diagnostics/logs/ralph-2026-07-30T12-32-32-599-447560.log` | P0 wave 最终进入 `SalvageNotMerged retry exhausted` / fan-in failure；没有 worker terminal | 证明外层 fail-close 发生，不能证明 Claude 进程层原因 | 高 |
| E2 | `.ralph/forge/2026-07-29-002-feat-parallel-forge-reuse-status-plan/units/F1-completion.md` | F1 completion 记录 commit `da33241d...` | 证明 worker 在失联前已留下可复用磁盘经验，禁止把 retry 设计成全新 worktree | 高 |
| E3 | `.ralph/agent/tasks.jsonl` 只读检查 | F1 等 task 仍为 open | 证明 executor output 未进入 settlement，不应由 retry 中间态关闭 task | 高 |
| E4 | `worker_outcome.rs::classify_worker_outcome` | `WorkerTerminalKind::Failed` 返回 `SlotOutcome::Completed(Failed)` | 直接解释主动 `exec.unit.failed` 为什么不进入当前 retry | 高 |
| E5 | `worker_outcome.rs::RETRYABLE_REASONS` | 只有 timeout/empty/missing/never-started 四类 | 必须显式新增 Exec 主动失败稳定码，不得用动态 reason 决定 retry | 高 |
| E6 | `dispatcher.rs::WorkerRequest::clone` | prompt、worker_events_path、cwd、timeout、backend 均被复制 | retry 可在原 slot/worktree 重入；当前缺少 retry context | 高 |
| E7 | `dispatcher.rs::ProductionExecutor::execute` | 每次执行都调用 `run_wave_worker` | 每次 attempt 可启动 fresh worker；需 PID 测试锁定 | 高 |
| E8 | `dispatcher.rs` attempt loop（约 4888–4990） | retry 通过再次 `executor.execute(current_request.clone())`；中间 sender 静默；只 final outcome 逃逸 | 最小修复应扩展现有 loop，不新增 retry service | 高 |
| E9 | `worker.rs::run_wave_worker_pty` | stdout 每行调用 `classify_heartbeat_line`，events-file 变化记 Strong | “headless 输出就是心跳”已在机制层成立 | 高 |
| E10 | `heartbeat.rs::decide_lease` / `LeaseState::tick` | Strong 刷新并清 weak_count；Weak 在 cap 内刷新；None 不刷新；hard cap 优先 | 本计划保持租约规则，只修 outer budget 协调 | 高 |
| E11 | heartbeat 与 PTY tests | 已有 `lease_hard_cap_wins_over_idle_continue`、`test_run_wave_worker_pty_strong_signal_keeps_alive_past_legacy_timeout` 等 | 新测试应增加 dispatcher 跨层保护，不重复改 heartbeat 算法 | 高 |
| E12 | `execute_wave_via_supervisor_with_executor` | slot binding 提供 cwd；同一 request 在 task 内 retry；permit 跨 attempts 持有 | fresh process 可复用同一 worktree，aggregate 必须计入批次与 attempts | 高 |
| E13 | `presets/schemas/parallel-forge.yml::exec.unit.failed` | `reason` required，来源为本 activation failure evidence | 可将其作为有界 prior detail 注入 retry prompt | 高 |
| E14 | `wave_prompt.rs::build_wave_worker_prompt` | prompt 统一由 `WaveWorkerContext` 构建，当前无 retry 字段 | retry context 应扩展此统一 builder，不在 preset 拼动态字符串 | 高 |
| E15 | `SupervisorConfig.slot_retry_budget` | 默认 1，允许 0..=2；2 表示总共最多 3 attempts | 无需新增配置字段；`parallel-forge` 显式设 2 | 高 |
| E16 | `dispatcher.rs::DispatchContext::build` | partial=aggregate 的 80%；global 被 clamp 到 aggregate | outer deadline 可早于 worker retry budget，必须有安全下限 | 高 |
| E17 | `dispatcher.rs::dispatch_wave_inner_with_release` | partial threshold 当前直接 `finalize_timeout`，并返回 `AggregateDeadlineExceeded` | 不能只声称 heartbeat 会续命；必须使 effective partial 晚于合法 attempts 预算 | 高 |
| E18 | `dispatcher.rs::aggregate_timeout_for` | fallback 已按 `worker_timeout × ceil(events/concurrency) + 30s` 计算批次 | 扩展为 attempt-aware 安全下限与现有模式一致 | 高 |
| E19 | `runner.rs` global deadline | 由 `max_runtime_seconds - elapsed` 计算，dispatcher 优先处理 | global 是绝对上限，不能由 heartbeat 或 retry 续期 | 高 |
| E20 | `presets/en/parallel-forge.yml` | executor timeout=3600、idle=600、weak cap=8、startup grace=300；supervisor aggregate=7200；未显式 retry budget | preset 必须显式 2；固定 7200 不能覆盖任意批次数×3 attempts | 高 |
| E21 | `parallel-forge` failure-handler instructions | 当前写明 `redrive_slots` 仅信息且“修复只能由 correction 完成” | 需澄清 automatic retry 已在 fan-in 前耗尽，handler 不负责 slot redispatch | 高 |
| E22 | `a957ab29` 与 preset triggers | `exec.wave.failed` 已接入 failure handler | 本计划不重建下游 correction，只保证该事件在 attempts 耗尽后才出现 | 高 |
| E23 | `ralph-tools-wave.md` 与 `skills/.../patterns.md` | 文档已描述 heartbeat 和四类自动 retry，但未含 Exec 主动失败与经验继承 | 行为变化必须同步两类 agent/operator 指南 | 高 |
| E24 | `scenarios.rs::run_workflow_guard_scenario` 与 `parallel_forge_exec_wave_failed_correction.yml` | 已有真实 EventLoop 的 exhausted failure→correction 场景 | 扩展该场景语义并新增 dispatcher acceptance，不用 source-only preset 文本测试 | 高 |
| E25 | Git 历史 `d36ae0af`、`a957ab29` | 最近已落地 startup grace/slot auto-retry 与 exhausted failure consumer | 本计划是补齐现有机制语义，不是新架构 | 中高 |

### 2.3 受影响范围

#### 生产模块

- `crates/ralph-core/src/supervisor/worker_outcome.rs`
- `crates/ralph-core/src/wave_prompt.rs`
- `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`

#### 测试模块

- 上述 core 源文件内 `#[cfg(test)]`；
- `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs`
- `crates/ralph-cli/src/loop_runner/tests/wave.rs`
- `crates/ralph-core/tests/scenarios/parallel_forge_exec_wave_failed_correction.yml`

#### 配置与 preset

- `presets/en/parallel-forge.yml`
- `presets/schemas/parallel-forge.yml`

#### Agent 与 operator 文档

- `crates/ralph-core/data/ralph-tools-wave.md`
- `skills/ralph-preset-common/references/patterns.md`
- `skills/ralph-preset-common/references/author-checklist.md`
- `skills/ralph-preset-common/references/finding-rubric.md` 仅检查；本计划不新增 lint finding，若实现未新增 finding 则不得无关修改。
- `skills/ralph-preset-common/references/commands.md` 仅检查；无 CLI 变化时不得修改。
- `skills/ralph-preset-author/SKILL.md`、`skills/ralph-preset-review/SKILL.md` 仅检查 workflow 是否已引用上述 shared references；无流程变化时不得修改。

#### 不受影响

- 无数据库 migration；
- 无新 API/CLI/UI；
- 无 adapter 协议变化；
- 无 builtin manifest/index/zsh completion 变化；
- 无 task ledger 文件手工变更；
- 无 `CLAUDE.md` / `AGENTS.md` 内容变化，除非实施发现需要新增 always-apply hard rule；若发生则必须停下重规划并保持两文件完全一致。

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | retry 放在哪一层 | A 新建 retry service；B operator redrive；C 扩展现有 dispatcher attempt loop | C | E6–E8、E15、E25 | A 增加无必要抽象；B 需要 loop 外 resume，不能满足自动 fresh executor | 0.98 |
| D2 | 哪些主动终态可重试 | A 所有 `*.unit.failed`；B 仅 supervisor `WaveKind::Exec` 的 `exec.unit.failed`；C 不重试主动失败 | B | E4、E13、用户确认范围 | A 会改变 review/fix 语义；C 不满足需求 | 0.95 |
| D3 | retry 判定使用什么 reason | A 动态 payload 文本；B 新稳定码 `executor_reported_failure` + 动态 detail；C 复用 `worker_cancelled` | B | E5、E13、现有 frozen-code 模式 | A 不稳定且可被 agent 文本控制；C 语义错误 | 0.96 |
| D4 | prior experience 从哪里来 | A 持久化全部 PTY stdout；B 只重用相同 prompt；C 累计最多两个既往 attempt 的稳定失败码 + bounded detail，并要求盘点同 worktree Git/artifact/test | C | E2、E6、E9、E13、E14 | A 工程量和敏感信息面过大；B 不能吸取经验；只保留最近一次会让 attempt 3 忘记 attempt 1 | 0.94 |
| D5 | fresh executor 如何实现 | A 新 worktree/branch；B 同 request 再次调用 backend，保留 cwd/slot binding；C 在同进程继续 turn | B | E6–E8、E12 | A 丢失已有成果；C 不是 fresh context/process | 0.97 |
| D6 | attempts 数 | A 无限；B 总 2；C 总 3（budget=2） | C | 用户确认、E15 | A 失控；B 不满足用户指定 | 0.99 |
| D7 | 中间 events 如何处理 | A 合并再补偿；B 仅静默 UI；C event/store/task/merge 全部不暴露，只 final outcome 逃逸 | C | E8、E3、现有 fan-in | A 会提前激活 reviewer；B 仍可能污染 ledger/store | 0.96 |
| D8 | heartbeat 算法是否重写 | A 所有 stdout 无条件续期；B 保持 Strong/Weak/None + weak cap + hard cap；C agent 主动 emit heartbeat | B | E9–E11、用户确认 hard ceiling | A 可被文字空转无限续命；C 增加 agent 负担且已有自动观测 | 0.98 |
| D9 | aggregate 与 retry 如何协调 | A 固定增大 parallel-forge 数字；B 忽略 aggregate；C effective aggregate=max(configured, attempt-aware batch floor)，global 仍最高 | C | E12、E16–E20 | A 无法覆盖任意 wave 宽度；B 失去 wave 安全边界 | 0.90 |
| D10 | 安全下限公式 | A `timeout×attempts`；B 先算 `work_budget=timeout×ceil(events/effective_concurrency)×attempts+30s`，再算 `aggregate_floor=ceil(work_budget/0.8)`；C heartbeat 动态刷新 aggregate | B | E12、E16–E18 | A 忽略排队批次和 80% partial；C 需要跨层 lease broadcast，超出最小修复 | 0.94 |
| D11 | 失败 detail 长度/存储 | A 不限长进 store；B 每 attempt 1 KiB、最多两项，仅用于 prompt/bounded tracing，store 使用稳定码；C 丢弃 detail | B | E13、安全边界 | A 可污染持久化/日志；C 不能吸取主动失败经验 | 0.93 |
| D12 | exhausted 后如何处理 | A 自动宣告成功；B 仍由现有 fan-in 发 `exec.wave.failed`，再走现有 correction；C 直接 `work.failed` | B | E21、E22 | A false green；C 绕过既有 bounded correction | 0.96 |
| D13 | 是否新增依赖/DB migration/CLI | A 新增；B 不新增 | B | E6–E23 | 现有结构足以完成，新增会扩大风险 | 0.99 |
| D14 | 验收层级 | A 全部 E2E；B 纯单测；C 纯函数单测 + fake backend integration + supervisor integration + 一条真实 EventLoop BDD | C | E11、E24、项目测试规则 | A 成本过高且难注入故障；B 不能证明 fresh process/跨模块行为 | 0.96 |
| D15 | 耗尽的 `exec.unit.failed` event batch 如何离开 attempt loop | A 原样交给 tracker；B 规范化为稳定失败 outcome并丢弃业务 batch；C 先合并再由 correction补偿 | B | E4、`dispatcher.rs::record_outcome` 的“有 events 即 result”行为、E8 | A 会让 store Failed 与 tracker result 分叉；C 会提前激活下游 | 0.97 |

所有关键决策均 ≥0.85，无需 BLOCKED 重新决策分支。

---

## 4. BDD 行为规格

```gherkin
Feature: Executor 失败后的经验继承自动重试

  Background:
    Given supervisor execution mode 已启用
    And 当前 wave kind 为 Exec
    And slot 已绑定一个隔离 worktree
    And slot_retry_budget 为 2

  Scenario S1: 主动 exec.unit.failed 后启动新的 executor 并续做
    Given attempt 1 在原 worktree 留下 commit、completion 草稿和测试证据
    And attempt 1 发出带非空 reason 的 exec.unit.failed
    When dispatcher 处理该 worker outcome
    Then slot 不进入 Completed 或 Failed 终态
    And dispatcher 为 attempt 2 启动新的 backend 进程
    And attempt 2 使用同一 worktree 与 slot identity
    And attempt 2 prompt 包含 attempt 编号、稳定失败码、上次 reason 和盘点既有成果的恢复协议

  Scenario S2: timeout 后新的 executor 从原 worktree 恢复
    Given attempt 1 因 worker_timeout 结束
    And 原 worktree 存在未上报 commit 或文件改动
    When retry budget 尚未耗尽
    Then dispatcher 启动新的 backend 进程
    And retry prompt 将 worker_timeout 标为上次失败原因
    And retry prompt 要求先检查现有 git/artifact/test 状态
    And runtime 不自动把已有 commit 视为 exec.unit.done

  Scenario S3: 第二次尝试成功时只暴露最终成功
    Given attempt 1 发出 exec.unit.failed
    And attempt 2 发出 exec.unit.done
    When supervisor 完成 slot 收集
    Then executor 调用次数为 2
    And 主 ledger 只出现 attempt 2 的 exec.unit.done
    And slot 只记录一次 Completed
    And task 在 forge.wave.settled 前保持 open

  Scenario S4: 三次失败后只生成一次 wave failure
    Given attempt 1、2、3 都以可重试原因失败
    When attempt 3 结束
    Then 不再启动 attempt 4
    And 最终 exec.unit.failed event batch 被规范化为失败并且不进入主 ledger
    And slot 记录一次 Failed
    And fan-in 生成一次 exec.wave.failed
    And slot_failures 包含稳定最终 reason
    And redrive_slots 只表示自动 retry 已耗尽后的 operator 候选

  Scenario S5: retry budget 为零时保持单次执行
    Given slot_retry_budget 为 0
    When executor 发出 exec.unit.failed 或发生 worker_timeout
    Then dispatcher 只调用 executor 一次
    And 失败直接进入现有 final outcome 路径

  Scenario S6: 非 Exec wave 的 failed terminal 不受影响
    Given wave kind 为 Review 或 Fix
    When worker 发出对应 unit.failed terminal
    Then 新增的 executor_reported_failure 规则不适用
    And 原有 worker outcome truth table 保持

Feature: Headless stdout heartbeat 与 wave deadline 协调

  Scenario S7: 合格 stdout 心跳刷新 idle lease
    Given worker hard cap 尚未到达
    And worker 周期性输出 Strong heartbeat
    When elapsed 时间超过 idle_heartbeat_secs
    Then worker 不因 idle timeout 被杀
    And weak_count 被 Strong 重置

  Scenario S8: 只有有限 Weak 输出不能无限续命
    Given worker 只输出 Weak heartbeat
    When连续 Weak 次数超过 idle_weak_signal_cap
    Then worker 以 idle timeout 结束
    And 该失败进入 worker_timeout retry family

  Scenario S9: heartbeat 不突破 per-attempt hard cap
    Given worker 持续输出 Strong heartbeat
    When elapsed 到达 hat timeout
    Then当前 attempt 被 hard kill
    And预算允许时启动新的 executor

  Scenario S10: aggregate 安全下限覆盖批次和三次 attempts
    Given wave 有 N 个 events、有效并发 C、per-attempt timeout T、max attempts 3
    And work_budget 为 T * ceil(N/C) * 3 + 30s
    And operator 配置的 aggregate 小于 ceil(work_budget / 0.8)
    When supervisor 构造 dispatch context
    Then effective aggregate 使用 ceil(work_budget / 0.8)
    And 80% partial threshold 不早于 work_budget

  Scenario S11: global deadline 仍可终止健康 worker
    Given runner 剩余 max runtime 小于 effective aggregate
    And worker 持续输出合格 heartbeat
    When global deadline 到达
    Then dispatcher 终止 wave 并返回 GlobalDeadlineExceeded
    And 不启动新的 attempt
```

---

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
|---|---|---|---|---|---|
| S1 | 两次 executor 调用；不同 PID；同 cwd；attempt 2 prompt 有 prior context；无中间 store terminal | `wave_supervisor.rs` planned fake-backend test | 集成 | State-machine + idempotency | 否 |
| S2 | timeout 后 same worktree；prompt 有 `worker_timeout`；commit 不自动成功 | `wave_supervisor.rs` + dispatcher tests | 集成 | Fault injection | 否 |
| S3 | final done only；主 ledger 无 failed；task 不提前 close | dispatcher supervisor integration | 集成 | Idempotency | 否 |
| S4 | CLI supervisor 集成测试证明恰好 3 calls、无第 4 次、一次 `exec.wave.failed`；existing real EventLoop BDD证明 exhausted failure 进入 correction | `wave_supervisor.rs` + existing correction BDD | 集成 + BDD | State-machine | 否 |
| S5 | budget 0 时 call_count=1 | dispatcher attempt-loop unit/integration | 单元/集成 | Characterization | 否 |
| S6 | Review/Fix truth table不变 | `worker_outcome.rs` + dispatcher classification tests | 单元 | Differential | 否 |
| S7 | 超过 idle window仍存活 | existing `wave.rs` test + regression | PTY 集成 | Characterization | 否 |
| S8 | weak cap 后 idle kill | existing `wave.rs` test + retry-family assertion | PTY 集成 | Fault injection | 否 |
| S9 | hard cap仍优先，且可进入下一 attempt | heartbeat unit + dispatcher integration | 单元/集成 | State-machine | 否 |
| S10 | exact反推公式、overflow-safe、partial不早于工作预算 | dispatcher helper unit + paused-time integration | 单元/集成 | Boundary | 否 |
| S11 | global deadline preempts | existing dispatcher paused-time test | 集成 | Characterization | 否 |

### 5.1 具体断言与不变量

- 所有 attempt 测试断言 executor call count，而非只检查最终 payload。
- fresh-process 测试断言 PID 全异、cwd 全同、slot/wave env 全同。
- prompt 测试断言结构化字段存在、attempt 3 累计 attempt 1/2 摘要、detail 截断安全、无凭证/完整 stdout。
- store 断言 attempt 1/2 不调用 `record_slot_result` / `record_slot_failure`，仅 final 调用一次。
- main events 断言只存在 final event；不得通过忽略中间事件来伪造 Green。
- task ledger 通过真实 projection/BDD 断言在 settlement 前未关闭；测试不得直接编辑 `.ralph/agent/tasks.jsonl`。
- deadline 测试使用 `tokio::time::pause/advance` 或现有暂停时钟模式，不用长 sleep。
- global deadline 测试必须继续优先于 aggregate floor。

### 5.2 测试运行方式

- core outcome/prompt：`cargo nextest run -p ralph-core -- worker_outcome`、`cargo nextest run -p ralph-core -- wave_prompt`
- CLI dispatcher/supervisor：`cargo nextest run -p ralph-cli --bin ralph -- executor_retry`
- PTY heartbeat：`cargo nextest run -p ralph-cli --bin ralph -- heartbeat`
- BDD：`cargo nextest run -p ralph-core --test scenarios -- parallel_forge_exec_wave_failed`
- preset：见第 9 节固定命令。

选择这些层级是因为纯规则用单测即可证明，fresh PID/cwd 和 store/main-ledger 边界必须用集成测试，preset 下游流转必须用真实 EventLoop BDD；live backend 不提供更强确定性。

---

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | Exec 主动失败进入 retry | S1/S3 | `executor_failed_terminal_retries_then_done` | attempt disposition truth table | supervisor integration | 否 | E4/E5/E13 |
| R2 | fresh process、same worktree | S1/S2 | `executor_retry_uses_fresh_pid_same_cwd` | request clone/context | fake backend process test | 否 | E6–E8/E12 |
| R3 | prior experience 注入 | S1/S2 | `retry_prompt_contains_prior_attempt_context` | prompt builder tests | fake backend captures prompt | 否 | E2/E13/E14 |
| R4 | 总 attempts=3 | S4/S5 | `retry_budget_two_runs_exactly_three_attempts` | budget boundary | supervisor integration | 否 | E15 |
| R5 | 中间 attempt 隔离 | S3/S4 | `intermediate_attempts_do_not_escape` | disposition/store spy | ledger/store integration | 否 | E8 |
| R6 | exhausted 后 wave failed | S4 | `parallel_forge_executor_retry_exhaustion`（CLI supervisor 集成测试） | failure payload helper regression | existing real EventLoop correction BDD | 否 | E21/E22/E24 |
| R7 | heartbeat 刷新 idle | S7/S8 | 现有 PTY heartbeat tests | lease tests | PTY integration | 否 | E9–E11 |
| R8 | hard/global cap 保留 | S9/S11 | hard/global deadline tests | lease hard cap | dispatcher paused-time | 否 | E10/E19 |
| R9 | aggregate 覆盖 batches×attempts | S10 | `attempt_aware_aggregate_floor` | formula/overflow | dispatch context integration | 否 | E12/E16–E20 |
| R10 | 非 Exec/legacy 兼容 | S5/S6 | `non_exec_failed_terminal_not_retried` | truth table | dispatcher regression | 否 | E4/E8 |
| R11 | 文档/preset 合同同步 | S4/S10 | preset strict lint + docs drift | config parse | scenario/preset tests | 否 | E20/E21/E23/E24 |

Scenario → Unit：S1/S2/S3/S5/S6 → Unit 1/2；S7/S8/S9/S10/S11 → Unit 3；S4 与 preset 下游 → Unit 4。

---

## 7. 严格串行开发单元

```text
Unit 1：Exec 主动失败进入现有 attempt loop
  ↓ 完成全部测试、重构和回归
Unit 2：fresh executor 注入上次经验并复用 worktree
  ↓ 完成全部测试、重构和回归
Unit 3：heartbeat 租约与 attempt-aware aggregate 收敛
  ↓ 完成全部测试、重构和回归
Unit 4：Parallel Forge 三次尝试、耗尽 fan-in 与文档合同
```

### Unit 1：Exec 主动失败进入现有 attempt loop

#### 1. Unit 目标

当 supervisor Exec worker 发出 `exec.unit.failed` 时，调用方观察到它是一次可重试失败 attempt，而不是 Completed slot；预算内 dispatcher 再调用 executor，且中间失败不写 store 终态。

#### 2. 对应需求与 Scenario

- Requirement：R1、R4、R5、R10
- Scenario：S1、S3、S5、S6
- Decision：D1、D2、D3、D6、D7、D15
- Evidence：E4、E5、E8、E13、E15

#### 3. 外部可观察结果

- budget=2 且 first=`exec.unit.failed`, second=`exec.unit.done` 时 executor call_count=2；
- store 只记录 final Completed；
- main ledger 只收到 final done；
- Review/Fix failed terminal 不进入该分支；
- budget=0 保持单次。

#### 4. 当前行为基线

E4 证明 `exec.unit.failed` 当前成为 `Completed(Failed)`；E8 证明 attempt loop 只检查 `SlotOutcome::Failed` + frozen reason。先新增 Characterization Test 固定这一本计划要推翻的旧行为：`classify_worker_outcome` 本身仍把 Failed marker 表示为 terminal-valid，不直接改通用 truth table；真正的 Exec 业务失败转换发生在 dispatcher 的 wave-kind-aware attempt disposition。

#### 5. 输入与输出

- 输入：`WaveKind::Exec`、`WaveWorkerOutcome`、accepted `exec.unit.failed`、budget；
- 输出：planned `AttemptDisposition::RetryableFailure`（名称为计划新增；若编码前已出现同义类型则停止并更新计划，禁止并存两套分类）；
- 错误：稳定 `executor_reported_failure`；
- 状态：预算内无 store terminal；final 再落 store；
- 耗尽规范化：final `exec.unit.failed` 不得以 `Ok(events, ...)` 逃逸；必须转换成稳定 Failed outcome，使外层 tracker 走 failure 分支并丢弃该 event batch；
- 不变量：通用 worker outcome truth table、Review/Fix 语义不变。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `worker_outcome.rs` | 稳定 reason/retry class | 新增 `REASON_EXECUTOR_REPORTED_FAILURE`，加入 retryable/failure-class 映射及测试 | 不让通用 truth table直接依赖 topic |
| `dispatcher.rs::WorkerRequest` | worker 调用合同 | 增加 typed `wave_kind`，Clone 同步 | 不增 backend/DB 字段 |
| `dispatcher.rs::classify_slot_result` 相邻分类层 | outcome → store/retry | 新增 planned `classify_slot_attempt(result, wave_kind)`；Exec Failed terminal 转成稳定失败；识别规范化后的稳定失败 | 不改变 non-Exec |
| `dispatcher.rs` attempt loop | 预算内 retry | 统一读取 attempt disposition；耗尽时把主动失败 batch 明确规范化为 `Err((REASON_EXECUTOR_REPORTED_FAILURE.to_string(), duration))` 后再返回 join task | 不实现 prior prompt（Unit 2） |
| `dispatcher.rs::record_outcome` | join outcome → tracker | 用集成断言证明规范化结果必走 tracker failure；不得给该函数增加 topic 特判 | 不改变普通 event batch |
| dispatcher tests / `wave_supervisor.rs` | 行为验证 | 新增 sequence executor/store spy tests | 不跑 live backend |

#### 7. 可依赖能力

- `WorkerTerminalKind`、`WaveKind`、`WorkerRequest::Clone`；
- `silent_request`；
- existing retry budget bridge；
- in-memory supervisor store/test bridge。

#### 8. 禁止依赖的未来能力

- 不依赖 Unit 2 retry prompt；
- 不依赖 Unit 3 aggregate floor；
- 不修改 `parallel-forge` budget；
- 不提前新增 BDD fixture。

#### 9. 验收测试

1. `executor_failed_terminal_retries_then_done`
   - 层级：dispatcher integration；
   - 前置：Exec、budget=2、sequence outcome failed→done；
   - 断言：calls=2、store result=1/failure=0、final events only done。
2. `executor_failed_terminal_budget_zero_does_not_retry`
   - Exec、budget=0；
   - 断言 calls=1、final Failed。
3. `non_exec_failed_terminal_keeps_existing_semantics`
   - Review/Fix；
   - 断言不使用 `executor_reported_failure`。
4. `intermediate_exec_failed_event_does_not_escape`
   - 断言 tracker/main ledger无 attempt 1 event。
5. `exhausted_exec_failed_event_is_normalized_before_tracker`
   - 断言 final failed event 不出现在 `CompletedWave.results`，`failures` 只含稳定 reason。

运行：`cargo nextest run -p ralph-cli --bin ralph -- executor_failed_terminal`

#### 10. Acceptance Red

- 首先运行 `executor_failed_terminal_retries_then_done`。
- 预期失败：call_count 实际为 1，store 走 result/Completed，或 main batch含 failed。
- 正确原因：当前 `Completed(Failed)` 不满足 retry predicate。
- 无效 Red：fixture 未构造合法 event、bridge 未启用 supervisor、命令过滤不到测试、编译环境错误。

#### 11. 单元测试拆分

- `REASON_EXECUTOR_REPORTED_FAILURE` 属于 retryable；
- failure class 为 `required_slot_failure`，而非 unknown；
- Exec+Failed marker → retryable disposition；
- Exec+Done → completed；
- Review/Fix+Failed → existing completed terminal；
- malformed/empty failed payload仍用稳定码。
- exhausted Exec failed batch → normalized failure。
- Fake：sequence executor、recording bridge。
- 不允许 Mock：attempt loop 的最终 record 分支与 main event batch。

#### 12. Red → Green → Refactor 顺序

```text
Acceptance Test failed→done Red
→ 新增稳定 reason 与 wave_kind 字段
→ disposition 单测 Red/Green
→ attempt loop 使用 disposition
→ Acceptance Green
→ budget=0 Red/Green
→ non-Exec regression Red/Green
→ exhausted batch normalization Red/Green
→ 抽取单一分类 helper
→ Integration/Regression
```

#### 13. 最小实现范围

- 必须：typed wave kind、稳定 reason、attempt disposition、最终 store 分类同源；
- 必须：中间 failed 不逃逸；
- 必须：final failed 记录为 stable failure，原业务 event batch不得进入 tracker result；
- 不实现：prompt 经验、PID 测试、timeout公式、preset配置。

#### 14. 集成验证

- 联合真实 dispatcher attempt loop + in-memory bridge + tracker；
- executor backend 可 Fake；
- store record/ledger batch必须真实走当前 helper；
- 命令同第 9 项；
- 预期所有 `executor_failed_terminal` 测试通过。

#### 15. 风险驱动测试

- Characterization：通用 truth table 不变，避免把 reviewer failure错误重试；
- State-machine：in-flight→final Completed/Failed 只能一次；
- Idempotency：中间 attempt不能重复 record。

#### 16. 回归范围

- `cargo nextest run -p ralph-core -- worker_outcome`
- `cargo nextest run -p ralph-cli --bin ralph -- slot_retry`
- `cargo nextest run -p ralph-cli --bin ralph -- worker_outcome`
- 原因：稳定 reason 和分类是 fan-in/redrive 共享契约。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/supervisor/worker_outcome.rs` | 修改现有生产文件/单测 | 新稳定 reason 与映射 | E5 |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | 修改现有生产文件/测试 | wave-kind-aware attempt disposition | E4/E8 |
| `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | 新增测试 | 跨 store/attempt 验收 | E24 |

#### 18. 完成标准

本 Unit Scenario、单测、集成、回归、`cargo build -p ralph-core -p ralph-cli`、`cargo clippy -p ralph-core -p ralph-cli`、`cargo fmt --check` 全绿；无 skip/only/弱化断言；不含 Unit 2–4 行为；Evidence/Decision 未降置信度；可独立提交。

#### 19. 停止条件

若 `WaveKind` 在 request 构造点不可获得、分类必须改 DB schema、Red 显示 failed event 已在 attempt 前合并、或 non-Exec 行为不可隔离，停止并按“记录新证据→更新影响→重决策→重算置信度→修订计划”处理。

#### 20. 风险与注意事项

| 风险 | 触发 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| 把 Review failure 误重试 | helper 不看 WaveKind | non-Exec regression | typed WaveKind gate | 低 |
| 动态 reason 污染稳定分类 | 直接 store payload reason | exact reason assertion | stable code | 低 |
| 中间 failed 泄漏 | record 在 loop 内发生 | spy/main ledger断言 | final-only boundary | 低 |

### Unit 2：fresh executor 注入上次经验并复用 worktree

#### 1. Unit 目标

发生可重试失败后，下一 attempt 是新的 backend 进程；它使用同一 worktree，并在 prompt 中得到全部既往 attempts 的有界失败摘要和必须盘点已有成果的恢复协议。

#### 2. 对应需求与 Scenario

- Requirement：R2、R3
- Scenario：S1、S2
- Decision：D4、D5、D11
- Evidence：E2、E6–E9、E12–E14

#### 3. 外部可观察结果

fake backend 记录 attempt 1/2 PID 不同，cwd/slot env 相同；attempt 2 prompt 明确显示 `2/3`、稳定 reason、bounded detail，并指导先读现有 Git/completion/test 状态。

#### 4. 当前行为基线

E6/E8 证明 prompt 当前逐 attempt 完全相同；E9 证明 stdout 不返回 outcome；E2 证明同 worktree artifact 足以作为续做经验。先固定“same request currently has same prompt”的 Characterization Test，再推翻 prompt 同一性。

#### 5. 输入与输出

- 输入：attempt number、max attempts、stable code、optional detail、cwd；
- 输出：planned `RetryContext { attempt, max_attempts, prior_attempts }` 与 planned `PriorAttempt { attempt, failure_code, detail }`（新增于 `wave_prompt.rs`）；
- detail：每项 trim、1 KiB UTF-8 安全截断；prior attempts 最多两项并按 attempt 升序；
- 副作用：仅改变后续 process prompt/tracing；
- 不变量：原 task payload、hat instructions、publish topics、env ACL 不变。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `wave_prompt.rs::WaveWorkerContext` | prompt context | 新增 optional `RetryContext` 与 prior-attempt列表；渲染 `# Retry Context` | 不读取磁盘 |
| `dispatcher.rs::WorkerRequest` | 每次执行的 prompt/cwd | 保留 base prompt 或足够重建信息；retry 前生成新 prompt | 不改变 cwd/worker path |
| `dispatcher.rs` attempt loop | 知道上次 disposition | 提取 bounded failure detail，构造下一 request | 不持久化 stdout |
| `wave_supervisor.rs` / existing fake backend helpers | process integration | 记录 PID/cwd/prompt/env 与 staged terminal | 不调用 live API |

#### 7. 可依赖能力

- Unit 1 attempt disposition；
- `floor_char_boundary` 或仓库已有 UTF-8 安全截断 helper；
- original `build_wave_worker_prompt`；
- test fake PATH backend 与 env scrub 规则。

#### 8. 禁止依赖的未来能力

- 不依赖 aggregate floor；
- 不显式配置 budget=2；
- 不改 failure-handler；
- 不新增 diagnostics stdout 文件。

#### 9. 验收测试

1. `retry_prompt_contains_prior_attempt_context`
   - prompt unit；
   - exact assert attempt 2/3、code、detail、续做动作与禁止破坏性清理。
2. `retry_prompt_truncates_detail_at_utf8_boundary`
   - >1 KiB 中文/emoji；
   - assert valid UTF-8、标记 truncated、长度有界。
3. `executor_retry_uses_fresh_pid_same_cwd`
   - fake backend attempt1 写 PID/cwd/prompt并发 failed；attempt2 写另一个 PID并发 done；
   - assert PID distinct、cwd same、env wave identity same。
4. `timeout_retry_does_not_claim_existing_commit_success`
   - worktree预置 commit；attempt2仍必须由 fake backend发 done 才成功。
5. `third_attempt_prompt_contains_both_prior_failures`
   - attempt1主动failed、attempt2 timeout、attempt3捕获prompt；
   - 断言按1、2顺序包含两个稳定码及各自detail/fallback，不重复、不丢失。

运行：
- `cargo nextest run -p ralph-core -- wave_prompt`
- `cargo nextest run -p ralph-cli --bin ralph -- executor_retry_uses_fresh_pid`

#### 10. Acceptance Red

- 首先运行 fresh PID/same cwd/prompt 测试。
- 预期 PID 已不同但 prompt 缺 `Retry Context`，因此精确断言失败。
- 该 Red 证明测试真正到达第二次 process invocation，而非仅测试字符串 helper。
- 无效 Red：fake script不可执行、PATH/env未 scrub、没有 supervisor bridge、attempt1 未发合法 failed。

#### 11. 单元测试拆分

- `RetryContext=None` 时 prompt与旧行为一致；
- attempt/max格式；
- timeout 无 detail 使用 `unavailable`；
- attempt 3 按序累计 attempt 1/2；
- JSON reason extraction；
- malformed reason fallback；
- UTF-8 bounded；
- same worktree instructions；
- 禁止自动成功文字。
- Fake：仅 backend process；
- 不允许 Mock：实际第二次 process spawn、cwd、prompt argv/stdin。

#### 12. Red → Green → Refactor 顺序

```text
Fresh-process acceptance Red
→ RetryContext prompt unit Red
→ 最小 prompt渲染 Green
→ detail extraction Red/Green
→ attempt loop更新下一 request
→ fresh PID/same cwd/prompt Green
→ UTF-8 boundary Red/Green
→ Refactor为单一 bounded helper
→ Integration/Regression
```

#### 13. 最小实现范围

- 必须：new process evidence、same cwd、retry context；
- 必须：reason extraction/fallback/limit；
- 必须：prompt要求盘点 Git/status/log/completion/test；
- 不实现：stdout capture、DB attempt history、自动 Git 操作。

#### 14. 集成验证

- 真实 `ProductionExecutor`/PTY + fake CLI；
- supervisor/store真实 in-memory；
- 外部 API Fake；
- 测试 helper 必须 scrub agent env 后再显式注入；
- 预期进程/路径/prompt断言全绿。

#### 15. 风险驱动测试

- Fault Injection：主动 fail 与 timeout 两条；
- Idempotency：同 worktree已有 commit不重复提交/不自动成功；
- Fuzz-lite boundary：多字节 detail 截断。

#### 16. 回归范围

- `cargo nextest run -p ralph-core -- wave_prompt`
- `cargo nextest run -p ralph-cli --bin ralph -- wave_worker`
- 带污染 env 重跑 fresh-process integration：
  `RALPH_CURRENT_HAT=executor RALPH_EVENTS_FILE=/tmp/x.jsonl cargo nextest run -p ralph-cli --bin ralph -- executor_retry_uses_fresh_pid`
- 原因：prompt所有 wave共用，process fixture易受外层 hat env 影响。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/wave_prompt.rs` | 修改生产/单测 | typed retry context | E14 |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | 修改生产/测试 | 每 attempt prompt重建、detail提取 | E6/E8 |
| `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | 新增集成测试 | fresh PID/same cwd | E7/E12 |

#### 18. 完成标准

当前 Scenario/单测/PTY integration/回归/build/clippy/fmt全绿；env pollution test绿；无 stdout持久化、无新依赖、无未来 Unit行为；Evidence/Decision≥0.85；可独立提交。

#### 19. 停止条件

若 backend prompt 不可在 fake fixture捕获、retry会更换 cwd/slot binding、失败 detail 只能通过完整 stdout取得、或需要新增敏感日志，停止并重规划。

#### 20. 风险与注意事项

| 风险 | 触发 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| prompt detail注入 | reason含指令文本 | prompt结构测试 | 标为 untrusted failure detail，要求只作证据；限长 | 中低 |
| 误清理已有成果 | retry文案要求从零开始 | prompt断言 | 明确禁止 reset/clean/overwrite | 低 |
| 假 fresh process | test只看 calls | PID断言 | real fake CLI process | 低 |

### Unit 3：heartbeat 租约与 attempt-aware aggregate 收敛

#### 1. Unit 目标

有效 headless heartbeat 继续刷新 idle lease；supervisor effective aggregate 同时覆盖并发批次与最大 attempts，使 80% partial threshold 不会早于合法 attempts 的硬预算；hard/global 上限仍生效。

#### 2. 对应需求与 Scenario

- Requirement：R7、R8、R9
- Scenario：S7–S11
- Decision：D8、D9、D10
- Evidence：E9–E12、E16–E20

#### 3. 外部可观察结果

同一配置下，健康 worker 不被 idle kill；第三次 attempt可在 wave budget内启动；weak-only仍会 kill；per-attempt hard和runner global仍能终止。

#### 4. 当前行为基线

existing heartbeat tests 固定 S7–S9；E16/E17 固定 outer partial abort；E18 的 fallback只算 batches、不算 attempts。先新增 formula Characterization：budget=0时新 helper必须等价 existing `aggregate_timeout_for`。

#### 5. 输入与输出

- 输入：T、N、C、retry_budget、configured aggregate；
- max_attempts=`retry_budget+1`；
- `work_budget=T × ceil(N/C) × max_attempts + 30s`，全部 saturating；
- `aggregate_floor=ceil(work_budget × 10 / 8)`；实现必须采用先除后补余数或等价的 overflow-safe saturating 算法，不能先无界乘 10；
- effective=`max(configured, aggregate_floor)`，仅 supervisor path；
- 输出：DispatchContext deadlines；
- 错误：无新运行时 error；
- 不变量：global deadline、hard cap、weak cap优先级不变。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `dispatcher.rs::aggregate_timeout_for` | batch budget | 扩展 planned `aggregate_timeout_for_attempts` 或等价单一 helper；返回使 80% partial 不早于 work budget 的 aggregate floor；budget=0时 work-budget部分兼容旧公式 | 不改 legacy dispatch path语义 |
| `execute_wave_via_supervisor_with_executor` | supervisor timeout解析 | effective=max(configured,floor) | 不改 global |
| dispatcher paused-time tests | deadline | 新增 attempts/batches/overflow/global tests | 不用 sleep |
| heartbeat tests | lease | 只回归，必要时增加跨层 test | 不改分类算法 |

#### 7. 可依赖能力

- Unit 1 的 retry budget；
- existing `aggregate_timeout_for`、tokio paused time；
- heartbeat state machine和PTY tests。

#### 8. 禁止依赖的未来能力

- 不依赖 preset budget=2；
- 不改 `exec.wave.failed` schema；
- 不新增 heartbeat event/topic；
- 不把 aggregate变成无限动态续期。

#### 9. 验收测试

- `aggregate_work_budget_zero_retry_matches_legacy_formula`
- `aggregate_floor_counts_three_attempts_and_batches`
- `aggregate_floor_keeps_partial_at_or_after_work_budget`
- `configured_aggregate_above_floor_is_preserved`
- `aggregate_floor_saturates_without_overflow`
- `healthy_worker_is_not_preempted_before_attempt_budget`
- existing hard/global/weak tests继续绿。

运行：
- `cargo nextest run -p ralph-cli --bin ralph -- aggregate_floor`
- `cargo nextest run -p ralph-cli --bin ralph -- heartbeat`
- `cargo nextest run -p ralph-cli --bin ralph -- global_deadline`

#### 10. Acceptance Red

先运行 `aggregate_floor_counts_three_attempts_and_batches`：当前实际只返回 configured 7200 或 single-attempt fallback，低于预期 floor。失败必须是 duration assertion，不得是 tokio clock未暂停。

#### 11. 单元测试拆分

- N=0按1；
- C=0按1；
- budget=0→attempts1，且中间 `work_budget` 等于旧公式；
- budget=2→attempts3；
- N>C批次 ceil；
- configured更大保留；
- u64 saturating；
- `aggregate_floor=ceil(work_budget/0.8)`；
- partial=ceil(80% aggregate)不早于 work_budget。
- 不 Mock heartbeat state machine。

#### 12. Red → Green → Refactor 顺序

```text
Formula acceptance Red
→ budget=0 work-budget兼容单测
→ attempts/batches work-budget最小实现 Green
→ 80%反推 aggregate floor Red/Green
→ configured max Green
→ overflow Red/Green
→ supervisor接线
→ paused-time worker test Red/Green
→ hard/global regression
→ Refactor单一 timeout helper
```

#### 13. 最小实现范围

- 必须：supervisor only attempt-aware work budget与80%反推 floor；
- 必须：saturating与global优先；
- 必须：现有 heartbeat语义不变；
- 不实现：跨层heartbeat broadcast、动态aggregate续租、preset数值硬编码。

#### 14. 集成验证

联合 dispatcher、attempt loop、paused time、fake executor；heartbeat pure/PTY path真实；global limit真实；外部 backend可Fake。预期第三 attempt可在 work budget 内结束，partial 不早于 work budget，global仍可抢占。

#### 15. 风险驱动测试

- Boundary：0、最大u64、并发>events；
- Differential：budget=0与旧公式一致；
- State-machine：idle/hard/global优先级；
- Fault Injection：weak cap与silence。

#### 16. 回归范围

- `cargo nextest run -p ralph-cli --bin ralph -- partial_threshold`
- `cargo nextest run -p ralph-cli --bin ralph -- aggregate_deadline`
- `cargo nextest run -p ralph-cli --bin ralph -- heartbeat`
- `cargo nextest run -p ralph-cli --bin ralph -- partial_timeout_events_visible`
- 原因：timeout公式影响wave所有 supervisor worker；race-sensitive子集必须遵守项目入口。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | 修改生产/测试 | attempt-aware work budget与80%反推 aggregate floor | E16–E18 |
| `crates/ralph-cli/src/loop_runner/tests/wave.rs` | 新增/修改测试 | 跨 heartbeat/hard regression | E9–E11 |

#### 18. 完成标准

公式、paused-time、heartbeat、hard/global、partial-timeout回归、build/clippy/fmt全绿；无扩大 timeout掩盖死锁（公式有明确合同）；无Unit4 preset变更；可独立提交。

#### 19. 停止条件

若实际 concurrency 与传入值不一致、permit在attempt间释放导致公式改变、global被effective aggregate覆盖、或测试显示partial仍能早于floor触发，停止重规划。

#### 20. 风险与注意事项

| 风险 | 触发 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| wave最坏耗时增长 | 多批次×3 attempts | duration公式/全局limit | global max仍封顶；仅retry-enabled floor | 中 |
| overflow导致小timeout | 极端配置 | max值测试 | saturating | 低 |
| heartbeat文字空转 | Weak无限 | weak-cap regression | 保持cap | 低 |

### Unit 4：Parallel Forge 三次尝试、耗尽 fan-in 与文档合同

#### 1. Unit 目标

`parallel-forge` 明确启用总计三次 executor attempts；只有三次耗尽后才产生一次 `exec.wave.failed` 并进入现有 correction，同时 agent/operator 文档准确描述经验继承、heartbeat和停止条件。

#### 2. 对应需求与 Scenario

- Requirement：R4、R6、R11
- Scenario：S4、S10
- Decision：D6、D12、D14
- Evidence：E20–E24

#### 3. 外部可观察结果

- preset parse后 `slot_retry_budget=2`；
- runtime scenario中三次失败后才有 `exec.wave.failed`；
- failure handler将其视为 auto retry exhausted；
- agent知道无需主动发heartbeat，新 executor会先检查原worktree；
- preset lint/schema/BDD/docs drift全绿。

#### 4. 当前行为基线

E20 显示未显式budget，当前为默认1；E21文案把 redrive/correction描述成唯一修复；E22/E24证明 exhausted failure下游已存在。先新增 config parse acceptance，预期 actual=1、expected=2。

#### 5. 输入与输出

- 输入：parallel-forge YAML、三次 scripted outcomes；
- 输出：final done或一次 exhausted `exec.wave.failed`；
- state：task不在 attempt失败时关闭；
- 不变量：failure handler三轮correction合同不变；`exec.wave.failed`仍runtime-owned。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | supervisor/executor/failure instructions | `slot_retry_budget: 2`；executor retry续做说明；handler说明auto retry已耗尽 | 不改hat拓扑/topic |
| `presets/schemas/parallel-forge.yml` | event字段语义 | `exec.wave.failed.redrive_slots`/slot failure docs澄清 | 不增required field |
| `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | CLI supervisor integration | 三次attempt exhausted→一次 runtime fan-in failure | 不用source text断言 |
| `scenarios/parallel_forge_exec_wave_failed_correction.yml` | real EventLoop BDD | 明确输入是“自动 attempts 已耗尽”的 runtime-owned failure，并继续验证 failure→correction | 不声称它启动 backend |
| `scenarios.rs` | BDD入口 | 复用现有 `run_workflow_guard_scenario` test | 不用stub，不新增重复入口 |
| `ralph-tools-wave.md` | agent guide | 加主动Exec失败、经验继承、三次与heartbeat停止条件 | 不泄漏内部函数/ledger路径 |
| operator shared refs | author/reviewer规则 | 同步retryable reason与resume协议 | 不新增无依据lint finding |

#### 7. 可依赖能力

- Unit 1–3已验证能力；
- existing parallel-forge failure/correction BDD；
- preset strict lint与schema parity。

#### 8. 禁止依赖的未来能力

- 不依赖 stdout diagnostics；
- 不新增CLI或DB；
- 不新增第4次attempt；
- 不修改F1或runtime state。

#### 9. 验收测试

- `parallel_forge_sets_three_total_executor_attempts`
- `parallel_forge_executor_retry_exhaustion` CLI supervisor integration；
- existing `test_parallel_forge_exec_wave_failed_routes_to_correction`；
- preset/schema strict lint；
- guide drift/static checks。

运行命令见第9节，不得省略。

#### 10. Acceptance Red

先运行 preset config test，预期 actual budget=1。再运行 CLI supervisor exhaustion integration，预期 call count 只有2或主动 failed 被记 Completed。existing BDD只负责 exhausted failure 的下游 correction 回归，不作为“三次进程重试”的 Red。有效Red必须来自预算/行为缺口，不得用精确prompt文案测试冒充。

#### 11. 单元测试拆分

- YAML parse budget=2；
- total attempts=3；
- attempt1/2无 wave failed；
- attempt3后 exactly one；
- slot_failures稳定 reason；
- redrive_slots只在exhausted payload；
- handler route to correction。
- 不允许只grep YAML文字。

#### 12. Red → Green → Refactor 顺序

```text
Preset budget Red
→ YAML最小配置 Green
→ CLI supervisor exhaustion integration Red
→ 接入前三Unit能力 Green
→ exhausted payload Green
→ existing correction BDD Green
→ schema docs与instructions同步
→ agent/operator docs同步
→ preset lint/scenarios/docs drift
→ full regression
```

#### 13. 最小实现范围

- 必须：budget2、exhausted once、docs/schema/instructions；
- 必须：AI guide说明触发条件/动作/字段来源/停止条件；
- 不实现：新event字段、hat/preset新增、zsh/manifest变更。

#### 14. 集成验证

- CLI supervisor retry dispatcher真实，backend outcomes可script；
- real EventLoop `run_workflow_guard_scenario` 只验证 exhausted failure 的下游 correction；
- preset/schema真实parse；
- correction下游真实event assertion；
- 预期事件顺序无中间 `exec.wave.failed`。

#### 15. 风险驱动测试

- State-machine：三次attempt与一次terminal；
- Idempotency：重复final不得重复failure；
- Contract：preset/schema/guide同义；
- Characterization：existing correction scenario保持。

#### 16. 回归范围

- parallel-forge scenarios；
- preset_lint三组；
- all embedded presets；
- agent docs drift；
- `./scripts/run-tests.sh`。
- 原因：preset/schema/event说明影响嵌入preset和真实flow。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改配置/instructions | budget与agent合同 | E20/E21 |
| `presets/schemas/parallel-forge.yml` | 修改字段文档 | exhausted语义 | E13/E21 |
| `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | 新增CLI supervisor集成测试 | 三次attempt与fan-in验收 | E8/E24 |
| `crates/ralph-core/tests/scenarios/parallel_forge_exec_wave_failed_correction.yml` | 修改现有BDD fixture | 澄清并回归exhausted failure下游 | E24 |
| `crates/ralph-core/data/ralph-tools-wave.md` | 修改agent guide | 新行为可执行说明 | E23 |
| `skills/ralph-preset-common/references/patterns.md` | 修改operator reference | author规则同步 | E23 |
| `skills/ralph-preset-common/references/author-checklist.md` | 修改checklist | review检查点 | E23 |

#### 18. 完成标准

所有S4/S10验收、BDD、preset/schema、docs drift、targeted、build/clippy/fmt、全量门禁通过；无skip/only/弱化断言；无未解释snapshot；未修改manifest/index/zsh；skills检查完成；可独立提交。

#### 19. 停止条件

若真实BDD无法驱动supervisor attempt loop、preset schema要求新增字段、failure handler拓扑需改、operator skill需要新finding、或 full regression暴露跨preset语义变化，停止重规划。

#### 20. 风险与注意事项

| 风险 | 触发 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| retry与correction重复修复 | 中间wave failed泄漏 | event sequence断言 | final-only | 低 |
| 文档泄漏内部实现 | 写函数/ledger路径 |人工review/drift | 按agent可执行动作写 | 低 |
| preset文本测试脆弱 | grep instructions | test review | 只测结构化语义/runtime | 低 |

---

## 8. Unit 串行依赖图

```text
Unit 1：Exec 主动失败进入 attempt loop
  ↓ 使用“Exec failed 是 retryable disposition”的已验证能力
Unit 2：fresh executor + prior experience
  ↓ 使用“下一 attempt 确实会被调度”的已验证能力
Unit 3：heartbeat + aggregate budget
  ↓ 使用“attempt 数和生命周期已确定”的已验证能力
Unit 4：parallel-forge budget / exhausted fan-in / docs
```

- Unit 2 不能先于 Unit 1：否则没有可靠的主动失败 retry 触发点，fresh-process测试只能覆盖 timeout。
- Unit 3 不能先于 Unit 1/2：aggregate公式必须按实际 attempts 和 permit生命周期计算。
- Unit 4 最后：preset只有在机制与时钟已通过时才能安全启用budget=2。
- 每个 Unit 禁止为后续 Unit 提前改 preset、文档或 timeout；通过文件变更表和独立提交检查防止跨界。

---

## 9. 执行命令清单

| 时机 | 命令 | 验证目的 | 预期结果 | 失败后能否继续 |
|---|---|---|---|---|
| 开工前 | `git status --short` | 保留用户变更 | 明确dirty范围 | 否，若覆盖目标文件先停 |
| Unit 1 Red/Green | `cargo nextest run -p ralph-core -- worker_outcome` | reason/truth table | Red后Green | 否 |
| Unit 1 | `cargo nextest run -p ralph-cli --bin ralph -- executor_failed_terminal` | attempt disposition | 全绿 | 否 |
| Unit 1回归 | `cargo nextest run -p ralph-cli --bin ralph -- slot_retry` | 旧retry语义 | 全绿 | 否 |
| Unit 2 | `cargo nextest run -p ralph-core -- wave_prompt` | retry prompt | 全绿 | 否 |
| Unit 2 | `cargo nextest run -p ralph-cli --bin ralph -- executor_retry_uses_fresh_pid` | fresh process/same cwd | 全绿 | 否 |
| Unit 2污染 | `RALPH_CURRENT_HAT=executor RALPH_EVENTS_FILE=/tmp/x.jsonl cargo nextest run -p ralph-cli --bin ralph -- executor_retry_uses_fresh_pid` | env scrub | 全绿 | 否 |
| Unit 3 | `cargo nextest run -p ralph-cli --bin ralph -- aggregate_floor` | timeout公式 | 全绿 | 否 |
| Unit 3 | `cargo nextest run -p ralph-cli --bin ralph -- heartbeat` | lease回归 | 全绿 | 否 |
| Unit 3 | `cargo nextest run -p ralph-cli --bin ralph -- global_deadline` | global优先 | 全绿 | 否 |
| Unit 3 race | `cargo nextest run -p ralph-cli --bin ralph -- partial_timeout_events_visible` | partial timeout回归 | 全绿 | 否 |
| Unit 4 supervisor | `cargo nextest run -p ralph-cli --bin ralph -- parallel_forge_executor_retry_exhaustion` | 三次attempt与fan-in | 全绿 | 否 |
| Unit 4 BDD回归 | `cargo nextest run -p ralph-core --test scenarios -- parallel_forge_exec_wave_failed` | exhausted→correction | 全绿 | 否 |
| preset lint | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | CLI preset lint | 全绿 | 否 |
| preset core | `cargo nextest run -p ralph-core -- preset_lint` | core lint | 全绿 | 否 |
| embedded preset | `cargo nextest run -p ralph-cli --bin ralph -- presets` | manifest/embed/schema parity | 全绿 | 否 |
| guide drift | `scripts/check-cli-doc-drift.sh` | agent guide同步 | exit 0 | 否 |
| 格式 | `cargo fmt --check` | Rust格式 | exit 0 | 否 |
| 构建 | `cargo build` | workspace build | exit 0 | 否 |
| lint | `cargo clippy` | workspace lint | exit 0 | 否 |
| doctest | `cargo test --workspace --exclude ralph-e2e --doc` | 允许的doc例外 | 全绿 | 否 |
| E2E mock | `cargo run -p ralph-e2e -- --mock` | 核心用户路径 | exit 0 | 否 |
| 最终全量 | `./scripts/run-tests.sh` | 两阶段nextest+doctest | 全绿 | 否 |
| flake兜底，仅必要 | `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | 判定race flake | 全绿 | serial仍失败则否 |

不得用裸 `cargo test -p ralph-cli`。不得只跑局部测试后声明完成。

---

## 10. 最终质量门禁

- S1–S11 全部通过并可追踪到测试与 Unit；
- R1–R11 均有 executable test；
- attempts：budget0=1、budget1=2、budget2=3；
- Exec failed-terminal retry、non-Exec regression通过；
- fresh backend PID不同、cwd/slot identity相同；
- retry prompt包含稳定码、bounded detail、artifact盘点与禁止破坏性动作；
- 中间 attempt 对 store/main ledger/task/RPC终态零泄漏；
- third failure exactly one `exec.wave.failed`；
- heartbeat Strong/Weak/None、weak cap、startup grace、hard cap保持；
- aggregate batches×attempts floor和global priority通过；
- Characterization/State-machine/Idempotency/Fault Injection通过；
- parallel-forge real EventLoop BDD通过；
- preset/schema/agent guide/operator reference同步；
- preset lint三组、docs drift、build、clippy、fmt、doctest、mock E2E、全量通过；
- 无 skip、`.only`、忽略、弱化断言、无解释 snapshot；
- 无新增依赖/migration/CLI/topic；
- 无未处理 BLOCKED 决策，所有关键决策≥0.85；
- 实际变更不超文件范围；若超出必须先更新Evidence/Decision/Plan；
- 四个 Unit 严格串行，每个形成 Acceptance Red→Unit Red→Green→Refactor→Integration→Regression→Close；
- 最终人工审阅 `git diff --stat` 和 `git diff`，确认没有 `.ralph/` runtime状态或ephemeral文件进入提交。

---

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 四个纵向可观察 Unit，每个有Red/Green/集成/回归 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D15已固定层、类型语义、公式、边界、测试 |
| 所有文件和接口是否有代码库证据 | 是 | E1–E25；新增类型/fixture均明确标“计划新增” |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | 最低D9=0.90 |
| 是否存在未处理的低置信度假设 | 否 | V1–V3是Unit内验证断言，不改变设计 |
| 每个 Unit 是否只有一个可观察行为 | 是 | retry分类、经验继承、租约预算、preset耗尽合同各一项 |
| 每个 Unit 是否可以独立验证 | 是 | 各Unit有固定targeted命令与提交边界 |
| 每个 Unit 是否有真实 Red | 是 | 每Unit第10项给出实际缺失能力与预期失败 |
| 每个 Unit 是否包含回归范围 | 是 | 每Unit第16项 |
| 是否存在未来 Unit 依赖 | 否 | 仅后Unit依赖已完成前Unit；每Unit列出禁止未来能力 |
| 是否存在泛化任务描述 | 否 | 修改位置、函数职责、测试名、断言、命令均具体 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 第5、6节矩阵 |
| 所有关键决策是否有 Evidence | 是 | D1–D15均引用E编号 |
| 计划是否可以严格串行执行 | 是 | 第7、8节线性依赖 |

### 计划后置信度复核

- **目标与范围：0.97。** 用户已确认三次、fresh executor、经验继承、heartbeat idle续租和hard/global上限。
- **机制定位：0.96。** `exec.unit.failed → Completed(Failed)` 与 outer deadline均有直接代码证据。
- **实现位置：0.95。** 复用现有 attempt loop、prompt builder和timeout helper，无新架构。
- **测试可执行性：0.93。** 已存在 fake backend、paused-time、supervisor和real EventLoop测试模式；PID fixture仍需Unit 2验证，但不改变技术选择。
- **回归边界：0.94。** 受影响调用方、preset/schema/docs入口已枚举。
- **总体：0.95，READY。**
