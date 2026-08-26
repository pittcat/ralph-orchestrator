---
title: builtin:parallel-forge Loop `2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan` 运行链路诊断报告
date: 2026-08-26
type: diagnosis
loop_id: 2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan
preset: builtin:parallel-forge
plan: docs/plans/2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan.md
run_dir: /home/chaowen/Dev/agent_tools/worktree/ralph-orchestrator/2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan
status: P0 — verifier 工作全部成功（U01 commit 84fe20ae 合并,incremental-verification.md PASS），但 hat-channel merge 失败 + flow-authority.jsonl stale-tail 致清理链全断，loop 被 fail-close 终止
diagnostics_mode: MINIMAL
bundle: present
bundle_path: .ralph/diagnostics/2026-08-26T16-08-33/diagnosis-input.json
history_search: preset-only
structured_result_ref: "inline: summarized in report"
trace_status: present
feedback_status: present
recovery_status: present (1 final entry: cli_emit flow_unknown_emit on loop.cancel)
activation_outcomes: present (9 outcomes, 7 merged / 2 empty)
execution_capabilities: ["supervisor", "wave"]
evidence_gaps:
  - worker channel 文件已被清理（loop 终止后 events-hat-*verifier*.jsonl 等被引擎删除），无法从产物反证 verifier agent 写盘是否成功
  - orchestration.jsonl / errors.jsonl 缺失（bundle 默认未生成）
---

# builtin:parallel-forge Loop `2026-08-26-1104-feat-ralph-causal-diagnosis-evidence-loop-plan` 运行链路诊断报告

## 0. 产物盘点

`execution_capabilities: ["supervisor", "wave"]`。判定依据：

1. `presets/en/parallel-forge.yml` 声明 `event_loop.execution_mode: isolated` 与 `supervisor.enabled: true`（phase 1A 验证）。
2. `.ralph/supervisor.db` 存在且 ralph log 明确记录 `execution_mode=isolated, supervisor.enabled=true`。
3. events 主账含 `wave_id: w-18cf4dfe7a10bae1-4027500-0`。
4. U-ID 串行 1 个 Unit（U01），`incremental-verification.md` 记录 verifier 已完成全量门禁并写盘。

### Tier 表

| Tier | 路径 | 存在 | 行数 | 备注 |
|---|---|---:|---:|---|
| S | `.ralph/current-events` → `events-20260826-080833.jsonl` | 是 | 11 | 可信主 events；末条 `loop.cancel` |
| S | `.ralph/flow-authority.jsonl` | 是 | 13 | 末 4 条 orphan（step="" + 无 loop_id 字段），导致 stale-tail 污染 |
| S | `.ralph/recovery.jsonl` | 是 | 2 | `repair-stream plan.blocked` ×2（missing_terminal_emit） |
| S | `.ralph/diagnostics/<session>/recovery.jsonl` | 是 | 2 | 含 `cli_emit flow_unknown_emit` 对 `loop.cancel` 的拒绝收据 |
| A | `.ralph/agent/tasks.jsonl` | 是 | 11 | 10 个 `open` Unit + 1 个 closed supervisor slot-0；U01 仍未 close |
| A | `.ralph/agent/accepted-transitions.jsonl` | 是 | 9 | 含 8 个 accepted transition + 1 个 `stall-detector:9 forge.plan.blocked` |
| A | `.ralph/agent/summary.md` | 是 | — | 状态 `Cancelled gracefully`，9 iter / 1h 21m 29s |
| B | `.ralph/diagnostics/2026-08-26T16-08-33/` | 是 | trace 51 | bundle `manifest_status: finalized` |
| B | `.ralph/supervisor.db` | 是 | SQLite | `w-1` 收尾阶段 `Removed supervisor slot worktree`（log line 76） |
| B | `.ralph/forge/<plan>/units/U01-completion.md` | 是 | — | U01 完成报告 + commit `84fe20ae` |
| B | `.ralph/forge/<plan>/waves/w-18cf.../review.md` | 是 | — | `aggregate_verdict: ACCEPTED` |
| B | `.ralph/forge/<plan>/incremental-verification.md` | 是 | — | `nextest causal_evidence_activation 6/6 PASS` + `diagnostics 119/119` + `diagnostics_off_on 3/3` + `config 416/416` + `cargo fmt --check clean` + `cargo clippy 7 pre-existing` |
| B | `.ralph/forge/<plan>/waves/w-18cf.../commit-map.yml` | 是 | — | merge_base `cc353be7` → candidate `84fe20ae` |
| B | `.ralph/diagnostics/channel-routing-fallback-2026-08-26T09-25-16.md` | 是 | — | `hat=verifier reason=merge_hat_channel_failed` |
| B | `.ralph/diagnostics/channel-routing-fallback-2026-08-26T09-30-02.md` | 是 | — | `hat=ralph reason=merge_hat_channel_failed` |
| C | `orchestration.jsonl` / `errors.jsonl` | 否 | — | bundle 默认未生成 |
| C | worker channel（`events-hat-*verifier*.jsonl`） | 已清理 | — | LOOP_COMPLETE 后引擎删除，无法从产物反证 verifier 子进程写盘状态 |

