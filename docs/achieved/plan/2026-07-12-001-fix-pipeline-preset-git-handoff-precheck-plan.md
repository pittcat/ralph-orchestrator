---
title: fix: Harden pipeline preset Git handoff prechecks
type: fix
status: active
date: 2026-07-12
origin: docs/report/2026-07-11-ce-executor-pipeline-primary-20260710-234537-diagnosis.md
---

# fix: 加固 pipeline preset 的 Git 交接检查

## Overview

`ce-executor-pipeline` 与 `ce-executor-pipeline-loop` 都运行在 isolated mode。每个 hat activation 是独立进程，下游只能依赖 Git 状态和事件 payload 判断上游交接是否完整。当前 preset 虽然已经零散要求 executor / fixer 提交并清理工作区，但没有一套统一、紧邻 emit 的交接协议；只读 reviewer 也没有统一的入口/出口 Git 检查来证明“脏状态在进入前已存在”或“本轮 reviewer 没有改文件”。

本计划仅从 preset 层加固流程，不修改 runtime 审计逻辑，不增加自动回滚，不新增 hat：

1. executor 和 fixer 在最终业务事件前执行同一套 Git handoff precheck；凡本 activation 产生且需要保留的修改必须提交，无法安全归属或无法形成有效提交时不得成功交接。
2. 每个只读 reviewer 在读取/评审前验证上游 SHA 与工作区状态，保存本 activation 的起始 Git 状态；在 emit 前再次验证 HEAD 未变且工作区仍然干净。
3. reviewer 不替上游提交、不执行 `git restore`、不清理未知脏状态；写入责任留在 executor / fixer。

## Problem Frame

诊断报告中的终止由 `audit_file_modifications` 检测到工作区相对 `HEAD` 有 24 行删除触发，但缺少 activation 级证据，无法证明修改来自当前 reviewer。Preset 层能做的最小稳定修复不是改变 runtime 的判断，而是建立明确的 Git 交接纪律：

- 写入型 hat 离开时必须把它产生的修改变成可追踪 commit，并交付 clean worktree。
- 只读型 hat 进入时必须先验收 clean handoff，并记录进入时的 HEAD；离开时证明 HEAD 和 worktree 未变化。
- loop preset 的第 2+ 轮 review 必须以 fixer 交付的 `head_sha` / `round_base_sha` 为当前轮基线，不能继续把初始 `executor_head_sha` 当作当前 HEAD。

这会显著减少“上游遗留 dirty 被下游 reviewer 命中”的事故，并让日志/评审产物能够说明责任边界，但不声称消除 runtime 全局 `git diff HEAD` 的固有误判能力。

## Requirements Trace

- **R1. 写入型 hat 统一 clean handoff：** 两个 preset 的 executor、fixer 在最终 emit 前必须提交本 activation 需要保留的所有非 `.ralph/` 修改，并确认工作区干净。
- **R2. 提交责任不跨 hat：** reviewer 不得提交、恢复或清理进入 activation 前已经存在的修改；无法通过入口检查时停止正常评审并记录交接失败证据。
- **R3. 入口 precheck：** 每个只读 dimension reviewer 和 alignment 在开始评审前验证当前 HEAD 等于 trigger 所声明的当前交接 SHA，并验证 `.ralph/` 之外无 tracked、staged 或 untracked 修改。
- **R4. 保存本轮状态：** 只读 hat 将起始 HEAD 与规范化的工作区状态写入 `.ralph/review/...` 下的本轮证据文件，作为本 activation 的可见基线。
- **R5. 出口 precheck：** 只读 hat 在 policy-check 和真实 emit 前确认 HEAD 仍等于起始 HEAD，且 `.ralph/` 之外仍然干净；不满足时不得宣称正常评审完成。
- **R6. 两阶段 emit 防竞态：** executor、fixer、reviewer 都必须在 `--policy-check` 前检查一次 Git 状态，在真实 emit 前再检查一次 HEAD/clean 状态，避免 precheck 之后又产生修改。
- **R7. loop 当前轮 SHA 正确：** `ce-executor-pipeline-loop` 第一轮 reviewer 使用 `round_base_sha == executor_head_sha`；fixer 后的第 2+ 轮使用 `review-reentry` 从 `fix.done.head_sha` 生成的 `round_base_sha`。
- **R8. 失败不能伪成功：** 写入型 hat 无法安全提交或清理自己产生的修改时，不得发成功语义；只读 hat 入口/出口失败时不得继续生成正常的“已完成评审”结论。
- **R9. 不引入脆弱文案测试：** 只验证 YAML 可解析、schema/required fields、事件拓扑和真实场景，不用大段字符串断言锁定 instructions 文案。

