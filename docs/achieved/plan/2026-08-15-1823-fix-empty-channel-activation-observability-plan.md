---
title: "fix: 空 channel activation 中间产物与诊断识别"
date: 2026-08-15
type: fix
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-brainstorm
execution: code
---

# fix: 空 channel activation 中间产物与诊断识别

## Goal Capsule

- **目标：**为 isolated hat activation 写入一条结构化、可审计的 activation outcome 记录，使空 channel 能与后端失败、watchdog timeout、用户中断、channel 路由失败、agent 成功但未 emit 区分；让 `ralph-run-diagnosis` 消费这条记录并在证据不足时明确输出 `unknown`，而不是把所有结果归为 generic stall。
- **权威边界：**本计划只增加 observability 和诊断 skill 的消费规则；不改变 `task.resume` 的生成、目标 hat、retry key、重试上限、`plan.blocked` 或既有 missing-terminal recovery 语义。
- **执行 profile：**严格按 Unit 1 → Unit 2 → Unit 3 → Unit 4 串行执行。每个 Unit 必须完成 Acceptance Red → Unit Red → Green → Refactor → Integration → Regression → Close 后才能进入下一 Unit。
- **停止条件：**实际调用链与本计划证据不符、无法保留旧 recovery 行为、需要新依赖、或任一实现决策的置信度低于 0.85 时停止并重新取证；不得让 Executor 临时改变字段契约或诊断分类。
- **Tail ownership：**Coding Agent 负责按本计划实现和验证；本计划不写生产代码；最终验收由第 10 节质量门禁负责。

## 0. 计划状态

**READY。**所有进入实施范围的关键技术决策均已用当前源码、现有测试/reader 行为或用户已确认的范围取证，D1-D9 置信度均不低于 0.85。READY 只表示计划可以交给 Coding Agent 执行，不表示功能已经实现。

- **代码库基线：**分支 `pittcat-dev`，HEAD `015782c5`（2026-08-15 调查时）；计划文件本身是本次唯一新增文件。
- **调查范围：**isolated hat channel 创建/merge/interrupt、loop runner execution outcome、PTY/CLI adapter result、runtime trace logger、core diagnosis reader、`ralph diagnose` integration tests、repo-local `ralph-run-diagnosis` skill/references/tests、相关历史计划和 workspace 测试入口。
- **已执行的只读调查命令：**`git rev-parse --short HEAD`、`git status --short`、`rg --files`、`rg -n`、`sed -n`、`git log --oneline --all -- <相关路径>`；另使用 ce-plan context fence 读取仓库上下文和计划规范。
- **本轮未执行：**没有运行 Rust build/lint/nextest、Python pytest、真实 `ralph run` 或 `ralph diagnose` workflow；ce-plan 阶段只做源码、测试、配置和历史取证。第 9 节命令清单是 Coding Agent 的验证契约。
- **外部研究：**未使用网络资料；本需求的实现入口、数据结构、测试和历史证据均已在仓库内确认，外部资料不能替代这些直接证据。
- **阻塞项：**无。当前计划不要求 Executor 在实现时决定新 sidecar、trace schema、merge API、分类层或 recovery 语义。

## Product Contract

### 1. 功能目标

#### 1.1 业务目标与调用方

调用方是运行 Ralph loop 的 operator，以及执行 `ralph-run-diagnosis` 的诊断 agent。目标是回答一个目前无法由中间产物可靠回答的问题：某个 hat 的 activation 结束时 channel 为空，究竟是后端没有正常结束、watchdog/中断、channel 路由失败，还是后端正常返回但 agent 没有产生任何可消费事件。

当前问题不是“恢复没有发生”。现有 `task.resume`/missing-terminal 路径已经承担恢复；缺失的是 activation 结束时的原始事实和后续诊断消费。计划不会把诊断责任重新放进 `task.resume`，也不会让 `ralph diagnose` 自动执行恢复。

#### 1.2 当前行为

- `prepare_hat_channel` 在 activation 开始时用 `fs::File::create` 创建零字节 channel。此时空文件是正常初始化状态，不能作为故障证据。
- isolated runner 在 backend 返回后先读取 channel metadata，再调用 `merge_hat_channel`。
- `merge_hat_channel` 读取到空内容时写 `.ralph/diagnostics/channel-routing-fallback-*.md`，删除 channel，返回错误；runner 随后只写 `merge_hat_channel_failed` fallback 日志，并基于预读的 `0` 字节继续现有 empty-terminal recovery 判断。
- 在 merge 成功且预读 channel 为零字节的分支，runner 才写一条普通 warning，包含 `backend_success`、`watchdog_timeout`、termination、output bytes 和 output 是否提到 emit；空 channel merge 失败分支没有同等结构化事实。
- `ExecutionOutcome` 不包含 `exit_code`，而 `PtyExecutionResult` 与 adapter `ExecutionResult` 都已有 `exit_code`；runner 当前转换时丢弃该字段。
- `ralph-run-diagnosis` 的 bundle-first 流程已经读取 `diagnosis-input.json`、`runtime-trace.jsonl`、`feedback.jsonl`，但 references 没有定义 activation outcome 记录的字段、分类和报告落点。

#### 1.3 目标行为与行为差异

- 在 runtime diagnosis artifacts 可用时，每个 isolated activation 在 channel merge/中断处理完成后追加一条 `runtime-trace.jsonl` 记录，`phase=activation`、`kind=hat_activation_outcome`；记录只包含有限的结构化事实，不写完整 backend 输出或 prompt。
- 空 channel 的记录必须保留 channel 在 merge 前的可观察状态，即使 `merge_hat_channel` 已删除空文件；至少能区分 `empty`、`missing`、`unreadable` 和 `merge_failed`。
- PTY 和 CLI adapter 的 `exit_code` 必须进入 `ExecutionOutcome`，以便记录后端进程是否非零退出；这只是事实传递，不改变 `success`、timeout、termination 或 recovery 分支。
- `ralph-run-diagnosis` 必须先读新记录，再把它与 events、recovery journal、fallback markdown 和现有 trace 对账，按明确优先级输出 activation diagnosis。若证据不完整，输出 `unknown`/证据缺口，不得凭空判定 agent 根因。
- `ralph diagnose` 的现有 runtime-trace 汇总保持兼容；本次不新增 CLI 命令，不改变 `ralph diagnose` 的退出码，不让 CLI reporter 复制一套 activation classifier。

#### 1.4 输入、输出、状态变化、副作用和错误语义

- **输入：**isolated activation 已存在的 hat、loop id、iteration、channel 路径及 merge 前 metadata；已有 `ExecutionOutcome` 的 success/exit code/watchdog/termination/output；已有 event processing 统计（candidate、accepted、rejected、wave policy rejection）；已有 terminal obligation 配置。
- **输出：**`.ralph/diagnostics/<session>/runtime-trace.jsonl` 中新增 `hat_activation_outcome` JSONL 行；报告模板增加 activation outcome 表；fallback markdown 和现有 recovery journal 保持原样。
- **记录字段：**固定使用 `phase=activation`、`kind=hat_activation_outcome`、`hat`、`ref`/`source_ref`、`status` 和 bounded `fields`。`fields` 至少包含 `loop_id`、`channel_exists`、`channel_bytes`（未知时为 null）、`channel_readable`、`merge_succeeded`、`backend_success`、`backend_exit_code`、`watchdog_timeout`、`backend_termination`、`output_bytes`、`output_mentions_emit`、`candidate_event_count`、`accepted_event_count`、`rejected_event_count`、`wave_policy_rejection_count`、`terminal_obligation_topics`。列表字段只保留 bounded topic 字符串，禁止写完整输出内容。
- **status 取值：**`merged`、`empty`、`missing`、`unreadable`、`merge_failed`、`interrupted`。`empty` 只表示 merge 前 channel metadata 明确为存在且零字节；它不单独等价于 agent 根因。
- **状态变化：**只新增 runtime-trace sidecar 行和诊断报告中的投影，不修改 main events、recovery journal、task state、retry counter、terminal state 或 workspace code。
- **错误语义：**runtime trace 写入继续遵守现有 best-effort 语义；写入失败只能让 trace degraded 并产生已有 warning，不能阻塞 loop。channel merge 的原有成功/失败语义不变。诊断 skill 读取不到新行时必须声明旧/降级证据模式，不得把缺失记录判作“没有发生空 channel”。
- **兼容性：**沿用 `run-diagnosis-trace/v1` 的 additive fields 兼容策略；旧 trace 行没有新 `kind` 时仍由现有 reader 接受。runtime diagnosis 未启用时不创建新 sidecar、不改变既有 artifact 数量。
- **性能：**每次 activation 最多追加一条 bounded JSONL 行；不保存完整 backend 输出，不增加网络、模型调用、数据库或无界重试。
- **安全与权限：**`ref` 只写 workspace-relative channel path 或现有短引用；不得把 prompt、secret、完整 stderr 或 backend 输出写入 trace；诊断 skill 仍然只读和 non-executing。