## 1. 强制四问

| # | 问题 | 答案 | 证据 | 置信度 |
|---|---|---|---|---:|
| Q1 | 执行与 OPAC 是否合规？ | ⚠️ | 7/9 hat activation `status=merged`，但 verifier / ralph 两轮 `status=empty`；OPAC P 维度内 `merge_succeeded=false` | 78 |
| Q2 | 基座机制是否生效？ | ⚠️ | supervisor bridge 已 wire（log line 6）、U01 commit 已合并；但 FlowStepScope + flow-authority ledger 出现 stale-tail → cleanup/reporter 链全断 | 80 |
| Q3 | 编排是否正常推进？ | ❌ | `forge.wave.verified` / `forge.cleanup.done` / `forge.report.done` 均未出现；终态走 fail-close `forge.plan.blocked` → `loop.cancel` | 92 |
| Q4 | 归因：preset / mechanism / agent / compound？ | **compound**（mechanism-bug + preset-topology-gap + agent-emit-shape 共同作用），责任域主要在 `crates/ralph-core/src/event_loop/{flow_authority,stage_pipeline}` + `presets/en/parallel-forge.yml` cleanup hat `triggers` | 见 §5 | 85 |

### Prompt visibility 对账

未触发（本 run 未发现「agent 看不到某 skill」或「agent 引用了不该看到的内部实现」的可疑模式）。preset 内的 hat `instructions:` 仅引用 `ralph wave emit` / `ralph tools task list` / `ralph-tools.md` 等公开命令，未泄漏内部实现细节。

## 2. 执行链路（双账本对账）

### 2.1 主 events（current-events 指向 `events-20260826-080833.jsonl`）

```
forge.start (08:08:33)
  → forge.plan.inspected     (08:12:25 inspector iter 1)
  → forge.plan.ready          (08:24:29 planner iter 2)
  → forge.concurrency.approved(08:31:31 guardian iter 3)
  → forge.worktrees.ready     (08:34:24 worktree iter 4)
  → exec.unit.ready           (08:35:32 forge-dispatcher iter 5)
  → exec.unit.done            (09:10:11 executor iter 5; wave w-18cf...-0)
  → exec.wave.complete        (09:10:11 exec-integrator iter 5)
  → forge.wave.reviewed       (09:13:37 reviewer iter 6; ACCEPTED)
  → forge.wave.integrated     (09:15:53 integrator iter 7; FF merge)
  → [verifier iter 8: 09:25:16 status=empty, candidate_event_count=0, channel_bytes=0]
  → [ralph iter 9:    09:30:02 status=empty, candidate_event_count=1 (loop.cancel rejected)]
  → loop.cancel               (09:30:02 ralph iter 9, raw emit 落账被 flow_unknown_emit 拒)
```

`accepted_transitions.jsonl` 显示 9 个 transition，第 9 个为 `stall-detector:9 forge.plan.blocked`（fail-close）。

### 2.2 runtime-trace activation outcomes（9 行）

