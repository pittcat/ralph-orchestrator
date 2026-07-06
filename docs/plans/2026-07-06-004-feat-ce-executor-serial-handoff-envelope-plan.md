# feat: ce-executor-serial Handoff Envelope 开关化

type: feat
status: active
created: 2026-07-06
owner: ralph-orchestrator
scope: builtin:ce-executor-serial
source: docs/brainstorms/2026-07-06-ce-executor-serial-handoff-envelope-requirements.md

## 目标

把 `ce-executor-serial` 的 hat 间交接从“松散 payload + prompt 拼接”升级为一个结构化的 Handoff Envelope。它要让接收方 agent 清楚知道：

1. root goal 是什么。
2. 当前 plan / step / task 状态是什么。
3. 上一个 hat 交给我什么。
4. 我现在必须做什么、不能做什么。
5. 我完成后应该发什么 success signal，失败时应该发什么 failure signal。

机制层能力必须默认关闭，只允许在 `builtin:ce-executor-serial` 里实验性开启。其它 preset、普通 emit、policy-check、state projection、wave、supervisor 行为不能被改变。

## 最高执行规则

本计划不是并行 roadmap，必须按下面方式施工。

### 1. 纯粹串行

开发顺序固定为 `Unit 1 -> Unit 2 -> Unit 3 -> ... -> Unit 12`。每次只做一个 Unit。

进入下一个 Unit 的唯一条件是：

1. 当前 Unit 的代码已完成。
2. 当前 Unit 的测试已先红后绿。
3. 当前 Unit 的重构已完成。
4. 当前 Unit 指定的最小测试命令通过。
5. 没有把当前 Unit 的边界问题留给后续 Unit。

禁止同时开发两个 Unit，禁止为了后续 Unit 先塞半成品接口。

### 2. 绝对隔离

每个 Unit 都必须是一个独立小岛。当前 Unit 可以依赖已经完成的前置 Unit 的稳定公开函数或类型，但不能依赖后置 Unit。

具体约束：

1. 当前 Unit 的测试只能验证当前 Unit 的输入输出。
2. 当前 Unit 的测试不能调用后置 Unit 未来才会实现的接口。
3. 当前 Unit 不允许为了后续 Unit 写“先空着以后补”的逻辑。
4. 当前 Unit 需要的数据必须用极简 fake data、内存对象、serde_json literal 或已有稳定 API 构造。
5. 只有到明确的 integration Unit，才允许接入更宽的 runtime 路径。

### 3. 原子 TDD

每个 Unit 都按同一个闭环推进：

1. Red：先写只针对当前 Unit 的失败测试。
2. Green：写最小实现让测试通过。
3. Refactor：只整理当前 Unit 内部，不扩大范围。
4. Verify：跑当前 Unit 指定命令。
5. Stop：提交给下一个 Unit 使用的稳定边界。

如果测试需要依赖“后面某个 Unit 的真实接口”，说明这个 Unit 拆错了，必须重新缩小边界。

## 术语边界

项目里已经有 EventRecord 的事件 envelope，也有 `EmitResult.handoff`。本计划里的 Handoff Envelope 指 payload/prompt 层的 hat 交接对象，不是底层 EventRecord envelope。

命名约束：

1. 使用 `HandoffEnvelopeConfig` 表示配置开关。
2. 使用 `HandoffEnvelopePayload` 表示业务事件 payload 里的 `handoff_envelope` 对象。
3. 使用 `HandoffEnvelopeView` 表示 prompt 注入时渲染用的视图。
4. 使用 `HandoffEnvelopeSummary` 表示 `EmitResult` 里的可选摘要。
5. 不使用 `EventEnvelope` 这类会和底层事件 envelope 混淆的名字。

## 最终 payload 形态

业务事件 payload 最终追加一个顶层字段：

```json
{
  "plan_name": "2026-07-06-example",
  "plan_path": "docs/plans/2026-07-06-example.md",
  "task_id": "task-live-id",
  "task_key": "2026-07-06-example:step-3:implement",
  "step": "step-3",
  "handoff_envelope": {
    "schema_version": "handoff-envelope.v1",
    "root_goal": "implement the requested feature without regressions",
    "plan": {
      "name": "2026-07-06-example",
      "path": "docs/plans/2026-07-06-example.md",
      "current_step": "step-3",
      "completed_steps": ["step-1", "step-2"]
    },
    "state": {
      "current_status": "ready_for_review",
      "last_signal": "work.done",
      "blocking_reason": null
    },
    "receiver_contract": {
      "to_hat": "goal-alignment-reviewer",
      "must_do": ["review goal alignment for the current unit"],
      "must_not_do": ["modify source code"],
      "success_signal": "review.dimension.passed",
      "failure_signal": "review.dimension.failed"
    }
  }
}
```

