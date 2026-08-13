---
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
type: fix
date: 2026-08-13
---

# `task.resume` 残留缺陷收敛实施计划

本计划承接 `docs/plans/2026-08-10-001-fix-task-resume-runtime-routing-plan.md`，不改写该历史计划。上一计划已经在当前分支落地统一 resolver、部分生产入口迁移、retry key、去重和 TOCTOU 检查；本计划只收敛当前 HEAD 上仍然存在的实现缺口。若执行中发现旧计划尚有未完成项，按本计划的边界补齐，不再创建第三套恢复路由。

## Goal Capsule

- **目标**：使 `task.resume` 成为真正可验证的“定向恢复信号”：每个 runtime 生成的恢复事件都解析出唯一已注册目标，保留任务/触发上下文，经过现有 Recovery durable-acceptance 与 checked-delivery 边界；目标不明、目标冲突、目标不存在、无 recipient 或 durable commit 失败时均 fail-close。
- **调用方**：`EventLoop` 的拒收/超时/阶段违规/缺失事件恢复路径、可信 JSONL `EventReader`、`ralph run --continue` 的操作者，以及被恢复的 hat。
- **权威入口**：现有 `crates/ralph-core/src/event_loop/resume_routing.rs` 的 resolver/publisher；禁止新增平行的 task.resume 发布机制。
- **执行形态**：严格串行 U1 → U2 → U3 → U4 → U5。每个 Unit 完成自己的 Acceptance Red、Unit Red、Green、Refactor、集成、回归和关闭检查后才能进入下一 Unit。
- **停止条件**：目标解析来源改变、持久化边界与现有 trusted events 语义冲突、Red 不是目标能力缺失、需要新依赖/新配置/新 topic、或任一关键决策置信度低于 0.85 时停止并更新 Evidence/Decision，不得由 Executor 临时拍板。
- **尾部责任**：最后一个 Unit 必须完成 agent-facing recovery 文档同步、静态 ingress guard、CLI/core/Bdd/全量回归；不得留下“后续再补入口/测试/文档”的债务。

## 0. 计划状态

**READY**：所有实施关键决策均由当前源码、已有测试结构、现有运行时持久化 API 或已执行的历史验证支持，置信度均不低于 0.85。计划不新增配置、依赖、preset、schema、manifest、topic 或持久化文件。

- **代码库基线**：分支 `pittcat-dev`，HEAD `43af71ca`（2026-08-13）。
- **调查范围**：`ralph-proto` 的 `Event`/`EventBus`；`ralph-core` 的 `EventReader`、`EventLoop`、`resume_routing`、rejection/correction、TaskStore、accepted transition、trusted JSONL 持久化、`loop.resume`；`ralph-cli` 的 loop runner、history logger 和 `integration_resume`；agent-facing recovery 文档；旧计划、solutions 和相关 Git 提交。
- **输出位置解析**：仓库没有 `.compound-engineering/config.yaml` 或 `config.local.yaml`，因此按 ce-plan 默认值写入 `docs/plans/`；该目录已由现有计划实际使用。
- **已执行的调查/验证**：在当前 HEAD 重新执行 `git rev-parse HEAD`、`git status --short`、相关源码 `rg`/`sed`、文件规模核验、历史 `git log`/`git show --stat`。既有 targeted nextest 结果仍仅作为历史证据（`cargo nextest run -p ralph-core -- ingress_inventory_regression_storm_dispatch` 与 `cargo nextest run -p ralph-cli --test integration_resume`），本轮计划同步没有新增测试、build 或 lint 执行。
- **独立研究**：已按 ce-plan 要求启动 repo-research 与 learnings 两个只读子代理；它们的结果只作为辅助证据，主计划不依赖未经主线源码复核的结论。
- **尚未执行**：本计划涉及的 Red、实现、targeted 回归、BDD、build、clippy、doctest、`./scripts/run-tests.sh` 和 CLI 文档 drift 检查均由 Executor 串行执行。
- **阻塞项**：无。旧计划被视为历史已实施计划；本文件是其残留收敛计划，不与旧计划并行执行。

## Product Contract

## 1. 功能目标

### 1.1 业务目标、输入与调用方

`task.resume` 不是 `loop.resume` 的别名。`loop.resume` 表示整个 loop/process 的 `--continue` 启动；`task.resume` 表示把一个具体的拒收、缺失、超时或中断恢复责任交还给责任 hat。输入可能来自可信 JSONL 的 `triggered`、payload 的 `target_hat`、payload 的 `task_id`/`task_key`、当前 loop 的 TaskStore owner，以及 runtime 产生的 correction/recovery payload。

### 1.2 当前行为

1. `resume_routing::resolve_resume_target` 已能校验显式 target、payload target、TaskStore owner、注册表、retry key 和 pending 去重，但生产 wrapper `publish_targeted_resume_for_hat_in` 仍总是把 `target_hint` 填入 `event_target`、把 `payload_target_hat` 置空，且多数调用方传入 `None` 的 TaskStore/task identity，因此 resolver 的关键 fallback/冲突检查在生产路径没有被真正使用。
2. `parse_and_emit.rs` 的 isolated anonymous business 分支和 `completion_and_termination.rs` 的 phase violation 分支仍直接构造并 `bus.publish(Event::new("task.resume", ...))`；现有 inventory test 按“单行包含字符串”扫描，无法识别跨行构造。
3. 最新 `runtime_precheck_rejection_for_event` 会在 `work.done`/`stabilization.done` 的 handoff 证据不一致时进入 recovery dispatch，并可能生成第三类生产 `task.resume`；该入口必须纳入统一 ingress 验收，不能只清理前述两个旧 direct path。
4. `EventBus::publish` 对显式 target 直达，但调用方忽略返回的 recipient 列表；未知/竞态/未注册目标可能表现为“调用成功但没有 hat 被唤醒”。
5. runtime helper 只写内存 EventBus；已有 `persist_system_injected_jsonl_event` 能将 system-injected 事件写入 trusted events JSONL 并推进 reader cursor，但 targeted resume 尚未复用该边界。因此进程在 bus publish 后、下一次 activation 前崩溃时，普通 runtime resume 可能丢失。
6. `is_correction_enabled()` 生产默认返回 true，`initialize_resume()` 因此调用 `initialize_resume_with_context(..., ResumeContext::default())`，输出 `loop.resume`，但 context 的 loop id、closed task count、progress summary、last iteration、scratchpad headline 均为空/零。`ralph-cli` 的 `EventLogger` 仍以 `task.resume` 作为 resume 的默认历史 topic，且 `integration_resume` 的部分断言在 marker/file 不存在时不执行断言。
7. payload 有两个 builder：`build_task_resume_payload` 与 `enrich_task_resume_payload_full`，它们对 `reason`、`kind`、`target_hat`、`retry_key`、原始触发上下文和 allowed topics 的覆盖不一致；agent-facing 文档又同时出现“第二次阻塞”和“第三次失败”的不同描述。

### 1.3 目标行为与行为差异

- 任何 runtime 生成的 `task.resume` 都从一个统一的 EventLoop 发布边界出去，事件带 `target = Some(registered_hat)`，payload 同时保留可审计的 `target_hat` 和 recovery identity。
- JSONL `triggered`、payload `target_hat`、task owner 三者一致时，目标 hat 收到且只有目标 hat 收到；任意冲突、空目标、未注册目标、跨 loop/closed task、无 recipient 或 registry race 都不发布给任何 hat，并产生现有诊断类型可检索的 Block 结果。
- runtime 生成的恢复意图先通过现有 Recovery durable-acceptance/checked-delivery 边界，再进入内存 bus；commit 失败时不 publish。本计划不把“普通 runtime resume 进程重启后的自动重放”伪装成已确认契约；manifest/reuse resume 的既有重放路径不在本次修改范围。
- `--continue` 只生成 `loop.resume`，并把真实 loop history、TaskStore、`progress.md`、scratchpad 摘要放入一次性 `ResumeContext`；不再把空 context 当成有效恢复状态，也不把 loop resume 误记成 task resume。
- 相同 `(loop, target, task identity, retry key, payload)` 的 pending recovery 仍只入队一次；不同 retry attempt 不得被空 retry key 或模糊 payload 比较误吞；现有预算/终态语义保持不变。

### 1.4 本次范围与非目标

**范围**：统一 runtime task.resume ingress；修复 payload/TaskStore target resolution；recipient fail-close；Recovery durable acceptance；真实 `ResumeContext`；retry/payload/documentation contract；core/CLI/BDD/regression tests。

**非目标**：不改 `loop.resume` topic 名、不改普通业务事件订阅、不给 hats 增加 `task.resume` trigger、不修改 preset YAML/schema/manifest/index、不新增环境变量或配置字段、不新增 crate 依赖、不新增数据库/ledger 文件、不改变 `EventBus` 对普通事件的广播语义、不改变 correction 3-strike 或 existing terminal guards 的业务阈值。

