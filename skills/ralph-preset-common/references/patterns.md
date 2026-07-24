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
- 结算二分契约（2026-07-24-002）：`completed_units` 非空一律走 `work.done`
  （`execution_status` 取 complete / partial；failed / blocked / skipped Units
  进对应 bill 字段，下游按 residual 处理）；`work.failed` 只保留给零交付
  dead-end（`completed_units` 为空，`reason` 以 `unreachable` /
  `no_deliverable_commits` / `cannot_produce_handoff` 开头）。
  `new_business_regressions_count` / `flaky_or_environmental_count` 是
  report-only：诚实写进 `verification_delta_file` 与
  `post_verification_status`，非零不得强制 `work.failed` 或
  `fix_status=blocked`（`fix.done` 的 `blocked` 同样只给真 dead-end，
  与 `review_verdict` 解耦）

**收尾双事件终态（reporter hat）**：preset 把 `event_loop.required_events: ["report.done"]` 与 `event_loop.completion_promise: LOOP_COMPLETE` 配对，作为 reporter 合法双 emit 的入口。其它 hat（plan-reviewer / executor / dimension hats / synthesizer / fix-planner / fixer / alignment）一律**不享受**该例外——它们的 `publishes` 不应同时包含 required_events 列表里的 topic 与 completion_promise。AAF 复核参见 `finding-rubric.md`「required-event-to-completion 窄例外」段。

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

## review/fix convergence pattern（通用：单链环形 review/fix）

适用范围：reviewer → gate 三选一 → 接受 / 修复 / 阻塞；同一业务 topic
仅一个显式消费者；后续轮次新发现但非当前修复导致的严重 findings
按 report-only residual 处理，不应继续扩大 fix-plan。

拓扑通则（适用于所有 preset，不要套某个 builtin preset 字段清单）：

- gate hat 必须是互斥三出口（accept / fix / blocked）中的一支；
  同一 activation 只发一个终端业务事件。
- 「fix request」只给 fix planner 一个 consumer；plan 完成事件只给
  fixer 一个 consumer；不要让两个 downstream hat 消费同一个 fix topic。
- 「reentry / next-round trigger」是上游 work / fix done 的唯一汇聚点；
  同一上游 topic 复用到多个下游 hat 时要确认所有消费者有合理理由。
- 主要矛盾（main conflict）按「轮次 P0/P1 ∪ 上一轮未关闭 ∪ 当前 fix
  diff 引入的回归」三类组合判定；严重程度本身不等于阻塞项。
- residual report-only 在第 N 轮（preset 设定的 `max_review_rounds`）
  仍有主要矛盾时只能走 blocked；不要把 residual report 重新升级为
  fix request。

**Trigger Context 适配（适用任何 preset）**：

- 把「主要矛盾 / 接受 / 必须修复 / 第 N 轮阻塞」分支判定收敛到
  `event_policy.schemas.<synthesized-topic>.trigger_context.routing_hints`，
  gate / fix planner / alignment 的 `instructions` 只引用 `## TRIGGER CONTEXT`
  区块并说「先读 Trigger Context，再按本 hat 职责执行」。
- `summary_fields` 至少包含 `review_round`、主要矛盾计数、residual
  计数、`loop_decision_basis`、`verdict`、与下游必需的 supporting path
  字段。
- hint `guidance` 用「本轮你应该如何处理」的语言表达；不要写 runtime
  控制命令、不要改 topic / hat / 工具权限、不要叫下游 hat 重新推断
  payload。
- 残留处理边界必须在 hint guidance 中显式说明（report-only vs fix-now），
  避免下游 hat 误把 residual findings 升级成 fix units。

## Terminal report pattern（通用：经理正文 + 技术附录）

适用范围：终态 reporter / shipper / summarizer 类 hat 需要把一次执行结果写成
人类可读报告，并随后发出 terminal event。

- 正文面向决策者：先给一句话结论，再解释目标、实际完成、未完成或刻意不做、
  决策理由、质量风险、需要人决定的下一步。
- 技术附录面向核验证据者：集中放 plan / artifact 路径、SHA、验证命令、计数、
  review/fix 证据指针；正文不应变成 payload 字段流水账。
- 结论文案与 emit verdict 必须一一对应：报告可以用本地语言表达，payload enum
  保持 schema 允许值；不要为了报告新增 sibling terminal fields。
- 失败或阻塞路径使用同一骨架，明确哪些阶段未运行、证据只到哪里、下一步需要
  人介入什么；禁止 silent-success 话术。
- 若 preset 有多轮 review/fix，报告必须按轮次汇总过程，不要只用最新事件替代
  全部历史。
- 报告 prose 不应用 byte-equality 测试锁死；用 schema、lint、事件场景和人工
  checklist 验证结构化合同。

## 起草反模式（禁止抄进 instructions）

