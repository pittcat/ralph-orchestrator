---
report_type: ralph-run-diagnosis
report_version: "1.0"
generated_at: "2026-08-16"
loop_id: 2026-08-15-2211-fix-terminal-artifact-admission-plan
preset: builtin:ce-executor-pipeline
bundle: finalized
bundle_path: ../worktree/ralph-orchestrator/2026-08-15-2211-fix-terminal-artifact-admission-plan/.ralph/diagnostics/2026-08-15T22-28-28
history_search: full
activation_outcomes: present
trace_status: present
feedback_status: present
structured_result_ref: inline
---

# Ralph 运行诊断报告

## 结论摘要

本次运行的代码交付链已完成，最终报告为 `pass_with_residuals`，并且人工补发的 `LOOP_COMPLETE` 已被接受；但循环没有自然、干净地闭合。主要问题是一个“终态事件目标”和“handoff 按 topic 推导的消费者”之间的契约不一致：`report.done` 在事件记录中被显式目标化为 `executor`，而 handoff 逻辑仍按 `report.done` 的 topic 等待 `reporter` 激活。结果是运行时产生了一个错误的 reporter 超时恢复记录，随后 executor 又发出收敛用的 `work.done.proposed`，被正确识别为重复事件，最终出现 `no_progress_turn_observed`。

因此，本次应判定为：交付结果有效，但运行时终态闭环存在 P0 级交接错误；`LOOP_COMPLETE` 是人工恢复后的有效关闭事件，不是原始编排自然产生的关闭结果。

## 1. 四个诊断问题

### 1.1 是否成功

部分成功。业务链已走完：`work.start`、计划审核、executor、稳定化、六维审核、修复、对齐和 `report.done` 均已出现；最终 HEAD 为 `4e0b1b6b35b7ddb6c78818f4d091464fe2d2359f`，报告 verdict 为 `pass_with_residuals`，测试结果在最终报告中为 8008/8008。

但循环闭合不成功：`report.done` 后没有按 preset 设计再次激活 reporter 发出 `LOOP_COMPLETE`，而是发生了 executor 重入、重复 `work.done.proposed`、no-progress，最后由人工执行 OPAC 补发 `LOOP_COMPLETE`。

### 1.2 哪个机制失效

失效的是终态 handoff 的目标一致性检查与交接追踪。当前实现把 JSONL 事件的 `triggered` 字段转换为直接目标，并优先按该目标调度；同时，事件被接受后又通过 `handoff_index.consumer_of(topic)` 按 topic 注册 pending handoff。两套选择结果没有被要求一致。

本次 `report.done` 的实际记录目标是 `executor`，但 session recovery 记录却等待 `reporter` 在 600 秒内激活，说明 handoff tracker 追踪的是静态 topic 消费者而非实际交付目标。

`work.done` 的 duplicate 拒绝和 no-progress 记录本身是保护机制正常工作，不是第一根因：它们阻止了已经完成的 step 被重复推进，并记录了空批次。

### 1.3 preset 编排是否合理

不完全合理。preset 的 reporter 指令明确规定：第一次激活发出 `report.done`，接受该事件后的下一次 reporter 激活只发出 `LOOP_COMPLETE`；schema 也把 `report.done` 和 `LOOP_COMPLETE`定义为终态闭环事件。

但是，preset 没有形成可审计的约束来保证接受的 `report.done` 必须回到 reporter，也没有阻止该事件携带 `triggered: executor`。现有静态测试只验证 reporter 发布 `report.done`，没有验证“接受后的目标”和“handoff 消费者”一致，也没有覆盖该终态重入路径。

### 1.4 根因归属

这是 runtime 与 preset 合同的复合问题：

