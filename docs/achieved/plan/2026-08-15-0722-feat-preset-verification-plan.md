---
title: Preset 可运行性验证 - Plan
type: feat
date: 2026-08-15
topic: preset-verification
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-brainstorm
execution: code
---

# Preset 可运行性验证 - Plan

## Goal Capsule

- **Objective：**为 preset author/reviewer 增加通用、确定性、可复现的 runtime verification，使静态 contract 通过但 workflow 不推进、失败不收敛、无输出卡住或终态未闭合的 preset 在交付前失败。
- **Authority hierarchy：**用户已确认的 Product Contract；本计划的 Decision Records；真实源码/测试 Evidence；实现细节不得反向改变 R1-R16 的行为目标。
- **Execution profile：**严格按 Unit 1→Unit 2→Unit 3→Unit 4→Unit 5；每个 Unit 完成 Red→Green→Refactor→Integration→Regression→Close 后才进入下一个。
- **Stop conditions：**触发任一 Unit 的停止条件、发现实际调用链与 Evidence 冲突、或关键决策低于 0.85 时停止并重新取证，不让 Executor 猜测。
- **Tail ownership：**实现阶段由 Coding Agent 执行本计划；最终质量门禁由 Unit 5 和第 10 节共同收口；本计划本身不写生产代码。

## Product Contract

### Summary

新增独立的 `ralph preset verify`。它先复用现有静态 contract/preflight，再用 version 1 scripted scenario 驱动真实 EventLoop，在隔离 workspace 中报告有序事件、失败/恢复结果、终态闭合和可复现 trace。`preset check` 保持静态，`inspect` 保持只读。Builtin 可增加 Ralph 源码证据，external preset 不依赖 Rust/Cargo/source-line。

### Problem Frame

现有 `preset check` 能判断声明层 contract，却不会执行 workflow；`inspect prompt` 能预览 prompt，却不会验证事件循环是否继续推进。现有测试中还存在只计迭代、不处理事件的 stub runner。author/review 因此可能在静态通过后漏掉无输出、runtime recovery 或终态闭合故障。

### Requirements

- R1. `preset check` 继续只做静态 contract，不调用 backend 或推进 workflow。
- R2. `preset verify` 在静态通过后用受控输入推进真实 workflow，并报告有序事件、终态和失败原因。
- R3. `inspect` 继续只读，不执行完整 workflow verification。
- R4. scenario 覆盖成功、failure/blocked、空业务输出、非法 contract、runtime recovery 和 terminal closure。
- R5. scenario 明确起点、response、允许事件、终态和有界停止条件；无进展必须失败并保留 last state。
- R6. verify 不依赖真实模型、网络、目标项目语言或构建系统；external 项目可以没有 Rust/Cargo。
- R7. 输出同时支持 human/JSON，并区分 static failure、scenario failure、runtime exception、timeout、no-progress、unclosed terminal。
- R8. 运行结果结构化暴露空输出、rejection、recovery、blocked 和 runtime terminal。
- R9. runtime 生成的 control/recovery/terminal event 必须能验证触发和合法后继/终态。
- R10. verifier 不能把未闭合或预算耗尽报告成成功，并提供可复现 trace。
- R11. author/review 按 builtin/external source mode 分流证据。
- R12. author 必须确认最小 success、failure/block、no-output/abnormal-output scenarios 并执行 verify。
- R13. reviewer 必须同时检查 static contract、scenario coverage、actual result 和 terminal closure。
- R14. builtin 可读源码/BDD/replay 作为补充，但仍需 actual verify。
- R15. external 只依赖公开 CLI、preset/schema/scenario/result，不要求 Rust/Cargo/source。
- R16. findings 使用通用能力分类，不绑定事故、preset 或业务 topic。

## 0. 计划状态

**READY。**所有进入实施范围的关键技术决策均有直接源码、现有测试或用户已确认的产品决策支持，置信度均不低于 0.85。这个 READY 只表示计划可交给 Coding Agent 执行，不表示功能已经实现。

- **代码库基线：**分支 `pittcat-dev`，HEAD `f90ccfe0`。
- **调查范围：**`ralph preset` CLI dispatch 和静态检查、`ralph inspect` 的只读实现、preflight 配置加载、EventLoop 生产构造和事件处理、现有 replay/smoke/BDD 场景、CLI 集成测试、author/review skill、注入式命令文档、zsh completion、相关 Git 历史。
- **已执行的只读调查命令：**`git rev-parse --short HEAD`、`git status --short`、`rg --files`、`rg -n`、`sed -n`、`nl -ba`、`git log --oneline --all -- <相关路径>`。
- **本轮未执行：**没有运行 build、lint、nextest、E2E 或真实 CLI workflow；`ce-plan` 阶段只做源码和配置取证。以下命令清单是 Coding Agent 在对应 Unit 中必须执行的验证契约。
- **阻塞项：**无。精确场景 YAML、报告字段和 verifier API 已在第 3 节明确，不留给 Executor 临时选择。

## 1. 功能目标

### 1.1 业务目标与调用方

新增独立的 `ralph preset verify`，在 author 和 review 阶段用确定性的受控输入实际推动 preset 的 workflow，验证事件序列、失败/恢复路径、无输出边界和最终终态闭合。

调用方是 preset author、preset reviewer，以及不包含 Rust/Cargo/Ralph 源码的外部项目 operator。验证器运行在 Ralph CLI 内部；目标项目只需要提供可加载的 core config、hat preset/schema 和验证场景文件，不需要 Rust 构建系统。

### 1.2 当前行为

- `ralph preset check` 在 `crates/ralph-cli/src/commands/preset.rs:828-887` 加载配置后调用 `RuntimeContractAggregator::aggregate`，只产生静态 `RuntimeContractReport`，不会推进 EventLoop，也不会调用 backend。
- `ralph inspect prompt` 在 `crates/ralph-cli/src/commands/inspect.rs:547-600` 构造临时 EventLoop 预览 prompt；代码明确保证不写事件、不启动 loop。candidate emit 也只在 `inspect.rs:764-799` 做只读评估。
- `crates/ralph-core/src/testing/scenario.rs:41-81` 的 `ScenarioRunner` 仅循环调用 `MockBackend` 并返回空事件列表；它不是实际 workflow runner，不能作为新能力的基础。
- `crates/ralph-core/src/testing/smoke_runner.rs` 的 `SmokeRunner` 读取 replay terminal output 并统计迭代/终止原因，但不驱动 EventLoop 的 hat 调度和 JSONL 事件处理，也不能替代 workflow verify。
- 真实 BDD 路径在 `crates/ralph-core/tests/scenarios.rs:1141-1360`：临时 workspace、`EventLoop::from_resolved`、`initialize`、`next_hat`、`build_prompt`、`process_output`、写入 JSONL、`process_events_from_jsonl`，并在 `run_workflow_guard_scenario` 中断言真实事件；该路径是实现行为的最高等级参考。

### 1.3 目标行为与行为差异

- `ralph preset check` 仍然是静态检查，不启动 workflow、不调用真实 backend、不执行场景。
- `ralph inspect` 仍然是只读观察/预览，不写状态、不推进 workflow。
- `ralph preset verify --scenario <file>` 先沿用 preflight 加载和严格静态 contract 检查，再在隔离临时 workspace 中用场景提供的脚本化 output 驱动真实 EventLoop，输出每个场景的有序事件、最后可观察状态、终止类别和通过/失败结果。
- verify 在达到场景步数预算、无进展预算或运行时终止时必须返回；没有终态不能报告成功。
- 外部项目的 verify 不读取目标项目 Rust 源码、不运行 Cargo、不要求 Rust；builtin review 可以在 verify 结果之外读取本仓库源码和 BDD/replay 测试作为补充证据。

### 1.4 输入、输出、状态、副作用和错误语义

- **输入：**现有全局 `-c/--config` 和 `-H/--hats`；一个 `--scenario` YAML 文件；可选 `--format human|json`。verify 不接受 remote config/hats，发现 `ConfigSource::Remote` 或 `HatsSource::Remote` 时在加载前返回静态输入错误，避免验证依赖网络。
- **场景输入：**文件包含 `version: 1` 和 `scenarios` 列表。每个 scenario 必须有 `name`、`responses`、`expect`、`limits`；`responses` 中每项包含可选 `hat`、`output`、可选 `success`，空字符串合法，表示激活后无业务输出。
- **期望输入：**`expect.start_event` 必须等于最终 config 的 `event_loop.starting_event`；`expect.accepted_events` 是有序 topic 列表；`expect.forbidden_events` 是不应被接受的 topic 列表；`expect.terminal` 为 `success|failure|blocked|none`；`expect.terminal_topic` 在非 `none` 时必填；可选 `expect.payload_fields` 按 topic 做结构化字段匹配。
- **预算输入：**`limits.max_steps` 为正整数；`limits.no_progress_steps` 为正整数且不大于 `max_steps`。验证器使用离散步骤预算，不使用真实时间等待，避免 CI 时间抖动。
- **输出：**人类格式打印场景摘要、静态结果、每步 hat/response/accepted topics、runtime 终止类别、最后状态和失败原因；JSON 格式输出稳定的 `PresetVerifyReport`，包含 `passed`、`source_kind`、`static`、`scenarios`、`failure_kind`、`last_observable_state`、`trace_digest`。
- **状态变化：**每个 scenario 使用独立临时 workspace 和临时 `.ralph` 状态；运行结束后清理。不得修改调用方工作树、生产代码、已有 `.ralph` 运行状态或目标项目构建产物。
- **错误语义：**配置/场景解析错误属于 `input_error`；静态 contract 失败属于 `static_contract_failure`；EventLoop 构造或事件读取异常属于 `runtime_exception`；未达到终态即耗尽 `max_steps` 属于 `timeout`；连续 `no_progress_steps` 无业务进展属于 `no_progress`；观察到失败/阻断但未满足合法终态或终态事件没有合法闭合属于 `unclosed_terminal`；期望事件不匹配属于 `scenario_failure`。所有上述结果退出非零，只有所有场景通过才退出零。
- **兼容性：**不改变现有 `check`、`inspect`、`run` 默认语义；旧 preset 不增加 verify 场景时仍可被现有命令加载，但 `ralph-preset-author`/`ralph-preset-review` 不得把缺少 verify 证据的 preset 判定为已完成 review。
- **性能：**不引入网络或模型调用；每个场景执行次数严格受 `max_steps` 限制；实现不得使用无界重试或 sleep 等待。
- **安全/权限：**verify 只允许读取 preset/config/schema/scenario 和创建临时隔离状态；禁止把场景中的脚本内容当 shell 命令执行，禁止读取外部项目 Rust 源码作为 external 模式的隐含步骤。

