---
artifact_contract: ce-unified-plan/v1
artifact_readiness: partially-ready
product_contract_source: ce-plan-bootstrap
execution: code
title: "fix: 收敛 OPAC 高置信度 gate 缺陷"
date: 2026-08-03
type: fix
baseline: 30d32132
---

# fix: 收敛 OPAC 高置信度 gate 缺陷

## Goal Capsule

目标、范围、阻塞状态和最终交接见本计划第 0、1、10、11 节；本文件是实现计划，不是执行结果报告。当前仅 R1–R3 达到可执行阈值，R4（Confirm runtime）和 R6（ticket authority）保持 BLOCKED；R5 依赖 R4 的最终公开行为，也暂不启动。

## Product Contract

第 1 节定义业务目标、兼容边界和非目标；第 4 节定义可观察行为；第 6 节将需求、Scenario、Evidence 和 Unit 逐项追踪。该契约要求现有 OPAC 命令形状和 human CLI 行为保持可用。R4 的“Confirm 后置阻断”暂不作为可执行承诺，直到公开 evidence surface 被代码证据锁定。

## Planning Contract

第 2 节记录真实代码证据，第 3 节记录技术决策与置信度；第 7、8 节锁定实现位置、串行依赖和停止条件。任何未在 Evidence/Decision 中达到 0.85 的关键选择都不得交给 Executor 临场决定。

## High-Level Technical Design

```text
verify
  → prepared ticket
  → 原子匹配并 claim
  → Apply
      ├─ 失败：restore
      └─ 成功：applied_pending_confirm
                         → public Confirm evidence
                         → confirmed
                         → 下一次 protected mutation 放行
```

U5 contract compile failure 在 agent governed emit path 中于 ticket、idempotency 和 event side effect 之前进入显式 deny；human/stand-down 分支不扩张。R6 的认证 capability 不进入该图，直到可信签发边界被代码证据确认。

## Implementation Units

第 7 节是严格串行的 U1–U3 可执行单元；U4、U5 和 R6 都是有明确解锁条件的阻塞决策门，不得提前实现。

## Verification Contract

第 5、6、9 节定义 Scenario 到测试、命令和回归门禁的追踪。测试必须使用仓库规定的 nextest 系列；最终全量验证只能使用 `./scripts/run-tests.sh`。

## Definition of Done

第 10 节定义 R1–R3 的当前完成门禁及 R4/R6 的解锁门禁；第 11 节给出自检结果。计划不会因测试尚未执行而声称实现完成，也不会因 R4/R6 未决而声称已解决 Confirm 或 ticket 防伪造。

## 0. 计划状态

**PARTIALLY READY**。

已达到执行阈值的范围：

- task ticket 的并发 TOCTOU；
- ticket 参数不匹配时误消费；
- task ticket 的单 workspace 单槽位覆盖；
- emit U5 execution contract 编译失败时 fail-open；
- task ticket scope/key 算法锁定；
- U5 contract compile failure fail-closed。

当前阻塞范围：

- Confirm 从 agent 提示升级为 runtime 可验证的 Apply 后闭环：当前没有 task 的公开 Confirm adapter、唯一持久化边界和事务状态迁移证据。
- “workspace 内同权限、恶意或被 prompt injection 的 agent 不能伪造 ticket”需要 runtime-held secret、受控 IPC 或等价的独立权威。当前仓库没有已确认的 secret/IPC 传递链。普通文件 ticket 和公开 SHA-256 不能提供该保证。

因此本计划不把 ticket MAC/密钥传递写成可直接执行的实现 Unit，也不声称已经解决恶意同权限 agent 的伪造问题。Coding Agent 不得在 Unit 1–3 中临时发明密钥传递方式；不得启动 U4/U5/R6，除非先完成对应解锁条件并新增达到 0.85 的 Decision Record。

基线与调查范围：

- 基线：`pittcat-dev`，HEAD `30d32132`。
- 调查范围：`task_verify_gate`、`wave_verify_gate`、`commands::emit::U5Gate`、`operation_guard`、`wave inspect`、task CLI、OPAC 注入文档、现有 task/wave/BDD/nextest 测试、相关历史计划与 `docs/solutions/`。
- 已执行的只读验证：源码/符号检索、调用链读取、现有测试定位、相关计划与 solution 文档检索、`git rev-parse --show-toplevel`、计划目录与 frontmatter 检查；追加核验 `task_cli.rs` 的 list/show/close、`TaskStore`、`wave inspect`/`wave_emissions`、`loop_runner/execution.rs`、wave worker env、`rpc_stdin.rs`、`control_plane.rs` 与最近相关 git 历史。
- 未执行的验证：未运行测试、构建、lint 或 CLI smoke；本计划阶段禁止把执行结果预先写成事实。
- 阻塞项：R6 已确认当前不存在可信 ticket authority；若要继续，必须新增并审定私有 issuer 协议，或明确把同权限 workspace 写入者排除在安全边界之外。R4 已确认当前不存在 task Confirm authority，必须先形成 D5a–D5c。

## 1. 功能目标

### 业务目标

让现有 OPAC 的 Precheck→Apply 保护在并发、参数漂移、失败恢复和 contract 编译异常下保持 fail-closed，同时让 Apply 后的 Confirm 有 runtime 可验证的公共状态。修复必须保留现有 agent 命令形状和 human CLI 行为。

### 用户与调用方

- isolated hat agent：调用 `ralph tools task verify`、`ralph tools task`、`ralph wave verify`、`ralph wave emit` 与公开 Confirm 查询。
- loop runtime：提供 agent context、loop/hat scope、execution contract 和后续状态推进。
- human operator：继续使用 human CLI bypass，不被 agent ticket 或 Confirm gate 锁死。
- Coding Agent：按本计划 Unit 顺序实现和验证，不重新选择 ticket 格式、并发协议或 Confirm 语义。

### 当前行为

