---
title: "fix: 坐稳 parallel-forge 显式 flow authority"
date: 2026-07-28
type: fix
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan
execution: code
---

# fix: 坐稳 parallel-forge 显式 flow authority

## 0. 计划状态

- **状态：READY。** 所有实施关键决策置信度均不低于 0.85；不存在未处理的 BLOCKED 决策。
- **代码库基线：** `3dcd37cb859a447823ef36a18aeaaf5a5e8ec535`。
- **工作树基线：**最终复核时仅本计划文件和用户提供的
  `docs/report/2026-07-28-parallel-forge-primary-20260727-162156-diagnosis.md`
  未跟踪；无生产代码改动。调查中曾短暂观察到其他未提交文件，但最终复核时已不存在，故不将其作为可依赖接口或测试入口。
- **工作树保护：**Executor 不得回退、覆盖或顺手提交用户提供的诊断报告；每个 Unit 开始前必须重新确认目标文件没有并行改动。
- **调查范围：**
  - 目标运行 `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph` 的 events、flow authority、recovery、日志和 supervisor 状态；
  - `presets/en/parallel-forge.yml` 的 hats、topic、supervisor 和 `mechanism.flow`；
  - `presets/en/implementation-review.yml` 的显式 transition、现有结构化测试和真实 EventLoop fan-in BDD；
  - `advance_plan_step`、`recover_current_plan_step`、FlowStepScope、preset lint 和 finding ID；
  - 所有 builtin preset 的 flow `kind` 清单；
  - 最近三份 `implementation-review` 并发诊断报告。
- **已执行验证：**
  - `cargo nextest run -p ralph-cli --bin ralph -- test_implementation_review_adopts_generic_mechanism_contract`：1/1 通过；
  - `cargo nextest run -p ralph-core --test scenarios -- implementation_review_wave_runtime`：2/2 通过；
  - `cargo nextest run -p ralph-core -- preset_lint`：258/258 通过；
  - `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`：11/11 通过；
  - `cargo nextest run -p ralph-cli --bin ralph -- presets`：56/56 通过。
- **尚未执行验证：**
  - 未运行当前工作树全量 `./scripts/run-tests.sh`；
  - 未对目标修复运行 Red/Green，因为本计划不编写生产代码；
  - 未重新执行外部 live LLM run，最终验收采用确定性 runtime/BDD，live run 只作人工补充证据。
- **阻塞项：**无。若执行时并行改动已改变 `advance_plan_step`、parallel-forge flow、implementation-review flow 或 supervisor fan-in 契约，必须按各 Unit 停止条件重新调查，不能照旧计划强行编码。

---

## 1. 功能目标

### 1.1 业务目标

让以下命令使用真实 `builtin:parallel-forge` 时，不再在 inspector 之后因 flow step 提前推进而拒绝 planner：

```bash
ralph run -H builtin:parallel-forge -p docs/plans/2026-07-22-001-feat-multi-sort-supervisor-e2e-plan.md
```

完整目标是让 preset 声明的每个跨 hat 交接都有显式 transition authority，使规划、supervisor exec wave、review、integration、verification、audit、report 和终态按 topic 驱动，而不是依赖“任一允许 topic 都走到下一个数组元素”的位置回退。

### 1.2 用户或调用方

- 直接运行 `ralph run -H builtin:parallel-forge -p <plan>` 的 operator；
- emit `forge.*` / `exec.*` / `work.failed` 的 parallel-forge hats；
- 在 live ingestion、restart replay 和 CLI policy-check 中共享 flow authority 的 runtime；
- preset 作者和 reviewer，他们需要在启动前识别同类完全位置化的多 topic linear flow。

### 1.3 当前行为

- `planning` 同时允许 `forge.plan.inspected`、`forge.plan.ready`、`forge.concurrency.approved`、`forge.worktrees.ready` 和 `forge.plan.blocked`。
- `forge.plan.inspected` 被接受后，`advance_plan_step` 找不到显式 `on` / `on_any_of` target，于是使用 legacy positional fallback，把当前 step 从 `planning` 提前推进到 `exec_wave`。
- planner 随后 emit `forge.plan.ready`，但 `exec_wave.allowed_emits` 不包含该 topic，因此 FlowStepScope 拒绝它。
- supervisor 数据库 0 wave 是上述拒绝的下游结果，不是首个故障。
- `integration` 也把五个顺序 handoff 和 `work.failed` 塞在一个 linear step；即使规划链修好，后续仍会在第一个成功 topic 后错误位置推进。

### 1.4 目标行为

- 每个跨 hat 成功 handoff 使用下一 step 的 `"on"`；
- 多来源 block 使用 `report.on_any_of` 汇合；
- `exec.wave.complete` 和 `exec.wave.failed` 分别进入 `exec_finalize` 和 `exec_failure`；
- `exec.unit.ready`、`exec.unit.done`、`exec.unit.failed` 保持在 `exec_wave`，不推进；
- `work.failed` 继续遵循现有 runtime 的 non-transition 语义，不修改通用机制；
- failure handler 或 integration/verifier/tester emit `work.failed` 后，reporter 的 `forge.report.done` 从当前失败 step 直接进入 `plan_end`，随后 `LOOP_COMPLETE` 被接受；
- live flow、`recover_current_plan_step` replay 和 CLI policy-check 使用同一个既有 authority，得到相同 step；
- 新 lint 在 strict preset check 阶段拒绝“非末尾、多个 allowed emits、完全没有显式 forward target 的 `kind: linear` step”；
- `implementation-review`、`ce-executor-supervisor` 和其他 builtin presets 的配置与现有行为不变。

### 1.5 行为差异

| 输入事件 | 当前 step | 当前结果 | 目标结果 |
|---|---|---|---|
| `forge.plan.inspected` | `planning` | 错进 `exec_wave` | 进入 `plan_authoring` |
| `forge.plan.ready` | `plan_authoring` | 当前无该 step | 进入 `concurrency_review` |
| `forge.worktrees.ready` | `worktree_setup` | 当前无该 step | 进入 `exec_wave` |
| `exec.unit.done` | `exec_wave` | 留在 `exec_wave` | 保持不变 |
| `exec.wave.complete` | `exec_wave` | 位置进入 `unit_review` | 进入 `exec_finalize` |
| `forge.exec.development.done` | `exec_finalize` | 当前无该 step | 进入 `unit_review` |
| `exec.wave.failed` | `exec_wave` | 位置进入 `unit_review` | 进入 `exec_failure` |
| `work.failed` | failure-capable step | 不推进 | 保持不变 |
| `forge.report.done` | report/failure-capable step | 依位置偶然推进 | 显式进入 `plan_end` |
| `LOOP_COMPLETE` | `plan_end` | 接受 | 保持接受 |

### 1.6 本次范围

- 修改 `parallel-forge` 的 `mechanism.flow.steps`，不修改 hats、triggers、publishes、event schemas、supervisor 参数或 agent instructions。
- 为 embedded `parallel-forge` 添加结构化 transition/recovery 测试。
- 为完整成功和失败收敛添加真实 EventLoop BDD。
- 新增窄范围 preset lint 和稳定 finding ID。
- 更新 preset operator finding rubric/pattern，只说明通用 authoring 规则。
- 执行所有 builtin preset、implementation-review 和全 workspace 回归。

### 1.7 非目标

- 不修改 `advance_plan_step` 的 positional fallback 或 `NON_TRANSITION_TOPICS`。
- 不修改 supervisor store/coordinator/dispatcher/worker、wave fan-in、delivery state 或 channel routing。
- 不修复最新 `implementation-review` 报告中的 fix-planner 沉默、长时间挂起或 agent 输出可观测性。
- 不修改 `implementation-review.yml`、其 schema、并发数、timeout 或 hat instructions。
- 不迁移 working presets 到新 flow 格式。
- 不新增 CLI 参数、config 字段、event topic、数据库迁移、feature flag、依赖或兼容层。
- 不用 live 模型测试替代确定性验收。

### 1.8 输入、输出和状态

- **输入：**`forge.start`、各 hat 业务事件、supervisor 注入的 `exec.wave.complete` / `exec.wave.failed`、operator plan path。
- **输出：**接受的主账本事件、`.ralph/flow-authority.jsonl` 当前 step、supervisor wave 终态、report artifact 和唯一 `LOOP_COMPLETE`。
- **状态变化：**current plan step 按目标 flow 表单向前进；unit terminal 和 `work.failed` 不推进；重复旧 handoff 不得 retrograde。
- **副作用：**仅 parallel-forge flow authority 的 step 粒度改变；topics、payload、artifact 路径和 supervisor DB schema 不变。

### 1.9 错误语义

- 当前 step 未允许的 topic 继续由 FlowStepScope fail-close，不放宽。
- `forge.plan.blocked` 从任一规划/review/audit step 显式跳到 `report`。
- `exec.wave.failed` 进入 `exec_failure`，等待 failure handler 产生 `work.failed`；不得伪装为 success。
- `work.failed` 保持当前 step，等待 reporter；不得修改为通用 transition。
- lint finding 是结构错误，strict builtin lint 必须失败并给出增加 `"on"` / `on_any_of` 的 action hint。

### 1.10 兼容、性能、安全

