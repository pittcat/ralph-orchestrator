---
title: "feat: 通用可信 Checkpoint 与 Worktree Continuation"
type: feat
date: 2026-09-01
updated: 2026-09-02
deepened: 2026-09-02
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
product_contract_source: ce-plan-bootstrap
baseline_branch: pittcat-dev
baseline_commit: 4884103905c77e115722ccef0b393e90080aee74
product_contract_preservation: "Product Contract unchanged — R1-R12 与 2026-09-01 原文一致"
---

# 通用可信 Checkpoint 与 Worktree Continuation 开发计划

本次更新：按 HEAD `48841039` 重基线；产品目标 R1–R12 不变。删除旧计划把 U5 排在 U4 之后的人为串行（无因果依赖）。U1 禁止改 `accepted_transition.rs`（已有公开 `read_outbox`）。`loop_runner/inner.rs` 已超 5000 行，禁止再堆生产逻辑。

## 0. 计划状态

- **状态：READY。** 全部实施关键决策 ≥ 0.85。
- **基线：** `pittcat-dev` @ `4884103905c77e115722ccef0b393e90080aee74`（`0a89cfdc` 之后：plan 提交 `721b8c61` + precheck/flow 修复 `48841039`）。调查时工作树相对该 HEAD 无本功能生产改动。
- **调查范围：** `ralph run` 的 `--continue` / `--worktree` / `--reuse-worktree` / hidden `--worktree-path`；scratchpad 预检顺序；reuse cleanup 与 PF manifest 父/子进程；`LoopLock`；`LoopHistory::is_completed`；`read_outbox`；prompt `store.load()`；`commands/resume.rs` 命名冲突；`inner.rs` 行数；既有集成测试与 CLI 文档；Git diff vs 旧基线。
- **已执行验证：** 只读源码/测试/文档检索、行数、`git log`/`git show`。计划阶段未跑构建或测试（遵循 `ce-plan`）。
- **尚未执行：** 任何 Red/Green、nextest、clippy、build、CLI smoke、doc-drift、全量测试。
- **阻塞项：** 无。若实施中发现 `LoopHistory` 成功终态不能由 `LoopCompleted{reason=completion_promise}` 唯一识别，或 combined TUI 无法在**单一层**持有目标 worktree `LoopLock` 贯穿 runner/child wait，立即按停止条件重决策。
- **与失败 forge 诊断的关系：** `docs/report/2026-09-02-parallel-forge-2026-09-01-2102-feat-trusted-worktree-continuation-plan-diagnosis.md` 记录的是用 `parallel-forge` **执行本计划** 时 worktree_setup 被 `flow_unknown_emit` 阻断，不是本功能产品缺陷。`48841039` 修的是 precheck desugar / flow scope，不改变 `--continue`+`--reuse-worktree` 语义。不得把该诊断当成 R1–R12 变更。

---

## 1. 功能目标

### 1.1 业务目标

让 operator 用同一个 Git worktree、同一个 logical loop 和现有 durable runtime state，在进程断开、崩溃或人工中断后安全续跑：

```text
ralph run --worktree --reuse-worktree --continue \
  --plan <same-plan> [existing run options]
```

checkpoint 是可验证的持久化执行边界集合，不是 LLM 隐藏上下文。由 loop identity、current-events、history、accepted-transition outbox、StateLedger、task/progress/scratchpad 等现有磁盘状态构成。恢复时先资格审计，再走现有 `--continue` 冷启动 repair/replay。

### 1.2 用户或调用方

- 直接调用 `ralph run` 的 human operator。
- TUI parent / RPC child 组合启动路径。
- 依赖同一 worktree 中 task、event、StateLedger、policy 连续性的所有 preset（不限定 Parallel Forge）。
- 受 memory 自动注入影响的 isolated/coordinator hats。

### 1.3 当前行为（HEAD 已确认）

- CLI 同时接受 `--continue` 与 `--worktree --reuse-worktree`（无 clap conflict）。
- `--continue` 在 worktree 选出之前，用主 workspace 的 `config.core.scratchpad.path` 做存在性检查。
- `--reuse-worktree` 命中已有 worktree 后**总是**调用 `clean_worktree_runtime_artifacts`，再跑 PF manifest gate。
- hidden `--worktree-path` 子进程在 `resume` 进入 inner 之前，若 preset 名字符串为 `parallel-forge`，会 `latest_archived_manifest` + `validate_manifest`；combined 若父进程已 cleanup，子进程仍可能被旧 archive 拒绝。
- worktree 模式与 `--worktree-path` 子进程当前**都不**获取 `LoopLock`（`_lock_guard = None`）。
- runner `resume=true` 忽略 resume manifest，RPC `loop_bootstrap` 在 `resume=true` 时返回 `Continue`。
- `LoopHistory::is_completed()` 对任意 `LoopCompleted` 返回 true（含 max_iterations/failure）。
- prompt 自动注入调用 `MarkdownMemoryStore::load()`，不走 `load_visible`。

### 1.4 目标行为与差异

- 启动意图显式为 Fresh / ContinuePrimary / ReuseFresh / ContinueReusedWorktree，互斥穷尽。
- `ContinueReusedWorktree` 必须绑定已存在且 Git 登记的 exact-name worktree；找不到则拒绝，**不得** first-create。
- 在**目标 worktree 路径**上取得独占 `LoopLock`，审计完成前不得归档/删除 live artifact。
- checkpoint 校验：loop-id 一致、current-events 指向存在文件、scratchpad 存在、history 可读、outbox 可读（或 NotFound 视为空）；真实 I/O 失败 fail-closed。
- 最近有效 history 终态为 `LoopCompleted(reason=completion_promise)` 则拒绝 combined continue，提示去掉 `--continue` 做 fresh reuse。
- 非成功 `LoopCompleted`（max_iterations / max_runtime / failure 等）与 `LoopTerminated` / 无终态允许继续。
- 资格通过后不 cleanup、不旋转 current-events、不换 logical loop id；进入既有 Continue 路径，发一次 `loop.resume`。
- 重启 repair 保持 at-most-once materialization/publish。
- standalone continue / standalone reuse（含 PF manifest）/ fresh worktree 不变。
- 自动 memory 注入仅为 shared + 当前 hat own-private。

### 1.5 Requirements

- **R1** 四种启动意图互斥穷尽；combined 不得落入 fresh reuse cleanup。
- **R2** combined 绑定 exact worktree 与该 worktree `current-loop-id`；不存在 / Git 未登记 / 显式 loop-id 不一致 → 非零退出、零持久业务副作用。
- **R3** combined 在读 checkpoint 前取得目标 worktree 独占锁，持有到 runner 或 TUI child wait 结束；第二进程拒绝。
- **R4** 审计通过前不得 `clean_worktree_runtime_artifacts`，不得改 current-events/history/tasks/scratchpad/outbox；失败不产生 reuse-history archive。
- **R5** 仅 `completion_promise` 视为可信成功终态。
- **R6** 原地续跑：同 path、同 loop id、同 current-events；一次 `loop.resume`；不重发 starting event。
- **R7** 冷启动先修 outbox-only StateLedger projection；重复恢复不重复副作用。
- **R8** R1–R7 不按 preset 名分支；PF manifest 只服务 standalone ReuseFresh。
- **R9** standalone `--continue`、`--reuse-worktree`、fresh worktree、RPC/TUI/no-TUI 既有测试断言不削弱。
- **R10** auto-injected memory 使用当前 hat 可见视图；budget 在过滤后应用。
- **R11** 无新持久格式、无新 crate、无定时 snapshot、无 backend session restore。
- **R12** combined flags 只更新 clap help 与 operator docs；不写入 agent-injected skill。memory skill 若已准确则不改文案。

### 1.6 输入、输出、状态、错误

- **输入：** resolved `RunArgs`、exact worktree name、Git worktree list、loop registry PID、目标 worktree `LoopLock`、`.ralph/current-loop-id`、`.ralph/current-events` 及其目标、`.ralph/history.jsonl`、`.ralph/agent/scratchpad.md`、outbox、可选 StateLedger/task/progress。
- **输出：** typed `RunIntent`；只读 assessment（Eligible / AlreadyCompleted / Refused）；成功则既有 `LoopBootstrap::Continue` + 一条 `loop.resume`；失败则稳定 CLI 错误。
- **状态：** 审计前仅允许 lock 文件瞬态；成功后由既有 continue runner 追加 history/resume。不得创建 reuse archive。
- **错误：** active/locked、missing target、identity mismatch、mandatory artifact 缺失、真实 I/O、credible completion 均 fail-closed。消息含目标 worktree 与下一步，不泄漏 memory 正文。
- **幂等：** assessment 无业务写；并发靠 lock；transition 靠既有 identity 去重。

