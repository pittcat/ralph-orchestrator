---
title: Run Diagnosis Trace Debug Enhancement - Implementation Plan
type: feat
date: 2026-08-12
topic: run-diagnosis-trace-debug-enhancement
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-brainstorm
execution: code
---

# Run Diagnosis Trace Debug Enhancement：实现计划

## 0. 计划状态

**READY**。本计划的实现关键决策均已完成仓库调查，置信度不低于 0.85；没有把文件布局、schema、兼容回退、开关语义或错误处理留给 Executor 临时决定。

- **代码库基线**：分支 `pittcat-dev`，HEAD `dc4f5ff3`（`修复任务恢复路由与去重逻辑`）。工作树已有用户变更：`CONCEPTS.md`；本计划文件为新增/重写文件，不覆盖 Rust 生产代码。
- **调查范围**：`DiagnosticsCollector` 激活矩阵与日志器、现有 `trace.jsonl`、diagnosis envelope/journal/reporter、`ralph diagnose` CLI、loop runner 生命周期、EventLoop 事件/activation/recovery/timeout 入口、`run-diagnosis` skill 及 references、README/运行时诊断文档/agent-facing recovery 文档、相关测试、Git 历史。
- **已执行的验证命令**：
  - `git rev-parse --show-toplevel`
  - `git status --short`
  - `git log -8 --oneline --decorate`
  - `rg` / `sed` 对生产入口、调用链、测试和文档的只读调查
  - `wc -l` 检查受影响大文件规模
  - 已有基线记录：前置调查中执行过 `cargo nextest run -p ralph-core -- diagnosis`，结果为 134 passed、4568 skipped；该结果只证明当前 diagnosis 相关基线，不证明本计划功能已实现。
- **计划阶段未执行**：新增验收测试不存在，不能提前运行；未执行实现后的 nextest、build、clippy、fmt、CLI doc drift、skill Python contract test。
- **阻塞项**：无。`runtime-trace.jsonl`、`feedback.jsonl`、`diagnosis-input.json` 的位置、兼容策略、metadata 生命周期、feedback source、fixed report shape 和 off/on comparator 已由 D1–D16 固定。

## 1. 功能目标

### 1.1 业务目标与调用方

- **业务目标**：保留现有“完整 run → `run-diagnosis` → 阅读报告 → 人工决定修复”的使用形式；缩短诊断前的人工盘点链路，让运行中的证据在 run 结束后可直接被诊断 skill 消费。
- **用户**：Operator 启动完整 run，随后调用 `run-diagnosis` skill，阅读 Markdown 报告及其结构化诊断结果。
- **运行时调用方**：`ralph-cli` 的 loop runner、`ralph-core` 的 EventLoop/DiagnosticsCollector。
- **诊断调用方**：`skills/ralph-run-diagnosis/SKILL.md` 及其 references；`ralph diagnose --format json|markdown` 作为已有确定性 session reporter。

### 1.2 当前行为与目标行为

- **当前行为**：启用诊断时已经存在 session 目录、`trace.jsonl`、recovery/drift/orchestration/errors 等产物；`trace.jsonl` 由 `DiagnosticTraceLayer` 捕获 tracing 文本，但 reporter 的 `load_session` 不读取它，也没有统一 bundle/index；skill 每次仍需先按 Tier 手工重建产物关系。
- **目标行为**：启用诊断的 run 在 session 中生成原子更新的 `diagnosis-input.json`，并旁路追加 `runtime-trace.jsonl` 与 `feedback.jsonl`。`run-diagnosis` Phase 0 先读 bundle、trace、feedback，再以旧 raw artifacts 做核验；缺 bundle 时按旧流程回退。
- **行为差异**：新增仅观察的文件和报告字段；不改变业务事件接纳、hat 选择、调度、恢复目标、重试预算、超时判定、终止原因、退出码或旧 raw artifacts。

### 1.3 输入、输出和状态变化

- **输入**：现有诊断开关（`RALPH_DIAGNOSTICS=1` 或 `telemetry.runtime_diagnosis.enabled && write_artifacts`）、已解析 `RalphConfig`、`config_path`、`hats_source_label`、loop id、workspace、当前 Git HEAD、EventLoop 的 accepted/rejected/recovery/activation/timeout/termination 事实。
- **新增输出**（相对 session 目录的路径）：
  - `diagnosis-input.json`：schema `run-diagnosis-input/v1`，自描述 manifest/index。
  - `runtime-trace.jsonl`：schema `run-diagnosis-trace/v1`，只记录运行时结构化生命周期事件。
  - `feedback.jsonl`：schema `run-diagnosis-feedback/v1`，按 `retry_key`/`diagnosis_id` 记录发现、证据、action、validation、final 状态。
- **保留输出**：已有 `trace.jsonl`、`recovery.jsonl`、`drift.jsonl`、`orchestration.jsonl`、`errors.jsonl`、`diagnosis-summary.json`、`active-activations.json` 不删除、不改既有 schema。
- **状态变化**：只增加 DiagnosticsCollector 内部的旁路 logger 和 bundle metadata；EventLoop/runner 的业务状态不增加任何影响决策的字段。
- **错误语义**：诊断文件创建、序列化、追加、刷新、manifest 更新失败均转换为 warning；collector 内存状态变为 `degraded`。若 manifest 仍可更新则落盘 `degraded`，若 manifest 本身不可创建/替换，则 reporter 只能以 `missing` + warning/evidence gap 表示，绝不伪称 degraded 已落盘；不得向 loop 主路径返回新错误。reporter 对坏行/缺文件保留 warning 并降低证据完备度。
- **兼容性**：旧 session 没有 `diagnosis-input.json` 时沿用当前 `load_session`/skill Tier inventory；旧 `human.guidance` 只作为历史/replay/compat 证据，不被新报告推荐为当前入口；`task.resume` 保留为当前 runtime recovery transport，但不再被描述为人工 guidance 通道。
- **性能**：每条新增记录是有限字段 JSONL；复用当前同步、flush、best-effort logger 语义，不增加外部网络依赖、不引入无限重试；bundle 使用现有 `NamedTempFile::persist` 原子替换。
- **安全/权限**：manifest 只写 workspace-relative artifact refs、配置/preset 的 hash 和已存在的运行元数据；不写 prompt 全文、agent secret 或无限大小 payload；`snippet`/message 使用现有 bounded display 规则。

### 1.4 本次范围、非目标与约束

- **范围**：session 输入 manifest；结构化 runtime trace；feedback lifecycle journal；bundle-first reporter；Markdown/JSON 诊断字段；`run-diagnosis` skill Phase 0/报告模板/references；README、runtime diagnosis guide、troubleshooting、agent-facing recovery 术语对齐；关闭/失败/旧数据/normal/failure/replay/wave/supervisor 回归。
- **非目标**：运行中人工控制台/UI；自动生成或执行下一轮 Ralph plan；改变 recovery runtime；删除 `task.resume` runtime transport；删除历史兼容 fixture；引入外部 tracing/数据库/新网络服务；新的 CLI 子命令或新的用户配置开关。
- **已知约束**：测试必须使用 `cargo nextest run` 系列；BDD 必须走真实 `run_workflow_guard_scenario`；不手工修改 `.ralph` 运行状态；单源码文件不超过 5000 行，新增职责放入独立 diagnostics 子模块。

### 1.5 已确认与待验证假设

**已确认假设**

- 诊断 session 目录由 `DiagnosticsCollector::with_options` 统一创建，并可被 CLI tracing layer 与 EventLoop 共享；完整 run metadata 需要在 `main.rs` 的早期 collector 创建后，由 `run_loop_impl_inner` 的已解析 config/context 二阶段补齐。
- `trace.jsonl` 已由 CLI 全局 `DiagnosticTraceLayer` 写入，但 collector 没有持有同一 writer；因此不能在该文件上并发追加结构化 runtime records。
- `record_recovery_envelope` 是 recovery envelope 进入 diagnostics/responder 的集中入口；EventLoop activation、`process_parse_result`、runner timeout、runner termination 都有可确认入口。

**不保留待验证假设**：原 requirements 中的文件布局、schema 版本、回退方式、性能策略、metadata 生命周期、feedback 入口、JSON 字段映射和 off/on 差分协议已分别由 D1–D16 固定，并在对应 Unit 的 Red/集成测试中验证。

## 2. 代码库现状与证据

### 2.1 当前实现入口

1. **诊断启用入口**：`crates/ralph-cli/src/main.rs` 创建唯一 `DiagnosticsCollector`；开关来自 `RALPH_DIAGNOSTICS=1` 或 `read_telemetry_write_artifacts(ralph.yml)`。
2. **collector 生命周期**：`crates/ralph-core/src/diagnostics/mod.rs::DiagnosticsCollector::with_options` 创建 session 和现有 logger；`LoopContext::with_prebuilt_diagnostics` 把同一 collector 传入 EventLoop。
3. **原始 trace**：`crates/ralph-core/src/diagnostics/trace_layer.rs::DiagnosticTraceLayer` 把 tracing event 写到 `trace.jsonl`；`main.rs` 安装该 layer。
4. **事件入口**：`crates/ralph-core/src/event_loop/parse_and_emit.rs::process_events_from_jsonl` → `process_parse_result`，这是 JSONL 事件验证、接纳、拒收、bus publish 的集中路径。
5. **activation 入口**：`crates/ralph-core/src/event_loop/event_processing.rs` 中 `hat_lifecycle_tracker.activate` 记录 active hat activation。
6. **recovery 入口**：`crates/ralph-core/src/event_loop/prompt_injection.rs::record_recovery_envelope` 同时写 recovery journal、orchestration audit 并更新 responder。
7. **timeout/termination 入口**：`crates/ralph-cli/src/loop_runner/inner.rs` 处理 `outcome.watchdog_timeout`，并在 `finalize_recovery_diagnosis` → `write_termination_diagnostics` 写 summary/active activations/session pointer。
8. **现有 reporter**：`crates/ralph-core/src/diagnosis/reporter.rs::load_session` 读取 summary/recovery/drift/orchestration/errors/active activations，不读取 `trace.jsonl`；`Report::from_session`、`render_markdown`、`render_json` 是稳定输出入口。
9. **CLI 入口**：`crates/ralph-cli/src/commands/diagnose.rs` 提供 `--session`、`--format markdown|json`、`--output`、`--diagnostics-root`、`--legacy` 等现有参数，不新增命令。
10. **skill 入口**：`skills/ralph-run-diagnosis/SKILL.md` 当前 Phase 0 先按 `current-events` 和 Tier S/A/B/C 盘点，再进行对账、源码归因和报告；`references/artifact-discovery.md` 与 `report-template.md` 是产物和报告结构入口。

### 2.2 Evidence Ledger

| ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `crates/ralph-core/src/diagnostics/mod.rs::DiagnosticsOptions`、`with_options` | 已有 disabled/minimal/full/trace-only 激活矩阵；诊断默认可 no-op。 | 复用现有开关，不新增 flag；新增 logger 只在 full/minimal 生效。 | 高 |
| E2 | `crates/ralph-cli/src/main.rs:285-365`、`LoopContext::with_prebuilt_diagnostics` | CLI 与 EventLoop共享一个 collector/session；初始化失败已有 warning 后继续语义。 | 新 writer 必须挂在 collector 上，初始化失败不得阻塞 run。 | 高 |
| E3 | `crates/ralph-core/src/diagnostics/trace_layer.rs`、`crates/ralph-cli/tests/trace_layer_integration_test.rs` | `trace.jsonl` 是全局 tracing layer 输出，已有独立 writer、上下文和测试。 | 保留 `trace.jsonl`；结构化 runtime trace 使用独立 `runtime-trace.jsonl`，避免多 writer 冲突。 | 高 |
| E4 | `crates/ralph-core/src/diagnostics/mod.rs::write_diagnosis_summary_seed/write_active_activations` | 诊断写入使用原子 tempfile + persist，失败只 warning。 | `diagnosis-input.json` 复用相同原子写入契约。 | 高 |
| E5 | `crates/ralph-core/src/diagnosis/envelope.rs`、`journal.rs` | recovery 已有 `diagnosis_id`、`retry_key`、attempt、outcome、evidence refs。 | feedback lifecycle 以这些稳定身份关联，不重新发明 recovery identity。 | 高 |
| E6 | `crates/ralph-core/src/event_loop/prompt_injection.rs::record_recovery_envelope` | recovery envelope 有集中记录入口；该入口从 runtime 侧可追加旁路记录。 | feedback 的 discovered/evidence 记录放在此入口，不能在各个拒收分支重复实现。 | 高 |
| E7 | `crates/ralph-core/src/event_loop/parse_and_emit.rs::process_parse_result` | 该函数集中完成 raw event 处理、accepted stream、rejection、ledger batch commit。 | runtime trace 的 batch/event/commit 摘要只在此入口采集，不能改校验顺序。 | 高 |
| E8 | `crates/ralph-core/src/event_loop/event_processing.rs` | `hat_lifecycle_tracker.activate` 是 activation 事实入口。 | activation trace 只在 tracker 调用旁路记录。 | 高 |
| E9 | `crates/ralph-cli/src/loop_runner/inner.rs` | watchdog timeout 和最终 summary/termination 都有集中 runner 入口。 | timeout/termination trace 在 runner 记录，不修改 backend outcome 或终止分支。 | 高 |
| E10 | `crates/ralph-core/src/diagnosis/reporter.rs::load_session` | 当前 reporter 对缺失/坏行采用 warnings + 尽可能出报告；旧 recovery 还有 workspace fallback。 | 新 bundle/trace/feedback 必须沿用 fail-soft 读取和 legacy fallback。 | 高 |
| E11 | `crates/ralph-cli/src/commands/diagnose.rs` | `ralph diagnose` 已有 markdown/json 输出、session selector 和 output 参数。 | 不新增 CLI；只扩展已有 JSON/Markdown 字段。 | 高 |
| E12 | `skills/ralph-run-diagnosis/SKILL.md` 与 references | skill 已有 Phase 0、diagnostics mode、capability inference、history disabled gate、四问和 confidence gate。 | 只把 bundle-first 插入 Phase 0，保留历史询问和深度归因流程。 | 高 |
| E13 | `docs/solutions/state-management/2026-08-02-disable-mode-expected-observation-artifacts.md` | DISABLED 模式缺诊断 artifacts 是预期，不能被诊断为机制故障。 | report 必须记录 diagnostics mode 和 evidence cap；关闭模式不创建新文件。 | 高 |
| E14 | `crates/ralph-core/src/event_loop/resume_routing.rs`、HEAD `dc4f5ff3` | 当前 runtime 仍使用 `task.resume`，且近期修复了 target、dedup、retry routing。 | 不删除/替换 task.resume；只修正文档中“已被完全取代”的错误说法。 | 高 |
| E15 | `crates/ralph-proto/src/topics.rs`、policy/drift tests | `human.guidance` 已不是当前 orchestrator control topic，但历史/compat 字符串仍存在。 | 新文档将其标历史/兼容，不删除旧 fixture 或 runtime 兼容解析。 | 高 |
| E16 | `crates/ralph-core/src/diagnosis/reporter.rs`、`DIAGNOSE_JSON_SCHEMA_VERSION` | reporter 仅在非 additive 改形时要求 bump schema。 | 新字段采用 additive JSON，保留 schema version `1`；原字段与旧消费者不变。 | 高 |
| E17 | `crates/ralph-core/Cargo.toml`、现有 `sha2` 使用 | workspace 已有 `sha2`，代码已有 SHA-256 helper/模式。 | manifest integrity 使用现有依赖，不新增外部依赖。 | 高 |
| E18 | `AGENTS.md`、`scripts/run-tests.sh`、`scripts/check-cli-doc-drift.sh` | nextest 是强制测试入口；skill 文档和 CLI 引用有 drift gate。 | 所有 Unit 按 nextest/venv/doc-drift/全量门禁执行。 | 高 |
| E19 | `crates/ralph-cli/src/main.rs:263-311`、`crates/ralph-cli/src/loop_runner/inner.rs:354-482` | CLI 在完整 `RalphConfig` 加载前创建 authoritative collector；`run_loop_impl_inner` 已持有 `config`、`hats_source_label`、`prebuilt_diagnostics`，并在构造 EventLoop 前完成 preset gate/context wiring。 | bundle 采用“早创建 pending metadata、run 入口补齐 config/preset/capability、终止入口 finalize”的两阶段 metadata 生命周期；不把完整 config load 搬到 `main.rs`。 | 高 |
| E20 | `crates/ralph-core/src/event_loop/prompt_injection.rs:58-100,315-389` | recovery envelope 记录入口在 `record_recovery_envelope`；runtime recovery actions 的实际应用函数也在 `prompt_injection.rs`，不是 `event_processing.rs`。 | Unit 2/3 必须把 action hook 写入 `prompt_injection.rs` 的真实函数；不使用不存在或错误的入口。 | 高 |
| E21 | `crates/ralph-core/src/drift/engine.rs:246-330`、`crates/ralph-cli/src/loop_runner/inner.rs:3737-3765` | `drain_hard_escalations` 是 task.resume/correction action 的实际发布入口；`check_recovery_for_iteration` 是 accepted evidence 后的 validation/outcome 更新入口；termination hint 在 `finalize_recovery_diagnosis` 消费。 | feedback lifecycle 的 action/validation/final 分别绑定这些真实调用点，不由 reporter 推测生命周期。 | 高 |
| E22 | `crates/ralph-core/src/event_loop/tests/mod.rs:1-131` | `event_loop/tests` 使用显式 `mod <name>;` 注册内部测试；当前没有 `mod diagnostics_equivalence;`。 | 新增差分测试必须同时修改该注册文件，Acceptance Red 先确认测试确实被 nextest 收集。 | 高 |
| E23 | `crates/ralph-core/src/diagnosis/reporter.rs:97-270,1961-2100` | `SessionData`/`Report` 是现有 reporter 的数据边界，`render_json` 是显式 JSON key 构造入口；现有 schema version 1 只要求非 additive 变更 bump。 | 新增字段类型、legacy 空值语义和 Markdown/JSON 一对一映射必须在计划中固定；不得让 Executor 自由设计 JSON shape。 | 高 |
| E24 | `crates/ralph-core/src/drift/engine.rs:246-330`、`crates/ralph-cli/src/loop_runner/inner.rs:3305-3380,3737-3765`、`crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs`、`tests/scenarios/`、wave/supervisor tests | 真实运行链包含 watchdog、task.resume、retry/dedup、accepted evidence、termination；wave/supervisor 有独立 scenario/测试入口。 | off/on 差分必须比较同一 fixture 的 accepted topics/payloads、state projection、recovery outcomes/targets、termination reason、CLI result/exit semantics，并覆盖 capabilities。 | 高 |

### 2.3 已确认受影响范围

- **生产模块**：`crates/ralph-core/src/diagnostics/mod.rs`；新增 diagnostics `input_bundle.rs`、`runtime_trace.rs`、`feedback.rs`；`event_loop/event_processing.rs`、`parse_and_emit.rs`、`prompt_injection.rs`；`ralph-cli/src/loop_runner/inner.rs`；`diagnosis/reporter.rs`。
- **测试模块**：已有 `crates/ralph-core/src/diagnostics/integration_tests.rs`、`crates/ralph-core/tests/diagnostics_e2e.rs`、`crates/ralph-cli/tests/trace_layer_integration_test.rs`、`crates/ralph-core/src/diagnosis/tests.rs`、`crates/ralph-cli/src/commands/diagnose.rs` 内测试、`event_loop/tests/workflow_guard.rs`、`recovery_envelope_u7_u8.rs`、`task_resume_runtime_routing.rs`、`replay_light_integration.rs`、`crates/ralph-core/tests/scenarios.rs`。
- **新增测试位置**：`crates/ralph-core/tests/diagnosis_input_bundle.rs`、`crates/ralph-core/tests/diagnosis_report_bundle.rs`、`crates/ralph-core/src/event_loop/tests/diagnostics_equivalence.rs`、`skills/tests/test_run_diagnosis_contract.py`；它们在进入相应 Unit 时新建，不是假定当前已存在。
- **配置/数据**：`TelemetryConfig` 只复用既有字段；session 下新增三个文件；旧 `.ralph` state、ledger、events、recovery 不迁移不删除。
- **CLI/API/UI**：`ralph diagnose` 的既有 format/output/session 接口扩展 JSON/Markdown 内容；不新增公开 CLI 参数、API 或 UI。
- **文档/skill**：`skills/ralph-run-diagnosis/SKILL.md`、其 `artifact-discovery.md`、`artifact-manifest.md`、`log-reconciliation.md`、`report-template.md`、`verification-pipeline.md`、`ssot-guardrails.md`；`README.md`、`docs/guide/runtime-diagnosis.md`、`docs/reference/troubleshooting.md`、Unit 6 明确列出的四个 agent-facing data 文档和三处代码注释。
- **构建目标**：`ralph-core`、`ralph-cli`、workspace tests/docs/clippy；skill Python contract tests。

## 3. 决策记录与置信度

| ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | 诊断输入包放在哪里？ | 改写 summary；新 session index；新增 workspace DB | 在现有 session 目录新增 `.ralph/diagnostics/<session>/diagnosis-input.json`，summary 保持 v1 | E1、E2、E4、E10 | 改 summary 会扩大旧 schema/fixtures 改动；DB 引入新持久化边界；session 已是现有 reporter 入口 | 0.97 |
| D2 | 结构化 trace 是否复用 `trace.jsonl`？ | 复用同文件；改 tracing layer 共用 writer；新增 sidecar | 新增 `runtime-trace.jsonl`，保留 raw `trace.jsonl` | E3、E7 | 同文件有两个 writer 会破坏 flush/顺序；改 global subscriber 扩大 CLI 风险；sidecar 不改变现有 raw trace | 0.96 |
| D3 | 是否新增配置开关？ | 新 flag；复用 full/minimal；默认强制开启 | 复用现有 DiagnosticsOptions：full/minimal 写新 sidecars，disabled/trace-only 不写 loop artifacts | E1、E2、E13 | 新 flag 会改变用户配置面；强制开启违反零副作用/默认 no-op 契约 | 0.96 |
| D4 | manifest 如何识别配置/preset/代码基线？ | 只记录路径；记录内容快照；记录相对引用+SHA-256+Git HEAD | 记录 workspace-relative refs、存在性/字节数/SHA-256、`get_head_sha` 结果、preset label、execution capabilities | E17、E9、E12 | 路径 alone 无法发现漂移；全文快照增加 secrets/体积风险；已有 hash 模式可复用 | 0.93 |
| D5 | feedback identity 与生命周期如何关联？ | 新增独立 UUID；按 topic；复用 envelope `diagnosis_id`/`retry_key` | `feedback_id=diagnosis_id`，`retry_key` 作为稳定聚合键；phase/status 为 discovered/evidence/action/validation/final | E5、E6、E10 | topic 不能区分重复 retry；另造 UUID 不能回指 recovery journal；现有 outcome/attempt 已覆盖重复/升级 | 0.94 |
| D6 | runtime 记录写失败如何处理？ | 使 run 失败；阻塞重试；warning + degraded manifest | 沿用 recovery/drift/summary 的 best-effort：单次写失败只 warning，bundle 标 `degraded`，run 继续 | E2、E4、E10、E13 | 失败 run 会引入诊断回归；无限重试违反 R17；已有 logger 契约已验证 | 0.97 |
| D7 | reporter 如何兼容旧 session？ | 只支持新 bundle；自动猜测补全；bundle-first + raw verification + legacy fallback | 有效 bundle 优先；缺失/坏 bundle 写 warning 后回退当前读取路径；不把缺失当故障 | E10、E13、E16 | 只支持新格式破坏历史 report；自动补全会制造无证据结论 | 0.98 |
| D8 | JSON schema 如何迁移？ | bump 到 v2；替换旧字段；保持 v1 的 additive 扩展 | 保持 `DIAGNOSE_JSON_SCHEMA_VERSION="1"`，追加 `diagnosis_input`、`runtime_trace`、`feedback_lifecycle`、`repair_suggestions`、`evidence_gaps` | E11、E16 | 旧字段无 breaking change；非 additive 才 bump，当前没有删除/改类型 | 0.94 |
| D9 | run-diagnosis 的使用形式是否改变？ | 新命令/实时控制台；自动 plan；现有 skill 加 bundle-first | 保持完整 run 后调用同一 skill；只改 Phase 0、对账顺序、报告字段和建议分层 | E12、需求确认、E14 | 用户明确保留原工作流；新控制面超出范围且有回归风险 | 0.99 |
| D10 | `human.guidance`/`task.resume` 如何迁移？ | 删除两者；把 task.resume 宣称已废弃；继续混用旧术语 | `human.guidance` 标 historical/compat；`task.resume` 标 runtime recovery transport；新报告/skill 用 recovery action/intervention/resume context | E14、E15、现有 data skills | 删除会破坏历史/compat；宣称 task.resume 不存在与 HEAD 代码冲突；混用维持当前文档矛盾 | 0.98 |
| D11 | 完整 bundle metadata 何时、从哪里写入？ | 在 `main.rs` 完整加载 config 后创建 collector；只写 session id；collector pending 后由 runner 补齐 | `main.rs` 继续按现有顺序创建 collector；Unit 1 支持 `pending` metadata；`run_loop_impl_inner` 在 `enforce_preset_lint_gate_with_preset_name` 通过且 EventLoop 构造前，调用 collector 的 metadata update，值固定来自现有 `config.config_path`、`hats_source_label`、`get_head_sha(ctx.workspace())`、`supervisor_path_enabled` 和已观察/已解析的 `wave_id` 信号；若未命中 supervisor/wave 信号则写 `single-chain`，不把未知能力误报为缺陷；`finalize_recovery_diagnosis` 写最终 artifact status/termination metadata。 | E19、E24、E4 | 改变 main 初始化顺序会影响诊断 eligibility 和子进程 TUI；只写 session id 无法满足 manifest 目标；从不存在的通用 config capabilities 字段读取会把实现决策留给 Executor；两阶段复用已存在的 authoritative collector。 | 0.96 |
| D12 | feedback 的 action/validation/final 从哪里取得？ | reporter 根据 raw logs 猜测；在每个拒收分支重复打点；复用真实生命周期入口 | discovered/evidence 在 `prompt_injection.rs::record_recovery_envelope`；runtime action 在 `prompt_injection.rs::apply_runtime_recovery_actions`，task.resume/correction action 另在 `drift/engine.rs::drain_hard_escalations`；validation 在 `drift/engine.rs::check_recovery_for_iteration` 的 outcome change；final 在 `loop_runner/inner.rs::finalize_recovery_diagnosis`/`write_termination_diagnostics`，使用真实 `TerminationReason`/`TerminationHint`。 | E20、E21 | 分支重复打点会漏路径并有回归风险；reporter 只能读取，不能创造事实；这些入口已覆盖正常 recovery、recovery action、accepted evidence 和终态。 | 0.94 |
| D13 | 新增 EventLoop 差分测试如何确保真的执行？ | 只新增文件；放在外部 integration test；注册内部 test module | 新增 `crates/ralph-core/src/event_loop/tests/diagnostics_equivalence.rs`，并在现有 `crates/ralph-core/src/event_loop/tests/mod.rs` 增加 `mod diagnostics_equivalence;`；测试入口固定为 `cargo nextest run -p ralph-core -- diagnostics_equivalence`。 | E22 | 未注册的 Rust module 不会执行；外部 E2E 无法精确比较内部 accepted/recovery projection；现有 tests/mod.rs 是事实注册入口。 | 0.99 |
| D14 | reporter 新增 JSON/Markdown 字段的固定 shape 是什么？ | 任意新增 keys；替换旧 keys；固定 additive typed fields | `Report`/`SessionData` 增加：`diagnosis_input` 状态对象（`present|legacy|degraded|missing`、path/schema、artifact statuses）；`runtime_trace` 摘要对象（status、record_count、malformed_lines、first/last sequence）；`feedback_lifecycle` 数组（feedback_id、retry_key、phase records、attempts、final_status、evidence refs）；`repair_suggestions` 数组（tier、finding_refs、evidence_refs、confidence、text）；`evidence_gaps` 数组（artifact/ref、reason、affects）。legacy 始终输出 `status=legacy`/空数组，不省略 keys；Markdown 按同名五段输出。 | E23、D7/D8 | 未定义 shape 会把关键 schema 决策留给 Executor；替换旧 keys 破坏消费者；固定空值语义可验证 legacy/degraded。 | 0.93 |
| D15 | 诊断文件写失败但 manifest 也无法写时如何观察？ | 要求 manifest 必须记录 degraded；让 run 失败；静默丢失 | collector 在内存保留 degraded 状态并继续 run；若 manifest 可更新则写 `degraded`，若 manifest 本身不可创建/替换，reporter 以 `diagnosis_input.status=missing` + warning/evidence gap 观察，不能声称 `degraded` 已落盘；Unit 1 分别测试 logger 可写/manifest 可写和 manifest 不可写两种路径。 | E2、E4、E10、E19 | 目标文件不可写时不存在可写的备用 manifest；让 run 失败违反 best-effort；静默会制造“无证据成功”。 | 0.98 |
| D16 | off/on 零回归如何形成可执行差分？ | 只比较 accepted topic；只跑现有全量场景；依赖人工观察 | 为每个固定 fixture 运行 diagnostics off/on 两次，序列化并比较：accepted event 的 `(iteration, source, target, topic, payload)`、state projection/ledger commit summary、recovery `(retry_key, outcome, target, attempt)`、task.resume routing/dedup、watchdog/termination reason、CLI result/exit semantics；允许差异仅为新 sidecar/report fields。matrix 覆盖 normal/failure/recovery-exhausted/replay/wave/supervisor。 | E24、E18 | 单一摘要无法证明 sidecar hook 没有改变 routing/retry/exit；现有各类测试是被比较的真实执行路径，不替代差分断言。 | 0.96 |

