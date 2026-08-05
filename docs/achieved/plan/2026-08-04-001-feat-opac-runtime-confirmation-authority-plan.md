---
artifact_contract: ce-unified-plan/v1
artifact_readiness: partially-ready
product_contract_source: ce-plan-bootstrap
execution: code
title: "feat: OPAC Task runtime confirmation 与 authority 调查"
date: 2026-08-04
type: feat
baseline: b439a803
---

# 0. 计划状态

**PARTIALLY READY**。当前只有 Task confirmation 的最小纵向切片达到可执行条件；Wave Confirm 是现有能力，先作为 characterization/regression 边界，不重复规划实现。runtime-owned ticket authority 仍 BLOCKED，不能让 Executor 自行选择 IPC、secret、平台降级或认证协议。

**执行状态（2026-08-05 更新）**：Unit 1 已完成并通过全部验收与质量门禁（commits `d5ea7ffc`..`4ed44310`，分支 `pittcat-dev`）。落地内容：`ralph tools task confirm <id> --reference --digest`、Apply 同快照铸造 pending confirmation、claim 前 fail-closed precheck（稳定 token `confirmation_required`）。验证结果：S1–S4 集成测试 + 审核循环补测共 27/27；四组单元测试落地；`integration_wave_protocol_closure` 7/7 无回归；`cargo build` / `cargo clippy --all-targets --all-features -- -D warnings` / `check-cli-doc-drift.sh --strict` / `./scripts/run-tests.sh` 全量（7493+23+19）全过；`integration_emit_policy` 未触发（Unit 1 调用链未触及 emit context）。4 维度并发代码审核发现并修复：跨 scope confirmation 覆盖洞（fail-open，新增 `confirmation_scope_conflict` 拒绝）、`execute_confirm` 竞态误报。D2 仍 BLOCKED、authority 未实现；计划整体保持 PARTIALLY READY，Unit 2 未解锁。

- 当前基线：`pittcat-dev`，HEAD `b439a803`；本计划不触碰该基线已有的 merge-batch 变更。
- 调查范围：Task CLI/JSONL store、Task OPAC ticket gate、Wave emission store/inspect/gate、agent context、loop worker spawn、OPAC skill 文档、现有 integration/BDD/nextest 入口及相关 Git 历史。
- 已执行验证：
  - `cargo nextest run -p ralph-cli --test integration_tasks`：16/16 通过；
  - `cargo nextest run -p ralph-cli --test integration_wave_protocol_closure`：7/7 通过；
  - `cargo nextest run -p ralph-core --lib supervisor::`：231/231 通过；
  - `git diff --no-index -- CLAUDE.md AGENTS.md || true`：无差异；
  - 源码、调用链、测试、配置、历史检索。
- 尚未执行：无（2026-08-05 全部补执行完成，结果见执行状态段）。
- 阻塞项：D2（runtime authority 的真实签发边界）低于 0.85。未完成前不得进入 authority implementation。

# 1. 功能目标

## 业务目标

让 agent 通过现有 `ralph tools task verify add|ensure` 保护的 Task mutation，在 Apply 成功后得到可查询、可恢复的 runtime confirmation；未确认或确认不可读时，下一次 agent-owned protected mutation 必须停止。现有 ticket claim/restore 语义和 human CLI 行为必须保持。

## 用户与调用方

- agent：使用 Task verify、Task mutation 和 JSON 查询结果；
- `ralph-cli`：执行 Task command、读取 `TaskStore`、调用 `task_verify_gate`；
- loop runtime：提供 agent context（当前实现由环境/marker 解析）；
- human operator：无 agent context 时继续使用现有 Task CLI，不被 confirmation gate 锁死。

## 当前行为（已确认）

- `task verify add|ensure` 记录带 fingerprint、loop、hat、timestamp 的 ticket；
- `add`/`ensure` 在 mutation 前调用 `verify_gate_claim`，成功后由 `settle_gate_claim` consume，失败 restore；
- Task JSONL 的坏行由 `TaskStore::load` 跳过；Task 当前状态只有 `open/in_progress/closed/failed`；
- 没有 Task `confirm` 子命令、Task confirmation 字段或“上次 Apply 未确认则阻止下一次 Apply”的已确认调用链；
- Wave 已有 `EmissionState::{Reserved,Applying,Applied,RecoveryRequired,Failed}`、SupervisorStore transition 和 `wave inspect` 的 public view；Wave Apply→Confirm 回归测试已存在。

