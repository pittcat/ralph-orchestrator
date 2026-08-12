---
title: "feat: 声明式事件发射前置检查(precheck_prompt LLM 关卡)"
type: feat
status: active
date: 2026-07-02
origin: docs/achieved/brainstorms/2026-07-02-event-emit-precheck-prompt-gate-requirements.md
---

# feat: 声明式事件发射前置检查(`precheck_prompt` LLM 关卡)

## Overview

给 preset 作者一个**声明式、逐事件的 LLM-as-judge 关卡**:在 SSOT 里给某个 topic 挂一段 checklist(`precheck.rules.<topic>`),系统在该事件真正生效前,自动跑一轮 LLM gate 逐点检查,过了才放行真事件,不过则带原因打回、有界重试、耗尽升级到终态。

实现走 **B1 脱糖**(不是 B2 内联 runtime judge):在配置规范化阶段把被守 topic `X` 的 producer 改为发 `X.proposed`,并**合成一个 gate hat**消费 `X.proposed`、发 `X`(过)/`X.rejected`(不过)。引擎、调度、replay 全部零改动——复用两处既有事实:LLM-as-judge 以 hat 落地(review 维度 reviewer),以及运行时合成 hat 的先例(`HatRegistry::add_builtin_ralph`)。

分三个里程碑:

- **里程碑 A(配置 + 脱糖,默认零行为变更)**:定义 `precheck` 配置类型与 SSOT schema、`build.rs` merge、`RalphConfig::normalize` 脱糖 transform、gate hat instructions 生成,外加**零回归守卫**(未声明 = 严格 no-op)。
- **里程碑 B(硬门 + 失败闭环)**:gate hat 二选一硬门语义;`X.rejected` 结构化原因 → `on_fail.target` 打回 → `retry_budget` 有界 → `on_exhausted` 终态升级。全部复用 `rejection.rs` / `repair_budget` / `plan.blocked`。
- **里程碑 C(preset_lint + 下游同步 + 验证)**:让 6 条 preset_lint 规则认识新机制与合成 hat;按 AGENTS.md 硬规则同步 schema/manifest/index/presets.rs/zsh/skill guide;用 mock e2e fixture 端到端验证通过/失败两条路径。

> **两条硬性约束(用户明确要求)**:①**关键字开启**——`precheck.enabled: true` 且有 `rules` 才生效,并有 `RALPH_PRECHECK_MODE=off` kill switch;②**零回归**——未启用的一切 preset / run 行为与今天结构等价,所有出厂 builtin preset 默认不带 `precheck`。

---

## Problem Frame

当前对"某个事件该不该现在发"的治理全是机械判定:
- `crates/ralph-core/src/config/loop_config.rs` 的 `EventSchema.required_fields` / `allowed_values`(字段);
- `crates/ralph-core/src/execution_contract.rs`(git / task / test 证据);
- `crates/ralph-core/src/event_loop/stages/flow_step_scope_stage.rs`(`mechanism.flow.allowed_emits`,step 允许集);
- `crates/ralph-core/src/event_policy.rs`(dedup / topic 白名单 / topic_deny)。

没有任何一层能"跑一段 prompt 让 LLM 核对主观检查点"。需要判断的那一类(如"6 个维度 findings 是否有实质内容")只能靠 prompt 叮嘱 agent 自查,而 agent 常忽略——历史上多次导致 `ce-executor-serial` 2 小时级 abort(诊断报告群见 `docs/report/2026-06-1x-*`)。

Ralph Tenet #2 明确支持"主观判断用 LLM-as-judge 出 binary pass/fail",现有落地形态是**整个 reviewer hat**(如 `dimension-reviewer`)。本计划把这个能力做成**声明式、可挂在任意关键 topic 上的关卡**,而不必每次手写一个 reviewer hat。

关键架构事实(已核实):`ralph-core` **不依赖 `ralph-adapters`、无网络路径、事件循环从不调 LLM**;LLM 调用只发生在 `crates/ralph-cli/src/loop_runner/runner.rs` 的 `.execute(&prompt, …)`,且只作为一轮 hat turn。因此 B1(合成 gate hat)是唯一不破坏引擎与 replay 的路径。

---

## Requirements Trace

承接需求文档 R1–R11:

