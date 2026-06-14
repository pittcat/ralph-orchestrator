---
date: 2026-06-14
plan_id: 2026-06-14-003
topic: ce-executor-isolated-agent-output-governance
status: active
---

# ce-executor-isolated Agent Output Governance 实施计划

## 1. 背景与目标

### 1.1 背景

`ce-executor-isolated` preset 在运行期反复出现四类失稳问题：

1. **review-synthesizer 不发射**：agent 自己数 wave 事件容易数错/漏数，导致 loop 饿死在 review 汇总阶段。
2. **源码树被 ephemeral 文件污染**：agent 把 `scratchpad.md` / notes 写到 `crates/` 等源码目录，触发无意义 review wave 和 P0 finding。
3. **CLI 写入前不 enforce `topic_deny_rules`**：此问题已由 `2026-06-14-001` 计划覆盖，本计划不再重复。
4. **coordinator 预创建未来 task 并标 failed**：plan 把多 U 塞进一个 Step 时，runtime 默认按 Step 批量建 task，导致 U2-U4.5 任务状态错误。
5. **hard gate 后 hat 路由漂移**：`missing_event_gate` 或 policy rejection 触发后，注入的 `task.resume` 没有明确 target hat，下一轮可能激活错误的 hat。

### 1.2 目标

本计划针对需求文档 `docs/brainstorms/2026-06-14-ce-executor-isolated-agent-output-governance-requirements.md` 中的 **R1、R3、R4、R5** 实施机制级修复，坚持**机制优先于编排补丁**原则：

- 不新增 preset 提示词 workaround；
- 在 CLI 写入、runtime 注入、产物隔离、task 生命周期、hat 路由五个卡点上加硬规则；
- 所有改动必须通过 `cargo nextest run --workspace --exclude ralph-e2e`。

## 2. 范围

### 2.1 本次覆盖

| 需求 | 名称 | 核心解决点 |
|---|---|---|
| R1 | Runtime 向 review-synthesizer 注入 wave 上下文 | synthesizer 激活时自动拿到 `wave_id` / `wave_total` / `received_count` / `missing_dimensions`，无需 agent 自己数 |
| R3 | Ephemeral 文件隔离机制 | agent 写到源码树的 `scratchpad.md` 类文件自动迁移到 `.ralph/agent/scratchpad.md`；review 前若只有 ephemeral untracked 文件则跳过 wave |
| R4 | Coordinator task 创建契约 | 一个 iteration 只创建当前 U 的 runtime task，禁止预创建后续 U |
| R5 | Hard gate 后 hat 路由稳定性 | policy / contract / workflow guard rejection 产生的 `task.resume` 必须路由回源 hat；synthesizer resume 同时注入 R1 wave 上下文 |

### 2.2 本次不覆盖

- **R2**：已由 `2026-06-14-001-fix-cli-emit-policy-gate-and-loop-termination-plan.md` 覆盖。
- `ralph emit` / `ralph wave emit` 独立 `dry-run` 子命令。
- dimension-reviewer worker 启动时写 `worker.started` 遥测事件。
- non-isolated / coordinator 模式的深度优化（保持 regression 覆盖即可）。
- 自动修复已存在的旧 worktree 中的 failed tasks。

## 3. 关键代码位置

| 文件 | 作用 |
|---|---|
| `crates/ralph-core/src/event_loop/mod.rs` | event loop 主流程、`build_prompt`、policy validation、wave partition、task resume 注入 |
| `crates/ralph-core/src/event_loop/rejection.rs` | `Rejection`、`build_task_resume_payload`、`resolve_target_hat` |
| `crates/ralph-core/src/event_loop/loop_state.rs` | `LoopState`、`pending_recovery_hat`、wave tracker 状态 |
| `crates/ralph-core/src/wave_context.rs`（新增） | wave 上下文解析、`resolve_wave_context_for_synthesizer` |
| `crates/ralph-core/src/wave_tracker.rs` | `WaveTracker`、worker 调度状态（仅 R1 aggregate timeout 场景参考） |
| `crates/ralph-core/src/task_store.rs` | `TaskStore::ensure`、task key 去重 |
| `crates/ralph-core/src/hatless_ralph.rs` | hat 选择、`next_hat`、dispatch |
| `crates/ralph-core/src/instructions.rs` | prompt 构建、上下文注入 |
| `presets/en/ce-executor-isolated.yml` | preset 配置、coordinator instructions |
| `crates/ralph-cli/src/commands/emit.rs` | `ralph emit` 命令 |
| `crates/ralph-cli/src/wave.rs` | `ralph wave emit` 命令 |

## 4. 现状分析

### 4.1 R1：synthesizer 缺少 wave 上下文

- `review-synthesizer` 是 aggregate hat，触发条件是 `review.dimension.done`。
- 当前 `build_prompt`（`crates/ralph-core/src/event_loop/mod.rs:2833`）不主动向 synthesizer 注入当前 wave 的元数据。
- agent 需要自行扫描 `events.jsonl` 计算 `received_count` 和 `missing_dimensions`，容易数错。
- 当 7 维度全部返回时，agent 可能因无法确认完整性而不发射 `review.passed`。

### 4.2 R3：ephemeral 文件污染源码树

- `ce-executor-isolated` 的 review-coordinator 在 `commit_count=0 / changed_lines=0` 时若发现 untracked 文件仍会 emit `review.wave.ready`。
- agent 常把 `scratchpad.md` 写到 `crates/ralph-core/` 等源码目录，成为 untracked 文件，触发无意义 review wave。
- 当前 runtime 没有自动识别并迁移 ephemeral 文件的机制。

### 4.3 R4：coordinator 预创建未来 task