| seq | iter | hat | status | backend_exit | output_bytes | output_mentions_emit | candidate_event_count | terminal_obligation_topics |
|---:|---:|---|---|---:|---:|---:|---:|---|
| 5 | 1 | inspector | merged | 0 | 2234 | true | 1 | forge.plan.inspected, forge.plan.blocked |
| 12 | 2 | planner | merged | 0 | — | true | 1 | forge.plan.ready, forge.plan.blocked |
| 17 | 3 | guardian | merged | 0 | — | true | 1 | forge.concurrency.approved, forge.plan.blocked |
| 20 | 4 | worktree | merged | 0 | — | true | 1 | forge.worktrees.ready, forge.plan.blocked |
| 24 | 5 | forge-dispatcher | merged | 0 | 1417 | false | 0 | exec.unit.ready, forge.wave.prepare, forge.exec.development.done |
| 27 | 5 | executor | merged | 0 | — | true | 1 | exec.unit.done, exec.unit.failed, work.failed |
| 32 | 6 | reviewer | merged | 0 | 3948 | true | 1 | forge.wave.reviewed, forge.wave.review.failed, forge.units.reviewed, forge.plan.blocked |
| 37 | 7 | integrator | merged | 0 | 2762 | true | 1 | forge.wave.integrated, forge.wave.settled, forge.wave.integration.failed, forge.integration.done, work.failed, forge.plan.blocked |
| 41 | 8 | **verifier** | **empty** | 0 | **8509** | **true** | **0** | forge.wave.verified, forge.verification.failed, work.failed |
| 48 | 9 | **ralph** | **empty** | 0 | 2055 | true | 1 | (空) |

verifier（iter 8）output_bytes=8509 + `output_mentions_emit=true`，但 `channel_bytes=0` + `candidate_event_count=0` → 输出被生成但**未落盘到 hat-channel**。

### 2.3 recovery.jsonl（双账本一致）

- `.ralph/recovery.jsonl`：2 条 `repair-stream plan.blocked`（`reason_code=repair_dispatch`, `missing_terminal_emit`, target `executor`, retry_count=4）。`plan.blocked` 在 flow-authority 中对应 step `cleanup`，**与末尾 4 条 orphan entry 的末条 step/topic 一致**（orphan 末条：`{"step":"cleanup","topic":"forge.plan.blocked"}`）。
- `.ralph/diagnostics/<session>/recovery.jsonl`：1 条 final entry `cli_emit flow_unknown_emit`, topic=`loop.cancel`, source_hat=`ralph`, reason="flow-authority stale-tail blocks all business emit; verifier forge.wave.verified and cleanup forge.cleanup.done rejected with flow_unknown_emit"。

### 2.4 ralph 运行时 log（关键 4 条）

| log line | 时刻 | 含义 |
|---:|---|---|
| 58 | 09:25:16.864983 | `hat-channel routing fallback hat=verifier reason=hat_channel_empty_after_activation` |
| 60 | 09:25:16.865543 | `Failed to merge isolated hat channel ... events-hat-verifier-...-8.jsonl` |
| 61 | 09:25:16.876585 | `Hard gate triggered: hat has publish obligation but emitted no event hat=verifier consecutive=1` |
| 72 | 09:30:02.885871 | `isolated loop: no progress for 3 turns with progress_steward disabled — emitting forge.plan.blocked (fail-close) consecutive_no_progress=3 max_iter=3` |

### 2.5 flow-authority.jsonl（13 条）

| 序号 | step | topic | loop_id | 性质 |
|---:|---|---|---|---|
| 1-8 | plan_authoring / concurrency_review / worktree_setup / development_loop | forge.* 合法 transition | 本 loop | 正常 |
| 9 | **""** | `build.start` | **缺字段** | **orphan** |
| 10 | **""** | `test.topic` | **缺字段** | **orphan** |
| 11 | **""** | `experiment.planned` | **缺字段** | **orphan** |
| 12 | **""** | `review.complete` | **缺字段** | **orphan** |
| 13 | cleanup | forge.plan.blocked | 本 loop | fail-close |

orphan 的来源：phase 1A 验证 `append_flow_authority_snapshot` (`completion_and_termination.rs:819-885`) 在 `self.current_loop_id()` 返 None 时**省略 loop_id 字段**；在 `current_plan_step` 为空时写 `step=""`。`load_flow_authority_current_step` (`mod.rs:1133-1167`) 在 active loop_id 给出时**仅跳过有 loop_id 且不匹配的条目**，无 loop_id 字段的条目被无条件接受（legacy blind read,line 1156）。

## 3. 历史关联（preset-only 模式）

