---
title: Parallel Forge Auditor Reporter 单事件终态收敛 - Plan
type: refactor
date: 2026-07-29
origin:
  - docs/achieved/brainstorms/2026-07-29-parallel-forge-wave-settlement-and-evidence-gates-requirements.md
  - docs/plans/2026-07-29-001-fix-parallel-forge-static-wave-settlement-plan.md
  - docs/plans/2026-07-29-002-feat-parallel-forge-reuse-status-plan.md
  - docs/plans/2026-07-29-003-feat-parallel-forge-readonly-hat-gates-plan.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan
execution: code
---

# Parallel Forge Auditor Reporter 单事件终态收敛 - Plan

## 0. 计划状态

- **状态：READY（受 001、002、003 完成门禁约束）**
- **代码基线：** `c96c47f3590e05729655715f0aebd37265b5cf09`
- **当前分支：** `pittcat-dev`
- **实施前置：**
  1. 001 的静态 wave 结算与失败恢复合同全部落地并通过 Definition of Done；
  2. 002 的 reuse checkpoint 与 accepted-event 幂等合同全部落地并通过 Definition of Done；
  3. 003 的 strict read-only runtime guard、`allowed_write_paths` 与 readonly evidence 合同全部落地并通过 Definition of Done；
  4. 实施时以 post-003 的真实文件、topic 和测试为准；若 001–003 实际接口与本计划引用不一致，必须触发停止条件并修订计划，不得猜测适配。
- **调查范围：**
  Parallel Forge `audit → report → plan_end` 流程、Auditor/Reporter hat 合同、event schema、completion promise、required event、payload consistency、manager report 模板、真实 EventLoop BDD、supervisor 终态清理和相关静态 preset 测试。
- **已执行的验证：**
  - 阅读 `presets/en/parallel-forge.yml` 的 flow、event loop、Auditor 与 Reporter；
  - 阅读 `presets/schemas/parallel-forge.yml` 的 `forge.audit.done`、`forge.report.done`、`LOOP_COMPLETE`；
  - 阅读成功与失败真实 EventLoop BDD；
  - 阅读 `crates/ralph-cli/src/presets.rs` 的 Parallel Forge completion 静态合同；
  - 阅读 runner 的 `handle_termination` 与 supervisor bridge 的 `finalize_terminal_cleanup`；
  - 阅读 001–003 计划及终态清理的已解决问题文档。
- **未执行的验证：**
  本计划不运行 Acceptance Red、nextest、lint、build 或 E2E；命令已固定在 Unit 与执行命令清单中。
- **工作区说明：**
  调查过程中曾短暂观察到并发工作产生的无关变更；最终自检时这些变更已不在
  `git status` 中。本计划未修改或清理它们，当前仅新增本计划文件。
- **阻塞项：** 无设计阻塞。

## 1. 功能目标

### 1.1 业务目标

把 Parallel Forge 的最后一段收敛为一条容易理解、容易恢复、没有重复职责的单链：

```text
forge.full.verified
  → Auditor：恰好一条审计事件
  → Reporter：恰好一条 forge.report.done
  → runtime 接受终态并清理 worktree
```

Auditor 负责决定“是否可交付以及交付到什么程度”。
Reporter 只负责把已经确定的事实整理成经理报告并原样发布终态。
Reporter 不再补发 `LOOP_COMPLETE`，也不再自行清理 worktree。

### 1.2 用户或调用方

- 操作者：只需查看一份与终态 payload 一致的经理报告。
- Auditor：只读最终证据，输出最终交付结论。
- Reporter：聚合已接受结论，写报告并发布唯一终态。
- EventLoop：校验 schema、payload consistency、单事件预算与 completion promise。
- loop runner/supervisor bridge：在接受终态后统一、幂等地清理 slot worktree。

### 1.3 当前行为

1. Auditor 已是单事件 hat，但 `verdict=REJECTED` 后究竟映射成 `PARTIAL` 还是 `FAILED` 由 Reporter 临场判断。
2. Reporter 一次业务收敛需要先发布 `forge.report.done`，下一次 activation 再发布 `LOOP_COMPLETE`。
3. `completion_payload_match` 专门比较两条终态事件中的 `report_path`，是双事件设计产生的补偿复杂度。
4. Reporter 还会读取 worktree map 并执行 `git worktree remove`。
5. runner 在任何业务终止时已经调用 `finalize_terminal_cleanup`；bridge 会遍历 supervisor slot worktree，`NotFound` 视为幂等成功，单个清理失败只记录 warning。
6. manager report 在 runtime 清理发生前写入，因此 Reporter 无法真实报告“清理已经成功”。

### 1.4 目标行为

- Auditor 继续只发布一条：
  - 可完成审计时发布 `forge.audit.done`；
  - 缺少关键证据、环境无法审计或合同无法判定时发布 `forge.plan.blocked`。
- `forge.audit.done` 同时携带：
  - `verdict: ACCEPTED | REJECTED`；
  - `delivery_status: COMPLETED | PARTIAL | FAILED`；
  - 审计 artifact、digest、最终 HEAD、Tester evidence 引用和 `plan_key`。
- 固定自洽关系：
  - `ACCEPTED` 只能配 `COMPLETED`；
  - `REJECTED` 只能配 `PARTIAL` 或 `FAILED`；
  - `BLOCKED` 不得伪装成 `forge.audit.done`，必须走 `forge.plan.blocked`。
