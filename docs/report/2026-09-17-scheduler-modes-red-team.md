# 调度模式三态机制(wave / dag_shadow / dag)红队诊断报告

- **日期**:2026-09-17
- **基线**:HEAD `1e3298cb`(`docs: 补齐调度模式（wave/dag_shadow/dag）文档与 skill 适配`)
- **对象**:`event_loop.supervisor.scheduler_mode` 三态机制及其文档宣称(`docs/explanation/scheduler-modes.md`、`docs/advanced/dag-scheduler.md`、AGENTS.md/CLAUDE.md 相关段落、agent 注入文档 `crates/ralph-core/data/ralph-tools-emit.md`)
- **方法**:静态代码链审计 + 4 个实证 PoC(3 个 nextest 临时测试 + 1 个 CLI 实测)。所有 PoC 代码已复原,未提交任何改动。
- **证据等级**:【实证】= 写了 PoC 并跑出结果;【静态实锤】= 代码链每一环均亲自读码闭合;【静态推断】= 关键锚点已复核,完整路径未执行。

---

## 结论摘要

三态机制的**文档宣称与实现之间存在系统性断裂**,且全部偏向同一个危险方向:文档承诺了比实现更强的保证。最严重的是 dag 模式的两条命门——runtime 自己发射的协调事件按静态链路**不可能通过 origin guard**(ack 回路断裂),以及文档宣称的 exactly-once 去重机制**根本不存在**(对应表与拒绝码零生产引用)。dag_shadow 则是一个**幻影模式**:文档描述的"旁路 dry-run 观察"在生产路径上不会发生,因为 DAG runtime 只在 `dag` 模式下构建。

| # | 严重度 | 发现 | 证据等级 |
|---|---|---|---|
| F1 | P0 | dag 模式 runtime 自发射的 `forge.unit.integrated` / `forge.exec.development.done` 被 origin guard 拒收,ack 回路断裂,流水线在首个 unit 集成后死锁 | 【静态实锤 + guard 层实证】 |
| F2 | P0 | 文档宣称的 exactly-once(`dag_terminal_deliveries` 三元组去重 + `duplicate_forge_unit_executed` 拒收)不存在;真实 fence 是另一张表、另一种粒度 | 【静态实锤】 |
| F3 | P1 | blocked plan 仍发射 `forge.exec.development.done`,违反本模块自声明的不变量 | 【实证】 |
| F4 | P1 | `dag_shadow` 是幻影:DAG runtime 不构建,shadow 观测永不发生;parallel-forge 切 shadow 会静默停摆 | 【静态实锤】 |
| F5 | P1 | 「dag 模式 legacy bridge 故意不构建」不成立:else 分支照建,引入跨模式污染与第二 spawn 权威 | 【静态实锤】 |
| F6 | P1 | 配置层两个静默降级口:错层书写被 serde 吞掉;operator 声明 `supervisor` 任意子键即静默丢弃 preset 的 `scheduler_mode: dag` | 【实证 + 静态实锤】 |
| F7 | P2 | 文档引用的 `ralph loops clean --ledger` 命令不存在,残留 dag.db 无官方复位手段 | 【实证】 |
| F8 | P2 | `ralph inspect loop` 并非只读(open 必跑 migrations);scheduler 块的 mode 标签与账本计数无一致性校验 | 【静态实锤】 |
| F9 | P2 | worktree 复用(`--reuse-worktree`)路径跳过 DAG 恢复,in-flight job 成孤儿 | 【静态实锤】 |
| F10 | P3 | 攻击面清单(6 项,未逐一手验) | 【静态推断】 |

---

## F1 [P0] dag 模式 runtime 协调事件过不了 origin guard —— ack 回路断裂

### 宣称

- `docs/advanced/dag-scheduler.md:136`:「`forge.unit.integrated` …runtime 在 lane CAS fast-forward 后发射,不由 agent 发送」。
- `crates/ralph-cli/src/loop_runner/dag_scheduler/mod.rs:645-647` 注释自述设计意图:「the runtime-emitted integrated event came back through **real acceptance** (the close-task projection ran) — ack the durable record and unlock dependents」。

