---
title: "feat: 统一证据约束的拒绝反馈与纠错回合"
type: feat
date: 2026-08-06
revised: 2026-08-06
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
target_repository: ralph-orchestrator
baseline_commit: a9dff24e3562cf42f1496108b369c651db6c35f7
supersedes: 2026-08-06-001（初版，对抗性审查后重写）
---

# 统一证据约束的拒绝反馈与纠错回合（重写版）

## 0. 计划状态

- **状态：** READY。所有进入实施的关键技术决策置信度均不低于 0.85；初版遗留的 3 个审查缺陷（F-A/F-B/F-C，见下）已在本版修正。
- **基线：** `a9dff24e3562cf42f1496108b369c651db6c35f7`（分支 `pittcat-dev`）。本文全部行号锚点在该基线逐行核对。**初版基线 90e399d8 已过期**：其后仅 `docs/`（三份文档）与 `presets/en/parallel-forge.yml`（0c174f5a）有变更，均不在本计划受影响范围内，范围结论不变。
- **重写原因（对抗性审查结论）：** 初版证据锚点 29/29 全部属实、决策方向正确，但存在三个必须修正的缺陷：
  1. **[F-A 致命·设计缺口] U1 target 过滤未定义队列生命周期。** 现状 `prepend_correction_and_resume`（@7495）渲染后**清空全部** correction_blocks（consume-on-use，@7517-7518）。一旦在渲染边界按 target 过滤，「target 不是当前 hat」的 correction 若被一并清空就会被**先构建 prompt 的无关 hat 永久吞掉**。初版没有规定 partition/drain 语义，把关键决策留给了 Executor（违反决策空间限制）。本版新增 D9 决策与 U1-S7 测试固定：只消费 target∈{None, 当前 hat} 的条目，其余保留在队列。
  2. **[F-B 致命·无效 Red] U4 的 Acceptance Red 不成立。** 初版声称「semantic finding 的 suggestion 分支未显式禁止，应真实失败」。实测：`finding_record`（@2881）对 SemanticGateViolation 产出时 `suggested_payload_shape`/`suggested_command` 即为 None；`enrich_validation_error`（@2292）的 `_ =>` 分支（@2371-2376）注释明确「不为 semantic gate 编造建议」；`enrich_validation_error_with_topic`（@2390）仅当 shape 存在才生成 command。**suggestion omission 今天已成立**——该测试只会直接通过，不是 Red。本版将其降为 characterization 守卫；U4 真正的 Red 是 semantic 输出缺少 observed facts / violated invariant / required proof 结构化字段。
  3. **[F-C 事实错误·悬空需求] U6 引用了不存在的 R10**（追踪矩阵只有 R1-R9）。本版补 R10 行。
  - 另修正：precheck normalization 的调用归属表述精确化（F-D）；synthetic rejection 的双表示策略显式化（F-E，legacy `failed_checks` 保持全索引语义不动，新 detail 层标记 unavailable）；H1 由「待验证假设」升级为「已验证事实」（三个渲染调用点均在 `build_prompt(hat_id)` 内，见 E3a）。
- **调查范围：** `CorrectionContext`/`PromptContext`（correction/mod.rs）、`Rejection`（event_loop/rejection.rs）、precheck 脱糖与拒绝分派（precheck_gate_runner.rs / precheck_gate_enforcement.rs / event_loop/mod.rs @13306-13500）、`SemanticGateViolation` 与 payload consistency（event_policy.rs / event_policy_payload_consistency.rs）、CLI policy-check（policy_check.rs）、真实 EventLoop BDD（tests/scenarios*）、agent-facing guide（data/ralph-tools-*.md）、preset author/review references、相关历史 plan/solutions。
- **已执行的验证：** 全部引用文件 29/29 存在性核查；全部关键符号逐行定位（E 表）；suggestion 现状行为经 @2292-2408 源码走查确认；git 历史 7cda2b03 / 3008a36c 核实。未运行测试或构建（计划阶段）。
- **尚未执行的验证：** 所有 Red/Green、编译、nextest、BDD、CLI policy-check、preset lint、guide drift、最终 workspace 基线留给执行阶段（第 9 节）。
- **阻塞项：** 无。

## Goal Capsule

- **Objective：** 将 precheck 与 payload consistency 的拒绝结果统一为「证据约束的纠错反馈」：责任 hat 看到可复核的问题、事实证据和必须重新证明的条件；禁止 gate 给出可直接复制的成功 payload 或替代字段值。
- **Product authority：** 本计划的 Product Contract 与已确认会话决策定义行为边界；现有 `event_policy`、`CorrectionContext`、`PromptContext`、execution contract 与测试契约定义实现入口；注入式 guide 只描述 agent 下一步可执行的通用动作。
- **Execution profile：** 严格 U1 → U2 → U3 → U4 → U5 → U6 串行；每个 Unit 完成 Acceptance Red、最小实现、单元测试、集成验证和回归后才进入下一 Unit。
- **Stop conditions：** 实际调用链与本计划不一致、需要新增未记录的 rejection 来源、无法区分机械 schema 错误与语义证据错误、或任一关键决策置信度跌破 0.85 时，停止当前 Unit，更新 Evidence/Decision 后重新规划。
- **Tail ownership：** 实现完成后由 `ce-work` 按第 10 节质量门禁执行；本计划不编写生产代码。

---

## 1. 功能目标

### 1.1 业务目标

让 agent 在业务事件被 precheck 或 payload consistency 拒绝后，回到真实工作和证据来源检查问题，而不是根据错误消息猜一个能通过 gate 的 payload。

### 1.2 用户或调用方

- **A1 In-loop agent：** 接收责任 hat 的纠错 prompt，检查 artifact、diff、测试和任务状态，修复真实问题后重新声明结果。
- **A2 Runtime：** 生成、持久化、筛选和消费结构化 rejection feedback，维护 retry key、次数和升级状态。
- **A3 Preset author/reviewer：** 为 precheck checklist 和 payload consistency rule 提供可验证的不变量，不把「正确 payload」写成规则提示。
- **A4 Operator/diagnosis consumer：** 从 recovery ledger 和 `plan.blocked` 看到最后一次拒绝的 rule、证据缺口、责任 hat 和升级原因。

### 1.3 当前行为（基线事实，逐行核对）

- `CorrectionContext`（correction/mod.rs:62）字段为 reason_code/stage/topic/source_hat/retry_key/retry_count/escalation_threshold/needs_escalation/last_message/expected_payload_template/allowed_topics/required_fields；**无 target 字段**。由 `from_rejection`（@114）从 `Rejection` 构造，经 `emit_correction_context`（@339）写入 `LoopState.prompt_context` 与 ledger。
- `PromptContext.correction_blocks: Vec<CorrectionContext>`（@499-504）在 `render_correction_block`（@547）整体渲染 `## ORCHESTRATOR CORRECTION`；`prepend_correction_and_resume`（event_loop/mod.rs:7495）渲染后清空整个队列（@7517-7518，consume-on-use）。三个渲染调用点（@5284/@5574/@5766）全部位于 `build_prompt(hat_id)`（@5187）内——**渲染边界始终持有当前 hat_id**。
- `Rejection`（rejection.rs:113）已有 `retry_key`（@131）与 `target_hat: Option<String>`（@143）；但 `CorrectionContext::from_rejection` 未搬运 target。
- precheck：`dispatch_precheck_rejection`（event_loop/mod.rs:13396）经 `runner::dispatch_rejection`（precheck_gate_runner.rs:137）算 budget 后，构造 `Rejection`（source=gate hat、target=on_fail.target）→ `emit_correction_context`，**同时**另发 legacy `task.resume`（`enrich_task_resume_payload_full`，rejection.rs:968）并 `redispatch_hat_obligation`。
- synthetic rejection：`RejectedPayload`（precheck_gate_enforcement.rs:62）中 `failed_checks: Vec<u32>`（1-based）、`reason`、`synthetic: bool`；`RejectedPayload::synthetic(total)` 把 failed_checks 填成**全部索引**并置 reason=`gate_silent_or_ambiguous`——即现状在数据层「声称每项都失败」，反馈层必须显式标记证据不可用。
- payload consistency：`SemanticGateViolation { gate, context, referenced_fields }`（event_policy.rs:81-85）；evaluator `evaluate`（event_policy_payload_consistency.rs:138）只读当前 payload，`collect_referenced_fields`（@98）按声明顺序静态收集。
- CLI：`finding_record`（policy_check.rs:2881）对 semantic finding 置 field=""、gate/referenced_fields=Some、**suggested_*=None**；`enrich_validation_error`（@2292）仅对 missing_required_field/invalid_field_value/payload_type_mismatch 填机械建议；semantic 走 `_ =>` 空分支（@2371-2376）；`enrich_validation_error_with_topic`（@2390）仅在 shape 存在时生成 suggested_command。**semantic 输出今天就没有 replacement 提示（F-B 依据）**，缺的是结构化观察事实。
- 现有 BDD：`correction_deterministic.yml`（assert_state.correction_block_present @97）、`correction_three_escalation.yml`、`payload_consistency/{reject,accept}_*_fix_done.yml`、`2026-07-02-precheck-gate-{pass,exhaust}.yml`；runner 分 `run_scenario`（@1936，仅断言 iterations 的 stub）与 `run_workflow_guard_scenario`（@1971，真 EventLoop）——本计划一律用后者。