### 1.5 范围与非目标

**本次范围：**新的 core verifier 数据模型和真实 EventLoop 驱动器；新的 `ralph preset verify` CLI；人类/JSON 结果和退出语义；builtin/external source mode；author/review skill 的通用门禁与 finding；skill fixture/anchor；命令文档、preset authoring 文档和 zsh completion；真实 runtime/CLI/skill 回归测试。

**非目标：**把动态执行加入 `preset check`；把执行加入 `inspect`；运行真实模型 API、网络 backend 或目标项目构建命令；为某个事故、preset 名称或业务 topic 编写特判；修改 builtin preset 的业务拓扑；复用 `ScenarioRunner` stub；把 replay terminal parser 当作 workflow verifier；本计划不要求新增数据库、外部服务或第三方依赖。

**已确认约束：**所有测试使用 `cargo nextest run` 系列；BDD 必须走真实 EventLoop；skill 文档必须区分 builtin 与 external；注入式 `crates/ralph-core/data/*.md` 只写 agent 下一步可执行的通用规则，不写 runtime 私有实现。

**待验证假设：**无影响实施方向的待验证假设。实现中若发现 `EventLoop` 公开边界不足，必须按 Unit 停止条件先记录证据并重新决策，不得让 Executor 临时暴露内部字段。

## 2. 代码库现状与证据

### 2.1 当前实现入口与调用链

外部 CLI 入口是 `crates/ralph-cli/src/main.rs:97-176` 的 `Cli`/`Commands`，全局 `-c/--config` 和 `-H/--hats` 在 `main.rs` 解析后，于 `main.rs:510-527` 传给 `commands::preset::execute`。`PresetCommands` 当前在 `crates/ralph-cli/src/commands/preset.rs:40-110` 没有 `Verify` 分支；dispatch 在 `preset.rs:233-269` 只处理 `Check`、`New`、`Diff` 等现有子命令。

配置入口是 `crates/ralph-cli/src/preflight.rs:197-255` 的 `load_config_for_preflight`：加载 core、可选 hats source、合并和 normalize、设置 workspace root、解析 schema 文件、应用 CLI override。新命令必须复用它，不得另建 config loader。

静态检查入口是 `preset.rs:861-887` 的 `build_report`，使用 `RuntimeContractAggregator`；运行前 hard gate 是 `crates/ralph-cli/src/loop_runner/preset_lint_gate.rs:58-105`。两者都属于动态执行前的静态层。

真实 runtime 构造入口是 `crates/ralph-core/src/execution_contract/compiler.rs:206-265` 的 `compile` 和 `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs:198-245` 的 `EventLoop::from_resolved`/`from_resolved_no_context`。有 workspace 的 verifier 必须使用 `LoopContext::primary(temp_workspace)` 和 `EventLoop::from_resolved`。

真实受控执行链是 `EventLoop::initialize`（`state_recovery.rs:108-117`）发布 configured `starting_event`，随后 `next_hat`（`state_recovery.rs:468`）、`build_prompt`（`event_processing.rs:1151`）、`process_output`（`dispatch_and_handoff.rs:558-563`）、`EventParser::parse`（`crates/ralph-core/src/event_parser.rs:224-245`）、写入 JSONL、`process_events_from_jsonl`（`parse_and_emit/legacy.rs:298-304`）。这些方法均已对外可用或已有 CLI/BDD 调用，不需要访问 EventLoop 私有字段。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `crates/ralph-cli/src/main.rs:97-176,510-527` | 全局 config/hats 参数在 CLI 入口解析并传入 preset command dispatch。 | 新命令必须加入现有 `Commands::Preset` 链路，不能创建第二个 CLI 入口。 | 高 |
| E2 | `crates/ralph-cli/src/commands/preset.rs:40-68,233-241` | `PresetCommands` 只有静态 `Check`，dispatch 只调用 `check_preset`。 | `Verify` 是新的独立子命令；check 的默认行为不应改写。 | 高 |
| E3 | `preset.rs:828-887` | `check_preset` 仅构建并打印 `RuntimeContractReport`，失败通过 report 映射退出。 | verify 必须把静态 report 和动态 report 分层输出，并复用 config/report 加载路径。 | 高 |
| E4 | `crates/ralph-cli/src/commands/inspect.rs:547-600,764-799` | inspect prompt 构造临时 EventLoop 做 preview，candidate emit 是只读评估。 | inspect 不应承担动态执行；新增能力放在 preset 命名空间。 | 高 |
| E5 | `crates/ralph-cli/src/preflight.rs:197-255` | preflight 统一完成 config/hats 合并、normalize、workspace root、schema 解析和 override。 | verifier 复用 `load_config_for_preflight`，不复制加载/合并逻辑。 | 高 |
| E6 | `crates/ralph-core/src/execution_contract/compiler.rs:206-265` | 生产 EventLoop 要先 compile 成 `ResolvedRuntimeConfig`，失败时不能构造 loop。 | verifier 的每个 scenario 都必须经过 compile，再用 `from_resolved`。 | 高 |
| E7 | `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs:198-245` | 有 workspace 的生产构造函数是 `EventLoop::from_resolved(resolved, LoopContext)`。 | 使用临时 workspace 的 verifier 复用真实生产构造路径。 | 高 |
| E8 | `crates/ralph-core/src/event_loop/state_recovery.rs:108-117,468` | `initialize` 发布配置的 starting event；`next_hat` 按真实 bus 调度 hat。 | scenario 的 start 必须核对最终 config starting event，不能只模拟一个固定 hat。 | 高 |
| E9 | `crates/ralph-core/src/event_loop/dispatch_and_handoff.rs:558-563` | `process_output` 会推进 iteration 并记录当前 hat，返回可选 termination reason。 | verifier 要把它纳入每步 trace，不能只解析 output 文本。 | 高 |
| E10 | `crates/ralph-core/src/event_loop/parse_and_emit/legacy.rs:298-304` | `process_events_from_jsonl` 是事件读取、校验、路由和 backpressure 的统一入口。 | 受控 output 必须写入临时事件文件后通过该入口，禁止绕过 runtime 直接拼 expected events。 | 高 |
| E11 | `crates/ralph-core/tests/scenarios.rs:1141-1360` | 真实 BDD helper 使用临时 workspace、from_resolved、initialize、next_hat、build_prompt、process_output、JSONL 和 process_events。 | 新 verifier 的 driver 直接抽象该真实链路；该文件是 Outside-In 行为参考。 | 高 |
| E12 | `crates/ralph-core/tests/scenarios.rs:1966-2016` | `run_scenario` 使用 stub `ScenarioRunner`；`run_workflow_guard_scenario` 才进入真实 EventLoop。 | 新测试禁止使用 stub runner；必须测试真实 driver。 | 高 |
| E13 | `crates/ralph-core/src/testing/scenario.rs:41-81` | `ScenarioRunner` 只执行预设迭代并返回空 events。 | 不修改它来承载 verify；避免假 Green。 | 高 |
| E14 | `crates/ralph-core/src/testing/smoke_runner.rs` | SmokeRunner 的输入是 replay terminal output，结果只有 iterations/events_parsed/termination_reason/output_bytes。 | Smoke/replay 可作为 builtin 补充证据，但不是 preset verify 的 workflow driver。 | 高 |
| E15 | `crates/ralph-core/src/event_parser.rs:224-245` 与 `crates/ralph-core/src/lib.rs:202` | EventParser 是公开 re-export，可解析脚本 output 中的事件。 | verifier 可复用现有 parser，不引入新的事件语法。 | 高 |
| E16 | `crates/ralph-core/src/loop_context.rs:93-190` | `LoopContext::primary` 和 `events_path`/`ralph_dir` 为 workspace 状态边界。 | 每个 scenario 使用独立临时 workspace，运行后清理。 | 高 |
| E17 | `crates/ralph-core/src/config/loop_config.rs:655-672` 与 `event_loop/tests/progress_steward.rs` | no-progress 可转为 `plan.blocked(reason=loop_stalled_max_iterations)`，并存在 enabled/disabled 两类行为测试。 | 报告必须区分 runtime 产生的 blocked 与 verifier 自己耗尽预算，且不能把无输出当成功。 | 高 |
| E18 | `crates/ralph-core/src/event_loop/types.rs:37-58` | `ProcessedEvents` 暴露 had_events、had_rejected_events、accepted_events、contract_rejections、payload violation。 | verifier 的 trace/result 可以基于公开结构化运行结果，不抓日志文本。 | 高 |
| E19 | `crates/ralph-cli/src/cli/shared.rs:169-211` | `HatsSource` 明确区分 File、Builtin、Remote，并有公开 label。 | verifier 能确定 builtin/external；Remote 在 deterministic verify 中拒绝。 | 高 |
| E20 | `skills/ralph-preset-author/SKILL.md` 与 `skills/ralph-preset-review/SKILL.md` | 当前流程有静态 check/AAF，但没有强制真实 runtime verify；review 默认不跑全量测试。 | 两套 skill 必须新增通用动态证据门禁，同时保留 builtin/external 分流。 | 高 |
| E21 | `skills/ralph-preset-author/references/commands.md`、`skills/ralph-preset-review/references/commands.md` | author/review 各自维护命令说明，不存在 common skill。 | 命令契约必须同步更新两份，不创建 `skills/ralph-preset-common`。 | 高 |
| E22 | `skills/ralph-preset-review/fixtures/`、`tests/test_skill_anchors.py` | review 有 fixture 和 anchor 测试，但没有 runtime liveness/closure negative fixture。 | 新增通用无输出/未闭合 fixture，并扩展 anchor，不绑定具体事故或 preset。 | 高 |
| E23 | `crates/ralph-core/data/ralph-tools-cmdref.md`、`scripts/ralph-zsh-plugin.zsh` | agent-facing command reference 和 preset completion 分别维护；completion 当前没有 verify。 | 新命令必须同步 agent 可执行说明、operator 文档和 completion。 | 高 |
| E24 | 最近历史 `10f91ecc`、`f0c26aba`、`bb70b710`、`22ad7464`、`c6eaeed3` | 静态 contract、required-topic、runtime failure closure 和红队事件链近期均有独立修复。 | 动态验证应覆盖静态通过但运行时不收敛的类别，不能只增加静态 finding。 | 中 |

### 2.3 受影响范围

**已确认生产入口/模块：**`crates/ralph-core/src/lib.rs`、`crates/ralph-core/src/event_loop/*`、`crates/ralph-core/src/execution_contract/*`、`crates/ralph-cli/src/commands/preset.rs`、`crates/ralph-cli/src/main.rs`、`crates/ralph-cli/src/cli/shared.rs`、`crates/ralph-cli/src/preflight.rs`。

