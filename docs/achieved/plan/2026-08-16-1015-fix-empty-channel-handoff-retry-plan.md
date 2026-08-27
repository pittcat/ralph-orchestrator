---
title: "fix: 修复空 channel 终态恢复与 handoff timeout 短路"
date: 2026-08-16
type: fix
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

# 空 channel 终态恢复与 handoff timeout：实现级开发计划

## Goal Capsule

### 0. 计划状态

- **READY**：所有进入实施的关键决策置信度均为 0.85 以上；没有把“test-stabilizer 未激活的更底层 transport 位置”伪装成已知事实。
- **代码基线**：仓库 `HEAD=3c8f76e58ae1dbf0e898bc7f8e0efb910ea0b4ee`，当前分支 `pittcat-dev`。已有用户工作区改动只包括三个未跟踪诊断/旧计划文档；本计划不覆盖它们，也不修改 `crates/ralph-core/src/event_loop/worktree_handoff.rs`。
- **调查范围**：`ralph-core` 的 missing-terminal recovery、task.resume payload、ProtocolView、recovery finalizer、handoff tracker、target routing、相关测试；`ralph-cli` 的空 channel 调用入口和既有观测；`ce-executor-pipeline` / loop preset 的 executor/fixer/stabilizer contract；`crates/ralph-core/data/*.md`；preset author/review operator 文档。
- **已执行的验证命令**：
  - `cargo nextest run -p ralph-core -- handoff_dispatch_timeout_forces_plan_blocked_when_pending`：1 个测试通过，确认当前基线会把 pending handoff timeout 直接变成 `ForcePlanBlocked`。
  - `cargo nextest run -p ralph-core -- test_missing_terminal_emit_recovery_targets_same_hat_with_typed_resume`：1 个测试通过，确认现有 missing-terminal recovery 会构造同一 hat 的 task.resume。
  - `cargo nextest run -p ralph-core -- jsonl_task_resume_preserves_target_and_activates_original_hat`：1 个测试通过，确认既有 target 路由和原 hat 激活路径可用。
  - `rg` / `sed` 对源码、preset、schema、skill 文档及测试位置进行了逐项核对。
- **尚未执行的验证**：本计划尚未修改代码，因此尚未执行计划内的新 Red、全量 `./scripts/run-tests.sh`、preset lint、BDD scenario、clippy、check 或 build；这些属于 Executor 按 Unit 串行执行的门禁。
- **阻塞项**：无实施阻塞项。历史 run 缺少 `orchestration.jsonl` 和 `agent-output.jsonl`，因此“test-stabilizer 更底层是 queue、merge、backend 还是 agent 未启动”仍是诊断未知项；本计划不依赖该未知项，而修复已经由 runtime trace、recovery feedback 和源码共同证明的短路策略。

### 0.1 P0 根因边界

P0 不是“所有 hat 都没有 emit”，也不是已证明的 `worktree_handoff.rs` 缺陷。已确认的 P0 是：`work.done` 被接受后，`test-stabilizer` handoff 发生 `handoff_dispatch_timeout`；`dispatch_and_handoff` 已生成 targeted `task.resume`，但 `recovery_runtime::finalize_recovery_outcome` 对仍处于 non-terminal 的 handoff recovery 立即发出 `ForcePlanBlocked`，下游没有机会完成一次有限重试。因此本计划同时做三件事：

1. 通用 data skill 动态描述“当前 hat 的终态契约”，不把 `work.done` / `work.failed` 写成全局规则。
2. executor/fixer 在各自 preset instructions 中明确自己的终态和 apply/confirm 收尾动作。
3. runtime 保留现有 targeted `task.resume` 路由，修正 missing-terminal payload 的字段语义，并把 handoff timeout 改成“先有限重试，达到既有上限才 blocked”。
4. 将终态路由补成可审计链：`topic → producer → explicit target → consumer → next terminal event`；`report.done` 必须显式 target `reporter`，且 runtime/preset 都拒绝 target 与唯一 consumer 不一致的事件。

## Product Contract

### 1. 功能目标

- **业务目标**：当一个 agent activation 成功退出但没有提交其当前 hat 声明的终态事件时，下一次恢复应给该责任 hat 足够且正确的动态契约；当下游 handoff 暂时超时时，runtime 不应在第一次 timeout 后立即终止整条审验链。
- **用户/调用方**：loop 内 agent、`ce-executor-pipeline` 的 executor/fixer/stabilizer 等 hat、EventLoop 的 handoff/recovery 调用方、preset author/reviewer。
- **当前行为**：
  - `crates/ralph-cli/src/loop_runner/inner.rs` 在 backend 成功、channel 为空且 hat 有 terminal obligation 时调用 `inject_missing_terminal_emit_recovery`。
  - `event_processing.rs` 使用 `build_task_resume_payload`，把 `terminal_topics` 传入该函数的 `required_fields` 参数；因此恢复 payload 的 `required_fields` 目前是 topic 名，而不是 schema 字段名。
  - `dispatch_and_handoff.rs` 已将 handoff timeout 转为 targeted `task.resume`，但 `finalize_recovery_outcome.rs` 在同一 retry key 仍为 non-terminal 时立即 `ForcePlanBlocked`。
  - `event_reader.rs` 将 JSONL 的 `triggered` 转为事件 target；`determine_active_hat_ids` 优先采用该 target；accepted event 的 handoff registration 却只调用 `handoff_index.consumer_of(topic)`，没有要求两者一致。
  - `ce-executor-pipeline` 的 reporter 已在 instructions、`triggers` 和 `terminal_events` 中表达 `report.done → reporter → LOOP_COMPLETE`，但 `EventSchema` 没有 target contract 字段。
  - data skill 已说明 `policy-check` 不等于写入完成，但没有在顶部提供足够短的、动态的 activation exit protocol。
- **目标行为**：
  - agent 离开 activation 前，只根据当前 hat 的 declared terminal contract 完成一个合法终态的 apply/confirm；没有终态义务的 hat 不得自行发明业务事件。
  - missing-terminal recovery payload 同时携带 `terminal_topics`、每个终态 topic 对应的真实 `terminal_required_fields`，并让既有 `required_fields` 表示 primary terminal topic 的字段；原始 trigger、target hat、allowed topics、retry key 保持可追踪。
  - 第一次 `handoff_dispatch_timeout` 不产生 `plan.blocked`；已有 targeted resume 留在队列/路由中。相同 retry key 达到配置的 `max_repeated_recoveries` 后才允许 finalizer 产生 `plan.blocked`。
  - 对声明了 `required_target_hat` 的终态 topic，缺失或错误的 explicit target 在进入 main bus 前被拒绝；`report.done` 的 target 必须是 `reporter`，接受后的下一次 reporter activation 才能发 `LOOP_COMPLETE`。
- **行为差异**：首次 handoff timeout 从“立即终止”变为“有限恢复”；missing-terminal agent 从“可能读到 topic 名作为字段名”变为“读取动态 topic→字段契约”；report.done 从“target 可任意覆盖但 handoff 仍按 topic 等待”变为“schema 声明 target，runtime 与 handoff consumer 一致”。已有 missing-terminal retry cap、业务终态伪造禁止、target 优先路由均保持。
- **本次范围**：data skill、两个 ce executor preset 的 executor/fixer/reporter instructions 与 operator contract、missing-terminal task.resume payload、handoff timeout finalizer 的 bounded retry、终态 target/consumer schema+linter+runtime guard、真实 EventLoop BDD/回归测试。
- **非目标**：不新增 direct reactivation API；不修改 `worktree_handoff.rs`；不新增业务 topic；不让 runtime 伪造 `work.done`、`fix.done` 或其他业务终态；不改空 channel 已落地的 activation outcome 观测；不通过保存完整 agent transcript 来推断 agent 根因。
- **输入**：activation 的 declared terminal topics、hat publishes、EventLoop event policy schema、原始 trigger、retry key/attempt、handoff timeout recovery envelope、`telemetry.runtime_diagnosis.max_repeated_recoveries`。
- **输出**：agent-facing 动态收尾指引；结构化 `task.resume` payload；targeted resume 或达到上限后的 `plan.blocked`；现有 recovery envelope/feedback；测试证据。
- **状态变化**：retry key 从 pending → targeted resume/retry；成功消费并发出合法终态后进入 recovered/下游 handoff；达到 retry cap 后进入 blocked。首次 timeout 不改变 plan 为 blocked。
- **错误语义**：`policy-check ok=true` 不是写入完成；只有 apply 返回 `recorded=true` 才算事件落盘。schema 不满足时 agent 必须根据结构化 rejection 修正；runtime 不替 agent 填充未知业务字段。retry exhausted 才是 blocked，且原因必须保留 retry key 和 target。
- **兼容性要求**：保留既有 `build_task_resume_payload` 调用者的现有字段；新增终态契约字段为 additive。仅对声明 `required_target_hat` 的 topic 开启 target fail-close；其他 topic 的既有 explicit target 语义不变。未携带新增字段的旧 task.resume 仍按现有 `required_fields` 处理。
- **性能要求**：不增加常态 activation 的等待、进程或网络调用；只在 recovery payload 构造时从已加载 config 建立 ProtocolView；retry 次数受已有配置上限约束。
- **安全/权限要求**：payload 只传 topic、字段名、retry 元数据和原始业务 trigger 中已有内容；不复制 prompt、完整 agent 输出、secret 或 ledger 内部路径。现有 event policy、target 和 emit ACL 继续生效。
- **已确认假设**：既有 targeted `task.resume` 可以被 `next_hat` 和 `determine_active_hat_ids` 选中；`RuntimeDiagnosisConfig.max_repeated_recoveries` 默认值为 3 且已验证存在；executor/fixer/reporter 的具体终态由 preset 的 `terminal_events`/`triggers` 声明；真实 scenarios 测试使用 `run_workflow_guard_scenario`。
- **待验证假设**：无实施关键的未决假设。Unit 3 仍必须用 test-first 验证“第一次 handoff timeout 不再 blocked、达到 cap 才 blocked”；Unit 4 必须先证明 `required_target_hat` 能在既有 emit pipeline 中 fail-close，再接入 preset/schema。若 Red 失败表现不是目标行为差异而是 fixture/调用链错误，必须停止并更新证据。

### 2. 代码库现状与证据

#### 2.1 当前实现入口

- **外部入口**：`ralph emit --policy-check` / apply 是 agent 可见的事件命令；activation close 在 `crates/ralph-cli/src/loop_runner/inner.rs` 判定 backend success + empty terminal channel 后进入 core recovery。
- **调用链**：`inner.rs::immediate_missing_terminal_emit` → `EventLoop::inject_missing_terminal_emit_recovery` → `rejection::build_task_resume_payload` → `resume_routing::task_resume_ingress`；handoff 侧为 `dispatch_and_handoff` 生成 timeout resume → `runtime_recovery_context` → `recovery_runtime::dispatch` → `finalize_recovery_outcome_on_flapping` → `apply_runtime_recovery_actions`。
- **核心模块**：`event_processing.rs` 负责 missing-terminal recovery；`rejection.rs` 负责 task.resume wire payload；`preset/engine/protocol.rs` 的 `ProtocolView` 是 event policy schema required fields 的 SSOT；`finalize_recovery_outcome.rs` 负责 recovery finalization；`handoff_tracker.rs` 负责一次 timeout escalation 和 pending 清理；`task_resume_runtime_routing.rs` 覆盖真实 target 路由。
- **终态路由模块**：`event_reader.rs::From<Event>` 把 `triggered` 转为 `ralph_proto::Event.target`；`dispatch_and_handoff.rs::determine_active_hat_ids` 按 target 优先选 hat；`acceptance_and_lifecycle.rs::apply_contract_committed_side_effects` 却按 `handoff_index.consumer_of(topic)` 注册 deadline；`event_loop/stage_pipeline.rs` 是所有 hat emit 的统一 gate，`target_hat_guard_stage.rs` 已存在但目前只检查 task.resume self-loop，默认 pipeline 未用于 report.done target/consumer contract。
- **数据边界**：JSONL event → EventReader/acceptance → EventBus/hat selection；恢复 payload 进入既有 task.resume ingress；preset YAML/schema 提供 terminal topics 和 required fields；runtime recovery envelope 提供 retry 状态。
- **外部依赖**：Rust workspace、serde JSON/YAML、EventBus、JSONL events file、backend activation；本计划不引入新 crate、不访问网络服务。
- **现有测试**：`event_loop/tests/loop_context.rs` 的 missing-terminal tests；`event_loop/tests/task_resume_runtime_routing.rs` 的 target 路由；`recovery_runtime/finalize_recovery_outcome.rs` 的 handoff finalizer tests；`event_loop/tests/handoff_dispatch.rs` 与 `recovery_envelope_u7_u8.rs` 的 handoff/recovery 证据；`skills/ralph-preset-review/tests/test_skill_anchors.py` 的 operator 文档稳定 anchor。
- **构建/验证方式**：仓库硬规则要求 `cargo nextest run` 系列；最终全量使用 `./scripts/run-tests.sh`，另跑 preset lint、BDD scenarios、`cargo check`、clippy、build 和 `scripts/check-cli-doc-drift.sh`。

#### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `docs/report/2026-08-16-ce-executor-pipeline-2026-08-15-2211-fix-state-machine-transaction-boundary-plan-diagnosis.md` 的 activation/recovery 对账 | `work.done` accepted 后没有任何 `test-stabilizer` activation；随后出现 `handoff_dispatch_timeout`、recovery escalation 和 blocked report。 | P0 修复必须覆盖 handoff timeout finalization，不能只改 agent prompt。 | 高：当前 run 的结构化 trace、feedback、recovery 对账 |
| E2 | `crates/ralph-core/src/recovery_runtime/finalize_recovery_outcome.rs::handoff_timeout_pending` | 只要同 key 有 StallRecovery handoff timeout 且 outcome non-terminal，就返回 `ForcePlanBlocked`；当前 baseline test 明确断言该行为。 | 首次 timeout 的立即 blocked 是可直接复现的策略缺口；Unit 3 先改它。 | 高：源码 + nextest 通过 |
| E3 | `crates/ralph-core/src/event_loop/dispatch_and_handoff.rs`、`state_recovery.rs` | timeout 已构造 targeted `task.resume`；target event 有 fast path；`jsonl_task_resume_preserves_target_and_activates_original_hat` 通过。 | 不新增第二套 direct reactivation；保留现有路由，只改变 finalizer 何时阻断。 | 高：调用链 + 可执行测试 |
| E4 | `crates/ralph-core/src/event_loop/event_processing.rs::inject_missing_terminal_emit_recovery` | `terminal_topics` 被作为 `build_task_resume_payload` 的第三个参数传入，而该参数名是 `required_fields`。 | Unit 2 必须修正字段来源，并增加 topic→字段的结构化 payload。 | 高：直接调用点 |
| E5 | `crates/ralph-core/src/event_loop/rejection.rs::build_task_resume_payload` 与其 `build_task_resume_payload_includes_all_context` 测试 | builder 语义明确把第三个参数写入 `required_fields`；已有测试传入 `plan_path` 等真实字段。 | 保留旧 builder 签名，新增专用 terminal-contract builder，避免破坏其他调用者。 | 高：源码 + 单测 |
| E6 | `crates/ralph-core/src/preset/engine/protocol.rs::ProtocolView::required_fields_for` | required fields 从 `event_policy.schemas` 与 execution contract 计算，支持按 topic 查询。 | Unit 2 使用 ProtocolView 动态获取字段，不复制 preset 字段表，不新增配置。 | 高：SSOT 实现 |
| E7 | `crates/ralph-core/src/recovery_runtime/mod.rs::RuntimeContext`、`prompt_injection.rs::runtime_recovery_context` | RuntimeContext 收集 retry state；EventLoop 可从 `self.config.telemetry.runtime_diagnosis.max_repeated_recoveries` 读取配置并构造 context。 | Unit 3 将把既有配置 cap 传入 finalizer，避免再造硬编码 cap。 | 高：类型和构造入口已确认 |
| E8 | `crates/ralph-core/src/config/telemetry.rs` | `max_repeated_recoveries` 已存在，默认值为 3，且 0 会被拒绝。 | 不新增 CLI/config 字段；handoff timeout 与现有 recovery retry policy 共用 cap。 | 高：配置定义、默认值和校验测试 |
| E9 | `presets/en/ce-executor-pipeline.yml:2241,3077,4877` 及 loop preset 对应 hat | executor 终态是 `work.done`/`work.failed`，stabilizer 是 `stabilization.done`/`stabilization.blocked`，fixer 是 `fix.done`；不同 hat 终态不同。 | 通用 skill 必须动态；executor/fixer 才能写具体 topic。 | 高：preset YAML |
| E10 | `crates/ralph-core/data/ralph-tools.md`、`ralph-tools-emit.md`、`ralph-tools-recovery-directives.md` | 已有 OPAC、policy-check/apply/confirm、task.resume 和 bounded retry 说明，但缺少顶部短 exit protocol；恢复说明已有动态字段原则。 | Unit 1 是增补与收敛，不创建第二套 emit 规则。 | 高：当前注入文档 |
| E11 | `presets/en/*-preset-author-notes.md` | loop preset author notes 仍写有 “无 runtime resume API”，与当前实际 task.resume recovery 不一致；executor/fixer contract 已分别列出。 | Unit 1 同步修正文档事实，避免 operator/reviewer 继续使用过时假设。 | 高：文档直接矛盾 |
| E12 | `cargo nextest` 基线测试 | handoff timeout force-block、missing-terminal same-hat resume、target preserve/activation 均通过。 | 证明 Red 可针对现有行为建立，且 target 路由不是待猜测问题。 | 高：可执行结果 |
| E13 | `AGENTS.md` preset/data skill 同步规则与 `skills/ralph-preset-review/tests/test_skill_anchors.py` | data docs 必须 agent-facing、preset author/review references 必须同步；已有 anchor 测试禁止锁定完整 prompt。 | 文档修改必须同步两套 operator references，测试只能锁稳定标题/结构，不锁完整 prose。 | 高：仓库硬规则 + 现有测试 |
| E14 | `docs/report/2026-08-16-ce-executor-pipeline-2026-08-15-2211-fix-terminal-artifact-admission-plan-diagnosis.md` | 实际 `report.done` 记录了 `triggered: executor`；handoff recovery 仍按 topic 等待 reporter；随后 executor 重入、duplicate work.done 和 no-progress，人工补发 LOOP_COMPLETE。 | 新增 P0 路由一致性 Unit；不能只修 handoff retry。 | 高：当前 run trace、feedback、recovery 与源码对账 |
| E15 | `presets/en/ce-executor-pipeline.yml:5474-5508`、`presets/schemas/ce-executor-pipeline.yml:1164-1189`、对应 loop 文件 | reporter 的 `triggers`/`terminal_events` 和 schema 文档表达 `report.done → reporter → LOOP_COMPLETE`，但 schema 没有 `required_target_hat`。 | schema SSOT 增加结构化 target contract；inline/schema 两份必须同步。 | 高：preset 与 schema 直接证据 |
| E16 | `crates/ralph-core/src/event_reader.rs:182-191`、`dispatch_and_handoff.rs:19-63` | `triggered` 变成 target，target 优先于 topic；该语义是当前路由实际行为，不是 agent prompt 推测。 | runtime guard 必须在 emit pipeline 中拒绝声明 target contract 的错误/缺失 target。 | 高：源码调用链 |
| E17 | `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs:1011-1059`、`workflow_contract/handoff_index.rs:228-230` | accepted event 的 handoff consumer 独立按 topic 查找并注册 tracker，没有消费 explicit target。 | 只有 target contract 校验通过的 event 才允许进入 handoff registration；成功事件的 target 与 consumer 必须相同。 | 高：源码直接证据 |
| E18 | `crates/ralph-core/tests/scenarios/ce_executor_pipeline.yml`、`scenarios.rs::run_workflow_guard_scenario`、最近 `cargo nextest run -p ralph-core --test scenarios ce_executor_pipeline` | 已有真实 EventLoop BDD 链覆盖 align.done/report.done/LOOP_COMPLETE，相关 21 tests 当前全通过；fixture 目前不能为 mock response 指定 `triggered`。 | 扩展 scenario fixture 的 `triggered` 字段，加入正确 target 与错误 target 的 accepted/absent 场景；不使用 stub。 | 高：真实测试入口 + 可执行基线 |
| E19 | `crates/ralph-cli/src/commands/emit/command_impl.rs:1119-1154`、`loop_runner/hat_channel.rs:125-150,305-316`、`crates/ralph-core/data/ralph-tools-emit.md:75-80,184-193`、本轮 `rg` 检查 | CLI 在 isolated 模式只对缺失 target 自动按 topic consumer 推导；显式 target 会被保留；agent-facing 文档要求普通 handoff 省略显式 target。当前未发现 reporter prompt 写入 `triggered: executor` 的证据，不能把来源归因给 prompt；但 CLI policy 只校验 target 是否存在于 topology，未校验 target 是否等于该 topic consumer。 | Unit 4 增加“来源核验 + CLI policy + runtime stage”三层检查；若继续缺少 agent-output，只保留 unknown，不强判 agent。 | 高：源码、skill 文档和搜索结果一致 |

#### 2.3 受影响范围

- **生产模块**：`crates/ralph-core/src/event_loop/event_processing.rs`、`event_loop/rejection.rs`、`recovery_runtime/mod.rs`、`recovery_runtime/finalize_recovery_outcome.rs`、`event_loop/prompt_injection.rs` 的 context 构造。
- **新增/扩展终态路由模块**：`crates/ralph-core/src/config/loop_config.rs::EventSchema`、计划新增的 `preset_lint/target_routing.rs`、计划新增的 `event_loop/stages/terminal_target_guard_stage.rs`、`stage_pipeline.rs`、`flow_wiring.rs`。
- **测试模块**：`event_loop/tests/loop_context.rs`、`event_loop/tests/task_resume_runtime_routing.rs`、`recovery_runtime/finalize_recovery_outcome.rs`、`event_loop/tests/handoff_dispatch.rs`、`ralph-cli` 的 `policy_check/u6_unified_path_tests.rs` 与 `commands/emit/tests_integration.rs`、必要时 `recovery_runtime/mod.rs` 单测。
- **配置/preset**：`presets/en/ce-executor-pipeline.yml`、`presets/en/ce-executor-pipeline-loop.yml`、两个对应 schema；Unit 1 只检查 instructions parity，Unit 4 同步已有 `report.done.required_target_hat`；不改变 topic topology、producer、consumer 或 payload required_fields。
- **agent-facing data**：`crates/ralph-core/data/ralph-tools.md`、`ralph-tools-emit.md`、`ralph-tools-recovery-directives.md`。
- **operator 文档**：两套 `skills/ralph-preset-author` / `skills/ralph-preset-review` 的 SKILL/reference；两个 ce executor preset author notes。
- **API/CLI/UI/外部服务**：不新增公开 API、CLI 参数、UI 或外部服务；既有 `ralph emit --policy-check`/apply 入口增加对已声明 required target 的 fail-close 校验，参数和非 contract topic 语义不变。
- **构建目标**：至少 `ralph-core`、`ralph-cli` preset lint/presets、BDD scenarios、全 workspace tests/build/clippy/check。
- **终态路由构建目标**：`ralph-core` config/schema deserialization、preset lint finding、stage pipeline、`ce_executor_pipeline` BDD scenario；`ralph-cli` 的 preset embedding/parity 作为回归。