`event_policy.schemas.required_fields` 最终只要求顶层 `handoff_envelope`。嵌套字段由 Handoff Envelope validator 校验，避免把深层业务契约硬塞进现有顶层 schema 机制。

## 代码事实

本计划基于这些已确认代码事实：

1. `crates/ralph-core/src/config/loop_config.rs` 已有 `MacroEdgeNextHintConfig`，默认关闭，可作为新开关写法参考。
2. `crates/ralph-core/src/event_loop/mod.rs` 的 isolated prompt 构建链已有 `prepend_orchestrator_context`、`prepend_macro_next_hint` 等注入点。
3. `crates/ralph-core/src/runtime_state.rs` 已能渲染 `## ORCHESTRATOR CONTEXT`，包含 plan、current_step、open_tasks、fix/review 状态。
4. `crates/ralph-core/src/emit_result/mod.rs` 已有 `EmitResult` 和轻量 `EmitHandoff`，只能追加 optional 字段，不能改变现有语义。
5. `crates/ralph-cli/src/policy_check.rs` 已有 dry-run validation 到 `EmitResult` 的路径。
6. `presets/schemas/ce-executor-serial.yml` 是 serial event schema 的 SSOT。
7. `docs/solutions/logic-errors/base-runtime-must-not-parse-business-markdown.md` 要求 base runtime 不解析业务 markdown。本计划必须遵守。

## Unit 1: 配置开关骨架

### 目的

先建立一个完全 no-op 的配置入口。这个 Unit 不做 payload、不做 prompt、不做 validator，只证明配置字段存在且默认关闭。

### 输入

极简 YAML 片段：

```yaml
event_loop:
  handoff_envelope:
    enabled: true
    prompt_injection: true
    validate_payload: true
    emit_result_summary: true
```

### 输出

`EventLoopConfig` 中出现 `handoff_envelope: HandoffEnvelopeConfig`，默认值全部为 `false`。

### Red

在 `crates/ralph-core/src/config/loop_config.rs` 附近新增单元测试：

1. `handoff_envelope_defaults_to_disabled`
2. `handoff_envelope_deserializes_explicit_flags`

测试只解析 config，不调用 event loop、不读 preset、不读 schema。

### Green

实现方式：

1. 在 `loop_config.rs` 增加：

   ```rust
   #[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
   pub struct HandoffEnvelopeConfig {
       #[serde(default)]
       pub enabled: bool,
       #[serde(default)]
       pub prompt_injection: bool,
       #[serde(default)]
       pub validate_payload: bool,
       #[serde(default)]
       pub emit_result_summary: bool,
   }
   ```

2. 实现 `Default`，所有字段 false。
3. 在 `EventLoopConfig` 增加字段：

   ```rust
   #[serde(default)]
   pub handoff_envelope: HandoffEnvelopeConfig,
   ```

4. 不修改任何 preset。
5. 不接入任何 runtime 调用点。

### Refactor

只整理 `loop_config.rs` 里的派生 trait、默认值位置和测试命名。不得顺手改 `MacroEdgeNextHintConfig`。

### Verify

```bash
cargo nextest run -p ralph-core -- handoff_envelope_defaults_to_disabled
cargo nextest run -p ralph-core -- handoff_envelope_deserializes_explicit_flags
```

### 完成边界

完成后，整个系统只是多了一个默认关闭的 config 字段。任何运行行为都不能变。

## Unit 2: Payload 纯类型与纯校验

### 目的

定义 `handoff_envelope` JSON 的 Rust 表达和最小校验规则。这个 Unit 不接入 policy-check、不接入 event loop、不修改 preset。

### 输入

`serde_json::Value` fake payload，只包含当前 Unit 需要的 JSON。

### 输出

一个纯函数：

```rust
pub fn validate_handoff_envelope_payload(value: &serde_json::Value) -> Result<HandoffEnvelopePayload, HandoffEnvelopeValidationError>
```

