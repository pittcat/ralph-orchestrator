---
title: Hat 交接状态 P1 闭环(loop 工作态载体 + runtime 级 receiver contract + trigger 提炼默认化与聚合)
type: feat
date: 2026-09-14
topic: hat-handoff-state-p1-closure
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: docs/brainstorms/2026-09-11-001-feat-hat-handoff-state-requirements.md (GAP-01 / GAP-02 / GAP-04)
execution: code
---

# Hat 交接状态 P1 闭环(GAP-01 / GAP-02 / GAP-04)

## 0. 计划状态

**READY** —— 所有实施关键决策置信度 ≥ 0.85,均有直接源码证据(§2.2 / §3)。

- **代码库基线**:工作区 HEAD `60df1a22`(DAG artifact handoff plan 2026-09-13-001 已全部落地,migration v24 在位,占位 digest 已清零)。
- **调查范围**:`crates/ralph-core/src/{memory.rs, memory_store.rs, memory_parser.rs, config/memories.rs, config/loop_config.rs, handoff_envelope.rs, trigger_context.rs, loop_context.rs, worktree.rs, file_lock.rs}`、`crates/ralph-core/src/event_loop/{prompt_injection.rs, event_processing.rs, dispatch_and_handoff.rs, state_recovery.rs, tests/(common/mod.rs, memory_visibility.rs, u3_trigger_context_prompt.rs, u4_handoff_envelope_prompt.rs, u6_handoff_envelope_wiring.rs)}`、`crates/ralph-cli/src/{tools.rs, memory.rs, operation_guard.rs, loop_runner/inner.rs, loop_runner/wave/dispatcher/dispatch.rs, loop_runner/dag_scheduler/{spawn.rs, job_context.rs}}`、`crates/ralph-core/src/wave_prompt.rs`、`crates/ralph-core/tests/{scenarios.rs, scenarios/*.yml, ralph_tools_doc_drift.rs}`、`crates/ralph-core/src/{skill_registry.rs, capability_inventory.rs}`、`scripts/{run-tests.sh, check-cli-doc-drift.sh}`、`presets/{en,schemas}/`。
- **已执行的验证**:四路全文/定向调查(memory 系统、envelope 与调度链、trigger_context 与 fan-in、测试基建与文档同步义务)+ 一轮定向补查(主仓 loop 生命周期、DAG prompt emit 契约原文、required_fields 可达性、wave consumer hat 注入链可达性),全部结论带行号锚点。
- **尚未执行的验证**:未运行任何测试(计划阶段不改代码);"现有测试零回归/仅 additive 变化"的结论来自对测试断言形态的点名核对(substring 断言、golden 锁单事件格式),由各 Unit 的 Acceptance Red 与回归命令实证。
- **阻塞项**:无。

## 1. 功能目标

- **业务目标**:闭环需求文档中剩余的三条 P1 缺口——
  - **GAP-01**:建立 loop-scoped 的"工作态"载体(按 key upsert、生命周期绑定 loop、与全局 memories 分层);
  - **GAP-02**:交接契约(下游必须发什么、成功/失败信号、必填字段)成为 runtime 派生的一等能力,覆盖串行与 wave consumer hat,并补齐 DAG job prompt 的 failure 契约;
  - **GAP-04**:trigger 提炼有 runtime 级默认(schema required_fields 兜底),且 fan-in(同一 activation 内多条上游事件)有结构化聚合视图。
- **用户/调用方**:(a) loop 内 hat agent(经 `ralph tools workstate` CLI 与 prompt 注入块);(b) preset 作者(零声明获得默认提炼);(c) operator(行为与文档一致)。
- **当前行为**(全部有源码证据,见 §2.2):
  1. 工作态无载体:memories 全局共享、append-only、跨 loop 永久存活(memory_store.rs:28/162-187;loop_context.rs:447-466 symlink 共享);payload 50KB 截断(event_logger.rs:208-226);单业务事件预算(legacy.rs:1264-1274);
  2. handoff_envelope 四 flag 全默认 false(loop_config.rs:642-674),当前**零 preset 启用**(grep presets/ 零命中),唯一使用者 ce-executor-serial 已于 commit `1088dad6` 删除;DAG `build_job_prompt` 的 emit 契约块只列 success topic 的 required_fields,failure topic 只给名字不给字段(spawn.rs:2501-2503,调用点 :1998);
  3. trigger_context 完全 opt-in(TriggerContextConfig `#[serde::default]`,loop_config.rs:125-131);全仓仅 2 个 preset、4 个 topic 声明;未声明时下游只收到 `format_event` 原始 payload dump(dispatch_and_handoff.rs:190-202);多条匹配事件时提炼只取最新一条(trigger_context.rs:86-105,last-wins 由 :1046-1056 测试钉死)。
- **目标行为**:
  1. hat 可经 `ralph tools workstate set/get/list/delete` 读写**本 loop** 的 key-value 工作态(同 key 覆盖 = upsert);runtime 在 prompt 注入当前 loop 的工作态清单(有内容才注入,预算截断);worktree 复用时随 runtime 工件归档清理;
  2. 所有 isolated 链 hat(串行 + wave consumer)的 prompt 自动携带 `## RECEIVER CONTRACT` 块:本 hat 的 publishes topic 各自的成功/失败信号与 schema 必填字段(runtime 从 hat 配置 + event_policy.schemas 派生,无需 producer 自觉);DAG job prompt 补齐 failure topic 的 required_fields;
  3. topic 未声明 `trigger_context` 时,runtime 用该 topic schema 的 `required_fields` 兜底生成 `## TRIGGER CONTEXT` 摘要;同一 activation 多条匹配事件时逐条渲染摘要(聚合视图),单事件渲染格式逐字节不变。
- **行为差异**:
  - hat 第一次拥有"本 loop 内、可覆盖更新、不污染全局知识库"的中间态通道;
  - 下游 hat 的"我必须发什么"从 preset 自然语言约定变为 runtime 机器派生、恒准确的 prompt 块;
  - 无声明 preset 的下游 hat 从"读原始 JSON dump"变为"读结构化摘要";并发 fan-in 时从"只看到最新一条便条的摘要"变为"看到每条便条的摘要"。
- **本次范围**:ralph-core(workstate store / 注入链 / trigger_context 兜底与聚合 / receiver contract 派生)+ ralph-cli(`ralph tools workstate` CLI、DAG prompt 补全)+ 注入文档与 drift 清单同步。
- **非目标**(明确不做,均附证据):
  - **不翻转 handoff_envelope 默认开关**:envelope 要求 producer 在每个 payload 里手填 envelope 对象,default-on 会使所有未携带 envelope 的 emit 在 `validate_payload` 下被拒(validation.rs:1815-1843),回归面不可控;本计划用"runtime 派生 contract"(D5/D6)实现 GAP-02 的底线语义,envelope 保持 opt-in 增强(对齐需求边界"runtime 契约是底线,preset 约定是增强");
  - **wave worker prompt 不接 receiver contract**:worker 是叶子 producer,trigger event 单条 verbatim 注入(wave_prompt.rs:204-221),且 envelope 的 `to_hat` 过滤无法区分同 hat 的多 slot 实例(agent 调查 E12);wave 的交接保证在 wave 边界(`*.wave.complete` 合成事件,fan_in.rs:60-78),不在 worker prompt;
  - **不改变"一 hat 一 activation 排空整个 pending 队列"的调度模型**(event_processing.rs:1553 `take_pending`);GAP-04b 只做渲染层聚合,不做"等齐 N 个"屏障——wave/DAG 的屏障语义已存在于 wave 边界与 DAG 依赖边(WaveTracker progress wave_tracker.rs:209-219;compute_admissions dag_scheduler.rs:169);
  - **工作态不做 per-hat 私有可见性**(memories 的 Private 语义不复制);unit 维度经 key 命名约定表达,runtime 不解析 key;
  - **memory upsert / 存储层强制隔离(GAP-05/GAP-06)、task 完成摘要(GAP-07)、terminal delivery 生产化(GAP-08)**:P2/P3,不在本计划;
  - **DAG terminal delivery / integrate 路径**:不动。
- **输入**:hat 经 CLI 的 workstate 写入(key + value);event payload(现有 schema);hat 配置(triggers/publishes)与 event_policy.schemas。
- **输出**:`.ralph/agent/workstate.jsonl` 行;prompt 新块 `## WORKSTATE` / `## RECEIVER CONTRACT` / 扩展版 `## TRIGGER CONTEXT`;DAG job prompt 的 failure 字段清单;CLI stdout(list/get 的渲染)。
- **状态变化**:新增 per-worktree 文件 `workstate.jsonl`(无 DB migration,不碰 supervisor.db/dag.db);无事件形状变化;无 topic 增删。
- **错误语义**:workstore 写失败 → CLI 非零退出 + stderr(agent 可见);注入侧读失败 → 跳过注入 + warn!(fail-soft,对齐 memory 注入哲学 prompt_injection.rs:519-585);CLI 跨 loop 操作 → 拒绝(对齐 tasks 授权矩阵)。
- **兼容性要求**:全部 prompt 变更为**纯追加块**;空 store / 无 schema / 单事件时渲染结果与现状逐字节一致;`ralph emit` / preset schema / 既有 CLI 参数不变;旧 `.ralph/` 目录无 workstate.jsonl 时一切行为不变。
- **性能要求**:注入路径每次 activation 多读一个 KB 级 JSONL + 一次 fold;预算截断复用既有 4 chars/token 模式;可忽略。
- **安全要求**:工作态内容与 contract 块中的 agent 可控字符串进 prompt 前统一转义(复用 `escape_for_prompt` 模式,handoff_envelope.rs:152-167);workstate key/value 长度上限(对齐 memory add 的 10K 字符上限,memory.rs:305-315);CLI 不读取 `.ralph/` 之外的任何路径。
- **已知约束**:测试入口必须 nextest(HARD RULE 1/2);ralph-cli 集成测试必须 `common::ralph_bin()` scrub(HARD RULE 5);新增 `ralph tools` 子命令必须同步 skill_registry + check-cli-doc-drift.sh 映射(§2.2 E19/E20);新增注入文档必须遵守 AGENTS.md:212-217 四规则(可读性/去计划化/作用域边界);`crates/ralph-core/tests/scenarios.rs` 已 5454 行超 5000 行上限,本计划**不新增 BDD scenario 测试函数**,跨事件流行为用 event_loop 内联测试覆盖。
- **已确认假设**(均有证据):isolated 链是所有非 wave-worker hat 的唯一 prompt 路径(`build_prompt` 全仓唯一调用点 inner.rs:2835);wave consumer hat 与串行 hat 同链(无 wave 特有绕过分支,dispatch.rs 仅两个 worker prompt 构造点);`EventSchema.required_fields` 与 `trigger_context` 同 struct(loop_config.rs:63-138),`prepend_trigger_context` 已持有 schema 引用(prompt_injection.rs:822);主仓 per-loop 隔离靠 loop_id 作用域而非文件重置(task_store.rs:603/610/639 先例)。
- **待验证假设**:无(全部已在调查阶段闭环)。

