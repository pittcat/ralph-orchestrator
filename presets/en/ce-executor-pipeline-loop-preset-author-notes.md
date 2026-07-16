# ce-executor-pipeline-loop preset author notes

## Change: executor / fixer anti-abdication settlement

目标：不新增 topic、不新增消费者。`fixer` 仍然只发布 `fix.done`，`review-reentry` 仍然是唯一消费者。`fix.done` 表示 fixer 完成本轮尝试报告，不再等同于“全部修复成功”。成功、部分完成、阻塞分别由 `fix_status` 表达，并由后续 review round 与既有 `review.loop.blocked` / `reporter` 链路收口。每一轮 fixer 只写自己的 `round-<NN>/baseline-verification.md`、`round-<NN>/final-verification.md`、`round-<NN>/verification-delta.md`，顶层只保留 executor 阶段总验证。

验证采用分层策略：每 Unit 跑 focused + affected integration，全部 Unit 后跑权威 full-suite；全量新增失败最多委派 3 次按失败簇隔离的 repair subagent，主 executor/fixer 不直接编辑修复代码。

## Single-Chain-First

1. **本 preset 的 unit 拆分能否由 executor/fixer 内部 subagent 完成？** ✓。fix Unit 仍由 fixer 内部 subagent 执行。
2. **任何业务 topic 是否超过一个消费者？** ✓。`fix.done` 仍只由 `review-reentry` 消费。
3. **fallback 是否可能路由到 success？** ✓。`fix_status` 只是报告字段，不直接跳 success。
4. **是否有 hat 把 tasks / progress / recovery 当业务事实？** ✓。本变更只读 trigger payload 与 git/test 结果。
5. **是否有 rescue hat 能改变业务链路？** ✓。没有新增 rescue hat。

## Hat: executor

- **Q1 使命:** 逐个 dispatch 原始计划的所有独立 U-ID；不得以规模或预计上下文压力替代执行。
- **Q2 输入:** `plan.ready`、原始计划 U-ID/Dependencies、subagent 返回、git 与验证报告。
- **Q3 执行:** Observe → baseline verifier → per-U dispatch/验收/commit → settlement → policy-check → emit/confirm。
- **Q4 输出:** `work.done` 或包含 planned/attempted/completed/failed/blocked/skipped 的 `work.failed`。
- **Q5 交接:** reporter 用结构化 Unit 账单生成 blocked 终态。

### Hat: executor — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性证据 | 身份检查 | 下游消费 | schema metadata |
|---|---|---|---|---|---|---|---|
| `work.failed` | Unit settlement arrays | string[] | plan、dispatch log、subagent 结果、git | executor instructions 与可见命令 | 不涉及 | reporter 解释真实执行状态 | 对应 `field_docs` |
| `work.failed` | `baseline_verification_file` | string | baseline-verifier 产物 | executor 可见文件 | 不涉及 | reporter 提供验证证据 | 对应 `field_docs` |
| `work.failed` | `decisions_file` / `reason` | string | decisions ledger 与观察到的失败 | executor 写入并可读 | 不涉及 | reporter 核验归因 | 对应 `field_docs` |

## Hat: fixer

- **Q1 使命:** 执行 `fix_plan_file` 中的 actionable Units，并无论成功、部分完成或阻塞，都发出一次 `fix.done` 尝试报告。
- **Q2 输入 (Observe 命令 + 期望字段):** trigger `review.complete` 提供 `plan_name`、`plan_path`、`review_round`、`fix_base_sha`、`fix_plan_file`、`verdict`；读取 `fix_plan_file`；用 `git status --short` 确认最终 clean；用 `git rev-parse HEAD` 得到 `fix_attempt_commit_sha`；baseline / final / delta 验证结果分别读取本轮 `round-<NN>/baseline-verification.md`、`round-<NN>/final-verification.md`、`round-<NN>/verification-delta.md`。
- **Q3 执行 (OPAC 命令序列):** Observe trigger/fix plan → Precheck `ralph emit --policy-check fix.done` → Apply per-Unit subagent + commit/verify → Confirm emit result。
- **Q4 输出 (topic + payload 合同):** 见下方 Payload Contract。
- **Q5 交接 (emit 字段 → 下游 Observe 路径):** `review-reentry` 从 `fix.done.next_review_plan` 与 status 字段构造下一轮 `review.round.ready`；后续 review/gate 判断是否继续修复或走 `review.loop.blocked`。
- **额外边界:** 如果发现密钥或其他敏感信息已经进入 git 历史，不把“重写历史”当成默认 loop 动作。先做本轮可见的本地修复和提交，再把需要旋转密钥、清理历史或通知仓库维护者的后续动作写进 `failure_reason` / `next_review_plan`。