- `presets/en/ce-executor-isolated.yml` 第 307-314 行 coordinator instructions 已写："Only create current step's tasks (do not pre-create future steps)"。
- 但这只限制**跨 step**预创建，没有限制**同一个 step 内多个 U**的预创建。
- `task_store.ensure`（`crates/ralph-core/src/task_store.rs:279`）按 `(loop_id, key)` 去重，key 形如 `ce-executor:{plan_name}:step-01:{slug}`，不区分 U。
- 当 plan.md 一个 Step 内列出 U1、U2、U3 时，coordinator 可能把三个 task 全建了，executor 只做完 U1，U2/U3 因超时/未引用被标 failed。

### 4.4 R5：hard gate 后 hat 路由不稳

- `apply_event_policy_validation`（`crates/ralph-core/src/event_loop/mod.rs:587`）中所有 policy rejection 产生的 `task.resume` 都使用：
  ```rust
  let recovery_event = Event::new("task.resume", &recovery_payload);
  bus.publish(recovery_event);
  ```
  **没有指定 target hat**（对比 5283 行 isolated scope rejection 已用 `.with_target(isolated_hat.clone())`）。
- `build_task_resume_payload`（`crates/ralph-core/src/event_loop/rejection.rs:316`）已支持 `original_trigger_topic` / `original_trigger_payload`，但 policy rejection 调用时传入 `None, None`。
- wave 事件（`review.dimension.done` 等）被拒绝时，resume payload 中没有 `wave_id` / `wave_index` / `wave_total` 等上下文。
- `review-synthesizer` 被 resume 时（如 aggregate timeout 2402 行、fallback event 2455 行）也未注入 R1 wave 上下文。

## 5. 详细实施方案

### 5.1 R1：Runtime 向 review-synthesizer 注入 wave 上下文

#### 5.1.1 目标

当 `review-synthesizer` 被激活时，runner 必须构造并注入当前 wave 的元数据：

- `wave_id`
- `wave_total`（来自 `review.wave.ready` 事件）
- `received_count`（已收到的 `review.dimension.done` 数量，同 `wave_id`）
- `expected_dimensions`（来自 wave payload 的 dimension 列表）
- `missing_dimensions`（期望维度列表减去已收到维度列表）
- `ALL_DIMENSIONS_RECEIVED: true/false`
- `AGGREGATE_TIMEOUT: true/false`（当 aggregate timeout 触发时）

#### 5.1.2 实现步骤

1. **新增 wave 上下文解析模块**
   - 文件：`crates/ralph-core/src/wave_context.rs`（或在 `event_loop/mod.rs` 内新增私有模块）
   - 新增结构体 `WaveContext`：
     ```rust
     pub struct WaveContext {
         pub wave_id: String,
         pub wave_total: u32,
         pub received_count: u32,
         pub expected_dimensions: Vec<String>,
         pub missing_dimensions: Vec<String>,
         pub timed_out: bool,
     }
     ```
   - 新增函数：
     ```rust
     pub fn resolve_wave_context_for_synthesizer(
         events_file: &Path,
         tail_lines: usize, // e.g. 2000
     ) -> Option<WaveContext>
     ```
   - 逻辑：
     - 读取 events 文件末尾 `tail_lines` 行；
     - 按 `wave_id` 分组收集 `review.wave.ready` 和 `review.dimension.done` 事件；
     - 从 `review.wave.ready` 的每个 payload 中提取 `dimension`，得到 `expected_dimensions`；
     - 从 `review.dimension.done` 的每个 payload 中提取 `dimension`，得到已收到 dimensions；
     - 选择**最相关**的 wave：优先选择 `review.dimension.done` 数量最多且未完全完成的 wave；若全部完成，选择最新的 wave；
     - 计算 `received_count`、`missing_dimensions`、`ALL_DIMENSIONS_RECEIVED`。

2. **在 `EventLoop` 中构造 wave 上下文**
   - 文件：`crates/ralph-core/src/event_loop/mod.rs`
   - 新增私有方法：
     ```rust
     fn build_wave_context_for_synthesizer(&self) -> Option<serde_json::Value>
     ```
   - 逻辑：
     - 调用 `resolve_wave_context_for_synthesizer(self.events_file_path(), 2000)`；
     - 将 `WaveContext` 序列化为 JSON Value，追加 `ALL_DIMENSIONS_RECEIVED` 字段；
     - 若本次激活是由 aggregate timeout 的 `task.resume` 触发，追加 `AGGREGATE_TIMEOUT: true`。
   - 为支持 timeout 标志，在 `LoopState` 中新增 `pending_synthesizer_timeout: Option<String>` 字段，当 aggregate timeout resume 触发时设置为对应 `wave_id`。

3. **注入到 prompt 和环境变量**
   - 在 `build_prompt`（`event_loop/mod.rs:2833`）中，当 `hat_id == "review-synthesizer"` 时：
     - 调用 `build_wave_context_for_synthesizer`；
     - 若返回 `Some(ctx)`，在 prompt 顶部追加固定格式块 `## WAVE CONTEXT`。
   - 同时暴露 `EventLoop::wave_context_json_for_hat(&self, hat_id: &HatId) -> Option<String>` 方法。
   - 在 `crates/ralph-cli/src/loop_runner/runner.rs:2535` 调用 `inject_hat_execution_env` 之后，若 `display_hat == "review-synthesizer"` 且 event_loop 返回 wave context JSON，则追加 `RALPH_WAVE_CONTEXT` 到 `effective_backend.env_vars`，供 bash tool 使用。
   - prompt 块示例：
     ```markdown
     ## WAVE CONTEXT
     The following wave metadata is injected by the runner. Do not count events manually.
     ```json
     {
       "wave_id": "w-abc",
       "wave_total": 7,
       "received_count": 7,
       "expected_dimensions": ["correctness", "testing", "maintainability", "standards", "requirements", "agent-native", "learnings"],
       "missing_dimensions": [],
       "ALL_DIMENSIONS_RECEIVED": true,
       "AGGREGATE_TIMEOUT": false
     }
     ```
     ```

