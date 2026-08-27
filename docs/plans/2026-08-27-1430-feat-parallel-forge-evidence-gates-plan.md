---
title: "feat: 为 parallel-forge 增加关键阶段证据门禁"
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
status: active
origin: docs/brainstorms/2026-08-12-003-feat-evidence-driven-orchestration-state-requirements.md
product_contract_source: ce-brainstorm
plan_depth: deep
plan_status: READY
baseline_commit: e94c07822bce7eacbf9b05cc4920fd4ed0aa4316
---

# feat: 为 parallel-forge 增加关键阶段证据门禁

> Executor 必须按下方 DAG 的 Ready Set 最大并发执行；不得按 Unit 编号或 Wave 形成全局 Barrier。禁止写生产代码以外的架构再设计。禁止扩展 payload consistency DSL。禁止给 `exec.unit.done` 加 guard。

---

## 0. 计划状态

* **状态：`READY`**
* **基线：** `e94c07822bce7eacbf9b05cc4920fd4ed0aa4316`（`fix(parallel-forge): 修复 worktree 恢复并强制 DAG 并发`）
* **调查范围：** `presets/en/parallel-forge.yml`、`presets/schemas/parallel-forge.yml`、`presets/scenarios/parallel-forge-*.yml`、`crates/ralph-core/src/config/{precheck,ralph_config}.rs`、`crates/ralph-core/src/event_policy/validation.rs`、`crates/ralph-core/src/event_policy_payload_consistency.rs`、`crates/ralph-core/src/event_loop/precheck_gate_runner.rs`、`crates/ralph-cli/src/{presets.rs,commands/emit/command_impl.rs}`、`crates/ralph-core/tests/scenarios/parallel_forge_*.yml`、`ce-executor-pipeline` 对照实现
* **已执行验证：** 读源码、测试与 Git diff；`git rev-parse HEAD`；`git show e94c0782`；`cargo nextest --version` → `0.9.140`
* **尚未执行（留给 Executor 的 Red/Green，不是计划阻塞）：** 新增证据门禁尚未实现，因此本计划内 S1–S22 的目标 Red/Green 尚未执行；最终全量 `./scripts/run-tests.sh` 也尚未执行。
* **阻塞项：** 无。所有实施决策置信度 ≥ 0.85

---

## 1. 功能目标

* **业务目标：** 只有证据完整、payload 自洽、目标 worktree 身份稳定的 `parallel-forge` 关键 handoff 才能推进状态或执行最终 `git merge --ff-only`。
* **用户/调用方：** builtin `parallel-forge` 的 producer hats（尤其 `worktree` / `reviewer` / `integrator` / `tester` / `auditor` / `forge-failure-handler`）、runtime EventLoop、`ralph preset check` / `ralph preset verify`、最终消费 merge 结果的 operator。
* **当前行为（基线，E1–E17）：**
  * `event_loop.precheck` **不存在**；只有 3 条 `payload_consistency`（三个 topic 的 `verified_base_commit eq ""`）。
  * `forge.worktrees.ready` 无 `target_start_sha` / `target_status_fingerprint`；instructions 已要求 `target_branch = RALPH_CURRENT_BRANCH`，但 schema `field_docs` 仍写 `git worktree list` 第一项。
  * `forge.wave.settled` 被 `CloseTaskBatch` 消费；`.proposed` 不存在，故当前任何 accepted `forge.wave.settled` 都会关 task。
  * `forge.audit.done` 无论 `verdict` 为何都触发 `finalizer`，随后真实 FF merge。
  * `work.failed` **publishers：** `forge-failure-handler`（真路径）、`integrator` / `verifier`（publishes 有、instructions 禁止）、**`tester`（publishes + instructions 第 6 步直接 emit）**。
  * **不存在** topic `forge.full.verification.failed`（仅出现在已归档计划文档，当前 YAML/schema 无此键）。
  * `parallel_forge_handoff::derive_plan_handoff` 已忽略 artifact 的 `execution_wave` 作为硬排序，按 `depends_on` 计算最早安全 wave；未知依赖和环会在 handoff 阶段报错。
  * `parallel_forge_resume::validate_manifest` 已允许 `plan_path` 在 parent/child TUI 边界发生 absolute/relative 表示变化，但其它 manifest identity 字段仍精确比较。
* **目标行为：** 见 §4 Scenarios。双 guard topic 在 producer emit 路径上先被确定性 consistency 拒绝（挂在 `<T>.proposed`），再进 LLM precheck；通过后才出现 accepted `<T>`。`work.failed` 只有 failure-handler 能发。最终 merge 只被 accepted 且 ACCEPTED 的 `forge.audit.done` 激活。
* **行为差异：** 非法/空/矛盾/证据不足的关键事件从「prompt 自律后仍推进」变为「reject_with_resume → 单 owner 修复；3 次耗尽 → runtime 注入 `forge.plan.blocked`」。
* **本次范围：** 只改 builtin `parallel-forge` 的 preset/schema/projection/hat instructions/ownership、对应 BDD 与 `presets/scenarios/`、author notes、规定下游同步清单（无改动则记录 no-op）。
* **非目标：** 不改 supervisor/wave/slot retry/三轮 final correction 业务语义；不加 `exec.unit.done` guard；不对 `forge.finalized` 做 LLM precheck；不扩展 consistency ops；不把 operator worktree CLI 写进 `crates/ralph-core/data/*.md`；不新增/重命名/删除 builtin preset 名。
* **输入：** producer 按现有 `ralph emit <topic>`（runtime 对有 precheck 的 topic 改写成 `<topic>.proposed`）。
* **输出：** accepted 业务 topic，或 `<topic>.rejected` + `task.resume`，或耗尽后 `forge.plan.blocked{reason,kind=precheck_exhausted}`。
* **状态变化：** 仅 accepted `forge.wave.settled` 触发 `CloseTaskBatch`；仅 accepted `forge.audit.done` 激活 finalizer。
* **错误语义：** consistency Hit = 拒绝（`gate: payload_consistency:<id>`）；precheck 失败 = `<T>.rejected{failed_checks,reason}`；耗尽 payload 由 runtime `build_exhausted_payload` 生成 `{topic,reason,kind:precheck_exhausted}`，**不经 schema required_fields**（与 pipeline 相同注入路径）。
* **兼容：** 无旧 preset 版本兼容义务。动态场景 `parallel-forge-blocked.yml` / `no-output.yml` 行为目的不变。
* **性能：** 禁止 Unit 级 LLM gate；既有两波并发 BDD 必须仍绿。
* **安全/权限：** 不扩大 hat 写权限；fingerprint 比较允许启动前已有且未变化的 dirty。
* **已知约束：**
  * consistency `when` 只支持 `eq/ne/gt/gte/exists/non_empty`；`non_empty` **忽略布尔值**，空数组要用 `{eq: []}` 才能 Hit。
  * `gte: 90` 在 Hit=拒绝 的极性下会拒绝合法高分，**禁止**用 consistency 表达「confidence≥90」；该阈值只放 precheck。
  * `on_exhausted` 字符串解析为 topic+reason；parallel-forge 清理链订阅的是 `forge.plan.blocked` 不是 `plan.blocked`。
* **已确认假设：** 复用现有 desugar / consistency / `reject_with_resume`；`RALPH_CURRENT_BRANCH` / `RALPH_WORKSPACE_ROOT` 由 `loop_runner/execution.rs` 注入；独立 Unit 可从同一基线开发，`integration_order` 只用于 deterministic merge。
* **待验证假设：** 无（全部已用代码证据关闭或标为确定决策）。

---

## 2. 代码库现状与证据

### 2.1 当前实现入口

