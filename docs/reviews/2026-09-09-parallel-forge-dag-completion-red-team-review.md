---
title: Parallel Forge DAG scheduler 完成度与同步情况攻击性审查
date: 2026-09-09
reviewed_head: c11309acef260f5fa812d0f42771300e847d7fcc
source_plan: docs/plans/2026-09-03-0959-feat-parallel-forge-runtime-dag-scheduler-plan.md
verdict: not-ready
scope: report-only
---

# Parallel Forge DAG scheduler 完成度与同步情况攻击性审查

## 1. 结论

**已有大量实现和后续修复，但不能按原计划的验收条件认定完成。** 三态配置、artifact 校验、durable job journal、PTY 执行、集成 candidate/CAS、部分恢复和 preset 切换均已落地；正常 Unit 接续、持续补槽、依赖基线、恢复收敛和 agent 输入契约仍有缺口。

本报告以当前 HEAD 为准，包含原计划之后的修复，不把已经修复的路径授权、结果身份校验、PTY 超时进程组处理等旧问题重新计入。按照用户最新要求，只定位问题并输出报告；没有修改生产代码、preset 或 skill，没有编写修复开发计划。

评级：**P0 0 项，P1 10 项，P2 3 项，共 13 项。** P0 为有证据的紧急灾难性风险；P1 为会阻断主要工作流、破坏关键运行契约或使核心验收失真的问题；P2 为可观测性、操作规程及只读边界问题。没有为了“Red Team”角色而提高评级。

这是主线程完成的攻击性代码审查及局部实验，未运行独立模型复审，也未启动真实 AI 后端的完整 Ralph loop。结论不代表穷尽所有缺陷。

## 2. 验证范围与证据等级

- **E1：隔离实验确认。** 临时 Rust harness 调用当前源码，断言实际错误行为；通过表示问题被复现，绝不表示产品验收通过。
- **E2：源码调用链确认。** 已阅读生产入口、调用者及相关持久化/状态转换，给出具体触发条件；尚未对每个窗口执行真实进程故障注入。
- **E3：验收或同步缺口。** 证明现有证据不能支持某项完成声明，不推断所有相关行为都失败。

已执行：

1. 当前仓库 `cargo nextest run -p ralph-cli --bin ralph -- dag_scheduler`：**159 passed，1951 skipped**。复核运行 ID：`eae491fb-31b2-4f36-8ca8-6cec0cfaaadb`。
2. 临时 harness 的三个攻击实验：**3 passed，20 skipped**，运行 ID：`cbd2917a-7e0a-44f3-95e5-0a8c60a92025`。对应 F01、F02、F04。
3. 测试命令只在子进程上清除了仓库要求的七个 Ralph agent-context 环境变量。

实验位于 `/tmp/ralph-dag-red-audit-2gIINo/`，不进入 git。Admission 与 Git port 直接引用当前 `ralph-core`；worktree 实验复制当前 `worktree.rs`，仅将其 `resume` 模块定位改为原文件的绝对路径，以解决独立 harness 的模块寻址问题，未改变业务逻辑。临时 harness 自行解析依赖版本，因此它是源码行为复现，不替代仓库锁文件下的基线验收。首次 harness 编译因模块路径失败，修正仅限临时目录后通过。

**未执行全量 `./scripts/run-tests.sh`、完整 mock E2E、跨进程 crash matrix 或真实 AI loop。** 本次是审查报告，不宣称实现最终验收通过。

## 3. 问题总表

| ID | 级别 | 问题 | 证据 | 原计划关联 |
|---|---|---|---|---|
| F01 | P1 | 全量静态 Unit 列表反复参与 admission，已完成项阻塞后续补槽 | E1+E2 | R3/R4/R16，U4/U6 |
| F02 | P1 | executor 提交后，reviewer/fixer 的 worktree acquire 拒绝已有成果 | E1+E2 | R5/R7，U6/U7 |
| F03 | P1 | 依赖解锁后的 Unit 仍从审批时旧基线启动 | E2 | R6/R7，U7 |
| F04 | P1 | CAS 更新当前分支 ref，却不更新目标工作区和 index | E1+E2 | R8/R13，U7/U10 |
| F05 | P1 | durable terminal/intent/emit fence 尚未形成完整恢复闭环 | E2 | R10/R13，U9 |
| F06 | P1 | plan receipt 晚于 EventLoop 投影，注册 crash window 仍存在 | E2 | R17/S20，U3/U5/U9 |
| F07 | P1 | correction 生产入口与 journal 允许的状态转换不一致 | E2 | R9/S8，U8 |
| F08 | P1 | 集成 gate 同步阻塞 scheduler，且无执行时限 | E2 | R3/R9，U7/U8 |
| F09 | P1 | DAG job prompt 与 preset 输入要求不匹配 | E2 | R5/R15/R18，U6/U10 |
| F10 | P1 | 关键测试绕过正式 DAG 调度，不能证明完整流程已通过 | E3 | S4/S7/S12/S15/S20，U10 |
| F11 | P2 | inspect 只有 receipt 计数，shadow 无跨进程观测事实 | E2+E3 | R11/R12，U5 |
| F12 | P2 | preset author/review 的执行模型词汇和负例仍停留在旧契约 | E3 | U10 文档/skill 同步 |
| F13 | P2 | 声称只读的 inspect 通过可写连接执行数据库迁移 | E2 | R12，U5 |