### 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | handoff timeout 应修改哪一层？ | 改 `worktree_handoff`/新增 direct reactivation；改 `dispatch_and_handoff` 路由；改 recovery finalizer 的阻断时机 | 修改 `recovery_runtime/finalize_recovery_outcome.rs`，保留现有 timeout→targeted resume 路由 | E1、E2、E3、E12 | 现有 target resume 已能路由并激活；事件未激活的已知 P0 是随后 immediate `ForcePlanBlocked`。 | 0.95 |
| D2 | retry cap 使用什么来源？ | 新增配置；硬编码新常量；复用 `RuntimeDiagnosisConfig.max_repeated_recoveries` | 复用已有配置，将有效 cap 放入 `RuntimeContext`；默认 3、配置为 0 已被拒绝 | E7、E8 | 新配置会扩大 CLI/config/schema 影响；硬编码会与现有诊断策略漂移。 | 0.90 |
| D3 | 修复 `required_fields` 的 wire contract 如何兼容多终态 topic？ | 直接把 topic 名继续塞入旧字段；把旧字段改成所有字段 union；保持旧 builder 并新增 `terminal_topics` + `terminal_required_fields`，旧 `required_fields` 表示 primary topic | 保持旧 builder API；新增专用 terminal contract helper，payload additive 写入 `terminal_topics`、`primary_terminal_topic`、`terminal_required_fields`，并传真实 primary fields 给 `required_fields` | E4、E5、E6、E9 | 多终态 topic 可能有不同 schema；map 能避免 agent 猜字段，旧调用者不需改签名。 | 0.91 |
| D4 | generic data skill 是否规定 `work.done/work.failed`？ | 全局固定两个 topic；按当前 hat 动态 contract；完全不写 exit rule | 按当前 hat terminal contract 动态规定；executor/fixer 在 preset 中各自写具体 topic | E9、E10、E13 | 固定 topic 会误导 stabilizer/reviewer/reporter；完全不写规则不能降低 no-emit 风险。 | 0.99 |
| D5 | 是否修改 preset schema/topology？ | 新增业务 topic/schema；修改已有 terminal events；只修改 instructions/author notes；为已有 report.done 增加 target contract | 不改变 topic topology/terminal events；Unit 1 只改 instructions/notes，Unit 4 在现有 `EventSchema` 增加 `required_target_hat: reporter` 并同步四份 SSOT/schema | E9、E11、E15、E18 | 不新增业务 topic、不改 producer/consumer topology；只增加 runtime 能执行的 per-topic target contract，正是本次 P0 缺失的结构化约束。 | 0.94 |
| D6 | 是否加入 emit audit 或保存完整 agent output？ | 新增完整 transcript 持久化；改已有 activation outcome 观测；本计划不扩展观测 | 本计划消费已有 empty-channel/outcome 观测，不新增 audit；agent 根因保持 unknown | E1、E10、E13 | 当前请求目标是 retry/task.resume/check；完整 transcript 有安全和范围风险，且不是 P0 必需证据。 | 0.93 |
| D7 | 如何表达 `report.done` 的显式目标？ | 只靠 reporter prompt；新增独立 preset 字段；在现有 per-topic `EventSchema` 增加 `required_target_hat` | 在 `EventSchema` 增加可选 `required_target_hat`；`report.done` 在 linear/loop 两套 SSOT 都声明 `reporter` | E15、E9 | prompt 不能作为 runtime gate；独立顶层表会创建第二套 topic SSOT；per-topic schema 已是事件字段契约入口。 | 0.93 |
| D8 | target/consumer 不一致在哪一层拒绝？ | accepted 后仅不注册 handoff；只做 preset lint；只改 CLI policy；CLI policy + StagePipeline defense-in-depth，并让 lint 做启动前 fail-fast | 静态 lint 校验 `required_target_hat == 唯一 triggers consumer`；对声明 required target 的 topic，isolated CLI 不先自动补全缺失 target，CLI `--policy-check`/apply 先拒绝缺失或错配；StagePipeline 对直接 JSONL/channel path 再在 main bus 前拒绝；accepted side effect 只处理已通过的 event | E16、E17、E18、E19 | 只跳过 tracker 会让错误 event 已进入 bus；只做 lint/CLI 无法保护绕过 CLI 的 channel；全局禁止所有 explicit target 会破坏已有 target override 语义。 | 0.92 |
| D9 | 是否为 `report.done` 增加新的 next-terminal 字段？ | 新增 `next_terminal_topic`；从 reporter `triggers`/`terminal_events` 派生；只依赖 scenario | 不新增 next-terminal 字段；保留现有 reporter `triggers=[report.done]`、`terminal_events=[report.done,LOOP_COMPLETE]`，由真实 scenario 验证下一激活 | E15、E18 | 现有 preset 已结构化表达 reporter 自闭环；新增字段会扩大 schema/preset parity 但不解决 target mismatch。 | 0.92 |

没有低于 0.85 的实施决策。若 Unit 3 发现 `RuntimeContext` 无法安全承载配置 cap，或 Unit 4 发现 CLI policy 和 StagePipeline 都无法在各自入口取得 event target contract，必须停止，不得由 Executor 自行改为硬编码、post-commit 修补或新增第二套路由；先补充构造链证据并重算 D2/D8。

### 4. BDD 行为规格

```gherkin
Feature: 按当前 hat contract 完成 activation 收尾并恢复 handoff

  Background:
    Given loop 使用 isolated hat activation
    And runtime 已加载该 hat 的 publishes、terminal_events 和 event policy schema

  Scenario: 通用 agent 收尾遵循当前 hat 的动态终态
    Given 当前 hat 的终态是其 contract 声明的 terminal topic
    When agent 离开 activation
    Then agent 只对 allowed_topics 中的一个合法终态执行 policy-check、apply 和 confirm
    And apply 的结果必须包含 recorded=true
    And agent 不得把 work.done 或 work.failed 当作所有 hat 的默认终态

  Scenario: 没有终态义务的 activation 不伪造业务事件
    Given 当前 hat 没有 declared terminal obligation
    When activation 正常结束且没有业务事件
    Then runtime 不创建 missing-terminal task.resume
    And runtime 不伪造任何业务终态

  Scenario: executor 和 fixer 使用各自的终态
    Given executor 的 contract 声明 work.done/work.failed
    And fixer 的 contract 声明 fix.done
    When executor 或 fixer 完成收尾
    Then executor 只从自己的 allowed terminal topics 中选择
    And fixer 不发送 executor 的 work.done/work.failed

  Scenario: 空 channel 恢复携带动态终态字段
    Given hat activation backend 成功退出
    And channel 为空且该 hat 有 terminal topics
    When runtime 创建 missing-event task.resume
    Then task.resume.target_hat 等于原责任 hat
    And payload.terminal_topics 等于该 hat 的 terminal topics
    And payload.terminal_required_fields[topic] 等于该 topic schema 的字段集合
    And payload.required_fields 是 primary terminal topic 的字段名而不是 topic 名
    And payload.original_trigger_topic、original_trigger_payload、allowed_topics、retry_key 保留

  Scenario: 第一次 handoff timeout 先恢复而不阻断
    Given work.done 已被接受并唯一消费者的 handoff 尚未激活
    When 第一次 handoff_dispatch_timeout 被记录
    Then runtime 保留 targeted task.resume
    And runtime 不产生 ForcePlanBlocked
    And target consumer 仍可被 next_hat 选中

  Scenario: handoff timeout 达到既有 cap 后阻断
    Given 同一个 handoff timeout retry key 的 outcome 仍是 non-terminal
    And attempt_count 已达到 telemetry.runtime_diagnosis.max_repeated_recoveries
    When recovery finalizer 再次运行
    Then runtime 产生一次 plan.blocked
    And blocked reason 包含 retry key
    And runtime 不再生成无限新的 task.resume

  Scenario: 成功恢复后不误阻断
    Given handoff timeout recovery 已产生 targeted task.resume
    When target hat 被激活并提交合法终态
    Then retry outcome 进入 terminal/recovered 语义
    And finalizer 不产生 ForcePlanBlocked
    And 下游 handoff 按原有 event topology 继续

  Scenario: report.done 的显式 target 与唯一 consumer 一致
    Given preset schema 为 report.done 声明 required_target_hat=reporter
    And reporter 是 report.done 的唯一 topic consumer
    When reporter 在 align.done 后发出 report.done 并显式 target reporter
    Then report.done 被接受并注册 reporter 的下一次 handoff
    And 下一次 reporter activation 只发 LOOP_COMPLETE

  Scenario: report.done 指向 executor 时在 main bus 前被拒绝
    Given preset schema 为 report.done 声明 required_target_hat=reporter
    When JSONL report.done 携带 triggered=executor
    Then emit gate 返回 terminal_target_mismatch
    And report.done 不进入 main EventBus
    And 不创建等待 reporter 的错误 handoff timeout

  Scenario: 真实 EventLoop 完成 align.done 到 reporter 终态闭环
    Given ce_executor_pipeline scenario 使用真实 run_workflow_guard_scenario
    And mock response 为 report.done/LOOP_COMPLETE 分别携带 triggered=reporter
    When EventLoop 处理 align.done、report.done、LOOP_COMPLETE
    Then accepted event 顺序包含 align.done、report.done、LOOP_COMPLETE
    And LOOP_COMPLETE 的 report_path 与 accepted report.done 一致
```

### 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐层级 | 风险补充测试 | 是否 E2E |
|---|---|---|---|---|---|
| S1 | 文档有稳定 `Activation exit protocol` anchor；内容引用当前 terminal contract、allowed_topics、apply `recorded=true`，没有全局 work.done/work.failed | `skills/ralph-preset-review/tests/test_skill_anchors.py` | 文档 contract test | 不锁全文；检查两套 operator skill anchor | 否 |
| S2 | YAML 结构仍声明 executor/fixer 各自 terminal_events；instructions 的具体 topic 与结构化 contract 对齐 | `cargo nextest ... preset_lint`、`presets` | preset structural/lint | 不新增 prose byte-equality test | 否 |
| S3 | executor/fixer author notes 不再声称无 runtime resume API，且各自 payload contract 仍对应 schema | 两个 `presets/en/*-preset-author-notes.md`，preset lint | 文档+结构化 lint | skill anchor 回归 | 否 |
| S4 | missing-terminal JSON payload 的 target、topics、map、primary fields、trigger、retry key 全断言；topic 名不得出现在 required_fields 数组 | `event_loop/tests/loop_context.rs`、`event_loop/rejection.rs` | 单元+EventLoop 集成 | 两个终态不同字段的 fixture，防止 union/错位 | 否 |
| S5 | attempt=1 的 handoff timeout finalizer 返回无 ForcePlanBlocked；target routing test 仍通过 | `recovery_runtime/finalize_recovery_outcome.rs`、`task_resume_runtime_routing.rs` | 单元+集成 | Fault injection：timeout 后 target consumer 未立即激活 | 否 |
| S6 | attempt=cap 返回恰好一个 ForcePlanBlocked；低于 cap 不 block；同 key 不产生无限动作 | finalizer tests、recovery runtime tests | 单元/state transition | boundary cap-1/cap/cap+1 | 否 |
| S7 | terminal/recovered outcome 不 block；无 terminal obligation 不触发 missing recovery | `loop_context.rs`、finalizer tests | 单元/集成 | 现有 no-op/supervisor coordination 回归 | 否 |
| S8 | report.done 的 schema target、唯一 consumer、runtime target 一致 | CLI policy、target contract lint、stage tests | 单元+结构化 lint | 缺失、错误、正确 target 三态；required-target 不被 auto-derive | 否 |
| S9 | align.done → reporter(report.done) → reporter(LOOP_COMPLETE) 真实闭环 | `crates/ralph-core/tests/scenarios/ce_executor_pipeline.yml` + `scenarios.rs` | BDD/EventLoop 集成 | accepted/absent event、payload 一致性 | 否 |

