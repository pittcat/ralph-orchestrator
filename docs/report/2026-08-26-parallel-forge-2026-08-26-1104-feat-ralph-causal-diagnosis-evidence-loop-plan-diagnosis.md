---
report_type: ralph-run-diagnosis
preset: builtin:parallel-forge
loop_id: 2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan
history_search: disabled
bundle: present
diagnostics_mode: MINIMAL
execution_capabilities: [supervisor, wave]
structured_result_ref: inline: summarized in report
---

# Ralph run 诊断：因果诊断证据闭环第二次执行

## 结论

这次没有继续执行 U02-U10，原因不是 reporter 把未完成的 Unit 判成完成，而是第二次 `--reuse-worktree` 复用了上一轮已经走到 `cleanup` 的 flow-authority 状态。

具体链路是：

1. 第一轮 U01 已实际完成并合并到 integration branch，但 `forge.wave.settled` 的 `settled_task_ids` / `settled_unit_ids` 被编码成 JSON 字符串，状态投影拒收，因此 U01 task 没有关闭，Wave 1 没有进入下一波。
2. 第一轮随后被 `LOOP_COMPLETE` 强制终止，flow-authority 留下 `cleanup` 尾部。
3. 第二次复用同一个 worktree 时，新的 `forge.start` 没有把 flow-authority 重置回 `planning` / `plan_authoring`。Inspector 的 `forge.plan.inspected` 和 `forge.plan.blocked` 都被 `flow_unknown_emit` 拒收，所以没有重新开始计划检查，更不可能派发 U02。
4. fail-close 进入 cleanup；`forge.cleanup.done` 被接受后触发 reporter。Reporter channel 确实写出了 `forge.report.done`，但该事件尚未合并到当前 events ledger，进程就在 14:14:22 收到用户 Abort。因此当前运行没有真正完成 report terminal，也没有 `LOOP_COMPLETE`。

## 产物盘点

`run_dir`：`/home/chaowen/Dev/agent_tools/worktree/ralph-orchestrator/2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan`

| Tier | 产物 | 状态 | 关键事实 |
|---|---|---|---|
| S | `.ralph/current-events` 指向 `events-20260826-060331.jsonl` | 存在 | 当前文件只有 `forge.start` 与 `forge.cleanup.done` 两条事件 |
| S | `events-history-20260826-060331.jsonl` | 存在 | 保存上一轮的 Wave 1、settlement 拒收和强制终止证据 |
| S | `.ralph/ledger.jsonl` | 2 行 | 仅记录本次 cleanup batch/observation |
| S | `.ralph/recovery.jsonl` | 4 行 | 重复记录 `missing_terminal_emit`，目标为 executor 的 `work.done` |
| A | `.ralph/agent/tasks.jsonl` | 0 行 | 当前盘面没有已投影的 task close；U01 的 task 仍由 forge 产物标记为 open |
| B | `.ralph/diagnostics/2026-08-26T14-03-31/` | MINIMAL | bundle present；runtime trace 13 行；没有 orchestration/errors sidecar |
| B | `.ralph/supervisor.db` | 存在 | preset 明确启用 supervisor，且实际使用 wave |
| C | `.ralph/forge/ralph-causal-diagnosis-evidence-loop/` | 部分存在 | 只有 Wave 1 产物；U02-U10 未触发 |

Activation outcome：Inspector=`empty`，Cleanup=`merged`。Inspector 空 channel 与 `flow_unknown_emit` 对账一致；不能据此单独推断 agent 没有 emit。

## 强制四问

### 1. 执行与 OPAC

本次是 `supervisor + wave` 能力，诊断模式为 `MINIMAL`。Supervisor bridge 已接通，Wave 1 的 executor、reviewer、integrator、verifier 也确实运行过。OPAC 结论：

- 第一轮 settlement：状态投影拒收，属于真实 payload contract 错误。
- 第二轮 inspector：事件未写入主 ledger，属于 flow scope/authority 拒收，不是 backend 成功但 agent 无产出。
- Reporter：channel 中存在 `forge.report.done`，但没有合并；Abort 发生在 channel merge 之前。

由于没有 orchestration.jsonl，细粒度 OPAC 证据降级；上述结论由主 events、flow-authority、recovery、channel 文件和 CLI log 交叉支持。

### 2. 基座机制是否生效

