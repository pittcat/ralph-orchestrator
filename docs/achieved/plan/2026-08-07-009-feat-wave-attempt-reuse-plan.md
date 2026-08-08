---
title: Wave 尝试账本与恢复复用 - Plan
type: feat
date: 2026-08-07
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
deepened: 2026-08-07
---

# Wave 尝试账本与恢复复用 - Plan

## Goal Capsule

在不替换 Ralph 编排模型、不改变 Tokio 并发和现有 wave 终态语义的前提下，把 supervisor slot 的每次 Worker 尝试记录到现有 `SupervisorStore`。同进程 retry 继续复用原 Worktree；redrive 子 wave 在 `ralph run --resume` 派发时读取父 slot 的持久化尝试历史，并仅在旧 Worktree 仍是 Git 登记的隔离 Worktree 时复用它，否则创建新的隔离 Worktree。任何已有提交、脏树或历史成功记录都只是恢复证据，不能跳过当前 Worker 的测试、终态事件或 supervisor 终态提交。

执行权威顺序为：本计划的 Requirement → Key Technical Decision → Unit → 当前代码与仓库硬规则。若实现发现必须改变公开 CLI、配置字段、事件 schema、retry budget、终态 cleanup、Tokio `Semaphore`/`JoinSet` 并发模型或 wave terminal projection，立即停止并重新规划。

执行配置为严格串行 `U1 → U2 → U3`。本计划依赖 `2026-08-07-008-refactor-wave-dispatcher-module-split-plan.md` 先完成：U1/U2/U3 开始前，必须通过 `rg` 将 dispatcher 符号重新定位到按职责命名的模块；不得把拆分工作混入本计划。每个 Unit 必须完成 Acceptance Red、Unit Red、Green、Refactor、Integration、Regression 和独立提交检查后才能进入下一 Unit。Tail ownership 由执行该计划的 Coding Agent 持有；本计划不授权提交、推送或创建 PR。

## 0. 计划状态

- 状态：`READY`。所有实施关键决策置信度均不低于 `0.85`，没有 launch-blocking open question。
- 代码库基线：当前 HEAD `181638ac349319551b7d8a6c627ea4ca026646b7`；相对原基线仅增加 event-policy 回归测试与计划文档更新，supervisor/wave dispatcher/worktree 生产代码未变化；执行前仍需确认工作树无关改动不会污染验证。
- 调查范围：supervisor store 与 migration、wave dispatcher retry、redrive boot dispatch、startup recovery、Worktree binding/cleanup、injected agent skill、相邻 tests、相关 Git 历史与 `docs/solutions/`。
- 已执行验证：
  - `cargo nextest --version` → `cargo-nextest 0.9.140`。
  - `cargo nextest run -p ralph-core --lib -E 'test(/supervisor::memory::tests::bind_worktree_rebind/) or test(/wave_prompt::tests::u2_retry_prompt/)'` → 4/4 通过。
  - `cargo nextest run -p ralph-core --features supervisor-db --lib -E 'test(/run_bumps_user_version_to_current/) or test(/run_is_idempotent_across_reopens/) or test(/required_tables_exist_after_run/)'` → 3/3 通过。
  - `cargo nextest run -p ralph-cli --bin ralph -E 'test(/executor_retry_uses_fresh_pid_same_cwd/) or test(/third_attempt_prompt_contains_both_prior_failures/) or test(/timeout_retry_does_not_claim_existing_commit_success/) or test(/test_u4_redrive_boot_dispatch_in_memory_multi_slot/) or test(/test_s3_rusqlite_backed_wave_supervisor_dispatch/)'` → 5/5 通过。
- 已补充验证：当前工作树干净，`cargo fmt --all -- --check` 通过；格式化 warning 属于可修复的机械问题，实施前应运行 `cargo fmt --all` 后再执行 `just fmt-check`，不应作为不可修复的启动阻塞。尚未执行 `./scripts/run-tests.sh`、`just lint`、`cargo build --workspace`；这些仍是实施期强制门禁。dispatcher 拆分计划尚未实施，本文中的 dispatcher 物理路径仍是拆分前证据。
- 阻塞项：无。

## 1. 功能目标

### 1.1 业务目标与调用方

业务目标是让 Ralph 的 slot retry 从“只存在于单个 Tokio task 的临时记忆”升级为“可重开、可诊断、可供 redrive 恢复使用的最小执行账本”，降低崩溃后重复探索和覆盖已有成果的概率。

调用方包括：

- supervisor wave dispatcher：创建和结束 attempt receipt。
- `ralph run --resume` 的 redrive boot dispatcher：读取父 slot 历史并构造恢复上下文。
- 新启动的 wave Worker：从 prompt 观察历史 Git 状态、稳定失败码以及 Worktree 是否真正复用。
- 内存与 rusqlite store 测试/诊断调用方：验证同一 `SupervisorStore` 合同。

### 1.2 当前行为

- 同进程自动 retry 已在同一个 Tokio task 内运行。每次 retry 启动新 Worker 进程，但复用相同 `WorkerRequest.cwd`。
- `prior_attempts` 只存在于 dispatcher 局部 `Vec`。进程退出后历史丢失。
- `wave_slots.attempt_count` 与 `max_attempts` 已由 v6 migration 创建，但正常 retry 路径不递增，也不能表达每次尝试的 Git 起止状态和失败码。
- `ralph run --resume` 只自动派发已创建但仍 Pending 的 redrive 子 wave；普通 active wave recovery 不会重新执行 Worker。
- redrive 子 wave复制 descriptor 与 parent-slot 映射，但不复制 slot resource，不读取父 attempt 历史。生产 `bind_slot` 总是为 Exec/Fix 新建 `{loop_id}-{kind}-{child_wave_id}-{slot}` Worktree。
- loop 业务终态会清理所有 supervisor slot Worktree。因 `kill -9` 未执行 cleanup 而幸存的 Worktree 可能仍可用；正常终态已清理的 Worktree 不可复用。

### 1.3 目标行为与行为差异

#### Attempt 账本

R1. 当 attempt store 健康时，每个 supervisor Worker attempt 在执行前获得单调递增的持久化 `attempt_seq`，保存起始 Git checkpoint；执行结束后保存 `succeeded` 或 `failed`、稳定失败码和结束 Git checkpoint。store 异常按 R3 降级。

R2. `attempt_seq` 分配在同一 `(wave_id, slot_index)` 下必须并发安全；rusqlite 重开后继续递增，重复的同值 finish 幂等，冲突 finish 被拒绝。

R3. attempt receipt 写入、读取或 Git checkpoint 探测失败时，只记录脱敏 warning 并继续原 Worker 流程；不得把原本会成功的 Worker 改判失败，也不得改变最终 slot outcome。

#### Retry 与恢复

R4. 同进程 retry 保持现有行为：新进程、相同 cwd、只暴露最终 attempt 的 progress/RPC/TUI/tracker/store terminal outcome、现有 retry budget 与 deadline 不变。

R5. redrive 子 wave 的恢复查询必须按现有 parent-child slot 映射读取父 slot receipt；查询成功时，`ralph run --resume` 派发第一个 child Worker的 prompt 显示持久化历史和 Worktree 复用结论。查询异常按 R3 降级且不得伪造历史。

R6. redrive 或任何同一 slot 的重新绑定仅在该 source 没有未终态 `running` receipt，且持久化路径仍存在、被 `git worktree list --porcelain` 登记、不是主 Worktree、分支与持久化 binding 匹配时复用；任一条件不满足都沿现有 factory 创建新的隔离 Worktree。

R7. 恢复 prompt 必须明确：历史内容是证据而不是指令；`succeeded` receipt、已有 commit、相同 HEAD 或干净状态均不能跳过当前测试和结果事件。

#### 兼容与 agent 合同

R8. 旧 v10 supervisor DB 自动迁移且保留原 wave/slot/redrive 数据；无 `supervisor-db` feature、空 `db_path` 或内存 store 路径仍工作，不新增配置项。

R9. Review 的 `SharedReadonly` 行为不变；它可以获得历史摘要，但不得被绑定到可写 Worktree。

R10. 更新 agent-facing wave skill，使收到 `# Recovery Context` 的 Worker 知道先检查 Git 与测试、仅补剩余工作、禁止破坏性清理；文档不得暴露内部 DB 路径或源码实现细节。

### 1.4 输入、输出与状态变化

- 输入：公开/store wave id、slot index、Worker cwd、Worker 最终分类结果、现有 redrive parent-child 映射和 slot resource。
- 输出：内部 `SlotAttemptReceipt` 列表；Worker prompt 的可读 `# Recovery Context`；原有 `WaveWorkerOutcome`、event 和 CLI 输出形状不变。
- 新状态：`running → succeeded|failed` 的 attempt receipt。进程崩溃时 `running` 可保留，恢复渲染为“interrupted/未观测到终态”，但不篡改原记录。
- 错误语义：未知 wave/slot、冲突 finish 是 store 合同错误；dispatcher 捕获 attempt 辅助写/读/探测错误并 fail-soft。原 supervisor 注册、descriptor、binding 和 terminal store 错误的既有 fail-closed 语义不变。
- 副作用：两次短 Git 只读探测；每个 attempt 最多一次 begin 和一次 finish store 写入；不写业务 events，不创建新 commit。

### 1.5 兼容、性能、安全与约束

