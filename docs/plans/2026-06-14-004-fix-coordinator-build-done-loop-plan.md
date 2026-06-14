---
title: 修复 ce-executor-isolated 下 coordinator 反复 emit build.done 导致的无限循环
type: fix
status: completed
date: 2026-06-14
origin: docs/brainstorms/2026-06-14-ce-executor-isolated-agent-output-governance-requirements.md
---

# 修复 ce-executor-isolated 下 coordinator 反复 emit build.done 导致的无限循环

---

## 概述

在 `ce-executor-isolated` preset 下，coordinator hat 被观察到反复发射 `build.done` 事件。该 topic 不在 coordinator 的 `publishes` 列表中（coordinator 仅允许 `work.ready` / `work.failed`），因此每次都被 `EventOriginGuard` / `isolated_publish_allowed` 拒绝，并注入一条指向 coordinator 的 `task.resume`。下一轮 coordinator 再次被激活，再次发射 `build.done`，形成无限循环，最终导致 loop 因 `max_iterations` 或 `consecutive_failures` 终止。

本计划通过两条独立防线解决该问题：

1. **消除污染源**：修正 `ralph-tools-emit.md` 中把 `build.done` 作为通用 `ralph emit` 示例的误导性内容，并补充 hat-scope 规则说明。
2. **运行期熔断**：在 isolated-scope rejection 路径增加 loop-level 连续同因 violation 计数器，当同一 hat 连续因同一越权 topic 被拒绝达到阈值时，终止重试并产生明确的终止提示，避免无限循环。

---

## 问题框定

### 现场证据

- 目标 worktree：`.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-calm-oak/.ralph/`
- events.jsonl 中出现 coordinator 发射的 `build.done` 事件序列。
- 每次 `build.done` 后紧跟 `event.isolation.boundary_violation` 与指向 coordinator 的 `task.resume`。
- coordinator prompt 中通过 `ralph tools skill load ralph-tools-emit` 加载了 `crates/ralph-core/data/ralph-tools-emit.md`，该 skill 第 27 行明确把 `build.done` 列为 `ralph emit <TOPIC>` 的示例。
- `ce-executor-isolated.yml` 中 coordinator 的 `publishes: ["work.ready", "work.failed"]`，不含 `build.done`。

### 根因定性

| 层级 | 问题 | 后果 |
|---|---|---|
| Skill 文档 | `ralph-tools-emit.md` 用 `build.done` 作为通用示例 | 任何加载该 skill 的 hat（包括 coordinator）都可能模仿该示例 |
| Preset 配置 | coordinator 未声明 `build.done` | 发射即越权 |
| Runtime 机制 | isolated-scope rejection 只注入 `task.resume`，无连续同因熔断 | 同一 hat 反复尝试同一非法 topic，loop 无法自行退出 |

---

## 需求追溯

本计划承接上游需求文档 `docs/brainstorms/2026-06-14-ce-executor-isolated-agent-output-governance-requirements.md`：

- **R2** CLI 写入前强制 policy 校验（已由 `2026-06-14-001` 覆盖，本计划不再重复实现，但测试需验证 `build.done` 仍被拒绝）。
- **R5** Hard-gate 后 hat 路由稳定性：policy / workflow guard / execution contract / isolated scope 的 rejection 产生 `task.resume` 时应路由到源 hat。本计划在 R5 基础上增加**连续同因 rejection 的熔断升级**。
- **SC5** 新增/修改测试覆盖核心路径；`cargo nextest run --workspace --exclude ralph-e2e` 通过。
- **SC6** 相关 skill 文档同步更新，并做 `--help` 冒烟。

新增本计划特有需求：

- **R-A.** `ralph-tools-emit.md` 中的示例 topic 不得与 `ce-executor-isolated` 中任一 hat 的受限 topic 冲突，避免误导。
- **R-B.** 当同一 hat 连续因同一越权 topic 触发 isolated-scope rejection 时，runtime 必须复用 `U2_REJECTION_RETRY_LIMIT` 的 bounded-retry 语义（前 3 次注入 `task.resume`，第 4 次升级为终止提示），而不是无限重试。
- **R-C.** 升级终止时，必须向 `.ralph/agent/summary.md` / `recovery.jsonl` / `loop.terminate` payload 写入清晰的诊断信息，包含 hat、topic、允许列表、连续次数。