### 1.5 输出、状态、副作用、错误与约束

- **输出**：目标 hat pending queue 中一条带 target 的 `task.resume`，以及现有 accepted-transition durable receipt（在 compiled production contract 路径）；或 Block diagnostic，不产生错误 hat 的 pending event。
- **状态变化**：target hat 被 `next_hat` 的 targeted fast path 选中；resume payload 进入该 hat 的 recovery directive/correction prompt；durable receipt 在 publish 前存在。
- **错误语义**：恢复目标缺失、冲突、未注册、owner 不可解析、recipient 不等于唯一 target、registry race 或写盘失败均 fail-close；不得 fallback 到 round-robin、`ralph` 或任意订阅者。
- **兼容性**：旧 JSONL 的可选字段继续可解析；旧 payload 只有 `target_hat` 时仍可路由；无 target 且无安全 task owner 时明确阻塞；`loop.resume` 仍是 continue 的唯一启动 topic。
- **性能**：已有 target 为 O(1) 注册表/Bus 路由；TaskStore 只按现有 `self.tasks_path()` 加载并按 current loop 查询；不扫描全历史、不调用网络服务。
- **安全/权限**：payload target 只在 registry 和可选 task owner 一致时生效；不可信 JSONL 不能用未注册 target 绕过 source/policy guard；system-injected 只表示编排器来源，不等于跳过 target 校验。
- **已确认假设**：`EventBus` target 直达、`next_hat` targeted 优先、`Task.owner_hat_id`、`EventReader` 可选 `triggered`、`LoopHistory::last_iteration`、`ProgressSnapshot::parse`、`persist_system_injected_jsonl_event` 和 `Disposition::Recovery` 均已存在。
- **待验证假设**：每个现有 runtime ingress 都能提供可审计的 retry key 和 target source；若某入口只有 `hat` 而没有 task identity，使用该入口已确认的 hat 作为 target，并把缺失 task identity 作为 payload 可选字段，不由 Executor 发明新的 owner 规则。验证动作属于 U1/U2 的 Red 前置检查，失败即停止并更新计划。

### 0.1 2026-08-13 仓库同步校准

- 最新 HEAD 新增 `crates/ralph-core/src/event_loop/worktree_handoff.rs`，并在 `parse_and_emit.rs` 增加 `runtime_precheck_rejection_for_event`。这条路径会在 `work.done`/`stabilization.done` 的 handoff 证据不一致时生成 precheck rejection，再由现有 recovery dispatch 注入 `task.resume`；Unit 1 的 ingress 清单必须把它作为真实生产入口覆盖。
- `event_processing.rs` 现在在构造 prompt 时记录 activation worktree baseline，`dispatch_and_handoff.rs` 在审计时按 activation 前后快照判定 scope violation。Unit 1 不应把这类 scope/precheck 事件误判为普通业务 fanout；Unit 5 的全量回归必须保留其 fail-closed 语义。
- 当前 `AcceptedTransition` 仍只对已有业务 transition 提供 outbox-before-publish；`publish_targeted_resume_for_hat*` 仍直接向 `EventBus` 发布并只写 block diagnostic。Unit 3 的“Recovery durable acceptance + checked delivery”是待实现边界，不能以现有 wrapper 测试通过替代验收。
- 当前 `StateLedger` 在 loop 初始化中始终挂载，但这不等于 `task.resume` 已进入 ledger；本计划不应把 StateLedger wiring 的存在描述成 recovery durable receipt 已完成。

## 规划执行契约

本计划的关键事实、决策、Unit 和验证命令必须以本仓库源码与可执行测试为准；执行中若证据冲突，按每个 Unit 的停止条件暂停，不由 Executor 临时扩大范围。

## 2. 代码库现状与证据

#### 2.1 当前实现入口、调用链与边界

可信 JSONL：`crates/ralph-core/src/event_reader.rs::Event` → `impl From<Event> for ralph_proto::Event` → `EventLoop::process_events_from_jsonl` / `parse_and_emit.rs::process_parse_result` → emit gate/accepted transition 或 recovery branch → `EventBus` → `next_hat` → prompt injection。

runtime 合成恢复：`event_processing.rs`、`parse_and_emit.rs`、`completion_and_termination.rs`、`state_recovery.rs`、`wave_scope.rs` 和 `drift/engine.rs` 的现有调用点 → 应统一进入 `resume_routing.rs` 的 EventLoop-owned publisher。普通 `task.resume` 本次使用现有 `Disposition::Recovery`/`AcceptedTransition` durable acceptance 和 checked delivery；`dispatch_and_handoff.rs::persist_system_injected_jsonl_event` 仍是可信 system-injected JSONL 的独立边界，不在本计划把普通 runtime resume 改成该格式或承诺进程重启重放。

continue：`ralph-cli/src/loop_runner/inner.rs` 调用 `EventLoop::initialize_resume`；`state_recovery.rs` 选择 `LOOP_RESUME`；`correction::ResumeContext` 被 `prompt_injection.rs::prepend_correction_and_resume` 消费一次；`LoopHistory`、TaskStore、`ProgressSnapshot` 和 scratchpad 是 context 的数据来源。

#### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `crates/ralph-proto/src/topics.rs`、`ralph-core/src/event_origin.rs` | `task.resume` 与 `loop.resume` 是不同恢复语义；二者均是编排控制入口。 | 不把 continue 修成 task.resume；runtime recovery 与 loop bootstrap 分开。 | 高 |
| E2 | `crates/ralph-proto/src/event.rs`、`event_bus.rs` | `Event` 已有 `target/source/system_injected`；explicit target 直达，未注册 target 不产生 recipient。 | 复用现有 target，不新增协议字段；publisher 必须检查 recipient。 | 高 |
| E3 | `crates/ralph-core/src/event_reader.rs::Event` 与 `From<Event>` | JSONL `triggered` 可选且已映射到 proto `Event.target`；旧记录缺字段可反序列化。 | 保留旧格式；accepted 重建不能丢 metadata。 | 高 |
| E4 | `crates/ralph-core/src/event_loop/parse_and_emit.rs` accepted rebuild | 已有 `jsonl_event_to_proto` 保留 metadata，但另有直接 recovery 构造绕过该路径。 | U1 同时修直接 ingress 和 accepted path，不只修 parser。 | 高 |
| E5 | `crates/ralph-core/src/event_loop/resume_routing.rs::resolve_resume_target` | 已有显式 target/payload target/owner fallback、registry、retry key、dedup、TOCTOU 类型，但生产 wrapper 将 payload target 固定为空且多数调用不传 store。 | U2 改生产调用边界，不能只增加孤立 resolver 单测。 | 高 |
| E6 | `crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs` | 现有 inventory 逐行查找 `Event::new("task.resume"`；此前 targeted test 通过，但跨行 direct construct 可漏检。 | 用真实 runtime 行为测试为主，inventory 改为 token/结构化范围 guard，不能作为唯一证明。 | 高 |
| E7 | `crates/ralph-proto/src/event_bus.rs::EventBus::publish` | publish 返回 recipient 列表，现有 targeted wrapper忽略结果。 | 增加 `recipient == [target]` 的 fail-close 断言及测试。 | 高 |
| E8 | `crates/ralph-core/src/event_loop/dispatch_and_handoff.rs::persist_system_injected_jsonl_event` | 已存在追加 trusted JSONL、flush、推进 reader position 的 runtime 边界。 | 这是潜在重放方案证据，但普通 runtime resume 是否必须使用它不在当前产品契约中确认。 | 高 |
| E9 | `crates/ralph-core/src/event_loop/disposition.rs::publish_synthetic` 与 `classify` | `task.resume` 属于 Recovery；有统一 durable outbox + publish API，但 outbox entry 只保存 digest，不保存 target/payload。 | 本计划选择已有 Recovery durable acceptance；不把仅含 digest 的 outbox描述成完整事件重放。 | 高 |
| E10 | `crates/ralph-core/src/correction/mod.rs::ResumeContext`、`state_recovery.rs` | ResumeContext 字段已定义，但 production `initialize_resume` 传 `Default`；prompt 只消费一次。 | U4 只补真实 context 构造和断言，不新增 context schema。 | 高 |
| E11 | `ralph-cli/src/loop_runner/inner.rs`、`event_logger.rs`、`tests/integration_resume.rs` | history logger 的 `default_start_topic` 仍为 `task.resume`；CLI 测试存在条件分支，可能无断言即通过。 | U4 更新 history/CLI characterization，确保真实 `loop.resume` 可见且不记录 task.resume bootstrap。 | 高 |
| E12 | `ralph-core/src/loop_history.rs`、`task_store.rs`、`step_handoff/progress_task_gate.rs`、`event_loop/terminal_routing.rs` | 已有 last iteration、TaskStore loading、ProgressSnapshot parse、scratchpad path。 | U4 复用现有来源，不另建状态缓存。 | 高 |
| E13 | `ralph-core/data/ralph-tools*.md` | recovery 文档要求 target/context/allowed topics，但 retry 文案同时出现第二次阻塞与第三次失败。 | U5 对齐 agent-facing 文案到 runtime 的现有阈值；同步检查引用和 drift。 | 高 |
| E14 | Git commits `e3234b5b`、`3be29581`、`dc4f5ff3`、`afdca021` | 统一 resolver、retry/TOCTOU、deterministic correction/loop.resume 已先后落地，当前问题是残留闭环而非重新设计。 | 复用当前接口/语义，旧计划只作历史证据。 | 高 |
| E15 | `docs/solutions/architecture-patterns/orchestrator-expected-event-ledger-ssot.md` 与 recovery alignment solution | 项目既有原则是账本/运行时状态优先于 agent 自行推断，recovery 必须在 runtime 边界提供 backpressure。 | 不把 target/retry/continue 责任转移给 prompt 文案。 | 中高 |

