---
title: U11 — 统一编排状态重构 Production 接入与全量修复
type: refactor
status: active
date: 2026-06-22
branch: pittcat-dev
origin:
  - docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md
  - docs/plans/2026-06-22-003-unified-orchestrator-state-plan-review-report.md
  - docs/plans/2026-06-21-002-adversarial-review.md
review_ref: docs/plans/2026-06-22-003-unified-orchestrator-state-plan-review-report.md
---

# U11 — 统一编排状态重构 Production 接入与全量修复

> **目标**:把 `2026-06-21-002` 计划(U0–U10)的所有模块从「已实现但不可用」状态推至「production 可用 + flag 默认全开 + 全量测试绿」。
>
> **范围**:全量修复 review 报告列出的 7 项 P0 + 7 项 P1(用户决策)+ flag 默认值反转 + 文档同步(用户决策)。
>
> **不在范围**:P2 优化项(报告内 P2-1~P2-5)——已记录为 follow-up,本次不处理。
>
> **风险等级**:HIGH。production event loop 接入会改动热路径,需要保证默认状态零回归 + flag-on 全绿。

---

## 1. 现状与根因

### 1.1 Review 报告结论

报告 `2026-06-22-003` 给出 **BLOCKED** 结论:虽然 U1–U7 新模块代码实现完成,但**核心架构目标在 production event loop 中未接入**。`event_loop/mod.rs` 的 `process_parse_result` 仍使用 legacy gate 栈,新代码是 dead code。

### 1.2 已经做的 hook(部分修复)

审查时已经做了若干 A1–A4 hook,但**不完整**:

| Hook | 位置 | 状态 |
|---|---|---|
| A1 end-of-batch ledger commit | `event_loop/mod.rs:10204-10244` | ⚠️ 只 commit scalars(iteration + completion/cancellation flags),未 commit per-event delta |
| A2 unified validation pipeline build | `event_loop/mod.rs:7512-7533` | ⚠️ pipeline 已构造但**未在 per-event gate 中调用** |
| A3 emit_correction_context hook | `event_loop/mod.rs:529-579` | ⚠️ 走 `publish_correction_via_context` 但用 throwaway PromptContext,**未 merge 到 `state.prompt_context`** |
| A4 commit_handoff_artifact | `event_loop/mod.rs:8588-8602` | ⚠️ 在 handoff gate 通过路径上调用,但**未在 macro-edge 缺失 path 的 auto-generate 路径**调用 |

### 1.3 未修的根因

1. **`StateLedger::new` 不调用 `replay_from_disk`** → ledger crash 后丢失状态(adversarial review P1-1)
2. **`process_parse_result` 未走 ValidationPipeline** → CLI/runtime 校验分叉
3. **`publish_correction_via_context` 用 throwaway context** → correction 注入不到 prompt
4. **`commit_handoff_artifact` 仅在 path 已存在时调用** → U5 auto-generate 未生效
5. **U10 验证报告虚假声明**(line 180-181 声称 "U4 validation pipeline 已替换所有 runtime 路径")
6. **feature flag 默认关闭** → 即使代码就绪也不走新路径

---

## 2. 设计决策

### D1. Production 接入策略:渐进式覆盖 + flag 默认开启

**方案**:在 `process_parse_result` 的关键节点**新增**新路径调用(不删除 legacy),通过 `UNIFIED_STATE_LEDGER=1` 等 env var 切换。验证后**反转默认值**(`default-on`),旧路径代码标记 `#[deprecated]`,但保留 escape hatch 1 个 minor version。

**为什么不是 big-bang**:plan 已明确(strangler fig 模式),flag 切换保证可回滚。

**为什么不留 flag off**:用户决策"flag 默认全开"。报告 P0-7 已识别 16 条 flag-on 失败,这些 gap 必须先修复。

### D2. ValidationPipeline 接入粒度:per-event 而不是 per-batch