### 1.4 目标行为

- precheck 与 payload consistency 的拒绝都生成统一的 evidence-bound feedback：责任 hat、拒绝类型、稳定 rule/gate、观察事实、受影响字段或 artifact、违反的不变量、必须重新证明的条件、原始 topic、retry 状态和升级状态。
- 语义拒绝不生成字段替代值、不生成可直接复制的成功 payload、不把 `message` 当作 agent 指令；agent 必须重新检查事实来源并从最新证据生成 payload。
- `PromptContext` 按 D9 的 partition 语义定向消费：构建 hat H 的 prompt 时只消费 target∈{None, H} 的 correction；target 为其它 hat 的条目保留在队列直到其 target 构建 prompt。
- precheck gate 的 LLM rejection 与 synthetic rejection 进入同一 feedback 规范；synthetic 明确标记 gate_silent_or_ambiguous + evidence unavailable，不得伪造具体检查结果；legacy `failed_checks` 字段形状不变（schema 契约）。
- consistency 的 policy-check 与真实 apply 继续使用同源 finding；CLI JSON 输出新增 observed facts / violated invariant / required proof 结构化字段，与 loop prompt 使用同一事实来源。
- 同一 `retry_key` 的拒绝仍有界；通过后清除该 key 的连续失败状态；耗尽时沿用现有 `on_exhausted`/升级路径并保留最后一份 evidence-bound feedback。

### 1.5 行为差异

| 场景 | 当前行为 | 目标行为 |
| --- | --- | --- |
| precheck 拒绝 | failed_checks 索引 + 自由文本 reason + correction | 反馈 checklist 缺口、观察证据与重新证明条件，不给成功 payload |
| consistency 拒绝 | gate/referenced_fields/message；字段实际观察值不结构化 | 增加字段观察值与违反不变量；只要求回查事实，不提供替代值 |
| correction 注入 | 无 target；整体渲染给任意先构建 prompt 的 hat | correction 携带 target；按 D9 partition 定向消费，非责任 hat 不处理 |
| semantic gate 输出 | 已无 replacement（现状正确），但无结构化证据字段 | 保留无 replacement + 新增 observed/invariant/required-proof |
| 重复拒绝 | retry count 与 escalation 存在，终态原因笼统 | 升级记录保留最后 feedback、rule、证据与责任方 |

### 1.6 本次范围 / 1.7 非目标

**范围：** 扩展 `CorrectionContext`/`PromptContext` 反馈模型与 D9 渲染规则；precheck rejection 结构化证据缺口并接入 correction；consistency `SemanticGateViolation` 追加观察事实/验证条件的数据传播；CLI `--policy-check` semantic/mechanical 分流输出；责任 hat 防作弊 prompt contract；真实 EventLoop BDD；同步 `ralph-tools-emit.md`/`ralph-tools-precheck.md` 与 preset author/review references；recovery/diagnosis 测试可追溯。

**非目标：** 不新增 `ralph correction` CLI 命令；不新增 hat；不把跨事件历史一致性纳入 payload consistency；不取消 legacy `task.resume` 兼容分派；不移除机械 schema 错误的 suggested_*（只禁止其出现在 semantic feedback）；不一次性改写所有 rejection 类型（只覆盖 precheck 与 payload consistency 两类）；不修改 builtin preset 业务规则、不新增 consistency rule；不改 `presets/manifest.yml`/`index.json`/zsh 补全。

### 1.8 输入、输出与状态变化

- **输入：** precheck `<X>.rejected` payload、payload consistency `PolicyFinding`、当前 event topic/payload、rule/gate metadata、责任 hat、retry key/count、schema/preset context。
- **输出：** 结构化 `CorrectionContext`、定向 prompt correction block、CLI `ValidationError` evidence guidance、recovery ledger record、耗尽时带最后反馈摘要的升级事件。
- **状态变化：** accepted 业务事件仍只在所有 gate 通过后进入 bus；rejected event 不推进成功状态；correction 按 D9 消费后从队列清除（仅被消费的条目）；同一 rejection key 计数成功后复位、耗尽后进入现有升级状态。
- **副作用：** correction 继续走现有 ledger-first/recovery-second 路径；不得因生成 feedback 而写入被拒绝的业务事件或修改业务 artifact。

### 1.9 错误语义

- 结构化拒绝证据缺失时，runtime 输出「evidence unavailable / feedback incomplete」并 fail-close，不得补造观察事实。
- malformed precheck rejection 只生成「rejected payload malformed」诊断，不得把缺失字段解释成通过条件。
- consistency 规则命中时 `gate` 与 `referenced_fields` 必须保留；实际观察值无法安全表达时保留字段名和 `unavailable`，不得从 message 猜值。
- semantic feedback 不带替代 payload；测试必须阻止 semantic 输出出现 `suggested_payload_shape`/`suggested_command`（characterization 守卫，今天已成立，防未来回归）。
- 非责任 hat 不因另一个 hat 的 correction 改变其当前业务动作。
- retry budget 耗尽仍走现有 `on_exhausted`/escalation；最终反馈包含最后稳定 `retry_key` 与 evidence summary。

### 1.10 兼容性、性能、安全与约束

- 未启用 precheck 或 payload consistency 的 preset 保持现有行为；默认关闭路径必须有回归覆盖。
- 现有 `CorrectionContext`/recovery JSONL 旧字段与历史记录仍可反序列化；新增字段用可选/默认表示。
- feedback 只保存有限数量的字段观察值、check finding 和 bounded diagnostic text；不得复制完整事件历史或完整 payload。
- agent-controlled 文本继续经 `safe_display`（correction/mod.rs:204 既有模式）与长度/控制字符约束；rule message 是诊断数据，不是 instruction channel。
- 单业务事件预算、origin guard、event policy、state projection、terminal monotonicity 不变。
- 所有 Rust 测试走 `cargo nextest run` 系列；BDD 一律 `run_workflow_guard_scenario`；注入 guide 不得泄露内部 ledger 路径、函数名、计划编号或 preset 专属案例。

### 1.11 已确认事实（含初版「待验证假设」的升级）

- **E-H1（原 H1，已验证）：** `build_prompt(hat_id)`（@5187）持有当前 hat_id，且全部三个 `prepend_correction_and_resume` 调用点（@5284/@5574/@5766）都在其内——渲染边界定向筛选无需改 prompt builder 外部 API。U1 仍以 characterization test 固化为回归守卫。
- `PromptContext` 是 agent-facing correction 的 canonical injection point；`task.resume` 是兼容/调度通道（E1/E6/E7）。
- `SemanticGateViolation` 是 consistency 的统一 policy finding 类型，CLI 与 runtime 共享 finding 结构（E8/E9）。
- precheck gate rejection 与 synthetic rejection 都在 `dispatch_precheck_rejection` 进入 correction；无需新事件 topic（E5/E10）。
- 机械 schema 反馈与语义 evidence 反馈信息策略不同；semantic 的 suggestion omission 现状已成立（E9a，F-B 修正）。

### 1.12 待验证假设

- **H2（保留）：** consistency `when` AST 可在不引入新表达式语言的情况下生成有限字段观察值。观察值来源限定为**当前 payload 按 referenced_fields 路径取值**（JSON pointer lookup），不做谓词内省。验证动作：U2 对 `all`/`any`/single predicate 建测试；某路径无法安全序列化时保留字段名 + `unavailable`。该假设不阻塞总体方案，失败时降级为「只给字段名 + unavailable」。

---

## 2. 代码库现状与证据

### 2.1 当前实现入口