## 2. 代码库现状与证据

### 2.1 当前实现入口

```
hat 子进程 ralph tools memory add → crates/ralph-cli/src/memory.rs(clap :79-110,dispatch tools.rs:47)
  → OperationGuard::detect_with_env(operation_guard.rs:67-82,解析 RALPH_CURRENT_HAT/LOOP_ID)
  → MarkdownMemoryStore::append(memory_store.rs:162-187,FileLock exclusive + 整文件重写)

hat activation prompt 构建(isolated 链,event_processing.rs:1551 分支):
  build_prompt(inner.rs:2835 唯一调用点)
  → prepend_hat_identity → orchestrator/wave context → recovery/correction 等(:1697-1735)
  → prepend_trigger_context(:1743 → prompt_injection.rs:789-849;无声明即 no-op,闸门 :795-829)
  → prepend_auto_inject_skills(:1744 → prompt_injection.rs:486-502 → inject_memories_and_tools_skill :519-615,
     load_visible :542 → format_memories_as_markdown → truncate_to_budget memory_store.rs:389-423)
  → scratchpad/state_files/ready_tasks(:1744-1747)
  → build_isolated_prompt_with_handoff(:1761-1773 → event_loop/mod.rs:198-255,
     gate: handoff_envelope.enabled && prompt_injection,:216)

wave worker prompt(独立路径):dispatch.rs:1442/:1911 build_wave_worker_prompt → wave_prompt.rs:167-253
DAG job prompt(独立路径):spawn.rs:2473-2572 build_job_prompt(emit 契约块 :2489-2518,failure 字段缺失 :2503)

loop 终态/复用清理:worktree.rs clean_worktree_runtime_artifacts(删除清单 :749-762 含 tasks.jsonl/scratchpad.md;
  保留清单 :764-776 含 memories.md);主仓 fresh start 清 scratchpad(inner.rs:428-453),tasks.jsonl 不重置(loop_id 作用域过期)
```

数据边界:per-worktree 隔离文件 = tasks.jsonl / scratchpad.md / events.jsonl(loop_context.rs:241-253);跨 loop 共享(symlink)= memories.md(loop_context.rs:447-466);runtime 内部 ledger(supervisor.db / dag.db / events.jsonl)hat 禁读。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | memory_store.rs:28, :162-187, :194-214 | memories 全局共享、append/delete-by-id、无 upsert、无 key/loop 字段 | GAP-01 属实;工作态必须新建载体,不可扩展 memories.md | 高 |
| E2 | task_store.rs:603/610/639 + inner.rs:428-453 + worktree.rs:749-776 | per-loop 隔离先例:tasks.jsonl per-worktree + loop_id 作用域过期(主仓不重置);worktree 复用时删除清单归档 per-loop 文件;scratchpad fresh-start 清理 | workstate 采用同模式:per-worktree JSONL + loop_id 作用域 + 复用归档清单加一行 | 高 |
| E3 | operation_guard.rs:50-55, :67-82, :151-165 | OperationContext 已解析 current_loop_id/current_hat_id/is_agent_context,memory 写入路径未使用 loop_id | workstate CLI 可直接复用同一 context,零新基建 | 高 |
| E4 | memory.rs:299-395(CLI add) | 授权先例:`--private` 需 agent context;delete 走 authorize_memory_action;human 全通;10K 字符上限 :305-315 | workstate CLI 授权/上限照抄该模式(简化:一律 agent 可写本 loop,human 全通,跨 loop 拒) | 高 |
| E5 | prompt_injection.rs:519-585 + memory_visibility.rs:33-84 | memory 注入:gating(enabled && Auto)→ load_visible → format → budget 截断(先过滤后截断,U5 教训);测试 fixture 模式 tempdir + store + EventLoop::new + 直调注入函数 | workstate 注入复刻同一结构与测试模式;空 store 必须渲染为空(零 prompt 变化) | 高 |
| E6 | handoff_envelope.rs:28-652 全文 | envelope 模型/校验/渲染/提取器全部是与执行面无关的纯函数;渲染有 escape_for_prompt + 列表截断先例 | receiver contract 块复用 escape/截断工具;envelope 本体不动 | 高 |
| E7 | loop_config.rs:629-674 + grep presets/ 零命中 + commit 1088dad6 | handoff_envelope 四 flag 默认 false,零 preset 启用,唯一使用者已删除 | default-on envelope = 回归炸弹(validate_payload 拒收);Gape-02 改走 runtime 派生 contract(D5) | 高 |
| E8 | inner.rs:2835(唯一 build_prompt 调用点)+ event_processing.rs:1551/1743/1761 + dispatch.rs:1442/:1911 | 串行与 wave consumer hat 全部走 isolated 注入链;wave worker 是唯一绕过者 | receiver contract 注入 isolated 链即覆盖串行 + wave consumer;worker 显式排除 | 高 |
| E9 | spawn.rs:2489-2518 + :1998 | DAG emit 契约块已含 success topic + required_fields 逐字段;failure topic 只给名字(call site 只传 success schema) | GAP-02 DAG 侧 = 小补全:传 failure schema 并逐字段渲染 | 高 |
| E10 | loop_config.rs:63-138 + prompt_injection.rs:796-829 | EventSchema 同 struct 内含 required_fields(:69)与 trigger_context(:131);prepend_trigger_context 已持有 schema,但 gate 只看 trigger_context 声明 | GAP-04a 兜底零结构改动:无声明时 summary_fields ← required_fields | 高 |
| E11 | trigger_context.rs:86-105 + :1046-1056 + event_processing.rs:1553/:1594-1598 | 多条匹配事件已同activation 排空(take_pending),但提炼只取最新一条(last-wins 测试钉死);其余事件以 `Event:` 原文行出现 | GAP-04b = 渲染层聚合:逐条渲染摘要;单事件格式不变以保 golden(:828-867) | 高 |
| E12 | wave_prompt.rs:204-221 + dispatch.rs:1413/1889 + fan_in.rs:60-78 + wave_tracker.rs:209-219 + dag_scheduler.rs:169 | wave worker 单事件 verbatim;fan-in 屏障已存在于 wave 边界(wave.complete 合成单事件)与 DAG 依赖边 | 不做通用"等齐 N"屏障(非目标);wave/DAG 屏障语义已满足 | 高 |
| E13 | event_logger.rs:208-226 + legacy.rs:1264-1274 | 50KB 是截断非拒收;单业务事件预算在 isolated 生效 | workstate 的价值定位证据(中间态不必挤 payload);本计划不动这两个机制 | 高 |
| E14 | event_loop/tests/common/mod.rs:15-311 | fixture 基建:minimal_isolated_config / write_event_to_jsonl / install_admitting_flow / init_git_workspace;新测试文件须注册 tests/mod.rs | 全部新 event_loop 测试复用该基建;记得注册 mod | 高 |
| E15 | memory_visibility.rs 全 212 行 | 注入可见性测试模式:tempdir + MarkdownMemoryStore::with_default_path + 直调 inject 函数 + 断言 prefix 内容 | workstate 注入测试、contract 注入测试同构 | 高 |
| E16 | tools.rs:28-51 + memory.rs:79-110 + task_cli/args.rs:148 | `ralph tools` 子命令注册模式:tools.rs enum variant + dispatch;各自 Args/Commands 文件 | workstate CLI 新增 workstate_cli 模块 + 两处注册 | 高 |
| E17 | skill_registry.rs:18-52, :103-125, :415-434 | 注入文档注册:`include_str!` 常量 + register_builtins + 注册断言测试 | 新增 ralph-tools-workstate.md 必须三处同步 | 高 |
| E18 | check-cli-doc-drift.sh:142-178 | COMMANDS_TO_DOCS 映射表,新增子命令须加映射,否则 drift 扫描漏检 | U1/U2 收尾同步 | 高 |
| E19 | ralph_tools_doc_drift.rs:40-109 | drift 测试锁:12 个 RALPH_DAG_* key、表名、plan-anchor 禁出现 | 本计划不增删 env key,drift 测试应零改动保持绿 | 高 |
| E20 | config/memories.rs:45-95 + loop_config.rs dag_pools 先例(commit 5e770421) | 新配置字段惯例:serde default + Default impl + roundtrip/deny_unknown 测试;默认 on 且空态零副作用时无需进 PRESET_OPT_IN 列表 | workstate/receiver_contract/trigger_fallback 三个配置的落地模板 | 高 |
| E21 | u3_trigger_context_prompt.rs(7 场景)+ trigger_context.rs:828-867 golden | "空声明→不注入"场景断言无 TRIGGER CONTEXT;golden 锁单事件渲染精确字符串 | U5 兜底会改变"空声明"场景的预期(有意行为变更,须更新该测试并记录);U6 不得动单事件 golden | 高 |
| E22 | scenarios.rs:5454 行 + AGENTS.md 5000 行 HARD RULE | BDD 入口文件已超 5000 行 | 本计划不新增 scenario 函数;跨事件行为用 event_loop 内联测试(E14 基建足够) | 高 |
| E23 | worktree.rs:749-762 删除清单 / :764-776 保留清单 | tasks.jsonl/scratchpad.md 在删除清单;memories.md 在保留清单 | workstate.jsonl 加进删除清单(一行),测试仿既有归档测试 | 高 |
| E24 | event_processing.rs:1756-1760 注释 | envelope 注入被显式设计为 default-closed 以保护非串行 preset | 佐证 D5:runtime 派生 contract 也必须默认渲染为空态安全(无 publishes/无 schema → 不渲染) | 高 |

### 2.3 受影响范围

