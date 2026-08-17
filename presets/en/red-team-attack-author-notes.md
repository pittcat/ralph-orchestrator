# Red Team Attack Preset — Author Notes

## Preset Intent Confirmation

- **目标：** 从已完成开发计划的 Git 历史反向定位实现，重建 patch，顺序执行全部攻击实验，保留每个实验的原始证据和独立评分，汇总合格 Finding 后生成零回归修复计划。
- **操作者与启动路径：** 操作者准备 `.ralph/red-team.prompt.md`，运行 `ralph run -c ralph.red-team-attack.yml -H builtin:red-team-attack`，最终阅读 `.ralph/red-team/REPORT.md`、`PLAN.md` 和 `QUESTIONS.md`。
- **输入与事实源：** prompt 文件、Git HEAD/tree、开发计划、`.ralph/red-team/04-attack-surface.md`、`05-experiment-plan.md`、各 RTE 实验 artifact、原始证据和 evidence board。
- **成功条件：** 所有计划实验均已记录；至少一个实验通过二元证据门禁和四项阈值；impact-boundary 写出至少一个合格 Finding 和 `PLAN.md`；independent-reviewer 发出 `PLAN_READY`；reporter 发出 `redteam.complete(success=true)`。
- **阻塞条件：** target/tree 改变、生产树变脏、artifact 不可读、实验计划连续 3 次无法通过通用可执行性校验、实验队列 handoff 无法修复、所有实验均未形成合格证据，或独立终审拒绝计划。
- **允许的修改范围：** 只允许写 `.ralph/red-team/` 业务 artifact、临时隔离环境和证据；禁止修改生产代码、正式测试、tracked 配置、Git 历史及运行时内部 ledger。
- **必须独立执行的评审：** evidence-gate 只读原始证据；impact-boundary 重新执行影响边界验证；independent-reviewer 独立检查全部 artifact、Finding、阈值和修复范围。
- **重要 artifact、生产方与消费者：** 见下方 Artifact Ownership 表；事件只传短状态、计数、路径和路由字段，完整结果留在 artifact。
- **execution_model：** `single-chain`；**why：** 实验必须按证据结果串行推进，不需要 wave 或 supervisor。
- **非目标：** 不自动重试被证据拒绝的实验，不生成没有正式 Finding 的 `PLAN.md`，不启动生产代码修复，不把通用 runtime 日志当作红队结论。
- **Author 推导与假设：** 实验计划先经过 `experiment-plan-validator`；计划无效时只回 mapper 重写当前实验，最多 3 次，耗尽后才走 `redteam.failed`；证据失败只计入 rejected 并继续独立实验；真正的生产者协议失败仍立即走 `redteam.failed`。
- **用户确认：** 已确认；Gate Scope=`hard`；用户确认单链、继续剩余实验、最终只交付合格 Finding。

## Gate Scope

`hard` 模式要求关键 handoff 的 Critical Ambiguities 与 Critical Unverified Assumptions 均为零；它不替代 schema、payload audit 或 precheck。

| 能力位置 | Gate Scope | 事实依据 |
|---|---|---|
| 攻击面与实验计划声明 | hard | `redteam.experiment.plan.ready` 携带实验计划路径、数量和验证尝试 |
| 实验计划可执行性校验 | hard | `experiment-plan-validator` 生成项目无关的可执行性报告并决定 valid/invalid |
| 单实验原始证据交接 | hard | `redteam.experiment.done` 携带证据路径、manifest 和 ledger hash |
| 实验证据汇总与队列继续 | hard | `redteam.experiment.next` / `redteam.evidence.gated` 携带计数与 evidence board |
| 失败收敛和最终交付 | hard | `redteam.failed` / `redteam.complete` 携带可读 artifact 路径和成功语义 |

## Key-stage event gate

`precheck` 是 LLM 主观证据检查；`payload_consistency` 是确定性字段矛盾检查；两类 budget 独立，均为 3。最终终态只使用确定性 gate，因为 precheck 耗尽会发 `plan.blocked`，不能截断已经写好的报告。

