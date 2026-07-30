---
title: "fix: 区分 coordinator hat 的 task lifecycle 权限与可执行性"
date: 2026-07-30
type: fix
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
product_contract_source: ce-plan-bootstrap
plan_depth: standard
origin: docs/report/2026-07-30-parallel-forge-primary-20260730-094057-diagnosis.md
---

# fix: 区分 coordinator hat 的 task lifecycle 权限与可执行性

## Summary

`forge-dispatcher` 在 loop `primary-20260730-094057` 中越权顶替了 `executor`：它自己认领 F1 unit、写代码、commit、关 task，却一条 `exec.unit.ready` 都没派发，导致整条 wave 链路断在起点。

根因是两个叠加缺口：

1. **机制层**：prompt 注入把「能 lifecycle-mutate 某 task」误当成「该自己执行该 task」。`forge-dispatcher` 在 `tasks.coordinator_hats` 里，于是所有 `executor`-owned unit task 在它的 prompt 里被渲染成**无 `[read-only]` 标记的可认领 ready task**。
2. **preset 层**：`forge-dispatcher` 的 `instructions:` 里没有任何「不要自己实现 unit / 不要认领 unit task」的禁令。

本计划把这两层各修一处，并修正一个把该 bug 锁成预期行为的现有单测。**不扩大到新增 gate**，也不碰已在 HEAD `ba6753fa` 修掉的 fail-close 双根因。

---

## Problem Frame

### 事故链路（已核实）

| 时刻 (UTC) | 事件 | 证据 |
|---|---|---|
| 09:58:12 | `forge.worktrees.ready`，`triggered=forge-dispatcher` | `.ralph/events-20260730-094057.jsonl` |
| 09:58:30 | dispatcher activation 启动，注入 `1 ready, 9 open, 0 closed` | CLI log `ralph-...-073-11755.log` |
| 09:59:53 | F1 task `started` | `.ralph/agent/tasks.jsonl` |
| 10:36:52 | F1 task `closed` | 同上 |
| 10:37:21 | F1 worktree 分支 commit `87dc029b` | `git log forge/2026-07-29-002-.../F1` |
| 10:37:40 | 同一 activation 结束，hat-channel 0 字节 → hard gate → fail-close | CLI log + `channel-routing-fallback-2026-07-30T10-37-40.md` |

**决定性反证**：`.ralph/supervisor.db` 的 `waves` / `wave_slots` / `dispatch_records` / `wave_emissions` 表全部 0 行，`wave_id_seq=0`；主 events 流 `exec.unit.ready` 0 条。即 **wave 派发从未发生**，F1 的活是 dispatcher 在自己那一轮 activation 里直接干的，`executor` hat 一次都没被 spawn（9 次 `PtyExecutor spawned` 可逐一映射到 inspector / planner / guardian / worktree / forge-dispatcher / reporter×4）。

fail-close 是**后果**而非原因。原诊断报告将此归因为 executor 的正常行为（OPAC 表给 `executor (F1)` 打了 `A=✅`），误判了因果方向。

### 缺口 1：lifecycle ACL 被当成可执行性

`crates/ralph-core/src/event_loop/mod.rs:7107-7114` 的 `is_actionable` 闭包直接复用 `can_hat_mutate_task_lifecycle`（`crates/ralph-core/src/task.rs:227-236`，语义为 `owner 匹配 OR caller ∈ coordinator_hats`）。后果：

- `mod.rs:7115` `any_actionable_ready == true` → header 走普通分支（`mod.rs:7124-7129`），而非 `none actionable for this hat`
- `mod.rs:7138-7142` `ro_marker == ""` → F1 不带 `[read-only]` 标记

dispatcher 的 prompt 里 F1 与它自己该做的调度工作**在视觉上无法区分**。

### 缺口 2：preset 缺少禁令

`presets/en/parallel-forge.yml:580-648` 的 dispatcher `instructions:` 详述了 wave 选择与 OPAC，但通篇没有「不要自己实现 unit」。

对比反差强烈：`worktree` hat（**不在** `coordinator_hats`）在 `yml:510-513` 明确写了「那些 task 对你**只读可见，不可 mutate**」。真正拥有 mutate 权限、因而更需要边界的 dispatcher 反而没有这句话。

### 缺口 3：单测把 bug 锁成预期

