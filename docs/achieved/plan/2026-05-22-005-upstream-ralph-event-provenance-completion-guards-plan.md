---
title: "upstream: 在 Ralph 原生层封闭事件 bypass、溯源与 completion 语义"
type: upstream
status: active
date: 2026-05-22
origin:
  - docs/report/nuttx-autoresearch-skill-bug-diagnosis-2026-05-22.md
  - docs/plans/2026-05-22-004-fix-nuttx-autoresearch-runtime-bugs-plan.md
related:
  - docs/report/ralph-upstream-runtime-contract-followups.md
  - docs/plans/2026-05-20-003-upstream-ralph-native-state-machine-pause-plan.md
  - docs/plans/2026-05-21-002-upstream-ralph-terminal-completion-lock-plan.md
  - docs/solutions/architecture-patterns/ralph-loop-state-recovery-sources-2026-05-18.md
---

# upstream: 在 Ralph 原生层封闭事件 bypass、溯源与 completion 语义

## Summary

Universal AutoResearch 可以通过 `safe_emit.py`、contract、runtime audit 和 review skill 修复大部分 NuttX 暴露的问题，但仍有三类问题无法在 Skill 层彻底根除：

1. Agent 或用户可以绕过 `safe_emit.py`，直接调用裸 `ralph event emit` 写入 events JSONL。
2. `ralph event emit` 写入的 simple event 缺少 `hat` / `triggered` provenance，导致主事件日志无法直接回答“谁发布了哪个事件、触发了谁”。
3. Ralph event loop 在首次 completion promise 被接受后，仍缺少强原生语义来拒绝或忽略同一 loop 内后续 terminal / business events，并稳定以 completion reason 终止。

本计划只写 Ralph 上游源码层必须修的最小项。Universal 仓库内的 contract、safe_emit、runtime audit 和 review 同步由 `docs/plans/2026-05-22-004-fix-nuttx-autoresearch-runtime-bugs-plan.md` 处理。

## Problem Frame

NuttX 运行现场说明，仅靠下游 Skill 指令无法确保事件链正确：

- Hat 可以使用错误示例或自行构造命令。
- safe_emit 失败后，Agent 仍可能尝试裸 `ralph emit`、纯文本 payload 或手写 JSONL。
- runtime audit 只能事后发现问题，不能阻止坏事件进入 `.ralph/events*.jsonl`。
- events 主日志缺少发布 Hat 时，审计只能靠 sidecar guard log 做间接推断。
- completion 事件进入 events 后，如果后续仍有业务事件进入同一 loop，Ralph 原生层应尽早拒绝/忽略，而不是把正确性完全交给 prompt 和后验审计。

Universal 可以在推荐路径上加防护，但不能禁止用户或 Agent 调用 Ralph CLI。因此，下面三类能力必须进入 Ralph 原生层：

- **Policy-by-default / guarded emit enforcement**：配置开启 event policy 时，`ralph event emit` 不能轻易绕过 policy。
- **Event provenance**：CLI emit 和 event loop 处理路径应能把当前 Hat / triggered Hat 写入 events。
- **Completion honored state**：同一 loop 内首次 completion 被接受后，后续 terminal/business event 必须被原生处理为 ignored/rejected/diagnostic，而不能继续正常推进。

## Requirements

