---
title: "feat: 通用可信 Checkpoint 与 Worktree Continuation"
type: feat
date: 2026-09-01
deepened: 2026-09-01
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
product_contract_source: ce-plan-bootstrap
baseline_branch: pittcat-dev
baseline_commit: 0a89cfdcc693319f444caa2f44a14728e56bf606
---

# 通用可信 Checkpoint 与 Worktree Continuation 开发计划

## 0. 计划状态

- **状态：READY。** 所有影响实施方向的决策均达到 `0.85`；没有需要 Executor 临时拍板的关键设计问题。
- **基线：** `pittcat-dev`，`0a89cfdcc693319f444caa2f44a14728e56bf606`，工作树在调查时为 clean。
- **调查范围：** `ralph run` 参数与 worktree 选择；`--continue`、RPC/TUI 转发和 loop-id 解析；worktree registry/锁/清理；`LoopHistory`；accepted-transition outbox、StateLedger 冷启动 repair；task/policy/event replay；Parallel Forge 专用 resume manifest；memory store 可见性及 prompt 自动注入；现有集成测试、项目测试入口、Git 历史与 achieved plans。
- **已执行验证：** 只读源码/测试/文档检索、相关 Git history、模块行数和测试入口检查。遵循 `ce-plan`，计划阶段没有运行构建、测试或应用。
- **尚未执行：** 本计划中的任何 Red/Green、nextest、clippy、build、CLI smoke、doc-drift 和全量测试；这些全部属于实施阶段。
- **阻塞项：** 无。实施中若发现 `LoopHistory(reason=completion_promise)` 并非接受 `LOOP_COMPLETE` 后的唯一成功记录，或 combined mode 无法在持有 worktree 独占锁期间贯穿 TUI 子进程，则立即按各 Unit 停止条件重新决策。

---

## 1. 功能目标

### 1.1 业务目标

让 operator 能用同一个 worktree、同一个逻辑 loop 和现有 durable runtime state，在进程断开、崩溃或人工中断后安全续跑：

```text
ralph run --worktree --reuse-worktree --continue \
  --plan <same-plan> [existing run options]
```

这里的 checkpoint 是“可验证的持久化执行边界集合”，不是 LLM 隐藏上下文快照。它由 loop identity、current-events、history、accepted-transition outbox、StateLedger、task/progress/scratchpad 等现有磁盘状态构成；恢复时先资格审计，再沿现有 `--continue` 冷启动路径修复和重放。

### 1.2 用户或调用方

- 直接调用 `ralph run` 的 human operator。
- TUI parent/RPC child 组合启动路径。
- 依赖同一 worktree 中 task、event、StateLedger 和 policy state 连续性的所有 preset；不限定 Parallel Forge。
- 受 memory 自动注入影响的 isolated/coordinator hats。

### 1.3 当前行为

- CLI 接受 `--continue` 与 `--worktree --reuse-worktree` 同时出现。
- reuse 分支先调用 `clean_worktree_runtime_artifacts`，把 `events/history/current-loop-id/tasks/scratchpad` 等移入 `.ralph/reuse-history/`，再启动 loop。
- runner 中 `resume=true` 会忽略 reuse manifest；RPC bootstrap 明确让 Continue 优先于 ManifestResume。
- combined invocation 因而先删除 continue 所需 live state，再声称继续；另外，早期 scratchpad precheck 针对主 workspace，而不是尚未选出的复用 worktree。
- standalone `--continue` 会保留 current-events，不重发 fresh starting event，复用 current-loop-id，并执行 StateLedger/outbox repair、policy hydration 和 `loop.resume` bootstrap。
- standalone `--reuse-worktree` 是 fresh reuse：归档旧运行态；Parallel Forge 另有专用 manifest gate。
- memory CLI 的 list/show/search/prime 使用 hat 可见性规则，但 prompt 自动注入调用 `store.load()`，会把其他 hat 的 private memory 注入当前 hat prompt。

### 1.4 目标行为与差异

- 将启动意图显式区分为 Fresh、ContinuePrimary、ReuseFresh、ContinueReusedWorktree；不再由独立 bool 的偶然分支顺序决定语义。
- `ContinueReusedWorktree` 必须精确定位已存在、Git 登记的同名 worktree；找不到时拒绝，不能像 fresh reuse 一样创建新 worktree。
- 在目标 worktree 上取得独占 loop lock，之后只读校验 checkpoint，且校验完成前不得归档或删除 live runtime artifact。
- checkpoint 必须验证 loop-id 一致、current-events marker 指向存在文件、scratchpad 存在、history 可读、outbox 可读；允许不存在的可选 ledger/progress 由现有 cold-start 逻辑安全降级。
- history 最近有效终态若是 `LoopCompleted(reason=completion_promise)`，拒绝 combined continue，并明确提示移除 `--continue` 以开始 fresh reuse。
- history 的 max-iterations/max-runtime/failure 等非成功 `LoopCompleted` 记录不是 credible `LOOP_COMPLETE`，允许继续；`LoopTerminated`/无终态也允许继续。
- 资格通过后不调用 worktree cleanup、不旋转 current-events、不生成新的 logical loop id；直接进入既有 `--continue` 路径。
- 重启 repair 必须保持 at-most-once materialization/publish：outbox-only crash window 被补投影，已提交 transition 不重复产生业务副作用。
- standalone continue、standalone reuse（含 Parallel Forge manifest）和 fresh worktree 行为保持不变。
- 自动 memory 注入只包含 shared + 当前 hat 自有 private memory；CLI 的既有可见性语义不变。

### 1.5 Requirements

- **R1 — 启动意图唯一化：** 四种启动意图必须互斥、可穷尽；combined mode 不能落入 fresh reuse cleanup。
- **R2 — 精确目标：** combined mode 必须绑定既有 exact-name worktree 和该 worktree 的 `current-loop-id`；不存在、Git 未登记、显式 loop-id 不一致均非零退出且零持久业务副作用。
- **R3 — 并发排他：** combined mode 在 checkpoint 读取前取得目标 worktree 的独占 loop lock，并持有到 runner/TUI child 完成；第二个进程必须被拒绝。
- **R4 — 先验证后变更：** checkpoint 审计通过前不得调用 `clean_worktree_runtime_artifacts`、不得改 current-events/history/tasks/scratchpad/outbox；失败时不产生 reuse-history archive。
- **R5 — 成功终态拒绝：** 最近有效 history 终态为 `LoopCompleted(reason=completion_promise)` 时拒绝 continue，并提示移除 `--continue`；其他 termination reason 不得误判为成功。
- **R6 — 原地续跑：** combined mode 保留 live runtime 文件、worktree path、loop-id 和 current-events，发布一次 `loop.resume`，不得重发 starting event。
- **R7 — 持久边界恢复：** cold start 先修复 outbox-only StateLedger projection，再 hydration；重复恢复不得重复 materialize、task mutation、flow advance 或 bus publish。
- **R8 — 通用性：** R1-R7 不根据 preset 名称分支；Parallel Forge manifest 仅继续服务 standalone fresh reuse。
- **R9 — 兼容隔离：** standalone `--continue`、standalone `--reuse-worktree`、fresh worktree、RPC/TUI 和 no-TUI 的现有行为与测试不变。
- **R10 — Memory 隔离：** auto-injected memory data 必须使用当前 hat 可见视图；shared 与 own-private 可见，other-private 不可见；budget 在过滤后应用。
- **R11 — 无新持久格式/依赖：** 不引入新数据库、第三方 crate、定期全量 snapshot 或 backend session restore；checkpoint assessment 是现有 durable artifacts 的 typed read-only view。
- **R12 — 文档边界：** combined flags 属于 operator control plane，只更新 clap help 和 operator docs，不写入 agent-injected skill；memory visibility 若现有 skill 已准确描述，仅以测试证明并不改文案。

### 1.6 输入、输出、状态和错误

- **输入：** resolved RunArgs、exact worktree name、Git worktree registry、loop registry PID、worktree loop lock、`.ralph/current-loop-id`、`.ralph/current-events`、其指向的 event log、`.ralph/history.jsonl`、`.ralph/agent/scratchpad.md`、accepted-transition outbox、StateLedger/task/progress 文件。
- **输出：** typed run intent；read-only checkpoint assessment（eligible/completed/refused + reason + loop id）；成功时既有 `LoopBootstrap::Continue` 和一个 `loop.resume`；失败时稳定、可操作的 CLI error。
- **状态变化：** 审计前只有临时锁；审计成功后由原有 continue runner 追加 history/resume event 并继续状态机。不得创建 reuse archive。
- **错误语义：** active/locked、missing target、identity mismatch、missing mandatory artifact、I/O/parse error、credible completion 均 fail-closed；错误消息包含目标 worktree 和 operator 下一步，不泄漏 memory 内容。
- **幂等性：** checkpoint assessment 本身无业务写；重复 continue 依靠 loop lock 串行，accepted-transition/outbox/StateLedger 依靠现有 transition identity 去重。

