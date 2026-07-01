---
title: fix: ce-executor-serial 通过 commit footer 元数据与 tasks.jsonl 指引 coordinator 正确进入 plan.complete
type: fix
status: active
date: 2026-07-01
origin: docs/brainstorms/2026-07-01-ce-executor-serial-fix-unit-terminal-guidance-requirements.md
---

# fix: ce-executor-serial 通过 commit footer 元数据与 tasks.jsonl 指引 coordinator 正确进入 plan.complete

## Overview

`ce-executor-serial` 在 isolated 模式下，最后一个 fix-unit 完成后需要由 coordinator 从 `work.ready(fix-NN)` 切换到 `plan.complete`。之前的 U6 尝试让 base runtime 扫描 `plan.md` / `fix-plan.md` 的 `### U{N}.` 标题来缓存拓扑并计算 `expected_event`，因破坏历史 plan 写法而被回滚。

本计划改用更轻量、职责更清晰的方案：
1. **executor 在 fix-unit 工作 commit 的 footer 中加入 `[fix-unit: fix-NN]` 元数据。**
2. **runtime 在 execution contract 中软校验该 footer**，缺失时发出诊断但不阻塞事件。
3. **coordinator 的 prompt 指令改为读取 `tasks.jsonl` 中的 fix-unit 任务列表**来判断 total_fix_units，不再数 fix-plan 标题。
4. **保留并验证已有机制兜底**：U1 终态事件预算优先、U3 `CoordinatorDecisionGateStage` topic 改写、U2 跨 activation `completion_honored` 守卫。

---

## Problem Frame

- isolated 模式规定每轮 activation 只有一个非 wave 业务事件能进入总线。若 coordinator 在最后一个 fix-unit 后先误发了一个 stray `work.ready`，真正的 `plan.complete` 会被静默丢弃，循环降级为 `plan.blocked`。
- coordinator 之前靠数 `fix-plan.md` 中的 `### U{N}.` 标题判断 total_fix_units，但该逻辑既容易出错，也与「base runtime 不解析业务 markdown」的回滚教训冲突。
- 需要给 coordinator 一个稳定、结构化、不依赖 plan 散文解析的终态判断信号。

参见 origin 文档的 `Problem Frame`、`Actors`、`Key Flows` 与 `Acceptance Examples`。

---

## Requirements Trace

- R1. executor 在 fix-unit 最终 commit message footer 加入 `[fix-unit: fix-NN]`。
- R2. footer 不影响标题主体，允许多 commit，至少一个包含标记。
- R3. runtime 软校验 commit footer，缺失时发出诊断但不 hard-block。
- R4. coordinator 禁止通过数 fix-plan 标题判断 total_fix_units。
- R5. coordinator 通过 `tasks.jsonl` 中 fix-unit 任务总数与已完成数判断是否为最后一个 fix-unit。
- R6. coordinator 的 `plan.complete` payload 必须携带当前 `step`（`fix-NN` 或对象形式）。
- R7. isolated 预算保持终态事件优先（U1）。
- R8. `CoordinatorDecisionGateStage` 继续改写 `work.ready(last_in_phase=true)` → `plan.complete`（U3）。
- R9. 跨 activation `completion_honored` 守卫不被破坏（U2）。
- R10. `presets/en/ce-executor-serial.yml` 删除「数 `### U{N}.` heading」指令，替换为读 `tasks.jsonl` + commit footer。
- R11. 相关 `crates/ralph-core/data/ralph-tools*.md` 同步更新。

**Origin actors:** A1 coordinator hat, A2 executor hat, A3 base runtime
**Origin flows:** F1 最后一个 fix-unit 正常终态，F2 coordinator 误判还有下一个 fix-unit
**Origin acceptance examples:** AE1–AE5

---

## Scope Boundaries