| key_stage | guard_selection | precheck_guard | precheck_retry_budget | payload_consistency_guard | payload_consistency_retry_budget | reason | confirmation_status |
|---|---|---:|---:|---:|---:|---|---|
| `redteam.experiment.plan.ready` → 计划校验 | payload_consistency | false | null | true | 3 | 计划路径、数量和重写次数先做确定性自洽检查，主观可行性检查由 validator 的 valid/invalid handoff 承担 | confirmed |
| `redteam.experiment.plan.valid` → runner 释放 | both | true | 3 | true | 3 | 只有有项目发现证据的可执行计划才能进入 runner | confirmed |
| `redteam.experiment.plan.invalid` → mapper 重写 | both | true | 3 | true | 3 | 失败实验、原因和重写次数必须可路由且不能超过预算 | confirmed |
| `redteam.experiment.done` → 单实验证据 | both | true | 3 | true | 3 | 缺 manifest/hash 时不得进入评分 | confirmed |
| `redteam.experiment.next` → 继续队列 | payload_consistency | false | null | true | 3 | 这是确定性计数和路由 handoff，无需第二次主观评分 | confirmed |
| `redteam.evidence.gated` → 汇总交接 | both | true | 3 | true | 3 | 必须证明全部实验已结算且至少一个合格 | confirmed |
| `redteam.failed` → 失败报告 | payload_consistency | false | null | true | 3 | 失败路径必须可收敛，不能被主观 gate 截断 | confirmed |
| `redteam.complete` → 最终交付 | payload_consistency | false | null | true | 3 | 终态成功/计划路径关系由确定性规则保护 | confirmed |

## Topology

```text
redteam.start
  → target.locked → plan.resolved → plan.ready
  → plan.valid → experiment.done → evidence-gate
       ├─ plan.invalid → mapper 重写当前 RTE（最多 3 次）
       ├─ rejected + remaining → experiment.next → experiment.done
       ├─ qualified + remaining → experiment.next → experiment.done
       ├─ queue exhausted + qualified → evidence.gated → plan.ready → reviewed → complete
       └─ queue exhausted + none qualified → failed → complete(success=false)

计划校验耗尽 / producer protocol failure → failed → reporter → complete(success=false)
```

`redteam.experiment.plan.ready` 只有 mapper 发布，`redteam.experiment.plan.valid` / `.invalid` 只有 validator 发布，`redteam.experiment.next` 只有 evidence-gate 发布；普通 handoff 不使用 `--triggered`。

## Hard questions — single-chain-first

1. **unit 是否可由 executor 内部 subagent 完成？** ✗。每个实验都需要独立 raw evidence、manifest、ledger hash 和 evidence-gate 结算；压进一个 subagent 会丢失独立 handoff。
2. **是否有业务 topic 超过一个消费者？** ✗。每个成功 topic 只有一个下游；`redteam.failed` 只有 reporter 消费。
3. **fallback 是否可能路由到 success？** ✗。失败只到 reporter 的 `success=false`；没有失败到成功脊的边。
4. **是否把 tasks/progress/recovery 当业务事实？** ✗。队列事实来自 experiment plan、RTE artifact 和 evidence board，不来自 runtime ledger。
5. **是否有 rescue hat 能改变业务链路？** ✗。失败由 operator 在下一次 loop 决定是否重跑。

Wave 与 supervisor hard questions：**N/A**，因为 `execution_model=single-chain`，没有 wave dispatcher 或 supervisor。

## Artifact Ownership

| artifact | 生产方 | 消费方 | 生命周期 |
|---|---|---|---|
| `01-target-lock.md` | target-locker | 所有后续 producer | 保留到 operator 归档 |
| `scope-manifest.json`、plan-resolution、patches | plan-resolver | attack-surface-mapper、reviewer、reporter | 保留到 operator 归档 |
| `04-attack-surface.md`、`05-experiment-plan.md` | attack-surface-mapper | experiment-runner、precheck gate、reviewer | 保留到 operator 归档 |
| `plan-validation-attempt-<N>.md` | experiment-plan-validator | mapper、runner、reviewer、reporter | 保留到 operator 归档 |
| `experiments/RTE-*.md` | experiment-runner | evidence-gate、impact-boundary、reviewer | 保留到 operator 归档 |
| `evidence/RTE-*/**`、`evidence-manifest.json` | experiment-runner | evidence-gate、reviewer | 保留到 operator 归档 |
| `07-evidence-board.md` | evidence-gate | experiment-runner、impact-boundary、reviewer、reporter | 保留到 operator 归档 |
| `07-retry-board.md` | evidence-gate | reporter、operator | 保留到 operator 归档 |
| `failures/<stage>.md` | 失败 producer | reporter、operator | 保留到 operator 归档 |
| `findings/RTF-*.md`、`08-impact-boundary.md`、`PLAN.md` | impact-boundary | independent-reviewer、reporter、operator | 保留到 operator 归档 |
| `10-independent-review.md` | independent-reviewer | reporter、operator | 保留到 operator 归档 |
| `REPORT.md`、`QUESTIONS.md` | reporter | operator | operator 归档或删除 |