1. runtime 正确执行了 `triggered` 的直接目标语义，但没有把该目标传递给 handoff tracker，也没有在目标不一致时拒绝或重算 handoff。
2. preset 允许 `report.done` 作为 reporter 自触发闭环，但未对终态事件的目标字段建立结构化约束。
3. 当前证据不足以把 `triggered: executor` 归因到 agent 文案、CLI 生成器或 runtime 注入中的某一个具体来源；本次 bundle 为 MINIMAL，缺少 `agent-output.jsonl` 和 `orchestration.jsonl`。不应据此指责 agent。

## 2. OPAC 审计

本次采用 MINIMAL 诊断模式；bundle 已 finalized，runtime trace、feedback 和 recovery 侧车均存在，但没有完整 orchestration 与 agent output。

用户提供的最终操作满足四阶段闭环：`LOOP_COMPLETE --policy-check` 返回 `ok:true, recorded:false`，随后正式 emit 返回 `ok:true, recorded:true`，并读回 hat-channel 文件确认事件、reason 和 report_path。对这次人工关闭动作，OPAC 证据充分；对整个运行期间的所有 agent 操作，因缺少 agent output，只能判定为部分可审计。

综合诊断置信度：85/100。P0 根因置信度：85/100；agent 具体归因置信度不超过 60/100。

## 3. 关键时间线

| 时间（UTC） | 事件 | 诊断意义 |
|---|---|---|
| 14:58:44 | `work.done.rejected` | U3 交付证据不足，属于正常 precheck 纠偏 |
| 15:02:21 | `work.done` | 主执行链恢复并继续推进 |
| 17:14:40 | `fix.done` | 修复链完成 |
| 17:19:15 | `align.done` | 进入 reporter |
| 17:23:09 | `report.done`，记录 `triggered: executor` | 终态事件实际目标与 reporter 预期不一致 |
| 17:26:33 | executor 发出 `work.done.proposed` | 发生错误的 post-report 收敛重入 |
| 17:27:03 | duplicate `work.done` 拒绝、`no_progress_turn_observed` | 保护机制生效，但循环进入空转 |
| 17:31:56 | 人工发出并接受 `LOOP_COMPLETE` | 最终人工关闭循环 |
| 17:37:33 | recovery 记录 reporter handoff timeout | 侧车仍按 reporter 等待，进一步证明 handoff 目标不一致 |

runtime trace 显示 23 个 activation/outcome 对均为 `merged`，没有证据表明某个 agent 激活失败；问题集中在 accepted event 的路由与 handoff bookkeeping，而非孤立 agent 崩溃。

## 4. 机制证据与源码定位

| 编号 | 证据 | 定位 |
|---|---|---|
| E1 | `triggered` 被转成事件 target | `crates/ralph-core/src/event_reader.rs:182-191` |
| E2 | 有 target 时优先按 target 调度，绕过 topic 订阅选择 | `crates/ralph-core/src/event_loop/dispatch_and_handoff.rs:19-63` |
| E3 | accepted event 的 handoff consumer 独立由 topic 查找 | `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs:1011-1059`；`crates/ralph-core/src/workflow_contract/handoff_index.rs:228-230` |
| E4 | handoff tracker 记录 consumer，并在激活时按 consumer 清理 | `crates/ralph-core/src/workflow_contract/handoff_tracker.rs:186-244` |
| E5 | 超时后向原 consumer 生成恢复目标 | `crates/ralph-core/src/workflow_contract/handoff_tracker.rs:252-281` |
| E6 | duplicate work.done 被明确拒绝 | `crates/ralph-core/src/event_policy/validation.rs:920-985` |
| E7 | 空事件批次被记录为 no-progress | `crates/ralph-core/src/event_loop/parse_and_emit/legacy.rs:450-470`、`4175-4205` |
| E8 | 终态后业务事件进入冻结/拒绝保护 | `crates/ralph-core/src/event_loop/terminal_closed_guard.rs:59-105` |

## 5. preset 证据

`presets/en/ce-executor-pipeline.yml:5472-5508`规定 reporter 消费 `report.done`，第一次发 `report.done`，下一次激活只发 `LOOP_COMPLETE`；`presets/en/ce-executor-pipeline.yml:5628-5641`和 `5677-5722`重复声明该闭环。