### 1.7 兼容、性能、安全、约束

- **兼容：** 不改 CLI 参数名；不迁移数据。combined 是此前错误组合。
- **性能：** 启动时线性读 history/outbox/marker；不遍历 worktree 全树。
- **安全：** 只操作 exact-name + Git-known 路径；private memory 不跨 hat。
- **文件规模：** `commands/run.rs` = 4395 行；`loop_runner/inner.rs` = 5100 行（已超硬上限）。新增逻辑进新模块；禁止向 `inner.rs` 增加生产代码。
- **测试：** `cargo nextest run` / `./scripts/run-tests.sh`；禁止裸 `cargo test -p ralph-cli`。

### 1.8 范围与非目标

**范围：** R1–R12。

**非目标：** 不恢复 LLM/PTY/网络；无 `ralph checkpoint` 命令；不泛化/删除 PF manifest；不改 preset/schema；不改 task.resume 的 agent 语义；不做 memory 排名/DB；不自动 merge/删除 worktree。

**Deferred：** checkpoint inspect 子命令、backend session resume、memory ranking、定期 compact snapshot。

### 1.9 事实 / 假设 / 决策

**已确认事实：** E1–E34。

**已确认假设：**

- A1：有可信 `LOOP_COMPLETE`（history `completion_promise`）则 combined 拒绝并提示去掉 `--continue`（session-settled）。
- A2：combined 是同一 logical loop 原地继续，不是 reuse 后新 lineage（session-settled）。
- A3：只恢复持久边界，不恢复隐藏模型上下文。

**待验证假设：** 无实施阻塞项。Red 的具体 stderr 字面量属执行证据。

---

## 2. 代码库现状与证据

### 2.1 当前实现入口与调用链

```text
ralph run / RunArgs
  → commands/run.rs
      scratchpad exists check  (resume=true，主 workspace 路径)
      resolve_exact_worktree_name(--worktree-name | --plan basename)
  → if worktree && reuse_worktree
      → find_reusable_worktree_by_name
      → clean_worktree_runtime_artifacts          # 当前总会执行
      → optional PF ResumeManifest gate
      → LoopContext::worktree；worktree 路径不持 LoopLock
  → else if worktree_path (TUI/RPC child)
      → LoopContext::worktree，无 LoopLock
      → if preset_name == "parallel-forge": latest_archived_manifest + validate
  → TUI parent spawn 或 run_loop_impl
      → resolve_loop_id（worktree 用 context id）
      → EventLoop 冷启动：read current-events → StateLedger → repair outbox → hydrate
      → resume ? initialize_resume : initialize
      → LoopBootstrap::{Continue, ManifestResume, Fresh}
```

数据权威：

```text
accepted Business/Recovery → AcceptedTransition outbox → materialize → StateLedger → EventBus
accepted LOOP_COMPLETE (LoopControl) → TerminationReason::CompletionPromise
  → LoopHistory::LoopCompleted(reason="completion_promise")
```

`LOOP_COMPLETE` 不进 outbox。成功判定用 history；业务 replay 用 outbox/ledger。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `commands/run.rs` `RunArgs.continue_mode` / `reuse_worktree` | 无 clap conflict，可同时解析 | 必须定义组合语义，不能靠禁止组合 | 高 |
| E2 | `run.rs` reuse 分支约 L1074–1136 | 命中 reusable 后总是 `clean_worktree_runtime_artifacts` | combined 必须在此之前分叉 | 高 |
| E3 | `worktree.rs::clean_worktree_runtime_artifacts` | 归档 events/history/current-loop-id/scratchpad/tasks 等 | 这些是 continue 的 live checkpoint | 高 |
| E4 | `loop_runner/rpc_bootstrap.rs::loop_bootstrap` | `resume=true` → `Continue`；测试 `continue_takes_precedence_over_reuse_manifest` | combined 继续用 Continue | 高 |
| E5 | `inner.rs` L594–599 | `resume=true` 时忽略 manifest | 两条协议：ReuseFresh vs Continue | 高 |
| E6 | `run.rs` L840–855 | scratchpad 检查在 worktree 选择前，路径来自主 config | combined 的 mandatory 检查必须在目标 worktree 上 | 高 |
| E7 | `find_reusable_worktree_by_name` | exact path ∩ Git worktree list；registry 活 PID 拒绝；无 registry 仍可复用 Git-known 目录 | 定位复用此函数；identity 另验 marker | 高 |
| E8 | `loop_registry.rs` | LoopEntry 无 completion status | 成功不能看 dead PID | 高 |
| E9 | `loop_lock.rs` `LOCK_FILE=".ralph/loop.lock"`；`try_acquire(workspace_root)` | 锁锚定传入根目录 | combined 传入**目标 worktree path** | 高 |
| E10 | `loop_history.rs::is_completed` | 任意 `LoopCompleted` 为 true | 禁止直接用该 API 做 R5 | 高 |
| E11 | `inner.rs` termination bookkeeping | Interrupted → `LoopTerminated`；max/failure → `LoopCompleted(reason_str)` | 必须按 reason 精确分类 | 高 |
| E12 | `event_loop/disposition.rs` | LoopControl 不走 AcceptedTransition | outbox 不能当成功权威 | 高 |
| E13 | `accepted_transition.rs::read_outbox` **已 `pub`** | NotFound→空 Vec；torn line salvage；真实 I/O `Err` | U1 只调用此 API，不改 commit/salvage | 高 |
| E14 | `acceptance_and_lifecycle.rs` | 冷启动 repair 后 hydrate | 成功 combined 复用此路径 | 高 |
| E15 | `accepted_transition.rs` 既有 repair 测试 | outbox/ledger split、重复 repair、rollback | U4 以回归/差分补 CLI 缺口，不重做事务 | 高 |
| E16 | `runner.rs::resolve_loop_id` | worktree 优先 context id；primary continue 才读 marker | combined context id 必须与 marker 一致 | 高 |
| E17 | `integration_resume.rs::test_continue_publishes_loop_resume_event` | continue 保留 marker、写 `loop.resume` | combined happy 复用同断言 | 高 |
| E18 | `loop_runner/tests/legacy/recovery.rs::u5_resume_branch_does_not_re_inject_work_start` | resume 不旋转 marker、不追加 starting event | combined 复用，不复制引擎 | 高 |
| E19 | `integration_worktree_isolation.rs` | standalone reuse archive/create/PID/manifest 真实 binary | R9 回归；不得削弱断言 | 高 |
| E20 | `parallel_forge_resume.rs` + achieved `2026-08-03-004` | PF 专用 fresh-reuse 协议 | combined 不消费 manifest | 高 |
| E21 | `memory_store.rs::load_visible` vs `prompt_injection.rs` L531 `store.load()` | 可见性 API 已存在；注入未用；`hat_id.as_str()` 已在同函数后段使用 | 最小修复 `load_visible(Some(hat_id.as_str()))` | 高 |
| E22 | `AGENTS.md`、`.config/nextest.toml`、`scripts/run-tests.sh`、`mise.toml` | nextest 0.9.140；两阶段全量；禁止裸 cargo test | 命令与门禁固定 | 高 |
| E23 | `run.rs` child `--worktree-path` L1350–1427 | 子进程无条件对 `parallel-forge` 再验 archived manifest | U3 必须按 RunIntent 跳过 ReuseFresh-only gate | 高 |
| E24 | `wc -l loop_runner/inner.rs` = 5100 | 已超过 5000 行硬上限 | 禁止向 inner.rs 增加生产代码；Red 若要求改 inner → STOP 拆分另议 | 高 |
| E25 | `commands/resume.rs` + `mod resume` | 已有废弃 `ralph resume` 子命令 | 新模块不得命名 `resume.rs`；用 `run_recovery.rs` | 高 |
| E26 | child 用 `child_preset_name == "parallel-forge"`；parent 用 `uses_parallel_forge_resume_manifest` | 判定入口不一致 | combined 跳过必须以 RunIntent 为准，不能只改 inner | 高 |
| E27 | `git show 48841039` | 改 precheck/flow/scenarios + 诊断报告；未改 run/continue | 重基线不改变本功能缺口 | 高 |
| E28 | `docs/report/2026-09-02-...-diagnosis.md` | forge 执行本计划在 worktree_setup fail-close | 不修改 R1–R12 | 高 |
| E29 | `lib.rs` `recovery_intent` / `recovery_runtime` | 名称已占用相邻概念 | 新模块名 `recovery_checkpoint` | 高 |
| E30 | `event_loop/tests/mod.rs` | 新测试文件必须在此 `mod` 注册才被 nextest 收集 | 仅 U5 新增 `memory_visibility` 模块行；U1 测试放 `recovery_checkpoint.rs` 内 `#[cfg(test)]` | 高 |
| E31 | `LoopLock::try_acquire` | `{workspace_root}/.ralph/loop.lock` | 传入 worktree path 即锁该 worktree，不锁主仓 | 高 |
| E32 | `run.rs` worktree 与 worktree_path 分支返回 `(ctx, None)` | 当前 worktree 路径无锁 | combined 新增锁；TUI 只能 parent 持锁，child 禁止再 acquire 同一锁 | 高 |
| E33 | `run.rs` = 4395 行 | 接近 5000 | 编排逻辑进 `run_recovery.rs`，run.rs 只薄接线 | 高 |
| E34 | `docs/guide/cli-reference.md` L98–108 | `--continue` 与 `--reuse-worktree` 分行，无组合语义 | U3 更新 operator 文档 | 高 |
| E35 | `ralph-tools-memories.md` Visibility 段 | CLI list/show 已写 hat 可见性；未声称 auto-inject 过滤 | U5 原则上不改 skill；若反查发现注入语义缺失，只补一句「自动注入与 list 同一可见性」 | 高 |
| E36 | `loop_context.rs` | worktree memories 符号链接到主仓 | U5 是 hat 可见性而非 worktree 隔离；与 combined 无写冲突 | 高 |
| E37 | `resolve_exact_worktree_name` + 既有 unit tests L3218+ | exact name 已有 SSOT | U2 复用，不重写命名规则 | 高 |
| E38 | `commands/mod.rs` | 无 `run_recovery` 模块 | 计划新增路径 | 高 |