- **兼容要求：**不要求旧格式兼容迁移，但本次主动保护所有现存 working preset；generic positional fallback 完全不变。
- **性能要求：**flow transition 查找复杂度不变；lint 仅遍历 preset steps，不能引入运行时开销；BDD 不使用无界等待。
- **安全/权限：**无权限模型变化；测试不访问网络、不读取用户凭证；测试 workspace 使用临时目录。

### 1.11 已知约束

- `"on"` 表示进入该 step 的 accepted topic，不是离开当前 step 的字段。
- declared target 只向前搜索，必须按拓扑顺序排列 steps。
- `NON_TRANSITION_TOPICS` 在显式 target 搜索之前返回；`work.failed` 和 unit topics不能靠 `"on"` 推进。
- reporter 在同一 activation 依次 publish `forge.report.done` 和 `LOOP_COMPLETE`；flow 必须先接受前者进入 `plan_end`，再接受后者。
- preset YAML 改动必须检查 schema 并跑三组指定 preset 校验。

### 1.12 已确认假设

- H1：实际目标 run 使用 parallel-forge hats 和 7200 秒 supervisor timeout；高置信度，见 E1-E4。
- H2：`forge.plan.inspected` 的 accepted snapshot 把 step 写成 `exec_wave`；高置信度，见 E2。
- H3：generic runtime 已支持显式 `on` / `on_any_of`，implementation-review 正在使用且测试为绿；高置信度，见 E7-E10。
- H4：只有 `parallel-forge` 使用 `kind: linear`；高置信度，见 E15。
- H5：本次不需要 schema 字段变化；高置信度，见 E13-E14。

### 1.13 待验证假设

没有影响实施方向的未确认假设。以下只作为执行期再确认，不影响 READY：

| 假设 | 为什么确认 | 验证方法 | 预期证据 | 失败影响 |
|---|---|---|---|---|
| 执行时工作树仍与本计划无冲突 | 工作树可能并行变化 | Unit 1 前运行 `git status --short` 和定向 diff | parallel-forge/event-loop flow 未被他人改动 | 停止并重做影响分析 |
| 新增 BDD 可复用现有 supervisor fan-in harness | 避免新测试基础设施 | 阅读 `run_bdd_supervisor_fan_in` 和既有 exec fixture | `exec.unit.done` 可驱动真实 `exec.wave.complete` | 若不成立，改用既有 lower-level EventLoop seam；不得新建第二套 fan-in |

---

## 2. 代码库现状与证据

### 2.1 当前实现入口

#### 外部入口

`ralph run -H builtin:parallel-forge -p <plan>` 由 CLI 解析 embedded preset；`presets/manifest.yml`、`crates/ralph-cli/src/presets.rs` 和 build embedding 保持 preset identity。

#### 调用链

```text
CLI builtin preset
→ RalphConfig::parse_yaml
→ mechanism.flow typed config
→ EventLoop 接受事件
→ FlowStepScopeStage 校验 current step 的 allowed_emits
→ advance_plan_step 计算下一 step
→ flow-authority.jsonl 持久化 accepted authority
→ restart / CLI policy-check 读取 authority 或 recover_current_plan_step
```

#### 核心模块

- `presets/en/parallel-forge.yml`：目标拓扑和 flow 声明。
- `crates/ralph-core/src/event_loop/mod.rs::advance_plan_step`：显式 forward target 优先，legacy positional fallback 兜底。
- `crates/ralph-core/src/event_loop/mod.rs::recover_current_plan_step`：用同一函数折叠 accepted topics。
- `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs`：emit-time step scope。
- `crates/ralph-core/src/preset_lint/flow_declaration.rs`：flow 声明静态规则。
- `crates/ralph-cli/src/presets.rs`：embedded preset 结构化契约测试。
- `crates/ralph-core/tests/scenarios.rs`：真实 EventLoop BDD runner。

#### 数据和外部边界

- `.ralph/flow-authority.jsonl` 是 accepted step snapshot。
- `.ralph/supervisor.db` 是 supervisor wave ledger；本次不修改其 schema或状态机。
- Agent/backend 是外部依赖；验收用 `MockBackend` / supervisor in-memory seam，不使用 live API。

#### 现有测试

- `presets::tests::test_implementation_review_adopts_generic_mechanism_contract` 已固定 implementation-review 分支。
- `test_implementation_review_wave_runtime_fan_in` 和 failed variant 使用真实 EventLoop + real Supervisor fan-in。
- `event_loop/mod.rs` 已有 declared-transition idempotency、branch 和 recovery 单测。
- `preset_lint::flow_declaration::tests` 是新 lint 的既有落点。
- `test_all_embedded_presets_pass_strict_lint` 和 `u6_all_builtin_presets_pass_lint_gate` 覆盖全部 builtin。

#### 构建和验证

- Targeted：`cargo nextest run ...`。
- Preset parity：core lint、CLI lint、CLI presets 三组。
- Rust gate：`cargo fmt --check`、`cargo clippy`、`cargo check --workspace`、`cargo build`。
- 最终：`./scripts/run-tests.sh`。

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | 用户给出的真实命令 | outer command 仅指定 `builtin:parallel-forge` 和 plan | 不按诊断报告虚构的 `-c ralph.supervisor.yml` 修复 | 高 |
| E2 | `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/flow-authority.jsonl` | 唯一记录为 `step=exec_wave, topic=forge.plan.inspected` | 首个故障是 accepted event 后错误 step advance | 高 |
| E3 | `/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/events-20260727-162156.jsonl` | `forge.start`、`forge.plan.inspected` 被接受；之后无 `forge.plan.ready`；末尾 reporter 非法收尾 | planner handoff 被 gate 阻断，supervisor 未开始 | 高 |
| E4 | 目标 run 日志 | hats source 是 `builtin:parallel-forge`；supervisor timeout 是 7200，与 parallel-forge 一致 | 原报告的 ce-executor-supervisor effective preset 判断不成立 | 高 |
| E5 | 目标 run `recovery.jsonl` | `forge.plan.ready` 进入 repair stream | 拒绝点位于 planner handoff，不是 planner artifact 缺失 | 高 |
| E6 | `presets/en/parallel-forge.yml::mechanism.flow` | `planning` 和 `integration` 把多个顺序 topic 塞在单一 linear step，未声明 transition | 必须拆成单 handoff steps；只改首段不够 | 高 |
| E7 | `crates/ralph-core/src/event_loop/mod.rs::advance_plan_step` | 显式 forward target 优先；无 target 时 positional fallback | 直接使用现有能力，不改通用语义 | 高 |
| E8 | 同上 `NON_TRANSITION_TOPICS` | `work.failed`、exec/review/fix unit topics在显式 target 搜索前 no-op | failure 收敛必须让 reporter 的 `forge.report.done` 推进 | 高 |
| E9 | `crates/ralph-core/src/event_loop/mod.rs` 现有 U6/U7 tests | 已覆盖 `on`、`on_any_of`、forward-only、recovery | 不需新增 runtime API | 高 |
| E10 | `presets/en/implementation-review.yml::mechanism.flow` | 使用 `scope_freeze → review_wave → synth_await → fix_plan → finalize` 显式分支 | 可作为并发 preset 的安全参考，不修改它 | 高 |
| E11 | 已执行 implementation-review tests | 结构化测试 1/1、真实 fan-in BDD 2/2 通过 | 建立修复前 non-regression baseline | 高 |
| E12 | 三份 implementation-review 报告 | 历史问题集中于 delivery state、私有/主账本分裂、共享输入写竞争；最新 run 已通过 fan-in 进入 `fix_plan` | 借鉴单一 authority、只读输入、显式失败收敛；不扩大到 supervisor 修复 | 中高 |
| E13 | `presets/schemas/parallel-forge.yml` | 目标 topics 和 required fields 已存在 | flow step 拆分不要求 schema 字段变化 | 高 |
| E14 | parallel-forge hats `triggers/publishes` | inspector→planner→guardian→worktree→dispatcher→reviewer→integrator→verifier→tester→auditor→reporter 的 handoff 已完整 | 只需让 flow authority与既有 hat 图一致 | 高 |
| E15 | 对 `presets/en/*.yml` 的 `kind: linear` 清单 | 只有 parallel-forge 有 linear steps | 窄 lint 不会强迫其他 preset 迁移 | 高 |
| E16 | `crates/ralph-core/src/preset_lint/flow_declaration.rs` | 已集中处理 flow finding 和 action hint | 新规则应加入现有模块，不建第二个 lint 层 | 高 |
| E17 | `crates/ralph-core/src/preset_lint/finding_id.rs` + `ALL_FINDING_IDS` | finding ID 是公开稳定契约 | 新 lint 必须有稳定 ID 和 lock coverage | 高 |
| E18 | `skills/ralph-preset-common/references/finding-rubric.md` | 已列 flow finding ID | 新 finding 必须同步 operator rubric | 高 |
| E19 | 已执行全 preset baselines | core lint 258/258、CLI lint 11/11、CLI preset 56/56 通过 | 后续任何其他 preset finding 都是回归 | 高 |
| E20 | 最终 `git status --short` | 无生产代码改动；仅本计划和用户诊断报告未跟踪 | 当前计划不依赖未提交生产实现；诊断报告不纳入修复 diff | 高 |

### 2.3 受影响范围

#### 生产模块

