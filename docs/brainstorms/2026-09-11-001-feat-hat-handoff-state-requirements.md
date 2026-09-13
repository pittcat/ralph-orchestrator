---
title: Hat 间交接状态增强需求(并发场景)
type: requirements
date: 2026-09-11
topic: hat-handoff-state
artifact_contract: ce-unified-plan/v1
artifact_readiness: requirements-only
product_contract_source: ce-brainstorm
execution: code
---

# Hat 间交接状态增强需求(并发场景)

## Goal Capsule

- **目标**:记录当前 Ralph 在 isolated / supervisor / DAG 执行面下,hat A 结束、hat B 被触发时状态交接机制的已确认缺口,并为后续规划提供优先级、源码证据和需求边界。
- **文档性质**:这是缺口审计与需求记录,不是实现计划,不规定模块拆分、代码顺序、数据结构或具体迁移方案。
- **权威边界**:当前源码中已经存在的交接通道(事件 payload、TRIGGER CONTEXT、handoff_envelope、memories、tasks、DAG JobContext)是现状事实;preset yml 中的 artifact-first 约定是 preset 层契约,不构成 runtime 级保证。

## Summary

isolated 模式下 hat 之间的交接只有四条通道,全部经磁盘 + runtime 注入:

1. **事件 payload**(trigger event 原文进下一个 hat 的 prompt,≤50KB,单业务事件预算);
2. **memories**(`.ralph/agent/memories.md`,append-only,hat-scoped 可见性过滤);
3. **tasks**(`.ralph/agent/tasks.jsonl`,只承载生命周期);
4. **prompt 注入链**(runtime 从 ledger/schema 派生的只读视图块,含可选的 `## TRIGGER CONTEXT` 提炼与 `handoff_envelope` 强类型交接)。

DAG 模式另有 runtime 注入的 typed `JobContext` env 通道。

对抗性审查后的结论是:问题不是"没有交接通道",而是这些通道各自解决不同问题且没有形成统一的**交接契约**——业务数据靠 payload + artifact 路径约定,工作态没有专属载体(memories 是跨 loop 知识库而非 per-loop 工作态),强类型交接(handoff_envelope)只在串行 preset 且默认关闭,DAG 的证据引用是占位 digest 无内容级校验。并发场景(多 Unit 并行 producer → 下游聚合 consumer)放大了这些缺口。

## Priority Meaning

| 级别 | 含义 |
|---|---|
| P0 | 可能造成下游 hat 基于缺失/不可验证的上游证据推进,必须先解决。 |
| P1 | 会让交接语义失真或被迫靠 prompt 约定兜底,导致并发编排成本上升或重复失败。 |
| P2 | 已有局部能力但缺少 per-loop / per-unit / 更新语义等通用维度。 |
| P3 | 影响长期演进与一致性收尾,不是第一阶段正确性的阻断点。 |

## Gap Register

### P0/P1 — 交接契约与证据可验证性

#### GAP-01:hat 间交接没有 per-loop / per-unit 的"工作态"载体

- **当前状态**:memories 是跨 loop、跨 session 的全局知识库(4 个固定 section:Patterns/Decisions/Fixes/Context),可见性只有 `Shared` / `Private(owner_hat)` 二级;写入方是 hat 自愿 `ralph tools memory add`,无强制门禁;读取方是下一个 hat 的 prompt 前缀(auto 注入,`load_visible(caller_hat_id)` 过滤)。
- **源码证据**:`crates/ralph-core/src/memory.rs:61-99`(固定 section)、`crates/ralph-core/src/memory.rs:172-200`(Memory 模型)、`crates/ralph-core/src/memory.rs:249-257`(`is_visible_to`)、`crates/ralph-core/src/memory_store.rs:80-86`(`load_visible` SSOT)、`crates/ralph-core/src/event_loop/prompt_injection.rs:542`(auto 注入经 hat 过滤)。
- **缺少什么**:没有 loop-scoped / unit-scoped 的工作态层——一种"本次 loop 内、本 Unit 上下游之间"的中间状态载体,生命周期绑定 loop(loop 结束归档或清理),支持按 key upsert(而非 append-only)。当前上游 hat 想把"我做了一半、结论草稿、中间假设"交给下游,只能塞进 payload(受 50KB + 单事件预算限制)或写进全局 memories(污染跨 loop 知识库,且 append-only 无法更新)。
- **风险**:并发场景下多个 Unit 的并行 producer 若共用全局 memories 写工作态,既互相污染又被预算粗暴截断(`memory_store.rs:389-423` 按 4 chars/token 字符截断);若全靠 payload 传,又撞上 OPAC 单业务事件预算与字段白名单。下游 hat 实际能拿到的上游中间态接近零,只能靠"trigger payload + 自己重新探查代码树"。
- **需求记录**:建立 loop-scoped(必要时 unit-scoped)的工作态存储:生命周期绑定 loop,支持按 key upsert/读取,prompt 注入按触发 hat 的拓扑位置(上游产出)过滤;与全局知识 memories 明确分层(工作态 ≠ 知识库),loop 终态时由 runtime 决定归档/清理。
- **边界**:本缺口只要求分层语义与生命周期,不在此文档决定存储格式(文件/表)或注入预算策略。

