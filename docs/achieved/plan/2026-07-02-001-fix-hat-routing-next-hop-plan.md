---
title: "fix: 修复 isolated 模式下一跳选路错配与 mechanism.flow 配置丢失"
type: fix
status: active
date: 2026-07-02
---

# fix: 修复 isolated 模式下一跳选路错配与 mechanism.flow 配置丢失

## Overview

修复 e2e 运行 `ce-executor-serial` preset 时定位到的一组机制层缺陷：第二个 unit（step-02）没有经过 coordinator 派活，executor 自己抢先建 task 并执行，破坏了 `coordinator → executor → validator` 的串行编排。

根因是三个 runtime（非编排）缺陷叠加，本计划按"两两分组、两个 commit"落地：

- **Phase 1（Commit 1，P0，治症状）**：修 WAC-U5 优先级抢占的"主题/内容错配"（直接凶手），并用回归场景钉死"重复事件 → 残留 `task.resume` → 下一跳必须回 coordinator"。
- **Phase 2（Commit 2，P1/P2，补护栏）**：根治 preset 加载时 `mechanism:` 块被多份分叉 key 列表丢弃（导致 `FlowStepScopeStage` 空转的越界护栏被关掉），并补 `flow_type` 的 serde rename 潜伏坑。

**不在本计划范围**：把选路整体换成"事件路由表"的战略机制（另开 spec）。本计划只修已定位的三个问题。

---

## Problem Frame

e2e 运行产物（`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/`，仅作证据，不属本仓）显示：

- `tasks.jsonl`：step-01 的 `owner_hat_id=coordinator`（正确），step-02 的 `owner_hat_id=executor` 且创建于 executor 自己那一轮 → 说明 iter4 是 executor 在跑，coordinator 被跳过。
- 事件文件全程只有一条 `work.ready`（step-01），从无 `work.ready(step-02)`。
- runtime 日志首行报 `preset YAML has no mechanism.flow block`，但源 preset `presets/en/ce-executor-serial.yml` 明明有完整 `mechanism.flow` 块，且二进制是当前源码新编译的（非版本问题）。

三个缺陷（详见根因链）：

1. **触发源（回归，commit `62a40b41` 2026-07-01 08:44 引入）**：isolated 单事件预算丢弃"多余业务事件"时，向源 hat 注入一条 targeted `task.resume` 作为背压反馈。iter2 executor 多吐一条 `work.done` 被丢，于是 executor 队列里被塞了一条一直没被消费的 `task.resume`。
2. **直接凶手（老隐患，commit `84b828147` 2026-06-12 引入，长期休眠）**：`EventLoop::next_hat` 的优先级抢占谓词按"该 handoff 主题的 **consumer 收件箱非空**"判定抢占资格，而非"consumer 收件箱里**确有该主题的 pending**"。`fix.exhausted`（consumer=executor）字母序排在 `test.passed`（consumer=coordinator）之前，于是 executor 队列里那条无关 `task.resume` 让扫描把 executor 当 priority hat，短路抢占，挤掉 coordinator 合法待处理的 `test.passed(step-01)`。纯轮询本会正确选中 coordinator。
3. **被卸掉的护栏（独立老洞，随 2026-06-27 mechanism 基础设施引入）**：`extract_hat_overlay_from_preset` 用一份硬编码顶层 key 白名单重建 overlay，**漏了 `mechanism`**（而校验用的 `ALLOWED_HATS_TOP_LEVEL` 里有）→ `config.mechanism=None` → `build_stage_pipeline_from_config` 回退到全放行的 `minimal_flow_declaration_yaml` → `FlowStepScopeStage` 空转，executor 越界 emit `work.done(step-02)` 无人拦下。同时这也是那条 warning 的来源。

分类结论：**全部为机制（runtime 代码）缺陷，编排（preset）正确。**

---

## Requirements Trace

- R1. isolated 模式下，某 handoff 主题的唯一 consumer **只有在该主题确实 pending 时**才可优先抢占；无关残留事件（如 targeted `task.resume`）不得触发抢占。
- R2. 复现原生产事故的回归覆盖必须存在：`work.done` 重复 → 残留 `task.resume` 场景下，`test.passed` 之后的下一跳必须是 coordinator。
- R3. preset 顶层 `mechanism:` 块必须在 `-H builtin:` 加载路径全程存活，使 `config.mechanism.flow` 为 `Some`，恢复 `FlowStepScopeStage` 的 step-scope 强制。
- R4. 消除"多份顶层 key 列表手工同步"的反模式：任何 preset 可设的顶层字段在加载 round-trip 后不得被静默丢弃，且有测试门自动兜底。
- R5. `FlowDeclarationConfig` 能正确反序列化 preset 的 `type:` 字段（补 `rename="type"`），关闭"非默认 flow type 被静默丢成默认值"的潜伏坑。