## 目标行为

仅在 Unit 1 完成并验证决策后，Task agent-owned `add`/`ensure` 的成功 Apply 产生一个 additive confirmation record；Task 查询以机器可读方式返回其状态；正确的 Confirm 转换为 confirmed；pending/unavailable/mismatch 在下一次同 scope protected Task mutation 前 fail-closed，且拒绝不写 Task JSONL、不产生第二条 Task。

## 行为差异与边界

- 输入：现有 add/ensure 参数、现有 canonical payload/fingerprint、当前 loop/hat、confirmation reference；
- 输出：现有输出保持兼容，新增字段只 additive；deny 使用现有 `task_verify_gate denied` 前缀并增加稳定 reason；
- 状态：Task 现有业务状态不改变；confirmation 是独立的可选 operation 状态，至少区分 `pending`、`confirmed`、`unavailable`、`mismatch`；
- 错误：拒绝发生在 TaskStore mutation 前；Apply 后无法安全持久化 confirmation 时不得静默成功，必须返回可恢复状态；
- 兼容：旧 Task JSONL 缺少新字段仍可读取；human CLI、`tasks.enabled`/verify gate 关闭、`allow_unsafe_task_mutate` 既有分支保持现状；
- 性能：复用现有 TaskStore lock/save，不扫描整个 workspace，不引入每个 event 的新全局解析；
- 安全：本计划不声称解决同 UID 可注入 runtime 的 OS-level attacker；workspace ticket、环境变量、公开 hash 不是 authority；
- 非目标：不改 Wave 已有 emission state machine；不改 preset 拓扑；不改 Task 业务状态机；不引入外部密钥服务、跨机器信任或 privileged broker。

## 已确认/待验证假设

- 已确认 A1：Task mutation 的受保护入口目前只有 `execute_add` 与 `execute_ensure` 的 `verify_gate_claim` 调用。
- 已确认 A2：对 `verify_gate_claim` 的全仓搜索只找到 `execute_add` 与 `execute_ensure` 两个 Task protected caller；`start/close/fail/reopen` 未接入该 gate，因此本计划不扩展到它们。若 Unit 1 实现时出现新 caller，必须暂停并扩展需求矩阵。
- 待验证 A3：Task receipt 放在同一 JSONL 是否能覆盖 Apply 与 receipt 的 crash window；验证动作是现有 `TaskStore::save` 的故障注入/characterization。失败则 Unit 1 阻塞，不改为 sidecar 猜测。
- 待验证 A4：spawn-owned authority 是否能在目标平台形成 runtime-owned、不可由 workspace writer 伪造的真实边界；当前无直接代码证据，属于 BLOCKED。

# 2. 代码库现状与证据

## 2.1 当前实现入口

Task 外部入口是 `crates/ralph-cli/src/task_cli.rs::execute` 的 `TaskCommands::Add/Ensure/List/Show/Verify` 分派；`execute_add`/`execute_ensure` 先生成 canonical payload，再调用 `verify_gate_claim`，执行 `TaskStore` mutation，最后调用 `settle_gate_claim`。ticket 生命周期在 `crates/ralph-cli/src/task_verify_gate.rs::{record_ticket,try_claim_matching_ticket,consume_claimed_ticket,restore_ticket_from_claim}`。持久化边界是 `crates/ralph-core/src/task_store.rs::{load,save,with_exclusive_lock}` 及 JSONL `Task` 序列化。

Wave 外部入口是 `crates/ralph-cli/src/wave.rs` 的 emit/inspect；状态边界是 `crates/ralph-core/src/supervisor/mod.rs::SupervisorStore` 的 emission reservation API，SQLite 实现位于 `supervisor/rusqlite.rs`，memory 实现位于 `supervisor/memory.rs`。已有 `integration_wave_protocol_closure.rs` 真实验证 Apply→Confirm、双进程 dedup、cleanup failure 和 human path。

Agent context 由 `crates/ralph-cli/src/operation_guard.rs::OperationContext::detect` 从 `.ralph/current-loop-id` 与 `RALPH_CURRENT_HAT/RALPH_CURRENT_LOOP_ID/RALPH_EVENTS_FILE/RALPH_WAVE_WORKER` 得出；现有 loop spawn 注入由 `crates/ralph-cli/src/loop_runner/execution.rs`、`runner.rs` 和 `wave/worktree_bind` 相关代码负责。调查未发现 runtime-owned ticket issuer 或进程间 authority protocol。