#### 1.5 本次范围与非目标

**范围：**runtime trace 的 activation outcome 记录；adapter exit code 传递；isolated normal/merge-error/interrupt 三条调用路径的观测；`ralph-run-diagnosis` 主 skill 和 references 的读取、分类、报告契约；已有 runtime/diagnosis/skill contract 测试。

**非目标：**不修改 `task.resume` 生成或消费；不重构 `merge_hat_channel` 的业务返回语义；不新增诊断数据库或单独 sidecar；不让 `ralph diagnose` 自动归因或执行修复；不改变 `MissingEventGate`、stall detector、retry budget、`plan.blocked`、preset topology、agent prompt 或 `crates/ralph-core/data/*.md`；不为某个 preset、hat 名称或单次事故写特判。

**已确认假设：**现有 `RuntimeTraceEntry.fields` 和 logger cap 足以承载上面的 bounded scalar/object 字段；现有 core diagnosis reader 已按通用 `schema_version/ts/iteration/sequence/phase` 接受任意 `kind`，因此不需要新增 trace schema version 或 reader 分支。

**待验证假设：**无影响正式实施方向的待验证假设。若实现时发现 event processing 统计在 interrupt 路径不可用，必须按 U2 停止条件保留 `null/0` 和 `interrupted` 原始事实，不能临时推断业务结果。

## 2. 代码库现状与证据

### 2.1 当前实现入口

外部运行入口是 `crates/ralph-cli/src/loop_runner/inner.rs` 的单 iteration backend 执行链：adapter 执行 → `ExecutionOutcome` → output/event processing → isolated channel snapshot → `merge_hat_channel` → missing-terminal/recovery 判断。正常中断入口是 `crates/ralph-cli/src/loop_runner/entry.rs::merge_isolated_channel_on_interrupt`，它复用同一个 merge helper。

核心观测边界是 `crates/ralph-core/src/diagnostics/runtime_trace.rs::RuntimeTraceEntry` 与 `RuntimeTraceLogger`。logger 已通过 `DiagnosticsCollector::log_runtime_trace` 暴露 best-effort 写入入口，并把记录写到现有 `runtime-trace.jsonl`；没有新文件或新依赖的必要。

诊断消费边界是 repo-local `skills/ralph-run-diagnosis/SKILL.md` 及其 `references/`。现有 `skills/tests/test_run_diagnosis_contract.py` 以结构化/词法契约测试 bundle-first 顺序、legacy fallback、报告 frontmatter 和 non-executing 约束；新契约应扩展这份已有测试，而不是新造第二套 skill test harness。

构建和验证必须遵守仓库 AGENTS 规则：Rust 测试使用 `cargo nextest run` 系列或最终 `./scripts/run-tests.sh`；Python skill tests 使用 `.venv`；不能用裸 `cargo test -p ralph-cli`。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `crates/ralph-cli/src/loop_runner/hat_channel.rs::prepare_hat_channel`, `merge_hat_channel` | activation 开始即创建零字节 channel；merge 时空内容才被判为异常并删除、写 fallback、返回错误 | 观测点必须在 backend 返回后的 channel-close，而不是 prepare；必须在删除前保存 metadata | 高 |
| E2 | `crates/ralph-cli/src/loop_runner/inner.rs` isolated merge 分支 | 空 channel 的结构化 warning 只在 merge 成功分支；merge error 分支只有泛化 `merge_hat_channel_failed` | 现有空 channel 失败路径缺少 backend/channel/event facts，是本次直接修复对象 | 高 |
| E3 | `crates/ralph-cli/src/loop_runner/inner.rs::immediate_missing_terminal_emit` | empty channel 之后已有责任 hat-specific missing-terminal recovery；条件包含 success、非 supervisor、无 valid/rejected candidate 和 terminal obligation | 本计划只记录恢复前事实，不改 task.resume、目标 hat、retry 或 blocked 行为 | 高 |
| E4 | `crates/ralph-adapters/src/pty_executor.rs::PtyExecutionResult`, `crates/ralph-adapters/src/cli_executor.rs::ExecutionResult` | 两个 adapter result 已有 `exit_code: Option<i32>` | `ExecutionOutcome` 应传递已有字段，不能重新执行进程或添加 adapter API | 高 |
| E5 | `crates/ralph-cli/src/loop_runner/execution.rs::ExecutionOutcome` 及 inner 两处构造 | runner outcome 当前没有 exit code，PTY/CLI 转换处丢弃该字段；测试有直接 struct literal | U2 必须更新两个生产构造和已确认的 `termination.rs` fixture，保证编译和事实完整 | 高 |
| E6 | `crates/ralph-core/src/diagnostics/runtime_trace.rs` | schema 是 `run-diagnosis-trace/v1`，`kind` 与 optional `fields` 已存在；logger 对 fields 做 8 KiB cap，写入失败 degraded 且不阻塞 runtime | 新 outcome 采用 additive kind/fields，复用 logger cap，不 bump schema、不新增依赖 | 高 |
| E7 | `crates/ralph-core/src/diagnosis/bundle.rs::read_runtime_trace_report` 和 `is_runtime_trace_record` | reader 按通用结构校验并统计记录，不限制具体 kind/fields；报告只展示 summary | CLI reader 可保持兼容；分类放在 run-diagnosis skill，避免重复两套归因逻辑 | 高 |
| E8 | `skills/ralph-run-diagnosis/SKILL.md` §0.2、`references/artifact-discovery.md`、`verification-pipeline.md`、`report-template.md` | skill 已 bundle-first 读取 runtime trace，但没有 activation outcome 字段、分类和报告表 | U3 必须在现有 bundle-first/L0-L7 流程内增加识别规则，不另建诊断入口 | 高 |
| E9 | `skills/tests/test_run_diagnosis_contract.py` | 已有测试锁定 sidecar inventory、bundle-first、frontmatter 和 non-executing | U3/U4 必须扩展该测试的 stable anchors，不能只改文档不验证 | 高 |
| E10 | `docs/achieved/plan/2026-08-08-003-fix-empty-hat-channel-retry-plan.md` 的 deferred follow-up | 既有空 channel retry 修复明确把“更丰富的 activation outcome 分类”留作后续 observability 工作 | 本计划承接已明确的后续范围，但不重新设计 recovery | 中高 |
| E11 | `Cargo.toml`, `mise.toml`, `scripts/run-tests.sh`, repo AGENTS instructions | workspace 已有 serde/serde_json/chrono/tempfile，nextest 版本和两阶段测试入口已固定 | 不加依赖；每个 Unit 和最终门禁使用真实仓库命令 | 高 |
| E12 | 当前 `git rev-parse --short HEAD` = `015782c5`；当前工作树状态检查未显示待处理文件 | 计划基于该代码基线；写计划不应覆盖其他用户变更 | Executor 开始前需重新检查 status，若出现非本计划变更按停止条件处理 | 高 |

### 2.3 受影响范围

- **生产模块：**`crates/ralph-cli/src/loop_runner/execution.rs`、`inner.rs`、`entry.rs`、`hat_channel.rs`；只修改 execution outcome 事实传递和 runtime trace 调用，不改 recovery 判定。
- **Core diagnostics：**`crates/ralph-core/src/diagnostics/runtime_trace.rs` 仅在确有必要时补充稳定测试/构造辅助；`crates/ralph-core/src/diagnosis/bundle.rs` 应保持解析行为并增加兼容测试，不新增 classifier。
- **诊断 skill：**`skills/ralph-run-diagnosis/SKILL.md` 及已确认的 references：`artifact-discovery.md`、`verification-pipeline.md`、`report-template.md`、`mechanism-checklist.md`、`source-trace-guide.md`、`confidence-rubric.md`、`examples/minimal-diagnostics-layout.md`。
- **测试模块：**`crates/ralph-cli/src/loop_runner/hat_channel.rs` 内已有测试、`crates/ralph-cli/src/loop_runner/tests/legacy/diagnosis.rs`、`recovery.rs`、`termination.rs`；`crates/ralph-core/src/diagnostics/integration_tests.rs`、`diagnosis/tests.rs`；`crates/ralph-cli/tests/diagnose.rs`；`skills/tests/test_run_diagnosis_contract.py`。
- **数据边界：**已有 `.ralph/<diagnostics-session>/runtime-trace.jsonl`、已有 channel fallback markdown、已有 main events/recovery artifacts；不新增数据库/服务/API。
- **调用方：**isolated normal iteration、isolated merge error、interrupt merge；非-isolated shared channel 运行不产生该 isolated outcome 行。
- **构建目标：**`ralph-cli`、`ralph-core`、`ralph-adapters` 及 workspace 全量 nextest/build/lint。

## Planning Contract

