---
title: "fix: ce-executor 编排四件 P0 守卫(partial wave / 伪 hat / topic deny / plan_name 锁)"
type: fix
status: active
date: 2026-06-09
---

# fix: ce-executor 编排四件 P0 守卫

## Summary

`builtin:ce-executor` 跑完 31 个迭代后 `loop_completed reason="stopped"`(运行 3h50m,worktree 终止于 review-synthesizer 在 `wait_for_all` 模式下静默等死)。诊断定位 4 个互相独立的 P0 根因,本 plan 在 **pittcat-dev** 分支上落地 4 个独立可测的守卫,每个守卫自成一个 Implementation Unit、自带回归测试,互不阻塞:

- **U1 partial wave dispatch** — wave worker 失踪后 `try_build_wave` 静默丢弃批,导致 aggregator 永远不激活;补"staleness 保护 + partial dispatch"
- **U2 builtin `ralph` hat 业务 topic 越界拦截** — `ralph` 伪 hat 冒充 executor 签 `review.complete`,EventOriginGuard 漏接
- **U3 topic-deny 规则** — EventPolicy 新增 `topic_deny_rules`,preset 加 `executor → build.done` 等明确禁令
- **U4 plan_name 字段值锁死** — `work.done` schema 已要求字段存在,但不校验值等于当前 plan_name;补"值相等等价约束"

落地点:在 **pittcat-dev**(不新建分支),修改 4 个核心文件 + 1 个 preset YAML,加 4 个 BDD scenario,跑 `cargo test` 全绿。

## Problem Frame

### 真实事件链(worktree `.worktrees/2026-06-08-003-feat-preset-static-lint-plan-sunny-lion`)

| 时刻 | 事件 | 期望 | 实际 |
|---|---|---|---|
| 07:32:08 | loop 启动,prompt 指向 dev plan | ralph.yml+preset 加载 | OK |
| 08:xx:xx | 9 个 dimension-reviewer worker 派发 | 9 个 `review.dimension.done` | 仅 2 个完成,7 个失踪 |
| 08:xx:xx | review-synthesizer 触发 | wait_for_all 等 9 个 done | 等到 K=2,不再推进 |
| 11:22:09 | 用户手动 Ctrl-C | (用户主动) | loop_completed reason=stopped |

### 4 个 P0 根因(互相独立)

| P-ID | 类型 | 锚点 | 现象 |
|---|---|---|---|
| P-LOOP-1 | wave dispatch | `wave_detection.rs:96-104` `try_build_wave` 在 batch_size 不足时 `return None` | 7/8 失踪 → 整 wave 静默丢弃 |
| P-LOOP-2 | origin guard | `event_origin.rs` builtin `ralph` hat (hat_registry.rs:83 注册) 拥有发 `work.start`/`review.ready` 业务 topic 的能力 (`test_ralph_as_builtin_hat_can_publish_executor_trigger_topics` line 759) | 伪 hat 冒充签字 |
| P-PRESET-1 | policy rule | `event_policy.schemas` 只有 `required_fields` 字段(ce-executor.yml:97-159) | `build.done` 等被禁 topic 没硬规则 |
| P-PRESET-2 | policy schema | `work.done.required_fields: [..., plan_name, ...]`(ce-executor.yml:105) 只检查字段在,不计值 | `plan_name` 可乱填 |

### 与 preset 既有约定的关系

- `presets/en/ce-executor.yml:841` 已经写 "**Partial timeout**: if some dimensions haven't arrived within the aggregate timeout, work with available findings and list missing dimensions in Coverage" —— 设计意图已存在,**只是没人实现**
- `presets/en/ce-executor.yml:818` `review-synthesizer.aggregate.timeout: 300` + `dimension-reviewer.timeout: 300` —— preset 现行超时基线
- `crates/ralph-core/src/wave_tracker.rs:191-197` `timed_out_waves()` 已存在但**没有 caller** —— 基础设施已搭好,缺调用