所有测试都必须断言副作用和不变量：target 不漂移、业务 event 不由 runtime 伪造、retry key 稳定、payload 不含 secret、现有非 terminal recovery 不被改变。

### 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | 通用收尾规则按当前 hat 动态执行 | S1、S2 | skill anchor + preset lint | 不适用 | preset structural checks | 否 | E9、E10、E13 |
| R2 | 无终态义务不伪造恢复/业务事件 | S2、S7 | missing-terminal guard test | `loop_context` no-terminal test | EventLoop recovery path | 否 | E9、E12 |
| R3 | executor/fixer 各自发送自己的终态 | S3 | preset lint + notes anchor | 不适用 | preset/schema parity | 否 | E9、E11 |
| R4 | empty channel resume payload 提供正确动态字段 | S4 | loop_context payload assertions | rejection helper test | EventLoop recovery integration | 否 | E4、E5、E6 |
| R5 | 第一次 handoff timeout 不立即 blocked | S5 | finalizer attempt=1 test | finalizer unit test | target routing integration | 否 | E1、E2、E3、E12 |
| R6 | retry cap 后明确 blocked 且不无限重试 | S6 | cap boundary tests | finalizer/recovery unit | recovery action dispatch | 否 | E7、E8 |
| R7 | 成功恢复后继续下游，不误 block | S7 | recovered-terminal test | finalizer unit | existing handoff tests | 否 | E1、E3 |
| R8 | 终态 topic 显式 target 必须与唯一 consumer 一致 | S8 | CLI policy + target guard/lint tests | EventSchema/target contract unit | CLI precheck 与 EventLoop emit rejection | 否 | E14–E19 |
| R9 | reporter 终态闭环自然完成，不依赖人工补发 LOOP_COMPLETE | S9 | scenario accepted event sequence | scenario harness unit | `run_workflow_guard_scenario` | 否 | E15、E18、E19 |

## Planning Contract

### 7. 严格串行开发单元

执行顺序固定为：

`Unit 1 → Unit 2 → Unit 3 → Unit 4`

每个 Unit 都必须完成 Acceptance Red → Unit Red → Green → Refactor → Integration → Regression → Close 后才能进入下一个 Unit。

### Unit 1：建立动态 activation exit protocol，并强化 executor/fixer 合同

#### 1. Unit 目标

让 agent 在离开 activation 前读取当前 hat 的终态 contract，并确认该 contract 允许的终态已 `recorded=true`；executor/fixer 在 preset 中分别看到自己的具体终态。该 Unit 不改 runtime。

#### 2. 对应需求与 Scenario

- Requirement：R1、R2、R3。
- Scenario：S1、S2、S3。
- Decision：D4、D5。
- Evidence：E9、E10、E11、E13。

#### 3. 外部可观察结果

agent prompt 顶部能看到短的动态 exit protocol；收到 recovery 时会从当前 contract/payload 取得 topic 和字段，不会把 work.done/work.failed 误用于 stabilizer、reviewer、reporter。executor/fixer instructions 与 YAML terminal_events 一致。

#### 4. 当前行为基线

`ralph-tools.md` 已有 emit/apply/confirm 和 recovery 说明，但没有独立短 exit protocol；`ce-executor-pipeline.yml` 已声明不同 hat 的不同 terminal_events；author notes 仍出现 “无 runtime resume API”。这些是 E9–E11 的真实基线。`test_skill_anchors.py` 当前没有新 anchor，因此先增加稳定 anchor test 使缺失文档成为真实 Red。

#### 5. 输入与输出

- 输入：当前 prompt 中的 hat terminal contract、`allowed_topics`、`required_fields`，以及 preset 中 executor/fixer 的既有 terminal_events。
- 输出：agent-facing 文档和 preset/operator 文档；不产生 runtime event。
- 错误：`policy-check` 通过但 apply 未 `recorded=true` 时，文档要求停止并修正/重试，不得宣称完成。
- 状态/副作用：只改变注入文本和 operator guidance；不改变事件拓扑、schema 或 CLI。
- 不变量：没有 terminal obligation 的 hat 不发明 topic；具体 topic 不进入 generic data skill 的固定规则；若当前 topic contract 声明了 required target，agent 必须使用该 contract 指定的 target，不能自行改写为其他 hat。

#### 6. 修改位置

- `crates/ralph-core/data/ralph-tools.md`：共享 agent 命令入口；增加短 `Activation exit protocol`，说明触发条件、terminal contract 来源、policy-check/apply/confirm 和失败停止条件。
- `crates/ralph-core/data/ralph-tools-emit.md`：emit 细节；补充 terminal topic/field 取值的动态来源和 `recorded=true` 作为完成条件；普通 handoff 仍省略显式 target，但遇到已声明 required target 的 topic 时必须使用该目标并在 policy-check 失败时停止。
- `crates/ralph-core/data/ralph-tools-recovery-directives.md`：恢复动作；把 missing-event recovery 的动态终态消费顺序写清楚，不增加 preset 专属 topic。
- `presets/en/ce-executor-pipeline.yml` 的 executor、fixer instructions：executor 具体列出 `work.done`/`work.failed`，fixer 具体列出 `fix.done`，均要求 apply 后确认 `recorded=true`。
- `presets/en/ce-executor-pipeline-loop.yml` 的 executor、fixer instructions：同步 loop 版本的专属终态和 resume 收尾。
- 两个 `presets/en/*-preset-author-notes.md`：删除与当前 task.resume recovery 冲突的“无 runtime resume API”表述，记录动态 contract 与各 hat 终态边界。
- `skills/ralph-preset-author/SKILL.md`、`skills/ralph-preset-review/SKILL.md` 及其 `references/commands.md`、`references/finding-rubric.md`：增加 operator 评审点；不复制完整 prompt。
- `skills/ralph-preset-review/tests/test_skill_anchors.py`：只增加上述稳定标题/契约 marker，不锁定完整 prose。

明确不修改：`presets/manifest.yml`、`presets/index.json`、zsh completion（preset 名称未变）；两个 schema 先不改；任何 Rust 生产代码不改。

#### 7. 可依赖能力

已有 data skill 的 `ralph emit` 文档、现有 YAML terminal_events、preset lint、skill anchor test。

#### 8. 禁止依赖的未来能力

不得提前写入 `terminal_required_fields` 新 payload 字段；不得提前修改 runtime retry finalizer；不得把 Unit 2 的 wire contract 当成当前已存在事实。

#### 9. 验收测试

- 测试名称：`test_skill_anchors` 新增 `Activation exit protocol`、动态 terminal contract、`recorded=true` 稳定 markers。
- 前置：checkout 当前基线；新 anchor 尚未存在。
- 动作：运行 Python anchor test；运行两个 preset lint 和 presets parity test。
- 断言：anchor 缺失时先 Red；Green 后两个 data skill 文档都明确 generic/dynamic 语义，YAML terminal_events 未被改写。
- 副作用/不变量：git diff 只包含文档/preset instructions/operator reference/test anchor；不出现全局 work.done/work.failed 规则。
- 命令：`skills/.venv/bin/python -m pytest skills/ralph-preset-review/tests/test_skill_anchors.py -q`；`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`；`cargo nextest run -p ralph-core -- preset_lint`；`cargo nextest run -p ralph-cli --bin ralph -- presets`。

#### 10. Acceptance Red

首先新增 anchor 断言并运行 `skills/.venv/bin/python -m pytest ...test_skill_anchors.py -q`；预期失败为指定 `Activation exit protocol` marker 不存在。该失败直接证明目标 agent-facing contract 尚未落地。若失败是 Python 环境缺失、导入错误或 fixture 缺失，不算有效 Red，必须先修复测试环境/路径并重新运行。

#### 11. 单元测试拆分

1. `test_activation_exit_protocol_anchor_present`：检查共享 data skill 的稳定标题；不检查全文。
2. `test_dynamic_terminal_contract_anchor_present`：检查 emit/recovery 文档包含动态来源和 `recorded=true` marker。
3. 现有 YAML structural lint：输入 executor/fixer topology，断言 publishes/terminal_events 没有被错误合并。
4. 不允许用测试锁定完整 preset instructions；不 Mock `ralph emit` 的结构化 lint。

#### 12. Red → Green → Refactor 顺序

`Activation exit anchor Red` → 写入 generic dynamic exit section → anchor Green → `recorded=true`/recovery marker Red → 补齐 emit/recovery 文档 → marker Green → 更新 executor/fixer 与两份 author notes → preset lint Green → 合并 author/review reference wording → rerun anchor test → Refactor 去重并保留一处共享定义。

#### 13. 最小实现范围

必须实现：generic dynamic exit rule、executor/fixer 专属终态 checklist、operator review anchor、resume 事实不被旧文档否定。必须保持：topic topology/schema/CLI 不变。明确不实现：runtime payload、新 retry cap、emit audit、任何业务事件自动生成。

#### 14. 集成验证

真实运行 preset lint、schema parity 和 skill anchor test；不需要真实 backend/E2E。检查 `presets/schemas/ce-executor-pipeline*.yml` 与 YAML topology 未发生结构性漂移。

#### 15. 风险驱动测试

只做文档 anchor 和结构化 lint。风险依据是 E13 明确禁止脆弱 prompt 文本测试；不需要 concurrency/fault injection。

#### 16. 回归范围