schema 在 `presets/en/ce-executor-pipeline.yml:1843-1868`要求 `report.done` 与 `LOOP_COMPLETE`携带匹配的报告路径，并将它们作为完成门禁。现有 `crates/ralph-cli/src/presets.rs:1043-1055`只要求 `report.done` 存在，相关静态测试只验证 reporter 发布该 topic，未验证 accepted event 的 target 与 handoff consumer 一致。

## 6. 历史复发性

本次按 `history_search: full` 扫描既有报告、方案、解决文档和允许的运行记录。相同的“终态交接/空 channel/no-progress/恢复超时”家族已多次出现：

| 历史记录 | 与本次关系 |
|---|---|
| `docs/report/2026-08-12-ce-executor-pipeline-2026-08-12-001-diagnosis.md` | 明确记录 `triggered` 语义被误当成 producer identity，造成事件自路由、empty channel 和 stale；与本次 target-routing 机制高度相关 |
| `docs/report/2026-08-16-ce-executor-pipeline-2026-08-15-2211-fix-state-machine-transaction-boundary-plan-diagnosis.md` | 记录 `work.done → test-stabilizer` handoff timeout 和 isolated empty-channel 家族，说明终态/交接可靠性仍有复发 |
| `docs/report/2026-08-13-ce-executor-pipeline-2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan-diagnosis.md` | 记录同 preset 的交接、恢复和工作树一致性问题，支持“编排闭环而非单个 agent 失败”的历史判断 |
| `docs/plans/2026-08-16-fix-terminal-event-delivery-recovery-plan.md` | 后续方案已把终态交付、direct fallback、task.resume 和 bounded recovery 列为系统性问题，和本次证据方向一致 |

历史记录支持“同一家族复发”，但不能单独证明本次 `triggered: executor` 的产生来源；本次根因判断仍以 current-events、runtime trace、recovery 和源码为准。

## 7. 影响、分级与建议

### P0 — 终态事件目标与 handoff consumer 不一致（置信度 85）

影响：reporter 的终态闭环被错误转向 executor；handoff tracker 产生虚假 reporter timeout，随后出现 post-report 重入和 no-progress，循环无法自然结束。

建议：接受事件后统一以最终 dispatch target 建立 handoff；或在接受阶段检测 explicit target 与 `consumer_of(topic)` 不一致并 fail-close，记录清晰的 misrouted 诊断。终态事件应增加结构化 target/consumer 一致性校验，并用真实 EventLoop 场景覆盖 `report.done → reporter → LOOP_COMPLETE`。

### P1 — preset 缺少终态 target 约束（置信度 85）

影响：preset 文案表达了 reporter 自闭环，但 schema/lint 没有把“report.done 必须由 reporter 消费并再次激活 reporter”变成可验证契约。

建议：在 preset lint 或 event policy 中校验终态 topic 的 producer、显式 target、topic consumer 和下一步 terminal event；补充 BDD 场景，禁止只做文本匹配测试。

### P2 — 人工 LOOP_COMPLETE 依赖与诊断可见性（置信度 70）

影响：人工 OPAC 能关闭循环，但掩盖了自然闭环失败；MINIMAL bundle 无法追溯 `triggered` 的最初生成者。

建议：诊断 bundle 默认保留终态事件的原始 producer、target、activation id 和 handoff correlation；对 terminal recovery 保留完整的 accepted/rejected/resume 链。

## 8. 结论

本次运行的业务交付可以验收，但 Ralph loop 的运行时闭环不能标记为无残留成功。最重要的修复方向是让“实际事件目标”和“handoff 追踪消费者”使用同一个权威路由结果，并把终态事件的目标一致性提升为结构化 runtime/preset contract。`duplicate_work_done` 与 `no_progress_turn_observed` 是后续保护信号，应保留用于暴露该类错误重入，而不应被视为根因。