**已确认测试范围：**新增 `crates/ralph-core/tests/preset_verify.rs`（计划新增）；新增 `crates/ralph-cli/tests/integration_preset_verify.rs`（计划新增）；现有 `crates/ralph-core/tests/scenarios.rs` 及 `crates/ralph-core/src/event_loop/tests/progress_steward.rs`、`plan_blocked_termination.rs`、`default_publishes.rs` 作为回归；现有 `crates/ralph-cli/src/commands/preset.rs` 单元测试作为 check 不变回归。

**已确认配置/数据边界：**preset/core YAML、schema 文件、scenario YAML、临时 `.ralph` 事件文件、`LoopContext` workspace；不涉及数据库迁移或外部服务。

**已确认文档/skill 范围：**两套 preset operator skill 的 SKILL/reference/fixture/anchor；`presets/README.md`、`docs/guide/preset-authoring.md`、`crates/ralph-core/data/ralph-tools-cmdref.md`、必要时 `ralph-tools.md`；`scripts/ralph-zsh-plugin.zsh`。

**未确认且不列为事实的范围：**没有确认任何 Web UI、API endpoint、外部数据库消费者或 builtin preset YAML 必须改变，因此本计划不包含这些路径。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | 动态验证放在哪里？ | 改 `preset check`；改 `inspect`；新建 `preset verify` | 新建独立 `ralph preset verify`。 (session-settled: user-approved — chosen over extending `preset check` or `inspect`: 保留静态检查和只读诊断的职责边界。) | E2, E3, E4；用户已明确同意新命令 | check 被 preflight/run gate 复用，加入执行会改变静态语义；inspect 明确是 read-only | 0.99 |
| D2 | 验证是否调用真实 workflow？ | 只解析 replay；复用 stub ScenarioRunner；直接驱动真实 EventLoop | 新增通用 verifier，调用真实 `EventLoop` 公共 API 和 JSONL 处理链。 | E10-E15 | replay 不调度 hat；ScenarioRunner 返回空 events；直接读取私有字段会破坏 API 边界 | 0.96 |
| D3 | 外部项目是否需要 Rust/Cargo？ | 运行目标项目构建；读取目标项目源码；只使用 Ralph CLI 的受控 verifier | 只使用 Ralph CLI、preset/config/schema/scenario；external 模式禁止 Rust/Cargo/source-line 证据。 (session-settled: user-directed — chosen over Rust-dependent author/review: 外部项目没有 Rust，preset 必须可运行。) | E5, E19-E21；用户明确约束 | 目标项目语言与 Ralph workflow 无关；构建会引入网络和环境非确定性 | 0.99 |
| D4 | 场景格式是否复用内部 BDD YAML？ | 直接暴露 `ScenarioYaml`；复用 stub `Scenario`；新增最小公共 verify schema | 新增 `version: 1` + `scenarios` 公共 schema，只保留 start/response/expect/limits，运行时 config 仍来自 `-c/-H`。 | E11-E14；内部结构含测试专属 fixture/supervisor 字段且 helper 在 integration test 私有 | 直接暴露内部 schema 会把测试实现细节变成外部契约；stub 不验证行为 | 0.93 |
| D5 | verifier 如何隔离副作用？ | 在用户 workspace 直接运行；复制整个项目；每个 scenario 使用临时 workspace | 使用 `tempfile` 临时 workspace，构造 `LoopContext::primary`，只写 verifier 所需的 `.ralph` 状态；不复制目标源码。 | E11, E16；现有 BDD 已验证 tempdir 方式 | 直接运行会污染用户 `.ralph`；复制项目违反 external 黑盒和成本约束 | 0.94 |
| D6 | 如何保证 deterministic？ | 真实模型/backend；shell fake backend 子进程；内存脚本 output 驱动 | 在 verifier 内将 scenario response 作为受控 hat output，按离散 `max_steps/no_progress_steps` 结束，不 sleep、不联网、不执行 shell。 | E9-E11、E17；Ralph 已有公开 parser/processing API | 真实 backend 非确定且可能联网；shell 子进程增加跨平台/权限风险 | 0.91 |
| D7 | 报告和退出语义如何固定？ | 复用 `RuntimeContractReport`；只打印日志；定义独立 `PresetVerifyReport` | 独立报告包含静态结果、场景结果、trace digest、last state、failure kind；只有全场景通过退出 0。 | E3、E18；现有 check report 只覆盖静态层 | 复用静态 report 无法表达 no-progress/terminal closure；日志不可机器消费 | 0.92 |
| D8 | builtin/external 如何选择证据？ | 所有来源都允许源码检查；所有来源都禁止源码检查；按 `HatsSource` 分流且未知按 external | builtin 可读 Ralph 源码/BDD/replay 作为补充，actual verify 对两者都必需；file/unknown 按 external。 | E19-E22；用户明确要求区分场景 | 不分流会让外部 skill 依赖 Rust；全部禁用会丢失 builtin 诊断价值 | 0.97 |
| D9 | 是否允许 remote preset verify？ | 允许网络加载；缓存后运行；verify 明确拒绝 Remote | verify 在执行前拒绝 Remote config/hats，要求本地或 builtin 输入；`check` 原有 remote 行为不改。 | E5、E19、R6；确定性和无网络约束 | 网络加载使相同场景结果依赖外部状态，且不属于本需求 | 0.89 |
| D10 | skill 如何判定动态证据充分？ | 只要求 YAML/prompt；只要求 verify pass；静态+场景覆盖+实际结果+closure | review 必须同时拥有严格静态结果、最小场景集合、actual verify 结果和终态闭合证据；builtin 源码只能补充。 | E20-E24；用户已确认 author/review 要尽量前置发现问题 | 单项证据无法覆盖 runtime stall 或 false success | 0.96 |

所有 D1-D10 均达到 0.85；没有需要在 Unit 开始前另行 Spike 的关键决策。

## Planning Contract

### 3.1 新增公共场景契约

只新增一个公开 scenario 文件契约，preset 内容仍由 `-c/--config` 和 `-H/--hats` 提供。场景文件的规范化结构如下，字段名是实现契约而非示例建议：

```yaml
version: 1
scenarios:
  - name: success-path
    responses:
      - hat: producer
        output: |
          <event topic="work.done">{"ok":true}</event>
        success: true
    expect:
      start_event: work.start
      accepted_events: [work.done, LOOP_COMPLETE]
      forbidden_events: []
      terminal: success
      terminal_topic: LOOP_COMPLETE
      payload_fields: {}
    limits:
      max_steps: 8
      no_progress_steps: 2
```

实现必须拒绝未知顶层 `version`、空 scenarios、重复 scenario name、空 response 序列、非正预算、`terminal != none` 却没有 `terminal_topic`、`expect.start_event` 与最终 config starting event 不一致，以及 `payload_fields` 不是对象的输入。`success: false` 只表示这一步的 agent execution outcome，不代表场景整体应该通过；整体通过由 `expect` 决定。

### 3.2 受控 driver 的边界

每个 scenario 的执行顺序固定为：创建临时 workspace 和 `.ralph` 目录 → 克隆/调整已加载 config 的 workspace root → `config.normalize` → `execution_contract::compile` → `EventLoop::from_resolved` → `initialize` → 对 response 序列逐步执行 `next_hat`、`build_prompt`、`process_output(hat, output, success)`、`EventParser::parse`、追加 JSONL、`process_events_from_jsonl` → 收集公开 `ProcessedEvents` 与 termination → 做 expect/limits 判定 → 生成 report 并清理。

场景中的 `hat` 如果存在，必须等于当前 `next_hat`；不相等时立即报告 `scenario_failure`，不得强行把 output 注入另一 hat。缺少 `hat` 时使用真实 `next_hat`，以便 coordinator/isolated 调度都由 EventBus 决定。response 耗尽前未达到终态时不能自动补一条成功事件。

### 3.3 报告契约

`PresetVerifyReport` 是新结构，不能复用 `RuntimeContractReport`。JSON 必须稳定包含：

- `passed: bool`。
- `source_kind: builtin | external`。
- `static: { passed: bool, warnings: usize, errors: usize, findings: [...] }`。
- `scenarios: [{ name, passed, steps, accepted_events, rejected_events, terminal_topic, termination, failure_kind, last_observable_state, trace_digest }]`。
- `failure_kind: null | input_error | static_contract_failure | scenario_failure | runtime_exception | timeout | no_progress | unclosed_terminal`。
- `trace_digest` 为场景输入和观察事件的确定性 digest；报告不泄漏绝对临时路径。

`last_observable_state` 至少包含 `step`、`last_hat`、`last_accepted_topic`、`last_runtime_termination`、`response_index`；它用于诊断 stall，不允许只写“failed”。人类格式和 JSON 必须来自同一报告对象，不允许各自重新推导结论。

### 3.4 静态与动态分层

verify 在运行前调用与 `preset check --strict` 等价的静态 aggregator。静态失败时报告 `static_contract_failure`，不创建 EventLoop，不消费 scenario response。静态通过后才进入 runtime。`preset check` 仍调用原有 `build_report`；`inspect` 不引用 verifier。

### 3.5 skill 分流规则

- source 为 `builtin:*` 时，author/review 可以读取 builtin YAML、schema、Ralph runtime 源码、BDD/replay/smoke 测试；但必须仍执行公开 verify，并把源码标为 supplemental evidence。
- source 为本地文件、未知来源或非 builtin URL 时，author/review 统一按 external 黑盒模式；只读公开 CLI help、preset/config/schema/scenario 和 verify report，不要求 Rust/Cargo、源码行号、内部 module 或私有 ledger。
- skill 的 finding 类别只描述通用能力：静态 contract、scenario schema、无输出无进展、恢复闭合、终态闭合、证据缺失、输入不可复现；禁止硬编码某个 preset/topic/事故路径。

## 4. BDD 行为规格

