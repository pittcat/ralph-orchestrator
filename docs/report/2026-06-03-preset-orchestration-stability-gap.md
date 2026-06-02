# Preset 编排稳定性 gap 调研报告

> 📅 2026-06-03 | 🔖 branch=`pittcat-dev` | 调研性质（非实施）

---

## 1. 总体结论

| 维度 | 状态 | 说明 |
|------|------|------|
| U1–U8 计划覆盖度 | 🟢 充分 | payload 字段契约这一单一维度已规划完整 |
| 业界方案对标 | 🟡 局部领先 | 在"启动期硬门槛"上思路正确；但 schema 版本化、漂移检测、Saga 补偿三个层面缺失 |
| 缺失能力数量 | 🟡 14 项 | 3 项 P0（与稳定性直接相关）、5 项 P1（重要非阻塞）、6 项 P2（长期演进） |
| 整体风险 | 🟡 中 | LLM 概率性带来的"明明 schema 都对但还是崩"无法被 U1–U8 覆盖 |

**一句话总结**：U1–U8 把"payload 字段对不对"打扎实了，但这只是 workflow 稳定性的 1/4；P0 级别的"schema 版本化、causality 检查、漂移检测"不补，LLM 概率性会让契约系统在生产中持续暴雷。

---

## 2. 为什么要做这次调研

> **背景**：payload 契约强制校验计划（`docs/plans/2026-06-02-005-feat-payload-contract-validation-plan.md`，U1–U8，共 8 个实施单元）正在筹备实施。计划本身的范围聚焦于"payload 字段对不对"这一单一维度。在投入实施前，需要从两个角度做横向校准：
>
> 1. **项目现状**：preset 编排的哪些"不稳定性"已经被现有代码（`preset_validator.rs`、`event_policy.rs`、`event_origin.rs`、`state_machine.rs`）覆盖，哪些没覆盖？
> 2. **业界方案**：2025–2026 主流的 multi-agent orchestration（Temporal + LangGraph + Prefect 三层模型、Agentable 静态分析、Prompt Contract Testing、Agent Stability Index、Schema Registry）已经演化出什么能力？Ralph 在哪些层面落后？
>
> **本次目标**：基于 Perplexity 调研 + 项目代码阅读，输出一份 gap 清单与优先级排序，让用户决定哪些缺口补在 U1–U8 范围内、哪些作为后续独立计划。

---

## 3. 现状梳理：U1–U8 覆盖了什么

| 计划单元 | 业界对应能力 | 覆盖度评估 |
|---------|-------------|-----------|
| U1 外部 schema 文件 + `schema_file` 字段 | Schema Registry / `schema_file` 模式（Confluent/Temporal） | ✅ 思路对，缺版本号与兼容性约束 |
| U2 instructions 字段启发式提取 | Heuristic static analysis | ⚠️ 偏简陋；业界已用 CPG + LLM（Agentable, arXiv 2412.18371） |
| U3 跨 hat payload 契约校验 | Consumer-driven contract testing | ✅ 思路对，缺因果/状态不变式 |
| U4 Hard Gate（`ralph run` 启动前强制） | Pre-flight validation gate | ✅ 业界标准做法 |
| U5 Loop Pause + JSON 诊断报告 | Runtime guard + structured diagnostics | ⚠️ 业界更倾向 Saga 补偿 + 持续 trace，而非"停车" |
| U6 ce-executor.yml schema（reference implementation） | Reference implementation | ✅ 标准做法 |
| U7 全量 preset 兼容性审计 | Compatibility matrix | ✅ 标准做法 |
| U8 文档迁移说明 | Documentation deliverable | ✅ |

**U1–U8 共同的"单一维度"假设**：只关心"事件字段在不在 schema 里"。**没覆盖**的：
- 字段版本演化（V1→V2 怎么迁移）
- 事件之间的因果顺序（A 必须先于 B）
- LLM 输出概率性导致的"字段今天在、明天不在"
- 跨 hat 调用次数与死循环
- 已发生副作用的撤销