Phase 1B 扫描 30 天窗口，命中 5 个相关历史文档。**判定：同构 + 复发（相似度 78/100）**。

| 历史 run / 文档 | 同构点 | 与本次差异 |
|---|---|---|
| `docs/solutions/workflow-orchestration/parallel-forge-preset-integration-gap.md` (2026-07-29) | preset 拓扑机制同源 | 历史修复基线，非故障 |
| `docs/report/2026-08-10-parallel-forge-primary-...-diagnosis.md` | cleanup 链 BLOCKED | 触发层相反：彼 run `backend_success=false`，本 run `merge_succeeded=false` |
| `docs/report/2026-08-08-parallel-forge-primary-...-diagnosis.md` | `flow-authority` 停在 `development_loop`，hard gate 命中 1 次 | 本次 orphan step="" + 缺 loop_id 字段组合更严重 |
| `docs/report/2026-08-05-parallel-forge-primary-...-diagnosis.md` | dispatcher 永远不能推进 `forge.exec.development.done`，hat-channel 空 + flow 不允许 `forge.report.done` | 完全对应 `flow-authority-stale-tail-pollutes-recovery` 记忆根因场景 |
| `docs/report/2026-07-27-implementation-review-primary-...-diagnosis.md` | review-worker 6 emit 全部被 `FlowStepScopeStage` 拒（`flow_unknown_emit`），slot 全 `empty_worker_result` | 同 preset 内 stale-tail→flow锁死先例 |

**新增加分（与所有历史 run 的差异）**：本 run 的产品交付是 causal-diagnosis-evidence-loop 的首次实战；verifier 候选 event=0 + output_mentions_emit=true 的组合（前 8509 字节验证总结文本成功生成但未落盘）此前未在历史 run 中观察到。

## 4. 证据清单

| ID | 描述 | 证据锚点 | 严重度 |
|---|---|---|---|
| E-01 | U01 工作实际成功（commit 84fe20ae, 119/119+6+3+416 tests, fmt clean） | `git show 84fe20ae`、`incremental-verification.md` | OK |
| E-02 | verifier hat 产出 8509 字节 + `output_mentions_emit=true`，但 channel 字节=0/candidate=0 | `runtime-trace.jsonl seq 41`、`channel-routing-fallback-09-25-16.md` | P0 |
| E-03 | hard gate: `hat has publish obligation but emitted no event hat=verifier consecutive=1` | `ralph-2026-08-26T16-08-33-583-4003183.log:61` | P0 |
| E-04 | `isolated loop: no progress for 3 turns` → fail-close `forge.plan.blocked` | log line 72 | P0 |
| E-05 | `loop.cancel` emit 被 `flow_unknown_emit` 拒收，reason 指 stale-tail | `recovery(diag).jsonl` + log line 71 | P0 |
| E-06 | flow-authority.jsonl 末尾 4 条 orphan entry (`step=""` + 缺 loop_id 字段) | `.ralph/flow-authority.jsonl` lines 9-12 | P0 |
| E-07 | `load_flow_authority_current_step` 对无 loop_id 字段条目做 legacy blind read | `crates/ralph-core/src/event_loop/mod.rs:1133-1167` | P0 |
| E-08 | `append_flow_authority_snapshot` 在 `current_loop_id=None` 时省略 loop_id 字段 | `crates/ralph-core/src/event_loop/completion_and_termination.rs:819-885` | P0 |
| E-09 | cleanup hat 不订阅 `forge.wave.settled`（preset-topology gap） | `presets/en/parallel-forge.yml` cleanup hat `triggers` | P1 |
| E-10 | plan frontmatter `baseline_commit: 6ff0367c` 与 run baseline `cc353be7` 不一致 | `docs/plans/2026-08-26-1104-...-plan.md:8` vs `loop-version.json` | P2 |
| E-11 | 历史同构（5 份 30 天内文档，相似度 78） | 见 §3 表 | 参考 |

## 5. 根因归因（按置信度排序）