### 1.7 兼容、性能、安全和约束

- **兼容性：** 不更改 CLI 参数名；旧 primary continue 与 old fresh reuse 保持；不迁移旧数据。combined 是此前未正确实现的组合语义。
- **性能：** 启动时线性扫描 history/outbox/current event metadata；不得遍历 worktree 全树或复制 artifacts。规模与现有 cold-start replay 同阶。
- **安全/权限：** 只操作 operator 精确指定/plan 派生且经 Git worktree 交叉验证的路径；沿用 worktree name 校验；private memory 不跨 hat。
- **文件规模：** `commands/run.rs` 已 4,395 行，接近 5,000 硬上限；新增恢复分类/审计逻辑必须进入新模块，`run.rs` 只保留薄编排。所有新增/修改源码实施后复查行数。
- **测试约束：** 所有 Rust 测试用 nextest；最终使用 `./scripts/run-tests.sh`；不得裸跑 `cargo test -p ralph-cli`。

### 1.8 范围与非目标

**本次范围：** R1-R12；通用 combined continuation；冷启动幂等回归；memory prompt visibility 漏洞；CLI/operator docs。

**非目标：**

- 不恢复 LLM token/context、backend tool stream、PTY session 或网络连接。
- 不新增 `ralph checkpoint` 命令、定时器、checkpoint 文件格式或人工 checkpoint 管理 UI。
- 不把 Parallel Forge manifest 泛化或删除；不改 preset/schema/event topology。
- 不改变 task.resume/loop.resume 的 agent-facing 定义，不改变 retry budget。
- 不做 memory 排名、向量检索、数据库迁移、压缩、跨仓库同步或完整 memory 重构。
- 不自动 merge、删除、重建或 checkout worktree。

**Deferred to Follow-Up Work：** 可观测 checkpoint inspect 子命令、backend session resume、memory relevance/ranking、定期 compact snapshot。

### 1.9 事实、假设与决策分离

**已确认事实：** 见 Evidence Ledger E1-E22。

**已确认假设：**

- A1：用户选择的成功终态语义是“有可信 `LOOP_COMPLETE` 则 combined continue 拒绝并提示去掉 `--continue`”；这是 session-settled 输入。
- A2：combined mode 的目标是同一 logical loop 原地继续，而不是 reuse code/worktree 后开始新 lineage；这是 session-settled 输入。
- A3：checkpoint 恢复持久状态边界，不恢复隐藏模型上下文；与仓库 tenet 和既有 resume plan 一致。

**待验证假设：** 无实施阻塞假设。实际 Red 的具体错误字符串属于执行证据，不是规划期事实；若与预期失败类别不同，Unit 必须停止。

---

## 2. 代码库现状与证据

### 2.1 当前实现入口与调用链

```text
ralph run / RunArgs
  → commands/run.rs: config + prompt + exact worktree name
  → if worktree && reuse_worktree
      → find_reusable_worktree_by_name
      → clean_worktree_runtime_artifacts          # 当前总会 fresh-clean
      → optional parallel_forge ResumeManifest
      → LoopContext::worktree
  → TUI parent 或 run_loop_impl
      → resolve_loop_id
      → EventLoop::with_context_and_diagnostics
          → read current-events
          → build StateLedger
          → repair projection from outbox
          → hydrate policy/state machine/task/progress
      → resume ? initialize_resume : initialize
      → LoopBootstrap::{Continue, ManifestResume, Fresh}
```

数据权威边界：

```text
accepted business/recovery event
  → AcceptedTransition durable outbox
  → materialize task/flow/state changes
  → optional StateLedger projection
  → EventBus publish

accepted LOOP_COMPLETE
  → TerminationReason::CompletionPromise
  → LoopHistory::LoopCompleted(reason="completion_promise")
```

`LOOP_COMPLETE` 是 LoopControl，不进入 outbox；因此 checkpoint assessment 必须用 history 判定“成功已完成”，用 outbox/ledger 判定业务 transition 的 durable replay。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `crates/ralph-cli/src/commands/run.rs` `RunArgs.continue_mode/reuse_worktree` | 两个参数无 clap conflict，可同时解析。 | 不能用新增参数规避；必须定义组合语义。 | 高 |
| E2 | `commands/run.rs` reuse 分支 | reuse 先构造可选 PF CaptureInputs，再调用 `clean_worktree_runtime_artifacts`。 | combined 必须在此分叉，绕过 fresh cleanup。 | 高 |
| E3 | `crates/ralph-core/src/worktree.rs::clean_worktree_runtime_artifacts` | cleanup 归档 events/history/current-loop-id/flow authority/scratchpad/tasks/summary/handoff/decisions，并写 resume-context。 | 这些正是 continue 的 live checkpoint；combined 不得调用。 | 高 |
| E4 | `crates/ralph-cli/src/loop_runner/rpc_bootstrap.rs::loop_bootstrap` | `resume=true` 优先返回 Continue；测试固定 `continue_takes_precedence_over_reuse_manifest`。 | combined 应继续使用 Continue，不把 PF manifest 混入同一 lineage。 | 高 |
| E5 | `crates/ralph-cli/src/loop_runner/inner.rs` manifest bootstrap | resume 时 manifest 被忽略；非-resume 才可 ManifestResume。 | standalone fresh reuse 与 combined continue 必须保持两条协议。 | 高 |
| E6 | `commands/run.rs` continue precheck | 当前 scratchpad 检查发生在 worktree 选择前，root 是主 workspace。 | combined 的 mandatory artifact 检查必须延后到目标 worktree。 | 高 |
| E7 | `crates/ralph-core/src/worktree.rs::find_reusable_worktree_by_name` | exact path 与 Git worktree list 交叉验证；相同 registry entry PID alive 时拒绝；无 registry entry 仍允许已有 Git worktree。 | 复用该定位能力；checkpoint identity 另校验 marker。 | 高 |
| E8 | `crates/ralph-core/src/loop_registry.rs` | registry 使用 flock/PID stale cleanup；LoopEntry 没有 completion status。 | 不可把 dead PID 等同成功；成功依据来自 history。 | 高 |
| E9 | `crates/ralph-core/src/loop_lock.rs`、`commands/run.rs` | primary loop 可持有 RAII `.ralph/loop.lock`；worktree mode当前跳过。 | combined 在目标 worktree 复用 LoopLock，关闭双重 attach race。 | 高 |
| E10 | `crates/ralph-core/src/loop_history.rs` | append-only history；`LoopCompleted` 带 reason；`is_completed()` 对任何 LoopCompleted 返回 true。 | 不直接用 `is_completed()`；新 assessment 必须判定最新终态且 reason 精确为 completion_promise。 | 高 |
| E11 | `loop_runner/inner.rs` termination bookkeeping | Interrupted 写 `LoopTerminated(SIGTERM)`；其他 termination（包括 max/failure）均写 `LoopCompleted(reason_str)`。 | R5 必须有 failure/non-success characterization，防止误拒绝。 | 高 |
| E12 | `crates/ralph-core/src/event_loop/disposition.rs` | Business/Recovery 走 AcceptedTransition；Diagnostic/LoopControl 直接 publish。 | outbox 不承担 LOOP_COMPLETE 成功判定。 | 高 |
| E13 | `accepted_transition.rs` | outbox 是 append-only/fsync durable receipt；read_outbox salvages malformed/torn lines；genuine I/O error fail-closed。 | checkpoint 可复用 durable entries，但需要 recovery assessment 显式报告无法可信读取的 I/O；不改变现有 salvage contract。 | 高 |
| E14 | `event_loop/acceptance_and_lifecycle.rs` | StateLedger always enabled；冷启动先 repair outbox-only projection，再 hydrate runtime。 | 成功 combined 直接进入现有路径，不新增第二套 replay engine。 | 高 |
| E15 | `accepted_transition.rs` repair tests 与 commits `9e102015`,`55d46dd8`,`1be0eff9` | 已覆盖 outbox/ledger split、重复 repair、commit failure rollback。 | U4 做 differential/fault regression，不重构事务协议。 | 高 |
| E16 | `loop_runner/runner.rs::resolve_loop_id` | worktree LoopContext 的 loop id优先；primary continue 才读 marker/explicit id。 | combined 的 context id必须与 marker一致；成功后天然维持同一 id。 | 高 |
| E17 | `integration_resume.rs::test_continue_publishes_loop_resume_event` | continue 保留 marker/current event并写 `loop.resume`，不写 legacy task.resume。 | combined 验收应沿用相同外部断言。 | 高 |
| E18 | `loop_runner/tests/legacy/recovery.rs::u5_resume_branch_does_not_re_inject_work_start` | resume 不旋转 marker、不追加 starting event。 | combined 必须复用而非复制此逻辑。 | 高 |
| E19 | `integration_worktree_isolation.rs` | standalone reuse 已有 archive、first-create、live PID、no-auto-merge、manifest 等真实 binary 测试。 | R9 回归范围明确；不得改这些断言去适配 combined。 | 高 |
| E20 | `parallel_forge_resume.rs` 与 achieved plan `2026-08-03-004` | manifest 明确是 PF-specific fresh-reuse protocol，绑定 plan/preset/config/worktree identity。 | 不泛化、不删除；combined 不消费 manifest。 | 高 |
| E21 | `memory_store.rs::load_visible` 与 `prompt_injection.rs::inject_memories_and_tools_skill` | store已有正确 visibility API；prompt自动注入却调用无过滤 `load()`，且已持有 hat_id。 | 最小修复是调用已有 API，并加 prompt-level测试。 | 高 |
| E22 | `AGENTS.md`、`.config/nextest.toml`、`scripts/run-tests.sh`、`mise.toml` | nextest 0.9.140；full suite 两阶段；禁止裸 cargo test；run.rs 接近 5000 行。 | 命令、模块拆分与最终门禁固定。 | 高 |
| E23 | `commands/run.rs` 的 hidden `--worktree-path` child 分支 | child 在进入 runner 前会重新读取/验证最新 PF archived manifest；后续 inner 虽在resume时忽略manifest，但旧manifest已可能提前拒绝combined。 | U3必须让parent和child共享RunIntent，并只在ReuseFresh执行child manifest gate。 | 高 |