直接相关：两个 preset lint、presets parity、skill anchor；相邻：author/review references；公开调用方：无；旧配置/数据：无变更；构建目标：`ralph-core` data embedding 和 `ralph-cli` preset embedding 由 lint/build 检查。因为只改文档与 instructions，不跑全量前不得宣称完成。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/data/ralph-tools*.md` 三个文件 | 修改 agent-facing 文档 | 动态 exit/recovery contract；补充 required target topic 的条件规则 | E10、E15、D4 |
| `presets/en/ce-executor-pipeline*.yml` 两个文件 | 修改 preset instructions | executor/fixer 专属终态 | E9 |
| `presets/en/ce-executor-pipeline*-preset-author-notes.md` 两个文件 | 修改 operator notes | 删除过时 resume 假设 | E11 |
| 两套 `skills/ralph-preset-{author,review}` SKILL/references | 修改文档 | 同步 operator 审计规则 | E13 |
| `skills/ralph-preset-review/tests/test_skill_anchors.py` | 修改测试 | 锁稳定 marker，不锁 prose | E13 |

#### 18. 完成标准

当前 S1–S3 通过；anchor、preset lint、schema parity 通过；无新增 skip/only、无削弱断言；未改 schema/topology；Evidence/Decision 更新；Unit 可独立提交。

#### 19. 停止条件

如果 preset 实际 terminal_events 与 E9 不符、generic 文档必须写死 topic 才能通过测试、或 operator reference 需要新增未调查 finding_id，停止；记录新证据，重新决策，不进入 Unit 2。

#### 20. 风险与注意事项

| 风险 | 触发条件 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| generic rule 误导非 emitter hat | 文档把具体 topic 写成默认 | anchor review + 人工 diff | 只写 contract/allowed_topics 动态语义 | agent 仍可能不遵守，交由 runtime gate/retry 兜底 |
| author notes 与 YAML 漂移 | 两个 preset 版本不同步 | preset lint + diff review | 同一 Unit 同步修改两套文件 | 未来新增 preset 仍需 operator audit |

### Unit 2：修复 missing-terminal task.resume 的动态字段契约

#### 1. Unit 目标

当 empty channel 触发 missing-terminal recovery 时，task.resume payload 提供真实的 topic→required fields 契约，同时保持原 hat target、原始 trigger 和既有 retry 语义。

#### 2. 对应需求与 Scenario

- Requirement：R4、R2。
- Scenario：S4、S7。
- Decision：D3。
- Evidence：E4、E5、E6、E9、E12。

#### 3. 外部可观察结果

恢复 agent 能从 payload 直接知道可发布的 terminal topics 和每个 topic 的字段；`required_fields` 不再包含 `work.done`/`work.failed` 等 topic 名。target 仍是原责任 hat，runtime 仍不伪造业务事件。

#### 4. 当前行为基线

`event_processing.rs` 将 `terminal_topics` 传给 `build_task_resume_payload(... required_fields ...)`；现有 `loop_context` 测试只检查 payload 包含 terminal topic 文本，没有约束字段语义。先新增双终态、不同 schema 字段的 acceptance test，当前实现应以 `required_fields` 错误语义失败，形成真实 Red。

#### 5. 输入与输出

- 输入：`hat_id`、`terminal_topics`、`allowed_topics`、`ProtocolView` 对每个 topic 的 required fields、原始 trigger、rejection/retry key。
- 输出：现有 task.resume payload 加 `terminal_topics`、`primary_terminal_topic`、`terminal_required_fields` object；既有 `required_fields` 改为 primary topic 的字段数组。
- 错误：topic 没有 schema 时该 topic 的 map 值为空数组；不得把 topic 名伪装成字段。原有 rejection/target 错误保持。
- 状态/副作用：只改变 recovery event payload，不改变 retry counter、事件接受策略或业务事件。
- 不变量：`allowed_topics` 仍来自 hat publishes；`primary_terminal_topic` 必须来自 terminal_topics；字段集合排序稳定；原始 trigger payload 不丢失。

#### 6. 修改位置

- `crates/ralph-core/src/event_loop/rejection.rs::build_task_resume_payload`：保持现有签名和现有 callers；新增明确命名的 terminal-contract helper，负责 additive 写入 map。
- `crates/ralph-core/src/event_loop/event_processing.rs::inject_missing_terminal_emit_recovery`：通过 `ProtocolView::from_event_loop(&self.config.event_loop)` 查询各 terminal topic 字段；调用新 helper；primary fields 传入旧 `required_fields`。
- `crates/ralph-core/src/event_loop/mod.rs`：只在需要对外暴露 core 内测试 helper 时增加已有风格的 re-export；若测试可走模块内路径则不改。
- `crates/ralph-core/src/event_loop/tests/loop_context.rs`：扩展现有 missing-terminal tests，加入不同 schema 的双终态断言。
- `crates/ralph-core/src/event_loop/rejection.rs` 内 tests：增加 helper wire-shape test。

明确不修改：`dispatch_and_handoff.rs`、`resume_routing.rs`、`ralph-cli` runner、preset schema/topology、旧 builder 的其他调用方。

#### 7. 可依赖能力

Unit 1 已确认 agent 将消费动态 fields；既有 ProtocolView、event policy schemas、task.resume ingress、missing-terminal retry counter 和 loop_context fixtures。

#### 8. 禁止依赖的未来能力

不得修改 handoff finalizer；不得改变 `U2_REJECTION_RETRY_LIMIT`；不得新增业务 topic 或自动填业务 payload；不得等待 Unit 3 的 timeout 策略。

#### 9. 验收测试

- 测试名称：`missing_terminal_resume_exposes_per_topic_required_fields`（新增）；`build_task_resume_payload_with_terminal_contract_preserves_legacy_fields`（新增）。
- 前置：构造包含两个 terminal topics 且两个 schema 字段不同的 EventLoop config；触发空 channel recovery。
- 动作：调用真实 `inject_missing_terminal_emit_recovery`，解析产生的 task.resume payload。
- 断言：`target_hat`、`allowed_topics`、`original_trigger_*`、`retry_key` 保持；`terminal_required_fields[topic]` 与 ProtocolView 一致；`required_fields` 等于 primary topic 字段；topic 名不在 fields 数组。
- 副作用/不变量：只产生一个 targeted task.resume；retry attempt 仍递增；不产生 work.done/work.failed 业务事件。
- 命令：`cargo nextest run -p ralph-core -- missing_terminal_resume_exposes_per_topic_required_fields`；`cargo nextest run -p ralph-core -- build_task_resume_payload_with_terminal_contract`；相关 `loop_context` 全部 targeted test。

#### 10. Acceptance Red

先写双终态 schema fixture 和精确 fields assertions，再运行 `cargo nextest run -p ralph-core -- missing_terminal_resume_exposes_per_topic_required_fields`。当前代码预期失败：`required_fields` 收到 topic 名，且 `terminal_required_fields` 不存在。这证明测试确实穿过真实 missing-terminal payload builder。编译错误、fixture 未加载或 0 tests run 不算有效 Red。

#### 11. 单元测试拆分

1. builder 的 additive JSON shape：输入 primary fields + per-topic map，断言 legacy fields 和新 fields。
2. fields 排序/空 schema：输入 HashSet 无序字段和 unknown topic，输出稳定数组/空数组。
3. EventLoop recovery integration：输入双终态真实 config，断言从 ProtocolView 获取而非手写字段。
4. 不允许 Mock ProtocolView 或只断言 payload 包含 topic 字符串；必须验证字段值和 target。

#### 12. Red → Green → Refactor 顺序

`双终态 fields assertion Red` → 新增 terminal-contract builder → builder test Green → `EventLoop payload Red` → 在 recovery 中构造 ProtocolView/map 并传 primary fields → integration Green → 增加 unknown/排序边界测试 → Refactor helper 命名/序列化顺序 → rerun rejection、loop_context、task.resume routing tests。

#### 13. 最小实现范围

必须实现 additive payload contract、真实 schema 查询、primary fields 语义修正、稳定序列化和现有上下文字段保留。必须保持 retry cap、target routing、event acceptance 不变。明确不实现 handoff retry、new public config、业务 event synthesis。

#### 14. 集成验证

真实联合 `EventLoop`、`ProtocolView`、rejection builder、task.resume ingress 和 EventBus/hat queue；可以用现有 in-memory EventLoop fixture，不 Mock payload builder、ProtocolView 或 target routing。执行 ralph-core targeted nextest 和 task.resume routing test。

#### 15. 风险驱动测试

需要 Characterization：现有 builder context tests 必须继续通过。需要 Contract：双终态 map 防止字段误读。需要 Property-like boundary：空/unknown schema 与字段排序。无需 E2E、并发或 fault injection，因为本 Unit 是纯 payload/模块协作边界。

#### 16. 回归范围

直接：`event_loop/rejection.rs` builder tests、`loop_context.rs` missing terminal tests、`hard_gate_payload_contract.rs`、`task_resume_runtime_routing.rs`。相邻：`r5_hard_gate_routing.rs`、`enrich_kind_wiring.rs`。公开消费者：所有 `build_task_resume_payload` callers，必须验证旧字段仍在。旧配置/旧 payload：无新字段时旧解析路径继续通过。构建/lint/typecheck：ralph-core targeted + check/clippy；Unit 关闭前不跳过全量最终门禁。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/rejection.rs` | 修改生产文件+新增单测 | 新增 terminal contract helper，保留旧 API | E5、D3 |
| `crates/ralph-core/src/event_loop/event_processing.rs` | 修改生产文件 | 从 ProtocolView 获取真实字段 | E4、E6 |
| `crates/ralph-core/src/event_loop/tests/loop_context.rs` | 修改测试 | 真实空 channel recovery contract | E4、E12 |
| `crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs` | 仅回归/必要时增测试 | 确认 target 不漂移 | E3、E12 |

#### 18. 完成标准

S4/S7、builder tests、loop_context、task.resume routing、hard gate 相关测试通过；`cargo check -p ralph-core`、`cargo clippy -p ralph-core --all-targets --all-features -- -D warnings` 通过；无 schema/topology 变更；payload 不含 topic-as-field；Evidence/Decision 更新；可独立提交。

#### 19. 停止条件

如果 `ProtocolView` 无法覆盖当前 config、旧 builder caller 需要破坏性签名变更、target 在真实 routing test 中漂移、或 Red 不是 payload 字段错误，停止并更新 D3/Evidence；不得改成手写 preset 字段表。

#### 20. 风险与注意事项

| 风险 | 触发条件 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| 多终态字段被错误合并 | 两个 topic schema 不同但只传 union | 双终态 fixture 精确逐 topic 断言 | 使用 map + primary fields | agent 仍可提交 schema-invalid payload，现有 emit gate 继续兜底 |
| 改 builder 破坏旧 caller | 旧单测 required_fields 断言变化 | 全部 builder caller targeted tests | 保持旧签名，新增 helper | 新字段消费者需文档同步，已由 Unit 1 处理 |

### Unit 3：handoff timeout 先有限恢复、达到 cap 才 blocked

#### 1. Unit 目标

改变 recovery finalizer 的唯一目标行为：第一次同 retry key 的 `handoff_dispatch_timeout` 不再立即 `ForcePlanBlocked`；只有达到现有 `max_repeated_recoveries` cap 才 blocked，成功 terminal/recovered 时永不误 block。

#### 2. 对应需求与 Scenario

- Requirement：R5、R6、R7。
- Scenario：S5、S6、S7。
- Decision：D1、D2、D6。
- Evidence：E1、E2、E3、E7、E8、E12。

#### 3. 外部可观察结果

`work.done → test-stabilizer` 首次 handoff timeout 后，loop 不会直接进入 reporter blocked；targeted task.resume 仍可让 consumer 被选中。重复到配置 cap 后才会有 `plan.blocked`，且 reason 可关联 retry key。

#### 4. 当前行为基线

现有 `handoff_dispatch_timeout_forces_plan_blocked_when_pending` 通过，证明 attempt=3/non-terminal 时会 block；E1 的实际 run 表明第一次/早期 timeout 也被 recovery finalizer 短路。`dispatch_and_handoff` 和 `next_hat` 已证明 targeted resume 路由存在。因此 Unit 3 只修改 finalizer/context cap，不修改 tracker/dispatch route。

#### 5. 输入与输出

- 输入：`RuntimeContext.retry_key_states` 的 retry key、last outcome、attempt_count；StallRecovery handoff timeout envelope；`RuntimeContext.handoff_retry_cap`，由已有 `max_repeated_recoveries` 填充。
- 输出：attempt < cap 时 finalizer 不返回 `ForcePlanBlocked`，让既有 targeted resume action 保持；attempt ≥ cap 时返回单个 `ForcePlanBlocked`；terminal/recovered 时返回空。
- 错误：cap=0 不允许进入有效 config；手工 Default test context 使用 3 作为安全默认，生产 context 使用配置值。
- 状态/副作用：不删除 pending handoff 的既有 tracker 语义；不新增无限 retry；不伪造业务事件。
- 不变量：flapping 和 long nonterminal history 的既有独立保护仍运行；review-chain `retry_cap` detector 仍按其既有 whitelist 运行；blocked reason 含 retry key。

#### 6. 修改位置

- `crates/ralph-core/src/recovery_runtime/mod.rs::RuntimeContext`：新增内部 `handoff_retry_cap` 字段并给 Default 安全默认 3；不增加公开 CLI/config 字段。
- `crates/ralph-core/src/event_loop/prompt_injection.rs::runtime_recovery_context`：从 `self.config.telemetry.runtime_diagnosis.max_repeated_recoveries` 转换并填充 context cap；转换必须饱和/可解释，不能 panic。
- `crates/ralph-core/src/recovery_runtime/finalize_recovery_outcome.rs::handoff_timeout_pending`：只有 `state.attempt_count >= ctx.handoff_retry_cap.max(1)` 且 outcome non-terminal 时返回 true；更新注释和测试名称，移除“任何 timeout 都立即 finalise”的过时说明。
- `crates/ralph-core/src/recovery_runtime/finalize_recovery_outcome.rs` tests：把现有 immediate-force test 改为 cap test，新增 under-cap 和 boundary tests。
- `crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs`：增补或复用 target 为 `test-stabilizer`、trigger 为 `work.done` 的真实 routing assertion。
- `crates/ralph-core/src/event_loop/tests/handoff_dispatch.rs`、`recovery_envelope_u7_u8.rs`：只在既有测试需要表达新 first-time retry contract 时更新断言。

