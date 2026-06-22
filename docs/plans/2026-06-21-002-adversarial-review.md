## 审查概要

- **Commit/PR**: `ab72546` (U0) → `5e7dfcf` (U10)  
- **审查范围**: `crates/ralph-core/src/state/*`, `validation/*`, `correction/*`, `event_loop/mod.rs`, `crates/ralph-cli/src/policy_check.rs`, `crates/ralph-core/src/diagnosis/reporter.rs`, `crates/ralph-core/src/hat_handoff/allocator.rs`  
- **总体结论**: **REQUEST_CHANGES**  
- **风险等级**: **HIGH**（当前 feature flag 默认关闭，生产无即时风险；但 flag 开启后存在 P0 级数据丢失和语义退化风险）

---

### P0 — 阻断问题（当前代码中无直接触发路径，但 feature flag 开启后即成为 P0）

> **对抗性声明**：以下问题在默认关闭的 feature flag 保护下未实际触发，但代码已经合入主干。一旦 flag 默认值翻转或运维人员手动开启，将直接引发数据丢失或系统崩溃。按“最坏情况”原则，这些问题在架构上属于 P0 级债务，必须在 flag 默认开启前修复。

1. **【数据持久化】`persist_commit` 非原子写入 + `sync_all` 错误被静默忽略，crash 后 commit log 可能损坏**
   - **位置**: `crates/ralph-core/src/state/ledger.rs:485-491`
   - **详细分析**: `persist_commit` 使用 `OpenOptions::append(true).open()` 直接追加 JSONL 行，随后 `f.sync_all().ok()` 将 `sync_all` 的 `Err` 转为 `()` 丢弃。若进程在 `write_all` 与 `sync_all` 之间崩溃，或 `sync_all` 本身失败（磁盘满、权限变更、网络文件系统断开），内核页缓存中的数据可能丢失，导致文件尾部出现**不完整的 JSONL 行**。当 `replay_from_disk` 随后解析时，会在断行处触发 `LedgerError::Parse`，整条 log 从断点之后的所有 commit 被丢弃。commit log 被设计为“第一级持久事实源”，这种写入模式违反了 append-only log 的 durability 契约。
   - **最坏情况**: 生产环境崩溃恢复后，`ledger.jsonl` 尾部损坏，operator 被迫选择截断文件（丢失最近状态）或拒绝启动。
   - **修复建议**: 采用 `temp-file + rename` 原子写模式（每次 commit 写新文件再 `rename`），或至少将 `sync_all` 错误向上传播为 `LedgerError::Io`，让调用者感知写入失败。
   - **验证方式**: 构造一个大于 `PIPE_BUF`（Linux 4KB）的 `CommitDelta::TaskInserted`（含大 `description`），在 `write_all` 后注入 `libc::kill(getpid(), SIGKILL)`，观察重启后 `replay_from_disk` 是否报错。

2. **【状态恢复】`replay_from_disk` 的 `FIX-10` 逻辑在多 loop 工作空间会恢复错误的 `iteration`**
   - **位置**: `crates/ralph-core/src/state/ledger.rs:292-347`
   - **详细分析**: `replay_from_disk` 在 replay 结束后用 `snapshot.iteration = iterations.iter().copied().max().unwrap_or(0)` 恢复迭代计数。`ledger.jsonl` 文件**从不截断**，同一工作空间内多次 loop（或 resume）产生的 commit 会累积。旧 loop 的 commit 可能携带更高的 `iteration`（例如旧 loop 运行到 iteration 100），新 loop 的 commit 从 iteration 0 开始。FIX-10 取 `max` 会把新 loop 的 `snapshot.iteration` 错误地恢复为 100，导致 `max_iterations` 检查、iteration-based dedup 等全部失效。
   - **最坏情况**: 用户 resume 一个 loop 后，新 loop 在第 1 次迭代就被判定为 `MaxIterations` 终止，或 iteration-based 去重逻辑认为所有事件已见过。
   - **修复建议**: 在 loop 边界（启动/终止）写入 `SnapshotReset` delta 或截断/轮转 ledger 文件；replay 时只读取最后一次 `SnapshotReset` 之后的记录。
   - **验证方式**: 在工作空间内连续运行两次 loop，第一次 iteration 到达 5，第二次从 0 开始；replay 后检查 `snapshot.iteration == 0` 而非 `5`。

---

### P1 — 严重问题

