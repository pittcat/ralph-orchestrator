---
title: Parallel Forge DAG P1 Closure - Plan
type: fix
date: 2026-09-09
execution: code
baseline: c11309acef260f5fa812d0f42771300e847d7fcc
reviewed_head: e72df6aa7540f35d866986fea12964dff0b4f0f4
deepened: 2026-09-09
origin: docs/reviews/2026-09-09-parallel-forge-dag-completion-red-team-review.md
planning_readiness: READY
artifact_readiness: implementation-ready
---

# Parallel Forge DAG P1 Closure - Plan

## 0. 计划状态

**READY（实施决策已确定；不是实现验收通过）。** 覆盖报告 F01–F10：报告实际为 P0 0 项、P1 10 项。用户已确认此范围；F11–F13 的独立 P2 改进不纳入。P1 所必需的 preset、schema、注入指南和 operator skill 同步属于本计划。

- 基线：`c11309acef260f5fa812d0f42771300e847d7fcc`。调查开始时仅原始 review 文档未跟踪；没有生产文件修改。
- 写作期间其他会话提交review与本计划初稿，HEAD前进到`e72df6aa7540f35d866986fea12964dff0b4f0f4`；`git diff --name-only c11309ac HEAD`仅包含这两份文档。本轮复核修订保留在工作区，未执行commit。源码基线未变，无需重跑同一组定向测试。
- 调查：CLI 主循环、DAG admission/spawn/recovery/integration、SQLite jobs/receipts/intent、EventLoop 投影顺序、worktree、PTY、preset/schema、skill 注入、测试入口与历史。
- 已执行：`cargo nextest --version` = 0.9.140；带七项环境清理的 `cargo nextest run -p ralph-cli --bin ralph -- dag_scheduler`：159 passed / 1951 skipped，run ID `7e50f1de-d450-4b4d-8448-9d97175da52a`。
- 已执行隔离 Git 实验：临时仓库中 update-ref 后文件仍为旧值；read-tree 两树更新使 index/文件一致；冲突的未提交改动被拒绝且保留。临时目录已删除。
- 未执行：新增测试的 Red/Green、完整 workspace、clippy、mock E2E、正式 CLI 跨进程 crash matrix、真实 AI loop。本计划不将报告的旧 harness 结果冒充本轮实验，也不声称预测 Red 已实际发生。
- 阻塞项：无实施方向阻塞。执行时出现第 7 节停止条件必须重新调查，不能用本页 READY 覆盖新证据。

本文按用户要求采用 0–11 节与每 Unit 20 项结构；不声明 `ce-unified-plan/v1`，避免将不同标题结构冒充该格式。Unit ID 稳定；进度记录在执行证据或 Git 中，不在计划里维护完成勾选。

## 1. 功能目标

**业务目标：** 经批准的 DAG 计划可以持续调度，已提交成果进入审查、验证及目标工作区；在本计划指定的持久化断点重启后，剩余工作继续推进且不重复启动已确认完成的 job。

**调用方：** 启动/恢复 `ralph run` 的 operator；executor/reviewer/verifier/fixer job；接收 `forge.exec.development.done` 的 tester；通过 task projection 观察 Unit 状态的调用方。

**当前行为与差异：** F01–F10 的当前机制及定位见 E2–E13。目标是把“局部 helper 已实现”转为正式调用链可观察的交付；不改变 DAG runtime 对 admission、集成及 task-close 的所有权。

### 需求

| Requirement ID | 可观察行为 | 来源 |
|---|---|---|
| R1 | 有空执行槽且存在 Ready Unit 时，下一个控制面 tick 可启动该 Unit；已结束 Unit 不再占 job pool | F01 |
| R2 | 同一资源域内持有 permits 总和不超 capacity；资源跨 review/verify/correction 保留，到 integrated ack 或确定的最终终止才释放 | F01 |
| R3 | executor/fixer 正常提交后，下一阶段在同一可信 worktree 的已接受 HEAD 工作；不回退代码 | F02 |
| R4 | 集成成功被确认前，目标 ref、index、tracked files 均对应 candidate；本地改动不被覆盖 | F04 |
| R5 | 依赖解锁后首次启动的 Unit，其固定 base 包含全部已确认集成依赖；恢复不重新选择 base | F03 |
| R6 | accepted terminal 与下一阶段 reserve 之间崩溃，恢复只启动缺失阶段，历史 job 不重跑 | F05.1 |
| R7 | tested intent 已持久化的 CAS 前后窗口可重放同一 candidate；不重新生成不同 candidate 掩盖丢失 record | F05.2 |
| R8 | development.done 写入意图后 append 失败或进程退出，恢复最终交付一个逻辑事件；完整主账本记录至多一条 | F05.3 |
| R9 | plan-ready 的 task 投影开始前已有可恢复 receipt；receipt 写入失败则不进行该次投影 | F06 |
| R10 | 已接纳 approval 对 receipt/plan/base 的激活全成或全不成；中断后可从已接纳证据完成激活 | F06 |
| R11 | execute failed 的合法 correction 获得一个新 fixer attempt；重复请求不重复消耗预算 | F07 |
| R12 | integration failed 经合法 correction 可修复并重新验证集成；verify accepted 本身不是 fixer 授权 | F07 |
| R13 | gate 挂起或持续输出时在有界时间、内存内失败，回收其进程组，不推进目标分支 | F08 |
| R14 | gate 等待期间其他 job completion 与 Ready 补槽继续处理；取消能结束 gate | F08 |
| R15 | 每种 DAG job 获得当前 Unit、可信输入 artifact、前阶段证据、唯一资源身份和可执行 emit 指南，无须猜测 wave/trigger/projection 输入 | F09 |
| R16 | 上述能力由正式 EventLoop/DagSchedulerRuntime/SQLite/Git 路径证明，至少一条完整 CLI 主路径与指定恢复窗口不绕过调度 | F10 |

**输入：** 经现有 handoff 校验的 execution-plan artifact；accepted approval/correction；当前 job 的身份元组与结果；Git 对象；已有 pools、resource claims、hat timeout。

**输出与状态：** durable job/Unit/lease/intent/receipt 状态、私有 job 结果与主账本协调事件、实际目标文件、task close。只有 accepted integrated projection + durable ack 才解锁依赖和允许 development.done。

**错误语义：** store/identity/HEAD 不一致不吞错、不视作成功；返回现有 `DagStoreError` / `IntegrationError` / `LaneError` 分类，增加稳定 reason（见决策）；容量不足是 durable pending；预算耗尽或无法验证事实是 durable blocked。失败本身不授权删除工作区或重跑不明进程。

**兼容：** wave/dag_shadow 无新执行副作用；默认 mode=wave 不变。旧 SQLite 只前向迁移，保留 wave 数据；旧活动 DAG 缺少可信新字段时，只从可核验旧 journal/Git/accepted evidence 重建，否则明确 blocked。不得猜测 base、伪造 accepted 记录或静默清库。无需兼容旧内部 Rust 接口；不增加 CLI 参数、不改公开业务 topic 集合。

**性能：** job pool 与 resource lease 分开计量；每次 tick 不等待整个 gate；gate 全命令集 wall-clock 上限沿用 verifier hat timeout（缺省 3600 秒），输出每路保留最多 64 KiB tail，最终 reason 最多现有 500 字符。测试用注入的短 deadline 与握手控制，不扩大 timeout 掩盖阻塞。

**权限：** 延续 artifact/path-policy、job token、source hat、loop identity 校验；reviewer 不改代码；agent 不读写 SQLite/内部 ledger 来决定调度。所有用户代码、未知 worktree 和外部资源禁止自动覆盖或接管。

**非目标：** F11 inspect 指标、F12 独立词汇/负例整体升级、F13 inspect 只读连接；分布式 scheduler、Windows 新进程平台、重写通用 EventLoop、重新设计任务 API、真实 AI 质量评测、性能调优项目。

**约束与假设：** 已确认项目使用 SQLite + Tokio + Unix process groups + Git；无需新依赖。Git/文件系统不是同一事务，必须通过 intent 和状态核验收敛，不能宣称物理跨介质原子性。gate 对外部服务的副作用不由 Ralph 回滚。任意第三方同时修改同一工作区不属于受控 writer；检测到则停止交付。不存在尚未确认且被当作实施前提的假设。

## 2. 代码库现状与证据

### 2.1 当前实现入口

`loop_runner/inner.rs` 调用 EventLoop 处理 JSONL，再把 accepted_events 交给 `DagSchedulerRuntime::observe_accepted_events`。DAG 内部由 `tick → maybe_spawn_executor → spawn_job` 启动 PTY；结果经 completion queue、main ledger、真实 EventLoop acceptance 回到 `observe_unit_event_dag`。Verify accepted 进入 `queue_integration → maybe_integrate_one → IntegrationOrchestrator::integrate`；集成事件读回后 ack task projection。

数据边界是 `CanonicalPlanHandoff`、`JobIdentity`、SQLite DAG 表、可信 worktree、`IntegrationIntent`、JSONL；外部依赖为本机 Git、SQLite 和 job/gate 子进程，不需要网络 API。现有测试为 Rust unit/integration、nextest、真实 Git TempDir 与假 backend 脚本；core YAML BDD 必须使用真实 runner。

### 2.2 Evidence Ledger

