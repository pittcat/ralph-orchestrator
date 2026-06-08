---
date: 2026-06-08
topic: hat-lifecycle-contract
---

# Hat 生命周期契约：Terminal 强制、Stall 超时、Topic 大小写 Runtime 强制

## Summary

为 hat 状态机补齐三类**运行时**契约：(1) 每个 hat 显式声明 `terminal_event`，未触发 terminal 即"被中断"时强制收尾而非 silently skip；(2) per-hat `stall_timeout_seconds` 替代当前全局 stall 阈值，让 stall_recovery 能区分"真卡住"与"任务量大"；(3) 运行时 topic 格式统一为 lowercase.dot.case，与 Doc 1 的 lint 形成"编排期 lint + 运行期 reject"双重防护。

## Problem Frame

`ce-executor` grand-lily run 的 recovery.jsonl 记录了 12 个事件，其中 3 类问题直指 hat 生命周期契约缺失：

1. **stall_recovery 重复 3 次**（iter 5、iter 8、iter 11）：同一种 stall 反复出现说明当前 stall 阈值要么过松（每次都跑满 timeout 才报），要么是 recovery 走通后未把"已发生 stall"状态传递到下一次，loop 不知道这已是第二次 stall 而非首次。这让"per-hat 阈值"和"stall 历史累积"成为必要。

2. **iteration 2 报 `TaskNotTerminal`**：executor 创建的 runtime task 收不到 `coordinator` 的 close 信号（参见 Doc 1 提到的 coordinator_hats 缺失），state machine 知道 task 永远 open 却没强制收尾，loop 只能把它当 in-flight 跑下去，最终 8 小时后 timeout 兜底。

3. **REVIEW_COMPLETE 大写 topic 顺利通过 runtime**（events-20260608-100217.jsonl 第 7 条）：当前 `verdict_gate.topic` 大小写敏感匹配，agent 误发大写版本后 policy 接受，循环正常推进。这意味着 preset 内部的 topic 命名约定在 runtime 没有任何守护 —— agent 改一下大小写就能让 verdict_gate 失效。

根因是 `state_machine.rs` 只做"实例级"状态机（open → active → terminal），但**不**做：(a) hat 级 terminal 强制（"这个 hat 必须发 X 才能 close"）；(b) per-hat stall 阈值（"X hat 卡了多久算 stall"）；(c) runtime topic 格式 enforce（"event topic 必须符合 preset 声明的格式"）。`event_policy` 已能 reject payload schema，但 topic 字符串本身不在它的检查范围。

## Key Decisions

### 1. hat 显式声明 `terminal_event`

- 每个 hat 必须声明 `terminal_event`（preset schema 新字段）：hat 进入 active 后，只允许 emit 这一类 event 作为收尾。
- 显式 vs 推断：state machine 不再从 `publishes` 数组"猜"哪个是 terminal event，必须显式。`publishes` 是允许集合，`terminal_event` 是责任集合。
- 一对一映射：一个 hat 只能有一个 terminal_event（避免"哪个才是真正收尾"歧义）。同一 terminal_event 可被多个 hat 共享（如 `work.done` 是 executor 和 debug-resolver 共用 terminal）。
- 缺 `terminal_event`：lint 在 Doc 1 已经报，runtime 在 loop 启动时拒（编排期就拦了，不进入 loop）。

### 2. per-hat `stall_timeout_seconds` 替代全局阈值

- preset 顶层 `event_loop.stall_policy.per_hat: { [hat_id]: seconds }`，默认 600s（10 分钟）。
- 当某 hat 进入 active 但 stall_timeout 内无新 events 写回 → 报 `stall_no_events`，并把该 hat 标记 `stalled=true`。
- 当前 `stall_recovery` 收到 `stalled=true` 帽子时**累积计数**（同一 hat 累计 ≥ 2 次升级为 `repeated_stall`），后续反馈路由用这个累积态（Doc 3）。
- 全局默认值仍存在（per-hat 缺省时回落），但允许 per-hat 覆盖。

不引入复杂策略（如"根据 step 复杂度动态调整"）—— YAGNI。

### 3. runtime topic 格式 enforce

