# Red Team Attack Preset — Author Notes

## Preset Intent Confirmation

- **目标：** 基于一个或多个已完成开发计划，从 Git 历史反向定位实现提交、重建 Patch、设计并执行真实攻击实验、通过硬阈值筛选生成零回归修复计划。全程只读代码树，不修改生产代码，最终交付 PLAN.md 等待人工确认。

- **操作者与启动路径：** 操作者运行 `ralph run -c ralph.red-team-attack.yml -H builtin:red-team-attack`，prompt 文件 `.ralph/red-team.prompt.md` 提供开发计划路径列表。

- **输入与事实源：** 
  - 必填：一个或多个开发计划路径（通过 prompt 文件）
  - 可选：target_branch / target_commit / verification_commands / allowed_test_environments / forbidden_external_targets
  - 事实源：Git 历史（commit / patch / blame）、当前最终代码树、真实实验执行结果

- **成功条件：** 
  - 至少一个计划的 Commit Match Confidence ≥ 85（含硬证据）
  - Patch Attribution Coverage ≥ 90，Critical Claim Traceability = 100
  - 每个正式 Finding 的四项指标（Confidence / Evidence Coverage / Verifiability / Impact Certainty）全部达标
  - 独立 Reviewer 给出 PLAN_READY
  - 交付 `.ralph/red-team/PLAN.md` + `.ralph/red-team/REPORT.md` + `.ralph/red-team/QUESTIONS.md`
  - tracked tree 与锁定时一致，无生产代码修改

- **阻塞条件：** 
  - 所有计划 Commit Match 耗尽 Retry 后仍 < 85 → REJECTED_NO_RESOLVED_PLAN
  - Patch 重建耗尽 Retry 后仍不达标 → PATCH_UNRESOLVED_AFTER_RETRY
  - Finding 四项指标任一耗尽 Retry 后仍不达标 → REJECTED_AFTER_RETRY_*
  - 目标 HEAD / tree 在实验期间被修改 → TARGET_HEAD_CHANGED / TARGET_TREE_CHANGED
  - 独立 Reviewer 给出 PLAN_REJECTED

- **允许的修改范围：** 
  - 允许：`.ralph/red-team/**` 下的实验辅助物（脚本 / 证据 / 日志 / 临时副本）
  - 禁止：修改生产代码 / 正式测试 / tracked 配置 / 开发计划 / git add / git commit / git merge / git rebase / git cherry-pick / git reset --hard / 应用修复 Patch / 启动 Coding Agent

- **必须独立执行的评审：** 
  - Evidence Gate（hat 05）独立审核 Experiment Runner 的原始证据，不接受主观结论
  - Independent Reviewer（hat 07）独立审查最终 PLAN.md，不能重新解释低分实验，不能将被拒绝项重新加入计划

- **重要 artifact、生产方与消费者：**
  - `.ralph/red-team/01-target-lock.md` — target-locker 写，后续所有 hat 读（验证 HEAD/tree 不变）
  - `.ralph/red-team/02-plan-resolution.md` + `commits/PLAN-*.md` + `03-patch-reconstruction.md` + `patches/**` — plan-resolver 写，attack-surface-mapper 读
  - `.ralph/red-team/04-attack-surface.md` + `05-experiment-plan.md` — attack-surface-mapper 写，experiment-runner 读
  - `.ralph/red-team/experiments/RTE-*.md` + `evidence/RTE-*/**` + `repros/RTE-*/**` — experiment-runner 写，evidence-gate 读
  - `.ralph/red-team/07-evidence-board.md` + `07-retry-board.md` — evidence-gate 写，impact-boundary 读
  - `.ralph/red-team/08-impact-boundary.md` + `findings/RTF-*.md` + `PLAN.md` — impact-boundary 写，independent-reviewer 读
  - `.ralph/red-team/10-independent-review.md` — independent-reviewer 写，reporter 读
  - `.ralph/red-team/REPORT.md` + `QUESTIONS.md` — reporter 写，操作者读

- **execution_model：** single-chain
  **why：** 8 hat 线性流程，无并行 / 无 supervisor 需求，单链即可；并行仅存在于 hat 内部 subagent 边界

- **非目标：** 
  - 不自动执行修复（PLAN.md 不是授权）
  - 不修改生产代码 / 正式测试 / tracked 配置
  - 不重新执行原始开发计划
  - 不重新 merge 分支
  - 不处理生产环境真实数据（仅本地 / 测试 / Staging / Mock）

