# ce-executor-pipeline preset author notes

## Preset Intent Confirmation（2026-08-21 dim side-effect precheck）

- **目标：** 允许六个维度 hat 使用可用工具；在维度完成事件进入下一阶段前，由现有 `precheck` 执行 Git 工作树一致性检查。发现非 `.ralph/` 副作用时拒绝本次完成事件，并通过既有 resume/retry 让同一维度清理后重新提交。
- **操作者与启动路径：** 沿用 `ralph run -H builtin:ce-executor-pipeline --plan …`。
- **输入与事实源：** `git status --porcelain=v1 --untracked-files=all`、`git diff --stat HEAD`、`git diff --name-status HEAD`，以及维度事件声明的 findings artifact。
- **成功条件：** 非 `.ralph/` 工作树干净、findings artifact 可读、完成事件原样转发；每个维度最多恢复 3 次。
- **阻塞条件：** 连续 3 次仍有非 `.ralph/` 副作用时，由 precheck 发出 `plan.blocked(reason=precheck_exhausted)`；不再因首次 dirty 直接终止整个循环。
- **允许的修改范围：** 六个维度可运行测试和辅助脚本，但不得把 source、config、lockfile、cache、generated 或其他非 `.ralph/` 副作用带入完成交接；不要求 runtime 代码改动。
- **必须独立执行的评审：** 六个维度完成事件各有一个现有 precheck gate；gate 只检查和转发/拒绝，不替维度修改文件。
- **重要 artifact：** findings artifact 仍由维度写入 `.ralph/`；dirty-path 证据由 gate 从 Git 命令获得。
- **execution_model：** single-chain；本次只增加六个完成事件的 precheck，不引入 wave/supervisor。
- **非目标：** 不新增 payload consistency 谓词；不恢复 `disallowed_tools: ["Edit"]`；不增加 runtime cleanup handler；不让 gate stage、commit 或删除外部 dirty 文件。
- **用户确认：** 已确认选择 1：六个维度统一 `precheck`，retry budget=3，payload consistency 不新增规则。

## Key-stage event gate（2026-08-21 dim side-effect precheck）

| key_stage | guard_selection | precheck_guard | precheck_retry_budget | payload_consistency_guard | payload_consistency_retry_budget | reason | confirmation_status |
|---|---|---:|---:|---:|---:|---|---|
| `review.goalalign.done` | precheck | true | 3 | false | null | `dimension_worktree_dirty` | confirmed |
| `review.correctness.done` | precheck | true | 3 | false | null | `dimension_worktree_dirty` | confirmed |
| `review.testing.done` | precheck | true | 3 | false | null | `dimension_worktree_dirty` | confirmed |
| `review.maintainability.done` | precheck | true | 3 | false | null | `dimension_worktree_dirty` | confirmed |
| `review.standards.done` | precheck | true | 3 | false | null | `dimension_worktree_dirty` | confirmed |
| `review.adversarial.done` | precheck | true | 3 | false | null | `dimension_worktree_dirty` | confirmed |

- **schema 结论：** 本次只增加已有 precheck 配置和作者指令，不增删事件字段、required_fields 或 payload consistency 谓词，因此 `presets/schemas/ce-executor-pipeline.yml` 无需修改。

## Preset Intent Confirmation（2026-08-17 recovery_guidance 增量）

- **目标：** 在现有 `event_loop.precheck` 与 `event_policy.payload_consistency` 上补充作者 `recovery_guidance`，让被拒的 executor / fixer / test-stabilizer 下一轮看到「修证据、不要只改字段」的纸条。不改判定、不改 resume/retry/exhaustion。
- **操作者与启动路径：** 沿用 `ralph run -H builtin:ce-executor-pipeline --plan …`。
- **输入与事实源：** 现有 gate `message` / `reason` / `failed_checks`；guidance 是提示不是事实。
- **成功条件：** strict lint 通过；consistency 命中时 correction/CLI 同源显示 common + 对应 rule id 的 by_check；precheck 拒绝（含 synthetic）显示 common。
- **阻塞条件：** guidance 形状不合法（空 item / 错误 key / 不安全字符）→ strict lint 拒绝启动。
- **允许的修改范围：** 仅 `presets/en/ce-executor-pipeline.yml` 的 precheck/consistency 块 + 本 notes。schema 无新 event 字段。
- **必须独立执行的评审：** 不新增评审 hat；guidance 不替代独立 precheck LLM 或 consistency 谓词。
- **重要 artifact：** 无新业务 artifact；guidance 不落盘。
- **execution_model：** single-chain
  **why：** 无拓扑/并行变更，沿用既有单链。