4. **aggregate timeout 场景**
   - 当 `inject_review_aggregate_timeouts`（`event_loop/mod.rs:2360`）触发时，在 `task.resume` payload 中写入 `"wave_id": "w-xxx"` 和 `"AGGREGATE_TIMEOUT": true`。
   - `build_wave_context_for_synthesizer` 检测到 `LoopState.pending_synthesizer_timeout` 与解析出的 `wave_id` 匹配时，设置 `AGGREGATE_TIMEOUT: true`。

#### 5.1.3 边界情况

- **多个 active waves**：选择最相关的一个注入（优先 pending，其次最新）；其余 wave 在诊断日志中记录。
- **没有 active wave**：不注入 `## WAVE CONTEXT`，synthesizer 按原逻辑执行（兼容非 wave 场景）。
- **events 文件末尾找不到 wave 事件**：若 wave 事件在 2000 行之前（极罕见），扩大扫描窗口或返回 None；synthesizer 按原逻辑执行。
- **wave 已完成但 synthesizer 被重新 resume**：解析函数仍能从 events 文件 reconstruct 完整 wave 上下文。

---

### 5.2 R3：Ephemeral 文件隔离机制

#### 5.2.1 目标

- agent 写到源码树的 `scratchpad.md` / `notes.md` / `tmp*.md` / `*.tmp.md` / `.agent-notes.md` 等运行时产物，自动迁移到 `.ralph/agent/scratchpad.md`。
- review-coordinator 触发 wave 前，若 untracked 文件**仅包含** ephemeral 模式文件，自动清理并走 `review.passed skip_reason: empty_diff`，不发 wave。
- 在下一轮 prompt 顶部注入 `"EPHEMERAL_RELOCATED": [...]`，告知 agent 原路径已迁移。

#### 5.2.2 实现步骤

1. **定义 ephemeral 文件模式**
   - 文件：`crates/ralph-core/src/config/event_loop_config.rs` 或新增 `crates/ralph-core/src/ephemeral_isolation.rs`
   - 默认模式列表（hardcoded，后续可配置化）：
     - `**/scratchpad.md`
     - `**/notes.md`
     - `**/tmp*.md`
     - `**/*.tmp.md`
     - `**/.agent-notes.md`
     - `**/wave-diff.patch.bak`
     - `**/findings-*.json.bak`
   - 允许写入的运行时区域：
     - `.ralph/agent/`
     - `.agents/scratchpad/`
     - `/tmp/`
     - `/var/tmp/`
   - 禁止落入的源码目录：
     - `crates/`
     - `src/`
     - `backend/`
     - `frontend/`
     - `examples/`
     - `docs/`（除非在 `.agents/scratchpad/` 子路径下）

2. **新增 `EphemeralIsolation` 模块**
   - 文件：`crates/ralph-core/src/ephemeral_isolation.rs`
   - 核心函数：
     ```rust
     pub fn scan_and_relocate(
         repo_root: &Path,
         loop_id: &str,
         allowlist: &[PathBuf],
     ) -> Vec<RelocationRecord>
     ```
   - 逻辑：
     - 使用 `std::process::Command` 执行 `git ls-files --others --exclude-standard`（与 `worktree.rs` 已有做法一致）；
     - 若 git 命令失败（非 git repo 或命令不可用），回退到遍历 `repo_root` 下直接子目录中的已知 ephemeral 文件名（保守策略）；
     - 对每个 untracked 文件匹配 ephemeral 模式；
     - 若命中且不在 allowlist 区域：
       - **先**将内容追加到 `.ralph/agent/scratchpad-{loop_id}.md`；
       - **再**删除源码树中的原文件；
       - 追加成功但删除失败时，记录 warning 但不抛错，避免中断 loop；
     - 返回 `RelocationRecord { from, to, size_bytes }` 列表。
   - 性能优化：`EphemeralIsolation` 内部缓存上次扫描的 events 文件大小 / mtime，无变化时跳过 git 调用。

3. **在 EventLoop 中触发扫描**
   - 文件：`crates/ralph-core/src/event_loop/mod.rs`
   - 触发点：每次 `process_output` 末尾，在处理完 agent 输出、发布事件之后、返回 `TerminationReason` 之前。
   - 推荐位置：在 `process_output` 中更新 `current_isolated_hat` 之后，新增扫描调用，并将 relocation 记录保存到 `LoopState`。
   - 仅在 `config.event_loop.ephemeral_isolation == true` 且 `execution_mode == Isolated` 时执行；其他 preset 完全不受影响。
   - 这样下一轮 `build_prompt` 时源码树已经干净，review-coordinator 读取到的 untracked 文件列表不再包含 ephemeral 文件。
   - 注意：扫描必须在 review-coordinator 计算 diff / 触发 wave 之前完成，否则 scratchpad.md 会被当成代码变更。

4. **注入 `EPHEMERAL_RELOCATED` 到 prompt**
   - 在 `build_prompt` 中，若本 iteration 有 relocated ephemeral 文件：
     - 在 prompt 顶部追加固定格式块，列出 from -> to 路径；
     - 明确告知 agent 不要重新在源码树创建这些文件。

