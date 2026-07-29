---
title: "修复 Parallel Forge 终态一致性"
date: 2026-07-29
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
target_repository: ralph-orchestrator
baseline_commit: 445d76103a4e485842b28d95e17e3fc6725355f5
---

# 修复 Parallel Forge 终态一致性

## 0. 计划状态

- 状态：`READY`
- 基线：`445d76103a4e485842b28d95e17e3fc6725355f5`
- 调查范围：parallel-forge preset/schema、事件完成门禁、task state projection、真实运行诊断报告及原始事件、BDD/runtime 测试、diagnosis skill、相关 Git 历史。
- 已执行的调查命令：`git status --short`、`git log`、`git show`、`rg`、`sed`、`jq`、`cmp`。
- 已执行的验证：静态解析调用链、配置和测试位置；复核真实运行的 `forge.audit.done`、`forge.report.done`、`LOOP_COMPLETE` 与 task 关闭时序。
- 尚未执行的验证：本计划不运行测试或构建；第 9 节命令全部留给实施阶段。
- 阻塞项：无。全部实施关键决策置信度均不低于 `0.85`。
- 外部研究：未执行。现有源码、真实运行事件、测试与 Git 历史已覆盖关键决策，不需要外部技术选型。

## Goal Capsule

在不改变 parallel-forge 的 13-hat wave 主链、不增加 fixer/re-audit hat、不把 `LOOP_COMPLETE` 改成“成功”事件的前提下，消除两类状态漂移：

1. Executor 已完成 Unit，但 task 仍长期保持 `open`，导致 resume 接手时误判未完成工作。
2. `forge.report.done` 已报告 `FAILED/REJECTED` 后，恢复过程覆盖了可变报告产物并另发 `LOOP_COMPLETE`，诊断却把整次运行描述成“零拒收、完整成功”。

成功标准是：accepted `exec.unit.done` 原子关闭对应 task；`LOOP_COMPLETE.report_path` 必须与最近已接受的 `forge.report.done.report_path` 相同；诊断按事件时序区分“首轮成功”和“失败终态后的恢复”。

## 1. 功能目标

### 业务目标

让 operator 能可信地区分“工作流结束”“工作流成功”和“失败终态后被 resume 修复”，同时避免已完成 Unit 因 task 未关闭而被恢复流程重复接手。

### 用户或调用方

- 运行 `builtin:parallel-forge` 的 operator。
- 依赖 task 状态选择 ready work 的 forge-dispatcher、executor 和 supervisor。
- 读取 `forge.report.done`、`LOOP_COMPLETE`、报告产物的诊断工具与维护者。

### 当前行为

- parallel-forge 的 `exec.unit.done` schema 不要求 `task_id`，EventLoop state projection 只在 `forge.plan.ready` 创建 task，未在 Unit 完成事件上关闭 task。
- Reporter 正确地把审计 `REJECTED` 映射为 `forge.report.done(status=FAILED, final_audit=REJECTED)`，随后仍可发 `LOOP_COMPLETE`，因为后者表示工作流终止。
- `LOOP_COMPLETE` 只要求存在 `report_path`；preset instruction 要求与 `forge.report.done` 相同，但 runtime 不比较两个事件的字段。
- diagnosis skill 未强制比较 immutable event chronology 与后续可变 artifact，导致首轮失败可被后续覆盖文件描述成“全程成功”。

### 目标行为与行为差异

- accepted `exec.unit.done` 必须携带对应 live task 的 `task_id`，由现有 state projector 关闭该 task；从“instruction 提醒手工关闭”变为“事件接受后的配置驱动投影”。
- `LOOP_COMPLETE` 仍允许对应 `COMPLETED`、`FAILED` 或 `BLOCKED` 报告，但其 `report_path` 必须与最近已接受的 `forge.report.done.report_path` 一致。
- resume 若在 `forge.report.done` 后中断，只能补发匹配的 `LOOP_COMPLETE`；不得重写既有 audit/report 事实来制造新的成功终态。
- diagnosis 必须先按事件确定首轮 verdict/status，再解释后续 artifact/commit；存在失败终态后修复时，输出“失败终态后恢复”，不得输出“零拒收”。

### 本次范围

- 扩展 parallel-forge 的 task 投影配置和 `exec.unit.done` 结构化契约。
- 增加通用、默认关闭、字段级 completion predecessor 配对门禁，并只在 parallel-forge 启用 `report_path` 比较。
- 更新 parallel-forge reporter/executor instruction、注入式 agent guide、preset operator guide。
- 更新 diagnosis skill 及其 `.agents` 镜像，加入终态时序一致性规则。
- 增加配置、state projector、EventLoop、preset lint/parity、BDD/runtime、skill contract 测试。

### 非目标

- 不增加 fixer、re-auditor 或新的 hat；hat 总数保持 13。
- 不改变 wave 调度、worktree、fan-in merge 或 supervisor 数据模型。
- 不把 `LOOP_COMPLETE` 限定为成功，也不禁止 `FAILED/BLOCKED` 正常终止。
- 不实现自动修复循环、报告版本化、artifact 不可变存储或全局 event ledger 重构。
- 不修改其他 builtin preset 的终态拓扑。
- 不兼容缺少新结构化字段的旧 parallel-forge preset；仓库明确不要求向后兼容。

### 输入、输出与状态变化

- 输入：`exec.unit.done` payload、`forge.report.done` payload、`LOOP_COMPLETE` payload、运行事件与报告产物。
- 输出：关闭后的 task 状态；接受或拒绝 completion event；带有首轮终态与恢复分类的 diagnosis。
- 状态变化：accepted `exec.unit.done` 将对应 task 从 `open` 变为 `closed`；accepted `forge.report.done` 记录 completion 配对基准；`LOOP_COMPLETE` 不修改业务 verdict。
- 副作用：task 依赖可在关闭后变为 ready；被拒绝的 mismatch completion 产生现有 correction 路径，不写入成功终态。

### 错误语义

- `exec.unit.done` 缺少 `task_id`：由 event schema 拒绝，task 保持原状态。
- `task_id` 不存在或不可关闭：现有 state projection 失败语义生效，事件不产生“已关闭”假象。
- predecessor payload 或 completion payload 不是 JSON object、字段缺失或值不相等：拒绝 `LOOP_COMPLETE`，返回稳定的字段级 mismatch reason，并使用现有 completion correction 机制要求同一 reporter 修正。
- `forge.report.done(status=FAILED|BLOCKED)` 与匹配 `LOOP_COMPLETE`：合法终止，不视为门禁错误。

### 兼容、性能、安全与约束

- 兼容性：新增 completion 配对配置为 `Option`，未配置 preset 保持现有行为；parallel-forge 明确收紧。
- 性能：每次仅保存一个目标 predecessor payload，并在 completion 时比较少量字段；不得扫描完整 JSONL。
- 安全/权限：不增加命令或权限；继续由 event policy、schema、origin guard 限制 publisher。
- 约束：preset/schema 改动必须按仓库清单同步；所有 Rust 测试走 `cargo nextest run`；Python 测试使用 `.venv`；注入 guide 只能描述通用 agent 行为。

