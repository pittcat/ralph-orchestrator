# red-team-attack 预设作者决策记录

本记录是 `presets/en/red-team-attack.yml` 的作者侧护栏确认，不是 loop 内 agent 的注入指南。

## Preset Intent Confirmation（2026-08-17 宿主/实验树契约）

- **Goal：** 在 loop **宿主 checkout** 保持只读的前提下，允许 experiment-runner 对每个 RTE 建独立 git worktree 做改代码攻击实验；证据仍落在宿主 `.ralph/red-team/**`；最终 PLAN.md 等人确认。
- **execution_model：** `single-chain`（isolated 单链；不加 supervisor / 不加 wave dispatcher）。用户确认「直接改」，未升级执行模型。
- **宿主：** 当前 loop 的 cwd（即使 `ralph run --worktree` 也算宿主）。HEAD/分支不变；只写 `.ralph/red-team/**`。
- **实验树：** `.worktrees/redteam-<RTE-id>/`，由 experiment-runner 从 lock HEAD `git worktree add --detach`；每个 RTE 一棵；可脏；hat 不 `worktree remove --force`。
- **成功条件：** 宿主 porcelain 在终态 emit 时除 `.ralph/red-team/**` 为空；`redteam.experiment.done` 携带真实 `experiment_worktree_path`；证据文件在宿主可读。
- **阻塞条件：** 宿主被改脏、实验树路径不是白名单或等于宿主 toplevel、未建树就伪造路径。
- **允许 mutation：** 仅实验树内代码/测试/配置；plan-resolver 只写 patch 文件，不 `git apply` 到宿主。
- **Non-goals：** runtime 不 auto reset/stash/clean/删实验树；不用整场 red-team 丢进一个 Ralph `--worktree` 代替实验隔离。
- **0d Gate Scope mode：** `record`（用户要求直接改、跳过交互菜单；不把 metric 当硬阻塞；既有 AAF/Payload/lint 仍生效）。
- **0e：** 不新增 `topic.workspace` 假 key；在真实 topic 上改 host 语义；`experiment.done` 追加实验树路径检查（checklist 第 6 项）。`payload_consistency` 无 path-prefix 算子，该位置不新增 consistency rule。
- **模板文件：** 已采用 `presets/templates/red-team-attack/`；本次只改 experiment 模板字段，不新增大段 instructions 模板。

## Gate Scope（初评，capability-triggered）

| Hat | Trigger reason | Applicable metrics | Evidence | Unverified assumptions | Critical ambiguities | Critical unverified assumptions | Mode | Decision |
|---|---|---|---|---|---|---|---|---|
| target-locker | key handoff（锁 HEAD） | Evidence Coverage, Impact Certainty, Verifiability | lock artifact + host porcelain precheck | 无 | 0 | 0 | record | pass |
| plan-resolver | artifact producer + key handoff + phase branching | Confidence, Evidence Coverage, Verifiability, Impact Certainty | scope manifest + patches 文件 | 无 | 0 | 0 | record | pass |
| attack-surface-mapper | artifact producer + key handoff | Evidence Coverage, Impact Certainty, Verifiability | 04/05 artifacts | 无 | 0 | 0 | record | pass |
| experiment-plan-validator | phase branching | Confidence, Evidence Coverage, Verifiability | validation report + host precheck | 无 | 0 | 0 | record | pass |
| experiment-runner | production mutation（实验树）+ artifact producer + key handoff | Evidence Coverage, Verifiability, Impact Certainty | worktree path + host porcelain + evidence artifacts | Mock verify 不能真建 worktree | 0 | 0 | record | pass |
| evidence-gate | multi-hat aggregation + phase branching | Confidence, Evidence Coverage, Verifiability | evidence board | 无 | 0 | 0 | record | pass |
| impact-boundary | artifact producer + key handoff | Evidence Coverage, Impact Certainty, Verifiability | PLAN.md | 无 | 0 | 0 | record | pass |
| independent-reviewer | phase branching | Confidence, Evidence Coverage, Verifiability | review artifact | 无 | 0 | 0 | record | pass |
| reporter | terminal authority | Confidence, Evidence Coverage, Verifiability | REPORT.md | 无 | 0 | 0 | record | pass |

## 设计确认