- 在范围内：commit footer 约定、runtime 软校验与诊断、coordinator prompt 改写、U1/U3 机制保持与验证、`tasks.jsonl` 读取逻辑、BDD 场景与 preset_lint。
- 不在范围内：重新引入 base runtime 解析 plan/fix-plan markdown 正文的逻辑。
- 不在范围内：把 fix-unit tag 变成 execution contract hard gate。
- 不在范围内：修改 commit message 标题格式或新增 Conventional Commits 类型。
- 不在范围内：修改 `review_step_state::prefill_fix_steps_from_plan` 的现有行为。

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/execution_contract.rs`：`validate_execution_contract` 是 execution contract 的统一入口，`GitEvidenceProvider` trait 提供 `is_git_repo` / `has_uncommitted_changes` / `has_new_commits_since`，可在 trait 中新增读取最近 commit messages 的方法。
- `crates/ralph-core/src/event_loop/stages/coordinator_decision_gate_stage.rs`：已存在 `CoordinatorDecisionGateStage`，负责把 `work.ready(last_in_phase=true)` 改写为 `plan.complete` 并补齐 payload 字段（U3）。
- `crates/ralph-core/src/event_loop/mod.rs`：`prepend_orchestrator_context` 生成 `## ORCHESTRATOR CONTEXT` 块注入 coordinator prompt；`prepend_correction_and_resume` / `build_prompt` 路径都可扩展。
- `crates/ralph-core/src/state_projector/task.rs`：`is_fix_unit_key` / `is_fix_unit_id` 用于识别 fix-unit task；`TaskStore` 提供任务读取能力。
- `crates/ralph-core/src/task_store.rs`：维护 `tasks.jsonl`，是 fix-unit 任务总数的权威来源。
- `presets/en/ce-executor-serial.yml`：第 817–892 行左右包含 coordinator 的 fix-unit 推进指令，需要删除「数标题」步骤并替换。
- `crates/ralph-core/data/ralph-tools*.md`：AI agent skill 文档，若涉及 fix-unit 工作流需同步。

### Institutional Learnings

- `docs/solutions/logic-errors/base-runtime-must-not-parse-business-markdown.md`：明确 base runtime 不应解析业务 markdown，语义理解应交给 LLM。
- `docs/plans/2026-07-01-001-fix-ce-executor-serial-p0-terminal-storm-plan.md`：U1/U2/U3/U4 的实施细节与测试要求，本计划继承其中 U1/U2/U3 的验证部分，U6 不再实施。

---

## Key Technical Decisions

- **Commit footer 替代 plan 扫描：** `[fix-unit: fix-NN]` 是工作产物上的元数据，runtime 与 coordinator 都能独立读取，不依赖 markdown 解析。
- **Runtime 只校验不阻塞：** 缺失 tag 时产生 `execution_contract.fix_unit_tag_missing` 诊断，但不拒绝事件，避免误伤历史场景或人工干预。
- **Coordinator 以 tasks.jsonl 为 total 权威：** `tasks.jsonl` 在 `review.complete(fix_plan_file)` 时已经预填所有 fix-unit 任务，总数可靠；commit footer 提供完成进度交叉验证。
- **保留 U1/U3 作为乱发兜底：** prompt 指引是第一道防线，机制改写/预算优先级是第二、三道防线。
- **不改动 `review_step_state::prefill_fix_steps_from_plan`：** 该函数仍按 fix-plan 标题预填 tracker，用于 plan_gate 豁免，但不是 coordinator 判断来源。

---

## Open Questions

### Resolved During Planning

- **Q:** execution contract 中扫描 commit messages 的最佳位置是什么？  
  **A:** 扩展 `GitEvidenceProvider` trait，新增 `recent_commit_messages(workspace, since_sha, max_count) -> Vec<String>` 方法。在 `validate_git_change` 之后增加一个独立的 soft check 函数 `check_fix_unit_commit_footer`，只在当前 step 为 `fix-*` 时调用。
- **Q:** coordinator prompt 中注入 fix-unit 列表的格式？  
  **A:** 在 `## ORCHESTRATOR CONTEXT` 块中新增 `fix_unit_state` 字段，包含 `total`、`completed`、`current` 三个字段，人类可读且结构化。同时保留 `tasks.jsonl` 原始路径提示，让 coordinator 可对账。