1. **【架构债务】`replay_from_disk` 从未在生产代码中被调用 — 持久化等于没持久化**
   - **位置**: `crates/ralph-core/src/state/ledger.rs:292`（定义）vs `event_loop/mod.rs`（无调用点）
   - **详细分析**: `build_state_ledger_from_env` 使用 `StateLedger::new(workspace, true)` 创建全新空 ledger。`replay_from_disk` 仅在单元测试中被调用。这意味着 R4（启动/恢复时从磁盘重建 `LedgerSnapshot`）**完全未实现**。commit log 被写入磁盘但不被读取，crash 后状态全部丢失，ledger 退化成了纯内存结构加无意义磁盘写放大。
   - **修复建议**: 在 `build_state_ledger_from_env` 中调用 `StateLedger::replay_from_disk` 并处理 `LedgerError`；或在 `StateLedger::new` 中自动 replay。

2. **【架构债务】`ValidationPipeline` 未接入主事件循环 `process_parse_result`**
   - **位置**: `crates/ralph-core/src/validation/pipeline.rs`（定义） vs `crates/ralph-core/src/event_loop/mod.rs`（无调用）
   - **详细分析**: `ValidationPipeline::from_config` 和 `validate_with_preview` 仅被 `ralph-cli/src/policy_check.rs`（CLI 路径）和单元测试调用。主事件循环的 `process_parse_result` 仍然使用 legacy gate 栈（`event_origin::validate_event_origin` → `event_policy::validate_event` → `execution_contract::validate_execution_contract` 等）。U4 的“统一验证”是**库层面**的统一，而非**runtime 层面**的统一。这导致：
     - CLI 和 runtime 的验证逻辑仍可能分叉（U6 已知 14 条测试失败 gap）
     - `ValidationRule` 的纯函数设计未在实际热路径上验证性能
   - **修复建议**: 在 `process_parse_result` 中根据 feature flag 调用 `ValidationPipeline`，并确保回滚逻辑与 legacy 路径一致。

3. **【语义退化】`HatHandoffRule` 对 macro-edge 事件完全透传，不执行任何校验**
   - **位置**: `crates/ralph-core/src/validation/rules_hat_handoff.rs:47-66`
   - **详细分析**: 当 `protocol_view.is_macro_edge(topic)` 返回 `true` 时，规则直接返回 `ValidationResult::accept()`，注释说明 “U6 will plumb the path... Until then the rule produces a structured passthrough”。但 U6 已经合入，且该透传行为在 feature flag 开启时会让所有 macro-edge handoff 校验**完全失效**。这与计划 R5/R6/R7 的要求（统一 gate、禁止绕过）相矛盾。
   - **修复建议**: 接入 `hat_handoff::validator::validate_artifact` 和 `HandoffIndex`，或至少返回 `ValidationResult::reject` 并明确标记为 `not_yet_implemented`。

4. **【语义退化】`OriginRule` 使用空 `HatRegistry`，origin 校验退化为 hatless 模式**
   - **位置**: `crates/ralph-core/src/validation/rules_origin.rs:39-46`
   - **详细分析**: `OriginRule::registry` 返回 `Arc::new(HatRegistry::default())`，而 `HatRegistry::default()` 是空注册表。`event_origin::validate_event_origin` 对空注册表采用 solo/hatless 模式，**接受所有事件**。这意味着在 unified pipeline 中，origin guard 完全不起作用，任何 hat 都可以发布任何 topic。与 legacy 路径的语义严重不一致。
   - **修复建议**: 将 `HatRegistry` 注入 `ProtocolView` 或 `ValidationPipeline`，在 `from_config` 阶段从 `EventLoopConfig` 构建真实注册表。

5. **【连锁反应】`RejectionRecord::retry_key` 与 `CorrectionContext::retry_key` 格式不匹配，跨重启计数归零**
   - **位置**: `crates/ralph-core/src/state/recovery_log.rs:101-103` vs `crates/ralph-core/src/event_loop/rejection.rs:273-282`
   - **详细分析**: 
     - `RejectionRecord::retry_key()` = `hat:topic:reason_code`（例如 `executor:work.done:origin:missing_field`）
     - `Rejection::compute_retry_key()` = `stage:source:topic:violation_class`（例如 `origin:executor:work.done:missing_field`）
   - `emit_correction_context` 使用 `CorrectionContext::retry_key`（后者格式）写入 `recovery.jsonl`，但 `retry_count_for` 读取时使用 `RejectionRecord::retry_key()`（前者格式）进行匹配。两者**永远不会匹配**。这意味着如果未来调用 `retry_count_for` 来恢复跨重启的 retry 计数，结果永远是 0，R11 的“3 次升级 human.guidance”机制在重启后失效。
   - **修复建议**: 统一 `retry_key` 格式；或让 `emit_correction_context` 写入 `record.retry_key` 字段时使用与 `RejectionRecord::retry_key()` 一致的格式。