- 所有进入 `event_bus` 的 event.topic 字符串在 `event_origin` 检查后**追加**一次格式校验：必须匹配 `^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$`，否则 `reject_topic_format` 错误。
- 白名单与 Doc 1 共享 preset 顶层 `topic_format_whitelist`（如 `LOOP_COMPLETE`）。
- 与 Doc 1 lint 的区别：lint 拦 preset 写错，runtime 拦 agent 行为漂移（agent 通过 prompt 注入、自定义 tool、或者历史 prompt 改写发错 topic）。两者串联。
- agent 看到 `[POLICY REJECTED] topic=REVIEW_COMPLETE format_invalid` 后，**不**自动重试（避免 agent 自我循环发同款错 topic），recovery 走 prompt alert（参见 Doc 3）。

### 4. TaskNotTerminal 的强制收尾

- state machine 检测到 hat 已 active 但 task 仍 open 且该 hat 已被标记 `stalled` 或 timeout → 触发"强制 terminal 收尾"：自动 emit `task.terminal_forced` 事件（payload 含被强制 hat_id、reason、accumulated_stall_count）。
- `task.terminal_forced` 是新事件类型，等价于 task 系统层面的"hat 不肯 close，我们帮它 close"。
- 这不是"忽略问题"：是"把问题显式化" —— recovery 能看到是谁被强制 close，看到累积 stall 数，看到 force 原因。

### 5. 不做 hat 自动 kill

- state machine **不**主动 kill 运行中的 backend agent process。
- 只标记"应进入 terminal 但未进"，把"是否 kill 进程"的决策权留给 orchestration loop 顶层（与现有 `task.resume` 路由机制一致）。
- 这保留"agent 有机会自己 wrap up"的窗口（如果 timeout 是因为 prompt 写错了，修 prompt 后还能继续），避免"state machine 一刀切 kill 掉 agent 上下文"的副作用。

## Actors

- **state machine（rust）**：执行 hat 生命周期判定，emit `task.terminal_forced` 等事件。
- **event_policy（rust）**：runtime topic 格式 + payload schema 双重 enforce。
- **diagnostics / recovery**：消费 stall 累积、forced terminal 等事件，写入 recovery.jsonl。
- **AI agent（loop 内）**：被 state machine 监测；其 emit 的 event 走 runtime 校验。
- **Preset 作者（人类）**：声明 `terminal_event` / `stall_timeout_seconds` / `topic_format_whitelist`。
- **编排器顶层 loop runner**：根据 `stalled` / `forced` 状态决定 `task.resume` / `task.kill` / 继续。

## Requirements

### R1. hat 显式 terminal_event

- R1.1 `HatConfig` 新增必填字段 `terminal_event: String`（指向某 topic）。
- R1.2 该 topic 必须出现在 hat 自己的 `publishes` 数组中（lint 检查）。
- R1.3 该 topic 必须出现在 Doc 1 引入的 `topic_owners` 中，且 owner == 该 hat（lint 检查）。
- R1.4 多个 hat 可共享同一 `terminal_event`（如 `work.done` 被 executor 和 debug-resolver 共用）。
- R1.5 runtime 监测：hat active 后，只有 emit `terminal_event` 才进入 terminal 状态；其他所有 publish 都进入"active 中间事件"分支，不 close hat。

### R2. per-hat stall 阈值与累积

- R2.1 `EventLoopConfig` 新增 `stall_policy: StallPolicy` 块：
  ```yaml
  stall_policy:
    global_default_seconds: 600
    per_hat:
      coordinator: 300
      executor: 1800
      review-coordinator: 900
      dimension-reviewer: 1200
      review-synthesizer: 600
      fixer: 900
      debug-resolver: 1200
      plan-gate: 300
      shipper: 300
      reporter: 300
  ```
- R2.2 stall 检测在每 hat `active_since` 时间戳上跑：超阈值无新 events → 写 `stall_no_events` 事件到 recovery.jsonl，并把 hat 标 `stalled=true`。
- R2.3 `stalled=true` 的 hat 若再次 stall（同一 loop 内累计 ≥ 2 次）→ 升级事件类型为 `repeated_stall`，payload 含 `accumulated_count`。
- R2.4 累积计数随 hat 成功 emit `terminal_event` 重置（避免"历史包袱"）。

### R3. runtime topic 格式 enforce