### 2.3 受影响范围

**生产：** `recovery_checkpoint.rs`（新增）、`lib.rs`（注册）、`loop_history.rs`（精确终态只读，可选）、`commands/run_recovery.rs`（新增）、`commands/mod.rs`、`commands/run.rs`（薄接线）、`prompt_injection.rs`。`accepted_transition.rs` **生产代码默认不改**。`inner.rs` **生产代码禁止改**。

**测试：** 新模块内测；`integration_resume.rs`；`integration_worktree_isolation.rs`；`rpc_bootstrap.rs` 既有；legacy recovery；`event_loop/tests/memory_visibility.rs`（新增）+ `tests/mod.rs` 一行；`integration_memory.rs` 回归；`accepted_transition` / `state_machine` 既有。

**文档：** `docs/guide/cli-reference.md`；必要时 `docs/guide/index.md`。不改 presets/schemas/manifest/zsh/agent-injected operator 控制面。

**不受影响：** HTTP API、Web UI、数据库 schema、网络服务、event topology。

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | combined 语义 | fresh reuse；原地 continue；backend session | **原地 continue** | E2–E6,E16–E20,A2 | fresh 清状态；session 不可通用验证 | 0.97 |
| D2 | 模式表达 | bool 顺序；clap 禁止组合；typed RunIntent | **四态 RunIntent** | E1–E6 | 禁止组合违背需求；bool 即缺陷 | 0.95 |
| D3 | checkpoint 形态 | 新文件；PF manifest；typed 只读 view | **只读 assessment** | E3,E10–E15,E20 | 双写漂移；manifest 非通用 | 0.93 |
| D4 | 成功权威 | PID；event 字符串；outbox；history reason | **最近有效 `LoopCompleted(completion_promise)`** | E8,E10–E12 | PID≠成功；LOOP_COMPLETE 不在 outbox | 0.91 |
| D5 | 业务恢复 | scratchpad；复制 JSONL；现有 repair | **现有 cold-start** | E12–E18 | scratchpad 不可信；复制会重复副作用 | 0.96 |
| D6 | 是否 cleanup | cleanup+restore；部分 archive；完全跳过 | **完全跳过 cleanup** | E2–E6,E17–E18 | restore 新事务；部分 archive 破坏一致性 | 0.96 |
| D7 | 无目标 worktree | 创建；回退 primary；拒绝 | **拒绝** | E7,A2 | 创建/回退不是续同一 loop | 0.94 |
| D8 | 并发 | 仅 PID；registry lock；目标 LoopLock | **目标 worktree LoopLock + 既有 PID 先验**；TUI 仅 parent 持锁 | E7–E9,E31–E32 | PID TOCTOU；child 再 lock 会死锁 | 0.91 |
| D9 | identity mismatch | warning；覆盖 marker；fail-closed | **fail-closed** | E3,E13,E16 | 覆盖导致归属漂移 | 0.93 |
| D10 | torn/I/O | 全拒绝；沿用 salvage；静默 | **调用现有 `read_outbox`；assessment 不改 parser** | E13–E15 | 全拒绝破坏 crash 恢复 | 0.92 |
| D11 | preset | 仅 PF；逐 preset；agnostic | **agnostic** | E14–E20 | runtime 边界已通用 | 0.92 |
| D12 | RPC/TUI | 新 variant；ManifestResume；Continue | **Continue**；parent 目标+锁；child 按 RunIntent 跳过 PF archive gate | E4–E5,E23,E26 | 新 variant 无新 agent 行为 | 0.92 |
| D13 | memory 层 | CLI；字符串删；`load_visible` | **`load_visible(Some(hat_id.as_str()))`** | E21,E36 | CLI 不覆盖注入；后过滤破坏 budget | 0.98 |
| D14 | 文档 | agent skill；operator docs；都写 | **help + cli-reference**；memory skill 仅反查 | E21,E22,E34,E35,R12 | 禁止 operator 控制面注入 agent | 0.95 |
| D15 | 模块 | 堆 run.rs；纯 core；core+CLI 两模块 | **`recovery_checkpoint` + `run_recovery.rs`** | E24,E25,E29,E33 | run.rs 接近上限；resume.rs 已占用 | 0.94 |
| D16 | U1 是否改 outbox 生产代码 | 加 helper；直接 `read_outbox` | **直接 `read_outbox`，默认零改 accepted_transition.rs** | E13 | 与 U4 测试写冲突且无新语义 | 0.93 |
| D17 | U5 与恢复链 | 串行「先恢复再隐私」；基线并发 | **U1∥U5 从同一基线并发** | E21,E30,E36 | 串行仅为归因习惯，无因果 | 0.96 |
| D18 | inner.rs | 最小补丁；禁止增量 | **禁止向 inner.rs 增加生产代码** | E5,E18,E24 | 已超 5000；resume 语义已足够 | 0.94 |

无低于 0.85 的关键决策。

### 3.1 High-Level Technical Design

| `--continue` | `--worktree` | `--reuse-worktree` | RunIntent | 目标不存在 | Runtime |
|---|---|---|---|---|---|
| 否 | 否 | 否 | Fresh | n/a | fresh |
| 是 | 否 | 否 | ContinuePrimary | 既有 primary 规则 | 原地 |
| 否 | 是 | 是 | ReuseFresh | **创建** first exact | 归档 + PF manifest |
| 是 | 是 | 是 | ContinueReusedWorktree | **拒绝，不创建** | **原地，不消费 manifest** |

`--worktree` 而无 `--reuse-worktree` 保持现有 fresh worktree（非本计划四态表核心，不得改其语义）。