## 4. BDD 行为规格

### Feature: 诊断启用时生成可自描述的运行输入包

  Background:
    Given 一个合法 preset 和已解析 RalphConfig
    And 运行使用现有 full 或 minimal diagnostics 激活矩阵

  Scenario S1: 正常终止的 run 生成 manifest
    Given run 已创建 diagnostics session
    When run 经过至少一个 iteration 并正常或失败终止
    Then session 包含 `diagnosis-input.json`
    And manifest 包含 session/run/preset/config/code baseline/capability/artifact integrity
    And manifest 列出 runtime-trace/feedback artifact 的路径和初始状态

  Scenario S2: minimal diagnostics 与 full diagnostics 都生成输入 manifest
    Given 两次 run 的唯一差异是既有 diagnostics mode
    When 两次 run 都完成终止写入
    Then 两个 session 都有 `diagnosis-input.json`
    And full mode 仍保留已有 full logger，minimal mode 不凭空创建 full logger

### Feature: 运行时反馈可以按生命周期追踪

  Scenario S3: activation、事件、拒收和终态进入有序 runtime trace
    Given EventLoop 观察到 activation、accepted event、rejected event 和 recovery action
    When runner 记录 timeout 或 termination
    Then runtime trace 按 timestamp/iteration/sequence 保持可排序
    And 每条记录带 phase、kind、source ref、target/hat/topic（适用时）
    And 记录不改变 bus、ledger 或 termination 的结果

  Scenario S4: 同一 retry_key 的重复恢复与恢复成功保持同一 feedback identity
    Given 同一 `retry_key` 产生两次 recovery envelope
    When 后续 accepted event 或终态表明恢复成功/升级/未决
    Then feedback records 使用同一 `feedback_id`/`retry_key`
    And 报告显示真实 attempt、重复、action、validation 和 final status

### Feature: feedback lifecycle 记录真实 action 与 validation

  Scenario S5: action、validation、final 分别来自真实运行入口
    Given EventLoop 记录 recovery envelope，runner 产生 recovery action，并在下一 iteration 检查 accepted evidence
    When run 正常结束、recovery exhausted 或被 watchdog/termination 终止
    Then feedback 依次记录 discovered/evidence、action、validation 和 final（存在时）
    And 每一阶段带对应 source ref，不由 reporter 推断缺失阶段

### Feature: 诊断写入失败不影响 run

  Scenario S6: runtime trace logger 创建失败时 run 仍完成原业务路径
    Given `runtime-trace.jsonl` 目标不可写
    When EventLoop 继续处理同一输入
    Then run 不因诊断 logger 初始化失败而返回新错误
    And manifest/报告标记 trace degraded 或 missing
    And accepted topics、recovery decision、termination reason 与无诊断基线一致

  Scenario S7: diagnostics 关闭时不创建新文件
    Given `DiagnosticsOptions::default()`
    When collector 和 EventLoop 运行同一输入
    Then session_dir、diagnosis-input、runtime-trace、feedback 均不存在
    And 事件结果、状态投影和退出结果保持原行为

### Feature: reporter 与 run-diagnosis 优先消费 bundle

  Scenario S8: 有效 bundle 被优先读取，raw artifacts 只用于核验
    Given session 有有效 `diagnosis-input.json`、runtime trace、feedback 和旧 raw logs
    When 执行 `ralph diagnose --format json` 或 `run-diagnosis`
    Then 报告包含 bundle 状态、生命周期、证据 refs、evidence gaps 和分层 repair suggestions
    And 报告仍以 accepted event/raw artifact 核验终态，不以结论覆盖事实

  Scenario S9: 旧 session 缺少新 bundle 时兼容回退
    Given session 只有旧 recovery/drift/summary/orchestration 产物
    When 执行 reporter 或 skill
    Then 仍生成旧格式可识别的报告
    And report 明确 `diagnosis_input=legacy`/盲区
    And 不凭空声称新 trace 或 feedback 存在

  Scenario S10: bundle 或 JSONL 坏行只降低证据完备度
    Given manifest 缺字段、trace/feedback 含坏行或某文件写入中断
    When 诊断生成报告
    Then 报告列出 warning/evidence gap
    And 相关根因置信度受 mode/证据完整度限制
    And 不输出没有直接证据支持的确定性根因

### Feature: 报告只给人工修复建议

  Scenario S11: 高置信度问题输出分层修复建议但不自动执行
    Given report 已有 runtime/preset/schema/agent/environment/orchestration finding
    When report 完成
    Then 每条建议能回指 finding、evidence ref 和 confidence
    And 建议分为短期 operator workaround、中期 preset/schema/instructions、长期机制
    And 不修改代码、preset、task、ledger，不启动新 run

### Feature: 旧恢复术语与当前运行路径不混淆

  Scenario S12: 新文档与 agent-facing skill 区分历史 human.guidance 和当前 task.resume transport
    Given 当前代码仍存在 task.resume runtime routing，human.guidance 已不再是 orchestrator control topic
    When operator 阅读 README、runtime diagnosis guide、skill 或 recovery directives
    Then human.guidance 只出现在历史/replay/compat 说明
    And task.resume 被说明为 runtime recovery transport，而非人工 guidance UI
    And 文档不宣称 task.resume 已完全被删除

## 5. 验收与测试策略

| Scenario | Unit | 验收条件/副作用/不变量 | 测试入口与层级 | 风险补充测试 | E2E |
|---|---|---|---|---|---|
| S1/S2 | Unit 1 | 断言 manifest 文件存在、固定 schema、session/loop/preset/config/code refs、artifact status；旧文件仍存在。 | 新增 `crates/ralph-core/tests/diagnosis_input_bundle.rs`；`cargo nextest run -p ralph-core -- diagnosis_input_bundle`。 | JSON round-trip + SHA-256 stability；不测试 prompt 全文。 | 否 |
| S3 | Unit 2 | 断言 runtime trace 有 activation/event/rejection/timeout/termination kind，sequence 单调，refs 可定位 raw artifacts；accepted payload/result 不变。 | `crates/ralph-core/src/event_loop/tests/diagnostics_equivalence.rs` + `event_loop/tests/mod.rs` 注册；`cargo nextest run -p ralph-core -- diagnostics_equivalence`。 | State-machine/characterization：同一 fixture off/on 比较 accepted event tuple、state、ledger、termination。 | 否 |
| S4/S5 | Unit 3 | 断言同 retry_key 的 feedback_id 不变；action/validation/final 阶段各回指真实入口，attempt/outcome/status 保留。 | 扩展 `crates/ralph-core/src/diagnosis/tests.rs`、新增 feedback integration；`cargo nextest run -p ralph-core -- feedback_lifecycle`。 | Idempotency + lifecycle state-machine：重复 envelope 不产生新错误终态，accepted evidence 只在下一 iteration 更新 recovery。 | 否 |
| S6/S7 | Unit 1/7 | 断言 logger/manifest 写失败不向 runner 返回新错误；manifest 可写时为 degraded，manifest 不可写时 reporter 为 missing+warning；disabled 不创建新文件。 | Unit 1 `diagnostics_write_failure`；Unit 7 off/on matrix；`cargo nextest run -p ralph-core -- diagnostics_write_failure`。 | Fault injection：目标路径为目录、坏 JSONL、缺文件、部分写入。 | 否 |
| S8 | Unit 4 | 断言 `load_session` 读 bundle/trace/feedback，JSON 固定 additive keys，Markdown 有对应五段；accepted/raw 核验优先。 | 新增 `crates/ralph-core/tests/diagnosis_report_bundle.rs`，扩展 CLI tests；`cargo nextest run -p ralph-core -- diagnosis_report_bundle`、`cargo nextest run -p ralph-cli --bin ralph -- diagnose`。 | Golden 只锁稳定字段/结构；变更说明新增字段且旧字段不变。 | 否 |
| S9/S10 | Unit 4/5 | 断言 legacy session 仍出报告，坏行/缺失只 warnings，mode/confidence cap 生效，缺证据不生成确定性根因。 | reporter integration + skill contract；同 S8 命令及 Python contract。 | Differential：完整 bundle vs 删除 bundle 的旧回退报告，旧 findings 不丢。 | 否 |
| S11 | Unit 4/5 | 断言每条 suggestion 有 tier/finding/evidence/confidence 关联，报告没有写操作性自动执行动作。 | Rust JSON assertions + `skills/tests/test_run_diagnosis_contract.py`；`.venv/bin/python -m pytest skills/tests/test_run_diagnosis_contract.py`。 | static contract 只锁稳定入口/字段，不锁整段 prompt 文案。 | 否 |
| S12 | Unit 6 | 断言 README/guide/skill/recovery docs 当前语义一致；保留历史/compat 文本；CLI help/doc drift 通过。 | `rg` characterization + `bash scripts/check-cli-doc-drift.sh --strict`；existing skill tests。 | 旧 fixture/history 文档保留检查，防止误删兼容证据。 | 否 |

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | Unit | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|---|
| R1 | 自描述输入包 | S1/S2 | Unit 1 | `enabled_run_writes_input_bundle` | bundle serialization/integrity | `diagnosis_input_bundle.rs` | 否 | E1/E2/E4/E17/E19 |
| R2 | 结构化时序 trace | S3 | Unit 2 | `runtime_trace_records_lifecycle` | trace entry bounds/order | EventLoop diagnostics equivalence | 否 | E3/E7/E8/E9/E20/E24 |
| R3 | feedback lifecycle | S4/S5 | Unit 3 | `feedback_reuses_retry_identity` | phase/status transition | recovery envelope/drift engine integration | 否 | E5/E6/E20/E21 |
| R4 | evidence refs/gaps | S8/S9/S10 | Unit 4/5 | `report_exposes_evidence_gaps` | ref/path validation | reporter bundle integration | 否 | E5/E10/E12/E23 |
| R5 | raw/derived/conclusion 分离 | S8/S9 | Unit 4 | `report_does_not_overwrite_event_fact` | provenance classification | reporter integration | 否 | E7/E10/E12/E23 |
| R6 | 失败降级不阻塞 | S6/S7 | Unit 1/7 | `diagnostic_write_failure_is_fail_soft` | logger error path | EventLoop failure injection | 否 | E2/E4/E10/E13/E15 |
| R7 | bundle-first diagnosis | S8/S9 | Unit 4/5 | `skill_requires_bundle_first_with_legacy_fallback` | manifest status parser | CLI/reporter contract | 否 | E10/E12/E19/E23 |
| R8 | 报告总览 | S8 | Unit 4 | `render_json_has_run_summary_and_status` | report field mapping | `diagnosis_report_bundle.rs` | 否 | E11/E16/E23 |
| R9 | 逐问题根因/证据/影响 | S8/S9/S10 | Unit 4/5 | `finding_has_confidence_refs_gaps` | finding mapper | reporter integration | 否 | E5/E10/E12/E23 |
| R10 | 分层修复建议 | S11 | Unit 4/5 | `suggestions_are_traceable_and_non_executing` | suggestion mapper | JSON/Markdown report | 否 | E11/E12/E23 |
| R11 | 深度流程/历史/低置信度 | S9/S10 | Unit 4/5 | `legacy_mode_caps_confidence` | mode parser | skill contract | 否 | E12/E13 |
| R12 | Markdown + structured result | S8/S11 | Unit 4/5 | `render_markdown_and_json_are_both_available` | additive JSON schema | CLI diagnose + skill sidecar | 否 | E11/E16/E23 |
| R13 | 统一术语 | S12 | Unit 6 | `diagnosis_docs_use_feedback_action_intervention_terms` | 不适用 | skill/docs contract | 否 | E12/E14/E15 |
| R14/R15 | 旧 topic 兼容语义 | S12 | Unit 6/7 | `legacy_topics_are_not_current_guidance` | existing replay tests retained | docs drift + runtime routing regression | 否 | E14/E15/E24 |
| R16/R17 | 开关/旁路零回归 | S6/S7 | Unit 1/2/3/7 | `diagnostics_off_on_is_equivalent` | collector no-op | full off/on differential | 否 | E1/E2/E7/E13/E24 |
| R18 | normal/failure/replay/wave/supervisor 覆盖 | S2/S3/S6/S7 | Unit 2/3/7 | mode matrix tests | existing mode tests | scenarios/replay/wave/supervisor tests | `cargo run -p ralph-e2e -- --mock` | E12/E18/E24 |
| R19 | schema/坏行/部分/重复/恢复 | S4/S5/S6/S8/S9/S10 | Unit 1/3/4/5/7 | all named tests | serde/idempotency | reporter + diagnostics integration | 否 | E4/E5/E10/E15/E16/E21/E23 |

