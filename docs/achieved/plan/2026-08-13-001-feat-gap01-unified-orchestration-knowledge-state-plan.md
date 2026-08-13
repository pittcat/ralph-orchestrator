---
type: feat
title: "建立统一、可回放的编排认知状态"
status: completed
date: 2026-08-13
origin: docs/brainstorms/2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
---

# GAP-01：统一、可回放的编排认知状态开发计划

## Goal Capsule

- 目标结果：编排器拥有一个由现有 `StateLedger` 持久化、可 replay、可计算新鲜度的认知状态子树；prompt 只读取它的压缩投影，不再成为跨 activation 的事实来源。
- 用户价值：当编排器重启、状态投影关闭、事件被拒收或证据与当前输入不一致时，系统能明确区分“已观察”“未验证”“过期”“未知”，而不是让 agent 把上一轮 prompt 或散落缓存当成当前事实。
- 主要约束：不改变业务事件的接受/拒绝结果、hat 路由、重试预算、终态、退出码、旧 `## ORCHESTRATOR CONTEXT` 字段语义；不依赖尚未 merge 的 `2026-08-12-001` worktree。
- 交付形态：本文件是实现级 dev plan，不包含生产代码；Coding Agent 必须按 Unit 1 → Unit 2 → Unit 3 → Unit 4 串行执行。

## Product Contract

### 0. 计划状态

- 状态：**已完成**。
- 当前代码库基线：分支 `pittcat-dev`，实现合并基线 HEAD `1585d922`；本次修复产生的工作区变更见 git diff。
- 调查范围：
  - `docs/brainstorms/2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md` 的 GAP-01 原始边界；
  - `StateLedger`、`LedgerSnapshot`、`CommitDelta` 的存储、回放与失败回滚；
  - `EventLoop` JSONL 接受后边界、事件 disposition、prompt 构建路径；
  - `RuntimeStateSnapshot`、`state_projection.enabled`、isolated/coordinator/custom/ralph prompt 分支；
  - 现有 state、event-loop、replay、scenario、CLI 文档测试入口；
  - 另一个 worktree 的 `2026-08-12-001` 计划与其未 merge 代码差异，仅用于兼容性边界。
- 已执行的验证命令：
  - `git status --short`
  - `git worktree list`
  - `git diff --stat a1dd6217..fc223418`
  - `rg` 搜索 state ledger、accepted event、prompt、测试与配置入口
  - `sed` 阅读上述符号的实现和测试
  - `wc -l` 检查受影响源码规模
  - `git log --oneline -- ...` 检查相邻实现历史
- 计划阶段未执行的验证：未运行 build、lint、nextest 或 E2E；`ce-plan` 阶段只做源码与配置调查，所有执行验证已明确放入 Unit 与最终门禁。不得以计划阶段未运行测试为理由跳过 Unit 的真实 Red 或最终回归。
- 实施后验证记录：已完成 U1→U4 的实现与修复；定向 `ralph-core` nextest 通过 168/168；最终 `./scripts/run-tests.sh` 通过 Phase 1 7693/7693、Phase 2 135/135、doctest 19/23（4 ignored）；`cargo build --workspace`、`cargo clippy -p ralph-core --all-targets --all-features -- -D warnings`、`scripts/check-cli-doc-drift.sh` 和 `git diff --check` 均通过。全仓 `cargo fmt --all -- --check` 仍受仓库既有非本计划格式漂移影响，未对无关文件做格式化扩散。
- 阻塞项：无。所有影响实现路径的决策均有直接代码/测试证据，置信度不低于 `0.85`。
- 重要基线说明：`2026-08-12-001-feat-run-diagnosis-trace-debug-enhancement-plan.md` 在另一个 worktree 的 HEAD `fc223418`，尚未 merge；本计划不引用其新增 Rust 类型，不要求其先 merge，也不修改其观察性 sidecar 合约。

### 1. 功能目标

#### 业务目标

解决 GAP-01：建立统一的编排认知状态，使系统能够保存并回放 claim、evidence、hypothesis、assumption、unknown、verified、falsified、decision、route reason 等语义类别，以及 producer、source ref、输入 fingerprint、freshness、verification status；prompt 只能是该状态的压缩读投影。

#### 用户或调用方

- `EventLoop`：在事件完成全部现有接受校验后，提交可回放的系统观察记录。
- `StateLedger` / `LedgerSnapshot`：作为跨 activation、重启和 resume 的唯一认知状态权威。
- isolated hat prompt：读取只读摘要，知道哪些记录是当前、过期、未验证或未知。
- 维护者与测试：通过 replay、故障回滚和关闭开关回归证明认知状态不会改变业务流程。

#### 当前行为

- `LedgerSnapshot` 已统一承载 task、progress、workflow、policy、state machine、recovery 等运行时状态，但没有认知记录子树。
- `RuntimeStateSnapshot` 只投影 plan/step/task/wave/git/fix/review 等运行时字段；它没有 claim/evidence/freshness/authority 语义。
- `LoopState` 仍有大量当前进程缓存；prompt 中的 `## ORCHESTRATOR CONTEXT` 只在 isolated custom-hat 路径注入。
- `state_projection.enabled=false` 时，现有 prompt 仍显示 disabled stub；该配置没有被业务接受路径关闭。
- 事件只有在经过 origin、policy、state-machine、projection、execution-contract 等接受链路后才进入 `accepted_log_events`；拒收候选不应被当成编排事实。

#### 目标行为

- 已接受且会推进业务或恢复流程的事件，在当前 batch 的接受边界之后生成一个不含原始 payload 的、带 digest/source/fingerprint 的观察记录。
- 记录进入现有 `StateLedger` 的 commit log，并在新进程 `StateLedger::new(workspace, true)` replay 后恢复。
- 同一个稳定 observation id 重复出现时幂等；快照内的展示记录有固定上限，不能无限增长 prompt。
- 缺少 fingerprint 或无法判断证据适用范围时显示 `unknown`；输入 fingerprint 不匹配时显示 `stale`；匹配时才显示 `current`。这里的 `current` 只表示“与调用方提供的比较 fingerprint 相同”，不等价于已证明当前 Git tree 正确；任何状态都不能仅凭 agent 叙述自动变成 `verified`。
- isolated prompt 在已有 `## ORCHESTRATOR CONTEXT` 之外追加一个仅在有记录时出现的认知摘要；旧区块、旧字段和空状态行为保持不变。
- 认知状态落盘失败只产生 warning 并把认知结果留在旧值/unknown；当前事件仍按原有业务路径继续处理。

#### 行为差异

变化的是“接受后多一份可回放的认知观察和可选 prompt 摘要”；不变化的是事件是否 accepted、是否 publish、hat 选择、route reason、recovery target、retry budget、timeout、termination、exit code、task/projector 投影和旧 prompt 区块。

#### 本次范围

- 新增 `state` 内的认知状态数据类型、边界、fingerprint/freshness 计算、摘要渲染与 commit delta。
- 把 accepted business/recovery 事件以 bounded digest 形式写入现有 ledger；只在现有 post-validation 观察边界接入。
- 为 replay、幂等、上限、持久化失败、projection disabled、isolated prompt 与非 isolated prompt 增加测试。
- 更新 `crates/ralph-core/data/ralph-tools.md`，说明 agent 如何解释新增的只读认知摘要；不新增命令。

#### 非目标

- 不实现 GAP-02 的 accepted transition 与所有 projection 的原子提交边界。
- 不实现 GAP-03 的独立终态 evaluator、acceptance gate 或“证据足够才允许终态”的业务策略。
- 不改变 `state_projection` 配置默认值、action schema、preset YAML、event policy schema、event topic、required fields 或 hat 拓扑。
- 不把任意 agent prompt、自由文本、`LoopState` 缓存或诊断报告自动升级为 claim/evidence/verified。
- 不读取或解析 `2026-08-12-001` 计划新增的 `diagnosis-input.json`、`runtime-trace.jsonl`、`feedback.jsonl`；它们将来只能通过 opaque `source_ref`/fingerprint 接入，不是本计划的编译依赖。
- 不把认知摘要注入 `HatlessRalph`、coordinator、backward-compatible custom-hat 或 `ralph` sentinel 路径。
- 不新增数据库、crate 依赖、CLI 参数、环境变量、preset 迁移或旧 ledger 文件迁移脚本。

#### 输入、输出与状态变化

| 项目 | 契约 |
|---|---|
| 输入 | 通过现有接受链路的 `accepted_log_events`；当前 loop 已知的 `iteration`、`loop_start_sha`、reconciled `plan_baseline_sha`；事件的 topic/source/target/wave metadata 与 payload digest |
| 输出 | `CommitDelta::KnowledgeObserved`（名称可按本计划固定为该名字）；`LedgerSnapshot` 中的认知记录；isolated prompt 中的 bounded read-only summary |
| 状态变化 | 成功 commit 后，ledger snapshot 增加/更新 observation；同 id 不重复；超过上限时丢弃最旧的展示记录，但 commit log 仍保持现有 append/replay 语义 |
| 错误语义 | 认知记录构造/持久化失败：warning + degraded/unknown；不返回业务错误，不撤销已接受事件，不改变原有 `ProcessedEvents` 结果 |
| 副作用 | 仅增加现有 `.ralph/ledger.jsonl` 的新 delta 和 isolated prompt 的可选摘要；不写新的 sidecar，不写 raw payload |
| 不变量 | 旧 delta replay 仍成功；accepted/rejected tuple 与开关关闭时相同；prompt 旧 heading/字段不变；摘要不含 raw payload、内部 ledger 路径或一次性诊断正文 |

#### 兼容性、性能、安全与权限

- 兼容性：旧 ledger 只有旧 delta 时新代码 replay 得到空认知状态；`StateLedger::new(workspace, false)` 仍是完全 no-op；无 ledger 的直接测试构造器不因认知功能失败。
- 性能：每个 parsed batch 最多提交一次认知 delta；不逐事件触发额外 commit；记录、证据引用和 prompt 字段均有固定上限；不做每次 prompt 的 git 子进程调用。
- 安全：不把原始 agent payload、完整事件历史、绝对路径、secret 或内部 ledger 路径写入 prompt；摘要只呈现 topic、状态、digest 前缀/opaque ref 和计数。
- 权限：沿用当前 `StateLedger` workspace 权限；认知记录不增加任何 agent CLI 权限，也不绕过现有 event policy/origin/contract gate。

#### 已确认假设

- `StateLedger` 是当前运行时持久化 SSOT，且生产 `EventLoop` 在 `acceptance_and_lifecycle.rs` 中总会初始化它。
- `accepted_log_events` 位于全部业务接受校验之后，`Disposition::Business` 与 `Disposition::Recovery` 是会推进流程的两类事件。
- 当前 prompt 安全边界只允许在现有 isolated `prepend_orchestrator_context` 链路追加；其它 prompt 路径已有 characterization tests。
- `sha2` 已是 workspace 现有依赖，当前已有 canonicalizer、artifact 与 payload digest 使用模式，不需引入新依赖。

#### 待验证假设

- 无影响正式实现路径的待验证假设。执行中若发现 `accepted_log_events` 在实际调用点不是 post-validation stream，必须触发 Unit 停止条件并重新决策，不能把前置事件流当事实源。

### 2. 代码库现状与证据

#### 2.1 当前实现入口

**外部入口与调用链**

