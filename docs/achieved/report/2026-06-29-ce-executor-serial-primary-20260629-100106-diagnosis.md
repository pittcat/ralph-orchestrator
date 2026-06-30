# ce-executor-serial primary-20260629-100106 链路诊断报告

> 报告生成时间:2026-06-29 11:08 UTC
> 诊断对象:`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/`,loop=`primary-20260629-100106`
> preset:`builtin:ce-executor-serial`(`presets/en/ce-executor-serial.yml`)
> plan:`docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`

---

## 1. 结论摘要

**本次 run 健康度:🔴 致命失败 — review chain 在最后一步断裂,5 道修复/兜底路径全部失效,recovery 循环触发同一失败,最终 LOOP_COMPLETE 没能干净收尾。**

- **关键异常**:6 个 P0(全部命中)+ 3 个 P1
- **历史重复性**:✅ 是 — **6/6 症状点 100% 命中 30 天内未闭环清单**,本次是第 12+ 次同型复发(2026-06-17 merry-lotus 起)
- **主因排序**(按归因密度):preset 拓扑缺陷 > 基座 `terminal_topics` 注册缺失 > recovery 重发目标错误 > progress-steward 自闭合缺失
- **实现产物本身**:✅ 完整正确(69 tests pass,sorts/ 4 个 step 全完成)

---

## 2. 执行链路对比图

```
预期 flow(unit_loop → review_walk → plan_end → ship):
[unit_loop×4]
step-01..04: work.ready → work.done → test.passed     ✅ ×4 (69 tests)
       ↓ all_done
[review_walk]
review.start → ready×6 ⇄ done×6                       ✅ 6/6 dim
            → review.dimensions.complete              → review-synthesizer
              → review.complete(fix_plan_file="null")
              → plan.complete
[plan_end] → [ship] → REVIEW_COMPLETE → report.done → LOOP_COMPLETE

实际 flow:
[unit_loop×4] 全部 ✅ (4 step 完成,69 tests pass)
       ↓
[review_walk] ✅ 6/6 dim done
       ↓
review.dimensions.complete #28  ❌ FlowStepScope: flow_unknown_emit
       ↓ (current_step 未从 unit_loop 推进 → review.dimensions.complete 不在 allowed_emits)
       ↓ review-synthesizer 永不激活 → review.complete / plan.complete 全部缺失
       ↓
human.guidance #29 (ralph 兜底)      ⚠️ ralph 不在 progress-steward triggers 内
LOOP_COMPLETE #30                     ❌ missing_field(缺 report.done)
task.resume #31(missing_event_gate)  — progress-steward 唤醒 coordinator
plan.blocked #32                     ❌ terminal_monotonicity_violation ×2
task.resume #33(plan_complete_not_emitted)
plan.blocked(hat-channel) #35        ❌ terminal_monotonicity_violation
recovery.jsonl 反复 dispatch 同一失败事件(repair_dispatch ×5 + semantic_gate_violation ×2)
       ⇣
[SHUTDOWN:implementation_complete, review_chain_broken]
```

---

## 3. 历史问题上下文

| 历史问题 | 类型 | 关联度 | 是否闭环 | 历史 case 数 |
|---------|------|--------|---------|-------------|
| **A1** `review.dimensions.complete` 被 `FlowStepScope` 拒(`flow_unknown_emit`) | event chain | **极高** | **未闭环**(P0-1,30 天 12+ 次) | 4+(keen-fern / warm-tiger / 28-loop-and-mech / 115810 / 032235) |
| **A2** `review-synthesizer` 30s `handoff_dispatch_timeout` 死信 | event chain | **极高** | **未闭环** | 11+(merry-lotus 起) |
| **B1** `plan.blocked` 后终态机未拦截 review 链继续 fire | terminality | **极高** | **未闭环** | 4+(28-070436 / 28-115810 / 032235) |
| **C1** `required_events: ["report.done"]` 缺失致 LOOP_COMPLETE 拒 | required_events | **极高**(本次字面同型) | **未闭环** | 6+(keen-fern / 24-092856 / 26-5dim / 28-loop-and-mech / 032235) |
| **D1** coordinator/ralph hat 在 isolated mode 下越权发 `loop.stalled` / `human.guidance` | semantic_gate | **极高**(本次字面同型) | **未闭环**(P0 D3,30 天 9+ 次) | 9+(merry-lotus / warm-tiger / 28-070436 / 28-115810 / 28-172725 / 032235) |
| **F1** `recovery_exhausted` 不走 `plan.blocked` 终态链路 | recovery | **极高**(本次 recovery stream 循环触发同型) | **未闭环** | 6+(merry-lotus / noble-peacock / 28-070436 / 28-115810 / 28-172725 / 032235) |
| **G2** preset `completion_promise: LOOP_COMPLETE` 与 fail 路径不 emit 矛盾 | architecture | **极高** | **未闭环** | — |