```mermaid
sequenceDiagram
    actor O as Operator
    participant C as Run command
    participant W as Target worktree
    participant G as Checkpoint gate
    participant R as Existing continue runner

    O->>C: --worktree --reuse-worktree --continue
    C->>W: exact-name + Git-known + dead-PID
    C->>W: acquire LoopLock on worktree path
    C->>G: identity, current-events, scratchpad, history, outbox
    alt completion_promise
        G-->>C: AlreadyCompleted
        C-->>O: reject; remove --continue
    else missing/mismatch/I/O
        G-->>C: Refused
        C-->>O: fail closed
    else eligible
        G-->>C: Eligible
        C->>R: Continue bootstrap
        R->>R: repair then hydrate
        R->>W: one loop.resume
    end
```

任何实现若把 archive 再拷回 live path，偏离 D3/D6，必须停止。

---

## 4. BDD 行为规格

```gherkin
Feature: 在复用 Git worktree 中可信地继续同一个 Ralph loop

  Background:
    Given operator 提供 --worktree --reuse-worktree --continue
    And worktree 名称由 --plan 或 --worktree-name 精确解析

  Scenario S1: 中断 loop 在原 worktree 原地继续
    Given exact-name worktree 已由 Git 登记且旧 PID 不存活
    And current-loop-id、current-events、event file、scratchpad 与 history 可读
    And 最近终态不是 completion_promise
    When operator 启动 combined continuation
    Then Ralph 复用同一 worktree 和 loop id
    And current-events marker 不旋转
    And 追加一次 loop.resume
    And 不追加 fresh starting event
    And 不创建 reuse-history archive

  Scenario S2: 已接受 LOOP_COMPLETE 的 loop 拒绝继续
    Given 最近有效 history 终态是 LoopCompleted reason completion_promise
    When operator 启动 combined continuation
    Then 命令非零退出
    And 错误提示移除 --continue 以执行 fresh reuse
    And events、history、tasks、scratchpad、outbox 均不变
    And 不创建 reuse-history archive

  Scenario S3: 非成功终止仍可继续
    Given 最近终态是 LoopTerminated 或 LoopCompleted reason max_iterations/max_runtime/failure
    When operator 启动 combined continuation
    Then 不把它误判为 credible LOOP_COMPLETE
    And 进入与 S1 相同的 continue bootstrap

  Scenario S4: combined mode 找不到既有 worktree
    Given exact-name worktree 不存在
    When operator 启动 combined continuation
    Then 命令非零退出
    And 不创建新的 worktree、loop id 或 runtime artifact

  Scenario S5: checkpoint identity 不一致
    Given worktree 名称或显式 --loop-id 与 current-loop-id 不一致
    When operator 启动 combined continuation
    Then 命令非零退出并报告 identity mismatch
    And 不覆盖 current-loop-id
    And 不归档任何 live artifact

  Scenario S6: mandatory checkpoint artifact 缺失或不可读
    Given current-events 目标、scratchpad 缺失，或 history/outbox 真实 I/O 错误
    When operator 启动 combined continuation
    Then 命令 fail-closed
    And 不启动 backend、不创建 archive、不修改业务状态

  Scenario S7: 两个进程竞争同一 checkpoint
    Given 第一个 combined continuation 已持有目标 worktree loop lock
    When 第二个进程尝试相同命令
    Then 第二个进程在读取或修改 checkpoint 前被拒绝
    And 只有第一个进程可追加 resume 状态

  Scenario S8: outbox-only crash window 在重启时只修复一次
    Given durable outbox 已有 transition、StateLedger 尚未应用
    When combined continuation 冷启动两次（串行，第一次随后中断）
    Then 第一次补齐 projection
    And 第二次 repair 是 no-op
    And task/flow/bus 等价副作用不重复

  Scenario S9: standalone 模式保持旧语义
    Given operator 只用 --continue 或只用 --worktree --reuse-worktree
    When 启动 Ralph
    Then primary continue 保留既有 marker 语义
    And fresh reuse 仍归档 live artifacts
    And Parallel Forge standalone reuse 仍执行 manifest gate

Feature: Hat 私有 memory 的自动注入隔离

  Scenario S10: 当前 hat 只收到可见 memories
    Given store 含 shared、hat-A private、hat-B private
    When 为 hat-A 构建 auto-injected prompt
    Then prompt 包含 shared 与 hat-A private
    And prompt 不包含 hat-B private
    And budget 在可见集合格式化后应用

  Scenario S11: 无 private 或注入关闭时行为不变
    Given 仅 shared 或 memories.inject 非 Auto
    When 构建 prompt
    Then shared 注入、budget、disabled no-op 与现有行为一致
```

---

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
|---|---|---|---|---|---|
| S1 | path/id/marker 保持；loop.resume=1；无 starting/archive | `integration_resume.rs` | CLI 集成 | Characterization + idempotency | mock backend binary |
| S2 | 非零、提示、关键文件 hash 不变 | `integration_resume.rs` | CLI 集成 | 失败原子性 | 否 |
| S3 | 非成功 reason 均 Eligible | `recovery_checkpoint.rs` + CLI 一例 | 单元+集成 | table-driven | 否 |
| S4 | 不创建 `.worktrees/<name>` | `integration_worktree_isolation.rs` | CLI 集成 | 负副作用 | 否 |
| S5 | mismatch 且 marker 不变 | core + CLI | 单元+集成 | tamper | 否 |
| S6 | missing/I/O fail-close | core assessment | 单元 | fault injection | 否 |
| S7 | 第二 attach 拒绝 | LoopLock + CLI 子进程 | 并发集成 | TOCTOU | 是 |
| S8 | repair first=1 second=0 | `accepted_transition` + CLI fixture | 模块+集成 | crash-window + differential | 否 |
| S9 | 旧套件原断言绿 | 既有 integration | 回归 | differential | 既有 |
| S10 | prompt 无 other-private | `memory_visibility.rs` | 模块集成 | confidentiality | 否 |
| S11 | disabled/shared/budget | preview/build_prompt | 单元/集成 | characterization | 否 |

拒绝路径必须比较关键文件 bytes 或不存在性；成功路径读真实 event log。

---

## 6. 需求—测试追踪矩阵

| Req | 需求 | Scenario | 验收 | 单元 | 集成 | E2E | Evidence | Unit |
|---|---|---|---|---|---|---|---|---|
| R1 | typed intent | S1,S9 | 分支 | intent matrix | binary | 是 | E1–E5 | U2,U3 |
| R2 | exact target | S4,S5 | 非零 | identity | worktree binary | 否 | E7,E16 | U1,U2 |
| R3 | 独占 | S7 | 第二进程拒绝 | lock | subprocess | 是 | E8–E9,E31–E32 | U2 |
| R4 | 先验证 | S2,S5,S6 | 无 archive | failure matrix | CLI | 否 | E2–E3 | U1,U2 |
| R5 | completion_promise | S2,S3 | 分类 | terminal table | CLI 一例 | 否 | E10–E12 | U1,U2 |
| R6 | 原地 | S1 | event/id | resolve | binary | 是 | E16–E18 | U3 |
| R7 | repair 幂等 | S8 | 计数 | repair | ledger/outbox | 否 | E13–E15 | U4 |
| R8 | agnostic | S1,S8,S9 | 无 preset 字段 | model | core+CLI | 否 | E14,E20 | U1,U4 |
| R9 | standalone | S9 | 旧套件 | rpc | reuse/continue | 是 | E4–E5,E17–E20 | U3,U4 |
| R10 | memory | S10,S11 | prompt | load_visible 既有 | build_prompt | 否 | E21 | U5 |
| R11 | 无新格式 | S1,S8 | inventory | API | Cargo diff | 否 | E13,E20 | U1–U4 |
| R12 | 文档边界 | S9,S10 | help | n/a | help smoke | 否 | E34–E35 | U3,U5 |

---

## 7. 最大并发开发单元

```text
Validated Baseline @ 48841039
  ├─ U1 Typed checkpoint assessment     @ WT-u1
  └─ U5 Memory prompt visibility        @ WT-u5    ← 与 U1 同时启动，不等待恢复链

ASAP
  U1 Close → 立即 U2 Combined gate      @ WT-u2    （不等待 U5）
  U2 Close → 立即 U3 In-place continue  @ WT-u3    （不等待 U5）
  U3 Close → 立即 U4 Durable replay     @ WT-u4    （不等待 U5）

Fan-in：全部 Unit Close 后 Final Regression Gate
U5 可在 U2/U3/U4 之前或之间随时合入；与恢复语义正交。
```