- 兼容：不增加 CLI 参数、YAML 配置、公开 JSON 字段、event topic、required fields 或 preset 变更。默认 `slot_retry_budget=1` 和允许范围 `0..=2` 不变。
- 性能：每个 attempt 仅增加常数次本地 SQLite 操作和两次 Git checkpoint；Git 命令通过 `tokio::task::spawn_blocking` 离开异步调度线程。查询只读取当前/父 slot 的有界最近记录。
- 安全：不执行来自 receipt 的文本；不自动 checkout/reset/clean；存在未终态 running receipt 时不复用旧目录；不复用未被 Git 登记或分支不匹配的目录；prompt 不显示内部 supervisor DB 路径。
- 已知约束：正常 loop 终态会清理 Worktree，因此“跨进程复用”只承诺复用仍合法存在、且没有未终态 running receipt 的 Worktree。被清理或无法证明无旧写者的 Worktree 必须回退为新 Worktree，但历史 receipt 仍可复用。
- 已确认假设：`SupervisorStore` 只有 `InMemorySupervisorStore` 与 `RusqliteSupervisorStore` 两个生产实现；`ralph-cli` 默认 feature 已启用 `supervisor-db`；redrive boot dispatch 只在 `resume=true` 时运行。
- 待验证假设：无。实现若发现第三个 `SupervisorStore` 实现、非 redrive 的 Worker resume 派发入口或不同 Worktree 分支规范，触发 Unit 停止条件。

### 1.6 本次范围与非目标

本次范围：最小 attempt receipt、内存/rusqlite parity、dispatcher 接线、Git checkpoint、安全 Worktree 复用、恢复 prompt、agent skill 和完整回归。

非目标：

- 不引入 SQLx；不替换已有 rusqlite supervisor store。
- 不建设通用 workflow checkpoint、模型 response cache、patch cache、transactional outbox 或所有 ledger 的统一数据库。
- 不实现持久化 Worker lease、heartbeat、fencing token 或僵尸 Worker 提交隔离。
- 不改变 retry budget 的计费和跨重启继承规则；receipt 是证据账本，不是新的调度授权。
- 不在看到已有 commit、成功 receipt 或测试产物后自动判定 Unit 成功。
- 不改变 terminal cleanup，也不保留已正常清理的 Worktree。
- 不重构大型 dispatcher 或 `wave_supervisor.rs`；只做当前行为需要的局部变更。

## 2. 代码库现状与证据

### 2.1 当前实现入口

外部入口是 `ralph run`。`crates/ralph-cli/src/loop_runner/inner.rs` 打开 supervisor store、运行 active-wave recovery，并在 `resume=true` 时调用 redrive boot dispatch。dispatcher 拆分计划完成后，`dispatch_pending_redrive_waves`、`dispatch_redrive_child_wave` 和正常 supervisor execute 入口应位于职责命名的 `crates/ralph-cli/src/loop_runner/wave/dispatcher/dispatch.rs`；执行本计划前必须用 `rg` 核对实际符号位置。

正常 wave 调用链为：检测 wave → `execute_wave_via_supervisor_with_executor` 注册/获取 store wave → `SupervisorBridge::bind_slot` 创建 Worktree → 构造 `WorkerRequest` → `dispatch_wave_inner_with_release` 在 `JoinSet` task 内通过 `Semaphore` 获取 permit → 局部 attempt loop 调 `WaveWorkerExecutor::execute` → 只把最终 outcome 记录到 tracker/store。

数据边界是 `ralph-core::supervisor::SupervisorStore`。内存实现位于 `memory.rs`，持久实现位于 `rusqlite.rs`，schema 由 `migrations.rs` 和 `migrations/v*.sql` 管理。Worktree 的创建、枚举和删除位于 `worktree.rs`。Worker retry prompt 位于 `wave_prompt.rs`。Agent-facing wave 指南位于 `crates/ralph-core/data/ralph-tools-wave.md`。

现有测试框架为 Rust unit/integration tests + cargo-nextest。与本功能直接相邻的 runner 集成测试位于 `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs`；dispatcher 拆分后 retry 辅助测试应位于 `wave/dispatcher_tests/worker_lifecycle.rs` 或实际承担该行为的同名测试文件，store/migration/worktree/prompt tests 位于 `ralph-core` 对应模块。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | dispatcher 拆分前 `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs::dispatch_wave_inner_with_release`；拆分后计划目标为 `wave/dispatcher/worker_lifecycle.rs` | attempt loop 在一个 Tokio task 内；`attempt` 与 `prior_attempts` 都是局部变量；只让最终 outcome 逃逸 | U2 必须局部接入，不能改 tracker/TUI/RPC 终态边界 | 高 |
| E2 | `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs::executor_retry_uses_fresh_pid_same_cwd` | 既有验收固定“新 PID + 相同 cwd” | R4 的 characterization 必须持续通过 | 高 |
| E3 | `crates/ralph-core/src/wave_prompt.rs::{PriorAttempt,RetryContext,render_retry_context}` | 已有有界、非可信的 retry detail 和禁止 reset/clean/直接认成功规则 | U3 复用相同安全语言和 renderer 模式，不创建第二套冲突规则 | 高 |
| E4 | `crates/ralph-core/src/supervisor/mod.rs::SupervisorStore` | store 为同步 trait；注释明确 rusqlite 同步边界 | 新 receipt 必须扩展现有 trait，不新增 SQLx repository | 高 |
| E5 | `crates/ralph-core/src/supervisor/{memory.rs,rusqlite.rs}` | 仓库内只有两个 `SupervisorStore` 生产实现 | parity 范围已穷举；无需可选第三 adapter | 高 |
| E6 | `crates/ralph-core/src/supervisor/migrations.rs` | `CURRENT_VERSION=10`，迁移按 `user_version` 前向执行；CREATE TABLE migration 不需要 column probe | U1 新增 v11 table，并扩展现有 migration tests | 高 |
| E7 | `crates/ralph-core/src/supervisor/migrations/v6.sql` 与源码搜索 | v6 已有 slot `attempt_count/max_attempts`，但正常 runtime 没有递增/历史写路径 | 不能把计数列误当完整 receipt；需独立 append-like attempt 行 | 高 |
| E8 | `crates/ralph-core/src/supervisor/rusqlite.rs::open` | production store 使用 WAL、busy timeout、互斥 Connection，并处理并发 open | v11 延续该连接与事务策略，不引入新连接池 | 高 |
| E9 | `crates/ralph-cli/src/loop_runner/inner.rs` | `recover_active_waves_at_startup` 只做 phase/projection 恢复；只有 Pending redrive child 在 `resume=true` 时重新派发 | 本计划不能声称普通 active wave 自动重跑；跨进程 Worker 恢复锚定 redrive boot path | 高 |
| E10 | `crates/ralph-core/src/supervisor/{memory.rs,rusqlite.rs}::create_redrive_wave` | child 保存 parent wave、parent-slot 映射和 descriptor，但不保存父 slot resource/attempt history | U3 必须在线性计划中补 parent resource 与 history 查询 | 高 |
| E11 | `crates/ralph-cli/src/loop_runner/wave/supervisor_bridge.rs::bind_slot` | Exec/Fix 每次都调用 factory 创建以 child wave id 命名的新 Worktree；Review 返回 `None` | U3 的安全复用点是生产 bridge binding，不是 dispatcher 手工改 cwd | 高 |
| E12 | `crates/ralph-core/src/worktree.rs::{list_worktrees,create_worktree}` | 已有 Git porcelain 枚举和 Worktree/branch 结构；create 返回实际 `ralph/*` branch | 复用验证应扩展现有 worktree 模块并校验路径、主树和规范化分支 | 高 |
| E13 | `crates/ralph-cli/src/loop_runner/wave/supervisor_bridge.rs::finalize_terminal_cleanup` | 每个业务终态遍历所有 wave 并删除 slot Worktree；不存在路径按幂等成功处理 | 不改变 cleanup；旧路径不存在时必须安全新建 | 高 |
| E14 | `docs/solutions/developer-experience/redrive-descriptor-persist-and-boot-dispatch-2026-07-29.md` | redrive 已采用“descriptor 持久化 → resume boot scan → digest fail-close”的相邻模式 | U3 在该真实路径增加 context/reuse，不创建旁路 redrive runner | 中高 |
| E15 | Git commit `44027682` | 现有 retry context 是为“同一工作目录盘点后继续”引入，并明确已有 commit 非成功信号 | R7 是既有安全合同的跨重启延伸 | 高 |
| E16 | Git commit `756f9ffa` 与当前代码 | redrive boot recovery 已接线，但当前绑定仍按 child wave 新建 Worktree | 计划需区分“已有 redrive 派发”与“缺少 artifact reuse” | 高 |
| E17 | `crates/ralph-cli/Cargo.toml` | CLI 默认 feature 为 `supervisor-db`；`ralph-core` 单包默认 feature 为空 | 命令必须对 core migration tests显式加 `--features supervisor-db` | 高 |
| E18 | `crates/ralph-core/src/config/loop_config.rs::SupervisorConfig` | db path 已默认 `.ralph/supervisor.db`；retry budget `0..=2`、默认 1 | 不新增配置；receipt 无需 operator opt-in | 高 |
| E19 | `crates/ralph-core/data/ralph-tools-wave.md` | 已向 agent 解释同进程 retry 的盘点、验证、禁止破坏性清理 | prompt 行为变化要求同步此 skill，但不改 CLI 参数表 | 高 |
| E20 | 已执行 nextest 基线 | migration、retry prompt、same-cwd、redrive memory/rusqlite 和“不信任已有 commit”共 12 个测试通过 | 每个 Unit 可用这些测试作为回归锚点 | 高 |
| E21 | `AGENTS.md` 测试硬规则 | ralph-cli 禁止裸 `cargo test`；全量入口是 `./scripts/run-tests.sh`；污染 agent env 必须 scrub | Verification Contract 必须只列 nextest/脚本入口并含污染环境回归 | 高 |
| E22 | `crates/ralph-core/src/supervisor/bridge.rs::store` | bridge 已能向 dispatcher提供 `Arc<dyn SupervisorStore>`，默认 mock 返回 `None` | U2 不需扩大所有 bridge mock 的必实现方法 | 高 |
| E23 | dispatcher 拆分前 `wave/dispatcher.rs::ProductionExecutor`、`wave/worker.rs::run_wave_worker`；拆分后目标为 `wave/dispatcher/worker_lifecycle.rs` 与 `wave/worker.rs` | production `execute` await `run_wave_worker`；worker 在读取结果前通过 blocking wait 回收 child process | 已终态 receipt 可证明该次 Worker 已退出；未终态 running receipt 不能证明，应禁止复用旧 cwd | 高 |