### 实际代码链(逐环闭合)

1. **写盘**:runtime 通过 `append_supervisor_coord_event` 落盘,硬编码 `system_injected: true`、`source: "ralph"`;`forge.unit.integrated` 不匹配 exec/fix/review 前缀,`hat` 落入 else 分支 = `"ralph"`(`crates/ralph-cli/src/loop_runner/wave/dispatcher/coordination.rs:890-891, 901-908`)。调用点:`dag_scheduler/integrate.rs:147`(恢复重发)、`:350`(development.done)、`:594`(集成后发射)。
2. **读回第一关(isolated scope 分区)**:`system_injected == Some(true)` 有 bypass,进 `accepted`(`crates/ralph-core/src/event_loop/parse_and_emit/legacy.rs:591-611`)。**此关能过。**
3. **读回第二关(origin guard)**:isolated 分支的输出继续进 `filter_events_by_origin`(`legacy.rs:1520-1525`)→ `validate_event_origin` 的第一个分支(`crates/ralph-core/src/event_origin.rs:416-426`):`system_injected == Some(true)` 且 topic 不在 `SUPERVISOR_COORDINATION_TOPICS`(仅 6 个 `exec/fix/review.wave.{complete,failed}`,`event_origin.rs:133-140`)、也不匹配 `*.wave.complete/failed` 后缀(`event_origin.rs:152-154`)→ **Rejected { reason: "system_injected_on_business_topic" }**。
4. **兜底也过不去**:即使第 3 环被豁免,`hat: "ralph"` + business topic 还会撞 `ralph_control_only` 拒收(`event_origin.rs:473-483`)。
5. **后果**:ack 回路 `observe_accepted_events` 只接收 **accepted** 事件(`crates/ralph-cli/src/loop_runner/inner.rs:3738-3739`)→ `on_unit_integrated_accepted`(`dag_scheduler/mod.rs:648-650` → `integrate.rs:234`)**永不触发** → `plan.integrated` 永不填充 → 依赖 unit 不解锁、`forge.exec.development.done` 不发 → **流水线在第一个 unit 集成后死锁**。`dag_scheduler/mod.rs:137` 的注释("Units whose `forge.unit.integrated` event was accepted")证实设计就是依赖 accepted 回流。
6. **无特判**:`crates/ralph-core/src/event_loop/` 全目录 grep `forge.unit.integrated` / `UNIT_INTEGRATED` **零命中**——event loop 对 dag runtime 协调 topic 没有任何豁免。`DAG_RUNTIME_UNIT_TOPICS`(`event_origin.rs:192-199`)只被 preset_lint / runtime_contract 静态检查使用,其文档注释(:201-205)明说「topic name alone is not sufficient for an exemption」。

### PoC(已执行,已复原)

在 `event_origin.rs` 测试模块临时加入 `redteam_runtime_emitted_dag_coord_event_origin_guard`:用 `append_supervisor_coord_event` 的真实产出形状(`system_injected: true, hat: "ralph"`)构造 `forge.unit.integrated` 与 `forge.exec.development.done` 两个事件喂给 `validate_event_origin`。

**结果:PASS**——两个 topic 均被拒,reason 恰为 `system_injected_on_business_topic`。拒绝行为被实证。

### 为什么现有测试全绿

测试全部绕过了真实接受链路:

- `integrate.rs` 的测试直调 `on_unit_integrated_accepted`(如 :1091、:1295),不经过事件读写;
- e2e(`crates/ralph-e2e/src/scenarios/parallel_forge.rs:311-312`)是 **mock agent 自己**把 `<event topic="forge.unit.integrated">` 文本吐进 stdout——文本解析出的事件 `system_injected: None, hat: None`,走 `event_origin.rs:441-443` 的 no-hat 放行,与生产路径(runtime 写盘、system_injected=true)完全不同;
- BDD scenario 同样用 mock hat 文本事件。

没有任何测试覆盖「runtime 写盘 → EventLoop 真实接受 → ack」这条回路。

### 影响与诚实声明