- **非目标：** 不改 `when`/checklist；precheck 不加 `by_check`（命名 `failed_checks` 与 1-based key 不对齐）；不动 `ce-executor-pipeline-loop`；不新增 `suggested_command`。
- **Author 推导与假设（历史增量）：** 当时用户已确认「consistency = common+by_check；precheck = 仅 common」。2026-08-21 新增的六个维度完成事件另行采用 `precheck`，不新增 consistency 谓词。
- **用户确认：** 已确认（对话内要求 `/ralph-preset-author` 按该方案修改）

## Key-stage event gate（本增量；0e 字段，非 0d mode）

既有 YAML 已挂 guard；本增量不改 `guard_selection` / budget，只加 `recovery_guidance`。`confirmation_status=confirmed`。

| key_stage | guard_selection | precheck_guard | precheck_retry_budget | payload_consistency_guard | payload_consistency_retry_budget | reason | confirmation_status |
|---|---|---|---|---|---|---|---|
| `work.done` executor 成功账单 | both | true | 3 | true | 3 | 结构矛盾用 consistency；证据诚实用 precheck。precheck 仅 common：gate 报命名 failed_checks，by_check 要 1-based index | confirmed |
| `work.failed` executor 死胡同账单 | both | true | 3 | true | 3 | 同上；consistency 仅 `work-failed-with-completed-units` | confirmed |
| `fix.done` fixer 结算 | both | true | 3 | true | 3 | 同上 | confirmed |
| `stabilization.done` test-stabilizer 交接 | precheck | true | 3 | false | null | 该 topic 无 payload_consistency 规则；本增量不新增谓词 | confirmed |

## Recovery guidance contract（本增量）

- precheck 四条：`work.failed` / `work.done` / `fix.done` / `stabilization.done` 仅 `recovery_guidance.common`。
- consistency 十条：每条 `common` + `by_check[<rule.id>]`。
- semantic prose only：无 suggested payload / suggested command。
- 模板文件机制：未采用（guidance 是短 bullet，不进 hat instructions）。

## Change: executor / fixer anti-abdication settlement

目标：不改变 topic 拓扑，只收紧 `executor` 与 `fixer` 的单链执行契约。计划规模、Unit 数量、文件数量、预计上下文压力和预计耗时均不能代替真实执行证据。主 hat 只 dispatch、验收、提交和结账；每个 Unit 的 RED/GREEN/REFACTOR 由唯一 subagent 完成。

验证采用分层策略：每个 Unit 完成后运行 focused tests 与受影响的跨边界/集成测试；全部 Unit 结束后运行一次权威 full-suite。全量新增失败按因果相关失败簇最多委派 3 次 fresh repair subagent，主 hat 不直接编辑修复代码。

## Single-Chain-First

1. **本 preset 的 unit 拆分能否由 executor/fixer 内部 subagent 完成？** ✓。原始 Unit 与 fix Unit 都在各自主 hat 内逐个 dispatch。
2. **任何业务 topic 是否超过一个消费者？** ✓。未改变既有单消费者拓扑。
3. **fallback 是否可能路由到 success？** ✓。失败账单只进入既有 reporter/alignment 路径。
4. **是否有 hat 把 tasks / progress / recovery 当业务事实？** ✓。tasks 关闭；状态来自 trigger、subagent 结果、git 与验证报告。
5. **是否有 rescue hat 能改变业务链路？** ✓。未新增 rescue hat。

## Recovery guidance（precheck common + 高频 consistency）

- precheck 四条规则（`work.done` / `work.failed` / `fix.done` / `stabilization.done`）只声明 `recovery_guidance.common`，与 `on_fail` 同级。不写 `by_check`：gate 上报的是 rubric 名字，而 precheck `by_check` key 只能是 `"1"`/`"2"`…，对不上。
- payload consistency 只给高频真矛盾加动作型 guidance（green+回归、complete+failed/blocked、completed+0 commit、`work.failed` 带 completed）。`message` 仍只诊断；guidance 写打开哪个证据文件、如何对账 git、再重建 payload。其余 consistency 规则继续只靠 `message` + Must re-prove。
- 禁止把 `suggested_command` 或成功 payload 写进 guidance。retry target 仍是既有 `on_fail.target`（executor / fixer / test-stabilizer）。