---

## 范围边界

### 本次覆盖

- `crates/ralph-core/data/ralph-tools-emit.md` 的内容修正与 hat-scope 提示补充。
- `crates/ralph-core/src/event_loop/mod.rs` 中 isolated-scope rejection 路径的连续同因 violation 熔断逻辑（复用 `LoopState` 已有的 `rejection_retry_counts`）。
- `crates/ralph-core/src/event_loop/loop_state.rs` 中新增终止标志或 `check_termination()` 识别逻辑。
- 针对上述改动的单元测试与集成测试。

### 本次不覆盖

- `ralph emit` / `ralph wave emit` CLI policy precheck 本身（由 `2026-06-14-001` 负责）。
- 修改 coordinator 的 `publishes` 列表以包含 `build.done`（这会破坏职责边界，不属于本修复）。
- 对所有 skill 文档进行全局审计（仅修正直接导致本问题的 `ralph-tools-emit.md`）。
- 引入通用 LLM-as-judge 来检测 prompt 中的误导性示例。
- 自动修复已存在 worktree 中的历史 `build.done` 事件。

### Deferred to Follow-Up Work

- 全局 skill 文档误导性示例扫描：后续可扩展为一个 lint 规则，扫描所有内置 skill 中的示例 topic 是否超出各 preset hat 的 `publishes`。
- 可配置熔断阈值：当前复用 `U2_REJECTION_RETRY_LIMIT = 3`（第 4 次触发熔断），后续可根据 preset 配置 `event_loop.scope_violation_circuit_breaker_threshold` 调整。

---

## 背景调研

### 相关代码与模式

- `crates/ralph-core/data/ralph-tools-emit.md`：`ralph emit` 参考 skill，第 27 行把 `build.done` 作为示例 topic。
- `presets/en/ce-executor-isolated.yml`：coordinator hat 的 `publishes: ["work.ready", "work.failed"]`；`topic_deny_rules` 中 `executor` 被禁止 emit `build.done`。
- `crates/ralph-core/src/event_loop/mod.rs`：
  - `isolated_publish_allowed`（约 1465 行）：判断 hat 是否有权发布某 topic。
  - `process_events_from_jsonl_with_waves` / `process_parse_result` 中的 isolated-scope rejection 分支（约 5267–5539 行）：发布 `event.isolation.boundary_violation`、写 recovery envelope、注入 `task.resume`。
  - `publish_policy_rejection_resume`（316 行）：构建指向源 hat 的 `task.resume`。
- `crates/ralph-core/src/event_loop/loop_state.rs`：`LoopState` 结构体，维护 iteration、last_event 等运行期状态。
- `crates/ralph-core/src/event_loop/rejection.rs`：`build_task_resume_payload`、`Rejection`、`WaveContextForResume`。
- `crates/ralph-core/src/diagnosis/`：runtime diagnosis 信封、`RecoveryResponder`、`TerminationHint`。

### 既有机制

- 当前 rejection 路径已有：
  - per-turn recovery envelope dedup（`envelopes_written_this_turn`），防止同一 turn 内多个相同 scope drop 重复写 `recovery.jsonl`。
  - pending `task.resume` dedup（`already_pending_recovery`），防止同一 turn 内向同一 hat 注入多个 `task.resume`。
- 当前缺失：跨 turn 的连续同因 violation 计数与熔断。

### 外部参考

- 上游需求：`docs/brainstorms/2026-06-14-ce-executor-isolated-agent-output-governance-requirements.md`。
- 相关计划：`docs/plans/2026-06-14-003-ce-executor-isolated-agent-output-governance-plan.md`（R1/R3/R4/R5）。

---

