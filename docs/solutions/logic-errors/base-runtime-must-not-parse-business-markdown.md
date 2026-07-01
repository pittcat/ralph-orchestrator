---
title: "Base runtime 不应解析业务 markdown — 回滚 U6 PlanTopologyCache"
date: 2026-07-01
module: ralph-core
problem_type: logic_error
component: development_workflow
severity: high
symptoms:
  - "primary-20260701-063033 跑 python-sort-algorithms plan 时只走完 step-01 就进 report.done，review/fix/ship 整段被跳过"
  - "ledger.jsonl 第一行就报 plan_topology_unparseable: '... did not match the `### U{N}.` convention'"
  - "5 个 commit 链（02101d32 → 2db8b48f → 7480b164 → 8593405c → a0f8b398）全部在补丁一个错的设计"
  - "U6 落地后任何用 ### UNIT 1: / ## Step 1 / ### 1. 项目骨架 等历史写法的 plan 全部静默失效"
root_cause: logic_error
resolution_type: code_fix
related_components:
  - "ralph-core/event_loop/orchestrator_state.rs（整文件删除）"
  - "ralph-core/event_loop/review_step_state.rs（删 U6 的 3 个共享 scan 函数；prefill_fix_steps_from_plan 属 2026-06-28-002 U1，还原为 pre-U6 内联版并补回归测试）"
  - "ralph-core/event_loop/mod.rs（删 prepend_orchestrator_state + try_install_plan/fix_topology + 装载循环）"
  - "presets/en/ce-executor-serial.yml（删 U6 HARD RULE 段）"
  - "LoopState.plan_topology 字段（loop_state.rs + summary_writer.rs 同步删）"
tags:
  - plan-parsing
  - orchestrator-state
  - expected-event
  - ce-executor-serial
  - multi-hat-isolation
  - u6-rollback
  - base-runtime-boundary
---

# Base runtime 不应解析业务 markdown — 回滚 U6 PlanTopologyCache

## Problem

`primary-20260701-063033` 用 ce-executor-serial 跑 `2026-06-20-001-feat-python-sort-algorithms-plan.md`，整条链路只走完 step-01 就进了 `report.done`，review/fix/ship 整段被静默跳过。5 个 step 1 之后到底发生了什么：

```
work.start → work.ready(step-01) → work.done ×2 → report.done
```

正确的 ce-executor-serial 流程应该是 plan-units → review walk → fix-units → plan.complete → shipper → reporter。整段 review/fix/ship 消失，loop 在第一个 step 就 `awaiting_decision`。

## Symptoms

- `ledger.jsonl` 第一行：`rejection_recorded` 携带 `plan_topology_unparseable`，提示 `### U{N}.` 严格格式不匹配
- `task-1782887490-8663`（step-01）发了两次 `work.done`（6:33:34 + 6:33:38）
- `tasks.jsonl` 只有一个 step-01 task，step-02 永远没创建
- `progress.md` 只有 `step-01` 完成标记
- 整个链路在 ledger 第一行就 fail-closed，下游全是兜底放行

## Root Cause

错的设计假设 + 错的设计层：

**错的设计假设**：U6 假定所有 plan 都用 `### U{N}.`（U 后直接接数字 + 点号）这种严格格式扫描。但**plan 是给人写的契约**，历史上多种合法写法（`### UNIT 1:` / `## Step 1` / `### 1. 项目骨架` / 中文小标题）都是 LLM 一眼能看懂的，扫不到就 fail-closed。

**错的设计层**：把"理解 plan"这件事硬塞进 base runtime 的 Rust 字符串匹配里（`review_step_state.rs::scan_unit_headings_with_prefix`），违反单一职责——base 该做的是校验结构化数据、推进状态机，**业务语义理解留给 LLM**。

**连锁失败**：
- `PlanTopologyCache::scan` 拿到空 Vec
- `install_plan_topology` 写 rejection + 返回 `Err`
- `plan_unit_ids` 为空 → `compute_expected_event` 拿不到下一步
- 兜底放行 → executor 的 `work.done` 跳过 review/fix 整段
- 5 个 commit 全在补丁这个错的设计

## Solution

**P0 整段回滚 U6 单元**，而不是继续打补丁：