## Scope Boundaries

### In Scope

- `presets/en/ce-executor-pipeline.yml`
- `presets/en/ce-executor-pipeline-loop.yml`
- `presets/schemas/ce-executor-pipeline-loop.yml`，仅当结构化事件字段需要同步时修改
- 两套 preset 相关 BDD scenario payload、preset lint 和 embedded preset 校验
- preset author/review operator skills 中与 Git handoff、只读 hat 可见性有关的通用规则

### Out of Scope

- 不修改 `crates/ralph-core/src/event_loop/mod.rs` 的 `audit_file_modifications`。
- 不实现 activation snapshot、自动 rollback、临时 worktree 或 retry 状态机。
- 不增加 handoff-cleaner / workspace-steward hat。
- 不让 reviewer 运行 `git add`、`git commit`、`git restore`、`git reset` 或 `git stash`。
- 不把 `.ralph/` 运行产物提交到 Git。
- 不改变现有 review/fix topic 拓扑、6 轮上限或单业务事件预算。
- 不新增只验证 preset prompt 包含固定句子的测试。

## Context & Existing Contracts

### `ce-executor-pipeline`

- executor 已要求 per-U commit、`work.done` 前 clean、`executor_head_sha = git rev-parse HEAD`，但这些规则分散在 HARD RULES、Commit Cadence 和 Emit-Readiness 中。
- fixer 已要求 per-fix-Unit commit 和 `fix.done` 前 clean，但普通版 `fix.done` 没有 loop 版同等级的 `head_sha` / `worktree_status` 结构化交接字段。
- 六个 dimension reviewer 与 alignment 均为只读角色；它们会写 `.ralph/review/...` 产物，但不得修改项目文件。

### `ce-executor-pipeline-loop`

- executor 后由 `review.round.ready` 建立第 1 轮 review 上下文。
- fixer 的 `fix.done` 已携带 `fixed_from_sha`、`head_sha`、`fix_attempt_commit_sha` 和 `worktree_status: clean`。
- `review-reentry` 应把最新 `fix.done.head_sha` 变成下一轮 `round_base_sha`；第 2+ 轮 reviewer 应校验这个当前轮 SHA，而不是初始 executor SHA。
- 六个 dimension reviewer 与 alignment 均声明 `disallowed_tools: ["Edit"]`，但 Write 仍用于 `.ralph/review/...` 评审产物，因此不能简单禁用所有 Write。

### Institutional Learning

- `docs/solutions/tooling-decisions/claude-disallowed-tools-edit-write-dimension-reviewer.md` 已证明只禁 Edit 挡不住 Write，而同时禁 Edit/Write 会破坏合法 findings 输出；本计划不把 adapter 工具权限当作 preset-only 修复手段。
- `docs/report/2026-07-11-ce-executor-pipeline-primary-20260710-234537-diagnosis.md` 证明全局 dirty 状态缺少 activation 归因；本计划通过 clean handoff + reviewer start/end proof 缩小不确定性，但不宣称修复 runtime 根因。

## Key Technical Decisions

### D1. 写入型 hat 负责提交，只读 hat 只负责验收

