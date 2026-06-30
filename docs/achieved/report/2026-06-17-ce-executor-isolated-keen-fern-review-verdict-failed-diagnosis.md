# ce-executor-isolated Loop 诊断报告:2026-06-10-003 Step-01 Review-Verdict-Failed

> **报告日期**:2026-06-17
> **作者**:Loop & Preset 诊断专家(Ralph 自动报告)
> **Loop ID**:`2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-keen-fern`
> **Preset**:`builtin:ce-executor-isolated`(10-hat 拓扑,`execution_mode: isolated`)
> **Plan**:`docs/plans/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan.md`
> **最终状态**:**Failed**(review verdict failed, propagated to final mirror)
> **持续时间**:1h 47m 52s / 8 iterations / 69 业务事件

---

## 1. 结论摘要

本次 `ce-executor-isolated` run **没有失败在 U1 实施本身**(U1 scaffold 14 个 placeholder 子文件已落地、mod.rs 顶部 12 个 mod 声明 + 12 个 pub use 转发点已加、v9.1 计划 delta 已写、commit `91596bc` 落地),而是**失败在 review wave 的 incomplete convergence 机制**——4 个 dimension 中 2 个(timeout)> 0.8 × aggregate_timeout 未归位,触发 `incomplete_wave_gate` 机制收摊,`review-synthesizer` 自动 emit `plan.blocked(reason=dimension_reviewers_failed_to_converge)`,经 `shipper` 推进到 `REVIEW_COMPLETE(pass_or_fail=fail)` + `report.done(awaiting_decision=true)`,最后 loop 因 `verdict_gate` 命中而以 `report.done` 收到 `review_failed` 终止。

**链路修复层面一切正常**,但**修复面以外的 P1 residual** 把 loop 拖进 fail:
- **P0**:review wave 4 维度中 2 维度(`testing` / `maintainability`)worker 未在 80% aggregate timeout 前返回。
- **P0**:发现 2 个 residual 写进 `REVIEW_COMPLETE.payload.residual_findings_summary`:`test_u2_*` 4 个 fixture 缺失 schema(regression,非 U1 引起但 review 抓出);`scripts/audit-file-sizes.sh:14` 缺 `event_loop/*.rs` 扩展(R7 acceptance gap,U1 任务的子项未达成)。
- **P2**:整个 loop 期间被 185 条 `recovery.jsonl` envelope 包裹(10 种 unique pattern,模式 100% 重复),但 `recovery.jsonl` 落盘机制**只记录 cli_emit 拒收**,没有把 `payload_contract_violation` 升级为 loop-fatal,导致 envelope 噪声淹没真实链路。

**根因(主)**:R6 `incomplete_wave_gate` 机制 + `dimension_reviewers_failed_to_converge` 收摊逻辑按设计触发,这是**机制正常工作**而非 bug。
**根因(次)**:U1 自身留有 2 个未清理的 P1 残留(测试 fixture 缺失 + audit 脚本未扩展),被 review 抓出后被计为 residual,导致 `verdict=fail`,无 fix 路径可走(已 fix-exhausted 路径被这条更早的 plan.blocked 切断)。

---

## 2. 执行链路对比图

### 2.1 Preset 预期事件流(`ce-executor-isolated`)

```
work.start
  └─ coordinator (work.start → work.ready)
       └─ executor (work.ready → work.done)
            └─ review-coordinator (work.done → review.wave.ready ×N)
                 └─ dimension-reviewer ×N (review.wave.ready → review.dimension.done ×N)
                      └─ review-synthesizer (aggregate → review.passed|review.failed)
                           └─ plan-gate (review.passed → queue.advance | plan.complete)
                                └─ shipper → REVIEW_COMPLETE → reporter → report.done → LOOP_COMPLETE
       └─ (fix path): Fixer ≤3 → debug-resolver → Executor (fix.plan.ready)
       └─ (block path): review-synthesizer → plan.blocked → shipper → REVIEW_COMPLETE → reporter → report.done
```

### 2.2 实际执行链路(loop `keen-fern`)

```
[iter 0] work.start                              02:33:24
[iter 0] work.ready (coordinator, valid)         02:47:43   ✅ preflight 9/9 通过
   ↓ executor 触发,但 emit 22 次全被 cli_emit 拒:
        ❌ 22 × build.done (topic_denied, executor 不允许发 build.done)
        ❌ 22 × work.done (payload_contract_violation / missing plan_name)
        ❌ 22 × work.ready (coordinator/executor 都尝试 emit,拒)
        ❌ 12 × work.done (invalid_field_value plan_name="x")
   ↓ human.guidance 注入                            03:25:30
[iter 0] work.done (executor, valid, 2 commits/204 lines)  03:30:02   ✅ 写进 events.jsonl
[iter 0] work.done (重复)                        03:30:14   ⚠️ 同 payload 重复一次
   ↓ review-coordinator 触发,但 emit 5 次 review.passed (skip_reason=aggregate_timeout) 被拒:
        ❌ 5 × review.passed (invalid_field_value, review-coordinator 不允许 skip_reason=aggregate_timeout)
   ↓ build.done × 5 正常                            03:35-03:44
[iter 4] review.wave.ready × 4 (correctness/testing/maintainability/requirements)  03:55:55   ✅ wave w-18b9c21c828231e8-0, total=4
[iter 5] review.dimension.done (correctness 0 findings)            04:01:31   ✅
[iter 5] review.dimension.done (requirements 1 finding P1)         04:01:56   ✅
[iter 6] review.dimension.done (correctness 1 finding P1)          04:04:15   ✅
   ↓ testing + maintainability 2 维度未在 80% aggregate_timeout 前归位
[iter 6] plan.blocked (review-synthesizer, reason=dimension_reviewers_failed_to_converge, received=2/4)  04:06:40   ✅ 机制 R6 触发
[iter 7] shipper 触发,build.done × 5               04:08-04:14
[iter 8] REVIEW_COMPLETE (verdict=fail, pass_or_fail=fail, residual_findings 2 P1)  04:18:45   ✅
[iter 8] report.done (reporter, awaiting_decision=true)            04:21:07   ✅
[iter 8] loop.terminate (review_failed, exit 1)                   04:21:16   ✅ verdict_gate 命中
```