```
Agent-facing correction 调用链：
  业务事件被 policy/gate 拒绝
    → Rejection (rejection.rs:113) / PolicyFinding
    → emit_correction_context (correction/mod.rs:339)
        [调用方1] event_loop/policy.rs:90-147（policy rejection，含 semantic gate）
        [调用方2] event_loop/mod.rs:13448-13456（dispatch_precheck_rejection）
    → LoopState.prompt_context.correction_blocks (Vec<CorrectionContext>)
    → prepend_correction_and_resume (event_loop/mod.rs:7495)  ← 渲染后清空全队列（D9 改造点）
    → build_prompt(hat_id) 调用点 @5284 / @5574 / @5766      ← 均持有当前 hat_id
    → 责任 hat 下一次 activation

Precheck 调用链：
  producer emit X.proposed → gate hat emit X.rejected
    → drive_precheck_gate_obligation (@13306) → dispatch_precheck_rejection (@13396)
        → runner::dispatch_rejection (precheck_gate_runner.rs:137)  // budget/exhausted
        → 预算内：CorrectionContext + task.resume(legacy 兼容) + redispatch
        → 耗尽：on_exhausted 终态（@13488-13500）

Payload consistency 调用链：
  event_policy::validate_event_with_options → evaluate(rule_when, payload)
    (event_policy_payload_consistency.rs:138, 只读当前 payload)
    → ViolationType::SemanticGateViolation { gate, context, referenced_fields }
    → CLI: finding_record (policy_check.rs:2881) → enrich_validation_error (@2292)
```

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
| --- | --- | --- | --- | --- |
| E1 | correction/mod.rs:62-106 | `CorrectionContext` 现有 12 字段，无 target；render_block @196 经 safe_display | 扩展现有 contract，不新建第二套反馈总线 | 高 |
| E2 | correction/mod.rs:499-560 | correction queue 整体渲染；条目无 target | 需 target 字段 + D9 partition | 高 |
| E3 | event_loop/mod.rs:7495-7540 | 渲染后 `pc.correction_blocks.clear()` 清空全队列（@7517-7518） | **F-A 根因**：过滤必须与选择性消费同时落地（D9） | 高 |
| E3a | event_loop/mod.rs:5284/5574/5766 均在 build_prompt(@5187) 内 | 渲染边界恒有当前 hat_id | H1 升级为事实；无需改 prompt builder API | 高 |
| E4 | rejection.rs:113-143 | Rejection 已有 source_hat/target_hat/retry_key/kind | target 直接搬运，不做自然语言猜测 | 高 |
| E5 | precheck_gate_runner.rs:137-190；event_loop/mod.rs:13396-13500 | precheck 拒绝 → correction + legacy task.resume 双发；exhausted 发 on_exhausted | 保留 retry/exhaustion 语义，只增强 detail | 高 |
| E6 | precheck_gate_enforcement.rs:62-99 | RejectedPayload{failed_checks: Vec<u32>(1-based), reason, synthetic}；synthetic 填全索引 | synthetic feedback 必须标 unavailable；legacy 字段形状不动（F-E） | 高 |
| E7 | event_loop/policy.rs:87-147 | policy rejection（含 semantic）已构造 Rejection 并 emit correction | consistency 与 precheck 可共享同一外壳 | 高 |
| E8 | event_policy.rs:81-85 | SemanticGateViolation{gate, context, referenced_fields} | 扩展同一 finding 传播观察事实，不新增 error type | 高 |
| E9 | policy_check.rs:2167-2212, 2881-2956 | ValidationError 已有 gate/referenced_fields/suggested_* 字段；finding_record 对 semantic 置 suggested_*=None | 新增 observed/invariant/proof 字段即可；无新类型 | 高 |
| E9a | policy_check.rs:2292-2408 | enrich 的 `_ =>` 分支显式不为 semantic 编造建议；wrapper 仅在 shape 存在时生成 command | **F-B 依据**：suggestion omission 今天已成立，只能作 characterization 守卫，不是 Red | 高 |
| E10 | event_policy_payload_consistency.rs:98/138 | evaluator 只读当前 payload；collect_referenced_fields 声明序去重 | 观察值只从当前 payload 派生；跨事件不纳入 | 高 |
| E11 | tests/scenarios/correction_deterministic.yml（assert_state.correction_block_present @97）、correction_three_escalation.yml | 真实 workflow correction 场景已有 | 扩展而非新建；一律 run_workflow_guard_scenario | 高 |
| E12 | tests/scenarios/payload_consistency/reject_inconsistent_fix_done.yml + accept_consistent_fix_done.yml | consistency reject 不产生 fix.done/LOOP_COMPLETE 成功，且产生 correction block | 增加「无 replacement、必须回查证据」结构化断言 | 高 |
| E13 | tests/scenarios/2026-07-02-precheck-gate-exhaust.yml | precheck 连续拒绝与 exhausted 终态场景已有 | 增加 evidence 与最后反馈保留断言 | 高 |
| E14 | data/ralph-tools-emit.md:88-104, 160-172 | 已要求读 gate/referenced_fields、不凭 message 猜字段 | 补充「semantic 只回查事实、禁止复制替代 payload」规则 | 高 |
| E15 | data/ralph-tools-precheck.md:52-78 | 已要求读 failed_checks/reason 并原 topic 重发 | 补「failed_checks 是线索不是模板；synthetic 不假设已验证」规则 | 高 |
| E16 | skills/ralph-preset-author/references/ 与 skills/ralph-preset-review/references/（各 6-7 文件，已列目录核实） | author/review references 存在且为同步对象 | 按仓库 HARD RULE 同步审计锚点 | 高 |
| E17 | docs/achieved/plan/2026-07-22-004-feat-payload-consistency-gates-plan.md | consistency 原范围：同 payload、默认关闭、SemanticGateViolation | 本计划只补反馈可信边界 | 高 |
| E18 | docs/achieved/plan/2026-07-02-004-…-precheck-prompt-gate-plan.md；docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md | precheck 脱糖/hard obligation/retry/原 hat 回流有既有模式 | 复用既有拓扑，不新增流程分支 | 高 |
| E19 | git 7cda2b03（target-hat resume payload）、3008a36c（>8KB trigger 截断信号） | target/provenance/截断标记是近期演进方向 | 不退回自由文本恢复 | 中高 |
| E20 | recovery_intent.rs:48（target_hat 字段） | diagnosis responder 已有 target 过滤先例 | D9 的模式先例 | 高 |
| E21 | chain_validation.rs:131（test_chain_validation_injects_correction_on_rejection）；hatless_ralph.rs:355（build_prompt） | correction 集成测试与多 hat prompt 入口存在 | U1/U3 测试落点确认 | 高 |

### 2.3 受影响范围

- **生产模块：** `crates/ralph-core/src/correction/mod.rs`、`crates/ralph-core/src/event_loop/mod.rs`（prepend/build_prompt 边界与 dispatch_precheck_rejection）、`crates/ralph-core/src/event_loop/precheck_gate_runner.rs`、`crates/ralph-core/src/event_loop/precheck_gate_enforcement.rs`、`crates/ralph-core/src/event_loop/rejection.rs`、`crates/ralph-core/src/event_policy.rs`、`crates/ralph-core/src/event_policy_payload_consistency.rs`、`crates/ralph-cli/src/policy_check.rs`。
- **测试模块：** correction/mod.rs 内联测试、rejection.rs 测试、precheck_gate_runner.rs 测试、event_loop/tests/chain_validation.rs、event_policy.rs 测试、policy_check.rs 测试、tests/scenarios.rs（断言辅助）。
- **BDD fixtures：** correction_deterministic.yml、correction_three_escalation.yml、payload_consistency/reject_inconsistent_fix_done.yml、2026-07-02-precheck-gate-exhaust.yml，必要时新增同目录 evidence-bound fixture。
- **Agent-facing guide：** data/ralph-tools-emit.md、data/ralph-tools-precheck.md。
- **Operator/preset review guide：** skills/ralph-preset-author/references/{commands,finding-rubric,patterns,prompt-visibility}.md 与 skills/ralph-preset-review/references/ 对应文件（含 agent-skill-audit.md）；必要时 skills/ralph-preset-review/tests/test_skill_anchors.py。
- **诊断边界：** diagnosis/envelope.rs、diagnosis/reporter.rs、recovery_intent.rs 仅在最后反馈无法沿现有 rejection record 保留时触达；U1 characterization 先确认，禁止预先扩大范围。
- **不受影响：** 无新 preset/CLI 子命令/数据库表/外部服务/UI/依赖；不动 presets/manifest.yml、index.json、zsh 补全。

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
| --- | --- | --- | --- | --- | --- | --- |
| D1 | 统一反馈放哪？ | 新建 correction bus；只改 task.resume payload；扩展 CorrectionContext/PromptContext | 扩展现有 CorrectionContext/PromptContext，保留 task.resume 兼容投递 | E1-E7、E11、E19 | 新 bus 重复事实源；只改 task.resume 覆盖不了 deterministic correction | 0.97 |
| D2 | 给替代 payload 还是证据缺口？ | 字段替代值/shape；纯自然语言；结构化观察事实+不变量+验证义务 | semantic 只给结构化证据缺口与验证标准；机械 schema rejection 保留现有 shape/command | E8-E10、E14-E15 | 替代值鼓励「改声明骗 gate」；纯自然语言不可机读 | 0.98 |
| D3 | precheck 与 consistency 如何共享 detail？ | 两套自由文本；precheck 并入 consistency；共享外壳+gate-specific detail | 共享 CorrectionContext 外壳；precheck 传 checklist/artifact evidence，consistency 传 field observations/invariant | E5-E10、E17-E18 | 两套格式会漂移；强行统一内部字段制造虚假精确性 | 0.96 |
| D4 | 责任 hat 如何接收？ | 全 hat 广播；只靠 source_hat；显式 target + 按当前 hat 筛选 | CorrectionContext 携带 target，build_prompt(hat_id) 按 D9 消费；无 target 沿用既有可见性 | E2-E4、E20 | 广播导致无关 hat 误处理；precheck 中 source 是 gate hat 而非责任 hat | 0.93 |
| D5 | semantic 是否带 suggested_*？ | 全带；全删；按 mechanical/semantic 分流 | semantic 不带 replacement；mechanical 保持现状（今天已如此，加守卫） | E9、E9a、D2 | 全删损害 schema repair；全带违反证据优先 | 0.97 |
| D6 | consistency 观察值如何产生？ | 解析 message；CLI 二次猜测；evaluator 同源路径从当前 payload 按 referenced_fields 取值 | 在 evaluator 同源路径按 referenced_fields 对当前 payload 做有界取值，传播到 CLI/CorrectionContext | E8-E10 | message 不可信；CLI 二次猜测造成 apply/CLI 漂移；跨事件超范围 | 0.91 |
| D7 | precheck failed_checks 是否升级为复杂 LLM schema？ | 保持纯索引；gate LLM 生成完整修复方案；可选结构化 finding + 缺失 fail-close | 增加可选 finding detail（保留索引兼容）；缺失证据标 unavailable，不接受为成功证明 | E5、E6、E13 | 复杂 schema 把 gate 变成答案生成器；纯索引回答不了「问题是什么」 | 0.88 |
| D8 | 是否立即删除 legacy task.resume？ | 删除；双写以 task.resume 为主；PromptContext canonical + task.resume 兼容 | 保留兼容通道；两者从同一 normalized feedback 构造，禁止第二套事实模型 | E1、E5、E7 | 直接删除破坏旧 fixture/消费者；双套独立构造重新漂移 | 0.94 |
| **D9（新增，修正 F-A）** | target 过滤下的队列生命周期？ | (a) 渲染后仍清空全队列（现状语义）；(b) 分区消费：只消费 target∈{None, 当前 hat}，其余保留；(c) enqueue 时按 target 分桶建多队列 | **(b) 分区消费**。`prepend_correction_and_resume` 改为接收当前 hat_id，渲染后仅 `retain` target 不匹配条目；无 target 条目按既有可见性消费 | E3、E3a、E20、E4 | (a) 会让先构建 prompt 的无关 hat 永久吞掉定向 correction（F-A 事故路径）；(c) 改变 PromptContext 结构面与序列化形状，扩面且无额外收益 | 0.92 |