executor / fixer 是唯一能判断本 activation 产物是否应保留、是否通过测试、应该使用什么 commit subject 的角色。Reviewer 看到脏状态时不得替上游提交，否则会把未知修改合法化，并使事件中的 SHA 与实际评审范围漂移。

### D2. clean 的定义必须覆盖 tracked、staged 和 untracked

不能只运行默认 `git diff`。Preset instructions 应要求使用统一的 porcelain 检查，并排除 `.ralph/` runtime 产物。实现阶段应选择一个跨仓库可执行、不会把 `.ralph/` 合法产物误判为项目修改的命令形态，同时覆盖：

- staged 修改
- unstaged 修改
- tracked 删除/rename
- untracked 文件

### D3. 起始状态以“预期 SHA + clean worktree”为最小快照

因为 reviewer 入口必须 clean，所以无需在 preset 层复制整个仓库内容。保存以下证据足以判断 reviewer 是否保持只读：

- `expected_head_sha`
- `actual_start_head_sha`
- 规范化入口 status 输出
- activation/round/dimension 标识

出口重新采集 HEAD/status 并与入口比较。证据写入 `.ralph/review/...`，不进入 Git diff。

### D4. 两个 preset 使用各自真实的当前交接 SHA

- 普通 pipeline 第一轮 dimension review：`expected_head_sha = work.done.executor_head_sha`。
- 普通 pipeline alignment：若 fixer 有提交，应使用 fixer 最终 HEAD；计划优先补齐普通版 `fix.done.head_sha` / `worktree_status`，避免 alignment 只能猜当前 HEAD。
- loop 第 1 轮：`expected_head_sha = review.round.ready.round_base_sha = executor_head_sha`。
- loop 第 2+ 轮：`expected_head_sha = review.round.ready.round_base_sha = latest fix.done.head_sha`。
- 同一轮后续 dimension reviewer 继续透传同一 `round_base_sha`，不得用自己读取到的任意 HEAD 覆盖事件基线。

### D5. 不新增 recovery topic；失败沿现有终态表达

Preset-only 方案不新增恢复状态机。写入型 hat 如果无法形成 clean、可验证的交接：

- executor 发 `work.failed`，并说明 dirty paths / commit failure。
- fixer 按现有 preset 语义提交可保留的 attempt，或安全撤销自己本轮无用修改；随后以 `fix.done{fix_status: partial|blocked, worktree_status: clean}` 交接。若连 clean 都无法保证，不得伪造 `worktree_status: clean`。

只读 reviewer 入口失败时记录 handoff violation，停止正常评审。实现阶段必须在不新增 topic 的前提下确认其唯一允许事件如何表达失败且不伪装成正常结论；若现有 schema 无法无歧义表达，则只在计划评审后选择最小结构化字段扩展，而不是让 reviewer 提交上游修改。

## High-Level Flow

```text
executor / fixer 开始写入工作
  -> 每个 Unit 测试通过后提交
  -> 最终验证
  -> Handoff Precheck A: 检查非 .ralph 修改
       -> 有本 hat 的可保留修改: 复核、测试、提交
       -> 有未知/不安全修改: 不得成功交接
  -> 计算 final HEAD 和事件指标
  -> ralph emit --policy-check
  -> Handoff Precheck B: HEAD 未变且仍 clean
  -> 真正 emit

只读 reviewer 被触发
  -> Entry Precheck: actual HEAD == expected handoff SHA
  -> Entry Precheck: 非 .ralph 工作区 clean
  -> 保存 start Git evidence 到 .ralph/review/...
  -> 只读评审，findings 只写 .ralph/review/...
  -> Exit Precheck A: HEAD == start HEAD 且仍 clean
  -> ralph emit --policy-check
  -> Exit Precheck B: HEAD == start HEAD 且仍 clean
  -> 真正 emit
```

## Implementation Units

- [ ] **U1. 定义并复用 Git handoff 协议文本**

**Goal:** 在两个 preset 中形成含义一致的写入型交接协议和只读型验收协议，避免 executor、fixer、六个 reviewer 各写一套互相漂移的规则。