## Requirements

- **R1** 当 wave worker 在 aggregate_timeout × 80% 阈值内仍未回报,`try_build_wave` 必须把已到达的 `review.dimension.done` 当作 **partial wave** dispatch 给 aggregator,不能整批丢弃
- **R2** builtin `ralph` hat 仍允许发 control topic(`LOOP_COMPLETE` / `loop.cancel` / `human.*`),但拒绝发任何业务 topic(`work.*` / `review.*` / `fix.*` / `plan.*` / `queue.*` / `build.done`)
- **R3** EventPolicy 新增 `topic_deny_rules` 字段,Enforce 模式下,`{hat_pattern, topic}` 命中的事件被 reject 并产生 `PolicyRejection { reason: "topic_denied", ... }`
- **R4** `ce-executor.yml` 的 `event_policy.topic_deny_rules` 至少包含 `executor → build.done` 这一对(对应 worktree 出现的 3 次违例)
- **R5** EventPolicy 新增 `plan_name_equality_required: bool` 字段,开启后 `work.done` 事件的 `payload.plan_name` 必须等于 `current_plan_name`(从 work.ready 注入)
- **R6** 4 个守卫的修改**互不耦合** — 任何一个 U 的修改不依赖其他 U 的代码
- **R7** 4 个守卫**不能引入回归** — 现有 BDD scenarios 全部继续通过,新加 scenario 验证新行为

## Key Technical Decisions

### KTD-1:partial wave 的"启动者"是谁

**选项 A**:`try_build_wave` 接受 partial(K < total),由 dispatcher 端自己注入"synthetic completion" 事件。
**选项 B**:aggregator 端(`review-synthesizer` 的 aggregate scheduler)等到 K/N,staleness 过后主动启动。

**选 A**。理由:aggregator 端只是"被动接收",它不知道 worker 是死了还是正在跑;只有 dispatcher 端的 `tokio::time::timeout(aggregate_timeout, ...)` 才知道 worker 真的不会回来了。decision 路径短、可测。

### KTD-2:`ralph` hat 的 control/business 边界

**选择**:用白名单(control topic allowlist),其他全部拒绝。理由:control topic 数量有限且稳定(`LOOP_COMPLETE` / `loop.cancel` / `human.interact` / `human.response` / `human.guidance`),白名单比"业务 topic 黑名单"更难误伤未来新增的 control topic。

### KTD-3:topic-deny 规则的匹配语义

**精确 `hat_id` 匹配 + topic 精确匹配**,不支持 glob。理由:glob 容易误伤(比如 `review.*` 拒绝掉正常 review 流程)。preset 写规则时手写全名,反而清楚。

### KTD-4:plan_name 注入点

**从 `work.ready` 事件注入**到 `EventPolicyRuntimeState.current_plan_name`,在 `queue.advance` / `plan.complete` 时清空。
理由:每个 loop 可能跑多个 plan,从 `work.ready` 注入最贴近真实事件流(也最贴近 preset `work.done` 注释"review-coordinator needs them for correlation")。

### KTD-5:测试策略

- 每个 U 加 2-3 个 **unit test**(在原文件 `#[cfg(test)] mod tests` 里)
- 每个 U 加 1 个 **BDD scenario** 放在 `crates/ralph-core/tests/scenarios/four-p0-guards/<U-name>.yml`
- 全部 BDD 跑通后跑 `cargo test --workspace --exclude ralph-e2e`
- 跑 `cargo clippy` + `cargo fmt --check`

## High-Level Technical Design

### Wave 事件流 + 4 个守卫的拦截点

