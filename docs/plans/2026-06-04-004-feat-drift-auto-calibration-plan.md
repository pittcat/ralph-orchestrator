---
title: "Drift Auto-Calibration: 运行时漂移监控与自校准免疫系统"
type: feat
status: active
date: 2026-06-04
---

# Drift Auto-Calibration: 运行时漂移监控与自校准免疫系统

## Overview

为 Ralph 编排器引入运行时漂移监控（Drift Monitoring），将其从"独立报警器"转变为**现有治愈体系的诊断增强层**。Drift Monitor 通过滑动窗口统计事件模式异常，以"软干预（prompt 注入精确诊断）→ 硬干预（Retry Window）→ 最终干预（Pause）"的分级策略，与现有四层自愈能力协同工作，实现**自校准闭环**。

> 核心设计哲学：Drift 不是第 5 层 Pause，而是治愈体系的"放射科"——拍片子，告诉医生病灶在哪。

---

## Problem Frame

### 现有治愈体系的结构性盲区

Ralph 当前拥有四层治愈能力：

| 层级 | 触发条件 | 治愈方式 | 核心问题 |
|---|---|---|---|
| **Stall Recovery** | Hat 完全没发事件 | `task.resume` 通用提醒 | 不知道"为什么"没发，盲打 |
| **Policy Reject** | 格式/字段明显非法 | `task.resume` + 错误说明 | 只抓"已知的坏"，漏"勉强合法" |
| **Contract Recovery** | 证据缺失（git/test） | Targeted retry 源 hat | 只检查"有没有"，不检查"好不好" |
| **Hook Retry** | 外部命令失败 | 指数退避重试 | 跟 LLM 行为无关 |

**这三大盲区现有机制完全无法覆盖：**

**盲区 1："合法但异常"——最致命**

```
Builder 发 work.done：
  之前：{"task_id":"T-1", "plan_name":"fix-auth", "test_evidence":"..."}
  现在：{"task_id":"T-1"}
```
- Policy：`plan_name` 不是 required → 不拦截
- Contract：`task_id` 有了 → 不拒绝
- Stall：发了事件 → 不触发
- **结果：没有任何治愈触发，但下游 Reviewer 已经不能正常工作了**

**盲区 2：治愈无效循环**

```
Builder 缺 plan_name → task.resume → Builder 再发 → 仍然缺 plan_name → 再 resume...
```

现有 `task.resume` 的 payload 是通用提醒："RECOVERY: Previous iteration did not publish an event..."。**Builder 收到后不知道具体该改什么**，只能凭运气重试，陷入"resume 震荡"——每次 resume 消耗 iteration，max_iterations 被浪费在无效重试上，最终超时或放弃。

**盲区 3：渐进式关联断裂**

`work.done` 后 `review.wave.ready` 的响应率从 100% → 60% → 30%。每个事件各自合法，没有任何 rejection，但**编排拓扑已经坏了**。

### 为什么需要"诊断增强"而非"新增治愈"

现有机制是**"症状治疗"**：
- Stall Recovery ≈ "你没吃药，记得吃药"（不知道什么病）
- Policy Reject ≈ "这个药吃错了，重新吃"（知道错了但不说为什么）
- Contract Recovery ≈ "检查报告不合格，重做"（知道哪项不合格）

**缺少的是：诊断报告。**

Drift 监控的价值在于提供**精确的、统计性的、跨事件的诊断信息**，让治愈从"盲打"变成"精准手术"。

---

## Requirements Trace

