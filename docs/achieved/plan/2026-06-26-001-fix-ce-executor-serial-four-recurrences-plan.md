---
title: "fix: ce-executor-serial 四项复发机制缺陷"
type: fix
status: active
date: 2026-06-26
origin: docs/brainstorms/2026-06-21-serial-preset-root-cause-fix-requirements.md
---

# fix: ce-executor-serial 四项复发机制缺陷

## Overview

`ce-executor-serial` 反复出现四类失败，表面看是配置/提示词/计数器问题，实质是四条编排**不变量（invariant）被软性约定替代**：

1. **Hat 作用域不变量** — 每个 hat 只能看到其 `triggers`/`event_filter` 允许的事件，也只能 emit 其 `publishes` 允许的事件。当前 `event_filter` 是提示层补丁，缺少从配置到 prompt 到 emit 的同一答案。
2. **义务履行不变量** — 当 hat 被触发时，它承担在合理时间内产生约定业务事件的义务。当前义务被替换成“激活时钟”，`task.resume` 重试会刷新时钟，义务永远无法到期。
3. **终态语义不变量** — `pass` / `pass_with_residuals` / `fail` 是三种互异的终态，不能退化成字符串 `pass`/`fail` 二值比较。当前二值化导致 shipper 把“带残留通过”误判为失败。
4. **自愈边界不变量** — completion rejection 必须被分类为“可恢复”或“结构性”，可恢复才能走有限 correction，结构性必须立即上报而不是无限 retry。当前 correction block 无分类、无上限、不消费。

本计划用 **7 个串行 Unit** 把这四条约定硬化为机制：2 个类型/模型基础单元 + 4 个机制修复单元 + 1 个全量回归单元。每个 Unit 先写测试再写实现，测试通过方可进入下一 Unit。

---

## Problem Frame

`ce-executor-serial` 的 10-hat isolated 链路依赖严格的 hat 能力边界：

```
coordinator → executor → validator → fixer
    ↓
review-coordinator → dimension-reviewer → review-synthesizer
    ↓
shipper → reporter → progress-steward
```

这条链路的可靠性不应依赖 agent 自觉，而应依赖编排器在四个边界上强制执行不变量：

- **可见性边界**：prompt 中事件集合 = f(hat.scope)。当前 `event_filter` 实现只是“建议”，agent 仍可能通过 shell-write、`ralph emit` 伪 hat、或被注入的 `## RECENT EVENTS` 块看到越界事件。
- **触发义务边界**：hat 被触发后必须在 grace window 内履行 emit 义务。当前用 `hat_activation_at` 兼作“开始时间”和“最后产出时间”，`task.resume` 把义务重新计时，形成无限静默。
- **终态判定边界**：`review.complete.verdict` 与 `REVIEW_COMPLETE.pass_or_fail` 必须有一致的 typed 解释。当前字符串二值化让 shipper、verdict_gate、reporter 各自为政。
- **失败自愈边界**：`LOOP_COMPLETE` 被拒后，编排器必须判断这是“agent 可修正的局部错误”还是“流程结构性错误”。当前无论原因都注入 correction，同一 rejection key 可无限累积。

本次改动的核心不是“加一条 lint”或“改一个 prompt”，而是**为每条边界建立单一事实源和强制机制**。

---

## Requirements Trace

- **R1 — Hat 作用域不变量** — 在 isolated 模式下，任何 hat 的 prompt 中不得出现其 `triggers`/`event_filter` 未允许的事件；任何 hat 不得 emit 其 `publishes`/`topic_deny_rules` 未允许的业务 topic。机制层必须能证明这一点。
- **R2 — 触发义务不变量** — 当 hat 被触发时，编排器记录一项未履行的 emit 义务；义务在 hat emit 约定业务事件后被解除；`task.resume` 只是义务的重新分派，不产生新义务，也不重置义务时钟。
- **R3 — 终态语义不变量** — `Verdict` 必须是三态类型：`Pass`、`PassWithResiduals { count }`、`Fail { reason }`。`PassWithResiduals` 按 `max_residuals` 阈值提升为 `Pass` 或降级为 `Fail`，该判定在 Rust 代码与 shipper/reporter prompt 中完全一致。
- **R4 — 自愈边界不变量** — completion rejection 必须先按 reason 分类：可恢复（如 missing_required_event）才允许有限 correction；结构性（如 verdict_fail / workflow_guard_rejection）必须直接升级为 `CompletionStuck` 或 `human.guidance`，禁止 blind retry。同一可恢复 key 的 correction 次数上限为 `U2_REJECTION_RETRY_LIMIT`（3 次），且 `correction_blocks` 必须 consume-on-use。
- **R5 — 下游同步不变量** — 任何 preset/schema 改动必须同步 BDD scenarios、config opt-in 列表、CLI presets、manifest/index，并跑 preset_lint + SSOT byte-equality + scenarios。