**Requirements:** R1, R2, R3, R4, R5, R6, R8

**Dependencies:** None

**Files:**
- Modify: `presets/en/ce-executor-pipeline.yml`
- Modify: `presets/en/ce-executor-pipeline-loop.yml`

**Approach:**
- 为写入型 hat 定义同一逻辑顺序：完成 Unit commits → 最终验证 → clean check → 计算 final SHA/metrics → policy-check → 二次 clean/SHA check → emit。
- 明确 `.ralph/` 是 runtime 输出区，不提交；其他路径的修改必须明确归属并提交，否则不得成功交接。
- 为只读型 hat 定义同一逻辑顺序：验证 expected SHA → clean check → 保存 start evidence → review → end evidence → policy-check → 二次检查 → emit。
- 明确 reviewer 禁止替上游提交、恢复、stash 或 reset。
- 引用现有 `ralph-tools-emit` / `ralph-tools-opac`，不复制通用 policy-check 说明。

**Test scenarios:**
- Test expectation: none -- 本单元只统一 agent 可执行流程；禁止新增大段 instructions 文案断言。

**Verification:**
- 人工对照两套 preset，确认同类 hat 的检查顺序与失败原则一致。
- 确认所有命令/字段均是该 isolated hat 可直接读取或调用的，不依赖内部 ledger。

- [ ] **U2. 加固两个 executor 的最终交接门**

**Goal:** 保证 `work.done` 只在 executor 的全部可保留修改已提交、HEAD 已固定、工作区 clean 时发出。

**Requirements:** R1, R6, R8

**Dependencies:** U1

**Files:**
- Modify: `presets/en/ce-executor-pipeline.yml`
- Modify: `presets/en/ce-executor-pipeline-loop.yml`

**Approach:**
- 将现有分散的 per-U commit、clean、SHA、policy-check 要求收敛到紧邻 emit 的 `Final Git Handoff Precheck`。
- 明确跳过执行的 fast path 也必须通过 clean/HEAD 检查，不能因为“没有新 Unit”跳过交接门。
- 第一次 clean check 后才计算 `executor_head_sha`、`commit_count`、`changed_lines`。
- policy-check 通过后再次确认 HEAD 等于刚计算的 `executor_head_sha` 且工作区 clean，再真实 emit。
- 若发现属于 executor 的遗漏修改，回到复核/测试/提交步骤，并重新计算所有 Git 派生字段；不能沿用旧 payload。
- 若发现无法确认归属的修改，发 `work.failed`，不得把未知修改打包进 executor commit。

**Test scenarios:**
- Happy path: 所有 U-ID 已提交、工作区 clean，`executor_head_sha` 等于真实 HEAD，允许 `work.done`。
- Recovery path: 最终检查发现 executor 遗漏文件，补充复核/测试/commit 后重新计算 payload，再允许 `work.done`。
- Error path: 存在无法安全归属的非 `.ralph/` 修改，禁止 `work.done`，走 `work.failed`。
- Error path: policy-check 后 HEAD 或 status 变化，必须重新执行最终交接门，不能直接 emit。
- Fast path: flow-audit skip 仍需 clean/HEAD 双检查。

**Verification:**
- 不新增事件 topic。
- `work.done.executor_head_sha` 的现有 required field 保持 SSOT 一致。

- [ ] **U3. 加固两个 fixer 的最终交接门**

**Goal:** 保证 fixer 无论 applied、partial、blocked 或 empty-plan fast path，都以 commit + clean worktree 完成交接。

**Requirements:** R1, R6, R8

**Dependencies:** U1

**Files:**
- Modify: `presets/en/ce-executor-pipeline.yml`
- Modify: `presets/en/ce-executor-pipeline-loop.yml`
- Modify: `presets/schemas/ce-executor-pipeline-loop.yml` only if existing structured field descriptions or required fields need synchronization

