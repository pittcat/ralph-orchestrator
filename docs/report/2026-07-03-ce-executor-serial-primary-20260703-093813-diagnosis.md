# ce-executor-serial 运行链路诊断报告

> **诊断对象**:`primary-20260703-093813` 当前 run(PID 1410426 仍存活,卡死中)
> **preset**:`presets/en/ce-executor-serial.yml`(10-hat isolated 模式)
> **诊断时间**:2026-07-03T10:12Z
> **诊断依据**:`/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/events-20260703-093813.jsonl`(25 行业务事件) + 主仓源码 + 11 份历史诊断 + 13 条 memory

---

## 1. 结论摘要

**健康度**:**部分健康**—— review 链 100% 打通(对比上次 020135 run 0% 是大幅进步),但 fix-unit 链 0% 推进,**loop 当前卡死**在 fix-01 第一修复单元 dispatch 阶段,累计运行 32 分钟无进展。

- **关键异常**:**P0 × 3、P1 × 3、P2 × 2**
- **P0 阻断点**:`work.ready(fix-01)` 被 state_projector 拒绝(task_id 复用),导致 fix-02/03、plan.complete、REVIEW_COMPLETE、LOOP_COMPLETE 整条链全部不触发
- **历史重复**:是——这是 `2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md` 已识别但 U1-U10+U12 全部 active 待执行的同一个面
- **机制 vs 编排**:**编排设计缺陷为主(70%)**,**机制白名单 gap 为辅(20%)**,**两 hat 兜底语义冲突(10%)**

---

## 2. 整体执行过程评估

### Phase 1:unit_loop(✅ 100%)

| Step | 事件 | 时间 | 结果 |
|---|---|---|---|
| 1 | `work.start` | 09:38:13Z | ✅ |
| 2 | `work.ready(step-01)` task=`task-1783071643-77fe` | 09:40:48Z | ✅ |
| 3 | `work.done(step-01)` commit=1, changed=252 | 09:42:25Z | ✅ |
| 4 | `test.passed(step-01)` 5/5 | 09:43:13Z | ✅ |
| 5 | `work.ready(step-02)` task=`task-1783071869-d363` | 09:44:38Z | ✅ |
| 6 | `work.done(step-02)` commit=1, changed=184 | 09:46:45Z | ✅ |
| 7 | `test.passed(step-02)` 17/17 | 09:47:17Z | ✅ |

### Phase 2:review_walk(✅ 100%)

| Step | 事件 | 时间 | 结果 |
|---|---|---|---|
| 8 | `review.start` | 09:47:52Z | ✅ |
| 9-20 | 6×`review.dimension.ready` + 6×`review.dimension.done` | 09:48:58Z-10:02:46Z | ✅ |
| 21 | `review.dimensions.complete` | 10:03:39Z | ✅ |
| 22 | `review.complete(verdict=fail, findings=3 P1, fix_plan_file=fix-plan.md)` | 10:06:04Z | ✅ |

### Phase 3:fix_units(❌ 0% 推进)

| Step | 事件 | 时间 | 结果 |
|---|---|---|---|
| 23 | `work.ready(fix-01)` task=`task-1783073243-0087` | 10:07:30Z | **❌ 被 state_projector 拒绝** |
| 24 | `task.resume(kind=fix_unit_complete, target_hat=coordinator)` | 10:09:46Z | ⚠️ progress-steward 兜底触发 |
| 25 | `loop.batch_sync` (无业务进展) | 10:10:50Z | ⏸️ |

**关键拒绝原因**:`.ralph/diagnostics/logs/ralph-2026-07-03T17-38-12-398-1410405.log:138`
> `state_projector` WARN: `task_id_reused_across_keys: work.ready reused open task id 'task-1783073243-0087' which is already bound to task_key None; new key is 'ce-executor:2026-06-20-001-feat-python-sort-algorithms:fix-01:test-routing-bug'. Mint a fresh id per fix-unit via Task::fix_unit_task_id(plan, fix_round, fix_unit_index, unix_ts).`