---

## Scope Boundaries

- 本次只硬化上述 4 条不变量，不扩展 ce-executor-serial 维度、不新增 hat、不新增 event topic。
- 不改 isolated execution mode、topic 所有权、终态事件集合。
- 不改 wave supervisor 协议、`ce-executor-isolated` / `ce-executor-wave`。
- `event_filter` 列表已在前期补全；本轮把它从“提示建议”升级为“强制作用域契约”，并补齐 lint/runtime  enforcement。

### Deferred to Follow-Up Work

- 把 hat-scope invariant 推广到所有 builtin preset 的 isolated 模式。
- 把 obligation-based missing-event gate 推广到 wave worker 模型。
- 把 typed `Verdict` 推广到 `plan.complete` / `plan.blocked` / `fix.applied` / `fix.exhausted` 等所有 branch decision。
- 引入跨-run obligation persistence（当前 obligation 仍是内存态，loop 重启后由 `from_events` replay 重建）。

---

## Context & Research

### Relevant Code and Patterns

- `presets/en/ce-executor-serial.yml` — 10-hat preset；`event_filter` 已补全；shipper instructions 在 `:1948-1978`。
- `presets/schemas/ce-executor-serial.yml` — SSOT，含 `verdict_gate`、topic_deny_rules、execution_contracts。
- `crates/ralph-core/src/event_loop/types.rs` — `TerminationReason` 定义。
- `crates/ralph-core/src/config/loop_config.rs` — `VerdictGateConfig`（`:465-483`）、`max_residuals`（`:81-91, 320-331`）。
- `crates/ralph-core/src/event_loop/loop_state.rs` — `hat_activation_at`（`:485`）、`record_hat_activation`（`:1141-1144`）、`U2_REJECTION_RETRY_LIMIT`（`:22`）、rejection digest（`:220, 951-958, 1055-1057`）。
- `crates/ralph-core/src/event_loop/mod.rs` — `inject_completion_correction`（`:1648-1704`）、`prepend_correction_and_resume`（`:4454-4471`）、`verdict_payload_is_fail`（`:1713-1721`）、`check_completion_event`（`:1384-1420`）。
- `crates/ralph-cli/src/loop_runner/hard_gate.rs` — `should_gate_missing_events`（`:55-80`）、`inject_missing_event_hard_gate_guidance_with_triggers`（`:735-920`）。
- `crates/ralph-cli/src/loop_runner/runner.rs` — 调用 hard gate 并注入 `task.resume`（`:4183-4212`）。
- `crates/ralph-cli/src/commands/emit.rs` — CLI `ralph emit` 入口，`:642-684` 处 `PolicyDecision::Block` 处理。
- `crates/ralph-core/src/preset_lint/` — lint 模块目录，`finding_id.rs` 管理 finding 常量。

### Institutional Learnings

- `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` — `review.passed` / `review.complete` / `pass_with_residuals` 漂移的 3 道防线设计（KTD-RTC）。
- `docs/solutions/integration-issues/ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md` — dedup prune > plan-gate trigger；serial 模式严禁给 plan-gate 加 `fix.applied`。
- `docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md` — `ralph-cli` 测试必须走 nextest serial，禁止裸 `cargo test -p ralph-cli`。
- `AGENTS.md` 硬规则：修改 preset/schema 后必须同步 7 处下游；测试入口用 `cargo nextest run`；`CLAUDE.md` 与 `AGENTS.md` 同步。

### External References

- 无外部依赖；所有模式均来自本地代码与历史 solutions。

---

## Key Technical Decisions

1. **类型先行，机制随后** — `Verdict` 与 `HatObligation`/`CompletionStuck` 是新机制的语义基础。先落地类型与单元测试，再在 gate/prompt 中使用，避免后续 Unit 同时改模型和业务逻辑。
2. **单一事实源：hat capability set** — 从 `triggers`/`publishes`/`event_filter` 推导出一个 hat 的 capability set；prompt filter、emit gate、lint 都从这个 set 回答“允许/不允许”，消除三层答案不一致。
3. **义务模型替代纯时钟模型** — MissingEventGate 的核心问题是“hat 是否履行了本次触发义务”，而不是“hat 多久没 emit”。引入 `HatObligation` 记录 `(trigger_event, expected_topics, created_at)`；业务事件 discharge 义务；`task.resume` 只重新分派，不产生新义务。
4. **终态判定单一函数** — `Verdict::resolve(gate_config)` 是 prompt 与 Rust 共享的唯一判定函数，避免 prompt 说 pass、gate 说 fail 的漂移。
5. **Rejection 分类与渐进 escalation** — completion rejection 先分类为 `Recoverable` / `Structural`；Recoverable 才走有限 correction；Structural 直接 `CompletionStuck` 或 `human.guidance`。
6. **Correction block consume-on-use** — 与 `resume_blocks` 一致，渲染后 `std::mem::take` 清空，避免 prompt 无限膨胀。