下文用路径别名缩短 Unit 清单：**D** = `crates/ralph-cli/src/loop_runner/dag_scheduler/`，**S** = `crates/ralph-core/src/supervisor/`；这是已确认目录，不是新增层。所有“计划新增”路径尚不存在。

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | Git HEAD/status；review 原文 | 基线一致；报告 P1 共十项 | 按 F01–F10 制定修复范围 | 高 |
| E2 | D/mod.rs::tick；S/dag_scheduler.rs::compute_admissions | 候选包含全量 units；计数从零开始 | U1 修生产 snapshot，不只改纯函数测试 | 高 |
| E3 | S/migrations/v14.sql；S/dag_store_rusqlite/jobs.rs::reserve_job | lease 表已有；reserve 未获取该资源表 permits | U2 复用表并把 lease+reserve 放同一事务 | 高 |
| E4 | D/jobs.rs::JobPipeline::{advance,release,restore_unit} | pool 是进程内计量；stage 与 fixer 有独立补充计数 | 不将跨阶段 resource lease 当 job slot | 高 |
| E5 | D/spawn.rs::spawn_job；D/worktree.rs::acquire；D/worktree/resume.rs::resume | 所有 stage acquire 初始 base；resume 可校验 expected HEAD/ancestor/clean/common-dir | U3 固定可信成果而非放宽 acquire | 高 |
| E6 | D/mod.rs::on_concurrency_approved；D/integrate.rs::integrate_unit | plan-level base 被所有 Unit 启动及 diff 重用 | U5 增 per-Unit base 并同步 diff 消费方 | 高 |
| E7 | S/integration_lane.rs::compare_and_swap_ff；本轮临时 Git 实验 | update-ref 仅推进 ref；两树 checkout 可收敛并拒冲突 dirty path | U4 明确工作区物化与恢复判断 | 高 |
| E8 | D/mod.rs::recover_after_restart | terminal job 被跳过；仅 verify 有补 queue 路径 | U6 以 current durable job 统一推进后继 | 高 |
| E9 | D/integration.rs::integrate；S/dag_store_rusqlite.rs::{prepare_intent,get_intent} | intent 先 CAS 后 record；CLI 正式恢复未消费 intent | U7 按原 intent 判别 before/after CAS | 高 |
| E10 | D/integrate.rs::{maybe_emit_development_done,reconcile_after_restart} | fence 赢后 append 失败只 warn；重启 fence 无 event blocked | U8 用可重放投递状态，不能把 fence 当送达 | 高 |
| E11 | inner.rs::process_events_from_jsonl_with_waves 调用；core event_loop/parse_and_emit/legacy.rs state projection 段 | projector.apply 早于 CLI receipt；也早于后续 unified policy | U9 前置 receipt 不能被当成最终 accepted 授权 | 高 |
| E12 | D/mod.rs::on_correction_requested；S/dag_store_rusqlite/jobs.rs::reserve_job | execute failed→fix、verify accepted 后 integration failed→fix 不在合法表 | U11/U12 显式失败事实，不放开任意转移 | 高 |
| E13 | D/integrate.rs::maybe_integrate_one；S/integration_lane.rs::run_gate_commands_in | tick 同步 gate；Command::output 无 deadline/输出限额 | U13/U14 分别约束子进程与控制面 | 高 |
| E14 | D/spawn.rs::build_job_prompt；presets/en/parallel-forge.yml executor/reviewer/verifier | 缺 artifact/前阶段证据，要求不存在的 wave 输入 | U15 结构化 job context + preset 同步 | 高 |
| E15 | D/spawn.rs::{verify_terminal_releases_slot_for_ready_sibling,real_dag_executor_canary_runs_in_git_worktree}；crates/ralph-cli/tests/integration_dag_scheduler.rs | 测试绕过 tick 或停于 merge queue；CLI 测试主要 inspect | R16 贯穿每 Unit，补完整正式路径 | 高 |
| E16 | core tests/scenarios/parallel_forge_dag_resume_runtime.yml；tests/scenarios.rs | 原 fixture 不启用真实 CLI DAG 恢复 | 不把现有同名 BDD 当 crash matrix | 高 |
| E17 | crates/ralph-cli/Cargo.toml；crates/ralph-core/Cargo.toml；scripts/run-tests.sh | CLI 默认 supervisor-db；core 默认无；nextest 两阶段脚本 | 同时覆盖 feature 开/关；最终不裸跑 workspace nextest | 高 |
| E18 | 本轮 nextest run ID 7e50f1de-d450-4b4d-8448-9d97175da52a | 159 全绿但未覆盖新 acceptance | 可重复开发基线，不能替代 Red | 高 |
| E19 | S/migrations.rs::CURRENT_VERSION；S/dag_store_rusqlite.rs::DagConnection | 当前 v17；共享连接；事务/版本拒绝已有模式 | 顺序新增 v18–v23，保持前向迁移 | 高 |
| E20 | core event_loop/accepted_transition.rs；core file_lock.rs | 已有 durable-before-publish、幂等 replay、try_exclusive 模式 | 复用思路和锁，不把 task projection 冒充已有原子 outbox | 高 |
| E21 | core skill_registry.rs::{register_builtin,is_hat_eligible}；event_loop/prompt_types.rs | OPAC 注册与 hat 可见性已有实现 | U15 复用 registry，不复制 skill 正文 | 高 |
| E22 | Git c11309ac、7b05aec4、a84bfce5、a3aba2b6、b842d593 | 已修 path policy、macOS 路径及部分 receipt recovery 测试 | 保留这些负例，不重复列旧风险 | 中（历史） |
| E23 | docs/solutions/workflow-orchestration/parallel-forge-preset-integration-gap.md | 历史 wave 契约已变；其跨层同步经验仍相关 | 只借鉴同步检查，不继承旧 topic/task-close 结论 | 中 |
| E24 | wc -l | inner.rs 4863、legacy.rs 4418、D/spawn.rs 2136、S/dag_store_rusqlite.rs 2198 | inner 只加调用；新增行为按职责独立模块，所有源码 <5000 行 | 高 |
| E25 | [Git read-tree 文档](https://git-scm.com/docs/git-read-tree)、[Git update-ref 文档](https://git-scm.com/docs/git-update-ref) + E7 | 两树 merge 的 -u 更新 worktree；不用 --reset 覆盖本地变化 | 仅支持 U4 Git 操作选择，不证明整个恢复实现 | 高 |
| E26 | S/merge_sink.rs::FileEventMergeSink::append_events；CLI commands/emit/command_impl.rs 实际 append；wave/dispatcher/coordination.rs::append_supervisor_coord_event | 三个实际主账本写入口；不能只为新writer加锁便宣称所有写者协调 | U8同步锁协议与消费者回归 | 高 |
| E27 | core event_loop/parse_and_emit/legacy.rs pending_publish loop；event_loop/state_machine_stage.rs::commit_state_machine_projection | 最终survivor才写AcceptedTransition；activation_id当前含iteration；无compiled contract有direct publish分支 | U9的accepted标记不能在较早accepted_events局部数组处写；测试须加载真实contract | 高 |
| E28 | CLI loop_runner/{entry,inner,hat_channel}.rs、loop_runner/wave/io.rs、loop_runner/wave/dispatcher/{salvage,dispatch}.rs、wave.rs；core event_loop/dispatch_and_handoff.rs::persist_system_injected_jsonl_event | 除E26外还有启动、guidance、hat归并、wave归并、恢复及系统事件写入主账本；recovery.jsonl/history/scratchpad是不同文件 | U8共享写锁必须覆盖这些真实入口，不以单线程假设替代跨进程协调 | 高 |

### 2.3 受影响范围

- 生产：D 下 mod/spawn/jobs/recovery/integrate/integration/worktree；S 下 dag_scheduler/dag_store_rusqlite/jobs/dag_integration/migrations；core EventLoop legacy 接入；skill registry 消费。
- 测试：现有 D inline tests、S inline tests、CLI `integration_dag_scheduler.rs`、`tests/common/mod.rs`；core `tests/scenarios.rs` 及真实 BDD fixture。
- 配置：现有 `scheduler_mode`、`dag_pools`、hat timeout；不新增配置字段。U15 修改 builtin instructions；schema 需核验输入说明；当前未发现 DAG slot_index 必填字段，不预设删除字段（逐字段见 U15）。
- 数据：新增有界 DAG 记录及迁移；保持旧 wave 表。内部 Rust trait/struct 消费方需同步编译；无 HTTP API/UI 变化，无外部服务协议变化。
- 文档：U15 的 preset/operator 同步清单。仅运行态细节变化的 U1–U14 不向 agent 注入内部存储知识。
- 构建：ralph-cli 默认/无默认 feature，ralph-core 默认/supervisor-db，全 workspace 最终基线。

## 3. 决策记录与置信度

评分是计划判断强度，不是成功概率：直接实现/调用方证据 0.40；已有 executable 基线或隔离实验 0.15；现有模式 0.15；主要失败边界覆盖 0.15；兼容与反证检查 0.15。每项均有直接源码；没有把未运行的新测试计为通过。低于 0.85 时执行必须停止重新调查。

| Decision ID | 问题、候选方案 | 最终选择 | 支持证据 / 排除理由 | 置信度 |
|---|---|---|---|---|
| D1 | 过滤已完成还是增大 cap | 只调度无当前 job 的 Ready Unit；snapshot 携带已有 global/stage 占用，阶段接续优先；durable current job 为真相 | E2/E4/E18；增大 cap 仍会饿死，spawn 后去重太迟 | 0.94 |
| D2 | resource 每 tick 计数还是持久 lease | 复用 dag_resource_leases；同一 IMMEDIATE 事务验证 cap、获取 claims、reserve 首 job；幂等持有/释放 | E3/E19；只用 HashMap 丢失跨阶段/重启事实 | 0.91 |
| D3 | 放宽 acquire 还是严格 resume | 首次 acquire；后续用 per-Unit base + runtime 核验并持久化的上阶段 HEAD 调 resume | E5/E18；信任当前任意 branch tip 会接纳篡改 | 0.94 |
| D4 | reset --hard、另设交付分支、或物化当前目标 | 保留当前 target；先 durable intent，CAS 后两树 read-tree -m -u 物化，验证 clean 后才 record/emit；dirty/未知状态 blocked | E7/E9/E25；reset 可能覆盖用户变化，换 target 改变 operator 交付接口 | 0.92 |
| D5 | approval base 或 admission base | 首次 admission 读取当前可信 target，验证全部依赖 ack 的 integrated commit 为 ancestor，事务固定 unit_base；diff/resume 共用它 | E6/E9；仅等待依赖不提供依赖代码 | 0.91 |
| D6 | 重放所有历史 job 或 current-job 推进 | 只读取 dag_units 当前身份 + terminal/evidence，live/recovery 共用 reconcile，幂等 reserve 缺失后继 | E8/E12/E18；遍历旧 accepted 会复活过期 attempt | 0.91 |
| D7 | 重建 candidate 或消费原 intent | 核对 identity/tree/current target；expected 时执行原 CAS，candidate 时补物化/record；其他 target 不猜测 | E9/E7；重建会触发 intent drift 并失去已测试身份 | 0.90 |
| D8 | fence-only 或可重放投递 | 新增 bounded terminal delivery 记录；prepare→append-once+fsync→delivered；主账本按稳定 key 核验，失败保留 pending | E10/E20/E26/E28；SQLite与JSONL无共享事务，需幂等外部写；已扩大写者调查并将全部已定位主账本入口纳入锁协议 | 0.87 |
| D9 | CLI 事后 receipt 或 core 投影前 receipt | 在 DAG plan-ready 的实际 StateProjector.apply 前写 candidate receipt；最终 accepted 后记 accepted evidence。candidate 永不单独授权 approval | E11/E20；不能把仍会被后续 policy 拒绝的事件当 accepted | 0.87 |
| D10 | 多 helper 串写或一个激活事务 | 在同连接一次事务中校验 accepted receipt、登记 plan/units、pin approval base、激活 receipt/target；已接受 approval receipt 可重放 | E11/E19；SQLite helper 各自原子不等于组合原子 | 0.90 |
| D11 | 放开 transition 或受授权 correction | execute failed + 当前 failure fingerprint + accepted correction 才 fix；预算事务扣一次；满池 durable pending，耗尽 blocked | E12/E4；任意 execute→fix 绕过失败与预算 | 0.91 |
| D12 | verify accepted 直接 fix 或集成失败状态 | 单独 durable integration-failed facts，合法 correction 消费该事实；候选 intent 按 attempt 隔离 | E9/E12；不能覆盖 accepted verify，不能复用旧候选 intent | 0.90 |
| D13 | output()/外层 timeout 或受控进程 runner | 新 core gate runner 管 process group、双流 bounded tail、总 deadline、cancel、wait；timeout=verifier hat timeout，缺省 3600 | E13/E17；外层 timeout 不能终止 blocking output 及后代 | 0.90 |
| D14 | 同步 tick 或异步 completion | 把整个 blocking integration 操作交给受控 worker，单 target 一个 active，tick 只 start/poll/cancel；结果带 generation | E13/E4；仅 spawn_blocking 后立刻 await 仍阻塞调度 | 0.91 |
| D15 | 伪造 wave 字段或 DAG 独立上下文 | 新 typed JobContext；当前 cwd + runtime identity +已核验 artifact/evidence；preset 改读该上下文；registry 提供 OPAC | E14/E21；伪造 slot/wave/map 延续错误契约 | 0.92 |
| D16 | 全 E2E 或分层正式入口 | 纯规则最小单测 + 真 DB/Git/runtime integration；CLI mock backend 完整主路径/重启少量 E2E | E15/E16/E18；helper-only 无法捕获报告反例，全真实 AI 不稳定且昂贵 | 0.94 |

### 实施协议（各 Unit 不得自行更换）

**新增数据合同（均为计划新增，不是已有表）。** 下列字段组是最低且确定的持久化形状；实现者可选择Rust字段排列，不能改变主键、状态及证据来源。身份/path字段必须使用现有校验，SHA为40/64 hex，hash为64 hex，timestamp为INTEGER毫秒，计数非负。artifact路径上限4096字节、单job最多16个引用、kind白名单；超过即拒绝，不截断身份字段。

| 迁移 / owning Unit | 表与唯一性 | 必须保存的字段 / 状态 | 旧数据处理 |
|---|---|---|---|
| v18 / U3 | dag_unit_bases：unit_key主键；dag_stage_evidence：job_id+token主键并关联完整JobIdentity | bases存plan_key/base_commit/created_at_ms；evidence存input_head/output_head/base_commit/terminal/result_digest及有界artifact refs | 新表空；旧current job只凭核验后的可信结果补证据，否则blocked |
| v19 / U4 | dag_checkout_intents：unit_key+target_branch+generation主键；同Unit/target最多一个非terminal generation | expected_head/candidate_head/candidate_tree/unit_commit/base_commit、worktree canonical identity、state=prepared/ref_advanced/materialized/superseded/blocked | 将旧intent完整内容作为generation=0导入，不修改其candidate；归属不唯一则blocked |
| v20 / U8 | dag_terminal_deliveries：plan_key+topic主键，delivery_key唯一 | artifact_digest、固定payload字段、固定timestamp、event_file identity、append_offset、serialized_line_digest、state=prepared/appending/delivered/blocked | 旧fence只有all-ack及一致plan证据可导入；绝不delete fence解锁 |
| v21 / U9 | dag_registration_evidence：plan_key+loop_id主键；dag_approval_evidence：plan_key+loop_id+approval_digest主键 | candidate来源hat/contract revision、artifact path/digest、accepted transition引用、projection_complete；approval存target/base/approved、accepted transition引用 | receipt原状态保留；没有accepted证据不推导approved |
| v22 / U11 | dag_correction_requests：unit_key+failure_fingerprint主键 | failed_job_id/token/attempt、correction_digest、bounded feedback path/hash、state=pending/reserved/blocked、reserved_job_id、reserved_attempt；budget由已reserve fixer attempts计算 | 不按收到事件次数扣预算；历史attempt只读核验 |
| v23 / U12 | dag_integration_failures：unit_key+attempt+generation主键；attempt与checkout generation关联 | 原verify identity、failure_class、observation path/hash、candidate引用、consumed correction key；新intent关联当前attempt | 旧intent归属需唯一plan与attempt证据；未知不自动绑定 |

U7的supersede不是覆盖旧candidate：先把原intent完整保留在v19 generation记录，再在同事务内条件替换旧active intent映射；仅允许target仍为expected且证明CAS未落地的generation。所有get_intent/prepare_intent/record/ack消费者同步使用plan-qualified unit_key；API字段名称即使暂沿用unit_id，也不得传裸U-ID。U12在此基础上再加attempt授权，v19历史不可删除。

U7还必须关闭“integrated事件已存在但ack未写”的相邻窗口：用durable integration record生成可信投影输入，核验loop/task_key/commit后对真实StateProjector幂等重放close，再写ack；任务已正确closed时是no-op。不能从未经验证的JSONL声称projection成功；不能因事件存在便直接ack。测试分别覆盖task open与task已closed。

U8固定序列化timestamp及payload以便重建原始行。在目标FileLock下确定append_offset，并保存appending证据后写入；DB事务只用于短元数据写，不持DB mutex等文件锁。重启时仅当文件identity、offset、此前完整前缀以及现有tail字节均与预定行匹配，才追加缺失后缀完成该行；不truncate、不删除其他字节。若已有后续完整行或无法证明tail归属，blocked。完整行存在时直接标delivered，不再append。E26/E28列出的主账本writer均使用相同的FileLock::new(events_path)协议，锁覆盖tail核验、整批写入和flush；先解析为相同的canonical父目录与文件名以避免路径别名生成不同锁。普通writer遇到非空且无换行的tail返回InvalidData并保留原字节，不在未完成行后继续追加；只有delivery writer可凭持久证据补齐自己的tail。DAG控制面使用try_exclusive，忙则pending；原同步writer复用短时exclusive。不改变其他writer的payload/路由及原有错误传播语义，也不为它们增加dedup。其他不遵守协议的writer视为外部改动并拒绝修复。

U9的最终accepted证据接入点是legacy.rs的最终pending_publish/AcceptedTransition成功结果，不是统一policy内部临时accepted_events数组。该证据绑定durable transition_id与原始contract revision；写标记中断时可从已提交AcceptedTransition核验补齐。不存在compiled contract的测试必须先装载builtin真实配置，不能拿direct publish测试替代该边界。

U14取消与CAS有明确线性化边界：worker在target锁内以短事务将当前generation从running转commit_authorized。取消先赢则禁止CAS并回收；commit_authorized先赢则完成U4的物化/持久化闭环，再停止接纳新工作，不能在CAS后人为制造半交付。completion携带generation，控制面只接受当前generation的结果。测试分别固定两种顺序。

1. **身份与资源域：** 内部 key 统一 `forge:<plan_key>:<unit_id>`；job_id/token 包含 plan_key、stage、attempt，不能跨 plan 冲突。pool cap 是一个 loop runtime 的实际并发 job 数，resource key 在一个 DAG store 内共享；同名 resource 的 capacity 声明冲突阻止 plan 激活。拥有资源但等待后继的 Unit 不占 job pool。未知存活进程的 lease 不释放；只有证实子进程已退出/取消且 Unit 最终 blocked/failed 才释放。
2. **持久扩展与迁移编号：** U3 新增 v18（Unit base、stage accepted evidence）；U4 v19（target checkout intent/状态）；U8 v20（terminal delivery）；U9 v21（registration candidate/accepted evidence）；U11 v22（correction request/failure/预算）；U12 v23（attempt-scoped integration facts/intents）。U2 使用已有表，无新迁移；U5/U6/U10 消费前置表。迁移 SQL 均为计划新增文件 `S/migrations/v18.sql` … `v23.sql`；每次同步 CURRENT_VERSION、迁移列表、reopen/old-wave-preservation 测试。不得为了方便覆盖旧 migration。
3. **stage evidence：** 以 current JobIdentity 为主键；记录 base、runtime 读取的 clean HEAD、accepted result digest、前阶段 artifact 的相对路径/hash、terminal。路径/哈希有界并复用现有 artifact 校验；不存任意原始 prompt/payload。Evidence 与 terminal 在一个事务写入；写失败不得 release/advance。review/verify 只能确认输入 HEAD，不能偷偷变更代码。execute/fix 可推进 descendant HEAD；失败退出若 worktree 脏，不 reset，correction 标为 blocked 要求人工保全，不声称自动恢复这种未知代码状态。
4. **目标物化状态机：** verified candidate→durable checkout intent(old SHA,new SHA,old tree,new tree,target identity)→CAS→index/files synchronize→verified materialized→integration record→accepted integrated/task projection→acked。在同一 Git common-dir 的 target 专属 FileLock 下操作；lock busy 进 pending，不阻塞 tick。CAS 后失败绝不发 integrated。恢复允许 index+files 全部对应 old 或全部对应 new；混合/用户 dirty、branch 换绑、foreign repo 均 blocked。不回滚 ref、不使用 reset/clean；fail 后保留 intent 供诊断。
5. **intent 分类：** 验证 pinned base、unit HEAD、candidate tree、parent、目标身份。target=expected 重试原 tested candidate；target=candidate 补物化/record；target 为已记录并 ack 后续 runtime integration 的可证明 descendant 时只补旧 record，不倒退工作区；其他 SHA blocked。StaleExpected 只有在证明旧 candidate 从未落地时才把该 intent 标 superseded，下一次 gate 创建新 generation。每次 correction 新 attempt，不覆盖旧 intent。
6. **terminal delivery：** key=plan_key+topic+artifact digest；payload 确定化并限制为现有 done 字段。先 DB prepare，再在 target main-events 专属 FileLock 下扫描完整记录并比 key/payload；已存在则不 append。缺失时一次序列化完整行并 sync_all，之后 delivered。不可读/不一致不当 absent。自己的可确认torn tail只按上文补齐缺失后缀；未知tail阻断，不截断文件。共享writer覆盖E26/E28。严禁持SQLite事务等待文件锁；取得文件锁后允许短元数据事务，不能把长时间扫描/fsync放进SQLite事务。at-least-once retry加幂等写提供完整事件记录的逻辑exactly-once；不声称磁盘故障下一定送达，也不承诺消费者进程跨崩溃只启动一次。
7. **registration：** candidate receipt 只保存 canonical artifact identity、来源身份/loop 与投影所需有界引用；record 失败拒绝该事件的 task projection。最终 accepted 标记在真正 publish/accepted-log 边界记录。恢复扫描 pending 和 active，不只 active plans；仅 candidate 的记录必须重新经过正常 EventLoop 验证/幂等 projection，不能直接激活；拒收不能产生 active plan/job。artifact drift blocked。已接纳 approval 的 bounded receipt 在激活事务前写；恢复只有此证据才能启动激活事务。target/base 在该次 approval 固定，不能 restart 时取新 HEAD 替代。
8. **correction：** correction identity 使用当前 Unit failure fingerprint + attempt（同一失败重复 accepted 请求为 replay）；已有 3 次 fixer 上限保持。unknown Unit、无 failure、旧 token、不匹配来源请求不改变状态。事务先登记 pending 再扣预算/reserve，容量不足不扣；多个 affected Units 分别可恢复，重复 Unit 去重。integration conflict/gate failure 写独立 failure fact，不能伪装成 verify job failed；U12 不更改业务 topic。
9. **gate：** core 新 `S/gate_process.rs`；同步接口便于已有 Git port 使用，但内部 deadline/cancel/输出均有界。Unix 为每个 gate 建独立 process group；正常/失败/timeout/cancel 都 drain、wait 并回收后代。总 deadline 覆盖整个 command set；每路 64 KiB ring tail，单行超长也按字节截断；不回显环境。CLI 新 `D/integration_worker.rs`，worker 不持 `&mut DagSchedulerRuntime`、不从内部调用 EventLoop，不持 SQLite mutex 跑命令；通过带 plan/unit/attempt/generation 的消息返回，stale result 无副作用。取消传播给 runner 并 join；不把 drop JoinHandle 当取消。
10. **job context：** U15 新 `D/job_context.rs`，包含 plan/unit/task identity、job/stage/attempt、worktree/current base/expected HEAD、verified execution-plan path+digest、当前 Unit 的 acceptance/tests/path policy、accepted completion/review/verification/correction artifact 引用、success/failure schema、资源 namespace=loop+plan+unit+stage+attempt。artifact 在 host 的 `.ralph` 未被 Git worktree 自动复制：runtime 为当前 job 物化只读输入副本（含 hash），并建立可写的 Unit/attempt 输出目录；结果返回时核验并汇入 host 对应 artifact，不能给 agent 一个不存在的 host-relative 路径。复用 artifact/path 约束，防 symlink/path escape。current cwd 是唯一 worktree，不创建 worktree_map；没有 slot_index/wave_id。用 SkillRegistry 的可见性选择 OPAC/emit 指南，缺少必要能力在 spawn 前 blocked。
11. **新模块边界：** `D/admission.rs`（U1）、`S/dag_store_rusqlite/admission.rs`（U2）、`S/dag_store_rusqlite/evidence.rs`（U3）、`S/target_checkout.rs`（U4）、`D/reconcile.rs`（U6）、`D/terminal_delivery.rs`（U8）、`core event_loop/dag_registration.rs`（U9）、`S/dag_store_rusqlite/registration.rs`（U10）、`S/dag_store_rusqlite/corrections.rs`（U11）、`S/gate_process.rs`（U13）、`D/integration_worker.rs`（U14）、`D/job_context.rs`（U15）均为计划新增。按职责拆分，现有 mod.rs 显式接线；inner.rs 仅保留薄调用，不把新逻辑堆入 4863 行文件。

## 4. BDD 行为规格

共同 Background：使用真实 Git TempDir、有效 artifact（至少两个可并行 Unit，依赖场景使用 U1/U2 独立 + U3 depends_on U1）、supervisor.enabled=true、execution_mode=isolated、scheduler_mode=dag。backend 可为可控脚本；DAG、store、Git、EventLoop 不 Mock。

```gherkin
Feature: 经批准的 DAG 计划可靠推进与交付

  Scenario: S1 空槽由未启动的 Ready Unit 补上
    Given pool cap 为 1 且先序 Unit 已完成当前 job
    When 正式 tick 处理后继与其他 Ready Unit
    Then 已完成 Unit 不重复占槽且可用槽启动符合条件的工作

  Scenario: S2 资源持有跨阶段且终态后释放
    Given U1 持有 capacity=1 的资源且正在 review
    When U2 请求同一资源并在 U1 integrated ack 后再次 tick
    Then ack 前 U2 不启动且 ack 后 U2 可启动

  Scenario: S3 下一阶段接续已提交的成果
    Given executor 在 Unit worktree 提交 C 并被 EventLoop 接纳
    When runtime 启动 reviewer
    Then reviewer 的 HEAD 为 C 且文件及初始 base 不被重写

  Scenario: S4 集成成功意味着目标工作区已物化
    Given tested candidate C 基于目标 B 且目标干净
    When 正式集成完成
    Then target ref 和 index 与 tracked files 均对应 C 才发布 integrated

  Scenario: S5 后继的输入包含已集成依赖
    Given U3 depends_on U1 且 U1 的 commit C 已集成并 ack
    When 首次 admission 启动 U3
    Then U3 的 pinned base 包含 C 且能读取 U1 新增文件

  Scenario: S6 terminal 到下一阶段之间中断可恢复
    Given current job accepted terminal 与可信 evidence 已提交且后继未 reserve
    When 新 runtime 从 SQLite 恢复
    Then 只 reserve 缺失后继且重启两次仍无重复 job

  Scenario: S7 CAS 完成但 integration record 缺失
    Given tested intent 已持久化且目标已变为 candidate
    When 新 runtime 恢复
    Then 复用原 candidate 补齐物化及 record 且不再次 squash 或重做 CAS

  Scenario: S8 done 投递可重放且不重复
    Given 全部 Unit 已 ack 且 done delivery 已 prepare
    When append 失败后重试或在 append 后 delivered 前重启
    Then 最终主账本恰有一条完整 done 且重复恢复无新增条目

  Scenario: S9 plan-ready 的任务投影具有预写 receipt
    Given 有效 plan-ready 即将进入真实 StateProjector
    When receipt 写成功后在 task projection 前后中断
    Then 重启能重验该 plan 并幂等补齐任务且未获 approval 时不启动 job

  Scenario: S10 已接纳 approval 激活不出现半状态
    Given accepted approval 已持久化
    When 激活事务中注入写失败并重启
    Then 失败时无部分 active 状态且恢复后 receipt plan base 同时有效

  Scenario: S11 execute failed 的 correction 合法且有界
    Given 当前 executor failed 且 worktree clean 可核验
    When accepted correction 重复到达
    Then 仅启动一个新 fixer attempt 且预算仅扣一次

  Scenario: S12 integration failed 的 correction 不越权
    Given verify accepted 后 gate 或集成冲突记录为失败
    When accepted correction 启动 fixer 并重新 review verify
    Then 使用新 attempt 的 candidate 再集成且旧 intent 不被覆盖

  Scenario: S13 挂起 gate 被有界终止
    Given gate 启动后代并持续输出或不退出
    When 总 deadline 到达
    Then gate 及后代退出且 bounded failure 返回且目标 ref 不变

  Scenario: S14 gate 等待不暂停 scheduler
    Given U1 gate 正等待握手且 U2 job 刚完成且 U3 Ready
    When scheduler 接收 U2 completion
    Then 在放行 gate 前 U2 后继或 U3 已获得空槽

  Scenario: S15 DAG agent 可从完整上下文完成任务
    Given 当前 stage 的 artifact 输入与 schema 已核验
    When 正式 PTY job 读取 context 并执行 stub backend 行为
    Then 它能读取当前 Unit 和前阶段证据并按 policy-check emit 正确结果

  Scenario: S16 伪造或过期事实不推进状态
    Given wrong token 或跨 plan 身份或 artifact hash 不符
    When 结果或 correction 被送入正式入口
    Then 无新 job 无 task close 无 integration 且保留明确拒收原因

  Scenario: S17 目标或 worktree 被改动时保全现场
    Given 用户 dirty 文件或错误 HEAD 或 symlink 替换
    When resume 或目标物化尝试运行
    Then blocked 且用户字节与 branch 不被回滚覆盖

  Scenario: S18 无 DAG authority 时无执行副作用
    Given wave 或 dag_shadow 或 supervisor-db 未启用
    When 相同事件经过配置允许的入口
    Then 不新增 DAG job worktree 或集成事件且无默认 wave 行为回归

  Scenario: S19 正式 CLI 完整主路径可交付
    Given 有效三 Unit artifact 且 backend 按 stage 提交审查验证
    When ralph run 驱动 approval 到 tester 触发
    Then 依赖文件可读 三个 task 各关闭一次 且 done 恰一次
```

S16–S18 是各 Unit 必须覆盖的参数化边界，不可在最后一个 Unit 才实现。S19 随 U15 的输入契约完整接线验收；前置 Unit 各自有独立正式 runtime 入口。

## 5. 验收与测试策略

所有以下测试名均为**计划新增**；不是声称已有测试。位置为已确认文件或明确新增模块。测试必须有非零执行数。ATDD 以 SQLite rows、job 启动计数、Git SHA/index/tree、文件内容、task 状态、accepted 事件为断言，不能只检查 prompt 字符串或 mock 调用次数。

| Scenario | 验收条件与副作用/不变量 | 测试入口/位置 | 层级与原因 | 风险补充 / E2E |
|---|---|---|---|---|
| S1 | cap=1 连续 tick 启动 U2，U1 启动数不变；多 plan 同 U-ID 不冲突 | D/mod.rs 新 dag_tick_refills_ready | runtime integration；发现候选构造错误 | state-machine；否 |
| S2 | held permits≤capacity，review/重启不释放，ack 后释放一次 | D/mod.rs + 新 S/dag_store_rusqlite/admission.rs | 真 DB 两连接 + runtime | concurrency/idempotency；否 |
| S3 | 提交 C 后 reviewer、verify、fix 接续 C；不 reset | D/spawn.rs 新 dag_committed_stage_handoff | 真 Git + PTY + EventLoop | dirty/foreign/旧数据；否 |
| S4 | ref/index/files 一致后才 integrated，dirty 保全 | D/integrate.rs 新 dag_checked_out_target_materialized | 真 Git port + store | fault injection；否 |
| S5 | U3 启动能读取依赖 C 文件；reopen base 相同 | D/spawn.rs 新 dag_dependency_base_is_pinned | 真 Git/runtime | 两依赖、target 被重写；否 |
| S6 | execute/review/fix accepted 后恢复各只启动缺失后继 | D/recovery.rs 新 dag_recovery_advances_current_terminal | store reopen + runtime | stale history/双重恢复；否 |
| S7 | 原 candidate SHA 保留，record 一条，最终 ack 一次 | D/integrate.rs 新 dag_recovery_consumes_tested_intent | 真 Git + SQLite reopen | CAS before/after/descendant/foreign；否 |
| S8 | prepare/append/fsync/delivered 窗口后完整 done=1 | 新 D/terminal_delivery.rs；D/integrate.rs | 文件锁 + DB + EventLoop | 两连接/torn tail/I/O；否 |
| S9 | projector 前 receipt 已存在；写失败 task=0；重放 task key 不变 | 计划新增 core tests/dag_registration.rs | 真 EventLoop+StateProjector+SQLite | policy reject/artifact drift；否 |
| S10 | 每个事务写点失败均无半 active；reopen 可激活一次 | 新 S/dag_store_rusqlite/registration.rs + D/mod.rs | SQLite transaction + runtime | 双连接同 target；否 |
| S11 | execute failure correction 去重、满池 pending、预算3封顶 | D/spawn.rs；新 corrections.rs | 真 journal+accepted correction | duplicate/stale/cap/reopen；否 |
| S12 | 仅 integration-failure 授权 fix；新候选可交付 | D/integrate.rs 新 dag_integration_failure_correction | 真 Git+gate+DB+EventLoop | conflict/gate/nonfailure；否 |
| S13 | deadline/cancel 回收 group，tail≤64KiB，目标不变 | 新 S/gate_process.rs；S/integration_lane.rs | 子进程 integration | 超长单行/后代持 pipe；否 |
| S14 | gate barrier 未释放即观测另一个 job 启动；cancel 后 join | 新 D/integration_worker.rs；D/mod.rs | 真 worker/channel + runtime | generation/stale/cancel；否 |
| S15 | typed context 可解析，artifact 真可读，policy-check 与 emit 真通过 | D/spawn.rs；新 D/job_context.rs | contract + real CLI command | hash/path/skill visibility；否 |
| S16–18 | 无未授权状态变化；wave/shadow 无 DAG 副作用 | 以上 owning test + 现有 preflight/preset_lint | 最低 owning 层 | 每 Unit 回归；否 |
| S19 | CLI 启动完整 builtin DAG 语义，实际 commit、review、verify、integration、task ack、tester | 现有 crates/ralph-cli/tests/integration_dag_scheduler.rs 新 dag_cli_completes_real_pipeline | mock backend 的真实 CLI E2E；最低完整产品边界 | 增进程 stop/restart 窗口；是 |

命令缩写在第 9 节展开。所有 filesystem/DB 故障由 TempDir 的不可写/目录替代或 test-only phase hook 注入；hook 只存在于测试编译，不能新增生产环境变量后门。跨进程测试可用 fixture 控制的 backend barrier 与进程 kill，不用 mock DAG 方法。确需内部精确写点 hook 的窗口在同进程新实例 reopen 测试中覆盖；不得将该测试声称为 OS kill 实验。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence / Unit |
|---|---|---|---|---|---|---|---|
| R1 | Ready 补槽 | S1,S16,S18 | dag_tick_refills_ready | candidate/current-state/cap | 真 tick + journal | S19 后续总验收 | E2/E4 / U1 |
| R2 | durable permits | S2,S16 | dag_resource_leases_survive_stages | claim/release/idempotent | 双连接 DB + tick | S19 | E3 / U2 |
| R3 | 已提交接续 | S3,S17 | dag_committed_stage_handoff | evidence identity/HEAD | 真 resume + PTY | S19 | E5 / U3 |
| R4 | 实际目标交付 | S4,S17 | dag_checked_out_target_materialized | checkout state classification | 真 Git port | S19 | E7 / U4 |
| R5 | 依赖 base | S5,S17 | dag_dependency_base_is_pinned | dependency ancestry/base immutability | Git + reopen | S19 | E6 / U5 |
| R6 | terminal 恢复 | S6,S16 | dag_recovery_advances_current_terminal | current stage routing | journal + runtime | S19 restart variation | E8 / U6 |
| R7 | intent 恢复 | S7,S17 | dag_recovery_consumes_tested_intent | before/after/superseded | Git + SQLite | 同上 | E9 / U7 |
| R8 | terminal 投递 | S8 | dag_terminal_delivery_replays_once | stable key/state/conflict | append + fsync + DB | 同上 | E10/E20 / U8 |
| R9 | 投影前 receipt | S9,S16,S18 | dag_registration_receipt_precedes_projection | candidate vs accepted | 真 EventLoop/projector | 同上 | E11 / U9 |
| R10 | 激活原子性 | S10,S16 | dag_approval_activation_is_atomic | activation replay/conflict | SQLite + DAG runtime | 同上 | E11/E19 / U10 |
| R11 | execute correction | S11,S16 | dag_execute_failure_correction_is_durable | budget/transition/replay | accepted correction + job journal | S19 failure variation | E12 / U11 |
| R12 | integration correction | S12,S16 | dag_integration_failure_correction | failure authorization/intent attempt | 真 gate/conflict/record | 同上 | E12/E9 / U12 |
| R13 | gate 有界 | S13,S17 | dag_gate_timeout_reaps_descendants | deadline/tail/cancel | 真子进程/Git | S19 gate variation | E13 / U13 |
| R14 | gate 不阻塞 | S14,S18 | dag_gate_does_not_block_tick | generation/result handling | runtime worker + barrier | S19 | E13 / U14 |
| R15 | agent 输入契约 | S15,S16,S19 | dag_job_context_contract、dag_cli_completes_real_pipeline | typed context/schema/path | 真 policy-check/emit | 是 | E14/E21 / U15 |
| R16 | 正式路径证据 | S1–S19 | 上列全部 | 不用 source-only 代替行为 | 每 Unit 的 integration | S19 | E15/E16/E18 / U1–U15 |

## 7. 严格串行开发单元

执行顺序：**Unit 1 → Unit 2 → … → Unit 15**。每个 Unit 完成 Acceptance Red → Unit Red → Green → Refactor → Integration → Regression → Close 才能继续。F10 的覆盖修复随行为落地，不设“最后补测试”Unit。

**公共 Close 契约 G：** 当前 Scenario、最小单测、真 integration、相关回归全部通过；`cargo fmt --all -- --check`、对应 crate `cargo check`/`cargo build`/`cargo clippy -- -D warnings` 通过；源码 <5000 行；无 skip/ignore/.only、无断言弱化或无解释 golden 更新；无未来 Unit 行为；记录实际 Red 日志、Green 命令、Evidence 和 Decision 更新；置信度≥0.85；独立提交边界成立。最后 Unit 另跑全量门禁。

**公共停止契约 H：** 源码与 Evidence 冲突、接口不存在/调用链不同、Red 原因不符、需要新依赖/公开调用方、兼容策略失败、置信度<0.85、回归范围扩大或 Unit 无法保持原子性时停止。记录新证据→更新影响分析→比较候选→重新决策评分→修订当前与后续 Unit；不能继续猜测。

### U1. Unit 1：完成 job 后持续补槽

**1. Unit 目标**

释放一个 job 槽后，正式 tick 能启动下一个符合条件的 Ready Unit。

**2. 对应需求与 Scenario**

R1,R16；S1,S16,S18；D1,D16；E2,E4,E15,E18。

**3. 外部可观察结果**

下一可用 tick 有 U2 execute reservation/启动，U1 execute 启动仍一次；将 U1 放 integrated 后再次 tick 同样不挡 U2。

**4. 当前行为基线**

tick 把已启动及已集成 Unit 再交给从零计数的 admission；spawn 的 executors_launched 去重导致空转。（E2,E4,E15,E18）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入 current durable jobs、pending follow-ups、caps 与集成集合；输出合法 admission 或明确 cap/dependency reason。已启动数不增加两次，global/stage 实际占用不超 cap。

**6. 修改位置**

修改 D/mod.rs::tick、D/jobs.rs 的只读计数快照与 key 调用；新增 D/admission.rs 管候选构造；修改 S/dag_scheduler.rs::AdmissionSnapshot 及 S/dag_shadow.rs 的适配；测试位于 D/mod.rs 与 S/dag_scheduler.rs。 新模块只承接上述职责；相邻职责边界：只处理 job pool 与候选资格；不在本 Unit 实现 durable resource 获取、stage HEAD 恢复或 gate worker。

**7. 可依赖能力**

已有 JobPipeline、SQLite job journal、真实 Git fixture；无需前置 Unit。

**8. 禁止依赖的未来能力**

不依赖 U2–U15 尚未交付的能力。只处理 job pool 与候选资格；不在本 Unit 实现 durable resource 获取、stage HEAD 恢复或 gate worker。

**9. 验收测试**

计划新增 `dag_tick_refills_ready`。前置：真实 Git fixture，U1/U2 独立，global=executor=1；正常 accepted 事件使 U1 job 结束，U2 尚未 reserve。 动作：由 observe_accepted_events/tick 推进；不得直接调用 pipeline.advance 为 U2 填槽。 断言与副作用：下一可用 tick 有 U2 execute reservation/启动，U1 execute 启动仍一次；将 U1 放 integrated 后再次 tick 同样不挡 U2。 层级：真 JobPipeline、journal、tick，backend 只作为可控退出脚本；记录 reserve 数、实际启动数和每轮 pool 占用。 运行：C1 的单测过滤 dag_tick_refills_ready；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧 snapshot 仍选 U1，U2 reservation 数为0；不是 backend 启动失败。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

candidate_filter_excludes_current_and_integrated（无当前 job 才是 candidate）；snapshot_subtracts_live_stages（review/verify/fix 占 global）；cross_plan_unit_id_is_distinct（同 U-ID 两计划 key 不同）；blocked_dependency_does_not_consume_capacity。 这些名称为计划新增最小测试；输入/输出按括号及D1与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

先 S1 正式 tick Red；再 candidate_filter Red→只修候选→Green；再 live_stages Red→加入已占用计数→Green；再 cross_plan Red→统一限定 key→Green；提取 admission.rs 后重跑。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入 current durable jobs、pending follow-ups、caps 与集成集合；输出合法 admission 或明确 cap/dependency reason。已启动数不增加两次，global/stage 实际占用不超 cap。 必须遵循D1,D16及第3节实施协议；只处理 job pool 与候选资格；不在本 Unit 实现 durable resource 获取、stage HEAD 恢复或 gate worker。

**14. 集成验证**

真 JobPipeline、journal、tick，backend 只作为可控退出脚本；记录 reserve 数、实际启动数和每轮 pool 占用。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

state-machine：terminal/pending 顺序可能重复释放；固定 accepted replay 与两个 plan 同 U-ID；不 Mock tick。

**16. 回归范围**

C1,C2；既有 driver/jobs/spawn 和 dag_shadow tests、preflight mode 组合；改变 snapshot 接口须编译全部消费者。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E2,E4,E15,E18 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E2,E4,E15,E18 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E2,E4,E15,E18 |

**18. 完成标准**

G；S1 cap=1/2 与 Ready 空列表均有断言，旧 facade test 保留但不再作为补槽唯一证据。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

重建 snapshot 后 pending follow-up 会先消耗槽，必须在 reserve 时复验实时 cap；不能基于已过期的预计算 admission 无条件启动。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U2. Unit 2：跨阶段资源容量不超售

**1. Unit 目标**

一个 Unit 从首次 execute 到最终退出持有资源 permits，其他 Unit 无法超 capacity 启动。

**2. 对应需求与 Scenario**

R2,R16；S2,S16；D2,D16；E3,E4,E19。

**3. 外部可观察结果**

ack 前 U2 无 execute、held sum=1；ack 后 U2 启动、旧 lease released 一次；任一 claim 失败时 reserve/其他 claims 均未提交。

**4. 当前行为基线**

v14 已有 lease 表，但正式 reserve_job 没有取得资源；纯函数只有本 tick 局部 leased。（E3,E4,E19）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入 plan capacities、Unit claims、current identity；输出 reserve+held leases 或资源 pending。后继阶段继承 lease，不重复获取；确定终止后释放一次。

**6. 修改位置**

新增 S/dag_store_rusqlite/admission.rs；修改既有 jobs.rs reserve 入口，D/admission.rs、D/spawn.rs reserve 接线及 D/integrate.rs ack/终止 release；测试在新增模块与 D/mod.rs。 新模块只承接上述职责；相邻职责边界：不改 resource artifact 格式，不新增 migration；不把临时 job failed 当最终 Unit failed；不实现 F11 指标。

**7. 可依赖能力**

已完成 U1–U1；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U3–U15 尚未交付的能力。不改 resource artifact 格式，不新增 migration；不把临时 job failed 当最终 Unit failed；不实现 F11 指标。

**9. 验收测试**

计划新增 `dag_resource_leases_survive_stages`。前置：capacity=1，U1/U2 都 claim=1；U1 从 execute 到 review，使用两个连接打开同一 DB。 动作：在 U1 review 时 tick U2，关闭重开 store 后重试，再为 U1 写真实 integrated acceptance/ack。 断言与副作用：ack 前 U2 无 execute、held sum=1；ack 后 U2 启动、旧 lease released 一次；任一 claim 失败时 reserve/其他 claims 均未提交。 层级：真 SQLite IMMEDIATE transaction 与 tick；连接并发用 barrier；终止 signal 可 Fake，但 process unresolved 必须保留 lease。 运行：C1 的单测过滤 dag_resource_leases_survive_stages；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧正式路径没有 held lease，或下一 tick 允许 U2 消耗同一资源。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

reserve_claims_atomic（两个资源第二个不足全回滚）；lease_replay_no_double_permits；release_requires_terminal_and_fenced_process；two_connections_one_capacity（barrier 竞争只能一赢）；capacity_conflict_rejected。 这些名称为计划新增最小测试；输入/输出按括号及D2与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S2 Red；atomic claims Red→同事务 reserve→Green；reopen Red→读 held rows→Green；双连接 Red→事务内复核→Green；release Red→只接 ack/确定终态→Green；Refactor 共享 SQL 校验。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入 plan capacities、Unit claims、current identity；输出 reserve+held leases 或资源 pending。后继阶段继承 lease，不重复获取；确定终止后释放一次。 必须遵循D2,D16及第3节实施协议；不改 resource artifact 格式，不新增 migration；不把临时 job failed 当最终 Unit failed；不实现 F11 指标。

**14. 集成验证**

真 SQLite IMMEDIATE transaction 与 tick；连接并发用 barrier；终止 signal 可 Fake，但 process unresolved 必须保留 lease。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

Concurrency/Idempotency：进程内 Mutex 不能证明两个连接安全；包括 permits>capacity、unknown key、empty claims 与重复 release。

**16. 回归范围**

C1,C2,C3；migrations、dag store reopen、job transition、resource parser 及 shadow 零执行副作用；无旧 wave 表修改。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E3,E4,E19 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E3,E4,E19 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E3,E4,E19 |

**18. 完成标准**

G；跨 review/verify/pending correction/restart 的持有证据与最终释放证据齐全。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

资源域为同一 DAG store；不同 plan 相同 key 不得各算一份容量。既有活动 jobs 无 lease 时先事务重建并校验总量，不能以0使用量恢复。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U3. Unit 3：审查接续 executor 已提交成果

**1. Unit 目标**

已提交并被接纳的 Unit 成果可被下一阶段原样接续。

**2. 对应需求与 Scenario**

R3,R16；S3,S16,S17；D3,D16；E5,E18,E19。

**3. 外部可观察结果**

后继读取 C 的文件，HEAD=C，base 不变；wrong commit/dirty/foreign repo 只 blocked；artifact output 不使 tracked code 脏。

**4. 当前行为基线**

所有 SpawnKind 调 acquire(initial base)；branch 已前进则 BaseMismatch；resume helper 已有真实 Git 防篡改测试。（E5,E18,E19）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入 accepted identity、核验后的 Git HEAD/base 和 bounded artifact refs；输出同一 worktree 的后继 job。HEAD/terminal evidence 原子持久，写失败不释放槽不 spawn。

**6. 修改位置**

修改 D/spawn.rs::observe_unit_event_dag/spawn_job 与 jobs terminal API；新增 S/dag_store_rusqlite/evidence.rs、S/migrations/v18.sql；S/migrations.rs 登记；使用 D/worktree/resume.rs，不降低其检查；测试 D/spawn.rs。 新模块只承接上述职责；相邻职责边界：首次 base 暂沿用现有 approval base；per-Unit row 从此可存 base，U5 再改变选取时机；本 Unit 不自动恢复缺失后继。

**7. 可依赖能力**

已完成 U1–U2；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U4–U15 尚未交付的能力。首次 base 暂沿用现有 approval base；per-Unit row 从此可存 base，U5 再改变选取时机；本 Unit 不自动恢复缺失后继。

**9. 验收测试**

计划新增 `dag_committed_stage_handoff`。前置：真实 Git Unit worktree；脚本 executor 修改允许文件并 commit C；真实 EventLoop 接纳结果。 动作：驱动 completion→ledger→accepted→reviewer spawn，再以相同方式进入 verifier；fix accepted 接 reviewer 用参数化分支。 断言与副作用：后继读取 C 的文件，HEAD=C，base 不变；wrong commit/dirty/foreign repo 只 blocked；artifact output 不使 tracked code 脏。 层级：真 PTY child、Git、EventLoop 与 SQLite；Fake 的仅是 AI 工作内容。接受后读取 journal 的 SHA，不使用 agent content_hash 代替 Git HEAD。 运行：C1 的单测过滤 dag_committed_stage_handoff；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧调用 acquire 返回 BaseMismatch，reviewer 启动数0；现有 resume helper 测试先 Characterization 通过。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

accepted_evidence_binds_current_identity；terminal_and_head_rollback_together；review_cannot_change_head；execute_head_must_descend_from_base；missing_evidence_blocks_resume。 这些名称为计划新增最小测试；输入/输出按括号及D3与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

现有 resume Characterization Green；S3 Acceptance Red；evidence 原子写 Red→最小持久化→Green；stage-specific acquire/resume Red→接线→Green；dirty/tamper Red→保持严校验→Green；Refactor evidence 模块。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入 accepted identity、核验后的 Git HEAD/base 和 bounded artifact refs；输出同一 worktree 的后继 job。HEAD/terminal evidence 原子持久，写失败不释放槽不 spawn。 必须遵循D3,D16及第3节实施协议；首次 base 暂沿用现有 approval base；per-Unit row 从此可存 base，U5 再改变选取时机；本 Unit 不自动恢复缺失后继。

**14. 集成验证**

真 PTY child、Git、EventLoop 与 SQLite；Fake 的仅是 AI 工作内容。接受后读取 journal 的 SHA，不使用 agent content_hash 代替 Git HEAD。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

Fault injection：terminal/evidence 写入之间故障必须回滚；旧 DB 有 terminal 无 HEAD 时只允许从退出进程+匹配 hash 的可信结果重建，否则 blocked。

**16. 回归范围**

C1,C2,C3；所有 worktree resume/acquire、forged accepted result、macOS canonical path tests；默认 feature 与无 DB 构建。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E5,E18,E19 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E5,E18,E19 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E5,E18,E19 |

**18. 完成标准**

G；正常 commit 不再挡 reviewer，所有现有 path/identity 负例仍通过；v17→v18 保留 wave rows。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

恢复所需 metadata 不能只放内存。未提交代码的失败 correction 不能靠放宽 clean check 混进本 Unit。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U4. Unit 4：集成确认前物化目标工作区

**1. Unit 目标**

发布 integrated 时，operator 工作区实际含 candidate 的代码。

**2. 对应需求与 Scenario**

R4,R16；S4,S17；D4,D16；E7,E9,E20,E25。

**3. 外部可观察结果**

成功时 branch SHA、write-tree、tracked file 内容等于 C，status clean，之后才出现 integrated；dirty 场景原始字节和未关联 refs 不变。

**4. 当前行为基线**

compare_and_swap_ff 仅 update-ref；本轮实验确认 HEAD 变而文件旧且 status dirty。（E7,E9,E20,E25）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入 clean target B、tested candidate C 与 target identity；输出 Materialized C 后 integration record/event，或 checkout blocked。CAS 已成功而 checkout 失败保留 intent，绝不假成功。

**6. 修改位置**

新增 S/target_checkout.rs、S/migrations/v19.sql；修改 S/integration_lane.rs::compare_and_swap_ff、D/integration.rs::integrate、D/integrate.rs failure/emit 边界及 migration 注册；测试 D/integrate.rs 与新模块。 新模块只承接上述职责；相邻职责边界：不换 target 分支、不自动 stash/reset/clean；U7 尚未处理所有 CAS/record 断点，但本 Unit 自己的 checkout 断点必须可分类恢复。

**7. 可依赖能力**

已完成 U1–U3；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U5–U15 尚未交付的能力。不换 target 分支、不自动 stash/reset/clean；U7 尚未处理所有 CAS/record 断点，但本 Unit 自己的 checkout 断点必须可分类恢复。

**9. 验收测试**

计划新增 `dag_checked_out_target_materialized`。前置：真实 Git host checkout target B；Unit 提交允许路径；gate 通过。 动作：调用正式 integrate_unit；用 test-only hook 在 CAS 后、checkout 后各中断再创建 checkout reconciler。 断言与副作用：成功时 branch SHA、write-tree、tracked file 内容等于 C，status clean，之后才出现 integrated；dirty 场景原始字节和未关联 refs 不变。 层级：真实 Git port + SQLite + FileLock；Git 不 Fake；只允许故障点 hook。未持 target lock 不得 CAS。 运行：C1 的单测过滤 dag_checked_out_target_materialized；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧实现 integrated 已出现，但文件仍是 B，index tree 也为 B。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

classify_old_tree_repairable；classify_new_tree_already_materialized；mixed_or_dirty_blocks；wrong_worktree_identity_blocks；target_lock_busy_is_pending。 这些名称为计划新增最小测试；输入/输出按括号及D4与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S4 Red；old/new 分类 Red→新 checkout 模块→Green；CAS后中断 Red→intent 记录及安全两树物化→Green；dirty/锁竞争 Red→拒绝/排队→Green；Refactor port 边界。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入 clean target B、tested candidate C 与 target identity；输出 Materialized C 后 integration record/event，或 checkout blocked。CAS 已成功而 checkout 失败保留 intent，绝不假成功。 必须遵循D4,D16及第3节实施协议；不换 target 分支、不自动 stash/reset/clean；U7 尚未处理所有 CAS/record 断点，但本 Unit 自己的 checkout 断点必须可分类恢复。

**14. 集成验证**

真实 Git port + SQLite + FileLock；Git 不 Fake；只允许故障点 hook。未持 target lock 不得 CAS。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

Fault injection/Concurrency：跨 DB/ref/index/files 非原子；old/new 两个完整状态可自动恢复，混合状态保持 blocked 防止覆盖人工修复。

**16. 回归范围**

C1,C2,C3,C4；lane stale CAS、forbidden rename/symlink/submodule、真实 gate、host-clean 后续 acquire tests。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E7,E9,E20,E25 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E7,E9,E20,E25 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E7,E9,E20,E25 |

**18. 完成标准**

G；两树 checkout 实测与 port integration 一致；v19 migration/reopen 通过。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

外部 writer 不遵守 FileLock 时只能检测而非保证多文件原子；读取目标身份与 clean 状态须在 mutation 前后复核。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U5. Unit 5：依赖 Unit 从已集成代码启动

**1. Unit 目标**

后继首次执行时能读取前置已交付代码。

**2. 对应需求与 Scenario**

R5,R16；S5,S17；D5,D16；E6,E7,E9。

**3. 外部可观察结果**

U3 base 包含所有依赖 commit；重启仍同 SHA；Unit diff 不包含已经存在于 base 的前置改动；未 ack 或 ancestry 不成立不启动。

**4. 当前行为基线**

on_unit_integrated_accepted 只扩 integrated set；spawn/integrate 始终使用 plan verified_base。（E6,E7,E9）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入 dependency ack records 与 materialized target HEAD；输出不可变 unit_base；恢复、changed diff、resume 都使用该值。

**6. 修改位置**

修改 D/admission.rs、D/spawn.rs::spawn_job、D/integrate.rs::integrate_unit；扩展已新增 evidence.rs 的首次 base pin 事务；测试 D/spawn.rs 和 D/integrate.rs。 新模块只承接上述职责；相邻职责边界：不重设已经启动 Unit 的 base，不引入 rebase 工作流，不修改 dependency artifact 语法。

**7. 可依赖能力**

已完成 U1–U4；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U6–U15 尚未交付的能力。不重设已经启动 Unit 的 base，不引入 rebase 工作流，不修改 dependency artifact 语法。

**9. 验收测试**

计划新增 `dag_dependency_base_is_pinned`。前置：U1/U2 独立，U3 depends_on U1；U1 新增模块 M 并真实集成 ack；target 又包含无关已集成 U2。 动作：tick 首次启动 U3，读文件 M；reopen store 并查询固定 base；核验 integrate diff 的起点。 断言与副作用：U3 base 包含所有依赖 commit；重启仍同 SHA；Unit diff 不包含已经存在于 base 的前置改动；未 ack 或 ancestry 不成立不启动。 层级：真 Git + ack/task projection + journal；artifact至少两独立 Unit 以满足既有 NoParallelWave 校验；不以单 Unit 假 fixture 测 parser。 运行：C1 的单测过滤 dag_dependency_base_is_pinned；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧 U3 base=approval SHA，读取 M 失败。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

all_dependencies_must_be_acked_ancestors；base_first_write_immutable；no_dependencies_pin_current_target；target_rewrite_blocks；diff_uses_unit_base。 这些名称为计划新增最小测试；输入/输出按括号及D5与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S5 Red；依赖 ancestor Red→可信 base 选择→Green；pin replay Red→固定写入→Green；diff 起点 Red→统一消费→Green；Refactor 去掉 DAG 路径 plan-base fallback。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入 dependency ack records 与 materialized target HEAD；输出不可变 unit_base；恢复、changed diff、resume 都使用该值。 必须遵循D5,D16及第3节实施协议；不重设已经启动 Unit 的 base，不引入 rebase 工作流，不修改 dependency artifact 语法。

**14. 集成验证**

真 Git + ack/task projection + journal；artifact至少两独立 Unit 以满足既有 NoParallelWave 校验；不以单 Unit 假 fixture 测 parser。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

State-machine：approval 与 admission 间 target 前进正常，target 被重写异常；以持久 record+Git ancestor 双证据分辨。

**16. 回归范围**

C1,C2,C4；原 sibling lane-time squash、unknown dependency、empty target、stage resume 及恢复 base 测试。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E6,E7,E9 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E6,E7,E9 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E6,E7,E9 |

**18. 完成标准**

G；两前置依赖和已启动 Unit 不改 base 的边界通过。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

target SHA 在读取后又前进不使已 pin base 无效，只要已核验依赖均在其中；不得无条件 pin 最新可见 HEAD。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U6. Unit 6：terminal 后中断自动补齐后继

**1. Unit 目标**

已接受前阶段结果但未 reserve 后继的 Unit 重启后继续工作。

**2. 对应需求与 Scenario**

R6,R16；S6,S16；D6,D16；E8,E12,E18。

**3. 外部可观察结果**

缺失后继 reservation=1、实际启动=1；前阶段启动数不增；最新 attempt=1 时旧 attempt=0 accepted 不驱动；满池有可恢复 pending。

**4. 当前行为基线**

recover_after_restart 遍历历史 jobs 且 terminal.is_some 直接 continue；verify 有特例队列。（E8,E12,E18）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入当前 job（通过 dag_units current identity 关联）及 U3 evidence；输出一个缺失后继/pending/integration queued；不会复活历史 attempt。

**6. 修改位置**

新增 D/reconcile.rs；修改 D/mod.rs::recover_after_restart、D/spawn.rs live accepted 后继推进、D/recovery.rs tests；store jobs.rs 增 current-job 查询而非依赖 list 顺序。 新模块只承接上述职责；相邻职责边界：只恢复 stage 边界；不补 integration intent/terminal delivery/registration（U7–U10），不重启未知 PID。

**7. 可依赖能力**

已完成 U1–U5；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U7–U15 尚未交付的能力。只恢复 stage 边界；不补 integration intent/terminal delivery/registration（U7–U10），不重启未知 PID。

**9. 验收测试**

计划新增 `dag_recovery_advances_current_terminal`。前置：参数化 execute accepted→review、review accepted→verify、fix accepted→review、verify accepted→integration；写真实证据后在下一 reserve 前停机。 动作：构造新的 runtime 读同 DB，运行 recovery/tick 两次。 断言与副作用：缺失后继 reservation=1、实际启动=1；前阶段启动数不增；最新 attempt=1 时旧 attempt=0 accepted 不驱动；满池有可恢复 pending。 层级：真 DB close/reopen、runtime tick、U3 resume；精确阶段 hook 使用 cfg(test)，真实进程存在/退出走既有 PID 检测。 运行：C1 的单测过滤 dag_recovery_advances_current_terminal；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧恢复读到 terminal 后跳过，后继 reservation=0。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

current_job_only_not_history；next_stage_from_terminal；existing_successor_no_relaunch；missing_evidence_blocks；unknown_pid_keeps_blocked_and_lease。 这些名称为计划新增最小测试；输入/输出按括号及D6与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S6 Red；current query Red→按 current join→Green；routing Red→共享 reconcile→Green；double reopen Red→幂等 reserve→Green；pending 容量 Red→正常队列→Green；Refactor 去掉 verify 特例重复分支。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入当前 job（通过 dag_units current identity 关联）及 U3 evidence；输出一个缺失后继/pending/integration queued；不会复活历史 attempt。 必须遵循D6,D16及第3节实施协议；只恢复 stage 边界；不补 integration intent/terminal delivery/registration（U7–U10），不重启未知 PID。

**14. 集成验证**

真 DB close/reopen、runtime tick、U3 resume；精确阶段 hook 使用 cfg(test)，真实进程存在/退出走既有 PID 检测。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

Idempotency/State-machine：将 list_jobs 反序当 current 会错误重放；以唯一 current reference 约束。损坏 current pointer 必须失败而非寻找任意历史 job。

**16. 回归范围**

C1,C2,C3；recovery_adopts_real_worker_without_duplicate_spawn、forged result、budget、job launch fencing。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E8,E12,E18 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E8,E12,E18 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E8,E12,E18 |

**18. 完成标准**

G；四种阶段终态窗口均以同一 reconcile 入口验证，无事件历史依赖。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

read/store 错误不得 .ok() 降为 absent 再启动；pending worker 只有 reservation 成功且 PID handshake 明确才计入 active。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U7. Unit 7：按已测试 intent 收敛集成断点

**1. Unit 目标**

CAS 前后重启不会丢失已测试 candidate 或重复构造不同集成结果。

**2. 对应需求与 Scenario**

R7,R16；S7,S17；D7,D16；E9,E7。

**3. 外部可观察结果**

candidate SHA 不变；CAS 后分支不再次推进；record/ack 各一次；不匹配 tree/target blocked；已核验 runtime descendant 保持前进。

**4. 当前行为基线**

get_intent 在 CLI 正式恢复未被消费；recovery 主要扫描 integration records。（E9,E7）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入 intent、Git target/tree、record/checkout状态；输出原 candidate 的补 CAS/物化/record 或明确冲突 blocked。

**6. 修改位置**

修改 D/integration.rs::integrate、D/integrate.rs::reconcile_after_restart、S/dag_integration.rs intent 生命周期与 S/dag_store_rusqlite.rs 查询/验证；测试 D/integrate.rs。本 Unit 使用 v19 checkout 状态标记未落地 candidate superseded，不另增版本。 新模块只承接上述职责；相邻职责边界：不覆盖已成功 candidate，不自动接受外部 target descendant；U12 后续才加入 correction attempt 维度。

**7. 可依赖能力**

已完成 U1–U6；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U8–U15 尚未交付的能力。不覆盖已成功 candidate，不自动接受外部 target descendant；U12 后续才加入 correction attempt 维度。

**9. 验收测试**

计划新增 `dag_recovery_consumes_tested_intent`。前置：真实 gate 已通过并保存 intent；分别停在 CAS 前、CAS 后无 record、record 后无 ack。 动作：新 runtime 恢复；再恢复一次并统计 candidate prepare/CAS/record。 断言与副作用：candidate SHA 不变；CAS 后分支不再次推进；record/ack 各一次；不匹配 tree/target blocked；已核验 runtime descendant 保持前进。 层级：真实 Git/SQLite/store/gate；用可控 gate 的执行 marker 证明恢复未重复跑已完成 gate；不 Mock current_target_oid。 运行：C1 的单测过滤 dag_recovery_consumes_tested_intent；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧 CAS后恢复重新 prepare 并遇 intent conflict，或无 record 一直不 ack。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

intent_target_expected；intent_target_candidate；intent_target_proven_runtime_descendant；foreign_target_blocks；stale_unapplied_intent_can_supersede。 这些名称为计划新增最小测试；输入/输出按括号及D7与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S7 Red；before/after 分类 Red→消费 get_intent→Green；record 幂等 Red→补写→Green；stale supersede Red→确认未落地才换 generation→Green；Refactor 单一恢复分类。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入 intent、Git target/tree、record/checkout状态；输出原 candidate 的补 CAS/物化/record 或明确冲突 blocked。 必须遵循D7,D16及第3节实施协议；不覆盖已成功 candidate，不自动接受外部 target descendant；U12 后续才加入 correction attempt 维度。

**14. 集成验证**

真实 Git/SQLite/store/gate；用可控 gate 的执行 marker 证明恢复未重复跑已完成 gate；不 Mock current_target_oid。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

Fault injection：DB record 丢失不等于 CAS 未发生；不得根据 absence 推测失败。record自然键必须绑定 plan-qualified Unit 身份，防跨 plan 同 U-ID。

**16. 回归范围**

C1,C2,C3,C4；intent replay drift、CAS stale retry、lane-time head、U4 checkout 断点与 U5 base。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E9,E7 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E9,E7 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E9,E7 |

**18. 完成标准**

G；before/after/mismatched 三分支都有正反断言。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

已经完成且 ack 的后续 runtime integration 可能推进 target；只能以 durable 链和 ancestry 证明，不把任意 descendant 当已知成功。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U8. Unit 8：完成事件在 append 故障后最终只投递一次

**1. Unit 目标**

done prepare 后发生 I/O/重启仍可补送，且不重复完整主账本事件。

**2. 对应需求与 Scenario**

R8,R16；S8,S16；D8,D16；E10,E20,E26,E28。

**3. 外部可观察结果**

有效完整 done=1，delivered 最终为真；tester正常路由；同 key 不同 payload 拒绝；未知 corrupt tail 不截断。消费者进程跨崩溃的启动次数不属于本Unit承诺。

**4. 当前行为基线**

try_record_terminal_emit 成功即永久消耗许可，append 失败只 warn；恢复 fence 有而 event 无则 block。（E10,E20）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入全部 ack 的 plan identity/digest；输出 pending/delivered delivery 和一条完整 done。payload 冲突或不可读文件是 blocked，不能视为未写。

**6. 修改位置**

新增 D/terminal_delivery.rs、S/migrations/v20.sql及core src/event_file_append.rs（计划新增共享锁与tail校验helper，core src/lib.rs导出）。修改 D/integrate.rs done/reconcile、S/dag_store_rusqlite.rs。E26/E28列出的全部现有主账本写入位置接入该helper：S/merge_sink.rs；CLI commands/emit/command_impl.rs、loop_runner/{entry,inner,hat_channel}.rs、loop_runner/wave/io.rs、loop_runner/wave/dispatcher/{coordination,salvage,dispatch}.rs、wave.rs；core event_loop/dispatch_and_handoff.rs。这些文件只替换append边界，保持payload构造与路由不变；inner.rs不得内联新逻辑。测试新增delivery/helper模块及D/integrate.rs，保留每个调用者原有错误处理测试。不修改recovery/history/scratchpad等非主账本写入；不把所有业务事件改造成新outbox，仅DAG done需要dedup。

**7. 可依赖能力**

已完成 U1–U7；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U9–U15 尚未交付的能力。不把所有业务事件改造成新 outbox；仅 DAG done 的投递需要 dedup。共享 writer 锁是必要防并发边界，不改变 wave payload。

**9. 验收测试**

计划新增 `dag_terminal_delivery_replays_once`。前置：全部 Unit ack；main ledger 写入可控失败，及写完完整行尚未标 delivered 的 durable fixture。动作：prepare→失败→reopen→retry；第二变体append后reopen；第三变体两个连接同时尝试同key。断言与副作用：有效完整done=1，delivered最终为真；真实EventLoop能路由tester；同key不同payload拒绝；未知corrupt tail不截断。层级：真FileLock、SQLite、main events readback和EventLoop路由；注入fsync/append故障使用可替换I/O adapter，不Mock dedup/DB。运行：C1单测过滤dag_terminal_delivery_replays_once；filter匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧 fence 无 event 变 blocked，事件数0；只有正常 repeated tick test 不算本项 Red。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

delivery_key_stable；prepare_conflict_rejected；append_once_under_lock；fsync_failure_keeps_pending；known_torn_tail_repaired_without_other_bytes_loss。 这些名称为计划新增最小测试；输入/输出按括号及D8与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S8 Red；DB pending Red→v20→Green；append/delivered 窗口 Red→读校验幂等写→Green；并发写 Red→共享锁→Green；tail Red→安全修复或blocked→Green；Refactor writer wrapper。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入全部 ack 的 plan identity/digest；输出 pending/delivered delivery 和一条完整 done。payload 冲突或不可读文件是 blocked，不能视为未写。 必须遵循D8,D16及第3节实施协议；不把所有业务事件改造成新 outbox；仅 DAG done 的投递需要 dedup。共享 writer 锁是必要防并发边界，不改变 wave payload。

**14. 集成验证**

真 FileLock、SQLite、main events readback 和 EventLoop tester 路由；注入 fsync/append 故障使用可替换 I/O adapter，不 Mock dedup/DB。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

Fault injection/Concurrency：fsync 返回错误可能已写完整行，重试必须先查账本；不得先删 fence 或无条件追加。

**16. 回归范围**

C1,C2,C3；对E26/E28各调用者运行现有emit、wave、hat_channel、guidance、default_publishes相关测试，及merge_sink、coordination/salvage、OPAC merge-one、现有done count tests（按新恢复契约更新旧blocked专用断言，记录理由）。共享append helper新增多进程writer与torn-tail保护测试，运行core过滤event_file_append；CLI消费者运行`cargo nextest run -p ralph-cli --bin ralph`并带第9节七项清理前缀，core消费者运行C6。同步运行doc drift扫描；命令语法未变，无新增参数文档。每Unit另执行B1/B2/B3/B4；失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E10,E20 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E10,E20 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E10,E20 |

**18. 完成标准**

G；所有写入窗口均验证完整行计数及真实路由；跨进程共享writer不得在torn tail后追加；v19→v20 wave rows保留。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

历史 fence 恢复：只有 all ack、plan digest 匹配且账本可核验时导入 delivery；证据不足 blocked，不清除旧 fence 猜测重送。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U9. Unit 9：任务投影前登记可恢复 plan receipt

**1. Unit 目标**

plan-ready 投影中断不留下无法归属和恢复的任务。

**2. 对应需求与 Scenario**

R9,R16；S9,S16,S18；D9,D16；E11,E20,E24。

**3. 外部可观察结果**

任何已产生的 task 都能关联预存 receipt；receipt失败 task=0；task key 幂等；未获 accepted approval 时 jobs=0；policy拒绝候选不 active。

**4. 当前行为基线**

真实 StateProjector.apply 早于 CLI on_plan_ready；后续 policy 仍可拒绝候选。（E11,E20,E24）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入合法 canonical handoff + source/loop identity；输出投影前 candidate receipt、最终 acceptance 标记；I/O 失败不投影；candidate 不授权 job。

**6. 修改位置**

新增 crates/ralph-core/src/event_loop/dag_registration.rs 与 crates/ralph-core/tests/dag_registration.rs；新增 S/migrations/v21.sql；修改 legacy.rs 两个薄接入点、event_loop/mod.rs 模块声明、D/mod.rs recovery/on_plan_ready；不把逻辑堆进 inner.rs。 新模块只承接上述职责；相邻职责边界：不重排所有 EventLoop 事件的全局投影顺序；只为 DAG plan-ready 加 receipt 协议，不修改 wave 语义。

**7. 可依赖能力**

已完成 U1–U8；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U10–U15 尚未交付的能力。不重排所有 EventLoop 事件的全局投影顺序；只为 DAG plan-ready 加 receipt 协议，不修改 wave 语义。

**9. 验收测试**

计划新增 `dag_registration_receipt_precedes_projection`。前置：真实 EventLoop+projection 配置，有效两并行 Unit artifact；在 projector.apply 前后设置 cfg(test) 观察/中断点。 动作：通过真实 JSONL 入口处理 plan-ready；分别 receipt I/O fail、投影前中断、投影后accepted前中断，再重放。 断言与副作用：任何已产生的 task 都能关联预存 receipt；receipt失败 task=0；task key 幂等；未获 accepted approval 时 jobs=0；policy拒绝候选不 active。 层级：真实 EventLoop、StateProjector、handoff 和 SQLite；不能直接调用 on_plan_ready 作为 Acceptance。fixture 不从 payload 提供 unit_tasks。 运行：C5 的单测过滤 dag_registration_receipt_precedes_projection；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧实现进入 projector 时 receipt rows=0；投影已发生而 CLI seam 未运行。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

candidate_is_not_accepted；receipt_digest_conflict；receipt_before_projection_failure；replay_pending_revalidates_artifact；accepted_marker_after_real_acceptance。 这些名称为计划新增最小测试；输入/输出按括号及D9与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S9 Red；candidate持久 Red→v21与helper→Green；I/O gate Red→投影前拒绝→Green；policy拒收 Red→独立accepted标记→Green；reopen Red→重验/幂等投影→Green；Refactor 薄 legacy 接入。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入合法 canonical handoff + source/loop identity；输出投影前 candidate receipt、最终 acceptance 标记；I/O 失败不投影；candidate 不授权 job。 必须遵循D9,D16及第3节实施协议；不重排所有 EventLoop 事件的全局投影顺序；只为 DAG plan-ready 加 receipt 协议，不修改 wave 语义。

**14. 集成验证**

真实 EventLoop、StateProjector、handoff 和 SQLite；不能直接调用 on_plan_ready 作为 Acceptance。fixture 不从 payload 提供 unit_tasks。 执行当前acceptance过滤及C1/C2的相关模块集和C5；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

State-machine/Fault injection：receipt-before-projection 不代表 receipt-before-all-validation；必须区分 candidate/accepted，防止未授权 plan 激活。

**16. 回归范围**

C1,C2,C3,C5；state_projector、accepted_transition、workflow_guard、legacy acceptance、wave/dag_shadow、artifact drift。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E11,E20,E24 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E11,E20,E24 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E11,E20,E24 |

**18. 完成标准**

G；两个实际调用点有测试观察顺序，legacy.rs 仍<5000，inner.rs无逻辑膨胀。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

恢复只candidate时重送经过普通来源/策略检查；不得用 system source 提升原 planner 权限。既有 accepted evidence 已证明接纳才允许完成标记。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U10. Unit 10：approval 激活全成或全不成

**1. Unit 目标**

合法 approval 不产生 receipt active、plan/base 缺失的半激活状态。

**2. 对应需求与 Scenario**

R10,R16；S10,S16；D10,D16；E11,E19。

**3. 外部可观察结果**

失败时 receipt status/plan status/unit rows/base 未出现部分新提交；恢复激活一次；不同 digest/target/base 同 key 拒绝。

**4. 当前行为基线**

receipts.activate→register_plan→activate_plan→record_verified_base 分多次操作。（E11,E19）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入 U9 accepted plan receipt 与 accepted approval、可信 target/base；输出一次原子 active 状态；冲突保留原始值，失败可由 approval receipt 重放。

**6. 修改位置**

新增 S/dag_store_rusqlite/registration.rs；修改 D/mod.rs::on_concurrency_approved/recover_after_restart 和 shared store API；使用 U9 v21 accepted approval 记录，不新增迁移；测试新 registration 模块与 D/mod.rs。 新模块只承接上述职责；相邻职责边界：不设计新 guardian 策略，不从任务存在推断 approved，不更改并发池默认值。

**7. 可依赖能力**

已完成 U1–U9；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U11–U15 尚未交付的能力。不设计新 guardian 策略，不从任务存在推断 approved，不更改并发池默认值。

**9. 验收测试**

计划新增 `dag_approval_activation_is_atomic`。前置：accepted plan/approval receipt 完整；在同一 SQLite transaction 的每个写点注入一次失败。 动作：调用正式 approval handler；查询全部相关表，reopen 后重放同一 accepted approval。 断言与副作用：失败时 receipt status/plan status/unit rows/base 未出现部分新提交；恢复激活一次；不同 digest/target/base 同 key 拒绝。 层级：两个 SQLite 连接 barrier 竞争同 target；真实 handler 输入来自 accepted boundary；不 Mock transaction。 运行：C1 的单测过滤 dag_approval_activation_is_atomic；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧 helper 串写中途失败留下 active receipt 或缺 base 的 plan。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

activation_transaction_rolls_back_each_write；approval_replay_same_identity；approval_conflict_no_mutation；unknown_receipt_no_activation；duplicate_target_owner_refused。 这些名称为计划新增最小测试；输入/输出按括号及D10与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S10 Red；rollback各点 Red→单事务→Green；replay Red→accepted approval 恢复→Green；冲突 Red→稳定错误→Green；Refactor 删除多 helper 组合入口。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入 U9 accepted plan receipt 与 accepted approval、可信 target/base；输出一次原子 active 状态；冲突保留原始值，失败可由 approval receipt 重放。 必须遵循D10,D16及第3节实施协议；不设计新 guardian 策略，不从任务存在推断 approved，不更改并发池默认值。

**14. 集成验证**

两个 SQLite 连接 barrier 竞争同 target；真实 handler 输入来自 accepted boundary；不 Mock transaction。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

Concurrency/Fault injection：approval base须在首次accepted时固定；restart读当前HEAD会改变原授权，必须拒绝缺失且不可验证证据。

**16. 回归范围**

C1,C2,C3,C5；receipt registration replay、target ownership、verified base、U2 capacity conflict 与 U9 pending receipt。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E11,E19 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E11,E19 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E11,E19 |

**18. 完成标准**

G；五个写点 rollback、跨连接与重启验证齐全。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

对旧 DB 的 active plan 缺 base 不做猜测补写；该状态继续清楚 blocked，不能以兼容为由绕过授权。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U11. Unit 11：执行失败 correction 可恢复且预算不重复扣减

**1. Unit 目标**

当前 executor 确定失败后，合法 correction 恰启动一个 fixer。

**2. 对应需求与 Scenario**

R11,R16；S11,S16；D11,D16；E12,E4,E19。

**3. 外部可观察结果**

满池时pending持久且budget未扣；释放后fix=1、attempt+1、budget扣1；耗尽明确blocked；旧token/unknown unit无改变。

**4. 当前行为基线**

journal 不接受 execute failed→fix，correction handler 未 admitted 只 debug。（E12,E4,E19）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入accepted correction/current failure；输出pending→fix reservation或budget blocked；重复请求不加 attempt。

**6. 修改位置**

新增 S/dag_store_rusqlite/corrections.rs、S/migrations/v22.sql；修改 jobs.rs reserve transition、D/mod.rs::on_correction_requested、D/spawn.rs pending与错误传播；测试D/spawn.rs及corrections.rs。 新模块只承接上述职责；相邻职责边界：不授予没有failure的fix权限；不处理integration失败（U12）；保留3次budget，不新增配置。

**7. 可依赖能力**

已完成 U1–U10；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U12–U15 尚未交付的能力。不授予没有failure的fix权限；不处理integration失败（U12）；保留3次budget，不新增配置。

**9. 验收测试**

计划新增 `dag_execute_failure_correction_is_durable`。前置：execute current terminal=failed、trusted clean worktree；fixer pool 被另一Unit占满；同一failure重复correction。 动作：通过accepted correction处理、重启、释放fixer pool并tick。 断言与副作用：满池时pending持久且budget未扣；释放后fix=1、attempt+1、budget扣1；耗尽明确blocked；旧token/unknown unit无改变。 层级：真EventLoop correction、journal和PTY fixer；Fake只提供失败及修复代码；job退出/PID fence须真实可见。 运行：C1 的单测过滤 dag_execute_failure_correction_is_durable；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧reserve返回invalid transition，fixer永不启动；满池请求不落盘。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

execute_failed_authorizes_only_current_correction；duplicate_request_dedups；pool_wait_does_not_spend_budget；attempt_three_is_last；dirty_failed_worktree_blocks_without_reset。 这些名称为计划新增最小测试；输入/输出按括号及D11与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S11 Red；状态规则Red→精确execute failed入口→Green；pending/reopen Red→v22→Green；重复预算Red→同事务扣减reserve→Green；Refactor共享pending管理。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入accepted correction/current failure；输出pending→fix reservation或budget blocked；重复请求不加 attempt。 必须遵循D11,D16及第3节实施协议；不授予没有failure的fix权限；不处理integration失败（U12）；保留3次budget，不新增配置。

**14. 集成验证**

真EventLoop correction、journal和PTY fixer；Fake只提供失败及修复代码；job退出/PID fence须真实可见。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

Idempotency：多个affected_unit_ids含重复项须去重；其中unknown项不改变已知Unit的合法请求，逐Unit记录结果可恢复。

**16. 回归范围**

C1,C2,C3；review-reject fixer现有路径、预算耗尽、global/fixer pool、lease保留/释放、forged identity。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E12,E4,E19 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E12,E4,E19 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E12,E4,E19 |

**18. 完成标准**

G；没有reserve失败只warn然后丢任务的路径；budget、pending、blocked各有持久断言。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

fix失败是否继续必须有新accepted correction；禁止无信号自循环。最终blocked只有确认进程已退出才释放Unit资源。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U12. Unit 12：集成失败 correction 可重新集成

**1. Unit 目标**

gate失败或集成冲突的Unit可经授权修复、复审、验证后重新交付。

**2. 对应需求与 Scenario**

R12,R16；S12,S16；D12,D16；E9,E12。

**3. 外部可观察结果**

新attempt成功集成一次；旧verify保持accepted；旧intent内容不变；无failure直接correction无新job；CAS已推进窗口先U7收敛不能fix。

**4. 当前行为基线**

integration失败时current job仍verify accepted，journal不允许fix；旧intent自然键也未区分correction attempt。（E9,E12）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入独立integration failure fact+accepted correction；输出新attempt fixer和新的candidate；旧accepted verify/intent保留审计。

**6. 修改位置**

修改D/integrate.rs::fail_integration、D/integration.rs、S/dag_integration.rs、S/dag_store_rusqlite.rs及corrections.rs；新增S/migrations/v23.sql存attempt/generation关联；测试D/integrate.rs。 新模块只承接上述职责；相邻职责边界：不放宽verify accepted→fix通用转移，不修改公开topic，不把CAS已成功但尚未物化当可随意修代码的失败。

**7. 可依赖能力**

已完成 U1–U11；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U13–U15 尚未交付的能力。不放宽verify accepted→fix通用转移，不修改公开topic，不把CAS已成功但尚未物化当可随意修代码的失败。

**9. 验收测试**

计划新增 `dag_integration_failure_correction`。前置：参数化真实merge冲突与真实gate非零；verify已accepted，保存integration failure；干净可信Unit worktree。 动作：accepted correction→fix commit→review→verify→新integration；随后replay旧correction和旧candidate completion。 断言与副作用：新attempt成功集成一次；旧verify保持accepted；旧intent内容不变；无failure直接correction无新job；CAS已推进窗口先U7收敛不能fix。 层级：真Git冲突/gate/store/EventLoop，不Mock失败产生点；故障后验证原target未改变，成功后task只close一次。 运行：C1 的单测过滤 dag_integration_failure_correction；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧journal拒绝verify accepted→fix，或旧intent阻止新candidate记录。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

integration_failure_authorizes_current_attempt；verify_acceptance_alone_insufficient；intent_keys_include_attempt；stale_candidate_cannot_integrate；cas_already_applied_requires_reconcile。 这些名称为计划新增最小测试；输入/输出按括号及D12与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S12 Red；failure fact Red→持久失败记录→Green；精确授权Red→correction查询→Green；candidate新attempt Red→v23/API→Green；stale Red→generation校验→Green；Refactor统一错误路径。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入独立integration failure fact+accepted correction；输出新attempt fixer和新的candidate；旧accepted verify/intent保留审计。 必须遵循D12,D16及第3节实施协议；不放宽verify accepted→fix通用转移，不修改公开topic，不把CAS已成功但尚未物化当可随意修代码的失败。

**14. 集成验证**

真Git冲突/gate/store/EventLoop，不Mock失败产生点；故障后验证原target未改变，成功后task只close一次。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

State-machine/Fault injection：v23迁移旧intent归属必须由unique current plan/unit验证；归属歧义blocked而非随机绑定。

**16. 回归范围**

C1,C2,C3,C4；U7 intent replay、U11 execute/review correction、path authorization、integration ack与done。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E9,E12 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E9,E12 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E9,E12 |

**18. 完成标准**

G；冲突与gate失败两来源均回归；同attempt不能两个active candidate。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

对不产生语义变化的CAS stale仅按U7重试规则，不消耗fixer预算；persistent foreign target仍blocked。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U13. Unit 13：gate 超时和输出有界

**1. Unit 目标**

失控gate不能无限运行、无限缓存输出或留下后代进程。

**2. 对应需求与 Scenario**

R13,R16；S13,S17；D13,D16；E13,E17。

**3. 外部可观察结果**

2s外层上限前typed timeout已返回；无活后代；两路tail各≤65536字节，reason≤既有格式允许上限；target SHA未变。

**4. 当前行为基线**

Command::output阻塞直到进程退出，完整stdout/stderr进入内存，之后才截stderr。（E13,E17）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入argv、candidate cwd、deadline、cancel；输出Pass或有界Fail reason；无论退出形式都回收process group并等待。

**6. 修改位置**

新增S/gate_process.rs；修改S/integration_lane.rs::run_gate_commands_in/GateCommandSpec消费与D/integrate.rs构造runner限制；core supervisor/mod.rs导出；测试新runner与lane。 新模块只承接上述职责；相邻职责边界：本Unit gate仍可同步占tick但最多deadline（U14再移出控制面）；不修改PTY AI backend协议，不新增公开配置。

**7. 可依赖能力**

已完成 U1–U12；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U14–U15 尚未交付的能力。本Unit gate仍可同步占tick但最多deadline（U14再移出控制面）；不修改PTY AI backend协议，不新增公开配置。

**9. 验收测试**

计划新增 `dag_gate_timeout_reaps_descendants`。前置：受控sh gate产生子进程、超长无换行stdout和stderr，或子进程独占pipe；测试deadline注入100ms，外层清理看门狗2s。 动作：调用真实Git port的targeted gate；deadline到达后查询process group及tail长度。 断言与副作用：2s外层上限前typed timeout已返回；无活后代；两路tail各≤65536字节，reason≤既有格式允许上限；target SHA未变。 层级：真实子进程/Git gate；时钟纯规则可Fake，killpg/wait不得Mock。日志不打印完整env。 运行：C2 的单测过滤 dag_gate_timeout_reaps_descendants；runner单测同时用 C2 过滤 gate_process；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧runner未按100ms deadline返回而触发测试自身清理并失败；不得让nextest无限挂起或把watchdog超时算通过。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

tail_ring_keeps_last_64k；oversize_single_line_bounded；deadline_covers_all_commands；spawn_error_returns_fail；cancel_reaps_group；nonzero_short_circuits_next_command。 这些名称为计划新增最小测试；输入/输出按括号及D13与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S13 Red；tail buffer Red→有界drain→Green；deadline Red→process group kill+wait→Green；cancel/后代pipe Red→清理闭环→Green；Refactor runner。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入argv、candidate cwd、deadline、cancel；输出Pass或有界Fail reason；无论退出形式都回收process group并等待。 必须遵循D13,D16及第3节实施协议；本Unit gate仍可同步占tick但最多deadline（U14再移出控制面）；不修改PTY AI backend协议，不新增公开配置。

**14. 集成验证**

真实子进程/Git gate；时钟纯规则可Fake，killpg/wait不得Mock。日志不打印完整env。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

Fault injection：leader退出但grandchild持pipe会卡drain；必须以group终止与drain deadline双约束。UTF-8截断不可panic。

**16. 回归范围**

C1,C2,C4；gate命令参数解析、empty gate fail、passing/failing gate；既有PTY lease/process group测试不受改动。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E13,E17 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E13,E17 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E13,E17 |

**18. 完成标准**

G；挂起、洪泛、后代持pipe、spawn失败和正常退出均有资源清理断言。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

使用已有nix/standard process能力，不新增timeout crate；Windows不新增支持，沿现有cfg边界确保无DB/非Unix构建边界可编译。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U14. Unit 14：gate 等待期间持续处理调度

**1. Unit 目标**

一个Unit的gate慢时，其他job仍可结束并补槽。

**2. 对应需求与 Scenario**

R14,R16；S14,S18；D14,D16；E13,E4,E24。

**3. 外部可观察结果**

gate未结束之前已处理completion并启动工作；取消后worker/group join完成；迟到generation无集成记录、无lease误释放。

**4. 当前行为基线**

tick→maybe_integrate_one→integrate_unit同步等待gate；Tokio其他线程能跑但控制面不推进。（E13,E4,E24）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入pending integration、completion、cancel；输出active integration worker和完成消息；每target最多一worker，stale消息不得commit/ack。

**6. 修改位置**

新增D/integration_worker.rs；修改D/mod.rs::tick/has_pending_work、D/integrate.rs integration发起/完成、D/integration.rs取消与commit前generation检查；inner.rs仅接取消/drive调用；测试D/mod.rs和worker。 新模块只承接上述职责；相邻职责边界：不并行同target集成、不改变排序；使用U13的真实有界runner，不引入第二scheduler。

**7. 可依赖能力**

已完成 U1–U13；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

不依赖 U15–U15 尚未交付的能力。不并行同target集成、不改变排序；使用U13的真实有界runner，不引入第二scheduler。

**9. 验收测试**

计划新增 `dag_gate_does_not_block_tick`。前置：U1 gate等待文件/pipe握手；U2 completion就绪，U3 Ready且有资源容量；worker在gate开始后发ready信号。 动作：不放行gate，驱动正常tick；观察U2后继或U3实际spawn，再取消loop。 断言与副作用：gate未结束之前已处理completion并启动工作；取消后worker/group join完成；迟到generation无集成记录、无lease误释放。 层级：真实integration worker、gate和completion channel；barrier证明因果先后，不用sleep阈值假设调度速度。 运行：C1 的单测过滤 dag_gate_does_not_block_tick；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧tick直到gate放行才返回，无法在握手放行前观察U3启动。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

one_active_integration_per_target；worker_completion_routes_current_generation；stale_completion_ignored；cancel_before_authorization_prevents_cas；authorization_before_cancel_finishes_materialization；has_pending_work_includes_worker。这些名称为计划新增最小测试；输入/输出按D14与本Unit第5项约束。取消两种测试以握手分别固定commit_authorized之前/之后，断言前者ref不变、后者完成物化且无新admission。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S14 Red；start/poll Red→受控worker→Green；one target Red→active map→Green；cancel/generation Red→commit前校验+join→Green；Refactor tick只保留控制面。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入pending integration、completion、cancel；输出active integration worker和完成消息；每target最多一worker，stale消息不得commit/ack。 必须遵循D14,D16及第3节实施协议；不并行同target集成、不改变排序；使用U13的真实有界runner，不引入第二scheduler。

**14. 集成验证**

真实integration worker、gate和completion channel；barrier证明因果先后，不用sleep阈值假设调度速度。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

Concurrency：spawn_blocking任务不可单靠abort取消；必须共享cancel并runner回收。SQLite guard或runtime可变引用不可跨worker边界。

**16. 回归范围**

C1,C2,C4；inner keepalive、cancel、U1补槽、U2lease、U7/CAS recovery、U13timeout；wave路径仍原有authority。 每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E13,E4,E24 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E13,E4,E24 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E13,E4,E24 |

**18. 完成标准**

G；inner.rs总行数<5000；shutdown所有worker均已join或明确blocked且有进程证据。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

锁顺序固定target lock→短DB事务，绝不持DB事务等待gate或FileLock；完成结果处理不在工作线程调用EventLoop。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

### U15. Unit 15：DAG job 输入足以驱动完整交付

**1. Unit 目标**

四种DAG job使用真实可读的当前输入，完成正式CLI从执行到tester触发的链路。

**2. 对应需求与 Scenario**

R15,R16；S15,S16,S18,S19；D15,D16；E14,E15,E16,E21。

**3. 外部可观察结果**

每stage能定位当前Unit与前阶段证据；review/verifier HEAD匹配；测试namespace无碰撞；输入缺失在spawn前blocked；正式CLI三Unit最终task各closed一次、done=1且tester得到触发。

**4. 当前行为基线**

builder缺execution_plan_path/前阶段证据，preset要求wave/map/projection；已有canary只检查文本并停在merge_queue。（E14,E15,E16,E21）现有相关正常/拒收测试先作 Characterization 保留；新增期望不能反过来改成旧错误行为。

**5. 输入与输出**

输入verified plan、U3 evidence、hat/schema/skills；输出typed context和job可读input bundle/隔离outputs。缺artifact/hash/skill拒绝spawn；结果artifact经校验回传。

**6. 修改位置**

新增D/job_context.rs及计划新增crates/ralph-cli/tests/fixtures/dag_backend.sh；修改D/spawn.rs、D/integrate.rs artifact消费、presets/en/parallel-forge.yml；测试D/spawn.rs、crates/ralph-cli/tests/integration_dag_scheduler.rs；文档/schema同步见本Unit最小范围。 新模块只承接上述职责；相邻职责边界：不伪造wave_id/slot_index/worktree_map，不扩展operator inspect指标；不把preset特例写入通用注入指南。

**7. 可依赖能力**

已完成 U1–U14；仅可使用其已验收能力及既有 TempDir/custom backend/SQLite。

**8. 禁止依赖的未来能力**

无后续 Unit；不扩大到 F11–F13。不伪造wave_id/slot_index/worktree_map，不扩展operator inspect指标；不把preset特例写入通用注入指南。

**9. 验收测试**

计划新增 `dag_job_context_contract`。前置：加载真实builtin config，runtime为Execute/Review/Verify/Fix构建输入；artifact在host .ralph但Unit Git worktree初始没有它；backend custom使用stdin。 动作：脚本解析typed context、读取bundle文件并核验hash，按stage提交/只读review/验证/修复，用真实ralph emit --policy-check后正式emit。 断言与副作用：每stage能定位当前Unit与前阶段证据；review/verifier HEAD匹配；测试namespace无碰撞；输入缺失在spawn前blocked；正式CLI三Unit最终task各closed一次、done=1且tester得到触发。 层级：真实CLI/common::ralph_bin、Git、SQLite、PTY、EventLoop；仅AI backend脚本Fake。planner/guardian仍通过真实事件接纳；不预写active DB，不直接pipeline.advance，不直接queue_integration。新增CLI运行使用已存在run --no-tui与配置custom backend。 运行：C1 的单测过滤 dag_job_context_contract；filter 匹配数必须大于0。

**10. Acceptance Red**

先运行上述 acceptance。预期真实失败：旧prompt无法解析要求字段或输入文件在worktree不存在，script明确返回context_missing而非随意continue；不得以字符串contains代替该Red。 编译错误、fixture缺失、命令错误、环境损坏、未执行到目标入口均不是有效Red，先修测试搭建再重跑。

**11. 单元测试拆分**

context_fields_for_each_stage；verified_input_bundle_paths_exist；bundle_digest_mismatch_blocks；output_artifact_import_checks_owner；skill_visibility_matches_hat；namespace_unique_across_attempts。 这些名称为计划新增最小测试；输入/输出按括号及D15与本Unit第5项约束。仅时钟、故障注入和AI工作内容可Fake；不得Mock本Unit真实规则与存储判断。

**12. Red → Green → Refactor 顺序**

S15 contract Red；typed context Red→builder→Green；文件可读Red→bundle物化→Green；前阶段ref Red→evidence消费→Green；policy-check Red→preset/registry接线→Green；S19完整CLI→修本Unit输入接线→Green；Refactor后全部回归。 每个Red记录实际断言/日志；最小Green之后才继续下一个断言。

**13. 最小实现范围**

输入verified plan、U3 evidence、hat/schema/skills；输出typed context和job可读input bundle/隔离outputs。缺artifact/hash/skill拒绝spawn；结果artifact经校验回传。 必须遵循D15,D16及第3节实施协议；不伪造wave_id/slot_index/worktree_map，不扩展operator inspect指标；不把preset特例写入通用注入指南。

文档与preset同步在本Unit内完成：executor/reviewer/verifier/fixer改读typed job context，删除DAG对slot/wave/map和普通trigger/projection的依赖；verifier输出改为Unit+attempt路径避免覆盖。检查 `presets/schemas/parallel-forge.yml`：当前slot_index只在failure描述文本中，不能误删wave兼容schema字段；公开required_fields/topic/triggers/publishes原则上不变，若本Unit改变任何一项必须逐层同步event_loop step-close/correction、preset_lint、真实BDD及schema。更新 `presets/en/parallel-forge-preset-author-notes.md`、`CLAUDE.md` 与 `AGENTS.md`、`.cursor/rules/multi-hat-isolation.mdc`、`scripts/ralph-zsh-plugin.zsh` 的当前DAG说明，安装并加载补全。manifest/index/PRESETS不增删重命名，逐项确认无需条目变化。

检查并仅修正 `crates/ralph-core/data/ralph-tools.md`、`ralph-tools-emit.md`、`ralph-tools-opac.md` 的通用可执行规则：当前context提供身份、先policy-check、缺输入停止，不注入SQLite路径、内部符号或preset专属流程。两套 `skills/ralph-preset-{author,review}/references/{commands,finding-rubric,prompt-visibility,agent-native-model}.md` 更新本次DAG job输入/可见性约束；不进行F12独立全库词汇清理。若原锚点引用源码行号，逐条sed复核；用真实help与check-cli-doc-drift核验。

**14. 集成验证**

真实CLI/common::ralph_bin、Git、SQLite、PTY、EventLoop；仅AI backend脚本Fake。planner/guardian仍通过真实事件接纳；不预写active DB，不直接pipeline.advance，不直接queue_integration。新增CLI运行使用已存在run --no-tui与配置custom backend。 执行当前acceptance过滤及C1/C2的相关模块集；预期全部通过且所有关键副作用有实物断言。

**15. 风险驱动测试**

Contract/State-machine：artifact在worktree不可读会被文本测试漏掉；用runtime真实物化路径读文件并校验。S19增加backend barrier处OS kill/restart变体；精确SQL窗口沿用U6–U10的reopen测试并如实标注。

**16. 回归范围**

C1–C8、C9a、C9b、F1–F4；preset_lint三条必跑；现有integration_dag_scheduler inspect仍绿；污染环境CLI测试；最终全量脚本。每Unit另执行B1/B2/B3/B4（对应受影响crate）；最终Unit执行第10节。失败不得进入下一Unit。

**17. 预期文件变更**

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| 本Unit第6项既有生产路径 | 修改现有生产文件 | 接入本Unit可观察行为 | E14,E15,E16,E21 |
| 本Unit明确标“新增”的模块/SQL/fixture | 计划新增生产模块、迁移或测试fixture | 按职责承载本Unit边界；没有列新增时为空集 | E14,E15,E16,E21 |
| 本Unit第6/9项测试位置 | 新增行为测试，保留既有有效断言 | 证明目标及负例 | E14,E15,E16,E21 |
| 本Unit第13项文档与preset | 修改文档/配置与必要schema说明 | agent真实输入与说明一致 | E14/E21 |\n
**18. 完成标准**

G+第10节全量门禁；F01–F10各有正式入口证据；不是159旧测试全绿即完成。不可预填新增测试结果。

**19. 停止条件**

必须执行公共H；特别是验收Red未出现或本Unit需要未来能力时停止，不把缺口转交下一Unit。

**20. 风险与注意事项**

如果完整CLI暴露非输入契约的新生产缺陷，停止并回溯对应Unit，补真实Red后修订计划；禁止在U15顺便修复无归属runtime行为。 检测由第9/11/15项测试负责；剩余风险在Unit Close证据中记录，不以“全部绿”删除边界说明。

## 8. Unit 串行依赖图

```text
U1 → U2 → U3 → U4 → U5 → U6 → U7 → U8 → U9 → U10 → U11 → U12 → U13 → U14 → U15
```

| 前→后 | 使用的已验证能力 / 排序理由 | 防止提前实现 |
|---|---|---|
| U1→U2 | candidate与pool计量先正确，资源reserve才有可靠入口 | U1不写lease表 |
| U2→U3 | 首job已事务reserve资源，stage接续沿同owner | U2不保存stage成果HEAD |
| U3→U4 | candidate来源HEAD可核验，再定义目标物化 | U3不改变target CAS |
| U4→U5 | 目标工作区与HEAD一致后才将其作为依赖输入 | U4不改变base选择 |
| U5→U6 | current stage evidence与不可变unit base足以重建后继 | U5不恢复缺失job |
| U6→U7 | verify恢复能进入integration，随后补intent断点 | U6不重建candidate |
| U7→U8 | 集成record/ack可靠后才能判断all done | U7不动terminal投递 |
| U8→U9 | 两者无功能依赖；固定顺序先关闭完成端恢复，再处理输入端；各自独立可提交 | U8不改registration |
| U9→U10 | accepted plan/approval证据先存在，才能原子激活 | U9不替代activation事务 |
| U10→U11 | 活动plan和资源域有确定授权，correction可事务reserve | U10不放开job transition |
| U11→U12 | 复用durable correction去重、预算和pending | U11不把verify accepted当失败 |
| U12→U13 | gate超时成为已可处理的integration failure | U12不新增进程runner |
| U13→U14 | 有界可取消runner是worker可靠shutdown前提 | U13不异步改tick |
| U14→U15 | 控制面能持续推进与取消，完整上下文canary才不被无关阻塞掩盖 | U14不写prompt合同 |

除明确的功能依赖外，这也是强制执行顺序，不能因模块不同而并行。每个箭头都意味着前Unit的全部TDD/回归/Close完成。

## 9. 执行命令清单

所有 targeted nextest 命令必须在**测试子进程**前加以下完整前缀；不能用于正式 `ralph emit` 等业务命令：

```bash
env -u RALPH_CURRENT_HAT \
    -u RALPH_CURRENT_LOOP_ID \
    -u RALPH_EVENTS_FILE \
    -u RALPH_WAVE_WORKER \
    -u RALPH_TRIGGERED_HAT \
    -u RALPH_HATS_SOURCE \
    -u RALPH_CONFIG \
    cargo nextest run -p ralph-cli --bin ralph -- dag_scheduler
```

下表C系列给出此前缀后的 `cargo ...` 部分，拼接方式与上方完整示例完全相同。单测时将模块substring替换为本Unit第9项明确测试名；不能把未命中的0 tests当通过。C1/C2/C5单测例分别是 `-- dag_tick_refills_ready`、`-- dag_gate_timeout_reaps_descendants`、`-- dag_registration_receipt_precedes_projection`。

| ID | 实际命令 | 时机 / 验证目的 | 预期 / 失败能否继续 |
|---|---|---|---|
| V0 | `cargo nextest --version` | 启动验证前确认mise钉死版本 | 0.9.140；不符先修工具，不把其失败记Red |
| C1 | `cargo nextest run -p ralph-cli --bin ralph -- dag_scheduler` | 每Unit正式runtime相关回归；单测改最后过滤词 | 非零测试全部pass；否 |
| C2 | `cargo nextest run -p ralph-core --features supervisor-db -- supervisor` | admission/store/lane/migration/runner规则 | 全pass；否 |
| C3 | `cargo nextest run -p ralph-core --features supervisor-db -- migrations` | 每次SQL迁移与旧数据/reopen | 全pass；否 |
| C4 | `cargo nextest run -p ralph-cli --bin ralph -- integration` | Git集成、intent、gate及相邻调用 | 全pass；否 |
| C5 | `cargo nextest run -p ralph-core --features supervisor-db --test dag_registration` | U9起新增真EventLoop integration target | 全pass；此前文件不存在，不提前执行 |
| C6 | `cargo nextest run -p ralph-core --features supervisor-db --test scenarios` | U9/U15真实workflow BDD回归 | 全pass；否 |
| C7 | `cargo nextest run -p ralph-cli --test integration_dag_scheduler` | U15正式CLI主路径/恢复及原inspect回归 | 全pass；否 |
| C8 | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | builtin变更后的第一必需校验 | 全pass；否 |
| C9a | `cargo nextest run -p ralph-core -- preset_lint` | builtin变更后的第二必需校验 | 全pass；否 |
| C9b | `cargo nextest run -p ralph-cli --bin ralph -- presets` | manifest/PRESETS/strict parity第三必需校验 | 全pass；否 |
| B1 | `cargo fmt --all -- --check` | 每Unit Refactor后 | 无diff；否 |
| B2 | `cargo check -p ralph-cli -p ralph-core --all-targets --all-features` | 每Unit类型与feature组合 | 成功；否 |
| B3 | `cargo build -p ralph-cli -p ralph-core --all-features` | 每Unit Build | 成功；否 |
| B4 | `cargo clippy -p ralph-cli -p ralph-core --all-targets --all-features -- -D warnings` | 每Unit Lint | 零warning；否 |
| B5 | `cargo check -p ralph-cli --no-default-features` | durable代码改动后的feature-off | 成功；否 |
| B6 | `cargo check -p ralph-core --no-default-features` | core无SQLite默认路径 | 成功；否 |
| F1 | `./scripts/run-tests.sh` | U15 Close前与最终交付；两阶段nextest+doctest | 全通过；否 |
| F2 | `cargo run -p ralph-e2e -- --mock` | 最终相邻mock E2E；加同样测试子进程环境清理 | 成功；否；不能代替C7 |
| F3 | `cargo clippy --all-targets --all-features -- -D warnings` | 最终workspace消费者Lint | 零warning；否 |
| F4 | `cargo build --workspace --all-features` | 最终全构建目标 | 成功；否 |

Core当前默认无supervisor-db，不能只运行默认core suite便声称SQL代码覆盖。U9的新integration target按现有Cargo自动发现测试文件；不用伪造已有test target。

**污染环境验收（不先scrub这条外层命令）：**

```bash
RALPH_CURRENT_HAT=executor RALPH_EVENTS_FILE=/tmp/x.jsonl \
  cargo nextest run -p ralph-cli --test integration_dag_scheduler
```

测试内部必须使用 `common::ralph_bin()`/scrub再显式注入需要的agent env；外层全量脚本清理不能替代该要求。实际fake agent调用emit必须保留runtime注入。

**U15文档与补全验证：**

```bash
cargo run -p ralph-cli --bin ralph -- emit --help
cargo run -p ralph-cli --bin ralph -- tools skill --help
cargo run -p ralph-cli --bin ralph -- preset check --help
scripts/check-cli-doc-drift.sh
cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh
zsh -f -c 'autoload -Uz compinit; compinit; source ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh; (( $+functions[_ralph] ))'
```

补全必须保持builtin带冒号值使用compadd；已确认脚本入口为`_ralph()`。operator skill references与help逐项核对，用现有 `skills/ralph-preset-review/fixtures/aaf-review-negative-fixture.yml` 按review流程重跑；不引入仅包含某段prompt文案的测试。

所有行为单测从第5/7节给出的入口进入。Contract tests就是typed context→真实文件读取→policy-check/emit；没有单独不存在的contract工具。关键E2E是C7中的S19及真实进程恢复变体；F2只是相邻mock框架回归。

全量如果出现已证实的竞态/时序flake，按仓库唯一兜底 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh`。记录原失败、证据、重跑结果；serial仍失败必须修复。不要手动裸跑全workspace nextest跳过隔离策略，不裸跑cargo test -p ralph-cli。

## 10. 最终质量门禁

- R1–R16与S1–S19均有通过的实测证据；第6节无遗漏；Unit严格U1→U15执行，每个都有Acceptance Red、最小Unit Red/Green、Refactor、Integration、Regression、Close记录。
- 预期Red与实际Red逐条对照；未出现Red时先调查覆盖或基线变化。不能预先写“新测试通过”。已有159条定向测试全绿只算E18基线。
- Characterization与负例保持：wave/shadow/default-feature、path escape、wrong repo/symlink、token/source、resource、dirty worktree、approval拒绝、旧数据不明状态、migration wave行保留。
- State-machine/idempotency/双连接concurrency、Git与append故障、gate timeout/cancel后代回收均有结果。精确SQL断点reopen与OS kill/restart区别报告，不混称。
- C1–C8、C9a、C9b、B1–B6、F1–F4通过；必要preset/schema parity、静态doc drift、CLI help、补全安装/加载与operator review流程通过。
- 无新增失败/skip/ignore/.only，无断言弱化、无source-only替代runtime、无无解释Snapshot/Golden更新、无扩大timeout掩盖阻塞。
- 每次迁移兼容旧wave数据；旧DAG可信字段不能重建时保全并blocked，不自动reset或清库。
- 新增模块有明确职责且全部源码≤5000行；特别检查inner.rs与legacy.rs；移除废弃实现和实验代码。
- 检查注入skill可执行性：触发、动作、字段来源、停止条件明确；无内部ledger路径/模块符号/计划编号/preset专属叙述泄漏。
- 未验证内容明确：真实AI后端质量和任意外部writer竞态不在本计划自动验收承诺；不把mock backend通过写成真实AI loop通过。
- 剩余风险明确：跨文件系统物化混合状态、未知PID、无法核验的旧数据会blocked；gate外部服务副作用不回滚；磁盘长期不可写不能保证送达。
- 无未处理BLOCKED决策；所有实施关键置信度≥0.85。超出F01–F10及必要同步的发现回到计划修订，F11–F13仍为独立待办。
- 过程性scratch/residuals不得commit；有价值结论合并plan或docs/solutions；本轮只交付计划，没有授权实现或发布。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是Roadmap吗 | 是 | 15个行为Unit，各自指定入口、Red原因、变更边界与验证 |
| Executor是否仍需做关键设计决策 | 否 | D1–D16与实施协议固定store/校验/错误/幂等/兼容/timeout/模块边界 |
| 所有文件和接口是否有代码库证据 | 是 | E ledger；新增路径显式标注，命令检查及文档复核不冒充已有接口 |
| 所有关键决策置信度是否≥0.85 | 是 | 第3节逐项评分与支持依据 |
| 是否存在未处理的低置信度假设 | 否 | 未运行实现结果与已确定方向分离；新反证走H |
| 每个Unit是否只有一个可观察行为 | 是 | 补槽、lease、接续、物化、依赖base及各恢复边界分别验收 |
| 每个Unit是否可以独立验证 | 是 | 第7节第9/14/16项；仅使用已有和前置已完成能力 |
| 每个Unit是否有真实Red | 是（规定了真实Red验收；尚未执行） | 15个明确失败表现；不把语法/fixture/环境错误计入Red；实际记录由执行产生 |
| 每个Unit是否包含回归范围 | 是 | 每Unit第16项与第9节命令 |
| 是否存在未来Unit依赖 | 否 | 第8项明确禁止；U13同步但有界为独立交付，U14才验证响应性 |
| 是否存在泛化任务描述 | 否 | 每项关联具体行为/入口/断言，G/H为显式公共完成/停止约束 |
| 所有Scenario是否可追踪到测试和Unit | 是 | 第5/6节矩阵；S16–S18参数化到各owning Unit |
| 所有关键决策是否有Evidence | 是 | D表引用E；Git外部语义另有官方文档与本轮实验 |
| 计划是否可以严格串行执行 | 是 | 第8节唯一线性顺序；无并行开发Unit |
| 是否编写生产代码 | 否 | 本次仅计划文件；nextest与临时Git实验未修改生产代码 |
| 是否把报告评级扩大 | 否 | P0=0，P1 F01–F10；P2仅必要同步交集 |
| 是否完整声明未验证内容 | 是 | 第0/10节；新Red/Green、完整CLI、全量尚未执行 |

文档复核遵循ce-doc-review的coherence、feasibility、scope、security、adversarial检查，按仓库工具映射在主线程串行完成；没有独立子agent或跨模型复审，不将其称作独立共识。已修订7组约束：持久化数据合同、intent历史与ack恢复、delivery写者/尾部保护、最终accepted证据位置、取消/CAS线性化、测试命令引用、文档基线。未留下待用户选择的实施方案；计划结构检查确认12节、15个Unit、每Unit20项，`git diff --check`通过。