#### GAP-02:强类型交接契约(handoff_envelope)只在串行 preset 且默认关闭,并发执行面无等价物

- **当前状态**:`handoff-envelope.v1` schema 已存在——payload 顶层 `handoff_envelope` 字段,含 root_goal/plan/state/receiver_contract(to_hat、success/failure signal、must_do),按 `to_hat` 过滤后注入接收方 prompt;但只在 preset 显式声明 `handoff_envelope.enabled` 时生效,且现有接线面向串行 preset。
- **源码证据**:`crates/ralph-core/src/handoff_envelope.rs:28-52`(envelope 模型)、`crates/ralph-core/src/event_loop/event_processing.rs:1761-1773`(注入点)、`crates/ralph-core/src/event_loop/event_processing.rs:1693-1773`(prompt 注入链全景)。
- **缺少什么**:DAG / wave 并发模式下没有等价的 receiver contract——下游 hat(尤其是聚合型 consumer,如 reporter / integrator / auditor)拿不到"上游对我承诺的成功/失败信号是什么、我必须做什么"的强类型声明;当前只能靠 preset yml 里自然语言写的 artifact-first 约定(`presets/en/parallel-forge.yml:506`、`:1136-1161`)约束 agent 行为,runtime 不校验。
- **风险**:并发场景下上游 producer 的 payload 形状漂移(少字段、错语义)时,下游只能"读到什么算什么";报告型 hat 被要求 artifact-first 但缺证据时只能写 "Information missing",没有 runtime 级拒收/打回机制把交接缺口暴露在上游。
- **需求记录**:交接契约(谁交给谁、成功/失败信号、必做项、证据引用)应成为所有执行模式的 runtime 级一等能力,而不是串行 preset 的 opt-in;DAG 模式下 receiver contract 应能由 runtime 从 stage 必填字段表(`job_context.rs:20-72`)派生,而非完全依赖 producer 自觉填写。
- **边界**:不要求消灭 preset 层的自然语言约定;runtime 契约是底线,preset 约定是增强。不规定 envelope 与现有 `## TRIGGER CONTEXT` 的合并方式。

#### GAP-03:DAG artifact_refs 未完整注入子进程 env,且 digest 是占位符、无内容级校验

