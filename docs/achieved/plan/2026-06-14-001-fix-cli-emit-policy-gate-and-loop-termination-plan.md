---
title: 修复 ralph emit 的 preset 策略门与 loop 终止父进程退出
type: fix
status: active
date: 2026-06-14
origin: docs/report/2026-06-14-ce-executor-isolated-task-closed-but-loop-running-diagnosis.md
---

# 修复 `ralph emit` 的 preset 策略门与 loop 终止父进程退出

## 概述

当前 `ralph emit` 加载配置的路径**不会**合并 builtin preset 的 `event_policy`,而 loop runner 会。这就导致格式错误的事件(例如 `work.done` 必须是 `json_object`,agent 却发了一个字符串)能顺利通过 CLI 预检、落盘到受信任的事件 JSONL,随后 loop 读到时以 `PayloadContractViolation` 终止。与此同时,task 系统已经在 `work.done` 落地时把任务标为 closed,而 stdio/TUI 父进程又没有被可靠地通知退出,于是用户同时看到三个互相矛盾的信号:任务已关、worktree 已推进、终端还卡着。

本计划修复两个 P0 根因:

1. **P0-1**:让 `ralph emit` 解析与 loop 相同的合并后配置,使 preset `event_policy` 在 CLI 入口就被 enforce。
2. **P0-2**:确保非 `LOOP_COMPLETE` 的 loop 终止能把退出信号传给 `ralph run` 的 stdio/TUI 父进程。

---

## 问题框定

- Agent 通过 `ralph emit <topic> --payload ...` 发出事件。
- Preset `ce-executor-isolated` 定义了严格的 `event_policy` schema(例如 `work.done` 必须是 JSON object 且含指定字段)。
- Loop runner 会执行这条策略并在违规时终止。
- CLI `emit` 命令目前看不到 preset 策略,因为它调用 `load_config_with_overrides`,该函数从不合并 `HatsSource` overlay。
- 当 loop 因 payload contract violation 终止时,父进程 `ralph run`(stdio 或 TUI)可能继续挂着,因为终止目前只是以 `Ok(TerminationReason::...)` 返回,没有保证父进程一定退出。

---

## 需求追溯

- R1. `ralph emit` 必须在写入受信任事件文件之前,拒绝违反当前 preset 有效 `event_policy` 的事件。
- R2. `ralph emit` 加载的配置必须与 loop runner 对 `event_policy`(及相关 CLI emit 开关)使用相同的 preset overlay。
- R3. 当 loop 因任何非成功 `LOOP_COMPLETE` 的原因终止时,父进程 `ralph run` 必须以非零退出码退出,而不是挂起。
- R4. 现有合法的 `ralph emit` 用法和 loop 流程必须保持不变。
- R5. CLI 入口拒绝的违规必须留下可观测痕迹(recovery envelope 或清晰 stderr),让 agent/用户知道 emit 为何失败。

---

## 范围边界

- **范围内**:CLI `emit` 配置加载、策略预检、loop 终止到父进程的信号传递。
- **范围内**:在 preset 中澄清 executor 的 U11 HARD RULES,降低 agent 发出畸形 payload 的概率。
- **范围外**:修改 task 系统「`work.done` 落地即 closed」的语义(该语义与 loop 终止解耦是设计上的)。
- **范围外**:对所有 `TerminationReason` 做超出父进程退出信号的大范围重构。
- **范围外**:在根目录 `ralph.yml` 中新增完整独立 `event_policy` 段(这是 P2 后续工作)。

### 延后到后续工作

- P2-3 细分恢复分级(STRING vs 缺字段):待遥测确认频率后再做。
- P3-1 更新 `docs/guide/runtime-diagnosis.md`:代码改动落地后再补文档。

---

## 背景与研究

### 相关代码与模式

- `crates/ralph-cli/src/cli/config_loader.rs` — `load_config_with_overrides`(CLI 路径,不合并 hats overlay)。
- `crates/ralph-cli/src/preflight.rs` — `load_config_for_preflight` 和 `merge_hats_overlay`(loop 路径,合并 preset overlay)。
- `crates/ralph-cli/src/policy_check.rs` — `resolve_policy_check_mode`、`should_policy_check_emit`。
- `crates/ralph-cli/src/commands/emit.rs` — `emit_command`、预检分支、JSONL 写入。
- `crates/ralph-cli/src/loop_runner/runner.rs` — `TerminationReason::PayloadContractViolation` 处理、`handle_termination`。
- `crates/ralph-cli/src/commands/run.rs` — stdio/TUI 父进程循环。
- `crates/ralph-core/src/event_policy.rs` — `validate_event`、payload type mismatch 判定。
- `crates/ralph-cli/tests/integration_preflight.rs` — 现有集成测试模式,使用 `CARGO_BIN_EXE_ralph`。
- `crates/ralph-cli/tests/integration_events_isolation.rs` — 现有事件文件集成测试。