**Approach:**
- 普通版与 loop 版 fixer 均执行与 executor 对称的 final handoff：Unit/attempt commits → verification → clean → final HEAD → policy-check → 二次 clean/SHA → `fix.done`。
- 有用的 partial attempt 必须形成 `fix-attempt(U<N>)` commit；无用修改只能由 fixer 撤销自己本 activation 产生的变化，不能清理进入前未知修改。
- empty-plan fast path 不创建 commit，但必须证明当前 HEAD 未被 fixer 改变且工作区 clean。
- loop 版继续使用 `head_sha`、`fix_attempt_commit_sha`、`worktree_status`，强化它们与真实 Git 状态的一致性。
- 普通版评估为 `fix.done` 增加最小的 `head_sha` 与 `worktree_status` 字段，使 alignment 能验收 fixer 的真实交接 SHA；若增加，则同步 inline schema、相关 emit 示例和下游 passthrough。

**Test scenarios:**
- Happy path: 全部 fix Units 各自提交，`fix.done.head_sha` 等于真实 HEAD，worktree clean。
- Partial path: 有用的未完成 attempt 已提交，`fix_status=partial`，仍然 clean 交接。
- Blocked path: 无安全修改可保留，恢复 fixer 本轮变化，HEAD 不变且 clean，明确 blocked reason。
- Error path: 存在无法安全归属的 dirty 文件，不得伪造 `worktree_status=clean`。
- Fast path: 无修复 Unit、不创建 commit，HEAD 保持不变且 clean。
- Race path: policy-check 后状态变化，必须重新计算 `head_sha` 和 payload。

**Verification:**
- loop schema 的 `head_sha` / `worktree_status` source 与 instructions 一致。
- 普通版若新增字段，下游 alignment/reporter 不再从当前工作区猜 fixer final HEAD。

- [ ] **U4. 为普通 pipeline 的只读 reviewers 增加入口/出口证明**

**Goal:** 六个 dimension reviewer 和 alignment 在 activation 内证明只读，并尽早发现不完整交接。

**Requirements:** R2, R3, R4, R5, R6, R8

**Dependencies:** U1, U2, U3

**Files:**
- Modify: `presets/en/ce-executor-pipeline.yml`

**Approach:**
- `dim:goal-alignment` 从 `work.done.executor_head_sha` 取得 expected HEAD。
- 后续五个 dimension reviewer 从上一维事件继续透传并使用同一 `executor_head_sha`。
- 每个 reviewer 在入口保存 dimension-specific Git evidence 到 `.ralph/review/{plan_name}/git-state-<dimension>-start.*`；文件需位于 `.ralph/`，不得污染 Git 状态。
- 每个 reviewer 在 policy-check 前和真实 emit 前检查 HEAD/status 未变化。
- alignment 若普通版 fixer 有最终 `head_sha`，使用 fixer SHA；无 fix commit 的 fast path使用不变 HEAD。不得只依赖初始 executor SHA 判断 fixer 后的当前 HEAD。
- 入口失败时记录明确的 `handoff_precheck_failed` finding/证据，不执行正常维度分析；不得修复或提交上游状态。实现阶段确认现有 topic/schema 能否无歧义承载该失败，必要时采用最小字段扩展。

**Test scenarios:**
- Happy path: HEAD 等于 expected SHA、工作区 clean，reviewer 完成且出口状态不变。
- Upstream failure: activation 一开始已有非 `.ralph/` dirty，reviewer记录交接失败，不修改、不提交。
- SHA mismatch: 当前 HEAD 不等于 trigger SHA，reviewer记录交接失败，不以错误 diff range开展评审。
- Reviewer violation: 入口 clean，出口出现项目文件修改或 HEAD 变化，禁止正常完成语义。
- Allowed output: `.ralph/review/...` 新增 findings/evidence 不算项目 dirty。

**Verification:**
- 所有 dimension 事件继续透传 `executor_head_sha`。
- alignment 使用的当前交接 SHA 与 fixer 实际 final HEAD 一致。

- [ ] **U5. 为 loop pipeline 的每轮只读 reviewers 增加轮次感知检查**