## Hat: executor

- **Q1 使命:** 逐个 dispatch 原始计划的所有独立 U-ID，验收、独立提交并发出完整执行账单。
- **Q2 输入:** 从 `plan.ready` 读取 `plan_path` 与 baseline SHA；从计划提取 U-ID/Dependencies；从 subagent 返回、git log 与验证报告取得尝试证据。
- **Q3 执行:** Observe → baseline verifier → 每 U-ID dispatch/验收/affected tests/commit → final full-suite → delegated repair（如需）→ settlement → policy-check → emit/confirm。
- **Q4 输出:** 有任一可审计交付 commit → `work.done`（`execution_status: complete|partial` + 完整 Unit 账单 + 诚实 red delta；回归计数 report-only，不强制 work.failed）；仅当零交付 commit / 无法产出验证交接 / 外部不可达 blocker → 结构化 `work.failed`（dead-end 前缀 reason：`unreachable` / `no_deliverable_commits` / `cannot_produce_handoff`）。再进入同一 worktree/loop 时先对账 `decisions.md` 的 `executor checkpoint:` 行与 `git log <DIFF_BASE>..HEAD`，只跑 remaining Units（runtime 通过 `task.resume` 通道从断点继续）。
- **Q5 交接:** reporter 从 `work.failed` 的 Unit 分类与 `reason` 生成 blocked 报告。

### Hat: executor — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性证据 | 身份检查 | 下游消费 | schema metadata |
|---|---|---|---|---|---|---|---|
| `work.failed` | `planned_units` | string[] | 原始计划 Implementation Units | executor 可读 `plan_path` | 不涉及 | reporter 展示完整范围 | `field_docs.planned_units` |
| `work.failed` | `attempted_units` | string[] | decisions dispatch log | subagent 返回与 checkpoint | 不涉及 | 证明不是预测性失败 | `field_docs.attempted_units` |
| `work.failed` | `completed_units` / `failed_units` | string[] | git log、Unit 验收与验证结果 | executor 命令输出 | 不涉及 | reporter 区分完成/真实失败 | 对应 `field_docs` |
| `work.failed` | `blocked_units` | string[] | plan Dependencies + 实际 failed Unit | 原始计划与 Unit 结果 | 不涉及 | reporter 解释阻塞边 | `field_docs.blocked_units` |
| `work.failed` | `decisions_file` / `reason` | string | `.ralph/agent/decisions.md` 与观察到的失败 | executor 写入并可读 | 不涉及 | reporter 核验失败原因 | 对应 `field_docs` |

## Hat: fixer

- **Q1 使命:** 逐个 dispatch 所有 actionable fix Unit；无论 applied、partial 或 blocked，都发出一次诚实的 `fix.done` 尝试报告。
- **Q2 输入:** `review.complete` trigger、`fix_plan_file`、Unit Dependencies、subagent 返回、git 与 baseline/final/delta 报告。
- **Q3 执行:** Observe → baseline verifier → 每 fix Unit dispatch/验收/affected tests/commit → final full-suite → delegated repair（如需）→ settlement → policy-check → emit/confirm。
- **Q4 输出:** `fix.done`，以 `fix_status` 表达成功、部分完成或阻塞。
- **Q5 交接:** alignment 用 Unit 分类、SHA、worktree 与验证字段核对原计划和 fix plan。

### Hat: fixer — Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性证据 | 身份检查 | 下游消费 | schema metadata |
|---|---|---|---|---|---|---|---|
| `fix.done` | `planned_fix_units` / `attempted_fix_units` | string[] | fix plan headings + dispatch log | fixer 可读 fix plan/subagent 返回 | 不涉及 | alignment 检查覆盖率 | 对应 `field_docs` |
| `fix.done` | `completed_fix_units` / `failed_fix_units` | string[] | commit 与 Unit 验收 | git/subagent/验证输出 | 不涉及 | alignment 判断实际落地 | 对应 `field_docs` |
| `fix.done` | `blocked_fix_units` / `skipped_fix_units` | string[] | Dependencies 或 non-actionable 分类 | fix plan 可见字段 | 不涉及 | alignment 记录残留 | 对应 `field_docs` |
| `fix.done` | `fix_status` / `failure_reason` | enum/string | settlement audit | fixer instructions | 不涉及 | alignment 不把 partial 当成功 | 对应 `field_docs` |
| `fix.done` | `decisions_file` | string | fixer execution ledger | fixer 写入并可读 | 不涉及 | alignment/人工核验 | `field_docs.decisions_file` |

