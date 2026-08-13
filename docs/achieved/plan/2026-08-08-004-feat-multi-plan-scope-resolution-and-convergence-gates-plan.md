---
title: "feat: 为多计划合并后的 preset 建立独立增量 scope 解析与置信度门禁"
type: feat
date: 2026-08-08
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
target_repository: ralph-orchestrator
baseline_commit: 818810763bb895b860ec1f78e5ab5b30469ab033
---

# 为多计划合并后的 preset 建立独立增量 scope 解析与置信度门禁

## Goal Capsule

- **Objective：** 让 `merge-batch`、`post-merge-converge` 和 `red-team-attack` 都能独立确定多份开发计划在最终目标分支中的实现范围、增量 diff、穿插提交、覆盖/回滚和未知改动，并在范围不能被证据证明时停止或降级为阻断报告。
- **范围 authority：** 当前最终 Git 树、开发计划文件内容、Git commit graph、commit patch、hunk 归属和结构化 scope manifest 共同构成证据；`.ralph/merge/merge-boundary.json` 只能作为可选交叉证据，不能替代 `post-merge-converge` 或 `red-team-attack` 的独立解析。
- **执行 profile：** 严格执行 Unit 1 → Unit 2 → Unit 3 → Unit 4 → Unit 5 → Unit 6 → Unit 7。每个 Unit 必须完成 Acceptance Red、最小实现、单元/集成测试、回归和证据更新后才能进入下一个 Unit。
- **停止条件：** 发现 scope base 无法确定、计划候选提交存在未解释并列候选、关键 hunk 归属未知、目标 HEAD/tree 在解析期间变化、`merge-boundary` 与独立结果冲突、或任何关键决策置信度低于 0.85 时，立即停止当前 Unit，更新 Evidence/Decision 并重做决策。
- **Tail ownership：** 本计划保留 agent-owned 的 Git scope 解析，同时为三个 preset 增加结构化 manifest、固定指标阈值和现有 emit precheck/payload-consistency guard；Coding Agent 不得把范围判断留到实现时临时设计，也不得修改生产代码来绕过 scope 阻断。

---

## Product Contract

### 0. 计划状态