- 新增
  `presets/templates/parallel-forge/final-audit.template.md`
  作为复杂状态判定表的唯一模板；Auditor 必须 materialize 后填写，
  hat instructions 只引用模板，不复制整张表。
- Reporter 只发布一条 `forge.report.done`。
- `forge.report.done` 成为 `event_loop.completion_promise` 和 flow 的唯一终态。
- `required_events: [forge.report.done]` 保留，防止公共 preset 被其他事件提前终止。
- 删除 `completion_payload_match`，因为不再存在第二条终态 payload。
- Reporter 根据 trigger 做纯转录：
  - `forge.audit.done`：`status=delivery_status`，`final_audit=verdict`；
  - `forge.plan.blocked`：`status=BLOCKED`，`final_audit=BLOCKED`；
  - `work.failed`：`status=FAILED`，`final_audit=FAILED`。
- payload consistency 在同一个 `forge.report.done` payload 内拒绝错误组合；不新增 precheck。
- Reporter 复用 003 的 strict read-only runtime guard，只允许写 `docs/reports/**`。
- Reporter 删除所有 worktree 删除命令；runtime 是唯一清理 owner。
- 报告只写“终态接受后由 runtime 清理”，不宣称尚未发生的清理结果。

### 1.5 本次需求

- **R1.** Auditor 每次 activation 恰好发布 `forge.audit.done` 或 `forge.plan.blocked` 之一。
- **R2.** Auditor 不引入 precheck；003 strict read-only guard 是其代码树与证据不变性的强门禁。
- **R3.** `forge.audit.done.verdict` 只允许 `ACCEPTED | REJECTED`。
- **R4.** `forge.audit.done.delivery_status` 只允许 `COMPLETED | PARTIAL | FAILED`，并遵守固定 verdict 映射。
- **R5.** Reporter 不得重新判断交付程度，只能转录 trigger 的已确定状态。
- **R6.** Reporter 每次 activation 恰好发布一条 `forge.report.done`，不得发布 `LOOP_COMPLETE`。
- **R7.** `forge.report.done` 是 Parallel Forge 唯一 completion promise、唯一 terminal topic 和 flow 终点。
- **R8.** `forge.report.done` 必须包含 `report_path`、`report_digest`、`status`、`final_audit`、`plan_key`、`source_topic`、`source_verdict`、`source_evidence_path`。
- **R9.** payload consistency 必须拒绝同 payload 内的错误 source/status/final_audit 组合。
- **R10.** Reporter 不引入 precheck；schema、payload consistency、单事件预算和 003 runtime guard 足以覆盖确定性合同。
- **R11.** Reporter 只能写 `docs/reports/**`，不得修改 Auditor/Tester artifact、源码、测试、Git ref 或 worktree。
- **R12.** worktree 清理只由 runner 的 terminal cleanup 执行；Reporter 不运行任何清理命令。
- **R13.** runtime 清理失败不得篡改已接受的 `forge.report.done`；现有 warning/诊断仍是运维证据。
- **R14.** accepted `forge.report.done` 后 loop 立即结束；resume 不得再唤醒 Reporter 补发第二条事件。
- **R15.** 现有成功、阻塞/失败真实 EventLoop BDD 必须改为单终态，且继续断言后续成功事件不会越过失败路径。
- **R16.** 不改变 001 wave/correction、002 reuse、003 Reviewer/Verifier/Tester 门禁。
- **R17.** `delivery_status` 的判定必须来自 final-audit 模板的逐 Unit
  delivery matrix，不得按自然语言印象判断。

### 1.6 输入、输出、状态与错误语义

| 边界 | 输入 | 输出 | 状态变化 | 错误语义 |
|---|---|---|---|---|
| Auditor accepted/rejected | accepted `forge.full.verified` 与其 evidence | 一条 `forge.audit.done` | flow 从 `audit` 进入 `report` | schema/consistency/readonly guard 拒绝时不推进 |
| Auditor blocked | 缺失或不可验证证据 | 一条 `forge.plan.blocked` | Reporter 被唤醒 | 必须先写 block artifact |
| Reporter audit path | accepted `forge.audit.done` | 一条 `forge.report.done` | completion promise honored | 任一字段不一致则拒收并保持 loop 未完成 |
| Reporter blocked path | accepted `forge.plan.blocked` | 一条 `forge.report.done` | completion promise honored | 固定映射为 BLOCKED/BLOCKED |
| Reporter failed path | accepted terminal `work.failed` | 一条 `forge.report.done` | completion promise honored | 固定映射为 FAILED/FAILED |
| runtime cleanup | accepted completion | 无业务事件 | slot worktree 被幂等移除 | 失败只记 warning，不回滚终态 |

### 1.7 兼容、性能、安全与非目标

- 不保持 Parallel Forge 内部 `LOOP_COMPLETE` 双事件兼容；仓库明确允许清理旧合同。
- 不改变其他 preset 使用 `LOOP_COMPLETE` 的行为。
- 少一次 Reporter activation，终态路径应减少一次 backend 调用。
- 不新增网络、数据库或外部依赖。
- 不新增 Auditor/Reporter precheck。
- 不把 Reporter 升级为审计者、修复者或清理者。
- 不改变 runtime 对其他 completion promise 的通用支持。
- 不扩展 003 strict readonly guard 的通用实现；只消费其已验证能力。