- **Author 推导与假设：**
  - 8-hat 合并方案：target-locker（01 锁仓）→ plan-resolver（02 Git 溯源+Patch 重建）→ attack-surface-mapper（03 攻击面+实验设计）→ experiment-runner（04 执行实验）→ evidence-gate（05 评分+Retry 决策）→ impact-boundary（06 影响边界+修复计划）→ independent-reviewer（07 独立审查）→ reporter（08 交付+完成）
  - Retry 路由：Evidence Gate 未达标 → emit `redteam.retry.required` → attack-surface-mapper（兼 Experiment Designer 职责）→ experiment-runner → evidence-gate
  - 实验执行保持独立 hat，确保证据真实性不被设计方污染
  - 操作者通过 `.ralph/red-team.prompt.md` 传入计划路径，参考 merge-batch 的 `ralph.merge.yml` + `merge.prompt.md` 模式
  - 采用模板文件机制压缩 instructions：`presets/templates/red-team-attack/` 含 experiment / finding / report / plan 四个模板，运行时通过 `ralph preset materialize-artifacts red-team-attack --plan-key <plan-key>` 复制填写
  - `redteam.plan.unresolved` 失败路径由 reporter 直接消费（不经过 reviewer），因为无计划可解析时无 PLAN.md 可审查

- **用户确认：** 已确认

---

## Hard questions — single-chain-first

1. **本 preset 的 unit 拆分能否由 executor 内部 subagent 完成？** ✓ — 本 preset 无 executor hat；experiment-runner 每次 activation 只执行一个实验，内部可自行拆分并行命令，无需 runtime 拓扑支持。
2. **任何业务 topic 是否超过一个消费者？** ✓ — 每个业务 topic 只有一个消费者（target.locked→plan-resolver；plan.resolved→attack-surface-mapper；attack.mapped→experiment-runner；experiment.done→evidence-gate；evidence.gated→impact-boundary；impact.qualified/rejected→independent-reviewer；reviewed/plan.unresolved→reporter）。`redteam.retry.required` 只由 attack-surface-mapper 消费。`redteam.impact.qualified` 和 `redteam.impact.rejected` 都由 independent-reviewer 消费（同一 hat，合理）。
3. **fallback 是否可能路由到 success？** ✗ — 不存在 fallback hat。失败路径（plan.unresolved）直接进入 reporter 的 FAIL 分支，`success: false`，不会路由到 success 终态。
4. **是否有 hat 把 tasks / progress / recovery 当业务事实？** ✗ — `tasks.enabled: false`，无 hat 使用 task API 作为业务输入；所有业务事实来自 trigger payload 或 `.ralph/red-team/` artifact 文件。
5. **是否有 rescue hat 能改变业务链路？** ✗ — 无 rescue hat；evidence-gate 的 retry 路由是显式业务事件（`redteam.retry.required`），由 attack-surface-mapper 消费，属于预设业务链路。

## Hard questions — wave fan-out

N/A — execution_model=single-chain，无 wave 拓扑。

## Hard questions — supervisor orchestration

N/A — execution_model=single-chain，未引入 `event_loop.supervisor.enabled`。

## Hard questions — Artifact-First Handoff

1. **每条写入型 hat 是否声明了当前 `.ralph/` 下的 artifact 路径集合，且拓扑层没有把这些路径描述为「preset 创建」？** ✓ — preset 注释明确写「owned by hats — not by this preset」；每个 hat 的 instructions 都声明了自己写入的 `.ralph/red-team/` 路径集合。
2. **每条 consumer hat 的 instructions 是否要求它从当前可见输入取得路径并显式读取 artifact，而不是依赖 prompt 中的长文本？** ✓ — 每个 hat 的 Steps 第 1 条都是「Read <trigger> payload. Read <artifact>」，明确要求读完整 artifact。
3. **每个被传递的完整结果、长内容或跨 hat 摘要是否都已先落盘，event / message 是否只保留短状态、短摘要、路径、必要身份与路由字段？** ✓ — event payload 只携带路径字段（`*_file_path` / `evidence_paths` / `attack_surface_path` 等）、短计数（`resolved_count` / `experiment_count`）、短状态（`control_passed` / `attack_reproduced`）、四项指标分数（0-100 整数）、必要身份（`experiment_id` / `finding_id` / `target_head` / `target_tree`）。
4. **是否有任何 hat 把 `.ralph/events.jsonl`、`.ralph/loops.json`、`.ralph/supervisor.db` 等 runtime internal ledger 当作自定义状态或 handoff 文件？** ✗ — guardrails 明确禁止；所有 hat 状态通过 trigger payload 和 `.ralph/red-team/` 业务 artifact 传递。
5. **每条声明「不落盘」的信息是否都标注了简短理由，并按恢复价值、审计价值和下游依赖解释，而非只按字符数判断？** ✓ — 短计数（`resolved_count` / `experiment_count`）、短状态（`control_passed` / `attack_reproduced`）、四项指标分数（可立即从 artifact 重算）、`verdict` 枚举（PLAN_READY/PLAN_REJECTED 单 token）均在 Payload Contract 中标注「不落盘 + 无恢复、审计或历史依赖」。

---

## Hat: target-locker