**关键判断**:**2026-06-28-004 fix plan(16 Unit)虽已全部 commit 落地,本次 100106 run 仍触发 6/6 同型症状**——证明 004 plan 落地后**回归**或**未完全覆盖**这 3 类根因:
1. `recovery_exhausted` 不走 `plan.blocked` 终态链路
2. `flow_lifecycle.current_step_id()` 字段语义错位(`record.source_topic` 而非 step id)
3. `stall_recovery` / `missing_event_gate` 双轨 retry_key 不感知

---

## 4. 证据清单

> 关键偏离 + 具体文件/行号/事件 ID

### 4.1 P0 偏离(致命,直接阻断链路)

| # | 偏离 | 证据 |
|---|------|------|
| **D1** | `review.dimensions.complete` 被 FlowStepScope 拒 | `events-20260629-100106.jsonl:28` ts=10:52:07 source=`review-coordinator` payload 含 6 required_fields 全在场 |
| **D2** | `review.complete` / `plan.complete` / `REVIEW_COMPLETE` / `report.done` 全部缺失 | `events-20260629-100106.jsonl:1-34` 共 34 行,4 个 topic 出现 0 次 |
| **D3** | LOOP_COMPLETE 拒收 `missing required event: report.done` | `ledger.jsonl:31` `rejection_recorded` key=`policy:unknown:loop.complete:missing_field` message=`LOOP_COMPLETE rejected: missing required events: ["report.done"]` |
| **D4** | `plan.blocked` 触发 `terminal_monotonicity_violation` | `ledger.jsonl:33,34` 双 reject_recorded reason=`event_policy:event_policy:terminal_monotonicity_violation` |
| **D5** | coordinator 代发 `loop.stalled` / `human.guidance` 触发 `semantic_gate_violation` | `recovery.jsonl:6,8` 字面匹配 `isolated scope violation: hat 'coordinator' is not allowed to publish topic 'loop.stalled'; allowed publishes: ["work.ready", "review.start", "plan.complete", "plan.blocked", "LOOP_COMPLETE"]` |
| **D6** | recovery stream 反复派发同一失败 LOOP_COMPLETE | `recovery.jsonl:4,7,10` 三次 `repair_dispatch topic=LOOP_COMPLETE`,reason 从 `implementation_complete_review_chain_broken_at_final_step` → `review_chain_broken_implementation_verified` → `review_chain_broken_at_dimensions_complete:implementation_complete_69_tests_pass:ralph_cannot_emit_business_topic` |

### 4.2 P1 偏离(阻断修复路径)

| # | 偏离 | 证据 |
|---|------|------|
| **D7** | `review-coordinator` `publishes` 无 `review-synthesizer` 唤醒链路,`triggers` 也不含 `task.resume` | `presets/en/ce-executor-serial.yml:1148-1157`;memory `task-resume-target-hat-dead-path.md` |
| **D8** | recovery `task.resume` 重发到 `shipper`/`validator` 被 `semantic_gate_violation` 拒(它们 publishes 只允许 `REVIEW_COMPLETE`/`test.*`) | `event_loop/mod.rs:6450-6489` + `recovery.jsonl:23-46` |
| **D9** | `progress-steward` hat 自身 `publishes` 空白,收到 `loop.stalled` 也不知该发什么 | `presets/en/ce-executor-serial.yml` 缺 `progress-steward.publishes:` 段 |

### 4.3 P2 偏离(产物瑕疵)