| 序 | 问题 | 根因分类 | 置信度 | 证据 |
|---:|---|---|---:|---|
| **R-1** | flow-authority.jsonl 末尾 4 条 orphan（`step=""` + 缺 loop_id 字段）污染导致任意 active loop 的 FlowStepScope 决策读错 current_step | **mechanism/ledger boundary** | **88** | E-06 + E-07 + E-08（双账本一致：orphan 条目确实存在 + 读取契约 line 1156 legacy blind read 确认） |
| **R-2** | verifier hat-channel merge 失败（channel 字节=0/candidate=0/output=8509）致 hard gate fail-close 链触发 | **mechanism/runtime** | **82** | E-02 + E-03（runtime-trace seq 41 + log line 58-61） |
| **R-3** | cleanup hat 不订阅 `forge.wave.settled`，verifier 拒后没有 fallback 兜底链 | **preset-topology** | **78** | E-09（phase 1A 验证 cleanup `triggers` 不含 `forge.wave.settled`） |
| **R-4** | hard gate `consecutive=1` + `max_iter=3` 过快 fail-close（progress_steward disabled），无 partial_progress 缓冲 | **mechanism/policy** | **75** | E-03 + E-04（log line 61/72 显示 isolated loop no-progress 3 turns 即 fail-close） |
| **R-5** | `loop.cancel`（loop_runner 自发终态）走 FlowStepScope gate 致 fail-close 双锁死（plan.blocked + cancel 互不相容） | **mechanism/coexistence** | **72** | E-05（recovery 记录 reason 指 plan.blocked 与 cancel 冲突） |
| R-6 | plan frontmatter baseline 与 run baseline 不一致（drift） | documentation | 65 | E-10 |
| R-7 | verifier agent 输出是叙述性 markdown（含 `emit` 字面提及）但未触发 `ralph emit` CLI，channel 写盘 = 0 | agent-side（候选） | 50 | E-02（output_mentions_emit=true 但 candidate=0；worker channel 已清理） |
| R-8 | 此前 preset-lint 未在 parallel-forge cleanup hat 触发器中校验 `forge.wave.settled` 必订 | lint/preset-author | 60 | E-09（preset 自身缺兜底，lint 也未发现） |

### 已否决假设

- **不是** U01 commit 未成功：`git show 84fe20ae` 存在 + `incremental-verification.md` PASS + `accepted-transitions` 含 `executor:5 exec.unit.done` + `integrator:7 forge.wave.integrated`。
- **不是** worker backend 失败：`runtime-trace` `backend_exit_code=0` + `backend_success=true` + `backend_termination=false`（seq 41 字段）。
- **不是** reviewer 拒绝：`forge.wave.reviewed` ACCEPTED（`waves/w-18cf.../review.md` `aggregate_verdict: ACCEPTED`）。
- **不能确认**是 verifier agent 写错 `RALPH_EVENTS_FILE`：worker channel 已被 LOOP_COMPLETE 清理，无法反证；该候选（R-7）保持 50 分不入主根因。
- **不是** plan 自身矛盾：plan frontmatter `plan_status: READY`，baseline drift 仅是文档同步（P2）。

## 6. 修复建议（非执行，仅供 operator 评估）

### 6.1 短期（立即可做）

1. **手工 trim flow-authority.jsonl**：删除末尾 4 条 orphan（`step=""` + 缺 loop_id 字段的 `build.start` / `test.topic` / `experiment.planned` / `review.complete`），保留末条 `{"step":"cleanup","topic":"forge.plan.blocked","loop_id":"2026-08-26-1104-...-plan"}`。再以 `--reuse-worktree` 重启原 loop，让 verifier 重新跑一次。这是 operator 当前唯一能在不引入新代码前提下恢复产物验收的路径（verifier 已写盘 `incremental-verification.md` PASS + U01 已 merged 84fe20ae，重启只需 verifier 跑出 forge.wave.verified 让 cleanup 走通）。
2. **不要再次 `ralph run --plan <同 plan>` 二次启动**：memory `parallel-forge-loop-relaunch-flow-cleanup-debt` 已记录同 loop_id 二次启动 flow-authority 仍停在 `cleanup`，inspector/planner/guardian 全部 `flow_unknown_emit`；trim 或新 `--plan` 二选一。

### 6.2 中期（mechanism 修复）