**方案**:在 `process_parse_result` 的 per-event gate stack 内,**每个 gate 调用点前**插入 `pipeline.validate_pre_commit_with_view(protocol_view, ledger_snapshot, event)` 调用,接受 → 走 legacy gate(向后兼容);拒绝 → 走 `publish_correction_via_context` 路径。

**为什么不替换 legacy gate**:legacy gate 是 `ApplyOutcome` + `gate_envelopes` 等结构化输出,新 pipeline 输出 `ValidationResult`。两者语义层不一致(legacy 是"通过/拒绝 + 落 envelopes",新的是"accepted/reason_code")。覆盖而不是替换可保留 BDD scenario 兼容性。

### D3. CorrectionContext 落地路径:StateLedger::commit + LoopState.prompt_context 双写

**方案**:在 `publish_correction_via_context` 中:
1. 替换 throwaway PromptContext 为 `state.prompt_context` 的 in-place 修改
2. 同时 `state_ledger.commit(CommitDelta::RejectionRecorded { key, ... })` 写入 ledger
3. `retry_count_for(key)` 读 ledger 的 `rejection_counts` 而非 throwaway 计数

**为什么是双写**:`state.prompt_context` 是 LoopState 内的 prompt injection queue(legacy),`ledger.rejection_counts` 是 durable record(plan KTD-1)。前者供 prompt 渲染,后者供 replay + diagnose。

### D4. Handoff auto-generate:走 StateLedger::commit_handoff_artifact

**方案**:在 `evaluate_event` 的 macro-edge accept 路径,如果 `handoff_path` 缺失或 `validate_artifact` 失败:
1. 调用 `ledger.commit_handoff_artifact(&HandoffAcceptedInputs { ... })`
2. 该函数内部 `resolve_handoff_path` 生成 canonical path,`write_skeleton` 写入文件
3. 把生成的 `handoff_path` 写回事件元数据,再 publish 到 EventBus

**为什么不是修复 resolve_handoff_path 的覆盖策略**:adversarial review P2-8 标记 `resolve_handoff_path` 强制覆盖是 follow-up;本次保留覆盖行为,只确保调用点生效。

### D5. U10 验证报告修复:声明改为"已实现,待接入"

**方案**:line 180-181 改为:
> "`UNIFIED_VALIDATION` / `UNIFIED_HANDOFF_AUTO` 源码未读取 env var(注释过时);`UNIFIED_STATE_LEDGER` / `UNIFIED_PROTOCOL_VIEW` / `UNIFIED_POLICY_CHECK` / `UNIFIED_DETERMINISTIC_CORRECTION` 实现已 commit,但默认关闭,runtime production 仍走 legacy。"

### D6. 测试 gap 修复:U6 unified pipeline 接入 events.jsonl 历史

**方案**:`policy_check.rs:740` 的 `run_policy_check_unified` 当前用 `LedgerSnapshot::cold_start()`,改为**先加载 `.ralph/events.jsonl`** 构建 `LedgerSnapshot`(读已有 terminal/business 状态),再调用 `pipeline.validate_with_preview`。

**为什么不改 LedgerSnapshot 内部加载**:职责分离——`LedgerSnapshot` 是数据,`policy_check` 是 CLI 入口。

### D7. Feature flag 默认值反转

修改 `build_state_ledger_from_env`、`build_unified_validation_pipeline`、`is_correction_enabled`、`policy_check_unified_enabled` 等 env var 读取函数,默认值从"env unset → false"翻转为"env unset → true"。env var 仍保留(用户可显式设 0 关闭,作为 1 minor version 的 escape hatch)。

**验证**:开启 flag + 默认状态两组都跑 `./scripts/run-tests.sh`,均需 0 失败。

---

## 3. 实施方案

按 review 报告的「修复计划」四阶段 + 用户决策,本文档固化为 7 个实施任务(每任务对应一个 code-task.md 文件):

### T1. StateLedger::new 接入 replay_from_disk(P0-1 部分 + adversarial P1-1)