* **外部入口：** `ralph run -H builtin:parallel-forge`；hats 用 `ralph emit`；dispatcher 用 `ralph wave emit exec.unit.ready`。
* **调用链（成功主路径）：** `forge.start` → inspector → planner → guardian → **worktree** `forge.worktrees.ready` → dispatcher fan-out → executor `exec.unit.done` → supervisor `exec.wave.complete` → reviewer `forge.wave.reviewed` → integrator `forge.wave.integrated` → verifier `forge.wave.verified` → integrator `forge.wave.settled`（CloseTaskBatch）→ … → dispatcher `forge.exec.development.done` → tester `forge.full.verified` → auditor `forge.audit.done` → **finalizer `git merge --ff-only`** → `forge.finalized` → cleanup → reporter `forge.report.done` + `LOOP_COMPLETE`。
* **核心模块：** preset YAML + schema SSOT；`RalphConfig::apply_precheck_desugar`；`event_policy::validate_event` 内 consistency；`precheck_gate_runner::dispatch`；`state_projector::project_close_task_batch`。
* **数据边界：** 业务 payload 只带路径/身份/分数；长文在 `.ralph/forge/<plan-key>/`。
* **现有测试：** `crates/ralph-core/tests/scenarios/parallel_forge_*.yml` + `scenarios.rs` 中 `test_parallel_forge_*`；`crates/ralph-cli/src/presets.rs` 内 `test_parallel_forge_*` / `test_all_embedded_presets_pass_strict_lint`；`presets/scenarios/parallel-forge-{blocked,no-output}.yml`。
* **构建验证：** `cargo nextest run`；`ralph preset check -H builtin:parallel-forge --strict`；`./scripts/run-tests.sh`。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
| --- | --- | --- | --- | --- |
| E1 | `presets/en/parallel-forge.yml:169-303` | `event_loop` 无 `precheck:`；`payload_consistency.rules` 仅 3 条 empty-base | 门禁必须新增，不是改已有 precheck | 高 |
| E2 | `presets/schemas/parallel-forge.yml:672-706` | `forge.worktrees.ready` required: execution_plan_path, worktree_map_path, integration_branch, target_branch, base_commit, plan_key；无 start SHA/fingerprint；`target_branch` source 写 porcelain 第一项 | 必须加字段并修正 field_docs | 高 |
| E3 | `presets/en/parallel-forge.yml:610-619` | worktree instructions 已要求 `RALPH_CURRENT_BRANCH` / `RALPH_WORKSPACE_ROOT` | instructions 补 SHA/fingerprint，不要改回 porcelain | 高 |
| E4 | `crates/ralph-cli/src/loop_runner/execution.rs:107-173` | 注入 `RALPH_CURRENT_BRANCH` | 身份以该 env 为准 | 高 |
| E5 | `presets/en/parallel-forge.yml:1072-1237` | integrator/verifier publishes 含 `work.failed` 但 instructions 禁止；**tester publishes+instructions 第 6 步直接 emit `work.failed`** | tester 必须改 typed failure；不能只改 deny rule | 高 |
| E6 | `rg forge.full.verification.failed presets/` | 当前 preset/schema **无此 topic**；只出现在 `docs/achieved/plan/` | 必须 **新增** schema+triggers，不能写成「改已有 topic」 | 高 |
| E7 | `presets/schemas/parallel-forge.yml:1146-1148` + `crates/ralph-cli/src/presets.rs:2988-3006` | accepted `forge.wave.settled` → `CloseTaskBatch{task_ids: settled_task_ids}` | precheck 把 producer 改写成 `.proposed` 后，投影仍只绑 accepted topic → 拒绝不会关 task | 高 |
| E8 | `presets/en/parallel-forge.yml:1268-1314` | finalizer `triggers: [forge.audit.done]`，无 verdict 过滤，然后 `git merge --ff-only` | 必须在 accepted `forge.audit.done` 前挡住非 ACCEPTED | 高 |
| E9 | `crates/ralph-core/src/config/ralph_config.rs:162-244` | desugar：producer `publishes`/`terminal_events`/`exempt_topics` 改写为 `.proposed`；合成 `precheck-<topic>`；consumer `triggers` 仍为 accepted topic | hat YAML 继续写 accepted topic 名；不要手写 `.proposed` 进 publishes | 高 |
| E10 | `crates/ralph-cli/src/commands/emit/command_impl.rs:1171-1175` + `event_policy/validation.rs:1867-1868` | emit 先 rewrite 到 `.proposed`，consistency **精确匹配 `rule.topic`** | **双 guard 的 consistency.topic 必须是 `<T>.proposed`**，否则会先花 LLM 再在 gate 转发时误伤 gate hat | 高 |
| E11 | `event_policy_payload_consistency.rs:265-291,321-328` | `non_empty` 不读 bool；空数组对 `non_empty:true` 是 Miss；类型不匹配对 `eq` 是 Hit | 空数组用 `{eq: []}`；不要写 `{non_empty:false}` | 高 |
| E12 | `precheck_gate_runner.rs:176-279` + `step_dispatch.rs:429-463` | 耗尽注入 `{topic,reason,kind:precheck_exhausted}`，**跳过 schema**；topic 取 `on_exhausted` 括号前 | `on_exhausted: "forge.plan.blocked(reason=precheck_failed)"` 才能唤醒 cleanup（订阅 `forge.plan.blocked`） | 高 |
| E13 | `crates/ralph-core/tests/scenarios.rs:2115+` + 14 个 `parallel_forge_*.yml` | 真 EventLoop BDD 入口已存在；动态 verify 只有 blocked/no-output | 新 BDD 放同目录并用 `run_workflow_guard_scenario`；success/recovery 场景为新增文件 | 高 |
| E14 | `crates/ralph-cli/src/{preflight,config_resolution}.rs` `PRESET_OPT_IN*` 已含 `"precheck"` | 加 `event_loop.precheck` 不会被 operator omit 静默丢掉 | U11 对这两处记 no-op | 高 |
| E15 | `ce-executor-pipeline.yml:92-172` + `ce_executor_pipeline_fail_gate_*.yml` | 对照：precheck YAML 形状、`on_fail.target`、耗尽 BDD、gate 转发 verbatim | 复制该形状，不要发明第二套 gate 机制 | 高 |
| E16 | `crates/ralph-core/src/config/precheck.rs:178-221` | desugar 为 `.proposed` 继承 guarded 的 `payload`+`required_fields`；`.rejected` 要求 `failed_checks`,`reason` | 不必手写 `.proposed` schema，除非 consistency 要在 normalize 前 lint 该 topic——本计划 lint 走 parse_yaml（含 desugar），故可把 rule.topic 设为 `.proposed` | 高 |
| E17 | `crates/ralph-core/src/parallel_forge_handoff.rs:353-460, 598-626`；`crates/ralph-core/src/parallel_forge_resume.rs:670-708, 1497-1514`；`git show e94c0782` | 当前基线已按 `depends_on` 重算最早安全 wave、拒绝未知依赖/环，并允许 resume 的 `plan_path` 绝对/相对表示变化 | 证据门禁计划不得再把 `execution_wave` 当硬串行边；新增或回归场景必须保留独立 Unit 并发和 parent/child resume 行为 | 高 |
| E18 | `presets/en/parallel-forge.yml:313-321, 415-505`；`crates/ralph-core/data/ralph-tools-wave.md:109-122`；`skills/ralph-preset-{author,review}/references/{patterns,finding-rubric}.md` | 当前 preset、注入 skill 与 author/review rubric 已明确「depends_on 决定 worker admission，integration_order 只决定 merge」 | 计划下游同步由“no-op”改为已落地基线确认；不得让后续 Unit 恢复人工串行语义 | 高 |
| E19 | `cargo nextest run -p ralph-core --lib artifact_first_handoff_rewrites_serial_layout_for_independent_units`；`cargo nextest run -p ralph-core --lib identity_accepts_parent_child_absolute_plan_path_representation`；`ralph preset check --help`；`bash scripts/check-cli-doc-drift.sh --strict` | 两项 e94c0782 回归测试通过；`preset check` 支持 `-H/--strict/--format json`；CLI 文档 drift 检查通过 | 更新后的 Unit gate 可复用这些真实命令；全量验证仍留给执行阶段 | 高 |

### 2.3 受影响范围（已确认路径）