## 4. P1 问题定位

### F01：已完成 Unit 仍占 admission 配额，后续 Unit 可能永远不启动

**位置：** `crates/ralph-cli/src/loop_runner/dag_scheduler/mod.rs:1030`（`tick`）；`crates/ralph-core/src/supervisor/dag_scheduler.rs:131`（`compute_admissions`）；`crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs:969`（`maybe_spawn_executor`）。

`tick` 每次把 `plan.units` 全量送入纯调度器，没有去掉 executing、reviewing、verified 或 integrated Unit。纯调度器每次把 leased permits、admitted_count 和 executor_count 从零开始，按 integration_order 分配名额。随后 `maybe_spawn_executor` 才用 `executors_launched` 去重。

**最小触发：** global=executor=1，独立 U1/U2，U1 排在前面。即使 U1 已 integrated，下一 tick 仍得到 U1=Admitted、U2=BlockedGlobalCap；spawn 又跳过已经启动过的 U1。U2 无法填补空槽。相同问题也影响容量为 1 的资源：已结束的前序候选继续在纯快照中占位。

**实测：** `audit_completed_unit_still_consumes_admission_cap` 将 U1 放入 integrated 集合，仍得到上述决策。运行时全量构造快照与延后去重使这个纯函数行为成为生产缺陷。

此外，当前 runtime admission 没有体现原计划要求的跨阶段 durable resource lease 获取、持有和释放；不能把同一 tick 内的局部计数当作 R4 的完整实现。此处不额外声称已经实测资源超售。

**影响：** “存在 Ready work 就持续补槽”这一核心目标不成立。应优先于调大 worker cap 处理。

### F02：正常提交使下一阶段 worktree acquire 失败

**位置：** `crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs:1070`；`crates/ralph-cli/src/loop_runner/dag_scheduler/worktree.rs:208`。

所有 SpawnKind 共用同一条 `UnitWorktree::acquire(..., approval_base)` 路径。acquire 仅在 existing branch tip 等于 verified_base 时允许 resume，否则返回 `BaseMismatch`。

**最小触发：** executor 在 Unit worktree 提交一个正常代码 commit，并成功结束。reviewer 启动时再次 acquire 同一 worktree，此时 tip 已经是 executor commit，不再是 approval base，启动被拒绝。fixer/verifier 接续也受同一检查影响。

**实测：** `audit_committed_executor_worktree_rejected_for_review` 创建真实 Git 仓库和 Unit worktree、提交文件修改，再次调用 acquire，稳定得到 `BaseMismatch`。

源码已经有 `UnitWorktree::resume`，但正式阶段接续入口没有以“已确认成果 HEAD + 初始 base”调用它。不能以 resume helper 存在作为此问题已解决的证据。

**影响：** 正常 executor→reviewer 链路无法接续已提交成果；与 preset 声称重用代码和 commits 的契约冲突。

### F03：后继 Unit 无法在启动时读取前置成果

**位置：** `crates/ralph-cli/src/loop_runner/dag_scheduler/mod.rs:819`；`crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs:1070`；`crates/ralph-cli/src/loop_runner/dag_scheduler/integrate.rs:378`。

审批时只记录一个 plan-level `verified_base_commit`，后续所有 Unit 启动都读取这个值。`on_unit_integrated_accepted` 只把 Unit ID 加入 integrated set，没有给后继 Unit 固定新的可信 base。

**触发：** U2 depends_on U1；U1 集成新增一个 U2 必须引用的模块。U2 admission 等到了 U1 integrated，但创建 worktree 时仍使用审批前 SHA，因此该模块不存在。