1. JSONL 输入由 `crates/ralph-core/src/event_reader.rs` 读取为带 topic/payload/source/target/wave metadata 的事件。
2. `crates/ralph-core/src/event_loop/parse_and_emit.rs` 执行 origin、policy、state-machine、state projection、统一 validation 与 execution-contract 检查；post-validation 通过的事件进入 `accepted_log_events` / `validated_events`。
3. `crates/ralph-core/src/event_loop/disposition.rs::classify` 将事件分为 Business、Recovery、DiagnosticObservation、LoopControl；只有前两类推进业务 transition。
4. 现有 batch 结束时，`parse_and_emit.rs` 已经通过 `self.state.state_ledger` 提交 iteration/no-progress/predecessor 等 delta；新认知 delta 必须挂在同一 post-validation batch 边界，且错误只能 warning。
5. `crates/ralph-core/src/state/ledger.rs::StateLedger::commit` 将 `CommitDelta` 应用到 `LedgerSnapshot`，原子写 `.ralph/ledger.jsonl`，失败时回放 surviving log 还原 snapshot。
6. `crates/ralph-core/src/state/snapshot.rs::LedgerSnapshot::apply_delta` 是所有 delta 的 exhaustive projection；`crates/ralph-core/src/state/tests.rs::apply_delta_is_exhaustive` 已是新增 delta 的测试入口。
7. isolated prompt 由 `crates/ralph-core/src/event_loop/event_processing.rs::build_prompt` 调用 `flow_authority.rs::prepend_orchestrator_context`；该 helper 读取 `RuntimeStateSnapshot` 或 disabled stub，并保持 `ralph` sentinel 早退。

**核心模块与数据边界**

- 持久化边界：`StateLedger` + `.ralph/ledger.jsonl`；不新增平行 authority 文件。
- 认知模型边界：新增 `crates/ralph-core/src/state/knowledge.rs`，由 `state/mod.rs` 导出；它只负责类型、bounded apply、fingerprint/freshness、idempotency 和 prompt-safe render。
- 现有状态边界：`LedgerSnapshot` 增加一个默认空的认知字段；`LoopState` 不新增第二份认知缓存。
- 事件边界：只读取已经 accepted 的事件；拒收候选、diagnostic observation、loop control 不生成权威业务观察。
- prompt 边界：在现有 isolated context 前后追加独立 heading；不改 `RuntimeStateSnapshot` 的旧字段和 `to_prompt_block` 输出契约。

认知 authority 顺序固定为：`LedgerSnapshot.knowledge`（内存读取）/其 `.ralph/ledger.jsonl` replay（跨 activation durable source）→ accepted event 仅作为写入观察输入 → recovery/diagnosis journal 仅作为 `source_ref` evidence pointer → `LoopState` cache 仅作当前进程辅助 → prompt projection 永远是只读末端，不能反向成为事实源。U1 必须把该顺序编码为 `KnowledgeAuthority`/source 语义或等价的不可混淆类型，而不是只写注释。

**现有测试与验证方式**

- state 单元/回放测试：`crates/ralph-core/src/state/tests.rs`。
- prompt 集成测试：`crates/ralph-core/src/event_loop/tests/runtime_state_injection.rs`，并由 `crates/ralph-core/src/event_loop/tests/mod.rs` 注册。
- 事件真实 runtime path：`crates/ralph-core/src/event_loop/tests/*.rs` 与 `crates/ralph-core/tests/scenarios.rs` 的 `process_events_from_jsonl` / workflow guard 路径。
- replay path：`crates/ralph-core/src/event_loop/tests/replay_light_integration.rs` 与 state replay tests。
- 测试入口硬规则：使用 `cargo nextest run` 系列；最终使用 `./scripts/run-tests.sh`，不使用裸 `cargo test -p ralph-cli`。

#### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `git rev-parse --abbrev-ref HEAD`、`git rev-parse HEAD`、`git status --short` | 当前基线是 `pittcat-dev@a1dd6217`，调查时工作区干净 | 计划必须以该基线为事实，不把其它 worktree 的代码当已存在能力 | 高 |
| E2 | `git worktree list`、`git diff --stat a1dd6217..fc223418` | run-diagnosis 计划在另一个 worktree，新增/修改大量 diagnostics/diagnosis 文件但尚未 merge；该分支还删除了原 brainstorm 文件 | GAP-01 计划不能 import 未 merge 类型；必须自带完整需求契约，并保持与未来 opaque source ref 兼容 | 高 |
| E3 | GAP-01 源文件 `docs/brainstorms/2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md` | 明确要求 unified recoverable orchestration cognitive state，prompt 只是压缩投影，且要求 claim/evidence/freshness/authority/unknown 语义；边界不规定存储格式 | 需要解决语义权威与回放，不需要同时实现 GAP-02/GAP-03 的 acceptance gate | 高 |
| E4 | `crates/ralph-core/src/state/mod.rs` | `LedgerSnapshot`、`CommitDelta`、`StateLedger` 已是 unified state + append/replay SSOT；生产注释说明 ledger always enabled，同时 API 保留 false no-op | 新认知状态应扩展现有 ledger，不得另建第二套 durable source；必须保留 false no-op | 高 |
| E5 | `crates/ralph-core/src/state/ledger.rs` | `commit` 先 apply、原子持久化，失败后 pop + replay surviving log；`new(..., true)` 从 `.ralph/ledger.jsonl` replay | 可以安全增加 additive delta；认知 commit failure 可以降级而不影响业务路径，且能测试 rollback | 高 |
| E6 | `crates/ralph-core/src/state/commit.rs`、`snapshot.rs` | delta 是增量 wire format，`apply_delta` exhaustive；快照中已有多类 runtime/recovery state，但没有 knowledge 子树 | 新增单一 `KnowledgeObserved` delta 和空默认字段，必须更新 exhaustive test；不能直接塞入 LoopState | 高 |
| E7 | `crates/ralph-core/src/event_loop/parse_and_emit.rs` | `accepted_log_events` 在多层验证之后收集；batch 末尾已有 ledger commits；文件约 4895 行 | 新逻辑必须放新 helper/module，`parse_and_emit.rs` 只留很小的调用接线，避免越过 5000 行硬限制 | 高 |
| E8 | `crates/ralph-core/src/event_loop/disposition.rs` | `Business`/`Recovery` 的 `advances_flow()` 为 true；DiagnosticObservation/LoopControl 不推进业务 | 只记录前两类作为权威 runtime observation，防止诊断/控制噪声进入 cognition | 高 |
| E9 | `crates/ralph-core/src/event_loop/lifecycle.rs`、`acceptance_and_lifecycle.rs` | 生产 EventLoop 初始化 `StateLedger::new(workspace, true)` 并放入 `LoopState.state_ledger` | 不需要新配置或 preset 适配；直接模式/旧测试构造器没有 ledger 时必须 no-op | 高 |
| E10 | `crates/ralph-core/src/runtime_state.rs`、`flow_authority.rs` | `RuntimeStateSnapshot` 是旧 `## ORCHESTRATOR CONTEXT` 的 source；isolated 路径才调用 `prepend_orchestrator_context`；ralph 会早退 | 只追加独立摘要，不修改旧 snapshot/heading，也不扩大到 coordinator/custom/ralph | 高 |
| E11 | `crates/ralph-core/src/event_loop/tests/runtime_state_injection.rs`、`event_loop/tests/mod.rs` | 已有 isolated happy path、projection disabled stub、ralph skip、backward-compat custom path tests | 新行为必须追加同一测试模块的 characterization/regression，不得删除或放宽现有断言 | 高 |
| E12 | `crates/ralph-core/src/state/tests.rs` | 已有 replay、corrupt ledger、failed commit rollback、feature-disabled no-op、exhaustive delta tests | Unit 1/2 必须沿用这些真实 fault/replay 入口，不新增假 adapter 绕开 StateLedger | 高 |
| E13 | `.cursor/rules/state-management.mdc`、`docs/solutions/architecture-patterns/orchestrator-expected-event-ledger-ssot.md` | 项目既有原则是 ledger 是 state SSOT，prompt 仅展示计算后的状态，不能让 agent 读 plan prose 推断事实 | 选择 ledger authority，并明确 prompt 不可反向写 state | 高 |
| E14 | `crates/ralph-core/src/artifact_canonicalizer.rs`、现有 digest 使用处 | workspace 已有 SHA-256 canonical/digest 模式；不需要新增依赖或另一种 hash | observation 只保存 digest/opaque ref，不保存 raw payload；复用现有 `sha2` | 高 |
| E15 | `crates/ralph-core/data/ralph-tools.md` | agent-facing skill 已明确 agent 不应从内部 ledger/event history 推断事实，并规定 prompt context 的读取与停止原则 | 新摘要必须是只读、可解释、禁止依赖内部路径；同步通用 agent 行为说明 | 高 |
| E16 | `crates/ralph-core/src/event_loop/accepted_transition.rs` | 现有 accepted transition 已有 durable outbox、publish 前验证与失败不 publish 基础 | GAP-01 不重做 atomic boundary；认知记录必须挂在 accepted 后，不能替代该基础 | 高 |
| E17 | `crates/ralph-core/src/event_loop/loop_state.rs` | `loop_start_sha` 与 `plan_baseline_sha` 已存在于 LoopState；prompt 还会从磁盘 reconciled baseline | fingerprint 可使用已有字段；不新增 git 命令或实时 HEAD 扫描，无法判断时保守为 unknown | 高 |
| E18 | `wc -l` 对受影响源码的结果 | `parse_and_emit.rs` 约 4895 行；新 state/knowledge 模块可独立控制规模 | 计划禁止在大文件中堆积实现；所有新增逻辑放 `state/knowledge.rs`，接线保持小 | 高 |

#### 2.3 受影响范围

**已确认会改的生产模块**

- `crates/ralph-core/src/state/mod.rs`：注册/导出新 knowledge 模块。
- `crates/ralph-core/src/state/knowledge.rs`：计划新增；认知模型、bounded apply、fingerprint/freshness、摘要 render、accepted observation builder。
- `crates/ralph-core/src/state/commit.rs`：增加一个 additive `CommitDelta` 变体。
- `crates/ralph-core/src/state/snapshot.rs`：增加默认空认知字段并处理新 delta。
- `crates/ralph-core/src/event_loop/parse_and_emit.rs`：仅在已确认的 batch ledger commit 区域增加小型调用接线；不重写接受链路。
- `crates/ralph-core/src/event_loop/flow_authority.rs`：在现有 isolated orchestrator context helper 中追加 knowledge projection；不改旧 snapshot block。

**已确认会改的测试/文档**

- `crates/ralph-core/src/state/tests.rs`：新 delta exhaustive、replay、idempotency、failure/no-op characterization。
- `crates/ralph-core/src/event_loop/tests/runtime_state_injection.rs`：isolated、disabled、ralph/custom compatibility 和 prompt redaction tests。
- `crates/ralph-core/data/ralph-tools.md`：新增 prompt 认知摘要的 agent-facing 解释与停止条件。

**明确不改的范围**

- `presets/`、`presets/schemas/`、`presets/manifest.yml`、`presets/index.json`、zsh completion：没有新 preset、topic、required field 或配置字段。
- `crates/ralph-cli`：没有新 CLI/API/UI 入口；`inspect prompt` 无 ledger 记录时继续保持原行为。
- `crates/ralph-core/src/config/state_projection.rs`：不改 enabled 默认值和动作语义。
- `crates/ralph-core/src/diagnostics`、`src/diagnosis`：不消费未 merge 的 run-diagnosis sidecar。
- `CLAUDE.md` / `AGENTS.md`：不改仓库规则；计划文档本身使用中文即可。