- **Q:** `invalid_step_target` 拒绝 reason code 用什么？  
  **A:** 复用或新增 `step_target_not_in_fix_plan` reason，由 emit schema / event policy 返回，并通过 `task.resume` 提示 coordinator 当前已是最后一个 fix-unit。

### Deferred to Implementation

- 是否需要新增 `ExecutionContractViolationKind` 变体来承载 `fix_unit_tag_missing`，还是复用现有 `NoGitEvidence` / 通用 finding？实现时根据现有 enum 扩展性决定。
- `fix_unit_state` 的生成函数放在 `runtime_state.rs` 还是直接在 `prepend_orchestrator_context` 中内联？实现时按最小侵入原则决定。

---

## Implementation Units

- [ ] U1. **在 `GitEvidenceProvider` 中增加读取最近 commit messages 的能力**

**Goal:** 让 execution contract 能够拿到自 loop 开始以来的 commit message 列表，用于软校验 `[fix-unit: fix-NN]` footer。

**Requirements:** R3

**Dependencies:** 无

**Files:**
- Modify: `crates/ralph-core/src/execution_contract.rs`
- Test: `crates/ralph-core/src/execution_contract.rs`

**Approach:**
- 在 `GitEvidenceProvider` trait 中新增 `recent_commit_messages(&self, workspace: &Path, since_sha: Option<&str>, max_count: usize) -> Vec<String>` 方法。
- `DefaultGitEvidenceProvider` 用 `git log --format=%B` 或 `git log --format=%s%n%b` 实现，读取 `since_sha..HEAD` 范围内的 commit messages。
- 如果 `since_sha` 为 `None`，读取最近 N 条（如 10 条）作为 fallback。
- 测试用 mock provider 验证接口契约。

**Patterns to follow:**
- 现有 `has_new_commits_since` 的 `git rev-list` 调用风格。
- 现有 `DefaultGitEvidenceProvider` 的 `Command::new("git")` 模式。

**Test scenarios:**
- Happy path: mock provider 返回包含 `[fix-unit: fix-02]` 的 messages，`recent_commit_messages` 正确返回。
- Edge case: `since_sha=None` 时返回最近 N 条。
- Edge case: 无 commit 范围时返回空 Vec。
- Error path: git 命令失败时返回空 Vec（不 panic）。

**Verification:**
- `cargo nextest run -p ralph-core -- execution_contract` 通过新增与现有测试。

---

- [ ] U2. **为 fix-unit 工作流增加 commit footer 软校验并发出诊断**

**Goal:** 当 `work.done` / `test.passed` / `fix.applied` 等事件携带 `step=fix-NN` 时，检查最近 commit 中是否有匹配的 `[fix-unit: fix-NN]` footer；缺失时产生诊断但不拒绝事件。