## 2. 代码库现状与证据

### 2.1 当前入口与调用链

```text
presets/en/parallel-forge.yml
  → event_loop.flow
  → HatRegistry 选择 auditor / reporter
  → EventLoop schema + policy + completion gate
  → runner 接受 completion promise
  → handle_termination
  → SupervisorBridge::finalize_terminal_cleanup
```

Schema 的 single source of truth 是
`presets/schemas/parallel-forge.yml`。
内嵌 preset 由现有 manifest/build 链路生成。
真实行为回归入口是
`crates/ralph-core/tests/scenarios/parallel_forge_declared_flow_runtime.yml`
和
`parallel_forge_declared_flow_failed_runtime.yml`，
均由 `run_workflow_guard_scenario` 驱动。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `presets/en/parallel-forge.yml` flow | 当前 `report` 接受 `forge.report.done`，再由 `plan_end` 等待 `LOOP_COMPLETE` | flow 可直接折叠为 report terminal | 高 |
| E2 | 同文件 `event_loop` | completion promise 是 `LOOP_COMPLETE`，required event 是 `forge.report.done`，并配置 path match | 双事件产生额外匹配合同 | 高 |
| E3 | 同文件 Auditor | 已经只发布 `forge.audit.done` 或 `forge.plan.blocked` | 不需要改变 Auditor 的单事件拓扑 | 高 |
| E4 | 同文件 Reporter | 同时发布 `forge.report.done`、`LOOP_COMPLETE`，并在完成后手工清理 worktree | 需删除双事件和重复清理职责 | 高 |
| E5 | `presets/schemas/parallel-forge.yml` | audit verdict 允许 BLOCKED；report status 的 REJECTED 映射可二选一；另有 LOOP_COMPLETE schema | 当前合同存在临场判断和多余 schema | 高 |
| E6 | `crates/ralph-cli/src/loop_runner/runner.rs::handle_termination` | 任何业务终止先调用 bridge terminal cleanup | runtime 已是清理入口 | 高 |
| E7 | `supervisor_bridge.rs::finalize_terminal_cleanup` | 遍历全部 wave worktree；NotFound 幂等；单个失败不阻断退出 | Reporter 不应重复清理 | 高 |
| E8 | 两个 Parallel Forge declared-flow scenario | success/failure 都需要 Reporter 两次 activation，且显式断言 LOOP_COMPLETE | 更新既有 BDD 即可，不需新建大矩阵 | 高 |
| E9 | `crates/ralph-core/tests/scenarios.rs` | 两个 scenario 使用真实 EventLoop runner | 满足项目 BDD 硬规则 | 高 |
| E10 | `crates/ralph-cli/src/presets.rs::test_parallel_forge_configures_report_done_path_match` | 静态测试锁定 002 的双 payload path match | 本计划必须替换该测试合同 | 高 |
| E11 | `EventLoopConfig.completion_promise` 与现有多 topic fixture | completion promise 是配置值，不限定为 LOOP_COMPLETE | `forge.report.done` 可直接成为 promise | 高 |
| E12 | `event_policy_payload_consistency` 与 ce pipeline 配置 | 支持当前 payload 的 enum/literal 组合拒绝，不做外部 I/O | 适合固定 Reporter 转录合同 | 高 |
| E13 | 003 计划 | strict readonly guard 和 `allowed_write_paths` 是前置能力；Auditor/Reporter 未纳入 003 | 本计划只为两个 hat 配置该能力，不重做 guard | 高 |
| E14 | `manager-report.template.md` | 模板含 status/final_audit 和 worktree 状态，但报告写在 runtime cleanup 之前 | 清理部分应改为 ownership/pending 说明 | 高 |
| E15 | 项目 preset/schema 硬规则 | topic、required fields、flow 变更必须同步 schema、BDD、skills 与文档 | Unit 2 必须完成整条下游同步 | 高 |

### 2.3 已确认受影响范围

| 范围 | 位置 | 影响 |
|---|---|---|
| Preset | `presets/en/parallel-forge.yml` | Auditor/Reporter、flow、completion、policy |
| Schema | `presets/schemas/parallel-forge.yml` | audit/report fields，删除 Parallel Forge 的 LOOP_COMPLETE schema |
| 模板 | `presets/templates/parallel-forge/manager-report.template.md` | 确定性状态与清理 ownership |
| 模板 | 新增 `presets/templates/parallel-forge/final-audit.template.md`，同步同目录 `README.md` 与现有 template registry | Auditor 复杂状态表的单一来源 |
| 内嵌 preset 测试 | `crates/ralph-cli/src/presets.rs` | 用单终态断言替换 path-match 断言 |
| EventLoop BDD | 两个 declared-flow YAML 与 `scenarios.rs` 注释 | success/failure 单事件终态 |
| runner 测试 | `crates/ralph-cli/src/loop_runner/` 现有 terminal cleanup 测试位置 | 非默认 completion promise 后仍清理 |
| Agent skill | `crates/ralph-core/data/ralph-tools-emit.md`、`ralph-tools-opac.md` | 单事件终态与无需补发 |
| Operator skill | `skills/ralph-preset-common/references/` 的相关 commands/checklist/rubric | completion topic 可为业务事件、单事件预算审计 |
| 文档 | `CLAUDE.md`、`AGENTS.md`、`.cursor/rules/multi-hat-isolation.mdc` | builtin preset 描述同步 |