* **生产：** `presets/en/parallel-forge.yml`、`presets/schemas/parallel-forge.yml`
* **新增文档：** `presets/en/parallel-forge-preset-author-notes.md`（计划新增）、`docs/solutions/workflow-orchestration/parallel-forge-evidence-gates.md`（U11 新增）
* **测试：** `crates/ralph-core/tests/scenarios.rs`、新增 `parallel_forge_*gate*_runtime.yml`、`crates/ralph-cli/src/presets.rs` 结构断言、`presets/scenarios/parallel-forge-success.yml` 与 `parallel-forge-evidence-recovery.yml`（计划新增）
* **配置：** `event_loop.precheck`（preset 内 opt-in；框架 strip 列表已包含，不改 Rust）
* **API/CLI/UI：** 无新子命令。使用已有 `ralph preset check` / `verify` / `inspect prompt` / `emit --policy-check`
* **调用方：** dispatcher（等 accepted worktrees.ready）、integrator（等 accepted reviewed）、CloseTaskBatch、finalizer、cleanup/reporter
* **构建：** `ralph-cli` embed（`include_str!` OUT_DIR presets）；改 yml 后走现有 `build.rs`，不改 `PRESETS` 数组名字

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
| --- | --- | --- | --- | --- | --- | --- |
| D1 | Gate Scope | hard / balanced / soft | **hard**：Confidence≥85、Coverage≥80、Verifiability≥80、Impact Certainty≥75；Critical Ambiguities=0、Critical Unverified=0 | 用户已批准；merge/task 边界需 fail-closed | balanced/soft 会让证据不足仍推进 merge | 0.95 |
| D2 | 关键 topic 的 guard 组合 | 仅 precheck / 仅 consistency / both | **both** 且预算独立：precheck retry_budget=3；consistency 走现有 3-strike（author notes 记录 `payload_consistency_retry_budget: 3`，**YAML 无此字段，禁止发明**） | E1、finding `preset.key_stage_event_gate_shared_budget` | 共享一个计数器会被 review skill 判违规 | 0.93 |
| D3 | 高频/收据 topic | 也加 LLM / 只 consistency / 都不加 | **`forge.exec.development.done` / `forge.full.verified` / `forge.finalized` / `forge.report.done` 仅 consistency**；**`exec.unit.done` neither** | 吞吐 + 不可逆动作已在 audit 挡住 | 给 unit.done 加 LLM 会串行化 slot | 0.95 |
| D4 | merge 门在哪 | 拦 `forge.finalized` / 拦 `forge.audit.done` | **拦 `forge.audit.done`**；finalizer 只消费 accepted 事件 | E8 | finalized 时 merge 已发生 | 0.95 |
| D5 | `work.failed` writer | 维持 tester 直发 / 单 writer | **唯一 publisher = `forge-failure-handler`**。tester 改为发 **新 topic** `forge.full.verification.failed`；failure-handler 增加该 trigger，**不**从 tester 失败再开新一轮 wave-fixer（保持今日「全量失败即终态失败」） | E5、E6 | 「已有 typed topic」不成立 | 0.92 |
| D6 | 数值阈值放哪 | 扩展 DSL / 只 precheck | **不扩展 DSL**；90/75 只写进 precheck prompt；consistency 只拒绝同 payload 结构矛盾 | E11；schema `forge.final.correction.settled` 已用 `allowed_values` 表达「必须=3」 | `gte:90` 极性会拒绝合法值 | 0.95 |
| D7 | 目标稳定性证据 | 仅 SHA / SHA+fingerprint | **`target_start_sha` = `git rev-parse HEAD`（在 `RALPH_WORKSPACE_ROOT`）；`target_status_fingerprint` = SHA-256(canonical porcelain)** | E3、E4 | 仅 SHA 看不见 untracked/dirty 漂移 | 0.90 |
| D8 | fingerprint 算法 | porcelain 全量 / 排除 `.ralph/` | **`git status --porcelain=v1 --untracked-files=all`，丢掉 path 以 `.ralph/` 开头的行，对剩余字节做 SHA-256（utf-8，保留 git 行序，行以 `\n` 结尾）** | 与 pipeline work.done precheck 使用同一 porcelain 形态 | 包含 `.ralph/` 会因 forge 产物误伤 | 0.88 |
| D9 | 耗尽 emit 哪个 topic | `plan.blocked` / `forge.plan.blocked` | **`on_exhausted: "forge.plan.blocked(reason=precheck_failed)"`** | E12；cleanup `triggers` 含 `forge.plan.blocked` | `plan.blocked` 不会激活 cleanup/reporter | 0.93 |
| D10 | 双 guard 时 consistency 的 `rule.topic` | 写 `<T>` / 写 `<T>.proposed` | **必须写 `<T>.proposed`** | E10、E16 | 写 `<T>` 会在 LLM 之后打在 gate 转发上 | 0.92 |
| D11 | 空数组怎么拒绝 | `non_empty:false` / `eq: []` | **`{field: settled_task_ids, eq: []}`**（`settled_unit_ids` 同） | E11 | `non_empty` 忽略 false | 0.95 |
| D12 | 是否改 Rust | 改 matcher 自动 strip `.proposed` / 纯配置 | **纯 preset/schema/测试**；不改 consistency matcher、不改 `build_exhausted_payload` | 现有配置已能表达 | 无证据证明配置不够 | 0.90 |
| D13 | `event_loop.precheck` 框架合并 | 改 opt-in 列表 / 不改 | **不改** | E14 | 列表已有 `precheck` | 0.95 |
| D14 | auditor 非 ACCEPTED | 仍发 `forge.audit.done` 靠 gate 拒 / 直接 `forge.plan.blocked` | **instructions：仅 ACCEPTED 才 emit `forge.audit.done`；否则 `forge.plan.blocked`。consistency 仍拒绝 `verdict ne ACCEPTED` 作第二道** | E8 | 只靠 prompt 会回到现状 | 0.88 |
| D15 | 测试落点 | 改 builtin 全量 fixture / 迷你 EventLoop YAML + 结构测试钉死 builtin | **builtin 用 `RalphConfig::parse_yaml(get_preset("parallel-forge").content)` 结构断言；行为用迷你 YAML（规则从 preset 复制，注释要求同步）+ `run_workflow_guard_scenario`** | E13、E15 | 把 12+ hat 全量塞进每个 BDD 不可维护 | 0.90 |
| D16 | Unit 调度依据 | 按 topic/文件顺序串行 / 按真实接口与写集合并发 | **U1/U2/U3 先从同一基线并发；U4 关闭 precheck 基础后，U5/U6/U7/U8/U9/U10 按各自直接依赖 ASAP 并发；U11 仅在最终 schema/拓扑汇合后执行** | E17、E18；当前 runtime 已按 `depends_on` 重算 wave | 旧线性图没有真实因果证据；同一 YAML 的不同语义区域可 deterministic merge | 0.93 |
| D17 | 当前基线变更归属 | 把 e94c0782 重新纳入本计划 / 将其视为已验证前置能力 | **视为已验证前置能力，不在本计划重复实现；新增回归覆盖 DAG 重算、环/未知依赖拒绝与 resume path 表示变化** | E17 | 该能力已经存在并有直接单测；重复实现会扩大范围 | 0.96 |

指纹计算伪代码（Executor 必须写进 worktree/auditor/precheck prompt，禁止另发明）：

```text
status = `git -C $RALPH_WORKSPACE_ROOT status --porcelain=v1 --untracked-files=all`
lines  = status 按 `\n` 分割，去掉末尾空行
kept   = 行内路径（rename 取 ` -> ` 右侧，否则取状态码后的 path）不以 `.ralph/` 开头的行
canonical = kept.join("\n") + (kept 非空则再加一个 `\n`，空则空串)
fingerprint = hex(SHA-256(canonical.as_bytes()))
```

---

## 4. BDD 行为规格

```gherkin
Feature: parallel-forge 关键阶段证据门禁

  Background:
    Given builtin parallel-forge 已 normalize（precheck desugar 已运行）
    And event_policy.mode 为 enforce 且 on_violation 为 reject_with_resume

  Scenario: S1 合法初始扇出通过并唤醒 dispatcher
    Given worktree 对 RALPH_WORKSPACE_ROOT 算出非空 target_start_sha 与 fingerprint
    And worktree-map 与 execution-plan 文件存在
    When worktree emit forge.worktrees.ready（经 .proposed）
    Then payload_consistency 未命中
    And precheck-forge.worktrees.ready 转发 accepted forge.worktrees.ready
    And forge-dispatcher 被该 accepted 事件触发
    And 不出现 forge.worktrees.ready.rejected

  Scenario: S2 空 verified_base_commit 在 LLM 前被拒绝
    When worktree emit forge.wave.worktrees.ready.proposed 且 verified_base_commit 为 ""
    Then 命中 rule id parallel-forge-worktrees-ready-empty-base（已有）或同等空 SHA 规则
    And 不出现 hat precheck-forge.wave.worktrees.ready 的激活
    And 不出现 exec.unit.ready

  Scenario: S3 目标分支与 RALPH_CURRENT_BRANCH 不一致则 precheck 拒绝
    Given payload.target_branch != env RALPH_CURRENT_BRANCH
    When 通过 consistency 的 forge.worktrees.ready.proposed 到达 gate
    Then gate emit forge.worktrees.ready.rejected
    And dispatcher 不被激活

  Scenario: S4 指纹或 map 不可验证则拒绝，修复后通过
    Given worktree-map 缺失或 fingerprint 与现场 porcelain 不一致
    When 第一次 proposed
    Then rejected 且 resume target 为 worktree
    When 第二次 proposed 证据已修复
    Then accepted forge.worktrees.ready 恰好一次

  Scenario: S5 扇出门禁耗尽收敛到 forge.plan.blocked
    When 同一 worktree 身份证据连续 3 次被 precheck 拒绝
    Then runtime 发出 forge.plan.blocked 且 payload.kind=precheck_exhausted
    And 不出现 accepted forge.worktrees.ready
    And cleanup 可被 forge.plan.blocked 触发

  Scenario: S6 合法 wave review 唤醒 integrator 一次
    Given review.md 覆盖每个 fan-in Unit 且与 unit_verdicts、aggregate_verdict=ACCEPTED 一致
    When reviewer emit forge.wave.reviewed
    Then accepted 后 integrator 激活恰好一次

  Scenario: S7 aggregate ACCEPTED 但存在 REJECTED unit 被拒
    When proposed 的 aggregate_verdict=ACCEPTED 且 unit_verdicts 含 REJECTED
    Then precheck rejected；integrator 不激活

  Scenario: S8 空 settled_task_ids 确定性拒绝且 TaskStore 不变
    When integrator emit forge.wave.settled.proposed 且 settled_task_ids=[]
    Then consistency Hit（eq: []）
    And CloseTaskBatch 不运行
    And 依赖 wave 仍不可 dispatch

  Scenario: S9 合法 settlement 原子关闭精确 batch
    Given settled_task_ids 为非空 live id 数组且与 settled_unit_ids 等长
    And settlement.md 与 verification 存在
    When accepted forge.wave.settled
    Then CloseTaskBatch 关闭恰好这些 id
    And 仅依赖该 wave 的后继可 dispatch

  Scenario: S10 两独立 wave 仍可并发 settle
    Given 现有 parallel_forge_two_wave_settlement_runtime 拓扑
    When 两条 wave 按原 fixture 推进
    Then 两个 forge.wave.settled 仍各出现一次（回归 R8）

  Scenario: S11 tester 全量失败不再直发 work.failed
    When tester 全量命令失败
    Then tester 只允许 emit forge.full.verification.failed
    And work.failed 的 origin 只能是 forge-failure-handler 或 precheck-work.failed 转发

  Scenario: S12 可恢复 verifier 失败走 correction 而非 work.failed
    When forge.verification.failed 到达 failure-handler 且 round<3
    Then emit forge.correction.requested
    And work.failed 不出现

  Scenario: S13 合法死胡同 work.failed 只落地一次
    Given correction 已耗尽或永久 class，artifact 支持 confidence≥90 coverage≥75
    When failure-handler emit work.failed
    Then consistency 未命中结构矛盾
    And precheck 转发 accepted work.failed 恰好一次

  Scenario: S14 结构矛盾死胡同 payload 零 LLM 拒绝
    When work.failed.proposed 含 dead_end_gate_passed=true 且 failure_evidence_status=incomplete
    Then consistency Hit；precheck-work.failed 不激活

  Scenario: S15 work.failed precheck 耗尽
    When 连续 3 次 work.failed.rejected
    Then forge.plan.blocked kind=precheck_exhausted
    And accepted work.failed 不出现
    And cleanup/report 仍可达

  Scenario: S16 稳定目标 + ACCEPTED audit 才 merge
    Given target HEAD 与 fingerprint 相对 fan-out 快照未变
    And git merge-base --is-ancestor target_start_sha integration_head 成功
    When auditor emit forge.audit.done verdict=ACCEPTED
    Then finalizer 激活并执行 ff-only
    And forge.finalized.target_commit_sha 等于 merge 后 HEAD

  Scenario: S17 目标漂移则永不 merge
    Given fan-out 后 target HEAD 或 fingerprint 变化
    When forge.audit.done.proposed
    Then rejected；finalizer 零激活；不执行 git merge

  Scenario: S18 非祖先 integration HEAD 拒绝 merge
    When merge-base --is-ancestor 失败
    Then audit proposed 被拒；无 merge

  Scenario: S19 false-success 全量验证被拒
    When forge.full.verified 且 all_required_passed=false
    Then consistency Hit；auditor 不激活

  Scenario: S20 报告 status/final_audit 矛盾被拒
    When forge.report.done status=COMPLETED 且 final_audit!=ACCEPTED
    Then consistency Hit；LOOP_COMPLETE 不出现

  Scenario: S21 exec.unit.done 无新 guard
    When executor emit exec.unit.done
    Then 无 precheck-exec.unit.done hat
    And 无针对该 topic 的新 consistency rule

  Scenario: S22 inspector 空输出仍 no_progress
    When 沿用 presets/scenarios/parallel-forge-no-output.yml
    Then accepted_events 为空且无 LOOP_COMPLETE
```