`crates/ralph-core/src/event_loop/tests/build_prompt.rs:793-849` 的 `ready_tasks_no_read_only_marker_for_coordinator_hat`，fixture 精确复刻了本次事故场景（`forge-dispatcher` + `coordinator_hats: [forge-dispatcher]` + owner 为 `executor` 的 `"F1 impl"` task），并断言 `!prompt.contains("[read-only]")`。修复必须同时修正该测试，否则改动无法通过。

---

## Requirements

| ID | 需求 |
|---|---|
| R1 | `coordinator_hats` 成员在 prompt 中看到**非自己 owner** 的 task 时，必须带 `[read-only]` 标记 |
| R2 | 该标记变更不得削弱 coordinator 的实际 lifecycle 权限（`add` / `close` / `ensure` 等 CLI 路径行为不变） |
| R3 | `forge-dispatcher` 的 `instructions:` 必须显式禁止认领/实现 unit task |
| R4 | 现有 `build_prompt.rs` 三个 read-only 相关单测反映修复后的正确语义 |
| R5 | `ce-executor-supervisor` 的 `coordinator` / `task-planner` 不因本次改动出现行为回归 |
| R6 | `crates/ralph-core/data/ralph-tools-tasks.md` 与新语义一致（CLAUDE.md skill 同步硬规则） |

---

## Key Technical Decisions

### KTD-1：在 prompt 注入层引入独立判定，不改 ACL 函数

`can_hat_mutate_task_lifecycle` 保持原样——它作为 lifecycle ACL 是正确的，且被 `task_cli.rs:574-620` / `797-809` 的授权路径依赖。只在 `mod.rs` 的 prompt 注入处新增一个「可执行性」判定：**owner 匹配才算 actionable；仅靠 coordinator 身份不算**。

这样 R1 与 R2 天然不冲突：coordinator 仍可 mutate，只是 prompt 不再邀请它去执行。

### KTD-2：语义是「非 owner 即 read-only」，不引入新配置开关

考虑过加 `tasks.coordinator_hats_can_execute` 配置项，否决：两个使用 `coordinator_hats` 的 builtin preset（`parallel-forge` 的 `forge-dispatcher`、`ce-executor-supervisor` 的 `coordinator` + `task-planner`）**都是纯路由/调度角色，都不自己执行 unit task**（`ce-executor-supervisor.yml:8-23` 的拓扑注释可证）。没有正当用例需要「coordinator 自己执行」，加开关是为不存在的需求付复杂度。

若将来真出现该需求，届时再加开关，届时也会有真实用例来定义它的语义。

### KTD-3：不新增兜底 gate

考虑过加「activation 零 emit 但有 task 状态变更/文件写入 → 判违规」的 gate。本次不做：

- 现有 hard gate（`crates/ralph-cli/src/loop_runner/hard_gate.rs:11-90` + `runner.rs:4797-4817`）**已经正确触发**了（log 10:37:40 `Hard gate triggered hat=forge-dispatcher consecutive=1`）。机制侧的检测没坏。
- 新 gate 需要跨 activation 关联 task 状态与文件写入，误判风险实在，收益边际——根因修掉后 dispatcher 不会再被邀请去执行 task。

如果后续仍观察到越权，再单独立计划。

### KTD-4：`[read-only]` 标记语义随之演进

改动后 `[read-only]` 的含义从「你不能 mutate 它」变为「这不是你该执行的活」。这个语义更贴合它在 prompt 中的实际作用（引导 agent 的行为选择），也是 `mod.rs:7049-7054` 注释里那句「so the agent does not start a task that the runtime ACL will reject」的**本意延伸**——原意图就是防止 agent 动不该动的 task，只是当初用 ACL 近似了它。

---

## High-Level Technical Design

修复前后 dispatcher 的 prompt 视图：

```
【修复前】forge-dispatcher 的 <ready-tasks>
## Tasks: 1 ready, 9 open, 0 closed          ← 普通 header
- [ ] [P1] 契约冻结与 cleanup Characterization (task-...67a8) — key: forge:...:F1
                                                              ↑ 无标记 = 看起来可认领

【修复后】
## Tasks: 1 ready, 9 open, 0 closed — none actionable for this hat (all read-only)
- [ ] [P1] 契约冻结与 cleanup Characterization (task-...67a8) — key: forge:...:F1 [read-only]
                                                              ↑ 明确不是自己的活
```

判定逻辑的分叉点：

```
task 渲染
  │
  ├─ owner_hat_id == caller?  ──是──→ actionable（无标记）
  │
  └─否─→ [read-only]
          （即使 caller ∈ coordinator_hats——
            它仍可 mutate，只是不该自己执行）
```