### 现状代码层的支撑

- `crates/ralph-core/src/preset_validator.rs`：已有 topology reachability 检查（`TopologyErrorKind::{UnreachableStart, UnreachableCompletion, UnreachableRequired}`），但**无** orphan publish、cycle bound、causality 检查
- `crates/ralph-core/src/event_policy.rs`：已有 `ViolationType` 与 `PolicyDecision::{Accept, Warn, RejectWithResume, Hold, Block, Ignore}`，但**无** idempotency、field_completeness metric、drift 监控
- `crates/ralph-core/src/event_origin.rs`：检查事件来源 hat 合法性，**无** payload 内容净化（防 prompt injection）
- `crates/ralph-core/src/state_machine.rs`：实例 lifecycle 强制（open→active→terminal），**无**跨 hat 因果约束
- `crates/ralph-core/src/diagnostics/`：仅在错误时记录（log_rotation、integration_tests、stream_handler），**无**持续 trace / OpenTelemetry 风格 span

---

## 4. 业界方案关键 takeaway（Perplexity 调研 2026-06）

调研覆盖 4 个维度：durable orchestration、static analysis for agents、drift detection、schema evolution。关键发现：

### 4.1 三层 orchestration 模型（LangGraph + Temporal + Prefect）

- **Agent logic 层**（LangGraph）：state machines、decision graphs
- **Durable runtime 层**（Temporal）：guaranteed completion、Saga 补偿、crash recovery、跨服务协调
- **Data pipeline 层**（Prefect）：数据/ML 交接、可观测性
- 业界共识："**Pick the boundary, not the brand**"——Ralph 当前相当于自带 LangGraph-style 的 hat 决策图，但缺 Temporal-style 的 durable runtime 与 Saga 补偿
- 参考：https://aiworkflowlab.dev/article/ai-workflow-orchestration-in-production-building-durable-agent-pipelines-with-langgraph-and-temporal

### 4.2 Static Analysis for LLM Agents

- **Agentable**（arXiv 2412.18371）用 Code Property Graphs + LLM 做缺陷检测，识别死分支、状态不一致、可疑控制流
- 业界推荐组合：**linting + 形式化建模 + CI 门禁**，针对 LLM agent 专用
- 我们的 U2 用纯正则启发式，**召回低、误报低**，但**无法识别 instructions 内部的死分支**

### 4.3 Drift Detection（最重要的发现）

- **Agent Stability Index（ASI）**（arXiv 2601.04170）—— 多 agent 系统在长任务中行为稳定性是 0.x 量级
- 三类 drift：
  - **Semantic drift**（意图漂移）
  - **Coordination drift**（协调漂移，hat 之间协议偏离）
  - **Behavioral drift**（行为漂移，emits 模式随时间变化）
- 关键数据：Prompt Contract Testing（tianpan.co）显示 **41–87% 的生产失败是协调类**，其中过半是因果错位
- 我们的 U1–U8 完全是"静态契约"思维，**无任何运行时 metric 监控**

### 4.4 Schema Evolution & Versioning

- Temporal：`WorkflowVersion` + activity definition 版本
- Inngest：event version 字段 + 显式路由
- Confluent Schema Registry：三种 `compatibility mode`（`BACKWARD` / `FORWARD` / `FULL`）
- 关键模式：版本字段、并行运行期、灰度路由、deprecated flag
- 我们的 U1 合并策略"内联优先"**不包含任何兼容性约束**

### 4.5 Contract Testing for LLM

- **Prompt Contract Testing**：把 microservices 的 consumer-driven contract testing 迁移到 LLM prompt/tool interface
- **llmcontract.dev**：用 session type 描述 hat 之间的合法对话序列
- **ContractBench**（arXiv 2605.17281）：contract compliance is unsolved at frontier，**且 non-monotonic with scale**——意思是模型越大未必越守约
- 业界结论：contract testing 必须**独立于 LLM 能力本身**，作为外部强制层