| 反模式 | 应改为 |
|---|---|
| 「reviewer 通过后你会收到…」 | Q2：`ralph tools task list` / trigger payload 字段名 |
| 「上一步 executor 已提交代码」 | Q2：Observe `work.done` 投影字段 |
| 「读 events.jsonl 末尾」 | `ralph events --events-source hat-channel` |
| 「整个 pipeline 有 12 个 hat」 | 删除；该 hat 不知拓扑 |
| 在 instructions 写长篇 recovery 散文 | 改为**触发状态表** + 引用 `ralph-tools-recovery-directives` |
| 把 preset 专用 trigger 表抄进 `ralph-tools*.md` | 专用表只放 preset YAML；data docs 保持通用 |
| reviewer "帮上游 commit 一下" 或 "git restore 一下" | 只读 hat 不替上游清理；记录 evidence + **Handoff failure emit**（发本 hat 唯一允许的 done/align 事件，带 `handoff_precheck_failed`，不伪装成功） |
| `worktree_status: clean` 当作可填字段 | 必须真实从 porcelain 推算；fabricate 是 contract violation |
| alignment Entry 用 `executor_head_sha` 验当前 HEAD | Entry 验当前交接 tip（`fix.done.head_sha` / 无 fix 时才是 executor tip）；`executor_head_sha` 只做 git-log 边界 |

## Git handoff protocol（写入型 vs 只读型，2026-07-12-001）

单链 preset 的下游 hat 只能从 Git 状态 + trigger payload 推断上游交接是否完整。
全局 `audit_file_modifications` 不知 activation 边界；因此 preset 必须强制下列纪律：

**写入型 hat（executor / fixer）— Final Git Handoff Precheck（两阶段，紧邻 emit）**

1. **Stage A（policy-check 前）**
   - 用 porcelain filter 抓 staged + unstaged + untracked，排除 `.ralph/`
     （runtime 产物不入项目 dirty 状态）：
     ```bash
     git status --porcelain --untracked-files=all \
       | awk '!/^\?\? \.ralph\// && !/^\?\? \.ralph$/ { print }'
     ```
   - 有本 hat 的可保留修改 → 复核 / 测试 / commit 后再算 final HEAD
   - 不可安全归属的修改 → **dirty blocked path**：`worktree_status: dirty`
     + blocked 语义（不得 fabricate `clean`）；或仅 revert 本 activation
     自产改动后走 clean success path
   - 计算 `final_head = git rev-parse HEAD`、`commit_count`、`changed_lines`
2. **Stage B（真实 emit 前）**
   - clean success path：porcelain 必须为空、HEAD == final_head、
     `worktree_status: clean`
   - dirty blocked path：porcelain 仍显示 payload 点名的脏路径、
     HEAD == final_head、`worktree_status: dirty`
   - 任一失败 → 放弃本次 emit，回到 Stage A 重建 payload
3. **fixer** 必须额外产出 `head_sha` / `worktree_status` / `fix_attempt_commit_sha`
   字段；empty-plan fast path 必须走 clean success path

**只读型 hat（6 dim / alignment / review-reentry）— Entry Precheck + Exit Precheck**

1. **Entry（trigger 后立刻）**
   - `expected_head_sha` 来自 trigger payload：普通 pipeline dim 用
     `executor_head_sha`；普通 pipeline alignment 用 `fix.done.head_sha`；
     loop dim 用 `round_base_sha`；loop alignment 在有 fix 轮时用
     最新 `fix.done.head_sha`，首轮无 fix 时用 `executor_head_sha`
   - `actual_start_head_sha = git rev-parse HEAD`，必须等于 expected
   - Porcelain filter（排除 `.ralph/`）必须为空
   - Save start evidence 到
     `.ralph/review/<plan>/{round-<NN>/}git-state-<hat>-start.txt`（不
     commit，仅 `.ralph/` 内）
   - 任何 violation → **Handoff failure emit**：发本 hat 唯一允许事件，
     带 `handoff_precheck_failed`（dim findings / align residuals /
     reentry residual_risks），禁止 silent-stop 挂起 loop
2. **Exit（policy-check + 真实 emit 两阶段）**
   - Stage A：HEAD == start HEAD + clean filter 空
   - `ralph emit --policy-check`
   - Stage B：再重检 HEAD + clean；变化则重建 payload
   - Save end evidence 到 `<...>/git-state-<hat>-end.txt`
   - Exit 失败同样走 **Handoff failure emit**，不得零事件退出

**Shared 纪律**

- reviewer / alignment **绝不**运行 `git add` / `git commit` /
  `git restore` / `git stash` / `git reset`
- `.ralph/` 不 commit；是 runtime 产物
- `worktree_status: clean` 必须真实；fabricate 是 contract violation
- 不需要为这套纪律写"大段 preset 文案 byte-equality"测试；以结构化
  schema（required fields `head_sha` / `worktree_status` /
  `fix_attempt_commit_sha`）和真实 EventLoop 场景验证

详见 `docs/plans/2026-07-12-001-fix-pipeline-preset-git-handoff-precheck-plan.md`。

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