### Red

新增 `crates/ralph-core/src/handoff_envelope.rs`，写只针对 payload 的测试：

1. `valid_payload_deserializes`
2. `missing_handoff_envelope_is_rejected`
3. `wrong_schema_version_is_rejected`
4. `missing_receiver_success_signal_is_rejected`
5. `missing_receiver_failure_signal_is_rejected`

测试直接构造 JSON，不经过 EventPolicy。

### Green

实现方式：

1. 新增 `HandoffEnvelopePayload`，字段对应 JSON 中的 `handoff_envelope`。
2. 新增内部结构：
   - `HandoffEnvelopePlan`
   - `HandoffEnvelopeState`
   - `HandoffEnvelopeReceiverContract`
3. `schema_version` 只接受 `"handoff-envelope.v1"`。
4. `root_goal`、`receiver_contract.to_hat`、`success_signal`、`failure_signal` 必须是非空字符串。
5. `must_do` 至少一项。
6. `must_not_do` 可以为空数组。
7. 错误类型先保持简单：

   ```rust
   pub struct HandoffEnvelopeValidationError {
       pub code: &'static str,
       pub message: String,
   }
   ```

8. 在 `lib.rs` 或相邻 module 声明中只暴露这个 module。

### Refactor

只允许整理该 module 内部的 helper，例如 `required_non_empty_string`。不得把它接入任何外部流程。

### Verify

```bash
cargo nextest run -p ralph-core -- handoff_envelope
```

### 完成边界

完成后，只有纯 JSON 校验能力。它可以独立运行，不依赖 future policy-check wiring。

## Unit 3: Prompt View 纯渲染器

### 目的

把一个已经校验过的 `HandoffEnvelopePayload` 渲染成 prompt 片段。这个 Unit 仍然不接入 event loop。

### 输入

Unit 2 产出的 `HandoffEnvelopePayload` 或手写 fake `HandoffEnvelopeView`。

### 输出

纯函数：

```rust
pub fn render_handoff_envelope_prompt(view: &HandoffEnvelopeView) -> String
```

### Red

在 `handoff_envelope.rs` 写渲染器测试：

1. `renders_handoff_envelope_heading`
2. `renders_root_goal_and_current_step`
3. `renders_receiver_contract_signals`
4. `render_is_stable_for_empty_must_not_do`
5. `render_truncates_long_lists_to_budget`

测试只断言字符串包含关键行，不需要完整快照。

### Green

实现方式：

1. 新增 `HandoffEnvelopeView`。
2. 提供 `impl From<&HandoffEnvelopePayload> for HandoffEnvelopeView`，先只做字段搬运。
3. 渲染固定 heading：

   ```markdown
   ## HANDOFF ENVELOPE
   ```

4. 渲染内容只包含：
   - Root goal
   - Current plan
   - Current step
   - Current state
   - Receiver contract
   - Success signal
   - Failure signal
5. 加一个非常简单的预算策略：列表超过 5 项只显示前 5 项并加 `...`。
6. 不读取 runtime state，不读取 events，不读取 plan markdown。

### Refactor

只整理字符串拼接 helper。不要引入模板引擎。

### Verify

```bash
cargo nextest run -p ralph-core -- render_handoff_envelope
```

### 完成边界

完成后，prompt 片段可以从 fake payload 独立渲染。event loop 仍然不知道它存在。

## Unit 4: Prompt 注入 no-op 接线

### 目的

把 Unit 3 的渲染器接到 isolated prompt 构建链，但必须保持默认 no-op。这个 Unit 只测试开关行为，不测试真实 serial preset。

### 输入

1. 一个关闭的 `EventLoopConfig`。
2. 一个开启的 `EventLoopConfig`。
3. 一个内存 fake `HandoffEnvelopePayload`。

### 输出

一个小函数：

```rust
fn prepend_handoff_envelope_if_enabled(
    prompt: String,
    config: &HandoffEnvelopeConfig,
    envelope: Option<&HandoffEnvelopePayload>,
) -> String
```

### Red

在 `crates/ralph-core/src/event_loop/mod.rs` 或已有 prompt 注入测试文件里写单元测试：

1. `handoff_envelope_prompt_is_noop_when_disabled`
2. `handoff_envelope_prompt_is_noop_when_missing_payload`
3. `handoff_envelope_prompt_is_prepended_when_enabled`