### 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | 认知状态的 durable authority 放在哪里？ | 新建认知 JSONL；放入 `LoopState`；扩展现有 `StateLedger`/`LedgerSnapshot` | 扩展现有 `StateLedger`/`LedgerSnapshot`，新 delta 由 replay 重建 | E4、E5、E13 | 新文件会产生第二权威源；LoopState 无跨进程回放能力；ledger 已有原子写、回滚和启动 replay | 0.96 |
| D2 | 事件观察在哪个边界产生？ | JSONL 读入后；各 gate 之前；`accepted_log_events` 之后；bus publish 后 | 在 `accepted_log_events` 形成之后、现有 batch ledger commit 区域执行 | E7、E8、E16 | 更早会把 rejected candidate 当事实；publish 后无法覆盖 accepted transition 已完成但认知 commit 失败的 degraded 语义 | 0.95 |
| D3 | 哪些事件产生权威 observation？ | 所有读到的事件；所有 bus publish；仅 `Business`/`Recovery` | 仅 `Disposition::Business` 或 `Disposition::Recovery` | E8 | DiagnosticObservation/LoopControl 的现有契约明确不推进业务；记录它们会污染 cognition 与 prompt | 0.95 |
| D4 | 认知 commit 失败是否阻塞业务 loop？ | fail-close；返回错误；warning + 保留旧/unknown | warning + degraded/unknown，原事件结果不变 | E5、E12、E16；用户明确要求当前功能不可用风险最低 | 认知状态是 GAP-01 的观察投影，不应改变已建立的业务 acceptance/termination；fail-close 会把新增观察故障扩大成业务回归 | 0.92 |
| D5 | 是否保存 raw payload？ | 保存全文；保存截断正文；只存 digest/opaque ref | 只存 topic/source/target/metadata、payload digest、stable source ref 和 bounded semantic fields | E14、E15；安全约束 | raw payload 会泄漏 prompt/内部数据并无限膨胀；agent-facing guide 禁止依赖内部 event ledger | 0.94 |
| D6 | 如何表达真实性与新鲜度？ | 缺字段默认 verified/current；不保存状态；保守 unknown/stale/current 双维状态 | `VerificationStatus` 与 `EvidenceFreshness` 分离；无 fingerprint=unknown，fingerprint mismatch=stale，匹配才=current；`current` 只相对于调用方提供的比较 fingerprint；accepted event 不自动 verified | E3、E17；GAP-01 明确要求 unknown/freshness | 其它方案会产生 silent-success/假验证；实时 HEAD 采集会增加 loop 风险且无当前统一入口；把 fingerprint equality 误称为 Git tree proof 会制造假确定性 | 0.90 |
| D7 | prompt 如何接入？ | 重写 `RuntimeStateSnapshot`；替换旧 context；所有 hat 路径注入；isolated 追加独立 block | 保持旧 `RuntimeStateSnapshot` 和 `## ORCHESTRATOR CONTEXT` 原样，只在既有 isolated helper 追加非空 knowledge block | E10、E11、E13 | 重写会扩大兼容面；扩大路径会破坏已有 coordinator/custom/ralph characterization；空状态 no-op 保证旧 prompt 不变 | 0.95 |
| D8 | 是否依赖未 merge 的 run-diagnosis 计划？ | 等它 merge 后再做；直接 import 新类型；当前定义 opaque refs | 当前不依赖、不 import；保留 bounded `source_ref`/fingerprint 兼容未来 sidecar adapter | E1、E2、E3 | 依赖未 merge worktree 无法在当前基线执行；重复定义诊断 sidecar 会扩大 GAP-01 范围 | 0.95 |
| D9 | 是否新增配置/CLI/preset？ | 加 flag；新增 preset schema；无开关，沿用当前 ledger wiring | 不新增任何 operator 配置或 CLI；认知收集随已有 ledger 存在而运行，prompt 只在既有 isolated path 有记录时出现 | E9、E10、E15 | 新配置会产生默认/迁移/预设同步面；没有业务行为需要 operator opt-in | 0.91 |
| D10 | 记录上限与写入频率如何控制？ | 每事件 commit、无限保留；每 batch 一次 bounded delta；外部数据库 | 每 parsed batch 最多一次 delta；快照展示最多 128 records、每 record 最多 8 evidence refs、prompt semantic field 每项最多 256 bytes | E5、E7、E12、E14、E18 | 每事件 commit 增加原子重写成本；无限 prompt/state 违背 bounded projection；外部数据库超出 GAP-01 与现有架构 | 0.88 |
| D11 | agent-facing 文档应如何同步？ | 不同步；新增 CLI 专项 skill；更新共享 `ralph-tools.md` 的 prompt 解释 | 只更新 `ralph-tools.md`，描述触发条件、读取动作、字段含义和 unknown/stale 停止条件；不写内部实现 | E15 与 AGENTS.md AI skill guide 规则 | 没有新命令或参数；写内部模块/ledger 路径会违反注入 skill 可读性边界 | 0.92 |

以上决策均达到 `>=0.85`，可直接进入 Unit。若执行中任何决策置信度下降到阈值以下，必须按 Unit 停止条件暂停并回到本节更新，不能由 Executor 临时改方案。

## Planning Contract

### 4. BDD 行为规格

```gherkin
Feature: 统一、可回放的编排认知状态

  Background:
    Given EventLoop 使用现有 StateLedger
    And 认知状态默认为空
    And 现有事件接受、路由、终态和 prompt 规则保持启用

  Scenario: S01 接受后的业务事件成为带来源和输入指纹的未验证观察
    Given 一个经过全部现有校验并被接受的 Business 事件
    When 当前 batch 完成认知状态提交
    Then LedgerSnapshot 中出现一个稳定 observation id
    And 记录包含 topic、producer/source、payload digest、source ref 和 loop/plan fingerprint
    And verification status 是 unverified 而不是 verified
    And 业务事件的 accepted/publish 结果与改动前相同

  Scenario: S02 恢复事件也进入认知状态但诊断与控制事件不进入
    Given 一个 accepted Recovery 事件、一个 DiagnosticObservation 事件和一个 LoopControl 事件
    When 当前 batch 完成认知状态提交
    Then 只有 Recovery 事件产生权威 observation
    And DiagnosticObservation 与 LoopControl 不产生 business knowledge record
    And 三类事件原有 bus/route 语义不变

  Scenario: S03 缺少输入指纹时认知状态显示未知
    Given 一个 observation 没有可用 loop 或 plan fingerprint
    When 计算其 freshness
    Then freshness 是 unknown
    And 系统不把该 observation 标记为 current 或 verified

  Scenario: S04 输入指纹不匹配时证据显示过期
    Given 一个 observation 保存了 fingerprint A
    When 使用 fingerprint B 计算 freshness
    Then freshness 是 stale
    And prompt 摘要只显示 stale 计数/短摘要
    And 不生成新的业务拒收、重试或终态事件

  Scenario: S05 相同 observation 重放不会重复计数
    Given ledger 已经保存 observation id X
    When 同一个 accepted event 在 replay 或重复输入中再次生成 X
    Then snapshot 中 X 只出现一次
    And sequence/replay 仍按现有 StateLedger 规则工作

  Scenario: S06 进程重启后认知状态可恢复
    Given 第一个 StateLedger 已提交 observation 与旧业务 delta
    When 新 StateLedger 从同一 workspace replay
    Then 认知记录和旧业务 snapshot 都恢复
    And replay 不需要 prompt、LoopState cache 或诊断 sidecar

  Scenario: S07 认知持久化失败不使当前编排不可用
    Given ledger 的认知 commit 因现有持久化错误失败
    When EventLoop 完成本 batch
    Then 只记录 warning 并保留 commit rollback 后的 snapshot
    And accepted events、publish、route、termination 与 exit semantics 不改变
    And 下一个 prompt 对缺失认知显示为空或 unknown

  Scenario: S08 isolated prompt 只追加非空认知压缩投影
    Given isolated hat 的 ledger 中存在一条认知记录
    When 调用现有 isolated build_prompt
    Then 旧 `## ORCHESTRATOR CONTEXT` heading 和字段原样存在
    And 额外出现独立的只读认知摘要
    And 摘要不含 raw payload、内部 ledger 路径或完整事件历史

  Scenario: S09 projection disabled 不关闭现有功能
    Given `state_projection.enabled=false`
    When 现有 EventLoop 处理一个合法 Business 事件并构造 isolated prompt
    Then 事件仍按改动前被接受/发布
    And 旧 ORCHESTRATOR CONTEXT 仍是 disabled stub
    And 若 ledger 有认知记录，只追加认知摘要；否则 prompt 与旧行为相同

  Scenario: S10 非 isolated prompt 路径保持不变
    Given coordinator、backward-compatible custom-hat 或 ralph sentinel prompt 路径
    When 调用现有 build_prompt
    Then 不注入认知摘要
    And 既有 prompt block title、accepted events 和路由结果保持不变

  Scenario: S11 agent 能按规则解释认知摘要
    Given prompt 出现 `## ORCHESTRATION KNOWLEDGE`
    When agent 读取摘要并准备下一步动作
    Then agent 将其视为只读的编排器投影，而不是可写事实源
    And agent 能区分 current、stale、unverified、unknown
    And stale 或 unknown 不能被当成终态证据；必要时 agent 停止并报告缺失证据