不修改 manifest、index 或 zsh completion，因为 preset 名称和可见性不变。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除原因 | 置信度 |
|---|---|---|---|---|---|---|
| KTD1 | 最终 completion topic | 保留 LOOP_COMPLETE；报告事件直接终止；新增第三事件 | `forge.report.done` 直接终止 | E1,E2,E8,E11 | 保留会继续双 activation；第三事件更复杂 | 0.99 |
| KTD2 | required_events | 删除；保留 `forge.report.done` | 保留 | E2,E10,E15；EventLoop 接受事件时先 `record_event`/`mark_required_event_seen`，再检查 completion | 公共 preset 仍需防提前完成，且当前 accepted event 会先满足 required event | 0.99 |
| KTD3 | completion_payload_match | 保留自比较；删除 | 删除 | E2,E10,E12 | 无第二 payload 后没有比较对象 | 0.98 |
| KTD4 | REJECTED 程度由谁决定 | Reporter；Auditor；runtime | Auditor按 final-audit 模板的逐 Unit delivery matrix写 `delivery_status`，Reporter转录 | E3–E5 | Reporter缺少审计 authority；runtime不应解释业务证据；无模板会把判断留给临场发挥 | 0.97 |
| KTD5 | 状态自洽如何保证 | prompt；precheck；payload consistency | schema + same-payload consistency | E5,E12 | prompt 太软；本合同无外部判断，不值得新增 activation | 0.98 |
| KTD6 | Auditor/Reporter 是否挂 precheck | 都挂；只 Auditor；都不挂 | 都不挂 | E12,E13 | 003 runtime guard覆盖写越权；剩余规则均为确定性 payload 合同 | 0.96 |
| KTD7 | worktree 谁清理 | Reporter；runtime；二者兜底 | runtime 唯一 owner | E4,E6,E7,E14 | 双 owner 会竞态且 Reporter 无法报告后发生的结果 | 0.99 |
| KTD8 | Reporter 写权限 | 不设；`.ralph/**`；`docs/reports/**` | 仅 `docs/reports/**` | E13,E14 | 不设无法硬约束；`.ralph/**` 过宽且 Reporter 不应改上游证据 | 0.96 |
| KTD9 | 计划拆分 | 单大 Unit；两 Unit；按文件多 Unit | 两个严格串行 Unit | E3–E5,E8 | 一个 Unit 混合审计语义和终止迁移；按文件拆分不可观察 | 0.95 |

所有实施关键决策均不低于 0.85。
无待验证的低置信度假设。

## 4. BDD 行为规格

```gherkin
Feature: Parallel Forge 以单事件完成最终审计与报告

  Background:
    Given 001、002、003 的合同已完成
    And 全部 Unit 已结算、集成并通过 Tester 全量验证

  Scenario S1: Auditor 接受完整交付
    Given Auditor 读取到完整且相互一致的最终证据
    When Auditor 完成只读审计
    Then 恰好发布一条 forge.audit.done
    And verdict 为 ACCEPTED
    And delivery_status 为 COMPLETED
    And 不发布 forge.plan.blocked

  Scenario S2: Auditor 拒绝部分或全部交付
    Given 最终证据证明交付不能全部接受
    When Auditor 完成只读审计
    Then 恰好发布一条 forge.audit.done
    And verdict 为 REJECTED
    And delivery_status 为 PARTIAL 或 FAILED
    And ACCEPTED 与非 COMPLETED 的组合被拒绝

  Scenario S3: Auditor 缺少关键证据
    Given 审计所需 artifact 缺失或无法验证
    When Auditor 无法形成可信 verdict
    Then 恰好发布一条 forge.plan.blocked
    And 不发布 forge.audit.done

  Scenario S4: Reporter 用一条报告事件完成成功路径
    Given accepted forge.audit.done 为 ACCEPTED 和 COMPLETED
    When Reporter 写完并校验 manager report
    Then 恰好发布一条 forge.report.done
    And status 为 COMPLETED
    And final_audit 为 ACCEPTED
    And EventLoop 立即接受 completion
    And 不需要也不接受 Reporter 补发 LOOP_COMPLETE

  Scenario S5: Reporter 确定性收敛 blocked 或 failed 路径
    Given Reporter 收到 accepted forge.plan.blocked 或 work.failed
    When Reporter 写完 manager report
    Then forge.plan.blocked 映射为 BLOCKED 和 BLOCKED
    And work.failed 映射为 FAILED 和 FAILED
    And 错误映射被 payload consistency 拒绝

  Scenario S6: runtime 独占终态清理
    Given forge.report.done 已被接受
    When runner 进入 terminal handler
    Then runtime 幂等清理 supervisor slot worktree
    And Reporter 不执行 git worktree remove
    And 单个清理失败只留下诊断且不产生第二条业务事件
```

## 5. 验收与测试策略

本计划只保留五个高价值测试闭环，不复制 003 已覆盖的通用 readonly guard 测试。