## 关键技术决策

1. ** skill 文档修正优先于 prompt 增强**
   - 不在 coordinator instructions 中反复强调「不要 emit build.done」，而是直接修正导致错误模仿的 skill 示例。
   - 理由：机制优先于编排补丁；prompt 增强对每次加载 skill 的 hat 都不可见（isolated 模式下各 hat prompt 独立）。

2. **复用 `U2_REJECTION_RETRY_LIMIT = 3` 的现有语义**
   - 理由：`LoopState` 已存在该常量及 `record_rejection_key` / `rejection_key_is_exhausted` 方法，其语义为「前 3 次允许 retry（注入 `task.resume`），第 4 次判定为 exhausted」。复用可避免引入第二套计数器，并保持与其他 bounded-retry 场景的行为一致。
   - 后续可通过 preset 配置暴露独立阈值，但本计划保持最小变更。

3. **计数器 key 为 (hat_id, topic)**
   - 仅对完全相同的 (hat, topic) 组合计数，不同 topic 或不同 hat 独立计数。
   - 理由：本问题的核心正是「同一 hat 反复 emit 同一越权 topic」。

4. **计数器在内存中维护，不持久化**
   - 重启 loop 后计数清零，这是可接受的：熔断针对的是单次 loop 运行中的急性循环。
   - 理由：持久化会增加复杂度，且问题场景通常是单次运行内快速循环。

5. **达到阈值后直接产生自包含的终止信号，不依赖 opt-in 的 runtime diagnosis**
   - `ce-executor-isolated.yml` 默认不启用 `telemetry.runtime_diagnosis`，而 `TerminationHint` 需要 drift engine 消费才会真正终止 loop。因此熔断必须直接设置 `LoopState` 中的终止原因（新增 `scope_violation_circuit_breaker_tripped` 字段）或在 `EventLoop::check_termination()` 中直接识别。
   - 同时发布 `loop.terminate` 诊断事件，便于 `ralph diagnose` 生成报告；runner 看到该事件或终止标志后立即退出。
   - 保持 recovery envelope 写入用于追溯，但实际停止不依赖 drift engine。

6. **不修改 `build.done` 的 topic_deny 规则**
   - `ce-executor-isolated.yml` 中 `build.done` 仍对 `executor` 禁止、coordinator 仍不可发布。
   - 理由：保持权限边界不变，仅修正文档示例和增加运行期熔断。

---

## 待解决问题

### 规划中已解决

- **Q1: 为什么 coordinator 会知道 `build.done`？**
  - 决议：通过 `ralph tools skill load ralph-tools-emit` 加载的 skill 文档把 `build.done` 作为示例 topic，coordinator 读取后模仿。
- **Q2: 是否应该增强 coordinator instructions 来禁止 `build.done`？**
  - 决议：不优先采用。本次修复聚焦 skill 文档示例修正 + 运行期熔断；如后续仍出现，再考虑补充 instructions。
- **Q3: 熔断阈值选多少？**
  - 决议：硬编码 3 次，基于「给一次修正机会，第三次终止」的经验法则。
- **Q4: 计数器放在哪里？**
  - 决议：放在 `LoopState` 中，key 为 `(hat_id, topic)`，值为连续拒绝次数，每次成功 publish 该 hat 的合法事件后清零。

### 延迟到实现阶段

- **Q5: 是否需要把连续同因 violation 的熔断逻辑扩展到 topic-deny / payload-contract / workflow-guard 等其他 rejection 类型？**
  - 延迟理由：本问题仅确认发生在 isolated-scope rejection；扩展需单独评估，避免过度泛化。
- **Q6: 终止提示的 severity 是否应为 `Critical` 而非 `Error`？**
  - 延迟理由：取决于实现时 `TerminationHint` 的分级对 diagnose 报告和 runner 行为的影响，可在实现 PR 中由 reviewer 决定。

---

## 高层技术设计

> *本图用于说明方案形状，是方向性指导而非实现规范。*