- **Q1 使命：** 锁定当前分支、HEAD、tree SHA，计算计划文件哈希，写 `01-target-lock.md`，emit `redteam.target.locked`。完成标准：lock artifact 落盘 + 单事件 emit。
- **Q2 输入 (Observe 命令 + 期望字段)：** prompt 文件中的 plan 路径列表、可选 target_branch / target_commit。命令：读 prompt + `git rev-parse` 系列 + `sha256sum`。
- **Q3 执行 (OPAC)：** Observe（读 prompt + git 命令）→ Precheck（检查 MERGE_HEAD/REBASE_HEAD/CHERRY_PICK_HEAD + dirty tree）→ Apply（写 `01-target-lock.md` + `ralph emit --policy-check`）→ Confirm（真实 emit + `--output json`）。
- **Q4 输出：** 见下方 Payload Contract 表（topic: `redteam.target.locked`）。
- **Q5 交接：** `target_head` / `target_tree` / `lock_file_path` → plan-resolver 从 trigger payload 读取并验证 HEAD/tree 一致。

### Hat: target-locker — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `redteam.target.locked` | `target_head` | string | `git rev-parse HEAD` | 本 hat 命令输出 | 不涉及 | plan-resolver 验证 HEAD 一致 | `field_docs.target_head.source` 指向 git 命令 | 必填 · 完整 lock 数据写到 `01-target-lock.md`，event 只携带 SHA |
| `redteam.target.locked` | `target_tree` | string | `git rev-parse HEAD^{tree}` | 本 hat 命令输出 | 不涉及 | plan-resolver 验证 tree 一致 | `field_docs.target_tree.source` 指向 git 命令 | 必填 · 同上 |
| `redteam.target.locked` | `plan_count` | integer | prompt 中 plan 路径数量 | 本 hat 读 prompt | 不涉及 | plan-resolver 知道需处理多少计划 | `field_docs.plan_count` | 可选 · 短计数，可从 artifact 重算；不落盘+无恢复、审计或历史依赖 |
| `redteam.target.locked` | `lock_file_path` | path | 本 hat 实际写入的 `01-target-lock.md` | 本 hat 写入结果 | 不涉及 | 所有下游 hat 读 lock 验证 HEAD/tree | `field_docs.lock_file_path` 说明文件语义 | 必填 · `.ralph/red-team/01-target-lock.md`；完整 lock 数据先落盘，event 只传路径 |

## Hat: plan-resolver

- **Q1 使命：** 对每个开发计划从 Git 历史反向定位实现提交（含 retry）、重建 Patch、建立 Claim 追踪矩阵，写 `02-plan-resolution.md` + `commits/PLAN-*.md` + `03-patch-reconstruction.md` + `patches/**`，emit `redteam.plan.resolved` 或 `redteam.plan.unresolved`。
- **Q2 输入：** `redteam.target.locked` payload（`target_head` / `target_tree` / `lock_file_path` / `plan_count`）；prompt 中的 plan 路径。命令：读 trigger payload + 读 lock artifact + `git log` / `git show` / `git blame` / `git format-patch` / `git diff` 系列。
- **Q3 执行：** Observe（读 payload + lock + git 命令）→ Precheck（验证 HEAD/tree == lock；commit match score ≥85 + 硬证据；patch coverage ≥90 + critical traceability =100）→ Apply（写 artifact + `--policy-check`）→ Confirm（真实 emit）。
- **Q4 输出：** 见 Payload Contract（topics: `redteam.plan.resolved` / `redteam.plan.unresolved`）。
- **Q5 交接：** `resolution_file_path` → attack-surface-mapper 读取完整 resolution + patch 数据。

### Hat: plan-resolver — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `redteam.plan.resolved` | `resolved_count` | integer | `02-plan-resolution.md` 中达标计划数 | 本 hat 写入结果 | 不涉及 | attack-surface-mapper 知道多少计划可攻击 | `field_docs.resolved_count` | 可选 · 短计数，可从 artifact 重算；不落盘+无恢复、审计或历史依赖 |
| `redteam.plan.resolved` | `unresolved_count` | integer | `02-plan-resolution.md` 中不达标计划数 | 本 hat 写入结果 | 不涉及 | attack-surface-mapper 知道多少计划被排除 | `field_docs.unresolved_count` | 可选 · 同上 |
| `redteam.plan.resolved` | `resolution_file_path` | path | 本 hat 实际写入的 `02-plan-resolution.md` | 本 hat 写入结果 | 不涉及 | attack-surface-mapper 读完整 resolution + patch | `field_docs.resolution_file_path` | 必填 · `.ralph/red-team/02-plan-resolution.md` + `commits/PLAN-*.md` + `03-patch-reconstruction.md` + `patches/**`；完整数据先落盘，event 只传路径 |
| `redteam.plan.unresolved` | `reason` | enum | 本 hat 最终 retry 状态 | 本 hat 决策 | 不涉及 | reporter 写 FAIL 报告 | `field_docs.reason` + `allowed_values` | 不需要 · 单 token 枚举，无恢复、审计或下游历史依赖 |
| `redteam.plan.unresolved` | `resolution_file_path` | path | 本 hat 实际写入的 `02-plan-resolution.md` | 本 hat 写入结果 | 不涉及 | reporter 读 retry 历史写报告 | `field_docs.resolution_file_path` | 必填 · 同上 |