### 2.3 受影响范围

| 范围 | 已确认位置 | 影响 |
|---|---|---|
| 生产 store 合同 | `crates/ralph-core/src/supervisor/mod.rs` | 新增 receipt/checkpoint/outcome 类型与 store 方法 |
| 内存 adapter | `crates/ralph-core/src/supervisor/memory.rs` | receipt 分配、finish、查询、redrive parent resource/history parity |
| SQLite adapter | `crates/ralph-core/src/supervisor/rusqlite.rs` | 事务分配、幂等 finish、查询、redrive parent source解析 |
| Migration | `crates/ralph-core/src/supervisor/migrations.rs`、计划新增 `crates/ralph-core/src/supervisor/migrations/v11.sql` | v10→v11 无损升级 |
| Git 边界 | `crates/ralph-core/src/worktree.rs` | checkpoint 只读探测与持久化 binding 合法性验证 |
| Prompt | `crates/ralph-core/src/wave_prompt.rs` | 新增有界 Recovery Context，不改变现有 Retry Context |
| Dispatcher | `crates/ralph-cli/src/loop_runner/wave/dispatcher/worker_lifecycle.rs`、`dispatch.rs`（以拆分后 `rg` 结果为准） | attempt begin/finish fail-soft；读取恢复历史 |
| Binding | `crates/ralph-cli/src/loop_runner/wave/supervisor_bridge.rs` | 合法旧 binding 复用，非法/缺失时走现有 factory |
| Runner 集成测试 | `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | true dispatcher/store/reopen/redrive 行为验收 |
| Agent skill | `crates/ralph-core/data/ralph-tools-wave.md` | Worker 收到恢复信号后的动作与停止条件 |
| 不受影响 | preset/schema、event policy、API/UI、Tokio concurrency types、terminal cleanup、CLI clap | 不得发生变更 |

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| KTD1 | 使用哪种持久化技术 | SQLx+新 DB；新 JSONL；扩展现有 rusqlite store | 扩展 `SupervisorStore` 与现有 rusqlite DB；不引入 SQLx。`(session-settled: user-approved — chosen over SQLx migration and a full workflow checkpoint system: the existing rusqlite supervisor already owns wave durability and the requested value is bounded retry reuse.)` | E4-E8,E17,E18 | SQLx 会形成第二套连接/迁移/错误语义；JSONL 不能复用现有事务与 parent-slot 关系 | 0.99 |
| KTD2 | receipt 如何建模 | 只递增 v6 计数；覆盖 slot 当前字段；独立 attempt 行 | 新 `slot_attempts` 表与内存 map；每次尝试一行，状态 `running/succeeded/failed` | E6,E7 | 计数无法恢复 Git/失败历史；覆盖字段丢历史 | 0.96 |
| KTD3 | attempt identity 与并发 | dispatcher 局部序号；UUID；store 原子序号 | store 在 `(wave_id,slot_index)` 内事务分配单调 `attempt_seq` | E4,E8,E20 | 局部序号重启归零；UUID 不利于排序和 prompt；事务序号满足 SQLite 单机并发 | 0.94 |
| KTD4 | receipt 字段 | 保存完整 model response/log；只保存失败字符串；最小 Git+分类 | 保存起止 HEAD、dirty 可用性、状态、稳定失败码、起止时间；dirty 由 `git status --porcelain --untracked-files=normal` 是否非空确定；时间由 store 在状态转换时写 Unix epoch毫秒，pre-epoch降为0；不保存模型响应和自由文本 | E1,E3,E7,E12,E15 | 完整响应扩大安全/容量范围；自由文本不是稳定决策依据；store-owned time避免caller伪造或adapter漂移 | 0.96 |
| KTD5 | 辅助持久化失败语义 | 阻断 Worker；静默忽略；warn 后继续 | begin/finish/list/Git probe 均 fail-soft 并带 wave/slot/阶段 warning；原 terminal store 写保持既有语义 | E1,E22 与用户确认范围 | 阻断会引入回归；静默无法诊断 | 0.92 |
| KTD6 | Git 探测如何进入 Tokio path | 同步阻塞 runtime；外部 hook；`spawn_blocking` 包装只读 Git helper | core 提供同步只读 helper，CLI 用 `tokio::task::spawn_blocking` 调用 | E1,E12 | 直接阻塞影响并发；hook 增加外部契约和失败面 | 0.90 |
| KTD7 | 跨重启如何呈现历史 | 替换现有 Retry Context；把 DB 行直接拼 prompt；独立 Recovery Context | 保留 Retry Context 作为本次 task 内重试；新增有界 Recovery Context 表达 durable receipt 和 reuse 结论 | E1,E3,E9,E15 | 替换会破坏同进程语义；直接拼接缺少统一安全边界 | 0.94 |
| KTD8 | redrive 是否复用旧 Worktree | 总是新建；创建 child 时复制 resource；只看路径存在；派发时解析并验证父 binding | 派发绑定时按现有 parent-slot 映射动态解析父 resource；若存在任何未终态 running receipt则直接新建；否则再校验 Git 登记、非主树、canonical path 和规范化分支，通过后把该 resource 绑定到 child | E10-E13,E16,E23 | 创建时复制会漏掉升级前已存在的 Pending child并扩大 redrive 事务；running receipt不能证明旧进程已退出；只看目录存在或无条件复用会并发写/信任 stale path | 0.98 |
| KTD9 | 正常终态 cleanup | 保留 failed Worktree；增加 cleanup lease；维持现状 | 维持 `finalize_terminal_cleanup`；只复用崩溃后仍合法存在的 Worktree | E13 | 保留会扩大磁盘泄漏和 branch 生命周期，超出最小 ROI | 0.97 |
| KTD10 | 已有成功/commit 是否短路 | 自动标 success；跳过 model；仅作证据 | 永不短路；新 Worker必须验证并发布自己的终态结果 | E3,E15,E20 与用户确认范围 | 无法证明旧结果已安全投影，自动信任会制造 silent success | 0.99 |
| KTD11 | 配置与公开接口 | 新开关/CLI inspect 字段；默认启用内部账本且不改公开表面 | 不新增配置、CLI、event/API 字段；store 存在即记录 | E17-E19 | 新开关增加组合回归；本行为是 supervisor durability 的内部延伸 | 0.95 |
| KTD12 | 文档同步 | 不更新；改 operator 文档；更新 injected wave skill 的恢复动作 | 更新 `ralph-tools-wave.md` 中 Worker 可执行的恢复规则；不加入 DB/源码内部信息 | E19,E21 | prompt 新语义必须被 agent理解；内部实现不属于 injected skill | 0.94 |

已确认事实、假设和决策已分别放在 §1.5、§2、§3。没有低于 0.85 的关键决策，因此无需计划前 spike，也没有后续 Unit 依赖未决事项。

### 3.1 高层技术设计与系统影响

以下是约束实现边界的流程图，不是生产代码：

```text
正常/同进程 retry
WorkerRequest.cwd
  → [spawn_blocking: Git start checkpoint]
  → SupervisorStore.begin_slot_attempt（失败仅 warning）
  → executor.execute（现有 Semaphore + JoinSet + retry budget）
  → classify_slot_attempt（现有唯一分类器）
  → [spawn_blocking: Git end checkpoint]
  → SupervisorStore.finish_slot_attempt（失败仅 warning）
  → 仅最终 attempt 进入 tracker / RPC / TUI / terminal store

redrive + resume
child wave/slot
  → 按 child_parent_slots/descriptor 解析 parent slot
  → 有界读取 parent receipts + parent SlotResource
  → receipt terminal gate + Git 校验 resource
       ├─ 无 running 且 Git合法：把同一 resource 绑定到 child，复用 cwd
       └─ running/非法/缺失：调用现有 factory 创建 child cwd
  → Recovery Context（只描述真实历史与复用结论）
  → 仍然执行当前 Worker并等待当前终态