- `presets/en/parallel-forge.yml`：修改 flow steps。
- `crates/ralph-core/src/preset_lint/flow_declaration.rs`：增加静态规则。
- `crates/ralph-core/src/preset_lint/finding_id.rs`：增加 finding ID。
- `crates/ralph-core/src/preset_lint/mod.rs`：若公开 re-export 规则要求，加入新 ID re-export。

#### 测试模块

- `crates/ralph-cli/src/presets.rs`：实际 embedded parallel-forge transition/recovery 契约。
- `crates/ralph-core/src/preset_lint/flow_declaration/tests.rs`：lint Red/Green 和 legacy non-regression。
- `crates/ralph-core/tests/scenarios.rs`：注册真实 EventLoop BDD。
- 计划新增 `crates/ralph-core/tests/scenarios/parallel_forge_declared_flow_runtime.yml`。
- 计划新增 `crates/ralph-core/tests/scenarios/parallel_forge_declared_flow_failed_runtime.yml`。

#### 文档/skill

- `skills/ralph-preset-common/references/finding-rubric.md`：新增 finding 映射。
- `skills/ralph-preset-common/references/patterns.md`：增加通用“multi-topic linear step 必须显式 handoff”的 authoring pattern；不得写本 plan ID 或事故路径。

#### 已确认不变

- **配置字段：**无新增/删除。
- **schema：**`presets/schemas/parallel-forge.yml` 只审计，不修改。
- **数据/数据库：**无迁移。
- **API/UI：**不涉及。
- **CLI：**命令和参数不变。
- **外部服务：**无变更。
- **其他 preset：**生产 YAML 不改。
- **构建目标：**workspace 不变。

---

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | 在哪里修首个 flow drift | A 修改 `advance_plan_step`；B 扩大 non-transition 白名单；C 拆 parallel-forge flow | C：只拆 preset flow并声明显式 target | E2、E6-E10 | A/B 会改变所有 preset 的通用语义并破坏 working flows | 0.99 |
| D2 | 是否保留 positional fallback | A 删除；B 仅对 parallel-forge 禁用；C 完全保留 | C | E7、E9、E15、用户范围确认 | 删除/条件化都会给其他 preset 带来不必要迁移和风险 | 0.98 |
| D3 | `work.failed` 如何进入终态 | A 从 NON_TRANSITION 移除；B 声明 `on: work.failed`；C 保持当前 step，允许 reporter 的 `forge.report.done` 显式进 `plan_end` | C | E8、E14 | A 改通用机制；B 在当前代码中不会生效且会制造假契约 | 0.96 |
| D4 | exec wave 成功/失败如何分支 | A 都位置进入 reviewer；B success/failure 显式进入不同 step | B：`exec_finalize` / `exec_failure` | E7-E9、E14 | A 重现当前错误；分步可验证 dispatcher 与 failure handler 的不同输出 | 0.97 |
| D5 | lint 覆盖多大范围 | A 所有 flow 必须显式；B 所有 multi-topic step；C 仅 non-final `kind: linear` 且完全没有 forward target | C | E15-E19 | A/B 会强迫 working presets 迁移；C 精确拦截本次结构 | 0.94 |
| D6 | 是否修改 implementation-review | A 同步重构；B 只复用经验并回归 | B | E10-E12、E19、用户硬约束 | 当前 preset 已有显式 flow 和通过的 BDD，改它没有问题依据 | 0.99 |
| D7 | 测试层级 | A 只测 YAML 文本；B 只跑 live E2E；C embedded 结构化 + recovery unit + 真实 EventLoop BDD + preset/full regression | C | E9、E11、E16-E19 | A 违反 preset 测试规则；B 不确定且成本高，不能稳定证明 transition | 0.96 |
| D8 | 是否修改 schema/docs/agent skills | A 全部同步改；B 按行为变化精确审计 | B：schema/agent CLI docs 不改，operator rubric/pattern 更新 | E13、E18 | topic/field/command 均不变，盲改会制造 drift | 0.96 |
| D9 | 目标 step 命名和顺序 | A 保留粗粒度 5 step；B 一 handoff 一 step并保持 forward-only 顺序 | B | E6-E9、E14 | 粗粒度无法表达实际 hat chain；乱序 target 不会被 forward search 命中 | 0.97 |

所有决策均达到阈值，无需计划前 spike。

### 3.1 目标 flow 契约

下表是 Executor 的确定实现目标，不得临场改名或改变顺序：

| 顺序 | Step ID | kind | 进入条件 | allowed emits | 推进语义 |
|---:|---|---|---|---|---|
| 1 | `planning` | `linear` | initial | `forge.plan.inspected`, `forge.plan.blocked` | inspected→2；blocked→13 |
| 2 | `plan_authoring` | `linear` | `forge.plan.inspected` | `forge.plan.ready`, `forge.plan.blocked` | ready→3；blocked→13 |
| 3 | `concurrency_review` | `linear` | `forge.plan.ready` | `forge.concurrency.approved`, `forge.plan.blocked` | approved→4；blocked→13 |
| 4 | `worktree_setup` | `linear` | `forge.concurrency.approved` | `forge.worktrees.ready`, `forge.plan.blocked` | ready→5；blocked→13 |
| 5 | `exec_wave` | `side_effect` | `forge.worktrees.ready` | `exec.unit.ready`, `exec.unit.done`, `exec.unit.failed`, `exec.wave.complete`, `exec.wave.failed` | unit topics stay；complete→6；failed→7 |
| 6 | `exec_finalize` | `await` | `exec.wave.complete` | `forge.exec.development.done` | development.done→8 |
| 7 | `exec_failure` | `await` | `exec.wave.failed` | `work.failed`, `forge.report.done` | work.failed stay；report.done→14 |
| 8 | `unit_review` | `linear` | `forge.exec.development.done` | `forge.units.reviewed`, `forge.plan.blocked` | reviewed→9；blocked→13 |
| 9 | `integration` | `linear` | `forge.units.reviewed` | `forge.integration.done`, `work.failed`, `forge.report.done` | success→10；work.failed stay；report.done→14 |
| 10 | `incremental_verify` | `linear` | `forge.integration.done` | `forge.incremental.verified`, `work.failed`, `forge.report.done` | success→11；work.failed stay；report.done→14 |
| 11 | `full_verify` | `linear` | `forge.incremental.verified` | `forge.full.verified`, `work.failed`, `forge.report.done` | success→12；work.failed stay；report.done→14 |
| 12 | `audit` | `linear` | `forge.full.verified` | `forge.audit.done`, `forge.plan.blocked` | audit.done/blocked→13 |
| 13 | `report` | `await` | `on_any_of: [forge.audit.done, forge.plan.blocked]` | `forge.report.done` | report.done→14 |
| 14 | `plan_end` | `terminal` | `forge.report.done` | `LOOP_COMPLETE` | terminal |

注意：

- `plan_end` 不再重复允许 `forge.report.done`；该 topic 是进入 `plan_end` 的 transition，由前一 step 接受。
- `exec_wave` 不再允许 `forge.exec.development.done`；该 topic 只能在 `exec_finalize` 接受。
- failure-capable steps 必须同时允许 `work.failed` 和 `forge.report.done`，否则 reporter 在 `work.failed` 后会被 FlowStepScope 拒绝。
- `report` 不以 `work.failed` 为 `on_any_of`，因为 runtime 在搜索显式 target 前已把 `work.failed` 判为 non-transition。

---

## 4. BDD 行为规格

```gherkin
Feature: parallel-forge 使用显式 flow authority 完成 plan-driven supervisor 编排

  Background:
    Given 使用当前 embedded builtin:parallel-forge
    And EventLoop 处于 isolated supervisor 模式
    And generic positional fallback 与 NON_TRANSITION_TOPICS 保持原样

  Scenario S1: 规划成功 handoff 逐步推进
    Given current step 是 planning
    When inspector、planner、guardian、worktree 依次产生成功 topic
    Then authority 依次为 plan_authoring、concurrency_review、worktree_setup、exec_wave
    And 每个 topic 在对应 current step 被允许

  Scenario S2: 任一规划或 review 审批阻断进入报告终态
    Given current step 是 planning、plan_authoring、concurrency_review、worktree_setup、unit_review 或 audit
    When 对应 hat emit forge.plan.blocked
    Then authority 跳到 report
    And reporter 的 forge.report.done 进入 plan_end
    And LOOP_COMPLETE 被接受

  Scenario S3: exec unit 并发事件不提前关闭 wave
    Given current step 是 exec_wave
    When 多个 exec.unit.ready、exec.unit.done 或 exec.unit.failed 被接受
    Then current step 仍是 exec_wave
    And 只有 exec.wave.complete 才进入 exec_finalize

  Scenario S4: exec wave 成功和失败走不同收敛路径
    Given current step 是 exec_wave
    When runtime 注入 exec.wave.complete
    Then authority 进入 exec_finalize
    And forge.exec.development.done 才进入 unit_review
    When runtime 改为注入 exec.wave.failed
    Then authority 进入 exec_failure
    And work.failed 不推进
    And forge.report.done 进入 plan_end

  Scenario S5: post-exec 成功链逐步进入唯一终态
    Given reviewer 已 emit forge.units.reviewed
    When integrator、verifier、tester、auditor 和 reporter 依次成功
    Then authority 依次为 integration、incremental_verify、full_verify、audit、report、plan_end
    And 只在 plan_end 接受 LOOP_COMPLETE

  Scenario S6: integration、incremental verify 或 full verify 失败后安全收尾
    Given current step 是任一 failure-capable post-exec step
    When 当前 hat emit work.failed
    Then current step 保持不变
    And reporter 被现有 trigger 激活
    And forge.report.done 进入 plan_end
    And 不出现后续 success handoff

  Scenario S7: replay 不产生 retrograde 或额外 advance
    Given accepted topics 已将 authority 推进到后续 step
    When recover_current_plan_step 重放相同 handoff 或旧 handoff
    Then recovered step 与 live step 相同
    And authority 不退回早期 step

  Scenario S8: lint 拒绝完全位置化的多 topic linear step
    Given 一个非末尾 kind: linear step 有至少两个 allowed emits
    And 后续 steps 没有任何 on 或 on_any_of 引用这些 topics
    When strict preset lint 运行
    Then 返回 preset.flow_linear_positional_ambiguity
    And action hint 要求增加显式 forward target

  Scenario S9: working presets 保持不变
    Given implementation-review、ce-executor-supervisor 和所有其他 builtin presets 当前严格 lint 为绿
    When parallel-forge 修复和新 lint 落地
    Then implementation-review 的 success/failure fan-in BDD 仍通过
    And implementation-review 的 flow recovery 结果不变
    And 所有 builtin presets 仍通过 strict lint
```