---

## Scope Boundaries

- 不引入"事件路由表"战略机制（另开 `.ralph/specs` 设计文档）。
- 不回滚 `62a40b41` 的 targeted `task.resume` 背压反馈——它本身是合法机制，修好 R1 后即不再捣乱（见 Key Technical Decisions）。
- 不改 `crates/ralph-proto/src/event_bus.rs` 的 `select_next_hat_with_pending` 函数签名（保持通用调度原语与 handoff 策略分层，见 Key Technical Decisions）。
- 不给注入的恢复类事件加 TTL / "激活即消费或过期"语义（记为 P3 后续，见 Open Questions）。

### Deferred to Follow-Up Work

- 恢复类事件（`task.resume` 等）在业务队列中的生命周期治理（TTL / 过期）：后续单独处理，P3。
- 事件路由表机制：另开 spec 与实施计划。

---

## Context & Research

### Relevant Code and Patterns

- `crates/ralph-core/src/event_loop/mod.rs` — `EventLoop::next_hat`（isolated 分支的 `priority_hat` 计算，约 2732-2752）、`build_stage_pipeline_from_config`（约 438-479）、`minimal_flow_declaration_yaml`（约 341-417）、per-turn 预算丢弃处注入 `task.resume` 的段落（约 8186-8256，`62a40b41` 引入）。
- `crates/ralph-proto/src/event_bus.rs` — `select_next_hat_with_pending`（priority 短路 258-267 + 轮询扫描 274-311）、`publish`（按订阅路由 83-164）、`peek_pending`（177-179）。
- `crates/ralph-core/src/event_loop/handoff_index.rs` — `handoff_index` 构造（约 125-218），topic→唯一 consumer 的 `BTreeMap`（这是"半成品路由表"）。
- `crates/ralph-cli/src/preflight.rs` — `extract_hat_overlay_from_preset`（约 789-824，key 列表 804-816）、`merge_hats_overlay`（约 888-1077）、`ALLOWED_HATS_TOP_LEVEL`（660-696，**含** `mechanism`）、`PRESET_OPT_IN_WHEN_OPERATOR_OMITS`（742）。
- `crates/ralph-cli/src/config_resolution.rs` — `PRESET_OPT_IN_KEYS`（约 65）同族手工列表。
- `crates/ralph-core/src/config/mod.rs:130` — `RalphConfig.mechanism: Option<MechanismConfig>`（`#[serde(default)]`）。
- `crates/ralph-core/src/config/loop_config.rs` — `MechanismConfig`（约 595-603）、`FlowDeclarationConfig`（约 614-630，`flow_type` 在 616 **缺** `rename="type"`）。
- `crates/ralph-core/src/event_loop/flow_declaration.rs:165` — `FlowDeclaration::from_yaml`（查找 `mechanism.flow`，缺则返回 `MissingMechanismFlow`）；`flow_type` 在 92 行有 `rename="type"`（与 config 侧不一致）。

### Institutional Learnings

- `AGENTS.md`「preset/schema 改动后的下游同步清单 HARD RULE」与 `crates/ralph-cli/src/preflight.rs:1946-1978` 注释，本身即在文档化"多份顶层 key 列表必须手工同步"的负担——已被证伪：`mechanism` 在校验列表有、抽取/合并列表无，正是这次 bug。教训：手工同步机制不可靠，必须用测试门替代。
- `crates/ralph-core/src/event_loop/mod.rs:2687` 注释早已记录 round-robin "would otherwise drift to executor" 的隐患，但未归因到抢占谓词。

### External References

- 无需外部研究：纯内部编排机制，本仓已有充分的 event_bus / preset_lint / BDD 测试范式可循。

---

## Key Technical Decisions