- **R1.** 声明式挂检查点,不手写 gate hat。 → U1
- **R2.** 加载期自动脱糖:producer 改发 `X.proposed` + 合成 gate hat;下游消费者不变。 → U2
- **R3.** gate hat 硬门:一轮必须且只能发 `X` 或 `X.rejected`。 → U5
- **R4.** `X.rejected` 携带结构化失败原因。 → U6
- **R5.** 失败路由回 `on_fail.target` 并注入下一轮 prompt。 → U6
- **R6.** 候选与真事件之间只插一轮 gate,受 dedup 约束。 → U2, U7
- **R7.** 重试有界(`retry_budget`)。 → U6
- **R8.** 预算耗尽升级到 `on_exhausted` 终态。 → U6
- **R9.** 关键字显式开启 + `RALPH_PRECHECK_MODE=off` kill switch;未启用严格 no-op。 → U1, U2, U4
- **R10.** 未启用行为逐字节/结构等价;出厂 builtin 默认不带 `precheck`。 → U4
- **R11.** 机械检查不进 `precheck_prompt`(仅文档/lint 提示,不强制)。 → U3(生成的 instructions 里显式声明职责边界), U8

---

## Implementation Units

### 里程碑 A — 配置与脱糖(默认零行为变更)

#### U1. `precheck` 配置类型 + SSOT schema + build.rs merge
- **改什么**:
  - 新增 `crates/ralph-core/src/config/precheck.rs`,仿 `config/execution_contracts.rs` 的 `{ enabled: bool, rules: BTreeMap<String, PrecheckRule> }`。`PrecheckRule { prompt: Vec<String>, on_fail: PrecheckOnFail }`;`PrecheckOnFail { target: HatId, retry_budget: u32(默认取 mechanism.repair_budget), on_exhausted: String(终态 topic+reason), reason: String }`。
  - 挂到 `EventLoopConfig`(新增 `precheck: Option<PrecheckConfig>`,`#[serde(default)]`)。
  - schema authoring:在 `presets/schemas/` 头部 Merge mapping 里追加 `precheck: → event_loop.precheck`,并让 `crates/ralph-cli/build.rs` 深合并该顶层块(与 `execution_contracts` 同路径)。
- **为什么**:R1/R9——声明式单一来源 + `enabled` 作为唯一开关。
- **验证**:`cargo nextest run -p ralph-core -- precheck::config`(round-trip 一个含 `precheck` 块的最小配置,断言反序列化正确、`enabled=false` 时 `rules` 被忽略)。

#### U2. `RalphConfig::normalize` 脱糖 transform(核心)
- **改什么**:在 `crates/ralph-core/src/config/ralph_config.rs` 的 `normalize()` 里,当 `event_loop.precheck.enabled == true && !rules.is_empty() && env RALPH_PRECHECK_MODE != "off"` 时,对每条 `rules.<X>`:
  1. 找出所有 `publishes`/`terminal_events` 含 `X` 的 hat,把这些条目**改写为 `X.proposed`**(下游订阅 `X` 的 hat 不动);
  2. 合成一个 `HatConfig`(id 形如 `precheck-<X>`):`triggers=[X.proposed]`, `publishes=[X, X.rejected]`, `terminal_events=[X, X.rejected]`, `instructions` 占位(U3 填),`max_activations` 合理上限;仿 `HatRegistry::add_builtin_ralph`(`crates/ralph-core/src/.../hat_registry.rs`)的程序化注册路径插入 `config.hats`。
  - 多 producer:全部改发 `X.proposed`,单 gate 消费。
- **为什么**:R2/R6——脱糖是整个特性的机制核心,复用合成 hat 先例,引擎无需改动。
- **验证**:`cargo nextest run -p ralph-core -- precheck::desugar`(断言:启用后 `config.hats` 出现 `precheck-<X>` 且 triggers/publishes 正确、原 producer 的 `X` 被改写为 `X.proposed`;`enabled=false` 时 `config.hats` 不变)。

#### U3. gate hat instructions 生成
- **改什么**:在脱糖时把 `rules.<X>.prompt` 的检查点渲染进合成 hat 的 `instructions`,包含:逐点 checklist、"你必须且只能 emit `X` 或 `X.rejected` 之一"的硬指令、"不过时在 `X.rejected` payload 写明哪几个检查点未过及原因"、以及"本关卡只做主观判断,机械检查由确定性门负责"(R11 职责边界)。复用 `crates/ralph-core/src/.../instructions.rs` 的 `build_custom_hat` 直通 `hat.instructions` + 自动生成的"emit exactly one"块。
- **为什么**:R3/R4/R11——把检查点变成可执行的 gate prompt。
- **验证**:`cargo nextest run -p ralph-core -- precheck::instructions`(对合成 hat 跑 `build_prompt`,断言含全部检查点文本 + 二选一硬指令)。