| # | 偏离 | 证据 |
|---|------|------|
| **D10** | `work.ready` task_id = task_key 字符串(应为 `task-{ts}-{rand}` 格式) | `events.jsonl:11` task_id=`ce-executor:2026-06-20-001-feat-python-sort-algorithms:step-04:u1-readme-integration` 触发 `agent/scratchpad.md:4` contract rejection |
| **D11** | step-04 `work.done` 连续 2 次(第一次 task_id 错误被拒,第二次补正) | `events.jsonl:12,13` |
| **D12** | step-01~03 任务列表显示 open 但实际已完成(projector 重复创建) | `agent/tasks.jsonl:2,3,5` |
| **D13** | dimension-reviewer scope_violation 风险(尝试修改 plan 文件) | `agent/scratchpad.md:19-31` |

---

## 5. 问题归因表(P0 / P1 / P2)

| 优先级 | 问题描述 | 根因分类 | 代码位置 | 证据 | 历史关联 |
|--------|---------|---------|---------|------|---------|
| **P0** | `review.dimensions.complete` 被 FlowStepScope 拒 | preset / loop | `flow_step_scope_stage.rs:135-158` + `presets/en/ce-executor-serial.yml:355-365` | DEFENSIVE_BYPASS line 55 已列该 bypass,但 `current_step.id = "unit_loop"` 时其 `allowed_emits` 不含该 topic;`advance_to_review_walk` (event_loop/mod.rs:9798-9850) 未在 unit_loop 全部 done 后推进 | **A1 / E1**(30 天 12+ 次同型) |
| **P0** | LOOP_COMPLETE 被 `missing required event: report.done` 拒 | preset | `event_loop/mod.rs:1804-1844` + `presets/en/ce-executor-serial.yml:180` | `required_events: ["report.done"]` 在 `reporter.triggers=["REVIEW_COMPLETE"]` 链断裂时永远满足不了 | **C1 / G2**(本次字面同型) |
| **P0** | `plan.blocked` 触发 `terminal_monotonicity_violation` | loop | `event_policy.rs:1227-1241` | `config.terminal_topics` 未含 `plan.blocked`,但 `business_topics` 含;两次发同一终结态 → monotonicity 阻;核心 bug:**`plan.blocked` 未列入 terminal_topics 与 `terminal_when` 列表对齐** | **B1 / B2**(30 天 4+ 次同型) |
| **P0** | coordinator 代发 `loop.stalled` / `human.guidance` 越权 | preset | `presets/en/ce-executor-serial.yml:651` + `presets/schemas/ce-executor-serial.yml` coordinator `allowed_publishes` | coordinator publishes 只含 `work.ready / review.start / plan.complete / plan.blocked / LOOP_COMPLETE`,**没有** `loop.stalled`;`loop.stalled` 是 progress-steward 专属 topic(event_loop/mod.rs:6486-6488 由 ralph 注入) | **D1**(本次字面同型,30 天 9+ 次) |
| **P1** | recovery 循环触发同一失败(re-emit storm) | loop / preset | `event_loop/mod.rs:6450-6489` + `event_loop/mod.rs:2091-2124` | `task.resume` 重发到 `shipper`/`validator` 被 semantic_gate 拒;`bump_consumer_stall_count ≥ 2` 后只发 `loop.stalled`,但 progress-steward 不接盘 → `inject_completion_correction` 熔断只覆盖 LOOP_COMPLETE 路径 | **F1 / F2 / F3**(30 天 6+ 次同型) |
| **P1** | `review-coordinator` 唤醒 `review-synthesizer` 链路缺失 | preset | `presets/en/ce-executor-serial.yml:1148-1157` | review-coordinator `triggers` 不含 `task.resume`;`task.resume target_hat=review-coordinator` 是死路径(memory `task-resume-target-hat-dead-path.md`) | **A2**(30 天 11+ 次) |
| **P1** | `progress-steward` 自闭合缺失 | preset | `presets/en/ce-executor-serial.yml` 缺 `progress-steward.publishes:` | hat `publishes` 空白,收到 `loop.stalled` 也不知该 emit 什么 | **F1**(30 天 6+ 次) |
| **P2** | work.ready task_id = task_key 字符串 | agent / task | `events.jsonl:11` + `state_projector/task.rs:86-88` | task_id 应是 `task-{ts}-{rand}` 格式,但被 task_key 字符串污染;后续 `with_loop_id` 兜底未生效 | **C2**(30 天 6+ 次) |
| **P2** | step-04 work.done 双重 emit | agent | `events.jsonl:12,13` | 第一次 task_id 错误被拒,第二次补正;`dedup_key` 未生效 | — |
| **P2** | step-01~03 任务列表状态错位 | task | `agent/tasks.jsonl:2,3,5` | projector 重复创建任务(无 loop_id 旧任务遗留) | — |

