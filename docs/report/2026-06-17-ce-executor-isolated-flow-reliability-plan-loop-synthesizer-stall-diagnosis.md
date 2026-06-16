# ce-executor-isolated Review-passed Bypass 链路诊断报告

> 📅 2026-06-17 | 🔖 loop `2026-06-17-001-feat-ce-executor-flow-reliability-plan-zippy-sparrow`(PID 603653, started 2026-06-16T03:06:37Z) | preset `ce-executor-isolated` | plan `2026-06-17-001-feat-ce-executor-flow-reliability-plan.md`

---

## 1. TL;DR — 一句话定位

**`review-coordinator` agent 在 04:06:52 emit `review.passed(skip_reason="empty_diff")` 走 fast-path 收尾 iteration,但 `ReviewStepTracker` 中 `open_wave_id` 仍指向未收敛的 `w-18b973005abbc281-700695-0`(4/11 维度已收),`check_semantic_gates()` 触发 `review_passed_while_wave_open` 守门将事件标 `InvalidFieldValue { field: "skip_reason", value: "review_passed_while_wave_open" }`,最终 `payload-contract-error-2026-06-16T04-08-04-...json` 落地,loop_runner 在 04:08:04 终止 (`TerminationReason::PayloadContractViolation`),5 iterations,review.passed 通路从未合法闭合**。

叠加 4 项二级偏离(均独立可证):

| 关注点 | 结论 | 证据(绝对路径) |
|---|---|---|
| **synthesizer 永不 fire** | 是(11→4 维中途 stall) | events.jsonl L5-19:11 wave_ready + 仅 4 dim_done;synthesizer/plan-gate/shipper/reporter 全空 |
| **executor 二次重发 work.done** | 是(异常) | events.jsonl L20:03:54:50 `triggered=review-synthesizer` 元数据(不在 schema) |
| **stall_recovery 4 次 escalation** | 是 | `.ralph/diagnostics/2026-06-16T11-06-37/recovery.jsonl` L4-7,03:55:17 同一秒 4 次 `handoff_dispatch_timeout` 升级 |
| **DEC-002 confidence 70 走 fast-path** | 是(根因) | `.agents/scratchpad/.../decisions.md:15-32` 显式记录 "复用 last_reviewed_sha 走 empty_diff 通路" |

---

## 2. 流程还原:预设 vs 实际

### 2.1 预设(`presets/en/ce-executor-isolated.yml`)期望链路

```
work.start (loop 启动)
   ↓
coordinator                              (triggers: work.start)
   ↓ work.ready
executor                                 (triggers: work.ready, fix.plan.ready)
   ↓ work.done
review-coordinator                       (triggers: work.done, fix.applied)
   ├─ empty diff → review.passed(skip_reason="empty_diff")
   └─ non-trivial → review.wave.ready (wave emit N dims 一次性)
        ↓
        dimension-reviewer × N            (concurrency: 9; wave_id w-...)
             ↓ review.dimension.done × N
        review-synthesizer               (aggregate: wait_for_all timeout 1800s)
             ├─ 完整齐 → review.passed / failed / complete
             ├─ 部分齐 + U6 incompleteness → plan.blocked
             └─ 超时 → review.failed(skip_reason=aggregate_timeout)
plan-gate                                (triggers: review.passed, review.complete, work.failed, queue.advance, loop.cancel)
   ↓ queue.advance + work.ready (dual-publish, WAC-U4)
executor (下一步)
   ... 循环
   ↓ plan.complete
shipper → REVIEW_COMPLETE → reporter → report.done → LOOP_COMPLETE
```

**关键约束**(preset + plan §Non-Regression):
- `event_policy.hat_allowed_values.skip_reason.review-coordinator = ["empty_diff"]`(L222-225),`review-synthesizer = ["aggregate_timeout"]`(L226-227)
- `event_policy.allowed_values.skip_reason = ["empty_diff", "trivial_step", "aggregate_timeout"]`(L215-216, hat-agnostic 兜底)
- U1 trivial_step semantic gate:`review.passed(skip_reason=trivial_step)` 在 findings_count > 0 或 changed_lines ≥ 50 时拒
- **U6 Completeness Check**(plan §Implementation Units U4 + preset instructions L1312-1354):synthesizer 收到 `received < expected` 时 emit `plan.blocked(reason="dimension_reviewers_failed_to_converge")`,**永不**伪造 `review.passed`
- review-coordinator DEC-002 hard rule:empty diff 通路要求 `commit_count == 0 && changed_lines == 0 && untracked empty && diff empty`(preset L742-746),**且** `last_reviewed_sha` 写入发生在前一波 wave 闭合之后

### 2.2 实际(`events-20260616-030637.jsonl`,21 行 + diagnostics 10 行 recovery)链路