- **当前状态**:DAG 模式每 stage 的 `JobContext.artifact_refs` 是 typed `BTreeMap<String, ArtifactRef{path, digest}>`,`validate_context` 在 spawn 前 fail-closed 校验 key 存在性;但:(a) env 注入的 `RALPH_DAG_ARTIFACT_REFS` 只序列化了 `verified_execution_plan_path` 一项,review/verify/fix 阶段的 executor/reviewer completion artifact、fingerprint、correction digest 进了 JobContext 用于校验却没有完整进入子进程 env;(b) Review/Verify 阶段 digest 是占位符 `"runtime-accepted"`,Fix 阶段是 `"runtime-correction"`,不是真实内容 digest,`DigestMismatch` 分支当前只查"非空"。
- **源码证据**:`crates/ralph-cli/src/loop_runner/dag_scheduler/job_context.rs:76-95`(JobContext/ArtifactRef 模型)、`job_context.rs:112-171`(`validate_context`)、`crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs:1702-1780`(spawn 边界构造+校验)、`spawn.rs:1824-1836`(env 只序列化单项)、`spawn.rs:1725,1739,1749`(占位 digest)。
- **缺少什么**:交接证据(env 可见性 + 内容可验证性)两层都缺:hat 子进程拿不到完整的上游 artifact 引用集,只能靠 prompt 里的 feedback 路径;runtime 也不校验 artifact 内容是否被篡改/漂移。
- **风险**:execute → review → fix 的证据链是"文件路径 + 占位 digest",fixer 拿到的失败证据可能是过期或被改写过的文件而 runtime 无感知;并发下多 attempt 复用路径时无法区分"哪个 attempt 的证据"。
- **需求记录**:DAG 交接证据必须 (a) 完整注入子进程可见面(env 或等价机制),(b) digest 必须是真实内容摘要并在消费侧校验,(c) 与 `resource_namespace`(plan|unit|stage|attempt)绑定,保证并发 attempt 间证据不串台。
- **边界**:本缺口只要求注入完整性与 digest 真实性,不决定摘要算法或篡改后的处置策略(拒收/降级/告警)。

#### GAP-04:payload 的结构化提炼(TRIGGER CONTEXT)完全 opt-in,并发下无多 producer 聚合语义

- **当前状态**:`## TRIGGER CONTEXT` 块由 preset schema 的 `trigger_context` 声明驱动,只渲染声明的 `summary_fields` + 条件 `routing_hints`,未声明字段即使存在也不注入(payload-leakage guard);未声明时下游 hat 只有 `format_event` 渲染的非结构化 `Event: <topic> - <payload>` 文本。
- **源码证据**:`crates/ralph-core/src/trigger_context.rs:18-38`(提炼+防泄漏)、`crates/ralph-core/src/config/loop_config.rs:157`(`TriggerContextConfig`)、`crates/ralph-core/src/event_loop/prompt_injection.rs:763-781` + `event_processing.rs:1743`(接线点)、`crates/ralph-core/src/event_loop/dispatch_and_handoff.rs:190-197`(`format_event` 原始渲染)。
- **缺少什么**:(a) 提炼层完全靠 preset 作者自觉声明,builtin preset 之外几乎没有覆盖;(b) 并发场景下一个 consumer hat 可能被多个 producer(多 Unit 的同类事件)触发/聚合,当前没有"N 个上游 payload 如何合并成一份 trigger context"的语义——每个触发独立 activation,consumer 无法在同一 activation 里看到上游全景。
- **风险**:聚合型 hat(reporter/auditor/integrator)在并发 fan-in 时被迫跨多次 activation 拼凑上游结果,或退化到"扫描 artifact 目录自行发现"(违反 artifact-first 的路径必须由 trigger 传递的规则)。
- **需求记录**:交接提炼应有 runtime 级默认(至少终态/交接类 topic 自带 summary 契约,而非全靠 preset 声明);fan-in 场景需要定义"聚合 trigger"语义——runtime 能把同一阶段多个 producer 的结果以结构化形式一次性交给 consumer。
- **边界**:不要求所有 topic 都强制 summary;不要求改变"一次 trigger 一次 activation"的基本调度模型,聚合语义由 runtime 在 trigger 生成侧完成。

### P2 — 工作态与知识库的分层维度

#### GAP-05:memories 无更新/合并语义(append-only + delete),无 loop/unit 维度

- **当前状态**:`append()` 独占文件锁插 section 头部,`delete()` 解析后整文件重写;没有 update/upsert;id 是 `mem-{ts}-{hex}` 随机生成,无业务 key;过滤维度只有 type/tags/recent,不能按 loop/plan/unit 过滤。
- **源码证据**:`crates/ralph-core/src/memory_store.rs:162-214`(append/delete)、`crates/ralph-core/src/memory.rs:172-200`(id 模型)、`crates/ralph-core/src/config/memories.rs`(budget/filter 配置)。
- **缺少什么**:同一条知识的演进(假设 → 确认 → 修正)只能"再加一条",旧条目成为噪音;loop 结束后本 loop 的工作态 memory 与跨 loop 知识混在一起,无法批量生命周期管理。
- **风险**:长期运行后 prompt 注入预算被过期/重复条目消耗,粗暴截断可能切掉的正是最新结论;并发 hat 各自 append 相似内容,无去重。
- **需求记录**:memory 需要按业务 key 的 upsert 语义(同 key 覆盖/演进,保留历史版本可审计);loop/unit 维度的归属标记与生命周期(与 GAP-01 的工作态层配套:工作态层消亡,知识层沉淀)。
- **边界**:不要求全文检索/向量检索等查询能力升级;不决定版本保留策略。