### 2.3 受影响范围

**生产模块：**

- `crates/ralph-core/src/loop_history.rs`：提供不误判非成功 reason 的只读终态查询，或由新 checkpoint 模块在公开 read API 上实现。
- `crates/ralph-core/src/event_loop/accepted_transition.rs`：仅在 assessment 需要区分 I/O/有效 entries 时复用或增加只读检查；不得改变 commit/salvage 语义。
- `crates/ralph-core/src/recovery_checkpoint.rs`（计划新增）：typed、preset-agnostic、read-only checkpoint assessment。
- `crates/ralph-core/src/lib.rs`：导出新增 assessment 能力。
- `crates/ralph-cli/src/commands/run_recovery.rs`（计划新增）：RunIntent 分类、目标 worktree eligibility、错误映射和锁编排。
- `crates/ralph-cli/src/commands/mod.rs`、`commands/run.rs`：薄接线。
- `crates/ralph-core/src/event_loop/prompt_injection.rs`：memory auto-injection 改用 visible view。

**测试模块：**

- 新模块自身 unit tests。
- `crates/ralph-cli/tests/integration_resume.rs`。
- `crates/ralph-cli/tests/integration_worktree_isolation.rs`。
- `crates/ralph-cli/src/loop_runner/tests/legacy/{misc,recovery}.rs` 与 `rpc_bootstrap.rs` 既有回归。
- `crates/ralph-core/src/event_loop/tests/state_machine.rs`、`accepted_transition.rs` 既有 fault/replay tests。
- `crates/ralph-core/src/event_loop/tests/memory_visibility.rs`（计划新增）及 `tests/mod.rs`。
- `crates/ralph-cli/tests/integration_memory.rs` 既有 CLI visibility 回归。

**CLI/docs：** `RunArgs` help、`docs/guide/cli-reference.md`、`docs/guide/index.md`（仅当入口文案需补 combined 示例）。不修改 presets、schemas、manifest、zsh builtin 补全或 agent-injected operator guidance。

**不受影响：** API/UI 数据契约、数据库、网络服务、builtin preset topology、event schema、worktree merge策略。

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| D1 | combined 是新 run 还是同 lineage continue | fresh reuse + manifest；原地 continue；恢复 backend session | **原地 continue**（session-settled: user-directed — chosen over fresh reuse：用户要求 continue 与 reuse-worktree 联合后仍续上同一工作） | E2-E6,E16-E20,A2 | fresh reuse 会清掉状态并换运行语义；backend session不可通用验证。 | 0.97 |
| D2 | 如何表达模式 | 继续依赖 bool 顺序；clap 禁止组合；typed RunIntent | typed RunIntent，四态穷尽 | E1-E6 | 禁止组合违背需求；bool 顺序正是当前缺陷根因。 | 0.95 |
| D3 | checkpoint 是什么 | 新 snapshot 文件；复制 PF manifest；现有 durable artifacts 的 typed view | typed read-only assessment，不新增持久格式 | E3,E10-E15,E20 | 新快照有双写/漂移；PF manifest非通用且代表 fresh lineage。 | 0.93 |
| D4 | 成功完成权威 | registry dead PID；events 中字符串；outbox；history completion reason | 最近有效 history terminal 为 `LoopCompleted(completion_promise)` | E8,E10-E12 | PID只说明进程；字符串可能是候选/拒收；LOOP_COMPLETE 不进 outbox。 | 0.91 |
| D5 | 业务恢复权威 | scratchpad；raw event复制；outbox+StateLedger+现有 hydration | 复用现有 cold-start repair/hydration | E12-E18 | scratchpad不可信；复制事件会重复副作用；现有边界已有 fault/idempotency证明。 | 0.96 |
| D6 | combined 是否 cleanup | cleanup后restore；选择性archive；完全保留 live runtime | 完全跳过 fresh cleanup | E2-E6,E17-E18 | cleanup后restore形成新事务与回滚风险；选择性archive容易破坏跨文件一致性。 | 0.96 |
| D7 | 无目标 worktree时 | 创建新；回退primary；拒绝 | 拒绝并提示去掉 continue或修正名称 | E7,A2 | 创建/回退都不是“续上同一 loop”。 | 0.94 |
| D8 | 并发控制 | 仅PID check；registry lock；目标worktree LoopLock全程持有 | 目标 worktree LoopLock + registry/PID先验 | E7-E9 | PID check有TOCTOU；registry锁不贯穿runner；LoopLock已有RAII模式。 | 0.90 |
| D9 | identity mismatch | warning继续；自动覆盖marker；fail-closed | fail-closed，零业务副作用 | E3,E13,E16 | 覆盖会让旧task/outbox归属漂移；warning违反可信恢复。 | 0.93 |
| D10 | torn/corrupt记录 | 全部拒绝；沿用现有 salvage；静默忽略所有错误 | 保持 torn-line salvage与genuine I/O fail-close；assessment不改变commit parser | E13-E15 | 全拒绝回归现有crash恢复；静默I/O错误会盲跑。 | 0.89 |
| D11 | preset 范围 | 只PF；逐preset适配；preset-agnostic startup + existing runtime | preset-agnostic | E14-E20 | runtime durable boundary已通用；逐preset会复制协议。 | 0.92 |
| D12 | RPC/TUI协议 | 新bootstrap variant；ManifestResume；现有Continue | 现有 Continue；parent负责目标/锁，child复用worktree path并按RunIntent跳过fresh-reuse manifest gate | E4-E5,E16-E18,E23 | 新variant无新agent行为；ManifestResume属于fresh reuse；仅在inner忽略manifest不足以避免child pre-run误拒绝。 | 0.92 |
| D13 | memory修复层 | CLI过滤；prompt后字符串删除；store visible API | prompt注入调用 `load_visible(Some(hat_id))` | E21 | CLI不覆盖自动注入；字符串后过滤易泄漏/破坏budget。 | 0.98 |
| D14 | 文档位置 | agent skill；operator docs/help；两者都写 | operator combined只写help/guide；memory skill文案若准确则不改 | E21-E22,R12 | AGENTS禁止把operator worktree控制面注入agent；现有memory skill已声明visibility。 | 0.95 |
| D15 | 模块位置 | 继续堆run.rs；core-only；core assessment + CLI orchestration模块 | 两个小模块，run.rs薄接线 | E22 | run.rs接近上限；纯core不应依赖clap/worktree选择；纯CLI会复制domain解析。 | 0.93 |

没有低于 `0.85` 的关键决策。

### 3.1 High-Level Technical Design

模式矩阵是启动语义的单一判定规则；后续代码只能消费 typed RunIntent，不能再次组合 bool 推导另一套含义。

| `--continue` | `--worktree` | `--reuse-worktree` | RunIntent | 目标不存在 | Runtime artifacts |
|---|---|---|---|---|---|
| 否 | 否 | 否 | Fresh | 不适用 | 新建/旋转fresh state |
| 是 | 否 | 否 | ContinuePrimary | 按既有primary规则拒绝 | 原地保留 |
| 否 | 是 | 是 | ReuseFresh | 创建第一个exact worktree | 旧state归档，PF可用manifest |
| 是 | 是 | 是 | ContinueReusedWorktree | **拒绝，不创建** | **原地保留，不消费manifest** |

可信恢复时序如下；`LoopHistory`只决定是否已经成功闭环，accepted-transition outbox/StateLedger只决定业务边界如何幂等恢复，两者不可互换。