### 3.1 阈值附近决策的执行期验证

- **D7（0.88）：** U2 第一项必须清点现有 `RejectedPayload` 消费方与全部 precheck BDD fixture；若发现某 preset 的 gate instruction/schema 已要求互斥格式，停止 U2，重比「可选 detail」与「按 gate version 分型」，未回到 0.85 不得实现。
- **D6（0.91）：** 字段来源限定 evaluator 的 `when` AST + `referenced_fields`；执行期不得扩展为 JSONPath/跨事件/自由表达式；无法安全序列化的值按 `unavailable`。
- **D9（0.92）：** U1 必须先落 characterization（现状清空语义）再改 partition；U1-S7 是 F-A 的回归守卫，未绿不得进入 U2。

---

## 4. BDD 行为规格

```gherkin
Feature: Evidence-bound rejection feedback
  作为被拒绝业务事件的责任 hat
  我需要知道真实问题和重新证明条件
  以便修复事实而不是伪造一个能通过 gate 的 payload

  Background:
    Given correction injection 已启用
    And 业务事件仍通过现有 schema、origin、policy 和 execution contract 处理

  Scenario: S1 consistency rejection exposes the violated invariant
    Given payload_consistency 规则引用 status 和 fixes_applied
    And 当前 payload 同时声明 status=applied 与 fixes_applied=0
    When emitter 对业务 topic 执行 policy-check
    Then policy-check 拒绝该 payload
    And 输出包含稳定 gate、reason_code、referenced_fields、observed facts 和 violated invariant
    And 输出不包含 suggested_payload_shape 或 suggested_command
    And 业务事件未写入事件流

  Scenario: S2 consistency correction prompt requires evidence re-check
    Given S1 的拒绝进入责任 hat 的 correction context
    When runtime 构建责任 hat 的下一次 prompt
    Then prompt 明确要求检查 artifact、测试或其他事实来源
    And prompt 明确要求从最新证据重新生成 payload
    And prompt 明确禁止只修改字段、复制被拒 payload 或伪造成功
    And prompt 不给出可直接复制的替代业务值

  Scenario: S3 unrelated hat does not receive another hat's correction as its task
    Given correction target_hat=executor
    When runtime 为 reviewer 构建下一次 prompt
    Then reviewer 的 prompt 不包含 executor 专属 correction block
    And 该 correction 仍保留在队列中（未被 reviewer 的构建清空）
    When runtime 为 executor 构建下一次 prompt
    Then executor 的 prompt 包含该 correction block
    And 消费后该条目从队列移除

  Scenario: S4 precheck rejection exposes checklist gap without inventing evidence
    Given precheck gate emits X.rejected with failed_checks=[2] and a reason
    When runtime dispatches the rejection to on_fail.target
    Then correction contains guarded topic、failed check identity、reason、target and retry state
    And correction contains the condition that must be re-proven
    And correction does not claim an artifact or test result not present in the rejection

  Scenario: S5 synthetic precheck rejection is explicit about missing evidence
    Given precheck gate is silent or emits an ambiguous terminal combination
    When runtime synthesizes X.rejected
    Then correction identifies gate_silent_or_ambiguous
    And correction marks evidence as unavailable
    And correction does not pretend that every checklist item was factually disproven
    And legacy failed_checks 字段仍为全索引（schema 契约不变）

  Scenario: S6 mechanical schema rejection keeps shape guidance
    Given an event is missing required field task_id
    When policy-check rejects the event
    Then output still contains field、expected、suggested_payload_shape and suggested_command
    And the output is classified as mechanical rather than evidence-bound

  Scenario: S7 repaired event is regenerated from evidence and accepted
    Given a previous semantic rejection exists
    And the responsible hat changes the underlying artifact or verification result
    When it builds a new payload from the changed evidence and policy-checks it
    Then the semantic rejection no longer fires when the invariant is satisfied
    And the accepted event reaches its existing downstream consumer
    And the previous retry key is reset after the successful pass

  Scenario: S8 changing only the rejected field does not create false success
    Given the underlying artifact still contradicts a success claim
    When the hat only changes the payload field and republishes without new evidence
    Then the event remains rejected by the applicable evidence/execution gate
    And no downstream success event or terminal success state is produced

  Scenario: S9 repeated identical feedback escalates with the final evidence
    Given the same rejection key is produced until its retry budget is exhausted
    When the final rejection is processed
    Then the configured exhausted path is emitted once
    And the final recovery/diagnosis record contains the stable rule, target hat, last evidence summary and retry count
    And no further automatic success retry is scheduled

  Scenario: S10 disabled semantic feedback path preserves existing behavior
    Given precheck and payload_consistency are disabled or absent
    When an event is processed
    Then no new evidence-bound correction is generated
    And all existing unrelated policy/schema behavior remains unchanged
```

S3 的第二、三条是 **F-A 修正**的直接体现：无关 hat 构建 prompt 不得清空非自身 target 的 correction。

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
| --- | --- | --- | --- | --- | --- |
| S1 | semantic finding 结构化携带 gate/引用字段/观察事实/验证条件；无 replacement | event_policy 测试；policy_check.rs 测试；CLI integration | 单元 + CLI 集成 | nested field/null/长值的有界序列化 | 否 |
| S2 | correction prompt 含事实、违反条件、重查动作、禁作弊规则 | correction/mod.rs 测试；真实 scenario prompt 断言 | 单元 + BDD | safe_display、控制字符、长 message、prompt injection 回归 | 否 |
| S3 | target 定向消费；**无关 hat 构建不吞队列**；无 target 走既有 fallback | event_loop prompt 测试；chain_validation.rs；真实场景 | 集成 | 多 correction、稳定排序、同一 loop 多 hat | 否 |
| S4 | LLM precheck rejection normalized 为 evidence-bound feedback | precheck_gate_runner.rs 测试；precheck exhaust BDD | 单元 + BDD | malformed JSON、空 failed_checks、未知 check index | 否 |
| S5 | synthetic 只表达 silence/ambiguity + unavailable；legacy 字段不变 | precheck_gate_enforcement.rs 测试；真实 precheck BDD | 单元 + BDD | pass/reject 双发、无 checklist、silent gate | 否 |
| S6 | 机械 rejection 保留 expected/actual/suggested_* | policy_check.rs 既有测试；integration_emit_policy.rs | 单元 + CLI 集成 | missing/type/allowed-values 三类 | 否 |
| S7 | 真实修复后可通过；retry key reset | consistency accept/reject scenario；precheck pass scenario | BDD 集成 | reset 后再次拒绝从 1 计数 | 否 |
| S8 | 只改声明不产生假成功 | consistency negative BDD；execution contract/terminal 测试 | BDD 集成 | fix.done/work.done/report completion 现有路径 | 否 |
| S9 | exhausted 只发一次且保留最后反馈 | correction_three_escalation.yml、precheck exhaust fixture | BDD 集成 | replay/restart 后计数与 ledger 一致 | 否 |
| S10 | disabled/legacy path no-op | normalization/preset parity 测试、既有 correction fixtures | 回归集成 | builtin presets、默认关闭路径 | 否 |