测试入口是 Rust unit/integration + core scenario；仓库硬规则要求 `cargo nextest run` 系列，最终全量入口是 `./scripts/run-tests.sh`，CLI 文档漂移入口是 `bash scripts/check-cli-doc-drift.sh --strict`，lint 命令是 `cargo clippy --all-targets --all-features -- -D warnings`。

## 2.2 Evidence Ledger

| ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `task_cli.rs::execute_add/execute_ensure`、`rg verify_gate_claim` | 当前 protected Task mutation 在 add/ensure；两处均有 claim→mutation→settle | Unit 1 先覆盖这两个真实入口，禁止凭空扩大 mutation 范围 | 高 |
| E2 | `task_verify_gate.rs` | ticket 是 workspace 文件；匹配后 rename 为 claim，Apply 成功 consume，失败 restore；human/gate-off/unsafe 分支 bypass | 保留现有 ticket 行为，confirmation 必须接在真实 settle 边界 | 高 |
| E3 | `task.rs::TaskStatus/Task` | 只有四种业务状态，Task 无 confirmation 字段 | confirmation 不能伪装成 TaskStatus；必须 additive 且兼容旧 JSONL | 高 |
| E4 | `task_store.rs::load/save` | JSONL load 跳过坏行；save 使用同目录 atomic temp+sync+rename 与 file lock | 先做 characterization；receipt 原子边界必须复用并验证，不能直接声称跨副作用事务 | 高 |
| E5 | `integration_tasks.rs`，nextest 16/16 | 现有 ticket 并发、mismatch、restore、scope isolation、human env scrub 已通过 | 新测试必须保护旧行为，不得替换或削弱现有断言 | 高 |
| E6 | `supervisor/mod.rs::EmissionState/SupervisorStore`、`wave.rs::WaveInspectView` | Wave emission 已有 applied/recovery_required/failed 与 inspect 映射 | Wave 不应被规划为从零实现；只做 characterization/regression | 高 |
| E7 | `integration_wave_protocol_closure.rs`，nextest 7/7 | 已验证 Apply→Confirm、双进程 dedup、失败清理和 human path | 原计划 R4 的“新增 Wave Confirm”与代码冲突，必须删除该实现 Unit | 高 |
| E8 | `operation_guard.rs::compute_is_agent_context` | agent identity 依赖 env/marker；human/agent 行为由此分支 | env/marker 不能写成 runtime authority；authority 范围必须单独阻塞 | 高 |
| E9 | `loop_runner/execution.rs`、`runner.rs`、`wave/worktree_bind` 搜索 | 找到 spawn/env 注入路径，未找到 issuer/IPC/descriptor authority | D2 不能进入正式 implementation Unit | 中高 |
| E10 | Git history：`04b72563`、`7af467ae`、`88a2f506`、`990392a5` | Task ticket claim/restore 与 Wave protocol closure 已由相邻计划实现 | 本计划应增量补缺，不重复历史已交付能力 | 高 |
| E11 | `AGENTS.md` 与 `CLAUDE.md` diff | 两份 instructions 当前一致 | 文档修改若发生必须继续同步 | 高 |
| E12 | `scripts/run-tests.sh`、`justfile`、`scripts/check-cli-doc-drift.sh` | 测试、lint、doc drift 的真实命令已确认 | 计划不得写裸 `cargo test -p ralph-cli` 或不存在命令 | 高 |

## 2.3 受影响范围

已确认生产范围：`crates/ralph-core/src/task.rs`、`task_store.rs`；`crates/ralph-cli/src/task_cli.rs`、`task_verify_gate.rs`、`operation_guard.rs`；Wave 相关 `wave.rs`、`wave_verify_gate.rs`、`supervisor/{mod.rs,memory.rs,rusqlite.rs}` 仅作为回归边界；loop spawn 文件仅作为 D2 调查入口。

已确认测试范围：`crates/ralph-cli/tests/integration_tasks.rs`、`integration_wave_protocol_closure.rs`、`crates/ralph-core/src/supervisor/*tests*`、`crates/ralph-core/tests/scenarios.rs`。已确认文档范围：`crates/ralph-core/data/ralph-tools-opac.md`、`ralph-tools-tasks.md`、`ralph-tools-wave.md`、`ralph-tools-cmdref.md`、`docs/guide/opac.md`、`AGENTS.md`、`CLAUDE.md`。未确认的新增文件不得写入 Unit 文件清单。