### 机构知识

- `docs/solutions/developer-experience/ralph-cli-loop-runner-tests-must-run-serial.md` — `ralph-cli` 测试必须通过 nextest 串行跑,因为有 process-global mutex 和时间敏感测试。
- `AGENTS.md` / `CLAUDE.md` — 测试入口必须是 `cargo nextest run`;默认并行,`ralph-cli` 除外。

---

## 关键技术决策

- **`ralph emit` 复用 `load_config_for_preflight`**:与其扩展 `load_config_with_overrides` 让它理解 hats,不如让 `emit_command` 调用 `ralph run` / `ralph preflight` 使用的同一个异步 loader。这样保证配置一致性,也避免维护两套合并逻辑。
- **保持 emit 命令对用户的同步体感**:`emit_command` 目前是同步函数。如果切到异步 loader,可以只在配置加载处用临时 `tokio::runtime::Runtime` block,尽量减少函数签名改动。
- **通过 `loop.fatal_termination` 事件 + 父进程监听实现退出**:不在 `runner.rs` 里到处 `process::exit`,而是在终止时发一个内部 envelope/事件,让 stdio/TUI 父进程循环轮询或接收它,然后以约定好的非零码退出。这样 RPC 和 stdio 路径一致,也便于测试。
- **不改变 task closed 语义**:task 系统听到 `work.done` 就关任务是有意设计;bug 在于坏事件根本不该落到事件文件里。

---

## 待决问题

### 规划阶段已解决

- **Q**:`ralph emit` 应该支持 `-H builtin:...`,还是从 `ralph.yml` 推断 hat collection?  
  **A**:`-H` 已经是 `main.rs` 里的全局参数;`emit_command` 目前忽略它。修复时应把 `hats_source` 传入 `emit_command`,行为与 `ralph run` 一致。
- **Q**:父进程退出应该在 loop runner 子进程里做,还是在 stdio/TUI 父进程里做?  
  **A**:在父进程里做,子进程已经返回 `Ok(reason)`;我们新增一个可靠通道把 reason 传给父进程,父进程再干净退出。

### 实现阶段已决定

- **U3 父进程退出码**(见 `crates/ralph-cli/src/commands/run.rs::parent_exit_code_for_reason`):
  - `0`: 干净完成(`CompletionPromise`、`Cancelled`)
  - `1`: 通用失败(`ConsecutiveFailures`、`LoopThrashing`、`LoopStale`、`ValidationFailure`、`Stopped`、`WorkspaceGone`、`RecoveryExhausted`、`ReviewFailed` 等)
  - `2`: `PayloadContractViolation`
  - `3`: `MaxIterations`
  - `4`: `MaxRuntime`
  - `5`: `MaxCost`
  - `6`: `RestartRequested`
  - `130`: `Interrupted`
- **U3 通知机制**: loop runner 在返回非成功 `TerminationReason` 前向 `.ralph/loop-termination-reason.json` 写入精确终止原因;`commands/run.rs` 在子进程退出后读取该哨兵文件,并据此以约定非零码退出。TUI/RPC 路径 additionally 通过并发等待子进程退出并发送 `terminated` 信号,让 TUI 立即退出而不是等待用户按 `q`。
- **U3 测试**: 扩展 `crates/ralph-cli/tests/integration_run.rs`,新增 `test_run_payload_contract_violation_exits_with_reason_code`(stdio 路径)和 `test_run_rpc_payload_contract_violation_exits_with_reason_code`(RPC 子进程路径)。

---

## 实现单元

- [x] U1. **把 `hats_source` 传入 `emit_command` 并加载合并后的配置**

**目标:**让 `ralph emit` 加载与 loop runner 相同的有效配置,包括 preset 的 `event_policy`。

**需求:**R1、R2

**依赖:**无

**文件:**
- 修改:`crates/ralph-cli/src/main.rs`
- 修改:`crates/ralph-cli/src/commands/emit.rs`
- 修改:`crates/ralph-cli/src/cli/config_loader.rs`(可选辅助函数)
- 测试:`crates/ralph-cli/tests/integration_events_isolation.rs` 或新建 `crates/ralph-cli/tests/integration_emit_policy.rs`

**方案:**
- 从 `main.rs` 把 `hats_source: Option<&HatsSource>` 传入 `commands::emit::emit_command`。
- 在 `emit_command` 内,把当前调用的 `load_config_with_overrides` 替换为 `load_config_for_preflight`(或等价的、会合并 hats overlay 的 helper),确保 `event_policy` 被合并。
- 解析出的合并配置用于 `resolve_policy_check_mode` 和 `validate_event`。

