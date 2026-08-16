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
- **Q3 执行:** Observe → baseline verifier → per-U dispatch/验收/commit → settlement → policy-check → emit/confirm；终态预检必须使用 schema 声明的 `required_target_hat` 与 `recorded=true`。
- **Q4 输出:** `work.done` 或包含 planned/attempted/completed/failed/blocked/skipped 的 `work.failed`；两者都必须按 schema 的目标与终态字段完成 policy-check，缺少目标或 `recorded=true` 时停止，不得把预检结果当作正式终态。
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
- **Q3 执行 (OPAC 命令序列):** Observe trigger/fix plan → Precheck `ralph emit --policy-check fix.done`（包含 schema 要求的目标与 `recorded=true`）→ Apply per-Unit subagent + commit/verify → Confirm emit result；预检拒绝时修正后重试，不能直接发布。
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

- **Q1 mission:** the only-run stabilization gate after the executor. Build a baseline, attribute failures, apply minimal corrections (production edits require correction IDs), run the full test suite, and emit `stabilization.done` or `stabilization.blocked`. **No self-approval authority** — the delivered HEAD must pass downstream review-reentry or the reporter.
- **Q2 input:** `work.done` carries `plan_name` / `plan_path` / `executor_head_sha` / `resolved_baseline_sha` / the Unit bill. Validation scope is restricted to `completed_units`; Units still listed under `planned_units` are explicitly out of scope.
- **Q3 execution:** Step 1 read the trigger + plan context (input fields vary by trigger type), derive the completed Units and their already-finished prior dependencies, and build the Scenario/ATDD→test traceability matrix → dispatch at most two read-only scouts in parallel (traceability gaps / risk hypotheses) and let the main hat pick the 1–3 highest-value risks → Step 2 baseline + dirty-worktree gate → Step 3 capture baseline + run the full suite → when a production defect needs a reproduction test, dispatch at most one test-worker serially (test edits only; submit the pre-fix stable-failure evidence first) → Step 4 failure attribution (5 classes) → Step 5 main hat accepts evidence then applies minimal corrections → Step 6 write `stabilization_audit_file` covering scope, traceability, scouts, risks, worker pre/post-fix evidence, and residual risk → Step 7/8 emit. Scouts / test-workers may not commit, emit, modify production code, read/write the runtime ledger, or declare any Unit closeable. The main Test Hat owns scope adjudication, corrections, policy-check, and the final emit.
- **Q4 output:** `stabilization.done` / `stabilization.blocked`, carrying `head_sha` / `tested_from_sha` / `stabilization_audit_file` / `correction_ids` / `classification_counts` / `worktree_status`.
- **Q5 handoff:** review-reentry must launch the first review round using the same `head_sha`; subsequent fixers run their own full-suite verification and `fix.done` triggers the next round directly.

### Hat: test-stabilizer — Payload Contract

| topic | field | type | value source | visibility evidence | identity check | downstream consumer | schema metadata |
|---|---|---|---|---|---|---|---|
| `stabilization.done` | `plan_name` / `plan_path` | string | passthrough from `work.done` trigger | trigger payload | matches `plan_name` equality | review-reentry | schema SSOT |
| `stabilization.done` | `tested_from_sha` | string | trigger head SHA (`executor_head_sha` or `fix.head_sha`) | git command output | n/a | review baseline | `field_docs.tested_from_sha` |
| `stabilization.done` | `head_sha` | string | `git rev-parse HEAD` before emit | git command output | equals the actual commit SHA in the audit | review-reentry anchor | `field_docs.head_sha` |
| `stabilization.done` | `stabilization_audit_file` | string | `.ralph/review/<plan>/stabilization/audit.md` | Write tool output | file is readable | review evidence index | `field_docs.stabilization_audit_file` |
| `stabilization.done` | `correction_ids` | string[] | assigned when production code is changed | `decisions.md` + commit message | non-empty iff a production commit exists | review finding linkage | `field_docs.correction_ids` |
| `stabilization.done` | `classification_counts` | object {5 keys} | Step 4 classification rollup | audit file + test command output | `unattributable == 0` | review visibility | `field_docs.classification_counts` |
| `stabilization.done` | `worktree_status` | enum | `git status --short` excluding `.ralph/` | git command output | must be `clean` | review-reentry precheck | `field_docs.worktree_status` |
| `stabilization.done` | `tests_run` / `tests_passed` | int | project-authoritative full-suite output | raw test command output | `passed == run` and zero failures | review evidence | schema metadata |
| `stabilization.blocked` | `reason` | enum | one of the seven canonical reasons | `decisions.md` | blocking classification | Reporter blocked report | `field_docs.reason` |

### Test Hat evidence discipline

- Every `completed_units` entry must have a Scenario/ATDD→test traceability row; Units still listed under `planned_units` must be enumerated under exclusions.
- At most two read-only scouts may run in parallel to return candidate evidence; the main hat only adopts the 1–3 risks directly tied to the current Unit, and records the adopt/reject rationale.
- At most one test-worker, never in parallel with build/test; the worker is limited to test edits and focused-test output. The main hat must accept a stable pre-fix failure before any production correction; bounded exceptions with substitute evidence are allowed only when safe automation is impossible.
- The audit must preserve scout returns, worker pre/post-fix test results, rejected overreach/weak-evidence entries, and residual risk. These additions do not extend the `stabilization.*` payload.

## Builtin Sync Checklist (post 2026-07-16-001 U3 + U5)

1. `event_loop/mod.rs`: terminal topic topology unchanged; the `stabilization.done` / `stabilization.blocked` business events are added, with schema injected from `presets/schemas/ce-executor-pipeline-loop.yml`.
2. `preset_lint`: strict lint passes (work.done → test-stabilizer; fix.done → review-reentry; both single-consumer).
3. BDD scenarios: the U3/U5 real-EventLoop scenario is left for U8 governance (no worker fixture yet, manual verification only).
4. config/preflight/config_resolution: no configuration fields were added.
5. CLI presets: no builtin preset was added or removed.
6. manifest/index: no preset was added or removed.
7. docs/zsh: `CLAUDE.md` / `AGENTS.md` already track the 16-hat topology and `test-stabilizer` description; zsh completion needs no change.

## Reporter artifact-first 收尾契约

- `plan.blocked`、`work.failed`、`stabilization.blocked`、`review.artifact.blocked`、`review.loop.blocked`、`align.done` 的生产者必须先写入 `ce-report-input.v1` bundle，经 JSON 校验、原子 rename 和重新打开确认后，才把仓库相对路径放入 `report_input_file`。
- bundle 必须保留 `review_round` 等 loop 身份、终态原因或 verdict、各阶段状态、按顺序排列的业务 artifact 引用、验证摘要和 residuals；未执行阶段必须显式写 `not_run`。
- reporter 只从当前 trigger 取得 `report_input_file`，只读取 bundle 声明的业务 artifact，不以 main event history、task/progress/recovery 状态重建业务事实。
- bundle 校验失败时，reporter 生成 blocked 报告，发送 `report.done` 后立即发送 completion promise；不得把失败重新路由到 review/fix/alignment。
- `report.done → LOOP_COMPLETE` 是唯一允许的 required-event-to-completion 双事件收尾形状；发送前必须分别通过同源 policy-check，且不得夹带第三个业务事件。