# 3. 决策记录与置信度

| ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | Task confirmation 放在哪里？ | Task JSONL additive；独立 sidecar；Supervisor DB | 只在 Unit 1 characterization 证明 crash boundary 后，优先扩展 TaskStore 同一 JSONL | E3、E4；Task 与 Task mutation 同一 store | sidecar/DB 无现有 Task caller 证据，新增双写事务边界 | 0.86 |
| D2 | ticket authority 如何由 runtime 颁发？ | env/file/hash；inherited descriptor/IPC；外部 broker | **BLOCKED，不作选择** | E8、E9 只证明现有 env/marker，没有 issuer | 缺少平台矩阵、父子进程协议、重启/同 UID threat evidence | 0.42 |
| D3 | confirmation gate 应覆盖哪些 mutation？ | 仅 add/ensure；所有 TaskCommands mutation；所有 event side effect | 仅覆盖现有 `verify_gate_claim` 的 add/ensure；不扩展 start/close/fail/reopen | E1、E5、全仓 `rg verify_gate_claim` | 其他命令没有当前 OPAC claim 调用证据；扩大范围会改变未确认行为 | 0.90 |
| D4 | Wave 是否新增 Confirm store/state？ | 新 sidecar；扩展现有 emission store；不改，仅回归 | 不新增；复用现有 state 并写 characterization/regression | E6、E7、E10 | 新 store 会重复已存在的 authority，且破坏既有 public view | 0.97 |
| D5 | human/gate-off 如何处理 pending confirmation？ | 统一拒绝；沿用 bypass；新增交互确认 | 沿用现有 human/gate-off/unsafe bypass，agent path 才 gate | E2、E5、E8 | 统一拒绝会违反已测试兼容行为 | 0.93 |
| D6 | Confirm 命令挂在哪里？ | Verify 子树新变体；TaskCommands 顶层变体；复用现有 close 骨架 | `TaskCommands::Confirm` 顶层变体（`ralph tools task confirm <id> --reference --digest`） | task_cli.rs 扁平动词族先例（Add/Ensure/…/VerifyEmitBridge）；Verify 子树契约是只读 dry-run | 挂 Verify 下破坏其只读契约 | 0.92 |
| D7 | confirmation receipt 存在哪里？ | 独立 sidecar；supervisor DB；Task 行内 additive 字段 | Task 行内 `Option<TaskConfirmation>`（随同一 save snapshot 落盘） | task_store save 是整 Vec 单 rename 原子快照（A3 characterization）；独立行类型会被旧 binary warn-skip 后永久丢失 | sidecar/DB 引入第二个持久化权威 | 0.90 |
| D8 | pending gate 插在 mutation 流程哪个位置？ | claim 之后 mutation 之前；claim 之前 bail；settle 之后 | `enforce_command_policy` 之后、claim 之前 bail | claim 有盘上副作用（.claimed rename）；claim 前拒绝保住 prepared ticket 供 Confirm 后重试 | claim 后拒绝需新增 restore 分支，现有机制无此路径 | 0.92 |

## 低置信度决策处理

D2 不能进入实施。进入任何 authority Unit 前必须：扫描真实 spawn caller；确认目标平台（当前可执行环境为 macOS，仓库 CI 还需从 workflow 确认 Linux）；做最小父子 subprocess 实验；验证 workspace writer、复制 env/file/hash、父进程重启、descriptor 丢失、同 UID 边界；比较 inherited descriptor、Unix socket、外部 broker 三方案；形成协议/平台/错误/恢复 Decision，置信度达到 0.85。任一步失败，保持 agent path fail-closed，并把 authority 作为后续独立计划，不得由 Executor 临时降级。

D3 已达到执行阈值；Unit 1 只需在实现前复核 diff 未新增其他 protected caller。若出现新 caller，当前计划立即 BLOCKED，更新追踪矩阵和 Unit 1，不得偷偷扩大范围。

# 4. BDD 行为规格

