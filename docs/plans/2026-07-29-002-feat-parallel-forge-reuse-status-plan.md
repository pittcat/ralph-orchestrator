---
title: Parallel Forge 跨运行 Reuse Status 与证据复用 - Plan
type: feat
date: 2026-07-29
origin: docs/plans/2026-07-29-001-fix-parallel-forge-static-wave-settlement-plan.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

# Parallel Forge 跨运行 Reuse Status 与证据复用 - Plan

## 0. 计划状态

- **状态：READY（受 P0 完成门禁约束）**
- **代码基线：** `d737b9b79a5683b2b59819c7f3ddf38e6590a8d7`
- **前置依赖：** 必须先完成
  `docs/plans/2026-07-29-001-fix-parallel-forge-static-wave-settlement-plan.md`
  的 Definition of Done。P0 计划新增的静态 wave、`forge.wave.settled`、
  `CloseTaskBatch`、per-wave review/integration/verification、correction artifacts
  是本计划的前置合同，不是当前代码已存在的事实。
- **调查范围：** `--reuse-worktree` 精确 worktree 复用与 runtime archive、
  plan baseline、`--continue` 同运行恢复、supervisor store/recovery、
  `ralph inspect`、precheck desugar/重试、state projection、Parallel Forge
  preset/schema/templates/BDD、模板嵌入与物化、相关 Git 历史和
  `docs/solutions/`。
- **已执行的验证：**
  - 读取 `crates/ralph-core/src/worktree.rs` 中 worktree 精确匹配、archive 和
    `resume-context.md` 生成路径。
  - 读取 `crates/ralph-cli/src/commands/run.rs` 中
    `--worktree --reuse-worktree` 与 `--continue` 的分流。
  - 读取 `crates/ralph-core/src/plan_baseline.rs`，确认 baseline 在
    `--reuse-worktree` 和 `--continue` 间保留。
  - 读取 supervisor startup recovery、redrive resume-only 约束和
    `ralph inspect loop` 的 agent-safe summary。
  - 读取 precheck 配置、desugar、synthetic rejection、3 次 budget 和
    `on_exhausted` 实现。
  - 读取 Parallel Forge preset/schema/templates、真实 EventLoop BDD 入口、
    模板 embed/materialize 注册表和 P0 计划合同。
  - 检查 Git 基线、工作区状态和 reuse 相关历史提交。
- **本轮未运行测试：** `ce-plan` 只调查和规划，不执行 Acceptance Red、
  nextest、lint、build 或 E2E。
- **实施入口门禁：**
  1. P0 全部 DoD 已通过；
  2. P0 实际落地的 topic、required fields、模板路径和 `CloseTaskBatch`
     与本计划引用一致；
  3. `cargo run -p ralph-e2e -- --mock --list` 包含
     `parallel-forge-dispatch-contract`，且
     `cargo run -p ralph-e2e -- --mock --filter parallel-forge-dispatch-contract`
     退出码为 0、输出不含 `placeholder`；这证明 P0 没有用“未注册占位场景”
     伪装 E2E Green；
  4. `git status --short` 不含计划范围外改动；
  5. baseline preset tests 没有新增触及 `parallel-forge` 的失败。
  任一不满足时停止 U1，更新 Evidence/KTD/Unit 后再实施。
- **阻塞项：** 无设计阻塞。P0 尚未实施是有明确验收条件的串行依赖，不授权
  Executor 在 P0 落地前提前实现本计划。

## Goal Capsule

- **目标：** 当操作者用 `--reuse-worktree` 在同一代码 worktree 启动一个
  新运行时，Ralph 能从上一运行的 Git、静态 execution plan、wave settlement
  和失败 artifacts 中计算可信 reuse status，并只从最早未被证据证明的检查点
  继续。
- **通俗解释：** 代码还在，不代表“做完了”；上一轮说通过，也不代表现在仍然
  通过。本功能让系统逐项回答“哪些真的还能用、哪些只需重验、哪些应继续修、
  哪些必须重跑、哪些证据互相打架必须停下”，而不是全盘重做或盲信旧结论。
- **权威顺序：** 当前 operator plan
  → 当前 `execution-plan.yml`/digest
  → 当前 Git object graph 和 working tree
  → 上一运行完整 archive manifest
  → P0 settlement/failure artifacts 及 digest
  → runtime reuse evaluator
  → precheck 独立重算
  → 当前运行的 accepted reuse event/state projection。
- **执行模型：** 保持 `supervisor + wave`。Reuse 只决定从哪个 P0 checkpoint
  继续，不新增第二套调度器、重试系统或 supervisor 数据库。
- **恢复边界：** 只自动接受“从 wave 1 开始的连续已证明 settlement 前缀”。
  第一个未证明 wave 及其后继不能被跳过；复杂 DAG 通过静态 wave 和传递依赖
  闭包计算，不靠 hat 猜 ready set。
- **失败经验：** 上次匹配失败的 fingerprint、尝试、证据和排除原因会注入新
  correction；旧 verdict、旧 correction round 和旧 retry budget 不继承。
- **停止条件：** archive 未完成或被篡改、settlement SHA 不可达、同一身份出现
  冲突证据、当前 plan/DAG 无法与旧合同对应、dirty paths 归属不清、或 precheck
  三次重算仍不一致时 fail closed。

## 1. 功能目标

### 1.1 业务目标

降低长计划在进程退出、人工停止或新一轮运行后的重复劳动，同时不把“目录里还有
代码”误当成“行为已验证”。Reuse 必须节省已证明的工作，也必须保住 P0 的
依赖、审查、集成和验证门禁。

### 1.2 用户或调用方

- 操作者：使用
  `ralph run --worktree --reuse-worktree --plan <plan> -H builtin:parallel-forge`
  开启新运行。
- `clean_worktree_runtime_artifacts`：归档旧运行并建立机器可核验的 reuse 来源。
- `ralph inspect reuse-status`：只读计算并输出 agent-safe reuse assessment。
- reuse-reconciler：物化模板、解释 runtime 结果并提出当前运行的 reuse handoff。
- precheck gate：独立重跑相同 evaluator，拒绝 stale/tampered assessment。
- P0 dispatcher/failure handler/reviewer/integrator/verifier：从 assessment 指定的
  checkpoint 继续。
- Reporter/操作者：从最终报告看到哪些工作被复用、重新验证、修复或重跑。

### 1.3 当前行为

1. `--reuse-worktree` 精确复用已结束的 worktree，并把事件、任务、diagnostics、
   review、scratchpad、summary、handoff 和 decisions 移到
   `.ralph/reuse-history/<timestamp>/`。
2. cleanup 不归档 `.ralph/supervisor.db` 及其 WAL/SHM sidecars，也不归档
   Parallel Forge 的 `.ralph/forge/` business artifacts；fresh run 可能看到旧
   wave ledger，并会与同 plan-key 的旧 artifact paths 相撞。
3. cleanup 只写自然语言 `resume-context.md`。该文件说明旧记录是 advisory，
   但没有 previous HEAD、loop ID、文件 digest、archive completeness 或完成标记。
4. `--continue` 会读取现有 scratchpad/events/supervisor state，恢复同一运行；
   `--reuse-worktree` 是清理 runtime state 后的新运行。二者没有共同的恢复语义。
5. Parallel Forge 目前没有 runtime reuse evaluator，也没有五态状态表、DAG
   downgrade 规则或 accepted reuse event。
6. `ce-executor-pipeline` 已有 prompt-driven archive mining，但主要依赖 agent
   读 archive/Git 后写 guidance，不能替代 runtime 权威判定。

### 1.4 目标行为与行为差异

- cleanup 在启动新运行前以“完整复制到 staging → 校验 digest → 按 manifest
  删除 live artifacts → 原子发布完整目录”的顺序归档旧 supervisor DB/WAL/SHM
  和其他 runtime 文件。中途失败由同一 cleanup lock 下的下一次调用继续，不把
  半成品当成可消费 archive。
- `ralph inspect reuse-status --execution-plan <path>
  --approved-base-commit <sha> --format json` 只读当前 plan、Git 和最新完整
  manifest，输出每 Unit 的证据状态、每 wave 的有效状态、连续 reusable prefix、
  首个恢复 checkpoint、失败经验和 stable digest。
- 五种公开状态固定为：
  - `reusable`：当前证据足以重建已结算事实；
  - `reverify`：实现仍可用，但 review/integration/verification/settlement 中至少
    一个证明已过期或缺失；
  - `resume_correction`：存在与当前合同仍匹配的失败，需要用上次经验继续修；
  - `rerun`：实现身份、依赖、路径合同或 commit 证据已改变，必须重新执行 Unit；
  - `blocked`：证据矛盾、archive 不完整或无法安全决定。
- 只有完整 reusable wave 构成的连续前缀可以在当前运行原子关闭 task。部分可复用
  wave 只保留实现/失败证据，不提前关闭任何 task。
- 第一个非 reusable wave 用 `resume_checkpoint` 选择 P0 的 `execute`、
  `correction`、`review`、`integration` 或 `verification` 入口；后续 wave 保持
  open，直到前置 wave 在当前运行 settlement。
- reuse-reconciler 不能自行改变 runtime 结果。precheck 必须重新调用 evaluator，
  比较 assessment digest、archive manifest digest、current HEAD 和 plan digest。
- 没有 archive 的首次运行输出全 `rerun`，不是错误；legacy/incomplete archive
  不猜测，输出 `blocked` 及稳定 reason code。

### 1.5 输入

- 当前 repo-relative operator plan path。
- P0 当前 `execution-plan.yml`、`execution_plan_digest`、静态 Unit DAG/waves。
- 当前 `HEAD`、branch、working tree path set、plan baseline。
- 最新完整 reuse archive manifest。
- archive 中的 P0 execution plan、events、settlement/review/integration/
  verification/correction/failure artifacts。
- Git 中 manifest/artifacts 引用的 full SHA、commit ancestry 和 changed paths。

### 1.6 输出

- `.ralph/reuse-history/<timestamp>/reuse-manifest.v1.json`。
- `.ralph/reuse-history/<timestamp>/COMPLETE` 原子完成标记。
- `ralph inspect reuse-status --format json` 的 `reuse_status.v1` 输出。
- `.ralph/forge/<plan-key>/reuse/reuse-status.yml`，按 builtin template 物化。
- `.ralph/forge/<plan-key>/reuse/reuse-evidence.md`，使用统一 rubric。
- accepted `forge.reuse.assessed` event 及 state projection。
- 最终 manager report 中的 reuse summary。

### 1.7 状态变化

- cleanup 只改变 runtime artifact 位置，不改变 Git branch、commit 或 plan baseline。
- accepted `forge.reuse.assessed` 可原子关闭“完整 reusable 连续 wave 前缀”的
  tasks，并把 `verified_base_commit` 投影为最后一个 reused settlement SHA。
- 部分 wave 不关闭 task；只投影 `resume_checkpoint`、affected Unit IDs 和
  prior failure references。
- 新运行的 precheck/correction 计数从 0 开始。

### 1.8 错误语义

| Reason code | 语义 | 结果 |
|---|---|---|
| `no_prior_archive` | 首次运行或无旧 runtime | 全部 `rerun`，允许继续 |
| `legacy_archive_unverifiable` | 旧 archive 无 v1 manifest/complete marker | `blocked` |
| `archive_incomplete` | manifest、COMPLETE 或成员 digest 缺失 | `blocked` |
| `artifact_digest_mismatch` | archive 文件内容与 manifest 不同 | `blocked` |
| `plan_identity_changed` | Unit ID 可对应但合同 digest 改变 | Unit/wave `rerun` |
| `dependency_not_reusable` | 传递前置未证明 | 下游不得 `reusable`；降级到 `rerun` |
| `commit_unreachable` | 引用 SHA 不在当前 Git object graph | `blocked` |
| `settlement_sha_conflict` | 同一 wave 有冲突 accepted settlement | `blocked` |
| `verification_stale` | 实现/commit 可达但验证不覆盖当前树 | `reverify` |
| `prior_failure_relevant` | failure fingerprint 与当前合同匹配 | `resume_correction` |
| `dirty_path_ambiguous` | dirty path 命中多个/零 Unit owner | `blocked` |
| `assessment_digest_changed` | proposal 后 plan/HEAD/archive 变化 | precheck reject |
| `precheck_exhausted` | 三次拒绝仍不能一致 | `plan.blocked` |

### 1.9 兼容性要求

- 不要求旧 reuse archive 自动升级；证据不足必须 fail closed。
- 不改变 `--continue` 行为、same-run supervisor recovery 或 redrive budget。
- 未使用 `parallel-forge` 的 preset 继续获得现有 archive cleanup；除旧 supervisor
  ledger 被正确归档外，不启用 reuse assessment/topology。
- P0 topic/schema 允许按本计划扩展，不要求兼容旧 Parallel Forge event log。

### 1.10 性能要求

- archive manifest 生成和 evaluator 对 archive 文件做单次线性扫描。
- DAG/状态传播为 O(V+E)。
- Git ancestry/path 检查可按 SHA 去重；不得为每个 Unit 重复遍历全部历史。
- `inspect reuse-status` 不运行测试、不启动 backend、不打开新 supervisor ledger。
- 100 Units、20 waves、500 archive artifacts 的本地 assessment 目标为 2 秒内；
  超时只作为性能回归门禁，不改变 correctness。

### 1.11 安全与权限要求

- evaluator 和 reuse-reconciler 只读代码与 archive；不得修改生产代码。
- archive path 必须 canonicalize 后仍位于当前 worktree
  `.ralph/reuse-history/`；拒绝 symlink/path traversal。
- JSON 输出不得泄漏 supervisor DB 路径、raw event log 或非必要 prompt 内容。
- reuse-reconciler 只能发布 assessment proposal/blocked；不能发布
  `exec.unit.done`、`forge.wave.settled` 或 `work.failed`。
- Verifier/Tester 继续只读；修复仍由 P0 executor/wave-fixer 完成。

### 1.12 本次范围

- Parallel Forge 的跨运行 reuse assessment。
- archive manifest/complete marker/supervisor ledger hygiene。
- 五态分类、复杂 DAG 闭包、连续 reusable prefix 和 checkpoint 选择。
- 失败经验继承但 retry/correction round 重置。
- runtime evaluator + read-only inspect CLI + precheck 独立重算。
- 两份复杂模板及 embed/materialize。
- 当前运行 task/state projection 与 P0 resume routing。
- CLI/AI skill/preset operator docs、schema、BDD 和 regression。