- task gate 使用固定 `.ralph/agent/.ralph-task-verify-ticket`；消费路径先读后删，再检查 fingerprint、loop、hat。
- task ticket 用 `std::fs::write` 覆盖固定文件；多个 pending intent 会相互覆盖。
- wave 已有 claim marker，但 verify ticket 仍是单文件写入，且 ticket 读取与 claim 不是一个统一原子协议。
- `U5Gate::resolve` 对 `execution_contract::compile(...).ok()` 使用 `Option`；编译失败后仍可能保持 active，但 capability 校验跳过。
- OPAC 文档要求 Apply 后 Confirm，但 runtime 没有统一的“未 Confirm 不得继续下一次受保护 mutation”的状态转移。
- ticket 内容与 SHA-256 fingerprint 是 workspace 可见的普通文件数据；同权限恶意 agent 的不可伪造性未得到代码证据支持。

### 目标行为

- ticket 先完整匹配，再以跨进程原子方式 claim；只有成功 Apply 才 consume，Apply 前失败可 restore，拒绝和参数漂移不消费。
- task ticket 按 operation scope 隔离，不再让 `add`、`ensure` 或不同 activation 互相覆盖；现有命令参数不变。
- agent context 下 U5 contract 编译失败直接拒绝，返回稳定结构化 reason，不落事件、不创建 Apply 副作用；human CLI 与既有 stand-down 分支保持不变。
- 每个受保护 Apply 产生可查询的 confirmation record；Confirm 证据缺失、不一致或 unavailable 时，后续受保护 mutation 停止。
- R6 只有在可信 runtime authority 被证据确认后，才允许把 ticket 改造成认证 capability；在此之前不得声称完成防伪造保证。

### 行为差异

新增的是 agent-context fail-closed 约束和可恢复状态，不改变以下行为：

- `ralph tools task verify <verb>` 与对应 Apply 的命令参数；
- `ralph wave verify` 与 `ralph wave emit` 的命令参数；
- human CLI 在无 agent context 下的 bypass；
- `tasks.require_verify_for_cli_mutate: false` 的默认路径；
- 已有 `allow_unsafe_task_mutate` 的 task recovery bypass；
- `wave inspect` 现有成功字段和 public `wave_id`。

### 输入、输出与状态变化

- 输入：operation kind、verb/topic、canonical payload、loop id、hat id、当前 contract digest、公开 Apply reference。
- 输出：稳定 deny reason、Apply 结果、operation/confirmation reference；现有 text/JSON 输出字段保持兼容，新增字段只能 additive。
- 状态：`prepared → claimed → applied_pending_confirm → confirmed`；Apply 前失败为 `restored`，Apply 已提交但确认不可得为 `recovery_required`。
- 副作用：拒绝不改 task store、event ledger、wave store 或 confirmation record；并发竞争最多一个 claimant。

### 错误语义

计划固定使用以下稳定分类；具体错误文本需沿用现有 prefix：

- `task_verify_gate denied`：task ticket 缺失、漂移、scope 不符、已 claim 或不可恢复；
- `wave_verify_gate denied`：wave ticket 同类错误；
- `contract_compile_failed`：U5 contract 无法编译；
- `precheck_stale`：Observe/contract/artifact 状态已变化，必须重新 Observe→Precheck；
- `confirmation_required`：上一次 Apply 未完成有效 Confirm；
- `confirmation_unavailable`：公共确认状态不可读或不一致；
- `applied_unconfirmed`：Apply 已提交，但确认 record 未能安全落盘。

### 兼容、性能与安全要求

- 兼容：旧命令、human bypass、既有 wave inspect JSON 成功字段和默认关闭配置继续工作。
- 性能：ticket claim 只增加一次受控 workspace lock；不得在每个 event 引入第二套 policy 解析或无界扫描。
- 安全：fail-closed 优先；不能把普通 SHA-256 文件内容描述成认证；不得把内部 ledger、ticket secret 或数据库路径注入 agent-facing skill。

### 本次范围

R1. 修复 task ticket 的原子 claim 与失败恢复。

R2. 按 operation/activation scope 隔离 task ticket，保留现有命令形状。

R3. 让 U5 contract compile failure 在 agent emit 路径 fail-closed。

R4. **BLOCKED**：为 task 与 wave 建立 runtime-owned confirmation record，并阻止未 Confirm 的下一次受保护 mutation；先完成 D5a–D5c 决策门。

R5. **BLOCKED，依赖 R4**：同步 agent-facing OPAC/task/wave/emit 文档与 drift 检查。

R6. 解决同权限恶意 agent 伪造 ticket的可信 authority问题；当前 BLOCKED，不进入可执行 Unit。

### 非目标

- 不重做 preset 业务拓扑、event schema 或 supervisor 调度。
- 不把 human CLI 改造成 agent ticket 模式。
- 不暴露 `.ralph/events.jsonl`、`.ralph/supervisor.db` 或 ticket 文件作为 Confirm 证据。
- 不新增一次性 preset 名称特判。
- 不引入没有证据支持的外部密钥服务、跨机器信任模型或新依赖。

### 已确认与待验证假设

已确认：

- wave 已有 `create_new` claim marker，可作为 task gate 的并发行为参考。
- `FileLock` 已存在并被 `TaskStore`、activation registry 等持久化模块使用。
- `wave inspect` 已是真实 Apply→Confirm 公共入口。
- `integration_tasks.rs`、`integration_wave_protocol_closure.rs`、`integration_wave_protocol_suite_u9.rs` 和 core scenarios 是真实测试入口。

待验证但不阻塞 R1–R3：

- task ticket 的具体 scope key 已在 D3 固定；只需由 U2 用测试证明，不得改变 tuple。

阻塞 R4/R6：

- 已确认当前 agent 子进程没有不由 workspace 文件伪造的 runtime authority；E19–E21 覆盖 runner env、RPC 和 control-plane。R6 只能通过新增并审定私有 issuer 协议解锁。
- 已确认当前 task 没有公开 Confirm 命令/DTO 或 confirmation state；E16–E18 覆盖 task query、close warning 和 wave public inspect。R4 必须先形成 D5a–D5c，不能在 U4 中临时创造。

## 2. 代码库现状与证据

### 2.1 当前实现入口

Task verify/apply 入口在 `crates/ralph-cli/src/task_cli.rs`：verify 通过 `verify_add`/`verify_ensure` 生成 canonical payload 并写 ticket；Apply 在 `add_task_with_args`/`ensure_task_with_args` 之前进入 gate，再调用 `TaskStore` mutation。

