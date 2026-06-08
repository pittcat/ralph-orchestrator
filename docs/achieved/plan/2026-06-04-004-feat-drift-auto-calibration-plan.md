---
title: "Runtime Diagnosis & Recovery Intelligence: 运行时诊断、精准恢复与离线报告体系"
type: feat
status: active
date: 2026-06-04
updated: 2026-06-05
origin: "2026-06-04 Drift Auto-Calibration plan + 2026-06-05 brainstorm"
---

# Runtime Diagnosis & Recovery Intelligence: 运行时诊断、精准恢复与离线报告体系

## Overview

本计划将原 **Drift Auto-Calibration** 升级为 Ralph 的运行时诊断与恢复智能层。目标不是再加一个孤立的 pause/retry 机制，而是让 Ralph 在每次 preset 编排运行后都能回答四个问题：

1. 哪里坏了？
2. 为什么坏？
3. Ralph 有没有自动恢复？
4. 恢复有没有效果，还是陷入了无效震荡？

新的能力由两部分组成：

- **精准治愈增强**：统一 Stall Recovery、Hard Gate、Workflow Guard、Execution Contract、Payload Contract、Drift Monitor 的诊断结构，让 `task.resume` / `human.guidance` / pause 都携带可执行的失败原因。
- **诊断报告产物**：把运行时碎片化证据保存为 session-scoped JSONL，再通过 `ralph diagnose` 生成面向 operator 的 Markdown/JSON 报告，用于快速定位 preset、hat、topic、payload contract 和拓扑缺陷。

> 核心设计哲学：Drift 不是第 5 层治愈，而是诊断层的一种信号源；真正的能力是把所有恢复路径变成可审计、可聚合、可报告的闭环。

---

## Problem Frame

### 现有自修复能力的结构性盲区

Ralph 当前已经具备多种治愈或门控机制，但它们的诊断输出分散，且恢复动作缺少统一的原因模型：

| 机制 | 触发条件 | 当前恢复/处置 | 主要缺口 |
|---|---|---|---|
| **Stall Recovery** | Hat 完全没发事件 | `task.resume` 通用提醒 | 不知道为什么没发，容易盲打 |
| **Missing-Event Hard Gate** | Hat 有 publish 义务但本轮无事件 | 注入 `human.guidance` | 能指出没 emit，但没有纳入统一恢复效果追踪 |
| **Policy Reject / Workflow Guard** | 格式、顺序或 workflow phase 非法 | `task.resume` + 局部错误说明 | 缺少跨轮统计，无法判断同类错误是否反复出现 |
| **Execution Contract Recovery** | `work.done` 等完成声明缺 task/git 字段证据 | 记录 `ExecutionContractRejected`，尝试 targeted recovery | 已有精准路由，但诊断仍停留在 orchestration log，不会形成 run-level 报告 |
| **Payload Contract Gate** | preset 启动或运行时 payload contract 明显不一致 | 写 violation report 并终止 | 是硬失败报告，不覆盖“合法但质量下降” |
| **Hook Retry** | 外部命令失败 | 指数退避重试 | 与 agent 行为缺陷没有统一归因 |

这些机制解决了“明显坏”和“完全没做”的问题，但仍有三类缺陷很难快速定位。

### 盲区 1：合法但异常

```text
Builder 发 work.done：
  之前：{"task_id":"T-1", "plan_name":"fix-auth", "test_evidence":"..."}
  现在：{"task_id":"T-1"}
```

- Policy：`plan_name` 如果不是 required，就不会拦截。
- Contract：只检查最小字段时可能放行。
- Stall / Hard Gate：事件确实发了，不触发。
- 结果：下游 Reviewer 仍被激活，但缺少真实可用上下文。

### 盲区 2：治愈无效循环

```text
Builder 缺 plan_name
→ task.resume
→ Builder 再发
→ 仍然缺 plan_name
→ 再 resume
```

如果 recovery payload 只说“上一轮没有正确发布事件”，agent 不知道具体修什么。每次 resume 都消耗 iteration，直到 max_iterations、loop stale 或人工介入。

### 盲区 3：渐进式拓扑断裂

`work.done` 后 `review.wave.ready` 的响应率从 100% 降到 60%，再降到 30%。单个事件各自合法，但 preset 的编排拓扑已经坏了。

### 目标重构

原计划把 Drift Monitor 视为“诊断增强”。这个判断是对的，但范围还不够。真正需要建设的是：

- 一个统一的 **Recovery Diagnosis Envelope**，描述每次恢复为什么发生、指向谁、期望对方做什么、后续是否恢复。
- 一个 session 级 **诊断证据链**，把 orchestration、agent output、contract rejection、hard gate、drift finding、recovery outcome 聚合起来。
- 一个 `ralph diagnose` CLI，让 operator 跑完任意 preset 后能快速得到缺陷定位报告。

---

## Requirements Trace

- **R1.** 记录统一的 `RecoveryDiagnosisEnvelope`，覆盖 recovery source、source hat、target hat、topic、reason、expected action、evidence、severity。
- **R2.** 现有 recovery 路径必须写入 recovery journal：stall recovery、missing-event hard gate、workflow guard rejection、execution contract rejection、payload contract violation、drift intervention。
- **R3.** 增强可恢复路径的 `task.resume` / `human.guidance` payload，让 agent 能看到具体缺陷和下一步动作。
- **R4.** 监控 `field_completeness`：统计每个 `(topic, field)` 的出现率，覆盖 required fields 和被下游使用的 optional fields。
- **R5.** 监控 `coord_join_rate`：统计 `(from_topic, to_topic)` 的关联率，发现 preset 拓扑断裂。
- **R6.** 监控 `emit_cadence`：统计每个 topic 的发射频率/间隔异常。
- **R7.** 实现恢复效果追踪：同一个 diagnosis key 在后续窗口恢复、恶化、重复失败时写入 outcome。
- **R8.** 实现 retry escalation：连续 N 次同类恢复失败后，不再盲目 retry，升级为 hard pause 或 human guidance。
- **R9.** 新增 diagnostics 产物：`recovery.jsonl`、`drift.jsonl`、`diagnosis-summary.json`，并继续写入 `orchestration.jsonl` 的关键事件。
- **R10.** 新增 `ralph diagnose`：读取 `.ralph/diagnostics/<session>/`，输出 Markdown 报告；支持 JSON 输出给后续工具消费。
- **R11.** 配置层新增 `telemetry.runtime_diagnosis`，控制采集、阈值、报告生成、最大 prompt 注入长度。
- **R12.** 文档说明如何用诊断报告调试 preset、hat instructions、payload contract 和 workflow topology。

---

## Scope Boundaries

### In Scope

- Rust 核心内的 runtime diagnosis 数据模型、journal、drift 指标、recovery outcome tracking。
- `EventLoop` / `loop_runner` 与现有 recovery 路径的最小接入。
- diagnostics session 目录下的 JSONL / JSON 产物。
- `ralph diagnose` CLI 生成 Markdown/JSON 报告。
- 针对 `ce-executor` / `ce-executor-wave` 的回放或集成测试。

### Out of Scope

- 不做 OpenTelemetry、Prometheus、Grafana exporter。
- 不做模型语义 drift、prompt embedding 相似度、LLM judge 自动归因。
- 不做 Python 版高级离线算法；MVP 的报告分析用 Rust 实现。
- 不改 `task.resume` 事件协议本身，只增强 payload 和诊断记录。
- 不让 Drift Monitor 直接决定业务正确性；它只产生诊断信号和升级建议。