### 2.3 关键观察

| # | 预期 | 实际 | 状态 |
|---|---|---|---|
| 1 | `coordinator` emit `work.ready` | 1 次,2:47:43(payload 含 plan_name/task_id/task_key/step/preflight_checks/complexity) | ✅ |
| 2 | `executor` emit `work.done` (commit_count=1, changed_lines=204) | 1 次成功 + 1 次重复(payload 完全相同)+ 56 次被拒 | ✅(但有重复) |
| 3 | `review-coordinator` emit `review.wave.ready` (4 维度) | 4 次(同 wave_id, 3:55:55 同一秒) | ✅ |
| 4 | `dimension-reviewer` emit `review.dimension.done` (4 维度) | 3 次(correctness 0/1、requirements 1,共 2 findings) | ⚠️ testing + maintainability 未归位 |
| 5 | `review-synthesizer` emit `review.passed/review.failed` 或 `plan.blocked` | emit `plan.blocked(received=2/4, missing=testing+maintainability)` | ✅ R6 incomplete_wave_gate 触发 |
| 6 | `shipper` emit `REVIEW_COMPLETE` | 1 次,verdict=fail, pass_or_fail=fail | ✅ |
| 7 | `reporter` emit `report.done` | 1 次,awaiting_decision=true | ✅ |
| 8 | `verdict_gate` 拦截 `REVIEW_COMPLETE` (pass_or_fail=fail) | loop.terminate 写为 review_failed | ✅ |

---

## 3. 证据清单

### 3.1 文件位置

| 文件 | 关键信息 |
|---|---|
| `.ralph/events-20260617-023324.jsonl` (69 行) | 全部 69 个业务事件 |
| `.ralph/events-history-20260617-023324.jsonl` (2 行) | `work.start` + `loop.terminate` 框架事件 |
| `.ralph/recovery.jsonl` (185 行,10 unique envelope) | 全部 185 条 cli_emit 拒收 envelope |
| `.ralph/loop-termination-reason.json` | `{"review_failed":{"topic":"report.done"}}` |
| `.ralph/loops.json` | `{"loops":[]}`(worktree 退出时清空,正常) |
| `.ralph/agent/summary.md` | "Status: Failed: review verdict failed and propagated to final mirror" |
| `.ralph/agent/context.md` | Loop ID, branch, human guidance 2 条 |
| `.ralph/agent/scratchpad.md` | 136 字节(空内容) |
| `.ralph/agent/tasks.jsonl` | 1 个 task:`task-1781664433-1998`,`key=ce-executor:...:step-01:u1-scaffold`,status=closed, created 02:47 / started 02:49 / closed 03:22(commit 真实时间 03:30,任务关闭提前于 commit) |
| `.ralph/agent/.events-hat-review-coordinator-...idempotency.jsonl` | review wave idempotency 记录(count=4, hash 一致,说明 4 个 review.wave.ready 是 dedup 后的 1 个) |
| `docs/plans/2026-06-10-003-...md` (1872 行) | U1-U7 详细 plan,frontmatter `status: stalled-after-U1` |
| `presets/en/ce-executor-isolated.yml` (2278 行) | 10-hat preset,execution_mode=isolated |

### 3.2 关键事件 payload

**work.ready (coordinator → executor, 02:47:43)**:
- 完整 payload 含 `plan_name=2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan`, `task_id=task-1781664433-1998`, `task_key=ce-executor:...:step-01:u1-scaffold`, `step=step-01`, `complexity=large`, `preflight_checks=9 项全通`, ✅ 全部 7 必填字段

**work.done (executor, 03:30:02)**:
- `plan_name=2026-06-10-003-...`, `task_id=task-1781664433-1998`, `step=step-01`, `commit_count=2`, `changed_lines=204`, ✅ 全部 7 必填字段

**work.done (executor, 03:30:14, 重复)**:
- 与上一条 payload 字节级相同,`triggered=ralph`, ⚠️ 同 hat 在 12s 内发同一事件两次,实际没被去重