Wave verify/apply 入口在 `crates/ralph-cli/src/wave.rs`：verify 调 `wave_verify_gate::record_ticket`；emit 依次运行 precheck、ticket claim、idempotency/store Apply、consume 或 restore；`wave inspect` 读取公开 emission state。

Single-event emit 入口在 `crates/ralph-cli/src/commands/emit.rs`：`emit_command_with_root` 在写 event 前解析 policy、origin、scope、U5 capability/token gate。`U5Gate::resolve` 在 `execution_contract::compile` 失败时使用 `resolved=None`。

Agent context 在 `crates/ralph-cli/src/operation_guard.rs::OperationContext` 中由 marker/env 解析。当前未确认统一 operation confirmation state；只确认了 `inspect loop` 和 wave inspect 公共诊断/确认表面。

测试框架为 Rust unit/integration tests、real EventLoop scenario runner 和 `cargo nextest`。仓库硬规则要求测试使用 `./scripts/run-tests.sh` 或 targeted `cargo nextest run`，禁止裸跑 `cargo test -p ralph-cli`。

### 2.2 Evidence Ledger

| ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `crates/ralph-cli/src/task_verify_gate.rs:49-110` | fingerprint 是 `verb + payload + loop + hat` 的公开 SHA-256；`record_ticket` 用 `std::fs::write` 写固定路径。 | R2 必须消除固定单槽位；R6 不能把 SHA-256 当认证。 | 高 |
| E2 | `crates/ralph-cli/src/task_verify_gate.rs:172-181` | `read_and_consume_ticket` 明确先读后删，存在并发窗口。 | R1 必须在同一锁保护下完成匹配与 claim。 | 高 |
| E3 | `crates/ralph-cli/src/task_verify_gate.rs:204-265` | `require_ticket` 在 fingerprint/loop/hat 比较前调用 read-and-consume。 | R1 必须让拒绝保持 prepared，避免误消费。 | 高 |
| E4 | `crates/ralph-cli/src/wave_verify_gate.rs:403-433` | wave 使用 `OpenOptions::create_new(true)` 创建 claim marker。 | R1 复用已验证的原子 claim 行为，不复制 task 的 TOCTOU。 | 高 |
| E5 | `crates/ralph-cli/src/wave.rs:252-352` | `wave inspect` 提供 public `wave_id`、phase、registered、availability 等确认状态。 | R4 保持既有 DTO，新增字段只能 additive。 | 高 |
| E6 | `crates/ralph-cli/src/commands/emit.rs:267-369` | U5 token 绑定 hat/topic/payload/contract revision；compile 失败后 `resolved=None`。 | R3 使用显式 CompileFailed 状态，禁止 fail-open。 | 高 |
| E7 | `crates/ralph-cli/src/commands/emit.rs:393-427` | capability gate 对无 resolved 的路径可跳过；token gate 另行拒绝，导致不一致。 | R3 统一 compile failure reason 和 side-effect 顺序。 | 高 |
| E8 | `crates/ralph-cli/src/operation_guard.rs:23-105,187-201` | OperationContext 有 agent/loop/hat 与 fail-closed helper，没有统一 confirmation state。 | R4 需要扩展既有 context/guard，不新增第二套 caller identity。 | 高 |
| E9 | `crates/ralph-cli/tests/integration_wave_protocol_closure.rs:107-170` | 真实 Verify→Emit→Inspect 已验证 public Confirm happy path。 | R4 以该测试模式扩展未 Confirm、不可用、重启恢复。 | 高 |
| E10 | `crates/ralph-cli/tests/integration_tasks.rs:1-68` | task 集成测试通过 `common::ralph_bin()`，已有 env scrub 兼容模式。 | R1/R2 使用相同 subprocess 入口，避免污染 agent context。 | 高 |
| E11 | `crates/ralph-core/src/file_lock.rs:43-257`、`task_store.rs:430-496` | FileLock 已用于跨进程读写和原子 read-modify-write。 | R1/R2 优先复用既有锁，不新增依赖。 | 高 |
| E12 | `crates/ralph-cli/src/hat_command_policy.rs:351-420` | 相邻 command policy 对 contract compile failure 有 `contract_compile_failed` deny 语义。 | R3 复用错误分类与 fail-closed 约定。 | 高 |
| E13 | `docs/plans/2026-08-01-001-fix-unified-execution-contract-p0-p1-plan.md` | 当前项目已有严格 evidence ledger、串行 U-ID、真实 BDD 与 no-silent-fallback 计划模式。 | 本计划沿用该结构与测试门禁。 | 中 |
| E14 | `crates/ralph-core/data/ralph-tools-opac.md:15-24,123-145` | OPAC 要求每个 Apply 前 Precheck、之后 Confirm，但规则主要注入 agent prompt。 | R4 需要把关键后置条件落到 runtime。 | 高 |
| E15 | `crates/ralph-cli/src/commands/emit.rs:354-359`、`crates/ralph-core/src/execution_contract/activation.rs:119-190` | 当前有 contract compile fail-open 分支；activation registry 有独立 fail-closed 模式，但未发现可作为 ticket secret 的 runtime authority。 | R6 阻塞；不得自创密钥传递。 | 高 |
| E16 | `crates/ralph-cli/src/task_cli.rs:1370-1425,1941-1995`、`crates/ralph-core/src/task_store.rs:548-761` | `task list`/`task show` 只读取并输出 TaskStore 的 id、status、priority、title、key、loop、owner 等任务字段；没有 confirmation state、receipt 或 Confirm transition。 | R4 不能复用现有 task 查询直接解锁；必须新增并审定 public adapter/状态边界。 | 高 |
| E17 | `crates/ralph-cli/src/task_cli.rs:1590-1685`、`crates/ralph-core/data/ralph-tools-tasks.md:159` | task close 后的 OPAC 反馈是 completion topic 缺失时的 stderr warning 与下一步提示，不是 runtime mutation gate。 | R4 不能把 warning 当作 confirmation evidence；现有行为需保留为兼容提示。 | 高 |
| E18 | `crates/ralph-cli/src/wave.rs:252-352`、`crates/ralph-core/src/supervisor/rusqlite.rs:2471-2495`、`crates/ralph-core/src/supervisor/migrations/v3.sql:40-51` | `wave inspect` 读取 `wave_emissions`/fan-in 状态，提供 public wave id、phase、availability 等 DTO；该状态模型只覆盖 wave emission。 | R4 的 wave 侧可复用既有 public DTO，但不能推导出 task 的 Confirm authority。 | 高 |
| E19 | `crates/ralph-cli/src/loop_runner/execution.rs:40-118`、`crates/ralph-cli/src/loop_runner/wave/worker.rs:214-228` | agent/worker spawn 只注入 hat、loop、events file、triggered hat、hats source、config、TERM/NO_COLOR；未发现 runtime-held secret、签发句柄或私有 capability channel。 | R6 不能通过现有 env 注入闭合；普通环境变量不可作为不可伪造 authority。 | 高 |
| E20 | `crates/ralph-cli/src/rpc_stdin.rs:1-220`、`crates/ralph-cli/src/loop_runner/runner.rs:1800-1905` | RPC stdin 处理 Prompt、Guidance、Steer、FollowUp、Abort、GetState、GetIterations；没有 ticket issue/verify 或 mutation authorization 方法。 | 现有 RPC 不能直接承担 R6 issuer；若复用必须新增明确协议与权限边界。 | 高 |
| E21 | `crates/ralph-core/src/control_plane.rs:1-220` | control-plane 只校验 events file 的绝对路径、workspace/slot 范围、symlink escape 和父目录可写性，不认证写入者。 | 路径隔离不等于 agent 身份认证；R6 继续 BLOCKED。 | 高 |