### Follow-Up Work

- OTel/Prometheus exporter。
- Python 或 notebook 离线分析器，用于自适应阈值、因果推断、跨 session 趋势。
- Web dashboard 展示诊断报告。
- 多 session preset 健康评分。

---

## Context & Research

### Repo Reality Check（2026-06-05）

当前仓库已经从旧的大文件结构继续拆分，计划必须基于下面的真实落点实施：

| 领域 | 当前落点 | 计划影响 |
|---|---|---|
| Config | `crates/ralph-core/src/config/` + `config/ralph_config.rs` + `config/mod.rs` | 新增配置应创建独立 `config/telemetry.rs`，再在 `RalphConfig` 中接入。不要再引用旧 `config.rs`。 |
| Prompt 注入 | `crates/ralph-core/src/event_loop/mod.rs` 的 `build_prompt()`，现有注入链为 `inject_phase_into_prompt()` → `prepend_auto_inject_skills()` → scratchpad/state/tasks | Drift Alert / Recovery Alert 应进入同一 prompt pipeline，且限制长度。 |
| Main loop | `crates/ralph-cli/src/loop_runner/runner.rs` | 每轮 agent 输出处理、contract rejection、payload violation、missing-event gate、termination summary 都在这里编排。 |
| Hard gate | `crates/ralph-cli/src/loop_runner/hard_gate.rs` | 已有 `should_gate_missing_events()`、`inject_missing_event_hard_gate_guidance()`、`handle_execution_contract_rejections()`。计划应增强记录和 envelope，不重写 gate。 |
| Diagnostics | `crates/ralph-core/src/diagnostics/` | 已有 `orchestration.jsonl`、`performance.jsonl`、`errors.jsonl`、`hook-runs.jsonl`、`agent-output.jsonl`、`prompt-log.md`。新增 logger 应跟现有 collector 模式一致。 |
| Summary | `crates/ralph-core/src/summary_writer.rs` | 已写 `.ralph/agent/summary.md`。诊断 summary 应作为 diagnostics session 产物，不混入 agent-facing summary。 |
| CLI | `crates/ralph-cli/src/main.rs` + `crates/ralph-cli/src/commands/` | 新增 `Diagnose` 子命令，命令实现放 `commands/diagnose.rs` 或独立 `diagnose.rs`，保持 clap 子命令模式。 |

### Relevant Existing Patterns

- `DiagnosticsCollector::with_enabled()`：session 目录和子 logger 初始化模式。
- `OrchestrationEvent` tagged enum：结构化、可扩展的 JSONL 事件格式。
- `handle_execution_contract_rejections()`：已经记录 `ExecutionContractRejected` 和 `ContractRecoveryRouted`，是 recovery journal 的首个接入点。
- `write_payload_contract_violation_report()`：硬失败时写独立诊断文件的既有模式。
- `SummaryWriter`：终止时聚合 events / tasks / commit 的报告思路，但不适合直接承载 operator 诊断。
- `docs/solutions/developer-experience/agent-execution-contract-gates-2026-06-03.md`：明确实施型 hat 不应用 `default_publishes` 制造假成功；contract rejection 必须 targeted retry；no safe target 必须 fail closed。

### External Research Decision

本计划主要扩展本仓库已有 orchestration、diagnostics 和 CLI 模式；没有引入外部协议、云服务或第三方分析框架。当前阶段不做外部 research，优先遵循本地代码和 institutional learning。

### Design Review Findings（实施前必须消化）

本轮审查发现若干会导致 bug 或计划落空的点，实施时必须显式处理：

| Finding | 当前现实 | 风险 | 计划处理 |
|---|---|---|---|
| **诊断 session 可能分裂** | `main.rs` 为 tracing layer 创建过 `DiagnosticsCollector`；`EventLoop::new/with_context` 又会创建自己的 collector | 同一次 run 的 trace、orchestration、recovery 被写到两个不同 timestamp 目录，`ralph diagnose latest` 只能读到半套证据 | 增加 U0，先明确 session ownership：每次 run 只有一个 authoritative diagnostics session，由 EventLoop/LoopContext 传递给所有 logger |
| **`RALPH_DIAGNOSTICS` 与 `telemetry.runtime_diagnosis` 职责不清** | 现有 diagnostics 只在 `RALPH_DIAGNOSTICS=1` 时创建 session；计划又新增配置项 | 用户启用 telemetry 但没有 session，报告命令找不到数据；或默认打开导致回归 | U0/U1 定义 activation matrix：默认 no-op；`RALPH_DIAGNOSTICS=1` 开 full diagnostics；`telemetry.runtime_diagnosis.write_artifacts=true` 可开 minimal diagnosis session |
| **Rejected event 不经过 EventBus observer** | workflow guard、state machine、execution contract 拒绝后，原事件不会 publish 到 bus | 如果 drift/recovery 只靠 observer，会漏掉最重要的失败事件 | U4 在 validation/rejection 分支直接写 envelope；U5 的 observer 只负责 accepted/published event 的 drift signal |
| **EventBus observer 是同步调用且无 panic 隔离** | `EventBus::publish()` 直接循环调用 observers | drift observer 阻塞或 panic 会破坏事件路由主路径 | U5 要求 observer closure 内部捕获错误、不 panic、non-blocking；测试覆盖 panic/满队列不影响 publish |
| **Workflow guard helper 没有 diagnostics 依赖** | `apply_workflow_guard_validation()` 当前只返回 accepted events，并直接 publish `task.resume` | 直接在 helper 中写 diagnostics 会引入全局依赖或破坏纯 helper 边界 | U4 要么返回 rejection diagnostics，要么注入轻量 sink，由调用方写日志 |
| **Prompt 注入路径多处重复** | `build_prompt()` 有 solo、coordinator、isolated、fallback 多条分支 | 只改一条路径会导致某些 hat 看不到 alert，或错注入到 coordinator | U6 必须抽统一 `apply_runtime_diagnosis_prompt()`，所有 final prompt 返回前都经过同一函数 |
| **现有 stale/thrashing 已有终止机制** | `LoopState` 已有 `consecutive_hard_gates`、`LoopStale`、`LoopThrashing` | 新 retry escalation 若再造计数，会和已有终止条件打架 | U6 只能复用或补充现有 loop-state 计数，不允许创建平行且互相不知道的终止机制 |
| **payload contract violation 已写 root-level report** | `write_payload_contract_violation_report()` 当前写 `.ralph/diagnostics/payload-contract-error-*.json` | 新 session report 如果不引用旧文件，会把硬失败证据分裂 | U4/U7 要把 root-level violation report path 作为 evidence ref 收进 summary/report |

---

## Key Technical Decisions

1. **统一诊断 envelope 优先于新增恢复机制。**  
   新能力先让所有恢复路径说清楚“为什么恢复、谁负责、怎么验证恢复”，再做 drift 算法。否则会继续制造新的碎片化日志。

2. **Recovery journal 是一等产物。**  
   `recovery.jsonl` 记录每次恢复尝试、升级、成功、失败。`orchestration.jsonl` 保留高层事件，`recovery.jsonl` 保存可报告的细节。

3. **Drift Monitor 是信号源，不是裁判。**  
   Drift 只生成 `DriftFinding` 和建议 severity；是否 prompt 注入、targeted resume、pause 由 recovery responder 统一处理。

4. **报告生成读取 diagnostics session，不重新执行 loop。**  
   `ralph diagnose` 是离线分析器，默认读取最近一次 diagnostics session，也可指定 session path。它不依赖 live EventBus。

