---
title: ce-executor-pipeline Loop `2026-08-12-001-feat-run-diagnosis-trace-debug-enhancement-plan` 运行链路诊断报告
date: 2026-08-12
type: diagnosis
loop_id: 2026-08-12-001-feat-run-diagnosis-trace-debug-enhancement-plan
preset: builtin:ce-executor-pipeline
run_dir: ../worktree/ralph-orchestrator/2026-08-12-001-feat-run-diagnosis-trace-debug-enhancement-plan
status: P0：终态事件被错误定向回生产者，重复三次后 loop_stale；P1：isolated 空 channel 未 fail-close
diagnostics_mode: MINIMAL
history_search: preset-only
---

# ce-executor-pipeline Loop `2026-08-12-001-feat-run-diagnosis-trace-debug-enhancement-plan` 运行链路诊断报告

> **生成时间**：2026-08-12
> **诊断对象**：`../worktree/ralph-orchestrator/2026-08-12-001-feat-run-diagnosis-trace-debug-enhancement-plan/.ralph/`
> **对照 preset**：`presets/en/ce-executor-pipeline.yml` + `presets/schemas/ce-executor-pipeline.yml`
> **诊断模式**：MINIMAL；有 session/recovery/drift/summary，但无 `orchestration.jsonl`、`agent-output.jsonl`、`trace.jsonl`
> **历史范围**：`preset-only`，按用户授权扫描近 30 天与本 preset/症状相关的 `docs/report/`、`docs/solutions/`、`docs/plans/`
> **execution_capabilities**：`[supervisor, wave]`。preset 的 `event_loop.supervisor.enabled: false`，但存在 `.ralph/supervisor.db` 且日志确认拾取 supervisor DB；preset 使用 isolated execution，事件本身未出现 `wave_id`，因此本次只把 supervisor 作为产物/默认 wave 证据，不把缺 `wave_id` 判故障。

## 0. 产物盘点

| Tier | 路径 | 存在 | 行数/状态 | 备注 |
|---|---|---:|---|---|
| S | `.ralph/current-events` → `.ralph/events-20260812-123809.jsonl` | 是 | 9 | 唯一可信 events 文件；最后为 `loop.terminate` |
| S | `.ralph/events-history-20260812-123809.jsonl` | 是 | 2 | 旁路历史，不覆盖 current-events |
| S | `.ralph/ledger.jsonl` | 是 | 8 | iteration 1、3–9 的 batch sync |
| S | `.ralph/recovery.jsonl` | 是 | 3 | repair-stream 1 条；semantic gate 2 条 |
| S | `.ralph/loop-termination-reason.json` | 是 | `"loop_stale"` | 终止原因明确 |
| A | `.ralph/agent/accepted-transitions.jsonl` | 是 | 8 | `dim:goal-alignment:7/8/9` 三次同一 transition topic，均 `delivered:false` |
| A | `.ralph/agent/tasks.jsonl` | 是 | 0 | preset `tasks.enabled: false`，属预期 |
| A | `.ralph/agent/summary.md` / `handoff.md` | summary 是；handoff 否 | — | stale 终止后有 summary；无 handoff 不单独判故障 |
| B | `.ralph/diagnostics/2026-08-12T20-38-09/` | 是 | MINIMAL | recovery 8 行、drift 0 行、active 0；无 orchestration/errors/trace |
| B | `.ralph/supervisor.db` | 是 | 139264 bytes | 与日志“picked up supervisor-db”一致 |
| B | `.ralph/diagnostics/channel-routing-fallback-*.md` | 是 | 3 个 | executor×2、test-stabilizer×1，均空 channel |
| C | `.ralph/review/<plan>/` | 是 | 多份 | normalized plan、trace、验证、goal-alignment 产物均存在；executor 已提交 7 个 commit |

`loop.lock` 已释放，`.ralph/loops.json` 已为空，termination reason 为 `loop_stale`；这是一次运行中被 stale breaker 终止，不是正常 `report.done/LOOP_COMPLETE` 完成。