5. **review-coordinator wave 前跳过逻辑**
   - 文件：`crates/ralph-core/src/event_loop/mod.rs`
   - runner 无法直接干预 review-coordinator 内部的 wave vs pass 决策，但可以在每次迭代开始时提前清理源码树中的 ephemeral untracked 文件，使 agent 下次读取 `git ls-files --others --exclude-standard` 时得到空列表。
   - 结合 preset 中已有的 HARD RULE（`commit_count == 0 / changed_lines == 0 / untracked 空` 时 emit `review.passed skip_reason: empty_diff`），只要 runner 在 agent 读取之前完成清理，agent 就会自然走 skip 路径。
   - 触发点：在 `process_output` 中、调用 `build_prompt` 之前执行 `scan_and_relocate`；这样下一个 hat（包括 review-coordinator）看到的源码树就是干净的。
   - 兜底：若仍有非 ephemeral untracked 文件，正常走 wave review。

6. **配置开关**
   - 在 `EventLoopConfig` 中新增：
     ```rust
     pub ephemeral_isolation: bool, // default: true for ce-executor-isolated
     ```
   - `presets/en/ce-executor-isolated.yml` 的 `event_loop` 段显式声明：
     ```yaml
     ephemeral_isolation: true
     ```

#### 5.2.3 边界情况

- **非 ephemeral 的 untracked 文件**：正常进入 wave review。
- **ephemeral 文件 + 代码变更同时存在**：只清理 ephemeral 文件，代码变更仍走 wave。
- **多 loop 共享 repo**：`.ralph/agent/scratchpad.md` 按 loop_id 分片，例如 `.ralph/agent/scratchpad-{loop_id}.md`。
- **agent 反复写入同一 ephemeral 路径**：每次追加到隔离文件，不重复创建原文件。

---

### 5.3 R4：Coordinator task 创建契约

#### 5.3.1 目标

- coordinator 必须遵守 **"当前 U 原则"**：一个 iteration 只创建与当前 `work.ready` 对应的 Implementation Unit 的 runtime task。
- 当 plan.md 一个 Step 内列出多个 U 时，只创建当前首个未完成 U 的 task，禁止预创建后续 U。
- plan-gate 推进到下一 U 时，由新的 coordinator 激活负责创建下一 U task。

#### 5.3.2 实现步骤

1. **preset 层：明确 coordinator instructions 并开启 runtime 校验**
   - 文件：`presets/en/ce-executor-isolated.yml`
   - 在 `event_loop` 段新增配置：
     ```yaml
     enforce_current_unit: true
     ```
   - 在 coordinator instructions 的 "Runtime Task Creation" 段（第 307 行起）增加 HARD RULE：
     > "**当前 U 原则（R4）**: 一个 Step 内若 plan.md 列出多个 Implementation Units（U1, U2, U3...），你必须只创建当前首个未完成 U 的 runtime task。不得预创建 U2、U3 的 task。U1 完成后，plan-gate 会推进到下一步并再次激活你，届时再创建 U2 的 task。"
   - 在 "Task Split Heuristics" 段明确：
     > "默认一个 U 一个 task。只有当 plan 明确把单 U 拆成 U1a/U1b/U1c 子单元时才创建多个 task；禁止因 '一个 Step 有多个 U' 而创建多个 task。"

2. **runtime 层：task ensure 的 step-U 契约校验**
   - 文件：`crates/ralph-core/src/task_store.rs`、`crates/ralph-core/src/config/event_loop_config.rs`
   - 在 `EventLoopConfig` 中新增字段 `#[serde(default)] pub enforce_current_unit: bool`（默认 false），旧 `ralph.yml` 不写该字段也能正常解析。
   - 在 `TaskStore` 中新增字段 `enforce_current_unit: bool`（默认 false）和方法 `pub fn set_enforce_current_unit(&mut self, enabled: bool)`。
   - 在 `TaskStore::ensure`（第 279 行）中，当 `enforce_current_unit == true` 时：
     - 若同一 `(loop_id, plan_name, step)` 下已存在**其他 U**的 open task，则拒绝创建新 task；
     - 同一 U 的多个 sub-unit（如 U1a/U1b，来自 003 plan 的 sub-task 拆分）允许并存。
   - 启用点：
     - `crates/ralph-cli/src/task_cli.rs:575` 的 `ensure_task_with_args`：加载 store 后，读取当前 active preset / ralph.yml 的 `event_loop.enforce_current_unit`（或从 `RALPH_ENFORCE_CURRENT_UNIT` 环境变量），设置为 true。
     - `crates/ralph-core/src/event_loop/mod.rs:3863` 的 `build_prompt`：加载 store 后，若当前 preset 启用该特性，同样设置。
   - 实现细节：
     - coordinator 必须按 preset 要求使用 key 格式：`ce-executor:{plan_name}:step-01:u1-impl`。
     - 从 key 最后一段 slug 提取 unit 标识：正则 `^u\d+[a-z]?`，例如 `u1-impl` -> `u1`，`u1a-impl` -> `u1`，`u10-impl` -> `u10`。
     - 若 slug 不匹配该模式，**跳过当前 U 校验**（fallback 到普通 ensure 行为），避免误伤非标准 key 或人类手动创建的 task。
     - 若同一 `(loop_id, plan_name, step)` 下已有 open task 且其 unit 与新 task 的 unit 不同，返回已有 task 或明确错误；agent 收到反馈后应只创建当前 U 的 task。
   - 为保持幂等性（R4.5），`ensure` 对同一 `(loop_id, plan_name, step, unit)` 仍然幂等：重复 ensure 返回已有 task，不重新打开 closed task。

3. **runtime 层：coordinator 创建 task 后的状态校验**
   - 文件：`crates/ralph-core/src/event_loop/mod.rs`
   - 当 coordinator 发布 `work.ready` 后，runner 在写入事件前校验：
     - 当前 step 内，不同 U 之间只能有一个 open task；
     - 若检测到多个 open task 来自同一 step 的**不同 U**，触发 policy rejection，向 coordinator 发送 `task.resume`，payload 说明 "multiple units have open tasks in current step; create only the current U's task"。
     - 同一 U 的多个 sub-unit task（U1a/U1b）不触发此拒绝，以兼容 003 plan 的 large-step 拆分。
   - 这个校验作为 preset 指令层的兜底，确保 agent 违反规则时被 runtime 硬拦截。