若此链成立,**dag 模式从未在生产端到端跑通过**——这与「builtin parallel-forge 已启用 dag 执行面」的宣称(AGENTS.md)直接冲突。我对第 1-6 环每一环都亲自读码确认,且 guard 行为有 PoC 实证;但未跑过一个完整的真实 dag loop 做终验(需要 mock backend 全链路)。**建议的最小终验**:起一个 mock-backend 的 dag loop,跑到首个 unit 集成,然后检查 `.ralph/events.jsonl` 中 `forge.unit.integrated` 行是否伴随 `policy_receipt` 拒收记录(`legacy.rs:1549-1557` 会为 origin 拒收写 receipt)。

### 修复方向

把 `forge.unit.integrated` / `forge.exec.development.done` 纳入 origin guard 的 system_injected 白名单(以 `scheduler_mode == dag` 为门控,防止 wave preset 里 agent 伪造),或为 runtime 协调写入建立不走 JSONL 重读的专用信任通道。

---

## F2 [P0] exactly-once 宣称是文档虚构

### 宣称

- `docs/advanced/dag-scheduler.md:123`:「DAG runtime 通过 `dag_terminal_deliveries` 持久去重表保证终态事件恰好交付一次:同一 `unit_key` + `job_id` + `job_token` 三元组的 `forge.unit.executed` 只能成功发送一次,重复发送被 runtime 拦截(emitter 会收到 `duplicate_forge_unit_executed` 类拒绝)」。
- `crates/ralph-core/data/ralph-tools-emit.md:662`(**agent 注入文档**):「同一 `unit_key` + `job_id` + `job_token` 三元组只能成功发送一次(重复发送由 runtime 持久去重表 `dag_terminal_deliveries` 拦截)」。

### 实际

- `duplicate_forge_unit_executed`:全仓 grep **零 Rust 命中**,只出现在上述两份文档里。
- `dag_terminal_deliveries`(migration v20):仅被 `crates/ralph-core/src/supervisor/migrations.rs:620, 744, 766`、`migrations/v20.sql` 和两个 doc-drift 测试引用——**没有任何生产读写方**。`terminal_delivery.rs` 文件自述 SKELETON-ONLY。同病:v19 `dag_checkout_intents`、v21 `dag_registration_evidence`/`dag_approval_evidence`、v22 `dag_correction_requests`、v23 `dag_integration_failures` 全部只有 migration 引用。AGENTS.md:143「SQL migrations v19-v23(DAG store)已落地」的表述因此具有误导性——表存在,机制不存在。
- 真实存在的去重是另一张表 `dag_terminal_emits`:`INSERT OR IGNORE`,**PRIMARY KEY 是 `(plan_key, topic)`**(`crates/ralph-core/src/supervisor/dag_store_rusqlite.rs:315-337`)——plan 粒度,不是文档说的 unit 三元组粒度;且它 fence 的是 runtime 终态事件的发射,不管 agent 发的 `forge.unit.executed`。

### 放大因素:job_id / job_token 是确定性可预测的

`crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs:1925, 1933`:

```rust
let job_id = format!("dag-{bare_unit_id}-{}-a{attempt}", kind.stage_str());
...
token: format!("tok-{bare_unit_id}-{}-a{attempt}", kind.stage_str()),
```

任何 DAG job 里的 agent 都能纯函数地算出所有兄弟 unit 的 job_id/token。`docs/advanced/dag-scheduler.md` 把这三元组描述成去重凭据,但它既不构成去重键(真实 fence 不用它),也不构成不可伪造性凭据。

### 影响

agent 注入文档(`ralph-tools-emit.md` 每次注入)在告诉 agent 一个假的安全边界;operator 按手册做崩溃恢复可靠性假设时会落空。重复的 `forge.unit.executed` 到达 seam 后的真实行为由 driver 的 stage 迁移合法性决定(非法迁移 → `fail_job(ContractViolation)` → 合成失败事件),即「重复即灾难」而非「重复被幂等拦截」。

### 修复方向

二选一:(a) 接线 v20 表与三元组去重,兑现文档;(b) 把 `dag-scheduler.md:123` 与 `ralph-tools-emit.md:662` 改写为 `dag_terminal_emits` 的真实语义(plan+topic 粒度 fence,只管 runtime 终态事件)。考虑到 `ralph-tools-emit.md` 是注入文档,这是 AGENTS.md「AI skill guide 同步规则」的硬违规。