---

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐层级 | 风险补充测试 | 是否需要 E2E |
|---|---|---|---|---|---|
| S1 | 实际 embedded config 的四个 handoff step 精确匹配 | `crates/ralph-cli/src/presets.rs` 计划新增测试 | 结构化集成 | recovery fold | 否 |
| S2 | blocked 从所有合法来源跳 report，report.done→plan_end→terminal | embedded config test + failed runtime fixture | 集成/BDD | branch table test | 是，真实 EventLoop BDD |
| S3 | unit topics后 step 仍为 exec_wave，complete 恰好推进一次 | event_loop recovery test + supervisor exec BDD | 单元+集成 | concurrency/fan-in 使用既有 harness | 是，真实 EventLoop BDD |
| S4 | complete/failed 分流；work.failed stays；失败终态无 success topics | embedded config test + failed runtime fixture | 集成/BDD | Fault Injection：forced failed fan-in | 是 |
| S5 | 全 success topic 和 authority 顺序精确；唯一 LOOP_COMPLETE | success runtime fixture | BDD | event topic count | 是 |
| S6 | 三个 failure-capable step 均能让 reporter 收尾，后续 success absent | table-driven recovery + failed runtime fixture | 单元+BDD | Fault Injection | 是 |
| S7 | live/recovery 等价，duplicate/old topic no retrograde | `recover_current_plan_step` tests | 单元 | Idempotency/replay | 否 |
| S8 | 精确 finding ID、severity、hat/step、action hint；legacy presets无 finding | flow lint tests | 单元 | Differential：全部 builtin 前后 lint | 否 |
| S9 | implementation-review 现有结构、fan-in和全部 builtin通过 | 现有测试命令 | 回归 | Characterization + full suite | 否 |

### 5.1 具体断言

- 结构化测试不得搜索 prompt 文案或比较完整 preset bytes。
- BDD 必须使用 `run_workflow_guard_scenario`，不得使用只检查 iteration 的 `run_scenario`。
- success BDD 断言每个 handoff topic存在、`exec.wave.complete` 恰好一次、`LOOP_COMPLETE` 恰好一次、失败 topics 不存在。
- failed BDD 断言 `exec.wave.failed` / `work.failed` / `forge.report.done` / `LOOP_COMPLETE` 存在，且 `forge.exec.development.done`、`forge.units.reviewed` 和后续 success topics 不存在。
- 所有 flow recovery test 同时断言中间 step，不能只断言最终 `plan_end`，防止错误位置推进“碰巧到达同一终点”。
- implementation-review fixture、preset YAML 和 schema不得因本计划更新 snapshot/golden。

### 5.2 测试层级理由

- Transition 是纯状态规则，优先用现有 `recover_current_plan_step` 做低成本精确测试。
- Embedded preset 是否正确接线属于模块协作，放在 `ralph-cli/src/presets.rs`。
- Supervisor coordination topic 必须来自真实 runtime seam，所以代表性 success/failed 路径使用真实 EventLoop BDD。
- Live LLM 不是确定性验收；可在全部 gate 通过后人工重跑原命令，但不作为 Green 的必要条件。

---

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence | Unit |
|---|---|---|---|---|---|---|---|---|
| R1 | 规划 handoff 不提前进入 exec_wave | S1 | embedded planning sequence | recovery intermediate states | CLI preset structured test | 无 | E2,E6-E9 | U1 |
| R2 | planning/review block 显式进入 report | S2 | blocked branch table | recover branch cases | failed runtime BDD | real EventLoop | E7-E10,E14 | U1/U3 |
| R3 | exec unit topics stay in wave | S3 | unit topic no-advance | advance/recover tests | supervisor fan-in BDD | real EventLoop | E8-E9,E12 | U2 |
| R4 | exec complete/failed 分流 | S4 | success/failure intermediate steps | recovery branch cases | failed fan-in BDD | real EventLoop | E7-E9,E14 | U2 |
| R5 | post-exec success逐步推进 | S5 | full success fixture | recovery step table | runtime BDD | real EventLoop | E6-E9,E14 | U3 |
| R6 | work.failed 安全收尾 | S6 | failure table + absent topics | non-transition assertions | failed runtime BDD | real EventLoop | E8,E14 | U3 |
| R7 | replay/live authority一致 | S7 | duplicate/old topic fold | recover tests | embedded config test | 无 | E2,E7,E9 | U1-U3 |
| R8 | exact anti-pattern 启动前被 lint | S8 | synthetic invalid preset | flow lint tests | all builtin strict lint | 无 | E15-E19 | U4 |
| R9 | implementation-review 和其他 preset 零回归 | S9 | existing characterization | existing flow tests | existing BDD + preset suite | 无新增 live E2E | E10-E12,E19-E20 | U1-U4 |
| R10 | schema/topic/CLI/DB 不变 | S9 | parity + drift audit | N/A | preset parity/full suite | 无 | E13-E14 | U4 |

---

## 7. 严格串行开发单元

```text
Unit 1：规划与阻断 handoff
  ↓ 完成全部测试、重构和回归
Unit 2：exec wave 成功/失败收敛
  ↓ 完成全部测试、重构和回归
Unit 3：post-exec 成功/失败终态
  ↓ 完成全部测试、重构和回归
Unit 4：窄 lint 与全 preset 发布门禁
```

以下 Unit Close 清单对 **每一个** Unit 强制生效，并与各 Unit 的专属完成标准取并集；任一项不满足，不得进入下一 Unit：

- 当前 Scenario 的 acceptance test通过；
- 当前 Unit 的所有单元测试和集成测试通过；
- 当前 Unit列出的直接、相邻和公开调用方回归通过；
- `cargo fmt --check`、本 Unit受影响 crate的 typecheck/build、`cargo clippy`通过；
- 没有新增 skip/ignore/only，没有删除或削弱断言；
- 没有提前实现未来 Unit，没有无关重构或新依赖；
- 实际 Red已记录，且失败原因是目标能力缺失；
- Evidence Ledger已用实现中发现的新事实更新；
- 相关 Decision置信度重新核对且仍不低于0.85；
- diff只包含本 Unit预期文件，用户诊断报告未被纳入；
- 当前 Unit可以独立提交。

### Unit 1：规划与阻断 handoff 使用显式 authority

#### 1. Unit 目标

operator 的 run 在接受 `forge.plan.inspected` 后进入 `plan_authoring`，并能沿 planner→guardian→worktree 逐步到达 `exec_wave`；任一 `forge.plan.blocked` 可进入 report 终态。

#### 2. 对应需求与 Scenario

- Requirement：R1、R2、R7、R9。
- Scenario：S1、S2、S7、S9。
- Decision：D1、D2、D6、D9。
- Evidence：E1-E11、E14、E19-E20。

#### 3. 外部可观察结果

- 原命令不再在 planner emit 前把 authority 写成 `exec_wave`。
- flow authority 中间状态按 `plan_authoring → concurrency_review → worktree_setup → exec_wave` 出现。
- planning/review block 可由 reporter形成终态，而不是继续落入错误业务 step。

#### 4. 当前行为基线

- E2 已证明 `forge.plan.inspected` 后错误 step 是 `exec_wave`。
- E11/E19 已固定其他 preset 当前 Green。
- 旧行为已有运行证据但无自动测试；本 Unit 首先新增 actual embedded preset 的 Red。

#### 5. 输入与输出

- 输入：`forge.plan.inspected`、`forge.plan.ready`、`forge.concurrency.approved`、`forge.worktrees.ready`、`forge.plan.blocked`。
- 输出：精确 current step 或 report/plan_end/terminal。
- 错误：非法 topic仍被拒绝。
- 状态变化：只向前。
- 副作用：只改变 parallel-forge flow declaration。
- 不变量：runtime、topics、schemas、hats、implementation-review不变。

#### 6. 修改位置