### Feature: Preset 的确定性可运行性验证

  Background:
    Given 一个本地或 builtin preset 已通过统一 preflight 加载
    And 一个 version 1 verify scenario 文件位于调用方提供的路径
    And scenario 使用有限的 scripted agent output，不调用网络或目标项目构建系统

  Scenario: 正常 workflow 到达声明的成功终态
    Given `expect.start_event` 等于 preset 的 configured starting event
    And response 序列产生期望的 accepted events 和成功 terminal topic
    When operator 执行 `ralph preset verify --scenario <file>`
    Then 事件按实际 EventLoop 顺序被观察到
    And JSON report 的 `passed` 为 true 且该 scenario 的 `failure_kind` 为 null
    And exit code 为 0

  Scenario: 业务失败通过合法 failure terminal 闭合
    Given scenario 的 terminal 类型为 `failure` 或 `blocked`
    And response 序列让 runtime 接受失败/阻断事件
    When operator 执行 verify
    Then report 记录该 terminal topic 和 ordered trace
    And scenario 只有在声明的 failure/blocked terminal 与实际结果一致时通过
    And 不把业务失败误报为成功完成

  Scenario: 激活后无业务输出触发有界失败或合法恢复
    Given response 的 output 为空或只包含非业务文本
    When verifier 推进到 `no_progress_steps` 或 runtime 的恢复边界
    Then verify 在有限步骤内返回 `no_progress`、`timeout` 或合法的 `blocked` terminal 结果
    And 没有合法闭合时 `passed` 为 false
    And `last_observable_state` 包含最后 hat、step 和 accepted topic

  Scenario: 非法事件 payload 被 runtime 拒绝且不会伪造 accepted event
    Given scripted output 包含不符合 event/payload contract 的事件
    When verifier 通过真实 `process_events_from_jsonl` 处理该 output
    Then report 区分 rejected event 与 accepted event
    And 被拒绝 topic 不得出现在 accepted event 序列中
    And 若恢复耗尽，报告明确为失败/阻断闭合结果而不是成功

  Scenario: runtime 生成恢复或阻断事件时验证其真实后继
    Given workflow 进入 missing-event、no-progress 或 contract rejection 路径
    When runtime 生成 recovery/control/blocked event
    Then trace 记录 runtime 生成事件及其来源/termination evidence
    And verifier 检查该事件是否进入允许的后继或 terminal
    And 运行时生成事件没有可行后继时报告 `unclosed_terminal`

  Scenario: scenario 耗尽预算但没有终态
    Given response 序列在 `max_steps` 内没有产生声明 terminal
    When verifier 消耗完 scripted responses 或 step budget
    Then report 为 `timeout` 或 `no_progress`
    And exit code 非零
    And verifier 不追加成功事件、不返回成功

  Scenario: 场景起点与 preset 配置不一致
    Given `expect.start_event` 与最终 config 的 `event_loop.starting_event` 不同
    When operator 执行 verify
    Then verify 在 runtime 构造前返回 `input_error` 或 `scenario_failure`
    And 不调用 EventLoop，不写 scenario runtime trace

  Scenario: external 非 Rust 项目完成验证
    Given `-H` 指向本地 preset 文件，项目没有 Rust、Cargo 或 Ralph 源码
    When external author/reviewer 只使用公开 CLI、scenario 和 report
    Then verify 可以完成静态和受控运行验证
    And skill 不要求读取 Rust 文件、运行 Cargo 或执行目标项目构建

  Scenario: remote source 被拒绝以保持确定性
    Given config 或 hats source 是 remote URL
    When operator 执行 verify
    Then verify 在运行前返回明确的输入错误
    And 不发起网络请求、不创建 workflow runtime

  Scenario: check 与 inspect 的旧职责不变
    Given 一个可加载 preset
    When operator 执行 `ralph preset check` 或 `ralph inspect prompt`
    Then check 只生成静态 contract report
    And inspect 只生成 preview/candidate emit 结果
    And 两者都不消费 verify scenario、不推进真实 workflow

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
|---|---|---|---|---|---|
| 成功终态 | accepted topic 顺序、terminal topic、`passed=true`、exit 0；临时 workspace 清理 | 新增 `crates/ralph-core/tests/preset_verify.rs` 的 real EventLoop test；新增 `crates/ralph-cli/tests/integration_preset_verify.rs` | core integration + CLI integration | Characterization：与现有真实 `run_workflow_guard_scenario` 的基础成功语义对照 | 需要，CLI 主路径一条 |
| failure/blocked 闭合 | failure/blocked terminal 只在 expect 匹配时通过；不能当 success | core integration | integration/state-machine | State-machine：合法 terminal、缺 terminal、错误 terminal 三态 | 不需要第二条 |
| 空 output/no-progress | 空 output 真实经过 `process_output` 和 event reader；有界返回；last state 非空 | core integration | integration | Characterization：`progress_steward`、`default_publishes`、`plan_blocked_termination` | 不需要 |
| 非法 payload | rejected 与 accepted 分离；被拒绝 topic 不进入 accepted 序列；恢复耗尽可观测 | core integration + 既有 event policy tests | integration/contract | Fault injection：缺 required field、格式非法、重复非法 output | 不需要 |
| runtime recovery/terminal | runtime event 被记录，后继或 terminal 可验证；无后继为 `unclosed_terminal` | core integration | state-machine/integration | Characterization：既有 recovery/terminal_closed_guard tests | 不需要 |
| budget exhaustion | max_steps/no_progress_steps 确定性停止，exit 非零，不伪造 success | core unit + integration | unit + integration | Property-based 可选：随机有限空/非空 response 序列，永不超预算 | 不需要 |
| malformed scenario/start mismatch | 解析前置失败，错误分类稳定，不构造 runtime | core unit + CLI integration | unit + integration | Fuzz 不纳入本次；YAML parser 已有 serde 路径，先覆盖边界样本 | 不需要 |
| external non-Rust | 只调用公开 binary/config/preset/scenario，不读取 Rust/Cargo | CLI integration fixture | black-box integration | external fixture 必须不含 Cargo/Rust 路径；review skill anchor 检查 | 需要，CLI binary |
| remote rejection | 无网络请求、非零、runtime 未启动 | CLI unit/integration | integration | 不使用真实 remote；使用 source parser 的 local fake input | 不需要 |
| check/inspect regression | 原有 report/preview 仍通过且 verify scenario 未被消费 | `commands/preset.rs` tests、现有 inspect tests、CLI docs drift | regression | Characterization：保留现有 check no-backend test | 不需要 |

测试不得通过 source-text assertion 验证 runtime；不得使用 `run_scenario` 或 `testing::ScenarioRunner` stub；不得把真实 EventLoop、event parser 或 contract gate mock 掉。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | check 保持静态 | check/inspect regression | `check_remains_static` | report builder existing tests | CLI preset check regression | 否 | E2,E3 |
| R2 | verify 实际推进 workflow | 成功终态 | `verify_success_path` | driver step transition | core real EventLoop + CLI | 是 | E6-E11 |
| R3 | inspect 保持只读 | check/inspect regression | existing inspect tests plus no scenario consumption | N/A | CLI inspect regression | 否 | E4 |
| R4 | 覆盖成功/失败/空输出/非法输出/recovery/terminal | 对应六个场景 | `verify_failure_closure`, `verify_empty_output`, `verify_invalid_payload`, `verify_runtime_recovery` | failure classifier tests | core runtime integration | 否 | E10,E17,E18 |
| R5 | 起点、允许事件、终态、有界停止 | start mismatch、budget exhaustion | `verify_rejects_start_mismatch`, `verify_budget_is_bounded` | schema/limits tests | core integration | 否 | E8,E17 |
| R6 | deterministic 且不依赖 Rust/Cargo/网络 | external non-Rust、remote rejection | CLI black-box tests | source mode/remote guard tests | CLI integration | 是 | E5,E19 |
| R7 | human/JSON 与 failure categories | 每个失败场景 | `verify_report_json_is_machine_readable` | report serialization tests | CLI format/exit tests | 是 | E3,E18 |
| R8 | 暴露结构化空输出/recovery/terminal | 空输出、recovery | `trace_contains_last_state` | ProcessedEvents mapping tests | core integration | 否 | E17,E18 |
| R9 | runtime control/recovery/terminal contract 可验证 | runtime recovery | `verify_runtime_terminal_closure` | terminal classifier tests | core contract integration | 否 | E10,E17 |
| R10 | 不伪造 success 且可复现 | budget exhaustion | `verify_never_succeeds_without_terminal` | digest determinism tests | repeat CLI verify | 是 | E11,E16 |
| R11 | author 判断 builtin/external | external/builtin | skill fixture review | source-kind parser test | CLI source mode integration | 否 | E19-E22 |
| R12 | author 生成/确认最小场景并执行 verify | author workflow | anchor/fixture checks | scenario coverage checker tests | CLI verify command | 是 | E20-E23 |
| R13 | reviewer 同时看静态、场景、结果、闭合 | reviewer workflow | review negative fixture | finding mapping tests | CLI report fixture | 否 | E20,E22 |
| R14 | builtin 可用源码补充但必须 verify | builtin workflow | builtin review fixture | source mode tests | builtin CLI verify | 是 | E20,E24 |
| R15 | external 不要求 Rust/Cargo/source | external non-Rust | external fixture anchor | no-source rule tests | CLI black-box | 是 | E19-E23 |
| R16 | finding 通用化 | negative fixture | generic finding anchor | category enum tests | skill fixture review | 否 | E20,E22 |

## Implementation Units

### 7. 严格串行开发单元

执行顺序固定为：

`Unit 1 → Unit 2 → Unit 3 → Unit 4 → Unit 5`

每个 Unit 必须完成 Acceptance Red、Unit Red、Green、Refactor、Integration、Regression、Close 后才能进入下一个 Unit。

### Unit 1：建立公共 scenario schema 与确定性报告模型

#### 1. Unit 目标

让 verifier 能把 version 1 scenario YAML 解析成严格的 typed input，并对非法场景返回可分类错误；同时固定 `PresetVerifyReport` 的 JSON 序列化字段，使后续 runtime driver 不再自行决定输入/报告契约。

#### 2. 对应需求与 Scenario

- Requirement：R5、R7、R10。
- Scenario：start mismatch、budget exhaustion、malformed scenario。
- Decision：D4、D7。
- Evidence：E13-E15、E18。

#### 3. 外部可观察结果

Verifier library 的 parse/validate 入口对合法 YAML 返回 typed scenario/report 初始对象；对缺 version、空 scenarios、非法 limits、缺 terminal topic、start mismatch 返回稳定分类错误。此 Unit 不推进 EventLoop。

#### 4. 当前行为基线

当前没有 `preset verify`、公共 scenario schema 或 `PresetVerifyReport`；只有测试专属 `ScenarioYaml` 和不验证事件的 `ScenarioRunner`。Acceptance Red 应真实显示新类型/入口不存在，而不是环境错误。

#### 5. 输入与输出

- 输入：scenario YAML 字节、最终 config 的 starting event、source kind。
- 输出：typed scenario list、validated limits、可序列化空报告/结构化 input error。
- 错误：`InputError::Parse`、`SchemaVersion`、`InvalidScenario`、`StartEventMismatch`、`InvalidLimit`。
- 状态变化：无 filesystem/runtime 状态变化。
- 副作用：无网络、无 shell、无 EventLoop 构造。
- 不变量：同一输入得到同一 normalized model 和同一 report serialization；未知场景字段不会静默改变已知字段。

#### 6. 修改位置