---

## F3 [P1] blocked plan 仍发射 forge.exec.development.done

### 宣称

`crates/ralph-cli/src/loop_runner/dag_scheduler/mod.rs:238-239`,字段注释:「A blocked plan is never admitted or **allowed to emit completion**」。

### 实际

- `maybe_emit_development_done` 的候选过滤(`integrate.rs:295-313`)只检查「所有 unit ∈ `plan.integrated`」,**不查 `blocked_plans`**。
- `recover_after_restart`(`mod.rs:391-404`)仅凭 durable ack 记录就把 unit 灌进 `plan.integrated`,不要求 trusted ledger 事件存在。
- 随后 `reconcile_after_restart`(`integrate.rs:118-124`,由 `recover_after_restart` 在 `mod.rs:564` 调用)发现「ack 存在但 ledger 事件缺失」→ `block_plan`。
- 两个状态并存:plan 既在 `blocked_plans` 里,又满足 all-integrated → tick 继续时 `maybe_emit_development_done` 照样 append `forge.exec.development.done` 并赢下 fence(`integrate.rs:342-367`)。

### PoC(已执行,已复原)

在 `integrate.rs` 测试模块临时加入 `redteam_blocked_plan_with_full_acks_must_not_emit_development_done`:两个 unit(U1/U2)全部 `record_integrated` + `ack`、ledger 中无 integrated 事件 → 重启恢复 → 断言 `blocked_plans` 含该 plan 且 `plan.integrated.len() == 2`(均通过)→ 调 `maybe_emit_development_done` → 断言 ledger 中 `forge.exec.development.done` 为 0。

**结果:FAILED,left = 1**——被 block 的 plan 的终态事件实际写入了 ledger。缺陷实证。

既有测试 `recovery_blocks_acked_integration_without_ledger_event`(`integrate.rs:1192-1235`)只 ack 了 U1(共 2 个 unit),`all(units integrated)` 恰好不满足,所以绿灯——它守不住「全部 ack」这个组合。

### 影响

崩溃窗口(ack 已落盘、事件未落盘)恢复后,被明确 block 的 plan 照样对下游宣告「开发完成」,reporter/closer 会基于虚假完成信号收尾。修复一行:候选过滤加 `!self.blocked_plans.contains(plan_key)`。

---

## F4 [P1] dag_shadow 是幻影:观测在生产路径上不发生

### 宣称

- `docs/explanation/scheduler-modes.md:84`:「DAG 调度器只计算并记录『如果我来调度,会放行/阻塞什么』,零副作用」;
- `docs/advanced/dag-scheduler.md:42, 71`:「DAG 调度器在旁路跑 dry-run」「shadow 观测在进程内按 tick 记录」;
- `inner.rs:1008-1016` 注释:「Constructed only when the preset opts into a runtime-owned scheduler mode (`dag_shadow` / `dag`)」。

### 实际

- 构建门槛是 `scheduler_mode == SchedulerMode::Dag`(**不含 DagShadow**):`crates/ralph-cli/src/loop_runner/inner.rs:1017-1031`。
- `DagSchedulerRuntime::new` 的生产调用点全仓唯一(inner.rs:1024),其余全部在测试中(grep 实证)。
- `compute_shadow_observation` 的唯一生产调用在 runtime 的 tick 内(`dag_scheduler/mod.rs:1223`)——runtime 不存在,观测永不发生。`observe_accepted_events`(inner.rs:3738-3739)挂在 `dag_scheduler.as_mut()` 上,shadow 模式恒为 `None`。
- 即「零副作用」字面上成立,但成立方式是「**根本没跑**」,不是「跑了但克制」。inner.rs:1008-1016 注释与它正下方的代码直接矛盾。

### 连锁后果