---

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
| --- | --- | --- | --- | --- | --- |
| S1,S3,S4,S5 | dispatcher 激活次数、rejected/blocked topic | 新增 `parallel_forge_worktrees_ready_gate_runtime.yml` | 集成 EventLoop | Characterization：现有 dispatch 场景仍绿 | 否 |
| S2 | empty base 无 precheck hat 事件 | 现有 empty-base 规则 + 同文件断言 `absent_events` 含 gate 转发 | 集成 | 无 | 否 |
| S6,S7 | integrator 次数 | `parallel_forge_wave_reviewed_gate_runtime.yml` | 集成 | 无 | 否 |
| S8,S9 | TaskStore 开/关集合 | `parallel_forge_wave_settled_gate_runtime.yml` | 集成 | projector 单测已有 CloseTaskBatch，作回归 | 否 |
| S10 | 两 settled | 现有 `test_parallel_forge_two_wave_settlement_runtime` | 回归 | 无 | 否 |
| S11 | publishes/deny + 迷你 BDD | `presets.rs` 结构测试 + `parallel_forge_tester_typed_failure_runtime.yml` | 单元结构+集成 | 无 | 否 |
| S12 | 现有 correction 场景 | `test_parallel_forge_correction_runtime` / `exec_wave_failed_correction` | 回归 | 无 | 否 |
| S13–S15 | 对照 pipeline fail-gate YAML | `parallel_forge_work_failed_gate_runtime.yml` | 集成 | 对照 E15 | 否 |
| S16–S18 | finalizer mock 不跑真 merge：断言 **不出现** finalizer hat 激活 / 或 mock 记录 merge 调用次数=0\|1 | `parallel_forge_audit_gate_runtime.yml` | 集成 | 不在 BDD 里对真实用户仓库 merge | 否 |
| S19,S20 | consistency Hit | 可与 U10 同一文件多 mock | 集成 | 无 | 否 |
| S21 | parse 后 hats 无 `precheck-exec.unit.done` | `presets.rs` | 结构 | 无 | 否 |
| S22 | CLI verify JSON | `ralph preset verify --scenario presets/scenarios/parallel-forge-no-output.yml` | CLI 动态 | 无 | 否 |

**断言约定：** `expected.events` 列 accepted 业务 topic；`absent_events` 列不应出现的 topic；耗尽场景断言 `forge.plan.blocked` 且 payload 含 `"kind":"precheck_exhausted"`。  
**运行：** `cargo nextest run -p ralph-core --test scenarios -- <name>`。  
**预期失败原因（Red）：** 缺 hat/缺 rule/缺字段 → 事件被接受或下游被唤醒；有效 Red **不是** 编译错误或 fixture YAML 语法错误。

---

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元/结构 | 集成 | E2E | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| R1 | 扇出前身份/快照/artifact | S1–S5 | worktrees gate YAML | schema required_fields | 是 | 否 | E2,E3,E4 |
| R2 | review artifact+verdicts | S6,S7 | reviewed gate YAML | 无 | 是 | 否 | schema 163-197 |
| R3 | settlement 才关 task | S8,S9 | settled gate YAML | CloseTaskBatch 既有单测 | 是 | 否 | E7 |
| R4 | 单 writer + 失败证据 | S11–S15 | ownership 结构测试+fail gate YAML | presets.rs | 是 | 否 | E5,E6 |
| R5 | merge 前目标未漂 | S16–S18 | audit gate YAML | 无 | 是 | 否 | E8 |
| R6 | 收据结构矛盾 | S19,S20 | consistency YAML | lint unknown_field | 是 | 否 | E11 |
| R7 | 独立 budget 3 + 耗尽 blocked | S5,S15 | exhaust YAML | desugar max_activations=budget+1 | 是 | 否 | E9,E12 |
| R8 | 不串行 Unit | S10,S21 | 既有 two-wave + 结构断言 | presets.rs | 是 | 否 | E1 |
| R9 | 下游同步 | U11 清单 | strict lint + run-tests.sh | presets/skills anchors | 是 | 否 | E14 |

---

## 7. 最大并发开发单元

以下 Unit 编号仅用于引用，不表示执行顺序。每个 Unit 都在独立 worktree/branch
中完成完整的 Acceptance Red → Unit Red → Green → Refactor → Integration →
Regression → Close；只有表中 `depends_on` 的 Release Gate 会释放后继 Unit。

```text
Validated Baseline e94c0782
  ├──────────────┬──────────────┬──────────────┐
  ↓              ↓              ↓              ↓
 U1 notes     U2 writer     U3 identity   U10 receipts
                 │              │             │
                 └──────┬───────┘             │
                        ↓                     │
                       U4 precheck base       │
                 ┌──────┼──────┬──────┬──────┐ │
                 ↓      ↓      ↓      ↓      ↓ │
                U5     U6     U7     U8     U9 │
                 └──────┴──────┴──────┴──────┴─┘
                                      ↓
                                     U11
```

`U1`, `U2`, `U3`、`U10` 从当前基线立即并发。`U4` 只等待 U3 的字段契约；U2
没有真实数据依赖，不能阻塞 U4。U5–U9 在 U4 的基础 precheck Release Gate
完成后立即并发；U11 才是真正的 Fan-In。U5–U9 之间不得因同一 preset 文件而
人为串行：它们写入不同的 schema/preset 语义区域，合并时按 YAML key 做
deterministic conflict review。

### Unit 并发执行元数据

| Unit | Wave ID | depends_on | ready_when / release_condition | blocks | can_run_parallel_with | worktree / branch | 预计写集合 | 验证资源 | 合并约束 |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| U1 | P0 | [] | 基线 checkout 完成；Close=notes 与 DAG characterization 回归通过 | 无 | U2,U3,U10 | `../pf-u1` / `plan/pf-u1` | author notes；`parallel_forge_handoff.rs` 独立测试符号（如需补边界） | cargo nextest；可并发 | 不与 U2/U3 重叠同一测试符号 |
| U2 | P0 | [] | 基线 checkout 完成；Close=typed failure 拓扑/BDD/strict lint 通过 | U11 | U1,U3,U10 | `../pf-u2` / `plan/pf-u2` | tester/failure-handler YAML 区域；新 failure schema；typed-failure scenario；独立结构测试 | cargo nextest；可并发 | 与 U3 仅在不同 YAML key 合并 |
| U3 | P0 | [] | 基线 checkout 完成；Close=required fields、projection、fixture 回归通过 | U4 | U1,U2,U10 | `../pf-u3` / `plan/pf-u3` | `forge.worktrees.ready` schema、projection、worktree instructions、相关 fixture | cargo nextest；可并发 | U4 只能消费 U3 commit 后的字段 |
| U4 | P1 | [U3] | U3 Release Gate；precheck 基础与 initial fan-out gate 测试通过 | U5,U6,U7,U8,U9 | U10 | `../pf-u4` / `plan/pf-u4` | event_loop.precheck 基础、`forge.worktrees.ready` guard、对应 scenario | cargo nextest；LLM mock 可并发 | 先合入 U3，再合入其余 gate Unit |
| U5 | P2 | [U4] | U4 Release Gate；lazy wave gate 通过 | U11 | U6,U7,U8,U9,U10 | `../pf-u5` / `plan/pf-u5` | wave worktree schema/rules/instructions/scenario | cargo nextest；可并发 | 与 U6–U9 按 YAML key 合并 |
| U6 | P2 | [U4] | U4 Release Gate；review gate 通过 | U11 | U5,U7,U8,U9,U10 | `../pf-u6` / `plan/pf-u6` | reviewed rules/instructions/scenario | cargo nextest；可并发 | 同上 |
| U7 | P2 | [U4] | U4 Release Gate；settled gate + TaskStore 回归通过 | U11 | U5,U6,U8,U9,U10 | `../pf-u7` / `plan/pf-u7` | settled rules/instructions/scenario | cargo nextest；TaskStore fixture 独立 | 同上；不得改 U3 projection 区域 |
| U8 | P2 | [U2,U4] | U2 writer 与 U4 precheck Gate 均通过；failure gate 通过 | U11 | U5,U6,U7,U9,U10 | `../pf-u8` / `plan/pf-u8` | work.failed schema/rules/instructions/scenario | cargo nextest；可并发 | 消费 U2 新 failure topic，不改 U2 writer 语义 |
| U9 | P2 | [U3,U4] | identity 与 precheck Gate 均通过；audit gate 通过 | U11 | U5,U6,U7,U8,U10 | `../pf-u9` / `plan/pf-u9` | audit schema/rules/instructions/scenario | cargo nextest；merge mock/临时 repo 隔离 | finalizer 仅接受 accepted topic |
| U10 | P0 | [] | 基线 checkout 完成；receipt consistency 结构/BDD 通过 | U11 | U1,U2,U3,U4,U5,U6,U7,U8,U9 | `../pf-u10` / `plan/pf-u10` | 四个 receipt rule、相关 instructions、scenario、结构测试 | cargo nextest；可并发 | 不得触碰 precheck 或 exec.unit.done |
| U11 | P3 | [U1,U2,U3,U4,U5,U6,U7,U8,U9,U10] | 全部直接前驱 Release Gate；动态 verify 与下游审计完成 | 最终质量门禁 | 无 | `../pf-u11` / `plan/pf-u11` | dynamic scenarios、solution doc、必要下游文档 | verify/strict lint；最终资源队列 | deterministic merge 后才跑全量 |