## Hat: test-stabilizer (2026-07-16-001 U3)

- **Q1 mission:** post-executor stabilization gate: build a baseline, attribute failures, apply minimal corrections (production-code edits gated by correction IDs), run the project's authoritative full test suite, and emit `stabilization.done` (success) or `stabilization.blocked` (unrecoverable). **No self-approval authority** — the delivered HEAD must still pass the downstream six-dimension review.
- **Q2 input:** only `work.done`; map `executor_head_sha` to `tested_from_sha` and set `review_phase: initial`. The unified plan artifact and trace identity must pass through verbatim. The validation scope is restricted to `work.done.completed_units`; Units still listed under `planned_units` are explicitly out of scope.
- **Q3 execution:** Step 1 read trigger + plan context, derive the completed Units and their already-finished prior dependencies, and build the Scenario/ATDD→test traceability matrix → dispatch at most two read-only scouts in parallel (traceability gaps / risk hypotheses) and let the main hat pick the 1–3 highest-value risks → Step 2 baseline + dirty-worktree gate (`git status` excluding `.ralph/`) → Step 3 capture baseline + run the full suite → when a production defect needs a reproduction test, dispatch at most one test-worker serially (test edits only; submit the pre-fix stable-failure evidence first) → Step 4 failure attribution (the 5 classes) → Step 5 minimal corrections (production edits require commit + correction ID) → Step 6 write `stabilization_audit_file` covering scope, traceability, scouts, risks, worker pre/post-fix evidence, and residual risk → Step 7 emit `stabilization.done` only when traceability evidence is complete and the selected risks are verified or bounded in the audit, or Step 8 emit `stabilization.blocked` when critical evidence is missing, the risks cannot be bounded, or pre-fix failure evidence cannot be established. Scouts / test-workers may not commit, emit, modify production code, read/write the runtime ledger, or declare any Unit closeable. The main Test Hat owns scope adjudication, corrections, policy-check, and the final emit.
- **Q4 output:** `stabilization.done` / `stabilization.blocked` carrying `plan_name` / `plan_path` / `tested_from_sha` / `head_sha` / `stabilization_audit_file` / `correction_ids` / `classification_counts` / `worktree_status` (done also carries `tests_run` / `tests_passed`; blocked also carries `reason`).
- **Q5 handoff:** the six-dim review (trigger now `stabilization.done`) / reporter (trigger now `stabilization.blocked`) must consume the same `head_sha` and `stabilization_audit_file`; production-change `correction_ids` flow into downstream finding association.

### Hat: test-stabilizer — Payload Contract

| topic | field | type | value source | visibility evidence | identity check | downstream consumer | schema metadata |
|---|---|---|---|---|---|---|---|
| `stabilization.done` | `plan_name` / `plan_path` | string | passthrough from `work.done` | `work.done` payload | matches `plan_name` equality | six-dim review | existing schema |
| `stabilization.done` | `tested_from_sha` | string | `git rev-parse HEAD` at start, equals `work.done.executor_head_sha` | git command output | n/a | review baseline | `field_docs.tested_from_sha` |
| `stabilization.done` | `head_sha` | string | `git rev-parse HEAD` before emit, equals `tested_from_sha` (no corrections) or the post-correction commit | git command output | equals the actual commit SHA in the audit | review anchor | `field_docs.head_sha` |
| `stabilization.done` | `stabilization_audit_file` | string | `.ralph/review/<plan>/stabilization/audit.md` (absolute path) | Write tool output | file is readable | review evidence index | `field_docs.stabilization_audit_file` |
| `stabilization.done` | `correction_ids` | string[] | assigned when production code is changed (see `ralph-tools-tasks`) | `decisions.md` + commit message | non-empty iff a production commit exists | review finding linkage | `field_docs.correction_ids` |
| `stabilization.done` | `classification_counts` | object {5 keys} | Step 4 classification rollup | audit file + test command output | `unattributable` must be 0 | review visibility | `field_docs.classification_counts` |
| `stabilization.done` | `worktree_status` | enum | `git status --short` excluding `.ralph/` | git command output | must be `clean` on the done path | review upstream precheck | `field_docs.worktree_status` |
| `stabilization.done` | `tests_run` / `tests_passed` | int | project-authoritative full-suite output | raw test command output | `passed == run` and zero failures | review evidence | schema metadata |
| `stabilization.blocked` | `reason` | enum | Step 8 blocking reason enumeration | `decisions.md` | one of the seven canonical reasons | Reporter blocked report | `field_docs.reason` |
| `stabilization.blocked` | `worktree_status` | enum | `clean` / `dirty` / `unattributable_dirty` | git command output | the blocked path permits `unattributable_dirty` | Reporter report | `field_docs.worktree_status` |