| 位置 | 当前职责 | 修改原因 | 修改边界 | 明确不修改 |
|---|---|---|---|---|
| `presets/en/parallel-forge.yml::mechanism.flow.steps` | 声明 flow scope/transition | 粗 planning step 触发 positional drift | 增加 `plan_authoring`、`concurrency_review`、`worktree_setup`、`report`，声明进入条件 | hats/instructions/supervisor/event schemas |
| `crates/ralph-cli/src/presets.rs` tests | 测 embedded preset 结构语义 | 建立真实 preset Red | 新增 transition/recovery 断言 | prompt 文本和完整 YAML equality |

#### 7. 可依赖能力

- `recover_current_plan_step`。
- `FlowStepConfig.on/on_any_of`。
- 现有 embedded preset loader。
- Unit 开始前已通过的 implementation-review tests。

#### 8. 禁止依赖的未来能力

- 不依赖 U2 exec split、U3 post-exec split、U4 lint。
- 不提前实现 lint。
- 不修改 supervisor或 EventLoop runtime。

#### 9. 验收测试

- 名称：计划新增 `test_parallel_forge_planning_flow_uses_declared_handoffs`。
- 层级：embedded preset 结构化集成。
- 前置：parse `get_preset("parallel-forge")`。
- 输入：按 S1 顺序逐个增加 accepted topic。
- 动作：每一步调用 `recover_current_plan_step`。
- 断言：每个中间 step 精确匹配；`forge.plan.inspected` 绝不能直接到 `exec_wave`。
- 副作用断言：config hats/topics/supervisor值不变。
- 不变量：implementation-review test保持 Green。
- 命令：

```bash
cargo nextest run -p ralph-cli --bin ralph -- test_parallel_forge_planning_flow_uses_declared_handoffs
```

#### 10. Acceptance Red

- 首先添加并运行上述测试。
- 预期实际失败：接受 `forge.plan.inspected` 后 actual=`exec_wave`，expected=`plan_authoring`。
- 这是有效 Red，因为测试已成功 parse 当前 embedded preset并执行真实 recovery authority，失败点正是 E2 的缺失能力。
- 编译失败、fixture 找不到、测试名过滤错误、其他 preset lint 失败都不算有效 Red。

#### 11. 单元测试拆分

1. `parallel_forge_inspection_enters_plan_authoring`：单 topic输入，期望 `plan_authoring`。
2. `parallel_forge_planning_success_reaches_exec_wave_in_four_handoffs`：逐步断言所有中间状态。
3. `parallel_forge_planning_blocked_jumps_to_report`：对四个 planning states 表驱动验证 block。
4. `parallel_forge_report_done_enters_plan_end`：从 report 接受 report.done。
5. `parallel_forge_old_planning_topic_does_not_retrograde`：replay 后保持后续 state。
- Fake/Stub：不需要。
- 不允许 Mock：`recover_current_plan_step` 和实际 embedded config。

#### 12. Red → Green → Refactor 顺序

```text
Test 1 Red（inspected 实际进入 exec_wave）
→ 最小拆出 plan_authoring 并声明 on
→ Test 1 Green
→ Test 2 Red（ready/approved/ready 中间状态缺失）
→ 最小增加 concurrency_review/worktree_setup
→ Test 2 Green
→ Test 3 Red（blocked 未跳 report）
→ 最小增加 report on_any_of 和 plan_end on
→ Test 3 Green
→ Test 4/5 Red
→ 补齐 report.done 与 replay断言
→ 全部 Green
→ 只整理 flow 表顺序和测试 helper
```

#### 13. 最小实现范围

- 必须实现目标 flow 表的 steps 1-5、13-14 及其 planning/block transition。
- 必须保留 exec/post-exec topics，使尚未完成的后续路径仍能被后续 Unit接管。
- 不实现 exec wave split、post-exec split或 lint。
- 不改任何 schema。

#### 14. 集成验证

- 联合真实 embedded loader、typed config、recovery authority。
- 不需要 supervisor启动。
- 运行 Unit测试、implementation-review结构测试、CLI presets subset。
- 预期所有通过。

#### 15. 风险驱动测试

- **Characterization：**先跑 implementation-review结构测试，固定 working flow。
- **Differential：**修改前后比较除 parallel-forge 外所有 embedded preset strict lint结果，必须相同。
- **Idempotency：**replay旧 handoff不得 retrograde。

#### 16. 回归范围

- `test_implementation_review_adopts_generic_mechanism_contract`：同一 transition authority消费者。
- event_loop declared-transition tests：防止错误理解 `"on"`。
- CLI presets tests：embedding、manifest、strict lint。
- 旧配置：其他 preset仍依赖 positional fallback。
- Build/lint/typecheck 在 Unit close 前运行 targeted crate范围。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改现有生产文件 | 拆 planning handoff和 block/report entry | E2,E6-E9,E14 |
| `crates/ralph-cli/src/presets.rs` | 新增测试 | 锁定 actual embedded transition | E9,E19 |

#### 18. 完成标准

- S1/S2 的 Unit范围全部通过。
- Acceptance Red 原因已记录，随后 Green。
- implementation-review结构测试通过。
- core flow authority tests通过。
- CLI preset tests通过。
- `cargo fmt --check`、`cargo check -p ralph-cli`、`cargo clippy` 通过。
- 无 skip/only/断言削弱。
- 用户诊断报告未被覆盖或纳入提交。
- 可独立提交。

#### 19. 停止条件

- `forge.plan.inspected` Red 不是 actual=`exec_wave`；
- parallel-forge 已被他人改为不同 flow；
- 需要修改 runtime才能让 planning tests Green；
- block reporter publishes/trigger 与 E14 不一致；
- implementation-review回归变红；
- 发现 schema字段必须变化。

停止后执行：记录新 Evidence → 更新影响分析 → 重算 D1/D2/D6/D9 → 修订 U1及后续 Unit。

#### 20. 风险与注意事项

| 风险 | 触发条件 | 检测 | 缓解 | 剩余风险 |
|---|---|---|---|---|
| `"on"` 写在错误 step | 测试只看最终结果 | 逐步断言中间 state | 按目标 flow表实现 | 低 |
| block 分支越过 reporter | 直接进 plan_end | 断言 report 中间态 | report on_any_of，plan_end on report.done | 低 |
| 顺手迁移其他 preset | diff出现其他 YAML | `git diff --name-only` | Unit scope硬限制 | 极低 |

### Unit 2：exec wave 成功/失败显式收敛

#### 1. Unit 目标

并发 unit topics保持在 `exec_wave`；runtime coordination complete/failed 分别进入 `exec_finalize`/`exec_failure`，成功到 reviewer，失败到 reporter终态。

#### 2. 对应需求与 Scenario

- Requirement：R3、R4、R7、R9。
- Scenario：S3、S4、S7、S9。
- Decision：D2-D4、D6-D7、D9。
- Evidence：E7-E12、E14、E19-E20。

#### 3. 外部可观察结果

- 第一条 `exec.unit.done` 不关闭 wave。
- `exec.wave.complete` 后 dispatcher可 emit `forge.exec.development.done`。
- `exec.wave.failed` 后 failure handler的 `work.failed` 被接受，reporter可完成终态。

#### 4. 当前行为基线

Unit 1完成后，`exec_wave` 可正确进入，但 current YAML仍会让 `exec.wave.complete/failed` 位置进入旧 `unit_review`；缺少 `exec_finalize/exec_failure`。

#### 5. 输入与输出

- 输入：exec unit topics、wave complete/failed、development.done、work.failed、report.done。
- 输出：对应 intermediate step和终态。
- 错误：agent不得伪造 supervisor-only coordination topic；现有 origin guard不变。
- 不变量：unit topics不推进；work.failed不推进。

#### 6. 修改位置

| 位置 | 当前职责 | 修改原因 | 修改边界 | 不修改 |
|---|---|---|---|---|
| `presets/en/parallel-forge.yml::mechanism.flow.steps` | exec wave scope | success/failure当前同走位置回退 | 增加 `exec_finalize`、`exec_failure`，调整 `unit_review` entry | supervisor/wave runner binding |
| `crates/ralph-cli/src/presets.rs` tests | embedded flow contract | 锁定 branch/intermediate states | 增加 exec sequence table | 其他 preset tests |

#### 7. 可依赖能力

- U1 已验证 planning→exec_wave。
- 现有 NON_TRANSITION unit topics。
- reporter现有 triggers/publishes。

#### 8. 禁止依赖的未来能力

- 不依赖 U3 post-exec chain和 U4 lint。
- 不改 supervisor coordinator/dispatcher/store。
- 不提前拆 integration。

#### 9. 验收测试

- 名称：计划新增 `test_parallel_forge_exec_wave_declares_success_and_failure_handoffs`。
- 前置：通过 U1序列到 `exec_wave`。
- 输入/动作：分别 fold unit topics、complete路径、failed路径。
- 断言：
  - unit topics后仍 `exec_wave`；
  - complete后 `exec_finalize`；
  - development.done后 `unit_review`；
  - failed后 `exec_failure`；
  - work.failed后仍 `exec_failure`；
  - report.done后 `plan_end`。
- 命令：

```bash
cargo nextest run -p ralph-cli --bin ralph -- test_parallel_forge_exec_wave_declares_success_and_failure_handoffs
```