```

系统影响被限制在 supervisor 持久状态、每次 attempt 的短 Git 只读探测、Worker prompt context 和 Exec/Fix binding。事件拓扑、公开 CLI、配置、API/UI、task projection、fan-in、terminal cleanup 与 Tokio 并发所有权不变。receipt 辅助失败向 tracing warning 传播，不进入业务 event；原注册、descriptor、binding 和 terminal store 错误继续沿既有 fail-closed 路径传播。

## 4. BDD 行为规格

```gherkin
Feature: supervisor slot 的尝试账本与恢复复用

  Background:
    Given wave 已在 SupervisorStore 注册
    And目标 slot 已获得 supervisor dispatch 授权

  Scenario S1: 一次成功尝试留下完整 receipt
    Given slot 的 Worktree 有可读取的 HEAD 和 dirty 状态
    When Worker 成功返回终态结果
    Then store 中存在一个从 running 完成到 succeeded 的 receipt
    And receipt 的起止 Git checkpoint 与实际探测一致
    And 原 slot terminal result 仍只记录一次

  Scenario S2: 可重试失败后成功记录两个 attempt 但只暴露最终结果
    Given slot_retry_budget 为 1
    And 第一个 Worker 返回 frozen retryable failure
    When 第二个 Worker 在相同 cwd 成功
    Then store 按序保存 failed 与 succeeded 两个 receipt
    And tracker、RPC、TUI 与 terminal store 只观察第二次结果

  Scenario S3: attempt receipt 写入失败不改变 Worker 结果
    Given store 的 attempt begin 或 finish 被故障注入拒绝
    When Worker 原本会成功
    Then Worker 仍成功并走原 terminal 记录路径
    And warning 指明 wave、slot 和失败阶段

  Scenario S4: 同一 slot 并发分配序号不重复
    Given 两个调用方并发开始同一 slot 的 attempt
    When store 分配 receipt
    Then 两个 attempt_seq 唯一且连续
    And DB 重开后的下一序号继续递增

  Scenario S5: v10 DB 自动升级
    Given 一个包含已有 wave、slot、resource 和 redrive 数据的 v10 DB
    When RusqliteSupervisorStore 打开该文件
    Then user_version 变为 11
    And 旧数据逐项保持
    And 新 attempt API 可写可读

  Scenario S6: 崩溃留下 running receipt
    Given attempt 已 begin 且进程在 finish 前退出
    When store 重开并读取该 slot 历史
    Then running receipt 保持不可变
    And Recovery Context 将它描述为未观测到终态
    And 系统不把它当 succeeded

  Scenario S7: redrive resume 复用仍合法的父 Worktree
    Given failed 父 slot 有 receipt 和持久化 Worktree binding
    And 父 slot 没有未终态 running receipt
    And该路径仍由 Git 登记且分支匹配
    And operator 已创建 Pending redrive child
    When `ralph run --resume` 派发 child slot
    Then child Worker cwd 等于父 Worktree 路径
    And prompt 包含父 attempt 历史及 worktree_reused=true 语义
    And Worker 仍必须运行验收并发布自己的结果

  Scenario S8: 父 Worktree 已清理时安全回退
    Given failed 父 slot 有 receipt 和持久化 binding
    But 该路径不存在、未被 Git 登记或分支不匹配
    When redrive child 在 resume 时绑定
    Then factory 创建 child 专属新 Worktree
    And prompt 明确旧文件未复用、历史只作参考
    And 不在主 workspace 执行 Exec/Fix Worker

  Scenario S9: Review redrive 保持 SharedReadonly
    Given Review child 能读取父 receipt
    When resume 派发 Review slot
    Then bind_slot 仍返回 None
    And 不创建或复用可写 Worktree

  Scenario S10: 历史成功或已有 commit 不短路
    Given父 receipt 状态为 succeeded 或结束 HEAD 与当前 HEAD 相同
    When child Worker 收到 Recovery Context
    Then dispatcher 仍调用 executor
    And 只有当前 Worker 的终态结果可完成 child slot

  Scenario S11: 无 supervisor-db 的内存路径保持可用
    Given运行时使用 InMemorySupervisorStore
    When slot 发生 retry
    Then receipt API 与 rusqlite 行为一致
    And 原 legacy/no-wave 路径不创建 SQLite 文件

  Scenario S12: receipt 查询失败时 redrive 仍可执行
    Given child binding 成功但 recovery history 查询失败
    When resume 派发 child Worker
    Then Worker 仍在安全绑定的 cwd 启动
    And prompt 不伪造历史
    And warning 可诊断查询失败

  Scenario S13: 未终态 attempt 禁止复用旧 Worktree
    Given 父 slot 最新历史包含 running receipt
    And 父 Worktree 路径仍由 Git 登记
    When redrive child 在 resume 时绑定
    Then factory 创建 child 专属新 Worktree
    And prompt 可以显示 interrupted 历史但不得声称旧文件已复用