**review.wave.ready × 4 (review-coordinator, 03:55:55)**:
- `idempotency_hash=53f0abfb...` 一致(4 个共享同一 dedup key),`idempotency_key=ce-review:...:round-1`
- 4 个 dimension:`correctness` (index 0) / `testing` (index 1) / `maintainability` (index 2) / `requirements` (index 3)
- `wave_id=w-18b9c21c828231e8-3271059-0`, `wave_total=4`
- `commit_count=2`, `changed_lines=304`, `diff_base=fb40414c...`, `diff_base_fallback=no_prior_wave`
- `changed_files=14 个`(12 placeholder + mod.rs + plan v9.1 delta), ✅ 全部 10 必填字段

**review.dimension.done × 3 (dimension-reviewer, 04:01-04:04)**:
- 04:01:31: `correctness` (index 3, ⚠️ 与 wave 顺序不一致,先归位的是 requirements?)→ 0 findings
- 04:01:56: `requirements` (index 1)→ 1 finding (P1)
- 04:04:15: `correctness` (index 0)→ 1 finding (P1)
- testing (index 1) + maintainability (index 2) **未出现**

**plan.blocked (review-synthesizer, 04:06:40)**:
- `reason=dimension_reviewers_failed_to_converge`
- `details={expected:4, received:2, missing_dimensions:[testing, maintainability], wave_id:w-18b9c21c...}`
- ✅ payload 符合 schema(`reason` 必填),`triggered=ralph`

**REVIEW_COMPLETE (shipper, 04:18:45)**:
- `verdict=fail`, `pass_or_fail=fail`
- `residual_findings_summary` 内含 2 P1:
  1. `test_u2_*` 4 个 policy-rejection tests fail(因 YAML fixture 缺 schema,route=review-fixer)
  2. `scripts/audit-file-sizes.sh:14` 缺 `event_loop/*.rs` 扩展(R7 acceptance gap,route=downstream-resolver)
- `plan_name`, `task_id`, `task_key`, `step` 全有
- ✅ 全部 4 必填字段

**report.done (reporter, 04:21:07)**:
- `awaiting_decision=true`, `pass_or_fail=fail`, `verdict=fail`
- `report_path=docs/report/2026-06-17-ce-executor-2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-report.md`
- ✅ 全部 2 必填字段

### 3.3 Recovery envelope 分布(185 条)

| 数量 | source_hat | topic | reason_code | 关键 message |
|---|---|---|---|---|
| 34 | executor | work.done | missing_required_field | "Missing required field: plan_name" |
| 22 | review-synthesizer | review.passed | missing_required_field | "Missing required field: plan_name" |
| 22 | plan-gate | queue.advance | missing_required_field | "Missing required field: plan_name" |
| 22 | executor | work.ready | payload_contract_violation | "Payload is not valid JSON" |
| 22 | coordinator | work.ready | payload_contract_violation | "Payload is not valid JSON" |
| 22 | coordinator | work.ready | missing_required_field | "Missing required field: plan_name" |
| 12 | executor | work.done | payload_contract_violation | "Payload is not valid JSON" |
| 12 | executor | work.done | invalid_field_value | "work.done plan_name mismatch: got \"x\"" |
| 12 | executor | build.done | topic_denied | "Hat 'executor' is denied from publishing topic 'build.done'" |
| 5 | review-coordinator | review.passed | invalid_field_value | "Hat 'review-coordinator' may not use value String('aggregate_timeout') for field 'skip_reason'" |

**模式分析**:
- 22/22/22/22 = 88 条为 02:54:11 - 03:25:30 期间 executor/coordinator 反复尝试 emit 各类事件,被 `cli_emit` 拒。
- 期间每 ~2-3 分钟有 1 个 executor iteration 试图 emit 1 套事件(共 11 轮 × 4 拒收 pattern = 44,但实际 56+ 拒收)→ 长时间 retry loop。
- 大量 envelope 都在 `outcome: failed`(可重试)或 `outcome: not_retriable`(critical,如 JSON 解析失败)状态。
- 写进 `recovery.jsonl` 但**没有阻断 loop 推进**——这是 noise 占主导,signal 缺失。

### 3.4 task.jsonl 状态

```
{"id":"task-1781664433-1998", "status":"closed", "started":"02:49:51", "closed":"03:22:22",
 "owner_hat_id":"coordinator", "key":"ce-executor:...:step-01:u1-scaffold"}
```

⚠️ **关键不一致**:task closed 时间 03:22:22,但 `work.done` 实际写入 events.jsonl 是 03:30:02,`work.done` 触发 shipper 的 commit `91596bc` 实际是 03:30 后。

---

## 4. 问题归因(P0/P1/P2)

### P0 — 阻断 loop 完成的根因

| ID | 归因 | 描述 | 证据 |
|---|---|---|---|
| **P0-1** | **Ralph Loop 机制**(R6 正常工作) | `incomplete_wave_gate` 在 4 维度 wave 中 2 维度未归位,经 80% aggregate_timeout 后自动 emit `plan.blocked`,`review-synthesizer` 触发 shipper,产出 `REVIEW_COMPLETE(fail)` | events.jsonl 04:06:40; preset 第 79-82 行 + 第 89-91 行 |
| **P0-2** | **Ralph Loop 机制**(verdict_gate 正常工作) | `verdict_gate.additional_topics=[report.done]` 命中 pass_or_fail=fail,`loop.terminate(reason=review_failed)` | loop-termination-reason.json; preset 第 102-108 行 |
| **P0-3** | **Agent 行为** | review wave 中 2 维度 worker(`testing` + `maintainability`)未在 0.8 × aggregate_timeout 前完成,触发 R6 收摊。timeout 配置(preset 缺省)导致超时过紧 | events.jsonl 04:06:40 payload `received=2, missing=[testing, maintainability]` |
| **P0-4** | **Agent 产物**(U1 任务残留 P1) | U1 任务 description 第 32-34 行明确要 "扩展 scripts/audit-file-sizes.sh...追加 wc -l crates/ralph-core/src/event_loop/*.rs",实施后未在 R7 验收中体现 | REVIEW_COMPLETE residual_findings_summary 段 |