## Artifact-First Handoff hard questions

1. 每个写入型 hat 均声明当前 `.ralph/red-team/` artifact 路径；preset 不自称文件生产方。**✓**，见 Artifact Ownership。
2. 每个 consumer 均从 trigger payload 或 evidence board 取得路径后读取完整文件。**✓**，见各 hat instructions。
3. 完整结果、原始证据、长摘要和决策依据均先落盘，event 只携带路径、计数和短状态。**✓**。
4. 是否把 `.ralph/events.jsonl`、`.ralph/loops.json`、`.ralph/supervisor.db` 当业务接口？**✗**，所有状态通过 artifact 和公开 runtime API 取得。
5. 不落盘例外是否有恢复、审计、下游依赖理由？**✓**，仅短计数和枚举状态不落盘，因可由当前 board 低成本重算且不承担审计事实。

## AAF 五问与 Payload Contract

以下每张表按 activated-hat 视角填写；`schema` 列直接对应 `presets/schemas/red-team-attack.yml` 的 `field_docs`，实际路径必须由当前 activation 写入并通过 `test -f` 验证。

### target-locker

- **Q1 使命：** 锁定 HEAD/tree 和输入计划摘要；完成标准是写出 `01-target-lock.md` 并发出唯一 `redteam.target.locked`。
- **Q2 输入：** 读取 prompt 文件中的计划路径和可选 target 参数；用 Git 命令取得 HEAD、tree、branch、状态和进行中的操作标记。
- **Q3 执行：** Observe prompt/Git → Precheck 锁 artifact 和 payload → Apply 写文件并 policy-check → Confirm 读取 hat-channel 事件。
- **Q4 输出：** 下表；失败仍发同一 topic 的 `lock_status=failed`，由 plan-resolver 转换失败汇。
- **Q5 交接：** `lock_file_path` 与 target SHA 进入 trigger payload；plan-resolver 读取锁文件并重新验证 HEAD/tree。

| topic | field | value source / visibility | downstream use | schema / artifact |
|---|---|---|---|---|
| `redteam.target.locked` | `target_head` | `git rev-parse HEAD`；当前 hat shell 输出 | 后续 producer 比对目标 | `field_docs.target_head`; 锁文件落盘 |
| `redteam.target.locked` | `target_tree` | `git rev-parse HEAD^{tree}`；当前 hat shell 输出 | 后续 producer 比对 tree | `field_docs.target_tree`; 锁文件落盘 |
| `redteam.target.locked` | `plan_count` | prompt 路径计数；当前 hat 读取 prompt | 解析计划数量 | `field_docs.plan_count`; 计数短值不单独落盘 |
| `redteam.target.locked` | `lock_file_path` | 当前 activation 写入的锁文件 | plan-resolver 读取完整锁事实 | `field_docs.lock_file_path`; 必填 `.ralph/red-team/01-target-lock.md` |
| `redteam.target.locked` | `lock_status` | Git 状态检查结果 | 决定继续解析还是转失败 | `field_docs.lock_status`; 状态短值不单独落盘，完整原因在锁/失败 artifact |

### plan-resolver

- **Q1 使命：** 为每个计划找到真实实现提交、重建 patch 和 scope manifest；只发 resolved 或 failed。
- **Q2 输入：** 读取 `redteam.target.locked`、`01-target-lock.md` 和 prompt 计划路径；Git 历史是 commit 事实源。
- **Q3 执行：** Observe 锁和计划 → Precheck scope manifest/digest → Apply 写 resolution、patch 和 manifest → Confirm policy-check 后发唯一事件。
- **Q4 输出：** resolved 字段必须来自 artifact 与 Git；失败字段来自本 activation 的 failure artifact。
- **Q5 交接：** `resolution_file_path`、`scope_manifest_path`、`resolved_patch_path` 让 attack-surface-mapper 读取完整 scope；短分数只作路由摘要。