```

## 5. 验收与测试策略

| Scenario | 验收条件与不变量 | 测试入口 | 推荐层级 | 风险补充测试 | E2E |
|---|---|---|---|---|---|
| S1 | 一行 succeeded；checkpoint 对应真实 temp Git；terminal result 一次 | 计划新增 `supervisor::attempt_tests` + dispatcher integration | store contract + 集成 | Round-trip | 否 |
| S2 | 两行有序；same cwd；仅最终 progress/outcome | `wave_supervisor.rs` 现有 retry executor seam | 集成 | Differential/characterization | 否 |
| S3 | executor success 不变；receipt 缺失；warning seam 被观测 | 扩展 `PersistFailingSupervisorStore` fault seam | 集成 | Fault Injection | 否 |
| S4 | 并发唯一序号；reopen 后续号 | rusqlite真实文件 + memory线程测试 | store contract | Concurrency | 否 |
| S5 | v10→v11、旧行保持、新表可用 | `migrations.rs` + rusqlite tests | migration integration | Compatibility | 否 |
| S6 | running 不被改写；renderer 不称成功 | store reopen + prompt unit | 集成/单元 | Crash-window | 否 |
| S7 | real Git worktree、rusqlite reopen、redrive child、resume dispatcher、同 cwd/有 prompt | `wave_supervisor.rs` | Outside-In 集成 | State-machine + Idempotency | 否；已覆盖真实 runtime seam，无 live model |
| S8/S13 | stale/非法/running source 都新建；绝不主目录 spawn | worktree helper unit + bridge/dispatcher integration | 单元+集成 | Security negative + zombie-writer guard | 否 |
| S9 | Review 无 writable binding | bridge integration | Characterization | Isolation | 否 |
| S10 | executor 调用计数为 1；current result 决定 terminal | injected executor integration | 集成 | Anti-silent-success | 否 |
| S11 | memory/rusqlite 对同一向量等价；no-wave 不产 DB | shared contract + existing lazy bridge tests | Differential | Feature-off | 否 |
| S12 | query fail 仍 spawn；无虚构 block | dispatcher fault seam | 集成 | Fault Injection | 否 |

最低成本原则：纯 receipt 状态机与 renderer 使用单元/contract tests；SQLite migration、重开、并发使用真实 rusqlite 集成；Worktree 复用使用临时 Git repo；关键恢复主路径使用注入 executor 的真实 dispatcher/store/bridge，不调用 live 模型，也不新增仅断言 prompt 文本存在的 preset 测试。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence | Unit |
|---|---|---|---|---|---|---|---|---|
| R1 | attempt start/finish receipt | S1,S2 | receipt count/status/checkpoint | attempt state | memory/rusqlite + dispatcher | 否 | E1,E4-E8 | U1,U2 |
| R2 | 并发与重开序号/幂等 | S4,S5 | unique/next/idempotent | memory concurrency | SQLite transaction/reopen | 否 | E6,E8 | U1 |
| R3 | fail-soft | S3,S12 | worker outcome不变 | helper error mapping | fault store dispatcher | 否 | E1,E22 | U2,U3 |
| R4 | 现有 retry 行为不变 | S2 | same pid/cwd/final-only | retry helper | wave supervisor | 否 | E1-E3,E20 | U2 |
| R5 | parent history 进入 child resume | S6,S7 | child prompt/history | recovery projection | redrive boot reopen | 否 | E9,E10,E14 | U3 |
| R6 | 安全 Worktree 复用/回退 | S7,S8,S13 | validated reuse/factory fallback | Git validator/receipt terminal gate | bridge+redrive | 否 | E11-E13,E23 | U3 |
| R7 | 不信任历史成功 | S10 | executor 必须调用 | prompt renderer | dispatcher integration | 否 | E3,E15,E20 | U3 |
| R8 | 旧 DB/feature-off | S5,S11 | migration/parity/no DB | migration tests | both adapters | 否 | E6,E17,E18 | U1 |
| R9 | Review 不变 | S9 | no writable binding | bridge | redrive review | 否 | E11 | U3 |
| R10 | agent skill 同步 | S7,S8,S10 | drift script +人工语义审阅 | 不做文本锁定 | embedded skill registry回归 | 否 | E19,E21 | U3 |

## 7. 严格串行开发单元

### Unit 1：持久化 Slot Attempt Receipt 合同（U1）

#### 1. Unit 目标

调用方可以在 memory 或 rusqlite store 中原子开始、幂等结束并有界读取一个 slot 的 attempt receipt；SQLite 重开和 v10 迁移后行为不变。

#### 2. 对应需求与 Scenario

- Requirements：R1、R2、R8。
- Scenarios：S1、S4、S5、S11 的 store 部分。
- Decisions：KTD1-KTD4、KTD11。
- Evidence：E4-E8、E17、E18、E20。

#### 3. 外部可观察结果

`SupervisorStore` 调用方能观察到：begin 返回唯一 `attempt_seq` 与 `running` receipt；finish 返回 `succeeded/failed` receipt；重读保持顺序；rusqlite 文件关闭重开后数据存在；旧 DB 自动升级。

#### 4. 当前行为基线

当前 v10 schema 没有 attempt history table，`SupervisorStore` 没有 begin/finish/list API。v6 的 `attempt_count/max_attempts` 不被正常 retry 路径写入。E6-E8 的 migration 基线测试已通过，必须先保留这些 characterization tests。

#### 5. 输入与输出

- 输入：wave id、slot index、`GitCheckpoint { head_sha: Option<String>, dirty: Option<bool> }`、terminal status、可选 frozen failure code。时间由 store 生成。
- 输出：`SlotAttemptReceipt`，含 `attempt_seq`、status、起止 checkpoint、stable failure code、started/finished time。
- 错误：unknown wave/slot；对不存在 attempt finish；running→不同 terminal 的冲突；存储错误。
- 状态：begin 新建 `running`；finish 只允许 `running→succeeded|failed`；相同 terminal payload 重放为幂等成功。
- 不变量：attempt_seq 从 1 开始并单调；failed 必须有稳定 failure code；succeeded 不得有 failure code；running 无 finish 字段。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `crates/ralph-core/src/supervisor/mod.rs` | supervisor types/store trait | 新增 receipt/checkpoint/status 类型和 3 个 store 方法；注册测试模块 | phase、dispatch、terminal API |
| `crates/ralph-core/src/supervisor/memory.rs` | 内存协议 adapter | 增加 attempt map 与同锁分配/finish/query | wave scheduling |
| `crates/ralph-core/src/supervisor/rusqlite.rs` | SQLite adapter | v11 table CRUD；begin 用单事务分配；finish first-terminal-wins | connection/WAL/open 策略 |
| `crates/ralph-core/src/supervisor/migrations.rs` | user_version migration | `CURRENT_VERSION=11`、注册 v11、required table/upgrade tests | v1-v10 内容 |
| 计划新增 `crates/ralph-core/src/supervisor/migrations/v11.sql` | 不存在 | 只创建 `slot_attempts` 与必要索引/FK | 不 ALTER 旧业务列 |
| 计划新增 `crates/ralph-core/src/supervisor/attempt_tests.rs` | 不存在 | 两个 adapter 的共享合同向量 | 不启动 CLI/runtime |

#### 7. 可依赖能力

现有 `SupervisorStoreError`、memory mutex、rusqlite `with_conn`、migration runner、v1 foreign key、tempfile 和 `supervisor-db` feature。

#### 8. 禁止依赖的未来能力

不得依赖 U2 的 dispatcher 写入、U3 的 Worktree validator/Recovery Context、未来 lease/fencing/model cache。不得预先修改 prompt 或 binding。

#### 9. 验收测试

- `attempt_contract_begin_finish_list_round_trips`：对 memory 和 rusqlite 运行同一向量；断言 running→failed/succeeded、字段和排序。
- `attempt_contract_concurrent_begin_allocates_unique_sequence`：两个线程同 slot begin；断言 `{1,2}`，无 duplicate。
- `attempt_contract_finish_is_idempotent_but_conflict_is_rejected`：相同 finish 两次成功，改 status/code 被拒绝且原行不变。
- `rusqlite_attempt_receipts_survive_reopen`：关闭/重开后读取，下一 begin 得到前序+1。
- `migration_v10_to_v11_preserves_existing_rows`：构造 v10 fixture，写 wave/slot/resource/redrive数据；打开 v11 后逐项断言。
- 运行：`cargo nextest run -p ralph-core --features supervisor-db --lib -- slot_attempt`；migration 单独用 `cargo nextest run -p ralph-core --features supervisor-db --lib -- migration_v10_to_v11`。

#### 10. Acceptance Red

先增加 `migration_v10_to_v11_preserves_existing_rows` 对 `CURRENT_VERSION==11` 和 `slot_attempts` 的断言。当前实际失败应是 version 仍为 10或 table 不存在。再增加 shared contract；当前实际失败应是目标 types/methods 不存在的编译错误。该编译错误是新合同缺失的有效 Red。

无效 Red：未启用 `supervisor-db` 导致 tests 未编译、SQL fixture 本身无 wave/slot 外键、测试命令命中 0 个、nextest 不可用或 Rust 语法错误。

#### 11. 单元测试拆分

- status 校验：running/succeeded/failed 字段组合。
- sequence 分配：单线程连续与双线程并发。
- finish transition：未知、幂等、冲突。
- bounded list：limit=0 返回空；limit=N 返回最新 N 条但按 attempt_seq 升序。
- adapter parity：不得 Mock SQLite；memory 与真实 rusqlite 对相同输入输出相同。

#### 12. Red → Green → Refactor 顺序

`v10 migration Red → 注册最小 v11 table/version → migration Green → memory begin/list Red → 最小 map/sequence Green → memory finish/冲突 Red → 最小状态机 Green → rusqlite round-trip/reopen Red → 最小事务 CRUD Green → concurrency Red → BEGIN IMMEDIATE/等价原子分配 Green → shared parity Green → Refactor 公共校验以消除 adapter 漂移`。

#### 13. 最小实现范围

必须实现 types、三个 store 方法、两个 adapter、v11 schema、旧 DB upgrade 和共享合同。不得写 dispatcher、prompt、Worktree、配置、CLI、event 或 docs；不得启用/消费 v6 `attempt_count` 作为调度预算。

#### 14. 集成验证

真实联合 `migrations.rs + RusqliteSupervisorStore + SQLite bundled engine`。memory 只用于 differential。执行 U1 两条 targeted 命令，并重跑 E20 的三条 migration baseline tests；所有命令必须命中大于 0 个测试。

#### 15. 风险驱动测试

- Compatibility：v10 实例自动升级且旧行不变，因 schema migration 是最大数据风险。
- Concurrency：同 slot 并发 begin，因 SQLite max+1 若不在事务中会重复。
- Idempotency：finish crash/replay，因 dispatcher 可能在边界重放。
- Differential：memory/rusqlite parity，因测试 seam 与 production store 必须同义。

#### 16. 回归范围

- `supervisor::migrations`：确保 v1-v10 fresh/reopen 不回归。
- `supervisor::memory_protocol_tests`、`redrive_tests`：trait 扩展不得改变 wave/redrive。
- default no-feature build：`ralph-core` 不启用 rusqlite 时仍编译。
- build/lint/typecheck：trait object safety 与 feature gate。
- 不要求全量 E2E；U1 未接 runtime。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/supervisor/mod.rs` | 修改现有生产文件 | types/trait | E4,E5 |
| `crates/ralph-core/src/supervisor/memory.rs` | 修改现有生产文件 | memory parity | E5 |
| `crates/ralph-core/src/supervisor/rusqlite.rs` | 修改现有生产文件 | persistent adapter | E5,E8 |
| `crates/ralph-core/src/supervisor/migrations.rs` | 修改现有生产文件/测试 | v11 registration | E6 |
| `crates/ralph-core/src/supervisor/migrations/v11.sql` | 新增 migration | attempt table | E6,E7 |
| `crates/ralph-core/src/supervisor/attempt_tests.rs` | 新增测试 | shared contract | E5 |

#### 18. 完成标准

S1/S4/S5/S11 store 验收、unit、integration、migration 回归、default feature build、`just fmt-check`、`just lint` 全绿；无 skip/only/ignored、新断言未削弱；Evidence/Decision 未降级；没有 U2/U3 行为；可独立提交。

#### 19. 停止条件

发现第三个 store 实现、v10 schema 与 E6 冲突、无法在单事务分配序号、需要新 crate/配置、Red 非目标缺失、旧数据不能无损迁移、trait 扩展迫使公开 API 变化或决策置信度低于 0.85 时停止。记录新证据，重做影响与 KTD 后修订后续 Unit。

#### 20. 风险与注意事项

| 风险 | 触发条件 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| 并发重复序号 | max+1 不在写事务 | 双线程 test | 原子事务/锁 | 多进程 SQLite busy 由现有 retry 管理 |
| finish 覆盖历史 | update 无状态谓词 | conflict test | first-terminal-wins | 手工 DB 修改不在范围 |
| migration 丢旧行 | 重建旧表 | v10 fixture diff | 只 CREATE 新表 | 极旧损坏 DB 仍按现有 open error |
| feature gate 漂移 | core default 无 rusqlite | no-feature build | cfg 与现有 migration模块一致 | 无 |

### Unit 2：将现有 Tokio Retry 接入 Attempt 账本（U2）

#### 1. Unit 目标

每次 supervisor Worker 执行前后都会尽力写 receipt；写入或 Git 探测失败不影响 Worker；同进程 retry 的并发、cwd、budget、deadline 和 final-only 可见性完全不变。

#### 2. 对应需求与 Scenario

- Requirements：R1、R3、R4、R8。
- Scenarios：S1-S3、S11 的 runtime 部分。
- Decisions：KTD3-KTD6、KTD10、KTD11。
- Evidence：E1-E3、E12、E20-E22。

#### 3. 外部可观察结果

使用 store 的测试/诊断调用方可在 Worker 完成后看到每次 attempt receipt；Worker、TUI、RPC、tracker 和 event consumers 仍只看到既有最终结果。

#### 4. 当前行为基线

E20 已证明 `executor_retry_uses_fresh_pid_same_cwd`、`third_attempt_prompt_contains_both_prior_failures` 与 `timeout_retry_does_not_claim_existing_commit_success` 通过。U2 开始前先重跑并记录；若基线不绿不得修改。

#### 5. 输入与输出