- **R1.** 监控 `field_completeness`：统计每个 (topic, field) 的出现率，覆盖 required fields 和 optional fields
- **R2.** 监控 `coord_join_rate`：统计 (from_topic, to_topic) 的关联率，发现拓扑断裂
- **R3.** 监控 `emit_cadence`：统计每个 topic 的发射频率/间隔异常
- **R4.** 软干预（🟡）：在 build_prompt 时自动注入 `## Drift Alert` 段，告知 LLM 具体漂移项
- **R5.** 硬干预（🟠）：3 轮软干预无效后，发布增强版 `task.resume`（附精确诊断），进入 Retry Window
- **R6.** 最终干预（🔴）：Retry Window 失败后 Pause，产出诊断报告
- **R7.** 自愈效果追踪：记录每次干预后的指标变化，形成闭环
- **R8.** YAML 配置：`telemetry:` 段控制开关、阈值、窗口大小
- **R9.** 产出物：`.ralph/metrics/drift.jsonl` + 整合进 diagnostics `orchestration.jsonl`

---

## Scope Boundaries

- **不做** OpenTelemetry 协议输出（后续迭代）
- **不做** Grafana/Prometheus 集成（后续迭代）
- **不做** Python 离线分析器（后续迭代）
- **不做** 独立项目/独立仓库——深度嵌入 ralph-core
- **不做** 模型级 drift（prompt 语义漂移）——只监控事件级/字段级 drift
- **不改** 现有四层治愈的核心逻辑——只增强其 payload 和决策依据

### Deferred to Follow-Up Work

- OTel/Prometheus 输出协议：`telemetry.exporter` 配置段
- Python 离线分析器：读 drift.jsonl 做自适应阈值、因果推理
- Schema 版本化：drift.jsonl 的 `schema_version` 字段治理

---

## Context & Research

### Repo Drift Note（2026-06-05 追加）

自本计划 6-04 写成后，仓库发生 2 个相关物理目录重构。本计划 Implementation Units 中所有 `crates/ralph-core/src/config.rs` 与精确行号引用须按下表替换再执行；其它路径（`event_loop/mod.rs`、`event_bus.rs`、`diagnostics/`、`event_logger.rs`、`session_recorder.rs`）位置仍然准确：

| 原路径 | 拆分后实际位置 | 备注 |
|---|---|---|
| `crates/ralph-core/src/config.rs` | `crates/ralph-core/src/config/` 21 个子模块 | `HatConfig` 在 `config/hat.rs:88`；`MemoriesConfig` 在 `config/memories.rs:46`（取代原计划 `config.rs:2303-2334` 的引用）。所有"在 config.rs 第 NN 行"的引用作废。 |
| `crates/ralph-core/src/event_loop/tests.rs` | `crates/ralph-core/src/event_loop/tests/` 29 个子文件 + `event_loop/tests/mod.rs` | U5 集成测试可放在 `event_loop/tests/execution_contract.rs` 或新建 `event_loop/tests/drift_integration.rs`；不建议再追加到 `event_loop/tests.rs`（已不存在）。 |
| `crates/ralph-cli/src/loop_runner.rs` | `crates/ralph-cli/src/loop_runner/` 18 个子模块 | 主循环在 `loop_runner/runner.rs`（122 KB）。U5 中"每轮 iteration 结束前"的钩子点在 `loop_runner/runner.rs` 或 `loop_runner/exit_conditions.rs` 中按需选择。 |

另：`presets/en/ce-executor-wave.yml` 是 21e8f47 提交新增的 wave preset，本计划 U7 集成测试可补一条"ce-executor-wave 编排下 coord_join_rate 监测"以验证 wave 拓扑的 drift 监控。

### Relevant Code and Patterns