#### 2.3 已确认受影响范围

- **生产模块**：`crates/ralph-core/src/event_loop/resume_routing.rs`、`parse_and_emit.rs`、`event_processing.rs`、`completion_and_termination.rs`、`dispatch_and_handoff.rs`、`state_recovery.rs`、`wave_scope.rs`、`drift/engine.rs`、`event_reader.rs`、`correction/mod.rs`、`step_handoff/progress_task_gate.rs`。
- **协议/基础设施**：`crates/ralph-proto/src/event_bus.rs`、`event.rs`；现有 `AcceptedTransition`/`Disposition` 只按需要验证，不改变普通业务协议。
- **任务/状态数据**：当前 loop 的 `tasks.jsonl`、`progress.md`、scratchpad、LoopHistory、trusted events JSONL；不新增数据文件。
- **CLI**：`crates/ralph-cli/src/loop_runner/inner.rs`、`entry.rs`、`event_logging.rs`；`crates/ralph-cli/tests/integration_resume.rs`。
- **Core 测试**：`crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs`、已有 EventLoop/context/accepted-transition 测试、BDD `crates/ralph-core/tests/scenarios.rs` 及其 `tests/scenarios/` fixture 目录。
- **Agent-facing 文档**：`crates/ralph-core/data/ralph-tools.md`、`ralph-tools-recovery-directives.md`；只改已确认存在的 recovery 规则，不写计划编号、内部函数名或 ledger 路径。
- **不受影响**：preset YAML/schema/manifest/index、普通业务 EventBus subscription fanout、外部网络服务、Web UI。当前没有证据表明这些调用方需要修改。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | 统一入口应扩展现有 helper 还是新增 task.resume topic/第二套 router？ | A：扩展 `resume_routing`；B：新增 topic/router；C：依赖 preset trigger。 | 选择 A：在现有 resolver/publisher 上增加 EventLoop-owned production boundary。 | E1、E5、E14 | B 破坏既有 topic；C 依赖订阅且无法保证 owner/target；当前已有 helper 是唯一合适 SSOT。 | 0.97 |
| D2 | target 应从哪里解析？ | A：只信 target_hint；B：显式 Event target → payload target → current-loop TaskStore owner，并做冲突校验；C：无 target 时广播/回退 ralph。 | 选择 B，冲突和未知均 fail-close。 | E2、E3、E5、E12 | A 正是当前 wrapper 绕过 payload/owner 的缺陷；C 会错误激活并违反 EventBus explicit-target 语义。 | 0.96 |
| D3 | runtime resume 的本次 durable 契约是什么？ | A：只在内存 bus；B：复用现有 `Disposition::Recovery` → `AcceptedTransition`，并用 checked delivery；C：把所有普通 resume 写入 trusted events JSONL 并承诺重启重放。 | 选择 B：compiled production path 先完成 durable acceptance 和 delivery preflight，再 publish；不在本计划决定普通 resume 的跨进程重放格式。 | E9、E14、E15 | A 绕过仓库已有 Recovery 规则；C 的产品契约和完整事件重放证据未确认，且 outbox entry 只含 digest，不能直接支持该承诺。 | 0.88 |
| D4 | recipient 是否需要检查？ | A：信任 `publish` 返回；B：要求返回列表恰为 resolved target；C：只查 registry。 | 选择 B，并把零/多 recipient 视为 Block。 | E2、E7 | C 不能发现 publish 时的 registry/queue 状态差异；A 无法证明“确实送达”。 | 0.94 |
| D5 | continue context 从哪里构造？ | A：继续 `Default`；B：让 CLI 传入所有字段；C：EventLoop 在 `initialize_resume` 通过 LoopHistory/TaskStore/progress/scratchpad 自己读取。 | 选择 C，保持 `initialize_resume` 单一入口并复用已存在的数据 API。 | E10、E11、E12 | A 已被当前 bug 证明无信息；B 把 runtime 状态收集泄漏到 CLI 并增加调用分叉；C 覆盖 CLI 和 core 调用方。 | 0.90 |
| D6 | retry/payload contract 是否新增 schema/config？ | A：新增字段/新 topic；B：统一已有 builder，保留现有字段并补 retry identity；C：只改文档。 | 选择 B；字段仍是现有 `reason/kind/target_hat/retry_key/original_trigger/allowed_topics`，只消除 builder drift。 | E5、E13、E14 | A 超出范围；C 无法修复 runtime payload；无证据需要新 schema。 | 0.93 |

所有关键决策均 ≥ 0.85；不存在需要进入实施前再次拍板的低置信度决策。若 D3 在 Red 阶段发现现有 AcceptedTransition 无法提供 success-before-publish 或 checked delivery，必须停止 U3，重新比较 B/C，并将计划 readiness 降为 BLOCKED；Executor 不得自行引入新 outbox 字段或重放格式。

### 3.1 Outside-In 技术边界

外部行为 → `ralph run --continue` / EventLoop runtime recovery → `initialize_resume` / recovery ingress → `resume_routing` target resolver/publisher → EventBus + Recovery durable acceptance → `next_hat`/prompt injection。每个 Unit 都完成一条纵向行为，不按 Model/Repository/Service 机械拆层。

## 4. BDD 行为规格

### Feature: `task.resume` 定向恢复与 `loop.resume` continue 分离

  Background:

  Given 当前 loop 注册 `executor` 与 `observer` 两个 hat，EventBus 支持 explicit target，TaskStore/LoopHistory/`progress.md`/scratchpad 可由测试 fixture 提供。

  Scenario: 可信 JSONL 的 triggered target 在 accepted 重建后仍定向到原 hat
    Given JSONL 有 `topic=task.resume`、`triggered=executor`、合法 payload，且 `observer` 也存在
    When EventReader 读取、EventLoop 接受并发布该事件
    Then pending queue 只有 `executor` 收到一条 `task.resume`
    And proto event 的 `target` 为 `executor`
    And accepted rebuild 不丢失 source、target、wave、system_injected metadata

  Scenario: payload-only target 与当前 loop task owner 一致时恢复 owner
    Given payload 只有 `target_hat=executor`、`task_id` 或 `task_key`，TaskStore 中存在当前 loop 的 open task 且 owner 为 `executor`
    When runtime 发布 task.resume
    Then `executor` 收到恢复事件
    And resolver 记录 target source 为 payload/owner 的实际来源

  Scenario: payload target 与 task owner 冲突时拒绝恢复
    Given payload target 为 `executor`，同一 loop open task owner 为 `observer`
    When runtime 尝试发布 task.resume
    Then 不存在任何 hat 的新增 pending resume
    And 返回 `TargetOwnerConflict` 类 Block 诊断

  Scenario: 目标缺失或未注册时不广播
    Given task.resume 没有可验证 target，也没有可解析的当前 loop owner，或 target 不在 registry
    When runtime 尝试发布 task.resume
    Then EventBus recipient 数为零
    And `executor`、`observer` 均不收到恢复事件
    And runtime 留下可检索 Block 结果

  Scenario: 发布返回非唯一 recipient 时 fail-close
    Given resolver 得到已注册 target，但 EventBus publish 返回空 recipient 或非 target recipient
    When runtime 发布 task.resume
    Then 不把该事件视为成功
    And 不向错误 hat继续投递

  Scenario: 相同恢复意图只进入 pending queue 一次
    Given 相同 loop、target、task identity、retry key、payload 的 resume 已 pending
    When runtime 重复发布该 resume
    Then 第二次返回 Duplicate
    And target queue 长度保持一条

  Scenario: 不同 retry attempt 不被空 key 或模糊匹配吞掉
    Given 同一任务同一 recovery kind 的 retry attempt 1 已 pending，attempt 2 payload/retry identity 不同
    When runtime 发布 attempt 2
    Then attempt 2 按现有 bounded retry 规则被独立判定，不被错误归类为 Duplicate

  Scenario: Recovery durable commit 失败时不产生内存假成功
    Given accepted-transition 的 StateLedger/outbox 目标不可写
    When runtime 尝试发布 targeted task.resume
    Then EventBus 不产生该恢复事件
    And 返回 durable commit 失败 Block/错误结果

  Scenario: checked delivery 在 durable commit 前拒绝未知 target
    Given resolver 给出未注册 target 或 EventBus 没有 recipient
    When runtime 尝试发布 targeted task.resume
    Then accepted-transition outbox 不新增 receipt
    And EventBus 不产生该恢复事件

  Scenario: 普通 runtime resume 的跨进程重放契约未被错误扩展
    Given当前运行路径只有内存 pending queue和已有 Recovery receipt
    When审查本计划范围
    Then不新增普通 resume 的 JSONL 重放格式或 outbox 字段
    And manifest/reuse resume 仍使用其已有独立恢复路径

  Scenario: `--continue` 只启动 loop.resume 并携带真实恢复上下文
    Given已有 LoopHistory、当前 loop tasks、progress.md 与 scratchpad
    When CLI 执行 `ralph run --continue`
    Then trusted/history 观察到 `loop.resume` 而不是 task.resume bootstrap
    And first prompt 的 `## LOOP RESUME CONTEXT` 包含 loop id、closed task count、progress summary、last iteration、scratchpad headline
    And 后续 prompt 不重复消费该 resume block

  Scenario: 旧 payload 缺少新可选字段仍按安全规则处理
    Given旧 JSONL/payload 只有 `triggered` 或只有 `target_hat`，没有 retry attempt/allowed topics 等新增可选内容
    When runtime 读取并路由
    Then 可验证 target 的旧事件仍正常定向恢复
    And 无法验证 target 的旧事件 fail-close，不猜测 hat