| topic | field(s) | value source / visibility | downstream use | schema / artifact |
|---|---|---|---|---|
| `redteam.plan.resolved` | `resolved_count`, `unresolved_count` | resolution artifact 的逐计划统计 | 判断是否所有计划 resolved | `field_docs.*`; 完整矩阵落盘 |
| `redteam.plan.resolved` | `resolution_file_path` | 本 activation 写入的 resolution 文件 | mapper 读取 commit 归因 | `field_docs.resolution_file_path`; 必填 artifact |
| `redteam.plan.resolved` | `scope_manifest_path`, `scope_digest` | manifest 文件和 canonical SHA-256 | typed scope gate 验证范围 | `field_docs.*`; manifest 必填并保留 |
| `redteam.plan.resolved` | `scope_status` | scope 分析结论 | 只有 resolved 才能进入攻击面 | `allowed_values` + `field_docs`; 完整依据落盘 |
| `redteam.plan.resolved` | `overall_confidence`, `critical_unknown_count` | resolution 矩阵计算 | scope threshold 决策 | `field_docs.*`; 矩阵落盘 |
| `redteam.plan.resolved` | `scope_base_sha` | Git anchor 或 prompt 明确 SHA | 绑定 patch 范围 | `field_docs.scope_base_sha`; manifest 落盘 |
| `redteam.plan.resolved` | `resolved_patch_path`, `patch_digest` | patch 文件和 SHA-256 | mapper 读取攻击对象 | `field_docs.*`; patch 必填并保留 |
| `redteam.plan.resolved` | `coverage`, `critical_traceability` | hunk/critical claim 矩阵 | 攻击面完整性判断 | `field_docs.*`; 矩阵落盘 |
| `redteam.plan.resolved` | `boundary_consistency`, `boundary_conflict` | scope 与 boundary 交叉检查 | 阻止冲突范围继续 | `field_docs.*`; 检查记录落盘 |

### attack-surface-mapper

- **Q1 使命：** 根据已 resolved scope 设计可执行攻击面和实验队列；完成标准是两个 artifact 完整且每个实验有 control/attack/oracle。
- **Q2 输入：** 读取 resolved payload、resolution、patch 和 scope manifest；不读 runtime ledger。
- **Q3 执行：** Observe scope/artifact 或 invalid 报告 → Precheck 计划修订 → Apply 写攻击面与实验计划 → Confirm policy-check 后发 `redteam.experiment.plan.ready`。
- **Q4 输出：** 只发一个 `redteam.experiment.plan.ready`；payload 只传路径、数量和 `validation_attempt`。
- **Q5 交接：** validator 从路径读取完整计划；runner 只从 `plan.valid` 取得已校验路径，不依赖 payload 长文本。

| topic | field | value source / visibility | downstream use | schema / artifact |
|---|---|---|---|---|
| `redteam.experiment.plan.ready` | `attack_surface_path`, `experiment_plan_path` | mapper 当前写入的两个 artifact | validator 读取完整计划 | `field_docs.*`; 必填 |
| `redteam.experiment.plan.ready` | `experiment_count`, `validation_attempt` | mapper 从计划与 trigger 计算 | validator 判断队列和预算 | `field_docs.*`; 短值可由 artifact 重算 |

### experiment-plan-validator

- **Q1 使命：** 在任何实验执行前，使用目标项目自己的 discovery/help/list 证据验证实验计划可执行；完成标准是写出 validation artifact 并发 valid、invalid 或失败。
- **Q2 输入：** 从 `redteam.experiment.plan.ready` 取得攻击面路径、实验计划路径、数量和尝试次数；读取当前项目能观察到的工具与测试入口。
- **Q3 执行：** 只做通用可执行性检查，不假设 Rust、Cargo 或任何语言；逐 RTE 验证命令、selector、oracle、证据和 cleanup。
- **Q4 输出：** `redteam.experiment.plan.valid`、`redteam.experiment.plan.invalid` 或 `redteam.failed`，完整依据写入 validation artifact。
- **Q5 交接：** valid 把 validation report path 交给 runner；invalid 把失败 RTE 和原因交给 mapper；第三次失败进入 reporter。

