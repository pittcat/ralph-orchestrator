---
title: Parallel Forge 跨运行 Reuse Status 与证据复用 - Plan
type: feat
date: 2026-07-29
origin: docs/plans/2026-07-29-001-fix-parallel-forge-static-wave-settlement-plan.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
parallel_model: wave-barrier-worktree
rewrite: 2026-07-30 Parallel Planner（串行 U1→U8 → 并行 Wave 模型）
---

# Parallel Forge 跨运行 Reuse Status 与证据复用 - Plan

## 0. 计划状态

- **状态：`PARALLEL READY`**
  - 所有实施关键决策置信度 ≥ 0.85（KTD1–KTD21）
  - 所有 Wave 并发安全置信度 ≥ 0.85（W0–W5）
  - 所有 Worktree 修改边界、契约 Owner、合并顺序已冻结
- **代码基线（Wave 0 起）：** `dffb6a33ae1e5a7bfe0340f2d310cf53d6852fa7`
  （`pittcat-dev`；原计划基线 `6ccacc37…` / `d737b9b7…` 已过时）
- **当前分支：** `pittcat-dev`
- **调查范围：** `--reuse-worktree` archive、`clean_worktree_runtime_artifacts`、
  supervisor.db 三件套、`.ralph/forge/`、`ralph inspect`、precheck/006 透明 emit、
  `CloseTaskBatch`、parallel-forge preset/schema/templates、BDD scenarios、
  template embed registry、sibling 007 InspectCommands 命名空间、001/005/006 落地接口
- **已执行的验证（2026-07-30 Parallel Planner 重勘）：**
  - `git rev-parse HEAD` = `dffb6a33…`
  - `clean_worktree_runtime_artifacts`（`worktree.rs:748`）仍为直接 rename；
    **不**归档 `supervisor.db{,-wal,-shm}`、**不**归档 `.ralph/forge/`、
    **无** `reuse-manifest.v1.json` / `COMPLETE`（E3/E32）
  - `InspectCommands` 仅 `Profiles|Loop|Prompt`（`inspect.rs:46`）；无 `reuse-status`（E11）
  - `StateProjectionAction` 有 `CloseTaskBatch`，**无** `CloseTaskWavePrefix`（E14）
  - `presets/templates/parallel-forge/` 现 **10** basename；
    `PARALLEL_FORGE_TEMPLATE_NAMES` / `presets.rs` 期望 10（E18）
  - parallel-forge preset **无** `event_loop.precheck`、**无** reuse-reconciler、
    **无** `forge.reuse.*` topic（E15）
  - 真实 EventLoop BDD 已有 two_wave_settlement / correction / exhaustion（E19/E29/E30）
  - E2E `parallel-forge-dispatch-contract` 仍为未注册 placeholder；**非**本计划硬门禁（E27）
- **本轮未运行：** Acceptance Red、全量 nextest、clippy、build（Planner 禁止写生产代码）
- **实施入口门禁（F1 前）：**
  1. HEAD = `dffb6a33…` 或其后已含同等 P0 spine 的提交；
  2. `cargo nextest run -p ralph-core --test scenarios -- parallel_forge_two_wave_settlement`
     与 `… parallel_forge_correction` 绿；
  3. `cargo nextest run -p ralph-cli --bin ralph -- presets` 绿；
  4. 工作区无计划范围外脏改动。
- **阻塞项：** 无
- **最大安全并发数：** **3**（仅 Wave 1：U1∥U2∥U3；由文件所有权矩阵推导，非主观）
- **推荐 Worktree 数量：** Wave 1 同时 3 个；其余 Wave 每 Unit 1 个（峰值 3）
- **可启动 / 不可启动：**
  - 可启动：Wave 0 F1（入口门禁通过后）
  - Wave 1 三 Unit：仅在 Barrier 0 通过后并行启动
  - Wave 2+：各自 Barrier 通过前不得启动

### 并发模型总览

```text
Global Prep / Wave 0: F1 契约冻结 + Characterization
        ↓ Barrier 0
Wave 1: U1 Archive ∥ U2 Evaluator+Inspect ∥ U3 Templates
        ↓ Barrier 1
Wave 2: U4 Prefix accept + CloseTaskWavePrefix + reconciler/precheck
        ↓ Barrier 2
Wave 3: U5 Partial checkpoint + failure experience
        ↓ Barrier 3
Wave 4: U6 Tamper fail-closed ∥ U7 Precheck exhaustion BDD
        ↓ Barrier 4
Wave 5: U8 Docs + live checklist
        ↓ Final Integration + Full Regression
```

---

## 1. 功能目标

### 1.1 业务目标

降低长计划在进程退出/人工停止后的重复劳动，同时不把「目录里还有代码」误当成
「行为已验证」。Reuse 只从最早未被证据证明的 P0 checkpoint 继续。

### 1.2 用户或调用方

- 操作者：`ralph run --worktree --reuse-worktree --plan <plan> -H builtin:parallel-forge`
- `clean_worktree_runtime_artifacts`：归档并建立可核验 reuse 来源
- `ralph inspect reuse-status`：只读 assessment
- reuse-reconciler + precheck：物化/提出/独立重算
- P0 dispatcher/failure/review/integrate/verify：按 checkpoint 继续
- Reporter：报告 reuse/reverify/correction/rerun/blocked 汇总

### 1.3 当前行为（已确认）

1. `--reuse-worktree` 精确复用已结束 worktree，runtime 文件直接 rename 到
   `.ralph/reuse-history/<timestamp>/`，无 staging/manifest/COMPLETE（E2/E3）
2. cleanup **不**归档 supervisor DB/WAL/SHM 与 `.ralph/forge/`（E3/E32）
3. 仅写 advisory `resume-context.md`（E4）
4. `--continue` 读 live state；`--reuse-worktree` 是清理后的新运行（E8/E9）
5. P0 spine 已落地（settlement/`CloseTaskBatch`/templates/BDD）；无 reuse 路径（E14/E20）
6. 无五态 evaluator、`forge.reuse.assessed`、`CloseTaskWavePrefix`、
   `inspect reuse-status`（E11/E14/E15）

### 1.4 目标行为与差异

- cleanup：lock → staging copy+hash → manifest → 删 live → `COMPLETE` → 原子 rename
- `ralph inspect reuse-status … --format json` 输出 `reuse_status.v1` 五态 assessment
- 仅连续完整 reusable wave 前缀可经 `CloseTaskWavePrefix` 原子关 task
- precheck 独立重跑 evaluator，比较 digest；三次耗尽 → `plan.blocked{kind=precheck_exhausted}`
- 无 archive → 全 `rerun`（非错误）；legacy/incomplete → `blocked`

### 1.5 输入 / 输出 / 状态 / 错误

- **输入：** operator plan、execution-plan.yml/digest、approved_base_commit、
  Git object graph、完整 archive manifest
- **输出：** `reuse-manifest.v1.json`+`COMPLETE`、`reuse_status.v1`、
  物化 `reuse-status.yml`/`reuse-evidence.md`、accepted `forge.reuse.assessed`、
  prefix task close、resume checkpoint
- **状态变化：** live runtime 清空；accepted 后 prefix tasks done；verified base 推进
- **错误语义：** cleanup 失败不启动新 run；assessment `blocked` 为零副作用 JSON/事件；
  precheck reject 回 producer；耗尽 blocked
- **兼容：** 不读旧 supervisor store；不继承旧 correction round/budget；
  不扩展 `loop_inspect.v2`
- **性能：** inspect 不做测试/backend；cleanup O(归档字节)
- **安全：** inspect/agent-safe DTO 不暴露 DB path/raw events；Verifier/Tester 不写 code

### 1.6 本次范围 / 非目标

**范围：** archive 完整性、五态 evaluator、inspect CLI、两模板、reuse-reconciler+precheck、
`CloseTaskWavePrefix`、partial checkpoint、tamper/exhaustion、docs/live 清单

**非目标：** 第二套调度器/supervisor schema 迁移、全 presets 通用化、完整 mock E2E cassette、
003 readonly gates 补齐、004 auditor 终态、007 `inspect execution-plan` 实现
（仅共享 `InspectCommands` 命名空间，互不覆盖）

### 1.7 已知约束与假设

**已确认事实：** E1–E32（见 §2.3）；P0/005/006 已落地；cleanup 缺口仍在。

**待验证假设（实施前/F1 内关闭，不得留给 Executor 拍板）：**

| ID | 假设 | 验证方法 | 失败影响 |
|---|---|---|---|
| H1 | supervisor DB 路径固定为 worktree `.ralph/supervisor.db{,-wal,-shm}` | F1 characterization + U1 fixture | 若可配置到 `.ralph/` 外 → BLOCKED，重开 KTD2 |
| H2 | `TaskStore::with_exclusive_lock` 可承载「先全验证再按 wave 组关闭」 | U4 Red 前读 `task_store.rs` + projector 试验 | 失败则 KTD13 重开，禁止逐个 CloseTask 伪装 |
| H3 | 007 未在同基线合并 `InspectCommands::ExecutionPlan` | F1/U2 前 `rg InspectCommands` | 若已合并 → U2 机械追加 variant，禁止覆盖 007 route |