- R3.1 在 `event_policy` 现有 payload schema 检查**之前**插入 topic 格式检查（先查格式再查 schema，更便宜）。
- R3.2 不匹配 `^[a-z][a-z0-9_]*(\.[a-z][a-z0-9_]*)*$` 且不在 `topic_format_whitelist` → reject event，emit `reject_topic_format` 到 `recovery.jsonl`。
- R3.3 reject 时**不**自动重试 agent（避免死循环）。recovery 升级为 prompt alert（参见 Doc 3 反馈层）。
- R3.4 与 Doc 1 lint 共享白名单配置：lint 检查 preset 写错白名单，runtime 检查 agent 行为漂移。

### R4. TaskNotTerminal 强制收尾

- R4.1 state machine 检测到 hat `stalled=true` 且 task 仍 open 超过 `stall_timeout_seconds * 1.5`（强制收尾比 stall 阈值多 50%）→ 触发 `task.terminal_forced` 事件。
- R4.2 `task.terminal_forced` payload schema：
  ```yaml
  task.terminal_forced:
    payload: json_object
    required_fields: [forced_hat, reason, accumulated_stall_count, forced_at_iso]
  ```
- R4.3 强制收尾后，state machine 主动把 task 标 `closed`（带 `closure_reason=forced_terminal`），让 loop 继续推进。
- R4.4 该 task 关联的 runtime `task_id` 仍记录在 `task.terminal_forced` payload 中，便于人类事后追溯。

### R5. state machine 与 event_policy 集成

- R5.1 `state_machine.rs` 扩展：hat 状态机加 `Stalled(accumulated_count)` 子状态。
- R5.2 `event_policy.rs` 扩展：检查顺序改为 `topic_format → owner_check → payload_schema`，三步都过才 accept。
- R5.3 recovery 写入路径：`stall_no_events` / `repeated_stall` / `task.terminal_forced` / `reject_topic_format` 四类事件统一写到 `recovery.jsonl` 的 `stall_recovery` 与 `execution_contract` envelope 中（按现有 envelope 约定）。
- R5.4 不破坏现有 `verdict_gate` 行为：大小写修复在 `event_policy` 入口完成，verdict_gate 收到的就是规范化后的 topic。

### R6. 现有 preset 迁移

- R6.1 `ce-executor.yml` 10 个 hat 各补 `terminal_event` 字段：
  - coordinator → `plan.gate.ready`
  - executor → `work.done`
  - review-coordinator → `review.wave.ready`
  - dimension-reviewer → `review.dimension.done`
  - review-synthesizer → `review.passed` / `review.failed`（二选一，文末决策点）
  - fixer → `fix.applied` / `fix.exhausted`
  - debug-resolver → `fix.plan.ready` / `debug.exhausted`
  - plan-gate → `plan.complete` / `plan.blocked`
  - shipper → `REVIEW_COMPLETE`（注意：当前 token 是大写，需先在 R3 的白名单里声明，或者同步改名，见 Doc 1 R6.1）
  - reporter → `report.done` / `LOOP_COMPLETE`
- R6.2 实际生产中部分 hat 拥有多个"收尾"event（如 fixer 的 `fix.applied` vs `fix.exhausted`），见 Open Question OQ1。
- R6.3 其余 7 个 preset 由维护者按同模式补齐。

## Acceptance Examples

- AE1. **stall 重复升级**
  - **Given** `executor` hat 配 `stall_timeout_seconds: 1800`。
  - **When** loop 跑 1 小时期间 executor 两次 stall（一次 30 分钟无新 events，一次 35 分钟）。
  - **Then** recovery.jsonl 出现 `stall_no_events`（首次）→ `repeated_stall`（第二次，含 `accumulated_count: 2`）。
- AE2. **大写 topic 被 runtime 拒**
  - **Given** agent 在某 iteration 误 emit `REVIEW_COMPLETE`（大写，未在白名单）。
  - **When** event 进 `event_bus`。
  - **Then** `event_policy` 拒收，emit `reject_topic_format` 到 recovery.jsonl，agent 不收到 retry 指令，recovery 路由走 prompt alert（参见 Doc 3）。
- AE3. **TaskNotTerminal 强制收尾**
  - **Given** `executor` hat `stalled=true` 且 `accumulated_count=2`，关联 task_id 仍 open。
  - **When** stall_timeout * 1.5 时间过去。
  - **Then** state machine emit `task.terminal_forced`，task 标 `closed`（closure_reason=forced_terminal），loop 继续推进。