| topic | field | value source / visibility | downstream use | schema / artifact |
|---|---|---|---|---|
| `redteam.experiment.plan.valid` | `validation_report_path` | validator 当前写入并 `test -f` 的报告 | runner/reviewer 读取验证依据 | `field_docs.validation_report_path`; 必填 |
| `redteam.experiment.plan.valid` | `validation_status` | 全部 RTE 通用检查结果 | 释放 runner | `allowed_values: valid` |
| `redteam.experiment.plan.invalid` | `failed_experiment_id`, `reason` | validation report 的首个失败项 | mapper 只重写当前 RTE | `field_docs.*`; 完整依据落盘 |
| `redteam.experiment.plan.invalid` | `validation_attempt` | ready trigger 的尝试次数 | bounded retry 路由 | `payload_consistency`; 0–2 才能继续 |

### experiment-runner

- **Q1 使命：** 每次 activation 只执行一个 RTE，完成 control/attack、原始证据、manifest 和 clean-tree 验证。
- **Q2 输入：** 初次从 `redteam.experiment.plan.valid` 读取已校验实验计划与 validation report；续跑从 `redteam.experiment.next.next_experiment_id` 取得精确 RTE ID，并读取 evidence board。
- **Q3 执行：** Observe queue/artifact → Precheck 环境和证据路径 → Apply 隔离执行、写 evidence/manifest/实验文件 → Confirm `test -f`、policy-check、emit。
- **Q4 输出：** 只发 `redteam.experiment.done` 或 producer failure；manifest 先落盘，事件再传路径和 hash。
- **Q5 交接：** evidence-gate 读取 `experiment_file_path`、`evidence_manifest_path` 和 `evidence_paths`；通过或拒绝后由 `experiment.next` 继续队列。

| topic | field | value source / visibility | downstream use | schema / artifact |
|---|---|---|---|---|
| `redteam.experiment.done` | `experiment_id` | 当前 trigger 指定或计划首个未执行 RTE | 定位本次实验 | `field_docs.experiment_id`; 实验文件名和 board 记录 |
| `redteam.experiment.done` | `control_passed` | control 原始输出与断言 | gate 判断 control binary gate | `field_docs.control_passed`; stdout/stderr 落盘 |
| `redteam.experiment.done` | `attack_reproduced` | attack 原始输出与 failure oracle | gate 判断攻击结果 | `field_docs.attack_reproduced`; stdout/stderr 落盘 |
| `redteam.experiment.done` | `evidence_paths` | 实际写入并 `test -f` 的文件 | gate 读取全部 raw evidence | `field_docs.evidence_paths`; evidence 目录必填 |
| `redteam.experiment.done` | `experiment_file_path` | 当前写入的 RTE 文件 | gate 读取实验定义与结果 | `field_docs.experiment_file_path`; 必填 |
| `redteam.experiment.done` | `evidence_manifest_path` | 当前写入的 manifest | gate 验证每项 hash 和 ledger | `field_docs.evidence_manifest_path`; 必填 JSON |
| `redteam.experiment.done` | `ledger_sha256` | manifest 中对真实 ledger/state 文件计算的 hash | gate 复算并拒绝缺证据 | `field_docs.ledger_sha256`; 64 hex 短值，原始命令和目标路径在 manifest |

### evidence-gate

- **Q1 使命：** 只读 raw evidence，逐实验评分，记录 accepted/rejected，按队列决定 next 或 aggregate gated。
- **Q2 输入：** 读取 `redteam.experiment.done` 的 manifest/path/hash；读取实验计划、当前 evidence board 和 retry board。
- **Q3 执行：** Observe 全部证据 → Precheck 独立复核 → Apply 更新 board → Confirm policy-check 后只发 next、aggregate gated 或 failed 之一。
- **Q4 输出：** `experiment.next` 是队列控制面；`evidence.gated` 只在全部实验结算且至少一个合格时发；完整评分与 rejected 原因落盘。
- **Q5 交接：** runner 从 next payload 取得下一个 RTE；impact-boundary 从 aggregate payload 取得 board 和 qualified IDs。