## 5. 验收与测试策略

| Scenario | 验收条件与副作用/不变量 | 测试入口与层级 | 风险补充测试 | E2E |
|---|---|---|---|---|
| S1/S2 | 断言 target、唯一 recipient、payload metadata、pending hat；普通 `plan.ready` fanout 不变。 | `ralph-core` EventLoop/resume routing tests；单元 + 集成。 | Characterization of current EventReader/accepted rebuild. | 否 |
| S3/S4/S5 | Block 不发布、不入任意 queue，diagnostic reason 稳定；recipient mismatch fail-close。 | `task_resume_runtime_routing.rs`；单元。 | registry race 与 unknown target existing tests 扩展。 | 否 |
| S6/S7 | 同一 identity 一条，不同 attempt 不误 dedup，existing retry budget/terminal behavior unchanged。 | `resume_routing.rs` tests + BDD scenario；单元 + workflow integration。 | Idempotency; no concurrency test unless file-lock experiment shows concurrent writer risk. | 否 |
| S8 | durable commit/recipient failure zero bus side effect、zero receipt；不扩展未确认的 ordinary restart replay contract。 | EventLoop fixture using temp StateLedger/outbox；集成。 | Fault injection for unwritable outbox and unknown target; no network dependency. | 否 |
| S9 | CLI run must create/resolve marker and assert actual `loop.resume`; context fields exact and one-shot. | `crates/ralph-cli/tests/integration_resume.rs`；CLI integration。 | Characterization of old continue behavior before changing assertions. | 关键路径仅 CLI integration，不启动真实外部 agent |
| S10 | old optional payload parses and safe target rule holds. | `event_reader`/resolver unit。 | Property-based not justified: schema is small and existing serde tests cover malformed input; add only explicit missing-field cases. | 否 |

测试层级选择依据：target resolution/payload conversion 是纯规则；EventReader→EventLoop→EventBus 是模块协作；CLI `--continue` 是唯一需要真实 binary 入口的关键路径。不得用 mock 替换真正的 EventBus target routing、Recovery durable boundary 或 prompt one-shot consumption。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | 所有 runtime ingress 统一定向发布 | S1/S4 | `runtime_generated_resume_is_targeted` | helper ingress decision tests | BDD workflow guard | 否 | E4/E5/E6 |
| R2 | target/payload/task owner 一致性与 fail-close | S2/S3/S4 | `payload_target_owner_conflict_blocks` | resolver matrix | EventLoop routing integration | 否 | E2/E5/E12 |
| R3 | recipient 必须唯一且正确 | S5 | `non_target_recipient_is_blocked` | EventBus boundary test | real EventLoop bus | 否 | E7 |
| R4 | runtime recovery 先 durable accept、再 checked publish，失败无假成功 | S8 | `targeted_resume_commit_precedes_publish` | persistence/delivery error test | AcceptedTransition recovery integration | 否 | E9/E14 |
| R5 | continue 使用 loop.resume 与真实 context | S9 | `continue_emits_loop_resume_with_context` | ResumeContext source test | CLI integration | 否 | E10/E11/E12 |
| R6 | dedup/retry/payload contract 不误吞、不漂移 | S6/S7/S10 | `resume_attempt_identity_is_stable` | builder/identity tests | BDD retry scenario | 否 | E5/E13/E14 |
| R7 | agent-facing recovery instructions 与 runtime 一致 | S6/S10 | 文档引用/命令 smoke | 不适用 | `scripts/check-cli-doc-drift.sh`（仅涉及 CLI 引用时） | 否 | E13 |

## Implementation Units

## 7. 严格串行开发单元

### Unit 1：消除生产直发并建立唯一 targeted runtime ingress

#### 1. Unit 目标

当任一 runtime recovery 分支（包括 handoff precheck rejection）产生 `task.resume` 时，只有统一 publisher 能创建/发布它；最终可观察结果是 target hat 收到带 `Event.target` 的恢复事件，错误 hat 不收到。

#### 2. 对应需求与 Scenario

R1、R3；S1、S4、S5；D1、D4；E4、E5、E6、E7。

#### 3. 外部可观察结果

isolated anonymous business 与 phase violation 两条当前 direct path 不再产生 `target=None` 的 task.resume；真实 EventBus pending queue 只出现正确 target；普通业务事件订阅 fanout保持原样。

#### 4. 当前行为基线

`parse_and_emit.rs` 约 605 行与 `completion_and_termination.rs` 约 971 行存在跨行 direct `Event::new("task.resume")` + `bus.publish`；现有 inventory test 因逐行扫描通过，不能证明没有 direct ingress。先增加真实行为 Characterization/acceptance test，记录当前 target 缺失或 recipient 不符合预期的失败。

#### 5. 输入与输出

- 输入：anonymous/phase recovery 或 handoff precheck recovery 触发的 source hat、reason、topic、loop id、标准 payload。
- 输出：`ResumeDecision::Allow` 后进入唯一 target pending queue，`event.target=Some(target)`。
- 错误：unknown target、recipient 非唯一、publisher Block；不得直接 `bus.publish`。
- 状态/副作用：保留已有 rejection digest/phase snapshot；只改变 task.resume 路由，不改变业务状态机。
- 不变量：普通 EventBus subscription、现有 plan.blocked/loop.stalled 分支不改变。

#### 6. 修改位置

- `crates/ralph-core/src/event_loop/resume_routing.rs`：当前 resolver/publisher SSOT；增加生产所需的带 target/recipient 结果的边界，不创建第二个 router。
- `crates/ralph-core/src/event_loop/parse_and_emit.rs`：替换 anonymous recovery direct publish；保留其 diagnostic event 与 accepted bookkeeping。
- `crates/ralph-core/src/event_loop/completion_and_termination.rs`：替换 phase violation direct task.resume；保留 phase budget/exhausted 分支。
- `crates/ralph-core/src/event_loop/worktree_handoff.rs`：只作为当前 handoff precheck 的生产入口与测试依据；除非 Red 证明其调用边界本身有缺陷，否则不改该文件。
- `crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs`：增加真实行为测试并改进 ingress guard；不把测试 fixture 的 bare Event 当生产 ingress。
- 明确不修改：`EventBus` 普通 publish 订阅算法、preset YAML/schema、业务 topic。

#### 7. 可依赖能力

现有 `ResumeDecision`、`ResumeBlockReason`、`EventBus::publish` 返回值、registry、`next_hat` targeted path、现有 `enrich_task_resume_payload_full`。

#### 8. 禁止依赖的未来能力

不提前实现 TaskStore payload fallback、trusted JSONL persistence、真实 ResumeContext、文档阈值改写；本 Unit 只保证生产 ingress 不再绕过 target boundary。

#### 9. 验收测试

- `runtime_generated_resume_is_targeted`：构造真实 EventLoop/registry，分别触发 anonymous、phase violation 和 handoff precheck recovery 分支，断言目标 queue 一条、非目标 queue 零条、event target 等于责任 hat。
- `production_ingress_inventory_has_no_direct_publish`：仅作为 architecture guard，必须识别跨行构造；不能替代前一测试。
- 运行：`cargo nextest run -p ralph-core -- ingress_inventory_regression_storm_dispatch`，再运行新增验收测试 `cargo nextest run -p ralph-core -- runtime_generated_resume_is_targeted`。

#### 10. Acceptance Red