5. **默认启用采集要保守。**  
   诊断日志写入应低成本、结构化、可关闭。Prompt 注入默认限制条数和字符数，避免诊断本身导致 token 膨胀。

6. **恢复升级必须 fail closed。**  
   同类问题重复出现时不能无限 `task.resume`。若无 safe target 或 retry window 耗尽，应 pause 并生成报告入口。

---

## Output Structure

```text
crates/ralph-core/src/
├── config/
│   └── telemetry.rs                 # NEW: runtime diagnosis / drift 配置
├── diagnosis/                       # NEW: 统一诊断模型和离线聚合
│   ├── mod.rs
│   ├── envelope.rs                  # RecoveryDiagnosisEnvelope
│   ├── journal.rs                   # recovery/drift JSONL record types
│   ├── reporter.rs                  # diagnostics session 聚合与 report model
│   └── tests.rs
├── drift/                           # NEW: drift 信号源
│   ├── mod.rs
│   ├── window.rs
│   ├── detector.rs
│   ├── alert.rs
│   └── tests.rs
└── diagnostics/
    ├── recovery.rs                  # NEW: recovery.jsonl logger
    ├── drift.rs                     # NEW: drift.jsonl logger
    └── mod.rs                       # 接入新 logger

crates/ralph-cli/src/
├── commands/
│   └── diagnose.rs                  # NEW: ralph diagnose
└── loop_runner/
    ├── hard_gate.rs                 # 增强 existing recovery 记录
    └── runner.rs                    # 接入 drift/recovery outcome hooks

docs/guide/
└── runtime-diagnosis.md             # NEW: operator 使用说明
```

Diagnostics session 目标结构：

```text
.ralph/diagnostics/<session>/
├── agent-output.jsonl
├── orchestration.jsonl
├── errors.jsonl
├── performance.jsonl
├── hook-runs.jsonl
├── prompt-log.md
├── recovery.jsonl                   # NEW
├── drift.jsonl                      # NEW
├── diagnosis-summary.json           # NEW
└── diagnosis-report.md              # NEW, 可由 ralph diagnose 生成或刷新
```

---

## High-Level Design

```text
Agent output / events JSONL
        │
        ▼
EventLoop processing
        │
        ├─ policy / workflow guard / contract / payload gate
        │       │
        │       └─ RecoveryDiagnosisEnvelope
        │              ├─ DiagnosticsCollector.log_recovery()
        │              ├─ OrchestrationEvent::* high-level audit
        │              └─ RecoveryResponder enhanced payload
        │
        ├─ EventBus observer
        │       └─ DriftMonitor → DriftFinding → RecoveryDiagnosisEnvelope
        │
        └─ Loop termination
                └─ diagnosis-summary.json seed

ralph diagnose
        │
        ├─ read diagnostics session
        ├─ aggregate recovery / drift / orchestration / errors
        └─ write diagnosis-report.md + diagnosis-summary.json
```

## How the Diagnosis Report Is Produced（通俗版）

诊断报告不是让另一个 agent 重新猜，也不是运行时临时拼一句总结。它分成两个动作：

1. **运行时只记录事实。**  
   Ralph 每一轮本来就会处理事件、选择 hat、执行 gate、写 diagnostics。本计划只是把这些“现场证据”按固定格式多存几份：谁没 emit、哪个 contract 拒绝了、哪个 topic 缺字段、哪次 recovery 发给了谁、后面有没有恢复。

2. **跑完后离线生成报告。**  
   `ralph diagnose` 读取 `.ralph/diagnostics/<session>/` 里的 JSONL 文件，把碎片化事实聚合成报告。它不重新跑 loop，不影响原 run，也不靠猜测。

可以把它理解成：

```text
ralph run 期间：
  事件流 + gate 结果 + recovery 结果 + drift 指标
        ↓
  写入 .ralph/diagnostics/<session>/*.jsonl

ralph diagnose 期间：
  读取这些 jsonl
        ↓
  聚合、排序、归因
        ↓
  输出 diagnosis-report.md / diagnosis-summary.json
```

报告回答的是 operator 最关心的问题：

- 哪个 hat 最容易出问题？
- 是没 emit、emit 错、payload 缺字段、contract 拒绝，还是下游没响应？
- Ralph 尝试恢复了吗？恢复给了哪个 hat？
- 同一个问题重复发生了几次？
- 最应该改 preset、hat instructions、contract，还是 runtime 配置？

首版报告不追求“智能推理很炫”，而是追求“证据可查、结论稳定、能直接定位问题”。

### Activation Matrix（什么时候会有报告数据）

| 场景 | 是否创建 diagnostics session | 写哪些文件 | 预期行为 |
|---|---|---|---|
| 默认运行，无 `RALPH_DIAGNOSTICS`，无 telemetry 配置 | 否 | 无新增文件 | 完全保持现有行为 |
| `RALPH_DIAGNOSTICS=1 ralph run ...` | 是，full session | 现有 diagnostics + recovery/drift/summary | 最完整报告 |
| `telemetry.runtime_diagnosis.write_artifacts: true` | 是，minimal diagnosis session | recovery/drift/summary，不强制 agent-output/prompt-log | 适合 preset 长期调试，低成本 |
| `ralph diagnose --session latest` 但没有 session | 否 | 不写 | 输出明确错误和建议命令，不 panic |
| `ralph diagnose --session <path>` 且文件缺失 | 读取已有文件 | 报告 warning | 降级报告，不阻断 |

这张表是实现的契约：`ralph diagnose` 只读落盘事实；如果没有事实，它只能提示如何重新运行，不能伪造诊断。

## Delivery Strategy

这个计划横跨 core、CLI、diagnostics 和 loop runner，不能按“先实现全部模型再一次性接所有路径”的方式推进。实施时按三个 gate 交付：

| Gate | 目标 | 必须完成 | 暂不要求 |
|---|---|---|---|
| **G1: 诊断证据链 MVP** | 跑完 loop 后能看到 recovery journal 和基础报告 | U0-U4 的 session ownership、missing-event / execution-contract / payload-contract 接入，U7 的基础 `ralph diagnose --format json` | Drift 指标、prompt 注入、retry escalation |
| **G2: Drift + 报告增强** | 报告能定位“合法但异常”和拓扑断裂 | U5，U7 Markdown 报告，`ce-executor-wave` coord join fixture | 自动 pause、复杂自适应阈值 |
| **G3: 精准恢复闭环** | Ralph 能根据同类问题重复失败升级恢复策略 | U6、U8、U9 | Web dashboard、多 session 健康评分 |

每个 gate 都必须保持现有 run 行为可回归；G1 先证明“记录和报告”价值，再让 G2/G3 引入更主动的 runtime 行为。

### RecoveryDiagnosisEnvelope（方向性结构）

```text
{
  schema_version,
  diagnosis_id,
  iteration,
  source,
  severity,
  source_hat,
  target_hat,
  topic,
  reason_code,
  message,
  expected_action,
  evidence,
  retry_key,
  retry_attempt,
  safe_target,
  outcome
}
```

说明：

- `source` 取值示例：`stall_recovery`、`missing_event_gate`、`workflow_guard`、`execution_contract`、`payload_contract`、`drift_monitor`、`hook_retry`。
- `retry_key` 用于跨轮聚合，建议由 `(source, target_hat, topic, reason_code, field/path)` 稳定生成。
- `outcome` 初始为 `pending`，后续可更新为 `recovered`、`repeated`、`escalated`、`failed`。