## Feature: Task Apply 后的 runtime confirmation

  Background:
    Given agent context 已由真实 runtime env 建立
    And Task verify gate 已启用
    And `ralph tools task verify add` 或 `ensure` 已记录匹配 ticket

  Scenario: 成功 Apply 后可查询 pending confirmation
    Given TaskStore 可读且 ticket fingerprint 与当前 payload 匹配
    When agent 执行受保护的 Task add 或 ensure
    Then mutation 成功且查询结果包含唯一 confirmation reference
    And confirmation state 为 `pending`
    And Task JSONL 只有一条对应业务记录

  Scenario: 正确 Confirm 后下一次同 scope mutation 放行
    Given 上一次 Apply 的 confirmation state 为 `pending`
    When agent 用同一 reference、digest、loop、hat 执行 Confirm
    Then state 变为 `confirmed`
    And 下一次同 scope protected mutation 执行成功

  Scenario: 未 Confirm 时下一次 mutation 被拒绝
    Given confirmation state 为 `pending`
    When agent 未执行 Confirm 又执行 protected mutation
    Then 命令退出非零并返回稳定 `confirmation_required`
    And TaskStore、事件和新业务 Task 均无变化

  Scenario: digest 或 scope 漂移时 Confirm 被拒绝
    Given confirmation state 为 `pending`
    When Confirm 使用不同 digest、loop、hat 或 reference
    Then 返回 `confirmation_unavailable` 或稳定 mismatch reason
    And state 仍为 `pending`

  Scenario: 旧 Task JSONL 可读取
    Given JSONL 行没有 confirmation 字段
    When agent 或 human 执行 list/show
    Then 命令成功并保留旧业务字段
    And 不把缺失字段解析成已确认

## Feature: 已存在的 Wave confirmation 兼容

  Scenario: Wave Apply→Confirm 既有闭环保持通过
    Given SupervisorStore 有 emission reservation
    When真实 CLI 执行 wave Apply、inspect、Confirm
    Then 现有 `applied`、`recovery_required`、`failed` 和 public view 语义不改变

## Feature: runtime authority（阻塞）

  Scenario: 无 runtime-owned authority 的 workspace writer 不得签发 ticket
    Given 当前代码尚无已确认 issuer 协议
    When调查实验尝试复制 env、ticket、公开 digest
    Then 在 D2 决策完成前只能记录 BLOCKED，不得把实验结果伪装成实现验收

# 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口/层级 | 风险补充 | E2E |
|---|---|---|---|---|
| S1 | 真实 `ralph tools task add/ensure` 后 JSON 有唯一 reference、pending，Task 仅一条 | `integration_tasks.rs` subprocess integration | Characterization：先固定旧 ticket 生命周期 | 否 |
| S2 | 正确 reference/digest/scope Confirm 后下一次同 scope mutation 成功 | `integration_tasks.rs`；若 CLI surface 未确认，先在 Unit 1 调查 | Idempotency：重复 Confirm 结果不新增记录 | 否 |
| S3 | pending 时下一次 mutation 非零、reason 稳定、无 JSONL/event 副作用 | `integration_tasks.rs` | State-machine：pending→deny 不消费 ticket | 否 |
| S4 | mismatch 不改变 pending；旧 JSONL 缺字段仍 list/show 成功 | `task_cli.rs`/`task_verify_gate.rs` unit + integration | backward compatibility characterization | 否 |
| S5 | Wave 既有 Apply→Confirm、recovery、dedup 仍通过 | `integration_wave_protocol_closure.rs` + supervisor tests | regression，不新增 duplicate store | 否 |
| S6 | authority 方案实验能区分持有 runtime channel 与仅有 workspace/env 的进程 | D2 独立 subprocess spike；当前不计为通过 | fault/restart/concurrency；未完成前 BLOCKED | 否 |

每项测试必须断言 exit code、结构化 stdout/stderr、持久化副作用和不变量；不得只断言文案。CLI 文档变化只有在真实命令 surface 已确定后才加入 `check-cli-doc-drift.sh --strict`。

# 6. 需求—测试追踪矩阵

| Requirement | 需求 | Scenario | 验收测试 | 单元 | 集成/契约 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1 | Apply 后有可查询 confirmation | S1 | 新增 integration test（入口在 Unit 1 前确认） | receipt round-trip | `integration_tasks` | 否 | E1,E3,E4 |
| R2 | digest/loop/hat/reference 绑定 | S2,S4 | mismatch/duplicate tests | typed transition tests | `integration_tasks` | 否 | E2,E8 |
| R3 | 未确认下一次 mutation fail-closed | S3 | no-side-effect integration test | gate decision tests | `integration_tasks` | 否 | E1,E2 |
| R4 | Wave 已有 confirmation 不能回归 | S5 | 现有 7 tests | supervisor state tests | closure integration | 否 | E6,E7 |
| R5 | 旧 JSONL/human/gate-off 兼容 | S4 | existing + additive tests | serde tests | task integration | 否 | E3,E5 |
| R6 | runtime authority 防伪造 | S6 | BLOCKED spike，未通过前无验收绿灯 | 不创建伪造单测 | subprocess contract待决 | 否 | E8,E9 |