先运行新增真实行为测试；当前 phase/anonymous 分支预期失败为“target 为 None 或目标 queue 为空/错误 recipient”。若 Red 只是编译错误、fixture 未加载、测试未进入 recovery branch、或现有 unrelated test 失败，均不是有效 Red，停止并修正测试入口。

#### 11. 单元测试拆分

1. direct recovery branch 通过 boundary 后 target 精确为 source/责任 hat。
2. `EventBus::publish` 返回空或错误 recipient 时 decision 为 Block 且无 pending 副作用。
3. 普通 `plan.ready` 仍按订阅 fanout 到两个 hat。
4. 测试不得 mock EventBus publish；只可 fake registry/diagnostic sink。

#### 12. Red → Green → Refactor 顺序

Acceptance Red → 为两个 direct branch 写最小 Unit Red → 实现统一 EventLoop-owned publisher 调用 → Unit Green → 增加 recipient fail-close Red/Green → 将两个生产调用点迁移 → refactor 重复调用参数/诊断格式 → 重跑 Unit 全部测试。

#### 13. 最小实现范围

必须移除两个生产 direct task.resume publish、保留诊断/phase budget、将 target 作为 Event metadata、检查唯一 recipient。不得实现 JSONL durable replay 或 retry contract 重写。

#### 14. 集成验证

真实模块：EventLoop registry、EventBus、`next_hat`；可 fake：诊断目录和输入 rejection。执行 targeted nextest；预期只有责任 hat pending。

#### 15. 风险驱动测试

Characterization（旧 direct path 确实无 target）；static ingress guard（防止回归）；registry race（已有 resolver test 扩展）。无 property/fuzz：本 Unit 不改变 parser。

#### 16. 回归范围

`ralph-core` resume routing、event-loop active hat/next_hat、EventBus subscription tests、phase violation/isolated scope tests；原因是改动同时触及 recovery branch 和 target queue，必须确认普通业务 fanout 不变。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/resume_routing.rs` | 修改现有生产文件 | 唯一 targeted ingress/recipient fail-close | E5/E7 |
| `crates/ralph-core/src/event_loop/parse_and_emit.rs` | 修改现有生产文件 | 移除 anonymous direct publish | E4 |
| `crates/ralph-core/src/event_loop/completion_and_termination.rs` | 修改现有生产文件 | 移除 phase violation direct publish | E6 |
| `crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs` | 新增/修改测试 | 真实行为与跨行 guard | E6 |

#### 18. 完成标准

Acceptance/Unit/集成/回归通过；build/clippy 通过；无 skip/削弱断言；两个 direct path 已消失；Evidence 更新；可独立提交；决策置信度不下降。

#### 19. 停止条件

实际 recovery branch 不经当前文件、recipient 返回语义与 E7 不符、或需要修改普通 EventBus；停止，记录新 Evidence，重做 D1/D4，不进入 U2。

#### 20. 风险与注意事项

风险是把 diagnostic `event.isolation.boundary_violation` 一并迁移导致观察语义变化；检测是相关测试的 topic/queue 断言；缓解是只迁移精确 `task.resume`，保留 diagnostic direct publish；剩余风险是 legacy test-only constructors 未启用 persistence，留给 U3 的 compatibility branch 验证。

### Unit 2：让 payload target 与当前 loop TaskStore 身份真正参与解析

#### 1. Unit 目标

当生产 resume 没有可靠显式 Event target 时，runtime 按 payload target 与当前 loop TaskStore owner 解析；一致则恢复，冲突/closed/cross-loop/duplicate key/unknown 则 fail-close。

#### 2. 对应需求与 Scenario

R2、R6；S2、S3、S4、S10；D2、D6；E3、E5、E12。

#### 3. 外部可观察结果

旧 JSONL payload-only target 和 task identity 可以恢复责任 hat；两个身份不一致时没有任何 hat 被唤醒；旧 payload 无可安全 target 时得到明确 Block 而不是广播。

#### 4. 当前行为基线

resolver 已有 owner/fallback tests，但 `publish_targeted_resume_for_hat_in` 固定 `payload_target_hat: None`；多个 production caller 将 `task_store=None`。先增加 production-shaped test，预期当前实现不能从 payload-only/TaskStore-only 输入得到 Allow。

#### 5. 输入与输出

输入为 `Event.target`、JSONL `triggered`、payload JSON optional `target_hat/task_id/task_key/retry_key`、current loop id、`self.tasks_path()`。

输出为统一 `ResumeDecision` 与标准 payload；Block 不 publish；task owner 查询只接受 open 且 loop 匹配的 task。

#### 6. 修改位置

- `resume_routing.rs`：新增已确认的 payload 解析/owned inputs 适配与 production publisher 入口；保留现有 resolver priority 和 reason enums。
- `parse_and_emit.rs`/`event_reader.rs`：只在 `task.resume` 边界把 payload target 提升为 routing input；普通 topic 不读取 payload target 作为 Event target。
- 所有已由 `rg` 确认的 production helper call sites：逐一传入当前 loop/task identity；不得对没有 identity 的入口伪造 task id。
- `task_store.rs`：仅在现有查询能力不足时扩展最小只读 helper；优先复用 `find_open_task_id_in_loop`，不改 TaskStore 持久化格式。
- 测试：`resume_routing.rs` 现有 resolver tests 与 `task_resume_runtime_routing.rs`。

#### 7. 可依赖能力

U1 的唯一 publisher、`self.tasks_path()`、`current_loop_id()`、TaskStore `load/find_open_task_id_in_loop`、registry。

#### 8. 禁止依赖的未来能力

不实现 trusted JSONL 写入/重启、不改变 continue context、不改 agent docs。

#### 9. 验收测试

- payload target + matching owner → Allow/target queue。
- target/owner conflict、closed task、wrong loop、duplicate task_key、unknown target → Block/no queue。
- legacy triggered-only JSONL → target preserved。
- 命令：`cargo nextest run -p ralph-core -- task_resume_runtime_routing`。

#### 10. Acceptance Red

先执行 payload-only 与 TaskStore-only tests；当前 wrapper 预期因 `payload_target_hat=None`/store None 而返回 MissingTarget 或错误 target。若测试当前已 Green，必须证明它走到 production wrapper 而不是直接调用 resolver；否则停止。

#### 11. 单元测试拆分

1. payload target JSON 类型错误/空字符串拒绝。
2. target 与 owner 一致 Allow。
3. target 与 owner 冲突 Block。
4. current loop filter 排除 closed/cross-loop。
5. 同 key 多 open task Block DuplicateTaskKey。
6. 不允许 mock TaskStore owner 结果；可用 temp TaskStore fixture。

#### 12. Red → Green → Refactor 顺序

payload parser Red → 最小 typed extraction Green → owner lookup Red → 接入 `self.tasks_path/current_loop_id` Green → conflict/legacy cases Red/Green → 统一两套 builder 输入 → Refactor callers，保持普通 topic 不解析 payload target。

#### 13. 最小实现范围

只增加 task.resume-specific target extraction、真实 TaskStore 传递、冲突检查和标准 identity；不扩展 schema、不新增 fallback hat、不扫描历史文件。

#### 14. 集成验证

真实 EventReader→EventLoop→TaskStore→EventBus；serde malformed payload 可 fake；执行 core targeted tests 和 BDD workflow scenario。

#### 15. 风险驱动测试

Characterization legacy payload；Idempotency identity matrix；State-machine-like invalid owner states。无需 concurrency/property/fuzz，除非 parser 证据显示 flexible payload 存在随机字节边界。

#### 16. 回归范围

TaskStore owner queries、event reader compatibility、active hat/next_hat、legacy loop-context tests；原因是 target source 与任务 loop scope共同变化。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/resume_routing.rs` | 修改现有生产文件 | payload/owner 实际接入 | E5/E12 |
| `crates/ralph-core/src/event_loop/parse_and_emit.rs` | 修改现有生产文件 | production-shaped inputs | E4/E5 |
| `crates/ralph-core/src/event_reader.rs` | 不修改生产代码；保留现有 `triggered` 映射 | legacy JSONL target metadata 已由现有转换保留 | E3 |
| `crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs` | 修改测试 | ATDD/owner/conflict matrix | E5 |

#### 18. 完成标准

所有 target-source scenarios、resolver tests、core integration、build/clippy 通过；旧 payload compatibility 保持；无猜测 fallback；独立提交可审阅。

#### 19. 停止条件

TaskStore 无法按 current loop 区分、payload target 与 Event target 语义冲突、或需要修改 TaskStore on-disk format；停止并重新决策，不进入 U3。

#### 20. 风险与注意事项

风险是将 payload `target_hat` 错误推广到普通业务 topic；检测是普通 EventReader tests 和 `plan.ready` target 不变断言；缓解是 extraction 只在 `task.resume` path。剩余风险是历史任务缺 owner，必须明确 Block，不能猜 hat。