---

## Open Questions

### Resolved During Planning

- **`event_filter` 已补全，本轮还要改 preset 吗？** 不改 filter 列表本身；改为把 `event_filter` 当作强制作用域契约，并新增 lint/runtime enforcement 与 shipper verdict 语义同步。
- **义务模型是否持久化？** 本轮内存态即可；loop 重启后由 `from_events` replay 重建义务。持久化放在 follow-up。
- **`max_residuals` 默认值是否变化？** 保持 `max_residuals: 8`；只从字符串匹配改为 typed `Verdict` 判定。
- **是否新增 termination reason？** 是，新增 `CompletionStuck { source: StuckSource, retry_key, attempts, last_reason }`，其中 `StuckSource::RejectionDigestExhausted` 用于 correction 耗尽，`StuckSource::StructuralRejection` 用于不可恢复 rejection。

### Deferred to Implementation

- `topic_deny_completeness` lint 的 exempt 配置字段名（如 `exempt_topics` / `exempt_hats`）在实现时与现有 lint 风格对齐。
- `Verdict` 序列化/反序列化是否需要在 `ralph-proto` 新增 proto 类型，还是仅作为 Rust 运行时类型在 `event_loop/types.rs` 定义——实现时根据现有 `TerminationReason` 的序列化路径决定。
- `ShellWriteBypass` 检测的具体启发规则（如事件 topic 出现在 stderr / shell output 中且 `hat` 字段缺失）在实现时与 `event_origin.rs` 的 `RALPH_CONTROL_TOPICS` 对齐。
- `Recoverable` vs `Structural` rejection 的分类映射表在实现时与现有 `RejectionKind` 对齐。

---

## High-Level Technical Design

> *本节用来说明改动形状，不是可复制粘贴的实现规范。*

```text
┌─────────────────────────────────────────────────────────────────────────────┐
│  U1: 类型基础                                                                │
│  Verdict ──► Pass / PassWithResiduals { count } / Fail { reason }           │
│  StuckSource ──► RejectionDigestExhausted / StructuralRejection / ...       │
│  TerminationReason 扩展 CompletionStuck { source, retry_key, ... }          │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U2: Hat 作用域不变量                                                        │
│  capability set = triggers ∪ publishes ∪ event_filter.events                │
│  ├─ lint: topic_deny_completeness（运行前）                                  │
│  ├─ runtime: prompt builder 断言 capability set（运行中）                    │
│  └─ runtime: ShellWriteBypass guard in ralph emit CLI（运行中）              │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U3: 义务模型基础                                                            │
│  LoopState 增加 hat_obligations: VecDeque<HatObligation>                    │
│  HatObligation { hat_id, trigger_topic, expected_topics, created_at }       │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U4: MissingEventGate 义务化                                                 │
│  event_loop 在 hat 被触发时 push obligation                                  │
│  event_loop 在 hat emit 预期业务事件时 discharge obligation                  │
│  hard_gate 检查未履行义务是否超时，而非检查激活时钟                            │
│  task.resume 重新分派现有义务，不创建新义务                                   │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U5: 终态语义机制                                                            │
│  Rust: 所有 verdict 判定走 Verdict::resolve(gate_config)                     │
│  Preset: shipper/reporter prompt 使用同一 resolve 语义描述                   │
│  State machine: review.complete(Verdict) → REVIEW_COMPLETE → LOOP_COMPLETE  │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U6: 分类有界自愈                                                            │
│  RejectionClassifier: Recoverable / Structural                               │
│  Recoverable → inject_completion_correction（最多 3 次同 key）               │
│  Structural → CompletionStuck { source: StructuralRejection, ... }           │
│  prepend_correction_and_resume 渲染后清空 correction_blocks                  │
└─────────────────────────────────────────────────────────────────────────────┘
                                    │
                                    ▼
┌─────────────────────────────────────────────────────────────────────────────┐
│  U7: 全量回归与下游同步                                                      │
│  preset_lint + SSOT byte-equality + BDD scenarios + run-tests.sh            │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## Implementation Units

- [ ] U1. **类型基础：Verdict 枚举与 CompletionStuck 终止原因**

**Goal:** 为终态判定与 completion correction 耗尽提供共享 typed 模型。

**Requirements:** R3, R4

**Dependencies:** 无

**Files:**
- Create: `crates/ralph-core/src/event_loop/verdict.rs`
- Modify: `crates/ralph-core/src/event_loop/types.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（仅 import/重新导出）
- Test: `crates/ralph-core/src/event_loop/tests/verdict.rs`（新建）

**Approach:**
- 在 `event_loop/verdict.rs` 定义 `Verdict`：
  ```rust
  enum Verdict {
      Pass,
      PassWithResiduals { count: u32 },
      Fail { reason: String },
  }
  ```