4. **plan-gate 推进逻辑不变**
   - plan-gate 现有逻辑：收到 `review.passed` 后 emit `queue.advance`，payload 含 `next_step`、`reviewed_task_id`、`reviewed_task_key`。
   - coordinator 收到含 `next_step` 的 `work.ready` 后，创建下一 U 的 task（复用现有流程）。

#### 5.3.3 边界情况

- **单 Step 单 U**：正常创建一个 task。
- **单 Step 多 U，agent 试图预创建 U2**：`ralph tools task ensure` 返回已有 U1 task 或错误，agent 收到明确反馈。
- **单 U 拆 sub-unit（003 plan）**：U1a、U1b 属于同一 U，允许同时 open；不会被 R4 校验拒绝。
- **U1 failed**：U1 task 进入 failed 状态，plan-gate 决定是 retry、skip 还是 blocked；不会自动创建 U2。
- **preset 显式声明 `multi_unit_step: true`**：允许单 Step 多 U，但本计划先不实现此配置，保持默认单 U。

---

### 5.4 R5：Hard gate 后 hat 路由稳定性

#### 5.4.1 目标

- policy rejection、workflow guard rejection、isolated scope rejection 产生的 `task.resume` 必须路由回**源 hat**。
- 当源 hat 是 `review-synthesizer` 时，必须同时注入 R1 wave 上下文。
- 当源 hat 是 `review-coordinator` 且 wave 事件被 policy 拒绝时，不得把 resume 路由到 `executor`。

#### 5.4.2 实现步骤

1. **统一 policy rejection 的 resume 路由**
   - 文件：`crates/ralph-core/src/event_loop/mod.rs`
   - 修改 `apply_event_policy_validation`（第 587 行）中所有 `PolicyDecision::RejectWithResume` / `Hold` 分支：
     - 从 `event.hat` 获取源 hat；
     - 调用 `resolve_target_hat(event.hat.as_deref(), event.hat.as_deref())` 得到 `HatId`；
     - 使用 `Event::new("task.resume", &recovery_payload).with_target(target_hat)` 发布；
     - 若 `event.hat` 为空或 target 解析失败，**保持现有行为**：发布无 target 的 `task.resume`，由 Ralph 兜底处理。

2. **向 resume payload 注入 wave 上下文**
   - 当 `event.wave_id` 存在时，在 `recovery_payload` 中追加：
     ```json
     {
       "wave_id": "w-abc",
       "wave_index": 3,
       "wave_total": 7,
       "original_hat": "dimension-reviewer"
     }
     ```
   - 修改 `build_task_resume_payload`（`rejection.rs:316`）签名，新增可选参数 `wave_context: Option<&WaveContext>`，并在 payload 中注入上述字段。
   - 更新所有调用点：`apply_event_policy_validation`、`isolated scope rejection`（`event_loop/mod.rs:5281`）、`execution contract rejection`（`event_loop/mod.rs:5845`）等，传入可用的 wave 上下文。

3. **workflow guard rejection 路由**
   - 文件：`crates/ralph-core/src/event_loop/mod.rs` 第 512-520 行
   - 当前 `task.resume` 无 target，改为从 `event.hat` 解析 target hat。

4. **isolated scope rejection 已正确路由，补充 wave 上下文**
   - 文件：`crates/ralph-core/src/event_loop/mod.rs` 第 5257-5287 行
   - 当前已用 `.with_target(isolated_hat.clone())`，但 `build_task_resume_payload` 传入 `None, None`。
   - 改为：若 `event.wave_id` 非空，构造 `WaveContext` 传入 `build_task_resume_payload`。
   - `original_trigger_topic/payload` 保持 `None`（isolated scope rejection 场景下源 hat 通常只需要知道 topic deny 信息即可）。

5. **review-synthesizer resume 注入 R1 上下文**
   - 文件：`crates/ralph-core/src/event_loop/mod.rs`
   - 在 `inject_review_aggregate_timeouts`（第 2360 行）中：
     - 构造 `task.resume` payload 时附加 `"AGGREGATE_TIMEOUT": true` 和 `"wave_id": "w-xxx"`；
     - 设置 `LoopState.pending_synthesizer_timeout = Some(wave_id)`。
   - 在 `inject_fallback_event`（第 2412 行）中，当 target 是 `review-synthesizer` 时：
     - 调用 `build_wave_context_for_synthesizer` 获取当前 wave 上下文；
     - 将 wave 上下文 JSON 写入 resume payload 的 `wave_context` 字段。

6. **review-coordinator wave policy rejection 不得路由到 executor**
   - 当前 policy rejection 已使用源 hat 路由，只要 `event.hat` 正确（wave 事件写入时已有 `hat` 字段），就不会漂到 executor。
   - 增加回归测试：模拟 `dimension-reviewer` emit 的 `review.dimension.done` 因 schema 缺字段被 policy reject，验证 resume 的 target 是 `dimension-reviewer`，不是 `executor`。

#### 5.4.3 边界情况

- **源 hat 未知**：若 `event.hat` 为空，resume 无 target，由 Ralph 兜底处理（现有行为）。
- **多个 rejection 同时发生**：每个 rejection 产生独立的 targeted resume。
- **target hat 未注册**：`EventBus` 会按 orphan 处理；此处增加日志警告。

## 6. 测试策略

### 6.1 测试入口

所有测试必须通过：

```bash
./scripts/run-tests.sh
# 等价于
cargo nextest run --workspace --exclude ralph-e2e
cargo test --workspace --exclude ralph-e2e --doc
```