**Requirements:** R1, R2, R3

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-core/src/execution_contract.rs`
- Test: `crates/ralph-core/src/execution_contract.rs`

**Approach:**
- 在 `validate_execution_contract` 的 git evidence 校验之后、test evidence 之前，增加一个 `check_fix_unit_commit_footer` 调用。
- 只对 payload 中 `step` 以 `fix-` 开头的事件触发。
- 调用 `git_provider.recent_commit_messages(workspace, loop_start_sha, 10)`，用正则匹配 `\[fix-unit:\s*(fix-\d{2})\]`。
- 若找到匹配且与当前 `step` 一致 → 无 finding。
- 若找不到匹配 → push 一个 finding，kind 为新增或复用的 `FixUnitTagMissing`，message 提示 coordinator 应在 commit footer 中加入 `[fix-unit: fix-NN]`。
- 该 finding 不影响 `Accept/Reject` 决策（即不加入 `findings` 拒绝列表），而是通过独立通道写入诊断/ledger。实现时若诊断通道不易接入，可先让 finding 进入 rejection 列表但配置为 recoverable；需评估对现有测试的影响。

**执行注意：** 实现前先写单元测试确定 finding 语义，避免影响现有 execution contract 测试。

**Patterns to follow:**
- 现有 `validate_git_change` / `validate_test_evidence` 的独立校验函数风格。
- 现有 `ExecutionContractFinding` 结构。

**Test scenarios:**
- Happy path: `step=fix-02`，commit messages 含 `[fix-unit: fix-02]`，无 finding。
- Edge case: 多个 commit，其中一个含匹配 footer，无 finding。
- Error path: `step=fix-02`，commit messages 无 footer，产生 `FixUnitTagMissing` finding。
- Error path: `step=fix-02`，commit messages 含 `[fix-unit: fix-01]`（不匹配），产生 finding。
- Error path: `step=step-01`（非 fix-unit），不触发 footer 校验。
- Integration: 真实 `validate_execution_contract` 调用返回 `Accept` 但附带 diagnostic finding（如果采用非拒绝通道）。

**Verification:**
- 新增 execution contract 单元测试通过。
- 现有 execution contract 测试保持通过。

---

- [ ] U3. **在 coordinator prompt 中注入 fix-unit 状态块**

**Goal:** 给 coordinator 一个结构化、只读的 fix-unit 进度视图，让它不再依赖解析 plan/fix-plan 标题。

**Requirements:** R4, R5, R10

**Dependencies:** 无（不依赖 U1/U2）

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-core/src/runtime_state.rs`（若决定把生成逻辑放在此处）
- Test: `crates/ralph-core/src/runtime_state.rs` 或 `crates/ralph-core/src/event_loop/mod.rs` 的 prompt 相关测试

**Approach:**
- 在 `prepend_orchestrator_context` 中读取 `tasks.jsonl`（通过 `state.state_projection` 或 `LoopContext` 路径），统计所有 `fix-*` task 的数量与状态。
- 新增 `fix_unit_state` 字段注入 `## ORCHESTRATOR CONTEXT` 块，格式示例：
  ```json
  {
    "fix_unit_state": {
      "total": 2,
      "completed": ["fix-01"],
      "current": "fix-02",
      "next_expected": "plan.complete"
    }
  }
  ```
- `next_expected` 只在能确定时填充（`current` 是最后一个 fix-unit → `plan.complete`，否则 → `work.ready(fix-{NN+1})`）。
- 生成逻辑应只依赖 `TaskStore` / `tasks.jsonl` 的结构化数据，不解析任何 markdown。

**Patterns to follow:**
- `prepend_orchestrator_context` 现有注入 `RuntimeStateSnapshot` 的方式。
- `TaskStore` 读取 `tasks.jsonl` 的现有模式。

**Test scenarios:**
- Happy path: tasks.jsonl 有 fix-01/02，fix-01 已完成，current=fix-02 是最后一个 → `next_expected=plan.complete`。
- Happy path: tasks.jsonl 有 fix-01/02/03，current=fix-02 → `next_expected=work.ready(fix-03)`。
- Edge case: tasks.jsonl 无 fix-unit 任务 → `fix_unit_state` 为空或不存在。
- Edge case: 当前 step 不是 fix-unit → `fix_unit_state` 仍存在但 `current=null`。
- Integration: `build_prompt` 为 coordinator 生成的 prompt 包含新的 `fix_unit_state` 字段。

**Verification:**
- 新增 prompt 相关单元测试通过。
- 现有 isolated 模式 prompt 测试保持通过。

---

- [ ] U4. **改写 ce-executor-serial preset 的 coordinator 指令**

**Goal:** 删除「数 fix-plan 标题」的步骤，让 coordinator 按 `tasks.jsonl` + commit footer + prompt 中的 `fix_unit_state` 判断终态。

**Requirements:** R10, R11

**Dependencies:** U3（U3 提供 `fix_unit_state` 后，preset 才能引用）

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`
- Test: `crates/ralph-cli/src/presets.rs`（SSOT byte-equality）、`crates/ralph-core/src/preset_lint/`

**Approach:**
- 定位 fix-unit 推进指令段落（约第 817–892 行）。
- 删除「Parse the `## Implementation Units` section … Count every `### U{N}.` heading」步骤。
- 新增指令：
  - 读取 prompt 中 `## ORCHESTRATOR CONTEXT` 的 `fix_unit_state`。
  - 若 `next_expected=plan.complete`，直接发射 `plan.complete`。
  - 若 `next_expected=work.ready(fix-{NN+1})`，发射对应 `work.ready`。
  - 作为交叉验证，可查看最近 commit message footer 的 `[fix-unit: fix-NN]`。