- 实现 `Verdict::from_payload(payload: &str, verdict_field: &str, count_field: Option<&str>) -> Result<Verdict, VerdictParseError>`，负责解析 JSON payload。
- 实现 `Verdict::resolve(self, max_residuals: Option<u32>) -> Verdict`：把 `PassWithResiduals` 按阈值提升/降级。
- 在 `types.rs` 新增 `StuckSource` 枚举：`RejectionDigestExhausted`、`StructuralRejection`、`MissingEventGate`。
- 在 `TerminationReason` 新增 `CompletionStuck { source: StuckSource, retry_key: String, attempts: u32, last_reason: String }`。

**Execution note:** 纯类型单元，先写单元测试验证 `Verdict` 解析、`resolve`、序列化，再写实现。

**Patterns to follow:**
- 参考 `event_loop/termination.rs` 的 `TerminationReason` 变体与序列化。
- 参考 `ralph_proto` Event payload 解析风格。

**Test scenarios:**
- Happy path: `{"verdict":"pass"}` → `Pass`。
- Happy path: `{"verdict":"pass_with_residuals","final_findings_count":5}` → `resolve(Some(8))` → `Pass`。
- Edge case: `final_findings_count == max_residuals` → `Pass`。
- Edge case: `final_findings_count > max_residuals` → `Fail { reason: "residuals exceed max_residuals" }`。
- Error path: `{"verdict":"fail","reason":"tests broke"}` → `Fail { reason: "tests broke" }`。
- Error path: 缺失 `verdict` 字段 → `VerdictParseError`。
- Edge case: `CompletionStuck` 序列化/反序列化 round-trip。

**Verification:**
- 新建 verdict 单元测试全部通过。
- 现有 `event_loop` 编译不破坏。

---

- [ ] U2. **Hat 作用域不变量：从提示建议到强制契约**

**Goal:** 建立“hat capability set 是 prompt 可见性与 emit 权限的单一事实源”。

**Requirements:** R1, R5

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（prompt builder 断言）
- Modify: `crates/ralph-core/src/preset_lint/mod.rs`
- Create: `crates/ralph-core/src/preset_lint/hat_scope_invariant.rs`
- Modify: `crates/ralph-core/src/preset_lint/finding_id.rs`
- Modify: `crates/ralph-cli/src/commands/emit.rs`
- Modify: `presets/en/ce-executor-serial.yml`（必要时添加 exempt 配置）
- Test: `crates/ralph-core/src/event_loop/tests/event_filter.rs`
- Test: `crates/ralph-core/src/preset_lint/tests.rs`（或内联测试）
- Test: `crates/ralph-cli/tests/emit_command_tests.rs`（或等效 CLI 测试）

**Approach:**
1. **Capability set 推导**：新增 helper `HatConfig::capability_topics() -> HashSet<String>`，合并 `triggers` + `publishes` + `event_filter.events`（去重）。
2. **Prompt 断言**：在 `build_prompt` 的 isolated 模式分支（`:3172-3193`）末尾，对最终 prompt 执行断言：所有被渲染的 `regular_events` 的 topic 必须在当前 hat 的 capability set 内；若发现越界事件，panic/test fail 并记录具体 topic。该断言在 release build 以 `debug_assert!` 或 tracing error 形式存在，避免性能损耗；在测试中以硬断言存在。
3. **Lint 规则 `hat_scope_invariant`**：
   - 检查每个 hat 的 `event_filter.enabled == true`（isolated 模式下强制为 true）。
   - 检查每个 hat 的 `publishes` 中所有 topic 都被 `topic_deny_rules` 覆盖，或出现在显式 `exempt` 列表中。
   - 检查 coordinator 的 `event_filter` 不包含任何 `review.dimension.*` / `review.dimensions.complete` / `review.complete`。
4. **Runtime ShellWriteBypass guard**：在 `ralph-cli/src/commands/emit.rs` 中，当 `hat` 缺失或为 `ralph` 且 topic 非 `RALPH_CONTROL_TOPICS` 时，直接 `exit 2` 并提示“业务事件必须使用注册 workflow hat 发布”。内部 `loop_runner` 调用保留 `RejectWithResume`。

**Execution note:** 先写 event_filter 测试：构造一个包含 coordinator + `review.dimension.done` 的 prompt，断言构建失败或越界事件被移除。再写 lint 测试和 CLI 测试，最后实现机制。

**Patterns to follow:**
- 参考 `preset_lint/review_terminal_coherence.rs` 的模块结构。
- 参考 `event_origin.rs` 的 `RALPH_CONTROL_TOPICS`。
- 参考 `AGENTS.md` 的 preset/schema 同步清单。