所有 Unit 的基线 commit 均为 `e94c07822bce7eacbf9b05cc4920fd4ed0aa4316`。
预计写集合之外的文件只允许读取；发现需要修改共享区域时必须停止并更新本计划，
不能在 worktree 中临时扩大范围。

---

### Unit 1：冻结作者契约并保留 DAG 并发回归

#### 1. Unit 目标
让 `parallel-forge` 的 Gate Scope / key-stage 矩阵可评审，并保留当前已验证的 `depends_on` 并发语义；不得把 `execution_wave` 或 prose 的“串行执行”当作 worker admission 依据。

#### 2. 对应需求与 Scenario
R9 的契约前置；S10/S21 的并发回归前置。Decision D1–D3,D16–D17。Evidence E17,E18。

#### 3. 外部可观察结果
仓库出现 `presets/en/parallel-forge-preset-author-notes.md`；`cargo nextest` 中新结构测试通过。

#### 4. 当前行为基线
E17：当前 handoff 已按 DAG 重算 wave，并已有独立 Unit 被重新并发的回归测试。对照 notes：`presets/en/ce-executor-pipeline-preset-author-notes.md`。

#### 5. 输入与输出
输入：无运行时输入。输出：notes 文件 + 表征测试。无状态副作用。

#### 6. 修改位置
| 位置 | 职责 | 为何改 | 边界 |
| --- | --- | --- | --- |
| `presets/en/parallel-forge-preset-author-notes.md` **新增** | AAF Intent + Gate Scope hard + 完整 key-stage 表 | review skill 0e 要求 | 不改 YAML 行为 |
| `crates/ralph-core/src/parallel_forge_handoff.rs` 测试模块 | 已有 DAG 重算回归测试 | 如缺少边界覆盖，补 unknown dependency/cycle 的最小 characterization；不重复实现 e94c0782 | 不改生产 handoff 逻辑 |

已有回归测试 `artifact_first_handoff_rewrites_serial_layout_for_independent_units` 必须继续通过；若 Unit 1 增加边界覆盖，测试必须验证未知依赖或环被拒绝，不能锁定 prompt 文本。禁止新增「无 precheck」表征断言，因为当前基线已为未来 gate Unit 预留 `precheck` 变更方向。

#### 7–8. 可依赖 / 禁止依赖
可依赖 `get_preset("parallel-forge")` + `RalphConfig::parse_yaml`。禁止实现任何 gate YAML。

#### 9–12. 验收 / Red / 单测顺序
1. 先运行现有 DAG 回归测试，确认 e94c0782 基线为 Green；如需补 unknown dependency/cycle characterization，先验证新增测试在当前实现上 Green，不得伪造缺陷 Red。
2. 写 notes 并通过 author/review anchor 校验；notes 缺失或矩阵不完整时才是本 Unit 的有效 Red。
3. 写 notes（字段必须含 execution_model: **supervisor+wave**，与 YAML `supervisor.enabled: true` + wave emit 一致）。  
4. `cargo nextest run -p ralph-core --lib artifact_first_handoff_rewrites_serial_layout_for_independent_units`

#### 13. 最小实现
只新增 notes + 测试。notes 中矩阵必须与本计划 §Key-stage 表逐行一致（见下）。

Key-stage 表（抄进 notes，confirmation_status=confirmed）：

| key_stage | topic | guard_selection | precheck | p_budget | consistency | c_budget |
| --- | --- | --- | --- | --- | --- | --- |
| initial fan-out | forge.worktrees.ready | both | true | 3 | true（规则 topic=`.proposed`） | 3 |
| lazy wave fan-out | forge.wave.worktrees.ready | both | true | 3 | true | 3 |
| review | forge.wave.reviewed | both | true | 3 | true | 3 |
| settlement | forge.wave.settled | both | true | 3 | true | 3 |
| merge auth | forge.audit.done | both | true | 3 | true | 3 |
| terminal failure | work.failed | both | true | 3 | true | 3 |
| dev fan-in | forge.exec.development.done | payload_consistency | false | null | true | 3 |
| full verify | forge.full.verified | payload_consistency | false | null | true | 3 |
| post-merge | forge.finalized | payload_consistency | false | null | true | 3 |
| report | forge.report.done | payload_consistency | false | null | true | 3 |
| slot done | exec.unit.done | neither | false | null | false | null |

#### 14–20
集成：`ralph preset check -H builtin:parallel-forge --strict --format json` 通过。回归：DAG 测试与 preset check。完成：notes 存在且测试绿。停止：若 handoff 仍按静态 `execution_wave` 串行化独立 Unit，或未知依赖/环未拒绝，停并更新 D16/D17。

---

### Unit 2：`work.failed` 单 writer 与 tester typed failure

#### 1. 目标
tester 全量失败改为 `forge.full.verification.failed`；integrator/verifier/tester/dispatcher **不能** publish `work.failed`；failure-handler 成为唯一业务 publisher。

#### 2. 对应
R4；S11,S12；D5；E5,E6。

#### 3. 可观察
`RalphConfig::parse_yaml` 后：仅 `forge-failure-handler` 的 `publishes` 含 `work.failed`（合成 `precheck-work.failed` 尚未存在）。tester `publishes` 含 `forge.full.verification.failed` 不含 `work.failed`。`topic_deny_rules` 含 tester/integrator/verifier/dispatcher × `work.failed`。

#### 4. 基线
E5。U2 自己新增 writer 结构断言；不得修改 U1 的 DAG characterization 或等待 U1 Close。

#### 5. IO
新 topic required_fields（锁死）：`plan_key`, `verification_report_path`, `failure_fingerprint`, `context_artifact_path`, `forge_artifact_root`, `reason`。  
failure-handler 在该 trigger 上：**不**发 `forge.correction.requested`；写 failures artifact 后发 `work.failed`（证据字段在 U8 才变为 required；本 Unit payload 仍用现有 5 字段即可，避免跨 Unit 依赖）。

#### 6. 修改位置
* `presets/schemas/parallel-forge.yml`：在 `forge.full.verified` 旁 **新增** `forge.full.verification.failed`（field_docs 齐）。  
* `presets/en/parallel-forge.yml`：
  * `mechanism.flow` `full_verify.allowed_emits` 加入该 topic；可保留 `work.failed`（handler 终态）。  
  * tester：`publishes`/`exempt_topics`/`terminal_events` 去掉 `work.failed`，加入新 topic；instructions 第 6 步改为 emit 新 topic，禁止 `work.failed`。  
  * integrator/verifier：从三列表删除 `work.failed`（instructions 已禁止）。  
  * `topic_deny_rules` 增加四条 hat×work.failed（dispatcher 已有，保留）。  
  * failure-handler `triggers`/`event_filter.events` 增加新 topic；instructions 增加「当 trigger 为 forge.full.verification.failed：重建 plan_key，写失败 artifact，emit work.failed；不要 correction.requested」。  
* `crates/ralph-cli/src/presets.rs`：新测试 `parallel_forge_work_failed_single_writer`。  
* 新增 `crates/ralph-core/tests/scenarios/parallel_forge_tester_typed_failure_runtime.yml` + `scenarios.rs` 函数 `test_parallel_forge_tester_typed_failure_runtime`。

不改：executor publishes、CloseTaskBatch、supervisor。

#### 9. 验收测试
迷你环：tester 收 `forge.exec.development.done` → emit `forge.full.verification.failed` → handler emit `work.failed` → cleanup/reporter 可省略（断言 handler 发出 work.failed 且 tester 未发）。  
运行：`cargo nextest run -p ralph-core --test scenarios -- tester_typed_failure`  
`cargo nextest run -p ralph-cli --bin ralph -- parallel_forge_work_failed_single_writer`

#### 10. Acceptance Red
结构测试失败原因：tester.publishes 仍含 work.failed。BDD 失败原因：seen_topics 含 tester 源的 work.failed 或不含新 topic。

#### 11–12 TDD 顺序
`Test writer 集合 Red` → 改 publishes/deny → Green → `BDD tester 失败路径 Red` → 改 instructions+handler trigger → Green → `strict lint`。

#### 13. 最小实现
拓扑+instructions+schema+测试。**不**加 precheck。