## Hat: attack-surface-mapper

- **Q1 使命：** （on plan.resolved）识别攻击面、设计实验，写 `04-attack-surface.md` + `05-experiment-plan.md`，emit `redteam.attack.mapped`；（on retry.required）读 retry delta、重新设计实验、更新 `05-experiment-plan.md`，emit `redteam.attack.mapped`。
- **Q2 输入：** on plan.resolved：`redteam.plan.resolved` payload + resolution/patch artifacts；on retry.required：`redteam.retry.required` payload（`experiment_id` / `failed_metrics` / `retry_delta` / `retry_board_path`）+ 原 experiment artifact。命令：读 trigger payload + 读 artifacts + `git rev-parse` 验证 HEAD/tree。
- **Q3 执行：** Observe（读 payload + artifacts）→ Precheck（验证 HEAD/tree == lock；攻击维度覆盖检查）→ Apply（materialize 模板 + 写 `04-attack-surface.md` + `05-experiment-plan.md` + `--policy-check`）→ Confirm（真实 emit）。
- **Q4 输出：** 见 Payload Contract（topic: `redteam.attack.mapped`）。
- **Q5 交接：** `attack_surface_path` / `experiment_plan_path` → experiment-runner 读完整实验设计。

### Hat: attack-surface-mapper — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `redteam.attack.mapped` | `surface_count` | integer | `04-attack-surface.md` 中攻击面数量 | 本 hat 写入结果 | 不涉及 | experiment-runner 知道实验范围 | `field_docs.surface_count` | 可选 · 短计数，可从 artifact 重算；不落盘+无恢复、审计或历史依赖 |
| `redteam.attack.mapped` | `experiment_count` | integer | `05-experiment-plan.md` 中实验数量 | 本 hat 写入结果 | 不涉及 | experiment-runner 知道需执行多少实验 | `field_docs.experiment_count` | 可选 · 同上 |
| `redteam.attack.mapped` | `attack_surface_path` | path | 本 hat 实际写入的 `04-attack-surface.md` | 本 hat 写入结果 | 不涉及 | experiment-runner 读攻击面详情 | `field_docs.attack_surface_path` | 必填 · `.ralph/red-team/04-attack-surface.md`；完整攻击面数据先落盘，event 只传路径 |
| `redteam.attack.mapped` | `experiment_plan_path` | path | 本 hat 实际写入的 `05-experiment-plan.md` | 本 hat 写入结果 | 不涉及 | experiment-runner 读实验设计 | `field_docs.experiment_plan_path` | 必填 · `.ralph/red-team/05-experiment-plan.md`；完整实验设计先落盘，event 只传路径 |

## Hat: experiment-runner

- **Q1 使命：** 每次 activation 执行一个实验（控制组 + 攻击组 + 重复 + 清理），保存原始证据到 `evidence/RTE-*/**`，写 `experiments/RTE-*.md`，验证 tracked tree 未变，emit `redteam.experiment.done`。
- **Q2 输入：** `redteam.attack.mapped` payload（`experiment_plan_path` / `attack_surface_path`）+ experiment plan artifact。命令：读 trigger payload + 读 experiment plan + 执行实验命令 + `git status` / `git diff` 验证 tree。
- **Q3 执行：** Observe（读 payload + plan）→ Precheck（验证 HEAD/tree == lock；环境隔离检查）→ Apply（执行控制组/攻击组 + 保存证据 + 写 experiment artifact + 验证 tree 未变 + `--policy-check`）→ Confirm（真实 emit）。
- **Q4 输出：** 见 Payload Contract（topic: `redteam.experiment.done`）。
- **Q5 交接：** `experiment_file_path` / `evidence_paths` → evidence-gate 读完整实验结果和原始证据。

