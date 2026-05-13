# Code Review Report

**Run ID:** `20260513-005547-6f19ec19`
**Branch:** `pittcat-dev` (compare against `main`)
**Intent:** 为 Ralph 编排引擎添加可选的工作流状态守卫机制，使 AutoResearch 等配置可以声明所需的事件主题序列，在乱序事件到达下游帽子之前将其拒绝，并发布恢复信号。
**Mode:** report-only
**Reviewers:** correctness (always), testing (always), maintainability (always), project-standards (always), ce-agent-native-reviewer (always), ce-learnings-researcher (always), adversarial (always), reliability (always)

---

## Findings

### P0 -- Critical

| # | File | Issue | Reviewer(s) | Confidence | Route |
|---|------|-------|------------|-------------|-------|
| 1 | `crates/ralph-core/tests/scenarios.rs:114` | **autoresearch_guard 场景测试是空壳** — `ScenarioRunner::run()` 永远不调用 `EventLoop::run()` 或 `process_events_from_jsonl()`。它只循环 MockBackend 然后返回空的 `ExecutionTrace`。测试只验证迭代次数，完全没有触及工作流守卫代码。autoresearch_guard.yml 里包含"experiment.evaluated attempted here but should be rejected"的注释，但从未被解析、消费或断言过。 | testing (100) | 100 | `manual` → `downstream-resolver` |

### P1 -- High

| # | File | Issue | Reviewer(s) | Confidence | Route |
|---|------|-------|------------|-------------|-------|
| 2 | `crates/ralph-core/src/event_loop/mod.rs:1859-1870` | **Advisory/Strict 混合链事件导致 progress 错误推进** — 当一个事件同时属于多个链（一个 advisory、一个 strict）时，`advance()` 对所有匹配的链都执行，不管其中一个是否已拒绝。这意味着如果链 A（advisory）接受了但链 B（strict）拒绝了，链 B 的 progress 仍会被尝试推进（虽然是 no-op），且如果链 A 先处理，progress 会为链 B 也推进一个 phase。正确的行为应该是：只有 is_valid==true 的链才推进 progress。 | adversarial (P0), correctness (P1) — **kept P0** | 75 | `manual` → `downstream-resolver` |
| 3 | `crates/ralph-core/src/event_loop/mod.rs:1843-1849` | **task.resume 恢复信号缺乏可操作上下文** — 拒绝乱序事件时发布的 `task.resume` 只说"Wait for the previous phase to complete"。没有告诉 agent：(1) 当前合法 phase 是什么，(2) 期望的主题序列，(3) 下一个应该发什么事件。Agent 无法自行纠正，必须重新读取配置。 | ce-agent-native (critical), reliability (low) | high | `manual` → `downstream-resolver` |
| 4 | `crates/ralph-core/src/event_loop/mod.rs:2402-2455` | **check_workflow_guard_completion 不区分 chain mode，Advisory 链也会阻止 LOOP_COMPLETE** — `check_workflow_guard_completion` 对所有链迭代，不管 mode 是 Strict 还是 Advisory。如果 Advisory 链的所有主题都发出（包括乱序重发），实例达到 terminal_phase 并阻止完成。但 Advisory 的设计目的是"不被阻止"。应跳过 Advisory 链或仅检查 Strict 链的终端状态。 | correctness (P1, 75) | 75 | `manual` → `downstream-resolver` |
| 5 | `crates/ralph-core/src/event_loop/mod.rs:1882-1905` | **extract_correlation_key 对两种情况都返回 None：'无 correlation 配置'和'提取失败'** — `chain.correlation.as_ref()?` 返回 None（无配置），`serde_json::from_str` 失败返回 None，路径不存在返回 None，`as_str()` 返回 None。无法区分"故意用全局实例"和"提取失败静默降级到全局实例"。一个有 correlation 配置的链在 payload 解析失败时会静默用全局实例跟踪，导致跨 experiment 污染。 | adversarial (P1, 50), reliability (medium), correctness (P2) | 50 | `advisory` → `human` |
| 6 | `crates/ralph-core/src/event_loop/mod.rs:2599` | **guards_config.clone() 在热路径中造成 O(chains * topics) 堆分配** — `process_parse_result` 每次迭代克隆整个 `guards_config`（所有 chains、topics、correlation configs），只是为了避免与 `self.apply_workflow_guard_validation` 的借用冲突。配置包含 `Vec<WorkflowChain>` 每个含 `Vec<String>`，每次 JSONL 读取周期都调用。 | adversarial (P2, 100), maintainability (borrow conflict) | 100 | `gated_auto` → `downstream-resolver` |
| 7 | `crates/ralph-core/src/event_loop/mod.rs:1772-1876` | **多链早期 break 语义不清晰** — 当一个事件属于多个链时，代码在第一个拒绝的链处 break（line 1831）。但如果事件同时属于 N 个链，应该在所有 N 个链都检查完后再决定是否拒绝。当前行为可能向订阅了有效链的 hat 抑制发布。语义是"任何链拒绝就拒绝"还是"所有链都拒绝才拒绝"未文档化。 | maintainability (P1, 75) | 75 | `advisory` → `human` |
| 8 | `crates/ralph-core/src/event_loop/mod.rs:691` | **LOOP_COMPLETE 被拒绝后 loop 终止，recovery 事件可能未被消费** — 当 `check_workflow_guard_completion` 返回 `Some(rejection)` 时，`task.resume` 发布后 `return None` 终止循环。如果这是最后一次迭代，bus 上的 `task.resume` 可能未被处理就终止了，Agent 永远看不到拒绝原因。 | adversarial (P1, 50) | 50 | `advisory` → `downstream-resolver` |
| 9 | `crates/ralph-core/src/event_loop/mod.rs:1849` | **task.resume 被多个不同场景共用，Agent 无法区分** — `task.resume` 用于：required_events 缺失、workflow_guard 不完整、persistent mode idle、fallback stall recovery、workflow_guard 拒绝。每种 payload 语义不同，Agent 无法编程式地区分，只能解析文本。 | ce-agent-native (warning) | high | `advisory` → `human` |
| 10 | `crates/ralph-core/src/event_loop/tests.rs:4782` | **没有测试验证 task.resume 恢复信号在拒绝时被发布** — `apply_workflow_guard_validation()` 在拒绝乱序事件时发布 `task.resume`，但所有 6 个工作流守卫测试只检查 `workflow_progress` 状态，从未验证总线上的发布事件。负路径（拒绝产生恢复信号）完全未验证。 | testing (P1, 75) | 75 | `manual` → `downstream-resolver` |
| 11 | `crates/ralph-core/src/event_loop/mod.rs:2416-2420` | **check_workflow_guard_completion 跳过 started-but-no-progress 实例** — 当 `get_phase` 返回 None 时实例被跳过（视为"未启动"）。但如果一个实例收到了被拒绝的 phase-1 事件（从未记录 progress），在 LOOP_COMPLETE 时它被静默跳过，看似完成实际未完成。 | adversarial (P1, 50) | 50 | `advisory` → `human` |