- R1. Ralph CLI 在 workspace config 开启 `event_loop.event_policy.enabled: true` 时，应提供可配置的默认 policy enforcement，降低裸 `ralph event emit` 绕过 policy 的概率。
- R2. `ralph event emit` 必须支持写入 event provenance：`hat`、`triggered`，以及可选 `source` / `loop_id`。
- R3. Event loop 或 CLI 应能从运行上下文环境变量中自动注入当前 Hat provenance，避免每个 Skill 手动传参。
- R4. Event provenance 必须写入 `.ralph/current-events` 指向的 JSONL 文件，并与 `EventRecord` 现有 rich schema 兼容。
- R5. Event policy 对 duplicate terminal 和 terminal 后 business event 的结果必须在 CLI emit 阶段和 event loop 读取阶段一致。
- R6. 首次 completion promise 被接受后，同一 loop 内后续 terminal event 不应再次触发 Hat 调度；应被忽略或转为 diagnostic。
- R7. 首次 completion promise 被接受后，同一 loop 内后续 business event 不应正常推进业务链；应被拒绝、忽略或转为 diagnostic，具体行为由配置决定，默认保护 completion。
- R8. Completion 后的 termination reason 必须稳定为 completion，而不是因为后续噪声事件或用户 stop 变成 stopped/error。
- R9. 所有新增行为必须默认兼容旧配置；未开启 event policy / completion guard 的旧项目行为不应破坏。
- R10. 上游测试必须覆盖 CLI emit、event loop replay、completion dedupe、provenance serialization 和旧配置兼容。

## Scope Boundaries

### In Scope