**影响：** 依赖约束仅控制启动时机，没有保证后继的代码输入包含依赖成果。最终 integration 时再 squash 到新 target 不能弥补 U2 编码和 targeted verification 时缺失依赖的问题。原计划 S9 明确要求从“依赖已集成的 commit C”启动。

### F04：集成成功后目标工作区仍停留在旧代码

**位置：** `crates/ralph-core/src/supervisor/integration_lane.rs:598`（`compare_and_swap_ff`，实际写 ref 在约 640 行）；`crates/ralph-cli/src/loop_runner/dag_scheduler/mod.rs:692`（target 取当前分支）。

生产 integration 用 `git update-ref refs/heads/<target> <candidate> <expected>` 推进分支。该操作只更新 ref，不更新这个已 checkout 分支的 index 和文件。当前 runtime 又将 loop 工作区当前分支作为 integration target。

**实测：** `audit_cas_moves_checked_out_ref_without_checkout` 使用真实 `RealGitIntegrationPort` 准备 candidate 并 CAS；结果是 `Advanced`，HEAD 已变化，文件内容仍为 `base`，`git status --porcelain` 非空。

**影响：** 后续在 loop 工作区执行的验证可能读取旧代码；后续 Unit acquire 的 host-clean 检查也可能拒绝启动。结果是 Git ref 显示集成成功，而实际工作区并未交付对应代码。本次没有证据表明它自动删除用户文件，因此定 P1 而非 P0。

### F05：持久化记录存在，但多个可恢复窗口仍不能自动收敛

**位置：** `crates/ralph-cli/src/loop_runner/dag_scheduler/mod.rs:279`（`recover_after_restart`）；`crates/ralph-cli/src/loop_runner/dag_scheduler/integrate.rs:77`（`reconcile_after_restart`）、`:300`（terminal emit fence）；`crates/ralph-cli/src/loop_runner/dag_scheduler/integration.rs:254`（intent-before-CAS）。

确认的三个窗口：

1. **阶段 terminal 已写、下一阶段未 reserve。** 正常路径先写 accepted terminal，再推进 pipeline/spawn。恢复循环对 `job.terminal.is_some()` 直接 continue；仅对 verify accepted 有专门 integration 补偿。execute accepted→review、review accepted→verify、fix accepted→review 的断点没有相同恢复。它们可以恢复出“上一阶段已结束、下一阶段不存在”的停滞状态。
2. **CAS 已完成、integration record 未写。** intent 已持久化，但生产恢复没有调用 `get_intent` 对照目标 SHA 完成确认；它扫描 integration records，而 records 恰好在这个窗口不存在。再次构造 candidate 不能替代按原 intent 收敛，且会与旧 intent 一致性检查冲突。检索到的 CLI `get_intent` 消费仅在测试中。
3. **development.done fence 已写、JSONL append 未完成。** live 路径 append 失败后只告警，fence 阻止再次发送；restart 明确把“fence 存在、事件缺失”设为 blocked。这里防止了重复许可，却没有保证最终事件可恢复送达。

**影响：** “拒绝重复执行”已部分实现，但不等于原计划 R10 要求的“不丢失可恢复工作”。对 PID 不明的确实危险窗口选择 block 可以合理；不能把所有已知 durable facts 下的恢复缺口都当成该例外。

本项为源码窗口分析，没有声称完成三个窗口的真实 kill/restart 实验。

### F06：registration receipt 的写入顺序不符合 R17

**位置：** `crates/ralph-cli/src/loop_runner/inner.rs:3720`、`:3740`；`crates/ralph-cli/src/loop_runner/dag_scheduler/mod.rs:669`、`:793`；`crates/ralph-core/src/event_loop/parse_and_emit/legacy.rs:1664`、`:2159`。

主循环先调用 EventLoop 的事件处理，内部完成 state projection 后返回 accepted_events；之后才调用 DAG seam 写 plan-ready receipt。原计划要求的是 receipt 在 ensure-task projection/ack 前可恢复。

**触发：** EventLoop 已接受 plan-ready 并投影 tasks，进程在返回 DAG seam 写 receipt 前退出。恢复入口主要枚举 active DAG plans；丢失 receipt 的任务投影不能仅靠该入口恢复。后续 approval 找不到 receipt 时只告警并跳过。