### 已确认事实、假设与决策

- 已确认事实见 E1-E13。
- 已确认假设：本次修复只要求恢复事实可追溯，不要求自动重跑审计。用户已明确认可。
- 待验证假设：无实施阻塞假设。
- Product Contract preservation：本计划直接由本次会话建立，无上游 requirements-only 文档。

## Product Contract

### Actors

- `A1 Operator`：启动、resume 并读取最终报告。
- `A2 Executor`：完成一个 Unit 并发布 `exec.unit.done`。
- `A3 Reporter`：根据 audit/block 生成唯一经理报告并发布终态事件。
- `A4 Diagnosis consumer`：重建运行时序并分类结果。

### Requirements

- `R1`：accepted `exec.unit.done` 必须关闭 payload 指向的 live task，且关闭失败不得伪装为成功。
- `R2`：parallel-forge 的 `LOOP_COMPLETE.report_path` 必须等于最近 accepted `forge.report.done.report_path`。
- `R3`：`LOOP_COMPLETE` 继续表示“工作流已终止”，允许 FAILED/BLOCKED，不承担成功 verdict。
- `R4`：diagnosis 必须保留首轮 audit/report 事件事实，并把后续无对应成功事件的 artifact 修复分类为“失败终态后恢复”。
- `R5`：修复保持 13-hat 主链和现有 wave/supervisor 机制，不引入自动 fix/re-audit loop。

### Key Technical Decisions

- `KTD-1`：使用现有 `StateProjectionAction::CloseTask` 关闭 task，而不是让 Executor 再执行一条非原子 CLI 命令。`Governs R1`。
- `KTD-2`：增加默认关闭的通用 completion payload match 配置，只比较声明字段。`Governs R2-R3`。
- `KTD-3`：事件 chronology 是终态事实源，artifact 和 Git commit 用于解释后续恢复，不能反向覆盖先前 verdict。`Governs R4`。
- `KTD-4`：保持最小三 Unit 修复，不引入 fixer/re-audit hat。`(session-settled: user-approved — chosen over 自动修复复审环: 当前 wave 主链效果可用，问题可由 task 投影、终态配对和诊断规则局部修复)`。`Governs R5`。

## 2. 代码库现状与证据

### 2.1 当前实现入口

- 外部入口：`presets/en/parallel-forge.yml` 定义 13-hat 拓扑、`event_loop`、publisher/subscriber 和 instruction。
- 配置链：preset YAML → `RalphConfig` → `EventLoopConfig` → EventLoop。
- 事件链：agent JSONL → parser/origin/policy/schema → StateMachine/EventLoop → state projector/task store → hats。
- task 数据边界：`forge.plan.ready` 通过 state projection 创建 batch；`StateProjectionAction::CloseTask` 已支持按 payload 字段关闭 task。
- completion 边界：`LoopState` 记录 required topic/verdict；`check_completion_event` 检查 required events 和 verdict，但不保留任意 predecessor payload。
- diagnosis 边界：`skills/ralph-run-diagnosis/references/*.md` 定义证据分层、终态判断和报告模板；`.agents/skills/ralph-run-diagnosis/` 是需同步的分发镜像。
- 现有测试：state projector 单元测试、EventLoop termination/runtime 测试、真实 EventLoop scenario、CLI builtin preset lint/parity、Python skill contract。
- 构建验证：`cargo nextest run`、`./scripts/run-tests.sh`、`cargo fmt --check`、`cargo clippy`、`.venv/bin/python -m pytest`。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
| --- | --- | --- | --- | --- |
| E1 | `presets/en/parallel-forge.yml` | 声明 13 个 hats；主链为 inspect→plan→guardian/worktree→dispatch/executor/review/integrate→verify/test/audit/report | 固定非目标：不增加 hat、不重排主链 | 高 |
| E2 | `presets/en/parallel-forge.yml` reporter instruction | `REJECTED` 映射 `FAILED`，随后仍发布 `LOOP_COMPLETE`；instruction 要求两个事件使用相同 `report_path` | 保留终止语义，只把软约束升级为 runtime gate | 高 |
| E3 | `presets/schemas/parallel-forge.yml` | `exec.unit.ready` 有 `task_id/task_key`，`exec.unit.done` 没有；`forge.report.done` 和 `LOOP_COMPLETE` 都要求 `report_path` | U1 增加 done 事件关联字段；U2 可比较现有共同字段 | 高 |
| E4 | `crates/ralph-core/src/config/state_projection.rs` | 已有 typed `CloseTask { task_id }` action | U1 复用现有机制，不新增 task API | 高 |
| E5 | `crates/ralph-core/src/state_projector/task.rs` | projector 能从 payload 取 task id 并调用 task store close | 关闭逻辑应配置化，不放进 preset instruction 手工编排 | 高 |
| E6 | `crates/ralph-core/src/state_projector/tests.rs` | 已有 close-task happy path 测试模式 | U1 在同一测试层增加缺失/错误 ID 与不变量覆盖 | 高 |
| E7 | `crates/ralph-core/src/config/loop_config.rs`、`crates/ralph-core/src/event_loop/loop_state.rs` | required events 只记录 topic 是否出现；verdict gate 记录最近 verdict payload；无 sibling payload match | U2 需要窄的 opt-in config 与 LoopState 快照字段 | 高 |
| E8 | `crates/ralph-core/src/event_loop/tests/termination.rs` | completion 的 required-event/verdict/correction 行为已有集中测试入口 | U2 在此层测试真正 acceptance/rejection，不另造平行 runner | 高 |
| E9 | `crates/ralph-core/tests/scenarios/parallel_forge_task_dispatch_runtime.yml`、`crates/ralph-core/tests/scenarios.rs` | parallel-forge 已有真实 EventLoop BDD 场景入口 | 保留并扩展结构化 workflow 回归；禁止 source-only 文案测试 | 高 |
| E10 | `docs/report/2026-07-29-parallel-forge-primary-20260729-020808-diagnosis.md` 与独立 `jq` 复核 | 首次 audit=REJECTED、report=FAILED；后续 artifact 被覆盖为 ACCEPTED；最终只有一个 LOOP_COMPLETE，且没有后续 accepted audit/report 事件 | U2 防止终态路径漂移；U3 强制 chronology 分类 | 高 |
| E11 | Git commit `78aca63a` | typed state projection 用于 planner 事件创建 task，是仓库既有配置驱动模式 | KTD-1 与现有架构一致 | 高 |
| E12 | `skills/ralph-run-diagnosis/references/log-reconciliation.md`、`report-template.md`、`verification-pipeline.md` | 有证据分层和 terminal 检查，但无“失败事件后 artifact 覆盖”明确规则 | U3 修改已确认的 skill 入口 | 高 |
| E13 | `crates/ralph-core/data/ralph-tools-tasks.md`、`ralph-tools-emit.md` 与 `skills/ralph-preset-common/references/` | agent/preset author guide 会受 event/state-projection 行为变化影响 | U1/U2 必须同步通用 guide，避免 prompt 契约漂移 | 高 |