#### 16. 回归
`cargo nextest run -p ralph-core --test scenarios -- parallel_forge`  
`cargo nextest run -p ralph-cli --bin ralph -- presets`  
原因：ownership/WAC 可能打破 activation。

#### 18. 完成标准
S11 结构+BDD 绿；strict lint 绿；S12 旧 correction 测试仍绿。

---

### Unit 3：扇出身份字段进入契约

#### 1. 目标
`forge.worktrees.ready` 必须携带 `target_start_sha` 与 `target_status_fingerprint`；投影给 auditor/finalizer；schema 修正 `target_branch` 来源。

#### 2. 对应
R1,R5 的数据前置；D7,D8；E2,E3。

#### 3. 可观察
缺两新字段的 emit `--policy-check` 失败（required_fields）。投影键 `forge.target_start_sha`、`forge.target_status_fingerprint` 存在。

#### 4. 基线
E2 六字段。现有 BDD 的 worktrees.ready payload 缺新字段 → **本 Unit 必须同步所有 `parallel_forge_*` fixture 里该 topic 的 JSON**，否则集成测会 Red（这是有效 Red：schema 收紧）。

#### 5. IO
`target_start_sha`：40-char hex 字符串。`target_status_fingerprint`：64-char hex SHA-256。  
worktree instructions Step 5–6：按 D8 计算后写入 payload。缺 `RALPH_CURRENT_BRANCH` 仍走 `forge.plan.blocked`（已有）。

#### 6. 修改位置
* schema `forge.worktrees.ready` required_fields + field_docs（target_branch source 改为 `RALPH_CURRENT_BRANCH`）。  
* YAML `state_projection` 在 `on: forge.worktrees.ready` 的 `set:` 增加两键。  
* worktree instructions。  
* 所有场景 YAML 中该 payload（grep `forge.worktrees.ready`）。

#### 9. 验收
新增/扩展 `presets.rs`：`parallel_forge_worktrees_ready_requires_target_identity` 读 schema required_fields。  
跑：`cargo nextest run -p ralph-core --test scenarios -- parallel_forge` 必须绿（fixture 已补字段）。

#### 10. Red
结构测试：required_fields 不含新名。场景：policy 缺字段拒绝导致 expected event 缺失。

#### 13. 不实现
不在本 Unit 写 precheck。fingerprint 算法只写进 instructions，不写 Rust hasher。

#### 16. 回归
全 `parallel_forge` scenarios + `preset_lint` + `presets`。

---

### Unit 4：`forge.worktrees.ready` 双 guard

#### 1. 目标
错误 target / 空 SHA / 空 fingerprint / 缺失 map 不能唤醒 dispatcher。

#### 2. 对应
S1,S3,S4,S5；R1,R7；D2,D9,D10。

#### 3. 可观察
accepted `forge.worktrees.ready` 仅在 gate 转发后出现；dispatcher 只订阅 accepted。

#### 4. 基线
无 gate hat。dispatcher triggers 已含 `forge.worktrees.ready`（E 保持）。

#### 5. IO / 错误
consistency（**topic: `forge.worktrees.ready.proposed`**）规则 id 锁定：
* `parallel-forge-worktrees-ready-empty-target-branch`：`{field: target_branch, eq: ""}`
* `parallel-forge-worktrees-ready-empty-start-sha`：`{field: target_start_sha, eq: ""}`
* `parallel-forge-worktrees-ready-empty-fingerprint`：`{field: target_status_fingerprint, eq: ""}`
* `parallel-forge-worktrees-ready-empty-map-path`：`{field: worktree_map_path, eq: ""}`

precheck key `forge.worktrees.ready`：
```
on_fail.target: worktree
retry_budget: 3
on_exhausted: "forge.plan.blocked(reason=precheck_failed)"
reason: "worktree_identity_evidence_insufficient"
```
prompt 必须逐步：打开 map；`test -f`；`target_branch` == `$RALPH_CURRENT_BRANCH`；在 `$RALPH_WORKSPACE_ROOT` 重算 SHA 与 fingerprint 与 payload 相等；hard 阈值。通过则 **verbatim** 转发 `forge.worktrees.ready`。  
recovery_guidance.common：打开 map、重算 fingerprint，禁止只改字段。

#### 6. 修改
YAML `event_loop.precheck.enabled: true` + 本条 rule；`payload_consistency.rules` 追加上述 id。  
**同步改 Unit 1 表征测试为正向：** `precheck.rules` 含该 key；hats 含 `precheck-forge.worktrees.ready`；`max_activations == 4`。  
新增 `parallel_forge_worktrees_ready_gate_runtime.yml`。  
worktree instructions：强调 `--policy-check`；被拒后只修证据。

#### 9. 验收文件
`crates/ralph-core/tests/scenarios/parallel_forge_worktrees_ready_gate_runtime.yml`  
函数 `test_parallel_forge_worktrees_ready_gate_runtime`  
场景内嵌与 preset **相同**的 4 条 consistency + 1 条 precheck（文件头注释：与 `presets/en/parallel-forge.yml` 同步）。

断言：
* 合法路径：events 含 `forge.worktrees.ready`，dispatcher mock 跑一次。  
* 空 sha：absent `forge.worktrees.ready`，absent gate hat 输出（consistency 先拒）。  
* 分支不一致：`forge.worktrees.ready.rejected` 一次，无 accepted。  
* 三次拒绝：`forge.plan.blocked`。

命令：`cargo nextest run -p ralph-core --test scenarios -- worktrees_ready_gate`

#### 10. Red
合法路径失败：dispatcher 在 `.proposed` 上被触发（说明 triggers 被误改成 proposed）。那是错误实现，必须停。正确 Red：无 precheck 时 rejected 场景里事件被直接接受。

#### 12. 顺序
空 SHA consistency 单场景 Red→加 rule Green → 分支 mismatch precheck Red→加 precheck Green → exhaust Red→budget 已是 3 Green → 合法路径。

#### 13. 不实现
wave.worktrees.ready 的新 precheck（Unit 5）。不要改 dispatcher 去订阅 `.proposed`。

#### 16. 回归
`parallel_forge` 全套（fixtures 的 worktrees.ready 必须能过新规则：非空 SHA/fingerprint）。

---

### Unit 5：`forge.wave.worktrees.ready` 双 guard

#### 1. 目标
lazy wave 空 base / 空 map / 空 wave_id 不能 fan-out。

#### 2. 对应
S2；R1；已有 rule `parallel-forge-worktrees-ready-empty-base` **保留 id**，并把 `topic` 改为 `forge.wave.worktrees.ready.proposed`（否则双 guard 后不再命中，E10）。

#### 3–6
新增规则：
* `parallel-forge-wave-worktrees-empty-map`：`worktree_map_path eq ""` topic `.proposed`
* `parallel-forge-wave-worktrees-empty-wave-id`：`wave_id eq ""`

precheck key `forge.wave.worktrees.ready`：target=`worktree`，budget 3，on_exhausted 同 D9，reason=`wave_worktree_evidence_insufficient`。  
prompt：`git cat-file -t` 校验 `verified_base_commit`；map 仅含本 wave；路径存在。

**改 topic 后必须更新任何依赖旧 topic 字符串的测试。** grep `parallel-forge-worktrees-ready-empty-base`。

测试文件：`parallel_forge_wave_worktrees_ready_gate_runtime.yml`。

回归：two-wave settlement（prepare→worktrees.ready 链）。

禁止：改 `exec.unit.done`。

---

### Unit 6：`forge.wave.reviewed` 双 guard

#### 1. 目标
无报告 / 矛盾 aggregate 不能叫醒 integrator。

#### 2. 对应
S6,S7；R2。

consistency topic `forge.wave.reviewed.proposed`：
* `parallel-forge-reviewed-empty-report`：`review_report_path eq ""`
* `parallel-forge-reviewed-not-accepted`：`aggregate_verdict ne "ACCEPTED"`  
  （REJECTED 必须走 `forge.wave.review.failed`，与现有 reviewer step 6 一致）

precheck target=`reviewer`，reason=`wave_review_evidence_insufficient`。  
prompt：打开 `review_report_path`；每个 fan-in Unit 一条；与 `unit_verdicts` 一致；ACCEPTED 当且仅当全部 ACCEPTED。

reviewer instructions：禁止在 REJECTED 时发 `forge.wave.reviewed`。

测试：`parallel_forge_wave_reviewed_gate_runtime.yml`。  
Red：无 gate 时 S7 payload 会激活 integrator。

禁止：本 Unit 改 CloseTaskBatch。

---

### Unit 7：`forge.wave.settled` 双 guard

#### 1. 目标
空/非法 settlement 不能关 task、不能解锁后继 wave。

#### 2. 对应
S8,S9,S10；R3；E7；D11。

将已有 `parallel-forge-settled-empty-base` 的 topic 改为 `forge.wave.settled.proposed`。  
新增：
* `parallel-forge-settled-empty-task-ids`：`settled_task_ids eq []`
* `parallel-forge-settled-empty-unit-ids`：`settled_unit_ids eq []`
* `parallel-forge-settled-empty-log`：`settlement_log_path eq ""`

precheck target=`integrator`，reason=`wave_settlement_evidence_insufficient`。  
prompt：log 存在；ids 为 JSON 数组非逗号字符串；与 `ralph tools task list` live id 一致；`verified_base_commit` 等于 integration HEAD。

integrator instructions 已有 resume 纪律，补一句：rejected 时 **禁止** `ralph tools task close`。