```mermaid
sequenceDiagram
    actor O as Operator
    participant C as Run command
    participant W as Target worktree
    participant G as Checkpoint gate
    participant R as Existing continue runner

    O->>C: --worktree --reuse-worktree --continue
    C->>W: exact-name + Git-known + dead-PID lookup
    C->>W: acquire exclusive loop lock
    C->>G: inspect identity, current-events, scratchpad, history, outbox
    alt completion_promise already recorded
        G-->>C: AlreadyCompleted
        C-->>O: reject; remove --continue for fresh reuse
    else missing, mismatched, or genuine I/O failure
        G-->>C: Refused(reason)
        C-->>O: fail closed; no cleanup/backend start
    else eligible interrupted checkpoint
        G-->>C: Eligible(same loop id)
        C->>R: existing Continue bootstrap on same workspace
        R->>R: repair outbox-only projection, hydrate durable state
        R->>W: append one loop.resume; no fresh starting event
        R-->>O: continue until termination
    end
```

该设计不新增第二套恢复事务：U1只产生read-only verdict，U2只拥有gate/lock，U3以后全部复用现有runner。任何实现若需要把archive复制回live paths，说明偏离D3/D6，必须停止。

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
    Given worktree 名称/LoopContext id 与 current-loop-id 不一致
    When operator 启动 combined continuation
    Then 命令非零退出并报告 identity mismatch
    And 不覆盖 current-loop-id
    And 不归档任何 live artifact

  Scenario S6: mandatory checkpoint artifact 缺失或不可读
    Given current-events marker、其目标 event file、scratchpad 或 history/outbox 遇到真实 I/O 错误
    When operator 启动 combined continuation
    Then 命令 fail-closed
    And 不启动 backend、不创建 archive、不修改业务状态

  Scenario S7: 两个进程竞争同一 checkpoint
    Given 第一个 combined continuation 已持有目标 worktree loop lock
    When 第二个进程尝试相同命令
    Then 第二个进程在读取/修改 checkpoint 前被拒绝
    And 只有第一个进程可追加 resume 状态

  Scenario S8: outbox-only crash window 在重启时只修复一次
    Given durable outbox 已有 transition projection 但 StateLedger 尚未应用
    When combined continuation 冷启动两次（串行且第一次随后中断）
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
    Given memory store 含 shared、hat-A private、hat-B private
    When 为 hat-A 构建 auto-injected prompt
    Then prompt 包含 shared 与 hat-A private
    And prompt 不包含 hat-B private
    And budget 在可见集合格式化后应用

  Scenario S11: 无 private memory 时行为不变
    Given store 只含 shared memories 或 memories auto-inject 关闭
    When 构建 prompt
    Then shared 注入、budget、disabled no-op 与现有行为一致
```

---

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐层级 | 风险补充测试 | E2E |
|---|---|---|---|---|---|
| S1 | path/id/marker保持；恰一条loop.resume；无starting event/archive | `integration_resume.rs` real binary | CLI集成 | Characterization + idempotency | 是（mock backend binary） |
| S2 | 非零、稳定提示、关键文件hash不变 | `integration_resume.rs` | CLI集成 | failure atomicity | 否 |
| S3 | 多种非成功reason均eligible | `recovery_checkpoint.rs` + CLI一例 | 单元+集成 | table-driven state classification | 否 |
| S4 | 不创建 `.worktrees/<name>` | `integration_worktree_isolation.rs` | CLI集成 | negative side-effect | 否 |
| S5 | identity mismatch且marker不变 | core assessment + CLI | 单元+集成 | tamper/differential | 否 |
| S6 | mandatory missing/I/O错误fail-close | core assessment | 单元 | fault injection | 否 |
| S7 | second attach拒绝，单writer | core LoopLock test + CLI subprocess | 并发集成 | concurrency/TOCTOU | 是 |
| S8 | repair count first=1 second=0，projection等价 | `accepted_transition.rs`/state_machine tests | 模块集成 | crash-window fault injection + differential | 否 |
| S9 | 旧archive/manifest/continue tests原样绿 | existing integration suites | 回归 | differential old-vs-new | 是（既有） |
| S10 | prompt不含other-private，含shared+own | `event_loop/tests/memory_visibility.rs` | 模块集成 | confidentiality negative assertion | 否 |
| S11 | disabled/shared/budget旧断言不变 | preview/build_prompt tests | 单元/集成 | characterization | 否 |

每个测试必须同时断言主结果、副作用和不变量：不能只匹配 stderr；拒绝路径必须比较关键文件内容或不存在性；成功路径必须验证真实 event log，而非 source-text assertion。

---

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence | Unit |
|---|---|---|---|---|---|---|---|---|
| R1 | typed intent隔离combined/fresh | S1,S9 | combined/no-combined分支 | intent matrix | binary regression | 是 | E1-E5 | U2,U3 |
| R2 | exact target与identity | S4,S5 | missing/mismatch非零 | assessment identity cases | worktree binary | 否 | E7,E16 | U1,U2 |
| R3 | 独占续跑 | S7 | second process拒绝 | lock outcome | subprocess race | 是 | E8-E9 | U2 |
| R4 | 先验证后变更 | S2,S5,S6 | file/hash/archive不变 | failure matrix | CLI拒绝 | 否 | E2-E3,E13 | U1,U2 |
| R5 | 只认completion_promise | S2,S3 | success拒绝/non-success允许 | terminal table | one real CLI case | 否 | E10-E12 | U1,U2 |
| R6 | 原地续跑 | S1 | event/id/path assertions | resolve/context assertions | binary combined | 是 | E16-E18 | U3 |
| R7 | durable repair幂等 | S8 | replay equivalence | repair counts | real ledger/outbox | 否 | E13-E15 | U4 |
| R8 | preset agnostic | S1,S8,S9 | minimal configs跨两种模式 | no preset field in model | core+CLI | 否 | E14,E20 | U1,U4 |
| R9 | standalone兼容 | S9 | existing suites unchanged | rpc bootstrap | reuse/continue suites | 是 | E4-E5,E17-E20 | U3,U4 |
| R10 | memory隔离 | S10,S11 | prompt content | load_visible existing tests | real build_prompt | 否 | E21 | U5 |
| R11 | 无新格式/依赖 | S1,S8 | artifact inventory | API shape | build/Cargo diff | 否 | E13-E15,E20 | U1-U4 |
| R12 | 文档边界 | S9,S10 | help/doc drift | 不适用 | CLI help smoke | 否 | E21-E22 | U3,U5 |

---

## 7. 严格串行开发单元

```text
U1 Typed checkpoint assessment
  ↓ 完成 Red/Green/Refactor/Regression
U2 Combined intent、锁与 fail-closed gate
  ↓ 完成 Red/Green/Refactor/Regression
U3 成功原地 continuation 与前端 parity
  ↓ 完成 Red/Green/Refactor/Regression
U4 Crash/replay 与 standalone regression
  ↓ 完成 Red/Green/Refactor/Regression