测试只调用这个小函数，不启动完整 EventLoop。

### Green

实现方式：

1. 在 `event_loop/mod.rs` 新增一个小的 private helper。
2. 判断条件必须是：

   ```rust
   config.enabled && config.prompt_injection
   ```

3. 没有 envelope 时直接返回原 prompt。
4. 有 envelope 时调用 Unit 3 的 renderer，并把结果 prepend 到 prompt 前面。
5. 暂时不从真实 ledger 自动推导 envelope。

### Refactor

只调整 helper 的参数顺序和测试可读性。不要接入实际 build_prompt 调用点之外的逻辑。

### Verify

```bash
cargo nextest run -p ralph-core -- handoff_envelope_prompt_is_
```

### 完成边界

完成后，代码里已有可注入能力，但默认关闭，而且没有真实 runtime 数据来源。

## Unit 5: 从最近事件提取 Envelope

### 目的

给 prompt 注入提供数据来源：从当前 hat 能看到的 recent regular events 中，找到最近一个带 `payload.handoff_envelope` 的事件。

这个 Unit 只做“从事件列表提取 payload”，不做调度、不做 policy-check、不改 preset。

### 输入

内存中的 fake event 列表，每个 event 只需要 topic、payload、hat_id 这些已有结构能表达的字段。

### 输出

纯函数：

```rust
fn latest_handoff_envelope_payload(events: &[Event]) -> Option<HandoffEnvelopePayload>
```

如果真实类型不是 `Event`，实现时使用现有 event loop 里 recent events 的实际类型，但测试仍然只构造内存对象。

### Red

写测试：

1. `latest_handoff_envelope_ignores_events_without_payload`
2. `latest_handoff_envelope_uses_most_recent_valid_payload`
3. `latest_handoff_envelope_ignores_invalid_payload`

测试不启动 EventLoop，不读 `.ralph/events.jsonl`。

### Green

实现方式：

1. 从后往前扫描 events。
2. 读取 `event.payload["handoff_envelope"]` 所在完整 payload。
3. 调 Unit 2 的 `validate_handoff_envelope_payload`。
4. 第一个合法 payload 返回。
5. 非法 payload 暂时忽略，不在 prompt 注入阶段报错。真正拦截由后续 validator/policy-check Unit 负责。

### Refactor

只整理扫描逻辑。不要加 warning，不要改 event policy。

### Verify

```bash
cargo nextest run -p ralph-core -- latest_handoff_envelope
```

### 完成边界

完成后，prompt 注入有了自包含数据来源，但还没有接入完整 isolated prompt 链。

## Unit 6: 接入 isolated prompt 构建链

### 目的

把 Unit 4 和 Unit 5 串起来：真实 isolated prompt 构建时，如果开关启用，就从 recent events 取最近 envelope 并注入。

### 输入

已有 event loop prompt builder 的参数和 recent events。

### 输出

serial-like 测试场景中，最终 prompt 包含 `## HANDOFF ENVELOPE`。默认关闭场景中不包含。

### Red

在现有 event loop prompt 测试附近新增测试：

1. `isolated_prompt_omits_handoff_envelope_by_default`
2. `isolated_prompt_includes_handoff_envelope_when_enabled_and_event_has_payload`

测试只构造 prompt builder 所需的最小对象。不要运行完整 loop，不调用 CLI。

### Green

实现方式：

1. 找到 `event_loop/mod.rs` 里 `prepend_macro_next_hint` 附近的 prompt 注入链。
2. 在合适位置调用：

   ```rust
   let envelope = latest_handoff_envelope_payload(&regular_events);
   final_prompt = prepend_handoff_envelope_if_enabled(
       final_prompt,
       &self.config.event_loop.handoff_envelope,
       envelope.as_ref(),
   );
   ```

3. 注入顺序建议放在 `prepend_orchestrator_context` 之后、`prepend_macro_next_hint` 之前，这样 envelope 比单行 next hint 更完整。
4. 不改变 `macro_edge_next_hint` 行为。
5. 不改变 recovery directives、state projection、ready tasks 注入顺序。

### Refactor

如果函数太长，只抽取本 Unit 需要的小 helper。禁止顺手重排整个 prompt 构建链。

### Verify

```bash
cargo nextest run -p ralph-core -- isolated_prompt_omits_handoff_envelope_by_default
cargo nextest run -p ralph-core -- isolated_prompt_includes_handoff_envelope_when_enabled_and_event_has_payload
```