- **生产模块(新增)**:`crates/ralph-core/src/workstate.rs`(模型+store);`crates/ralph-core/src/config/workstate.rs`(配置);`crates/ralph-cli/src/workstate_cli.rs`(CLI);`crates/ralph-core/data/ralph-tools-workstate.md`(注入文档)。
- **生产模块(修改)**:`crates/ralph-core/src/lib.rs`(模块导出);`crates/ralph-core/src/config/mod.rs`(配置导出);`crates/ralph-core/src/event_loop/prompt_injection.rs`(WORKSTATE 块 + RECEIVER CONTRACT 块 + trigger 兜底/聚合);`crates/ralph-core/src/config/loop_config.rs`(EventLoopConfig 两个新字段);`crates/ralph-core/src/worktree.rs`(归档删除清单 +1 行);`crates/ralph-cli/src/tools.rs`(注册);`crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs`(failure schema 传入与渲染);`crates/ralph-core/src/skill_registry.rs`(注册新文档);`crates/ralph-core/src/loop_context.rs`(布局注释 +1 行)。
- **测试模块**:上述各文件内联 tests;`event_loop/tests/` 新增 2 个文件(注册 tests/mod.rs);`crates/ralph-cli/tests/integration_workstate.rs`(新增,用 common::ralph_bin)。
- **配置**:新增 `workstate.*`、`event_loop.receiver_contract.enabled`、`event_loop.trigger_context_fallback`,全部默认 on 且空态零副作用,不进 PRESET_OPT_IN 列表(E20)。
- **文档**:`crates/ralph-core/data/ralph-tools.md`(命令速查 +1 行)、`ralph-tools-memories.md`(分层说明 +1 节)、新增 `ralph-tools-workstate.md`;`scripts/check-cli-doc-drift.sh` 映射 +3 条;AGENTS.md/CLAUDE.md 反向核对(预期仅 loop 布局描述若有列举需补)。
- **不变**:preset yml / schema、manifest/index.json、zsh 补全(无 builtin preset 变更)、`ralph emit`、env key 集合、DB migration。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | 工作态存哪里 | (a) 新 JSONL `.ralph/agent/workstate.jsonl`;(b) supervisor.db 新表;(c) 扩展 memories.md | **(a)** | E2(tasks.jsonl per-worktree + loop_id 作用域先例)、E1(memories 语义不可混) | (b) supervisor.db 是 runtime 内部 ledger,agent 写入须经 CLI,引入 migration 与 feature gate 复杂度;(c) 违反工作态≠知识库分层 | 0.90 |
| D2 | upsert 语义 | (a) append-only + 读时 fold(last-wins per (loop_id,key));(b) 写时整文件重写 | **(a)** | E2(tasks.jsonl 模式);JSONL append 天然并发友好(对比 memory 整文件锁重写 memory_store.rs:162-187) | (b) 复制 GAP-06 已批评的写放大模式 | 0.85 |
| D3 | workstate CLI 形状与授权 | `ralph tools workstate set/get/list/delete`;agent context 限本 loop,human 全通,跨 loop 拒;10K 上限 | **同左** | E3(context 现成)、E4(授权/上限先例)、E16(注册模式) | 不做 update 子命令(set 即 upsert);不做 clear(删除清单归档已覆盖生命周期) | 0.85 |
| D4 | 工作态注入与生命周期 | 注入 `## WORKSTATE` 块(仅当前 loop、非空才渲染、预算截断、转义);配置 `workstate.{enabled,inject,budget}` 默认 on;worktree 归档清单 +1 行 | **同左** | E5(注入/截断/测试模式)、E6(转义先例)、E23(归档清单位置)、E20(配置模板);默认 on 回归安全因空 store 零渲染 | 不做 per-hat 可见性过滤(GAP-01 未要求,memories 的 Private 语义不复制) | 0.85 |
| D5 | GAP-02 实现形态 | (a) envelope default-on;(b) runtime 从 hat publishes + schemas 派生 `## RECEIVER CONTRACT` 块注入 isolated 链 | **(b)** | E7(envelope default-on = emit 拒收回归)、E8( isolated 链覆盖串行+wave consumer)、E24(default-closed 设计哲学 → 空态不渲染) | (a) 回归面不可控,且需求边界允许"runtime 契约是底线"的别样实现 | 0.85 |
| D6 | DAG 侧 contract | build_job_prompt 调用点补传 failure schema,契约块逐字段渲染 failure required_fields | **同左** | E9(缺口精确到 :1998/:2503) | 无需新块,补齐既有块即可 | 0.90 |
| D7 | GAP-04a 兜底来源 | 无 trigger_context 声明时 summary_fields ← schema.required_fields;`event_loop.trigger_context_fallback` 默认 true | **同左** | E10(required_fields 同 struct、函数已持有);required_fields 是 schema 声明字段,不破坏 leakage guard | 用 known_fields 兜底会扩大注入面(known_fields 是 hint 条件用 pass-through,语义不符) | 0.85 |
| D8 | GAP-04b 聚合形态 | 渲染层:多条匹配事件逐条渲染摘要块;单事件格式逐字节不变;不改调度 | **同左** | E11(队列已 drain,缺的是逐条摘要)、E21(golden 锁单事件格式)、需求边界(不改一次 trigger 一次 activation) | 做"等齐 N"屏障 = 改调度模型,且 wave/DAG 已有屏障(E12),重复造 | 0.85 |
| D9 | source_hat 是否接入聚合视图 | 不接(保持现状 None → "(unknown source hat)") | **同左** | prompt_injection.rs:836-842 现状恒 None,解析 event.hat 是额外行为变更 | 控制范围;topic 名已足以区分 producer | 0.85 |

全部 ≥ 0.85,无 BLOCKED 决策。

## 4. BDD 行为规格

Feature: loop 工作态载体(U1/U2)

  Background:
    Given 一个 isolated 模式的 loop,loop_id 为 L1

  Scenario: hat 写入并读回工作态
    When hat 执行 `ralph tools workstate set draft-conclusion "方案A待验证"`
    Then `ralph tools workstate get draft-conclusion` 输出 "方案A待验证"
    And 该条目属于 loop L1

  Scenario: 同 key 覆盖(upsert)
    Given key `draft-conclusion` 已有值 "方案A待验证"
    When hat 执行 `ralph tools workstate set draft-conclusion "方案A已确认"`
    Then get 返回 "方案A已确认"
    And store 中该 (loop,key) 的有效值只有一条(读时 fold last-wins)

  Scenario: 跨 loop 不可见
    Given loop L1 有 key `draft-conclusion`
    When loop L2(fresh start)的 hat 执行 `ralph tools workstate list`
    Then 输出不含 `draft-conclusion`

  Scenario: 工作态进 prompt
    Given 本 loop 有 2 条工作态
    When 某 hat 构建 activation prompt
    Then prompt 含 `## WORKSTATE` 块,列出两个 key 与内容(转义后)
    And 无工作态时 prompt 不出现该块(与现状逐字节一致)

  Scenario: 预算截断
    Given 工作态总渲染超过配置 budget
    Then 块在条目边界截断并附截断标记,不切断 UTF-8/条目中间

  Scenario: 非法输入
    When key 为空或 value 超过 10K 字符
    Then CLI 非零退出,stderr 指明原因,不写任何行

  Scenario: worktree 复用归档
    Given worktree 的 `.ralph/agent/workstate.jsonl` 有内容
    When 该 worktree 被复用(reuse)
    Then 旧 workstate.jsonl 被移入 `.ralph/reuse-history/<ts>/`,新 loop 从空开始

Feature: runtime 派生 receiver contract(U3/U4)

  Scenario: isolated hat 看到自己 publishes 的契约
    Given hat `executor` publishes `work.done`,schema 声明 required_fields=[summary, commit]
    When executor 构建 prompt
    Then prompt 含 `## RECEIVER CONTRACT` 块,列出 `work.done` 及其 required_fields
    And hat 无 publishes 或无 event_policy 时不渲染该块(零变化)

  Scenario: wave 模式 consumer hat 同样获得契约
    Given supervisor/wave preset 的 consumer hat(非 worker)
    When 其经 build_prompt 构建 prompt
    Then 同样含 `## RECEIVER CONTRACT` 块

  Scenario: 配置关闭
    Given `event_loop.receiver_contract.enabled: false`
    Then 任何 hat prompt 均无该块

  Scenario: DAG job prompt 列出 failure 必填字段
    Given DAG 模式某 stage 的 failure topic schema 有 required_fields
    When runtime 构建该 stage 的 job prompt
    Then emit 契约块逐字段列出 failure topic 的 required_fields(现状只给 topic 名)