### Unit 3：让 runtime targeted resume 经过 Recovery durable acceptance 与 checked delivery

#### 1. Unit 目标

runtime 在发布 targeted resume 前复用现有 `Disposition::Recovery`/`AcceptedTransition` durable acceptance，并用 checked delivery 验证目标；commit 成功才进入内存 bus。普通 runtime resume 的进程重启自动重放不属于本 Unit。

#### 2. 对应需求与 Scenario

R4；S8；D3；E9、E14。

#### 3. 外部可观察结果

在 publish 前验证可投递；compiled production contract 下 accepted-transition outbox 先有对应 Recovery receipt；delivery validation 或 durable commit 失败时 bus 没有假成功；target 不存在时不写 receipt。

#### 4. 当前行为基线

`resume_routing` 当前只 `bus.publish`；`task.resume` 已被 `Disposition::Recovery` 分类，但 runtime helper 未调用 `publish_synthetic`/AcceptedTransition。并且 `AcceptedTransition::commit_unlocked` 在写入 outbox 后仍调用 permissive `EventBus::publish`，没有消费 checked publish 的结果。先用 temp workspace 的 outbox/recipient test 建立 Red：当前 helper 不生成 Recovery receipt，或 durable publish 在 checked delivery 断言下无法证明唯一 recipient。

#### 5. 输入与输出

输入为 U1/U2 已解析的 Event、target、payload、loop id、activation id、execution contract digest、StateLedger。

输出为 accepted-transition durable receipt + immediate target queue；validate/commit 失败返回 error/Block，不产生 bus side effect。

#### 6. 修改位置

- `crates/ralph-core/src/event_loop/resume_routing.rs`：production publisher 调用现有 `disposition::publish_synthetic`，固定使用已存在的 `Disposition::Recovery`、current loop/activation/contract 参数和已解析的 targeted Event；直接 free function 仅保留 test/legacy compatibility，不新增第二个 durable publisher。
- `crates/ralph-core/src/event_loop/accepted_transition.rs`：在现有 `commit_unlocked` 的 durable outbox 写入之后使用 `EventBus::publish_checked`，把 delivery error 映射为现有 `TransitionError`；不新增 outbox schema 字段、不新增重放协议、不改 transition identity。
- `crates/ralph-core/src/event_loop/disposition.rs`：只复用现有 `classify`/`publish_synthetic`，不修改 topic classification 或其公开签名。
- `crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs`、`accepted_transition.rs`/`disposition.rs` 现有测试：temp workspace、commit-before-publish、unknown target/no recipient、checked recipient tests。
- `crates/ralph-core/src/event_loop/dispatch_and_handoff.rs`：本 Unit 不修改；其 trusted JSONL writer 只作为未决的后续重放方案证据，不纳入本次实现。

#### 7. 可依赖能力

U1 target/recipient boundary、U2 standard payload、existing `Disposition::Recovery`/`publish_synthetic`、`AcceptedTransition::commit_idempotent`、`EventBus::publish_checked`、StateLedger 和 execution contract。

#### 8. 禁止依赖的未来能力

不改变 `Disposition::Recovery` classification、accepted transition schema、trusted JSONL、loop.resume context、agent docs。

#### 9. 验收测试

- `targeted_resume_commit_precedes_publish`：读 accepted-transition outbox，断言 receipt 先于 target queue；同一 transition 重复提交不二次 publish。
- `targeted_resume_commit_failure_has_zero_bus_side_effect`：outbox/ledger 写失败，断言 no queue。
- `targeted_resume_unknown_target_has_no_receipt`：target validation failure 不写 receipt、不入 queue。
- 命令：`cargo nextest run -p ralph-core -- task_resume_runtime_routing accepted_transition disposition`。

#### 10. Acceptance Red

当前实现下第一项应失败为 accepted-transition outbox 中找不到 runtime-generated task.resume receipt；failure path 应失败为 bus 仍有 event。若测试只调用 `EventBus` fixture、没有经过 recovery publisher，或只检查 diagnostic history，不是有效 Red。

#### 11. 单元测试拆分

1. Recovery classification selects the existing durable transition path。
2. preflight validates target before outbox append。
3. outbox commit success precedes checked bus publish。
4. commit error prevents bus publish。
5. `publish_checked` validates the resolved target and the targeted queue contains only that target。
6. 不 mock EventBus/AcceptedTransition；只使用 temp workspace/StateLedger。

#### 12. Red → Green → Refactor 顺序

Recovery receipt Red → 接入 `Disposition::Recovery` Green → checked-delivery Red/Green → commit failure Red/Green → idempotent transition Red/Green → Refactor publisher/context 参数，重跑 AcceptedTransition/普通 Recovery characterization。

#### 13. 最小实现范围

只支持 targeted task.resume 的 existing Recovery durable receipt、preflight 和 publish 顺序；不得新建 recovery.jsonl/outbox 格式、不得修改普通 system-injected records 的字段语义、不得承诺普通 resume 的 restart replay。

#### 14. 集成验证

真实 EventLoop、AcceptedTransition、StateLedger、filesystem、EventBus；可 fake 只有受控 commit failure fixture。验证 commit-before-checked-publish，不构造第二进程重放测试，因为该产品契约尚未确认；由于 `AcceptedTransition` 是共享 durable boundary，必须同时运行既有 Business/Recovery transition tests。

#### 15. 风险驱动测试

Fault Injection（outbox/ledger 写失败、unknown target）；Idempotency（accepted transition 重复提交）；不加 restart/concurrency 测试，除非后续明确产品契约并重新完成 D3 决策。

#### 16. 回归范围

