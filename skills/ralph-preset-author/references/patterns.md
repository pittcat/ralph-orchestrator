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

**成功脊门禁（`path_required_events`）**：当 preset 同时有成功路径（例如 `work.done` → `plan.complete`）与失败早退（`plan.blocked` / `work.failed` → `LOOP_COMPLETE`）时，不要把成功脊 handoff 写进 `required_events`。改用：

```yaml
event_loop:
  required_events: [LOOP_COMPLETE]
  path_required_events:
    - anchor: plan.complete
      require: [work.done]
```

`ralph preset check` 会对绕过 `require` 到达 `anchor` 的边报 `topology.path_required_event_not_on_all_paths`；runtime 在 admit `anchor` 前也会要求 `require` 已见过。

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

## Paired completion pattern（通用：终态事件字段一致性）

适用范围：preset 的收尾 hat 需要先发出一个终态报告事件（如 `forge.report.done`），再发出 `LOOP_COMPLETE`。为防止 resume 或 recovery 用不同路径覆盖既有终态事实，启用 `event_loop.completion_payload_match` 强制两事件声明字段一致。

- 配置：在 `event_loop.completion_payload_match` 声明 `topic`（前置终态事件）和 `fields`（必须一致的顶层字段，如 `report_path`）。runtime 会记录最近 accepted 的 `topic` payload，并在 `LOOP_COMPLETE` 时比较声明字段。
- Reporter instructions：必须写明「resume 时不得重写既有报告事实，只能补发匹配 completion；若需新报告，必须先重发 `topic` 建立新基准」。
- Schema 配套：`LOOP_COMPLETE` 的 schema `required_fields` 必须包含 `completion_payload_match.fields` 中声明的字段。
- 未配置时行为不变（默认关闭）。FAILED / BLOCKED 报告仍可通过匹配 completion 正常终止。

## Declarative flow authority pattern（通用：跨 hat handoff 必须显式声明）

适用范围：任何 preset 通过 `mechanism.flow` 表达多 hat handoff 时。

- 通用语义：`advance_plan_step` 显式 forward target 优先，positional fallback 兜底。
  现行通用 runtime（`recover_current_plan_step` / `FlowStepScope` / 各类协
  调 fan-in）已支持显式 `on` / `on_any_of`，起草 preset 时**不应再依赖**
  「任一允许 topic 都推进到下一个 step」的 positional 行为。
- 多 topic 顺序 handoff 必须各占一个 step：每个跨 hat handoff 用下一
  step 的 `"on": <topic>`；同一 step 的 `allowed_emits` 只承载进入当前
  step 那一刻可能浮现的 topic，不要把多个顺序成功 handoff 塞在同一
  `kind: linear` step 里。
- 多源 block 走 `on_any_of`：当 N 个不同 step 都可能因业务失败 / plan
  阻塞汇聚到同一个收敛 step 时，在收敛 step 上声明 `on_any_of: [t1, t2, ...]`
  而不是把每个源的「block topic」分别塞回原 step 的 `allowed_emits`。
- 资源生命周期必须显式收敛：若 preset 创建 worktree / branch / child
  process 等临时资源，使用独立 cleanup step/hat 在报告或终态收敛之前处理；cleanup
  必须输出逐资源结果，成功路径删除本轮拥有的资源，失败路径明确记录
  preset 定义的 pending 字段或保留诊断现场。报告/终态收敛 owner 只消费 cleanup
  结果，不重复执行副作用清理。
- `exec_wave` 等 side-effect step 的 unit topic 保持 non-transition：
  `exec.unit.ready` / `exec.unit.done` / `exec.unit.failed` 不应被任何下一
  step 的 `on` 引用——它们的 step 推进由 supervisor 注入的
  `exec.wave.complete` / `exec.wave.failed` 决定。`kind: side_effect` /
  `kind: await` 不强制声明 `runs`，但要让 `allowed_emits` 与下一 step
  的 `on` 自洽。
- `work.failed` 始终 non-transition：它由 runtime 的 `NON_TRANSITION_TOPICS`
  在显式 target 搜索前 no-op；preset 不应用 `on: work.failed` 制造假分支，
  也不把 work.failed 当作进入 terminal 的 transition。reporter 由
  `forge.plan.blocked` / `exec.wave.failed` / `work.failed` 等 trigger
  唤醒后，在自己的 `forge.report.done` 单步里把当前 step 推到 `plan_end`。