**Goal:** reviewer 在首轮和 fixer 后重审时都校验正确的当前轮基线，不把初始 executor HEAD 误当作永远不变的 HEAD。

**Requirements:** R2, R3, R4, R5, R6, R7, R8

**Dependencies:** U1, U2, U3

**Files:**
- Modify: `presets/en/ce-executor-pipeline-loop.yml`
- Modify: `presets/schemas/ce-executor-pipeline-loop.yml` only if field documentation/required fields require synchronization

**Approach:**
- `review-reentry` 明确建立：首轮 `round_base_sha=executor_head_sha`；fix 后 `round_base_sha=fix.done.head_sha`。
- 六个 dimension reviewer 入口都校验 `actual HEAD == round_base_sha`，而不是第 2+ 轮仍校验初始 `executor_head_sha`。
- 起始 evidence 路径带 `round-<NN>` 和 dimension，避免多轮覆盖。
- 同轮后续 reviewer透传完全相同的 `round_base_sha`。
- alignment 使用最终 accepted round 对应的 current HEAD；如发生 fix round，必须与最新 `fix.done.head_sha` 一致。
- 入口/出口失败原则与普通 pipeline 相同：记录、停止正常评审、不提交、不恢复。

**Test scenarios:**
- Round 1: `round_base_sha == executor_head_sha == HEAD`，允许评审。
- Round 2: `round_base_sha == latest fix.done.head_sha == HEAD`，即使不同于初始 executor SHA，也允许评审。
- Error path: Round 2 reviewer错误使用初始 executor SHA 应被结构化场景/契约发现。
- Error path: `review-reentry` 未把 fixer `head_sha` 传入下一轮，禁止正常 review round。
- Reviewer violation: 任一轮入口 clean、出口 dirty，禁止正常完成语义。

**Verification:**
- `review.round.ready`、六个 dimension events、`review.synthesized` 的 round/base 字段保持 schema parity。
- 不改变现有 fix/re-review 拓扑和最大轮数。

- [ ] **U6. 同步结构化契约与真实场景**

**Goal:** 用结构化 schema 和真实 EventLoop 场景保护 handoff SHA 的传递，不锁死 prompt 文案。

**Requirements:** R7, R8, R9

**Dependencies:** U2, U3, U4, U5

