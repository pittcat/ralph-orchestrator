---
date: 2026-06-12
topic: workflow-activation-contract
title: Workflow Activation Contract — ce-executor-isolated 编排机制护栏
---

## Summary

在 Ralph 编排层建立 **Workflow Activation Contract**：`ralph preset check` / preflight 启动前静态校验 hat 的 trigger→publish→下游可达性；isolated 运行时对 handoff topic 保证目标 hat 被 dispatch、对 terminal/business payload 硬拒收。以 `ce-executor-isolated` 为验收夹具，把 wave/dispatch 类故障从「反复改 preset YAML」升级为「机制硬拦 + 窄域运行时兜底」。

## Problem Frame

`ce-executor-isolated` 的 wave 与 step 推进问题已多次以 preset 补丁、instructions 微调、运行时兜底等方式修复，但在真实 plan 运行中仍复发。`docs/report/2026-06-12-ce-executor-isolated-dispatch-gap-diagnosis.md` 记录的 loop `2026-06-10-003-...-merry-wren` 中，U1 已闭环，但 `plan-gate` 发出 `queue.advance (next_step=step-02)` 后 executor 10 分钟未被 dispatch；ralph hat 兜底后 executor 尝试 re-emit `queue.advance` 被 isolated scope 拒收，最终只能 `loop.cancel` 终止 plan。

现有机制各自覆盖了一部分，但存在系统性盲区：

- `preset_lint`（ownership、topic 格式、multi-hat isolation）不校验「hat 被某 topic 激活后能否合法推进工作流」。
- `preset_validator`（拓扑可达性）与 `declared_edges_from_hats`（drift 观测）把 `(queue.advance → work.done)` 视为合法边——图论上通，但无法阻止 agent re-emit 触发 topic、也无法保证 handoff 后及时 dispatch。
- `hatless_ralph` 对 unreachable trigger 仅 **warn**，不阻断启动。
- `event_policy` / drift 对 null payload、string-as-json-object 偏观测或 warn，stall 时 ralph 注入的 null `review.passed` 仍会污染事件流并阻塞 `plan-gate`。

用户目标不是再打一版 YAML，而是让同类编排错误在 **机制层** 不可启动、handoff 在 **运行时** 不可静默卡死。

## Key Decisions

- **静态优先于运行时魔法**：不在 runner 层做隐式 topic 桥接（例如自动把 `queue.advance` 转成 `work.ready`）。编排语义必须在 preset 中显式声明，机制只负责校验与兜底。
- **分层交付**：P0 为静态 Workflow Activation Contract（启动硬门）；P1 为运行时 handoff dispatch 保证 + payload hard gate。两层独立可测，但 acceptance 需同时满足。
- **handoff dispatch 是窄例外**：仅覆盖「有唯一消费者的单播 handoff topic」，不整体推翻 isolated 模式下的 U4 round-robin fair scheduling。
- **handoff topic 清单采用混合策略**：内置常见 handoff topic 种子列表 + 从 preset 图自动推导「唯一消费者」topic；二者取并集，冲突时以推导结果为准并 emit lint 提示。
- **Lint 严格度**：builtin preset（含 `ce-executor-isolated`）在 preflight / `ralph run` 路径一律 **strict error**；用户自定义 preset 默认 warn，可通过 `--strict` 或配置升为 error。
- **ce-executor-isolated 是验收夹具，不是唯一 deliverable**：机制落地后必须同步修 preset 使 contract 通过，但需求交付物是机制能力本身。

## Actors

- **A1. Preset 作者** — 编写或修改 hat collection YAML；需要在 `ralph preset check` 阶段看到可操作的 contract 违规报告。
- **A2. Loop runner（isolated 模式）** — 消费 handoff 事件、执行 hat 选择、enforce payload 与 dispatch 保证。
- **A3. Workflow hat（executor、plan-gate、review-coordinator 等）** — 在 contract 约束下 emit 事件；不再依赖 instructions 单独承担拓扑正确性。
- **A4. Ralph hat（fallback）** — 仅在机制允许的 control topic 范围内兜底；不应再成为推进 multi-step plan 的唯一路径。
- **A5. Plan 操作者** — 运行 `ralph run -H builtin:ce-executor-isolated`；期望 step 推进与 review wave 无需人工介入 rescue。

## Key Flows