#### 10. Acceptance Red

- Unit 1 Green基线上先添加测试。
- 正确 Red：`exec.wave.complete` actual=`unit_review`，expected=`exec_finalize`；failed同样错误进入 `unit_review`。
- 若 failure来自 supervisor测试基础设施或编译错误，不是有效 Red。

#### 11. 单元测试拆分

1. 三类 unit topic no-advance。
2. complete→exec_finalize。
3. development.done→unit_review。
4. failed→exec_failure。
5. work.failed stays。
6. report.done→plan_end。
7. 重复 wave terminal不越过合法 target。
- 不允许 Mock：actual embedded flow、advance/recovery函数。

#### 12. Red → Green → Refactor 顺序

```text
unit no-advance Characterization Green
→ complete branch Red
→ 增加 exec_finalize
→ Green
→ failed branch Red
→ 增加 exec_failure
→ Green
→ development.done Red
→ 让 unit_review on development.done
→ Green
→ work.failed/report.done Red
→ 最小补齐 exec_failure allowed_emits
→ Green
→ Refactor 测试表
```

#### 13. 最小实现范围

- 只实现目标 flow表 steps 5-8。
- 保留 `runs: supervisor.exec.wave`。
- 不更改 max concurrency/timeout/db path。
- 不更改 event payload或 agent instructions。

#### 14. 集成验证

- 运行 existing supervisor exec fanout BDD，确认真实 `exec.wave.complete` seam仍绿：

```bash
cargo nextest run -p ralph-core --test scenarios -- test_opac_sb1_supervisor_exec_wave_fanout
```

- 运行 implementation-review真实 fan-in BDD，确认 supervisor变更为零。
- Unit 2不要求新增完整 parallel-forge BDD；U3在完整 flow闭合后新增。

#### 15. 风险驱动测试

- **Concurrency：**复用真实 Supervisor fan-in fixture，证明 coordination topic不是 agent mock。
- **Fault Injection：**recovery sequence模拟 `exec.wave.failed`。
- **Differential：**unit topics对 step的结果与修改前一致。

#### 16. 回归范围

- event_loop `exec_wave` non-transition tests。
- ce-executor-supervisor exec fanout scenario。
- implementation-review fan-in scenarios。
- CLI embedded presets。
- 不运行/修改 live API。

#### 17. 预期文件变更

| 位置 | 变更类型 | 变更原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改现有生产文件 | 增加 exec success/failure steps | E7-E9,E14 |
| `crates/ralph-cli/src/presets.rs` | 新增测试 | 锁定 wave分支和 non-transition | E8-E9,E19 |

#### 18. 完成标准

- S3/S4全部断言 Green。
- existing supervisor exec BDD Green。
- implementation-review BDD Green。
- U1全部回归 Green。
- Build/lint/typecheck通过。
- 没有生产 runtime diff。
- 可独立提交。

#### 19. 停止条件

- unit topic实际推进，且与 E8冲突；
- real Supervisor fan-in测试失败；
- 需要修改 coordinator/dispatcher才能完成本 Unit；
- reporter不按 E14消费 `work.failed`；
- 并行改动导致既有 supervisor基线测试先红。

#### 20. 风险与注意事项

| 风险 | 触发 | 检测 | 缓解 | 剩余 |
|---|---|---|---|---|
| 把 `forge.exec.development.done`留在 exec_wave | complete后直接 scope reject | intermediate test | 只允许在 exec_finalize | 低 |
| 用 `on: work.failed`制造假分支 | 测试未断言中间态 | work.failed stays断言 | 按 D3实现 | 极低 |
| 误触 supervisor通用机制 | runtime diff出现 | file scope audit | 本 Unit只改 preset/test | 极低 |

### Unit 3：post-exec 成功/失败形成唯一终态

#### 1. Unit 目标

review、integration、incremental verify、full verify、audit、report按显式 handoff运行；任一真实 `work.failed` 或 `forge.plan.blocked` 都能安全进入唯一终态。

#### 2. 对应需求与 Scenario

- Requirement：R2、R5-R7、R9-R10。
- Scenario：S2、S5-S7、S9。
- Decision：D1-D4、D6-D9。
- Evidence：E6-E14、E19-E20。

#### 3. 外部可观察结果

- 成功 run 不再在 `forge.integration.done` 后错过 verifier。
- 失败 run 不继续执行后续 success hats。
- reporter的两个事件按 `forge.report.done → LOOP_COMPLETE` 闭合。

#### 4. 当前行为基线

Unit 2后仍保留粗 `integration` step；第一个 success emit会位置进入后续 step，无法表达 verifier/tester/auditor handoff。E6已确认该潜在二次故障。

#### 5. 输入与输出

- 输入：reviewed、integration.done、incremental.verified、full.verified、audit.done、plan.blocked、work.failed、report.done、LOOP_COMPLETE。
- 输出：目标 flow表 steps 9-14。
- 状态/副作用：只更新 authority；artifact写入仍由 hats负责。
- 不变量：失败后不接受后续 success topic。

#### 6. 修改位置

| 位置 | 当前职责 | 原因 | 边界 | 不修改 |
|---|---|---|---|---|
| `presets/en/parallel-forge.yml::mechanism.flow.steps` | post-exec flow | 粗 integration step无法表达顺序 | 按目标表增加 verify/audit/report | hats/schema/instructions |
| `crates/ralph-cli/src/presets.rs` | actual preset contract | 逐步/失败表 | 新增 recovery tests | 文本 equality |
| `crates/ralph-core/tests/scenarios.rs` | 注册 BDD | 真实 EventLoop验收 | 增加两个 test函数 | `run_scenario` |
| 两个计划新增 scenario YAML | success/failed runtime fixtures | 覆盖生产 EventLoop seam | 最小 hats/schema/mock responses | live model |

#### 7. 可依赖能力

- U1/U2完整 flow前半段。
- `run_workflow_guard_scenario`。
- `supervisor_fan_in` 现有 exec support。
- `ExpectedYaml.absent_events` 和 event topic counts。

#### 8. 禁止依赖的未来能力

- 不依赖 U4 lint。
- 不新增 ScenarioYaml字段或第二套 runner。
- 不改 implementation-review fixtures。

#### 9. 验收测试

- 结构测试：`test_parallel_forge_post_exec_flow_converges_success_and_failure`。
- BDD success：`test_parallel_forge_declared_flow_runtime`。
- BDD failed：`test_parallel_forge_declared_flow_failed_runtime`。
- success fixture：
  - 真实 EventLoop；
  - supervisor fan-in注入 `exec.wave.complete`；
  - mock agent只产生业务 topics；
  - 断言全部 success handoff和唯一终态；
  - absent `exec.wave.failed`、`work.failed`、`forge.plan.blocked`。
- failed fixture：
  - force terminal failed fan-in或在 post-exec注入一个 `work.failed`，选择最低成本且确实穿过目标逻辑的路径；
  - 断言 reporter收尾；
  - absent后续 success topics。
- 命令：

```bash
cargo nextest run -p ralph-cli --bin ralph -- test_parallel_forge_post_exec_flow_converges_success_and_failure
cargo nextest run -p ralph-core --test scenarios -- parallel_forge_declared_flow
```

#### 10. Acceptance Red

- 先加 embedded结构测试：正确 Red 是 `forge.integration.done` 后 actual不是 `incremental_verify`。
- 再加 success BDD：正确 Red 是 verifier/tester/auditor handoff被 FlowStepScope拒绝或 completion=false。
- failed BDD 的正确 Red 是 reporter `forge.report.done` 在 failure current step被拒绝，或错误出现后续 success event。
- mock response顺序错误、缺 required field、fixture parse失败不是有效 Red。

#### 11. 单元测试拆分

1. reviewed→integration。
2. integration.done→incremental_verify。
3. incremental.verified→full_verify。
4. full.verified→audit。
5. audit.done/plan.blocked→report。
6. report.done→plan_end。
7. LOOP_COMPLETE只在 plan_end。
8. 三个 failure-capable step：work.failed stays，report.done→plan_end。
9. failure后 success topic不推进。
- Fake：MockBackend事件文本。
- 不允许 Mock：EventLoop、FlowStepScope、SupervisorCoordinator fan-in。

#### 12. Red → Green → Refactor 顺序

```text
post-exec structure Test 1 Red
→ 拆 integration/incremental_verify
→ Green
→ Test 2 Red
→ 增加 full_verify/audit
→ Green
→ block/report Test Red
→ 完成 report/plan_end
→ Green
→ success BDD Red
→ 只补 fixture和遗漏的 flow scope
→ Green
→ failed BDD Red
→ 为 failure-capable step补 forge.report.done
→ Green
→ Refactor fixture重复数据，但不抽象掉可读事件顺序
```

#### 13. 最小实现范围

- 完成目标 flow表 steps 8-14。
- 处理 success、plan.blocked、exec failure和 post-exec work.failed。
- 不增加 retry、timeout或新错误类型。
- 不更改 reporter publishes。

#### 14. 集成验证

- success/failed BDD必须通过真实 EventLoop。
- supervisor fan-in可用 InMemory store；业务 hats用 MockBackend。
- 不能直接伪造 `exec.wave.complete` 为 agent事件。
- 运行完整 `crates/ralph-core --test scenarios` 相关子集。