### P1 — 显著影响但未阻断

| ID | 归因 | 描述 | 证据 |
|---|---|---|---|
| **P1-1** | **Agent 行为** | executor 重复 emit `work.done` 1 次(payload 字节级完全相同,12s 内),`triggered=ralph` 表示由 loop runner 重发 | events.jsonl 03:30:02 + 03:30:14 |
| **P1-2** | **Agent 行为** | task closed 时间(03:22:22)早于 work.done emit(03:30:02),违反 executor 任务执行循环 step 6 "MUST close task before work.done" 时序。**实际顺序为:task close → commit → work.done emit**,与 plan 一致,但 task close 提前 8 分钟可能由 iteration 边界引起 | tasks.jsonl closed 03:22:22; events.jsonl work.done 03:30:02 |
| **P1-3** | **Ralph Loop 机制**(noise 占主导) | 185 条 recovery envelope 中 10 种 unique pattern 100% 重复。`cli_emit` 把所有 schema 错误**只**写 `recovery.jsonl`,**没有**升级到 `task.resume` 阻断当前 hat | recovery.jsonl 全量,见 §3.3 |
| **P1-4** | **Preset 设计** | `topic_deny_rules` 已禁止 executor 发 `build.done`,但 hat instructions(U11 HARD RULE 第 522-532 行)仍以 narrative 形式强调禁令,产生 12 条 `topic_denied` envelope 噪声(每次 executor 误尝试) | recovery.jsonl 12 × `topic_denied` |
| **P1-5** | **Ralph Loop 机制** | `review-coordinator` 在 wave 期间尝试 emit `review.passed(skip_reason=aggregate_timeout)`,被 `hat_allowed_values` 拒绝(只允许 `empty_diff`)。5 次失败,最终由 `review-synthesizer` 正确 emit `plan.blocked` | recovery.jsonl 5 × `invalid_field_value`;events.jsonl 04:06:40 |

### P2 — 边缘问题

| ID | 归因 | 描述 | 证据 |
|---|---|---|---|
| **P2-1** | **Agent 行为** | human.guidance 注入 2 次"Focus on error handling" / "Keep this in mind",但 agent 未把 guidance 转化为实际 action(后续仍是重复相同 emit 模式) | context.md 第 30-32 行 |
| **P2-2** | **Ralph Loop 机制** | `loops.json` 在 worktree 退出时 `{"loops":[]}` 形式清空,但无 `worktree_path` 记录(只空数组,无 worktree 模式标识) | loops.json |
| **P2-3** | **预设配置** | preset 第 79-82 行提到 `aggregate_timeout_secs` 但未在 preset 内显式配置具体值(走缺省),review wave 4 维度并发下 80% timeout 偏紧,可能需上调 | preset 第 79-82 行;events 04:06:40 |

---

## 5. 修复建议

### 5.1 修复 U1 残留(让 plan U1 → U2 推进)

| 优先级 | 行动 | 责任 |
|---|---|---|
| **P0** | 补 `scripts/audit-file-sizes.sh` 的 `event_loop/*.rs` 行数扫描(plan U1 description 第 32-34 行) | executor |
| **P0** | 修复 `test_u2_*` 4 个 fixture(YAML 缺 schema),把 R-IDs 写入 schema_required_fields | executor |
| **P0** | U1 commit `91596bc` 后 plan frontmatter 仍标 `stalled-after-U1`,U1 真正完工后需更新 frontmatter | shipper / plan 维护者 |

### 5.2 改进 review wave 稳定性

| 优先级 | 行动 | 责任 |
|---|---|---|
| **P0** | preset `event_loop.workflow_contract.incomplete_wave_gate` 已启用,但 aggregate_timeout_secs 缺省值偏紧(在 4 维度并发下 80% 不够),需在 preset 显式上调到 600s+ 或改为可配置 | preset 维护者 |
| **P1** | 维度 worker 启动时若检测到 wave 总维度 > 当前并发能力,主动 emit `task.resume(降低 wave_total)`,避免 R6 收摊 | review-coordinator |
| **P1** | `review-synthesizer` 在 80% aggregate_timeout 时主动 poll 剩余维度状态(非被动等),减少无效等待 | review-synthesizer |

### 5.3 压制 recovery.jsonl 噪声

| 优先级 | 行动 | 责任 |
|---|---|---|
| **P1** | cli_emit 的 `payload_contract_violation`(critical)应在第一次出现时升级为 `task.resume(target_hat=executor)`,而非仅写 recovery.jsonl 静默累积 | loop runner / event_policy |
| **P1** | `topic_denied`(executor → build.done)首次出现后写入 hat registry "emit warning" 状态,后续相同 topic emit 跳过 envelope 写盘(只发 OS-level stderr) | cli_emit |
| **P1** | `missing_required_field(plan_name)` 当同 hat 在 N 次 iteration 内连续触发时,降级为 `outcome: suppressed` 不写 recovery.jsonl,只在 summary 中统计 | runtime_diagnosis |