- 执行模型：`isolated` + Intent `execution_model: single-chain`。宿主代码树只读；实验 mutation 仅在 `.worktrees/redteam-<RTE-id>/`。
- 终态：成功路径由 `redteam.complete(success=true)` 表示；失败路径由 `redteam.complete(success=false)` 表示。
- 关键门禁：scope 阈值由 plan-resolver 明确计算，`proceed` 只在全部阈值满足时为 `true`；攻击面映射生成的计划必须先经过通用 `experiment-plan-validator`，最多重写 3 次。
- 证据原则：控制组、攻击组、原始证据、文件可读性和**宿主**干净工作树均为硬约束；实验 worktree 允许脏。

## 关键阶段与 guard 选择

```yaml
key_stages:
  - key_stage: "target-locker -> redteam.target.locked"
    guard_selection: precheck
    precheck_guard: true
    precheck_retry_budget: 1
    payload_consistency_guard: false
    payload_consistency_retry_budget: null
    reason: "宿主 HEAD/porcelain 需 LLM 对照 lock；锁定字段本身由 schema 校验。budget=1 沿用既有 YAML。"
    confirmation_status: confirmed
  - key_stage: "plan-resolver -> redteam.plan.resolved"
    guard_selection: payload_consistency
    precheck_guard: false
    precheck_retry_budget: null
    payload_consistency_guard: true
    payload_consistency_retry_budget: 3
    reason: "scope_status、proceed、阈值和边界字段必须保持结构一致。"
    confirmation_status: confirmed
  - key_stage: "attack-surface-mapper -> redteam.experiment.plan.ready"
    guard_selection: payload_consistency
    precheck_guard: false
    precheck_retry_budget: null
    payload_consistency_guard: true
    payload_consistency_retry_budget: 3
    reason: "必须通过 scope 阈值后进入通用计划可执行性校验。"
    confirmation_status: confirmed
  - key_stage: "experiment-plan-validator -> redteam.experiment.plan.valid|invalid"
    guard_selection: both
    precheck_guard: true
    precheck_retry_budget: 3
    payload_consistency_guard: true
    payload_consistency_retry_budget: 3
    reason: "只有项目发现证据支持的计划才能执行；无效计划回 mapper，最多重写 3 次。"
    confirmation_status: confirmed
  - key_stage: "experiment-runner -> redteam.experiment.done"
    guard_selection: precheck
    precheck_guard: true
    precheck_retry_budget: 3
    payload_consistency_guard: false
    payload_consistency_retry_budget: null
    reason: "实验完成依赖真实控制组、攻击组、宿主干净树、白名单实验 worktree 路径。无 path-prefix consistency 算子故不新增 payload_consistency。"
    confirmation_status: confirmed
  - key_stage: "evidence-gate -> redteam.experiment.next"
    guard_selection: payload_consistency
    precheck_guard: false
    precheck_retry_budget: null
    payload_consistency_guard: true
    payload_consistency_retry_budget: 3
    reason: "队列是否继续由 durable evidence board 与 remaining_count 结构化决定。"
    confirmation_status: confirmed
  - key_stage: "evidence-gate -> redteam.evidence.gated"
    guard_selection: both
    precheck_guard: true
    precheck_retry_budget: 3
    payload_consistency_guard: true
    payload_consistency_retry_budget: 3
    reason: "只有全部实验结清且至少一个实验满足硬门与四项阈值才能进入影响分析。"
    confirmation_status: confirmed
  - key_stage: "impact-boundary -> redteam.plan.ready"
    guard_selection: neither
    precheck_guard: false
    precheck_retry_budget: null
    payload_consistency_guard: false
    payload_consistency_retry_budget: null
    reason: "计划与影响边界由该 hat 的证据核验和产物可读性直接决定。"
    confirmation_status: confirmed
  - key_stage: "independent-reviewer -> redteam.reviewed"
    guard_selection: neither
    precheck_guard: false
    precheck_retry_budget: null
    payload_consistency_guard: false
    payload_consistency_retry_budget: null
    reason: "审查结果由独立读取的计划、证据板和审查产物决定。"
    confirmation_status: confirmed
  - key_stage: "reporter -> redteam.complete"
    guard_selection: payload_consistency
    precheck_guard: false
    precheck_retry_budget: null
    payload_consistency_guard: true
    payload_consistency_retry_budget: 3
    reason: "success、review verdict、report、plan 与 questions 的终态组合必须一致。"
    confirmation_status: confirmed
```