### 1.13 非目标

- 不为所有 builtin presets 建立通用 checkpoint DSL。
- 不把 `--continue` 改成 archive reuse。
- 不自动 cherry-pick、reset、force-push 或解决语义冲突。
- 不复用旧 supervisor store 作为当前运行状态。
- 不继承旧 retry budget、precheck rejection count 或 correction round。
- 不新增 Web UI、远程 artifact store 或数据库迁移。
- 不允许 hat 用主观 confidence 覆盖 runtime 的 hard evidence gate。

### 1.14 已知约束、事实与假设

**已确认事实**

- worktree reuse 已是 exact-name + completed-loop lookup。
- cleanup 当前使用 rename 把 runtime artifacts 移入 timestamp archive，I/O 错误会
  阻止新运行启动。
- plan baseline 已持久化且不会在 reuse 时重写。
- fresh boot 不消费旧 redrive；resume-only recovery 是现有明确边界。
- precheck 现有默认 retry budget 为 3，会在拒绝后给 producer 注入
  `task.resume`，耗尽可发 `plan.blocked`。
- `ralph inspect` 是现有 read-only/agent-safe 命名空间。
- P0 已锁定 static wave、settlement、failure/correction 和
  `supervisor + wave` 合同。

**已确认假设**

- P0 按其计划落地后，每个 accepted settlement 和 failure artifact 都能由
  repo-relative path、full SHA 和 digest 唯一锚定。
- Git object graph 与完整 archive 足以做跨运行 reuse 判断，不需要复用旧
  supervisor DB 的活状态。

**待验证假设**

- 无可推迟到 Executor 临时决定的假设。U1 的 P0 entry gate 是验证前置合同；
  失败时修订计划，不允许边写代码边猜 P0 接口。

## Product Contract

### Requirements

#### Archive 与运行边界

- **R1.** `--reuse-worktree` 必须把旧 supervisor DB、`-wal`、`-shm`、
  `.ralph/forge/` 与现有 runtime artifacts 一起归档，并保证新运行 live path
  没有旧 supervisor 状态或旧 Parallel Forge business artifacts。
- **R2.** 每次有旧 runtime 可归档时必须生成 versioned manifest，记录 previous
  loop、previous HEAD、plan baseline、archive members/digests 和 source metadata。
- **R3.** cleanup 必须用 worktree-scoped exclusive lock 串行化，并使用可恢复
  staging：全部成员复制/hash成功后，才按 manifest 删除 live artifacts；全部
  live target 消失后才写 `COMPLETE` 并原子 rename 为可消费 archive。partial
  staging 必须可由下一次调用继续，且不得启动 reuse assessment。
- **R4.** `--continue` 继续读取 live runtime state；`--reuse-worktree` 只读取
  immutable archive，二者不共享计数或 verdict。

#### Runtime reuse assessment

- **R5.** runtime 必须输出固定五态 `reusable`、`reverify`、
  `resume_correction`、`rerun`、`blocked`，并为每项给稳定 reason code 和证据。
- **R6.** Unit identity 必须由 `unit_id + unit_contract_digest` 确定；digest 覆盖
  depends_on、execution_wave、integration_order、allowed/forbidden paths、
  shared contracts、验收行为和测试命令。
- **R7.** `reusable` 必须同时证明合同未变、commit 可达、artifact digest 正确、
  settlement accepted、verified SHA 与 Git ancestry 对齐、传递依赖均 reusable。
- **R8.** 实现可达但 review/integration/verification/settlement 证明缺失或过期时
  必须是 `reverify`，并给 `review|integration|verification` checkpoint。
- **R9.** 当前合同仍匹配且存在未结算 failure fingerprint 时必须是
  `resume_correction`；旧失败经验必须可读，旧 round/budget 必须重置。
- **R10.** 合同、依赖或实现身份改变时必须 `rerun`；不得凭相同 Unit 名复用。
- **R11.** archive/证据矛盾、SHA 不可达、digest 不符、dirty path 归属不清时
  必须 `blocked`，不得自动降级成“可能可用”。
- **R12.** 没有 prior archive 的首次运行必须得到确定的全 `rerun` 结果并继续。

#### DAG 与恢复

- **R13.** 只有从 wave 1 开始连续、完整、全部 Unit `reusable` 的 wave 前缀可
  在当前运行被接受为“settlement 等价的 reuse prefix”；它通过
  `forge.reuse.assessed` 投影，不伪造 P0 `forge.wave.settled` 事件。
- **R14.** 一个 wave 内只要存在非 reusable Unit，该 wave 任何 task 都不得因
  reuse 提前关闭；成功 sibling 的实现证据可以保留供后续全 wave review。
- **R15.** 第一个非 reusable wave 的恢复点必须取所有 Unit 最早未证明 checkpoint：
  `execute < correction < review < integration < verification < settlement`。
- **R16.** 非连续后继即使旧 archive 声称 settled，也必须因前置不成立而降级；
  dispatcher 不得跨 wave。
- **R17.** `rerun` 只派发受影响 Unit 的执行槽位；未受影响 sibling 以 preserved
  completion evidence 参与 fan-in 后的全 wave review，不伪造 supervisor slot terminal。
- **R18.** `resume_correction` 直接进入 P0 correction handler；修复后必须重新
  review/integrate/verify 整个 wave。
- **R19.** `reverify` 从 assessment 指定 checkpoint 开始，但最终仍必须产生当前
  运行的 settlement。

#### 事件与证据门禁

- **R20.** reuse-reconciler 只能转录 runtime assessment；不能自行提升状态。
- **R21.** `forge.reuse.assessed` 必须挂 precheck。gate 必须重跑 evaluator，并
  比较 assessment/manifest/plan/HEAD digest 与 Unit/wave counts。
- **R22.** precheck rejection 必须返回稳定 `failed_checks`，目标仍是
  reuse-reconciler；三次耗尽产生
  `plan.blocked{kind: precheck_exhausted}`。
- **R23.** accepted assessment 才能投影 reusable task batch、verified base 和
  resume checkpoint；proposal/rejected 事件零 task 副作用。
- **R24.** assessment 重放必须幂等；相同 digest 不重复关 task，不同 digest
  在相同 reuse scope 下 fail closed。

#### 模板、可观察性与报告

- **R25.** 复杂状态决策表必须放在
  `presets/templates/parallel-forge/reuse-status-rubric.template.md`，作为
  reconciler/precheck/reporter 的同一 SSOT。
- **R26.** 机器 artifact 必须使用
  `presets/templates/parallel-forge/reuse-status.template.yml`，字段与
  `reuse_status.v1` JSON 对齐。
- **R27.** 两个模板必须 compile-time embed，并能通过
  `ralph preset materialize-artifacts parallel-forge --plan-key <key>` 物化。
- **R28.** `ralph inspect reuse-status` 必须提供 human/JSON 两种同源输出；
  JSON 不暴露内部 ledger 路径或 raw event payload。
- **R29.** manager report 必须列出 reusable prefix、恢复 wave/checkpoint、
  各状态计数、采用的 prior failures 和 blocked reason。
- **R30.** agent skill docs、preset author/review references、schema、zsh/AGENTS
  描述在行为变化后保持同步。

### Actors

- **A1 操作者：** 选择 reuse-worktree，观察 assessment 和最终报告。
- **A2 runtime archiver/evaluator：** 生成不可变证据并作五态权威计算。
- **A3 reuse-reconciler/precheck：** 物化、提出、独立复核，不替代 runtime。
- **A4 P0 development loop：** 从 checkpoint 恢复并重新 settlement。
- **A5 Reporter：** 汇总结果，不重新判定状态。

### Key Flows

- **F1 首次运行：** 无 archive → 全 `rerun` → 正常 P0 wave 1。
- **F2 全部已结算：** 完整 archive → 全 wave reusable → 原子关闭当前 tasks →
  跳到 P0 full Tester，不重复 executor。
- **F3 settled prefix + failed wave：** prefix reusable → 失败 wave
  `resume_correction` → 注入旧失败 → current-run correction/review/verify/settle。
- **F4 verification stale：** 实现/commit 可达 → `reverify` →
  从最早缺失 checkpoint 恢复。
- **F5 plan/DAG 改动：** 受影响 Unit `rerun`，传递后继 downgrade，未受影响
  settled prefix 仍可复用。
- **F6 矛盾证据：** evaluator `blocked` → 零 task 关闭 →
  reporter/plan.blocked 提供 reason 和 evidence。
- **F7 proposal 后漂移：** HEAD/plan/archive digest 改变 → precheck reject →
  reconciler 重算；第三次仍漂移则 blocked。

### Acceptance Examples

```gherkin
Feature: Parallel Forge 跨运行状态复用

  Background:
    Given P0 static-wave settlement 已完成并通过其 Definition of Done
    And 当前运行使用 --worktree --reuse-worktree 和同一 operator plan

  Scenario S1: 首次运行没有历史证据
    Given worktree 中没有完整 reuse archive
    When runtime 计算 reuse status
    Then 每个 Unit 状态为 rerun 且 reason_code 为 no_prior_archive
    And 不读取或创建 supervisor.db
    And 正常从 wave 1 execute 开始

  Scenario S2: 完整结算的连续 wave 前缀被复用
    Given wave 1 和 wave 2 的合同 digest 未变
    And settlement artifacts、verified SHA 和 Git ancestry 全部匹配
    And wave 3 没有 settlement
    When precheck 接受 forge.reuse.assessed
    Then wave 1 和 wave 2 tasks 被一次原子投影为 done
    And verified_base_commit 等于 wave 2 settlement SHA
    And dispatcher 只能从 wave 3 开始

  Scenario S3: 同 wave 的一个 Unit 需要重跑
    Given wave 2 的 U2 合同未变且实现证据可用
    And wave 2 的 U3 allowed_paths 与当前 plan 不同
    When runtime 计算 wave 2
    Then U2 保留 preserved evidence
    And U3 状态为 rerun
    And wave 2 resume_checkpoint 为 execute
    And wave 2 任一 task 都不因 reuse 提前关闭

  Scenario S4: 上次验证失败后继续修复
    Given settled prefix 到 wave 1
    And wave 2 有与当前合同匹配的 verification failure fingerprint
    When runtime 计算 wave 2
    Then wave 2 状态为 resume_correction
    And 输出上次失败证据、已尝试方案及未采用理由
    And current correction_round 从 1 开始而不是继承旧值

  Scenario S5: 实现可用但验证已过期
    Given Unit commit 可达且合同未变
    And 当前 HEAD 的相关路径不再等于旧 verified tree
    When runtime 计算 reuse status
    Then Unit 状态为 reverify
    And checkpoint 是最早缺失的 review、integration 或 verification
    And Unit 不进入 reusable prefix

  Scenario S6: 下游不能越过未证明依赖
    Given wave 1 reusable
    And wave 2 rerun
    And archive 声称 wave 3 settled
    When runtime 传播 DAG 状态
    Then wave 3 不得 reusable
    And reason_code 包含 dependency_not_reusable
    And dispatcher 不得跳到 wave 3

  Scenario S7: 证据被篡改时阻塞
    Given settlement artifact 内容与 archive manifest digest 不同
    When runtime 计算 reuse status
    Then assessment 为 blocked
    And reason_code 为 artifact_digest_mismatch
    And 零 task 状态变化

  Scenario S8: proposal 后 HEAD 漂移
    Given reconciler 写出的 assessment 绑定 HEAD A
    And precheck 前当前 HEAD 变为 B
    When gate 独立重算
    Then forge.reuse.assessed 被拒绝
    And failed_checks 包含 assessment_digest_changed
    And 新 assessment 不继承旧 rejection count 之外的任何 verdict

  Scenario S9: 三次 precheck 仍无法稳定
    Given 同一 reuse scope 已连续三次 assessment mismatch
    When 第三次 rejection 被 runtime 接受
    Then runtime 发布 plan.blocked kind=precheck_exhausted
    And 不关闭任何尚未被 accepted assessment 覆盖的 task

  Scenario S10: reuse cleanup 隔离 supervisor 状态
    Given 上一运行留下 supervisor.db、supervisor.db-wal 和 supervisor.db-shm
    When --reuse-worktree 启动新运行
    Then 三个文件都进入同一个完整 archive
    And live .ralph 下不存在这些文件
    And fresh inspect 不显示旧 active wave

  Scenario S11: 同一 assessment 重放幂等
    Given forge.reuse.assessed digest D 已投影 reusable prefix
    When 相同 scope 和 digest D 被重放
    Then task、verified base 和 resume checkpoint 不重复变化
    And 不创建第二份 accepted reuse settlement

  Scenario S12: legacy archive 不被猜测
    Given reuse-history 只有旧式 archive 且没有 v1 COMPLETE marker
    When runtime 计算 reuse status
    Then assessment 为 blocked
    And reason_code 为 legacy_archive_unverifiable
    And 输出建议启动非 reuse 新运行或人工补充可验证证据

  Scenario S13: binary-only 环境物化统一 reuse 模板
    Given 操作者只安装了 ralph binary 而没有源码 templates 目录
    When 执行 ralph preset materialize-artifacts parallel-forge --plan-key demo
    Then 输出目录包含 reuse-status.template.yml
    And 输出目录包含 reuse-status-rubric.template.md
    And machine template 字段与 reuse_status.v1 JSON 合同一致

  Scenario S14: mock E2E 证明跨运行复用主路径
    Given P0 Parallel Forge marker cassette 场景已注册并可独立通过
    And 上一运行留下两个 settled waves 和第三 wave 的 correction failure
    When 新运行通过 --reuse-worktree 执行该 mock 场景
    Then 前两个 waves 不重复执行并原子关闭其当前 tasks
    And 第三 wave 收到旧失败经验、完成 correction 和重新 settlement
    And full Tester、Auditor、Reporter 各按 P0 终局合同完成
    And 场景退出码为 0 且不使用 live API

```

### Success Criteria

- 五态及 DAG downgrade 在纯 evaluator、CLI integration 和真实 EventLoop BDD
  三层一致。
