---
title: "fix: Isolated hat-handoff review findings (CLI seq, task.resume, context fields)"
type: fix
status: active
date: 2026-06-18
origin: Review of docs/plans/2026-06-18-002-feat-isolated-hat-handoff-plan.md execution
revised: 2026-06-18
---

# fix: Isolated hat-handoff review findings (CLI seq, task.resume, context fields)

## Overview

`docs/plans/2026-06-18-002-feat-isolated-hat-handoff-plan.md` 的机制主体已落地并默认关闭，但对抗性 review 发现 1 个 P0 与 3 个 P1 缺口。本计划修复这些缺口，使 CLI 预检、运行时恢复流、ORCHESTRATOR CONTEXT 与文档四方对齐，确保 Phase 3 开启 `hat_handoff.enabled: true` 前基线可靠。

---

## Adversarial Review of This Plan

对初版计划进行自我对抗性审查后，发现以下设计缺陷并已在本文修正：

1. **初版 U1 让 CLI 完全跳过 seq/iteration 校验**，虽然消除了 P0 误杀，但削弱了 CLI `--policy-check` 与 runtime gate 的语义镜像，也放弃了在 loop 子进程内提前捕获 seq 错误的能力。**修正**：新增 `RALPH_LOOP_ITERATION` / `RALPH_HAT_HANDOFF_SEQ` 环境变量（与现有 `RALPH_WAVE_CONTEXT` 模式一致），CLI 有 env 时严格校验，无 env 时降级为结构/R15/路径安全预检。
2. **初版 U3 只暴露 `hat_handoff_next_seq`**，但 `ralph tools handoff prepare` 需要 `--current-seq`（即已 accept 的 seq），agent 需要做一次 `next_seq - 1` 的心算，易错。**修正**：`## ORCHESTRATOR CONTEXT` 同时暴露 `hat_handoff_seq`（当前已 accept）与 `hat_handoff_next_seq`（下一次应使用），并对应文档示例。
3. **初版 U4 提议直接 `Event::new("task.resume", ...)` 注入**，绕过 `Rejection` retry 预算，可能导致同一 handoff 错误无限重发。**修正**：通过 `Rejection` 系统构造 `task.resume`，使 hat_handoff 拒收进入标准 retry_key 与 recovery envelope 路径。
4. **初版 U6 建议把 `diagnostic.hat_handoff.rejected` 改名为 `event.hat_handoff.rejected`**，这会破坏已合并的 BDD 场景，收益低、风险高。**修正**：保持现有 diagnostic topic 不变，仅作为审计 side-effect；统一工作退化为可选 P2，不阻塞本计划。

---

## Problem Frame

2026-06-18-002 plan 要求：

- CLI `ralph emit --policy-check` 与 runtime gate 共享 reason_code SSOT（U7 / T14）。
- 宏观边 handoff 校验失败时向 emit hat 发 `task.resume`（R4 / KTD-10）。
- `## ORCHESTRATOR CONTEXT` 在 enabled 时追加 `hat_handoff_next_seq` 与 `hat_handoff_dir`（U2）。
- `ralph-tools-handoff.md` 描述 agent 修复路径（U12）。

当前实现存在以下缺口：

1. **P0**：`crates/ralph-cli/src/policy_check.rs` 的 `check_hat_handoff_gate` 硬编码 `iteration: 1, current_seq: 0`，会误杀真实 loop 中迭代/seq 非 1 的合法 handoff emit。
2. **P1**：gate 拒收只 publish `diagnostic.hat_handoff.rejected`，未按契约注入 `task.resume`。
3. **P1**：`RuntimeStateSnapshot` 未实现 `hat_handoff_next_seq` / `hat_handoff_dir`，agent 无法从 `## ORCHESTRATOR CONTEXT` 读取当前 handoff 状态。
4. **P1**：`ralph-tools-handoff.md` 示例使用不存在的 `$RALPH_LOOP_ITERATION` / `$RALPH_HAT_HANDOFF_SEQ` 环境变量。
5. **P1**：gate 已定义 `REASON_CODE_HAT_HANDOFF_FILE_READ_FAIL`，但 event_loop 读盘失败时仍映射为 `file_not_found`。
6. **P1/P2**：T9（policy accept → hat_handoff reject → `HandoffTracker::cancel_pending`）与 CLI seq 路径缺少集成/单元测试；`prepend_hat_handoff` 为死代码。