## 发布前检查

- 动态 success+failure：`presets/scenarios/red-team-attack-success.yml`；`ralph preset verify -H presets/en/red-team-attack.yml --scenario presets/scenarios/red-team-attack-success.yml --format json` → `passed=true`，含 `terminal-success-contract` 与 `producer-failure-to-complete-false`。
- 动态 no-output：`presets/scenarios/red-team-attack-no-output.yml`；同命令 → 总 `passed=false`、`failure_kind=no_progress`（locker 空输出，`no_progress_steps=1`）。这是异常路径证据，不是 success 合同失败。
- Prompt visibility（experiment-runner）：`ralph -c presets/en/red-team-attack.yml inspect prompt --hat experiment-runner --format json` → `ralph-tools-emit` 在 `on_demand`；instructions 先 `ralph tools skill load ralph-tools-emit`。
- [x] P0/P1 阈值、scope digest、自洽 `proceed` 和上游 handoff 约束已写入 prompt/schema/runtime policy。
- [x] 每个跨 hat artifact path 都声明了来源、`.ralph/` 相对路径语义、结构化示例和 `test -f` 读取要求。
- [x] `redteam.complete.plan_path` 明确区分成功路径真实路径与失败路径空字符串。
- [x] `agent_skill_audit`：skipped（本次只评审并修复 preset YAML 与其 schema）。
- Builtin 7 点同步（本次已做，review 复核）：schema SSOT、index.json 描述、presets.rs 描述、CLAUDE.md/AGENTS.md 列表句、zsh 描述、preset YAML、author notes。manifest 名称未改。

## 2026-08-17 宿主只读 + 实验 worktree

- 适用范围：五个高风险 hat 的 precheck **只验宿主 cwd**。experiment-runner 必须为每个 RTE 创建或复用 `.worktrees/redteam-<id>/`（`.worktrees` 已 gitignore，不污染宿主 porcelain）。
- 不实现 runtime 通用 workspace collector、自动 reset / stash / clean / delete / `worktree remove`。
- `redteam.experiment.done` 新增 required `experiment_worktree_path`；precheck checklist 第 6 项校验白名单且非宿主 toplevel；`by_check` `"4"`/`"5"`/`"6"`。
- Mock `presets/scenarios/*.yml` **不能**真的 `git worktree add`，因此实验树 mutation 真触发路径仍 BLOCKED。
- HARD RULE 3 仍禁止 agent 给 **Ralph loop** 私建 `--worktree`；本路径是 preset 白名单实验 scratch，instructions 已写明不得当作 loop reuse key。

## 2026-08-17-1841 plan U6 — workspace precheck 与 recovery guidance

- 适用范围：五个高风险 hat 在 entry/exit 自检 **宿主** workspace 状态：entry 写 `.ralph/red-team/<NN>-workspace-<hat>.md` 记录 `git status --porcelain=v1 --untracked-files=all` 快照；exit 写同样路径并比较；超出 `.ralph/red-team/**` 的宿主 tracked/untracked 修改一律视为越界。
- 不实现 runtime 通用 workspace collector、自动 reset / stash / clean / delete。
- 每个 hat 的 **真实终态 topic** 上的 precheck rule 附带 `recovery_guidance`；禁止 `topic.workspace` 幽灵 key。
- `by_check` key 与追加后的 1-based checklist 索引对齐（validator 为 `"3"`/`"4"`，runner 为 `"4"`/`"5"`/`"6"`）。
- Mock `presets/scenarios/*.yml` **不能**写入生产树 mutation，因此 tracked/untracked/ownership 真触发路径仍 BLOCKED。
- ownership 不明时停止而非 broad cleanup；agent-facing common 项强调禁止 `git restore` / `git checkout` / `git clean` / `git stash`。
- 仍属 builtin preset 行为变更：已同步 `presets/schemas/red-team-attack.yml`、`presets/index.json`、`crates/ralph-cli/src/presets.rs` description、`CLAUDE.md`/`AGENTS.md` 列表句。zsh 补全名称未变。
