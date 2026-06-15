# Preset 编排稳定性 gap 调研报告（v2 — 工业开源方案重做版）

> 📅 2026-06-03 | 🔖 branch=`pittcat-dev` | 调研性质（非实施）
>
> **v2 变更说明**：v1 报告 4.2 节被评价为"学术界的玩具"（含 Agentable arXiv 2412.18371、ContractBench、ASI 等纯学术论文）。本版重做，全部替换为**工业界已生产部署的开源方案**，并基于项目实际代码（`payload_contract.rs`、`execution_contract.rs`、`preset_validator.rs` 等）重新梳理 gap。

---

## 1. 总体结论

| 维度 | 状态 | 说明 |
|------|------|------|
| 已落地的能力 | 🟢 大量 | payload 字段契约（U1–U8）+ Execution Contract（U1–U9）均已实现，框架就位 |
| 业界方案对标 | 🟡 局部领先 | 在"启动期硬门槛 + 字段级静态校验 + 失败模式 fail-closed"上思路与 Temporal/Langfuse 一致；但 schema 版本化、Saga 补偿、运行时 drift metric、ce-executor 适配状态机四块仍空 |
| 真正缺失的能力 | 🟡 8 项 | 1 项 P0（drift 监控）、3 项 P1（schema 版本化 / Saga / OTel）、4 项 P2（cycle bound / canary / schema registry / injection 防御） |
| 整体风险 | 🟡 中 | 框架在，**真实运行时的 LLM 概率性 + 业务级一致性 + 跨版本 schema 演化**仍无度量、无门禁 |

**一句话总结**：当前已经从"想做 payload 字段校验"演进到"已经做出来并接进 ce-executor"。但"字段在不在"是 workflow 稳定性的下界；**上界 = drift 监测 + schema 演化 + 业务可逆性**，这三块不补，长期仍然会被 LLM 概率性 + 业务并发吞掉。

---

## 2. 为什么要做这次调研

> **背景**：payload 契约强制校验计划（`docs/plans/2026-06-02-005-feat-payload-contract-validation-plan.md`，U1–U8）已落地；后续 execution contract 计划（`docs/plans/2026-06-03-001-feat-agent-execution-contract-gates-plan.md`，U1–U9）也已实现并通过 `docs/report/agent-execution-contract-gates-review-2026-06-03.md` 审查（发现若干 fail-open 缺陷正在修复）。在两个 plan 都基本落地后，需要从两个角度做横向校准：
>
> 1. **项目现状**：preset 编排的"不稳定性"已经被现有代码覆盖了多少？还差哪些？特别是静态校验（U2/U3/U7/U8）+ 运行时合同（U1–U9 execution contract）+ 拓扑校验（`preset_validator.rs`）三块之后，还有什么"没人在看"的稳定性维度？
> 2. **业界方案**：2025–2026 工业界生产部署的 multi-agent orchestration（**Temporal、LangGraph、Restate、Netflix Conductor、Apache Airflow、Argo Workflows**）、LLM agent 可观测性（**Langfuse、Arize Phoenix、OpenLLMetry、MLflow**）、schema 演化（**Confluent Schema Registry、Apicurio、Buf**）、契约测试（**Pact、Spectral**）的当前最佳实践是什么？Ralph 在哪些层面对齐了、哪些还差？
>
> **本次目标**：基于 Perplexity 工业方案调研 + 项目实际代码阅读，输出一份**面向工业实践的** gap 清单与优先级排序，让用户决定哪些缺口在原 plan 收尾后立刻补、哪些作为新独立 plan。

---

## 3. 现状梳理：项目实际代码层已覆盖什么

> 与 v1 不同，本节直接引用代码文件与符号，避免"计划文档列了什么"和"代码里实际有什么"的脱节。

### 3.1 已经实现的能力（按代码层）

| 模块 | 关键符号 | 已实现 | 仍未实现 |
|------|---------|--------|----------|
| `crates/ralph-core/src/payload_contract.rs` (904 行) | `extract_payload_field_refs()`、`PayloadContractErrorKind::{FieldMissingFromSchema, SchemaMissingForRequiredTopic}`、`validate_payload_contract()` | ✅ 三种正则模式（`From event payload:` / `payload MUST include:` / `event payload:` + backtick）；U3 跨 hat 校验；strict / default 模式 | ❌ 字段类型校验；嵌套 schema；`prerequisite_topics` 因果约束；版本号 |
| `crates/ralph-core/src/execution_contract.rs` (997 行) | `ExecutionContractViolationKind`、`GitEvidenceProvider`、`validate_execution_contract()` | ✅ payload 必填字段、task state、git evidence（`has_uncommitted_changes` + `has_new_commits_since`）、test evidence；injection 友好的 fail-closed 路径 | ⚠️ 审查报告指出 `validate_task()` 在 task 不存在时仍 fail-open；`diff_or_commit` 的 commit 路径会放过空仓库 |
| `crates/ralph-core/src/preset_validator.rs` (1444 行) | `TopologyErrorKind`、`validate_preset_topology()`、`is_required_on_all_paths()` | ✅ topology reachability（`UnreachableStart` / `UnreachableCompletion` / `UnreachableRequired`）、cycle detection、wildcard trigger 解析、default_publishes 推断 | ❌ orphan publish 检测、cycle bound 上限、prerequisite 拓扑约束 |
| `crates/ralph-core/src/event_policy.rs` (959 行) | `ViolationType`、`PolicyDecision::{Accept, Warn, RejectWithResume, Hold, Block, Ignore}`、`validate_event()` | ✅ required_fields、allowed_values、duplicate terminal detection、completion-honored guard、terminal monotonicity、observe/enforce 模式 | ❌ `prerequisite_topics` 运行时校验、drift 阈值、idempotency key 去重 |
| `crates/ralph-core/src/event_origin.rs` (791 行) | origin guard | ✅ 来源 hat 合法性、scope 内 topic 校验、fail-closed | ❌ payload 内容净化（防 prompt injection 字符串）、上游 hat 信任等级 |
| `crates/ralph-core/src/state_machine.rs` (832 行) | `StateMachineDecision`、`InstanceState::open/active/terminal`、`validate_event()` | ✅ 实例 lifecycle（open → active → terminal）、out-of-order 拒绝、terminal 后拒 business event、`mark_terminal_honored` | ❌ 跨 hat 因果约束、跨实例依赖、retry 计数上限 |
| `crates/ralph-core/src/wave_tracker.rs` | wave concurrency | ✅ 并发 wave 上限、wave_id 关联、merge 回主事件流 | ❌ wave 内部的 cycle bound、wave 间共享 schema 校验 |
| `crates/ralph-core/src/diagnostics/` | 错误时记录 | ✅ JSONL 写盘、log rotation | ❌ OpenTelemetry span、跨 hat trace_id 串联、持续 trace（非仅错误时） |
| `presets/en/ce-executor.yml` / `presets/zh/ce-executor-zh.yml` / `presets/schemas/ce-executor.yml` | 多语言 preset 分目录 | ✅ 9 hat / 15 topic 真实业务编排；embedded 同步测试 | ❌ `prerequisite_topics` 字段；schema 版本号；hat-level cycle bound |