### Hat: experiment-runner — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `redteam.experiment.done` | `experiment_id` | string | experiment plan 中分配的 RTE-NNN | 本 hat 读 plan | 不涉及 | evidence-gate 定位实验 | `field_docs.experiment_id` 说明格式 | 不需要 · 短标识符，无恢复、审计或下游历史依赖 |
| `redteam.experiment.done` | `control_passed` | boolean | 控制组执行结果 | 本 hat 执行结果 | 不涉及 | evidence-gate 二元门禁判定 | `field_docs.control_passed` | 不需要 · 短状态枚举，无恢复、审计或下游历史依赖 |
| `redteam.experiment.done` | `attack_reproduced` | boolean | 攻击组执行结果 | 本 hat 执行结果 | 不涉及 | evidence-gate 二元门禁判定 | `field_docs.attack_reproduced` | 不需要 · 同上 |
| `redteam.experiment.done` | `evidence_paths` | array | 本 hat 实际保存的 `evidence/RTE-*/**` 文件 | 本 hat 写入结果 | 不涉及 | evidence-gate 读原始证据评分 | `field_docs.evidence_paths` | 必填 · `.ralph/red-team/evidence/RTE-<NNN>/**`；原始证据先落盘，event 只传路径数组 |
| `redteam.experiment.done` | `experiment_file_path` | path | 本 hat 实际写入的 `experiments/RTE-*.md` | 本 hat 写入结果 | 不涉及 | evidence-gate 读完整实验记录 | `field_docs.experiment_file_path` | 必填 · `.ralph/red-team/experiments/RTE-<NNN>.md`；完整实验记录先落盘，event 只传路径 |

## Hat: evidence-gate

- **Q1 使命：** 独立审核 experiment 的二元门禁和四项指标，全部达标 emit `redteam.evidence.gated`，任一未达标生成 retry delta 并 emit `redteam.retry.required`（回 attack-surface-mapper）。
- **Q2 输入：** `redteam.experiment.done` payload（`experiment_id` / `control_passed` / `attack_reproduced` / `evidence_paths` / `experiment_file_path`）+ experiment artifact + 所有原始证据文件。命令：读 trigger payload + 读 experiment artifact + 读所有 evidence 文件。
- **Q3 执行：** Observe（读 payload + artifact + evidence）→ Precheck（验证二元门禁 + mandatory evidence）→ Apply（计算四项指标 + 写 `07-evidence-board.md` 或 `07-retry-board.md` + `--policy-check`）→ Confirm（真实 emit）。
- **Q4 输出：** 见 Payload Contract（topics: `redteam.evidence.gated` / `redteam.retry.required`）。
- **Q5 交接：** on gated：`evidence_board_path` + 四项分数 → impact-boundary 读完整评分结果；on retry：`retry_delta` + `failed_metrics` → attack-surface-mapper 重新设计实验。

### Hat: evidence-gate — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `redteam.evidence.gated` | `experiment_id` | string | trigger payload `experiment_id` | trigger payload | 必须匹配 trigger | impact-boundary 定位实验 | `field_docs.experiment_id` | 不需要 · 短标识符，无恢复、审计或下游历史依赖 |
| `redteam.evidence.gated` | `confidence` | integer | 本 hat 从原始证据计算 | 本 hat 评分结果 | 不涉及 | impact-boundary 验证阈值 | `field_docs.confidence` 说明评分构成 | 不需要 · 短计数，可从 evidence board 重算；不落盘+无恢复、审计或历史依赖 |
| `redteam.evidence.gated` | `evidence_coverage` | integer | 本 hat 从 mandatory evidence 计算 | 本 hat 评分结果 | 不涉及 | impact-boundary 验证阈值 | `field_docs.evidence_coverage` | 不需要 · 同上 |
| `redteam.evidence.gated` | `verifiability` | integer | 本 hat 从可复现性分析计算 | 本 hat 评分结果 | 不涉及 | impact-boundary 验证阈值 | `field_docs.verifiability` | 不需要 · 同上 |
| `redteam.evidence.gated` | `impact_certainty` | integer | 本 hat 从初步影响分析计算 | 本 hat 评分结果 | 不涉及 | impact-boundary 重新评分（不沿用） | `field_docs.impact_certainty` | 不需要 · 同上 |
| `redteam.evidence.gated` | `evidence_board_path` | path | 本 hat 实际写入的 `07-evidence-board.md` | 本 hat 写入结果 | 不涉及 | impact-boundary 读完整评分依据 | `field_docs.evidence_board_path` | 必填 · `.ralph/red-team/07-evidence-board.md`；完整评分依据先落盘，event 只传路径 |
| `redteam.retry.required` | `experiment_id` | string | trigger payload `experiment_id` | trigger payload | 必须匹配 trigger | attack-surface-mapper 定位需重新设计的实验 | `field_docs.experiment_id` | 不需要 · 短标识符 |
| `redteam.retry.required` | `attempt` | integer | 本 hat 从 retry 历史计数 | 本 hat 计数结果 | 不涉及 | attack-surface-mapper 知道当前 retry 轮次 | `field_docs.attempt` 说明预算限制 | 不需要 · 短计数 |
| `redteam.retry.required` | `failed_metrics` | array | 本 hat 评分结果中未达标指标 | 本 hat 评分结果 | 不涉及 | attack-surface-mapper 定向补强 | `field_docs.failed_metrics` | 不需要 · 短枚举数组 |
| `redteam.retry.required` | `retry_delta` | string | 本 hat 从失败分析生成 | 本 hat 分析结果 | 不涉及 | attack-surface-mapper 按 delta 重新设计 | `field_docs.retry_delta` 要求具体非空 | 必填 · 长文本（>200字符）时落盘到 `07-retry-board.md`，event 携带短 delta + `retry_board_path` |
| `redteam.retry.required` | `retry_board_path` | path | 本 hat 实际写入的 `07-retry-board.md` | 本 hat 写入结果 | 不涉及 | attack-surface-mapper 读完整 retry 历史 | `field_docs.retry_board_path` | 必填 · `.ralph/red-team/07-retry-board.md`；完整 retry 历史先落盘，event 只传路径 |