### Hat: fixer — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性证据 | 身份检查 | 下游消费 | schema metadata |
|---|---|---|---|---|---|---|---|
| `fix.done` | `fix_status` | enum string | fixer 对本轮 Unit/验证结果的判定 | fixer instructions 的 Step 6 与 Failure Handling | 不涉及 | `review-reentry` 放入 `review_plan`，review hats 检查 partial/blocked attempt | `field_docs.fix_status` |
| `fix.done` | `failure_reason` | string | verification 失败、未完成 Unit、或 blocker 记录 | fixer 可见的 subagent 结果、验证输出、`.ralph/agent/decisions.md` | 不涉及 | 下一轮 review 和最终 reporter 解释未收敛原因 | `field_docs.failure_reason` |
| `fix.done` | `failed_fix_units` | string array | fixer per-Unit execution log | fix plan Unit headings + subagent result | 不涉及 | 下一轮 review 聚焦未完成 Unit | `field_docs.failed_fix_units` |
| `fix.done` | `attempted_fix_units` / `blocked_fix_units` | string array | dispatch log / fix plan Dependencies | fixer 可见 subagent 结果与 fix plan | 不涉及 | 下一轮 review 区分真实失败与依赖阻塞 | 对应 `field_docs` |
| `fix.done` | `decisions_file` | string | `.ralph/agent/decisions.md` | fixer 写入并可读 | 不涉及 | 下一轮 review/人工核验 | `field_docs.decisions_file` |
| `fix.done` | `fix_attempt_commit_sha` | string | `git rev-parse HEAD` after final fixer commit | fixer 可运行 git rev-parse | 不涉及 | 下一轮 review 用该提交作为 attempt 证据 | `field_docs.fix_attempt_commit_sha` |
| `fix.done` | `worktree_status` | string | `git status --short` 为空 | fixer 可运行 git status | 不涉及 | 保证下一个 hat 不接脏工作区 | `field_docs.worktree_status` |
| `fix.done` | `next_review_plan` | object | fixer 根据本轮 attempt 构造 | fixer instructions Step 6 | 不涉及 | `review-reentry` 的 review plan SSOT | 既有 `required_fields` + status 字段补充 |

## Hat: review-reentry

- **Q1 使命:** 将首次 `stabilization.done` 或后续 `fix.done` 规范化为下一轮 `review.round.ready`。
- **Q2 输入 (Observe 命令 + 期望字段):** trigger `stabilization.done` 提供 `head_sha`、`tested_from_sha`、`stabilization_audit_file`、`correction_ids`、`classification_counts`、`worktree_status`、`tests_run`/`tests_passed`、`resolved_baseline_sha`。
- **Q3 执行 (OPAC 命令序列):** Observe trigger → generate review diff artifacts (anchor = `head_sha` 优先, 回退 `executor_head_sha`) → Precheck `review.round.ready` → emit。
- **Q4 输出 (topic + payload 合同):** `review.round.ready`，`review_round: 1`（stabilization.done 路径），`source_topic: stabilization.done`，`round_base_sha = head_sha`，`diff_ranges = [<resolved_baseline_sha>..<head_sha>]`。
- **Q5 交接 (emit 字段 → 下游 Observe 路径):** dimension review hats 从 `review_plan` / diff patch / `correction_ids` 识别 Test Hat 修正后的 HEAD；review-synthesizer 汇总后由 review-gate 决定继续修或阻塞。

### Hat: review-reentry — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性证据 | 身份检查 | 下游消费 | schema metadata |
|---|---|---|---|---|---|---|---|
| `review.round.ready` | `review_plan.fix_status` | string | 仅 `fix.done` trigger 携带；stabilization.done 路径不存在 | trigger payload | 不涉及 | reviewers 判断本轮是否检查未完成 attempt | `fix.done.field_docs.fix_status` |
| `review.round.ready` | `review_plan.residual_risks` | string array | 仅 fix.done 路径 | trigger payload | 不涉及 | review-synthesizer 识别未收敛原因 | `fix.done.field_docs.failure_reason` |
| `review.round.ready` | `round_base_sha` | string | stabilization.done 路径 = `head_sha`；fix.done 路径 = `head_sha`（已自带） | trigger payload | 不涉及 | reviewers 设定 diff 起点 | 既有 schema |
| `review.round.ready` | `source_topic` | string | `stabilization.done`（首次）/ `fix.done`（修复轮） | trigger topic | 不涉及 | reporter 区分首轮 / 修复轮 | 既有 schema |

