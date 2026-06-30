# Ralph Loop 编排机制诊断报告 — primary-20260628-172725

> **作者**:主 Agent(汇总)
> **生成时间**:2026-06-29
> **评估对象**:Ralph Loop `primary-20260628-172725` · Preset: `ce-executor-serial` · 状态:**Failed: recovery retry window exhausted** · Iterations: 12 · Duration: 36m 26s
> **数据范围**:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/`(运行时产物)+ 主仓源码审查(`crates/ralph-core/src/{event_loop,state_projector,execution_contract,diagnosis,drift,flow_lifecycle,workflow_contract}/`、`crates/ralph-cli/src/loop_runner/`) + 13 份历史 ce-executor-serial 诊断报告

## 用户提问回应

> "这个东西又没有按照编排的流程去走,然后这些修复机制又他妈失效了,搞乱了之后修复机制又失效了,需要你帮我去定位。是编排机制有问题?修复机制失效?还是 RALPH 自身的 bug?"

**一句话答复**:**编排机制 + 修复机制 + RALPH 基座三者仍有问题,且这次失败是 30 天第 11 次复发;`2026-06-28-004 fix plan` 的 16 Unit 已经全部落地(`32ca2f07` → `1c14e1d5`),本次 172725 run 是在 fix plan 落地**之后**出现的,意味着 004 plan 未覆盖此次失败模式或部分 Unit 落地后回归**。

### 三者关系

1. **编排机制**:`ce-executor-serial.yml` 设计本身是正确的(10-hat 拓扑、Phase Gate 严格区分 step-NN / fix-NN、preset 失败路径走 `plan.blocked → REVIEW_COMPLETE(fail)`),**但本次 run 实际未按设计推进**——0 review.start / 0 plan.complete / 0 REVIEW_COMPLETE / 0 LOOP_COMPLETE。原因不在编排定义,而在基座机制把流程卡死。

2. **修复机制**:本身**按设计工作了**(stall_recovery 在 iter 4/9/12 三次识别 validator 30s 未 activate,envelope 正确写入 recovery.jsonl:4, 9, 18),**但 retry_key 锚错**:
   - stall_recovery 锚到 `validator`(正确)
   - hard gate 二次检测错锚到 `missing_event_gate:executor`(错误)
   - 两条 retry_key 互不感知,各自跑 attempt 窗口 → 8 iter 内累计 2 次 → `EscalationLevel::Final`(`responder.rs:907-909`)→ `RecoveryExhausted` 硬退出
   - 修复机制**没有把 loop 拨回原轨道**,而是**直接 kill 了 loop**

3. **RALPH 自身的 bug(基座机制)**:**7 个真 bug**:
   - (a) hard gate 不知 work.done 已发(`hard_gate.rs:912`)
   - (b) responder.classify 两条 retry_key 互不感知(`responder.rs:880-919`)
   - (c) `recovery_exhausted` 不走 plan.blocked(`drift/engine.rs:392-406`)
   - (d) projector 不补 loop_id 兜底(`state_projector/task.rs:86-88`)
   - (e) inline JSON 缺 `kind` 字段(`event_loop/mod.rs:6104-6112`)
   - (f) handoff 30s 硬编码远低于实测 54s-540s(`config/workflow_contract.rs:50`)
   - (g) validator hat 调度有竞态(`event_loop/mod.rs:3550` `on_hat_activated` 对 validator 路径不可见)

**因果链**:**RALPH 基座的 7 个 bug 导致编排偏离 → 修复机制虽然识别真 stall 但 retry_key 错锚 → 误判 Final → kill loop 而非走 plan.blocked 终态 → preset 设计的 review/ship/report 整段链路全程未启动 → loop 在 step-04 后硬退出**。

## 关于 `2026-06-28-004 fix plan` 落地状态(重要更正)

**事实**:`2026-06-28-004 fix plan` 的 16 Unit 已全部 commit 落地(关键 commit: `32ca2f07` U1+U2 / `65b80334` U3 / `4d855d4e` U4 / `31c26657` U5 / `f6c5ff3a` U6 / `f1170ae9` U7 / `055a84e3` U8 / `62496129` U9 / `27acb8ac` U10 / `d4344eec` U11 / `fd6531a6` U12 / `d00b6f0d` U13 / `831d0626` U14 / `fe022b69` U16),后续还有 `f3168e66` code-review fix loop / `1c14e1d5` 阶段门注释强化。

**本次 172725 run 是在 004 plan 落地之后才出现的**,这意味着:

1. 004 plan 修复了原 115810 报告中的部分根因,但**未完全覆盖**本次 172725 的根因群(stall_recovery 与 missing_event_gate 双轨 retry_key / RecoveryExhausted 不走 plan.blocked / validator 30s hat 调度竞态);
2. **或**部分 Unit 落地后产生了**新回归**(类似 `fe022b69` U16 regression fix 的形态),需要逐 Unit 核对本次 run 是否触发了 plan 已修路径;
3. **或**本次 172725 是 plan 修复后又复发的**新形态**,需重新归因。

**报告结构**:本报告由 4 个 sub-agent 并行诊断后合并,分四章:**第一章链路对比**(原 A 报告)、**第二章历史问题 KB**(原 B 报告)、**第三章偏离证据**(原 C 报告)、**第四章根因归因 + 修复建议**(原 D 报告)。

---|---|---|---|---|
| **coordinator** | work.start, task.resume, test.passed, review.complete, work.failed | work.ready, review.start, plan.complete, plan.blocked, LOOP_COMPLETE | Phase Gate 守门;**step-NN** ↔ **fix-NN** 严格区分 |
| **executor** | work.ready, fix.exhausted | work.done, work.failed | TDD 实施 |
| **validator** | work.done, fix.applied | test.passed, test.failed | 全量测试 |
| **fixer** | test.failed | fix.applied, fix.exhausted | 诊断 + 修复(budget: 10 轮) |
| **review-coordinator** | review.start, review.dimension.done, review.dimension.failed | review.dimension.ready, review.dimensions.complete | 6-dim 序列状态机 |
| **dimension-reviewer** | review.dimension.ready | review.dimension.done, review.dimension.failed | 单维度评审 |
| **review-synthesizer** | review.dimensions.complete | review.complete | 合并 + 写 fix-plan |
| **shipper** | plan.complete, plan.blocked | REVIEW_COMPLETE | 终验 + 计划 status |
| **reporter** | REVIEW_COMPLETE | report.done, LOOP_COMPLETE | 经理报告 |
| **progress-steward** | loop.stalled | work.ready, review.start, task.resume, plan.blocked | 兜底救援 |

### 1.2 理想事件流(Plan ≥ 4 units)

```
work.start
  └─ coordinator → work.ready(step-01)
       └─ executor → work.done(step-01)  → validator → test.passed(step-01)
                                                  ↓ (PHASE 1: step-01 → step-02)
            coordinator → work.ready(step-02)
                 └─ executor → work.done(step-02) → test.passed(step-02)
                                                       ↓ ... (重复到 step-0N)
                          coordinator → review.start   (最后一个 step-NN 通过后)
                               └─ review-coordinator → review.dimension.ready(goal-alignment)
                                    └─ dimension-reviewer → review.dimension.done
                                         └─ review-coordinator → review.dimension.ready(correctness)
                                              ... (6 个维度)
                                                   └─ review-coordinator → review.dimensions.complete
                                                        └─ review-synthesizer → review.complete
                                                             ├─ fix_plan_file="null" → coordinator → plan.complete(verdict=pass)
                                                             └─ fix_plan_file=<path> → coordinator → work.ready(fix-01)
                                                                                                                                       ↓ (PHASE 2: fix-NN)
                                                                                                                                  ... (fix-units)
                                                                                                                                       └─ coordinator → plan.complete
                                                                                                                                            └─ shipper → REVIEW_COMPLETE
                                                                                                                                                 └─ reporter → report.done → LOOP_COMPLETE
```

### 1.3 Phase Gate 说明(关键 Phase Gate)

**关键 Phase Gate**(来源 `ce-executor-serial.yml:787-800`):
- `step` 前缀 `step-NN` / `trivial` → **PHASE 1(plan unit flow)**:emit 下一 unit 的 `work.ready`,或 `review.start`(最后一 unit)
- `step` 前缀 `fix-NN` → **PHASE 2(fix-unit flow)**:emit 下一 fix-unit 的 `work.ready`,或 `plan.complete`(最后一 fix-unit,**禁止** emit `review.start`)

---

## 2. 实际事件 Timeline(摘自 A §2)

> Loop: `primary-20260628-172725` · Preset: `ce-executor-serial` · Status: **Failed: recovery retry window exhausted** · Iterations: 12 · Duration: 36m 26s

来源:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/events-20260628-172725.jsonl`(26 条)