## Hat: impact-boundary

- **Q1 使命：** 对达标实验继续动手验证影响边界（调用者/消费者/配置/生命周期/兼容性），重新评分 impact_certainty，达标则创建正式 finding（`findings/RTF-*.md`）并写 `08-impact-boundary.md`，所有 finding 处理完后写 `PLAN.md`，emit `redteam.impact.qualified` 或 `redteam.impact.rejected`。
- **Q2 输入：** `redteam.evidence.gated` payload（`experiment_id` / 四项分数 / `evidence_board_path`）+ evidence board + experiment artifact。命令：读 trigger payload + 读 artifacts + 执行边界实验命令（测试/调用/配置/重启/兼容性验证）。
- **Q3 执行：** Observe（读 payload + artifacts）→ Precheck（验证 HEAD/tree == lock）→ Apply（执行边界实验 + 重新评分 + materialize 模板 + 写 finding + `08-impact-boundary.md` + 全部处理完后写 `PLAN.md` + `--policy-check`）→ Confirm（真实 emit）。
- **Q4 输出：** 见 Payload Contract（topics: `redteam.impact.qualified` / `redteam.impact.rejected`）。
- **Q5 交接：** `finding_file_path` / `impact_file_path` → independent-reviewer 读完整 finding 和影响边界数据。

### Hat: impact-boundary — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `redteam.impact.qualified` | `finding_id` | string | 本 hat 分配的 RTF-NNN | 本 hat 分配结果 | 不涉及 | independent-reviewer 定位 finding | `field_docs.finding_id` 说明格式 | 不需要 · 短标识符 |
| `redteam.impact.qualified` | `severity` | enum | 本 hat 影响分析结果 | 本 hat 决策 | 不涉及 | independent-reviewer 验证阈值（P0/P1 更高） | `field_docs.severity` + `allowed_values` | 不需要 · 单 token 枚举 |
| `redteam.impact.qualified` | `finding_file_path` | path | 本 hat 实际写入的 `findings/RTF-*.md` | 本 hat 写入结果 | 不涉及 | independent-reviewer 读完整 finding | `field_docs.finding_file_path` | 必填 · `.ralph/red-team/findings/RTF-<NNN>.md`；完整 finding 先落盘，event 只传路径 |
| `redteam.impact.qualified` | `impact_file_path` | path | 本 hat 实际写入的 `08-impact-boundary.md` | 本 hat 写入结果 | 不涉及 | independent-reviewer 读完整边界数据 | `field_docs.impact_file_path` | 必填 · `.ralph/red-team/08-impact-boundary.md`；完整边界数据先落盘，event 只传路径 |
| `redteam.impact.rejected` | `experiment_id` | string | trigger payload `experiment_id` | trigger payload | 必须匹配 trigger | independent-reviewer 记录被拒绝候选 | `field_docs.experiment_id` | 不需要 · 短标识符 |
| `redteam.impact.rejected` | `reason` | enum | 本 hat 最终 retry 状态 | 本 hat 决策 | 不涉及 | independent-reviewer 验证拒绝合法性 | `field_docs.reason` + `allowed_values` | 不需要 · 单 token 枚举 |
| `redteam.impact.rejected` | `impact_file_path` | path | 本 hat 实际写入的 `08-impact-boundary.md` | 本 hat 写入结果 | 不涉及 | independent-reviewer 读拒绝历史 | `field_docs.impact_file_path` | 必填 · 同上 |

## Hat: independent-reviewer

- **Q1 使命：** 独立审查所有 artifact 和 `PLAN.md`，验证审查清单全部通过，写 `10-independent-review.md`，emit `redteam.reviewed`（verdict: PLAN_READY 或 PLAN_REJECTED）。
- **Q2 输入：** `redteam.impact.qualified` / `redteam.impact.rejected` payload + 所有上游 artifact（target-lock / plan-resolution / patch-reconstruction / attack-surface / experiment-plan / experiments / evidence board / retry board / impact boundary / findings / PLAN.md）。命令：读 trigger payload + 读所有 artifacts + `git rev-parse` 验证 HEAD/tree。
- **Q3 执行：** Observe（读 payload + 所有 artifacts）→ Precheck（验证 HEAD/tree == lock + 审查清单）→ Apply（写 `10-independent-review.md` + `--policy-check`）→ Confirm（真实 emit）。
- **Q4 输出：** 见 Payload Contract（topic: `redteam.reviewed`）。
- **Q5 交接：** `verdict` / `review_file_path` / `plan_file_path` → reporter 读审查结论并写最终报告。

