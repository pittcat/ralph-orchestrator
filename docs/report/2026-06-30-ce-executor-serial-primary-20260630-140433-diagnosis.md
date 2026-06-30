# RALPH 链路诊断报告 — primary-20260630-140433 (v3 · 终态证据)

> **run**: `primary-20260630-140433`
> **preset**: `ce-executor-serial`（isolated mode，10-hat）
> **plan**: `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`（2 plan-unit + 2 fix-unit）
> **run_dir**: `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/`
> **loop.lock PID**: `62564`（**终态事件已发出但进程未退出**）
> **诊断日期**: 2026-06-30
> **报告版本**: v3（在 v2 基础上整合新一轮证据：修复链路实际闭环成功）

---

## 第 0 部分：结论摘要

**整体健康度**: 🟡 **业务侧已闭环，但进程未退出** —— 4 个 unit/fix-unit 全部跑完、shipper 评审 pass、report.md 已落地（git commit `c745636`，2026-06-30）、`LOOP_COMPLETE` 已由 reporter 在 **15:01:29Z** 发出。但 `loop.lock PID 62564` 至今仍占、`loops.json` 无 `ended` 字段，progress-steward 在 15:02:39 / 15:03:03 / 15:11:12 持续注入 `task.resume`。

### v3 与 v2 的根本性差异

| 维度 | v2 报告（错误） | v3 报告（实测） |
|---|---|---|
| 业务是否闭环 | ❌ 终态 4 个事件全缺失 | ✅ 终态 4 个事件 **已发出**（L35-L38 events.jsonl）|
| P0-1 现象描述 | plan.complete 落 repair_sink 没进 bus，loop 卡死 | shipper/REVIEW_COMPLETE 是通过 fallback 路径产出（fix-units 全 closed 后 shipper 自动 emit），**和 v2 的归因错误** |
| 真正阻塞原因 | （未识别）| **LOOP_COMPLETE 发出后进程不退出**（progress-steward 持续 stall_recovery 循环）|
| report.md 是否生成 | ❌ 无 | ✅ 4751 bytes，git commit `c745636` |
| 处置优先级 | 把 P0-1 作为机制 + 编排 + agent 三件套致命故障去修 | 把"LOOP_COMPLETE 后不退出"作为 P0 修；把 v2 提到的 5 个 P0 整体降级 |

**v2 报告归因错误的核心原因**：我把 `recovery.jsonl:2-28` 的 27 条 `plan.complete` 解读为"plan.complete 走 `AcceptRepairStream` 没进 bus，shipper 0 事件"，这是错的——实测源码 `event_loop/mod.rs:10677-10683` 与 `emit_gate.rs:114-127`，plan.complete 不在 REPAIR_TOPICS 里，**不**会走 `AcceptRepairStream`。recovery.jsonl 的 reason_code=`repair_dispatch` 是 `record_repair_event`（line 10885-10894）的 sink reason_code，仅当事件被识别为 repair topic 时才会落到这里——本 run 中被识别为 repair topic 的很可能是 `task.relocate_legacy` 类的中间事件（preset 的 `topic_format_whitelist` 提到了这个 topic），不是 plan.complete 本身。

**真相**：本 run 业务成功，机制 fallback（shipper 自我激活）正确运转；问题只在 **进程级收尾**（LOOP_COMPLETE 后不退出）。

---

## 第 1 部分：实测事件流（38 行 events.jsonl）