### 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | 中间产物存放在哪里 | 新建 activation sidecar；写入 main events；复用 `runtime-trace.jsonl` | 复用现有 `runtime-trace.jsonl`，新增 `phase=activation/kind=hat_activation_outcome` 行 | E6、E7 | 新 sidecar 会增加 manifest/读取/生命周期；main events 会污染业务账本并改变事件语义；现有 trace 已有 phase/kind/fields/cap | 0.97 |
| D2 | 空 channel 何时取证 | `prepare_hat_channel` 创建时；merge 前后；只在 recovery 后 | backend 返回后、merge 调用前 snapshot metadata，并在 merge/interrupt 处理后落一条 outcome | E1、E2、E3 | prepare 时零字节是正常状态；删除后再读无法保留空 channel 事实；recovery 后取证可能混入下一轮状态 | 0.97 |
| D3 | 是否改变 `merge_hat_channel` API | 改为结构化 Result；新增第二套 merge API；保留 `Result<()>`，由已存在 runner snapshot 生成 outcome | 保留 `merge_hat_channel` 的 `Result<()>` 和删除/fallback 语义；runner 使用已有 snapshot + merge result 生成记录 | E1、E2、已有 call sites/test call sites | 改 API 会扩大 entry/inner/test 影响面，且 merge 业务返回不是本需求；现有 runner 已有 merge result 和 bytes snapshot | 0.91 |
| D4 | 如何得到进程失败证据 | 从 output 文本猜测；重新访问 adapter；把 exit code 单独写到另一个文件；贯通已有 `exit_code` | 给 `ExecutionOutcome` 增加 `exit_code: Option<i32>`，从 PTY/CLI result 原样传递到 outcome trace | E4、E5 | 文本猜测不可靠；重新执行不可能；新文件重复事实；字段已在 adapter 边界存在 | 0.98 |
| D5 | 诊断归因放哪一层 | core `ralph diagnose` classifier；runtime 直接写根因；`ralph-run-diagnosis` 消费原始事实并对账 | runtime 只写 raw facts；`ralph-run-diagnosis` 在 bundle-first 流程中分类，并以 events/recovery 交叉验证 | E3、E7、E8、现有 skill confidence rubric | runtime 不应推断 agent 根因；CLI summary 当前只有通用 trace summary；skill 已是 post-run deep diagnosis 入口 | 0.94 |
| D6 | 是否 bump trace schema | bump 到 v2；保持 v1 additive；新增独立 schema | 保持 `run-diagnosis-trace/v1`，只新增可选 kind/fields，并保留旧行兼容 | E6、E7 | 当前 schema 明确 non-additive 才 bump；旧 reader 对 kind 通用接受 | 0.96 |
| D7 | 是否记录完整 output 以帮助判断 | 保存全文；保存 excerpt；保存 bounded scalar facts | 只记录 `output_bytes` 和已有 `output_mentions_emit`，不保存内容 | E6、现有 8 KiB cap、权限要求 | 全文/excerpt 可能泄露 prompt/secret、产生大字段和不稳定诊断；scalar 已足够区分主要机制分支，剩余内容由已有 artifacts 对账 | 0.93 |
| D8 | 恢复语义是否同步改造 | 重写 task.resume；让 diagnosis 触发恢复；只增加 trace/skill 识别 | 不改 task.resume、retry、blocked；只在 recovery 旁边补 activation evidence | E3、E10 | 用户已明确恢复行为不需要本次解决；改变恢复会扩大风险并混淆验证目标 | 0.99 |
| D9 | `ralph diagnose` 是否增加详细 activation projection | 改 core report JSON/Markdown；只保持 generic summary，skill 原始读取；新 CLI 命令 | 保持 CLI generic summary，skill 读取 raw `runtime-trace.jsonl` 并在自身报告增加表 | E7、E8、现有 report/template 分层 | CLI 不是本次 post-run classifier；改 public report schema 会扩大调用方和 compatibility 面；skill 已能访问 session artifacts | 0.90 |

以上决策均达到 0.85；没有低置信度关键决策进入 Unit。D9 虽然是最接近阈值的决策，直接依据是 reader 的 generic validation、skill 的 bundle-first raw artifact 读取和用户明确的“存好中间产物，最后 diagnosis skill 识别”目标；若实现中发现 skill 无法获取 raw trace，必须在 U3 停止并重新评估是否需要 CLI projection。

### 3.1 实现约束

1. outcome row 必须在同一 activation 的 merge/interrupt 路径结束后写入；不得在 `prepare_hat_channel` 写“空 channel”故障记录。
2. 空 channel 的 `status=empty` 必须由 merge 前 `metadata.len()==Some(0)` 证明；`channel_path` 不存在、metadata 失败、read 失败分别使用 `missing`/`unreadable`，不能折叠为 empty。
3. 正常 merge、empty merge error、non-empty merge error、interrupt 都必须覆盖；非-isolated 运行不加入虚假的 isolated channel outcome。
4. 记录失败不能改变 loop 结果；必须继续使用现有 `DiagnosticsCollector::log_runtime_trace` best-effort 入口。
5. 新字段不得包含完整输出、prompt、stderr、secret、绝对路径或不受限 topic 数组。
6. `ralph-run-diagnosis` 的分类必须以 raw activation row 为入口，以 events/recovery/fallback 为交叉证据；不能只根据 `backend_success=true` 宣称 agent 未 emit。

## Product Contract: BDD 行为规格

### Feature: isolated hat activation 的空 channel 可观测性

  **Background:**
    Given runtime diagnosis artifacts are enabled
    And an isolated hat activation has a per-hat channel
    And the existing recovery and merge semantics are unchanged

  **Scenario S1: backend 成功但 terminal-obligation activation 留下空 channel**
    Given the backend returns success
    And the pre-merge channel exists with zero bytes
    And the hat has a terminal obligation
    When the activation closes and the channel merge path finishes
    Then `runtime-trace.jsonl` contains one `hat_activation_outcome` row with `status=empty`
    And the row records backend success, channel bytes zero, channel path reference, and terminal obligation topics
    And the existing missing-terminal recovery outcome is unchanged

  **Scenario S2: backend 非零退出且 channel 为空**
    Given the adapter returns a non-zero exit code
    And the pre-merge channel exists with zero bytes
    When the activation closes
    Then the outcome row preserves `status=empty` and the non-zero `backend_exit_code`
    And diagnosis can classify backend failure without treating it as successful no-emit
    And no task.resume or retry behavior changes

  **Scenario S3: watchdog timeout or termination 与空 channel**
    Given the backend is marked watchdog timeout or terminated
    And the channel is empty or incomplete
    When the activation closes
    Then the outcome row records timeout/termination facts
    And diagnosis gives timeout/termination precedence over successful-no-emit classification

  **Scenario S4: non-empty channel 正常 merge**
    Given the pre-merge channel contains at least one line
    When merge succeeds
    Then the outcome row has `status=merged` and a positive or known byte count
    And accepted/rejected event processing remains unchanged
    And the row does not classify the activation as empty

  **Scenario S5: channel 缺失、不可读或 merge 写入失败**
    Given the channel is missing, metadata/read fails, or a non-empty merge cannot write the target
    When the activation closes
    Then the outcome status distinguishes `missing`, `unreadable`, and `merge_failed` when the raw fact is available
    And diagnosis reports a channel/routing evidence category rather than successful-no-emit

  **Scenario S6: 用户中断路径**
    Given an isolated activation is interrupted before normal iteration completion
    When `merge_isolated_channel_on_interrupt` handles the channel
    Then one outcome row records `status=interrupted` and available channel/backend facts
    And the existing interrupt fallback and termination behavior remain unchanged

  **Scenario S7: diagnostics disabled**
    Given runtime diagnosis artifacts are disabled
    When an isolated activation closes with an empty channel
    Then no new runtime trace sidecar is created
    And existing channel fallback, recovery, and loop behavior remain unchanged
    And diagnosis tooling reports the evidence as unavailable rather than inferring a cause

  **Scenario S8: diagnosis skill joins raw outcome with recovery and events**
    Given a session contains a `hat_activation_outcome` row, event ledger, and recovery journal
    When `ralph-run-diagnosis` runs its bundle-first workflow
    Then the report contains an activation outcome table with raw facts, correlated artifacts, classification, and confidence
    And a backend failure, timeout, channel failure, attempted-but-rejected event, and successful-no-terminal-emit are distinguished
    And missing corroboration is reported as an evidence gap or `unknown`
    And the skill does not execute recovery commands