- fresh run 不读取旧 supervisor state。
- 全 reusable plan 不重复执行 Unit，但仍运行 P0 full final gate。
- partial plan 从正确 checkpoint 恢复，后继不越过未证明 wave。
- prior failure 可见且 retry/correction round 重置。
- 任一 hard evidence mismatch 都零 task 副作用。

### Scope Boundaries

- 本计划只服务 `parallel-forge` 的 P0 artifact/topology；runtime evaluator 使用
  可复用的纯数据结构，但不增加可配置 checkpoint 平台。
- archive hygiene 是所有 reuse-worktree 的通用修复；五态路由由
  Parallel Forge 显式 opt-in。
- 不把 operator plan 内容本身存进 public inspect JSON；只输出 path/digest。

### Sources

- 前置合同：
  `docs/plans/2026-07-29-001-fix-parallel-forge-static-wave-settlement-plan.md`
- archive 当前实现：`crates/ralph-core/src/worktree.rs`
- reuse CLI 接线：`crates/ralph-cli/src/commands/run.rs`
- baseline：`crates/ralph-core/src/plan_baseline.rs`
- inspect 模式：`crates/ralph-cli/src/commands/inspect.rs`
- supervisor agent-safe summary：`crates/ralph-core/src/supervisor/mod.rs`
- precheck：`crates/ralph-core/src/config/precheck.rs`、
  `crates/ralph-core/src/event_loop/precheck_gate_runner.rs`
- template embed：`crates/ralph-cli/src/builtin_artifact_templates.rs`、
  `crates/ralph-cli/build.rs`
- archive reuse 历史：Git `71faad67`、`fdd3ca9f`、`756f9ffa`
- recovery learnings：
  `docs/solutions/developer-experience/redrive-descriptor-persist-and-boot-dispatch-2026-07-29.md`
  和
  `docs/solutions/database-issues/emission-store-concurrent-open-race.md`

## 2. 代码库现状与证据

### 2.1 当前实现入口

```text
ralph run --worktree --reuse-worktree --plan <plan>
  → commands/run.rs 精确解析 worktree 名
  → worktree.rs::find_reusable_worktree_by_name
  → worktree.rs::clean_worktree_runtime_artifacts
  → 旧 runtime rename 到 .ralph/reuse-history/<timestamp>/
  → 写 .ralph/agent/resume-context.md
  → 重建 LoopContext / symlink / context / PROMPT
  → fresh EventLoop + supervisor startup
```

`--continue` 在 `run_command` 更早读取 live scratchpad，并在 runner 中调用
resume 初始化和 active-wave recovery；它不调用上述 archive cleanup。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `crates/ralph-core/src/worktree.rs::find_reusable_worktree_by_name` | 精确 worktree path、git worktree list 和 live registry 三重检查 | 保留 exact reuse identity，不新增模糊匹配 | 高 |
| E2 | `crates/ralph-cli/src/commands/run.rs` reuse 分支 | cleanup 在新 LoopContext 启动前执行 | manifest/ledger hygiene 应在此原子边界完成 | 高 |
| E3 | `crates/ralph-core/src/worktree.rs::clean_worktree_runtime_artifacts` | 归档 events/history/diagnostics/agent/review；不含 supervisor DB 和 `.ralph/forge/` | R1 是真实缺口 | 高 |
| E4 | 同函数 `resume-context.md` | 只写 advisory 文本与 archive path | 需要 machine manifest；保留人类提示作为补充 | 高 |
| E5 | `crates/ralph-core/src/worktree.rs` cleanup tests | 覆盖 archive、symlink、context、幂等；无 supervisor/manifest 测试 | U1 测试位置已确认 | 高 |
| E6 | `crates/ralph-cli/tests/integration_worktree_isolation.rs` | 覆盖 exact reuse、archive、代码保留 | 扩展真实 CLI reuse 场景 | 高 |
| E7 | `crates/ralph-core/src/plan_baseline.rs` | baseline create-new、有效 SHA、reuse/continue 保留 | evaluator 使用原 baseline，不重锚定 | 高 |
| E8 | `crates/ralph-cli/src/commands/run.rs` continue 分支 | continue 要求 live scratchpad | 明确 same-run 与 cross-run 分离 | 高 |
| E9 | runner supervisor startup/recovery | active wave recovery 属于 resume；fresh boot 不消费旧 redrive | 旧 supervisor DB 不能成为 cross-run 状态 | 高 |
| E10 | `crates/ralph-core/src/supervisor/mod.rs::SupervisorInspectSummary` | 公开 summary 不暴露 DB path/raw events | reuse inspect 遵循同一安全边界 | 高 |
| E11 | `crates/ralph-cli/src/commands/inspect.rs` | inspect 是 read-only；human/JSON 同源；loop schema 变更需 bump | 新增独立子命令，不污染 loop_inspect.v2 | 高 |
| E12 | `crates/ralph-core/src/config/precheck.rs` | precheck desugar为 proposed/gate/rejected；default budget 3 | 复用现有 gate，不建第二套 retry | 高 |
| E13 | `precheck_gate_runner.rs` | rejection 注入 producer；耗尽发 configured topic | assessment mismatch 有现成恢复链 | 高 |
| E14 | `crates/ralph-core/src/config/state_projection.rs` | 现有 EnsureTaskBatch/CloseTask；P0 计划新增 CloseTaskBatch | accepted assessment 可复用 P0 batch close | 高 |
| E15 | `presets/en/parallel-forge.yml` | supervisor+wave、isolated、task projection、artifact-first 已存在 | P1 只加前置 reconcile 和 resume route | 高 |
| E16 | `presets/schemas/parallel-forge.yml` | schema 是 event required-fields SSOT | 新 topic/fields 必须同步 | 高 |
| E17 | `presets/templates/parallel-forge/README.md` | 明确 producer、路径、先 materialize | 新复杂表必须进入 templates | 高 |
| E18 | `builtin_artifact_templates.rs` / `build.rs` | template basename 显式双注册 | 两处及 materialization tests 必改 | 高 |
| E19 | `crates/ralph-core/tests/scenarios.rs::run_workflow_guard_scenario` | 真实 EventLoop BDD 已用于 parallel_forge fixtures | reuse 拓扑必须用此入口 | 高 |
| E20 | P0 plan Product/Planning Contract | 锁定 static waves、settlement、failure correction、task close 时点 | 本计划按 P0 checkpoint 恢复 | 高 |
| E21 | Git `756f9ffa` | archive reuse 明确为 advisory，旧失败不消耗新 budget | failure experience 与 verdict 分离 | 高 |
| E22 | redrive solution | fresh boot 不消费残留；descriptor 缺失/digest conflict fail closed | reuse manifest 缺失/冲突同样 fail closed | 高 |
| E23 | SQLite concurrency solution | strict 中间态不可当成功；WAL/SHM 是 DB 状态一部分 | archive 三文件且不放宽状态机 | 高 |
| E24 | `agent_doc_sync::compute_sha256_hex` 等现有用法 | 仓库已有 SHA-256 helper/pattern | 不引入 hashing dependency | 高 |
| E25 | AGENTS.md preset hard rules | schema、runtime、BDD、docs、operator skills 有固定同步清单 | U4–U8 文件/验证范围不可省略 | 高 |
| E26 | `crates/ralph-core/src/task_store.rs::TaskStore::{load,all,with_exclusive_lock}` | shared lock只读live tasks；exclusive API在同锁内reload/modify/save | inspect可经公开TaskStore取identity，prefix action可在现有原子边界实施 | 高 |
| E27 | `crates/ralph-e2e/src/scenarios/parallel_forge.rs`、`crates/ralph-e2e/src/main.rs::get_all_scenarios`、`cassettes/e2e/README.md` | Parallel Forge E2E 当前只是固定 scenario ID 的失败占位壳；未注册到 runner，marker cassette/harness 仅有书面 wire contract | P0 完成门禁必须证明该 scenario 已注册且 filtered mock run 真通过；P1 只能扩展已落地的 scenario/cassette，不得把“全量 E2E 退出 0”误当成覆盖 Parallel Forge | 高 |

### 2.3 受影响范围

**当前已确认文件**

- 通用 archive：`crates/ralph-core/src/worktree.rs`
- reuse CLI 启动：`crates/ralph-cli/src/commands/run.rs`
- inspect CLI：`crates/ralph-cli/src/commands/inspect.rs`、
  `crates/ralph-cli/src/main.rs`
- preset/schema：`presets/en/parallel-forge.yml`、
  `presets/schemas/parallel-forge.yml`
- templates：`presets/templates/parallel-forge/`
- template embed/materialize：`crates/ralph-cli/src/builtin_artifact_templates.rs`、
  `crates/ralph-cli/build.rs`、
  `crates/ralph-cli/tests/integration_preset_materialize_artifacts.rs`
- preset structural tests：`crates/ralph-cli/src/presets.rs`
- state projection：`crates/ralph-core/src/config/state_projection.rs`、
  `crates/ralph-core/src/state_projector/mod.rs`、
  `crates/ralph-core/src/state_projector/tests.rs`
- BDD：`crates/ralph-core/tests/scenarios.rs`、
  `crates/ralph-core/tests/scenarios/parallel_forge_*.yml`
- agent docs：`crates/ralph-core/data/ralph-tools.md`、
  `crates/ralph-core/data/ralph-tools-cmdref.md`、
  `crates/ralph-core/data/ralph-tools-opac.md`
- preset operator references：
  `skills/ralph-preset-common/references/{agent-native-model,author-checklist,commands,finding-rubric,patterns}.md`
- domain/docs：`CONCEPTS.md`、`CLAUDE.md`、`AGENTS.md`。

**计划新增文件**

- `crates/ralph-core/src/reuse_status.rs`
- `crates/ralph-cli/tests/integration_reuse_status.rs`
- `presets/templates/parallel-forge/reuse-status-rubric.template.md`
- `presets/templates/parallel-forge/reuse-status.template.yml`
- `crates/ralph-core/tests/scenarios/parallel_forge_reuse_*.yml`

**不受影响**

- Web dashboard/API。
- supervisor schema/migrations/store；旧 store 只被归档，不被 reuse evaluator读取。
- 非 Parallel Forge preset 的 event topology。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| KTD1 | reuse 与 continue 是否共用状态 | 共用；完全分离 | 完全分离 | E2,E8,E9,E21 | 旧 runtime retry/active wave 不能跨新运行 | 0.99 |
| KTD2 | 旧 supervisor DB 怎么处理 | 原地复用；删除；连 WAL/SHM 归档 | 同 archive 归档 DB/WAL/SHM | E3,E9,E22,E23 | 原地复用污染 fresh run；删除丢证据 | 0.98 |
| KTD3 | archive 可消费条件与失败恢复 | 直接 rename；不可恢复 partial；lock+copy staging+manifest驱动清理+原子发布 | lock+可恢复 staging+原子 COMPLETE目录 | E3–E5,E22,E23 | 直接 rename 中途失败会把证据拆在 live/partial 两处；只看目录会误收 | 0.95 |
| KTD4 | 谁计算 reuse status | hat；runtime；runtime+hat解释 | runtime pure evaluator，hat只物化解释 | E10–E13,E21 | hat-only 可主观提升；runtime-only 缺 artifact workflow | 0.97 |
| KTD5 | CLI 放哪里 | 新顶级 reuse；inspect 子命令；diagnose | `ralph inspect reuse-status` | E11 | read-only语义与现有 inspect一致；diagnose 太重 | 0.95 |
| KTD6 | 状态集合 | bool；三态；五态 | 固定五态 | R5–R12,E20,E21 | bool/三态不能区分重验、继续修和矛盾阻塞 | 0.96 |
| KTD7 | Unit identity | unit_id；commit；合同 digest | unit_id + contract digest | E15–E20 | 名称/commit 单独都不能证明当前意图相同 | 0.95 |
| KTD8 | DAG reuse 粒度 | 任意 Unit；整 plan；连续 wave prefix | Unit评估、wave保守聚合、只接受连续 prefix | E14,E15,E20 | 任意跳过破坏依赖；整 plan 浪费可证明工作 | 0.98 |
| KTD9 | 部分 wave task 何时关闭 | 可复用 Unit立即关；整 wave当前运行 settlement | 非完整 reusable wave 零 task 提前关闭 | E14,E20 | 依赖可消费边界是 wave settlement | 0.99 |
| KTD10 | 从哪里继续 | 永远 executor；固定 correction；最早未证明 checkpoint | 固定 checkpoint 顺序取最早值 | E20,E21 | 全重跑浪费；固定 correction 不覆盖 stale verify/plan变化 | 0.96 |
| KTD11 | 失败历史继承什么 | 全状态；只文本；结构化经验但重置计数 | fingerprint/证据/尝试继承，round/budget 重置 | E12,E13,E21 | 继承 verdict/预算把新运行变成旧运行延续 | 0.99 |
| KTD12 | event 前如何证明 | schema；LLM precheck；runtime结果+precheck重算 | runtime assessment digest + 现有 precheck独立重算 | E11–E13,E20 | schema 不检查事实；单次 hat判断不独立 | 0.94 |
| KTD13 | accepted reuse 如何影响 tasks | 伪造 slot done；逐 wave agent事件；新 prefix batch projection；手工 task CLI | 新 `CloseTaskWavePrefix` projection action按 wave拓扑原子关闭 | E14,E20,E25 | 伪造 supervisor terminal错误；逐 wave agent事件扩大重放窗口；P0单-wave action不能表达多wave原子前缀 | 0.93 |
| KTD14 | 模板数量 | prompt内大表；一个自由文档；rubric+machine YAML | 两个嵌入模板 | E17,E18,用户约束 | prompt复制漂移；单文档不能同时服务机器和人 | 0.99 |
| KTD15 | legacy archive | 尽量推断；全 rerun；blocked | blocked | E21,E22 | 无 digest 不能区分旧结论与真实 settlement | 0.91 |
| KTD16 | 通用化范围 | 所有 presets；Parallel Forge 专用；完全硬编码 | pure evaluator可复用，topology只 opt-in Parallel Forge | E15,E20,Anti-Pattern | 全平台超范围；全硬编码难测试 | 0.93 |
| KTD17 | 是否扩展 loop_inspect.v2 | 增可选字段并 bump；独立命令 | 独立命令 | E10,E11 | loop inspect 是当前 live state，不是 archive comparison | 0.94 |
| KTD18 | 是否新增依赖/DB migration | 新库/表；复用 serde_yaml/sha2/Git | 无新依赖、无迁移 | E18,E23,E24 | 现有能力足够且避免平台化 | 0.96 |