### 2.3 受影响范围

生产模块：`crates/ralph-cli/src/task_verify_gate.rs`、`task_cli.rs`、`wave_verify_gate.rs`、`wave.rs`、`commands/emit.rs`、`operation_guard.rs`；R4 的新增 confirmation projection 尚未进入可执行影响范围，必须先通过 D5a–D5c 决策门。

测试模块：`crates/ralph-cli/src/task_verify_gate.rs`、`task_cli.rs`、`crates/ralph-cli/tests/integration_tasks.rs`、`integration_wave_protocol_closure.rs`、`integration_wave_protocol_suite_u9.rs`、emit policy 集成测试、`crates/ralph-core/tests/scenarios/opac/` 与 `scenarios.rs`。

文档模块：`crates/ralph-core/data/ralph-tools-opac.md`、`ralph-tools-tasks.md`、`ralph-tools-wave.md`、`ralph-tools-emit.md`、`docs/guide/opac.md`；若实际 CLI/schema 能力改变，再按仓库硬规则同步 `CLAUDE.md`、`AGENTS.md`、preset operator references 与 zsh completion。

未确认的新增路径不作为当前事实；R4 的 confirmation projection、task public adapter 和 gate owner 尚未进入影响范围，必须在 D5a–D5c 达到阈值后再加入。

## 3. 决策记录与置信度

| ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | task claim 如何避免并发双消费？ | 先读后删；复用 FileLock/原子 claim | 复用 FileLock，在同一独占锁内完成读取、完整匹配、claim；成功 Apply 后 consume | E2,E4,E11 | 先读后删已被 E2 直接证明有窗口 | 0.98 |
| D2 | mismatch 是否消费 ticket？ | 保持当前；拒绝后保留 prepared | 拒绝、scope mismatch、过期和格式错误均不消费；只有成功 claim 后才消费/restore | E3,E14 | 当前行为与失败可恢复目标冲突 | 0.99 |
| D3 | 如何消除固定单 ticket 覆盖？ | 继续单文件；按 operation scope 分片 | ticket key 固定为 `workspace identity + loop + hat + operation kind + canonical payload digest`；同一 key 的重复 verify 复用/刷新同一 prepared record，不影响其他 key；命令形状不变 | E1,E10,E11 | 单文件已被 E1 证明覆盖 intent；该 tuple 与现有 fingerprint 输入一致 | 0.90 |
| D4 | U5 compile failure 如何处理？ | `resolved=None` 继续；显式 CompileFailed deny | agent context + governed U5 active 时直接拒绝；human/stand-down 继续原行为 | E6,E7,E12 | fail-open 与邻近 policy deny 冲突 | 0.98 |
| D5 | Confirm 如何增强而不破坏 OPAC？ | 继续纯 prompt；新增外部强制 shell；runtime-owned record | **BLOCKED：先锁定唯一 record store、task public evidence adapter、Apply/Confirm/gate 原子边界；未锁定前不实现 R4** | E5,E8,E9,E14 | 当前仅 wave 有 public inspect；没有 task Confirm 入口或统一 record state 的直接证据 | 0.70 |
| D6 | 是否现在实现 ticket MAC/IPC？ | 普通文件 SHA；环境变量 HMAC；runtime secret/IPC；明确排除 | **BLOCKED：先确认 runtime authority 与威胁模型；不得实现普通 SHA 或未经证据支持的 secret 传递** | E1,E15,E19–E21 | runner 只有普通 env，RPC 无 issuer，control-plane 只做路径隔离 | 0.68 |

### D6 阻塞处理

缺失证据：runner 如何向 agent 子进程传递不可由 workspace 文件伪造的 authority；同权限 agent 是否属于必须防御的 threat model。

调查结论：已沿 `crates/ralph-cli/src/loop_runner/` 的 spawn、env、activation contract、RPC/stdio 边界检查；E19–E21 证明当前只有可读写环境和路径隔离，没有签发 authority。后续若继续推进，必须先提出新的私有 issuer 协议/权限边界 Decision Record，不能在现有 env 上加固定 secret。

重新决策条件：找到现有可信 authority，或明确设计并批准新的私有 issuer 协议，且证明 agent 进程不能通过 workspace/env 伪造；否则 R6 保持 blocked，不得进入执行。

### D5 阻塞处理

候选架构已单独记录在 [`docs/plans/2026-08-03-002-design-opac-confirm-authority-architecture.md`](2026-08-03-002-design-opac-confirm-authority-architecture.md)；该文档仍处于 `DECISION REQUIRED`，不解除本计划的 BLOCKED 状态。

缺失证据：task 是否应新增可作为 Confirm 的公开命令/DTO；task、wave、single emit 是否应共享同一 record store；Apply 写入、record 写入、public evidence 读取和下一次 mutation gate 的事务边界；重启、损坏和 store unavailable 的唯一状态迁移。E16–E18 证明当前不存在可直接复用的 task Confirm state。