### Test Hat evidence discipline

- Every `completed_units` entry must have a Scenario/ATDD→test traceability row; Units still listed under `planned_units` must be enumerated under exclusions.
- At most two read-only scouts may run in parallel to return candidate evidence; the main hat only adopts the 1–3 risks directly tied to the current Unit, and records the adopt/reject rationale.
- At most one test-worker, never in parallel with build/test; the worker is limited to test edits and focused-test output. The main hat must accept a stable pre-fix failure before any production correction; bounded exceptions with substitute evidence are allowed only when safe automation is impossible.
- The audit must preserve scout returns, worker pre/post-fix test results, rejected overreach/weak-evidence entries, and residual risk. These additions do not extend the `stabilization.*` payload.

## Builtin Sync Checklist（2026-08-17 recovery_guidance 增量）

1. runtime: 无 topic/completion 变更；guidance 走现有 correction 通道。
2. preset_lint: 必须 `ralph preset check -H presets/en/ce-executor-pipeline.yml --strict` 通过（含 recovery_guidance findings）。
3. BDD: 不新增 scenario；判定面未改。回归 `ce_executor_pipeline_fail_gate_*` 与 CLI payload_consistency 结构化测试。
4. config: 仅可选 `recovery_guidance` 字段，serde default。
5. CLI presets: 未增删 preset 名。
6. manifest/index: 不变。
7. docs/zsh: 不改 builtin 列表；hat instructions 未改，无需 inspect prompt。


1. runtime: no change to topic/completion semantics; the `stabilization.done` / `stabilization.blocked` business events are added, with the linear preset supplying the `event_policy.schemas` block. The loop preset already injects schema via `presets/schemas/ce-executor-pipeline-loop.yml`.
2. preset_lint: the linear preset passes strict lint after adding `event_policy.schemas.stabilization.*`; the loop preset already passes.
3. BDD: `ce_executor_pipeline_post_fix_review.yml` uses the real EventLoop runner to prove `fix.done → alignment → reporter`, and asserts that no stabilization/review/fix-plan event is re-emitted.
4. config: no configuration fields were changed.
5. CLI presets: no builtin preset was added or removed.
6. manifest/index: no preset was added or removed.
7. docs/zsh: `CLAUDE.md` / `AGENTS.md` already track the 14-hat / 16-hat topology and `test-stabilizer` description; zsh completion needs no change (preset names are unchanged).

## Reporter artifact-first 收尾契约

- `plan.blocked`、`work.failed`、`stabilization.blocked`、`review.artifact.blocked`、`align.done` 的生产者必须先写入 `ce-report-input.v1` bundle，经 JSON 校验、原子 rename 和重新打开确认后，才把仓库相对路径放入 `report_input_file`。
- bundle 必须包含 plan identity、终态原因或 verdict、各阶段状态、按顺序排列的业务 artifact 引用、验证摘要和 residuals；未执行阶段必须显式写 `not_run`，不得靠 reporter 猜测。
- reporter 只从当前 trigger 取得 `report_input_file`，只读取 bundle 声明的业务 artifact，不以 main event history、task/progress/recovery 状态重建业务事实。
- bundle 缺失、不可读、schema version 不符、plan identity 不匹配或 artifact 缺失时，reporter 生成 blocked 报告，发送 `report.done` 后立即发送 completion promise；不得回发上游 blocked topic。
- `report.done → LOOP_COMPLETE` 是唯一允许的 required-event-to-completion 双事件收尾形状；发送前必须分别通过同源 policy-check，且不得夹带第三个业务事件。