明确不修改：`crates/ralph-core/src/event_loop/worktree_handoff.rs`、`dispatch_and_handoff.rs` 的生产逻辑、`handoff_tracker.rs`、`ralph-cli` 的 activation close、任何 preset topology。

#### 7. 可依赖能力

Unit 1 的 agent contract 和 Unit 2 的 dynamic payload 已通过；既有 handoff timeout targeted resume、EventLoop target fast path、RuntimeDiagnosisConfig cap、RecoveryAction dispatch。

#### 8. 禁止依赖的未来能力

不得实现新的 direct reactivation/fallback API；不得修改 U2 missing-terminal cap；不得等待 agent transcript/audit；不得提前把 `plan.blocked` 变成 runtime 伪造业务终态。

#### 9. 验收测试

- 测试名称：`handoff_dispatch_timeout_does_not_block_before_retry_cap`；`handoff_dispatch_timeout_blocks_at_configured_retry_cap`；`handoff_dispatch_timeout_ignores_terminal_outcome`；`handoff_timeout_targeted_resume_reaches_test_stabilizer`。
- 前置：构造同 retry key 的 StallRecovery envelope、non-terminal state 和 explicit cap=3；routing test 使用真实 EventLoop/EventReader/EventBus。
- 动作：分别以 attempt 1、2、3 调用 finalizer/dispatch；再注入 targeted task.resume 并调用 next_hat。
- 断言：attempt 1/2 没有 ForcePlanBlocked；attempt 3 恰有一个 ForcePlanBlocked；terminal outcome 始终为空 action；target consumer 被选中。
- 副作用/不变量：第一次 timeout 不发 blocked；达到 cap 不产生第二个重复 blocked；retry key/target 不变；不生成业务 event。
- 命令：`cargo nextest run -p ralph-core -- handoff_dispatch_timeout_does_not_block_before_retry_cap`；`cargo nextest run -p ralph-core -- handoff_dispatch_timeout_blocks_at_configured_retry_cap`；`cargo nextest run -p ralph-core -- handoff_timeout_targeted_resume_reaches_test_stabilizer`；`cargo nextest run -p ralph-core -- handoff_dispatch`。

#### 10. Acceptance Red

先把 acceptance test 固定为 attempt=1 + handoff envelope + non-terminal outcome，并断言 actions 不含 `ForcePlanBlocked`；当前基线的 `handoff_timeout_pending` 无条件返回 true，预期会看到 force-block assertion 失败。这是有效 Red，因为现有 baseline test 已确认同一调用链当前会 ForcePlanBlocked。若测试只失败在 context fixture 缺字段或没有执行目标 detector，不算有效 Red。

#### 11. 单元测试拆分

1. under-cap：attempt=1、2，StallRecovery handoff envelope，期望无 ForcePlanBlocked。
2. exact-cap：attempt=3，期望一个 `ForcePlanBlocked`，reason 含 retry key。
3. over-cap：attempt=4 不新增多个 block action，验证调用层的幂等/去重边界。
4. terminal outcome：last_outcome 为 Failed/terminal，任意 attempt 都无 handoff finalizer block。
5. config wiring：生产 `runtime_recovery_context` 使用 `max_repeated_recoveries`；不允许把配置读取 Mock 成固定值。
6. 不允许删除原有 flapping/long-history tests，也不允许用扩大 timeout 让测试通过。

#### 12. Red → Green → Refactor 顺序

`attempt=1 no-block Red` → 增加 RuntimeContext cap 与 finalizer cap predicate → test Green → `attempt=cap block` Red/更新旧 immediate test → cap boundary implementation Green → context wiring test Green → target `test-stabilizer` routing integration Green → Refactor 注释/命名、确保 flapping/long-history 顺序不变 → full recovery/handoff regression。

#### 13. 最小实现范围

必须实现：RuntimeContext cap、生产 config wiring、finalizer under-cap gate、cap/terminal tests、target routing regression。必须保持：existing targeted resume、handoff tracker one escalation、other recovery detectors、missing-terminal cap。明确不实现：new API、new config field、dispatch queue fallback、event audit、业务 event synthesis。

#### 14. 集成验证

联合真实 `RuntimeContext` 构造、recovery dispatch、`RecoveryAction` 应用语义、EventLoop target routing、`test-stabilizer` event_filter/trigger。可以使用现有 in-memory fixtures；不得 Mock 掉 finalizer 或 next_hat。若 integration 证明 target resume 在 finalizer 放行后仍无法激活，停止并重新调查 dispatch 内部证据，不自行改 `dispatch_and_handoff.rs`。

#### 15. 风险驱动测试

必须做 State-Machine/boundary test（attempt cap-1/cap/cap+1），原因是错误阈值会再次导致死路或无限循环；必须做 Fault Injection（handoff timeout）和 recovery envelope regression，原因是 P0 只在 timeout 路径发生；不需要 fuzz/parser test。

#### 16. 回归范围