### 完成边界

完成后，机制层 prompt 注入已经可用，但仍然默认关闭，也没有任何 schema required。

## Unit 7: serial preset 只开启 prompt 注入

### 目的

只让 `ce-executor-serial` 打开 Handoff Envelope prompt 注入。这个 Unit 不要求 agent 产出 envelope，也不收紧 schema。

### 输入

`presets/en/ce-executor-serial.yml`。

### 输出

serial effective config 中：

```yaml
event_loop:
  handoff_envelope:
    enabled: true
    prompt_injection: true
    validate_payload: false
    emit_result_summary: false
```

### Red

新增或扩展 preset config 测试：

1. `ce_executor_serial_enables_handoff_envelope_prompt`
2. `non_serial_presets_leave_handoff_envelope_disabled`

测试只检查配置解析结果，不跑完整 workflow。

### Green

实现方式：

1. 修改 `presets/en/ce-executor-serial.yml`，只加 config block。
2. 如 schema SSOT 会覆盖或合并 config，则同步 `presets/schemas/ce-executor-serial.yml` 中对应 config 描述。
3. 不修改 instructions。
4. 不修改 event `required_fields`。

### Refactor

只整理 YAML 位置，放在现有 `event_loop` 配置附近。

### Verify

```bash
cargo nextest run -p ralph-cli --bin ralph -- ce_executor_serial_enables_handoff_envelope_prompt
cargo nextest run -p ralph-cli --bin ralph -- non_serial_presets_leave_handoff_envelope_disabled
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
```

### 完成边界

完成后，只有 serial 会尝试显示已有 envelope。因为还没有 required 和 validator，旧 payload 不会被拒绝。

## Unit 8: policy-check 中接入 payload validator

### 目的

让 `ralph emit --policy-check` 在 `validate_payload: true` 时检查 `payload.handoff_envelope`。这个 Unit 先只做机制，不改 serial preset 打开 validate。

### 输入

一个 policy-check 上下文，config 中手动设置：

```yaml
handoff_envelope:
  enabled: true
  validate_payload: true
```

### 输出

policy-check 对缺失或错误 envelope 返回 reject；开关关闭时不检查。

### Red

写聚焦测试：

1. `policy_check_does_not_require_handoff_envelope_when_disabled`
2. `policy_check_rejects_missing_handoff_envelope_when_validation_enabled`
3. `policy_check_rejects_invalid_handoff_envelope_when_validation_enabled`
4. `policy_check_accepts_valid_handoff_envelope_when_validation_enabled`

测试只走 policy-check validation 函数。不要启动 loop，不读 preset 文件。

### Green

实现方式：

1. 找到 `crates/ralph-cli/src/policy_check.rs` 或 core validation pipeline 中现有 payload validation 的最小接入点。
2. 在已有 topic/schema 校验通过后追加：

   ```rust
   if handoff_config.enabled && handoff_config.validate_payload {
       validate_handoff_envelope_payload(payload)?;
   }
   ```

3. 错误 code 使用稳定字符串，例如：
   - `handoff_envelope_missing`
   - `handoff_envelope_invalid_schema_version`
   - `handoff_envelope_missing_success_signal`
4. 错误进入现有 validation report，不新建第二套 report。
5. 开关关闭时完全绕过。

### Refactor

如果 CLI 直接拿不到 config，不要在本 Unit 大改 config resolution。先把 validator 接到最靠近已有 validation report 的地方，并用测试构造上下文。

### Verify

```bash
cargo nextest run -p ralph-cli --bin ralph -- handoff_envelope
```

### 完成边界

完成后，policy-check 具备可选校验能力，但 serial 还没启用 validate，不会影响真实 serial 跑法。

## Unit 9: EmitResult 可选摘要

### 目的

让 policy-check / emit 返回值能把当前 handoff envelope 的摘要告诉 agent，方便 agent 看到自己发出的交接是否被识别。

### 输入

Unit 2 的 `HandoffEnvelopePayload`。

### 输出

`EmitResult` 增加 optional 字段：

```rust
pub handoff_envelope: Option<HandoffEnvelopeSummary>
```

### Red

在 `crates/ralph-core/src/emit_result/` 下写测试：