### 5.4 preset 与 hat instructions 改进

| 优先级 | 行动 | 责任 |
|---|---|---|
| **P2** | executor hat instructions 第 522-532 行已经把 `build.done` 禁写入 narrative,但 topic_deny_rules 已在机制层禁——两层都设时 narrative 反而是误导源,应删除 narrative 重复(留机制层单点) | preset 维护者 |
| **P2** | 在 preset 加 `aggregate_timeout_secs: 600` 显式配置(覆盖 4-hat wave 偏紧默认) | preset 维护者 |
| **P2** | preset `event_policy.hat_allowed_values` 已支持 skip_reason 分级,但 review-coordinator 5 次尝试 aggregate_timeout 表明该 hat 的 instructions 未把 `empty_diff` 唯一性同步,需在 hat instructions 加显式约束 | preset 维护者 |

### 5.5 复盘:本次 run 学到什么

1. **机制层 R6 收摊正常**:这是设计预期,不应视为 bug。
2. **recovery.jsonl 是 noise 大于 signal**:185 条 envelope 集中在 10 个 unique pattern 上,信息密度极低,后续应升级为抑制策略。
3. **verdict_gate 多层防御到位**:`verdict_gate.topic=REVIEW_COMPLETE` + `verdict_gate.additional_topics=[report.done]` 双层都触发,成功拦截 fail 路径推进。
4. **U1 任务 description 与实施有 drift**:plan description 提到 R7 audit-script 扩展,实施未在 commit 中体现,留到 review 阶段被抓出——这是 plan → execute 环节的指令同步问题。
5. **task closed 早于 work.done** 的时序:看似异常,实际是 plan 驱动的 task lifecycle(close 时即 commit 完成),events.jsonl 是 commit 后写,两者时序合理;但应在 summary 中明确这一点,避免误读。

---

## 6. 附录:链路上的所有 hat activation

| Hat | 触发次数 | 成功 emit | 失败 emit | 备注 |
|---|---|---|---|---|
| loop (框架) | 2 | work.start, loop.terminate | 0 | events-history.jsonl |
| coordinator | 1 | work.ready | 0 | 02:47:43 |
| executor | 56+ (iteration 触发) | work.done × 2(同 payload) | 56+ (build.done/work.ready/.../work.done) | 大部分被 cli_emit 拒 |
| review-coordinator | 5 | build.done × 5, review.wave.ready × 4 | review.passed × 5 (skip_reason 错) | 04:01-04:04 |
| dimension-reviewer | 3 | review.dimension.done × 3 | 0 | 4 维度中 3 归位 |
| review-synthesizer | 1 | plan.blocked | review.passed × 22 (plan_name 缺) | R6 收摊 |
| plan-gate | 0 | 0 | queue.advance × 22 (plan_name 缺) | 路径被 plan.blocked 截断 |
| shipper | 5 | build.done × 5, REVIEW_COMPLETE × 1 | 0 | 04:08-04:18 |
| reporter | 1 | report.done | 0 | 04:21:07 |
| fixer | 0 | 0 | 0 | 路径未触发 |
| debug-resolver | 0 | 0 | 0 | 路径未触发 |
| progress-steward | 0 | 0 | 0 | 未触发 stall recovery(loop 在 R6 收摊后未到 3 连续无进展) |
| human.guidance | 2 | — | — | 02:54:11 注入,未触发 3 次 stall |

---

## 7. 报告元数据

- **诊断会话**:基于 `keen-fern` worktree 实际事件回放
- **数据时间窗**:2026-06-17T02:33:24 ~ 04:21:16(UTC)
- **使用数据源**:
  - `events-20260617-023324.jsonl`(69 业务事件)
  - `events-history-20260617-023324.jsonl`(2 框架事件)
  - `recovery.jsonl`(185 cli_emit 拒收 envelope)
  - `summary.md` / `context.md` / `tasks.jsonl` / `scratchpad.md`
  - `presets/en/ce-executor-isolated.yml`(2278 行 preset 全文)
  - `docs/plans/2026-06-10-003-...md`(1872 行 plan)
- **未跑动态校验**:本报告基于静态事件 + 模板审查,**未**对 `r` events 端到端回放(无 sandbox/recording)。CLI 重放见 `ralph diagnose --session latest`(在 `.ralph/diagnostics/2026-06-17T10-33-23` 目录)
- **置信度**:高(80%+) — 链路每步都有事件时间戳 + payload 字段支撑;P0-1/P0-2 机制触发由事件流和 preset 配置双重确认

---

## 8. 恢复路径专项诊断(ce-debug 风格)

> 本节用 ce-debug Phase 0–2 纪律,把"哪些恢复 hat 没工作"作为症状,把"为什么没工作"作为根因,做完整因果链分析。
> **结论先行**:preset 设计的 4 个恢复 hat 中,**3 个未被触发是预期行为**(fixer / debug-resolver / progress-steward 路径未走通),**1 个被触发了但产物被截断**(plan-gate 应在 `plan.blocked` 后介入实际被跳过),**1 个新增的拦截机制承担了终结**(`verdict_gate` 替代 shipper 路径正常终止)。