## Verification Contract: 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
|---|---|---|---|---|---|
| S1 | trace 有 exactly one `hat_activation_outcome`、`status=empty`、bytes=0、hat/ref/terminal topics；recovery/event ledger 不变 | `crates/ralph-cli/src/loop_runner/hat_channel.rs` 现有 empty-file test + `crates/ralph-cli/src/loop_runner/tests/legacy/diagnosis.rs`/`recovery.rs` | CLI loop-runner integration/unit seam | Characterization：固定当前 fallback markdown 和 recovery 结果；bounded-field assertion | 否 |
| S2 | trace 保留 adapter non-zero exit code；诊断证据可分 backend failure；不得新增 retry | `crates/ralph-cli/src/loop_runner/tests/legacy/termination.rs` 既有 `ExecutionOutcome` literal 与 execution conversion tests | unit + loop-runner integration | Contract：PTY/CLI 两 adapter exit code propagation | 否 |
| S3 | timeout/termination flags 出现在 outcome；classifier 不归为 success/no-emit | `crates/ralph-cli/src/loop_runner/inner.rs` 相关 diagnosis tests、existing watchdog tests | integration | Fault characterization：现有 watchdog trace 与 termination trace 对照 | 否 |
| S4 | non-empty merge 是 `merged`，事件内容/hat stamping/triggered backfill 与既有断言一致 | `hat_channel.rs` existing non-empty merge tests | unit/integration | Regression：malformed-line preservation、policy rejected path | 否 |
| S5 | missing/unreadable/non-empty write failure 不被写成 `empty` | `hat_channel.rs`/`legacy/diagnosis.rs` 的 channel failure seam；若当前无 write-failure seam，先用现有 fake path backend/fixture 证明入口 | integration | Fault injection：只注入文件边界错误，不 mock merge 业务 | 否 |
| S6 | interrupt outcome 可见，fallback/termination unchanged | `crates/ralph-cli/src/loop_runner/entry.rs` 调用路径及 `legacy/recovery.rs` interrupt tests | integration | Characterization：保留现有 interrupt event integrity | 否 |
| S7 | diagnostics disabled 不产生 trace 且旧行为绿 | existing diagnostics mode tests + targeted CLI loop tests | integration | Compatibility：disabled/default path | 否 |
| S8 | skill references 明确读取、分类、置信度和 non-executing；contract tests 通过 | `skills/tests/test_run_diagnosis_contract.py` | repository skill contract test | Synthetic fixture/anchor test；不调用 LLM、不执行 ralph | 否 |

### 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | isolated activation close 记录结构化 outcome | S1, S4 | `hat_activation_outcome` row assertions | runtime-trace serialization/fields cap | loop-runner diagnosis tests | 否 | E1,E2,E6 |
| R2 | 空 channel 分类保留 raw channel facts | S1,S2,S5 | status/bytes/existence/readability assertions | outcome field construction | empty/failed merge tests | 否 | E1,E2 |
| R3 | adapter exit code 贯通 | S2,S3 | PTY/CLI exit code appears in row | `ExecutionOutcome` construction | runner propagation tests | 否 | E4,E5 |
| R4 | existing recovery semantics unchanged | S1,S2,S3,S6,S7 | existing recovery/termination assertions remain | no new recovery API | legacy recovery/interrupt tests | 否 | E3,E10 |
| R5 | trace schema backward compatible and bounded | S4,S7 | old rows read; new fields capped | runtime trace tests | diagnosis bundle reader tests | 否 | E6,E7 |
| R6 | diagnosis skill reads and classifies activation evidence | S8 | report/template contract anchors | n/a | Python skill contract tests | 否 | E8,E9 |
| R7 | evidence incomplete yields unknown/gap | S5,S7,S8 | no unsupported root-cause claim | n/a | skill contract + synthetic evidence fixture | 否 | E7,E8 |
| R8 | interrupt path is observable | S6 | interrupted row and unchanged fallback | outcome field edge test | interrupt recovery tests | 否 | E1,E2 |

## Implementation Units

### 串行顺序

Unit 1
  ↓ 完成 trace contract、Red/Green、集成与回归
Unit 2
  ↓ 完成 runner/adapter observation、Red/Green、集成与回归
Unit 3
  ↓ 完成 diagnosis skill contract、Red/Green、集成与回归
Unit 4

### Unit 1：固定 activation outcome 的 trace 兼容契约

#### 1. Unit 目标

让现有 `runtime-trace.jsonl` 能承载并兼容读取 `phase=activation/kind=hat_activation_outcome` 的 bounded raw facts；不接入 runner，不改变任何 runtime decision。

#### 2. 对应需求与 Scenario

- Requirement：R1、R2、R5、R7
- Scenario：S1、S4、S5、S7
- Decision：D1、D6、D7
- Evidence：E6、E7、E11

#### 3. 外部可观察结果

给定一个新 outcome entry，runtime trace logger 会按既有 schema 写入、cap fields、递增 sequence；给定旧 trace 行，core reader 仍报告 valid/present；fields 超过 cap 时不会把完整 path/output 写入磁盘。

#### 4. 当前行为基线

当前 `RuntimeTraceEntry` 已支持 `phase`、任意 `kind`、optional `fields`，但没有 stable outcome field contract 测试。当前 core reader 只做 generic record validation。先添加 characterization/contract tests 固定该兼容行为，再由最小 schema contract 扩展保护。

#### 5. 输入与输出

- 输入：`RuntimeTraceEntry::new(iteration, sequence, Activation)` 加 outcome kind、hat/ref/status、bounded fields。
- 输出：可序列化 JSONL row；旧 row 仍可读取；sequence 和 schema version 不变。
- 错误：invalid/malformed row 仍按现有 degraded/malformed 统计；oversized fields 走现有 cap，不 panic。
- 状态变化：仅测试临时 session trace 文件；无 loop/recovery 状态变化。
- 副作用：无新文件、无新依赖。
- 不变量：`RUNTIME_TRACE_SCHEMA_VERSION` 保持 `run-diagnosis-trace/v1`；logger 的 best-effort/degraded 语义保持。

#### 6. 修改位置

- `crates/ralph-core/src/diagnostics/runtime_trace.rs`：当前负责 entry 序列化和 logger cap；只增加稳定字段/兼容测试所需的最小实现边界，不改变 logger 错误语义。
- `crates/ralph-core/src/diagnosis/bundle.rs`：当前负责 raw trace summary；只增加新 kind 的读取兼容测试/必要的结构化投影验证，不实现根因 classifier。
- `crates/ralph-core/src/diagnostics/integration_tests.rs` 或其已确认的 runtime trace test module：承载临时 session 写入/读取测试；不修改 recovery/events 测试。

#### 7. 可依赖能力

现有 `RuntimeTraceEntry` builder、`RuntimeTraceLogger`、`read_runtime_trace_report`、serde_json、tempfile。

#### 8. 禁止依赖的未来能力

不得在本 Unit 修改 runner、`ExecutionOutcome`、channel merge、task.resume、诊断 skill 文案或报告模板；不得提前写分类逻辑。

#### 9. 验收测试

- **T1. `hat_activation_outcome` round-trip**：临时 logger append 一条新 kind；读取 JSONL，断言 phase/kind/status/hat/ref/关键 scalar fields 完整，sequence 由 logger 分配且连续。
- **T2. 旧 trace compatibility**：写一条没有新 kind/fields 的既有 activation row，再调用 core reader，断言 record_count、status、malformed_lines 与旧语义相同。
- **T3. bounded fields**：写入超长 ref/fields，断言输出不超过现有 cap，logger 标记/行为与现有 cap contract 一致；不得断言“保留完整字符串”。
- 运行：`cargo nextest run -p ralph-core -- runtime_trace`，再运行相关 `diagnosis` 测试过滤器；命令必须由实现后实际 `--list`/测试名确认，若过滤器不存在按停止条件处理，不得改用裸 cargo test。

#### 10. Acceptance Red

首先运行新增 T1/T2。Red 必须是当前代码没有对 `hat_activation_outcome` 结构字段/兼容断言的失败，或测试中期望的新字段不存在；不能接受 workspace 编译环境、路径错误、fixture 缺失导致的失败。T3 若先行失败必须证明来自新 bounded contract，而非测试输入非法。

#### 11. 单元测试拆分

1. Entry JSON shape：固定 phase/kind/status/fields，不 mock serializer。
2. Logger sequence：使用真实 `RuntimeTraceLogger` 和临时目录，断言真实 JSONL。
3. Reader compatibility：使用真实 `read_runtime_trace_report`，不 mock reader。
4. Cap boundary：使用已有 cap helper/logger 边界输入；不以放宽断言换 Green。

#### 12. Red → Green → Refactor 顺序

T1 Red → 增加新 kind/fields 的最小 serializable contract → T1 Green；
T2 Red → 保持 reader generic validation 并补 compatibility assertion → T2 Green；
T3 Red → 复用 logger 既有 cap boundary 并补新字段输入 → T3 Green；
随后 refactor 测试 fixture/命名，重新运行 Unit 1 全部测试。

#### 13. 最小实现范围

必须实现新 kind/fields 能写入和旧 row 能读；必须保持 v1、sequence、cap、best-effort；不实现 runtime outcome 构造，不实现 diagnosis classification，不新增文件。

#### 14. 集成验证