- Ralph Rust 源码：
  - `crates/ralph-cli/src/main.rs`
  - `crates/ralph-cli/src/loop_runner.rs`
  - `crates/ralph-core/src/event_policy.rs`
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/event_logger.rs`
  - `crates/ralph-core/src/event_reader.rs`
  - `crates/ralph-core/src/config.rs`
  - `crates/ralph-proto/src/event.rs`（仅当需要扩展 proto event metadata）
- Ralph tests:
  - core unit tests
  - CLI emit tests
  - event loop integration tests
  - compatibility tests
- Ralph docs:
  - event system docs
  - configuration docs
  - CLI emit help text

### Out of Scope

- 不修改 Universal AutoResearch 文件；Universal 适配在 004 计划执行。
- 不实现完整 JSON Schema / field_types 验证；当前只围绕已存在 `EventSchema` 能力。
- 不把 `.ralph/loops.json` 改为历史账本。
- 不设计完整 pause/resume 状态机；unsafe rollback native pause 可继续按已有上游计划独立推进。
- 不改变 events JSONL append-only 属性。
- 不强制所有旧项目默认启用 policy enforcement。

## Ralph Source Evidence

本计划基于本地 Ralph 源码确认以下事实。

### `crates/ralph-cli/src/main.rs`

- `EmitArgs` 当前支持：
  - 位置参数 `topic`
  - 位置参数 `payload`
  - `--json`
  - `--file`
  - `--policy-check`
- `emit_command_with_root()` 当前只有在用户显式传 `--policy-check` 时才加载 config 并调用 `validate_event()`。
- 如果未传 `--policy-check`，CLI 会直接将 event record 追加到 `.ralph/current-events` 或 fallback file。
- 当前 record shape 是：

```json
{
  "topic": "...",
  "payload": "...",
  "ts": "..."
}
```

- wave metadata 可从 env vars 自动写入，但普通 Hat provenance 还没有等价机制。

### `crates/ralph-core/src/event_policy.rs`

- `validate_event()` 已支持：
  - JSON object payload 校验
  - required fields
  - allowed values
  - terminal 后 business event
  - duplicate terminal event
- `PolicyRuntimeState::from_events()` 可从 events JSONL replay terminal state。
- `validate_event()` 本身不负责写 events，也不负责终止 loop；调用方决定如何处理 `PolicyDecision`。

### `crates/ralph-core/src/event_loop/mod.rs`

- event loop 会在 scope enforcement 后、workflow guard 前应用 event policy。
- `RejectWithResume` 会 drop 坏事件并发布 `task.resume`。
- `Hold` 会写 `.ralph/hold-state.json` 并发布 `task.resume`。
- 已有 policy runtime state 可以记录 terminal observed，但 completion 后的“已兑现 completion，不再调度业务链”需要更强约束。

### `crates/ralph-core/src/event_logger.rs`

- `EventRecord` rich schema 已有：
  - `iteration`
  - `hat`
  - `triggered`
  - `wave_id`
  - `wave_index`
  - `wave_total`
- simple agent-written event 缺少字段时会 default：
  - `hat: ""`
  - `triggered: None`
- 因此 Ralph 已有序列化字段基础，问题主要是 CLI emit 和运行上下文没有填值。

### `crates/ralph-cli/src/loop_runner.rs`

- `run_loop_impl()` 写 `.ralph/current-loop-id`。
- fresh run 写 `.ralph/current-events`。
- loop runner 可以作为注入 `RALPH_CURRENT_LOOP_ID`、`RALPH_CURRENT_HAT`、`RALPH_EVENTS_FILE` 等上下文的自然位置。

## Technical Decisions

### D1. policy enforcement 不应破坏旧配置

默认兼容策略：

- 未配置 `event_loop.event_policy`：`ralph event emit` 行为保持现状。
- 配置了 `event_policy.enabled: false`：行为保持现状。
- 配置了 `event_policy.enabled: true`：
  - 新增配置项决定是否要求 CLI emit 默认 policy-check。
  - 未开启 strict 默认时，CLI 可 warning。
  - 开启 strict 后，裸 emit 必须拒绝或要求显式 `--no-policy-check --allow-unsafe` 之类的逃逸参数。

这样可以避免破坏已有 Ralph 用户，同时给 Universal 这类长循环工作流一个原生强保护选项。

### D2. provenance 优先用 CLI 参数 + 环境变量

新增 CLI 参数：

- `ralph event emit <topic> <payload> --hat <hat-id>`
- `ralph event emit <topic> <payload> --triggered <hat-id>`
- 可选：`--source agent|cli|system|safe_emit`

新增环境变量 fallback：

- `RALPH_CURRENT_HAT`
- `RALPH_TRIGGERED_HAT`
- `RALPH_CURRENT_LOOP_ID`
- `RALPH_EVENTS_FILE`（已存在优先级语义可沿用）

优先级：

1. 显式 CLI flag。
2. 环境变量。
3. 空值 / None，保持旧行为。

这样 safe_emit 可以显式传 `--hat strategist`，而 Ralph event loop 也可以在 Hat backend 运行时自动注入 `RALPH_CURRENT_HAT`。

### D3. completion honored state 属于 loop runtime state

Completion 是否已兑现，不应只靠 events replay 的 terminal topic 粗略判断。需要在 event loop runtime state 中保留：

- `completion_honored: bool`
- `completion_topic: Option<String>`
- `completion_event_index: Option<u64>` 或等价位置
- `completion_iteration: Option<u32>`

行为：

- 首次接受 completion promise 后设置 `completion_honored = true`。
- 后续 duplicate terminal 不进入 normal routing。
- 后续 business event 不进入 normal routing。
- 根据配置选择：
  - reject：发布 diagnostic/recovery，不写入业务 bus。
  - ignore：静默忽略并 trace。
  - warn：允许但 warning，仅用于兼容。

### D4. CLI emit 与 event loop policy 口径必须一致

如果 CLI `--policy-check` 会拒绝 duplicate terminal，那么 event loop 读取 events 时也必须拒绝同类事件；反过来也一样。

实现要求：

- CLI 和 event loop 都调用同一个 `validate_event()`。
- 对 completion honored state 的检查如果不在 `event_policy.rs`，也必须有 shared helper，避免 CLI / loop 行为分叉。
- 测试使用同一 JSONL fixture 验证两条路径。

## Proposed Config Surface

### Option A: event_policy 增强字段（推荐）

在 `EventPolicyConfig` 下增加：

```yaml
event_loop:
  event_policy:
    enabled: true
    mode: enforce
    on_violation: reject_with_resume
    require_policy_check_for_cli_emit: true
    unsafe_cli_emit_escape: false
    completion_after_terminal:
      duplicate_terminal: reject
      business_after_completion: reject
      write_diagnostic_event: true
```

优点：

- 与已有 event policy 语义集中。
- Universal 只需要生成一个 policy block。
- CLI 可以读取同一个 config 决定是否允许裸 emit。

缺点：

- `event_policy` 配置会更复杂。
- 需要兼容旧 YAML unknown field 处理。

### Option B: 新增 `event_loop.emit_policy`

```yaml
event_loop:
  emit_policy:
    require_policy_check: true
    require_provenance: true
    allow_unsafe_override: false