### 2.3 受影响范围

- 生产模块：`crates/ralph-core/src/config/loop_config.rs`、`crates/ralph-core/src/config/mod.rs`、`crates/ralph-core/src/event_loop/loop_state.rs`、`crates/ralph-core/src/event_loop/mod.rs`、`crates/ralph-core/src/state/snapshot.rs`、`crates/ralph-core/src/state/commit.rs`。
- preset/schema：`presets/en/parallel-forge.yml`、`presets/schemas/parallel-forge.yml`。
- Rust 测试：`crates/ralph-core/src/state_projector/tests.rs`、`crates/ralph-core/src/event_loop/tests/termination.rs`、`crates/ralph-core/tests/scenarios/parallel_forge_task_dispatch_runtime.yml`、`crates/ralph-core/tests/scenarios.rs`、`crates/ralph-cli/src/presets.rs`。
- 注入 guide：`crates/ralph-core/data/ralph-tools-tasks.md`、`crates/ralph-core/data/ralph-tools-emit.md`。
- operator guide：`skills/ralph-preset-common/references/agent-native-model.md`、`author-checklist.md`、`patterns.md`；若 lint finding 未变化，`finding-rubric.md` 只做适用性复核，不修改。
- diagnosis：`skills/ralph-run-diagnosis/references/log-reconciliation.md`、`report-template.md`、`verification-pipeline.md` 及 `.agents/skills/ralph-run-diagnosis/` 对应镜像；`skills/tests/test_execution_model_contract.py`。
- 不影响 API/UI/数据库迁移/外部服务/zsh completion/manifest/index：没有新增、删除或重命名 preset/命令。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
| --- | --- | --- | --- | --- | --- | --- |
| D1 | Unit 完成如何关闭 task | Executor 手工 `task close`；supervisor 隐式关闭；state projection | 在 `exec.unit.done` 上配置 `CloseTask`，payload 必须有 `task_id` | E3-E6、E11 | 手工命令与 done emit 非原子；supervisor 隐式行为无法由 preset 契约审计 | 0.97 |
| D2 | 如何约束两个终态事件路径一致 | 仅 instruction；专用 parallel-forge 分支；通用 opt-in 字段比较 | 新增默认关闭的通用 `completion_payload_match`，parallel-forge 配置 topic=`forge.report.done`、fields=`report_path` | E2、E3、E7、E8 | instruction 已被真实运行突破；专用分支增加 preset 耦合；全 payload 相等会错误约束 status 等不同字段 | 0.90 |
| D3 | mismatch 如何处理 | 接受并告警；改写 path；拒绝并 correction | 拒绝 completion，保持 predecessor 事实并走现有 correction | E7、E8、E10 | 接受无法消除歧义；改写 agent payload 隐藏错误且破坏事件真实性 | 0.94 |
| D4 | FAILED/BLOCKED 后能否 LOOP_COMPLETE | 禁止；改名失败完成事件；继续允许 | 继续允许，completion 只表达 terminated | E2、E10、用户确认 | 禁止会把正确失败报告变成无限循环；改名扩大所有 preset/API 范围 | 0.98 |
| D5 | diagnosis 的事实优先级 | 最新 artifact；Git 最终状态；accepted event chronology | 事件确定首轮终态，artifact/commit 只解释后续恢复 | E10、E12 | artifact 可覆盖；Git 不能证明 audit/report 事件已发布 | 0.96 |
| D6 | 是否引入 fix/re-audit loop | 新 hats；复用现有 hats 自环；不引入 | 不引入，只修复状态和诊断一致性 | E1、用户确认 | 超出最小范围，改变当前有效 wave 主链 | 0.98 |
| D7 | replay/resume 如何保存配对基准 | 扫描完整 JSONL；只在内存；纳入 LoopState snapshot/replay | 与现有 LoopState 持久化模式一致，保存最近匹配 predecessor payload | E7、E10 | 全量扫描成本和耦合更高；仅内存会在 resume 丢失门禁 | 0.89 |

低于 `0.85` 的决策：无。

## Planning Contract

### 调用链变化

1. `exec.unit.done` 通过 schema 后，EventLoop 执行配置的 `CloseTask` 投影；task 成功关闭后，依赖调度读取到真实 closed 状态。
2. accepted `forge.report.done` 更新 LoopState 中配置 topic 的最近 payload，并随 snapshot/replay 恢复。
3. `LOOP_COMPLETE` 进入 completion 检查时，先完成既有 required/verdict 检查，再比较声明字段；mismatch 复用 correction，不改变报告 verdict。
4. diagnosis 先建立 accepted event terminal timeline，再读取 mutable artifact 和 Git，输出“首轮终态 + 后续恢复”双层结论。

### 配置形状

方向性配置（非生产代码）：

```yaml
event_loop:
  completion_payload_match:
    topic: forge.report.done
    fields: [report_path]
```

约束：

- `topic` 非空，`fields` 非空且字段名唯一。
- 未配置时完全保持现有 completion 行为。
- 只比较 JSON object 顶层字段的值；本 Unit 不支持 JSONPath、转换或多 predecessor。
- schema 继续负责字段存在性；runtime gate 负责跨事件相等性。

## 4. BDD 行为规格

