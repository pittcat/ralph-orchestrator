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

- [x] P0/P1 阈值、scope digest、自洽 `proceed` 和上游 handoff 约束已写入 prompt/schema/runtime policy。
- [x] 每个跨 hat artifact path 都声明了来源、`.ralph/` 相对路径语义、结构化示例和 `test -f` 读取要求。
- [x] `redteam.complete.plan_path` 明确区分成功路径真实路径与失败路径空字符串。
- [x] `agent_skill_audit`：skipped（本次只评审并修复 preset YAML 与其 schema）。