**遵循模式:**
- `crates/ralph-cli/src/preflight.rs` 中的 `load_config_for_preflight`
- `crates/ralph-cli/src/main.rs` 中 `ralph run` 对 `hats_source` 的解析与分发

**测试场景:**
- **Happy path**: `ralph -H builtin:ce-executor-isolated emit work.done --json '{"plan_name":"x","plan_path":"y","task_id":"z","task_key":"k","step":"s","commit_count":1,"changed_lines":10}' --hat executor` 成功,事件进入事件文件。
- **Error path**: `ralph -H builtin:ce-executor-isolated emit work.done --payload "free text" --hat executor` 以非零退出,不写入受信任事件文件,stderr 明确提示 `payload_type_mismatch`。
- **Error path(无 `-H`)**:未激活 preset 且 `ralph.yml` 没有 `event_policy` 时,`ralph emit` 保持宽松兼容,但记录 info 级日志说明策略检查被跳过。
- **Integration**:`ralph run` 和 `ralph emit` 使用相同 `--hat-collection` 时,`resolve_policy_check_mode` 结果一致。

**验收:**
- `cargo nextest run -p ralph-cli --bin ralph -- emit` 通过。
- 手动跑上述两条 error path 命令,退出码符合预期。

---

- [x] U2. **CLI 入口拒绝时写入 recovery envelope**

**目标:**`ralph emit` 在 CLI 入口拒绝事件时留下审计痕迹。

**需求:**R5

**依赖:**U1

**文件:**
- 修改:`crates/ralph-cli/src/commands/emit.rs`
- 测试:与 U1 相同的集成测试

**方案:**
- 当 `emit_command` 中 `validate_event` 返回 findings 时,向 `recovery.jsonl` 写入一条 envelope,`source` 为 `cli_emit`,根据违规类型设 `severity` 为 `critical` 或 `warning`,`payload type mismatch` 等不可重试场景设 `outcome` 为 `not_retriable`。
- 如果 `crates/ralph-core/src/diagnosis/` 已提供 recovery writer,直接复用;否则在 `commands/emit.rs` 里写一个小 helper。

**遵循模式:**
- `crates/ralph-core/src/diagnosis/recovery_writer.rs`(若存在)或 `event_loop/mod.rs` 中写 `recovery.jsonl` 的方式

**测试场景:**
- **Integration**:STRING payload 被拒绝后,`recovery.jsonl` 中存在 `source: cli_emit` 且 `reason_code: payload_contract_violation` 的 envelope。
- **Edge case**:如果 recovery 写入本身失败,`ralph emit` 仍应以非零退出并打印原始校验错误到 stderr。

**验收:**
- 集成测试断言 recovery envelope 的存在与字段。

---

- [x] U3. **把 loop 终止信号传给 stdio/TUI 父进程**

**目标:**loop 异常终止时,父进程 `ralph run` 退出而不是挂起。

**需求:**R3

**依赖:**无(可与 U1/U2 并行)

**文件:**
- 修改:`crates/ralph-cli/src/loop_runner/runner.rs`
- 修改:`crates/ralph-cli/src/commands/run.rs`
- 测试:`crates/ralph-cli/tests/integration_run.rs` 或新建测试

**方案:**
- 在 `runner.rs` 中,对非成功 reason 返回 `Ok(TerminationReason::...)` 之前,向父进程发送一个轻量级终止通知。实现时可选:
  1. 写一个哨兵文件/行,父进程轮询;
  2. 在专用 channel 上发内部 `loop.fatal_termination` 事件;
  3. RPC 模式下确保 `LoopTerminated` RPC 事件始终发送,父进程将其视为硬退出信号。
- 在 `commands/run.rs` 中,子进程返回后检查终止 reason(或接收通知),当 reason 不是干净完成时调用 `std::process::exit(code)` 并以文档约定的非零码退出。
- 确保 TUI 路径和普通 stdio 路径都遵守该逻辑。

**遵循模式:**
- TUI 模式下现有的 `LoopTerminated` RPC 处理。
- `commands/run.rs` 中现有的子进程拉起逻辑。

**测试场景:**
- **Integration**:用会产生 `PayloadContractViolation` 的 fixture 跑 `ralph run`;断言 `ralph run` 进程在 10 秒内以 code 2 退出,不会挂起。
- **Happy path**:正常跑到 `LOOP_COMPLETE`;断言退出码 0。
- **Edge case**:TUI/RPC 模式下 (`--tui`) 发生致命终止;断言 TUI 父进程也退出。

**验收:**
- 报告中原本挂起的场景现在能在限定时间内以非零码退出。
- 现有 `integration_run.rs` 测试仍通过。

---