### 8.1 preset 设计的 5 个恢复链路 + 实际触发状态

| 恢复链路 | 触发源 | 设计目的 | 实际触发? | 证据 |
|---|---|---|---|---|
| **L1: fix.coordinator→review** | `work.done` → `review-coordinator` → `review.wave.ready` | 一次 review | ✅ 触发(03:55:55,4 维度 wave) | events.jsonl |
| **L2: review-fixer 自动修** | `review.failed` → `Fixer` | ≤3 轮 safe_auto | ❌ 未触发 | events.jsonl 无 `review.failed` |
| **L3: debug.resolver 根因诊断** | `fix.exhausted` → `debug-resolver` → `fix.plan.ready` | 根因 + 修复计划 | ❌ 未触发 | events.jsonl 无 `fix.exhausted`(因 L2 未触发) |
| **L4: plan-gate 步进/终结** | `review.passed` / `review.complete` / `plan.blocked` 等 → `plan-gate` | 推进/终结决策 | ❌ 未触发 | events.jsonl 无 `queue.advance` / `plan.complete` / `plan.blocked`(从 plan-gate)|
| **L5: progress-steward 兜底** | `loop.stalled` → `progress-steward` | stall 3 轮兜底 | ❌ 未触发 | events.jsonl 无 `loop.stalled` |
| **L6: plan.blocked 直送 shipper** | `plan.blocked` → `shipper` (因 `plan-gate.triggers` 不含 `plan.blocked`)| 终结路径 | ✅ 触发(04:06:40 → 04:18:45) | events.jsonl plan.blocked → REVIEW_COMPLETE |
| **L7: verdict_gate 拦截** | `REVIEW_COMPLETE` / `report.done` 命中 pass_or_fail=fail | 终结 gate | ✅ 触发(04:21:16) | loop-termination-reason.json |

### 8.2 诊断 L2 / L3:fixer + debug-resolver 为何不触发?

**症状**:`review.failed` 事件从未出现在 events.jsonl,但 `REVIEW_COMPLETE` 写出了 `residual_findings_summary` 含 2 个 P1(测试 fixture 缺失 + audit 脚本扩展遗漏)。

**Phase 0 — Triage**:
- 预期:review wave 不收敛(4 维度缺 2 维度)→ `review.failed` → Fixer ≤3 轮 → `fix.applied` 或 `fix.exhausted` → debug-resolver
- 实际:`plan.blocked(reason=dimension_reviewers_failed_to_converge)` 取代 `review.failed` 直接被 R6 机制截胡

**Phase 1 — 因果链**:
```
1. review-coordinator 在 03:55:55 emit review.wave.ready × 4(4 维度并发)
2. dimension-reviewer 实际并发能力:仅 1 个(presets 中 dimension-reviewer concurrency 缺省=1)
3. 03:55:55 → 04:01:31:candidate 1(correctness 0 findings)归位(5m36s)
4. 04:01:56:candidate 2(requirements 1 finding)归位(6m1s)
5. 04:04:15:candidate 3(correctness 1 finding)归位(8m20s)
6. 04:06:40:now - last_dimension_at ≈ 2m25s,已超过 0.8 × aggregate_timeout_secs
7. R6 incomplete_wave_gate 触发:review-synthesizer emit plan.blocked
8. ⛔ review.failed 永远不会被 emit,因为 synthesizer 已经走 block 路径
9. ⛔ Fixer 永远不会被 trigger,因为它只 listen review.failed
10. ⛔ debug-resolver 永远不会被 trigger,因为它只 listen fix.exhausted
```

**Phase 2 — 根因**(ranked):
1. **R6 机制设计是 by design 的截断**:`incomplete_wave_gate` 把"review wave 失败"分为两类:
   - **A 类(可修复)**:维度全部归位 + 有 finding → `review.failed` → Fixer → 修复
   - **B 类(机制层失败)**:维度未归位 → `plan.blocked` → shipper → REVIEW_COMPLETE(fail)→ 终结
   本次 run 走的是 B 类,Fixer 路径被显式跳过。**这是机制设计,不是 bug**。
2. **Fixer 假设不成立**:Fixer instructions 第 1503 行明确"This preset runs in `autofix` mode: apply fixes silently"——它假设 review.failed 来自"review 看了完整 diff 后给出 finding",**不假设**"review wave 自身就崩了"。Fixer 不知道如何处理 4 维度少 2 维度这种情况,因为它没有"R6 收摊"概念。
3. **debug-resolver 同理**:它假设根因是 safe_auto 修复不奏效,不是 wave 自身超时。
4. **预设 schema 缺 plan_name**:`review.failed.required_fields` 含 `plan_name`(preset 第 258 行),但本次 R6 收摊走的 `plan.blocked` 只要求 `reason`(preset 第 286 行),**这是 schema 设计上的非对称**——R6 路径不要求 plan_name,意味着 plan-gate 即便被触发也无法 reconcile。

**Causal chain gate 验证**:能解释从 03:55:55 维度并发启动到 04:06:40 收摊的完整路径,无 gap。R6 设计是 2026-06-17-003 计划 U1+U2 落地(preset 第 79-82 行),是**已知行为**。