所有测试必须断言副作用：被拒事件不进入 accepted events；下游 hat 不被错误触发；成功状态投影不提前；correction 按 D9 消费语义清除。BDD 一律 `run_workflow_guard_scenario`，禁止 run_scenario stub。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| R1 | 两类语义 rejection 共用 evidence-bound feedback 外壳 | S1、S4 | correction/policy BDD | CorrectionContext 构造与序列化 | precheck + consistency workflow | 否 | E1、E5、E8 |
| R2 | feedback 说明事实、缺口、不变量与验证条件 | S1、S2、S4、S5 | prompt/CLI 断言 | finding normalization/render | 真 EventLoop scenarios | 否 | E6-E10 |
| R3 | semantic rejection 不提供替代 payload 或成功值 | S1、S2、S8 | CLI JSON + prompt 断言 | semantic/mechanical 分支测试 + suggestion omission 守卫 | negative BDD | 否 | E9、E9a、D2 |
| R4 | correction 只回到责任 target hat，且不丢于无关 hat 构建 | S2、S3、S4 | target prompt scenario + D9 partition 测试 | filtering/ordering/retain 测试 | multi-hat workflow | 否 | E2-E4、E3 |
| R5 | precheck LLM/synthetic rejection 共享反馈语义但保留不同证据能力 | S4、S5 | precheck BDD | rejected payload normalization | pass/reject/exhaust workflow | 否 | E5、E6 |
| R6 | mechanical schema guidance 保持不变 | S6、S10 | 既有 CLI 测试 | ValidationError 分支测试 | emit policy integration | 否 | E9 |
| R7 | 通过后 retry key reset，耗尽后保留最后反馈 | S7、S9 | correction/precheck BDD | retry state 测试 | replay/exhaustion workflow | 否 | E5、E11、E13 |
| R8 | 默认关闭路径无行为变化 | S10 | preset parity 回归 | normalization no-op | workspace 回归 | 否 | E17 |
| R9 | agent guide 描述证据优先恢复 | S2、S8 | guide contract 测试 | skill anchor 测试 | CLI/doc drift 检查 | 否 | E14-E15 |
| R10（新增，修正 F-C） | preset author/review 规程可审计证据优先 gate 设计 | S2、S8 | review rubric mapping | anchor/fixture 测试 | aaf-review-negative-fixture 复跑 | 否 | E16 |

## 7. 严格串行开发单元

### U1. 建立 target-aware evidence feedback 模型（含 D9 分区消费）

- **Goal：** 在现有 `CorrectionContext`/`PromptContext` 上建立可序列化、可渲染、可按 target **分区消费**的 evidence-bound feedback 最小模型，并先固定当前 correction 注入行为。
- **Requirements：** R1、R2、R4、R7、R8。**Decision：** D1、D4、D5、D9。**Evidence：** E1-E4、E3a、E20、E21。
- **Dependencies：** 无。
- **Files：** `crates/ralph-core/src/correction/mod.rs`；`crates/ralph-core/src/event_loop/mod.rs`（prepend_correction_and_resume @7495 及其三个调用点 @5284/@5574/@5766）；`crates/ralph-core/src/event_loop/tests/chain_validation.rs`；`crates/ralph-core/tests/scenarios/correction_deterministic.yml`（若现有 runner 能表达 target 可见性）。**明确不修改：** precheck/consistency 生产路径（属 U2）、CLI（属 U4）。
- **Approach：**
  1. characterization：固定现有 correction 的排序（retry_key 序）、consume-on-use、ledger-first、无 target 全量渲染行为。
  2. `CorrectionContext` 新增可选 `target_hat: Option<String>` 与 evidence-bound detail 结构化字段（semantic/mechanical 分型）；`from_rejection` 搬运 `Rejection.target_hat`；旧结构反序列化兼容（新字段缺省 None/空）。
  3. **D9 落地：** `prepend_correction_and_resume` 增加当前 hat 上下文（三个调用点都在 `build_prompt(hat_id)` 内，E3a），渲染后由 `clear()` 改为 `retain(|c| c.target_hat 不属于 {None, 当前 hat})` 语义：无 target 与 target==当前 hat 的条目被渲染并消费；其余保留。
  4. semantic/mechanical 分型渲染：仅 mechanical 允许携带 expected_payload_template/required_fields 提示；semantic 渲染禁止任何替代值 section。
  5. 渲染顺序固定为「观察事实 → 违反条件 → 必须重新证明 → 禁止事项 → 重试状态」；不可信文本全部经 safe_display（既有 @204 模式）。
- **Test scenarios：**
  - U1-S1：现有无 target rejection 渲染结果仍含原 reason/stage/topic/retry（characterization）。
  - U1-S2：target=executor 的 correction 只在 executor prompt 出现，reviewer prompt 不出现。
  - U1-S3：**（F-A 守卫）** reviewer 先构建 prompt 后，target=executor 的 correction 仍在队列；executor 随后构建时可见；消费后移除。
  - U1-S4：semantic correction 渲染无 Expected payload / suggested replacement / 可复制业务值。
  - U1-S5：mechanical correction 仍可渲染 required fields 与现有 schema template。
  - U1-S6：恶意/超长/控制字符 message、topic、finding text 不破坏 block 结构。
  - U1-S7：多个 correction 按 retry_key 稳定排序；混合 target 队列的部分消费不影响其余条目顺序。
- **Acceptance Red：** 先跑 U1-S2/S3（target 注入 + 分区消费验收测试）：当前 CorrectionContext 无 target 字段且渲染后全清空，S2 因「reviewer 也能看到 executor correction」失败、S3 因「reviewer 构建后队列被清空」失败。失败必须来自断言而非编译错误/fixture 缺失。
- **最小单元测试拆分：** target 字段序列化/缺省/旧结构兼容；semantic/mechanical 分型与默认值；partition retain 的 match/non-match/None 三分支；semantic render 禁 replacement；mechanical render 保留 guidance；safe_display 与排序回归。
- **Red → Green → Refactor：** S2/S3 Red → 最小 target 字段 + retain 语义 → S2/S3 Green → S4/S5 渲染分支 Red → 分型渲染 → Green → characterization（U1-S1）保持 Green → Refactor 字段命名/渲染顺序 → U1 集成与回归。
- **最小实现范围：** 只改 correction 数据与 prompt 注入/渲染边界；不接 precheck/consistency 新 finding；不改 retry 算法；不删 task.resume。
- **集成验证：** 真实 `EventLoop::build_prompt(hat_id)` 与现有 correction scenario；不得 mock prompt filtering。
- **风险驱动测试：** Characterization（旧行为未以 target 断言固定）；prompt injection 回归（agent-controlled text）；serialization round-trip（recovery/prompt state）。
- **回归范围：** correction 模块测试、chain_validation、correction_deterministic、correction_three_escalation、prompt build 相关 EventLoop 测试；builtin preset 默认关闭路径不变。
- **预期文件变更：** correction/mod.rs（生产+测试：target/detail/render）；event_loop/mod.rs（partition 消费）；chain_validation.rs（新增 target acceptance 断言）；correction_deterministic.yml（target 可见性，若 runner 支持）。
- **完成标准：** U1-S1~S7 全绿；旧 correction 行为无意外变化；无新 bus topic；Evidence/Decision 已更新；可独立提交。
- **停止条件：** 若三个渲染调用点中任一无法获得当前 hat（与 E3a 冲突）、或 retain 语义破坏既有 consume-on-use 测试且无法在不改全局 prompt API 的前提下修复，停止并重比「队列分桶（D9-c）vs 渲染期 partition（D9-b）」。
- **风险与注意事项：** coordinator/ralph 诊断可见性：无 target 条目保持既有可见性，partition 只作用于有 target 的 detail；目标 hat 永不再激活时条目滞留队列——滞留条目随 retry_count 走既有 escalation（needs_escalation 时按现状转 human.guidance），不得无限堆积绕过升级。

### U2. 统一 precheck 与 consistency 的 evidence detail