```text
┌─────────────────────────────────────────────────────────────────────┐
│                         问题循环现状                                │
│  coordinator emit build.done                                        │
│       ↓                                                             │
│  isolated_publish_allowed(coordinator, build.done) == false         │
│       ↓                                                             │
│  publish event.isolation.boundary_violation                         │
│  write recovery envelope                                            │
│  publish task.resume → target=coordinator                           │
│       ↓                                                             │
│  下一轮激活 coordinator → 再次 emit build.done ...                   │
└─────────────────────────────────────────────────────────────────────┘

┌─────────────────────────────────────────────────────────────────────┐
│                         修复后流程                                  │
│  第 1–3 次：保持现有行为（task.resume + 允许列表提示）                │
│       ↓                                                             │
│  第 4 次：命中熔断 → 不再注入 task.resume                            │
│       ↓                                                             │
│  设置 LoopState 终止标志 + 发布 loop.terminate 诊断事件                 │
│  runner 终止 loop， diagnose 报告指向 skill 示例 / hat / topic        │
└─────────────────────────────────────────────────────────────────────┘
```

---

## 实现单元

- [x] U1. **修正 `ralph-tools-emit.md` 中的误导性 topic 示例并补充 hat-scope 规则**

**目标：** 消除 coordinator 模仿 `build.done` 的污染源，让 skill 文档示例不会暗示 hat 可以发布其未声明的 topic。

**需求：** R-A、SC6

**依赖：** 无

**文件：**
- 修改：`crates/ralph-core/data/ralph-tools-emit.md`
- 测试：`crates/ralph-core/src/skill.rs` 或 `crates/ralph-core/src/event_loop/tests/`（skill 内容通过 `include_str!` 注册；prompt 注入测试在 event loop 测试目录）

**方案：**
1. 把第 27 行「事件主题，如 `build.done`、`review.complete`」改为不与其他 hat 专属 topic 冲突的通用示例。建议使用 `my-event`（中性、不属于任何 hat）或 `task.completed`，并明确说明「示例仅供参考，实际可发布 topic 以当前 hat 的 `publishes` 为准；isolated 模式下越权 topic 会被拒绝」。
2. 在「反模式 / 注意事项」下新增一条：
   - 发射 topic 前必须确认当前 hat 的 `publishes` 列表包含该 topic；isolated 模式下越权 topic 会被拒绝并触发 `task.resume`。
3. 检查同文件中是否还有其他示例 topic 与 `ce-executor-isolated` 的 hat scope 冲突（如 `review.complete` 是否在 review-synthesizer 的 publishes 中；若是，保留但加注释说明所属 hat）。

**模式遵循：**
- 保持 skill 文档的 YAML frontmatter 不变。
- 保持 `ralph-tools-emit.md` 作为「完整参考」的定位，不删减功能说明，只修正示例。

**测试场景：**
- **Happy path**：解析 `ralph-tools-emit.md` 内容，确认 `<TOPIC>` 示例行不再包含 `build.done`。
- **集成场景**：在 `ce-executor-isolated` 的 coordinator prompt 构建测试中，确认 `build.done` 不会作为 `ralph emit` 示例出现在注入的 skill 文本中。

**验收标准：**
- `ralph tools skill load ralph-tools-emit` 输出中不再出现 `build.done` 作为推荐示例。
- 现有 `ce-executor-isolated` 下 coordinator 的 prompt 不再因该 skill 而包含 `build.done` 示例。

---

- [x] U2. **在 isolated-scope rejection 路径实现熔断升级逻辑**

**目标：** 当同一 (hat, topic) 连续越权达到阈值时，停止注入 `task.resume`，改为直接产生自包含的终止信号并写诊断。

**需求：** R-B、R-C、R5

**依赖：** U1

**文件：**
- 修改：`crates/ralph-core/src/event_loop/mod.rs`
- 修改：`crates/ralph-core/src/event_loop/loop_state.rs`（新增终止标志或扩展 `check_termination()`）
- 测试：`crates/ralph-core/tests/scenarios/` 新增 BDD 场景 或 `crates/ralph-core/src/event_loop/tests.rs`（如存在）