6. **【功能缺口】`maybe_escalate_to_human_guidance` 从未在生产代码中被调用**
   - **位置**: `crates/ralph-core/src/correction/mod.rs:563-581`
   - **详细分析**: R11 的“同一 hat+reason_code 短窗口内 ≥ 3 次升级 `human.guidance` / `loop.suspend`”逻辑已实现，但仅在单元测试中被调用。主事件循环的 policy rejection 路径仍调用 legacy `publish_policy_rejection_resume`（或等效逻辑），从未调用 `emit_correction_context` 和 `maybe_escalate_to_human_guidance`。
   - **修复建议**: 在 `process_parse_result` 的 rejection 处理分支中接入 `emit_correction_context` + `maybe_escalate_to_human_guidance`，或标记为 U7a follow-up 并在 flag 默认开启前完成。

7. **【状态漂移】`LedgerSnapshot::apply_delta` 大量 variant 是 no-op，replay 丢失状态**
   - **位置**: `crates/ralph-core/src/state/snapshot.rs:302-448`
   - **详细分析**: `ReviewStepUpdated`, `HandoffTrackerUpdated`, `FlowLifecycleUpdated`, `HandoffAccepted` 等 variant 在 `apply_delta` 中没有任何操作。这意味着即使 commit log 记录了这些事件，replay 后 `snapshot.review_step_tracker`、`snapshot.handoff_tracker`、`snapshot.flow_lifecycle` 等仍保持 `default()` 状态。这些状态在 legacy 路径中驱动了重要的 loop 行为（如 step 终端判断、handoff 超时）。
   - **修复建议**: 为每个 no-op variant 实现对应的 `apply_delta` 逻辑；或添加 `compile_error!` / `todo!` 防止这些 commit 被错误地生成。

8. **【性能风险】`StateLedger::commit` 每次 clone 整个 `LedgerSnapshot`**
   - **位置**: `crates/ralph-core/src/state/ledger.rs:245`
   - **详细分析**: `let prior_snapshot = self.snapshot.clone();` 在每次 commit 时深拷贝整个 `LedgerSnapshot`（包含所有 `HashMap`、`Vec<Task>`、`ReviewStepTracker` 等）。在高频事件场景（如 wave 模式大量 `work.ready` 事件）中，这会导致明显的内存分配压力和 GC 延迟。
   - **修复建议**: 按受影响子结构进行快照（如仅拷贝 `tasks` 和 `progress`），或使用 `im::HashMap` 等持久化数据结构实现 O(1) 快照。

9. **【性能风险】`StepHandoffRule` 每次 clone `progress` + `tasks`**
   - **位置**: `crates/ralph-core/src/validation/rules_step_handoff.rs:47-48`
   - **详细分析**: `let progress = ledger_snapshot.progress.clone(); let tasks: Vec<Task> = ledger_snapshot.tasks.clone();` 在每次 validation 时深拷贝两个集合。`check_alignment_with_snapshot` 只读取不修改，完全可以通过 `&ProgressSnapshot` + `&[Task]` 借用避免拷贝。
   - **修复建议**: 修改 `check_alignment_with_snapshot` 签名为借用引用。

---

### P2 — 建议

1. **【命名误导】`ValidationResult::accept()` 硬编码 `stage: Origin`**
   - **位置**: `crates/ralph-core/src/validation/result.rs:45-52`
   - **说明**: `accept()` 返回的 `ValidationResult` 的 `stage` 字段永远是 `ValidationStage::Origin`。当 post-commit rule（如 `ExecutionContractRule`）调用 `ValidationResult::accept()` 时，返回的 `stage` 却是 `Origin`，造成诊断日志误导。建议 `accept()` 接受可选 `stage` 参数，或每个 rule 用 `ValidationResult { accepted: true, stage: Self::name(), .. }` 手动构造。