```

### 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
|---|---|---|---|---|---|
| S01 接受后 observation | commit log 有 `knowledge_observed`；记录有稳定 id、topic、producer、digest、fingerprint；不存在 raw payload；accepted tuple 不变 | `crates/ralph-core/src/state/tests.rs` + `crates/ralph-core/src/event_loop/tests/runtime_state_injection.rs` | state unit + EventLoop integration | Property-like table test 覆盖 Business/Recovery/metadata 缺省 | 否 |
| S02 disposition 筛选 | Business/Recovery 有记录，DiagnosticObservation/LoopControl 无权威记录；各原有输出仍存在 | `crates/ralph-core/src/state/knowledge.rs` unit；event-loop integration | unit + integration | 复用 `disposition::classify`，不得 mock classifier | 否 |
| S03/S04 freshness | 无 fingerprint=unknown；相同=current；不同=stale；状态不触发 route/gate | `state/knowledge.rs` unit | unit | table/property-like 三态组合 | 否 |
| S05 幂等 | 同 observation id apply 两次，snapshot 只有一条且 prompt count 不变 | `state/knowledge.rs` unit + `state/tests.rs` | unit + replay integration | 重复 batch characterization | 否 |
| S06 replay | 新 ledger replay 后认知字段与旧业务代表字段恢复；旧 ledger 无 knowledge delta 仍成功 | `state/tests.rs` | integration | 旧 delta fixture + new delta fixture differential replay | 否 |
| S07 persist failure | 复用现有 ledger path-as-directory fault；新 delta commit 返回错误后认知与业务 snapshot rollback；EventLoop helper 不把错误向上冒泡 | `state/tests.rs` | unit/integration | fault injection 使用现有真实文件错误，不 mock StateLedger | 否 |
| S08 isolated prompt | 有记录时有新 heading；无记录时无新 heading；旧 context exact/semantic assertions 全通过；raw payload/path 不出现 | `event_loop/tests/runtime_state_injection.rs` | integration | prompt redaction assertion | 否 |
| S09 projection disabled | disabled stub、accepted/published event 结果与 baseline 一致；knowledge block 只在有记录时出现 | `runtime_state_injection.rs` + `crates/ralph-core/tests/scenarios.rs` 真实 runner path | integration/BDD-like runtime | off-path differential | 否 |
| S10 非 isolated | coordinator/custom/ralph 不出现新 heading；原有 block titles/acceptance 不变 | `runtime_state_injection.rs` | characterization/integration | 现有 tests 全部保留 | 否 |
| S11 agent-facing 解释 | 新 block 出现时 agent guide 明确“只读、来源、字段、unknown/stale 停止条件”；不泄漏内部模块/路径 | `crates/ralph-core/data/ralph-tools.md` 静态人工审阅 + `scripts/check-cli-doc-drift.sh` | 文档契约/静态 drift | 不新增文本锁定型生产测试 | 否 |

测试层级选择理由：认知纯函数与 bounded/digest/freshness 使用 unit；ledger apply/replay/failure 使用 state integration；事件过滤和 prompt 组合必须经过真实 EventLoop；没有新 CLI/API/跨服务边界，不添加 E2E 或 contract test；现有 scenario runner 只用于证明 `state_projection.enabled=false` 的真实 acceptance path 未变化。

### 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | Ledger 是认知状态唯一 durable authority | S01/S06 | `knowledge_observation_delta_round_trips_through_replay` | `knowledge_record_apply_is_bounded_and_idempotent` | StateLedger replay | 否 | E4-E6 |
| R2 | 只从 post-validation accepted Business/Recovery 观察 | S01/S02 | `accepted_business_and_recovery_events_create_observations` | `observations_from_accepted_events_filters_disposition` | EventLoop accepted path | 否 | E7-E8 |
| R3 | 记录有 producer/source/fingerprint/freshness/verification 语义 | S01/S03/S04 | `knowledge_freshness_is_conservative` | `knowledge_freshness_is_conservative` | prompt summary | 否 | E3、E17 |
| R4 | unknown/stale 不可被当成 verified | S03/S04 | `knowledge_freshness_is_conservative` | `KnowledgeKind`/`VerificationStatus` round-trip | 无业务 route 变化断言 | 否 | E3、E13 |
| R5 | observation idempotency 与 bounded snapshot | S05 | `knowledge_record_apply_is_bounded_and_idempotent` | `observation_id_is_stable` | replay duplicate batch | 否 | E5、E12、E14 |
| R6 | 认知 commit 失败不阻塞当前功能 | S07 | `knowledge_commit_failure_does_not_change_processed_result` | `commit_accepted_observations_is_fail_soft` | EventLoop result unchanged | 否 | E5、E12、E16 |
| R7 | prompt 是只读压缩 projection | S08/S09/S10 | `isolated_prompt_includes_knowledge_projection_when_non_empty` | `render_prompt_block_never_contains_raw_payload_or_path` | existing prompt injection tests | 否 | E10-E11 |
| R8 | `state_projection.enabled=false` 仍可运行 | S09 | `disabled_projection_keeps_old_stub_and_adds_only_knowledge` | `feature_disabled_knowledge_commit_is_noop` | real workflow/scenario path | 否 | E9、E11 |
| R9 | 非 isolated/ralph 路径不扩大注入面 | S10 | `ralph_and_legacy_custom_paths_do_not_get_knowledge_projection` | 无 | prompt integration | 否 | E10-E11 |
| R10 | agent 能正确解释摘要且不接触内部 ledger | S11 | 文档检查清单 | 无 | `check-cli-doc-drift.sh` | 否 | E15 |
| R11 | 不依赖未 merge run-diagnosis 代码 | 全部 | 编译基线不引用 diagnosis 新类型 | opaque source ref unit | 两 worktree merge 后仍可 replay | 否 | E1-E2 |

## Implementation Units

### 7. 严格串行开发单元

以下 Unit 严格串行。每个 Unit 都必须完成 Acceptance Red → Unit Red → Green → Refactor → Integration → Regression → Close，才能进入下一个 Unit。

### U1. 建立可回放的认知状态数据契约

#### 1. Unit 目标

让 `LedgerSnapshot` 能表示并通过现有 `StateLedger` replay 保存 bounded cognition records；本 Unit 不接入 EventLoop，也不改 prompt。

#### 2. 对应需求与 Scenario

- Requirement：R1、R3、R4、R5
- Scenario：S03、S04、S05、S06
- Decision：D1、D5、D6、D10
- Evidence：E4、E5、E6、E12、E14、E17、E18

#### 3. 外部可观察结果

对 state API 调用方而言，空 ledger 的既有 snapshot 不变；新增 API 可以构造 claim/evidence/hypothesis/assumption/unknown/verified/falsified/decision/route-reason/observation 记录，经过 commit 后 replay 恢复；同 id 不重复，超限展示记录有界。

#### 4. 当前行为基线

当前 `LedgerSnapshot` 没有 knowledge 字段，`CommitDelta` 没有 knowledge variant；`apply_delta_is_exhaustive` 覆盖现有 delta，`new_replays_from_disk` 只恢复现有字段。因为目标是新增行为，Acceptance Red 必须在改生产代码前先添加并运行会编译/断言失败的 state tests；不得以旧测试全绿替代真实 Red。

#### 5. 输入与输出

- 输入：`KnowledgeRecord`，包含 kind、stable id、subject、verification status、source ref、producer、evidence refs、`InputFingerprint`、observation iteration/time；字段均做长度和数量限制。
- 输出：`CommitDelta::KnowledgeObserved { records }`；`LedgerSnapshot.knowledge`；`KnowledgeView` 的 current/stale/unknown/unverified counts。
- 错误：非法/超限记录由 builder 归一化或拒绝为 `Result`；apply 不 panic；旧 ledger 行仍可 replay。
- 状态变化：同 id apply 是 no-op；新记录按 observation sequence 保留；展示集合最多 128 条。
- 副作用：只在调用 `StateLedger::commit` 时写已有 `.ralph/ledger.jsonl`；不写 prompt、不写新文件。
- 不变量：不保存 raw payload；`verified`/`falsified` 只能由显式 typed status 输入产生，不由 accepted event builder 生成；无 fingerprint 的记录 freshness 必须是 unknown。

#### 6. 修改位置

- `crates/ralph-core/src/state/knowledge.rs`（新增）：拥有所有认知类型、常量、fingerprint/freshness、bounded apply、idempotent apply、prompt-safe view/render 所需的纯逻辑；不拥有 EventLoop 路由。
- `crates/ralph-core/src/state/mod.rs`（修改）：注册 `knowledge` module 和公开需要的类型；不改 StateLedger 初始化。
- `crates/ralph-core/src/state/commit.rs`（修改）：只增加一个 `KnowledgeObserved` delta；不重命名/重排旧 variant。
- `crates/ralph-core/src/state/snapshot.rs`（修改）：增加 `pub knowledge: OrchestrationKnowledgeState` 的空默认值，并在 exhaustive `apply_delta` 中处理新 delta；不改 task/progress/policy/state-machine 字段。
- `crates/ralph-core/src/state/tests.rs`（修改）：扩展现有 exhaustive/replay/failure/no-op 测试；不删除旧测试或改弱断言。

#### 7. 可依赖能力

- 现有 `serde::{Serialize, Deserialize}`、`sha2`、`BTreeMap/Vec`、`StateLedger::commit/replay`。
- 现有 `LedgerSnapshot::cold_start`、`StateLedger` atomic rollback。
- 现有 `artifact_canonicalizer` 的 digest 约定可作为 hash 形态参考，但不得复制其 raw artifact 内容。

#### 8. 禁止依赖的未来能力

- 不调用 EventLoop、disposition、prompt builder 或 run-diagnosis modules。
- 不实现 event acceptance 接线、prompt injection、agent guide 文档或 GAP-02/GAP-03 gate。

#### 9. 验收测试

- `knowledge_record_apply_is_bounded_and_idempotent`：构造重复 id、超过 128 条、超过 8 evidence refs 和超长 semantic field；断言只保留 bounded 最新视图、重复 id 一条、无 raw payload。
- `knowledge_freshness_is_conservative`：无 fingerprint→unknown；相同 fingerprint→current；不同 fingerprint→stale；verification status 不被 freshness 改写。
- `knowledge_observation_delta_round_trips_through_replay`：commit 新 delta，重新 `StateLedger::new(workspace, true)`，断言 knowledge view 与旧 business scalar 同时恢复。
- `old_ledger_without_knowledge_replays_to_empty_knowledge`：只写旧 `CounterChanged`，新 replay 成功且 knowledge empty。
- `feature_disabled_knowledge_commit_is_noop`：沿用现有 `StateLedger::new(dir, false)`，断言不创建文件、不改 snapshot、不增加 commit。

运行：`cargo nextest run -p ralph-core -- knowledge` 与 `cargo nextest run -p ralph-core -- state`。

#### 10. Acceptance Red

1. 先在 `state/tests.rs` 写 `knowledge_observation_delta_round_trips_through_replay` 和 `knowledge_freshness_is_conservative`，再运行 `cargo nextest run -p ralph-core -- knowledge`。
2. 预期失败必须是编译错误（`knowledge` module/field/delta/API 不存在）或目标断言失败（snapshot 没有记录）；这证明测试确实触及待交付能力。
3. 不接受的 Red：命令拼错、nextest 未执行目标测试、fixture 解析失败、既有 unrelated test 失败、通过 `#[ignore]`/删除断言规避。

#### 11. 单元测试拆分

- `InputFingerprint::freshness_against`：输入 none/same/different，期望 unknown/current/stale；不 mock。
- `KnowledgeRecord::validate_and_bound`：输入超长/超量，期望 deterministic truncation/rejection；不允许保存 raw payload。
- `OrchestrationKnowledgeState::apply`：输入新 id/重复 id/超过上限，期望一条、幂等、bounded。
- `KnowledgeKind` 与 `VerificationStatus` round-trip：覆盖全部 first-class categories；不让 serde 默认值隐式变成 verified。
- `render_prompt_summary`：输入记录含 payload-like text/path，断言输出只含 digest/opaque ref/计数，不含正文或绝对路径。

#### 12. Red → Green → Refactor 顺序

`knowledge_freshness_is_conservative` Red → 新增 `InputFingerprint` 与 freshness 逻辑 → Green → `knowledge_record_apply_is_bounded_and_idempotent` Red → 新增 bounded/idempotent apply → Green → `knowledge_observation_delta_round_trips_through_replay` Red → 新增 delta、snapshot field、apply match 和 module export → Green → `old_ledger_without_knowledge_replays_to_empty_knowledge` Green → `feature_disabled_knowledge_commit_is_noop` Green → Refactor 类型命名、serde defaults、prompt-safe helper → 重新运行 Unit 全部测试。

#### 13. 最小实现范围

- 必须：可序列化 delta、空默认 state、全部 first-class kind、`KnowledgeAuthority`/source precedence、verification/freshness 分离、stable id、128/8/256 bounds、replay、idempotency、raw payload 不进入 state。
- 必须修改：`state/mod.rs`、`commit.rs`、`snapshot.rs` 和新增 `knowledge.rs`。
- 必须处理：旧 ledger、feature disabled、duplicate id、超过上限、缺 fingerprint。
- 必须保持：所有旧 delta 语义和旧 state tests。
- 不实现：EventLoop observation wiring、prompt wiring、任何 gate/route。

#### 14. 集成验证

