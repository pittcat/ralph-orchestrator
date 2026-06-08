---
title: CE Executor 事件拒绝恢复与发布契约修复计划
type: fix
status: active
date: 2026-06-07
origin: docs/report/2026-06-07-ce-executor-drift-plan-diagnostic-analysis.md
---

# CE Executor 事件拒绝恢复与发布契约修复计划

## Overview

本计划修复 `ce-executor` 在代理超时、事件写错或 wave 结果不完整时“知道出错了，但不知道应该把工作交还给谁”的问题。

修复依据来自 2026-06-06 两个 worktree 的真实运行文件和当前源码。基本原则是：错误事件继续拒绝，但拒绝后必须把原任务和明确的错误原因交还给正确角色，不能靠扩大权限或猜测缺失字段来绕过检查。

## Problem Frame

失败链路已由运行产物和源码双向确认：

1. `executor` 超时后没有写入事件，missing-event hard gate 正常触发。
2. Coordinator 执行模式实际由 `ralph` 运行具体 hat，但输出归因到显示 hat；一次 `work.done` 缺少 `plan_path`，被 execution contract 拒绝。
3. 后续输出被归因为 `review-coordinator`，却再次发送了未声明的 `work.done`，origin guard 按设计拒绝。
4. 拒绝事件没有形成稳定的“原业务 hat 重试”闭环，后续迭代再次产生错误事件。
5. hard gate 只依据 `publishes 非空且无 default_publishes` 判断发布义务，无法表达条件发布、非法事件已尝试、超时和完全未发事件之间的差异。
6. preset 中 operator-controlled 预算会被过滤并警告；这不是静默丢弃，但把无效预算写在 builtin preset 中仍会持续制造误解。
7. 8 个 wave worker 实际全部成功，但写回主事件文件的结果格式不统一：其中 3 条缺少 `wave_id/wave_index`，时间字段还混用了 `ts` 和 `timestamp`。这足以解释聚合器无法确认“这 8 条属于同一批且已经全部到齐”。但当前源码里的标准合并函数已经会补 `wave_id/wave_index`，所以真实缺陷不是“合并函数没补字段”，而是有事件绕过了标准合并路径，或历史运行使用了旧路径/人工补写路径。
8. `review-synthesizer` 没有正常发事件时，框架会默认发布 `review.complete`。它不会直接绕过 plan-gate，但可能把“synthesizer 执行失败”伪装成“审查已经完成”，或者因缺少业务字段再次被拒绝。
9. 主 `.ralph/loops.json` 为空符合当前 primary loop 主要使用 `loop.lock` 的实现，不能据此认定注册失败；真正的问题是 CLI 和诊断没有把 registry、lock 和 worktree 状态统一解释。
10. RPC 日志能够确认 stdout 写入端失去了读取者，但当前日志丢弃了原始 I/O 错误，无法判断是客户端断开、父进程退出还是管道被关闭。
11. 本次 wave 的 8 个维度属于合法选择：7 个固定维度加 `reliability`。`learnings` 已实际运行，不存在固定缺失维度的问题。

第二个 worktree 的 `plan.complete` 证明 plan-gate 并非整体不可达。worktree 的 runtime tasks 按 `LoopContext` 设计就是逐 loop 隔离，主仓库和 worktree 的任务 ID 不应直接混合比较。

## Requirements Trace

- R1. 非法或 contract-invalid 事件必须保持拒绝，不能通过放宽 hat 发布范围绕过契约。
- R2. 被拒事件必须携带稳定的来源 hat、目标 topic、违反项和安全重试目标，使下一迭代能回到负责该业务事件的 hat。
- R3. hard gate 必须区分“未尝试发布”“声称发布但文件不可见”“写入了被拒事件”和“条件发布合法为空”。
- R4. `ce-executor` 的 preset、instructions、schemas、execution contracts 和运行时 gate 必须能被静态一致性检查。
- R5. runtime task、event、diagnostics 和 loop registry 的证据必须通过 `loop_id/workspace/task_id` 关联，避免跨 worktree 误诊。
- R6. 预算字段的 operator/preset 所有权必须在 preset 校验阶段明确呈现，不能依赖运行时警告才暴露。
- R7. 修复必须由失败事件流回放和真实 runtime 集成测试证明闭环恢复。
- R8. wave worker 无论使用什么合法输出形式，写回主事件文件前都必须被标准化，并补齐可信的 wave 关联字段。
- R9. synthesizer 执行失败必须被明确表示为失败或恢复状态，不能默认伪装成审查完成或审查通过。
- R10. RPC 输出失败必须记录具体 I/O 错误和连接生命周期信息，使历史产物足以判断断开原因。
- R11. 修复不得破坏现有正常功能：wave 成功结果合并、partial-timeout 可见结果、worker failure synthetic events、Coordinator 模式 hat 归因、通用 `default_publishes`、primary/worktree registry 生命周期都必须保持原语义。