U5 Memory prompt visibility
```

### U1：建立通用只读 Checkpoint Assessment

#### 1. Unit 目标

给定一个现有 worktree 和 expected loop id，返回一个 typed、preset-agnostic、无业务写的恢复资格：`Eligible`、`AlreadyCompleted` 或带稳定原因的 `Refused`。

#### 2. 对应需求与 Scenario

- Requirements：R2、R4、R5、R8、R11。
- Scenarios：S2-S6。
- Decisions：D3-D5、D9-D11、D15。
- Evidence：E3、E7、E10-E16、E22。

#### 3. 外部可观察结果

本 Unit 先提供可被 CLI 消费的 core verdict；测试可观察 completion_promise 被精确分类、非成功终止仍 eligible、identity/missing/I/O 被拒绝，且 fixture 内容不变。

#### 4. 当前行为基线

当前没有通用 checkpoint assessment；`LoopHistory::is_completed()` 会把 max/failure 等所有 LoopCompleted 都视为 completed，不能满足 R5。先写 characterization 固定该现状，再新增不复用该宽泛判定的精确 API。

#### 5. 输入与输出

- 输入：workspace path、expected loop id。
- 输出：typed assessment，包含 resolved loop id、history terminal classification、current event path和拒绝reason code。
- 错误：真实I/O/marker path无效/identity mismatch/missing mandatory state。
- 状态变化/副作用：无；不得创建目录、lock、marker、archive或规范化文件。
- 不变量：不读取preset名，不修改 outbox salvage/commit行为。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `crates/ralph-core/src/recovery_checkpoint.rs`（新增） | 无 | assessment模型、marker/path/history/outbox只读验证及unit tests | 不启动runner、不cleanup、不repair |
| `crates/ralph-core/src/lib.rs` | core模块/export | 注册必要公开类型 | 不扩大其他exports |
| `crates/ralph-core/src/loop_history.rs` | history append/read/query | 若需要，增加“最新终态+reason”只读查询和tests | 不改变现有`is_completed()`兼容语义 |
| `crates/ralph-core/src/event_loop/accepted_transition.rs` | durable outbox commit/read/repair | 仅复用现有read API；若无法表达I/O verdict，增加只读helper | 不改commit order、salvage、ack或identity |

#### 7. 可依赖能力

`LoopHistory::read_all`、`read_outbox`、`LoopContext`路径约定、现有 Memory/FileLock read semantics；本 Unit不需要CLI fake。

#### 8. 禁止依赖的未来能力

不得依赖 U2 RunIntent/lock、U3 runner wiring、U4新增replay测试或U5 memory修改；不得提前实现CLI错误文案。

#### 9. 验收测试

- `assessment_marks_only_completion_promise_as_already_completed`：table输入 completion_promise/max_iterations/max_runtime/recovery_exhausted/terminated/none；只第一项 completed。
- `assessment_accepts_matching_interrupted_checkpoint_without_writes`：完整fixture，前后递归artifact inventory和内容digest一致。
- `assessment_refuses_loop_identity_mismatch`：expected与marker不同，reason稳定，marker不变。
- `assessment_refuses_missing_current_event_target`：marker存在但target不存在。
- `assessment_refuses_real_outbox_io_error`：outbox path为目录，fail-closed。
- `assessment_preserves_torn_tail_salvage_contract`：有效entry+torn tail仍可评估，不改变文件。

运行：`cargo nextest run -p ralph-core -- recovery_checkpoint`；history/outbox targeted分别运行`cargo nextest run -p ralph-core -- loop_history`与`cargo nextest run -p ralph-core -- accepted_transition`。

#### 10. Acceptance Red

先添加 assessment tests并运行 core targeted。预期 Red 是模块/API不存在或 completion table无法得到精确 verdict。`LoopHistory::is_completed()` 把非成功 reason判真可作为 characterization 证据，但不得修改旧测试。编译环境、fixture路径错误、nextest未安装不算有效Red。

#### 11. 单元测试拆分

1. 最新有效 terminal record选择：输入多次 started/resumed/terminated/completed，输出最后终态。
2. completion reason分类：精确匹配 `completion_promise`，不做contains/大小写模糊。
3. marker path：trim、空值、绝对/逃逸路径按现有 workspace path规则拒绝。
4. identity：expected/marker相等、不等、缺失。
5. outbox：missing为空、valid、torn tail、真实I/O错误。
6. no-write：所有分支fixture digest不变。

不得mock `LoopHistory`/filesystem parser；使用TempDir真实文件。Git worktree存在性属于U2，可fake/不测。

#### 12. Red → Green → Refactor 顺序

```text
completion classification Red
→ 最小 history/query 能力
→ Green
→ identity/mandatory artifact Red
→ 最小 assessment
→ Green
→ outbox I/O/torn-tail Red
→ 复用现有 read contract
→ Green
→ no-write differential
→ Refactor typed reason/去重读取
```

#### 13. 最小实现范围

只实现恢复资格读模型及稳定reason；mandatory为loop marker、current-events及目标、scratchpad、可读history/outbox。允许missing history/outbox作为“尚无记录”的旧/早期checkpoint，但真实I/O错误拒绝。不得生成checkpoint文件、修复state、归档或更改旧API语义。

#### 14. 集成验证

联合真实 `LoopHistory` 和 accepted-transition outbox fixture；StateLedger可不构造，因为本Unit只评估。运行core targeted并确认文件digest不变。

#### 15. 风险驱动测试

- Characterization：固定`is_completed`宽泛旧语义，避免回归调用方。
- Fault injection：outbox path directory、dangling marker。
- Differential：assessment前后文件树/bytes相同。
- Parser edge：torn tail遵循既有salvage，不新增不兼容严格解析。

#### 16. 回归范围

`loop_history`、`accepted_transition`、state machine recovery targeted、`ralph-core` compile。原因：增加只读API不能改变history/outbox现有容错和commit语义。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/recovery_checkpoint.rs` | 新增生产模块+测试 | 隔离通用assessment | E10-E15,E22 |
| `crates/ralph-core/src/lib.rs` | 修改现有生产文件 | 注册模块 | E22 |
| `crates/ralph-core/src/loop_history.rs` | 修改现有生产文件 | 精确terminal只读能力 | E10-E11 |
| `crates/ralph-core/src/event_loop/accepted_transition.rs` | 条件性最小修改 | 仅若现有read接口不足 | E13-E15 |

#### 18. 完成标准

所有U1测试/集成/回归通过；无写副作用；旧`is_completed`测试不变；build/clippy针对core通过；无skip/弱断言；模块行数合规；Decision置信度不下降；可独立提交。

#### 19. 停止条件

若成功终态无法由history reason唯一识别、mandatory artifact在真实旧continue中并非必需、需要新持久格式/依赖、read_outbox语义必须破坏性改变，停止并执行：记录新证据→更新影响→比较方案→重算D3-D5/D10。

#### 20. 风险与注意事项

- 风险：history“completed”命名误导。检测：reason matrix。缓解：typed exact classifier。
- 风险：assessment无意创建lock/parent。检测：file tree differential。缓解：纯read API。
- 剩余风险：无法证明外部网络副作用；本方案只承诺Ralph持久边界幂等，不恢复不可观测外部事务。

### U2：隔离 Combined Intent、独占锁与 Fail-Closed 启动门禁

#### 1. Unit 目标

combined invocation 在任何 cleanup/backend启动前精确定位并锁定现有 worktree，消费U1 assessment；所有拒绝路径零持久业务副作用。

#### 2. 对应需求与 Scenario

R1-R5、R8、R11；S2-S7；D1-D3、D6-D9、D11-D12、D15；E1-E11、E16、E22。

#### 3. 外部可观察结果

missing、completed、identity drift、mandatory artifact错误和并发attach均非零且给出操作建议；不会新建worktree/归档/覆盖marker；单独模式仍走原分支。

#### 4. 当前行为基线

当前combined会进入reuse cleanup，且scratchpad在主root提前检查。先在`integration_resume.rs`添加真实binary characterization，确认当前失败/破坏表现与Red原因，再接线。

#### 5. 输入与输出

- 输入：RunArgs四个相关字段、resolved exact name、workspace root、prompt summary。
- 输出：RunIntent、locked LoopContext或错误。
- 错误：目标不存在/active/lock busy/assessment refused/completed。
- 状态：成功前仅允许RAII lock metadata的瞬态；不得业务写。
- 不变量：standalone reuse仍允许first-create；combined绝不first-create。

#### 6. 修改位置

| 位置 | 当前职责 | 修改边界 | 不修改 |
|---|---|---|---|
| `crates/ralph-cli/src/commands/run_recovery.rs`（新增） | 无 | RunIntent、combined target/lock/assessment/CLI error | 不运行backend、不cleanup |
| `crates/ralph-cli/src/commands/mod.rs` | commands注册 | 注册私有/公开模块 | 其他commands |
| `crates/ralph-cli/src/commands/run.rs` | 大型run编排 | 用typed intent替换相关bool分支；combined调用helper；锁guard线程化 | standalone内部协议、merge |
| `crates/ralph-cli/tests/integration_resume.rs` | continue集成 | combined拒绝/并发场景 | PF fixture职责 |
| `crates/ralph-cli/tests/integration_worktree_isolation.rs` | worktree/reuse集成 | missing target/no-create，old reuse回归 | 既有断言不削弱 |

#### 7. 可依赖能力

U1 assessment、`find_reusable_worktree_by_name`、`LoopLock::try_acquire`、`LoopContext::worktree`、existing common::ralph_bin fixtures。

#### 8. 禁止依赖的未来能力

不得在U2启动成功runner或实现loop.resume（U3）；不得修改StateLedger repair（U4）或memory（U5）；不得提前改PF manifest。

#### 9. 验收测试

- combined+missing exact worktree：非零，目录不存在。
- combined+completed：提示remove `--continue`；events/history/tasks/scratchpad/outbox bytes与archive列表不变。
- combined+identity mismatch：非零，marker不被覆盖。
- combined+main root无scratchpad但target worktree完整：门禁不得误报主root（可通过dry test/helper验证；成功runner留U3）。
- combined+target lock held：第二process非零且artifact不变。
- intent matrix：fresh/primary continue/reuse fresh/combined四态。

运行：`cargo nextest run -p ralph-cli --test integration_resume -- combined`；`cargo nextest run -p ralph-cli --test integration_worktree_isolation -- combined`；unit substring `cargo nextest run -p ralph-cli --bin ralph -- run_intent`。

#### 10. Acceptance Red

首先运行completed/missing combined binary tests。正确Red：当前命令创建/clean worktree、移走live files、或错误检查主workspace。若backend缺失先于目标gate、clap解析失败或fixture不是Git-known，不是有效Red。

#### 11. 单元测试拆分

1. RunIntent 16组合中合法组合映射（无新clap conflict）。
2. combined target absent拒绝，ReuseFresh absent返回create intent。
3. completed assessment到稳定CLI message映射。
4. lock busy在assessment/cleanup前返回。
5. explicit loop-id（若提供）与worktree id/marker冲突拒绝。

真实Git worktree lookup和flock不得mock；helper纯映射可直接unit test。

#### 12. Red → Green → Refactor 顺序