- **F1. Preset 启动前 contract 校验**
  - **Trigger:** `ralph preset check`、`ralph preflight`、`ralph run` 加载 preset。
  - **Actors:** A1, A2
  - **Steps:** 解析 hats 的 triggers/publishes/terminal_events → 运行 Workflow Activation Contract 规则族 → builtin preset 违规为 error 阻断启动；用户 preset 默认 warn。
  - **Outcome:** 类似 `executor` 订阅 `queue.advance` 却不能 publish 且存在 re-emit trap 的配置，在 loop 启动前被拒绝。
  - **Covered by:** R1–R6

- **F2. Handoff 事件落盘后的 dispatch 保证**
  - **Trigger:** isolated 模式下某 hat publish 一条 handoff topic（如 `queue.advance`、`work.ready`）。
  - **Actors:** A2, A3
  - **Steps:** 识别 handoff topic 的唯一消费者 hat → 将该 hat 标记为高优先级 pending → 下一轮 hat 选择必须选中该消费者（在 fair scheduling 域内作为窄例外）→ 若在配置时限内仍未 dispatch，写入 recovery 诊断并 escalation。
  - **Outcome:** `queue.advance` 后 executor 在可配置时限内（默认 30s）被启动，不出现 10 分钟静默 gap。
  - **Covered by:** R7–R9

- **F3. Terminal / business payload 硬拒收**
  - **Trigger:** agent 通过 `ralph emit` 或 JSONL 写入 business/terminal 事件。
  - **Actors:** A2, A3
  - **Steps:** `event_policy` 对配置的 topic 集合执行 null payload reject、json_object schema 的类型强制（含 string 双重 JSON 编码的 normalize-or-reject）→ 拒收事件不进入主事件流 → 注入可路由的 recovery 信号（非 null 占位 terminal）。
  - **Outcome:** 不出现 `review.passed` payload=null 污染事件流；`review.wave.ready` 不以裸 string 形态落盘。
  - **Covered by:** R10–R12

- **F4. ce-executor-isolated 多 step plan 验收跑通**
  - **Trigger:** 使用 `docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md` 或等价 8-step plan 启动 loop。
  - **Actors:** A5, A2, A3
  - **Steps:** U1 闭环 → `queue.advance` → U2 executor 启动 → review wave 4 维并行 → synthesizer `review.passed` full payload → 重复至 plan 终态。
  - **Outcome:** 到达 `REVIEW_COMPLETE → LOOP_COMPLETE`，无 `loop.cancel` 因 dispatch gap 触发。
  - **Covered by:** R13–R15

## Requirements

**静态 Workflow Activation Contract**

- R1. `ralph preset check` 与 preflight 必须运行 Workflow Activation Contract 规则族，输出带稳定 finding ID、hat、topic、action hint 的报告。
- R2. **Re-emit trap**：若 hat H 的 `triggers` 包含 topic T，且 T 由另一 hat 的 `publishes` 声明，且 T ∉ H.`publishes`，则报 **error**（strict）或 **warn**（default）。Finding 必须点名「agent 常会尝试 re-emit 触发 topic」风险。
- R3. **Activation egress**：对每个 `(hat, trigger)` 对，必须存在至少一条长度 ≤2 的可达路径：从该 hat 的任一 `publishes` topic 出发，能触发至少一个其他 hat 或命中 preset 声明的 terminal/completion 链。无路径则违规。
- R4. **Handoff pairing**：若 hat A publish topic T 且仅 hat B trigger T（唯一消费者），则 B 的 activation egress（R3）必须能到达该 plan 的下一业务阶段（例如 `work.done` → review 链，或 `work.ready` → executor 实施链）。仅「理论上能发某个 publish」不足以通过，必须连到实际下游消费者。
- R5. **Trigger/publish 不对称告警**：若 hat H trigger topic T 但 H 的合法响应集合（`publishes` + 允许的 terminal 路径）无法闭合 T 所代表的业务阶段，报 error。覆盖 `work.retry` 等「能触发但无法响应」的已知缺口类。
- R6. Builtin preset（`presets/manifest.yml` embedded 列表）违反 R2–R5 任一规则时，preflight 与 `ralph run` **必须拒绝启动**（exit non-zero），错误消息可供 CI 字面匹配。

**运行时 handoff dispatch 保证**

