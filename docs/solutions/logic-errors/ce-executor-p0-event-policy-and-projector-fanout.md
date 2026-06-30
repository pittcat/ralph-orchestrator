---
title: "ce-executor-serial P0 chain: event-policy double-filter + projector empty-headings + fix-unit task_id fanout"
date: 2026-06-30
category: docs/solutions/logic-errors/
module: ralph-core::state_projector_and_event_policy_and_task_store
problem_type: logic_error
component: development_workflow
symptoms:
  - "Loop exits with `consecutive_failures` after ~53 minutes; validator stalls at fix-02"
  - "Runner injects stall-recovery `task.resume` and gets `EVENT_POLICY_TOPIC_DENIED` rejection even when the originating hat emits a system-control topic"
  - "`state_projector/progress.rs` emits `WARN: progress.md written with empty headings` mid-phase and the validator cannot read phase boundaries"
  - "`tasks.jsonl` reuses a fix-unit task_id under a different task_key silently; reviewer sees two fix units sharing a row"
  - "Ledger `event_policy:event_policy:topic_denied` while the same `task.resume` was already written to events.jsonl (ledger vs events out-of-sync)"
root_cause: logic_error
resolution_type: code_fix
severity: critical
related_components:
  - ralph-core::state_projector::progress
  - ralph-core::state_projector::task
  - ralph-core::event_policy
  - ralph-core::task_store
  - ralph-core::task
tags:
  - ce-executor-serial
  - consecutive-failures
  - event-policy
  - task-resume
  - state-projector
  - progress-md
  - task-id-dedup
  - fix-unit
---

# ce-executor-serial P0 chain: event-policy double-filter + projector empty-headings + fix-unit task_id fanout

## Problem

`ce-executor-serial` primary loop `primary-20260629-170451`(53m50s)终止于 `consecutive_failures` ≥ 5。在 fix-02 `work.done` 之后,validator prompt 显示 "0 ready / 0 open / 6 closed",根本看不到 fix-02 任务上下文,无法 emit `test.passed`。progress-steward 注入的兜底 `task.resume` 又被 `event_policy` 的 `topic_denied` 二次过滤拒收;ledger 末段两条 `event_policy:event_policy:topic_denied`,而 events.jsonl 仍写入(ledger 与 events 口径不一致),反复 `loop.batch_sync.no_progress`,最终 `consecutive_failures ≥ 5` 终止。

**全链缺失** `plan.complete` / `REVIEW_COMPLETE` / `report.done` / `LOOP_COMPLETE`。commit `23dcfdaf fix(ralph-core): 修复 ce-executor-serial primary-20260629-153653 链路诊断 P0/P1` 未覆盖的四处机制缺口在本次 run 上同时暴露。本次 run 17:04:51 启动 vs commit `23dcfdaf` 17:05:xx 完成 → 本质是修完即复跑的对照基线。

## Symptoms

### P0-1 — `state_projector` 中段损坏

- validator 在 fix-02 `work.done` 后看到 "0 ready / 0 open / 6 closed",根本看不到 fix-02 任务上下文
- `WARN: progress.md written with empty headings` 在 `current_step` flip 中段持续刷出,validator 把这条 warning 当作 "progress 信号为空" 的判据
- `tasks.jsonl` 第二条记录(带 owner / 不带 owner)重复投影,fix-02 的 `started: null → closed` 跳过 `start` 状态

### P0-2 — `task.resume` system-control topic 二次过滤

- ledger 条目 `event_policy:topic_denied` 命中 `task.resume`,但同一 `task.resume` 写入了 events 文件(ledger 与 events 口径不一致)
- stall-recovery 注入的 `task.resume` 在两次重试后死锁,循环以 `consecutive_failures` 终止
- 历史复发:`noble-peacock (2026-06-17)`、`primary-153653 (2026-06-29)` 字面同型

### P0-3 — fix-unit task_id 复用 + projector dedup 缺口

- coordinator fix-01 / fix-02 在 21 秒内 emit `work.ready`,**两次复用同一 `task_id`**(prompt template 副作用)
- `tasks.jsonl` 在不同 `task_key` 下出现两行记录,身份重叠,`LoopState.fix_round_key(plan, step, task_id)` 折叠到同一个 key → fix unit 间计数器重置
- 历史复发:`primary-153653` 已记录 `p0_2_fix_units_share_task_id_close_by_key_independently`,本次是 v3 variant

## What Didn't Work