所有实施关键决策均 ≥0.85。若 P0 实际实现改变 E14/E20 的接口，相关 KTD 失去
证据基础，U1 必须停止并重新评估，不能只修改置信度数字。

## Planning Contract

### 高层技术设计

```text
--reuse-worktree cleanup
  │ archive old runtime + supervisor db/wal/shm
  │ write reuse-manifest.v1.json
  └ atomic COMPLETE
        │
        ▼
P0 planner + guardian freeze current execution-plan digest
        │
        ▼
reuse-reconciler calls:
ralph inspect reuse-status --execution-plan ... --format json
        │
        ├─ pure evaluator:
        │   archive integrity → plan identity → Git reachability
        │   → per-unit five-state → DAG propagation
        │   → per-wave checkpoint → continuous reusable prefix
        │
        ├─ materialize reuse-status.yml
        └─ fill reuse-evidence.md from shared rubric
        │
        ▼
forge.reuse.assessed.proposed
        │
        ▼
precheck gate reruns same command and compares all digests/counts
        │
        ├─ reject → reconciler recompute (≤3)
        ├─ exhaust → plan.blocked
        └─ accept → forge.reuse.assessed
                     │
                     ├─ atomic close reusable prefix tasks
                     ├─ project verified base + checkpoint
                     └─ route first unproven wave into P0
```

### 状态格与 checkpoint

公开 `status` 不直接等价于执行入口；`reverify` 另带 checkpoint。wave 的有效入口
按下表取所有 Unit 的最早值：

| Unit status | 默认 checkpoint | 可接受证据 | 不满足时降级 |
|---|---|---|---|
| `reusable` | `settlement` | current contract + accepted settlement + verified SHA + ancestry + digest + dependency closure | `reverify`/`blocked` |
| `reverify` | `review` / `integration` / `verification` | implementation commit 与合同仍有效；从最早缺失 artifact 开始 | `rerun`/`blocked` |
| `resume_correction` | `correction` | current contract + matching unresolved failure fingerprint + readable evidence | `rerun`/`blocked` |
| `rerun` | `execute` | 无可接受旧实现，或合同/依赖变化 | 不自动提升 |
| `blocked` | `blocked` | hard contradiction | 只能人工消除矛盾后重算 |

checkpoint 顺序固定：

```text
execute < correction < review < integration < verification < settlement
```

### Archive manifest 合同

`reuse-manifest.v1.json` 至少包含：

| 字段 | 来源 | 规则 |
|---|---|---|
| `schema_version` | runtime 常量 | 固定 `reuse_manifest.v1` |
| `archive_id` | timestamp 目录名 | 与目录 basename 完全相同 |
| `previous_loop_id` | archived current-loop-id/registry | 缺失可为 null，但要 reason |
| `previous_head_sha` | cleanup 前 `git rev-parse HEAD` | full 40-char SHA |
| `plan_baseline_sha` | preserved marker | full SHA 或 null |
| `created_at` | UTC | RFC3339 |
| `members[]` | 实际归档成员 | repo-relative archive path、size、sha256 |
| `supervisor_members[]` | DB/WAL/SHM 子集 | present files 必须全在 members |
| `source_plan_path` | loop anchor/prompt | 可选 repo-relative |
| `source_plan_digest` | source plan | 有 path 时必须有 digest |
| `archive_digest` | canonical manifest payload | 不含自身 digest 字段后计算 |

cleanup 先以 create-new lock 占有当前 worktree reuse 边界，再在
`.ralph/reuse-history/.staging-<archive-id>/` 复制并 hash 全部成员。manifest
完成后按其 members 清理 live paths；中途失败保留 staging，下一次持锁调用先验证
已有 member digest，再补复制或继续删除。全部 live targets 消失后，在 staging
内写 `COMPLETE`（内容为 `archive_digest` 和 schema version），flush/sync 后把
整个 staging 目录原子 rename 为 `<archive-id>/`。最终目录没有 `COMPLETE` 或仍以
`.staging-` 命名时永远不可消费。

### Reuse assessment 合同

`reuse_status.v1` JSON/YAML 至少包含：

- schema_version、plan_key、execution_plan_path/digest、approved_base_commit；
- archive_id/manifest_digest；
- current_head_sha、plan_baseline_sha；
- assessment_digest、assessed_at；
- unit_count/wave_total；
- `units[]`：identity digest、status、reason_codes、checkpoint、evidence refs、
  commit SHA、prior failure summary；
- `waves[]`：unit IDs、effective_status、resume_checkpoint、settlement SHA、
  fully_reusable；
- `reusable_prefix_wave_count`、`reused_task_ids`、`reused_unit_ids`；
- `reused_waves[]`：按 wave_index 排序的 unit IDs/task IDs 分组；
- `resume_wave_index`、`resume_checkpoint`、affected/preserved Unit IDs；
- blocked reasons、reuse_scope_key。

assessment digest 对上述语义字段 canonical JSON 计算，不包含 `assessed_at` 和输出
文件 path，确保同一事实重算得到同一 digest。

首次运行没有 archive 时，`archive_id` 和 `archive_manifest_digest` 使用公开
sentinel `"none"`，reason code 为 `no_prior_archive`。全 wave reusable 时，
`resume_wave_index = wave_total + 1`、`resume_checkpoint = settlement`；dispatcher
据此发布 P0 `forge.exec.development.done`，不得构造一个不存在的 wave。
`reusable_prefix_wave_count = 0` 时，`verified_base_commit` 必须等于 CLI 参数和
Guardian trigger 共同提供的 `approved_base_commit`；prefix 非零时等于最后一个
reused wave 的 verified settlement SHA。

### 事件合同

P0 落地后新增：

| Topic | 生产者 | 消费者 | 语义 |
|---|---|---|---|
| `forge.reuse.assessed` | reuse-reconciler，经 precheck desugar 后由 gate accepted | state projector、forge-dispatcher、reporter projection | 当前运行接受的 reuse assessment |
| `forge.reuse.assessed.rejected` | synthetic precheck gate | reuse-reconciler | digest/count/evidence 不一致 |

`forge.reuse.assessed` required fields：

- `plan_key`
- `execution_plan_path`
- `execution_plan_digest`
- `reuse_status_path`
- `reuse_evidence_path`
- `assessment_digest`
- `archive_manifest_digest`
- `current_head_sha`
- `approved_base_commit`
- `reuse_scope_key`
- `unit_count`
- `wave_total`
- `reusable_prefix_wave_count`
- `reused_waves`
- `reused_task_ids`
- `reused_unit_ids`
- `verified_base_commit`
- `resume_wave_index`
- `resume_checkpoint`
- `affected_unit_ids`
- `preserved_unit_ids`
- `blocked`

`blocked: true` 时不得发 success topic；reconciler 写 block artifact 并发
`forge.plan.blocked`。因此 accepted topic 永远是可路由 assessment。

### 幂等键

```text
reuse_scope_key =
sha256(plan_key + execution_plan_digest + approved_base_commit + archive_manifest_digest)
```

同 scope + 同 assessment digest 是 replay；同 scope + 不同 digest 是冲突，除非
前一次只是 `.rejected` 且未产生 accepted projection。accepted 后 HEAD/plan 变化
必须开新运行或新 scope，不能覆盖已投影事实。

### 复杂 DAG 规则

1. 先按 current execution plan 建图并验证 P0 static waves。
2. 用 Unit contract digest 与 archive Unit 对应；缺旧 Unit 为 `rerun`。
3. 先做局部证据分类，再按拓扑序传播：
   - 任一依赖 `blocked` → 当前 Unit `blocked`；
   - 任一依赖非 `reusable` → 当前 Unit 不得 `reusable`；
   - 合同 changed 的 Unit 及其实现依赖它的后继至少 `rerun`；
   - 仅验证时效变化不自动让独立后继重跑，但它们不能进入 reusable prefix。
4. 每 wave 汇总最早 checkpoint。
5. 从 wave 1 扫描 fully_reusable；遇到第一个 false 立即停止 prefix。
6. 后续 archive settlement 只能作为 preserved history，不能关闭当前 task。

### Documentation / Operational Notes

- 新命令进入 `ralph-tools.md`/`cmdref`/`opac`，按“agent 下一步做什么”描述。
- preset author/review references 加 reuse event、required fields、template、
  precheck 和 no-ledger-read 规则。
- `CONCEPTS.md` 增 `reuse status`、`reusable prefix`、`resume checkpoint`。
- Parallel Forge builtin 描述变化时同步 `CLAUDE.md`/`AGENTS.md`；二者保持一致。
- 不新增 builtin preset 名，因此 zsh completion 名单无需改；仍需检查脚本无描述
  绑定漂移。

## 4. BDD 行为规格

Product Contract 的 S1–S14 是完整 BDD SSOT。实施时拆成至少以下真实 runtime
fixtures，不允许 source-text assertion：

- `parallel_forge_reuse_no_archive_runtime.yml`：S1。
- `parallel_forge_reuse_prefix_runtime.yml`：S2、S11。
- `parallel_forge_reuse_partial_wave_runtime.yml`：S3、S5、S6。
- `parallel_forge_reuse_correction_runtime.yml`：S4。
- `parallel_forge_reuse_tamper_runtime.yml`：S7、S8、S9、S12。
- CLI/worktree integration 单独覆盖 S10。
- artifact materialize integration 单独覆盖 S13。
- `ralph-e2e` marker cassette 单独覆盖 S14。

每个 fixture 由 `crates/ralph-core/tests/scenarios.rs` 的
`run_workflow_guard_scenario` 驱动真实 EventLoop；不得用 `run_scenario` stub。

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
|---|---|---|---|---|---|
| S1 | no archive→全 rerun、零 DB open | new evaluator tests + CLI integration + BDD | 单元+集成 | Characterization | 否 |
| S2 | prefix batch close、base推进、从 wave3开始 | state projector + BDD | 集成 | State-machine/Idempotency | 是，mock |
| S3 | partial wave零提前关闭、只 rerun affected | evaluator + BDD | 单元+集成 | DAG property cases | 否 |
| S4 | prior failure注入、round重置 | evaluator + correction BDD | 集成 | Differential | 否 |
| S5 | stale verify选最早checkpoint | evaluator table tests | 单元 | Mutation-style checkpoint deletion | 否 |
| S6 | non-contiguous settlement downgrade | evaluator + BDD | 单元+集成 | Property-Based DAG | 否 |
| S7 | digest mismatch blocked零副作用 | archive/evaluator/state projector | 单元+集成 | Fault Injection | 是，mock |
| S8 | HEAD漂移 precheck reject | precheck runtime + BDD | 集成 | Concurrency/Fault Injection | 否 |
| S9 | 第三次 exhausted blocked | precheck BDD | 集成 | boundary 2/3/4 | 否 |
| S10 | DB/WAL/SHM归档且 fresh inspect无旧 wave | worktree unit + CLI integration | 集成 | crash-window | 是，mock |
| S11 | same digest replay无重复变化 | projector/evaluator | 单元+集成 | Idempotency | 否 |
| S12 | legacy archive blocked | CLI integration | 集成 | malformed/fuzzed manifest | 否 |
| S13 | binary物化两份模板且字段对齐 | materialize integration | 集成/契约 | registry parity | 否 |
| S14 | settled prefix→correction→终局报告完整闭环 | registered marker-cassette scenario | mock E2E | activation cursor/group completeness | 是，mock |

**共同断言**

- 具体断言：status、reason code、checkpoint、digest、task batch、verified SHA、
  event sequence 完全匹配。
- 副作用断言：proposal/reject/blocked 零 task close；inspect 零文件创建；
  cleanup 不改 Git HEAD/branch/plan baseline。
- 不变量：不能越过 first unproven wave；旧 budget 不继承；Verifier/Tester 不写
  code；不读取旧 supervisor store 作为状态。
- 测试层级理由：分类/DAG 是纯规则，用单元/属性测试；archive/CLI/Git 用集成；
  event routing/state mutation 用真实 EventLoop BDD；关键 operator flow 用 mock E2E。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
|---|---|---|---|---|---|---|---|
| R1–R4 | archive与运行边界 | S10,S12 | reuse cleanup integration | worktree cleanup | integration_worktree_isolation | S10 | E1–E9,E23 |
| R5–R12 | 五态与identity | S1,S3–S7,S12 | inspect JSON contract | reuse_status module | integration_reuse_status | S7 | E7,E11,E20–E24 |
| R13–R19 | DAG/prefix/checkpoint | S2–S6,S11 | runtime BDD | DAG/state table | scenarios + projector | S2 | E14,E15,E19,E20 |
| R20–R24 | runtime+precheck+幂等 | S7–S9,S11 | precheck BDD | digest/replay | real EventLoop | S7 | E12–E14,E22 |
| R25–R27 | 模板SSOT | S13 | materialize integration | template registry | CLI materialize | 否 | E17,E18 |
| R28–R30 | inspect/report/docs | S1,S2,S4,S7–S9,S13,S14 | CLI JSON/human + report BDD | serialization | presets/docs drift | S14 | E10,E11,E25,E27 |

Scenario→Unit 的执行归属固定为：

| Scenario | Owner Unit |
|---|---|
| S1 | U2（分类）+ U4（正常路由） |
| S2 | U4 |
| S3–S6 | U5 |
| S7–S8 | U6 |
| S9 | U7 |
| S10 | U1 |
| S11 | U4 |
| S12 | U1（archive形态）+ U2（分类）+ U6（终局路由） |
| S13 | U3 |
| S14 | U8 |

不存在无 Unit、无测试或无 Evidence 的需求。

## 7. 严格串行开发单元

```text
U1 Archive 完整性与 supervisor 隔离
  ↓ 完成全部测试、重构和回归
U2 Runtime 五态 evaluator 与 inspect CLI
  ↓ 完成全部测试、重构和回归
U3 Reuse templates 的嵌入与物化
  ↓ 完成全部测试、重构和回归
U4 Accepted reusable prefix 的 precheck 与原子投影
  ↓ 完成全部测试、重构和回归
U5 Partial wave 的 checkpoint 恢复与失败经验
  ↓ 完成全部测试、重构和回归
U6 篡改与漂移证据 fail-closed
  ↓ 完成全部测试、重构和回归
U7 Precheck exhaustion 阻塞闭环
  ↓ 完成全部测试、重构和回归
U8 Mock E2E、agent guides 与下游合同闭合
```