1. **cutover 工作流无数据源**:`scheduler-modes.md:150` 步骤 2 让 operator「用 inspect 的 scheduler 块观察 shadow 决策与旧权威的差异」——shadow 下该块恒为零计数(F11 把原因误述为「没有持久记录」,真实原因是没有观测)。
2. **parallel-forge 切 shadow 即静默停摆**:其 executor/reviewer/verifier hat 是 `runtime_driven: true`(`presets/en/parallel-forge.yml:720, 831, 870`),而 runtime-driven 抑制只在 `== Dag` 生效(`crates/ralph-core/src/event_loop/dispatch_and_handoff.rs:35-42`);shadow 下既没有 runtime spawn 它们,wave 路径也没有触发拓扑(executor 无有效 trigger 链)→ guardian 批准后 loop 假死,inspect 全零——「零风险验证窗」产出零信息。

### 修复方向

要么把 inner.rs:1017 的门槛改为 `!= Wave`(spawn 面有 `spawn.rs:27-29` 的 `== Dag` 二次门槛兜底,副作用仍被锁住),并补「门槛 pin 测试」;要么把 `dag_shadow` 从文档与选型表下架,承认它从未工作。

---

## F5 [P1] dag 模式「bridge 故意缺席」不成立 —— else 分支照建

### 宣称

- `inner.rs:712-717` 注释:「In dag mode the runtime-owned DAG scheduler is the sole execution authority, so the legacy bridge is **deliberately absent**」;
- `docs/explanation/scheduler-modes.md:89`:「legacy supervisor bridge 故意不构建(inner.rs:719-720 的 scheduler_mode != Dag 条件),不存在第二权威」。

### 实际

`supervisor_path_enabled`(`inner.rs:719-720`)在 dag 模式下为 false,控制流落入 **else 分支**(`inner.rs:871-1001`):只要 `supervisor.db_path` 非空,就照样 `build_supervisor_bridge` + `recover_active_waves_at_startup` + `inject_timed_out_failed_coord`(:887-969)。而 builtin `parallel-forge` 恰好声明了 `db_path: .ralph/supervisor.db`(`presets/en/parallel-forge.yml:165`)——**生产 dag 配置下 bridge 必建**,且首次运行还会顺带创建 supervisor.db 文件,与「账本刻意分离」的宣称(dag-scheduler.md:88)矛盾。

### 后果

1. **跨模式污染**:同一仓库历史上跑过 wave 模式留下的 pending wave,会在 dag 模式启动时被 `inject_timed_out_failed_coord` 以 system_injected 的 `exec.wave.failed` 注入主账本(:935-939),可能激活 forge-failure-handler。
2. **第二 spawn 权威**:`--continue` 时 redrive boot scan(:1218-1224 起)在 `Some(bridge)` 下会真实 spawn wave worker。
3. **可用性耦合**:dag 模式下 supervisor.db 打不开会 abort 整个 loop(:981-995 fail-closed)——一个宣称「不存在」的组件却能杀死 dag loop。

### 修复方向

dag 模式显式跳过 else 分支的 bridge 构建(或反之为该组合 fail-closed),并把 `inner.rs:712-717` 注释改到与代码一致。

---

## F6 [P1] 配置层两个「静默降级 wave」口

文档宣称(`scheduler-modes.md:60`):「**无静默降级**:非法组合不会回落到 wave,而是直接拒绝启动」。以下两个口子使该宣称不成立。

### F6a 错层书写被 serde 静默吞掉【实证】

`EventLoopConfig` 没有 `#[serde(deny_unknown_fields)]`(`crates/ralph-core/src/config/loop_config.rs:335-336`)——对比 `SupervisorConfig` 有,且有测试钉死(`loop_config.rs:1118-1129`)。preset 作者把 `scheduler_mode: dag` 写到 `event_loop:` 直下(漏一层 `supervisor:`)时,serde 静默忽略未知键 → 生效值 = `wave` → 校验通过 → loop 以 legacy wave 启动,零警告。

**PoC**:`redteam_mislayered_scheduler_mode_is_silently_ignored`(临时加入 loop_config.rs 测试模块,已复原)——错层 YAML `serde_yaml::from_str::<RalphConfig>` 解析**无错误**,生效 `scheduler_mode == Wave`。**PASS,缺陷实证**。

### F6b operator 声明 `supervisor` 任意子键 → preset 整个 supervisor 子树静默丢弃【静态实锤】