- 真实联合 `StateLedger`、`CommitDelta`、`LedgerSnapshot::apply_delta`、JSONL persistence/replay。
- 可以使用 `tempfile::TempDir`；不得 mock `StateLedger` 的 apply/replay。
- 必须真实验证 old delta + new delta 混合 replay、持久化失败 rollback、feature false no-op。
- 命令：`cargo nextest run -p ralph-core -- state`；失败不得进入 U2。

#### 15. 风险驱动测试

- Characterization：现有 `apply_delta_is_exhaustive`、`new_replays_from_disk`、`failed_commit_preserves_snapshot`、`feature_disabled_commit_is_noop` 必须原样通过；因为新增 delta 最容易破坏 exhaustive/replay/failure。
- Differential replay：旧 delta-only ledger 与新 binary replay 的旧代表字段相同；因为用户要求当前功能不可用风险最低。
- Property-like table：kind/status/fingerprint 组合不能把 unknown 自动升为 verified；因为 GAP-01 的核心风险是假事实。

#### 16. 回归范围

- 直接：`cargo nextest run -p ralph-core -- state`，覆盖 commit/apply/replay/feature off。
- 相邻：`cargo nextest run -p ralph-core -- replay_light_integration`，确保 replay/恢复基础不变。
- 公开消费者：所有 `LedgerSnapshot`/`CommitDelta` compile consumers；不得遗漏 exhaustive match。
- 旧配置/默认关闭：本 Unit 不改配置，但必须保留 `StateLedger::new(..., false)` 测试。
- Build/Lint/Typecheck：`cargo fmt --all -- --check`、`cargo clippy -p ralph-core --all-targets --all-features -- -D warnings`、`cargo build -p ralph-core`。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/state/knowledge.rs` | 新增生产文件 | cognition model/bounds/freshness/replay projection | E4-E6、E14、E18 |
| `crates/ralph-core/src/state/mod.rs` | 修改生产文件 | 注册/导出 module | E4 |
| `crates/ralph-core/src/state/commit.rs` | 修改生产文件 | additive knowledge delta | E5-E6 |
| `crates/ralph-core/src/state/snapshot.rs` | 修改生产文件 | 存储/应用 knowledge state | E6 |
| `crates/ralph-core/src/state/tests.rs` | 新增测试 | replay/fault/no-op/exhaustive acceptance | E12 |

#### 18. 完成标准

当前 Unit 的五个验收测试与 state 全部 targeted tests 通过；old delta replay、failure rollback、feature false no-op 通过；fmt/clippy/build 通过；无 skipped/only/弱断言；`parse_and_emit.rs` 尚未接线；Evidence/Decision 记录不下降；可独立提交。

#### 19. 停止条件

新增 delta 导致旧 ledger 无法 replay、StateLedger 只能通过新配置启用、feature false 写盘、需要改现有 delta wire shape、测试 Red 不是目标缺失、文件规模需要把实现塞进 `parse_and_emit.rs`，或 D1/D4/D6/D10 置信度低于 0.85 时停止；记录新 Evidence，重新比较 D1-D10 后再继续。

#### 20. 风险与注意事项

- 风险：新增 delta 被错误设计成 whole snapshot，导致 ledger 膨胀。检测：检查 JSONL line 与 bounded state；缓解：只允许 `KnowledgeObserved` 增量和 128 条展示上限；剩余风险是长期 commit log 仍按现有 ledger 策略增长。
- 风险：serde 默认把缺失 status 当 verified。检测：round-trip/unknown tests；缓解：所有可选字段显式 default unknown/unverified。
- 风险：新的 `apply_delta` 分支漏处理。检测：exhaustive test 与编译；缓解：Unit close 前必须跑 state 全包。

### U2. 在 accepted batch 边界记录真实运行观察

#### 1. Unit 目标

让现有 EventLoop 只把 post-validation 的 Business/Recovery accepted events 转成 U1 已验证的数据契约，并以每 batch 最多一次、fail-soft 的方式提交；不改变任何业务事件结果。

#### 2. 对应需求与 Scenario

- Requirement：R2、R5、R6、R8、R11
- Scenario：S01、S02、S05、S07、S09
- Decision：D2、D3、D4、D8、D9、D10
- Evidence：E2、E7、E8、E9、E16、E17、E18

#### 3. 外部可观察结果

合法 Business/Recovery 事件处理完成后，ledger 产生一条 bounded knowledge delta；rejected/diagnostic/control 不产生权威记录；认知 commit 错误只 warning，`ProcessedEvents` 和业务 loop 结果完全保持原语义。

#### 4. 当前行为基线

当前 `parse_and_emit.rs` 已在 `accepted_log_events` 收集后执行 predecessor 与 batch sync ledger commit，但没有 knowledge observation。现有 `accepted_events`/route 由 integration tests 覆盖；新增 Acceptance Red 必须先断言 accepted business event 对应 knowledge record，预期因没有 wiring 而失败。

#### 5. 输入与输出

- 输入：当前函数已有的 `accepted_log_events`、`self.state.iteration`、`self.state.loop_start_sha`、`resolve_reconciled_plan_baseline_sha` 可用 fingerprint；由 `disposition::classify(topic).advances_flow()` 过滤。
- 输出：调用 U1 的 `KnowledgeObservation::from_accepted_event`，生成一个 `KnowledgeObserved` delta 并调用现有 `StateLedger::commit` 一次。
- 错误：构造或 commit 错误用 `tracing::warn!`，不使用 `?` 把错误带出业务处理函数；`state_ledger=None` 或 `feature_enabled=false` 直接 no-op。
- 状态变化：只在 commit 成功时更新 knowledge snapshot；原有 iteration/no-progress/predecessor commits 不删除、不合并、不改顺序语义。
- 副作用：每 batch 最多一个额外 ledger commit；不写 events.jsonl，不 publish 新 event，不创建 sidecar。
- 不变量：观察发生在现有 accepted stream 之后；同一个 accepted event 的 accepted/publish/route 顺序不变；不记录 raw payload。

#### 6. 修改位置

- `crates/ralph-core/src/state/knowledge.rs`：新增 `observations_from_accepted_events` / `commit_accepted_observations` helper；只消费 proto event 与已知 fingerprint，不调用 gates。
- `crates/ralph-core/src/event_loop/parse_and_emit.rs`：在现有 batch ledger commit 区域增加小型 helper 调用；调用前先在 immutable borrow 阶段取得 `iteration`、`loop_start_sha` 和 reconciled `plan_baseline_sha`，再把这些值与 accepted events 传入 helper，避免在 `&mut state_ledger` 借用期间重新借用 `self`；不得在 `accept_event!` 宏、state-machine validation、projection validation 或 publish 前插入 cognition side effect。
- `crates/ralph-core/src/state/tests.rs`：增加 helper 的 accepted/disposition/failure tests。
- `crates/ralph-core/src/event_loop/tests/runtime_state_injection.rs` 或现有真实 event-loop test 文件：增加合法 Business/Recovery 与 rejected/diagnostic/control characterization；具体使用已存在的 `process_events_from_jsonl` fixture，不新建 mock loop。

#### 7. 可依赖能力

- U1 已完成并关闭的 model/delta/replay 能力。
- 现有 `Disposition::classify`，必须真实调用。
- 现有 `StateLedger`、`ProcessedEvents.accepted_events`、JSONL fixture 与 `process_events_from_jsonl`。

#### 8. 禁止依赖的未来能力

- 不改变 AcceptedTransition 原子性；不把 knowledge commit 当 accepted transition gate。
- 不生成 claim/evidence/verified 判断；accepted event 只生成 `Observation + Unverified`。
- 不添加 prompt block、agent guide、CLI/preset/config。

#### 9. 验收测试

- `accepted_business_and_recovery_events_create_observations`：写入合法 Business 与 Recovery event，运行真实 `process_events_from_jsonl`，断言 `ProcessedEvents.accepted_events` 与 baseline 相同，ledger knowledge 有对应两条 stable ids。
- `rejected_and_non_advancing_events_do_not_create_observations`：分别覆盖 policy/state/projection rejection、DiagnosticObservation、LoopControl；断言无 knowledge record，原有 rejection/bus/loop-control 断言保留。
- `knowledge_commit_failure_does_not_change_processed_result`：将 `.ralph/ledger.jsonl` 建为 directory，运行 helper/真实 batch；断言处理结果不变、业务 ledger 旧字段不变、warning 产生且没有 panic。
- `one_batch_has_at_most_one_knowledge_commit`：一个 batch 多个 accepted events，读取 commit log，断言 knowledge delta 数量最多一条。

运行：`cargo nextest run -p ralph-core -- accepted_business_and_recovery_events`、`cargo nextest run -p ralph-core -- runtime_state_injection`、`cargo nextest run -p ralph-core -- event_policy`、`cargo nextest run -p ralph-core -- disposition`。

#### 10. Acceptance Red

1. 在现有 EventLoop integration fixture 中增加对 `ledger.snapshot().knowledge` 的断言，先运行目标测试。
2. 预期 Red 是 accepted events 仍成功但 knowledge record 数为 0；这是准确证明“新 observation wiring 缺失”的 Red。
3. 若测试在 accepted event 断言前因 config/fixture/环境变量污染失败，先修正 fixture 并记录 Evidence；若事件被已有 gate 拒绝，不能把它算作有效 Red。

#### 11. 单元测试拆分

- `observations_from_accepted_events_filters_disposition`：输入四 disposition topics，期望只返回 Business/Recovery；使用真实 `classify`，不 mock。
- `accepted_event_observation_contains_digest_not_payload`：输入含敏感/长 payload 的 event，期望 digest/source ref/metadata 存在，原文不存在。
- `observation_id_is_stable`：相同 iteration/index/topic/source/payload digest 两次生成同 id，不同任一字段生成不同 id。
- `commit_accepted_observations_is_fail_soft`：commit path-as-directory，期望 helper 返回 degraded/no-op signal 而不是错误冒泡；不得 mock `StateLedger::commit`。

#### 12. Red → Green → Refactor 顺序

`accepted_business_and_recovery_events_create_observations` Red → 在 `knowledge.rs` 实现 accepted-event builder/filter → 在 `parse_and_emit.rs` batch ledger 区域加入一次 helper call → Green → `rejected_and_non_advancing_events_do_not_create_observations` Red → 修正 disposition 过滤和空 batch 行为 → Green → `knowledge_commit_failure_does_not_change_processed_result` Red → 将 commit error 限定为 warning/no-op → Green → `one_batch_has_at_most_one_knowledge_commit` Red → 确保 helper 收集后单次 commit → Green → Refactor 接线注释/借用范围 → targeted integration/regression。

#### 13. 最小实现范围

- 必须：accepted post-validation 接线、Business/Recovery 过滤、batch single commit、fail-soft、fingerprint 传递、真实 digest/source ref。
- 必须处理：state ledger absent/false、空 batch、重复 event、ledger persist failure。
- 必须保持：原有 accepted/rejected/publish/route/terminal/output。
- 不实现：prompt、full claims/evidence evaluator、run-diagnosis adapter、new event/topic/config。

#### 14. 集成验证

- 真实联合 `EventLoop::process_events_from_jsonl`、`Disposition::classify`、`StateLedger`、现有 event bus 与 `ProcessedEvents`。
- 允许使用现有 temp workspace/event JSONL fixture；禁止用 mock accepted result 替代真实 parser/gate。
- 必须真实检查 accepted/rejected tuple、ledger delta count、replay-compatible JSONL。
- 命令：`cargo nextest run -p ralph-core -- runtime_state_injection`、`cargo nextest run -p ralph-core -- event_policy`、`cargo nextest run -p ralph-core -- disposition`；全部通过后才进 U3。

#### 15. 风险驱动测试

- Characterization：对同一合法输入保存改动前 accepted tuple/termination-relevant result，在接线后逐项比较；因为 observation 接线最容易误放在 acceptance 前或改变顺序。
- Fault Injection：ledger path-as-directory，验证新 commit 失败只影响 cognition；因为用户明确要求当前功能继续可用。
- Idempotency：重复同 batch，knowledge 不重复且业务 event 不被二次路由；因为 replay/重启是 GAP-01 的核心。

#### 16. 回归范围

- 直接：state + `runtime_state_injection` + `event_policy` + `disposition`。
- 相邻：`replay_light_integration`、`u3_jsonl_emit_gate`、`u11_unified_pipeline_integration`，因为接线位于统一 acceptance 后。
- 公开消费者：`ProcessedEvents.accepted_events`、`AcceptedTransition`、bus observers、termination paths；只断言结果不变。
- 旧配置：projection on/off、无 event policy、旧 preset fixture；无 preset 文件变更。
- 构建/Lint/Typecheck：`cargo fmt --all -- --check`、`cargo clippy -p ralph-core --all-targets --all-features -- -D warnings`、`cargo build -p ralph-core`。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/state/knowledge.rs` | 修改生产文件 | accepted observation builder/filter/commit helper | E7-E9 |
| `crates/ralph-core/src/event_loop/parse_and_emit.rs` | 修改生产文件 | 仅增加 batch-boundary wiring | E7、E18 |
| `crates/ralph-core/src/state/tests.rs` | 修改测试 | helper/replay/fault assertions | E12 |
| `crates/ralph-core/src/event_loop/tests/runtime_state_injection.rs` | 修改测试 | 真实 accepted/non-advancing behavior | E7-E11 |