## Scope Boundaries

- 不给 `review-coordinator` 增加 `work.done` 发布权限。
- 不自动补全 `plan_path`、`task_id` 等业务 payload 字段。
- 不把 worktree runtime `tasks.jsonl` symlink 到主仓库。
- 不修改 origin guard 的 fail-closed 原则。
- 不因为本次事件直接把 `review-synthesizer.default_publishes` 改为 `review.passed`；该语义必须由独立契约测试决定。
- 不在本计划中实现新的通用 hat handoff 协议或重写 EventBus。
- 不把本次 8 个 review 维度改成固定 9 个；条件维度继续按 diff 内容选择。
- 不把主 `.ralph/loops.json` 为空直接当作 registry 数据损坏。
- 不重写 `merge_wave_results_to_events_file` 的已工作语义；只修复能复现的旁路或缺少防护的入口。
- 不全局删除或改变 `default_publishes` 机制；只调整 ce-executor 中会伪装业务结论的具体 hat 配置或运行时解释。

## Context & Research

### 已确认事实

- `presets/en/ce-executor.yml` 中 `review-coordinator` 仅允许发布 `review.wave.ready` 和 `review.passed`；它发出的 `work.done` 属于真实越权事件。
- `crates/ralph-core/src/event_origin.rs` 和 `crates/ralph-core/src/hat_registry.rs` 会拒绝注册 hat 的未声明业务 topic，行为符合安全设计。
- `crates/ralph-cli/src/loop_runner/runner.rs` 在 Coordinator 模式下执行 hat 为 `ralph`，再通过 `display_hat` 和 `output_hat_id` 将输出归因到业务 hat。
- execution-contract rejection 当前只在已存在匹配 `task.resume` 时识别安全目标；拒绝本身不会主动创建重试。
- hard gate 的发布义务判定是通用布尔规则，未建模 topic 级义务和激活路径。
- `LoopContext::tasks_path()` 明确规定 runtime task 按 loop 隔离；报告把主仓库和 worktree task 文件直接对账属于错误前提。
- `merge_hats_overlay` 已对被过滤预算输出 warning，因此 M4 的“静默”表述已过时；但 builtin preset 仍声明无效预算，存在配置契约漂移。

### 进一步复核后的结论

- **Synthesizer 为什么没有运行：故障形态已定位，当前源码还需定位旁路入口。** 8 个 worker 全部完成，但 3 条结果缺少 wave 关联字段，聚合器无法判断同一批结果已经收齐。当前标准合并函数已经会补这些字段，因此实施前必须先用 fixture 证明是哪条入口绕过了标准合并。
- **`review.complete` 是否绕过 plan-gate：已排除。** 它仍会进入 plan-gate；真正风险是默认业务事件掩盖 synthesizer 自身失败。
- **主 `loops.json` 为空：不是注册失败证据。** primary loop 使用 `loop.lock`，worktree loop 才有显式 registry 记录；需要修的是统一展示和诊断语义。
- **RPC stdout：故障范围已缩小。** 确认是 stdout 写入失败，但现有日志不足以区分客户端、父进程或管道哪一方先关闭。
- **Wave 维度数量：已排除异常。** 本次 8 个维度符合 preset 的固定加条件选择规则，`learnings` 已执行。

## Key Technical Decisions

