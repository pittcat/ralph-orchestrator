# Handoff — 2026-07-01 ce-executor-serial 重复 emit 修复

## 当前任务
修复 `ce-executor-serial` preset 运行中 `review.start` / `review.dimension.ready` 被重复 emit 导致的 loop 噪音与潜在 stall。

## 已完成

### 1. 机制层（运行时硬性去重）
- `crates/ralph-core/src/event_policy.rs`
  - 新增 `review_start_seen_keys` 去重集合，key 格式：`{plan_name}::{task_id}`（无 step）或 `{plan_name}::{task_id}::{step}`（有 step）。
  - `validate_event_with_hat` 中对 `review.start` 做重复检测，重复时返回 `RejectWithResume(DuplicateWorkDone)`。
  - `from_events` 回放 `review.start` 事件以填充去重集合，支持 loop 重启/新 `ralph` 进程继承状态。
  - `fix.applied` 回放时 prune 对应 `(plan_name, task_id)` 的 `review.start` 记录，允许 fix 后的合法 re-review。
- `crates/ralph-core/src/event_loop/mod.rs`
  - `with_context_and_diagnostics` 启动时从 `events_path` 调用 `PolicyRuntimeState::from_events` 初始化 `state.policy_runtime_state`，使去重状态跨进程持久化。
  - `fix.applied` accept 处调用 `policy_state.prune_review_start_bucket`。

### 2. Preset 层（hat-specific 单 emit 约定）
- `presets/en/ce-executor-serial.yml`
  - `coordinator` instructions 增加 `### Single-emit guard for review.start`：emit 前检查事件文件是否已有同 `(plan_name, task_id)` 的 `review.start`，有则停止 emit。
  - `review-coordinator` instructions 增加 `### Single-emit guard for review.dimension.ready`：emit 前检查是否已有同 `(plan_name, task_id, step, dimension)` 的 `review.dimension.ready`，有则停止 emit。

### 3. Data skill 层（通用原则）
- `crates/ralph-core/data/ralph-tools-emit.md`
  - 新增 `**状态驱动的 emit 规则（通用）**` 小节：强调 emit 前检查事件文件、一个 turn 只发一个业务事件、不盲目重发。

### 4. 测试
- `crates/ralph-core/src/event_policy.rs`
  - 新增 9 个单元测试覆盖 `review.start` 去重、不同 key 隔离、缺字段跳过、step 参与 key、from_events 回放、fix.applied prune。
- 已跑过的 targeted tests：
  - `cargo nextest run -p ralph-core -- event_policy::tests::review_start` → 9 passed
  - `cargo nextest run -p ralph-core --test scenarios -- ce_executor_serial_review ce_executor_serial_fix_applied_rereview` → passed
  - `cargo nextest run -p ralph-core --test scenarios -- test_u3_fix_unit_terminal_guard` → passed
  - `cargo nextest run -p ralph-core -- preset_lint` → 129 passed
  - `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` → 11 passed
  - `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded` → passed

### 5. 报告
- 更新 `docs/report/2026-07-01-ce-executor-serial-primary-20260701-112002-diagnosis.md` v2，纠正为“启动段/重复 emit 噪音”而非 review 挂死。

## 变更文件
- `crates/ralph-core/src/event_policy.rs`
- `crates/ralph-core/src/event_loop/mod.rs`
- `presets/en/ce-executor-serial.yml`
- `crates/ralph-core/data/ralph-tools-emit.md`
- `docs/report/2026-07-01-ce-executor-serial-primary-20260701-112002-diagnosis.md`
- `HANDOFF.md`（本文件）
- `task.md`（任务描述同步更新）

## 验证状态
- **targeted tests 全部通过**。
- **全量 `./scripts/run-tests.sh` 被用户中断，尚未完成**。

## 下一步
1. 继续/重新跑 `./scripts/run-tests.sh` 完成全量验证。
2. 如全量通过，本轮修复可标记完成。

## 备注
- 没有使用兜底/stall 恢复作为修复手段；改动集中在“正常路径上拒绝重复 emit” + “让 agent 知道不该重复 emit”。
- `data skill` 只写通用规则，未放入具体 hat/event 细节；hat-specific 规则放在 preset instructions。