```gherkin
Feature: Parallel Forge Unit 与终态事实一致

  Background:
    Given parallel-forge preset 已通过 strict lint

  Scenario S1: accepted Unit 完成事件关闭对应 task
    Given task_id 指向一个 open task
    When executor 发布满足 schema 的 exec.unit.done
    Then 事件被接受
    And 对应 task 变为 closed
    And 其他 task 的状态不变

  Scenario S2: Unit 完成事件缺少 task_id 时不关闭任何 task
    Given 存在一个 open task
    When executor 发布缺少 task_id 的 exec.unit.done
    Then schema 拒绝该事件
    And 原 task 保持 open

  Scenario S3: Unit 完成事件引用不存在的 task 时失败关闭
    Given payload 的 task_id 不对应 live task
    When exec.unit.done 触发 state projection
    Then projection 返回可观察错误
    And 不产生任意 task closed 的副作用

  Scenario S4: 成功或失败报告均可由相同路径正常结束
    Given 最近 accepted forge.report.done 的 report_path 为 P
    And status 为 COMPLETED 或 FAILED 或 BLOCKED
    When reporter 发布 report_path 为 P 的 LOOP_COMPLETE
    Then LOOP_COMPLETE 被接受
    And status 不被 completion gate 改写

  Scenario S5: completion 使用不同报告路径时被拒绝
    Given 最近 accepted forge.report.done 的 report_path 为 P1
    When reporter 发布 report_path 为 P2 的 LOOP_COMPLETE
    Then LOOP_COMPLETE 被拒绝并产生 completion correction
    And P1 对应的报告事实保持不变

  Scenario S6: resume 后仍使用先前报告路径完成
    Given forge.report.done 已在上一次运行被接受并持久化
    When loop 从 snapshot 或 replay 恢复
    And reporter 发布相同 report_path 的 LOOP_COMPLETE
    Then LOOP_COMPLETE 被接受
    And 不要求重发 forge.report.done

  Scenario S7: 诊断识别失败终态后的恢复
    Given accepted audit 为 REJECTED 且 accepted report 为 FAILED
    And 后续 artifact 被改为 ACCEPTED 但没有后续 accepted 成功 audit/report
    When diagnosis 重建时序
    Then 结论标记为失败终态后恢复
    And 不得声称零拒收或首轮完整成功

  Scenario S8: 诊断保留真实首轮成功结论
    Given accepted audit 为 ACCEPTED 且 accepted report 为 COMPLETED
    And artifact 与事件一致
    When diagnosis 重建时序
    Then 结论标记为首轮成功
    And 不产生恢复警告
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
| --- | --- | --- | --- | --- | --- |
| S1 | done accepted；目标 task closed；非目标 task 不变 | `crates/ralph-core/src/state_projector/tests.rs` + builtin preset parse | 单元+配置集成 | Characterization：先固定现有 close action | 否 |
| S2 | schema 拒绝；task open | `crates/ralph-cli/src/presets.rs`、preset lint tests | 配置契约 | 结构化 schema parity | 否 |
| S3 | projection error；零误关闭 | `crates/ralph-core/src/state_projector/tests.rs` | 单元 | Fault path | 否 |
| S4 | 三种 status 均接受 matching path | `crates/ralph-core/src/event_loop/tests/termination.rs` | EventLoop 集成 | 表驱动兼容测试 | 否 |
| S5 | mismatch 拒绝；correction 出现；predecessor 不变 | `crates/ralph-core/src/event_loop/tests/termination.rs` | EventLoop 集成 | Negative contract | 否 |
| S6 | snapshot/replay 后 matching completion 接受 | `crates/ralph-core/src/event_loop/tests/termination.rs` 或现有 snapshot 测试模块 | 状态恢复集成 | Recovery test | 否 |
| S7 | 输出含失败后恢复；不含零拒收 | `skills/tests/test_execution_model_contract.py` | Skill contract | Golden-free semantic assertion | 否 |
| S8 | 输出/规则保留首轮成功分类 | `skills/tests/test_execution_model_contract.py` | Skill contract | 对照组 | 否 |

测试层级采用最低成本原则：核心风险位于配置解析、EventLoop 接受门禁和 diagnosis 规则，无需启动真实外部 backend；现有 `run_workflow_guard_scenario` 负责主链 runtime 回归。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence | Unit |
| --- | --- | --- | --- | --- | --- | --- | --- | --- |
| R1 | done 原子关闭 task | S1-S3 | projector/preset tests | close/missing/unknown task | schema parity + runtime scenario | 不需要 | E3-E6、E11 | U1 |
| R2 | 两个终态 report_path 一致 | S4-S6 | termination tests | config validation/field compare | EventLoop recovery | 不需要 | E2、E3、E7-E10 | U2 |
| R3 | FAILED/BLOCKED 可结束 | S4 | status table test | comparison helper | EventLoop completion | 不需要 | E2、E8 | U2 |
| R4 | diagnosis 保留首轮事实 | S7-S8 | Python skill contract | 规则结构断言 | 双样本对照 | 不需要 | E10、E12 | U3 |
| R5 | 不改变 13-hat 主链 | S1、S4、S8 | preset strict lint | 无 | existing parallel-forge BDD | 不需要 | E1、E9 | U1-U3 |

## Implementation Units

严格串行：

```text
U1
  ↓ 完成全部测试、重构和回归
U2
  ↓ 完成全部测试、重构和回归