- **状态：** READY。P0/P1 对抗性问题已修正；所有进入实施的关键技术决策置信度均不低于 0.85；没有未处理的阻塞决策。
- **基线：** 分支 `pittcat-dev`，HEAD `818810763bb895b860ec1f78e5ab5b30469ab033`，tree `754f03216929a8dc97663fbecec4cdda53ddb895`。该 HEAD 只归档既有计划文件；代码证据仍以同一工作树内容为准。
- **调查范围：** 三个 builtin preset 及其 schema、operator prompt、author notes、`implementation-review` 的既有 scope-manifest 协议、Git 操作封装、plan baseline、真实 EventLoop BDD harness、preset strict lint、preset manifest/嵌入同步、preset author/review skills、当前 merge report 和 first-parent Git 历史。
- **已执行的验证：** `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 通过；`cargo nextest run -p ralph-core -- preset_lint` 通过；`cargo nextest run -p ralph-cli --bin ralph -- presets` 通过；`./scripts/check-cli-doc-drift.sh` 通过；当前 Git evidence command smoke 通过；当前 first-parent history 同时包含 merge commit 和目标分支直接 commit；本轮对抗性审查已完成。
- **尚未执行的验证：** 本计划中的 resolver Red/Green、emit provenance gate、BDD 新 fixture、scope fixture、preset author/review skill 更新、完整构建、clippy 和最终 `./scripts/run-tests.sh` 均留给执行阶段；本轮只修改计划文件，没有修改生产代码。
- **阻塞项：** 无。

#### 0.1 对抗性审查修复记录

| 严重度 | 发现 | 已采取的计划修复 |
| --- | --- | --- |
| P0 | scope 归属主要依赖 agent prompt，agent 可以提交字段正确但 payload 与 artifact 不一致的 manifest，且 `--unsafe-no-policy-check` 可能成为绕过入口 | D2/D12/Unit 1 固定 agent-owned manifest、现有 emit precheck、`payload_consistency` 规则、artifact/path/digest/HEAD-tree/threshold guard；scope topic 不受 unsafe bypass 影响 |
| P0 | 增量 diff 下界未定义，redteam 的 `<global-baseline>` 不能执行；merge、direct-target 和 target 穿插历史会被错误合并 | D5 固定 locked HEAD/tree、显式 base ancestor 校验、direct/merge anchor 识别和 root/并列/非 ancestor fail-close；Unit 3/5 必须覆盖无 boundary direct-target |
| P1 | commit/hunk、覆盖、revert、rename、binary 和 critical unknown 的分类仍有实现选择空间 | D6 固定 candidate scoring、rev-list、hunk key、classification、`critical_paths` 和 binary `not_applicable` 语义；H2 只验证声明能力，不允许临时换算法 |
| P1 | confidence 的 no-boundary 语义、boundary 冲突和 85/89/90 阈值可能被平均分稀释 | D7 固定维度权重、可用分数、`cross_check=not_applicable`、关键维度下限和 critical unknown=0；Unit 4/6 增加边界值与冲突测试 |
| P1 | resolver 可读无限 history/patch 或写越界路径，缺少可执行资源门禁 | D13 固定 plan/commit/patch/manifest/hunk 上限、repo-relative allowed root、固定 Git argv、无 shell、原子写入和稳定 `resource_limit` |
| P1 | 新 scope contract 与 agent-facing guide、author/review audit 之间可能漂移 | D11/Unit 7 同步 scope contract、emit/cmdref、negative fixtures 和 anchor/parity/doc-drift 回归 |

### 1. 功能目标

#### 1.1 业务目标

多个开发计划落入同一个目标分支后，系统必须回答“哪些最终改动属于这些计划、哪些是中间穿插的目标分支提交、哪些被后续提交覆盖、哪些仍然无法归属”，而不是把 `scope_base..HEAD` 的全部内容错误地当作计划增量。

#### 1.2 用户与调用方

- **Operator：** 向三个 preset 提供目标分支、已完成计划路径或计划 ID，以及可选验证命令。
- **`merge-batch`：** 在实际批量 merge 完成后写出本轮 merge boundary，供后续审查参考。
- **`post-merge-converge`：** 在当前最终树上做系统收敛，只审查已确定的计划范围和最终系统状态，不重新执行开发计划。
- **`red-team-attack`：** 在只读代码树上独立反向定位计划实现，重建可复核 patch，并把攻击实验绑定到确定的 scope。
- **Preset author/reviewer：** 审查三套 preset 是否拥有独立解析、阻断、置信度、artifact 和交接契约。

#### 1.3 当前行为

- `post-merge-converge` 的 baseline 只记录当前 branch/HEAD/worktree/验证结果，change-mapper 主要依据显式计划列表或 Git chronology 给出 `plan_match_confidence: high|medium|low`。
- 当前 `plan_match_confidence: high` 可以由“prompt 列出计划且路径都能解析”触发，不能证明每个 commit 或 hunk 的归属，也没有独立的 `scope_base`、interleaved diff、unknown diff 或 override 结果。
- `red-team-attack` 已从 Git history 搜索计划、commit、路径、symbol、patch-id 和 blame，但 combined patch 使用未定义的 `<global-baseline>..<locked-head>`，因此无法证明 patch 下界；它也没有强制输出多计划共享/穿插/覆盖/未知 hunk 的统一 manifest。
- `merge-batch` 记录实际 merge commit SHA 和 target base/head，但 `merge.integrated` schema 没有 boundary artifact path 或 digest，后续 preset 无法消费可验证的结构化 merge 证据。

#### 1.4 目标行为

- 三个 preset 都生成自己的 byte-stable scope manifest，记录 target branch、locked HEAD/tree、scope base、base 来源、计划 digest、计划候选提交、最终 commit/hunk 分类、完整 diff、计划 scoped diff、interleaved diff、override/revert diff、unknown diff、置信度和最终 decision。
- `post-merge-converge` 和 `red-team-attack` 每次运行都独立从计划和 Git history 重建 scope；存在 `merge-boundary` 时只做独立复算和交叉验证，不能直接信任它的 base、commit 集合或 confidence。
- 支持三种历史：纯 branch merge、计划直接提交到 target branch、merge 与 target 直接提交混合。target branch 中穿插的非计划提交必须单独分类，不得静默包含到计划 scoped diff。
- 后续 commit 修改计划引入的同一 hunk 时，manifest 必须标记 `overridden_later` 并保留“原计划 commit → 后续覆盖 commit → 当前最终 hunk”的链路；revert 必须标记 `reverted`，不能继续声称该改动属于当前有效实现。
- scope base、plan identity、commit attribution、hunk attribution、interleaved detection、override detection 和 `cross_check` 分开评分；无 boundary 时 `cross_check=not_applicable`，有 boundary 时才比较两份结果。关键未知、冲突或低置信度必须进入 `ambiguous`/`blocked`，不能进入 PASS、VERIFIED 或攻击实验。
- `post-merge-converge` 只有在 scope decision 为 `resolved` 且关键 unknown 为零时才允许进入六维审计；否则生成完整阻断 artifact 并进入既有报告链。
- `red-team-attack` 只有在 scope decision 为 `resolved` 且 patch attribution coverage 与 critical traceability 达标时才允许进入 attack-surface；否则沿既有 `redteam.plan.unresolved` 失败路径结束。

#### 1.5 行为差异

| 能力 | 当前行为 | 目标行为 |
| --- | --- | --- |
| 增量下界 | baseline HEAD 或未定义 placeholder | manifest 中已验证的 `scope_base_sha` 与来源 |
| 计划识别 | 路径/标题匹配即可提高 confidence | 计划文件 digest、候选提交、多信号证据和并列候选判定 |
| target 穿插提交 | 可能被全量 `base..HEAD` 混入 | 单独标记 `interleaved`，生成独立 diff |
| 后续覆盖 | 没有统一结果 | `overridden_later`，保留原始与最终归属链 |
| 共享 hunk | 没有统一结果 | `shared_by_multiple_plans`，降低 hunk/overall confidence |
| 未知 hunk | 可能随全量 diff 一起进入审查 | `unknown`；关键 unknown 直接阻断 |
| merge-batch 证据 | 只有 Markdown report 和 merge SHA | 追加独立可读 boundary manifest 与 digest |
| red-team patch | 使用 `<global-baseline>` placeholder | 只能使用 manifest 中已验证的 base，无法解析则阻断 |

#### 1.6 本次范围

- 修改 `merge-batch` 的 integrator/reporter artifact 与 `merge.integrated`/`merge.stabilized` payload contract。
- 修改 `post-merge-converge` 的 prompt 输入、change-mapper、auditor/closer/reporter gate 和 schema contract。
- 修改 `red-team-attack` 的 prompt 输入、plan-resolver、attack-surface gate、independent-reviewer/reporter gate 和 schema contract。
- 采用 `implementation-review` 已验证的 scope-manifest/digest/ambiguous→blocked 证据风格，但扩展为多计划、混合历史和 hunk attribution。
- 增加真实 EventLoop BDD 场景，验证新 payload schema、resolved/ambiguous/blocked 路由和禁止越过 scope gate 的行为。
- 更新三套 preset author notes、preset author/review references、review fixtures、预设指南和 builtin 描述。

#### 1.7 非目标

- 不新增 builtin preset。
- 不让 `post-merge-converge` 或 `red-team-attack` 依赖某次 `merge-batch` 运行、`.ralph/merge/merge-boundary.json` 是否存在，或 merge-batch 的 event history。
- 不新增 `ralph inspect scope resolve` 或任何新的 CLI surface，不引入 `git2`/`gix` 等依赖；`change-mapper` 和 `plan-resolver` 保留各自的 agent-owned Git 解析，但必须遵守同一 manifest、指标、阈值和 evidence contract。
- 不重新 merge、rebase、cherry-pick 或重跑开发计划。
- 不改变既有六维审计、reproducer/fixer、自带攻击实验、清洁环境、reporter terminal topic 的业务目的。
- 不把所有目标分支提交自动归入“范围外”；无法证明与计划无关的改动必须保留为 unknown 或 interleaved residual。
- 不允许 fixer 以 scope 不确定为理由修改 production code。

#### 1.8 输入、输出和状态变化

- **输入：** operator prompt 中的 target branch、计划路径/ID、可选 `scope_base`、可选 `merge_boundary_path`、target commit，以及当前 Git branch/HEAD/tree/status/history/plan bytes。
- **输出：** 每个 preset 自己的 scope manifest、scope analysis/block artifact、full/scoped/interleaved/override/unknown patch artifact、scope digest；随后把 manifest path/digest/status/confidence 作为 event payload 的短字段传递。
- **状态变化：** `merge.integrated` 增加 boundary evidence；`postmerge.changemap.ready` 携带 scope decision；`redteam.plan.resolved` 只在 scope gate 通过时产生；失败路径使用既有 `postmerge.complete(success:false)` 或 `redteam.plan.unresolved`。
- **副作用：** 只允许写对应 `.ralph/merge/`、`.ralph/post-merge/` 或 `.ralph/red-team/` artifact；red-team 继续保持生产代码、正式测试、tracked config 和 Git history 只读。
- **不变量：** locked HEAD/tree 在每个解析和 emit 前后不变；manifest digest 与 patch digest 可重新计算；event payload 只传短字段和 path；完整证据必须先落盘；一次 activation 仍只发一条业务 event。

#### 1.9 错误语义

- 计划文件不存在或 digest 无法读取：`plan_file_not_found`/`plan_file_unreadable`，阻断。
- 没有候选提交：`no_candidate`，阻断该计划；所有计划都失败时走既有 unresolved terminal path。
- 多个同强度候选、merge parent 无法确定、root commit、目标 HEAD/tree 变化：`ambiguous_candidates`、`merge_parent_ambiguous`、`root_commit`、`scope_drift`，阻断。
- `scope_base` 无法被证明为所有计划候选范围的共同下界：`scope_base_unresolved`，不得以当前 HEAD 或默认 branch 起点替代。
- hunk attribution 不完整、关键 hunk unknown、merge-boundary 与独立结果不一致：`unknown_hunk`/`boundary_conflict`，scope decision 为 `ambiguous` 或 `blocked`，不进入审计/攻击。
- 置信度 85–89：只允许作为非关键 residual 被报告，不能作为 resolved gate；置信度低于 85 或关键维度缺失：blocked。

#### 1.10 兼容、性能、安全和约束

- 旧 prompt 省略计划列表时仍可运行，但必须进入自动发现路径；旧 prompt 省略 merge boundary 时不得失败。
- 旧 `merge.integrated`/`postmerge.changemap.ready`/`redteam.plan.resolved` 事件 fixture 需要更新为新 required fields；不保留不安全的“旧 payload 继续成功”兼容路径。
- 解析以当前仓库已使用的 Git CLI 为边界，不新增依赖；Git 解析由各自 scope owner agent 执行；单次解析允许读取完整 Git history，但 manifest、patch 和 event payload 必须 bounded，不能把完整 history 搬入 payload。
- scope manifest 不能只靠 event schema 的字符串字段证明正确性；scope 相关 topic 在现有 `ralph emit --policy-check` 和真实 emit 路径都必须经过 payload/artifact consistency、digest、HEAD/tree、状态和阈值校验，`--unsafe-no-policy-check` 不能绕过该 scope gate。该 guard 证明交接内容自洽和未漂移，不宣称替 agent 完成 hunk attribution。
- 三个 preset 的 scope artifact 只能写 `.ralph/` 对应目录；不得把计划内容、完整 patch 或事件 history 写进 event payload。
- `red-team-attack` 仍必须在每项实验前后验证 tracked tree 未变化；scope 解析不能执行 merge/rebase/cherry-pick/reset 或任何生产修改。

#### 1.11 已确认假设

- `implementation-review` 的 scope-preparer 已证明仓库接受“agent 执行 Git 命令 → 写 byte-stable manifest → 计算 digest → 事件只传 digest/path → 下游复核”的模式。
- 现有三个 preset 已经是 agent-driven、artifact-first 工作流；scope owner 仍由 agent 执行 Git 判断，但 correctness 不再只由 prompt 文案担保，而由 manifest contract、payload consistency、emit precheck 和 independent preset cross-check 共同约束。
- 现有 BDD harness 的 `run_workflow_guard_scenario` 会把 mock event 送入真实 EventLoop，并断言 accepted/absent/workflow events；`run_scenario` stub 不足以证明 scope gate 路由。
- `presets/schemas/*.yml` 是三个 preset payload schema 的 authoring SSOT；`build.rs` 会在编译时 deep-merge，不应在三个 preset YAML 内另造同名 schema。

#### 1.12 假设状态与验证动作

- **H1（已验证事实）：** 当前 agent 的 Git 版本支持计划所需的 `git show --format=fuller`、`git diff --binary --full-index`、`git blame --line-porcelain`、`git patch-id --stable` 和 NUL status。Evidence E19 记录了每条命令在当前仓库的最小 smoke 通过结果；Unit 3/4/5/6 仍需在临时 fixture 中验证 agent 输出是否满足固定 manifest contract，不得把命令可用性等同于 scope correctness。
- **H2（待验证的 agent protocol 边界，不是待决策的 CLI/算法）：** 进入 Unit 3/4/5/6 前必须用包含 rename、binary、revert 和同文件交错提交的临时 Git fixture 复现 D6，并验证 agent artifact 能被三个 preset 的固定 manifest/threshold contract 表达。binary 永远只达到 file-level attribution，line-level 固定为 `not_applicable`；若文本/rename fixture 无法稳定表达，则记录为 `unknown` 并走 blocked/residual，不允许 Executor 临时放宽阈值。该验证只决定 agent protocol 的可表达能力，不改变 D6/D7 的既定语义。

---

## Planning Contract

### 2. 代码库现状与证据

#### 2.1 当前实现入口与调用链

```text
merge.start
  → merge-batch/reviewer
  → merge.reviewed
  → merge-batch/integrator
  → merge.integrated
  → merge-batch/stabilizer (merge.retest self-loop)
  → merge.stabilized
  → merge-batch/reporter
  → merge.batch.complete

postmerge.start
  → baseline
  → postmerge.baseline.ready
  → change-mapper
  → postmerge.changemap.ready
  → system-auditor (六份审计 artifact)
  → test-gap
  → reproducer self-loop
  → fixer self-loop
  → closer
  → reporter
  → postmerge.complete

redteam.start
  → target-locker
  → redteam.target.locked
  → plan-resolver
  → redteam.plan.resolved / redteam.plan.unresolved
  → attack-surface-mapper
  → experiment-runner
  → evidence-gate retry self-loop
  → impact-boundary
  → independent-reviewer
  → reporter
  → redteam.complete
```

- `presets/en/merge-batch.yml` 的 reviewer 使用 `git log <target>..<branch>`、三点 diff 和 `git merge-tree`；integrator 使用 `git merge --no-ff`，并把 merge SHA 传给 stabilizer/reporter。
- `presets/en/post-merge-converge.yml` 的 baseline 负责冻结 final-tree 状态；change-mapper 负责计划发现和 change map；后续 auditor、test-gap、reproducer、fixer、closer 通过 artifact path 消费上游结果。
- `presets/en/red-team-attack.yml` 的 target-locker 锁定 branch/HEAD/tree；plan-resolver 已有多信号 Git 搜索和 patch-id 设计，但 combined patch 的 `<global-baseline>` 未被定义；后续 attack-surface、experiment、evidence gate 都消费 plan-resolver artifact。
- `presets/schemas/merge-batch.yml`、`presets/schemas/post-merge-converge.yml` 和 `presets/schemas/red-team-attack.yml` 分别定义现有 event required fields、allowed values 和 field docs。
- `presets/en/implementation-review.yml` 的 `scope-preparer` 已定义单计划 `C^..HEAD` freeze、`scope-manifest.json`、`scope_digest`、`patch_digest`、候选冲突 fail-close、dirty intersection 和 pre-emit recheck；这是本计划的直接可复用模式，但其单计划模型不能直接满足本需求。
- `crates/ralph-core/src/git_ops.rs` 只有通用的 Git command wrapper；`crates/ralph-core/src/plan_baseline.rs` 保存单个 loop/plan 的 baseline SHA，不提供多计划 attribution，也不能成为 post-merge 的 authority。
- `crates/ralph-core/tests/scenarios.rs` 的 `run_workflow_guard_scenario` 走真实 EventLoop，已有 `implementation_review_scope` 等 fixture 证明 scope blocked/accepted 的 BDD 写法。
- `crates/ralph-cli/src/presets.rs` 已有 builtin YAML parse、manifest/index/zsh parity、strict lint 和 payload contract tests；`crates/ralph-cli/build.rs` 会读取 `presets/manifest.yml` 并 deep-merge schema。

#### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
| --- | --- | --- | --- | --- |
| E1 | `git rev-parse HEAD`, `git rev-parse HEAD^{tree}`, `git status --short`, `git show --stat --oneline HEAD` | 当前基线是 `81881076`，HEAD 是只归档既有计划的 docs-only commit，tree 为 `754f0321` | 计划 frontmatter 使用当前 HEAD；代码行为证据仍需区分 docs-only archive 与实现基线 | 高 |
| E2 | `presets/en/post-merge-converge.yml` baseline/change-mapper instructions | baseline 记录当前 HEAD；change-mapper 只用计划列表、chronology、branch subjects 计算 coarse `plan_match_confidence` | 必须增加独立 scope base、commit/hunk attribution 和 hard gate | 高 |
| E3 | `presets/schemas/post-merge-converge.yml` 的 `postmerge.changemap.ready` | 当前 required fields 只有 `proceed`、artifact path、high-risk count、coarse plan confidence | schema 要增加 scope manifest/digest/status/confidence/diff paths | 高 |
| E4 | `post-merge.prompt.md` | 计划列表和 final branch 是可选；没有 scope base 或 boundary 输入契约 | 必须新增可选 scope base/boundary 输入，同时保持省略计划列表可自动发现 | 高 |
| E5 | `presets/en/red-team-attack.yml` target-locker/plan-resolver | 已搜 `git log`、`-S/-G`、blame、patch-id，但 combined patch 使用 `<global-baseline>..<locked-head>` | 必须由独立 resolver 产出并验证 `scope_base_sha`，消除 placeholder | 高 |
| E6 | `red-team.prompt.md` | 只要求计划列表、可选 target branch/commit，没有 merge boundary 依赖 | red-team 必须在没有 merge boundary 时仍能自主解析 | 高 |
| E7 | `presets/schemas/red-team-attack.yml` | `redteam.target.locked`/`redteam.plan.resolved` 没有 scope manifest、scope base、diff attribution contract | 增加 resolved/blocked scope payload fields，并扩展 unresolved reason | 高 |
| E8 | `presets/en/merge-batch.yml` 与 `presets/schemas/merge-batch.yml` | integrator 已知道 target、分支顺序、merge SHAs、pre-merge/full verification，但没有结构化 boundary path/digest | merge-batch 增加可选证据产物，不成为下游 authority | 高 |
| E9 | `.ralph/merge/REPORT.md` 当前 report | 实际报告记录了 base `0033a79b`、三条 merge commit、最终 head 和全量测试结果 | 真实历史证明需要区分 merge boundary 与 final post-merge direct commit | 高 |
| E10 | `git log --first-parent --format='%H %P %s' -12` | 当前历史包含多条 merge commit，并在最后 merge 后有直接 target commit `ece2014b` | 必须覆盖 mixed mode，不能只做 `merge-batch` 结果解析 | 高 |
| E11 | `presets/en/implementation-review.yml` scope-preparer Step 2–6；`presets/schemas/implementation-review.yml` | 已有候选集合、多信号、ambiguous→blocked、`scope-manifest.json`、line-strip digest、patch digest、dirty/pre-emit recheck | 复用已验证 artifact/digest/阻断模式，不重新发明单计划协议 | 高 |
| E12 | `crates/ralph-core/src/git_ops.rs`、`crates/ralph-core/src/plan_baseline.rs`、`crates/ralph-core/src/lib.rs` | 当前只有通用 Git wrapper、单 plan baseline，且 `git_ops` 是 crate 内部模块；没有多计划 resolver 或 scope API | 不新增 scope CLI/core resolver；保留 agent-owned Git protocol，并把一致性/阈值 guard 接入已有 event policy 和 CLI emit precheck | 高 |
| E13 | `crates/ralph-core/tests/scenarios.rs` `run_workflow_guard_scenario` 与 `implementation_review_scope` tests | 真实 EventLoop BDD 已用于 scope accepted/blocked 和事件 absence 断言 | 新场景必须使用该入口，不得使用 stub `run_scenario` | 高 |
| E14 | `crates/ralph-cli/src/presets.rs`、`crates/ralph-cli/build.rs`、`presets/manifest.yml` | builtin 内容由 manifest/build.rs/embedded array 形成同步链；strict lint 和 payload contract 已存在 | 修改已有 preset/schema 但不新增 preset；必须跑完整 parity/lint | 高 |
| E15 | `skills/ralph-preset-author`、`skills/ralph-preset-review` 与 `skills/ralph-preset-review/tests/test_skill_anchors.py` | preset capability 变化必须同步 author/review references、fixtures 和 anchor tests | 新增 scope-resolution finding/rubric/fixture，不让评审继续接受无 scope gate 的 preset | 高 |
| E16 | `crates/ralph-core/data/ralph-tools-emit.md`、`ralph-tools-cmdref.md`、`skills/ralph-preset-*/references/commands.md` | 现有 emit/policy-check、payload consistency 和 schema 查看方式已稳定 | 同步 scope manifest、指标阈值、emit precheck/payload-consistency 使用说明和 preset author/review command references；不新增 CLI help surface | 中高 |
| E17 | Git history `c3f7c16d`、`8afab643` 及当前 preset author notes | 相邻 preset 能通过 artifact-first、schema SSOT、author/review fixture 进行闭环同步 | 新增行为必须保持同样的 operator/reviewer 可审计链 | 高 |
| E18 | 已执行 preset/lint/presets/doc-drift 命令输出 | 当前 builtin strict/lint/parity/doc drift 基线通过 | 实现后以相同命令作为每个相关 Unit 的回归门禁 | 高 |
| E19 | 当前基线上的 Git scope command smoke：`git show --format=fuller --no-patch HEAD`、`git diff --binary --full-index --no-color HEAD^..HEAD -- <file>`、`git blame --line-porcelain`、`git diff-tree --root`、`git patch-id --stable`、`git status --porcelain --untracked-files=all -z` | 所需 Git 命令均可执行并返回预期非空/可解析输出；命令可用性已确认，但混合历史归属算法仍需 H2 临时 fixture 验证 | H1 从待验证假设升级为已验证事实；Unit 3/4/6 仍需验证解析边界和 binary/rename/revert 归属 | 高 |
| E20 | `crates/ralph-core/tests/scenarios.rs::init_git_workspace`、`crates/ralph-core/Cargo.toml` 的 `tempfile` 依赖，以及现有 `crates/ralph-core/tests/*` 临时 Git 测试 | 当前仓库已有在 nextest 集成测试中创建临时 Git 仓库、提交 fixture 并运行真实 Git 命令的模式 | 新增 multi-plan Git evidence replay 应落在 `ralph-core` nextest integration test，不在当前 checkout 做 merge/rebase/reset，也不把 prompt grep 当作算法验证 | 高 |
| E21 | `crates/ralph-core/src/event_policy/validation.rs` 的 `payload_consistency`、`crates/ralph-cli/src/policy_check/gates.rs` 的既有 precheck、`crates/ralph-cli/src/commands/emit/command_impl.rs` 的 policy/provenance/isolated-scope/step-handoff 链 | 已有同 payload 一致性规则和 emit 前置拒绝链；可在真实 emit 前拒绝自相矛盾、过期或低于阈值的 scope handoff | scope owner 仍是 change-mapper/plan-resolver；新增 scope consistency guard、payload rules 和 artifact recheck 接入现有链，不新增 CLI 或 core resolver | 高 |

#### 2.3 受影响范围

**确认会修改的生产配置/文档/测试边界：**

- `crates/ralph-cli/src/policy_check/gates.rs`：已有 emit precheck 增加 scope handoff consistency/path/threshold guard。
- `crates/ralph-cli/src/commands/emit/command_impl.rs`：已有 emit precheck 链接入 scope guard；不增加命令。
- `crates/ralph-core/src/event_policy/validation.rs`、`crates/ralph-core/src/event_policy_payload_consistency.rs`：已有 payload consistency evaluator 增加 scope handoff predicate 支持。
- `crates/ralph-cli/src/commands/emit/tests_policy_check_reject.rs`、`tests_policy_check_accept.rs`、`crates/ralph-cli/tests/integration_emit_policy.rs`：覆盖 scope guard、payload consistency 和 unsafe bypass。

- `presets/en/merge-batch.yml`、`presets/schemas/merge-batch.yml`、`merge.prompt.md`；`presets/en/merge-batch-author-notes.md` 当前调查确认不存在，Unit 2 将按已存在的 post-merge/red-team author notes 结构新增该文件，作为明确的 planned addition，不把它描述成已有接口或既存文档。
- `presets/en/post-merge-converge.yml`、`presets/schemas/post-merge-converge.yml`、`post-merge.prompt.md`、`presets/en/post-merge-converge-preset-author-notes.md`。
- `presets/en/red-team-attack.yml`、`presets/schemas/red-team-attack.yml`、`red-team.prompt.md`、`presets/en/red-team-attack-author-notes.md`。
- `crates/ralph-core/tests/scenarios.rs` 和 `crates/ralph-core/tests/scenarios/*.yml`（新增真实 EventLoop BDD fixtures）。
- `crates/ralph-core/tests/multi_plan_scope_git.rs`（planned addition）：使用现有 `tempfile` 测试模式建立 direct-target、merge、mixed、override、revert、rename、binary fixture，执行真实 Git evidence replay；该测试验证证据数据和边界分类输入，不声称能够执行 agent prompt。
- `crates/ralph-core/data/ralph-tools-emit.md`、`ralph-tools-cmdref.md`：同步 agent-owned scope manifest、固定指标和 precheck/payload-consistency 使用说明。
- `crates/ralph-cli/src/presets.rs`（仅新增结构化 preset contract assertions，不锁定完整 prompt 文本）。
- `crates/ralph-cli/tests/integration_preset_builtin.rs`（仅在需要 CLI 级 builtin parse/strict contract 的测试已确认入口中追加行为断言）。
- `skills/ralph-preset-author/SKILL.md`、`skills/ralph-preset-author/references/{commands,finding-rubric,patterns,prompt-visibility}.md`。
- `skills/ralph-preset-review/SKILL.md`、`skills/ralph-preset-review/references/{commands,finding-rubric,patterns,prompt-visibility,agent-skill-audit}.md`、`skills/ralph-preset-review/fixtures/`、`skills/ralph-preset-review/tests/test_skill_anchors.py`。
- `docs/guide/presets.md`、`AGENTS.md`、`CLAUDE.md`、`.cursor/rules/multi-hat-isolation.mdc`（同步已有 builtin 行为描述；不是新增 preset manifest）。

**明确不受影响或不应修改的边界：**

- `presets/manifest.yml`、`presets/index.json`、`scripts/ralph-zsh-plugin.zsh` 不因本计划新增条目而修改；执行者仍需运行 parity 检查确认没有漂移。
- `crates/ralph-core/src/plan_baseline.rs` 不在实现范围内；它仍只表示单 loop/plan baseline。`crates/ralph-core/src/git_ops.rs` 不扩展为 scope resolver，不把 scope policy 混入既有 auto-commit/landing 逻辑。`crates/ralph-cli/src/main.rs`、`crates/ralph-cli/src/commands/inspect.rs` 不修改；`policy_check/gates.rs` 与 `emit/command_impl.rs` 是本计划确认的 guard 入口。
- 不修改 `.ralph/merge/`、`.ralph/post-merge/`、`.ralph/red-team/` 既有运行时状态文件；测试只能在临时 workspace 或运行时自身产物目录写入。

### 3. Decision Records 与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
| --- | --- | --- | --- | --- | --- | --- |
| D1 | 是否新增一个 preset | 新增 `multi-plan-converge`；扩展现有三个 preset | 扩展现有 `merge-batch`、`post-merge-converge`、`red-team-attack`，不新增 preset | E2、E5、E8、E14；三个现有入口已经覆盖 merge、converge、attack | 新 preset 会拆散同一 scope contract，且无法解决现有两个 preset 各自需要独立解析的问题 | 0.98 |
| D2 | scope 归属由谁执行、如何阻止伪造交接 | 新增 scope CLI/core resolver；三个 prompt 自由判断；agent-owned Git protocol + precheck consistency guard | 保留 `change-mapper` 和 `plan-resolver` 的 agent-owned Git 判断；两个 owner 必须写同一 `multi-plan-scope/v1` manifest、完整 evidence table、固定 score/status/reason；现有 emit precheck 和 `payload_consistency` 在 policy-check 与真实写入前检查 payload↔artifact、digest、locked HEAD/tree、阈值和 resolved↔proceed 一致性；不新增 CLI/core resolver | E11 的 artifact/digest/ambiguous→blocked 模式；E16/E21 的 payload consistency 与 precheck 代码；现有 preset owner topology | 新 CLI/core resolver 扩大 surface 且不是用户需要；纯 schema 只能校验字段形状，因此补足 existing precheck/consistency guard；agent 仍负责 Git attribution，runtime 不虚称已独立证明 hunk 归属 | 0.94 |
| D3 | manifest 格式与 digest | Markdown；普通 YAML；byte-stable JSON + digest | 使用 `multi-plan-scope/v1` 的 byte-stable JSON；顶层字段固定为 `schema_version`、`target_branch`、`locked_head_sha`、`locked_tree_sha`、`scope_base_sha`、`scope_base_source`、`plans`、`candidate_commits`、`hunk_classifications`、`artifacts`、`scores`、`decision`、`reason_codes` 和 `scope_digest`。对象字段按字典序序列化、数组保持语义顺序、UTF-8、无额外空白、固定一个结尾换行。计算 digest 时只从 canonical bytes 中移除 `scope_digest` 字段对应的 canonical member，再对剩余 bytes 做 SHA-256；写回的 `scope_digest` 不得参与自身计算。沿用 `implementation-review/scope-manifest.v1` 的 line-strip digest 语义，但明确上述 canonicalization 规则 | E11、E14 的 artifact-first/schema SSOT 证据 | Markdown/YAML 不利于下游精确复算；普通 JSON re-serialize 会产生 key order/whitespace digest drift；不明确自引用排除规则会产生 digest 循环；不固定顶层字段会让 validator 和 schema 漂移 | 0.96 |
| D4 | merge-boundary 是否是下游 authority | 必须由 merge-batch 先跑；存在就直接采用；可选并独立交叉验证 | 可选 evidence；postmerge/redteam 必须从计划/Git history 独立解析，差异即 `boundary_conflict` | E6、E10、用户已确认直接 target/mixed mode；E9 仅能证明某次 batch 结果 | 强依赖会使 direct-target 计划无法运行，也会把 merge-batch 误判传递到攻击审查 | 0.99 |
| D5 | scope base 如何确定 | 当前 branch 起点；baseline HEAD；最早计划候选 commit 的 first-parent parent；operator 显式 base | 固定算法：先锁定 HEAD/tree；显式 `scope_base` 必须是 40-char SHA、是 locked HEAD 和所有 selected candidate 的 ancestor；未提供时，对每份计划选择唯一的 earliest anchor：direct-target 为该计划最早 candidate；branch merge 只有在 merge commit 是 first-parent commit、存在多个 parent 且候选计划 tip 从非-first parent 可达时，才使用该 merge commit 的 first-parent parent，否则退回 direct candidate anchor。随后按 target first-parent 拓扑顺序取最早 anchor 的 first-parent parent；root/无 parent、多个同强度 anchor、非 ancestor 或无法证明时 blocked。禁止用 current HEAD、prompt 顺序或默认 branch 起点兜底 | E4、E5、E10、E11；E21 的 locked HEAD/tree 可复用；implementation-review 已对 `C^` fail-close | “最早共同下界”没有可执行定义，Executor 会在 merge commit、squash 和 direct commit 间自行选择；固定 anchor/parent 规则才能复现 | 0.94 |
| D6 | 多计划/混合历史如何分类 | 全量 `base..HEAD`；按 commit subject 粗分；commit + hunk 双向归属 | 核心 resolver 使用固定规则：candidate hard-signal 评分（显式 SHA/plan-id/path=40，唯一 expected file/symbol/test=30，patch-id/等价 patch=20，commit message 其它匹配=10；时间/作者/subject 相似度只能作 tie-break，不能单独 resolve）；分数并列或无 hard evidence → ambiguous。对 `git rev-list --topo-order --reverse <head> ^<base>` 的每个 commit 先做 plan ownership，再用 `git diff --unified=0 --find-renames=50%`、最终 blob OID、normalized path + normalized added-content + 3-line context 计算 hunk key；最终 text hunk 通过 blame/patch 链归属，后续非 owner 修改同 key → `overridden_later`，净删除且存在反向 patch → `reverted`，多 owner → `shared_by_multiple_plans`，无法证明 → `unknown`。`critical_paths` 固定为所有 plan expected files、candidate patch paths 和显式 claim/symbol 所属文件的并集；critical path 上的 unknown hunk 或 unknown commit 必须阻断，非 critical unknown 仍写入 residual。binary/rename 只允许 file-level unique owner，line-level 标为 `not_applicable`，不能虚报 coverage | E5 的 Git signals；E10 的 mixed history；E11 的 candidate/ambiguous pattern；E20 的临时 Git fixture 能执行这些命令 | 仅写“commit+hunk 双向归属”仍留下 hunk key、rename、binary、revert 和 critical unknown 的实现选择；固定输入集合、匹配 key 和 fail-close 规则才能避免模型自由发挥 | 0.91 |
| D7 | 置信度如何影响流程 | 只有单一 high/medium/low；85 即放行；任何低置信度都继续 | 固定维度与权重：`identity 15%`、`base 15%`、`commit_attribution 20%`、`hunk_attribution 25%`、`interleaved 10%`、`override_revert 5%`、`cross_check 10%`。各维度只允许 0/85/90/100；resolved 必须 overall≥90、`base/hunk/interleaved/cross_check`≥90、每份计划 identity≥85、critical unknown=0。无 boundary 时 `cross_check=not_applicable`，不因缺少可选证据阻断；有 boundary 时必须为 100/matched，冲突为 0/blocked。85–89 只能 residual，<85 或关键未知直接 blocked | E2/E3 当前 coarse confidence 是缺陷；E5 现有 redteam threshold 85/90；E11 有 ambiguous fail-close；S6 明确 no-boundary 必须可运行 | “分维度评分”没有权重和 no-boundary 语义，Executor 仍可通过任意平均方式放行；固定表和 `not_applicable` 规则消除绕过空间 | 0.96 |
| D8 | 是否新增 event topic | 新增 scope 专题 topic；复用现有 `changemap`/`plan.resolved` 事件 | 不新增 topic，只扩展现有 payload required fields、allowed values 和 artifact path；scope 仍由现有 change-mapper/plan-resolver owner 负责 | E2、E5、E7、E8 的已有 topology；E14 的 preset lint 约束 | 新 topic 会扩大 routing/ownership/required-event/BDD 变更，且 scope 是现有 hat 的职责 | 0.91 |
| D9 | full/scoped/interleaved/unknown diff 如何保存 | 只写一个 combined patch；只写摘要；分别落盘并在 manifest 建引用 | 分别写 full tree diff、计划 scoped diff、interleaved/override diff、unknown diff；event 只传 path/digest/count | E5 已有 per-commit/combined patch artifact；E11 已有 binary patch + digest | 只有摘要无法复核；单一 patch 无法证明增量边界和范围外提交 | 0.95 |
| D10 | post-merge fixer 是否每个 Finding 跑全量 suite | 当前每个 Finding 都跑全量；每个 Finding 只跑 targeted；targeted 后全量一次 | 单个 Finding 先跑最小 failing/regression/受影响 package，再在 closer 的 clean validation 跑一次 `./scripts/run-tests.sh`；任何 fix 前仍需 test-gap/reproducer 证据 | E2 当前 instructions 重复要求全量；仓库硬规则和现有 merge report 支持全量最终门禁 | 每个 Finding 全量会放大时序 flake、耗时和反馈噪声；只跑 targeted 会失去最终系统门禁 | 0.94 |
| D11 | scope contract 是否需要同步 agent-facing guide | 只修改 preset prompt；新增 CLI guide；同步现有 emit/cmdref 与 preset author/review references | 同步 `crates/ralph-core/data/ralph-tools-emit.md`、`ralph-tools-cmdref.md` 和 author/review commands，说明 agent-owned manifest 的字段来源、固定指标、`ralph emit --policy-check`/真实 emit、失败停止条件；不新增 `ralph-tools-scope.md`，不把完整算法实现细节写入通用 guide | E16、E21、AGENTS 的 skill guide 规则；现有 emit/payload consistency 已是 agent 可执行入口 | 新 CLI guide 会制造用户不需要的 surface；只改 prompt 会让通用 emit contract 漂移 | 0.95 |
| D12 | 如何阻止 agent 提交不一致或低置信度 scope handoff | 只依赖 agent 自报 path/digest；只在 EventLoop schema 校验字符串；允许 unsafe bypass | 在现有 `crates/ralph-cli/src/policy_check/gates.rs`/`command_impl.rs` precheck 链增加 scope handoff guard，并在三份 schema 的 `event_policy.payload_consistency` 增加结构化规则：`scope_status=resolved` 必须同时满足 `overall_confidence≥90`、`base/hunk/interleaved/cross_check≥90`、每个 plan identity≥85、`critical_unknown_count=0`、artifact status=complete；postmerge 还必须 `proceed=true`，redteam 还必须 `resolved_count≥1`；`scope_status=ambiguous|blocked` 不得 emit resolved/proceed success。真实 emit 和 `--policy-check` 都检查 manifest path/root、manifest/diff digest、locked HEAD/tree、固定分数、critical unknown、`proceed/resolved` 一致性；scope topics 即使 `--unsafe-no-policy-check` 也执行该 guard。guard 不重算 agent 的 hunk attribution，只拒绝自相矛盾、过期、越界或低于门槛的交接 | E16/E21 的现有 payload consistency 和 emit precheck；D7 固定 threshold；E11 的 artifact-first/recheck 模式 | 现有 schema 只检查 required fields；新增 CLI/core resolver 超出用户需求；guard 可以在不替换 agent 判断的前提下阻断明显伪造/漂移 | 0.93 |
| D13 | agent scope 解析如何保证资源边界与只读性 | 允许无限 history/patch；agent 自由写任意目录；新增 resolver 代替 agent | 在三个 preset prompt 中固定 repo-relative allowed roots、`.git`/runtime ledger 禁止路径、`MAX_PLANS=32`、每份 plan `≤1 MiB`、`MAX_COMMITS=4096`、每个 patch `≤32 MiB`、manifest `≤1 MiB`、hunk records `≤100000`、固定 Git argv/no shell/只读命令和原子 artifact 写入；超过上限必须写 blocked artifact 并不得 emit resolved。precheck 复核路径和 artifact size，不截断后放行 | E11 的 bounded artifact 模式；E20 的 tempfile Git pattern；AGENTS 的 runtime-state/安全边界 | 保留 agent-owned workflow；固定 protocol 与现有 precheck 足以防止资源/路径风险；不需要新增 CLI | 0.92 |

所有 Decision 均达到 0.85。D2 的残余风险明确为“agent 可能错误执行 Git 命令或错误解释结果”；D12 的 existing emit precheck/payload-consistency guard 会拒绝 payload/artifact 不一致、过期、越界和低于阈值的 handoff，但不宣称 runtime 已独立重算 agent 的 hunk attribution。对于“自洽但错误”的 attribution，必须依靠 postmerge/redteam 各自独立解析、boundary cross-check 和 unknown/冲突阻断；D7 的 hard gate 会阻止未知范围进入审计/攻击。

### 4. BDD 行为规格

#### Feature: 多计划最终树的独立 scope 解析

  **Background:** 目标 branch 已锁定，当前 HEAD/tree 可读取，计划文件路径可以是显式输入，也可以由 Git history 发现。

  **Scenario S1: merge-batch 成功后写出可交叉验证的 boundary manifest**

    Given integrator 按 prompt 顺序成功 merge 两个 worktree branch
    When integrator 完成最后一个 merge 并写入 `.ralph/merge/merge-boundary.json`
    Then `merge.integrated` payload 包含真实 `merge_boundary_path`、`merge_boundary_digest` 和 target before/after identity
    And manifest 列出每个 branch tip、merge commit、target base/head/tree 和 verification boundary
    And reporter 在文件不可读或 digest 不匹配时不发成功终态

  **Scenario S2: 计划直接连续提交到 target branch 时 post-merge 独立解析**

    Given target first-parent history 中依次包含 Plan A 的 direct commits 和 Plan B 的 direct commits
    And prompt 没有 `merge-boundary`
    When `post-merge-converge/change-mapper` 解析计划和 Git history
    Then manifest 为两份计划分别列出 plan digest、candidate commits、scope base 和 current owned hunks
    And `postmerge.changemap.ready.scope_status` 为 `resolved`
    And scoped diff 不包含未归属 commit/hunk

  **Scenario S3: merge 与 target 直接提交混合时识别穿插 commit**

    Given history 为 `B0 → merge Plan A → target direct commit X → merge Plan B → HEAD`
    When change-mapper 或 plan-resolver 独立重建范围
    Then X 被标记为 `interleaved`
    And X 的 patch path 出现在 `interleaved_diff_path` 而不出现在 `scoped_diff_path`
    And report/attack context 明确列出 X，不得称整个 `B0..HEAD` 都是计划实现

  **Scenario S4: 后续提交覆盖计划 hunk 时保留归属链**

    Given Plan A 引入某个 hunk，后续 target commit 修改同一 hunk
    When resolver 运行 commit 与 hunk attribution
    Then manifest 标记 `overridden_later`
    And scoped diff 以最终 tree 为准，同时保留 Plan A commit、覆盖 commit 和最终 hunk 的证据链
    And confidence 下降但不因“有覆盖”自动把覆盖 commit 当作 Plan A owned

  **Scenario S5: 关键 hunk 无法归属时 fail-close**

    Given full diff 中存在无法通过计划、commit、patch-id、blame 或 declared file 解释的关键 hunk
    When resolver 完成候选和 attribution
    Then `decision` 为 `ambiguous` 或 `blocked`
    And postmerge 不进入 system-auditor，redteam 不进入 attack-surface
    And blocker artifact 列出 unknown hunk、尝试过的证据和下一步调查动作

  **Scenario S6: merge-boundary 缺失时 postmerge/redteam 仍可运行**

    Given target tree 由 direct-target commits 产生且不存在 `.ralph/merge/merge-boundary.json`
    When 两个 preset 各自启动 scope resolution
    Then 两个 preset 都从计划/Git history 独立解析，不等待或创建虚假的 merge-boundary dependency
    And 两个 manifest 的 `scope_source` 不得是 `merge-batch-required`

  **Scenario S7: merge-boundary 与独立解析冲突时阻断**

    Given merge-boundary 声明的 target base 或 owned commit 集合与 postmerge/redteam 独立复算不一致
    When consumer 做交叉验证
    Then 独立结果保留为主证据但 `boundary_consistency=false`
    And scope decision 为 `blocked`，不得沿用 merge-boundary 的较高 confidence 放行

  **Scenario S8: red-team 使用已验证 scope base 重建 patch**

    Given red-team resolver 已把 `scope_base_sha` 绑定到 manifest digest
    When plan-resolver 写 combined current patch
    Then patch 命令使用 manifest 的真实 `scope_base_sha..locked_head`
    And artifact 中不出现 `<global-baseline>` 或未解析 placeholder
    And patch digest、manifest digest 和 target lock 在 emit 前可复算

  **Scenario S9: 置信度阈值区分 resolved、residual 和 blocked**

    Given plan identity=88、hunk attribution=94、无 critical unknown
    When resolver 汇总维度分数
    Then decision 不是 `resolved`，而是 `ambiguous`/residual
    Given overall=92、各关键维度≥90、unknown critical=0、cross-check=not_applicable（没有 merge-boundary）
    When resolver 汇总维度分数
    Then decision 为 `resolved`

  **Scenario S10: scope 解析期间 HEAD/tree 漂移时拒绝终态**

    Given scope owner agent 已写 patch/manifest 但 emit 前 `git rev-parse HEAD` 或 tree 与 lock 不同
    When agent 执行现有 emit precheck
    Then 写入 scope drift blocker
    And 不发 resolved handoff，不把旧 digest 传给下游

  **Scenario S11: 伪造或篡改 manifest 时 emit-time validator 拒绝**

    Given agent 已生成 manifest，但随后修改了 manifest、scope base、diff digest 或 locked tree 后重新计算了 event payload
    When agent 执行 `ralph emit <scope-topic> --policy-check`
    Then emit 被现有 precheck 中的 `scope_handoff_consistency`/`payload_consistency:<rule_id>` guard 拒绝
    And 不写入 event ledger，不发下游 success topic
    And `--unsafe-no-policy-check` 仍不能绕过该拒绝

  **Scenario S12: 同一 Git fixture 的 agent scope artifact 可重复且满足 contract**

    Given 临时仓库包含 direct-target、merge、interleaved、override、revert、rename 和 binary fixture
    When scope owner agent 按相同 plan bytes、locked HEAD/tree、base 和 boundary 输入分别执行两次 Git protocol
    Then 两次 manifest canonical bytes、scope digest、classification 和 confidence 完全一致
    And 输出目录外的 tracked tree、staged diff、unstaged diff 均不变化

  **Scenario S13: agent scope 输入越界或资源超限时 fail-close**

    Given plan/output/boundary path 指向 repo 外、`.git/`、runtime ledger，或 plan/history/patch 超过固定上限
    When agent 执行 scope protocol 并准备 emit
    Then agent 必须写入稳定 reason code 的 blocked artifact，且现有 emit precheck 拒绝 resolved handoff
    And 不写越界文件、不写 success manifest、不允许任何 scope topic resolved

### 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
| --- | --- | --- | --- | --- | --- |
| S1 | `merge.integrated` 缺 boundary path/digest 或 path 不可读时被拒绝；完整 payload 被接受并能进入 stabilizer | `crates/ralph-core/tests/scenarios.rs` 的真实 workflow scenario；`crates/ralph-cli/src/presets.rs` schema assertion | EventLoop 集成 + schema contract | Characterization：当前 merge chain 的 success/failure terminal；artifact path existence | 否 |
| S2 | 无 merge-boundary、direct target 两计划时产生 resolved scope 字段并进入 change-map | postmerge scope-resolved scenario fixture | BDD EventLoop + preset structural contract | 临时 Git history replay 记录命令输出；不把模型文本当断言 | 否 |
| S3 | interleaved commit 不进入 scoped diff，count/path/classification 一致 | postmerge mixed-history scenario；redteam mixed-history scenario | BDD payload gate + artifact contract review | State/trace characterization：first-parent 与 merge-parent 分类 | 否 |
| S4 | override/revert 记录链路且 confidence/decision 不虚高 | postmerge attribution scenario；redteam attribution scenario | Artifact contract + schema/route integration | Differential：最终 hunk 与原始 patch 的双向归属对照 | 否 |
| S5 | unknown critical 只能进入 blocked/unresolved，禁止下游 success topic | postmerge scope-blocked / redteam plan-unresolved scenario | 真实 EventLoop BDD，断言 `expected.absent_events` | Fault injection：缺失 plan、坏 SHA、坏 patch、Git 命令失败 | 否 |
| S6 | 两个 preset 在没有 merge-boundary 时仍能接受 direct-target 输入 | postmerge/redteam independent-input fixtures；builtin contract test | Preset config + BDD routing | Characterization：删掉 boundary 文件的 replay | 否 |
| S7 | boundary digest/base/commit disagreement 进入 blocked；不采用上游更高分 | boundary-conflict fixtures | BDD negative path + artifact review | Differential：独立 resolver 与 boundary manifest 的集合差异 | 否 |
| S8 | redteam combined patch 只能引用 manifest base，placeholder 缺失 | redteam plan-resolved schema + structural assertion | Schema contract + read-only artifact inspection | Mutation：把 base 替成 HEAD、branch tip、未定义变量，测试必须失败 | 否 |
| S9 | 85–89 不 resolved，≥90 且关键维度满足才 resolved | schema allowed values + scenario event payload assertions | Unit/contract + BDD | Boundary-value tests 85、89、90、critical unknown=1 | 否 |
| S10 | drift 后不发成功 handoff，reporter 只能输出 blocked/fail | postmerge/redteam drift scenario | 真实 EventLoop BDD | Fault injection：HEAD drift、tree drift、manifest digest tamper | 否 |
| S11 | 伪造/篡改 manifest 不能通过现有 emit precheck，unsafe bypass 也不能放行 | `crates/ralph-cli/src/commands/emit/tests_policy_check_reject.rs`、`crates/ralph-cli/tests/integration_emit_policy.rs` | CLI precheck integration | Mutation：篡改 base/digest/status/locked tree；验证 events file 未增加 | 否 |
| S12 | agent scope protocol 对同一 fixture 产出 byte-stable、只读、可重复 artifact | `crates/ralph-core/tests/scenarios/scope_agent_contract.yml`、`crates/ralph-core/tests/scenarios.rs` | EventLoop contract + artifact fixture | Differential：两次 canonical manifest/digest 对比；不把 mock 当 Git 算法证明 | 否 |
| S13 | 越界路径、超限 plan/history/patch 进入 blocked，不能以 resolved payload 通过 precheck | `crates/ralph-cli/src/commands/emit/tests_policy_check_reject.rs`、真实 EventLoop scenario | CLI precheck + BDD integration | Fault injection：绝对路径、`.git`、runtime ledger、超限 fixture | 否 |

测试原则：scope 解析仍由 `change-mapper`/`plan-resolver` agent 执行；BDD 负责真实验证 EventLoop 的 routing/schema/gate，现有 `payload_consistency` 和 CLI precheck 测试负责 payload/artifact/threshold/path/status 一致性，Git fixture 负责固定 protocol 的 characterization/differential 证据。不得把 mock payload 当作 agent Git 判断正确性的证明，也不得新增只检查 prompt 文案的测试；preset strict lint 只验证 topology/ownership/schema。

### 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| R1 | 三个 preset 各自独立解析 scope | S2、S6 | `postmerge_scope_direct_target`、`redteam_scope_direct_target` | manifest field/decision assertions | workflow guard scenarios | N/A | E2、E5、E6、D4 |
| R2 | scope base 必须可证明 | S2、S8 | `scope_base_is_manifest_bound` | base source/ancestor contract | changemap/plan.resolved payload | N/A | E5、E11、D5 |
| R3 | 支持 merge、direct、mixed history | S1、S2、S3 | three fixture acceptance tests | classification enum assertions | merge/post/red route tests | N/A | E9、E10 |
| R4 | interleaved/override/revert/shared/unknown 可追踪 | S3、S4、S5 | attribution fixtures | manifest classification table assertions | blocked gate scenario | N/A | E5、E10、D6 |
| R5 | merge-boundary 仅为可选交叉证据 | S6、S7 | no-boundary success + conflict block | `boundary_consistency` contract | post/red blocked scenario | N/A | E4、E6、D4 |
| R6 | confidence 分维度并硬门禁 | S5、S9 | threshold tests | score aggregation boundary tests | changemap/plan.resolved gate | N/A | E3、E5、D7 |
| R7 | postmerge 只在 resolved scope 后审计 | S5、S9、S10 | no-audit-on-blocked scenario | `proceed` consistency assertions | real EventLoop absent events | N/A | E2、E3 |
| R8 | redteam patch 不使用未定义 baseline | S8 | placeholder rejection/manifest base acceptance | patch path/base field assertion | plan.resolved/unresolved BDD | N/A | E5、E7 |
| R9 | merge-batch 输出 machine-readable boundary | S1 | merge boundary artifact contract | payload required-field test | merge integrated/stabilized workflow | N/A | E8、E9 |
| R10 | artifact-first、digest、HEAD/tree recheck | S7、S10 | tamper/drift tests | digest/path/status assertions | terminal reporter gate | N/A | E11、E14 |
| R11 | preset author/review 能发现 scope contract 缺失 | S5、S7 | negative fixture review run | anchor uniqueness test | strict preset review workflow | N/A | E15 |
| R12 | 不新增 preset/CLI surface/第三方依赖 | 全部 | preset inventory + schema/emit contract regression | builtin count/name parity + no new command assertion | full preset suite | N/A | E12、E14、D1、D2 |
| R13 | precheck 语义校验阻止不一致/篡改 scope artifact | S11、S10 | policy-check reject/accept tests | guard/path/digest/HEAD-tree/threshold assertions | emit policy-check + real emit | N/A | E21、D12 |
| R14 | agent scope protocol 有界、只读且可重复表达 | S12、S13 | `scope_agent_contract`、policy-check tests | canonical/differential/fault-injection tests | EventLoop + emit precheck integration | N/A | E20、D13 |

---

## Implementation Units

### Unit 1：建立 agent-owned scope manifest 与 emit consistency gate

#### 1. Unit 目标

让 `change-mapper` 和 `plan-resolver` 按同一 agent-owned scope protocol 生成结构化 manifest，并让现有 `ralph emit --policy-check`/真实 emit 在写 event 前检查 payload、manifest 和 scope 阈值的一致性；不新增 CLI。该 Unit 不替 agent 重算 Git attribution，只阻止不完整、自相矛盾、过期、越界或低于阈值的 scope handoff。

#### 2. 对应需求与 Scenario

- Requirements：R2、R6、R8、R10、R13、R14。
- Scenarios：S8、S9、S10、S11、S12、S13。
- Decisions：D2、D3、D5、D6、D7、D12、D13。
- Evidence：E11、E12、E20、E21。

#### 3. 外部可观察结果

`change-mapper`/`plan-resolver` 在无 merge-boundary、direct-target、merge 或 mixed fixture 中都能按同一 protocol 写出 manifest；相同输入要求 agent 产出相同 canonical bytes/digest。`merge.integrated`、`postmerge.changemap.ready` 和 `redteam.plan.resolved` 的 handoff 在 `ralph emit --policy-check` 和真实 emit 中都必须通过 consistency/threshold guard；伪造字段、payload↔artifact 不一致、过期 HEAD/tree、越界路径或低于阈值不能写入 success event。

#### 4. 当前行为基线

当前 `ralph-core` 没有多计划 resolver；`git_ops` 只有通用 wrapper，`plan_baseline` 只保存单 loop/plan SHA。当前三个 schema 只要求字符串字段，现有 payload consistency 尚未覆盖 scope 字段。先用真实 EventLoop/emit characterization 证明旧 success-shaped scope payload 可以绕过 scope consistency，再以 agent artifact fixture 建立 Red。

#### 5. 输入与输出

- 输入：scope owner agent 的 target branch/HEAD/tree、计划路径/ID、可选 `scope_base`、可选 merge boundary、agent 执行的 Git evidence、manifest/diff artifact 和 event payload；所有 artifact path 必须 repo-relative 且位于对应 `.ralph` 根目录。
- 输出：`.ralph/{merge,post-merge,red-team}/scope-manifest.json` 与 full/scoped/interleaved/override/unknown patch artifacts；manifest 使用 `multi-plan-scope/v1`、canonical JSON bytes、D3 固定的顶层字段、`scope_digest` 和固定维度 score。
- 错误：plan 不可读、candidate 无 hard evidence、base 非 ancestor、anchor/parent 不唯一、hunk unknown、resource limit、path 越界、HEAD/tree drift、manifest/diff digest 不一致或 score/status 矛盾 → agent 写 blocked artifact；现有 emit precheck 拒绝 resolved handoff。
- 状态变化：不新增 CLI、event topic 或 core resolver；扩展三份 schema 的 scope fields/payload consistency，并在现有 emit precheck 链增加 scope handoff guard。
- 副作用：scope owner agent 只写对应 `.ralph/merge/`、`.ralph/post-merge/` 或 `.ralph/red-team/` artifact；不执行 merge/rebase/cherry-pick/reset，不调用 shell wrapper，不写 runtime ledger。
- 不变量：相同 input snapshot 的 canonical bytes/digest/classification/score 一致；scope topic 的 `--unsafe-no-policy-check` 不能跳过 validator；`scope_base_sha` 必须由显式校验或固定 anchor/parent 算法产生。

#### 6. 修改位置

- `crates/ralph-cli/src/policy_check/gates.rs`：在现有 precheck gate 集合中增加 scope handoff consistency guard；只检查 artifact/payload/threshold/lock/path 一致性，不实现 Git attribution。
- `crates/ralph-cli/src/commands/emit/command_impl.rs`：在现有 provenance/isolated-scope/step-handoff 链中调用 scope guard；scope topics 即使 `--unsafe-no-policy-check` 也必须执行该 gate。
- `crates/ralph-core/src/event_policy/validation.rs`、`crates/ralph-core/src/event_policy_payload_consistency.rs`：复用并扩展现有 `payload_consistency` 规则，用于 resolved/proceed、score、unknown/status 和 artifact reference 的同 payload 校验。
- `presets/schemas/{merge-batch,post-merge-converge,red-team-attack}.yml`：扩展 manifest/status/score/threshold fields 和 `event_policy.payload_consistency` contract；schema 不宣称证明 hunk attribution。
- `crates/ralph-cli/src/commands/emit/tests_policy_check_reject.rs`、`tests_policy_check_accept.rs`、`crates/ralph-cli/tests/integration_emit_policy.rs`：覆盖 scope guard、payload consistency、tamper/drift、unsafe bypass。
- `crates/ralph-core/tests/scenarios.rs`/`tests/scenarios/*.yml`：只增加真实 EventLoop routing/absent-event fixtures；不把 mock payload 当 resolver correctness test。
- 明确不修改：`crates/ralph-core/src/event_loop/` 的路由语义、`crates/ralph-core/src/plan_baseline.rs`、`presets/manifest.yml`、`presets/index.json`、builtin preset 数量和 zsh preset values。

#### 7. 可依赖能力

现有 agent Git protocol、artifact canonicalization pattern、emit precheck chain、`payload_consistency` evaluator、schema deep-merge、event policy、strict lint、`run_workflow_guard_scenario` 和 builtin parse/parity tests。

#### 8. 禁止依赖的未来能力

不得在本 Unit 修改三个 preset 的业务 topology、merge boundary 采集或 auditor/attack 业务语义；不得把 semantic validator 改成只读 event payload 字符串；不得新增 `git2`/`gix` 或 shell dependency。

#### 9. 验收测试

- 测试名称：`scope_agent_manifest_characterization`；层级：真实 EventLoop BDD；前置：临时 Git fixture 及预先记录的 agent command/evidence 输出；动作：重放 direct/mixed/override/revert artifact；断言：manifest 字段、canonical bytes/digest、classification/score 与 fixture 一致，tracked/staged/unstaged tree 不变。该测试固定 agent protocol，不伪装成 runtime Git resolver 测试。
- 测试名称：`scope_handoff_consistency_rejects_tamper`；层级：CLI emit precheck integration；断言：篡改 manifest/base/diff digest/HEAD/tree/status/score 时 `--policy-check` 与真实 emit 都拒绝，事件文件不增加；`--unsafe-no-policy-check` 仍拒绝。
- 测试名称：`scope_payload_consistency_rejects_incoherent_status`；层级：`payload_consistency` unit + 真实 EventLoop BDD；断言：`scope_status=resolved` 搭配低分/critical unknown/proceed=false、或 blocked 搭配 resolved handoff 均被拒绝。
- 测试名称：`scope_payload_contract_rejects_missing_manifest_fields`；层级：真实 EventLoop BDD + schema contract；断言：缺字段的旧 success-shaped payload 不进入 accepted events，完整 payload 可进入下一跳。
- 命令：`cargo nextest run -p ralph-cli --bin ralph -- policy_check`、`cargo nextest run -p ralph-cli --test integration_emit_policy -- scope`、`cargo nextest run -p ralph-core --test scenarios -- scope_payload_contract`、`cargo nextest run -p ralph-core --test scenarios -- scope_agent_contract`。

#### 10. Acceptance Red

先运行 scope handoff consistency、payload consistency 和真实 EventLoop contract 测试；当前 schema/guard 不要求 scope fields 或不拒绝 incoherent payload，预期看到旧 success-shaped payload 被接受的真实 Red。若失败来自 fixture parse、测试未运行、未知 topic、错误命令或仅仅是 schema mock 失败，不算有效 Red，必须修正测试后重跑。

#### 11. 单元测试拆分

- `scope_base_uses_explicit_or_fixed_anchor_parent`：显式 base 非 ancestor、root、多个 anchor 或 HEAD fallback 均失败；合法 direct/merge anchor 通过。
- `scope_candidate_tie_is_ambiguous`：同分 candidate/无 hard evidence 不得 resolve；soft subject/time/author 不能单独通过。
- `scope_hunk_classification_is_deterministic`：interleaved/shared/override/revert/rename/binary 按固定 hunk key 和 file-level rule 输出；binary 不声称 line coverage。
- `scope_confidence_uses_fixed_weights`：85/89/90、critical unknown、no-boundary not_applicable、boundary conflict 的结果精确匹配 D7。
- `scope_handoff_guard_rechecks_artifact_references`：path、canonical digest、diff digest、locked HEAD/tree、allowed root 和 artifact size 均真实复核；不 mock guard 真实逻辑。
- `scope_payload_consistency_enforces_thresholds`：resolved/proceed、overall/key dimensions、critical unknown、boundary conflict 和 blocked reason 的组合按 D7/D12 fail-close。

#### 12. Red → Green → Refactor 顺序

1. `scope_payload_contract_rejects_missing_manifest_fields` Red：先证明旧 payload 可以进入 accepted path。
2. 扩展三个 schema 的 scope fields/allowed values 和 `payload_consistency` rules；缺字段/不一致 payload Green。
3. `scope_handoff_consistency_rejects_tamper` Red：在 `policy_check/gates.rs` 和 `emit/command_impl.rs` 现有链中增加 guard；tamper/drift/unsafe bypass Green。
4. `scope_agent_manifest_characterization` Red：加入 direct/mixed/override/revert/rename/binary/limit artifact fixture，固定 agent protocol 的输入输出和 blocked reason。
5. 更新三个 preset prompt，要求 owner agent 生成固定字段、分数、证据表和 bounded artifact；BDD Green。
6. Refactor：统一 manifest field names、reason codes、payload consistency rule IDs 和 schema field docs；不得把 scope 判断退回无结构 prompt。

#### 13. 最小实现范围

必须完成 agent-owned scope protocol、三个 schema contract、payload consistency rules、现有 emit precheck scope guard 和受影响 BDD payload 更新；不得新增 CLI、topic、依赖或 preset；不得让旧 success payload、payload/artifact 不一致、越界 artifact、低于阈值 handoff 或 unsafe bypass 继续通过。

#### 14. 集成验证

真实联合 scope owner artifact protocol、临时 Git evidence、emit precheck、`RalphConfig` parse、HatRegistry、EventLoop event policy、payload consistency 和 BDD runner；Git command transcript 使用临时仓库真实 Git，guard 本身不替 agent 执行 attribution。运行本 Unit targeted nextest 命令，并运行三个 preset 的 strict lint 子集。

#### 15. 风险驱动测试

使用 Characterization Test 固定旧 payload/旧 guard 缺失行为；使用 Differential Test 比较两次 agent artifact canonical output；使用 Fault Injection 覆盖 tamper/drift/path traversal/resource limit；使用 boundary-value 测试覆盖 fixed score 85/89/90、critical unknown 和 no-boundary not_applicable。

#### 16. 回归范围

直接相关：`policy_check/gates.rs`、`emit/command_impl.rs`、`event_policy/validation.rs`、`event_policy_payload_consistency.rs`、三份 schema、`preset_lint`、`presets`、所有现有 implementation-review/payload schema BDD。相邻范围：event-policy schema parity、topic ownership、required event path、emit token/precheck regressions。旧配置/旧数据：不修改 runtime ledger；确认非 scope topic 不误套 scope guard。构建目标：`ralph-core`、`ralph-cli`；Lint/typecheck 使用仓库标准命令。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| --- | --- | --- | --- |
| `presets/schemas/merge-batch.yml` | 修改现有配置 | boundary payload contract | E8 |
| `presets/schemas/post-merge-converge.yml` | 修改现有配置 | scope handoff contract | E3 |
| `presets/schemas/red-team-attack.yml` | 修改现有配置 | independent resolution contract | E7 |
| `crates/ralph-core/tests/scenarios.rs` | 新增测试注册 | 真实 EventLoop acceptance | E13 |
| `crates/ralph-core/tests/scenarios/scope_payload_contract.yml` | 新增测试 fixture | missing/complete payload cases | E13 |
| `crates/ralph-core/tests/scenarios/*.yml` 中经 `rg` 确认受影响的既有 fixture | 修改现有 fixture | 为 schema required fields 提供最小合法 payload，避免保留测试债务 | E3、E7、E8、E13 |
| `crates/ralph-cli/src/policy_check/gates.rs` | 修改现有 precheck | scope handoff consistency/path/threshold guard | E21、D12 |
| `crates/ralph-cli/src/commands/emit/command_impl.rs` | 修改现有 CLI | 在现有 emit 链接入 scope guard | E21、D12 |
| `crates/ralph-core/src/event_policy/validation.rs` | 修改现有验证 | 注册/执行 scope 相关 payload consistency | E21、D12 |
| `crates/ralph-core/src/event_policy_payload_consistency.rs` | 修改现有 evaluator | 复用固定 predicate 评估，不新增 CLI | E21 |
| `crates/ralph-cli/src/commands/emit/tests_policy_check_reject.rs`、`tests_policy_check_accept.rs` | 修改现有单元测试 | tamper/drift/threshold/unsafe bypass | E21、D12 |
| `crates/ralph-cli/tests/integration_emit_policy.rs` | 修改现有集成测试 | 真实 emit precheck contract | E21、D12 |

#### 18. 完成标准

agent-owned scope protocol、emit consistency/threshold guard、schema Red/Green 通过；S8–S13 相关 BDD/emit policy integration 和 strict lint 通过；没有新增 CLI、skip 或弱断言；scope handoff 不再只依赖自由文本 prompt；Evidence Ledger/Decision Record 已更新；Unit 可独立提交。

#### 19. 停止条件

如果 scope owner 无法在不调用 shell wrapper 的前提下获得所需 Git evidence、guard 无法读取/复核 artifact、`--unsafe-no-policy-check` 能绕过 scope gate、payload consistency rule 无法表达固定阈值、schema deep-merge 不读取新增字段、或非 scope topic 被误拦截，停止并重新检查 `policy_check/gates.rs`、emit precheck boundary、event policy contract 和 schema SSOT，不继续写 preset instructions。

#### 20. 风险与注意事项

字段过多会使 payload 变成长内容；缓解方式是只传 path/digest/count/status，详细内容留在 artifact。若 field docs 与 instructions 发生漂移，strict lint 可能无法发现，后续 author/review fixture 必须覆盖。

### Unit 2：让 merge-batch 产出可验证但非权威的 merge boundary

#### 1. Unit 目标

成功或失败的 `merge-batch/integrator` 都写出 `.ralph/merge/merge-boundary.json`，并把真实 path/digest/status 交给 reporter；该 artifact 只提供证据，不成为其他 preset 的启动依赖。

#### 2. 对应需求与 Scenario

- Requirements：R3、R5、R9、R10。
- Scenarios：S1、S7。
- Decisions：D3、D4、D9。
- Evidence：E8、E9、E10、E11。

#### 3. 外部可观察结果

merge report 会明确 target merge 前后 SHA/tree、每个 branch tip 与 merge commit、batch 内是否存在 target direct/interleaved commit、boundary digest 和失败状态；没有 boundary 文件或 digest 不一致不能报告 batch success。

#### 4. 当前行为基线

当前 integrator 已在 `.ralph/merge/integration.md` 记录 merge order/conflicts/SHA，reporter 已读取这些 Markdown artifact，但 `merge.integrated` 没有 machine-readable boundary path/digest。现有 `.ralph/merge/REPORT.md` 是 characterization evidence，记录了真实三分支 batch，但不是结构化 contract。

#### 5. 输入与输出

- 输入：target branch、prompt branch order、pre-merge target SHA/tree、每次 merge 前后 target SHA/tree、branch tip SHA、merge commit SHA、失败/abort reason。
- 输出：`.ralph/merge/merge-boundary.json`，包括 `schema_version: multi-plan-scope/merge-boundary.v1`、target identity、batch base/head、branch entries、merge commits、interleaved target commits observed in the batch window、verification command/result、`boundary_digest`、`boundary_status`。
- 错误：任何 merge 失败、merge in progress、manifest 写入失败、digest 重算失败 → `boundary_status: incomplete`，`integration_complete:false`，不伪造完整 boundary。
- 状态变化：现有 `merge.integrated`/`merge.stabilized` 事件字段扩展；stabilizer 仍只在 integration_complete 且 full verification 通过时 success。
- 副作用：只写 `.ralph/merge/`；不写计划 scoped diff，因为 merge-batch 不知道后续 direct target 计划范围。
- 不变量：不执行额外 merge；不把 boundary artifact 当作 post/red 的 required input。

#### 6. 修改位置

- `presets/en/merge-batch.yml` integrator：在每个真实 merge 后记录 checkpoint，最后一次成功/失败路径写 boundary JSON；不得修改 merge 顺序、abort 规则或全量验证门禁。
- `presets/en/merge-batch.yml` reporter/stabilizer：读取并验证 boundary path/digest，将其加入 report，但不改变 merge batch 的 completion topic。
- `presets/schemas/merge-batch.yml`：落地 Unit 1 contract 的 boundary fields。
- `merge.prompt.md`：说明 boundary 是本 batch 的证据，不能声明它涵盖后续 target direct commits。
- `presets/en/merge-batch-author-notes.md`：当前已确认不存在；Unit 2 新增该 author note，复用现有 post-merge/red-team author notes 的结构，说明 boundary 是 optional cross-check、不能成为下游 authority。
- 明确不修改：post/red preset、`presets/index.json`、target branch Git 历史。

#### 7. 可依赖能力

Unit 1 schema；现有 integrator 的 real `git merge --no-ff`、integration artifact、stabilizer/report chain；现有 report 的 branch/merge SHA 内容。

#### 8. 禁止依赖的未来能力

不得在本 Unit 判断某个 merge commit 属于哪个发展计划的最终 hunk；不得写 post/red 的 scope resolver；不得把 boundary 的 base 直接作为下游默认 base。

#### 9. 验收测试

- 测试名称：`merge_batch_boundary_payload_and_failure_path`。
- 前置：真实 workflow fixture 中 integrator success path 和 one-branch-failure path 各一份。
- 动作：模拟 integrator 发出带/不带 boundary 的 `merge.integrated`，再走 stabilizer/reporter。
- 断言：成功 payload 的 path/digest/status/target identity 完整；缺 path 或 digest 的 success event 被拒绝；失败路径写 incomplete semantics 且不得发 `passed:true`。
- 副作用断言：reporter 只消费 boundary artifact，不触发 postmerge/redteam topic。
- 命令：`cargo nextest run -p ralph-core --test scenarios -- merge_batch_boundary`。

#### 10. Acceptance Red

当前测试 fixture 不要求 boundary 字段，若新增测试直接把完整 boundary 断言设为必需，旧 preset/config 会先以“event payload 缺失”失败；这是目标缺失能力的有效 Red。若 test 只解析 YAML 文案而不经过 EventLoop，不算有效 Red。

#### 11. 单元测试拆分

- `boundary_manifest_contains_real_target_identity`：SHA/tree/path 必须非空且格式正确。
- `boundary_manifest_records_incomplete_merge`：failed branch、abort reason、status 与 `integration_complete:false` 一致。
- `boundary_digest_is_recomputed_before_report`：篡改 artifact 后 reporter 不得 success。
- `boundary_is_not_a_downstream_prerequisite`：没有 boundary 文件的 post/red independent fixtures 仍能进入各自 resolver（由 Unit 3/5 完成最终测试，本 Unit 只固定 merge artifact 不发依赖事件）。

#### 12. Red → Green → Refactor 顺序

1. 运行 boundary success/failure BDD，确认当前 merge payload 缺 fields Red。
2. 在 integrator instructions 加 checkpoint/boundary artifact 最小步骤。
3. 更新 schema payload 与 reporter/stabilizer 消费，测试 Green。
4. 加 digest tamper 和 incomplete merge tests Red。
5. 加 pre-report recheck 和 fail-close，测试 Green。
6. Refactor artifact 字段说明与 author notes 对齐。

#### 13. 最小实现范围

必须写 boundary JSON、传 path/digest/status、在 reporter 复核、在失败路径保持 false；不实现多计划 hunk attribution，不新增事件 topic，不改变 full suite 门禁。

#### 14. 集成验证

真实联合 merge-batch hat topology、schema policy、artifact path 和 reporter；Git merge 可用现有 workflow fixture/临时 workspace；不允许真实 target branch merge 作为测试副作用。成功/失败各跑一次 scenario。

#### 15. 风险驱动测试

Fault Injection：manifest 写入失败、digest 不匹配、第三个 branch merge 失败。Characterization：当前 report 的 branch/merge SHA/verification 信息仍保留。不要测试“report 含某段固定文案”。

#### 16. 回归范围

`merge-batch` preset_lint、merge schema parity、merge BDD、`presets` embedded strict lint、report path contract、全量 `ralph-core` scenario。确认 `merge.batch.complete` 的实际 terminal topic不改为文档中的 `MERGE_COMPLETE`。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| --- | --- | --- | --- |
| `presets/en/merge-batch.yml` | 修改现有 preset | boundary write/verify instructions | E8 |
| `presets/schemas/merge-batch.yml` | 修改现有 schema | boundary payload fields | E8 |
| `merge.prompt.md` | 修改 operator prompt | 说明 boundary 语义和限制 | E4、E9 |
| `presets/en/merge-batch-author-notes.md` | 新增 author note（planned addition） | author contract | E15 |
| `crates/ralph-core/tests/scenarios/merge_batch_boundary.yml` | 新增测试 fixture | success/failure boundary route | E13 |
| `crates/ralph-core/tests/scenarios.rs` | 新增测试注册 | real EventLoop acceptance | E13 |

#### 18. 完成标准

成功/失败 boundary scenario 通过；merge strict lint、schema parity、相关 preset tests 通过；reporter 不能读取过期/篡改 boundary；post/red 未被加入任何 required dependency；Unit 可独立提交。

#### 19. 停止条件

如果 integrator 无法在 merge commit 创建后取得 target SHA/tree、schema required fields 会使 incomplete path无法收敛、或现有 post-merge/red-team author notes 结构不足以定义 boundary 的 optional cross-check 语义，停止并更新决策；文件缺失本身不是阻塞，因为新增路径已在本计划中明确声明为 planned addition。

#### 20. 风险与注意事项

boundary 只描述 batch window，不能声称它是最终多计划 scope。文档和 payload 必须同时写明“optional cross-check only”，否则下游可能误用。

### Unit 3：让 post-merge-converge 独立解析 direct-target 与 branch-merge scope

#### 1. Unit 目标

让 `post-merge-converge/change-mapper` 在没有 merge-boundary 的情况下，根据显式或自动发现的计划和 Git history 产出 resolved 多计划 manifest，并把 verified scope 传给后续审计。

#### 2. 对应需求与 Scenario

- Requirements：R1、R2、R3、R5、R6、R7。
- Scenarios：S2、S6、S9。
- Decisions：D2、D4、D5、D6、D7。
- Evidence：E2、E4、E5、E6、E10、E11。

#### 3. 外部可观察结果

同一 branch 直接完成的多份计划不再因没有 merge commit 而被视为一个不透明 diff；change-map 会携带 `scope_base_sha`、计划集合、scoped/full diff path 和独立 confidence，并只有 resolved 才 `proceed:true`。

#### 4. 当前行为基线

当前 prompt 允许显式计划列表并把所有路径解析成功视为 high；没有 `scope_base_sha` 或 multi-plan manifest。先用 direct-target fixture 验证当前 change-map payload 没有新 scope fields，作为有效 Red。

#### 5. 输入与输出

- 输入：`post-merge.prompt.md` 的 final branch、计划列表（可省略）、可选 explicit scope base、可选 boundary path、baseline artifact、当前 HEAD/tree。
- 输出：`.ralph/post-merge/scope-manifest.json`、`02-change-map.md`、`.ralph/post-merge/diffs/{full,scoped,interleaved,override,unknown}.patch`；`postmerge.changemap.ready` 携带各 path/digest/status/confidence/count。
- 错误：计划路径不可读、candidate 为零、base 无法证明、HEAD/tree drift → `proceed:false` 和 blocker artifact。
- 状态变化：resolved 才触发完整 system-auditor；blocked/ambiguous 保持现有 short-circuit artifact chain。
- 副作用：不修改 code/test/plan，不重跑开发计划。
- 不变量：显式计划列表是输入证据，不是自动把路径变化全部算作 owned；省略列表时自动发现必须记录 unmatched。

#### 6. 修改位置

- `post-merge.prompt.md`：增加可选 `scope_base`、`merge_boundary_path`、scope mode 说明；保留省略计划列表的自动发现。
- `presets/en/post-merge-converge.yml` baseline/change-mapper：锁定 target identity，由 change-mapper 按固定 Git protocol 解析显式/自动发现的 plan 和可选 boundary，写 postmerge-owned manifest；audit 只读取通过 emit consistency/threshold guard 的 resolved manifest，不重复改变 attribution。
- `presets/schemas/post-merge-converge.yml`：使用 Unit 1 contract。
- `presets/en/post-merge-converge-preset-author-notes.md`：更新每个 payload 字段的 source/consumer/artifact owner。
- `crates/ralph-core/tests/scenarios.rs` 和新增 `postmerge_scope_direct_target.yml`：真实事件链。
- `crates/ralph-core/tests/multi_plan_scope_git.rs`：新增 `direct_target`、`branch_merge` replay，确认显式/推导 base、first-parent、merge-parent 和 locked HEAD/tree 的原始证据可复核；这是可执行的 Git evidence test，不是 prompt 文本测试。
- 明确不修改：`implementation-review.yml`、任何 runtime state file；postmerge prompt 必须遵守共享 scope protocol，但不引入新的 resolver module。

#### 7. 可依赖能力

Unit 1 schema、Unit 2 的可选 boundary artifact 语义、implementation-review 的候选/manifest/digest pattern、现有 baseline/change-map/auditor chain。

#### 8. 禁止依赖的未来能力

不得在本 Unit 决定 interleaved/override 的最终政策细节；Unit 4 会补 mixed history hard gate。不得让 postmerge 读取 `.ralph/merge/` 作为必需输入。

#### 9. 验收测试

- 测试名称：`postmerge_scope_direct_target_without_merge_boundary`。
- 前置：scenario fixture 声明两个计划、direct target commit 证据和 mock `postmerge.baseline.ready`。
- 动作：change-mapper 发 resolved `postmerge.changemap.ready`，随后 audit stub 仅验证触发条件和 scope fields。
- 断言：scope manifest path/digest/base/full/scoped paths 存在于 payload；`scope_source` 为 explicit/direct-target/inferred 之一；`merge_boundary_required` 不存在；后续 audit event 被接受。
- 副作用：不得生成 merge event，不得要求 branch list，不得把 entire target history 写进 payload。
- 命令：`cargo nextest run -p ralph-core --test scenarios -- postmerge_scope_direct_target`。

#### 10. Acceptance Red

旧 change-mapper payload 缺少 scope fields，新增 BDD 对新字段和 resolved gate 的断言先失败；若当前 instructions 仍能让测试 mock 发旧 payload 并被接受，说明 schema/fixture没有接到真实 EventLoop，必须停止修测试。

#### 11. 单元测试拆分

- `postmerge_plan_digest_is_recorded`：每个计划 path 的 SHA256 与读取 bytes 绑定。
- `postmerge_direct_target_scope_base_is_not_current_head`：base 必须是已验证 ancestor，不能等于 locked HEAD。
- `postmerge_resolved_payload_carries_diff_paths`：四类 diff path 和 digest/count 一致。
- `postmerge_explicit_base_must_be_ancestor`：非 ancestor 显式 base 阻断。
- 通过真实 Git fixture 重放 change-mapper agent 的 Git command/evidence protocol，验证输入输出；不 mock payload consistency、scope handoff guard 或 policy gate 本身。

#### 12. Red → Green → Refactor 顺序

1. Direct-target BDD Red。
2. 更新 prompt contract，要求 change-mapper 依固定 protocol 写 manifest/evidence，并只在 emit precheck 通过后消费 scope handoff。
3. 接入 resolver manifest/diff paths 并扩展 event payload，Green。
4. 显式 base 非 ancestor 与计划 digest mismatch Red。
5. 加 fail-close/blocked artifact，Green。
6. Refactor instructions 让所有 downstream 只读 trigger path，不扫描 runtime ledger。

#### 13. 最小实现范围

只实现 direct-target/branch-merge 的 plan candidate、scope base、manifest、digest、resolved gate；不实现 mixed interleaved/override 结论，不修改 auditor 业务审计内容，不运行 fix loop。

#### 14. 集成验证

联合 `baseline → change-mapper → system-auditor` 的真实 EventLoop routing；Git fixture 只读；完整 path 在 event payload 和 artifact contract 中复核；运行 postmerge strict lint 与 BDD。

#### 15. 风险驱动测试

Characterization：显式计划列表和省略计划列表两条输入都覆盖。Fault Injection：计划文件删除、scope base 非 ancestor、manifest digest tamper。不要把“模型写对命令”作为唯一测试。

#### 16. 回归范围

postmerge baseline/change-map schema、six audit short-circuit、test-gap/reproducer/fixer triggers、`postmerge.fix.ready` 与 closer/reporter terminal、旧 prompt 省略计划路径行为、默认 full suite discovery 不变。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| --- | --- | --- | --- |
| `post-merge.prompt.md` | 修改 operator prompt | 增加可选 scope 输入 | E4 |
| `presets/en/post-merge-converge.yml` | 修改现有 preset | 独立 direct/branch resolver | E2、E11 |
| `presets/schemas/post-merge-converge.yml` | 修改现有 schema | resolved handoff fields | E3 |
| `presets/en/post-merge-converge-preset-author-notes.md` | 修改说明 | payload/artifact ownership | E15 |
| `crates/ralph-core/tests/scenarios/postmerge_scope_direct_target.yml` | 新增 BDD fixture | direct target/no boundary | E13 |
| `crates/ralph-core/tests/scenarios.rs` | 新增测试注册 | real EventLoop path | E13 |
| `crates/ralph-core/tests/multi_plan_scope_git.rs` | 新增集成测试 | direct-target/branch-merge Git evidence replay | E20 |

#### 18. 完成标准

direct-target/no-boundary resolved scenario 通过；explicit base failure 通过；postmerge strict lint、schema parity、相关 tests 通过；auditor 只能在 resolved gate 后运行；Unit 可独立提交。

#### 19. 停止条件

如果 change-mapper 只能从 merge-boundary 取得 base、显式计划列表仍绕过 candidate evidence、或 `proceed:false` 不能阻断 auditor，停止并重新决策。

#### 20. 风险与注意事项

计划文件可能在实现提交后再次修改；manifest 必须保存 plan SHA256 和读取时间，并以 locked HEAD 下的 plan bytes 为证据，不能只保存 path。

### Unit 4：让 post-merge-converge 识别穿插、覆盖、共享和未知改动

#### 1. Unit 目标

在 Unit 3 的 resolved direct/branch 基础上，令 postmerge 对 mixed history 做最终 hunk attribution；关键未知或 boundary conflict 必须阻断审计，而不是把 full diff 当增量 diff。

#### 2. 对应需求与 Scenario

- Requirements：R3、R4、R5、R6、R7、R10。
- Scenarios：S3、S4、S5、S7、S9、S10。
- Decisions：D4、D6、D7、D9、D10。
- Evidence：E2、E9、E10、E11。

#### 3. 外部可观察结果

change-map 明确列出 interleaved/overridden/shared/unknown commit/hunk；`proceed:false` 时 system-auditor 不会把未知范围当已审范围；已确定的非关键 residual 进入 report 的待核实区。

#### 4. 当前行为基线

当前 change-mapper 只生成四张 change-map 表和 coarse confidence，无法证明 `merge Plan A → target X → merge Plan B` 的 X 被排除。先用 mixed fixture 证明旧行为会把完整范围当作计划交付或缺失分类，作为有效 Red。

#### 5. 输入与输出

- 输入：Unit 3 manifest、target first-parent/merge-parent graph、每个 candidate commit patch、当前 final diff、optional boundary evidence。
- 输出：manifest 的 `history_classification`、`hunk_attribution`、`interleaved_diff_path`、`override_diff_path`、`unknown_diff_path`、各 confidence 维度；`02-change-map.md` 增加冲突/未知表。
- 错误：critical unknown、hunk coverage<90、HEAD/tree drift，或提供 boundary 且 `cross_check=0` → blocked；无 boundary 时 `cross_check=not_applicable`，不能构造虚假的 disagreement。
- 状态变化：resolved only → `postmerge.changemap.ready.proceed:true`；blocked/ambiguous → full audit artifacts short-circuit and closer FAIL。
- 副作用：不修改生产代码；fixer 不处理 scope ambiguity。
- 不变量：final scoped diff 以 final tree 为准，同时保留原始 commit evidence；interleaved 不会因 shared file 自动变成 out-of-scope clean。

#### 6. 修改位置

- `presets/en/post-merge-converge.yml` change-mapper：增加 commit-centric/hunk-centric 两路证据、classification table、confidence gate 和 boundary cross-check。
- `presets/en/post-merge-converge.yml` system-auditor/test-gap/closer/reporter：消费 scope manifest；scope blocked 时只生成短路 artifact，closer/reporter 不得 PASS。
- `presets/schemas/post-merge-converge.yml`：扩展 status/count/diff/confidence fields 和 blocker reason allowed values。
- `crates/ralph-core/tests/scenarios/postmerge_scope_mixed_history.yml`、`postmerge_scope_blocked.yml`、`postmerge_scope_drift.yml`：新增 BDD fixtures。
- `crates/ralph-core/tests/multi_plan_scope_git.rs`：扩展 `mixed_history`、`override_revert`、`shared_hunk`、`rename_binary` replay；binary 只验证 file-level evidence，无法 line-level 时必须输出 unsupported/unknown。
- `crates/ralph-core/tests/scenarios.rs`：注册并断言 accepted/absent events。
- 明确不修改：reproducer/fixer 的 root-cause semantics；它们只接收 resolved scope。

#### 7. 可依赖能力

Unit 3 的 manifest/base/digest；Git `first-parent`、`show`、`diff-tree`、`blame`、`patch-id` 命令；现有 six-audit short-circuit/closer/reporter chain。

#### 8. 禁止依赖的未来能力

不得把 red-team 的 resolver 当 postmerge second opinion；Unit 6 才实现 redteam 独立交叉。不得用 fixer 修复未知 scope，不得把 confidence 调低后继续审计。

#### 9. 验收测试

- `postmerge_scope_mixed_history_classifies_interleaved`：S3，断言 X 在 interleaved path、不得出现在 scoped path。
- `postmerge_scope_unknown_hunk_blocks_audit`：S5，断言 `postmerge.audit.ready` absent、最终 verdict FAIL。
- `postmerge_scope_boundary_conflict_blocks`：S7，断言 boundary conflict 不沿用较高 confidence。
- `postmerge_scope_drift_blocks_before_emit`：S10，断言 no stale resolved event。
- 命令：`cargo nextest run -p ralph-core --test scenarios -- postmerge_scope_`。

#### 10. Acceptance Red

mixed fixture 在 Unit 3 实现后仍会缺 classification/unknown gate，预期新断言失败；如果它直接通过，说明测试只断言 report 文字而没有断言 event payload/absent event，应重写。Red 必须来自真实 EventLoop 接受了错误的 resolved event，或缺少预期 blocked route。

#### 11. 单元测试拆分

- `classify_target_commit_between_two_plan_merges`：X 只能是 interleaved。
- `classify_later_hunk_override`：后续 commit 不得继承原计划 owner，必须有 override edge。
- `shared_hunk_reduces_attribution_confidence`：两个计划共同修改同 hunk 时 shared 分类和 confidence 变化一致。
- `critical_unknown_forces_blocked`：unknown critical=1 即 blocked。
- `boundary_conflict_forces_blocked`：base/owned commit 集合不一致即 blocked。
- `head_tree_drift_invalidates_manifest`：drift 不得复用旧 digest。

#### 12. Red → Green → Refactor 顺序

1. Mixed/interleaved fixture Red。
2. 增加 history/hunk classification artifact 和分维度 score。
3. S3 Green。
4. unknown/override/boundary conflict fixtures Red。
5. 增加 hard gate、blocked artifacts、closer/report propagation。
6. S4/S5/S7/S10 Green。
7. Refactor change-map tables与manifest字段，确保 reporter只读路径。

#### 13. 最小实现范围

必须完成 mixed history classification、critical unknown gate、boundary conflict gate、pre-emit recheck；不新增 finding 修复行为，不改变六维审计内容，不让低置信度进入 VERIFIED。

#### 14. 集成验证

真实联合 change-mapper → system-auditor/test-gap/closer/reporter，断言 blocked path 的每个短路 artifact 和 terminal payload；Git fixture 只读并保存命令输出到临时测试证据。

#### 15. 风险驱动测试

采用 Differential Test：比较 change-mapper agent 两次 protocol artifact、计划 commit patch union、最终 blame/hunk evidence 三者；采用 Fault Injection：unknown hunk、boundary digest tamper、HEAD/tree drift。对 binary/rename 按 D6 的 file-level rule 验证，不能假定 line-level 成功。

#### 16. 回归范围

直接相关：postmerge change-map/audit/test-gap/reproduce/fix/closer/reporter。相邻模块：EventLoop required fields、terminal path、artifact-first report。兼容路径：省略计划列表、没有 merge boundary、旧 `.ralph/post-merge/` 残留。最终仍要跑仓库全量入口，不能用每个 Finding 的全量跑法替代 closer 全量门禁。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| --- | --- | --- | --- |
| `presets/en/post-merge-converge.yml` | 修改现有 preset | mixed attribution/blocked propagation/targeted verification | E2、E10 |
| `presets/schemas/post-merge-converge.yml` | 修改现有 schema | classification/confidence/block fields | E3 |
| `crates/ralph-core/tests/scenarios/postmerge_scope_mixed_history.yml` | 新增 BDD fixture | interleaved/override/shared | E13 |
| `crates/ralph-core/tests/scenarios/postmerge_scope_blocked.yml` | 新增 BDD fixture | unknown/conflict fail-close | E13 |
| `crates/ralph-core/tests/scenarios/postmerge_scope_drift.yml` | 新增 BDD fixture | pre-emit drift | E13 |
| `crates/ralph-core/tests/scenarios.rs` | 新增测试注册 | real route/absent events | E13 |
| `crates/ralph-core/tests/multi_plan_scope_git.rs` | 修改集成测试 | mixed/override/revert/shared/rename/binary evidence replay | E20 |

#### 18. 完成标准

S3/S4/S5/S7/S9/S10 通过；postmerge blocked path 不进入 auditor/fixer success；targeted fixer verification 与 closer full verification 指令明确；strict lint/BDD/regression 通过；Unit 可独立提交。

#### 19. 停止条件

如果 hunk attribution coverage 无法计算、interleaved 与 shared 无法区分、或 blocked path 仍能触发 fixer 修改 production code，停止并更新 D6/D7，不继续 redteam 工作。

#### 20. 风险与注意事项

Git blame 只能说明最终行 owner，不能单独证明计划意图；必须同时保留 commit patch、计划 claims、路径/symbol evidence 和 final hunk。报告应把“最终行 owner”和“计划归属”分开呈现。

### Unit 5：让 red-team-attack 独立解析 scope 并使用真实 patch base

#### 1. Unit 目标

让 `red-team-attack/plan-resolver` 在不读取 merge-batch boundary 的情况下完成 direct-target 或 branch-merge 的 scope freeze，并用 manifest 的真实 `scope_base_sha` 替换当前未定义的 `<global-baseline>`。

#### 2. 对应需求与 Scenario

- Requirements：R1、R2、R3、R5、R6、R8、R10。
- Scenarios：S2、S6、S8、S9。
- Decisions：D2、D4、D5、D9。
- Evidence：E5、E6、E7、E11、E12。

#### 3. 外部可观察结果

redteam 的 `redteam.plan.resolved` 明确携带真实 scope base、manifest/patch paths、digest 和 confidence；在没有 merge-boundary 时仍可 resolved；combined patch 不再引用 placeholder。

#### 4. 当前行为基线

当前 target-locker 已锁定 HEAD/tree，plan-resolver 已有 Git search 和 per-plan patch 规则，但 Step 3 使用 `<global-baseline>..<locked-head>`。先对现有 instructions 做一个 plan-resolved fixture characterization，证明当前 schema 不要求 scope base/manifest 和 placeholder 仍未被 contract 拦截；这是有效 Red 的事实依据。

#### 5. 输入与输出

- 输入：`red-team.prompt.md` 的 plans、target branch/commit、allowed environments、target lock artifact、当前 locked HEAD/tree。
- 输出：`.ralph/red-team/scope-manifest.json`、`02-plan-resolution.md`、`03-patch-reconstruction.md`、`.ralph/red-team/patches/full-current.patch`、scoped/interleaved/override/unknown patch paths；`redteam.plan.resolved` 携带 path/digest/status/confidence。
- 错误：scope base/candidate/plan attribution 无法证明 → `redteam.plan.unresolved`，reason 来自 schema allowed values。
- 状态变化：只有 resolved 才触发 `redteam.attack.mapped`；unresolved 继续既有 reporter fail path。
- 副作用：所有 patch/manifest 只写 `.ralph/red-team/`；不修改 tracked tree/Git history。
- 不变量：target lock identity、scope manifest digest、patch digest 在 resolver emit 前后均一致。

#### 6. 修改位置

- `red-team.prompt.md`：增加可选 scope base/boundary 输入和明确 no-boundary 支持。
- `presets/en/red-team-attack.yml` target-locker/plan-resolver：保留 target lock，由 plan-resolver 按固定 Git protocol 传入本次独立 lock/plan/base/boundary 参数并写 redteam-owned manifest/diff artifacts；禁止依赖 postmerge/merge-batch artifact 或省略 candidate/base/hunk evidence。
- `presets/schemas/red-team-attack.yml`：使用 Unit 1 fields，扩展 unresolved reason。
- `presets/en/red-team-attack-author-notes.md`：更新 target-lock/plan-resolution payload ownership 和 artifact paths。
- `crates/ralph-core/tests/scenarios/redteam_scope_direct_target.yml`、`redteam_scope_placeholder_blocked.yml`、`crates/ralph-core/tests/scenarios.rs`。
- `crates/ralph-core/tests/multi_plan_scope_git.rs`：增加 `redteam_independent` replay，使用与 postmerge 相同的 Git fixture 输入，但从独立 target lock/base 参数开始，证明 redteam 不读取 postmerge/merge-batch artifact。
- 明确不修改：`presets/templates/red-team-attack/` 既有 experiment/finding/report/plan 模板内容，除非测试证明 `PLAN.md` 必须新增 scope reference；优先在 instructions/artifact contract 中引用 manifest。

#### 7. 可依赖能力

Unit 1 schema、现有 target-locker clean/in-progress guard、现有 plan-resolver Git search/patch-id、implementation-review scope protocol。

#### 8. 禁止依赖的未来能力

不得在本 Unit 处理 mixed interleaved/override final policy；不得读取 merge-batch path 作为必需条件；不得设计 attack surface 或执行实验。

#### 9. 验收测试

- `redteam_scope_direct_target_without_merge_boundary`：无 boundary 仍产生 resolved handoff。
- `redteam_patch_reconstruction_requires_manifest_base`：patch artifact path/base/digest 完整，placeholder 不允许。
- 前置：real EventLoop fixture 发送 target.locked，mock plan-resolver success/blocked payload。
- 断言：resolved event 含真实 scope fields，attack.mapped 仅在 resolved path出现；placeholder/缺 base 进入 absent success + unresolved path。
- 命令：`cargo nextest run -p ralph-core --test scenarios -- redteam_scope_`。

#### 10. Acceptance Red

旧 redteam plan-resolved payload 缺 scope base/manifest；新增 test 先失败于 required field/absent event。若 test 仅 grep `red-team-attack.yml` 是否包含某字符串，不算有效 Red；必须经过真实 EventLoop schema/ownership gate。

#### 11. 单元测试拆分

- `redteam_scope_manifest_binds_locked_head_and_tree`：lock identity 与 manifest 一致。
- `redteam_scope_base_is_not_placeholder`：scope base 必须是 40-char Git SHA 且能被 artifact 证据复核。
- `redteam_plan_resolved_requires_scoped_patch_path`：缺 path/digest 失败。
- `redteam_unresolved_reason_accepts_scope_block`：新增 reason 经过 allowed values。
- `redteam_patch_digest_is_rechecked_before_emit`：tamper 后不可成功。

#### 12. Red → Green → Refactor 顺序

1. Plan-resolved missing scope fields Red。
2. 更新 plan-resolver instructions，要求按固定 scope protocol 写稳定 reason code/path/digest，并把 agent evidence 原样交给 schema handoff。
3. Direct/no-boundary success Green。
4. Placeholder/base/digest tamper Red。
5. 加 pre-emit recheck 和 unresolved route，Green。
6. Refactor reporter/attack-surface 只读取 trigger path，不自行重建不存在的 baseline。

#### 13. 最小实现范围

必须实现独立 direct/branch scope base、manifest、patch path/digest、placeholder hard prohibition、resolved/unresolved route；不实现 mixed classification 和 attack experiment。

#### 14. 集成验证

联合 target-locker → plan-resolver → attack-surface 的真实 EventLoop；使用只读临时 Git fixture验证 command evidence；实验 runner 不在本 Unit 运行。

#### 15. 风险驱动测试

Mutation Test：将 base 替换为 `HEAD`、当前 branch tip、未定义 placeholder 或 merge boundary path，必须被 contract/fixture 判定失败。Fault Injection：计划文件删除、target lock drift、patch 写入失败。

#### 16. 回归范围

redteam target lock clean/in-progress guard、plan unresolved reporter path、existing attack mapped/evidence gate、artifact-only write restriction、`redteam.complete` terminal payload。确保不增加 production code mutation 权限。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| --- | --- | --- | --- |
| `red-team.prompt.md` | 修改 operator prompt | no-boundary/direct-target input contract | E6 |
| `presets/en/red-team-attack.yml` | 修改现有 preset | independent resolver/real base | E5 |
| `presets/schemas/red-team-attack.yml` | 修改现有 schema | resolved/unresolved fields | E7 |
| `presets/en/red-team-attack-author-notes.md` | 修改说明 | artifact/payload ownership | E15 |
| `crates/ralph-core/tests/scenarios/redteam_scope_direct_target.yml` | 新增 BDD fixture | independent success | E13 |
| `crates/ralph-core/tests/scenarios/redteam_scope_placeholder_blocked.yml` | 新增 BDD fixture | placeholder fail-close | E5 |
| `crates/ralph-core/tests/scenarios.rs` | 新增测试注册 | real EventLoop path | E13 |
| `crates/ralph-core/tests/multi_plan_scope_git.rs` | 修改集成测试 | redteam independent base/patch evidence replay | E20 |

#### 18. 完成标准

no-boundary direct success、placeholder blocked、digest tamper 通过；redteam strict lint/BDD/相关 CLI preset tests 通过；attack-surface 不能越过 unresolved；Unit 可独立提交。

#### 19. 停止条件

如果 plan-resolver 仍需要从 merge-batch 获取 base、`<global-baseline>` 能进入任何 artifact/event、或 unresolved path 没有 reporter terminal，停止并重新比较 D2/D4。

#### 20. 风险与注意事项

redteam 的“计划实现 commit”与“最终有效 hunk owner”不是同一概念；攻击实验必须同时绑定两者，不能只绑定 commit subject。

### Unit 6：让 red-team-attack 对混合历史和置信度冲突 fail-close

#### 1. Unit 目标

让 redteam 独立识别 interleaved、shared、override、revert、unknown 和 optional boundary disagreement，并阻止不满足阈值的 scope 进入攻击实验。

#### 2. 对应需求与 Scenario

- Requirements：R3、R4、R5、R6、R8、R10、R11。
- Scenarios：S3、S4、S5、S7、S9、S10。
- Decisions：D4、D6、D7、D9。
- Evidence：E5、E7、E9、E10、E11、E15。

#### 3. 外部可观察结果

redteam report/PLAN.md 能区分可攻击的计划 owned patch 和 interleaved/unknown residual；存在 critical unknown 或 boundary conflict 时只生成 unresolved report/questions，不运行 attack experiment。

#### 4. 当前行为基线

当前 redteam 有 patch attribution coverage≥90 和 critical claim traceability=100 的 gate，但没有统一 mixed-history manifest、scope base agreement 或 `scope_status`；因此可能在 combined patch 下界不明时继续 attack-surface。

#### 5. 输入与输出

- 输入：Unit 5 independent manifest、locked target、plan claims、commit/hunk evidence、optional merge boundary。
- 输出：更新 `.ralph/red-team/02-plan-resolution.md`、`.ralph/red-team/03-patch-reconstruction.md`、scope manifest、`redteam.plan.resolved/unresolved` payload；resolved 才生成 attack-surface inputs。
- 错误：unknown critical、coverage<90、identity<85、key dimension<90、提供 boundary 且 `cross_check=0`、或 drift → unresolved；不提供 boundary 时 `cross_check=not_applicable`，不是 disagreement，也不能被误判为阻断原因。
- 状态变化：unresolved 直接进入 reporter failure path；不得创建正式 attack finding。
- 副作用：保持 redteam read-only 和每项实验 clean-tree proof。
- 不变量：redteam 的 independent result 是 authority；merge boundary 仅用于比较；不会把 boundary 的高 confidence 覆盖独立低 confidence。

#### 6. 修改位置

- `presets/en/red-team-attack.yml` plan-resolver、attack-surface-mapper、evidence-gate、impact-boundary、independent-reviewer、reporter：传递和验证 scope fields；scope unresolved 时禁止 attack mapped。
- `presets/schemas/red-team-attack.yml`：补 classification/confidence/coverage/blocked reason fields。
- `presets/en/red-team-attack-author-notes.md`：补 mixed/blocked payload contract。
- `presets/templates/red-team-attack/plan.template.md`、`report.template.md`：只在现有模板实际缺少 scope identity section 时追加机器可读字段；不得把完整 patch 放进 PLAN/REPORT。
- 新增 `crates/ralph-core/tests/scenarios/redteam_scope_mixed_history.yml`、`redteam_scope_boundary_conflict.yml`、`redteam_scope_unknown_blocked.yml` 并注册。
- 扩展 `crates/ralph-core/tests/multi_plan_scope_git.rs` 的 `redteam_mixed`、`redteam_boundary_conflict`、`redteam_unknown` replay；这些测试只验证 Git 证据集与预期分类，不执行攻击实验。

#### 7. 可依赖能力

Unit 5 的 independent manifest/base/patch；现有 evidence-gate retry、impact-boundary、independent-reviewer 和 reporter terminal。

#### 8. 禁止依赖的未来能力

不得把 postmerge 的 manifest 当 redteam 输入；不得修改 experiment runner 的攻击维度；不得在 unknown scope 上创建 RTF finding 或 PLAN fix unit。

#### 9. 验收测试

- `redteam_scope_mixed_history_classifies_interleaved`：mixed direct/merge history 中 X 只在 interleaved evidence。
- `redteam_scope_boundary_conflict_unresolved`：boundary 与独立结果不一致，`redteam.plan.unresolved`，`redteam.attack.mapped` absent。
- `redteam_scope_unknown_hunk_blocks_attack`：critical unknown blocks before attack.
- `redteam_scope_threshold_boundaries`：85/89/90 和 critical unknown 边界。
- 命令：`cargo nextest run -p ralph-core --test scenarios -- redteam_scope_`。

#### 10. Acceptance Red

在 Unit 5 的 direct path Green 后加入 mixed fixture；当前 redteam instructions/schema 会继续把 plan-resolved 发送到 attack-surface，预期 absent event 断言失败。若没有真实 `redteam.attack.mapped` accepted event，必须检查 fixture 是否经过 plan-resolver owner，而不是弱化断言。

#### 11. 单元测试拆分

- `redteam_interleaved_commit_excluded_from_attack_patch`。
- `redteam_override_and_revert_edges_are_traceable`。
- `redteam_boundary_consistency_disagreement_is_blocked`。
- `redteam_unknown_critical_never_creates_finding`。
- `redteam_confidence_gate_is_monotonic`：低维度不能被其他高分平均掩盖。
- `redteam_reporter_failure_path_validates_plan_unresolved_artifact`。

#### 12. Red → Green → Refactor 顺序

1. Mixed-history attack mapping Red。
2. 加 classification/coverage/critical unknown 结构和 plan unresolved gate。
3. S3/S5 Green。
4. boundary conflict/threshold/drift Red。
5. 加 cross-check、monotonic score、reporter blocked path。
6. S7/S9/S10 Green。
7. Refactor attack-surface instructions 只消费已验证 scoped patch，移除任何隐式全量 diff fallback。

#### 13. 最小实现范围

必须实现 mixed classification、confidence/coverage gate、boundary conflict fail-close、attack-surface refusal；不改变实验命令、控制组/攻击组要求、finding severity 或 repair plan 内容。

#### 14. 集成验证

联合 plan-resolver → attack-surface/evidence/impact/reviewer/reporter 的成功和 unresolved 路径；验证 redteam artifact 全在 `.ralph/red-team/`，tracked tree 不变；不执行真实外部服务实验。

#### 15. 风险驱动测试

Differential：independent manifest 与 optional boundary 的 commit/hunk set 对比。Mutation：删除 unknown gate、把 89 改成 90、把 `scope_base` 替为 HEAD，测试必须发现。Fault Injection：target/tree drift、boundary digest tamper、missing artifact。

#### 16. 回归范围

redteam strict lint、所有 existing redteam BDD/fixture、target lock、evidence-gate retry、impact boundary qualified/rejected、independent review、reporter unresolved terminal。确认现有 `control_passed`、`attack_reproduced`、`evidence_paths` contract 不变。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| --- | --- | --- | --- |
| `presets/en/red-team-attack.yml` | 修改现有 preset | mixed/confidence gates | E5 |
| `presets/schemas/red-team-attack.yml` | 修改现有 schema | blocked/coverage fields | E7 |
| `presets/en/red-team-attack-author-notes.md` | 修改说明 | payload/artifact contract | E15 |
| `presets/templates/red-team-attack/plan.template.md` | 条件性修改模板 | 仅补 scope identity if missing | E17 |
| `presets/templates/red-team-attack/report.template.md` | 条件性修改模板 | 仅补 scope residual section if missing | E17 |
| `crates/ralph-core/tests/scenarios/redteam_scope_mixed_history.yml` | 新增 BDD fixture | mixed attribution | E13 |
| `crates/ralph-core/tests/scenarios/redteam_scope_boundary_conflict.yml` | 新增 BDD fixture | optional evidence conflict | E13 |
| `crates/ralph-core/tests/scenarios/redteam_scope_unknown_blocked.yml` | 新增 BDD fixture | hard gate | E13 |
| `crates/ralph-core/tests/scenarios.rs` | 新增测试注册 | real EventLoop path | E13 |
| `crates/ralph-core/tests/multi_plan_scope_git.rs` | 修改集成测试 | redteam mixed/conflict/unknown evidence replay | E20 |

#### 18. 完成标准

所有 redteam mixed/conflict/unknown/threshold scenarios 通过；attack-surface 不能绕过 unresolved；reporter 交付 REPORT/PLAN/QUESTIONS 的 blocked 版本；strict lint、BDD、artifact contract、read-only checks 通过；Unit 可独立提交。

#### 19. 停止条件

如果 redteam 仍可在 scope unresolved 时生成 experiment plan/finding、任何实验修改 tracked tree、或 boundary conflict 被高 confidence 覆盖，停止并回退到 D4/D7 重新决策。

#### 20. 风险与注意事项

攻击实验可能产生未跟踪临时文件；必须限定到 `.ralph/red-team/` 或临时目录并在每项实验后检查 `git status --porcelain=v1`、unstaged diff 和 staged diff。不得通过删除证据来制造 clean tree。

### Unit 7：同步 operator、preset author/review 和文档质量门禁

#### 1. Unit 目标

让 preset 作者、评审者和 operator 能发现三套 preset 缺少独立 scope、错误依赖 merge-boundary、误用 coarse confidence 或使用未定义 diff base；同时保持 builtin manifest/index/completion 数量不变。

#### 2. 对应需求与 Scenario

- Requirements：R11、R12、R14。
- Scenarios：S1–S13 的文档/审计映射。
- Decisions：D1、D2、D11、D12、D13。
- Evidence：E14、E15、E16、E17、E21。

#### 3. 外部可观察结果

`ralph-preset-author`/`ralph-preset-review` 的 checklist、finding rubric、commands、patterns、prompt visibility 和 negative fixture 会把 scope contract 当 capability 评审；`docs/guide/presets.md` 会准确说明三套 preset 的输入、独立性、阻断和输出；builtin 数量和 completion 不增加。

#### 4. 当前行为基线

author/review skills 已有 capability-triggered audit、artifact-first、payload contract 和 fixture anchor，但没有 multi-plan scope findings；`docs/guide/presets.md` 对三个 workflow 的描述不完整且 merge completion 文案存在与实际 topic 不一致风险。

#### 5. 输入与输出

- 输入：Unit 1–6 的 schema/preset behavior、当前 author/review references、现有 fixture anchor test、builtin docs。
- 输出：new scope finding IDs、正/负 fixture、author/review 命令表、scope pattern、prompt visibility checks、指南和 builtin 描述同步；不新增 scope CLI 文档。
- 错误：fixture 宣传 scope gate 但 preset 缺 fields、只依赖 merge-boundary、把 85–89 放行、使用 placeholder base → review finding/block。
- 状态变化：不改 runtime event topology，不增 preset manifest entry。
- 副作用：只修改文档/skill fixtures/tests；不触碰 `.ralph/` runtime state。
- 不变量：skill 文档仍 agent-facing/可执行，不泄漏内部 ledger、源码函数名或一次性事故路径；不锁定完整 prompt 文本。

#### 6. 修改位置

- `skills/ralph-preset-author/SKILL.md`、`references/commands.md`、`finding-rubric.md`、`patterns.md`、`prompt-visibility.md`：增加 capability-triggered scope audit。
- `skills/ralph-preset-review/SKILL.md`、同名 references 和 `agent-skill-audit.md`：独立 reviewer 视角、finding severity/confidence 和输入可见性。
- `skills/ralph-preset-review/fixtures/`：新增 scope-positive、missing-scope-negative、merge-boundary-dependency-negative、placeholder-base-negative、confidence-gate-negative fixtures；每个 finding ID 唯一。
- `skills/ralph-preset-review/tests/test_skill_anchors.py`：只锁定稳定 heading/finding ID/fixture anchors，不锁定完整 prompt 文案。
- `docs/guide/presets.md`、`AGENTS.md`、`CLAUDE.md`、`.cursor/rules/multi-hat-isolation.mdc`：同步已有 builtin behavior description。
- `crates/ralph-core/data/ralph-tools-emit.md`、`ralph-tools-cmdref.md`：同步 agent-owned scope manifest、固定指标、payload consistency、emit precheck 和 `--unsafe-no-policy-check` 的不可绕过语义；运行既有 help/doc drift 验证。
- `scripts/ralph-zsh-plugin.zsh`、`presets/manifest.yml`、`presets/index.json`：执行 parity check，预期不修改。

#### 7. 可依赖能力

Unit 1–6 已通过的字段、artifact、gate、status 和 path contract；现有 author/review workflow、fixtures、anchor test。

#### 8. 禁止依赖的未来能力

不得新增 preset、CLI surface 或 unrelated generic injected skill；scope contract 只能同步到已有 emit/cmdref guide，不得固定为某个计划 ID、某次 merge report 或某个仓库专属路径。

#### 9. 验收测试

- `scope_resolution_negative_fixture_is_detectable`：review workflow 对缺 scope/错误 dependency/placeholder/confidence fixture 产生预期 finding。
- `scope_resolution_positive_fixture_has_independent_authority`：正 fixture 不误报 merge-boundary dependency。
- `scope_anchor_contracts_are_unique`：anchor test 通过且 finding IDs 无碰撞。
- `docs_and_builtin_inventory_remain_aligned`：preset/index/manifest/zsh parity 通过，数量未增加。
- `scope_emit_contract_matches_help`：既有 `ralph-tools-emit.md`/`ralph-tools-cmdref.md` 中的 policy-check、payload consistency、scope fields 和停止条件与 `ralph emit --help`/`ralph emit --schema` 一致。
- 命令：`skills/.venv/bin/python -m pytest skills/ralph-preset-review/tests/test_skill_anchors.py -q`，`cargo nextest run -p ralph-cli --bin ralph -- emit`，以及已有 preset strict/parity/doc drift 命令。

#### 10. Acceptance Red

新增 negative fixture 在缺少 rubric/anchor 时不能被验证；先运行 anchor test，预期 finding ID/fixture anchor 缺失失败。正 fixture 不得只通过字符串 grep；必须由现有 review fixture loader 解析 YAML 结构和 advertised finding ID。

#### 11. 单元测试拆分

- author/review 两份 `finding-rubric.md` 都包含同一 scope finding ID 与 severity/confidence。
- commands reference 都包含 scope contract 的真实 preset/schema/emit/policy-check 动作。
- `ralph-tools-emit.md`/`ralph-tools-cmdref.md` 只描述 agent 下一步可执行的 emit/policy-check 动作、字段来源和失败停止条件，不泄漏内部 guard 实现。
- patterns/reference 都要求 independent resolver、optional boundary、unknown hard gate、manifest digest。
- fixture advertised ID 唯一且 positive/negative 语义互补。
- `scripts/ralph-zsh-plugin.zsh`、`presets/index.json`、`manifest.yml` 的 builtin name set 相等，但 set 不增加。

#### 12. Red → Green → Refactor 顺序

1. Anchor/fixture test Red。
2. 更新 author/review skill references、rubric 和 fixtures。
3. Anchor/fixture test Green。
4. docs drift/parity tests Red（若当前 merge completion 文案或 builtin 描述不一致）。
5. 更新 `docs/guide/presets.md`、AGENTS/CLAUDE/rule 描述。
6. 运行 doc drift/parity Green。
7. Refactor duplicated scope terminology，保留统一字段名和 finding ID。

#### 13. 最小实现范围

必须同步 operator/author/review 可审计契约、已有 emit/cmdref guide 和验证 fixture；不新增 preset/CLI surface，不修改 event topology，不锁定全文 prompt。

#### 14. 集成验证

联合 skill anchor tests、`ralph preset check --strict`、builtin manifest/index/zsh parity 和 CLI doc drift；schema/event 命令说明通过 `ralph emit --schema`/`--policy-check` smoke 验证。

#### 15. 风险驱动测试

使用 negative fixture 防止 reviewer 对无 scope gate 假阴性；使用 anchor uniqueness 防止两份 skill 分叉；使用 doc drift 防止 `merge.batch.complete` 与旧 `MERGE_COMPLETE` 文案再次不一致。无需 snapshot/golden 更新。

#### 16. 回归范围

author/review skill anchors、所有既有 fixture、builtin preset count/name parity、zsh values、`scripts/check-cli-doc-drift.sh`、`ralph preset check --strict`、`cargo nextest` preset suites。确认 `crates/ralph-core/data/*.md` 没有新内部路径、函数名、计划编号或 preset-specific runtime ledger。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
| --- | --- | --- | --- |
| `skills/ralph-preset-author/SKILL.md` | 修改文档 | capability audit | E15 |
| `skills/ralph-preset-author/references/{commands,finding-rubric,patterns,prompt-visibility}.md` | 修改文档 | scope review contract | E15 |
| `skills/ralph-preset-review/SKILL.md` | 修改文档 | independent review gate | E15 |
| `skills/ralph-preset-review/references/{commands,finding-rubric,patterns,prompt-visibility,agent-skill-audit}.md` | 修改文档 | finding/audit contract | E15 |
| `skills/ralph-preset-review/fixtures/` | 新增 fixtures | positive/negative scope cases | E15 |
| `skills/ralph-preset-review/tests/test_skill_anchors.py` | 修改测试 | stable anchors and ID uniqueness | E15 |
| `docs/guide/presets.md` | 修改文档 | operator behavior contract | E17 |
| `AGENTS.md`、`CLAUDE.md`、`.cursor/rules/multi-hat-isolation.mdc` | 修改文档 | builtin behavior sync | E14、E17 |
| `crates/ralph-core/data/ralph-tools-emit.md`、`ralph-tools-cmdref.md` | 修改 agent-facing 文档 | scope manifest、payload consistency、emit precheck contract | E16、E21 |

#### 18. 完成标准

所有 author/review fixtures、anchors、strict lint、manifest/index/zsh parity、CLI doc drift、`ralph emit --help`/`--schema` smoke 和相关 nextest 通过；没有新增 preset/CLI surface；agent-facing emit guide 满足作用域和可执行性规则；Unit 可独立提交。

#### 19. 停止条件

如果 emit guide 需要泄漏内部 guard 实现、出现 skill author/review finding ID 分歧、`ralph emit --help`/`--schema` 与 guide 漂移、或 builtin inventory 发生无计划增删，停止并重新审查 D1/D2/D11/D12。

#### 20. 风险与注意事项

scope protocol 在三份 preset YAML 中有重复文本，未来可能漂移；以 schema field names、author/review rubric、negative fixtures 和每次 strict lint 作为最低闭环。不要通过新建“共享 prompt 文件”让 cross-project builtin 依赖源码 checkout。

---

## Verification Contract

### 8. Unit 串行依赖图

```text
Unit 1
  ↓ schema/event contract 已验证
Unit 2
  ↓ merge boundary 可选证据已验证
Unit 3
  ↓ postmerge direct/branch independent scope 已验证
Unit 4
  ↓ postmerge mixed/unknown hard gate 已验证
Unit 5
  ↓ redteam direct/no-boundary real base 已验证
Unit 6
  ↓ redteam mixed/conflict/attack gate 已验证
Unit 7
  ↓ operator/author/review/docs quality gate 已验证
最终全量门禁
```

- Unit 2 必须先于 Unit 3，因为 postmerge 的 boundary cross-check contract 需要先有可验证的 optional artifact；Unit 3 仍必须证明 no-boundary 能运行。
- Unit 3 必须先于 Unit 4，因为 mixed classification 只能扩展已验证的 direct/branch manifest/base，不得同时解决 baseline 和 override 两个问题。
- Unit 5 与 Unit 3 严格串行，虽然两者可以共享 D3/D5；Unit 5 必须在 postmerge 的 independent rule 已稳定后再次独立实现，防止 redteam 偷读 postmerge artifact。
- Unit 6 必须先于 Unit 7，author/review fixture 需要最终的 classification/confidence/reason 字段；Unit 7 不允许提前改变 runtime topology。
- 任一 Unit 未完成 Red → Green → Refactor → Integration → Regression → Close，不得开始下一 Unit；不得并行修改三个 preset 的同一行为并在最后统一补测试。

### 9. 执行命令清单

| 命令 | 运行时机 | 验证目的 | 预期结果 | 失败是否允许下一步 |
| --- | --- | --- | --- | --- |
| `cargo nextest run -p ralph-cli --bin ralph -- scope_handoff` | Unit 1 Red/Green | 验证现有 emit precheck 的 scope handoff consistency、tamper/drift、threshold 和 unsafe bypass | 篡改/越界/过期/低阈值 manifest 均拒绝 | 否 |
| `cargo nextest run -p ralph-cli --test integration_emit_policy -- scope` | Unit 1 Red/Green | 验证真实 emit policy/payload consistency 对 scope payload/artifact reference 的拒绝与接受 | 全绿 | 否 |
| `cargo nextest run -p ralph-core --test scenarios -- scope_payload_contract` | Unit 1 Red/Green | 验证三个 scope payload 的真实 EventLoop schema gate | 新 fixture 全绿 | 否 |
| `cargo nextest run -p ralph-core --test scenarios -- scope_agent_contract` | Unit 1 Red/Green | 验证 agent artifact characterization、blocked/resolved route 和重复 canonical bytes | 新 fixture 全绿 | 否 |
| `cargo nextest run -p ralph-core --test scenarios -- merge_batch_boundary` | Unit 2 | 验证 merge boundary success/incomplete route | 全绿，失败路径无 passed true | 否 |
| `cargo nextest run -p ralph-core --test multi_plan_scope_git -- direct_target` | Unit 3 | 在临时仓库复演 direct-target/base/HEAD/tree evidence | 原始 Git evidence 与预期边界一致 | 否 |
| `cargo nextest run -p ralph-core --test scenarios -- postmerge_scope_direct_target` | Unit 3 | 验证无 boundary direct target resolver handoff | `postmerge.changemap.ready` 被接受 | 否 |
| `cargo nextest run -p ralph-core --test multi_plan_scope_git -- mixed_history` | Unit 4 | 在临时仓库复演 interleaved/override/shared/rename/binary evidence | 分类输入与预期一致；unsupported binary 不被伪报为 line-level | 否 |
| `cargo nextest run -p ralph-core --test scenarios -- postmerge_scope_` | Unit 4 | 验证 mixed/blocked/drift/threshold | resolved 与 blocked 分支均符合 expected/absent events | 否 |
| `cargo nextest run -p ralph-core --test multi_plan_scope_git -- redteam_independent` | Unit 5 | 验证 redteam 使用独立 target lock/base 输入 | 不读取 postmerge/merge-boundary artifact | 否 |
| `cargo nextest run -p ralph-core --test scenarios -- redteam_scope_direct_target` | Unit 5 | 验证 redteam 无 boundary 的 direct-target real base handoff | `redteam.plan.resolved` 被接受且不含 placeholder | 否 |
| `cargo nextest run -p ralph-core --test scenarios -- redteam_scope_placeholder_blocked` | Unit 5 | 验证 placeholder/缺 base 的 unresolved path | `redteam.plan.unresolved` 被接受，`redteam.attack.mapped` absent | 否 |
| `cargo nextest run -p ralph-core --test multi_plan_scope_git -- redteam_mixed` | Unit 6 | 验证 redteam mixed/conflict/unknown 的独立 evidence replay | 分类冲突/unknown 与预期一致 | 否 |
| `cargo nextest run -p ralph-core --test scenarios -- redteam_scope_` | Unit 6 | 验证 mixed/conflict/unknown/threshold gate | attack mapped 只在 resolved 出现 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | 每个 preset schema/prompt 改动后 | ralph-cli preset lint | 通过 | 否 |
| `cargo nextest run -p ralph-core -- preset_lint` | 每个 schema/topology 改动后 | core preset lint/schema parity | 通过 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- presets` | Unit 1、7及最终 | embedded builtin parse/strict/parity tests | 通过 | 否 |
| `ralph preset check -H builtin:merge-batch --strict --format json` | Unit 2 | merge preset runtime contract | passed=true 且无新增 finding | 否 |
| `ralph preset check -H builtin:post-merge-converge --strict --format json` | Unit 3–4 | postmerge runtime contract | passed=true 且无新增 finding | 否 |
| `ralph preset check -H builtin:red-team-attack --strict --format json` | Unit 5–6 | redteam runtime contract | passed=true 且无新增 finding | 否 |
| `ralph emit --schema merge.integrated -H builtin:merge-batch` | Unit 2 | 查看 boundary required fields | 输出与 merge schema 一致 | 否 |
| `ralph emit --schema postmerge.changemap.ready -H builtin:post-merge-converge` | Unit 3–4 | 查看 scope fields/allowed values | 输出与 post schema 一致 | 否 |
| `ralph emit --schema redteam.plan.resolved -H builtin:red-team-attack` | Unit 5–6 | 查看 red scope fields | 输出与 red schema 一致 | 否 |
| `skills/.venv/bin/python -m pytest skills/ralph-preset-review/tests/test_skill_anchors.py -q` | Unit 7 | author/review anchor/fixture contract | 全绿 | 否 |
| `./scripts/check-cli-doc-drift.sh` | Unit 7及最终 | CLI/docs drift | 无输出且 exit 0 | 否 |
| `cargo build` | Unit 1、7及最终 | build.rs schema embed、全 workspace build | 通过 | 否 |
| `cargo clippy` | 最终前 | lint/type correctness | 无新增 warning/error | 否 |
| `./scripts/run-tests.sh` | 只有全部 Unit Close 后 | 仓库最终 nextest phase 1/2 + doctest | 全绿 | 否 |
| `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | 仅全量出现时序 flake | 串行 fallback 复核 | 通过才可完成 | 否 |

计划阶段不执行上述实现命令；执行阶段不得使用裸 `cargo test -p ralph-cli`。所有 Git scope fixture 必须在临时仓库中运行，禁止在当前主 checkout 执行 merge/rebase/reset 或改动 `.ralph` 运行时状态。

### 10. 最终质量门禁

- S1–S13 全部拥有真实 EventLoop/emit precheck acceptance test 或明确的 agent protocol/Git fixture acceptance evidence。
- R1–R14 全部在追踪矩阵中关联 Scenario、测试和 Evidence。
- 三个 preset 的 scope resolver 都能在 no-boundary/direct-target/mixed history 输入下独立启动。
- `merge-boundary` 只作为 optional evidence；任何缺失或冲突都不会使 postmerge/redteam 失去独立解析能力。
- `scope_base_sha`、manifest digest、patch digest、locked HEAD/tree 和 plan digest 都能重新计算；不存在 `<global-baseline>`、HEAD fallback 或未解释 placeholder。
- interleaved、shared、override、revert、unknown 分类均有 artifact path/count/证据；critical unknown、coverage 不足、confidence 不足和 drift 均 fail-close。
- `post-merge-converge` 不会在 scope unresolved 时触发审计/修复成功路径；`red-team-attack` 不会在 scope unresolved 时运行 attack-surface/experiment/finding。
- merge-batch 的 full verification 和 report terminal semantics 不改变；postmerge 的 closer 仍是独立终审 owner；redteam 仍只读代码树并等待人工确认 PLAN。
- `cargo nextest` preset/core/BDD、`cargo build`、`cargo clippy`、`./scripts/check-cli-doc-drift.sh`、author/review `.venv` tests 和最终 `./scripts/run-tests.sh` 全部通过。
- 没有新增 skipped/only/ignored 测试，没有削弱断言，没有无解释 snapshot/golden 更新，没有修改当前运行时 ledger。
- `presets/manifest.yml`、`presets/index.json`、zsh builtin set 与 `crates/ralph-cli/src/presets.rs` 保持一致且没有新增 preset。
- `crates/ralph-core/data/*.md` 复核后没有泄漏内部 ledger/function/plan-specific scope 实现；已有 emit/cmdref guide 只描述 agent 下一步动作、字段来源和失败停止条件；若 schema/命令说明漂移，已同步修复并通过 drift scan。
- 每个 Unit 均形成完整 TDD 闭环，严格按 Unit 1→7 顺序完成，所有关键决策置信度仍不低于 0.85。

### 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
| --- | --- | --- |
| 这是实施计划而不是 Roadmap 吗 | 是 | 每个 Unit 都绑定真实文件、入口、Red、最小实现、测试和停止条件 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D13 已确定；H2 只允许按 Unit 3–6 的固定 agent fixture 验证，不允许临时放宽 protocol/threshold |
| 所有文件和接口是否有代码库证据 | 是 | E2–E21；新增 fixture、既有 policy_check guard 修改、payload-consistency contract 和 `merge-batch` author note 均明确标为 planned addition 或已有入口 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1–D13 均在表中达到 0.91 或更高 |
| 是否存在未处理的低置信度假设 | 否 | H1/H2 已有验证方法和失败影响，不作为无条件事实使用 |
| 每个 Unit 是否只有一个可观察行为 | 是 | Unit 1 agent manifest/precheck gate、Unit 2 boundary、Unit 3 direct scope、Unit 4 mixed gate、Unit 5 red base、Unit 6 red confidence gate、Unit 7 review/doc gate |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 都有独立测试入口、命令、Red 和完成标准 |
| 每个 Unit 是否有真实 Red | 是 | 先用旧 payload/旧 gate/缺 artifact 的真实失败建立 Red；禁止文案 grep 充 Red |
| 每个 Unit 是否包含回归范围 | 是 | Unit 1–7 均列出直接、相邻、旧输入、构建和全量影响 |
| 是否存在未来 Unit 依赖 | 否 | 依赖图严格线性，当前 Unit 禁止实现未来分类/攻击/文档行为 |
| 是否存在泛化任务描述 | 否 | 使用具体 preset、hat、topic、artifact、字段、测试和命令 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | S1–S13 映射至 R、Unit、BDD/emit precheck/agent protocol contract 测试 |
| 所有关键决策是否有 Evidence | 是 | D1–D13 均引用 Evidence Ledger |
| 计划是否可以严格串行执行 | 是 | Unit 1→7 及每个 Unit 的 Red→Green→Refactor→Integration→Regression→Close 已固定 |

### 12. 计划范围外的明确调查结果

- 不新增 `multi-plan-converge` preset；现有三个 preset 已分别承载 merge、post-merge convergence 和 red-team attack 责任。
- 不把 `merge-batch` 变成 postmerge/redteam 的前置服务；它只补 machine-readable boundary evidence。
- 不增加 `ralph inspect scope resolve` 或任何新的 CLI surface；scope resolver 保持在 `change-mapper`/`plan-resolver` agent protocol 中，runtime 只通过现有 emit precheck/payload-consistency guard 检查交接一致性和阈值。
- 不把当前 branch 的 loop baseline marker 当作多计划 scope base；它是单 plan/loop persistence 辅助能力，不具备本需求所需的多计划 attribution 语义。