approval 内部又把 receipt.activate、register_plan、activate_plan 顺序分开执行，verified base 更晚写入；这些窗口不能以“每一个 helper 自己使用 SQLite”推导出整个注册过程原子。

**影响：** 可能出现任务已存在但 DAG authority 未建立，或 activation 不完整。原计划 S20 仍不能验收。

### F07：失败后的 correction 在正式 journal 中被拒绝

**位置：** `crates/ralph-cli/src/loop_runner/dag_scheduler/mod.rs:562`；`crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs:661`、`:1016`；`crates/ralph-core/src/supervisor/dag_store_rusqlite/jobs.rs:157`。

failure-handler 的 accepted correction 请求会尝试新 fixer；但 durable reservation 只允许 review/verify 的 rejected 或 failed 转 fix。

**两个具体不匹配：**

- executor 执行失败后，当前 durable stage=execute、terminal=failed。correction 尝试 fix，journal 的匹配表没有 execute failed→fix，拒绝 reservation。
- integration conflict/gate failure 发生在 verify accepted 之后。correction 尝试 fix，但 journal 没有 verify accepted→integration-failed→fix 这条状态路径；它仍看到 verify accepted。

reservation 被拒时 `spawn_job` 只告警并 return；correction 入口对未 admitted 结果也只记录日志，没有形成统一、可恢复的 pending/blocked 收尾。

**影响：** review reject 路径的局部成功不能证明执行失败或集成失败的 bounded correction 已落地。这里定位状态机不一致，不建议靠放宽所有 transition 验证解决。

### F08：集成 gate 可让 scheduler 长时间停顿

**位置：** `crates/ralph-cli/src/loop_runner/dag_scheduler/mod.rs:1089`；`crates/ralph-cli/src/loop_runner/dag_scheduler/integrate.rs:193`、`:367`；`crates/ralph-core/src/supervisor/integration_lane.rs:372`。

`tick` 同步执行 `maybe_integrate_one`，后者同步调用 targeted gate；gate 使用 `std::process::Command::output()`，没有 timeout、输出流上限或取消接线。

**触发：** 某 Unit 的合法 tests 命令运行十分钟、挂起或持续输出。主 scheduler 的调用栈被占住，不能及时处理其他 job 完成事件并推进补槽；无限挂起还会持有 integration lane。即使 Tokio 其他线程上的 worker 仍运行，调度控制面也无法及时推进。

**影响：** “同一 tick 持续补槽”和有界运行不成立；末尾只截取 500 字符 stderr 并不限制 `output()` 已缓存在内存中的完整输出。

### F09：DAG agent 的实际输入与 preset instructions 不一致

**位置：** `crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs:1256`（完整 `build_job_prompt`）；`presets/en/parallel-forge.yml:762`、`:789`、`:850`、`:889`。

实际 prompt 由 DAG identity/path-policy/schema/tests/feedback block 加当前 hat instructions 构成。没有普通 activation 的完整 trigger/projection/skill 注入流程。

**确认的缺失/矛盾：**

- executor 要求从 runtime job context 读取 `execution_plan_path`、`worktree_map_path`、`slot_index`；实际 block 没提供这些字段。
- executor 的测试资源命名仍要求 `wave_id + slot_index`，DAG job identity 没有这两个输入。
- reviewer 要求从 current projection 读取 execution_plan_path；verifier 要求从 trigger 读取 accepted review evidence。spawn 构造没有传入这些上下文；feedback 仅用于 Fix。
- instructions 把 `ralph-tools-opac` 描述为已注入，实际 builder 没有加入该 skill 内容。要求 agent 另外 load emit skill 不等同于 OPAC 已注入。

**影响：** agent 无法严格按照当前 preset 获得 Unit 任务描述、审查输入和隔离身份。已有 path-policy prompt 修复是真实进展，但不能证明整个 agent 输入契约已经适配。

### F10：现有“DAG 完成”证据跳过关键运行路径

**位置：** `crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs:1627`、`:1673`；`crates/ralph-core/tests/scenarios/parallel_forge_dag_resume_runtime.yml:10`；`crates/ralph-cli/tests/integration_dag_scheduler.rs:17`。

- 补槽 facade 测试直接调用 `pipeline.advance("U-ready", ...)`，没有通过 `tick` 的候选构造，因此 F01 可以存在而测试仍绿。
- real executor canary 在成功事件进入 merge_queue 后停止，没有提交代码后继续 reviewer→verifier→integration 的完整验证，因此 F02 不会被发现。
- `parallel_forge_dag_resume_runtime.yml` 未配置 supervisor.enabled/scheduler_mode: dag；它测试普通 EventLoop 的 hat resume 与 `forge.unit.*` topic，不是 CLI DagSchedulerRuntime 从 SQLite 恢复 jobs。
- CLI integration_dag_scheduler 明确主要覆盖 inspect，不启动完整 `ralph run`。