#### U4. 零回归守卫(硬性)
- **改什么**:新增 golden/回归测试:
  - 对每个出厂 builtin preset(不含 `precheck`)断言脱糖为**严格 no-op**——`normalize` 前后 `config.hats` / topic 拓扑结构等价(HatRegistry 结构快照比对)。
  - `RALPH_PRECHECK_MODE=off` 时即使声明了 `precheck` 也跳过脱糖(AE5)。
  - 确认所有 `presets/en/*.yml` 默认不含 `precheck` 块。
- **为什么**:R9/R10——这是"零回归"的契约测试,必须先立住再往下做。
- **验证**:`cargo nextest run -p ralph-core -- precheck::no_regression` + `cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded`(SSOT byte-equality 不被破坏)。

### 里程碑 B — 硬门与失败闭环

#### U5. gate hat 硬门语义
- **改什么**:用合成 hat 的 `terminal_events=[X, X.rejected]` + `obligations`(仿 reviewer)强制"一轮必发且只发其一";沉默/两者都发由现有 obligation gate 处理。在 `presets/schemas/` 为 `X.proposed` / `X.rejected` 补 schema `required_fields`。
- **为什么**:R3——软门(agent 自律)不可靠,必须硬约束二选一。
- **验证**:BDD `crates/ralph-core/tests/scenarios/2026-07-02-precheck-gate.yml` + `scenarios.rs`,用 `run_workflow_guard_scenario`(真 EventLoop runner,断言事件),覆盖 gate 发 `X`(过)与 `X.rejected`(不过)两支。

#### U6. 失败闭环:结构化原因 → 打回 → 有界 → 升级
- **改什么**:
  - `X.rejected` payload 带 `failed_checks` / `reason`(R4);
  - 路由回 `on_fail.target`,复用 `crates/ralph-core/src/.../rejection.rs`(`build_task_resume_payload`)+ `LintResumeHint`/rejection-digest 把原因注入 target 下一轮 prompt(R5);
  - `retry_budget` 复用 `mechanism.repair_budget` 计数(与 `stall_recovery_counts` 对齐),每次 `X.rejected` 递增;
  - 耗尽 → 发 `on_fail.on_exhausted`(默认 `plan.blocked(reason=precheck_failed)`),复用 verdict/plan.blocked 现成升级路径(R7/R8)。
  - 在 `presets/schemas/` 的 `plan.blocked.allowed_values.reason` 白名单加入 `precheck_failed`。
- **为什么**:防止重建历史 stall bug——不过必须有界收敛到终态。
- **验证**:BDD 场景:gate 连续判不过 3 次 → 断言最终事件为 `plan.blocked(reason=precheck_failed)` 而非再次打回(AE2);另一场景断言原因注入了 target 的 prompt(AE3)。

#### U7. topic 白名单 / dedup / origin-guard 兼容
- **改什么**:确保 `X.proposed` / `X.rejected` 进入 `event_policy` 的 allowed topics(`build_allowed_topics`)、有 dedup key(避免同一候选重复插 gate,R6)、EventOriginGuard `can_publish` 授权(producer 可发 `X.proposed`、gate hat 可发 `X`/`X.rejected`)。
- **为什么**:脱糖新引入两个 topic,必须被现有 runtime 门认识,否则被误拒。
- **验证**:`cargo nextest run -p ralph-core -- event_policy` + `-- event_origin`(断言新 topic 不被 topic-format / origin 门误拒)。

### 里程碑 C — preset_lint + 下游同步 + 端到端验证