| Test ID | Scenario | 测试入口 | 层级 | 核心断言 | 风险依据 |
|---|---|---|---|---|---|
| T1 | S1–S3 | 现有 preset/schema 结构化测试 | 单元 | Auditor topic 二选一、required fields、verdict/status 合法组合 | 防合同漂移 |
| T2 | S4 | `parallel_forge_declared_flow_runtime.yml` | 真实 EventLoop BDD | 15 次而非 16 次；`forge.report.done` 恰好一次并完成；LOOP_COMPLETE absent | 主成功路径 |
| T3 | S5 | `parallel_forge_declared_flow_failed_runtime.yml` | 真实 EventLoop BDD | failure 后无成功越界；单条 FAILED 或 BLOCKED report 完成 | 失败收敛 |
| T4 | S4–S5 | payload consistency 现有测试位置 | 单元 | 三种 source 的合法映射接受，代表性非法映射拒绝 | 防 Reporter 临场改写 |
| T5 | S6 | loop runner 现有 terminal cleanup 测试位置 | 集成 | completion promise 为业务 topic 时仍只调用一次 cleanup；NotFound 幂等 | 清理 ownership |

不新增浏览器 E2E、网络测试、property test、fuzz 或 mutation test。
最终使用现有全量 gate 覆盖其余回归。

## 6. 需求—测试追踪矩阵

| Requirement | Scenario | Test | Unit | Evidence |
|---|---|---|---|---|
| R1–R4,R17 | S1–S3 | T1 | Unit 1 | E3,E5,E13 |
| R5–R10 | S4–S5 | T2–T4 | Unit 2 | E1,E2,E8,E10–E12 |
| R11 | S4–S5 | T1–T4 + 003 回归 | Unit 2 | E13,E14 |
| R12–R14 | S6 | T5 | Unit 2 | E6,E7 |
| R15 | S4–S5 | T2,T3 | Unit 2 | E8,E9 |
| R16 | S1–S6 | 001–003 targeted regression + full gate | Unit 2 | E13,E15 |

## 7. 严格串行开发单元

```text
Unit 1：Auditor 固化最终交付结论
  ↓ Acceptance Red → Unit Red → Green → Refactor → Integration → Regression → Close
Unit 2：Reporter 单事件终止并移交 runtime 清理
```

### Unit 1：Auditor 固化最终交付结论

#### 1. Unit 目标

当 Tester 成功后，Auditor 用一条事件明确给出可交付程度，不再把
`REJECTED → PARTIAL/FAILED` 的关键判断留给 Reporter。

#### 2. 对应合同

- Requirements：R1–R4、R17。
- Scenarios：S1–S3。
- Decisions：KTD4、KTD6、KTD8。
- Evidence：E3、E5、E12、E13。

#### 3. 外部可观察结果

- accepted delivery：`forge.audit.done{verdict: ACCEPTED, delivery_status: COMPLETED}`。
- rejected delivery：`forge.audit.done{verdict: REJECTED, delivery_status: PARTIAL|FAILED}`。
- blocked audit：只发布 `forge.plan.blocked`。
- Auditor 没有修代码、改测试或修改 Tester evidence 的出口。

#### 4. 输入、输出与不变量

`forge.audit.done` 固定 required fields：

| 字段 | 规则 |
|---|---|
| `verdict` | `ACCEPTED | REJECTED` |
| `delivery_status` | `COMPLETED | PARTIAL | FAILED` |
| `audit_report_path` | `.ralph/forge/<plan_key>/final-audit.md` |
| `audit_report_digest` | 该文件 SHA-256，64 位小写十六进制 |
| `verification_report_path` | 原样引用 Tester trigger |
| `verification_report_digest` | 原样引用 Tester accepted evidence |
| `final_head` | Auditor 审计的 candidate HEAD |
| `plan_key` | 原样继承 |

`final-audit.template.md` 固定包含逐 Unit delivery matrix：

| Unit 结论 | 必须同时满足 | 对总体状态的贡献 |
|---|---|---|
| `DELIVERABLE` | Unit commit 已进入 `final_head`；对应 Scenario/验收通过；无未解决 must-fix finding；证据路径与 digest 可复核 | 计入 `deliverable_units` |
| `NOT_DELIVERABLE` | 上述任一条件不满足 | 计入 `non_deliverable_units` 并写精确原因 |
| `UNVERIFIABLE` | 证据缺失、digest 不符、HEAD 无法关联或环境阻止验证 | 不发布 audit.done，改走 plan.blocked |

总体判定算法固定为：

```text
若存在 UNVERIFIABLE
  → forge.plan.blocked
否则若 non_deliverable_units = 0 且全部计划内 Scenario/全量门禁通过
  → ACCEPTED + COMPLETED
否则若 deliverable_units > 0 且每个 DELIVERABLE Unit 可独立保留、无全局阻断回归
  → REJECTED + PARTIAL
否则
  → REJECTED + FAILED
```

“全局阻断回归”包括：最终树无法 build、required full gate 失败、线性历史不可追踪、
交付子集无法从当前 `final_head` 安全使用。
Auditor 必须在模板中列出计数和证据，不能只写最终枚举。

不变量：

- `ACCEPTED + COMPLETED` 合法；
- `REJECTED + PARTIAL` 合法；
- `REJECTED + FAILED` 合法；
- 其他 verdict/status 组合全部拒绝；
- `BLOCKED` 只属于 `forge.plan.blocked`；
- 单 activation 只接受一个业务 event；
- Auditor 的 `allowed_write_paths` 精确为
  `.ralph/forge/{plan_key}/final-audit.md` 和
  `.ralph/forge/{plan_key}/blocks/auditor-blocked.md`；