禁止裸跑 `cargo test -p ralph-cli`。

### 6.2 R1 测试

#### 6.2.1 单元测试

- **文件**：`crates/ralph-core/src/wave_context.rs` 的 `#[cfg(test)]` 模块
- **测试 1**：`test_resolve_wave_context_basic`
  - 构造 events 文件：1 个 `review.wave.ready` wave（7 dimensions）+ 3 个 `review.dimension.done`；
  - 调用 `resolve_wave_context_for_synthesizer`；
  - 断言 `received_count=3`，`missing_dimensions` 含未返回的 4 个维度。
- **测试 2**：`test_resolve_wave_context_all_received`
  - 构造 events 文件：7 个 `review.wave.ready` + 7 个 `review.dimension.done`；
  - 断言 `received_count=7`，`ALL_DIMENSIONS_RECEIVED=true`，`missing_dimensions` 为空。
- **测试 3**：`test_resolve_wave_context_no_wave_events`
  - 构造空 events 文件；
  - 断言返回 None。

#### 6.2.2 集成测试

- **文件**：`crates/ralph-core/src/event_loop/tests.rs` 或新增 `crates/ralph-core/tests/wave_context_injection.rs`
- **测试 4**：`synthesizer_prompt_contains_wave_context`
  - 构造 EventLoop，注册 7 个 hat，发布 `review.wave.ready`（7 dimensions）和 7 个 `review.dimension.done`；
  - 激活 `review-synthesizer`；
  - 读取 `build_prompt` 输出；
  - 断言 prompt 包含 `"wave_total": 7`、`"received_count": 7`、`"ALL_DIMENSIONS_RECEIVED": true`。
- **测试 5**：`synthesizer_prompt_on_timeout`
  - 注册 wave，只返回 3 个 result；
  - 触发 aggregate timeout；
  - 激活 synthesizer；
  - 断言 prompt 包含 `AGGREGATE_TIMEOUT: true` 和 missing dimensions 列表。

#### 6.2.3 BDD / YAML 场景

- **文件**：`crates/ralph-core/tests/scenarios/`
- 新增 `wave-context-injection.yml`：
  - Given: events 文件已有 7 个 `review.dimension.done`；
  - When: runner 激活 synthesizer；
  - Then: prompt 顶部含 `## WAVE CONTEXT`。

### 6.3 R3 测试

#### 6.3.1 单元测试

- **文件**：`crates/ralph-core/src/ephemeral_isolation.rs` 的 `#[cfg(test)]` 模块
- **测试 1**：`test_detects_scratchpad_in_crates`
  - 创建临时 repo，写入 `crates/ralph-core/scratchpad.md`；
  - 调用 `scan_and_relocate`；
  - 断言原文件被删除，`.ralph/agent/scratchpad.md` 含原内容。
- **测试 2**：`test_ignores_allowed_paths`
  - 写入 `.agents/scratchpad/notes.md`；
  - 断言不被迁移。
- **测试 3**：`test_detects_multiple_patterns`
  - 写入 `src/tmp-notes.md`、`backend/api.tmp.md`；
  - 断言全部被迁移。

#### 6.3.2 集成测试

- **文件**：`crates/ralph-core/src/event_loop/tests.rs`
- **测试 4**：`ephemeral_files_relocated_before_review`
  - 构造 EventLoop，agent 输出创建 `crates/ralph-core/scratchpad.md`；
  - runner 处理输出；
  - 断言原文件不存在，`.ralph/agent/scratchpad.md` 存在；
  - 断言 review-coordinator 不因此触发 wave。
- **测试 5**：`ephemeral_relocation_injected_to_prompt`
  - 迁移发生后激活任意 hat；
  - 断言 prompt 含 `EPHEMERAL_RELOCATED`。

#### 6.3.3 BDD / YAML 场景

- 新增 `ephemeral-isolation.yml`：
  - Given: untracked `crates/ralph-core/scratchpad.md`；
  - When: loop 迭代；
  - Then: 文件被迁移，下一轮 prompt 含迁移记录。

### 6.4 R4 测试

#### 6.4.1 单元测试

- **文件**：`crates/ralph-core/src/task_store.rs` 的 `#[cfg(test)]` 模块
- **测试 1**：`test_ensure_current_unit_rejects_precreation`
  - 开启 `enforce_current_unit`；
  - ensure U1 task；
  - 尝试 ensure U2 task（同 step）；
  - 断言返回 U1 task 或报错，U2 未创建。
- **测试 2**：`test_ensure_same_unit_is_idempotent`
  - 多次 ensure 同一 `(loop_id, plan_name, step, unit)`；
  - 断言只有一个 task。

#### 6.4.2 集成测试

- **文件**：`crates/ralph-core/src/event_loop/tests.rs`
- **测试 3**：`coordinator_cannot_pcreate_future_unit_tasks`
  - 构造 EventLoop，coordinator 试图同时创建 U1、U2、U3 task；
  - runner 处理后；
  - 断言只有 U1 task 为 open，U2/U3 不存在或为 blocked；
  - 断言向 coordinator 发送 `task.resume` 说明错误。
- **测试 4**：`next_unit_task_created_after_advance`
  - U1 完成，plan-gate 发送 `queue.advance`；
  - coordinator 创建 U2 task；
  - 断言 U2 task 存在且为 open。

#### 6.4.3 CLI / 端到端测试

- **文件**：`crates/ralph-cli/src/loop_runner/tests.rs`
- 增加 smoke fixture：
  - 模拟 coordinator 预创建多 U task；
  - 验证 runner 拒绝并 resume coordinator。

### 6.5 R5 测试

#### 6.5.1 单元测试