联合真实 `RuntimeTraceLogger`、临时 session 和 core bundle reader；可以 fake 仅临时路径和 JSON input，不能 mock logger 或 reader 的核心行为。预期新行被计入 valid trace，旧行不退化，malformed 行仍 degraded。

#### 15. 风险驱动测试

Characterization（旧 row）和 boundary/cap test 必须有，因为 schema additive 变更最容易破坏旧报告或产生敏感大字段；不需要 concurrency/fuzz/E2E。

#### 16. 回归范围

直接跑 `ralph-core` diagnostics/diagnosis tests；相邻跑 runtime trace、bundle reader、reporter compile tests；旧 trace/disabled mode compatibility；workspace build/lint/typecheck。原因是该 Unit 位于所有诊断 sidecar 的公共格式边界。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/diagnostics/runtime_trace.rs` | 修改现有生产文件/测试 | 固定 additive outcome row contract 或其测试 | E6 |
| `crates/ralph-core/src/diagnosis/bundle.rs` | 新增/修改测试 | 证明新 kind 与旧 row reader 兼容 | E7 |
| `crates/ralph-core/src/diagnostics/integration_tests.rs` | 新增测试 | 真实临时 sidecar round-trip/cap | E6,E11 |

#### 18. 完成标准

T1-T3 通过；相关 core 集成测试、build、lint、typecheck 通过；无新增 skip/only/弱断言；v1 和旧 row compatibility 仍成立；Evidence/Decision 记录在实现提交说明中更新；Unit 可独立提交。

#### 19. 停止条件

若 reader 实际限制未知 kind、fields schema 或必须 bump v2，停止并重新调查 E7/D6；若新增字段超过 cap 或需要新依赖，停止并重新比较 D1/D7；不得进入 Unit 2。

#### 20. 风险与注意事项

- 风险：把动态业务字段编码成固定 schema，导致未来 row 读失败。检测：旧 row/new row round-trip；缓解：只用现有 optional fields。
- 风险：fields 过大泄露 output。检测：cap boundary 检查磁盘内容；缓解：只允许 scalar/count/bounded topic list。
- 剩余风险：老 session 没有 outcome row，只能由 skill 声明 evidence gap；这是兼容性允许的降级，不可伪造历史记录。

### Unit 2：贯通 adapter exit code 并记录 isolated activation outcome

#### 1. Unit 目标

让 isolated normal merge、merge failure 和 interrupt 三条真实调用路径各自最多写一条 outcome row，并保留现有 merge、fallback、event processing、recovery 和 termination 行为。

#### 2. 对应需求与 Scenario

- Requirement：R1、R2、R3、R4、R8
- Scenario：S1、S2、S3、S4、S5、S6、S7
- Decision：D2、D3、D4、D8
- Evidence：E1、E2、E3、E4、E5、E6

#### 3. 外部可观察结果

启用 runtime diagnosis 后，operator 能在同一 session 的 `runtime-trace.jsonl` 看到 activation close 的 status、channel raw facts、backend exit code/timeout/termination 和事件统计；未启用 diagnostics 或非-isolated 模式保持旧产物/行为。

#### 4. 当前行为基线

当前空 channel merge error 只生成 fallback markdown/泛化 log，成功 merge 的零字节分支才有 warning；`ExecutionOutcome` 丢失 exit code。已有 `hat_channel.rs` empty/non-empty tests 和 `legacy/recovery.rs` interrupt tests 固定当前业务结果，必须先在这些测试保护下加 outcome assertions，不能修改已有 recovery assertion 以适应新实现。

#### 5. 输入与输出

- 输入：merge 前 channel metadata；merge result；`ExecutionOutcome`（新增 exit code）；已有 output/event counters；interrupt kind。
- 输出：正常 `merged`、空内容 `empty`、不存在 `missing`、读失败 `unreadable`、非空 merge error `merge_failed`、中断 `interrupted` 的 trace row。
- 错误：snapshot metadata unknown 不得写成 zero；merge error 仍走原 fallback；trace append failure不阻塞。
- 状态变化：只追加 runtime trace；channel 删除、main events 写入、recovery 判定保持原路径。
- 副作用：不新增 sidecar；不写完整 output；每次 activation 不超过一行 outcome。
- 不变量：`empty_terminal_channel` 的既有条件和 `immediate_missing_terminal_emit` 逻辑不变；`task.resume`/retry/blocked 事件序列不变。

#### 6. 修改位置

- `crates/ralph-cli/src/loop_runner/execution.rs`：给 `ExecutionOutcome` 增加 optional exit code；不改 execute 的 success/timeout/termination 语义。
- `crates/ralph-cli/src/loop_runner/inner.rs`：在已有 channel snapshot/merge/result/event-statistics 边界组装并写 outcome；修改两处 outcome 构造传递 PTY/CLI exit code；不重排 recovery 分支。
- `crates/ralph-cli/src/loop_runner/entry.rs`：在 interrupt merge path 写 `interrupted` outcome；保留现有 fallback/error/termination。
- `crates/ralph-cli/src/loop_runner/hat_channel.rs`：只复用已有 channel helper/测试边界；除非真实调用链证明必须，否则不改 `merge_hat_channel` 返回签名。
- `crates/ralph-cli/src/loop_runner/tests/legacy/termination.rs`、`diagnosis.rs`、`recovery.rs`、`hat_channel.rs` 现有测试位置：增加行为断言和 direct outcome fixture 字段。

#### 7. 可依赖能力

Unit 1 的 trace contract；现有 adapter `exit_code`；现有 channel snapshot、merge result、fallback diagnostic、event counters、diagnostics logger、recovery tests。

#### 8. 禁止依赖的未来能力

不得在本 Unit实现 diagnosis skill 分类、修改 `ralph diagnose` reporter schema、改变 task.resume/retry、修改 preset 或 agent-facing skill。

#### 9. 验收测试

1. Empty merge error：用现有 empty channel test seam，断言 fallback 仍存在、channel 仍删除、目标 events 未污染，并额外断言 runtime trace row 是 `empty` 且包含 backend/channel facts。
2. Success empty channel：断言既有 missing-terminal recovery/event assertions不变，trace row 同时落盘。
3. Non-zero exit propagation：在 PTY/CLI conversion test 中分别构造已有 adapter result，断言 `ExecutionOutcome.exit_code` 原样传递。
4. Normal non-empty merge：复用已有 merge tests，断言 `merged`，不改变 stamping/backfill/malformed line assertions。
5. Missing/read failure/target write failure：使用已确认的 fake path/backend seam 或现有 test helper 注入文件错误，断言分别不被误记为 `empty`。
6. Interrupt：复用 `legacy/recovery.rs` interrupt test，断言 `interrupted` outcome 和原有 event integrity。
7. Diagnostics disabled：复用 existing diagnostics mode fixture，断言没有 runtime trace sidecar新增。

#### 10. Acceptance Red

先运行 1、2、3。预期 Red：trace 中没有 `hat_activation_outcome`，或 `ExecutionOutcome` 没有 exit code/测试无法断言其传递；这证明测试到达真实 merge/转换逻辑。若失败来自 borrow/编译错误但未执行行为断言，不算有效 Red，先修正测试入口；若既有 recovery 断言先失败，立即停止，不能把它改成新预期。

#### 11. 单元测试拆分

- `ExecutionOutcome` exit code field：PTY success/nonzero、CLI success/nonzero、timeout with no exit code。
- outcome status mapping：metadata Some(0)、Some(>0)、None/path absent、read error、merge error、interrupt。
- fields projection：scalar counts/boolean/terminal topics bounded；不 mock `RuntimeTraceLogger`。
- preservation tests：真实 `merge_hat_channel`、真实 fallback/removal、真实 event/recovery assertions；只 fake filesystem boundary。

#### 12. Red → Green → Refactor 顺序

T1 empty merge outcome Red → 在 inner merge close 后最小写 row → T1 Green；
T2 success empty/recovery preservation Red → 把同一 row 写入成功分支且不改 gate → T2 Green；
T3 exit code propagation Red → 修改两个 `ExecutionOutcome` 构造和 fixture → T3 Green；
T4 non-empty/error/interrupt status Red → 补齐三条调用路径的 raw status → T4 Green；
T5 disabled mode Red → 复用 diagnostics guard，不在 disabled 创建 row → T5 Green；
Refactor 统一字段构造和 bounded mapping，随后完整跑 Unit 2 回归。

#### 13. 最小实现范围

必须传递 exit code、保存 pre-merge snapshot、在 normal/error/interrupt 记录 outcome、保持现有 recovery/merge semantics；不改变 channel deletion、main events、fallback markdown、task.resume、retry 或 terminal decisions。

#### 14. 集成验证

必须联合真实 `CliExecutor`/PTY result conversion、loop runner、channel helper、diagnostics logger 和 existing recovery path；可 fake backend output/filesystem error，但不可 mock `merge_hat_channel` 或 recovery decision。运行 targeted CLI nextest 和 core diagnostics tests，预期 trace row 与现有 side effects 同时成立。

#### 15. 风险驱动测试

必须做 Characterization（现有 empty/recovery/interrupt）、Contract（PTY/CLI exit code）、Fault Injection（missing/read/write failure），因为本 Unit 横跨 process result、filesystem merge 和 recovery boundary。无需 E2E 或 fuzz；已有真实 runner integration 足以覆盖。

#### 16. 回归范围

直接：`hat_channel`、loop-runner diagnosis/recovery/termination/pty tests；相邻：watchdog、event processing、diagnostics runtime trace/bundle；旧配置/diagnostics disabled/non-isolated；`ralph diagnose` integration JSON/Markdown compatibility；build/lint/typecheck；最后全量 `./scripts/run-tests.sh`。原因是 `ExecutionOutcome` 是 shared internal type，inner merge 位于每个 isolated iteration 的公共路径。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/loop_runner/execution.rs` | 修改现有生产文件 | 传递已有 adapter exit code | E4,E5 |
| `crates/ralph-cli/src/loop_runner/inner.rs` | 修改现有生产文件 | normal merge/error 分支记录 outcome | E2,E3 |
| `crates/ralph-cli/src/loop_runner/entry.rs` | 修改现有生产文件 | interrupt merge 记录 outcome | E1 |
| `crates/ralph-cli/src/loop_runner/hat_channel.rs` | 修改现有测试，生产仅按实际最小需要 | 固定空/非空 merge 事实 | E1 |
| `crates/ralph-cli/src/loop_runner/tests/legacy/{diagnosis,recovery,termination}.rs` | 新增/修改测试 | exit code、recovery preservation、interrupt contract | E3,E5 |