## 7. 严格串行开发单元

执行顺序固定：

`Unit 1 → Unit 2 → Unit 3 → Unit 4 → Unit 5 → Unit 6 → Unit 7`

每个 Unit 必须在进入下一个 Unit 前完成 Acceptance Red → Unit Red → Green → Refactor → Integration → Regression → Close。

### Unit 1：建立诊断输入 manifest 的数据契约

#### 1. Unit 目标

启用 full/minimal diagnostics 时，collector 能原子写出固定 shape 的 `diagnosis-input.json`；此 Unit 只完成输入 manifest 和 collector metadata/status，不接入 EventLoop 生命周期，也不创建 runtime trace/feedback logger。

#### 2. 对应需求与 Scenario

R1、R6、R7、R19；S1、S2、S6、S7；D1、D3、D4、D6、D11、D15；E1–E4、E17、E19。

#### 3. 外部可观察结果

给定 metadata 调用 collector 后，session 中出现可解析 manifest；manifest 固定列出 raw/derived artifact、integrity/status、schema，且 metadata 可以先 `pending`、在 run 入口补齐、终止时 finalize；manifest 写失败只产生 warning，不向调用方抛出新的 runtime error。

#### 4. 当前行为基线

当前只有 `diagnosis-summary.json`、旧 JSONL logger 和 raw `trace.jsonl`；没有 `diagnosis-input.json`、`runtime-trace.jsonl`、`feedback.jsonl`。E1–E4 已确认 disabled/minimal/full 的现有行为，新增测试必须先锁定 disabled 不写文件的 characterization。

#### 5. 输入与输出

- 输入：`DiagnosticsOptions`、session path、可选 `RunMetadata`（session/loop/preset/config/baseline/capabilities）。
- 输出：原子 manifest；manifest 状态 `pending|present|missing|degraded|legacy|not_applicable`。
- 错误：序列化/路径/权限失败 → warning + status，不返回 loop-facing error。
- 状态变化：仅 collector 的 metadata/status；完整 metadata 在 Unit 2 的 runner wiring 中补齐。
- 副作用：仅诊断 session 文件；disabled/trace-only 不产生 loop sidecars。
- 不变量：旧 logger 文件和 `DiagnosisSummary` schema 不变。

#### 6. 修改位置

- `crates/ralph-core/src/diagnostics/input_bundle.rs`（新增）：定义 `RunMetadata`、固定的 `DiagnosisInputBundle`、artifact/integrity/status 类型和原子 writer；不读取 EventLoop、不做结论。
- `crates/ralph-core/src/diagnostics/mod.rs`：注册/导出 input bundle，在 collector 中持有 metadata/status；不修改既有 logger 的创建条件和方法语义。
- `crates/ralph-core/tests/diagnosis_input_bundle.rs`（新增）：round-trip、disabled、atomic/degraded 测试。

#### 7. 可依赖能力

现有 `DiagnosticsOptions`、session 创建、`NamedTempFile::persist`、`sha2`、serde、现有 warning 习惯；Unit 2 将使用 collector 的 metadata update/finalize API。

#### 8. 禁止依赖的未来能力

不得读取 EventLoop 状态；不得在本 Unit 接入 activation/event/recovery/timeout；不得创建 runtime-trace/feedback logger；不得改 reporter、skill、README；不得新增配置项或依赖。

#### 9. 验收测试

- `enabled_run_writes_input_bundle`：用 full/minimal collector + TempDir + 固定 metadata；断言 manifest schema/session/preset/config/hash/status/artifact refs 和固定 artifact 列表。
- `disabled_collector_writes_no_new_artifacts`：用 `DiagnosticsOptions::default()`；断言无 session/new files，且既有 no-op。
- `bundle_write_failure_is_degraded_not_error`：分两例注入：manifest 可创建但更新失败时断言落盘 `degraded`；manifest 本身不可创建/替换时断言不 panic/不返回新 runtime error，后续 reporter 只能观察 `missing` + warning/evidence gap。
- 命令：`cargo nextest run -p ralph-core -- diagnosis_input_bundle`。

#### 10. Acceptance Red

先新增 `enabled_run_writes_input_bundle`，在生产类型/导出/collector manifest writer 尚不存在时运行上述 nextest；预期是目标 API/类型缺失导致的编译 Red，或 manifest 文件不存在的断言 Red。若出现 cargo toolchain、TempDir、无关 crate 编译失败，不算有效 Red，必须先排除。

#### 11. 单元测试拆分

1. `DiagnosisInputBundle` serde round-trip：固定 metadata，期望字段完全可读。
2. artifact status：present/missing/degraded/not_applicable 四态映射。
3. SHA-256：同一 bytes 得同一 hash，不读全文快照。
4. fixed artifact status：四态和 `pending/legacy` 的序列化语义固定。
5. metadata update：早期 pending → run 入口 complete → termination final 的字段保留。
6. atomic replacement：旧 manifest 不因半写被暴露；使用 Fake path，不 mock serde/hash/atomic write 的真实行为。

#### 12. Red → Green → Refactor 顺序

`enabled_run_writes_input_bundle` Red → 新增固定 bundle schema/writer 最小实现 → Green → `metadata_update_preserves_complete_run_identity` Red → 增加 pending/update/finalize API → Green → `disabled_collector_writes_no_new_artifacts` characterization Green → `bundle_write_failure_is_degraded_not_error` Red → best-effort/error-state 实现 → Green → 抽取 path/hash/atomic helper → Unit 全测 Green。

#### 13. 最小实现范围

必须实现 input bundle schema、collector metadata/status API、原子 manifest、disabled/trace-only no-op、失败 warning/degraded-or-missing 观察语义；不实现 runtime trace/feedback lifecycle、不实现 reporter 派生、不改旧 schema/配置。

#### 14. 集成验证

真实 `DiagnosticsCollector::with_options` + TempDir + serde/hash/atomic filesystem；路径错误可 Fake，序列化和实际写盘必须真实。运行 `cargo nextest run -p ralph-core -- diagnosis_input_bundle`，失败不得进入 Unit 2。

#### 15. 风险驱动测试

Characterization（disabled no-op）和 Fault Injection（目标目录/坏路径）是必需的，因为 R6/R16/R19 要证明 manifest 失败不改变正常运行；不做 concurrency test，Unit 1 尚未共享 EventLoop 生命周期。

#### 16. 回归范围