### Phase 4:plan_end / ship / terminal(⏸️ 0% 触发)

step 15-18 全部未发生,loop 永远停滞在 unit_loop 阶段。

---

## 3. RALPH 基座机制评估

**机制本身的设计是对的**:
- ✅ `default_publishes` 注入是合理的 backpressure 设计(避免 hat 沉默时 loop 死锁)
- ✅ shipper 白名单的"STRICT-MATCH exact + 最小集"也是对的(防止误判 pass)
- ✅ event loop 的 plan-gate / queue 推进逻辑无重大缺陷
- ❌ **错在三者没有对齐**——新兜底入口出现后白名单未跟进,fix-unit 桥接缺协议

**已识别的机制层问题**:

| ID | 问题 | 证据 | 严重性 |
|---|---|---|---|
| M-1 | `current-hat-events` 指针指向 0 字节 hat-channel,isolated 模式路由失效但静默降级到主 events | `.ralph/current-hat-events:1` → 空文件 | P0 |
| M-2 | shipper `recoverable_whitelist` 不含 `default_publishes`,无法将"机制兜底注入"识别为可恢复路径 | `presets/en/ce-executor-serial.yml:2646-2675` | P0 |
| M-3 | ralph hat `loop.cancel/LOOP_COMPLETE` 兜底与 reporter `awaiting_decision=true` 语义冲突(本 run 未触发,但 075227 已暴露) | `events-075227.jsonl:4-5` | P1 |
| M-4 | state_projector SSOT 同步:`tasks.jsonl:2-3` 同 task_id 双实体(line 2 closed 09:46:31, line 3 closed 09:46:50) | `.ralph/agent/tasks.jsonl:2-3` | P1 |
| M-5 | `completion_after_terminal` 守卫在跨 batch 后失效(本 run 因未达成 terminal 未触发) | 历史 020135 已识别 | P0(潜在) |

---

## 4. 编排合理性评估

**编排层的问题**比机制层更严重:

### 编排设计缺陷

| ID | 问题 | 证据 | 严重性 |
|---|---|---|---|
| O-1 | coordinator 在 fix-unit dispatch 时**复用了已绑定到 None key 的 task_id**,违反 `presets/en/ce-executor-serial.yml:986-994` 的 `MUST be freshly minted` HARD RULE | `events-093813.jsonl:23` + `.ralph/diagnostics/logs/...:138` | P0 |
| O-2 | executor 关闭 task-1783073243-0087(`tasks.jsonl:4 status:closed`)但**未 emit `work.done`**,违反 `presets/en/ce-executor-serial.yml:1123` `executor.publishes=["work.done","work.failed"]` | `tasks.jsonl:4` + 事件流空白 | P0 |
| O-3 | progress-steward 触发 `task.resume` 后,coordinator 接到 signal 但**未 emit `work.ready(fix-02)`**,task.resume → coordinator 推进链断 | `events-093813.jsonl:24-25` | P0 |
| O-4 | `progress.md` 仍停在 `Completed Steps: step-01, step-02`,fix-01 完成后未更新 | `.ralph/agent/progress.md` | P1 |
| O-5 | `fix-plan.md:7` 用 `parent_plan` 字段名,而 plan.md 用 `## Plan file:`,两套命名并存 | `fix-plan.md:7` vs `context.md:8` | P1 |
| O-6 | `review-trace.json` 含**伪造 verified_at 时间戳**(未来 1-1.5h,agent 回放模板字符串未替换) | `review-trace.json:21,24,30,33,36` | P1 |
| O-7 | `work.start` 走 `triggered:"planner"` 而非 coordinator(planner 不在 10-hat 拓扑),loop-bootstrap 绕过 preset 拓扑 | `events-history-093813.jsonl:1` | P1(本 run 现象,075227 同根) |

---

## 5. 中间产物与机制一致性