编号不表示启动顺序。

---

### Unit 1：建立通用只读 Checkpoint Assessment

- **Wave ID：** L0（展示）
- **depends_on：** 无
- **ready_when：** 基线 checkout
- **release_condition：** §18；`pub` assessment API 可被 U2 调用
- **blocks：** U2
- **can_run_parallel_with：** U5
- **worktree / branch：** `.worktrees/u1-checkpoint-assessment` / `plan/2026-09-01-2102/u1-checkpoint-assessment`
- **基线：** `48841039`
- **生产写集合：** `crates/ralph-core/src/recovery_checkpoint.rs`（新增）、`crates/ralph-core/src/lib.rs`、可选 `crates/ralph-core/src/loop_history.rs`（只读终态查询）
- **测试写集合：** 同上文件 `#[cfg(test)]`；**不**改 `event_loop/tests/mod.rs`、**不**改 `accepted_transition.rs`
- **共享只读：** `read_outbox`、`LoopHistory::read_all`、路径约定
- **验证资源：** CPU；可与 U5 并行跑 nextest（不同 crate 包内不同 substring）
- **合并：** 可先于 U5 合入；不依赖 U5
- **Release Gate：** U1 nextest + loop_history + accepted_transition 既有测试绿 + clippy/build targeted

#### 1. 目标

给定 workspace path 与 expected loop id，返回 `Eligible` / `AlreadyCompleted` / `Refused{reason}`，无业务写。

#### 2. 对应

R2,R4,R5,R8,R11；S2–S6；D3–D5,D9–D11,D15–D16；E3,E10–E16,E22,E29,E33。

#### 3. 外部可观察

测试可见精确分类与 fixture digest 不变。CLI 文案属 U2。

#### 4. 当前基线

无通用 assessment。`is_completed()` 过宽。先 characterization 固定旧 API，再新增精确 API，**不改** `is_completed()` 语义。

#### 5. I/O

输入：workspace、expected loop id。输出：typed verdict + resolved id + 拒绝码。副作用：无。不变量：不读 preset 名。

#### 6. 修改位置

| 位置 | 职责 | 边界 | 不修改 |
|---|---|---|---|
| `recovery_checkpoint.rs` 新增 | assessment | 只读验证 + unit tests | runner/cleanup/repair |
| `lib.rs` | 导出 | `pub mod recovery_checkpoint` | 其他 export 扩张 |
| `loop_history.rs` | history | 仅当需要「最新终态+reason」公开查询 | `is_completed()` 行为 |

#### 7. 可依赖

`LoopHistory::read_all`、`read_outbox`、TempDir。

#### 8. 禁止未来

U2 RunIntent/锁、U3 runner、U4 CLI crash fixture、U5 memory。

#### 9. 验收测试（均在新模块）

- `assessment_marks_only_completion_promise_as_already_completed`
- `assessment_accepts_matching_interrupted_checkpoint_without_writes`
- `assessment_refuses_loop_identity_mismatch`
- `assessment_refuses_missing_current_event_target`
- `assessment_refuses_real_outbox_io_error`（path 为目录）
- `assessment_preserves_torn_tail_salvage_contract`

命令：`cargo nextest run -p ralph-core -- recovery_checkpoint`；回归 `cargo nextest run -p ralph-core -- loop_history` 与 `cargo nextest run -p ralph-core -- accepted_transition`。

#### 10. Acceptance Red

模块/API 不存在，或 table 把 max_iterations 标成 completed。无效 Red：nextest 未装、fixture 路径写错。

#### 11. 单元拆分

最新终态选择；reason 精确等于 `completion_promise`；marker 空/逃逸拒绝；identity；outbox missing/valid/torn/I/O；全分支 no-write digest。禁止 mock filesystem parser。

#### 12. TDD 顺序

```text
completion table Red → history 只读查询 → Green
→ identity/mandatory Red → assessment → Green
→ outbox I/O/torn Red → 调用 read_outbox → Green
→ no-write differential → Refactor reason 枚举
```

#### 13. 最小范围

mandatory：marker、current-events 目标、scratchpad、可读 history；outbox NotFound=空，I/O=拒绝。不写 checkpoint 文件。

#### 14. 集成

真实 `LoopHistory` + outbox fixture；不构造 StateLedger。

#### 15. 风险测试

Characterization `is_completed`；fault：dangling marker、outbox 为目录；differential 文件树。

#### 16. 回归

`loop_history`、`accepted_transition` 既有测试。原因：只读 API 不得改 salvage。

#### 17. 文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/recovery_checkpoint.rs` | 新增 | assessment | E10–E15 |
| `crates/ralph-core/src/lib.rs` | 修改 | 注册 | E29 |
| `crates/ralph-core/src/loop_history.rs` | 条件修改 | 精确终态查询 | E10–E11 |

#### 18. 完成标准

U1 测试绿；无写副作用；旧 `is_completed` 测试不变；core clippy/build；无 skip；可独立提交。

#### 19. 停止

成功终态无法由 history reason 识别；真实旧 continue 不需要某 mandatory 文件；必须改 `read_outbox` 语义或新持久格式。

#### 20. 风险

误用 `is_completed`；assessment 创建目录。检测：reason matrix + digest。剩余：不覆盖进程外副作用。

---

### Unit 5：修复 Memory Auto-Injection 的 Hat 可见性

- **Wave ID：** L0
- **depends_on：** 无
- **ready_when：** 基线 checkout
- **release_condition：** S10/S11 绿；CLI memory 回归绿
- **blocks：** 无后续功能 Unit
- **can_run_parallel_with：** U1, 以及 U1 关闭后的 U2/U3/U4（写集合不重叠）
- **worktree / branch：** `.worktrees/u5-memory-visibility` / `plan/2026-09-01-2102/u5-memory-visibility`
- **基线：** `48841039`
- **生产写集合：** `crates/ralph-core/src/event_loop/prompt_injection.rs`（仅 `store.load()` → `load_visible`）
- **测试写集合：** `crates/ralph-core/src/event_loop/tests/memory_visibility.rs`（新增）、`crates/ralph-core/src/event_loop/tests/mod.rs`（增加一行 `mod memory_visibility;`）
- **禁止写入：** `run.rs`、`recovery_checkpoint.rs`、`accepted_transition.rs`、`integration_resume.rs`
- **验证资源：** CPU；可与 U1 并行
- **合并：** 与 U1 任意顺序；与 U2+ 无语义冲突
- **Release Gate：** memory_visibility + preview + integration_memory

#### 1. 目标

hat-A 自动 prompt 含 shared+A-private，不含 B-private；budget 在过滤后计算。

#### 2. 对应

R10,R12；S10–S11；D13–D14,D17；E21,E30,E35,E36。

#### 3. 外部可观察

build_prompt 字符串；CLI list 权限不变。

#### 4. 基线

`load()` 泄漏 other-private。先写 leak reproducer。

#### 5. I/O

输入：store、HatId、budget、inject mode。输出：prefix。无 memory 写。读失败保持空 Vec。

#### 6. 修改位置

`inject_memories_and_tools_skill` 内 `store.load()` 改为 `store.load_visible(Some(hat_id.as_str()))`；日志 count 改为可见条数。不改 format/truncate 算法本身，只改输入集合。

#### 7. 可依赖

`MarkdownMemoryStore::load_visible`、`Memory::is_visible_to`。

#### 8. 禁止

不得做 ranking/存储重构；不得改 CLI 授权；不得改 combined 启动。

#### 9. 验收

hat-A/B 对称；only-shared；inject disabled；小 budget 不被 B-private 挤掉 visible；read error 不中断。

命令：`cargo nextest run -p ralph-core -- memory_visibility`；`cargo nextest run -p ralph-core -- preview_api`；`cargo nextest run -p ralph-core -- preview_characterization`；`cargo nextest run -p ralph-cli --test integration_memory`。

#### 10. Red

hat-A prompt 含 B-private 正文。无效：未 enable Auto inject、未走 build_prompt。

#### 11. 单元

shared / owner match / mismatch / budget-after-filter / disabled / read error。真实文件，不 mock 可见性规则。

#### 12. TDD

```text
cross-hat leak Red → load_visible 替换 → Green
→ budget-order Red → Green
→ disabled/read-error → Refactor 日志
```

#### 13. 最小