## Implementation Units

### U1：生成完整 archive manifest 并隔离旧 supervisor ledger

#### 1. Unit 目标

操作者复用 completed worktree 时，新运行只能看到 clean live runtime；旧 runtime
、Parallel Forge business artifacts 和 supervisor DB 三件套被归档为一个可机器
核验、完成标记原子提交的证据包。

#### 2. 对应需求与 Scenario

- Requirements：R1–R4。
- Scenario：S10、S12 的 archive 前置部分。
- Decisions：KTD1–KTD3、KTD18。
- Evidence：E1–E9、E22、E23。

#### 3. 外部可观察结果

`--reuse-worktree` 成功后，latest archive 有 manifest+COMPLETE；live
`.ralph/supervisor.db{,-wal,-shm}` 与旧 `.ralph/forge/` 不存在；Git HEAD、
branch、user code 和 plan baseline 不变。两个进程同时申请同一 worktree reuse
时只有持有 cleanup lock 的进程可继续，另一个明确失败且零 archive/live
mutation。

#### 4. 当前行为基线

E3–E6 证明当前 cleanup 会 archive 多数 runtime 文件并写 advisory
`resume-context.md`，但 supervisor DB 和 machine manifest 缺失。先扩展现有
characterization test，固定当前保留/删除集合，再写新断言形成 Red。

#### 5. 输入与输出

- 输入：completed worktree path、cleanup 前 HEAD、现有 runtime paths。
- 输出：archive dir、`reuse-manifest.v1.json`、`COMPLETE`、
  保留的 `resume-context.md`。
- 错误：git HEAD 读取失败、path escape、copy/hash/write/sync/delete 失败均返回
  `WorktreeError`，新 run 不启动。
- 不变量：不删除旧 archive；不修改 Git refs；不打开 supervisor DB。

#### 6. 修改位置

- `crates/ralph-core/src/worktree.rs`：当前 archive owner；新增 manifest DTO、
  canonical member hashing、supervisor sidecars、complete marker 和 tests。
- `crates/ralph-cli/src/commands/run.rs`：只消费 richer cleanup result 并打印
  archive/manifest summary；不承载分类逻辑。
- `crates/ralph-cli/tests/integration_worktree_isolation.rs`：真实 CLI reuse 验证。

不修改 supervisor store/migrations；archive 是文件生命周期操作。

#### 7. 可依赖能力

- 现有 target 命名/筛选逻辑可从 `unique_reuse_archive_dir`、
  `archive_if_exists`、`archive_files_matching` 提取；`archive_if_exists` 的直接
  rename 语义必须被 discover→copy→verify→delete 取代，不能在 staging完成前
  移走唯一 live copy。
- `agent_doc_sync::compute_sha256_hex` 或现有 sha2 dependency。
- `plan_baseline::read_plan_baseline`、`git_ops::get_head_sha`。

#### 8. 禁止依赖的未来能力

不得依赖 U2 evaluator、U3 templates、P1 events；本 Unit 只建立可信来源。

#### 9. 验收测试

- `test_clean_worktree_runtime_artifacts_writes_complete_manifest`。
- `test_clean_worktree_runtime_artifacts_archives_supervisor_sidecars`。
- `test_cleanup_failure_leaves_resumable_staging_and_refuses_current_start`。
- CLI integration：填充 `.ralph/forge/<plan-key>/` 和 supervisor DB/WAL/SHM 后
  reuse，断言 live clean、archive members/digests、HEAD/baseline/code preserved。
- 运行：
  `cargo nextest run -p ralph-core -- worktree::tests::test_clean_worktree_runtime_artifacts`
  和
  `cargo nextest run -p ralph-cli --test integration_worktree_isolation -- reuse`。

#### 10. Acceptance Red

先在现有 cleanup test fixture 创建 DB/WAL/SHM，并断言 archive manifest/COMPLETE
存在。当前实现既不移动三文件，也不写 manifest，因目标能力缺失而 Red。
环境错误、git fixture 未初始化、fixture 路径错不算有效 Red。

#### 11. 单元测试拆分

1. manifest member path 只能是 archive-relative regular file。
2. member size/digest 与真实 bytes 相同。
3. canonical manifest digest 排除自引用字段。
4. COMPLETE 在全部 member 复制、校验且 live target 全部清理后才存在。
5. 任一 copy/hash/delete failure 不产生 COMPLETE。
6. DB absent 时不虚构 members；任一 sidecar present 时正确归档。
7. `.ralph/forge/` 整棵旧 artifact tree 进入 archive，live path 被清空。
8. cleanup 二次调用产生独立 archive，不覆盖旧 archive。
9. resume-context 指向 latest complete archive。
10. 同一 staging 在 copy、delete、COMPLETE 前三个失败窗口都可继续。
11. 并发 cleanup 只有一个持锁者。

不允许 Mock Git HEAD 或真实 copy/delete/rename；只对时间/ID生成使用
deterministic seam。

#### 12. Red → Green → Refactor 顺序

```text
supervisor sidecar Red
→ 扩展归档集合
→ Green
→ manifest member/digest Red
→ copy-to-staging manifest writer
→ Green
→ partial failure/continue Red
→ manifest驱动 live cleanup + staging resume
→ Green
→ concurrent cleanup Red
→ create-new worktree-scoped lock
→ Green
→ COMPLETE目录原子发布 Red
→ staging内complete + directory rename
→ Green
→ CLI reuse Red
→ 接线 richer result
→ Green
→ Refactor 共用 member collector
```

#### 13. 最小实现范围

必须归档当前已列 runtime artifacts、`.ralph/forge/` 和 DB/WAL/SHM；必须 lock/staging/
version/digest/complete；必须保留 advisory context。不得读取 DB 内容、做
migration、压缩 archive 或删除历史 archive。

#### 14. 集成验证

用 temp Git repo 和真实 worktree运行 cleanup/CLI；真实比较 `git rev-parse HEAD`、
plan baseline、user file、archive bytes。不得 fake filesystem rename。

#### 15. 风险驱动测试

- Fault Injection：copy 中、manifest 后、部分 live delete 后、COMPLETE 前失败。
- Idempotency：同一 worktree 连续两次 reuse。
- Concurrency：两个进程同时 cleanup 同一 worktree。
- Fuzz/validation：`../`、symlink、非法 SHA/member path。
- SQLite lifecycle：DB-only、DB+WAL、DB+SHM 三组合。

#### 16. 回归范围

现有 worktree create/list/remove/sync、exact reuse、symlink preservation、
plan baseline、fresh worktree create。原因：cleanup result/保留集合发生变化。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/worktree.rs` | 修改现有生产/测试 | archive manifest + DB hygiene | E1–E5,E23 |
| `crates/ralph-cli/src/commands/run.rs` | 修改现有生产文件 | 消费/显示完整 archive | E2 |
| `crates/ralph-cli/tests/integration_worktree_isolation.rs` | 修改集成测试 | S10 | E6 |

#### 18. 完成标准

当前 Scenario、单元/集成/回归、build/clippy/fmt 通过；无 `.skip/.only`；旧 archive
不丢失；Evidence 更新；可独立提交。

#### 19. 停止条件

发现 supervisor DB path 可配置到 `.ralph/` 外、cleanup 与 live supervisor
进程并发、或 rename 后无法定义可恢复事务边界时停止，重新调查 exact target/
locking；不得用 recursive delete 或把 partial archive标 COMPLETE。

#### 20. 风险与注意事项

最大风险是 manifest 承诺完整但 cleanup 中途失败。检测靠 COMPLETE 最后提交和
fault injection；剩余风险是 fsync 在不同文件系统语义差异，必须在 plan支持平台
上用原子 rename/create-new，不声称跨文件系统事务。

### U2：提供 runtime 五态 evaluator 与只读 inspect CLI

#### 1. Unit 目标

操作者和 hat 能用一个只读命令获得对当前 plan/Git/archive 的确定五态 assessment，
相同输入得到相同 digest。

#### 2. 对应需求与 Scenario

- R5–R19、R28；S1、S3–S7、S12。
- KTD4–KTD11、KTD15–KTD18。
- E7、E10、E11、E20–E24。

#### 3. 外部可观察结果

`ralph inspect reuse-status --execution-plan <path>
--approved-base-commit <sha> --format json` 返回 `reuse_status.v1`；human 输出来自
同一 DTO；命令不创建文件、不启动 loop/backend/supervisor。

#### 4. 当前行为基线

`ralph inspect` 只有 profiles/loop/prompt；没有 archive comparison。当前 archive
无 manifest 的基线由 U1 改变。先写 CLI parse/JSON contract test，当前缺
subcommand 而 Red。

#### 5. 输入与输出

- 输入：root、execution plan path、Guardian 批准的 full
  `approved_base_commit`、可选 explicit archive id；默认 latest complete
  archive；经 `TaskStore::load/all` 读取 current plan 的 live task identity。
- 输出：完整 assessment DTO/JSON/human。
- 首次运行：全 rerun。
- 错误：current execution plan 本身 invalid 是 CLI error；历史矛盾是
  successful JSON with `blocked`，便于流程报告。
- 不变量：不写 artifact；不读取 live supervisor DB。

#### 6. 修改位置

- 计划新增 `crates/ralph-core/src/reuse_status.rs`：DTO、manifest reader、
  contract digest、Git evidence adapter input、五态/DAG/checkpoint pure evaluator。
- `crates/ralph-core/src/lib.rs`：公开 agent-safe API。
- `crates/ralph-cli/src/commands/inspect.rs`：新增 args、解析、Git/file adapter、
  human/JSON rendering。
- `crates/ralph-cli/src/main.rs`：Clap route/parse tests。
- 计划新增
  `crates/ralph-cli/tests/integration_reuse_status.rs`，使用
  `tests/common::ralph_bin()` scrub 外层 hat env。

#### 7. 可依赖能力

U1 manifest、P0 execution plan/settlement合同、serde/serde_yaml/serde_json、
sha2 helper、git command pattern、inspect output模式。

#### 8. 禁止依赖的未来能力

不得写 U3 artifact或发事件；不得关闭 task；不得依赖 U4 precheck。

#### 9. 验收测试

- JSON schema/version/field contract。
- no archive/legacy/incomplete/tampered。
- per-state table和 checkpoint。
- complex DAG：diamond、multi-root、10+ waves、非连续旧 settlement。
- current dirty paths 单 owner、多 owner、无 owner。
- unreachable/conflicting SHA。
- CLI read-only：before/after filesystem tree除 access time 外一致。
- 命令：
  `cargo nextest run -p ralph-core -- reuse_status`
  和
  `cargo nextest run -p ralph-cli --test integration_reuse_status`。

#### 10. Acceptance Red

新增 CLI integration 调用 subcommand 并断言 JSON schema。当前 Clap 报 unknown
subcommand，证明外部能力缺失；错误 command 拼写或无 binary 不算有效 Red。

#### 11. 单元测试拆分

1. Unit contract canonicalization 不受 YAML map key 顺序影响。
2. 任一合同字段变化改变 digest。
3. status 五态表逐行测试。
4. checkpoint minimum。
5. transitive dependency downgrade。
6. continuous prefix stop-on-first-gap。
7. archive selection只取 COMPLETE，timestamp相同用 archive_id稳定排序。
8. assessment digest排除 assessed_at/path。
9. prior failure summary不含旧 round/budget verdict。
10. public DTO不序列化 internal ledger/raw events。

Git adapter必须真实运行 temp repo ancestry/diff；pure evaluator可用 fixture input，
不 Mock 目标状态规则。

#### 12. Red → Green → Refactor 顺序

```text
CLI parse/JSON Red
→ 最小 subcommand + no_archive DTO
→ Green
→ contract digest Red
→ canonical identity
→ Green
→ 五态表 Red
→ evaluator
→ Green
→ DAG/prefix Red
→ topology propagation
→ Green
→ Git/archive hard gates Red
→ adapters
→ Green
→ human/JSON parity
→ Refactor
```

#### 13. 最小实现范围

只读 evaluator/CLI；不自动修复 archive、不运行 tests、不推断 legacy。reason
codes 使用本计划固定枚举；unknown evidence 默认 blocked，不加 `Unknown` 第六态。

#### 14. 集成验证

temp Git repo 构造真实 commits/branches/diffs和 U1 archives；CLI JSON反序列化为
测试 DTO；运行两次比较 assessment digest；检查无新 supervisor DB/文件。

#### 15. 风险驱动测试

- Property-Based：随机 DAG 的 prefix/依赖不变量。
- Fuzz：manifest/YAML truncated、duplicate IDs、超长 path。
- Differential：相同 semantic YAML不同 key 顺序同 digest。
- Performance：100 Units/500 artifacts 2 秒门禁。

#### 16. 回归范围

inspect profiles/loop/prompt parse与输出、loop_inspect.v2、plan baseline、Git helper、
hat env scrub。原因：新增同 namespace subcommand/共享 imports。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/reuse_status.rs` | 新增生产/单元测试 | evaluator SSOT | E20–E24 |
| `crates/ralph-core/src/lib.rs` | 修改公开模块 | 暴露 DTO/API | E10 |
| `crates/ralph-cli/src/commands/inspect.rs` | 修改生产/测试 | read-only CLI | E11 |
| `crates/ralph-cli/src/main.rs` | 修改 CLI parse tests | route | E11 |
| `crates/ralph-cli/tests/integration_reuse_status.rs` | 新增集成测试 | Git/archive/CLI | E6,E11 |

#### 18. 完成标准

五态/复杂 DAG/CLI parity/read-only/performance 全绿；schema/reason codes稳定；无
新依赖；相关回归通过；Evidence/KTD仍 ≥0.85；可独立提交。

#### 19. 停止条件

P0 execution plan 无法提供稳定 Unit contract字段、Git ancestry 不能锚定
settlement SHA、或 evaluator需要读 supervisor DB 才能决定时停止。不得降低为
hat猜测或读取内部 ledger。

#### 20. 风险与注意事项

最大风险是 canonical digest 漂移。检测靠 map-order differential与全部字段
mutation；缓解为独立 canonical DTO，不对原 YAML bytes直接 hash。