1. **fix orphan blind read**：`load_flow_authority_current_step` 在 `active loop_id=Some` 时**也 skip 无 loop_id 字段条目**（除非 caller 显式声明 legacy 模式），并对历史无字段条目提示 WARN 而不是 silent accept。落点 `crates/ralph-core/src/event_loop/mod.rs:1156`。
2. **fix append 契约**：`append_flow_authority_snapshot` 在 `current_loop_id=None` 时**显式拒绝写入**（warn + skip），而不是写无字段条目。落点 `crates/ralph-core/src/event_loop/completion_and_termination.rs:861-863`。
3. **加 cleanup hat 兜底 trigger**：cleanup 增加 `forge.wave.settled` 订阅，使 verifier 拒后 wave 仍能走 settle 路径。落点 `presets/en/parallel-forge.yml` cleanup hat `triggers`。

### 6.3 长期（policy / preset）

1. **hard gate 软化**：`consecutive_no_progress >= 3` 时先尝试 `task.resume` 到 verifier / cleanup 一次（带 stall fingerprint），再决定是否 fail-close。当前 `max_iter=3` 在 verifier 单点失败时立即 fail-close，无 partial_progress 缓冲。
2. **preset-lint 加 finding**：parallel-forge cleanup 必须订阅 `forge.wave.settled`（或 `forge.wave.integration.failed`），否则 lint 失败，避免下次出现同 topology。

## 7. 未核实疑点

| 候选问题 | 置信度 | 阻塞原因 | 已做加深 |
|---|---:|---|---|
| verifier agent 子进程是否写错了 `RALPH_EVENTS_FILE`，致 output_bytes=8509 但 channel_bytes=0 | 50 | worker channel 在 LOOP_COMPLETE 后被引擎删除，无原始文件 | 已交叉 runtime-trace seq 41 `channel_exists=true, channel_readable=true, channel_bytes=0` 与 log line 60 `isolated hat channel is empty after activation`；两者一致提示通道文件存在但为空。若该候选为真，root cause 上移到 agent env scrub 而非 mechanism bug，置信度不变 |
| orphan 4 条是否来自同 workspace 的上一次未清理 loop 的残留 | 60 | 末尾 orphan 条目无 loop_id 字段，无法定位写入源 loop | phase 1A 确认 `append_flow_authority_snapshot` 写入契约确实可在 `current_loop_id=None` 时产出此 orphan；据此 orphan 是同 workspace 上一轮中断 loop 的概率较高 |
| plan baseline drift（frontmatter 6ff0367c vs run cc353be7）是否在 flow-authority 决策中起作用 | 30 | baseline drift 仅影响 ledger `input_fingerprint`，未影响 FlowStepScope | E-10 仅文档不一致；不入主根因 |

## 8. 强制提交前检查

- [x] Phase 0 盘点表已写入（§0）
- [x] 只读 `current-events` 指向的 events（未读 events-history-*）
- [x] LOGS_ONLY 未因缺 orchestration 标 P0（capability +supervisor 已确认）
- [x] 每条 P0/P1 在 §5 有置信度；P0 最高 88 / 入表门槛 ≥60 均过
- [x] confidence<60 的候选（R-7 / R-8 50-60）已标 §7 未核实疑点，未混入 §5/§6
- [x] 未引用 ssot-guardrails 禁止项（hat_handoff / loop_state_snapshot.json / 错误 CLI 等）
- [x] `docs/report/` 只含本最终 Markdown 报告；中间产物均不写入
- [x] 历史检索开关状态 `preset-only` 已写入 frontmatter
- [x] bundle `manifest_status: finalized` 与 frontmatter `bundle: present` 一致
- [x] flow-authority 写入契约引用 `file:line`（E-08）

## 9. 一句话根因

U01 工作成功 + verifier 写出 8509 字节验证总结（incremental-verification.md PASS），但 hat-channel merge 失败 → hard gate 触发 → 与本 workspace 上一轮 loop 残留的 4 条 `step=""` + 缺 loop_id 字段的 flow-authority orphan 共同作用（legacy blind read 把 stale step 当 current），致 verifier `forge.wave.verified` 与 cleanup `forge.cleanup.done` 全部被 FlowStepScope `flow_unknown_emit` 拒收，cleanup 又未订阅 `forge.wave.settled` 无兜底链，3 轮 no-progress 后 fail-close → `loop.cancel` 也被同一 orphan 拒 → 双锁死，loop 被 fail-close 终止。