`supervisor` 在 `PRESET_OPT_IN_WHEN_OPERATOR_OMITS`(`crates/ralph-cli/src/preflight.rs:911`),合并是整棵子树粒度(preflight.rs:1179-1186):operator 的 ralph.yml 一旦声明 `event_loop.supervisor` 的**任何**子键(例如只想改 `db_path`),preset 的整个 supervisor 块(含 `scheduler_mode: dag` + `dag_pools`)被跳过,**无 warning**——:1206-1212 的 eprintln 警告只覆盖另一类被安全边界过滤的 key,opt-in 抢占分支完全静默。

对 builtin parallel-forge 这会被 `runtime_driven` 校验偶然兜住(掉回 wave 后 preflight 拒绝);但任何没有 runtime_driven hat 的自定义 dag preset 都被静默降级为 wave。

### 修复方向

F6a:给 `EventLoopConfig` 加 `deny_unknown_fields`(注意配置合并是先 YAML 层 merge 再反序列化,需验证 overlay 白名单键不受影响)。F6b:opt-in 抢占分支加 eprintln warning,或把 supervisor 改为字段级合并。

---

## F7 [P2] 文档引用不存在的命令 `ralph loops clean --ledger`

- 宣称:`AGENTS.md:143` / `CLAUDE.md:143` / `docs/advanced/dag-scheduler.md:94` 均以「`ralph loops clean --ledger` + migration runner」作为旧库升级/复位路径。
- 实际:`LoopsCommands` 枚举全集(`crates/ralph-cli/src/loops.rs:44-83`)为 list/logs/history/retry/discard/stop/resume/prune/attach/diff/merge/process/merge-button-state,**没有 clean**。
- **CLI 实证**:`./target/debug/ralph loops clean --ledger` → `error: unrecognized subcommand 'clean'`。
- 叠加文档自认的 F11 限制(dag-scheduler.md:82:「切换模式前若不复位账本,读数可能混有历史数据」)→ operator 面对残留 dag.db **没有任何官方复位手段**(手动删文件又被「不要手动编辑 .ralph」规则禁止)。

---

## F8 [P2] inspect 的两个宣称问题

1. **「只读」不成立**:`build_scheduler_summary`(`crates/ralph-cli/src/commands/inspect.rs:1431`)打开 dag.db 走 `DagConnection::open`(`crates/ralph-core/src/supervisor/dag_store_rusqlite.rs:97-141`),**每次 open 必跑 `migrations::run`**(:115)——含 WAL journal mode 切换与按需 DDL/`user_version` 写。`inspect.rs:1398-1400` 与 `dag-scheduler.md:64` 的「read-only / 只读入口」宣称字面不成立;对正在被 loop 持有的库还引入 WAL/busy 竞争(有 5 次 busy 重试兜底)。好的一面:`inspect.rs:1425` 有 `db_path.exists()` 前置守卫,inspect 不会**创建** dag.db,懒打开宣称不受影响。
2. **mode 标签与账本计数无一致性校验**:mode 标签取自**当前配置文件**(`inspect.rs:1408-1418`),receipt 计数取自**盘上 dag.db**(:1424-1436)。`dag → wave → dag_shadow` 切换后,旧 dag run 的 receipt 尸体数据会贴着 `dag_shadow` 标签显示;db 损坏时静默降级为空块(仅 `tracing::debug`,:1446-1452)。在 F4(shadow 无真实观测)的背景下,这是 operator 在 shadow 模式下唯一能看到的非零读数来源——**尸体数据可能被误读为「shadow parity 达标」而触发过早 cutover**。

---

## F9 [P2] worktree 复用路径跳过 DAG 恢复

`inner.rs:1213-1215`:`dag.recover_after_restart()` 只在 `resume`(`--continue`)为真时执行。`--reuse-worktree --plan X` 的 manifest 复用通道 resume=false → **不跑 DAG 恢复**:dag.db 里 terminal=NULL 的旧 job 不被 adopt/settle(孤儿进程无人收割),重放 `forge.plan.ready` / `forge.concurrency.approved` 后 spawn 撞 replay 去重被静默跳过 → unit 永久卡住。docs 宣称的「身份校验通过则 task.resume 从 pending hat 继续」(dag-scheduler.md:108)对 dag 模式 in-flight job 不成立。