#### 18. 完成标准

所有 U2 验收与 targeted regression 通过；事件 accepted/rejected/publish/route/termination 与 baseline 相同；故障注入不冒泡；每 batch 最多一条 knowledge delta；无新 topic/config/preset；`parse_and_emit.rs` 未越过 5000 行；可独立提交。

#### 19. 停止条件

需要修改 gate 顺序、AcceptedTransition、event policy/schema、`ProcessedEvents` 对外语义、终态/退出码，或发现 accepted stream 不是 post-validation，或 persist failure 只能通过 fail-close 才能满足测试，或出现新的公开调用方时停止；更新 Evidence/Decision，禁止继续到 U3。

#### 20. 风险与注意事项

- 风险：把 diagnostic/control 记录进权威 cognition。检测：disposition table test；缓解：只调用 `advances_flow()`。
- 风险：认知 commit 在 accepted transition 前执行，失败改变业务。检测：代码 review 接线位置 + accepted tuple differential；缓解：只放在已有 batch ledger区域。
- 风险：额外 ledger rewrite 影响长 batch。检测：每 batch commit count/targeted benchmark smoke；缓解：batch single commit + bounded delta；剩余风险是现有 ledger 全量重写成本。

### U3. 在现有 isolated prompt 中追加只读认知投影

#### 1. Unit 目标

当且仅当现有 isolated prompt 对应的 ledger 有认知记录时，追加一个安全、bounded、只读的摘要；保持旧 `## ORCHESTRATOR CONTEXT`、disabled stub、ralph/coordinator/custom 路径不变。

#### 2. 对应需求与 Scenario

- Requirement：R3、R4、R7、R8、R9、R10
- Scenario：S03、S04、S08、S09、S10
- Decision：D5、D6、D7、D9、D11
- Evidence：E10、E11、E13、E15、E17

#### 3. 外部可观察结果

isolated hat prompt 在旧 context 之外可看到 `## ORCHESTRATION KNOWLEDGE` 摘要：来源是 durable state ledger，展示 bounded counts/短 subject/status/freshness/source ref；无记录时完全不出现。agent guide 告知它只读、不能由它反向修改状态，unknown/stale 不能当作完成证据。

#### 4. 当前行为基线

`flow_authority.rs::prepend_orchestrator_context` 只生成 `RuntimeStateSnapshot::to_prompt_block()`；现有 tests 锁定 isolated context、disabled stub、ralph skip 和 backward-compatible custom path。新的 prompt Red 必须先在有 seeded knowledge ledger 的 isolated fixture 中断言新 heading，当前应失败；同时添加 no-record no-op 断言。

#### 5. 输入与输出

- 输入：U1/U2 已持久化的 `LedgerSnapshot.knowledge`，当前 prompt 可用的 loop/plan fingerprint；不读 events.jsonl、diagnosis sidecar、prompt prose。
- 输出：`## ORCHESTRATION KNOWLEDGE`；字段固定为 source authority、record count、unknown/stale/unverified counts、最多若干最新 subject/status/source ref；不输出 raw payload。
- 错误：ledger snapshot 不可用/无记录/渲染失败时返回原 prompt，不阻塞 hat activation。
- 状态变化：prompt build 只读；不消费 knowledge record、不清空 ledger、不改变 handoff/activation tracker。
- 副作用：仅字符串追加；旧 `## ORCHESTRATOR CONTEXT` 输出不得重写。
- 不变量：`hat_id == ralph` 仍早退；coordinator/custom legacy 路径不新增 heading；projection disabled 旧 stub 仍在。

#### 6. 修改位置

- `crates/ralph-core/src/state/knowledge.rs`：提供 prompt-safe `to_prompt_block`/view render；不读取外部文件，不暴露内部 path。
- `crates/ralph-core/src/event_loop/flow_authority.rs`：在现有 `prepend_orchestrator_context` 内，保留旧 snap 生成和 `format!`，只在适用时组合 knowledge block；不要修改 `RuntimeStateSnapshot` 类型或旧 block renderer。
- `crates/ralph-core/src/event_loop/tests/runtime_state_injection.rs`：扩展现有模块测试 seeded/non-seeded/disabled/ralph/custom/coordinator。
- `crates/ralph-core/data/ralph-tools.md`：增加 `## ORCHESTRATION KNOWLEDGE` 的 agent-facing 规则，必须写触发条件、读取动作、字段来源/含义、unknown/stale 停止条件；不得写 Rust 模块名、内部 ledger 路径、计划编号或 reviewer-only 语义。

#### 7. 可依赖能力

- U1 的 renderer/view 和 U2 的 accepted observations。
- 现有 `prepend_orchestrator_context` isolated-only 边界和 runtime_state injection tests。
- 现有 `ralph-tools.md` prompt context 规则。

#### 8. 禁止依赖的未来能力

- 不修改 `RuntimeStateSnapshot` fields/heading/serialization。
- 不扩大到 `HatlessRalph`、coordinator、custom legacy、ralph sentinel。
- 不改 `inspect prompt` CLI 参数/JSON contract；空 ledger 的 preview 必须保持旧 block titles。
- 不把 prompt 摘要作为下一轮事实输入，也不新增 agent command。

#### 9. 验收测试

- `isolated_prompt_includes_knowledge_projection_when_non_empty`：直接通过现有 StateLedger commit seed 一条 safe record，再调用现有 isolated `build_prompt`；断言旧 heading/字段和新 heading 同时存在，raw payload/path 不存在。
- `isolated_prompt_omits_empty_knowledge_projection`：无 records，断言新 heading 不存在，旧 prompt 与 baseline 相同。
- `disabled_projection_keeps_old_stub_and_adds_only_knowledge`：`state_projection.enabled=false`，seed knowledge；断言 disabled stub、旧 heading、新摘要都符合契约；事件 acceptance 不由 prompt 测试替代，沿用 U2 real path。
- `ralph_and_legacy_custom_paths_do_not_get_knowledge_projection`：覆盖 ralph sentinel 和 backward-compatible custom/coordinator，断言新 heading 不出现。
- `knowledge_prompt_is_redacted_and_bounded`：用长/敏感 source ref 与 payload-like subject，断言摘要 bounded、无 raw 内容、无绝对路径。

运行：`cargo nextest run -p ralph-core -- runtime_state_injection`、`cargo nextest run -p ralph-core -- prompt_preview`。

#### 10. Acceptance Red

先在 `runtime_state_injection.rs` 添加 `isolated_prompt_includes_knowledge_projection_when_non_empty`，seed U1 record 后运行测试；预期 prompt 不含新 heading，断言失败且旧 prompt 已生成。这是有效 Red，因为证明缺失的是 prompt projection，而非 loop 初始化/fixture。

无效 Red：prompt 为 `None`、hat 未注册、旧 ORCHESTRATOR CONTEXT 本身缺失、环境变量污染导致非 human 语义、或仅因为 snapshot 字符串格式误改而失败；这些必须先修复或停止，不能进入实现。

#### 11. 单元测试拆分

- `render_prompt_block_empty_is_noop`：空 state→空字符串。
- `render_prompt_block_contains_authority_and_counts`：非空 state→source authority/counts/status。
- `render_prompt_block_never_contains_raw_payload_or_path`：输入危险 text→输出不含原文/绝对路径。
- `render_prompt_block_caps_records`：超过上限→固定上限且顺序稳定。
- `prepend_orchestrator_context_preserves_legacy_block`：旧 snapshot block 内容保持 existing assertions；新 block 单独断言。

#### 12. Red → Green → Refactor 顺序

`isolated_prompt_includes_knowledge_projection_when_non_empty` Red → 在 `knowledge.rs` 实现 safe renderer并在 `flow_authority.rs` 读取 ledger snapshot → Green → `isolated_prompt_omits_empty_knowledge_projection` Green → `disabled_projection_keeps_old_stub_and_adds_only_knowledge` Red → 修正 projection-disabled 下旧 stub/新 block 的组合 → Green → `ralph_and_legacy_custom_paths_do_not_get_knowledge_projection` Green → `knowledge_prompt_is_redacted_and_bounded` Red → 修正 bounded/redaction → Green → 更新 `ralph-tools.md` → 运行 drift check → Refactor block placement/comments → regression。

#### 13. 最小实现范围

- 必须：isolated-only、non-empty-only、read-only、bounded、redacted、source/status/counts、旧 block untouched。
- 必须处理：projection disabled、empty state、ralph/custom/coordinator、ledger unavailable。
- 必须保持：现有 prompt block titles、auto-inject、handoff/resume、activation side effects。
- 不实现：CLI preview 新参数、其它 hat path、prompt-to-state writeback、agent emit workflow。

#### 14. 集成验证

- 真实联合 `EventLoop::build_prompt`、`flow_authority`、`StateLedger` 和 existing prompt tests。
- 可在 test fixture 中直接 seed ledger；必须真实调用 build_prompt，不得只测试 renderer 就关闭 integration。
- 必须真实验证 `state_projection.enabled=false` 和 ralph/custom no-op。
- 命令：`cargo nextest run -p ralph-core -- runtime_state_injection`、`cargo nextest run -p ralph-core -- prompt_preview`。

#### 15. 风险驱动测试

- Characterization：现有 disabled/ralph/custom prompt tests 不删不弱；因为 prompt injection scope 扩大是主要回归面。
- Redaction/bounded test：防止 prompt 泄漏 raw payload、内部路径或把大 state 注入 token；依据 E15 与用户安全/可用性要求。
- Differential prompt：空 knowledge 的新 prompt 与 baseline 逐 block/heading 比较；因为 empty no-op 是兼容核心。