- **文件**：`crates/ralph-core/src/event_loop/rejection.rs` 的 `#[cfg(test)]` 模块
- **测试 1**：`build_task_resume_payload_includes_wave_context`
  - 构造含 `wave_id` / `wave_index` / `wave_total` 的 `WaveContext`；
  - 调用 `build_task_resume_payload`；
  - 断言 payload 含上述字段。

#### 6.5.2 集成测试

- **文件**：`crates/ralph-core/src/event_loop/tests.rs`
- **测试 2**：`policy_rejection_resume_targets_source_hat`
  - `executor` emit `build.done`；
  - policy `topic_deny_rules` reject；
  - 验证 bus 上 `task.resume` 的 target = `executor`。
- **测试 3**：`wave_event_policy_rejection_targets_dimension_reviewer`
  - `dimension-reviewer` emit 缺字段的 `review.dimension.done`；
  - policy reject；
  - 验证 resume target = `dimension-reviewer`，payload 含 `wave_id`。
- **测试 4**：`workflow_guard_rejection_targets_source_hat`
  - 构造 workflow guard violation；
  - 验证 resume target = 源 hat。
- **测试 5**：`synthesizer_resume_includes_wave_context`
  - 触发 aggregate timeout；
  - 验证 resume payload 含 `wave_context`。

#### 6.5.3 BDD / YAML 场景

- 新增 `hard-gate-routing.yml`：
  - Given: executor 发 `build.done`；
  - When: runner policy reject；
  - Then: `task.resume` target = executor。

### 6.6 回归测试

- 运行完整 `cargo nextest run --workspace --exclude ralph-e2e`。
- 特别关注：
  - `ralph-cli` 包测试（串行）；
  - `ralph-core` 的 `smoke_runner`（需 `--features recording`）；
  - `ralph-core` 的 `scenarios` 集成测试。

### 6.7 手动冒烟测试

- 在目标 worktree 运行：
  ```bash
  ralph run -H builtin:ce-executor-isolated -p docs/plans/my-plan.md
  ```
- 验证：
  - synthesizer prompt 顶部出现 `## WAVE CONTEXT`；
  - 源码树无 `scratchpad.md`；
  - `.ralph/agent/tasks.jsonl` 无 U2-U4.5 预创建 failed task；
  - `ralph emit build.done` 被 CLI 拦截（R2 已由 001 覆盖，此处回归验证）。

## 7. 验收标准

### 7.1 功能验收

| 编号 | 验收项 | 验证方式 |
|---|---|---|
| AC1 | 7 维度 review wave 完成后，synthesizer prompt 含 `wave_total=7, received_count=7, ALL_DIMENSIONS_RECEIVED=true` | 集成测试 + 手动冒烟 |
| AC2 | aggregate timeout 触发后，synthesizer prompt 含 `AGGREGATE_TIMEOUT=true` 和 missing dimensions | 集成测试 |
| AC3 | agent 写入 `crates/ralph-core/scratchpad.md`，runtime 自动迁移到 `.ralph/agent/scratchpad.md` 并删除原文件 | 单元测试 + 手动冒烟 |
| AC4 | review-coordinator 前只有 ephemeral untracked 文件时，不发 wave，走 `review.passed skip_reason=empty_diff` | 集成测试 |
| AC5 | coordinator 预创建同 step 多 U task 时被 runtime 拦截，只保留当前 U task | 单元测试 + 集成测试 |
| AC6 | policy rejection 产生的 `task.resume` target 等于源 hat | 集成测试 |
| AC7 | wave 事件 policy rejection 的 resume payload 含 `wave_id` / `wave_index` / `wave_total` | 单元测试 + 集成测试 |
| AC8 | synthesizer resume 同时触发 R1 wave 上下文注入 | 集成测试 |

### 7.2 测试验收

- `cargo nextest run --workspace --exclude ralph-e2e` 全绿。
- 新增测试代码行覆盖 R1/R3/R4/R5 核心路径，每个 requirement 至少 3 个测试用例（单元/集成/BDD 至少各 1）。
- 现有 smoke fixtures 不破环。

### 7.3 文档验收

- `crates/ralph-core/data/ralph-tools.md` 如涉及 `ralph tools task ensure` 行为变更，同步更新。
- `presets/en/ce-executor-isolated.yml` 的 coordinator instructions 已更新 R4 规则。
- `CLAUDE.md` / `AGENTS.md` 如有相关条目变更，同步更新（本项目要求两者一致）。

## 8. 风险与回滚

### 8.1 风险

| 风险 | 可能性 | 影响 | 缓解措施 |
|---|---|---|---|
| R1 wave 上下文注入错误，导致 synthesizer 拿到错误数据 | 中 | 高 | 增加单元测试覆盖多种 wave 状态；保留 agent 手动计数作为 fallback；在 prompt 中明确 "do not count manually, use this context" |
| R3 ephemeral 模式误伤合法 untracked 文件 | 中 | 中 | 模式列表从保守开始（仅明确运行时笔记文件）；`ephemeral_isolation` 默认 false，仅在 `ce-executor-isolated` 开启；可通过 `event_loop.ephemeral_isolation: false` 关闭；review 前打印迁移日志 |
| R4 task ensure 校验过于严格，阻断正常单 Step 多 U 场景 | 低 | 高 | 校验仅在 `ce-executor-isolated` 开启；保留 `multi_unit_step` 配置扩展点；测试覆盖单 U、多 U、子任务拆分场景 |
| R5 target hat 路由错误，resume 漂到错误 hat | 低 | 高 | 每个 rejection 路径增加回归测试；未知 hat 时回退到现有无 target 行为 |
| 改动影响 coordinator 模式（non-isolated） | 中 | 中 | 所有新机制默认对 non-isolated 关闭或保持兼容；重点测试 isolated 模式 |