**Test scenarios:**
- Happy path: 正常 `ce-executor-serial` preset 通过 hat_scope_invariant lint。
- Error path: 临时把 coordinator 的 `event_filter` 加上 `review.dimension.done` → lint 报 `preset.coordinator_review_chain_leak`。
- Error path: 临时移除 coordinator 对 `review.dimension.ready` 的 topic_deny 规则 → lint 报 `preset.topic_deny_incomplete`。
- Error path: CLI `ralph emit work.ready --payload '{}' --hat ralph` → exit 2，events.jsonl 不增加。
- Error path: CLI `ralph emit work.ready --payload '{}'`（无 hat） → exit 2。
- Happy path: prompt builder 对正常事件流不 panic/test fail。
- Error path: prompt builder 测试传入越界事件 → 断言失败并指明 topic。

**Verification:**
- `cargo nextest run -p ralph-core --test event_filter` 通过。
- `cargo nextest run -p ralph-core -- preset_lint` 通过。
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 通过。
- 新增 CLI emit 测试通过。

---

- [ ] U3. **义务模型基础：HatObligation**

**Goal:** 在 `LoopState` 中建立“触发-履行”义务结构，替代单纯的激活时间戳。

**Requirements:** R2

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/event_loop/loop_state.rs`
- Test: `crates/ralph-core/src/event_loop/tests/loop_state.rs`（若不存在则新建）

**Approach:**
- 在 `loop_state.rs` 新增结构体：
  ```rust
  pub struct HatObligation {
      pub hat_id: HatId,
      pub trigger_topic: String,
      pub expected_topics: Vec<String>,
      pub created_at: Instant,
      pub redispatch_count: u32,
  }
  ```
- 在 `LoopState` 新增 `pub hat_obligations: VecDeque<HatObligation>`。
- 新增 API：
  - `push_hat_obligation(hat_id, trigger_topic, expected_topics)` — 触发时调用。
  - `discharge_hat_obligation(hat_id, emitted_topic) -> bool` — emit 预期业务事件时调用，返回是否成功 discharge。
  - `redispatch_obligation(hat_id)` — `task.resume` 时调用，增加 `redispatch_count`，不更新 `created_at`。
  - `overdue_obligation(hat_id, grace_duration) -> Option<&HatObligation>` — 返回超期未履行的义务。
- 保持 `hat_activation_at` 不变，作为统计/调试用途。

**Execution note:** 纯数据结构单元，先写单元测试验证 push/discharge/redispatch/overdue，再实现。

**Patterns to follow:**
- 与现有 `hat_activation_at` / `pending_recovery_hat` 字段风格一致。
- 使用 `VecDeque` 便于按触发顺序处理。

**Test scenarios:**
- Happy path: push 后 discharge 同一 hat 的预期 topic → obligations 为空。
- Edge case: discharge 非预期 topic → obligation 仍在。
- Edge case: redispatch 后 `created_at` 不变，`redispatch_count` +1。
- Edge case: overdue 在 grace window 内返回 `None`，超时返回 `Some`。
- Edge case: 多个 hat 的义务独立存在，不互相影响。

**Verification:**
- 新增单元测试通过。
- 现有 `loop_state` 测试不破坏。

---

- [ ] U4. **MissingEventGate 义务化**

**Goal:** MissingEventGate 基于未履行的触发义务判定静默，不再因 `task.resume` 重试而重置。

**Requirements:** R2

**Dependencies:** U3

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（触发时 push obligation，emit 时 discharge）
- Modify: `crates/ralph-cli/src/loop_runner/hard_gate.rs`（读 overdue obligation）
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`（必要时调整调用签名）
- Test: `crates/ralph-cli/src/loop_runner/tests/hard_gate_payload_contract.rs`
- Test: `crates/ralph-core/src/event_loop/tests/replay_light_integration.rs`

**Approach:**
- 在 `event_loop/mod.rs` 中，当 hat 被选中执行（收到 trigger 事件）时，调用 `state.push_hat_obligation(hat_id, trigger_topic, hat.terminal_events)`。
- 当某个 hat emit 的业务事件被 accept 时，调用 `state.discharge_hat_obligation(hat_id, topic)`。
- 修改 `hard_gate::should_gate_missing_events`：
  - 首先检查 `state.overdue_obligation(hat_id, grace_duration)`。
  - 若存在超期义务，返回 true（触发 gate）。
  - 若无明确义务，fallback 到 `hat_last_emit_at`（U3 预留字段）或 `hat_activation_at` 的 grace window 检查。
- `inject_missing_event_hard_gate_guidance_with_triggers` 注入 `task.resume` 时，调用 `state.redispatch_obligation(hat_id)`，不创建新义务。

**Execution note:** 先写 hard_gate_payload_contract 测试：模拟 dimension-reviewer 被激活后未 emit，注入一次 `task.resume`，再等待 grace period 仍未 emit，触发 hard escalation；验证 `redispatch_count` 递增但义务未重置。

**Patterns to follow:**
- 参考 `hard_gate.rs:55-80` 的 grace window 计算。
- 参考 `event_loop/mod.rs` 中事件 accept 路径。