#### 16. 回归范围

- 直接：`runtime_state_injection`、`prompt_preview`、`state_projector` prompt tests。
- 相邻：`crates/ralph-cli/tests/inspect_prompt.rs`、`preview_characterization`；CLI 不应因空 ledger/new block 发生 drift。
- 公开消费者：isolated hat prompt、coordinator/custom/ralph prompt、agent skill injection；只追加新 block，不改既有消费者。
- 旧配置：projection on/off、无 loop context、无 ledger 的 test constructors。
- Build/Lint/Typecheck：`cargo fmt --all -- --check`、`cargo clippy -p ralph-core --all-targets --all-features -- -D warnings`、`cargo build -p ralph-core`、`scripts/check-cli-doc-drift.sh`。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/state/knowledge.rs` | 修改生产文件 | prompt-safe view/render | E10、E13-E15 |
| `crates/ralph-core/src/event_loop/flow_authority.rs` | 修改生产文件 | isolated prompt additive wiring | E10-E11 |
| `crates/ralph-core/src/event_loop/tests/runtime_state_injection.rs` | 修改测试 | prompt acceptance/regression | E11 |
| `crates/ralph-core/data/ralph-tools.md` | 修改 agent-facing 文档 | 解释新只读 block 和停止条件 | E15 |

#### 18. 完成标准

所有 U3 prompt tests、CLI inspect/preview characterization、state regression、fmt/clippy/build/drift 通过；空状态旧 prompt 不变；disabled/ralph/custom/coordinator scope 通过；文档不泄漏内部实现；无 config/preset/CLI 变化；可独立提交。

#### 19. 停止条件

需要改 `RuntimeStateSnapshot` 旧输出、扩大 prompt 路径、修改 inspect CLI schema、改变 disabled stub、读取内部 ledger path/raw events、或现有 prompt characterization 无法在保留断言下通过时停止；更新 D7/D9/D11，不让 Executor 自行扩大范围。

#### 20. 风险与注意事项

- 风险：agent 把 unverified/current observation 当 verified proof。检测：prompt 文案与 status tests；缓解：双维 status + guide 明示；剩余风险是 agent 可能忽略说明，留给 GAP-03 evaluator 处理。
- 风险：新增 block 改变 token/解析顺序。检测：bounded snapshot、block title regression；缓解：仅非空、固定字段、isolated-only。
- 风险：文档误写内部实现。检测：人工按 AI skill guide 规则检查 + `scripts/check-cli-doc-drift.sh`；缓解：只写 agent 下一步动作。

### U4. 完成全链路差分回归并关闭计划

#### 1. Unit 目标

证明启用 GAP-01 后，已有业务行为与关闭/空认知路径一致，且 replay、failure、projection off、非 isolated prompt 都完成回归；本 Unit 只做已实现行为的整合验证和必要的小型修正，不引入新功能。

#### 2. 对应需求与 Scenario

- Requirement：R1-R11
- Scenario：S01-S10
- Decision：D1-D11
- Evidence：E1-E18，以及 U1-U3 已更新的执行证据

#### 3. 外部可观察结果

同一输入在认知功能实际启用与认知状态为空/ledger disabled 的对照下，accepted event tuple、state projection/ledger 业务摘要、recovery outcome/target/attempt、task.resume routing/dedup、watchdog/termination reason、CLI result/exit semantics 一致；差异仅限认知 ledger delta 和有记录的 isolated prompt block。

#### 4. 当前行为基线

当前 run-diagnosis 计划本身定义了 off/on differential 关注的业务字段；本计划采用同样的差分思想，但不依赖其未 merge 代码。已有全量 nextest 两阶段入口是最终 baseline；U4 必须先收集当前 targeted 结果再运行最终门禁。

#### 5. 输入与输出

- 输入：现有 state/event-loop tests、真实 scenario fixtures、projection on/off、isolated/coordinator/custom/ralph、旧 ledger/new knowledge ledger、fault workspace。
- 输出：所有 scenario/test 通过；evidence ledger 追加真实命令结果；计划达到 close。
- 错误：任何业务差异、非预期新文件、未解释 doc drift、snapshot 无解释变化、跳过/only/弱断言都阻塞关闭。
- 状态变化：只允许修改计划内生产/测试/agent guide 文件；不得修改 `.ralph/` runtime state 手工文件。
- 副作用：测试临时目录可清理；仓库不留下 ephemeral artifacts。
- 不变量：用户当前功能可继续运行；未 merge worktree 仍不被直接修改或引用。

#### 6. 修改位置

- `crates/ralph-core/src/state/knowledge.rs`、`state/{mod,commit,snapshot}.rs`、`event_loop/{parse_and_emit,flow_authority}.rs`：仅处理 U1-U3 测试暴露的计划内缺陷；不做 unrelated refactor。
- `crates/ralph-core/src/state/tests.rs`、`event_loop/tests/runtime_state_injection.rs`：补齐 U1-U3 已识别且与验收直接对应的断言；不删除/弱化旧断言。
- `crates/ralph-core/data/ralph-tools.md`：若 drift check 发现命令/引用漂移，只修正计划内新增说明；不添加实现背景。

#### 7. 可依赖能力

- U1 的可回放模型。
- U2 的 accepted observation wiring。
- U3 的 isolated prompt projection 与 agent-facing guide。
- 仓库规定的 `./scripts/run-tests.sh`、nextest、clippy/build/fmt/drift 命令。

#### 8. 禁止依赖的未来能力

- 不等待/操作另一 worktree 的 branch，不 cherry-pick 其实现，不修改 run-diagnosis 代码。
- 不把 U4 变成 GAP-02/GAP-03 实现、preset 迁移、CLI feature 或性能重构。

#### 9. 验收测试

- `cargo nextest run -p ralph-core -- state`。
- `cargo nextest run -p ralph-core -- runtime_state_injection`。
- `cargo nextest run -p ralph-core -- replay_light_integration`。
- `cargo nextest run -p ralph-core --test scenarios`，必须使用真实 workflow guard runner；若 scenario 只覆盖旧 behavior，不新增 source-text-only assertion。
- `cargo nextest run -p ralph-cli --test inspect_prompt`，确认空 ledger/inspect prompt 不发生非预期 CLI drift。
- `./scripts/check-cli-doc-drift.sh`。
- `cargo fmt --all -- --check`、`cargo clippy -p ralph-core --all-targets --all-features -- -D warnings`、`cargo build --workspace`。
- 最终 `./scripts/run-tests.sh`，按仓库两阶段 nextest 入口执行。

#### 10. Acceptance Red

U4 不新增生产行为，Acceptance Red 是先运行一项“有认知观察但业务结果必须不变”的差分断言；在 U2/U3 未完成时，knowledge/prompt 差异断言必须失败但业务 tuple 断言应通过。若 U1-U3 都已 green 却 Red 不是预期差异，停止并更新 Evidence，不得把整个全量失败当作可忽略噪声。

#### 11. 单元测试拆分

- 对照 digest/freshness/status：同输入两次输出稳定；不同 fingerprint 只改 freshness，不改 route。
- 对照 accepted tuple：Business/Recovery/Diagnostic/LoopControl 组合，业务结果不受 knowledge delta 影响。
- 对照 prompt：empty state 字节/块语义保持；non-empty 只多出约定 heading/字段。
- 对照 failure：ledger path-as-directory 只让 knowledge degraded，旧 state rollback 正确。

#### 12. Red → Green → Refactor 顺序

先运行 U1-U3 全部 targeted tests 并记录真实失败 → 只修复对应当前 Unit 的最小实现/测试缺口 → 运行 state/event-loop/CLI targeted tests Green → Refactor 仅限重复 helper、命名和文档可读性 → 运行相邻 replay/scenario → 运行全量脚本 → 若失败，按测试所属 Unit 回退处理，不在 U4 添加未计划行为。

#### 13. 最小实现范围

- 必须：所有计划内 scenario/test/命令通过，业务差分无变化，认知 delta/prompt diff 可解释。
- 必须保持：旧配置、旧 ledger、关闭/空路径、非 isolated 路径、CLI 退出语义。
- 明确不实现：任何未在 R1-R11、D1-D11、U1-U3 文件表中的功能。

#### 14. 集成验证

- 真实联合 core state、event loop、prompt、CLI integration、scenario runner、workspace temp fault。
- 不使用 live API；不修改 `.ralph` 运行时状态文件；不把另一个 worktree 当测试依赖。
- 预期：目标测试、相邻测试、全量两阶段门禁均通过。

#### 15. 风险驱动测试

- Differential：run diagnosis 计划定义的 off/on 业务结果集合全部作为回归字段；依据 E2 与用户“不可导致当前功能不可用”的要求。
- Characterization：已有 prompt/state/replay tests 必须仍通过；依据 E11/E12。
- Fault Injection：persist failure 与 corrupt/old ledger replay；依据 E5/E12。
- No E2E：没有新增用户端/跨服务入口，E2E 成本不能增加与风险不匹配；真实 EventLoop/scenario 已覆盖关键 path。

#### 16. 回归范围

- 直接：`cargo nextest run -p ralph-core -- state`、`runtime_state_injection`、`replay_light_integration`、`--test scenarios`。
- 相邻：`u3_jsonl_emit_gate`、`u11_unified_pipeline_integration`、`transition_ingress_inventory`、`event_policy`、`disposition`、`cargo nextest run -p ralph-cli --test inspect_prompt`。
- 公开消费者：所有 core 依赖 compile、CLI inspect prompt、isolated agent prompt、legacy custom/coordinator/ralph。
- 旧配置/数据：旧 `ledger.jsonl`、无 knowledge delta、`state_projection.enabled=false`、feature false state test、无 loop context。
- 构建目标：`cargo build --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`、`cargo fmt --all -- --check`、`scripts/check-cli-doc-drift.sh`。
- 最终：`./scripts/run-tests.sh`，不得手工改用裸 `cargo test` 代替。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| U1-U3 已列文件 | 仅计划内修正 | 通过最终回归发现的直接缺陷 | E1-E18 + execution evidence |
| `docs/plans/2026-08-13-001-feat-gap01-unified-orchestration-knowledge-state-plan.md` | 修改计划记录 | 追加真实验证结果/剩余风险 | 本 Unit 命令输出 |

不得新增未在 U1-U3 预期文件表中确认的生产/配置/preset 文件；若必须新增，停止并重新评估范围与置信度。

#### 18. 完成标准

所有 Scenario 有通过测试；state/event-loop/prompt/CLI/scenario/全量测试通过；build/lint/typecheck/fmt/drift 通过；无 skipped/only/弱断言；无无解释 snapshot/golden；无手工 `.ralph` 修改；业务 differential 无变化；U1-U4 各自有完整 TDD 证据；计划内 Evidence/Decision 已更新；当前 Unit 可独立提交。

#### 19. 停止条件

任一业务差分字段变化、旧 ledger 无法 replay、projection off 不能运行、non-isolated prompt 被意外注入、全量失败归因不明、需要更改 run-diagnosis worktree、出现新公开调用方、或任一决策低于 0.85 时停止；不得用更新 snapshot、放大 timeout、skip/ignore、删除断言或修改 unrelated preset 让 Green。

#### 20. 风险与注意事项

- 真实风险：ledger 每 batch 多一次全 log 原子重写。触发条件是高事件量长 loop；检测方式是 commit count/targeted performance smoke；缓解是 batch single delta + bounded snapshot，剩余风险记录为后续性能计划，不在 GAP-01 偷渡优化。
- 真实风险：accepted observation 的语义强度被误读成终态证据。触发条件是 agent 直接把 unverified/current 当完成；检测方式是 prompt/guide 审阅；缓解是 status 分离与 GAP-03 明确非目标。
- 真实风险：另一 worktree merge 后与新 `diagnosis` 模块发生命名/模块冲突。触发条件是合并顺序不同；检测方式是 merge 后重新跑 U1-U4 targeted tests；缓解是当前只新增 `state::knowledge` 与 opaque refs，不 import diagnostics 类型。

## 8. Unit 串行依赖图

```text
U1：认知状态数据契约与 replay
  ↓ 只有 U1 的类型、delta、bounds、freshness、replay 全部 Green 后