- 保留「EMIT EXACTLY ONE EVENT THIS TURN」的 HARD RULE。
- 保留 `task_id` 必须用 `Task::fix_unit_task_id`、禁止手写时间戳的约束。

**Patterns to follow:**
- AGENTS.md 中 preset SSOT 4/5 处同步规则。
- 现有 `## ORCHESTRATOR CONTEXT` 引用风格。

**Test scenarios:**
- 正常路径：schema 收紧/改写后 `preset_lint` 仍通过。
- 集成：SSOT byte-equality 测试通过。
- 集成：BDD 场景回放最后一个 fix-unit 终态序列，`plan.complete` 被接纳。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
- `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`
- `cargo nextest run -p ralph-core -- preset_lint`

---

- [ ] U5. **同步更新 AI skill 文档**

**Goal:** 若 `crates/ralph-core/data/ralph-tools*.md` 中涉及 fix-unit 工作流、commit 约定或 coordinator 判断逻辑，保持与代码一致。

**Requirements:** R11

**Dependencies:** U4

**Files：**
- 检查并修改：`crates/ralph-core/data/ralph-tools*.md`
- 同步符号链接：`.claude/skills/ralph-tools/SKILL.md`

**Approach:**
- 搜索这些文档中提及 fix-unit 推进、commit message、`plan.complete` 发射的内容。
- 若提到「数 plan 标题」或旧 U6 `orchestrator_state.expected_event`，替换为新方案。
- 更新源码引用行号（若文档中有 `xxx.rs:NN-MM` 形式引用）。

**Test scenarios:**
- 运行 `scripts/check-cli-doc-drift.sh` 做静态 drift 扫描。
- 若文档涉及命令，跑对应命令做冒烟测试。

**Verification:**
- `scripts/check-cli-doc-drift.sh` 通过。

---

- [ ] U6. **新增/更新 BDD 场景覆盖 fix-unit 终态路径**

**Goal:** 用真实 EventLoop runner 验证最后一个 fix-unit 后 `plan.complete` 被正确接纳，stray `work.ready` 被丢弃或改写。

**Requirements:** R7, R8, R9

**Dependencies:** U3, U4

**Files:**
- 新增/修改：`crates/ralph-core/tests/scenarios/*.yml`
- 修改：`crates/ralph-core/tests/scenarios.rs`

**Approach：**
- 新增场景：`fix_unit_last_emits_plan_complete`。
  - mock responses 模拟 executor 完成 fix-02 并发射 `test.passed(fix-02)`。
  - 断言 ledger 中最终只有一笔 `plan.complete`，无重复 `work.ready`。
- 新增场景：`fix_unit_stray_work_ready_dropped`。
  - 同一 activation 中 coordinator 先发射 `work.ready(fix-02, last_in_phase=true)` 再发射 `plan.complete`。
  - 断言 U3 改写 + U1 预算保证只有 `plan.complete`。
- 新增场景：`tasks_jsonl_drives_next_expected`。
  - 验证 prompt 中的 `fix_unit_state.next_expected` 与 tasks.jsonl 一致。

**Patterns to follow:**
- AGENTS.md：必须用 `run_workflow_guard_scenario`（真 EventLoop runner），禁止用 `run_scenario` stub。
- 现有 scenario YAML 的 `mock_responses` payload 字段 + `expected.events` 列表风格。

**Test scenarios：**
- Happy path: fix-02 是最后一个 → ledger 有 `plan.complete`。
- Error path: coordinator 发射 `work.ready(fix-03)` 而 tasks.jsonl 只有 fix-01/02 → 事件被拒绝。
- Integration: `LOOP_COMPLETE` honor 后任何业务事件被拒绝（回归 U2）。