- 不配置 precheck。

#### 5. 修改位置

| 位置 | 变更 | 明确边界 |
|---|---|---|
| `presets/en/parallel-forge.yml` | 更新 Auditor fields、状态表、003 readonly 配置 | 不改 Tester 或 Reporter 终态 |
| `presets/schemas/parallel-forge.yml` | 扩展 `forge.audit.done` required fields 与 field docs | 不在本 Unit 删除 LOOP_COMPLETE |
| 新增 `presets/templates/parallel-forge/final-audit.template.md` | 固定逐 Unit delivery matrix、总体判定算法和 evidence 表 | 不把状态表复制进 prompt |
| `presets/templates/parallel-forge/README.md` 与现有 template registry/materialization 测试 | 注册并解释新模板 | 不做整文件 byte-equality |
| `crates/ralph-cli/src/presets.rs` 现有 Parallel Forge 结构化测试区 | 增加/修改 Auditor 结构化合同断言 | 不断言 prompt 精确文案 |

#### 6. Acceptance Red

先修改 T1，使它要求 `delivery_status`、digest、Tester evidence 和 `final_head`。
在生产配置未改时，Red 必须因 required fields 缺失或非法组合未被拒绝而失败。
YAML 解析错误、fixture 缺失或错误命令不算有效 Red。

#### 7. Red → Green → Refactor

```text
T1 required-fields Red
→ 最小扩展 schema 与 Auditor emit contract
→ T1 Green
→ final-audit template materialization/结构 Red
→ 注册最小模板并让 Auditor引用
→ template test Green
→ T1 verdict/status consistency Red
→ 最小增加 same-payload rules
→ T1 Green
→ 去除 Auditor instructions 中 BLOCKED verdict 与 Reporter 决策措辞
→ targeted preset/schema regression
```

#### 8. 最小实现范围

- 只定义 Auditor 的最终审计事实。
- 复用 003 strict readonly guard，不修改其 runtime。
- 不挂 precheck。
- 不触碰 flow completion。
- 不提前修改 Reporter。

#### 9. 集成与回归

```bash
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- presets
```

三个命令全部通过才可关闭 Unit。

#### 10. 完成标准

- T1 真实 Red 后 Green。
- schema、preset 和结构化测试一致。
- final-audit 模板可在 binary-only 安装中 materialize，且包含可解析的逐 Unit
  delivery matrix 和总体计数；测试不锁定整份文案。
- Auditor 仍只有两个互斥 output topics。
- Auditor 没有 precheck。
- 003 readonly 配置未被放宽。
- 可形成独立提交。

#### 11. 停止条件

- post-003 没有可复用的 strict readonly guard；
- Tester 的 post-003 event 不提供可引用的 evidence digest/final HEAD；
- payload consistency 无法表达固定 enum 组合；
- Auditor 实际存在第三个必须保留的终态 topic。

触发后记录新证据、修订 KTD 和 Unit 2，禁止 Reporter 临时补判断。

### Unit 2：Reporter 单事件终止并移交 runtime 清理

#### 1. Unit 目标

Reporter 写完报告后只发布一条 `forge.report.done`，该事件立即完成 loop；
runtime 随后统一清理 worktree。

#### 2. 对应合同

- Requirements：R5–R16。
- Scenarios：S4–S6。
- Decisions：KTD1–KTD3、KTD5–KTD8。
- Evidence：E1、E2、E4、E6–E15。

#### 3. 外部可观察结果

- 成功链少一次 Reporter activation。
- 事件历史中没有 Parallel Forge Reporter 发出的 `LOOP_COMPLETE`。
- manager report 与 `forge.report.done` 的状态、digest 和来源一致。
- completion 后 runtime 清理 slot worktree。
- cleanup warning 不会导致 Reporter 再次激活或改写报告。

#### 4. Reporter 确定性映射

| `source_topic` | trigger 条件 | `status` | `final_audit` | `source_verdict` |
|---|---|---|---|---|
| `forge.audit.done` | `ACCEPTED + COMPLETED` | `COMPLETED` | `ACCEPTED` | `ACCEPTED` |
| `forge.audit.done` | `REJECTED + PARTIAL` | `PARTIAL` | `REJECTED` | `REJECTED` |
| `forge.audit.done` | `REJECTED + FAILED` | `FAILED` | `REJECTED` | `REJECTED` |
| `forge.plan.blocked` | accepted block | `BLOCKED` | `BLOCKED` | `BLOCKED` |
| `work.failed` | accepted terminal failure | `FAILED` | `FAILED` | `FAILED` |

`source_evidence_path`：

- audit path：来自 `audit_report_path`；
- blocked/failed path：来自 `context_artifact_path`；
- audit digest 和 `final_head` 仍写入 manager report 的证据章节，但不强制进入所有来源共用的终态 payload，因为当前 `forge.plan.blocked`/`work.failed` 合同不保证提供这两个字段；
- `report_digest` 由 Reporter 在 emit 前对刚写完的 manager report 计算。

#### 5. 修改位置