| 产物 | 与事件流一致性 | 备注 |
|---|---|---|
| `tasks.jsonl` | ⚠️ | 4 行,line 2-3 同 id 双实体;line 4 fix-01 闭合但事件流无对应 work.done |
| `progress.md` | ❌ | 停在 step-02,fix-01 未记录 |
| `findings.md` | ✅ | 3 P1 findings 与 review.complete 一致 |
| `fix-plan.md` | ✅ | 3 fix-units,parent_plan 字段名软契约不一致 |
| `review-sequence.json` | ✅ | 6 维顺序与事件流一致 |
| `review-trace.json` | ⚠️ | verified_at 伪造未来时间,audit trail 不可信 |
| `events-*.jsonl` | ✅ | 25 行连续,无跳号 |
| `ledger.jsonl` | ⚠️ | seq 22/23 同 ts,cosmetic 但下游 analytics 排序可能错位 |
| `loops.json` | ✅ | loop 状态与 PID 一致 |

---

## 6. 核心问题归因(机制 vs 编排)

**直接结论**:**问题 70% 在编排层、20% 在机制层、10% 在两 hat 兜底语义不一致**。

### 6.1 编排层主导(P0-1, P0-2, P0-3)

**根因**:`ce-executor-serial` 的设计前提是 "plan active + tasks in_progress",fix-unit dispatch 阶段缺乏 fresh task_id mint 的强制路径。preset `presets/en/ce-executor-serial.yml:986-994` 写了 `MUST be freshly minted`,但 coordinator 在 fix-01 dispatch 时实际复用了 task-1783073243-0087(该 id 早先由 `ralph tools task create` 绑定到 None key)。

**这是编排错误,不是机制问题**:
- state_projector 拒绝行为**完全正确**(防止 task 账本被污染)
- 拒绝消息**完全准确**(给出了正确修复路径 `Task::fix_unit_task_id(plan, fix_round, fix_unit_index, unix_ts)`)
- 错的是 coordinator 没遵守 preset 的 hard rule

### 6.2 机制层 gap(P0-2 shipper 白名单)

**根因**:shipper `recoverable_whitelist`(`presets/en/ce-executor-serial.yml:2646-2675`)是冻结的"最小集",但 075227 run 暴露了"coordinator 沉默 → runtime 注入 default_publishes"的新兜底入口,白名单未跟进。这是机制白名单宽度策略考虑不周,但**仅在 075227 run 触发了 P0-2 失败路径**(本 run 093813 未触发该问题)。

**判断**:机制本身设计合理,错在"两次诊断才扩一次"的节奏偏慢。

### 6.3 两 hat 兜底语义冲突(075227 run)

**根因**:reporter `awaiting_decision=true` 与 ralph `loop.cancel/LOOP_COMPLETE` 在 075227 run 出现矛盾(本 run 未触发)。这是新机制不一致,**不是编排问题**——两个 hat 都在按自己的设计走,只是设计没对齐。

---

## 7. 与历史 30 天 10 次复发的关系

**这是同根簇的第 11 次复发**(`perky-maple / noble-peacock / merry-lotus` 簇),但**机制翻面了**:

| Run | 卡点 | 现象 |
|---|---|---|
| 020135 | review → synthesizer 桥接 | review 链 0% 工作 + 误发 plan.blocked |
| **093813(本次)** | **fix_unit → plan_end 桥接** | **review 链 100% 工作 + fix 链 0% 工作** |
| 075227 | coordinator 沉默 + runtime 注入 | 0 业务事件 + report.done fail → ralph cancel |

**共同根因**:`2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md` 的 U1-U10+U12 全部 active 待执行,只闭合了 U11(`5a58b8ac`)。本次 run 进步说明 U11 修复的 synthesizer 输出格式已生效,但 fix_unit 阶段的桥接(本次 run 的卡点)尚未在 active plan 中被 U 覆盖。

**本次新发现(未在 active plan 中)**:
- shipper 白名单不含 `default_publishes`(075227 暴露,本 run 复核确认)
- `current-hat-events` 指针指向空文件(isolated 模式静默降级到主 events)
- `tasks.jsonl` 同 task_id 双实体(state projector SSOT 被破坏)
- `review-trace.json` 伪造未来时间戳(agent 行为问题)

---

## 8. 修复建议(按优先级)