- **EventBus observer**：`crates/ralph-proto/src/event_bus.rs` — `add_observer()` 接收所有事件，sync closure，适合轻量入队
- **Diagnostics 系统**：`crates/ralph-core/src/diagnostics/` — 按 session 组织的 JSONL 子 loggers，结构化 enum
- **Config 扩展模式**：`crates/ralph-core/src/config.rs` — 新建 struct → `#[serde(default)]` → `RalphConfig` 字段 → `Default` 实现 → `validate()` → tests
- **Prompt 注入点**：`crates/ralph-core/src/event_loop/mod.rs` — `inject_phase_into_prompt()`（`:2107`）、`prepend_auto_inject_skills()` 展示注入模式。U5 注入点推荐位置：`build_prompt()` 在 `:1715` 入口，调用 `inject_drift_alert()` 后再走现有 phase/skill 注入流。
- **Stall Recovery**：`crates/ralph-core/src/event_loop/mod.rs:1655` — `inject_fallback_event()` 展示 recovery event 发布模式
- **Targeted Recovery**：`crates/ralph-core/src/event_loop/mod.rs:3300-3800 区间`（contract_rejection 相关处理散布于此：例如 `contract_rejections` 字段在 `:61`、`:3306`，rejection 处理逻辑在 `:3598-3767`）— Contract rejection 后精准 retry 源 hat。**取代原计划的"~3698"近似行号**——文件已增至 204 KB，单一行号不再稳定。

### Institutional Learnings

- 调研报告确认 Gap 1（运行时 drift 监控）为 P0
- 现有 diagnostics 是"现场录像"（orchestration/performance/errors.jsonl），drift 是"体检报告"（跨事件聚合统计），两者互补
- EventBus observer 是 sync blocking vec，**不能在里面做重计算**——必须用 channel 解耦

---

## Key Technical Decisions

1. **Observer 只做轻量入队，计算在独立线程**：EventBus observer 推入 `crossbeam-channel`，`DriftMonitor` 在独立线程消费并计算。避免阻塞事件路由热路径。

2. **Prompt 注入而非新增 task.resume（轻度）**：软干预不新增 Pause/Resume，而是在 `build_prompt()` 中类似 `inject_phase_into_prompt()` 注入 `## Drift Alert`。这是最小侵入、最有效的纠正方式——直接告诉 LLM"你哪里偏了"。

3. **Retry Window 复用现有 task.resume（中度）**：不新增事件类型，而是在现有 `task.resume` 的 payload 中附加 drift 诊断信息。保持与现有治愈体系的事件协议兼容。

4. **指标计算用纯 Rust，算法后续切 Python**：MVP 用 Rust 做固定阈值（80%）+ 滑动窗口。自适应阈值（IsolationForest、Prophet）留给 Python 离线分析器后续接入。

5. **窗口大小和阈值可配置**：默认窗口 100 事件，field_completeness 阈值 80%，coord_join_rate 阈值 70%，emit_cadence 用 2σ 规则。

---

## Output Structure

```
crates/ralph-core/src/
├── drift/
│   ├── mod.rs              # DriftMonitor 主结构，初始化/启动/停止
│   ├── window.rs           # DriftWindow — 环形滑动窗口
│   ├── detector.rs         # DriftDetector — 三种指标计算
│   ├── responder.rs        # DriftResponder — 分级响应逻辑
│   ├── alert.rs            # DriftAlert — Alert 结构体 + prompt 格式化
│   ├── metrics.rs          # DriftMetrics — 指标序列化（drift.jsonl）
│   └── tests.rs            # 单元测试
├── config.rs               # 新增 TelemetryConfig / DriftConfig
├── event_loop/mod.rs       # 接入 observer、prompt 注入、retry window
└── diagnostics/
    └── mod.rs              # 新增 log_drift() 方法
```

---

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

