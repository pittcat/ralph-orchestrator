---
title: Parallel Forge Reviewer Verifier Tester 只读权限与证据门禁 - Plan
type: feat
date: 2026-07-29
origin:
  - docs/achieved/brainstorms/2026-07-29-parallel-forge-wave-settlement-and-evidence-gates-requirements.md
  - docs/plans/2026-07-29-001-fix-parallel-forge-static-wave-settlement-plan.md
  - docs/plans/2026-07-29-002-feat-parallel-forge-reuse-status-plan.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan
execution: code
---

# Parallel Forge Reviewer Verifier Tester 只读权限与证据门禁 - Plan

## 0. 计划状态

- **状态：READY（受 001、002 完成门禁约束）**
- **代码基线：** `95fe5ff5ed9bb8991a0ec4b3948230fa28669447`
- **当前分支：** `pittcat-dev`
- **基线变更说明：**
  调查开始时
  `docs/plans/2026-07-29-002-feat-parallel-forge-reuse-status-plan.md`
  尚未跟踪；置信度检查时它已由并发工作提交为 `95fe5ff5`。本计划重新读取该提交，
  只引用其已落盘合同，不修改、删除或覆盖 002。
- **前置依赖：**
  1. `docs/plans/2026-07-29-001-fix-parallel-forge-static-wave-settlement-plan.md`
     的 Definition of Done 全部完成；
  2. `docs/plans/2026-07-29-002-feat-parallel-forge-reuse-status-plan.md`
     的 Definition of Done 全部完成；
  3. 实施前确认 001 实际落地的
     `forge.wave.reviewed`、`forge.wave.review.failed`、
     `forge.wave.verified`、`forge.wave.verification.failed`、
     `forge.full.verified`、`forge.full.verification.failed`
     topic、required fields、correction route 与本计划一致；
  4. 002 的 reuse checkpoint 不得绕过本计划的 Reviewer、Verifier、Tester
     只读门禁；`reverify`、`review`、`verification` checkpoint 进入对应 hat
     时必须重新建立当前 activation 的工作树基线。
- **调查范围：**
  Parallel Forge 三类 hat、P0/P1 计划合同、`HatConfig.disallowed_tools`、
  `allowed_write_paths`、backend tool policy、EventLoop 文件修改审计、
  scope violation 硬拒、payload consistency、precheck desugar、schema、
  artifact template embed/materialize、preset lint、真实 EventLoop BDD、
  agent skill 与 preset operator skill。
- **已执行的验证：**
  - 阅读当前 `presets/en/parallel-forge.yml` 与
    `presets/schemas/parallel-forge.yml`；
  - 阅读 001 的 per-wave topic、失败恢复和 read-only 决策；
  - 阅读 002 的 reuse checkpoint 与 Verifier/Tester 只读约束；
  - 阅读 `HatConfig` 的 `disallowed_tools`、`allowed_write_paths`；
  - 阅读 normal runner 与 supervisor wave worker 的 backend tool policy 接线；
  - 阅读 `EventLoop::audit_file_modifications`、scope violation severity 与
    `ScopeViolationHardRejected`；
  - 阅读 payload consistency 的白名单操作符与 same-payload 限制；
  - 阅读 precheck 配置、desugar、rejection retry 与 exhausted 语义；
  - 阅读现有 Parallel Forge template registry、materialize 测试与 BDD runner；
  - 阅读 Claude headless tool restriction 的已解决问题文档和相关 Git 历史。
- **本轮未执行的验证：**
  `ce-plan` 不运行 Acceptance Red、nextest、lint、build、E2E 或真实 backend
  实验；这些命令均列入各 Unit 的执行闭环。
- **阻塞项：** 无实施设计阻塞。001、002 尚未落地属于明确的串行前置依赖，
  不授权 Executor 提前修改 post-P0 topic 或猜测 post-P1 checkpoint 接口。

## 1. 功能目标

### 1.1 业务目标

让 Parallel Forge 的 Reviewer、Verifier、Tester 真正成为三个“只观察、记录、
下结论，但绝不修代码”的独立裁判。三者只有在代码树、Git 引用和证据 artifact
与 activation 开始时的基线一致时，才能发布成功或失败结论。验证失败必须进入
001 已定义的 `wave-fixer` correction 路径，不能由验证者偷偷修改测试或业务代码
后再宣布通过。

### 1.2 用户或调用方

- 操作者：需要可信的 per-wave review、incremental verification 和 final gate。
- Reviewer：审查当前 wave/correction 的 Unit 产物，只写 review evidence。
- Verifier：运行当前 candidate HEAD 的增量命令，只写 verification evidence。
- Tester：在全部 wave settled 后运行全量门禁，只写 final evidence。
- EventLoop：在 activation 前建立工作区快照，结束后独立比较。
- payload consistency：拒绝当前 payload 内自相矛盾的终态声明。
- synthetic precheck hat：打开 evidence、复跑只读 Git 命令并独立核验。
- `wave-fixer`：唯一承接普通 review/verification 失败后的代码修复。
- Integrator：只消费经过只读门禁接受的 verdict；本计划不改变其 Git 写权限。

### 1.3 当前行为

1. 当前 Reviewer 文案写了 “Read-only on code”，但没有 `disallowed_tools`，
   也没有机器级只读 evidence contract。
2. 当前 Verifier/Tester 文案要求运行测试和写报告，但没有明确禁止修改测试、
   业务代码、Git refs 或执行 merge。
3. `disallowed_tools` 只对 Claude backend 合并 `--disallowedTools`；其他 backend
   没有等价硬限制，仍需运行时审计兜底。
4. 当前 `audit_file_modifications` 只在 activation 结束后运行
   `git diff --stat HEAD`：
   - 能看到 tracked staged/unstaged 内容差异；
   - 看不到新提交后干净的工作树；
   - 看不到 branch/ref 切换；
   - 看不到 protected untracked 文件；
   - 不比较 activation 前已有 dirty state 与结束状态；
   - 只对 `dimension-reviewer`/`dim:*` 首次越权硬停止，普通
     Reviewer/Verifier/Tester 只增加失败计数。
5. `allowed_write_paths` 已存在于 `HatConfig` 和文档，但当前只被
   dimension reviewer lint 读取，不参与运行时 diff 过滤。
6. payload consistency 只检查同一 payload 的 literal comparison，不能读取
   Git、artifact 或历史事件，也不支持字段与字段比较。
7. precheck 可以独立读取 artifact、运行命令并拒绝 proposed event，但它发生在
   producer 已执行之后，不能替代工作树越权审计。

### 1.4 目标行为与行为差异

- 三个 hat 均配置 `disallowed_tools: ["Edit", "Write"]`，保留 `Bash` 以运行
  Git、测试、lint、build 和写 `.ralph/forge/...` evidence；任何通过 Bash、
  backend 特殊工具或 Git 命令产生的越权仍由 runtime snapshot 捕获。
- 三个 hat 均配置明确 `allowed_write_paths`，以 trigger 的 required `plan_key`
  展开唯一占位符 `{plan_key}`，只允许各自的
  `.ralph/forge/<plan-key>/...` evidence 目录；不允许源码、测试、配置、计划、
  branch、HEAD、index 或 Git operation state 改变。
  精确配置值固定为：
  - Reviewer：`.ralph/forge/{plan_key}/reviews/**`；
  - Verifier：`.ralph/forge/{plan_key}/verification/**`；
  - Tester：`.ralph/forge/{plan_key}/final-verification/**`。
  Executor 不得扩大为 `.ralph/**` 或 `.ralph/forge/**`。
- `EventLoop::build_prompt` 已持有本次 hat订阅的 `regular_events`；它在选择最近
  匹配 trigger时把完整 trigger payload保存为仅当前 activation有效的 transient
  context；目标三个业务 hat按需从中取得 required `plan_key`，其他不使用变量的
  strict hat不要求该字段。normal isolated runner在 prompt构建后、backend启动前调用
  `EventLoop::begin_workspace_mutation_guard` 建立 snapshot；结束后
  `EventLoop::process_output` 在处理终态收敛前比较 end snapshot。
- snapshot 比较 activation 前后状态，不把 activation 前已有 dirty state归责于
  当前 hat；但 Reviewer/Verifier/Tester 的业务 verdict仍要求
  `baseline_clean=true`，不允许在不可信基线上推进流程。
- 同时禁用 `Edit` 和 `Write` 的 hat 被识别为 strict read-only；第一次 protected
  mutation 即产生 `<hat>.scope_violation` 和
  `ScopeViolationHardRejected`，不得通过重试“洗白”。
- 三个 hat 的成功与失败 topic 全部挂 precheck。普通验证失败可以重做 evidence
  或进入 001 correction；实际 mutation 已由 runtime 直接停止，不交给 producer
  自我修复。
- precheck desugar生成的 gate hat本身固定为
  `disallowed_tools: ["Edit", "Write"]`、
  `allowed_write_paths: Some([])`，不拥有 business artifact写路径；它可运行
  checklist要求的只读 Git spot-check，但不能修复、重写 producer evidence或代码。
- 六个终态 payload 都携带统一只读 evidence 字段；payload consistency 拒绝
  “声称 guard 通过但 change count 非零/HEAD changed/Git operation 不 clean”
  等当前 payload 矛盾。
- 复杂字段表与命令证据表只维护在
  `presets/templates/parallel-forge/readonly-hat-evidence.template.md`，
  hat instructions 只引用模板和动作，不复制整张状态表。

### 1.5 本次需求

- **R1.** Reviewer、Verifier、Tester 都是 strict read-only；三者不得修改源码、
  测试、配置、计划、Git index、branch、HEAD 或开始/继续 merge、rebase、
  cherry-pick、revert、bisect。
- **R2.** 三者可以运行只读 Git/测试命令，并只写各自声明的
  `.ralph/forge/<plan-key>/...` evidence 文件。
- **R3.** runtime 必须在 activation 前后比较同一个 repository/worktree 的
  protected snapshot；不得把“结束时 `git diff HEAD` 为空”当作唯一通过条件。
- **R4.** snapshot 至少覆盖 HEAD SHA、symbolic branch/detached state、index、
  tracked worktree changes、protected untracked paths、Git operation state。
  对 synthetic precheck gate，baseline还必须解析 proposed trigger payload中所有
  repo-relative、以 `_path` 结尾的 string field，记录现存 regular file的 digest；
  end compare发现任一被审输入 artifact变化即越权。逃逸、symlink或非regular path
  fail closed。
- **R5.** activation 开始时已有的相同 dirty state不构成本次 scope violation；
  任何新增、删除或改变的 protected state构成越权。三个业务 hat的 precheck
  仍须因 `baseline_clean=false` 拒绝 verdict，直到操作者或有写权限的 owner
  在下一 activation前恢复可信基线。