## 1. 结论摘要

### 1.1 健康度

- **判定**：P0 假闭环/错误自路由，最终由 `loop_stale` fail-close 终止。
- **P0 / P1 数量**：P0×1、P1×1（均满足置信度门槛）。
- **最高根因置信度**：P0-001 = **85/100**（MINIMAL 模式硬顶）。
- **历史复发**：是。近 30 天内至少命中 2026-08-08 同 preset 的 goal-alignment 空 channel/no-progress 家族，以及 2026-07-22/23 的 stale、isolated scope、hat-channel fallback 家族；本次进一步证明了显式 `--triggered` 的自路由链。

### 1.2 强制四问

| # | 问题 | 答案 | 证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 执行与 OPAC 是否合规？ | ⚠️ 编排可观测，但 OPAC 只能 MINIMAL 审计 | events/recovery/ledger 可核对；无 agent-output，无法证明每次是否先 policy-check | 50 |
| Q2 | 基座机制是否生效？ | ✅ stale breaker 生效；但空 channel 诊断/恢复不足 | `wave_scope.rs:417-429` 在第 3 次同 fingerprint 后返回 `LoopStale`；`hat_channel.rs:65-87` 空 channel 仍继续 fallback | 85 |
| Q3 | 编排是否合理、正常运行？ | ❌ 不合理 | `review.goalalign.done` 的 producer 显式把自己写成 target，随后 goal-alignment 7/8/9 重复激活，未进入 correctness | 85 |
| Q4 | 归因是 preset / mechanism / agent / compound？ | **preset 主因 + mechanism 后果**；不是 agent 单独定论 | preset 指令错误使用 `--triggered`；runtime 将其作为 target；stale 只是正确终止重复链 | 85 |

### 1.3 根因一句话

`dim:goal-alignment` 的 preset instructions 把 `--triggered` 当成“发布者身份”使用，实际语义是“被事件触发的目标 hat”；`review.goalalign.done` 因此被重新定向给自身，连续三次重复同一 payload 后触发 `loop_stale`。**根因置信度：85/100。**

### 1.4 终态时序一致性

| 项目 | 内容 |
|---|---|
| 首轮终态 | 失败；accepted events 在 `review.goalalign.done` 后没有推进到 `review.correctness.done`，最终没有 `report.done` / `LOOP_COMPLETE` |
| 恢复状态 | 有限恢复；executor 空 channel 后 targeted missing-terminal recovery，随后链路继续；但 goal-alignment 自路由重复未恢复 |
| 最终代码状态 | worktree HEAD `80c77494`，events 记录 executor 已完成 U1–U7、test-stabilizer 完成，review 只重复 goal-alignment；loop 终止为 `loop_stale` |
| 一致性告警 | ⚠️ 运行报告中的 executor/test-stabilizer 验证成功不等于整个 loop 成功；没有后续 accepted `report.done`，不得把最终代码状态反写成 loop 成功 |

## 2. 执行链路与故障时序

```text
work.start
  → plan.ready
  → executor 空 hat-channel（backend_success=true，无 emit）
  → task.resume 定向恢复
  → executor work.done / precheck work.done
  → test-stabilizer stabilization.done
  → dim:goal-alignment review.goalalign.done  [target=dim:goal-alignment：错误自路由]
  → dim:goal-alignment review.goalalign.done  [同 payload]
  → dim:goal-alignment review.goalalign.done  [同 payload]
  → stale breaker count=3
  → loop.terminate(loop_stale)
```

关键事件为 current-events 的 L6–L9；独立的 accepted-transition outbox L6–L8 也记录了 `dim:goal-alignment:7/8/9` 三次 activation，三次 `review.goalalign.done` 的 `payload_digest` 相同。

## 3. 历史问题上下文