```
┌─────────────┐  emit   ┌────────────────┐
│ executor    │────────▶│ events.jsonl   │
│ (8 dims)    │  K=2/8  │ (durable)      │
└─────────────┘  failed └────────┬───────┘
                                 │ read
                                 ▼
                        ┌────────────────┐
                        │ EventReader    │
                        └────────┬───────┘
                                 │ JsonlEvent stream
                                 ▼
    ┌────────────────────────────────────────────────────┐
    │ 1. U2: event_origin.filter_events_by_origin        │  ← reject "ralph"+业务 topic
    │    (EventOriginGuard)                              │
    └────────────────────┬───────────────────────────────┘
                         │ accepted
                         ▼
    ┌────────────────────────────────────────────────────┐
    │ 2. U3: event_policy.validate_event                  │  ← reject by topic_deny_rules
    │    (EventPolicy)                                    │
    └────────────────────┬───────────────────────────────┘
                         │ accepted
                         ▼
    ┌────────────────────────────────────────────────────┐
    │ 3. U4: event_policy.work_done.plan_name equality    │  ← reject if plan_name != current
    │    (plan_name lock)                                 │
    └────────────────────┬───────────────────────────────┘
                         │ accepted
                         ▼
    ┌────────────────────────────────────────────────────┐
    │ 4. U1: wave_detection.try_build_wave                │  ← accept partial wave after staleness
    │    (WaveTracker.staleness check)                    │
    └────────────────────┬───────────────────────────────┘
                         │ DetectedWave
                         ▼
                  ┌──────────────┐
                  │ dispatcher   │  → spawn workers
                  │ + aggregator │
                  └──────────────┘
```

### Staleness 状态机(U1 核心)

```
register_wave(wave_id, total=N, started_at=T0)
  │
  │  workers report 0, 1, ..., K-1 (K < N)
  │
  ├─ t < wave_timeout ──────────► wait
  │
  ├─ t ≥ wave_timeout && K < N ──► mark timed_out
  │  (tokio::time::timeout fires in dispatcher.rs:437)
  │
  └─ t ≥ aggregate_timeout × 0.8 && K < N
     │
     ▼
   try_build_wave: accept K-of-N partial
   ┌─────────────────────────────────────┐
   │ events = [review.dimension.done × K]│
   │ wave_total = N (preserved)         │
   │ partial = true (NEW field)         │
   └─────────────────────────────────────┘
     │
     ▼
   aggregator (review-synthesizer) 启动
   pass_or_fail = "partial" (NEW)
   missing_dimensions 列出 in Coverage
```

## Implementation Units

### U1. Wave partial dispatch 守卫

- **Goal**: worker 失踪 80% of aggregate_timeout 后,`try_build_wave` 不再静默丢弃批,而是按 K-of-N partial wave dispatch 给 aggregator
- **Requirements**: R1, R6
- **Dependencies**: 无
- **Files**:
  - `crates/ralph-core/src/wave_tracker.rs` — 加 `force_take_wave_results` API,允许 partial completion
  - `crates/ralph-core/src/wave_detection.rs` — `try_build_wave` 接受 partial,通过 staleness 查询判断
  - `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` — aggregate timeout 80% 时点调 `try_build_wave` with partial flag
  - `crates/ralph-core/tests/scenarios/four-p0-guards/u1-partial-wave-dispatch.yml` — BDD scenario
- **Approach**:
  1. `WaveTracker` 加 `force_take_wave_results(wave_id) -> Option<CompletedWave>`,复用现有 `take_wave_results` 逻辑但跳过 `is_complete` 检查
  2. `try_build_wave` 接收 `staleness: Duration` 参数;当 `events.len() < wave_total` 时,先查 tracker 的 `timed_out_waves(staleness)`,只在命中时才接受 partial
  3. `dispatcher.rs:432` aggregate_timeout 计算保留;新增 `partial_threshold = aggregate_timeout × 0.8`;到 partial_threshold 时调 `try_build_wave` with partial flag
  4. `CompletedWave` 加 `partial: bool` 字段(默认 false);aggregator 据此设 `pass_or_fail = "partial"` 并在 Coverage 列 missing dimensions