**方案：**
1. 复用 `LoopState` 已有的 `rejection_retry_counts` 基础设施（`crates/ralph-core/src/event_loop/loop_state.rs:124-136`）。该计数器 key 为稳定的字符串，由 `Rejection::compute_retry_key` 生成，格式为 `stage:source_hat:topic:violation_class`；本次使用已存在的 `scope_drop_retry_key`（已按 `source:scope_hat:topic:reason_code` 命名空间化，并包含 `wave_id/wave_index` 信息）。
2. 在 `process_events_from_jsonl_with_waves` / `process_parse_result` 的 isolated-scope rejection 分支中，当前代码已：
   - 计算 `scope_hat`。
   - 发布 `event.isolation.boundary_violation`。
   - 构造 `scope_drop_retry_key` 并写 recovery envelope。
   - 构建 allowed topics 列表。
   - 检查 `already_pending_recovery`。
3. 在构建 `task.resume` 之前插入熔断判断：
   - 调用 `let count = self.state.record_rejection_key(&scope_drop_retry_key);`
   - 调用 `if self.state.rejection_key_is_exhausted(&scope_drop_retry_key)`（即第 4 次触发时返回 true，与 `U2_REJECTION_RETRY_LIMIT` 语义保持一致：前 3 次允许 `task.resume`，第 4 次熔断）：
     - 不再构建/发布 `task.resume`。
     - 在 `LoopState` 中设置终止标志，例如新增字段 `scope_violation_circuit_breaker_tripped: Option<TerminationReason>`（推荐新增一个 `TerminationReason::ScopeViolationCircuitBreaker { hat, topic, allowed_topics, violation_count }` 变体，退出码非零）。
     - 发布 `loop.terminate` 诊断事件，payload 包含 `hat`、`topic`、`violation_count`、`allowed_topics`、`reason_code: "scope_violation_circuit_breaker_tripped"`。
     - 继续把本次 `event.isolation.boundary_violation` 和 recovery envelope 写入日志（保留可追溯性）。
   - 若未耗尽：
     - 保持现有逻辑，注入 `task.resume`（含 allowed topics）。
4. 在 `EventLoop::check_termination()`（或 runner 读取 loop 终止状态的等价位置）中识别 `scope_violation_circuit_breaker_tripped` 标志，立即结束 loop。该路径不依赖 `telemetry.runtime_diagnosis` 或 drift engine。
5. 清零/预算恢复：当某 hat 后续成功发布一个合法事件时，需要清除该 hat/topic 对应的 rejection key 计数，避免旧计数影响未来不同 topic 的判断。可选方案：
   - 方案 A（推荐）：在事件被 `observe_accepted` 时，若 `event.hat` 与某 rejection key 的 source hat 匹配，则 `self.state.rejection_retry_counts.remove(key)`。
   - 方案 B：更简单但较粗——每次任意 hat 成功发布合法事件后，清除该 hat 对应的所有 rejection key。
   - 实现时评估哪种 reset 与现有 `rejection_retry_counts` 的其它消费者兼容。

**模式遵循：**
- 复用现有 `rejection_retry_counts`、`record_rejection_key`、`rejection_key_is_exhausted`，不引入第二套计数器。
- 复用 `event.isolation.boundary_violation` 和 recovery envelope 写入逻辑。
- `task.resume` 构建继续走 `build_task_resume_payload` helper，保证格式一致。
- 终止路径与 drift engine / runtime diagnosis 解耦，符合 `ce-executor-isolated` 默认不启用 telemetry 的设计。