#### 18. 完成标准

S1-S7 测试通过；PTY/CLI exit code 贯通；empty/missing/unreadable/merge_failed/interrupted 不混淆；既有 recovery/event/fallback/termination 断言不变；相关 build/lint/typecheck 通过；无 skip/only/弱断言；Unit 可独立提交。

#### 19. 停止条件

若必须改 `merge_hat_channel` 返回 API、现有 `task.resume` payload、retry key、event ordering 或 recovery test expected events，停止并重新评估 D3/D8；若无法从当前调用链获取 counter，记录 null/unknown 并修订 schema，不能用主 events 事后猜测；若出现新公开调用方，先扩大影响分析。

#### 20. 风险与注意事项

- 风险：在 merge error 分支把所有 `None` metadata 误写成 empty。检测：missing/unreadable fault tests；缓解：status 由 `Option` 三态和 merge error 独立映射。
- 风险：记录时机改变 recovery gate。检测：existing recovery/event sequence exact assertions；缓解：只在旁路 log outcome，不移动 gate。
- 风险：某一 adapter conversion 漏传 exit code。检测：PTY/CLI 双路径 contract tests；缓解：两个构造点必须同时更新。
- 剩余风险：trace I/O 失败时诊断仍可能降级；这是现有 best-effort 设计，报告必须显式显示 degraded。

### Unit 3：让 ralph-run-diagnosis 识别 activation outcome

#### 1. Unit 目标

在现有 bundle-first/L0-L7 流程中读取 `hat_activation_outcome`，输出 raw facts、证据关联、分类、置信度和 evidence gaps；不执行任何恢复命令，不修改 runtime。

#### 2. 对应需求与 Scenario

- Requirement：R6、R7
- Scenario：S1、S2、S3、S5、S8
- Decision：D5、D7、D9
- Evidence：E7、E8、E9

#### 3. 外部可观察结果

诊断报告在已有 `## 0. 产物盘点` 与 `## 5. 问题归因` 之外增加可定位的 activation outcome 表/段，至少展示 activation sequence、hat、status、backend success/exit code、timeout/termination、channel bytes、event counts、correlated recovery/event refs、classification、confidence 和 evidence gaps。没有 row 时报告 `activation_outcomes: unavailable`/legacy gap，而非静默忽略。

#### 4. 当前行为基线

现有 skill 已要求 bundle-first 和 raw runtime trace，但 report template 只有 trace summary；confidence rubric 的计分卡没有 activation row 作为证据项。现有 contract tests 不检查新字段。先对当前 references 建立 Red anchors，再加入最小规则。

#### 5. 输入与输出

- 输入：`diagnosis-input.json`、`runtime-trace.jsonl`、current events、recovery journal、channel fallback markdown、preset/schema/BDD（若存在）。
- 输出：报告中的 activation outcome table、每行分类和 confidence/evidence refs；frontmatter 增加 `activation_outcomes` 状态（`present|missing|degraded|legacy`）或等价稳定字段，必须与模板一致。
- 分类优先级：`timeout_or_termination` → `backend_failure`（nonzero/unsuccessful）→ `channel_routing_failure`（missing/unreadable/merge_failed）→ `attempted_but_rejected`（rejected/policy evidence）→ `successful_no_terminal_emit`（success、empty、terminal obligation、无 accepted/rejected candidate 且 recovery corroborates）→ `unknown`。
- 置信度：activation row + file/line source mapping + recovery/events 双账本可提升 mechanism confidence；只有 activation row 或只有 empty bytes 时不得宣称 agent 根因；按现有 confidence-rubric 的 mode cap 执行。
- 不变量：`task.resume` 继续被描述为 runtime recovery transport；repair suggestions 明确 non-executing；历史/legacy 没有新 row 时不重写旧结论。

#### 6. 修改位置

- `skills/ralph-run-diagnosis/SKILL.md`：更新 Phase 0.2、Phase 0 inventory、L2/L3/L4/L6/L7 规则，明确读取 `hat_activation_outcome` 和分类优先级；保持 bundle-first/legacy/non-executing。
- `skills/ralph-run-diagnosis/references/artifact-discovery.md`：把 activation outcomes 作为 runtime-trace 内 Tier S 子项，定义计数、缺失、malformed 和 session 对应关系。
- `references/verification-pipeline.md`：在 L0 盘点 trace coverage，在 L2/L3 对账 activation row 与 channel/recovery/events，在 L6 源码反查 merge/runner branch。
- `references/report-template.md`：增加 frontmatter 状态键和 activation outcome 表/报告段；不改变既有 required fields。
- `references/mechanism-checklist.md`：增加 activation-close observation、channel state、backend outcome 与 terminal obligation 对账项。
- `references/source-trace-guide.md`：增加 `hat_channel_empty_after_activation`、`merge_hat_channel_failed`、`merge_hat_channel_failed_on_interrupt` 到已有 source mapping，明确它们是 evidence anchors，不是 root cause 本身。
- `references/confidence-rubric.md`：增加 activation row 证据项、分类门槛和“仅空 bytes 不足以归因”的规则。
- `references/examples/minimal-diagnostics-layout.md`：增加最小 runtime trace row/empty-channel evidence 示例；标明示例不等于真实 run。
- `skills/tests/test_run_diagnosis_contract.py`：扩展 stable anchors；不新增 LLM/E2E harness。

#### 7. 可依赖能力

Unit 2 生成的 raw row；现有 bundle-first workflow、report template、confidence rubric、source-trace guide、Python contract test。

#### 8. 禁止依赖的未来能力

不得修改 runtime、core reporter、task.resume、recovery journal schema、preset、agent prompt；不得要求 Executor 发明新的诊断命令或自动执行修复。

#### 9. 验收测试

- **T1 skill inventory**：contract test 断言 skill/ref 明确 `hat_activation_outcome`、runtime-trace nested evidence、legacy/missing behavior。
- **T2 classification contract**：contract fixture/anchors 覆盖 timeout、backend failure、channel routing failure、attempted-but-rejected、successful-no-terminal-emit、unknown 的顺序和证据要求。
- **T3 report contract**：断言 template 有 activation outcome status/table、raw facts、correlated refs、classification、confidence/evidence gap，不破坏既有 frontmatter fields。
- **T4 non-executing/compatibility**：existing tests for task.resume transport, non-executing repair suggestions, human.guidance historical/legacy、bundle-first invocation 仍通过。
- 运行：`.venv/bin/python -m pytest skills/tests/test_run_diagnosis_contract.py`；不得直接调用 LLM 或运行 `ralph run`。

#### 10. Acceptance Red

先运行扩展后的 contract tests。Red 必须显示新 anchor/field/classification contract 在当前 skill references 中缺失；不能是 Python 环境、路径或正则语法错误。若现有 task.resume/non-executing test 失败，先停止并保持旧契约，不得通过删 assertion 取得 Green。

#### 11. 单元测试拆分

1. Artifact discovery anchor：runtime trace 内 activation outcome 必须位于 bundle-first sidecar inventory。
2. Classification anchor：六类状态、优先级、unknown/evidence gap 规则可检索。
3. Report template anchor：frontmatter/表格字段存在且不删旧 required fields。
4. Source mapping anchor：三种 fallback reason 与具体 Rust入口映射存在。
5. Non-executing regression：既有 static contract tests 继续真实执行。