---

## Implementation Units

- [ ] U0. **Diagnostics Session Ownership & Activation（先做地基）**

**Goal:** 确保一次 `ralph run` 只产生一个 authoritative diagnostics session，并定义 `RALPH_DIAGNOSTICS` 与 `telemetry.runtime_diagnosis` 的关系。

**Requirements:** R9, R10, R11

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/diagnostics/mod.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-cli/src/main.rs`
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`
- Test: `crates/ralph-core/src/diagnostics/integration_tests.rs`
- Test: `crates/ralph-cli/src/loop_runner/tests.rs`
- Test: `crates/ralph-cli/tests/diagnose.rs`

**Approach:**
- 新增 `DiagnosticsOptions` 或等价结构，明确：
  - `full_diagnostics`：由 `RALPH_DIAGNOSTICS=1` 控制，保持现有 agent-output、prompt-log、trace 行为。
  - `runtime_diagnosis_artifacts`：由 `telemetry.runtime_diagnosis.write_artifacts` 或 full diagnostics 打开。
  - `session_dir`：一次 run 内只能生成一次，并被 EventLoop、loop_runner、trace layer、diagnose summary 共用。
- 避免 `main.rs` 和 `EventLoop::new()` 各自创建不同 session。推荐方向：
  - `main.rs` 只负责 tracing/log setup；
  - `EventLoop::with_context_and_diagnostics()` 接收已解析 options；
  - 或将 `DiagnosticsCollector::new()` 改为可复用已存在 session path。
- `ralph diagnose` 自身不创建 run diagnostics session；它只读 session。

**Non-regression constraints:**
- 默认无 env、无 telemetry 时，不创建 `.ralph/diagnostics/<session>/`。
- `RALPH_DIAGNOSTICS=1` 的现有文件仍存在：`orchestration.jsonl`、`performance.jsonl`、`errors.jsonl`、`hook-runs.jsonl`、`agent-output.jsonl`、`prompt-log.md`。
- diagnostics 初始化失败时，loop 仍能按现有 fallback 跑，只输出 warning。

**Test scenarios:**
- 默认运行构造 EventLoop：collector disabled，session_dir 为 None。
- full diagnostics：只创建一个 session 目录，所有 logger 写同一目录。
- minimal runtime diagnosis：创建 session，但不强制创建 agent-output/prompt-log。
- diagnostics init 失败：run 不 panic，collector disabled。
- `ralph diagnose --session latest` 在无 session 时返回明确错误信息。

**Verification:**
- `rtk cargo test -p ralph-core diagnostics`
- `rtk cargo test -p ralph-cli diagnose`

---

- [ ] U1. **配置层：Telemetry / RuntimeDiagnosisConfig**

**Goal:** 新增配置段，控制 runtime diagnosis、drift 指标、retry escalation 和报告生成。

**Requirements:** R4, R5, R6, R8, R11

**Dependencies:** None

**Files:**
- Create: `crates/ralph-core/src/config/telemetry.rs`
- Modify: `crates/ralph-core/src/config/mod.rs`
- Modify: `crates/ralph-core/src/config/ralph_config.rs`
- Test: `crates/ralph-core/src/config/telemetry.rs`
- Test: `crates/ralph-core/src/config/ralph_config.rs`

**Approach:**
- 新增 `TelemetryConfig`，包含 `runtime_diagnosis: RuntimeDiagnosisConfig`。
- `RuntimeDiagnosisConfig` 包含：
  - `enabled`
  - `write_artifacts`（是否允许 minimal diagnosis session；默认 false）
  - `prompt_injection_enabled`
  - `max_prompt_findings`
  - `max_prompt_chars`
  - `retry_window_iterations`
  - `max_repeated_recoveries`
  - `artifact_retention`
  - `malformed_jsonl_policy`
  - `drift: DriftConfig`
- `DriftConfig` 包含 `window_size`、`field_completeness_threshold`、`coord_join_rate_threshold`、`emit_cadence_sigma`。
- 全字段 `#[serde(default)]`，并在 `RalphConfig::validate()` 中校验阈值和窗口大小。
- 不复用现有 `enable_metrics` 字段；该字段当前是 deferred warning，继续保持原语义，避免破坏兼容。
- 配置命名必须避免和 `event_loop.event_policy` / `execution_contracts` 混淆：telemetry 只控制观测、诊断产物和 prompt alert，不控制 contract 是否 enforce。

**Patterns to follow:**
- `config/memories.rs`、`config/tasks.rs` 的 serde/default 模式。
- `config/ralph_config.rs` 的字段接入、`Default` 和 `validate()` 模式。

**Test scenarios:**
- YAML 省略 `telemetry` 时默认值稳定。
- `telemetry.runtime_diagnosis.enabled: true` 正确解析。
- `write_artifacts: true` 不要求 `RALPH_DIAGNOSTICS=1`，但只创建 minimal diagnosis session。
- `enabled: false` 且 `write_artifacts: true` 时 validate 返回 warning 或 error，必须明确一种行为。
- `window_size: 0` 返回 config error 或 warning，不静默运行。
- 阈值小于 0 或大于 1 时 validate 失败。

**Verification:**
- `rtk cargo test -p ralph-core config::telemetry`
- `rtk cargo test -p ralph-core config::ralph_config`

---

- [ ] U2. **统一诊断模型：RecoveryDiagnosisEnvelope + Journal Records**

**Goal:** 定义所有 recovery / drift / report 共享的数据结构，避免每条路径自造 payload。

**Requirements:** R1, R2, R7, R9

**Dependencies:** U1

**Files:**
- Create: `crates/ralph-core/src/diagnosis/mod.rs`
- Create: `crates/ralph-core/src/diagnosis/envelope.rs`
- Create: `crates/ralph-core/src/diagnosis/journal.rs`
- Modify: `crates/ralph-core/src/lib.rs`
- Test: `crates/ralph-core/src/diagnosis/tests.rs`

**Approach:**
- 定义 `RecoveryDiagnosisEnvelope`、`DiagnosisSource`、`DiagnosisSeverity`、`DiagnosisOutcome`、`EvidenceRef`。
- 提供 builder/helper，让现有代码能用低摩擦方式构造 envelope。
- 定义 JSONL record 类型：`RecoveryJournalEntry`、`DriftJournalEntry`。
- `retry_key` 必须稳定、可聚合，并避免包含长 payload。
- `DiagnosisSource` 首版必须包含：
  - `stall_recovery`
  - `missing_event_gate`
  - `workflow_guard`
  - `execution_contract`
  - `payload_contract`
  - `drift_monitor`
  - `hook_retry`
  - `loop_stale`
- `DiagnosisOutcome` 首版必须包含：
  - `pending`
  - `recovered`
  - `repeated`
  - `escalated`
  - `failed`
  - `not_retriable`
- `EvidenceRef` 优先保存短字段和文件引用，不保存完整 prompt 或完整 payload；长文本必须截断。

**Outcome update rules:**

| Source | recovered 判断 | repeated 判断 | failed/escalated 判断 |
|---|---|---|---|
| missing_event_gate | 后续同一 target hat 产生 allowed topic | 同一 target hat 再次无事件 | 达到 hard gate 上限或 loop stale |
| execution_contract | 后续同一 source hat 发出通过 contract 的同 topic | 同一 topic 再次被同类 finding 拒绝 | no safe target 或 retry window 耗尽 |
| workflow_guard | 后续出现 next expected topic 或 phase progress | 同一 chain/instance 再次 out-of-order | workflow stale breaker 触发 |
| payload_contract | 无自动 recovered；用户改 preset 后新 run 验证 | 同一 violation kind 再次出现 | 当前 run 终止即 failed |
| drift_monitor | 窗口内指标回到阈值以上 | 同一 finding 连续低于阈值 | retry window 耗尽后 escalated |