1. **删除 `crates/ralph-core/src/event_loop/orchestrator_state.rs`**（756 行）—— 整个 `PlanTopologyCache` + `compute_expected_event` + `ComputeInput/Output` + 4 个测试
2. **删除 `review_step_state.rs` 里 U6 新增的 3 个共享 scan 函数**（`scan_unit_headings` / `scan_unit_headings_as_steps` / `scan_unit_headings_with_prefix` + 其 U6 单元测试）。**注意**：`prefill_fix_steps_from_plan` 及其 `review.complete(fix_plan_file)` 调用点**不属于 U6**——它们来自计划 2026-06-28-002 U1（commit `40765b6f`，早于 U6），U6 只是把其内部解析器改成共享的 `scan_unit_headings`。因此这两处**不删除**，而是**还原成 U6 之前的自包含内联版本**，保留按 fix-plan 为每个 `fix-{NN}` 预填 `synth_terminal` 的能力。
3. **删除 `mod.rs` 的接线**（约 182 行）—— `prepend_orchestrator_state` + `try_install_plan/fix_topology` + 装载 for-loop + `extract_plan_path`/`extract_fix_plan_file`
4. **删除 `LoopState.plan_topology` 字段**（loop_state.rs + summary_writer.rs 同步）
5. **删除 `presets/en/ce-executor-serial.yml` 里的 U6 HARD RULE 段**（12 行）

代码侧净删除约 **976 行**（新增 83 / 删除 1059，已计入 prefill 还原与下述回归测试）。

**保留 / 还原**：
- `LoopState.last_test_passed_step` 字段（prompt 诊断用）—— 不依赖 markdown 解析，保留。
- `prefill_fix_steps_from_plan` + `review.complete(fix_plan_file)` 调用点（2026-06-28-002 U1）—— **还原**为 pre-U6 内联解析器；并**新增回归测试** `prefill_fix_steps_from_plan_seeds_all_fix_units_on_review_complete` 把该路径钉死。此前该函数从无专用测试，正是 U6 回滚一度误删它时无人察觉的根因。

### 验证

```bash
cargo check --workspace --exclude ralph-e2e       # OK
cargo nextest run -p ralph-cli --bin ralph -- preset_lint   # 11/11 pass
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded   # 1/1 pass
cargo nextest run -p ralph-core -- review_step_state   # 22/22 pass
cargo nextest run -p ralph-core --test scenarios   # 71/71 pass (BDD 真事件循环)
./scripts/run-tests.sh   # 全 workspace nextest + doctest 全过
```

## Why This Works

新架构把"理解 plan"放回 LLM 该在的地方：

| 职责 | 谁做 | 怎么校验 |
|---|---|---|
| 读 plan.md 散文、识别 UNIT 边界 | **coordinator hat (LLM)** | prompt 引导，coordinator 自己决定下一步发什么 |
| 校验事件 schema（required_fields、payload 形状） | **base runtime** | `validate_event` + `PayloadContractViolation` |
| 推进状态机（plan_gate / wave / completion guard） | **base runtime** | `event_loop/mod.rs` 的 30+ stages |

修复后 coordinator 不再依赖 `## ORCHESTRATOR STATE` prompt 块（也删了），coordinator 自己读 `progress.md` + 自身对 plan.md 的理解来发事件。这正是十诫之一「**Let Ralph Ralph — 坐*在*循环上，不是*进*循环里**」的实践。

## Prevention

### 硬规则（建议加到 CLAUDE.md）

**base runtime 不解析业务 markdown。所有业务语义理解走 LLM prompt。** 检测到 base 在新增涉及 markdown/plan 文档字符串匹配的代码时必须先问"这个事 LLM 做是不是更合适"。

### 配套 review checklist

新增涉及"扫描用户文档"的代码时：

1. 该文档是不是人写的契约？如果是 → 解析权归 LLM
2. 解析失败时链路会怎样？当前是 fail-closed 还是兜底放行？**fail-closed 路径必须有显眼告警**（这次 ledger 写入的 rejection 没人去读）
3. 解析逻辑在 base 还是 in agent？base 只校验结构化数据
4. 是否破坏了"任何历史 plan 都能跑"的不变量？如果需要历史 plan 改写才能跑 → 设计有问题

### 触发历史回滚的检测器

- 在 `release-check` 或 pre-merge hook 里加一条：扫描 `crates/ralph-core/src/event_loop/` 下的 `std::fs::read_to_string` 调用，凡是直接读 markdown 路径的都打 warning
- review 任何 plan/fix-plan 解析相关 PR 时，**首先验证它能处理至少 3 种不同格式的 plan 文档**（`### U{N}.` / `### UNIT {N}:` / `## Step {N}`）

## Cross-References

- `docs/solutions/architecture-patterns/orchestrator-expected-event-ledger-ssot.md` — 描述 U6 设计的文档，**设计本身已被本次回滚**。新文档解释了为什么这个设计错了。
- `docs/plans/2026-07-01-001-fix-ce-executor-serial-p0-terminal-storm-plan.md` — 原始 6-U 实施计划，**U6 单元不应当被实施**
- `docs/solutions/2026-06-16-isolated-wave-stability-and-progress-steward.md` — 同期的 multi-hat isolation 工作，本次回归未触及
- memory: 无直接相关 auto-memory 条目（根因是新的设计边界问题，不在历史 lessons 中）