- AE4. **terminal_event 一致性**
  - **Given** hat 显式声明 `terminal_event: work.done`，且 `topic_owners.work.done = executor`。
  - **When** executor 误 emit `review.complete`（不属于自己 terminal）。
  - **Then** 进入"active 中间事件"路径，state machine 不 close executor。executor 仍能继续 emit `work.done` 完成生命周期。
- AE5. **白名单豁免**
  - **Given** preset 显式 `topic_format_whitelist: ["LOOP_COMPLETE"]`。
  - **When** agent emit `LOOP_COMPLETE` 作为 completion_promise。
  - **Then** runtime 不拒，`verdict_gate` 继续判定，loop 正常结束。

## Success Criteria

- [ ] `crates/ralph-core/src/state_machine.rs` 加 `Stalled(accumulated_count)` 子状态，emit `task.terminal_forced`。
- [ ] `crates/ralph-core/src/event_policy.rs` 检查顺序改为 `topic_format → owner_check → payload_schema`。
- [ ] `crates/ralph-core/src/stall_tracker.rs`（新文件）维护 per-hat stall 累积。
- [ ] `crates/ralph-core/src/diagnostics/` 把新四类事件写进 recovery.jsonl。
- [ ] `presets/en/ce-executor.yml` 10 个 hat 全部补 `terminal_event`。
- [ ] grand-lily run 重放（用同一 plan + 同样 backend mock）走完后：recovery.jsonl 至少出现 1 次 `repeated_stall`（证明累积生效），且 `task.terminal_forced` 出现次数 ≤ grand-lily 中的 `TaskNotTerminal` 次数。
- [ ] `cargo test` 通过（`./scripts/run-tests.sh` 走完 nextest + doctest）。

## Scope Boundaries

### 包括（In Scope）

- hat 状态机扩展（`Stalled` 子状态 + `task.terminal_forced`）
- per-hat stall 阈值 + 累积计数
- runtime topic 格式 enforce + 白名单共享
- 8 个内置 preset 的 `terminal_event` 字段补齐

### 不包括（Out of Scope）

- **payload 字段 schema 校验**（`2026-06-02-payload-contract-validation-requirements.md` 覆盖）
- **owner_hat 字段定义**（Doc 1 覆盖）
- **AI 自动修 preset**（所有 preset 改动由人类 review）
- **per-hat 复杂调度策略**（如优先级、抢占）—— YAGNI
- **state machine 主动 kill agent process**（决策权留给 loop runner）
- **wave worker 的 sub-lifecycle**（wave 内部多 worker 的状态机留给 wave 单独 doc）

## Dependencies / Assumptions

- Doc 1 的 `topic_owners` 字段已落地（Doc 1 R1）—— 本 doc R1.3 依赖之。
- 现有 `state_machine.rs` 的 instance lifecycle 模式（`open → active → terminal`）保持不变，本次只加 `Stalled` 子状态与 `task.terminal_forced` 旁路。
- 现有 `event_policy.rs` 的 payload schema 检查不变，本次在它前面插入 topic_format 与 owner_check 两步。
- 假设人类维护者接受为 8 个 preset 各 hat 补 `terminal_event` 字段；这是 R1 的必备 schema。
- 假设 `stalled=true` 状态在 hat 成功 terminal 时被重置（R2.4），不会出现"历史 stall 污染下次 run"。

## Sources / Research

- 现场证据 1：`.ralph/diagnostics/2026-06-08T18-02-16/recovery.jsonl` iter 5/8/11 三次 `stall_no_events`（`stall_recovery` envelope）。
- 现场证据 2：`.ralph/diagnostics/2026-06-08T18-02-16/recovery.jsonl` iter 2 `TaskNotTerminal` 事件（`execution_contract` envelope）。
- 现场证据 3：`.worktrees/2026-06-05-002-feat-preset-template-versioning-plan-bold-wolf/.ralph/events-20260608-100217.jsonl` 第 7 条 `REVIEW_COMPLETE` 大写事件（被 `verdict_gate` 接受）。
- 现场证据 4：`.ralph/diagnostics/2026-06-08T18-02-16/recovery.jsonl` iter 9 `MissingPayloadField plan_path`（`execution_contract` envelope，与 R3 关联但本次不重复 fix）。
- 现有 doc：`2026-06-08-preset-static-lint-requirements.md`（本系列 Doc 1，定义 `topic_owners` 字段）。
- 现有 doc：`2026-05-31-event-origin-guard-requirements.md`（runtime owner 检查，Doc 2 的 owner_check 步骤是其延伸）。
- 现有 doc：`2026-06-02-payload-contract-validation-requirements.md`（payload schema enforce，Doc 2 的 R5.2 与之串联）。