**Patterns to follow:**
- `execution_contract::ExecutionContractFinding` 的结构化 finding 模式。
- `diagnostics/orchestration.rs` 的 serde tagged enum 模式。

**Test scenarios:**
- 每种 `DiagnosisSource` 可序列化/反序列化。
- 相同 source/hat/topic/reason/field 生成相同 `retry_key`。
- evidence 中长文本会被截断或引用化，不膨胀 JSONL。
- 缺少 target hat 时可表达 `safe_target: false`。

**Verification:**
- `rtk cargo test -p ralph-core diagnosis`

---

- [ ] U3. **Diagnostics Journal：recovery.jsonl / drift.jsonl / summary seed**

**Goal:** 扩展 diagnostics collector，把恢复与漂移作为一等 session artifact。

**Requirements:** R2, R7, R9

**Dependencies:** U2

**Files:**
- Create: `crates/ralph-core/src/diagnostics/recovery.rs`
- Create: `crates/ralph-core/src/diagnostics/drift.rs`
- Modify: `crates/ralph-core/src/diagnostics/mod.rs`
- Modify: `crates/ralph-core/src/diagnostics/orchestration.rs`
- Test: `crates/ralph-core/src/diagnostics/integration_tests.rs`

**Approach:**
- `DiagnosticsCollector` 新增 `recovery_logger` 和 `drift_logger`。
- 新增 `log_recovery()`、`log_drift()`、`write_diagnosis_summary_seed()`。
- `OrchestrationEvent` 增加高层 variants，例如 `RecoveryDiagnosed`、`RecoveryEscalated`、`DriftDetected`。
- 遵守 U0 activation matrix：
  - full diagnostics：创建现有全部 logger + 新 recovery/drift logger。
  - minimal runtime diagnosis：只创建 recovery/drift/summary 相关 logger，不强制 prompt-log 或 agent-output。
  - disabled：全部 no-op。
- `log_recovery()` 失败不能返回错误给 caller；内部吞掉 I/O 错误并通过 tracing warning 暴露。
- 每条 recovery journal 都必须带 `session_id` 或可从路径恢复的 session metadata，方便报告验证来源。
- `diagnosis-summary.json` 是 report seed，不是最终报告；它可在 termination 时先写，`ralph diagnose` 再刷新。

**Patterns to follow:**
- `diagnostics/orchestration.rs` 和 `diagnostics/hook_runs.rs` 的 logger 初始化与 JSONL 写法。
- `DiagnosticsCollector::with_enabled()` 的可选 logger 模式。

**Test scenarios:**
- diagnostics enabled 时创建 `recovery.jsonl` 和 `drift.jsonl`。
- diagnostics disabled 时 `log_recovery()` / `log_drift()` no-op。
- minimal runtime diagnosis enabled 时，不要求 `agent-output.jsonl` 存在，但 `recovery.jsonl` 存在。
- `RecoveryDiagnosed` 写入 `orchestration.jsonl`，详细字段写入 `recovery.jsonl`。
- JSONL 每行都是合法 JSON。
- logger 目录不可写时，collector disabled 或部分 logger disabled，不影响 loop。
- 同一 run 内所有 diagnostics 文件在同一个 session 目录。

**Verification:**
- `rtk cargo test -p ralph-core diagnostics`

---

- [ ] U4. **接入现有恢复路径：RecoveryDiagnosisEnvelope Everywhere**

**Goal:** 将已有 recovery/gate 统一记录到 recovery journal，并增强恢复 payload。

**Requirements:** R1, R2, R3, R7, R8

**Dependencies:** U2, U3

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner/hard_gate.rs`
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`
- Modify: `crates/ralph-cli/src/loop_runner/payload_contract_gate.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-cli/src/loop_runner/tests.rs`
- Test: `crates/ralph-core/src/event_loop/tests/workflow_guard.rs`
- Test: `crates/ralph-core/src/event_loop/tests/replay_light_integration.rs`

**Approach:**
- Missing-event hard gate：构造 envelope，记录 expected topics、target hat、expected command。
- Execution contract rejection：复用 `handle_execution_contract_rejections()`，补写 `log_recovery()`，并把 `ContractRecoveryRouted` 与 envelope 关联。
- Workflow guard rejection：当前 `apply_workflow_guard_validation()` 只接收 `guards`、`workflow_progress` 和 `bus`，没有 diagnostics collector。实施时不要在 helper 内硬塞全局依赖；应改为返回 `WorkflowGuardRejection` 诊断结果，或注入一个轻量 sink/callback，由调用方统一写 `log_recovery()`。
- Payload contract violation：终止前写 envelope 和 hard-failure artifact reference。
- Stall fallback：`inject_fallback_event()` 增强 `task.resume` payload，并写 envelope。
- 恢复 payload 增加统一段落：

```text
## Recovery Diagnosis
- reason:
- target:
- expected action:
- retry attempt:
- evidence:
```

**Per-source implementation detail:**

| Source | Hook point | Envelope target | Must not change |
|---|---|---|---|
| stall_recovery | `EventLoop::inject_fallback_event()` | last active hat 或 ralph fallback | 原有 fallback 选择和 return bool |
| missing_event_gate | `loop_runner/runner.rs` 调用 `inject_missing_event_hard_gate_guidance()` 前后 | `display_hat` | `agent_wrote_any_valid_or_rejected` 计算和 default_publishes skip 逻辑 |
| execution_contract | `loop_runner/hard_gate.rs::handle_execution_contract_rejections()` | safe retry target 或 none | rejected event 不进入 bus；targeted retry 仍按现有逻辑 |
| workflow_guard | `apply_workflow_guard_validation()` rejection 分支 | 触发 out-of-order 的 source hat 或 coordinator | 不 advance workflow progress；不记录 rejected event |
| payload_contract | `runner.rs` 处理 `payload_contract_violation` 处 | none / preset author | `TerminationReason::PayloadContractViolation` 不变 |
| hook_retry | hook executor/dispatcher 的 retry disposition | hook name + stage | hook retry/backoff 行为不变 |

**Implementation caution:**
- 不要把 `human.guidance` 当作 active hat routing 的替代品；已有经验表明只发 guidance 可能让 coordinator 截胡。能 safe target 时仍要发 targeted `task.resume`。
- 不要把 envelope 写入 events JSONL；它是 diagnostics artifact，不是业务事件，避免触发 hat。
- 不要让 recovery journal 的写入结果影响 `agent_wrote_any_valid_or_rejected`。

**Test scenarios:**
- missing-event gate 后 `recovery.jsonl` 有 `source=missing_event_gate`。
- execution contract rejected 后 envelope 的 target 是原发 hat；no safe target 时 `safe_target=false`。
- execution contract rejected 后下游 reviewer 不激活。
- workflow guard out-of-order 后 envelope 包含 next expected topic。
- workflow guard rejected 后 workflow progress 不推进。
- payload contract violation 终止后 report reference 指向写出的 violation file。
- payload contract violation 的 termination reason 仍是 `PayloadContractViolation`。
- 增强 payload 不破坏现有 active hat 选择。