- 上溯到 `23dcfdaf` 与 `2a29e24`(remove `human.guidance` topic):这些 commit 覆盖了 `plan.complete` step 字段、`close_by_key` 优先 task_key、`project_close_task` 优先级,但**未覆盖** `task.resume` 二次过滤、projector 写空 headings、`ensure_task` dedup、fix-unit task_id 不唯一四处机制缺口
- P0-1 初版方案试图给 `close_by_key` 加 "started is None 则跳过" 守卫,被 `p0_1_close_unstarted_task_is_unchanged` 回归测试击穿 —— 这条路径会破坏合法的 fix-unit close。回滚到一个 fail-closed 路径(仅在 `task_id` / `task_key` 真的找不到时才报错),把 `CloseOutcome` 显式 enum 起来
- P0-3 初版日志用 `tracing::warn!`,但 nextest 默认通过 stdout 捕获,既有的 `test_task_ensure_deduplicates_by_key_and_updates_metadata` 严格断言 `second_id == first_id`,ANSI 着色的 WARN 行漏进 stdout,击穿测试。降级到 `debug!`,确认 `RUST_LOG=ralph_core::task_store=debug` 可重新打开供 operator 观测

## Solution

### P0-1 — `state_projector` 中段损坏

#### `crates/ralph-core/src/state_projector/progress.rs:147-200` — `write_progress` 加 `(none)` placeholder + `debug!`

`current_step` 为 `None` 时回落到 `## Current Step\n(none)\n\n` 占位文档,而不是发出 heading-only 文件。空 heading 日志从 `warn!` 降级到 `debug!`,且只在**双 heading 均为空**(真正的 bootstrap 态)时才打日志:

```rust
// 2026-06-30 P0-1 (primary-20260629-170451 diagnosis):
// The pre-fix projector rewrote `progress.md` with
// empty headings on every close event when `snap` had no
// `current_step`; that produced the
// `WARN: progress.md written with empty headings` log
// line that the validator's prompt picks up as
// "0 ready / 0 open / N closed". Falling back to a
// `(none)` placeholder is friendlier than emitting a
// heading-only document the `progress_task_gate`
// consumer interprets as a fresh empty state.
match &snap.current_step {
    Some(step) => {
        buf.push_str(CURRENT_STEP_HEADING);
        buf.push_str(step);
        buf.push_str("\n\n");
    }
    None => {
        buf.push_str(CURRENT_STEP_HEADING);
        buf.push_str("(none)\n\n");
    }
}
// ...
if snap.empty_headings {
    debug!("progress.md written with no current_step and no completed_steps");
}
```

#### `crates/ralph-core/src/state_projector/task.rs:175-258` — `project_close_task` 用 `CloseOutcome` 收紧

把 "找不到 task 时怎么 fail" 与 "close-by-key 还是 close-by-id" 拆成两个独立分支,封进 `CloseOutcome` enum,消除 ambiguous-`None` 旧路径吞掉 "started=None close" 的可能:

```rust
enum CloseOutcome {
    Closed,
    Missing,
}
let outcome = if let Some(task_key) = json_pointer(payload, "task_key") {
    if store.get_by_key_mut(task_key).is_some() {
        if store.close_by_key(task_key).is_some() {
            CloseOutcome::Closed
        } else {
            CloseOutcome::Missing
        }
    } else if store.close(&task_id).is_some() {
        CloseOutcome::Closed
    } else {
        CloseOutcome::Missing
    }
} else if store.close(&task_id).is_some() {
    CloseOutcome::Closed
} else {
    CloseOutcome::Missing
};
match outcome {
    CloseOutcome::Closed => {}
    CloseOutcome::Missing => {
        return Err(format!("task_not_found: {task_id}"));
    }
}
```

### P0-2 — `task.resume` system-control topic 短路

#### `crates/ralph-core/src/event_policy.rs:815-836` — 新增 `is_system_control_topic` helper

`loop.cancel`、`task.resume`、`build.task.abandoned` 归为 runtime 编排 topic,completion promise 故意排除,让 ralph hat-aware deny rules 继续 gate 它:

```rust
pub fn is_system_control_topic(topic: &str) -> bool {
    matches!(
        topic,
        "loop.cancel" | "task.resume" | "build.task.abandoned"
    )
}
```

#### `crates/ralph-core/src/event_policy.rs:879-911` — `check_topic_deny_rules` 入口短路

在 deny-rule 矩阵遍历之前,系统控制 topic 直接返回 `None`,让 `build_allowed_topics` 的允许集真正生效:

```rust
// 2026-06-30 P0-2 (primary-20260629-170451 diagnosis):
// System control topics (`loop.cancel`, `task.resume`,
// `build.task.abandoned`) are orchestrated by the loop
// runner — the per-hat `topic_deny_rules` must not gate
// them, even when `event.hat` falls under a hat the preset
// declared a deny rule for (e.g. validator / coordinator /
// executor are all on the deny list for `task.resume`).
if is_system_control_topic(topic) {
    return None;
}
for rule in &config.topic_deny_rules {
    // ... existing matrix walk ...
}
```

### P0-3 — fix-unit task_id 复用 + projector dedup 缺口

#### `crates/ralph-core/src/task.rs:143-185` — 新增 `Task::fix_unit_task_id` 确定性生成器

按 `(plan, fix_round, fix_unit_index, unix_ts)` 四元组,生成 `task-{plan_slug}-fix{NN}u{NN}-{ts:x}` 格式 id,避免 coordinator 复用上一轮 id:

```rust
pub fn fix_unit_task_id(
    plan_name: &str,
    fix_round: u32,
    fix_unit_index: u32,
    unix_ts: Option<u64>,
) -> String {
    let plan_slug = sanitize_plan_slug(plan_name);
    let ts = unix_ts.unwrap_or_else(|| {
        use std::time::{SystemTime, UNIX_EPOCH};
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0)
    });
    format!("task-{plan_slug}-fix{fix_round:02}u{fix_unit_index:02}-{ts:x}")
}
```

#### `crates/ralph-core/src/task_store.rs:413-433` — 新增 `find_open_task_id_in_loop` 循环作用域查找

projector 用它在 `ensure()` 之前先检测 "同 task_id 但不同 task_key" 的违例,只跳过终态行(`Closed` / `Failed`):

```rust
pub fn find_open_task_id_in_loop(
    &self,
    task_id: &str,
    loop_id: Option<&str>,
) -> Option<&Task> {
    self.tasks.iter().find(|t| {
        t.id == task_id
            && t.loop_id.as_deref() == loop_id
            && !t.status.is_terminal()
    })
}
```

#### `crates/ralph-core/src/state_projector/task.rs:89-118` — `project_ensure_task` 投影边界检查

在调用 `ensure()` 之前先查 `find_open_task_id_in_loop`,若发现 "同 task_id 但不同 key" 就在投影边界 `debug!`(而不是 `warn!`,避免污染 nextest stdout):

```rust
if let Some(provided_id) = json_pointer(payload, "task_id") {
    let candidate_id = provided_id.to_string();
    if let Some(existing) =
        store.find_open_task_id_in_loop(&candidate_id, task.loop_id.as_deref())
    {
        if existing.key.as_deref() != Some(key.as_str()) {
            tracing::debug!(
                candidate_task_id = %candidate_id,
                new_task_key = %key,
                existing_task_key = ?existing.key.as_deref(),
                existing_loop_id = ?existing.loop_id.as_deref(),
                "P0-3: work.ready reused a task_id that is already bound to a \
                 different task_key. Mint a fresh id via Task::fix_unit_task_id \
                 (plan, fix_round, fix_unit_index, unix_ts) to keep \
                 tasks.jsonl ↔ progress.md ↔ events.jsonl in lockstep."
            );
        }
    }
}
```

## Why This Works

**P0-1** 双线修复:占位 `progress.md` 让 validator 在 fix-02 后仍看到合法 `## Current Step (none)` 与完整 `completed_steps` 列表;`CloseOutcome` enum 让 `close_by_key` 与 `task_not_found` 走显式分支,合法的 fix-unit close 路径不再被 started=None 守卫击穿,而真正找不到 task 时 fail-closed。回归测试 `p0_1_close_unstarted_task_is_unchanged` 同时钉住这两条路径。

**P0-2** 系统控制 topic 在 deny-rule 矩阵入口被短路后,`build_allowed_topics` 的允许集与 `check_topic_deny_rules` 的过滤在语义上对齐:runtime 编排的 `task.resume` 永远走 `event_policy.rs:794` 的 `allowed.insert("task.resume")` 路径,ledger 不再误记 `topic_denied`,events / ledger 口径恢复一致,stall-recovery 的兜底 `task.resume` 真正起到 re-prompt 作用。四个 deny hats(validator / coordinator / executor / `ralph + loop.cancel` / `shipper + build.task.abandoned`)加 `build.done` 反例都钉在 `test_p0_2_system_control_topics_short_circuit_deny_rules`。