- 非末尾、kind=linear、allowed_emits≥2、无后续 forward target 的组合
  会被严格 lint 拒收（finding_id：`preset.flow_linear_positional_ambiguity`）。
  拆 step、加 `on` / `on_any_of` 是唯一合规修复，不要改 severity 也不要
  按 preset 名走 exemption。
- 起草完后用结构性契约测试验证：
  - `recover_current_plan_step` 对每个 cross-hat success handoff 顺序
    推进到下一 step，且不会从单 topic 跨过早一节；
  - 任意 `forge.plan.blocked` 收敛到 reporter 的 step（通常是 `report`
    或对等的收敛点）；
  - failure-capable step 的 `allowed_emits` 同时含 `work.failed` 和
    收敛 topic（如 `forge.report.done`），使 reporter 在 work.failed 后
    不会再次被 FlowStepScope 拒。
  - 若存在临时资源 cleanup hat，则 failure topic 先进入 cleanup，再由
    cleanup 的收敛 topic 唤醒报告/终态收敛 owner；不要让两个 owner 竞争同一资源。

<!-- anchor: evidence-bound -->
## Evidence-bound correction pattern（plan 2026-08-06-001 U4）

适用于所有使用 evidence-bound correction / semantic rejection 的 preset（通过 `ralph emit --policy-check` 返回 `reason_code: semantic_gate_violation` + `gate` 字段触发）。reviewer 在 Payload Audit 阶段按以下四条检查：

**Anchor（patterns.md）**：`evidence_bound_missing_invariant` / `evidence_bound_replacement_payload` / `evidence_bound_no_target` / `evidence_bound_unbounded_retry`

- `evidence_bound_missing_invariant`：semantic rejection 的 `correction` payload 必须包含 `violated_invariant` 字段，说明哪个业务不变量被违反；缺失则 agent 无法自修复
- `evidence_bound_replacement_payload`：semantic rejection **禁止**携带 `replacement` / `suggested_payload` / `fix_suggestion` 等替代语义字段——此类字段属于 `correction.replacement` 而非 `correction.evidence`，会混淆 semantic vs mechanical rejection 语义
- `evidence_bound_no_target`：semantic rejection 的 `correction` payload 必须包含 `target_hat` 字段，使 bounded retry 机制能路由到正确的重试目标；缺失则 retry 无法定位
- `evidence_bound_unbounded_retry`：preset 的 correction / retry 循环必须包含 evidence progression check（每次重试的 `violated_invariant` / `observed` 必须与前一次不同），否则构成无界重试循环

四条 finding 均 review-only，不进 `ralph preset check` JSON；触发条件是 `correction` payload 形状 + 语义，不是 preset 名称。fixture 顶部注释与 `skills/ralph-preset-review/fixtures/README.md` §8 标注 anti-pattern 轴、expected finding id 与本表对照命中。

## Recovery guidance pattern

适用于声明 `event_loop.precheck.rules.<topic>.recovery_guidance` 或 `event_policy.payload_consistency.rules[].recovery_guidance` 的 preset。

- `recovery_guidance.common` / `by_check` 可选；省略等于旧行为。
- precheck `by_check` key 必须是 `1..=prompt.len()` 的十进制字符串（禁止 `"01"`）。
- consistency `by_check` key 必须等于该 rule 的 `id`。
- YAML key 必须是真实 emit topic：不要用 `topic.workspace` 再挂第二条永远不触发的 rule。同一 topic 的语义检查与 workspace 检查应写在同一 `prompt` 列表。
- semantic guidance 是 prose，禁止 `suggested_command` / replacement payload。
- synthetic / 空 `failed_checks` 只渲染 `common`。

Runtime lint：`preset.recovery_guidance_unknown_check` / `preset.recovery_guidance_empty_item` / `preset.recovery_guidance_unsafe_item`。Review-only：`preset.workspace_precheck_fake_topic` / `preset.workspace_precheck_missing_entry_exit`。


## Scope handoff guard pattern（merge-batch / post-merge-converge / red-team-attack）

适用于三套独立 scope preset（`merge-batch` / `post-merge-converge` / `red-team-attack`）。这些 preset 必须由各自的 hat 独立解析 scope，不依赖其它 preset 的 merge boundary 作为 authority。