# 7. 严格串行开发单元

> 只有 Unit 1 达到完成标准后才能进入后续 Unit。D2 未达 0.85 前不存在 authority implementation Unit。

## Unit 1：Task confirmation 的最小纵向切片

> **✅ 已完成（2026-08-05）**：实现 commit `d4b74ff5`，审核修复循环 `492a3756`/`4ed44310`；Acceptance Red 有效（surface 缺失），S1–S4 全绿，完成标准逐项通过。

### 1. Unit 目标

完成一个外部行为：agent 的 protected Task add/ensure 成功后可查询 pending confirmation，正确 Confirm 后同 scope 下一次 mutation 放行，未 Confirm 时无副作用拒绝。

### 2. 对应需求与 Scenario

R1/R2/R3/R5；S1/S2/S3/S4；D1、D3、D5；E1–E5、E8。

### 3. 外部可观察结果

`ralph tools task ... --format json` 的真实输出增加 additive confirmation view；Confirm surface 只能在现有 CLI parser 调查确认后确定。pending、confirmed、deny 的 exit code、reason、Task JSONL 数量和重复请求结果可观察且可断言。

### 4. 当前行为基线

当前 add/ensure 只有 verify ticket claim/settle，没有 confirmation；当前业务 Task 状态无 confirmation 字段（E1–E4）。旧 ticket 并发、mismatch、restore、human bypass 已由 `integration_tasks` 16/16 固定（E5）。若 Acceptance Red 不是“confirmation surface/状态缺失”，立即停止并更新证据。

### 5. 输入与输出

- 输入：现有 add/ensure args、canonical payload、ticket fingerprint、loop/hat、confirmation reference/digest；
- 输出：pending/confirmed 结构化状态，或 `confirmation_required`/mismatch/unavailable；
- 状态：业务 Task 状态保持原语义，confirmation 单独前进，不允许 confirmed 回退 pending；
- 副作用：成功仅一条业务记录和一条对应 confirmation；拒绝不改 store、event 或 ticket；
- 不变量：旧 JSONL 可读；重复 Confirm 幂等；错误 Confirm 不消费 pending；human/gate-off/unsafe bypass 不变。

### 6. 修改位置

- `crates/ralph-core/src/task.rs`：当前 Task/TaskStatus serde 模型；只允许增加已由 red 锁定的 additive confirmation 类型，不改业务状态枚举。
- `crates/ralph-core/src/task_store.rs`：当前 JSONL load/save/lock；只扩展与同一 snapshot 可证明一致的 receipt 读写；不引入第二个持久化权威。
- `crates/ralph-cli/src/task_cli.rs`：当前 add/ensure/list/show/verify 分派和真实 mutation；只接入 S1–S4 需要的入口，不改 start/close/fail/reopen，除非 D3 调查找到其真实 protected caller。
- `crates/ralph-cli/src/task_verify_gate.rs`：当前 ticket claim/consume/restore；只增加 pending-confirm gate 的调用边界，保留现有 mismatch 不消费和失败 restore。
- `crates/ralph-cli/tests/integration_tasks.rs`：现有真实 subprocess 入口；新增 S1–S4 测试，禁止替换既有 16 项断言。

### 7. 可依赖能力

现有 `common::ralph_bin()`、`scrub_agent_runtime_env`、TaskStore file lock/atomic save、Task ticket scoped path、claim/restore/consume，以及现有 integration fixtures。

### 8. 禁止依赖的未来能力

不得实现 runtime-owned IPC、secret、descriptor、peer credential；不得修改 Wave store；不得把 environment/file/hash 伪装成 authority；不得扩展未经 D3 确认的 Task mutation。

### 9. 验收测试

先调查真实 Confirm CLI/parser 入口；若不存在，Unit 立即 BLOCKED，不让 Executor发明命令名。确认入口后，S1–S4 均通过 `common::ralph_bin()` subprocess，断言 JSON 字段、exit code、TaskStore 行数、重复 Confirm 和拒绝无副作用。命令：`cargo nextest run -p ralph-cli --test integration_tasks`。

### 10. Acceptance Red