---

## Requirements Trace

- R1. CLI `--policy-check` 对真实 loop 中的合法 handoff emit 不误杀，同时在已知 loop 状态时仍能校验 seq/iteration（修复 P0）。
- R2. 宏观边 handoff 拒收时必须向 emit hat 注入 `task.resume(reason_code=hat_handoff_*, message=...)`，并进入标准 rejection retry 预算（R4 / KTD-10）。
- R3. `## ORCHESTRATOR CONTEXT` 在 `hat_handoff.enabled` 时输出 `hat_handoff_seq`、`hat_handoff_next_seq` 与 `hat_handoff_dir`（U2）。
- R4. 文档与代码一致：agent 可从 `## ORCHESTRATOR CONTEXT` 或环境变量读取状态，再显式传给 `ralph tools handoff prepare`。
- R5. 文件存在但不可读时返回 `hat_handoff_file_read_fail` reason_code。
- R6. 补齐 T9 与 CLI seq 路径测试；清理死代码。

---

## Scope Boundaries

- 不开启任何 preset 的 `hat_handoff.enabled: true`（仍属 U11 follow-up）。
- 不改动 `HandoffTracker` accept 时机与 SLA 计时起点（KTD-5 不变）。
- 不新增 per-preset 模板或 coordinator 模式 handoff。
- 不修改 macro 边判定规则（自环/豁免/显式 macro_topics 保持现状）。

### Deferred to Follow-Up Work

- U11（preset 开启 `enabled: true` 与 schema 文档更新）仍为独立 follow-up，需本计划全部测试绿后再执行。
- 诊断事件 topic 统一（`diagnostic.hat_handoff.rejected` vs `event.hat_handoff.inject_failed`）作为可选 P2，不在本计划阻塞路径内。

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-cli/src/loop_runner/runner.rs:2888-2906` — `inject_hat_execution_env` 与 `RALPH_WAVE_CONTEXT` 注入点，新 env var 应放在相邻位置。
- `crates/ralph-cli/src/policy_check.rs` — CLI `check_hat_handoff_gate` 镜像点；当前硬编码 `iteration`/`current_seq`。
- `crates/ralph-core/src/event_loop/mod.rs:7767-7858` — 运行时 hat_handoff gate；当前只发 diagnostic。
- `crates/ralph-core/src/event_loop/rejection.rs` — `Rejection` / `build_task_resume_payload` / `resolve_target_hat` / `RejectionStage`，用于构造带 retry 预算的 `task.resume`。
- `crates/ralph-core/src/runtime_state.rs` — `RuntimeStateSnapshot` / `## ORCHESTRATOR CONTEXT` 渲染点。
- `crates/ralph-core/src/hat_handoff/gate.rs` — `evaluate_event` 纯函数与 reason_code 常量。
- `crates/ralph-core/data/ralph-tools-handoff.md` — agent 深参考文档。
- `crates/ralph-core/tests/scenarios.rs` — BDD harness，已有 `prompt_contains` 与 `fixture_files` 扩展。

### Institutional Learnings

- `docs/plans/2026-06-14-003-feat-wave-context-env-var-plan.md` 先例：`RALPH_WAVE_CONTEXT` 通过 runner 注入 backend env。本计划遵循同一模式，新增 `RALPH_LOOP_ITERATION` / `RALPH_HAT_HANDOFF_SEQ`，使 CLI 在 loop 子进程内可获得当前 handoff 状态。

---

## Key Technical Decisions