---

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

  Scenario S14: 操作者 live 验收跨运行复用主路径（真端到端）
    Given 上一运行留下两个 settled waves 和第三 wave 的 correction failure
    And 操作者用 --worktree --reuse-worktree 启动新 live 运行（真实 backend）
    When reuse assessment 被接受且 development loop 收敛
    Then 前两个 waves 不重复执行并原子关闭其当前 tasks
    And 第三 wave 收到旧失败经验、完成 correction 和重新 settlement
    And full Tester、Auditor、Reporter 按合同完成
    And 自动化不要求完整 mock marker cassette；可选只冒烟关键节点
      （accepted assessment / prefix close / resume checkpoint）
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

- 前置合同（已落地）：
  `docs/plans/2026-07-29-001-fix-parallel-forge-static-wave-settlement-plan.md`
  + `docs/plans/2026-07-29-005-fix-parallel-forge-preset-integration-gap-plan.md`
- 集成缺口修法：
  `docs/solutions/workflow-orchestration/parallel-forge-preset-integration-gap.md`
- precheck 透明 emit：
  `docs/plans/2026-07-29-006-fix-precheck-desugar-emit-transparency-plan.md`
- sibling（边界，不阻塞 U1）：003 readonly / 004 terminal / 007 execution-plan
  inspect
- archive 当前实现：`crates/ralph-core/src/worktree.rs`
- reuse CLI 接线：`crates/ralph-cli/src/commands/run.rs`
- baseline：`crates/ralph-core/src/plan_baseline.rs`
- inspect 模式：`crates/ralph-cli/src/commands/inspect.rs`
- CloseTaskBatch：`crates/ralph-core/src/config/state_projection.rs`、
  `crates/ralph-core/src/state_projector/task.rs`
- supervisor agent-safe summary：`crates/ralph-core/src/supervisor/mod.rs`
- precheck：`crates/ralph-core/src/config/precheck.rs`、
  `crates/ralph-core/src/config/hat.rs::rewrite_emit_topics`、
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
  → worktree.rs::clean_worktree_runtime_artifacts   # 直接 rename，无 manifest
  → 写 .ralph/agent/resume-context.md
  → 重建 LoopContext → fresh EventLoop + supervisor