---

## 6. 修复建议

### 6.1 紧急止血(SSOT 改动,马上能用)

#### A. 让 coordinator 在 isolated mode 下能 emit `loop.stalled` 兜底 — **P0,D5 直接止血**
- 文件:`presets/en/ce-executor-serial.yml:651` 区域 coordinator hat `publishes` / `allowed_publishes` 段
- 修改:加 `loop.stallowed` 到 coordinator allowed publishes
- 同步:`crates/ralph-core/src/preset_lint/` 的 `topic_deny_rules` 生成规则放行该组合
- 验证:`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
- 效果:11:02:29 的 `semantic_gate_violation` 不再发生;但救场链路不通(review-coordinator 唤醒链未修)前仍卡死

#### B. LOOP_COMPLETE 缺 `report.done` 临时放行 — **P0,D3 临时止血**
- 文件:`presets/en/ce-executor-serial.yml:180`
- 修改:`required_events: ["report.done"]` → 临时改为 `[]`
- 代价:reporter 不发 `report.done` 就直接 LOOP_COMPLETE,绕过 review pipeline 完整性检查
- 适用:临时跑通验证下游;**不能作长期方案**

#### C. 给 coordinator hat 的 `allowed_publishes` 加 `loop.stalled` — **P0,D5 同步修**
- 文件:`presets/schemas/ce-executor-serial.yml` + `presets/en/ce-executor-serial.yml` coordinator `publishes`
- 验证:`cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`(SSOT byte-equality)

### 6.2 机制修复(改 ralph-core,需跑测试)

#### D. `plan.blocked` 应进 `terminal_topics` — **P0 根因修复,D4 真止血**
- 文件:`crates/ralph-core/src/config/loop_config.rs` `terminal_topics` 字段,加 `"plan.blocked"`(与 `LOOP_COMPLETE` 并列)
- 同步:`presets/en/ce-executor-serial.yml` `flow.steps[].allowed_emits` 把 `plan.blocked` 标为 `terminal_when` 终结条件
- 同步:`crates/ralph-core/src/event_policy.rs:1227-1241` 的 monotonicity 判定对齐(只对 `terminal_topics` 内 topic 触发)
- 验证:`cargo nextest run -p ralph-core -- terminal_monotonicity` + BDD scenarios
- 预期:第二次 `plan.blocked` 直接 `DuplicateTerminalEvent` 走 fail-closed,不再触发 semantic_gate_violation 反复重试

#### E. recovery 循环 re-emit storm 防护 — **P1,D8**
- 文件:`crates/ralph-core/src/event_loop/mod.rs:6450-6489` + `event_loop/mod.rs:2091-2124` `handle_completion_rejection`
- 修改:给 LOOP_COMPLETE 拒收后的 `inject_completion_correction` 加"已 3 次未前进则不再 task.resume,直接 `plan.blocked(reason=loop_stalled_max_iterations)`"硬熔断
- 验证:`cargo nextest run -p ralph-core -- completion_rejection` + `cargo nextest run -p ralph-cli --bin ralph -- recovery_exhaustion`

#### F. `flow_unknown_emit` 真正根因(U1b advance) — **P0,D1**
- 文件:`crates/ralph-core/src/event_loop/mod.rs:9798-9850` `advance_to_review_walk` + `presets/en/ce-executor-serial.yml:130-141` `flow.steps`
- 修改:`unit_loop` 全部 done 后强制推进 `current_step.id = "review_walk"`,而不是依赖 DEFENSIVE_BYPASS 兜底;同时 `flow.steps[review_walk].allowed_emits` 显式声明 `review.dimensions.complete / review.complete`
- 验证:`cargo nextest run -p ralph-core -- flow_step_scope` + BDD scenarios

### 6.3 SSOT 修复(改 preset,需重建 + 校验)

#### G. preset lint `allowed_publishes` 同步 — **P0,D5 / D9 同步**
- 文件:`presets/schemas/ce-executor-serial.yml` + `crates/ralph-core/src/preset_lint/`
- 修改:`coordinator` hat `allowed_publishes` 加 `loop.stalled`(与 D5 同步);`review-coordinator` `allowed_publishes` 加 `review.dimensions.complete`(与 line 401 解除 deny 对称);`progress-steward` `publishes` 加 `task.resume` / `plan.blocked` / `loop.cancel` 兜底(D9 自闭合)
- 验证:`cargo nextest run -p ralph-cli --bin ralph -- preset_lint` + `cargo nextest run -p ralph-core -- preset_lint` + `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`

#### H. work.done `task_id` 校验收紧(防 task_key 污染) — **P2,D10**
- 文件:`crates/ralph-core/src/state_projector/task.rs:86-88` + execution_contract 的 `require_task.id_field` 校验
- 修改:`task_id` 必须匹配 `^task-\d+-[a-f0-9]+$` 格式,否则 reject

---

## 7. 关键判断题 — 直接回答

> "是编排机制有问题?还是修复机制失效?还是说 ralph 自身的 bug?"

**三个都是,且层层叠加。**

1. **编排机制(preset / hat topology)— 主因**
   - `flow.steps` 声明过窄:只声明 `unit_loop → plan_end → ship` 三步,中间 `review_walk` 是缺位的;`current_step` 停留在 `unit_loop` 时,review-coordinator 任何一个 review 事件都得靠 DEFENSIVE_BYPASS 兜底
   - `coordinator.publishes` 不含 `loop.stalled`,导致救场路径在 isolated mode 下必拒
   - `progress-steward.publishes` 空白,收到 `loop.stalled` 也不知该发什么
   - `report.done` 链断裂:`reporter.triggers=["REVIEW_COMPLETE"]`,但 shipper 把 `plan.complete → REVIEW_COMPLETE` 升级逻辑没在 preset 里显式声明可达性

2. **修复机制(recovery / repair_stream / progress-steward)— 部分失效**
   - `task.resume` 重发机制**工作是工作的**(看 recovery.jsonl 16+ 次重试),但重发目标判断错——反复发到 `validator`/`shipper`,而它们的 publishes 严格不允许发原事件,所以每次重发都进 `semantic_gate_violation` 的"可恢复桶",3 次后 progress-steward 接管,但 progress-steward hat 自身 `publishes` 没定义,**收到了也不知道该发什么**
   - `inject_completion_correction` 熔断只覆盖 LOOP_COMPLETE 这一条路径,plan.blocked 连续拒收没有对应熔断器,recovery 会无限重试

3. **Ralph 基座(事件循环 / state machine / semantic_gate)— 有一个真 bug**
   - **`plan.blocked` 没进 `terminal_topics`**(`event_policy.rs:1228` 过滤条件 `config.terminal_topics.contains(topic)`)
   - 后果:发过一次 `plan.blocked` 之后,state 的 `terminal_observed` 没被设;但代码里又有个 implicit 终态判定让它直接 `RejectWithResume`,两个独立终态机制(`terminal_topics` 与 `plan.*` 的 `terminal_when`)互相不识别
   - 这是 monotonicity 检查的语义裂缝,本次 symptom 4 即由此派生

**三者因果链(按时间顺序)**:
1. **preset 拓扑缺陷**(`flow.steps` 缺 `review_walk`、`reporter` 触发链未声明可达)→ 编排阶段就让 review-coordinator 进 `unit_loop` 卡死 → **symptom 1, 2**
2. **preset `coordinator.publishes` 缺 `loop.stalled`** + **基座 `plan.blocked` 未列入 `terminal_topics`** → 编排失败后,coordinator 想救场时再次被拒 → **symptom 4, 5**
3. **基座缺 plan.blocked 熔断** + **recovery `task.resume` 重试到错误 hat** → 救场失败后,recovery 循环反复触发同一种拒绝 → **symptom 6**
4. **progress-steward 自身 `publishes` 空白** → 救场路径最终 `loop.stalled` 也被接受后,没人接盘 → **symptom 5 的延续**

**最便宜的一刀切修复路径(优先级 C + D)**:
- 修 `presets/schemas/ce-executor-serial.yml` 的 `coordinator.allowed_publishes` 加 `loop.stalled` + `plan.blocked` 进 `loop_config.terminal_topics`
- **马上**让编排救场路径通
- 同步补 F(`unit_loop.advance_to_review_walk`)让 review 链路在 unit_loop 步就合法
- **不解 F,后续 10 个恢复轮还会以不同形式卡死**

---

## 附录:本次产物状态快照

- `sorts/` 实现完整:`quick_sort.py` + `_base.py` + `_compare.py` + `__init__.py` + README + 69 个测试全 pass
- `progress.md`:Completed Steps = step-01~04,Current Step = (none)
- `tasks.jsonl`:8 条任务,4 个 closed + 4 个重复 open(无 loop_id)
- `loops.json`:`primary-20260629-100106`,pid=1327468,workspace=`/home/chaowen/Dev/agent_tools/ralph-e2e`
- 进程已停(loop 已被 ralph 自身 shutdown);产物未损坏,可继续推进
---

## 修复记录(2026-06-29)

### 根因复核

对当前 `HEAD` 代码进行复核后发现,报告中列举的 preset 层 symptoms 大多已经自愈:

| 原 symptom | 当前代码状态 | 结论 |
|---|---|---|
| D1 `review.dimensions.complete` 被 `FlowStepScope` 拒 | `DEFENSIVE_BYPASS` 已包含 `(review-coordinator, review.dimensions.complete)`;`drive_step_transition` 会在 unit_loop `total_units` 完成后推进到 `review_walk` | 已自愈,非本次复发根因 |
| D5 coordinator 越权发 `loop.stalled` / `human.guidance` | `loop.stalled` 改由运行时 `ralph` 发出(`event_loop/mod.rs:6486-6488`),coordinator 不再越权 | 已自愈 |
| D9 `progress-steward.publishes` 空白 | preset 中 `progress-steward.publishes` 已声明 `[work.ready, review.start, task.resume, plan.blocked]` | 已自愈 |

真正导致本次复发且当前仍会触发的是 **D4/D6/D8 共享的根因**:事件循环在 `LOOP_COMPLETE` 尚未通过 `required_events` / verdict gate 校验之前,就提前将 `policy_runtime_state.terminal_observed` 置为 `true`。这导致 `LOOP_COMPLETE` 因缺 `report.done` 被拒后,后续 recovery 路径发出的 `plan.blocked` / `task.resume` 被 `terminal_monotonicity_violation` 拒绝,从而进入 re-emit storm。

### 修复改动

**`crates/ralph-core/src/event_loop/mod.rs`**
- `process_parse_result` 中更新 `terminal_observed` 时,排除 `completion_promise`(即 `LOOP_COMPLETE`)。
- `check_completion_event` 真正准备返回 `TerminationReason::CompletionPromise` 时,再同步置位 `terminal_observed`,与 `completion_honored` 一致。

**`crates/ralph-core/src/event_loop/tests/chain_validation.rs`**
- 新增回归测试 `test_rejected_loop_complete_does_not_poison_terminal_state`:
  - 配置 `required_events: ["report.done"]`
  - 先让 `LOOP_COMPLETE` 因缺 `report.done` 被拒
  - 断言 `terminal_observed` 仍为 false
  - 再发 `plan.blocked`,断言被接受、不触发 `terminal_monotonicity_violation`

### 未采纳 report 原建议的原因

- **未将 `plan.blocked` 加入 `event_policy.terminal_topics`**:正常失败链路是 `plan.blocked → REVIEW_COMPLETE → report.done → LOOP_COMPLETE`;若 `plan.blocked` 为 terminal,`REVIEW_COMPLETE` 会被 `terminal_observed` 后的 monotonicity 拒绝,破坏正常失败路径。
- **未给 coordinator 加 `loop.stalled`**:运行时自己以 `ralph` 身份发出,无需 coordinator 越权。
- **未临时清空 `required_events: ["report.done"]`**:这是绕过完整性检查,不是修复。

### 验证结果

```bash
cargo nextest run -p ralph-core -- chain_validation          # 14 passed
cargo nextest run -p ralph-core --test scenarios             # 69 passed
cargo nextest run -p ralph-cli --bin ralph -- preset_lint    # 11 passed
cargo nextest run -p ralph-core -- preset_lint               # 120 passed
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded  # 1 passed
./scripts/run-tests.sh                                       # ✅ nextest + doctest 全过
```

### 修复提交

- `fix(ralph-core): LOOP_COMPLETE 被拒时不再污染 terminal_observed`
- 文件:`crates/ralph-core/src/event_loop/mod.rs`、`crates/ralph-core/src/event_loop/tests/chain_validation.rs`
- 测试:新增 `test_rejected_loop_complete_does_not_poison_terminal_state`