测试：`parallel_forge_wave_settled_gate_runtime.yml` 必须 **读 TaskStore**（与 `parallel_forge_resume_wave_replay_runtime.yml` 同样断言 close 集合）。  
S10：原样跑 `test_parallel_forge_two_wave_settlement_runtime`。

Red：空数组仍 `CloseTaskBatch` 报错或关闭 0 个仍前进——必须 consistency 在投影前拒绝。  
注意：`project_close_task_batch` 对空数组已经 Err（`state_projector/task.rs:810`）；本 Unit 要在 **更早** 的 emit 路径拒绝，避免 integrator 进入投影错误而非 `.rejected`。

禁止：给 `exec.unit.done` 加 close_task。

---

### Unit 8：`work.failed` 证据字段与双 guard

#### 1. 目标
假失败（无证据、门未过、correction 未耗尽却声称 dead-end）不能成为 accepted `work.failed`。

#### 2. 对应
S13–S15；R4,R6；D6。

schema `work.failed` **追加 required_fields**（锁死名字）：
`dead_end_confidence`, `dead_end_evidence_coverage`, `dead_end_evidence_file`, `failure_evidence_status`, `correction_status`, `dead_end_gate_passed`  
保留原 `reason/plan_path/context_artifact_path/forge_artifact_root/plan_key`。

`allowed_values`：
* `failure_evidence_status`: `[complete, incomplete]`
* `correction_status`: `[exhausted, not_exhausted, not_applicable]`
* `dead_end_gate_passed`: 布尔；用 `field_types` 或 schema 现有 bool 惯例（对照 `all_required_passed`：BDD 用 JSON bool）。

consistency topic `work.failed.proposed`：
* `parallel-forge-failed-empty-context`：`context_artifact_path eq ""`
* `parallel-forge-failed-empty-evidence-file`：`dead_end_evidence_file eq ""`
* `parallel-forge-failed-gate-without-complete-evidence`：`all: [{dead_end_gate_passed eq true}, {failure_evidence_status eq incomplete}]`
* `parallel-forge-failed-gate-without-exhausted-correction`：`all: [{dead_end_gate_passed eq true}, {correction_status eq not_exhausted}]`

**禁止** `{dead_end_confidence gte 90}`。

precheck target=`forge-failure-handler`，reason=`dead_end_evidence_insufficient`。  
prompt：对齐 pipeline work.failed 步骤，打开 `dead_end_evidence_file`；独立评分 ≥90 / ≥75；`dead_end_gate_passed` 必须与评分一致；verbatim 转发。

failure-handler instructions 终态 bullet：填齐新字段。

测试：`parallel_forge_work_failed_gate_runtime.yml`（结构抄 `ce_executor_pipeline_fail_gate_rejected_then_pass.yml` 与 `..._exhaust.yml`，on_exhausted 用 `forge.plan.blocked`）。

**同步所有** 现有 fixture 里的 `work.failed` payload（grep）。

回归：`test_parallel_forge_fail_close_runtime`、blocked 动态场景。

---

### Unit 9：`forge.audit.done` 双 guard（阻止错误 merge）

#### 1. 目标
非 ACCEPTED、目标漂移、非祖先 → finalizer 零激活。

#### 2. 对应
S16–S18；R5；D4,D14。

schema 追加 required（从投影回填，auditor 必须抄到 payload）：`target_branch`, `target_start_sha`, `target_status_fingerprint`, `integration_head_sha`  
`allowed_values.verdict`: `[ACCEPTED, REJECTED, BLOCKED]`（若尚未声明则补上）。

consistency topic `forge.audit.done.proposed`：
* `parallel-forge-audit-not-accepted`：`verdict ne "ACCEPTED"`
* `parallel-forge-audit-empty-report`：`audit_report_path eq ""`
* `parallel-forge-audit-empty-start-sha`：`target_start_sha eq ""`

precheck target=`auditor`，reason=`final_audit_evidence_insufficient`。  
prompt：打开 `final-audit.md` 确为 ACCEPTED；重解 `$RALPH_CURRENT_BRANCH`/`$RALPH_WORKSPACE_ROOT`；HEAD 与 payload.start_sha 相等；fingerprint 按 D8 复算相等；`git merge-base --is-ancestor <target_start_sha> <integration_head_sha>`。

auditor instructions：非 ACCEPTED → `forge.plan.blocked`，不要发 audit.done。  
finalizer：**零改 merge 命令**；triggers 保持 `forge.audit.done`（accepted）。

测试：`parallel_forge_audit_gate_runtime.yml`：finalizer mock 若被触发则 emit `forge.finalized`；漂移场景 `absent_events: [forge.finalized]`。

禁止：改 `git merge` 实现代码。

---

### Unit 10：四个收据 topic 仅 consistency

#### 1. 目标
false-success 与空收据字段在 emit 时被拒，无新 LLM hat。

#### 2. 对应
S19,S20,S21；R6,R8；D3。

规则（topic 为 **bare** 名，因为无 precheck、无 rewrite）：

`forge.exec.development.done`：
* `parallel-forge-dev-done-empty-plan`：`execution_plan_path eq ""`
* `parallel-forge-dev-done-nonzero-failed`：`failed_unit_count gt 0`

`forge.full.verified`：
* `parallel-forge-full-verified-false`：`all_required_passed eq false`  （JSON bool）
* `parallel-forge-full-verified-empty-report`：`verification_report_path eq ""`

`forge.finalized`：
* `parallel-forge-finalized-empty-sha`：`target_commit_sha eq ""`
* `parallel-forge-finalized-empty-target-branch`：`target_branch eq ""`
* `parallel-forge-finalized-empty-report`：`finalization_report_path eq ""`

`forge.report.done`：
* `parallel-forge-report-empty-path`：`report_path eq ""`
* `parallel-forge-report-completed-without-accepted-audit`：`all: [{status eq COMPLETED}, {final_audit ne ACCEPTED}]`

结构测试：`!hats.contains_key("precheck-exec.unit.done")` 且 consistency rules 无 `topic == exec.unit.done`。

测试文件可合并为 `parallel_forge_receipt_consistency_runtime.yml` 多个 mock_responses 段落 **或** 每 topic 一个 `#[test]`——必须 **逐条** Red 再 Green，禁止一次提交四条无 Red。

tester/dispatcher/finalizer/reporter instructions：false 时不要发 success topic（tester 已改 failed topic）。

---

### Unit 11：动态 verify 与下游同步

#### 1. 目标
CLI `ralph preset verify` 覆盖 success / 拒绝后修复 / 仍 blocked / no-output；下游清单有证据。

#### 2. 对应
R7–R9；S22。

#### 6. 修改
* **新增** `presets/scenarios/parallel-forge-success.yml`：有界 mock 走到 `LOOP_COMPLETE`，payload 满足全部新 required_fields 与 consistency miss。  
* **新增** `presets/scenarios/parallel-forge-evidence-recovery.yml`：一次 `.rejected` 后修复再 accepted。  
* 更新 `parallel-forge-blocked.yml` 仅当 schema 需要（inspector blocked 路径不应被新 required 打断）。  
* `docs/solutions/workflow-orchestration/parallel-forge-evidence-gates.md` **新增**（实现后的教训，禁止 commit `.ralph/review/**`）。  
* 下游审计表写入该 solution（见下）。  
* CLI/finding/注入 skill 的 DAG 语义已在基线 e94c0782 同步；Executor 只需审计是否有证据门禁变更造成新的文档 drift，不得回退为串行语义。

下游清单（Executor 逐项打开确认，无改动写「no-op + 原因」）：

| 下游 | 预期 |
| --- | --- |
| `event_loop` step-close / `inject_completion_correction` | no-op：终态仍 `LOOP_COMPLETE` / `forge.report.done` |
| `preset_lint/*` | 无删 finding；跑现有测试 |
| BDD `scenarios/*.yml` | 已在 U2–U10 更新 |
| `loop_config.rs` / `PRESET_OPT_IN*` | no-op（E14） |
| `crates/ralph-cli/src/presets.rs` PRESETS 名字 | no-op |
| `presets/manifest.yml` `index.json` | no-op |
| `CLAUDE.md`/`AGENTS.md` parallel-forge 一句：可补「关键 handoff 双 guard」 | 若改 CLAUDE 必须 `cp CLAUDE.md AGENTS.md` |
| `.cursor/rules` / zsh plugin | no-op（未改 preset 名） |
| `crates/ralph-core/data/*.md` | 已由基线 e94c0782 更新 `ralph-tools-wave.md` 的 DAG/资源边界；本计划不重复改，仍须回归 drift 检查 |
| author/review skill | 已由基线 e94c0782 同步 `patterns.md`/`finding-rubric.md` 的 DAG 语义；本计划不重复改，仍须跑 anchor 测试 |

验证命令见 §9。动态 verify **必须**断言 JSON `passed` / `accepted_events` / `failure_kind`，禁止只看 exit 0。

---

## 8. Unit 最大并发依赖图

```text
Validated Baseline
  ├──────────────┬──────────────┬──────────────┐
  ↓              ↓              ↓              ↓
 U1             U2             U3             U10
                                ↓
                                U4
                  ┌─────────────┼─────────────┬─────────────┬─────────────┐
                  ↓             ↓             ↓             ↓             ↓
                 U5            U6            U7            U8            U9
                  └─────────────┴─────────────┴─────────────┴─────────────┴──→ U11
```

### Serial Edge Ledger