- `crates/ralph-core/src/lib.rs`：当前导出 core 公共 API；增加新 verifier module 的导出，不改变现有 event exports。
- `crates/ralph-core/src/preset_verify.rs`：**计划新增**；职责仅限 scenario model/parser/validation、failure enum、report model/serialization，不放 EventLoop driver。
- `crates/ralph-core/tests/preset_verify.rs`：**计划新增**；放 parser/report unit tests，不使用 stub runner。
- `crates/ralph-core/Cargo.toml`：只有确认现有 serde/serde_yaml/sha2 不足时才修改；优先复用已存在依赖，默认不新增依赖。

不修改 `crates/ralph-core/src/testing/scenario.rs`、`smoke_runner.rs`、`runtime_contract.rs` 或 `EventLoop` 行为。

#### 7. 可依赖能力

现有 serde/serde_yaml/serde_json/sha2、公开 `EventParser` 和配置类型；无需前置 Unit。

#### 8. 禁止依赖的未来能力

不得在本 Unit 实现 EventLoop 驱动、CLI dispatch、backend 执行、skill 文档或动态 terminal 判断；不得为了通过 parser test 返回固定成功报告。

#### 9. 验收测试

- `parse_version_1_scenario`: 合法 YAML 得到一条 scenario，保留 response 顺序和空 output。
- `rejects_invalid_scenario_shape`: 缺 version/scenarios/name/responses/expect/limits 均返回 input error。
- `rejects_non_positive_limits`: 0 或负值被拒绝，不能被替换成默认值。
- `rejects_start_event_mismatch`: scenario start 与 config starting event 不一致时失败。
- `verify_report_json_has_stable_fields`: report JSON 包含 `passed/source_kind/static/scenarios/failure_kind/last_observable_state/trace_digest`。

运行：`cargo nextest run -p ralph-core --test preset_verify`。Unit 失败不允许进入 Unit 2。

#### 10. Acceptance Red

首先运行上述新测试；预期失败原因是 `ralph_core::preset_verify` module、scenario parser 和 report type 尚不存在。若失败来自 Rust toolchain、依赖下载、测试没有被编译或 YAML fixture 路径错误，不算有效 Red，必须修正测试入口后重跑。

#### 11. 单元测试拆分

1. typed scenario 必填字段和默认 `success=true`；输入含空 output，期望保留空字符串。
2. version/unknown top-level/duplicate name 校验；Fake 只使用内存 YAML，不 mock parser。
3. limits 正整数和 no-progress ≤ max-steps；不允许通过 clamp 或默认值绕过。
4. start event exact match；使用真实 `RalphConfig` 的 event_loop starting event。
5. report serde round-trip 与 deterministic digest；不测试绝对临时路径。

#### 12. Red → Green → Refactor 顺序

`parse_version_1_scenario Red → 新增 typed model/parser → Green → invalid shape Red → 增加 fail-closed validation → Green → limits/start validation Red → 最小错误分类实现 → Green → report serialization/digest Red → 最小 report 实现 → Green → Refactor`。

#### 13. 最小实现范围

必须实现 version 1 schema、字段校验、错误分类、source kind、report model 和确定性 digest；必须复用现有 serde/sha2；不得实现 runtime execution、CLI exit 或 source fetching。

#### 14. 集成验证

只联合 core config 类型和 serde parser；EventLoop、filesystem、backend 均不接入。运行 `cargo nextest run -p ralph-core --test preset_verify`，预期所有 model/parser/report tests 通过。

#### 15. 风险驱动测试

Characterization：固定当前内部场景 YAML 中合法的 `name/mock_responses/expected` 语义，但不复制其私有字段。Property-based 不纳入本 Unit，因为 parser field set 仍在形成；用边界表覆盖更可读。

#### 16. 回归范围

运行 `cargo nextest run -p ralph-core --test scenarios`，证明未触碰现有 BDD；运行 `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`，证明静态 preset lint 未受 core module 导出影响；运行 `cargo check -p ralph-core` 作为 build/typecheck 预门禁。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/preset_verify.rs` | 新增生产文件 | 公共 schema/report contract | E13-E15,E18 |
| `crates/ralph-core/src/lib.rs` | 修改现有生产文件 | 导出新 module/API | E15 |
| `crates/ralph-core/tests/preset_verify.rs` | 新增测试 | parser/report acceptance | E11-E13 |

#### 18. 完成标准

所有 parser/report tests 通过；错误分类和字段稳定；无 skip/only/削弱断言；现有 scenarios 和 preset_lint 回归通过；没有实现未来 Unit；Evidence/Decision 记录不下降；Unit 可独立提交。

#### 19. 停止条件

如果现有 serde 配置无法表达严格 schema、需要新增依赖、或 report 字段与现有 CLI serialization 约定冲突，停止并补 E/D 记录；不得让 Executor 自行改 schema。

#### 20. 风险与注意事项

真实风险是将内部测试 YAML 误变成公共契约。检测方式是 review 新 schema 是否只包含本 Unit 定义的字段；缓解方式是新建独立 model，保留内部 harness 不变；剩余风险是 future scenario v2 需要新增版本分支，本 Unit 不实现。

### Unit 2：用真实 EventLoop 驱动一个有界 scripted workflow

#### 1. Unit 目标

让一个合法 scenario 的 scripted responses 真实经过 `EventLoop` 调度、prompt 构造、output accounting、事件 parser、JSONL reader 和 accepted event 路由，并返回 ordered trace；这是 verify 的最小可观察运行行为。

#### 2. 对应需求与 Scenario

- Requirement：R2、R4、R5、R8、R9、R10。
- Scenario：正常 workflow 到达成功终态。
- Decision：D2、D5、D6。
- Evidence：E6-E12、E16、E18。

#### 3. 外部可观察结果

对一个 success fixture，driver 返回与实际 runtime accepted event 顺序一致的 trace、实际 hat 顺序、step 数和 terminal evidence；driver 不读取目标项目 Rust/Cargo，不执行 shell。

#### 4. 当前行为基线

当前没有 production verifier；真实 EventLoop 驱动只存在于 `tests/scenarios.rs:1141-1360`。stub runner 的 events 永远为空，不能作为基线。先由新的 core integration test 证明 driver 尚未存在，再实现。

#### 5. 输入与输出

- 输入：Unit 1 validated scenario、preflight 后 config、source kind。
- 输出：`ScenarioTrace`，包含每步 response index/hat/output success、prompt existence、accepted/rejected events、termination、last state、digest。
- 错误：compile failure、EventLoop I/O failure、unexpected hat、response exhaustion、runtime termination。
- 状态变化：仅临时 workspace 的 `.ralph` 文件。
- 副作用：没有目标项目代码/构建文件修改。
- 不变量：每个 scripted response 至多消费一次；事件只能通过 `process_events_from_jsonl` 进入 accepted trace；response 顺序和 accepted event 顺序可复现。

#### 6. 修改位置

- `crates/ralph-core/src/preset_verify.rs`：扩展为 driver；只调用公开 EventLoop API，不访问私有 fields。
- `crates/ralph-core/src/lib.rs`：导出 driver entry point 和 trace types。
- `crates/ralph-core/tests/preset_verify.rs`：增加真实 integration fixtures/tests；创建 tempdir，不能调用 `ScenarioRunner`。

不修改 `crates/ralph-core/tests/scenarios.rs` 的既有 helper，不把测试 helper移动到 production；不修改 EventLoop routing semantics。

#### 7. 可依赖能力

Unit 1 typed model/report；`execution_contract::compile`、`EventLoop::from_resolved`、`LoopContext::primary`、`initialize`、`next_hat`、`build_prompt`、`process_output`、`EventParser`、`process_events_from_jsonl`。

#### 8. 禁止依赖的未来能力

不得在本 Unit 实现 failure category 的完整判定、CLI command、human/json renderer、skill 文档或 remote rejection；只返回足够的 raw structured trace 供 Unit 3/4 使用。

#### 9. 验收测试

- `driver_runs_real_event_loop_success`: fixture 定义 producer/consumer/terminal，assert ordered accepted topics 和 terminal。
- `driver_uses_actual_next_hat`: response 指定错误 hat 时失败；不允许 driver 强行路由。
- `driver_preserves_empty_output`: 空 output 仍调用 process_output 并记录 step。
- `driver_repeats_same_input_deterministically`: 同一 config/scenario 两次 trace digest 相同。
- `driver_never_uses_stub_runner`: test setup 不构造 `ralph_core::testing::ScenarioRunner`，并通过 accepted events 断言真实 runtime。

运行：`cargo nextest run -p ralph-core --test preset_verify`。

#### 10. Acceptance Red

首先运行 `driver_runs_real_event_loop_success`；预期失败是 verifier driver/API 不存在或 trace 为空，而不是仅迭代数量 mismatch。若测试没有创建 temp workspace、没有通过 `process_events_from_jsonl`，其失败不算有效 Red。

#### 11. 单元测试拆分

1. temp workspace/event file lifecycle：真实 `LoopContext::primary`，assert no absolute path in report。
2. compile-before-construction：坏 contract 返回 compile error，`from_resolved` 不被调用。
3. initialization/next_hat：使用 configured starting event 和实际 hat selection。
4. response processing：真实 EventParser + JSONL + ProcessedEvents，禁止 mock accepted event list。
5. trace digest：同输入 deterministic；不包含时间戳和临时绝对路径。

#### 12. Red → Green → Refactor 顺序

`success driver Red → 创建 temp workspace/config/compile/from_resolved 骨架 → Green（能初始化）→ event sequence Red → 接入 next_hat/build_prompt/process_output/parser/JSONL/process_events → Green → empty output Red → 保留空 output 并记录 ProcessedEvents → Green → deterministic digest Red → 过滤非确定字段并计算 digest → Green → Refactor`。

#### 13. 最小实现范围

必须实现真实 driver、temp workspace、compile boundary、逐 response 调度、事件写入/读取、trace 收集和 deterministic digest；必须保留 runtime 对 malformed/rejected event 的真实结果；不实现 CLI。

#### 14. 集成验证

真实联合 core config、EventLoop、EventParser、EventReader、EventBus；可以 Fake 的只有脚本 response 输入和 temp workspace；不得 fake EventLoop/accepted event processing。运行 `cargo nextest run -p ralph-core --test preset_verify` 和 `cargo nextest run -p ralph-core --test scenarios`。

#### 15. 风险驱动测试

Characterization：成功路径与现有 `run_workflow_guard_scenario` 真实链路对照。State-machine：response exhaustion、terminal accepted、unexpected hat 三态。Idempotency：同一 scenario 重跑 digest 相等、不会读取旧 temp state。

#### 16. 回归范围

直接回归 `cargo nextest run -p ralph-core --test scenarios`、`cargo nextest run -p ralph-core --test smoke_runner`、以及分别以 `progress_steward`、`plan_blocked`、`default_publishes` 为过滤词执行 `cargo nextest run -p ralph-core -- <filter>`；构建 `cargo check -p ralph-core`。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/preset_verify.rs` | 修改新增生产文件 | 真实 EventLoop driver/trace | E6-E12,E16,E18 |
| `crates/ralph-core/src/lib.rs` | 修改现有生产文件 | 导出 driver | E15 |
| `crates/ralph-core/tests/preset_verify.rs` | 修改新增测试 | real runtime acceptance | E11-E13 |