#### 12. Red → Green → Refactor 顺序

T1 Red → 更新 SKILL Phase 0.2 和 artifact discovery → T1 Green；
T2 Red → 更新 verification pipeline、confidence rubric、source guide → T2 Green；
T3 Red → 更新 report template/example → T3 Green；
T4 Red → 保持/修正现有 legacy/non-executing 文字与 anchors → T4 Green；
Refactor references 之间的术语和状态值，重跑完整 Python contract test。

#### 13. 最小实现范围

必须定义 raw row 读取、证据关联、六类分类、confidence/evidence gap、报告落点、legacy/unknown/non-executing；不实现自动执行、不新增 runtime classifier、不改变 CLI diagnose。

#### 14. 集成验证

用 repository-local skill contract tests 检查五类 references 一致；以 Unit 2 产生的真实 row shape 作为示例契约；可以使用静态/synthetic fixture，不需要 mock runtime，因为 runtime 已由 U2 验证。

#### 15. 风险驱动测试

必须做 contract/characterization：skill 文档是诊断 agent 的行为接口，旧 bundle-first、task.resume terminology 和 non-executing 规则不能回归。对证据不足场景用 synthetic unknown fixture，防止分类器过度归因；不需要 E2E。

#### 16. 回归范围

`skills/tests/test_run_diagnosis_contract.py` 全部测试；repo-local skill references 互相引用检查；与 `ralph diagnose` 现有 output template/JSON summary 的兼容性检查；不改 core CLI，但需运行 `cargo nextest run -p ralph-cli --test diagnose` 验证 generic trace summary 没有退化。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `skills/ralph-run-diagnosis/SKILL.md` | 修改文档 | bundle-first 读取与分类规则 | E8 |
| `skills/ralph-run-diagnosis/references/{artifact-discovery,verification-pipeline,report-template,mechanism-checklist,source-trace-guide,confidence-rubric}.md` | 修改文档 | 盘点、校验、报告、置信度和源码映射 | E8 |
| `skills/ralph-run-diagnosis/references/examples/minimal-diagnostics-layout.md` | 修改示例 | 展示新 raw artifact，不伪造真实 run | E8 |
| `skills/tests/test_run_diagnosis_contract.py` | 修改测试 | stable anchors/legacy regression | E9 |

#### 18. 完成标准

S8 和 T1-T4 通过；报告契约能区分六类或输出 unknown；旧 bundle/legacy/non-executing/task.resume 规则全绿；CLI diagnose integration test 通过；不引入自动执行、删断言或计划专属 preset 文案；Unit 可独立提交。

#### 19. 停止条件

若 skill 无法直接访问 raw runtime trace，停止并重新评估 D9；若分类必须改变 runtime/recovery schema，停止并回到 D5/D8；若 confidence rubric 与现有 mode cap 冲突，先更新证据规则和测试，不让 agent自行选择阈值。

#### 20. 风险与注意事项

- 风险：把 `status=empty` 直接写成 agent 根因。检测：unknown/attempted/recovery correlation fixture；缓解：分类必须满足 terminal obligation + ledger corroboration。
- 风险：skill reference 之间状态值漂移。检测：contract anchors 对所有状态值；缓解：在同一 Unit 同步更新 SKILL、references、template、test。
- 风险：diagnose CLI summary 与 raw report 认知不一致。检测：现有 CLI diagnose integration；缓解：明确 CLI generic summary、skill raw classifier 分层。

### Unit 4：跨边界回归与最终质量门禁

#### 1. Unit 目标

用现有 Rust nextest、CLI diagnose integration、Python skill contract 和全量测试证明 Unit 1–3 的新增证据不会改变既有 loop/recovery/diagnose 行为；不新增功能。

#### 2. 对应需求与 Scenario

- Requirement：R1-R8
- Scenario：S1-S8
- Decision：D1-D9
- Evidence：E1-E12

#### 3. 外部可观察结果

同一个空 channel run 可以从 session trace → existing events/recovery/fallback → `ralph diagnose` generic summary → `ralph-run-diagnosis` activation table 完整追踪；旧 run、disabled diagnostics、non-isolated、正常 merge、timeout/interrupt 不产生错误回归。

#### 4. 当前行为基线

既有 `hat_channel`、recovery/termination、diagnose integration、core diagnostics tests 和 skill contract tests 是回归基线；旧 behavior 必须全部保留，新增只应是 trace row 和 skill report evidence。

#### 5. 输入与输出

- 输入：Unit 1–3 代码/文档和已有 test fixtures。
- 输出：所有 targeted/related/full gates 通过；测试报告能追溯到 R/S/U；无计划外文件。
- 错误：任何真实失败、skip/only、断言弱化、schema drift、trace artifact mismatch 都阻止关闭。
- 状态变化：只产生测试临时目录和诊断工作目录；不修改运行中的 `.ralph` 状态。

#### 6. 修改位置

本 Unit 只允许修改因回归发现的本计划内测试 fixture/文档 anchor；不得为了过门禁扩大生产范围。若发现真实代码冲突，按停止条件回退到对应 Unit 重新决策。

#### 7. 可依赖能力

Unit 1–3 已关闭且证据/测试通过；真实 workspace nextest、Python `.venv`、现有 CLI diagnose integration。

#### 8. 禁止依赖的未来能力

不得添加未计划的 E2E、外部服务、第三方依赖、自动修复、new sidecar 或 task.resume 变化。

#### 9. 验收测试

- 运行 Unit 1–3 的全部 targeted tests。
- 运行 CLI diagnose integration：`cargo nextest run -p ralph-cli --test diagnose`。
- 运行 core diagnosis/diagnostics tests：使用 `cargo nextest run -p ralph-core -- diagnostics` 和 `-- diagnosis` 过滤器，实际测试名不存在时按停止条件执行 `cargo nextest list` 调查。
- 运行 `.venv/bin/python -m pytest skills/tests/test_run_diagnosis_contract.py`。
- 运行 `./scripts/check-cli-doc-drift.sh`（本计划不改 CLI help，若文档 drift 由现有基线导致需记录，不得顺手扩大范围）。
- 最终运行 `./scripts/run-tests.sh`。

#### 10. Acceptance Red

Unit 4 的 Red 不是预先制造失败；先跑所有 targeted tests，若发现回归必须记录真实失败模块和触发证据。若全量失败来自环境/工具不可用，不得视为行为 Red；按仓库规定执行可用的 nextest/serial fallback 并记录。

#### 11. 单元测试拆分

1. Targeted Rust regression：new trace + merge + recovery + diagnose。
2. Python contract regression：all existing/new anchors。
3. Compatibility: old rows/legacy session/disabled mode。
4. Full workspace: build/lint/typecheck/nextest/doctest as required by script。

#### 12. Red → Green → Refactor 顺序

Targeted Rust/Python Red characterization → 修复仅属于本计划的真实 drift → targeted Green → 运行相邻 integration → 运行 full script → 若只需文档/fixture refactor，完成后重跑全部 gates；不得直接更新 snapshot/golden 掩盖行为变化。

#### 13. 最小实现范围

只做跨边界验证和计划内 drift 修正；不引入新业务行为，不扩大受影响文件。

#### 14. 集成验证

必须联合 core logger/reader、CLI runner/channel/recovery、CLI diagnose、skill contract；所有真实行为使用 nextest/pytest，不用 mock 掉被验证的边界。

#### 15. 风险驱动测试

需要 compatibility、Characterization、Fault Injection 已在 U1–U3 覆盖；Unit 4 只重跑，不新增无证据测试类别。必要时用 serial fallback 处理已知时序 flake，但不能把真实失败标为 flake。

#### 16. 回归范围

直接相关 Rust tests、相邻 diagnostics/diagnosis/event/recovery/watchdog tests、CLI diagnose JSON/Markdown、legacy/disabled/non-isolated path、workspace build/lint/typecheck、Python skill contracts、最终全量 `./scripts/run-tests.sh`。这些范围覆盖共享 `ExecutionOutcome`、runtime trace reader 和 loop-runner merge 的所有已确认消费者。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| Unit 1–3 已列文件 | 仅计划内修改 | 处理真实回归 drift | E1-E11 |
| 其它位置 | 禁止变更 | 防止借回归扩大范围 | D8 |

#### 18. 完成标准

所有 S1-S8、R1-R8 有通过测试；targeted/related/full gates 通过；没有新增 skip/only、弱断言、无解释 snapshot/golden、未处理 BLOCKED；git diff 只含本计划范围；Unit 1–4 均形成完整闭环并可独立提交。

#### 19. 停止条件

任何跨边界行为改变、全量回归超出 `ExecutionOutcome`/trace/diagnose/skill 影响范围、发现未记录消费者、或只能通过削弱断言 Green，立即停止并回到对应 Unit 更新 Evidence/Decision。