**P0-3** 双层防御:确定性 `fix_unit_task_id` 让 coordinator prompt 在 fix-01 / fix-02 之间不可能复用 id(即便 prompt template 仍想复用,生成器保证不同);`find_open_task_id_in_loop` + `project_ensure_task` 的 `debug!` 把 "同 id 不同 key" 的违例拉到投影边界,即便上游忘了换 id,Ralph 也会在 nextest 之外(`RUST_LOG=ralph_core::state_projector=debug` 显式启用)的高频故障面留下可观测的诊断痕迹。`p0_3_reused_task_id_with_different_key_warns_via_open_lookup` 锁定 key-scoped 性质(关 fix-02 不影响 fix-01)。

## Prevention

- **回归测试 #1**:`p0_1_close_unstarted_task_is_unchanged` —— 钉住合法 unstarted-close 路径与 `task_not_found` 错误路径共存,任何试图重新引入 "skip unstarted close" 守卫的回归会立即被击穿
- **回归测试 #2**:`test_p0_2_system_control_topics_short_circuit_deny_rules` —— 覆盖四个 deny hats + `build.done` 反例,任何把 system-control topic 重新拉回 deny 矩阵遍历的改动会让 ledger / events 口径立刻失同步
- **回归测试 #3**:`test_fix_unit_task_id_is_unique_per_triple` + `test_fix_unit_task_id_handles_unicode_plan_name` + `p0_3_find_open_task_id_in_loop_skips_terminal_rows` + `p0_3_reused_task_id_with_different_key_warns_via_open_lookup` —— 锁定 fix-unit id 的 `(plan, fix_round, fix_unit_index, unix_ts)` 四元组唯一性、unicode plan 名称兼容、`find_open_task_id_in_loop` 跳过终态行,以及 "fix-02 close 不影响 fix-01" 的 key-scoped 性质

**headless P0-3 template coord-fix**:`Task::fix_unit_task_id` helper 已落地,但 `ce-executor-serial.yml` 的 coordinator hat prompt 仍引用可能复用 task_id 的旧模板;将生成器 wiring 到 preset hat prompt 是 follow-up。本次修复在没有 preset 改动的前提下,先通过 projector 边界 + Rust 层生成器保证机制安全,prompt 层收敛留给后续单独 plan。

## Related Issues

**诊断报告**:
- `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/report/2026-06-30-ce-executor-serial-primary-20260629-170451-diagnosis.md`(v3,2026-06-30)——本次 fix 的完整根因链与修复方案
- `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/report/2026-06-30-ce-executor-serial-primary-20260629-153653-diagnosis.md`(v2,2026-06-30)——直接前置 loop;已记录 P0-2/P0-3 但未诊断出 P0-2 的 event_policy 二次过滤根因;**两报告不可合一**(153653 已定稿为诊断 v2,170451 是事件 v3)

**历史相关 solutions doc**(同领域、邻近时段):
- `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md` —— P0-2 的 task.resume 上游 schema 缺 `reason`/`target_hat` 字段、孤立 scope precheck false positive 的同类机制;本 fix 对其 "task.resume 落空" 提供最终闭环
- `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` —— 同链路邻近时段的机制级闭环修复,使用 "lint + runtime + fail-closed" 三层防御同一类隐式假设漂移
- `docs/solutions/integration-issues/mechanism-foundation-validation-2026-06-27.md` —— 机制 SC 测量基线,本次 3-P0 修复后 SC-3 修复空转 / SC-4 drift 必填的对照数据需要刷新

**自动记忆(用户跨会话 evidence)**:
- `task-resume-target-hat-dead-path.md` —— `target_hat` 单独不足以唤醒 hat,`task.resume` 必须在 hat 的 `triggers` 中才被激活;`is_system_control_topic` 短路后,这条"死路径"风险已被对冲,但仍要继续验证 hat.trigger 列表
- `ralph-emit-policy-check-still-writes.md` —— `ralph emit --policy-check` 仍写盘,只控 schema 预检不控落盘;P0-2 短路的是 deny 矩阵,与 policy-check 写盘语义正交,但两件事在 ledger 上字面相邻,后续若写 `ralph emit` 的单元测试要避免与该约束冲突

**机制基座 plan**:
- `docs/plans/2026-06-27-001-feat-ralph-orchestrator-mechanism-foundation-plan.md` —— U4 / U6 / U8 单元为本次 3-P0 修复提供基座;本 fix 是 mechanism-foundation 已落地单元的具体接线

**历史关联**:
- `noble-peacock (2026-06-17)` —— 字面同型 `task.resume` 二次过滤;`23dcfdaf` 未修,本次闭环
- `primary-20260629-153653 (2026-06-29)` —— `close_by_key` 与 fix-unit task_id 复用已部分覆盖,本次是该 v3 variant