#### GAP-06:per-hat memory 是"单文件 + 过滤层"软隔离,并发写靠整文件锁重写

- **当前状态**:private 隔离由 `is_visible_to` 过滤实现,任何人手读 `.ralph/agent/memories.md` 可见全部;并发安全靠 FileLock shared/exclusive + 整文件重写;曾发生注入路径绕过过滤直接 `load()` 导致跨 hat private 泄漏(plan 2026-09-01-2102 U5 已修,固化测试 `event_loop/tests/memory_visibility.rs:1-20`)。
- **源码证据**:`crates/ralph-core/src/memory_store.rs:1-15`(FileLock)、`crates/ralph-core/src/memory_store.rs:80-86`(`load_visible`)。
- **缺少什么**:过滤层是单点纪律——任何新消费路径(CLI、注入、未来 API)绕开 `load_visible` 即泄漏;整文件重写在高并发写下是序列化瓶颈且放大写放大。
- **风险**:随并发度上升(DAG 多 job 同时 activation),memory 写冲突/锁等待成为隐形瓶颈;新增消费路径时的泄漏回归只能靠测试防守。
- **需求记录**:memory 的可见性应在存储层强制(分文件/分表/等价机制),而非每个消费点自觉走过滤函数;写入路径应支持并发友好的粒度(不必整文件锁)。
- **边界**:与 GAP-05 的 upsert 语义可合并设计,但本缺口只要求隔离与并发写语义,不要求改变 agent 侧 CLI 形状。

#### GAP-07:tasks 不承载业务数据,supervisor/DAG 投影行 worker 只读,交接语义缺位

- **当前状态**:task 只有 `description` 一个自由文本字段,定位是"谁在做什么/做完没"的生命周期协调;supervisor/DAG 模式下 runtime(dispatcher/projector)是唯一写方,worker 禁止直接碰 tasks.jsonl;跨 loop 的 start/close/fail/reopen 直接拒绝。
- **源码证据**:`crates/ralph-core/src/task.rs:144-172`(Task 模型)、`crates/ralph-core/data/ralph-tools-tasks.md:19-27`(supervisor 投影只读)、`:80-113`(跨 loop/hat 授权矩阵)。
- **缺少什么**:task 作为交接载体是空位——上游 hat 完成 task 时,除 status 翻转外没有结构化的"产出摘要/证据指针"挂在 task 上;下游从 `<ready-tasks>` 注入块只能看到任务存在,看不到完成内容。
- **风险**:hat 想表达"我这个 task 做完了、产出在哪、下游该看什么",只能绕回 payload/artifact 约定;task 系统与交接系统完全脱节。
- **需求记录**:task close 时应能附着结构化的完成摘要(产出 artifact 引用、结论一句话、给下游的指针),并在下游 hat 的 ready-tasks/trigger 注入中可见;supervisor 模式下由 runtime 在投影时填充。
- **边界**:不把 task 变成第二个 payload 通道;摘要字段应有大小上限并引用 artifact 而非内嵌内容。

### P3 — 并发收尾一致性

#### GAP-08:DAG terminal delivery(exactly-once close ack)仍是骨架,未接线生产路径

- **当前状态**:`terminal_delivery.rs` 定义了 `DeliveryState: Prepared→Appending→Delivered/Blocked` 与 `DeliveryKey = plan_key|topic|artifact_digest`,但文件自述 SKELETON-ONLY,未接线生产路径;v20 `dag_terminal_deliveries` 表已落地。
- **源码证据**:`crates/ralph-cli/src/loop_runner/dag_scheduler/terminal_delivery.rs:9-12`(skeleton 声明)、`:17-57`(状态机)、`crates/ralph-core/src/supervisor/migrations.rs:712`(v20)。
- **缺少什么**:终态交接(全部 Unit ack → 恰好一次 `forge.exec.development.done`)的生产化接线。
- **风险**:在接线完成前,终态事件的恰好一次语义依赖过渡期路径,交接链的最后一环没有与上游相同的强保证。
- **需求记录**:terminal delivery 应按既有设计接线生产路径,使终态交接与 GAP-03 的真实 digest 形成端到端一致的证据链。
- **边界**:不重新设计 delivery 语义,只要求现有骨架的生产化。