Feature: trigger 提炼默认化与聚合(U5/U6)

  Scenario: 未声明 trigger_context 的 topic 获得兜底摘要
    Given topic `review.done` 未声明 trigger_context,schema required_fields=[verdict, report_path]
    And payload 含 verdict/report_path 及未声明字段 secret_note
    When 下游 hat 构建 prompt
    Then `## TRIGGER CONTEXT` 含 verdict 与 report_path 的值
    And 不含 secret_note(leakage guard:只渲染 schema 声明字段)

  Scenario: 显式声明优先于兜底
    Given topic 声明了 trigger_context.summary_fields=[a]
    Then 摘要只含 a(兜底不叠加)

  Scenario: 多条匹配事件逐条聚合
    Given 同一 activation 排空出 3 条匹配 hat triggers 的事件(不同 topic 或同 topic 多 producer)
    Then `## TRIGGER CONTEXT` 含 3 个摘要段,按到达顺序排列,每段标注 source topic

  Scenario: 单事件渲染不变
    Given 只有 1 条匹配事件
    Then 渲染输出与现状 golden 逐字节一致

  Scenario: 兜底总开关关闭
    Given `event_loop.trigger_context_fallback: false` 且 topic 无声明
    Then 不渲染 `## TRIGGER CONTEXT`(现状行为)

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | 需要 E2E |
|---|---|---|---|---|---|
| set/get/list/delete | CLI stdout 正确;文件行符合 JSONL 折叠语义 | `crates/ralph-cli/tests/integration_workstate.rs`(common::ralph_bin)+ store 单测 | 集成(进程级)+ 单元 | 并发:两进程同时 set 不同 key 不丢行(append-only + FileLock,对齐 tasks 先例) | 否 |
| upsert | 同 key 两次 set,get 返回新值;fold 后单值 | workstate.rs 单测 | 单元 | 无 | 否 |
| 跨 loop 不可见 | L1 写入,L2 list 为空 | store 单测(两 loop_id)+ CLI 集成(env 注入 RALPH_CURRENT_LOOP_ID) | 单元+集成 | 无 | 否 |
| prompt 注入/空态零渲染 | 有条目时块出现且转义正确;空时 prefix 与基线 diff 为空 | `event_loop/tests/workstate_prompt.rs`(新增,仿 memory_visibility.rs:33-84) | 集成(in-process EventLoop) | 预算截断在条目边界(仿 truncate_to_budget 语义) | 否 |
| 非法输入 | 空 key / 超限 value → exit≠0,零行写入 | CLI 集成 | 集成 | 无 | 否 |
| worktree 归档 | 复用后旧文件进 reuse-history,新 loop 空 | worktree.rs 内联测试(仿既有归档测试) | 单元 | 无 | 否 |
| receiver contract 注入 | publishes+schema → 块含 topic 与字段;无 publishes → 无块 | `event_loop/tests/receiver_contract_prompt.rs`(新增) | 集成(in-process) | 字符串转义(含 backtick/换行的字段名不可能,字段名是 schema 标识符;内容侧无 agent 文本,块内容全部来自 schema——转义需求仅防 topic 名异常,单测覆盖) | 否 |
| wave consumer 同链 | 构造 wave 模式 config,consumer hat build_prompt 含块 | 同上测试文件 | 集成 | 无 | 否 |
| 配置关闭 | enabled=false → 无块 | 同上 | 单元 | 无 | 否 |
| DAG failure 字段 | job prompt 含 failure topic 每个 required_field | spawn.rs 内联测试(prompt 纯函数断言) | 单元 | 无 | 否 |
| 兜底摘要 | 无声明 topic 注入 required_fields 值;未声明字段不出现 | 扩展 `event_loop/tests/u3_trigger_context_prompt.rs` 或新文件 | 集成(in-process) | leakage guard 负断言(secret 字段名不出现在 prompt) | 否 |
| 显式优先 | 声明 topic 不受兜底影响 | trigger_context.rs 单测 | 单元 | 无 | 否 |
| 多事件聚合 | 3 事件 → 3 段、顺序、各带 topic | 注入层测试(多事件 write_event_to_jsonl) | 集成(in-process) | 无 | 否 |
| 单事件 golden 不变 | 现有 golden 测试零改动保持绿 | trigger_context.rs:828-867 既有测试 | 单元 | Characterization(先跑基线) | 否 |
| 兜底关闭 | fallback=false → 无块 | 注入层测试 | 单元 | 无 | 否 |

层级选择理由:失败路径全部在 CLI/纯函数层(无子进程调度),in-process EventLoop 即可真实验证 prompt 内容(注入是本行为本体);不需要起真实 backend 或大 E2E。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成测试 | Evidence |
|---|---|---|---|---|---|---|
| R1 | workstate CLI upsert 读写(GAP-01) | U1 场景 1/2/6 | `workstate_cli_set_get_roundtrip` 等(integration_workstate.rs) | store fold/upsert/校验单测 | 进程级 CLI | E1/E2/E3/E4/E16 |
| R2 | loop 作用域与生命周期 | U1 场景 3 + U2 归档场景 | `workstate_cross_loop_invisible`、`worktree_reuse_archives_workstate` | loop 过滤单测 | CLI env 注入测试 | E2/E23 |
| R3 | 工作态 prompt 注入(GAP-01) | U2 场景 4/5 | `workstate_prompt_injects_current_loop_entries`、`workstate_prompt_empty_store_no_block` | 渲染/截断/转义单测 | in-process EventLoop | E5/E6 |
| R4 | runtime 派生 receiver contract(GAP-02) | U3 场景 1/2/3 | `receiver_contract_block_lists_publishes_requirements`、`receiver_contract_wave_consumer_reachable`、`receiver_contract_disabled_no_block` | 派生逻辑单测 | in-process EventLoop | E7/E8/E24 |
| R5 | DAG failure 契约补全(GAP-02) | U4 场景 | `dag_job_prompt_lists_failure_required_fields` | prompt 纯函数断言 | — | E9 |
| R6 | trigger 兜底摘要(GAP-04a) | U5 场景 1/2/5 | `trigger_context_fallback_uses_required_fields`、`trigger_context_explicit_declaration_wins`、`trigger_context_fallback_disabled` | builder 单测 | in-process EventLoop | E10/E21 |
| R7 | fan-in 聚合视图(GAP-04b) | U6 场景 3/4 | `trigger_context_aggregates_multiple_pending_events` + 既有 golden 保持绿 | 多事件 builder 单测 | in-process EventLoop | E11/E21 |
| R8 | 文档与 drift 同步 | 全部 | ralph_tools_doc_drift / capability_inventory / check-cli-doc-drift.sh / skill_registry 注册断言 | — | — | E17/E18/E19 |

## 7. 严格串行开发单元

```
Unit 1(workstate store + CLI)
  ↓ 完成全部测试、重构和回归
Unit 2(workstate prompt 注入 + 配置 + 生命周期归档)
  ↓ 完成全部测试、重构和回归
Unit 3(receiver contract 派生与 isolated 链注入)
  ↓ 完成全部测试、重构和回归
Unit 4(DAG job prompt failure 契约补全)
  ↓ 完成全部测试、重构和回归
Unit 5(trigger_context 兜底摘要)
  ↓ 完成全部测试、重构和回归
Unit 6(多事件聚合视图)
```

---

### Unit 1:workstate 存储与 `ralph tools workstate` CLI

**1. Unit 目标**:agent/human 可经 `ralph tools workstate set/get/list/delete` 读写当前 loop 的 key-value 工作态;同 (loop_id, key) 后写覆盖先写(upsert);跨 loop 不可见;非法输入拒绝。

**2. 对应需求与 Scenario**:R1/R2(前半);U1 Feature 场景 1/2/3/6;D1/D2/D3;E1/E2/E3/E4/E16。

**3. 外部可观察结果**:`ralph tools workstate set k v`  exit 0;`get k` 输出值;`list` 列出本 loop 全部 key;`delete k` 后 get 失败;`--root` 全局参数与 memory 一致;`.ralph/agent/workstate.jsonl` 出现 append 行。

**4. 当前行为基线**:该命令不存在(`tools.rs:28-37` enum 仅 Memory/Task/Skill)。无旧行为需 pin;需 pin 的既有行为:`ralph tools --help` 输出仅追加新子命令(drift 脚本基线同步,见 §17)。

**5. 输入与输出**:
- 输入:`set <key> <value>` / `get <key>` / `list` / `delete <key>`;agent context 经 `OperationGuard::detect_with_env`(E3)取 loop_id;human 无 loop_id 时操作"无 loop 作用域"(loop_id=None,与 loop 内数据互不可见)。
- 输出:get → 值原文 stdout;list → 每行 `key<TAB>updated_at`;set/delete → 静默 exit 0(对齐 memory add 风格)。
- 错误:空 key、value > 10K 字符(E4 上限先例)、key 含控制字符/空白 → exit≠0 + stderr;store IO 失败 → exit≠0。
- 状态变化:append 一行 JSONL `{loop_id, key, value, updated_at_ms, hat?}`;delete = append 墓碑行 `{..., "deleted": true}`(fold 时剔除)。
- 副作用:仅该文件;FileLock exclusive 写(E: file_lock.rs:74/89 现成)。
- 不变量:只追加不 rewrite;读 = fold 后 last-wins;跨 loop 行永不返回。

**6. 修改位置**:
- `crates/ralph-core/src/workstate.rs`(**新增**,≤400 行):`WorkstateEntry` struct;`WorkstateStore { path }`,`with_default_path(root)` → `<root>/.ralph/agent/workstate.jsonl`;`set(loop_id, key, value, hat)`(形状校验 → FileLock exclusive → append);`get/list/delete(loop_id, ...)`(shared lock → fold → 过滤 loop_id);fold 纯函数 `fold_entries(lines) -> BTreeMap<(Option<loop_id>, key), Entry>`。
- `crates/ralph-core/src/lib.rs`:模块导出(对照 memory 导出方式)。
- `crates/ralph-cli/src/workstate_cli.rs`(**新增**):`WorkstateArgs`/`WorkstateCommands`(仿 memory.rs:79-110),execute 用 `OperationGuard` 取 context,agent context 强制带 loop_id(无 loop_id 的 agent context = 异常,报错);human 用 None。
- `crates/ralph-cli/src/tools.rs:28-51`:enum variant + import + dispatch(E16)。
- 明确不修改:memory.rs、task_cli/、main.rs(Tools 已挂载)。

**7. 可依赖能力**:FileLock(E:file_lock.rs);OperationGuard(E3);memory.rs CLI 全套模式(E4);tools.rs 注册模式(E16)。

**8. 禁止依赖的未来能力**:不得做任何 prompt 注入(U2);不得写 `workstate.jsonl` 进 worktree 归档清单(U2);不得新增注入文档(U2 统一做,避免半吊子文档);不得新增配置字段(U2)。

**9. 验收测试**:
- `workstate_cli_set_get_roundtrip`(integration_workstate.rs,新增,`common::ralph_bin()` + tempdir `--root`):set → get 断言 stdout;list 断言含 key;delete → get exit≠0。
- `workstate_cli_upsert_last_write_wins`:两次 set 同 key,get 返回第二次值。
- `workstate_cli_cross_loop_invisible`:env 注入 `RALPH_CURRENT_HAT=x RALPH_CURRENT_LOOP_ID=L1` set;再以 L2 list → 空(先 scrub 再显式 env,HARD RULE 5)。
- `workstate_cli_rejects_invalid_input`:空 key / 11K value → exit≠0;文件无新行。
- store 单测(workstate.rs 内联):fold last-wins;墓碑剔除;loop 过滤;human(None)与 loop 隔离;并发:两个 store 实例交替 append 不丢行(FileLock)。
- 运行命令:`cargo nextest run -p ralph-core -- workstate`;`cargo nextest run -p ralph-cli --test integration_workstate`。

**10. Acceptance Red**:先写 `workstate_cli_set_get_roundtrip`:Red 形态 = CLI 报 `unrecognized subcommand`(真实行为失败,证明命令不存在)。store 单测 Red = 编译失败(新 API 不存在,如实记录)→ 补最小 trait/struct 后断言失败(无写入)→ 接线后转绿。无效 Red 排除:tempdir 权限、二进制未构建(先 `cargo build -p ralph-cli`)。