| 文档 | problem_type | 关联 | 结论 |
|---|---|---:|---|
| `docs/report/2026-08-08-ce-executor-pipeline-2026-08-07-003-refactor-emit-module-split-plan-diagnosis.md` | goal-alignment 空 channel / 未产生下游终态 | 高 | 同一 preset、同一 goal-alignment 链路；历史上只能定位到 no-emit，本次补足了自路由证据 |
| `docs/report/2026-07-23-ce-executor-supervisor-primary-20260723-082003-diagnosis.md` | hat-channel fallback / orphan / no-progress | 高 | 同一 isolated channel fallback 家族，说明 P1 不是偶发单点 |
| `docs/report/2026-07-22-ce-executor-supervisor-primary-20260722-084810-diagnosis.md` | stale / 终态链断裂 | 中-高 | 同一 stale breaker 终止家族；当前 stale 机制本身按设计工作 |
| `docs/solutions/state-management/2026-08-02-disable-mode-expected-observation-artifacts.md` | diagnostics artifact 语义 | 中 | 仅用于确认缺失 artifact 不能直接判机制故障；本次为 MINIMAL，不适用旧的 disabled 结论 |

本次扫描窗口：`preset-only (30d sliding)`。历史关联只用于复发确认，当前结论以 current-events、recovery、accepted-transitions 和当前源码为准。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度初判 | 置信度初估 | 已计分证据项 | 证据缺口 |
|---|---|---|---|---:|---|---|
| DEV-001 | `review.goalalign.done` 连续三次以同一 payload fingerprint 进入同一 hat activation | current-events:L6-L9；accepted-transitions:L6-L8 | P0 | 85 | file:line(+25)、双账本(+20)、preset行号(+15)、历史同根因(+10) | MINIMAL 无 agent-output，不能确认 agent 是否误读 prompt |
| DEV-002 | `triggered` 的实际语义是 target，preset 却把 producer 自身作为 target | `presets/en/ce-executor-pipeline.yml:3628`；`event_reader.rs:190`；`dispatch_and_handoff.rs:32-40` | P0 | 85 | file:line(+25)、preset行号(+15)、双账本(+20)、历史同根因(+10) | 没有 BDD 场景直接覆盖本次完整链路 |
| DEV-003 | isolated activation 结束时 channel 为空，runtime 只诊断并继续 fallback | log:L16-L18,L24-L26,L44-L46；`hat_channel.rs:65-87`；runner `inner.rs:3498-3511` | P1 | 85 | file:line(+25)、双账本(+20)、历史同根因(+10)、Tier C(+10) | MINIMAL 无 agent-output，无法判定 backend 未 emit 的具体内部原因 |
| DEV-004 | stale breaker 在重复签名三次后终止 | log:L62-L63；`wave_scope.rs:417-429`；`loop-termination-reason.json` | P1（机制正常） | 85 | file:line(+25)、双账本(+20)、历史同根因(+10) | 无缺口影响该机制是否生效的判断 |

### 4.1 OPAC 逐 hat 审计（MINIMAL）

| Hat | O | P | A | C | 证据 | 置信度 |
|---|---|---|---|---|---|---:|
| executor | ✅ | ⚠️ | ✅ | ⚠️ | events/recovery/ledger；log 明确 backend success 与空 channel，但无 agent-output | 50 |
| test-stabilizer | ✅ | ⚠️ | ✅ | ⚠️ | `stabilization.done` accepted；空 channel fallback；无 tool-call 证据 | 50 |
| dim:goal-alignment | ✅ | ⚠️ | ⚠️ | ❌ | 三次同 payload、显式 `triggered` 自路由；无法逐条确认 policy-check | 50 |
| dim:correctness | ⚠️ | N/A | ❌ | N/A | 未形成 accepted `review.correctness.done`；被错误 target 链截断 | 40 |

OPAC 结论仅为 MINIMAL 模式弱审计；未见 policy-check 不能单独升级为 agent P0，agent 归因不超过 60。

## 5. 问题归因表