只替换加载入口。

#### 14. 集成

真实 EventLoop build_prompt + MarkdownMemoryStore。不得只测已存在的 `load_visible`。

#### 15. 风险

Confidentiality 负向断言；budget 边界。

#### 16. 回归

memory_store 既有、preview、integration_memory。shared 路径格式不得无故变化。

#### 17. 文件

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `prompt_injection.rs` | 修改生产 | visible load | E21 |
| `event_loop/tests/memory_visibility.rs` | 新增测试 | leak reproducer | E21,E30 |
| `event_loop/tests/mod.rs` | 一行注册 | nextest 收集 | E30 |
| `ralph-tools-memories.md` | 条件 | 仅反查不准确 | E35 |

#### 18. 完成

leak Green；CLI 回归；skill 反查；无 skip。

#### 19. 停止

HatId 无法对应 owner；preview 与 live 数据管线分裂且修复需改持久格式。

#### 20. 风险

日志打印 memory 正文或其他 hat id。过滤必须在 format/budget 前。

---

### Unit 2：隔离 Combined Intent、独占锁与 Fail-Closed 门禁

- **Wave ID：** L1
- **depends_on：** U1
- **ready_when：** U1 Release Gate
- **release_condition：** 拒绝路径 binary 绿；intent matrix 绿；standalone reuse 仍 create/archive
- **blocks：** U3
- **can_run_parallel_with：** U5（若仍在跑）
- **worktree / branch：** `.worktrees/u2-combined-gate` / `plan/2026-09-01-2102/u2-combined-gate`
- **基线：** U1 合入后的集成 commit（不得以未合入的 U5 为基线）
- **生产写集合：** `crates/ralph-cli/src/commands/run_recovery.rs`（新增）、`commands/mod.rs`、`commands/run.rs`（intent 分支、推迟 scratchpad 检查、combined 跳过 cleanup、获取目标 LoopLock）
- **测试写集合：** `run_recovery.rs` 内测、`integration_resume.rs`（拒绝/并发）、`integration_worktree_isolation.rs`（no-create）
- **禁止：** 成功路径 `loop.resume` 断言（U3）；改 PF validate 成功语义；改 `inner.rs`；改 `prompt_injection.rs`
- **验证资源：** CLI binary 构建；与 U5 的 core 测试可并行；与 U5 无文件冲突
- **合并：** 必须在 U3 之前进入集成基线
- **Release Gate：** 见命令清单 U2 行

#### 1. 目标

combined 在 cleanup/backend 前定位、加锁、消费 U1；拒绝路径零业务写。

#### 2. 对应

R1–R5,R8,R11；S2–S7；D1–D3,D6–D9,D11–D12,D15,D18；E1–E11,E16,E23–E26,E31–E33,E37–E38。

#### 3. 外部可观察

missing/completed/mismatch/lock busy 非零；无新 worktree/archive/marker 覆盖。

#### 4. 基线

combined 会 cleanup；scratchpad 查主 root。先 binary characterization。

#### 5. I/O

输入：RunArgs 四字段、exact name、workspace、prompt summary。输出：RunIntent + locked context 或错误。成功前仅 lock 瞬态。

#### 6. 修改位置

`run_recovery.rs`：分类、lookup、`LoopLock::try_acquire(worktree_path)`、调用 U1、错误映射。`run.rs`：reuse 分支 `if ContinueReusedWorktree` 跳过 cleanup；将 L840 scratchpad 预检限制为 ContinuePrimary，combined 改由 U1/U2 在目标树检查。TUI：parent 持 `_lock_guard` 直到 child wait；child `worktree_path` **不得**再 acquire 同锁。

#### 7. 可依赖

U1、`find_reusable_worktree_by_name`、`resolve_exact_worktree_name`、`LoopLock`、`common::ralph_bin`。

#### 8. 禁止

U3 happy resume；U4 repair；U5；提前改 PF manifest 校验规则。

#### 9. 验收

missing worktree；completed + 文件 bytes；identity；主 root 无 scratchpad 但 target 完整不得误报；lock held 第二进程；intent 四态。

命令：`cargo nextest run -p ralph-cli --bin ralph -- run_intent`；`cargo nextest run -p ralph-cli --test integration_resume -- combined`；`cargo nextest run -p ralph-cli --test integration_worktree_isolation -- combined`。

#### 10. Red

cleanup 发生、或主 root scratchpad 误检。无效：backend 缺失先于 gate、fixture 非 Git worktree。

#### 11. 单元

合法 flag 映射；combined absent vs ReuseFresh create；completed 文案含 remove `--continue`；lock busy 在 assessment 前。

#### 12. TDD

```text
intent matrix Red → classifier → Green
→ missing/completed Red → gate+U1 → Green
→ lock race Red → LoopLock → Green
→ failure atomicity → Green
→ standalone characterization → 薄化 run.rs
```

#### 13. 最小

只做 pre-run gate 与锁生命周期。无新 flag/env/crate。

#### 14. 集成

真实 temp git+worktree；拒绝测试不得启动 backend。并发用持锁进程+有界超时，不用加长 sleep 掩盖。

#### 15. 风险

Concurrency 双 attach；Differential standalone 仍 archive。

#### 16. 回归

`run.rs` 既有 worktree 名测试、integration_worktree_isolation 全文件、loop_lock tests。

#### 17. 文件

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `commands/run_recovery.rs` | 新增 | 门禁 | E1–E9,E25,E33 |
| `commands/mod.rs` | 修改 | `mod run_recovery` | E38 |
| `commands/run.rs` | 修改 | 接线 | E2,E6,E32 |
| `tests/integration_resume.rs` | 测试 | 拒绝/并发 | E17 |
| `tests/integration_worktree_isolation.rs` | 测试 | no-create | E19 |

#### 18. 完成

拒绝路径无 backend、无 archive；run.rs<5000；standalone 绿。

#### 19. 停止

LoopLock 无法仅由 parent 贯穿 TUI child wait；gate 必须先改业务文件；lookup 删除 registry。

#### 20. 风险

TOCTOU；parent/child 自锁。测试比较 artifacts 时排除 `loop.lock`。

---

### Unit 3：完成原地 Continuation、RPC/TUI Parity 与 Operator Contract

- **Wave ID：** L2
- **depends_on：** U2
- **ready_when：** U2 Release Gate
- **release_condition：** S1 binary + RPC Continue + child 跳过 PF gate + help/docs
- **blocks：** U4
- **can_run_parallel_with：** U5
- **worktree / branch：** `.worktrees/u3-inplace-continue` / `plan/2026-09-01-2102/u3-inplace-continue`
- **基线：** U2 合入后
- **生产写集合：** `commands/run.rs`（成功 combined 传 `resume=true`、forward `--continue`/`--worktree-path`、child 按 RunIntent 跳过 L1404–1426 gate）、`docs/guide/cli-reference.md`、必要时 `docs/guide/index.md`；`rpc_bootstrap.rs` **默认只加测试**
- **禁止生产写入：** `inner.rs`（E24/D18）；`prompt_injection.rs`；`accepted_transition.rs`
- **测试：** `integration_resume.rs` happy/non-success/RPC；`rpc_bootstrap` 既有保持；forward_prompt_args / resolve_loop_id / resume_branch 既有
- **验证：** CLI binary；可与 U5 队列化同一 runner 但开发并发
- **Release Gate：** U3 命令行 + `scripts/check-cli-doc-drift.sh --strict` + `ralph run --help`

#### 1. 目标

资格通过后同一 worktree/loop/current-events 走现有 Continue；三前端一致。

#### 2. 对应

R1,R3,R6,R8–R9,R12；S1,S3,S9；D1–D2,D5–D6,D11–D12,D14,D18；E4–E6,E16–E20,E23,E26,E34。

#### 3. 外部可观察

真实 binary：同 loop id、marker 不旋转、loop.resume=1、无 starting、无 archive；RPC Continue。

#### 4. 基线

standalone continue 已有 E17/E18；combined 仍 cleanup（U2 关闭后应已不再 cleanup）。本 Unit 接成功路径。

#### 5. I/O

允许 append resume/history/业务事件；loop-id 与 current-events 路径不变。

#### 6. 修改位置