### U3：嵌入并物化 reuse status 复杂模板

#### 1. Unit 目标

binary-only 安装环境也能物化一份机器 status 模板和一份统一复杂决策 rubric，
reconciler/precheck/reporter 不再在 prompt 内复制状态表。

#### 2. 对应需求与 Scenario

- R25–R27；S13，并为 S2–S9 提供证据载体。
- KTD14、KTD18。
- E17、E18、用户明确约束。

#### 3. 外部可观察结果

materialize parallel-forge 后 templates 目录包含
`reuse-status.template.yml` 和 `reuse-status-rubric.template.md`；README 列出
producer/consumer/output path；重复 materialize 与现有语义一致。

#### 4. 当前行为基线

registry 只有六个 Parallel Forge 模板；P0 完成后会再增加四个。测试先以 P0
落地后的 registry 为基线，再断言本计划两个 basename，当前缺失而 Red。

#### 5. 输入与输出

- 输入：preset name、plan key。
- 输出：两个原样嵌入模板。
- 错误：registry/build copy/README 不一致应在 build/test fail。
- 不变量：不覆盖用户已填写业务 artifacts；模板不含具体 plan实例。

#### 6. 修改位置

- 新增两个 template files。
- 更新 `presets/templates/parallel-forge/README.md`。
- 更新 `crates/ralph-cli/build.rs` copy list。
- 更新 `crates/ralph-cli/src/builtin_artifact_templates.rs` names/includes/tests。
- 扩展 `integration_preset_materialize_artifacts.rs`。

#### 7. 可依赖能力

现有 materialize command、U2 DTO字段、P0 templates registry。

#### 8. 禁止依赖的未来能力

不得修改 preset topology/state projection；模板只定义合同。

#### 9. 验收测试

- registry count/names。
- binary materialize 输出两个文件。
- YAML template 可 parse，含所有 reuse_status.v1 语义字段。
- rubric 含五态、checkpoint、hard block、DAG downgrade、prior failure、
  precheck failed_checks 表。
- 不断言完整 prompt/模板 byte equality。
- 命令：
  `cargo nextest run -p ralph-cli -- builtin_artifact_templates`
  和
  `cargo nextest run -p ralph-cli --test integration_preset_materialize_artifacts`。

#### 10. Acceptance Red

materialize integration 断言两个 basename存在；当前 registry不认识而失败。构建
环境损坏或 OUT_DIR stale 不算有效 Red。

#### 11. 单元测试拆分

1. template name registry与include count。
2. YAML required keys/enum/checkpoint parse。
3. rubric稳定章节 marker而非全文。
4. README producer/consumer/path结构。
5. unsafe plan key仍拒绝 path traversal。

#### 12. Red → Green → Refactor 顺序

```text
materialize basename Red
→ build copy/include registry
→ Green
→ YAML contract Red
→ 完整 machine template
→ Green
→ rubric contract Red
→ 五态/硬门/failed_checks表
→ Green
→ README + registry parity
→ Refactor
```

#### 13. 最小实现范围

只加两模板。rubric 是 reference SSOT，不直接作为业务 artifact；reconciler 将其
复制/填写为 reuse-evidence.md。不得把整张表再复制进 hat instructions。

#### 14. 集成验证

运行真实 `ralph preset materialize-artifacts parallel-forge --plan-key reuse-demo`
到 temp root；解析 YAML并检查文件集合。

#### 15. 风险驱动测试

- Contract：U2 DTO字段与模板字段一一对应。
- Path traversal：恶意 plan key。
- Differential：源码 checkout和 embedded output结构相同，不用完整 byte lock。

#### 16. 回归范围

red-team/ce-pipeline template registries、现有 parallel-forge六个+P0四个模板、
materialize overwrite/path rules。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `presets/templates/parallel-forge/reuse-status.template.yml` | 新增模板 | machine artifact | E17,E18 |
| `presets/templates/parallel-forge/reuse-status-rubric.template.md` | 新增模板 | 复杂状态/证据SSOT | E17,用户约束 |
| `presets/templates/parallel-forge/README.md` | 修改文档 | lifecycle | E17 |
| `crates/ralph-cli/build.rs` | 修改 build | embed copy | E18 |
| `crates/ralph-cli/src/builtin_artifact_templates.rs` | 修改生产/测试 | registry/include | E18 |
| `crates/ralph-cli/tests/integration_preset_materialize_artifacts.rs` | 修改测试 | binary materialize | E18 |

#### 18. 完成标准

两个模板在 source/build/binary/runtime四层可见；结构化测试通过；无全文 prompt
锁定；P0 template集合不丢；可独立提交。

#### 19. 停止条件

若 P0 改用动态目录扫描而非显式 registry，停止并以实际实现更新变更位置；不得
同时保留两套注册机制。

#### 20. 风险与注意事项

主要风险是三处 basename drift。build-time explicit list + registry parity +
integration materialize 三层检测；README 仍需人工审查 producer/consumer。

### U4：通过 precheck 接受并原子投影 reusable 连续前缀

#### 1. Unit 目标

当前 plan 经 Guardian 批准后，reconciler 物化 runtime assessment；只有 precheck
独立重算一致时，`CloseTaskWavePrefix` 才按静态 wave 顺序在一个 task-store
exclusive lock 内关闭完整 reusable prefix 并推进 base。

#### 2. 对应需求与 Scenario

- R13–R16、R20–R24、R28；S2、S6、S8、S11。
- KTD8–KTD10、KTD12、KTD13、KTD16。
- E12–E20、E25。

#### 3. 外部可观察结果

事件序列出现 proposed→accepted；task list只关闭 prefix；projection含
verified base/resume wave；相同 digest replay无重复变化。

#### 4. 当前行为基线

P0 实现后 Guardian直接进入 wave 1 preparation；没有 reuse reconcile。先新增
two-wave-prefix BDD，预期旧 topology仍派发 wave1而 Red。

#### 5. 输入与输出

- 输入：Guardian approved current plan/digest、U2 assessment。
- 输出：filled status/evidence artifacts、accepted event、projection。
- 错误：blocked assessment发 `forge.plan.blocked`，不发 success。
- 不变量：gate 重算；proposal/reject零 task mutation；prefix之外全 open。

#### 6. 修改位置

- `presets/en/parallel-forge.yml`：新增 reuse-reconciler hat、precheck rule、
  Guardian→reuse→dispatcher flow、projection。
- `presets/schemas/parallel-forge.yml`：新增 topic required fields/field_docs。
- `crates/ralph-core/src/config/state_projection.rs`、
  `crates/ralph-core/src/state_projector/mod.rs` 和
  `crates/ralph-core/src/state_projector/tests.rs`：新增确定的
  `CloseTaskWavePrefix` action。输入为
  ordered `waves[]`、declared wave count、每 wave live task IDs；先验证 wave 从
  1 连续、每组 task exact/open、依赖只指向更早组或已 done task，再在一个 lock
  内按组关闭。verified base/resume metadata 由同 topic 的普通 projection set
  完成。
- `crates/ralph-cli/src/presets.rs`：结构化 topology/schema/projection tests。
- new BDD fixture/scenarios.rs。

#### 7. 可依赖能力

U1 archive、U2 command、U3 templates、P0 Ensure/CloseTaskBatch与TaskStore锁、
precheck desugar和 static dispatcher。

#### 8. 禁止依赖的未来能力

不得实现 partial wave/correction routing（U5）；本 Unit只处理 no archive 和
完整 reusable prefix，首个非reusable先按 execute继续。

#### 9. 验收测试

- no archive accepted with prefix=0 and wave1 execute。
- wave1/2 prefix accepted，一次 batch close，wave3 prepare。
- mismatched assessment digest rejected/producer resume。
- replay同 digest idempotent；different digest conflict。
-真实 EventLoop BDD + state projector tests。
- 命令：
  `cargo nextest run -p ralph-core -- state_projector`
  和
  `cargo nextest run -p ralph-core --test scenarios -- parallel_forge_reuse_prefix`
  及 preset tests。

#### 10. Acceptance Red

BDD archive提供两 wave settlements，断言 wave1/2无 exec.unit.ready且 wave3有。
P0-only flow没有 `forge.reuse.assessed`，会派发 wave1，正确 Red。

#### 11. 单元测试拆分

1. schema required fields。
2. precheck desugar producer/gate ownership。
3. reusable wave groups必须从1连续，task IDs live、全局唯一、exact prefix。
4. prefix close前全验证，任一invalid零写。
5. prefix verified SHA与最后 settlement一致。
6. resume wave=prefix+1或 all-settled sentinel。
7. projection replay same digest no-op；conflict reject。
8. blocked true不得走 success。

不 Mock TaskStore batch atomicity或 evaluator命令输出的 digest comparison。

#### 12. Red → Green → Refactor 顺序

```text
topology Red
→ reuse-reconciler + proposed/accepted route
→ Green
→ precheck digest Red
→ 独立重算 checklist/fields
→ Green
→ prefix batch projection Red
→ CloseTaskWavePrefix action chain
→ Green
→ replay/conflict Red
→ scope+digest idempotency
→ Green
→ two-wave BDD
→ Refactor prompt重复到rubric引用
```

#### 13. 最小实现范围

reconciler只运行 U2 inspect、复制 U3 templates、填 artifacts、emit。precheck读取
rubric并重跑同命令。accepted event只驱动 `CloseTaskWavePrefix` 和 resume
metadata；不伪造 `forge.wave.settled`或 supervisor events。

#### 14. 集成验证

真实 EventLoop、真实 TaskStore、fixture artifacts/Git；precheck gate使用mock
backend response但走真实 desugar/obligation/rejection；验证 events顺序和tasks。

#### 15. 风险驱动测试

- State-machine：proposal/rejected/accepted/replay/conflict。
- Idempotency：crash after task close before next dispatch。
- Concurrency：assessment后HEAD改变。
- Mutation-style：逐个删 digest/count/path，必须 rejected。

#### 16. 回归范围

P0 static dispatcher、CloseTaskBatch、precheck synthetic hats、multi-hat isolation、
topic ownership、single-event budget、existing parallel-forge success BDD。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改配置 | reconciler/precheck/flow | E12,E15,E20 |
| `presets/schemas/parallel-forge.yml` | 修改 schema | accepted event | E16,E25 |
| `crates/ralph-core/src/config/state_projection.rs` | 修改生产 | 新增 prefix action 配置 | E14,E20 |
| `crates/ralph-core/src/state_projector/mod.rs` / `tests.rs` | 修改生产/测试 | atomic wave-prefix projection | E14 |
| `crates/ralph-cli/src/presets.rs` | 修改结构化测试 | topology/schema | E16 |
| `crates/ralph-core/tests/scenarios/parallel_forge_reuse_prefix_runtime.yml` | 新增BDD | S2,S11 | E19 |
| `crates/ralph-core/tests/scenarios.rs` | 修改测试注册 | 真实runtime | E19 |

#### 18. 完成标准

prefix acceptance、precheck mismatch、atomicity、replay、P0 regression全绿；
schema/preset parity通过；无 text-only prompt tests；可独立提交。

#### 19. 停止条件

TaskStore现有锁边界无法承载 `CloseTaskWavePrefix` 的“先全验证、后按组关闭”、
precheck无法从公开命令重算、或 flow必须伪造 supervisor terminal才能跳prefix
时停止并重决策；不得逐个手工 task close。

#### 20. 风险与注意事项

最大风险是“accepted event 与 task batch不同步”。state projector必须先全量验证
再持锁写；event被拒绝时不得留下半关 tasks。

### U5：从 partial wave 的最早 checkpoint 恢复并注入失败经验

#### 1. Unit 目标

第一个非 reusable wave 不被全盘重做：系统按 wave effective checkpoint只运行
必要 Unit/阶段，保留成功 sibling证据，并让匹配失败经验进入新 correction。

#### 2. 对应需求与 Scenario

- R8–R19、R29；S3–S6。
- KTD6–KTD11。
- E15、E20、E21。

#### 3. 外部可观察结果

partial assessment 中可看到 affected/preserved Unit；dispatcher只派发 rerun Unit；
resume_correction进入 fixer；reverify从准确 gate开始；最终仍对整 wave
review/integrate/verify并产生 current-run settlement。

#### 4. 当前行为基线

P0 dispatcher只从一个 static wave的准备/exec开始；没有 reuse checkpoint route。
先写 partial/correction BDD，旧 flow会重跑全部或无法触发目标 hat，形成 Red。

#### 5. 输入与输出

- 输入：accepted assessment的 resume wave/checkpoint、affected/preserved units、
  prior failure summaries、P0 worktree/candidate refs。
- 输出：subset exec/correction handoff、preserved completion map、全 wave review/
  integrate/verify/current settlement。
- 错误：preserved commit/evidence在路由前失效→回 U2重算/blocked。
- 不变量：partial wave task全 open直到 current settlement；后继不启动。

#### 6. 修改位置

- Parallel Forge dispatcher、worktree、failure-handler、reviewer/integrator/verifier
  instructions/triggers/projection。
- schema required fields：resume checkpoint、affected/preserved units、prior failures。
- P0 execution-plan/reuse status templates引用，不复制状态表。
- BDD partial/correction fixtures。
- Reporter reuse summary。

#### 7. 可依赖能力

U4 accepted projection、P0 lazy worktree/candidate/correction/review chain、U2 prior
failure结构。

#### 8. 禁止依赖的未来能力

不得自动解决 semantic conflict；不得关闭 partial tasks；不得让 tester/verifier
修 code；不得继承旧 correction round。

#### 9. 验收测试

- mixed wave: one preserved + one rerun，只发一个 exec slot；review覆盖两 Unit。
- resume_correction：旧 fingerprint可见、round=1、不同策略、修后全 wave gate。
- reverify review/integration/verification 三 checkpoint table。
- downstream archived settlement因 dependency gap不被调度。
- current settlement后才关闭 wave tasks。

#### 10. Acceptance Red

partial fixture断言只 affected Unit出现 `exec.unit.ready`，preserved sibling无 slot
但出现在 review coverage。P0/U4 flow会派发whole wave或缺coverage，正确 Red。

#### 11. 单元测试拆分