lifecycle 权限路径（`task_cli.rs` 授权）不经过这个分叉，因此不受影响。

---

## Implementation Units

### U1. 在 prompt 注入层解耦可执行性判定

**Goal**：`coordinator_hats` 成员对非自己 owner 的 task 看到 `[read-only]`，同时保留其 lifecycle 权限。

**Requirements**：R1, R2

**Dependencies**：无

**Files**：
- `crates/ralph-core/src/event_loop/mod.rs`（约 7107-7142，`is_actionable` 闭包与两处 `ro_marker` 使用点）

**Approach**：
把 `is_actionable` 闭包的判定从 `can_hat_mutate_task_lifecycle(task, caller, coordinator_hats)` 改为仅比较 owner：`task.owner_hat_id.as_deref() == Some(caller)`。`caller_hat_str` 为 `None` 时（测试/无 hat 上下文）保持返回 `true` 的既有行为。

不要修改 `crates/ralph-core/src/task.rs` 的 `can_hat_mutate_task_lifecycle`——它服务 CLI 授权路径，语义正确。

同步更新 `mod.rs:7049-7054` 的注释，说明该闭包判断的是「该不该由本 hat 执行」而非「能不能 mutate」，并点明二者已刻意分离。

**Patterns to follow**：`mod.rs:7115-7130` 的 `any_actionable_ready` → header 分支结构无需改动，它会自动因判定变化走到 `none actionable` 分支。

**Test scenarios**（改在 U3，此处只列语义）：
- coordinator hat + 非自己 owner 的 task → 渲染带 `[read-only]`，header 含 `none actionable for this hat`
- coordinator hat + 自己 owner 的 task → 无标记
- 非 coordinator 非 owner 的 hat → 带 `[read-only]`（现有行为不变）
- owner hat 看自己的 task → 无标记（现有行为不变）
- `caller_hat_str == None` → 全部 actionable（现有行为不变）

**Verification**：`cargo nextest run -p ralph-core -- build_prompt` 通过（U3 完成后）；`task_cli` 授权相关测试不受影响。

---

### U2. 补 `forge-dispatcher` instructions 的执行边界禁令

**Goal**：dispatcher 明确知道自己不实现 unit、不认领 unit task。

**Requirements**：R3

**Dependencies**：无（可与 U1 并行）

**Files**：
- `presets/en/parallel-forge.yml`（`forge-dispatcher.instructions`，约 580-648）

**Approach**：
在 `### Single-shot budget` 段之前插入一段简短的角色边界声明，内容覆盖三点：

- 你只做**派发**：读 execution plan、选 wave、`ralph wave emit exec.unit.ready`。你不实现任何 Unit。
- prompt 里的 `<ready-tasks>` 对你**只读可见**：unit task 的 owner 是 executor，你可能拥有 lifecycle 权限用于协调，但**不要** `task start` 去认领、不要写 Unit 代码、不要 commit Unit 产物。
- 如果本轮无法派发（无 open wave、payload 构造失败等），发你允许的失败/推进事件，**不要**改为自己动手完成 Unit。

按 CLAUDE.md HARD RULE 4 用 hat 自视角措辞：只说「你该做什么 / 不该做什么 / 做完发什么」，不解释 `coordinator_hats` / `is_actionable` 等框架实现细节。涉及命令语法时引用 `ralph-tools-wave` red box，不复述内容。

参照 `yml:510-513`（`worktree` hat 的只读提示）的措辞风格与长度，保持简短。

**Patterns to follow**：`presets/en/parallel-forge.yml:510-513`

**Test scenarios**：`Test expectation: none — preset 文本改动`。按 CLAUDE.md「Preset 测试规则」，不新增校验 instructions 文本包含某段字符串的测试。结构化约束由 U4 的 preset lint 全量校验覆盖。