- **KTD-1**: runner 在调用 hat backend 前注入 `RALPH_LOOP_ITERATION` 与 `RALPH_HAT_HANDOFF_SEQ` 环境变量，与 `RALPH_WAVE_CONTEXT` 注入点相邻。CLI `prepare` / `emit --policy-check` 优先读取这些 env；缺失时 CLI `emit --policy-check` 降级为不校验 seq/iteration，避免误杀。
- **KTD-2**: `## ORCHESTRATOR CONTEXT` 暴露 `hat_handoff_seq`（已 accept 的 seq）与 `hat_handoff_next_seq`（= seq + 1）以及 `hat_handoff_dir`。agent 可直接用 `hat_handoff_seq` 作为 `--current-seq`，无需心算。
- **KTD-3**: gate 拒收通过 `Rejection` 系统注入 `task.resume`，stage 新增 `HatHandoff`，retry_eligible=true，target_hat=from_hat。这样拒收进入标准 retry_key 与 recovery envelope，避免无限重发。
- **KTD-4**: `evaluate_event` 的 file-content 参数从 `Option<&str>` 改为可区分「文件不存在」与「存在但不可读」的类型，使 `file_read_fail` reason_code 真正可用。CLI 无读盘失败场景时传 `Ok(Some/None)` 即可。

---

## Implementation Units

- [ ] U1. **Export handoff state env vars from loop runner**

**Goal:** 让 loop 内的 agent 和 CLI 预检都能拿到当前 iteration 与 handoff seq。

**Requirements:** R1, R4

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`
- Test: `crates/ralph-cli/src/loop_runner/runner.rs` 或新增集成测试

**Approach:**
- 在 `inject_hat_execution_env` 调用之后、backend 执行之前，向 `effective_backend.env_vars` push：
  - `RALPH_LOOP_ITERATION` = `event_loop.state().iteration.to_string()`
  - `RALPH_HAT_HANDOFF_SEQ` = `event_loop.state().hat_handoff_seq.to_string()`
- 仅当 `config.event_loop.hat_handoff.enabled && execution_mode == Isolated` 时注入，coordinator / disabled 不污染 env。
- 注释说明用途，与 `RALPH_WAVE_CONTEXT` 注释风格一致。

**Patterns to follow:**
- 参考 `RALPH_WAVE_CONTEXT` 注入位置与条件判断（`crates/ralph-cli/src/loop_runner/runner.rs:2902-2906`）。

**Test scenarios:**
- Integration: isolated + enabled 时，backend env vars 包含 `RALPH_LOOP_ITERATION` 与 `RALPH_HAT_HANDOFF_SEQ`。
- Edge case: coordinator 模式或 disabled 时，env vars 不存在。
- Edge case: iteration 变化后（`process_output` 已重置 seq），env vars 反映新值。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- loop_runner` 绿（或对应测试子集）。

---

- [ ] U2. **Fix CLI `check_hat_handoff_gate` seq validation**

**Goal:** 消除 P0 误杀，同时保留已知 loop 状态时的 seq/iteration 校验。

**Requirements:** R1

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-cli/src/policy_check.rs`
- Test: `crates/ralph-cli/src/policy_check.rs` (现有 `hat_handoff_tests` 模块)

**Approach:**
- 在 `check_hat_handoff_gate` 中读取 `RALPH_LOOP_ITERATION` 和 `RALPH_HAT_HANDOFF_SEQ` 环境变量。
- 若两个 env 都存在且可解析为 u32，则按真实值传给 `GateInputs`；否则将 `iteration`/`current_seq` 置为 `0` 或引入 `skip_seq_check` 语义，使 gate 跳过 seq/iteration 文件名校验（仍保留路径安全、结构、R15 校验）。
- 保持 reason_code SSOT 不变。

**Patterns to follow:**
- `evaluate_event` 纯函数支持“CLI 模式”与“runtime 模式”两种校验级别；最小化改动，不改变 runtime gate 行为。

**Test scenarios:**
- Happy path (env present): `RALPH_LOOP_ITERATION=3`, `RALPH_HAT_HANDOFF_SEQ=1`，`handoff_path: ".ralph/agent/hat-handoff/3-2-executor-review_coordinator.md"` 通过预检。
- Happy path (env absent): `handoff_path: ".ralph/agent/hat-handoff/3-2-executor-review_coordinator.md"` 通过预检（不校验 seq）。
- Error path (env present, seq mismatch): `RALPH_HAT_HANDOFF_SEQ=1` 但 path seq=3，报 `hat_handoff_filename_mismatch`。
- Edge case: 绝对路径 / `..` 仍被 `path_escape` 拒绝。
- Error path: 缺失 `handoff_path` 的宏观边仍报 `hat_handoff_missing_path`。
- Error path: R15 非法 topic 仍报 `hat_handoff_illegal_emit_topic`。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- hat_handoff` 绿。