- **Goal：** 让 precheck rejection 与 payload consistency finding 都能填充 U1 的 evidence-bound detail，保持两者证据能力边界不同。
- **Requirements：** R1、R2、R3、R5、R7。**Decision：** D2、D3、D6、D7。**Evidence：** E5-E10。
- **Dependencies：** U1。
- **Files：** precheck_gate_enforcement.rs；precheck_gate_runner.rs；event_loop/mod.rs（dispatch_precheck_rejection @13396）；event_policy_payload_consistency.rs；event_policy.rs；对应测试模块。**不修改：** prompt 文案/prose（属 U3）、CLI 输出面（属 U4）。
- **Approach：**
  1. 先清点 RejectedPayload 消费方与全部 precheck BDD fixture（D7 阈值守卫）。
  2. precheck：保留 `failed_checks` 索引与 `reason` 兼容字段（**形状不变**，E6/F-E）；新增可选 finding detail（check identity、observed evidence 或 unavailable、violated condition、required proof）；不要求 gate LLM 生成修复方案。
  3. synthetic：detail 只含 gate_silent_or_ambiguous、checklist scope、evidence unavailable；**禁止**把全索引解释为「每项已被事实验证失败」；legacy `failed_checks` 仍按 `RejectedPayload::synthetic` 现状填全索引（schema 契约）。
  4. consistency：在 evaluator 同源调用处，按 `when` AST 的 referenced_fields 对当前 payload 做有界取值（H2 的验证路径），构造稳定 violated invariant / required proof；不读历史、不解析 rule message 取字段。
  5. 两类 detail 注入 U1 CorrectionContext；`dispatch_precheck_rejection`（@13396）、policy rejection helper（policy.rs:90-147）与 legacy task.resume payload（enrich_task_resume_payload_full）从同一 normalized feedback 派生。
  6. semantic path 不填 expected_payload_template/suggested_*；mechanical path 保留旧字段。
- **Test scenarios：** U2-S1 LLM rejection 的 check/reason/target/retry/detail 完整进入 correction；U2-S2 malformed JSON 只产 malformed/unavailable；U2-S3 synthetic 标 unavailable 且 legacy failed_checks 不变；U2-S4 `all` 规则 referenced fields 按声明序进入 observed detail；U2-S5 nested/missing/null/string/number 的 observed 序列化有界稳定；U2-S6 semantic finding 的 loop feedback 不带 replacement（CLI 面 U4 接管）；U2-S7 mechanical 仍带 expected/actual/suggested；U2-S8 pass/miss 不生成 feedback。
- **Acceptance Red：** payload consistency semantic detail 验收与 precheck evidence 验收先跑：当前 SemanticGateViolation 无 observed detail、precheck 只有索引/reason → 缺结构化字段的真实断言失败。
- **最小单元测试拆分：** precheck finding normalization；synthetic unavailable 语义；consistency observation collector（payload pointer lookup）；有界序列化；semantic/mechanical 分支；legacy task.resume parity。
- **Red → Green → Refactor：** consistency observation Red → evaluator 派生有界观察 → Green → precheck normalization Red → 解析与 synthetic 分支 → Green → 同源断言 Red → 共同 normalized feedback → Green → 仅当行为测试全绿后整理重复格式代码。
- **最小实现范围：** 不扩展 predicate language、不读 history、不引入新 rejection topic；只增当前拒绝所需有限 detail 与传播。
- **集成验证：** 真实 payload_consistency reject/accept BDD、precheck pass/exhaust BDD；被拒事件不触发 downstream。
- **风险驱动测试：** property-style 表测试覆盖 predicate shapes；fuzz-like 有界输入（malformed JSON/超长 message）；round-trip（rejection payload → correction/task.resume）。
- **回归范围：** event_policy semantic gate 测试、payload_consistency 测试、precheck_gate_enforcement/runner 测试、policy_check 测试、既有 correction/precheck BDD。
- **预期文件变更：** precheck_gate_enforcement.rs（detail 构造）；precheck_gate_runner.rs（解析/normalization/dispatch detail）；event_loop/mod.rs（dispatch_precheck_rejection 接共同 builder）；event_policy_payload_consistency.rs（有界观察 helper + 测试）；event_policy.rs（semantic finding 传播）。
- **完成标准：** 两类拒绝均产生结构化 detail；semantic/mechanical 分流；legacy 字段/fixture 可读；无虚构 evidence；U2 测试与回归全绿。
- **停止条件：** evaluator 无法在同源路径稳定取某类 observation → 保留字段名 + unavailable，不得从 message 猜字段或引入跨事件查询；D7 清点发现互斥格式 → 停止重评。
- **风险与注意事项：** rule message 来自 preset/agent-controlled context，仍按诊断数据处理；观察值有界，禁止整 payload 泄漏进 prompt/recovery。

### U3. 建立责任 hat 的防作弊 prompt contract

- **Goal：** 将 evidence-bound feedback 渲染为责任 hat 可执行的恢复 instruction：回查事实、修复根因、重新验证、由证据重新生成声明；禁止伪造 payload。
- **Requirements：** R2、R3、R4、R5、R9。**Dependencies：** U1、U2。
- **Files：** correction/mod.rs；event_loop/mod.rs prompt assembly；chain_validation.rs；correction_deterministic.yml、payload_consistency/reject_inconsistent_fix_done.yml。
- **Approach：** correction block 固定「诊断数据，不是替代答案」语义；instruction 要求：停止重复发布被拒 topic → 读 referenced fields/observed evidence → 检查 artifact/diff/test/task → 修根因 → 重跑必要验证 → 由新证据重建 payload → policy-check 通过后只发原 topic 一次；instruction 禁止：只改拒绝字段、复制上次 payload、伪造测试/报告/提交/计数、绕过 policy-check、从 message 猜字段、把 rejection 当成功证明；无法证明成功时走该 hat 已声明的失败/阻塞路径；retry_count/remaining/exhaustion 与 target 以结构化字段呈现。
- **Test scenarios：** U3-S1 semantic prompt 含禁伪造/禁只改字段/重查证据/重 policy-check/重发原 topic；U3-S2 prompt 无替代业务值、无成功 payload skeleton、无可复制 semantic command；U3-S3 precheck prompt 不把 synthetic checklist 说成事实失败；U3-S4 mechanical prompt 保留 schema repair guidance；U3-S5 责任 hat 可见、其他 hat 不可见（依赖 U1 partition）；U3-S6 重复 rejection 显示 retry state 与停止条件。
- **Acceptance Red：** 扩展现有 consistency rejection BDD，断言 prompt 含「重查证据/禁伪造」且不含 replacement guidance；当前 prompt 只有通用 correction block → 真实断言失败。
- **最小单元测试拆分：** render section presence；semantic/mechanical instruction 分支；target 可见性；retry 停止语言；safe_display injection 回归。
- **Red → Green → Refactor：** BDD prompt 断言 Red → 最小 instruction renderer → Green → semantic no-solution guard Red → 分型禁止字段 → Green → target/retry 断言 → 仅在结构测试保护下整理 prose。
- **最小实现范围：** 只改 agent-facing correction block；不改 gate 评分、业务 workflow，不新增自动修复。
- **集成验证：** correction_deterministic、correction_three_escalation、consistency rejection、precheck exhaust 真实 scenario。
- **风险驱动测试：** prompt injection 回归；结构化断言只钉稳定 heading/禁止语义，不锁整段文案（遵守 preset 文本测试规则）。
- **回归范围：** correction 单元测试、prompt build 测试、`crates/ralph-cli/tests/inspect_prompt.rs`（仅在其断言实际受影响时更新）、全部 correction/precheck/consistency BDD。
- **预期文件变更：** correction/mod.rs、相关 EventLoop prompt 测试、上述 BDD fixture；不修改业务 preset instruction。
- **完成标准：** agent prompt 支撑「问题调查—真实修复—验证—重新声明」闭环；semantic prompt 无替代答案；既有 prompt contract 全过。
- **停止条件：** prompt 文案变化触发大范围不稳定 snapshot → 改用结构化 marker/关键句断言，禁止锁定整段文案。
- **风险与注意事项：** 防作弊 instruction 只能是通用 agent-facing 规则；不写内部 ledger 路径、函数名、一次性事故、具体 preset 名。

### U4. 统一 CLI policy-check 与 runtime feedback 输出