- **范围**:`crates/ralph-core/src/state/ledger.rs:167` 的 `new()` 方法
- **动作**:在 `new()` 内部调用 `Self::replay_from_disk(workspace)` 并把结果填入 `self.snapshot`(失败时 fallback cold_start + log warn)
- **验收**:`StateLedger::new(workspace)` 后 `self.snapshot` 等价于 ledger.jsonl replay 结果
- **测试**:新增 `state/tests.rs::new_replays_from_disk` + `new_falls_back_when_ledger_missing`

### T2. process_parse_result 接入 ValidationPipeline per-event(P0-2)

- **范围**:`crates/ralph-core/src/event_loop/mod.rs:9046-9110`(per-event gate stack)
- **动作**:
  - 在每个 per-event gate 调用前,执行 `pipeline.validate_pre_commit_with_view(...)`
  - 拒绝 → 走 `publish_correction_via_context`(已存在 hook,需要打通)
  - 接受 → 继续走 legacy gate(向后兼容)
- **验收**:`UNIFIED_VALIDATION=1` 时,所有 BDD scenarios 仍 63/63 通过,新增 production 测试覆盖 unified path
- **测试**:`event_loop/tests/u11_unified_pipeline_integration.rs`(新建)

### T3. emit_correction_context 走真实 PromptContext + StateLedger commit(P0-3)

- **范围**:`crates/ralph-core/src/event_loop/mod.rs:473-527`(`publish_correction_via_context`)
- **动作**:
  - 替换 throwaway `PromptContext::default()` 为 `state.prompt_context` 的 in-place 修改
  - 添加 `state_ledger.commit(CommitDelta::RejectionRecorded { ... })` 调用
  - `retry_count_for` 从 ledger 读取
- **验收**:recoverable rejection 后下一个 prompt 包含 `## ORCHESTRATOR CORRECTION` 块,2 条 BDD `#[ignore]` 移除后通过
- **测试**:移除 `tests/scenarios.rs:1698` + `:1718` 的 `#[ignore]`

### T4. commit_handoff_artifact 走 macro-edge 缺失 path(P0-4)

- **范围**:`crates/ralph-core/src/event_loop/mod.rs:8588-8602`(handoff accept 路径)
- **动作**:
  - 扩展 A4 hook:不只接受 path 已存在的 handoff,**也接受 path 缺失**
  - 调用 `ledger.commit_handoff_artifact(&HandoffAcceptedInputs { ... })` 在缺失 path 时生成
  - 生成的 path 写回事件元数据
- **验收**:BDD `hat_handoff/macro_handoff_inject.yml` 通过 U11 wire-up 版
- **测试**:已有 `hat_handoff/tests.rs` + `test_u9_handoff_auto_generate_scenario`

### T5. snapshot.rs 4 个 no-op delta 实现(adversarial review B1-B4 已部分修复,需复核)

- **范围**:`crates/ralph-core/src/state/snapshot.rs:434-557`
- **现状**:B1-B4 注释显示 reviewer 已实现(走 audit log 路径),需复核实际是否产生期望效果
- **动作**:跑 `cargo nextest run -p ralph-core -- state::snapshot` 全过 + 新增 `apply_delta_rebuilds_audit_logs` 测试

### T6. U10 验证报告虚假声明修复(P0-6)

- **范围**:`docs/plans/2026-06-21-002-unified-state-u10-verification.md:180-181`
- **动作**:改写为"已实现但默认关闭,production 仍走 legacy"的准确描述

### T7. Feature flag 默认值反转 + U6 unified pipeline 修复(P0-7)

- **范围**:
  - `event_loop/mod.rs:10942`(`UNIFIED_STATE_LEDGER`)
  - `event_loop/mod.rs:458`(`UNIFIED_VALIDATION`)
  - `event_loop/mod.rs`(其他 flag)
  - `crates/ralph-cli/src/policy_check.rs:740`(`run_policy_check_unified` 用 `LedgerSnapshot::cold_start()`)