`run.rs` 成功 combined：不 cleanup、context 用 reusable path+id、`resume=true`。child：`ContinueReusedWorktree` 跳过 `latest_archived_manifest` 校验；`ReuseFresh` 保持。`forward` 已含 `--continue`（L2571）。docs 写清组合与「已完成则去掉 continue」。

若 Red 指向 `inner.rs`：**停止**，不得加行。

#### 7. 可依赖

U1–U2、`initialize_resume`、mock backend `true`。

#### 8. 禁止

U4 新 fault 生产改动；U5；manifest 当 combined 恢复协议。

#### 9. 验收

主 root 无 scratchpad、worktree 有完整 checkpoint 的 happy binary；至少一个 non-success reason 可 continue；RPC Continue；TUI argv 含 continue+worktree-path、不双锁；PF 旧 incomplete archive 时 combined child 不 fail、去掉 `--continue` 的 ReuseFresh 仍 fail-closed。

命令：`cargo nextest run -p ralph-cli --test integration_resume -- combined_continue`；`cargo nextest run -p ralph-cli --bin ralph -- rpc_bootstrap`；`-- resolve_loop_id`；`-- resume_branch`；`-- forward_prompt_args`。

#### 10. Red

无 loop.resume、marker 旋转、或 child 被 stale manifest 拒绝。无效：TTY 不可控（用 argv helper）。

#### 11. 单元

resolve_loop_id 等于 context；bootstrap Continue；child 跳过 PF gate 的纯函数/分支。

#### 12. TDD

```text
happy binary Red → wiring → Green
→ RPC Red → Green
→ TUI lock ownership Red → Green
→ PF child gate Red → Green
→ docs/help → Refactor
```

#### 13. 最小

复用 `resume=true`；不改 auto-merge；不改 RPC enum。

#### 14. 集成

真实 CLI + git worktree + custom backend；读 current-events。

#### 15. 风险

Idempotency 一次 bootstrap；RPC contract；path identity。

#### 16. 回归

integration_resume 全文件、worktree isolation、legacy recovery、cli-doc-drift。

#### 17. 文件

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `commands/run.rs` | 修改 | 成功接线 + child intent | E2,E23,E26 |
| `rpc_bootstrap.rs` | 测试为主 | Continue parity | E4 |
| `docs/guide/cli-reference.md` | 文档 | 组合契约 | E34 |
| `docs/guide/index.md` | 条件 | 入口示例 | E34 |
| `integration_resume.rs` | 测试 | S1/S3 | E17 |

#### 18. 完成

S1 真 event 断言；三前端；standalone 回归；help；doc-drift；无 agent skill。

#### 19. 停止

需新 RPC variant、改 inner.rs、旋转 events、新 loop id、改 auto-merge 或 preset。

#### 20. 风险

parent lock 在 spawn 后 drop；history 追加 LoopStarted 干扰「最新终态」——U1 规则必须用**最近终态记录**而非首次。

---

### Unit 4：固化 Crash-Window Repair、重复恢复与 Standalone 回归

- **Wave ID：** L3
- **depends_on：** U3
- **ready_when：** U3 Release Gate（必须是最终 production 启动链）
- **release_condition：** S8 + S9
- **blocks：** 无（最终功能 fan-in）
- **can_run_parallel_with：** U5（若尚未合入）
- **worktree / branch：** `.worktrees/u4-durable-replay` / `plan/2026-09-01-2102/u4-durable-replay`
- **基线：** U3 合入后
- **生产写集合：** **默认空**。仅当 CLI-to-cold-start Red 指向 U1–U3 接线缺口时最小修那些文件；禁止 `inner.rs`、禁止改 outbox 格式
- **测试写集合：** `accepted_transition.rs` 的 `#[cfg(test)]`、`event_loop/tests/state_machine.rs`、`integration_resume.rs` crash fixture
- **禁止：** `tests/mod.rs` 新模块（避免与 U5 冲突；用已注册文件）
- **Release Gate：** 见命令清单 U4

#### 1. 目标

证明 combined 走同一 durable repair：first repair=1、second=0；standalone 不回归。

#### 2. 对应

R7–R9,R11；S8–S9；D3–D6,D10–D12,D16,D18；E12–E20,E24。

#### 3. 外部可观察

ledger snapshot 等价；task/flow/bus 不双发；PF manifest 测试仍绿。

#### 4. 基线

core 已有 repair 测试。缺口是 CLI combined → EventLoop 冷启动。若直接 Green，只留 characterization，零生产改动。

#### 5. I/O

不变量：exactly-once logical materialization。

#### 6. 修改位置

优先只加测试。生产仅修 U1–U3 绕过 cold-start 的接线。

#### 7. 可依赖

U1–U3 完整路径、StateLedger、Outbox、EventBus observer。

#### 8. 禁止

U5；新 DB；复制 event；改 retry budget；改 inner.rs。

#### 9. 验收

outbox-only first/second；duplicate delivered；SM disabled 仍 continue；genuine outbox I/O combined fail 且 bus 未启动；PF reuse 测试保持。

命令：`cargo nextest run -p ralph-core -- accepted_transition`；`-- state_machine`；`cargo nextest run -p ralph-cli --test integration_resume -- checkpoint_repair`；`--test integration_worktree_isolation -- reuse_worktree`。

#### 10. Red

重复 projection 或未进 cold-start。已实现则 Green=characterization，不改生产。

#### 11. 单元

repair counts；disabled SM；I/O fail-close。禁止 mock ledger/outbox 核心。

#### 12. TDD

```text
CLI crash fixture Red/Char → 仅修接线 → Green
→ matrix → standalone differential → helper 整理
```

#### 13. 最小

零生产优先。

#### 14. 集成

真实 FS/ledger/outbox/EventLoop。

#### 15. 风险

Fault injection；Idempotency；Differential。并发已由 U2 覆盖。

#### 16. 回归

core accepted_transition/state_machine；cli resume/worktree；随后 `-p ralph-core` 与 `-p ralph-cli`。

#### 17. 文件

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `accepted_transition.rs` | 测试为主 | repair | E13–E15 |
| `event_loop/tests/state_machine.rs` | 测试 | cold-start | E14 |
| `integration_resume.rs` | 测试 | CLI 路径 | E17 |
| U1–U3 生产 | 条件 | 仅真实 Red | 新 Evidence |

#### 18. 完成

S8/S9 绿；未削弱旧断言；Cargo.lock 无新依赖。

#### 19. 停止

需改 outbox 格式、公开 API 扩大、inner.rs、分布式外系统事务。

#### 20. 风险

只断言 ledger 不断言 materialize/bus。必须三者一起看。

---

## 8. Unit 最大并发依赖图

```text
Validated Baseline 48841039
  ├──────────────────────────┐
  ↓                          ↓
 U1 assessment              U5 memory visibility
  ↓ close                    │（独立持续到 Close）
 U2 combined gate            │
  ↓ close                    │
 U3 in-place continue        │
  ↓ close                    │
 U4 durable replay           │
  └──────── fan-in ──────────┘
        Final Regression
```

| Unit | Layer | depends_on | ready_when | blocks | parallel | worktree | 写集合 | 验证 | Release |
|---|---|---|---|---|---|---|---|---|---|
| U1 | 0 | — | baseline | U2 | U5 | u1-… | recovery_checkpoint, lib.rs, 可选 loop_history | nextest core | assessment API |
| U5 | 0 | — | baseline | — | U1–U4 | u5-… | prompt_injection, tests/mod.rs 一行, memory_visibility.rs | nextest core+cli memory | S10/S11 |
| U2 | 1 | U1 | U1 gate | U3 | U5 | u2-… | run_recovery.rs, mod.rs, run.rs, 两集成测 | nextest cli | fail-closed gate |
| U3 | 2 | U2 | U2 gate | U4 | U5 | u3-… | run.rs, docs, integration_resume | nextest cli + help | S1 |
| U4 | 3 | U3 | U3 gate | — | U5 | u4-… | 测试为主 | core+cli | S8/S9 |

### Serial Edge Ledger