**11. 单元测试拆分**:
- key/value 形状校验纯函数:合法 Ok;空 key、含 `\n`/`\t`、超长 Err。
- fold:乱序行 last-wins;墓碑;坏行跳过 + warn(不 panic,对齐 memory parser 容错)。
- loop 过滤:L1/L2/None 三向。
- 不允许 mock 的真实行为:store 必须真写临时文件;FileLock 真实加锁。

**12. Red → Green → Refactor 顺序**:
1. 形状校验 Red → 实现 → Green;
2. fold 纯函数 Red → Green;
3. store set/get/list/delete Red → Green;
4. CLI Red(unrecognized subcommand)→ clap + dispatch + execute → Green;
5. 跨 loop / 非法输入逐个 Red → Green;
6. Refactor:确认与 memory.rs 重复的形状校验逻辑是否可共用(仅当自然,不强行抽象)。

**13. 最小实现范围**:store + CLI 四命令 + 校验。必须处理:IO 失败、坏行、并发 append、超长输入。必须保持:memory/task CLI 零变化。**不实现**:注入、配置、归档、文档。

**14. 集成验证**:integration_workstate.rs 全文件即进程级集成;另跑 `cargo nextest run -p ralph-cli --test integration_memory`(相邻 CLI 回归)。

**15. 风险驱动测试**:并发 append(FileLock 边界,已列);Fuzz 不做(key/value 经形状校验 + JSON 转义,攻击面小);无外部服务。

**16. 回归范围**:
- `cargo nextest run -p ralph-core`(lib 新增模块,全 core 回归);
- `cargo nextest run -p ralph-cli --bin ralph -- memory`(相邻 CLI 单测);
- `cargo nextest run -p ralph-cli --test integration_memory`;
- `cargo clippy --all-targets` + `cargo fmt --check`;
- 理由:纯新增,理论零回归面,但 lib.rs/tools.rs 是共享注册点。

**17. 预期文件变更**:

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| crates/ralph-core/src/workstate.rs | 新增 | store + 模型 | D1/D2 |
| crates/ralph-core/src/lib.rs | 修改 | 导出 | — |
| crates/ralph-cli/src/workstate_cli.rs | 新增 | CLI | D3/E16 |
| crates/ralph-cli/src/tools.rs | 修改 | 注册 | E16 |
| crates/ralph-cli/tests/integration_workstate.rs | 新增 | 进程级验收 | HARD RULE 5 |

**18. 完成标准**:全部验收/单测绿;回归绿;clippy/fmt 绿;无跳过/削弱;未实现 U2+ 内容;可独立提交。

**19. 停止条件**:FileLock 对"不存在文件首次 append"的语义与假设不符 → 停下查证;OperationGuard 在 `ralph tools` 下的 loop_id 解析与 E3 不符 → 停下;tools.rs 注册方式变更 → 停下。

**20. 风险与注意事项**:
- R1:human 无 loop context 的语义(None 作用域)需写进 CLI help 与后续文档,避免 operator 困惑;
- R2:JSONL 无限增长——loop 量大后 fold 成本上升;缓解:单行 KB 级、loop 归档(U2)清理 worktree 侧;主仓侧量小(单 loop 串行),记录为已知残留,不做压缩。

---

### Unit 2:workstate prompt 注入 + 配置 + worktree 归档

**1. Unit 目标**:hat activation prompt 在非空时携带 `## WORKSTATE` 块(当前 loop 条目、转义、预算截断);`workstate.{enabled,inject,budget}` 配置生效;worktree 复用时旧 workstate.jsonl 归档清理。

**2. 对应需求与 Scenario**:R2(归档)/R3;U1 Feature 场景 4/5/7;D4;E5/E6/E20/E23。

**3. 外部可观察结果**:有条目时 prompt 出现 `## WORKSTATE` 块;空 store / `enabled:false` / `inject:manual` 时 prompt 与现状逐字节一致;worktree 复用后 `.ralph/reuse-history/<ts>/` 含旧 workstate.jsonl。

**4. 当前行为基线**:prompt 无该块(E5 注入链无此环节);worktree 归档删除清单不含 workstate.jsonl(E23)。Characterization 先行:`workstate_prompt_empty_store_no_block` pin 住"空 store 时注入函数对 prefix 零修改"。

**5. 输入与输出**:
- 输入:当前 hat 的 loop_id(注入路径:EventLoop 持有的 loop context;确认点 = EventLoop 结构上的 loop id 字段,进入 Unit 时先 grep `self.*loop_id` 于 event_loop/,预期存在,state_recovery.rs 多处使用)+ workstate.jsonl。
- 输出:prefix 追加 `## WORKSTATE\n- key: value(转义)\n...`。
- 错误:文件不存在/读失败/坏行 → 跳过注入(fail-soft,E5 哲学)。
- 不变量:只读当前 loop 行;先 fold 后截断(对齐 U5 教训"先过滤后截断");空态零渲染。

**6. 修改位置**:
- `crates/ralph-core/src/config/workstate.rs`(**新增**):`WorkstateConfig { enabled, inject: InjectMode, budget }`(复用 memories.rs:8-18 的 InjectMode 或抽出共享——若抽出会动 memories.rs,**选择原地复用 `config::memories::InjectMode` 重导出**,不抽象新类型);默认 enabled=true/inject=Auto/budget=0(无限,与 memories 默认一致)。
- `crates/ralph-core/src/config/mod.rs:237-239` 旁:RalphConfig 加 `workstate` 字段(serde default)。
- `crates/ralph-core/src/event_loop/prompt_injection.rs`:`inject_memories_and_tools_skill`(:519-615)之后新增 `inject_workstate(&self, prefix, loop_id)`;接线进 `prepend_auto_inject_skills`(:486-502)尾部。渲染函数独立纯函数 `render_workstate_block(entries, budget) -> Option<String>`(None=空态)。
- `crates/ralph-core/src/worktree.rs:749-762`:删除清单加 `.ralph/agent/workstate.jsonl`。
- `crates/ralph-core/src/loop_context.rs:14-34`:布局注释 +1 行。
- 明确不修改:memory 注入逻辑本体;scratchpad 注入。

**7. 可依赖能力**:U1 的 store;E5 注入/截断/测试模式;`truncate_to_budget` 语义参考(memory_store.rs:389-423,不直接复用——它绑 memory block 标记,workstate 渲染自有条目边界截断);E20 配置模板。

**8. 禁止依赖的未来能力**:不做 receiver contract(U3);不动 trigger_context(U5/U6);不写 ralph-tools-workstate.md 之外的文档改动(文档在本 Unit 一并完成,见 §17,避免文档滞后硬规则)。

**9. 验收测试**:
- `workstate_prompt_injects_current_loop_entries`(event_loop/tests/workstate_prompt.rs,新增并注册 tests/mod.rs;仿 memory_visibility.rs:33-84):tempdir + store 写两条 + EventLoop::new + 直调注入 → prefix 含两 key,值转义(写一条含 backtick/换行的值断言转义)。
- `workstate_prompt_empty_store_no_block`(characterization):空 store → prefix 逐字节不变。
- `workstate_prompt_disabled_or_manual_no_block`:enabled=false / inject=manual → 不变。
- `workstate_prompt_budget_truncates_at_entry_boundary`:budget 小 → 截断在条目边界 + 截断标记。
- `workstate_prompt_only_current_loop`:L1 条目 + L2 条目 → 只见 L1。
- `worktree_reuse_archives_workstate`(worktree.rs 内联,仿既有归档测试):复用后旧文件在 reuse-history,新路径不存在。
- 配置 roundtrip/deny_unknown 测试(config/workstate.rs 内联,仿 loop_config.rs:2171-2211)。
- 文档门禁:`cargo nextest run -p ralph-core -- skill_registry`(注册断言)+ `bash scripts/check-cli-doc-drift.sh`。
- 运行命令:`cargo nextest run -p ralph-core -- workstate`;`cargo nextest run -p ralph-core -- event_loop`。

**10. Acceptance Red**:`workstate_prompt_injects_current_loop_entries` 先跑:Red = prefix 不含 `## WORKSTATE`(注入函数不存在→编译 Red 如实记录,补空实现后断言失败→真实行为 Red)。归档测试 Red:复用后 workstate.jsonl 未被归档(真实行为失败)。

**11. 单元测试拆分**:
- `render_workstate_block`:空→None;单条;多条按 key 排序;转义(backtick/换行/控制符);预算边界(恰好/超一条/零预算无限)。
- 配置:默认值、roundtrip、未知字段拒。
- 不允许 mock:注入测试真读临时文件 store;EventLoop 真实构造。

**12. Red → Green → Refactor 顺序**:
1. characterization(空态零渲染)先绿;
2. render 纯函数 Red → Green;
3. 注入接线 Red → Green;
4. 配置 gating Red → Green;
5. 归档清单 Red → Green;
6. 文档(ralph-tools-workstate.md 新增 + 注册 + drift 映射 + ralph-tools.md 速查行 + memories.md 分层节)→ 门禁 Green;
7. Refactor:检查 prompt_injection.rs 行数(现 1163,预计 +200 内,远低于 5000)。

**13. 最小实现范围**:配置 + 注入 + 归档 + 文档。必须保持:空态/关态逐字节等价;memory 注入不变;既有归档清单其他项不变。**不实现**:receiver contract、trigger 改动。

**14. 集成验证**:workstate_prompt.rs 全文件;`cargo nextest run -p ralph-core -- prompt_injection`;`cargo nextest run -p ralph-core --test scenarios`(注入链在 BDD 全链路中,确认 prompt_contains 断言不破——新增块为 prefix 追加,substring 断言方向安全)。

**15. 风险驱动测试**:转义(不可信输入进 prompt,已列);截断边界(已列);并发:注入是只读 fold,FileLock shared,无风险,不加并发测试。

**16. 回归范围**:
- `cargo nextest run -p ralph-core`(注入链全局);
- `cargo nextest run -p ralph-core --test scenarios`(prompt_contains 面);
- `cargo nextest run -p ralph-cli --bin ralph -- worktree`(归档相邻);
- `cargo nextest run -p ralph-core -- skill_registry` + `cargo nextest run -p ralph-core -- capability_inventory` + `bash scripts/check-cli-doc-drift.sh`;
- clippy/fmt。