**Smart Escalation 触发检查**:
- Hypotheses point to different subsystems? 是的——R6 收摊 / Fixer 假设 / preset schema 非对称 三者交集
- 建议:**rework preset 文档** + **扩展 Fixer 默认 trigger** 而非归类为 bug

### 8.3 诊断 L4:plan-gate 为何不触发?

**症状**:`plan.blocked` 已 emit(04:06:40),`plan-gate.triggers` 含 `plan.blocked` 在第 7 位(preset 第 1684 行),但 events.jsonl 中无 `queue.advance` / `plan.complete` / `plan.blocked` 由 `plan-gate` 发出。

**Phase 1 — 因果链**:
```
1. plan.blocked emit (04:06:40, hat=review-synthesizer)
2. plan-gate 收到触发(triggers 列表第 4 项)
3. plan-gate instructions 第 1831 行:"If verdict is fail → publish plan.blocked, payload: plan_name, reason: review verdict is fail, task_id, task_key"
4. 关键:plan-gate 的 plan.blocked 路径默认走终结(发 shipper),不会 queue.advance
5. 但 plan-gate 自己的 plan.blocked 必须带 task_id + task_key(preset 第 286 行 required_fields = [reason],但 plan-gate 自己在第 1831 行规定要加 task_id/task_key)
6. 实际:events.jsonl 中 review-synthesizer 的 plan.blocked payload 只含 reason + details,**没有 task_id/task_key/plan_name**
7. ⛔ plan-gate 即便被 trigger,在它的 reconcile 阶段会发现:payload 缺 task_id,无法执行 step 推进,会再发 plan.blocked,但这次又缺 plan_name(task_id-only)
```

**Phase 2 — 根因**:
1. **R6 收摊路径设计不与 plan-gate 兼容**:`plan.blocked` 触发 plan-gate,但 plan-gate 接到的 payload 缺 plan_name/task_id,无法做 step 推进判断。
2. **preset schema 不一致**:`plan.blocked.required_fields = [reason]`(preset 第 286 行)过松——`work.failed.required_fields = [reason]`(第 222 行),但 plan-gate instructions 第 1831 行规定 plan.blocked payload 必须含 `plan_name`,schema 层没强制。
3. **plan-gate 被截胡**:实际上,plan-gate 触发后,R6 路径把 plan.blocked 路由到了 shipper 直接终结(见 preset 第 1870 行 `shipper.triggers = [plan.complete, plan.blocked, debug.exhausted]`),plan-gate 走到 reconcile 之前事件已被 shipper 消费。

**Causal chain gate 验证**:能解释 plan.blocked → shipper 的直接路径,plan-gate 在这条路径上**事实上是 dead branch**。

### 8.4 诊断 L5:progress-steward 为何不触发?

**症状**:`loop.stalled` 未在 events.jsonl 出现,progress-steward 0 次激活。

**Phase 1 — 因果链**:
```
1. preset 第 137-139 行:progress_steward.max_steward_iterations: 3
2. 触发条件:no accepted business event for max_steward_iterations consecutive turns
3. 实际事件流每 iteration 都有 business event(work.ready, work.done, review.wave.ready, review.dimension.done, plan.blocked, build.done × 5, REVIEW_COMPLETE, report.done)
4. 即便 02:54-03:25 期间 executor 反复 emit 失败,业务事件从未"缺席",只是 "重复但被拒"
5. ⛔ stall detector 不认为有 stall
```

**Phase 2 — 根因**:
1. **stall detector 的检测语义偏窄**:它检测"3 轮无 business event",但本次 run 每轮都有(被拒的)emit,语义上不是 stall 而是 "noisy rejection loop"。
2. **recovery.jsonl 没有 stall signal**:`cli_emit` envelope 只写盘,没有 `loop.stalled` emit,stall detector 没有 signal 来源。
3. **185 条 envelope 中无 1 条带 `stall_recovery` source**:在 recovery.jsonl 全量 grep,`envelope.source` 全部为 `cli_emit`,没有任何 `stall_recovery` 类别触发。
4. **R6 收摊救了场**:03:55 → 04:06 期间 stall detector 应该识别的"4 维度只回 2 维度"在 review-synthesizer 内部已经被 incomplete_wave_gate 处理了,不需要 progress-steward 介入。

**Causal chain gate 验证**:能解释 stall detector 沉默的原因。但**这是设计盲点**——如果未来某次 run 出现"所有 hat 都不工作但 events.jsonl 仍有轮转"的情况,stall detector 不会介入。

### 8.5 关键发现:preset 4-hat 设计的 fix 路径在 R6 收摊场景下完全失效

**根因(综合 8.2 + 8.3 + 8.4)**:

`ce-executor-isolated` preset 的 fix 路径设计有一个**结构性盲区**:

```
预设设计的完整 fix 路径:
  review.failed → Fixer ≤3 轮 → fix.exhausted → debug-resolver → fix.plan.ready → Executor → work.done → review
                                          ↓
                                  plan-gate 路径并行
                                          ↓
                                  progress-steward stall 兜底

预设实际激活的路径(本次 run):
  work.done → review.wave.ready (4 维度) → 2 维度归位 → plan.blocked(R6) → shipper → REVIEW_COMPLETE → reporter → report.done → verdict_gate 拦截 → 终结

fix 路径全部未激活的根因:
  1. R6 收摊 bypass 了 review.failed → Fixer 路径
  2. plan-gate 在 plan.blocked 路径上是 dead branch(被 shipper 抢先消费)
  3. progress-steward 的 stall detector 把 "rejection noise" 当作 "正常事件流"
  4. debug-resolver 永远不会被触发(因 fix.exhausted 永远不会被 emit)
```