- R7. Isolated 模式下，handoff topic 集合至少包含：`queue.advance`、`work.ready`、`fix.plan.ready`、`work.failed`（作为失败 handoff）；实现可扩展，但不得少于上述种子。
- R8. 当 handoff topic T 被 publish 且存在唯一消费者 hat B 时，B 必须在 **默认 30s** 内进入 activation（可通过 preset 或全局配置覆盖，上限 120s）。超时必须写 `recovery.jsonl` envelope，source 为 `stall_recovery` 或专用 source，且不得静默等待 round-robin 多轮。
- R9. Handoff priority dispatch 不得破坏多消费者 topic 的 fair scheduling：仅当消费者 hat 数量为 1 时启用优先 dispatch；多消费者 topic 仍走 round-robin。

**Payload / schema 硬执行**

- R10. 对配置的 business/terminal topic（至少包含 `review.passed`、`review.failed`、`review.complete`、`work.done`、`queue.advance`、`review.wave.ready`），`payload: null` 必须 **Reject**（非 Warn），不写入主 `events.jsonl`。
- R11. 对 schema 声明 `payload: json_object` 的 topic，若收到 JSON string 且内容为合法 JSON object，允许 **normalize** 为 object 后接受；若无法解析为 object，Reject。
- R12. Wave emit 路径必须在写入前校验 `wave_total == len(payloads)`；违反时拒收整批并返回可操作的 CLI 错误（延续 `docs/solutions/integration-issues/ce-executor-wave-emission-must-batch-in-single-emit-2026-06-09.md` 的机制化，而非仅靠 instructions）。

**验收与 preset 同步**

- R13. 机制落地后，`presets/en/ce-executor-isolated.yml` 与 `presets/zh/ce-executor-isolated-zh.yml` 必须通过 R6 strict contract，且作为 CI 回归用例。
- R14. 在 `2026-06-10-003` plan worktree 或等价 fixture 上，U1 `queue.advance` 后 U2 executor activation 时间 **< 30s**（相对 `queue.advance` 事件时间戳）。
- R15. 同一验收跑中，主事件流 **0 条** `review.passed` / `review.wave.ready` 的 null 或 string-as-object 违规落盘；review wave 以单次 batch（`wave_total > 1`）发射。

## Acceptance Examples

- **AE1. Re-emit trap 拦住 executor+queue.advance**
  - **Covers:** R2, R6
  - **Given:** preset 中 `executor.triggers` 含 `queue.advance`，`executor.publishes` 不含 `queue.advance`
  - **When:** 运行 `ralph preset check --strict` 或 preflight
  - **Then:** 输出 error finding，ID 稳定；`ralph run` 不进入 event loop

- **AE2. queue.advance 后 executor 及时启动**
  - **Covers:** R7, R8, R14
  - **Given:** 通过 contract 的 `ce-executor-isolated` preset，U1 已 `work.done`，plan-gate 发出 `queue.advance (next_step=step-02)`
  - **When:** 记录 `queue.advance` 与 executor 首次 activation 时间戳
  - **Then:** 间隔 < 30s；无 10 分钟级事件空窗

- **AE3. null review.passed 被拒收**
  - **Covers:** R10, R15
  - **Given:** synthesizer stall 或 ralph 兜底尝试写入 `review.passed` payload=null
  - **When:** 事件进入 `event_policy`
  - **Then:** 事件不进入主 events.jsonl；产生 recovery/escalation 记录；plan-gate 不被 null payload 假阳性触发

- **AE4. review.wave.ready string payload 规范化**
  - **Covers:** R11
  - **Given:** agent emit `review.wave.ready`，payload 为合法 JSON 的 string 形式
  - **When:** 解析与 policy 校验
  - **Then:** 落盘 payload 为 object；dimension-reviewer wave 正常并发

- **AE5. 多消费者 topic 仍 fair scheduling**
  - **Covers:** R9
  - **Given:** 某 topic 有两个 hat 同时 trigger
  - **When:** 连续 publish 该 topic 多次
  - **Then:** 两个 hat 按 round-robin 被选中，不固定偏向字典序首项

## Success Criteria