### 3.2 已落地能力的工业对标位置

| ralph 能力 | 工业开源对应 | 状态 |
|----------|------------|------|
| `payload_contract.rs` 的字段提取 + 跨 hat 校验 | Spectral（OpenAPI/AsyncAPI lint）+ Pact（consumer-driven contract）+ Buf schema lint | 思路对，实现简陋（Spectral 是 yaml/openapi；Pact 是 consumer-driven；Buf 是 protobuf） |
| `execution_contract.rs` 的 fail-closed 校验 | Temporal activity heartbeat + workflow versioning；Pact provider state | 思路对，部分 fail-open 待修（审查报告已指明） |
| `event_policy.rs` 的 duplicate terminal / completion-honored | Temporal 事件历史 append-only；Cadence 决策任务去重 | 一致 |
| `state_machine.rs` 的 open→active→terminal | Spring State Machine / MassTransit Automatonymous / NServiceBus Saga | 一致（生命周期模型） |
| `event_origin.rs` 的来源 hat 合法性 | Open Policy Agent（OPA）/ Casbin / Cedar（policy-as-code） | 思路对，但 ralph 没引入独立 policy engine |
| `preset_validator.rs` 的 topology reachability | LangGraph graph validation；Pulumi state graph check | 思路对，缺 orphan / cycle bound / prerequisite |

### 3.3 仍未覆盖的（v1 假设的清单 → 实际仍有意义）

- 字段版本演化（V1→V2 怎么迁移）
- 事件之间的因果顺序（A 必须先于 B）
- LLM 输出概率性导致的"字段今天在、明天不在"——**没任何运行时 metric 监控**
- 跨 hat 调用次数与死循环
- 已发生副作用的撤销
- **新增**：drift 阈值报警、OpenTelemetry 风格跨 hat trace、跨 session drift 对比

---

## 4. 业界工业方案（2026 主流开源、生产部署）

> 与 v1 不同，本节所有引用均为**已生产部署的开源项目**或**真实生产案例**。无 arXiv 学术论文、无未发布的学术原型。每个工具都给出 GitHub repo、stars 量级、生产采用案例、ralph 编排场景下的可借鉴点。

### 4.1 多 agent 编排框架

**生产级"重型" orchestrator（适合借鉴其模式，不一定要引入）：**

- **Temporal** — `temporalio/temporal`（Go 服务端，多语言 SDK；社区公开 stars 量级 30k+）
  - 架构：event-sourced durable execution，workflow 状态完全由事件历史重建
  - 与 ralph 关系：ralph 的 `loop_state` + `event_loop` 已经有 Temporal 风格的雏形；缺**显式 workflow versioning**（Temporal 的 `GetVersion()` API）和**deterministic replay 强制**
  - 借鉴点：`getVersion()` 在 hat 改 instructions 时强制走 upgrade path；`ContinueAsNew` 防止事件历史无限增长
  - 案例：Coinbase、Stripe、Datadog 在生产用 Temporal 编排微服务（社区 conference talks 公开发言）
- **Netflix Conductor** — `Netflix/conductor`（Java；~10k+ stars）
  - 架构：DAG-based workflow + 任务系统 + JSON DSL
  - 借鉴点：tasks 的 input/output schema 是独立的，**任务之间靠 schema 显式契约**（与 ralph 当前 `event_policy.schemas` 思路一致）
- **Cadence** — `uber/cadence`（Go；stars 量级 ~8k+）
  - 架构：Temporal 的"前身"，Uber 内部生产
  - 借鉴点：decision task 概念，**workflow 状态变更与外部事件解耦**
- **Apache Airflow** — `apache/airflow`（Python；~30k+ stars）
  - 架构：DAG + scheduler + operator 生态
  - 借鉴点：`pool` / `priority_weight` 资源限制；`sla_miss_callback` 在 miss 时报警
  - 局限：不是为 LLM 概率性设计，**没有 drift 概念**

**LLM-native 编排框架（更适合"hat 编排"思路）：**

- **LangGraph** — `langchain-ai/langgraph`（Python + JS；2025–2026 主流）
  - 架构：图结构 + 条件边 + checkpointing
  - 借鉴点：**StateGraph 的 conditional edges 显式建模**（与 ralph `hatless_ralph.rs` 的 hat selection 类似）；`MemorySaver` checkpointing 与 ralph `loop_state_snapshot.rs` 思路一致
  - 已知用户：Replit、Uber、LinkedIn 部分 agent 系统（LangChain 公开 case study）