#### 18. 完成标准

success/hat mismatch/empty output/determinism tests 通过；trace 来自真实 ProcessedEvents；无 stub、无绕过、无未处理边界；scenarios 和 event-loop 直接回归通过；Unit 可独立提交。

#### 19. 停止条件

如果生产 API 无法在不暴露私有字段的情况下收集所需 trace，停止并增加公共只读方法的决策记录；如果发现 runtime 真实结果与现有 BDD helper 不一致，停止，不把其中一方静默改成另一方。

#### 20. 风险与注意事项

风险是测试 helper 中存在 test-only constructor，而生产 verifier 只能使用 `from_resolved`；检测方式是 default-feature `cargo check` 和生产 CLI 编译；缓解措施是严格使用 compile/from_resolved。剩余风险是某些 preset 依赖真实文件内容，下一 Unit 只允许 verifier 明确报 runtime/input failure，不复制整个项目。

### Unit 3：实现失败分类、终态闭合和报告/退出契约

#### 1. Unit 目标

让 verifier 对 success、failure/blocked、invalid payload、runtime recovery、no-progress、budget exhaustion、unclosed terminal 产生不可混淆的结构化结果，并保证没有 terminal 的场景不能通过。

#### 2. 对应需求与 Scenario

- Requirement：R4、R5、R7、R8、R9、R10。
- Scenario：业务失败闭合、空 output、非法 payload、runtime recovery、budget exhaustion。
- Decision：D6、D7。
- Evidence：E9、E10、E17、E18、E24。

#### 3. 外部可观察结果

同一场景结果在 human/JSON 中使用同一 `failure_kind` 和 last state；runtime 产生的 blocked/recovery 与 verifier 自己预算耗尽可区分；任何未闭合场景均为 `passed=false`。

#### 4. 当前行为基线

现有 EventLoop 已有 no-progress、`plan.blocked(reason=loop_stalled_max_iterations)`、recovery and terminal guard 测试，但没有统一 public verify result。`ProcessedEvents` 已提供 accepted/rejected/contract violation，driver 还未映射成场景 verdict。

#### 5. 输入与输出

- 输入：Unit 2 `ScenarioTrace` 和 Unit 1 `expect/limits`。
- 输出：`ScenarioResult`、全局 `PresetVerifyReport`、failure category、last state、terminal evidence。
- 错误：分类必须按 1.4 定义；runtime exception 保留 error context，但不泄漏临时绝对路径。
- 状态变化：无新增持久化；报告只在 stdout。
- 副作用：失败不能追加成功事件或改变原 preset。
- 不变量：只有 `expect.terminal` 与实际 terminal 一致且所有 expected/forbidden/payload assertions 通过，scenario 才 passed。

#### 6. 修改位置

- `crates/ralph-core/src/preset_verify.rs`：增加 verdict evaluator、terminal/recovery mapping、budget/no-progress logic。
- `crates/ralph-core/tests/preset_verify.rs`：新增 failure/closure tests。

不修改 `event_processing.rs`、`terminal_closed_guard.rs`、`progress_steward` 的 runtime semantics；只消费其公开结果。

#### 7. 可依赖能力

Unit 1 report/schema 和 Unit 2 raw trace；`ProcessedEvents` 的 accepted/rejected/contract fields；既有 termination enums/tests。

#### 8. 禁止依赖的未来能力

不得在本 Unit 加 CLI parser、skill instruction、preset-specific topic rules 或任何网络/时间等待；不得用字符串日志推断 runtime result。

#### 9. 验收测试

- `failure_terminal_must_match_expectation`。
- `blocked_terminal_is_not_success_terminal`。
- `empty_output_is_bounded`。
- `invalid_payload_is_rejected_not_accepted`。
- `runtime_recovery_must_have_terminal_or_allowed_successor`。
- `budget_exhaustion_never_passes`。
- `report_human_and_json_share_verdict`（core renderer input model，不执行 CLI）。

运行：`cargo nextest run -p ralph-core --test preset_verify`。

#### 10. Acceptance Red

先运行 failure fixtures；预期 Red 是 driver 当前只返回 raw trace，无法区分 no-progress、unclosed terminal 和 expected failure。若测试因为 malformed fixture、命令错误或未执行到 evaluator 失败，不算有效 Red。

#### 11. 单元测试拆分

1. terminal classifier：success/failure/blocked/none 与 topic exact match。
2. forbidden/ordered accepted events：重复、缺失、额外事件分别失败。
3. payload field match：JSON object 字段精确匹配；非法 JSON 不能被当作满足。
4. no-progress counter：空/非业务 output 只按 step budget 计数，不 sleep。
5. runtime-vs-verifier termination：runtime blocked 与 max_steps exhaustion 使用不同 category。
6. digest/report repeatability：同 trace 结果 JSON stable。

#### 12. Red → Green → Refactor 顺序

`failure terminal Red → 实现 terminal evaluator → Green → empty/no-progress Red → 实现离散预算判定 → Green → invalid payload Red → 映射 ProcessedEvents rejected/contract fields → Green → unclosed runtime Red → 实现 successor/terminal closure check → Green → report consistency Red → 统一 report object 渲染输入 → Green → Refactor`。

#### 13. 最小实现范围

必须实现所有 1.4 failure categories、ordered event/forbidden/payload assertion、terminal closure、step/no-progress bounds、last state 和 deterministic report fields；不实现 CLI formatting details 和 skill。

#### 14. 集成验证

联合真实 EventLoop trace 和现有 progress/recovery/terminal semantics；允许 Fake 只有 scripted response；必须使用真实 event policy/rejection。运行 core preset_verify、scenarios、相关 event_loop tests。

#### 15. 风险驱动测试

State-machine：terminal 前、terminal 后、duplicate terminal、failure recovery exhausted。Fault injection：missing required field、malformed payload、empty output。Property-based：对有限 response 序列验证 driver 永远不超过 `max_steps`；如果仓库现有 property test harness 不适合，使用确定性边界表，不引入新框架。

#### 16. 回归范围

运行 `cargo nextest run -p ralph-core --test scenarios`，并分别以 `progress_steward`、`plan_blocked_termination`、`default_publishes`、`post_terminal_rejection` 为过滤词执行 `cargo nextest run -p ralph-core -- <filter>`；运行 `cargo check -p ralph-core` 和 `cargo clippy -p ralph-core --all-targets --all-features -- -D warnings`，命令失败不得进入 Unit 4。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/preset_verify.rs` | 修改新增生产文件 | verdict/closure/report mapping | E17,E18,E24 |
| `crates/ralph-core/tests/preset_verify.rs` | 修改新增测试 | failure/recovery/terminal acceptance | E17,E18 |

#### 18. 完成标准

全部 failure/closure tests 通过；空 output 不会无限等待；未闭合不通过；同一 trace human/json 使用同一 verdict；core runtime 回归、build、lint 通过；无假 Green。

#### 19. 停止条件

如果无法仅基于公开 `ProcessedEvents`/termination 可靠区分 runtime blocked 与 verifier budget exhaustion，停止并增加 API 证据；不得通过日志正则或固定 topic 猜测。

#### 20. 风险与注意事项

风险是把业务失败 terminal 错误归类为成功。检测方式是 success/failure/blocked 三组互斥 fixture；缓解是 terminal type + topic 双重匹配。剩余风险是 preset 自定义 terminal 语义复杂，报告应保留 raw topic/termination 供 reviewer 判断，不添加业务特判。

### Unit 4：接入 `ralph preset verify` CLI 与 builtin/external source mode

#### 1. Unit 目标

让 operator 通过公开 CLI 加载现有 preset、先做严格静态检查、再执行 Unit 3 verifier，并得到 human/JSON 输出和正确退出码；同时禁止 remote source 进入 deterministic verify。

#### 2. 对应需求与 Scenario

- Requirement：R1、R2、R3、R6、R7、R10、R11、R14、R15。
- Scenario：external non-Rust、remote rejection、check/inspect regression、正常 CLI verify。
- Decision：D1、D3、D7、D8、D9。
- Evidence：E1-E5、E19、E23。

#### 3. 外部可观察结果

`ralph preset verify --help` 显示 `--scenario` 和 `--format human|json`；`-H builtin:<name>` report 为 builtin，local file report 为 external；静态失败不运行 scenario；动态失败非零；全部通过为零。`preset check` 和 `inspect prompt` 的现有行为不变。

#### 4. 当前行为基线

当前 Clap 解析 `preset check` 等已有子命令但不认识 `verify`；`build_report` 可静态加载配置；CLI 集成已有 preset builtin tests 和 `commands/preset.rs` 的 no-backend checks。Acceptance Red 应是 `verify` 子命令解析失败，而不是环境问题。

#### 5. 输入与输出

- 输入：现有 global config/hats source、required `--scenario PathBuf`、`--format`。
- 输出：Unit 3 report；human 不打印 JSON-only implementation detail，JSON 不混入 tracing/log；source kind stable。
- 错误：remote source/input/static/runtime/scenario 分类沿用 Unit 3；process exit 0/1，不用 `std::process::exit` 隐藏 report generation errors。
- 状态变化：scenario tempdir only；CLI 本身不写调用方 `.ralph`。
- 副作用：不调用真实 backend、不联网、不执行目标项目命令。
- 不变量：`check` 使用原 `build_report`；`inspect` 不引用 verify module 的 execution entry。

#### 6. 修改位置

- `crates/ralph-cli/src/commands/preset.rs:40-68,233-241`：新增 `Verify` args、dispatch 和 renderer，调用 core verifier 与现有 preflight/build_report，不复制 config loader。
- `crates/ralph-cli/tests/integration_preset_verify.rs`：**计划新增**；使用真实 `ralph` binary、临时 non-Rust fixture project、local preset/scenario，验证 exit/stdout/stderr/report。
- `crates/ralph-cli/src/commands/preset.rs` 现有 tests：增加 Clap/help/check regression，不删除 process-exit tests。
- `scripts/ralph-zsh-plugin.zsh`：新增 preset verify completion 和 options，保持现有 completion style。

不修改 `inspect.rs` 的 execution semantics，不修改 `preflight.rs` 的加载规则，不修改 `run` backend adapter。

#### 7. 可依赖能力

Unit 1-3 core verifier；`preflight::load_config_for_preflight`、`HatsSource` source label、现有 `PresetCheckFormat`/`OutputFormat` patterns、`ralph_bin` integration helper。

#### 8. 禁止依赖的未来能力

不得把 verify 接到真实 backend、`run --dry-run` 或 `SmokeRunner`；不得要求 external fixture 有 Cargo/Rust；不得在本 Unit 修改 skills（Unit 5 负责）。

#### 9. 验收测试

- `preset_verify_help_exposes_public_contract`。
- `preset_verify_success_returns_zero_and_json`。
- `preset_verify_static_failure_does_not_consume_scenario`。
- `preset_verify_failure_returns_nonzero_with_category`。
- `preset_verify_builtin_and_file_sources_are_classified`。
- `preset_verify_rejects_remote_source_without_network`。
- `preset_check_still_works_without_backend`。
- `inspect_prompt_does_not_run_scenario`。

运行：`cargo nextest run -p ralph-cli --test integration_preset_verify`；命令语法另跑 `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 覆盖 parser/dispatch subset。

