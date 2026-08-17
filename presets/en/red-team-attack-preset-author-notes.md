# red-team-attack 预设作者决策记录

本记录是 `presets/en/red-team-attack.yml` 的作者侧护栏确认，不是 loop 内 agent 的注入指南。

## 设计确认

- 执行模型：`isolated`；所有 hat 只读代码树，实验原始证据和审计产物写入 `.ralph/`。
- 终态：成功路径由 `redteam.complete(success=true)` 表示；失败路径由 `redteam.complete(success=false)` 表示。
- 关键门禁：scope 阈值由 plan-resolver 明确计算，`proceed` 只在全部阈值满足时为 `true`；攻击面映射生成的计划必须先经过通用 `experiment-plan-validator`，最多重写 3 次。
- 证据原则：控制组、攻击组、原始证据、文件可读性和干净工作树均为硬约束。

## 关键阶段与 guard 选择

```yaml
key_stages:
  - key_stage: "target-locker -> redteam.target.locked"
    guard_selection: neither
    precheck_guard: false
    precheck_retry_budget: null
    payload_consistency_guard: false
    payload_consistency_retry_budget: null
    reason: "锁定结果由结构化字段与 HEAD/tree 校验决定，不增加主观阶段门。"
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
    reason: "实验完成依赖真实控制组、攻击组、原始证据和工作树完整性。"
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

- 动态 success 场景：`presets/scenarios/red-team-attack-success.yml`；运行 `ralph preset verify -H presets/en/red-team-attack.yml --scenario presets/scenarios/red-team-attack-success.yml --format json`，应接受 9 个业务事件并以 `redteam.complete` 收口。
- [x] P0/P1 阈值、scope digest、自洽 `proceed` 和上游 handoff 约束已写入 prompt/schema/runtime policy。
- [x] 每个跨 hat artifact path 都声明了来源、`.ralph/` 相对路径语义、结构化示例和 `test -f` 读取要求。
- [x] `redteam.complete.plan_path` 明确区分成功路径真实路径与失败路径空字符串。
- [x] `agent_skill_audit`：skipped（本次只评审并修复 preset YAML 与其 schema）。

## 2026-08-17-1841 plan U6 — workspace precheck 与 recovery guidance

- 适用范围：五个高风险 hat（target-locker、plan-resolver、attack-surface-mapper、experiment-plan-validator、experiment-runner）在 entry/exit 自检 workspace 状态：entry 写 `.ralph/red-team/<NN>-workspace-<hat>.md` 记录 `git status --porcelain=v1 --untracked-files=all` 快照；exit 写同样路径并比较；超出 `.ralph/red-team/**` 的 tracked/untracked 修改一律视为越界，由现有 precheck gate 拒绝并 retry。
- 不实现 runtime 通用 workspace collector、自动 reset / stash / clean / delete（plan D6 / R11 明确禁止）。
- 每个 hat 的 **真实终态 topic** 上的 precheck rule 附带 `recovery_guidance`：`target.locked` / `plan.resolved` / `experiment.plan.ready` 为独立 workspace rule；`plan.valid` / `plan.invalid` / `experiment.done` 把 workspace checklist **追加到已有语义 rule 的 prompt**（同一 topic 只能有一条 precheck rule；禁止 `topic.workspace` 幽灵 key）。
- `by_check` key 与追加后的 1-based checklist 索引对齐（validator 为 `"3"`/`"4"`，runner 为 `"4"`/`"5"`）。
- Mock `presets/scenarios/*.yml` **不能**写入生产树 mutation，因此 tracked/untracked/ownership 真触发路径仍 BLOCKED；不得把 instructions 里的 `git status` 当成该路径已验收。
- ownership 不明时停止而非 broad cleanup（plan R11；agent-facing common 项强调禁止 `git restore` / `git checkout` / `git clean` / `git stash`）。
- 失败路径走既有 `redteam.failed` 主题（每个 hat 已 declared on publishes），on_exhausted 携带 `failure_kind=workspace_precheck_failed`，由现有 target → bounded retry 链处理；runtime 不自动重置 / 自动删除文件。
- 仍属 preset-only 改动：未新增 builtin preset，未修改 manifest / index / zsh builtin 名称。