调查结论：已沿 `task_cli.rs` 的 `task list/show`、`TaskStore` mutation、`wave.rs` inspect/emission store 和 `commands/inspect.rs` 的 public DTO 核验；当前只有 wave public Confirm surface。后续若继续推进，必须先提出 D5a（store/schema）、D5b（task 新增 public evidence adapter）、D5c（gate boundary）三个 Decision Record，并分别定义新增 CLI/API surface、状态迁移和兼容策略。当前不得把这些新增接口写进执行 Unit，也不得让 Executor 临场创造。

重新决策条件：为 D5a–D5c 各提供真实源码入口、调用方、状态迁移和 ATDD Red；三项置信度均达到 0.85 后，才能把 U4 拆成 task Confirm、wave Confirm、下一 mutation gate 三个串行 Unit。否则 R4 保持 blocked。

## 4. BDD 行为规格

### Feature: task ticket 原子 claim

  Background:
    Given agent context has a valid loop id and hat id
    And `tasks.require_verify_for_cli_mutate` is enabled

  Scenario: matching task Apply claims exactly once
    Given `task verify add` has created a matching prepared ticket
    When two Apply processes use the same operation
    Then exactly one process claims the ticket
    And at most one task mutation is committed
    And the losing process receives `task_verify_gate denied`

  Scenario: fingerprint mismatch does not consume the prepared ticket
    Given a prepared ticket for task title A
    When Apply uses task title B
    Then Apply is denied with a fingerprint mismatch
    And the ticket remains available for a correct Apply
    And no task is written for B

  Scenario: caller mismatch does not consume the prepared ticket
    Given a ticket bound to loop L and hat H
    When another loop or hat attempts Apply
    Then Apply is denied
    And the original ticket remains available to L/H

### Feature: scoped task ticket isolation

  Scenario: add and ensure tickets coexist
    Given `task verify add` and `task verify ensure` run in one workspace
    When each matching Apply runs
    Then neither ticket overwrites the other
    And each operation can be applied at most once

  Scenario: later verify does not invalidate an unrelated pending operation
    Given operation A and operation B have different intent keys
    When B is verified after A
    Then A remains applicable with its own matching scope
    And B remains applicable with its own matching scope

### Feature: U5 contract compile fail-closed

  Scenario: agent emit denies a contract compile failure
    Given the agent context activates U5
    And the execution contract returns a compile finding
    When the agent runs policy-check or Apply
    Then the command exits non-zero with `contract_compile_failed`
    And no event or idempotency row is written

  Scenario: valid U5 flow remains compatible
    Given a valid compiled contract
    When the agent runs the existing policy-check token flow
    Then valid token behavior remains unchanged
    And missing or mismatched tokens retain their existing stable errors

### Feature: runtime Confirm obligation

  Scenario: task Apply produces a public confirmation record
    Given a valid task Apply
    When the mutation commits
    Then a confirmation record references the task identity and intent digest
    And the existing task query remains usable

  Scenario: missing task Confirm blocks the next protected mutation
    Given a prior agent Apply is `applied_pending_confirm`
    When the agent attempts another protected mutation
    Then the mutation is denied with `confirmation_required`
    And no new task/event/wave side effect is written

  Scenario: public Confirm releases the next mutation
    Given a pending confirmation record matches the public task or wave reference
    When the agent performs the existing public Confirm query successfully
    Then the record becomes `confirmed`
    And the next matching protected mutation is allowed

  Scenario: unavailable Confirm fails closed
    Given Apply committed but the public Confirm store is unavailable or inconsistent
    When the agent attempts the next protected mutation
    Then the mutation is denied with `confirmation_unavailable` or `applied_unconfirmed`
    And the system does not report a false terminal success

### Feature: compatibility boundaries

  Scenario: human CLI bypass remains usable
    Given no agent runtime context
    When a human runs existing task or emit commands
    Then no agent ticket or Confirm gate is required
    And the existing command result remains valid

  Scenario: legacy plaintext ticket is not trusted
    Given an old-format plaintext ticket is placed in the legacy path
    When an agent attempts Apply
    Then Apply is denied with a stable re-verify or migration hint
    And the old ticket is not silently accepted as authority

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
|---|---|---|---|---|---|
| task matching claim | 两个真实进程最多一个 Apply，store 最多一个结果 | `integration_tasks.rs` + gate tests | unit + integration | concurrency/idempotency | 否 |
| mismatch preservation | mismatch 后 ticket 可被正确参数重试，B 不产生副作用 | `task_verify_gate.rs` | unit | characterization | 否 |
| scoped isolation | add/ensure、不同 loop/hat 的 ticket 不互相覆盖 | `integration_tasks.rs` | integration | cross-process | 否 |
| U5 compile failure | agent policy-check/Apply 非零，event ledger 与 idempotency 无变化 | emit unit + CLI emit integration | unit + integration | fault injection/config corruption | 否 |
| valid U5 compatibility | valid token、missing token、mismatch token 与 stand-down 分支保持原断言 | existing emit policy tests | regression | differential | 否 |
| task Confirm | Apply record、query、未 Confirm 阻断、Confirm 后放行 | **BLOCKED：D5b 解锁后确认 task public adapter 与测试目标** | integration + BDD | restart/unavailable store | 否 |
| wave Confirm | 既有 Verify→Emit→Inspect 继续通过，pending/unavailable 路径 fail-closed | `integration_wave_protocol_closure.rs`（现有 happy path 已确认；新增 gate 断言待 D5 解锁） | integration | public DTO additive check | 否 |
| human compatibility | human task/emit 不需要 agent receipt | `integration_tasks.rs` / emit tests | integration | env scrub | 否 |
| legacy ticket | 旧明文 ticket 不被信任 | gate test + subprocess | unit + integration | tamper/fuzz truncated record | 否 |

所有新增断言必须验证：exit code、稳定 reason、实际 store/ledger 副作用、ticket state、公开 Confirm state 和后续 mutation 是否被允许。不得只检查源码文本或 prompt 文案。

## 6. 需求—测试追踪矩阵