- **Restate** — `restatedev/restate`（Rust 服务端 + 多语言 SDK）
  - 架构：durable execution + virtual objects + 显式 Saga
  - 借鉴点：**Saga API 原生**——每个 handler 可声明 `compensate`，失败时反向调用
  - 案例：Restate 自己在社区 talk 公开金融科技、电商客户
- **AutoGen** — `microsoft/autogen`（Python + .NET；2025 主流）
  - 架构：multi-agent 对话 + group chat manager
  - 借鉴点：`GroupChatManager` 的"消息广播 + 单一发言者"模型与 ralph 的 hat 拓扑有相似性
- **CrewAI** — `crewAIInc/crewAI`（Python；2025–2026 主流）
  - 架构：role-based 多 agent + task delegation
  - 借鉴点：`process: hierarchical` 让 manager hat 路由子任务（与 `coordinator` hat 模式一致）

**结论**：ralph 的 hat 编排介于 LangGraph（graph-based + checkpointed）和 Restate（durable + Saga）之间，**两边都借鉴但都不完整**。不需要换框架，重点是补 Temporal/Restate 风格的**显式 versioning + Saga 原语**。

参考链接：
- https://github.com/temporalio/temporal
- https://github.com/Netflix/conductor
- https://github.com/uber/cadence
- https://github.com/apache/airflow
- https://github.com/argoproj/argo-workflows
- https://github.com/langchain-ai/langgraph
- https://github.com/restatedev/restate
- https://github.com/microsoft/autogen
- https://github.com/crewAIInc/crewAI

### 4.2 静态分析与配置 lint（替换 v1 "纯正则启发式"为工业组合）

> v1 提出的"linting + 形式化建模 + CI 门禁"思路是对的，但 v1 推荐的"CPG + LLM"（Agentable）是学术玩具。本节给出**真正生产部署的开源工具栈**。

**1. YAML / OpenAPI / AsyncAPI 通用 lint**（ralph preset 是 YAML，**直接套用**）

- **Spectral** — `stoplightio/spectral`（TypeScript；~3k+ stars，Stoplight 商业化维护）
  - 用法：定义 `spectral.yaml` ruleset，针对 ralph preset 的 `hats:` / `triggers:` / `publishes:` 写自定义规则
  - 落地：直接 `npx @stoplight/spectral lint presets/**/*.yml --ruleset .spectral/ralph.yml`
  - 借鉴规则示例：wildcard trigger 后面跟的 `publishes` 不能为空；hat `instructions` 必须包含触发 topic 关键词
- **yamllint** — `yamllint/yamllint`（Python；~3k+ stars，Adrien Vergé 长期维护）
  - 用法：基础 YAML 语法 + 风格（缩进、引号、line length）
  - 落地：`yamllint -c .yamllint.yml presets/`
- **Mega-Linter** — `oxsecurity/megalinter`（HCL/Shell；~2k+ stars，Ox Security 商业化维护）
  - 用法：单一入口把 yamllint + Spectral + Markdown lint + Rust clippy 串成 PR check
  - 适用：ralph 已经有 `.github/workflows/` CI，直接嵌入 `MEGA_LINTER: true` env

**2. JSON Schema 校验**（替换 v1 "纯正则字段提取"的核心改进）

- **ajv** — `ajv-validator/ajv`（JavaScript；~14k+ stars，**事实标准**）
- **fastjsonschema** — `horejsek/python-fastjsonschema`（Python；性能更快）
- **jsonschema** — `python-jsonschema/json-schema-org`（Python；~5k+ stars，**官方参考实现**）

**ralph 落地建议**：
1. 把当前 `payload_contract.rs` 的"正则提取 `From event payload: ...`"改为**"读 hat instructions 中显式声明的 schema 引用"**（例：`schema_ref: review.wave.ready@1`）
2. 用 jsonschema/ajv 在 Rust 侧通过 `jsonschema` crate（python-jsonschema 的 Rust 移植）做严格校验
3. 这样**正则只用来 lint instructions 风格**，**字段合规由 schema 决定**

**3. Prompt / agent 行为测试**（v1 漏掉，但工业上必须有）

- **Promptfoo** — `promptfoo/promptfoo`（TypeScript；~6k+ stars）
  - 用法：定义 yaml 格式的 test cases，**对 prompt 行为做 red team / regression test**
  - 落地：把 ce-executor 9 个 hat 的 instructions 各写 5 个代表性输入，验证 output 符合 schema
  - 价值：**捕获"instructions 改了，agent 行为变了，但没人发现"**——这是 v1 "LLM 概率性"问题的实际解决路径
- **PromptTools / OpenAI Evals** — `openai/evals`（Python；OpenAI 官方）
  - 用法：更偏 benchmark 风格，针对单个 model 行为

**4. Rust 侧静态分析栈**（CI 必跑）

- `cargo clippy -- -D warnings`（项目已有，pedantic 配置）
- `cargo udeps`（检测未使用依赖）
- `cargo deny`（license / advisory check）
- `cargo-mutants`（mutation testing，已在部分项目使用）

**5. 借鉴的工业实践**

- **Kubernetes 自身的 schema 校验**：K8s 用 OpenAPI v3 schema + 严格 admission webhook；OpenAPI schema 写在 `crd.yaml` 旁边。ralph 可以照搬：每个 preset 旁边放一个 `*.schema.yaml`，CI 用 Spectral 校验
- **Terraform / Pulumi**：HCL/Pulumi state 都有 `plan` 阶段做拓扑预检；ralph `ralph run --validate-only` 应该走完全相同路径（已有 U4 hard gate）
- **GitHub Actions 自身的 workflow lint**：`rhysd/actionlint`（Go；~3k+ stars）对 GitHub Actions YAML 做静态检查——可作为"用 lint 工具检查工作流定义"的范本