**Verification**：`cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 与 `-- presets` 通过；`ralph preset check parallel-forge --strict` 无新增 finding。

---

### U3. 修正被 bug 锁定的单测并补齐语义覆盖

**Goal**：`build_prompt.rs` 的 read-only 单测反映修复后的正确语义。

**Requirements**：R4

**Dependencies**：U1

**Files**：
- `crates/ralph-core/src/event_loop/tests/build_prompt.rs`（677-849）

**Approach**：
重写 `ready_tasks_no_read_only_marker_for_coordinator_hat`（793-849）。它的 fixture 恰好是本次事故场景，断言方向反了。改为断言 coordinator 看到非自己 owner 的 task **带** `[read-only]` 标记且 header 含 `none actionable for this hat`，并把函数名与注释改为陈述新语义（当前注释「they must see actionable rendering」正是被修掉的错误假设）。

新增一个测试覆盖「coordinator 看自己 owner 的 task」——确认改动没有把 coordinator 一律降级为只读。这是 U1 判定的正向边界，现有三个测试都没覆盖。

`ready_tasks_marks_non_mutable_tasks_read_only_for_non_coordinator_hat`（677）与 `ready_tasks_no_read_only_marker_for_owner_hat`（738）预期不变，跑通即可，作为无回归证据。

**Execution note**：先改测试断言到期望语义、确认它对着当前代码失败（红），再让 U1 的改动使其转绿。这样能证明测试真的在守护这个行为，而不是恰好通过。

**Patterns to follow**：`build_prompt.rs:677-736` 的 fixture 构造与断言风格。

**Test scenarios**：
- coordinator hat + owner=`executor` 的 task → prompt 含 `[read-only]`，header 含 `none actionable for this hat`
- coordinator hat + owner=该 coordinator 自己的 task → prompt 不含 `[read-only]`，header 不含 `none actionable`
- coordinator hat 同时有自己的 task 和别人的 task → 前者无标记、后者有标记，header 走普通分支（存在至少一个 actionable）
- 非 coordinator 非 owner hat → 含 `[read-only]`（回归保护）
- owner hat 看自己 task → 不含 `[read-only]`（回归保护）

**Verification**：`cargo nextest run -p ralph-core -- build_prompt` 全绿；先红后绿的过程可复述。

---

### U4. 同步 skill doc 与全量门禁

**Goal**：注入给 agent 的 task 指南与新语义一致；全量测试基线绿。

**Requirements**：R5, R6

**Dependencies**：U1, U2, U3

**Files**：
- `crates/ralph-core/data/ralph-tools-tasks.md`（74-104，`Cross-Loop and Cross-Hat Authorization` 段）

**Approach**：
该段第 84-86 行目前只讲 mutate 权限（「only the task's owner hat (or any hat listed in `tasks.coordinator_hats`) may mutate it」），与修复后的 prompt 行为之间存在一处 agent 可感知的落差：coordinator 会看到带 `[read-only]` 的 task，却在文档里读到自己「可以 mutate」，无从判断该不该动手。

补一句区分：coordinator 身份给的是**协调用的 lifecycle 权限**；prompt 里带 `[read-only]` 的 task 表示**不该由本 hat 执行**。按 CLAUDE.md 注入 skill 规则写成通用规则，不提 `parallel-forge` / `forge-dispatcher` / 本次事故、不带 plan 编号、不泄漏 `is_actionable` 等内部函数名。

反向验证（CLAUDE.md 硬规则）：检查该文件内所有 `xxx.rs:NN-MM` 形式的源码引用是否仍准确；跑 `scripts/check-cli-doc-drift.sh`。

`ce-executor-supervisor` 的 `coordinator` / `task-planner` 不需改 preset——U1 后它们对非自己 owner 的 task 变为只读可见，与其纯路由/调度定位一致（`ce-executor-supervisor.yml:8-23`）。R5 通过全量测试验证而非改动验证。

**Test scenarios**：`Test expectation: none — 文档改动`。行为覆盖已由 U3 承担。

**Verification**：
- `./scripts/run-tests.sh` 全绿（含 preset_lint、scenarios、doctest）
- `scripts/check-cli-doc-drift.sh` 无新增 drift
- 带污染环境复跑一次（CLAUDE.md HARD RULE 5）：`RALPH_CURRENT_HAT=executor RALPH_EVENTS_FILE=/tmp/x.jsonl cargo nextest run -p ralph-core -- build_prompt`

---

## Verification Contract

| 门禁 | 命令 | 通过标准 |
|---|---|---|
| 单元语义 | `cargo nextest run -p ralph-core -- build_prompt` | 5 个场景全绿；U3 可复述先红后绿 |
| preset 结构 | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` + `-- presets` | 无新增 finding |
| preset strict | `ralph preset check parallel-forge --strict` | 无新增 finding |
| 全量基线 | `./scripts/run-tests.sh` | 全绿（flake 时走 `RALPH_BASELINE_SERIAL=1`） |
| 文档漂移 | `scripts/check-cli-doc-drift.sh` | 无新增 drift |
| env 污染 | `RALPH_CURRENT_HAT=executor ... cargo nextest run -p ralph-core -- build_prompt` | 绿 |