**Files:**
- Modify: `presets/schemas/ce-executor-pipeline-loop.yml` as required
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_pipeline.yml` as required
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_blocked.yml` as required
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop.yml` as required
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_fix_reentry.yml` as required
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_pipeline_loop_max_round_blocked.yml` only if affected payloads change
- Modify: `crates/ralph-core/tests/scenarios.rs` only if a new behavior scenario is genuinely needed
- Modify: `crates/ralph-cli/src/presets.rs` only for structured assertions required by changed fields; do not add prompt text assertions

**Approach:**
- 普通 pipeline 若新增 `fix.done.head_sha` / `worktree_status`，同步 inline schema、scenario payload 与下游 required fields。
- loop schema 保持 `fix.done.head_sha` → `review.round.ready.round_base_sha` 的来源说明和 required-field parity。
- 优先扩展现有 fix-reentry BDD，断言第 2 轮事件携带 fixer HEAD；不新增仅检查 YAML/instructions 文本的测试。
- BDD 必须继续使用真实 `run_workflow_guard_scenario` 路径并断言事件。
- 失败语义若增加最小字段，测试字段/事件结果，不测试具体提示句子。

**Test scenarios:**
- 普通 pipeline：executor HEAD 贯穿六维 review；fixer final HEAD 可供 alignment 验收。
- Loop no-fix：首轮 base 等于 executor HEAD，直接 accepted。
- Loop fix-reentry：fixer `head_sha=B`，第 2 轮 `round_base_sha=B`，六维事件继续透传 B。
- Blocked handoff：写入型 hat 不满足 clean contract 时不得出现成功语义事件。

**Verification:**
- `cargo nextest run -p ralph-core --test scenarios -- ce_executor_pipeline`
- `cargo nextest run -p ralph-core --test scenarios -- ce_executor_pipeline_loop`
- `cargo nextest run -p ralph-core --test scenarios -- ce_executor_pipeline_loop_fix_reentry`

- [ ] **U7. 同步 preset operator skills 与 agent skill guides 审计**

**Goal:** 让 preset 作者/评审者知道“写入 hat clean handoff、只读 hat start/end proof”的通用规则，并确认注入 skill 无漂移。

**Requirements:** R1, R2, R3, R6, R9

**Dependencies:** U1, U3, U5

**Files:**
- Modify: `skills/ralph-preset-common/references/agent-native-model.md` if handoff visibility needs clarification
- Modify: `skills/ralph-preset-common/references/author-checklist.md`
- Modify: `skills/ralph-preset-common/references/patterns.md`
- Modify: `skills/ralph-preset-common/references/finding-rubric.md` only if an existing finding mapping is affected
- Review: `skills/ralph-preset-common/references/commands.md`
- Review: `crates/ralph-core/data/ralph-tools.md`
- Review: `crates/ralph-core/data/ralph-tools-emit.md`
- Review: `crates/ralph-core/data/ralph-tools-opac.md`

**Approach:**
- operator checklist 增加通用审查项：写入型 hat 是否 clean + commit 后交接；只读型 hat 是否校验 trigger SHA、保存 start evidence、出口证明未改变。
- 强调 reviewer 不得替上游提交/回滚。
- 本计划不新增 CLI 命令、不改变 `ralph emit` 行为，预计无需修改 `crates/ralph-core/data/*.md`；实现完成后必须反向核对。如果 preset instructions 引用的命令或 policy-check 规则发生变化，再同步对应 guide。
- 不在注入 skill 中写入具体 preset 名、事故路径或本计划编号。

**Test scenarios:**
- Test expectation: none -- operator/agent guide 同步通过人工审计和 drift 脚本验证。

**Verification:**
- `skills/ralph-preset-common/references/commands.md` 与实际 CLI 帮助一致。
- `scripts/check-cli-doc-drift.sh` 通过。

- [ ] **U8. Preset lint、schema parity 与全量验收**

**Goal:** 证明两套 builtin preset 仍可解析、严格 lint、schema parity 和真实链路均通过。

**Requirements:** R9

**Dependencies:** U1-U7

**Files:**
- Modify: none expected

**Approach:**
- 先运行 targeted preset lint 和 BDD。
- 再运行 embedded preset parity tests。
- 最终按仓库硬规则运行全 workspace 脚本。
- 不使用裸 `cargo test -p ralph-cli`。

**Test scenarios:**
- 两个 preset 均通过严格 lint。
- loop schema 与 preset inline schema 一致。
- 普通 pipeline、blocked branch、loop accept、loop fix-reentry、max-round blocked 场景通过。
- 全 workspace nextest + doctest 通过。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
- `cargo nextest run -p ralph-core -- preset_lint`
- `cargo nextest run -p ralph-cli --bin ralph -- presets`
- `cargo nextest run -p ralph-core --test scenarios -- ce_executor_pipeline`
- `cargo nextest run -p ralph-core --test scenarios -- ce_executor_pipeline_loop`
- `scripts/check-cli-doc-drift.sh`
- `./scripts/run-tests.sh`

## System-Wide Impact

- **Interaction graph:** 不新增 topic 或 hat；仅强化写入 hat 的 egress 和只读 hat 的 ingress/egress。
- **Git history:** executor/fixer 更严格地按 Unit/attempt 形成 commit；reviewer 不产生 commit。
- **Event payload:** loop 版继续使用现有 `head_sha` / `round_base_sha`；普通版可能最小增加 fixer final `head_sha` / `worktree_status`，以消除 alignment 猜测。
- **Failure propagation:** executor 无法 clean 时走 `work.failed`；fixer用现有 partial/blocked 语义；reviewer 不修复上游 dirty。
- **Observability:** `.ralph/review/...` 增加 start/end Git evidence，便于区分入口已脏和 reviewer 本轮引入变化。
- **Runtime behavior:** `audit_file_modifications` 完全不变，因此其全局 dirty 误判能力仍是已知残余风险。

## Risks And Mitigations

| Risk | Impact | Mitigation |
|---|---|---|
| 仅靠 prompt，agent 仍可能跳过检查 | 仍可能触发硬拒 | 将检查集中在 emit 紧邻位置，执行两次，并让事件 SHA/status 字段与实际命令输出绑定 |
| reviewer 入口发现 dirty 但 runtime 仍会全局审计终止 | preset-only 无法恢复该 run | reviewer 不扩大损害；留下入口证据。把 runtime activation-aware audit 明确保留为后续独立工作 |
| 自动提交未知修改 | 污染历史、错误归责 | 明确禁止 reviewer 代提交；executor/fixer 只提交能归属并验证的本 activation 修改 |
| `.ralph/` 文件被 status 检查误判 | 合法 findings 阻塞 reviewer | 统一规范化 status 过滤，仅排除 `.ralph/`，不扩大忽略范围 |
| loop 第 2+ 轮继续使用初始 executor SHA | review diff 范围错误 | 以 `round_base_sha` 作为当前轮 expected HEAD，并用 fix-reentry BDD 锁定 `fix.done.head_sha` 传递 |
| 普通版新增 fixer SHA 字段扩大改动 | schema/scenario 漂移 | 只增加 alignment 真正需要的最小字段；同步 inline schema 和结构化场景，不增加文案测试 |
| policy-check 与真实 emit 之间状态变化 | payload SHA 过期 | policy-check 后再次检查 HEAD/status；变化则重新生成 payload 和 precheck |

## Acceptance Criteria

- [ ] 两个 preset 的 executor 都在 `work.done` 前执行统一的双阶段 Git handoff precheck。
- [ ] 两个 preset 的 fixer 都在 `fix.done` 前执行统一的双阶段 Git handoff precheck，包括 partial/blocked/fast path。
- [ ] 所有需要保留的 executor/fixer 非 `.ralph/` 修改在交接前均有 commit；无法安全归属时不成功交接。
- [ ] reviewer 明确禁止替上游提交、恢复、stash 或 reset。
- [ ] 普通 pipeline 六个 dimension reviewer 和 alignment 均执行入口/出口 HEAD + clean 检查。
- [ ] loop pipeline 六个 dimension reviewer 和 alignment 均执行轮次感知的入口/出口检查。
- [ ] loop 第 2+ 轮 expected HEAD 来自最新 `fix.done.head_sha` 经 `round_base_sha` 传递。
- [ ] reviewer 的 start/end Git evidence 只写 `.ralph/review/...`，不污染项目 Git 状态。
- [ ] 没有新增 prompt 文案 byte-equality / substring 锁死测试。
- [ ] preset lint、schema parity、相关 BDD、CLI doc drift 和全量测试通过。

## Residual Risk / Follow-Up

即使本计划全部完成，runtime 仍使用全局 `git diff --stat HEAD` 做事后审计。当 operator 或外部进程在 reviewer activation 前/期间修改共享工作区时，preset 无法从机制上阻止误归因。本计划通过 clean handoff 和 start/end evidence 将正常 pipeline 的风险降到最低，并为后续 activation-aware runtime audit 提供证据，但不把该 runtime 改造混入本次最小修复。

## Recommended Execution Order

1. U1 定义统一协议。
2. U2/U3 先保证写入型 hat 交付 clean 状态。
3. U4/U5 再让只读 hat 验收并证明未修改。
4. U6 同步 schema/BDD，重点锁定 loop fixer HEAD 到下一轮 base 的传递。
5. U7 完成 operator skill 与 agent guide 反向审计。
6. U8 执行 targeted + full verification。
