---
title: "fix: ce-executor-serial runtime protocol stability"
type: fix
status: active
date: 2026-07-07
execution_model: strictly-sequential-atomic-tdd
origin:
  - docs/brainstorms/2026-07-06-ce-executor-serial-protocol-ssot-convergence-requirements.md
  - docs/brainstorms/2026-07-06-ce-executor-serial-handoff-envelope-requirements.md
source_report: docs/report/2026-07-07-ce-executor-serial-primary-20260706-230230-diagnosis.md
related_plans:
  - docs/plans/2026-07-06-001-feat-ce-executor-serial-protocol-ssot-convergence-plan.md
  - docs/plans/2026-07-06-004-feat-ce-executor-serial-handoff-envelope-plan.md
  - docs/plans/2026-07-07-001-fix-ce-executor-serial-handoff-envelope-review-gaps-plan.md
---

# fix: ce-executor-serial runtime protocol stability

## Overview

`ce-executor-serial` 已经具备 Handoff Envelope、policy-check、EmitResult 摘要、phase authority、recovery 等多层机制，但 2026-07-06 23:02:30 的实跑仍出现假闭环：`LOOP_COMPLETE` 后继续写业务事件，`REVIEW_COMPLETE` 早于 `validator test.passed`，`work.done` 先进入 events 后被 execution contract 拒收，tasks ledger 对同一 step 出现双行，coordinator 在 recovery 路径中反复误发 `work.ready`。

本计划不是继续扩展 envelope 字段，也不是重做 `2026-07-06-001` 的 EmitResult/progress-steward 计划。它要把 serial 的现有机制收敛成一个强状态机协议：只有通过全部 gate 的事件才能写入主 events；终态 honored 后业务事件流冻结；shipper 必须等待当前 step 的 validator 终态；task ledger 以 live task identity 幂等；preset instructions 改成 agent 可机械执行的状态表。

## 强制执行模型

本计划必须按 `Unit 1 -> Unit 2 -> ... -> Unit 10` 纯粹串行执行。每个 Unit 是独立孤岛：先写只覆盖本 Unit 输入输出的失败测试，再做最小实现，再只重构本 Unit 边界内代码。前一个 Unit 的代码、测试、文档同步和验收全部完成前，禁止打开后一个 Unit。

禁止事项：

- 禁止并行开发两个 Unit。
- 禁止在当前 Unit 中调用后置 Unit 未来才会提供的接口。
- 禁止用端到端场景替代当前 Unit 的小输入/小 fixture 测试。
- 禁止把当前 Unit 的边界问题留给后续 Unit。
- 禁止在 Unit 1-9 跑全 workspace 基线作为“证明”；全量只属于 Unit 10。
- 禁止裸跑 `cargo test -p ralph-cli` 或 `cargo test -p ralph-cli --bin ralph`。

## Problem Frame

报告中的关键事实显示，serial 不稳的主因不是缺少单个字段或单个 gate，而是多套状态语义没有统一裁判：

- 主 events 记录了 `work.done`，但 execution contract 随后又拒收同一事件。
- `LOOP_COMPLETE` 已被请求/写入后，后续仍出现 `plan.blocked`、`work.done(step-02)` 和第二个 `REVIEW_COMPLETE`。
- shipper 使用 `pass_with_residuals` 兜底提前收尾，但 validator 后续才确认 `step-02` 14/14 通过。
- `.ralph/agent/tasks.jsonl` 中 step-01 和 step-02 都出现两种 task 记录形态，破坏 task identity 单一事实源。
- Handoff Envelope 已在 payload 中存在，但它目前只能说明“agent 认为自己在交接什么”，不能阻止错误顺序、错误下游或错误终态。

## Requirements Trace

- R1. 被 execution contract、event policy、phase authority 或 terminal guard 拒收的业务事件不得写入主 events。
- R2. `LOOP_COMPLETE` honored 后，所有业务事件、terminal-adjacent 事件和 repair-stream 业务写入都必须 fail-closed 或 ignore-with-diagnostic，不得继续推进工作流。
- R3. shipper 不得在当前 step validator 终态之前发出 `REVIEW_COMPLETE`；`pass_with_residuals` 不能作为 validator 缺席时的成功替代。
- R4. tasks ledger 对同一 loop/task_key/step 只能存在一个 live identity；重复 ensure/add 必须幂等返回同一记录或明确拒绝。
- R5. `task.resume` / recovery 路径必须给出确定动作：重发同一 task 的 terminal signal、等待指定 hat、或 fail-close；coordinator instructions 不得让 agent 自由猜测下一步。
- R6. `ce-executor-serial` instructions 必须改成按触发事件索引的状态表，明确每个触发下唯一允许动作、禁止动作、成功/失败 signal 和 envelope 模板引用。
- R7. 变更 preset、runtime 能力或 agent 可见命令语义后，必须同步 `crates/ralph-core/data/*.md`、preset operator skills、`CLAUDE.md`/`AGENTS.md` 中受影响描述。
- R8. 最终回归必须用真实 runtime path 覆盖正常链路、终态后拒写、shipper 等 validator、task ledger 幂等和 missing-envelope/错 phase 拒收。
- R9. agent 违反协议时必须有 bounded recovery：第一次违规返回结构化 correction 和可执行 retry target；第二次同类违规仍失败时升级 fail-close，不得无限 `task.resume` 或 silent-success。
- R10. `crates/ralph-core/data/*.md` 是通用 agent skill guide，更新时必须保持通用语义；不得写入 `ce-executor-serial` 专用拓扑、hat 名称、报告编号、计划编号、一次性诊断术语或 preset 内部注释。serial 专用状态表和拓扑细节只能写在 `presets/en/ce-executor-serial.yml` 与 preset operator skills 中。

## Scope Boundaries