- **Test scenarios**:
  - **Happy path** — register 9-worker wave,8 个 worker record_result,第 9 个不报到,等 80% aggregate_timeout,force take 返回 partial wave(8 results + 1 failure synthetic,`partial: true`)
  - **Edge** — register 9-worker wave,9 个 worker 都 record_result(在 80% 阈值前),正常 take,`partial: false`
  - **Edge** — register 9-worker wave,0 个 worker 报到,等 80% 阈值,force take 返回 partial wave(0 results + 9 failures,`partial: true`)
  - **Error** — register 9-worker wave,5 个 worker 报到,5 个**panic 提前**(`record_failure`),不触发 staleness,但 `is_complete` 已为 true,正常 take
  - **Integration** (BDD): 9-worker dimension review,7 个 worker 静默 kill,loop 应在 280s 内到达 `review.complete` 而非永远等
- **Verification**:
  - `cargo test -p ralph-core wave_tracker` 全绿(包括新加的 3 个 test)
  - `cargo test -p ralph-core wave_detection` 全绿
  - BDD scenario u1-partial-wave-dispatch 通过
  - 跑 `cargo test --workspace --exclude ralph-e2e` 全绿(无回归)

### U2. builtin `ralph` hat 业务 topic 越界拦截

- **Goal**: `ralph` 伪 hat 不能再发业务 topic,只能发 control topic
- **Requirements**: R2, R6
- **Dependencies**: 无
- **Files**:
  - `crates/ralph-core/src/event_origin.rs` — `validate_event_origin` 加 control-topic 白名单分支
  - `crates/ralph-core/src/event_origin.rs` — 测试更新(`test_ralph_as_builtin_hat_can_publish_executor_trigger_topics` 改为期望 reject)
  - `crates/ralph-core/tests/scenarios/four-p0-guards/u2-ralph-pseudo-hat-rejection.yml` — BDD scenario
- **Approach**:
  1. 在 `event_origin.rs` 加常量 `RALPH_CONTROL_TOPICS: &[&str] = &["LOOP_COMPLETE", "loop.cancel", "loop.start", "human.interact", "human.response", "human.guidance"]`
  2. `validate_event_origin` 在 `hat == Some("ralph")` 分支:topic 在白名单 → `Accepted`;不在 → `Rejected { reason: "ralph_control_only" }`
  3. 更新 `test_ralph_as_builtin_hat_can_publish_executor_trigger_topics` (line 759) → 改名 `test_ralph_pseudo_hat_rejected_for_business_topics`,断言 reject
  4. 保留 `test_ralph_as_builtin_hat_passes_origin_guard` (line 696) 不变(`LOOP_COMPLETE` 仍 Accepted)
- **Test scenarios**:
  - **Happy path** — `hat: "ralph"` + `LOOP_COMPLETE` → Accepted(向后兼容)
  - **Happy path** — `hat: "ralph"` + `human.guidance` → Accepted(ROBOT guidance 路径不能断)
  - **Error** — `hat: "ralph"` + `work.start` → Rejected { reason: "ralph_control_only" }
  - **Error** — `hat: "ralph"` + `review.complete` → Rejected(对应 worktree 现象)
  - **Error** — `hat: "ralph"` + `queue.advance` → Rejected
  - **Integration** (BDD): 模拟伪 hat 签 `review.complete`,验证后续 verdict_gate 把它拒了
- **Verification**:
  - `cargo test -p ralph-core event_origin` 全绿(包括改写后的测试)
  - 旧测试 `test_ralph_as_builtin_hat_passes_origin_guard` 仍通过(不破契约)
  - BDD u2 scenario 通过

### U3. EventPolicy topic-deny 规则