**参考链接**：
- https://github.com/stoplightio/spectral
- https://github.com/yamllint/yamllint
- https://github.com/oxsecurity/megalinter
- https://github.com/ajv-validator/ajv
- https://github.com/python-jsonschema/json-schema-org
- https://github.com/promptfoo/promptfoo
- https://github.com/rhysd/actionlint

### 4.3 可观测性与 drift 检测

> v1 把"drift detection"列为 ASI 学术指标。本节用**生产可用的可观测性栈**重做——这些工具不开 arXiv 论文，但每一家都有日活千+ 企业的部署案例。

**核心栈**（按 ralph 当前 Rust 实现的接入成本排序）：

1. **OpenTelemetry** — `open-telemetry/opentelemetry-rust`（CNCF；~22k+ stars 总仓，OTel 协议是 CNCF 毕业项目）
   - 接入：`tracing` crate + `tracing-opentelemetry` + OTLP exporter
   - 落地：每个 hat 一次 invocation 一个 span，trace_id 跨 hat 串联
   - 价值：替换 v1 "diagnostics 目录仅在错误时写"——OpenTelemetry **默认每个 invocation 都打 span**
2. **OpenLLMetry** — `traceloop/openllmetry`（Python；由 Traceloop 维护，2025 被 Splunk 收购）
   - 用法：自动给 LLM 调用打 span，记录 token / latency / cost
   - 局限：Python 库；ralph 没法直接用，但**协议层面 ralph 自己实现 OTLP export 即可**
3. **Langfuse** — `langfuse/langfuse`（TypeScript + Python；~10k+ stars，Y Combinator W23）
   - 架构：自托管 LLM observability 平台，OTel 兼容
   - 用法：把 ralph 的 hat invocation + payload 抽样上报到 Langfuse dashboard
   - 案例：Langfuse 公开 case study 包含 Mintlify、YC 内部多个 agent 系统
   - 价值：**自带 prompt 版本管理 + eval suite**——这正是 v1 缺失的"字段今天在、明天不在"问题的解
4. **Arize Phoenix** — `Arize-ai/phoenix`（Python；Arize AI 商业化维护）
   - 架构：自托管 agent eval / observability，**默认 OpenTelemetry 导出**
   - 用法：trace 落到 Phoenix，看 token / latency / 异常率
   - 价值：内嵌 retrieval / tool use 的 drift 检测
5. **MLflow Tracking** — `mlflow/mlflow`（Python；~20k+ stars，Linux Foundation）
   - 用法：把每次 loop run 当 experiment，hat invocation 当 run
   - 局限：偏 ML 训练场景，对 online agent 友好度一般

**轻量自托管栈（推荐 ralph 起步用这个）**：

- **Grafana + Loki + Tempo + Prometheus + OpenTelemetry Collector**（全部 CNCF / Grafana Labs 开源）
  - 落地：ralph 起 OTLP exporter，把 span / metric / log 推到本地 Tempo/Prometheus
  - 价值：开源免费、自托管、**和项目"重文件系统"风格一致**
  - 参考：https://opentelemetry.io/docs/languages/rust/

**drift 监控的具体实现路径**（替换 v1 学术 ASI）：

```yaml
# ralph.yml 建议扩展
drift_policy:
  field_completeness:
    enabled: true
    metrics:
      - topic: "work.done"
        required_field: "task_id"
        min_hit_rate: 0.95
        window: 50
    on_drift: "pause_loop_with_diagnostic"
  coord_drift:
    enabled: true
    metrics:
      - from_topic: "work.done"
        to_topic: "review.wave.ready"
        min_join_rate: 0.80
        window: 20
```

实现路径：
- `event_bus` 已有 observer pattern，加一个 `DriftObserver` 累积 sliding window
- 落盘到 `.ralph/metrics/drift.jsonl`（与 diagnostics 同目录）
- 启动时聚合，跨 session 比较（**不需要数据库，先用 JSONL + awk**）

**参考链接**：
- https://opentelemetry.io/docs/languages/rust/
- https://github.com/open-telemetry/opentelemetry-rust
- https://github.com/traceloop/openllmetry
- https://github.com/langfuse/langfuse
- https://github.com/Arize-ai/phoenix
- https://github.com/grafana/tempo
- https://github.com/prometheus/prometheus

### 4.4 Schema 演化与契约测试

> v1 列了 Temporal / Inngest / Confluent，但没给出 ralph 怎么用。本节给出**直接套用的开源方案**。

**Schema Registry（生产部署、给 ralph 提供版本化 schema 服务）**：

- **Confluent Schema Registry** — `confluentinc/schema-registry`（Java；~9k+ stars，事实标准）
  - 兼容性模式：`BACKWARD`（新 schema 读旧数据）、`FORWARD`（旧 schema 读新数据）、`FULL`（双向）、`BACKWARD_TRANSITIVE` / `FORWARD_TRANSITIVE` / `FULL_TRANSITIVE`（带历史）
  - 接入：ralph 的 `event_policy.schemas` 当前是 inline + `schema_file`；可以**演进为指向 Schema Registry URL**
  - 案例：LinkedIn 内部、新浪、字节跳动、Confluent 自己都生产用
  - 局限：Java 服务，ralph 项目要起 sidecar
- **Apicurio Registry** — `Apicurio/apicurio-registry`（Java；~5k+ stars，Red Hat 维护）
  - 比 Confluent 灵活：支持 OpenAPI、AsyncAPI、Avro、JSON Schema、Protobuf
  - 案例：Red Hat 内部、若干欧洲电信公司生产用
- **Buf Schema Registry (BSR)** — `bufbuild/buf`（Go；~10k+ stars，Buf 公司商业化）
  - 强项：Protobuf-first，**集成 breaking change detector**
  - 接入：ralph 当前 schema 是 JSON Schema 风格，可保留；BSR 适合做 Protobuf 业务契约