| Edge | 为什么必须串行 | Evidence | 拆分/隔离尝试 | 仍不能并发 | 置信度 |
|---|---|---|---|---|---|
| U1→U2 | U2 CLI/gate 必须调用 U1 真实 verdict；binary 拒绝测试不能对 stub 验收 R4/R5 | E10,D2,D4 | stub 接口会双源分类 | 验收要真实 assessment | 0.93 |
| U2→U3 | 成功 continue 必须建立在「已跳过 cleanup + 已持锁」上，否则 S1 与竞态不可归因 | E2,E32,D6,D8 | 不能在未接线 gate 上测 happy | 同一 `run.rs` 成功/失败分支语义耦合 | 0.94 |
| U3→U4 | S8 必须走最终 production CLI→EventLoop 链 | E14,E17 | 仅 core 测试无法证明 combined 未绕过 | 缺口正是入口 | 0.92 |

**已消除的避免串行：** 旧 U4→U5（仅「归因清晰」）——写集合正交（E21 vs E2），D17。

**同文件分析：** `run.rs` 由 U2 再 U3 顺序修改（失败门禁 vs 成功接线），不可并行。`integration_resume.rs` 沿 U2→U3→U4 追加用例，串行于依赖链。`tests/mod.rs` 仅 U5 新增一行。`accepted_transition.rs` 生产默认无人改；U4 只加测试且在 U1 之后，无并行写。

### 8.1 Parallelism Summary

| 指标 | 值 | 说明 |
|---|---|---|
| Total Units | 5 | U1–U5 |
| DAG Depth | 4 | U1→U2→U3→U4；U5 深度 1 |
| Critical Path | U1→U2→U3→U4 | 锁/cleanup/continue/replay 因果链 |
| Initial Ready Set | U1, U5 | 必须同时启动 |
| Max Planned Concurrency | 2 | 无执行器上限时 L0 为 2；之后 U2/U3/U4 可与未完成的 U5 形成 2 |
| Serial Edges | 3 | 见 Ledger |
| Avoidable Serialization | **0** | U5 已从关键路径拿掉 |
| Global Barrier Count | 0 | 无「Wave 全完成才启动」；仅最终全量回归 fan-in |

若执行器 K=1：优先 U1（释放最长链），再 U2、U3、U4，U5 可插空但不得阻塞 U1。

---

## 9. 执行命令清单

均在仓库根；required 失败不得进入下一步。substring 无匹配时先 `cargo nextest list`，禁止裸 `cargo test -p ralph-cli`。spawn ralph 用 `common::ralph_bin()` scrub。

| 时机 | 命令 | 目的 | 预期 | 失败能否继续 |
|---|---|---|---|---|
| 前置 | `cargo nextest --version` | 钉死 0.9.140 | `cargo-nextest 0.9.140` | 否 |
| U1 | `cargo nextest run -p ralph-core -- recovery_checkpoint` | assessment TDD | Green | 否 |
| U1 回归 | `cargo nextest run -p ralph-core -- loop_history` | history 兼容 | Green | 否 |
| U1/U4 | `cargo nextest run -p ralph-core -- accepted_transition` | outbox 未破坏 | Green | 否 |
| U5 | `cargo nextest run -p ralph-core -- memory_visibility` | S10 | Green | 否 |
| U5 回归 | `cargo nextest run -p ralph-core -- preview_api` | preview | Green | 否 |
| U5 回归 | `cargo nextest run -p ralph-core -- preview_characterization` | 旧注入 | Green | 否 |
| U5 CLI | `cargo nextest run -p ralph-cli --test integration_memory` | CLI 可见性 | Green | 否 |
| U2 | `cargo nextest run -p ralph-cli --bin ralph -- run_intent` | 四态 | Green | 否 |
| U2/U3 | `cargo nextest run -p ralph-cli --test integration_resume -- combined` | combined | Green | 否 |
| U2/U3 回归 | `cargo nextest run -p ralph-cli --test integration_worktree_isolation` | reuse | Green | 否 |
| U3 | `cargo nextest run -p ralph-cli --bin ralph -- rpc_bootstrap` | Continue | Green | 否 |
| U3 | `cargo nextest run -p ralph-cli --bin ralph -- resolve_loop_id` | id | Green | 否 |
| U3 | `cargo nextest run -p ralph-cli --bin ralph -- resume_branch` | 无 starting | Green | 否 |
| U3 | `cargo nextest run -p ralph-cli --bin ralph -- forward_prompt_args` | argv | Green | 否 |
| U3 docs | `cargo run -p ralph-cli -- run --help` | 组合说明 | 可读 | 否 |
| U3 drift | `bash scripts/check-cli-doc-drift.sh --strict` | 文档 | exit 0 | 否 |
| U4 | `cargo nextest run -p ralph-core -- state_machine` | cold-start | Green | 否 |
| U4 CLI | `cargo nextest run -p ralph-cli --test integration_resume -- checkpoint_repair` | S8 | Green | 否 |
| 格式 | `cargo fmt --all -- --check` | fmt | exit 0 | 否 |
| Lint | `cargo clippy` | lint | exit 0 | 否 |
| Build | `cargo build` | build | exit 0 | 否 |
| 包回归 | `cargo nextest run -p ralph-core` | core | Green | 否 |
| 包回归 | `cargo nextest run -p ralph-cli` | cli | Green | 否 |
| 最终 | `./scripts/run-tests.sh` | 两阶段+doctest | 全绿 | 否 |
| 仅 flake | `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | 判别 | serial 仍失败=真失败 | 否 |

---

## 10. 最终质量门禁

- S1–S11 可追踪到 R/Unit。
- 拒绝路径：无 archive、无 marker 覆盖、无 backend、关键 bytes 不变。
- 成功路径：同 path/id/current-events、loop.resume=1、无 fresh starting。
- 双进程单 writer。
- repair first=1/second=0；materialize/task/flow/bus 不重复。
- standalone continue/reuse/PF/fresh/RPC/TUI 回归。
- other-private 不进 live prompt。
- fmt、clippy、build、help、doc-drift、core/cli、`./scripts/run-tests.sh`。
- 无 skip/ignore/only；无削弱断言；Cargo 无新依赖；无新 checkpoint 文件。
- 未改 presets/schemas/manifest/zsh。
- `run.rs`<5000；`inner.rs` 生产行数不增加。
- operator 内容未进 `crates/ralph-core/data/*.md`（除非 E35 反查最小补丁）。
- Decision 仍 ≥0.85。
- Ready Set 实际按 U1∥U5 启动；无 Wave 全局屏障。
- Avoidable Serialization = 0。

---

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 实施计划而非 Roadmap | 是 | 五 Unit 均有入口、Red、边界、命令 |
| Executor 是否仍需关键设计 | 否 | D1–D18 |
| 文件接口有证据 | 是 | E1–E38；新增已标注 |
| 决策 ≥0.85 | 是 | 最低 D4/D8=0.91 |
| 未处理低置信假设 | 否 | 无阻塞假设 |
| 每 Unit 一个可观察行为 | 是 | assessment / gate / continue / replay / memory |
| 可独立验证 | 是 | 各自 nextest |
| 真实 Red | 是 | 各 Unit §10 |
| 含回归 | 是 | §16 |
| 依赖未来 Unit | 否 | 仅前置 Release Gate |
| 泛化任务 | 否 | |
| Scenario 可追踪 | 是 | §5–6 |
| 决策有 Evidence | 是 | |
| DAG 明确 | 是 | §7–8 |
| Initial Ready Set 完整 | 是 | U1,U5 |
| 无依赖已最大并发 | 是 | |
| 依赖解除 ASAP | 是 | |
| 串行边有 Evidence | 是 | 3 条 Ledger |
| Avoidable Serialization=0 | 是 | U5 已并行 |
| 无多余 Wave Barrier | 是 | |
| 独立 worktree | 是 | |
| 无未处理写冲突 | 是 | run.rs 已串行；mod.rs 仅 U5 |
| 稀缺资源只限验证 | 是 | CLI binary 可队列 |
| Fan-In Gate 已定义 | 是 | §9 最终 + §10 |

### Sources & References

- `CONCEPTS.md`：Accepted Transition、Recovery Intent（与本计划 `recovery_checkpoint` 不同模块）。
- `docs/achieved/plan/2026-08-03-004-feat-parallel-forge-execution-resume-plan.md`
- `docs/achieved/plan/2026-08-15-2211-fix-state-machine-transaction-boundary-plan.md`
- 诊断报告仅作「本计划曾被 PF 执行失败」的操作记录，不改产品合同
- 代码优先于文档；E1–E38