---

## F10 [P3] 攻击面清单(静态推断,关键锚点已复核但未逐一手验完整路径)

1. **跨 plan job_id 碰撞 → 静默卡死**:job 去重查询 `WHERE job_id=?1 OR token=?2` 不带 plan_key(`crates/ralph-cli/src/loop_runner/dag_scheduler/jobs.rs:154-163`),而 job_id 不含 plan_key(spawn.rs:1925);两个并发 plan 含同名 unit 时后一个 reserve 冲突,spawn 路径只 warn 后 return(约 spawn.rs:1975-1978),不写 terminal、不发失败事件 → pipeline 槽位被占,永久静默卡死。
2. **unit id 字符集分层不一致**:journal `JobIdentity::validate` 只许 `[A-Za-z0-9-_]`,上游 handoff/校验更宽 → plan 里写 `id: U.1` 一路绿灯到 spawn 才被拒,同样静默卡死。
3. **`ralph wave emit` 对配置校验失败 warn-and-proceed**:`crates/ralph-cli/src/wave.rs:1148-1160`,配置解析失败(含 scheduler_mode 非法)时打印 warning 并**关闭 policy enforcement** 继续,与 fail-closed 宣称在 emit 面上不一致。
4. **plan_key 无字符集校验**:进 inspect JSON 的 `plan_keys` 是未校验字符串(`dag_plan_receipt.rs:116-130` 无约束;对比 JobIdentity 有字符集白名单),可含 ANSI 转义序列,与 inspect.rs:1395-1397 的「脱敏」自述有差距。
5. **合成失败的崩溃窗口**:失败终态「先写 journal terminal 再排队 merge」(spawn.rs:740-741 一带);恢复重驱分支(mod.rs:465-513 一带)要求 `worker_events` 非空,而合成失败(timeout/contract violation)的 events file 为空 → failure 事件永久丢失,plan 无声卡死。
6. **`record_stage_evidence` fail-soft**(spawn.rs:1452-1461 一带,写失败仅 warn):「下一阶段 base 钉在上一阶段 accepted commit」的证据链在 store 写失败时静默退化为 plan base。

---

## 验证过程与复原记录

- 临时改动 3 个源文件各加一个 PoC 测试:`event_origin.rs`(F1)、`loop_config.rs`(F6a)、`integrate.rs`(F3);另执行 1 个 CLI PoC(F7)。
- 运行方式遵守仓库 HARD RULE 1:`cargo nextest run -p ralph-core -- redteam_...` / `cargo nextest run -p ralph-cli --bin ralph -- redteam_...`。
- PoC 结果:F1 PASS(拒绝行为证实)、F6a PASS(静默吞键证实)、F3 **按预期 FAIL**(left=1,blocked plan 实际发射了终态事件,缺陷证实)、F7 clap 报错证实命令不存在。
- **复原**:三个文件已 `git checkout` 还原,`git status` 干净,未提交任何 commit。

## 修复优先级建议

1. **先裁决 F1**:写一个真实 scenario(mock backend + dag 模式,跑到首个 unit 集成)终验 ack 回路;若证实,这是 dag 模式的可用性命门,修 origin guard 白名单。
2. **F2 文档与实现对齐**:agent 注入文档里的假安全边界优先清除(要么接线 v20,要么改写为 `dag_terminal_emits` 真实语义)。
3. **F3 一行修复**:`maybe_emit_development_done` 候选过滤加 `blocked_plans` 排除,并把既有测试补一个「全 ack」变体。
4. **F4/F5 门槛与注释对齐**:决定 dag_shadow 的生死(接线或下架);dag 模式跳过 else 分支 bridge。
5. **F6 配置静默口**:`deny_unknown_fields` + opt-in 抢占 warning。
6. **F7-F9 文档/操作面**:删除或实现 `loops clean --ledger`;inspect 标注「会触发 migration」或加只读打开模式;worktree 复用路径补 DAG 恢复。