```
┌─────────────────────────────────────────────────────────────────┐
│                        EventBus.publish()                        │
└─────────────────────────────┬───────────────────────────────────┘
                              │
        ┌─────────────────────┼─────────────────────┐
        ▼                     ▼                     ▼
  ┌──────────┐        ┌──────────────┐       ┌──────────┐
  │ Session  │        │ DriftObserver│       │   TUI    │
  │ Recorder │        │ (sync,轻量)   │       │ Observer │
  └──────────┘        └──────┬───────┘       └──────────┘
                             │ try_send(event)
                             ▼
                     ┌───────────────┐
                     │ crossbeam::   │
                     │ channel       │
                     └───────┬───────┘
                             │ recv()
                             ▼
                     ┌───────────────┐
                     │ DriftMonitor  │
                     │ (独立线程)     │
                     │               │
                     │ ┌───────────┐ │
                     │ │DriftWindow│ │ ◄── 滑动窗口 (默认100)
                     │ └─────┬─────┘ │
                     │       ▼       │
                     │ ┌───────────┐ │
                     │ │DriftDetector│ ◄── field / coord / cadence
                     │ └─────┬─────┘ │
                     │       ▼       │
                     │ ┌───────────┐ │
                     │ │DriftResponder│ ◄── 分级响应决策
                     │ └─────┬─────┘ │
                     └───────┼───────┘
                             │
              ┌──────────────┼──────────────┐
              ▼              ▼              ▼
        ┌──────────┐  ┌──────────┐  ┌──────────┐
        │ 软干预    │  │ 硬干预    │  │ 最终干预  │
        │(prompt   │  │(enhanced │  │(Pause +  │
        │ 注入)     │  │ resume)  │  │ diagnostic)
        └────┬─────┘  └────┬─────┘  └────┬─────┘
             │             │             │
             ▼             ▼             ▼
        build_prompt   EventBus      EventLoop
        + drift_alert  .publish      .pause()
```

**自愈闭环：**

```
Round N:   Detector 发现漂移 → Responder 决策为"轻度"
           → EventLoop::build_prompt() 注入 drift_alert
           
Round N+1: Detector 检查同一 hat/topic 的指标
           ├─ 回升 → 标记"自愈成功"，关闭 alert
           └─ 未变 → 计数器 +1

Round N+3: 计数器达到 3 → Responder 升级为"中度"
           → 发布增强 task.resume（附诊断）
           → 进入 Retry Window（3 轮观察）
           
Round N+6: Retry Window 内恢复 → 标记"自愈成功"
           Retry Window 失败 → 升级为"重度" → Pause
```

---

## Implementation Units

- [ ] U1. **TelemetryConfig / DriftConfig（配置层）**

**Goal:** 在 `RalphConfig` 中新增 `telemetry:` 配置段，控制 drift 监控的开关、阈值、窗口大小。

**Requirements:** R8

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/config/hat.rs`（取代已拆分的 `config.rs`；参见 Repo Drift Note；`HatConfig` 在 `:88`）
- Modify: `crates/ralph-core/src/config/memories.rs`（参考 `MemoriesConfig` 模式，在 `:46`；取代原计划引用的 `config.rs:2303-2334`）
- Test: `crates/ralph-core/src/config/hat.rs`（在子文件测试模块追加，取代原"config tests 底部追加"）

**Approach:**
- 新建 `TelemetryConfig` struct，包含 `enabled: bool`、`drift: DriftConfig`
- `DriftConfig` 包含：`window_size`、`field_completeness_threshold`、`coord_join_rate_threshold`、`retry_window_iterations`、`enabled_metrics: Vec<String>`
- 所有字段 `#[serde(default)]`，`Default` 显式实现
- 在 `RalphConfig::validate()` 中检查阈值合法性（0.0-1.0）
- 参考 `MemoriesConfig` / `FeaturesConfig` 的已有模式

**Patterns to follow:**
- `MemoriesConfig`（`config/memories.rs:46`，**取代原计划引用的 `config.rs:2303-2334`**；参见 Repo Drift Note）的字段定义模式
- `RalphConfig` 中 `#[serde(default)]` 的接入方式
- 底部 `#[cfg(test)]` 模块的测试模式

**Test scenarios:**
- Happy path: YAML 中 `telemetry.enabled: true` 正确解析
- Happy path: 省略 `telemetry` 段时全部走默认值
- Edge case: `field_completeness_threshold > 1.0` 时 validate 报错
- Edge case: `window_size: 0` 时 validate 报错或自动修正为最小值

**Verification:**
- `cargo test -p ralph-core config::tests` 通过
- 新测试覆盖 DriftConfig 的解析、默认值、验证

---

- [ ] U2. **DriftWindow + DriftDetector（核心采集与统计）**

**Goal:** 实现滑动窗口存储和三种指标计算。