#### 20. 风险与注意事项

- 风险：只跑 targeted 导致 workspace 其它 adapter/diagnosis consumer 回归。检测：最终 `./scripts/run-tests.sh`；缓解：Unit 4 强制全量。
- 风险：Python contract 通过但 skill 语义互相矛盾。检测：人工按 bundle-first→L0-L7→report template 走一次；缓解：状态值和分类表集中于同一 Unit 更新。
- 剩余风险：真实历史 run 没有新 row；诊断只能标 legacy/evidence gap，不能补写历史事实。

## 8. Unit 串行依赖图

```text
Unit 1
  ↓ 提供已验证的 v1 runtime-trace outcome row、旧 row compatibility 和 bounded fields contract
Unit 2
  ↓ 提供已验证的 normal/error/interrupt raw outcome 以及 adapter exit_code 事实
Unit 3
  ↓ 提供已验证的 bundle-first 分类、report、confidence 和 unknown/evidence-gap contract
Unit 4
```

Unit 2 不能先于 Unit 1，因为 runner 不能自行决定 trace kind/field/schema；Unit 3 不能先于 Unit 2，因为 skill 示例和分类必须以真实 row shape 为准；Unit 4 不能交换到前面，因为它验证前三个边界的最终一致性。任一 Unit 不得提前实现后续 Unit 行为。

## 9. 执行命令清单

以下命令是基于已确认仓库配置的执行契约；具体 filter 名称若在实现后不存在，先用 `cargo nextest list` 调查，不得静默替换成裸 `cargo test`。

| 时机 | 命令 | 验证目的 | 预期结果 | 失败是否可进入下一步 |
|---|---|---|---|---|
| U1 Red/Green | `cargo nextest run -p ralph-core -- runtime_trace` | trace serializer/logger/cap | 新旧 row、sequence、cap 测试通过 | 否 |
| U1 reader | `cargo nextest run -p ralph-core -- diagnosis` | bundle reader compatibility | generic kind/old row 通过 | 否 |
| U2 targeted | `cargo nextest run -p ralph-cli --bin ralph -- hat_channel` | channel merge/empty outcome | empty/non-empty/error assertions通过 | 否 |
| U2 runner | `cargo nextest run -p ralph-cli --bin ralph -- diagnosis` | loop-runner diagnosis tests | outcome/fallback/recovery 通过 | 否 |
| U2 recovery | `cargo nextest run -p ralph-cli --bin ralph -- recovery` | task.resume/interrupt behavior unchanged | existing events/recovery 通过 | 否 |
| U2 adapter | `cargo nextest run -p ralph-cli --bin ralph -- termination` | `ExecutionOutcome.exit_code` fixtures and termination | PTY/CLI conversion compile/test through | 否 |
| U3 skill | `.venv/bin/python -m pytest skills/tests/test_run_diagnosis_contract.py` | skill/reference/report contract | all existing/new anchors pass | 否 |
| U3 CLI compatibility | `cargo nextest run -p ralph-cli --test diagnose` | generic `ralph diagnose` output unchanged | existing JSON/Markdown tests pass | 否 |
| U4 doc drift | `./scripts/check-cli-doc-drift.sh` | repository docs/CLI drift gate | exit 0；若 baseline drift，记录并停止 | 否 |
| U4 full | `./scripts/run-tests.sh` | nextest phases、doctest、workspace regression | full repository gate passes | 否 |
| Fallback only if nextest unavailable | `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | repository-approved serial flake fallback | serial baseline passes | 否 |
| Build/lint | `cargo build` / `cargo clippy` / `cargo fmt -- --check` | compile/lint/format | exit 0 | 否 |

不得运行裸 `cargo test -p ralph-cli` 或裸 `cargo test -p ralph-cli --bin ralph`。每条测试失败都必须记录实际失败、原因和是否属于有效 Red；失败不能通过 skip、only、放宽断言或扩大 timeout 绕过。

## 10. 最终质量门禁

- 所有 S1-S8 均有对应测试且通过。
- R1-R8 全部在追踪矩阵中有 Scenario、验收测试、Unit 和 Evidence。
- Unit 1 的旧 trace compatibility、schema v1、sequence、field cap 通过。
- Unit 2 的 normal/empty/missing/unreadable/merge_failed/interrupted 事实状态和 PTY/CLI exit code 通过。
- Unit 2 的 existing merge、event stamping、fallback、recovery、retry、blocked、termination 断言没有改变。
- Unit 3 的 bundle-first、legacy fallback、task.resume runtime transport、non-executing、confidence mode cap 和 unknown/evidence gap 通过。
- `ralph diagnose` generic JSON/Markdown integration 通过；不要求其新增 activation classifier。
- Python skill contract 使用 `.venv` 通过。
- `cargo build`、`cargo clippy`、`cargo fmt -- --check`、相关 nextest 和最终 `./scripts/run-tests.sh` 通过。
- 没有新增 skip/only、削弱断言、无解释 snapshot/golden、计划外依赖、计划外文件或未处理 BLOCKED decision。
- 所有关键 Decision 置信度仍不低于 0.85；若实现发现反证，必须回滚到对应 Unit 的停止条件处理。
- 每个 Unit 的 Acceptance Red → Unit Red → Green → Refactor → Integration → Regression → Close 证据已记录，且 Unit 严格串行完成。
- 遗弃的实验代码、临时 fixture、诊断 workdir、`.ralph/review` residual/scratch 不进入提交；计划只保留必要的 repo-local skill/reference 与测试变更。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 有真实入口、字段契约、调用链、测试 Red/Green、Unit 完成标准和停止条件 |
| Executor 是否仍需做关键设计决策 | 否 | D1-D9 已决定存储、时机、API边界、exit code、诊断层、schema、敏感字段、恢复范围 |
| 所有文件和接口是否有代码库证据 | 是 | E1-E12；新增位置均明确标为“修改/新增测试”且不伪装成现有接口 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1-D9 为 0.90–0.99 |
| 是否存在未处理的低置信度假设 | 否 | 只有已确认约束；实现冲突时有停止/重新决策路径 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 trace contract、U2 runtime observation、U3 diagnosis consumption、U4 regression gate |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 有入口、Red、测试命令、集成和完成标准 |
| 每个 Unit 是否有真实 Red | 是 | U1-U3 先对缺失能力/anchor 运行；U4 只接受真实回归 Red，不制造失败 |
| 每个 Unit 是否包含回归范围 | 是 | 每个 Unit 第 16 项明确直接、相邻、兼容和构建回归 |
| 是否存在未来 Unit 依赖 | 否 | 依赖图只有 U1→U2→U3→U4，且各 Unit 禁止提前实现未来行为 |
| 是否存在泛化任务描述 | 否 | 每项均绑定文件、函数/边界、输入输出、断言和命令 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | S1-S8 在验收表、矩阵和 U1-U4 中逐一出现 |
| 所有关键决策是否有 Evidence | 是 | D1-D9 均引用 E1-E11；E12 约束基线 |
| 计划是否可以严格串行执行 | 是 | 每个 Unit 完成全部测试/重构/回归/Close 后才进入下一 Unit |

## Definition of Done

### Per-unit

- 当前 Unit 的 Scenario、Acceptance Red、Unit tests、Green、Refactor、Integration、Regression 全部有执行证据。
- 没有改变本 Unit 明确禁止的 recovery、event、schema 或 CLI 行为。
- 预期文件变更与 Unit 表一致，Unit 可形成独立提交。

### Global

- `runtime-trace.jsonl` 在 diagnostics enabled 的 isolated activation 中保留可关联的 raw outcome；空 channel 不再只有泛化 fallback。
- `ralph-run-diagnosis` 能识别并报告 activation outcome；不能识别时明确 unknown/evidence gap，不做无证据根因断言。
- `task.resume` 为什么发生、是否重试、如何耗尽的既有行为完全不在本计划范围内且回归通过。
- 所有构建、lint、typecheck、nextest、Python contract 和最终全量门禁通过。
- 只提交必要的生产/测试/skill reference 变更；无临时产物、无 `.ralph` runtime 状态、无 residual/scratch。

## Appendix

### 关键事实到结论的最短链

```text
prepare_hat_channel 创建零字节文件
  → backend 返回
  → inner.rs 预读 channel metadata
  → merge_hat_channel 可能删除空 channel并返回错误
  → 当前 error 分支只留下泛化 fallback，且 ExecutionOutcome 丢失 exit_code
  → runtime-trace 缺少 activation outcome
  → ralph-run-diagnosis 无法在中间产物上区分 backend/channel/agent 结果
```

### 明确不改的恢复链

```text
empty_terminal_channel
  → immediate_missing_terminal_emit
  → existing missing-terminal recovery / task.resume
  → existing retry / plan.blocked
```

本计划只在该链旁边追加 activation evidence，不改变任何箭头的条件、目标、计数或终态。