- **抢占策略留在 `next_hat`，`event_bus` 原语保持策略无关**：handoff 优先级是策略，`select_next_hat_with_pending` 是通用调度原语。把 topic 精确判定放进 `next_hat` 的 `priority_hat` 计算处即修复机制本身；不把 topic 感知泄漏进 `event_bus`（那是分层污染）。原语侧仅补契约 doc + 契约测试兜底未来调用方。
- **不回滚 `62a40b41`**：targeted `task.resume` 是合法背压反馈，会在目标 hat 下次激活 `take_pending` 时消费。修好 R1 后它不再污染抢占；轮询仍可因它选中该 hat（这是期望行为——该 hat 本就该被唤起处理 resume）。改为用回归场景钉死其与选路的交互。
- **Fix C 用"单一事实源 + 通用默认分支 + 完整性测试门"，而非"再加一个 key"**：抽取列表从 `ALLOWED_HATS_TOP_LEVEL` 派生，杜绝抽取 vs 校验分叉；`merge_hats_overlay` 给无特殊合并语义的 key 一个通用默认（整块 replace/insert），特殊语义（`event_loop` 深合并、`topic_format_whitelist`/`telemetry` union）保留显式分支；再加 serde round-trip 完整性测试，任何顶层字段被丢即报红。
- **Fix D 补 `#[serde(rename = "type")]`**：与 `event_loop::flow_declaration::FlowDeclaration.flow_type` 对齐，关闭"非默认 flow type 被静默丢成默认值"的潜伏坑。
- **可观测**：抢占命中处补一条 `debug`/`trace`（`priority pre-empt: topic=<T> consumer=<C>`），下次 drift 有痕可查。

---

## Open Questions

### Resolved During Planning

- 是否回滚 `62a40b41`？→ 否，见 Key Technical Decisions。
- 是否改 `event_bus` 短路使其 topic-aware？→ 否，分层污染；策略修在 `next_hat`，原语补契约测试。
- 修复症状是否需要同时修 Fix C？→ Fix A 单独即可治"跳过 coordinator"的症状；Fix C 修的是独立的"护栏被关"，分到 Phase 2，降低单次 blast radius。

### Deferred to Implementation

- `next_hat` 修改后其它 preset/scenario 的迭代顺序是否有可接受的变化：需在实现时跑全量 BDD guard 场景观察实际事件序列后确认。
- `merge_hats_overlay` 通用默认分支对既有特殊分支 key 是否有交叠副作用：实现时以完整性测试 + 既有 preflight 测试验证。

---

## High-Level Technical Design

> *以下用于向评审传达修复方向，是指导性说明、非实现规范。实现者应视其为上下文，而非照抄的代码。*

Fix A 的判定改动（伪代码，示意）：

```
# 现状（错）：consumer 收件箱非空即抢占
priority_hat = first handoff entry (by topic) where
                 peek_pending(entry.consumer) is non-empty

# 目标（对）：consumer 收件箱里确有该主题的 pending 才抢占
priority_hat = first handoff entry (topic T, consumer C) where
                 peek_pending(C) contains an event whose topic == T
```

配合本次事故的时间线（Fix A 修好后应得到的正确走向）：

```
iter3 validator 发 test.passed(step-01) → 路由到 coordinator 队列
iter4 next_hat：executor 队列仅有残留 task.resume（非 handoff 主题）
      → 不再触发抢占 → 纯轮询绕回 coordinator（字母序 coordinator<executor）
      → coordinator 上场，发 work.ready(step-02) ✓
```

---

## Implementation Units

- [ ] U1. **Fix A：next_hat 优先级抢占改为主题精确判定 + 可观测 + 原语契约**

**Goal:** 让 handoff 优先级抢占只在"该 handoff 主题的 pending 确实存在于其唯一 consumer 队列"时触发，消除被无关残留事件（如 targeted `task.resume`）误导的选路。

**Requirements:** R1

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`next_hat` 的 `priority_hat` 计算：把"consumer 队列非空"改为"consumer 队列含 topic==该 handoff 主题的事件"；抢占命中补 `debug`/`trace` 日志）
- Modify: `crates/ralph-proto/src/event_bus.rs`（不改 `select_next_hat_with_pending` 签名/逻辑，仅补 doc 契约：调用方须保证传入的 `priority_hat` 是真正 eligible 的 consumer）
- Test: `crates/ralph-proto/src/event_bus.rs`（`#[cfg(test)] mod tests` 内新增抢占契约测试）
- Test: `crates/ralph-core/src/event_loop/tests/coordinator_dispatch_coverage.rs`（新增 `next_hat` 主题精确抢占单测）