**Requirements:** R1, R2, R3

**Dependencies:** U1

**Files:**
- Create: `crates/ralph-core/src/drift/window.rs`
- Create: `crates/ralph-core/src/drift/detector.rs`
- Create: `crates/ralph-core/src/drift/metrics.rs`

**Approach:**
- `DriftWindow`：固定容量环形缓冲区（`VecDeque<EventSnapshot>`），容量来自 `DriftConfig.window_size`
- `EventSnapshot`：轻量快照（topic、source_hat、payload 字段集合、timestamp、iteration）
- `DriftDetector`：消费窗口数据，计算：
  - `field_completeness(topic, field)` = 该 topic 的事件中带此字段的比例
  - `coord_join_rate(from_topic, to_topic)` = from_topic 后 N 轮内出现 to_topic 的比例
  - `emit_cadence(topic)` = 最近 N 轮的平均间隔 + 方差
- 指标结果输出为 `DriftFinding` 列表

**Technical design:**
- `field_completeness`：解析 payload JSON，检查字段存在性。用 `serde_json::Value` 做轻量解析。
- `coord_join_rate`：遍历窗口，找 from_topic 事件，检查后续 M 个事件内是否有 to_topic。M 可配置（默认 5）。
- `emit_cadence`：记录同一 topic 的 timestamp 差，计算均值和标准差。超出 2σ 视为异常。

**Patterns to follow:**
- `EventRecord`（`event_logger.rs`）的结构化记录模式
- `DiagnosticsCollector` 的"enabled gate"模式

**Test scenarios:**
- Happy path: 100 个事件窗口，计算 field_completeness 为 95%
- Edge case: 窗口未满时的计算（应基于实际数量而非容量）
- Edge case: payload 不是 JSON（String payload）时字段检测不 panic
- Edge case: coord_join_rate 中 from_topic 从未出现 → 返回 None（不报警）
- Error path: 非法 JSON payload → 记为 parse_error，不影响其他指标

**Verification:**
- `cargo test -p ralph-core drift::` 通过
- 测试覆盖三种指标的计算正确性和边界行为

---

- [ ] U3. **DriftResponder + DriftAlert（分级响应）**

**Goal:** 根据指标异常程度决定响应级别，生成格式化的 alert 文本。

**Requirements:** R4, R5, R6, R7

**Dependencies:** U2

**Files:**
- Create: `crates/ralph-core/src/drift/responder.rs`
- Create: `crates/ralph-core/src/drift/alert.rs`

**Approach:**
- `DriftResponder::respond(findings)` → 返回 `Vec<DriftResponse>`
- 响应级别决策：
  - 轻度：任何 finding 的当前值 > threshold × 0.8（即接近阈值但未跌破）
  - 中度：finding 值 < threshold，且该 (topic, field) 未被标记为 retry_window
  - 重度：finding 值 < threshold，且 retry_window 已耗尽（超过配置轮数）
- `DriftAlert`：将 findings 格式化为 Markdown 文本，用于 prompt 注入
- `RetryWindowTracker`：记录每个 (hat, topic, field) 的 retry 状态（当前轮数、干预历史）

**Technical design:**
```markdown
## Drift Alert（自动注入）

最近 {window_size} 轮监测到以下异常：

- `work.done.plan_name` 出现率：{current}%（预期 ≥ {threshold}%）
- `work.done → review.wave.ready` 关联率：{current}%（预期 ≥ {threshold}%）

请检查输出格式，确保包含 plan_name 并触发下游 review。
```

**Patterns to follow:**
- `ExecutionContractFinding` 的结构化诊断模式
- `OrchestrationEvent::ContractRecoveryRouted` 的审计记录模式

**Test scenarios:**
- Happy path: 95% → 轻度响应 → DriftAlert 生成正确
- Happy path: 70% → 中度响应 → RetryWindow 开启
- Happy path: 70% 持续 3 轮 → 重度响应
- Edge case: 同一 finding 在 retry_window 期间改善 → 自愈成功，关闭 window
- Edge case: 多个 finding 同时存在 → alert 合并为一条