---

- [ ] U3. **Expose handoff state in `## ORCHESTRATOR CONTEXT`**

**Goal:** 实现 U2 要求，让 agent 在 prompt 内即可读取当前/下一次 handoff seq 与目录。

**Requirements:** R3

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/runtime_state.rs`
- Test: `crates/ralph-core/src/runtime_state.rs`

**Approach:**
- 在 `RuntimeStateSnapshot` 新增：
  - `hat_handoff_seq: Option<u32>`（已 accept 的 seq）
  - `hat_handoff_next_seq: Option<u32>`（= seq + 1）
  - `hat_handoff_dir: Option<String>`
- `RuntimeStateSnapshot::build` 接收 handoff 状态；enabled 时输出三个字段，disabled 时为 `None`。
- `to_prompt_block` 在 enabled 时追加三行：`- hat_handoff_seq: N`、`- hat_handoff_next_seq: N+1`、`- hat_handoff_dir: <path>`。
- `disabled_stub` 与 `snapshot_from_disk` 保持 `None`。

**Patterns to follow:**
- 与 `wave` 字段类似的条件输出模式；保持块内格式一致。

**Test scenarios:**
- Happy path: enabled + current_seq=1 时，`to_prompt_block` 包含 `hat_handoff_seq: 1`、`hat_handoff_next_seq: 2` 与 `hat_handoff_dir: .ralph/agent/hat-handoff`。
- Edge case: disabled 时，块内不出现 handoff 字段。
- Edge case: `snapshot_from_disk` / `disabled_stub` 字段为 `None`。

**Verification:**
- `cargo nextest run -p ralph-core --lib runtime_state` 绿。

---

- [ ] U4. **Update `ralph-tools-handoff.md` to reference context and env vars**

**Goal:** 消除文档与实现不一致，agent 知道如何拿到当前 iteration/seq。

**Requirements:** R4

**Dependencies:** U1, U3

**Files:**
- Modify: `crates/ralph-core/data/ralph-tools-handoff.md`

**Approach:**
- 把 §5.5.2 `ralph tools handoff prepare` 示例改为两种等效方式：
  1. 从 `## ORCHESTRATOR CONTEXT` 读取 `hat_handoff_seq` 和 `hat_handoff_dir`；
  2. 在 loop 子进程内使用环境变量 `$RALPH_LOOP_ITERATION` / `$RALPH_HAT_HANDOFF_SEQ`。
- 示例命令：
  ```bash
  ralph tools handoff prepare \
    --from executor --to review-coordinator --topic work.done \
    --iteration <ORCHESTRATOR CONTEXT 中的 iteration> \
    --current-seq <ORCHESTRATOR CONTEXT 中的 hat_handoff_seq>
  ```
- 在 §5.5.3 reason_code 表中新增/修正 `hat_handoff_file_read_fail` 说明。

**Test scenarios:**
- Test expectation: none — 纯文档变更；执行 `ralph tools handoff prepare --help` 做冒烟验证。

**Verification:**
- 文档中不再出现未实现的 env var。
- `ralph tools handoff prepare --help` 输出与参数表一致。

---

- [ ] U5. **Inject `task.resume` on hat_handoff gate reject via Rejection system**

**Goal:** 实现 R4 / KTD-10：拒收不仅是 diagnostic，还要给 emit hat 可操作的恢复事件，并进入 retry 预算。