### Hat: independent-reviewer — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `redteam.reviewed` | `verdict` | enum | 本 hat 审查清单结果 | 本 hat 决策 | 不涉及 | reporter 决定 success true/false | `field_docs.verdict` + `allowed_values` | 不需要 · 单 token 枚举 |
| `redteam.reviewed` | `review_file_path` | path | 本 hat 实际写入的 `10-independent-review.md` | 本 hat 写入结果 | 不涉及 | reporter 读审查详情写报告 | `field_docs.review_file_path` | 必填 · `.ralph/red-team/10-independent-review.md`；完整审查记录先落盘，event 只传路径 |
| `redteam.reviewed` | `plan_file_path` | path | impact-boundary 写入的 `PLAN.md`，本 hat 验证存在 | 本 hat 验证结果 | 不涉及 | reporter 验证 plan 可读并随报告交付 | `field_docs.plan_file_path` | 必填 · `.ralph/red-team/PLAN.md`（impact-boundary 写，reviewer 验证，reporter 交付） |

## Hat: reporter

- **Q1 使命：** （on reviewed）写 `REPORT.md` + `QUESTIONS.md`，验证 `PLAN.md` 可读，emit `redteam.complete`（success = verdict == PLAN_READY）；（on plan.unresolved）写 FAIL 版 `REPORT.md` + `QUESTIONS.md`，emit `redteam.complete`（success: false, plan_path: ""）。
- **Q2 输入：** on reviewed：`redteam.reviewed` payload（`verdict` / `review_file_path` / `plan_file_path`）+ review artifact + 所有上游 artifact；on plan.unresolved：`redteam.plan.unresolved` payload（`reason` / `resolution_file_path`）+ resolution artifact。命令：读 trigger payload + 读 artifacts + `test -f` 验证文件可读。
- **Q3 执行：** Observe（读 payload + artifacts）→ Precheck（验证 HEAD/tree == lock + `test -f` 三个交付文件）→ Apply（materialize 模板 + 写 `REPORT.md` + `QUESTIONS.md` + `--policy-check`）→ Confirm（真实 emit + `--output json` + 打印 `DELIVERABLE_PATH:`）。
- **Q4 输出：** 见 Payload Contract（topic: `redteam.complete`）。
- **Q5 交接：** `report_path` / `plan_path` / `questions_path` → 操作者最终阅读并确认。

### Hat: reporter — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| `redteam.complete` | `success` | boolean | trigger `verdict` == PLAN_READY（on reviewed）/ false（on plan.unresolved） | trigger payload + 本 hat 决策 | 不涉及 | runtime 完成门禁 + 操作者 | `field_docs.success` | 不需要 · 短状态枚举 |
| `redteam.complete` | `report_path` | path | 本 hat 实际写入的 `REPORT.md` | 本 hat 写入结果 | 不涉及 | 操作者读最终报告 | `field_docs.report_path` 要求 `test -f` 验证 | 必填 · `.ralph/red-team/REPORT.md`；完整报告先落盘，event 只传路径 |
| `redteam.complete` | `plan_path` | path | impact-boundary 写入的 `PLAN.md`（on reviewed）/ 空字符串（on plan.unresolved） | 本 hat 验证结果 | 不涉及 | 操作者读修复计划 | `field_docs.plan_path` | 必填 · `.ralph/red-team/PLAN.md`（impact-boundary 写，reporter 验证并交付） |
| `redteam.complete` | `questions_path` | path | 本 hat 实际写入的 `QUESTIONS.md` | 本 hat 写入结果 | 不涉及 | 操作者读需确认问题 | `field_docs.questions_path` | 必填 · `.ralph/red-team/QUESTIONS.md`（可为空文件）；先落盘，event 只传路径 |

---

## 收尾双事件终态复核

本 preset 的 reporter hat `publishes` 只包含 `redteam.complete`（completion_promise），不包含 `event_loop.required_events[]` 中的 `redteam.target.locked`。因此**不触发**收尾双事件终态例外——reporter 每次 activation 只 emit 一条 `redteam.complete`，符合单事件预算。

## 全路径 vs 成功脊门禁复核

`event_loop.required_events: ["redteam.target.locked"]` 是所有完成路径（含 `redteam.plan.unresolved` 失败早退）都会经过的收敛 topic：
- 成功路径：start → target.locked → plan.resolved → ... → reviewed → complete
- 失败路径：start → target.locked → plan.unresolved → complete

两条路径都经过 `target.locked`，符合 `required_events` 语义。成功脊专用的 `reviewed` 不放入 `required_events`，避免 `topology.required_event_not_on_all_paths`。

## 模板文件机制决定