- **Goal：** `ralph emit --policy-check --output json` 与真实 runtime 对 semantic/mechanical rejection 使用同一可机读分类；semantic 输出携带 observed/invariant/required-proof，且无 replacement answer。
- **Requirements：** R2、R3、R6、R8。**Dependencies：** U2、U3。
- **Files：** ralph-cli/src/policy_check.rs（ValidationError @2167、finding_record @2881、enrich @2292、wrapper @2390）；ralph-cli/tests/integration_emit_policy.rs；event_policy.rs 相关测试；必要时 emit_schema_hint.rs。
- **Approach：** 在 finding enrichment 中按 `reason_code=semantic_gate_violation` 分流；semantic path 输出 gate、referenced_fields、observed facts、violated invariant、required proof；`suggested_payload_shape`/`suggested_command` 保持 None（**现状已如此，E9a——此处是加固不是新行为**）；mechanical path 保持现有 field/expected/actual/field_description/suggested 逻辑；`--output text` 只渲染结构化字段为可读摘要；policy-check 与 apply 继续同源。
- **Test scenarios：** U4-S1 semantic consistency hit 的 JSON 有 gate/referenced_fields/observed/required proof 且无 suggested_*；U4-S2 timing/state gate 字段列表为空或 unavailable，不把 gate ID 填进 field；U4-S3 missing field 保持 suggested shape/command；U4-S4 invalid value/type mismatch 保持 expected/actual；U4-S5 text/json 同源；U4-S6 policy-check 通过仍不写盘、apply 同 decision。
- **Acceptance Red（修正 F-B）：** Red = semantic 输出契约测试：断言 JSON 含 observed facts / violated invariant / required proof 字段——当前 ValidationError 无这些字段 → 真实失败。**suggestion omission 测试单独作为 characterization 守卫：它在基线上即通过（E9a），不是 Red，作用是防未来回归；执行时不得把它当 Acceptance Red 报告。**
- **最小单元测试拆分：** semantic enrichment 分支；observed/invariant/proof 传播；mechanical 回归分支；text/json projection；suggestion omission 守卫（characterization）；policy-check/apply 同 decision。
- **Red → Green → Refactor：** semantic JSON（observed/proof 缺失）Red → 扩展 finding/ValidationError 传播 → Green → suggestion omission 守卫直接绿（记录为 characterization）→ mechanical 回归 Red/Green → text projection → integration。
- **最小实现范围：** 只改 policy finding → CLI response 的传播与分流；不改 ralph emit 参数、不增命令、不改 evaluator 语义。
- **集成验证：** `cargo nextest run -p ralph-cli --test integration_emit_policy` 相关子集；`cargo nextest run -p ralph-core -- event_policy payload_consistency`。
- **风险驱动测试：** JSON round-trip；long/unicode/control 输入有界输出；CLI/apply contract parity。
- **回归范围：** 全部 policy-check ValidationError 测试、CLI emit integration、core semantic gate 测试、disabled path。
- **预期文件变更：** policy_check.rs、integration_emit_policy.rs，必要时 core finding 类型/测试。新增 ValidationError 字段必须 optional/skip-empty；旧字段不得复用承载不同含义。
- **完成标准：** semantic/mechanical 严格分流；旧机械输出无回归；policy-check/apply 同源；无新增公开 CLI 参数。
- **停止条件：** observed/required proof 需要改变公开 JSON 版本或破坏既有 consumer → 停止并记录 API 兼容决策，不得静默改字段语义。
- **风险与注意事项：** ValidationError 是用户可见契约；新增字段 optional/skip-empty。

### U5. 用真实 EventLoop 场景证明证据约束闭环

- **Goal：** 将「拒绝不成功、责任 hat 收到证据反馈、只改声明不能成功、真实修复后可通过、耗尽保留最后反馈」转化为可执行 BDD/ATDD。
- **Requirements：** R1-R8。**Dependencies：** U1-U4。
- **Files：** tests/scenarios/ 下 correction_deterministic.yml、correction_three_escalation.yml、payload_consistency/reject_inconsistent_fix_done.yml（+accept sibling）、2026-07-02-precheck-gate-exhaust.yml，必要时新增 evidence-bound fixture；tests/scenarios.rs（run_workflow_guard_scenario @1971 的断言辅助）；必要时 chain_validation.rs。
- **Approach：** 现有场景断言从「有 correction block」扩展为结构化字段与 prompt contract（不锁整段文案）；consistency negative fixture 加第二轮（仅改字段仍拒）与第三轮（改事实 artifact 后合法事件进 downstream）；若 runner 无 artifact mutation seam，新增最小 fake evidence source，不 mock 真正 policy/apply；precheck fixture 覆盖 LLM rejection、synthetic silent rejection 与 exhaust；multi-hat fixture 断言 target-specific 可见性（含 F-A 分区语义）；全部走 run_workflow_guard_scenario。
- **Test scenarios：** U5-S1 consistent payload accepted + downstream；U5-S2 inconsistent rejected + evidence correction queued；U5-S3 重复同 payload 无假终态 + retry 递增；U5-S4 证据改变后重生成 accepted + retry reset；U5-S5 precheck LLM reject 结构化路由 target；U5-S6 synthetic silent 标 unavailable；U5-S7 exhaust 终态恰好一次 + 最后反馈保留；U5-S8 target correction 不注入无关 hat（且不被其吞掉）；U5-S9 disabled 路径行为不变。
- **Acceptance Red：** 先跑新增/扩展场景；正确 Red 必须来自目标行为缺失，不是 YAML 语法/fixture 路径/runner 注册错误。
- **最小单元测试拆分：** 不复制 U1-U4 纯函数测试；只补跨模块事件时序、prompt 注入、terminal 不污染、retry reset。
- **Red → Green → Refactor：** false-success 场景 Red → 接 U1-U4 行为 Green → precheck LLM/synthetic Red → Green → target 可见性 Red → Green → exhaust/最后反馈 Red → Green → fixture 整理。
- **最小实现范围：** 只改真实场景与 runner 断言；若生产代码暴露真正缺口，回到拥有该行为的 U1-U4，不在 U5 写生产 workaround。
- **集成验证：** `cargo nextest run -p ralph-core --test scenarios -- correction`、`-- payload_consistency`、`-- precheck` 及受影响 EventLoop 测试。
- **风险驱动测试：** state-machine/recovery（拒绝不污染 terminal）；idempotency（exhausted 不重发终态）；若现有场景支持 replay/restart 则覆盖 correction ledger 保留。
- **回归范围：** 全部 correction/precheck/consistency scenario、chain validation、terminal guard、workflow guard、preset 静态 lint scenarios。
- **预期文件变更：** 上述 YAML fixtures、scenarios.rs 断言辅助、必要的真实 EventLoop 测试；不新增 source-only 文本测试。
- **完成标准：** S1-S10 均可执行且通过；拒绝路径无成功副作用；真实修复路径可达；耗尽稳定；旧场景保持通过。
- **停止条件：** runner 无法观察事实变更 → 先记录 fake seam 最小接口与证据再停止；不把「改 payload 就通过」当作 S7 实现。
- **风险与注意事项：** mock response 必须表达真实 runtime path；禁止用预注入 accepted event 绕过 policy/gate。

### U6. 同步 agent guide、preset review 规则与文档契约