**Test scenarios:**
- Happy path: dimension-reviewer 在 grace window 内 emit `review.dimension.done` → obligation discharge，gate 不触发。
- Edge case: dimension-reviewer 首次激活后 5s 无 emit → 仍在 grace window，gate 被抑制。
- Error path: dimension-reviewer 被 `task.resume` 唤醒后，又过了 grace window 仍无业务事件 → gate 触发；再次 `task.resume` 后仍无 → `redispatch_count` 增加，gate 继续触发，最终 hard escalation。
- Error path: `task.resume` 不创建新 obligation，因此不会把静默时间“归零”。
- Happy path: emit 非 terminal 业务事件不 discharge 义务（terminal_events 才是义务目标）。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- hard_gate` 通过。
- `cargo nextest run -p ralph-core --test replay_light_integration -- missing` 通过。

---

- [ ] U5. **终态语义机制：Verdict-aware shipper 与状态机**

**Goal:** 让 Rust verdict gate、shipper prompt、reporter prompt 对 `pass_with_residuals` 有一致的 typed 解释。

**Requirements:** R3

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`verdict_payload_is_fail`、`check_completion_event`）
- Modify: `crates/ralph-core/src/config/loop_config.rs`（`VerdictGateConfig` 增加 `max_residuals`、`verdict_field`、`residual_count_field`）
- Modify: `presets/en/ce-executor-serial.yml`（shipper + reporter instructions 中 verdict 翻译段）
- Test: `crates/ralph-core/src/event_loop/tests/state_machine.rs`
- Test: `crates/ralph-core/src/event_loop/tests/termination.rs`

**Approach:**
- 把所有 verdict 判定收敛到 `Verdict::from_payload(...).resolve(gate.max_residuals)`。
- `verdict_payload_is_fail` 改为返回 `matches!(verdict, Verdict::Fail { .. })`。
- `check_completion_event` 的 `ReviewFailed` 分支使用 typed `Verdict`。
- `VerdictGateConfig` 增加字段：`verdict_field`（默认 `"verdict"`）、`residual_count_field`（默认 `"final_findings_count"`）、`max_residuals`（默认 `None`，表示不提升）。
- 同步 shipper instructions：
  - 读取 `plan.complete.verdict`。
  - 若 `verdict == "pass"` → `REVIEW_COMPLETE.pass_or_fail = "pass"`。
  - 若 `verdict == "pass_with_residuals"` 且 `final_findings_count <= max_residuals` → promote to `"pass"`。
  - 否则 → `"fail"`，并在 `report.done` 中说明原因。
- 同步 reporter instructions：使用同一语义描述 `awaiting_decision` 条件。

**Execution note:** 先写 state_machine 测试让旧逻辑失败（`pass_with_residuals` 被误判为 fail），再改实现使其通过，最后同步 preset prompt。

**Patterns to follow:**
- 参考 `ce-executor-serial-mechanism-close-loop-2026-06-23.md` 中 KTD-RTC 的三道防线。
- 参考 `loop_config.rs:81-91` 的 `max_residuals` 注释语义。

**Test scenarios:**
- Happy path: `REVIEW_COMPLETE(pass_or_fail=pass)` → gate 放行。
- Happy path: upstream `pass_with_residuals` + `count <= max_residuals` → gate 放行。
- Error path: `pass_with_residuals` + `count > max_residuals` → gate 拒绝，reason 为 residuals exceed threshold。
- Error path: `pass_or_fail=fail` → gate 拒绝。
- Integration: `check_completion_event` 在 `review.complete` 阶段即按 typed Verdict 判定。
- Edge case: payload 缺失 `final_findings_count` → `PassWithResiduals { count: 0 }` 或按配置处理。

**Verification:**
- `cargo nextest run -p ralph-core --test state_machine -- verdict` 通过。
- `cargo nextest run -p ralph-core --test termination -- verdict` 通过。
- preset_lint 通过。

---

- [ ] U6. **分类有界自愈：Completion correction 机制**

**Goal:** completion rejection 必须按 reason 分类，可恢复才允许有限 correction，结构性立即 stuck；correction block 消费即用。

**Requirements:** R4

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`inject_completion_correction`、`prepend_correction_and_resume`）
- Create or modify: `crates/ralph-core/src/event_loop/rejection_classifier.rs`（若实现简单可内联在 `mod.rs`）
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`（映射新的 TerminationReason）
- Test: `crates/ralph-core/src/event_loop/tests/state_machine.rs`
- Test: `crates/ralph-core/src/event_loop/tests/termination.rs`

**Approach:**
1. **Rejection 分类器**：根据 rejection reason / stage 分类：
   - `Recoverable`: `missing_required_event`, `engine_rejected:required_field`, `payload_schema_mismatch` 等 agent 可修正的问题。
   - `Structural`: `verdict_fail`, `workflow_guard_rejection`, `plan.blocked` 等上游流程已决定失败的问题。