| topic | field(s) | value source / visibility | downstream use | schema / artifact |
|---|---|---|---|---|
| `redteam.experiment.next` | `next_experiment_id` | experiment plan 与 board 的未执行集合 | runner 执行精确下一项 | `field_docs.next_experiment_id`; board 落盘 |
| `redteam.experiment.next` | `completed_count`, `remaining_count` | board 统计 | 判断是否继续 | `field_docs.*`; 完整队列状态落盘 |
| `redteam.experiment.next` | `accepted_count`, `rejected_count` | board accepted/rejected 统计 | reporter 展示进度 | `field_docs.*`; board 落盘 |
| `redteam.experiment.next` | `evidence_board_path` | 当前写入的 board | runner/reviewer 读取 | `field_docs.evidence_board_path`; 必填 |
| `redteam.evidence.gated` | `qualified_experiment_ids` | board 中通过全部门禁的 RTE 列表 | impact-boundary 遍历 Finding 候选 | `field_docs.qualified_experiment_ids`; board 落盘 |
| `redteam.evidence.gated` | `qualified_experiment_count` | qualified ID 数量 | 防止空 qualified 汇总 | `field_docs.qualified_experiment_count`; 短计数可由 board 重算 |
| `redteam.evidence.gated` | `rejected_experiment_count` | board rejected 数量 | 报告被拒候选 | `field_docs.rejected_experiment_count`; 原因在 retry board |
| `redteam.evidence.gated` | `total_experiment_count` | attack plan 与结算 board | 完整性校验 | `field_docs.total_experiment_count`; 计划和 board 均保留 |
| `redteam.evidence.gated` | `minimum_confidence`, `minimum_evidence_coverage` | qualified 评分最小值 | 下游阈值摘要 | `field_docs.*`; 详细评分在 board |
| `redteam.evidence.gated` | `minimum_verifiability`, `minimum_impact_certainty` | qualified 评分最小值 | 下游阈值摘要 | `field_docs.*`; 详细评分在 board |
| `redteam.evidence.gated` | `evidence_board_path` | final board 路径 | impact-boundary/reviewer 读取完整汇总 | `field_docs.evidence_board_path`; 必填 |

### impact-boundary

- **Q1 使命：** 对所有 qualified RTE 重新验证调用者、消费者、配置、生命周期和回归边界，写 Findings、影响报告和 PLAN。
- **Q2 输入：** 从 aggregate payload 取得 board path 与 qualified IDs，逐一读取实验文件和 raw evidence。
- **Q3 执行：** Observe aggregate → Precheck scope and expected modules → Apply 写 findings/impact/PLAN → Confirm policy-check 后发 plan.ready 或 failed。
- **Q4 输出：** `plan_file_path`、`finding_count`、`impact_file_path`；完整 Finding 和边界证据不内联。
- **Q5 交接：** independent-reviewer 读取三类 artifact，reporter 读取 reviewed payload 和 PLAN。

| topic | field | value source / visibility | downstream use | schema / artifact |
|---|---|---|---|---|
| `redteam.plan.ready` | `plan_file_path` | 当前写入并 `test -f` 的 PLAN | reviewer 审查修复计划 | `field_docs.plan_file_path`; 必填 |
| `redteam.plan.ready` | `finding_count` | findings 目录实际计数 | reviewer 校验计划覆盖 | `field_docs.finding_count`; Finding 文件落盘 |
| `redteam.plan.ready` | `impact_file_path` | 当前写入的影响边界文件 | reviewer 复核影响 | `field_docs.impact_file_path`; 必填 |

### independent-reviewer

- **Q1 使命：** 独立审查所有实验、证据门禁、Finding、修复边界和零回归约束；只能发 reviewed 或 failed。
- **Q2 输入：** 读取 plan.ready 的三个路径，并从当前 `.ralph/red-team/` 列出所有 evidence、retry、Finding、PLAN 文件。
- **Q3 执行：** Observe 全部 artifact → Precheck 完整性 → Apply 写独立审查 → Confirm policy-check 后发唯一事件。
- **Q4 输出：** verdict、review path、plan path；详细审查写文件。
- **Q5 交接：** reporter 从 trigger payload 读取 review/plan 路径并读取全文，成功只接受 `PLAN_READY`。