```text
work.start (loop_started)                             03:06:37
   ↓
coordinator → work.ready                              03:08:33  (L1, R4 single-U; task-1781579295-5308)
   ↓
executor (探测/失败注入 6 次)                          03:39:34  [events L2-3 + recovery.jsonl L1-6]
   ├─ executor → build.done × 2  → topic_denied
   ├─ executor → work.ready → payload_contract_violation (JSON parse)
   ├─ executor → work.done × 3  → plan_name mismatch / missing field / JSON parse
   └─ coordinator → work.ready → payload_contract_violation
   ↓
executor → work.done                                  03:42:39  (L4; commit_count=1, changed_lines=1475)
   ↓
review-coordinator → review.wave.ready × 11           03:46:12  (L5-15; wave_id w-18b973005abbc281-700695-0, wave_total=11)
   ↓ 1 维/emit,统一 idempotency_key,11 events 共享 same wave
dimension-reviewer × 4 → review.dimension.done × 4   03:51:25~03:52:32  (L16-19)
   ├─ L16 correctness (wave_index=3)
   ├─ L17 adversarial (wave_index=2)
   ├─ L18 adversarial (wave_index=8)  ← 同 dim 重复 emit
   └─ L19 adversarial (wave_index=10, wave_total=11)  ← 同 dim 第 3 次
   ↓
synthesizer 缺 7 维度(correctness 1 次 + adversarial 3 次 = 4 dim,实际只 2 个 unique dim)
aggregator 等待 wait_for_all timeout 1800s
   ↓
stall_recovery × 4 handoff_dispatch_timeout           03:55:17  [diagnostics/recovery.jsonl L4-7]
   (每条 "consumer review-synthesizer did not activate within 30s")
   ↓ outcome: Pending → Repeated 升级
   ↓
executor 异常 emit work.done (重发)                   03:54:50  (L20; payload 缺 schema 必填,带 triggered=review-synthesizer 元数据)
   ↓ plan-gate 走 task.resume 派发到 review-coordinator 重新激活
review-coordinator 再激活,检测:
   ├─ last_reviewed_sha = 76e6d5c = HEAD (已在 wave emit 后被 persist)
   ├─ 上一轮 4/11 dim done 来自中途 abort
   └─ DEC-002 confidence=70:复用 last_reviewed_sha 走 empty_diff fast-path
review-coordinator → review.passed                   04:06:52  (L21; skip_reason="empty_diff", diff_base_fallback="last_reviewed_sha_equals_HEAD")
   ↓
ReviewStepTracker.check_semantic_gates()
   ├─ hat=review-coordinator & topic=review.passed (L126 命中)
   ├─ step_key_from_event 找到 state (53c8 仍 register 着 wave)
   ├─ wave_open(state) = true (open_wave_id=w-18b973005abbc281-700695-0 仍存在)
   └─ emit PolicyFinding { InvalidFieldValue { field: "skip_reason", value: "review_passed_while_wave_open" } }
   ↓
apply_event_policy_validation → capture_violation → finding_to_payload_contract_violation
   ├─ kind = AllowedValueMismatch (L1262-1265 mapping)
   ├─ field = "skip_reason" (synthesized from semantic gate)
   ├─ source_hat = ["review-coordinator", "review-synthesizer"] (validator 索引,topic→多 hat 都 publish review.passed)
   ├─ target_hat = ["plan-gate"] (plan-gate 订阅 review.passed)
   └─ 写入 diagnostics/payload-contract-error-2026-06-16T04-08-04-273632282+00-00.json
   ↓
loop_runner.rs:3403-3500
   ├─ write_payload_contract_violation_report → diagnostics/payload-contract-error-*.json
   ├─ record_recovery_envelope (severity=critical, outcome=NotRetriable)
   ├─ handle_termination(reason=PayloadContractViolation)
   └─ return Ok(TerminationReason::PayloadContractViolation)
loop terminate                                         04:08:04  (diagnosis-summary.json: total_iterations=5)
```

**关键偏离汇总**:
- 事件 #21 在 events.jsonl L21 落盘(被 event reader 接受),但 validator 在次轮 reader 中拒
- `source_hats` 是 validator 索引(所有 publish 该 topic 的 hat),**不是**单次 emit 的 hat 来源
- `source_hat` 显示 `["review-coordinator", "review-synthesizer"]` 是因为 preset L667 + L1177 都声明 `publishes: review.passed`
- `target_hat` 显示 `["plan-gate"]` 是因为 plan-gate `triggers: [review.passed, review.complete, ...]`

---

## 3. 证据清单(全部绝对路径)

### 3.1 worktree 主目录产物

| 文件 | 行/大小 | 关键发现 |
|---|---|---|
| `.ralph/events-20260616-030637.jsonl` | 21 行 | L1 work.ready / L2-3 debug.step / L4 work.done (1475 lines) / L5-15 review.wave.ready×11 / L16-19 review.dimension.done×4 / L20 work.done 二次 / L21 review.passed (empty_diff) |
| `.ralph/recovery.jsonl` | 6 条 | 03:39:34 集中 — executor 6 次 cli_emit 拒(topic_denied / plan_name mismatch / JSON parse / missing field);**无** reviewer 拒收 |
| `.ralph/loops.json` | 1 entry | PID 603653, workspace + worktree_path = `.worktrees/...zippy-sparrow` |
| `.ralph/agent/tasks.jsonl` | 1 条 | `task-1781579295-5308` status=closed (closed 03:42:32,与 L4 work.done 时间一致);**无** U3 task 创建 |
| `.ralph/agent/scratchpad.md` | 6 行 | 2 条 human guidance 重复("Focus on error handling" / "Keep this in mind") |
| `.ralph/agent/.events-hat-review-coordinator-...-3.jsonl.idempotency.jsonl` | 1 行 | wave emit dedup 记录 (wave_id=w-18b973005abbc281-700695-0, count=11, scope_key=0eb1bfac8f442cd0d228092999d632d2fbd965d2294603693e767a5009a24322) |
| `.ralph/history.jsonl` | 1 行 | loop_started |

### 3.2 worktree diagnostics 目录(U7 runtime diagnosis)

