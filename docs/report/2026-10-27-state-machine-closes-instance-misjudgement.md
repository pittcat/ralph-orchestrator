# 状态机 `closes_instance` 误判：投影层形状启发式导致多步实验链在第二个业务事件处被强制关闭

- **日期**：2026-10-27
- **基线**：HEAD `09d28843`（`fix(bootstrap): 尊重项目测试规则并修复套件刷新`，2026-10-10）；发布二进制 `~/.cargo/bin/ralph` = `target/release/ralph`，sha256 `3caa014a…`（与 HEAD 同源构建）
- **对象**：`event_loop.state_machine` 的**投影层**（写进 ledger 的 `StateMachineTransitionDelta` 的 `opens_instance` / `closes_instance` 标志）与**校验层**（`validate_business_event`）的 open/close 语义一致性
- **缺陷引入**：`b8bdca87`（2026-08-16，"opens_instance semantic"，计划 2026-08-15-2211 U5）
- **方法**：源码静态链路审计（每一环亲自读码闭合）+ 消费方项目的一次真实运行取证（ledger 投影 + accepted 事件流 + emit 回执三方互证）
- **证据等级**：【静态实锤】= 代码链每一环均读码闭合；【运行实证】= 消费方真实运行留下的账本/事件流证据
- **受影响消费方**：`nuttx_source_tutorial` AutoResearch preset（`execution_mode: isolated`，5 段实验链 `planned → ready → measured → scored → attacked → evaluated`），循环 `primary-20261026-121511` 已因此 abort

---

## 结论摘要

状态机存在**两套互不一致的 open/close 语义**：

- **校验层**（`crates/ralph-core/src/state_machine.rs`）读 preset 转移表声明的 `transition.opens_instance` / `transition.closes_instance`——**声明驱动**；
- **投影层**（`crates/ralph-core/src/event_loop/state_machine_stage.rs`）改用「pre-validation 实例映射的形状」反推这两个标志——**形状驱动**，且 `closes_instance` 的推导条件是**恒真式**：只要实例当前处于 open 映射中，无论该转移是否声明 close，一律判为 `closes=true`。

两者在「open 之后的第一条中间态转移」上必然分叉。于是**任何 `opens_instance=true` 之后跟一条未声明 close 的中间态转移的链，都会在第二个业务事件处把实例标记为关闭**；该标志被持久化进 ledger 与 accepted-transitions，冷启动重放会再次施加同样的关闭，因此**不可自愈**。下游表现是：紧随其后的业务事件被拒收（`Instance '...' has already been closed at state '...' (topic '...'). Reopening is not allowed.`），事件链断裂，`required_events` 永不满足，`LOOP_COMPLETE` 不可达。

| # | 严重度 | 发现 | 证据等级 |
|---|---|---|---|
| F1 | **P0** | 投影层用映射形状反推 `closes_instance`，对已 open 实例恒判 `closes=true`；与校验层的转移声明语义分叉 | 【静态实锤 + 运行实证】 |
| F2 | P1 | 错误投影被持久化进 ledger / accepted-transitions，冷启动重放再次施加 → 不可自愈 | 【静态实锤 + 运行实证】 |
| F3 | P1 | 现有回归测试只覆盖「open → close」两事件链，**无任何 `A(opens) → B(mid) → C(mid)` 三连用例**，缺陷与测试一起漏掉 | 【静态实锤】 |
| F4 | P2 | terminal 事件（`LOOP_COMPLETE`）无对应 transition，形状启发式给出 `opens=true`，`apply_transition_delta` 会把 terminal 的 instance_key **插入** `open_instances`——语义上等于"终态事件开了一个新实例" | 【静态实锤】 |
| F5 | P2 | `StateMachineRuntimeState::classify_open_close` 是同一套形状启发式的公开副本，注释断言"the flags that the validator's open/close decision would produce"——该假设本身错误，构成第二套待清理的语义源 | 【静态实锤】 |
| F6 | P3 | 消费方侧两条广为流传的恢复路径（清理 ledger 错误投影 / 换新 `task_key`）实际**都不能解决问题**，需在下游文档中更正 | 【静态实锤】 |

---

## F1 [P0] 投影层与校验层的 open/close 语义分叉

### 两处判定的代码对照

**校验层**（`crates/ralph-core/src/state_machine.rs:585-608`，`validate_business_event`）——读转移声明：

```rust
if transition.opens_instance {
    self.open_instances.insert(k.clone(), InstanceState { state: new_state.clone(), last_topic: topic.to_string() });
} else if transition.closes_instance {
    if self.open_instances.remove(k).is_some() {
        self.closed_instances.insert(k.clone(), InstanceState { state: new_state.clone(), last_topic: topic.to_string() });
    }
} else {
    // Advance state
    if let Some(instance) = self.open_instances.get_mut(k) {
        instance.state = new_state.clone();
        instance.last_topic = topic.to_string();
    }
}
```