- **拒绝后重试原业务 hat，而非切换执行器**：Coordinator 模式本来就由 `ralph` 执行；恢复目标应是业务归因 hat 和触发上下文。
- **恢复使用原始触发事件快照**：重试 prompt 应重新提供导致该 hat 激活的 accepted event，禁止从错误 payload 猜字段。
- **发布义务显式化**：在 hat 配置中增加最小的 obligation 元数据，或从 activation topic 到允许结果 topic 的契约中派生，不维护硬编码终端 topic 白名单。
- **非法事件与未发事件分流**：origin rejection、contract rejection、missing event、late event 分别记录和处理，不共享同一 hard-gate 计数语义。
- **静态校验优先**：builtin preset 在启动前检查 instructions 中声明的 publish 行为、`publishes`、schemas、required events 和 execution contracts 的一致性。
- **任务隔离保持不变**：补充 session-aware 诊断关联，不改变 runtime task 存储拓扑。
- **Wave 结果由框架统一包装**：worker 只负责产生业务结果，`wave_id`、`wave_index`、`wave_total` 和标准时间字段由 dispatcher 在合并时填写，不能信任代理自行填写。
- **不重复实现已存在的 wave 合并能力**：源码已有 `merge_wave_results_to_events_file` 给结果补 `wave_id/wave_index`，修复应聚焦“为什么历史结果没有经过它”或“哪些输入没有被它的测试覆盖”。
- **默认事件不能代表业务成功**：synthesizer 未输出时应该进入明确的失败或恢复路径，不能自动推导出 review verdict。
- **Registry 是多份状态之一**：CLI 统一读取 `loop.lock`、`current-loop-id`、主 registry 和 worktree registry，再给出面向用户的活动 loop 视图。

## Regression Guardrails

这些是实现时必须保护的现有功能。每项修复都要先跑对应测试或补齐同等覆盖，不能用“修当前 bug”换来已有功能回退。

- Wave 正常成功：多个 worker 成功时，标准合并函数继续把每个业务事件追加到主 events 文件，并带上框架生成的 `wave_id/wave_index`。
- Wave 部分超时：worker 已经写出事件但进程超时，现有行为是保留可见结果，不额外合成 failure；不能改成一超时就丢结果。
- Wave worker 失败：spawn 失败或无事件超时时，继续生成 `wave.worker.failed` 和对应 publish topics 的 synthetic failure 事件，让下游 aggregator 能看到部分失败。
- Coordinator hat 归因：`ralph` 作为执行器时，输出仍按当前激活的业务 hat 处理；不能让所有事件都退化成 `ralph` 来源。
- 通用 `default_publishes`：其他 preset/hat 依赖默认事件完成链路，不能全局禁用。只允许对 ce-executor 的 review-synthesizer 做局部安全处理。
- `review.passed` 与 `review.complete` 路由：两者继续进入 plan-gate，不能直接进入 shipper 或 reporter。
- Registry 生命周期：worktree loop 继续注册到主 repo registry；primary loop 继续由 `loop.lock/current-loop-id` 表示，除非另有明确设计迁移。
- RPC 协议输出：增加错误日志不能改变 JSON-RPC stdout 正常输出格式，避免破坏调用方解析。

## 修完以后应该是什么样

- Executor 发错 `work.done` 时，系统明确告诉 Executor 缺了什么，并把原任务重新交给它，而不是让 Review Coordinator 接手补错。
- 8 个 review worker 全部结束后，系统能够确认 8 个结果都属于同一批，随后自动进入 Review Synthesizer。
- Review Synthesizer 自己失败时，系统显示“审查汇总失败，正在恢复”或明确终止，不会显示成“审查已完成”。
- `ralph loops` 和诊断报告能够同时说明 primary loop 与 worktree loop 的状态，不再需要人工比对多个 `.ralph` 目录。
- RPC 输出中断后，日志能够说明是 broken pipe、stdin EOF、channel 关闭还是其他 I/O 错误。
- Review 维度仍按代码变化动态选择；本次合法的 8 个维度不会被强行改成 9 个。

## High-Level Technical Design

> 以下为方向性设计，用于约束恢复语义，不是实现代码。