| 序 | topic | hat / source | ts (UTC+8) | 关键 payload | 状态 |
|---|---|---|---|---|---|
| 1 | `work.start` | loop-bootstrap | 22:04:33 | prompt 指向 plan.md | ✅ |
| 2 | `work.ready(step-01)` | coordinator | 22:05:48 | u1-skeleton | ✅ |
| 3 | `work.done(step-01)` | executor | 22:08:26 | commit=1, changed=266, tests_passed=6 | ✅ |
| 4 | `test.passed(step-01)` | validator | 22:09:22 | PHASE 1 → step-02 | ✅ |
| 5 | `work.ready(step-02)` | coordinator | 22:10:25 | u2-enhancement | ✅ |
| 6 | `work.done(step-02)` | executor | 22:12:19 | commit=1, changed=294 | ✅ |
| 7 | `test.passed(step-02)` | validator | 22:13:04 | Branch B → review.start | ✅ |
| 8 | `review.start` | coordinator | 22:13:44 | unit_index=2, total_units=2 | ✅ |
| 9-24 | 6 维 review walk | review-coord + dim-reviewer | 22:14 - 22:31 | 含 4 次 testing 重复 + 1 次 testing2 漂移 + 3 次 dimensions.complete 重复 | ⚠️ **偏离 (但被 isolated mode 丢弃，synthesizer 收到 1 次，事件 #28 完整)** |
| 25-27 | `review.dimensions.complete` × 3 | review-coord | 22:32 (字节级一致)| 同 payload 重复 | ⚠️ 偏离 |
| 28 | `review.complete` | review-synthesizer | 22:35:55 | verdict=pass_with_residuals, fix_plan=fix-plan.md, findings=2 | ✅ |
| 29 | `work.ready(fix-01)` | coordinator | 22:36:51 | u1-fix-init-exports | ✅ |
| 30 | `work.done(fix-01)` | executor | 22:38:15 | commit=1, changed=6 | ✅ |
| 31 | `test.passed(fix-01)` | validator | 22:39:26 | tests_passed=15 | ✅ |
| 32 | `work.ready(fix-02)` | coordinator | 22:40:49 | u2-add-worst-case-tests, **plan_path 回退原 plan** | ⚠️ 偏离 (P1) |
| 33 | `work.done(fix-02)` | executor | 22:43:13 | commit=1, changed=75 | ✅ |
| 34 | `test.passed(fix-02)` | validator | 22:44:41 | tests_passed=18 | ✅ |
| **35** | **`REVIEW_COMPLETE`** | **shipper** | **23:00:19** | **verdict=pass, final_findings=5** | **✅ 闭环信号** |
| **36** | **`report.done`** | **reporter** | **23:01:26** | **report_path=docs/report/...report.md** | **✅ 闭环信号** |
| **37** | **`LOOP_COMPLETE`** | **reporter** | **23:01:29** | **reason=All planned steps completed** | **✅ 闭环信号** |
| 38 | `task.resume` | progress-steward | 23:03:03 | reason=fix_unit_complete_plan_complete_pending, target=coordinator | ⚠️ **不预期：LOOP_COMPLETE 后又注入** |

**核心事件时间线**（北京时间）：

```
14:04:33  loop 启动
14:04:33 - 14:44:41  业务侧执行（4 个 unit/fix-unit 全完成）
14:44:41  fix-02 test.passed
14:44:41 - 15:00:19  ⚠️ 15 分钟静默（v2 误诊的"卡死"窗口）
                       期间 progress-steward stall_recovery 在循环
15:00:19  ✅ shipper 自己 emit REVIEW_COMPLETE(pass)
           机制：fix-units 全部 closed，shipper 不依赖 plan.complete 进入 bus
15:01:26  ✅ reporter emit report.done（report.md 4751 bytes 已落地 commit c745636）
15:01:29  ✅ reporter emit LOOP_COMPLETE
15:02:39  ⚠️ progress-steward 注入 stall_recovery task.resume(target=ralph)
15:03:03  ⚠️ progress-steward 注入 fix_unit_complete_plan_complete_pending task.resume(target=coordinator)
15:03:08  recovery envelope：outcome=Recovered
15:11:12  recovery envelope：outcome 更新（继续）
...     ⚠️ 进程未退出，loop.lock 永占
```

```mermaid
flowchart LR
    A(["loop 启动 14:04:33"]) --> B["业务执行 14:04:33 - 14:44:41"]
    B --> C["fix-02 test.passed 14:44:41"]
    C --> D["静默 14:44:41 - 15:00:19 stall_recovery 循环"]
    D --> E["shipper REVIEW_COMPLETE pass 15:00:19"]
    E --> F["reporter report.done 15:01:26"]
    F --> G["reporter LOOP_COMPLETE 15:01:29"]
    G --> H["progress-steward task.resume 15:03:03"]
    H --> I["recovery envelope Recovered 15:03:08"]
    I --> J["recovery envelope 更新 15:11:12"]
    J --> K(["loop 进程未退出 lock PID 62564 仍占"])
```

---

## 第 2 部分：v2 报告归因错误的追溯

### 2.1 v2 的错误推断

v2 报告假设：`recovery.jsonl:2-28` 的 27 条 `plan.complete` 是 cooridinator 实时 emit 的 `plan.complete` 落入 `repair_sink`，导致 shipper 看不到 `plan.complete` 而激活失败、loop 卡死。

### 2.2 实测否定

| v2 的 3 个支撑点 | 实测发现 |
|---|---|
| 假设: events.jsonl 终止于 L34 fix-02 test.passed | ✅ v2 时确实如此（34 行），但**最新一次运行后扩到 38 行**：L35-L37 加入了 shipper/REVIEW_COMPLETE + reporter/report.done + reporter/LOOP_COMPLETE |
| 假设: plan.complete 走 `AcceptRepairStream` 落 repair_sink | ❌ **不对**。`is_repair_topic`（`repair_dispatch_stage.rs:39-46` 的 `REPAIR_TOPICS` 列表）只有 4 项：`task.relocate / task.relocate_legacy / repair.budget.exhausted / repair.close`。`plan.complete` 不在内，所以根本不会走 `AcceptRepairStream`，进入的是 `AcceptMainBus` 分支（`emit_gate.rs:127`） |
| 假设: shipper 0 事件 → 没收到 plan.complete | ❌ **实测否定**：shipper hat-events 文件仍是 0 字节，但 **`REVIEW_COMPLETE` 进入了 events.jsonl**——所以 shipper 走的是 `EventLoop.publish_event`（内存调用 bus），不依赖 hat-events 子流 |

### 2.3 真正的因果链（修正）

```
fix-02 test.passed 抵达（14:44:41）
  ↓
[15 分钟静默 window，期间 stall_recovery 在跑]
  ↓
shipper 内部判定：fix-units 全部 closed + tasks.jsonl 全部 closed
                  → shipper 直接 publish REVIEW_COMPLETE 到 bus（绕过 plan.complete 中间事件）
  ↓
reporter 看到 REVIEW_COMPLETE → emit report.done + LOOP_COMPLETE
  ↓
LOOP_COMPLETE 之后，runtime 应退出：
  - loop.lock 应释放 → 实际未释放（PID 62564 至今占）
  - loops.json 应写入 ended → 实际未写
  - 进程应退出 → 实际未退出（占 CPU 2.27 秒 cpu time 至今）
  ↓
progress-steward 不知道 LOOP_COMPLETE 已发，继续按 max_steward_iterations 节奏注入 task.resume
  ↓
进程 idle 在 progress-steward 唤醒节奏，永远在等下一次 task.resume
```

**真正根因**：**LOOP_COMPLETE 发出后没有触发 process termination 信号**。可能是：
1. `state.completion_honored = true` 已设置（`mod.rs:2159`），但 termination 检测未 trip
2. `check_termination`（`mod.rs:1667`）未在 `REVIEW_COMPLETE` / `LOOP_COMPLETE` 进入 bus 后的下一 dispatch tick 中调用
3. loop main loop 在等下一次 `process_events_from_jsonl` 唤醒，而 progress-steward 的 task.resume 把这当成新事件让 process 继续

这与 v1 历史报告 `primary-20260630-032648` 的"卡死 pattern F（P-M8 completion_requested 缺 guard）"同源。

---

## 第 3 部分：真实 P0 列表（v3 重写）

| ID | 描述 | 根因分类 | 严重度 | 处置 |
|---|---|---|---|---|
| **P0-A**（升级自 P0-5）| LOOP_COMPLETE 已发出（15:01:29Z），但 loop 进程未退出（lock 永占、loops.json 无 ended），progress-steward 在 15:02/15:03/15:11 持续注入 task.resume | **机制**：`check_termination` 或 `completion_honored` 翻转后未触发 process 退出 | **进程级阻塞** | 排查 `mod.rs:1667 check_termination` / `2159 completion_honored = true` 后的 process exit 信号链路 |
| P0-B（降级自 v2 P0-1）| 静默 15 分钟窗口（14:44:41 → 15:00:19）：fix-02 test.passed 后 shipper/reporter 没有立即被驱动，等 15 分钟后由 shipper 兜底 | **编排**：preset 没有声明 fix-unit chain close → shipper direct trigger | **业务可用但慢** | 把 shipper 直接订阅 `test.passed(step=fix-NN)` 作为兜底 trigger（当前 shipper 只订阅 `plan.complete / plan.blocked`）|
| P0-C（降级自 v2 P0-3/P0-4）| review-coordinator 4 次重发 `review.dimension.ready` + 3 次重发 `review.dimensions.complete` | **机制 + agent**（混合） | **浪费 ~6 次 iteration** | 增加 extra_business_event_dropped 的 task.resume 拒绝反馈环 |
| P0-D（首例 - 历史同源 `032648` DE-005）| `recovery.jsonl` 出现 1 条 `task_key` 截断在 `step-` 的 `work.ready`（coordinator 派生 task_key 失败）| **agent** | **影响 1 次 iteration** | coordinator prompt 用 rand hex |
| P0-E（降级自 v2 P0-2）| dimension-reviewer 6 次改 plan frontmatter（Write 未禁，circuit breaker 阈值过高未 trip）| **机制 + agent** | **影响产物可观察性** | disallowed_tools 加 Write + threshold=3 |

**注意**：**没有 P0-1（P0-1 误诊已撤销）**。原 v2 假设的"plan.complete 落 repair_sink 没进 bus"被实测否定：plan.complete 不在 REPAIR_TOPICS，不会走 AcceptRepairStream。本 run 业务正常运行只有慢 + 进程级退出缺陷。

---

## 第 4 部分：P1 / P2 偏离（仍有效）

| ID | 描述 | 处置 |
|---|---|---|
| P1-A | fix-02 `work.ready.plan_path` / `work.done.plan_path` 回退到原 plan 路径（违反 preset L1208）| agent 行为约束 → skill doc 修复 |
| P1-B | `review.complete.findings_count=2` vs `residual_findings_count=0` + summary "3 P2" 自洽性 | 机制（synthesizer 字段语义模糊）|
| P1-C | `ralph.yml max_iterations: 500` 缺注释 | 编排 |
| P2-A | recovery.jsonl 行数(31) << log WARN 数 | 可观测性：WARN 升级为 envelope |
| P2-B | `.ralph/agent/progress.md` 与 `.agents/scratchpad/...progress.md` 双源 drift | 同源已修，本次重发 |
| P2-C | `triggered=ralph` 字段语义稀释（多次同 topic 都标 ralph） | 标注即可 |

---

## 第 5 部分：修复优先级（基于 v3 真实归因）

| # | 目标 | 真实根因 | 优先级 |
|---|---|---|---|
| 1 | P0-A：process exit after LOOP_COMPLETE | `event_loop/mod.rs:1667 check_termination` + `2159 completion_honored=true` 后未触发 process exit | **P0 必加** |
| 2 | P0-B：shipper 兜底直接订阅 fix-unit test.passed | preset `mechanism.flow.plan_end` 没有声明 shipper 兜底，shipper.triggers=[plan.complete, plan.blocked] 不含 test.passed | **P0 必加**（让 shipper 立刻被驱动）|
| 3 | P0-C：extra_business_event_dropped 反馈环 | mod.rs 7943 附近（推测）| **P0 必加** |
| 4 | P0-D：coordinator task_key hex 强制随机生成 | state_projector/task.rs | **P0 必加** |
| 5 | P0-E：dimension-reviewer disallowed_tools + Write | preset L1859 | **P0 必加** |
| 6 | P1-A：fix-unit plan_path 沿用 fix-plan | skill doc 强化 | **P1** |
| 7 | P1-B：synthesizer 字段语义 | 机制 | **P1** |
| 8 | P2-A：recovery.jsonl 与 log 同步 | 机制 WARN 升级 | **P2** |
| 9 | P2-B：progress.md 双源 SSOT 校验 | scripts/check-progress-drift.sh | **P2** |
| 10 | P2-C：triggered=ralph 语义 | 标注 | **P2** |

---

## 第 6 部分：用户原始 4 问的更新答案

### Q1. 整体执行过程有没有问题？

**业务侧正常，进程侧阻塞**。fix-02 test.passed 后等 15 分钟 shipper 兜底并产 REVIEW_COMPLETE（pass），reporter 产 report.done + LOOP_COMPLETE 闭环。但进程没退出，progress-steward 持续注入 task.resume。

### Q2. RALPH 基座机制是否正常生效？

**大部分正常**：4 个 unit 全完成、6-dim review、shipper verdict-promotion、reporter report —— 都按 preset 正常运转。**机制问题集中在进程退出**：`check_termination` 后未真正退出 process。

### Q3. 编排是否合理？

**业务编排合理**：preset `ce-executor-serial` 适配 4-unit 计划，所有 hat routing、execution_contracts、PHASE gate 工作正常。**编排缺口**：shipper.triggers 不含 `test.passed(step=fix-NN)` 兜底，导致 shipper 不会立刻被驱动，要等 stall_recovery 15 分钟后才生效。

### Q4. 真问题是机制 vs 编排 vs agent？

**机制 60% + 编排 30% + agent 10%** —— 进程不退出的根因是 `check_termination → process exit` 链路；慢的根因是 shipper.triggers 缺兜底；这都是机制 / 编排问题。**v2 报告把根因归到"plan.complete 落 repair_sink 没进 bus"是错的**，已被实测否定（plan.complete 不在 REPAIR_TOPICS，不会走 AcceptRepairStream）。

---

## 附录 A：源码核实交叉索引（v3）

| 文件 | 行号 | 实测内容 | 与 v3 归因对应 |
|---|---|---|---|
| `presets/en/ce-executor-serial.yml` | shipper hat | triggers=`['plan.complete', 'plan.blocked']`, publishes=`['REVIEW_COMPLETE']` | P0-B：缺 test.passed 兜底 trigger |
| `crates/ralph-core/src/event_loop/stages/repair_dispatch_stage.rs` | L39-46 | `REPAIR_TOPICS = ['task.relocate', 'task.relocate_legacy', 'repair.budget.exhausted', 'repair.close']` | v2 错纠正：plan.complete 不会走 AcceptRepairStream |
| `crates/ralph-core/src/event_loop/emit_gate.rs` | L114-127 | `evaluate_emit_gate` 三路：Reject / AcceptRepairStream / AcceptMainBus；plan.complete 不是 repair topic → AcceptMainBus | v2 错纠正：plan.complete 进 bus |
| `crates/ralph-core/src/event_loop/mod.rs` | L2159 | `state.completion_honored = true`（LOOP_COMPLETE 抵达时） | P0-A：翻转后 process exit 链路 |
| `crates/ralph-core/src/event_loop/mod.rs` | L1667 | `check_termination` | P0-A：未触发进程退出 |
| `crates/ralph-core/src/event_loop/recovery_finalizer.rs` | （推测）| LOOP_COMPLETE 后的 finalizer 链 | P0-A：未触发 |
| `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/events-20260630-140433.jsonl` | L35-37 | **REVIEW_COMPLETE 15:00:19 + report.done 15:01:26 + LOOP_COMPLETE 15:01:29** | v3 主证据：业务已闭环 |
| 同上 | L38 | `task.resume` @ 15:03:03 by progress-steward | P0-A 实证：LOOP_COMPLETE 后又注入 |
| `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/diagnostics/2026-06-30T22-04-32/recovery.jsonl` | L1-3 | `stall_no_events` warning @ 15:02:39 + 2 次 `recovery_outcome_update` | P0-A 实证：loop 持续被 stall_recovery 唤醒 |
| `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/loops.json` | 全文 | 无 ended 字段 | P0-A 实证：未写 ended |
| `/Users/pittcat/Dev/Rust/ralph-e2e/docs/report/2026-06-30-ce-executor-2026-06-20-001-feat-python-sort-algorithms-plan-report.md` | 全文 4751 bytes | reporter 产出；git commit c745636 | 闭环证据：业务报告落地 |

---

## 附录 B：v3 相对 v1/v2 的关键改正

| 项 | v1 / v2 | v3（实测） |
|---|---|---|
| `REVIEW_COMPLETE / report.done / LOOP_COMPLETE` 是否发出 | 否（"4 个终态事件缺失"）| ✅ 是（L35-L37 events.jsonl）|
| 是否真的循环卡死 | 是（"loop.lock 永占"）| 不准确：业务正常，进程 idle |
| P0-1 是不是 plan.complete 没进 bus 导致 shipper 0 事件 | 是（归因）| 否（实证 plan.complete 进 bus，shipper 走兜底路径）|
| 真正的 P0 是 | P0-1 / P0-2 / P0-3 / P0-4 / P0-5 | **P0-A 进程级退出** + **P0-B shipper 兜底 trigger 缺失** + 三个降级 P0（C/D/E）|

---

**报告版本**: v3(2026-06-30)
**整理人**: 主 Agent(实测 events.jsonl 38 行 + 源码核实)
**数据源范围**:
- 运行产物:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/` 全量(events-38行/recovery-31行/ledger-30行/agent/scratchpad/diagnostics)
- 实测产物:`/Users/pittcat/Dev/Rust/ralph-e2e/docs/report/2026-06-30-ce-executor-2026-06-20-001-feat-python-sort-algorithms-plan-report.md`(4751 bytes, shipper/reporter 闭环产出)
- 主仓库源码:`presets/en/ce-executor-serial.yml`、`crates/ralph-core/src/event_loop/{mod.rs,emit_gate.rs,stages/repair_dispatch_stage.rs,recovery_finalizer.rs}`
- 历史报告:`docs/report/2026-06-30-ce-executor-serial-primary-20260630-032648-diagnosis.md` 及其前文 commit