### P2 -- Moderate

| # | File | Issue | Reviewer(s) | Confidence | Route |
|---|------|-------|------------|-------------|-------|
| 12 | `crates/ralph-core/src/event_loop/mod.rs:1849` | **重复拒绝同事件会累积 task.resume** — 同一 out-of-order 事件每次迭代都被拒绝并发布新的 `task.resume`。没有去重机制，一个迭代中 N 个被拒事件产生 N 个 recovery 信号，可能干扰后续处理。 | adversarial (P2, 75), reliability (low) | 75 | `advisory` → `downstream-resolver` |
| 13 | `crates/ralph-core/src/event_loop/loop_state.rs:125` | **WorkflowProgress HashMap 键类型 Option<String> 导致语义混淆** — `None` 既表示"全局实例（无 correlation 配置）"又表示"提取失败降级到全局"。相同外层 HashMap bucket 被不同语义共享。如果链有 correlation 配置但提取失败，实例被跟踪到 None（全局），与故意用全局的链冲突。 | adversarial (P2, 50), correctness (P2, 50), reliability | 50 | `advisory` → `human` |
| 14 | `crates/ralph-core/src/event_loop/loop_state.rs:168` | **is_phase_valid 允许重发旧 phase 但 advance 是 no-op** — `is_phase_valid` 对 phase <= highest+1 返回 true，但 `advance` 对 phase <= current_highest 静默 no-op。如果实例已在 phase 5（terminal），重发 phase 1 通过 is_phase_valid（1 <= 6）被接受但不推进 progress。事件仍被发布到总线。 | adversarial (P2, 50), maintainability (low, 50) | 50 | `advisory` → `human` |
| 15 | `crates/ralph-core/src/event_loop/mod.rs:1793` | **重复迭代：matching_chains 先用 contains() 过滤再用 position() 重新扫描** — 对每个事件，先收集所有含该 topic 的链（contains），然后立即在内部链循环中用 `position()` 重新扫描 topic 列表。白白低效更重要的是混淆逻辑。 | maintainability (P2, 50) | 50 | `advisory` → `downstream-resolver` |
| 16 | `crates/ralph-core/src/event_loop/mod.rs:2417-2420` | **check_workflow_guard_completion 只报告第一个不完整实例** — 如果多个实例在多个链中不完整，只返回一个。Agent 需要多次迭代才能发现全部不完整实例。 | reliability (info, low) | low | `advisory` → `human` |

---

## Requirements Completeness

Plan: `docs/plans/2026-05-12-002-fix-autoresearch-workflow-state-guard-plan.md` (source: `explicit`)