- `ce-executor-isolated` 在 strict contract 下 preflight 通过率 100%；修复前已知 P0 违规（re-emit trap、handoff pairing、`work.retry` 不对称）在 preset 同步后清零。
- `2026-06-10-003` 类 multi-step plan 可无人值守从 U1 推进到 U2+，不因 dispatch gap 触发 `loop.cancel`。
- 主事件流中 handoff 与 terminal topic 的 null payload 计数为 0；`review.wave.ready` string 违规计数为 0（normalize 后不计违规）。
- CI 新增 contract 回归：`cargo test` / BDD scenario 覆盖至少 AE1、AE3；replay fixture 或 scenario YAML 覆盖 AE2 的可自动化部分。
- 同类故障复发时，操作者首选动作变为「读 preset check finding 修编排」，而非「再改一轮 instructions」。

## Scope Boundaries

**Deferred for later**

- `diagnosis-summary.json` counter 与 `active-activations.json` stale 状态修复（诊断报告 P1-2、P2-3）。
- `prerequisite_topics` 因果顺序、schema 版本化、Saga 补偿（见 `docs/report/2026-06-03-preset-orchestration-stability-gap.md`）。
- Coordinator 强制创建 `decisions.md`、task 标题与 task_key 对齐等 agent 产物规范（P2 级）。
- 扩展 `RALPH_CONTROL_TOPICS` 让 ralph hat 模拟 workflow publish。

**Outside this product's identity**

- Runner 隐式 topic 桥接（`queue.advance` → `work.ready` 自动转换）作为主修复路径。
- 禁用 `review_step_state` synth_terminal gate 以「让 plan 先跑起来」。
- 用 round-robin 全局优先级覆盖替代窄域 handoff dispatch（破坏 U4 公平调度原则）。
- 纯 instructions / prompt 改动而不伴随机制护栏。

## Dependencies / Assumptions

- U3 isolated 终态 authority（`publishes` 显式声明）与 U4 fair scheduling 保持不动；本需求在其之上叠加 contract 与窄例外。
- `preset_lint`、`preset_validator`、`event_policy`、`event_origin`、`event_bus` hat 选择逻辑为扩展点；不要求新建独立 crate。
- `ce-executor-isolated` 继续作为 10-hat isolated builtin preset；验收 plan 使用现有 `docs/plans/2026-06-10-003-*.md`。
- 假设「唯一消费者」可从静态 preset 图可靠推导；若 wildcard trigger 导致多消费者歧义，R9 优先于 R8（不启用 priority dispatch）。
- 假设 strict null reject 后，stall 路径仍有非 null 的 escalation（如 `plan.blocked`、`review.failed` 含 `aggregate_timeout`），需 planning 阶段定义，但不阻塞本需求 P0 静态 contract。

## Outstanding Questions

**Resolve Before Planning**

- （无）Path B 已确认：handoff 混合推导策略、builtin strict / 用户 preset warn 分级、拒绝隐式桥接。

**Deferred to Planning**

- Handoff 超时阈值是否暴露为 `event_loop.workflow_contract.handoff_dispatch_timeout_seconds` 或复用现有 stall 配置。
- String→object normalize 是否对所有 `json_object` topic 全局启用，还是仅 whitelist（建议全局，planning 评估 perf 与兼容）。
- Re-emit trap 对「hat 同时 trigger 且 publish 同一 topic」（自环）是否豁免——初步判断应豁免，planning 用图算法确认。

## Sources / Research

- `docs/report/2026-06-12-ce-executor-isolated-dispatch-gap-diagnosis.md` — 本次 dispatch gap 因果链、P0-1/P0-2/P0-3 归因、修复建议与反模式清单。
- `docs/report/2026-06-09-ce-executor-mechanism-vs-orchestration-diagnosis.md` — 机制 vs 编排分层；历史上「改 preset」路径的局限。
- `docs/report/2026-06-03-preset-orchestration-stability-gap.md` — `preset_validator` 已有能力与 orphan/prerequisite 缺口。
- `docs/solutions/integration-issues/ce-executor-wave-emission-must-batch-in-single-emit-2026-06-09.md` — wave batch 机制化先例。
- `crates/ralph-core/src/preset_lint/` — 现有静态 lint 入口与 finding ID 契约。
- `crates/ralph-core/src/preset_validator.rs` — 拓扑可达性 `validate_preset_topology`。
- `crates/ralph-core/src/drift/engine.rs` — `declared_edges_from_hats`（观测边，非硬门）。
- `crates/ralph-core/src/hatless_ralph.rs` — unreachable trigger 仅 warn 的现有行为。
- `presets/en/ce-executor-isolated.yml` — executor / plan-gate triggers、publishes 不对称的具体配置。