- **R6.** `allowed_write_paths` 采用 repo-relative、正向 `/` 路径；只接受精确文件
  或目录前缀 `path/**` 两种模式，并只允许 `{plan_key}` 这一变量段。
  仅当某条 rule实际含 `{plan_key}` 时，runtime才在 backend启动前从当前 trigger
  required field取得 `plan_key`，校验为单一安全路径段后展开；字段缺失、包含
  `/`、`\`、`.`、`..` 或展开后逃逸时 baseline capture fail closed。空
  `allowed_write_paths` 的 synthetic gate不要求 trigger含 `plan_key`。绝对路径、
  其他占位符、空路径、仓库根、`.git/**` 在 strict lint 中报错。
- **R7.** 同时声明 `Edit` 和 `Write` 为 disallowed 的 hat 视为 strict
  read-only；无需增加第二个 `read_only` 配置字段。
- **R8.** strict read-only 第一次越权即硬停止，产生 typed
  `ScopeViolationHardRejected`；不自动 reset、checkout、clean 或删除用户文件。
- **R9.** Reviewer 的批准与拒绝都必须基于完整 current-wave review artifact；
  Reviewer 不能修复被拒 Unit。
- **R10.** Verifier 的通过与失败都必须基于真实 incremental command、exit code、
  candidate HEAD 和日志；Verifier 不能 merge 或修复。
- **R11.** Tester 的通过与失败都必须基于真实 full gate command、exit code、
  verified base HEAD 和日志；Tester 不能集成或修复。
- **R12.** 六个终态 topic 都必须包含统一 readonly evidence fields，并由 schema
  强制存在、类型正确。
- **R13.** payload consistency 必须拒绝当前 payload 中至少以下矛盾：
  `readonly_guard_passed=false` 却发布终态、`protected_change_count>0`、
  `baseline_clean=false`、`head_unchanged=false`、
  `git_operation_state!="clean"`、
  `readonly_evidence_path` 或 digest 为空。
- **R14.** payload consistency 不承担 start/end SHA 字段相等比较；独立 precheck
  必须打开 artifact 并把 payload、当前 Git 状态、trigger candidate/base SHA
  三方复核。
- **R15.** precheck 拒绝缺失、空、digest 不匹配、命令无 exit code、HEAD 锚点
  错误、allowed path 越界或 runtime mutation finding 未解释的 proposed event。
- **R16.** evidence 缺字段但 protected tree 未变时，precheck 最多退回 producer
  3 次补证；三次耗尽进入 `plan.blocked(kind=precheck_exhausted)`。
- **R17.** 真实 protected mutation 不进入上述 3 次补证预算，runtime 立即停止。
- **R18.** 普通 review/verification failure 在门禁通过后进入 001 的 failure
  handler/`wave-fixer`；三类只读 hat没有任何修复业务代码的出口。
- **R19.** 002 的 reuse/reverify checkpoint 必须重新建立 snapshot 和 evidence，
  不复用上一运行的 readonly verdict 作为当前 activation 的证明。
- **R20.** 模板必须在 binary-only 安装中可 materialize，并由结构化测试验证文件
  注册、字段章节和 parseable machine table；不得用整份 prompt 文本等值测试。
- **R21.** preset lint 必须阻止 strict read-only hat 缺少
  `allowed_write_paths`、配置非法模式或把 protected source roots加入白名单。
  `None` 表示合同缺失并报错；显式 `Some([])` 表示该 hat没有任何业务 artifact
  写权限，是 synthetic precheck gate 的合法配置。
  finding ID固定为：
  - `preset.strict_readonly_missing_write_contract`：双禁但字段为 `None`；
  - `preset.strict_readonly_invalid_write_path`：路径模式、变量、逃逸或protected
    root非法。
- **R22.** BDD 必须通过 `run_workflow_guard_scenario` 走真实 EventLoop；
  runtime Git mutation 另用真实 temp Git repo 的集成测试证明。
- **R23.** agent skill 与 preset operator skill 必须解释触发条件、应执行动作、
  evidence 字段来源和 mutation 停止条件；不得写入只适用于 003 的计划编号。
- **R24.** synthetic precheck gate 本身必须 strict read-only。通用 gate
  instructions 必须说明：deterministic runtime guard仍是权威，gate不得替代它；
  但当 rule checklist明确要求 evidence/Git spot-check时，gate必须执行只读复核，
  不能以“deterministic checks由其他层处理”为由跳过。gate不得改变 proposed
  payload引用的任何 `*_path` 输入 artifact。

### 1.6 输入

- active hat ID、current trigger payload 与 `HatConfig`；目标三个业务 hat的
  trigger必须含 required `plan_key`；
- activation 前 repository/worktree 路径；
- `allowed_write_paths`；
- Reviewer 的 current wave/correction trigger；
- Verifier 的 candidate branch/head 与 incremental command set；
- Tester 的 verified base HEAD 与 full gate command set；
- 001 的 failure/correction topic；
- 002 的 resume checkpoint；
- materialized readonly evidence template。

### 1.7 输出

- activation baseline snapshot 与 end comparison（runtime 内存状态）；
- 合法 evidence artifact：
  - `.ralph/forge/<plan-key>/reviews/...`
  - `.ralph/forge/<plan-key>/verification/...`
  - `.ralph/forge/<plan-key>/final-verification/...`
- 六个 proposed/accepted terminal payload 的 readonly evidence fields；
- mutation 时 `<hat>.scope_violation` 与 typed termination；
- precheck rejection 的稳定 `failed_checks` 和 `reason`；
- binary materialized `readonly-hat-evidence.template.md`。

### 1.8 状态变化

- 合法路径写入不改变 protected snapshot。
- 普通 verdict accepted 后按 001 flow 推进 review、integration、verification、
  correction 或 final gate。
- protected mutation 不接受最终业务 verdict，不推进 Integrator、settlement、
  Auditor 或 Reporter。
- 本计划不自动恢复被 mutation 污染的工作树；操作者根据 diff 决定恢复方式。

### 1.9 错误语义

| Reason code | 触发条件 | 结果 |
|---|---|---|
| `readonly_baseline_capture_failed` | activation 前无法读取 Git 状态 | 不启动该 hat，结构化阻塞 |
| `readonly_head_changed` | start/end HEAD 不同 | scope violation，立即停止 |
| `readonly_ref_changed` | branch/detached state不同 | scope violation，立即停止 |
| `readonly_index_changed` | index snapshot不同 | scope violation，立即停止 |
| `readonly_tracked_tree_changed` | tracked path/content state增量变化 | scope violation，立即停止 |
| `readonly_untracked_changed` | allowed roots 外 untracked path变化 | scope violation，立即停止 |
| `readonly_git_operation_changed` | merge/rebase/cherry-pick/revert/bisect 状态变化 | scope violation，立即停止 |
| `readonly_allowed_path_invalid` | 白名单绝对/逃逸/根/.git/非法 glob | strict preset lint error |
| `readonly_path_variable_invalid` | 配置使用 `{plan_key}` 但trigger缺字段，或值不是安全单路径段 | 不启动该 hat，结构化阻塞 |
| `readonly_evidence_missing` | payload path不存在或为空 | precheck reject，最多补证 3 次 |
| `readonly_evidence_digest_mismatch` | artifact digest与 payload不同 | precheck reject |
| `readonly_input_artifact_changed` | precheck activation前后任一 trigger `*_path` digest变化 | scope violation，立即停止 |
| `readonly_command_evidence_incomplete` | 命令、exit code、日志引用不完整 | precheck reject |
| `readonly_payload_contradiction` | consistency rule hit | emit reject_with_resume |
| `precheck_exhausted` | 三次补证仍失败 | `plan.blocked` |

### 1.10 兼容性要求

- 向后兼容不作为目标；P0 topic/schema 允许直接迁移到新 required fields。
- 未同时禁用 `Edit`、`Write` 的现有 hat 保持旧审计 severity，避免意外把有合法
  写职责的 Executor/Integrator/Fixer改成 hard reject。
- 不改变 `--continue`、supervisor slot terminal 或 `exec.unit.failed` 原生 fan-in。
- 不增加新的 CLI 命令、数据库或 feature flag。

### 1.11 性能要求

- 每个 strict read-only activation 只做一次 baseline 与一次 end snapshot。
- snapshot 只遍历 Git status/index/ref，不读取所有 tracked 文件内容；
  状态比较复杂度以 changed/untracked path 数量线性增长。
- snapshot 命令失败必须 fail closed，不能为节省开销跳过审计。
- 不在 precheck 中重复运行完整测试；precheck只复核命令日志、exit code、SHA、
  digest，并可运行有界只读 Git spot-check。

### 1.12 安全与权限要求

- path canonicalization 必须保证 allowed path留在 workspace，拒绝 symlink escape。
- `.git/**` 永远不允许由 `allowed_write_paths` 放行。
- 终止报告不得泄漏内部 event ledger 或 supervisor DB 路径。
- 不自动执行 destructive Git 命令。
- 非 Claude backend 即使没有 tool deny，也必须受到 snapshot guard。

### 1.13 本次范围

- Reviewer、Verifier、Tester 三个 hat。
- 通用 strict read-only workspace snapshot/audit。
- `allowed_write_paths` runtime semantics 和 lint。
- 六个 terminal topic 的 schema、consistency、precheck。
- 一个共享复杂 evidence template。
- real Git tests、real EventLoop BDD、docs/skills sync。

### 1.14 非目标

- 不收紧 Executor、Integrator、wave-fixer、Auditor、Reporter。
- 不解决 Integrator 冲突权限；001 已单独定义其指标与 correction。
- 不把 Verifier/Tester升级为 test stabilizer。
- 不增加路径型 Claude `allowedTools`/`disallowedTools`，已知 headless 不可靠。
- 不自动恢复 mutation。
- 不为全部 preset 一次性迁移 `allowed_write_paths`。
- 不修改 002 的 reuse evaluator 设计，只验证其 checkpoint不会绕过当前门禁。
- 不新增 UI。

### 1.15 已确认事实

- `HatConfig` 已有 `disallowed_tools` 和 `allowed_write_paths`。
- normal runner 和 supervisor wave worker都把 `disallowed_tools` 传给 backend
  policy；当前仅 Claude 有实际 CLI deny。
- EventLoop 已有 scope violation event、audit dispatcher 和 typed hard termination。
- 当前 hard termination 只识别 `dimension-reviewer`/`dim:*`。
- payload consistency 操作符只有 `eq`、`ne`、`gt`、`gte`、`exists`、
  `non_empty` 与 `all`/`any`，且只读取当前 payload。
- precheck 已支持 proposed/gate/rejected、三次 retry 和 exhausted。
- Parallel Forge template 已有 compile-time embed 与 CLI materialize 闭环。
- 001 已决定 Verifier/Tester 不写代码，普通失败由 `wave-fixer` 修复。
- 002 已决定 reverify checkpoint仍由只读 Verifier/Tester执行。

### 1.16 待验证假设

无进入实施的低置信度假设。001/002 的实际落地接口由“实施入口门禁”核对；如果
与计划合同不同，属于计划基线变化，必须停止并修订本计划，不允许 Executor自行
选一个近似 topic。

## 2. 代码库现状与证据

### 2.1 当前实现入口

```text
presets/en/parallel-forge.yml
  → RalphConfig::parse_yaml
  → HatConfig { disallowed_tools, allowed_write_paths }
  → loop_runner/runner.rs 选择 active hat
  → apply_hat_tool_policy（Claude CLI best-effort）
  → backend activation
  → isolated channel merge
  → EventLoop::process_output
  → audit_file_modifications（当前仅 git diff --stat HEAD）
  → event JSONL / EventPolicy
  → payload_consistency
  → precheck synthetic hat
  → accepted terminal topic
  → 001 review/integration/verification/correction flow
```

核心数据边界：

- 配置与 topic：preset/schema；
- 运行时基线：当前 workspace Git state；
- evidence：`.ralph/forge/<plan-key>/...`；
- agent 结论：JSON event payload；
- 独立复核：precheck synthetic hat；
- 测试：core unit、CLI/runner real Git integration、real EventLoop BDD。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `presets/en/parallel-forge.yml::reviewer` | 只有自然语言 “Read-only on code”，无 `disallowed_tools` | 必须补机器合同 | 高 |
| E2 | `presets/en/parallel-forge.yml::verifier/tester` | 可运行测试并写报告，但未禁止改测试/业务代码/Git | 三者需统一收紧 | 高 |
| E3 | requirements R47–R50、SC8 | Verifier 不 merge、Tester 不集成、结论需真实命令，修改权限不能模糊 | 明确 strict read-only 与修复 owner | 高 |
| E4 | 001 KTD/U5/U6 | post-P0 是 per-wave review→candidate integration→verify→settle；Verifier/Tester 不写代码 | 本计划绑定 post-P0 topic | 高 |
| E5 | 002 §1.11/route | reuse checkpoint仍要求 Verifier/Tester只读并重新验证 | reuse 不得复用旧 verdict绕过门禁 | 高 |
| E6 | `crates/ralph-core/src/config/hat.rs::HatConfig` | 已有 `disallowed_tools`、`allowed_write_paths` | 复用现有配置，不加 `read_only` DSL | 高 |
| E7 | `crates/ralph-cli/src/loop_runner/runner.rs` | normal activation 在 backend spawn 前可取得 active hat/config | 在此建立 baseline | 高 |
| E8 | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | wave worker也桥接 tool policy；三类目标 hat不作为 executor slot | 本计划不改 wave worker snapshot 路径 | 高 |
| E9 | `crates/ralph-adapters/src/tool_policy.rs` | 只有 Claude合并整工具 deny，其他 backend no-op | tool deny不能作唯一强制层 | 高 |
| E10 | `EventLoop::audit_file_modifications` | 当前结束时仅 `git diff --stat HEAD` | 必须改成 before/after snapshot | 高 |
| E11 | `TerminationReason::ScopeViolationHardRejected` | 已有 typed hard stop，当前文案/识别偏 dimension reviewer | 泛化 strict read-only，不新增终止类型 | 高 |
| E12 | `event_policy_payload_consistency.rs` | same-payload、literal-only、白名单操作符固定 | 用 derived bool/count，SHA equality交 precheck | 高 |
| E13 | `config/precheck.rs` 与 ce pipeline preset | 已有 proposed/gate/rejected、retry=3、exhausted | 复用现有 precheck runtime | 高 |
| E14 | `builtin_artifact_templates.rs`、`build.rs` | Parallel Forge templates compile-time embed并 materialize | 新模板接现有 registry | 高 |
| E15 | `integration_preset_materialize_artifacts.rs` | CLI真实物化和安全 plan-key已有测试 | 扩展结构化模板测试 | 高 |
| E16 | `crates/ralph-core/tests/scenarios.rs::run_workflow_guard_scenario` | Parallel Forge BDD可走真实 EventLoop | 拒绝 source-only/prompt 文本测试 | 高 |
| E17 | `preset_lint/dimension_reviewer_write_paths.rs` | `allowed_write_paths`当前仅 lint 且只覆盖 dim reviewer/docs plans | 新增通用 strict-readonly lint | 高 |
| E18 | tooling decision solution | Edit-only可被 Write绕过；Edit+Write会阻断正常 Write；路径型 Claude权限不可靠 | 保留 Bash+runtime audit，合法 evidence走白名单 | 高 |
| E19 | `docs/guide/configuration.md` | `allowed_write_paths`已是公开配置项，但无精确匹配语义 | 本计划必须补语义和 agent docs | 高 |
| E20 | project hard rules | preset/schema/topic变更需 lint、BDD、skills、docs与全量 nextest | U4闭合下游同步 | 高 |
| E21 | Git `6e19f28a`、scope audit历史 | 只读 reviewer越权已有真实事故和硬拒模式 | 优先复用 typed rejection | 中高 |
| E22 | `EventLoop::build_prompt`、`prepend_trigger_context` | isolated prompt构建时已持有 filtered `regular_events`，并按 hat triggers选择最近事件 | 同一 seam提取 required `plan_key`，避免 runner猜 trigger | 高 |
| E23 | `RalphConfig::apply_precheck_desugar`、`build_gate_instructions` | synthetic gate使用 `HatConfig::default()`，当前无工具限制；通用文案要求不检查deterministic Git evidence | gate自身需双禁，并修正文案避免与rule checklist冲突 | 高 |

### 2.3 受影响范围

**确认修改或新增：**

- 生产模块：
  - `crates/ralph-core/src/config/hat.rs`
  - `crates/ralph-core/src/config/ralph_config.rs`
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/event_loop/types.rs`
  - 计划新增 `crates/ralph-core/src/workspace_mutation_guard.rs`
  - `crates/ralph-core/src/lib.rs`
  - `crates/ralph-cli/src/loop_runner/runner.rs`
  - `crates/ralph-cli/src/builtin_artifact_templates.rs`
  - `crates/ralph-cli/build.rs`
- preset/schema：
  - `presets/en/parallel-forge.yml`
  - `presets/schemas/parallel-forge.yml`
- template：
  - 计划新增
    `presets/templates/parallel-forge/readonly-hat-evidence.template.md`
  - `presets/templates/parallel-forge/README.md`
- lint：
  - 计划新增 `crates/ralph-core/src/preset_lint/strict_readonly_hat.rs`
  - `crates/ralph-core/src/preset_lint/mod.rs`
  - `crates/ralph-core/src/preset_lint/finding_id.rs`
- tests：
  - `crates/ralph-core/src/event_loop/tests/audit_severity_ssot.rs`
  - `crates/ralph-cli/src/loop_runner/tests/legacy.rs`
  - `crates/ralph-cli/tests/integration_preset_materialize_artifacts.rs`
  - `crates/ralph-core/tests/scenarios.rs`
  - 计划新增三个 `parallel_forge_readonly_*_runtime.yml` scenario
- agent/operator docs：
  - `crates/ralph-core/data/ralph-tools-emit.md`
  - `crates/ralph-core/data/ralph-tools-opac.md`
  - `docs/guide/configuration.md`
  - `CONCEPTS.md`
  - `skills/ralph-preset-common/references/agent-native-model.md`
  - `skills/ralph-preset-common/references/author-checklist.md`
  - `skills/ralph-preset-common/references/finding-rubric.md`
  - `skills/ralph-preset-common/references/patterns.md`
  - `skills/ralph-preset-common/fixtures/aaf-review-negative-fixture.yml`
  - `AGENTS.md` 与 `CLAUDE.md`（仅同步 builtin 描述，且保持完全一致）

**确认不修改：**

- supervisor DB/schema/store；
- `exec.unit.*` slot terminal；
- `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` 的 worker snapshot；
- Integrator、Executor、wave-fixer权限；
- `presets/manifest.yml`、`presets/index.json`、`crates/ralph-cli/src/presets.rs`
  的 preset注册项和 `scripts/ralph-zsh-plugin.zsh` 的 builtin 名称补全；
  本计划不新增、删除或重命名 builtin preset/hat collection。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| KTD1 | 三个 hat 是否有修复权 | 可修测试；可修全部；严格只读 | 三者严格只读，修复只给 wave-fixer | E3–E5 | 自修破坏验证独立性；P0已有 correction owner | 0.99 |
| KTD2 | 如何标记 strict read-only | 新 `read_only` 字段；hat ID硬编码；Edit+Write双禁 | Edit+Write同时 disallowed即 strict read-only | E6,E10,E11,E18 | 新字段重复；ID硬编码不可复用 | 0.94 |
| KTD3 | 合法 evidence 如何放行 | 全 `.ralph/**`；prompt约定；静态路径；trigger变量展开 | 复用 `allowed_write_paths`，仅支持精确/`/**`与 `{plan_key}` 单变量展开 | E4–E6,E17,E19 | 全 `.ralph`过宽；静态路径无法隔离当前 plan；通用模板语言过度 | 0.94 |
| KTD4 | 如何识别 mutation | end diff；文件系统 watcher；before/after Git snapshot | activation前后 protected Git snapshot | E7,E10,E18 | end diff漏 commit/ref/untracked；watcher跨平台复杂 | 0.96 |
| KTD5 | dirty workspace如何处理 | 强制开始 clean；忽略 dirty；区分归责与可信度 | start/end相同的既有dirty不归责为scope violation，但业务verdict因baseline不clean被precheck拒绝 | E7,E10 | 强制把既有dirty归责给hat会误判；允许verdict又会污染验证 | 0.96 |
| KTD6 | mutation 后如何恢复 | 自动 reset；退回 producer；立即硬停 | 第一次立即 typed hard stop，不自动清理 | E11,E18 | reset破坏用户数据；producer无清理权限 | 0.98 |
| KTD7 | 哪些 verdict挂 precheck | 只失败；只成功；成功和失败 | 六个成功/失败 topic全部挂 | E3,E4,E13 | 只 gate 失败仍可无证据宣称成功 | 0.97 |
| KTD8 | consistency检查什么 | 比较所有外部事实；只做当前 payload自洽 | derived bool/count/enum自洽，外部事实交 precheck | E12,E13 | evaluator不支持 field-to-field或I/O | 0.99 |
| KTD9 | evidence表放哪里 | 每个 prompt复制；三个模板；一个共享模板 | 一个 `readonly-hat-evidence.template.md` | E14,E15,用户约束 | prompt复制会漂移；三模板重复同一合同 | 0.96 |
| KTD10 | 普通验证失败如何走 | 终止；原 hat自修；P0 correction | evidence gate通过后进入 001 wave-fixer | E4,E5 | 终止过早；自修越权 | 0.99 |
| KTD11 | runtime guard 放哪 | 只 process_output；只 runner；build_prompt记 trigger + runner capture + core compare | build_prompt保存当前 trigger payload；runner在 spawn 前调用 core capture并仅按需解析plan_key；process_output compare/terminate | E7,E10,E11,E22 | process_output单点无事前状态；runner单点拿不到可信 trigger且绕开事件审计 | 0.95 |
| KTD12 | 是否扩展 wave worker guard | 同时改；不改 | 本计划不改，目标三 hat走 normal isolated activation | E4,E8 | 扩展 executor slots超出范围且改变写角色 | 0.91 |
| KTD13 | 001/002 未落地是否阻塞计划 READY | 标 BLOCKED；允许无门禁提前做；设实施入口门禁 | READY但严格串行依赖，接口不符即停 | E4,E5,当前 Git | 设计已确定；提前实现会猜接口 | 0.95 |
| KTD14 | precheck gate自身权限 | 保持默认；允许写evidence；strict read-only | 所有 synthetic precheck gate双禁Edit/Write、`allowed_write_paths=Some([])`；对trigger `*_path`输入做digest guard；checklist只读复核 | E13,E18,E23 | 可写gate能篡改被审证据；Git snapshot看不到ignored artifact；gate无合法修复职责 | 0.98 |

所有执行关键决策均 ≥0.85，无待补证决策。

### 3.1 Snapshot 采集与比较合同

Executor 不得自行选择等价但语义不同的 Git 命令；U1 固定使用以下只读来源，并对
stdout按原始 bytes做 SHA-256（仓库已有 `sha2` 依赖）：

| 维度 | 采集方式 | 比较规则 |
|---|---|---|
| HEAD | `git rev-parse --verify HEAD` | full SHA逐字相等 |
| symbolic ref | `git symbolic-ref -q HEAD`；detached时记录固定标记 | start/end逐字相等 |
| index | `git diff --cached --raw -z --no-renames HEAD --` | 原始NUL输出digest相等 |
| tracked worktree | `git diff --raw -z --no-renames --` | 原始NUL输出digest相等 |
| untracked | `git ls-files --others --exclude-standard -z --` | path集合经normalize/allowed过滤后相等 |
| Git operation | 对 `MERGE_HEAD`、`CHERRY_PICK_HEAD`、`REVERT_HEAD`、`BISECT_LOG`、`rebase-merge`、`rebase-apply` 分别用 `git rev-parse --git-path <name>` 解析真实worktree git-dir位置；文件hash内容，目录按relative path排序后hash每个regular file的path+内容，其他类型fail closed | operation集合与digest相等且业务verdict要求两端均clean |
| precheck input artifacts | 从 proposed trigger顶层string `*_path`字段解析repo-relative regular file，拒绝escape/symlink；流式计算SHA-256 | 每个path的存在性、类型、digest相等 |

命令任一退出非零、输出无法解析、路径不是UTF-8时，不降级为“无变化”，统一返回
`readonly_baseline_capture_failed` 或对应 end-capture reason并停止。path matcher只做：

1. 把 `\` 拒绝而不是替换；
2. 拒绝 absolute、空segment、`.`、`..`、`.git`；
3. rule含 `{plan_key}` 时才从 trigger取安全单segment并展开；不含变量时不要求
   trigger提供该字段；
4. exact rule做完整path相等；
5. 以 `/**` 结尾的rule做目录边界前缀匹配，不能用普通字符串前缀误把
   `reviews-old/` 视为 `reviews/`。

### 3.2 六个 terminal topic 的公共 schema 合同

下表字段在 001 各 topic原有 required fields之外追加；不得用 optional field
替代。`sha256` 均为64位小写hex。

| 字段 | 类型/allowed value | 来源与填充规则 |
|---|---|---|
| `readonly_evidence_path` | non-empty repo-relative string | 当前hat角色专属allowed root内的模板实例 |
| `readonly_evidence_digest` | sha256 string | evidence文件最终bytes |
| `readonly_guard_passed` | boolean，accepted时必须true | producer自报；runtime mutation优先硬停，precheck独立复核 |
| `baseline_head` | full Git SHA | Reviewer=candidate head；Verifier=candidate head；Tester=verified base |
| `baseline_clean` | boolean，accepted时必须true | evidence baseline表；precheck用只读Git复核 |
| `baseline_protected_change_count` | non-negative integer，accepted时必须0 | baseline staged/tracked/protected-untracked计数 |
| `observed_head` | full Git SHA | producer结束前 `git rev-parse --verify HEAD` |
| `head_unchanged` | boolean，accepted时必须true | `baseline_head == observed_head` 的derived值 |
| `protected_change_count` | non-negative integer，accepted时必须0 | runtime start/end delta的producer记录 |
| `protected_changes` | array of repo-relative strings，accepted时必须empty | delta path/category摘要，不放长diff |
| `git_operation_state` | `clean`或`dirty`，accepted时必须`clean` | merge/rebase/cherry-pick/revert/bisect sentinel汇总 |
| `command_evidence_count` | positive integer | evidence表中含command、cwd、exit、log digest的行数 |

六个 precheck rule 的 checklist顺序固定，`failed_checks` 使用现有 runtime要求的
1-based string编号，不引入第二套命名协议：

1. evidence path存在、位于角色allowed root、模板章节完整；
2. evidence SHA-256与payload一致；
3. baseline/current/trigger HEAD及clean state一致；
4. command rows含cwd、exit code、log path/digest，数量与payload一致；
5. protected delta为零、allowed path没有越界、runtime没有scope finding；
6. verdict字段与review/command结果一致。

Reviewer、Verifier、Tester的rule可以在这六项下增加角色专属子检查，但不得重排或
改写1–6的含义；这样 rejection重试和模板版本间的 `failed_checks` 保持稳定。

## 4. BDD 行为规格

```gherkin
Feature: Parallel Forge 三类验证 hat 的只读权限与有据结论

  Background:
    Given 001 与 002 的全部 Definition of Done 已完成
    And Parallel Forge 使用 isolated execution 与 supervisor + wave
    And readonly evidence template 已物化

  Scenario S1: Reviewer 只写允许的 review evidence 后批准 current wave
    Given Reviewer activation 开始时已记录 protected workspace snapshot
    And current wave 每个 Unit 都有 completion artifact
    When Reviewer 只写 .ralph/forge/<plan-key>/reviews/** 并提出 forge.wave.reviewed
    Then runtime end snapshot 与 baseline 的 protected state 相同
    And payload consistency 不命中矛盾
    And precheck 复核 summary、digest、candidate HEAD 后接受 verdict
    And Integrator 只收到一条 forge.wave.reviewed

  Scenario S2: Reviewer 修改源码后不得批准
    Given Reviewer activation 已建立 baseline
    When Reviewer 修改 tracked source 并提出 forge.wave.reviewed
    Then runtime 发布 reviewer.scope_violation
    And loop 以 ScopeViolationHardRejected 终止
    And 不产生 accepted forge.wave.reviewed
    And Integrator 不激活
    And runtime 不自动回滚该修改

  Scenario S3: Reviewer 的普通拒绝进入 correction 而不是自修
    Given current wave 的一个 Unit 存在可复现审查缺陷
    When Reviewer 写完整 evidence 且 protected snapshot 未变
    Then forge.wave.review.failed 通过 consistency 与 precheck
    And 001 failure handler 创建 correction request
    And Reviewer 不修改被拒 Unit

  Scenario S4: Verifier 运行真实增量命令后通过
    Given candidate HEAD 与 forge.wave.integrated trigger 一致
    When Verifier 运行计划指定命令并记录 command、exit code、log digest
    And 只写允许的 verification evidence
    Then forge.wave.verified 经 precheck 接受
    And settlement 只针对相同 candidate HEAD

  Scenario S5: Verifier 修改测试使失败变绿时硬停止
    Given incremental test 初始失败
    When Verifier 修改 test file 后提出 forge.wave.verified
    Then protected test path变化被 runtime 捕获
    And verifier.scope_violation 首次即硬停止
    And candidate 不 promotion
    And 不进入三次 evidence 补证预算

  Scenario S6: Verifier 诚实报告失败并交给 wave-fixer
    Given incremental command 在未修改代码时退出非零
    When Verifier 写完整失败日志并提出 forge.wave.verification.failed
    Then precheck 接受真实失败 observation
    And wave-fixer 收到 correction
    And Verifier 不拥有修复出口

  Scenario S7: Tester 全量门禁通过后才触发 Auditor
    Given 所有计划 wave 已 settled
    And Tester baseline HEAD 等于最新 verified base
    When Tester 运行全量 nextest、lint、build并只写 final evidence
    Then forge.full.verified 经 precheck 接受
    And Auditor 只在 accepted event 后激活

  Scenario S8: Tester 修改业务代码或测试时不得发布结论
    Given Tester 已建立 baseline
    When Tester 修改 protected path并提出成功或失败
    Then tester.scope_violation 首次即硬停止
    And Auditor、Reporter、wave-fixer均不因该 proposed verdict激活

  Scenario S9: Tester 诚实的全量失败进入 final correction
    Given full gate 命令退出非零且 protected snapshot 未变
    When Tester 提出 forge.full.verification.failed 并附完整 evidence
    Then precheck 接受失败
    And 001 final correction route激活 wave-fixer
    And correction settled 后 Tester重新建立新 baseline并重跑全量门禁

  Scenario S10: 新提交、切分支或未跟踪源码不能绕过空 diff
    Given strict read-only hat activation 已建立 baseline
    When hat 通过 commit 让工作树恢复 clean
    Or 切换 symbolic branch
    Or 在 allowed roots 外创建 untracked source
    Then end snapshot 与 baseline 不同
    And runtime hard reject

  Scenario S11: activation 开始前已有 dirty state不归责于当前 hat但禁止 verdict
    Given baseline 已包含操作者的一个 protected dirty path
    When strict read-only hat不改变该 path并只写 allowed evidence
    Then runtime不把既有 dirty state误报为本轮 scope violation
    But precheck因 baseline_clean=false 拒绝 proposed verdict
    And 不激活下游业务 hat

  Scenario S12: 自报 clean 与真实 evidence 不一致时 precheck拒绝
    Given payload 声称 readonly_guard_passed=true
    And evidence 中的 end HEAD 或 digest 与当前 Git不一致
    When synthetic precheck 独立复核
    Then proposed verdict被拒绝
    And failed_checks包含稳定的 evidence mismatch名称

  Scenario S13: payload 自相矛盾在 precheck前被拒绝
    Given terminal payload 的 protected_change_count 大于 0
    And readonly_guard_passed=true
    When ralph emit --policy-check 验证 payload
    Then payload_consistency 拒绝该 event

  Scenario S14: 缺证据最多补三次
    Given protected tree未变但 readonly evidence缺少 exit code
    When producer连续提出不完整 verdict
    Then precheck每次退回同一 producer补证
    And 第三次耗尽后产生 plan.blocked kind precheck_exhausted

  Scenario S15: reuse checkpoint 不复用旧只读 verdict
    Given 002 输出 review 或 verification checkpoint
    When 当前运行激活 Reviewer、Verifier或Tester
    Then runtime建立当前 activation 的新 baseline
    And 当前 verdict引用当前 evidence digest
    And 上一运行的 readonly evidence只能作为历史输入

  Scenario S16: binary-only 安装能物化统一模板
    Given 操作者只有 ralph binary
    When 运行 ralph preset materialize-artifacts parallel-forge --plan-key demo
    Then templates目录包含 readonly-hat-evidence.template.md
    And 模板包含 protected snapshot、command evidence、verdict evidence三张表

  Scenario S17: synthetic precheck gate不能修改被审证据或代码
    Given producer已提出带 readonly evidence 的 terminal event
    When synthetic precheck gate尝试修改 producer evidence或protected source
    Then protected Git snapshot或trigger input artifact digest guard发现变化
    And gate自身触发 scope violation并硬停止
    And 不接受原 terminal event
    And gate只能通过只读命令完成 checklist spot-check
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
|---|---|---|---|---|---|
| S1–S3 | Reviewer合法写入放行、源码修改硬停、失败进入 correction | real Git runner + workflow BDD | 集成 | State-machine | 是 |
| S4–S6 | candidate SHA、真实命令、禁止自修 | workflow BDD + precheck | 集成 | Fault injection | 是 |
| S7–S9 | full gate、Auditor顺序、final correction重跑 | workflow BDD | 集成 | State-machine | 是 |
| S10–S11 | commit/ref/untracked/既有dirty delta | `workspace_mutation_guard` + runner | 单元+集成 | Property-style table | 否 |
| S12–S14,S17 | artifact mismatch、payload contradiction、3次耗尽、gate越权 | event policy/precheck tests | 单元+集成 | Mutation-style | 否 |
| S15 | reuse checkpoint重建 baseline | post-002 workflow BDD | 集成 | Differential resume/reuse | 是 |
| S16 | Reviewer首次消费并物化共享模板 | CLI materialize + Reviewer BDD | 集成 | Round-trip/parse | 否 |

每个测试必须同时断言：

- accepted/blocked topic；
- 下游 hat是否激活；
- protected Git state；
- allowed evidence副作用；
- scope violation/termination reason；
- 不自动 reset/clean；
- 无重复 accepted terminal。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1–R2 | 三 hat严格只读且可写 evidence | S1,S4,S7 | readonly happy paths | path matcher/snapshot | runner Git tests | S1/S4/S7 | E1–E11 |
| R3–R8 | before/after snapshot与首次硬停 | S2,S5,S8,S10,S11 | mutation reject | guard matrix | loop runner | S2/S5/S8 | E6–E11,E18 |
| R9–R11,R18 | verdict职责与 correction owner | S3,S6,S9 | failure routing | schema/policy | workflow BDD | S3/S6/S9 | E3–E5 |
| R12–R17 | schema、consistency、precheck | S12–S14 | emit/precheck rejection | payload evaluator | precheck runtime | S12 | E12,E13 |
| R19 | reuse不绕门禁 | S15 | checkpoint re-entry | snapshot reset | reuse workflow | S15 | E5 |
| R20–R21 | template与lint | S16,S1 | materialize/lint | matcher/lint | CLI preset tests | 否 | E14–E19 |
| R22–R24 | 真实路径测试、docs、只读precheck | S1–S17 | scenario suite | desugar/doc drift | full gate | mock E2E | E16,E20,E23 |

Scenario 到 Unit：

- S10、S11、S17 → U1；
- S1、S2、S3、S13、S14、S16 → U2；
- S4、S5、S6、S12 → U3；
- S7、S8、S9、S15 与跨 hat 回归 → U4。

## 7. 严格串行开发单元

```text
U1 strict read-only runtime guard
  ↓ 完成 Acceptance Red、Unit Red/Green、集成和回归
U2 Reviewer 只读 verdict 门禁
  ↓ 完成 Acceptance Red、Unit Red/Green、集成和回归
U3 Verifier 只读增量验证门禁
  ↓ 完成 Acceptance Red、Unit Red/Green、集成和回归
U4 Tester 只读全量门禁与跨运行闭环
```

### U1：建立 strict read-only 工作树 guard

#### 1. Unit 目标

任何同时禁用 `Edit`、`Write` 的 hat，在 activation 前后 protected Git state发生
增量变化时首次硬停止；只写 `allowed_write_paths` evidence时正常完成。

#### 2. 对应需求与 Scenario

- Requirement：R1–R8、R21、R24。
- Scenario：S10、S11、S17。
- Decision：KTD2–KTD6、KTD11、KTD14。
- Evidence：E6–E11、E17–E19、E22–E23。

#### 3. 外部可观察结果

- strict read-only hat commit、切 branch、改 index/source/test、创建 protected
  untracked file都会得到 `<hat>.scope_violation` 和 exit failure。
- activation前已有但未变化的 dirty state不误报。
- `.ralph/forge/...` 合法 evidence写入放行。

#### 4. 当前行为基线

现有 `audit_file_modifications` 只在结束时看 `git diff --stat HEAD`，且普通 hat
不是首次 hard reject；先以 real temp Git test固定“tracked working-tree change
会被发现、clean commit/ref/untracked会漏”的 characterization，再新增目标 Red。

#### 5. 输入与输出

- 输入：hat ID/config、workspace root、start/end Git snapshot。
- 输出：`WorkspaceMutationSnapshot`、delta、allowed/protected path分类、
  scope violation与 typed termination。
- 错误：capture命令失败、path规则非法、canonicalization逃逸均 fail closed。
- 状态：baseline只活在当前 activation，process_output后清除。
- 不变量：不修改 Git、用户文件或 evidence；不为非-strict hat升级 severity。

#### 6. 修改位置

- 计划新增 `crates/ralph-core/src/workspace_mutation_guard.rs`：
  snapshot DTO、Git采集、trigger `*_path` artifact digest、path normalize、
  allowed matcher、delta分类。
- `crates/ralph-core/src/config/hat.rs`：
  固化 `allowed_write_paths` 精确/`/**`和 `{plan_key}` 单变量语义文档。
- `crates/ralph-core/src/lib.rs`：
  注册 planned-new workspace guard模块；`ralph-cli` 只调用 EventLoop公开入口，
  不直接操作 snapshot DTO。
- `crates/ralph-cli/src/loop_runner/runner.rs`：
  prompt构建后、backend spawn前调用 EventLoop baseline capture；
  capture失败不启动 backend。
- `crates/ralph-core/src/event_loop/mod.rs`：
  在 `build_prompt` 选定最近匹配 trigger时保存 transient payload，
  提供 begin-guard入口，并用 snapshot delta替换 strict read-only 的旧
  `git diff --stat` 分支；非-strict保留旧路径。baseline在 process_output、
  capture失败或 activation放弃时清除，绝不跨 hat复用。
- `crates/ralph-core/src/event_loop/types.rs`：
  保留现有 termination variant，更新“仅 dimension reviewer”过窄说明。
- `crates/ralph-core/src/config/ralph_config.rs`：
  synthetic precheck HatConfig固定双禁Edit/Write与
  `allowed_write_paths: Some(vec![])`；修正
  `build_gate_instructions`，允许并要求执行rule checklist声明的只读
  evidence/Git spot-check，同时声明不能替代runtime guard。
- 计划新增 `crates/ralph-core/src/preset_lint/strict_readonly_hat.rs` 及
  `mod.rs`、`finding_id.rs`：
  检查双禁、allowed paths、非法/逃逸规则。
- 测试：
  `crates/ralph-cli/src/loop_runner/tests/legacy.rs`、
  `crates/ralph-core/src/event_loop/tests/audit_severity_ssot.rs`。

不修改 wave dispatcher、P0 topic 或三 hat instructions。

#### 7. 可依赖能力

- 现有 `HatConfig`；
- normal runner active hat seam；
- EventLoop audit/termination；
- temp Git repo测试模式。

#### 8. 禁止依赖的未来能力

- 不依赖 U2–U4 的具体 terminal topic。
- 不提前配置 Reviewer/Verifier/Tester。
- 不实现 Reviewer/Verifier/Tester 的业务 precheck rule或 correction route；
  仅加固所有 synthetic gate 的通用只读属性。
- 不把 Executor/Integrator变成 strict read-only。

#### 9. 验收测试

测试名/行为：

1. `strict_readonly_clean_commit_is_hard_rejected`
2. `strict_readonly_branch_switch_is_hard_rejected`
3. `strict_readonly_index_change_is_hard_rejected`
4. `strict_readonly_protected_untracked_change_is_hard_rejected`
5. `strict_readonly_existing_dirty_state_is_not_attributed_but_verdict_is_blocked`
6. `strict_readonly_allowed_evidence_write_is_allowed`
7. `strict_readonly_capture_failure_blocks_before_backend`
8. `synthetic_precheck_gate_is_strict_readonly`
9. `synthetic_precheck_rule_may_require_readonly_git_spotcheck`

运行：

```bash
cargo nextest run -p ralph-core -- workspace_mutation_guard
cargo nextest run -p ralph-cli --bin ralph -- strict_readonly
```

断言 protected state delta、topic、typed reason、backend未启动；不变量
是无自动清理且非-strict hat行为不变。

#### 10. Acceptance Red

先写 clean commit、branch switch、protected untracked 三个 real Git测试。当前
`git diff --stat HEAD` 为空或不含 untracked，预期没有 hard termination，测试因
“缺失 before/after snapshot”失败；这才是有效 Red。Git命令不可用、fixture未
初始化、测试未到 `process_output` 不算有效 Red。

#### 11. 单元测试拆分

1. strict识别：Edit+Write双禁=true；单禁/无禁=false。
2. path parser：exact、`/**`和安全 `{plan_key}`展开通过；absolute、`..`、
   root、`.git/**`、未知变量、unsafe plan key拒绝。
3. snapshot equality：相同既有 dirty集合为相等。
4. HEAD/ref/index/tracked/untracked/operation任一 delta分类稳定。
5. allowed evidence delta被过滤；protected delta保留。
6. capture error生成稳定 reason。
7. lint对无白名单、非法白名单、protected root白名单报稳定 finding ID。
8. precheck desugar生成双禁gate，且gate instructions不跳过rule要求的spot-check。
9. precheck trigger `*_path`输入的内容、symlink或类型变化都会产生artifact delta。

不允许 Mock Git delta算法的最终行为；单元层可 Fake command output，集成层必须
用真实 temp Git repo。

#### 12. Red → Green → Refactor 顺序

```text
clean commit / branch / untracked Acceptance Red
→ snapshot DTO + capture seam
→ delta Unit Red/Green
→ allowed path matcher Red/Green
→ typed hard-reject integration Green
→ existing dirty characterization Green
→ lint Red/Green
→ precheck gate权限/通用文案 Red/Green
→ Refactor为单一 snapshot/delta SSOT
```

#### 13. 最小实现范围

- 实现 before/after snapshot、delta、allowed filtering与 strict hard reject。
- snapshot只存必要 Git事实，不保存文件内容。
- 使用现有 termination reason。
- 不增加 CLI命令、DB或后台 watcher。

#### 14. 集成验证

联合真实 runner、EventLoop、Git repo。Claude tool deny可单元
验证参数合并，但 correctness必须由 non-Claude-compatible runtime snapshot测试
证明。上述命令全部通过方可进入 U2。

#### 15. 风险驱动测试

- Characterization：旧 tracked diff审计仍能发现修改。
- Property-style：每个 snapshot维度单独变化都产生 delta。
- Fault injection：`git rev-parse`、status、index capture失败。
- Security：symlink/path traversal/`.git/**`白名单拒绝。
- Idempotency：同一 baseline只消费一次，下一 activation建立新 baseline。

#### 16. 回归范围

- dimension reviewer首次 hard reject；
- 非-strict executor/fixer旧审计 severity；
- backend tool policy；
- isolated loop output处理；
- preset lint现有 findings；
- summary/display对 termination reason的文案。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/workspace_mutation_guard.rs` | 新增生产/单测 | snapshot SSOT | E7,E10 |
| `crates/ralph-core/src/config/hat.rs` | 修改生产/文档 | path/变量语义 | E6,E19 |
| `crates/ralph-core/src/lib.rs` | 修改生产 | 注册guard模块 | E7,E10 |
| `crates/ralph-cli/src/loop_runner/runner.rs` | 修改生产 | pre-spawn baseline | E7 |
| `crates/ralph-core/src/event_loop/mod.rs` | 修改生产 | end compare/hard reject | E10,E11 |
| `crates/ralph-core/src/event_loop/types.rs` | 修改文档 | 泛化 typed reason | E11 |
| `crates/ralph-core/src/config/ralph_config.rs` | 修改生产/单测 | gate自身只读/spot-check语义 | E23 |
| `crates/ralph-core/src/preset_lint/strict_readonly_hat.rs` | 新增生产/单测 | config fail-fast | E17 |
| `crates/ralph-core/src/preset_lint/{mod.rs,finding_id.rs}` | 修改生产 | lint注册 | E17 |
| `crates/ralph-cli/src/loop_runner/tests/legacy.rs` | 修改测试 | real Git guard | E10 |

#### 18. 完成标准

当前 Scenario、unit、real Git integration、相关 regression、
build/lint/typecheck均通过；无 skip/only/断言削弱；strict与non-strict边界清晰；
Evidence/KTD未下降；U1可独立提交。

#### 19. 停止条件

若 `build_prompt` 无法确定唯一的最近匹配 trigger/plan_key、runner在 backend
spawn前无法取得唯一 active hat/workspace、snapshot需要新依赖，或
`allowed_write_paths` 实际有其他 runtime消费者，停止并更新 Evidence/KTD；
不得硬编码三个 hat ID或放宽为整个 `.ralph/**`。

#### 20. 风险与注意事项

- 风险：合法 evidence被误判。触发：path normalize漂移。检测：exact/`/**`矩阵。
  缓解：单一 matcher + lint。剩余风险：symlink race，使用 canonical parent检查。
- 风险：baseline留存跨 activation。检测：连续两次不同 hat测试。缓解：消费即清。
- 风险：Git命令慢。检测：1000 changed paths基准性测试。缓解：只采 status/ref/index。

### U2：用共享 evidence template 建立 Reviewer 只读 verdict 门禁

#### 1. Unit 目标

Reviewer 从 binary materialized共享 template产出 current wave/correction evidence；
只有 evidence完整且 protected tree未变时才能发布批准或拒绝；拒绝只进入
correction，Reviewer不修代码。

#### 2. 对应需求与 Scenario

- Requirement：R1–R2、R9、R12–R18、R20–R24。
- Scenario：S1、S2、S3、S13、S14。
- Decision：KTD1、KTD7–KTD10。
- Evidence：E1,E3,E4,E12–E20。

#### 3. 外部可观察结果

批准后 Integrator激活一次；合法拒绝后 failure handler/wave-fixer激活；源码 mutation
立即硬停；无 evidence或自相矛盾 payload不能生成 accepted verdict。

#### 4. 当前行为基线

当前 Reviewer在全部开发完成后自由格式审查，只有 prompt read-only声明。001 会把
它改成 per-wave topic，但只保证拓扑，不提供本计划的 runtime snapshot与成功
precheck。以 post-001 happy/failure BDD为 characterization，再添加 readonly Red。

#### 5. 输入与输出

- 输入：`exec.wave.complete`/correction trigger、wave/unit IDs、candidate HEAD、
  completion reports。
- 输出：post-001 两个 Reviewer terminal及统一 readonly fields。
- evidence：review summary + shared readonly evidence。
- 错误：coverage不足、digest/SHA错误、mutation、payload contradiction。
- 不变量：一个 activation一条业务 event；Reviewer不调用修复/merge。

统一 readonly required fields：

`readonly_evidence_path`、`readonly_evidence_digest`、
`readonly_guard_passed`、`baseline_head`、`baseline_clean`、
`baseline_protected_change_count`、`observed_head`、
`head_unchanged`、`protected_change_count`、`protected_changes`、
`git_operation_state`、`command_evidence_count`。

#### 6. 修改位置

- `presets/en/parallel-forge.yml`：Reviewer双禁、allowed review roots、模板步骤、
  success/failure precheck与 consistency。
- `presets/schemas/parallel-forge.yml`：两个 Reviewer topic required fields/docs。
- 计划新增
  `presets/templates/parallel-forge/readonly-hat-evidence.template.md`，固定：
  activation identity、baseline/end snapshot、allowed/protected delta、
  command/exit/log evidence、verdict/digest、precheck checklist六部分。
- `presets/templates/parallel-forge/README.md`、
  `crates/ralph-cli/build.rs`、
  `crates/ralph-cli/src/builtin_artifact_templates.rs`：
  注册、embed并materialize共享模板。
- `crates/ralph-cli/tests/integration_preset_materialize_artifacts.rs`：
  真实CLI物化与parseable table验证。
- `crates/ralph-core/src/preset_lint/strict_readonly_hat.rs`：
  Reviewer配置结构化 lint覆盖。
- 计划新增
  `crates/ralph-core/tests/scenarios/parallel_forge_readonly_reviewer_runtime.yml`
  并在 `scenarios.rs` 注册。
- payload/precheck现有单元测试按新 topic增加结构化 cases。

#### 7. 可依赖能力

U1 guard/template；001 per-wave review/correction；002 checkpoint只作为 trigger输入。

#### 8. 禁止依赖的未来能力

不依赖 U3/U4；不让 Reviewer运行 Verifier/Tester全套命令；不提前收紧其他 hat。

#### 9. 验收测试

1. allowed evidence + approved → accepted `forge.wave.reviewed`。
2. complete per-unit verdict coverage但有拒绝 → accepted
   `forge.wave.review.failed` → correction。
3. tracked mutation + success proposal → scope hard stop。
4. `protected_change_count=1` + guard true → payload consistency reject。
5. evidence digest mismatch → precheck reject。
6. 三次 evidence缺字段 → precheck exhausted。
7. binary-only materialize输出共享模板，Reviewer复制后按字段填写。

```bash
cargo nextest run -p ralph-core -- payload_consistency
cargo nextest run -p ralph-core -- precheck
cargo nextest run -p ralph-core --test scenarios -- parallel_forge_readonly_reviewer
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-cli --test integration_preset_materialize_artifacts -- parallel_forge
```

#### 10. Acceptance Red

post-001 scenario中让 Reviewer提出成功但省略 readonly fields；当前 schema/precheck
会接受或只因未知字段合同失败，目标断言“必须拒绝且 Integrator不激活”形成正确
Red。YAML parse错误或缺 post-001 topic不算有效 Red，必须先满足实施入口门禁。

#### 11. 单元测试拆分

1. Reviewer两个 topic schema都要求完整 readonly fields。
2. success/failure consistency规则对 false/nonzero/nonclean命中。
3. legal zero/true/clean不命中。
4. precheck missing/digest/head/coverage分别返回稳定 failed_checks。
5. lint要求双禁和唯一 evidence roots。
6. topic deny禁止 Reviewer发布 integration/verification/correction done。

不 Mock summary coverage；BDD必须真实读取 fixture artifact。

#### 12. Red → Green → Refactor 顺序

```text
missing readonly schema Red
→ schema + Reviewer config Green
→ contradiction Red
→ consistency rules Green
→ evidence mismatch Red
→ success/failure precheck Green
→ template materialize/parse Red/Green
→ correction routing Green
→ Reviewer mutation real Git Green
→ Refactor共享 prompt片段为模板引用
```

#### 13. 最小实现范围

只改变 Reviewer配置、两个 topic合同、共享模板与测试。precheck success失败都
独立打开 summary与 readonly artifact；失败 evidence合格后仍走001 correction。
不得在 Reviewer instructions中复制整张表或加入任何修复命令。

#### 14. 集成验证

真实 EventLoop验证 proposed/gate/final、single business event、Integrator/correction
互斥路由；real Git runner证明 mutation硬停；真实CLI证明binary-only模板可用。
全部命令绿方可进入 U3。

#### 15. 风险驱动测试

- State-machine：success、failure、rejected三出口互斥。
- Mutation-style：逐个翻转 readonly field。
- Idempotency：重复 proposed只接受一个 final。
- Fault injection：summary missing/unreadable/digest mismatch。

#### 16. 回归范围

P0 per-wave fan-in、Reviewer coverage、Integrator trigger、correction round、precheck
desugar、topic ownership、schema parity、single event budget、reuse review checkpoint。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改配置 | Reviewer权限/门禁 | E1,E4 |
| `presets/schemas/parallel-forge.yml` | 修改schema | readonly fields | E4,E12 |
| `presets/templates/parallel-forge/readonly-hat-evidence.template.md` | 新增模板 | 复杂表SSOT | E14,用户约束 |
| `presets/templates/parallel-forge/README.md` | 修改文档 | 模板目录 | E14 |
| `crates/ralph-cli/{build.rs,src/builtin_artifact_templates.rs}` | 修改生产/单测 | embed/register | E14 |
| `crates/ralph-cli/tests/integration_preset_materialize_artifacts.rs` | 修改测试 | binary物化 | E15 |
| `crates/ralph-core/tests/scenarios/parallel_forge_readonly_reviewer_runtime.yml` | 新增BDD | S1–S3 | E16 |
| `crates/ralph-core/tests/scenarios.rs` | 修改测试注册 | real runner | E16 |
| payload/precheck/preset lint现有测试 | 修改测试 | consistency/gate | E12,E13,E17 |

#### 18. 完成标准

Reviewer happy/reject/mutation/补证耗尽与binary template物化全绿；Integrator和
wave-fixer互斥；schema/preset strict lint绿；build/lint/typecheck/回归绿；
无文本锁定测试；U2可独立提交。

#### 19. 停止条件

若001实际 Reviewer topic、failure target或summary字段不同，停止并修订本计划；
若 precheck success topic导致 flow ownership冲突，不得绕过 gate，必须修正
post-001 flow/schema后重新决策。

#### 20. 风险与注意事项

- 风险：success precheck增加 activation/hop。检测：workflow activation lint。
  缓解：复用 synthetic hat豁免与现有 hop模型。
- 风险：Reviewer shell写 evidence同时可写源码。检测：U1 snapshot。
  剩余风险：修改已发生但不会被自动恢复，报告明确 diff供操作者处理。

### U3：为 Verifier 建立只读增量验证门禁

#### 1. Unit 目标

Verifier 真实运行 candidate HEAD 的增量命令，只记录并发布结果；不得 merge、
改测试或改业务代码。普通失败交给 wave-fixer。

#### 2. 对应需求与 Scenario

- Requirement：R1–R2、R10、R12–R19、R22–R24。
- Scenario：S4、S5、S6、S12。
- Decision：KTD1、KTD7、KTD8、KTD10。
- Evidence：E2–E5、E10–E13、E18。

#### 3. 外部可观察结果

candidate SHA与 evidence锚定一致才接受 pass/fail；修改 test/source或HEAD时立即硬停；
诚实的非零退出进入 correction，candidate不 promotion。

#### 4. 当前行为基线

当前 Verifier从一次性 `forge.integration.done`运行增量回归并可直接 `work.failed`；
001 改为 per-wave candidate验证和 failure observation，但只读仍是声明。先固定
post-001 SHA/promotion行为，再写 mutation与evidence Red。

#### 5. 输入与输出

- 输入：candidate branch/head、wave identity、incremental commands。
- 输出：`forge.wave.verified` 或 `forge.wave.verification.failed` proposed/final。
- evidence：每条命令、cwd、start/end时间、exit code、log path/digest、candidate SHA。
- 不变量：不运行 merge/rebase/cherry-pick；不修改 tests/source；失败不 promotion。

#### 6. 修改位置

- `presets/en/parallel-forge.yml`：Verifier双禁、allowed verification root、
  只读命令合同、两个 precheck。
- `presets/schemas/parallel-forge.yml`：两个 topic readonly fields + command fields。
- 计划新增
  `crates/ralph-core/tests/scenarios/parallel_forge_readonly_verifier_runtime.yml`
  与 `scenarios.rs` 注册。
- U1 real Git runner tests增加 verifier test-file mutation case。

#### 7. 可依赖能力

U1 guard/template、U2成功/失败 gate模式、001 candidate integration/correction、
002 reverify checkpoint。

#### 8. 禁止依赖的未来能力

不依赖 Tester；不运行全量 final gate；不修改 Integrator；不创建 stabilizer。

#### 9. 验收测试

1. candidate HEAD + exit 0 + complete logs → accepted verified。
2. exit nonzero + no mutation → accepted verification failed → wave-fixer。
3. 修改 test file后exit 0 → hard stop/no promotion。
4. commit test fix后clean tree → HEAD delta hard stop。
5. payload HEAD声称一致但 evidence/current Git不一致 → precheck reject。
6. reuse `reverify` checkpoint建立新 baseline。

```bash
cargo nextest run -p ralph-core --test scenarios -- parallel_forge_readonly_verifier
cargo nextest run -p ralph-cli --bin ralph -- strict_readonly
cargo nextest run -p ralph-core -- precheck
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
```

#### 10. Acceptance Red

在 temp repo让 Verifier mock backend修改测试并commit，再提出 pass。当前 end diff为空，
预计 promotion仍可能推进；目标断言 hard termination/no promotion失败，证明测试
覆盖 clean-commit绕过。若命令根本没执行或 candidate fixture错误，不是有效 Red。

#### 11. 单元测试拆分

1. command evidence空列表/缺exit code/缺digest拒绝。
2. success要求所有 exit code=0。
3. failure要求至少一条非零/timeout且failed_commands对应。
4. candidate head三方一致：trigger、artifact、current Git。
5. consistency拒绝 change count/head unchanged/git state矛盾。
6. topic deny拒绝 Verifier发布 integration/correction done。

真实命令 exit/log合同不得完全 Mock；BDD fixture至少执行/模拟 runner可观察的命令
结果，Git mutation集成必须真实。

#### 12. Red → Green → Refactor 顺序

```text
clean commit mutation Red
→ Verifier strict config + U1 guard Green
→ command evidence Red/Green
→ success precheck Red/Green
→ failure precheck/correction Red/Green
→ candidate SHA三方 mismatch Red/Green
→ reuse reverify Green
→ Refactor Reviewer/Verifier共享 consistency生成结构
```

#### 13. 最小实现范围

Verifier只执行 execution/development plan列出的增量命令；evidence artifact按共享模板
填写。失败通过后交001 failure handler。不新增测试自动修复、不允许更新 snapshot/
golden来适配错误行为。

#### 14. 集成验证

联合 post-001 candidate branch、precheck、promotion guard、wave-fixer route和
post-002 reverify checkpoint。断言 verifier pass前正式 integration branch不移动。

#### 15. 风险驱动测试

- Fault injection：timeout、signal、missing log、invalid UTF-8摘要。
- State-machine：fail→correction→review→integrate→reverify。
- Differential：uninterrupted与reuse reverify结果相同。
- Mutation：测试文件修改、commit、branch switch。

#### 16. 回归范围

candidate base/head、promotion、settlement、failure fingerprint、correction round、
incremental commands、reuse checkpoint、precheck和schema parity。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改配置 | Verifier只读/门禁 | E2,E4 |
| `presets/schemas/parallel-forge.yml` | 修改schema | command/readonly fields | E4,E12 |
| `crates/ralph-core/tests/scenarios/parallel_forge_readonly_verifier_runtime.yml` | 新增BDD | S4–S6,S12 | E16 |
| `crates/ralph-core/tests/scenarios.rs` | 修改测试注册 | real EventLoop | E16 |
| `crates/ralph-cli/src/loop_runner/tests/legacy.rs` | 修改测试 | clean commit/test mutation | E10,E18 |

#### 18. 完成标准

pass/fail/mutation/SHA mismatch/reuse reverify全部验证；失败只到wave-fixer；正式 branch
promotion门禁不变；targeted/build/lint/typecheck/回归绿；U3可独立提交。

#### 19. 停止条件

若001把 Verifier放入 supervisor wave worker而非 normal isolated activation，停止并
将 U1 baseline seam扩展到 dispatcher后重新评估 KTD12；不得只靠 precheck掩盖
缺失runtime guard。

#### 20. 风险与注意事项

- 风险：测试工具自身写 tracked cache/snapshot。触发：命令产生 repo内文件。
  检测：snapshot delta。缓解：计划命令必须先修正为不写或明确由 Executor处理；
  不能把源码根加入白名单。
- 风险：timeout后子进程继续写。检测：end capture前确保 backend子树已终止；
  若无法保证，停止并扩大 runner lifecycle调查。

### U4：为 Tester 建立只读全量门禁并闭合跨运行验证

#### 1. Unit 目标

Tester 只在所有 wave settled后对当前 verified base执行全量门禁；成功才触发
Auditor，失败只进入 final correction；同时闭合 Reviewer/Verifier/Tester 的
schema、BDD、agent docs和全量回归。

#### 2. 对应需求与 Scenario

- Requirement：R1–R2、R11–R24。
- Scenario：S7、S8、S9、S15及S1–S17跨链回归。
- Decision：KTD1、KTD7–KTD10、KTD13。
- Evidence：E2–E5、E12–E20。

#### 3. 外部可观察结果

full gate pass后只有一个 Auditor activation；诚实失败进入 final correction并在
settled后重跑；任何 Tester mutation硬停；reuse路径重新验证，不复用旧 gate。

#### 4. 当前行为基线

当前 Tester从一次性 incremental verified触发全量门禁并可直接 `work.failed`。
001改为 all-waves-settled后 final gate与correction重跑；002增加reuse checkpoint。
先用两计划的 final route测试作为 characterization，再加入只读和docs gate。

#### 5. 输入与输出

- 输入：`forge.exec.development.done`/`forge.final.correction.settled`、
  settled counts、verified base、full commands。
- 输出：`forge.full.verified` 或 `forge.full.verification.failed`。
- evidence：全量命令、exit code、log digest、verified base、readonly snapshot。
- 状态：accepted success进入 Auditor；accepted failure进入 final correction。
- 不变量：Tester不关闭 task、不集成、不修代码、不发布 work.failed。

#### 6. 修改位置

- `presets/en/parallel-forge.yml`：Tester双禁、allowed final evidence root、
  success/failure precheck和明确重跑。
- `presets/schemas/parallel-forge.yml`：两个 final topic readonly fields。
- 计划新增
  `crates/ralph-core/tests/scenarios/parallel_forge_readonly_tester_runtime.yml`
  并在 `scenarios.rs` 注册。
- agent docs、operator skills、configuration、concepts、AGENTS/CLAUDE同步。
- 必须检查001/002落地后的现有 Parallel Forge scenarios并扩展，不新建
  source-only assertion。

#### 7. 可依赖能力

U1–U3；001 final correction；002 reuse status/checkpoint；现有 full test脚本。

#### 8. 禁止依赖的未来能力

无未来 Unit；不得把发现的其他 preset read-only漂移塞进本 Unit，记录 follow-up。

#### 9. 验收测试

1. all settled + full commands green + no mutation → Auditor一次。
2. full red + no mutation → final correction；settled后 Tester重跑。
3. Tester修改 business/test → hard stop；Auditor/Reporter/fixer不因该 verdict激活。
4. reuse final/reverify checkpoint建立新 snapshot。
5. 六 topic schema/consistency/precheck parity。
6. strict lint negative fixture。
7. agent skill命令/字段静态 drift。
8. 完整 Parallel Forge mock E2E。

```bash
cargo nextest run -p ralph-core --test scenarios -- parallel_forge_readonly_tester
cargo nextest run -p ralph-core --test scenarios -- parallel_forge
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-cli --bin ralph -- presets
cargo nextest run -p ralph-cli --test integration_preset_materialize_artifacts
scripts/check-cli-doc-drift.sh
cargo run -p ralph-e2e -- --mock --filter parallel-forge-dispatch-contract
./scripts/run-tests.sh
```

#### 10. Acceptance Red

在 post-001 final failure scenario让 Tester修改文件并提出 failure；当前可能进入
wave-fixer。目标断言“mutation直接hard stop且不进入correction”形成 Red。另先令
success缺 readonly evidence，断言 Auditor不激活。若001/002 DoD未完成导致 route
缺失，不算有效 Red。

#### 11. 单元测试拆分

1. final success/failure schema parity。
2. full command evidence成功/失败逻辑。
3. all settled/verified base一致。
4. final correction后新 baseline而非复用旧 snapshot。
5. Tester topic deny：integration/correction/task/work.failed。
6. operator lint fixture覆盖双禁/allowed path/六 topic precheck。
7. docs字段与CLI/preset schema一致。

不 Mock final flow authority；BDD必须真实 EventLoop。

#### 12. Red → Green → Refactor 顺序

```text
Tester mutation Acceptance Red
→ strict config/guard Green
→ full success evidence Red/Green
→ full failure correction Red/Green
→ correction settled重跑 Red/Green
→ reuse baseline Red/Green
→ 六 topic parity/negative lint fixture
→ docs/skills drift Green
→ E2E/full workspace regression
→ Refactor并删除临时fixture
```

#### 13. 最小实现范围

只完成 Tester final门禁、跨三 hat contract parity和文档。Agent skill写成通用：
“当 hat同时禁用Edit/Write并声明allowed paths时如何收集证据、emit、遇到mutation
停止”，不得写 `parallel-forge`、003或内部 ledger路径。

#### 14. 集成验证

完整链：

```text
wave fan-in
→ Reviewer readonly gate
→ Integrator candidate
→ Verifier readonly gate
→ settlement
→ all waves settled
→ Tester readonly full gate
→ Auditor
```

失败链：

```text
Reviewer/Verifier/Tester ordinary failure
→ accepted evidence
→ wave-fixer
→ review/integrate/verify
→ Tester rerun when final
```

mutation链在 source hat立即终止，不能进入上述 correction。

#### 15. 风险驱动测试

- State-machine：三类成功/失败/mutation互斥。
- Fault injection：full timeout、log丢失、precheck耗尽。
- Differential：fresh、continue、reuse从相同 checkpoint达到相同 gate结果。
- Idempotency：duplicate final proposed不重复Auditor。
- Security：negative fixture放宽source root必须lint error。

#### 16. 回归范围

全部 Parallel Forge scenarios、precheck/payload consistency、scope termination、
template registry、all embedded presets、agent docs、hat-env污染、E2E mock、
workspace nextest/doctest。若 full baseline flake，按项目规则使用
`RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 仅作诊断兜底；serial仍失败即真实
失败，不能完成。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改配置 | Tester只读/final gate | E2,E4 |
| `presets/schemas/parallel-forge.yml` | 修改schema | final readonly fields | E4,E12 |
| `crates/ralph-core/tests/scenarios/parallel_forge_readonly_tester_runtime.yml` | 新增BDD | S7–S9,S15 | E16 |
| `crates/ralph-core/tests/scenarios.rs` | 修改测试注册 | real EventLoop | E16 |
| `crates/ralph-core/data/ralph-tools-{emit,opac}.md` | 修改agent docs | 通用只读动作 | E20 |
| `docs/guide/configuration.md`、`CONCEPTS.md` | 修改文档 | 配置/术语 | E19,E20 |
| `skills/ralph-preset-common/references/{agent-native-model,author-checklist,finding-rubric,patterns}.md` | 修改operator docs | AAF审查 | E20 |
| `skills/ralph-preset-common/fixtures/aaf-review-negative-fixture.yml` | 修改fixture | 新finding | E20 |
| `AGENTS.md`、`CLAUDE.md` | 同步修改 | builtin权限描述 | E20 |
| `docs/solutions/architecture-patterns/strict-readonly-hat-workspace-snapshot-and-precheck.md` | 新增学习 | snapshot+precheck分层 | E18,E20 |

#### 18. 完成标准

S1–S17全绿；六 topic合同一致；三个 hat及其precheck gate无代码修复出口；
ordinary failure进入正确
correction；mutation首次硬停；reuse重建 baseline；preset/schema/lint/docs/E2E/
全 workspace绿；AGENTS/CLAUDE完全一致；无残留/ephemeral/skip/only/弱断言；
U4可独立提交。

#### 19. 停止条件

001/002实际接口漂移、全量serial仍失败、schema/preset parity失败、agent docs泄漏
内部路径、AGENTS/CLAUDE不一致、或 mutation只能在下游激活后才被发现时停止。
记录新证据→更新影响→重新决策→修订对应 Unit，不得声明完成。

#### 20. 风险与注意事项

- 风险：precheck数量增加导致 max iterations不足。检测：full BDD实际activation
  count。缓解：按真实合成 hat数量调整上限并通过 workflow lint，不凭估算。
- 风险：全量命令写 repo cache。检测：snapshot。缓解：把 cache移到已忽略运行目录
  或修正命令；禁止放宽source白名单。
- 风险：docs描述成runtime内部实现。检测：preset operator review。
  缓解：agent docs只写触发、命令、字段来源、停止条件。

## 8. Unit 串行依赖图

```text
U1 strict read-only runtime guard
  ↓ U2复用 guard/lint并新增共享template，为Reviewer建立第一个业务纵向切片
U2 Reviewer gate
  ↓ U3复用相同event合同，并接入candidate/correction
U3 Verifier gate
  ↓ U4复用已验证模式，闭合final/reuse/docs/full regression
U4 Tester gate + system closure
```

- U2不能先于U1：否则只能靠 prompt/precheck，mutation已发生且无法可靠识别。
- U3不能先于U2：Reviewer先验证 shared success/failure gate模式，Verifier再增加
  candidate SHA与命令证据。
- U4不能先于U3：Tester final correction依赖已验证的增量 correction/reverify链。
- 每个 Unit禁止提前实现后续 hat；共享抽象只在当前 Scenario需要时增加。

## 9. 执行命令清单

| 命令 | 运行时机 | 验证目的 | 预期结果 | 失败后可继续 |
|---|---|---|---|---|
| `cargo nextest run -p ralph-core -- workspace_mutation_guard` | U1 Red/Green | snapshot/matcher | 全绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- strict_readonly` | U1–U4 | runner real Git/hard stop | 全绿 | 否 |
| `cargo nextest run -p ralph-cli --test integration_preset_materialize_artifacts -- parallel_forge` | U1/U4 | template物化 | 全绿 | 否 |
| `cargo nextest run -p ralph-core -- payload_consistency` | U2–U4 | self-contradiction | 全绿 | 否 |
| `cargo nextest run -p ralph-core -- precheck` | U2–U4 | proposed/reject/exhaust | 全绿 | 否 |
| `cargo nextest run -p ralph-core --test scenarios -- parallel_forge_readonly_reviewer` | U2 | Reviewer链 | 全绿 | 否 |
| `cargo nextest run -p ralph-core --test scenarios -- parallel_forge_readonly_verifier` | U3 | Verifier链 | 全绿 | 否 |
| `cargo nextest run -p ralph-core --test scenarios -- parallel_forge_readonly_tester` | U4 | Tester链 | 全绿 | 否 |
| `cargo nextest run -p ralph-core --test scenarios -- parallel_forge` | 每Unit回归/U4 | 全preset flow | 全绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | preset每次修改 | CLI lint | 全绿 | 否 |
| `cargo nextest run -p ralph-core -- preset_lint` | preset/lint每次修改 | core lint/finding | 全绿 | 否 |
| `cargo nextest run -p ralph-cli --bin ralph -- presets` | schema/preset修改 | embedded parity | 全绿 | 否 |
| `scripts/check-cli-doc-drift.sh` | U4 docs后 | agent docs命令/字段 | exit 0 | 否 |
| `cargo fmt --check` | 每Unit close | 格式 | exit 0 | 否 |
| `cargo clippy` | 每Unit close | lint/type | exit 0 | 否 |
| `cargo build` | 每Unit close | build | exit 0 | 否 |
| `RALPH_CURRENT_HAT=executor RALPH_CURRENT_LOOP_ID=loop-x RALPH_EVENTS_FILE=/tmp/x.jsonl cargo nextest run -p ralph-cli --bin ralph -- strict_readonly` | U1/U4 | hat-env污染 | 全绿 | 否 |
| `cargo run -p ralph-e2e -- --mock --filter parallel-forge-dispatch-contract` | U4 | E2E | exit 0，无placeholder | 否 |
| `./scripts/run-tests.sh` | 最终 | workspace+doctest | 全绿 | 否 |

不存在独立契约测试框架；schema/preset parity + real EventLoop BDD承担本计划契约测试。
禁止裸跑 `cargo test -p ralph-cli`。

## 10. 最终质量门禁

- S1–S17全部通过。
- R1–R24均有可执行测试。
- Reviewer、Verifier、Tester均双禁 Edit/Write并有合法 evidence path。
- commit、branch、index、tracked、protected untracked、Git operation全部受 guard。
- 既有 dirty不误报，新增 delta不漏报。
- mutation首次 typed hard stop，不自动清理、不进入补证/correction。
- 普通失败有真实命令/evidence并进入正确 wave-fixer。
- 六 topic schema、consistency、precheck一致。
- success与failure均不能无证据发布。
- precheck三次耗尽语义正确。
- 002 reuse checkpoint重新建立当前 baseline。
- template binary materialize通过。
- preset lint negative fixture命中稳定 finding ID。
- Characterization、state-machine、fault injection、idempotency、security tests通过。
- cargo fmt、clippy、build、nextest、E2E、doctest、doc drift全绿。
- 无新增失败/skip/only、无断言削弱、无无解释 snapshot/golden更新。
- AGENTS.md与CLAUDE.md完全一致。
- 无 `.ralph/review/<plan-id>/scratch`、residual或ephemeral文件进入git。
- 所有关键决策置信度≥0.85，无 BLOCKED事项。
- 实际变更未扩展到其他 hats/presets。
- U1→U2→U3→U4严格串行并各自形成完整TDD闭环。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 四个行为纵向 Unit，各有真实入口/Red/Green/回归 |
| Executor 是否仍需做关键设计决策 | 否 | KTD1–KTD14锁定权限、snapshot、gate、routing |
| 所有文件和接口是否有代码库证据 | 是 | 现有路径均见E；新增路径明确标“计划新增” |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | 最低KTD12=0.91 |
| 是否存在未处理的低置信度假设 | 否 | 001/002接口差异是入口停止条件 |
| 每个 Unit 是否只有一个可观察行为 | 是 | guard、Reviewer、Verifier、Tester各一纵向行为 |
| 每个 Unit 是否可以独立验证 | 是 | 各有targeted nextest与独立提交边界 |
| 每个 Unit 是否有真实 Red | 是 | clean commit、无证据 verdict、test mutation、final mutation |
| 每个 Unit 是否包含回归范围 | 是 | 每Unit §16 |
| 是否存在未来 Unit 依赖 | 否 | 只依赖已完成前置Unit |
| 是否存在泛化任务描述 | 否 | 修改入口、字段、测试、停止条件具体 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §5、§6矩阵 |
| 所有关键决策是否有 Evidence | 是 | KTD表逐项引用E |
| 计划是否可以严格串行执行 | 是 | U1→U2→U3→U4 |