**Verification:**
- `rtk cargo test -p ralph-cli hard_gate`
- `rtk cargo test -p ralph-core workflow_guard`
- `rtk cargo test -p ralph-core replay_light_integration`

---

- [ ] U5. **Drift Monitor 信号源**

**Goal:** 实现运行时 drift 监控，输出 finding，而不是直接操纵 loop。

**Requirements:** R4, R5, R6

**Dependencies:** U1, U2, U3

**Files:**
- Create: `crates/ralph-core/src/drift/mod.rs`
- Create: `crates/ralph-core/src/drift/window.rs`
- Create: `crates/ralph-core/src/drift/detector.rs`
- Create: `crates/ralph-core/src/drift/alert.rs`
- Modify: `crates/ralph-core/src/lib.rs`
- Test: `crates/ralph-core/src/drift/tests.rs`
- Test: `crates/ralph-core/tests/drift_integration.rs`

**Approach:**
- `DriftWindow` 保存轻量 `EventSnapshot`：topic、source hat、payload field set、timestamp、iteration、wave id。
- `DriftDetector` 计算：
  - `field_completeness(topic, field)`
  - `coord_join_rate(from_topic, to_topic)`
  - `emit_cadence(topic)`
- `DriftFinding` 转换为 `RecoveryDiagnosisEnvelope`，source 为 `drift_monitor`。
- EventBus observer 只观察已经 publish 的事件；rejected event 由 U4 的 rejection 分支直接记录，不进入 drift observer。
- EventBus observer 只做轻量入队，计算在独立 worker 或 loop-safe accumulator 中完成。初版优先选择简单、可测试实现；若使用线程，observer 必须 bounded + non-blocking。
- observer closure 内不能 panic；所有解析和 send failure 都转成 internal warning/drop counter。

**Metric detail:**
- `field_completeness` 的字段来源分三类：
  - event_policy schema required fields
  - execution_contract required payload fields
  - downstream declared/read fields（若当前代码已有结构化来源，否则 G2 只覆盖前两类）
- `coord_join_rate` 不做全局任意 topic 推断，首版只对配置或 preset 拓扑中声明的 edge 计算，避免误报。
- `emit_cadence` 低样本时不报警；窗口内样本数低于 `min_samples` 只写 insufficient-data。
- wave 场景必须识别 `wave_id`，避免把同一 wave 的并发 worker 顺序误判为 cadence 异常。

**Patterns to follow:**
- `crates/ralph-proto/src/event_bus.rs` 的 `add_observer()` 模式。
- `session_recorder.rs` 的 observer closure 模式。

**Test scenarios:**
- 100 个事件中 95 个带字段，field completeness 为 95%。
- payload 非 JSON 不 panic，记录 parse miss。
- from topic 不存在时 coord join 不报警。
- `ce-executor-wave` 风格事件中 `work.done → review.wave.ready` 关联下降时生成 finding。
- observer 满载时不阻塞 `EventBus::publish()`。
- observer 内部解析失败或 panic-like error 不影响 EventBus recipients。
- 低样本窗口不会生成 hard finding。

**Verification:**
- `rtk cargo test -p ralph-core drift`
- `rtk cargo test -p ralph-core drift_integration`

---

- [ ] U6. **Recovery Responder：Prompt 注入、Retry Window、升级策略**

**Goal:** 将 diagnosis/finding 转化为 soft alert、targeted retry 或 hard pause，并追踪 outcome。

**Requirements:** R3, R7, R8

**Dependencies:** U2, U3, U4, U5

**Files:**
- Create: `crates/ralph-core/src/diagnosis/responder.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`
- Test: `crates/ralph-core/src/event_loop/tests/drift_integration.rs`
- Test: `crates/ralph-cli/src/loop_runner/tests.rs`

**Approach:**
- `RecoveryResponder` 维护 retry key 的窗口状态：attempt count、last iteration、last severity、outcome。
- Soft：在 `build_prompt()` 注入 `## Runtime Diagnosis Alert`，限制 finding 数量和长度。
- Hard：对 safe target 发布 enhanced `task.resume`，target 指向责任 hat。
- Final：retry window 耗尽或 no safe target 时 pause / terminate，并确保诊断报告入口可见。
- 每轮 accepted events 处理后，根据同一 retry key 对应的 topic/field 是否恢复，更新 outcome。
- 不新建与 `LoopState::consecutive_hard_gates`、stale breaker、`LoopThrashing` 平行的终止系统。Responder 只给现有终止检查提供诊断上下文，或复用已有计数。

**Prompt 注入位置：**
- `event_loop/mod.rs` 的 `build_prompt()` 中抽一个统一 helper，例如 `apply_runtime_diagnosis_prompt(prompt, hat_id)`。
- 该 helper 必须在所有 final prompt 返回前调用，覆盖：
  - solo ralph path
  - multi-hat coordinator path
  - isolated hat path
  - backward-compat custom hat path
- 推荐顺序：先 `inject_phase_into_prompt()`，再 runtime diagnosis alert，再 `prepend_auto_inject_skills()`，避免技能索引被诊断文本打断。

**Escalation detail:**

| Level | Action | Preconditions | Regression guard |
|---|---|---|---|
| soft | prompt alert only | finding 出现但未重复达到阈值 | 不 publish 新事件，不改变 termination |
| hard | targeted `task.resume` | safe target 存在，且同 retry_key 重复失败 | target 必须是 registered hat，且不会触发下游业务 hat |
| final | pause/terminate with report hint | no safe target 或 retry window 耗尽 | termination reason 必须可解释，不能吞原 payload violation / stale reason |

**Test scenarios:**
- soft finding 注入 prompt，且不超过配置字符数。
- 同一 retry key 连续失败达到阈值后升级。
- 恢复成功后下一轮不再注入同一 alert。
- no safe target 不发布 targeted resume，只记录 pause/human intervention。
- isolated hat mode 下 alert 只注入目标 hat。
- diagnostics disabled 时 prompt 完全不含 Runtime Diagnosis Alert。
- coordinator path 和 isolated path 的注入行为一致。
- final escalation 不覆盖已有 `PayloadContractViolation` termination reason。

**Verification:**
- `rtk cargo test -p ralph-core drift_integration`
- `rtk cargo test -p ralph-cli loop_runner`

---

- [ ] U7. **`ralph diagnose` 离线诊断报告**

**Goal:** 给 operator 一个简单命令，从 session artifacts 生成问题定位报告。

**Requirements:** R9, R10, R12

**Dependencies:** U3, U4, U5, U6

**Files:**
- Create: `crates/ralph-core/src/diagnosis/reporter.rs`
- Create: `crates/ralph-cli/src/commands/diagnose.rs`
- Modify: `crates/ralph-cli/src/commands/mod.rs`
- Modify: `crates/ralph-cli/src/main.rs`
- Test: `crates/ralph-cli/tests/diagnose.rs`
- Test: `crates/ralph-core/src/diagnosis/tests.rs`

**CLI shape:**

```text
ralph diagnose
ralph diagnose --session latest
ralph diagnose --session .ralph/diagnostics/2026-06-05T10-20-30
ralph diagnose --format json
ralph diagnose --output .ralph/diagnostics/<session>/diagnosis-report.md
```

**Report sections:**
- Run summary：status、iterations、termination reason、session path。
- Top findings：按 severity 和重复次数排序。
- Recovery timeline：每次 recovery、target、outcome。
- Drift findings：field completeness、coord join、emit cadence。
- Preset topology health：哪些 topic 没下游、哪些下游响应率下降。
- Contract health：schema/contract/instructions/read-state 字段不一致线索。
- Suggested next actions：修改 preset、hat instructions、contract、workflow guard 或 runtime config。

