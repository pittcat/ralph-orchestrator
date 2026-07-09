# Preset Topology Patterns

> **仅拓扑阶段参考。** 起草 `instructions:` 时不得把下列拓扑描述抄进 hat 文案。

## debug（4 hat，isolated）

适合学习 AAF handoff 与 OPAC。Builtin：`builtin:debug`。

```
debug.start
  → investigator → hypothesis.test
  → tester → hypothesis.confirmed | hypothesis.rejected
  → fixer → fix.propose → fix.applied
  → verifier → fix.verified | fix.failed
  → investigator (fix.verified) → DEBUG_COMPLETE
```

| Hat | 典型 Q4 emit | 下游 Q2 |
|---|---|---|
| investigator | `hypothesis.test`, `fix.propose`, `DEBUG_COMPLETE` | tester/fix 从 trigger payload + orchestrator context |
| tester | `hypothesis.confirmed`, `hypothesis.rejected` | fixer 读 `fix.propose` payload |
| fixer | `fix.applied`, `fix.blocked` | verifier 读 `fix.applied` |
| verifier | `fix.verified`, `fix.failed` | investigator 读 `fix.verified` |

参考：`presets/en/debug.yml`。

## ce-executor-pipeline（13 hat，isolated；Ralph primary path；2026-07-07-006 Unit 1）

**唯一推荐 CE executor 拓扑。** 单链 plan-driven 执行 + 串行多维 review，
executor 内部按 U-ID 分配 subagent，但 Ralph runtime 主链只看到 `work.done` /
`work.failed`。Builtin：`builtin:ce-executor-pipeline`。

高层事件流（简化）：

```
plan-gate / work.start
  → plan-reviewer（plan.reviewed）
  → executor（每个 U-ID 一个 subagent；主 executor 验收/提交/最终 emit）
  → 6× dimension hats（review.dimension.done）
  → review-synthesizer（review.complete）
  → fix-planner → fixer（fix.applied）
  → alignment（alignment.done）
  → reporter（LOOP_COMPLETE）
```

- Schema SSOT：`presets/en/ce-executor-pipeline.yml` 内联 `event_policy.schemas`
- 改 topic / `required_fields` / `state_projection` 须同步 schema 与 7 点清单
- executor 的 unit-level 证据走现有 `work.done` payload 字段（`tests_run` /
  `tests_passed` / `commit_count` / `executor_head_sha` / `changed_lines`），
  **不**新增 runtime unit-loop topic

参考：`presets/en/ce-executor-pipeline.yml`。

## ce-executor-pipeline-loop（15 hat，isolated；单链环形 review/fix；2026-07-08）

`builtin:ce-executor-pipeline-loop` 是 `ce-executor-pipeline` 的环形版本。
它不是旁路广播：每个业务 topic 仍然只有一个显式消费者。

关键拓扑：

```
work.done / fix.done
  → review-reentry（review.round.ready）
  → 6× 串行 dimension hats
  → review-synthesizer（review.synthesized）
  → review-gate（三选一）
      ├─ review.accepted → alignment → reporter → LOOP_COMPLETE
      ├─ fix.requested → fix-planner → review.complete → fixer → fix.done → review-reentry
      └─ review.loop.blocked → reporter → LOOP_COMPLETE
```

起草/评审检查点：

- `review-gate` 必须是互斥三出口：`review.accepted`、`fix.requested`、
  `review.loop.blocked` 同一 activation 只能发一个。
- `fix.requested` 只给 `fix-planner`，`review.complete` 只给 `fixer`；
  不要让两个 downstream hat 消费同一个 fix topic。
- `work.done` 和 `fix.done` 都只给 `review-reentry`，由它统一生成
  `review.round.ready`。
- P0/P1 先判断是否为当前 loop 的主要矛盾；严重程度本身不自动等于
  阻塞项。主要矛盾包括 round-1 P0/P1、上一轮 fix-plan 要求修但仍未
  关闭的问题、以及当前 fix diff 明确引入的新 P0 回归。
- 后续轮次新发现但不是当前修复导致的 P0/P1 进入 report-only residual，
  不应继续扩大 fix-plan；第 6 轮仍有主要矛盾时只走
  `review.loop.blocked`。
- `fix.done.next_review_plan` 是下一轮 review 的输入；`review-reentry`
  不应重新推断修复意图，也不应读取内部 ledger。
- `fix.done.next_review_plan` 必须是非空 JSON object，不能是 `null`；
  至少包含 `focus_areas`、`fixed_findings`、`verification_performed`、
  `residual_risks`、`diff_ranges` 五个数组字段，即使数组为空也要发出。
- 分裂维度 reviewer（`dim:*`）如果声明 `disallowed_tools: ["Edit"]`
  或 `["Write"]`，就按只读 reviewer 处理：不能把 `docs/plans/` 放进
  `allowed_write_paths`，也不能在 instructions 中要求直接改原计划文件。

参考：`presets/en/ce-executor-pipeline-loop.yml`。

## 起草反模式（禁止抄进 instructions）

| 反模式 | 应改为 |
|---|---|
| 「reviewer 通过后你会收到…」 | Q2：`ralph tools task list` / trigger payload 字段名 |
| 「上一步 executor 已提交代码」 | Q2：Observe `work.done` 投影字段 |
| 「读 events.jsonl 末尾」 | `ralph events --events-source hat-channel` |
| 「整个 pipeline 有 12 个 hat」 | 删除；该 hat 不知拓扑 |
| 在 instructions 写长篇 recovery 散文 | 改为**触发状态表** + 引用 `ralph-tools-recovery-directives` |
| 把 preset 专用 trigger 表抄进 `ralph-tools*.md` | 专用表只放 preset YAML；data docs 保持通用 |

## Historical anti-pattern: serial CE preset

> **2026-07-08 起 `ce-executor-serial` 已从 builtin 公共面删除。**
> 历史实验品，被 `ce-executor-pipeline`（见上文）取代。复发问题：
> 多状态源（tasks / progress / recovery 都被当作业务事实）、fallback
> 救场（shipper 路径能走到 success 终态）、prompt wall（orchestrator
> 与 review-synthesizer 互相 know）、terminal 后业务事件（post-`LOOP_COMPLETE`
> 仍有 `work.ready` 流过）。任何新 preset 都不应复刻这一拓扑；
> unit-by-unit 是 executor 内部策略，不是 runtime 拓扑。如果一个用户场景
> 你认为「必须」用 multi-consumer / fallback-success / rescue hat，请先
> 与 `ralph-preset-review` 沟通，确认单链语义确实表达不了，再立项。
> `references/finding-rubric.md` 的「Single-chain-first audit」段列出对应 finding。