U3
```

### U1：由 `exec.unit.done` 原子关闭 task

#### 1. Unit 目标

当 executor 发布一个被接受的 `exec.unit.done` 时，唯一对应 task 变为 `closed`。

#### 2. 对应需求与 Scenario

- Requirement：R1、R5
- Scenario：S1-S3
- Decision：D1
- Evidence：E3-E6、E9、E11、E13

#### 3. 外部可观察结果

`ralph task list --status open` 不再列出已完成 Unit；依赖该 task 的 ready 选择使用真实 closed 状态；非法 done 不改变 task。

#### 4. 当前行为基线

`forge.plan.ready` 创建 task，但 `exec.unit.done` schema 无 `task_id` 且 state projection 无 close action（E3-E6）。先用现有 projector 测试固定 `CloseTask` 的成功/失败语义。

#### 5. 输入与输出

- 输入：含 `task_id`、`task_key` 及现有 Unit 字段的 `exec.unit.done`。
- 输出：accepted event 与 closed task。
- 错误：缺字段由 schema 拒绝；未知 task 由 projector 报错。
- 副作用：只关闭 payload 指向 task。
- 不变量：不关闭 sibling task；不改变 wave/slot/commit 字段；拒绝事件不改变 task store。

#### 6. 修改位置

- `presets/schemas/parallel-forge.yml`：为 `exec.unit.done` 增加 required `task_id/task_key`，不改变其他 event。
- `presets/en/parallel-forge.yml`：在 EventLoop state projection 增加 `exec.unit.done → close_task(task_id)`；executor instruction 从 ready payload 透传字段，不手工 close。
- `crates/ralph-core/src/state_projector/tests.rs`：覆盖 close 成功、未知 task、零旁路副作用。
- `crates/ralph-cli/src/presets.rs`：结构化断言 builtin 解析后的 schema/action，不断言 instruction 文案。
- `crates/ralph-core/tests/scenarios/parallel_forge_task_dispatch_runtime.yml`、`crates/ralph-core/tests/scenarios.rs`：不修改；作为真实 EventLoop 主链回归运行。task close 的状态断言固定在 projector 集成测试，避免用当前不暴露 task snapshot 的 scenario fixture 编造断言。
- `crates/ralph-core/data/ralph-tools-tasks.md`：通用说明 projection-owned terminal event 的 agent 动作。
- `skills/ralph-preset-common/references/agent-native-model.md`、`author-checklist.md`、`patterns.md`：说明事件携带 live task id 与配置投影模式。

不修改 task store API、supervisor DB、dispatcher 算法和其他 preset。

#### 7. 可依赖能力

现有 `CloseTask` action、task store、event schema required-fields、builtin strict lint、`run_workflow_guard_scenario`。

#### 8. 禁止依赖的未来能力

不得依赖 U2 的 completion gate 或 U3 的 diagnosis 规则；不得提前增加 report path 状态。

#### 9. 验收测试

- `accepted_exec_unit_done_closes_exact_task`：创建两个 task，发布带 live `task_id` 的 done；断言目标 closed、另一个 open。
- `parallel_forge_exec_unit_done_requires_task_identity`：解析 builtin schema，断言 `task_id/task_key` required，并存在 `CloseTask` projection。
- `exec_unit_done_unknown_task_does_not_close_any_task`：未知 id；断言 error 且所有 task 不变。
- 运行：第 9 节 U1 命令。

#### 10. Acceptance Red

先运行 builtin contract test。预期因 `exec.unit.done` 不要求 task identity、无 close projection 而失败；这直接证明测试命中目标缺口。编译错误、fixture 解析错误或错误测试过滤器不算有效 Red。

#### 11. 单元测试拆分

- Test 1：配置解析后的 action 类型、topic、payload field。
- Test 2：live task close 成功和 sibling 不变量。
- Test 3：unknown task error 和零副作用。
- Fake：使用现有临时 task store fixture。
- 不允许 Mock：不得 Mock `StateProjector::apply` 或真实 task close。

#### 12. Red → Green → Refactor 顺序

```text
配置契约 Test 1 Red
→ schema 与 preset 最小修改
→ Test 1 Green
→ projector Test 2 Red
→ 接通 exec.unit.done close action
→ Test 2 Green
→ failure Test 3 Red
→ 保持现有错误传播与原子边界
→ Test 3 Green
→ 去除重复 fixture，保持 typed action
```

#### 13. 最小实现范围

必须：done 事件透传 identity、配置 close action、结构化测试、通用 guide 更新。不得：新建 task API、在 instruction 中编排额外 CLI、修改 task dependency 算法。

#### 14. 集成验证

真实联合 preset parser、schema、state projector、task store；backend 可使用现有 mock response。运行 preset lint/parity 和现有 parallel-forge runtime scenario，预期事件拓扑及 13 hats 不变。

#### 15. 风险驱动测试

- Characterization：先确认现有 `CloseTask` 对 live/unknown task 的行为，避免错误归因。
- Contract：验证 schema required-fields 与 state projection 同时存在，防止只改一侧。
- 不需要 concurrency：事件投影仍走现有串行 acceptance 边界。

#### 16. 回归范围

- state projector 全部测试：共享 action enum 与 task store。
- parallel-forge strict lint/preset tests：schema/preset parity。
- parallel-forge runtime BDD：确保 ready→done 主链仍被真实 EventLoop 接受。
- 注入 guide drift：agent 不会同时手工 close 和投影 close。
- 全量 workspace：preset 是 builtin，可能影响 embedded config 构建。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| --- | --- | --- | --- |
| `presets/en/parallel-forge.yml` | 修改配置/instruction | 透传 task identity 并投影关闭 | E1-E5 |
| `presets/schemas/parallel-forge.yml` | 修改 schema | done required-fields 与 projection 对齐 | E3 |
| `crates/ralph-core/src/state_projector/tests.rs` | 修改测试 | 固定关闭与失败不变量 | E6 |
| `crates/ralph-cli/src/presets.rs` | 修改测试 | 结构化 builtin contract | E9 |
| `crates/ralph-core/data/ralph-tools-tasks.md` | 修改文档 | 同步 agent 动作契约 | E13 |
| `skills/ralph-preset-common/references/agent-native-model.md` | 修改文档 | 同步 AAF 模型 | E13 |
| `skills/ralph-preset-common/references/author-checklist.md` | 修改文档 | 增加 projection 检查 | E13 |
| `skills/ralph-preset-common/references/patterns.md` | 修改文档 | 记录可复用配置模式 | E13 |

#### 18. 完成标准

S1-S3 通过；U1 单元/集成/回归通过；format、clippy、build 通过；无 skip/only/削弱断言；没有提前实现 U2/U3；Evidence/Decision 未失效；可独立提交。

#### 19. 停止条件

若 `CloseTask` 不是 accepted event 后执行、done payload 无法取得 live task id、真实调用链与 E3-E6 不符、Red 未命中目标、需要新依赖或 regression 扩大，停止并执行：

```text
记录新证据 → 更新影响分析 → 重新比较方案 → 重新决策
→ 重新计算置信度 → 修订当前及后续 Unit
```

#### 20. 风险与注意事项

- 风险：projection 失败但 event 已被持久化。检测：failure test 同时检查 event/task 状态；缓解：沿用现有 projector 的 acceptance/rollback 顺序，不另写旁路。
- 风险：executor 使用 `task_key` 代替 `task_id`。检测：schema + instruction + fixture 三方 parity；缓解：两字段都透传，close action只取 `task_id`。
- 剩余风险：外部旧 preset payload 不兼容；仓库明确不要求向后兼容，且改动仅 builtin parallel-forge。

### U2：强制 `forge.report.done` 与 `LOOP_COMPLETE` 使用同一路径

#### 1. Unit 目标

当 completion path 与最近 accepted report path 不同时拒绝 `LOOP_COMPLETE`；相同时无论报告成功或失败均正常终止。

#### 2. 对应需求与 Scenario

- Requirement：R2、R3、R5
- Scenario：S4-S6
- Decision：D2-D4、D7
- Evidence：E2、E3、E7-E10、E13

#### 3. 外部可观察结果

operator 只会看到指向已发布 `forge.report.done` 报告的 completion；resume 不能以另一个覆盖文件路径制造新终态；FAILED/BLOCKED 仍能结束。

#### 4. 当前行为基线

required events 只证明 topic 出现；instruction 的“same report_path”没有 runtime enforcement，真实运行发生 report=FAILED 后 artifact 被覆盖且 completion 仍被接受（E2、E7、E10）。先增加 characterization test 证明未配置 gate 时旧行为不变。

#### 5. 输入与输出

- 输入：可选 `completion_payload_match`、predecessor JSON payload、completion JSON payload。
- 输出：accepted completion 或稳定 mismatch rejection/correction。
- 状态：保存最近 accepted 配置 topic payload并纳入 snapshot/replay。
- 不变量：不修改 status/final_audit；未配置 preset 行为不变；不扫描完整日志。

#### 6. 修改位置

- `crates/ralph-core/src/config/loop_config.rs`：新增可选 typed config 与校验。
- `crates/ralph-core/src/config/mod.rs`：按现有配置类型模式 re-export。
- `crates/ralph-core/src/event_loop/loop_state.rs`：记录最近 predecessor payload，提供字段比较结果。
- `crates/ralph-core/src/event_loop/mod.rs`：accepted predecessor 更新状态；completion 检查接入 mismatch correction。
- `crates/ralph-core/src/state/snapshot.rs`：在统一 `LedgerSnapshot` 中保存配对基准，并在 `apply_delta` 恢复对应 commit。
- `crates/ralph-core/src/state/commit.rs`：新增可 replay 的 predecessor-payload 更新 delta；不依赖不可持久化的纯内存赋值。
- `crates/ralph-core/src/state/tests.rs`：覆盖 commit round-trip 与 replay 后字段恢复。
- `crates/ralph-core/src/event_loop/tests/termination.rs`：未配置、matching、mismatch、缺字段/非法 JSON、三 status、resume/replay。
- `presets/en/parallel-forge.yml`：启用 path match；reporter resume instruction 只补发匹配 completion，不重写既有 report/audit。
- `crates/ralph-cli/src/presets.rs`：结构化断言 builtin 启用正确 topic/field。
- `crates/ralph-core/data/ralph-tools-emit.md`：通用说明 paired completion 的字段来源和停止条件。
- `skills/ralph-preset-common/references/agent-native-model.md`、`author-checklist.md`、`patterns.md`：同步配置能力。

不修改 verdict gate、required event 语义、Event 类型、CLI 子命令或其他 preset。

#### 7. 可依赖能力

现有 EventLoop completion gate、JSON payload parser、completion correction、LoopState snapshot、required event schema。

#### 8. 禁止依赖的未来能力

不得依赖 U3 diagnosis 输出；不得引入 artifact hash、报告版本、自动审计或多 topic join。

#### 9. 验收测试

- `completion_payload_match_accepts_matching_report_path_for_all_terminal_statuses`
- `completion_payload_match_rejects_different_report_path_and_injects_correction`
- `completion_payload_match_survives_snapshot_resume`
- `completion_payload_match_is_noop_when_unconfigured`
- `parallel_forge_configures_report_done_path_match`
- 断言 mismatch 时未标记 completion accepted、predecessor payload 未被 completion 覆盖、correction reason 指出 topic/field。

#### 10. Acceptance Red

先运行 mismatch EventLoop test。当前实现会接受 P2，因此“期望拒绝但实际接受”为有效 Red，并直接证明软 instruction 没有 runtime gate。配置解析失败、snapshot fixture 版本错误或测试未送达 completion 分支不算有效 Red。

#### 11. 单元测试拆分

- Test 1：config 缺 topic/空 fields/重复 fields 拒绝，未配置可解析。
- Test 2：matching field equality。
- Test 3：mismatch、missing field、non-object payload 产生确定 rejection。
- Test 4：predecessor 只在 accepted target topic 更新。
- Test 5：commit delta round-trip、ledger replay 与 LoopState 恢复。
- 不允许 Mock：EventLoop completion acceptance 和 correction 必须走真实路径。

#### 12. Red → Green → Refactor 顺序

```text
未配置 characterization Green
→ config validation Test 1 Red
→ 最小 typed config
→ Test 1 Green
→ matching/mismatch Tests 2-3 Red
→ LoopState 记录与字段比较
→ Tests 2-3 Green
→ accepted-topic Test 4 Red
→ 接入真实 event acceptance
→ Test 4 Green
→ resume Test 5 Red
→ CommitDelta + LedgerSnapshot apply_delta 的最小持久化
→ Test 5 Green
→ 统一错误 reason 与 correction 接口
```

#### 13. 最小实现范围

必须：单 predecessor、顶层字段数组、默认关闭、字段级错误、snapshot/replay、parallel-forge 启用。不得：JSONPath、多事件聚合、payload rewrite、新状态机、改变 verdict。

#### 14. 集成验证

真实联合 RalphConfig、LoopState、EventLoop、snapshot/correction 和 builtin preset。运行 termination tests、parallel-forge BDD、preset strict lint；预期 matching FAILED 也完成，mismatch 不完成。

#### 15. 风险驱动测试

- Recovery：predecessor 在旧进程、completion 在 resume 后。
- Contract：缺失字段与 schema 同时验证，防止只依赖 instruction。
- Differential：未配置 config 的 completion 结果与改动前一致。
- Fault injection：非法 JSON/非 object/字段缺失均 fail-close，但错误可修正。

#### 16. 回归范围

- 所有 completion/termination tests：共享 completion path。
- snapshot/state tests：新增 optional 状态字段。
- preset lint/config tests：新字段解析和 strict config。
- parallel-forge scenario：主链仍可结束。
- 其他 builtin preset tests：默认关闭不得改变行为。
- CLI doc drift/preset operator fixture：agent 可正确取得 predecessor path。
- workspace 全量：EventLoopConfig 是共享公开配置。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| --- | --- | --- | --- |
| `crates/ralph-core/src/config/loop_config.rs` | 修改生产文件 | 新增 opt-in typed config | E7 |
| `crates/ralph-core/src/config/mod.rs` | 修改生产文件 | re-export 配置类型 | E7 |
| `crates/ralph-core/src/event_loop/loop_state.rs` | 修改生产文件 | 保存并比较 predecessor payload | E7 |
| `crates/ralph-core/src/event_loop/mod.rs` | 修改生产文件 | 接入 acceptance 与 correction | E8 |
| `crates/ralph-core/src/state/snapshot.rs` | 修改生产文件 | snapshot 应用 replay delta 后保留匹配基准 | E7、E10 |
| `crates/ralph-core/src/state/commit.rs` | 修改生产文件 | predecessor 更新可写入并 replay | E7、E10 |
| `crates/ralph-core/src/state/tests.rs` | 修改测试 | commit round-trip 与 ledger replay | E7、E10 |
| `crates/ralph-core/src/event_loop/tests/termination.rs` | 修改测试 | completion/recovery 行为 | E8 |
| `presets/en/parallel-forge.yml` | 修改配置/instruction | 启用 match 并约束 resume | E2、E10 |
| `crates/ralph-cli/src/presets.rs` | 修改测试 | 结构化 builtin contract | E9 |
| `crates/ralph-core/data/ralph-tools-emit.md` | 修改文档 | 同步 agent completion 动作 | E13 |
| `skills/ralph-preset-common/references/agent-native-model.md` | 修改文档 | 同步 AAF 能力 | E13 |
| `skills/ralph-preset-common/references/author-checklist.md` | 修改文档 | 增加 paired completion 检查 | E13 |
| `skills/ralph-preset-common/references/patterns.md` | 修改文档 | 记录通用配置模式 | E13 |

#### 18. 完成标准

S4-S6 通过；未配置 differential 通过；snapshot/replay、preset lint、BDD、全量回归通过；FAILED/BLOCKED matching case 明确通过；无 skip/only/断言削弱；可独立提交。

#### 19. 停止条件

若 LoopState snapshot 无法稳定恢复、accepted event hook 晚于 completion check、correction 无法复用、必须更改公开 Event 类型或其他 preset 出现行为变化，立即停止并按统一重新决策流程修订计划。

#### 20. 风险与注意事项

- 风险：新增 delta 但漏接 `LedgerSnapshot::apply_delta`，导致同进程通过、resume 丢失。检测：commit round-trip + `StateLedger::replay_from_disk`；缓解：delta 定义、apply 分支和 replay 测试同一 Unit 完成。
- 风险：记录了被拒绝的 `forge.report.done`。检测：accepted/rejected 对照测试；缓解：只在 policy/schema acceptance 后更新。
- 风险：mismatch correction 形成循环。检测：先 P2 拒绝，再 P1 接受；缓解：reason 明确指出原 topic、字段和 expected source。
- 剩余风险：同一运行多次 accepted `forge.report.done` 时采用“最近一次”；本 preset reporter 是单 owner，符合现有契约。

### U3：诊断区分首轮终态与后续恢复

#### 1. Unit 目标

当 accepted event 显示 REJECTED/FAILED、后续仅有 artifact/commit 修复时，diagnosis 必须报告“失败终态后恢复”，不得报告“零拒收、首轮成功”。

#### 2. 对应需求与 Scenario

- Requirement：R4、R5
- Scenario：S7-S8
- Decision：D5、D6
- Evidence：E10、E12

#### 3. 外部可观察结果

诊断报告同时给出首轮终态、恢复动作和最终代码状态；manager 不会因覆盖后的 Markdown 文件误判原始审计链。

#### 4. 当前行为基线

diagnosis references 有事件/工件证据层级，但 report template 未要求 audit→report→completion→artifact/commit 的时序表，也未禁止用后写 artifact 覆盖先前 accepted verdict（E10、E12）。用真实事故抽象成最小正反样本。

#### 5. 输入与输出

- 输入：accepted audit/report/completion events、artifact mtimes/content、Git commits、task transitions。
- 输出：`initial_terminal_status`、`recovery_status`、最终分类与一致性告警。
- 错误：缺失 event 时标记证据不足，不猜测成功。
- 不变量：`LOOP_COMPLETE` 只证明 terminated；mutable artifact 不反写 event verdict。

#### 6. 修改位置

- `skills/ralph-run-diagnosis/references/log-reconciliation.md`：增加终态 chronology 决策表与冲突优先级。
- `skills/ralph-run-diagnosis/references/report-template.md`：要求“首轮终态/恢复/最终代码状态”分栏和禁用表述。
- `skills/ralph-run-diagnosis/references/verification-pipeline.md`：L4 增加 event-artifact temporal consistency。
- `.agents/skills/ralph-run-diagnosis/references/` 对应三个文件：与 canonical 内容同步。
- `skills/tests/test_execution_model_contract.py`：加入 rejected→artifact accepted 和 clean accepted 对照 contract。

不修改 diagnosis CLI、runtime event、原始事故报告或生产 Rust。

#### 7. 可依赖能力

现有 Tier A/B/C 证据分层、terminal table、report template、Python contract test、U2 固化的“completion 不等于成功”术语。

#### 8. 禁止依赖的未来能力

不得要求 artifact versioning、重新审计事件或新 diagnosis parser；不得为特定 plan/preset 写一次性规则。

#### 9. 验收测试

- `test_diagnosis_requires_terminal_event_artifact_chronology`
- `test_diagnosis_forbids_zero_rejection_claim_after_failed_terminal`
- `test_diagnosis_preserves_clean_first_pass_success`
- 断言 canonical 与 `.agents` 镜像一致；断言规则使用通用术语，不出现本次 plan id、事故路径或特定 preset 名。

#### 10. Acceptance Red

先运行新增 contract test，预期因 references 缺少 chronology/failed-after-recovery 规则而失败。路径拼错、镜像未加载、Python 环境缺包不算有效 Red。

#### 11. 单元测试拆分

- Test 1：log reconciliation 明确 event 优先和两层终态。
- Test 2：report template 必含 recovery classification 且禁止“LOOP_COMPLETE=成功”。
- Test 3：verification L4 包含冲突检查。
- Test 4：canonical/mirror parity。
- 不 Mock 实际 reference 内容；不做整文件 byte snapshot，避免锁死可演进文案。

#### 12. Red → Green → Refactor 顺序

```text
chronology contract Test 1 Red
→ 最小 reconciliation 规则
→ Test 1 Green
→ report/verification Tests 2-3 Red
→ 最小模板与门禁更新
→ Tests 2-3 Green
→ mirror Test 4 Red
→ 同步 .agents 镜像
→ Test 4 Green
→ 去除事故专用措辞，保留通用决策表
```

#### 13. 最小实现范围

必须：事件优先级、首轮终态、恢复分类、成功对照、镜像同步。不得：写新 parser、修改真实事故报告、加入某次运行路径/plan id/preset 名。

#### 14. 集成验证

运行单个 Python contract 文件和全部 `skills/tests`；人工以 E10 时序代入模板，预期得到“initial FAILED/REJECTED + recovered artifact/code”，而不是“zero rejection”。

#### 15. 风险驱动测试

- Differential：同一规则同时验证 failed-then-recovered 与 clean-success，防止一律降级。
- Contract：canonical 和分发镜像一致。
- 不用 Golden：文案可演进，只断言语义义务和禁止性结论。

#### 16. 回归范围

- `skills/tests/test_execution_model_contract.py`：共享 skill 契约。
- 全部 `skills/tests`：镜像与 operator skill 可能共享 helper。
- `ralph-run-diagnosis` canonical/mirror diff：防止安装来源行为不同。
- 不运行 Rust targeted test作为本 Unit 局部门禁，但最终仍执行 workspace 全量。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| --- | --- | --- | --- |
| `skills/ralph-run-diagnosis/references/log-reconciliation.md` | 修改文档 | 固化事件/工件时序优先级 | E10、E12 |
| `skills/ralph-run-diagnosis/references/report-template.md` | 修改文档 | 输出首轮终态与恢复 | E10、E12 |
| `skills/ralph-run-diagnosis/references/verification-pipeline.md` | 修改文档 | 增加 L4 一致性门禁 | E12 |
| `.agents/skills/ralph-run-diagnosis/references/log-reconciliation.md` | 修改镜像 | 同步 canonical | E12 |
| `.agents/skills/ralph-run-diagnosis/references/report-template.md` | 修改镜像 | 同步 canonical | E12 |
| `.agents/skills/ralph-run-diagnosis/references/verification-pipeline.md` | 修改镜像 | 同步 canonical | E12 |
| `skills/tests/test_execution_model_contract.py` | 修改测试 | 语义 contract 与对照组 | E12 |

#### 18. 完成标准

S7-S8 通过；canonical/mirror 一致；Python contract/all skill tests 通过；无事故专用内容；不把 LOOP_COMPLETE 等同成功；最终 Rust/文档全量门禁通过；可独立提交。

#### 19. 停止条件

若 canonical/mirror 实际生成关系与 E12 不符、现有测试禁止语义级断言、需要新增 parser 才能表达规则，停止并按统一重新决策流程更新计划。

#### 20. 风险与注意事项

- 风险：规则过度保守，把真正二次成功误报为失败。检测：若存在后续 accepted ACCEPTED/COMPLETED audit/report，允许更新最终终态但保留首轮记录；缓解：按事件序列而非“首次事件永远最终”。
- 风险：测试锁死文案。检测：review test assertion；缓解：断言必备概念和禁用结论，不断言整段文本。
- 剩余风险：旧运行缺少完整事件时只能标记“不确定”，不能恢复缺失事实。

## 8. Unit 串行依赖图

```text
U1 task 完成投影
  ↓