#### U8. preset_lint 认识新机制
- **改什么**:让 `crates/ralph-core/src/preset_lint/` 六条规则对脱糖后的图成立:`schema_parity`(`X.proposed`/`X.rejected` 有 schema)、`ownership`(gate hat 拥有 `X`)、`multi_hat`(**合成 hat 计入 hat 数**——加 gate 可能顶破 3-hat coordinator 上限,须触发 isolated 要求;`ce-executor-serial` 已 isolated 不受影响)、`topic_format`、`workflow_activation`(gate hat 可达)、`state_projection`。lint 应在**脱糖后**的配置上跑。
- **为什么**:AGENTS.md 硬规则——preset/schema 改动必须过 preset_lint;合成 hat 不能绕过静态门。
- **验证**:`cargo nextest run -p ralph-cli --bin ralph -- preset_lint` + `cargo nextest run -p ralph-core -- preset_lint`。

#### U9. 下游同步(AGENTS.md 硬规则)+ skill guide
- **改什么**:
  - 若给任何示例/ fixture preset 加 `precheck`:同步 `presets/manifest.yml` / `presets/index.json` / `crates/ralph-cli/src/presets.rs` 的 `PRESETS` / `scripts/ralph-zsh-plugin.zsh`;
  - **AI skill guide**:`precheck` 是 agent 会遇到的新运行时行为(gate hat prompt、`.proposed`/`.rejected` topic),按 HARD RULE 更新 `crates/ralph-core/data/ralph-tools*.md` 相关段落,并 `sed -n` 复核所有 `xxx.rs:NN-MM` 引用;
  - `CLAUDE.md` / `AGENTS.md` 同步(`cp`),`.cursor/rules/*.mdc` 相关段(multi-hat-isolation / feature-flags)补一节。
- **为什么**:AGENTS.md 明确规定这些改动必须逐层同步,漏改视为违规。
- **验证**:`bash scripts/check-cli-doc-drift.sh`;涉及命令语法处跑对应 `ralph <cmd> --help` 冒烟。

#### U10. mock e2e fixture 端到端验证
- **改什么**:新增一个**仅用于测试**的启用 `precheck` 的 fixture preset(不入出厂 builtin),配 e2e cassette,跑通过/失败两条路径。
- **为什么**:证明 gate hat 轮次被 replay cassette 捕获(架构核实:gate 是普通 hat turn,`runner.rs` 的 `.execute` 边界照常被 mock CLI 拦截),且失败闭环端到端收敛。
- **验证**:`cargo run -p ralph-e2e -- --mock`(用该 fixture,断言过→真 `X`、连续不过→`plan.blocked(reason=precheck_failed)`)。

---

## 最终验证基线

按 AGENTS.md HARD RULE 1/2(nextest 入口、默认并发、ralph-cli 串行):

```bash
# 子集(开发中)
cargo nextest run -p ralph-core -- precheck
cargo nextest run -p ralph-core --test scenarios -- precheck
cargo nextest run -p ralph-cli --bin ralph -- preset_lint

# 最终基线(准备完成前)
./scripts/run-tests.sh            # nextest 全 workspace + doctest
cargo run -p ralph-e2e -- --mock  # e2e
bash scripts/check-cli-doc-drift.sh
```

---

## Risks / Trade-offs

- **成本**:每个被守事件多一轮 LLM 调用(更慢 + 更多 token)。缓解:文档强调只守少数关键终态事件(`review.complete` / `plan.complete` 类),R11 禁止把机械检查塞进 `precheck_prompt`。
- **误判**:gate hat 本身是 LLM,可能误过/误拒。缓解:失败有界 + 升级到终态(U6),不会因误拒无限卡死;误过由下游确定性门兜底。
- **isolation 上限**:合成 gate hat 计入 hat 数,可能把小 preset 顶过 3-hat coordinator 上限。缓解:U8 让 `multi_hat` lint 在脱糖后图上判定并给出明确报错;大 preset(已 isolated)不受影响。
- **多 producer topic**:`plan.complete`/`plan.blocked` 可能有多个 producer。缓解:U2 全部改发 `X.proposed`;`on_fail.target` 显式指定打回对象消解歧义。
- **零回归**:U4 的 golden/结构快照是硬门,必须先立;任何破坏 SSOT byte-equality 或改变未启用 preset 拓扑的改动都要在 U4 处 fail。

---

## Out of Scope

- B2 内联 runtime LLM judge(破坏引擎纯 CPU 契约 + replay,已排除)。
- 给出厂 builtin preset 预置 `precheck` 块(保零回归)。
- human-in-the-loop 人工审批(后续可选)。
- 机械检查迁入 `precheck_prompt`(职责边界,归确定性门)。