**Verification:**
- 测试覆盖三种级别的决策逻辑和状态转换
- RetryWindowTracker 的轮数计数正确

---

- [ ] U4. **DriftMonitor（整合层 + EventBus Observer）**

**Goal:** 将 Window、Detector、Responder 整合为 DriftMonitor，以 observer 形式挂载到 EventBus。

**Requirements:** R1-R7

**Dependencies:** U2, U3

**Files:**
- Create: `crates/ralph-core/src/drift/mod.rs`

**Approach:**
- `DriftMonitor` 持有 `DriftWindow`、`DriftDetector`、`DriftResponder`
- `DriftMonitor::observe(event)`：sync 方法，只做 `try_send` 入 channel
- `DriftMonitor::run()`：独立线程（或 tokio task）的循环，消费 channel 计算指标
- `DriftMonitor::take_alerts_for_hat(hat_id)`：EventLoop 在 build_prompt 前调用，取该 hat 的 pending alerts
- `DriftMonitor::take_responses()`：EventLoop 每轮结束后调用，取需要发布的硬干预事件

**Technical design:**
- Channel 用 `crossbeam::channel::bounded(1024)`，满了丢弃最旧（`try_send` 失败不阻塞）
- 独立线程用 `std::thread::spawn`，因为 drift 计算是 CPU-bound（JSON 解析 + 统计）
- `DriftMonitor` 实现 `Drop`，确保线程正确 join 或标记停止

**Patterns to follow:**
- `SessionRecorder::create_observer()`（`session_recorder.rs:208`）的 observer 闭包模式
- `DiagnosticsCollector` 的"可选启停"模式

**Test scenarios:**
- Integration: 发布 10 个事件 → observer 接收 → 窗口有 10 条 → 指标计算正确
- Integration: channel 满时 → `try_send` 失败 → 不 panic、不阻塞
- Integration: `Drop` DriftMonitor → 线程优雅停止
- Integration: `take_alerts_for_hat` 返回正确 hat 的 alert，消费后清空

**Verification:**
- 测试覆盖 observer 端到端流程
- 多线程安全性验证

---

- [ ] U5. **EventLoop 集成（Prompt 注入 + Retry Window + Pause）**

**Goal:** 在 EventLoop 中接入 DriftMonitor，实现三级干预的实际生效。

**Requirements:** R4, R5, R6, R7

**Dependencies:** U4

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/event_loop/tests/`（或现有测试追加）

**Approach:**
1. **初始化**：`EventLoop::new()` 中根据 `config.telemetry.enabled` 创建 `DriftMonitor`，挂载为 EventBus observer
2. **软干预**：`build_prompt()` 中，在 `inject_phase_into_prompt()` 之后、prepend skills 之前，调用 `drift_monitor.take_alerts_for_hat(hat_id)`，将 alerts 注入 prompt
3. **硬干预**：每轮 iteration 结束前（`process_output` 末尾），调用 `drift_monitor.take_responses()`，如果有中度响应，发布增强版 `task.resume`
4. **最终干预**：如果有重度响应，调用现有 Pause 机制
5. **效果追踪**：每次干预后，记录干预类型 + 指标值到 `DriftWindow` 的元数据中

**Technical design:**
- Prompt 注入位置：`build_prompt()` 中 `let with_drift = self.inject_drift_alert(base_prompt);`
- 增强 resume payload：在现有 recovery payload 后追加 `\n\nDrift Diagnosis: ...`
- Pause 集成：复用现有 `self.state.should_pause = true` 或等效机制

**Patterns to follow:**
- `inject_phase_into_prompt()`（`event_loop/mod.rs:2107`）的注入位置和格式
- `inject_fallback_event()`（`event_loop/mod.rs:1655`）的 recovery event 发布
- `log_orchestration` 的 diagnostics 记录模式

**Test scenarios:**
- Integration: DriftMonitor 发现轻度漂移 → build_prompt 输出包含 `## Drift Alert`
- Integration: 3 轮无效后 → EventLoop 发布带诊断的 task.resume
- Integration: Retry Window 失败 → EventLoop 进入 Pause
- Integration: 软干预后指标恢复 → 下轮 prompt 不再注入 alert
- Edge case: telemetry.disabled → DriftMonitor 为 None，build_prompt 无任何变化