采用模板文件机制。原因：attack-surface-mapper / experiment-runner / evidence-gate 的 instructions 包含大段固定格式文档（实验模板、评分标准、Retry 路由表），超过 80 行。模板文件：`presets/templates/red-team-attack/experiment.template.yml` / `finding.template.yml` / `report.template.md` / `plan.template.md`。已同步扩展 `crates/ralph-cli/src/builtin_artifact_templates.rs`（`RED_TEAM_ATTACK_TEMPLATES` + `templates_for_preset` + `default_red_team_templates_dir` + materialize 校验）和 `crates/ralph-cli/build.rs`（`copy_preset_templates` 泛化 + red-team-attack 分支）。

## Builtin 7 点同步清单（已执行）

1. `crates/ralph-core/src/event_loop/mod.rs` — N/A（无新终态语义，`redteam.complete` 走标准 completion_promise 路径）
2. `crates/ralph-core/src/preset_lint/` — N/A（无新 lint 规则）
3. `crates/ralph-core/tests/scenarios/*.yml` + `scenarios.rs` — N/A（新 preset 无 BDD scenario，后续按需补）
4. `crates/ralph-core/src/config/loop_config.rs`、`preflight.rs`、`config_resolution.rs` — N/A（无新配置字段）
5. `crates/ralph-cli/src/presets.rs` — ✓ 已加 `red-team-attack` EmbeddedPreset + 更新 3 个计数测试 + zsh_values 列表
6. `presets/manifest.yml`、`presets/index.json` — ✓ 已加 `red-team-attack`
7. `CLAUDE.md` / `AGENTS.md`、`.cursor/rules/multi-hat-isolation.mdc`、`scripts/ralph-zsh-plugin.zsh` — ✓ CLAUDE.md/AGENTS.md 已加 builtin 列表条目；zsh 已加 `builtin:red-team-attack` + 描述；`.cursor/rules/multi-hat-isolation.mdc` 的 builtin 列表由该文件自身维护（见其 `**/*.mdc` glob），本 notes 不重复改

## 校验结果

- `cargo build -p ralph-cli`：✓ 通过（1 个 dead_code 警告：`default_red_team_templates_dir` 预留给 materialize 命令后续接入）
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`：✓ 11/11 通过
- `cargo nextest run -p ralph-cli --bin ralph -- presets`：✓ 57/57 通过（含 `test_all_embedded_presets_pass_strict_lint` / `test_all_public_presets_pass_authoring_contract`）
- `./scripts/run-tests.sh`：1 个失败（`implementation_review_dispatcher_contract_has_no_resume_redrive`），已验证为修改前既有问题（git stash 后依然失败），与本 preset 无关

## Review 修复记录（2026-07-28）

AAF 评审发现 3 个问题，已全部修复：

1. **P0-1: PLAN.md 完成无显式信号** — impact-boundary 写 `PLAN.md` 后没有 emit 事件，independent-reviewer 可能在 `PLAN.md` 写完前被触发。
   - 修复：新增 `redteam.plan.ready` topic；impact-boundary 在「所有 findings 处理完且 PLAN.md 写完」时 emit `redteam.plan.ready`；independent-reviewer 的 trigger 改为 `redteam.plan.ready` + `redteam.impact.rejected`。
   - 同时删除 `redteam.impact.qualified` topic（原设计让 reviewer 跟踪每个 qualified finding，但 isolated 模式下中间信号无 consumer 会挂起 lint）。impact-boundary 在单个 finding 达标时只写 finding artifact 不 emit；全部完成后才 emit `redteam.plan.ready`。

2. **P1-1: `lock_status` 幽灵字段** — target-locker instructions 要求 emit `lock_status: locked/failed`，plan-resolver 依赖它做判断，但 schema `required_fields` 未声明。
   - 修复：`redteam.target.locked` schema 增加 `lock_status` 到 `required_fields` 和 `field_docs`（meaning / source / fill_rule 完整）。

3. **P1-2: `TARGET_HEAD_CHANGED` 不在 `reason` 允许值中** — plan-resolver instructions 说 HEAD 不匹配时 emit `redteam.plan.unresolved` with `reason: TARGET_HEAD_CHANGED`，但 schema `fill_rule` 只提到 `REJECTED_NO_RESOLVED_PLAN`。
   - 修复：`redteam.plan.unresolved` schema `field_docs.reason.fill_rule` 扩展为 `One of REJECTED_NO_RESOLVED_PLAN, TARGET_HEAD_CHANGED, TARGET_TREE_CHANGED`。

修复后校验：
- `ralph preset check -H builtin:red-team-attack --strict`：✓ PASS
- `cargo nextest run -p ralph-cli --bin ralph -- presets`：✓ 57/57 通过

## 建议

调用 `ralph-preset-review` 对 `presets/en/red-team-attack.yml` + `presets/schemas/red-team-attack.yml` 做 AAF 评审（不替代 `ralph preset check -H builtin:red-team-attack --strict`）。