accepted-transition/disposition Recovery tests、EventBus checked delivery、StateLedger/outbox；不回归 trusted JSONL writer，因为本 Unit 不修改它。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/disposition.rs` | 不修改生产代码；复用现有接口 | Recovery classification/dispatcher is already present | E9 |
| `crates/ralph-core/src/event_loop/resume_routing.rs` | 修改现有生产文件 | route targeted resume through Recovery durable boundary | E5/E9 |
| `crates/ralph-core/src/event_loop/accepted_transition.rs` | 修改现有生产文件 | use checked publish after durable commit | E2/E7/E9 |
| `crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs` 与 `accepted_transition.rs`/`disposition.rs` tests | 新增/修改测试 | durable acceptance/fault acceptance | E9/E14 |

#### 18. 完成标准

durable commit 成功/失败、checked delivery、idempotent Recovery、existing accepted-transition regression、build/clippy/targeted integration 全通过；无新文件格式；可独立提交。

#### 19. 停止条件

无法用现有 `Disposition::Recovery`、`AcceptedTransition::commit_idempotent` 和 `EventBus::publish_checked` 形成 durable-before-publish；或需要新增 outbox 字段/重放协议；停止并将 D3 标为 BLOCKED。

#### 20. 风险与注意事项

风险是 durable receipt 成功后进程在 bus publish 前崩溃，现有 outbox 只保存 transition metadata 而非完整 Event replay；本计划不声称解决该跨进程重放问题。检测方式是把它列为剩余风险/后续决策，而不是写成已通过的验收；如果产品要求 ordinary resume crash recovery，必须另立调查和计划，不能在本 Unit 临时扩展 outbox。

### Unit 4：让 `--continue` 注入真实 `ResumeContext` 并修正 CLI 观测

#### 1. Unit 目标

`ralph run --continue` 启动 `loop.resume` 时，第一 prompt 获得真实且一次性的 loop context，history 记录不再把该启动误标成 `task.resume`。

#### 2. 对应需求与 Scenario

R5；S9；D5；E10、E11、E12。

#### 3. 外部可观察结果

CLI test 必须真的创建/解析 current-events marker，可信 event/history 中存在 `loop.resume`，不存在 task.resume bootstrap；prompt 中字段与 fixture 的 loop history/tasks/progress/scratchpad 精确对应，第二次 build 不重复 block。

#### 4. 当前行为基线

production correction path 调用 `ResumeContext::default()`；`inner.rs` history logger 用 `default_start_topic = task.resume`；integration test 有条件分支会在 marker/file 缺失时跳过断言。先新增 characterization 断言当前 topic/context 缺失，再实现。

#### 5. 输入与输出

输入：`LoopContext`、`LoopHistory::last_iteration`、current-loop TaskStore open/closed counts、`.ralph/agent/progress.md` 的 `ProgressSnapshot::parse`、scratchpad first meaningful heading、prompt。

输出：`ResumeContext::new(...)`、`loop.resume` bus event、first prompt context block；读取失败使用现有安全空值/诊断规则，不伪造进度。

#### 6. 修改位置

- `crates/ralph-core/src/event_loop/state_recovery.rs`：增加 EventLoop 内部 context builder，替换 `Default`；保留 `initialize_resume_with_context` 对显式测试 context 的 API。
- `crates/ralph-cli/src/loop_runner/inner.rs`：`default_start_topic` 改为 `loop.resume` 的实际 runtime topic；不改变 EventLogger trusted/history 文件分工。
- `crates/ralph-cli/tests/integration_resume.rs`：修复条件断言，确保 marker/path 不存在即失败而不是跳过；断言 `loop.resume`/无 task.resume bootstrap/context observable。
- `crates/ralph-core/src/loop_history.rs`、`step_handoff/progress_task_gate.rs`、`terminal_routing.rs`：只复用现有 API，不修改职责。
- `crates/ralph-core/src/event_loop/tests/loop_context.rs`：补 core one-shot context test；该文件已在当前仓库确认存在。

#### 7. 可依赖能力

U1–U3 已验证的 recovery separation，不依赖 targeted task.resume 才能构造 continue context；现有 `ResumeContext` renderer/one-shot consumer。

#### 8. 禁止依赖的未来能力

不改变 runtime task.resume payload、retry budget、preset instructions；不把 scratchpad 全文注入 ResumeContext，只取已有窄字段/标题。

#### 9. 验收测试

- `continue_emits_loop_resume_with_context`：CLI temp workspace fixture，断言实际 events/history file存在、topic、context fields、无 bootstrap task.resume。
- `resume_context_is_consumed_once_for_real_hat`：core prompt build twice，first contains block, second not。
- `continue_missing_history_or_progress_does_not_fabricate_values`：缺文件时断言安全空值/诊断，不填假值。
- 命令：`cargo nextest run -p ralph-cli --test integration_resume`；再运行 `cargo nextest run -p ralph-core -- resume_context_is_consumed_once_for_real_hat`。

#### 10. Acceptance Red

当前 CLI test 若强化为“marker/file 必须存在且包含 loop.resume”应因实际仍记录 task.resume/没有 context 而失败；当前条件分支的“测试通过但无断言”不能算 Red。若 binary 因 backend 提前退出而没有进入 initialize_resume，应修 fixture/入口而非改弱断言。

#### 11. 单元测试拆分

1. loop id source：current loop marker/LoopContext。
2. closed count：current loop TaskStore only。
3. progress summary：`ProgressSnapshot::parse` current step/completed narrow summary。
4. last iteration：LoopHistory existing result/None handling。
5. scratchpad headline：first stable heading, empty file safe。
6. prompt one-shot consumption；不 mock `prepend_correction_and_resume` 的真实 queue。

#### 12. Red → Green → Refactor 顺序

强化 CLI topic assertion Red → 修正 history topic Green → context source unit Red/Green → wire `initialize_resume` Red/Green → one-shot prompt test Red/Green → missing-file cases → Refactor context builder，确保 explicit `initialize_resume_with_context` tests不变。

#### 13. 最小实现范围

只替换 production default context 和 stale history label；复用已有 fields/API；不新增 ResumeContext 字段、不把 CLI 改成多套 resume path。

#### 14. 集成验证

真实 `ralph` binary、LoopContext marker、EventLogger/history、EventLoop prompt chain；backend 仍使用现有 test custom command，不调用外部 API。

#### 15. 风险驱动测试

Characterization（旧 conditional integration test）；compatibility（missing optional files）；no E2E/network because CLI integration exercises actual binary boundary。

#### 16. 回归范围

所有 `integration_resume`、core initialization/loop_context/prompt tests、EventLogger path tests、continue scratchpad validation；原因是修改启动 topic、history label 和 prompt context source。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/state_recovery.rs` | 修改现有生产文件 | default context 真实化 | E10/E12 |
| `crates/ralph-cli/src/loop_runner/inner.rs` | 修改现有生产文件 | history topic 与 runtime 一致 | E11 |
| `crates/ralph-cli/tests/integration_resume.rs` | 修改测试 | 消除 conditional no-op assertion | E11 |
| `crates/ralph-core/src/event_loop/tests/loop_context.rs` | 新增/修改测试 | one-shot/source coverage | E10/E12 |

#### 18. 完成标准

CLI/core acceptance、旧 scratchpad guard、prompt one-shot、build/clippy 通过；无 task.resume continue assertion；独立提交。

#### 19. 停止条件

实际 loop history path 与 LoopContext 不一致、`ProgressSnapshot` 无法安全读取、或 CLI 观察到的 topic 不是 EventLoop 实际 topic；停止并更新 D5。

#### 20. 风险与注意事项

风险是把 runtime current iteration 当 previous last iteration；检测是用 LoopHistory fixture 断言，不读取新 state 的默认 0；缓解是严格使用 `LoopHistory::last_iteration`。剩余风险是旧用户 workspace 缺 history，context 字段只能为空但 loop.resume 仍必须成立。

### Unit 5：统一 recovery payload/retry 契约、同步 agent skill 并完成回归门禁

#### 1. Unit 目标

同一类 task.resume 的所有生产路径生成一致的可执行 payload 和 retry identity，agent-facing 文档与 runtime 实际阈值一致，且全量 ingress/回归门禁能阻止新漂移。

#### 2. 对应需求与 Scenario

R6、R7；S6、S7、S10；D6；E5、E13、E14。

#### 3. 外部可观察结果

恢复 payload 的 `reason/kind/target_hat/retry_key` 与可选原始触发/allowed topics 结构一致；相同 intent 只重试一次，不同 attempt 独立；agent 收到的 recovery directive 不再要求与代码相反的次数。

#### 4. 当前行为基线

`build_task_resume_payload` 与 `enrich_task_resume_payload_full` 字段形状不同；recovery docs 同时存在第二次阻塞/第三次失败表述。先写 payload shape/retry attempt characterization，预期当前存在字段缺失或文档契约不一致。

#### 5. 输入与输出

输入为每个现有 recovery reason、target、original trigger、retry key、allowed topics、attempt count。

输出为统一 JSON payload；runtime 继续使用现有 bounded counters/terminal guard；文档明确“触发条件—agent action—字段来源—停止条件”。

#### 6. 修改位置

- `crates/ralph-core/src/event_loop/rejection.rs`：统一两个 builder 的共用字段/调用边界，保留已有 optional compatibility 字段。
- 已确认的 recovery call sites（`event_processing.rs`、`parse_and_emit.rs`、`completion_and_termination.rs`、`dispatch_and_handoff.rs`、`wave_scope.rs`、`drift/engine.rs`）：改为传递真实 retry identity，不复制另一套 builder。
- `crates/ralph-core/src/event_loop/resume_routing.rs`：dedup identity 使用 payload retry attempt/稳定 retry key；修正 pending projection 对空 retry key 的模糊匹配。
- `crates/ralph-core/data/ralph-tools.md`、`ralph-tools-recovery-directives.md`、必要的 `ralph-tools-emit.md`：按 agent-facing 可执行规则同步；不得写内部函数名、ledger 路径、计划编号或 preset 专属事故。
- `crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs`、rejection/correction tests：shape/identity tests。

#### 7. 可依赖能力

U1 target boundary、U2 source resolution、U3 durable metadata、U4 loop/continue distinction；existing rejection/correction counters。

#### 8. 禁止依赖的未来能力

不改 retry budget 常量、correction 3-strike 状态机、preset/schema；不把文档测试替代真实 runtime test。

#### 9. 验收测试

- `resume_payload_contract_is_consistent_across_builders`：required fields/optional fields/target/retry identity一致。
- `same_resume_identity_dedups_but_next_attempt_is_admitted`：queue and decision assertions。
- `recovery_directives_match_runtime_thresholds`：只测稳定 agent-facing contract fields，不锁死无关 prompt prose。
- 命令：`cargo nextest run -p ralph-core -- task_resume_runtime_routing`、相关 rejection/correction tests、`scripts/check-cli-doc-drift.sh`（仅若修改了源码行号/CLI 引用）。

#### 10. Acceptance Red

当前两个 builder 的字段集合/重试 identity 对比应失败；文档阈值 characterization 应指出 contradiction。若测试只比较完整 prompt 文本而不执行 payload/runtime，则不是有效 Red。

#### 11. 单元测试拆分

1. required fields always non-empty string。
2. target and retry key survive every known builder path。
3. original trigger preserved when supplied, omitted safely when absent。
4. exact duplicate vs next attempt identity。
5. correction/recovery target partition remains intact。
6. 文档只做命令/字段/停止条件 smoke，不用 byte-equality 锁定整篇 prompt。

#### 12. Red → Green → Refactor 顺序

builder shape Red → common contract Green → retry identity Red/Green → call-site migration → docs contradiction Red/Green → target partition regression → Refactor shared helper names and comments → targeted and full regression。

#### 13. 最小实现范围

只统一 recovery payload/retry identity和通用 agent skill；不增加字段 schema、不改变阈值、不新增 prompt mechanism。

#### 14. 集成验证

真实 rejection→correction→task.resume→prompt chain；可 fake rejection input but not prompt renderer/queue. BDD scenario exercises real EventLoop runner as required by repository rules。

#### 15. 风险驱动测试