---

## 5. 缺失能力清单（按优先级）

### 🔴 P0 — 与稳定性直接相关，建议尽快补

#### Gap 1：Schema 版本化 + 向后/向前兼容

- **问题**：`schemas.yml` 无 `version` 字段，无 `compatibility mode`
- **后果**：加新字段后老 hat 立刻报 contract violation；无法灰度
- **业界**：Temporal `WorkflowVersion`、Inngest event version、Confluent `BACKWARD`/`FORWARD`/`FULL`
- **建议**：
  ```yaml
  # schemas.yml
  schema_version: 2
  compatibility: BACKWARD
  topics:
    review.wave.ready:
      v1_required: [...]
      v2_required: [...]  # 包含 v1 + 新字段
  ```
- **接入点**：U1 扩展为 `EventPolicyConfig.schema_version` + U5 诊断报告加 `expected_version` / `actual_version` 字段
- **工作量**：小（1–2 天）

#### Gap 2：Causality / State Invariant 检查

- **问题**：U3 只查"字段在不在 schema"，不查"事件 A 必须在事件 B 之前"
- **例子**：`plan.ready` 之前必须 `plan.gate.passed`，否则 reviewer 收到没 gate 过的 plan
- **业界**：tianpan.co Prompt Contract Testing、llmcontract.dev session types
- **建议**：`EventSchema` 加 `prerequisite_topics: Vec<String>`，U3 静态校验 + U5 运行时校验
- **接入点**：U3 扩展（增加 prerequisite 校验维度）
- **工作量**：中（3–5 天）

#### Gap 3：Drift Detection（Agent Stability Index 风格）

- **问题**：U1–U8 是"静态对了 = 不会坏"。LLM 输出有概率性，今天发的字段都对，明天可能漏
- **业界证据**：ASI 显示多 agent 系统长任务稳定性 0.x 量级；41–87% 生产失败是协调类
- **建议**：
  ```yaml
  # preset 中
  drift_policy:
    field_completeness_min: 0.95  # task_id 命中率 < 95% 触发
    window: 50  # 滑动窗口
    on_drift: pause_loop_with_diagnostic
  ```
- **接入点**：U5 之后新增独立单元（依赖 U5 的诊断报告 writer + event_bus observer）
- **工作量**：中（5–7 天，含 metric store 设计）

### 🟡 P1 — 重要但非阻塞

#### Gap 4：Dead Code / Unreachable Path Detection（preset 层）

- 现状：`preset_validator.rs` 查 topology reachability，但不查：
  - hat instructions 中的"如果 X 那么 Y"分支是否真的能走到
  - orphan publish（发了但无人订阅）
  - hat 配置了但 triggers 全是 wildcard 实际从未命中
- 业界：Agentable 用 CPG + LLM 做死分支检测
- 建议：U3 加 `detect_orphan_publishes` + `detect_unreachable_branches`
- 接入点：U3 扩展
- 工作量：中（3–5 天）

#### Gap 5：Cycle / Loop Bound（防 agent 推理死循环）

- 现状：wave_tracker 有限制但 hat-level 循环无限制
- 风险：reviewer 觉得 plan 不好→ `plan.gate.failed`→ coordinator 重新生成→ 循环
- 业界：Inngest `maxAttempts`、Temporal `ContinueAsNew` 配 cycle counter
- 建议：`HatConfig` 加 `max_invocations_per_loop: Option<u32>`，U5 诊断附 invocation 计数
- 接入点：U5 + HatConfig 扩展
- 工作量：小（1–2 天）

#### Gap 6：Idempotency Key

- 问题：LLM 网络重试或 schema 迁移期间，同一 hat 可能被触发两次
- 业界标准：所有 emit 携带 `idempotency_key`，event bus 自动去重
- 建议：`EventPolicyConfig` 加 `idempotency: { mode: dedupe_by_key, window_secs: 3600 }`
- 接入点：U5 扩展 event_policy.rs
- 工作量：小（1–2 天）