- **Goal**: EventPolicy 新增 `topic_deny_rules: Vec<TopicDenyRule>`,Enforce 模式下严格 reject
- **Requirements**: R3, R4, R6
- **Dependencies**: 无
- **Files**:
  - `crates/ralph-core/src/config/event_policy.rs` — 新增 `TopicDenyRule { hat_id: String, topic: String }` 和 `EventPolicyConfig.topic_deny_rules`
  - `crates/ralph-core/src/event_policy.rs` — `validate_event` 调 `check_topic_deny_rules` 新函数
  - `presets/en/ce-executor.yml` — `event_policy:` 块加 `topic_deny_rules: [{hat_id: executor, topic: build.done}]`
  - `crates/ralph-core/tests/scenarios/four-p0-guards/u3-topic-deny-rule.yml` — BDD scenario
- **Approach**:
  1. 在 `config/event_policy.rs` 加 `TopicDenyRule` 结构,`EventPolicyConfig` 加 `pub topic_deny_rules: Vec<TopicDenyRule>`,default 空
  2. `validate_event` 在 payload schema 校验**之前**调 `check_topic_deny_rules(event.hat, event.topic)`:命中 → return `PolicyDecision::Reject(vec![PolicyFinding { reason: "topic_denied", ... }])`
  3. preset 加 `topic_deny_rules: [{hat_id: "executor", topic: "build.done"}]`,覆盖 worktree 出现的 3 次违例
  4. `payload_deny_list` 与现有 `deny-list obligation` (hat.rs:1307) 不冲突 — 前者是"hat 不能发什么 topic",后者是"hat 不能不发什么 topic",两个方向独立
- **Test scenarios**:
  - **Happy path** — `hat: "executor"` + `work.done` + 在 deny rules → Accepted(不是 deny topic)
  - **Error** — `hat: "executor"` + `build.done` + 在 deny rules → Rejected { reason: "topic_denied" }
  - **Edge** — `hat: "reviewer"` + `build.done` → Accepted(规则只匹配 executor)
  - **Edge** — Observe 模式下命中 deny rule → 只 Warn 不 Reject(模式不变性)
  - **Integration** (BDD): executor 误发 `build.done`,验证 `recovery.jsonl` 出现 `topic_denied` rejection
- **Verification**:
  - `cargo test -p ralph-core event_policy` 全绿
  - BDD u3 scenario 通过
  - 现有 hat.rs obligation deny-list 测试(deny_list_pre_condition_runs_even_when_must_emit_satisfied, line 1307)仍通过(没动 obligation 逻辑)

### U4. plan_name 字段值锁死

- **Goal**: `work.done` 的 `plan_name` 字段值必须等于当前 plan_name,只检查"字段在"不够
- **Requirements**: R5, R6
- **Dependencies**: 无
- **Files**:
  - `crates/ralph-core/src/config/event_policy.rs` — `EventPolicyConfig` 加 `plan_name_equality_required: bool`,default false
  - `crates/ralph-core/src/event_policy.rs` — `EventPolicyRuntimeState` 加 `current_plan_name: Option<String>`;`from_events` 从 work.ready 注入;`validate_event` 在 work.done 时检查相等
  - `presets/en/ce-executor.yml` — `event_policy.plan_name_equality_required: true`
  - `crates/ralph-core/tests/scenarios/four-p0-guards/u4-plan-name-equality.yml` — BDD scenario
- **Approach**:
  1. `EventPolicyRuntimeState` 加 `current_plan_name: Option<String>`,`from_events` 扫 `work.ready` 事件并 `plan_name = Some(payload.plan_name)`
  2. `EventPolicyConfig` 加 `plan_name_equality_required: bool` (default false,**不破契约**)
  3. `validate_event`:当 `topic == "work.done"` 且 `plan_name_equality_required == true` 且 `runtime.current_plan_name.is_some()` 且 `event.payload.plan_name != current_plan_name` → Reject { reason: "plan_name_mismatch" }
  4. preset 开启 `plan_name_equality_required: true`,ce-executor 显式激活
  5. preset line 105 注释更新:`# commit_count + changed_lines are required so the review-coordinator gate can distinguish "empty diff" from a non-trivial diff short-circuited to review.passed. plan_name MUST equal the value from work.ready (see event_policy.plan_name_equality_required).`