**Approach:**
- 在 `next_hat` 的 `handoff_index.entries` 遍历里，对每个 `(topic T, consumer C)`，用 `peek_pending(C)` 判断队列中是否存在 `event.topic == T` 的事件；只有存在才把 C 作为 `priority_hat` 候选。
- `event_bus` 原语保持通用；其短路仍信任 `priority_hat`，但由 `next_hat` 保证其正确性。补 doc 明确该契约。
- 抢占命中时打印 `priority pre-empt: topic=<T> consumer=<C>`（`debug` 或 `trace`）。

**Execution note:** 先写失败单测（executor 队列塞非 handoff 主题事件 + coordinator 有 `test.passed`，断言 `next_hat` 返回 coordinator），再改实现。

**Patterns to follow:**
- 既有 `crates/ralph-proto/src/event_bus.rs` 抢占测试（`priority_advances_cursor_for_next_round`、`priority_dispatch_selects_immediately` 等）。
- `crates/ralph-core/src/event_loop/tests/coordinator_dispatch_coverage.rs` 既有 `next_hat` 测试范式。

**Test scenarios:**
- Happy path：coordinator 队列有 `test.passed`、executor 队列有一条 `work.ready`（handoff 主题）→ 抢占正确选中 executor（保留 WAC-U5 原意）。
- Edge case（核心回归）：coordinator 队列有 `test.passed`、executor 队列**只有** `task.resume`（非 handoff 主题）→ `next_hat` 必须返回 coordinator，不得抢占 executor。
- Edge case：executor 队列有 `work.ready` **且** coordinator 队列有 `test.passed` → 按字母序/契约确定的抢占对象；断言结果确定且不回归。
- Edge case：无任何 handoff 主题 pending、仅有非 handoff 残留事件 → 回落纯轮询，游标行为不变。
- Integration：抢占命中时产生可观测日志（可用日志捕获或对决策路径断言）。

**Verification:**
- 新单测通过；既有 `event_bus` 抢占/轮询测试全绿；无 `priority_hat` 被"非空但主题不符"误判的路径残留。

---

- [ ] U2. **Fix B：复现原生产事故的端到端回归场景（不回滚 62a40b41）**

**Goal:** 用真 EventLoop runner 的 BDD 场景钉死完整事故链：executor 重复 `work.done` → 系统注入 targeted `task.resume` → `test.passed` 之后下一跳必须回 coordinator（而非 executor 抢先跑 step-02）。

**Requirements:** R2

**Dependencies:** U1

**Files:**
- Create: `crates/ralph-core/tests/scenarios/2026-07-02-hat-routing-next-hop.yml`（新 BDD 场景）
- Modify: `crates/ralph-core/tests/scenarios.rs`（用 `run_workflow_guard_scenario` 注册并断言事件序列）
- Modify: `crates/ralph-core/src/event_loop/tests/isolated_complex_regression.rs`（补一条聚焦"残留 task.resume 不夺 coordinator 下一跳"的集成回归）

**Approach:**
- 场景 mock 出：coordinator 发 `work.ready(step-01)` → executor 发**两条** `work.done(step-01)`（触发单事件预算丢弃 + `task.resume` 注入）→ validator 发 `test.passed(step-01)`；断言**下一次激活为 coordinator** 且随后出现 `work.ready(step-02)`，断言事件序列中不出现"executor 在 coordinator 之前产出 step-02 的 `work.done`"。
- 必须用 `run_workflow_guard_scenario`（真 runner、断言 events），**禁用 `run_scenario` stub**（stub 只查 iterations 数、会静默吞掉拓扑失配）——遵循 `AGENTS.md` 下游同步清单 HARD RULE。

**Execution note:** 该场景应在 U1 未修时**失败**（复现跳过 coordinator），U1 修后**通过**——以此证明修复触及根因而非症状。

**Patterns to follow:**
- `crates/ralph-core/tests/scenarios/2026-06-29-007-u2-flow-step-scope-bypass.yml` 等既有 guard 场景结构；`crates/ralph-core/tests/scenarios.rs` 的 `run_workflow_guard_scenario` 调用范式。
- `crates/ralph-core/tests/scenarios/isolated_boundary_violation.yml`（`62a40b41` 触碰过，含单事件预算/丢弃语义）。

**Test scenarios:**
- Covers R2. 事故链复现：dup `work.done` → 残留 `task.resume` → `test.passed` 后下一跳 = coordinator；出现 `work.ready(step-02)`；无 executor 越序 step-02。
- Edge case：无重复 `work.done`（无残留 `task.resume`）的正常链路仍按 coordinator→executor→validator 走（防止过度修正）。