```

优点：

- 把 CLI 写入策略和 event schema policy 分开。

缺点：

- 新增第二套 policy 名称，Universal 需要同时生成与校验，漂移面更大。

### Decision

采用 Option A。把 CLI emit enforcement 作为 `event_policy` 的运行入口策略，而不是新增独立 policy。

## Implementation Units

### U1. 扩展 Ralph config schema

**Goal:** 为 policy-aware CLI emit 和 completion 后行为提供配置开关。

**Files:**

- Modify: `crates/ralph-core/src/config.rs`
- Modify: `crates/ralph-core/src/event_policy.rs`
- Modify: docs / config examples
- Test: config deserialization tests

**Fields:**

- `event_policy.require_policy_check_for_cli_emit: bool`，默认 `false`
- `event_policy.allow_unsafe_cli_emit: bool`，默认 `true` for compatibility
- `event_policy.require_emit_provenance: bool`，默认 `false`
- `event_policy.completion_after_terminal.duplicate_terminal: warn|reject|ignore`
- `event_policy.completion_after_terminal.business_after_completion: warn|reject|ignore`
- `event_policy.completion_after_terminal.write_diagnostic_event: bool`

**Steps:**

1. 扩展 `EventPolicyConfig`。
2. 为新增字段提供 `serde(default)`，确保旧配置可解析。
3. 增加 enum 类型而不是裸 string，避免拼写错误。
4. config tests 覆盖：
   - 旧配置无新增字段可解析。
   - 新配置字段可解析。
   - 非法 enum 值失败。

**Acceptance Criteria:**

- 旧 Ralph YAML 解析结果行为不变。
- Universal 未来可生成 strict config，而无需 Ralph CLI flags 全靠 prompt 约束。

### U2. CLI emit 默认 policy enforcement

**Goal:** 配置开启 strict policy 时，裸 `ralph event emit` 不再绕过 policy。

**Files:**

- Modify: `crates/ralph-cli/src/main.rs`
- Test: CLI emit tests

**Behavior:**

- 当 config `event_policy.enabled == true` 且 `require_policy_check_for_cli_emit == true`：
  - 即使用户未传 `--policy-check`，CLI 也必须执行 policy check。
  - 如果 policy 拒绝，返回非零，不写 events。
- 当 config 未要求 strict：
  - 用户传 `--policy-check` 时维持当前行为。
  - 用户未传时维持当前行为，最多 warning（可选）。
- 如果提供 unsafe escape：
  - 需要显式 flag，例如 `--unsafe-no-policy-check`。
  - config `allow_unsafe_cli_emit == false` 时，该 flag 也失败。

**Steps:**

1. 抽取 helper：`should_policy_check_emit(args, config) -> PolicyCheckMode`。
2. `emit_command_with_root()` 读取 config 后决定 policy-check 是否强制。
3. 注意当前逻辑只在 `args.policy_check` 时读取 config；强制模式需要先尝试读取 config。
4. 如果找不到 config：
   - 显式 `--policy-check` 仍按当前行为失败。
   - strict 默认无法判断时应 fail closed 或提示 config missing；实现时按 config 找不到选择 fail closed only when user passed policy flag or workspace has recognizable Ralph config marker。
5. 保持 `RALPH_EVENTS_FILE` / `.ralph/current-events` 优先级不变。

**Acceptance Criteria:**

- strict config + missing required field + no `--policy-check`：CLI 返回非零，不写 events。
- strict config + duplicate terminal + no `--policy-check`：CLI 返回非零，不写 events。
- non-strict config + no `--policy-check`：旧行为不变。
- explicit `--policy-check` 行为保持现有兼容。

### U3. CLI emit provenance flags

**Goal:** 让 events 主日志携带发布 Hat 与触发目标。

**Files:**

- Modify: `crates/ralph-cli/src/main.rs`
- Modify: `crates/ralph-core/src/event_reader.rs`（如需要读取新增字段）
- Modify: docs
- Test: CLI emit serialization tests

**CLI:**

```bash
ralph event emit experiment.planned '{"task_key":"x"}' \
  --json \
  --policy-check \
  --hat strategist \
  --triggered implementer