```text
intent matrix Red → typed classifier → Green
→ missing/completed Red → combined resolver+U1 gate → Green
→ lock race Red → target LoopLock贯穿返回值 → Green
→ failure atomicity Red → 调整ordering → Green
→ standalone characterization → Refactor薄化run.rs
```

#### 13. 最小实现范围

只实现combined pre-run gate和锁生命周期。必须先lock再assessment，再允许后续context准备；不得调用cleanup/manifest capture。错误消息必须说明target和下一动作。不得新增CLI flag/config/env/dependency。

#### 14. 集成验证

真实临时Git repo/worktree、registry、LoopLock和binary；backend不应在拒绝测试被调用。并发测试用持锁guard/子进程，不用sleep扩大掩盖race；使用事件/进程同步和有界超时。

#### 15. 风险驱动测试

Concurrency：双attach；Fault injection：permission/I/O；Differential：standalone reuse archive仍发生，combined拒绝不发生；Security：worktree path仍经exact-name/Git-known校验。

#### 16. 回归范围

`commands/run.rs` unit tests、integration_resume、integration_worktree_isolation、worktree core tests、loop_lock tests。原因：ordering和guard持有会影响TUI/primary/worktree选择。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/commands/run_recovery.rs` | 新增生产文件+测试 | 防止run.rs超过5000并隔离门禁 | E1-E9,E22 |
| `crates/ralph-cli/src/commands/mod.rs` | 修改 | 模块注册 | E22 |
| `crates/ralph-cli/src/commands/run.rs` | 修改 | typed branch/guard接线 | E1-E6,E9 |
| `crates/ralph-cli/tests/integration_resume.rs` | 修改测试 | combined refusal/concurrency | E17 |
| `crates/ralph-cli/tests/integration_worktree_isolation.rs` | 修改测试 | no-create/standalone differential | E19 |

#### 18. 完成标准

U2 Red真实、全部Green；拒绝路径backend未启动且artifact不变；standalone tests绿；run.rs<5000；clippy/build targeted绿；无skip/弱化；可独立提交。

#### 19. 停止条件

若LoopLock不能跨TUI parent/child持有、helper需在验证前修改业务文件、exact worktree lookup会删除registry证据、combined合法性需新用户选择，停止重决策D7-D8/D12。

#### 20. 风险与注意事项

- TOCTOU：PID检查后另一进程attach。检测双进程。缓解target lock覆盖assessment到run结束。
- parent/child自锁：检测TTY/RPC parity。缓解锁只由一个层持有并显式传递ownership，U3验证。
- lock文件是瞬态写，不计业务checkpoint变更；测试应排除锁metadata，仅比较业务artifacts。

### U3：完成原地 Continuation、RPC/TUI Parity 与 Operator Contract

#### 1. Unit 目标

资格通过的 combined invocation 在同一 worktree/loop/current-events 上执行现有 Continue bootstrap，且所有前端路径一致。

#### 2. 对应需求与 Scenario

R1、R3、R6、R8-R9、R12；S1、S3、S9；D1-D2、D5-D6、D11-D12、D14-D15；E4-E6、E14-E20、E22。

#### 3. 外部可观察结果

真实binary运行后路径和loop id不变、无reuse archive/marker rotation、恰一条loop.resume、无fresh starting event；RPC/no-TUI/TUI child都报告Continue。

#### 4. 当前行为基线

standalone continue已有E17/E18；combined当前cleanup后manifest被忽略。先复制行为断言到combined fixture，不复制实现。

#### 5. 输入与输出

- 输入：U2 locked combined context、原RunArgs/config。
- 输出：existing Continue bootstrap/termination结果。
- 错误：runner启动/repair错误原样传播并保留checkpoint。
- 状态：允许append loop started/resumed/events和正常业务transition；loop-id/current-events保持。
- 不变量：不生成ManifestResume，不fresh start，不改变auto-merge/no-auto-merge规则。

#### 6. 修改位置

- `commands/run.rs`：把locked context与continue mode正确传给direct/TUI subprocess；仅薄接线。
- `loop_runner/rpc_bootstrap.rs`：原则上不改生产逻辑，只补combined contract test；若实际Red显示metadata缺失，最小调整但保持Continue variant。
- `commands/run.rs` 的 hidden `--worktree-path` child 分支：必须从typed RunIntent得知这是ContinueReusedWorktree，并跳过仅属于ReuseFresh的PF archived manifest读取/验证；不能依赖inner稍后忽略manifest。
- `loop_runner/inner.rs`/`runner.rs`：只在真实Red证明 worktree continue未复用marker时最小修复；不得重写resume引擎。
- `integration_resume.rs`：happy/non-success/RPC combined。
- `docs/guide/cli-reference.md`、必要时`docs/guide/index.md`：说明组合语义、completed提示、standalone差异。

#### 7. 可依赖能力

U1 assessment、U2 locked context、现有resolve_loop_id/initialize_resume/cold-start/RPC protocol和mock custom backend。

#### 8. 禁止依赖的未来能力

不得依赖U4新增fault tests或U5memory；不得把manifest recovery用于combined；不得增加checkpoint命令。

#### 9. 验收测试

- complete combined happy binary：主root无scratchpad、worktree有完整checkpoint；assert same loop-id/current-events bytes path、loop.resume count=1、starting topic count不增、reuse-history count不增。
- non-success reason table中至少一个real binary（max_iterations或Interrupted）可continue。
- RPC/no-TUI：bootstrap序列化为Continue，termination可观察。
- subprocess TUI argument forwarding：`--continue`、loop id和worktree path各一次，不创建第二worktree、不双锁。
- PF worktree内存在旧的invalid/incomplete archived manifest时，combined child不执行fresh-reuse manifest gate；相同fixture去掉`--continue`后standalone ReuseFresh仍fail-closed。
- standalone regression原测试不改断言。

运行：`cargo nextest run -p ralph-cli --test integration_resume -- combined_continue`；runner seams分别运行`cargo nextest run -p ralph-cli --bin ralph -- rpc_bootstrap`、`cargo nextest run -p ralph-cli --bin ralph -- forward_prompt_args`、`cargo nextest run -p ralph-cli --bin ralph -- resolve_loop_id`和`cargo nextest run -p ralph-cli --bin ralph -- resume_branch`。

#### 10. Acceptance Red

happy combined首先Red；正确失败是live artifacts被cleanup、主root scratchpad误检、marker被旋转、loop id改变或无loop.resume。backend fixture/TTY不可控不算有效Red，TUI wiring用现有argument helper test而非依赖人工终端。

#### 11. 单元测试拆分

1. locked worktree context的resolve_loop_id等于context/marker。
2. LoopBootstrap combined为Continue，manifest不消费。
3. child args包含continue+loop-id+worktree-path且不包含worktree创建flag。
4. child RunIntent为ContinueReusedWorktree时不读取/验证PF archive manifest；ReuseFresh仍验证。
5. resume branch不调用starting-event persist。
6. lock ownership不会parent/child同时acquire。

不得mock真实event append/marker；argument纯函数可unit test。

#### 12. Red → Green → Refactor 顺序

```text
happy binary Red → context/continue接线 → Green
→ RPC/no-TUI Red → forwarding修复 → Green
→ TUI ownership Red → 单一锁owner接线 → Green
→ standalone differential → docs/help → Refactor
```

#### 13. 最小实现范围

复用现有 `resume=true`；combined context不cleanup；显式传同loop id。保持RPC enum、manifest、starting event、auto-merge现有协议。operator docs说明：combined只续未成功loop；成功loop去掉continue执行fresh reuse。

#### 14. 集成验证

真实CLI binary + temp Git worktree + custom `true`/fixture backend；读取actual current-events。RPC contract使用现有proto序列化。TUI仅测试参数/ownership seam，避免环境依赖人工TTY。

#### 15. 风险驱动测试

State-machine主路径验收；idempotency（一次bootstrap）；differential standalone；contract test RPC enum；path identity。无需live API。

#### 16. 回归范围

integration_resume、integration_worktree_isolation、loop_runner legacy recovery/misc、rpc bootstrap、run arg forwarding、worktree create/reuse/no-auto-merge/PF manifest tests、CLI help/doc drift。原因：同一编排入口跨多个frontends。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-cli/src/commands/run.rs` | 修改生产文件 | successful combined wiring | E2-E6,E16 |
| `crates/ralph-cli/src/loop_runner/rpc_bootstrap.rs` | 测试/条件性生产修改 | Continue parity | E4 |
| `crates/ralph-cli/src/loop_runner/{inner,runner}.rs` | 条件性最小修改 | 仅真实Red所需 | E14-E18 |
| `crates/ralph-cli/tests/integration_resume.rs` | 修改测试 | happy/non-success/RPC | E17 |
| `docs/guide/cli-reference.md` | 修改文档 | operator组合契约 | E1-E6 |
| `docs/guide/index.md` | 条件性修改文档 | 若需入口示例 | E22 |

#### 18. 完成标准

所有U3测试Green；真实event断言满足S1；三种frontend parity；standalone完全回归；`ralph run --help`文案正确；`scripts/check-cli-doc-drift.sh --strict`无新增drift；build/clippy绿；无agent skill误写；可独立提交。