`crates/ralph-core/src/diagnostics/integration_tests.rs`、`diagnostics_e2e.rs`、`crates/ralph-cli/tests/trace_layer_integration_test.rs`、`crates/ralph-cli/src/loop_runner/tests/legacy/diagnosis.rs`；原因是 collector 初始化、raw trace 和 termination summary 共享 session 目录。执行 targeted nextest、`cargo fmt --all -- --check`、`cargo clippy --all-targets --all-features -- -D warnings`。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/diagnostics/input_bundle.rs` | 新增生产文件 | manifest schema/writer | E1/E4/E17 |
| `crates/ralph-core/src/diagnostics/mod.rs` | 修改生产文件 | 注册/持有 optional logger | E1/E2 |
| `crates/ralph-core/tests/diagnosis_input_bundle.rs` | 新增测试 | ATDD/TDD | E1/E4/E17 |

#### 18. 完成标准

S1/S2/S6/S7 中属于 manifest 的测试通过；Unit 1 targeted nextest、相关 diagnostics tests、fmt/clippy 通过；旧 trace/summary tests 通过；无 skipped/弱断言；只新增上述文件；Evidence/Decision 更新；可独立提交。

#### 19. 停止条件

发现 collector 在 minimal/full 条件不一致、现有 `trace.jsonl` writer 被迫共享、原子写入需要新依赖、写失败会传播到 loop、文件超过边界时停止；记录新证据并重做 D1/D3/D4/D6/D11/D15，不进入 Unit 2。

#### 20. 风险与注意事项

真实风险是 logger 初始化时文件冲突和 manifest 原子替换跨平台差异；以目标为目录/重复写测试检测，复用 `NamedTempFile::persist`，剩余风险是外部 filesystem quota，报告必须显示 degraded。

### Unit 2：接入结构化 runtime trace

#### 1. Unit 目标

在不改变 EventLoop/runner 决策的前提下，启用诊断的 run 生成可排序的 `runtime-trace.jsonl`；本 Unit 不写 `feedback.jsonl`，也不修改 recovery policy。

#### 2. 对应需求与 Scenario

R2、R6、R16、R18；S3、S6、S7；D2、D3、D6、D11、D13、D16；E3、E7–E9、E19、E20、E22、E24。

#### 3. 外部可观察结果

一次启用诊断的真实 run 可以按 `iteration + sequence` 看到 activation、batch/event accepted/rejected、commit、watchdog timeout、termination；关闭诊断时 accepted payload/result、state projection、ledger summary、termination reason 和 exit semantics 不变。

#### 4. 当前行为基线

`process_parse_result` 已产生 accepted/rejected/commit 事实，`event_processing.rs` 已调用 activation tracker，runner 已处理 watchdog/termination，但没有统一结构化 runtime sidecar。`trace.jsonl` 仍由独立 `DiagnosticTraceLayer` 写入，不能复用。

#### 5. 输入与输出

- 输入：已有事实对象，不复制原始 payload 全文。
- 输出：`RuntimeTraceEntry` 固定字段 `schema_version/ts/iteration/sequence/phase/kind/hat/topic/ref/status/fields`；`kind` 固定覆盖 `activation|batch|accepted|rejected|commit|watchdog_timeout|termination`。
- 错误：logger error 只 warning；不返回 loop-facing error。
- 状态变化：仅 collector sidecar 状态。
- 副作用：仅追加 `runtime-trace.jsonl`，并更新 manifest artifact status。
- 不变量：accepted events 顺序、bus publish 顺序、ledger commit 顺序、timeout continuation、termination reason 不变。

#### 6. 修改位置

- `crates/ralph-core/src/diagnostics/runtime_trace.rs`（新增）：定义上述固定 entry/logger；不复用 `trace_layer.rs` writer。
- `crates/ralph-core/src/diagnostics/mod.rs`：在 full/minimal 有效、disabled/trace-only no-op 的条件下持有 optional runtime logger；在 manifest 中登记 artifact。
- `crates/ralph-core/src/event_loop/event_processing.rs`：只在已确认 `hat_lifecycle_tracker.activate` 调用点旁路记录 activation。
- `crates/ralph-core/src/event_loop/parse_and_emit.rs`：只在 `process_parse_result` 中已有 batch/accepted/rejected/commit 事实确定后记录；不在 validation 分支增加控制分支。
- `crates/ralph-cli/src/loop_runner/inner.rs`：在既有 watchdog 分支和 `finalize_recovery_diagnosis`/`write_termination_diagnostics` 路径记录 timeout/termination，并在 `run_loop_impl_inner` 已解析 config/context 后调用 D11 的 metadata update；不改变 return reason。
- `crates/ralph-core/src/event_loop/tests/diagnostics_equivalence.rs`（新增）和 `crates/ralph-core/src/event_loop/tests/mod.rs`：加入 `mod diagnostics_equivalence;`，确保内部差分测试被收集。

#### 7. 可依赖能力

Unit 1 manifest writer/status、现有 `ProcessedEvents`、activation tracker、`TerminationReason`、runner hooks、D11 的 metadata update API。

#### 8. 禁止依赖的未来能力

不得写 feedback lifecycle；不得读取 reporter；不得改变 `task.resume`/`loop.resume` 路由、retry/dedup、wave/supervisor topology；不得生成 repair suggestion；不得改 `DiagnosisSummary`；不得把 trace 写入 bus/events/ledger。

#### 9. 验收测试

- `runtime_trace_records_lifecycle`：真实 EventLoop fixture，断言 activation、accepted/rejected、commit、watchdog、termination 记录和固定 kind/sequence。
- `runtime_trace_metadata_is_completed_by_runner`：断言 early collector 的 pending metadata 在 `run_loop_impl_inner` 入口变为 config/preset/capability 完整值，终止后 artifact status 可读。
- `diagnostics_off_on_is_equivalent`：同一 fixture off/on 比较 accepted `(iteration, source, target, topic, payload)`、state projection、ledger summary、timeout continuation、termination reason、CLI result/exit semantics。
- `watchdog_timeout_is_observation_only`：已有 `ExecutionOutcome.watchdog_timeout` fixture，断言新增 timeout 记录但 loop continuation/termination semantics 不变。
- 命令：`cargo nextest run -p ralph-core -- diagnostics_equivalence`、`cargo nextest run -p ralph-cli --bin ralph -- diagnosis`。

#### 10. Acceptance Red

先新增并在 `event_loop/tests/mod.rs` 注册 `runtime_trace_records_lifecycle`；当前 runtime logger 不存在，预期编译缺失或 `runtime-trace.jsonl` 缺少指定 kind。若测试没有被收集、出现 fixture path/toolchain/global subscriber 无关失败，先修测试入口/隔离，不把它当有效 Red。

#### 11. 单元测试拆分

1. entry serde 固定字段和 schema round-trip。
2. sequence 单调、同 iteration 可重复且 bounded fields 不携带无限 payload。
3. activation 使用 tracker 的 hat/trigger，不重新推断。
4. accepted/rejected/commit 的 status/ref 映射来自 `process_parse_result` 已有结果。
5. timeout 与 termination kind 区分，disabled logger 全部 no-op。
6. metadata pending/update/finalize 不丢失 loop/preset/config/capability。

#### 12. Red → Green → Refactor 顺序

`runtime_trace_records_lifecycle` Red → 新增 runtime entry/logger 和 collector hook → Green → `runtime_trace_metadata_is_completed_by_runner` Red → 在真实 runner 入口补 metadata update/finalize → Green → `diagnostics_off_on_is_equivalent` Red → 补齐 disabled/no-op 与 observer-only wiring → Green → `watchdog_timeout_is_observation_only` Red → 加 timeout observer → Green → 抽取统一 trace ref/bounded helper → Unit 全测 Green。

#### 13. 最小实现范围

必须接入 activation、batch/event、commit、watchdog、termination 五类 runtime trace 事实和 D11 两阶段 metadata；必须保留现有顺序/返回/target/dedup；不得日志失败重试 loop；不实现 feedback、reporter、skill。

#### 14. 集成验证

真实 EventLoop、real `process_parse_result`、real activation tracker、real runner timeout/termination；外部 backend 用现有 test fake 隔离。运行 `cargo nextest run -p ralph-core -- diagnostics_equivalence`、`cargo nextest run -p ralph-cli --bin ralph -- diagnosis`、现有 `task_resume_runtime_routing`，任一失败不得进入 Unit 3。

#### 15. 风险驱动测试

State-machine/characterization（off/on exact tuple）、Fault Injection（logger error）、watchdog observation test 是必需的，因为 observer hook 可能改变顺序或 timeout branch；wave/supervisor 拓扑不在本 Unit 修改。

#### 16. 回归范围

`event_loop/tests/task_resume_runtime_routing.rs`、`event_loop/tests/recovery_envelope_u7_u8.rs`、`event_loop/tests/hat_lifecycle_jsonl_e2e.rs`、`event_loop/tests/progress_steward*.rs`、`replay_light_integration.rs`、`crates/ralph-cli/src/loop_runner/tests/legacy/{diagnosis,recovery}.rs`；原因是 trace 入口覆盖 activation、event processing、timeout、termination，并必须证明 task.resume 不变。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/diagnostics/runtime_trace.rs` | 新增生产文件 | runtime trace schema/logger | E3/E20 |
| `crates/ralph-core/src/diagnostics/mod.rs` | 修改生产文件 | optional logger/manifest status | E1/E2 |
| `crates/ralph-core/src/event_loop/event_processing.rs` | 修改生产文件 | activation trace | E8/E20 |
| `crates/ralph-core/src/event_loop/parse_and_emit.rs` | 修改生产文件 | batch/event/commit trace | E7 |
| `crates/ralph-cli/src/loop_runner/inner.rs` | 修改生产文件 | metadata/timeout/termination trace | E9/E19 |
| `crates/ralph-core/src/event_loop/tests/diagnostics_equivalence.rs` | 新增测试 | exact off/on differential | E22/E24 |
| `crates/ralph-core/src/event_loop/tests/mod.rs` | 修改测试注册 | 确保新 module 被收集 | E22 |

#### 18. 完成标准

S3、S6、S7 中属于 runtime trace 的测试通过；off/on exact tuple、watchdog、metadata、task.resume 回归通过；fmt/clippy/build targeted 通过；无业务代码路径新增会改变结果的 diagnostics 分支；可独立提交。

#### 19. 停止条件

任何新日志调用改变 accepted tuple、bus、ledger、target、retry count、timeout continuation 或 exit semantics；测试 module 未被收集；出现需要修改 recovery policy 的要求；trace 写入必须等待/重试。停止并更新 D2/D6/D11/D13/D16，不进入 Unit 3。

#### 20. 风险与注意事项

最大风险是 observer hook 放错在“事实确定前”或“业务副作用后”导致时序误读；只允许在已有事实变量确定点记录，exact differential 覆盖 off/on。进程被 kill 时尾部 JSONL 可能截断，后续 reporter 必须按坏行降级。

### Unit 3：接入 feedback lifecycle journal

#### 1. Unit 目标

在不改变 recovery responder、task.resume/correction routing 或终态判定的前提下，生成与真实 recovery 生命周期对应的 `feedback.jsonl`；本 Unit 不修改 reporter/skill。

#### 2. 对应需求与 Scenario

R3、R6、R16、R18、R19；S4、S5、S6、S7；D5、D6、D12、D16；E5/E6/E14/E20/E21/E24。

#### 3. 外部可观察结果

同一 `retry_key` 的 recovery envelope、action、accepted-evidence validation 和最终 outcome 在 `feedback.jsonl` 中共享同一 `feedback_id=diagnosis_id`（无 diagnosis_id 时使用同一 retry_key 聚合），每阶段带真实 source ref；重复恢复不会创造第二个 identity。

#### 4. 当前行为基线

`record_recovery_envelope` 已写 recovery/orchestration 并更新 responder；`drift/engine.rs::drain_hard_escalations` 发布 task.resume/correction；`check_recovery_for_iteration` 更新 outcome 并写 final recovery journal；`finalize_recovery_diagnosis` 消费 termination hint。当前没有 feedback sidecar。

#### 5. 输入与输出

- 输入：`RecoveryDiagnosisEnvelope`、`EscalationDecision`、`RecoveryAction`、accepted evidence outcome change、`TerminationHint`/`TerminationReason`。
- 输出：`FeedbackEntry` 固定 `schema_version/ts/iteration/sequence/feedback_id/retry_key/phase/action_kind/outcome/attempt/evidence_refs/source_ref/status`；phase 固定 `discovered|evidence|action|validation|final`。
- 错误：feedback logger error 只 warning，不改变 responder 或 runner 返回值。
- 状态变化：仅 collector sidecar 状态。
- 副作用：仅追加 `feedback.jsonl` 并更新 manifest status。
- 不变量：recovery target/dedup、attempt、accepted evidence、task.resume/correction payload、termination reason 不变。

#### 6. 修改位置

- `crates/ralph-core/src/diagnostics/feedback.rs`（新增）：定义固定 phase/entry/logger；不推断缺失生命周期。
- `crates/ralph-core/src/diagnostics/mod.rs`：持有 optional feedback logger，沿用 Unit 1 best-effort status。
- `crates/ralph-core/src/event_loop/prompt_injection.rs::record_recovery_envelope`：记录 discovered/evidence 和真实 envelope refs；不改 dedup/responder 更新顺序。
- `crates/ralph-core/src/event_loop/prompt_injection.rs::apply_runtime_recovery_actions`：记录 `InjectDirective`/`ForcePlanBlocked`/`DedupeEnvelope` action；不改 action match 的业务分支。
- `crates/ralph-core/src/drift/engine.rs::drain_hard_escalations`：记录 task.resume/correction action 发布；不改 publish payload/target。
- `crates/ralph-core/src/drift/engine.rs::check_recovery_for_iteration`：记录 outcome change 为 validation；不重新计算 outcome。
- `crates/ralph-cli/src/loop_runner/inner.rs::finalize_recovery_diagnosis` / `write_termination_diagnostics`：记录 final；使用既有 termination/hint，不新增终态。
- `crates/ralph-core/tests/feedback_lifecycle.rs`（新增）：生命周期、identity、idempotency/fault tests。

#### 7. 可依赖能力

Unit 1 manifest/status、Unit 2 runtime trace sequence、现有 envelope/responder/drift engine/runner lifecycle；`diagnosis_id`/`retry_key` 原有稳定 identity。

#### 8. 禁止依赖的未来能力

不得让 feedback logger 触发 recovery action；不得改变 task.resume/CorrectionContext、retry budget、dedup、`check_recovery` 规则；不得读取 reporter 或生成 suggestions；不得修改历史 recovery JSONL schema。

#### 9. 验收测试

- `feedback_reuses_retry_identity`：同 `retry_key` 两次 envelope + same diagnosis id，断言所有 feedback rows identity/attempt 映射稳定。
- `feedback_records_real_lifecycle_sources`：真实 runner fixture 断言 discovered/evidence/action/validation/final 分别带 `record_recovery_envelope`、action publisher、`check_recovery_for_iteration`、termination source refs。
- `feedback_does_not_change_recovery_routing`：off/on 比较 target/dedup/retry/accepted evidence/outcome/termination。
- `feedback_write_failure_is_fail_soft`：feedback path 不可写时 run 继续，manifest 可写则 degraded，不可写则后续 reporter 只能 missing+warning。
- 命令：`cargo nextest run -p ralph-core -- feedback_lifecycle`、`cargo nextest run -p ralph-cli --bin ralph -- diagnosis`。

#### 10. Acceptance Red

先新增 `feedback_reuses_retry_identity`，当前没有 feedback logger，预期编译缺少 `FeedbackEntry`/文件或断言无 rows；若失败来自未收集测试、fixture 或不相关 recovery regression，不算有效 Red，先修入口/隔离。

#### 11. 单元测试拆分

1. phase enum/entry serde 和 fixed field bounds。
2. diagnosis_id 优先、retry_key fallback 的 identity grouping。
3. repeated envelope 不改变 feedback identity，attempt 保留真实值。
4. action source/ref mapping 覆盖 runtime action 与 task.resume/correction。
5. validation 只接受 `check_recovery_for_iteration` 的 outcome change。
6. final status 映射 `TerminationHint`/`TerminationReason`，缺失时保持 unresolved 而不是猜测。
7. logger failure/no-op 不改变 responder。

#### 12. Red → Green → Refactor 顺序

`feedback_reuses_retry_identity` Red → 新增 feedback entry/logger → Green → `feedback_records_real_lifecycle_sources` Red → 在四个真实入口追加旁路记录 → Green → `feedback_does_not_change_recovery_routing` Red → 完成 fail-soft/no-op wiring → Green → `feedback_write_failure_is_fail_soft` Red → manifest status/error propagation → Green → 抽取 identity/source-ref helper → Unit 全测 Green。

#### 13. 最小实现范围

必须实现固定五 phase、identity、真实入口 mapping、fail-soft logger；必须保持 recovery responder/action/validation/final 原有结果；不实现 reporter、skill、自动修复。

#### 14. 集成验证