| Requirement | 需求 | Scenario | 验收测试 | 单元测试 | 集成/BDD | E2E | Evidence | Unit |
|---|---|---|---|---|---|---|---|---|
| R1 | task claim 原子且最多一次 | S1–S3 | concurrent task Apply | gate claim cases | `integration_tasks.rs` | — | E2–E4,E11 | U1 |
| R2 | task ticket 不覆盖且 mismatch 可恢复 | S4–S5 | add/ensure isolation | namespace/path cases | `integration_tasks.rs` | — | E1,E10,E11 | U2 |
| R3 | U5 compile failure fail-closed | S6–S7 | compile-failure emit | U5 resolution cases | emit policy integration | — | E6,E7,E12 | U3 |
| R4 | Apply 后 Confirm runtime 可验证 | S8–S10 | **BLOCKED：D5a–D5c 解锁后指定** | 不得提前新增 | D5 解锁后新增真实 core/CLI integration | — | E5,E8,E9,E14 | U4 blocked |
| R5 | agent-facing文档与真实命令一致 | S11–S12 | **BLOCKED：依赖 R4 public behavior** | 不得提前新增 | D5/U4 解锁后指定 | — | E5,E14 | U5 blocked |
| R6 | 同权限恶意 agent 无法伪造 ticket | — | **BLOCKED** | 不创建伪造成功断言 | 不进入实现 | — | E1,E15,E19–E21 | blocked |

## 7. 严格串行开发单元

### U1. 修复 task ticket 的原子 claim 与误消费

**目标：** 让 task ticket 在完整匹配后原子 claim；并发 Apply 最多一个成功，拒绝和 Apply 前失败不消费 ticket。

**需求/Scenario/Decision/Evidence：** R1；S1–S3；D1–D2；E2–E4、E11。

**修改位置：**

- `crates/ralph-cli/src/task_verify_gate.rs`：替换 `read_and_consume_ticket` 在 gate 中的使用，先完成 scope/fingerprint 校验，再在独占 `FileLock` 或既有等价锁内 claim。
- `crates/ralph-cli/src/task_cli.rs`：保持现有 verify/apply 调用链，只接入 claim/restore/consume 结果。
- `crates/ralph-core/src/file_lock.rs`：仅在现有公开能力不足时扩展最小锁操作；不得改变既有 lock 语义。
- `crates/ralph-cli/src/task_verify_gate.rs` tests：增加真实并发与失败恢复覆盖。

**当前 Red：** 先运行 mismatch、并发 Apply 和 Apply 前保存失败测试；当前 mismatch 会先消费票据，两个进程可能都读到票据，因此应分别以“ticket missing/第二 Apply成功”失败。

**最小实现：** 保持 ticket fingerprint 算法和命令形状；新增 `prepared/claimed/restored/consumed` 状态；拒绝路径不改变 prepared；成功 Apply 后才 consume。不得在本 Unit 重做多 operation store 或 U5/Confirm。

**测试：**

- fingerprint、loop、hat 任一不匹配后，正确参数可再次 Apply；
- 在 `crates/ralph-cli/tests/integration_tasks.rs` 新增真实 subprocess 测试 `task_verify_concurrent_apply_claims_once`，两个 `common::ralph_bin()` 进程同时 Apply，恰一个 claim 成功；
- Apply 写入失败后 ticket 可重试；
- claim marker 遗留时按现有 wave recovery 语义返回稳定 deny，不重复 mutation；
- human CLI、gate off、unsafe task path 保持现有结果。

**验证：** `cargo nextest run -p ralph-cli --bin ralph -- task_verify_gate`；`cargo nextest run -p ralph-cli --test integration_tasks`。命令失败不得进入 U2。

**不依赖/不实现：** 不依赖 U2 的多槽位 store；不实现 MAC；不改变 wave gate。

### U2. 隔离 task operation ticket

**目标：** 让不同 operation kind、intent key、loop/hat 的 pending ticket 共存，后一次 verify 不覆盖无关操作。

**需求/Scenario/Decision/Evidence：** R2；S4–S5；D3；E1、E10、E11。

**修改位置：** `crates/ralph-cli/src/task_verify_gate.rs` ticket resolver/store；`task_cli.rs` 的 verify_add/verify_ensure 与 apply gate 接线；`integration_tasks.rs` 新增 subprocess 场景。

**当前 Red：** 先运行交错 `verify add A → verify ensure B → apply A`；当前固定路径只能保留最后一次写入，A 无法可靠 Apply。

**最小实现：** 在 workspace 内增加按 operation kind、scope 和 intent key 的新 ticket 命名空间；保留旧路径只作为无效 legacy 识别入口；旧明文不静默接受。现有 `verify` 与 Apply 参数、text/quiet 输出不变。

**测试：**

- add 与 ensure ticket 并存并分别只能消费一次；
- 不同 loop/hat 不可互相消费；
- 同一 intent 的重复 verify 具有确定覆盖/刷新语义，并且不影响其他 intent；
- 旧固定路径 plaintext 被拒绝并提示重新 verify；
- gate off 与 human CLI 不读取新 agent ticket store。

**验证：** `cargo nextest run -p ralph-cli --test integration_tasks`；相关 gate targeted nextest。全部通过后才进入 U3。

**不依赖/不实现：** 不修改 wave ticket；不引入 MAC/secret；不添加新的 preset 配置。

### U3. 修复 U5 contract compile failure fail-open

**目标：** agent context 下 U5 active 且 contract 编译失败时直接 deny，并在任何 event/idempotency/ticket 副作用前返回稳定 `contract_compile_failed`。

**需求/Scenario/Decision/Evidence：** R3；S6–S7；D4；E6–E7、E12。

**修改位置：** `crates/ralph-cli/src/commands/emit.rs` 的 `U5Gate::resolve`、`capability_denied`、`token_violation`；现有 emit policy 单测与真实 emit 集成测试。

**当前 Red：** 在现有 `commands::emit` 单测位置新增明确测试 `u5_compile_failure_denies_before_event_write`，使用会触发 `execution_contract::compile` finding 的最小 config fixture 运行 agent policy-check/Apply；当前 `resolved=None` 路径不会按 capability 统一拒绝，预期看到非零、`contract_compile_failed` 和无 event/idempotency 副作用的测试应失败。若无法构造稳定 finding，停止 U3 并先补证据，不得把配置错误当作 Red。