**投影层**（`crates/ralph-core/src/event_loop/state_machine_stage.rs:88-97`，candidate 阶段）——读映射形状：

```rust
// Capture the candidate's instance maps BEFORE the validator
// mutates them so the open/close decision reflects the
// pre-validation state — validate_event inserts the
// transition's effect into the maps, so a post-mutation read
// would always report `opens_instance=false, closes_instance=true`
// for opening transitions, which inverts the semantics the
// apply stage relies on.
let (pre_opens_map, pre_closed_map) = { … };

let opens_instance  = !pre_opens_map.contains_key(key_ref) && !pre_closed_map.contains_key(key_ref);
let closes_instance =  pre_opens_map.contains_key(key_ref) && !pre_closed_map.contains_key(key_ref);
```

第二个条件与转移声明无关：**实例一旦 open，任何后续业务事件都会被判成 `closes=true`**。注释中"避免 open 语义被反转"的诉求成立，但用形状去猜校验器的判定结果，在校验器实际读转移声明的前提下，对「中间态推进」是系统性错误。

`state_machine_stage.rs:388-402` 把这两个标志原样交给 `project_transition_delta`（`state_machine.rs:649-685`，纯透传，不做二次判断），随后 `apply_transition_delta`（`state_machine.rs:718-747`）按 `delta.closes_instance` 把 key 从 `open_instances` 移入 `closed_instances`：

```rust
if delta.closes_instance {
    self.open_instances.remove(key);
    self.closed_instances.insert(key.to_string(), InstanceState { … });
} else if delta.opens_instance { … }
```

### 完整因果链（消费方一次真实运行，逐环可验）

1. 消费方 preset 的转移表：`experiment.planned` 声明 `opens_instance: true`；`experiment.ready` / `measured` / `scored` / `attacked` **未声明 close**；仅 `experiment.evaluated` / `experiment.blocked` 声明 `closes_instance: true`。
2. `experiment.planned` 被接受 → 实例进入 `open_instances`。此时 pre-map 不含该 key，形状启发式给出 `opens=true, closes=false`——**恰好正确**（消费方 ledger 第 1 条 `loop.state_machine_projection` 即为此值）。
3. `experiment.ready` 到达 → candidate 阶段取 pre-validation 快照，`pre_opens_map` **已含**该 key。
4. 形状启发式给出 `opens=false, closes=true`——**preset 声明的 `closes_instance` 从未被读取**。
5. 标志被封入 delta 并落盘：消费方 ledger 第 4 条 `loop.state_machine_projection` 记录 `topic=experiment.ready, opens=false, closes=true`；同一投影写入 `accepted-transitions.jsonl`（`activation_id=implementer:2`）。【运行实证】
6. `apply_transition_delta` 按 `closes=true` 把 key 移入 `closed_instances` → live 状态机认为实例已在 `ready` 关闭。
7. 校验层命中 `closed_instances`，直接 Reject（`state_machine.rs:519-534`），错误消息为 `Instance '…' has already been closed at state 'ready' (topic 'experiment.ready'). Reopening is not allowed.`。
8. 消费方 accepted 事件流中 `experiment.measured` **缺席**（`experiment.start → experiment.planned → experiment.ready → loop.cancel`），而 `emit_attempt_receipts.jsonl` 记录该事件 `recorded_state=recorded`——**写盘成功、接收侧拒收**。【运行实证】
9. `experiment.scored` / `evaluated` 无上游 → `required_events` 永不满足 → `LOOP_COMPLETE` 不可达 → 循环空转至 stall 阈值后只能 abort。

### 预测检验

若根因是形状启发式，则**任何** `opens_instance=true` 之后的第一条非关闭转移都会得到 `closes=true` 投影，且与 preset 内容、实例 key、payload、时序均无关。

- 观测 1：消费方 preset 明确未给 `experiment.ready` 声明 close，投影却是 `closes=true`——一致。
- 观测 2：从 idle 打开的 `experiment.planned` 投影正确（`opens=true, closes=false`）——与"仅在实例已 open 时触发"一致。
- 反向排除：若根因是 preset 写错，ledger 投影应能在 preset 中找到声明依据；实际投影与 preset 声明**直接矛盾**，矛盾方是运行时。

---

## F2 [P1] 错误投影持久化 → 冷启动重放再次施加 → 不可自愈

- delta 经 `commit_state_machine_projection` 持久化到 ledger 与 accepted-transitions。
- 冷启动时 `crates/ralph-core/src/state/snapshot.rs:536-558` 对每条 `CommitDelta::StateMachineTransition` 调用 `target.apply_transition_delta(delta)` 重建 runtime；`crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs:530-538` 再把快照中的 `state_machine_runtime_state` 装配回运行态。
- 因此**重启 loop 会立即回到同一 closed 状态**；且 preset 中没有任何 transfer 能从 `closed` 状态清空该 key（对 closed 实例的转移同样被 Reject）。