---

## Definition of Done

- [ ] coordinator hat 在 prompt 中对非自己 owner 的 task 看到 `[read-only]`，对自己的 task 不看到
- [ ] coordinator 的 CLI lifecycle 权限（`add` / `start` / `close` / `ensure`）行为无变化
- [ ] `forge-dispatcher` instructions 含执行边界禁令，措辞为 hat 自视角
- [ ] `build_prompt.rs` 的 coordinator 测试断言正确语义，函数名与注释同步；新增自有 task 的正向覆盖
- [ ] `ralph-tools-tasks.md` 区分 lifecycle 权限与执行边界，无 plan/preset/事故特定内容
- [ ] Verification Contract 全部门禁通过

---

## Scope Boundaries

### 本次范围内
- prompt 注入层的可执行性判定（U1）
- `parallel-forge` dispatcher 的 instructions 边界（U2）
- 相关单测修正与补齐（U3）
- skill doc 同步与全量门禁（U4）

### 明确不做
- **新增「零 emit + 有副作用」兜底 gate**：见 KTD-3。现有 hard gate 已正确触发，根因修掉后收益边际、误判风险实在。
- **改 `can_hat_mutate_task_lifecycle`**：ACL 语义正确，改它会波及 `task_cli.rs` 授权路径。
- **新增 `coordinator_hats_can_execute` 配置**：见 KTD-2，无真实用例。
- **fail-close 双根因**：已在 HEAD `ba6753fa` 修掉（`derive_blocked_topic` + `resolve_escape_step` + `run_stall_detector_with_authority_advance` 已在位）。

### 延后到后续工作
- **fail-close 路径 BDD scenario**：`ba6753fa` 的修复没有配套 BDD 覆盖，`crates/ralph-core/tests/scenarios/parallel_forge_*.yml` 无 `consecutive_no_progress → forge.plan.blocked` 场景。本次按最小化原则不纳入，建议独立立计划，用 `run_workflow_guard_scenario`（禁用 `run_scenario` stub）。
- **`recovery.jsonl` 的 `topic` 字段格式异常**：原诊断 DEV-005，`topic` 被写成 stringified payload JSON。与本根因无关，需独立定位 `repair_sink` 写盘路径。
- **`hat_lifecycle` activation key 失配**：原诊断 DEV-007（`primary:1:inspector, completed_count=0`），与本次终态失败无因果。
- **`loops.json` 残留 pid 11768**：运维清理，`ralph loops clean`。

---

## Operational Notes

事故残留（不属本计划改动，但实施前需知晓）：

- F1 worktree（`.worktrees/2026-07-29-002-feat-parallel-forge-reuse-status-F1`）分支上有 commit `87dc029b`，**未 merge** 到 `pittcat-dev`。处置前不要 `git worktree remove`，否则丢该 commit。
- 该 commit 是 dispatcher 越权产物，但其代码内容本身可能有效（F1 的 DTO 冻结）。是否保留由操作者判断，不在本计划范围。
- `.ralph/loops.json` 残留 pid 11768 记录，`ralph loops clean` 清理。

---

## Sources & Research

- 诊断报告：`docs/report/2026-07-30-parallel-forge-primary-20260730-094057-diagnosis.md`（其 P0 已过期，executor 归因有误——本计划 Problem Frame 给出修正）
- 运行时产物：`.ralph/events-20260730-094057.jsonl`、`.ralph/agent/tasks.jsonl`、`.ralph/ledger.jsonl`、`.ralph/supervisor.db`、`.ralph/diagnostics/logs/ralph-2026-07-30T17-40-57-073-11755.log`
- 机制源码：`crates/ralph-core/src/event_loop/mod.rs:7049-7196`、`crates/ralph-core/src/task.rs:227-256`、`crates/ralph-cli/src/task_cli.rs:574-620,797-809`、`crates/ralph-cli/src/loop_runner/hard_gate.rs:11-90`、`crates/ralph-cli/src/loop_runner/runner.rs:4797-4817`
- Preset：`presets/en/parallel-forge.yml:143-147,510-513,552-648`、`presets/en/ce-executor-supervisor.yml:8-23,202-210`
- 现有测试：`crates/ralph-core/src/event_loop/tests/build_prompt.rs:677-849`
- 相关 memory：`ce-executor-task-ownership`（同构的 task 所有权/ACL 混淆问题）