#### 15. 风险驱动测试

- **State-Machine：**每个 intermediate step精确断言。
- **Fault Injection：**exec failed或 post-exec `work.failed`。
- **Idempotency：**replay不 retrograde。
- **Concurrency：**Supervisor fan-in产生唯一 complete。
- 不需要 property/fuzz/mutation：输入集合是有限声明 topics，表驱动覆盖更直接。

#### 16. 回归范围

- U1/U2所有测试。
- implementation-review成功/失败 BDD。
- ce-executor-supervisor exec fanout BDD。
- 全 `ralph-core --test scenarios` 中与 wave/flow相关子集。
- CLI presets strict lint。
- `docs/plans/2026-07-27-005-fix-implementation-review-wave-stability-plan.md`
  描述的未来 outside-in 稳定性门禁不属于本计划依赖；本计划只运行当前已存在并已验证的结构测试和真实 EventLoop BDD。

#### 17. 预期文件变更

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| `presets/en/parallel-forge.yml` | 修改现有生产文件 | 完成 post-exec显式链 | E6-E9,E14 |
| `crates/ralph-cli/src/presets.rs` | 新增测试 | actual embedded transition表 | E9,E19 |
| `crates/ralph-core/tests/scenarios.rs` | 修改测试注册 | 注册 real EventLoop BDD | E11 |
| `crates/ralph-core/tests/scenarios/parallel_forge_declared_flow_runtime.yml` | 新增测试 | success runtime path | E11,E14 |
| `crates/ralph-core/tests/scenarios/parallel_forge_declared_flow_failed_runtime.yml` | 新增测试 | failed convergence | E11-E12,E14 |

#### 18. 完成标准

- S2/S5/S6/S7全部 Green。
- 两个 BDD通过真实 EventLoop。
- success/failed各自的 absent events正确。
- 所有 U1/U2和 implementation-review回归通过。
- Build/lint/typecheck通过。
- schema parity通过。
- 可独立提交。

#### 19. 停止条件

- BDD harness不能产生 exec coordination topic；
- 需要新增测试 runner字段；
- reporter同 activation的两个事件不能按顺序被 EventLoop接受；
- failure path要求改 `NON_TRANSITION_TOPICS`；
- implementation-review或其他 preset发生回归；
- fixture必须复制/锁定 prompt文本才能运行。

#### 20. 风险与注意事项

| 风险 | 触发 | 检测 | 缓解 | 剩余 |
|---|---|---|---|---|
| 最终 state对但中间错误 | 只断终态 | 每步断言 + authority progress | 保持表驱动 | 低 |
| BDD mock伪造coordination | fixture直接emit wave.complete | origin/fixture审查 | real fan-in harness | 低 |
| failure后success继续 | absent未断言 | absent_events | 每失败场景显式列出 | 低 |
| 共享输入写竞争复发 | fixture让worker写共享文件 | artifact路径审查 | immutable/read-only fixture | 低 |

### Unit 4：新增窄 lint 并关闭全 preset 回归

#### 1. Unit 目标

strict preset lint 能在启动前拒绝本次“多 topic linear step 完全依赖 positional fallback”的结构，同时全部 working presets保持 Green。

#### 2. 对应需求与 Scenario

- Requirement：R8-R10。
- Scenario：S8-S9。
- Decision：D2、D5-D8。
- Evidence：E15-E20。

#### 3. 外部可观察结果

- `ralph preset check --strict` 对 synthetic ambiguous linear flow返回稳定 finding。
- 修复后的 parallel-forge和所有 builtin仍通过 strict lint。
- preset作者文档能解释修复动作。

#### 4. 当前行为基线

- 当前 `check_flow_declaration` 不检查 positional ambiguity。
- E19证明所有现有 preset strict lint Green。
- 修复前 parallel-forge满足新 anti-pattern；U1-U3已先消除，因此 U4不会使主干出现内置 preset红灯。

#### 5. 输入与输出

- 输入：raw preset YAML。
- 输出：`preset.flow_linear_positional_ambiguity` finding或空。
- 错误：规则为结构错误；不得通过 warning在 strict外静默。
- 状态/副作用：无 runtime state。
- 不变量：非-linear、末尾 linear、单 topic linear、存在至少一个 forward target的 linear均不触发。

#### 6. 修改位置

| 位置 | 当前职责 | 原因 | 边界 | 不修改 |
|---|---|---|---|---|
| `crates/ralph-core/src/preset_lint/flow_declaration.rs` | flow lint聚合 | 缺 anti-pattern guard | 加一个纯结构检查 | runtime transition |
| `crates/ralph-core/src/preset_lint/flow_declaration/tests.rs` | flow lint单测 | TDD | synthetic fixtures | preset全文 |
| `crates/ralph-core/src/preset_lint/finding_id.rs` | stable IDs | 新公开 finding | 常量+ALL列表 | 旧 ID |
| `crates/ralph-core/src/preset_lint/mod.rs` | public exports | 保持公开面一致 | 按现有模式 re-export | 其他模块 |
| operator references | author/review规则 | 新 finding可操作 | 通用说明 | plan/preset特例 |

#### 7. 可依赖能力

- U1-U3修复后的 explicit flow。
- `FlowDeclaration::from_yaml`。
- `LintFinding`和稳定 ID机制。
- 全 builtin strict lint tests。

#### 8. 禁止依赖的未来能力

- 不增加 lint config开关。
- 不修改 runtime或迁移其他 preset。
- 不新增 CLI命令。

#### 9. 验收测试

- 新测试：
  - ambiguous multi-topic non-final linear → exactly one new finding；
  - 有 forward `on` → no finding；
  - 有 forward `on_any_of` → no finding；
  - single-topic linear → no finding；
  - terminal/last linear → no finding；
  - foreach/side_effect/await legacy shapes → no finding；
  - implementation-review和ce-executor-supervisor raw YAML → no new finding；
  - repaired parallel-forge → no new finding。
- 命令：

```bash
cargo nextest run -p ralph-core -- flow_linear_positional_ambiguity
cargo nextest run -p ralph-core -- preset_lint
```

#### 10. Acceptance Red

- 先新增 synthetic invalid test。
- 正确 Red：findings中不存在 `preset.flow_linear_positional_ambiguity`。
- 不允许通过修改 fixture为已有 finding来制造 Red。

#### 11. 单元测试拆分

1. exact invalid shape。
2. `on` target exemption。
3. `on_any_of` target exemption。
4. non-linear legacy exemption。
5. last/single topic exemption。
6. finding message、step id/hat、action hint。
7. stable ID in `ALL_FINDING_IDS`。
8. all builtin strict pass。
- 不允许 Mock：raw YAML parser和 lint aggregator。

#### 12. Red → Green → Refactor 顺序

```text
invalid fixture Red
→ 增加 finding ID和最小规则
→ Green
→ explicit target tests Red/Green
→ legacy exemption tests Red/Green
→ finding metadata test Red/Green
→ all builtin strict regression
→ Refactor helper，保持单次step scan
→ 更新operator docs
```

#### 13. 最小实现范围

规则只在以下全部成立时触发：

1. 当前 step 不是最后一个；
2. `kind == "linear"`；
3. `allowed_emits.len() >= 2`；
4. 所有后续 step 的 `on` / `on_any_of` 与当前 allowed topics 的交集为空。

不得扩展为“所有 topic必须声明 target”，不得读取 hat prompt，不得更改 severity随 strictness的通用机制。

#### 14. 集成验证

- 运行三组 preset硬规则命令。
- 运行实际 `ralph preset check --strict` 的现有 CLI测试；若需要命令烟测，使用修复后的 builtin和一个临时 invalid YAML。
- 不修改 CLI parser。

#### 15. 风险驱动测试

- **Differential：**全部 builtin前后 finding集合除新规则对 synthetic fixture外无变化。
- **Characterization：**implementation-review/ce-supervisor保持零新 finding。
- **Mutation思路：**临时删除 parallel-forge任一规划 step的显式 target，应触发新 finding；只作本地验证，不保留 mutation。

#### 16. 回归范围

- flow declaration全部 tests。
- finding ID lock。
- core preset lint 258+ tests。
- CLI preset lint gate和56+ preset tests。
- implementation-review结构测试和真实 EventLoop BDD。
- schema parity、manifest/index/zsh completion tests。
- `scripts/check-cli-doc-drift.sh`。
- 最终 `./scripts/run-tests.sh`。

#### 17. 预期文件变更

| 位置 | 变更类型 | 原因 | Evidence |
|---|---|---|---|
| `crates/ralph-core/src/preset_lint/flow_declaration.rs` | 修改现有生产文件 | 增加窄 lint | E15-E16 |
| `crates/ralph-core/src/preset_lint/flow_declaration/tests.rs` | 新增测试 | TDD和non-regression | E16,E19 |
| `crates/ralph-core/src/preset_lint/finding_id.rs` | 修改现有生产文件 | stable finding ID | E17 |
| `crates/ralph-core/src/preset_lint/mod.rs` | 修改现有生产文件 | 公开 re-export | E17 |
| `skills/ralph-preset-common/references/finding-rubric.md` | 修改文档 | finding映射 | E18 |
| `skills/ralph-preset-common/references/patterns.md` | 修改文档 | 通用 authoring pattern | E18 |

