---
date: 2026-07-02
topic: event-emit-precheck-prompt-gate
---

# 声明式事件发射前置检查(`precheck_prompt` LLM 关卡)

> 面向 `/ce-plan` 的需求文档。这是一次技术/架构型 brainstorm,包含机制层细节;但不锁定具体字段 schema、函数签名等实现细节(留给计划阶段)。

## Problem Frame

`ce-executor-serial` 这类多 hat 编排里,一个 hat 一轮可以发很多终态事件(`work.done` / `review.complete` / `plan.complete` …)。当前对"某个事件该不该现在发"的治理全是**机械判定**(字段在不在、git 有没有改、这一步准不准发这个 topic、去重),没有任何一层能"跑一段 prompt、让 LLM 核对若干主观检查点、通过了才准发"。

结果是 agent 反复"乱发事件"——发早、发错、越权、重复/幻觉发射——而现有机械门拦不住需要判断的那一类(比如"6 个维度的 findings 是不是真有实质内容"),只能靠 prompt 里叮嘱 agent 自查,而 agent 常常忽略指令。历史上这直接导致 `ce-executor-serial` 多次 2 小时级 abort(见诊断报告群)。

本需求要新增一个**声明式、逐事件的 LLM 前置检查关卡**:作者在 preset 里给某个 topic 挂一段 checklist prompt,系统在该事件真正生效前,自动跑一轮 LLM-as-judge 逐点检查,过了才放行真事件,不过则带原因打回、有预算、耗尽升级到终态。

受影响面:多 hat preset 的运行可靠性;preset 作者对"关键事件质量门"的可声明能力。

---

## Actors

- A1. Runtime 调度器:识别被守 topic、调度 gate 轮次、执行失败闭环。**关卡的最终权威。**
- A2. Producer hat:干完活,发出**候选信号**(`X.proposed`),不直接发真事件。
- A3. Gate hat(合成,不在作者手写的 YAML 里):跑 checklist prompt,二元判决,发真 `X`(过)或 `X.rejected`(不过)。
- A4. Preset 作者:在 SSOT 里声明 `precheck` 规则(检查点 + 失败策略),不手写 gate hat。

---

## Key Flows

- F1. 通过路径(pass)
  - **Trigger:** producer 干完活,发 `X.proposed`。
  - **Actors:** A2, A1, A3
  - **Steps:** producer 发 `X.proposed`(内部信号,不驱动下游)→ 系统按订阅路由到 gate hat → gate hat 独立一轮跑 checklist(可读文件/查 git/看 tasks)→ 判为过 → 发真正的 `X` → 下游被驱动。
  - **Outcome:** 只有真正满足检查点的 `X` 才生效。
  - **Covered by:** R1, R2, R3, R6

- F2. 失败路径(fail)+ 有界升级
  - **Trigger:** gate hat 判为不过。
  - **Actors:** A3, A1, A2
  - **Steps:** gate hat 发 `X.rejected`(带"哪几个检查点没过 + 原因")→ 路由回 `on_fail.target`(默认原 producer),原因注入其下一轮 prompt → producer 针对失败点重做、再发 `X.proposed` → 受 `retry_budget` 约束;预算耗尽 → 发 `on_exhausted` 终态(如 `plan.blocked(reason=precheck_failed)`)确定性收尾。
  - **Outcome:** 不过时有可执行反馈、有界重试、绝不无限打回。
  - **Covered by:** R4, R5, R7, R8

- F3. 关键字开启 + 零回归
  - **Trigger:** preset 没声明 `precheck` / 或 `RALPH_PRECHECK_MODE=off`。
  - **Actors:** A1
  - **Steps:** 脱糖 transform 检测到未启用 → 完全不改 hat 图、不合成 gate hat、不改任何 topic → 行为与今天逐字节一致。
  - **Outcome:** 纯功能增强,不引入回归。
  - **Covered by:** R9, R10

---

## Requirements

**声明与脱糖(作者只写声明,不手写 gate hat)**
- R1. 作者在 preset SSOT 里以声明式块(`precheck.enabled` + `precheck.rules.<topic>`)挂检查点,不得手写 gate hat 或 `.proposed`/`.rejected` 拓扑。
- R2. 系统在配置加载/规范化阶段自动脱糖:把被守 topic `X` 的所有 producer 改为发 `X.proposed`,并合成一个 gate hat(`triggers=[X.proposed]`, `publishes=[X, X.rejected]`, 检查点生成进其 instructions)。下游订阅 `X` 的消费者不变。
- R3. Gate hat 必须是**硬门**:一轮必须且只能发 `X` 或 `X.rejected` 之一(靠 `terminal_events`/`obligations` 强制),不允许沉默或两者都发。

**失败闭环(不重蹈 stall)**
- R4. `X.rejected` 必须携带**结构化失败原因**(哪几个检查点没过、为什么),供 producer 下一轮有依据地重做。
- R5. 失败必须路由回可配置的 `on_fail.target`(默认原 producer),并复用现有 `task.resume` / rejection-digest 管道把原因注入下一轮 prompt。
- R7. 失败重试必须**有界**(`on_fail.retry_budget`,默认复用 `repair_budget` 语义)。
- R8. 预算耗尽必须**升级到终态事件**(`on_fail.on_exhausted`,如 `plan.blocked(reason=precheck_failed)`),确定性收尾,禁止无限打回。