### P0-1:编排修复 — coordinator 必须为每个 fix-unit mint fresh task_id

- **目标**:`presets/en/ce-executor-serial.yml:986-994` 现有 `MUST be freshly minted` 段
- **修改**:
  - 在 coordinator `instructions` 中新增 "Fix-Unit Task ID Minting" 强制段
  - 给出 `ralph tools task create --fix-unit <plan> <fix_round> <fix_unit_index>` 的 CLI 调用模板
  - 在 shipper 校验里加"work.ready 的 task_id 必须是 task-{unix_ts}-{4hex} 且 unix_ts > 上一个 fix-unit"的硬约束
- **预期**:消除 fix-01 阶段 task_id 复用,直接解决本 run P0-1 阻断

### P0-2:机制修复 — shipper 白名单扩 `default_publishes`

- **目标**:`presets/en/ce-executor-serial.yml:2675`(白名单末尾)及附近注释
- **修改**:
  - 在 `Recoverable reasons` 列表中新增 `- "default_publishes"`
  - 注释说明 "coordinator 沉默是 backpressure 而非实现失败;验证 1-2 通过时路由到 pass"
  - 同步更新 `presets/schemas/ce-executor-serial.yml` SSOT
- **预期**:075227 run 场景(9 pytest + build OK)会从 `verdict=fail` 变为可恢复路径

### P0-3:编排修复 — executor 关闭 task 必须 emit work.done

- **目标**:`presets/en/ce-executor-serial.yml:1123` executor `publishes` 段及前后指令
- **修改**:
  - 在 executor `instructions` 中新增 "Task Closure & Event Emission" 段
  - 强制 `ralph tools task close` 之后必须 `ralph emit work.done`,失败时 `work.failed`
  - 在 validator 触发条件里加 "看到 task closed 但无 work.done → emit task.resume(target_hat=executor, kind=missing_work_done)"
- **预期**:消除 fix-01 阶段"task 关闭但 work.done 缺失"的断链

### P0-4:机制修复 — isolated mode hat-channel 路由恢复

- **目标**:`crates/ralph-core/src/event_loop/mod.rs` `current-hat-events` 解析与写入段
- **修改**:
  - 当 `current-hat-events` 指向的文件大小为 0 时,fallback 路径应 emit 诊断到 `.ralph/diagnostics/channel-routing-fallback-{ts}.md`,而非静默降级
  - 验证 `wc -l $(cat .ralph/current-hat-events)` 落盘后与主 events 一致
- **预期**:暴露而非掩盖 hat-channel 路由失效

### P1-1:机制修复 — state_projector 去重逻辑强化

- **目标**:`crates/ralph-core/src/state_projection.rs`(或对应文件)的 tasks 投影段
- **修改**:
  - 写入 tasks.jsonl 前检查 `(plan_name, step, task_key)` 三元组唯一性
  - 重复时拒绝并 emit 诊断(不静默写双实体)
- **预期**:消除 P0-3 的 tasks.jsonl 同 id 双实体

### P1-2:编排修复 — progress.md 强制在每 unit 闭合时更新

- **目标**:`presets/en/ce-executor-serial.yml:1104-1113` progress 段
- **修改**:
  - 强制 `ralph emit work.done` 的 payload 校验包含 `progress_md_updated: true`
  - validator 拒绝 progress.md 未更新的 work.done
- **预期**:消除 fix-01 完成后 progress.md 漏更新

### P1-3:机制修复 — `date -u` 在 agent 中强制 shell 替换

- **目标**:`crates/ralph-core/data/*.md` 中所有提到 `date -u +%FT%TZ` 的 agent 指南
- **修改**:
  - 强调"必须由 shell 执行,不要回放为模板字符串"
  - 在 `ralph-tools-emit.md` 中加 review-trace 时间戳校验段
- **预期**:消除 review-trace.json 伪造未来时间戳

### P2(运维改进,可选):reporter 报告模板区分"实现 pass + 编排异常"+ 补 `ralph plan audit` 命令