**核心契约（四步顺序，禁止跳过）：**
1. **写 manifest**：实际执行的 hat 或其 sub-agent 先把 scope 内容写到 `.ralph/{merge,post-merge,red-team}/<name>.json`，内容是要交给下游的字节稳定 scope 声明。
2. **计算 digest**：对 canonical JSON（排除 `scope_digest` 字段本身）计算 SHA-256，得到 64-char hex 填入 `scope_digest`。
3. **policy-check**：先跑 `ralph emit --policy-check <scope-topic>` 预检；`--unsafe-no-policy-check` **不能**绕过 scope handoff guard。
4. **真实 emit**：通过后去掉 `--policy-check` 正式 emit，payload 携带 `scope_manifest_path` / `scope_digest` / `scope_status` / `scope_base_sha` / `overall_confidence` / `critical_unknown_count`。

**Manifest path 规则**：`scope_manifest_path` 必须以 `.ralph/{merge,post-merge,red-team}/` 开头，文件必须在 emit 前落盘可读。

**Base SHA 规则**：`scope_base_sha` 必须是真实 Git SHA（40 hex chars），禁止 `<global-baseline>` 等占位符。

**Boundary authority 规则**：scope 解析的 authority 必须在当前 preset 的 hat 内部，不应依赖下游 merge 结果作为上游 scope 的 authority（否则触发 `scope.contract.boundary_authority` finding）。

**Threshold gate**：resolved scope 必须同时满足 `overall_confidence >= 90` + `critical_unknown_count == 0` + `proceed == true`；任一不满足则 scope 仍为 unresolved，下游不应据此推进。

**反模式**：
- 先 emit 再补写 manifest ❌
- 用 `--unsafe-no-policy-check` 绕过 scope handoff guard ❌
- `scope_base_sha` 用占位符而非真实 SHA ❌
- 把 merge boundary 当作 scope authority（preset 间耦合）❌
- `overall_confidence` 低于 90 或 `critical_unknown_count` 非零时仍标记 `proceed = true` 并推进 scope（触发 `scope.contract.confidence_gate_bypass` finding）❌

参考：`presets/en/merge-batch.yml` / `presets/en/post-merge-converge.yml` / `presets/en/red-team-attack.yml`。
## Key-stage event gate pattern

按 capability signal 逐位置记录 `key_stage`、`guard_selection`、两个 guard 布尔值、各自 retry budget、`reason` 和 `confirmation_status`。`event_loop.precheck` 是事件级 LLM gate；`ralph emit --policy-check` 是独立的确定性 schema/ownership 预检，不能互相替代。`neither` 或 budget 低于 3 必须有可审计理由，未确认时停止最终 YAML/schema 设计。

## Projection-Owned Task DAG pattern（通用：单事件原子建 task DAG）

适用范围：任何 preset 由一个 hat 一次性声明 N 个内部 task（unit / fix-unit / dimension 等），并交给 StateProjector 一次性原子落盘。下游 hat 通过 `ralph tools task list` 读取 live `task_id`，**不再**走 agent 自己 `task add`。

- 唯一写者：在 `event_loop.state_projection.actions` 配置 typed `ensure_task_batch` action；Projector 在单次原子边界内完成全批校验 → ID mint → 持久化，任一失败整批零写。禁止把 batch 拆成多次 `task add`。
- 输入权威二选一：普通 batch 可由 schema-required payload items 提供；artifact-first batch 的公开 event 只声明 artifact path / identity / digest，runtime 从有界 artifact 派生内部 items。artifact-first 模式禁止把 derived task/wave/order 数组同时放进 payload。
- Schema 配套：payload-backed 模式声明 items/count 字段；artifact-backed 模式只声明 artifact reference/identity 字段，并在 field_docs 写清 artifact 路径来源、runtime 派生字段和失败停止条件。
- Hat `instructions`：**禁止**让 projector-owned hat 同时调 `ralph tools task add` / `task ensure`；lint `preset.instructions_task_mutation_authority_conflict` 会拒收。artifact-backed instructions 必须要求先写 artifact、再 policy-check、且不得手抄 derived task 数组。
- 下游消费：下游 hat 只从 `ralph tools task list` 取得 runtime mint 的 live `task_id`，不从上游 payload 猜测。
- 起草后用结构性契约测试验证：一笔 handoff 后 `TaskStore` 行数/依赖与权威输入一致；重复 handoff 幂等；invalid payload/artifact 整批零写；另跑一个非 artifact-backed preset 的 differential regression，证明 legacy projector 行为未变。

## Projection-Owned Task Close pattern（通用：单事件原子关 task）