| 文件 | 大小 | 关键发现 |
|---|---|---|
| `.ralph/diagnostics/2026-06-16T11-06-37/diagnosis-summary.json` | 578B | schema_version=1, total_iterations=**5**, loop_terminated_at=04:08:04, recovery_count=0 (但 recovery.jsonl 内 10 条!) |
| `.ralph/diagnostics/2026-06-16T11-06-37/recovery.jsonl` | 10 条 | L1 agent_doc_sync info / L2-3 isolated_scope_violation (executor→debug.step,03:42:52) / **L4-7 4 条 stall_recovery handoff_dispatch_timeout for review-synthesizer (03:55:17)** / L8-9 drift_monitor outcome update (Pending → Repeated) / **L10 payload_contract_violation on review.passed (04:08:04)** |
| `.ralph/diagnostics/2026-06-16T11-06-37/drift.jsonl` | 0 行 | drift 检测未触发 |
| `.ralph/diagnostics/2026-06-16T11-06-37/active-activations.json` | 2B `[]` | **诊断**写时已空,印证 loop terminate |
| `.ralph/diagnostics/payload-contract-error-2026-06-16T04-08-04-273632282+00-00.json` | 0.6KB | `error_type: allowed_value_mismatch`, `topic: review.passed`, `field: skip_reason`, `source_hat: [review-coordinator, review-synthesizer]`, `target_hat: [plan-gate]`, `payload_excerpt` 含 `skip_reason=empty_diff` |

### 3.3 scratchpad 产物(coordinator 写入)