1. `emit_result_omits_handoff_envelope_summary_when_disabled_or_absent`
2. `emit_result_includes_handoff_envelope_summary_when_present`
3. `emit_result_rejection_does_not_invent_handoff_envelope_summary`

测试只调用 `EmitResult::assemble` 或相邻纯函数，不跑 CLI。

### Green

实现方式：

1. 在 `emit_result/mod.rs` 增加 `HandoffEnvelopeSummary`。
2. Summary 只包含短字段：
   - `schema_version`
   - `to_hat`
   - `success_signal`
   - `failure_signal`
3. 给 assemble path 增加 optional 参数或新增 helper，选择侵入最小的做法。
4. rejection 时不能凭 payload 硬塞 summary；沿用现有 reject 清理策略，避免给 agent 错觉。

### Refactor

只整理 `emit_result` 内部模块，不改变 `allowed_next`、`handoff`、`target_path` 的语义。

### Verify

```bash
cargo nextest run -p ralph-core -- emit_result
cargo nextest run -p ralph-core -- handoff_envelope
```

### 完成边界

完成后，EmitResult 只多一个 optional 字段。旧调用方不读取它也不会受影响。

## Unit 10: serial 启用 validate + schema required + instructions

### 目的

最后才让 serial 真正要求 agent 写 `handoff_envelope`。这是第一个同时触碰 preset instructions 和 serial schema 的 Unit，所以必须放在所有底层能力之后。

### 输入

已经完成的：

1. Unit 7 的 serial prompt 开关。
2. Unit 8 的 policy-check validator。
3. Unit 9 的 EmitResult summary。

### 输出

`ce-executor-serial` 中关键业务 topic 的 payload required field 包含 `handoff_envelope`，相关 hat instructions 明确如何构造它。

### Red

先写 serial 专属测试：

1. `ce_executor_serial_schema_requires_handoff_envelope_for_work_ready`
2. `ce_executor_serial_schema_requires_handoff_envelope_for_work_done`
3. `ce_executor_serial_policy_check_accepts_valid_handoff_envelope_payload`
4. `ce_executor_serial_policy_check_rejects_missing_handoff_envelope_payload`

这些测试只针对 serial schema/policy-check，不跑完整 multi-hat loop。

### Green

实现方式：

1. 修改 `presets/en/ce-executor-serial.yml`：
   - `validate_payload: true`
   - `emit_result_summary: true`
2. 修改 `presets/schemas/ce-executor-serial.yml`：
   - 对 `work.ready`、`work.done`、`test.passed`、`test.failed`、`review.dimension.ready`、`review.dimension.passed`、`review.dimension.failed`、`fix.ready`、`fix.applied`、`fix.failed` 等关键 topic 增加顶层 required field：`handoff_envelope`
3. 更新 serial 里会 emit 的 hat instructions：
   - 只写该 hat 自己要构造的 envelope。
   - 明确 `receiver_contract.to_hat`。
   - 明确 success/failure signal。
   - 继续强制先 `ralph emit --policy-check`，通过后再真实 emit。
   - 引用 `ralph-tools-emit` / `ralph-tools-tasks`，不复制大段命令文档。
4. 不改其它 preset。
5. 不改 builtin preset 列表，不改 zsh completion，除非实际新增/删除/重命名 preset。

### Refactor

只整理 serial YAML 内部重复结构。不要把 Handoff Envelope 抽成跨 preset 共享宏；这是 serial 实验，不扩散。

### Verify

```bash
cargo nextest run -p ralph-cli --bin ralph -- ce_executor_serial_schema_requires_handoff_envelope
cargo nextest run -p ralph-cli --bin ralph -- ce_executor_serial_policy_check
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
```

### 完成边界

完成后，serial 才真正进入“必须带 Handoff Envelope”的状态。其它 preset 仍然默认关闭。

## Unit 11: agent skill 文档同步

### 目的

把 agent 在 loop 里需要知道的 emit 规则同步到 skill guide，避免 prompt 能力和文档漂移。

### 输入

Unit 10 已稳定的字段、policy-check 行为和 serial instructions。

### 输出

agent 文档能说明：

1. 什么是 `handoff_envelope`。
2. 哪些字段必须写。
3. 必须先 `--policy-check`。
4. 这个能力目前是 serial 实验，不是所有 preset 默认要求。

### Red

文档类 Unit 不写 Rust 失败测试，先做静态检查清单：