适用范围：执行类 hat 的完成事件与 task 关闭必须原子发生，避免「Unit 已完成但 task 仍 open」的漂移。配置后 emit 即关闭，agent 不再手工 `task close`。

- 唯一写者：在 `event_loop.state_projection.actions` 配置 typed action，例如 `exec.unit.done: { kind: close_task, task_id: task_id }`。Projector 在 accepted event 上原子关闭 payload 指向的 live task。
- Schema 配套：preset `presets/schemas/<name>.yml` 在 `schemas.<topic>` 把 `task_id`/`task_key` 加到 `required_fields`，`field_docs.task_id.source` 指向 trigger payload。
- Hat `instructions`：**禁止**让 projector-owned hat 同时调 `ralph tools task close` 走 CLI；改为「emit 即关闭，禁止手工 close」。
- 下游消费：dispatcher / supervisor 通过 `ralph tools task list` 读到真实 closed 状态，不再依赖 agent 自觉。
- 起草后用结构性契约测试验证：accepted `exec.unit.done` 只关闭 payload 指向的 task，sibling 保持 open；未知 `task_id` 被 fail-closed 拒绝且零副作用。

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

## Re-entry dirty-worktree reconciliation pattern（Step 1.25，2026-07-28-002）

写入型 executor hat（如 `ce-executor-pipeline` 的 executor）在**再入激活**（loop 重启 / resume / 崩溃恢复后重新进入同一 worktree）时，工作区可能残留上一次激活未提交的脏改动。preset 的 executor instructions 必须包含 Step 1.25「Re-entry Dirty-Worktree Reconciliation」协议，核心契约：

- **先只读盘点，后归因**：`git status --porcelain --untracked-files=all` + `git diff` / `git diff --cached` + `.ralph/agent/decisions.md` checkpoint 行 + `git log <DIFF_BASE>..HEAD`，把脏路径集合映射到 plan 的剩余 U-ID（按 Unit 白名单文件 + approach + checkpoint 记录）。
- **唯一归属 → 续做不重放**：脏改动恰好归属一个 U-ID 且无矛盾编辑时，追加 `executor re-entry:` 决策行，把现有 diff 当作该 U-ID 的进行中起点，要求子代理检查、补完、测试后合并提交一次；**不得**重放已存在的编辑。
- **不可归属 → 原样保留 + fail-closed**：归属歧义、跨 U 部分改动、无关 operator 改动、冲突标记或不可读 untracked 内容时，追加 `executor re-entry blocked:` 决策行，**严禁** reset / restore / checkout / clean / stash / overwrite / delete / stage / commit 这些改动；仅在已有匹配 U-ID commit 时发 `work.done` partial，否则发 `work.failed` 且 `reason: "unattributable_dirty_worktree: <paths>"`。
- **基线重定义**：可归属脏树接管后，baseline 描述的是续做后的 worktree 状态，必须在 `baseline-verification.md` 记录 `baseline_context: resumed_attributable_dirty_worktree`。

**评审检查点**（ralph-preset-review 对写入型 executor hat 的 AAF 扩展轴）：

- executor instructions 缺少再入脏树盘点步骤 → `executor_reentry_reconciliation_missing`（P1）；
- instructions 允许对不可归属脏改动执行破坏性 git 操作（reset/checkout/clean/stash/overwrite）→ `executor_reentry_destructive_on_unattributable`（P0）；
- 不可归属分支没有 fail-closed 终态（既不 `work.failed` 也不 partial）→ `executor_reentry_no_fail_closed`（P0）。

正/反例见 `fixtures/reentry-dirty-worktree-positive-fixture.yml` 与 `fixtures/reentry-dirty-worktree-negative-fixture.yml`。

## Wave worker 双时钟（hat 级 idle_heartbeat_secs + startup_grace_secs）

Wave worker 走双时钟（仅限 wave worker PTY 路径，不影响主 loop `PtyExecutor`），
三个字段挂在 hat 级而非 `SupervisorConfig`：

- `hats.<id>.timeout`（u32 秒）— StartToClose 硬顶，从 worker spawn 起算。
- `hats.<id>.idle_heartbeat_secs`（u32 秒）— HeartbeatTimeout 静默窗口，自上次
  合格进度信号起计时。`0` 或省略 = 关闭 idle 模式，仅 StartToClose 墙钟。
- `hats.<id>.idle_weak_signal_cap`（u32 次）— 连续仅靠弱信号（assistant text /
  thinking / `TextDelta`）续租的次数上限；用尽后必须等到强信号（tool 事件 /
  events file 增长）或硬顶到达，否则 idle kill。