#### Gap 7：Saga / Compensation（动作可撤销）

- 现状：Loop Pause = 全停。已发生的副作用（git commit、文件写入）保留，无补偿
- 业界：Temporal Saga pattern 是工作流引擎的标配
- 建议：`HatConfig` 加 `compensate_with: Option<String>`，U5 触发 Loop Pause 时按"已 commit 顺序"反向调用补偿 hat
- 接入点：U5 之后独立单元
- 工作量：大（7–10 天）

#### Gap 8：Glass-Box / 结构化测试

- 不仅测"输出对不对"，还测"内部决策路径对不对"
- 例如：reviewer 收到缺字段时必须 emit `review.rejected`，不能继续往下走
- 业界：arXiv 2601.18827
- 接入点：BDD scenarios 扩展（`crates/ralph-core/tests/scenarios/`）
- 工作量：中（3–5 天）

### 🟢 P2 — 长期演进

#### Gap 9：Schema Registry / 跨 preset 共享

- 现在每个 preset 自己一套 schema，worktree loop 多 preset 并行时无法对齐
- 业界：Confluent Schema Registry（中心化服务）
- 建议：建轻量级 `.ralph/schemas/` 目录做本地 registry，跨 preset 引用

#### Gap 10：Prompt Injection 防御

- payload 含 "ignore previous instructions" 可注入下游 LLM hat
- origin guard 检查来源合法性，**不查内容**
- 建议：`EventPolicyConfig` 加 `content_sanitization: { strip_instruction_overrides: true }`

#### Gap 11：Token / Cost Budget 静态校验

- preset 没限制单 hat prompt 长度上限、没总成本预算
- 业界：Inngest `stepBudget`、Dagster resource limits
- 建议：`HatConfig` 加 `prompt_budget_tokens: Option<u32>`、`loop_cost_budget_usd: Option<f64>`

#### Gap 12：可观测性升级（OpenTelemetry 风格）

- `.ralph/diagnostics/` 只在出错时写
- 业界标配：每个 hat 一次 invocation 一个 span，跨 hat trace_id 串联
- 接入点：event_bus 已有 observer 模式，扩展 cost 低
- 建议：复用现有 observer，加 `tracing` crate + `tracing-subscriber` + OpenTelemetry exporter

#### Gap 13：多版本 preset 并存 / Canary 路由

- 没办法"50% 用老 ce-executor，50% 用新版"
- 业界：feature flag / canary
- 建议：在 `event_loop` 启动参数加 `--preset-version` + 流量分配

#### Gap 14：Prompt Contract Testing（运行时协议）

- 业界 llmcontract.dev 用 session type 描述 hat 间合法对话序列
- 现状：只查单事件字段，跨事件对话规则无强约束
- 建议：增加"对话脚本"维度，例：`合法序列 = plan.ready → review.wave.ready → review.dimension.done → fix.ready → ship.ready`

---

## 6. 三个演进方向（优先级矩阵）

| 方向 | 包含 gap | 工作量 | 适用场景 |
|------|---------|--------|---------|
| **A. 完善 Payload 契约** | P0: 1+2 | 1–2 周 | 解决最痛的 80% 问题；U1–U8 计划补充 |
| **B. 漂移 + 死分支 + Cycle bound** | P0: 3 + P1: 4+5 | 3–4 周 | 把 LLM 概率性纳入控制；新建独立计划 |
| **C. Saga + Idempotency + OTel** | P1: 6+7+8, P2: 12 | 4–6 周 | 往 Temporal 方向靠拢；分多 plan 演进 |

**个人建议**：A 先做（U1–U8 实施期间直接补），B 中期（U1–U8 落地后立刻开新 plan），C 长期（看用户业务场景是否需要"重型 orchestration"）。

---

## 7. 需要用户拍板的事