真实 recovery envelope、drift engine action/validation、runner termination，外部 agent/backend 用已有 fake；运行 feedback lifecycle、task.resume routing、replay/recovery tests，任一失败不得进入 Unit 4。

#### 15. 风险驱动测试

Idempotency/state-machine 是核心（同 retry key 多次 envelope、下一 iteration accepted evidence）；Fault Injection 必需（feedback file/manifest path）；不做新 concurrency，因为 writer 仍由单 collector 同步持有。

#### 16. 回归范围

`crates/ralph-core/src/diagnosis/tests.rs`、`diagnosis/responder.rs` tests、`event_loop/tests/recovery_envelope_u7_u8.rs`、`task_resume_runtime_routing.rs`、`replay_light_integration.rs`、`crates/ralph-cli/src/loop_runner/tests/legacy/recovery.rs`、drift engine tests；原因是 feedback hook 直接邻接 identity/outcome/action。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/diagnostics/feedback.rs` | 新增生产文件 | feedback schema/logger | E5/E20/E21 |
| `crates/ralph-core/src/diagnostics/mod.rs` | 修改生产文件 | optional feedback logger | E1/E2 |
| `crates/ralph-core/src/event_loop/prompt_injection.rs` | 修改生产文件 | discovered/evidence/runtime actions | E20 |
| `crates/ralph-core/src/drift/engine.rs` | 修改生产文件 | action/validation source hooks | E21 |
| `crates/ralph-cli/src/loop_runner/inner.rs` | 修改生产文件 | final feedback + termination refs | E9/E21 |
| `crates/ralph-core/tests/feedback_lifecycle.rs` | 新增测试 | lifecycle/idempotency/fault | E5/E21 |

#### 18. 完成标准

S4/S5/S6/S7 中属于 feedback 的测试通过；identity/action/validation/final source assertions、task.resume/recovery/replay regressions、fmt/clippy/build 通过；没有改变 recovery result；可独立提交。

#### 19. 停止条件

任何 hook 改变 retry key、attempt、target/dedup、task.resume/correction payload、accepted evidence 或 termination；无法从真实入口取得某 phase；logger 写失败传播到 loop；停止并更新 D5/D6/D12/D16，不进入 Unit 4。

#### 20. 风险与注意事项

最大风险是把“action 已请求”误记为“action 已生效”，或把 outcome change 误记为 final；字段必须使用 D12 的 source ref 和 phase，reporter 后续才可区分事实。进程 kill 造成尾行损坏由后续 reporter warning 处理。

### Unit 4：让 `ralph diagnose` bundle-first 并输出可追溯建议

#### 1. Unit 目标

现有 reporter/CLI 优先读取 `diagnosis-input.json`、`runtime-trace.jsonl`、`feedback.jsonl`，并按 D14 固定 shape 在 Markdown/JSON 中输出生命周期、evidence gaps、confidence 和分层 repair suggestions；旧 session 完整回退。

#### 2. 对应需求与 Scenario

R4–R12、R19；S8–S11；D7、D8、D14；E10/E11/E13/E16/E23。

#### 3. 外部可观察结果

执行既有 `ralph diagnose --session ... --format markdown|json` 时，报告先显示 bundle 状态与总览，再显示 raw 核验结果、feedback lifecycle、root causes、evidence gaps、short/mid/long suggestions；旧字段和旧 session 输出继续可读。

#### 4. 当前行为基线

`load_session` 不读取新文件；`Report` 没有 bundle/trace/feedback/suggestions structured fields；已有 `Suggested next actions` 只从 `RankedFinding` 生成。E10/E11 已确认读取和 CLI output 行为。

#### 5. 输入与输出

- 输入：有效/缺失/坏的 bundle、runtime trace、feedback、旧 raw artifacts。
- 输出：`SessionData`/`Report` 的固定字段：`diagnosis_input` 状态对象、`runtime_trace` 摘要对象、`feedback_lifecycle` 数组、`repair_suggestions` 数组、`evidence_gaps` 数组；Markdown 五段与 JSON keys 一对一。
- 错误：缺失/坏行加入 warnings；不因新文件缺失而返回 NoSession。
- 状态变化：reporter 只读，不写 session；legacy 永远输出 `diagnosis_input.status=legacy` 和空的新增数组。
- 不变量：accepted event chronology 优先；旧 recovery fallback 保持；schema version `1` 和旧 JSON keys 保持。

#### 6. 修改位置

- `crates/ralph-core/src/diagnosis/reporter.rs`：增加 bundle/trace/feedback reader、D14 fixed typed fields、provenance/lifecycle mapping、Markdown/JSON renderer；不修改 ledger report 的独立入口语义。
- `crates/ralph-core/src/diagnosis/tests.rs`：新增 reader/legacy/bad-row/suggestion assertions。
- `crates/ralph-core/tests/diagnosis_report_bundle.rs`（新增）：完整 bundle/legacy/differential report。
- `crates/ralph-cli/src/commands/diagnose.rs`：只运行现有 CLI tests 验证新 output；本 Unit 不修改该 CLI 入口或参数。

#### 7. 可依赖能力

Unit 1 manifest、Unit 2 runtime records、Unit 3 feedback records、现有 `load_session` warning model、`aggregate_recovery`、`render_markdown/render_json`、现有 safe display。

#### 8. 禁止依赖的未来能力

不得让 reporter 修改 artifact；不得自动调用 git 修复/ralph run；不得把 skill 的源码归因逻辑移入 reporter；不得删除旧字段或要求新 bundle 才能出报告。

#### 9. 验收测试

- `bundle_first_report_contains_lifecycle`：完整 fixture，断言 D14 的 `diagnosis_input.status=present`、runtime summary、feedback rows、suggestion/evidence refs。
- `legacy_session_falls_back`：只放旧文件，断言 report status legacy/warning，旧 findings 仍存在。
- `malformed_bundle_and_trace_are_warnings`：坏 JSON/坏行，断言继续输出且 evidence gap/confidence cap。
- `suggestions_are_non_executing`：断言每条建议含 `tier/finding_refs/evidence_refs/confidence/text`，且 reporter 没有文件写入/命令执行副作用。
- 命令：`cargo nextest run -p ralph-core -- diagnosis_report_bundle`、`cargo nextest run -p ralph-cli --bin ralph -- diagnose`。

#### 10. Acceptance Red

先新增 `bundle_first_report_contains_lifecycle`，在 reporter 尚未读取新文件时预期 D14 的 `diagnosis_input`/lifecycle keys 缺失或状态错误；若测试只因 fixture 路径/JSON 语法失败，不算有效 Red。

#### 11. 单元测试拆分

1. manifest parser：valid/missing/bad schema。
2. trace JSONL reader：valid/bad/blank/partial tail。
3. feedback lifecycle grouping：same id/retry key and final status。
4. evidence gap aggregation：missing artifact/ref does not become fact。
5. additive JSON mapping：old keys exact, new keys present。
6. Markdown escaping/truncation：复用现有 safe display。
7. suggestion tier mapper：runtime/preset/agent/environment/orchestration。

#### 12. Red → Green → Refactor 顺序

`bundle_first_report_contains_lifecycle` Red → `SessionData`/D14 readers → Green → `malformed_bundle_and_trace_are_warnings` Red → fail-soft readers → Green → `legacy_session_falls_back` Red → bundle absent fallback with fixed legacy shape → Green → `suggestions_are_non_executing` Red → additive report mapper/renderers → Green → 抽取 reader/provenance helpers → full reporter tests Green。

#### 13. 最小实现范围

必须实现 D14 fixed shape、bundle-first、raw verification、legacy fallback、warning/evidence gap、lifecycle/suggestions 的 Markdown/JSON；不得改 ledger mode、CLI args、旧 schema、历史 confidence rubric。

#### 14. 集成验证

真实 `load_session` + real files + `Report::from_session` + `render_json/markdown`；CLI 使用现有 `diagnose` dispatch。运行 core reporter and CLI diagnose targeted nextest。

#### 15. 风险驱动测试

Differential（bundle present vs absent）证明 legacy findings 不丢；Fault Injection（bad manifest/JSONL）证明 fail-soft；Golden 只允许新增稳定 section/key，必须人工审 diff，不得无解释更新。

#### 16. 回归范围

`crates/ralph-core/src/diagnosis/tests.rs`、`diagnostics_e2e.rs`、`crates/ralph-cli/src/commands/diagnose.rs` embedded tests、`tests/scenarios/diagnose_from_ledger.yml`、`mechanism/foundation/diagnosis_count_matches_final_state.yml`；这些覆盖旧 report、ledger-first、schema count 和 CLI session resolution。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/diagnosis/reporter.rs` | 修改生产文件 | bundle-first reader/D14 report fields | E10/E11/E16/E23 |
| `crates/ralph-core/src/diagnosis/tests.rs` | 修改测试 | parser/legacy/bad-row/suggestion | E10 |
| `crates/ralph-core/tests/diagnosis_report_bundle.rs` | 新增测试 | ATDD/differential/fixed schema | E10/E13/E23 |
| `crates/ralph-cli/src/commands/diagnose.rs` | 运行现有测试，不修改生产入口 | 保持 CLI output contract | E11 |

#### 18. 完成标准

S8–S11 和 R4–R12/R19 测试通过；旧 diagnose CLI/ledger/scenario tests 通过；D14 old keys unchanged/new keys fixed；Markdown/JSON both available；无写操作；fmt/clippy/build 通过；可独立提交。

#### 19. 停止条件

旧 JSON consumer 需要 schema bump、D14 字段无法固定、legacy report 无法生成、reporter 发生写操作、bundle 结论覆盖 accepted chronology、suggestion 需要自动执行命令时停止并更新 D7/D8/D14。

#### 20. 风险与注意事项

风险是 reporter 3394 行文件继续膨胀和旧 JSON contract 漂移；新增 reader/helper 可拆入 `diagnosis` 新模块（不超过 5000 行），只做 D14 additive fields，并跑旧 embedded tests；剩余风险是外部消费者对未知 JSON key 的错误解析，v1 旧 keys 保持且文档明确 additive。

### Unit 5：把 `run-diagnosis` 改为 bundle-first 并保留原工作流

#### 1. Unit 目标

同一个 `run-diagnosis` skill 在 Phase 0 先消费 bundle/structured session report，再做 raw verification、四问、history gate、源码归因和修复建议；缺 bundle 时回退旧 Tier 流程；Markdown 之外固定生成同 basename 的 JSON sidecar。

#### 2. 对应需求与 Scenario

R7–R12、R18–R19；S8–S11；D7、D9、D14；E10–E13/E18/E23。

#### 3. 外部可观察结果

Operator 仍执行原 skill invocation；skill 复用已有 CLI，以 `--legacy --session latest --diagnostics-root "$RUN/.ralph/diagnostics" --format json --output "$REPORT_BASE.json"` 生成 session-scoped 结构化 sidecar，不引入新命令或实时控制台；报告前置显示 `diagnosis_input`、runtime trace/feedback 状态、source integrity、evidence gaps，随后才进入历史和深度归因。

#### 4. 当前行为基线

当前 Phase 0 直接读 `current-events` 和 Tier inventory；`artifact-discovery.md` 只列旧 diagnostics 文件；`report-template.md` 没有 bundle/lifecycle/suggestion structured refs。历史默认 disabled 和四问/置信度门禁是必须保持的基线。

#### 5. 输入与输出

- 输入：`run_dir`、preset、可选 plan、`--include-history`；可选 `diagnosis-input.json`、runtime trace、feedback。
- 输出：现有 `docs/report/YYYY-MM-DD-<preset-basename>-<loop_id>-diagnosis.md`，以及同 basename 的 `.json` 结构化 sidecar；frontmatter/sections 增加 bundle status、trace/feedback status、structured result ref、evidence gaps；建议仍只人工修复建议。
- `REPORT_BASE` 解析算法固定复用现有 `artifact-discovery.md`/`report-template.md`：`PRESET_BASENAME` 为 `builtin:<name>` 去掉前缀后的 name，或 preset 文件 basename 去掉 `.yml`；`LOOP_ID` 依次读 `$RUN/.ralph/current-loop-id`、`loops.json` 中 workspace 匹配 RUN 的最新 entry、trusted `current-events` 文件名；仍无值使用字面 `unknown` 并写 evidence gap；`REPORT_BASE=docs/report/$(date +%Y-%m-%d)-$PRESET_BASENAME-$LOOP_ID-diagnosis`。session selector 固定为已有 CLI 的 `latest`，diagnostics root 固定为 `$RUN/.ralph/diagnostics`，不得让 Executor 重新选择 selector。
- 错误：缺 bundle → legacy；缺 `current-events` 的原有停止门禁不变；坏新文件 → report warning/置信度降级。
- 状态变化：只在主仓写诊断报告，不写 run artifacts、不改代码/plan/task。
- 不变量：history disabled 默认不跨仓扫描；四问不合并；不因 capability 缺 artifact 错报机制故障。

#### 6. 修改位置