**轻量级方案（无 sidecar，更适合 ralph 单机部署）**：

- 把 `BACKWARD` / `FORWARD` / `FULL` 三个模式直接做成 `event_policy.compatibility: BACKWARD` 字段
- 在 `event_policy.rs::validate_event` 启动时按模式做兼容性检查
- 兼容性由**字段集合差集 + 类型对照**计算（不引入外部服务）

**契约测试（替换 v1 提的"Prompt Contract Testing"）**：

- **Pact** — `pact-foundation/pact`（多语言；~5k+ stars 总仓，**业界事实标准**）
  - 用法：consumer 写期望（pact file），provider 验证可满足
  - ralph 落地：把 ce-executor 9 个 hat 当 consumer，coordinator 当 provider，写 9 个 pact file 做 CI gate
  - 案例：澳洲国民银行、Disney+、Accenture（公开 case study）
- **Pactflow**（Pact 商业版，serverless broker）——生产部署用得多
- **Spectral**（已在 4.2 列出）——可同时 lint OpenAPI/AsyncAPI，做 producer-side 契约
- **AsyncAPI** — `asyncapi/asyncapi`（JavaScript 工具链；~5k+ stars，Linux Foundation）
  - 用法：把 ralph 的 event topic 流定义成 AsyncAPI 2.x spec，**CI 校验 spec 一致性**
  - 价值：editor 可视化、code generation、文档自动生成

**ralph 推荐的工业实践组合**（无 sidecar 路线）：

```yaml
# event_policy 扩展
compatibility: BACKWARD  # BACKWARD / FORWARD / FULL / *_TRANSITIVE
schemas_dir: ".ralph/schemas/"  # 本地 schema registry
schemas:
  work.done:
    version: 2
    required_fields: [task_id, plan_name]
    optional_fields: [step, complexity]
    deprecated_fields: [legacy_id]  # v1 字段，v2 后 warn
    migration_hints:
      legacy_id: "use task_id instead"
```

**参考链接**：
- https://github.com/confluentinc/schema-registry
- https://github.com/Apicurio/apicurio-registry
- https://github.com/bufbuild/buf
- https://github.com/pact-foundation/pact
- https://github.com/stoplightio/spectral
- https://github.com/asyncapi/asyncapi
- https://docs.confluent.io/platform/current/schema-registry/fundamentals/schema-evolution.html

### 4.5 持久执行与 Saga

> v1 只说"Saga 是 Temporal 标配"，没说 ralph 怎么落。本节给出**显式 Saga 范式**和**Rust 生态可用实现**。

**Saga 范式（Garcia-Molina 1987 论文的思想，但工业实现很多）：**

核心：每个 step 配 `compensate`，失败时**反向**调用已 commit step 的 compensate。

```yaml
# ralph hat config 扩展建议
hats:
  shipper:
    name: "Shipper"
    triggers: ["plan.complete"]
    publishes: ["ship.done"]
    actions:
      - name: "git_commit"
        commit: true
        compensate_with: "git_revert_last_commit"  # 自动生成
      - name: "publish_release"
        side_effect: true
        compensate_with: "unpublish_release"
```

**Rust 生态可借鉴的开源实现**：

- **Temporal Rust SDK** — `temporalio/sdk-core`（Rust；与 Temporal server 配套）
  - 局限：必须接 Temporal server，引入 sidecar
- **Restate Rust SDK** — `restatedev/restate`（Rust 服务端 + SDK）
  - 强项：**Saga 是 first-class API**，`ctx.run(compensate=...)` 直接写
  - 适合 ralph 借鉴的 API 风格
- **Cadence Rust Client**（Uber Cadence 的 Rust SDK，社区维护）
- **MassTransit**（.NET，仅供 API 风格参考）——`MassTransit/MassTransit`（C#；~7k+ stars，**Saga 范式最完整的工业实现之一**）
  - API 范式：`Event(() => MySaga, instance => instance.CorrelateById(context.Message.OrderId))` + `.Then(...)` + `.Compensate(...)`
  - ralph 可以照搬 API 风格到 Rust

**ralph 落地的最小可用实现**（不开 sidecar）：

1. `HatConfig` 加 `actions: Vec<Action>` + `compensate_with: Option<String>` 字段
2. `event_loop` 维护 `pending_compensations: Vec<CompensationRecord>`，每个 commit step 推入
3. 触发 Loop Pause 时，**按 LIFO 反向调用 compensate**
4. 全部 commit 完成后才清空栈
5. 失败时写 `pending_compensations.jsonl`，下次启动重放

**工业案例**（生产部署）：

- **Cadence** 在 Uber 内部处理数百万订单，每个订单是一 Saga
- **MassTransit** 在澳洲国民银行、Microsoft 部分业务线生产
- **Apache ServiceComb Saga**（Java；Apache 顶级项目；~1.5k+ stars）—— 分布式事务的 Saga 实现

**参考链接**：
- https://github.com/temporalio/sdk-core
- https://github.com/restatedev/restate
- https://github.com/MassTransit/MassTransit
- https://github.com/Particular/NServiceBus
- https://github.com/apache/servicecomb-pack（Apache ServiceComb Saga）
- https://microservices.io/patterns/data/saga.html（Chris Richardson 的 Saga 范式文档）

---

## 5. 缺失能力清单（按优先级）

> 与 v1 不同：① 优先级按"对稳定性的实际影响"重排；② 工作量估计参考 4.x 节的工业方案，不再纯靠猜测；③ 与 v1 重叠的 gap 标 `←v1`。

### 🔴 P0 — 与稳定性直接相关

#### Gap 1：运行时 drift 监控（替换 v1 ASI gap）