- `hats.<id>.startup_grace_secs`（u32 秒）— **可选**，冷启动容忍窗口。**仅
  在 `idle_heartbeat_secs > 0` 时生效**（idle 关闭时该字段被忽略）。在首个
  合格进度信号到达之前用 startup_grace 取代 idle 窗口；首信号到达后恢复
  idle 语义。`0` 或省略 = 关闭。超时归因为 `startup_kill`（归
  `worker_timeout` family，可重试可 redrive）。

KTD7 推荐值：worker / fix-worker 用 `timeout: 1800, idle_heartbeat_secs: 120,
idle_weak_signal_cap: 8`；review-batch-worker 用 `timeout: 900,
idle_heartbeat_secs: 90, idle_weak_signal_cap: 8`。若 backend 实测冷启动
P50 > `idle_heartbeat_secs`（典型场景：Claude / Gemini / Codex headless 在
spawn 到第一行输出之间超过 120s），加 `startup_grace_secs: 300` 保护慢热
backend。这是 `parallel-forge` builtin preset 的当前值；新建
preset 应当按 `commands.md` 提到的 `preset_lint` + `cargo nextest run -p
ralph-core -- hat` 验证 hat 字段解析。

agent 不需要主动刷 heartbeat：orchestrator 观察 stream JSON 与
`RALPH_EVENTS_FILE` 增长来续租 idle 窗口。hat `instructions:` 写「不需要主动
发 heartbeat」即可，**不要**复述 idle / hard 文案细节；具体分类与 family 映射
在 `crates/ralph-core/data/ralph-tools-wave.md`「Worker 终止语义」段落。

## Wave slot 自动重试（`event_loop.supervisor.slot_retry_budget`）

`event_loop.supervisor.slot_retry_budget`（u32，默认 1，允许 0..=2，>2 启
动期拒绝）控制同一 wave 内 supervisor 对单个 slot 的自动重派次数。preset
作者配置时注意：

- `0` = 关闭自动重派，任何可重试失败直接走 redrive 路径。
- `1`（默认）= 初始执行 + 1 次自动重派，吸收瞬时 backend 错误。
- `2` = 初始执行 + 2 次重派，副作用必须保证幂等（agent 工作可能在原 slot
  上重新执行）。

可重试 reason 固定为 5 个 frozen code（`worker_timeout` /
`empty_worker_result` / `missing_worker_terminal` / `slot_never_started` /
`executor_reported_failure`），非白名单 reason 永不重试。中间 attempt 的
progress / RPC / TUI 副作用被截断（只有最终 attempt 的 outcome 暴露给
reporter），不会让 TUI 计数漂移。preset 启用 `slot_retry_budget > 0` 时，
确认 agent 副作用可幂等可重入。

**执行波次的主动失败也消耗 attempt**：`WaveKind::Exec` 的 slot 若 worker
自己 emit `*.unit.failed` 终态，dispatcher 记为 `executor_reported_failure`
并按预算重试；review / fix 波次保持原有 `Completed(Failed)` 语义，不受影
响。预算耗尽后该 reason 稳定进入 `slot_failures[].reason`，`failure_class`
为 `required_slot_failure`，因此仍会出现在 `redrive_slots` 里。

**重试 = 新进程 + 同一 worktree（resume 协议）**：runtime 不回滚上次
attempt 的代码、提交与报告，新进程 `cwd` 不变，且 prompt 末尾追加
`# Retry Context`（第几次尝试 + 此前每次由 agent 自己写入 `reason` 的失败
描述，内容不可信、可能缺失或被截断）。preset 作者据此在 worker hat
`instructions:` 里写清 resume 动作：先盘点已有成果与实测验收结果、只补缺
口、禁止回退或重做、已有提交不等于成功；**不要**复述注入块的格式或字段，
细节以 `crates/ralph-core/data/ralph-tools-wave.md`「Slot 自动重试」为准。

**下游 hat 的措辞**：消费 `*.wave.failed` 的 handler hat 必须知道该事件代表
「自动重试已耗尽」，不是「失败了一次」，`redrive_slots` 只是 operator 提示；
不要写成「重跑一次即可」。

**聚合期限**：启用重试后 wave 的聚合期限会按预算内的多次尝试自动放宽，
preset 作者**不需要**手动把 `aggregate_timeout_secs` 乘以尝试次数。