2. **【测试缺口】`ValidationPipeline::validate_pre_commit` 使用空 `ProtocolView`**
   - **位置**: `crates/ralph-core/src/validation/pipeline.rs:185-208`
   - **说明**: `validate_pre_commit`（无 view 版本）构造 `ProtocolView::default()` 传入。`RequiredFieldsRule` 依赖 `ProtocolView` 的 schema 配置，空 view 会导致 required-fields 校验失效。该方法的文档说 “PreCommit rules are designed to operate without a fully-loaded ProtocolView”，但 `RequiredFieldsRule` 在 `from_config` 的 pipeline 中被列为 pre-commit rule。这意味着调用 `validate_pre_commit` 的代码路径会 silently 跳过 required-fields 检查。建议删除该方法，强制所有调用者通过 `validate_pre_commit_with_view` 提供真实 view。

3. **【维护成本】`apply_counter_change` 使用字符串匹配分发 counter**
   - **位置**: `crates/ralph-core/src/state/snapshot.rs:569-597`
   - **说明**: 用 `match counter { "iteration" => ..., "hat_handoff_seq" => ... }` 分发 16+ 个 counter。拼写错误会导致 no-op（虽然注释说“未知 counter 是 best-effort no-op”），但这会 silently 丢失状态更新。建议用 enum 替代字符串，或在 `CommitDelta::CounterChanged` 中直接存 `enum Counter { Iteration, HatHandoffSeq, ... }`。

4. **【维护成本】`StateLedger::snapshot_mut()` 是 commit log 不变性的 footgun**
   - **位置**: `crates/ralph-core/src/state/ledger.rs:187-189`
   - **说明**: 文档说 “Callers that go through this path must skip the on-disk write”，但没有任何编译期或运行期检查来 enforce 这一点。调用者可以直接 `snapshot_mut().iteration = 100` 然后 `commit()`，此时 `commit.iteration` 会记录为 100，但没有任何 `CounterChanged` delta 记录这个变更，导致 replay 逻辑不一致。

5. **【死代码】`CommitDelta::SnapshotReset` 从未被生产代码生成**
   - **位置**: `crates/ralph-core/src/state/commit.rs:184`
   - **说明**: 该 variant 的文档说 “reserved for U3 migration”，但 U3 已合入且未使用。建议删除或标记为 `#[deprecated]`。

6. **【测试设计】环境变量 feature flag 导致测试隔离失败**
   - **位置**: `crates/ralph-core/src/preset/engine/protocol.rs` 等处
   - **说明**: U10 报告已识别：2 条测试在 `UNIFIED_PROTOCOL_VIEW=1` 时因共享进程 env 失败。`std::env::set_var` 是全局副作用，并发测试互相污染。建议用 `serial_test` 或显式参数化 API 替代 env var 读取。

7. **【已知债务】U6 unified pipeline 14 条 CLI 测试 gap**
   - **位置**: `crates/ralph-cli/src/policy_check.rs:740`
   - **说明**: `run_policy_check_unified` 使用 `LedgerSnapshot::cold_start()` 而不读 `events.jsonl` 历史，导致 `business_after_terminal` 和 `duplicate_terminal` 拒绝行为与 legacy 不一致。这是 U10 已记录的已知 gap，必须在 `UNIFIED_POLICY_CHECK` 默认开启前修复。

8. **【安全/维护】`resolve_handoff_path` 的 regenerate 策略会覆盖 agent 提供的文件**
   - **位置**: `crates/ralph-core/src/state/ledger.rs:419-466`
   - **说明**: 当 `validate_artifact` 失败时，`resolve_handoff_path` 调用 `write_skeleton(..., true)` 强制覆盖 canonical path。如果 `validate_artifact` 存在误报（例如对合法但非标准格式的 handoff 文件返回 `Err`），agent 的劳动成果会被 runtime 生成的 skeleton 覆盖。虽然 path 被 `resolve_jailed` 限制在 repo 内，但破坏性覆盖行为应至少增加 `warn!` 级别日志和确认机制。

---

## 兼容性评估