#### 19. 停止条件

若必须新增RPC variant、改变manifest语义、旋转events、生成新loop id、改变auto-merge或修改preset才能成功，停止并重新评估D1/D6/D12。

#### 20. 风险与注意事项

- 风险：parent锁guard在spawn后提前drop。检测并发attach E2E。缓解guard生命周期绑定子进程wait。
- 风险：resume history追加`LoopStarted`造成终态分类混淆。检测U1“最新终态”规则与重复continue。
- 文档：operator-only，严禁加入`crates/ralph-core/data/*.md`。

### U4：固化 Crash-Window Repair、重复恢复与 Standalone 回归

#### 1. Unit 目标

证明combined continuation复用的 durable boundary 在outbox/ledger split、重复冷启动和普通preset下不重复副作用，并证明 standalone paths未回归。

#### 2. 对应需求与 Scenario

R7-R9、R11；S8-S9；D3-D6、D10-D12；E12-E20、E22。

#### 3. 外部可观察结果

首次cold start补齐一次projection，第二次为no-op；相同transition不重复task/flow/bus；无StateMachine配置也可continue；PF standalone manifest仍工作。

#### 4. 当前行为基线

accepted_transition/state_machine已有PMI-011、rollback、repair tests。先运行characterization；新增combined入口覆盖从CLI checkpoint到真实EventLoop cold start的缺口。若全部已覆盖，不修改事务生产代码。

#### 5. 输入与输出

输入outbox-only projection、healthy ledger、duplicate transition、state-machine enabled/disabled配置；输出修复计数/ledger snapshot/runtime summary/event counts；错误真实I/O fail-close。状态不变量是exactly-once logical materialization。

#### 6. 修改位置

- `accepted_transition.rs` tests：补combined所需edge（优先测试，不改生产）。
- `event_loop/tests/state_machine.rs`：cold-start differential。
- `integration_resume.rs`：最小真实combined crash fixture。
- U1-U3生产文件：仅当预期Red指向直接缺陷时最小修复。

#### 7. 可依赖能力

U1-U3完整combined路径；StateLedger、AcceptedTransition、EventBus observer、TaskStore真实fixtures。

#### 8. 禁止依赖的未来能力

不得依赖U5；不得新增snapshot/DB、修改preset、复制event、重设retry budget。

#### 9. 验收测试

- outbox有projection/ledger无：first repair=1，snapshot等价。
- same workspace second cold start：repair=0。
- delivered/undelivered duplicate：bus/materialize计数不增。
- state_machine disabled/no projection：continue正常，无SM delta伪造。
- genuine outbox I/O：combined fail，backend/bus未启动。
- standalone PF reuse manifest tests和archive tests保持。

运行：`cargo nextest run -p ralph-core -- accepted_transition`、`cargo nextest run -p ralph-core -- state_machine`、`cargo nextest run -p ralph-cli --test integration_resume -- checkpoint_repair`、`cargo nextest run -p ralph-cli --test integration_worktree_isolation -- reuse_worktree`。

#### 10. Acceptance Red

新增CLI-to-cold-start test应在U3后先执行。正确Red是重复projection/materialization、repair遗漏或combined绕过cold-start；若已有实现直接Green，记录characterization evidence并继续回归，不制造生产改动。环境/fixture错误不是Red。

#### 11. 单元测试拆分

repair first/second；projection present/none；duplicate delivered状态；commit failure rollback；I/O fail-close；state-machine disabled。不得mock StateLedger/outbox/EventBus核心行为。

#### 12. Red → Green → Refactor 顺序

```text
CLI crash fixture Red/Characterization
→ 仅修直接wiring缺口
→ Green
→ repair/dedupe matrix
→ Green
→ disabled/standalone differential
→ Green
→ 无生产改动则只整理测试helper
```

#### 13. 最小实现范围

优先零生产改动；若失败，只修U1-U3导致的cold-start绕过/ordering。不得重新设计AcceptedTransition、改变outbox格式、ack/commit order、history或policy语义。

#### 14. 集成验证

真实filesystem/ledger/outbox/EventLoop；CLI fixture必须进入production cold-start。PF manifest验证用既有测试，不新增preset-specific combined逻辑。

#### 15. 风险驱动测试

Fault Injection、State-Machine、Idempotency、Differential old-vs-combined；并发由U2覆盖，不重复铺开。

#### 16. 回归范围

ralph-core accepted_transition/state/state_machine/replay；ralph-cli integration_resume/worktree/runner；所有PF manifest tests；随后core+cli package tests。原因：这是最高数据完整性风险。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/accepted_transition.rs` | 新增/修改测试；生产条件性 | repair/idempotency | E13-E15 |
| `crates/ralph-core/src/event_loop/tests/state_machine.rs` | 修改测试 | cold-start differential | E14-E15 |
| `crates/ralph-cli/tests/integration_resume.rs` | 修改测试 | CLI-to-runtime crash path | E17 |
| U1-U3生产文件 | 条件性最小修复 | 仅真实Red | 对应新Evidence |

#### 18. 完成标准

repair/dedupe/fault/disabled/standalone全部Green；未削弱旧测试；无无关生产改动；build/clippy/package regression绿；Evidence更新；可独立提交。

#### 19. 停止条件

若发现未计划的外部side effect、transition identity迁移、outbox格式变化、公开API消费者扩大、修复需大事务重构，停止并将计划降为BLOCKED后重决策。

#### 20. 风险与注意事项

主要风险是测试只证明projection不证明业务副作用；必须同时观察ledger、task/flow或materialize计数和bus。剩余风险为进程在非Ralph外部系统调用中死亡，此计划不提供分布式事务保证。

### U5：修复 Memory Auto-Injection 的 Hat 可见性

#### 1. Unit 目标

当前hat的自动prompt只注入shared和own-private memories，绝不注入other-private，同时保持disabled/budget/shared行为。

#### 2. 对应需求与 Scenario

R10、R12；S10-S11；D13-D14；E21-E22。

#### 3. 外部可观察结果

hat-A prompt不含hat-B private文本；hat-A own/private与shared仍存在；CLI list/search/prime现有权限不变。

#### 4. 当前行为基线

`load_visible`及CLI测试已有正确行为；prompt自动注入调用`load()`。先添加prompt-level泄漏reproducer，确认other-private字符串真实出现且测试到达production build_prompt。

#### 5. 输入与输出

输入memories.enabled/inject/budget、store records、HatId；输出prompt prefix。错误读取仍安全降级为空（保持现有）；无memory写。过滤后再format/truncate。

#### 6. 修改位置

- `event_loop/prompt_injection.rs`：单点改用visible load，保留日志/budget/order。
- `event_loop/tests/memory_visibility.rs`（新增）与`tests/mod.rs`：真实build_prompt测试。
- `integration_memory.rs`：仅运行既有CLI visibility回归，除非发现缺口才补测试。
- `ralph-tools-memories.md`：现有文案已准确，原则上不改；实现后反查准确性。

#### 7. 可依赖能力

`MarkdownMemoryStore::load_visible`、Memory::is_visible_to、existing common EventLoop fixtures。

#### 8. 禁止依赖的未来能力

无未来Unit；不得扩展为memory排名/存储重构，不得修改CLI授权。

#### 9. 验收测试

- shared+A-private+B-private，hat-A build_prompt：contains shared/A，not B。
- hat-B对称。
- only-shared行为与旧格式一致。
- inject disabled不含任何memory data。
- 小budget在过滤后计算：B-private不能先占预算导致visible内容消失。
- read failure保持无memory、不中断loop。

运行：`cargo nextest run -p ralph-core -- memory_visibility`；preview回归分别运行`cargo nextest run -p ralph-core -- preview_api`与`cargo nextest run -p ralph-core -- preview_characterization`；CLI回归运行`cargo nextest run -p ralph-cli --test integration_memory`。

#### 10. Acceptance Red

先运行hat-A leak reproducer；正确Red是prompt包含B-private或visible内容被其budget挤掉。fixture未启用auto-inject、未走build_prompt、字符串格式错误不算有效Red。

#### 11. 单元测试拆分

shared、owner match、owner mismatch、budget after filter、disabled、read error。真实store文件，不mock visibility规则；可以用TempDir隔离。

#### 12. Red → Green → Refactor 顺序

```text
cross-hat leak Red
→ load_visible(Some(hat_id))最小替换
→ Green
→ budget-order Red/Green
→ disabled/read-error regression
→ Refactor日志计数准确性
```

#### 13. 最小实现范围

仅替换自动注入的数据加载入口，并确保日志的memory count指可见集合。不得改变Memory格式、CLI命令、limits、budget算法或skill injection顺序。

#### 14. 集成验证

真实EventLoop build_prompt +真实MarkdownMemoryStore；CLI integration验证同一visibility policy。不得只测`load_visible`（它已存在且不是泄漏入口）。

#### 15. 风险驱动测试

Security/confidentiality negative assertion；differential shared/disabled；budget boundary。无需E2E/browser。

#### 16. 回归范围

memory_store/memory_parser/memory model、event_loop preview/build_prompt、integration_memory、core package。原因：prompt顺序和budget可能受过滤集合变化影响，这是预期安全变化，shared路径不能变。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/event_loop/prompt_injection.rs` | 修改生产文件 | 使用已有visible view | E21 |
| `crates/ralph-core/src/event_loop/tests/memory_visibility.rs` | 新增测试 | production prompt泄漏reproducer | E21 |
| `crates/ralph-core/src/event_loop/tests/mod.rs` | 修改测试注册 | 收集新tests | E22 |
| `crates/ralph-core/data/ralph-tools-memories.md` | 条件性文档修改 | 仅若反查发现不准确 | E21-E22 |