| Edge | 为什么必须串行 | Evidence | 是否尝试拆分/隔离 | 为什么仍不能并发 | 置信度 |
| --- | --- | --- | --- | --- | --- |
| U3→U4 | U4 的 consistency/precheck 规则引用 U3 新增的 required fields，字段不存在时 `unknown_field` 会使 Red 失真 | E2,E9,E16 | 已将 U3 限定为字段/schema/projection，U4 限定为 gate | U4 必须消费 U3 Release Gate 后的稳定字段契约 | 0.94 |
| U4→U5 | U5 依赖已启用并可运行的 precheck desugar/gate 基础 | E9,E15,E16 | U5 使用独立 topic、独立 fixture | 没有 U4 的基础，U5 的失败无法归因于 wave gate | 0.91 |
| U4→U6 | 同上，review gate 必须消费 precheck 基础 | E9,E15,E16 | U6 与 U5 写集合分离 | 仅共享框架能力，不依赖 U5 的 review 前置业务结果 | 0.91 |
| U4→U7 | settled gate 必须在 precheck 基础上运行且投影只消费 accepted topic | E7,E9,E16 | U7 不改 projection | U7 可在 U4 后独立实现，不等待 U5/U6 | 0.92 |
| U4→U9 | audit gate 同样依赖 accepted/proposed rewrite | E8,E9,E16 | finalizer 不改 merge 实现 | U9 只消费 U4 与 U3，不依赖 review/settlement Unit 的代码 | 0.92 |
| U2→U8 | U8 的 failure-handler trigger 必须消费 U2 新增的 typed failure topic | E5,E6 | 已把 U2 限为 writer/topic，U8 限为证据字段/gate | 新 topic 未经 U2 Release Gate 时，U8 无法验证真实 failure 路径 | 0.92 |
| U1,U2,U3,U4,U5,U6,U7,U8,U9,U10→U11 | 动态 verify success/recovery 必须使用最终 schema、拓扑和所有 rule | E13,E15,E16 | U11 不实现新规则，只汇合验证 fixture | 这是唯一真实 Fan-In；U11 不应提前锁定会被前驱改变的 payload | 0.90 |

以上之外没有串行边。U5/U6/U7/U8/U9 之间没有真实因果依赖；同一
`parallel-forge.yml` 的不同 YAML key 区域可以独立编辑，合并时若出现同一
符号/语义区域冲突必须停止并回到计划修订，不得由 Merger 猜测语义。

### Parallelism Summary

| 指标 | 值 | 说明 |
| --- | --- | --- |
| Total Units | 11 | U1–U11 |
| DAG Depth | 4 | P0 → P1 → P2 → P3 |
| Critical Path | U3 → U4 → U9 → U11 | 真实字段、gate 基础、merge 授权、最终动态验证 |
| Initial Ready Set | U1,U2,U3,U10 | 全部从 e94c0782 独立启动 |
| Max Planned Concurrency | 5 | U5–U9 可同时执行；资源不足时只队列化验证，不改变 DAG |
| Serial Edges | 8 | 全部见 Serial Edge Ledger |
| Avoidable Serialization | 0 | 已移除旧 U1→…→U11 人工排序及 U5→U6 假依赖 |
| Global Barrier Count | 0 | U11 是真实 Fan-In，不是无关 Unit 的 Wave Barrier |

### ASAP Release

* U3 Close 后立即启动 U4；不等待 U1、U2、U10。
* U2 Close 后，若 U4 已通过则立即启动 U8；否则 U8 只等待 U4，不等待 U5/U6/U7/U9。
* U4 Close 后立即同时启动 U5/U6/U7/U9；U8 在 U2 也已关闭时同时启动。
* 任一 U5–U10 Close 后只更新 U11 的依赖计数；U11 必须等全部直接前驱通过，不能提前消费部分最终 schema。

---

## 9. 执行命令清单

| 时机 | 命令 | 目的 | 预期 | 失败能否下一步 |
| --- | --- | --- | --- | --- |
| 每 Unit Red | `cargo nextest run -p ralph-core --test scenarios -- <substring>` | 当前 BDD | 因缺能力失败 | 否 |
| 每 Unit 结构 | `cargo nextest run -p ralph-cli --bin ralph -- parallel_forge` | parse/lint 结构 | 绿 | 否 |
| 每 Unit preset | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 与 `-p ralph-core -- preset_lint` | lint | 绿 | 否 |
| 每 Unit 后 | `ralph preset check -H builtin:parallel-forge --strict --format json` | embed 合并 schema | `"passed"` 类无 error | 否 |
| U4 起 prompt | `ralph inspect prompt -H builtin:parallel-forge --hat worktree --format json`（及 reviewer/integrator/auditor/forge-failure-handler） | 确认能看见投影字段 | JSON 含 instructions/skills | 否（缺字段则停） |
| U2–U10 回归 | `cargo nextest run -p ralph-core --test scenarios -- parallel_forge` | 旧 BDD | 全绿 | 否 |
| U11 | `ralph preset verify -H builtin:parallel-forge --scenario presets/scenarios/parallel-forge-success.yml --format json`（及 recovery/blocked/no-output） | 动态 | JSON 字段符合场景 expect | 否 |
| U11 | `scripts/check-cli-doc-drift.sh --strict` | skill/CLI | 0 | 否 |
| 最终 | `./scripts/run-tests.sh` | 全量 | 0 | 否 |
| 最终 | `cargo clippy` / `cargo fmt --check` / `cargo build` | lint/type/build | 0 | 否 |

禁止：`cargo test -p ralph-cli` 无 nextest。禁止 `RALPH_PRECHECK_MODE=off` 来让测试变绿。

---

## 10. 最终质量门禁

* 全部 S1–S22 对应测试绿  
* 矩阵 R1–R9 均有测试  
* `exec.unit.done` 无新 guard  
* two-wave 场景绿  
* 无 `.skip` / `.only` / 削弱断言  
* 无未解释 snapshot  
* 无 BLOCKED 决策  
* 无超范围 Rust DSL  
* 每个 Unit 有完整 TDD 闭环且按自身依赖提交；无编号或 Wave 人工排序
* Initial Ready Set 的 U1/U2/U3/U10 全部从同一基线启动；U4/U5–U9/U11 按 Release Gate ASAP 释放
* Avoidable Serialization = 0；Global Barrier Count = 0；所有保留串行边均在 §8 ledger 中有证据
* 未提交 `.ralph/review/**/{scratch,residuals*,draft}`

---

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
| --- | --- | --- |
| 这是实施计划而不是 Roadmap 吗 | 是 | Unit 指向具体 YAML 键、rule id、测试函数名 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D17 已锁 topic 名、on_exhausted、`.proposed`、fingerprint、DAG 调度 |
| 所有文件和接口是否有代码库证据 | 是 | E1–E19；新增文件均标「计划新增」 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | 最低 D8/D14=0.88 |
| 是否存在未处理的低置信度假设 | 否 | 无 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 契约表征；U2 writer；U3 字段；U4–U9 各一个 topic 门；U10 收据类；U11 验证同步 |
| 每个 Unit 是否可以独立验证 | 是 | 每 Unit 有命令 |
| 每个 Unit 是否有真实 Red | 是 | 缺能力导致事件被接受或下游唤醒 |
| 每个 Unit 是否包含回归范围 | 是 | parallel_forge scenarios + preset_lint |
| 是否存在未来 Unit 依赖 | 否 | 后 Unit 不用未建 API |
| 是否存在泛化任务描述 | 否 | 无「完善逻辑」 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §5–§7 |
| 所有关键决策是否有 Evidence | 是 | 表内 E* |
| Unit 依赖 DAG 是否明确 | 是 | §7/§8 已列 depends_on、ready_when、Release Gate |
| Initial Ready Set 是否完整列出 | 是 | U1、U2、U3、U10 |
| 所有无真实依赖 Unit 是否已最大量并发 | 是 | U1/U2/U3/U10；U5–U9 均无彼此依赖 |
| 依赖解除后后继 Unit 是否 ASAP 启动 | 是 | §8 ASAP Release |
| 每条串行边是否都有 Evidence | 是 | §8 Serial Edge Ledger |
| Avoidable Serialization 是否为 0 | 是 | 旧线性链已移除 |
| 是否不存在不必要的全局 Wave Barrier | 是 | 仅 U11 真实 Fan-In |
| 可并发 Unit 是否有独立 worktree 边界 | 是 | §7 元数据表 |
| 并发 Unit 是否不存在未处理的语义写冲突/共享资源冲突 | 是 | 写集合按 YAML key/测试符号隔离；冲突即停 |
| 稀缺验证资源是否仅限制验证而非整个开发 Unit | 是 | §7 资源列；验证可队列化 |
| 必要的 Fan-In / Merge Gate 是否定义 Integration / Regression | 是 | U11 与最终门禁 |

---

## Executor 硬性禁令（再声明）

1. 不得把 consistency `rule.topic` 写成无 `.proposed` 后缀的双 guard topic。  
2. 不得写 `{non_empty: false}` 或 `{dead_end_confidence gte 90}`。  
3. 不得给 `exec.unit.done` 加 rule/precheck。  
4. 不得用 `plan.blocked` 作为 parallel-forge 耗尽 topic。  
5. 不得通过 `RALPH_PRECHECK_MODE=off`、删断言、改 fixture 去匹配错误拓扑来获得 Green。  
6. 不得在 U4 之前打开其它 topic 的 precheck（U4 第一次设置 `precheck.enabled: true` 时只注册 `forge.worktrees.ready` 一条）。  
7. 不得修改 `build_exhausted_payload` 去填 `plan_path` 等字段。  
8. 不得手写 producer `publishes: [forge.worktrees.ready.proposed]`——desugar 会改写 accepted 名。