2. **Recoverable 路径**：
   - 计算 `retry_key`。
   - 若 `rejection_key_is_exhausted` → 返回 `CompletionStuck { source: RejectionDigestExhausted, ... }`。
   - 否则注入 correction block。
3. **Structural 路径**：
   - 直接返回 `CompletionStuck { source: StructuralRejection, retry_key, attempts: 1, last_reason }`。
   - 不注入 correction，不消耗 retry budget。
4. **Drain correction_blocks**：在 `prepend_correction_and_resume` 中，渲染后用 `std::mem::take` 清空 `correction_blocks`。
5. **Runner 映射**：在 `runner.rs` 中把 `CompletionStuck` 记录为结构化日志，exit 非零。

**Execution note:** 先写 state_machine 测试：
- 模拟 `verdict_fail` 拒绝 → 第 1 次即 `CompletionStuck { StructuralRejection }`。
- 模拟 `missing_required_event` 拒绝 3 次 → 前 2 次注入 correction，第 3 次 `CompletionStuck { RejectionDigestExhausted }`。

**Patterns to follow:**
- 参考 `handle_completion_rejection`（`:1599-1632`）的 stale-breaker 逻辑。
- 参考 `loop_state.rs:1055-1057` 的 `rejection_key_is_exhausted`。
- 参考 `prepend_correction_and_resume` 中对 `resume_blocks` 的 `std::mem::take`。

**Test scenarios:**
- Happy path: 第 1 次 `missing_required_event` 被拒 → `correction_blocks` 增加 1 块；渲染后队列清空。
- Happy path: 第 2 次不同 signature 但同 retry_key 被拒 → 注入新 correction；渲染后清空。
- Error path: 第 3 次同 retry_key 被拒 → 不再注入，返回 `CompletionStuck { RejectionDigestExhausted }`。
- Error path: 第 1 次 `verdict_fail` 被拒 → 直接 `CompletionStuck { StructuralRejection }`，不注入 correction。
- Edge case: 3 次 rejection signature 交替变化但 retry_key 相同 → 仍按 retry_key 计数。
- Edge case: `correction_blocks` 渲染后清空，下一轮 prompt 不再重复显示旧 correction。

**Verification:**
- `cargo nextest run -p ralph-core --test state_machine -- correction` 通过。
- `cargo nextest run -p ralph-core --test termination -- completion` 通过。

---

- [ ] U7. **全量回归验证与下游同步**

**Goal:** 确保机制改动不引入回归，preset/schema/scenarios/config 全部一致。

**Requirements:** R5

**Dependencies:** U1, U2, U3, U4, U5, U6

**Files:**
- Modify: `presets/schemas/ce-executor-serial.yml`（如 U2/U5 需要同步 `verdict_gate`、`topic_deny_rules`、exempt 配置）
- Modify: `crates/ralph-cli/src/presets.rs`（如 preset 内容变化导致 embedded 不一致）
- Modify: `crates/ralph-cli/src/preflight.rs`（如新增 config opt-in key）
- Modify: `crates/ralph-cli/src/config_resolution.rs`（如新增 strip key）
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_serial_*.yml`（如 verdict 或事件拓扑变化）
- Modify: `AGENTS.md` / `CLAUDE.md`（如 builtin preset 列表或 hard rule 有变）
- Modify: `scripts/ralph-zsh-plugin.zsh`（如 builtin preset 列表有变）
- Test: 全 workspace 测试入口

**Approach:**
- 检查 U2/U5 是否引入新的 `event_loop.*` 配置字段；若有，同步到 `PRESET_OPT_IN_WHEN_OPERATOR_OMITS` 与 `PRESET_OPT_IN_KEYS`。
- 检查 U2 是否引入新的 lint finding ID；若有，同步到相关测试与文档。
- 检查 U2/U5 是否改变 preset/schema 的 event 拓扑；若有，同步 BDD scenarios。
- 跑全量校验：
  - `cargo build -p ralph-cli`
  - `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
  - `cargo nextest run -p ralph-core -- preset_lint`
  - `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`
  - `cargo nextest run -p ralph-core --test scenarios -- ce_executor_serial`
  - `./scripts/run-tests.sh`

**Execution note:** 纯验证单元，不写新功能代码，只做同步与测试。

**Patterns to follow:**
- 参考 `AGENTS.md`「preset/schema 改动后的下游同步清单」7 步。
- 参考 `.cursor/rules/multi-hat-isolation.mdc` 的 preset 同步规则。

**Test scenarios:**
- Happy path: `preset_lint`（ralph-cli + ralph-core）全部通过。
- Happy path: SSOT byte-equality 测试通过。
- Happy path: 3 个 `ce_executor_serial_*` BDD scenarios 通过。
- Happy path: `./scripts/run-tests.sh` 全绿。

**Verification:**
- `./scripts/run-tests.sh` 返回全部通过。