**Verification:**
- 现有 EventLoop tests 不 regression
- 新增 integration test 验证三级干预流程

---

- [ ] U6. **Diagnostics 整合（drift.jsonl + orchestration 事件）**

**Goal:** 将 drift 发现物和干预决策记录到 diagnostics 系统。

**Requirements:** R9

**Dependencies:** U4, U5

**Files:**
- Modify: `crates/ralph-core/src/diagnostics/mod.rs`
- Create: `crates/ralph-core/src/diagnostics/drift.rs`（新 logger）
- Modify: `crates/ralph-core/src/drift/metrics.rs`

**Approach:**
1. **DiagnosticsCollector 扩展**：新增 `drift_logger: Option<Arc<Mutex<DriftLogger>>>`
2. **DriftLogger**：写 `drift.jsonl`（与 orchestration/performance/errors 同级）
3. **日志格式**：每条记录包含 `timestamp`、`iteration`、`finding`（topic/field/current/threshold）、`response_level`、`hat_id`
4. **OrchestrationEvent 扩展**：新增 `DriftDetected`、`DriftIntervened`（Soft/Hard/Final）variants
5. **Metrics 文件**：即使 diagnostics 关闭，也写 `.ralph/metrics/drift.jsonl`（这是 runtime 状态，不是诊断）

**Technical design:**
- `drift.jsonl` 目录：`.ralph/metrics/drift.jsonl`（独立于 diagnostics session）
- 文件写方式：参考 `EventLogger::log()` 的单条 `write_all` 原子追加
- `OrchestrationEvent::DriftDetected`：`{ topic, field, current_rate, threshold, window_size }`
- `OrchestrationEvent::DriftIntervened`：`{ level, hat, findings, outcome }`

**Patterns to follow:**
- `OrchestrationLogger`（`orchestration.rs`）的 tagged enum 模式
- `EventLogger`（`event_logger.rs`）的 JSONL 追加模式
- `DiagnosticsCollector::new()` 的子 logger 初始化模式

**Test scenarios:**
- Happy path: drift detected → drift.jsonl 有对应记录
- Happy path: soft intervention → orchestration.jsonl 有 `DriftIntervened`
- Integration: diagnostics disabled → drift.jsonl 仍然写（因为 metrics 独立于 diagnostics）
- Edge case: 大量 drift 记录 → 文件正确追加，无截断

**Verification:**
- 测试验证 drift.jsonl 和 orchestration.jsonl 的记录格式
- 文件追加的原子性（POSIX O_APPEND）

---

- [ ] U7. **Smoke / Integration 测试**

**Goal:** 验证 drift 自校准闭环的端到端行为。

**Requirements:** R1-R9

**Dependencies:** U1-U6

**Files:**
- Create: `crates/ralph-core/tests/drift_integration.rs`
- Create: `crates/ralph-core/src/drift/tests.rs`

**Approach:**
- 使用 mock EventBus + 预置事件序列，验证 DriftMonitor 的完整流程
- 测试"事件序列 → 指标计算 → 响应决策 → prompt 注入"的闭环
- 测试 Retry Window 的轮数计数和升级逻辑

**Test scenarios:**
- Integration（Happy）: 50 个正常事件 → 无 alert
- Integration（Happy）: 40 个正常 + 10 个缺字段 → 轻度 alert 注入 prompt
- Integration（Happy）: 连续 30 个缺字段 → 中度 alert → 增强 resume
- Integration（Edge）: 中度后 3 轮恢复 → 自愈成功，不再 resume
- Integration（Error）: 中度后 3 轮未恢复 → 重度 → Pause
- Integration（Edge）: coord_join_rate 断裂 → 检测正确，alert 包含关联信息