**Verification:**
- 新场景在 U1 之前红、之后绿；`cargo nextest run -p ralph-core --test scenarios` 全绿。

---

- [ ] U3. **Fix C：消除 overlay 顶层 key 列表分叉 + round-trip 完整性测试**

**Goal:** 让 preset 顶层 `mechanism:`（及所有可设顶层字段）在 `-H builtin:` 加载全程存活，恢复 `FlowStepScopeStage` 强制；并从机制上杜绝"多份手工同步列表"再次漏字段。

**Requirements:** R3, R4

**Dependencies:** None（与 U1/U2 独立，属 Phase 2）

**Files:**
- Modify: `crates/ralph-cli/src/preflight.rs`（`extract_hat_overlay_from_preset` 的抽取 key 集合改为从 `ALLOWED_HATS_TOP_LEVEL` 派生；`merge_hats_overlay` 增加"无特殊合并语义 key 走通用默认（整块 replace/insert）"分支，`mechanism` 走默认）
- Modify: `crates/ralph-cli/src/config_resolution.rs`（核对 `PRESET_OPT_IN_KEYS` 是否需同步；如仅影响 `event_loop` 子键则记录说明）
- Test: `crates/ralph-cli/src/preflight.rs`（`#[cfg(test)]` 内新增 round-trip 完整性测试）

**Approach:**
- 抽取列表不再硬编码独立字面量，而是复用 `ALLOWED_HATS_TOP_LEVEL`（去掉纯元数据 `name`/`description` 如原逻辑所需），保证抽取 ⊇ 校验允许集。
- `merge_hats_overlay` 保留 `hats`/`events`/`tasks`/`event_loop`/`topic_format_whitelist`/`telemetry` 的既有特殊合并；对不在特殊集里的允许 key（如 `mechanism`）走通用"整块写入 core_mapping"默认分支。
- 完整性测试：取内嵌 `ce-executor-serial` preset 字符串，走真实 `load_hats_value → merge → deserialize` 路径，断言 `config.mechanism.flow.is_some()`；并对 `RalphConfig` 每个 preset 可设顶层字段做"设置→加载→仍在"的断言。删除抽取列表任一 key 该测试须变红。

**Execution note:** 先写完整性测试（当前应因 `mechanism` 被丢而红），再改抽取/合并使其变绿。

**Patterns to follow:**
- `crates/ralph-cli/src/preflight.rs` 既有 overlay 测试（约 1946-1978、2305 附近）与 `merge_hats_overlay` 的 `telemetry` union 分支（约 1070-1077）作为"通用/特殊分支"参照。
- `presets/en/ce-executor-serial.yml` 的 `mechanism.flow` 作为断言基准。

**Test scenarios:**
- Covers R3. 内嵌 `ce-executor-serial` 经完整加载路径后 `config.mechanism.flow` 为 `Some`（step 数 = 源 preset 声明数）。
- Covers R4. 顶层字段完整性：对每个可设顶层字段（含 `mechanism`）断言 round-trip 后不丢；人为从抽取集移除某 key → 测试红。
- Edge case：operator 的 `ralph.yml` 同时提供 `mechanism` 与 preset 提供 `mechanism` 时的合并结果符合预期（默认分支的 replace/insert 语义明确）。
- Integration：加载后不再触发 `preset YAML has no mechanism.flow block` 警告（可对 preset_lint 输出断言）。

**Verification:**
- 完整性测试红→绿；`cargo nextest run -p ralph-cli --bin ralph -- preflight`（overlay/round-trip 子集）+ `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 全绿；运行 `ce-executor-serial` 不再出现 mechanism.flow 缺失警告。

---

- [ ] U4. **Fix D：FlowDeclarationConfig.flow_type 补 serde rename**

**Goal:** 关闭 config 侧 `flow_type` 无 `rename="type"` 导致 preset 的 `type:` 被忽略、静默取默认值的潜伏坑。

**Requirements:** R5

**Dependencies:** U3（同属 Phase 2，随 C 一起 commit）

**Files:**
- Modify: `crates/ralph-core/src/config/loop_config.rs`（`FlowDeclarationConfig.flow_type` 加 `#[serde(rename = "type")]`，与 `event_loop::flow_declaration::FlowDeclaration.flow_type` 对齐）
- Test: `crates/ralph-core/src/config/loop_config.rs`（`#[cfg(test)]` 或就近测试模块新增反序列化断言）

**Approach:**
- 仅加 serde 属性；不改字段名/默认值逻辑。