| Requirement | Status | Notes |
|-------------|--------|-------|
| **R1**: Ralph 必须能够在乱序工作流事件发布到总线之前将其拒绝 | **met** | `apply_workflow_guard_validation` 在总线发布前验证 |
| **R2**: AutoResearch 必须能够强制 `experiment.evaluated` 不能在匹配的 `experiment.scored` 之前发生 | **met** | 按 chain 验证，Strict 模式拒绝乱序 |
| **R3**: 守卫必须按每个实验实例工作，而不仅仅是按全局主题 | **met** | `WorkflowProgress` 按 `chain_name + instance_key` 跟踪 |
| **R4**: 没有新守卫的现有配置必须保持当前行为 | **met** | `workflow_guards` 是 `Option`，默认 None 禁用 |
| **R5**: 被拒绝的事件必须产生可操作的反馈 | **partial** | task.resume 已发布但上下文不足（见 #3） |
| **R6**: 完成检查必须检测不完整的工作流实例 | **met** | `check_workflow_guard_completion` 验证所有实例 |
| **R7**: 内置的 AutoResearch 预设应在引擎支持后选择加入守卫 | **met** | `presets/autoresearch.yml` 已配置 `workflow_guards` |

| Implementation Unit | Status | Notes |
|---------------------|--------|-------|
| **U1**: 添加工作流守卫配置 | **met** | `WorkflowGuardsConfig`, `WorkflowChain`, `WorkflowChainMode`, `CorrelationConfig` |
| **U2**: 在循环状态中跟踪工作流实例进度 | **met** | `WorkflowProgress`, `WorkflowInstanceProgress` |
| **U3**: 在总线发布前拒绝乱序事件 | **met** | `apply_workflow_guard_validation` + task.resume |
| **U4**: 加强守卫链的完成验证 | **met** | `check_workflow_guard_completion` |
| **U5**: 更新 AutoResearch 预设以选择加入工作流守卫 | **met** | presets 已配置 |
| **U6**: 记录工作流守卫和恢复行为 | **partial** | `docs/reference/hatless-workflow-guards.md` 已创建，但 `docs/guide/configuration.md` 等未更新 |
| **U7**: 添加原始故障的重放或场景覆盖 | **not addressed** | `autoresearch_guard.yml` 是 stub，`ScenarioRunner::run()` 未调用 EventLoop（见 #1） |

---

## Pre-existing Issues

（无）

---

## Learnings & Past Solutions

| Source | Relevance |
|--------|-----------|
| `docs/plans/2026-05-12-002-fix-autoresearch-workflow-state-guard-plan.md` | 该计划正是本 PR 要实现的功能，详细记录了问题根因（Hatless Ralph 多帽模式导致 periodic.review 触发时 Reviewer 和 Evaluator 同时活跃）和 7 个实现单元拆解 |
| `docs/reference/hatless-workflow-guards.md` | 解释了 Hatless Ralph 设计意图和局限性：多帽模式下 `next_hat()` 仍返回 `ralph`，`build_prompt()` 会注入多个活跃帽子的 instructions，仅靠 prompt 规范无法保证严格阶段顺序 |
| `crates/ralph-core/src/event_loop/loop_state.rs` | `WorkflowProgress` 的 `is_phase_valid()` 确保只有相邻阶段才能推进（phase <= highest + 1），现有实现已支持单实例顺序、多实例独立、幂等重放场景 |

---

## Agent-Native Gaps

- `workflow_guards` 配置可通过 ralph.yml / presets 访问（配置项），但未在 `ralph-tools.md` 中文档化，Agent 可能无法主动发现此功能
- `task.resume` 恢复信号存在但可操作性不足（见 #3）
- 缺乏 Agent 可查询工作流进度状态的机制

---

## Coverage

- **Suppressed**: 0 findings below anchor 75 (all P0/P1 findings were 75+)
- **Mode-aware demotion suppressions**: 0 (report-only mode)
- **Validator drops**: stage 5b not run in report-only mode
- **Failed/timed-out reviewers**: 0 of 8
- **Untracked files excluded**: `docs/plans/2026-05-12-002-fix-autoresearch-workflow-state-guard-plan.md` (plan doc, pre-existing), `docs/reference/hatless-workflow-guards.md` (new, untracked), `task.md` (pre-existing)

---

## Verdict

**Not ready.**

代码核心机制（配置、循环状态、验证、完成检查）已实现且符合计划意图，但存在一个 P0 测试空壳问题和多个需要修复的 P1 逻辑/测试问题：

1. **P0**: `autoresearch_guard.yml` 场景测试是 stub，从未调用 EventLoop — 原始故障形态的回归测试不存在
2. **P1**: Advisory/Strict 混合链的 progress 推进 bug — 会导致状态分歧
3. **P1**: Advisory 链会阻止 LOOP_COMPLETE — 与设计意图违背
4. **P1**: task.resume 恢复信号缺乏可操作上下文
5. **P1**: 相关集成测试缺口（recovery 信号验证、幂等性验证等）

**Fix order:**
1. 先修复 #1（测试空壳）和 #4（Advisory completion check bug）— 这两个是可验证的逻辑问题
2. 再修复 #3（recovery payload）和 #6（borrow clone）— 设计权衡
3. 最后补充缺失的集成测试（#10 相关）

---

*Generated by ce-code-review skill · 2026-05-13*