### 8.2 回滚

- 所有改动以独立 commit 完成，按 R1/R3/R4/R5 分批提交。
- 若某一部分导致 CI 失败或现场 regression，可单独 revert 该 commit。
- preset 配置变更若引发问题，可临时在 `ralph.yml` 中覆盖：
  ```yaml
  event_loop:
    ephemeral_isolation: false
  ```

## 9. 时间安排

| 阶段 | 任务 | 预计时间 | 产出 |
|---|---|---|---|
| 1 | R1 实现：新增 `wave_context` 解析模块 + `build_wave_context_for_synthesizer` + prompt/环境变量注入 | 4h | 代码 + 单元测试 |
| 2 | R3 实现：`EphemeralIsolation` 模块 + EventLoop 触发 + review 前跳过逻辑 | 4h | 代码 + 单元测试 |
| 3 | R4 实现：preset 指令更新 + task_store 校验 + EventLoop 后置校验 | 3h | 代码 + 单元测试 |
| 4 | R5 实现：统一 policy/workflow resume target + wave context 注入 | 3h | 代码 + 单元测试 |
| 5 | 集成测试 + BDD 场景 + 手动冒烟 | 4h | 测试文件 + fixture |
| 6 | 运行 `cargo nextest run --workspace --exclude ralph-e2e` 并修复问题 | 2h | CI 绿 |
| 7 | 文档同步与计划归档 | 1h | 更新后的 md 文件 |
| **总计** | | **~21h** | |

## 10. 待确认问题

本计划已回答需求文档中的 Q1-Q4：

- **Q1**：`RALPH_WAVE_CONTEXT` 以环境变量 + prompt 块两者同时提供。
- **Q2**：ephemeral 文件模式默认 hardcoded，配置开关仅控制是否启用；模式列表后续可配置化。
- **Q3**：coordinator 识别 "当前 U" 不解析 plan.md，而是依赖 `task_key` 中的 `uN-` 前缀 + task_store 校验兜底。
- **Q4**：本计划不引入 `multi_unit_step` 配置，保持默认单 U；后续若需支持再扩展。
- **Q5**：topic_deny 与 schema 违规的 CLI 错误输出复用 `policy_check.rs` 的 `emit_policy_validation_failure`，已由 001 计划统一。

## 11. 回归防护清单（不回归承诺）

本计划的所有改动必须遵守以下原则，确保不引入回归：

### 11.1 默认关闭新行为

- `event_loop.ephemeral_isolation` 默认 `false`，仅在 `ce-executor-isolated` preset 中显式开启。
- `event_loop.enforce_current_unit` 默认 `false`，仅在 `ce-executor-isolated` preset 中显式开启。
- 新增配置字段均加 `#[serde(default)]`，旧 `ralph.yml` 不配置也能正常解析。

### 11.2 非 isolated 模式零侵入

- R1、R3、R4、R5 的新机制仅在 `execution_mode: isolated` 下生效。
- coordinator 模式（`execution_mode: coordinator`）的现有行为保持不变。

### 11.3 失败即回退

- R1 的 wave 上下文解析失败时返回 `None`，synthesizer 按原逻辑执行，不阻塞 loop。
- R3 的 git 扫描失败或文件迁移失败时记录 warning，不 panic、不中断 loop。
- R4 的 task key 不符合 `uN-xxx` 模式时跳过校验，保持原有 ensure 行为。
- R5 的 target hat 解析失败时发布无 target 的 `task.resume`，保持现有 Ralph 兜底行为。

### 11.4 不删除可能重要的文件

- R3 的 ephemeral 模式列表从保守开始，只包含明确的运行时笔记文件（`scratchpad.md`、`notes.md`、`tmp*.md` 等）。
- 迁移策略：**先追加内容到 `.ralph/agent/scratchpad-{loop_id}.md`，再删除原文件**；追加成功但删除失败时不抛错。
- 用户可通过 `event_loop.ephemeral_isolation: false` 完全关闭此机制。

### 11.5 不改变现有 API 签名

- `TaskStore::ensure` 的对外签名不变，仅内部增加可选校验。
- `build_task_resume_payload` 增加可选参数为最后的位置参数或 `Option`，所有现有调用点通过传入 `None` 保持兼容。
- `EventLoopConfig` 新增字段均有默认值，不影响已有配置反序列化。

### 11.6 每个改动都有回归测试

- 每个 requirement 至少 3 个测试用例（单元/集成/BDD 或 CLI smoke）。
- 修改前运行基线：`cargo nextest run --workspace --exclude ralph-e2e`。
- 修改后全量运行同一命令，确保没有 test 失败或 fixture 破环。
- 对 `ralph-cli` 包使用 nextest 串行配置，不裸跑 `cargo test -p ralph-cli`。

### 11.7 分批提交，可独立回滚

- R1、R3、R4、R5 分别作为独立 commit 提交。
- 任何一部分导致 CI 失败或现场 regression，可单独 revert，不影响其他部分。

## 12. 参考文档

- `docs/brainstorms/2026-06-14-ce-executor-isolated-agent-output-governance-requirements.md`
- `docs/brainstorms/2026-06-13-wave-dispatch-policy-gate-requirements.md`
- `docs/plans/2026-06-14-001-fix-cli-emit-policy-gate-and-loop-termination-plan.md`
- `presets/en/ce-executor-isolated.yml`
- `crates/ralph-core/src/event_loop/mod.rs`
- `crates/ralph-core/src/event_loop/rejection.rs`
- `crates/ralph-core/src/wave_context.rs`（新增）
- `crates/ralph-core/src/wave_tracker.rs`
- `crates/ralph-core/src/task_store.rs`
- `crates/ralph-cli/src/policy_check.rs`