#### 10. Acceptance Red

先运行 `preset_verify_help_exposes_public_contract` 和 binary success test；预期第一项因 Clap 没有 `verify`，第二项因 dispatch 不存在而失败。若 binary 没构建、fixture 无法加载或测试没有执行子命令，不算有效 Red。

#### 11. 单元测试拆分

1. Clap parse：required scenario、format enum、global `-H/-c` 顺序。
2. source mode：`HatsSource::Builtin` 为 builtin，其余 local/unknown 为 external，remote 先拒绝。
3. static gate：mock 只替换已加载 report input，不 mock core verifier；assert static failure prevents driver call。
4. renderer：human/json 都消费同一 report；JSON parseable、无 log pollution。
5. exit mapping：pass 0、任何 failure 1、input/runtime error nonzero。

#### 12. Red → Green → Refactor 顺序

`help parse Red → 加 Verify args → Green → binary success Red → 接 preflight + strict report + core driver → Green → JSON/exit Red → 统一 report renderer和exit mapping → Green → source/remote Red → 加 source guard → Green → check/inspect regression Red → 保持旧分支并补测试 → Green → Refactor`。

#### 13. 最小实现范围

必须实现 CLI args、preflight reuse、strict static gate、core driver call、human/JSON output、exit mapping、source mode、remote rejection、completion；不实现 skill authoring。

#### 14. 集成验证

真实联合 CLI binary、preflight、runtime contract、core verifier、EventLoop 和 local fixtures；backend 可完全缺席；remote 用 source parser/integration fake 验证拒绝，不访问真实 URL。运行 CLI integration 和已有 preset/inspect tests。

#### 15. 风险驱动测试

Contract：CLI JSON fields/exit mapping。Characterization：existing `build_report_no_backend_required`、preset check parse tests、inspect read-only tests。Fault injection：static fail、malformed scenario、runtime rejection、empty output。

#### 16. 回归范围

直接：`cargo nextest run -p ralph-cli --test integration_preset_verify`、`cargo nextest run -p ralph-cli --test integration_preset_builtin`、`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`；相邻：`cargo nextest run -p ralph-cli --bin ralph -- inspect`（按现有 target/subset）、`cargo nextest run -p ralph-core --test scenarios`；文档语法：`./scripts/check-cli-doc-drift.sh`；构建/lint/typecheck：`cargo check --workspace`、`cargo clippy --workspace --all-targets --all-features -- -D warnings`。所有命令失败均阻断下一 Unit。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/commands/preset.rs` | 修改现有生产文件 | Verify args/dispatch/report | E1-E5 |
| `crates/ralph-cli/tests/integration_preset_verify.rs` | 新增测试 | black-box CLI acceptance | E1,E19 |
| `scripts/ralph-zsh-plugin.zsh` | 修改脚本 | preset completion parity | E23 |

#### 18. 完成标准

CLI help、success/failure/static/remote/source mode tests 全通过；check/inspect regression 通过；JSON 可机器解析；exit code 正确；无真实 backend/network；build/lint/typecheck/doc drift 通过；Unit 可独立提交。

#### 19. 停止条件

如果现有 preflight 对 verify 临时 workspace 的 prompt/schema 解析产生未计划兼容问题，停止并记录具体配置来源；不得偷偷复制整个项目或改变 preflight 全局语义。若发现新的公开 CLI consumer，补影响分析后再继续。

#### 20. 风险与注意事项

风险是 CLI 输出被 tracing 污染导致 JSON 不可解析。检测方式是 integration test 直接 parse stdout；缓解是 renderer 使用单一 report 并将日志隔离到 stderr。风险是 zsh completion 与 Clap drift，完成后必须复制脚本到用户插件目录并验证加载；剩余风险是非 zsh shell completion 不在本范围。

### Unit 5：同步 author/review skill、文档、通用 fixtures 并建立 review 门禁

#### 1. Unit 目标

让 author/review skill 在交付判断中强制要求静态检查、最小 verify 场景、实际 verify report 和 terminal closure，并根据 builtin/external source mode 选择证据范围；外部规则不得依赖 Rust。

#### 2. 对应需求与 Scenario

- Requirement：R11-R16，以及 R1-R10 的 operator handoff。
- Scenario：external non-Rust、builtin supplemental evidence、review negative fixture、check/verify command workflow。
- Decision：D3、D8、D10。
- Evidence：E19-E24。

#### 3. 外部可观察结果

author skill 会确认 source mode、生成/确认 success/failure/no-output 最小场景并运行 verify；review skill 会拒绝只有 YAML/prompt/check/inspect 证据而无 actual verify/closure 的 preset；builtin 可增加源码证据；external 说明不出现 Cargo/Rust/source-line 必需步骤。

#### 4. 当前行为基线

现有 author/review skill 有静态 check、AAF、fixture/anchor，但没有 first-class runtime verify 和 liveness/closure finding；review SKILL 明确默认不跑全量 `./scripts/run-tests.sh`。现有 `test_skill_anchors.py` 固定 heading/fixture anchors。

#### 5. 输入与输出

- 输入：preset source label、公开 CLI help、check report、scenario file、verify human/JSON report；builtin 可附源码/BDD/replay evidence。
- 输出：更新后的中文 skill instructions、author/review commands/rubric/checklist、通用 negative fixture/README/anchor tests、命令文档。
- 错误：缺 source mode、缺 scenario、verify failed/no-progress/unclosed terminal、external skill 请求 Rust/Cargo，均形成 generic finding 或停止条件。
- 状态变化：只修改 skill/docs/fixture/test anchor；不修改 runtime state。
- 副作用：无 preset-specific code path；不新增 common skill。
- 不变量：author/review 各自有一份同步命令表和 finding rubric；commands 与 `ralph preset verify --help` 一致。

#### 6. 修改位置

- `skills/ralph-preset-author/SKILL.md` 及其 `references/{commands,finding-rubric,author-checklist,patterns,prompt-visibility,agent-native-model}.md`：增加 source mode、scenario minimum、verify evidence、停止条件。
- `skills/ralph-preset-review/SKILL.md` 及其 `references/{commands,finding-rubric,author-checklist,patterns,prompt-visibility,agent-skill-audit,agent-native-model}.md`：增加 static+dynamic+closure gate、builtin/external boundary、generic findings。
- `skills/ralph-preset-review/fixtures/README.md`、**计划新增**通用 runtime verification negative fixture：覆盖空 output/no-progress/unclosed terminal，不引用具体事故/preset/topic。
- `skills/ralph-preset-review/tests/test_skill_anchors.py`：增加 verify command、source-mode、generic finding anchors；测试结构化 anchor，不锁定 prompt 全文。
- `crates/ralph-core/data/ralph-tools-cmdref.md` 及必要的 `ralph-tools.md`：增加 agent 可执行的 verify 命令说明，写触发条件、命令、字段来源、失败停止条件，不写私有实现。
- `presets/README.md`、`docs/guide/preset-authoring.md`：补充 operator-facing check/inspect/verify 职责和 scenario 契约。

不修改 `skills/ralph-preset-common`（不存在且禁止重新创建）；不在注入文档写 Rust 源码路径、事故报告、内部 ledger、具体 preset/topic。

#### 7. 可依赖能力

Unit 4 的真实 CLI help、JSON report、exit semantics；AGENTS 规定的 skill allowed edit paths；现有 fixture/anchor conventions。

#### 8. 禁止依赖的未来能力

不得先实现未存在的 verify 命令文案或伪造通过结果；不得把 review 全量 `run-tests` 当成每个 external 项目的前置条件；不得要求 skill 读取 Rust 源码来判断 external preset。

#### 9. 验收测试

- `test_skill_anchors.py` 新 anchor：两份 commands 都出现 `preset verify`、source mode、dynamic closure gate 和 generic finding anchors。
- negative fixture review：静态拓扑可通过但 scripted no-output/unclosed terminal 必须进入动态 finding。
- external wording check：author/review external instructions 不出现 Cargo/Rust/source-line 作为必需动作。
- builtin wording check：允许 supplemental source/BDD/replay，但仍要求 actual verify。
- command docs drift：commands 文档字段/参数与 `ralph preset verify --help` 一致。

运行：`python3 -m venv .venv` 仅在仓库已有 Python 测试入口需要时使用；实际按仓库既有 `.venv` 规则运行 `python -m pytest skills/ralph-preset-review/tests/test_skill_anchors.py`；`./scripts/check-aaf-fixtures.sh`；`./scripts/check-cli-doc-drift.sh`。不能用纯文本 grep 代替 fixture/runtime 验收。

#### 10. Acceptance Red

先运行现有 anchor/fixture 流程并新增期望 anchors；预期 Red 是两套 skill 缺少 `preset verify`、source-mode 分流和 closure finding。若测试失败原因是 fixture YAML 无法解析或 pytest 未找到文件，不算有效 Red。

#### 11. 单元测试拆分

1. author commands/rubric anchor：verify command and failure categories。
2. review commands/rubric anchor：static+dynamic+closure and source mode。
3. external negative fixture：不含 Rust/Cargo 依赖，静态 pass + runtime no-output fail。
4. builtin fixture：可引用 source supplement，但 absence of actual verify remains finding。
5. fixture README/anchor：generic wording，不锁定全段 prompt 文本。

#### 12. Red → Green → Refactor 顺序

`new anchors Red → 更新两份 commands/finding-rubric/SKILL 最小规则 → Green → external/builtin wording Red → 增加 source-mode 分流 → Green → no-output/unclosed fixture Red → 新增通用 fixture 与 review path → Green → docs/help drift Red → 同步 cmdref/operator docs/zsh completion → Green → Refactor`。

#### 13. 最小实现范围

必须同步两套 skill 的命令表和 finding rubric、更新 workflow guardrails、增加 generic fixture/anchor、更新 agent/operator docs 和 completion；不增加 preset-specific rule、不要求 Rust external、不改 runtime。

#### 14. 集成验证

联合真实 `ralph preset verify --help`、skill fixture parser/anchor、`check-aaf-fixtures.sh`、CLI doc drift script；builtin fixture 可在 Ralph repo 中使用源码补充，external fixture 必须只使用公开 CLI/文件/report。

#### 15. 风险驱动测试

Characterization：现有 negative fixture 仍能触发原有 static AAF findings。Regression：现有 anchor heading 和 fixture IDs 不被删除。Contract：commands 文档与 Clap help 一致。不会新增 source-text-only preset YAML tests。

#### 16. 回归范围

运行 skill anchor、AAF fixture、CLI doc drift；运行 `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`、`cargo nextest run -p ralph-core --test scenarios`；运行完整 `./scripts/run-tests.sh` 作为最终 repo regression；如果相关 builtin preset/manifest 未改动，不新增 builtin parity scope，但必须确认 `git diff` 未误改 preset/schema。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `skills/ralph-preset-author/SKILL.md` 与 allowed references | 修改文档 | author dynamic gate/source mode | E20,E21 |
| `skills/ralph-preset-review/SKILL.md` 与 allowed references | 修改文档 | review dynamic closure/finding | E20-E22 |
| `skills/ralph-preset-review/fixtures/README.md` 与新 generic fixture | 修改/新增 fixture | no-output/unclosed negative path | E17,E22 |
| `skills/ralph-preset-review/tests/test_skill_anchors.py` | 修改测试 | skill contract anchors | E22 |
| `crates/ralph-core/data/ralph-tools-cmdref.md`、`ralph-tools.md`（如命令索引受影响） | 修改 agent docs | 可执行 command contract | E23 |
| `presets/README.md`、`docs/guide/preset-authoring.md` | 修改 operator docs | check/inspect/verify boundary | E2-E5 |
| `scripts/ralph-zsh-plugin.zsh` | 已在 Unit 4 修改；本 Unit 复核 | completion/doc parity | E23 |

#### 18. 完成标准

两套 skill 的 commands/finding-rubric 同步；external 不依赖 Rust；builtin source supplement 明确为可选补充；generic fixture 能捕获 no-output/unclosed terminal；anchor/AAF/doc drift/full baseline 全通过；没有新增 skip/only、弱化断言或 plan-specific wording；Unit 可独立提交。

#### 19. 停止条件

如果 skill allowed edit path 与仓库实际目录冲突、anchor 依赖全文 byte equality、或 commands 与 help 无法一致，停止并记录证据；不得创建 common skill 或用一次性事故名绕过通用性要求。

#### 20. 风险与注意事项

最大风险是把“可以读 builtin Rust”写成“所有项目都要读 Rust”。检测方式是逐条审查 external 分支；缓解是 source unknown fail-closed 到 external。第二个风险是 skill 只要求命令存在不要求 closure；用 generic negative fixture 强制 dynamic evidence。剩余风险是现有 skill anchor 测试对文本布局敏感，修改时只保留稳定 heading/contract anchors。

## 8. Unit 串行依赖图

```text
Unit 1
  ↓ validated public scenario/report contract