**影响：** 159 条定向测试通过只能证明各自断言成立，不能支撑原计划 S4/S7/S12/S15/S20 已完成。此项独立于具体 runtime 缺陷：验收框架若继续绕过生产边界，后续修复仍可能获得误导性的绿灯。

## 5. P2 与文档/skill 同步

### F11：inspect 没有提供计划要求的 scheduler 运行态

**位置：** `crates/ralph-core/src/supervisor/dag_inspect.rs:38`、`:119`；`crates/ralph-cli/src/commands/inspect.rs:1404`；`crates/ralph-cli/src/loop_runner/dag_scheduler/mod.rs:648`。

inspect 当前从 receipt rows 构造摘要：admitted_total 是 active/consumed receipt 数，blocked_total 是 pending receipt 数，而非 Unit/job 的实际 admission/阻塞状态。没有各池利用率、资源 owner/blocker、integration queue、Unit stage 或实际 blocked reason。

shadow store 留在进程内；另一个 inspect 进程不能读取其观测。无 DB 时返回空计数，存在旧 DAG DB 时又可能读取旧 receipt 并贴上当前 mode 标签。

**影响：** operator 无法用该入口区分“正在工作”“因资源等待”“恢复已 blocked”。文档写“观测计数”时应明确这实际是 receipt 计数；R12/S14 的丰富运行态要求尚未完成。

### F12：operator skill 更新不完整，词汇及 fixture 与新模式脱节

**位置：** `skills/ralph-preset-author/SKILL.md:65`、`:162`、`:246`；`skills/ralph-preset-review/fixtures/scheduler-mode-negative-fixture.yml:3`；其 `fixtures/README.md:25`。

已更新两套 references 中的 scheduler_mode/commands/finding-rubric/patterns，这是实质同步，不能说“skill 完全没改”。但 author 主流程仍冻结四种执行模型：single-chain / wave / supervisor / supervisor+wave，未纳入仓库当前对 parallel-forge 使用的 supervisor+dag；默认模式升级和最终 checklist 也只列这四种。

negative fixture 的注释仍以“dag 实际等同 wave”“runtime-owned per-Unit admission 是未接线能力”作为预期命中依据，而新 rubric 已改为审查 agent 越权与真实 runtime ownership。该匿名 fixture 本身仍可能因没有实际 runtime contract 而无效，但旧理由已不能证明新版 rubric 的三个攻击轴全部覆盖。

**影响：** author 可能无法表达仓库正式使用的模型；review 验收可能沿用过时理由。需要补齐同步的证据，不能仅靠新增 scheduler-mode 段落判定全部适配完成。

### F13：inspect 的数据库访问不是严格只读

**位置：** `crates/ralph-cli/src/commands/inspect.rs:1430`；`crates/ralph-core/src/supervisor/dag_store_rusqlite.rs:85`、`:638`；`crates/ralph-core/src/supervisor/migrations.rs:119`。

inspect 调用 receipt store 的普通 `open`，内部为 SQLite 可写连接并执行 migrations。数据库文件已经存在不代表 schema 已最新；旧库会被该“查看”命令修改 schema/user_version，还会执行 WAL 等 pragma。

**触发：** 较新 CLI 对已有旧版本 DAG DB 执行 `ralph inspect loop --format json`。

**影响：** 与 read-only inspect 契约不符；取证和恢复前的观察可能改变被观察数据库。当前 DB 已最新时未必有 schema 改动，因此不将影响扩大为每次 inspect 都迁移。

### 同步状态矩阵