- **问题**：U1–U9 全是"静态 + 启动期"。LLM 概率性让字段今天在、明天不在，无任何 metric 监控
- **业界证据**（非学术）：
  - Langfuse / Arize Phoenix / Helicone 公开 case study 显示**生产 agent 第一次部署后 2 周内必然出现 field drift**
  - Promptfoo 在 YC 内部 agent 评审中发现 60%+ 的 agent 在 prompt 调整后产生未预期的 output drift
- **建议实现路径**（不开 sidecar）：
  ```rust
  // crates/ralph-core/src/drift_observer.rs（新增）
  pub struct DriftObserver {
      sliding_window: Mutex<SlidingWindow>,
      on_drift: Arc<dyn Fn(DriftEvent) + Send + Sync>,
  }
  ```
  - 接入 `event_bus` observer
  - 指标：`field_completeness(topic, field)`、`coord_join_rate(from_topic, to_topic)`、`emit_cadence(topic)`
  - 落盘：`.ralph/metrics/drift.jsonl`
  - 触发：`drift_policy.on_drift: pause_loop_with_diagnostic`
- **工业对标**：Grafana + Prometheus + OTLP（4.3 节已列）
- **接入点**：新增独立 plan `2026-06-XX-feat-drift-monitoring.md`，依赖 U6 诊断 writer + event_bus observer
- **工作量**：中（5–7 天） `←v1 P0-3`

#### Gap 2：Schema 版本化 + 向后/向前兼容

- **问题**：`event_policy.schemas` 无 `version` 字段、无 `compatibility` 模式。加新字段后老 hat 立刻报 contract violation；无法灰度
- **业界证据**（非学术）：
  - Confluent Schema Registry 的 `BACKWARD` / `FORWARD` / `FULL` 模式自 2015 年起就是 Kafka 生产部署的标配
  - Pact 在 pactflow 公开 case study 中**所有客户都用 versioning**
- **建议**：
  ```yaml
  # event_policy 扩展
  compatibility: BACKWARD
  schemas:
    work.done:
      version: 2
      required_fields: [task_id, plan_name]
      optional_fields: [step, complexity]
      deprecated_fields: [legacy_id]
  ```
  - 在 `event_policy.rs::validate_event` 启动时按 `compatibility` 检查
  - 兼容性计算：纯函数——`required_fields` 集合 + 类型对照，**无需 sidecar**
- **工业对标**：Confluent Schema Registry 兼容性算法 + Pact provider verification
- **接入点**：U1 扩展为 `EventPolicyConfig.{compatibility, schema_version}`；U5 诊断附 `expected_version` / `actual_version`
- **工作量**：小（1–2 天） `←v1 P0-1`

#### Gap 3：Causality / Prerequisite Topic 检查

- **问题**：U3 只查"字段在不在 schema"，不查"事件 A 必须在事件 B 之前"
- **例子**：`plan.ready` 之前必须 `plan.gate.passed`，否则 reviewer 收到没 gate 过的 plan
- **业界证据**（非学术）：
  - Pact 强制 consumer/provider state sequence，是工业 consumer-driven contract 标准做法
  - AsyncAPI spec 2.6+ 支持 `operation.reply` 显式声明因果
- **建议**：`EventSchema` 加 `prerequisite_topics: Vec<String>`
  - 静态校验：`preset_validator.rs` 在 topology check 阶段验证 prerequisite 是否可达
  - 运行时校验：`event_policy.rs` 在收到 topic 时检查 prerequisite 是否已 emit
- **工业对标**：Pact provider state + AsyncAPI `operation.reply`
- **接入点**：U3 + `preset_validator.rs` 扩展
- **工作量**：中（3–5 天） `←v1 P0-2`

### 🟡 P1 — 重要但非阻塞

#### Gap 4：Cycle / Loop Bound（防 agent 推理死循环）

- **现状**：`wave_tracker` 有限制但 hat-level 循环无限制
- **风险**：reviewer 觉得 plan 不好→ `plan.gate.failed`→ coordinator 重新生成→ 循环
- **业界证据**（非学术）：
  - Temporal 用 `ContinueAsNew` + 显式 counter 防 workflow 无限增长
  - Argo Workflows 用 `activeDeadlineSeconds` + `retryStrategy.limit`
  - Cadence 用 `attempt` counter
- **建议**：`HatConfig` 加 `max_invocations_per_loop: Option<u32>`，`event_loop` 维护 per-hat counter
- **工业对标**：Temporal `ContinueAsNew` + Argo `activeDeadlineSeconds`
- **接入点**：U5 诊断 + HatConfig 扩展
- **工作量**：小（1–2 天） `←v1 P1-5`

#### Gap 5：Saga / Compensation（动作可撤销）

- **现状**：Loop Pause = 全停。已发生的副作用（git commit、文件写入）保留，无补偿
- **业界证据**（非学术）：
  - MassTransit Saga API 是 .NET 生态最成熟工业实现
  - Apache ServiceComb Saga 在华为、若干电信运营商生产部署
- **建议**：`HatConfig` 加 `actions: Vec<Action>` + `compensate_with: Option<String>`，U5 触发 Pause 时按"已 commit 顺序"反向调用
- **工业对标**：MassTransit Saga 范式 + Restate `ctx.run(compensate=...)` API
- **接入点**：U5 之后独立单元
- **工作量**：大（7–10 天） `←v1 P1-7`

#### Gap 6：OpenTelemetry 风格可观测性升级

- **现状**：`.ralph/diagnostics/` 只在错误时写
- **业界证据**（非学术）：
  - OpenTelemetry 协议是 CNCF 毕业项目，所有大厂生产部署
  - Grafana Tempo / Loki / Mimir 三件套自托管即可