- **API 变更**: 新增 `StateLedger`、`LedgerSnapshot`、`ValidationPipeline`、`CorrectionContext` 等公共类型，但均在 `ralph-core` 内部 crate 或新增模块中，未改变现有 `EventLoop`、`LoopState` 的公共 API 签名。legacy 路径的函数签名保持不变（`build_task_resume_payload` 等标记为 `#[deprecated]` 但未删除）。
- **数据格式变更**: 新增 `.ralph/ledger.jsonl` 和 `.ralph/recovery.jsonl` 文件格式。旧工作空间无这些文件时行为不变（冷启动）。`ledger.jsonl` 的 `CommitDelta` 使用 `#[serde(tag = "kind", rename_all = "snake_case")]`，未来添加新 variant 时需注意 forward-compatibility。
- **依赖变更**: 无新增外部依赖。
- **回滚安全性**: 由于 feature flag 默认关闭，回滚到旧版本只需关闭 flag（删除 env var）。但若用户已开启 flag 并写入 `ledger.jsonl`，旧版本代码无法读取该文件，会将其视为普通文件忽略，不会破坏数据，但状态恢复会降级为 legacy 路径（从 `tasks.jsonl` / `progress.md` 恢复，可能丢失 ledger 特有的 rejection history 等）。

## 测试充分性评估

- **新增测试覆盖**: 
  - `state/tests.rs` (U1): 覆盖了 commit、rollback、replay、feature flag no-op、U5 handoff artifact 生成。
  - `validation/tests.rs` (U4): 覆盖了 pre/post commit 规则、stage 字符串匹配、`ValidationReport` 聚合。
  - `correction/mod.rs` (U7): 包含 32 个单元测试，覆盖 `CorrectionContext` 构造、渲染、`RetryCounter` 阈值、`human.guidance` 发布。
  - BDD scenarios (U9): 新增 5 个 YAML scenario，覆盖 deterministic correction、3 次升级、auto handoff、diagnose from ledger、CLI/runtime parity。
- **缺失测试场景**:
  - `persist_commit` 的 crash 安全测试（用 `kill -9` 或模拟 partial write）。
  - `replay_from_disk` 在多 loop 工作空间的 iteration 恢复测试。
  - `ValidationPipeline` 在 **真实 `process_parse_result` 热路径** 上的集成测试（当前仅 CLI 和孤立 unit test）。
  - `LedgerSnapshot::apply_delta` 的 no-op variant 在 replay 后的状态一致性测试（如 `ReviewStepUpdated`  replay 后 tracker 是否等价）。
  - `retry_key` 格式端到端一致性测试（写入 `recovery.jsonl` → 读取 → `retry_count_for` 计数）。
- **回归风险**:
  - 默认 flag 关闭时，5075/5075 测试通过，BDD/smoke/doctest 全绿，回归风险极低。
  - 开启所有 flag 时，16 条 CLI 测试失败（14 条 U6 gap + 2 条 env 隔离），无 runtime 崩溃或 smoke/BDD 失败。

## 对抗性审查声明

> 本审查基于对抗性原则执行，已排查语义欺骗、隐藏副作用、边界漏洞和连锁反应风险。以下对抗性发现值得特别注意：
> 
> 1. **语义欺骗**: `HatHandoffRule` 的 `validate` 方法表面上是“hat-handoff gate”，实际对 macro-edge 事件完全透传，属于“命名做 A，实际做 B”的欺骗性代码。
> 2. **隐藏副作用**: `StateLedger::snapshot_mut()` 允许调用者绕过 commit log 直接修改 snapshot，破坏“snapshot 是 commit log 投影”的不变性，且无任何运行期检查。
> 3. **假设开发者犯错**: `OriginRule` 的 `registry()` 方法使用空 registry，若未来某个开发者误以为 unified pipeline 已 production-ready 并 flip feature flag，origin guard 将完全失效。
> 4. **连锁反应推演**: `ValidationPipeline` 未接入 `process_parse_result`，但 `policy_check.rs` 已使用它。这意味着 CLI `--policy-check` 和 runtime loop 的校验结论已分叉，随着 future delta 的积累，两者会越离越远。
> 5. **极端条件注入**: `persist_commit` 在大 commit 行（>4KB）+ 进程崩溃场景下会产生断行，当前无测试覆盖此场景。
> 6. **回滚安全评估**: 旧版本不读 `ledger.jsonl`，回滚安全；但 `ledger.jsonl` 和 `recovery.jsonl` 一旦产生，旧版本不会清理它们，磁盘空间会缓慢增长。
> 7. **隐式契约审查**: `FIX-10` 假设“所有 commit 的 `iteration` 字段是单调的或至少最大值为最终值”，但如果支持 loop 边界截断/轮转，该假设会被打破。