首先写/启用 S1，运行上述 nextest。有效 Red 必须是当前 CLI 没有 confirmation 字段/transition，或测试无法在 pending 状态观察到目标结果；该失败直接证明真实入口缺少目标能力。编译错误、fixture 错误、命令拼写错误、未执行 subprocess、既有无关测试失败均不是有效 Red，必须停止修正测试/调查。

### 11. 单元测试拆分

1. receipt serde：旧 JSON 缺字段读取为 legacy/未确认，不得为 confirmed；Fake 仅用于纯 serde，不 Mock TaskStore。
2. transition：pending→confirmed、重复 confirmed 幂等、mismatch 保持 pending；使用内存值对象，不 Mock transition 规则。
3. gate：pending/unavailable 在 mutation 前 deny，human/gate-off/unsafe bypass 保持旧结果；使用真实 `OperationContext` 构造，不绕过 gate。
4. atomic boundary：receipt 与业务 Task 同一保存快照；注入 save error 时断言明确 unavailable/applied-unconfirmed，不能只断言 panic。

### 12. Red → Green → Refactor 顺序

S1 Acceptance Red → 锁定缺失 surface → 最小 receipt read/write Green；
receipt round-trip Red → 最小 additive serde/transition Green；
mismatch/idempotency Red → 最小 gate transition Green；
pending no-side-effect Red → 在 `execute_add/ensure` 真实 mutation 前接 gate Green；
旧 JSONL/human/失败 restore 回归 → 只做必要 refactor → 再跑全部 S1–S4。

### 13. 最小实现范围

必须实现 pending/confirmed/unavailable 的可观察状态、同 scope gate、旧 JSONL additive 读取、重复 Confirm 幂等和失败无副作用。必须保持 TaskStatus、ticket claim/restore、human bypass。明确不实现 Wave、authority、其他 TaskCommands 和新依赖。

### 14. 集成验证

必须真实联合 `ralph-cli` Task parser、`OperationContext`、`task_verify_gate`、`TaskStore` 和 subprocess；纯 transition 可用内存值对象。不得 Mock 真正的 JSONL save、claim、gate 或 CLI path。运行 `cargo nextest run -p ralph-cli --test integration_tasks`，全 16 个旧测试及新增 S1–S4 通过。

### 15. 风险驱动测试

必须做 Characterization（E5 已证明 ticket 旧行为）；Idempotency（重复 Confirm）；State-machine（pending/confirmed/mismatch）；Fault Injection（save/receipt 不可用）。不做无证据的 fuzz、mutation、E2E；只有 D2 解锁后才增加跨进程 authority contract。

### 16. 回归范围

直接：`integration_tasks`、`task_cli.rs`/`task_verify_gate.rs` unit；相邻：`integration_emit_policy`（agent context/OPAC）、`integration_wave_protocol_closure`（确认不影响现有 Wave）；旧 JSONL、human CLI、gate-off、unsafe path、build、clippy、doc drift。原因是 Unit 修改共享 Task/OperationContext/gate 边界（E1–E8）。最终再跑 `./scripts/run-tests.sh`。

### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/task.rs` | 修改现有生产文件（仅若 red 证明需要） | additive confirmation 类型 | E3 |
| `crates/ralph-core/src/task_store.rs` | 修改现有生产文件（仅若 D1 验证通过） | 同一 snapshot 持久化 | E4 |
| `crates/ralph-cli/src/task_cli.rs` | 修改现有生产文件 | add/ensure/query/confirm 真实入口 | E1 |
| `crates/ralph-cli/src/task_verify_gate.rs` | 修改现有生产文件 | pending gate 接入 | E2 |
| `crates/ralph-cli/tests/integration_tasks.rs` | 新增测试 | S1–S4 真实验收 | E5 |

### 18. 完成标准

S1–S4、Unit tests、integration_tasks、相邻回归、build、clippy、typecheck/doc drift 通过；无 skip/.only、无削弱断言、旧行为仍绿；D1/D3/D5 不低于 0.85；证据台账更新；Unit 可独立提交。若 Confirm parser 仍未确认，则不能关闭 Unit。

### 19. 停止条件

发现实际 protected caller 超出 add/ensure、TaskStore 不能形成明确 crash 语义、Red 非目标失败、需新依赖、旧 JSONL 不兼容、D1/D3 低于 0.85 或必须依赖 D2 时停止；记录 E/D，重算影响范围，不继续猜测。

### 20. 风险与注意事项