**Reporter pipeline:**
1. Resolve session：
   - `latest` 只选择 `.ralph/diagnostics/` 下形如 timestamp 的目录；
   - 忽略 `logs/` 子目录和 root-level `payload-contract-error-*.json` 文件；
   - 指定 path 时允许相对路径。
2. Read inputs：
   - `recovery.jsonl`
   - `drift.jsonl`
   - `orchestration.jsonl`
   - `errors.jsonl`
   - optional `diagnosis-summary.json`
   - optional root-level payload contract violation reports referenced by evidence。
3. Normalize：
   - malformed JSONL 行进入 warnings；
   - 缺文件进入 warnings；
   - 同一 `retry_key` 聚合为一组。
4. Rank：
   - severity 优先；
   - repeated/escalated 优先；
   - terminal failure 优先；
   - 最近 iteration 优先。
5. Render：
   - Markdown 面向人读；
   - JSON 面向工具，字段稳定并带 `schema_version`。

**Output contract:**
- `--format json` 不能输出 markdown heading。
- `--output` 写文件时，stdout 只输出路径或简短摘要。
- 无 session 时 exit code 非 0，并给出 `RALPH_DIAGNOSTICS=1 ralph run ...` 或 telemetry 配置建议。
- 有 session 但缺部分文件时 exit code 为 0，报告中列 warnings。

**Test scenarios:**
- 无参数时选择最近 session。
- 指定 session path 时只读取该 session。
- `latest` 不会误选 `.ralph/diagnostics/logs`。
- 缺少 `recovery.jsonl` 时报告降级显示“无 recovery journal”，不失败。
- malformed JSONL 行被计入 warning，不阻断报告生成。
- `--format json` 输出机器可读结构。
- 无 session 时非 0 退出，并给出重新运行建议。

**Verification:**
- `rtk cargo test -p ralph-cli diagnose`
- `rtk cargo run -p ralph-cli -- diagnose --help`

---

- [ ] U8. **Loop Summary / Termination Integration**

**Goal:** 运行结束时留下可发现的诊断入口，并在异常终止时自动写 summary seed。

**Requirements:** R9, R10