**最小实现：** 用显式三态 `Inactive/Active/CompileFailed` 或等价 Result 状态替代 compile failure 的 `None`；compile failure 在 idempotency、event write 和 emit 结果落盘前拒绝。当前没有证据证明 single emit 调用 task ticket，因此本 Unit 不断言或修改 task ticket；valid contract、human CLI、pseudo-hat、wave worker、无 hats stand-down 保持既有行为。

**测试：**

- compile failure 的 policy-check 和 Apply 均拒绝且不写 event；
- 不产生空 token，不误报 `missing_policy_check_token`；
- valid contract 的 allow/deny/token success/mismatch 保持现有断言；
- human CLI 与已有 stand-down 条件不受影响；
- contract digest 改变使旧 token 失效。

**验证：** `cargo nextest run -p ralph-cli --bin ralph -- test_emit_policy_check`（验证 `commands::emit` 中已确认存在的 U5/策略单测）；`cargo nextest run -p ralph-cli --test integration_emit_policy`（验证已确认存在的真实 emit 集成目标）。

**不依赖/不实现：** 不改变 execution contract 编译器规则；不改变 ticket store；不实现 Confirm。

### U4. 建立 runtime-owned Confirm 后置条件

**状态：BLOCKED，不得启动实现。** 保留现有 public Confirm 查询，同时使未 Confirm、Confirm unavailable、digest 不一致的 agent 后续 protected mutation fail-closed；但当前没有 task public Confirm adapter、唯一 record store 或事务边界证据。

**需求/Scenario/Decision/Evidence：** R4；S8–S10；D5；E5、E8、E9、E14。

**解锁调查：** 先沿 `task_cli.rs` 的 `task list/show`、`TaskStore` mutation、`wave.rs` inspect/emission store 和 `commands/inspect.rs` public DTO 做调用链核验。必须形成 D5a（唯一 store/schema/事务边界）、D5b（task/wave evidence adapter）、D5c（下一 mutation gate）三个 Decision Record，且每项置信度达到 0.85；否则停止，不写 U4 红测，不新增 `task confirm` 或通用 confirmation record。

**修改位置：** 已确认入口为 `operation_guard.rs`、`task_cli.rs`、`wave.rs`、`commands/inspect.rs`、`integration_wave_protocol_closure.rs`、`integration_tasks.rs`、core OPAC scenarios。confirmation projection 的新增文件只有在前置调查确认后才能写入计划/Unit。

**当前没有有效 Red：** 现有代码缺少 task Confirm 入口和统一 runtime record，不能把一个尚未锁定接口的测试伪装成可执行 Red。解锁调查完成后必须先确定真实 public command/DTO、fixture 和失败 reason，再将本节拆成独立实现 Unit。

**最小实现：** Apply 成功写入 runtime-owned record；record 绑定 operation/intent digest、loop、hat、public reference、outcome、confirmation state；下一次 protected mutation 检查 record；public Confirm 成功后状态变为 confirmed。wave 复用 `wave inspect`，既有 JSON 字段保持，新增字段 additive；human CLI 不受影响。

**测试：**

- task Apply→公开 task 查询→Confirm→下一次 Apply；
- task Apply 未 Confirm 阻止下一次 Apply且无副作用；
- wave Verify→Emit→Inspect→下一次 Apply保持现有 happy path；
- Confirm unavailable、digest mismatch、record corruption 均 fail-closed；
- Apply 已写入但 cleanup/receipt 失败返回 `applied_unconfirmed`，重启不重复写入；
- human CLI、gate off、既有 wave inspect success 字段不变。

**验证：** `cargo nextest run -p ralph-cli --test integration_wave_protocol_closure`；`cargo nextest run -p ralph-cli --test integration_tasks`；`cargo nextest run -p ralph-core --test scenarios`。若 scenario fixture 不走真实 `run_workflow_guard_scenario`，先修测试入口，不得用 stub 替代。

**不依赖/不实现：** 当前不实现 R4、R6，不暴露内部 ledger，不把 Confirm 简化成检查 stdout。

### U5. 同步 agent-facing 文档与回归门禁（阻塞）

**状态：BLOCKED，不得启动实现。** 让注入给 agent 的 OPAC/task/wave/emit 指南准确描述新行为，并通过静态 drift 检查；必须等 U4 的 public behavior 锁定后才能产生真实 Red。

**需求/Scenario/Decision/Evidence：** R5；S11–S12；D1–D5；E5、E14。

**修改位置：** `crates/ralph-core/data/ralph-tools-opac.md`、`ralph-tools-tasks.md`、`ralph-tools-wave.md`、`ralph-tools-emit.md`、`docs/guide/opac.md`；只有实际新增 CLI 参数或 preset/operator 可见行为时，才同步 `CLAUDE.md`、`AGENTS.md`、preset author/review references 与 zsh completion。

**文档要求：** 说明触发条件、agent 应执行的现有命令、关键字段来源、失败停止条件；不写内部函数名、ticket 文件路径、secret、ledger 路径或一次性 plan 背景。

**当前没有有效 Red：** help、skill anchor 和 drift 检查是现有 Green 入口，不能为了满足 TDD 人为削弱断言或伪造失败。U4 解锁后，先从实际 CLI 行为生成必须变化的文档契约测试，再更新文档。

**验证：** `./scripts/check-cli-doc-drift.sh`；涉及命令时运行对应 `ralph <cmd> --help`；`cargo nextest run -p ralph-cli --test integration_agent_reference`；相关 core skill inventory/anchor tests。

**不依赖/不实现：** 不把 R6 blocked 内容写成 agent 可执行规则；不复制 runtime 内部实现细节到注入 skill。

### R6 阻塞单元（不得启动实现）

目标是证明或否定同权限恶意 agent 的 ticket 不可伪造性。当前不能进入正式 Unit。

必须先完成：runner spawn/activation authority 调查、威胁模型确认、候选 authority 的最小实验设计。若无可信 authority，必须把 R6 从当前计划移入 follow-up，并将安全边界写成“不防御同 OS 用户可写 workspace 的恶意进程”。

禁止：在 Unit 1–5 中临时增加环境变量 HMAC、固定常量、普通 SHA-256、或把 ticket 文件权限当作独立 authority。

## 8. Unit 串行依赖图