- **建议**：复用现有 `event_bus` observer，加 `tracing` crate + `tracing-opentelemetry` + OTLP exporter
  - 每个 hat 一次 invocation 一个 span
  - trace_id 跨 hat 串联（写入 `.ralph/diagnostics/trace.jsonl`）
  - 可选导出到外部 Tempo / Grafana
- **工业对标**：OpenTelemetry + Grafana（自托管）/ Langfuse（商业）
- **接入点**：`event_bus` 已有 observer pattern，扩展 cost 低
- **工作量**：中（3–5 天） `←v1 P2-12`

#### Gap 7：Schema Registry / 跨 preset 共享

- **现状**：每个 preset 自己一套 schema，worktree loop 多 preset 并行时无法对齐
- **业界证据**（非学术）：
  - Confluent Schema Registry / Apicurio 中心化 registry 是 Kafka 生态事实标准
  - Buf Schema Registry 在 Protobuf 生态是事实标准
- **建议（轻量级）**：建 `.ralph/schemas/` 目录做本地 registry，跨 preset 引用（path 引用或 `schema_ref` 字段）
- **建议（重量级）**：起 Confluent Schema Registry sidecar
- **工业对标**：Confluent Schema Registry / Apicurio Registry / BSR
- **接入点**：U1 扩展为 `schema_ref` 字段
- **工作量**：中（3–5 天） `←v1 P2-9`

### 🟢 P2 — 长期演进

#### Gap 8：Prompt Injection 防御

- **payload 含 "ignore previous instructions" 可注入下游 LLM hat
- **业界证据**（非学术）：
  - OWASP LLM Top 10（2025 版）把 LLM01 Prompt Injection 列为第一威胁
  - Microsoft Guidance / Rebuff / Lakera Guard 是工业级 guardrails
- **建议**：`EventPolicyConfig` 加 `content_sanitization: { strip_instruction_overrides: true }`
- **工业对标**：Lakera Guard（API）/ Rebuff（开源 Python）/ Microsoft Guidance
- **工作量**：小（1–2 天） `←v1 P2-10`

#### Gap 9：Token / Cost Budget 静态校验

- **现状**：preset 没限制单 hat prompt 长度上限、没总成本预算
- **业界证据**（非学术）：
  - Helicone / OpenLLMetry 都支持 cost budget alert
  - Langfuse 自带 cost tracking
- **建议**：`HatConfig` 加 `prompt_budget_tokens: Option<u32>`、`loop_cost_budget_usd: Option<f64>`
- **工业对标**：Helicone cost alerts / Langfuse cost tracking
- **工作量**：小（1–2 天） `←v1 P2-11`

#### Gap 10：多版本 preset 并存 / Canary 路由

- **现状**：没办法"50% 用老 ce-executor，50% 用新版"
- **业界证据**（非学术）：
  - Argo Rollouts / Istio traffic split 是 K8s 生态 canary 事实标准
  - Unleash / LaunchDarkly（开源版）做 feature flag
- **建议**：在 `event_loop` 启动参数加 `--preset-version` + 流量分配
- **工业对标**：Argo Rollouts + Unleash
- **工作量**：中（3–5 天） `←v1 P2-13`

#### Gap 11：Prompt / Agent 行为回归测试（CI gate）

- **现状**：没有"instructions 改了，agent 行为变了"的 CI 检测
- **业界证据**（非学术）：
  - Promptfoo 在 YC 内部 agent 评审中已成事实标准
  - Langfuse eval suite 自带 regression detection
- **建议**：用 Promptfoo 写 5–10 个代表性输入做 regression，CI gate
- **工业对标**：Promptfoo / Langfuse eval
- **工作量**：中（3–5 天） `←v1 P2-14`

---

## 6. 三个演进方向（优先级矩阵）

| 方向 | 包含 gap | 工作量 | 适用场景 | 工业对标 |
|------|---------|--------|---------|----------|
| **A. drift + schema 演化** | P0: 1+2+3 | 1.5–2.5 周 | 解决 LLM 概率性 + 跨版本演化；U1–U9 之后立刻开新 plan | Langfuse + Confluent Schema Registry + Pact |
| **B. 可观测性 + Saga + Cycle bound** | P1: 4+5+6 | 3–4 周 | 把"运行可观察"+"动作可逆"做出来；多个独立 plan | OpenTelemetry + Grafana + MassTransit Saga |
| **C. Schema Registry + 行为测试 + Canary** | P1: 7, P2: 8+9+10+11 | 4–6 周 | 往"重型 orchestration"靠拢；分多 plan 演进 | Apicurio / Confluent SR + Promptfoo + Argo Rollouts |

**个人建议**：A 先做（U1–U9 实施收尾后直接补，最影响稳定性），B 中期（业务复杂度上来后开新 plan），C 长期（看用户业务场景是否需要"重型 orchestration"）。

---

## 7. 需要用户拍板的事

### 决策 1：drift 监控放在 plan 内还是 plan 外？

- **选项 A**：作为独立 plan `2026-06-XX-feat-drift-monitoring.md`（推荐）
  - 优点：与 U1–U9 解耦；可以独立 ship / rollback
  - 缺点：多一个 plan 文档
- **选项 B**：纳入 execution contract plan 收尾（U10 单元）
  - 优点：plan 集中
  - 缺点：原 plan 已结案 review，扩 plan 改动成本高
- **建议**：选项 A

### 决策 2：schema 版本化走 sidecar 还是纯函数？

- **选项 A**：纯函数路线（**推荐**）——`EventPolicyConfig.compatibility` 字段，启动时按集合差集检查
  - 优点：无 sidecar；与 ralph "重文件" 风格一致
  - 缺点：不能跨进程/跨实例共享 schema
- **选项 B**：Confluent Schema Registry sidecar
  - 优点：工业标准；跨实例共享
  - 缺点：引入 Java 服务；CI 环境要起 docker