```text
accepted trigger event
        |
        v
business hat activation ----> agent output candidates
        |                            |
        |                  +---------+---------+
        |                  |                   |
        |              accepted          rejected event
        |                                      |
        |                         origin / payload / execution
        |                                      |
        +<---- targeted retry envelope <-------+
                    |
          original business hat
          original trigger snapshot
          violation + allowed topics

no candidate event
        |
        +--> obligation policy --> missing-event hard gate

wave workers × N
        |
        v
framework normalizes every result
  - wave_id
  - wave_index
  - wave_total
  - ts
        |
        v
all indexes present? ---- no ----> explicit partial-wave failure
        |
       yes
        v
review-synthesizer
```

## Implementation Units

- [ ] **Unit 1: 固化失败链路的回放与证据模型**

**Goal:** 将两个 worktree 的关键事件缩减为可提交的匿名化 replay fixture，先证明当前代码的实际分支和错误分类。

**Requirements:** R2, R3, R5, R7

**Dependencies:** None

**Files:**
- Create: `crates/ralph-core/tests/fixtures/ce-executor-rejected-event-recovery.jsonl`
- Create: `crates/ralph-cli/tests/ce_executor_recovery.rs`
- Modify: `crates/ralph-core/src/diagnostics/orchestration.rs`
- Test: `crates/ralph-cli/tests/ce_executor_recovery.rs`

**Approach:**
- fixture 只保留触发事件、缺字段 `work.done`、越权 `work.done`、wave 结果和必要 task 状态。
- 每条诊断证据记录 `loop_id`、workspace、iteration、business hat、executor hat、topic、task_id 和拒绝阶段。
- 明确断言独立 worktree 的 task store 不参与另一 loop 的 contract 验证。

**Execution note:** 先写失败回放测试，再修改恢复逻辑。

**Test scenarios:**
- Integration: `executor` 激活后无事件，产生 missing-event 诊断并保留原触发事件。
- Integration: `work.done` 缺 `plan_path`，事件被拒且不得进入 review-coordinator。
- Integration: `review-coordinator` 发 `work.done`，origin guard 拒绝且不得放宽权限。
- Edge case: 两个 workspace 存在不同 task ID，诊断只关联当前 `loop_id` 的 task store。

**Verification:**
- 当前实现下测试稳定重现停滞点，证据字段足以定位具体 loop 和业务 hat。

- [ ] **Unit 2: 建立统一的拒绝分类与定向重试**

**Goal:** 让 origin、payload policy 和 execution contract 的拒绝都能生成统一恢复请求，并重新激活原业务 hat。

**Requirements:** R1, R2, R3

**Dependencies:** Unit 1

**Files:**
- Modify: `crates/ralph-core/src/event_origin.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-core/src/event_loop/loop_state.rs`
- Modify: `crates/ralph-cli/src/loop_runner/hard_gate.rs`
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`
- Test: `crates/ralph-core/src/event_loop/tests/replay_light_integration.rs`
- Test: `crates/ralph-cli/tests/ce_executor_recovery.rs`

**Approach:**
- 将拒绝归一为包含 stage、source/business hat、topic、violation、原触发事件引用和 retry eligibility 的结构。
- 对已注册业务 hat 的可修复拒绝，主动发布 targeted `task.resume` 或等价内部恢复动作，而不是仅扫描是否碰巧已存在。
- 重试 prompt 提供违反项、允许 topics、required fields 和原始触发 payload；不得修改被拒 payload 后重新发布。
- 对未知 hat、越权 topic 和无法确定原激活上下文保持 fail-closed，并升级为人工指导。
- 为相同 rejection key 设置有界重试和终态，避免无限恢复循环。

**Test scenarios:**
- Happy path: 缺一个 required field 的 `work.done` 被拒后，下一迭代重新激活 executor，并携带原 `work.ready` payload。
- Error path: review-coordinator 越权发 `work.done` 后，重试 review-coordinator 但提示只能发声明 topics；再次越权达到阈值后终止为不可恢复。
- Error path: 未注册 hat 的事件不生成自动重试目标。
- Edge case: 同 iteration 多个拒绝按稳定 key 去重，不重复发布 resume。
- Integration: 修正后的 executor 重新发出合法 `work.done` 后进入 review wave。

**Verification:**
- 回放链路不再依赖 human.guidance 才能前进，同时所有非法事件仍被拒绝。

- [ ] **Unit 3: 统一 wave 结果格式，确保 Synthesizer 能收到完整一批结果**

**Goal:** 找出历史运行中 3 条 review 结果为什么没有经过标准 wave 合并，修掉真实旁路，同时保护已有 wave 正常合并行为。

**Requirements:** R7, R8, R9

**Dependencies:** Unit 1

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner/wave/io.rs`
- Modify: `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`
- Modify: `crates/ralph-core/src/wave_tracker.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-cli/src/loop_runner/tests.rs`
- Test: `crates/ralph-core/src/event_loop/tests/wave_results.rs`
- Test: `crates/ralph-cli/tests/ce_executor_recovery.rs`