**17. 预期文件变更**:

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| crates/ralph-core/src/config/workstate.rs | 新增 | 配置 | D4/E20 |
| crates/ralph-core/src/config/mod.rs | 修改 | RalphConfig 字段 | E20 |
| crates/ralph-core/src/event_loop/prompt_injection.rs | 修改 | 注入接线 + render | E5 |
| crates/ralph-core/src/event_loop/tests/workstate_prompt.rs | 新增 | 验收 | E15 |
| crates/ralph-core/src/event_loop/tests/mod.rs | 修改 | 注册 mod | E14 |
| crates/ralph-core/src/worktree.rs | 修改 | 归档清单 +1 行 | E23 |
| crates/ralph-core/src/loop_context.rs | 修改 | 布局注释 | — |
| crates/ralph-core/data/ralph-tools-workstate.md | 新增 | agent 注入文档(HARD RULE) | E17 |
| crates/ralph-core/src/skill_registry.rs | 修改 | include_str + register + 断言 | E17 |
| crates/ralph-core/data/ralph-tools.md | 修改 | 命令速查 +1 行 | AGENTS.md:216 |
| crates/ralph-core/data/ralph-tools-memories.md | 修改 | "工作态≠知识库"分层一节 | AGENTS.md:216 |
| scripts/check-cli-doc-drift.sh | 修改 | COMMANDS_TO_DOCS +3 | E18 |
| scripts/cli-doc-drift.baseline | 修改(如有新增 flag 基线) | drift 基线 | E18 |

**18. 完成标准**:同 U1 标准 + 文档门禁三绿 + 注入文档遵守 AGENTS.md:212-217(人工逐条核对:无内部函数名/行号/ledger 路径、无 plan 编号、agent 可执行动作导向)。

**19. 停止条件**:EventLoop 上拿不到 loop_id(与假设冲突)→ 停下查证注入点应取何身份;空态零渲染无法满足(现状 prefix 有隐藏变动)→ 停下;`minimal_isolated_config`(common/mod.rs:15-33)缺 workstate 配置段导致全 fixture 连锁修改 → 停下评估(预期 serde default 零修改)。

**20. 风险与注意事项**:
- R1(行为收紧面):默认 on 意味着所有 preset 的 hat 一旦写了 workstate 就进 prompt——空态零变化保证"不写则无感";检测 = scenarios 全绿;
- R2(文档):ralph-tools-workstate.md 是新注入文档,须按"agent 下一步能执行什么"写,禁止泄露 JSONL 路径等内部细节(可见性:文档只讲 CLI)。

---

### Unit 3:runtime 派生 receiver contract(isolated 链注入)

**1. Unit 目标**:所有走 isolated 链的 hat(串行 + wave consumer)prompt 自动携带 `## RECEIVER CONTRACT` 块:本 hat publishes 的每个 topic + 其 schema required_fields(+ publishes 中无 schema 的 topic 只列名);无 publishes / 无 event_policy / 配置关闭时不渲染。

**2. 对应需求与 Scenario**:R4;U3 Feature(本计划 §4 Feature 2)场景 1/2/3;D5;E7/E8/E24。

**3. 外部可观察结果**:有 publishes 的 hat prompt 出现新块;`event_loop.receiver_contract.enabled: false` 全局关闭;wave 模式 consumer hat 同效。

**4. 当前行为基线**:prompt 无该块;"hat 必须发什么"只在 preset instructions 自然语言与 DAG prompt(另一路径)中存在。Characterization:`receiver_contract_no_publishes_no_block` pin 住无 publishes hat 零变化(覆盖 E24 default-closed 哲学)。

**5. 输入与输出**:
- 输入:当前 hat 的 `HatConfig.publishes`(hat.rs,进入 Unit 时确认字段名,hat 配置结构在 config/hat.rs)+ `self.config.event_loop.event_policy.schemas`。
- 输出:prefix 追加 `## RECEIVER CONTRACT` 块:逐 topic `- <topic>: required fields: a, b, c`;无 schema 的 topic `- <topic>: (no schema declared)`。
- 错误:无(IO-free,全部输入已在内存)。
- 不变量:内容 100% 来自 preset 声明(hat 配置 + schema),不含 agent 生成文本;块为追加,不改既有任何段。

**6. 修改位置**:
- `crates/ralph-core/src/config/loop_config.rs`:`EventLoopConfig` 加 `receiver_contract: ReceiverContractConfig { enabled: bool(default true) }`( serde default;roundtrip/deny_unknown 测试,仿 :1228-1306 handoff_envelope 配置测试)。
- `crates/ralph-core/src/event_loop/prompt_injection.rs`:新增 `build_receiver_contract_block(hat_config, event_policy) -> Option<String>` 纯函数 + `prepend_receiver_contract` 接线;接线点:`event_processing.rs:1743` 附近(prepend_trigger_context 之后,build_isolated_prompt_with_handoff 之前——顺序变更需同步该处段间注释)。
- `crates/ralph-core/src/event_loop/event_processing.rs`:仅加一行调用(:1743 后)。
- 明确不修改:handoff_envelope 全部代码(opt-in 增强保留原样);wave_prompt.rs(worker 排除,D5 非目标)。

**7. 可依赖能力**:E8(链路覆盖证据)、E10(schema 访问模式)、E20(配置模板)、E14/E15(测试基建)。

**8. 禁止依赖的未来能力**:不动 trigger_context 渲染(U5/U6);不接 DAG(U4 另行);不改任何 preset yml。

**9. 验收测试**(event_loop/tests/receiver_contract_prompt.rs,新增并注册):
- `receiver_contract_block_lists_publishes_requirements`:config fixture hat publishes=[work.done],schema required_fields=[summary,commit] → prompt 含块、topic、两字段。
- `receiver_contract_no_publishes_no_block`(characterization):publishes 空 → 无块,prefix 逐字节不变。
- `receiver_contract_topic_without_schema_listed_by_name`:publishes 有 topic 但 schemas 无该键 → 列名 + "(no schema declared)"。
- `receiver_contract_disabled_no_block`:enabled=false → 无块。
- `receiver_contract_wave_consumer_reachable`:构造 supervisor enabled + wave 语义的 config(仿 scenarios.rs:782+ 的 supervisor fixture 或 minimal_isolated_config 变体),consumer hat build_prompt 含块——证明 wave 非 worker hat 走同链(E8 实证)。
- 运行命令:`cargo nextest run -p ralph-core -- receiver_contract`。

**10. Acceptance Red**:`receiver_contract_block_lists_publishes_requirements` 先跑:Red = prompt 无块(函数不存在→编译 Red→空实现→断言失败,真实行为 Red)。注意确认 fixture 的 hat 确实声明 publishes,否则 Red 无效(fixture 错配不属于有效 Red)。

**11. 单元测试拆分**:派生纯函数:多 topic 排序稳定(BTreeSet/排序);schema 缺失分支;enabled gate;deny_unknown 配置。不允许 mock:注入测试用真 EventLoop + 真 config parse。

**12. Red → Green → Refactor 顺序**:characterization 先绿 → 派生纯函数 Red→Green → 接线 Red→Green → wave consumer 用例 Red→Green → 配置 gate Red→Green → Refactor(段间注释同步)。

**13. 最小实现范围**:配置 + 派生 + 注入。必须保持:无 publishes/无 policy/关态零渲染;envelope 链不变;wave worker prompt 不变。**不实现**:DAG 侧(U4)、trigger 侧(U5/U6)、文档(本块内容全部来自 schema,operator 可见性低;在 U3 内同步 ralph-tools.md 核心规则一节一句话提及即可,归入 §17)。

**14. 集成验证**:receiver_contract_prompt.rs 全文件;`cargo nextest run -p ralph-core --test scenarios` 全量(prompt_contains 面 + wave BDD 面:implementation_review_wave*.yml 等 fan-in 场景确认 prompt 断言不破)。

**15. 风险驱动测试**:转义需求低(内容全是 schema 标识符),单测覆盖 topic 名含异常字符的防御即可;无外部输入。

**16. 回归范围**:`cargo nextest run -p ralph-core`;`cargo nextest run -p ralph-core --test scenarios`;`cargo nextest run -p ralph-cli --bin ralph -- dag`(确认 DAG 路径不受影响——不应触碰,若红了说明误接 DAG 链);clippy/fmt。

**17. 预期文件变更**:

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| crates/ralph-core/src/config/loop_config.rs | 修改 | ReceiverContractConfig | E20 |
| crates/ralph-core/src/event_loop/prompt_injection.rs | 修改 | 派生 + 接线 | E8/E10 |
| crates/ralph-core/src/event_loop/event_processing.rs | 修改 | 一行调用 + 注释 | E8 |
| crates/ralph-core/src/event_loop/tests/receiver_contract_prompt.rs | 新增 | 验收 | E15 |
| crates/ralph-core/src/event_loop/tests/mod.rs | 修改 | 注册 | E14 |
| crates/ralph-core/data/ralph-tools.md | 修改 | 核心规则一节一句话(块的存在与来源) | AGENTS.md:216 |

**18. 完成标准**:同前 + 全 BDD 绿 + DAG 测试面零变化实证。

**19. 停止条件**:HatConfig 的 publishes 字段形状与假设不符 → 停下;wave consumer 用例证明 wave hat 不走 isolated 链(E8 被证伪)→ 停下重查 dispatch 路径;某 BDD 因新块变红且非 substring 安全 → 停下分析(预期为零)。

**20. 风险与注意事项**:
- R1:prompt 变长(每 hat +publishes 行数);hat publishes 通常 ≤5 条,token 影响可忽略,不量化测试;
- R2:与 envelope 块并存时语义重叠——文档与注释中说明:RECEIVER CONTRACT 是 runtime 派生底线,HANDOFF ENVELOPE 是 producer 填写的增强,两者不冲突。

---

### Unit 4:DAG job prompt failure 契约补全

**1. Unit 目标**:DAG job prompt 的 emit 契约块逐字段列出 failure topic 的 required_fields(现状只给 topic 名)。

**2. 对应需求与 Scenario**:R5;Feature 2 场景 4;D6;E9。

**3. 外部可观察结果**:DAG job prompt 中 failure topic 一行变为字段清单;success 侧逐字节不变。

**4. 当前行为基线**:spawn.rs:2503 failure 只渲染 "plus its own required fields";调用点 :1998 只传 success schema(E9)。Characterization:既有 prompt 相关断言(若有)先跑绿。

**5. 输入与输出**:输入 = exec.schemas 的 failure topic schema;输出 = prompt 块多 N 行字段名。不变量:success 块、UPSTREAM ARTIFACTS 块不变。