**Requirements:** R2

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-core/src/event_loop/rejection.rs`（新增 `RejectionStage::HatHandoff` 或复用现有 stage）
- Test: `crates/ralph-core/tests/scenarios/hat_handoff/next_rejected.yml` 或新增单元测试

**Approach:**
- 在 `RejectionStage` 新增 `HatHandoff` 变体（或文档明确复用 `Policy` stage 的风险与理由）。
- 在 gate reject 分支构造 `Rejection`：
  - `stage: HatHandoff`
  - `source_hat: from_hat`, `business_hat: from_hat`
  - `topic: ev.topic`
  - `violation: message`
  - `retry_eligible: true`
  - `target_hat: from_hat`
  - `original_event_id: format!("{}:{}", ev.ts, ev.topic)`
- 调用 `build_task_resume_payload` 生成 payload，publish `Event::new("task.resume", payload).with_target(from_hat)`。
- 保留 `diagnostic.hat_handoff.rejected` 事件作为可观测审计。

**Patterns to follow:**
- 参考 `event_loop/mod.rs` 中其他 rejection 注入点（如 origin/policy reject）如何构造 `Rejection` 与 publish `task.resume`。

**Test scenarios:**
- Integration: macro edge 因 `## next` 反模式被拒收后，`task.resume` 出现在 bus/seen_topics，且 `target` 为 emit hat。
- Integration: `work.done` 被拒收时，`work.done` 不在 seen_topics，但 `diagnostic.hat_handoff.rejected` 与 `task.resume` 都在 seen_topics。
- Edge case: 连续 4 次同一 (hat, topic, hat_handoff reason) 拒收后应触发 retry budget 升级（若 `Rejection` retry 计数生效）。

**Verification:**
- `cargo nextest run -p ralph-core --test scenarios hat_handoff` 绿。

---

- [ ] U6. **Wire `hat_handoff_file_read_fail` reason_code**

**Goal:** 区分「文件不存在」与「文件存在但不可读」。

**Requirements:** R5

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/hat_handoff/gate.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-cli/src/policy_check.rs`（适配新签名）
- Test: `crates/ralph-core/src/hat_handoff/gate.rs`

**Approach:**
- 将 `evaluate_event` 的 `file_content` 参数从 `Option<&str>` 改为 tri-state 类型（例如 `FileContent::Missing` / `FileContent::Read(String)` / `FileContent::ReadError(io::Error)`）。
- 在 `event_loop/mod.rs` 读盘时捕获 `std::fs::read_to_string` 的 `Err` 并传入 `FileContent::ReadError`。
- gate 对 `ReadError` 返回 `REASON_CODE_HAT_HANDOFF_FILE_READ_FAIL`。
- CLI `check_hat_handoff_gate` 无读盘失败场景，统一传 `FileContent::Read(content)` 或 `Missing`。

**Patterns to follow:**
- 最小改动 `evaluate_event` 签名；CLI 与 runtime 共用同一 tri-state 类型。

**Test scenarios:**
- Error path: 文件存在但权限 000 时，reason_code 为 `hat_handoff_file_read_fail`。
- Error path: 文件不存在时仍为 `hat_handoff_file_not_found`。

**Verification:**
- `cargo nextest run -p ralph-core --lib hat_handoff` 绿。

---

- [ ] U7. **Add missing tests and code-quality cleanup**

**Goal:** 补齐 T9 与 CLI seq 路径测试，清理死代码。

**Requirements:** R6

**Dependencies:** U1, U5, U6

**Files:**
- Create: `crates/ralph-core/src/event_loop/tests/hat_handoff_gate.rs`
- Modify: `crates/ralph-cli/src/policy_check.rs`（新增测试）
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（删除/合并 `prepend_hat_handoff` 死代码）

**Approach:**
- **T9 测试**：在 `event_loop/tests/hat_handoff_gate.rs` 中构造场景：
  - 配置 isolated + hat_handoff enabled + 两个 hat。
  - executor emit `work.done` 带 macro handoff_path；policy accept 时 `handoff_tracker.pending_count() == 1`。
  - gate 因结构违规 reject；断言 `handoff_tracker.pending_count() == 0`。
- **CLI seq 测试**：在 `policy_check.rs` 的 `hat_handoff_tests` 中补充 env-present 与 env-absent 的合法路径用例。
- **死代码**：删除未调用的 `prepend_hat_handoff`，将其实现合并到 `prepend_hat_handoff_from_pending`，或确认无调用后直接删除。

**Test scenarios:**
- Integration T9: policy accept → handoff reject → Tracker 无 phantom pending。
- Unit: CLI policy_check env-present 时接受 iteration=3/seq=2。
- Unit: CLI policy_check env-absent 时接受任意合法 seq。
- Regression: 所有现有 hat_handoff BDD 场景仍绿。

**Verification:**
- `cargo nextest run -p ralph-core --lib hat_handoff`
- `cargo nextest run -p ralph-core --test scenarios hat_handoff`
- `cargo nextest run -p ralph-cli --bin ralph -- hat_handoff`

---

## System-Wide Impact

- **Interaction graph:**
  - `## ORCHESTRATOR CONTEXT` 新增字段，下游 prompt 解析工具（agent、测试 scraper）可感知。
  - `task.resume` 注入通过 `Rejection` 系统进入 retry 预算与 recovery envelope 路径。
  - 新增 env vars 会随 isolated backend 调用传递，影响所有在 loop 内执行的 agent bash/CLI 调用。