```

**Record Shape:**

```json
{
  "topic": "experiment.planned",
  "payload": {"task_key":"x"},
  "ts": "2026-05-22T00:00:00Z",
  "hat": "strategist",
  "triggered": "implementer",
  "source": "cli"
}
```

**Steps:**

1. 在 `EmitArgs` 增加 `--hat`、`--triggered`、`--source`。
2. 如果 CLI flag 缺失，读取 env：
   - `RALPH_CURRENT_HAT`
   - `RALPH_TRIGGERED_HAT`
   - `RALPH_EVENT_SOURCE`
3. 如果 `event_policy.require_emit_provenance == true` 且 hat 仍为空：
   - 返回非零，不写 events。
4. `record` JSON 中只在有值时写字段，保持旧 simple schema 兼容。
5. EventReader / EventRecord 已能 default 缺失字段；如 reader 当前不保留 `hat`，则补齐结构字段。

**Acceptance Criteria:**

- `ralph event emit ... --hat strategist --triggered implementer` 写入 events 后可被 `EventHistory` 读出。
- env var fallback 生效。
- strict provenance config 下缺 hat 会失败。
- 旧 event lines 没有 hat 仍可读。

### U4. Hat execution context env injection

**Goal:** Ralph 运行 Hat backend 时自动注入当前 Hat provenance，减少下游 Skill 手动传参。

**Files:**

- Modify: `crates/ralph-cli/src/loop_runner.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: backend invocation code path as needed
- Test: loop runner / backend env tests

**Behavior:**

- 当 event loop 选择某个 Hat 执行时，backend process env 包含：
  - `RALPH_CURRENT_HAT=<hat-id>`
  - `RALPH_CURRENT_LOOP_ID=<loop-id>`
  - `RALPH_EVENTS_FILE=<current-events resolved path>` if already used elsewhere, preserve current semantics
- 如果当前 event 会触发下一个 Hat，可设置：
  - `RALPH_TRIGGERED_HAT=<next-hat-id>`
- 对 coordinator / hatless 模式：
  - 无明确 Hat 时不设置或设置 `ralph` / `hatless`，需保持 docs 清楚。

**Steps:**

1. 找到 backend command spawn 的统一入口。
2. 将 loop context 和 current Hat id 注入 env。
3. 不覆盖用户已显式设置的 env，除非这是 Ralph 保留变量。
4. tests 使用 fake backend 输出 env，确认存在。

**Acceptance Criteria:**

- Hat 运行期间调用 `ralph event emit ...` 不传 `--hat` 也能写 provenance。
- Hatless mode 不崩溃，不写误导性 Hat。
- 多 Hat / isolated mode provenance 正确区分不同 Hat。

### U5. Completion honored state

**Goal:** 首次 completion 被接受后，同一 loop 内后续 terminal/business event 不再正常推进。

**Files:**

- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-core/src/event_policy.rs`
- Modify: `crates/ralph-core/src/loop_state.rs` 或等价 state holder
- Test: event loop completion tests

**Behavior:**

- 首次 accepted completion topic：
  - set `completion_honored = true`
  - record completion metadata
  - schedule graceful termination reason completion
- 后续 duplicate terminal：
  - default reject/ignore by config
  - 不发布到 normal EventBus
  - 可发布 diagnostic event，如 `event.completion.duplicate_ignored`
- 后续 business event：
  - default reject/ignore by config
  - 不触发 Hat routing
  - 可发布 diagnostic event，如 `event.completion.business_after_completion`
- 如果 completion topic 出现在同一 batch 中间：
  - completion 后同 batch 后续 business event 也应受保护。

**Steps:**

1. 在 loop state 中增加 completion honored fields。
2. 在 policy validation 或 post-policy pre-routing 阶段识别 completion topic。
3. 确保 workflow guard / required events 与 completion honored 的交互清楚：
   - completion 被 policy 接受前仍可被 required events / workflow guard 拒绝。
   - completion 被接受后才进入 honored state。
4. termination reason 以首次 honored completion 为准。
5. diagnostics 记录 ignored/rejected event topic、payload preview、reason。

**Acceptance Criteria:**

- events batch: `LOOP_COMPLETE`, `LOOP_COMPLETE` -> 第二个不触发 normal routing。
- events batch: `LOOP_COMPLETE`, `experiment.planned` -> planned 不触发 strategist/implementer链路。
- next iteration 再写 business event -> 不正常推进。
- loop exit reason completion。

### U6. Shared CLI / event loop policy replay tests

**Goal:** 防止 CLI policy-check 和 event loop policy-check 行为分叉。

**Files:**

- Modify/Add: `crates/ralph-core/src/event_policy.rs` tests
- Modify/Add: `crates/ralph-cli` tests
- Modify/Add: event loop integration tests

**Fixtures:**

1. valid chain:

```jsonl
{"topic":"experiment.planned","payload":{"task_key":"a","hypothesis":"h","falsification_condition":"f"}}
{"topic":"LOOP_COMPLETE","payload":{"reason":"done"}}
```

2. duplicate terminal:

```jsonl
{"topic":"LOOP_COMPLETE","payload":{"reason":"done"}}
{"topic":"LOOP_COMPLETE","payload":{"reason":"retry"}}
```

3. business after terminal:

```jsonl
{"topic":"LOOP_COMPLETE","payload":{"reason":"done"}}
{"topic":"experiment.planned","payload":{"task_key":"b","hypothesis":"h","falsification_condition":"f"}}
```

4. missing provenance under strict config:

```jsonl
{"topic":"experiment.planned","payload":{"task_key":"a"}}
```

**Acceptance Criteria:**

- CLI emit and event loop produce same accept/reject classification for each fixture.
- Provenance fields are preserved by reader/history APIs.
- Old simple event fixtures still parse.

### U7. Documentation and migration guidance

**Goal:** 让 downstream Skill 明确该怎么使用 Ralph 原生能力。

**Files:**

- Modify: Ralph event system docs
- Modify: Ralph configuration docs
- Modify: CLI help text if needed

**Docs Must Cover:**

- When `event_policy.require_policy_check_for_cli_emit` is enabled, `ralph event emit` validates by default.
- How to use `--hat` / `--triggered`.
- How environment provenance injection works.
- How completion after terminal is handled.
- How to temporarily bypass policy in non-strict configs.
- Why `.ralph/loops.json` is not history.

**Acceptance Criteria:**

- `ralph event emit --help` mentions `--hat`, `--triggered`, `--policy-check`, unsafe bypass if implemented.
- Configuration docs include strict event policy example.
- Completion semantics are documented as loop-level monotonic behavior.

## Rollout Strategy

### Phase 1: Config + CLI provenance

Implement U1 and U3 first. These are mostly additive and low risk.

Expected result:

- Ralph can write provenance fields.
- Old configs still work.
- Universal can start passing `--hat` without waiting for completion guard.

### Phase 2: Strict CLI policy enforcement

Implement U2 after config fields exist.

Expected result:

- New strict configs can prevent裸 emit bypass.
- Existing users not using strict config are not broken.

### Phase 3: Hat env injection

Implement U4 so provenance does not depend entirely on downstream command examples.

Expected result:

- Hat-authored CLI emits naturally include current Hat.
- Manual CLI can still pass explicit flags.

### Phase 4: Completion honored state

Implement U5 and U6 after policy enforcement path is stable.

Expected result:

- Terminal duplication and terminal-after-business are stopped by Ralph runtime, not only post-run audit.
- Termination reason becomes stable.

### Phase 5: Docs and downstream coordination

Update Ralph docs and then update Universal generator in the 004 plan implementation to consume the new upstream fields.

## Test Plan

### Core Tests

```bash
cargo test -p ralph-core event_policy
cargo test -p ralph-core completion
```

Required scenarios:

- old event_policy config deserializes.
- strict event_policy config deserializes.
- invalid completion action enum fails.
- duplicate terminal rejected/ignored according to config.
- business after completion rejected/ignored according to config.
- provenance fields parse and serialize.

### CLI Tests

```bash
cargo test -p ralph-cli emit
```

Required scenarios:

- strict config + no `--policy-check` still validates.
- strict config + missing required payload field returns non-zero and does not write events.
- non-strict config + no `--policy-check` keeps old behavior.
- explicit unsafe bypass works only when config allows it.
- `--hat` / `--triggered` fields are written.
- env fallback writes `hat`.

### Event Loop Tests

Required scenarios:

- first `LOOP_COMPLETE` schedules completion termination.
- duplicate `LOOP_COMPLETE` does not trigger normal routing.
- `experiment.planned` after completion does not trigger normal routing.
- completion in middle of batch protects rest of batch.
- termination reason remains completion.
- old config without strict completion guard behaves as before or only warns, according to compatibility decision.

### Integration With Universal

After Ralph changes land, Universal 004 implementation should verify:

```bash
python3 tests/run_regression.py
```

And a generated target should be able to run:

```bash
python3 .uar/scripts/safe_emit.py \
  --project-root . \
  --hat strategist \
  --topic experiment.planned \
  --payload '{"task_key":"demo-exp-1","hypothesis":"h","falsification_condition":"f"}' \
  --emit