1. effective checkpoint最小值。
2. affected/preserved partition完整、互斥。
3. subset wave payload count/slot index连续。
4. preserved evidence digest在review前复核。
5. correction context只带历史经验，不带旧 round/budget。
6. reviewer coverage=affected completions + preserved completions。
7. integration candidate以最后 verified base重建。
8. reverify checkpoint不能跳过更早缺失artifact。
9. partial settlement task IDs=整 wave。

不允许 Mock reviewer coverage或Git commit reachability。

#### 12. Red → Green → Refactor 顺序

```text
affected partition Red
→ dispatcher subset route
→ Green
→ preserved review coverage Red
→ completion map合并
→ Green
→ correction history Red
→ fresh round context
→ Green
→ checkpoint table Red
→ review/integration/verification routes
→ Green
→ current settlement Red
→ 回接P0完整chain
→ Green
→ Refactor共用 wave resume context
```

#### 13. 最小实现范围

只支持本计划五态/checkpoint；不增加任意 stage字符串。成功 sibling不执行新 slot，
但其 commit/evidence在current review重新审查；`rerun` Unit在当前代码基础上执行
完整TDD验证，不要求先回滚旧实现。

#### 14. 集成验证

真实 EventLoop BDD、supervisor subset fan-in、temp Git branches、P0 correction和
state projector；断言 event顺序、slot count、review coverage、task close时点。

#### 15. 风险驱动测试

- Complex state-machine：5 status × checkpoint组合。
- Fault Injection：preserved artifact在assessment后删除。
- Differential：uninterrupted P0与reuse recovery最终 tree/tasks相同。
- Concurrency：rerun Unit与preserved shared contract path冲突→blocked。

#### 16. 回归范围

P0 full-wave exec/fan-in、failure correction、review coverage、candidate promotion、
final Tester、reporter status。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改配置 | checkpoint routes/hat contracts | E15,E20 |
| `presets/schemas/parallel-forge.yml` | 修改 schema | resume context fields | E16 |
| `presets/templates/parallel-forge/reuse-status.template.yml` | 修改模板 | 若U5发现已决字段缺失 | U2,U3合同 |
| `presets/templates/parallel-forge/manager-report.template.md` | 修改模板 | reuse summary | E17,R29 |
| `crates/ralph-core/tests/scenarios/parallel_forge_reuse_partial_wave_runtime.yml` | 新增BDD | S3,S5,S6 | E19 |
| `crates/ralph-core/tests/scenarios/parallel_forge_reuse_correction_runtime.yml` | 新增BDD | S4 | E19 |
| `crates/ralph-core/tests/scenarios.rs` | 修改注册 | real EventLoop | E19 |

#### 18. 完成标准

三checkpoint、subset exec、fresh correction、整wave re-settlement、DAG barrier、
P0 regression全绿；模板/schema同步；可独立提交。

#### 19. 停止条件

若 preserved sibling无法由 current review重新证明、P0 supervisor要求whole-wave
slot count且不支持subset fan-in、或旧 candidate ref无法安全定位，停止并更新
P0/P1合同；不得合成假的 `exec.unit.done`。

#### 20. 风险与注意事项

最大风险是 subset fan-in 与 whole-wave review coverage混淆。二者必须使用不同
计数：supervisor expected slots只等于 affected Unit；review expected units等于
current execution plan整 wave。

### U6：篡改、legacy 与 proposal 漂移必须零副作用阻塞

#### 1. Unit 目标

任何无法核验的 archive、被篡改的 artifact 或 precheck 时发生的 HEAD/plan 漂移，
都只产生稳定的拒绝/阻塞证据，不关闭 task、不推进 verified base。

#### 2. 对应需求与 Scenario

- Requirements：R11、R20–R23。
- Scenarios：S7、S8、S12。
- Decisions：KTD12、KTD15。
- Evidence：E10–E13、E19、E21、E22。

#### 3. 外部可观察结果

`ralph inspect reuse-status` 返回 `blocked` 和稳定 reason code；真实 EventLoop
不发布 accepted `forge.reuse.assessed`，TaskStore 与 verified base 保持不变。

#### 4. 当前行为基线

U1–U5 已有 happy/partial 路径，但尚未用真实 runtime 场景证明 artifact byte
变化、legacy archive 和 proposal 后 HEAD 漂移不会被降级成可继续的 `rerun`。

#### 5. 输入与输出

- 输入：digest mismatch、无 v1 COMPLETE 的 archive、assessment 后 HEAD/plan 变化。
- 输出：`artifact_digest_mismatch`、`legacy_archive_unverifiable` 或
  `assessment_digest_changed`；rejected/block artifact。
- 状态变化：零 task close、零 verified-base advance。
- 不变量：hard mismatch 永远不能自动转成 `rerun`。

#### 6. 修改位置

- `crates/ralph-core/tests/scenarios/parallel_forge_reuse_tamper_runtime.yml`
  （计划新增）。
- `crates/ralph-core/tests/scenarios.rs`。
- U2 evaluator、U4 precheck/state projection 的已确认实现位置，仅在 Red 证明
  fail-open 时作最小修正。
- `presets/en/parallel-forge.yml` 与 `presets/schemas/parallel-forge.yml` 的
  rejected/blocked 合同。

#### 7. 可依赖能力

U1 immutable archive、U2 pure evaluator、U4 independent recompute 与原子投影。

#### 8. 禁止依赖的未来能力

不得依赖 U7 的 exhaustion 计数或 U8 的 E2E/docs；不得新增人工 override。

#### 9. 验收测试

真实 EventLoop fixture 分别构造 S7、S8、S12，断言 accepted topic 缺席、拒绝
reason 精确、TaskStore 快照与 verified base 前后一致。

#### 10. Acceptance Red

先修改 settlement artifact byte但保留旧 digest；当前实现若仍发布 accepted event
或关闭 task 即为有效 Red。fixture 未执行 evaluator、YAML parse 失败不算 Red。

#### 11. 单元测试拆分

1. manifest/artifact digest mismatch 分类。
2. legacy archive 无 COMPLETE 分类。
3. assessment/current HEAD mismatch 分类。
4. blocked assessment 禁止进入 projection。
5. 同 mismatch 重放保持相同 reason/digest。

不允许 Mock TaskStore 的状态不变量或跳过真实 EventLoop 路由。

#### 12. Red → Green → Refactor 顺序

```text
S7 Red → digest hard reject → Green
→ S12 Red → legacy hard reject → Green
→ S8 Red → precheck 独立重算与拒绝 → Green
→ 幂等/零副作用回归 → Refactor
```

#### 13. 最小实现范围

只补 hard mismatch 分类、拒绝和零副作用保证；不实现 retry exhaustion、报告终局
或 E2E harness。

#### 14. 集成验证

运行 tamper runtime fixture、reuse-status CLI integration、precheck 和
state-projector targeted tests；所有测试必须经过 nextest。

#### 15. 风险驱动测试

Fault Injection：truncate manifest、missing COMPLETE、digest mismatch、HEAD race；
Mutation-style：逐个翻转 hard gate，确保断言会失败。

#### 16. 回归范围