- 只收敛 `builtin:ce-executor-serial` 的不稳链路；其它 preset 只作为非回归对象。
- 不重写 Handoff Envelope v1 的字段形态；已有 nested validator 和 prompt renderer 继续沿用。
- 不实现全新的 Hat Completion API。
- 不删除 events/recovery/diagnostics 审计账本；本计划只规定它们对外不再互相争夺状态权威。
- 不把 SC1×3 金丝雀自动化进 CI；本轮只保留为 Unit 10 的 operator 验收清单。
- 不把 serial 专用词汇扩散进 `crates/ralph-core/data/*.md`；data skill docs 面向所有 agent，只写通用 emit/task/recovery/precheck 行为。

### Deferred to Separate Tasks

- `ralph wave emit` 的 EmitResult parity：另开 wave 专项。
- `ce-executor-supervisor` 的并行 supervisor 终态语义：另开 supervisor 专项。
- 将 tasks ledger 从 JSONL 迁移到 sqlite：本轮先做 JSONL 幂等键和拒收语义。

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/event_loop/mod.rs`：主事件处理、isolated prompt、completion/terminal handling、shipper routing、repair sink 接入点。
- `crates/ralph-core/src/event_loop/loop_state.rs`：`completion_requested`、`completion_honored`、terminal-adjacent dedup、`work_done_seen_tasks`、`last_test_passed_step` 等状态。
- `crates/ralph-core/src/execution_contract.rs`：`TaskNotTerminal` 规则和 `allowed_terminal_statuses=["closed"]`。
- `crates/ralph-core/src/event_loop/rejection.rs`：policy/origin/execution_contract 拒收 envelope 和 `task.resume` enrichment。
- `crates/ralph-core/src/event_loop/phase_authority/shipper_helper.rs`：shipper phase gate 的现有纯函数模式。
- `crates/ralph-core/src/shipper_reason.rs`：`pass_with_residuals`、recoverable plan.blocked reason、stall fail-close 语义。
- `crates/ralph-cli/src/doctor.rs` 和 task CLI 相关模块：`.ralph/agent/tasks.jsonl` 的读写和 plan frontmatter drift 检查。
- `presets/en/ce-executor-serial.yml`：coordinator/executor/validator/shipper/reporter instructions 和 topology。
- `presets/schemas/ce-executor-serial.yml`：serial event schema SSOT。
- `crates/ralph-core/tests/scenarios.rs` 与 `crates/ralph-core/tests/scenarios/*.yml`：真实 workflow guard scenario 模式。

### Institutional Learnings

- `docs/solutions/logic-errors/base-runtime-must-not-parse-business-markdown.md`：runtime 只能消费结构化 payload/state/config，不解析业务 markdown。
- `docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md`：`ralph-cli` 测试必须通过 nextest 串行配置，禁止裸 cargo test。
- `docs/brainstorms/2026-07-06-ce-executor-serial-protocol-ssot-convergence-requirements.md`：agent 可见协议必须收敛到请求 JSON + 统一响应 JSON，失败不能 fail-open。
- `docs/brainstorms/2026-07-06-ce-executor-serial-handoff-envelope-requirements.md`：Handoff Envelope 是交接工作包，不是全文教材；runtime state 优先于 agent narrative。

### External References

- 无。该工作完全位于 Ralph runtime/preset 内部，仓库已有机制和报告比外部资料更权威。

## Key Technical Decisions

- Accepted-event commit boundary 优先于 preset 重写：只要事件能先写进主 events 再被拒收，agent prompt 和诊断都会继续看到矛盾事实。
- 终态冻结是 runtime 责任：`LOOP_COMPLETE` honored 后不能再靠 hat instructions 自觉停止。
- shipper gate 以 validator terminal snapshot 为前置：`plan.blocked` 或 stall recovery 不能绕过 validator 终态直接变成 `pass_with_residuals`。
- task identity 以 `(loop_id, task_key, step)` 为幂等键，`task_id` 是 live record id，不允许同一 key/step 双行分裂。
- preset instructions 只写 agent 可执行状态表，不复述 schema、CLI 语法和 framework 细节；命令语法引用 `ralph-tools-*`。
- 协议违规不靠 agent 自觉修正：runtime 必须把拒收原因、正确 next action、正确 payload identity 和 retry target 写成结构化 correction；同类违规 bounded retry 后 fail-close。
- data skill docs 与 preset instructions 分层：data docs 只讲通用命令、通用 correction/retry 规则和通用字段语义；serial 的具体 trigger、hat 角色、状态表、topic 编排留在 preset。
- 真 runner 场景只放在最后：前面 Unit 用纯函数、小 fixture 或窄 pipeline 测试闭环，避免端到端失败反向污染单元边界。

## Open Questions

### Resolved During Planning

- 是否继续扩展 handoff envelope 字段：否。当前不稳不是字段不足，而是 runtime 没有用状态机语义裁判事件顺序。
- terminal 后事件应 reject 还是 ignore：对业务事件和 terminal-adjacent 事件 fail-closed 并写诊断；对纯 inspect/diagnostic control topic 保持不改变。
- task ledger 本轮是否迁 sqlite：否。先用 JSONL 幂等键稳定语义，避免把存储迁移和 runtime 协议收敛绑在一个计划里。
- agent 违反协议后是否允许 retry：允许，但必须 bounded。第一次同类违规给结构化 correction 和目标 hat；第二次同类违规不再继续猜测，直接 fail-close 并写诊断。

### Deferred to Implementation

- accepted-event commit boundary 的最终 helper 名称：由实现时周边代码决定。
- post-terminal guard 在 pipeline 中的精确 stage 位置：实现时以“早于主 events 写入，晚于能识别 loop terminal state”为准。
- task CLI 具体入口文件名：实现时沿现有 task command/store 模块定位，计划只约束行为和测试。

## High-Level Technical Design

> This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.

### State Authority Order

| 层级 | 权威内容 | 不能做什么 |
|------|----------|------------|
| Runtime accepted state | 哪些事件真正被接受、当前 phase、completion honored、validator terminal、live task identity | 不能让 agent narrative 覆盖 |
| Task API / tasks ledger | live task id、task_key、step、status | 不能由 handoff envelope 自行创造 closed 状态 |
| Handoff Envelope | 当前 activation 的工作包、receiver contract、signal contract | 不能证明事件已完成或覆盖 runtime state |
| Preset instructions | 角色职责、状态表、质量标准、文档引用 | 不能维护第二套路由表 |
| Agent skill docs (`crates/ralph-core/data/*.md`) | 通用 emit/task/recovery/precheck 规则 | 不能承载 serial 专用拓扑、hat 名称或一次性报告术语 |
| Memory / scratchpad | 经验、辅助索引、非权威笔记 | 不能保存当前 step/phase 权威状态 |

### Event Acceptance Shape

事件只有在通过全部相关 gate 后才进入主 events。被拒收事件只进入 rejection/recovery diagnostics，不成为后续 hat 的事实输入。

| 阶段 | 输入 | 输出 |
|------|------|------|
| Parse | agent emit candidate | candidate event |
| Policy/origin/phase/terminal checks | candidate event + runtime state | accept 或 rejection |
| Execution contract | accepted candidate + task state | accept 或 rejection |
| Commit | accepted event | 主 events + state projection update |
| Diagnostics | rejection | recovery/diagnostics record，不写主 events |

### Protocol Violation Recovery Shape

当 agent 违反协议时，runtime 不把错误事件写入主 events，而是生成一条结构化 correction。下一次 activation 只能按 correction 做一件事；如果同一 `(hat, topic, task_key, step, violation_code)` 再次失败，runtime 必须 fail-close。

| 输入违规 | 第一次处理 | 第二次同类处理 |
|----------|------------|----------------|
| `TaskNotTerminal` 的 `work.done` | 给 executor/coordinator correction：同一 task identity 先 close 再补发同一 `work.done` | fail-close，reason 指向 `protocol_violation_repeated:task_not_terminal` |
| duplicate `work.ready` | 给 coordinator correction：不要新建 task，不要重发 ready；等待或路由同一 task terminal signal | fail-close，reason 指向 `protocol_violation_repeated:duplicate_work_ready` |
| wrong step/task_id mismatch | 给 source hat correction：使用 live task identity 重构 payload | fail-close |
| post-terminal business event | 不 retry，直接 reject/ignore-with-diagnostic | 保持 terminal closed |
| shipper before validator terminal | 不让 shipper retry pass；等待 validator 或 fail-close | fail-close |

## Implementation Units

- [ ] **Unit 1: Accepted-event commit boundary helper**

**Goal:** 建立一个窄的纯函数/小模块，用统一结果表示 candidate event 是 accepted、rejected 还是 ignored，确保后续 runtime wiring 有一个明确的“通过后才可写主 events”边界。

**Requirements:** R1

**Dependencies:** None

**Files:**
- Create: `crates/ralph-core/src/event_loop/accepted_event.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/event_loop/accepted_event.rs`

**Approach:**
- 只定义 commit boundary 的数据形状和纯 helper，不接入真实 event loop。
- 输入使用内存里的 candidate topic/payload、rejection reason、accepted metadata。
- 输出必须能区分“可写主 events”和“只写 rejection diagnostics”。
- 不读取 `.ralph/`，不触碰 `presets/`，不调用后续 terminal/shipper/task helper。

**Execution note:** 先写当前 helper 的纯单元测试，测试只验证输入到输出，不启动 EventLoop。

**Patterns to follow:**
- `crates/ralph-core/src/event_loop/rejection.rs` 中的 typed rejection envelope。
- `crates/ralph-core/src/emit_result/assemble.rs` 的小型 assemble/helper 风格。

**Test scenarios:**
- Happy path: 输入 accepted candidate `work.done`，输出标记为 committable，并保留原 topic/payload。
- Error path: 输入 `TaskNotTerminal` rejection，输出标记为 non-committable，并携带 rejection stage/reason。
- Edge case: 输入 ignored duplicate terminal-adjacent event，输出标记为 non-committable 但不是 hard rejection。

**Verification:**
- 本 Unit 测试证明 rejected/ignored event 不会被误分类为 committable。
- 没有生产路径行为变化；现有测试不需要靠本 Unit 新 helper 才能通过。

- [ ] **Unit 2: Execution contract before main events write**

**Goal:** 将 execution contract 拒收结果接入 Unit 1 的 commit boundary，保证 `work.done` 等事件只有在 contract 通过后才写入主 events，消除“events 先写、ledger 后拒”的双账本矛盾。

**Requirements:** R1

**Dependencies:** Unit 1

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-core/src/event_loop/rejection.rs`
- Test: `crates/ralph-core/src/event_loop/tests/execution_contract_commit_boundary.rs`
- Test: `crates/ralph-core/src/tests/execution_contract.rs`

**Approach:**
- 找到当前 `work.done` 从 parse/accepted 到 events write 的路径，把 execution contract 置于主 events commit 前。
- 拒收时只写 rejection/recovery 诊断，不把原业务事件交给 EventBus 或主 JSONL。
- 保留已有 `task.resume` enrichment，但它必须基于 rejected candidate 生成，不得让 rejected candidate 成为 accepted event。
- 不修改 task ledger idempotency；同一 task 双行问题留给 Unit 7。

**Execution note:** 先加 characterization 测试复现 `work.done` task 未 closed 时不应出现在 accepted events，再改 wiring。

**Patterns to follow:**
- `crates/ralph-core/src/tests/execution_contract.rs` 中“rejected work.done must not be accepted”的现有测试意图。
- `crates/ralph-core/src/event_loop/rejection.rs` 的 rejection journal 写法。

**Test scenarios:**
- Error path: `work.done` payload 引用 open task，execution contract 返回 `TaskNotTerminal`，主 accepted events 不包含该 `work.done`。
- Happy path: 同一 payload 引用 closed task，`work.done` 进入 accepted events，并更新 `work_done_seen_tasks`。
- Integration: rejected `work.done` 仍产生可诊断 rejection envelope，且 safe target 指向负责重试的 hat，不丢失 recovery 信息。

**Verification:**
- `TaskNotTerminal` 不再能同时存在于主 events 和 rejection ledger。
- 已有 execution contract 测试继续通过。

- [ ] **Unit 3: Terminal-closed guard as pure decision**

**Goal:** 定义 terminal honored 后哪些 topic 必须被冻结，哪些 control/diagnostic topic 不受影响。这个 Unit 只做纯决策，不接入真实 event loop。

**Requirements:** R2

**Dependencies:** Unit 1

**Files:**
- Create: `crates/ralph-core/src/event_loop/terminal_closed_guard.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/event_loop/terminal_closed_guard.rs`

**Approach:**
- 输入：`completion_honored`、topic、topic class（business、terminal-adjacent、control、diagnostic）。
- 输出：allow、reject-post-terminal、ignore-duplicate-terminal。
- 明确 `work.ready`、`work.done`、`plan.blocked`、`REVIEW_COMPLETE`、`report.done`、`LOOP_COMPLETE` 属于 terminal 后冻结集合。
- 明确 inspect/diagnostic 这类只读或诊断 control topic 不属于本计划冻结对象。

**Execution note:** 先写纯函数测试，不读取 runtime state 文件。

**Patterns to follow:**
- `crates/ralph-core/src/event_loop/loop_state.rs` 中 terminal-adjacent topic 集合和 dedup 语义。
- `crates/ralph-core/src/event_policy.rs` 中 completion-after-terminal action 的分类方式。

**Test scenarios:**
- Happy path: `completion_honored=false` 时业务 topic 允许继续进入后续 gate。
- Error path: `completion_honored=true` 时 `work.done`、`plan.blocked`、`REVIEW_COMPLETE` 被判为 post-terminal rejection。
- Edge case: `completion_honored=true` 时重复 `LOOP_COMPLETE` 被判为 duplicate/ignored，而不是新业务推进。
- Edge case: control/diagnostic topic 在 terminal 后不被错误归类为 business rejection。

**Verification:**
- 冻结集合和允许集合在测试中显式覆盖，后续 wiring 不需要重新发明分类。

- [ ] **Unit 4: Wire terminal-closed guard into runtime and repair stream**

**Goal:** 将 Unit 3 的 terminal guard 接入主 event loop 和 repair-stream 写入路径，确保 `LOOP_COMPLETE` honored 后不会再出现报告中的 `plan.blocked`、`work.done`、第二个 `REVIEW_COMPLETE`。

**Requirements:** R2

**Dependencies:** Unit 3

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-core/src/event_loop/repair_stream_sink.rs`
- Modify: `crates/ralph-core/src/event_loop/rejection.rs`
- Test: `crates/ralph-core/src/event_loop/tests/completion_honored.rs`
- Test: `crates/ralph-core/src/event_loop/tests/post_terminal_rejection.rs`

**Approach:**
- 在任何主 events write 前检查 terminal guard。
- repair stream 中如果收到 terminal 后业务事件，只写 diagnostic/rejection，不再转入主事件事实链。
- post-terminal rejection message 必须稳定，方便报告和 BDD 断言。
- 不改变 completion honored 的判定规则；本 Unit 只消费已有状态。

**Execution note:** 先用内存 EventLoop fixture 复现 terminal 后 `work.done` 仍可写的问题，再接入 guard。

**Patterns to follow:**
- `crates/ralph-core/src/event_loop/tests/completion_honored.rs` 的 completion 相关 fixture。
- `crates/ralph-core/src/event_loop/repair_stream_sink.rs` 的 repair event 记录风格。

**Test scenarios:**
- Error path: 先 accepted `LOOP_COMPLETE` 并 honored，再输入 `work.done`，主 events 不新增 `work.done`，diagnostics 中有 post-terminal rejection。
- Error path: terminal 后 repair sink 收到 `plan.blocked`，不写主 events。
- Edge case: terminal 后重复相同 `LOOP_COMPLETE` 不造成第二次 state transition。
- Integration: terminal guard 不阻止 completion honored 前的正常 `report.done -> LOOP_COMPLETE` 链。

**Verification:**
- terminal 后业务事件无法推进 EventBus、phase authority 或 downstream hat routing。
- diagnostics 足够解释事件被拒原因。

- [ ] **Unit 5: Shipper waits for validator terminal snapshot**

**Goal:** 建立纯决策 helper，要求 shipper 在当前 step 的 validator 终态之后才能发 `REVIEW_COMPLETE`。这先作为独立 shipper gate，不接入 runtime routing。

**Requirements:** R3

**Dependencies:** Unit 1

**Files:**
- Modify: `crates/ralph-core/src/event_loop/phase_authority/shipper_helper.rs`
- Modify: `crates/ralph-core/src/shipper_reason.rs`
- Test: `crates/ralph-core/src/event_loop/phase_authority/shipper_helper.rs`
- Test: `crates/ralph-core/src/shipper_reason.rs`

**Approach:**
- 输入：phase snapshot、plan terminal state、latest step、latest validator terminal topic、stall/recovery reason。
- 输出：allow shipper、deny wait-for-validator、hard-fail reason。
- `test.passed` / `test.failed` 均视为 validator terminal；缺失 validator terminal 时不允许 `pass_with_residuals`。
- `stall_recovery` / `handoff_dispatch_timeout` 不能在 validator 缺席时变成 recoverable success。

**Execution note:** 先写纯 helper 测试，覆盖报告中的“shipper 早于 validator 1 分 8 秒”形态。

**Patterns to follow:**
- `crates/ralph-core/src/event_loop/phase_authority/shipper_helper.rs` 的 `Deny` / `Forward` 判定模式。
- `crates/ralph-core/src/shipper_reason.rs` 中 recoverable reason allowlist 测试。

**Test scenarios:**
- Error path: plan terminal reason 为 stall recovery，当前 step 没有 validator terminal，输出 deny/wait-for-validator，不允许 pass。
- Happy path: 当前 step 已有 `test.passed`，plan terminal accepted，输出 allow shipper。
- Happy path: 当前 step 已有 `test.failed`，输出允许进入 fail/fix 语义，而不是 success ship。
- Edge case: validator terminal 属于旧 step，当前 step 不匹配，输出 deny。

**Verification:**
- 纯 helper 能独立表达“缺 validator terminal 时 shipper 不得成功收尾”。

- [ ] **Unit 6: Wire shipper validator gate into routing**

**Goal:** 将 Unit 5 接入真实 shipper routing，确保 runtime 不会在 validator 缺席时激活 shipper/reporter 生成 `REVIEW_COMPLETE(pass_with_residuals)`。

**Requirements:** R3

**Dependencies:** Unit 5

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-core/src/event_loop/loop_state.rs`
- Modify: `crates/ralph-core/src/event_loop/phase_authority/on_accepted.rs`
- Test: `crates/ralph-core/src/event_loop/tests/shipper_waits_for_validator.rs`

**Approach:**
- 在 accepted `test.passed` / `test.failed` 时更新 current step validator terminal snapshot。
- 在 shipper activation/routing 前调用 Unit 5 helper。
- deny 时写明确 diagnostics，并保持 loop 未被伪装为 success；如果 recovery 已耗尽，应走 fail-close terminal reason，而不是 pass_with_residuals。
- 不修改 preset instructions；agent 可见指令留到 Unit 8。

**Execution note:** 先加 runtime-level 测试：构造 plan terminal/stall recovery 但无 validator terminal，断言 shipper 不被激活。

**Patterns to follow:**
- `crates/ralph-core/src/event_loop/loop_state.rs` 中 `last_test_passed_step` 的记录方式。
- `crates/ralph-core/src/event_loop/phase_authority/on_accepted.rs` 的 accepted event snapshot update。

**Test scenarios:**
- Error path: accepted `plan.blocked(recovery_exhausted:stall_recovery...)` 但无 current step validator terminal，不产生 `REVIEW_COMPLETE(pass_with_residuals)`。
- Happy path: `work.done -> test.passed(current step) -> plan.complete` 后允许 shipper。
- Edge case: `test.passed(step-01)` 后 current step 已推进到 `step-02`，shipper 不把旧 validator terminal 当作当前 step 证据。
- Integration: deny shipper 时不会触发 reporter，也不会写 `LOOP_COMPLETE`。

**Verification:**
- 报告中的 shipper/validator 时间倒挂在 runtime 测试中不再可能。

- [ ] **Unit 7: Task ledger idempotency by live task identity**

**Goal:** 稳定 `.ralph/agent/tasks.jsonl` 的 live task identity，防止同一 loop/task_key/step 被 coordinator 和 recovery 路径写成两条不同形态记录。

**Requirements:** R4

**Dependencies:** Unit 2

**Files:**
- Modify: `crates/ralph-cli/src/task_cli.rs`
- Modify: `crates/ralph-core/src/config/tasks.rs`
- Modify: `crates/ralph-core/src/execution_contract.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-cli/src/task_cli.rs`
- Test: `crates/ralph-core/src/execution_contract.rs`
- Test: `crates/ralph-cli/src/hat_command_policy.rs`

**Approach:**
- 定义 live identity key：`loop_id + task_key + step`；缺 `task_key` 的 legacy task 只能走 legacy path，不得与 keyed task 混写。
- `task ensure` 对同一 identity 返回同一 live record，不追加第二行。
- `task add` 如显式创建同一 identity，应拒绝或提示使用 ensure，避免产生同 id 双 title 记录。
- `task_id` 必须来自 live record；event payload 声称的 task_id 与 identity 不匹配时由 execution contract 拒收。
- 不迁移历史 JSONL；只保证新写路径幂等。
- 执行时先在 `crates/ralph-cli/src/task_cli.rs` 定位现有 `execute_add` / `execute_ensure` / `ensure_task_with_args` 等入口；如果实现已经把读写逻辑拆到局部 helper，只修改该 helper，不新增第二套 task store。

**Execution note:** 先写 task store 层测试，不启动 loop。

**Patterns to follow:**
- `crates/ralph-cli/src/doctor.rs` 中对 `tasks.jsonl` 的读取和 drift 汇总。
- `crates/ralph-cli/src/hat_command_policy.rs` 对 coordinator hats 的 task command 限制。

**Test scenarios:**
- Happy path: 第一次 ensure keyed task 写入一条记录并返回 task_id。
- Happy path: 第二次 ensure 同一 loop/task_key/step 返回同一 task_id，不追加 JSONL 行。
- Error path: add 同一 loop/task_key/step 的第二条记录被拒绝或转换为 ensure 语义，行为必须稳定。
- Error path: `work.done` payload 的 task_id 与 identity key 对不上，execution contract 拒收。
- Edge case: legacy 无 task_key 记录不被错误合并到 keyed record。

**Verification:**
- 同一 step 不再出现“Step 02: ...”和“step-02”两种 live task 记录并存。
- task command 文档和 policy 仍然限制 worker hats 不能创建/ensure task。

- [ ] **Unit 8: Protocol violation bounded retry and correction context**

**Goal:** 明确 agent 违反协议后的恢复闭环：第一次同类违规给结构化 correction 和 bounded retry；再次同类违规 fail-close，避免“错了就错了”或无限 `task.resume`。

**Requirements:** R5, R9

**Dependencies:** Unit 1, Unit 2, Unit 4, Unit 6, Unit 7

**Files:**
- Modify: `crates/ralph-core/src/event_loop/rejection.rs`
- Modify: `crates/ralph-core/src/correction/mod.rs`
- Modify: `crates/ralph-core/src/event_loop/loop_state.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/event_loop/tests/protocol_violation_recovery.rs`
- Test: `crates/ralph-core/src/correction/mod.rs`

**Approach:**
- 定义 retry signature：`hat_id + topic + task_key + step + violation_code`。
- 第一次同类违规生成结构化 correction，至少包含 `violation_code`、`rejected_topic`、`target_hat`、`required_action`、`forbidden_action`、`live_task_id/task_key/step`、`suggested_command` 或 `emit_payload_patch`。
- correction 只能进入 recovery/correction context 或 targeted `task.resume`，不得把 rejected business event 写入主 events。
- 第二次同类违规达到 retry budget 后 fail-close，写 `plan.blocked` 或 terminal diagnostic reason，但不得经 shipper 翻成 pass/pass_with_residuals。
- post-terminal business event 不进入 retry；terminal closed 是硬边界。
- shipper-before-validator 不让 shipper 自己 retry pass，只能等待 validator terminal 或 fail-close。

**Execution note:** 先写纯 signature/budget/correction tests，再接入 event loop rejection path。

**Patterns to follow:**
- `crates/ralph-core/src/correction/mod.rs` 现有 `emit_correction_context` / duplicate work done hint。
- `crates/ralph-core/src/event_loop/loop_state.rs` 现有 rejection signature / stale breaker 计数模式。
- `crates/ralph-core/src/event_loop/rejection.rs` 的 structured rejection envelope。

**Test scenarios:**
- Error path: 第一次 `TaskNotTerminal(work.done)` 生成 correction，target 指向需要补救的 hat，主 events 不含 rejected `work.done`。
- Error path: 第二次同一 signature 的 `TaskNotTerminal(work.done)` fail-close，不再继续注入相同 `task.resume`。
- Error path: duplicate `work.ready` 第一次 correction 明确 forbidden action 为“不要重发 work.ready / 不要新建 task”。
- Edge case: 同一 hat 不同 step 的违规 signature 不互相污染 retry budget。
- Edge case: post-terminal `work.done` 不生成 retry correction，只走 terminal guard rejection。
- Integration: correction context 能进入下一 activation prompt，但不会突破 isolated 单业务事件预算。

**Verification:**
- agent 违反协议时有一次可执行恢复机会。
- 同类重复违规不会无限恢复，也不会 silent-success。
- correction 中明确写出 agent 应做什么和禁止做什么。

- [ ] **Unit 9: Rewrite ce-executor-serial instructions and agent skill docs as trigger state table**

**Goal:** 将 `ce-executor-serial` 中 agent 容易误解的长篇恢复说明改成状态表：每个触发事件对应唯一允许动作、禁止动作、成功/失败 signal 和 Handoff Envelope 模板引用。

**Requirements:** R5, R6, R7, R9, R10

**Dependencies:** Unit 2, Unit 4, Unit 6, Unit 7, Unit 8

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`
- Modify: `presets/schemas/ce-executor-serial.yml`
- Modify: `crates/ralph-core/data/ralph-tools-emit.md`
- Modify: `crates/ralph-core/data/ralph-tools-tasks.md`
- Modify: `crates/ralph-core/data/ralph-tools.md`
- Modify: `crates/ralph-core/data/ralph-tools-recovery-directives.md`
- Modify: `crates/ralph-core/data/ralph-tools-precheck.md`
- Modify: `skills/ralph-preset-common/references/agent-native-model.md`
- Modify: `skills/ralph-preset-common/references/author-checklist.md`
- Modify: `skills/ralph-preset-common/references/patterns.md`
- Modify: `skills/ralph-preset-common/references/finding-rubric.md`
- Modify: `CLAUDE.md`
- Modify: `AGENTS.md`
- Test: `crates/ralph-cli/src/presets.rs`

**Approach:**
- Coordinator instructions 增加触发表，至少覆盖 `work.start`、`task.resume(TaskNotTerminal/work.done)`、`task.resume(duplicate_work_done)`、`test.passed(step-N)`、`review.complete`、`work.failed`。
- 对每行写清楚：Observe source、allowed action、forbidden action、emit topic、receiver_contract、必须引用的 `ralph-tools-*` 章节。
- 明确 `task.resume(TaskNotTerminal/work.done)` 下 coordinator 不得新建 task、不得发新 step 的 `work.ready`，只能让同一 task identity 的 executor 补齐 close->emit 或 fail-close。
- Executor instructions 明确 close task 必须先于 `work.done` emit，并引用 task 三字段同源规则。
- Validator/shipper/reporter instructions 明确不能绕过 current step validator terminal；shipper 缺 validator terminal 时必须 fail/stop，不得发 pass_with_residuals。
- Runtime correction / `task.resume` 相关段落必须明确：收到 correction 后只执行 correction 指定的唯一动作；不要自由重试；不要在同 activation 发第二个业务事件；同类第二次失败会 fail-close。
- `crates/ralph-core/data/ralph-tools-emit.md` 必须新增或强化“协议违规后的 EmitResult/correction 响应”说明：`ok=false`、`recorded=false`、`errors[].code`、`allowed_next`、`suggested_command`、`handoff_envelope` 摘要如何读。该文档必须保持通用，不得出现 `coordinator`、`executor`、`validator`、`shipper`、`ce-executor-serial`、报告编号或某个 preset 的固定 topic 链。
- `crates/ralph-core/data/ralph-tools-tasks.md` 必须明确通用 live task identity：`task_id` 从 `ralph tools task list/show/ensure` 获取，`task_key + step` 是稳定匹配键，禁止手写或复用 closed task id。示例必须使用中性名称，如 `worker` / `reviewer` / `task-key`，不得使用 serial 专用 unit 名或 hat 名。
- `crates/ralph-core/data/ralph-tools-recovery-directives.md` 必须明确通用 correction/recovery directive 优先级：runtime correction 高于 agent narrative；只能按 target role、required action、forbidden action 执行。不得写入 serial 的具体 recovery 表；serial 表只放 preset instructions。
- `crates/ralph-core/data/ralph-tools-precheck.md` 必须强调 protocol correction 后仍先 `--policy-check`，通过后再真实 emit；precheck 失败不允许同 activation 继续真实 emit。示例必须是通用 emit 示例，不得引用本计划的诊断案例。
- `crates/ralph-core/data/ralph-tools.md` 必须在 always-injected 总入口里指向上述 recovery/correction 文档，避免 agent 只读主指南不知道 retry 规则。总入口只写“何时加载哪个通用 skill doc”，不写 serial 专用流程。
- `presets/en/ce-executor-serial.yml` 才承载 serial 专用状态表、hat 名称、topic 编排、receiver contract 模板和禁止动作。
- `skills/ralph-preset-common/references/*.md` 可以写 preset 作者/评审如何检查“通用 docs 与专用 preset 分层”，但也不得把 serial 的一次性运行报告内容当成通用规则。
- Preset schema 如只改 instructions 不需要新增字段，但仍必须检查 schema 与 event topology 是否漂移；若新增 contract enum 或 required field，按 preset/schema 同步规则更新。
- `CLAUDE.md` 与 `AGENTS.md` 必须保持完全一致。

**Execution note:** 先写 preset text/static tests，扫描状态表覆盖关键 trigger 和 forbidden action，再改 YAML。

**Patterns to follow:**
- `crates/ralph-core/data/ralph-tools-emit.md` 的 policy-check 和 Handoff Envelope 章节。
- AGENTS hard rule 中“Hat instructions 必须用 hat 视角编写”和“引用 skill doc，不复述其内容”。

**Test scenarios:**
- Static happy path: coordinator instructions 中每个关键 trigger 都有状态表行。
- Static error path: coordinator instructions 不再包含“task.resume 后直接 re-emit work.ready”这类宽泛恢复指令。
- Static error path: executor instructions 包含 close-before-work.done 的明确约束和 `ralph-tools-tasks` 引用。
- Static error path: shipper instructions 包含 wait-for-validator terminal 的约束，不允许 validator 缺席时 pass_with_residuals。
- Static error path: data skill docs 明确同类协议违规 bounded retry 和第二次 fail-close。
- Static error path: `ralph-tools-recovery-directives.md` 明确 correction 的 required/forbidden action 语义。
- Static error path: `crates/ralph-core/data/*.md` 不包含 serial 专用 hat 名、preset 名、报告编号、计划编号或本轮诊断术语；这些词只允许出现在 preset、plan、report 或 preset operator skill 中。
- Integration: preset lint 通过，embedded preset 与 schema merge 后保持 byte-equality。

**Verification:**
- Agent 可见 instructions 从散文规则变成可机械执行状态表。
- 文档、preset operator skills、AI skill guides 与新 runtime 语义一致。
- data skill docs 明确告诉 agent：协议违规后看哪里、怎么修、最多重试几次、什么时候停。
- data skill docs 保持通用，不会让非 serial preset 的 agent 误以为自己处在 serial 拓扑中。
- `CLAUDE.md` 与 `AGENTS.md` 无差异。

- [ ] **Unit 10: True runtime regression scenarios and final baseline**

**Goal:** 用真实 runtime path 验证 Unit 1-9 串起来后能防止本次报告中的问题复发，并完成项目要求的全量基线。

**Requirements:** R1-R9

**Dependencies:** Unit 1-9 全部完成

**Files:**
- Create: `crates/ralph-core/tests/scenarios/ce_executor_serial_runtime_protocol_happy_path.yml`
- Create: `crates/ralph-core/tests/scenarios/ce_executor_serial_rejects_post_terminal_business_event.yml`
- Create: `crates/ralph-core/tests/scenarios/ce_executor_serial_shipper_waits_for_validator.yml`
- Create: `crates/ralph-core/tests/scenarios/ce_executor_serial_task_identity_idempotent.yml`
- Create: `crates/ralph-core/tests/scenarios/ce_executor_serial_protocol_violation_retry_then_fail_close.yml`
- Modify: `crates/ralph-core/tests/scenarios.rs`
- Modify: `scripts/check-cli-doc-drift.sh` only if Unit 9 introduced new source line references

**Approach:**
- 所有新增 BDD 必须使用真实 workflow guard runner，禁止使用只断言 iteration 数的 stub。
- happy path 断言 `work.ready -> work.done -> test.passed -> review path -> REVIEW_COMPLETE -> report.done -> LOOP_COMPLETE`，且 terminal 后无业务事件。
- post-terminal scenario 模拟 `LOOP_COMPLETE` honored 后 agent/recovery 尝试发 `work.done` 或 `plan.blocked`，断言拒收和 diagnostics。
- shipper scenario 模拟 stall/recovery plan terminal 但缺 validator terminal，断言不产生 `REVIEW_COMPLETE(pass_with_residuals)`。
- task identity scenario 模拟 repeated ensure/add，同一 key/step 只保留一个 live task id。
- protocol violation scenario 模拟第一次 bad emit 得到 correction、第二次同类 bad emit fail-close；断言没有无限 `task.resume`，也没有 silent-success。
- 最后执行项目标准全量基线；如遇时序 flake，只按项目规则使用 serial fallback。

**Execution note:** 本 Unit 是唯一允许做跨层/端到端验证的 Unit；若发现生产缺陷，必须回到对应 Unit 修正，不能在本 Unit 混杂实现。

**Patterns to follow:**
- `crates/ralph-core/tests/scenarios.rs` 中 `run_workflow_guard_scenario` 的真 runner 场景。
- `crates/ralph-core/tests/scenarios/*.yml` 中 mock responses 与 expected events 写法。

**Test scenarios:**
- Integration happy path: 完整 serial happy path 只有一条终态链，且终态后无业务事件。
- Integration error path: terminal 后 `work.done` 被拒收，主 events 不增加该 topic。
- Integration error path: terminal 后 repair sink `plan.blocked` 被拒收或仅诊断，不触发 shipper/reporter。
- Integration error path: shipper 在 validator terminal 缺席时不发 `REVIEW_COMPLETE(pass_with_residuals)`。
- Integration happy path: validator terminal 当前 step 匹配后 shipper 正常运行。
- Integration edge case: repeated task ensure 同一 loop/task_key/step 不产生双行。
- Integration error path: `work.done` task_id 与 live identity 不匹配时拒收，且 rejected event 不写主 events。
- Integration error path: 同一协议违规第一次产生 correction，第二次同 signature fail-close，主 events 不出现 rejected business event。

**Verification:**
- 新增 scenario 全部通过。
- preset lint、schema parity、SSOT byte-equality、doc drift 全部通过。
- 全量 `./scripts/run-tests.sh` 通过，或按项目规则记录 serial fallback 结果。
- operator 金丝雀 SC1×3 手工清单可执行：同一 plan 连续三次正规终态，无 progress-steward activation、无终态后业务写入、无 silent-success、无无限 retry。

## System-Wide Impact

- **Interaction graph:** event parser、event policy、execution contract、event loop commit、repair stream、shipper routing、task CLI、preset instructions、agent skill docs 都会受影响。
- **Error propagation:** 拒收必须进入 typed rejection/diagnostics，而不是主 events；agent 通过 EmitResult/recovery directive 获得下一步；同类重复违规必须 fail-close。
- **State lifecycle risks:** 最大风险是旧路径仍绕过 accepted-event commit boundary，必须用 Unit 10 的 post-terminal、TaskNotTerminal 和 protocol retry 场景覆盖。
- **API surface parity:** `ralph emit --policy-check`、正式 emit、runtime accepted path 必须同源；本计划不扩展 wave parity。
- **Documentation layering:** `crates/ralph-core/data/*.md` 是通用 agent guide，serial 专用知识必须留在 preset instructions / preset operator skills，避免 prompt 注入时污染其它 preset 的 agent。
- **Integration coverage:** Unit 1-9 的单元测试不足以证明链路，Unit 10 必须走真 workflow guard runner。
- **Unchanged invariants:** isolated 单业务事件预算不变；Handoff Envelope v1 字段形态不变；非 serial preset 默认行为不应改变。

## Risks & Dependencies

| Risk | Likelihood | Impact | Mitigation |
|------|------------|--------|------------|
| accepted-event commit boundary 改动触达主事件路径，可能影响非 serial preset | Medium | High | Unit 1-2 先以小 helper 和 execution_contract characterization 收敛，再由 Unit 9 验证非 serial 基线 |
| terminal guard 误杀 control/diagnostic topic | Medium | Medium | Unit 3 用 topic class 明确业务/terminal-adjacent/control 边界 |
| shipper gate 过严导致合法 plan.blocked 无法收尾 | Medium | High | Unit 5 区分 validator terminal 缺席、validator failed、plan blocked fail-close，不把所有 blocked 都等同 |
| task ledger 幂等影响历史 legacy records | Medium | Medium | Unit 7 只对 keyed live task identity 生效，legacy 无 key 记录保留旧路径 |
| 协议违规 retry 变成新一轮无限恢复 | Medium | High | Unit 8 以 retry signature + bounded budget + fail-close 测试固定语义 |
| 通用 data skill docs 被 serial 专用术语污染，导致其它 preset agent 误判上下文 | High | Medium | Unit 9 明确 docs 分层，并加静态扫描测试阻止 serial 专用词进入 `crates/ralph-core/data/*.md` |
| preset instructions 再次漂移 | High | Medium | Unit 9 加 static tests 扫状态表和 forbidden action，并同步 agent/preset skill docs |
| BDD mock 与真实 run 仍有差距 | Medium | Medium | Unit 10 保留 SC1×3 operator 金丝雀作为最终人工验收清单 |

## Documentation / Operational Notes

- 修改 `presets/en/ce-executor-serial.yml` 后必须检查 `presets/schemas/ce-executor-serial.yml` 是否需要同步，并执行 preset lint / schema parity / SSOT byte-equality。
- 修改 `crates/ralph-core/data/*.md` 中源码行号引用后，必须用对应源码片段复核引用仍准确，并执行 doc drift 检查。
- 本计划明确要求检查并按需更新 `crates/ralph-core/data/ralph-tools.md`、`ralph-tools-emit.md`、`ralph-tools-tasks.md`、`ralph-tools-recovery-directives.md`、`ralph-tools-precheck.md`。这些文件只能写通用 agent 行为，不能写 serial 专用拓扑、hat 名、报告编号、计划编号或一次性诊断术语；跳过任一文件必须在 Unit 9 记录原因。
- serial 专用状态表、hat 名称、topic 编排和本轮诊断上下文只允许写入 `presets/en/ce-executor-serial.yml`、本计划、报告或 preset operator skill 的评审规则中。
- 修改 `CLAUDE.md` 或 `AGENTS.md` 任一文件后，必须同步另一个，保持内容完全一致。
- 本计划不要求安装 zsh completion，除非 Unit 9 实际改变 builtin preset 名称或补全列表。

## Success Metrics

- 主 events 中不再出现被 execution contract 拒收的业务事件。
- `LOOP_COMPLETE` honored 后主 events 不再出现新的业务 topic 或 terminal-adjacent topic。
- shipper 不再早于 current step validator terminal 生成 `REVIEW_COMPLETE`。
- `.ralph/agent/tasks.jsonl` 对同一 loop/task_key/step 不再出现双 live record。
- `ce-executor-serial` instructions 中关键 recovery/trigger 路径可按状态表机械执行。
- agent 违反协议时第一次收到结构化 correction；同类第二次违规 fail-close，不出现无限恢复或 silent-success。
- `crates/ralph-core/data/*.md` 中 agent 可见指南明确说明 correction/retry/fail-close 规则。
- `crates/ralph-core/data/*.md` 保持通用，不含 serial 专用拓扑、hat 名称或本轮诊断术语。
- 金丝雀 plan 连续三次不以 silent-success 或 pass_with_residuals 伪成功收尾。

## Sources & References

- Origin: `docs/brainstorms/2026-07-06-ce-executor-serial-protocol-ssot-convergence-requirements.md`
- Origin: `docs/brainstorms/2026-07-06-ce-executor-serial-handoff-envelope-requirements.md`
- Diagnosis: `docs/report/2026-07-07-ce-executor-serial-primary-20260706-230230-diagnosis.md`
- Related plan: `docs/plans/2026-07-06-001-feat-ce-executor-serial-protocol-ssot-convergence-plan.md`
- Related plan: `docs/plans/2026-07-06-004-feat-ce-executor-serial-handoff-envelope-plan.md`
- Related plan: `docs/plans/2026-07-07-001-fix-ce-executor-serial-handoff-envelope-review-gaps-plan.md`
- Runtime: `crates/ralph-core/src/event_loop/mod.rs`
- Runtime state: `crates/ralph-core/src/event_loop/loop_state.rs`
- Contract: `crates/ralph-core/src/execution_contract.rs`
- Shipper: `crates/ralph-core/src/shipper_reason.rs`
- Preset: `presets/en/ce-executor-serial.yml`
- Schema: `presets/schemas/ce-executor-serial.yml`