部分生效。Supervisor、isolated hat channel、cleanup 路由和 reporter 触发均生效；但 reuse-worktree 的新 loop 没有重新初始化 flow-authority，导致起始阶段被旧的 `cleanup` 状态污染。这是 runtime flow 初始化/复用机制缺陷。

### 3. 编排是否合理

目标拓扑本身是合理的：`forge.wave.verified` → `forge.wave.settled` → 下一波准备。实际失败点有两个：

- Integrator 产生了错误类型的 settlement payload，绕过了“真实数组”这一结构化契约。
- 同一 plan/worktree 第二次启动没有清理或重建上一轮 flow 状态，导致 inspector 无法发起新一轮 planning。

因此 U01 的业务交付存在，但编排状态没有把它投影为已关闭 task，也没有把执行游标推进到 U02。

### 4. 归因

主根因：`runtime`，置信度 95/100。

贡献根因：

- `preset/runtime contract`，置信度 92/100：Integrator settlement payload 使用字符串而非数组，直接触发 `close_task_batch` 拒收。
- `runtime`，置信度 95/100：reuse-worktree 未重置 flow-authority，第二次 inspector 的两个起始事件均收到 `flow_unknown_emit`。
- `operator abort`，置信度 100/100：最后的 reporter channel merge 被用户 Abort 截断；这解释了为何 report 文件存在但主 events 没有 `forge.report.done`。

## 关键证据

- `.ralph/forge/ralph-causal-diagnosis-evidence-loop/blocks/inspector-blocked.md:13-32`：Inspector 的两个候选事件均 `recorded=false`，错误为 `flow_unknown_emit`；flow 当前步骤是遗留的 `cleanup`。
- `.ralph/flow-authority.jsonl:1-21`：第二次启动前后仍从旧链一路停在 cleanup，最后才接受 `forge.cleanup.done` 并进入 report。
- `.ralph/diagnostics/logs/ralph-2026-08-26T14-03-31-855-3574328.log:11-15`：Inspector channel empty、三轮无进展、fail-close。
- 同一 log 的 `:21-31`：cleanup 后进入 reporter；14:14:22 收到 `User requested abort`，随后终止进程树。
- `.ralph/agent/events-hat-reporter-2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan-3.jsonl`：Reporter 已生成 `forge.report.done`，但它仍在 hat-channel，不在当前主 events 文件。
- `.ralph/forge/ralph-causal-diagnosis-evidence-loop/commit-map.yml`：U01 有 final commit `e34642290ea11d651380f076228953e1f8f03300`；没有 U02-U10 条目。
- `.ralph/forge/ralph-causal-diagnosis-evidence-loop/cleanup.md:308-355`：上一轮事件 ledger 轮换后 cleanup 成功，说明 cleanup 成功不等于开发波次完成。

## Unit 状态

| Unit | 状态 | 说明 |
|---|---|---|
| U01 | 代码实质完成，但 task 未关闭 | commit、review、verifier 5021/5021 PASS 均存在；settlement projection 失败 |
| U02-U10 | 未触发 | 没有进入新的 development-loop wave |

## 非执行性修复建议

1. 在下一次启动前，先让 operator 选择“从 U02 续跑”还是“从 U01 全量重跑”；不要继续直接复用当前 stale flow 状态。
2. 修复/验证 Integrator 的 `forge.wave.settled` payload 构造，确保 `settled_task_ids` 和 `settled_unit_ids` 是实际 JSON 数组。
3. 为 reuse-worktree + 同 plan_key 增加 flow-authority fresh bootstrap 或明确的可审计恢复路径；启动时必须回到 `planning` / `plan_authoring`，不能继承 `cleanup`。
4. 重新运行前检查 U01 task 的投影状态和 integration branch；不要把 reporter 产物或 manager report 的存在当作 Unit 完成证明。

## 盲区

- `diagnosis-input.json` 的 `execution_capabilities` 为空，与 preset/YAML 和 supervisor.db 证据不一致；本报告按源码与产物信号推断为 `[supervisor, wave]`，并将 bundle 字段视为采集缺口。
- 本次 session 缺少 orchestration.jsonl 和 feedback.jsonl；没有使用历史目录作归因，`history_search=disabled`。
- 未执行任何代码修复、状态文件修改、重跑、cargo 命令或删除操作。