U2 终态 payload 配对
  ↓
U3 diagnosis 时序一致性
```

- U1→U2：先消除“完成工作仍 open”的恢复噪声，U2 的 resume 测试才能只聚焦终态配对；不得在 U1 提前引入 completion 状态。
- U2→U3：U3 使用 U2 明确的语义边界“completion=terminated，不等于 success”；不得在 U2 修改 diagnosis 文案。
- U3 与生产代码逻辑独立，但仍严格后置，以最终 runtime 契约为诊断规则事实源。

## Verification Contract

### 9. 执行命令清单

| 时机 | 命令 | 验证目的 | 预期结果 | 失败后可继续 |
| --- | --- | --- | --- | --- |
| 开始前 | `cargo nextest --version` | nextest 版本符合 `0.9.140` | 输出 `cargo-nextest 0.9.140` | 否 |
| U1 Red/Green | `cargo nextest run -p ralph-core -- state_projector` | task close 投影 | 目标测试由预期 Red 转 Green | 否 |
| U1 contract | `cargo nextest run -p ralph-cli --bin ralph -- presets` | builtin 结构化配置 | parallel-forge contract 通过 | 否 |
| U1/U2 BDD | `cargo nextest run -p ralph-core --test scenarios parallel_forge` | 真实 EventLoop 主链 | 相关 scenarios 通过 | 否 |
| U2 Red/Green | `cargo nextest run -p ralph-core -- completion_payload_match` | matching/mismatch/recovery | 全部通过 | 否 |
| preset lint | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | CLI preset lint | 通过 | 否 |
| preset lint | `cargo nextest run -p ralph-core -- preset_lint` | core schema/lint | 通过 | 否 |
| schema smoke | `ralph preset check -H builtin:parallel-forge --strict` | builtin strict check | 无 error | 否 |
| schema smoke | `ralph emit --schema exec.unit.done -H builtin:parallel-forge` | 查看 done required fields | 含 `task_id/task_key` | 否 |
| schema smoke | `ralph emit --schema LOOP_COMPLETE -H builtin:parallel-forge` | completion schema 可用 | 含 `report_path` | 否 |
| U3 contract | `.venv/bin/python -m pytest skills/tests/test_execution_model_contract.py -q` | diagnosis 语义 | 通过 | 否 |
| U3 regression | `.venv/bin/python -m pytest skills/tests -q` | skill 全量回归 | 通过 | 否 |
| doc drift | `scripts/check-cli-doc-drift.sh --strict` | 注入 guide/CLI 漂移 | 通过 | 否 |
| format | `cargo fmt --check` | Rust 格式 | 无 diff | 否 |
| lint | `cargo clippy` | 仓库配置的静态检查 | 零 error | 否 |
| build | `cargo build` | 仓库构建目标 | 通过 | 否 |
| 最终 | `./scripts/run-tests.sh` | 两阶段 nextest + doctest 全量 | 全绿 | 否 |
| flake 兜底 | `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | 仅确认竞态/时序 flake | 全绿；仅在默认全量疑似 flake 时使用 | 否 |