**Verification:**
- `cargo test -p ralph-core drift_integration` 全部通过
- 测试覆盖 R1-R9 的所有核心路径

---

## System-Wide Impact

- **Interaction graph：**
  - `EventBus::publish()` → DriftObserver（新增 observer，与 SessionRecorder、TUI 并列）
  - `EventLoop::build_prompt()` → `inject_drift_alert()`（新增注入点，在 phase/skill 之后）
  - `EventLoop::process_output()` → `drift_monitor.take_responses()`（每轮末尾检查硬干预）
  - `DiagnosticsCollector` → `DriftLogger`（新增子 logger）

- **Error propagation：**
  - DriftMonitor 内部错误（channel 满、JSON 解析失败）→ 记录 error，**不阻断**事件循环
  - Observer panic → 被 `std::panic::catch_unwind` 捕获（或确保 observer 内部无 panic），不破坏 EventBus

- **State lifecycle risks：**
  - DriftWindow 是内存状态，loop 重启后丢失。这是设计意图（drift 监控是运行时状态，不是持久状态）
  - RetryWindowTracker 也是内存状态，loop 重启后重置

- **API surface parity：**
  - `RalphConfig` 新增 `telemetry` 段，preset YAML 可以配置
  - 不改现有 CLI 参数

- **Unchanged invariants：**
  - 现有四层治愈的核心逻辑完全不变
  - EventBus 的路由逻辑不变
  - `task.resume` 的事件协议不变（只是 payload 增强）
  - Pause 触发条件不变（只是新增一种触发源）

---

## Risks & Dependencies

| Risk | Likelihood | Impact | Mitigation |
|---|---|---|---|
| Observer 性能瓶颈（大量事件时阻塞 publish） | 中 | 高 | Observer 只做 `try_send`，计算在独立线程；channel bounded 且满时丢弃 |
| Prompt 注入导致 token 膨胀 | 中 | 中 | DriftAlert 限制长度（最多 3 条 finding，总长度 ≤ 500 字符） |
| 误报（正常波动被判定为 drift） | 中 | 中 | 窗口大小和阈值可配置；MVP 用保守阈值；后续用 Python 自适应阈值 |
| Retry Window 导致 iteration 浪费 | 低 | 中 | Retry Window 默认可配置；重度才 Pause，轻/中度不阻断 |
| 新增线程增加资源消耗 | 低 | 低 | 线程在 telemetry.enabled 时才创建；channel 消费是事件驱动，非轮询 |
| 与现有 diagnostics 格式冲突 | 低 | 低 | DriftLogger 使用独立文件（drift.jsonl），不修改现有文件格式 |

---

## Documentation / Operational Notes

- `AGENTS.md` / `CLAUDE.md` 需要更新：新增 `RALPH_DIAGNOSTICS=1` 时 drift 监控的行为说明
- `docs/guide/` 可新增 `drift-monitoring.md` 用户文档（说明三种指标、三级干预、配置方式）
- Preset 示例：`presets/en/ce-executor.yml` 中可添加 `telemetry:` 段作为示范

---

## Sources & References

- **调研报告结论：** Gap 1（运行时 drift 监控）为 P0，建议通过 DriftObserver + EventBus observer + 滑动窗口实现
- **相关代码：**
  - `crates/ralph-proto/src/event_bus.rs`（observer 模式）
  - `crates/ralph-core/src/event_loop/mod.rs`（build_prompt、inject_fallback_event）
  - `crates/ralph-core/src/diagnostics/`（diagnostics 系统）
  - `crates/ralph-core/src/config.rs`（配置扩展模式）
- **工业对标：** Langfuse / Arize Phoenix 案例显示生产 agent 2 周内必然出现 field drift；Promptfoo 发现 60%+ agent 在 prompt 调整后产生未预期 output drift