- **目标**:`presets/en/ce-executor-serial.yml` reporter 段
- **修改**:
  - reporter 报告模板新增 "编排状态" 段,区分"代码已交付 / 编排异常 / 编排正常"
  - `crates/ralph-cli/src/commands/` 新增 `ralph plan audit --plan <path>` 命令,扫描 frontmatter 状态 + tasks.jsonl 一致性
- **预期**:减少 manager 看到 fail+cancel 矛盾报告

---

## 9. 验收路径(必须)

修复后必须用 `run_workflow_guard_scenario`(真 EventLoop runner,**禁止用 `run_scenario` stub**——stub 只查 iterations 数,会静默吞掉拓扑失配),按以下三组场景验收:

| 场景 | 验证目标 | 期望事件链 |
|---|---|---|
| SC1-1 | 同一 plan 走正规链 1 | `work.start → work.ready → work.done → test.passed → plan.complete → REVIEW_COMPLETE → report.done → LOOP_COMPLETE` |
| SC1-2 | 走 fix-unit 链 1 | `... → review.complete(verdict=fail) → work.ready(fix-01,fresh_task_id) → work.done → test.passed → plan.complete → ...` |
| SC1-3 | 走 fix-unit 链 2(连续 fix-01/02/03) | `... → fix-01/02/03 全部闭合 → plan.complete → REVIEW_COMPLETE → ...` |

---

## 10. 关键证据索引

| 类别 | 路径 |
|------|------|
| preset | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/en/ce-executor-serial.yml`(2962 行) |
| preset schema SSOT | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/presets/schemas/ce-executor-serial.yml` |
| 事件循环 | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/crates/ralph-core/src/event_loop/mod.rs` |
| 事件流(本次) | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/events-20260703-093813.jsonl` |
| 拒绝日志(本次) | `/home/chaowen/Dev/agent_tools/ralph-e2e/.ralph/diagnostics/logs/ralph-2026-07-03T17-38-12-398-1410405.log:138` |
| 历史 075227 诊断 | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/report/2026-07-03-ce-executor-serial-primary-20260703-075227-diagnosis.md` |
| 历史 020135 诊断 | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/report/2026-07-03-ce-executor-serial-primary-20260703-020135-diagnosis.md` |
| 历史知识库 | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/report/2026-07-03-ce-executor-serial-primary-20260703-history-knowledge-base.md` |
| 30 天 9 次复发簇 fix | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/achieved/solutions/developer-experience/ce-executor-serial-30day-6th-recurrence-fix.md` |
| 核心修复 plan(未完成) | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/docs/plans/2026-07-02-005-fix-ce-executor-serial-p0-terminal-path-plan.md` |

---

## 11. 总结

本次 run `primary-20260703-093813` 的核心问题是 **编排设计缺陷(coordinator fix-unit dispatch 复用 task_id 违反 HARD RULE)+ 机制白名单 gap(shipper 不认 default_publishes)**,共同导致 fix-unit 链 0% 推进,loop 卡死。这是 30 天 11 次同根复发的新翻面——上次卡在 review→synthesizer,本次卡在 fix_unit→plan_end。

**修复责任分配**:
- 70% 在 preset 编排层(必须改 `presets/en/ce-executor-serial.yml` 的 coordinator/executor `instructions` 段)
- 20% 在 shipper 白名单机制(`presets/en/ce-executor-serial.yml:2646-2675` 扩 `default_publishes`)
- 10% 在 event_loop 兜底守卫(`crates/ralph-core/src/event_loop/mod.rs` 强化 channel 路由失败可见性)

**RALPH 机制本身没有重大设计问题**——它的拒绝行为、backpressure、白名单最小集都是对的。错的是 preset 编排没遵守自己写的 HARD RULE,以及机制白名单策略对新兜底入口反应过慢。这两个问题都在 `2026-07-02-005` plan 范围内,但 U1-U10+U12 全部 active 待执行,只闭合了 U11。**建议把本报告的 P0-1/P0-2/P0-3 修复追加到 `2026-07-02-005` plan 的 U 列表中,优先 P0-1 修复 coordinator 强制 mint fresh task_id**。