- **Error propagation:**
  - CLI 预检在已知 loop 状态时校验 seq，未知时降级，避免误杀合法 emit。
  - runtime gate 的 `task.resume` 让拒收事件进入标准恢复流与 retry 预算。
- **State lifecycle risks:**
  - `Rejection::retry_key` 应包含 `hat_handoff` stage 与具体 reason_code，避免与其他 rejection 合并导致预算错配。
  - `HandoffTracker::cancel_pending` 在 reject 时调用，防止 phantom pending。
- **API surface parity:**
  - CLI `ralph tools handoff prepare` 参数不变；文档示例补充 env var 与 context 两种来源。
  - CLI `ralph emit --policy-check` 在 loop 子进程内与 runtime 同语义，独立使用时保留结构/R15/路径安全预检。
- **Unchanged invariants:**
  - `HandoffTracker::on_handoff_accepted` 记录点不变。
  - macro 边判定规则（自环、豁免、显式 macro_topics）不变。
  - preset `hat_handoff.enabled: false` 不变。
  - diagnostic topic `diagnostic.hat_handoff.rejected` 不变。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| 新增 env var 与现有 backend env 命名冲突 | 使用 `RALPH_LOOP_ITERATION` / `RALPH_HAT_HANDOFF_SEQ` 前缀，与现有 `RALPH_*` 命名一致 |
| `task.resume` 注入导致循环重试 | 通过 `Rejection` 系统纳入 retry 预算；retry_key 包含 stage 与 reason_code |
| ORCHESTRATOR CONTEXT 字段增加 token 消耗 | 仅 enabled 时输出三行，默认关闭无影响 |
| `RejectionStage::HatHandoff` 新增变体影响序列化 | 若 `RejectionStage` 已序列化到 recovery.jsonl，新增变体需向后兼容；仅用于 `task.resume` payload 内部，不持久化时可接受 |

---

## Documentation / Operational Notes

- 更新 `crates/ralph-core/data/ralph-tools-handoff.md`（U4）。
- 无需更新 `presets/index.json` 或 `scripts/ralph-zsh-plugin.zsh`（未新增/删除 builtin preset 或 CLI 子命令）。
- 完成后按 AGENTS.md 反向验证：用 `sed -n` 复核 `crates/ralph-core/data/*.md` 中的源码引用行号是否仍准确。

---

## Sources & References

- **Origin document / 被修复的计划：** `docs/plans/2026-06-18-002-feat-isolated-hat-handoff-plan.md`
- Review 输出：本次 review 发现的 P0/P1/P2 问题清单
- 相关代码：
  - `crates/ralph-cli/src/loop_runner/runner.rs`
  - `crates/ralph-cli/src/policy_check.rs`
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/event_loop/rejection.rs`
  - `crates/ralph-core/src/runtime_state.rs`
  - `crates/ralph-core/src/hat_handoff/gate.rs`
  - `crates/ralph-core/data/ralph-tools-handoff.md`
  - `crates/ralph-core/tests/scenarios.rs`