**6. 修改位置**:
- `crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs:1998`:调用点补传 `exec.schemas.get(kind.failure_topic())`(failure topic 获取方式:进入 Unit 时 grep `failure_topic` 于 dag_scheduler/,预期与 success_topic 对称存在;若不存在,用 kind 的既有失败 topic 常量)。
- `spawn.rs:2501-2503`:渲染段补 failure 字段循环。
- 明确不修改:契约块其余部分;job_context.rs。

**7. 可依赖能力**:E9(缺口精确位置);spawn.rs 内联测试基建。

**8. 禁止依赖的未来能力**:不碰 isolated 链(U3 已完成的块不复用于 DAG——DAG 自有 builder,保持分离)。

**9. 验收测试**:`dag_job_prompt_lists_failure_required_fields`(spawn.rs 内联):构造含 failure required_fields 的 schemas → prompt 含逐字段;success 段 diff 为空。运行:`cargo nextest run -p ralph-cli --bin ralph -- dag_job_prompt`。

**10. Acceptance Red**:测试先跑:Red = prompt 无 failure 字段名(真实行为失败,现状只印 topic 名)。

**11. 单元测试拆分**:failure schema 缺失时回退现状文案(一行);failure 字段为空 Vec 时回退现状文案。不允许 mock:纯函数断言。

**12. Red → Green → Refactor 顺序**:正向 Red → 传参 + 渲染 → Green → 两个回退用例 Red → Green → Refactor(检查 spawn.rs 行数,现约 3700+,本次 +30 内,无拆分必要)。

**13. 最小实现范围**:一处传参 + 一段渲染 + 测试。**不实现**:DAG 侧 receiver 语义增强、allowed_values 等其他 schema 约束注入(残留记录)。

**14. 集成验证**:`cargo nextest run -p ralph-cli --bin ralph -- dag` 全量;`cargo nextest run -p ralph-cli --test integration_dag_scheduler`。

**15. 风险驱动测试**:无额外(纯 prompt 文本)。

**16. 回归范围**:上述两条 + `cargo run -p ralph-e2e -- --mock`(cassette 场景断言若锁 prompt 文本会变红——预期不锁,变红即停止条件);clippy/fmt。

**17. 预期文件变更**:

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs | 修改 | 传参 + 渲染 + 测试 | E9 |

**18. 完成标准**:测试绿 + 回归绿 + 可独立提交。

**19. 停止条件**:failure topic 无对称获取 API 且现有结构不支持简单获取 → 停下;ralph-e2e cassette 锁了该段 prompt 文本 → 停下评估(预期无)。

**20. 风险与注意事项**:R1:prompt 变长 ≤10 行,可忽略;R2(残留):allowed_values/element_constraints 未注入,记录为已知残留。

---

### Unit 5:trigger_context 兜底摘要(required_fields fallback)

**1. Unit 目标**:topic 未声明 `trigger_context` 时,`## TRIGGER CONTEXT` 用该 topic schema 的 `required_fields` 渲染摘要;显式声明优先;`event_loop.trigger_context_fallback: false` 恢复现状。

**2. 对应需求与 Scenario**:R6;Feature 3 场景 1/2/5;D7;E10/E21。

**3. 外部可观察结果**:无声明 preset 的下游 hat 开始收到结构化摘要(替代只有 `Event:` 原文行的现状——原文行保留不变);显式声明的 4 个 topic 渲染不变;开关关闭回到现状。

**4. 当前行为基线**:无声明即 no-op(prompt_injection.rs:825-829 gate);E21:`u3_trigger_context_prompt.rs` 的"空声明→不注入"场景将**有意变更预期**(若其 fixture topic 有 required_fields),更新该测试并在此记录理由:行为变更正是本 Unit 目标,不属于削弱断言(断言从"无块"改为"块含 required_fields 值且不含未声明字段",约束更强)。golden(:828-867)锁的是显式声明渲染,不受影响。

**5. 输入与输出**:输入 = schema.required_fields + trigger payload;输出 = TriggerContextView.summary 用 required_fields 构建。错误:required_fields 为空且无声明 → no-op(零渲染)。不变量:只渲染 schema 声明字段(leakage guard,D7);`<missing>` 语义不变。

**6. 修改位置**:
- `crates/ralph-core/src/config/loop_config.rs`:`EventLoopConfig.trigger_context_fallback: bool`(default true)+ 测试。
- `crates/ralph-core/src/trigger_context.rs`:`build()`(:254-274)增加 fallback 参数/变体:无声明时用给定字段集构建 summary(复用 extract_summary_fields :280-290)。
- `crates/ralph-core/src/event_loop/prompt_injection.rs:795-848`:gate 改为"无声明且 fallback on 且 required_fields 非空 → 用 required_fields 继续";否则维持 no-op。
- 明确不修改:renderer(:390-429)、hint 求值、format_event。

**7. 可依赖能力**:E10(字段可达);E21(测试基线);extract/render 全套纯函数。

**8. 禁止依赖的未来能力**:不多事件聚合(U6——build 仍取单事件);不改 SYSTEM_TOPICS。

**9. 验收测试**(trigger_context.rs 单测 + u3_trigger_context_prompt.rs 更新):
- `trigger_context_fallback_uses_required_fields`:无声明 + required_fields=[verdict,report_path] + payload 含 secret_note → 块含两字段值,**不含** "secret_note" 字符串(leakage 负断言)。
- `trigger_context_explicit_declaration_wins`:声明 topic → summary 只含声明字段。
- `trigger_context_fallback_disabled`:fallback=false + 无声明 → 无块。
- `trigger_context_fallback_empty_required_fields_noop`:无声明且 required_fields 空 → 无块。
- 运行:`cargo nextest run -p ralph-core -- trigger_context`。

**10. Acceptance Red**:`trigger_context_fallback_uses_required_fields` 先跑:Red = prompt 无 TRIGGER CONTEXT(真实行为失败,现状 gate 直接 no-op)。

**11. 单元测试拆分**:fallback 字段集构建;dot-path 缺失 → `<missing>`(复用既有语义);显式优先;空集 no-op。不允许 mock:builder 纯函数 + 真 EventLoop 注入测试。

**12. Red → Green → Refactor 顺序**:正向 Red → builder fallback + gate → Green → 显式优先/空集/开关 Red → Green → 更新 u3 空声明场景预期(记录理由)→ Green → Refactor。

**13. 最小实现范围**:配置 + builder 变体 + gate。**不实现**:多事件(U6)、source_hat 解析(D9)。

**14. 集成验证**:u3_trigger_context_prompt.rs 全文件;`cargo nextest run -p ralph-core --test scenarios`(无声明 preset 的 BDD prompt 断言面)。

**15. 风险驱动测试**:leakage 负断言(已列,安全关键);Property-based 不做(字段提取纯函数既有覆盖足)。

**16. 回归范围**:`cargo nextest run -p ralph-core`;`cargo nextest run -p ralph-core --test scenarios`;`cargo nextest run -p ralph-cli --bin ralph -- trigger_context`(CLI 侧引用);`bash scripts/check-cli-doc-drift.sh`(ralph-tools.md 若改);clippy/fmt。

**17. 预期文件变更**:

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| crates/ralph-core/src/config/loop_config.rs | 修改 | fallback 配置 | E20 |
| crates/ralph-core/src/trigger_context.rs | 修改 | builder fallback + 单测 | E10 |
| crates/ralph-core/src/event_loop/prompt_injection.rs | 修改 | gate | E10 |
| crates/ralph-core/src/event_loop/tests/u3_trigger_context_prompt.rs | 修改 | 空声明场景预期更新(有意行为变更,注释记录) | E21 |
| crates/ralph-core/data/ralph-tools.md | 修改 | 核心规则第 7 条补兜底语义一句 | AGENTS.md:216 |

**18. 完成标准**:同前 + u3 更新处有注释说明行为变更来源 + 全 BDD 绿。

**19. 停止条件**:某无声明 preset 的 BDD 因新块变红且非 substring 安全 → 停下分析;required_fields 在某些 schema 中含嵌套 dot-path 导致渲染异常 → 停下(预期平名字段)。

**20. 风险与注意事项**:R1:全 preset prompt 面扩大——空态/关态安全 + BDD 全绿为检测;R2:`<missing>` 增多(声明了 required 但 payload 缺)是既有语义,如实渲染。

---

### Unit 6:多事件聚合 trigger 视图(fan-in 渲染层)

**1. Unit 目标**:同一 activation 排空出多条匹配 hat triggers 的事件时,`## TRIGGER CONTEXT` 逐事件渲染摘要段(每段标注 source topic,按到达顺序);单事件渲染与 golden 逐字节一致。

**2. 对应需求与 Scenario**:R7;Feature 3 场景 3/4;D8/D9;E11/E21。

**3. 外部可观察结果**:多 producer 事件同 activation 时,consumer 看到 N 段结构化摘要而非仅最新一条;单事件零变化。

**4. 当前行为基线**:`find_matching_trigger_event` 只取最新(trigger_context.rs:86-105,last-wins 测试 :1046-1056 钉死——该测试保留,函数保留);其余事件以 `Event:` 原文行进 prompt(event_processing.rs:1594-1598,保留)。Characterization:golden :828-867 与单事件注入场景先跑绿,U6 完成后必须仍绿(零改动)。

**5. 输入与输出**:输入 = 该 hat 的 regular_events 全部匹配项;输出 = 单事件→现状渲染;多事件→`## TRIGGER CONTEXT` 下每事件一个小节(`### event <i>: <topic>` + 该事件 summary)。错误:某事件 payload 非 JSON → 该事件摘要全 `<missing>`(既有降级语义,trigger_context.rs:96-101)。不变量:leakage guard 逐事件生效;SYSTEM_TOPICS 排除不变;调度不变。

**6. 修改位置**:
- `crates/ralph-core/src/trigger_context.rs`:新增 `find_all_matching_trigger_events`(:86-105 的多事件版,保序)+ `build_multi` / render 多事件变体(单事件路径调用既有 build/render,确保 golden 不变)。
- `crates/ralph-core/src/event_loop/prompt_injection.rs:795-848`:匹配数 >1 时走多事件渲染;fallback 字段集(U5)在多事件下逐事件生效。
- 明确不修改:take_pending/dispatch(调度);format_event 原文行(保留,摘要是其补充)。