| topic | field | value source / visibility | downstream use | schema / artifact |
|---|---|---|---|---|
| `redteam.reviewed` | `verdict` | 独立审查 checklist 结论 | reporter 决定 success | `field_docs.verdict`; 完整理由在 review 文件 |
| `redteam.reviewed` | `review_file_path` | 当前写入的 review 文件 | reporter 读取审查全文 | `field_docs.review_file_path`; 必填 |
| `redteam.reviewed` | `plan_file_path` | reviewer 验证的 PLAN 路径 | reporter 验证交付 | `field_docs.plan_file_path`; 必填成功计划 |

### reporter

- **Q1 使命：** 将成功或失败事实写成 operator 可读报告并发唯一 completion；失败不得写成 Finding 或成功。
- **Q2 输入：** `redteam.reviewed` 时读取 review/PLAN；`redteam.failed` 时读取 failure artifact、retry board 和已有 evidence board。
- **Q3 执行：** Observe 路径 → Precheck 文件存在、结论和路径语义 → Apply 写 REPORT/QUESTIONS → Confirm 三文件 `test -f`、policy-check、emit complete。
- **Q4 输出：** `success` 必须与 verdict/PLAN 是否存在一致；长正文只在 REPORT/QUESTIONS。
- **Q5 交接：** operator 从 complete payload 取得三份交付路径；运行时只把 complete 当终态，不把 generic success 日志当业务结论。

| topic | field | value source / visibility | downstream use | schema / artifact |
|---|---|---|---|---|
| `redteam.complete` | `success` | reviewed verdict 或 failed trigger | 终态成功语义 | `field_docs.success`; REPORT 结论落盘 |
| `redteam.complete` | `report_path` | 当前写入并验证的 REPORT | operator 阅读主交付 | `field_docs.report_path`; 必填 |
| `redteam.complete` | `plan_path` | reviewed 的 PLAN 或失败时空字符串 | operator 是否可启动修复 | `field_docs.plan_path`; 成功必填，失败必须为空 |
| `redteam.complete` | `questions_path` | 当前写入并验证的 QUESTIONS | operator 决定补证/重跑 | `field_docs.questions_path`; 必填，可为空正文 |

## Payload Consistency 规则摘要

- `redteam.plan.resolved` 保留现有 scope contradiction gates。
- `redteam.experiment.plan.ready` 拒绝超出 3 次的 validation_attempt。
- `redteam.experiment.plan.valid` / `.invalid` 强制 validation_status 与路由方向一致。
- `redteam.experiment.plan.invalid` 拒绝第三次重写后的继续事件。
- `redteam.experiment.next` 拒绝 `remaining_count=0`，避免队列空转。
- `redteam.evidence.gated` 拒绝零 qualified 或空总队列。
- `redteam.complete` 拒绝 `success=true` 但无 PLAN，以及 `success=false` 却携带 PLAN。

## Prompt Visibility 证据

已对 9 个 declared hats 执行：

```bash
ralph -c presets/en/red-team-attack.yml inspect prompt --hat <hat-id> --format json
```

所有 hat 的共同可见性结果为：`ralph-tools`、`ralph-tools-memories`、`ralph-tools-opac` 自动注入；`ralph-tools-emit`、`ralph-tools-precheck`、`ralph-tools-recovery-directives` 按需可加载。Emitter instructions 明确要求加载 `ralph-tools-emit` 并先 `--policy-check`；precheck gate 的规则由 runtime 生成，不要求 producer 读取内部 ledger。

## 交 review 前自检

- [x] `execution_model=single-chain` 与 YAML 一致；无 wave/supervisor。
- [x] 每个实验均有显式 queue continuation；计划无效先回 mapper 重写，失败实验不会阻塞独立实验。
- [x] `ledger_sha256` 从实验定义、模板、schema、runner instructions 到 evidence-gate 均一致。
- [x] 每个完整结果先落盘；event 只传短状态、计数、路径和 hash。
- [x] hard Gate Scope、Key-stage guard、独立 retry budget 均已记录。
- [x] `ralph preset check -H presets/en/red-team-attack.yml --strict` 已通过。
- [ ] `ralph-preset-review` 独立评审：待本轮 author 完成后执行。

## Builtin 同步清单

本次改变 builtin preset 的 hat 拓扑与 YAML/schema/author notes，但不改变 preset 名称或 manifest 入口；需确认 builtin manifest/index、CLI 嵌入和 zsh 补全的 preset 名称仍保持 parity，并执行 preset/schema 结构化测试和全量 nextest 验证。新增 hat 不应新增 Rust 专用命令或项目绑定。