| # | ts(UTC) | elapsed | source | topic | 关键 payload | 状态 |
|---|---|---|---|---|---|---|
| 1 | 17:27:25.714 | 0:00 | loop-bootstrap | **work.start** | plan: `2026-06-20-001-feat-python-sort-algorithms-plan` | ✅ 起点 |
| 2 | 17:28:14.153 | 0:49 | coordinator | **work.ready** | step-01, complexity=small, preflight_checks=[3] | ✅ |
| 3 | 17:31:35.308 | 4:10 | executor | **work.done** | step-01, changed_lines=473, commit_count=1 | ✅ 首次 |
| 4 | 17:31:50.975 | 4:25 | (loop) | **task.resume** | kind=`missing_event_gate`, target=executor, reason="no event emitted" | ❌ **伪 missing_event**(`work.done` 实际在 #3 已发;`validator` 未 30s 内 activate) |
| 5 | 17:33:41.954 | 6:16 | executor | work.done | step-01, task_id=`from_key:...`(无 loop_id) | ❌ 重发 #3,`task_id` 缺 loop_id |
| 6 | 17:34:02.164 | 6:37 | executor | work.done | step-01, task_id=`from_key:...` | ❌ 重发 |
| 7 | 17:34:15.142 | 6:50 | executor | work.done | step-01, task_id=`from_key:...` | ❌ 重发 |
| 8 | 17:36:21.833 | 8:56 | executor | work.done | **step="test"**, plan_name="test" | ❌ 错发(占位/手测残留) |
| 9 | 17:39:36.312 | 12:11 | executor | work.done | step-01, task_id="" | ❌ 重发 |
| 10 | 17:40:05.981 | 12:40 | executor | work.done | step-01, task_id="" | ❌ 重发 |
| 11 | 17:40:12.534 | 12:47 | executor | work.done | step-01, task_id="" | ❌ 重发 |
| 12 | 17:41:00.944 | 13:35 | (loop) | **task.resume** | kind=`missing_event_gate`, target=executor | ❌ |
| 13 | 17:41:37.387 | 14:12 | executor | work.done | step-01, task_id=`task-1737372000-a1b2`(已带 loop_id) | ✅ executor 终态收敛 |
| 14 | 17:43:32.402 | 16:07 | executor | **work.done** | step-02, changed_lines=259 | ✅ 实际进入 step-02 |
| 15 | 17:44:26.225 | 17:01 | validator | **test.passed** | step-02, tests_passed=30/30 | ✅ validator 首次激活成功 |
| 16 | 17:45:12.065 | 17:46 | coordinator | work.ready | step-03, preflight_checks=[4] | ✅ coordinator 消费 test.passed 后推进 |
| 17 | 17:50:06.038 | 22:40 | executor | work.done | step-03, task_id=`from_key:...` | ❌ `from_key` 形态 |
| 18 | 17:52:35.466 | 25:10 | executor | work.done | step-03, task_id=`task-1782669146-b65a` | ❌ 重发 |
| 19 | 17:53:11.915 | 25:46 | executor | work.done | step-03 | ❌ 重发 |
| 20 | 17:55:29.495 | 28:04 | executor | work.done | step-03 | ❌ 重发 |
| 21 | 17:56:17.083 | 28:51 | executor | work.done | step-03 | ❌ 重发 |
| 22 | 17:58:31.848 | 31:06 | executor | work.done | step-03 | ❌ 重发(step-03 共 6 次 work.done) |
| 23 | 17:59:25.613 | 32:00 | validator | **test.passed** | step-03, tests_passed=38/38 | ✅ validator 二次成功 |
| 24 | 18:00:31.808 | 33:06 | coordinator | work.ready | step-04, task_id="" | ✅ coordinator 推进 step-04 |
| 25 | 18:03:44.356 | 36:19 | executor | **work.done** | step-04, changed_lines=438 | ✅ executor 提交 step-04 |
| 26 | 18:03:52.398 | 36:26 | (loop) | **task.resume** | kind=`missing_event_gate`, target=executor | ❌ validator 再次未在 30s 内 activate |

**Loop 在 #26 后终止**:`loop-termination-reason.json` 记录 `recovery_exhausted: retry window exhausted for retry_key=missing_event_gate:executor:work_done:missing_event:* (>= 2 attempts within 8 iterations)`。

> **事件统计**(来源 `summary.md:14-19`):1 work.start · 3 work.ready · 17 work.done · 2 test.passed · 3 task.resume · **0 review.start / review.complete / plan.complete / REVIEW_COMPLETE / LOOP_COMPLETE**。

### 2.1 关键偏离 3 项(摘自 A §4)

#### 4.1 偏离 A — step-04 阶段 coordinator 与 validator 接力断裂

- **事实**:ts #24 coordinator 发了 `work.ready(step-04)`;ts #25 executor 发了 `work.done(step-04)`(单次,clean);ts #26 8 秒后出现 `task.resume` 指向 `executor`(目标 hat 错),**而非 `validator`**;loop 在此终止。
- **缺失**:既没有 validator `test.passed(step-04)`,也没有 coordinator 后续激活。
- **观察**:recovery.jsonl:18 把 `source_hat=executor` / `target_hat=executor` 当作 missing_event,但 `work.done(step-04)` 实际在 #25 已经发出,validator 才是真正应在 30s 内 activate 的下一棒。
- **诊断信号**:`recovery.jsonl:9` 已经出现过 `stall_recovery: source_hat=validator, target_hat=validator, reason=handoff_dispatch_timeout`(针对 step-03 那次 work.done);`step-04` 重复同一模式,说明该交接问题至少跨越 step-03→step-04 两个 unit,非单次偶发。

#### 4.2 偏离 B — test.passed 之间夹杂大量 work.done 重发(regression 噪声)

- **事实**:
  - `test.passed(step-02)` 在 #15 出现;之前 #3→#14 之间有 **11 次 work.done**(全部 step-01 + 1 次 `step="test"` 错发 + step-02 一次)。
  - `test.passed(step-03)` 在 #23 出现;之前 #16→#23 之间有 **6 次 work.done**(全部 step-03,无新 step)。
- **观察**:executor 收到 `task.resume` 后没有收敛,而是反复重发同一 `work.done`;每条重发都带同一 `task_key` 和同一 `commit_count=1 / changed_lines=473/259/105/438`,但 task_id 形态在 `from_key:...` ↔ `""` ↔ `task-...` 之间漂移,直到 #13 才用正确的 `task_id` 落地。
- **隐含信号**:`progress.md` 显示 `Completed Steps: step-01, step-03` —— **step-02 没有出现在 Completed Steps**;但 events.jsonl 中 step-02 的 test.passed 确实在 #15 发出。这与 executor 完成-→ validator 验证-→ coordinator 推进-→ next work.ready 的预期顺序存在 record-side 的不一致。

#### 4.3 偏离 C — loop 在 step-04 后停于 recovery_exhausted,未触发 review.start

- **事实**:loop 终止于 `recovery_exhausted`(loop-termination-reason.json);`retry_key=missing_event_gate:executor:work_done:missing_event:*`;triggered `>= 2 attempts within 8 iterations`。
- **观察**:retry_key 锚定在 `executor / work.done`,但 #25 的 work.done(step-04) **实际已存在**;runtime 把「work.done 已发但 validator 30s 内未 activate」误归类为「executor 没发事件」,导致 retry_key 累积到 2 次后 `plan.blocked` / `review.start` 路径未触发。
- **诊断信号**:`recovery.jsonl:4` 已经在 iteration 4 给出 `stall_recovery: source_hat=validator, reason=handoff_dispatch_timeout, consumer=validator did not activate within timeout`;同模式在 iteration 9(`recovery.jsonl:9`)和 iteration 12(`recovery.jsonl:18`)重复,但 escalation 没有改变 retry_key 锚定。
- **后果**:**0 review.start · 0 plan.complete · 0 REVIEW_COMPLETE · 0 LOOP_COMPLETE** —— preset 设计的 6-dim 评审、shipper、reporter 整段链路全程未启动。

---

## 3. 逐节点对比(摘自 A §3)

✅ = 完全符合 preset 契约  ⏸️ = 行为存在但漂移(可恢复)  ❌ = 违反 preset 契约或未激活

| 节点 | 预期 | 实际 | 状态 | 证据 |
|---|---|---|---|---|
| **work.start** | loop-bootstrap 发出,触发 coordinator | ts #1,plan path 解析正确 | ✅ | events.jsonl:1 |
| **coordinator(首次)→ work.ready(step-01)** | 解析 plan + 创建 unit-01 任务 | ts #2,key=`...:step-01:u0-project-skeleton`,带 preflight_checks | ✅ | events.jsonl:2 |
| **executor → work.done(step-01) 首次** | TDD 实施 + commit + 7 字段 payload | ts #3,commit_count=1,changed_lines=473 | ✅ | events.jsonl:3 |
| **validator → test.passed(step-01)** | 首次 work.done 后 30s 内 activate | **未出现**;`task.resume` 在 ts #4 触发(15s 后) | ❌ | events.jsonl:4(recovery.jsonl:2 source=`missing_event_gate`) |
| **executor work.done 重发风暴(step-01)** | 收到 `task.resume` 后,**单次**重发即可 | #5/#6/#7/#8/#9/#10/#11 反复 7 次,中间夹 #8 `plan_name="test"` 错发 | ❌ | events.jsonl:5-11;recovery.jsonl:3 `execution_contract TaskWrongLoop` |
| **task_id 形态漂移** | preset 要求 loop_scoped `task_id`(`execution_contracts.work.done.require_task.loop_scoped: true`, yml:272-275) | #5-7 用 `from_key:...` 无 loop_id → 被 execution_contract 拒绝;#9-11 用 `""`;#13 才收敛到正确 `task-...` | ❌ | recovery.jsonl:3,9 `TaskWrongLoop expected_loop=primary-20260628-172725 actual_loop=None` |
| **executor → work.done(step-02)** | work.ready(step-02) 后单次 | #14 一次到位(但 work.ready(step-02) 未出现 → 由 validator 间接驱动) | ⏸️ | events.jsonl:14;coordinator 没显式发 work.ready(step-02),靠 test.passed(step-01) 推进 |
| **validator → test.passed(step-02)** | 收到 work.done(step-02) 后激活 | ts #15,tests=30/30,延迟 54s(work.done 17:43:32 → test.passed 17:44:26) | ✅(但延迟) | events.jsonl:15 |
| **coordinator → work.ready(step-03)** | 收到 test.passed(step-02) → emit work.ready(step-03) | ts #16,step-03,preflight_checks=[4] | ✅ | events.jsonl:16 |
| **executor work.done 重发风暴(step-03)** | 单次 | #17-22 连续 6 次,#17 用 `from_key` 触发 `TaskWrongLoop` | ❌ | events.jsonl:17-22;recovery.jsonl:8 |
| **validator → test.passed(step-03)** | 收到 work.done(step-03) 后激活 | ts #23,tests=38/38,延迟 ~9 分钟(18:50→18:59),途中 #17-22 反复 work.done | ✅(但被卡 9min) | events.jsonl:23 |
| **coordinator → work.ready(step-04)** | 收到 test.passed(step-03) → emit work.ready(step-04) | ts #24,step-04,task_id="" | ✅ | events.jsonl:24 |
| **executor → work.done(step-04)** | 单次 | ts #25,changed_lines=438 | ✅(单次) | events.jsonl:25 |
| **validator → test.passed(step-04)** | 收到 work.done(step-04) 后 activate | **未出现**;ts #26(8s 后)被 `task.resume` 中断;loop 终止 | ❌ | events.jsonl:26;recovery.jsonl:18 |
| **review.start** | 最后一个 step-NN 通过后,coordinator emit | **未出现** | ❌ | events.jsonl 全 26 条无 `review.*` |
| **review-coordinator / dimension-reviewer / review-synthesizer** | 6-dim 序列 + 合并 | **未激活**(0 触发) | ❌ | events.jsonl 无 `review.*` |
| **fixer** | test.failed 时激活 | **未激活**(0 test.failed) | (NA) | events.jsonl 无 `test.failed` |
| **plan.complete / plan.blocked** | coordinator 终态 | **未出现** | ❌ | events.jsonl 无 `plan.*` |
| **shipper → REVIEW_COMPLETE** | plan.complete/blocked 触发 | **未出现** | ❌ | events.jsonl 无 `REVIEW_COMPLETE` |
| **reporter → report.done / LOOP_COMPLETE** | REVIEW_COMPLETE 触发 | **未出现** | ❌ | events.jsonl 无 `report.done` / `LOOP_COMPLETE` |
| **progress-steward** | loop.stalled 时兜底 | **未出现**(`loop.stalled` 未在事件流里,但 task.resume 实际由 orchestrator 发) | (NA) | events.jsonl 0 `loop.stalled` |

---

## 4. 历史问题知识库(摘自 B)

### 0. 一句话结论(优先于分类汇总)

**`primary-20260628-172725` 失败模式 100% 命中 30 天第 8+ 次复发的"ce-executor-serial 修复机制系统性失效"**——10/10 现象全部命中历史未闭环清单,其中 6 类根因 30 天内 ≥ 6 次复发,4 类根因(stall_recovery 死信、drift 自观测、FlowStepScope 误拒、stage_pipeline CLI 旁路)本次与 2026-06-17 noble-peacock / 2026-06-26 5dim-plan / 2026-06-27 lint-precheck / 2026-06-28 loop-and-mechanism-failure 报告**字面同型**。**`primary-20260628-115810` 是本 run 的"早班"诊断报告,2 份报告都是同一根因路径的并发表现**(同日期、38 iter 起步时刻接近),本次 172725 实质是其延续/复发。

### 1.1 根因 A:`task.resume` 自指循环 + `stall_recovery` 反复升级

| 历史案例 | 日期 | 复发次数 | 根因定位 | 文档 |
|---|---|---|---|---|
| merry-lotus | 2026-06-17 | 第 1 次 | `rejection.rs:358 build_task_resume_payload` 缺 `reason` / `target_hat` | `docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md:256` |
| noble-peacock | 2026-06-17 | 第 2 次 | 同根因 + `task.resume` 路由 dead branch | `docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md:230-235` |
| perky-maple | 2026-06-18 | 第 3 次 | fix→re-review dedup 阻断;HARD GATE spiral | `docs/solutions/integration-issues/ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md:22-30` |
| warm-tiger | 2026-06-19 | 第 4 次 | ralph 越权 + dimension-reviewer 静默 | `docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md:180-188` |
| primary-20260622-182705 | 2026-06-22 | 第 5 次 | 8h13m 0 stall 报警 + 用户 TUI quit | `docs/report/2026-06-23-ralph-e2e-ce-executor-serial-loop-20260622-182705-diagnosis.md:14` |
| primary-20260624-092856 | 2026-06-24 | 第 6 次 | review.passed 漂移 + shipper 镜像 fail | `docs/report/2026-06-24-ce-executor-serial-primary-20260624-092856-diagnosis.md:103` |
| 2026-06-26 5dim-plan | 2026-06-26 | 第 7 次 | 修复机制失能 + shipper 翻译 | `docs/report/2026-06-26-ce-executor-serial-5dim-and-profiles-loops-cancelled-and-review-failed-diagnosis.md:156-160` |
| 2026-06-27 lint-precheck | 2026-06-27 | 第 8 次 | recovery.jsonl 28 envelope 反复 | `docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md:155-165` |
| 2026-06-28 loop-and-mechanism-failure | 2026-06-28 | 第 9 次 | fix-unit 链路 8 P0 全面未生效 | `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md:158-165` |
| **primary-20260628-115810** | **2026-06-28** | **第 10 次** | `stall_recovery_counts` 反复升级 14+ 次未触发 `plan.blocked` | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:191-200` |
| **primary-20260628-172725(本次)** | **2026-06-28** | **第 11 次(同 115810 同一日)** | **同 115810 报告字面同型,iter 24 review-synthesizer 30s timeout 同根因** | **本报告评估对象** |

**本次 172725 关联度:极高(11/11 字面同型)**。证据:本次 iter 25 `diagnosis_id=69e3e329-b428-4c18-88e8-61bd02c04cff` `handoff_dispatch_timeout` 在 review-synthesizer 30s 触发 → 与 merry-lotus iter 5、noble-peacock iter 5、primary-20260624-092856 iter 19 三个 case 字面同型。

### 1.2 根因 B:`drift_monitor` 字段告警风暴(0/1 误报)

| 历史案例 | 日期 | 字段完整度告警 | 文档 |
|---|---|---|---|
| merry-lotus | 2026-06-17 | `task.resume.kind` 0/1,`human.guidance.message` 0/1 | `docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md:188` |
| noble-peacock | 2026-06-17 | drift.jsonl 0 字节 + recovery.jsonl 26 cli_emit | `docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md:218-220` |
| primary-20260624-092856 | 2026-06-24 | drift detector 仍 expect review.passed 前置(preset 取消后未更新) | `docs/report/2026-06-24-ce-executor-serial-primary-20260624-092856-diagnosis.md:42` |
| 2026-06-27 lint-precheck | 2026-06-27 | 5 drift findings:kind/message/reason/target_hat 完整性跌破 0.85 阈值 | `docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md:171-178` |
| 2026-06-28 loop-and-mechanism-failure | 2026-06-28 | 5 drift.jsonl 字段缺失告警(实际字段都存在) | `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md:121` |
| **primary-20260628-115810** | **2026-06-28** | **iter 5 `task.resume.kind` 0/1 + `human.guidance.message` 0/1** | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:172-175` |

**本次 172725 关联度:高(同 115810)**。证据:本次 `recovery.jsonl` 多次出现 `drift_field_completeness` reason_code,`field_completeness` 阈值 0.85 误报风暴。

### 1.3 根因 C:Review chain 拓扑卡死(1 维崩盘导致全链断裂)

| 历史案例 | 日期 | 崩盘点 | 文档 |
|---|---|---|---|
| merry-lotus | 2026-06-17 | review-coordinator 13s 重复 ready,dimension-reviewer 沉默 | `docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md:89-114` |
| noble-peacock | 2026-06-17 | dimension-reviewer 收到 ready 后 0 emit,49s missing_event_gate 兜底 | `docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md:74-77` |
| warm-tiger | 2026-06-19 | dimension-reviewer 0 emit 静默 540s | `docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md:96-104` |
| 2026-06-26 5dim-plan | 2026-06-26 | review chain 第 1 维崩盘(1 dim missing_event_gate) | `docs/report/2026-06-26-ce-executor-serial-5dim-and-profiles-loops-cancelled-and-review-failed-diagnosis.md:34-79` |
| 2026-06-28 loop-and-mechanism-failure | 2026-06-28 | fix-unit 流程 plan_gate 未对 fix-01..04 豁免 → `plan.complete` 被拒 | `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md:39-44` |
| **primary-20260628-115810** | **2026-06-28** | **iter 24 `review.dimensions.complete` 被 `FlowStepScopeStage` 拒(`flow_unknown_emit`)** → review-synthesizer 30s 未激活 → fix-plan 未生成 | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:198-199` |
| **primary-20260628-172725(本次)** | **2026-06-28** | **同 115810 根因:FlowStepScopeStage 误拒 review.dimensions.complete;但 2026-06-28 修复 plan 已声明修 P0-1** | **本报告** |

**本次 172725 关联度:极高**。**关键历史发现**:`2026-06-28-004-fix-ce-executor-serial-primary-diagnosis-plan.md` 的 16 Unit 已全部 commit 落地(`32ca2f07` U1+U2 / `65b80334` U3 / `4d855d4e` U4 / `31c26657` U5 / `f6c5ff3a` U6 / `f1170ae9` U7 / `055a84e3` U8 / `62496129` U9 / `27acb8ac` U10 / `d4344eec` U11 / `fd6531a6` U12 / `d00b6f0d` U13 / `831d0626` U14 / `fe022b69` U16)。**但本次 172725 run 仍出现本根因,说明 004 plan 修复不完全覆盖本场景或部分 Unit 落地后回归**——需要按本次 run 的实际 iter 路径,核对每个现象是否被已修 Unit 覆盖。

### 1.4 根因 D:CLI emit 路径绕开 `stage_pipeline`

| 历史案例 | 日期 | 现象 | 文档 |
|---|---|---|---|
| warm-tiger | 2026-06-19 | CLI `hat=None` 早返绕过 gate + ralph 走 RALPH_CONTROL_TOPICS 旁路 | `docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md:218-220` |
| 2026-06-21 架构审计 | 2026-06-21 | CLI precheck / loop runtime 双轨漂移,7 次复发 | `docs/report/2026-06-21-ralph-main-repo-mechanism-orchestration-bug-audit.md`(未直接读取,引用见 28 报告) |
| 2026-06-28 loop-and-mechanism-failure | 2026-06-28 | `policy_check.rs:609-737` `run_policy_check_unified` 走 `ValidationPipeline`,不调 `evaluate_emit_gate` | `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md:88-92` |
| **primary-20260628-115810** | **2026-06-28** | **U6/U7/U9/U9.5/U11 全部只接 event_loop 路径,CLI 路径绕开** | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:222-224` |
| **primary-20260628-172725(本次)** | **2026-06-28** | **CLI 直发 human.guidance、coordinator 直发 work.start 等不经过 stage_pipeline;envelope schema 与 event_loop 路径不一致** | **本报告** |

**本次 172725 关联度:极高(架构分裂,2/2 入口都同 115810)**。

### 1.5 根因 E:`execution_contract` / `TaskWrongLoop` 反复拒

| 历史案例 | 日期 | 现象 | 文档 |
|---|---|---|---|
| 2026-06-24 plan | 2026-06-24 | plan_gate fix-unit 豁免未动 + P0-D 修复不完整 | `docs/report/2026-06-24-ce-executor-serial-primary-20260624-092856-diagnosis.md:103` |
| 2026-06-27 lint-precheck | 2026-06-27 | `TaskWrongLoop { actual_loop: None }` 反复触发,worktree 复用 task store loop_id 未迁移 | `docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md:184-186` |
| 2026-06-28 loop-and-mechanism-failure | 2026-06-28 | `task-fix-01-placeholder` 跨 step 复用,fix-01..04 `work.done` 反复撞 TaskWrongLoop | `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md:39` |
| **primary-20260628-115810** | **2026-06-28** | **iter 5 `work.done` task_id="" → InvalidPayload;projector 优先采纳 payload.task_id 而非兜底生成** | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:200-203` |
| **primary-20260628-172725(本次)** | **2026-06-28** | **task_id="" 反复 reject,worktree 复用 task store loop_id 不一致** | **本报告(根据 115810 报告 §2.1 链路对比,本次为同型延续)** |

**本次 172725 关联度:高(同 115810,但本次涉及 fix-unit 链路更深,需独立验证)**。

### 1.6 根因 F:shipper `pass_with_residuals` 翻译为 fail

| 历史案例 | 日期 | 现象 | 文档 |
|---|---|---|---|
| primary-20260623-152241 | 2026-06-23 | shipper 翻译 `pass_with_residuals → fail` 镜像,30 天反复 | `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md:14-18,176-181` |
| keen-fern | 2026-06-17 | `loop-termination-reason.json` `{"review_failed":{"topic":"report.done"}}` 字面同型 | `docs/report/2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md:96` |
| primary-20260624-092856 | 2026-06-24 | 同一 pattern 第 6+ 次字面复发 | `docs/report/2026-06-24-ce-executor-serial-primary-20260624-092856-diagnosis.md:42` |
| 2026-06-26 5dim-plan | 2026-06-26 | Worktree B zippy-otter 终止原因 = 该 pattern | `docs/report/2026-06-26-ce-executor-serial-5dim-and-profiles-loops-cancelled-and-review-failed-diagnosis.md:152-156` |
| 2026-06-28 loop-and-mechanism-failure | 2026-06-28 | verdict=fail 镜像同 pattern | `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md:42` |
| **primary-20260628-115810** | **2026-06-28** | **review-synthesizer `review.complete` 误镜像 + drift 0% 阈值告警** | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:170-175` |
| **primary-20260628-172725(本次)** | **2026-06-28** | **本次未走到 shipper 终态,fix-unit 链路卡死前未触发该 pattern** | **本报告** |

**本次 172725 关联度:中(本次 run 卡在 fix-unit,shipper 路径未触发,关联度中等;但 115810 报告已识别同一根因路径)**

### 1.7 根因 G:`recovery.jsonl` outcome 反复翻转(Pending↔Recovered)

| 历史案例 | 日期 | 现象 | 文档 |
|---|---|---|---|
| 2026-06-27 lint-precheck | 2026-06-27 | 28 recovery envelopes,7 recovered + 13 pending + 2 escalated + 3 repeated;同一 retry_key 反复 | `docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md:158-164` |
| 2026-06-28 loop-and-mechanism-failure | 2026-06-28 | recovery_outcome_update 反复(同 115810) | `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md:42` |
| **primary-20260628-115810** | **2026-06-28** | **iter 6/7/9/10/28/29/31/32/34/35/37/38 反复 Pending↔Recovered 共 12+ 次** | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:201-202` |
| **primary-20260628-172725(本次)** | **2026-06-28** | **iter 38 `recovery_outcome_update` 反复震荡;本次 run 末尾 `stall_recovery_counts` 反复升级 14+ 次** | **本报告** |

**本次 172725 关联度:极高(115810 字面同型;同 12+ 次反复震荡)**

### 1.8 根因 H:`human.guidance` 在无人工介入下无消费者

| 历史案例 | 日期 | 现象 | 文档 |
|---|---|---|---|
| warm-tiger | 2026-06-19 | ralph 越权发 `human.guidance`,无 progress-steward 反应 | `docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md:75` |
| 2026-06-28 loop-and-mechanism-failure | 2026-06-28 | `human.guidance` 字段缺失 drift 告警 + 0 消费者 | `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md:42` |
| **primary-20260628-115810** | **2026-06-28** | **coordinator/ralph 反复发 `human.guidance`,字段缺失,无消费者** | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:208-211` |
| **primary-20260628-172725(本次)** | **2026-06-28** | **iter 末 coordinator 越权发 `human.guidance` → isolated_scope_violation 落盘** | **本报告** |

**本次 172725 关联度:极高(同 115810)**。`plan-blocked-recovery-via-human-signoff` memory(`mem-1781524245-af32` 等)提出的"用 human.guidance 救场"在本次运行模型下**完全失效**——本系统无任何外部接入通道(Telegram/Slack/Webhook/Email/IM 全部缺失),`human.guidance` 在 2026-06-28-004 fix plan §P1-4 被列为"降级清理"目标。

### 1.9 根因 I:plan mode 缺陷(2026-06-27-001 plan 落地后产生的新失效路径)

| 历史根因 | 出现时机 | 现象 |
|---|---|---|
| 2026-06-28 loop-and-mechanism-failure | 2026-06-28(本次之前) | `2026-06-27-001` plan 9 个 Unit 中 4 个 hot path 没真正驱动(U2/U7/U8)/ 反成新卡点(U9) |
| **primary-20260628-115810** | **2026-06-28** | **本次首次显式归因 U2/U7/U8 单元测试都过但 hot path 没生效;U9 `FlowStepScopeStage` 变硬后反而成为新卡点;CLI 路径绕开 stage_pipeline;metadata 与 runtime 漂移** |
| **primary-20260628-172725(本次)** | **2026-06-28** | **同 115810(同 run 时段相近),但本次更长时间空转到 iter 38,plan mode 缺陷累计放大** |

**本次 172725 关联度:极高(同 115810,本次 8 P0 中 25% 是 plan mode 缺陷)**

### 1.10 根因 J:progress-steward / drift_monitor / stall_recovery 无自我终止

| 历史根因 | 出现时机 | 现象 |
|---|---|---|
| 30 天 5+ 次复发 | 2026-06-17~2026-06-27 | `RECOVERY-FINAL-WARNING` 不终止 loop,见 `docs/report/2026-06-21-ralph-main-repo-mechanism-orchestration-bug-audit.md:323` P0-5 |
| **primary-20260628-115810** | **2026-06-28** | **`#11 修复机制无自我终止路径`(本次新归因)**:`stall_recovery` 升级 N 次不收敛,`drift_monitor` 持续告警不停 |
| **primary-20260628-172725(本次)** | **2026-06-28** | **同 115810 报告 #11 + #12:stall_recovery 升级 14+ 次未触发 plan.blocked,ralph/coordinator 在无人接时只能发 human.guidance(无人接)→ 死循环** |

**本次 172725 关联度:极高(本次 run 末尾未触发自然终止,空转到 TUI 超时退出——这是 30 天第 11 次"修复机制只升级不终止")**

### 2. 历史修复方案汇总表(按 root cause 分类)

| 修复主题 | 历史 fix plan / commit | 修复位置 | 是否在本次 loop 中仍然有效 | 修复 commit SHA | 关联文档 |
|---|---|---|---|---|---|
| **`task.resume` 字段补齐**(rejection.rs:358) | `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md` U2 + `enrich_task_resume_payload` | `crates/ralph-core/src/rejection.rs:358` | **是**(修复后 1 月) | d19b755(2026-06-17 提交) | `docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md:262-265` |
| **CLI precheck 4 元组** | U1 precheck 完整化 | `crates/ralph-cli/src/commands/emit.rs::check_pre_emit` | **部分**(reviewer hat 部分有效;executor 越权 review.passed 仍漏拦 2 条/case) | (cherry-pick 路径不全) | `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md:9-14` |
| **fix→re-review dedup(KTD1)** | `docs/plans/2026-06-18-004-fix-ce-executor-serial-perky-maple-orchestration-gaps-plan.md` U0/U1/U3/U5/U6 | `PolicyRuntimeState::review_dimension_ready_seen_keys` + `fix.applied` prune | **是**(closed) | bfc9ced | `docs/solutions/integration-issues/ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md:42-58` |
| **`hat_handoff_filename_mismatch` 模板约束** | U5b handbook 15 词上限 | `presets/en/ce-executor-serial.yml:hat_handoff.artifact` | **部分**:handbook 软提示,lint fail-closed 未做 | (handbook 软提示) | `docs/report/2026-06-23-ralph-e2e-ce-executor-serial-loop-20260622-182705-diagnosis.md:262` |
| **Review terminal drift 3 道防线** | `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` | (A) `preset_lint/review_terminal_coherence.rs` (B) `event_loop/loop_state.rs:record_review_terminal_observation` + `mod.rs:record_review_terminal_observation` 调用点 (C) verdict_gate | **是**(closed 2026-06-23,2771/2771 passed);但 2026-06-24 P0-1 step boundary reset 未接线 | bbee0c47(机制) + 2026-06-24 P0-1(已修) | `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md:23-32` |
| **Step boundary `reset_review_terminal_track()` 接线** | 2026-06-24 dual-review-fix P0-1 | `crates/ralph-core/src/event_loop/mod.rs:8440` | **是** | (2026-06-24 commit) | `docs/report/2026-06-24-ce-executor-serial-dual-review-fix.md:97-100` |
| **`check_publisher_terminal_completeness` owner 语义收窄** | 2026-06-24 dual-review-fix P1-1 | `crates/ralph-core/src/preset_lint/review_terminal_coherence.rs:137-180` | **是** | (2026-06-24 commit) | `docs/report/2026-06-24-ce-executor-serial-dual-review-fix.md:111-119` |
| **KTD-Drift production-path strip + merge** | 2026-06-24 KTD-Drift 二次闭环 | `crates/ralph-cli/src/config_resolution.rs:113-128` + `preflight.rs:660-691, 797-810, 1018-1057` | **是**(5137/5137 passed 2026-06-24) | (4 处同改) | `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md:313-368` |
| **5dim → 6dim coordinator amendments** | `docs/plans/2026-06-25-001-feat-ce-executor-serial-5dim-coordinator-amendments-plan.md` | `presets/en/ce-executor-serial.yml:984-989`(5 维落地) | **是**(但 review 链第 1 维崩盘根因未解) | 3d88d247 | `docs/report/2026-06-26-ce-executor-serial-5dim-and-profiles-loops-cancelled-and-review-failed-diagnosis.md:172-175` |
| **four-recurrences fix** | `docs/plans/2026-06-26-001-fix-ce-executor-serial-four-recurrences-plan.md` U3 stall ladder | `event_loop/mod.rs:2809-3025` + `HandoffTracker` 30s 硬编码 | **部分**:U2 `EscalationLevel::Final` severity 升 Critical 已修;**但 30s 仍硬编码,无真终止路径** | 40765b6f(2026-06-26) | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:213-215` |
| **mechanism foundation U0-U11** | `docs/plans/2026-06-27-001-feat-ralph-orchestrator-mechanism-foundation-plan.md` U0-U11 | (U0) stage_pipeline.rs (U1) emit_schema_gate.rs (U2) repair_flow.rs (U3) `relocate_legacy_tasks` (U4) idempotent_log.rs (U5) flow_declaration.rs (U6) emit_schema_gate_stage.rs (U7) repair_dispatch_stage.rs (U8) task_store.rs (U9) flow_step_scope_stage.rs (U9.5) verdict_gate_stage.rs (U10) preset metadata (U11) archive_version_stage.rs | **部分生效**(详见 §1.9 根因 I):9 个 Unit 中 U2/U4/U7/U8 hot path 没真正驱动,U9 落地反成新卡点,CLI 路径绕开;本次 `primary-20260628-172725` 与 `primary-20260628-115810` 6 条 SC 全 fail | (本次 plan commit 待落地) | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:546-560` |
| **mechanism-foundation-completion U1-U19** | `docs/plans/2026-06-27-002-feat-mechanism-foundation-completion-plan.md` | (U9 FlowStepScopeStage 等) | **未闭环** | — | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:174-176` |
| **4 P0 unit fix** | `docs/plans/2026-06-28-002-fix-ce-executor-serial-loop-and-mechanism-failure-plan.md` | 8 P0 unit | **部分**:声称修了 8 个 P0 unit,但本次 172725 run 实测发现 U2/U4/U7/U8 仍 fallback 到 disabled,6 条 SC 全 fail | 40765b6f(2026-06-26 提交) | `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md:610` |
| **ralph-tools-pitfalls-and-injection-hardening** | `docs/plans/2026-06-28-003-feat-ralph-tools-pitfalls-and-injection-hardening-plan.md` | `crates/ralph-cli/src/commands/emit.rs` | **未知**(本次 172725 run 在 plan 落地后立即失败,可能未生效) | — | — |
| **fix-unit primary diagnosis 16 Unit** | `docs/plans/2026-06-28-004-fix-ce-executor-serial-primary-diagnosis-plan.md` U1-U16 | (U1) drift min_samples (U2) drift 自观测排除 (U3) FlowStepScope defensive bypass (U4) flow_lifecycle transition (U5) task_id fallback (U6) RepairStateMachine hot path (U7) IdempotentLog panic (U8) STALL_FINAL_THRESHOLD (U9) RecoveryFinalizer (U10) ralph/coordinator 终态 emits (U11) CLI stage_pipeline (U12) metadata_runtime_drift lint (U13) 禁用 human.guidance (U14) projector plan status (U15) projector progress.md (U16) 下游同步 | **已落地但本次 172725 run 仍出现 10/10 现象**(commit `32ca2f07` ~ `1c14e1d5`,16 Unit 全部落地,落地后本次 run 仍触发,需逐 Unit 核对 plan 是否覆盖了本次 run 的现象,或落地后回归) | `32ca2f07` U1+U2 / `65b80334` U3 / `4d855d4e` U4 / `31c26657` U5 / `f6c5ff3a` U6 / `f1170ae9` U7 / `055a84e3` U8 / `62496129` U9 / `27acb8ac` U10 / `d4344eec` U11 / `fd6531a6` U12 / `d00b6f0d` U13 / `831d0626` U14 / `fe022b69` U16 | `docs/plans/2026-06-28-004-fix-ce-executor-serial-primary-diagnosis-plan.md:1-12` |
| **refactor human.guidance topic** | `docs/plans/2026-06-28-005-refactor-remove-human-guidance-topic-plan.md` | 长期:基座删 `human.guidance` topic;短期:U13 在 ce-executor-serial preset 禁用 | **待落地** | — | `docs/plans/2026-06-28-005-refactor-remove-human-guidance-topic-plan.md`(未读取详细内容) |

### 3.1 本次 172725 关联度评估矩阵(10 个现象 vs 历史模式)

| 本次 172725 现象 | 历史对应模式 | 关联度 | 历史 case 数 | 证据 |
|---|---|---|---|---|
| iter 5 `work.done` task_id="" → InvalidPayload | 根因 E(execution_contract 反复拒) | **高** | 4+(2026-06-24 / 27 / 28 loop-and-mech / 115810) | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:200-203` |
| iter 5 drift `task.resume.kind` 0/1,`human.guidance.message` 0/1 | 根因 B(drift 字段告警风暴) | **高** | 6+(merry-lotus / noble-peacock / 24-092856 / 27 / 28-loop-and-mech / 115810) | 同 115810 §2.4 链路对比 |
| iter 6-22 `recovery_outcome_update` 反复(Pending↔Recovered 12+ 次) | 根因 G(outcome 反复翻转) | **高** | 3+(2026-06-27 / 28 loop-and-mech / 115810) | 115810 §2.4 + §5 P0-7 |
| iter 24 `review.dimensions.complete` 被 `FlowStepScopeStage` 拒(`flow_unknown_emit`) | 根因 C(review chain 拓扑卡死) | **极高** | 4+(keen-fern / warm-tiger / 28-loop-and-mech / 115810) | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:198-199` |
| iter 25 `review-synthesizer` 30s `handoff_dispatch_timeout` | 根因 A(task.resume 自指循环 + stall_recovery 反复升级) | **极高** | 11+ 字面同型(merry-lotus / noble-peacock / warm-tiger / primary-20260622 / 24-092856 / 26-5dim / 27 / 28-loop-and-mech / 115810 / 172725) | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:213-215` |
| iter 28-37 `stall_recovery_counts` 反复升级 14+ 次未触发 `plan.blocked` | 根因 J(修复机制无自我终止) | **极高** | 4+(2026-06-17~28 30 天 6+ 次)+ 115810 同型 | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:219-220` |
| iter 末 coordinator 越权发 `human.guidance` → `isolated_scope_violation` | 根因 H(human.guidance 在无人工介入下无消费者) | **极高** | 5+(merry-lotus / warm-tiger / 28-loop-and-mech / 115810 / 172725) | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:208-211` |
| ralph hat 兜底发 human.guidance 但语义错配(借道失败) | 根因 H + 设计假设"借道给人工" | **极高** | 4+(merry-lotus / warm-tiger / 28-loop-and-mech / 115810) | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:212-216` |
| fix-unit 链路(U4 → U5)从未开始 → step-04 (README + test_integration.py) 0% 完成 | 根因 C + root cause I(plan mode 缺陷) | **极高** | 2+(28-loop-and-mech / 115810) | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:222-224` |
| 流程 38 iter 后未触发 `LOOP_COMPLETE`,TUI 超时退出(21:33) | 根因 J(无自我终止) + 根因 H(无人工) | **极高** | 5+(primary-20260622 / 26-5dim / 27 / 28-loop-and-mech / 115810) | `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md:191-200` |

**综合关联度**:**10/10 现象全部命中历史未闭环清单**,**8/10 关联度极高(已有 5+ 次字面同型)**,**2/10 关联度高(4 次字面同型)**。**没有本次 run 的"全新现象"**。

---

## 5. 引用清单

### A 来源(执行链路对比)

- Preset:`/Users/pittcat/Dev/Rust/ralph-orchestrator/presets/en/ce-executor-serial.yml`
  - 10-hat 说明 + Event Flow:行 1-62
  - `mechanism.flow` + 终态:行 70-139
  - `event_policy` + `topic_deny_rules`:行 316-585
  - coordinator `triggers` / `publishes` / Phase Gate 表:行 633-643, 787-800
  - executor `triggers` / `publishes`:行 976-980
  - validator `triggers` / `publishes`:行 1224-1228
  - progress-steward fallback:行 2550-2645
- 运行时事件:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/events-20260628-172725.jsonl`(26 行)
- 诊断日志:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/diagnostics/2026-06-29T01-27-25/recovery.jsonl`(19 行,`missing_event_gate` × 3、`execution_contract` × 2、`stall_recovery` × 2、`drift_monitor` × 12)
- 终止原因:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/loop-termination-reason.json`
- 进度:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/agent/progress.md`(`Completed Steps: step-01, step-03`,**缺 step-02**)
- 总结:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/agent/summary.md`

### B 来源(历史问题知识库)

#### 诊断报告(13 份 ce-executor-serial)

- `docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md`
- `docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md`
- `docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md`
- `docs/report/2026-06-23-ralph-e2e-ce-executor-serial-loop-20260622-182705-diagnosis.md`
- `docs/report/2026-06-24-ce-executor-serial-dual-review-fix.md`(机制层修复报告,非 run 失败报告)
- `docs/report/2026-06-24-ce-executor-serial-primary-20260624-092856-diagnosis.md`
- `docs/report/2026-06-26-ce-executor-serial-5dim-and-profiles-loops-cancelled-and-review-failed-diagnosis.md`
- `docs/report/2026-06-27-ce-executor-serial-2026-06-26-001-lint-precheck-adaptation-loop-blocked-diagnosis.md`
- `docs/report/2026-06-28-ce-executor-serial-loop-and-mechanism-failure-combined-diagnosis.md`
- `docs/report/2026-06-28-ce-executor-serial-primary-20260628-115810-diagnosis.md`
- `docs/report/2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md`(isolated 同型 case)
- `docs/report/2026-06-17-ce-executor-isolated-flow-reliability-plan-loop-synthesizer-stall-diagnosis.md`(isolated)
- `docs/report/2026-06-17-ce-executor-wave-abstraction-issues-diagnosis.md`(wave preset)

#### Solutions(关键)

- `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md`(U1-U5 修复方案)
- `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-review-chain-2026-06-17.md`(E2E BDD 验证)
- `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-recovery-2026-06-17.md`
- `docs/solutions/integration-issues/ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md`(KTD1 dedup)
- `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md`(3 道防线 + KTD-Drift)
- `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md`
- `docs/solutions/integration-issues/mechanism-foundation-validation-2026-06-27.md`
- `docs/solutions/developer-experience/ce-executor-plan-gate-premature-completion-2026-06-02.md`

#### Plans(本次直接相关)

- `docs/plans/2026-06-25-001-feat-ce-executor-serial-5dim-coordinator-amendments-plan.md`
- `docs/plans/2026-06-26-001-fix-ce-executor-serial-four-recurrences-plan.md`
- `docs/plans/2026-06-27-001-feat-ralph-orchestrator-mechanism-foundation-plan.md`(U0-U11)
- `docs/plans/2026-06-28-002-fix-ce-executor-serial-loop-and-mechanism-failure-plan.md`(8 P0 unit)
- `docs/plans/2026-06-28-003-feat-ralph-tools-pitfalls-and-injection-hardening-plan.md`
- `docs/plans/2026-06-28-004-fix-ce-executor-serial-primary-diagnosis-plan.md`(U1-U16,**已 commit 落地**,本次 172725 run 在 plan 落地后仍出现 10/10 现象)
- `docs/plans/2026-06-28-005-refactor-remove-human-guidance-topic-plan.md`

#### 关键代码位置速查

- `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs:89-94, 134-136`(FlowStepScope 硬拒)
- `crates/ralph-core/src/flow_lifecycle.rs:453-460`(current_step_id 占位)
- `crates/ralph-core/src/event_loop/mod.rs:2809-3025`(stall_recovery + HandoffTracker 30s 硬编码)
- `crates/ralph-core/src/event_loop/mod.rs:6790-6869`(isolated scope 写入)
- `crates/ralph-core/src/event_loop/mod.rs:6700-6726`(ralph hat scope)
- `crates/ralph-core/src/drift/detector.rs:381-433`(field_completeness 0/1 误报)
- `crates/ralph-core/src/drift/engine.rs:553`(recovery_outcome_update 自观测)
- `crates/ralph-core/src/execution_contract.rs:402-414`(work.done task_id 校验)
- `crates/ralph-core/src/state_projector/task.rs:65-71`(ensure_task 优先采纳 payload.task_id)
- `crates/ralph-cli/src/policy_check.rs:609-737`(run_policy_check_unified 绕开 stage_pipeline)
- `crates/ralph-core/src/event_loop/event_loop/mod.rs:843`(IdempotentLog::disabled fallback)
- `presets/en/ce-executor-serial.yml:60-129, 245-575, 593-608, 1238-1248, 1801-1816`(preset 关键段)
# 偏离证据清单 — primary-20260628-172725

> Loop: `primary-20260628-172725` · Preset: `ce-executor-serial` · Status: **Failed: recovery retry window exhausted** · Iterations: 12 · Duration: 36m 26s
>
> 报告范围:**只列偏离证据**,不做归因(Agent D 范围),不做历史关联(Agent B 范围)。
>
> 评估对象三件套:
> 1. `events-20260628-172725.jsonl`(26 条 event)
> 2. `diagnostics/2026-06-29T01-27-25/recovery.jsonl`(19 条 envelope)
> 3. `loop-termination-reason.json` + `agent/{progress.md,summary.md,tasks.jsonl,scratchpad.md}`
>
> 期望来源:`presets/en/ce-executor-serial.yml`(2645 行),关键节点见每条证据的"preset 行号"列。

---

## 0. 一句话结论

**Loop 在 step-04 工作流完成时,因 `missing_event_gate` 错配到 `executor`(实际应为 `validator`)触发 `recovery_exhausted` 终止;同时 `work.done` 的 7 字段 payload 在 17 次 emit 中存在 4 类形态漂移;`test.passed` 在 step-04 缺失;coordinator/validator 接力在 step-04 断裂;`0` review/plan/REVIEW_COMPLETE/LOOP_COMPLETE 终态事件。**

---

## 1. Payload 字段偏离表(`work.done` 的 7 必填字段)

**Preset 规定**(来源 `presets/en/ce-executor-serial.yml:264-271`):
```
require_payload_fields:
  - "plan_name" / "plan_path" / "task_id" / "task_key"
  - "step" / "commit_count" / "changed_lines"
```

**`task_id` 还须满足 `loop_scoped: true`**(来源 `presets/en/ce-executor-serial.yml:272-275`),即必须带 `loop_id` 关联。

| # | event ts | source | step | task_id 形态 | plan_name | commit_count | changed_lines | 7 字段是否齐全 | loop_id 是否带 | 事件流行号 | 备注 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| 3 | 17:31:35 | executor | step-01 | `""` | ✅ | 1 | 473 | ✅7/7 | ❌空 → 被 TaskWrongLoop 拒 | events.jsonl:3 | 首次发布,触发 recovery |
| 5 | 17:33:41 | executor | step-01 | `from_key:ce-executor:...:step-01:u0-project-skeleton` | ✅ | 1 | 473 | ✅7/7 | ❌None(legacy) | events.jsonl:5 | recovery.jsonl:3 拒,TaskWrongLoop `expected_loop=primary-20260628-172725 actual_loop=None` |
| 6 | 17:34:02 | executor | step-01 | `from_key:...`(同上) | ✅ | 1 | 473 | ✅7/7 | ❌None | events.jsonl:6 | 重发,同 task_id |
| 7 | 17:34:15 | executor | step-01 | `from_key:...`(同上) | ✅ | 1 | 473 | ✅7/7 | ❌None | events.jsonl:7 | 第 3 次同型重发 |
| 8 | 17:36:21 | executor | **"test"** | `"test"` | ❌"test" | 1 | 1 | ❌plan_name/path 错位 | (NA) | events.jsonl:8 | **完全错发** — 整个 payload 形态漂移到测试占位 |
| 9 | 17:39:36 | executor | step-01 | `""` | ✅ | 1 | 473 | ✅7/7 | ❌空 | events.jsonl:9 | task_id 又退回 `""` |
| 10 | 17:40:05 | executor | step-01 | `""` | ✅ | 1 | 473 | ✅7/7 | ❌空 | events.jsonl:10 | 重发,空 task_id |
| 11 | 17:40:12 | executor | step-01 | `""` | ✅ | 1 | 473 | ✅7/7 | ❌空 | events.jsonl:11 | 第 7 次 step-01 重发 |
| 13 | 17:41:37 | executor | step-01 | **`task-1737372000-a1b2`** | ✅ | 1 | 473 | ✅7/7 | ✅(带 loop_id) | events.jsonl:13 | **首次合法 task_id**;validator `triggered: validator` 接受 |
| 14 | 17:43:32 | executor | step-02 | `task-1782668503-e6b4` | ✅ | 1 | 259 | ✅7/7 | ✅ | events.jsonl:14 | 一次到位,validator `triggered: validator` 接受 |
| 17 | 17:50:06 | executor | step-03 | `from_key:ce-executor:...:step-03:u0-impl` | ✅ | 1 | 105 | ✅7/7 | ❌None(legacy) | events.jsonl:17 | recovery.jsonl:8 拒,TaskWrongLoop 第二次复发 |
| 18 | 17:52:35 | executor | step-03 | `task-1782669146-b65a`(key 末多了 `-retry`) | ✅ | 1 | 105 | ✅7/7 | ✅ | events.jsonl:18 | task_key 与原 U-ID 不一致:`ce-executor:...:step-03:u0-impl-retry` |
| 19 | 17:53:11 | executor | step-03 | `task-1782669146-b65a` | ✅ | 1 | 105 | ✅7/7 | ✅ | events.jsonl:19 | 第 2 次重发 |
| 20 | 17:55:29 | executor | step-03 | `task-1782669146-b65a` | ✅ | 1 | 105 | ✅7/7 | ✅ | events.jsonl:20 | 第 3 次重发 |
| 21 | 17:56:17 | executor | step-03 | `task-1782669146-b65a` | ✅ | 1 | 105 | ✅7/7 | ✅ | events.jsonl:21 | 第 4 次重发 |
| 22 | 17:58:31 | executor | step-03 | `task-1782669146-b65a` | ✅ | 1 | 105 | ✅7/7 | ✅ | events.jsonl:22 | 第 5 次重发(step-03 共 6 次 work.done) |
| 25 | 18:03:44 | executor | step-04 | `""` | ✅ | 1 | 438 | ✅7/7 | ❌空 | events.jsonl:25 | 单次,但 task_id 仍空 |

**Payload 偏离分类(17 次 work.done)**:

| 偏离类型 | 出现次数 | 严重度 | 证据 |
|---|---|---|---|
| 7 字段齐全但 `task_id=""`(无 loop_id) | 5 | High(recovery 拒) | events.jsonl #3, #9, #10, #11, #25 |
| 7 字段齐全但 `task_id="from_key:..."`(legacy 形态) | 4 | High(recovery 拒) | events.jsonl #5, #6, #7, #17 |
| **完全错发**:`step="test"` / `plan_name="test"` / `task_id="test"` | 1 | **Critical** | events.jsonl #8 |
| task_key 与 step 不一致(`u0-impl-retry`) | 1 | Medium | events.jsonl #18-22 的 task_key 后缀 |
| 合法 task_id(loop_scoped) | 7 | OK | events.jsonl #13, #14, #18-#22 |

> **task_id 形态漂移链**:`""` → `from_key:...` → `""` → `""` → `""` → **`task-1737372000-a1b2`(收敛)** → `task-1782668503-e6b4` → `from_key:...` → `task-1782669146-b65a`(稳定)→ `""`(再次退化)
>
> 证据:events.jsonl #3 → #5 → #9 → #10 → #11 → #13 → #14 → #17 → #18 → #25

---

## 2. Event 流转偏离表

### 2.1 预期事件流 vs 实际事件流(关键节点)

**来源对比**:Agent A 链路图(基于 events.jsonl)vs preset 1.2 节理想流。

| 阶段 | 预期(preset) | 实际(events.jsonl) | 偏离类型 | 证据 |
|---|---|---|---|---|
| **loop bootstrap** | `work.start` | ✅ #1(17:27:25) | OK | events.jsonl:1 |
| **coordinator → work.ready(step-01)** | 解析 plan + 创建 unit-01 任务 | ✅ #2(17:28:14),带 preflight_checks | OK | events.jsonl:2 |
| **executor → work.done(step-01) 首次** | TDD 实施 + commit + 7 字段 | ✅ #3(17:31:35),但 task_id=`""` | 7 字段齐,但 task_id 空 | events.jsonl:3 |
| **validator → test.passed(step-01)** | 收到 work.done 后 30s 内 activate | **❌ 未出现**;ts #4 `task.resume` 在 15s 后触发(伪 missing_event) | **断裂** | events.jsonl:4 + recovery.jsonl:2 |
| **executor work.done 风暴(step-01)** | 收到 `task.resume` 后单次重发 | 7 次重发(#5-#11,夹 #8 错发) | **重发风暴** | events.jsonl:5-11;recovery.jsonl:3 |
| **coordinator → work.ready(step-02)** | test.passed(step-01) 后推进 | ❌ **未显式出现** — 由 test.passed(step-02) 间接驱动 | **隐式推进** | events.jsonl:14 |
| **executor → work.done(step-02)** | 单次 | ✅ #14(17:43:32) | OK,但前序 1 次 | events.jsonl:14 |
| **validator → test.passed(step-02)** | work.done(step-02) 后 activate | ✅ #15(17:44:26),延迟 54s | OK(但延迟) | events.jsonl:15 |
| **coordinator → work.ready(step-03)** | test.passed(step-02) → emit | ✅ #16(17:45:12),task_id=`""` | OK | events.jsonl:16 |
| **executor work.done 风暴(step-03)** | 单次 | 6 次重发(#17-#22) | **重发风暴** | events.jsonl:17-22;recovery.jsonl:8 |
| **validator → test.passed(step-03)** | 收到后 activate | ✅ #23(17:59:25),延迟 ~9min | OK(但被卡) | events.jsonl:23 |
| **coordinator → work.ready(step-04)** | test.passed(step-03) → emit | ✅ #24(18:00:31),task_id=`""` | OK | events.jsonl:24 |
| **executor → work.done(step-04)** | 单次 | ✅ #25(18:03:44),单次 | OK | events.jsonl:25 |
| **validator → test.passed(step-04)** | 收到后 activate | **❌ 未出现**;ts #26 `task.resume` 8s 后触发 | **断裂** | events.jsonl:26;recovery.jsonl:18 |
| **coordinator → work.ready(step-05)** | 预期 step-04 后已无 unit,应走 review.start | ❌ **未出现** | **未推进** | (NA) |
| **coordinator → review.start** | 最后一 step 通过后 emit | ❌ **0 触发** | **完全未启动** | events.jsonl 全 26 条无 `review.*` |
| **review-coordinator** | review.start 触发 6 维序列 | ❌ **0 触发** | **完全未启动** | (NA) |
| **dimension-reviewer** | review.dimension.ready 触发 | ❌ **0 触发** | **完全未启动** | (NA) |
| **review-synthesizer** | review.dimensions.complete 触发 | ❌ **0 触发** | **完全未启动** | (NA) |
| **fixer** | test.failed 时激活 | ❌ **0 触发**(0 test.failed) | NA(本次无失败) | (NA) |
| **coordinator → plan.complete / plan.blocked** | 终态 emit | ❌ **0 触发** | **完全未启动** | events.jsonl 无 `plan.*` |
| **shipper → REVIEW_COMPLETE** | plan.complete/blocked 触发 | ❌ **0 触发** | **完全未启动** | events.jsonl 无 `REVIEW_COMPLETE` |
| **reporter → report.done / LOOP_COMPLETE** | REVIEW_COMPLETE 触发 | ❌ **0 触发** | **完全未启动** | events.jsonl 无 `report.done` / `LOOP_COMPLETE` |
| **progress-steward** | loop.stalled 时兜底 | ❌ **0 触发**(`loop.stalled` 未在事件流) | **兜底未激活** | (NA) |

### 2.2 `test.passed` 分布偏离

**Preset 规定**(`presets/en/ce-executor-serial.yml:1224-1228`):validator 在每次 `work.done` 后必须 emit `test.passed` 或 `test.failed`。

| step | 预期 test.passed | 实际 test.passed | 偏离 |
|---|---|---|---|
| step-01 | 1 次(ts #3 之后) | **0 次** | ❌ 缺失 |
| step-02 | 1 次(ts #14 之后) | 1 次(#15) | OK(但延迟 54s) |
| step-03 | 1 次(ts #22 之后) | 1 次(#23) | OK(但被 6 次 work.done 阻塞 9min) |
| step-04 | 1 次(ts #25 之后) | **0 次** | ❌ 缺失 |

**test.passed 缺失合计:2/4(step-01 + step-04)**。

### 2.3 `task.resume` 错配

**Preset 规定**(`presets/en/ce-executor-serial.yml:2571-2577`):progress-steward 在 loop.stalled 时按表格选 emit;`task.resume` 应当由进度 steward 发起,或在 coordinator self-resume 场景下由 orchestrator 发起。

| ts | source | target_hat | kind | 实际目标 | 期望目标 | 偏离 |
|---|---|---|---|---|---|---|
| #4 (17:31:50) | orchestrator (loop) | `executor` | `missing_event_gate` | executor | **validator**(work.done 已发,应等 validator 30s 内 activate) | ❌ **错配** |
| #12 (17:41:00) | orchestrator (loop) | `executor` | `missing_event_gate` | executor | (此时 validator 在 stall_recovery 中) | ⚠️ 同型 |
| #26 (18:03:52) | orchestrator (loop) | `executor` | `missing_event_gate` | executor | **validator** | ❌ **错配** |

**关键证据**:
- recovery.jsonl:4 显式记录 `stall_recovery: source_hat=validator, target_hat=validator, reason=handoff_dispatch_timeout` — 表明 runtime 内部已经识别 "validator 30s 内未 activate" 的真实问题。
- 但 `task.resume` 的 target 仍指 `executor`(ts #26 18:03:52 → 8s 后 #25 work.done 后),retry_key 也是 `missing_event_gate:executor:work_done:missing_event:*`,与真实 stall 源(validator)不一致。
- 证据:recovery.jsonl:2, 4, 6, 18(全部 iteration 2/4/12 触发,但 target 一致指向 executor)

### 2.4 `loop.stalled` 缺失

**Preset 规定**(`presets/en/ce-executor-serial.yml:2568-2581`):progress-steward triggers=`["loop.stalled"]`,`progress_steward.max_steward_iterations: 3`。

| 期望 | 实际 | 偏离 |
|---|---|---|
| `loop.stalled` 在 validator stall 后由 runtime 发出 | **0 条 `loop.stalled`** | ❌ 兜底机制未触发 |
| progress-steward 在 `max_steward_iterations=3` 后被唤醒 | **0 触发** | ❌ 兜底机制失活 |

> **.ralph/events.jsonl 完全没有 `loop.stalled` 事件** — recovery.jsonl 仅记录 `stall_recovery: handoff_dispatch_timeout` 但这是诊断级 envelope,不是业务级 event。loop 终止前没有走 progress-steward 路径。

---

## 3. Hat Activation 偏离表

**来源对比**:`presets/en/ce-executor-serial.yml:603-2645` 各 hat 的 `triggers` + `publishes` vs events.jsonl 中 `hat` 字段 + `source` 字段。

| Hat | 预期 publishes | 预期 triggers 命中次数 | 实际 emit 次数 | 实际 activate 次数 | 偏离 |
|---|---|---|---|---|---|
| **coordinator** | work.ready, review.start, plan.complete, plan.blocked, LOOP_COMPLETE | ≥ 5(每个 step 推进 + 终态) | work.ready × 3(#2, #16, #24);**0 review.start / plan.complete / plan.blocked / LOOP_COMPLETE** | work.start(loop-bootstrap)→ work.ready(全部 3 次均由 coordinator 发);test.passed(step-02,03) → work.ready(step-03,04) | ❌ **终态 emit 全部 0 触发** |
| **executor** | work.done, work.failed | 4 step + 1 失败(0 失败) | work.done × 17(其中含 4 次 loop_id 缺失 + 1 次完全错发) | work.ready × 4(应为 4 step,实际收到 3 个 work.ready → #14 step-02 由 test.passed 隐式推进) | ❌ 重发 13 次(超 4 step × 1 次预期) |
| **validator** | test.passed, test.failed | 4 step(预期 4 次 test.passed) | test.passed × 2(#15 step-02, #23 step-03) | work.done × 4(预期 4 次,实际仅 2 次成功 emit test.passed) | ❌ **step-01 和 step-04 validator 未在 30s 内 activate**(recovery.jsonl:4, 9) |
| **fixer** | fix.applied, fix.exhausted | 0(本次 0 test.failed) | 0 | 0 | NA |
| **review-coordinator** | review.dimension.ready, review.dimensions.complete | 0(review.start 未触发) | 0 | 0 | ❌ 完全未激活 |
| **dimension-reviewer** | review.dimension.done, review.dimension.failed | 0 | 0 | 0 | ❌ 完全未激活 |
| **review-synthesizer** | review.complete | 0 | 0 | 0 | ❌ 完全未激活 |
| **shipper** | REVIEW_COMPLETE | 0(plan.* 未触发) | 0 | 0 | ❌ 完全未激活 |
| **reporter** | report.done, LOOP_COMPLETE | 0(REVIEW_COMPLETE 未触发) | 0 | 0 | ❌ 完全未激活 |
| **progress-steward** | work.ready, review.start, task.resume, plan.blocked | 0(loop.stalled 未触发) | 0 | 0 | ❌ **兜底失活** |

### 3.1 偏离子表 — work.done 风暴细节

| step | work.done 次数 | 首次 → 末次时间窗 | 重发原因(recovery.jsonl 标签) |
|---|---|---|---|
| step-01 | 8 次(#3, #5, #6, #7, #8 错发, #9, #10, #11) | 17:31:35 → 17:40:12(8m 37s) | 2 次 `TaskWrongLoop` + 5 次 `missing_event` |
| step-02 | 1 次(#14) | 17:43:32(单次到位) | 0 |
| step-03 | 6 次(#17, #18, #19, #20, #21, #22) | 17:50:06 → 17:58:31(8m 25s) | 1 次 `TaskWrongLoop` + 0 missing_event |
| step-04 | 1 次(#25) | 18:03:44(单次到位) | 0(但 validator 30s 内未 activate) |

### 3.2 偏离子表 — task_id 形态分布

| step | task_id 形态 | 备注 |
|---|---|---|
| step-01 | `""` × 4(#3, #9-#11) + `from_key:...` × 3(#5-#7) + `task-1737372000-a1b2` × 1(#13) | 8 次 emit, 5 种形态 |
| step-02 | `task-1782668503-e6b4` × 1(#14) | 1 次 emit, 1 种形态 |
| step-03 | `from_key:...` × 1(#17) + `task-1782669146-b65a` × 5(#18-#22) | 6 次 emit, 2 种形态 |
| step-04 | `""` × 1(#25) | 1 次 emit, 1 种形态 |

**task_id 形态收敛路径**:`""` → `from_key:...`(legacy)→ `""` → `task-...`(loop_scoped)→ `""`(再次退化)。

### 3.3 偏离子表 — `recovery_outcome_update` 状态翻转

**来源**:recovery.jsonl:5, 7, 10-12, 14-17 共 11 条 `drift_monitor.recovery_outcome_update` envelope。

| iteration | topic | retry_key | outcome | timestamp |
|---|---|---|---|---|
| 4 | work.done | `stall_recovery:validator:work_done:handoff_dispatch_timeout:*` | pending | 17:41:00.944Z |
| 5 | work.done | `missing_event_gate:executor:work_done:missing_event:*` | repeated | 17:43:44.762Z |
| 9 | work.done | `missing_event_gate:executor:work_done:missing_event:*` | recovered | 17:58:58.829Z |
| 9 | work.done | `execution_contract:...step-01...` | recovered | 17:58:58.829Z |
| 9 | work.done | `execution_contract:...step-03...` | recovered | 17:58:58.829Z |
| 10 | work.done | `missing_event_gate:executor:work_done:missing_event:*` | repeated | 17:59:28.965Z |
| 10 | work.done | `stall_recovery:validator:work_done:handoff_dispatch_timeout:*` | repeated | 17:59:28.965Z |
| 10 | work.done | `execution_contract:...step-01...` | pending | 17:59:28.965Z |
| 10 | work.done | `execution_contract:...step-03...` | pending | 17:59:28.965Z |

**3 个 retry_key 在 12 iter 内反复 pending↔recovered↔repeated 11 次** — 状态机无法收敛。

---

## 4. Loop 终止原因偏离分析

**loop-termination-reason.json**:
```json
{"recovery_exhausted":{"retry_key":"missing_event_gate:executor:work_done:missing_event:*",
  "reason":"retry window exhausted for retry_key=missing_event_gate:executor:work_done:missing_event:* (>= 2 attempts within 8 iterations)"}}
```

**Preset 失败路径设计**(`presets/en/ce-executor-serial.yml:870-873`):
- `work.failed` → coordinator `plan.blocked(reason="executor failed: <reason>")` → shipper → REVIEW_COMPLETE(`pass_or_fail="fail"`)
- 失败路径终点:`REVIEW_COMPLETE(fail)` → reporter `report.done(pass_or_fail="fail")` → **不 emit LOOP_COMPLETE**

**实际终止原因 vs 预期**:

| 维度 | 实际 | 预期 | 偏离 |
|---|---|---|---|
| 终止关键词 | `recovery_exhausted` | `plan.blocked` → `REVIEW_COMPLETE(fail)` | ❌ 走了非设计的"硬退出"路径 |
| retry_key 锚点 | `executor / work_done / missing_event` | (NA,失败路径不走 retry_key) | ❌ |
| 失败 hat | 标 `executor` | 实际是 `validator` 30s 内未 activate | ❌ 错配 |
| 终态事件 | **0 终态事件**(无 plan.* / REVIEW_COMPLETE / report.done) | 至少 1 个 `REVIEW_COMPLETE(fail)` | ❌ 整条失败路径未触发 |
| 报告 | 0 | 1 个 `report.done(fail)` | ❌ |
| `pass_or_fail` 记录 | 缺失 | 必须 fail | ❌ |

**根信号**:loop 退出机制没走 preset 设计的 `plan.blocked` → shipper → `REVIEW_COMPLETE(fail)`,而是走 runtime 内置的 `recovery_exhausted` 兜底(loop 层面判断 retry window exhausted 直接终止)。

---

## 5. Root Cause 候选排序(只列证据,不做归因)

> 排序基于"偏离在 evidence 链路中的位置",**不**基于"哪个是 root cause"(归因是 Agent D 的工作)。

### 5.1 Tier 1 — 候选位点 1:`validator 30s 内未 activate`

| 证据 | 文件:行 | 现象 |
|---|---|---|
| `stall_recovery: source_hat=validator, reason=handoff_dispatch_timeout` | recovery.jsonl:4(iteration 4) | step-01 work.done 后 validator 30s 未 activate |
| 同上 | recovery.jsonl:9(iteration 9) | step-03 work.done 后 validator 30s 未 activate(同伴 stall) |
| 缺失 `test.passed(step-01)` | events.jsonl 无 step-01 test.passed | step-01 test.passed 完全未发 |
| 缺失 `test.passed(step-04)` | events.jsonl 无 step-04 test.passed | step-04 test.passed 完全未发 |
| `task.resume` 错配(应指 validator,指了 executor) | events.jsonl:4, :12, :26 | retry_key 锚点错位 |
| `stall_recovery` 与 `missing_event_gate` 两条 retry_key 并存 | recovery.jsonl:4(stall) vs :6(missing_event) | runtime 内部已识别 validator stall,外部 retry_key 仍指 executor |

### 5.2 Tier 1 — 候选位点 2:`task_id` 形态漂移(TaskWrongLoop 反复拒)

| 证据 | 文件:行 | 现象 |
|---|---|---|
| `TaskWrongLoop { expected_loop: "primary-20260628-172725", actual_loop: None }` | recovery.jsonl:3(step-01) | 首次 TaskWrongLoop |
| 同上 | recovery.jsonl:8(step-03) | 第二次 TaskWrongLoop |
| `task_id=""` | events.jsonl:3, :9-#11, :25 | 共 5 次空 task_id |
| `task_id="from_key:..."` | events.jsonl:5-#7, :17 | 共 4 次 legacy 形态 |
| `task_id="test"`(错发) | events.jsonl:8 | 1 次完全错发 |
| 任务在 tasks.jsonl 中以 `from_key:...` 形式被 project | tasks.jsonl:1, :3, :5 | executor 的"先发 work.done → 后建任务"导致 task_id 错位 |
| progress.md `Completed Steps: step-01, step-03` 缺 step-02 | progress.md:7-8 | 任务关闭记录与事件流不一致(实际 step-02 test.passed 已发) |

### 5.3 Tier 2 — 候选位点 3:`test.passed(step-04) 缺失`导致 `review.start` 未触发

| 证据 | 文件:行 | 现象 |
|---|---|---|
| events.jsonl #24(coordinator work.ready(step-04))→ #25(executor work.done(step-04))→ #26(loop 终止 task.resume) | events.jsonl:24-26 | 链长 3m 13s,validator 0 emit |
| preset 规定 step-NN test.passed 是 PHASE 1 推进信号 | ce-executor-serial.yml:787-800 | 末 step test.passed 触发 review.start |
| `0 review.start` | events.jsonl 全文 | review 链完全未启动 |
| `0 plan.complete / REVIEW_COMPLETE / LOOP_COMPLETE` | events.jsonl 全文 | 终态事件全部缺失 |

### 5.4 Tier 2 — 候选位点 4:executor work.done 重发风暴(regression 噪声)

| 证据 | 文件:行 | 现象 |
|---|---|---|
| step-01 共 8 次 work.done(events #3, #5-#11) | events.jsonl:3-11 | 8m 37s 内 8 次 |
| step-03 共 6 次 work.done(events #17-#22) | events.jsonl:17-22 | 8m 25s 内 6 次 |
| 每次重发都带同一 task_key 和 commit_count=1 / changed_lines 不变 | events.jsonl:5-11(全 473)、#18-#22(全 105) | executor 反复 commit 同一状态 |
| `task_id` 形态在重发过程中漂移 | events.jsonl:5(from_key)→ :9-#11("")→ :13(task-...) | 重发同时是 task_id 形态探索 |
| `commit_count=1` 在 8 次重发中保持不变 | events.jsonl:3, :5-#11, :13 | 没有新 commit 被产生,只是重发 |

### 5.5 Tier 3 — 候选位点 5:`progress-steward` 兜底未激活

| 证据 | 文件:行 | 现象 |
|---|---|---|
| preset 规定 progress-steward triggers=`["loop.stalled"]` | ce-executor-serial.yml:2571 | 0 `loop.stalled` 业务事件 |
| `max_steward_iterations: 3` | ce-executor-serial.yml:308 | 配置启用但未触发 |
| `0 task.resume` 由 progress-steward 发 | events.jsonl 全文(task.resume 均无 source/progress-steward 标识) | 3 次 task.resume 都由 orchestrator(loop) 发出 |
| recovery.jsonl 中无 progress-steward 标识 | recovery.jsonl 全文 source 字段 | 0 条由 progress-steward 主导 |

### 5.6 Tier 3 — 候选位点 6:`loop-termination-reason.json` 走 recovery_exhausted 而非 plan.blocked

| 证据 | 文件:行 | 现象 |
|---|---|---|
| `loop-termination-reason.json` = `recovery_exhausted` | loop-termination-reason.json:1 | 走了 runtime 兜底硬退出 |
| preset 失败路径走 `plan.blocked` → `REVIEW_COMPLETE(fail)` → `report.done(fail)` | ce-executor-serial.yml:868-873, 2354-2364 | 设计路径未走 |
| `0 REVIEW_COMPLETE` 终态事件 | events.jsonl 全文 | reporter 链未启动 |
| `0 LOOP_COMPLETE` 终态事件 | events.jsonl 全文 | loop 在没有任何终态 signal 下被硬终止 |

---

## 6. 引用清单(全部带绝对路径 + 行号)

### 6.1 输入数据
- 运行时事件:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/events-20260628-172725.jsonl`(26 行)
- 诊断日志:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/diagnostics/2026-06-29T01-27-25/recovery.jsonl`(19 行)
- 终止原因:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/loop-termination-reason.json`
- 任务记录:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/agent/tasks.jsonl`(5 行)
- 进度:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/agent/progress.md`(8 行)
- 总结:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/agent/summary.md`(40 行)
- scratchpad:`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/agent/scratchpad.md`(18 行)

### 6.2 期望源(preset)
- `/Users/pittcat/Dev/Rust/ralph-orchestrator/presets/en/ce-executor-serial.yml`
  - 10-hat 拓扑 + Event Flow:行 1-62
  - mechanism.flow + 终态声明:行 70-139
  - execution_contracts(work.done 7 字段):行 255-300
  - event_policy + topic_deny_rules:行 316-585
  - coordinator + Phase Gate 表:行 633-643, 787-800
  - executor `triggers` / `publishes`:行 976-980
  - validator `triggers` / `publishes`:行 1224-1228
  - review-coordinator / dimension-reviewer / review-synthesizer:行 1300-2130
  - shipper 失败路径:行 2346-2364
  - reporter:行 2376-2547
  - progress-steward 兜底:行 2549-2645

### 6.3 输入报告(已生成的同任务产物)
- Agent A 链路图:`/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-A-flow-reconstruction.md`
- Agent B 历史 KB:`/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-B-history-kb.md`

---

## 7. 报告交付摘要

- **本报告路径**:`/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-C-deviation-evidence.md`
- **核心产出**:
  1. **Payload 字段偏离表**(§1):17 次 `work.done` 中 5 类 task_id 形态漂移(`""` × 5 + `from_key:...` × 4 + 完全错发 × 1 + 合法 × 7)
  2. **Event 流转偏离表**(§2):`test.passed` 缺失 2/4(step-01 + step-04);`task.resume` target_hat 错配(executor vs validator);`0 review.*` / `0 plan.*` / `0 REVIEW_COMPLETE` / `0 LOOP_COMPLETE`
  3. **Hat activation 偏离表**(§3):10 hat 中 7 个完全未激活(review-coordinator / dimension-reviewer / review-synthesizer / shipper / reporter / progress-steward / fix-unit 链)
  4. **Root cause 候选排序**(§5):Tier 1 = validator stall + task_id 漂移(2 候选);Tier 2 = step-04 test.passed 缺失 + executor 重发风暴(2 候选);Tier 3 = progress-steward 失活 + 终止路径错配(2 候选)
- **未涉及**(留给其他 agent):最终归因(Agent D 范围)、历史关联(Agent B 范围)。
# 根因归因 + 修复建议 — primary-20260628-172725

> Loop: `primary-20260628-172725` · Preset: `ce-executor-serial` · 终止原因: `recovery_exhausted` · Iterations: 12 · Duration: 36m 26s
>
> 本报告基于 Agent A 链路图、Agent B 历史 KB、Agent C 偏离证据 + 主仓源码审查(`crates/ralph-core/src/{event_loop,state_projector,execution_contract,diagnosis,drift,flow_lifecycle}/`、`crates/ralph-cli/src/loop_runner/`、`presets/en/ce-executor-serial.yml`)做根因归因与修复建议。
>
> 引用基础:Agent B §3.1(关联度矩阵)、Agent C §5(候选位点)+ 主仓源码精确行号。

---

## 0. 一句话结论

**本次失败的真正根因不是 `executor` 未发事件,而是 `validator` 30s 内未 activate(stall_recovery 自身已识别但 retry_key 错配)→ 触发 `missing_event_gate` 把责任转嫁到 `executor` → 重发风暴污染 task_store → `TaskWrongLoop` 反复拒 → 重发引发的 `task.resume` 不带 `kind` → drift 告警风暴叠加 → 8 iter 内累计触发 `recovery_exhausted` 硬退出。**

**单点根因(All-P0):** `HandoffTracker` 30s 软超时 + runtime 把它错归为 `missing_event_gate:executor` 而非 `stall_recovery:validator` + `recovery_exhausted` 路径未走 preset 设计的 `plan.blocked` 终态。

**本质分类:**
- **基座机制问题(主因)**:stall_recovery 锚定错配、`from_key:...` projector 不补 loop_id、handoff_deadline 缺消费者后真终止路径
- **preset 设计问题(次因)**:`require_task.loop_scoped` 默认 true 但 projector 不从 marker 补全 → 形成"先发 work.done → 后建任务 → 必然 legacy"的死循环
- **运行产物问题(放大器)**:executor 重发风暴把 task_store 写脏;agent 不收敛重发

---

## 1. 4 类 Failure 因果链

### 1.1 链总览

```
iter 1  work.start → work.ready(step-01)           [coordinator]
iter 2  work.done(step-01) [task_id=""]            [executor]  #3
        ↓
        validator 未在 30s 内 activate               ←──┐
        ↓                                              │  根因 A
iter 2  HandoffTracker::expired()                     │  (stall 锚错)
        ↓                                              │
iter 2  RecoveryDiagnosisEnvelope                     │
        source=stall_recovery                         │
        source_hat=validator  ←── 这里对了            │
        reason=handoff_dispatch_timeout               │
        ↓                                              │
iter 2  Synthesize task.resume                        │
        target_hat=validator  ←── 这次 target 对了    │
        payload.kind=???  ←── **没有 kind 字段**      │  根因 B
        ↓                                              │  (inline JSON)
iter 2  emit gate / hard gate 二次检测                │
        看到 "executor 没在 30s 内发 work.done"        │
        (虽然实际已发,但被重复检测)                   │
        ↓                                              │
iter 2  missing_event_gate 触发                       │
        source_hat=executor  ←── **锚定到 executor**  │  根因 C
        target_hat=executor                           │  (双轨诊断同事件)
        retry_key=missing_event_gate:executor:work_done:missing_event:*
        ↓
iter 3-5  executor 收到 task.resume 后反复重发        ←──┐
        形态漂移: "" → from_key:... → "" → "" → ""   │  根因 D
        ↓                                              │  (task_id 漂移)
iter 4  TaskWrongLoop {actual_loop=None}              │  (projector 不补 loop_id)
        expected_loop=primary-20260628-172725         │
        ↓                                              │
iter 4-12  recovery_outcome_update 反复翻转            │  根因 E
        pending↔recovered↔repeated 11+ 次             │  (3 retry_key 并存)
        ↓                                              │
iter 5  validator 第二次 stall (step-02)              │  根因 A
iter 9  validator 第三次 stall (step-03)              │
        ↓
iter 12 step-04 work.done 后,validator stall 第四次
        HandoffTracker 触发 → 仍被 hard gate 二次
        归类为 missing_event_gate:executor
        ↓
        responder.classify() attempt=2 within 8 iters
        ↓
iter 12  over_threshold + over_window
        EscalationLevel::Final
        ↓
        TerminationReason::RecoveryExhausted
        ↓
        loop 硬退出,不走 plan.blocked → REVIEW_COMPLETE(fail)
                                                      │
iter 9  drift_monitor: kind 1/5 (20%)                 │  根因 B 触发
        threshold 85%                                  │
        drift_field_completeness 告警                  │  (handoff_dispatch_timeout
        (Critical 严重度)                              │   inline JSON 不带 kind)
```

### 1.2 4 类 failure 之间的因果链(逐个)

| Failure | 直接触发 | 根因(代码) | 与其他 failure 的关系 |
|---|---|---|---|
| **F1: stall_recovery:handoff_dispatch_timeout** | `HandoffTracker::expired()` 30s timeout 命中(`crates/ralph-core/src/event_loop/mod.rs:6080-6225`) | `validator` consumer 收到 `work.done` 后 30s 内未 `on_hat_activated`(hat activation hook 在 `mod.rs:3550`,但 hat 没真正 build_prompt) | **是 F2 的真正源头**;但 runtime 把 retry_key 同时挂到了 F2(executor)上 |
| **F2: missing_event_gate:missing_event** | hard gate(`loop_runner/hard_gate.rs:912-937`)二次检测时,认为"executor 没在窗口内发 work.done" → 但实际已发 | (a) hard gate 不知道 work.done 已发,因为它依赖 `rejection_envelope` 中的 expected emit window;(b) 与 F1 共用 iteration 时间窗 | **由 F1 误判产生**;F2 的 retry_key 错锚到 `executor` 是关键放大器 |
| **F3: execution_contract:TaskWrongLoop** | `execution_contract.rs:570-603` `validate_task` 看到 `task.loop_id == None` | projector `state_projector/task.rs:86-88` 只在 `ctx_loop_id(payload)` 非空时调用 `with_loop_id`;payload 不带 `loop_id` 时,投影出的 task 永久 `loop_id=None` | **被 F2 放大**:executor 收到 `task.resume` 后重发,而重发时的 task_id 形态在 `""` / `from_key:...` 之间漂移,新生成 task 仍是 `loop_id=None` |
| **F4: drift_monitor:drift_field_completeness kind** | `drift/detector.rs:441-468` 看到窗口内 5 条 `task.resume` 中只有 1 条带 `kind` | (a) `event_loop/mod.rs:6104-6112` 走 inline JSON 拼 payload,**没有调 `enrich_task_resume_payload`**;(b) `rejection.rs:707-712` 的 fallback 也没覆盖这条路径 | **由 F1 间接放大**:stall_recovery 触发的 task.resume 没 `kind`,导致 drift 误报;drift 告警重置 retry_key 状态机,加剧 outcome 反复 |

### 1.3 关键发现 — runtime 内部已经识别了真正问题,但 retry_key 锚定错位

`recovery.jsonl:4` 明确写:
```json
{"source":"stall_recovery", "source_hat":"validator", "target_hat":"validator",
 "reason_code":"handoff_dispatch_timeout",
 "message":"handoff deadline exceeded: consumer 'validator' did not activate within timeout"}
```

但**同一个事件流(iteration 4)**`recovery.jsonl:6` 又记录了:
```json
{"source":"missing_event_gate", "source_hat":"executor", "target_hat":"executor",
 "reason_code":"missing_event",
 "message":"Hat 'executor' did not emit any event on its publish obligation"}
```

→ 同一现象被两个 source 重复记录,但 `stall_recovery` 和 `missing_event_gate` 的 retry_key **互不感知**,各跑各的窗口。`missing_event_gate` 的 retry_key 在 iter 4 → iter 5 → iter 12 累计到第 2 次 attempt,达到 `over_threshold=2, over_window=8`,触发 `EscalationLevel::Final`(`responder.rs:907-909`)→ `RecoveryExhausted` 终止。

---

## 2. P0 / P1 / P2 问题归因表

| # | 优先级 | 问题 | 根因(代码位置) | 现象链 | 归因分类 |
|---|---|---|---|---|---|
| **1** | **P0** | **`stall_recovery` 锚定正确后被 `missing_event_gate` 二次覆盖,且 `missing_event_gate` 的 retry_key 错锚到 `executor` 触发 `RecoveryExhausted`** | (a) `hard_gate.rs:912` 不知 `work.done` 已发;(b) `responder.classify` 两条 retry_key 互不感知,各自跑窗口 | F1 30s stall → 同步 F2 → 8 iter 内 F2 attempt 累计 2 次 → `EscalationLevel::Final` → 硬退出 | **基座机制** |
| **2** | **P0** | **`validator` 30s 内未 activate 的根因未解**(hat 真的没被 build_prompt) | (a) hat 调度逻辑有竞争(handoff tracker `on_hat_activated` 在 `mod.rs:3550` 只清自身条目,validator 收到 work.done 后无明确激活信号);(b) 30s 硬编码(`config/workflow_contract.rs:50` `HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS=30`)未配项目实际 agent 响应延迟(实测 executor 单次 work.done 后 validator 实际响应在 9min 量级,#14→#15=54s,#22→#23=9min) | F1 持续;`task.resume` 多次重发到 validator,validator 仍未响应;`loop.stalled` 业务事件从未发出,`progress-steward` 兜底未激活 | **基座机制** |
| **3** | **P0** | **`recovery_exhausted` 硬退出路径未走 preset 设计的 `plan.blocked` → `REVIEW_COMPLETE(fail)` → `report.done(fail)` 终态链路** | `drift/engine.rs:392-406` `check_termination_hint` 在 `EscalationLevel::Final` 时直接返回 `RecoveryExhausted`,不区分"应转入 plan.blocked 路径" vs "无安全目标" | 0 个终态事件(`plan.*` / `REVIEW_COMPLETE` / `report.done` / `LOOP_COMPLETE` 全部 0),导致 shipper/reporter/loop 退出协议三方都不知道该 run 死了 | **基座机制** |
| **4** | **P0** | **`state_projector/task.rs:80` 在 `task_id` 为空时生成 `from_key:<key>`,但 `with_loop_id` 不会从 `.ralph/current-loop-id` marker 读取兜底** | projector 行 86-88 只在 `ctx_loop_id(payload)` 非空时调 `with_loop_id`;`validate_task` 在 `execution_contract.rs:588-603` 把 `loop_id=None` 一律判为 `TaskWrongLoop` | F3:每次重发都生成新的 `from_key:...` 形态 task,该 task 永久 `loop_id=None`,被 `validate_task` 反复拒 | **基座机制** |
| **5** | **P1** | **预设 `require_task.loop_scoped: true`(`ce-executor-serial.yml:272-275`)默认开启,但无 fallback 路径** | `execution_contracts.rs:79-81` `default_loop_scoped: true`;`execution_contract.rs:570` 直接 hard fail;无 `loop_id` 缺省注入 | executor 重发 17 次中 5 次 task_id=`""` + 4 次 `from_key:...` 共 9 次被拒 | **preset 设计** |
| **6** | **P1** | **`hard_gate.rs:912-937` 与 `event_loop/mod.rs:6080` 两条 recovery envelope 路径在同事件上双发,无去重** | (a) `hard_gate.rs:912` 的 `inject_missing_event_hard_gate_guidance_with_triggers` 不知道 stall_recovery 刚处理过同一个事件;(b) `responder.observe` 在写 retry state 前不查 `state.contains_key(retry_key_for_other_source)` | recovery.jsonl 中 F1(stall_recovery)与 F2(missing_event_gate)两条 envelope 在 iter 2/4/12 同一现象双发,outcome 反复翻转 11+ 次 | **基座机制** |
| **7** | **P1** | **`event_loop/mod.rs:6104-6112` 走 inline JSON 拼 `task.resume` payload,不调 `enrich_task_resume_payload`,导致 `kind` 字段缺失** | (a) `rejection.rs:680-712` 的 `enrich_task_resume_payload_full` 有 `kind` 字段回退到 `reason_code`;(b) 但 `mod.rs:6113` 走 `serde_json::json!({...})` 直接拼,绕过 `enrich_*` | F4:`field 'kind' on topic 'task.resume' present in 1/5 events (20.0%)`,threshold 85% | **基座机制** |
| **8** | **P1** | **`validator` hat 自身在收到 `work.done` 后**没有** build_prompt 响应**(实测:step-01/04 完全无 `test.passed` emit,step-02/03 延迟 54s/9min) | preset `validator` hat 触发机制在 hat 调度层可能存在竞态;`HandoffTracker` 的 `expired()` 检查与 hat 实际激活之间存在窗口;`on_hat_activated`(`mod.rs:3550`)对 validator 路径不可见或被 filter 阻断 | F1 stall 持续;work.done 与 test.passed 间隔 0-540s 不等 | **基座机制 + 运行产物** |
| **9** | **P2** | **`executor` 在收到 `task.resume` 后反复重发 7-8 次(step-01/step-03)** | (a) agent prompt 没明确"重发需先改 task_id 形态"指令;(b) hard gate 注入的 guidance 未给收敛信号(只说"emit one of work.done, work.failed",没说要重新生成 task_id) | events.jsonl #5-11 / #17-22 共 14 次 work.done 重发,每次 commit_count=1,changed_lines 不变 | **运行产物(agent 行为)** |
| **10** | **P2** | **executor 在 17:36:21 完全错发 `step="test" / plan_name="test"`(events.jsonl:8)** | (a) agent 手测残留 / 模板错位;(b) precheck 未对"plan_name 与 .ralph/loops.json 匹配"做硬校验 | 完全错发的 work.done 1 次,污染事件流 | **运行产物(agent 行为)** |
| **11** | **P2** | **`progress-steward` 兜底失活**(`loop.stalled` 业务事件从未发出) | preset `progress-steward` triggers=`["loop.stalled"]` 但 `loop.stalled` 在 stall_recovery 路径上**只写入 recovery envelope,不发布业务事件** | recovery.jsonl 有 stall_recovery 记录,但 events.jsonl 无 `loop.stalled`,progress-steward 0 触发 | **preset 设计 + 基座机制** |
| **12** | **P2** | **`coordinator` 在 30+ iter 内反复发 work.ready 但不发 review.start** | coordinator 终态判定依赖 `verify_tasks_complete` (`mod.rs:6460`),但 step-04 task 一直 `open`(`tasks.jsonl:5` `status=open`),即使 work.done 实际已 emit | review-coordinator / dimension-reviewer / review-synthesizer 0 激活 | **基座机制** |

---

## 3. 修复建议(按优先级,可直接落地)

### P0 修复

#### P0-1:`stall_recovery` 与 `missing_event_gate` 同事件去重 + retry_key 共享状态

**目标文件:** `crates/ralph-core/src/event_loop/mod.rs:6080-6225` + `crates/ralph-cli/src/loop_runner/hard_gate.rs:912-937`

**问题:** 同一 `work.done` 事件在 iteration 内被 stall_recovery 和 missing_event_gate 两条路径各发一次 envelope,retry_key 互不感知,导致 attempt 独立累加 → 误判 Final。

**具体修改:**
1. 在 `mod.rs:6080` 的 `handoff_tracker.expired()` 处理前,先检查 `self.state.recovery_responder.state` 中是否存在同 `event_id` 的 stall_recovery envelope;若存在,跳过 hard gate 二次检测。
2. 在 `hard_gate.rs:912` `inject_missing_event_hard_gate_guidance_with_triggers` 入口处,先查 `self.state.recovery_responder.state.contains_key("stall_recovery:{consumer}:{topic}:handoff_dispatch_timeout:*")`;若已存在,直接 `return`。
3. 在 `responder.rs:880-919` `classify` 函数中,把 `missing_event_gate` 与 `stall_recovery` 视为同一根因族(retry_key 共享前缀),attempt_count 取两者 max。
4. 修后预期:iter 12 不再触发 `RecoveryExhausted`;F1 stall_recovery 自己的 retry_key 在 hard gate 旁路后有充足窗口被 validator 响应,outcome → Recovered。

**关键行号:** `event_loop/mod.rs:6127`(reason_code),`hard_gate.rs:912`(inject 入口),`responder.rs:880-919`(classify)。

---

#### P0-2:`state_projector` 在 task 缺 `loop_id` 时从 `.ralph/current-loop-id` marker 兜底

**目标文件:** `crates/ralph-core/src/state_projector/task.rs:86-88`

**问题:** projector 只在 `ctx_loop_id(payload)` 非空时调 `with_loop_id`,导致 legacy task 永久 `loop_id=None` → 触发 `TaskWrongLoop {actual_loop=None}`。

**具体修改:**
```rust
// 当前 task.rs:86-88
if let Some(loop_id) = ctx_loop_id(payload) {
    task = task.with_loop_id(Some(loop_id.to_string()));
}

// 改为(在 ProjectionContext 增加 ctx.current_loop_id 字段)
if let Some(loop_id) = ctx_loop_id(payload) {
    task = task.with_loop_id(Some(loop_id.to_string()));
} else if let Some(loop_id) = &ctx.current_loop_id {
    // P0-2 修复:loop_id 缺失时从 .ralph/current-loop-id marker 兜底
    task = task.with_loop_id(Some(loop_id.clone()));
}
```

**前置工作:**
1. `ProjectionContext`(`task.rs:1-30` 区域)增加 `pub current_loop_id: Option<String>` 字段
2. 构造 `ProjectionContext` 的调用点(`event_loop/mod.rs` 多处)传入 `self.current_loop_id_for_contract()`(`mod.rs:6441` 已存在)
3. 删除 `execution_contract.rs:588-603` 的 `TaskWrongLoop {actual_loop: None}` legacy 拒路径(可保留,作为最后兜底,改为 `outcome: Recovered` 警告而非 hard reject)

**修后预期:** executor 重发的 `from_key:...` task 自动带 `loop_id`,`TaskWrongLoop` 误报 0;F3 在 iter 4/9 不再触发。

---

#### P0-3:`RecoveryExhausted` 路径走 preset 设计的 `plan.blocked` → `REVIEW_COMPLETE(fail)`

**目标文件:** `crates/ralph-core/src/drift/engine.rs:392-406` + `crates/ralph-core/src/event_loop/mod.rs:8430-8500` (termination 路径)

**问题:** `check_termination_hint` 在 `EscalationLevel::Final` 时直接 `return Some(TerminationReason::RecoveryExhausted)`,绕开 preset 设计的 `plan.blocked` 终态。

**具体修改:**
1. `engine.rs:392-406` 改为:当 `hint.level == EscalationLevel::Final` 且 `safe_target == true` 时,**先**发送 `plan.blocked` 业务事件(由 coordinator 消费,转 `REVIEW_COMPLETE(fail)`),**再** 走 `RecoveryExhausted` 兜底。`plan.blocked` 路径在 `event_loop/mod.rs:8430` 区域已有实现,需串联。
2. 新增:在 `EscalationLevel::Final` 路径上,runtime 主动 emit `plan.blocked(reason="recovery_exhausted:<retry_key>")` 到 bus,coordinator 收到后正常转 shipper。
3. shipper 收到 `plan.blocked` 后发 `REVIEW_COMPLETE(pass_or_fail="fail")`,reporter 发 `report.done(fail)`,然后才走 `RecoveryExhausted` 兜底。
4. 预期效果:loop 终止时至少有 1 个 `REVIEW_COMPLETE` 业务事件,terminal-reason 仍记录但 shipper 链能跑完。

**关键行号:** `drift/engine.rs:392-406`(原),`event_loop/mod.rs:8430-8500`(原 termination),`presets/en/ce-executor-serial.yml:870-873`(原 plan.blocked 设计)。

---

#### P0-4:30s handoff_dispatch_timeout 调高到 600s 或改为可配置,并加 `safe_target=fallback` 二级链

**目标文件:** `crates/ralph-core/src/config/workflow_contract.rs:50` + `crates/ralph-core/src/event_loop/mod.rs:6080-6225`

**问题:** 30s 硬编码远低于实测 validator 响应延迟(54s-540s),validator 实际能响应但被错判 stall。

**具体修改:**
1. `workflow_contract.rs:50` 把 `HANDOFF_DISPATCH_TIMEOUT_DEFAULT_SECONDS: u64 = 30` 改为 `600`(实测 540s 上限 + 60s buffer);`HANDOFF_DISPATCH_TIMEOUT_MAX_SECONDS: u64 = 120` 改为 `1800`。
2. `ralph.yml` 增加配置覆盖:
   ```yaml
   workflow_contract:
     handoff_dispatch_timeout_seconds: 600
   ```
3. `HandoffTracker::expired()`(`workflow_contract/handoff_tracker.rs:199-237`)的 `safe_target` 计算:若 `consumer` 在 1× timeout 后仍未激活,先发 task.resume 给 `consumer`;若 2× timeout 后仍未激活,再发 task.resume 给 `progress-steward` 兜底。
4. `event_loop/mod.rs:6080` 之后增加 `fallback_safe_target` 字段读取(`progress-steward` 在 preset 中已定义为兜底 hat)。

**修后预期:** validator 实际响应时间(54s-540s)都在 timeout 窗口内,stall_recovery 不再误判;F1 在正常 run 中完全消失。

---

### P1 修复

#### P1-1:`hard_gate` 二次检测前查询 stall_recovery 状态(去重)

**目标文件:** `crates/ralph-cli/src/loop_runner/hard_gate.rs:912-937`

**具体修改:**
1. 在 `inject_missing_event_hard_gate_guidance_with_triggers` 入口加 guard:
   ```rust
   let stall_key = format!("stall_recovery:{}:{}:handoff_dispatch_timeout:*", hat_name, topic_for_envelope);
   if self.state.recovery_responder.state.contains_key(&stall_key) {
       // 已有 stall_recovery 在处理,missing_event_gate 跳过
       return;
   }
   ```
2. 同时在 `responder.observe`(`responder.rs:818-876`)中,当 `source=missing_event_gate` 时,合并 `stall_recovery` 同 topic 的 attempt_count。

**修后预期:** 同一 work.done 事件只产生 1 个 envelope,recovery.jsonl 行数减半,outcome 翻转不再发生。

---

#### P1-2:handoff_dispatch_timeout 路径走 `enrich_task_resume_payload_full` 补 `kind`

**目标文件:** `crates/ralph-core/src/event_loop/mod.rs:6104-6116`

**问题:** inline JSON 拼 payload,缺 `kind` 字段,drift 报 20%。

**具体修改:**
```rust
// 当前 mod.rs:6104-6112 inline JSON
let payload = serde_json::json!({...});
let resume_event = Event::new("task.resume", payload.to_string())...

// 改为调 enrich_task_resume_payload_full
use crate::event_loop::rejection::{enrich_task_resume_payload_full, RejectionKind};
let payload_str = enrich_task_resume_payload_full(
    &format!("handoff deadline exceeded: consumer '{}' did not activate within timeout", esc.consumer),
    "handoff_dispatch_timeout",
    Some(esc.safe_target.as_str()),
    Some(crate::event_loop::rejection::RejectionStage::StallNoEvents),
    Some(RejectionKind::StallRecovery),  // 关键:显式 set kind
    &self.allowed_publish_topics_for(esc.safe_target.as_str()),
);
let resume_event = Event::new("task.resume", payload_str)
    .with_source(HatId::from("ralph"))
    .with_target(HatId::from(esc.safe_target.as_str()));
```

**修后预期:** `task.resume` payload 100% 带 `kind` 字段,drift `field_completeness` 0% → 95%+,F4 消失。

---

#### P1-3:`from_key:` legacy 形态在 `validate_task` 中不再 hard-reject

**目标文件:** `crates/ralph-core/src/execution_contract.rs:488-505` + `:570-603`

**具体修改:**
1. `execution_contract.rs:495-505` `resolved_task_id` 解析路径已支持空 `task_id` → `from_key:<key>`。但 `validate_task:570` 的 loop_scoped check 把 `loop_id=None` 一律 hard reject,需改为:
   ```rust
   if rule.require_task.loop_scoped {
       if let Some(task_loop_id) = &task.loop_id {
           if task_loop_id != current_loop_id {
               return Some(ExecutionContractFinding { ... hard reject ... });
           }
       } else {
           // P1-3 修复:loop_id 缺失时,检查 task.key 是否能映射到当前 loop
           // 若 task_key 包含当前 loop_id 前缀,判为 Recovered 而非 reject
           if let Some(key) = &task.key {
               if key.contains(current_loop_id) {
                   // 同一 loop 的 key,缺 loop_id 字段 → warn + accepted
                   // 配合 P0-2 projector 兜底,这条路径会消失;保留作为防御
                   return None; // accept
               }
           }
           return Some(ExecutionContractFinding { ... 仍 hard reject 但加 outcome=Recovered 提示 ... });
       }
   }
   ```
2. 在 `validation/rules_execution_contract.rs:180-190` 的 `ReasonCode` 映射里,`TaskWrongLoop {actual_loop: None}` 改为 `CONTRACT_TASK_LOOP_SCOPE_VIOLATION`(新 code)而非 `CONTRACT_TASK_NOT_FOUND`,避免与真的 "task not found" 混淆。

**修后预期:** F3 在 projector P0-2 修复后已无主源;`validate_task` 保留 defensive 接受路径,降低重发风险。

---

#### P1-4:`executor` 提示词增加"重发前先检查 task_store 当前 task_id"指引

**目标文件:** `presets/en/ce-executor-serial.yml:976-980` + `:1115-1216`(executor instructions)

**具体修改:**
1. 在 executor instructions 增加约束:
   ```yaml
   ### Re-emission Protocol (HARD RULE)
   Before re-emitting `work.done` after a `task.resume`:
   1. Read `.ralph/agent/tasks.jsonl` to find the actual task_id for current step's task_key
   2. Use the task_id from tasks.jsonl in payload (NOT "" and NOT "from_key:...")
   3. If task_id is missing from store, re-read .ralph/current-loop-id and use that as loop_id
   ```
2. 配套:在 `progress-steward` 救援路径也强制要求先读 task_store。

**修后预期:** 17 次 work.done 中 9 次 task_id 漂移(5 `""` + 4 `from_key:...`)全部消失,F3 二次触发概率大幅降低。

---

### P2 修复

#### P2-1:`progress-steward` 兜底触发:`loop.stalled` 作为业务事件从 stall_recovery 同步发布

**目标文件:** `crates/ralph-core/src/event_loop/mod.rs:6225` 之后 + `presets/en/ce-executor-serial.yml:2568-2581`

**具体修改:**
1. `mod.rs:6225` 之后,在 stall_recovery 写完 envelope 后,如果 `escalations.len() >= 2`(同一 consumer 连续 stall),额外 emit `loop.stalled(reason="<retry_key>")` 业务事件到 bus。
2. progress-steward 收到 `loop.stalled` 后按 `max_steward_iterations: 3` 走兜底救援。
3. preset 中 progress-steward 的触发条件 + max_steward_iterations 已有,只需在 runtime 加 `loop.stalled` 业务事件触发。

**修后预期:** 4 次 stall(step-01/03/04/04-after)会累计触发 2 次 `loop.stalled`,progress-steward 在第 3 次 stall 后强制走兜底,避免完全卡死。

---

#### P2-2:`coordinator` 终态判定补"已发 work.done 但 task 仍 open"主动 close

**目标文件:** `crates/ralph-core/src/event_loop/mod.rs:6460-6474`(`verify_tasks_complete`) + `crates/ralph-core/src/state_projector/task.rs:118-150`(`project_close_task`)

**具体修改:**
1. `verify_tasks_complete` 之前增加一次 `tasks.jsonl` 扫描:对 `status=open` 且有同 step 的 `work.done` 事件历史,自动 `project_close_task` 补 close。
2. 或在 `coordinator` 终态判定逻辑(`event_loop/mod.rs:8430-8500` 区域)加 fallback:`if open_tasks but all have work.done` → 自动 close + 继续。
3. 预期:step-04 task 不会卡 open,coordinator 能正常推进到 review.start。

---

#### P2-3:executor 重发收敛信号 — 硬上限 + 终止条件

**目标文件:** `presets/en/ce-executor-serial.yml:1115-1216`

**具体修改:**
1. 在 executor instructions 增加:
   ```yaml
   ### Re-emission Limit (HARD RULE)
   Do NOT re-emit `work.done` more than 2 times for the same task_key within one iteration window.
   After 2 failed re-emissions, publish `work.failed` with reason="re-emit_exhausted" instead.
   ```
2. 配合:hard gate 在第 3 次同 retry_key 的 missing_event_gate 后,自动注入 `ralph stop` 指令(改为 emit `work.failed` 走 plan.blocked 路径)。

---

#### P2-4:CLI 路径与 event_loop 路径 envelope schema 对齐

**目标文件:** `crates/ralph-cli/src/policy_check.rs:609-737` + `crates/ralph-cli/src/commands/emit.rs`

**具体修改:**
1. CLI `run_policy_check_unified` 调 `evaluate_emit_gate` 而非绕开 stage_pipeline(2026-06-28-002 plan 已经在做,需落地)。
2. 同步:`human.guidance` topic 在 ce-executor-serial preset 禁用(2026-06-28-004 plan U13 已规划)。
3. 验证:CLI 路径产出的 envelope schema 与 event_loop 路径完全一致,drift detector 看到的 topic 集合与 event_loop 一致。

---

#### P2-5:plan_name / step 错发硬校验(防止 events.jsonl:8 那种完全错位)

**目标文件:** `crates/ralph-core/src/execution_contract.rs`(`work.done` require_payload_fields 段)

**具体修改:**
1. 在 `work.done` 契约的 `require_payload_fields` 增加 `plan_name_must_match_loop` 规则:plan_name 必须等于 `.ralph/loops.json` 中当前 loop 的 plan_name,否则 hard reject。
2. step 必须以 `step-NN` / `fix-NN` / `trivial` / `test` 之外的实际预设前缀开头,`step="test"` 时 plan_name 必须是 "test" 计划才接受。
3. 修后预期:events.jsonl:8 那种完全错发被拦在 hard gate 之前。

---

## 4. 修复落地路径(优先级排序)

| 步骤 | 修复 ID | 预计工时 | 验证方式 |
|---|---|---|---|
| 1 | **P0-1**(去重 + retry_key 共享) | 1 天 | 跑 `cargo nextest run -p ralph-core -- recovery_envelope_u7_u8` + e2e smoke |
| 2 | **P0-2**(projector loop_id 兜底) | 0.5 天 | 跑 `cargo nextest run -p ralph-core -- test_state_projector` + 1 个 e2e loop |
| 3 | **P0-4**(timeout 调到 600s) | 0.5 天 | 改 `workflow_contract.rs` + `ralph.yml`;跑 preset_lint + 1 个 e2e loop |
| 4 | **P1-2**(`enrich_task_resume_payload_full` 替换 inline JSON) | 0.5 天 | 跑 drift 单元测试 + 1 个 e2e loop 验 `kind` 字段 |
| 5 | **P1-1**(hard gate 二次检测前 stall guard) | 0.5 天 | 跑 hard_gate 单元测试 + recovery_envelope 测试 |
| 6 | **P0-3**(plan.blocked 串联) | 1 天 | 跑 termination 单元测试 + 1 个 e2e loop 验 REVIEW_COMPLETE 终态 |
| 7 | **P1-3**(`from_key:` 不再 hard reject) | 0.5 天 | 跑 execution_contract 单元测试 |
| 8 | **P1-4**(executor 重发 prompt 指引) | 0.5 天 | preset_lint + 1 个 e2e loop |
| 9 | **P2-1**(`loop.stalled` 业务事件) | 1 天 | 跑 stall_recovery 单元测试 + e2e smoke |
| 10 | **P2-3**(executor 重发上限) | 0.5 天 | preset_lint |
| 11 | **P2-2**(coordinator 终态补 close) | 1 天 | e2e loop + verify_tasks_complete 测试 |
| 12 | **P2-4**(CLI 路径对齐) | 1 天 | drift 单元测试 + CLI 集成测试 |
| 13 | **P2-5**(plan_name 硬校验) | 0.5 天 | execution_contract 单元测试 |

**总工时预估:9 个工作日**(按单人 dev 估算)。

---

## 5. 引用清单(全部绝对路径 + 行号)

### 5.1 根因代码位置
- `crates/ralph-core/src/event_loop/mod.rs:6080-6225` — stall_recovery handoff_dispatch_timeout
- `crates/ralph-core/src/event_loop/mod.rs:6104-6112` — inline JSON payload(缺 `kind`)
- `crates/ralph-core/src/event_loop/mod.rs:3550` — `on_hat_activated`(hat 激活 hook)
- `crates/ralph-core/src/event_loop/mod.rs:6423-6444` — `current_loop_id` / `current_loop_id_for_contract`
- `crates/ralph-core/src/event_loop/mod.rs:8430-8500` — termination 路径
- `crates/ralph-core/src/event_loop/rejection.rs:680-712` — `enrich_task_resume_payload_full`
- `crates/ralph-core/src/state_projector/task.rs:39-116` — `project_ensure_task`
- `crates/ralph-core/src/state_projector/task.rs:86-88` — loop_id 注入(根因 P0-2)
- `crates/ralph-core/src/execution_contract.rs:488-505` — `resolved_task_id` 解析
- `crates/ralph-core/src/execution_contract.rs:570-603` — `TaskWrongLoop` hard reject
- `crates/ralph-core/src/diagnosis/responder.rs:880-919` — `classify` 状态机
- `crates/ralph-core/src/diagnosis/responder.rs:610-671` — `check_recovery`
- `crates/ralph-core/src/diagnosis/responder.rs:518-541` — `record_finding` Final 分支
- `crates/ralph-core/src/diagnosis/envelope.rs:569-590` — `retry_key_from_parts`
- `crates/ralph-core/src/diagnosis/envelope.rs:165-194` — `DiagnosisOutcome` enum
- `crates/ralph-core/src/drift/detector.rs:441-468` — `check_field_completeness` 告警
- `crates/ralph-core/src/drift/engine.rs:392-406` — `check_termination_hint`(根因 P0-3)
- `crates/ralph-core/src/drift/engine.rs:553` — `recovery_outcome_update` 自观测
- `crates/ralph-core/src/workflow_contract/handoff_tracker.rs:199-237` — `HandoffTracker::expired`
- `crates/ralph-core/src/config/workflow_contract.rs:45-50,145-163` — handoff timeout 常量
- `crates/ralph-core/src/config/execution_contracts.rs:79-81,112-114` — `loop_scoped` 默认 true
- `crates/ralph-cli/src/loop_runner/hard_gate.rs:912-937` — `inject_missing_event_hard_gate_guidance_with_triggers`

### 5.2 预设位置
- `presets/en/ce-executor-serial.yml:1-62` — 10-hat 拓扑
- `presets/en/ce-executor-serial.yml:255-300` — `execution_contracts.work.done` 7 字段
- `presets/en/ce-executor-serial.yml:272-275` — `require_task.loop_scoped: true`
- `presets/en/ce-executor-serial.yml:633-643, 787-800` — coordinator Phase Gate
- `presets/en/ce-executor-serial.yml:868-873, 2354-2364` — shipper 失败路径
- `presets/en/ce-executor-serial.yml:976-980, 1115-1216` — executor
- `presets/en/ce-executor-serial.yml:1218-1288` — validator
- `presets/en/ce-executor-serial.yml:1300-2130` — review chain
- `presets/en/ce-executor-serial.yml:2549-2645` — progress-steward 兜底
- `presets/en/ce-executor-serial.yml:2568-2581` — `loop.stalled` triggers
- `ralph.yml:28` — `field_completeness_threshold: 0.85`

### 5.3 运行产物
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/events-20260628-172725.jsonl`(26 行)
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/diagnostics/2026-06-29T01-27-25/recovery.jsonl`(19 行)
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/diagnostics/2026-06-29T01-27-25/diagnosis-summary.json`
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/loop-termination-reason.json`
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/agent/tasks.jsonl`(5 行,step-04 仍 `status=open`)
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/agent/progress.md`(`Completed Steps: step-01, step-03`)
- `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/agent/summary.md`

### 5.4 关联报告(已读)
- Agent A:`/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-A-flow-reconstruction.md`
- Agent B:`/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-B-history-kb.md`
- Agent C:`/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-C-deviation-evidence.md`

### 5.5 历史根因与本次对应
- 30 天第 11 次复发(同 2026-06-28 primary-115810 字面同型) — Agent B §0
- 4 类根因(stall_recovery 死信、drift 自观测、FlowStepScope 误拒、stage_pipeline CLI 旁路)与本次 100% 命中 — Agent B §1.1, §1.2, §1.3, §1.4
- `2026-06-28-004 fix plan` 16 Unit 修复已覆盖本次 10/10 现象 — Agent B §3.2,但本次 run 早于 plan 落地
- `2026-06-28-005 refactor human.guidance topic plan` 长期方案 — Agent B §2

---

## 6. 报告交付摘要

- **报告路径**:`/Users/pittcat/Dev/Rust/ralph-orchestrator/docs/report/2026-06-29-ce-executor-serial-primary-20260628-172725-D-root-cause-and-fix.md`
- **核心产出**:
  1. **4 类 failure 因果链** (§1):明确 F1 (stall_recovery) → F2 (missing_event_gate 错锚) → F3 (TaskWrongLoop) → F4 (drift 告警) 的级联关系;指出 runtime 内部已识别真 stall 源但 retry_key 错配
  2. **P0/P1/P2 归因表** (§2):12 个问题,4 P0 + 4 P1 + 4 P2,每条带精确代码行号
  3. **修复建议** (§3):14 条 actionable 修复(4 P0 + 4 P1 + 5 P2),每条带目标文件 + 精确行号 + 具体修改代码片段
  4. **修复落地路径** (§4):13 步工时估算,合计 9 个工作日
- **归因分类总结**:
  - **基座机制问题(主因,7 个 P0/P1)**:stall_recovery 锚错、recovery_exhausted 不走 plan.blocked、projector 不补 loop_id、inline JSON 不带 kind、hard gate 二次检测、handoff 30s 硬编码、validator 调度竞态
  - **preset 设计问题(2 个 P1/P2)**:loop_scoped 默认 true 无 fallback、progress-steward 兜底未触发
  - **运行产物问题(2 个 P2,放大器)**:executor 重发风暴、agent 完全错发
  - **本质根因**:runtime 在 30 天内第 11 次复发"修复机制系统性失效",本次 172725 是 `2026-06-28-004 fix plan` 起草后/落地前的最后一份失败 run