---

## F3 [P1] 测试覆盖缺口

`crates/ralph-core/src/event_loop/tests/state_machine.rs` 中所有涉及 `closes_instance` 的用例（:99 / :155 / :711 / :781 / :872 / :964 / :1303 / :1499 / :1676 / :1769 / :1787 / :1918 / :2060）覆盖的都是**两事件链**（`opens → closes`）或"实例尚未打开"的情形（:1022-1023 断言 `B revalidated against empty live must show closes_instance=false`）。

**不存在任何 `A(opens) → B(mid) → C(mid)` 三连用例**，也没有对 ledger 投影中 `closes_instance` 标志的断言——投影层写出的错误值从未被测试观测到。这正是缺陷能随 `b8bdca87` 一路存活到发布二进制的原因。

---

## F4 [P2] terminal 事件的相邻风险

`LOOP_COMPLETE` 走同一 Accept 分支，但 `terminal_topics` 不在 `transitions` 表中。形状启发式在实例未 open 时给出 `opens=true, closes=false`，`apply_transition_delta` 随即把 terminal 的 `instance_key` **插入** `open_instances`——语义上等于"终态事件开了一个新实例"。当前 preset 的 `terminal_guard.require_no_open_instances` 多半掩盖了该行为，但修复 F1 时必须显式处理"无对应 transition"分支（见 §修复方案要点 2）。

---

## F5 [P2] `classify_open_close` 是同一套错误假设的公开副本

`crates/ralph-core/src/state_machine.rs:693-702`：

```rust
pub fn classify_open_close(&self, key: &Option<String>) -> (bool, bool) {
    let key = match key { … };
    let opens  = !self.open_instances.contains_key(key) && !self.closed_instances.contains_key(key);
    let closes =  self.open_instances.contains_key(key) && !self.closed_instances.contains_key(key);
    (opens, closes)
}
```

注释称其返回 "the flags that the validator's open/close decision would produce"——**该断言不成立**：校验器读的是转移声明。修复 F1 时应同步废弃此方法或让其委托同一份转移声明逻辑，避免留下第二套语义源。（当前调用点仅在 `crates/ralph-core/src/state/tests.rs:2451/2484`。）

---

## F6 [P3] 两条流传中的恢复路径实际无效

消费方先前的诊断文档给出的恢复选项需要更正——以下两条**都不能解决问题**：

| 建议 | 结论 | 理由 |
|---|---|---|
| 清理 ledger 第 4/5 条 `experiment.ready` 投影 + `accepted-transitions.jsonl` 对应行 | ❌ 无效 | 删掉错误投影后实例仍处于 open(`planned`)；重放时 `experiment.ready` 再次被接受 → 形状启发式**再次**算出 `closes=true` → 下一事件再次被拒。只是把故障推迟一轮。 |
| 换新 `task_key` 重开实验链 | ❌ 无效 | 启发式条件是"该 key 当前在 open 映射中"，与 key 本身无关。新 key 的 `planned` 会正确得到 `opens=true`，但其**下一条** `ready` 依然命中恒真条件 → 以完全相同的方式挂掉。 |

有效路径只有上游修复；或临时关闭 preset 的 `event_loop.state_machine.enabled`——但这并非零成本：state machine 关闭时 ralph 改用 workflow guard 链做完成判定（`crates/ralph-core/src/event_loop/wave_scope.rs:790-800` → `crates/ralph-core/src/event_loop/parse_and_emit/legacy.rs:226-275`），而该机制以 guard chain 的**末位 topic** 为终态相位，对红队并非每轮触发的链需单独验证；实例生命周期守卫（`require_no_open_instances`）同时失效。

---

## 修复方案

### 核心改动

`crates/ralph-core/src/event_loop/state_machine_stage.rs:93-97` —— 投影层改为**转移声明驱动**，与校验层对齐：

```rust
// 现状（错）：从 pre-validation 映射形状反推
let opens_instance  = !pre_opens_map.contains_key(key_ref) && !pre_closed_map.contains_key(key_ref);
let closes_instance =  pre_opens_map.contains_key(key_ref) && !pre_closed_map.contains_key(key_ref);

// 改为：直接读 preset 转移表（sm_config 在本函数作用域内可用）
let (opens_instance, closes_instance) = sm_config
    .transitions
    .iter()
    .find(|t| t.topic == topic)
    .map(|t| (t.opens_instance, t.closes_instance))
    .unwrap_or((false, false));   // terminal topic 无对应 transition → 两者皆 false
```

要点：