---

## System-Wide Impact

- **Interaction graph:**
  - `Verdict` 类型被 `verdict_payload_is_fail`、`check_completion_event`、shipper/reporter prompt 共享。
  - `HatObligation` 被 `event_loop/mod.rs` 写入、`hard_gate.rs` 读取。
  - `hat_scope_invariant` lint 被 `preset_lint/mod.rs` 注册、`preset_lint_gate.rs` 自动调用。
  - prompt builder 断言在测试和 debug build 中校验 hat 可见性。
  - `ShellWriteBypass` guard 在 `ralph emit` CLI 路径生效，不影响 loop_runner 内部 publish。
  - `CompletionStuck` 终止原因被 `event_loop/mod.rs` 产生、被 `runner.rs` 日志消费。
- **Error propagation:**
  - `VerdictParseError` 在 gate 内部按“absence = not failing”处理，不向上抛异常。
  - `CompletionStuck` 作为 `TerminationReason` 传到 loop_runner，最终 CLI exit 非零。
- **State lifecycle risks:**
  - `hat_obligations` 是内存态，loop 重启后清空；但 `from_events` replay 会重建触发-emit 关系，不损失关键判定。
  - `correction_blocks` 清空后，若同一问题再次发生，会重新注入新的 correction，不是丢失历史。
- **API surface parity:**
  - 无新增公开 API；`Verdict`、`HatObligation`、`StuckSource` 是内部类型。
  - CLI `ralph emit` 对 `hat=ralph` / 无 hat 业务事件的 exit 行为收紧为 exit 2。
- **Integration coverage:**
  - BDD scenarios 验证 serial 全链路终态语义。
  - CLI emit 测试验证 ShellWriteBypass。
  - hard_gate_payload_contract 测试验证 obligation-based 静默检测。
- **Unchanged invariants:**
  - `event_filter` 列表与行为不变，只是从建议升级为强制契约。
  - isolated execution mode、topic 所有权、终态事件集合不变。
  - `ralph` hat 对 control topics 的 emit 能力不变。
  - `U2_REJECTION_RETRY_LIMIT` 常量值不变。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| `Verdict` 解析改动影响其他 preset 的 verdict gate | 保持字符串 fallback；只在 `verdict` 显式为 `pass_with_residuals` 时走新路径；其他 preset 行为不变。 |
| `HatObligation` 首次执行场景无记录，导致 gate 过早触发 | fallback 到 `hat_last_emit_at` / `hat_activation_at`；首次执行 grace window 与之前一致。 |
| `hat_scope_invariant` lint 对非 serial preset 误报 | 本轮只注册到 `ce-executor-serial` 的 schema/preset 校验；通用化放在 follow-up。 |
| CLI `ralph emit` exit 2 破坏现有脚本 | 业务 topic 本就不该用 `ralph` / 无 hat 发出；在 release note 中说明；必要时后续加隐藏 bypass flag。 |
| `correction_blocks` 清空导致依赖累积提示的 agent 行为变化 | correction 设计本就是 consume-on-use；之前未清空是 bug。 |
| BDD scenarios 迭代数变化 | 本轮不新增/删除事件 topic，只改语义和模型；scenario 迭代数预期不变。 |
| Obligation 模型增加内存/复杂度 | 使用 `VecDeque` 和有限生命周期；每个 hat 同时只保留最新未履行义务。 |

---

## Documentation / Operational Notes

- 在 `docs/solutions/integration-issues/` 下新增一篇 solution 文档，总结 4 条不变量与机制位置（U7 完成后由实现者补充）。
- 若 `AGENTS.md` / `CLAUDE.md` 的 preset 列表或 hard rule 因本次改动变化，必须 `cp CLAUDE.md AGENTS.md` 同步。
- `scripts/ralph-zsh-plugin.zsh` 中 builtin preset 列表未变，无需更新。

---

## Sources & References

- **Origin document:** `docs/brainstorms/2026-06-21-serial-preset-root-cause-fix-requirements.md`
- **Previous related plan:** `docs/plans/2026-06-25-001-feat-ce-executor-serial-5dim-coordinator-amendments-plan.md`
- **Solutions:**
  - `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md`
  - `docs/solutions/integration-issues/ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md`
- **Code references:**
  - `presets/en/ce-executor-serial.yml`
  - `presets/schemas/ce-executor-serial.yml`
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/event_loop/loop_state.rs`
  - `crates/ralph-core/src/event_loop/types.rs`
  - `crates/ralph-core/src/event_loop/verdict.rs`
  - `crates/ralph-core/src/config/loop_config.rs`
  - `crates/ralph-core/src/preset_lint/`
  - `crates/ralph-cli/src/commands/emit.rs`
  - `crates/ralph-cli/src/loop_runner/hard_gate.rs`
  - `crates/ralph-cli/src/loop_runner/runner.rs`