#### 18. 完成标准

leak Red→Green；shared/own/private/budget/disabled/read-error全绿；CLI权限回归；skill文档反查；doc drift（若文档受影响）、build/clippy/core+cli回归；无skip/弱断言；可独立提交。

#### 19. 停止条件

若HatId无法对应owner_hat_id、prompt preview与live path使用不同数据管线、修复需改变memory持久格式/CLI授权，则停止并重决策D13。

#### 20. 风险与注意事项

这是隐私边界缺陷；日志不得输出memory content或other-private id。过滤必须发生在format/budget前。剩余风险：shared memory本来就对所有hat可见，非本Unit改变范围。

---

## 8. Unit 串行依赖图

```text
U1 Typed checkpoint assessment
  ↓ U2消费typed verdict，不能先写CLI自有解析
U2 Combined gate与独占锁
  ↓ U3只能在门禁通过并持锁后启动真实continue
U3 原地continuation与frontend parity
  ↓ U4必须对最终production路径做crash/replay证明
U4 Durable regression
  ↓ U5虽功能独立，固定最后执行以避免安全修复干扰恢复回归归因
U5 Memory visibility
```

- U1→U2：U2使用U1的唯一checkpoint分类，避免CLI/core双源。
- U2→U3：未先建立锁和fail-closed ordering，happy path会重新引入竞态/破坏。
- U3→U4：U4必须测试真实最终启动链，不能对未接线helper做伪集成。
- U4→U5：逻辑上独立但仍严格串行；这样恢复机制基线先封闭，再处理prompt隐私，失败归因清晰。
- 每个Unit禁止提前实现下一Unit；若某Red暴露未来行为，只记录，不顺手实现。

---

## 9. 执行命令清单

以下命令均在仓库根目录运行；任何required命令失败都不得进入下一步。substring若未收集到预期test，先用nextest list确认名称，不得改用裸cargo test。

| 时机 | 命令 | 目的 | 预期 |
|---|---|---|---|
| 环境前置 | `cargo nextest --version` | 验证钉死工具 | `cargo-nextest 0.9.140` |
| U1 | `cargo nextest run -p ralph-core -- recovery_checkpoint` | assessment TDD | 目标tests Green |
| U1回归 | `cargo nextest run -p ralph-core -- loop_history` | history兼容 | Green |
| U1/U4 | `cargo nextest run -p ralph-core -- accepted_transition` | outbox/repair | Green |
| U2 | `cargo nextest run -p ralph-cli --bin ralph -- run_intent` | CLI pure wiring | Green |
| U2/U3 | `cargo nextest run -p ralph-cli --test integration_resume -- combined` | combined binary | Green |
| U2/U3回归 | `cargo nextest run -p ralph-cli --test integration_worktree_isolation` | fresh reuse/worktree | Green |
| U3 | `cargo nextest run -p ralph-cli --bin ralph -- rpc_bootstrap` | frontend contract | Green |
| U3 | `cargo nextest run -p ralph-cli --bin ralph -- resolve_loop_id` | loop-id seam | Green |
| U3 | `cargo nextest run -p ralph-cli --bin ralph -- resume_branch` | resume event seam | Green |
| U3 | `cargo nextest run -p ralph-cli --bin ralph -- forward_prompt_args` | TUI child argv seam | Green |
| U4 | `cargo nextest run -p ralph-core -- state_machine` | cold-start/disabled | Green |
| U5 | `cargo nextest run -p ralph-core -- memory_visibility` | privacy acceptance | Green |
| U5回归 | `cargo nextest run -p ralph-cli --test integration_memory` | CLI visibility | Green |
| 文档/CLI | `cargo run -p ralph-cli -- run --help` | help smoke | combined契约清楚 |
| 文档drift | `bash scripts/check-cli-doc-drift.sh --strict` | command/docs一致 | exit 0 |
| 格式 | `cargo fmt --all -- --check` | Rust格式 | exit 0 |
| Lint | `cargo clippy` | 仓库规定的lint/typecheck入口 | exit 0 |
| Build | `cargo build` | 仓库规定的build入口 | exit 0 |
| Core package | `cargo nextest run -p ralph-core` | core回归 | Green |
| CLI package | `cargo nextest run -p ralph-cli` | CLI回归 | Green |
| 最终全量 | `./scripts/run-tests.sh` | 项目规定两阶段nextest+doctest | 全绿 |
| Flake兜底，仅竞态/时序flake | `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | 判定flake还是真失败 | serial仍失败则必须修复 |

不得用裸 `cargo test -p ralph-cli`。测试子进程若显式启动ralph，继续使用`common::ralph_bin()`自动scrub agent runtime env；模拟agent时先scrub再显式注入。

---

## 10. 最终质量门禁

- S1-S11全部通过并可追踪到R/Unit。
- checkpoint拒绝路径证明无archive、无marker覆盖、无backend启动、关键文件bytes不变。
- 成功路径证明same path/id/current-events、恰一条loop.resume、无fresh starting event。
- 双进程竞争只有一个writer。
- outbox-only repair first=1/second=0；materialize/task/flow/bus不重复。
- standalone primary continue、fresh reuse、PF manifest、fresh worktree、RPC/TUI/no-TUI均回归。
- memory other-private在live prompt中不可见；shared/own-private/budget/disabled不回归。
- 所有unit/integration/contract/fault/differential tests通过。
- fmt、clippy `-D warnings`、build、CLI help、doc drift、core/cli package、`./scripts/run-tests.sh`通过。
- 无新增skip/ignore/only、无削弱断言、无无解释snapshot/golden变化。
- Cargo.toml/Cargo.lock无新依赖变化；无新持久checkpoint格式。
- 未改presets/schemas/manifest/zsh；若实际变更触及这些，视为超范围并停止。
- `commands/run.rs`和所有源码<5000行；新模块职责清晰。
- operator-only内容未进入agent-injected skills；memory skill反查准确。
- 所有Decision仍≥0.85，无BLOCKED事项；实际变更不超计划。
- U1→U5每个均有Acceptance Red/Unit Red/Green/Refactor/Integration/Regression/Close证据并可独立提交。

---

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是Roadmap吗 | 是 | 5个外部行为Unit，均有位置、Red、最小边界和验收 |
| Executor是否仍需做关键设计决策 | 否 | D1-D15已选型，条件性文件修改只允许由预期Red直接触发 |
| 所有文件和接口是否有代码库证据 | 是 | 现有路径见E1-E22；新增路径明确标注计划新增 |
| 所有关键决策置信度是否≥0.85 | 是 | 最低D10/D12=0.89 |
| 是否存在未处理的低置信度假设 | 否 | 待验证假设为空；有明确停止条件 |
| 每个Unit是否只有一个可观察行为 | 是 | assessment、gate、success、durable replay、memory isolation各一项 |
| 每个Unit是否可以独立验证 | 是 | 各自targeted nextest与完成标准 |
| 每个Unit是否有真实Red | 是 | 每Unit列出目标测试、缺失能力与无效Red |
| 每个Unit是否包含回归范围 | 是 | 各Unit第16节 |
| 是否存在未来Unit依赖 | 否 | 仅依赖已完成前置Unit，第8节线性 |
| 是否存在泛化任务描述 | 否 | 所有动作绑定具体行为/模块/断言 |
| 所有Scenario是否可追踪到测试和Unit | 是 | 第5、6节矩阵 |
| 所有关键决策是否有Evidence | 是 | D1-D15均引用E/A |
| 计划是否可以严格串行执行 | 是 | U1→U2→U3→U4→U5 |

### Sources & References

- `CONCEPTS.md`：Accepted Transition、Recovery Intent、artifact-first handoff、orchestration knowledge术语。
- `docs/achieved/plan/2026-08-03-004-feat-parallel-forge-execution-resume-plan.md`：PF专用manifest边界与“不恢复LLM上下文”。
- `docs/achieved/plan/2026-08-15-2211-fix-state-machine-transaction-boundary-plan.md`：outbox/ledger split与fault regression。
- Git history：`9e102015`、`55d46dd8`、`1be0eff9`、`59c0dcf0`等相关恢复修复。
- 本计划E1-E22列出的当前源码与测试；代码/可执行测试优先于历史文档。