1. `TransitionDef` 的 `opens_instance` / `closes_instance` 为 `#[serde(default)] bool`（`crates/ralph-core/src/config/state_machine.rs:58-62`），缺省即 `false`，与校验层读取口径一致——两层就此对齐。
2. **terminal topic 没有对应 transition，必须走 `unwrap_or((false, false))`**；否则会复现 F4 的"终态开实例"。
3. `b8bdca87` 原本要解决的 TOCTOU（open 转移被误判为 closes）由"读转移声明"天然解决，不再需要 pre-map 快照；`pre_opens_map` / `pre_closed_map` 的采集代码可一并清理。
4. `StateMachineRuntimeState::classify_open_close`（F5）同步废弃或改为委托同一份声明逻辑。

### 必补回归测试

置于 `crates/ralph-core/src/event_loop/tests/state_machine.rs`：

- **三连链（核心）**：`planned(opens) → ready(mid) → measured(mid)`，断言 `ready` 的 delta 为 `opens=false, closes=false`，且 `measured` 被 Accept。
- **尾部 close**：`… → evaluated(closes_instance=true)` 的 delta 必须 `closes=true`，实例从 `open_instances` 移入 `closed_instances`。
- **terminal 分支**：`LOOP_COMPLETE` 的 delta 必须 `opens=false, closes=false`，且不得向 `open_instances` 插入任何 key（F4）。
- **冷启动重放**：写入 ledger 后重建 runtime，实例映射状态与在线路径一致，防止"重放即关闭"回归（F2）。
- **投影值断言**：直接断言写入 ledger 的 `loop.state_machine_projection` 中 `closes_instance` 字段——F3 的缺口正是在这里。

---

## 验证方法

```bash
cd <ralph-orchestrator>

# 1) 静态复核：投影层是否仍用形状启发式
sed -n '88,100p' crates/ralph-core/src/event_loop/state_machine_stage.rs

# 2) 回归测试
cargo test -p ralph-core --lib event_loop::tests::state_machine

# 3) 重建并安装发布二进制
cargo build --release && install -m 0755 target/release/ralph ~/.cargo/bin/ralph
sha256sum ~/.cargo/bin/ralph        # 应与旧值 3caa014a… 不同

# 4) 端到端复测（消费方，新 task_key）
#    判据：ledger 的 loop.state_machine_projection 中
#    experiment.ready 的 closes_instance 必须为 false
```

第 4 步判据若仍为 `true`，说明二进制未替换或改动未生效。

---

## 证据清单（本仓库内）

| 位置 | 用途 |
|---|---|
| `crates/ralph-core/src/event_loop/state_machine_stage.rs:80-97` | 缺陷代码本体（pre-map 快照 + 形状推导） |
| `crates/ralph-core/src/event_loop/state_machine_stage.rs:388-416` | 标志进入 delta 并 apply 到 live |
| `crates/ralph-core/src/state_machine.rs:519-534` | closed 实例拒收（错误消息出处） |
| `crates/ralph-core/src/state_machine.rs:585-608` | 校验层的声明驱动语义 |
| `crates/ralph-core/src/state_machine.rs:649-685` | `project_transition_delta` 纯透传 |
| `crates/ralph-core/src/state_machine.rs:693-702` | `classify_open_close` 启发式副本（F5） |
| `crates/ralph-core/src/state_machine.rs:718-747` | `apply_transition_delta` 按 delta 关闭实例 |
| `crates/ralph-core/src/state/snapshot.rs:536-558` | 冷启动重放 delta（F2） |
| `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs:530-538` | runtime rehydrate 装配点（F2） |
| `crates/ralph-core/src/config/state_machine.rs:58-62` | `TransitionDef` 的 flags 字段与 serde default |
| `crates/ralph-core/src/event_loop/tests/state_machine.rs:99/155/711/1022-1023` | 现有两事件链用例（F3 覆盖缺口） |
| `crates/ralph-core/src/event_loop/wave_scope.rs:790-800`、`parse_and_emit/legacy.rs:226-275` | state machine 关闭时的完成判定替代路径（F6） |
| `b8bdca87` | 缺陷引入提交 |

## 消费方侧运行证据（不在本仓库，供交叉核对）

| 文件 | 用途 |
|---|---|
| `.ralph/ledger.jsonl` seq=1 / seq=4 | 两条 `loop.state_machine_projection`：`closes=false` → `closes=true` 的转折点 |
| `.ralph/agent/accepted-transitions.jsonl`（`strategist:1` / `implementer:2`） | 同一投影的副本 |
| `.ralph/events-history-*.jsonl` | accepted 流中 `experiment.measured` 缺席 |
| `.ralph/agent/emit_attempt_receipts.jsonl` | `experiment.measured` 已 `recorded`（写盘成功、接收被拒） |