- 输入：`WorkerRequest.cwd`、store wave id、slot index、classifier 的 stable reason、executor outcome。
- 输出：U1 receipt；原 `WaveWorkerOutcome` 不变。
- 错误：Git 命令失败、spawn_blocking join 失败、store begin/finish 失败均 warning+continue。
- 状态：每次 `executor.execute` 对应一个 begin；有返回 outcome 时对应 finish；abort 可留下 running。
- 不变量：permit 仍覆盖整个 attempt loop；retry decision 仍只读 frozen code；最终 outcome normalization 不变。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `crates/ralph-core/src/worktree.rs` | Git Worktree 工具 | 新增只读 `capture_git_checkpoint(path)`，复用现有 Git command/error 模式 | create/remove/sync/reuse cleanup |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher/worker_lifecycle.rs`、`dispatch.rs` | attempt loop | 在每次 execute 前后调用异步 wrapper；复用同一 classifier；warning fail-soft | Semaphore、JoinSet、budget、deadline、silent_request、final outcome |
| `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | supervisor runtime 集成 | 扩展 retry tests 断言 receipt；扩展 fault wrapper | 不拆文件、不改 preset |

#### 7. 可依赖能力

U1 的 receipt API；现有 `bridge.store()`；`WorkerRequest.cwd`；worker classifier；`tokio::task::spawn_blocking`；temp Git fixtures；现有 `PersistFailingSupervisorStore` delegation seam。

#### 8. 禁止依赖的未来能力

不得依赖 U3 的 parent history、Recovery Context、redrive parent source解析或 binding reuse。不得提前改 `WaveWorkerContext`。

#### 9. 验收测试

- `executor_retry_records_failed_then_succeeded_attempt_receipts`：budget=1，第一次 timeout/failed，第二次 success；断言两行、same cwd、terminal result一次。
- `single_success_records_one_succeeded_attempt_receipt`：无 retry，起止 Git状态有值。
- `attempt_persistence_failure_does_not_change_successful_worker_outcome`：begin fail 与 finish fail 两个子用例；断言 executor 被调用、outcome success、原 slot terminal record success。
- `git_checkpoint_failure_records_unavailable_without_failing_worker`：不存在/非 Git cwd；receipt checkpoint 为 None 或 begin 降级，Worker成功。
- 运行：`cargo nextest run -p ralph-cli --bin ralph -- attempt_receipt` 与现有三条 retry characterization filter。

#### 10. Acceptance Red

先扩展 `executor_retry_uses_fresh_pid_same_cwd` 或新增相邻集成测试，执行后应因 store receipt 数量为 0 而断言失败，同时 executor 调用和 existing outcome 仍成功。该 Red 证明目标逻辑已走到 retry loop。

无效 Red：factory 未建 Worktree、wave 未获 dispatch approval、test executor 没返回目标 frozen code、agent-context env 污染、测试超时或 filter 命中 0。

#### 11. 单元测试拆分

- Git checkpoint clean/dirty/HEAD：真实 temp Git，不 Mock git 输出。
- outcome→receipt terminal mapping：success、retryable failure、non-retryable failure、executor_reported_failure normalization。
- begin failure：Fake/fault store，仅 Mock attempt write；不得 Mock executor outcome。
- finish failure：同上。
- abort/running：store contract 已由 U1保证，U2 只验证未调用 finish 时不伪造 terminal。

#### 12. Red → Green → Refactor 顺序

`Git checkpoint clean Red → 最小 helper Green → dirty/unavailable Red/Green → single attempt receipt Acceptance Red → begin/finish 接线 Green → retry two receipts Red → loop 内逐 attempt 接线 Green → begin failure Red/Green → finish failure Red/Green → characterization regression → Refactor 将 fail-soft warning 和 spawn_blocking 包成局部 helper`。

#### 13. 最小实现范围

必须覆盖每次 execute 的 begin/finish、stable code、Git 起止与 fail-soft。必须在移动 `terminal_bridge` 进 release guard 前克隆 store handle。不得修改结果分类、重试集合、max attempts、timeout 公式、TUI/RPC channel 或 terminal projection。

#### 14. 集成验证

真实联合 production dispatcher、memory store、real Git Worktree 与 injected executor。SQLite receipt 已在 U1 证明；U2 至少再用 rusqlite bridge 运行一个 retry integration，确认 trait 接线不是 memory-only。执行污染环境回归：`RALPH_CURRENT_HAT=executor RALPH_EVENTS_FILE=/tmp/x.jsonl cargo nextest run -p ralph-cli --bin ralph -- attempt_receipt`。

#### 15. 风险驱动测试

- Characterization/Differential：receipt 接线前后 final outcome、prompt、cwd 相同。
- Fault Injection：begin/finish/Git失败不改变 Worker。
- Concurrency：至少两个不同 slot 并发执行，receipt 不串 slot；Tokio semaphore 既有测试仍绿。
- Anti-silent-success：已有 commit 不跳过 executor。

#### 16. 回归范围