- **Test scenarios**:
  - **Happy path** — work.ready `plan_name=A` → work.done `plan_name=A` → Accepted
  - **Error** — work.ready `plan_name=A` → work.done `plan_name=B` → Rejected { reason: "plan_name_mismatch" }
  - **Edge** — `plan_name_equality_required=false` → work.done `plan_name=B` 仍 Accepted(向后兼容)
  - **Edge** — 没有 work.ready 触发,work.done 直接发 → Accepted(`current_plan_name` 为 None,跳过)
  - **Integration** (BDD): plan_name 漂移,验证 `recovery.jsonl` 出现 `plan_name_mismatch`
- **Verification**:
  - `cargo test -p ralph-core event_policy` 全绿
  - 旧 work.done 测试(用相等 plan_name)仍通过
  - BDD u4 scenario 通过

## Scope Boundaries

### In scope(本 plan 做)

- 4 个 P0 守卫的代码实现
- 4 个新 BDD scenarios
- preset 增 1-2 个新字段(topic_deny_rules + plan_name_equality_required)
- 单测 + BDD + `cargo test` 全跑通
- preset schema reference 同步更新(如有)

### Out of scope(明确不做)

- ❌ 重构 dispatcher 整条链路
- ❌ 改 builtin `ralph` hat 的注册逻辑(保留其 control-topic 发声能力)
- ❌ 改 preset 的 hat 列表或 trigger 列表
- ❌ 改 `event_policy` 的现有 PayloadType / EventPolicyMode 枚举
- ❌ 新增 wave_tracker 公共 API 之外的方法
- ❌ 跑 live API(只跑 replay + BDD)
- ❌ 更新 `presets/schemas/ce-executor.yml` 镜像文件(架构层指示其仅为参考)
- ❌ 改 `R8` 等 existing solutions 文档
- ❌ 修其它 worktree(2026-06-08-003)的 commit 历史(本 plan 不改 commit,只修当前分支)

### Deferred to Follow-Up Work

- **P1**: 扩展 verdict_gate 到 plan-gate 的中间事件(plan-gate 现在只卡 LOOP_COMPLETE,理论上应卡 `plan.complete` / `plan.blocked` 前的所有 review 链路事件) —— 等 P0 稳定后单独开 plan
- **P1**: 给 `ralph.yml` 加 `coordinator_hats` 文档警告("ralph" 是 fallback hat name,不是 workflow hat) —— 文档层,先不开 plan
- **P1**: 实时诊断(uat U0-U8)中加 4 个 P0 各自的 envelope source —— 与本 plan 解耦,等 P0 行为稳定
- **P2**: 给所有 builtin preset 做一次 `event_policy.topic_deny_rules` + `plan_name_equality_required` 的统一化(目前只 ce-executor)

## Risks & Dependencies

### Risks

| Risk | 严重度 | 缓解 |
|---|---|---|
| U1 误杀"还在跑的 worker"(staleness 阈值给得太短) | 高 | 用 aggregate_timeout × 80% (方案 B),比单 worker timeout 短,但给 60s 宽限期 |
| U2 误伤正常 RObot guidance 路径 | 中 | control topic 白名单显式包含 `human.*`,独立 test 覆盖 |
| U3 规则过严,导致 preset 演化受阻 | 中 | `TopicDenyRule` 是精确匹配(`hat_id`+`topic`),未来加新 hat 不会被误伤;preset 改 deny 列表是显式改 YAML,reviewer 必看 |
| U4 误伤"无 work.ready 触发的孤儿 work.done"(理论上不该发生) | 低 | `current_plan_name` 为 None 时跳过,不影响向后兼容 |
| 4 个 U 都改 preset YAML,合并冲突风险 | 低 | 4 个 U 加的字段互不冲突,都加在 `event_policy:` 块下,可串行 commit |