**Test scenarios:**
- Happy path：反序列化含 `type: declared` 的 `mechanism.flow` → `flow_type == "declared"`（来自 YAML，而非默认兜底）。
- Edge case：给一个非默认 `type` 值（哨兵）→ `flow_type` 等于该哨兵值（证明不再被静默丢成默认）。

**Verification:**
- 新反序列化测试通过；`cargo nextest run -p ralph-core -- flow_declaration`（config 侧）相关子集全绿。

---

## System-Wide Impact

- **Interaction graph:** U1 改的是所有 isolated preset 的 hat 选择路径（`next_hat`），blast radius 覆盖全部多 hat 串行/并行 preset 的迭代顺序。
- **Error propagation:** 无新错误路径；U1 使抢占更保守（更少误抢占），最坏情况回落到纯轮询（既有正确行为）。
- **State lifecycle risks:** 残留 `task.resume` 仍会长期占用队列（P3 后续），本计划不改其生命周期，仅使其不再污染抢占。
- **API surface parity:** `event_bus.select_next_hat_with_pending` 签名不变，其它调用方不受影响；仅补契约 doc。
- **Integration coverage:** U2 的 guard 场景覆盖"选路 + 单事件预算 + task.resume 注入"的跨层交互，是单元 mock 证不出的部分。
- **Unchanged invariants:** 不改 preset 语义、不改 `62a40b41` 的背压反馈行为、不改 `event_bus` 轮询算法与游标语义。

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| U1 改选路顺序，可能改变其它 preset/scenario 的迭代顺序 | 跑全量 `crates/ralph-core/tests/scenarios`（guard runner）+ `./scripts/run-tests.sh`；对比事件序列，确认仅收敛掉误抢占而非改变正确编排 |
| U3 的 `merge_hats_overlay` 通用默认分支与既有特殊分支交叠 | 完整性测试 + 既有 preflight 测试双重覆盖；特殊语义 key 显式分支优先，默认分支只兜未列出的允许 key |
| 测试入口误用裸 `cargo test -p ralph-cli` 触发 loop_runner Mutex flake | 严格遵守 `AGENTS.md` HARD RULE 1/2：全程 `cargo nextest run`；ralph-cli 走串行、其它包并行 |
| BDD 用 stub 而非真 runner 静默吞失配 | U2 强制 `run_workflow_guard_scenario`，禁 `run_scenario` |
| skill guide 漂移（若行为面向 agent 变化） | 本计划不改 `ralph` 子命令/事件类型/工作流，无需改 `crates/ralph-core/data/*.md`；实现后按 `scripts/check-cli-doc-drift.sh` 复核确认无漂移 |

---

## Phased Delivery

### Phase 1（Commit 1，P0，治症状）
- U1（Fix A：主题精确抢占 + 可观测 + 原语契约）
- U2（Fix B：端到端回归场景，先红后绿）
- 目标：`ce-executor-serial` step-02 重新经过 coordinator 派活。

### Phase 2（Commit 2，P1/P2，补护栏）
- U3（Fix C：消除 overlay key 分叉 + 完整性测试门，恢复 FlowStepScopeStage）
- U4（Fix D：flow_type serde rename）
- 目标：越界护栏通电、mechanism.flow 警告消失、杜绝顶层字段再被静默丢弃。

---

## Documentation / Operational Notes

- 两个 commit 落地后跑最终基线 `./scripts/run-tests.sh`（nextest + doctest）；如遇竞态 flake 用 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底。
- 无 CLI/事件/工作流面向 agent 的行为变化，无需同步 `crates/ralph-core/data/*.md`；仍按惯例跑 `scripts/check-cli-doc-drift.sh` 确认。
- 可在 `docs/solutions/` 记录本次"选路启发式被残留事件误导"的根因与教训（可选，后续用 `ce-compound`）。

---

## Sources & References

- 证据（外部 e2e 产物，非本仓）：`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/events-20260701-152639.jsonl`、`tasks.jsonl`、`diagnostics/logs/ralph-2026-07-01T23-26-39-228-3350.log`
- 回归引入 commit：`62a40b41`（2026-07-01 08:44，task.resume 注入）
- 休眠隐患引入 commit：`84b828147`（2026-06-12，WAC-U5 优先级抢占）
- 相关代码：`crates/ralph-core/src/event_loop/mod.rs`、`crates/ralph-proto/src/event_bus.rs`、`crates/ralph-cli/src/preflight.rs`、`crates/ralph-core/src/config/loop_config.rs`