禁止使用裸 `cargo test -p ralph-cli`。若修改/新增 spawn `ralph` 的测试，必须先使用 `common::ralph_bin()` scrub agent runtime env，并用污染环境再跑相关 nextest。

### 10. 最终质量门禁

- S1-S8 全部通过并追踪到 U1-U3。
- task close、schema parity、completion match、snapshot/replay、diagnosis contract 全部通过。
- matching `COMPLETED/FAILED/BLOCKED` 都可终止；mismatch 不可终止。
- Characterization、Differential、Recovery、Contract 测试通过。
- parallel-forge 仍为 13 hats，现有 wave 主链和 runtime BDD 通过。
- preset/schema/instruction、注入 guide、operator skill、diagnosis mirror 同步。
- `cargo fmt --check`、clippy、build、`./scripts/run-tests.sh` 全部通过。
- 没有新增失败/跳过测试、`.only`、忽略标记、削弱断言或无解释 snapshot/golden 更新。
- 没有未处理 BLOCKED 决策，关键决策置信度均 ≥0.85。
- 实际变更不超出 U1-U3；每个 Unit 独立提交并完成 Red→Green→Refactor→Integration→Regression→Close。

## Definition of Done

1. 已完成 U1→U2→U3，未交替开发。
2. accepted `exec.unit.done` 关闭且只关闭对应 task。
3. parallel-forge runtime 拒绝不同 `report_path` 的 completion，并在 resume 后接受相同路径。
4. FAILED/BLOCKED 报告仍能正常 `LOOP_COMPLETE`。
5. diagnosis 对 E10 类时序输出“失败终态后恢复”，不输出“零拒收、首轮完整成功”。
6. 所有文档同步规则、preset/schema 下游清单和全量测试门禁满足。
7. 三个 Unit 均可形成独立提交，不包含 `.ralph/review/*/scratch` 等临时产物。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
| --- | --- | --- |
| 这是实施计划而不是 Roadmap 吗 | 是 | U1-U3 都以外部行为和真实入口组织 |
| Executor 是否仍需做关键设计决策 | 否 | D1-D7 已确定配置、入口、错误与恢复语义 |
| 所有文件和接口是否有代码库证据 | 是 | E1-E13；条件文件明确要求先验证可观察性 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | 最低 D7=0.89 |
| 是否存在未处理的低置信度假设 | 否 | 无阻塞假设 |
| 每个 Unit 是否只有一个可观察行为 | 是 | task close、path pair、diagnosis chronology |
| 每个 Unit 是否可以独立验证 | 是 | 各 Unit 有 targeted tests 和完成标准 |
| 每个 Unit 是否有真实 Red | 是 | 分别为缺 projection、mismatch 被接受、缺 chronology rule |
| 每个 Unit 是否包含回归范围 | 是 | 各 Unit 第 16 节 |
| 是否存在未来 Unit 依赖 | 否 | 仅依赖已完成前置 Unit |
| 是否存在泛化任务描述 | 否 | 均指向具体行为、文件、命令和断言 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 第 5、6 节 |
| 所有关键决策是否有 Evidence | 是 | D1-D7 均引用 Evidence |
| 计划是否可以严格串行执行 | 是 | U1→U2→U3 |