**测试场景：**
- **Happy path（未熔断）**：coordinator 第 1–3 次 emit `build.done` → 每次都被拒绝并注入 `task.resume`；第 4 次之前未触发终止。
- **Error path（熔断）**：coordinator 第 4 次 emit `build.done` → 拒绝，**不**注入 `task.resume`，`LoopState` 中设置 `scope_violation_circuit_breaker_tripped`，并发布 `loop.terminate`。
- **Edge case（阈值边界精确性）**：2 次 violation 注入 `task.resume`，3 次 violation 仍注入 `task.resume`，4 次 violation 终止且不再注入 `task.resume`。
- **Edge case（恢复清零）**：coordinator 第 1 次 emit `build.done`（拒绝），随后 emit 合法 `work.ready`（接受），再 emit `build.done` → 对应 rejection key 被清除，计数重新从 1 开始，不会提前熔断。
- **Edge case（不同 topic 独立计数）**：coordinator 连续 3 次 emit `build.done`，再连续 3 次 emit `review.complete` → 各自计数独立，均未达到熔断（因为 key 包含 topic）。
- **Edge case（不同 hat 独立计数）**：`coordinator:build.done` 与 `executor:build.done` 使用不同 key，各自计数独立。
- **Edge case（wave 事件 namespace）**：同一 wave 内 8 个不同 `wave_index` 的越权事件因 `scope_drop_retry_key` 包含 `wave_id/wave_index` 而生成 8 个不同 key；验证该行为不会导致正常 wave 误熔断。
- **Edge case（event.hat fallback）**：当 JSONL record 的 `event.hat` 与 `current_isolated_hat` 不一致时，验证计数/reset key 使用 `scope_hat`（即 `event.hat` 优先、fallback 到 `current_isolated_hat`）。
- **Integration scenario（默认配置无 runtime diagnosis）**：在 `ce-executor-isolated` 默认配置下（不设置 `RALPH_DIAGNOSTICS=1`），模拟 coordinator 连续 4 次 emit `build.done`，验证 loop 仍会终止。
- **Integration scenario（BDD YAML）**：新增 scenario，coordinator 输出 `build.done` 四次，验证第四次后 loop 终止、`recovery.jsonl` 包含 `scope_violation_circuit_breaker_tripped`、`loop.terminate` payload 包含 `hat`/`topic`/`violation_count`/`allowed_topics`。

**验收标准：**
- 新增/修改的单元测试与集成测试通过。
- `ce-executor-isolated` 默认配置下熔断能终止 loop。
- 现有 `ce-executor-isolated` 相关测试不回归。

---

- [x] U3. **运行全量验证（nextest、clippy、doctest）**

**目标：** 确保改动不引入回归，满足 AGENTS.md 中的测试入口要求。

**需求：** SC5

**依赖：** U1、U2

**文件：**
- 受影响：全 workspace（除 `ralph-e2e` 按默认 CI 路径排除）

**方案：**
1. 运行 `./scripts/run-tests.sh`（等价于 `cargo nextest run --workspace --exclude ralph-e2e` + `cargo test --workspace --exclude ralph-e2e --doc`）。
2. 运行 `cargo clippy --workspace --exclude ralph-e2e`。
3. 运行 `cargo fmt --check`。
4. 运行 `cargo doc --no-deps`（确认新增文档无警告）。
5. 对 `ralph-cli` 的 `loop_runner` 二进制测试使用 `cargo nextest run -p ralph-cli --bin ralph -- <subset>` 形式，遵循 `.config/nextest.toml` 的 `cli-serial` 配置。

**模式遵循：**
- AGENTS.md 中「默认走并发，ralph-cli 串行」的分级表。
- 不使用裸 `cargo test -p ralph-cli`。

**测试场景：**
- 全 workspace nextest 通过。
- doctest 通过。
- clippy 无新增 warning。

**验收标准：**
- `./scripts/run-tests.sh` 退出码为 0。
- `cargo clippy --workspace --exclude ralph-e2e` 退出码为 0（允许现有 pre-existing warning，但不得新增由本改动引入的 warning）。

---

## 系统级影响