**Approach:**
- 先用本次历史文件建立 characterization：同一个 wave 中混入了标准事件和缺少 wave 元数据的事件。
- 对照源码确认标准路径：`execute_wave` 读取 per-worker 文件，`WaveTracker` 按 worker index 记录，`merge_wave_results_to_events_file` 追加到主 events 文件并补 `wave_id/wave_index`。
- 定位旁路来源：检查 worker 是否可能写入主 events 文件、late event recovery 是否可能读取旧主文件、人工/旧版本事件是否被 replay 混入。
- 只修旁路：如果是 worker 能写主文件，强化 `RALPH_EVENTS_FILE` 隔离和 emit path allowlist；如果是 replay/late recovery 混入，补过滤和诊断；如果仅是旧历史产物，不改生产路径，只补诊断提示。
- 保留现有 partial-timeout 行为：worker 超时但已经写出结果时，结果继续可见，不强制合成失败。
- 给合并结果增加可观测性：日志写出 expected indexes、merged indexes、missing indexes、duplicate indexes，但不改变正常输出格式。

**Execution note:** 使用本次 8 条混合格式结果作为 characterization fixture。

**Test scenarios:**
- Happy path: 8 个 worker 都通过标准路径返回结果，主事件文件生成 8 条带 `wave_id/wave_index` 的事件，synthesizer 被激活。
- Regression: 现有 text、Claude stream、Pi stream、ACP partial-timeout 测试继续通过，已写出的 partial 结果不能被丢弃。
- Regression: worker spawn 失败继续生成 `wave.worker.failed` 和 publish topic synthetic events。
- Edge case: worker 原始事件缺少 `wave_id/wave_index`，标准合并路径仍按 worker 上下文正确补齐。
- Edge case: worker 试图写主 events 文件时被拒绝或被诊断为旁路写入，不能污染 aggregate。
- Error path: 同一个 wave 出现缺 index 或重复 index 时，日志明确列出 indexes；正常结果仍可被下游看到。

**Verification:**
- 能解释并防止本次“部分结果缺 wave 元数据”的旁路；现有 wave backend、partial-timeout 和 synthetic failure 测试不回退。

- [ ] **Unit 4: 将 hard gate 从 hat 级布尔值改为激活级发布义务**

**Goal:** 准确区分真正未履约和合法条件分支，避免拒绝事件被误报为“未发事件”。

**Requirements:** R3, R4

**Dependencies:** Unit 2