### Dependencies

- 无运行时外部依赖
- 工具链:已用 `cargo nextest` / `cargo test --doc` / `cargo clippy` / `cargo fmt` —— 全部现有
- 不依赖 `ralph-e2e`(排除在 cargo test 之外)

## Sources & Research

### 已读源码锚点

| 文件 | 行号 | 关键发现 |
|---|---|---|
| `crates/ralph-core/src/wave_tracker.rs` | 191-197 | `timed_out_waves()` 已存在,**无 caller** |
| `crates/ralph-core/src/wave_tracker.rs` | 171-180 | `take_wave_results` 依赖 `is_complete` 才返回 Some |
| `crates/ralph-core/src/wave_detection.rs` | 96-104 | `try_build_wave` 在 batch_size 不足时静默 return None |
| `crates/ralph-core/src/wave_detection.rs` | 31-42 | `timeout_secs()` 优先级: `hat.timeout > hat.aggregate.timeout > 300s` |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | 432-433 | `aggregate_timeout = wave_timeout × ceil(N/concurrency) + 30s` |
| `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | 474-489 | 已有 "synthetic failure" 兜底,但只对 `take_wave_results` 有效,对 `try_build_wave` 静默丢弃无效 |
| `crates/ralph-core/src/event_origin.rs` | 135-198 | `validate_event_origin` 已有 fail-closed 分支 |
| `crates/ralph-core/src/event_origin.rs` | 696-740 | `test_ralph_as_builtin_hat_passes_origin_guard` 验证 `ralph` hat 可发 `LOOP_COMPLETE` |
| `crates/ralph-core/src/event_origin.rs` | 759-820 | `test_ralph_as_builtin_hat_can_publish_executor_trigger_topics` 暴露漏洞 |
| `crates/ralph-core/src/hat_registry.rs` | 83 | builtin `ralph` hat 注册点 |
| `crates/ralph-core/src/config/hat.rs` | 1307 | 已有的 obligation deny-list 测试,与本 plan 方向相反,无冲突 |
| `presets/en/ce-executor.yml` | 94-159 | `event_policy.schemas` 已有 `required_fields` 校验,无 deny 规则 |
| `presets/en/ce-executor.yml` | 105 | `work.done.required_fields: [..., plan_name, ...]` 只检查字段存在 |
| `presets/en/ce-executor.yml` | 818 | `review-synthesizer.aggregate.timeout: 300` |
| `presets/en/ce-executor.yml` | 841 | 注释 "Partial timeout: ... work with available findings and list missing dimensions in Coverage" —— 设计意图已写 |

### 关键 docs/solutions 引用

- `docs/solutions/` 中无与本 plan 直接相关条目(已 grep "wave" / "partial" / "deny")
- 现有 solution `2026-06-05-wave-emission-fixes` 与本 plan 互补(那次修的是 worker 端 emit,这次修的是 aggregator 端 partial)

## Documentation Impact

- [ ] 更新 `crates/ralph-core/data/ralph-tools.md` 中关于 wave 的部分(如有引用 `take_wave_results` 行为)
- [ ] 更新 `crates/ralph-cli/src/presets.rs` 的 ce-executor 注释(如果列了 event_policy 字段)
- [ ] 不需要更新 `presets/COLLECTION.md`(plan 行为未变)
- [ ] 不需要更新 `docs/guide/harness-extensions.md`(本 plan 在底层,不在 extension 表面)

## Operational Notes

- 4 个 U 各自独立 commit,commit message 风格 fix/presets/feat 三选一,与既有 commit 风格一致
- 跑测试顺序:`cargo build` → `cargo test --workspace --exclude ralph-e2e` → `cargo clippy --workspace` → `cargo fmt --check`
- 不 push(按 CLAUDE.md 规则,本地 commit 即可,留给用户 push)
- 不创建分支(在 pittcat-dev 直接 commit)