- `skills/ralph-run-diagnosis/SKILL.md`：在 Phase 0 插入 bundle-first、manifest verification、trace/feedback 读取顺序；保留 AskUserQuestion/history gate/四问/低置信度规则。
- `skills/ralph-run-diagnosis/references/artifact-discovery.md`、`artifact-manifest.md`：登记新文件、status、integrity 和 legacy fallback。
- `references/log-reconciliation.md`、`verification-pipeline.md`：规定 runtime trace/feedback → accepted events/raw artifacts 的核验优先级。
- `references/report-template.md`：新增 frontmatter/sections，固定 suggestion tiers/evidence gaps/structured result ref。
- `references/ssot-guardrails.md`、`confidence-rubric.md`：补充新 artifact 和 mode cap，不改变既有分数规则。
- `skills/tests/test_run_diagnosis_contract.py`（新增）：只检查稳定入口/字段/顺序/禁止项，不锁全文 prompt。

#### 7. 可依赖能力

Unit 4 `ralph diagnose --legacy --format json` additive output、Unit 1/2/3 manifest/trace/feedback、现有 skill references、history gate。

#### 8. 禁止依赖的未来能力

不得新增实时 agent/hat；不得自动生成 plan/执行修复；不得把 `ralph diagnose` 失败变成当前 skill 的唯一 hard dependency；不得删旧 Tier inventory。

#### 9. 验收测试

- `skill_bundle_first_contract`：读取 skill/references，断言 bundle verification 在 raw inventory/历史前，legacy fallback、history disabled、四问和 repair-only 仍存在。
- `skill_report_template_tracks_lifecycle`：断言 frontmatter/sections 有 diagnosis_input/trace/feedback/evidence gaps/suggestion tiers/structured ref。
- `skill_writes_structured_report_sidecar`：断言 skill 在已按固定算法得到 `REPORT_BASE` 后执行 `ralph diagnose --legacy --session latest --diagnostics-root "$RUN/.ralph/diagnostics" --format json --output "$REPORT_BASE.json"`，并在 Markdown frontmatter 指向该路径；该命令失败时保留 Markdown 报告并写 evidence gap，不自动执行任何修复。
- `skill_legacy_mode_contract`：断言 manifest 缺失仍执行 current-events/Tier legacy path，且不把缺新文件标 P0。
- 命令：`.venv/bin/python -m pytest skills/tests/test_run_diagnosis_contract.py`；若 `.venv` 不存在，先按仓库 Python 环境约定准备，不用系统 Python 代替。

#### 10. Acceptance Red

先新增 contract test，当前 skill 缺 bundle-first/新 artifact anchors，预期断言失败；若失败来自 Python environment/pytest missing，先恢复 `.venv`，不能把环境失败当有效 Red。

#### 11. 单元测试拆分

1. argument contract unchanged。
2. Phase ordering: bundle verify before raw/history。
3. legacy fallback and current-events hard stop unchanged。
4. history disabled default/placeholder unchanged。
5. report template stable frontmatter/sections。
6. repair-only wording and no auto-plan/execute rule。
7. terminology: human.guidance historical, task.resume runtime transport。

#### 12. Red → Green → Refactor 顺序

`skill_bundle_first_contract` Red → 更新 SKILL Phase 0 → Green → `skill_report_template_tracks_lifecycle` Red → 更新 template/references → Green → `skill_writes_structured_report_sidecar` Red → 接入已有 `ralph diagnose --legacy --session latest --diagnostics-root ... --format json --output ...` → Green → `skill_legacy_mode_contract` Red → 补 discovery/verification fallback → Green → 抽取 SSOT artifact vocabulary/交叉链接 → Python contract suite Green。

#### 13. 最小实现范围

只修改 skill/reference Markdown 和新增 contract test；保留原参数、history gate、四问、report path、低置信度规则；复用已有 `ralph diagnose` CLI 生成 JSON sidecar，不新增 Rust runtime 行为或新的 CLI。

#### 14. 集成验证

用真实仓库 references 运行 skill contract tests；用一个已存在旧 report/fixture 进行人工 checklist，确认 skill 不要求新文件才能继续；使用一个临时 `RUN` fixture 验证 `PRESET_BASENAME`、`LOOP_ID`、`REPORT_BASE`、`--legacy --session latest --diagnostics-root` 参数和 JSON output path；运行 `scripts/check-cli-doc-drift.sh --strict` 前置 build。

#### 15. 风险驱动测试

Characterization（legacy path）、contract test（稳定 anchors）、mode-aware confidence test（DISABLED/MINIMAL）是必需的；不做 LLM judge 或 E2E，因为本 Unit 改操作规程，不改变 runtime。

#### 16. 回归范围

`skills/tests/test_execution_model_contract.py`、`test_prompt_visibility_contract.py`、所有 `skills/ralph-run-diagnosis/references` 链接；原因是新增 references 可能破坏路径、SSOT 锚点和 prompt visibility 规则。运行 `.venv/bin/python -m pytest skills/tests`（完成新增 targeted test 后）。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `skills/ralph-run-diagnosis/SKILL.md` | 修改文档/skill | bundle-first workflow | E12 |
| `skills/ralph-run-diagnosis/references/{artifact-discovery,artifact-manifest,log-reconciliation,report-template,verification-pipeline,ssot-guardrails,confidence-rubric}.md` | 修改文档 | artifact/lifecycle/report contract | E10/E12/E13 |
| `skills/tests/test_run_diagnosis_contract.py` | 新增测试 | stable skill contract | E12/E18 |

#### 18. 完成标准

S8–S11 skill contract 通过；legacy/history/capability rules 未削弱；报告仍只给建议；references 链接有效；Python tests/doc drift precheck 通过；可独立提交。

#### 19. 停止条件

skill 必须改变用户 invocation、需要实时控制台、缺 bundle 时无法工作、历史 disabled 被绕过、低置信度门禁变弱时停止，回退 D9 重新决策。

#### 20. 风险与注意事项

风险是新 bundle-first 文案误把 runtime reporter 当自动执行器，或 `latest` diagnostics root 解析到错误 worktree；在文档中固定“只读、只报告、只建议”，contract test 检查 no-auto-plan/execute 和固定 selector/output algorithm；剩余风险是不同安装环境缺少新 CLI binary，保留 Markdown legacy path。

### Unit 6：修正 README、运行时诊断指南和 agent-facing recovery 术语

#### 1. Unit 目标

让用户文档、skill references、agent-facing recovery guidance 和代码注释准确描述当前真实路径：`human.guidance` 仅历史/兼容，`task.resume` 是 runtime recovery transport，correction/resume context 是 agent 可见的结构化上下文。

#### 2. 对应需求与 Scenario

R13–R15；S12；D10；E14/E15。

#### 3. 外部可观察结果

Operator 不会再从 README 得到“runtime 发布 human.guidance”或“task.resume 已完全不存在”的矛盾指引；agent-facing 文档仍能按真实 task.resume/recovery directives 行动。

#### 4. 当前行为基线

`README.md:146,206` 把 human.guidance 描述为当前 runtime guidance；`docs/guide/runtime-diagnosis.md:5` 又宣称 task.resume 被完全取代，但后文和当前代码仍使用 task.resume；`troubleshooting.md`、部分 comments 也有不一致。E14/E15 已确认运行时事实。

#### 5. 输入与输出

- 输入：当前代码真实 topic/routing/recovery docs。
- 输出：README/guide/troubleshooting/agent skill/comments 一致；历史 fixture/compat test 文案明确标注。
- 错误：文档错误不能通过删除 runtime 兼容路径修复。
- 状态变化：仅文本/注释/contract tests。
- 不变量：`task.resume` runtime routing、recovery target/dedup、human.guidance historical parsing/test fixtures 保持。

#### 6. 修改位置

- `README.md`：修正两处 current human guidance 描述，改为 diagnose/report/operator signal semantics。
- `docs/guide/runtime-diagnosis.md`：删除顶部“task.resume 已完全取代”矛盾声明，统一其实际 recovery routing、correction context、report terminology。
- `docs/reference/troubleshooting.md`：保留 task.resume troubleshooting，删除 human guidance current-path 说法。
- `crates/ralph-core/data/ralph-tools.md`、`ralph-tools-recovery-directives.md`、`ralph-tools-emit.md`、`ralph-tools-tasks.md`：只澄清 transport/intervention/resume context，不改 agent 可执行命令或字段。
- `crates/ralph-core/src/correction/mod.rs`、`diagnosis/mod.rs`、`event_loop/prompt_injection.rs`：只修正已由 characterization grep 命中的、与当前代码冲突的 module docs/comments；不改运行分支。
- `skills/tests/test_run_diagnosis_contract.py`：补术语 contract；保留 existing legacy strings 的 negative/compat assertions。

#### 7. 可依赖能力

Unit 3 已验证 runtime task.resume 路由不变；Unit 5 已固定 skill vocabulary；现有 data docs source-of-truth 规则。

#### 8. 禁止依赖的未来能力

不得删除 task.resume、human.guidance 兼容 fixture、旧 replay parser；不得改 topic allowlist、EventOriginGuard、resume routing；不得增加新 control topic。

#### 9. 验收测试

- `current_docs_match_runtime_recovery_terms`：`rg` 检查 current docs 不把 human.guidance 作为 current control，不把 task.resume 写成已删除；同时检查 recovery directives 仍含实际字段/命令。
- `legacy_compat_terms_remain_scoped`：历史/replay/compat 位置可保留 human.guidance/task.resume，但必须邻近标识 historical/legacy/compat。
- `scripts/check-cli-doc-drift.sh --strict`：涉及 `ralph` 命令/字段的文档引用与 help 一致。

#### 10. Acceptance Red

在修改文档前执行 `rg` characterization；必须命中 README current human.guidance 和 runtime guide 的互相冲突声明。若没有命中，说明基线已变化，停止并更新 E15/D10，不直接改文案。

#### 11. 单元测试拆分

1. README current wording negative check。
2. runtime guide task.resume current transport check。
3. human.guidance historical/compat scope check。
4. agent-facing recovery command/field anchors remain。
5. CLI doc drift check。

#### 12. Red → Green → Refactor 顺序

`current_docs_match_runtime_recovery_terms` Red → 修正 README/guide/troubleshooting → Green → `legacy_compat_terms_remain_scoped` Red → 标注历史/compat 区域 → Green → recovery directives anchor Red → 只修 comments/术语而不删命令 → Green → 去重同义术语、复核所有引用 → docs gate Green。

#### 13. 最小实现范围

仅文档/注释/稳定 contract checks；不修改 runtime code paths、event schemas、CLI args、preset topology。

#### 14. 集成验证

真实 `rg` + skill tests + `scripts/check-cli-doc-drift.sh --strict`；运行相关 `ralph <cmd> --help` 只在命令语法引用有变时执行，命令不变则以现有 source/help drift script 为准。

#### 15. 风险驱动测试

Characterization/negative grep 必需，防止把现有矛盾误判为已修复；compat scope 检查防止历史证据被误删；不做生产 differential，因为 Unit 明确不改 runtime。

#### 16. 回归范围

`task_resume_runtime_routing`、`resume_routing` tests、policy/origin tests、`ralph-tools` agent-facing docs、README/guide links、skill contract tests。原因是文档改动不能诱导 agent 使用错误入口，且 comments 与真实 routing 必须一致。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `README.md` | 修改文档 | 删除 current human.guidance 错误描述 | E14/E15 |
| `docs/guide/runtime-diagnosis.md` | 修改文档 | 统一 task.resume/correction 事实 | E14 |
| `docs/reference/troubleshooting.md` | 修改文档 | current recovery terminology | E14/E15 |
| `crates/ralph-core/data/ralph-tools.md`、`ralph-tools-recovery-directives.md`、`ralph-tools-emit.md`、`ralph-tools-tasks.md` | 修改 agent-facing 文档 | transport/intervention 语义 | E14/E18 |
| `crates/ralph-core/src/correction/mod.rs`、`diagnosis/mod.rs`、`event_loop/prompt_injection.rs` | 修改注释 | 移除冲突描述，不改行为 | E14/E15/E20 |
| `skills/tests/test_run_diagnosis_contract.py` | 修改测试 | 术语 contract | E12/E15 |

#### 18. 完成标准

S12 通过；现有 recovery/task.resume/legacy tests 通过；doc drift strict 通过；没有删除 compat fixture/测试；只改文本/comments；可独立提交。

#### 19. 停止条件

发现文档修复要求改 runtime routing、删除旧 topic、变更 agent 命令或新增 CLI 时停止；记录冲突证据，回到 D10，不在文档 Unit 中实现行为变化。

#### 20. 风险与注意事项

最大风险是批量替换误伤真实 `task.resume` 操作规则；必须按文件/段落人工审 diff，保留字段/命令/停止条件；剩余风险是历史报告仍使用旧术语，skill 必须标注 historical/compat 而不是改写历史事实。

### Unit 7：跨模式零回归闭环与最终证据更新

#### 1. Unit 目标

对固定 fixtures 执行 diagnostics off/on exact differential，证明业务结果等价，差异仅为新增旁路 artifacts/report fields；这是本 Unit 唯一的最终可观察行为，不在此 Unit 修复生产逻辑。

#### 2. 对应需求与 Scenario

R16–R19；S6、S7、S9、S10、S12；D16；E1/E7/E10/E12/E13/E18/E24。

#### 3. 外部可观察结果

同一输入/基线下，diagnostics off/on 的 `(iteration, source, target, topic, payload)` accepted tuples、state projection/ledger summary、recovery `(retry_key, outcome, target, attempt)`、task.resume routing/dedup、watchdog continuation、termination reason、CLI result/exit semantics 一致；on 额外有 bundle/trace/feedback/report fields；full/minimal 和 wave/supervisor artifacts 按 capability 解释，不错报缺失。