直接：`finalize_recovery_outcome` 全部 tests、`recovery_runtime` dispatch/retry_cap tests、`handoff_dispatch.rs`、`recovery_envelope_u7_u8.rs`、`task_resume_runtime_routing.rs`。相邻：`loop_context.rs` missing-terminal bounded retry、`r5_hard_gate_routing.rs`、`u1_plan_blocked_reporter_target.rs`。公开调用方：所有 `RuntimeContext` struct literals（编译器和 targeted tests 检查）；旧配置：runtime diagnosis 默认/自定义 max_repeated_recoveries；默认关闭路径：runtime diagnosis disabled 仍不改变 event loop basic behavior。构建/lint/typecheck：ralph-core、ralph-cli、全 workspace。最终必须跑仓库规定全量入口。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/recovery_runtime/mod.rs` | 修改生产文件 | 传递既有 retry cap | E7、E8、D2 |
| `crates/ralph-core/src/event_loop/prompt_injection.rs` | 修改生产文件 | 从 config 填充 context cap | E7、E8 |
| `crates/ralph-core/src/recovery_runtime/finalize_recovery_outcome.rs` | 修改生产文件+测试 | first retry 放行、cap blocked | E1、E2、D1 |
| `crates/ralph-core/src/recovery_runtime/finalize_recovery_outcome.rs` tests | 修改/新增测试 | boundary/fault injection | E12 |
| `crates/ralph-core/src/event_loop/tests/task_resume_runtime_routing.rs` | 修改/新增测试 | test-stabilizer target route | E3、E12 |
| `crates/ralph-core/src/event_loop/tests/handoff_dispatch.rs`、`recovery_envelope_u7_u8.rs` | 仅必要时修改测试 | 保持 timeout/envelope 回归 | E3、E7 |

#### 18. 完成标准

S5–S7 通过；attempt<cap 不 block、attempt=cap block、terminal outcome 不 block、target consumer routing 通过；相关 nextest、check、clippy、build 通过；全量 `./scripts/run-tests.sh` 通过；无新 skip/only、无削弱断言、无 `worktree_handoff.rs` 改动；Evidence/Decision 更新；Unit 可独立提交。

#### 19. 停止条件

如果 config cap 无法从现有 EventLoop context 传递、first-time Red 不是 immediate ForcePlanBlocked、target routing regression 失败、flapping/long-history 与 handoff cap 产生未计划的重复 blocked、或需要修改公开配置/新依赖，必须停止并重新调查；不得由 Executor 临时选择硬编码、direct reactivation 或扩大重试。

#### 20. 风险与注意事项

| 风险 | 触发条件 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| 放行 timeout 后形成循环 | target consumer 连续不激活 | cap boundary、full recovery tests、runtime trace | 使用已有 max_repeated_recoveries；cap 后 blocked | cap 前仍消耗有限迭代 |
| 多 detector 重复 blocked | handoff finalizer 与 retry_cap/flapping 同时命中 | dispatch action count/reason assertions | 保持 detector 顺序与现有去重；只改变 handoff predicate | 多个独立 finding 仍可能同时报告，需保留事实 |
| 把 agent 未 emit 错判为机制成功 | recovery 重新激活但 agent 再次空 channel | activation outcome + missing-terminal retry tests | runtime 不伪造业务 event，最终 cap blocked | 无 agent-output 时仍只能判 unknown |

### Unit 4：补齐终态 target/consumer 契约并验证 reporter 自闭环

#### 1. Unit 目标

让声明了 `required_target_hat` 的终态 topic 同时具备可 lint、可 runtime 执行的 producer/explicit target/consumer 契约；对 `report.done` 固定 `reporter` target，并用真实 EventLoop 场景证明 `align.done → report.done → LOOP_COMPLETE` 自然闭环。

#### 2. 对应需求与 Scenario

- Requirement：R8、R9。
- Scenario：S8、S9，以及 S2 的 executor/fixer 非 report terminal 回归。
- Decision：D7、D8、D9。
- Evidence：E14、E15、E16、E17、E18、E19。

#### 3. 外部可观察结果

`report.done` 缺失 target 或携带 `triggered: executor` 时，事件在进入 main EventBus 前被结构化拒绝，且不会注册错误的 reporter handoff。正确的 `triggered: reporter` 被接受后，reporter 下一次 activation 只发 `LOOP_COMPLETE`；人工补发不再是正常路径。

#### 4. 当前行为基线

当前 `event_reader.rs` 把 `triggered` 转成 target，active-hat selection 优先 target；`acceptance_and_lifecycle.rs` 却按 topic 的 `handoff_index.consumer_of` 注册 consumer。`report.done` schema 只要求 `plan_name/report_path/verdict`，没有 target field；既有 `ce_executor_pipeline` BDD scenario 通过，但 mock response 只能设置 `hat`，不能设置 `triggered`，所以没有覆盖错误 target。先新增 `required_target_hat` fixture、错误 target scenario 和 runtime guard assertion；当前 schema/parser/lint/runtime 预期分别因字段不存在、无 lint、无 guard 而 Red。

#### 5. 输入与输出

- 输入：`EventSchema.required_target_hat`、topic 唯一 producer（hat publishes）、topic 唯一 consumer（hat triggers/HandoffIndex）、`ralph_proto::Event.target`、reporter 的既有 terminal_events。
- 输出：静态 lint finding；runtime `StageReject` reason `terminal_target_mismatch` 或 `terminal_target_contract_invalid`；正确 target 的 accepted event 和正常 handoff registration；BDD accepted sequence。
- 错误：required target 缺失、target 与 required target 不同、required target 与唯一 consumer 不同，均 fail-close；错误 event 不进入 main bus、不注册 handoff。
- 状态/副作用：拒绝写入既有 recovery stream/diagnostic，不推进 accepted workflow；正确事件仍按原 handoff tracker 注册。
- 不变量：没有 `required_target_hat` 的 topic 保持现有 explicit target 语义；`LOOP_COMPLETE` 不新增 required target；producer/consumer 由当前 topology 推导，不在第二份配置中复制。

#### 6. 修改位置

- `crates/ralph-core/src/config/loop_config.rs::EventSchema`：新增计划字段 `required_target_hat: Option<String>`，serde default；只表示该 topic 要求的 explicit target，不改变 payload required_fields。
- `presets/en/ce-executor-pipeline.yml`、`presets/schemas/ce-executor-pipeline.yml`、`presets/en/ce-executor-pipeline-loop.yml`、`presets/schemas/ce-executor-pipeline-loop.yml`：在 `event_policy.schemas.report.done` 同步声明 `required_target_hat: reporter`；不要给 LOOP_COMPLETE 添加 target requirement。
- `crates/ralph-core/src/preset_lint/target_routing.rs`（计划新增）及 `preset_lint/mod.rs`、`finding_id.rs`：新增 generic lint，检查 declared required target 已注册、等于 topic unique consumer；finding IDs 固定为计划新增的 `preset.terminal_target_not_registered`、`preset.terminal_target_consumer_mismatch`。
- `crates/ralph-cli/src/policy_check/unified.rs::check_envelope_triggered` 及其现有 policy/emit 测试：对声明 required target 的 topic 增加缺失/错误 target 的结构化拒绝；无 contract 的 topic 保留当前 target-exists/self-target 语义，不把所有 explicit target 全局禁用。
- `crates/ralph-cli/src/commands/emit/command_impl.rs::maybe_derive_triggered_for_isolated` 及 `commands/emit/tests_integration.rs` 的现有 derivation tests：当 schema 声明 required target 时不再把“省略 target”静默自动补全，令缺失显式 target 到达 policy gate；没有 required target 的普通 isolated handoff 继续沿用按唯一 consumer 自动推导。
- `crates/ralph-core/src/event_loop/stages/terminal_target_guard_stage.rs`（计划新增）及 `stage_pipeline.rs`：新增 emit stage，读取已构造的 per-topic target contract；缺失/错误 target 返回 `terminal_target_mismatch`，不改变没有 contract 的 topic。
- `crates/ralph-core/src/event_loop/flow_wiring.rs`：从完整 `RalphConfig` 和 `HandoffIndex::from_config` 构造 target contract map，并将 stage 加入 default/phase/hat-only 三条真实 pipeline；不通过全局环境变量读取。
- `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs`：保留现有按 topic 的 handoff registration，但由于 guard 已在 committed 前 fail-close，增加 assertion/测试确认 accepted report.done 的 target 与 consumer 相同；不得在这里 post-commit 才拒绝。
- `crates/ralph-core/tests/scenarios.rs::MockResponseYaml`：新增可选 `triggered`，写入 scenario JSONL event；旧字符串和只带 `hat` 的 fixture 默认 None，保持兼容。
- `crates/ralph-core/tests/scenarios/ce_executor_pipeline.yml`：report.done 和 LOOP_COMPLETE mock response 显式带 `triggered: reporter`；新增错误 target/absent event 场景 fixture 或等价独立 scenario，必须仍走 `run_workflow_guard_scenario`。
- `crates/ralph-core/src/preset_lint/tests/`、`event_loop/stage_pipeline/tests.rs`、`event_loop/tests/termination.rs`、`event_loop/tests/isolated_complex_regression.rs`：新增结构化 lint、guard、reporter route assertions。
- `skills/ralph-preset-author/references/{commands,finding-rubric}.md` 与 review 对应文档：增加 target/consumer contract finding 说明；`test_skill_anchors.py` 增加稳定 finding marker。

明确不修改：`worktree_handoff.rs`、`dispatch_and_handoff.rs` 的 target 优先 selection、所有非 contract topic 的 target semantics、业务 topic 名称、`presets/manifest.yml`/`index.json`/zsh completion（名称未变）；不把 `triggered: executor` 的来源未经证实归因给 agent prompt。

#### 7. 可依赖能力

Unit 1 已定义动态终态消费规则；Unit 2 已定义 task.resume fields；Unit 3 已让 timeout recovery 不在首次 handoff 时提前 blocked；当前 EventReader target conversion、HandoffIndex consumer lookup、StagePipeline 和真实 BDD harness 已存在。

#### 8. 禁止依赖的未来能力

不得用 reporter prompt 文本代替 `required_target_hat`；不得只在 `apply_contract_committed_side_effects` 中记录 warning 后继续接受；不得把所有 explicit target 全局禁止；不得新增 `next_terminal_topic` 字段；不得用 stub scenario 或只断言 preset YAML 文本。

#### 9. 验收测试

- 测试名称：`event_schema_required_target_hat_round_trips`；`target_routing_lint_rejects_consumer_mismatch`；现有 CLI policy test 的 required-target missing/mismatch cases；`terminal_target_guard_rejects_wrong_reporter_target`；`terminal_target_guard_accepts_reporter_target`；`test_ce_executor_pipeline_reporter_targeted_completion`。
- 前置：构造 reporter 唯一消费 `report.done` 的 config；schema required target 分别为空、executor、reporter；EventLoop 使用真实 pipeline。
- 动作：运行 preset lint 和 CLI `--policy-check`；将 JSONL `report.done` 分别以 no target、`triggered: executor`、`triggered: reporter` 送入 `process_events_from_jsonl`；运行真实 scenario 的 align/report/complete sequence。
- 断言：前两种不进入 main bus、不注册 handoff，recovery reason 精确；正确 target accepted；scenario accepted 顺序和 `report_path` 一致；LOOP_COMPLETE 由 reporter 下一次 activation 发出。
- 副作用/不变量：错误 target 不触发 reporter timeout；旧无 target 的非 contract topic 仍通过；无人工 LOOP_COMPLETE 注入。
- 命令：`cargo nextest run -p ralph-core -- event_schema_required_target_hat_round_trips`；`cargo nextest run -p ralph-core -- target_routing_lint`；`cargo nextest run -p ralph-cli --bin ralph -- test_maybe_derive_triggered_for_isolated`；`cargo nextest run -p ralph-cli --bin ralph -- u7_check_envelope_triggered`；`cargo nextest run -p ralph-core -- terminal_target_guard`；`cargo nextest run -p ralph-core --test scenarios ce_executor_pipeline`。

#### 10. Acceptance Red

先在既有 `test_maybe_derive_triggered_for_isolated` 增加 required-target topic 的 no-autofill assertion；当前 baseline 会把缺失 target 自动补成唯一 consumer，形成真实 Red。再给 `EventSchema` fixture 增加 `required_target_hat: reporter` 和 wrong-target assertion，运行 `terminal_target_guard_rejects_wrong_reporter_target`；当前 baseline 预期会错误接受或继续进入现有路由。另给 preset lint fixture 声明 target/consumer 不一致，预期 strict lint 当前不产生 finding。若测试只因 harness 不支持 `triggered`、schema 未反序列化或 0 tests run 失败，不算有效 Red。

#### 11. 单元测试拆分

1. `EventSchema` YAML round-trip：读取/序列化 `required_target_hat`，默认 None 保持旧 fixture。
2. target routing lint：registered target、missing target、target≠unique consumer 三态；producer ownership 由现有 publishes lint 覆盖，不复制检查。
3. CLI derivation/policy：required-target topic 不自动补全 missing target；wrong/missing/correct target 三态；non-contract topic 仍自动按唯一 consumer 推导。
4. guard：wrong/missing/correct target；non-contract topic pass；LOOP_COMPLETE pass。
5. pipeline wiring：default、phase-authority、hat-only 三条构造路径都包含 guard；顺序在 schema gate 后、accepted side effect 前。
6. EventLoop integration：错误 report.done 不进入 main EventBus，不注册 `handoff_tracker.pending`；正确 report.done 注册 reporter。
7. Scenario harness：旧 string fixture 仍可解析；`triggered` mapping 写入 JSONL；不 Mock EventLoop 的 target selection。

#### 12. Red → Green → Refactor 顺序

`EventSchema round-trip Red` → 增加 optional field → Green → `target lint Red` → 增加 finding IDs、lint module 和 strict wiring → Green → `CLI required-target no-autofill Red` → 修改 isolated derivation 分支并保留普通 topic 自动推导 → Green → `CLI required-target policy Red` → 扩展 `check_envelope_triggered` 与其真实 emit/policy 测试 → Green → `guard wrong-target Red` → 新增 stage/contract map/wiring → Green → `scenario triggered field Red` → 扩展 harness + valid target fixture → Green → 增加 wrong-target absent assertion → Refactor 统一 contract map 与 finding message → 跑全量 preset/termination/handoff/CLI policy regression。

#### 13. 最小实现范围

必须实现：per-topic required target field、两套 preset/schema 同步、generic lint、runtime pre-main-bus guard、三条 pipeline wiring、scenario triggered support、reporter valid/invalid coverage。必须保持：existing target precedence、HandoffIndex derivation、reporter terminal pair、非 contract topic compatibility。明确不实现：global target ban、producer field duplication、next-terminal schema field、new handoff tracker。

#### 14. 集成验证

真实联合 `RalphConfig` → `HandoffIndex` → StagePipeline → JSONL EventReader conversion → emit gate → EventBus/handoff registration；scenario 使用真实 `run_workflow_guard_scenario`。可以用 in-memory/temp workspace，不能 Mock out StagePipeline 或直接调用 lint 代替 runtime test。必须确认错误 event 在 committed side effect 前被拦截。

#### 15. 风险驱动测试

需要 Contract test：schema target 与 unique consumer 的静态一致性；需要 State/flow test：report.done accepted 后 reporter 再发 LOOP_COMPLETE；需要 Fault Injection：wrong target 不得留下 reporter timeout。需要 Compatibility test：旧 EventSchema 无字段和非 contract explicit target 保持旧行为。无需 fuzz。

#### 16. 回归范围

直接：`preset_lint` 全部 strict tests、`stage_pipeline` order tests、`termination.rs`、`isolated_complex_regression.rs`、`ce_executor_pipeline.yml` scenario、`ralph-cli` 现有 `u7_check_envelope_triggered*` policy tests。相邻：`handoff_dispatch.rs`、`u16_resume_routing.rs`、`payload_types.rs`、`post_terminal_rejection.rs`、所有 EventSchema literal 编译点。旧配置/数据：没有 `required_target_hat` 的 schema、旧 scenario string/list format、非 contract topic target。默认关闭路径：无 event_policy 或 no target contract 的 preset。CLI/build：preset lint、presets parity、全 workspace check/clippy/build、最终 full suite。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/config/loop_config.rs` | 修改生产文件 | EventSchema 增加 optional target contract | E15、D7 |
| `presets/en/ce-executor-pipeline*.yml`、`presets/schemas/ce-executor-pipeline*.yml` | 修改配置/schema | report.done target=reporter | E15 |
| `crates/ralph-core/src/preset_lint/target_routing.rs`、`preset_lint/mod.rs`、`finding_id.rs` | 新增/修改生产 lint | static producer/target/consumer consistency | E15、D8 |
| `crates/ralph-cli/src/commands/emit/command_impl.rs`、`commands/emit/tests_integration.rs` | 修改 CLI derivation/测试 | required-target topic 不被 isolated auto-derive 静默补全；普通 topic 保持旧推导 | E19、D8 |
| `crates/ralph-cli/src/policy_check/unified.rs` 及现有 policy/emit 测试 | 修改 CLI policy/测试 | 在 CLI 入口先拒绝 required-target 缺失或错配 | E19、D8 |
| `crates/ralph-core/src/event_loop/stages/terminal_target_guard_stage.rs`、`stage_pipeline.rs`、`flow_wiring.rs` | 新增/修改生产 runtime | pre-main-bus fail-close | E16、E17、D8 |
| `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs` | 修改生产文件或仅增断言 | accepted handoff invariant | E17 |
| `crates/ralph-core/tests/scenarios.rs`、`tests/scenarios/ce_executor_pipeline.yml` | 修改测试 harness/fixture | real triggered target BDD | E18 |
| `crates/ralph-core/src/preset_lint/tests/`、`event_loop/stage_pipeline/tests.rs`、`event_loop/tests/{termination,isolated_complex_regression}.rs` | 新增/修改测试 | lint/guard/closed-loop coverage | E15、E18 |
| 两套 preset author/review references 与 `test_skill_anchors.py` | 修改文档/anchor test | operator contract sync | E13、D7 |