**7. 可依赖能力**:U5 的 fallback;E11 的排空语义;E14/E15 测试基建(多事件经 write_event_to_jsonl 逐条写入)。

**8. 禁止依赖的未来能力**:无后续 Unit;不做"等齐 N"屏障(D8 非目标)。

**9. 验收测试**:
- `trigger_context_aggregates_multiple_pending_events`(event_loop 注入测试):3 条匹配事件(2 个不同 topic,各含声明/required 字段)→ prompt 含 3 段、顺序 = 到达序、每段含各自 topic 与字段值。
- `trigger_context_single_event_render_unchanged`:1 条事件 → 与 golden 字符串等价(调用同一 renderer 路径,直接复用既有 golden 断言形态)。
- `trigger_context_mixed_declaration_and_fallback`:多事件中一个有显式声明、一个走 fallback → 各自正确。
- 运行:`cargo nextest run -p ralph-core -- trigger_context`。

**10. Acceptance Red**:聚合测试先跑:Red = prompt 只含最新一条的摘要(真实行为失败,last-wins)。golden 红 = 无效 Red(说明误改单事件路径,停下)。

**11. 单元测试拆分**:find_all 保序与过滤(系统 topic 排除);多事件 render 形状;非 JSON payload 事件降级;混合声明。不允许 mock:注入测试真 EventLoop + 真 JSONL 写入。

**12. Red → Green → Refactor 顺序**:golden/单事件 characterization 先绿 → find_all Red → Green → 多事件 render Red → Green → 注入接线 Red → Green → 混合声明 Red → Green → Refactor(trigger_context.rs 现 1057 行,预计 +250 内)。

**13. 最小实现范围**:find_all + 多事件渲染 + 接线。必须保持:单事件 golden 逐字节;调度零改动。**不实现**:屏障、去重、source_hat 解析。

**14. 集成验证**:trigger_context 全测试面;`cargo nextest run -p ralph-core --test scenarios`(多事件 pending 的既有场景:wave fan-in 系列确认不破)。

**15. 风险驱动测试**:State-machine 不适用;顺序断言(到达序)已列;泄漏逐事件生效已列。

**16. 回归范围**:`cargo nextest run -p ralph-core`;`cargo nextest run -p ralph-core --test scenarios`;clippy/fmt;最终 `./scripts/run-tests.sh`(全量门禁,含两阶段 nextest + doctest)。

**17. 预期文件变更**:

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| crates/ralph-core/src/trigger_context.rs | 修改 | find_all + 多事件渲染 + 单测 | E11 |
| crates/ralph-core/src/event_loop/prompt_injection.rs | 修改 | 接线 | E11 |
| crates/ralph-core/data/ralph-tools.md | 修改 | 核心规则第 7 条补多事件语义一句 | AGENTS.md:216 |

**18. 完成标准**:同前 + golden 零改动绿 + 全量 run-tests.sh 绿。

**19. 停止条件**:单事件渲染无法复用既有 renderer(golden 被迫改)→ 停下重新设计(宁可多事件走完全独立渲染函数);take_pending 语义与 E11 不符 → 停下。

**20. 风险与注意事项**:R1:极多事件(如 20 条)时块变长——render 加条目上限(参考 MAX_RENDERED_LIST_ITEMS=5 先例,handoff_envelope.rs:145),超出折叠为 "...(and N more)",单测覆盖;R2:与 U5 fallback 叠加时字段集可能各不相同,逐事件独立计算,单测覆盖。

## 8. Unit 串行依赖图

```
Unit 1(workstate store + CLI)
  ↓
Unit 2(workstate 注入 + 配置 + 归档)
  ↓
Unit 3(receiver contract 注入)
  ↓
Unit 4(DAG failure 契约)
  ↓
Unit 5(trigger 兜底)
  ↓
Unit 6(多事件聚合)
```

- **U2 依赖 U1**:注入消费 U1 的 store API 与 JSONL 文件;无 U1 则无数据源。不可交换。
- **U6 依赖 U5**:多事件视图逐事件复用 U5 的 fallback 字段集语义(混合声明场景);U5 先行可在单事件路径上独立验证 fallback。不可交换。
- **U3/U4 独立于 U1/U2**:不同 prompt 路径、不同文件;排在 U2 后仅为线性纪律与回归基线递增(每 Unit 收尾全 core 绿,后续 Unit 的 Red 解释不受前序半成品干扰)。
- **U4 与 U3 独立**(DAG 自有 builder),但同属 GAP-02,相邻执行便于统一审查 contract 语义。
- 避免提前实现:U1 禁止注入接线;U2 禁止 contract/trigger 改动;U5 禁止多事件;各 Unit §8 已列。

## 9. 执行命令清单

| 命令 | 时机 | 目的 | 预期 | 失败可否继续 |
|---|---|---|---|---|
| `cargo nextest run -p ralph-core -- workstate` | U1/U2 每步 | store 与注入 | Red→Green | 预期 Red 时可继续 TDD |
| `cargo nextest run -p ralph-cli --test integration_workstate` | U1 每步 | 进程级 CLI 验收 | Red→Green | 同上 |
| `cargo nextest run -p ralph-cli --test integration_memory` | U1 收尾 | 相邻 CLI 回归 | 绿 | 否 |
| `cargo nextest run -p ralph-core -- event_loop` | U2/U3/U5/U6 每步 | 注入链 | 绿 | 否 |
| `cargo nextest run -p ralph-core -- receiver_contract` | U3 每步 | contract 注入 | Red→Green | 同上 |
| `cargo nextest run -p ralph-cli --bin ralph -- dag` | U3/U4 收尾 | DAG 面回归(U3 应零影响;U4 目标面) | 绿 | 否 |
| `cargo nextest run -p ralph-core -- trigger_context` | U5/U6 每步 | builder + 注入 | Red→Green | 同上 |
| `cargo nextest run -p ralph-core --test scenarios` | U2/U3/U5/U6 收尾 | BDD prompt_contains / fan-in 面 | 绿 | 否 |
| `cargo nextest run -p ralph-cli --test integration_dag_scheduler` | U4 收尾 | CLI 平面回归 | 绿 | 否 |
| `cargo run -p ralph-e2e -- --mock` | U4 收尾 | cassette 场景 | 绿 | 否 |
| `cargo nextest run -p ralph-core -- skill_registry capability_inventory` | U2 收尾 | 文档注册/anchor | 绿 | 否 |
| `cargo nextest run -p ralph-core --test ralph_tools_doc_drift` | U2/U5/U6 收尾 | 文档结构门禁 | 绿 | 否 |
| `cargo build -p ralph-cli && bash scripts/check-cli-doc-drift.sh` | U2/U5/U6 收尾 | CLI flag 文档漂移 | 绿 | 否 |
| `cargo nextest run -p ralph-core` | 每 Unit 收尾 | core 全量 | 绿 | 否 |
| `cargo clippy --all-targets` / `cargo fmt --check` | 每 Unit 收尾 | Lint/格式 | 绿 | 否 |
| `./scripts/run-tests.sh` | 最终门禁(U6 后) | 全量两阶段 + doctest | 绿 | 否 |

注意:全部测试入口为 nextest 系列(HARD RULE 1/2);ralph-cli 进程级集成测试一律 `common::ralph_bin()` scrub(HARD RULE 5),模拟 agent 的用例先 scrub 再显式 `.env(...)`;`ralph-core` 无 supervisor-db 默认特性,本计划不触碰 supervisor/DAG store,无需 `--features supervisor-db`。

## 10. 最终质量门禁

- §4 三个 Feature 的全部 Scenario 有对应测试且绿;R1-R8 每条至少一个可执行测试(§6 矩阵);
- 无新增跳过/`.only`/削弱断言;唯一预期变更的既有断言 = u3_trigger_context_prompt.rs 空声明场景(U5,有意行为变更,注释记录理由);
- golden(trigger_context.rs:828-867)零改动保持绿;
- 空态/关态逐字节等价:`workstate_prompt_empty_store_no_block` / `receiver_contract_no_publishes_no_block` / `trigger_context_fallback_disabled` 三个 pin 全绿;
- `ralph_tools_doc_drift` / `capability_inventory` / `skill_registry` / `check-cli-doc-drift.sh` 四项静态门禁全绿;
- `./scripts/run-tests.sh` 全绿;clippy/fmt 绿;
- 决策置信度未因实现发现跌破 0.85;无未处理 BLOCKED;
- 六个 Unit 各自完整 TDD 闭合并按序完成、各自可独立提交。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 每 Unit 落到具体文件/行号锚/测试名 |
| Executor 是否仍需做关键设计决策 | 否 | D1-D9 全部定案;两处"进入 Unit 时确认"(hat publishes 字段名、failure_topic 获取 API)为查证动作而非决策,均已给出预期与停止条件 |
| 所有文件和接口是否有代码库证据 | 是 | E1-E24;新增文件均已标注"新增" |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | §3 最低 0.85(D2/D3/D4/D5/D7/D8/D9) |
| 是否存在未处理的低置信度假设 | 否 | §1 待验证假设:无 |
| 每个 Unit 是否只有一个可观察行为 | 是 | CLI 读写 / 注入+归档 / contract 注入 / DAG 补全 / 兜底 / 聚合 |
| 每个 Unit 是否可以独立验证 | 是 | 各 Unit §9/§14/§16 |
| 每个 Unit 是否有真实 Red | 是 | 各 §10(含新 API 编译 Red 的如实标注) |
| 每个 Unit 是否包含回归范围 | 是 | 各 §16 |
| 是否存在未来 Unit 依赖 | 否 | §8 |
| 是否存在泛化任务描述 | 否 | 修改点均带行号锚;新增文件均标注 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §6 矩阵 |
| 所有关键决策是否有 Evidence | 是 | §3 支持证据列 |
| 计划是否可以严格串行执行 | 是 | §8 |

## 12. 残留记录(明确不在本计划闭环)

- GAP-02 的 envelope default-on 不实施(D5 决策记录理由);receiver contract 不覆盖 wave worker(E12 证据,叶子 producer + slot 不可寻址);
- DAG 契约块不注入 allowed_values/element_constraints(U4 残留);
- `source_hat` 在 trigger context 注入层保持 None(D9);
- workstate.jsonl 主仓侧无压缩/清理策略(U1 R2,量小);
- GAP-05/06/07(P2)与 GAP-08(P3)留待后续独立 plan。