**开关与零回归(硬性约束)**
- R6. 被守 topic 每次要发出时,在"候选 → 真事件"之间插入且仅插入一轮 gate(受现有 dedup 约束,不重复插)。
- R9. 功能必须**关键字显式开启**:`precheck.enabled: true` 且存在 `rules` 才生效;缺失/为 false/被 `RALPH_PRECHECK_MODE=off` 关闭时,脱糖为严格 no-op。
- R10. 未启用该功能的 preset / run 行为必须与当前**逐字节/结构等价**;所有出厂 builtin preset 默认不带 `precheck` 块。

**成本自律(不滥用 LLM 轮次)**
- R11. 机械可判定的检查(字段/ git / task 状态 / step 允许集)不得进入 `precheck_prompt`,应继续交给现有确定性门;`precheck_prompt` 只用于必须读产物、要推理的主观判断。

---

## Acceptance Examples

- AE1. **Covers R2, R6.** 给定 `precheck.rules.review.complete` 已声明,当 `review-synthesizer` 发 `review.complete` 时,runtime 实际收到的是 `review.complete.proposed`,并激活合成 gate hat 一轮;真 `review.complete` 在 gate 判过之前不出现在 bus 上。
- AE2. **Covers R3, R7, R8.** 给定 gate hat 连续 3 轮判不过,当第 3 次 `X.rejected` 后,runtime 必须发 `plan.blocked(reason=precheck_failed)` 而非再次打回。
- AE3. **Covers R4, R5.** 给定 gate 判不过并写明"检查点 2:findings 文件为空",当 producer 下一轮启动时,该原因必须出现在其 prompt 的 rejection 反馈块里。
- AE4. **Covers R9, R10.** 给定一个不含 `precheck` 块的 builtin preset,当 `cargo build` 生成 embedded 配置并构建 HatRegistry 时,结果必须与未引入本功能前结构等价(golden 断言)。
- AE5. **Covers R9.** 给定 `precheck.enabled: true` 但设置 `RALPH_PRECHECK_MODE=off`,当加载配置时,脱糖必须被跳过,不合成 gate hat。

---

## Success Criteria

- 人的判断:对被守的关键事件,"发早/发错/凭空发"当场被 gate 拦下并给出可执行反馈;不再靠 prompt 叮嘱自律。
- 可靠性:失败路径永远有界并收敛到终态,不再出现"打回去没人能过 → 2 小时 abort"的 stall。
- 零回归:未启用的一切 preset / run 行为不变(有 golden/回归测试背书)。
- 下游交接:`/ce-plan` 拿到本文档 + 配套计划即可实施,无需再发明产品行为、范围或成败标准。

---

## Scope Boundaries

- 不做 B2(runtime 事件循环内联同步调 LLM):会让确定性路由器长出 LLM 副作用、破坏 replay CI、给每次 emit 注入不确定延迟。明确排除。
- 不把 `precheck` 块加进任何出厂 builtin preset(保零回归);只提供机制 + 测试 fixture/示例。
- 不做 human-in-the-loop 人工审批(默认判决者是自动 LLM gate hat);人工审批是后续可选项。
- 不承担机械检查职责:字段/ git / task / step 允许集仍归现有确定性门(R11)。

---

## Key Decisions

- **B1(脱糖成合成 gate hat)而非 B2(内联 runtime judge)**:复用两处既有事实——LLM-as-judge 以 hat 形式落地(review 维度 reviewer)、运行时合成 hat 的先例(`add_builtin_ralph`);引擎零改动,replay 天然兼容。
- **候选 + 关卡 = 硬保证的唯一形状**:先有 `X.proposed` 候选,才谈得上检查;真 `X` 在判过前不生效。软保证(producer 自查)不可靠,排除。
- **失败必须有界 + 升级**:直接复用 `repair_budget` / rejection / `plan.blocked` 现成机制,防止重建历史 stall bug。
- **关键字开启 + 严格 no-op**:`precheck.enabled` 是唯一开关;未启用时脱糖不触碰任何配置,保证零回归。

---

## Dependencies / Assumptions

- 复用现成件:`RalphConfig::normalize`(脱糖落点)、`HatRegistry::add_builtin_ralph`(合成 hat 先例)、`InstructionBuilder::build_custom_hat`(instructions 生成)、`rejection.rs` / `LintResumeHint`(失败反馈)、`mechanism.repair_budget`(预算)、schema SSOT + `build.rs` merge(声明传播)。
- 假设(计划阶段核实):被守 topic 的多 producer 场景由"全部 producer 改发 `X.proposed` + 单 gate 消费"覆盖;`on_fail.target` 显式指定消解打回歧义。
- 依赖 AGENTS.md 的下游同步硬规则(schema/manifest/index/presets.rs/zsh/preset_lint/BDD)与 nextest 测试入口规则。

---

## Outstanding Questions

### Resolve Before Planning
- (无)核心产品决策已在本轮讨论锁定。

### Deferred to Planning
- [Affects R2][Technical] `precheck` 块在 SSOT 里的确切 schema 形状,以及 `build.rs` merge 到 `event_loop.precheck` 的映射。
- [Affects R3][Technical] gate hat 硬门用 `terminal_events` 还是 `obligations` 强制"二选一",与现有 obligation gate 如何衔接。
- [Affects R6][Technical] `X.proposed` / `X.rejected` 的 topic 命名、schema `required_fields`、dedup key、以及 EventOriginGuard `can_publish` 授权。
- [Affects R7][Technical] `retry_budget` 复用 `repair_budget` 还是独立计数,预算 key 如何与 `stall_recovery_counts` 对齐。
- [Affects R10][Needs research] 零回归 golden 断言的口径(embedded 配置 byte-equality vs HatRegistry 结构等价)。

---

## Next Steps

-> 见配套实施计划 `docs/plans/2026-07-02-004-feat-event-emit-precheck-prompt-gate-plan.md`(本文档 origin)。