Unit 2
  ↓ real EventLoop trace and deterministic bounded driver
Unit 3
  ↓ failure categories, terminal closure, final report semantics
Unit 4
  ↓ public CLI, source mode, exit/output contract
Unit 5
  ↓ author/review skill and documentation gate
```

- Unit 2 依赖 Unit 1 的 typed scenario/limits/report fields；不能交换，因为 driver 不应自行决定场景格式。
- Unit 3 依赖 Unit 2 的真实 trace；不能先做，因为 failure classifier 若没有 runtime accepted/rejected/termination evidence 会退化成日志猜测。
- Unit 4 依赖 Unit 3 的稳定 report/exit mapping；不能先做，因为 CLI 不能自行定义失败分类。
- Unit 5 依赖 Unit 4 的真实 help/report；不能先做，因为 skill 命令表和验收规则不能引用尚不存在的 CLI 契约。
- 每个 Unit 只使用已完成前置能力，不提前实现后续 CLI、skill 或 preset-specific behavior。

## Verification Contract

### 9. 执行命令清单

以下命令是实现阶段的真实仓库命令。命令失败时禁止进入同一 Unit 的下一步；除最后明确标注的完整 baseline 外，不得用裸 `cargo test` 替代 nextest。

| 命令 | 运行时机 | 验证目的 | 预期结果 | 失败处理 |
|---|---|---|---|---|
| `cargo nextest run -p ralph-core --test preset_verify` | Unit 1-3 每次 Red/Green/Close | parser、driver、failure/closure 全部 acceptance | 新增 core tests 全绿 | 停止当前 Unit，检查真实 Red/实现，不跳过 |
| `cargo nextest run -p ralph-cli --test integration_preset_verify` | Unit 4 每次 CLI Green/Close | binary 真实命令、source mode、human/json、exit | CLI integration 全绿 | 不得进入 docs/skill Unit |
| `cargo nextest run -p ralph-cli --test integration_preset_builtin` | Unit 4 regression | builtin list/show/source 不回归 | 既有 builtin integration 全绿 | 修复或停止，不改断言 |
| `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | Unit 1/4/5 | preset CLI lint subset和命令 binary | 通过 | 视为静态回归失败 |
| `cargo nextest run -p ralph-core --test scenarios` | Unit 2-5 regression | 真实 BDD workflow unchanged | 既有 scenarios 全绿 | 停止，检查 EventLoop side effect |
| 分别执行 `cargo nextest run -p ralph-core -- progress_steward`、`-- plan_blocked_termination`、`-- default_publishes`、`-- post_terminal_rejection` | Unit 2-3 | no-progress/recovery/terminal adjacent tests | 相关测试全绿 | 停止并更新影响分析 |
| `cargo check -p ralph-core` | Unit 1-3 | default production constructor/API boundary | 通过 | 不得用 test-support 构造器掩盖 |
| `cargo check --workspace` | Unit 4 Close | workspace build/typecheck | 通过 | 阻断 |
| `cargo clippy -p ralph-core --all-targets --all-features -- -D warnings` | Unit 3 Close | core lint | 通过 | 阻断 |
| `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Unit 4/5 Close | workspace lint | 通过 | 阻断 |
| `python -m pytest skills/ralph-preset-review/tests/test_skill_anchors.py` | Unit 5 Red/Green | skill anchor behavior | 通过 | 使用仓库 `.venv`，失败停止 |
| `./scripts/check-aaf-fixtures.sh` | Unit 5 | review fixture contract | 通过 | 修复 fixture/skill，不弱化断言 |
| `./scripts/check-cli-doc-drift.sh` | Unit 4/5 | command docs/help line drift | 通过 | 同步 docs/help |
| `./scripts/run-tests.sh` | Unit 5 最终质量门禁 | full nextest + documented workspace baseline | 全量通过 | 不声明完成；必要时按 AGENTS serial fallback 规则调查 |
| `cargo run -p ralph-e2e -- --mock` | 仅当新增/受影响 E2E 被纳入 | mock E2E 主路径 | 通过 | 若没有新增 E2E，不虚构为每 Unit 必跑 |

## 10. 最终质量门禁

实施完成前必须满足：

- 所有 BDD scenario、core verifier tests、CLI integration tests 通过。
- 每个 R-ID 至少有一个可执行测试，每个 scenario 追踪到 Unit。
- `check` 仍静态、`inspect` 仍只读、verify 才执行受控 workflow。
- 空 output、非法 payload、runtime recovery、failure/blocked terminal、budget exhaustion 和 unclosed terminal 都有真实 runtime evidence。
- external 规则不要求 Rust/Cargo/source；builtin source evidence 只是补充，actual verify 仍必需。
- JSON report 可解析、字段稳定、trace digest 可复现；human/JSON verdict 一致。
- 无真实模型、网络、shell backend 或目标项目 build dependency。
- 没有新增 skip、only、ignore、无解释 snapshot/golden、弱化断言或固定返回值绕过真实逻辑。
- build、lint、typecheck、CLI doc drift、skill anchor、AAF fixture 和最终 `./scripts/run-tests.sh` 通过。
- 没有未处理 BLOCKED decision；所有实现关键决策仍 ≥ 0.85。
- 每个 Unit 按 1→5 串行完成完整 TDD 闭环，并形成独立提交边界。
- 清理所有失败尝试留下的 dead code、临时 fixture、调试输出和未使用 API；不能把 abandoned implementation 留在 diff。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 有源码入口、调用链、真实 Red、最小修改边界、命令和 Unit Close；没有阶段/周次列表。 |
| Executor 是否仍需做关键设计决策 | 否 | D1-D10 已选定 CLI、schema、driver、隔离、报告、source mode 和 skill gate。 |
| 所有文件和接口是否有代码库证据 | 是 | 现有路径均有 E1-E24；新增路径明确标注“计划新增”。 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1-D10 为 0.89–0.99，依据列出直接源码/测试/用户决策。 |
| 是否存在未处理的低置信度假设 | 否 | 1.5 明确无阻塞待验证假设；发现 API 冲突时有停止条件。 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 schema/report、U2 real trace、U3 verdict closure、U4 CLI、U5 skill gate，边界不交叠。 |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 有明确 test entry、Red、命令和 regression。 |
| 每个 Unit 是否有真实 Red | 是 | 每个 Unit 的 Acceptance Red 写明缺失能力和无效 Red 排除条件。 |
| 每个 Unit 是否包含回归范围 | 是 | 每个 Unit 第 16 项列出现有 scenarios、CLI、lint/build/doc drift 范围。 |
| 是否存在未来 Unit 依赖 | 否 | 依赖图只允许前置 Unit；每个 Unit 明确禁止提前实现未来能力。 |
| 是否存在泛化任务描述 | 否 | 没有“完善逻辑/添加测试”等空泛动作，均指向对象、行为、断言和命令。 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 第 5、6 节和各 Unit 第 2/9 项建立映射。 |
| 所有关键决策是否有 Evidence | 是 | D1-D10 均关联 E-ID，用户决定有 session-settled 标记。 |
| 计划是否可以严格串行执行 | 是 | Unit 1→2→3→4→5，每个 Unit 必须 Close 后进入后一个。 |

## Definition of Done

全局完成条件是第 10 节全部通过；Unit 完成条件是各 Unit 第 18 节全部满足。任何第 7 节停止条件触发都必须暂停当前 Unit，新增 Evidence、重新比较方案、更新 Decision 置信度和后续 Unit，不能用猜测继续执行。