### 决策 1：U1–U8 是否扩展包含 P0 三个 gap？

- **选项 A**：U1–U8 严格按当前范围实施（仅 payload 字段契约），P0 三个 gap 作为独立后续 plan
  - 优点：U1–U8 范围明确、可控
  - 缺点：LLM 概率性带来的"字段今天在、明天不在"无法被覆盖
- **选项 B**：U1–U8 扩展，把 P0-1（schema 版本化）和 P0-2（causality）直接纳入 U1/U3
  - 优点：一次到位
  - 缺点：U1/U3 实施复杂度上升
- **选项 C**：U1–U8 不变，但在 U5 之后追加 U9/U10 单元做 P0-1 和 P0-2
  - 优点：渐进式、保持原 plan 不动
  - 缺点：plan 文档结构散乱
- **建议**：选项 C（保持 U1–U8 范围清晰，追加 U9/U10）

### 决策 2：U1–U8 是否包含 P0-3（Drift Detection）？

- **选项 A**：不做（维持"轻协调"哲学）
- **选项 B**：做，但**仅做 field_completeness 单一维度**（不做 ASI 全套）
  - 实现：`event_bus` observer 累积 sliding window 命中率；超阈值时触发 Loop Pause
  - 工作量：5–7 天
- **建议**：选项 B（解决最常见的 LLM 漏字段问题，且实施成本可控）

### 决策 3：是否引入 metric store？

- 当前所有状态走文件系统（`.ralph/` 目录）
- drift metric 需要聚合、跨 session 对比，文件系统效率低
- **选项 A**：纯文件（追加 JSONL + 启动时聚合）
  - 优点：架构简单
  - 缺点：跨 session 对比需要手动脚本
- **选项 B**：本地 SQLite（`.ralph/metrics.db`）
  - 优点：跨 session 查询快
  - 缺点：引入 SQL 依赖
- **建议**：选项 A（先纯文件，如果 drift detection 用得起来再升级 SQLite）

### 决策 4：drift detection 跟"暂停"还是"告警"？

- **选项 A**：超阈值 → Loop Pause（与 U5 一致）
- **选项 B**：超阈值 → 仅写 diagnostic，loop 继续
- **建议**：选项 A（与现有"严格不放过"哲学一致；选项 B 会让 LLM 概率性问题被忽略）

---

## 8. 下一步计划

1. **本次报告交付**：`docs/report/2026-06-03-preset-orchestration-stability-gap.md` 提交
2. **等待用户决策 1–4**
3. **决策确定后**：
   - 若选"选项 C"：在 `docs/plans/2026-06-02-005-feat-payload-contract-validation-plan.md` 追加 U9（schema 版本化）、U10（causality / prerequisite_topics）
   - 若选"选项 B for P0-3"：新增独立 plan `2026-06-XX-feat-drift-detection.md`
4. **B 方向**（P0-3 + P1-4+5）：作为 U1–U8 落地后立刻开的新 plan
5. **C 方向**（Saga / Idempotency / OTel）：观察用户业务场景再决定优先级

---

## 9. 引用与参考

### 项目内部

- `docs/plans/2026-06-02-005-feat-payload-contract-validation-plan.md`（payload 契约计划 U1–U8）
- `docs/brainstorms/2026-06-02-payload-contract-validation-requirements.md`（需求 origin）
- `docs/solutions/developer-experience/ce-executor-plan-gate-premature-completion-2026-06-02.md`（相关：plan-gate 修复）
- `docs/solutions/tooling-decisions/ralph-preset-embedded-compilation-2026-05-26.md`（embedded preset 同步）
- `crates/ralph-core/src/preset_validator.rs`（现状：topology reachability only）
- `crates/ralph-core/src/event_policy.rs`（现状：ViolationType + PolicyDecision）
- `crates/ralph-core/src/event_origin.rs`（现状：来源合法性 only，无内容净化）
- `crates/ralph-core/src/state_machine.rs`（现状：lifecycle only，无跨 hat 因果）
- `presets/ce-executor.yml`（1109 行，9 个 hat，15 个 topic——本次调研的 reference preset）