- JSONL snapshot 与外部业务副作用存在 crash window；触发于 save 注入失败，必须以 unavailable/applied-unconfirmed 阻断并测试，剩余风险需写入结果。
- 当前 confirmation CLI 入口尚未确认；触发于 `task_cli.rs` parser 无对应命令，Unit 阻塞，禁止自行命名。
- agent identity 当前来自 env/marker；触发于把它作为 authority，审查拒绝并转 D2。
- 用户工作区已有 merge-batch 修改；触发于 diff 涉及其文件，停止并报告，不覆盖。

## Unit 2 及以后：未解锁

Wave 仅执行 S5 characterization/regression，不创建重复实现 Unit。runtime authority 只有在 D2 达到 0.85 后，才能产生一个新的、独立的 Unit；其入口、协议、平台支持、错误语义、重启和并发测试必须由新的 Decision Record 明确。当前计划不把未确认文件或接口列为事实。

# 8. Unit 串行依赖图

`Unit 1 Task confirmation` → `Unit 2 authority（D2 解锁后才可创建）`。

Unit 2 必须使用 Unit 1 已验证的 confirmation reference/digest/gate seam；不能交换顺序，因为 authority 需要绑定已确定的 operation identity。D2 未解锁时不得创建或实现 Unit 2，避免提前实现未来行为。

# 9. 执行命令清单

- `cargo nextest run -p ralph-cli --test integration_tasks`：Unit 1 Acceptance Red/Green/回归；失败不得进入下一步。
- `cargo nextest run -p ralph-cli --test integration_wave_protocol_closure`：确认 Wave 既有闭环未回归；失败不得关闭 Unit 1。
- `cargo nextest run -p ralph-core --lib supervisor::`：Supervisor state 相邻回归；失败不得关闭 Unit 1。
- `cargo nextest run -p ralph-cli --test integration_emit_policy`：agent OPAC 相邻回归；仅在 Unit 1 真实调用链触及 emit context 时运行，触及则必跑。
- `cargo build`：生产编译；Unit close 前必须通过。
- `cargo clippy --all-targets --all-features -- -D warnings`：lint/typecheck；Unit close 前必须通过。
- `bash scripts/check-cli-doc-drift.sh --strict`：新增 CLI surface 后运行；若没有 surface 变化，记录“未触发”。
- `./scripts/run-tests.sh`：最终全量 nextest + doctest；不得以裸 `cargo test -p ralph-cli` 替代。
- `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh`：仅全量出现竞态 flake 时使用，不是默认路径。

# 10. 最终质量门禁

所有已解锁 Scenario 必须有真实测试且通过；Wave characterization 仍通过；旧 JSONL、human/gate-off、idempotency、fault injection 通过；无 skip/.only/断言削弱；build、clippy、doc drift、最终全量 nextest 通过；无 BLOCKED 决策被偷偷实现；所有进入执行的 Decision ≥0.85；D2 若仍未达标，最终状态必须保留 PARTIALLY READY/BLOCKED，不能声称 authority 完成。

# 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap | 是 | Unit 1 锁定入口、Red、测试、边界和停止条件 |
| Executor 是否仍需做关键设计决策 | 否 | D2 已明确阻塞，不进入 Executor 执行范围 |
| 所有文件和接口是否有代码库证据 | 是 | E1–E12；未确认位置未列为事实 |
| 所有关键决策是否 ≥0.85 | 否，故状态不是 READY | D2=0.42、D3=0.78，均有补证动作 |
| 是否存在未处理低置信度假设 | 否 | A2–A4 均有验证方法和阻塞影响 |
| 每个 Unit 是否只有一个可观察行为 | 是 | 当前仅一个已解锁 Unit |
| 每个 Unit 是否可以独立验证 | 是 | Unit 1 使用真实 integration_tasks |
| 每个 Unit 是否有真实 Red | 是（执行时必须验证） | S1 明确目标缺失 Red；非目标失败停止 |
| 每个 Unit 是否包含回归范围 | 是 | Unit 1 §16 |
| 是否存在未来 Unit 依赖 | 否（未解锁 Unit 不进入执行） | D2 解锁前不创建 Unit 2 |
| 是否存在泛化任务描述 | 否 | 修改位置、调用链、断言和命令已具体化 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §5、§6、Unit 1 |
| 所有关键决策是否有 Evidence | 是 | D1–D5 均引用 E；D2 明确证据不足 |
| 计划是否可以严格串行执行 | 是（PARTIALLY READY 范围） | Unit 1 完成后才可申请 D2/Unit 2 |