U2：accepted batch 观察接线
  ↓ 只有 U2 已证明业务 accepted/publish/route 不变后
U3：isolated prompt 只读投影与 agent guide
  ↓ 只有 U3 已证明旧 prompt/path/disabled 行为不变后
U4：差分回归、全量门禁与计划关闭
```

- U2 使用 U1 已验证的 `KnowledgeObserved` delta 和 `KnowledgeObservation` builder；若没有 U1，Executor 会被迫决定 persistence/error 语义，因此不能交换顺序。
- U3 使用 U1 的 safe renderer 和 U2 已实际生成的 ledger records；若提前做 prompt，容易拿 fixture 假数据掩盖 runtime 没有 observation 的问题。
- U4 必须最后执行，因为它比较 U1-U3 组合后的真实业务 tuple；不能提前用不完整实现得出“无回归”。
- 不允许 U1 提前写 EventLoop，U2 提前写 prompt，U3 提前改 CLI/preset，U4 提前实现 GAP-02/GAP-03。

## Verification Contract

### 9. 执行命令清单

| 命令 | 运行时机 | 验证目的 | 预期结果 | 失败时是否可进入下一步 |
|---|---|---|---|---|
| `cargo nextest run -p ralph-core -- knowledge` | U1 每个 Red/Green 后 | knowledge model/freshness/bounds unit | 目标测试通过 | 否 |
| `cargo nextest run -p ralph-core -- state` | U1 close、U2/U4 regression | StateLedger apply/replay/fault/no-op | state 包目标通过 | 否 |
| `cargo nextest run -p ralph-core -- runtime_state_injection` | U2/U3/U4 | accepted wiring、isolated/disabled/ralph/custom prompt | 目标测试通过 | 否 |
| `cargo nextest run -p ralph-core -- event_policy` | U2 | rejection path 不产生 knowledge，policy 原语不变 | 通过 | 否 |
| `cargo nextest run -p ralph-core -- disposition` | U2 | Business/Recovery filter 与现有 classifier 一致 | 通过 | 否 |
| `cargo nextest run -p ralph-core -- replay_light_integration` | U2/U4 | replay/resume 业务路径与认知 persistence | 通过 | 否 |
| `cargo nextest run -p ralph-core --test scenarios` | U4 | 真实 workflow guard/scenario runtime path | 通过；不得用 stub 替代 | 否 |
| `cargo nextest run -p ralph-cli --test inspect_prompt` | U3/U4 | CLI inspect prompt 无非预期 drift | 通过 | 否 |
| `scripts/check-cli-doc-drift.sh` | U3 后/U4 | agent/CLI 文档引用不漂移 | 退出码 0 | 否 |
| `cargo fmt --all -- --check` | 每 Unit close | 格式 | 无 diff | 否 |
| `cargo clippy -p ralph-core --all-targets --all-features -- -D warnings` | U1/U2/U3 close | core lint/type safety | 0 warnings/errors | 否 |
| `cargo build -p ralph-core` | U1/U2/U3 close | core build | 成功 | 否 |
| `cargo build --workspace` | U4 | workspace build | 成功 | 否 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | U4 | workspace lint | 成功 | 否 |
| `./scripts/run-tests.sh` | U4 最终 | 仓库规定的两阶段 nextest + doctest/最终门禁 | 全部通过 | 否 |

不得用 `cargo test -p ralph-cli` 替代 nextest；不得手工运行整个 `cargo nextest run --workspace` 替代项目的 `./scripts/run-tests.sh` 两阶段隔离入口。

### 10. 最终质量门禁

- 所有 S01-S10 有可执行并通过的验收测试；每个 R1-R11 至少有一个测试映射。
- U1-U4 按顺序完成，每个 Unit 都有真实 Acceptance Red、Unit Red、Green、Refactor、Integration、Regression、Close 记录。
- StateLedger old ledger/new delta/replay/duplicate/bounded/failure/feature false 全部通过。
- accepted/rejected/publish/route/termination/recovery/task resume/watchdog/exit semantics 没有非计划差异。
- `state_projection.enabled=false` 仍能处理合法事件；旧 disabled stub 行为保持；empty cognition 不产生额外 prompt block。
- isolated prompt 只追加安全摘要；coordinator/custom/ralph 不被扩大注入；旧 `## ORCHESTRATOR CONTEXT` 字段与 block title 不变。
- `crates/ralph-core/data/ralph-tools.md` 已同步，首次出现的术语、触发条件、agent 动作、字段来源和停止条件清楚；没有内部模块名、ledger 路径、计划编号或 reviewer-only 细节。
- `cargo fmt`、core/workspace clippy、core/workspace build、`scripts/check-cli-doc-drift.sh`、`./scripts/run-tests.sh` 全部通过。
- 没有新增 skip/ignore/only，没有删除/弱化断言，没有无解释 snapshot/golden 更新，没有手工 `.ralph` runtime state 变更，没有 ephemeral 文件进入 git。
- 没有 import 未 merge 的 run-diagnosis 类型；与该 worktree 的兼容只通过 opaque source ref/fingerprint 保持。
- 所有关键 Decision 置信度仍 `>=0.85`；没有未处理 BLOCKED decision；剩余性能/语义风险已明确留给后续 GAP-02/GAP-03 或独立性能计划。

## Definition of Done

- [x] U1 的 bounded knowledge model、freshness、idempotency、additive delta、replay、failure rollback、feature false no-op 已通过。
- [x] U2 只在 post-validation accepted Business/Recovery batch 记录观察；rejected/diagnostic/control 不进权威 cognition；业务结果差分无变化。
- [x] U3 isolated prompt 的新摘要是非空才出现、只读、bounded、redacted；旧 prompt 路径与 disabled 行为不变；agent-facing guide 已同步。
- [x] U4 完成 state/event-loop/prompt/CLI/scenario/全量回归、workspace build、core clippy、CLI 文档漂移检查和最终 nextest 门禁；仓库既有 fmt 漂移未扩大处理范围。
- [x] 计划内 Evidence Ledger 已追加真实命令结果；没有把未 merge worktree 当作当前代码事实。
- [x] 实际文件变更未超出 U1-U3 预期文件表；知识接线因 5000 行硬限制提取到独立 event-loop 模块，并已重新验证。
- [x] 计划不引入新配置、preset、event schema、CLI、外部依赖或新的持久化 authority。
- [x] 用户当前编排功能仍可用：任何认知状态缺失或持久化故障均只能降级 cognition，不得阻塞既有业务流程。

### 实施后对抗性复核记录

- 修复 freshness 自比较：prompt/view 现在必须接收当前 loop/plan fingerprint；旧记录在 fingerprint 不一致时显示 `stale`，缺失时显示 `unknown`。
- 修复 observation id 稳定性：改为带版本域分隔的 SHA-256，不依赖进程/编译器的 `DefaultHasher` 行为。
- 加固 observation id 规范化：topic/source/digest 使用长度前缀编码，避免 NUL/分隔符构造字段边界歧义。
- 修复 source_ref 绕过路径：builder、snapshot insert 与 `StateLedger::commit` 的 `KnowledgeObserved` 边界均执行清理和长度限制，防止公开 wire 字段绕过 prompt scrubber 后进入持久化状态。
- 补齐跨平台路径防御：prompt/storage scrubber 现在覆盖 Unix、Windows drive 和 UNC 路径，并有对应回归测试。
- 收紧 verification：普通 builder 不再暴露 verification setter；accepted observation 保持默认 `Unverified`，读取通过只读 accessor 完成。
- 修复单文件规模：`parse_and_emit.rs` 从 5014 行降至 4970 行，知识提交接线位于 `event_loop/knowledge_wiring.rs`。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 已按真实文件/符号/调用边界拆成 U1-U4，每个 Unit 有 Red、Green、命令、文件和完成标准 |
| Executor 是否仍需做关键设计决策 | 否 | D1-D11 已固定 authority、边界、错误语义、上限、测试层级和兼容策略 |
| 所有文件和接口是否有代码库证据 | 是 | 现有路径均由 E4-E18 确认；新增 `state/knowledge.rs` 明确标记为新文件 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1-D11 最低 0.88；执行中下降必须停止重决策 |
| 是否存在未处理的低置信度假设 | 否 | Product Contract 已列出待验证假设；没有影响正式路径的未决项 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1=state contract/replay，U2=accepted observation，U3=isolated projection，U4=integrated regression gate |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 都列出了测试入口、Red、命令、集成与回归范围 |
| 每个 Unit 是否有真实 Red | 是 | U1/U2/U3 明确了目标能力缺失时的 Red；U4 使用差分 Red，不把环境失败当 Red |
| 每个 Unit 是否包含回归范围 | 是 | 每个 Unit 第 16 节列出 direct/adjacent/public/legacy/build/lint/typecheck |
| 是否存在未来 Unit 依赖 | 否 | 依赖图仅线性前置依赖；禁止提前实现后续行为 |
| 是否存在泛化任务描述 | 否 | 每项均指定对象、输入、输出、错误、副作用、命令与完成断言 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 5 节、6 节和 U1-U4 对应关系完整 |
| 所有关键决策是否有 Evidence | 是 | D1-D11 均列 E-ID，E-ID 来源均为当前代码/测试/配置/历史 |
| 计划是否可以严格串行执行 | 是 | U1 → U2 → U3 → U4，无并行单元 |

本计划没有把 run-diagnosis 另一个 worktree 的实现当作前置条件，因此可以在当前基线执行；如果该计划先 merge，仍需按 U1→U4 重新跑 targeted/full regression，而不是跳过兼容验证。

## Appendix A：与未 merge 的 run-diagnosis 计划的兼容边界

- 当前计划只消费稳定的语义字段：`producer`、`source_ref`、`input_fingerprint`、`freshness`、`verification_status`；不依赖 `DiagnosisInputBundle`、`RuntimeTraceEntry` 或 `FeedbackEntry` 类型。
- `source_ref` 可以在未来指向诊断 trace/feedback 的稳定逻辑引用，但当前 runtime 使用 `accepted-event:<iteration>:<batch-index>:<observation-id>` 形式，不把内部绝对路径写进 prompt。
- run-diagnosis 计划的 observer-only 约束与本计划的 D4 一致：诊断/认知记录故障不能改变 business acceptance、route、recovery、retry、termination 或 exit semantics。
- 两个计划 merge 后的最小验证顺序：先确认 `git status`/编译无模块冲突，再运行 U1 state replay，随后 U2 accepted path、U3 prompt、U4 full gate；发现 sidecar 需要直接成为 cognition authority 时，停止并另立计划，不在本计划里扩张。