**Dependencies:** U3, U7

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`
- Modify: `crates/ralph-core/src/summary_writer.rs`
- Test: `crates/ralph-cli/src/loop_runner/tests.rs`
- Test: `crates/ralph-core/src/summary_writer.rs`

**Approach:**
- `handle_termination` 写完 `.ralph/agent/summary.md` 后，若 diagnostics enabled，写 `diagnosis-summary.json` seed。
- `.ralph/agent/summary.md` 可追加一行 operator-facing 链接/路径，指向 diagnostics session 和 `ralph diagnose --session ...` 命令。优先由 `loop_runner/runner.rs` 在调用 `SummaryWriter` 时传入可选 hint；只有现有 `SummaryWriter` 接口无法承载该 hint 时，才扩展 `summary_writer.rs`。
- 不把完整诊断报告塞进 agent summary，避免 agent-facing 文件过重。

**Test scenarios:**
- diagnostics enabled + loop termination 后 summary 包含 diagnostics session hint。
- diagnostics disabled 时 summary 不出现无效路径。
- payload contract violation 终止时 diagnosis summary 包含 violation reference。

**Verification:**
- `rtk cargo test -p ralph-core summary_writer`
- `rtk cargo test -p ralph-cli loop_runner`

---

- [ ] U9. **文档、preset 示例和最终验证**

**Goal:** 让用户知道如何开启、运行、查看和解读诊断报告。

**Requirements:** R11, R12

**Dependencies:** U1-U8

**Files:**
- Create: `docs/guide/runtime-diagnosis.md`
- Modify: `AGENTS.md`
- Modify: `CLAUDE.md`
- Optional Modify: `presets/en/ce-executor.yml`
- Optional Modify: `presets/en/ce-executor-wave.yml`

**Approach:**
- 文档说明：
  - `RALPH_DIAGNOSTICS=1 ralph run ...`
  - `ralph diagnose --session latest`
  - recovery/drift/summary artifacts 各自含义
  - 如何用报告定位 preset 问题
- 若修改 `AGENTS.md`，必须同步 `CLAUDE.md`，保持完全一致。
- 只在有实际配置示范价值时修改 preset；不要为了展示而污染默认 preset。

**Test scenarios:**
- `ralph diagnose --help` 展示完整参数。
- 文档中的命令能在本地跑通。
- `AGENTS.md` 与 `CLAUDE.md` 内容保持一致。

**Verification:**
- `rtk cargo run -p ralph-cli -- diagnose --help`
- `rtk diff AGENTS.md CLAUDE.md` 无差异（如果修改两者）
- `./scripts/run-tests.sh` 或 fallback `rtk cargo test --workspace --exclude ralph-e2e -- --test-threads=1 --skip acp_executor::tests::test_create_terminal_and_output`

---

## System-Wide Impact

- **Runtime path：**
  - `EventLoop` 和 `loop_runner` 会在 recovery/gate 分支多写一条结构化诊断记录。
  - `EventBus` 可新增 Drift observer，但 observer 必须 non-blocking。
  - `build_prompt()` 可能新增诊断 alert 注入，必须受配置和长度限制。

- **Diagnostics path：**
  - diagnostics session 新增 `recovery.jsonl`、`drift.jsonl`、`diagnosis-summary.json`、`diagnosis-report.md`。
  - `orchestration.jsonl` 只承载高层审计事件，详细 evidence 进入新 journal。

- **CLI surface：**
  - 新增 `ralph diagnose` 子命令。
  - 不改变 `ralph run`、`ralph emit`、`ralph tools` 的既有命令语义。

- **Compatibility：**
  - `task.resume` topic 不变。
  - payload 增强是 additive 文本变化。
  - diagnostics disabled 时现有运行路径应尽量保持无行为差异。

- **Operational posture：**
  - 诊断报告成为 preset 调试的首选入口。
  - 对 hard gate / contract / workflow guard 的重复失败，从“看日志猜”变为“报告直接列出 retry key 和失败次数”。

---

## Regression Safety & Test Strategy

这个计划的硬约束是：**不能因为诊断能力引入编排回归**。实现顺序必须先 characterization，再扩展行为；每个 gate 都要证明原有路径不变。

### 回归安全原则

1. **默认 no-op。** diagnostics disabled 或 `telemetry.runtime_diagnosis.enabled: false` 时，现有 `ralph run` 行为应保持不变。
2. **先记录，后干预。** G1 只写 journal 和报告，不改变 active hat 选择、事件路由、termination reason。
3. **新增诊断不吞错误。** 诊断写文件失败只能记录 warning，不能让原本成功的 loop 失败。
4. **恢复 payload 是 additive。** `task.resume` topic、target routing、EventBus 语义不变，只追加结构化说明。
5. **Drift 初版只 soft signal。** Drift 不直接拦业务事件；硬干预必须经过 retry window 和 safe target 判断。

### 必须覆盖的测试矩阵

| 区域 | 必须新增/强化的测试 | 防止的回归 |
|---|---|---|
| Config | telemetry 默认关闭、显式开启、非法阈值、非法窗口 | 默认配置破坏现有 preset |
| Diagnostics | enabled 创建新文件，disabled no-op，JSONL 每行合法，写入失败不影响 run | 诊断 I/O 影响编排主路径 |
| Missing-event hard gate | 没 emit 时仍 gate；写 recovery journal；active hat 不变 | 假成功回归、hat 选择漂移 |
| Execution contract recovery | rejected event 不进 bus；targeted retry 仍回源 hat；no safe target fail closed | 未验证事件触发下游、恢复路由错 |
| Workflow guard | out-of-order 仍拒绝；diagnosis 记录不改变 phase progress | workflow 顺序保护失效 |
| Payload contract violation | violation report 仍写；termination reason 不变；新增 envelope 只补充证据 | 硬失败被软化或吞掉 |
| Prompt 注入 | diagnostics disabled 无注入；enabled 注入长度受限；coordinator/isolated path 都覆盖 | prompt 膨胀、漏注入、错 hat 注入 |
| Drift | 非 JSON payload 不 panic；低样本不误报；coord join fixture 稳定 | drift 误报打断正常 run |
| Diagnose CLI | latest session、指定 session、缺文件降级、malformed JSONL warning、JSON/Markdown 输出 | 报告工具脆弱不可用 |
| End-to-end | `ce-executor`、`ce-executor-wave` 至少各一条 fixture/集成测试 | preset 级真实链路回归 |

### Gate 级验证要求

- **G1 完成前**：只能新增记录和报告；必须证明 existing hard gate、contract rejection、payload violation 的旧测试仍通过。
- **G2 完成前**：drift finding 必须只进入报告和 soft alert，不允许默认改变 termination。
- **G3 完成前**：retry escalation 必须有重复失败、恢复成功、no safe target 三类集成测试。

### Characterization Tests（改行为前先补）

实施者在修改对应生产代码前，必须先补或确认这些现有行为测试：

| Before touching | Characterization must prove |
|---|---|
| `DiagnosticsCollector` 初始化 | 默认 disabled 不创建 session；`RALPH_DIAGNOSTICS=1` 创建现有文件；同一 run 不分裂 session |
| `EventLoop::build_prompt()` | solo、coordinator、isolated、legacy custom hat 四条路径当前 prompt 内容和顺序可预测 |
| `inject_fallback_event()` | 有 last active hat 时 target 该 hat；无 last active hat 时 fallback 到 ralph；返回值保持现状 |
| `inject_missing_event_hard_gate_guidance()` | guidance 写入当前 events file；不会触发 default_publishes；hard gate count 增加 |
| `handle_execution_contract_rejections()` | rejected event 不触发下游；safe target 时 targeted `task.resume` 存在；no safe target 时不伪造 target |
| `apply_workflow_guard_validation()` | rejected out-of-order event 不进入 accepted events；workflow progress 不 advance；recovery `task.resume` 仍发布 |
| `write_payload_contract_violation_report()` | violation file 仍写到当前兼容位置；stderr/report summary 仍可见；termination reason 不变 |
| `EventBus::publish()` observer | observer 在 routing 前被调用；unknown source event 被 drop 且 observer 不看到 |
| `SummaryWriter` | 默认 summary 内容不变；新增 hint 前后旧字段仍存在 |

最终合并前必须跑项目标准测试：优先 `./scripts/run-tests.sh`；如果 nextest 不可用，使用 AGENTS.md 指定的 serial fallback。

---

## Risks & Mitigations

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| 诊断层写入过多导致 I/O 噪音 | 中 | 中 | JSONL 只写结构化摘要，长 evidence 截断或引用文件路径 |
| Prompt 注入导致 token 膨胀 | 中 | 中 | `max_prompt_findings` + `max_prompt_chars` 双限制 |
| Drift 误报造成不必要 retry | 中 | 中 | Drift 初版默认 soft signal，硬干预必须经过 responder 和重复窗口 |
| Recovery journal 与 orchestration 重复 | 中 | 低 | orchestration 写高层事件，recovery 写细节和 outcome |
| 多线程 Drift observer 阻塞 EventBus | 低 | 高 | observer 只 bounded try-send；计算与 publish 解耦 |
| `ralph diagnose` 对旧 session 兼容差 | 中 | 低 | 缺文件降级，报告 warning，不失败 |
| 修改多个 recovery 分支造成回归 | 中 | 高 | 每个分支先补 characterization test，再接 envelope |

---

## Documentation / Operational Notes

- 新增 `docs/guide/runtime-diagnosis.md`，面向 operator 和 preset 作者。
- `AGENTS.md` / `CLAUDE.md` 需要补充 diagnostics 和 `ralph diagnose` 用法；修改时必须同步。
- 计划实施时如果改动 `ralph tools` 子命令或 `crates/ralph-core/data/*.md` 引用，必须遵守项目的反向验证规则；本计划本身不要求改 `ralph tools`。
- `ralph diagnose` 是用户可见 CLI，新增后需要跑 `ralph diagnose --help` 冒烟。

---

## Acceptance Criteria

- [ ] 同一次 `ralph run` 只有一个 authoritative diagnostics session，`ralph diagnose latest` 不会读到半套数据。
- [ ] 默认配置下 runtime diagnosis 完全 no-op，不创建新 session、不改变 prompt、不改变 termination。
- [ ] `RALPH_DIAGNOSTICS=1` 与 `telemetry.runtime_diagnosis.write_artifacts=true` 的 activation 行为符合 Activation Matrix。
- [ ] 所有主要 recovery/gate 路径都会写 `RecoveryDiagnosisEnvelope`。
- [ ] `recovery.jsonl` 能展示每次恢复的 source、target、reason、expected action、outcome。
- [ ] `drift.jsonl` 能展示 field completeness、coord join、emit cadence 的 finding。
- [ ] 重复同类恢复失败会升级，不会无限盲目 `task.resume`。
- [ ] `ralph diagnose --session latest` 能生成 Markdown 报告。
- [ ] 报告能指出至少三类问题：missing emit、contract rejection、drift/topology degradation。
- [ ] rejected event 不会因为 drift observer 或 recovery journal 重新进入 EventBus。
- [ ] workflow guard、execution contract、payload contract、missing-event hard gate 的现有 characterization tests 通过。
- [ ] `ce-executor` / `ce-executor-wave` 至少各有一条相关测试或 fixture 覆盖。
- [ ] 文档说明完整，`AGENTS.md` 与 `CLAUDE.md` 如被修改则保持一致。
- [ ] `./scripts/run-tests.sh` 或项目指定 fallback 测试通过。

---

## Sources & References

- `crates/ralph-core/src/event_loop/mod.rs`：prompt 构建、workflow guard、fallback recovery、event processing。
- `crates/ralph-cli/src/loop_runner/runner.rs`：主循环、contract rejection 处理、payload violation 终止、missing-event gate、termination summary。
- `crates/ralph-cli/src/loop_runner/hard_gate.rs`：missing-event gate、execution contract recovery diagnostics。
- `crates/ralph-core/src/diagnostics/`：diagnostics collector 与 JSONL logger 模式。
- `crates/ralph-core/src/summary_writer.rs`：loop termination summary 模式。
- `crates/ralph-proto/src/event_bus.rs`：observer 模式。
- `docs/solutions/developer-experience/agent-execution-contract-gates-2026-06-03.md`：execution contract / hard gate 的 institutional learning。
- `docs/plans/2026-06-04-001-fix-contract-rejection-hat-retry-plan.md`：targeted retry 和 no-safe-target 设计背景。
- `docs/plans/2026-06-04-004-feat-ce-executor-wave-preset-plan.md`：wave preset 下 coord join / topology health 的测试背景。