Idempotency（exact duplicate/next attempt）；Characterization（legacy payload）；contract test（agent-facing command/field semantics）；不做 mutation/fuzz，当前风险是 shape drift 而非 arbitrary bytes。

#### 16. 回归范围

所有 task.resume runtime routing/rejection/correction tests、BDD scenarios、agent skill command smoke、旧 payload tests、`cargo clippy`/build/doctest/全量 nextest；原因是这是 shared contract。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/rejection.rs` | 修改现有生产文件 | single payload contract | E5/E13 |
| `crates/ralph-core/src/event_loop/resume_routing.rs` | 修改现有生产文件 | exact identity semantics | E5 |
| `crates/ralph-core/src/event_loop/{event_processing.rs,parse_and_emit.rs,completion_and_termination.rs,dispatch_and_handoff.rs,wave_scope.rs}` | 修改现有生产文件 | call sites carry actual identity | E5/E14 |
| `crates/ralph-core/data/ralph-tools.md` | 修改文档 | general task.resume action contract | E13 |
| `crates/ralph-core/data/ralph-tools-recovery-directives.md` | 修改文档 | threshold/field/stop conditions | E13 |
| `crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs` 与 rejection/correction tests | 新增/修改测试 | shape/idempotency regression | E5/E13 |

#### 18. 完成标准

payload/identity/BDD/docs smoke/relevant regression/build/clippy/doctest/full tests pass；无 skip、无断言削弱、无 doc drift、无未处理 BLOCKED；可独立提交。

#### 19. 停止条件

统一 builder 需要改变 preset schema/threshold、文档与 runtime 仍无法一致、或发现新 public caller；停止，记录证据并回改本 Unit 及追踪矩阵。

#### 20. 风险与注意事项

风险是把不同 recovery kind 强行压成同一 identity；检测是按 reason/kind/task_key/step 逐类断言；缓解是共用字段 contract、保留 kind-specific retry key。剩余风险是历史 third-party payload 的非 JSON 形状，只能安全 Block/兼容解析，不能猜测。

## 8. Unit 串行依赖图

```text
Unit 1
  ↓ 统一生产 ingress、target metadata、唯一 recipient 已验证
Unit 2
  ↓ payload/task identity 与当前 loop owner 已验证
Unit 3
  ↓ Recovery durable acceptance 与 checked delivery 已验证
Unit 4
  ↓ loop.resume 与真实 ResumeContext 已验证
Unit 5
```

U2 不能先于 U1，因为它必须把 payload/owner 解析接入唯一 production publisher；U3 不能先于 U2，因为 persistence record 必须携带已验证 target/identity；U4 不能先于 U3，以免 continue/history 测试把 runtime recovery durable record 混入；U5 最后执行，因为它要以所有真实 call-site 形状和最终阈值为依据统一文档/contract。任何 Unit 不得提前实现后续行为。

## Verification Contract

## 9. 执行命令清单

| 时机 | 命令 | 目的 | 预期 | 失败处理 |
|---|---|---|---|---|
| 每个 Unit Acceptance Red 后 | 使用该 Unit 第 9 节列出的具体命令；CLI Unit 4 使用 `cargo nextest run -p ralph-cli --test integration_resume` | 确认目标缺陷真实失败 | 只因目标能力缺失 Red | 非目标 Red 不得进入 Green |
| U1 | `cargo nextest run -p ralph-core -- ingress_inventory_regression_storm_dispatch` | 生产 ingress guard | 通过且无 direct task.resume | 失败停止 U1 |
| U2 | `cargo nextest run -p ralph-core -- task_resume_runtime_routing` | resolver/owner/conflict/dedup | 全通过 | 不得跳过 |
| U3 | `cargo nextest run -p ralph-core -- task_resume_runtime_routing`；`cargo nextest run -p ralph-core -- accepted_transition`；`cargo nextest run -p ralph-core -- disposition` | Recovery durable acceptance/checked delivery | 全通过 | I/O 或 delivery failure 停止 U3 |
| U4 | `cargo nextest run -p ralph-cli --test integration_resume` | 真实 CLI continue | 全通过且强断言执行 | 不得接受条件跳过 |
| U5 | `cargo nextest run -p ralph-core --test scenarios` | real EventLoop BDD | scenarios 全通过 | 不得改用 stub `run_scenario` |
| 文档引用变更后 | `scripts/check-cli-doc-drift.sh` | 检查 CLI 引用 drift | exit 0 | 立即同步文档 |
| 每个 Unit close | `cargo build` | build | exit 0 | 不进入下 Unit |
| 每个 Unit close | `cargo clippy` | lint | exit 0 | 修复后重跑 |
| U5 close | `cargo test --workspace --exclude ralph-e2e --doc` | doctest | exit 0 | 失败则修复/重跑；这是允许的 doctest 例外 |
| 最终 | `./scripts/run-tests.sh` | repository hard-rule 全量 nextest + phase 2 + doctest | exit 0 | 失败必须走 targeted/serial fallback 规则，不得宣称完成 |

除 `cargo test --doc` 外，不得使用裸 `cargo test`；尤其不得运行裸 `cargo test -p ralph-cli`。若全量 baseline 出现已知时序 flake，只能按 AGENTS 规定执行 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 作为最后兜底，并记录原因。

## Definition of Done

每个 Unit 必须满足：Scenario acceptance、Unit tests、真实集成测试、受影响回归、build、clippy、必要文档检查均通过；没有 skip/only、没有削弱断言、没有未来行为偷渡、Evidence/Decision 已更新且置信度 ≥ 0.85；Unit 可独立提交。

## 10. 最终质量门禁

- 所有 S1–S10 均有可执行测试并通过；R1–R7 追踪矩阵无空项。
- 所有 runtime task.resume ingress 均经统一 publisher；跨行/static guard 与真实 EventLoop 行为均通过。
- target、payload target、TaskStore owner、registry、recipient 和 Recovery durable acceptance 的不变量全部通过；未知/冲突/commit 失败无假成功。
- duplicate/next-attempt、legacy payload、correction target partition 均通过；manifest/reuse resume 的既有 replay 回归保持通过；ordinary runtime resume 的 restart replay 不在本计划验收范围，已列为剩余风险。
- `--continue` 的 CLI/core 测试证明 `loop.resume`、真实 ResumeContext、one-shot prompt、无条件跳过断言均成立。
- agent-facing 文档只描述 agent 可见动作，首次出现的术语已解释，字段来源/失败停止条件明确；无计划编号、内部 ledger 路径、内部函数名或 stale threshold。
- Build、clippy、doctest、`./scripts/run-tests.sh` 和受影响构建目标均通过；无新增失败/跳过、无无解释 snapshot/golden、无未处理 BLOCKED。
- 实际变更仅在本计划列出的 production/test/docs 范围；每个 Unit 按 U1→U5 串行完成。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每个 Unit 指向已确认入口、Red 原因、最小边界、命令和 DoD。 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D6 已固定入口、target 优先级、持久化边界、context 来源和 payload 策略。 |
| 所有文件和接口是否有代码库证据 | 是 | 现有路径均来自 E1–E15；新增内容明确标为“新增/修改”。 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1–D6 为 0.88–0.97；D3 已收窄为有直接代码证据的 Recovery durable acceptance，不把未确认的 restart replay 当决策。 |
| 是否存在未处理的低置信度假设 | 否 | 待验证项均绑定到 Red 前检查，且失败会阻塞，不交给 Executor 猜测。 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 ingress、U2 identity、U3 durability、U4 continue context、U5 contract/doc consistency。 |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 都有独立 acceptance、targeted nextest、集成、回归和停止条件。 |
| 每个 Unit 是否有真实 Red | 是 | 每个 Unit 明确当前实现导致的目标失败；环境/fixture/未执行不算 Red。 |
| 每个 Unit 是否包含回归范围 | 是 | 第 16 节逐 Unit 列出直接、相邻、兼容和构建回归。 |
| 是否存在未来 Unit 依赖 | 否 | 依赖图只允许前置 Unit 的已验证能力；每 Unit 明确禁止未来能力。 |
| 是否存在泛化任务描述 | 否 | 没有“完善逻辑/增加测试”式空泛任务，均列出对象、输入、断言和命令。 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | S1–S10 → R → 测试入口 → U1–U5 映射完整。 |
| 所有关键决策是否有 Evidence | 是 | D1–D6 均引用 E1–E15。 |
| 计划是否可以严格串行执行 | 是 | U1 → U2 → U3 → U4 → U5，无并行关系。 |

### 计划外验证与 review 状态

`ce-doc-review` 未在本轮执行：当前工具面没有可调用的跨 skill invocation 入口，且本轮未伪造 review 结果。计划已按 `ce-plan` markdown contract 做人工 post-write audit：frontmatter 无 lifecycle status、路径全部 repo-relative、稳定 unified-plan headings 存在、无 HTML/绝对路径/固定宽度换行。生成后可通过后续 review 选项显式触发 `ce-doc-review`。