**Verification:**
- `cargo nextest run -p ralph-core --test scenarios` 通过。

---

- [ ] U7. **全量回归验证**

**Goal:** 确保改动没有破坏现有 preset、isolated 模式、review 流程与文档一致性。

**Requirements:** 全部

**Dependencies:** U1–U6

**Files:**
- 无需新增文件，跑测试与脚本。

**Approach：**
- 按 AGENTS.md 的测试入口规则跑全量。
- 重点跑：
  - `./scripts/run-tests.sh`
  - `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
  - `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`
  - `cargo nextest run -p ralph-core -- preset_lint`
  - `cargo nextest run -p ralph-core --test scenarios`
  - `scripts/check-cli-doc-drift.sh`

**Verification:**
- 所有上述命令通过。

---

## System-Wide Impact

- **Interaction graph：** `GitEvidenceProvider` trait 新增方法，所有实现者（production + test mocks）都需要更新。`## ORCHESTRATOR CONTEXT` 块新增 `fix_unit_state` 字段，所有依赖该块解析的 preset/agent 都会看到新字段（向后兼容）。
- **Error propagation：** `FixUnitTagMissing` finding 走诊断通道，不进入 rejection 列表，因此不会触发 `PayloadContractViolation` 终止；但需要确保诊断事件被正确写入 ledger 以便审计。
- **State lifecycle risks：** `recent_commit_messages` 读取的是 git 历史，不会修改工作区；`tasks.jsonl` 只读，不改变 projector 的单一写入者规则。
- **API surface parity：** 无 CLI/API 变更。`## ORCHESTRATOR CONTEXT` 是只读提示块。
- **Integration coverage：** BDD 场景必须走真实 EventLoop，验证 prompt 注入 → coordinator emit → isolated 预算 → stage 改写 的完整链路。
- **Unchanged invariants：**
  - `review_step_state::prefill_fix_steps_from_plan` 仍按 fix-plan 标题预填 tracker，用于 plan_gate 豁免。
  - base runtime 不解析业务 markdown 的硬规则保持。
  - `tasks.jsonl` 仍由 projector 单一写入。

---

## Risks & Dependencies

| 风险 | 缓解措施 |
|------|---------|
| coordinator 忽略 `fix_unit_state` 仍按旧习惯数标题 | U3 注入 + U1/U3 机制兜底；preset 删除旧指令 |
| executor 忘记加 `[fix-unit: fix-NN]` footer | runtime 软校验发诊断；prompt 明确要求 |
| `tasks.jsonl` 中 fix-unit 任务数量与实际不一致 | 该文件由 projector 在 `review.complete(fix_plan_file)` 时预填，已有机制保证 |
| `recent_commit_messages` 在大型仓库中慢 | 限制 `max_count=10` 且用 `since_sha..HEAD` 范围 |
| 测试 mock 需要同步更新 | U1 中同步新增 trait 方法并提供默认空实现，减少破坏 |

---

## Documentation / Operational Notes

- 更新 `AGENTS.md` / `CLAUDE.md` 中 `ce-executor-serial` 的 preset 描述（如 builtin preset 列表有变）。
- 若新增 reason code 或诊断事件，更新相关 `docs/solutions/` 条目或新增一条简短记录。
- 不需要 rollout 或 feature flag；本计划为 `ce-executor-serial` preset 的指令与机制修复。

---

## Sources & References

- **Origin document:** `docs/brainstorms/2026-07-01-ce-executor-serial-fix-unit-terminal-guidance-requirements.md`
- **Related plan:** `docs/plans/2026-07-01-001-fix-ce-executor-serial-p0-terminal-storm-plan.md`
- **Rollback lesson:** `docs/solutions/logic-errors/base-runtime-must-not-parse-business-markdown.md`
- **Related code:** `crates/ralph-core/src/execution_contract.rs`、`crates/ralph-core/src/event_loop/mod.rs`、`crates/ralph-core/src/event_loop/stages/coordinator_decision_gate_stage.rs`、`crates/ralph-core/src/task_store.rs`、`presets/en/ce-executor-serial.yml`