- [x] U4. **澄清 executor U11 HARD RULES 与 preset 一致性**

**目标:**降低 agent 发出畸形 `work.done` 或禁止的 `build.done` 事件的概率。

**需求:**R1(降低失败率)

**依赖:**U1

**文件:**
- 修改:`presets/en/ce-executor-isolated.yml`
- 测试:现有 preset lint / `ralph preset check`(若存在)

**方案:**
- 在 executor `instructions` 段增加一小节 "PAYLOAD SCHEMA CHECKLIST",列出 preset schema 中 `work.done` 真正的必填字段:`plan_name`、`plan_path`、`task_id`、`task_key`、`step`、`commit_count`、`changed_lines`。
- 提供一个可直接复制的 `--json` 示例命令。
- 消除 `build.done` 的歧义:要么删除 `topic_deny_rules` 中 `executor` → `build.done`(如果 U11 允许它作为自检),要么强化 instruction 明确禁止并保证 deny rule 生效。**决策**:保留 deny rule,instruction 明确禁止。

**遵循模式:**
- 同 preset 中现有的 U11 HARD RULES 块。

**测试场景:**
- **Happy path**: `ralph preset check builtin:ce-executor-isolated` 通过。
- **Dogfood**:grep executor instructions,确认七个 `work.done` 必填字段名都在。

**验收:**
- `ralph preset check` 通过;dogfood grep 通过。

---

- [x] U5. **为 CLI 策略检查跳过时增加 info 级日志**

**目标:**让未来排查「CLI 预检被跳过」更容易。

**需求:**R5

**依赖:**U1

**文件:**
- 修改:`crates/ralph-cli/src/commands/emit.rs`
- 测试:在现有或新建集成测试中断言日志

**方案:**
- 当 `resolve_policy_check_mode` 返回 `Skip` 时,输出一行 `tracing::info!("cli emit policy check skipped: no event_policy in resolved config")`。

**测试场景:**
- **Edge case**:无 preset 且无 `event_policy` 时跑 `ralph emit`,验证 info 日志出现(通过 `RALPH_LOG` 或捕获 stderr)。

**验收:**
- 测试通过。

---

## 系统级影响

- **交互图:**`ralph emit` 现在依赖 `HatsSource` 和 preflight 配置加载器,增加了 emit 命令与 preflight 之间的耦合。这是可接受的,因为复用了现有合并路径而非新建一套。
- **错误传播:**CLI emit 拒绝现在表现为进程退出码 ≠ 0 加一条 recovery envelope。Agent loop 必须处理 `ralph emit` 失败,不能假设一定成功。
- **状态生命周期风险:**如果 `ralph emit` 在 partially 写入文件后崩溃(校验和写入之间),事件不能落地。保持"先校验、后追加"的原子顺序。
- **API 表面对齐:**其他会发出或校验事件的 CLI 子命令(如 `ralph wave emit`、未来命令)也应审计是否存在同样的 config loader bug。本计划只聚焦 `ralph emit`。
- **明确不变量:**task 系统仍在 `work.done` 时关任务;loop 仍因 payload contract violation 终止;preset schema 仍由 YAML 定义。

---

## 风险与依赖

| 风险 | 缓解 |
|---|---|
| 修改 `emit_command` 签名破坏其他调用方 | `emit_command` 只在 `main.rs` 调用;改动局部化并更新调用点。 |
| 让 `emit_command` 异步带来复杂度蔓延 | 只在配置加载处用临时 runtime block,命令函数保持同步。 |
| 父进程退出改动导致 TUI/RPC 回归 | 为 stdio 和 TUI 终止路径都加集成测试。 |
| Preset instruction 改动让 agent 困惑 | 以 checklist/示例形式新增,不删除现有 U11 文字。 |

---

## 文档/运维说明

- 后续在 `docs/guide/runtime-diagnosis.md` 增加「payload contract violation 后父进程未退出」症状条目(P3-1 后续)。
- 选定退出码后,在 `docs/api/cli-exit-codes.md` 或合适位置文档化。

---

## 来源与参考

- **原始报告:**[docs/report/2026-06-14-ce-executor-isolated-task-closed-but-loop-running-diagnosis.md](../report/2026-06-14-ce-executor-isolated-task-closed-but-loop-running-diagnosis.md)
- 相关代码:`crates/ralph-cli/src/cli/config_loader.rs`、`crates/ralph-cli/src/preflight.rs`、`crates/ralph-cli/src/commands/emit.rs`、`crates/ralph-cli/src/loop_runner/runner.rs`、`crates/ralph-cli/src/commands/run.rs`、`presets/en/ce-executor-isolated.yml`
- 相关前期计划:`docs/plans/2026-06-13-004-fix-wave-result-hat-attribution-plan.md`