- **动作**:
  - 修改 4 个 env var 默认值:unset → `true`(显式 0 才关闭)
  - `run_policy_check_unified` 改为先读 `.ralph/events.jsonl` 构建 `LedgerSnapshot`
  - 测试隔离缺陷 2 条(`u3_feature_flag_default_off_explicit_on` + `pipeline_records_protocol_view_feature_flag`):改用 `serial_test` 或重构为不依赖 env 状态
- **验收**:`UNIFIED_STATE_LEDGER=1` + 其他 flag 全开 + 默认状态两组都跑 `./scripts/run-tests.sh` 全绿(0 失败)

### T8. 文档同步(P1-5/6/7)

- **范围**:
  - `docs/report/2026-06-21-top-3-architectural-instability-factors.md`(P1-5)
  - `docs/guide/runtime-diagnosis.md`(P1-6)
  - `crates/ralph-core/data/ralph-tools*.md`(P1-7)
- **动作**:
  - report 末尾加"修复状态"章节:已实现模块、production 接入状态、剩余 follow-up
  - runtime-diagnosis 补 `--from-ledger` 用法、ledger-based rejection log 路径
  - ralph-tools 反向验证:`correction`、`loop.resume`、`StateLedger` 概念补全

---

## 4. 验收标准(全量)

按 review 报告「修复计划与验收标准」+ 用户决策:

- [ ] T1: `StateLedger::new` 调用 `replay_from_disk`,fallback 逻辑存在
- [ ] T2: `event_loop/mod.rs` 的 per-event gate 出现 `pipeline.validate_pre_commit_with_view` 调用
- [ ] T3: `state.prompt_context` 在 correction 时被 in-place 写入,2 条 BDD `#[ignore]` 移除并通过
- [ ] T4: macro-edge 缺失 path 时 `commit_handoff_artifact` 触发
- [ ] T5: snapshot.rs 4 个 delta 变体实现具体逻辑(可能已 done)
- [ ] T6: U10 报告虚假声明修正
- [ ] T7: 4 个 feature flag 默认值反转,`./scripts/run-tests.sh` 两组状态全绿(0 失败)
- [ ] T8: 3 处文档同步完成
- [ ] `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底通过(默认 + flag-on)

---

## 5. 风险与缓解

| 风险 | 缓解 |
|---|---|
| per-event ValidationPipeline 影响热路径性能 | U3 benchmark 基线;若退化 >5% 加缓存 |
| throwaway PromptContext 替换可能破坏现有 BDD | 跑 63/63 BDD scenarios,任何失败立刻回滚 |
| events.jsonl 加载到 LedgerSnapshot 大文件 O(n) | CLI `--policy-check` 用 `--history-limit N` 限制 |
| flag 默认反转破坏依赖旧行为的脚本 | escape hatch 保留 1 minor version;changelog 标注 |
| 文档同步引发反向验证规则 violation | 跑 `sed -n 'NN,MMp' <file>` 复核行号 |

---

## 6. 不在范围(明确排除)

- P2-1 `event_loop/mod.rs` 10689 行拆分 → 留 follow-up
- P2-2 `StateLedger::commit` 全量 clone snapshot 优化 → 留 follow-up
- P2-3 `#[allow(dead_code)]` 清理 → 留 follow-up
- P2-4 两套 gate 栈长期并存 → 已决定本次反转 default + 1 minor version 后清理
- P2-5 PR 描述缺失 → 本次修复时补充

---

## 7. 引用

- Review 报告:`docs/plans/2026-06-22-003-unified-orchestrator-state-plan-review-report.md`
- 对抗性审查:`docs/plans/2026-06-21-002-adversarial-review.md`
- U10 验证报告(待修):`docs/plans/2026-06-21-002-unified-state-u10-verification.md`
- 原 plan:`docs/plans/2026-06-21-002-refactor-unified-orchestrator-state-plan.md`
- CLAUDE.md 硬规则:nextest 入口 + ralph-cli 串行