## Hat: test-stabilizer (2026-07-16-001 U3)

- **Q1 使命:** executor 后仅运行一次的稳定化门禁。建基线、归类失败、最小修正（含生产代码 + correction ID）、跑全量测试，发 `stabilization.done` 或 `stabilization.blocked`。**无自批权**——交付 HEAD 必须经下游 review-reentry 或 Reporter。
- **Q2 输入:** `work.done` 携带 `plan_name` / `plan_path` / `executor_head_sha` / `resolved_baseline_sha` / Unit 账单。
- **Q3 执行:** Step 1 读触发与计划上下文（按 trigger 类型区分输入字段）→ Step 2 baseline + dirty-worktree gate → Step 3 捕获 baseline + 跑全量测试 → Step 4 失败归类（5 类）→ Step 5 最小修正 → Step 6 写 `stabilization_audit_file` → Step 7/8 emit。
- **Q4 输出:** `stabilization.done` / `stabilization.blocked`，含 `head_sha` / `tested_from_sha` / `stabilization_audit_file` / `correction_ids` / `classification_counts` / `worktree_status`。
- **Q5 交接:** review-reentry 必须使用同一 `head_sha` 启动首轮 review；后续 fixer 自己完成全量测试并由 `fix.done` 直接触发下一轮 review。

### Hat: test-stabilizer — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性证据 | 身份检查 | 下游消费 | schema metadata |
|---|---|---|---|---|---|---|---|
| `stabilization.done` | `plan_name` / `plan_path` | string | `work.done` trigger 透传 | trigger payload | 与 plan_name equality 一致 | review-reentry | schema SSOT |
| `stabilization.done` | `tested_from_sha` | string | trigger head SHA（executor_head_sha 或 fix.head_sha） | git 命令输出 | 不涉及 | 复审基线 | `field_docs.tested_from_sha` |
| `stabilization.done` | `head_sha` | string | emit 前 `git rev-parse HEAD` | git 命令输出 | 等于 audit 中实际 commit SHA | review-reentry anchor | `field_docs.head_sha` |
| `stabilization.done` | `stabilization_audit_file` | string | `.ralph/review/<plan>/stabilization/audit.md` | Write 工具输出 | 文件可读 | 复审证据索引 | `field_docs.stabilization_audit_file` |
| `stabilization.done` | `correction_ids` | string[] | 生产代码改动时分配 | decisions.md + commit message | 非空 ⟺ 存在生产 commit | 复审 finding 关联 | `field_docs.correction_ids` |
| `stabilization.done` | `classification_counts` | object{5 keys} | Step 4 分类汇总 | audit 文件 + 测试命令输出 | `unattributable == 0` | 复审可见 | `field_docs.classification_counts` |
| `stabilization.done` | `worktree_status` | enum | `git status --short` 排除 `.ralph/` | git 命令输出 | 必须是 `clean` | review-reentry 前置门禁 | `field_docs.worktree_status` |
| `stabilization.done` | `tests_run` / `tests_passed` | int | 项目权威 full-suite 输出 | 测试命令原始输出 | passed == run 且无失败 | 复审证据 | schema metadata |
| `stabilization.blocked` | `reason` | enum | 7 canonical reasons 之一 | decisions.md | 阻塞归类 | Reporter 阻塞报告 | `field_docs.reason` |

## Builtin Sync Checklist (2026-07-16-001 U3 + U5 后)

1. `event_loop/mod.rs`: 未改 topic 拓扑终态；新增 `stabilization.done`/`stabilization.blocked` 业务事件，schema 由 `presets/schemas/ce-executor-pipeline-loop.yml` 注入。
2. `preset_lint`: 通过 strict lint（work.done → test-stabilizer；fix.done → review-reentry，均为单消费者）。
3. BDD scenarios: U3/U5 真实 EventLoop scenario 暂留 U8 治理补完（worker 没有补 fixture,仅手动验证）。
4. config/preflight/config_resolution: 未新增 config 字段。
5. CLI presets: 未新增/删除 builtin preset。
6. manifest/index: 未新增/删除 preset。
7. docs/zsh: `CLAUDE.md`/`AGENTS.md` 已同步 16-hat 与 test-stabilizer 描述；zsh 补全无需改。