#### 18. 完成标准

S8/S9 通过；`report.done` valid target accepted、wrong/missing target rejected before main bus；handoff tracker 只注册 reporter；真实 scenario accepted `align.done → report.done → LOOP_COMPLETE`；schema parity、preset lint、termination/handoff regression、check/clippy/build、全量 `./scripts/run-tests.sh` 通过；无新增 topic/config CLI、无 skip/only、Evidence/Decision 更新；Unit 可独立提交。

#### 19. 停止条件

如果 target contract 只能在 post-commit 才校验、StagePipeline 三条构造路径无法共享 contract map、现有非 contract target 用例失败、schema parity 要求新增第二份 SSOT、或 BDD scenario 不能表达 `triggered`，必须停止并重新调查；不得降级为 reporter prompt 检查或 warning-only。

#### 20. 风险与注意事项

| 风险 | 触发条件 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| 误伤合法 target override | 非 contract topic 也被 guard | compatibility tests、现有 target routing tests | guard 只读取 required_target_hat contract | 未声明 contract 的旧 preset 仍需后续显式治理 |
| schema 与 inline preset 漂移 | 只改一份 report.done schema | preset schema parity/lint | 四份 linear/loop 文件同一 Unit 同步 | 新 preset 仍需 author/review 审核 |
| 错误 event 已注册 handoff | guard 放在 commit 后 | integration 断言 tracker.pending 和 bus | guard 作为 emit pipeline stage，先于 accepted side effect | 其他绕过 pipeline 的 legacy 外部调用需保留回归证据 |

## Unit 串行依赖图

```text
Unit 1
  ↓ 已验证动态 agent/operator contract
Unit 2
  ↓ 已验证 task.resume 的 terminal contract 与真实 schema fields
Unit 3
  ↓ 已验证 handoff timeout 的有限恢复与 cap 终止
Unit 4
```

- Unit 2 依赖 Unit 1，因为 payload 中新增字段必须有已确认的 agent-facing 消费规则；不能先产生无人能正确消费的 wire contract。
- Unit 3 依赖 Unit 2，因为 handoff timeout 放行后，target agent 必须能读取正确的终态 fields；否则只能把 timeout 继续推给下一个错误契约。
- Unit 4 依赖 Unit 3，因为它要在已验证的 recovery/retry 语义上，给 accepted terminal event 增加 target/consumer fail-close；先完成 Unit 4 的 target contract 再考虑任何新恢复动作。
- 四个 Unit 严格串行；不得把 Unit 4 的 target guard 提前混入 Unit 1–3，也不得把 Unit 3 的 timeout 修复推迟到 route contract 之后才补测试。

## Verification Contract

### 8. 执行命令清单

| 时机 | 命令 | 验证目的 | 预期结果 | 失败处理 |
|---|---|---|---|---|
| Unit 1 Acceptance Red/Green | `skills/.venv/bin/python -m pytest skills/ralph-preset-review/tests/test_skill_anchors.py -q` | 稳定 operator/data skill anchors | Red 缺 marker；Green 全部通过 | Red 非目标失败则停止修 fixture；不得跳过 |
| Unit 1 contract | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | builtin preset 结构/lint | 通过 | 失败不得进 Unit 2 |
| Unit 1 contract | `cargo nextest run -p ralph-core -- preset_lint` | core preset lint | 通过 | 同上 |
| Unit 1 parity | `cargo nextest run -p ralph-cli --bin ralph -- presets` | embedded preset/manifest parity | 通过 | 解释 instructions-only 变更不应改 manifest |
| Unit 2 Red/Green | `cargo nextest run -p ralph-core -- missing_terminal_resume_exposes_per_topic_required_fields` | 动态 payload fields | 当前 Red 为 topic-as-field；实现后 Green | 非目标 Red 停止 |
| Unit 2 regression | `cargo nextest run -p ralph-core -- build_task_resume_payload_with_terminal_contract` | builder wire contract | 通过 | 不改弱断言 |
| Unit 2 integration | `cargo nextest run -p ralph-core -- test_missing_terminal_emit_recovery` | empty-channel recovery retry | 通过 | 失败不得进 Unit 3 |
| Unit 2 routing | `cargo nextest run -p ralph-core -- jsonl_task_resume_preserves_target_and_activates_original_hat` | target preserve | 通过 | 若 route 失败，停止调查 |
| Unit 3 Red/Green | `cargo nextest run -p ralph-core -- handoff_dispatch_timeout_does_not_block_before_retry_cap` | first timeout policy | Red 必须是 immediate block；Green 无 block | 非目标 Red 停止 |
| Unit 3 boundary | `cargo nextest run -p ralph-core -- handoff_dispatch_timeout_blocks_at_configured_retry_cap` | cap boundary | 恰一个 block | 失败不得宣称完成 |
| Unit 3 integration | `cargo nextest run -p ralph-core -- handoff_timeout_targeted_resume_reaches_test_stabilizer` | target consumer routing | 通过 | 失败重新调查 dispatch，不猜测修改 |
| Unit 3 related | `cargo nextest run -p ralph-core -- handoff_dispatch`、`cargo nextest run -p ralph-core -- recovery_envelope` | handoff/envelope regression | 通过 | 不得跳过 |
| Unit 4 CLI derivation | `cargo nextest run -p ralph-cli --bin ralph -- test_maybe_derive_triggered_for_isolated` | required-target 不被 isolated auto-derive 静默补全；普通 topic 保持旧推导 | Red 必须显示 required-target missing 被补成 consumer；Green 保留普通 topic derivation | 非目标 Red 停止 |
| Unit 4 CLI Red/Green | `cargo nextest run -p ralph-cli --bin ralph -- u7_check_envelope_triggered` | CLI policy wrong/missing/correct required target | Red 必须是 report.done 错误/缺失 target 被接受；Green 只允许 reporter target，既有非 contract missing-target 测试仍通过 | 非目标 Red 停止 |
| Unit 4 Red/Green | `cargo nextest run -p ralph-core -- terminal_target_guard` | runtime wrong/missing/correct target | Red 必须是错误 target 被接受；Green 只允许 reporter target | 非目标 Red 停止 |
| Unit 4 lint | `cargo nextest run -p ralph-core -- target_routing_lint` | schema target 与唯一 consumer | mismatch 有 stable finding | 失败不得进最终门禁 |
| Unit 4 BDD | `cargo nextest run -p ralph-core --test scenarios ce_executor_pipeline` | align→report→complete 真实闭环 | 相关 scenario 全部通过 | 失败不得用 stub 替代 |
| Unit close | `cargo check --workspace` | typecheck | 通过 | 修复后重跑 |
| Unit close | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | lint | 通过 | 不降级 lint |
| Unit close | `cargo build --workspace` | build | 通过 | 不进入下一 Unit |
| 文档/CLI 相关 | `scripts/check-cli-doc-drift.sh` | CLI docs drift | 通过 | 若仅 data prose 仍需按硬规则执行 |
| 最终 | `cargo nextest run -p ralph-core --test scenarios` | 真实 EventLoop BDD | 通过 | 失败即未完成 |
| 最终 | `./scripts/run-tests.sh` | workspace + doctest 按仓库两阶段策略全量回归 | 通过 | 失败必须修复；不得裸跑 ralph-cli cargo test 替代 |

E2E `cargo run -p ralph-e2e -- --mock` 只有在实际变更触及 e2e preset path 时执行；本计划不新增跨服务边界，因此不是强制验收项，不能用它替代 core runtime tests。

### 9. 最终质量门禁

- S1–S9 全部有通过的可执行测试；每个 R1–R9 至少关联一个 Scenario 和测试。
- Unit 1、2、3、4 严格按顺序闭合了 Red → Green → Refactor → Integration → Regression。
- `required_fields` 不再把 terminal topic 当字段；`terminal_required_fields` 与 ProtocolView schema 一致。
- first handoff timeout 不再 immediate blocked；达到既有 cap 才 blocked；成功 terminal 不误 block。
- `report.done` 的 `required_target_hat=reporter` 与唯一 consumer 一致；错误/缺失 target 在 main bus 前拒绝；valid target 的 reporter 自闭环自然产生 LOOP_COMPLETE。
- executor/fixer 与其他 hat 的终态规则没有被 generic data skill 混淆；没有把 `work.done/work.failed` 写成全局规则。
- 既有 target routing、missing-terminal cap、flapping/long-history、review retry cap、旧 task.resume 字段和默认关闭 runtime diagnosis 通过回归。
- preset lint、schema parity、BDD scenarios、nextest、build、check、clippy、CLI drift 全部通过。
- 无新增 `.skip`、`.only`、ignored test、无解释 snapshot/golden 更新、无削弱断言、无新依赖、无 `worktree_handoff.rs` 改动。
- 所有关键 Decision 仍 ≥0.85；没有未处理 BLOCKED；实际 diff 只落在 Unit 预期文件。
- 诊断结论仍区分“runtime P0 已修复”和“agent 更底层根因未知”；不得凭代码全绿把缺失的下游审验 retroactively 判为已运行。

## Definition of Done

### 10. 完成交付条件

计划执行完成时，Coding Agent 必须提供：

1. 四个独立提交边界或等价的四个可审计 Unit diff，并标明每个 Unit 的 Red/GREEN 命令和实际输出。
2. 更新后的 Evidence Ledger：至少补充每个新测试的失败/通过证据、runtime cap wiring 证据、preset/data 文档同步证据。
3. 更新后的 Decision Record：若任何决策置信度下降到 0.85 以下，不能继续交付，必须回到调查。
4. 最终测试命令和结果；不能以局部 targeted tests 代替 `./scripts/run-tests.sh`。
5. 变更范围审计：确认没有改 `worktree_handoff.rs`、没有新增业务 topic/config、没有手工改 `.ralph/` 运行时状态文件。

### 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 四个 Unit 均指定真实入口、Red、最小实现、测试和停止条件 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D9 已选定 finalizer、已有 cap、payload shape、target schema、runtime guard、动态 skill 和不改范围 |
| 所有文件和接口是否有代码库证据 | 是 | E2–E19；新增 helper/字段明确标为计划新增 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1 0.95、D2 0.90、D3 0.91、D4 0.99、D5 0.94、D6 0.93、D7 0.93、D8 0.90、D9 0.92 |
| 是否存在未处理的低置信度假设 | 否 | 更底层 transport/agent 根因明确列为诊断未知且不进入实施依赖 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 动态收尾；U2 payload contract；U3 timeout policy；U4 target/consumer contract |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 有独立 Red、targeted nextest/lint/BDD 和 close 条件 |
| 每个 Unit 是否有真实 Red | 是 | U1 anchor 缺失；U2 topic-as-field；U3 immediate ForcePlanBlocked；U4 target field/lint/guard 缺失，均有当前代码证据 |
| 每个 Unit 是否包含回归范围 | 是 | 每个 Unit 第 16 项列出直接/相邻/旧配置/构建回归 |
| 是否存在未来 Unit 依赖 | 否 | Unit 依赖图只有已完成能力的线性传递，不提前实现未来行为 |
| 是否存在泛化任务描述 | 否 | 所有修改均绑定文件、符号、输入、输出、测试和失败语义 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | S1–S9 在第 5、6 节和 Unit 1–4 中逐项对应 |
| 所有关键决策是否有 Evidence | 是 | D1–D9 均绑定 E IDs |
| 计划是否可以严格串行执行 | 是 | `Unit 1 → Unit 2 → Unit 3 → Unit 4`，每个 Unit 明确 Close 后才能进入下一项 |

本计划不包含生产代码；它只规定已取证的实现边界、测试行为和停止条件。