| 表面 | 当前状态 | 判断 |
|---|---|---|
| 三态配置及非 DAG 默认路径说明 | 源码及仓库说明均存在 | 已有实现，不列为缺失 |
| AGENTS.md / CLAUDE.md 的 DAG authority 描述 | 已说明 active DAG、runtime jobs/integration/completion | 已同步主要定位；不能作为 runtime 全部兑现的证明 |
| `.cursor/rules/feature-flags.mdc`、`multi-hat-isolation.mdc` | 已说明 active DAG 与旧 wave 声明非 authority | 主要定位已同步；观测/恢复能力仍须按实际限制理解 |
| author/review references 的 commands、finding-rubric、patterns | 已更新 scheduler mode 与 runtime ownership | 部分完成 |
| author 主 SKILL.md 的模型菜单/checklist | 仍只有旧四种模型 | F12 |
| review scheduler negative fixture 与 README | 预期理由仍为旧过渡态 | F12 |
| parallel-forge executor/reviewer/verifier instructions | 已改 per-Unit topics，但仍依赖旧输入及未提供 projection | F09 |
| `crates/ralph-core/data/*.md` | 有通用 task projection 指引；不是全部需要写 DAG operator 说明 | 不因缺少 DAG 字样单独报错；实际 skill 可见性与 job 行为矛盾见 F09 |
| DAG inspect 文档及接口 | 当前为 receipt 摘要，非完整调度运行态 | F11/F13 |

通用注入 skill 应保持 agent-facing、可复用，不能为了同步把 scheduler 内部 DB、原计划编号或 operator 启动流程塞进去。这里的问题是实际输入/动作契约没有闭合，不是文档中 DAG 关键词数量不足。

## 6. 原计划完成度评估

| 原 Unit | 当前证据 | 结论 |
|---|---|---|
| U1 mode gate | 三态配置、validation、runtime authority gate 均存在 | 主要实现已落地；本轮未独立重跑所有配置组合 |
| U2 artifact v2 | canonical handoff、typed resource/path metadata 被生产拓扑消费 | 已有实现；不对全部恶意 artifact case 作通过声明 |
| U3 durable state | receipts/plans/jobs/intents/terminal fences 均存在 | 部分完成：注册顺序及完整恢复状态仍缺，F05/F06/F07 |
| U4 admission/resource | 纯引擎及测试存在，已接 tick | 未达到核心验收，F01 |
| U5 shadow/inspect | shadow sink、CLI scheduler block、receipt 摘要存在 | 部分完成，F11/F13 |
| U6 per-Unit jobs | 真实 PTY launch、身份字段、job terminal、follow-up wiring 存在 | 部分完成，F02/F09 |
| U7 worktree/integration | candidate、targeted gate、CAS、真实 Git contract 已存在 | 未达到完整交付条件，F02/F03/F04/F08 |
| U8 timeout/correction | live PTY lease 与 correction attempt 机制存在 | 部分完成，F07/F08；恢复 deadline 另有待测风险 |
| U9 recovery/exactly-once | PID adoption、部分重投影、重复 fence 和 reopen tests 存在 | 未覆盖全部 crash windows，F05/F06 |
| U10 cutover/regression/docs | builtin 已切 dag，references 和局部测试已更新 | 切换已发生，但验收和输入同步未闭合，F09/F10/F12 |

不提供“完成 90%”一类比例：这里缺的是正常流程与恢复流程的必要条件，不能按文件数、commit 数或测试数量平均成完成率。

## 7. 未升级为已确认问题的风险

以下仅作为下一次审查的关注点，不计入 13 项评级：

- 恢复活 PID 的 monitor 从恢复时重新计时，而且超时分支只发送 completion；原 hard deadline 是否可跨 restart 延长、是否仍有写入中的旧进程，需要真实存活进程实验。位置：`dag_scheduler/spawn.rs:321`。
- gate 在临时 checkout 中运行后没有显式验证 tracked tree 是否仍等于 candidate。会改源码的测试/生成器可能使实际验证内容与 candidate 不同；尚未做对照实验，不声称已有确定绕过。
- in-memory pipeline、executors_launched 及 job_id 的 Unit 身份部分未带 plan_key，而 durable unit_key 已带；同一运行范围多 plan 复用 Unit ID 的正式可达性需要进一步核实。
- 当前实现使用独立 `.ralph/dag.db`，偏离原计划复用 supervisor SQLite 的设计。这里只记录设计偏差；没有把“两个 DB”本身等同于已发生一致性事故。

## 8. 报告边界

本报告已经定位主要缺口，并明确其影响与验证程度。**整体评级：Not ready，尚不能按原计划声明完整完成。** 当前最优先的事实是 F01/F02/F04 三个已复现的正常流程故障；F03、F05–F09 是源码可定位的关键契约缺口；F10–F13 解释了现有绿灯与同步记录为什么不足以证明完成。

本轮唯一仓库交付是本 Markdown 报告。没有修复实现，没有创建修复计划，没有 commit/push，也没有运行会消耗真实 AI 后端的 Ralph loop。