- **交互图：**
  - `EventLoop::process_events_from_jsonl_with_waves` / `process_parse_result` 与 `LoopState` 新增交互：调用 `record_rejection_key` / `rejection_key_is_exhausted`，并在熔断时设置 `scope_violation_circuit_breaker_tripped`。
  - `LoopState` 与 runner 的终止检查路径新增交互（`check_termination()` 识别熔断标志）。
  - `EventBus` 继续接收 `event.isolation.boundary_violation`，但在熔断后不再接收针对同一 rejection key 的 `task.resume`。
- **错误传播：**
  - 熔断直接设置 `LoopState` 终止标志并发布 `loop.terminate`，不依赖 drift engine / runtime diagnosis。
  - `ralph diagnose` 可通过 `recovery.jsonl` 中的 `scope_violation_circuit_breaker_tripped` 生成诊断报告。
- **状态生命周期风险：**
  - `rejection_retry_counts` 仅内存维护，loop 重启后清零，不会遗留错误状态。
  - 成功发布合法事件会清除对应 rejection key，避免误熔断。
- **API 表面一致性：**
  - 不修改公共 API；`LoopState` 为 crate 内部结构。
  - `ralph-tools-emit.md` 是用户可见文档，修改后需确保 `--help` 和 skill load 输出符合预期。
- **集成覆盖：**
  - 单元测试覆盖 `rejection_retry_counts` 在 isolated-scope 路径的复用。
  - 集成测试/BDD scenario 覆盖连续 4 次同因越权 → 熔断并终止 loop 的完整路径。
  - 集成测试验证默认配置下（无 `RALPH_DIAGNOSTICS=1`）熔断仍能终止 loop。
- **不变量：**
  - `ce-executor-isolated.yml` 中各 hat 的 `publishes` 不变。
  - `topic_deny_rules` 不变。
  - isolated-scope rejection 的日志记录（boundary_violation event + recovery envelope）不变。

---

## 风险与依赖

| 风险 | 缓解措施 |
|---|---|
| 修正 skill 文档示例后，仍可能有其他来源让 coordinator 知道 `build.done` | U3 熔断作为第二道防线；若熔断触发，诊断信息会指出 topic，便于进一步追踪污染源 |
| 熔断阈值 3 次可能过严，导致正常恢复也被终止 | 成功发布合法事件会清零计数；测试覆盖「拒绝→合法→拒绝」场景 |
| 在 `LoopState` 新增终止标志破坏现有行为 | `LoopState` 为 crate 内部结构，不对外序列化；新增 `Option<TerminationReason>` 字段有默认值；实现前 grep 确认无 `Serialize`/`Deserialize` derive |
| 现有测试依赖于连续 rejection 后仍注入 `task.resume` | 在修改前运行 `./scripts/run-tests.sh` 获取基线；若相关测试失败，评估是需要更新测试还是逻辑有误 |
| 修改 `ralph-tools-emit.md` 可能影响其他 preset 的 agent | 新示例 `my-event` / `task.completed` 为中性通用 topic，不绑定到任何 hat；补充的 hat-scope 提示对所有 preset 都是正面约束 |

---

## 文档与操作说明

- `crates/ralph-core/data/ralph-tools-emit.md` 无对应符号链接需要同步；`.claude/skills/ralph-tools/SKILL.md` 指向的是 `crates/ralph-core/data/ralph-tools.md`，与本文件无关。
- 若后续将熔断阈值配置化，需在 `docs/guide/runtime-diagnosis.md` 或相关配置文档中补充说明。
- 运行 `ralph diagnose --session latest` 应能展示 `scope_violation_circuit_breaker_tripped` 相关诊断。

---

## 来源与参考

- **上游需求文档：** `docs/brainstorms/2026-06-14-ce-executor-isolated-agent-output-governance-requirements.md`
- **相关计划：** `docs/plans/2026-06-14-003-ce-executor-isolated-agent-output-governance-plan.md`
- **相关代码：**
  - `crates/ralph-core/data/ralph-tools-emit.md`
  - `presets/en/ce-executor-isolated.yml`
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/event_loop/loop_state.rs`
  - `crates/ralph-core/src/event_loop/rejection.rs`
  - `crates/ralph-core/src/diagnosis/`