U1 archive tests、U2 evaluator/CLI、U4 precheck/projector、P0 Parallel Forge BDD；
原因是 U6 收紧这些边界的失败语义。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/tests/scenarios/parallel_forge_reuse_tamper_runtime.yml` | 新增BDD | S7,S8,S12 | E19 |
| `crates/ralph-core/tests/scenarios.rs` | 修改测试注册 | 真实 EventLoop入口 | E19 |
| `presets/en/parallel-forge.yml` | 修改preset | hard reject路由 | E15 |
| `presets/schemas/parallel-forge.yml` | 修改schema | rejected/blocked字段parity | E16 |
| U2/U4 已确认生产文件 | 仅按 Red 修改 | 修复 fail-open | E11–E14 |

#### 18. 完成标准

S7、S8、S12 全绿；accepted event 缺席和零状态变化均有断言；targeted lint/build
通过；无 skip/only/弱断言；可独立提交。

#### 19. 停止条件

若 mismatch 必须读取 raw supervisor DB、需要人工 override、或实际拒绝点不在
U2/U4 调用链，停止并更新 Evidence/KTD。

#### 20. 风险与注意事项

最大风险是把“无法证明”当作“证明需要重跑”。前者是矛盾/损坏，必须 blocked；
只有证据完整但能力缺失才是 rerun。

### U7：连续三次 precheck 不稳定时形成可诊断阻塞

#### 1. Unit 目标

同一 reuse scope 的 assessment 连续三次被独立 precheck 拒绝后，系统恰好一次
发布带 artifact 的 `plan.blocked(kind=precheck_exhausted)`，不再无限重算。

#### 2. 对应需求与 Scenario

- Requirements：R24、R29、R30。
- Scenario：S9。
- Decisions：KTD11、KTD12。
- Evidence：E12、E13、E21、E25。

#### 3. 外部可观察结果

第 1、2 次拒绝唤醒 reconciler 重算；第 3 次生成 block artifact 并终止该 flow；
Reporter 能准确显示 exhaustion，未 accepted 的 tasks 保持 open。

#### 4. 当前行为基线

通用 precheck 已有 default budget 3 和 `on_exhausted`，但 Parallel Forge reuse
尚未把 rejection scope、block artifact 和 reporter 合同接入。

#### 5. 输入与输出

- 输入：同一 `reuse_scope_key` 的三次 rejected assessment。
- 输出：前两次 retry；第三次 `plan.blocked`、`kind=precheck_exhausted`、
  `block_artifact_path`。
- 不变量：新 scope 计数从 0 开始；旧运行 budget 不继承；终态只发一次。

#### 6. 修改位置

- `presets/en/parallel-forge.yml` reuse precheck 配置。
- `presets/schemas/parallel-forge.yml` exhausted payload。
- `crates/ralph-core/tests/scenarios/parallel_forge_reuse_exhaustion_runtime.yml`
  （计划新增）。
- `crates/ralph-core/tests/scenarios.rs`。
- `presets/templates/parallel-forge/reuse-status-rubric.template.md` 的失败记录区。

#### 7. 可依赖能力

U6 stable rejection reasons，以及现有 precheck gate 的 attempt/budget/on_exhausted。

#### 8. 禁止依赖的未来能力

不得依赖 U8 E2E/docs；不得建立第二套 retry counter 或继承旧运行 budget。

#### 9. 验收测试

真实 EventLoop 依次送入 1、2、3、4 次 mismatch：1/2 只产生 rejected+retry；
第 3 次恰好一次 blocked 且 artifact 存在；第 4 次不得产生第二终态。

#### 10. Acceptance Red

先启用 S9 fixture；当前 topology 若第 3 次仍重算、没有 artifact 或重复终态，即为
有效 Red。不相关 producer 失败不算 Red。

#### 11. 单元测试拆分

1. attempt 1/2 返回 retry。
2. attempt 3 返回 exhausted。
3. attempt 4 幂等。
4. scope key 改变重置计数。
5. block artifact path 必填且文件存在。
6. 未 accepted tasks 保持 open。

#### 12. Red → Green → Refactor 顺序

```text
attempt 1/2 Red → reuse现有retry → Green
→ attempt 3 Red → artifact + exhausted终态 → Green
→ attempt 4 Red → terminal幂等 → Green
→ new-scope reset Red → Green → Refactor
```

#### 13. 最小实现范围

只接线现有 precheck exhaustion、artifact 与 reporter projection；不修改通用 budget
默认值，不新增通用恢复框架。

#### 14. 集成验证

运行 exhaustion fixture、precheck targeted tests、preset/schema strict lint；断言
事件数量和 task 快照，而非 prompt 文案。

#### 15. 风险驱动测试

State-machine boundary 0/1/2/3/4；Idempotency 验证终态重放；不做 fuzz，因为输入
集合有限且风险在计数边界。

#### 16. 回归范围

通用 precheck fixtures、P0 precheck gates、Parallel Forge reporter/blocked route；
原因是共享 desugar/runner 行为。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改preset | reuse exhaustion接线 | E12,E15 |
| `presets/schemas/parallel-forge.yml` | 修改schema | blocked payload parity | E16 |
| `crates/ralph-core/tests/scenarios/parallel_forge_reuse_exhaustion_runtime.yml` | 新增BDD | S9 | E19 |
| `crates/ralph-core/tests/scenarios.rs` | 修改注册 | 真实runtime验收 | E19 |
| `presets/templates/parallel-forge/reuse-status-rubric.template.md` | 修改模板 | 记录拒绝与耗尽证据 | E17 |

#### 18. 完成标准

S9 及 0–4 边界全绿；第三次恰好一个终态；artifact 可读；strict lint 与相关回归
通过；可独立提交。

#### 19. 停止条件

若现有 precheck attempt 不能按 reuse scope 隔离、on_exhausted 无法携带 artifact
path、或必须改变全局 budget 语义，停止并重做 KTD12。

#### 20. 风险与注意事项

最大风险是把不同 HEAD/plan 的 assessment 误算成同一 retry chain；
`reuse_scope_key` 必须参与计数身份。

### U8：公开 mock E2E 与 agent-facing 合同闭合 reuse 主路径

#### 1. Unit 目标

一个已注册、可单独运行的 mock E2E 从 settled prefix 经 correction 到终局报告
完整通过，并且 agent/operator 文档只暴露可执行的公开命令和字段。

#### 2. 对应需求与 Scenario

- Requirements：R25–R30。
- Scenario：S14；回归引用 S2、S4、S10、S13。
- Decisions：KTD14、KTD16–KTD18。
- Evidence：E17–E19、E25、E27。

#### 3. 外部可观察结果

`--list` 能看到 `parallel-forge-dispatch-contract`；filtered mock run exit 0，
证明两个 waves 被复用、失败 wave 完成 correction、full Tester/Auditor/Reporter
收口；agent 可按 guide 完成 Observe→Precheck→Apply→Confirm。

#### 4. 当前行为基线

E27 证明当前 scenario 是未注册的失败占位壳。实施入口要求 P0 先将原始
Parallel Forge marker scenario 落地；U8 只在其上扩展 reuse groups，不负责设计
marker harness。

#### 5. 输入与输出

- 输入：P0 marker cassette + reuse archive/assessment fixtures。
- 输出：E2E pass、准确 final report、更新后的公开 help/guides。
- 副作用：只在 E2E 临时 workspace；不调用 live API。
- 不变量：AGENTS/CLAUDE byte-identical；无 internal ledger/path 泄漏。

#### 6. 修改位置

- `crates/ralph-e2e/src/scenarios/parallel_forge.rs`。
- `crates/ralph-e2e/src/main.rs::get_all_scenarios`。
- `cassettes/e2e/parallel-forge-dispatch-contract.jsonl`（P0 计划新增）。
- `cassettes/e2e/README.md`。
- `crates/ralph-core/data/ralph-tools.md`、
  `crates/ralph-core/data/ralph-tools-cmdref.md`、
  `crates/ralph-core/data/ralph-tools-opac.md`。
- `skills/ralph-preset-common/references/` 中受 event/template/gate 影响的文件。
- `CONCEPTS.md`、`CLAUDE.md`、`AGENTS.md`；按实际影响检查
  `.cursor/rules/multi-hat-isolation.mdc`。

#### 7. 可依赖能力

U1–U7 全部已验证能力、P0 已注册 marker scenario/cassette、doc drift 脚本。
进入 U8 前必须执行 P0 filtered E2E 门禁；不满足即停止。

#### 8. 禁止依赖的未来能力

不得新增 Web UI、generic reuse DSL、live API cassette recording 或手工 runtime
ledger 编辑。

#### 9. 验收测试

S14 filtered mock E2E；`--list` 注册断言；污染 hat env 的 CLI integration；
docs command help/drift；strict preset/schema/template parity；最终全量 gate。

#### 10. Acceptance Red

先在 P0 cassette 增加 reuse activation groups 和场景断言；当前实现会重复执行
settled waves、缺 correction experience 或未到终局，因此 filtered run 非零。
找不到 cassette/仍是 placeholder 不是 P1 Red，而是 P0 前置门禁失败。

#### 11. 单元测试拆分

1. scenario 显式注册且 ID 稳定。
2. activation group 数与 cursor 终值一致。
3. settled waves 无 executor activation。
4. correction group 含 prior failure fingerprint 且 round=1。
5. Tester/Auditor/Reporter 次数符合 P0 合同。
6. docs 仅引用公开命令/字段。
7. AGENTS/CLAUDE byte-equal。

#### 12. Red → Green → Refactor 顺序

```text
S14 marker cassette Red
→ 补最小reuse groups/断言
→ filtered E2E Green
→ --list/污染env Red→Green
→ docs help/drift Red→Green
→ strict parity与全量回归
→ Refactor仅在测试保护下
```

#### 13. 最小实现范围

只补 P1 reuse 的 E2E activation groups、场景断言和下游公开文档。若 E2E 暴露
U1–U7 bug，回到拥有该行为的 Unit 修订并重跑，不在 U8 堆积生产逻辑。

#### 14. 集成验证

先跑 filtered E2E，再跑全 mock E2E；使用 replay/mock backend。所有 spawn
`ralph` 的 human-CLI 测试先 scrub agent runtime env。

#### 15. 风险驱动测试

Differential：reuse 与 uninterrupted 最终 tree/tasks/report；marker group
completeness；无需 live fault injection。

#### 16. 回归范围

全 workspace nextest/doctest、P0/P1 BDD、worktree reuse、inspect、precheck、
state projector、template materialize、supervisor recovery、全 mock E2E、CLI docs
drift。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `crates/ralph-e2e/src/scenarios/parallel_forge.rs` | 修改E2E scenario | S14主路径 | E27 |
| `crates/ralph-e2e/src/main.rs` | 修改/确认注册 | 禁止全量命令静默漏跑 | E27 |
| `cassettes/e2e/parallel-forge-dispatch-contract.jsonl` | 修改cassette | reuse activation groups | E27 |
| `cassettes/e2e/README.md` | 修改文档 | group contract | E27 |
| `crates/ralph-core/data/ralph-tools.md` | 修改AI guide | 新Observe命令 | E25 |
| `crates/ralph-core/data/ralph-tools-cmdref.md` | 修改AI guide | CLI args/output | E25 |
| `crates/ralph-core/data/ralph-tools-opac.md` | 修改AI guide | Observe/Precheck动作 | E25 |
| `skills/ralph-preset-common/references/*.md`（仅受影响文件） | 修改operator docs | event/template/gate审计 | E25 |
| `CONCEPTS.md` | 修改域词汇 | reuse terms | E25 |
| `CLAUDE.md` / `AGENTS.md` | 同步修改 | builtin行为说明 | E25 |

#### 18. 完成标准

S1–S14、targeted、strict lint、docs drift、build/clippy/fmt、filtered/full mock E2E、
`./scripts/run-tests.sh` 全部通过；无新增 skip/only/弱断言；AGENTS/CLAUDE一致；
实际 diff 无超范围；可独立提交。

#### 19. 停止条件

P0 scenario 未注册/仍 placeholder、E2E 需要 live API、docs 与实际 help 不一致、
新 public caller 扩大范围或任何 KTD 低于 0.85 时停止并修订计划。

#### 20. 风险与注意事项

最大风险是全量 E2E 因 scenario 未注册而假绿；filtered scenario 和 `--list` 两个
断言都必须执行，不能只看 `cargo run -p ralph-e2e -- --mock` 的总退出码。

## 8. Unit 串行依赖图

```text
U1 Archive manifest + clean supervisor boundary
  ↓ U2 读取 U1 的完整 immutable archive
U2 Runtime evaluator + inspect JSON
  ↓ U3 按 U2 DTO 冻结 machine/human templates
U3 Embedded templates
  ↓ U4 reconciler/precheck 使用 U2 命令与 U3 SSOT
U4 Accepted reusable prefix
  ↓ U5 在 U4 accepted projection 上路由 partial checkpoint
U5 Partial/correction recovery
  ↓ U6 用负向证据证明 hard mismatch 零副作用
U6 Tamper/legacy/drift fail-closed
  ↓ U7 在稳定 rejection identity 上验证 exhaustion
U7 Precheck exhaustion
  ↓ U8 扩展完整实现链的 public mock E2E 与文档
U8 E2E/docs closure
```

- U2 不能先于 U1：否则 evaluator会依赖不完整、无 digest 的 archive。
- U3 不能先于 U2：machine template字段必须来自稳定 DTO。
- U4 不能先于 U3：hat/precheck不得复制自由格式状态表。
- U5 不能先于 U4：partial route需要 accepted assessment和prefix projection。
- U6 不能先于 U5：negative cases 必须覆盖完整 assessment/projection 调用链。
- U7 不能先于 U6：exhaustion 必须按 U6 稳定的 rejection scope/reason 计数。
- U8 最后：它验证完整链并同步所有 public/operator contract。
- 每 Unit 只实现本 Unit 外部行为；禁止提前写后续 topology/docs来“顺手完成”。

## 9. 执行命令清单

以下命令基于 AGENTS.md 和当前 repo 配置；禁止裸跑 ralph-cli `cargo test`。

| 时机 | 命令 | 目的 | 进入下一步条件 |
|---|---|---|---|
| 每 Unit 编辑后 | `cargo fmt --all -- --check` | Rust格式 | 必须通过 |
| U1 | `cargo nextest run -p ralph-core -- worktree::tests::test_clean_worktree_runtime_artifacts` | archive单元 | 必须通过 |
| U1 | `cargo nextest run -p ralph-cli --test integration_worktree_isolation -- reuse` | CLI worktree复用 | 必须通过 |
| U2 | `cargo nextest run -p ralph-core -- reuse_status` | evaluator | 必须通过 |
| U2 | `cargo nextest run -p ralph-cli --test integration_reuse_status` | inspect/Git/archive | 必须通过 |
| U3 | `cargo nextest run -p ralph-cli -- builtin_artifact_templates` | embed registry | 必须通过 |
| U3 | `cargo nextest run -p ralph-cli --test integration_preset_materialize_artifacts` | runtime materialize | 必须通过 |
| U4–U8 | `cargo nextest run -p ralph-core -- state_projector` | atomic projection | 必须通过 |
| U4–U8 | `cargo nextest run -p ralph-core -- precheck` | gate/retry/exhaust | 必须通过 |
| U4–U8 | `cargo nextest run -p ralph-core --test scenarios -- parallel_forge_reuse` | real EventLoop BDD | 必须通过 |
| preset每次改动 | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | CLI preset lint | 必须通过 |
| preset每次改动 | `cargo nextest run -p ralph-core -- preset_lint` | core lint | 必须通过 |
| preset每次改动 | `cargo nextest run -p ralph-cli --bin ralph -- presets` | manifest/embedded/strict parity | 必须通过 |
| CLI/docs改动 | `cargo run -p ralph-cli -- inspect reuse-status --help` | help冒烟 | 必须通过 |
| CLI/docs改动 | `scripts/check-cli-doc-drift.sh` | skill/docs drift | 必须通过 |
| 污染环境回归 | `RALPH_CURRENT_HAT=executor RALPH_CURRENT_LOOP_ID=loop-x RALPH_EVENTS_FILE=/tmp/x.jsonl cargo nextest run -p ralph-cli --test integration_reuse_status` | env scrub | 必须通过 |
| U8 E2E注册 | `cargo run -p ralph-e2e -- --mock --list` | scenario不可静默漏跑 | 输出包含`parallel-forge-dispatch-contract` |
| U8 filtered E2E | `cargo run -p ralph-e2e -- --mock --filter parallel-forge-dispatch-contract` | S14 operator主路径 | 必须通过且不含placeholder |
| E2E | `cargo run -p ralph-e2e -- --mock` | 全mock回归 | 必须通过 |
| lint | `cargo clippy --workspace --all-targets --all-features -- -D warnings` | Rust lint | 必须通过 |
| build | `cargo build --workspace --all-targets` | build | 必须通过 |
| docs | `cargo test --workspace --exclude ralph-e2e --doc` | doctest允许例外 | 必须通过 |
| final | `./scripts/run-tests.sh` | 两阶段nextest+doctest全量 | 必须通过 |
| flake兜底 | `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | 仅竞态/时序flake恢复 | serial仍失败则停止 |

单测命令可按真实 test substring进一步缩小，但不得用错误 subset获得假 Green。
`cargo run -p ralph-cli -- ...` 的 binary参数若当前 package要求 `--bin ralph`，实施者
在 U2 以 `cargo run -p ralph-cli --bin ralph -- inspect reuse-status --help`
为准，并同步文档；这是命令包装差异，不是架构决策。

## Verification Contract

### Acceptance Red 规则

每 Unit 首先运行其明示验收测试并记录实际失败。只有因当前能力缺失的 assertion/
unknown subcommand/missing event失败才有效。编译环境、fixture语法、错误命令、
不相关 baseline failure均不得作为 Red。

### Unit Red/Green/Refactor

每个最小规则单独 Red→最小 Green；禁止删除/弱化断言、skip/only、mock目标行为、
无解释更新 snapshot/golden、扩大 timeout掩盖 race。Refactor后重复 Unit全部
targeted tests。

### Integration 与 Regression

- archive/Git用真实 temp repo/worktree。
- event/state用 `run_workflow_guard_scenario`。
- supervisor不读/写旧 DB作为 reuse依据。
- full E2E用 mock/replay，不用 live API。
- 每 Unit结束运行直接和相邻回归；U8运行全量。

### Evidence 更新

Executor在每 Unit close前把实际 Red、Green、命令、commit和新发现更新到 plan
执行记录或最终可提交 solution文档；不得把
`.ralph/review/<plan-id>/scratch|residuals|draft` commit。

## 10. 最终质量门禁

- S1–S14 全部通过。
- R1–R30 全部有测试和 Unit trace。
- archive manifest/COMPLETE/DB sidecars 负面与正面测试通过。
- 五态、checkpoint、DAG propagation/property tests通过。
- CLI human/JSON同源、read-only、agent-safe。
- reusable prefix task batch atomic/idempotent。
- partial wave不提前关闭task，后继不越过gap。
- prior failure经验注入且旧round/budget不继承。
- precheck独立重算、HEAD race、tamper、三次耗尽通过。
- templates source/build/embed/materialize/README parity通过。
- preset/schema/topic ownership/state projection/BDD parity通过。
- P0全部回归继续通过。
- mock E2E通过。
- `cargo fmt`、clippy、build、doctest、docs drift、全量 tests通过。
- 无新增失败/skip/only、无弱断言、无无解释snapshot/golden变化。
- AGENTS.md/CLAUDE.md完全一致。
- AI skill docs不泄漏内部 ledger/path/function，命令与help一致。
- 无未处理 BLOCKED decision，所有执行关键KTD ≥0.85。
- 实际变更不超本计划；每 Unit独立提交且严格串行。

## Definition of Done

- U1–U8 按顺序各自完成 Acceptance Red → Unit Red → Green → Refactor →
  Integration → Regression → Close。
- P0 DoD和本计划DoD同时为真。
- 一个完整成功 replay证明“旧运行settled prefix + failed wave”可在新运行从
  correction恢复，并与 uninterrupted run得到相同最终 tree/tasks/report。
- 一个 tamper replay证明任何 hard evidence冲突零task副作用且产生可操作blocked
  report。
- abandoned experiments、临时fixtures、dead code、过程产物从diff移除。
- 最终 manager report明确复用/重验/修复/重跑/阻塞统计和恢复点。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 八个纵向行为 Unit，含文件/Red/验证 |
| Executor 是否仍需做关键设计决策 | 否 | KTD1–KTD18已固定 |
| 所有文件和接口是否有代码库证据 | 是 | 已存在路径见E；新增路径明确标记 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | 最低KTD15=0.91 |
| 是否存在未处理的低置信度假设 | 否 | P0是entry gate，不是隐含假设 |
| 每个 Unit 是否只有一个可观察行为 | 是 | archive、inspect、模板、prefix、partial、tamper、exhaustion、E2E/docs contract |
| 每个 Unit 是否可以独立验证 | 是 | 各Unit有targeted命令和回归 |
| 每个 Unit 是否有真实 Red | 是 | 每Unit §10明确旧行为失败原因 |
| 每个 Unit 是否包含回归范围 | 是 | 每Unit §16 |
| 是否存在未来 Unit 依赖 | 否 | 只依赖已完成前置Unit |
| 是否存在泛化任务描述 | 否 | 均给出具体入口、行为、断言 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §5、§6、U1–U8 |
| 所有关键决策是否有 Evidence | 是 | KTD表引用E1–E27 |
| 计划是否可以严格串行执行 | 是 | §7、§8 |