```

Expected:

- events JSONL has JSON object payload.
- events JSONL includes `hat: strategist`.
- Ralph policy check rejects schema/terminal violations before write.

## Compatibility Risks

| Risk | Severity | Mitigation |
|------|----------|------------|
| Existing users rely on naked `ralph event emit` with string payload | High | Strict behavior only when config enables it; old configs unchanged |
| New provenance fields break simple event readers | Medium | Fields are optional; EventRecord already defaults missing fields |
| Completion guard drops events users expected to inspect | Medium | Emit diagnostic events / traces; make behavior configurable |
| Unsafe bypass becomes too easy and defeats strict mode | Medium | Config can disable unsafe bypass; unsafe flag name must be explicit |
| Hat env injection leaks stale Hat between processes | Medium | Set env only for backend child process; tests assert isolation |
| CLI config lookup fails in unusual workspace | Low | Preserve explicit `--policy-check` failure semantics; strict mode requires discoverable config |

## Migration Notes for Universal AutoResearch

Once this upstream plan is implemented, Universal should update its generator to:

- Generate strict `event_policy.require_policy_check_for_cli_emit: true`.
- Generate `event_policy.require_emit_provenance: true` if Ralph release supports it.
- Keep using `safe_emit.py` for Universal contract checks, but rely on Ralph for native enforcement.
- Pass `--hat` explicitly until env provenance injection is confirmed in the installed Ralph version.
- Treat events missing `hat` as historical/legacy or bypass risk in runtime audit.
- Remove any downstream workaround that writes provenance only to sidecar guard logs as the primary source.

## Acceptance Checklist

- [ ] Ralph config supports strict CLI policy enforcement fields with old config compatibility.
- [ ] `ralph event emit` can no longer bypass policy in strict config.
- [ ] `ralph event emit` supports `--hat` and `--triggered`.
- [ ] Hat backend execution injects current Hat provenance env.
- [ ] Events JSONL preserves provenance fields and old simple events still parse.
- [ ] First completion promise creates loop-level honored state.
- [ ] Duplicate terminal after honored completion does not route normally.
- [ ] Business event after honored completion does not route normally.
- [ ] Completion termination reason remains completion.
- [ ] Core, CLI, and event loop tests cover the new behavior.
- [ ] Ralph docs explain strict emit, provenance, and completion monotonicity.

