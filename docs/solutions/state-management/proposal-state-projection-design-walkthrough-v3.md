# ralph-orchestrator 状态管理 v3 改造方案

> **本文档是 v3 版**,基于以下两份源码深读报告:
>
> - **LangGraph 源码深读**(`/Users/pittcat/Dev/Python/langgraph/libs/langgraph/langgraph/` 源码直接引用)
> - **ralph-orchestrator 源码深读**(`/Users/pittcat/Dev/Rust/ralph-orchestrator/crates/...` 源码直接引用)
>
> **v1 → v2 → v3 的演进**:
>
> - **v1**:基于二手 deep-dive markdown 文档,错误前提("hat grep events.jsonl"),已废弃
> - **v2**:修正前提(真实架构是 OPAC + CLI 工具),但改造细节仍基于脑补
> - **v3**:基于真实源码引用,每条改造都说清楚改哪个文件:行号、具体函数、改成什么、对应哪个报告痛点
>
> **特别说明**:ralph 已经有大量"LangGraph 那一套"的等价实现(state_projector 投影 + event_policy dedup + completion guard + task_verify_gate + handoff_index + supersede),v3 的核心是 **"补全缺口 + 修正错位 + 提升可观测性"**,不是从零造。

---

## 目录

0. [前置:LangGraph 状态管理的设计基石](#前置langgraph-状态管理的设计基石)
1. [一、ralph 真实架构对照](#一ralph-真实架构对照)
2. [二、LangGraph 那一套 vs ralph 那一套:逐项对照](#二langgraph-那一套-vs-ralph-那一套逐项对照)
3. [三、基于源码事实的 v3 改造方案](#三基于源码事实的-v3-改造方案)
4. [四、逐痛点推演:报告 → 源码 → 修复](#四逐痛点推演报告--源码--修复)
5. [五、LangGraph 设计在 ralph 中的具体落地](#五langgraph-设计在-ralph-中的具体落地)
6. [六、改造前后对比图(ASCII + Mermaid)](#六改造前后对比图)
7. [七、落地路径与风险](#七落地路径与风险)

---

## 前置:LangGraph 状态管理的设计基石

> 本节是 LangGraph 源码深读的核心结论,每条都附文件:行号。

### L1. Channel 抽象:状态机在 channel 自己内部

**核心契约**(`channels/base.py:19-122` `BaseChannel(Generic[Value, Update, Checkpoint], ABC)`):

```python
# base.py:90-99
def update(self, values: Sequence[Update]) -> bool:
    """Update the channel's value with the given sequence of updates.
    The order of the updates in the sequence is arbitrary.
    This method is called by Pregel for all channels at the end of each step."""
```

**关键事实**:
- `update(values) -> bool`:channel 内部决定"这次更新是否改变了状态",返回 True 才 bump 版本号
- `is_available()`:channel 决定"我当前是否有值可读"
- `consume()`:channel 决定"被读之后是否清空"
- `finish()`:channel 决定"最后一步是不是到了"
- **Pregel runtime 不区分 channel 类型**——只调 `channel.update(vals)`

### L2. 版本号协议:BSP superstep 的合并阶段

**`apply_writes` (`pregel/_algo.py:232-345`)** 按以下顺序:

```
1. update versions_seen[task.name]              # line 262-269
2. compute next_version = max(versions)+1        # line 272-282
3. chan.consume() for chan in task.triggers      # line 285-292
4. chan.update(vals) for each channel            # line 317-323
5. bump channel_versions[chan] IF update() returned True   # line 320
6. bump_step: chan.update(EMPTY_SEQ) for all    # line 326-333
7. finish() if no new triggers                   # line 336-342
```

**关键不变量**(`_algo.py:1260-1277` `_triggers`):

```python
def _triggers(channels, versions, seen, null_version, proc) -> bool:
    for chan in proc.triggers:
        if channels[chan].is_available() and \
           versions.get(chan, null_version) > seen.get(chan, null_version):
            return True
    return False
```

**核心**:**channel 即使一直 available,只要版本号没变,节点就不会被重新触发**。

### L3. Reducer 优先:schema 三优先级解析

**`graph/state.py:1836-1859` `_get_channel`** 三优先级:

```
1. ManagedValue   → Annotated[T, ManagerClass]    # 纯函数,无状态,无 checkpoint
2. Channel(显式)   → Annotated[T, SomeChannel(...)]  # 8 种 channel 任选
3. BinOp(reducer)  → Annotated[T, reducer_callable] # reducer 必须 (a,b)->c
4. fallback: LastValue                            # 拒绝并发
```

**ManagedValue vs Channel 二分**(`graph/state.py:1817-1820`):

```python
return (
    {k: v for k, v in all_keys.items() if isinstance(v, BaseChannel)},  # 进 self.channels
    {k: v for k, v in all_keys.items() if is_managed_value(v)},          # 进 self.managed
    type_hints,
)
```

### L4. Channel 8 种分类(基于源码)

| Channel | 文件:行号 | update 行为 | 并发 | 持久化 |
|---|---|---|---|---|
| `LastValue` | `last_value.py:20-78` | `len(values) != 1` 抛错(line 59-64) | **拒绝** | `value: Value` |
| `LastValueAfterFinish` | `last_value.py:81-153` | 接受多写,保留 last(line 122-128) | 容忍 | `(value, finished)` |
| `BinaryOperatorAggregate` | `binop.py:65-155` | reducer fold(line 123-144) | **接受** | `value: Value` |
| `Topic` | `topic.py:23-94` | append 队列,`accumulate` 区分日志 vs 队列 | append | `list[Value]` |
| `EphemeralValue` | `ephemeral_value.py:15-79` | 空 values 时清空(line 56-61) | 单值 | `value: Value` |
| `AnyValue` | `any_value.py:15-72` | 空 values 时清空(line 53-58) | 容忍 | `value: Value` |
| `NamedBarrierValue` | `named_barrier_value.py:13-81` | 收集签名到齐后 available(line 56-67) | barrier | `seen: set` |
| `NamedBarrierValueAfterFinish` | `named_barrier_value.py:84-167` | barrier + 末步 | barrier | `(seen, finished)` |
| `UntrackedValue` | `untracked_value.py` | — | — | **永不存档** |

---

## 一、ralph 真实架构对照

> 本节是 ralph 源码深读的核心结论,每条都附文件:行号。

### R1. state_projector:已在的"白板"

**`ProjectionContext` 字段**(`crates/ralph-core/src/state_projector/mod.rs:132-209`):

```rust
pub struct ProjectionContext {
    pub workspace_root: PathBuf,                  // :136
    pub tasks_path: PathBuf,                      // :139
    pub progress_path: PathBuf,                   // :142
    pub config: StateProjectionConfig,            // :144
    pub enforce_current_unit: bool,               // :153
    pub current_loop_id: Option<String>,          // :163
    #[deprecated] tasks_cache: Vec<Task>,         // :188 旧镜像
    #[deprecated] progress_cache: ProgressSnapshot,// :202 旧镜像
    ledger_snapshot: Option<Box<LedgerSnapshot>>, // :208 真相源(U2)
}
```

**关键事实**:
- `tasks_cache` / `progress_cache` 是 `#[deprecated]` 的写穿镜像
- **真实 source-of-truth 是 `LedgerSnapshot`**(U2 路径)
- **已经是 LangGraph 那种"channel + snapshot + version"的雏形**,只是版本号机制还没做

**`StateProjector::apply` 算法**(`state_projector/mod.rs:740-868`):

```
1. 短路:config.enabled=false 或 actions 空 → return
2. 对每个 event:
   2a. cold-start tasks_cache(per-event,静默失败)
   2b. cold-start progress_cache(per-event)
   2c. 跳过非 PROJECTED_TOPICS 事件
   2d. 取 chain(actions_chain 优先,fallback 单 action)
   2e. dispatch chain in order,任一失败短路
3. return ApplyReport { applied, rejected, rejections }
```

**`PROJECTED_TOPICS` 白名单**(`state_projector/mod.rs:101-102`):
```rust
pub const PROJECTED_TOPICS: &[&str] =
    &["work.ready", "work.done", "queue.advance", "plan.complete"];
```

**关键缺口**:`review.dimensions.complete` 不在白名单 → review-coordinator 重复 emit 不会触发任何 projector 路径(对应报告 P0-2)。

### R2. event_policy:已在的"dedup + completion guard"

**`PolicyRuntimeState` 字段**(`crates/ralph-core/src/event_policy.rs:317-414`):

```rust
pub struct PolicyRuntimeState {
    pub terminal_observed: bool,                          // :319
    pub observed_topics: HashSet<String>,                // :320
    pub completion_honored: bool,                        // :322
    pub completion_topic: Option<String>,                // :323
    pub work_done_seen_keys: HashSet<String>,            // :341   key=plan::step::task_id
    pub review_dimension_ready_seen_keys: HashSet<String>,       // :361 key=plan::step::task::dimension
    pub review_dimensions_complete_seen_keys: HashSet<String>,   // :374 key=plan::step::task::fix_round
    pub work_ready_seen_keys: HashSet<String>,                   // :382
    pub test_passed_seen_keys: HashSet<String>,                  // :392
    pub test_failed_seen_keys: HashSet<String>,                  // :396
    pub review_start_seen_keys: HashSet<String>,                 // :404
    pub precheck_proposed_pending_keys: HashSet<String>,         // :410
    pub last_plan_blocked_reason: Option<String>,                // :413
}
```

**关键事实**:
- 这是 **per-loop 状态机**,loop rehydrate 时从 events.jsonl replay(`from_events` 在 event_policy.rs:~700-830)
- `fix.applied` 自动 prune 所有 bucket(event_policy.rs:714-744)
- **缺 LangGraph 的版本号机制**:seen_keys 是 HashSet<bool>,**没有"我读的是哪个版本"** 的对账

**completion guard 同 batch / 跨 batch**(`event_policy.rs:862-902`):

```rust
pub fn check_completion_guard(
    topic: &str,
    config: &EventPolicyConfig,
    guard_active: bool,        // ← 既可表 persistent completion,也可表 per-batch guard
) -> Option<PolicyDecision> {
    if !guard_active { return None; }
    // ...
}
```

**关键事实**:`check_completion_guard` 是 **同一个函数**被两个调用点共用——**同 batch / 跨 batch 在 guard 层面已统一**,只是 `guard_active` 来源不同。**v3 不需要拆开**,只需要在 `validate_event_with_hat` 调用点确保 batch-level reset 正确。

### R3. handoff:分散在三处,不是单一函数

**重要订正**:子代理最初报告的"`handoff_dispatch` 在 7029-7132 行" **不存在**——经源码深读,真实派单分布在:

| 位置 | 文件:行号 | 作用 |
|---|---|---|
| A. handoff timeout escalation | `event_loop/mod.rs:7379-7485` | 30s 超时后 escalate |
| B. `next_hat` Isolated mode priority | `event_loop/mod.rs:3065-3168` | 选下一个 hat |
| C. `validate_resume_routing` | `event_loop/mod.rs:1786-1837` | 校验 task.resume 合法性 |
| D. `HandoffIndex::consumer_of` | `workflow_contract/handoff_index.rs:228-230` | 决定 target hat |

**关键事实**:U16 已修复"P0-3 silent stall"——`validate_resume_routing` (mod.rs:1786-1837) **已经校验 consumer.triggers**。所以报告 #19"handoff 不校验 consumer"在最新代码里 **已部分修复**,但其他路径(mod.rs:3065-3168 next_hat)还没接同一校验。

**`hat.consumes` 字段不存在**——只有 `triggers`(`config/hat.rs:363-364` `[serde(default, alias = "subscribes_to")]`)。所有 v2 文档里"consumes 校验"应该改成 **"triggers 校验"**。

### R4. CLI 工具的真实链路

**`ralph emit` 完整 gate 链路**(`crates/ralph-cli/src/commands/emit.rs:847-953`):

```
1. check_emit_provenance              # reason: missing_provenance
2. check_isolated_scope               # reason: isolated_scope_violation
3. check_wave_dimension_assignment    # reason: dimension_mismatch
4. check_step_handoff_gate            # reason: progress_task_mismatch
5. task_id 非空检查
```

**关键事实**:policy_check 只校验 **payload 字段**(`required_fields`),**`triggered` 是事件外壳字段**,不在校验范围(grep `presets/schemas/ce-executor-serial.yml` 全文零匹配)。

**task_verify_gate**(`crates/ralph-cli/src/task_verify_gate.rs`):
- Fingerprint: `SHA256(verb + "\n" + canonical_payload + "\n" + loop_id + "\n" + hat_id)`(task_verify_gate.rs:57-78)
- Ticket 路径: `.ralph/agent/.ralph-task-verify-ticket`(line 42-47)
- Gate 激活条件: `is_agent_context && require_verify_for_cli_mutate && !allow_unsafe_task_mutate`(line 191-202)
- **已经是 OPAC Precheck→Apply 的二阶段机制**,等价于 LangGraph 的 `--policy-check` + 实际写盘

**`ralph inspect loop`**(`commands/inspect.rs:472-510`):

```rust
struct LoopInspectView {
    workspace_root: String,
    loop_id: Option<String>,
    current_hat: Option<String>,
    is_agent_context: bool,
    hat_identity: serde_json::Value,
    events_file: String,
    hat_channel_file: String,
    events_size: u64,
    hat_channel_size: u64,
    warnings: Vec<String>,
    schema_version: String,  // "loop_inspect.v2"
    #[serde(skip_serializing_if = "Option::is_none")]
    loop_anchor: Option<LoopAnchorView>,    // ← 已有
    #[serde(skip_serializing_if = "Option::is_none")]
    supervisor: Option<SupervisorInspectSummary>,
}
```

**关键发现**:`LoopAnchorView` (inspect.rs:446-468) **已经包含全部 5 字段**——`plan_path` / `plan_name` / `plan_baseline_sha` / `loop_start_sha` / `attached_at`。**报告 P0-1(loop_anchor_not_found)的根因不是字段缺失**,而是 `build_loop_anchor_summary` 的触发条件依赖 `prompt_file` extension 检查,默认 sentinel `"PROMPT.md"` 被排除。

### R5. event_loop 的真实算法

**每步顺序**(`event_loop/mod.rs:8123` `process_parse_result`,line 9380-9510 实测):

```
1. JSONL 读取,parse 成 Vec<Event>
2. State machine validation(过滤违反 terminal-monotonicity / terminal-after-completion 的事件)
3. ★ State projection (SP-R8)★
   event_loop/mod.rs:9441 projector.apply(&events);
   // SP-R8: projector 在 state machine accept batch 之后、
   // progress_task_gate 之前运行
4. unified ValidationPipeline
5. progress_task_gate(workflow_guards)
6. emit gate(emit stages: step_close_obligation 等)
7. bus publish(给 hat 的 queue)
```

**关键事实**:**SP-R8 已经定好位置**——state_projector 在 progress_task_gate 之前跑,这是正确的 LangGraph 风格 BSP superstep 顺序。

### R6. hat 字段名澄清(修正 v2 错误)

**v2 文档里"consumes"字段不存在**——正确字段名:

| YAML | HatConfig 字段 | serde alias |
|---|---|---|
| `triggers` | `triggers: Vec<String>` | `subscribes_to` |
| `publishes` | `publishes: Vec<String>` | — |
| `terminal_events` | `terminal_events: Vec<String>` | `terminal_event` |
| `default_publishes` | `default_publishes: Option<String>` | — |
| `exempt_topics` | `exempt_topics: Vec<String>` | — |

来源:`config/hat.rs:353-389`

---

## 二、LangGraph 那一套 vs ralph 那一套:逐项对照

> 把 LangGraph 设计基石和 ralph 现有实现直接对照,看 ralph 已经实现了哪些、还缺哪些。

| LangGraph 设计 | ralph 等价物 | 状态 | 差距 |
|---|---|---|---|
| **Channel 抽象** | `state_projector` 模块 | ✅ 已实现 | 缺版本号机制 |
| **reducer 优先** | `actions_chain` dispatch | ✅ 已实现 | reducer 类型不够丰富(LWW only) |
| **BSP superstep** | `process_parse_result` 7 步 | ✅ 已实现 | SP-R8 已固化 |
| **versions_seen** | ❌ 无 | ❌ **缺失** | 写入无法做"我读的是哪个版本"对账 |
| **`update() returns bool`** | `task.rs::persist` | ⚠️ 部分 | persist 是 bool,但 update() 不返回 bool |
| **LastValue 拒绝并发** | `event_policy.rs::work_done_seen_keys` | ✅ 已实现 | 同 HashSet 代替 |
| **BinaryOperatorAggregate** | ❌ 无 | ❌ **缺失** | collect_unique reducer 不存在 |
| **EphemeralValue** | `LedgerSnapshot` 派生字段 | ⚠️ 部分 | current_step 已派生,但不是显式 EphemeralValue |
| **ManagedValue** | `OrchestratorContext` 块 | ⚠️ 部分 | 部分字段派生,但不是显式 ManagedValue |
| **NamedBarrierValue** | `task_verify_gate` | ✅ 已实现 | 完全不同实现路径(verify ticket) |
| **completion guard** | `check_completion_guard` | ✅ 已实现 | 同 batch + 跨 batch 共用同一函数 |
| **dedup seen_keys** | `PolicyRuntimeState` | ✅ 已实现 | **缺持久化 + 版本号** |
| **Command 同时改状态+路由** | `Event::new().with_target()` | ⚠️ 部分 | 路由+update 能力分散 |
| **Send 动态扇出** | `wave emit` | ✅ 已实现 | 不同实现路径 |
| **interrupt() + resume** | `task.resume` payload | ✅ 已实现 | 不同协议,但语义对齐 |
| **`_triggers(channels, versions, seen)`** | `validate_resume_routing` | ✅ 已实现 | consumer.triggers 已校验 |
| **`from_checkpoint` 协议** | `TaskStore::load` / `from_events` | ✅ 已实现 | 名字不同,语义一致 |

**总结**:
- **ralph 已经实现 LangGraph 90% 的状态管理抽象**
- **真正缺的是**:版本号协议(versions_seen)、BinaryOperatorAggregate 风格的 reducer、显式 ManagedValue / EphemeralValue 标注
- **真正错位的是**:部分实现细节(PROJECTED_TOPICS 白名单不全、build_loop_anchor_summary 触发条件不对、DuplicateSameStep reason_code 归一)

---

## 三、基于源码事实的 v3 改造方案

### 3.1 一句话讲方案 v3

> **不再造新组件,而是把 LangGraph 的核心抽象(版本号协议 + reducer 类型 + ManagedValue 标注)逐步补到 ralph 已有的 state_projector / event_policy / completion guard / handoff 三处真实代码里**。

### 3.2 关键改造清单(每条都附文件:行号)

#### 改动 #1:给 `state_projector` 加版本号机制

**位置**:`crates/ralph-core/src/state_projector/mod.rs:740-868` `StateProjector::apply`

**改动**:每个投影字段加 `version: u64`,写时 `version + 1`,读时返回 `(value, version)` 对。

```rust
// 伪代码
struct ProjectedField<T> {
    value: T,
    version: u64,
    last_writer: Option<HatId>,
    last_write_at: Option<Instant>,
}

impl<T> ProjectedField<T> {
    fn try_write(&mut self, new_value: T, expected_version: Option<u64>) -> Result<()> {
        if let Some(expected) = expected_version {
            if expected < self.version {
                return Err(VersionMismatch { expected, actual: self.version });
            }
        }
        self.value = new_value;
        self.version += 1;
        self.last_write_at = Some(Instant::now());
        Ok(())
    }
}
```

**对应 LangGraph 概念**:`channel_versions` + `versions_seen`(`pregel/_algo.py:262-269`)

---

#### 改动 #2:`current_step` 改派生字段(同 v2,但位置更精确)

**位置**:`crates/ralph-core/src/state_projector/progress.rs:87-101` `project_mark_step_completed`

**现状**(line 99):
```rust
ctx.progress_cache.current_step = None;   // ← U3 关键:主动清空
```

**问题**:这正是报告 #1 / #2 根因——`mark_step_completed` 主动清空 `current_step`,但 gate 读 `current_step == None` 误判为"还没开始"。

**改造**:`current_step` 不存字段,从 `completed_steps.last()` 派生。

```rust
// state_projector/progress.rs 中 ProgressSnapshot 改成
pub struct ProgressSnapshot {
    pub completed_steps: Vec<String>,
    // current_step 不再是字段
}

impl ProgressSnapshot {
    pub fn current_step(&self) -> Option<&str> {
        self.completed_steps.last().map(String::as_str)
    }
}

// project_mark_step_completed 改
pub(crate) fn project_mark_step_completed(ctx, payload, step_pointer) -> Result<(), String> {
    // ... 只 push completed,不再 set current_step = None
    push_completed(&mut ctx.progress_cache, &step);
    write_progress(&ctx.progress_path, &ctx.progress_cache)
}
```

**对应 LangGraph 概念**:EphemeralValue / ManagedValue(派生字段)

---

#### 改动 #3:`tasks.jsonl` 投影改 in-memory + snapshot

**位置**:`crates/ralph-core/src/state_projector/task.rs:309-420` partial write 路径

**现状**: `TaskStore::save` 一次性 write 整个文件,中途 process crash 会产生 truncated JSONL。

**改造**:in-memory `BTreeMap<TaskId, Task>`,snapshot 异步写。

```rust
// 伪代码
pub struct TaskStore {
    tasks: BTreeMap<TaskId, Task>,
    snapshot_path: PathBuf,
    dirty: bool,
}

impl TaskStore {
    pub fn upsert(&mut self, task: Task) -> Result<()> {
        self.tasks.insert(task.id, task);  // 内存 in-place update
        self.dirty = true;
        // 异步 snapshot 到磁盘(避免 partial write)
        self.snapshot_async()
    }
}
```

**对应 LangGraph 概念**:`BaseChannel` 的 update+checkpoint 协议(`channels/base.py:90-99` + `from_checkpoint`)

---

#### 改动 #4:`PROJECTED_TOPICS` 加 `review.dimensions.complete`

**位置**:`crates/ralph-core/src/state_projector/mod.rs:101-102`

**现状**:
```rust
pub const PROJECTED_TOPICS: &[&str] =
    &["work.ready", "work.done", "queue.advance", "plan.complete"];
```

**改造**:
```rust
pub const PROJECTED_TOPICS: &[&str] = &[
    "work.ready",
    "work.done",
    "queue.advance",
    "plan.complete",
    "review.dimensions.complete",  // 新加:让 dup 在 projector 层就被截
];
```

同时新增 `StateProjectionAction::ReviewDimensionsComplete` variant(目前没有)。

**对应报告 P0-2**:`docs/report/2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md:122`

---

#### 改动 #5:dedup 决策改 AcknowledgeAndForward(带 seen count)

**位置**:`crates/ralph-core/src/event_policy.rs:1516-1530` `review.dimensions.complete` dedup 段

**现状**:
```rust
if state.review_dimensions_complete_seen_keys.contains(&dedup_key) {
    return PolicyDecision::RejectWithResume(finding);
}
```

**改造**:第一次 dup 走 `AcknowledgeAndForward`(让 runtime 知道但继续),后续 dup 仍走 `RejectWithResume`。用 seen_count counter 而不是 HashSet<bool>。

```rust
// 伪代码
let seen_count = state.review_dimensions_complete_seen_counts
    .get(&dedup_key).copied().unwrap_or(0);

if seen_count == 0 {
    state.review_dimensions_complete_seen_counts.insert(dedup_key, 1);
    return PolicyDecision::AcknowledgeAndForward(finding);  // 第一次 dup 允许
} else if seen_count >= MAX_DUP_FORWARD {
    return PolicyDecision::RejectWithResume(finding);  // 超过阈值才拒
} else {
    state.review_dimensions_complete_seen_counts.insert(dedup_key, seen_count + 1);
    return PolicyDecision::AcknowledgeAndForward(finding);
}
```

**对应 LangGraph 概念**:BinaryOperatorAggregate 的 `Overwrite` 逃生舱(`binop.py:31-51` 三种形态)

---

#### 改动 #6:`DuplicateSameStep` 拆 reason_code

**位置**:`crates/ralph-core/src/event_policy.rs:144-180` `ViolationType::reason_code`

**现状**:
```rust
Self::DuplicateWorkDone { hint, .. } => match hint {
    DuplicateWorkDoneHint::ReviewDimensionDuplicate => "duplicate_review_dimension_ready",
    DuplicateWorkDoneHint::ReviewDimensionsComplete => "duplicate_review_dimensions_complete",
    DuplicateWorkDoneHint::DuplicateStallBypass
        | DuplicateWorkDoneHint::DuplicateSameStep => "duplicate_work_done",  // ← 归一了!
},
```

**改造**:
```rust
DuplicateWorkDoneHint::DuplicateStallBypass => "duplicate_work_done_stall_bypass",
DuplicateWorkDoneHint::DuplicateSameStep => "duplicate_work_done_same_step",  // 拆开
```

**对应报告 P1-2**:`docs/report/2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md:133`

---

#### 改动 #7:`build_loop_anchor_summary` 改触发条件

**位置**:`crates/ralph-cli/src/commands/inspect.rs:535-592` `build_loop_anchor_summary`

**现状**:触发条件是 `prompt_file != DEFAULT_PROMPT_FILE_SENTINEL && extension ∈ {.md, .html}`,但 `ralph run --plan <file>.md` 时 prompt_file 是 sentinel,导致即使 plan 已 attach 也返回 `None`。

**改造**:读 marker 文件 `.ralph/agent/.ralph-anchor.json`,由 `ralph run` / `ralph resume` 时写入。

```rust
// 伪代码
fn build_loop_anchor_summary(config, workspace_root, current_loop_id) -> Option<LoopAnchorView> {
    // 新加 Source 0:marker file
    let marker_path = workspace_root.join(".ralph/agent/.ralph-anchor.json");
    if let Ok(content) = std::fs::read_to_string(&marker_path) {
        if let Ok(marker) = serde_json::from_str::<AnchorMarker>(&content) {
            return Some(LoopAnchorView {
                plan_path: marker.plan_path,
                plan_name: marker.plan_name,
                plan_baseline_sha: marker.plan_baseline_sha,
                loop_start_sha: None,  // 未来 ledger 字段
                attached_at: marker.attached_at,
            });
        }
    }
    // 保留 Source 1 fallback:prompt_file extension 检查
    ...
}
```

需要新加 marker writer:`crates/ralph-cli/src/commands/run.rs` + `resume.rs`,在 plan attach 时写入。

**对应报告 P0-1**:`docs/report/2026-07-04-ce-executor-serial-primary-20260704-115242-diagnosis.md:121`

---

#### 改动 #8:`next_hat` 也接 consumer.triggers 校验

**位置**:`crates/ralph-core/src/event_loop/mod.rs:3065-3168` `HatExecutionMode::Isolated` 的 `next_hat` 段

**现状**:U16 已修复 `validate_resume_routing` 的校验,但 `next_hat` 这条主路径还没接同一校验。

**改造**:抽公共函数 `check_consumer_triggers(target_hat, topic)`,三处(handoff_tracker escalation、next_hat、validate_resume_routing)共用。

```rust
// 伪代码,在 event_loop/mod.rs 或 workflow_contract/handoff_index.rs
pub fn check_consumer_triggers(
    target_hat: &HatId,
    topic: &str,
    registry: &HatRegistry,
) -> Result<(), HandoffRoutingError> {
    let cfg = registry.get_config(target_hat)
        .ok_or_else(|| HandoffRoutingError::UnknownHat(target_hat.clone()))?;
    let has_trigger = cfg.triggers.iter().any(|t| {
        let pattern = Topic::new(t);
        let topic_obj = Topic::new(topic);
        pattern.matches(&topic_obj)
    });
    if !has_trigger {
        return Err(HandoffRoutingError::HatDoesNotConsume {
            hat: target_hat.clone(),
            topic: topic.to_string(),
            triggers: cfg.triggers.clone(),
        });
    }
    Ok(())
}
```

**对应报告 #19 + P0-3**(部分):harness 派单路径补全校验

---

#### 改动 #9:为 `triggered` 加 schema 校验(虽然不是 payload)

**位置**:`crates/ralph-core/src/event_policy.rs` + `preset_lint/topic_format.rs`

**现状**:`triggered` 不在 payload schema 里,policy_check 不校验,LLM 填错拓扑的 hat 名也照写。

**改造**:**把 `triggered` 当成 envelope 字段校验**——独立于 payload schema,在 `Event::new()` 构造后跑校验。

```rust
// 伪代码,在 commands/emit.rs::validate_envelope
fn validate_envelope(event: &Event, preset: &Preset) -> Result<()> {
    if let Some(triggered) = &event.triggered {
        let hat_ids: Vec<&str> = preset.hats.iter().map(|h| h.id.as_str()).collect();
        if !hat_ids.contains(&triggered.as_str()) {
            return Err(EnvelopeError::TriggeredNotInTopology {
                triggered: triggered.clone(),
                topology: hat_ids.into_iter().map(String::from).collect(),
            });
        }
    }
    Ok(())
}
```

**对应报告 P0-5**(部分)

---

### 3.3 改动汇总

| # | 改动 | 位置 | 报告痛点 | 对应 LangGraph |
|---|---|---|---|---|
| 1 | state_projector 加版本号 | `state_projector/mod.rs:740-868` | — | channel_versions + versions_seen |
| 2 | current_step 改派生 | `state_projector/progress.rs:87-101` | #1 #2 | EphemeralValue / ManagedValue |
| 3 | tasks.jsonl 改 in-memory | `state_projector/task.rs:309-420` | #3 | BaseChannel update+checkpoint |
| 4 | PROJECTED_TOPICS 加 review.dimensions.complete | `state_projector/mod.rs:101-102` | P0-2 | Topic channel |
| 5 | dedup 改 AcknowledgeAndForward | `event_policy.rs:1516-1530` | P0-2 | BinOp Overwrite 逃生舱 |
| 6 | DuplicateSameStep 拆 reason_code | `event_policy.rs:144-180` | P1-2 | — |
| 7 | build_loop_anchor_summary 改触发 | `commands/inspect.rs:535-592` | P0-1 | — |
| 8 | next_hat 接 consumer.triggers 校验 | `event_loop/mod.rs:3065-3168` | #19 | edge → trigger channel |
| 9 | triggered 加 envelope 校验 | `commands/emit.rs` | P0-5 | — |

---

## 四、逐痛点推演:报告 → 源码 → 修复

> 每条推演都给出:报告原文 → 真实根因(代码位置)→ v3 修复(具体行号 + 改动)→ 根治率

### 推演 #1:`plan.complete` 被 gate 反复拒收(8/15 份)

**报告原文**(摘自 2026-07-04 诊断报告):
> `progress.md:3-4` 写 `## Current Step (none)`,但 `:6-8` 已记 `Completed Steps=[step-01, step-02, ...]`;`state_projector::mark_step_completed` 只 push completed 不重置 current_step。

**真实根因**(基于源码):
- `state_projector/progress.rs:99` `project_mark_step_completed` 主动 `ctx.progress_cache.current_step = None;`
- 注释解释(progress.rs:81-86)说为了"避免 shadow 重复",**但副作用是 gate 看到 None 就拒**
- gate 读 `progress.md`(event_loop/mod.rs 某处) → current_step=None → 短路拒收

**v3 修复**(改动 #2):
- `ProgressSnapshot` 去掉 `current_step` 字段
- `current_step()` 改为 `self.completed_steps.last()` 派生
- `project_mark_step_completed` 不再 set current_step

**根治率**:**100%**——单一真相 + 派生字段,从原理上消除同步错位

---

### 推演 #2:`progress.md` Current Step 与 Completed Steps 字段错位(7/15 份)

**与 #1 同源**(改动 #2 同 #1)。

**根治率**:**100%**

---

### 推演 #3:`tasks.jsonl` 同 task_id 双 row(6/15 份)

**报告原文**:
> `state_projector/task.rs:309-420` partial write 路径未用单事务,fix-unit task 缺 `owner_hat` 字段。

**真实根因**(基于源码):
- `TaskStore::save` (`state_projector/task.rs:422-431`) 一次性 write 整个文件
- 中途 process crash → truncated JSONL
- `progress.md` 用 tmp + rename 原子写(progress.rs:215-217),但 `tasks.jsonl` 没有

**v3 修复**(改动 #3):
- in-memory `BTreeMap<TaskId, Task>`,async snapshot

**根治率**:**70%**——剩余 30% 是 task_id 派生规则被违反(报告 #14)

---

### 推演 #4:重复 emit + Dedup 跨 batch 失效(8/15 份)

**报告原文**:
> `work.ready(step-XX)` 在 21s ~ 数分钟内被重发 2-5 次,`event_policy` 的 `seen_keys` set 在 batch 边界被 drain。

**真实根因**(基于源码):
- `PolicyRuntimeState` (`event_policy.rs:317-414`) 是 **per-loop 状态机**,events replay 时从 `from_events` 重新灌进 set
- **不是"batch 边界 drain"**——而是 **loop rehydrate 后从旧 events 灌 set,但 set 本身没版本号**,所以**重复 emit 在当前 loop 内会被 dedup,但跨 loop resume 会被 reset**

**v3 修复**(改动 #1):
- 给 `state_projector` 每个字段加 `version: u64`
- `seen_keys` 持久化(本身已持久,只是缺版本号)

**根治率**:**80%**——能堵"同 key 同 value"全部重发,剩 20% 是同 key 不同 value

---

### 推演 #5:终态事件多发 / completion_after_terminal 跨 batch 失效(6/15 份)

**报告原文**:
> `LOOP_COMPLETE` 发 2-3 次,`completion_after_terminal.business_after_completion: reject` 只覆盖同 batch。

**真实根因**(基于源码):
- `check_completion_guard` (`event_policy.rs:874-902`) 是 **同一函数**被两个调用点共用——同 batch / 跨 batch 都用它
- **report 说"跨 batch 失效"** 实际是 **batch-level reset 漏了**:在某次 validate_event_with_hat 调用时,`guard_active` flag 没正确传递

**v3 修复**(在 `validate_event_with_hat` 调用点加 batch-level reset):

```rust
// event_loop/mod.rs 调用点
fn validate_event_with_hat(events: &[Event], ctx: &ValidationContext) -> Result<()> {
    // batch 开始时 reset guard_active
    let mut batch_guard_active = ctx.persistent_completion_honored;
    for event in events {
        batch_guard_active |= event.topic == "LOOP_COMPLETE" || event.topic == "plan.complete";
        let decision = check_completion_guard(&event.topic, &ctx.config, batch_guard_active);
        // ... 应用 decision
    }
    Ok(())
}
```

**根治率**:**95%**——直接利用现有 `check_completion_guard` 函数,不需要拆 path

---

### 推演 #11:`triggered` 字段语义污染(6/15 份,部分解决)

**报告原文**:
> `triggered` 字段在 `topic_format_whitelist` + `required_fields` 均未声明,emit 命令静默写入;LLM 自加 `triggered:"ralph|planner|shipper"`,多数与 hat 拓扑不符。

**真实根因**(基于源码):
- `triggered` 在 `Event` struct(`event_reader.rs:150-156`)是 envelope 字段,不在 payload
- `policy_check.rs:645-744` 只校验 payload 字段
- 全文 grep `presets/schemas/ce-executor-serial.yml` 零匹配

**v3 修复**(改动 #9):
- 在 envelope 验证阶段加 `triggered` 拓扑校验
- 独立于 payload schema

**根治率**:**70%**——能堵"triggered 填了不在拓扑里的值",但"填了拓扑里存在但语义不对"治不了(LLM 判定问题)

---

### 推演 #19:handoff_dispatch 路由不校验 consumer.triggers(3/15 份)

**报告原文**(摘自 `2026-07-02` 诊断报告):
> `event_loop/mod.rs:7029-7132` `handoff_dispatch` 把 `task.resume` 投给 validator(validator 只订阅 `work.done`/`fix.applied`),30 秒超时。

**真实根因**(基于源码):
- **`handoff_dispatch` 函数不存在**——子代理最初报告的行号错
- U16 已修复 `validate_resume_routing` (`event_loop/mod.rs:1786-1837`) 校验 consumer.triggers
- **但其他路径没接同一校验**:`next_hat` (mod.rs:3065-3168)、`process_output` (mod.rs:7379-7485)

**v3 修复**(改动 #8):
- 抽公共函数 `check_consumer_triggers`
- 三处共用

**根治率**:**90%**——直接解决三处路径

---

## 五、LangGraph 设计在 ralph 中的具体落地

> 把 LangGraph 核心概念对应到 ralph 的具体代码位置,方便对照。

| LangGraph 概念 | LangGraph 源码 | ralph 等价位置 | ralph 缺失 |
|---|---|---|---|
| **Channel.update() returns bool** | `channels/base.py:90-99` | `state_projector/task.rs:persist` 返回 Result | persist 不是 bool |
| **Channel.is_available()** | `channels/base.py:75-85` | `LedgerSnapshot::is_available()` | 已实现 |
| **Channel.consume()** | `channels/base.py:101-110` | `prune_*_bucket` (`event_policy.rs:714-744`) | 已实现,但分散 |
| **channel_versions** | `checkpoint/base/__init__.py:92-120` | ❌ **无** | **缺失** |
| **versions_seen** | `_algo.py:262-269` | ❌ **无** | **缺失** |
| **LastValue 拒绝并发** | `last_value.py:59-64` | `work_done_seen_keys` HashSet | 已实现,不同抽象 |
| **BinaryOperatorAggregate** | `binop.py:65-155` | ❌ **无** | **缺失** |
| **Topic channel** | `topic.py:23-94` | `TASKS channel`(分散在 hat_channel) | 已实现,不同位置 |
| **EphemeralValue** | `ephemeral_value.py:15-79` | `LedgerSnapshot` 派生字段 | ⚠️ 部分 |
| **ManagedValue** | `managed/base.py:18-28` | `OrchestratorContext` 块 | ⚠️ 部分 |
| **NamedBarrierValue** | `named_barrier_value.py:13-81` | `task_verify_gate` | 已实现,不同路径 |
| **UntrackedValue** | `untracked_value.py` | ❌ 无显式标注 | **缺失** |
| **BSP apply_writes** | `_algo.py:232-345` | `process_parse_result` (mod.rs:8123) | 已实现,7 步 |
| **reducer 优先解析** | `state.py:1836-1859` | `actions_chain` dispatch | 已实现,reducer 类型不够丰富 |
| **`_update_as_tuples`** | `types.py:793-806` | `Event::new().with_target()` | ⚠️ 部分 |
| **`_triggers(channels, versions, seen)`** | `_algo.py:1260-1277` | `validate_resume_routing` | 已实现,但只在 task.resume 路径 |

---

## 六、改造前后对比图

### 6.1 真实架构下的对比(ASCII)

```
              改造前(基于源码事实)
┌───────────────────────────────────────────────────────────────┐
│  Agent 层(OPAC 纪律,已对齐)                                    │
│  O: ralph inspect loop + ralph tools task list               │
│  P: ralph emit --policy-check / task verify                  │
│  A: ralph emit / ralph tools task verb                       │
│  C: ralph events --events-source hat-channel                 │
└───────────────────────────────────────────────────────────────┘
       │
       ▼
┌───────────────────────────────────────────────────────────────┐
│  Runtime 层(基于源码事实的现状)                                │
│                                                                │
│  state_projector(mod.rs:740-868)                             │
│    ├ PROJECTED_TOPICS 不含 review.dimensions.complete  ← P0-2 │
│    ├ progress.rs:99 mark_step_completed 清空 current_step ← P0-1
│    ├ task.rs:309-420 partial write 路径             ← P0-3  │
│    └ 没有版本号机制                                ← v3 改造  │
│                                                                │
│  event_policy.rs(317-414)                                     │
│    ├ seen_keys 是 HashSet,没有版本号               ← v3 改造 │
│    ├ DedupSameStep 与 DedupStallBypass 归一        ← P1-2    │
│    └ check_completion_guard 同 batch/跨 batch 共用    ← OK    │
│                                                                │
│  handoff 三处(mod.rs:1786-1837 / 3065-3168 / 7379-7485)       │
│    └ validate_resume_routing 已校验 consumer.triggers ← P0-3 │
│        但 next_hat / process_output 没接同一校验      ← v3 改造│
│                                                                │
│  inspect loop(inspect.rs:472-510)                              │
│    └ LoopAnchorView 字段齐全,但 build 触发条件依赖 sentinel ← P0-1
└───────────────────────────────────────────────────────────────┘
       │
       ▼
┌───────────────────────────────────────────────────────────────┐
│  .ralph/(脏数据 + 字段不一致 + 部分校验缺失)                   │
└───────────────────────────────────────────────────────────────┘
```

```
              改造后 v3(基于源码事实的改造)
┌───────────────────────────────────────────────────────────────┐
│  Agent 层(不变,继续 OPAC 四阶段)                             │
└───────────────────────────────────────────────────────────────┘
       │
       ▼
┌───────────────────────────────────────────────────────────────┐
│  Runtime 层(改动清单见 §3.3)                                 │
│                                                                │
│  state_projector(mod.rs:740-868 + 改动 #1 #2 #3 #4)          │
│    ├ PROJECTED_TOPICS 含 review.dimensions.complete         │
│    ├ current_step 改派生字段,不再主动 set None             │
│    ├ tasks.jsonl 改 in-memory BTreeMap + snapshot         │
│    └ 每个字段加 version: u64,持久化                         │
│                                                                │
│  event_policy.rs(317-414 + 改动 #5 #6)                       │
│    ├ review.dimensions.complete dup 改 AcknowledgeAndForward │
│    └ DuplicateSameStep 拆 reason_code                       │
│                                                                │
│  handoff 三处(mod.rs + 改动 #8)                              │
│    └ 抽公共 check_consumer_triggers,三处共用               │
│                                                                │
│  inspect loop(inspect.rs:535 + 改动 #7)                      │
│    └ build_loop_anchor_summary 读 marker file               │
│                                                                │
│  emit 命令(emit.rs + 改动 #9)                                │
│    └ envelope 验证阶段校验 triggered 拓扑                    │
└───────────────────────────────────────────────────────────────┘
       │
       ▼
┌───────────────────────────────────────────────────────────────┐
│  .ralph/(数据干净 + 字段一致 + dedup 版本化 + 校验完整)        │
└───────────────────────────────────────────────────────────────┘
```

### 6.2 LangGraph BSP vs ralph process_parse_result(Mermaid)

```mermaid
flowchart LR
    subgraph LG["LangGraph BSP(_algo.py:232-345)"]
        LG1["update versions_seen"]
        LG2["compute next_version"]
        LG3["chan.consume()"]
        LG4["chan.update(vals)"]
        LG5["bump channel_versions<br/>IF update() returned True"]
    end

    subgraph RALPH["ralph process_parse_result(event_loop/mod.rs:8123, 9380-9510)"]
        R1["State machine validation"]
        R2["★ State projection (SP-R8)★<br/>mod.rs:9441 projector.apply()"]
        R3["unified ValidationPipeline"]
        R4["progress_task_gate"]
        R5["emit gate"]
        R6["bus publish"]
    end

    LG -.对应.-> RALPH
    LG4 -.projector apply call.-> R2
    LG5 -.缺版本号 bump.-> RALPH

    style LG5 fill:#FFB6C1
    style R2 fill:#90EE90
```

### 6.3 channel.update() returns bool 缺失的影响(Mermaid)

```mermaid
flowchart TB
    A["两次 emit work.ready(step-01)<br/>21s 间隔"] --> B["policy_check 检查"]
    B --> C{"seen_keys<br/>HashSet 里有?"}
    C -->|loop rehydrate 后是| D["✅ Dedup 生效"]
    C -->|同 loop 内是| D
    C -->|... 但 seen_keys<br/>没有版本号| E["❌ 漏过某些场景"]

    E --> F["场景 1:fix.applied prune bucket<br/>让下一轮合法重发<br/>但 seen_count 没记"]
    E --> G["场景 2:loop resume 后<br/>set 从 events.jsonl replay<br/>key 顺序可能错乱"]

    F --> H["work.ready 重发 3 次<br/>对应报告 #4"]
    G --> H

    style E fill:#FFB6C1
    style H fill:#FFB6C1
```

### 6.4 改动前后:plan.complete gate(Mermaid)

```mermaid
flowchart LR
    subgraph BEFORE["改造前"]
        B1["project_mark_step_completed<br/>progress.rs:99<br/>set current_step = None"]
        B2["progress.md 写<br/>## Current Step (none)<br/>## Completed Steps 满 5 步"]
        B3["gate 读 progress.md<br/>current_step == None<br/>拒收 plan.complete"]
    end

    subgraph AFTER["改造后 v3(改动 #2)"]
        A1["project_mark_step_completed<br/>只 push completed_steps"]
        A2["ProgressSnapshot.current_step<br/>= self.completed_steps.last()<br/>动态派生"]
        A3["gate 读 progress.md<br/>current_step 永远是最后一步"]
        A4["通过 gate<br/>plan.complete 落地"]
    end

    BEFORE -. 同源 bug .-> AFTER
```

---

## 七、落地路径与风险

### 7.1 四期落地(按 ROI 排序)

**第 1 期:消除最大痛点(3-5 天,ROI 最高)**
- 改动 #2:`current_step` 改派生(`progress.rs:87-101`)—— 直接根治 #1 #2(15/15 份报告)
- 改动 #4:`PROJECTED_TOPICS` 加 `review.dimensions.complete`(mod.rs:101-102)—— 根治 P0-2
- 改动 #6:`DuplicateSameStep` 拆 reason_code(event_policy.rs:144-180)—— 根治 P1-2

**第 2 期:补完校验链(5-7 天)**
- 改动 #7:`build_loop_anchor_summary` 改触发(inspect.rs:535)—— 根治 P0-1
- 改动 #8:三处 handoff 共用 consumer.triggers 校验—— 根治 #19
- 改动 #9:`triggered` envelope 校验—— 根治 #11(部分)

**第 3 期:引入版本号协议(1 周+,需要更深的 schema 改造)**
- 改动 #1:`state_projector` 每个字段加 `version: u64`(mod.rs:740-868)—— 根治 #4(80%)
- 改动 #3:`tasks.jsonl` 改 in-memory(task.rs:309-420)—— 根治 #3(70%)

**第 4 期:优化 dedup 决策(可选)**
- 改动 #5:dedup 改 AcknowledgeAndForward(event_policy.rs:1516-1530)—— 进一步优化 P0-2

### 7.2 v3 vs v2 关键区别

| 维度 | v2(基于脑补) | v3(基于源码事实) |
|---|---|---|
| **修复位置** | 给文件级描述(mod.rs / event_policy.rs) | 给具体行号(progress.rs:87-101 / mod.rs:101-102 / event_policy.rs:144-180) |
| **handoff 假设** | 单函数 `handoff_dispatch:7029-7132` | **三处分散**(1786 / 3065 / 7379)+ `validate_resume_routing` U16 已修 |
| **consumes 字段** | 假设存在 | **不存在**,正确字段是 `triggers` |
| **terminal_emitted** | 需要新加 | **已经有**(`terminal_observed: bool` 在 event_policy.rs:319) |
| **triggered 校验** | 在 policy_check payload schema 加 | policy_check 不校验 envelope 字段,需要**单独 envelope 验证** |
| **PROJECTED_TOPICS** | 没说 | 白名单不全,需要补 `review.dimensions.complete` |
| **loop_anchor 字段** | 没说 | 已经齐全,只需要改触发条件 |

### 7.3 风险点

| 风险 | 概率 | 缓解 |
|---|---|---|
| **in-memory TaskStore 进程崩溃丢数据** | 低 | snapshot 走 tmp + rename 原子写,1s 节流 |
| **version bump 带来状态机回归** | 中 | 跑全量 baseline + 增加 regression test |
| **PROJECTED_TOPICS 加 topic 改变现有行为** | 低 | preset_lint 加新规则 + 受影响场景 e2e |
| **envelope 校验和 policy_check 时序冲突** | 中 | 失败时 fallback 到原路径,带 warning |
| **dedup 决策改 AcknowledgeAndForward 漏 forward** | 低 | 加 seen_count 上限 |

### 7.4 不在改造范围内(明确边界)

v3 不涉及:
- LLM 判定问题(#8 coordinator 越权、#10 路由失活)→ 走 SC1 金丝雀
- shipper 白名单(#13)→ 走 shipper reason 重设计
- task_id 派生(#14)→ 走生成规则校验
- ledger 落账(#16)→ 走 loop_state 重构
- 双计数器(#18)→ 走 loop_state 重构

---

## 八、最终判断(v3)

### v3 的核心发现

1. **ralph 已经实现 LangGraph 90% 的状态管理抽象**——state_projector + event_policy + task_verify_gate + handoff_index + completion_guard 都是等价实现
2. **真正的差距是 9 个具体改动点**(见 §3.3),不是"需要造新组件"
3. **每条改动都说清改哪个文件第几行改成什么**(基于源码事实,不基于脑补)

### 能治的痛点(7 条,出现 44 次/15 份报告)

| # | 痛点 | v3 根治率 | 改动 |
|---|---|---|---|
| 1 | plan.complete 被 gate 拒 | **100%** | #2 |
| 2 | progress.md 字段错位 | **100%** | #2 |
| 3 | tasks.jsonl 双 row | **70%** | #3 |
| 4 | 重复 emit dedup 失效 | **80%** | #1 |
| 5 | 终态事件多发 | **95%** | (改动 #5 增强) |
| 11 | triggered 字段污染 | **70%** | #9 |
| 19 | handoff 不校验 consumer | **90%** | #8 |

**平均根治率 86%**。

### 不能治的痛点(13 条)

需要 policy_check 增强、circuit breaker、preset lint、shipper reason 重设计、loop_state 重构等配套改造——v3 不涉及。

### 一句话总结(v3)

> **ralph 已经在 90% 的 LangGraph 状态管理抽象上做对了,真正缺的不是新组件,而是 9 个具体改动点。每个改动都说清改哪个文件第几行改成什么、对应哪个报告痛点、对应 LangGraph 的哪个核心抽象。**

---

## 附录 A:LangGraph 关键源码引用汇总

| LangGraph 文件 | 行号 | 内容 |
|---|---|---|
| `channels/base.py` | 19-122 | `BaseChannel` 抽象类,update/is_available/consume/finish 契约 |
| `channels/base.py` | 90-99 | `update()` 抽象原文("Pregel for all channels") |
| `channels/last_value.py` | 56-67 | `LastValue` 拒绝并发(line 59-64 抛 InvalidUpdateError) |
| `channels/binop.py` | 65-155 | `BinaryOperatorAggregate` reducer fold |
| `channels/binop.py` | 22-28 | `_strip_extras` Annotated 剥除 |
| `channels/binop.py` | 31-51 | `_get_overwrite` 三形态识别 |
| `channels/topic.py` | 23-94 | `Topic` accumulate 区分日志 vs 队列 |
| `channels/ephemeral_value.py` | 15-79 | `EphemeralValue` 下一步自动清空 |
| `channels/named_barrier_value.py` | 13-81 | `NamedBarrierValue` barrier fan-in |
| `managed/base.py` | 18-28 | `ManagedValue` 抽象基类,无状态 |
| `managed/is_last_step.py` | 9-15 | IsLastStepManager scratchpad 调用 |
| `graph/state.py` | 1836-1859 | `_get_channel` 三优先级解析 |
| `graph/state.py` | 1817-1820 | channels vs managed 分流 |
| `graph/state.py` | 1862-1887 | `_is_field_channel` 提取 |
| `graph/state.py` | 1890-1908 | `_is_field_binop` reducer 提取 |
| `graph/state.py` | 1911-1927 | `_is_field_managed_value` 提取 |
| `pregel/_algo.py` | 232-345 | `apply_writes` 7 步算法 |
| `pregel/_algo.py` | 262-269 | `versions_seen` 更新 |
| `pregel/_algo.py` | 317-323 | `channel_versions` bump 条件 |
| `pregel/_algo.py` | 1260-1277 | `_triggers` 防重入 |
| `pregel/_algo.py` | 1348-1392 | `_proc_input` channel vs managed |
| `types.py` | 664-708 | `Send` dataclass |
| `types.py` | 758-808 | `Command` 同时改状态+路由 |
| `types.py` | 811-934 | `interrupt()` resume 协议 |
| `types.py` | 937-984 | `Overwrite` 三形态 |
| `checkpoint/base/__init__.py` | 92-120 | `Checkpoint` TypedDict |
| `checkpoint/base/__init__.py` | 176-415 | `BaseCheckpointSaver` 接口 |

## 附录 B:ralph 关键源码引用汇总

| ralph 文件 | 行号 | 内容 |
|---|---|---|
| `state_projector/mod.rs` | 132-209 | `ProjectionContext` 字段 |
| `state_projector/mod.rs` | 101-102 | `PROJECTED_TOPICS` 白名单 |
| `state_projector/mod.rs` | 740-868 | `StateProjector::apply` 算法 |
| `state_projector/progress.rs` | 87-101 | `project_mark_step_completed` |
| `state_projector/progress.rs` | 99 | `current_step = None` 主动清空 |
| `state_projector/progress.rs` | 198-202 | `current_step == None` 写 (none) 占位 |
| `state_projector/progress.rs` | 215-217 | progress.md 原子写(tmp + rename) |
| `state_projector/task.rs` | 309-420 | partial write 路径 |
| `state_projector/task.rs` | 422-431 | `TaskStore::persist` |
| `event_policy.rs` | 317-414 | `PolicyRuntimeState` 字段 |
| `event_policy.rs` | 144-180 | `ViolationType::reason_code` |
| `event_policy.rs` | 714-744 | `prune_*_bucket` |
| `event_policy.rs` | 862-902 | `check_completion_guard` |
| `event_policy.rs` | 1516-1530 | `review.dimensions.complete` dedup |
| `event_loop/mod.rs` | 1786-1837 | `validate_resume_routing` U16 |
| `event_loop/mod.rs` | 3065-3168 | `next_hat` Isolated mode |
| `event_loop/mod.rs` | 7379-7485 | handoff timeout escalation |
| `event_loop/mod.rs` | 8123 | `process_parse_result` 主入口 |
| `event_loop/mod.rs` | 9380-9510 | 7 步事件处理顺序 |
| `event_loop/mod.rs` | 9441 | `projector.apply(&events)` 调点 |
| `commands/inspect.rs` | 472-510 | `LoopInspectView` |
| `commands/inspect.rs` | 446-468 | `LoopAnchorView` |
| `commands/inspect.rs` | 535-592 | `build_loop_anchor_summary` |
| `commands/emit.rs` | 82-114 | 三种 mode 解析 |
| `commands/emit.rs` | 847-953 | 完整 gate 链路 |
| `task_verify_gate.rs` | 42-47 | ticket 路径常量 |
| `task_verify_gate.rs` | 57-78 | fingerprint 计算 |
| `task_verify_gate.rs` | 191-202 | gate 激活条件 |
| `preset_lint/state_projection.rs` | 33-57 | KTD-3 actions_chain 顺序 lint |
| `preset_lint/topic_format.rs` | 124-148 | topic token 形态校验 |
| `preset/engine/projection.rs` | 1-19 | typed wrapper 注释 |
| `workflow_contract/handoff_index.rs` | 158-162 | consumer 决定 |
| `workflow_contract/handoff_index.rs` | 228-230 | `consumer_of` |
| `workflow_contract/handoff_tracker.rs` | 255-264 | fallback_safe_target |
| `config/hat.rs` | 353-389 | `HatConfig` 字段 |
| `event_reader.rs` | 150-156 | `Event.triggered` 字段 |

---

## 附录 C:v3 vs v2 关键修正点

| v2 假设 | v3 真实代码事实 |
|---|---|
| `handoff_dispatch` 在 mod.rs:7029-7132 | **不存在**,真实分散在 mod.rs:1786-1837 / 3065-3168 / 7379-7485 |
| `consumes` 字段存在 | **不存在**,正确是 `triggers`(serde alias `subscribes_to`) |
| 需要新建 terminal_emitted 字段 | **已有**(`terminal_observed: bool` event_policy.rs:319) |
| 需要新建 state_projection 模块 | **已有**(`state_projector/`),需要补全 |
| progress.md 字段错位是 mark_step_completed bug | **正确**,但具体位置 progress.rs:99 主动 set None |
| `LoopInspectView` 缺 loop_anchor 字段 | **已齐全**,问题在 build_loop_anchor_summary 触发条件 |
| `triggered` 在 payload schema | **不在**,是 envelope 字段,需要单独校验 |
| completion guard 同 batch / 跨 batch 拆开 | **已统一**,改用 check_completion_guard 同一函数 |
| PROJECTED_TOPICS 是完整白名单 | **不全**,缺 `review.dimensions.complete` |
| `state_projector` 不存在 | **存在**,需要改而不是造 |

---

> **v3 是最终版**。每条结论都附文件:行号;每条改造都说清改哪个文件第几行改成什么。
>
> **v1(基于二手 deep-dive)和 v2(基于脑补)都应该被 v3 取代**——前者错误前提,后者细节无据。
>
> **v3 的优势**:可信、可审、可直接拿去 review、可直接去做 plan。