## Gap Priority Summary

| Gap | 优先级 | 主题 |
|---|---|---|
| GAP-01 | P1 | per-loop/per-unit 工作态载体缺失 |
| GAP-02 | P1 | 强类型交接契约未覆盖并发执行面 |
| GAP-03 | P0 | DAG artifact_refs 注入不完整 + 占位 digest |
| GAP-04 | P1 | 结构化提炼 opt-in + 无 fan-in 聚合语义 |
| GAP-05 | P2 | memory 无 upsert / 无 loop 维度 |
| GAP-06 | P2 | per-hat memory 软隔离 + 整文件锁 |
| GAP-07 | P2 | task close 无完成摘要附着 |
| GAP-08 | P3 | terminal delivery 骨架未生产化 |

## Existing Strengths Not to Reclassify as Gaps

- **artifact-first 约定**:上游写版本化 repo-relative artifact、payload 只传路径、reporter 禁扫 event history(`presets/en/parallel-forge.yml:506`、`:1136-1161`,AGENTS.md HARD RULE 第 11 条)——这是已验证的正确方向,缺口在于它是 preset 层约定而非 runtime 保证(见 GAP-02),不在于方向本身。
- **OPAC + policy-check 两步式**:emit 前的同源 schema 预检 + token 绑定(`crates/ralph-cli/src/policy_check.rs`)已解决"发出去的交接事件字段不合规"问题。
- **JobContext spawn 前 fail-closed**:`validate_context` 缺字段即 `ContractViolation` 拒绝 spawn(`spawn.rs:1702-1780`),身份围栏(job_token/attempt)已防串台。
- **payload-leakage guard**:TRIGGER CONTEXT 的"未声明不注入"方向正确,缺的是覆盖率和聚合语义(GAP-04),不是机制。
- **memory 可见性修复**:`load_visible` 过滤已固化测试(`memory_visibility.rs`),GAP-06 要求的是存储层强制,不是重复修同一 bug。

## Requirements Boundary

### 本文要记录的内容

- hat 间交接通道的现状事实与已确认缺口;
- 并发(wave/DAG)场景下交接语义的需求边界;
- 各缺口的优先级与源码证据。

### 本文不记录的内容

- 工作态层的存储实现(文件/表/kv)与序列化格式;
- envelope 与 TRIGGER CONTEXT 的合并/取舍方案;
- digest 算法、预算数值、清理策略等实现参数;
- 任何 preset 级 instructions 文案修订(属于实现阶段的下游同步事项)。

## Evidence Basis

- `crates/ralph-core/src/memory.rs` / `memory_store.rs` / `config/memories.rs`(memories 模型、可见性、注入);
- `crates/ralph-core/src/task.rs` / `task_store.rs`、`crates/ralph-core/data/ralph-tools-tasks.md`(task 生命周期与授权);
- `crates/ralph-core/src/event_loop/event_processing.rs` / `prompt_injection.rs` / `dispatch_and_handoff.rs`(prompt 注入链);
- `crates/ralph-core/src/trigger_context.rs` / `handoff_envelope.rs`(结构化交接两层);
- `crates/ralph-core/src/event_logger.rs:208-226`(50KB payload 硬顶)、`parse_and_emit/legacy.rs:1442`(单业务事件预算);
- `crates/ralph-cli/src/loop_runner/dag_scheduler/job_context.rs` / `spawn.rs` / `terminal_delivery.rs` / `reconcile.rs`(DAG 执行面);
- `crates/ralph-core/src/supervisor/correction.rs` / `migrations/`(correction 与 v19-v23 表);
- `presets/en/parallel-forge.yml`(artifact-first 约定)、`docs/brainstorms/2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md`(相邻审计)。