| 优先级 | 问题 | 根因分类 | 置信度 | 证据 DEV | 已计分证据项 | 历史关联 | 加深轮次 |
|---|---|---|---:|---|---|---|---|
| P0 | `dim:goal-alignment` 显式设置 `--triggered dim:goal-alignment`，事件被定向回自身；重复三次后 loop_stale | **preset 主因 + mechanism 后果** | **85** | DEV-001/002/004 | file:line(+25) + 双账本(+20) + preset行号(+15) + 历史(+10)；主因=85，stale机制=85，整行=min=85 | 高；2026-08-08 同 preset goal-alignment no-emit 复发 | 第1轮：源码语义；第2轮：events + accepted-transitions + 历史对照 |
| P1 | 空 isolated hat-channel 只生成 fallback diagnostic 并继续，无法在首次空终态时 fail-close | **mechanism** | **85** | DEV-003 | file:line(+25) + 双账本(+20) + 历史(+10) + Tier C(+10) | 高；2026-07-23/08-08 同类复发 | 第1轮：`hat_channel.rs`/runner；第2轮：logs + fallback artifacts |

## 6. 修复建议

### 6.1 短期

- 暂时不要重跑该 preset 的同一链路；若必须验证，先在隔离测试 workspace 中移除 goal-alignment instructions 的显式 `--triggered`，让 CLI/runner按唯一下游自动推导。
- 运行后优先检查 `accepted-transitions.jsonl` 的 `activation_id`、`payload_digest` 与 current-events；不得把 `work.done` 或 review artifact 的成功状态当作 loop 完成。

### 6.2 中期：preset / agent-facing contract

- 删除 `presets/en/ce-executor-pipeline.yml:3628` 的显式 `--triggered dim:goal-alignment`；发布者身份使用当前 hat 上下文，`triggered` 只表示下游 target。
- 同步检查该 preset 其它 emitter instructions 中对 `--triggered` 的使用；以 `crates/ralph-core/data/ralph-tools-emit.md:41,75-80` 的 target 语义为准，必要时更新 agent-facing 文档中的冲突示例。
- 为真实 EventLoop 增加结构化场景：`stabilization.done → review.goalalign.done → review.correctness.done`，断言 goal-alignment 只激活一次且下游 target 为 correctness。不得用纯 YAML 文本包含断言替代真实 runtime 场景。

### 6.3 长期：机制

- 对 isolated 模式增加 emit-time 一致性门禁：当 producer hat 的业务终态 `triggered == current producer` 且 topic 存在明确唯一下游时，policy-check 应返回可行动的 contract error，而不是允许形成自路由。
- 空 terminal channel 在已知 terminal obligation 且 `backend_success=true` 时应立即写入结构化 recovery/activation evidence，并按责任 hat fail-close 或进入有界恢复；不能只保留 fallback 后继续制造“成功但无业务事件”的长等待。
- 新增 stale 诊断字段，明确记录 `source`、`target`、`activation_id` 和 `payload_digest`，避免仅显示 topic `review.goalalign.done`。

## 7. 未核实疑点

| 候选问题 | 当前置信度 | blocked_by | 已做加深 |
|---|---:|---|---|
| executor/test-stabilizer 空 channel 的具体原因是 agent 未 emit、marker race、还是 backend 输出路径问题 | 50 | 缺 `agent-output.jsonl` 与 FULL orchestration | 已查 logs、fallback artifacts、events、recovery；不作为本次 P0 根因 |
| 三次 goal-alignment activation 是否由某个未落盘的 task.resume 触发 | 55 | current-events 只有最终 accepted 账本，缺完整 orchestration | 已查 accepted-transitions、history、preset triggers；不改变自路由结论 |

## 8. 提交前检查

- [x] Phase 0 产物盘点表已写入报告。
- [x] 只读 `.ralph/current-events` 指向的唯一 events 文件。
- [x] MINIMAL 模式未因缺 orchestration 标 P0。
- [x] P0 置信度 85，P1 置信度 85；未将 agent 弱证据写成定论。
- [x] 历史检索状态已写入 frontmatter：`preset-only`。
- [x] 未使用 `hat_handoff`、`loop_state_snapshot.json` 等过时概念。
- [x] 报告写入主仓 `docs/report/`，未修改 run 的 `.ralph` 状态。