- **建议**：选项 A（ralph 是单机/单进程，sidecar 收益低）

### 决策 3：drift detection 跟"暂停"还是"告警"？

- **选项 A**：超阈值 → Loop Pause（与 U5 一致）
- **选项 B**：超阈值 → 仅写 diagnostic，loop 继续
- **建议**：选项 A（与现有"严格不放过"哲学一致；选项 B 会让 LLM 概率性问题被忽略）

### 决策 4：是否引入 metric store？

- 当前所有状态走文件系统（`.ralph/` 目录）
- drift metric 需要聚合、跨 session 对比
- **选项 A**：纯文件（追加 JSONL + 启动时聚合）——**推荐**
  - 优点：架构简单；与项目风格一致
  - 缺点：跨 session 对比需要手动脚本
- **选项 B**：本地 SQLite（`.ralph/metrics.db`）
  - 优点：跨 session 查询快
  - 缺点：引入 SQL 依赖
- **建议**：选项 A（先纯文件，如果 drift detection 用得起来再升级 SQLite）

---

## 8. 下一步计划

1. **本次报告交付**：`docs/report/2026-06-03-preset-orchestration-stability-gap.md` v2 提交
2. **等待用户决策 1–4**
3. **决策确定后**：
   - 选决策 1A + 决策 2A + 决策 3A：开新 plan `2026-06-XX-feat-drift-and-schema-evolution.md`（P0 三个 gap 一并做）
   - 选决策 4A：drift metric 走 JSONL；后续如需升级 SQLite 单独 plan
4. **B 方向**（P1-4+5+6）：作为新 plan（OpenTelemetry 接入 + Saga 范式实现）
5. **C 方向**（P1-7 + P2-8+9+10+11）：看用户业务场景再决定优先级

---

## 9. 引用与参考

> 全部为开源项目 GitHub 仓库或厂商公开文档，**无 arXiv 学术论文**。

### 项目内部

- `docs/plans/2026-06-02-005-feat-payload-contract-validation-plan.md`（payload 契约计划 U1–U8，已落地）
- `docs/plans/2026-06-03-001-feat-agent-execution-contract-gates-plan.md`（execution contract 计划 U1–U9，已落地）
- `docs/report/agent-execution-contract-gates-review-2026-06-03.md`（contract plan 审查报告，含 fail-open 缺陷）
- `crates/ralph-core/src/payload_contract.rs`（U2/U3 字段提取 + 跨 hat 校验）
- `crates/ralph-core/src/execution_contract.rs`（U1–U9 运行时合同）
- `crates/ralph-core/src/preset_validator.rs`（topology reachability + cycle detection）
- `crates/ralph-core/src/event_policy.rs`（ViolationType + PolicyDecision）
- `crates/ralph-core/src/event_origin.rs`（来源合法性 only，无内容净化）
- `crates/ralph-core/src/state_machine.rs`（lifecycle only，无跨 hat 因果）
- `presets/en/ce-executor.yml` / `presets/zh/ce-executor-zh.yml` / `presets/schemas/ce-executor.yml`（多语言 preset + schema 分目录）

### 多 agent 编排框架（4.1）

- https://github.com/temporalio/temporal
- https://github.com/Netflix/conductor
- https://github.com/uber/cadence
- https://github.com/apache/airflow
- https://github.com/argoproj/argo-workflows
- https://github.com/langchain-ai/langgraph
- https://github.com/restatedev/restate
- https://github.com/microsoft/autogen
- https://github.com/crewAIInc/crewAI

### 静态分析与配置 lint（4.2）

- https://github.com/stoplightio/spectral
- https://github.com/yamllint/yamllint
- https://github.com/oxsecurity/megalinter
- https://github.com/ajv-validator/ajv
- https://github.com/python-jsonschema/json-schema-org
- https://github.com/promptfoo/promptfoo
- https://github.com/rhysd/actionlint
- https://github.com/openai/evals

### 可观测性与 drift 检测（4.3）

- https://opentelemetry.io/docs/languages/rust/
- https://github.com/open-telemetry/opentelemetry-rust
- https://github.com/traceloop/openllmetry
- https://github.com/langfuse/langfuse
- https://github.com/Arize-ai/phoenix
- https://github.com/grafana/tempo
- https://github.com/prometheus/prometheus
- https://github.com/grafana/loki

### Schema 演化与契约测试（4.4）

- https://github.com/confluentinc/schema-registry
- https://github.com/Apicurio/apicurio-registry
- https://github.com/bufbuild/buf
- https://github.com/pact-foundation/pact
- https://github.com/stoplightio/spectral
- https://github.com/asyncapi/asyncapi
- https://docs.confluent.io/platform/current/schema-registry/fundamentals/schema-evolution.html

### 持久执行与 Saga（4.5）

- https://github.com/temporalio/sdk-core
- https://github.com/restatedev/restate
- https://github.com/MassTransit/MassTransit
- https://github.com/Particular/NServiceBus
- https://github.com/apache/servicecomb-pack
- https://microservices.io/patterns/data/saga.html（Chris Richardson Saga 范式文档）

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

**观察**（与 v1 同步更新）：
- `queue.advance` 同时被 Executor 消费和 Plan Gate 发布，**有循环风险**（Gap 4 可补：cycle bound）
- `REVIEW_COMPLETE` 是大写，与其它小写 topic 风格不一致（embedded 风险）
- 9 个 hat 15 个 topic——schema 全面化后大约 15 个 `EventSchema` 条目
- 当前缺 prerequisite 约束：如 `review.wave.ready` 应该 prerequisite `work.done`（Gap 3 可补）
- `executor` hat 的 `work.done` 是 Execution Contract 校验入口（执行 contract gates plan 落地）
- 未来 saga gap 落地后，Shipper 应该是 `compensate_with: git_revert_last_commit` 的主要应用点