- 直接：所有 `executor_*retry*`、`u5_s7/s8/s10/s12`、`third_attempt_prompt*`、`timeout_retry*`。
- 相邻：partial/aggregate deadline、worker heartbeat/startup grace、slot projection、terminal classifier。
- legacy/no supervisor：`enabled_false_uses_wave_tracker`、`u2_no_phantom_bridge_when_no_detected_wave`。
- build：ralph-core default/no DB，ralph-cli default DB。
- 全量门禁留到 U3，但 U2 close 前必须跑 ralph-cli targeted、fmt、lint、build。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/worktree.rs` | 修改现有生产文件/测试 | Git checkpoint | E12 |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher/worker_lifecycle.rs`、`dispatch.rs` | 修改拆分后的生产文件 | attempt lifecycle 接线 | E1,E22 |
| `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | 修改现有测试 | Outside-In/fault/runtime | E2,E20 |

#### 18. 完成标准

S1-S3 runtime 验收、U1合同、相关 retry/deadline/heartbeat/legacy 回归、build/fmt/lint 通过；0 skipped/削弱；终态观察次数不变；无 U3 行为；Evidence/KTD有效；可独立提交。

#### 19. 停止条件

若接线要求把 receipt 写失败升级成 Worker失败、需要修改 classifier/retry budget、Git helper必须阻塞 Tokio线程、无法保留 final-only notification、发现 receipt 会包含敏感 model output或并发测试显示 slot串台，停止并重新决策。

#### 20. 风险与注意事项

| 风险 | 触发条件 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| 阻塞 Tokio | 直接运行 git command | 并发/代码审查 | spawn_blocking | 本地 Git慢仍占 blocking pool |
| receipt 影响结果 | `?` 传播辅助错误 | fault test | 显式 warn+continue | warning丢失不影响业务 |
| classifier 漂移 | 重写 outcome mapping | 同一 outcome 双断言 | 只调用现有 classifier | 新 failure code需未来同步 |
| 重复通知 | 中间 attempt 使用真实 channel | 既有 final-only tests | 不动 silent_request | 无 |

### Unit 3：Redrive Resume 的历史注入与安全 Worktree 复用（U3）

#### 1. Unit 目标

Pending redrive child 在 `ralph run --resume` 派发时，能读取父 slot durable receipt；若父 Worktree 仍合法则复用，否则安全新建，并把真实结论以有界 Recovery Context 提供给 Worker。

#### 2. 对应需求与 Scenario

- Requirements：R3、R5-R10。
- Scenarios：S6-S10、S12、S13。
- Decisions：KTD5、KTD7-KTD12。
- Evidence：E3、E9-E16、E19-E23。

#### 3. 外部可观察结果

注入 executor 能观察 child Worker 的 cwd 和 prompt：合法父 Worktree 时 cwd 相同；不合法时 cwd 为 child 新 Worktree；两者都包含准确的历史/复用说明且 executor 必须运行。Review 不创建可写 Worktree。

#### 4. 当前行为基线

当前 redrive memory/rusqlite boot tests 通过，但它们只证明 descriptor 派发；production `bind_slot` 总是创建 child Worktree，prompt 只有本 task 内 Retry Context。`finalize_terminal_cleanup` 删除终态 Worktree。开始前重跑 E20 的 redrive tests 和 cleanup/Review characterization。

#### 5. 输入与输出

- 输入：child wave/slot、parent mapping、父 receipt、父/child `SlotResource`、Git worktree list、current binding。
- 输出：安全 `SlotBinding`；`RecoveryContext { worktree_reused, receipts }`；原 redrive outcome。
- 错误：history/source 查询 fail-soft；把合法旧 resource 绑定到 child 时沿现有 `bind_worktree` 错误语义；Git binding 不合法不是错误而是 fallback。
- 状态：派发时动态解析父 resource；验证通过后将等价 resource 绑定到 child，失败则写入 child新 resource；父 wave/receipt不变。升级前已经存在的 Pending child 走同一解析路径。
- 不变量：parent ledger不改；child idempotency triple不变；descriptor digest gate不变；Review共享只读；terminal cleanup不变。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `crates/ralph-core/src/supervisor/mod.rs` | types/store trait | 新增 recovery history 查询结果和“按 child 映射解析 parent receipt/source resource”的 store 方法 | redrive public result/CLI shape |
| `crates/ralph-core/src/supervisor/memory.rs` | redrive adapter | 按 child-parent 映射解析父 receipt/resource | parent terminal rows、create_redrive 事务 |
| `crates/ralph-core/src/supervisor/rusqlite.rs` | redrive persistence | 用既有 mapping/descriptor 表 JOIN 查询父 history/resource | redrive idempotency/digest/create事务 |
| `crates/ralph-core/src/worktree.rs` | Git Worktree真值 | 新增持久化 binding validator：canonical path、Git登记、非main、normalized branch | generic `--reuse-worktree` 流程 |
| `crates/ralph-core/src/wave_prompt.rs` | Worker prompt | 新增 bounded Recovery Context renderer；准确区分 reused/fallback/running/succeeded/failed | Retry Context owner规则 |
| `crates/ralph-cli/src/loop_runner/wave/supervisor_bridge.rs` | production binding | 先读取 existing resource并验证；合法则构造既有 env/cwd，非法走原 factory+bind | Review None、公开 wave id env防泄漏 |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher/dispatch.rs`、`worker_lifecycle.rs` | 修改拆分后的生产文件 | bind 后读取 recovery context、比较 source/current path、注入 base prompt；query fail-soft | boot gating/descriptor/final outcome |
| `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | runtime tests | 真实 Git+rusqlite reopen+redrive boot、fallback、Review、no-shortcut | 不大规模重构 |
| `crates/ralph-core/data/ralph-tools-wave.md` | agent-facing wave规则 | 更新收到 Recovery Context 后的动作、关键字段来源、fallback和停止条件 | 不暴露 DB路径/内部函数/计划编号 |

#### 7. 可依赖能力

U1 receipt query，U2真实 receipts；现有 `child_parent_slots`/descriptor映射、redrive boot dispatcher、`list_worktrees`、factory注入、`PriorAttempt`安全文案模式、drift脚本。

#### 8. 禁止依赖的未来能力

不得实现 lease/fencing、自动 resume 普通 active wave、强制保留终态 Worktree、模型缓存、自动测试跳过、CLI inspect新字段、preset/schema变化。

#### 9. 验收测试

- `redrive_resume_reuses_git_registered_parent_worktree_and_history`：真实 temp repo/Worktree、rusqlite parent receipt、failed parent、child create、store reopen、boot dispatch；断言 factory不被调用、cwd相同、Recovery Context准确、executor调用一次。
- `redrive_resume_falls_back_when_parent_worktree_missing`：删除父 Worktree但保留DB binding；断言child factory调用、cwd不同、prompt说未复用。
- `redrive_resume_falls_back_on_unregistered_or_branch_mismatch`：两个负向子例；不得在主 workspace启动。
- `redrive_recovery_running_receipt_forces_fresh_worktree_and_is_not_success`：crash-window running行；即使旧路径Git合法也调用factory；prompt标 interrupted且未复用；executor仍调用。
- `redrive_prior_succeeded_receipt_does_not_skip_executor`：父 succeeded receipt/commit；child仍靠当前结果完成。
- `review_redrive_remains_shared_readonly_with_history`：history可读但 bind None/factory 0。
- `recovery_history_query_failure_still_dispatches`：fault store list失败；prompt无伪造 block；executor运行。
- `redrive_boot_dispatch_is_idempotent_after_reopen`：第二次 scan不重复spawn；receipt和resource不重复。
- 文档：`bash scripts/check-cli-doc-drift.sh --strict`，并人工核对 skill 只写 agent下一步动作。

#### 10. Acceptance Red

先新增 `redrive_resume_reuses_git_registered_parent_worktree_and_history`。当前正确失败应为：child `bind_slot` 调 factory创建新 path，导致 cwd不等父 path；prompt也没有 Recovery Context。redrive descriptor仍应成功派发，证明测试已到目标 runtime seam。

无效 Red：parent phase不允许redrive、descriptor未持久化、digest冲突、resume=false、hat trigger缺失、Git fixture没登记worktree、环境污染或 executor未被调用。

#### 11. 单元测试拆分

- binding validator：合法 canonical path；symlink等价；主树拒绝；不存在拒绝；Git未登记拒绝；branch mismatch拒绝；stored bare branch与Git `ralph/*`规范化匹配。
- recovery query：normal slot读自身；child读parent mapped slot；limit/排序；parent无receipt为空；unknown mapping错误不伪造。
- recovery source：memory/rusqlite child每个slot解析正确parent resource；升级前已存在、未复制resource的Pending child也能解析；child已有新binding时不把它误当父source。
- renderer：running/succeeded/failed、checkpoint unavailable、reused true/false；bounded records；不把receipt detail当指令。
- bridge：valid existing不调用factory；invalid调用一次；Review永不调用。
- 不允许 Mock：真实 Git registration、rusqlite reopen、dispatcher executor调用。

#### 12. Red → Green → Refactor 顺序

`Git validator negative/positive Red → 最小validator Green → store parent history/resource resolution Red → memory Green → rusqlite Green/reopen/legacy-child → prompt renderer Red → Recovery Context Green → bridge valid reuse Red → existing binding fast path Green → invalid fallback Red/Green → dispatcher history injection Red/Green → running/succeeded no-shortcut Red/Green → Review/query-failure Red/Green → doc更新+drift → 全量 Refactor/Regression`。

#### 13. 最小实现范围

必须实现 parent history/resource resolution、安全 validator、binding fast path/fallback、Recovery Context和skill同步。history读取固定复用现有 `RETRY_MAX_PRIOR_ATTEMPTS` 上限并按序渲染；不得显示内部路径。不得修改 redrive create事务、CLI output、child id、attempt_epoch、retry budget、cleanup和业务event。

#### 14. 集成验证

必须真实联合 `RusqliteSupervisorStore`、migration v11、`create_redrive_wave`、store reopen、`dispatch_pending_redrive_waves`、production bridge、真实 Git Worktree和injected executor。可以 Fake backend进程，但不能 Mock binding validator、SQLite或redrive descriptor gate。运行 U3 targeted、污染环境 targeted、core worktree/prompt/store tests、skill drift，再跑全量。

#### 15. 风险驱动测试

- State-Machine：parent failed→child pending→dispatch→terminal，running receipt不被误判。
- Idempotency：重复 redrive/create/boot scan 不重复 Worker或resource。
- Security negative：stale path、主树、branch mismatch、Git未登记均不能复用。
- Fault Injection：history read失败仍安全dispatch。
- Compatibility：正常 cleanup 后 fallback；Review sharedreadonly；feature-off memory parity。
- Differential：合法 reuse 与 fresh fallback除了cwd/recovery说明外，最终 outcome相同。

#### 16. 回归范围

- 直接：`test_u4_redrive_boot_dispatch_in_memory_multi_slot`、`test_s3_rusqlite_backed_wave_supervisor_dispatch`、descriptor unavailable/conflict、boot skipped when not resume。
- Binding：Exec/Fix unique branch/cwd、same-loop different-wave distinct、bind failure fail-close、Review None、env不泄漏internal wave id。
- Worktree：create/list/remove/find reusable、symlink/canonical path、terminal cleanup。
- Retry：U2所有 receipt/retry/final-only/deadline/heartbeat tests。
- Store：U1 migration/parity/concurrency、redrive tests。
- Agent skill：embedded inventory/registry与 `scripts/check-cli-doc-drift.sh --strict`。
- 旧配置/数据：无新字段；v10 migration；supervisor disabled/no-wave不产DB。
- 构建目标：`ralph-core` default与`--features supervisor-db`、`ralph-cli` default、workspace。
- 最终必要全量：`./scripts/run-tests.sh`，若仅出现竞态/时序 flake再用 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh`；serial仍失败视为真实回归。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/supervisor/mod.rs` | 修改现有生产文件 | recovery query contract | E4,E10 |
| `crates/ralph-core/src/supervisor/memory.rs` | 修改现有生产文件/测试 | parent resource/history解析 parity | E5,E10 |
| `crates/ralph-core/src/supervisor/rusqlite.rs` | 修改现有生产文件/测试 | parent JOIN/query/reopen | E8,E10 |
| `crates/ralph-core/src/worktree.rs` | 修改现有生产文件/测试 | Git-truth validator | E12,E13 |
| `crates/ralph-core/src/wave_prompt.rs` | 修改现有生产文件/测试 | Recovery Context | E3,E15 |
| `crates/ralph-cli/src/loop_runner/wave/supervisor_bridge.rs` | 修改现有生产文件/测试 | binding reuse/fallback | E11-E13 |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher/dispatch.rs`、`worker_lifecycle.rs` | 修改拆分后的生产文件 | history injection | E1,E9,E22 |
| `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | 修改现有测试 | true runtime ATDD | E2,E20 |
| `crates/ralph-core/data/ralph-tools-wave.md` | 修改 agent skill | 恢复动作合同 | E19,E21 |

#### 18. 完成标准

S6-S10/S12/S13验收、U1/U2全部测试、redrive/binding/worktree/prompt/skill integration、全量 nextest+doctest、build/fmt/lint/typecheck通过；无skip/only/ignored/断言削弱；无未解释 snapshot/golden；没有未来能力；Evidence/Decision均≥0.85；实际diff仅限本 Unit §17 已列位置；Unit可独立提交。

#### 19. 停止条件

若旧Worktree无法用Git真值验证、复制resource破坏redrive事务/idempotency、Recovery Context需要公开DB路径、必须改变cleanup/CLI/event/preset、Review获得可写cwd、dispatcher必须自动信任旧成功、发现普通active wave自动重派的新调用方、测试Red未到目标seam、回归范围显著扩大或Unit失去原子性，停止并按“新证据→影响分析→候选比较→重新决策→置信度→修订计划”处理。

#### 20. 风险与注意事项

| 风险 | 触发条件 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| stale path劫持 | 只看目录存在 | security negatives | Git porcelain+canonical+branch+non-main | 恶意篡改Git metadata不在威胁模型 |
| branch表示漂移 | DB bare名、Git `ralph/*` | normalization test | 单一normalize helper | 历史非ralph branch回退fresh |
| prompt误导 | fallback仍称same cwd | renderer/integration | 明确worktree_reused分支 | Agent仍需遵守指令，靠测试门禁 |
| parent ledger被改 | recovery query误写parent | before/after diff | parent查询只读；仅bind child | 手工DB修改不在范围 |
| 正常终态无artifact | cleanup先删除 | cleanup+fallback test | 保留receipt、new worktree | 无法恢复未提交文件，这是明确约束 |
| query扩大 | 每次读全历史 | limit test | bounded latest records+索引 | 单slot总行仍小 |

## 8. Unit 串行依赖图

```text
U1 持久化 Attempt Receipt 合同
  ↓ U2 使用已验证的 begin/finish/list 与 memory/rusqlite parity
U2 Tokio Retry 接入账本
  ↓ U3 使用真实 runtime 产生的 receipt，而不是测试手工伪造所有历史
U3 Redrive Resume 历史注入与安全 Worktree 复用
```

- U2 不能先于 U1：否则 dispatcher 必须临时设计/Mock persistence，无法证明重开与并发。
- U3 不能先于 U2：否则只能测试手写 receipt，不能证明真实 retry→reopen→redrive 链路。
- U1 不得提前接 dispatcher；U2 不得提前复制 parent resource或改 prompt；U3 只消费已验证能力。
- 即使某些测试可独立编写，也不得并行实施，避免同时改 `SupervisorStore` 和 dispatcher 导致 Red 原因混杂。

## 9. 执行命令清单

| 时机 | 命令 | 验证目的 | 预期 | 失败后进入下一步 |
|---|---|---|---|---|
| 开始前 | `cargo nextest --version` | nextest pin | `0.9.140` | 否 |
| U1 Red/Green | `cargo nextest run -p ralph-core --features supervisor-db --lib -- slot_attempt` | receipt contract/parity/concurrency | Red因能力缺失，后Green且命中>0 | 否 |
| U1 migration | `cargo nextest run -p ralph-core --features supervisor-db --lib -- migration_v10_to_v11` | 旧DB升级 | Green且旧行保持 | 否 |
| U1 regression | `cargo nextest run -p ralph-core --features supervisor-db --lib -- migrations` | 全部migration | Green | 否 |
| U1 no-feature | `cargo build -p ralph-core --no-default-features` | feature-off编译 | Green | 否 |
| U2 Red/Green | `cargo nextest run -p ralph-cli --bin ralph -- attempt_receipt` | dispatcher接线/fault | Red后Green，命中>0 | 否 |
| U2 characterization | `cargo nextest run -p ralph-cli --bin ralph -E 'test(/executor_retry_uses_fresh_pid_same_cwd/) or test(/third_attempt_prompt_contains_both_prior_failures/) or test(/timeout_retry_does_not_claim_existing_commit_success/)'` | same-cwd/final-only/no-shortcut | 3/3 Green | 否 |
| U2污染环境 | `RALPH_CURRENT_HAT=executor RALPH_EVENTS_FILE=/tmp/x.jsonl cargo nextest run -p ralph-cli --bin ralph -- attempt_receipt` | human fixture env隔离 | Green | 否 |
| U3 core | `cargo nextest run -p ralph-core --features supervisor-db --lib -E 'test(/recovery_context/) or test(/registered_worktree_binding/) or test(/slot_attempt/) or test(/redrive/)'` | prompt/worktree/store | Green且命中>0 | 否 |
| U3 runtime | `cargo nextest run -p ralph-cli --bin ralph -E 'test(/redrive_resume/) or test(/redrive_boot/) or test(/attempt_receipt/)'` | reopen/redrive/binding | Green且命中>0 | 否 |
| U3相邻回归 | `cargo nextest run -p ralph-cli --bin ralph -E 'test(/wave_supervisor/) or test(/dispatcher_tests/)'` | wave dispatcher/supervisor | Green | 否 |
| 文档 drift | `bash scripts/check-cli-doc-drift.sh --strict` | agent skill与CLI contract | 退出0 | 否 |
| Format | `just fmt-check` | 格式 | Green | 否 |
| Lint | `just lint` | Clippy | Green | 否 |
| Typecheck | `just typecheck` | workspace check | Green | 否 |
| Build | `cargo build --workspace` | 所有构建目标 | Green | 否 |
| 最终全量 | `./scripts/run-tests.sh` | nextest两阶段+doctest | 全绿 | 否 |
| flake兜底，仅需要时 | `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | 区分竞态flake/真实失败 | 全绿；serial失败是真回归 | 否 |

本功能不新增跨服务 contract 或浏览器 UI，不需要 browser E2E。它的关键用户路径由真实 SQLite、Git Worktree、production bridge、redrive boot dispatcher 和 injected executor 联合验证，避免 live模型调用。

## 10. 最终质量门禁

- S1-S13 全部通过并可追踪到 R/U/Evidence；S13 是未终态复用保护场景。
- U1-U3 的 Acceptance Red 均记录了实际目标失败，且不是环境/fixture/filter错误。
- 所有 unit、integration、migration、concurrency、idempotency、fault injection、characterization tests通过。
- v10 compatibility、memory/rusqlite differential、feature-off/no-wave路径通过。
- 同进程 fresh PID+same cwd、final-only notification、budget/deadline/heartbeat不变。
- redrive合法Worktree复用与三类非法binding fallback通过；Review保持SharedReadonly。
- running/succeeded receipt均不触发自动success；当前executor必被调用。
- `bash scripts/check-cli-doc-drift.sh --strict`、`just fmt-check`、`just lint`、`just typecheck`、`cargo build --workspace`、`./scripts/run-tests.sh`通过。
- 没有新增失败/skip/only/ignored测试，没有削弱断言，没有无解释snapshot/golden变化。
- 没有新依赖、配置、CLI、event、preset/schema/API/UI变更。
- 没有未处理BLOCKED；所有关键Decision仍≥0.85；未验证内容与剩余风险已明确。
- 实际变更未超出每个Unit文件范围；死路/实验代码已删除；U1→U2→U3严格串行且各自可独立提交。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每个Unit指向真实入口、测试Red、最小实现和回归 |
| Executor 是否仍需做关键设计决策 | 否 | KTD1-KTD12冻结store/schema/failure/reuse/prompt策略 |
| 所有文件和接口是否有代码库证据 | 是 | 现有位置见E1-E23；新文件明确标“计划新增” |
| 所有关键决策置信度是否 ≥0.85 | 是 | 最低KTD6=0.90 |
| 是否存在未处理的低置信度假设 | 否 | §1.5无待验证假设；发现冲突触发停止 |
| 每个Unit是否只有一个可观察行为 | 是 | U1 store合同；U2 runtime receipt；U3 safe recovery dispatch |
| 每个Unit是否可以独立验证 | 是 | 各自targeted命令、DoD和独立提交边界 |
| 每个Unit是否有真实Red | 是 | schema/table、receipt count、redrive cwd/prompt三类目标Red |
| 每个Unit是否包含回归范围 | 是 | 各Unit §16 |
| 是否存在未来Unit依赖 | 否 | 每Unit §8禁止提前依赖；仅依赖已完成前置Unit |
| 是否存在泛化任务描述 | 否 | 所有行为、位置、错误、命令、断言具体化 |
| 所有Scenario是否可追踪到测试和Unit | 是 | §5、§6矩阵 |
| 所有关键决策是否有Evidence | 是 | KTD表逐项引用E-ID |
| 计划是否可以严格串行执行 | 是 | §8线性依赖 |

## Planning Contract

§0-§3 构成本计划的 Planning Contract。实现者必须以 KTD1-KTD12 为机制权威，以 §7 的 Unit 停止条件处理任何证据冲突。§2 的路径和符号是当前基线入口；若后续已合并的模块拆分计划改变物理文件位置，先通过 `rg` 重定位同一符号并更新 Evidence，不得把路径漂移当成授权重构。

## Implementation Units

§7 的 U1-U3 是唯一实施单元。不得把 schema、adapter、dispatcher、prompt 或文档拆成并行技术层工作流；每个 Unit 按其纵向行为闭环执行。

## Verification Contract

§5、§6、§9 与每个 Unit 的 §9-§16 构成 Verification Contract。所有 ralph-cli tests 必须使用 cargo-nextest；最终测试必须使用仓库两阶段脚本。新增测试必须证明命中目标逻辑，不能通过只检查源码文本、preset prompt 文案或 fixture 存在来替代 runtime 行为。

## Definition of Done

§10 是全局完成定义。单元完成还必须满足对应 Unit §18。任何 Unit 的 Red、integration、regression、build、lint、typecheck或Evidence更新未完成，均不得开始下一 Unit；任何 abandoned spike、临时schema、测试专用生产分支或未使用接口必须在声明完成前删除。

## Appendix

### A. 调查过但明确不采用的相邻机制

- `crates/ralph-core/src/event_loop/accepted_transition.rs` 的 JSONL durable outbox解决业务event原子提交，不适合存Worker attempt。
- `.ralph/ledger.jsonl` 与 task/memory ledgers 是不同语义边界，本计划不统一它们。
- `parallel_forge_resume.rs` 的 whole-loop Worktree reuse manifest提供Git provenance先例，但slot redrive已有自己的resource与parent映射，应在supervisor边界完成。
- heartbeat 当前是进程内 StartToClose/idle/startup grace。本计划不把它升级为持久lease。

### B. 剩余风险

- `kill -9` 发生在Worker完成与receipt finish之间时只留下running记录；恢复Worker必须在新 Worktree重新验证，不能精确知道旧进程是否完成。这是KTD8/KTD10的安全取舍。
- 正常终态cleanup会删除未提交artifact；receipt只能说明Git状态，不能恢复已删除文件。改变cleanup属于后续独立需求。
- rusqlite store仍是单机同步Connection+Mutex；本计划不承诺跨机器或分布式Worker。