```

`--continue` 不调用上述 cleanup；读 live scratchpad / supervisor recovery。

### 2.2 并发热点（默认同 Wave 禁止多 Owner）

| 热点 | 路径 | 处理 |
|---|---|---|
| Archive cleanup | `crates/ralph-core/src/worktree.rs` | Wave1 仅 U1 |
| Reuse DTO/evaluator | `crates/ralph-core/src/reuse_status.rs`（计划新增） | F1 建类型；Wave1 仅 U2 扩 evaluator |
| Inspect 路由 | `crates/ralph-cli/src/commands/inspect.rs` | Wave1 仅 U2；与 007 协调 |
| Template registry | `builtin_artifact_templates.rs` + `build.rs` | Wave1 仅 U3 |
| State projection | `config/state_projection.rs` + `state_projector/*` | Wave2 仅 U4 |
| Preset/schema | `presets/en|schemas/parallel-forge.yml` | Wave2 U4 → Wave3 U5 串行；其后冻结 |
| Scenario 注册 | `crates/ralph-core/tests/scenarios.rs` | append-only；按 Merge Order |
| Skill docs | `crates/ralph-core/data/ralph-tools*.md` | Wave5 仅 U8 |
| Cargo.lock | — | 本计划禁止改依赖（KTD18） |

### 2.3 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `worktree.rs::find_reusable_worktree_by_name` | 精确 path/git list/registry 三重检查 | 保留 exact reuse，不加模糊匹配 | 高 |
| E2 | `commands/run.rs` reuse 分支 ~1078 | cleanup 在新 LoopContext 前 | archive 原子边界在此完成 | 高 |
| E3 | `worktree.rs::clean_worktree_runtime_artifacts:748` | 归档 events/history/diagnostics/agent/review；无 DB/forge/manifest | R1 真实缺口；U1 Owner | 高 |
| E4 | 同函数 `resume-context.md` | 仅 advisory 文本 | 保留人类提示；机器真相在 manifest | 高 |
| E5 | `worktree.rs` cleanup tests ~1729+ | 有 archive/symlink/idempotent；无 supervisor/manifest | U1/F1 测试落点 | 高 |
| E6 | `tests/integration_worktree_isolation.rs` | exact reuse/archive/代码保留 | 扩展 S10 CLI | 高 |
| E7 | `plan_baseline.rs` | reuse/continue 保留 baseline | evaluator 不重锚定 | 高 |
| E8 | `run.rs` continue 分支 | continue 要 live scratchpad | cross-run 与 same-run 分离 | 高 |
| E9 | runner supervisor recovery | active wave 属 resume | 旧 DB 不得作 cross-run 状态 | 高 |
| E10 | `SupervisorInspectSummary` | 不暴露 DB path/raw events | reuse inspect 同安全边界 | 高 |
| E11 | `inspect.rs::InspectCommands` | 仅 Profiles/Loop/Prompt | 新增独立子命令，不改 loop_inspect.v2 | 高 |
| E12 | `config/precheck.rs` | desugar proposed/gate/rejected；budget 默认 3 | 复用 gate，不建第二套 retry | 高 |
| E13 | `precheck_gate_runner.rs` | reject→producer；耗尽→configured topic | mismatch 恢复链现成 | 高 |
| E14 | `state_projection.rs::CloseTaskBatch` | 单 wave batch close；无 WavePrefix | 必须新 action，禁止多次 CloseTaskBatch 伪装原子前缀 | 高 |
| E15 | `presets/en/parallel-forge.yml` | supervisor+wave+P0；无 reuse hat/precheck | U4 首次接线须守 006 | 高 |
| E16 | `presets/schemas/parallel-forge.yml` | event required-fields SSOT | 新 topic 必须同步 | 高 |
| E17 | `presets/templates/parallel-forge/README.md` | producer/path/先 materialize | 新模板进 templates | 高 |
| E18 | `builtin_artifact_templates.rs` + `presets.rs:888` | 10 basename 双注册 | U3 加 2 个并更新期望 | 高 |
| E19 | `scenarios.rs::run_workflow_guard_scenario` | 真实 EventLoop BDD | reuse fixtures 必须用此入口 | 高 |
| E20 | HEAD + 001/005 merge | P0 settlement/CloseTaskBatch/BDD 可观测 | 按已落地 checkpoint 恢复 | 高 |
| E21 | Git `756f9ffa` 等 archive 语义 | advisory；旧失败不耗新 budget | failure experience ≠ verdict | 高 |
| E22 | redrive solution docs | fresh boot 不消费残留；冲突 fail closed | manifest 缺失/冲突同策略 | 高 |
| E23 | SQLite concurrency solution | WAL/SHM 是 DB 状态一部分 | 三文件同归档 | 高 |
| E24 | `agent_doc_sync::compute_sha256_hex` | 已有 SHA-256 | 无新 hashing 依赖 | 高 |
| E25 | AGENTS.md preset hard rules | schema/runtime/BDD/docs/skills 同步清单 | 不可省略下游 | 高 |
| E26 | `TaskStore::{load,all,with_exclusive_lock}` | exclusive 可原子改 | prefix action 落现有锁 | 高 |
| E27 | `ralph-e2e/.../parallel_forge.rs` | placeholder 未注册 | mock 全流程非硬门禁 | 高 |
| E28 | schema `forge.wave.settled` required fields | wave_id/settled_* /verified_base/plan_key/… | evaluator 锚点固定 | 高 |
| E29 | `parallel_forge_two_wave_settlement_runtime.yml` | 两 wave settled + CloseTaskBatch | 入口门禁/U4 基线 | 高 |
| E30 | `parallel_forge_correction_runtime.yml` | correction→re-settlement | U5 基线 | 高 |
| E31 | plan 006 + `HatConfig::rewrite_emit_topics` | bare→`.proposed` | reconciler 发 bare | 高 |
| E32 | HEAD cleanup 行为 | 仍无 DB/forge/manifest/COMPLETE | U1 Red 成立 | 高 |
| E33 | `PARALLEL_FORGE_TEMPLATE_NAMES` 10 项 | 与目录一致 | U3 必须同步常量+build.rs+测试期望 | 高 |
| E34 | sibling `2026-07-29-007-…inspector-plan.md` 存在 | 共享 InspectCommands | U2 只追加 ReuseStatus，不覆盖 007 | 高 |

### 2.4 受影响范围

**生产：** `worktree.rs`、`run.rs`、`reuse_status.rs`（新）、`lib.rs`、`inspect.rs`、
`state_projection.rs`、`state_projector/{mod,task,tests}.rs`、
`presets/en|schemas/parallel-forge.yml`、`presets/templates/parallel-forge/*`、
`builtin_artifact_templates.rs`、`build.rs`、`presets.rs`（测试）

**测试：** worktree unit、`integration_worktree_isolation`、`integration_reuse_status`（新）、
`integration_preset_materialize_artifacts`、`state_projector` tests、
`scenarios/parallel_forge_reuse_*.yml`（新）、`scenarios.rs`

**文档：** `ralph-tools*.md`、preset operator references、`CONCEPTS.md`、`CLAUDE.md`/`AGENTS.md`

**不受影响：** Web/API、supervisor schema/migrations、非 parallel-forge topology、Cargo.lock

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| KTD1 | reuse vs continue 状态 | 共用；分离 | **完全分离** | E2,E8,E9,E21 | 旧 retry/active wave 不可跨新运行 | 0.99 |
| KTD2 | 旧 supervisor DB | 复用；删除；归档三件套 | **归档 DB/WAL/SHM** | E3,E9,E22,E23 | 复用污染；删除丢证据 | 0.98 |
| KTD3 | archive 完成条件 | 直接 rename；不可恢复 partial；staging+COMPLETE | **lock+staging+COMPLETE 原子发布** | E3–E5,E22,E23 | rename 中途拆证据 | 0.95 |
| KTD4 | 谁算 status | hat；runtime；runtime+hat 解释 | **runtime pure evaluator；hat 只物化** | E10–E13,E21 | hat 可主观提升 | 0.97 |
| KTD5 | CLI 位置 | 顶级 reuse；inspect；diagnose | **`ralph inspect reuse-status`** | E11,E34 | 与 inspect 只读语义一致 | 0.95 |
| KTD6 | 状态集合 | bool；三态；五态 | **五态固定** | R5–R12,E20,E21 | 无法区分重验/续修/阻塞 | 0.96 |
| KTD7 | Unit identity | id；commit；合同 digest | **unit_id + contract digest** | E15–E20 | 名称/commit 不足 | 0.95 |
| KTD8 | DAG 粒度 | 任意 Unit；整 plan；连续 prefix | **Unit 评估 + wave 聚合 + 连续 prefix** | E14,E15,E20 | 跳过破坏依赖 | 0.98 |
| KTD9 | 部分 wave 关 task | 可复用 Unit 即关；整 wave settlement | **非完整 reusable wave 零提前关闭** | E14,E20 | 依赖边界是 wave settlement | 0.99 |
| KTD10 | 继续点 | 永远 executor；固定 correction；最早 checkpoint | **最早未证明 checkpoint** | E20,E21 | 全重跑浪费；固定 correction 覆盖不全 | 0.96 |
| KTD11 | 失败历史 | 全状态；只文本；经验+重置计数 | **fingerprint/证据继承，round/budget 重置** | E12,E13,E21 | 继承预算=旧运行延续 | 0.99 |
| KTD12 | 接受前证明 | schema；LLM；runtime+precheck 重算 | **assessment digest + precheck 重算** | E11–E13,E20 | schema 不验事实 | 0.94 |
| KTD13 | 关 prefix tasks | 伪造 slot；逐 wave 事件；新 prefix action；手工 CLI | **`CloseTaskWavePrefix`** | E14,E20,E25,E26 | 单 wave CloseTaskBatch 不能表达多 wave 原子前缀 | 0.93 |
| KTD14 | 模板 | prompt 大表；单文档；rubric+YAML | **两嵌入模板** | E17,E18 | 防漂移 | 0.99 |
| KTD15 | legacy archive | 推断；全 rerun；blocked | **blocked** | E21,E22 | 无 digest 不可验 | 0.91 |
| KTD16 | 通用化 | 全 preset；PF 专用；硬编码 | **pure evaluator 可复用；topology 仅 PF opt-in** | E15,E20 | 全平台超范围 | 0.93 |
| KTD17 | loop_inspect.v2 | 加字段 bump；独立命令 | **独立命令** | E10,E11 | loop inspect≠archive 比较 | 0.94 |
| KTD18 | 新依赖/迁移 | 新库/表；复用现有 | **无新依赖、无 DB migration** | E18,E23,E24 | 现有足够 | 0.96 |
| KTD19 | 原串行能否并行 | 保持 U1→U8 串行；安全并行拆 Wave | **Wave1 三角并行；preset 热点串行 U4→U5；U6∥U7** | E3,E11,E14,E15,E18,§2.2 | 强行并行 preset/schema 必语义冲突 | 0.92 |
| KTD20 | 共享 DTO Owner | U1/U2 各写一份；F1 冻结 | **F1 唯一定义 manifest/status DTO** | 并发热点规则 | 禁止双 SSOT | 0.97 |
| KTD21 | exhaustion 配置 Owner | U7 改 preset；U4 一次配齐 | **U4 配齐 budget/on_exhausted；U7 只 BDD 证明** | E12,E13,KTD19 | 避免 Wave4 与 preset 热点重叠 | 0.90 |

无 <0.85 决策。若 P0 字段/action 与 E14/E20/E28 冲突，停止并重开相关 KTD（禁止只改数字）。

### Wave 并发安全置信度

| Wave | 结论 | 支持证据 | 潜在冲突 | 缓解 | 置信度 |
|---|---|---|---|---|---|
| W0 | 可串行执行 | E3,E5,KTD20 | 无并发 | 单 Worktree | 0.97 |
| W1 | **可三路并行** | 文件矩阵无交集；契约已冻 | `lib.rs` 导出 | F1 已 `pub mod`；U2 只扩模块内 | 0.91 |
| W2 | 单 Unit | preset/projection 热点 | 无 | 唯一 Owner U4 | 0.94 |
| W3 | 单 Unit | 续改 preset | 与 U4 同文件 | Barrier2 后基线；禁止回改 U4 事件合同字段语义 | 0.90 |
| W4 | **可两路并行** | U6/U7 预设冻结后只加 scenario；U6 可修 evaluator | `scenarios.rs` append | Merge Order U6→U7；机械冲突 | 0.88 |
| W5 | 单 Unit | docs Owner | 无 | — | 0.95 |

W4 置信度 0.88≥0.85：若实施中发现必须改 `parallel-forge.yml`，立即取消并行，改为 U6→U7 串行并修订计划。

---

## Planning Contract（决策冻结后的设计 SSOT；实施不得偏离）

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


---

## 4. BDD 行为规格

Product Contract S1–S14 为 SSOT。完整 Gherkin：

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

  Scenario S14: 操作者 live 验收跨运行复用主路径（真端到端）
    Given 上一运行留下两个 settled waves 和第三 wave 的 correction failure
    And 操作者用 --worktree --reuse-worktree 启动新 live 运行（真实 backend）
    When reuse assessment 被接受且 development loop 收敛
    Then 前两个 waves 不重复执行并原子关闭其当前 tasks
    And 第三 wave 收到旧失败经验、完成 correction 和重新 settlement
    And full Tester、Auditor、Reporter 按合同完成
    And 自动化不要求完整 mock marker cassette；可选只冒烟关键节点
      （accepted assessment / prefix close / resume checkpoint）
```

实施 fixtures（真实 `run_workflow_guard_scenario`，禁止 `run_scenario` stub）：

| Fixture（计划新增） | Scenarios | Owner |
|---|---|---|
| `parallel_forge_reuse_no_archive_runtime.yml` | S1 | U4（路由）+ U2（分类） |
| `parallel_forge_reuse_prefix_runtime.yml` | S2,S11 | U4 |
| `parallel_forge_reuse_partial_wave_runtime.yml` | S3,S5,S6 | U5 |
| `parallel_forge_reuse_correction_runtime.yml` | S4 | U5 |
| `parallel_forge_reuse_tamper_runtime.yml` | S7,S8,S12 | U6 |
| `parallel_forge_reuse_exhaustion_runtime.yml` | S9 | U7 |
| CLI/worktree integration | S10 | U1 |
| materialize integration | S13 | U3 |
| live checklist | S14 | U8（人工；可选关键节点冒烟） |

---

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | Owner | 合并后重跑 |
|---|---|---|---|---|---|---|
| S1 | 全 rerun；零 DB open | `reuse_status` + BDD | 单元+集成 | Characterization | U2+U4 | 是 |
| S2 | prefix 原子关闭；从 wave3 起 | projector + BDD | 集成 | Idempotency | U4 | 是 |
| S3 | partial 零提前关闭 | evaluator + BDD | 单元+集成 | DAG property | U5 | 是 |
| S4 | failure 注入；round=1 | correction BDD | 集成 | Differential | U5 | 是 |
| S5 | 最早 checkpoint | table tests | 单元 | Mutation checkpoint | U2+U5 | 是 |
| S6 | 非连续降级 | evaluator + BDD | 单元+集成 | Property DAG | U2+U5 | 是 |
| S7 | digest mismatch 零副作用 | tamper BDD | 集成 | Fault Injection | U6 | 是 |
| S8 | HEAD 漂移 reject | precheck BDD | 集成 | Fault Injection | U6 | 是 |
| S9 | 第三次 exhausted | exhaustion BDD | 集成 | boundary 2/3/4 | U7 | 是 |
| S10 | DB 三件套归档 | worktree+CLI | 集成 | crash-window | U1 | 是 |
| S11 | digest replay 幂等 | projector | 单元+集成 | Idempotency | U4 | 是 |
| S12 | legacy blocked | CLI+evaluator+BDD | 集成 | malformed manifest | U1+U2+U6 | 是 |
| S13 | 两模板物化 | materialize | 集成/契约 | registry parity | U3 | 是 |
| S14 | live 主路径 | 人工 checklist | 人工 | 可选 key-node smoke | U8 | 否（人工） |

**共同断言：** status/reason/checkpoint/digest/task batch/verified SHA/event 序列精确匹配；
proposal/reject/blocked 零 task close；inspect 零写盘；cleanup 不改 HEAD/branch/baseline。

**命令基线（真实）：** `cargo nextest run …`；禁止裸 `cargo test -p ralph-cli`。

---

## 6. 需求—测试—Unit 追踪矩阵

| Requirement ID | 需求 | Scenario | 验收/单元/集成 | E2E | Owner Unit | Evidence |
|---|---|---|---|---|---|---|
| R1–R4 | archive 边界 | S10,S12 | worktree+CLI | 否 | F1(表征)+U1 | E1–E9,E23,E32 |
| R5–R12 | 五态/identity | S1,S3–S7,S12 | reuse_status+inspect | 否 | F1(DTO)+U2 | E7,E11,E20–E24 |
| R13–R19 | DAG/prefix/checkpoint | S2–S6,S11 | BDD+projector | 否 | U4+U5 | E14,E15,E19,E20 |
| R20–R24 | precheck/幂等 | S7–S9,S11 | precheck BDD | 否 | U4+U6+U7 | E12–E14,E22 |
| R25–R27 | 模板 SSOT | S13 | materialize | 否 | U3 | E17,E18,E33 |
| R28–R30 | inspect/report/docs | S1,S2,S4,S7–S9,S13,S14 | CLI+docs+live | 人工 S14 | U2+U8 | E10,E11,E25,E27 |

无无测试需求、无 Owner 行为、无 Evidence 关键决策。

---

## 7. 依赖图

```text
F1 (DTO + Characterization + 契约冻结)
  ├── U1 Archive          [需 F1::ReuseManifestV1]
  ├── U2 Evaluator+Inspect [需 F1::ReuseStatusV1 + fixture manifest]
  └── U3 Templates         [需 F1 字段冻结列表]

U1 ──┐
U2 ──┼── U4 Prefix+Precheck+CloseTaskWavePrefix
U3 ──┘

U4 ──── U5 Partial checkpoint + failure experience

U5 ──┬── U6 Tamper fail-closed
     └── U7 Exhaustion BDD

U6 ──┐
U7 ──┴── U8 Docs + live checklist
```

| 边 | 依赖能力 | 类型 | 为何不能同 Wave | 可否用 Fake 消除 |
|---|---|---|---|---|
| F1→U1 | Manifest DTO | 接口 | U1 消费冻结类型 | 否，必须同类型 |
| F1→U2 | Status DTO | 接口 | 同上 | fixture 可代 U1 产物，仍需 F1 |
| F1→U3 | 字段列表 | 合同 | 模板必须对齐 JSON | 否 |
| U1→U4 | 完整 archive | 行为/数据 | U4 BDD 需真实 cleanup 产物或等价 fixture；合并后组合验 | Wave1 内 U4 不可并行；U4 可用 U1 fixture 格式 |
| U2→U4 | inspect 命令 | 行为 | reconciler 调真实命令 | 不可与 U4 同 Wave |
| U3→U4 | 两模板 | 数据 | materialize 路径 | 不可与 U4 同 Wave |
| U4→U5 | accepted projection + 冻结 event 合同 | 行为 | U5 改 dispatcher 路由依赖 U4 topology | 否 |
| U5→U6/U7 | preset 冻结 | 基线 | U6/U7 不得再改 schema 语义 | U6/U7 互不依赖，可并行 |
| U6/U7→U8 | 行为稳定 | 文档 | docs 描述最终行为 | 否 |

---

## 8. 并发 Wave 总览

| Wave | Unit | 可并发 | 基线 | 输出 Barrier | 并发安全置信度 |
|---|---|---|---|---|---|
| 0 | F1 | 否（串行） | `dffb6a33…` | 冻结 DTO+表征绿 | 0.97 |
| 1 | U1,U2,U3 | **三路并行** | Barrier0 commit | archive+inspect+templates | 0.91 |
| 2 | U4 | 否 | Barrier1 commit | prefix/precheck 绿 | 0.94 |
| 3 | U5 | 否 | Barrier2 commit | partial/correction 绿 | 0.90 |
| 4 | U6,U7 | **两路并行** | Barrier3 commit | tamper+exhaustion 绿 | 0.88 |
| 5 | U8 | 否 | Barrier4 commit | docs/drift 绿 | 0.95 |

```text
Wave 0:
  F1

Barrier 0
  ↓

Wave 1:
  U1 ─┐
  U2 ─┼─ Parallel (max=3)
  U3 ─┘

Barrier 1
  ↓

Wave 2:
  U4

Barrier 2
  ↓

Wave 3:
  U5

Barrier 3
  ↓

Wave 4:
  U6 ─┐
  U7 ─┴─ Parallel (max=2)

Barrier 4
  ↓

Wave 5:
  U8

Final Integration → Full Regression
```

---

## 9. 文件、行为与契约所有权

### 9.1 文件所有权矩阵

| 路径或模块 | Owner | Wave | 其他只读 | 冲突风险 | 策略 |
|---|---|---|---|---|---|
| `crates/ralph-core/src/reuse_status.rs` | F1 创建类型；U2 实现 evaluator；U6 仅修 fail-open | 0/1/4 | 是 | 中 | 分 Wave；禁止同 Wave 双写 |
| `crates/ralph-core/src/lib.rs` | F1（`pub mod`） | 0 | 是 | 低 | 后续只读 |
| `crates/ralph-core/src/worktree.rs` | F1 表征测试；U1 生产实现 | 0/1 | 是 | 中 | 分 Wave |
| `crates/ralph-cli/src/commands/run.rs` | U1（summary 显示） | 1 | 是 | 低 | — |
| `crates/ralph-cli/tests/integration_worktree_isolation.rs` | U1 | 1 | 是 | 低 | — |
| `crates/ralph-cli/src/commands/inspect.rs` | U2 | 1 | 是 | 中（007） | 只追加 variant |
| `crates/ralph-cli/tests/integration_reuse_status.rs` | U2（新） | 1 | — | 无 | — |
| `presets/templates/parallel-forge/reuse-status*.template.*` | U3（新） | 1 | 是 | 低 | — |
| `builtin_artifact_templates.rs` / `build.rs` | U3 | 1 | 是 | 低 | — |
| `integration_preset_materialize_artifacts.rs` | U3 | 1 | 是 | 低 | — |
| `config/state_projection.rs` + `state_projector/*` | U4 | 2 | 是 | 高 | 唯一 Owner |
| `presets/en/parallel-forge.yml` | U4→U5 | 2/3 | 之后只读 | 高 | 串行；Barrier3 后冻结 |
| `presets/schemas/parallel-forge.yml` | U4→U5 | 2/3 | 之后只读 | 高 | 同上 |
| `scenarios/parallel_forge_reuse_prefix_*.yml` | U4 | 2 | — | 无 | — |
| `scenarios/parallel_forge_reuse_partial_*.yml` + `*_correction_*.yml` | U5 | 3 | — | 无 | — |
| `scenarios/parallel_forge_reuse_tamper_*.yml` | U6 | 4 | — | 无 | — |
| `scenarios/parallel_forge_reuse_exhaustion_*.yml` | U7 | 4 | — | 无 | — |
| `tests/scenarios.rs` | 各 Unit append 自己的 `fn test_…` | 2–4 | — | 中 | Merge Order；机械冲突 |
| `crates/ralph-core/data/ralph-tools*.md` + skills refs | U8 | 5 | 是 | 低 | — |
| `CONCEPTS.md` / `CLAUDE.md` / `AGENTS.md` | U8 | 5 | 是 | 低 | `cp CLAUDE.md AGENTS.md` |

### 9.2 行为所有权矩阵

| 行为/规则 | Owner | 消费者 | SSOT | 验证 |
|---|---|---|---|---|
| Manifest 字段/COMPLETE 语义 | F1/U1 | U2,U4 | `ReuseManifestV1` | U1 tests |
| 五态分类与 DAG 传播 | U2 | U4–U7 | `reuse_status` evaluator | unit+inspect |
| 模板内容与 embed | U3 | U4,U8 | templates + registry | materialize |
| precheck + CloseTaskWavePrefix + reconciler topology | U4 | U5–U7 | preset/schema/projection | BDD S2/S11 |
| partial checkpoint / failure experience 路由 | U5 | U8 | preset instructions + BDD | S3–S6 |
| tamper/legacy 零副作用 | U6 | U8 | evaluator reason + BDD | S7/S8/S12 |
| exhaustion 终态可观测性 | U7 | U8 | BDD（配置属 U4） | S9 |
| docs/live 清单 | U8 | 操作者 | skill/CONCEPTS | drift+S14 |

### 9.3 共享契约矩阵

| 契约 | 定义 Owner | 消费者 | 冻结时机 | 修改规则 |
|---|---|---|---|---|
| `reuse_manifest.v1` | F1 | U1,U2,U4 | Barrier 0 | 禁止私改；需重规划 |
| `reuse_status.v1` JSON 字段 | F1 | U2,U3,U4 | Barrier 0 | 同上 |
| 五态/reason code 枚举 | F1 | U2,U5–U7 | Barrier 0 | 同上 |
| checkpoint 全序 | F1/计划 | U2,U5 | Barrier 0 | 同上 |
| `CloseTaskWavePrefix` 输入形状 | U4 | projector/BDD | Barrier 2 | 后续只消费 |
| `forge.reuse.assessed` required_fields | U4 | schema/precheck/U5 | Barrier 2 | U5 只加 resume 上下文字段，不改已冻语义 |
| 两模板 basename | U3 | registry | Barrier 1 | U5 若缺字段→停止重规划，不私加第三模板 |
| InspectCommands::ReuseStatus | U2 | CLI | Barrier 1 | 007 不得覆盖 |


---

## 10. Worktree 执行单元

**分支命名约定（仓库无强制规则时的描述性占位）：**
- Worktree 名：`reuse-002-<unit-id>-<slug>`
- 本地分支：`plan/2026-07-29-002/<unit-id>-<slug>`
- 创建：`git worktree add -b <branch> .worktrees/<name> <Wave基线commit>`

---

### Wave 0

#### Wave 目标

冻结 `reuse_manifest.v1` / `reuse_status.v1` DTO 与表征测试，消除 Wave 1 双 SSOT 风险。

#### Wave 基线

- Base：`dffb6a33ae1e5a7bfe0340f2d310cf53d6852fa7`
- 前置：§0 入口门禁
- 冻结产出：DTO 字段名/默认/sentinel/`none`、reason code 枚举、checkpoint 全序
- 启动条件：入口门禁绿

#### Wave 并发安全说明

单 Unit；置信度 0.97。

---

#### Unit F1：契约冻结与 cleanup Characterization

##### 1. Unit 目标

在不改变生产 cleanup 行为的前提下，落地可序列化 DTO + roundtrip 测试，并表征
「当前不归档 supervisor DB / forge / 无 COMPLETE」的基线，供 U1 形成真实 Red。

##### 2. Worktree Contract

- Worktree ID：`WT-F1`
- 基线：`dffb6a33…`
- 名：`reuse-002-f1-contracts` / 分支 `plan/2026-07-29-002/f1-contracts`
- **允许：**
  - 新增 `crates/ralph-core/src/reuse_status.rs`（仅 DTO/枚举/canonical digest helper/roundtrip tests；**无**五态业务规则实现）
  - `crates/ralph-core/src/lib.rs`：`pub mod reuse_status;`
  - `worktree.rs` **仅测试**：characterization 断言当前 live 仍保留 supervisor.db（若 fixture 创建）且无 manifest/COMPLETE
  - 可选 fixtures：`crates/ralph-core/src/reuse_status/fixtures/*.json`（若用子目录则 `mod fixtures`）
- **禁止：** 修改生产 `clean_worktree_runtime_artifacts` 行为；改 preset/schema/inspect/projection；加依赖；实现 evaluator 状态表
- 行为所有权：契约形状 + 当前 cleanup 缺口表征
- 数据所有权：DTO schema_version 常量
- 依赖冻结契约：无（本 Unit 定义）
- 对外输出：`ReuseManifestV1`、`ReuseStatusV1`、`ReuseUnitStatus` 五态枚举、`ReuseReasonCode`、`assessment_digest`/`archive_digest` 计算规则（不含业务分类）
- 合并前置：DTO roundtrip + characterization 绿；clippy/fmt
- 合并顺序：Wave0 唯一

##### 3. 需求与 Scenario

- R2/R5 字段形状；S10/S12 表征前置；KTD6/15/18/20；E3,E5,E24,E32

##### 4. 外部可观察结果

代码可 `serde_json` roundtrip 冻结字段；characterization 测试固定「DB 仍留在 live」的现状。

##### 5. 当前行为基线

E3/E32：无 manifest。先写表征，再在 U1 翻转断言。

##### 6. 输入输出

- 输入：计划 §Planning Contract 字段表
- 输出：Rust 类型 + 测试
- 错误：非法 schema_version 反序列化失败
- 不变量：不改变磁盘上任何 worktree

##### 7. 修改位置

| 位置 | 职责 | 为何改 | 边界 |
|---|---|---|---|
| `reuse_status.rs`（新） | DTO SSOT | Wave1 共享 | 无 evaluator match 臂业务 |
| `lib.rs` | 导出 | 可见性 | 只加 mod |
| `worktree.rs` tests | 表征 | 钉住缺口 | 不改生产函数体 |

##### 8. 同 Wave 隔离

仅 F1。

##### 9–10. 可依赖 / 禁止依赖

可依赖 serde/sha2 现有；禁止依赖 U1–U8 未合并实现。

##### 11. 验收测试

- `reuse_manifest_v1_roundtrip`
- `reuse_status_v1_roundtrip_excludes_assessed_at_from_digest`
- `characterization_cleanup_leaves_supervisor_db_without_manifest`
- 命令：`cargo nextest run -p ralph-core -- reuse_status` 与
  `cargo nextest run -p ralph-core -- worktree::tests::characterization`

##### 12. Acceptance Red

表征测试若在改生产前已绿（描述现状）——F1 的「Red」针对 **缺失 DTO 模块**：先写 roundtrip 编译失败/mod 缺失，再加空壳类型 Green。不得通过改生产 cleanup 让表征「提前」变成目标行为。

##### 13–14. 单元拆分与 TDD 序

```text
mod 缺失 Red → 空模块 + 类型骨架 Green
→ roundtrip Red → serde 实现 Green
→ digest 排除 assessed_at Red → Green
→ characterization 断言现状 Green（钉基线）
→ Refactor
```

##### 15. 最小实现

只类型与表征；不实现 staging/COMPLETE/evaluator。

##### 16–18. 本地集成 / 风险 / 回归

`cargo nextest run -p ralph-core -- reuse_status worktree::tests`；
风险：表征写错导致 U1 假 Green——要求断言「manifest 不存在」而非「DB 存在即可」。

##### 19. 预期文件变更

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `reuse_status.rs` | 新增 | DTO | KTD20 |
| `lib.rs` | 修改 | mod | — |
| `worktree.rs` tests | 修改 | 表征 | E3 |

##### 20–21. 提交与完成标准

1–2 commits：`test(reuse): characterize cleanup gap` + `feat(reuse): freeze manifest/status DTOs`；
完成：roundtrip绿、表征绿、未改生产 cleanup、可独立提交。

##### 22. 停止条件

H1 失败（DB 路径可配置外置）；或发现已有冲突模块名——停止重规划。

##### 23. 风险

DTO 过度超前实现业务规则 → 禁止；检测：PR diff 不得含 status 匹配表。

---

### Wave 1

#### Wave 目标

并行交付：完整可消费 archive、只读五态 inspect、两 reuse 模板 embed。

#### Wave 基线

- Base：Barrier 0 合并后的 commit（记为 `B0`）
- 冻结契约：F1 DTO
- 启动：三 Worktree 均从 `B0` 创建

#### Wave 并发安全说明

| Unit | 独占写 | 为何不冲突 |
|---|---|---|
| U1 | `worktree.rs` 生产 + `run.rs` + worktree integration | 不碰 inspect/templates |
| U2 | `reuse_status.rs` evaluator + `inspect.rs` + 新 integration | 不碰 worktree 生产；读 F1 类型 |
| U3 | templates + registry + build.rs | 不碰 core evaluator |

置信度 0.91。`scenarios.rs` 本 Wave **不改**。

---

#### Unit U1：完整 archive manifest 与 supervisor 隔离

##### 1. Unit 目标

`--reuse-worktree` 后 latest archive 含 manifest+COMPLETE；live 无旧 DB/forge；代码/HEAD/baseline 不变。

##### 2. Worktree Contract

- WT-U1 / `reuse-002-u1-archive` / `plan/2026-07-29-002/u1-archive`
- 基线：`B0`
- **允许：** `worktree.rs`（生产+测试）、`run.rs`（仅打印 archive/manifest 摘要）、
  `integration_worktree_isolation.rs`
- **禁止：** `reuse_status` evaluator、`inspect.rs`、preset/schema、templates、projection、skill docs
- 行为：archive 生命周期；数据：archive 目录成员
- 依赖：F1 `ReuseManifestV1`
- 输出：可消费 archive（COMPLETE+manifest）；cleanup API 仍返回 archive path（可扩展摘要字段，但不得迫使 U2 改签名——若改返回类型，须保持向后可解包或同 PR 文档化，且仅 U1/run.rs 消费）
- 合并序：Barrier1 内 **先 U1**（为组合验提供真实 archive 路径习惯）

##### 3. 对应：R1–R4；S10；KTD1–3,18；E1–E6,E22,E23,E32

##### 4–6. 可观察 / 基线 / IO

见原 U1；基线由 F1 表征钉住。错误→`WorktreeError`，新 run 不启动。

##### 7. 修改位置

`worktree.rs`：将 `archive_if_exists` 的 rename-before-verify 改为
discover→copy→hash→manifest→delete live→COMPLETE→rename staging；纳入 DB/WAL/SHM/`.ralph/forge/`；
worktree-scoped create-new lock；可恢复 staging。
`run.rs`：显示摘要。
不打开 DB 内容、不做 migration。

##### 8. 同 Wave 隔离

与 U2/U3 无文件交集；不读其他 WT 未合并代码；manifest 字节合同以 F1 为准。

##### 9–10. 依赖 / 禁止

可依赖 F1、`compute_sha256_hex`、`get_head_sha`、`read_plan_baseline`；
禁止 U2 命令、U4 事件。

##### 11–14. 测试与 TDD

验收名：
- `test_clean_worktree_runtime_artifacts_writes_complete_manifest`
- `test_clean_worktree_runtime_artifacts_archives_supervisor_sidecars`
- `test_cleanup_failure_leaves_resumable_staging_and_refuses_current_start`
- CLI reuse 断言 live clean

命令：
`cargo nextest run -p ralph-core -- worktree::tests::test_clean_worktree_runtime_artifacts`
`cargo nextest run -p ralph-cli --test integration_worktree_isolation -- reuse`

Acceptance Red：表征断言翻转——期望 manifest/COMPLETE/DB 进入 archive；当前缺失→失败。

```text
sidecar Red → 扩归档集合 Green
→ manifest digest Red → staging writer Green
→ partial failure Red → resume staging Green
→ concurrent lock Red → Green
→ COMPLETE 原子发布 Red → Green
→ CLI Red → run.rs 接线 Green
→ Refactor
```

##### 15–18. 范围 / 集成 / 风险 / 回归

必须：lock/staging/digest/COMPLETE/forge/DB 三件套；保留 resume-context。
禁止：读 DB、压缩、删历史 archive。
风险测试：fault injection、idempotency、concurrency、path escape、DB-only/WAL/SHM 组合。
回归：worktree create/list/remove/exact reuse/symlink/baseline。

##### 19–21. 变更 / 提交 / 完成

| 位置 | 类型 | Evidence |
|---|---|---|
| `worktree.rs` | 改生产/测 | E3 |
| `run.rs` | 改 | E2 |
| `integration_worktree_isolation.rs` | 改 | E6 |

完成：S10 相关绿；表征旧断言删除或改为目标行为；无越界；可提交。

##### 22–23. 停止 / 风险

H1/并发 live supervisor 持锁 DB 无法定义边界→停止。最大风险：COMPLETE 承诺不完整——靠最后写 COMPLETE + fault injection。

---

#### Unit U2：五态 evaluator 与 `inspect reuse-status`

##### 1. Unit 目标

`ralph inspect reuse-status --execution-plan <p> --approved-base-commit <sha> --format json`
返回确定 `reuse_status.v1`；只读；同输入同 digest。

##### 2. Worktree Contract

- WT-U2 / `reuse-002-u2-inspect` / `plan/2026-07-29-002/u2-inspect`
- 基线：`B0`
- **允许：** `reuse_status.rs`（在 F1 DTO 上实现 evaluator/adapters）、
  `inspect.rs`（追加 `ReuseStatus` variant+args+handler）、
  新 `integration_reuse_status.rs`（必须 `common::ralph_bin()` scrub）、
  如需：`main.rs` 仅当现有路由未覆盖 subcommand 测试时补 parse 测试
- **禁止：** `worktree.rs`、templates/registry、preset/schema、projection、skill docs、改 loop_inspect schema
- 依赖：F1 DTO；**可用 fixture manifest**（不依赖 U1 未合并代码）
- 输出：公开 evaluator API + CLI
- 合并序：Barrier1 内 U1 之后、U3 之前或之后均可（建议 U1→U2→U3）

##### 3. 对应：R5–R19,R28；S1,S3–S7,S12 分类；KTD4–11,15–18；E7,E10,E11,E20–E24,E34

##### 4–7. 行为 / 修改

无 archive→全 rerun + `archive_id="none"`；legacy/incomplete→blocked JSON（成功退出码+blocked）；
plan invalid→CLI error。
实现：contract digest、Git ancestry adapter（真实 temp repo）、DAG 传播、prefix 扫描、
prior failure summary（无 round/budget）。

##### 8. 隔离

不写 `worktree.rs`；不与 U3 抢 registry。

##### 9–10. 依赖 / 禁止

可依赖 F1、TaskStore load/all、serde、git 命令模式；禁止关 task、发事件、写 artifact。

##### 11–14. 测试与 TDD

`cargo nextest run -p ralph-core -- reuse_status`
`cargo nextest run -p ralph-cli --test integration_reuse_status`
污染回归：
`RALPH_CURRENT_HAT=executor RALPH_EVENTS_FILE=/tmp/x.jsonl cargo nextest run -p ralph-cli --test integration_reuse_status`

Red：unknown subcommand。

```text
CLI parse Red → 最小 no_archive DTO Green
→ 五态表 Red → Green
→ DAG/prefix Red → Green
→ Git/archive gates Red → Green
→ human/JSON parity → Refactor
```

##### 15–23. 范围 / 完成 / 停止

最小：只读 evaluator/CLI；unknown→blocked 非第六态。
停止：007 已占用同名 variant；或必须读 supervisor DB——重开 KTD。
风险：与 007 机械冲突→按 KTD5/E34 并列追加。

---

#### Unit U3：reuse 模板 embed 与物化

##### 1. Unit 目标

binary-only 环境 `ralph preset materialize-artifacts parallel-forge --plan-key <k>`
产出两模板且字段对齐 `reuse_status.v1`。

##### 2. Worktree Contract

- WT-U3 / `reuse-002-u3-templates` / `plan/2026-07-29-002/u3-templates`
- 基线：`B0`
- **允许：**
  - 新 `presets/templates/parallel-forge/reuse-status-rubric.template.md`
  - 新 `presets/templates/parallel-forge/reuse-status.template.yml`
  - `builtin_artifact_templates.rs`、`build.rs`、`integration_preset_materialize_artifacts.rs`
  - `presets.rs` 中 **仅** 与 template 计数相关的断言（`names.len()` 10→12）
- **禁止：** evaluator、worktree、preset event topology、schema topics、inspect
- 依赖：F1 字段列表（只读）
- 合并序：Barrier1 建议最后（更新计数断言）

##### 3. 对应：R25–R27；S13；KTD14；E17,E18,E33

##### 4–14. 要点

Acceptance Red：materialize 后缺两文件或 registry len≠12。
TDD：加文件→常量→embed→materialize 断言→README 索引一行（若 README 列出 templates）。

##### 15–23.

禁止在 prompt 复制状态表；停止条件：需要第三模板或改 DTO 字段——回 F1 重冻。

---

### Wave 2

#### Wave 目标

Guardian 后插入 reuse-reconciler；precheck 独立重算；accepted 后 `CloseTaskWavePrefix` 原子关闭连续 reusable 前缀。

#### Wave 基线

- Base：`B1`（Wave1 三 Unit 按 U1→U2→U3 合并且 Barrier1 绿）
- 冻结：F1 DTO、U2 CLI、U3 模板路径
- 启动：单 WT-U4

#### Wave 并发安全：单 Owner；置信度 0.94

---

#### Unit U4：precheck 接受与 CloseTaskWavePrefix

##### 1. Unit 目标

proposed→accepted 后，一次性按 wave 拓扑关闭完整 reusable prefix，推进 verified base；
同 digest 重放幂等；**并在本 Unit 配齐** precheck budget=3 与 `on_exhausted→plan.blocked`（供 U7 只测）。

##### 2. Worktree Contract

- WT-U4 / `reuse-002-u4-prefix` / `plan/2026-07-29-002/u4-prefix`
- 基线：`B1`
- **允许：**
  - `presets/en/parallel-forge.yml`（reuse-reconciler hat、precheck、Guardian→reuse→dispatcher、projection、exhaustion 配置）
  - `presets/schemas/parallel-forge.yml`（`forge.reuse.assessed` / `.rejected` required_fields）
  - `state_projection.rs`、`state_projector/mod.rs`、`task.rs`、`tests.rs`：`CloseTaskWavePrefix`
  - `presets.rs` 结构化 topology 测试
  - 新 `parallel_forge_reuse_no_archive_runtime.yml`、`parallel_forge_reuse_prefix_runtime.yml`
  - `scenarios.rs` 注册上述测试
- **禁止：** 实现 partial/correction 细路由（属 U5）；改 U1 archive；改模板 basename；改 inspect DTO 字段语义；教手写 `.proposed`（006）
- 依赖：U1–U3 已合并能力
- 输出：可运行 prefix 路径 + 冻结 event/projection 合同
- 合并序：Wave2 唯一

##### 3. 对应：R13–R16,R20–R24；S1路由,S2,S11；KTD8–13,16,21；E12–E20,E25,E26,E31

##### 4–7. 行为 / 修改

006：emit bare `forge.reuse.assessed`。
`CloseTaskWavePrefix`：输入 ordered waves[] + counts；先全验证再单锁按组关；
verified base/resume metadata 同 topic 普通 set。
禁止伪造 `forge.wave.settled`。

##### 8. 隔离：本 Wave 仅 U4

##### 11–14. 测试

`cargo nextest run -p ralph-core -- state_projector`
`cargo nextest run -p ralph-core --test scenarios -- parallel_forge_reuse_prefix`
`cargo nextest run -p ralph-core --test scenarios -- parallel_forge_reuse_no_archive`
`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`
`cargo nextest run -p ralph-cli --bin ralph -- presets`

Red：BDD 期望跳过 wave1/2 exec，旧拓扑仍派 wave1。

```text
topology Red → reconciler route Green
→ precheck digest Red → Green
→ CloseTaskWavePrefix Red → Green
→ replay/conflict Red → Green
→ no_archive+prefix BDD → Refactor
```

##### 15–23.

停止：TaskStore 锁无法「先验后写」或必须伪造 supervisor terminal——重开 KTD13。
风险：accepted 与 task batch 不同步——projector 全量验证后再写。

---

### Wave 3

#### Wave 目标

第一个非 reusable wave 按最早 checkpoint 恢复；注入失败经验；partial 零提前关 task。

#### Wave 基线：`B2`；单 WT-U5；置信度 0.90

---

#### Unit U5：partial checkpoint 与失败经验

##### 1. Unit 目标

affected 才派 exec slot；preserved 参与全 wave review；`resume_correction` 进 correction 且 round=1；
仍产出 current-run settlement。

##### 2. Worktree Contract

- WT-U5 / `reuse-002-u5-partial` / `plan/2026-07-29-002/u5-partial`
- 基线：`B2`
- **允许：** `presets/en|schemas/parallel-forge.yml`（resume/affected/preserved/prior failure 字段与 hat 路由）、
  新 partial/correction BDD + `scenarios.rs` 注册、
  `manager-report.template.md` reuse 摘要区（若 R29 需要）
- **禁止：** 改 `CloseTaskWavePrefix` 语义；改 F1/U2 DTO；改 U3 basename；关 partial tasks；继承旧 round
- **若发现模板缺字段：** 停止并重规划（禁止私改冻结 YAML 合同字段名）
- 合并序：Wave3 唯一；之后 **preset/schema 语义冻结**

##### 3. 对应：R8–R19,R29；S3–S6；KTD6–11；E15,E20,E21,E30

##### 11–14.

`cargo nextest run -p ralph-core --test scenarios -- parallel_forge_reuse_partial`
`cargo nextest run -p ralph-core --test scenarios -- parallel_forge_reuse_correction`
+ P0 correction/settlement 回归

Red：whole-wave 派发或缺 review coverage。

停止：supervisor 不支持 subset fan-in——更新合同，禁止合成 `exec.unit.done`。

---

### Wave 4

#### Wave 目标

证明篡改/漂移零副作用；证明三次 precheck 耗尽终态。Preset **只读**。

#### Wave 基线：`B3`；U6∥U7 从同一 `B3` 创建；置信度 0.88

#### 并发安全

- U6 独占：`parallel_forge_reuse_tamper_runtime.yml` + 必要时 `reuse_status.rs` 最小 fail-open 修复
- U7 独占：`parallel_forge_reuse_exhaustion_runtime.yml`
- 共享：`scenarios.rs` append-only；合并 U6→U7
- **禁止** 任一修改 `presets/en|schemas/parallel-forge.yml`（已冻）；若 Red 证明缺配置→停止改回串行并重开 KTD21

---

#### Unit U6：篡改 / legacy / 漂移零副作用

##### 1. 目标

blocked/reject 路径零 task close、零 verified-base 推进；稳定 reason code。

##### 2. Worktree Contract

- WT-U6 / `reuse-002-u6-tamper` / `plan/2026-07-29-002/u6-tamper`
- 允许：tamper BDD、`scenarios.rs` 追加、`reuse_status.rs` 仅当 Red 证明 fail-open 的最小修复、
  `integration_reuse_status` 补充 legacy/tamper 用例（若需）
- 禁止：preset/schema、projection、templates registry、docs
- 对应：R11,R20–R23；S7,S8,S12；KTD12,15；E10–E13,E22

##### 11–14.

`cargo nextest run -p ralph-core --test scenarios -- parallel_forge_reuse_tamper`
+ reuse_status / integration_reuse_status 相关

Red：被降级成 rerun 或产生 task 副作用。

---

#### Unit U7：precheck exhaustion BDD

##### 1. 目标

同 scope 第 3 次 mismatch → 恰好一次 `plan.blocked{kind=precheck_exhausted}`；第 4 次无第二终态。

##### 2. Worktree Contract

- WT-U7 / `reuse-002-u7-exhaustion` / `plan/2026-07-29-002/u7-exhaustion`
- 允许：exhaustion BDD + `scenarios.rs` 追加
- 禁止：改 preset（配置属 U4）；改 evaluator；改 docs
- 对应：R22,R24；S9；KTD11,12,21；E12,E13

##### 11–14.

`cargo nextest run -p ralph-core --test scenarios -- parallel_forge_reuse_exhaustion`

若失败因缺 on_exhausted 配置→**停止**，不得在本 WT 改 yml；回 U4 补丁。

---

### Wave 5

#### Unit U8：Agent guides、下游合同与 live 清单

##### 1. 目标

skill/CONCEPTS/AGENTS 与真实 CLI/事件一致；S14 live 验收清单可执行；可选关键节点冒烟（非硬门禁）。

##### 2. Worktree Contract

- WT-U8 / `reuse-002-u8-docs` / `plan/2026-07-29-002/u8-docs`
- 基线：`B4`
- 允许：`ralph-tools.md`/`cmdref`/`opac`、`skills/ralph-preset-common/references/*`、
  `CONCEPTS.md`、`CLAUDE.md`+同步 `AGENTS.md`；可选 e2e key-node（不得注册完整假 cassette 当硬门禁）
- 禁止：改 runtime 行为「补锅」；改 preset topology；把 S14 写成 CI 硬红
- 对应：R28–R30；S14；E25,E27

##### 11–14.

`scripts/check-cli-doc-drift.sh`
`cargo run -p ralph-cli --bin ralph -- inspect reuse-status --help`
preset author/review 相关引用人工核对 finding-rubric

完成：drift 绿；CLAUDE=AGENTS；live checklist 写入计划 DoD 附录。

---

## 11. Wave 汇合计划

### Wave 0 Merge Order

| 顺序 | Unit | 原因 | 合并前 | 合并后 |
|---|---|---|---|---|
| 1 | F1 | 冻结契约 | reuse_status+characterization | 同左 + `cargo check -p ralph-core` |

**Gate0：** DTO 字段与 §Planning Contract 表一致；生产 cleanup **未**改；无 skip 测试。

### Wave 1 Merge Order

| 顺序 | Unit | 原因 | 合并前 | 合并后 |
|---|---|---|---|---|
| 1 | U1 | archive 真相来源 | worktree+CLI reuse | 同上 + 表征目标行为 |
| 2 | U2 | 消费 manifest 合同 | reuse_status+integration_reuse_status | 用真实 U1 archive 再跑 inspect 烟雾 |
| 3 | U3 | registry 计数 | materialize+presets len | materialize 12 + presets |

**Gate1：** 三 Unit 无越界；用 U1 产出的 archive 跑一次 `inspect reuse-status`（组合）；
`cargo nextest run -p ralph-cli --bin ralph -- presets`；无语义冲突。

**失败：** 机械冲突（import）可修；DTO 字段分歧→语义冲突→停并重冻 F1。

### Wave 2 Merge Order

| 1 | U4 | — | projector+reuse BDD+preset_lint+presets | 加跑 P0 two_wave_settlement/correction 回归 |

**Gate2：** schema/preset parity；006 bare emit；CloseTaskWavePrefix 单测+BDD；
冻结 `forge.reuse.*` 与 projection 合同文本。

### Wave 3 Merge Order

| 1 | U5 | — | partial/correction BDD + P0 回归 | reporter 字段若改则 materialize 回归 |

**Gate3：** preset/schema **冻结声明**；subset fan-in 与 review coverage 计数分离已测。

### Wave 4 Merge Order

| 顺序 | Unit | 原因 | 合并前 | 合并后 |
|---|---|---|---|---|
| 1 | U6 | tamper 先于 exhaustion 叙述 | tamper BDD | 组合 scenarios |
| 2 | U7 | append scenarios.rs | exhaustion BDD | 全 reuse scenarios 子集 |

**Gate4：** 未改 preset；S7–S9 绿；若有人改了 yml→门禁失败。

### Wave 5 Merge Order

| 1 | U8 | — | drift+help | 文档与 `--help` 一致 |

**Gate5：** CLAUDE=AGENTS；无计划残留文件进 git。

### 各 Wave 失败处理

- 可直接修：格式化、append 冲突、测试注册顺序
- 必须回退 Unit：越界改冻结契约、削弱断言、引入新依赖
- 语义冲突：双 reason code、双默认值、投影与 evaluator 不一致→停+重规划
- 不得在 Barrier 失败时启动下一 Wave

---

## 12. 最终集成计划

**最终合并顺序：** F1 → U1 → U2 → U3 → U4 → U5 → U6 → U7 → U8

**最终基线：** Barrier5 commit

**关键主路径（自动化）：** S1/S2/S7/S9/S10/S13 fixtures + inspect CLI + materialize  
**关键主路径（人工）：** S14 live reuse→correction→Tester/Auditor/Reporter

**跨模块不变量：**
- 不读旧 supervisor store 作状态
- 不继承旧 round/budget
- 不跳过 first unproven wave
- proposal/reject/blocked 零 task close
- Verifier/Tester 不写 code

**最终回归：**
```bash
./scripts/run-tests.sh
```
（含 nextest 两阶段 + doctest；禁止手动 `--workspace` 跳过 phase2）

可选：`cargo nextest run -p ralph-core --test scenarios -- parallel_forge_reuse`
`cargo nextest run -p ralph-cli --bin ralph -- presets`

---

## 13. 执行命令清单

| 时机 | 命令 | 目的 | 失败可否继续 |
|---|---|---|---|
| 入口 | `cargo nextest run -p ralph-core --test scenarios -- parallel_forge_two_wave_settlement` | P0 门禁 | 否 |
| 入口 | `cargo nextest run -p ralph-core --test scenarios -- parallel_forge_correction` | P0 门禁 | 否 |
| 入口 | `cargo nextest run -p ralph-cli --bin ralph -- presets` | preset 基线 | 否 |
| F1 | `cargo nextest run -p ralph-core -- reuse_status` | DTO | 否 |
| U1 | `cargo nextest run -p ralph-core -- worktree::tests::test_clean_worktree_runtime_artifacts` | archive | 否 |
| U1 | `cargo nextest run -p ralph-cli --test integration_worktree_isolation -- reuse` | S10 | 否 |
| U2 | `cargo nextest run -p ralph-core -- reuse_status` | evaluator | 否 |
| U2 | `cargo nextest run -p ralph-cli --test integration_reuse_status` | CLI | 否 |
| U2 | 带 `RALPH_CURRENT_HAT=…` 的同上 | HARD RULE 5 | 否 |
| U3 | `cargo nextest run -p ralph-cli --test integration_preset_materialize_artifacts` | S13 | 否 |
| U4 | `cargo nextest run -p ralph-core -- state_projector` | prefix action | 否 |
| U4 | `cargo nextest run -p ralph-core --test scenarios -- parallel_forge_reuse_prefix` | S2/S11 | 否 |
| U4 | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | lint | 否 |
| U5 | `… parallel_forge_reuse_partial` / `…_correction` | S3–S6 | 否 |
| U6 | `… parallel_forge_reuse_tamper` | S7/S8/S12 | 否 |
| U7 | `… parallel_forge_reuse_exhaustion` | S9 | 否 |
| U8 | `scripts/check-cli-doc-drift.sh` | docs | 否 |
| U8 | `cargo run -p ralph-cli --bin ralph -- inspect reuse-status --help` | 冒烟 | 否 |
| 每 Unit | `cargo fmt` / `cargo clippy -p <affected> -- -D warnings`（或仓库惯用 clippy 入口） | 质量 | 否 |
| 最终 | `./scripts/run-tests.sh` | 全量 | 否 |
| E2E | `cargo run -p ralph-e2e -- --mock` | 仅可选冒烟 | 是（非硬门禁） |

不得编造 nextest 以外的 ralph-cli 测试入口。

---

## 14. 最终质量门禁

- [ ] S1–S13 自动化通过；S14 人工清单完成或明确延期签字
- [ ] R1–R30 均有测试/清单追踪
- [ ] 无新增 skip/only；无削弱断言；无无解释 snapshot
- [ ] 无 BLOCKED 决策；KTD 与 Wave 置信度 ≥0.85
- [ ] Worktree 无越界；契约唯一 Owner
- [ ] 无重复业务规则实现
- [ ] 全部 Barrier 通过；`./scripts/run-tests.sh` 绿
- [ ] CLAUDE.md 与 AGENTS.md 一致
- [ ] 无 `.ralph/review/**/residuals|scratch|draft` 入 git
- [ ] 实际 diff ⊆ 本计划所有权矩阵

---

## 15. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 是实施计划而非 Roadmap | 是 | Wave/Worktree/文件边界/命令均具体 |
| Executor 仍需关键设计决策 | 否 | KTD1–21 已冻；未决仅 H1–H3 在 F1/U4 关闭 |
| 文件/接口有代码库证据 | 是 | E1–E34 |
| 关键决策置信度 ≥0.85 | 是 | 最低 KTD21=0.90 |
| Wave 并发安全 ≥0.85 | 是 | 最低 W4=0.88 |
| 未处理低置信度假设 | 否 | H1–H3 有验证动作与失败影响 |
| 明确 DAG | 是 | §7 |
| 明确 Wave | 是 | §8 |
| 每 Unit 单一可观察行为 | 是 | F1/U1–U8 |
| 独立 Worktree 可验证 | 是 | §10 Contract |
| Worktree 修改边界明确 | 是 | 允许/禁止列表 |
| 同 Wave 未处理文件重叠 | 否 | W1/W4 矩阵无未处理重叠 |
| 同 Wave 未处理行为重叠 | 否 | 行为 Owner 唯一 |
| 共享契约唯一 Owner | 是 | §9.3 |
| 共享热点有策略 | 是 | §2.2 / Foundation / 串行 preset |
| 每 Unit 真实 Red | 是 | §10 各 Acceptance Red |
| 每 Unit 本地回归 | 是 | §10 |
| 每 Wave 汇合门禁 | 是 | §11 |
| 确定性合并顺序 | 是 | F1→U1→…→U8 |
| 机械/语义冲突处理 | 是 | §11 |
| Scenario 可追踪 | 是 | §5–6 |
| 决策有 Evidence | 是 | §3 |
| 最大并发由证据推导 | 是 | max=3（W1 文件不相交） |
| 组合验证+全量回归 | 是 | §12–14 |
| 存在泛化任务描述 | 否 | 已消除「相关模块并行」类表述 |

---

## Definition of Done

1. Barrier0–5 全部通过且最终 `./scripts/run-tests.sh` 绿
2. `--reuse-worktree` 产生 COMPLETE archive（含 DB 三件套与 `.ralph/forge/`）
3. `ralph inspect reuse-status` human/JSON 同源可用
4. PF reuse 路径：prefix 原子关闭、partial checkpoint、tamper/exhaustion 行为符合 S1–S13
5. 两模板可 materialize；docs/skills 同步
6. S14 live 验收完成或产品签字延期（不得用 mock 全链冒充）

---

## 执行纪律（Coding Agent）

1. 一 Agent 一 Worktree 一 Unit；不读其他 WT 未合并实现
2. 不改冻结契约与他者所有权文件
3. 不在 Barrier 前开下一 Wave
4. 语义冲突停止并重规划
5. Unit 完成必须提交：实际文件、Red/Green 证据、回归、clippy/fmt、偏差、Commit ID