| 位置 | 变更 | 明确边界 |
|---|---|---|
| `presets/en/parallel-forge.yml` | 折叠 flow；切换 completion promise；保留 required event；删除 path match、Reporter LOOP_COMPLETE 和 cleanup；配置 Reporter readonly path；添加 consistency rules | 不改变其他 hat 拓扑 |
| `presets/schemas/parallel-forge.yml` | 扩展 report required fields；删除 Parallel Forge `LOOP_COMPLETE` schema | 不影响其他 preset schema |
| `manager-report.template.md` | 固定状态来源、证据字段和 cleanup ownership 文案 | 不宣称 cleanup 已完成 |
| 两个 declared-flow scenario | 各删除一次 Reporter activation 与 LOOP_COMPLETE；补齐新 payload | 不新增第三个大型 scenario |
| `crates/ralph-core/tests/scenarios.rs` | 更新现有测试注释 | 不改 runner 选择 |
| `crates/ralph-cli/src/presets.rs` | 用“report.done 即唯一 completion”替换 path-match 静态断言 | 不保留过时双事件测试 |
| loop runner 现有测试模块 | T5：业务 completion topic 仍触发一次 cleanup | 不改 cleanup production code，除非 Red 证明接线缺失 |
| agent/operator docs | 同步单事件动作和审查规则 | 不写计划编号进注入 skill |
| `CLAUDE.md`、`AGENTS.md`、`.cursor/rules/multi-hat-isolation.mdc` | 更新 builtin preset 描述；两主文件保持完全一致 | 不改 preset 名称 |

#### 6. Acceptance Red

按顺序建立：

1. T2 将成功 fixture 改成第 15 次直接发 `forge.report.done` 并要求 completion honored；
   当前 completion promise 仍是 LOOP_COMPLETE，因此正确 Red 是 loop 未完成或迭代耗尽。
2. T3 将失败 fixture 的最后一次也改为单 `forge.report.done`；
   正确 Red 同上。
3. T4 添加一个代表性非法映射：
   `source_topic=forge.plan.blocked,status=COMPLETED,final_audit=ACCEPTED`；
   当前无规则时事件错误通过。
4. T5 配置非 LOOP_COMPLETE 的 completion promise；
   若现有 runner 已正确调用 cleanup，可作为 characterization Green；
   只有它失败才允许修改 production runner。

#### 7. Red → Green → Refactor

```text
T2 success single-terminal Red
→ 折叠 flow、切换 completion promise、删除 Reporter LOOP_COMPLETE
→ T2 Green
→ T3 failure single-terminal Red/Green
→ T4 invalid mapping Red
→ 增加最小 payload consistency rules
→ T4 Green
→ T5 terminal cleanup characterization/Green
→ 删除 Reporter cleanup 与 completion_payload_match
→ 更新模板、schema、skills、operator docs
→ targeted regression
→ full regression
```

#### 8. 最小实现范围

- `event_loop.completion_promise: forge.report.done`。
- `terminal_topics` 只把 `forge.report.done` 作为 Parallel Forge terminal。
- flow 的 `report` 直接是 terminal step；删除 `plan_end`。
- `required_events` 继续只含 `forge.report.done`。
- 删除 `completion_payload_match` 及其 Parallel Forge 专用测试。
- Reporter `publishes`、`exempt_topics`、`terminal_events` 只含 `forge.report.done`。
- Reporter `allowed_write_paths` 只含 `docs/reports/**`。
- 删除 Reporter 的 `git worktree remove`、cleanup owner 和双事件 resume 说明。
- 不新增 runtime 状态表、不新增 retry 状态、不新增 precheck。

#### 9. 集成验证

```bash
cargo nextest run -p ralph-core -- test_parallel_forge_declared_flow_runtime
cargo nextest run -p ralph-core -- test_parallel_forge_declared_flow_failed_runtime
cargo nextest run -p ralph-core -- payload_consistency
cargo nextest run -p ralph-cli --bin ralph -- terminal_cleanup
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- presets
scripts/check-cli-doc-drift.sh
```

若实际 targeted substring 不匹配任何测试，必须从现有测试符号重新取准确 substring，
记录 Evidence 后更新本计划命令，禁止以“0 tests”作为 Green。

#### 10. 回归范围

- 001 的 wave success/failure/correction BDD；
- 002 的 reuse/resume/idempotency tests；
- 003 的 strict readonly scope、precheck 与 payload tests；
- 两个 Parallel Forge declared-flow BDD；
- public preset required-events lint；
- event policy terminal/duplicate/business-after-completion tests；
- supervisor terminal cleanup tests；
- preset schema parity、embedded preset 和 artifact materialization；
- agent skill doc drift；
- 全 workspace nextest/doctest。

#### 11. 完成标准

- T2–T5 全绿。
- 成功和失败 BDD 中 `forge.report.done` 各恰好一次。
- Parallel Forge Reporter 发布的 LOOP_COMPLETE 明确 absent。
- 非法 source/status/final_audit 组合被 deterministic gate 拒绝。
- Reporter 无 worktree cleanup 命令。
- runtime cleanup 测试证明非默认 completion topic 同样触发。
- manager report 不虚构尚未发生的 cleanup 结果。
- schema/preset/skills/operator docs/CLAUDE/AGENTS 同步。
- `CLAUDE.md` 与 `AGENTS.md` 字节一致。
- 全量门禁通过，可独立提交。

#### 12. 停止条件