**Files:**
- Modify: `crates/ralph-core/src/config/hat.rs`
- Modify: `crates/ralph-cli/src/loop_runner/hard_gate.rs`
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`
- Modify: `presets/en/ce-executor.yml`
- Test: `crates/ralph-cli/src/loop_runner/tests.rs`
- Test: `crates/ralph-cli/tests/ce_executor_recovery.rs`

**Approach:**
- 为 activation 定义“必须产生一个允许结果事件”的契约，表达触发 topic 对应的合法结果集合；没有显式 obligation 的 hat 继续走现有 `publishes + default_publishes` 兼容逻辑。
- `processed.had_raw_events`、accepted、origin rejected、policy rejected 和 contract rejected 分别驱动状态转换。
- 只有完全没有候选事件且 activation obligation 未满足时增加 missing-event 计数。
- 声称 emit 但 late reader 未读到事件继续走 late-event 分支；已有拒绝事件不再增加 missing-event 计数。
- 将当前通用 `publishes 非空且无 default` 规则保留为无显式 obligation 时的兼容 fallback；本次不做全局删除。

**Test scenarios:**
- Happy path: executor 无事件触发 hard gate。
- Happy path: executor 发合法 `work.failed` 满足义务。
- Error path: executor 发 contract-invalid `work.done` 进入 rejection recovery，不进入 missing-event gate。
- Edge case: review-coordinator 根据空 diff 发 `review.passed`，根据非空 diff 发 wave，两个分支都满足义务。
- Edge case: aggregate hat 等待未齐结果时不被当作缺失发布。
- Regression: 现有 `test_should_hard_gate` 和 `test_missing_event_hard_gate` 的非 ce-executor 语义继续成立，除非对应 hat 显式配置新 obligation。

**Verification:**
- hard-gate 日志能够准确说明未发、晚到或已拒绝，连续计数只累计同类未恢复故障。

- [ ] **Unit 5: 修正 Synthesizer 失败时的默认行为并增强 preset 静态校验**

**Goal:** 在运行前发现 instructions、publishes、triggers、schemas 和 gate 配置的矛盾，并保证 synthesizer 没有输出时不会伪装成正常审查结论。

**Requirements:** R4, R6, R9

**Dependencies:** Unit 3, Unit 4

**Files:**
- Modify: `crates/ralph-cli/src/preset_contract.rs`
- Modify: `crates/ralph-cli/src/preflight.rs`
- Modify: `presets/en/ce-executor.yml`
- Modify: `presets/schemas/ce-executor.yml`
- Test: `crates/ralph-cli/src/preset_contract.rs`
- Test: `scripts/validate-builtin-presets.sh`

**Approach:**
- 校验每个 required workflow topic 至少有一个声明 publisher 和 subscriber。
- 校验 execution-contract topic 在 event policy 中有 schema，required fields 不互相冲突。
- 校验 activation obligation 的结果 topics 都在对应 hat 的 `publishes` 中。
- 对 builtin preset 声明 operator-owned 预算直接报 preset contract error，或从 preset 删除这些字段并在使用文档给出 operator 配置示例。
- 只对 ce-executor 的 `review-synthesizer` 删除默认业务结论，或改为专门的失败/恢复信号；不要改变核心 `check_default_publishes` 对其他 hats 的行为。
- 保留正常决策语义：0 findings 由 synthesizer 明确发布 `review.passed`；存在 residual findings 时明确发布 `review.complete`。
- 确认 `review.complete` 和 `review.passed` 都进入 plan-gate，不允许直接进入 shipper。

**Test scenarios:**
- Error path: instructions/obligation 引用未声明 publish topic 时校验失败。
- Error path: preset 声明 `max_runtime_seconds` 等 operator-owned key 时给出可执行错误。
- Error path: required event 无 publisher 或无 subscriber 时校验失败。
- Happy path: `ce-executor` 和全部 builtin presets 通过 contract matrix。
- Error path: synthesizer 被激活但没有发事件时，不得自动产生 `review.passed` 或看似完整的 `review.complete`。
- Integration: 显式 `review.complete` 继续进入 plan-gate，不直接进入 shipper。
- Regression: `crates/ralph-core/src/event_loop/tests/default_publishes.rs` 继续通过，证明全局 default_publishes 没被破坏。

**Verification:**
- 本次报告涉及的 preset 配置漂移能在 agent 启动前被发现。

- [ ] **Unit 6: 统一 loop 状态展示与 RPC 断开诊断**

**Goal:** 让主 loop、worktree loop 和历史产物能被可靠区分，并让 RPC stdout 断开能够从日志判断原因。

**Requirements:** R5, R10

**Dependencies:** Unit 1

**Files:**
- Modify: `crates/ralph-core/src/loop_context.rs`
- Modify: `crates/ralph-core/src/loop_registry.rs`
- Modify: `crates/ralph-core/src/diagnostics/mod.rs`
- Modify: `crates/ralph-cli/src/loops.rs`
- Modify: `crates/ralph-cli/src/rpc_stdin.rs`
- Test: `crates/ralph-core/src/loop_context.rs`
- Test: `crates/ralph-cli/src/loops.rs`
- Test: `crates/ralph-cli/src/rpc_stdin.rs`

**Approach:**
- 所有诊断 session metadata 写入 loop_id、repo_root、workspace、is_primary、events_path 和 tasks_path。
- 明确 primary loop 是否进入 `loops.json`；若设计上不进入，则 CLI 和文档不得把空 registry 解释为“无运行 loop”。
- `ralph diagnose` 和 loop 列表按 workspace/loop_id 定位产物，检测到同名 plan 的多个 loop 时要求显式选择。
- 保持 worktree runtime tasks 隔离，只在报告层按 session metadata 聚合。
- stdout write/flush 失败时记录原始 `io::Error`、error kind 和当前 RPC 生命周期状态。
- 区分 broken pipe、客户端正常 EOF、response channel 关闭和未知 I/O 错误，不把它们统一压成一句“写 stdout 失败”。
- RPC 正常事件仍逐行输出原有 JSON，不在 stdout 混入人类可读日志；新增诊断只走 tracing/diagnostics。

**Test scenarios:**
- Happy path: primary 和 worktree 同时运行时分别定位自己的 events/tasks/diagnostics。
- Edge case: 同一 plan 名存在多个 loop，诊断拒绝模糊选择。
- Edge case: primary registry 为空但 loop.lock 有效时，CLI 输出符合明确语义。
- Error path: registry workspace 与实际路径不一致时报告 stale/corrupt 状态。
- Error path: RPC reader 提前关闭时记录 EOF；stdout broken pipe 时记录错误类型，二者可在历史日志中区分。
- Regression: 正常 `get_state`、`guidance`、`steer` RPC 响应 JSON 格式不变。

**Verification:**
- 无需人工比较绝对路径即可判断事件和任务属于哪个 loop。

- [ ] **Unit 7: 端到端闭环与回归验证**

**Goal:** 证明修复后 ce-executor 在超时和非法事件场景下可恢复，并保持安全边界。

**Requirements:** R1-R11

**Dependencies:** Unit 2, Unit 3, Unit 4, Unit 5, Unit 6

**Files:**
- Modify: `crates/ralph-core/tests/scenarios/ce_executor_recovery.yml`
- Modify: `crates/ralph-e2e/src/scenarios/`
- Test: `crates/ralph-core/tests/scenarios/ce_executor_recovery.yml`
- Test: `crates/ralph-cli/tests/ce_executor_recovery.rs`

**Approach:**
- BDD 使用真实 EventLoop、origin guard、event policy、execution contract 和 task store 路径。
- mock E2E 依次模拟无事件、缺字段事件、修正事件、wave 完成和 plan-gate 完成。
- fresh worktree dogfood 只在自动化回归通过后执行，保留完整 diagnostics。

**Test scenarios:**
- Integration: 首轮 executor 无事件，第二轮合法完成，最终到达 `report.done` 和 `LOOP_COMPLETE`。
- Integration: 首轮 contract-invalid，targeted retry 后合法完成。
- Integration: 8 个混合格式 worker 结果被标准化，synthesizer 随后正常运行。
- Regression: wave backend matrix、partial-timeout、synthetic failure、ce-executor routing、default_publishes、loop registry、RPC command tests 全部保留。
- Security: review-coordinator 持续越权不能进入 review 或 ship 阶段。
- Failure: 超过同一 rejection retry 上限后生成明确终止原因和诊断摘要。
- Isolation: 并发两个 worktree 时事件、任务和恢复计数互不污染。

**Verification:**
- replay、BDD、mock E2E、smoke tests 和 workspace test 全部通过；fresh worktree 产生完整闭环事件。

## System-Wide Impact

- **Interaction graph:** JSONL parser、origin guard、event policy、execution contract、recovery responder、hard gate、prompt builder 和 EventBus 调度都会参与。
- **Error propagation:** 可修复拒绝转为有界 targeted retry；不可修复拒绝保持 fail-closed 并进入明确终态。
- **State lifecycle risks:** 重试事件重复、旧 trigger snapshot 被复用、跨 iteration 计数未重置是主要风险。
- **API surface parity:** CLI 日志、TUI/RPC diagnostics、`ralph diagnose` 和 replay 必须展示一致分类。
- **Integration coverage:** 必须覆盖真实文件 reader offset、task store、wave 聚合和 plan-gate，不能只测纯函数。
- **Unchanged invariants:** hat 只能发布声明 topic；contract-invalid 事件不可进入业务总线；runtime tasks 按 loop 隔离。
- **Wave invariant:** worker 的业务 payload 可以来自代理，但 wave 归属字段必须由框架生成。
- **Regression invariant:** 本计划修的是 ce-executor 恢复链路和旁路污染，不改变正常 wave、正常 default_publishes、正常 RPC JSON 协议。

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| targeted retry 形成无限循环 | 使用稳定 rejection key、同类计数上限和明确不可恢复终态 |
| 原触发事件快照过大或含敏感内容 | 只保存结构化引用和必要字段，沿用 event projection/redaction |
| obligation 配置增加 preset 复杂度 | 仅为无 default 且必须产生结果的 hats 显式配置，并由静态检查维护 |
| 修改 runner 核心路径引入回归 | 先加 replay characterization，再分阶段改 rejection 与 hard gate |
| 误改已工作的 wave 合并路径 | 先证明旁路来源；标准合并函数已有的成功、partial-timeout、failure 行为必须由回归测试保护 |
| 局部删除 synthesizer default 影响其他 hats | 只改 ce-executor preset 或 obligation 解释，不改全局 `check_default_publishes` 语义 |
| RPC 日志增强污染 stdout 协议 | 新增诊断只写 tracing/diagnostics，stdout 继续只输出 JSON-RPC event line |
| 当前 sleek-sparrow 有大量未提交实现 | 实施时在独立 worktree 完成，不覆盖或清理现有改动 |
| 报告中的多个次要异常缺乏复现 | 不纳入行为修复，先由 Unit 1/6 增加证据后再开独立任务 |

## Deferred to Separate Tasks

- scope violation 是否应禁止修改 orchestrator 自身代码：属于产品策略，不由本次恢复故障决定。
- aggregate timeout 策略优化：当前问题是结果关联字段缺失，不先调整 timeout。
- 通用 hat handoff 协议：当前 targeted retry 足以解决已确认问题，避免扩大范围。

## Documentation / Operational Notes

- 更新 ce-executor 使用文档，明确 operator-owned budgets 应写入 `ralph.yml`。
- 诊断输出必须同时展示 executor hat 与 business/display hat，避免把 Coordinator 模式误解为 hat 未切换。
- wave 日志必须显示预期 worker 数、实际标准化结果数、缺失 indexes 和最终 aggregate 状态。
- fresh run 验证前保留现有 `.ralph` 与两个 worktree 产物，不作清理或覆盖。
- 如修改 `ralph tools` 或其文档引用，按仓库规则同步反向验证源码行号；本计划当前不涉及这些子命令。

## Sources & References

- **Origin document:** `docs/report/2026-06-07-ce-executor-drift-plan-diagnostic-analysis.md`
- `presets/en/ce-executor.yml`
- `crates/ralph-cli/src/loop_runner/runner.rs`
- `crates/ralph-cli/src/loop_runner/hard_gate.rs`
- `crates/ralph-cli/src/preflight.rs`
- `crates/ralph-core/src/event_origin.rs`
- `crates/ralph-core/src/hat_registry.rs`
- `crates/ralph-core/src/event_loop/mod.rs`
- `crates/ralph-core/src/loop_context.rs`
- `.ralph/events-20260606-002000.jsonl`
- `.ralph/diagnostics/logs/ralph-2026-06-06T08-19-19-480-19603.log`
- `.worktrees/2026-06-04-004-feat-drift-auto-calibration-plan-sleek-sparrow/.ralph/events-20260606-001920.jsonl`
- `.worktrees/implement-dev-plan-docs-plans-2026-06-05-001-feat-runtime-contract-consolidation-md-happy-finch/.ralph/events-20260606-005140.jsonl`