#### 18. 完成标准

- S8/S9全部 Green。
- 新 finding精确、稳定、可操作。
- 所有 builtin strict lint Green。
- implementation-review所有指定回归 Green。
- `presets/schemas/parallel-forge.yml` 审计确认无字段变化，且未产生无理由 diff。
- `crates/ralph-core/data/*.md` 审计确认无 agent命令/字段变化，不修改。
- `CLAUDE.md` / `AGENTS.md`、manifest/index/zsh审计确认 preset identity/topology描述无需变化，不修改。
- fmt/clippy/typecheck/build/full test Green。
- 无 skip/only/snapshot更新/断言削弱。
- 可独立提交。

#### 19. 停止条件

- 新 lint命中任何 working builtin；
- 必须添加 preset-name exemption；
- finding无法在 raw flow结构上确定；
- 需要修改 `advance_plan_step`；
- operator docs要求写入特定事故/plan内容；
- full suite出现与本计划相关的非预期回归。

#### 20. 风险与注意事项

| 风险 | 触发 | 检测 | 缓解 | 剩余 |
|---|---|---|---|---|
| lint过宽破坏其他 preset | existing builtin新 finding | all builtin differential | 四条件窄规则 | 极低 |
| lint过窄漏掉部分 ambiguity | 有一个 target但其他 topic位置推进 | 已知限制记录 | 本计划只拦截实际事故形状；后续另案 | 中低 |
| finding docs drift | rubric无ID | rg/fixture review | 同 Unit更新 | 低 |
| 全量被并行改动影响 | unrelated test失败 | baseline对比 | 不擅自扩大范围，记录并停止 | 中 |

---

## 8. Unit 串行依赖图

```text
Unit 1：规划与阻断 handoff
  ↓ 使用已验证的 planning→exec_wave entry
Unit 2：exec wave 成功/失败收敛
  ↓ 使用已验证的 unit_review entry 和 failure terminal
Unit 3：post-exec 成功/失败终态
  ↓ 使用完整显式 flow，避免 builtin 在 lint落地时自我失败
Unit 4：窄 lint 与全 preset门禁
```

- U2依赖U1：必须先能合法进入 `exec_wave`，否则 wave branch测试没有真实前置状态。
- U3依赖U2：必须先能合法到达 `unit_review` / `exec_failure`。
- U4依赖U3：新 lint落地前 parallel-forge必须已消除完全位置化 linear step，否则全 builtin strict lint会红，U4无法独立关闭。
- 不可交换；每个 Unit不得提前实现后续行为。

---

## 9. 执行命令清单

| 时机 | 命令 | 目的 | 预期 | 失败后能否继续 |
|---|---|---|---|---|
| 每 Unit 开始 | `git status --short` | 识别并行改动 | 用户诊断报告保留，无目标文件冲突 | 否 |
| U1 Red/Green | `cargo nextest run -p ralph-cli --bin ralph -- test_parallel_forge_planning_flow_uses_declared_handoffs` | planning authority | Red原因匹配后 Green | 否 |
| U2 Red/Green | `cargo nextest run -p ralph-cli --bin ralph -- test_parallel_forge_exec_wave_declares_success_and_failure_handoffs` | exec wave branch | Red原因匹配后 Green | 否 |
| U2 integration | `cargo nextest run -p ralph-core --test scenarios -- test_opac_sb1_supervisor_exec_wave_fanout` | real exec fan-in | 通过 | 否 |
| U3 Red/Green | `cargo nextest run -p ralph-cli --bin ralph -- test_parallel_forge_post_exec_flow_converges_success_and_failure` | post-exec states | Red原因匹配后 Green | 否 |
| U3 BDD | `cargo nextest run -p ralph-core --test scenarios -- parallel_forge_declared_flow` | real EventLoop success/failure | 全通过 | 否 |
| U4 lint Red/Green | `cargo nextest run -p ralph-core -- flow_linear_positional_ambiguity` | 新规则 | Red原因匹配后 Green | 否 |
| 每 Unit flow回归 | `cargo nextest run -p ralph-core -- advance_plan_step` | generic semantics | 全通过 | 否 |
| implementation-review结构回归 | `cargo nextest run -p ralph-cli --bin ralph -- test_implementation_review_adopts_generic_mechanism_contract` | working preset保护 | 通过 | 否 |
| implementation-review BDD回归 | `cargo nextest run -p ralph-core --test scenarios -- implementation_review_wave_runtime` | success/failed fan-in保护 | 通过 | 否 |
| core lint | `cargo nextest run -p ralph-core -- preset_lint` | flow lint全回归 | 全通过 | 否 |
| CLI lint | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` | CLI gate | 全通过 | 否 |
| builtin parity | `cargo nextest run -p ralph-cli --bin ralph -- presets` | manifest/strict/schema/zsh | 全通过 | 否 |
| scenario回归 | `cargo nextest run -p ralph-core --test scenarios` | BDD全量 | 全通过 | 否 |
| 文档 drift | `scripts/check-cli-doc-drift.sh` | agent CLI docs准确 | 退出0 | 否 |
| 格式 | `cargo fmt --check` | 格式门禁 | 退出0 | 否 |
| Typecheck | `cargo check --workspace` | Rust类型检查 | 退出0 | 否 |
| Lint | `cargo clippy` | workspace lint | 退出0 | 否 |
| Build | `cargo build` | 构建目标 | 退出0 | 否 |
| 最终全量 | `./scripts/run-tests.sh` | nextest两阶段+doctest | 全通过 | 否 |

说明：

- 禁止裸跑 `cargo test -p ralph-cli`。
- 本计划不新增 spawn `ralph` 的 CLI 集成测试，因此不新增 hat-env scrub 命令；若 Executor 改变测试层级并新增此类测试，必须停止并修订计划，不能临场绕过 HARD RULE 5。
- 若 full suite只出现已知时序 flake，按仓库规则使用 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底；serial仍失败即是真失败。
- Contract/API/DB migration测试不适用，因为没有公开契约或数据格式变化。

---

## 10. 最终质量门禁

- [ ] S1-S9全部通过。
- [ ] R1-R10都有可执行测试。
- [ ] 所有新增单元测试通过。
- [ ] 两个 parallel-forge真实 EventLoop BDD通过。
- [ ] Supervisor exec fan-in BDD通过。
- [ ] implementation-review结构化 flow测试通过。
- [ ] implementation-review success/failed fan-in BDD通过。
- [ ] replay/idempotency断言通过。
- [ ] failure fault-injection断言通过。
- [ ] 所有 builtin strict lint通过。
- [ ] parallel-forge schema parity通过且 schema无不必要修改。
- [ ] `cargo fmt --check`、`cargo check --workspace`、`cargo clippy`、`cargo build`通过。
- [ ] `scripts/check-cli-doc-drift.sh`通过。
- [ ] `./scripts/run-tests.sh`通过。
- [ ] 未新增失败/skip/ignore/only。
- [ ] 未更新 snapshot/golden。
- [ ] 未削弱断言。
- [ ] 未修改 `advance_plan_step`、`NON_TRANSITION_TOPICS` 或 supervisor runtime。
- [ ] 未修改 `presets/en/implementation-review.yml` 或其 schema。
- [ ] 未修改其他 builtin preset生产 YAML。
- [ ] 未覆盖或提交用户提供的诊断报告。
- [ ] 新 finding已进稳定 ID表和 operator rubric。
- [ ] 没有未处理 BLOCKED决策。
- [ ] 所有实际变更都在本计划预期文件范围。
- [ ] U1→U2→U3→U4严格串行，各自形成完整 TDD闭环和独立提交边界。

---

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 四个 Unit 都以可观察行为、真实 Red、最小实现和命令闭环 |
| Executor 是否仍需做关键设计决策 | 否 | 目标 flow表、lint四条件、文件和测试入口均已确定 |
| 所有文件和接口是否有代码库证据 | 是 | E6-E20；新 fixture明确标记计划新增 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1-D9最低0.94 |
| 是否存在未处理的低置信度假设 | 否 | 执行期变化有停止条件，不影响当前决策 |
| 每个 Unit 是否只有一个可观察行为 | 是 | planning、exec wave、post-exec、lint分别独立 |
| 每个 Unit 是否可以独立验证 | 是 | 每 Unit有targeted命令和回归 |
| 每个 Unit 是否有真实 Red | 是 | U1-U4均写明修改前实际失败值/原因 |
| 每个 Unit 是否包含回归范围 | 是 | 每 Unit §16 |
| 是否存在未来 Unit 依赖 | 否 | 每 Unit只依赖已完成前置，不依赖未来能力 |
| 是否存在泛化任务描述 | 否 | 修改位置、测试名、输入输出、断言均具体 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §5、§6矩阵 |
| 所有关键决策是否有 Evidence | 是 | D1-D9引用E-ID |
| 计划是否可以严格串行执行 | 是 | §7、§8线性依赖 |
| 是否保护 implementation-review | 是 | D6、S9、各 Unit回归、最终门禁 |
| 是否保持 generic runtime语义 | 是 | D1-D3；明确禁止修改 runtime |
| 是否检查 schema和agent/operator docs | 是 | D8、U4完成标准和命令 |

计划自检满足输出条件，状态保持 READY。