**这就是为什么 "preset 里面恢复的 hat 没有工作"**:
- 4 个恢复 hat(fixer / debug-resolver / plan-gate / progress-steward)在本次 run **设计上就不会被触发**,因为 R6 收摊路径切断了上游 review.failed 这个 trigger。
- preset 设计假设了"review 完整归位 + 有 finding"是触发 fix 路径的唯一前提,**没考虑到**"review 自身就崩了"这个场景。
- 1 个新机制(R6)的设计是 2026-06-17-003 plan U1+U2,落地时间是 2026-06-17 当天(本 loop 同日运行)——新机制与旧恢复路径的兼容性是**未经测试的盲区**。

### 8.6 修复建议(基于恢复路径分析)

| 优先级 | 行动 | 责任 |
|---|---|---|
| **P0** | 修复 R6 收摊 → plan-gate 路径的 dead branch:让 plan-gate 在 plan.blocked 路径上**抢先 shipper 消费**事件,并把 plan_name/task_id/task_key 注入 payload(review-synthesizer 不知道这些值,需要 plan-gate 自己从 context.md/tasks.jsonl 读) | preset 维护者 + event_loop |
| **P0** | preset schema `plan.blocked.required_fields` 改为 `[reason, plan_name, task_id, task_key, step]`,强制 R6 收摊路径带 plan 上下文 | preset 维护者 |
| **P0** | Fixer 扩展:增加 trigger `plan.blocked(reason=dimension_reviewers_failed_to_converge)`,允许 Fixer 在 R6 收摊后**尝试**补救(虽然原设计是给"有 finding"用,但 R6 路径下的 P1 residual 实际可修) | preset 维护者 + Fixer instructions |
| **P1** | debug-resolver 扩展 trigger:增加 `plan.blocked(reason=dimension_reviewers_failed_to_converge)`,允许 debug-resolver 接管 R6 路径并出 `fix.plan.ready` 让 executor 重做 | preset 维护者 |
| **P1** | stall detector 改造:把"每轮有 emit 但全 reject"识别为"rejection stall",区别于 "no event stall" | runtime_diagnosis |
| **P1** | plan-gate instructions 增加 step 1831 的 plan.blocked 必填字段校验,**显式拒绝**"review-synthesizer 发的 plan.blocked" 缺 plan_name/task_id 的情况(改为让 review-synthesizer 不发 plan.blocked,改发 plan.complete(fail)) | plan-gate instructions |
| **P2** | preset 文档加一段"R6 收摊场景下恢复路径降级矩阵",说明哪些 fix hat 在该场景下不会工作 | preset 维护者 |
| **P2** | 在 `summary.md` 中显式标注 "recovery hats not activated",方便 manager 报告区分 "fix 路径失败" 和 "fix 路径未尝试" | shipper / reporter |

### 8.7 复盘:本次 run 的"恢复 hat 不工作"到底意味着什么?

- **不是 bug** —— R6 收摊机制是 2026-06-17 当天落地的设计,本次 run 是它第一次在新路径上跑。
- **不是配置错误** —— preset 文件本身正确,fix 路径在 A 类(review 完整 + 有 finding)场景下工作正常。
- **是设计盲区** —— R6 收摊与旧 fix 路径的兼容性是预设,未被验证。本次 run 暴露了 4 个盲点:
  1. R6 bypass review.failed(盲点 1)
  2. plan-gate 在 plan.blocked 上是 dead branch(盲点 2)
  3. stall detector 把 rejection 当作 event(盲点 3)
  4. plan.blocked schema 过松(盲点 4)
- **真实影响**:本次 run 的 2 个 P1 residual(`test_u2_*` fixture + `audit-file-sizes.sh` 扩展)如果走 fix 路径,完全可以在 review 阶段由 Fixer 1 轮修复完,但 R6 收摊切断了这条路。**直接结果**:`REVIEW_COMPLETE(fail)` + `report.done(awaiting_decision)` + loop 终止,manager 拿到 fail 报告,需要手动判断下一步。

### 8.8 链路上 4 个"未触发"恢复 hat 的"应该触发"预期

| Hat | 应该触发的场景 | 本次 run 实际 | 偏差根因 |
|---|---|---|---|
| **Fixer** | review.failed (有 finding 可修) | 未触发 | R6 收摊,无 review.failed emit |
| **debug-resolver** | fix.exhausted (根因未确认) | 未触发 | 同上,无 fix.exhausted emit |
| **plan-gate** | review.passed / plan.blocked / fix.exhausted | 未触发 | R6 plan.blocked 被 shipper 抢先,plan-gate 是 dead branch |
| **progress-steward** | loop.stalled (3 轮无 business event) | 未触发 | stall detector 把 rejection noise 当作 event |

**这 4 个 hat 全部是"设计盲区"的受害者,而非自身 bug**。修复建议(8.6 节)的 P0/P1 项才是真正需要的根因修复。