- **Goal：** loop 内 agent、preset author、preset reviewer 使用同一「证据优先、语义 gate 不给答案」规则，防止代码行为与注入 prompt/评审规程漂移。
- **Requirements：** R3、R6、R9、**R10（本版补齐，修正 F-C）**。**Dependencies：** U1-U5。
- **Files：** data/ralph-tools-emit.md；data/ralph-tools-precheck.md；skills/ralph-preset-author/references/{commands,finding-rubric,patterns,prompt-visibility}.md；skills/ralph-preset-review/references/ 对应文件（含 agent-skill-audit.md）；必要时 skills/ralph-preset-review/tests/test_skill_anchors.py；CONCEPTS.md 仅在术语已稳定且有对应模式时增补；CLAUDE.md/AGENTS.md 仅在通用硬规则实际变化时同步（cp 保持一致）。
- **Approach：** emit guide 明确 semantic rejection 读 gate/referenced_fields/observed facts/required proof，回查 artifact/测试/任务，重新 policy-check，由证据生成原 topic；禁止复制 payload、伪造状态、从 message 猜字段；precheck guide 明确 failed_checks 是线索不是模板、synthetic/unavailable 时不得假设 checklist 已被事实验证、修复后原 topic 重发；author/review references 增加通用审计锚点（semantic gate 是否描述 violated invariant/required proof、是否避免替代答案、是否有责任 target/bounded retry/honest failure path）；行号引用改动后 `sed` 复核；跑 scripts/check-cli-doc-drift.sh；注入 guide 不写计划编号/preset 专属案例/内部 ledger 路径/reviewer-only 背景。
- **Test scenarios：** U6-S1 guide 区分 mechanical schema repair 与 semantic evidence repair；U6-S2 每条规则含触发条件、动作、字段来源、停止条件；U6-S3 author/review references 对缺失 invariant/替代答案/无 target/无限 retry 产生可审计 finding；U6-S4 注入 guide 不含具体 builtin preset 名、计划编号、事故路径、内部 ledger 路径、reviewer-only 术语；U6-S5 文档行号/命令参数与源码和 `ralph <cmd> --help` 一致。
- **Acceptance Red：** 先跑受影响的 skill anchor/doc drift 测试；当前 guide 缺 evidence-bound 规则或旧引用漂移时产生真实 Red。
- **最小单元测试拆分：** guide anchor presence；禁词/禁案例扫描；命令帮助对齐；review rubric mapping；reference parity。
- **Red → Green → Refactor：** skill contract Red → 更新 emit/precheck guide → Green → author/review anchor Red → 更新两套 references/tests → Green → drift/help smoke → 文字精简与去计划化检查。
- **最小实现范围：** 只同步 agent-facing 与 operator review 规程；不泄露 runtime 实现细节进注入 prompt；不改 builtin preset topology。
- **集成验证：** scripts/check-cli-doc-drift.sh；`.venv/bin/python -m pytest skills/ralph-preset-review/tests`；`ralph emit --help` / `ralph preset check --help` smoke。
- **风险驱动测试：** static contract 扫描；命令 schema drift；skill mirror/anchor parity。
- **回归范围：** inspect_prompt.rs、test_skill_anchors.py、相关 author/review fixture 流程、data guide drift。
- **预期文件变更：** 上述 data/*.md 与明确受影响的 skills/ references/tests。
- **完成标准：** guide 规则按「触发—动作—字段来源—停止条件」写成通用规则；命令/anchor/drift 验证通过；无计划化或内部实现泄漏。
- **停止条件：** 现有 references 无对应 finding/rubric anchor → 先记录新增 finding ID 影响并同步两份 rubric/commands，不得只改一份或新增只查文案的脆弱测试。
- **风险与注意事项：** 本 Unit 最易 scope expansion；不新增 preset 专属说明、不复制完整字段表、不新增 byte-equality 文案测试。

## 8. Unit 串行依赖图

```
U1（target-aware 模型 + D9 分区消费）
  ↓ U2 使用 U1 的 detail/target/分型
U2（precheck/consistency evidence detail）
  ↓ U3 依赖两类 evidence data
U3（防作弊 prompt contract）
  ↓ U4 与已定 prompt contract 对齐
U4（CLI/runtime 输出契约）
  ↓ U5 观察全链路
U5（真实 EventLoop BDD 闭环）
  ↓ U6 在行为固定后同步文档
U6（guide 与 review 规程同步）
```

- U1→U2：U2 必须复用 U1 已验证的 detail/target/分型，否则出现第二套 precheck/consistency payload。
- U2→U3：只有 evidence data 可用后，prompt 才能不依赖自然语言猜测。
- U3→U4：CLI 契约必须与 prompt contract 一致，避免 CLI 给答案而 prompt 禁答案。
- U4→U5：BDD 需同时观察 runtime feedback、CLI/decision 语义、accepted events 与 downstream 副作用。
- U5→U6：guide 只在可观察行为与字段由真实场景固定后同步，避免文档先于实现编造字段。
- 禁止提前实现：U1 不接 gate；U2 不改 prompt prose；U3 不改 CLI JSON；U4 不改 BDD 语义；U5 不写生产 workaround；U6 不新增 runtime 行为。

## 9. 执行命令清单

| 时机 | 命令 | 验证目的 | 预期结果 | 失败处理 |
| --- | --- | --- | --- | --- |
| U1 Red/Green | `cargo nextest run -p ralph-core -- correction` | CorrectionContext/PromptContext/render/partition/retry | 当前 Unit 测试通过 | 停止；确认测试真正执行 |
| U1 EventLoop 集成 | `cargo nextest run -p ralph-core --test scenarios -- correction` | 真实 correction 注入与消费 | correction scenarios 通过 | 停止 U1 |
| U2 core | `cargo nextest run -p ralph-core -- event_policy_payload_consistency precheck` | evaluator/precheck parser/runner/结构化 detail | 通过 | 停止 U2 |
| U2 policy 集成 | `cargo nextest run -p ralph-core -- event_policy` | SemanticGateViolation 同源传播与 disabled path | 通过 | 停止 U2 |
| U3 prompt/BDD | `cargo nextest run -p ralph-core --test scenarios -- correction payload_consistency precheck` | 防作弊 prompt 与真实 rejected path | 通过 | 停止 U3 |
| U4 CLI policy | `cargo nextest run -p ralph-cli --test integration_emit_policy` | policy-check JSON/text、机械/语义分流 | 通过 | 停止 U4 |
| U4 CLI unit | `cargo nextest run -p ralph-cli --bin ralph -- policy_check` | enrichment 与 semantic 字段传播 | 通过 | 停止 U4 |
| U5 BDD | `cargo nextest run -p ralph-core --test scenarios -- correction precheck payload_consistency` | 完整状态流、拒绝副作用、修复通过、耗尽 | 通过 | 停止 U5 |
| U5 EventLoop | `cargo nextest run -p ralph-core -- chain_validation workflow_guard terminal` | rejected terminal 不污染、下游不误触发 | 通过 | 停止 U5 |
| U6 drift | `scripts/check-cli-doc-drift.sh` | CLI 文档/源码引用漂移 | exit 0 | 修文档重跑 |
| U6 skill tests | `.venv/bin/python -m pytest skills/ralph-preset-review/tests` | anchor/rubric contract | 通过 | 停止 U6 |
| U6 help smoke | `cargo run -p ralph-cli --bin ralph -- emit --help`；`cargo run -p ralph-cli --bin ralph -- preset check --help` | 命令语法与 guide 一致 | 正常输出 | 修文档重跑 |
| 每 Unit | `cargo fmt --check`；`cargo clippy` | 格式/lint | exit 0 / 无新 lint | 修复重跑 |
| 最终基线 | `./scripts/run-tests.sh` | workspace nextest 两阶段 + doctest | 全绿 | 按仓库 flake 兜底规则诊断，不得宣称完成 |

失败一律不得进入下一步；环境/命令错误先修正环境并重新取证。

## 10. 最终质量门禁

- R1-R10 均至少关联一个 BDD Scenario、一个单元/集成测试与一个 Evidence。
- S1-S10 全部经真实 EventLoop/CLI 测试；无 source-only 行为替代测试。
- semantic rejection 反馈含稳定 gate/reason、observed facts、violated invariant、required proof、target 与 retry state；不含 replacement payload（suggestion omission 守卫证明现状不回退）。
- mechanical rejection 保留字段级 expected/actual/suggested shape/command。
- precheck LLM/synthetic/consistency rejection 均进入统一 correction path；synthetic 不伪造事实且 legacy failed_checks 形状不变。
- target-specific correction 不被无关 hat 处理、**不被无关 hat 的 prompt 构建吞掉**（D9/U1-S3/U5-S8）；无 target 的既有 diagnosis fallback 不回归。
- rejected event 不写入 accepted business events、不触发 downstream、不污染 terminal success。
- 真实证据改变后重生成事件可通过；只改声明无假成功。
- retry key 成功复位；耗尽只发一次终态并保留最后 feedback。
- 默认关闭/旧配置/旧 rejection fixture 可读；builtin preset topology 无变化。
- nextest 相关测试、run-tests.sh、fmt、clippy、CLI drift、Python tests 全绿。
- 无新增 skip/ignore、弱化断言、无解释 snapshot/golden 更新；data/*.md 满足触发条件/动作/字段来源/停止条件/去计划化约束。
- 所有执行关键决策置信度不低于 0.85；实现中发现新公开调用方、不同 prompt routing 或 JSON contract 时先停止并更新计划。

## Definition of Done

### 全局完成条件

- U1-U6 严格按序完成，每个 Unit 有真实 Acceptance Red、最小 Green、Refactor、Integration、Regression、Close 证据。
- diff 只覆盖本计划列出的 behavior/test/guide 范围；废弃实验代码已删除。
- 生产代码无「骗过 gate」快捷路径；semantic feedback 只描述问题与验证条件。
- Evidence Ledger / Decision Record 在执行后补充实际测试结果，不把执行发现伪装成计划前事实。

### 每个 Unit 完成条件

- 当前 Unit 的 Scenario、单元、集成与受影响回归均通过。
- Acceptance Red 由目标能力缺失导致，不是环境/fixture/命令/语法错误；characterization 守卫（如 U4 suggestion omission）不计为 Red。
- 无提前实现后续 Unit、无测试债务、无无关清理。
- Build/fmt/clippy 通过，无新增 skip/ignore。
- 当前 Unit 可独立提交；下一 Unit 只依赖其已验证能力。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
| --- | --- | --- |
| 这是实施计划而不是 Roadmap | 是 | 每 Unit 有真实入口、行为、Red、测试、回归与停止条件 |
| Executor 是否仍需做关键设计决策 | 否 | 反馈载体、分流、target routing、**D9 队列生命周期**、Unit 顺序与测试入口均已固定 |
| 所有文件和接口是否有代码库证据 | 是 | 29/29 路径存在性核查 + E1-E21 逐行锚点（基线 a9dff24e） |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1-D9 最低 0.88（D7），阈值附近项带执行期验证与停止条件 |
| 是否存在未处理的低置信度假设 | 否 | 原 H1 已验证升级（E3a）；H2 有降级路径不阻塞 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 模型+定向消费 / U2 normalization / U3 prompt contract / U4 CLI contract / U5 BDD / U6 文档同步 |
| 每个 Unit 是否可以独立验证 | 是 | 各有 Acceptance Red、命令与回归范围 |
| 每个 Unit 是否有真实 Red | 是 | U4 的无效 Red 已修正（F-B）；characterization 与 Red 严格区分 |
| 每个 Unit 是否包含回归范围 | 是 | 各 Unit 明列直接/相邻/默认关闭路径 |
| 是否存在未来 Unit 依赖 | 否 | 依赖图线性；每 Unit 禁止提前实现 |
| 是否存在泛化任务描述 | 否 | 全部绑定具体符号、行号与断言 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 第 5、6 节矩阵 + 各 Unit Test scenarios |
| 所有关键决策是否有 Evidence | 是 | D1-D9 引用 E1-E21 |
| 计划是否可以严格串行执行 | 是 | 第 8、9 节固定顺序与失败停止 |