## 实现计划指引

给后续 ce-plan 的参考信息。

### 修改文件列表

1. **`crates/ralph-core/src/config.rs`**
   - `HatConfig` 新增必填字段 `terminal_event: String`
   - `EventLoopConfig` 新增 `stall_policy: StallPolicy { global_default_seconds, per_hat }`
2. **`crates/ralph-core/src/state_machine.rs`**
   - 加 `Stalled(u32)` 子状态
   - `task.terminal_forced` 事件 emit 逻辑
3. **`crates/ralph-core/src/event_policy.rs`**
   - 检查顺序改为 `topic_format → owner_check → payload_schema`
   - 与 Doc 1 的 `topic_format_whitelist` 共享
4. **`crates/ralph-core/src/stall_tracker.rs`**（新文件）
   - `pub struct StallTracker { hat_states: HashMap<HatId, StallState> }`
   - `StallState { accumulated_count: u32, last_stall_at: Instant }`
5. **`crates/ralph-core/src/event_loop/mod.rs`**
   - 集成 `stall_tracker`：hat active 时启动 timer，event emit 时重置
6. **`crates/ralph-core/src/diagnostics/`**
   - `stall_no_events` / `repeated_stall` / `task.terminal_forced` / `reject_topic_format` 四类事件写入 recovery.jsonl
7. **`presets/en/ce-executor.yml`**
   - 10 个 hat 各补 `terminal_event`
   - 加 `stall_policy.per_hat` 块
8. **`presets/en/{autoresearch,code-assist,debug,hatless-baseline,merge-loop,pdd-to-code-assist,research,review}.yml`**
   - 逐 preset 补 `terminal_event` + `stall_policy`

### 测试策略

- **单元测试**：
  - `state_machine.rs`：hat active → stalled → forced_terminal 路径
  - `stall_tracker.rs`：累积计数在 terminal 时重置
  - `event_policy.rs`：检查顺序、新增的 topic_format 拒收
- **集成测试**：
  - 构造一个永远不 emit terminal 的 hat fixture，验证 `task.terminal_forced` 触发
  - 构造 agent 误发大写 topic，验证 runtime 拒收且不重试
- **冒烟测试**：
  - replay grand-lily events-20260608-100217.jsonl，验证 `REVIEW_COMPLETE` 被拒（现状是接受），且新跑后 recovery.jsonl 出现 `repeated_stall` 至少 1 次
  - 8 个内置 preset `terminal_event` 全部配齐后，`ralph hats validate` strict 通过

### 增量交付顺序

1. PR 1：`terminal_event` 字段 schema + state machine `Stalled` 子状态 + 单测
2. PR 2：`stall_tracker` + per-hat timeout + 累积逻辑 + 现有 preset 补 `stall_policy`
3. PR 3：runtime topic_format enforce + 白名单共享
4. PR 4：`task.terminal_forced` 强制收尾 + recovery.jsonl 写入

## Outstanding Questions

- **OQ1（Resolve Before Planning）**：review-synthesizer 的 `terminal_event` 选 `review.passed` 还是 `review.failed`？两者都可能是 terminal（依 review 结果）。候选方案：
  - 方案 A：声明多个 `terminal_events: [review.passed, review.failed]`（多 terminal 允许）
  - 方案 B：引入"hat 完成后下一个状态由 result 决定"语义，state machine 支持 multi-terminal
  - 方案 C：声明 `review.complete` 作为统一 terminal，让 review-synthesizer 在 publish `review.passed` / `review.failed` 之后**强制** emit `review.complete`（多一步但显式）
- **OQ2（Deferred to Planning）**：`topic_format_whitelist` 在 builtin preset 失效问题（参见 `2026-06-02-payload-contract-validation-requirements.md` 的 R1.1 `schema_file` 反思）—— runtime 端的白名单配置如何与编译进 binary 的 preset 同步？