| 文件 | 大小 | 关键发现 |
|---|---|---|
| `.agents/scratchpad/ce-executor/.../context.md` | 6204B | 模板头 + complexity=large + R-IDs 表 + start_sha=7e13ec3 + **last_reviewed_sha: 76e6d5c6f19846bd0f0dee7e8df660ec76f9b1f4 (line 112)** |
| `.agents/scratchpad/ce-executor/.../plan.md` | 2926B | 9 step (step-01=U1 当前,step-02~09 占位) |
| `.agents/scratchpad/ce-executor/.../progress.md` | 2778B | **Active Wave 段 L9:未清空**(注释段标记 "Active Wave: (none)" 但物理仍写有 wave_id);Completed Steps 写 step-01/U1 done;R4 decision 引 DEC-001 |
| `.agents/scratchpad/ce-executor/.../decisions.md` | ~990B | **DEC-001** R4 single-U (confidence=95) + **DEC-002** empty_diff fast-path (**confidence=70**, 在 preset "50-80 继续但记录" 区间) |
| `.agents/scratchpad/ce-executor/.../findings-correctness-task-1781579295-5308.json` | 4786B | 4 findings (2P2+2P3),正常 task_id |
| `.agents/scratchpad/ce-executor/.../findings-correctness-task-1781522658-25ee.json` | 2709B | ⚠️ **错 task_id**(前 worktree 残留 `2026-06-17-002`) |
| `.agents/scratchpad/ce-executor/.../findings-correctness-dim-review-tmpwOfzt5.json` | 5192B | ⚠️ **临时文件名**,task_id 未写入 |
| `.agents/scratchpad/ce-executor/.../findings-adversarial-task-1781579295-5308.json` | 11378B | 8 findings (3P1+4P2+1P3),**文件被 3 次覆盖累积** (#17/#18/#19 都写此 file) |
| `.agents/scratchpad/ce-executor/.../findings-learnings-task-1781579295-5308.json` | 884B | 仅 1 finding (advisory) |
| `.agents/scratchpad/ce-executor/.../wave-diff.patch` | 71594B | review-coordinator 写出 1475 lines |

### 3.4 主仓 preset + SSOT schema (与 build artifact 对账)

| 文件 | 关键约束 |
|---|---|
| `presets/en/ce-executor-isolated.yml` L48-86 | `execution_mode: isolated`, `verdict_gate` REVIEW_COMPLETE + report.done |
| `presets/en/ce-executor-isolated.yml` L117-138 | `event_policy.mode: enforce`, `on_violation: reject_with_resume`, `require_policy_check_for_cli_emit: true`, `allow_unsafe_cli_emit: false` |
| `presets/en/ce-executor-isolated.yml` L148-159 | topic_deny_rules 含 `{ralph, review.passed}` 等 6 项 ralph hat 限制 |
| `presets/en/ce-executor-isolated.yml` L201-227 | review.passed schema:required_fields 8 个 + allowed_values=`["empty_diff","trivial_step","aggregate_timeout"]` + hat_allowed_values(review-coordinator=[empty_diff], review-synthesizer=[aggregate_timeout]) |
| `presets/en/ce-executor-isolated.yml` L667 | review-coordinator `publishes: [review.wave.ready, review.passed]` |
| `presets/en/ce-executor-isolated.yml` L1177 | review-synthesizer `publishes: [review.passed, review.failed, review.complete, plan.blocked]` |
| `presets/schemas/ce-executor-isolated.yml` (SSOT) | review.passed 与 inline preset 一致 |
| `target/debug/build/ralph-cli-ea60dd1e44de5893/out/presets/ce-executor-isolated.yml` | merged artifact: review.passed allowed_values 与 hat_allowed_values 都正确存在(empty_diff 在两个 list 中) |

### 3.5 根因唯一性证伪表

| 假设 | 是否成立 | 证据 |
|---|---|---|
| A. `empty_diff` 不在 preset 的 `allowed_values` 中导致拒收 | **不成立** | preset L213-216 显式 `["empty_diff", "trivial_step", "aggregate_timeout"]`;build artifact grep 确认 |
| B. `last_reviewed_sha_equals_HEAD` 不是合法 `diff_base_fallback` 值 | **不成立** | 该字段不在 `event_policy.schemas.review.passed.required_fields` 中,L685-703 + L709-730 的 hat-agnostic/hat-aware 都只检 `skip_reason` |
| C. `findings_count: 0` 触发 U1 trivial_step gate | **不成立** | L776-810 只对 `skip_reason == "trivial_step"` 触发,本事件 `skip_reason == "empty_diff"` |
| D. `hat_allowed_values` 不允许 `review-coordinator → empty_diff` | **不成立** | preset L222-225 显式 `hat_id: review-coordinator, values: [empty_diff]` |
| E. plan_name equality check 拒收 | **不成立** | L733-760 只对 `topic == "work.done"` 触发 |
| F. **U6 Completeness Check 在 Rust 端 enforcement,wave 未 closed 拒 `review.passed`** | **成立** | `crates/ralph-core/src/event_loop/review_step_state.rs:126-147` `check_semantic_gates` 命中 `hat==review-coordinator && topic==review.passed && wave_open(state)` → emit `review_passed_while_wave_open` |
| G. review-coordinator 走 task.resume 后再次激活是合规路径 | **部分成立** | task.resume 路由在 R5 Hard-gate 后已可路由回源 hat;但 agent 收到第二次 work.done 后的决策(走 empty_diff)是 DEC-002 主观判断,preset 没有强约束 "review-coordinator 收到重发 work.done 必须 emit plan-gate review,不能跳过 wave" |
| H. executor 二次 work.done 是非法 emit | **可能** | 事件 #20 03:54:50 `triggered=review-synthesizer` 字段不在 schema,但被 events.jsonl 接受;后续 L4 触发 source 字段等于事件 hat=executor,**execution_contract 应拒** 但似乎未生效 |

---

## 4. 执行链路对比图

```
                          预设期望                              实际(events.jsonl + diagnostics)
                          ─────                              ───────────────────────────────
work.start                触发 coordinator                    ✅ 03:06:37 loop_started
   ↓
coordinator.work.ready    emit                                ✅ 03:08:33
   ↓
executor (cold start)     实施 U1                             ⚠️ 03:39:34 6 次 cli_emit 探测失败注入(预期 preset 拦截)
   ↓
executor.work.done        emit                                ✅ 03:42:39 (1475 lines)
   ↓ handoff              instant                             ⚠️ 03:39-03:42 期间无明确 handoff timeout 记录
review-coordinator        接收 work.done                      ✅
   ↓
review-coordinator        emit 11 review.wave.ready            ✅ 03:46:12 (wave_id 一致, idempotency_key 复用)
.wave.ready × 11          (1 次 wave 全部维度)
   ↓
dimension-reviewer × 4    各自 emit review.dimension.done     ⚠️ 03:51-03:52 仅 4/11 收齐
                          (correctness ×1, adversarial ×3 重复)
   ↓
review-synthesizer        wait_for_all 1800s                  ❌ 永不 fire (4/11 缺,无 U6 completeness 触发)
   ↓ handoff 30s timeout  stall_recovery × 4                  ❌ escalation 03:55:17 (recovery.jsonl L4-7)
review-synthesizer        emit review.passed/failed/complete  ❌ 永不 fire
   ↓
executor.work.done (二次) agent 异常重发                      ❌ 03:54:50 triggered=review-synthesizer 元数据
   ↓ task.resume routing  ralph → review-coordinator          ⚠️ R5 Hard-gate routing 未防御 "重复 work.done"
review-coordinator        DEC-002 confidence=70 决策          ❌ 走 empty_diff fast-path
   ↓ 决策
review-coordinator        emit review.passed                  ⚠️ 04:06:52 (events.jsonl L21 落盘)
review.passed             skip_reason="empty_diff"
   ↓ next iteration read
ReviewStepTracker         check_semantic_gates                 ❌ 04:08:04 拒收:wave_open(w-18b973...-0) 仍为 true
                          → review_passed_while_wave_open
   ↓
loop_runner.rs:3403       write_payload_contract_violation_report
                          → diagnostics/payload-contract-error-*.json
   ↓
handle_termination        PayloadContractViolation              ❌ 04:08:04 loop terminate
                          total_iterations=5
plan-gate                 接收 verdict                        ❌ 永不 fire
shipper                   REVIEW_COMPLETE                     ❌ 永不 fire
reporter                  report.done + LOOP_COMPLETE         ❌ 永不 fire
```

**结论**:整条链在 `dimension-reviewer → review-synthesizer` 这一段断裂(11→4 维中途 stall);前面 4 步都成功推进;第 5 步(synthesizer 收齐/超时后降级)开始全断;synthesizer 缺失 7 维未触发 U6 Completeness Check → plan.blocked;executor 二次 work.done 触发 plan-gate task.resume;review-coordinator agent 走 DEC-002 fast-path(置信度 70,在 50-80 区间);`check_semantic_gates` 在 wave_open 状态下拒收 `review.passed`;loop terminate with `TerminationReason::PayloadContractViolation`。

---

## 5. 问题归因表(P0/P1/P2)

| 严重度 | 编号 | 问题 | 归因 | 证据(绝对路径) |
|---|---|---|---|---|
| **P0** | P0-1 | `check_semantic_gates` 在 `wave_open=true` 时拒收 `review.passed(empty_diff)` 触发 `payload_contract_violation`,loop 终止 | **Ralph 基座机制**(`review_step_state.rs:126-147` semantic gate 是正确防御,触发点准确) | `.ralph/diagnostics/payload-contract-error-...json` field=skip_reason / L21 events.jsonl / `crates/ralph-core/src/event_loop/review_step_state.rs:126-147` |
| **P0** | P0-2 | 11→4 维中途 stall 后,`review-synthesizer` 永不 fire,`U6 Completeness Check` 未在 Rust 端 enforcement(plan §U4 §Implementation Units 文字写了但代码端是 agent 自觉) | **preset 设计** + **Ralph 基座机制** | events.jsonl L5-19 (11→4) / `progress.md` L9 Active Wave 未清 / preset L1312-1354 仅 instructions,无 Rust enforcement |
| **P0** | P0-3 | stall_recovery 4 次 escalation 后,plan-gate 未启动,review-coordinator 二次激活走 DEC-002 fast-path | **Ralph 基座机制**(stall→task.resume 路由允许 review-coordinator 二次激活 + 接收重发 work.done) | diagnostics/recovery.jsonl L4-7 (4 次 stall) / events.jsonl L20 (executor 重发) / L21 (review-coordinator empty_diff) / `decisions.md:15-32` DEC-002 |
| **P1** | P1-1 | 4 维 dimension.done 中 adversarial 重复 3 次(wave_index 2/8/10 都写 adversarial),实际只 2 个 unique dimension(correctness+adversarial),其他 9 维(7 dim + 2 adversarial)worker 完全没跑 | **agent 行为**(dimension-reviewer worker 自定 wave_index 时不严格对位 wave_total) | events.jsonl L16-19 (4 dim done, 3 个 wave_index 全 adversarial) |
| **P1** | P1-2 | executor 二次 work.done (事件 #20,03:54:50) 被 events.jsonl 接受,payload 缺 schema 必填字段,`triggered=review-synthesizer` 元数据不在 schema | **Ralph 基座机制**(execution_contract 在 L4 已接受,二次没重新 contract check) **+ agent 行为** | events.jsonl L20 / preset L184-191 schema 8 必填,事件 #20 payload 缺 plan_path/task_id/task_key/step |
| **P1** | P1-3 | DEC-002 confidence=70 走 empty_diff fast-path,但 preset L742-746 要求 `commit_count==0 && changed_lines==0 && untracked empty && diff empty` 四条件 AND,实际 commit_count=1 changed_lines=1475(来自 work.done #4 二次重发,但事件 #4 是初次 commit),review-coordinator 在二次激活时检查 git state 应是非空 | **agent 行为**(违反 preset HARD RULE,但置信度 70 在 50-80 区间被 preset 允许继续) | preset L742-746 / `decisions.md:17` "复用 last_reviewed_sha 走 empty_diff 通路" / events.jsonl L4 commit_count=1 |
| **P1** | P1-4 | `progress.md` Active Wave 段未清空(注释段标记 "(none)" 但物理仍写 wave_id) | **agent 行为**(synthesizer 不 fire → progress.md 永不更新) | `.agents/scratchpad/.../progress.md:9` |
| **P2** | P2-1 | dimension-reviewer 写 findings-*.json 用错 task_id(`task-1781522658-25ee` 前 worktree 残留) + 临时文件名(`tmpwOfzt5`) | **agent 行为**(worker prompt 顶部 task_id 注入未强约束) | `findings-correctness-task-1781522658-25ee.json` / `findings-correctness-dim-review-tmpwOfzt5.json` |
| **P2** | P2-2 | executor 03:39 期间 6 次 cli_emit 拒收(build.done/work.ready/work.done),表明 agent 探测阶段反复试错 | **agent 行为**(executor prompt 缺少 "NEVER emit build.done/debug.step" 强约束,只 topic_deny_rules 兜底) | events.jsonl L2-3 / recovery.jsonl L1-6 / diagnostics/recovery.jsonl L2-3 |
| **P2** | P2-3 | `last_reviewed_sha` 字段在 wave emit 后被 persist,但 wave 未闭合(4/11 dim done 后终止) | **Ralph 基座机制**(`persist_last_reviewed_sha_after_terminal` 的 terminal 判定是 wave emit 完成,不是 wave aggregator 完成) | preset L919 / `context.md:112` last_reviewed_sha 写入 |
| **P2** | P2-4 | scratchpad human guidance 重复 2 次("Focus on error handling" / "Keep this in mind") | **Ralph 基座机制**(`ralph tools interact progress` 或类似 squash 逻辑缺失) | `.ralph/agent/scratchpad.md:2-7` |
| **P2** | P2-5 | diagnosis-summary.json `recovery_count=0` 但 diagnostics/recovery.jsonl 有 10 条 envelope | **Ralph 基座机制**(`finalize_recovery_diagnosis` 在 terminate 路径没正确汇总) | diagnosis-summary.json:11 / recovery.jsonl 10 行 |

---

## 6. 修复建议(分级)

### 6.1 P0 修复(必须,本 loop 已 terminate,预防下次同型)

#### P0-1: `check_semantic_gates` 错误归类为 `InvalidFieldValue`

**问题**:`review_step_state.rs:131-144` emit `ViolationType::InvalidFieldValue { field: "skip_reason", value: "review_passed_while_wave_open" }`。这个 value 不是真实的 `skip_reason` 值(`empty_diff` / `trivial_step` / `aggregate_timeout`),而是诊断分类标签。这导致:
- `finding_to_payload_contract_violation` 把它误映射为 `AllowedValueMismatch`
- `source_hats: [review-coordinator, review-synthesizer]` 是 validator 索引,不能定位真凶
- `payload-contract-error.json` 的 `payload_excerpt` 显示的是真实 payload(`skip_reason=empty_diff`),但 `field/value` 误导读者

**修复方案**(推荐 A):

```rust
// crates/ralph-core/src/event_loop/review_step_state.rs:131
// before:
return Some(PolicyFinding {
    topic: topic.to_string(),
    violation_type: ViolationType::InvalidFieldValue {
        field: "skip_reason".to_string(),
        value: Value::String("review_passed_while_wave_open".to_string()),
    },
    ...
});
// after (新增 violation 类型):
return Some(PolicyFinding {
    topic: topic.to_string(),
    violation_type: ViolationType::SemanticGateViolation {
        gate: "review_passed_while_wave_open".to_string(),
        context: format!(
            "open_wave_id={:?} received={}/{}",
            state.open_wave_id, state.dimensions_received.len(), state.wave_expected
        ),
    },
    ...
});
```

并在 `event_policy.rs` 的 `ViolationType` 枚举新增 `SemanticGateViolation { gate: String, context: String }`;`finding_to_payload_contract_violation` 把它映射为新的 `PayloadContractViolationKind::SemanticGateViolation`(独立于 AllowedValueMismatch)。

**位置**:`crates/ralph-core/src/event_loop/review_step_state.rs:131-144` + `crates/ralph-core/src/event_policy.rs:74` (新增 variant) + `crates/ralph-core/src/event_loop/mod.rs:1262-1265` (mapping)

**测试**:在 `crates/ralph-core/src/event_loop/tests/review_step_state.rs` 加 `test_review_passed_while_wave_open_emits_semantic_gate_violation_not_invalid_field_value`。

#### P0-2: U6 Completeness Check Rust 端 enforcement

**问题**:preset L1312-1354 写了 "synthesizer 必须 emit `plan.blocked(reason=dimension_reviewers_failed_to_converge)`",但这是 agent instructions,**没有 Rust 端 enforcement**。Synthesizer agent 不 fire,这条规则永远不触发。

**修复方案**(推荐 B):

在 `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` 的 `execute_wave` 末尾加 `aggregate_deadline` watcher:

```rust
// 伪代码
loop {
    if received_count == wave_total {
        break; // 完整 → normal aggregator 路径
    }
    if Instant::now() >= aggregate_deadline {
        // 不是 synthesizer,机制层自己 emit plan.blocked
        let block_event = Event::with_target("plan-gate")
            .topic("plan.blocked")
            .hat("review-synthesizer")  // 借 synthesizer hat provenance
            .payload(json!({
                "reason": "dimension_reviewers_failed_to_converge",
                "expected": wave_total,
                "received": received_count,
                "missing_dimensions": missing,
                "wave_id": wave_id,
            }))
            .build();
        bus.publish(block_event);
        break;
    }
    sleep(5s);  // 或 wait_for_event
}
```

或在 `loop_runner.rs::run_iteration` 阶段加 stall watchdog:若 `received_count` 在 80% `aggregate_timeout_secs` 内没增长,主动 emit `plan.blocked`。

**位置**:`crates/ralph-cli/src/loop_runner/wave/dispatcher.rs::execute_wave` + `crates/ralph-core/src/event_loop/mod.rs::inject_review_aggregate_timeouts`

**测试**:scenario `flow_reliability/aggregate_timeout_degraded.yml`(plan §Unit 5 已规划但未落地)

#### P0-3: stall_recovery escalation 后阻断 review-coordinator fast-path

**问题**:4 次 stall_recovery 后,synthesizer 仍不 fire,plan-gate 也没启动;task.resume 路由让 review-coordinator 二次激活,agent 走 DEC-002 fast-path 是合规的(agent 行为),但 preset 缺少 "wave 在 stall 时禁止 review-coordinator 走 empty_diff 通路" 的强约束。

**修复方案**(推荐 C):在 preset L735-748 的 empty_diff 通路判定条件中加:

```yaml
Emit `review.passed` (with `skip_reason: "empty_diff"`) ONLY when ALL of these are true:
- `commit_count == 0` ...(原有 4 条件)...
- **NEW: `last_reviewed_sha` 写入发生在前一波 `review.wave.ready` 闭合之后** (即 `state.open_wave_id` 为 None)
- **NEW: 上一波 wave 的 `received_count == wave_total`** (否则必须 emit plan.blocked)
```

并在 `check_semantic_gates` L126 增加 hard gate:如果 `received_count < wave_total`,agent **必须** emit `plan.blocked`,**禁止**走 empty_diff。

**位置**:`presets/en/ce-executor-isolated.yml:735-748` + `crates/ralph-core/src/event_loop/review_step_state.rs:126` (L129 加 `received_count < wave_expected` 检查)

**测试**:`crates/ralph-cli/src/loop_runner/tests.rs` 加 `test_review_coordinator_empty_diff_rejected_when_wave_incomplete`(需 cli-serial)。

### 6.2 P1 系列(显著改进,本计划 U2/U3/U4 范围)

#### P1-1: wave emit 按 dimension 去重

**问题**:4 维 dimension.done 中 adversarial 重复 3 次(wave_index 2/8/10),其他 6 维完全空缺。Wave emit idempotency 复用了同一 key,但 dispatch 时每个 worker 独立 spawn,wave_index 由 emit 时分配。

**修复**:在 `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs::dispatch_wave` spawn 前按 `(dimension, focus)` 去重;同 wave_id 同 dimension 不发第二次。

#### P1-2: executor 二次 work.done 防 bypass

**问题**:事件 #20 (03:54:50) executor 在 wave 中途 emit 二次 work.done,payload 缺 plan_path/task_id/task_key/step 等 schema 必填字段,`triggered=review-synthesizer` 元数据不在 schema 内,但 events.jsonl 接受了。

**修复**:在 `event_policy.rs::validate_event_with_hat` 增加同 `(loop_id, task_id, work.done)` 拒收:第一次 `work.done` 接受后,在 `state.work_done_observed_tasks: HashSet<String>` 加 task_id,第二次 emit 同 task_id 的 `work.done` 走 `RecoverableRejection`(可重试但 hint "请 emit `queue.advance` 或 `plan.complete`")。

**位置**:`crates/ralph-core/src/event_policy.rs` + `event_loop/mod.rs::state.work_done_observed_tasks`

#### P1-3: DEC-002 confidence 70 强制 validator 升级

**问题**:preset §Confidence 协议 `50-80 继续 + 记录`,但 DEC-002 走 fast-path 触发 P0 链。confidence 70 在 50-80 区间是允许的,但**该决策影响 isolated scope 的 wave 状态**,应升级到 >80 才允许继续。

**修复**:preset L295 confidence protocol 段加 "wave_open 状态下不允许 50-80 confidence 的决策" 硬规则:

```yaml
- "Confidence protocol: score decisions 0-100. >80 proceed autonomously; 50-80 proceed + document in .ralph/agent/decisions.md; <50 choose safe default + document. **Wave-open-state decisions require confidence >90 OR must emit plan.blocked/review.failed as fail-safe.**"
```

### 6.3 P2 系列(小修)

- **P2-1**:wave_prompt 注入 worker 时把 `RALPH_CURRENT_TASK_ID` 注入环境变量,worker bash 必须用此值写 findings file。
- **P2-2**:executor agent prompt 加 "NEVER emit build.done/debug.step/test.done/lint.done (topic_deny_rules 拦截会浪费 1 iteration)"。
- **P2-3**:`persist_last_reviewed_sha_after_terminal` 改成仅在 `wave_closed=true`(aggregator 收到全部 dimension.done)后才写,不在 `review.wave.ready` emit 后立刻写。
- **P2-4**:`ralph tools interact progress` 写 scratchpad.md 时去重。
- **P2-5**:`finalize_recovery_diagnosis` 在 terminate 路径汇总 recovery.jsonl envelope count 到 diagnosis-summary.json。

### 6.4 防御性测试建议

| 测试位置 | 内容 |
|---|---|
| `crates/ralph-core/src/event_loop/review_step_state.rs` 单测 | `review_passed_while_wave_open` 必须 emit `SemanticGateViolation`,**不**是 `InvalidFieldValue` |
| `crates/ralph-cli/src/loop_runner/tests.rs` (cli-serial) | 11 维 wave 收 4 维后 stall,1800s 后必须 emit `plan.blocked(reason=dimension_reviewers_failed_to_converge)`,**不**是 loop_stale |
| `crates/ralph-core/src/event_policy.rs` 单测 | 二次同 task_id work.done 必须 reject;新 `SemanticGateViolation` 不被映射为 `AllowedValueMismatch` |
| preset lint | `topic_deny_rules` + `event_policy.schemas` 必须每个 review.* topic 都有 schema |
| scenario `flow_reliability/review_passed_while_wave_open.yml` | 模拟 DEC-002 决策,验证 check_semantic_gates 拒收 + plan.blocked 通路 |
| scenario `flow_reliability/aggregate_timeout_degraded.yml` | 压缩时钟,验证 Unit 5 DegradedCompletionRouter 在 4/11 维度时正确 emit review.failed(skip_reason=aggregate_timeout) |

---

## 7. 一句话行动项

**先修 P0-1**:`review_step_state.rs` 把 `review_passed_while_wave_open` 的 `InvalidFieldValue` violation 改为新增的 `SemanticGateViolation` 类型,并在 `event_policy.rs::ViolationType` 加此变体,`finding_to_payload_contract_violation` 把它映射到独立的 `PayloadContractViolationKind::SemanticGateViolation`,**不**混入 `AllowedValueMismatch`。这样:
1. 拒收诊断 `field/value` 不再误导(`value=review_passed_while_wave_open` vs 真实 payload 的 `skip_reason=empty_diff`);
2. `source_hats` 列表保持 validator 索引语义,但 `error_type` 让操作者一眼看出是 wave 状态守门拒收,**不**是 skip_reason 值非法;
3. 配合 P0-2 在 Rust 端 enforcement U6 Completeness Check,synthesizer 缺席时由机制层主动 emit `plan.blocked`,**不**依赖 agent 自觉。

---

## 附录 A:关键源码位置

| 组件 | 文件:行 | 角色 |
|---|---|---|
| `check_semantic_gates` 拒 `review.passed` when wave_open | `crates/ralph-core/src/event_loop/review_step_state.rs:126-147` | **P0-1 触发点** |
| `apply_event_policy_validation` | `crates/ralph-core/src/event_loop/mod.rs:833-887` | 把 `PolicyFinding` 转 `PayloadContractViolation` |
| `finding_to_payload_contract_violation` | `crates/ralph-core/src/event_loop/mod.rs:1245-1312` | `InvalidFieldValue` → `AllowedValueMismatch` 映射 |
| `write_payload_contract_violation_report` | `crates/ralph-cli/src/loop_runner/payload_contract_gate.rs:51-100` | 写 `.ralph/diagnostics/payload-contract-error-*.json` |
| `loop_runner.rs` terminate 路径 | `crates/ralph-cli/src/loop_runner/runner.rs:3403-3500` | 触发 `TerminationReason::PayloadContractViolation` |
| `ViolationType::InvalidFieldValue` 枚举 | `crates/ralph-core/src/event_policy.rs:27-30` | (需扩展为含 SemanticGateViolation) |
| `validate_event_with_hat` | `crates/ralph-core/src/event_policy.rs:493` | hat=None 时 hat_allowed_values 跳过(L708 `if let Some(hat_id) = hat`) |
| `extract_json_field` | `crates/ralph-core/src/event_policy.rs:830` | allowed_values 检查的 field path 解析 |
| `aggregate_timeout` 注入 | `crates/ralph-core/src/event_loop/mod.rs::inject_review_aggregate_timeouts` | (plan U3 计划路径,目前只 timeout signal,无 plan.blocked) |
| `source_hats_by_topic` 构建 | `crates/ralph-core/src/event_loop/mod.rs:6461-6478` | topic → 多 hat 索引(不是单 emit hat) |
| preset review.passed schema | `presets/en/ce-executor-isolated.yml:201-227` | `["empty_diff","trivial_step","aggregate_timeout"]` + hat_allowed_values |
| preset SSOT schema | `presets/schemas/ce-executor-isolated.yml` (行 60-90) | 与 inline 一致,build.rs deep-merge |
| review-coordinator empty_diff HARD RULE | `presets/en/ce-executor-isolated.yml:735-748` | 4 条件 AND (需扩展加 `wave_closed`) |

## 附录 B:关键时间线(UTC)

| 时间 | 事件 | 文件 |
|---|---|---|
| 03:06:37 | loop started | `.ralph/history.jsonl` |
| 03:08:33 | coordinator → work.ready (task-1781579295-5308) | events L1 |
| 03:39:34 | executor 6 次 cli_emit 拒(冷启动探测) | events L2-3 + recovery L1-6 + diagnostics L2-3 |
| 03:42:39 | executor → work.done (commit 76e6d5c, 1475 lines) | events L4 |
| 03:46:12 | review-coordinator 11 维 wave ready(w-18b973005abbc281-700695-0) | events L5-15 |
| 03:51:25 | dim-reviewer correctness → done | events L16 |
| 03:51:28 | dim-reviewer adversarial → done | events L17 |
| 03:51:40 | dim-reviewer adversarial → done (重复) | events L18 |
| 03:52:32 | dim-reviewer adversarial → done (重复) | events L19 |
| 03:54:50 | executor 二次 work.done (异常重发) | events L20 |
| 03:55:17 | stall_recovery 4 次 escalation(handoff_dispatch_timeout) | diagnostics/recovery.jsonl L4-7 |
| 04:06:52 | review-coordinator → review.passed (DEC-002 empty_diff) | events L21 |
| 04:08:04 | payload_contract_violation → loop terminate | diagnostics/payload-contract-error + summary.json |

## 附录 C:关键产物路径

| 类别 | 路径 |
|---|---|
| Loop 入口 | `/home/chaowen/Dev/agent_tools/ralph-orchestrator/.worktrees/2026-06-17-001-feat-ce-executor-flow-reliability-plan-zippy-sparrow/` |
| 事件流 | `.ralph/events-20260616-030637.jsonl` (21 行) |
| 失败诊断 | `.ralph/diagnostics/payload-contract-error-2026-06-16T04-08-04-273632282+00-00.json` |
| Session summary | `.ralph/diagnostics/2026-06-16T11-06-37/diagnosis-summary.json` |
| Stall envelope | `.ralph/diagnostics/2026-06-16T11-06-37/recovery.jsonl` (10 条) |
| 决策记录 | `.agents/scratchpad/ce-executor/2026-06-17-001-feat-ce-executor-flow-reliability-plan/decisions.md` (DEC-001/002) |
| Findings (正常) | `.agents/scratchpad/ce-executor/2026-06-17-001-feat-ce-executor-flow-reliability-plan/findings-*-task-1781579295-5308.json` |
| Findings (异常) | `.agents/scratchpad/ce-executor/2026-06-17-001-feat-ce-executor-flow-reliability-plan/findings-correctness-task-1781522658-25ee.json` + `findings-correctness-dim-review-tmpwOfzt5.json` |
| Preset | `presets/en/ce-executor-isolated.yml` |
| SSOT schema | `presets/schemas/ce-executor-isolated.yml` |
| Plan | `docs/plans/2026-06-17-001-feat-ce-executor-flow-reliability-plan.md` |
| 同型历史 | `docs/report/2026-06-13-ce-executor-isolated-wave-synthesizer-no-fire-diagnosis.md`,`docs/report/2026-06-15-plan-gate-dual-publish-blocking-diagnosis.md`,`docs/report/2026-06-15-ce-executor-isolated-work-ready-payload-contract-violation-diagnosis.md` |