```text
U1 atomic task claim
  ↓
U2 scoped task ticket store
  ↓
U3 U5 compile fail-closed
  ↓
D5a/D5b/D5c decision gate (blocked)
  ↓
U4 runtime Confirm gate (仅解锁后)
  ↓
U5 agent-facing docs and regression gate (仅解锁后)
  ↓
R6 threat-model / authority decision (blocked, not implementation)
```

U2 依赖 U1 已验证的 claim/restore 生命周期，否则会重新实现并发协议。U3 依赖 U2 确认 ticket side-effect 顺序，但只验证 emit 自身已证实的 event/idempotency side effect。D5a/D5b/D5c 必须先锁定 U4 的 store、公开 adapter 和 gate 边界；U4 完成后才能启动 U5。R6 不得被任何前置 Unit 假设已完成。

## 9. 执行命令清单

所有 Rust 测试使用 nextest 系列；命令失败不得进入下一个 Unit。

| 时机 | 命令 | 目的 | 预期 |
|---|---|---|---|
| U1 gate | `cargo nextest run -p ralph-cli --bin ralph -- task_verify_gate` | task gate 原子 claim/恢复 | targeted tests 全绿 |
| U2 gate | `cargo nextest run -p ralph-cli --test integration_tasks` |真实 task subprocess、scope 与 human/env 兼容 | 全绿 |
| U3 gate | `cargo nextest run -p ralph-cli --bin ralph -- test_emit_policy_check` | U5 policy/compile 单元 | 全绿 |
| U3 integration | `cargo nextest run -p ralph-cli --test integration_emit_policy` | 真实 emit side-effect 与错误输出 | 全绿 |
| U4 gate | **BLOCKED**；解锁后再确定真实 task/wave integration target | Confirm store、公开 adapter 与恢复 | 未解锁不得运行 |
| U4 BDD | **BLOCKED**；解锁后新增并注册真实 `run_workflow_guard_scenario` | 真实 EventLoop OPAC flows | 未解锁不得运行 |
| U5 docs | `./scripts/check-cli-doc-drift.sh`（仅在 U4 解锁后） | CLI 文档漂移 | 退出码 0 |
| 最终 | `./scripts/run-tests.sh` | workspace nextest 两阶段、doctest 与全量门禁 | 全绿 |
| 最终 | `cargo build` | build | 成功 |
| 最终 | `cargo clippy` | lint | 无新增 failure |

不得手动 `cargo nextest run --workspace` 代替仓库规定的最终两阶段入口。不得裸跑 `cargo test -p ralph-cli`。

## 10. 最终质量门禁

- R1–R3 都有 BDD/ATDD Scenario、Evidence、Unit 和可执行测试入口；R4/R5 在阻塞解除前不得宣称完成；
- U1–U3 严格按顺序完成，每个 Unit 具备真实 Acceptance Red、最小实现、targeted Green、集成验证和回归；U4/U5 在解锁前不启动；
- task mismatch 不消费 ticket；并发 Apply 最多一个 winner；不同 operation/activation 不覆盖；
- U5 compile failure 永不在 agent governed path fail-open；不写 event/idempotency/ticket side effect；
- Apply 后 Confirm record、public query、未 Confirm 阻断、Confirm 后放行和 unavailable fail-closed 目前未完成，必须由 D5a–D5c 解锁后补齐真实 runtime 断言；
- human CLI、gate-off、unsafe recovery、wave inspect 旧字段和现有 OPAC 命令形状保持兼容；
- 文档遵守 agent 下一步可执行规则，不泄漏内部 ledger/secret/path；
- `./scripts/run-tests.sh`、`cargo build`、`cargo clippy`、CLI doc drift 全部通过；
- 无新增 skip/only、无削弱断言、无无解释 snapshot/golden 变化、无临时 ticket/receipt 文件；
- R6 未达到 0.85 前不得标记计划 READY，不得声称 ticket 防伪造已完成；
- 任何实现中发现的调用链冲突、未知 public consumer、需要新依赖或状态存储分叉，都必须停止当前 Unit 并更新 Evidence/Decision。

## 11. 最终计划自检

| 检查项 | 结果 | 说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap | 是 | 有入口、Evidence、Decision、BDD、Unit、命令和 DoD |
| Executor 是否仍需做关键设计决策 | 否（当前可执行范围） | R1–R3 已锁定；D5a–D5c、R6 明确 blocked，不得临场设计 |
| 所有文件和接口是否有代码库证据 | 是（当前可执行范围） | R1–R3 位置有源码证据；D5 阻塞项明确标为未确认 |
| 所有关键决策置信度是否 ≥ 0.85 | 否 | D5=0.70、D6=0.68，均已显式阻塞，未伪装 READY |
| 是否存在未处理的低置信度假设 | 否 | D5、D6 均有缺失证据、调查动作和重新决策条件 |
| 每个 Unit 是否只有一个可观察行为 | 是（当前可执行范围） | U1 claim、U2 scope、U3 compile；U4/U5 尚未解锁 |
| 每个 Unit 是否可以独立验证 | 是（当前可执行范围） | U1–U3 有 targeted nextest 与 stop gate |
| 每个 Unit 是否有真实 Red | 是（当前可执行范围） | U1–U3 先写对应失败测试；U4/U5 明确没有 Red 且不启动 |
| 每个 Unit 是否包含回归范围 | 是 | 各 Unit 明确相关 integration/BDD/compatibility |
| 是否存在未来 Unit 依赖 | 否（当前可执行范围） | U1→U2→U3 后先经过 D5a–D5c；未解锁不进入 U4/U5，R6 是 blocked gate |
| 是否存在泛化任务描述 | 否 | 每个 Unit 指定行为、入口、失败原因和完成证据 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | R/S/E/Unit 矩阵已列出 |
| 所有关键决策是否有 Evidence | 是 | D1–D6 均链接 Evidence |
| 计划是否可以严格串行执行 | 是 | READY 范围可串行；R6 未决不得启动 |

## Product Contract preservation

本次为直接 `ce-plan` bootstrap，没有上游 requirements-only artifact；用户确认的范围被保留为“修复高置信度 OPAC P0/P1 问题且不破坏现有 OPAC 用法”。由于 R4 的 task Confirm authority 和 R6 的 ticket authority 均缺少现有实现证据，计划明确标记 PARTIALLY READY，而不是改变用户范围或伪造技术确定性。