1. 搜索 `ralph-tools-emit.md` 中是否已有 `EmitResult` 说明。
2. 搜索 `ralph-tools.md` 中是否已有 isolated 单事件预算和 policy-check 章节。
3. 搜索 preset operator skill references 中是否已有 event_loop config / prompt 注入说明。

### Green

涉及文件：

1. `crates/ralph-core/data/ralph-tools-emit.md`
2. `crates/ralph-core/data/ralph-tools.md`
3. 可选新增 `crates/ralph-core/data/ralph-tools-handoff-envelope.md`
4. `skills/ralph-preset-common/references/agent-native-model.md`
5. `skills/ralph-preset-common/references/author-checklist.md`
6. `skills/ralph-preset-common/references/patterns.md`
7. `skills/ralph-preset-common/references/finding-rubric.md`，仅当新增 lint finding

实现方式：

1. 如果内容短，直接加到 `ralph-tools-emit.md`。
2. 如果内容超过一个屏幕，新增 `ralph-tools-handoff-envelope.md` 并在 `ralph-tools.md` 中说明按需 load。
3. 文档写规则，不写重复的完整 YAML 大段。
4. 若文档含源码行号引用，必须用 `sed -n` 复核行号。

### Refactor

只整理文档结构，不改代码。

### Verify

```bash
./scripts/check-cli-doc-drift.sh
```

如果本 Unit 没有 CLI 行号/命令引用变化，也要在实施记录里说明为何只做静态 doc drift。

### 完成边界

完成后，agent prompt 文档和 serial 行为一致。

## Unit 12: 真 EventLoop 场景回归

### 目的

最后才做集成级验证。前面所有 Unit 都是孤岛测试；这个 Unit 专门证明串起来能跑。

### 输入

已完成的 serial preset、schema、validator、prompt 注入、docs。

### 输出

BDD 或 replay 场景覆盖 serial happy path 和 reject path。

### Red

新增或扩展场景：

1. `ce_executor_serial_handoff_envelope_happy_path.yml`
2. `ce_executor_serial_handoff_envelope_rejects_missing_payload.yml`

对应 `crates/ralph-core/tests/scenarios.rs` 新增测试函数，必须用 `run_workflow_guard_scenario`。

### Green

实现方式：

1. happy path mock responses 中每个关键 emit 都带最小合法 `handoff_envelope`。
2. reject path 故意漏掉 `handoff_envelope`，断言 policy-check 或 workflow guard 拒绝。
3. expected events 断言真实 topic 链路，不只断言 iteration 数。
4. 不使用 `run_scenario` stub。

### Refactor

只整理 scenario fixture，避免复用过度复杂的 mock payload。

### Verify

```bash
cargo nextest run -p ralph-core -- ce_executor_serial_handoff_envelope
```

### 完成边界

完成后，才算完成端到端验证。

## 最终全量验收

所有 Unit 串行完成后，按顺序跑：

```bash
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
./scripts/run-tests.sh
```

如果全量出现竞态/时序类 flake，只允许按项目规则兜底：

```bash
RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh
```

禁止裸跑：

```bash
cargo test -p ralph-cli
cargo test -p ralph-cli --bin ralph
```

## 回归防线

1. Unit 1 保证默认关闭。
2. Unit 4 保证 prompt helper 关闭时 no-op。
3. Unit 6 保证真实 prompt 构建链关闭时无变化。
4. Unit 7 只给 serial 开 prompt 注入，不要求 payload。
5. Unit 8 只提供可选 validator，不启用 serial validate。
6. Unit 10 最后才收紧 serial schema。
7. 每个 Unit 测试只验证当前 Unit，集成测试只放到 Unit 12。
8. runtime 不解析业务 markdown，只消费结构化 payload、state projection、config。

## 完成定义

1. `ce-executor-serial` 关键业务事件 payload 带 `handoff_envelope`。
2. serial activation prompt 能显示稳定 `## HANDOFF ENVELOPE`。
3. `ralph emit --policy-check` 能拒绝缺失或错误的 envelope。
4. 非 serial preset 不受影响。
5. `EmitResult` 可选返回 envelope 摘要。
6. `crates/ralph-core/data/*.md` 与 preset operator skills 已同步。
7. BDD 真 runner 场景覆盖 happy path 和 reject path。
8. 全量 `./scripts/run-tests.sh` 通过。