#### 4. 当前行为基线

AGENTS hard rules 指定 nextest、BDD real runner、E2E mock、最终全量入口；已有 scenarios/replay/wave/supervisor 测试分别覆盖这些路径。E1、E13、E18 是零回归和 disabled mode 的直接证据。

#### 5. 输入与输出

- 输入：现有 scenario fixtures、replay fixtures、mock E2E、diagnostics on/off matrix。
- 输出：测试结果、最终 Evidence/Decision 更新；不新增生产行为。
- 错误：任意既有失败都禁止以更新 snapshot/skip/放宽断言解决。
- 不变量：业务事件/终态/退出码相等；只允许附加 diagnostics file/report fields 差异。

#### 6. 修改位置

- 已有 `crates/ralph-core/tests/scenarios.rs` 与 scenario fixtures：只运行既有真实 workflow scenarios，必须使用 `run_workflow_guard_scenario`；若 D16 矩阵缺少既有场景覆盖则停止，不在本 Unit 临时设计新 fixture。
- `crates/ralph-core/src/event_loop/tests/diagnostics_equivalence.rs`：扩展已注册的 comparator，按 D16 固定比较协议覆盖 normal/failure/recovery-exhausted；不得改生产代码。
- 已有 `crates/ralph-core/src/event_loop/tests/replay_light_integration.rs`、wave/supervisor tests：运行回归，不顺手重构。
- 已有 `crates/ralph-e2e` mock entry：运行回归，不改 E2E contract。
- 本计划文件：更新 Evidence/Decision/Unit completion records；不修改 `.ralph` runtime state。

#### 7. 可依赖能力

Units 1–6 全部已验证的 sidecar、reporter、skill/document contract 和现有测试 harness。

#### 8. 禁止依赖的未来能力

不得在此 Unit 修复新生产逻辑；若发现行为缺陷，停止并回到产生缺陷的 Unit；不得新增绕过测试的 env/skip/snapshot。

#### 9. 验收测试

- targeted diagnostics/reporter/skill tests 全通过。
- `cargo nextest run -p ralph-core --test scenarios`：真实 workflow guard scenarios。
- `cargo nextest run -p ralph-core --features recording --test smoke_runner`：replay/smoke。
- `cargo run -p ralph-e2e -- --mock`：E2E mock。
- `./scripts/run-tests.sh`：最终 workspace nextest + doctest。

#### 10. Acceptance Red

先运行已新增的 off/on comparator（它是本 Unit 的 Acceptance Red/回归入口）；若任一 D16 字段仍不相等，Unit 7 立即停止并回到产生差异的 Unit；全量基线失败不得被归咎于“环境”而跳过，须按 AGENTS 的 serial fallback 规则调查。

#### 11. 单元测试拆分

1. disabled/full/minimal matrix。
2. normal/failure/recovery exhausted。
3. replay light and ledger report。
4. wave/supervisor capability artifact interpretation。
5. docs/skill contract。
6. full test/build/lint/doc drift。

#### 12. Red → Green → Refactor 顺序

Unit 1–6 targeted suites Green → D16 diagnostics equivalence matrix Red/Green → scenario/replay/wave/supervisor Green → E2E mock Green → fmt/clippy/build/doc drift Green → `./scripts/run-tests.sh` Green → 只做计划内证据整理，不改实现。

#### 13. 最小实现范围

只做验证、必要 fixture 和本计划证据更新；不扩大功能、不新增 runtime branch、不修改历史运行状态。

#### 14. 集成验证

真实 scenario runner、recording smoke、mock E2E、workspace nextest/doctest；所有命令失败都阻止完成。

#### 15. 风险驱动测试

Differential/characterization 是核心：按 D16 比较 diagnostics off/on 的完整结果 tuple；wave/supervisor 是 capability-triggered regression；Fault Injection 复用 Unit 1/3/4，不重复造 mock。

#### 16. 回归范围

直接相关 diagnostics/diagnosis/loop runner tests；相邻 EventLoop recovery/origin/policy/activation/replay/wave/supervisor；旧配置、默认关闭路径、旧 session、旧 task.resume replay；`ralph-core`/`ralph-cli`/`ralph-e2e` build targets；fmt/clippy/doc drift；最终 `./scripts/run-tests.sh`。这些范围覆盖 E1–E24 证明的所有受影响调用方。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/tests/scenarios.rs` / existing scenario fixtures | 运行回归，不修改 fixture | 真实 workflow path | E18 |
| `crates/ralph-core/src/event_loop/tests/diagnostics_equivalence.rs` | 修改测试 | D16 exact off/on comparator | E22/E24 |
| `docs/plans/2026-08-12-001-feat-run-diagnosis-trace-debug-enhancement-plan.md` | 更新计划证据 | 记录执行结果 | 全部 |

#### 18. 完成标准

所有 Scenario、targeted/full tests、build、lint、fmt、doc drift、E2E mock 通过；无 skip/only/断言削弱/无解释 snapshot；所有 Decision ≥0.85；每个 Unit 有 Red→Green→Refactor→Integration→Regression→Close 记录；变更范围未超计划。

#### 19. 停止条件

任一 D16 tuple/exit semantics 不等价、诊断 off 产生新文件、full/minimal capability 误报、发现未计划公开调用方、任何 Decision <0.85 时停止；记录失败证据，回到对应 Unit，不宣布完成。

#### 20. 风险与注意事项

全量测试耗时/并发 flake 必须按 `./scripts/run-tests.sh` 两阶段策略和 AGENTS serial fallback 处理，不能手工裸跑 ralph-cli cargo test；剩余风险是外部 filesystem/kill 造成尾行损坏，reporter 的坏行降级测试必须保持通过。

## 8. Unit 串行依赖图

```text
Unit 1
  ↓ 提供已验证的 input manifest schema、原子 writer、metadata/status、disabled/no-op 契约
Unit 2
  ↓ 提供已验证的 runtime trace records、runner metadata wiring、off/on 基线 comparator
Unit 3
  ↓ 提供已验证的 feedback lifecycle records、真实 action/validation/final refs
Unit 4
  ↓ 提供已验证的 bundle-first reporter、D14 legacy fallback、JSON/Markdown 建议字段
Unit 5
  ↓ 提供已验证的 run-diagnosis bundle-first 操作规程和固定 sidecar 解析算法
Unit 6
  ↓ 提供已验证的当前 recovery terminology 与 agent-facing 文档一致性
Unit 7
```

- Unit 2 不能早于 Unit 1，因为所有 runtime hook 必须调用已验证的 sidecar contract；Unit 1 不得提前接入 hook。
- Unit 3 不能早于 Unit 2，因为 feedback sequence/iteration 必须复用已验证的 runtime trace identity；Unit 3 不得提前修改 reporter。
- Unit 4 不能早于 Unit 3，因为 reporter 的 lifecycle assertions 必须消费真实 runtime 和 feedback records；Unit 4 不得提前修改 EventLoop。
- Unit 5 不能早于 Unit 4，因为 skill 需要稳定的 D14 bundle/report 字段；Unit 5 不得新增 CLI/runtime behavior。
- Unit 6 不能早于 Unit 5，因为文档术语必须和最终 skill instructions 一致；Unit 6 不得通过删 runtime topic 解决文字冲突。
- Unit 7 必须最后执行，因为它按 D16 比较 Units 1–6 的最终结果；发现实现缺陷必须回到原 Unit，不在 Unit 7 临时拍板。

## 9. 执行命令清单

| 命令 | 时机 | 目的 | 预期 | 失败处理 |
|---|---|---|---|---|
| `cargo nextest run -p ralph-core -- diagnosis_input_bundle` | Unit 1 Red/Green | input manifest contract | targeted tests pass | 不得进入 Unit 2 |
| `cargo nextest run -p ralph-core -- diagnostics_equivalence` | Unit 2/7 | runtime trace + D16 differential | fixed records/invariants pass | 回到产生差异的 Unit |
| `cargo nextest run -p ralph-core -- feedback_lifecycle` | Unit 3 | feedback identity/source lifecycle | tests pass | 不得进入 Unit 4 |
| `cargo nextest run -p ralph-core -- diagnosis_report_bundle` | Unit 4 | reporter D14 bundle/legacy/bad-row | Markdown/JSON pass | 不得进入 Unit 5 |
| `cargo nextest run -p ralph-cli --bin ralph -- diagnose` | Unit 4 | CLI diagnose path | existing CLI tests pass | 回到 Unit 4 |
| `ralph diagnose --legacy --session latest --diagnostics-root "$RUN/.ralph/diagnostics" --format json --output "$REPORT_BASE.json"` | Unit 5 | 生成 skill 的稳定结构化 sidecar | JSON sidecar 可读，旧 session 仍可 fallback | 记录 evidence gap；不得自动执行修复 |
| `.venv/bin/python -m pytest skills/tests/test_run_diagnosis_contract.py` | Unit 5/6 | skill/terminology contract | anchors/fallback/sidecar pass | 先修 Python env/skill，不跳过 |
| `.venv/bin/python -m pytest skills/tests` | Unit 7 | all skill regression | all pass | 不得完成 |
| `bash scripts/check-cli-doc-drift.sh --strict` | Unit 5/6/7 | CLI/doc references | no drift | 修文档/命令引用 |
| `cargo fmt --all -- --check` | 每个 Rust Unit close | formatting | pass | 不得进入下一 Unit |
| `cargo clippy --all-targets --all-features -- -D warnings` | Unit 1–4/7 | lint/type-level quality | pass | 修复后重跑 |
| `cargo build` | Unit 1–4/7 | workspace build | pass | 不得完成 |
| `cargo nextest run -p ralph-core --test scenarios` | Unit 7 | real BDD workflow | pass | 回到失败 Unit |
| `cargo nextest run -p ralph-core --features recording --test smoke_runner` | Unit 7 | replay/smoke | pass | 不得完成 |
| `cargo run -p ralph-e2e -- --mock` | Unit 7 | mock E2E | pass | 不得完成 |
| `./scripts/run-tests.sh` | Unit 7 final | required full nextest + doctest | pass | 按 AGENTS serial fallback 调查，不跳过 |

## 10. 最终质量门禁

- 所有 S1–S12 和 R1–R19 均有通过的验收测试与 Unit 映射。
- bundle/trace/feedback schema round-trip、坏行、缺失、部分写入、重复 recovery、失败后恢复、legacy fallback 均通过。
- diagnostics off/on 业务事件、recovery decision、termination reason、exit code 等价；full/minimal/wave/supervisor 只产生 capability-appropriate artifacts。
- `ralph diagnose` Markdown/JSON 输出通过，旧 JSON keys 未删除/改类型；repair suggestions 有 evidence/confidence 关联且不执行修改。
- `run-diagnosis` skill 仍按现有 invocation 使用；history 默认 disabled、四问、低置信度门禁和 current-events hard stop 均保留。
- human.guidance 不再被新文档当作当前 control；task.resume 的当前 runtime transport 文档与代码一致；旧 compat/replay 证据保留。
- `cargo nextest` targeted/full、BDD scenario、recording smoke、mock E2E、fmt、clippy、build、doc drift、skill `.venv` tests 全通过。
- 没有新增 skip/only、削弱断言、无解释 snapshot/golden、手工 `.ralph` 状态修改或未处理 BLOCKED 决策。
- 所有 Decision 仍 ≥0.85；实际变更不超出本计划；每个 Unit 独立提交且严格串行完成。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每个 Unit 指定真实入口、Red、最小实现、命令、回归和停止条件。 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D16 已固定路径、schema、开关、错误、兼容、metadata、生命周期来源、差分协议和报告映射。 |
| 所有文件和接口是否有代码库证据 | 是 | 现有路径均由 E1–E24 确认；新增文件明确标注“新增”。 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1–D16 为 0.93–0.99。 |
| 是否存在未处理的低置信度假设 | 否 | 原 open questions 已转为 D1–D16；失败条件要求停止重查。 |
| 每个 Unit 是否只有一个可观察行为 | 是 | Unit 1 manifest、Unit 2 runtime trace、Unit 3 feedback lifecycle、Unit 4 reporter、Unit 5 skill sidecar、Unit 6 terminology、Unit 7 exact off/on equivalence 各自独立。 |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 都有 targeted command、Red、集成和回归范围。 |
| 每个 Unit 是否有真实 Red | 是 | Unit 1–6 明确新增测试后的缺能力/缺字段/冲突文档 Red；Unit 7 运行已存在的 exact comparator，任何不等价都是有效 Red。 |
| 每个 Unit 是否包含回归范围 | 是 | 第 16 项逐 Unit 列明直接/相邻/公开消费者/关闭路径。 |
| 是否存在未来 Unit 依赖 | 否 | 依赖只向前，禁止提前实现未来行为。 |
| 是否存在泛化任务描述 | 否 | 未使用“完善逻辑/视情况修改”等替代实现边界。 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 第 5、6、7 节逐项映射。 |
| 所有关键决策是否有 Evidence | 是 | D1–D16 各自列 E IDs。 |
| 计划是否可以严格串行执行 | 是 | 第 8 节只有 Unit 1→2→3→4→5→6→7。 |