### 业界资料（Perplexity 2026-06 调研）

- **三层 orchestration 模型**：
  - https://aiworkflowlab.dev/article/ai-workflow-orchestration-in-production-building-durable-agent-pipelines-with-langgraph-and-temporal
  - https://www.bestaiweb.ai/how-to-build-a-production-ai-workflow-with-langgraph-temporal-and-prefect-in-2026/
  - https://agentmarketcap.ai/blog/2026/04/11/temporal-inngest-prefect-agent-orchestration-platforms-2026
- **Static analysis for LLM agents**：
  - https://arxiv.org/pdf/2412.18371.pdf（Agentable，CPG + LLM 缺陷检测）
  - https://arxiv.org/html/2601.18827v1（structural testing）
- **Drift detection**：
  - https://arxiv.org/abs/2601.04170（Agent Stability Index / Quantifying Agent Drift）
  - https://www.emergentmind.com/papers/2601.04170
  - https://tianpan.co/blog/2026-05-05-prompt-contract-testing-multi-agent-coordination（41–87% 生产失败是协调类）
- **Contract testing for LLM**：
  - https://llmcontract.dev（session type runtime protocol monitor）
  - https://tianpan.co/blog/2026-04-27-contract-tests-llm-tool-surfaces
  - https://arxiv.org/pdf/2605.17281.pdf（ContractBench）
- **Schema evolution**：
  - https://bandageek.com/schema-evolution-compatibility-modes-and-versioning-tactics（BACKWARD/FORWARD/FULL）
  - https://akka.io/blog/inngest-vs-temporal
  - https://www.inngest.com/compare-to-temporal
- **Observability**：
  - https://jangwook.net/en/blog/en/ai-agent-observability-production-guide/
  - https://zylos.ai/research/2026-04-29-agent-observability-production-debugging/
- **Saga / Durable execution**：
  - https://agentmarketcap.ai/blog/2026/04/08/agent-workflow-state-management-temporal-inngest-aws-step-functions
  - https://agentmarketcap.ai/blog/2026/04/07/durable-execution-temporal-inngest-cloudflare-workflows-agent-amnesia

---

## 10. 附录：ce-executor.yml 触发/发布图（用于 gap 评估）

| Hat | Triggers | Publishes |
|-----|----------|-----------|
| 📋 Coordinator | `work.start` | `work.ready`, `work.failed` |
| ⚙️ Executor | `work.ready`, `queue.advance`, `work.retry` | `work.done`, `work.failed` |
| 🔍 Review Coordinator | `work.done`, `fix.applied` | `review.wave.ready`, `review.passed` |
| 🔬 Dimension Reviewer | `review.wave.ready` | `review.dimension.done` |
| 🧩 Review Synthesizer | `review.dimension.done` | `review.passed`, `review.failed`, `review.complete` |
| 🔧 Fixer | `review.failed` | `fix.applied`, `fix.exhausted` |
| 🚦 Plan Gate | `review.passed`, `review.complete` | `queue.advance`, `plan.complete`, `plan.blocked` |
| 📦 Shipper | `plan.complete`, `plan.blocked`, `fix.exhausted` | `REVIEW_COMPLETE` |
| 📊 Reporter | `REVIEW_COMPLETE` | `report.done`, `LOOP_COMPLETE` |

**观察**：
- `queue.advance` 同时被 Executor 消费和 Plan Gate 发布，**有循环风险**（P1-5）
- `REVIEW_COMPLETE` 是大写，与其它小写 topic 风格不一致（embedded 风险）
- 9 个 hat 15 个 topic——schema 全面化后大约 15 个 `EventSchema` 条目
- 当前缺 prerequisite 约束：如 `review.wave.ready` 应该 prerequisite `work.done`（P0-2 可补）