- current completion event 在 required-events accounting 前被检查，导致自 required event 无法满足；
- flow terminal step 不允许业务 topic 作为 completion promise；
- accepted completion 后 terminal cleanup 没有执行；
- post-003 Reporter strict readonly 无法允许 `docs/reports/**`；
- 当前 `forge.plan.blocked` 或 `work.failed` 缺少 `context_artifact_path`，导致 Reporter 无法引用来源证据；
- 删除 LOOP_COMPLETE 后其他 Parallel Forge 下游仍直接依赖该 topic；
- 实际回归范围扩展到其他 preset。

停止后必须：

```text
记录新证据
→ 更新受影响范围
→ 重新比较候选方案
→ 重新计算置信度
→ 修订 Unit 2
```

不得通过恢复双事件、放宽 schema 或让 Reporter猜测来绕过。

## 8. Unit 串行依赖图

```text
Unit 1：Auditor 确定 verdict + delivery_status
  ↓ Reporter 必须消费已验证的明确结论
Unit 2：Reporter 纯转录 + forge.report.done 直接终止
```

Unit 2 不能先于 Unit 1。
否则 Reporter 仍需在 `REJECTED` 下自行选择 PARTIAL/FAILED，核心决策空间没有被消除。
Unit 1 不提前改变 completion；Unit 2 不回头改变审计语义。

## 9. 执行命令清单

| 时机 | 命令 | 目的 | 进入下一步 |
|---|---|---|---|
| Unit 1 | `cargo nextest run -p ralph-cli --bin ralph -- presets` | Auditor schema/preset 合同 | 必须通过 |
| Unit 1/2 | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | CLI preset lint | 必须通过 |
| Unit 1/2 | `cargo nextest run -p ralph-core -- preset_lint` | core lint/schema parity | 必须通过 |
| Unit 2 | `cargo nextest run -p ralph-core -- test_parallel_forge_declared_flow_runtime` | 成功单终态 BDD | 必须通过且非 0 tests |
| Unit 2 | `cargo nextest run -p ralph-core -- test_parallel_forge_declared_flow_failed_runtime` | 失败单终态 BDD | 必须通过且非 0 tests |
| Unit 2 | `cargo nextest run -p ralph-core -- payload_consistency` | 确定性映射 | 必须通过 |
| Unit 2 | `cargo nextest run -p ralph-cli --bin ralph -- terminal_cleanup` | runtime 清理 ownership | 必须通过且非 0 tests |
| Unit 2 | `scripts/check-cli-doc-drift.sh` | agent CLI/skill drift | 必须通过 |
| 最终 | `cargo fmt --check` | 格式 | 必须通过 |
| 最终 | `cargo clippy` | lint | 必须通过 |
| 最终 | `cargo build` | build/typecheck | 必须通过 |
| 最终 | `./scripts/run-tests.sh` | 项目规定的全量 nextest + doctest | 必须通过 |

禁止裸跑 `cargo test -p ralph-cli`。
若全量仅出现确认的竞态/时序 flake，才允许
`RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底；
serial 仍失败即是真失败。

## 10. 最终质量门禁

- S1–S6 全部可追踪并通过。
- Auditor/Reporter 均保持单 activation 单业务事件。
- Auditor 无 precheck，Reporter 无 precheck。
- Auditor 决定交付程度，Reporter 不含二义性判断。
- `forge.report.done` 是唯一 completion promise。
- `required_events` 仍阻止提前完成。
- Parallel Forge preset/schema/flow 中不再保留 Reporter 的 LOOP_COMPLETE 路径。
- completion payload match 的 002 旧补偿合同已删除。
- Reporter 不执行 cleanup。
- runtime 对 accepted completion 统一清理且保持幂等。
- 001–003 回归未破坏。
- preset、schema、BDD、skills、operator docs 和项目文档同步。
- 没有 prompt 文案等值测试、跳过测试、`.only`、削弱断言或无解释 snapshot 更新。
- 全量测试、lint、build、doc drift 通过。
- 所有关键决策置信度仍不低于 0.85。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 两个可观察纵向 Unit |
| Executor 是否仍需做关键设计决策 | 否 | topic、字段、映射、owner、测试均已固定 |
| 所有文件和接口是否有代码库证据 | 是 | E1–E15 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | KTD1–KTD9 |
| 是否存在未处理的低置信度假设 | 否 | 无 |
| 每个 Unit 是否只有一个可观察行为 | 是 | 审计结论；单事件终止 |
| 每个 Unit 是否可以独立验证 | 是 | T1；T2–T5 |
| 每个 Unit 是否有真实 Red | 是 | 各 Unit 已定义正确失败原因 |
| 每个 Unit 是否包含回归范围 | 是 | targeted + full gate |
| 是否存在未来 Unit 依赖 | 否 | Unit 2 只依赖已完成 Unit 1 |
| 是否存在泛化任务描述 | 否 | 修改位置、字段和命令明确 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §6 |
| 所有关键决策是否有 Evidence | 是 | §3 |
| 计划是否可以严格串行执行 | 是 | Unit 1 → Unit 2 |

## Definition of Done

当且仅当以下结果同时成立，本计划完成：

1. Auditor 用一条事件确定最终交付程度；
2. Reporter 用一条 `forge.report.done` 完成 loop；
3. Parallel Forge 不再需要 Reporter 补发 `LOOP_COMPLETE`；
4. Reporter 不再清理 worktree；
5. runtime 在 accepted completion 后统一、幂等清理；
6. success/failure 真实 EventLoop BDD 均证明单链成立；
7. 001–003 和全 workspace 回归全部通过。
