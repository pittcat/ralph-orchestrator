---
date: 2026-07-07
topic: ralph-single-chain-execution-primary
status: draft
related:
  - docs/brainstorms/2026-07-02-ce-executor-pipeline-preset-requirements.md
  - docs/achieved/brainstorms/2026-07-06-ce-executor-serial-protocol-ssot-convergence-requirements.md
  - docs/brainstorms/2026-07-05-ralph-preset-author-review-skills-requirements.md
  - docs/report/2026-07-07-ce-executor-serial-primary-20260707-110748-diagnosis.md
origin: 对话收敛 — serial 机制复杂度反噬；Ralph 主线转向单链执行；沉淀可复用机制并删除无效复杂度
---

# Ralph 单链执行主线与 serial 机制清算 — 需求文档

## Problem Frame

过去一段时间对 `ce-executor-serial` 的尝试证明了一件事：对业务交付来说，**单链执行**比重型编排更稳定。`serial` 想保留 unit-by-unit、validator、fixer、review、shipper 等多段职责，但为了让这些职责协作，引入了 tasks/progress、phase authority、handoff envelope、progress-steward、shipper reason whitelist、stall recovery、terminal guard 等多层机制。结果不是更稳，而是出现了多套状态源互相矛盾、fallback 绕过主链、blocked 被翻译成 success、terminal 后继续业务 emit 等复发问题。

`ce-executor-pipeline` 的稳定性来自相反方向：一条显式链路、一个事件一个消费者、一个阶段一个决策者、没有 LLM 救场 coordinator、没有 shipper 把机制兜底翻译成成功。用户进一步确认：即使要做“一个 unit 一个 subagent”，也可以由单链 executor 在自己的 activation 内部分配 subagent；这仍然是单链，不需要 Ralph runtime 做多角色状态机。

因此，本需求改变方向：**Ralph 后续主线做单链执行，不再把 `ce-executor-serial` 作为主力执行模型继续修补**。`serial` 过程中沉淀出的有价值机制可以迁移到单链模型；没价值或制造复杂度的机制应删除、停用或降级为诊断。

## Target Model

主力执行模型是单链：

```text
work.start
  -> plan-reviewer 或 executor
  -> executor（可在内部按 unit 分配 subagent，但对 Ralph 只 emit 一个 work.done / work.failed）
  -> dimension review 串行链
  -> review-synthesizer
  -> fixer（按修复计划执行）
  -> alignment
  -> reporter
  -> LOOP_COMPLETE
```

如果 executor 需要 unit-by-unit 执行，它在自己的职责范围内完成：

```text
executor activation
  - 读 plan units
  - 为每个 unit 分配一个 subagent 或内部步骤
  - 主 executor 负责验收、提交、记录证据
  - 所有 unit 通过后只向 Ralph emit work.done
  - 任一关键 unit 阻塞则 emit work.failed
```

Ralph runtime 不再为每个 unit 建立独立的跨 hat 编排状态机。unit 状态是 executor 内部执行证据，不是 runtime 主链的多阶段事实源。

## Requirements

### 主线定位

- **R1.** Ralph 的计划执行主线应以单链 preset 为主，`ce-executor-pipeline` 或其后继形态成为默认推荐执行模型。
- **R2.** `ce-executor-serial` 不再作为主力执行模型继续叠加修复；后续只能作为实验、兼容或待弃用对象存在。
- **R3.** 新功能优先增强单链执行能力，而不是继续增强 serial 的 coordinator/validator/fixer 跨 hat 状态机。
- **R4.** “一个 unit 一个 subagent”应建模为 executor 内部执行策略，而不是 Ralph runtime 的多 hat unit loop。

### 单链编排原则

- **R5.** 每个业务 topic 必须有唯一消费者；需要串行多维 review 时，通过显式 topic 串成一条链，而不是同一事件多消费者。
- **R6.** 每个阶段只有一个决策者：executor 决定执行是否完成，review-synthesizer 决定 review 汇总，fixer 决定修复计划是否执行，reporter 决定最终报告与终态。
- **R7.** fallback 只能 fail-close，不能推进成功链。沉默、超时、policy 拒收、handoff 失败只能产生 blocked/failed 或诊断，不得产生 pass/pass_with_residuals。
- **R8.** terminal 一旦 requested 或 honored，业务事件不得重新进入主链；重复 terminal 或后置业务事件只能记诊断。
- **R9.** 单链 preset 不应依赖 tasks/progress 作为业务事实源。需要任务列表时，作为 executor 内部计划或报告证据，不作为 runtime 路由依据。

### 从 serial 沉淀并保留的机制

- **R10.** 保留 `event_policy`、payload schema、topic ownership、hat scope/origin guard 这类边界机制；它们防止越权与坏 payload，不制造第二业务事实源。
- **R11.** 保留 `--policy-check` / 统一 emit 响应方向。agent 应能从一次 emit 结果知道是否写入、失败原因、允许下一步，而不是读 recovery 散文。
- **R12.** 保留 terminal guard，并把它作为全 preset 通用安全机制加强；它不属于 serial 专用复杂度。
- **R13.** 保留 diagnostics / recovery log 作为观测资产，但它们只能用于诊断和报告，不得参与成功路径判定。
- **R14.** 保留单链多维 review 的产物化经验：每个维度一个明确产物，synthesizer 统一汇总，fixer 只执行 fix-plan。
- **R15.** 保留 alignment 关作为报告前核对，但 alignment 只能记录 residual，不回环、不救场、不改变主链事实。

### 从 serial 删除、停用或降级的机制

- **R16.** 删除或停用 `progress-steward` 式 LLM 救场。Ralph 不应在 loop 卡住时再唤醒一个 agent 猜测下一步。
- **R17.** 删除或停用 `phase_authority` 对 serial unit loop 的复杂路由职责。单链 preset 可有简单链路校验，但不需要 runtime 管每个 unit 的跨 hat phase。
- **R18.** 删除 shipper reason whitelist 的“恢复为成功”语义。机制兜底 reason 不得被翻译成 pass/pass_with_residuals。
- **R19.** 删除 `default_publishes` / stall / ForcePlanBlocked 进入成功终态的路径。它们只能导向 blocked/fail 或诊断。
- **R20.** 降级 tasks/progress/state_projection：如果保留，只作为 agent 便利视图或报告证据，不再作为主链路由权威。
- **R21.** 删除 prompt 中围绕 serial phase、allowed topic、recovery reason 的大段 HARD RULE；这些规则要么内化为 runtime 校验，要么写入通用 skill 文档，要么删除。
- **R22.** 停止为 serial 新增专用 runtime gate，除非该 gate 能被单链 preset 复用且不制造第二状态源。

### 单链 executor 的 unit 支持

- **R23.** 单链 executor 必须能读 plan 中的 Implementation Units，并按 unit 组织内部执行、测试、提交和证据记录。
- **R24.** executor 可以为每个 unit 分配 subagent，但主 executor 必须负责最终验收与唯一 `work.done` / `work.failed` emit。
- **R25.** executor 的 `work.done` payload 必须包含足够证据让后续 review/report 知道 unit 覆盖情况，例如 unit count、commit count、tests run、executor head sha、残留摘要等；具体字段由后续 plan 对齐 schema。
- **R26.** 任一关键 unit 未完成时，executor 不得 emit `work.done`；应 emit `work.failed`，由 reporter 生成 blocked 报告。
- **R27.** unit 级详细状态应落在 executor 产物或报告附件中，而不是 runtime 主链中产生 `unit.ready/unit.done/unit.validated` 等跨 hat topic。

### 审核 skill 加强

- **R28.** `ralph-preset-review` 必须新增“单链优先 / serial 复杂度清算”审计维度，判断 preset 是否偏离单链原则。
- **R29.** 审核 skill 必须能标记以下结构风险：
  - 一个业务 topic 多消费者；
  - runtime 为 unit loop 管理多阶段跨 hat 状态；
  - fallback 可达成功终态；
  - blocked/failed reason 可被翻译为 pass；
  - tasks/progress/recovery 与 events 同时充当业务事实；
  - prompt 要求 agent 理解内部 ledger、phase authority 或其它 hat 行为；
  - LLM 救场 hat 能改变业务链路。
- **R30.** 审核 finding 必须给出处理建议：迁移到单链 executor、保留为通用边界机制、降级为诊断、删除、或保留为实验 preset。
- **R31.** `ralph-preset-author` 必须引导作者优先设计单链 topic flow；只有在明确证明单链无法表达时，才允许提出复杂 runtime 编排。
- **R32.** skill 规则必须同步落到共享 references：`finding-rubric.md` 增加单链原则与 serial 复杂度风险映射，`author-checklist.md` 增加“能否由 executor 内部 subagent 解决”自检，`patterns.md` 增加 pipeline 正例和 serial 反例。

### 下游同步

- **R33.** 后续 plan 若弃用、重命名或改变 builtin preset，必须同步 `presets/manifest.yml`、`crates/ralph-cli/src/presets.rs`、`presets/index.json`、`scripts/ralph-zsh-plugin.zsh`、`AGENTS.md` / `CLAUDE.md`。
- **R34.** 后续 plan 若改变 agent 可见命令、emit 响应、topic、event schema 或 runtime 行为，必须同步 `crates/ralph-core/data/*.md`。
- **R35.** 后续 plan 若改变 preset 作者/审核者规则，必须同步 `skills/ralph-preset-author`、`skills/ralph-preset-review` 及共享 references。

## Success Criteria

- **SC1.** 新推荐执行路径能用单链 preset 跑完计划：执行、review、fix、alignment、report、LOOP_COMPLETE 均按单消费者链路出现。
- **SC2.** executor 可在内部按 unit 执行或分配 subagent，但 Ralph trusted events 主链不出现 unit loop 多阶段跨 hat状态机。
- **SC3.** fallback、stall、policy reject、executor failure 均不会产生成功终态；blocked/fail 报告清晰落地。
- **SC4.** `ce-executor-serial` 的后续状态被明确：弃用、实验保留、或兼容保留；不再作为默认推荐主线。
- **SC5.** `ralph-preset-review` 能静态审出 serial 类复杂度风险，并能建议“迁移到单链 executor 内部”而不是继续补 runtime gate。
- **SC6.** 从 serial 沉淀出的有价值机制被迁移或保留为通用能力；无价值的 serial 专用机制有删除/停用清单。

## Scope Boundaries

### 本次覆盖

- Ralph 执行主线从 serial 式重型 runtime 编排转向单链模型。
- 判断 serial 机制“保留 / 迁移 / 降级 / 删除”的需求标准。
- 单链 executor 如何承载 unit-by-unit 和 per-unit subagent。
- preset 审核 skill 如何加强，以防止后续重新引入 serial 类复杂度。

### 本次不覆盖

- 不在需求阶段决定具体删除哪些 Rust 类型、函数或模块。
- 不要求立即删除所有 serial 文件；具体弃用/移除节奏由后续 plan 评估。
- 不否定所有多 hat：review、fix、alignment、report 仍可多 hat 串行；反对的是 runtime 为 unit loop 引入多源状态和救场机制。
- 不把 subagent 禁掉；subagent 可以存在，但应由 executor 内部管理，而不是成为 Ralph runtime 主链 topic。

## Key Decisions

| 决策 | 理由 |
|------|------|
| **单链成为主线** | 实跑证明 pipeline 风格比 serial 重型编排稳定 |
| **unit subagent 放进 executor 内部** | 保留 unit 级并行/拆分能力，同时避免 runtime 多状态源 |
| **serial 不再继续补丁化修复** | 继续修 serial 会把失败转移到下一层机制 |
| **有价值机制迁移，serial 专用复杂度删除** | 不浪费已沉淀的 policy/check/diagnostics 经验，但清掉无效救场 |
| **审核 skill 作为防回归门** | 没有 author/review 规则，后续 preset 很容易重新长回 serial 复杂度 |

## Dependencies / Assumptions

- `ce-executor-pipeline` 已证明单链模型的稳定性优于 serial 式 unit runtime 编排。
- executor 内部 subagent 能满足“一个 unit 一个执行者”的业务诉求；Ralph runtime 不需要感知每个 unit 的生命周期。
- `serial` 中部分机制仍有价值，但价值在通用边界、诊断、agent 可见协议，而不是 serial 专用路由。

## Outstanding Questions

### Resolve Before Planning

（无。用户已确认 Ralph 后续主线做单链，并倾向去掉 serial 及其无效机制。）

### Deferred to Planning

- [Affects R1-R4][Technical] 新推荐 preset 是直接扶正 `ce-executor-pipeline`，还是创建 pipeline v2 / single-chain executor preset。
- [Affects R16-R22][Technical] serial 专用机制的删除顺序：先停用 builtin preset，还是先删除 progress-steward/shipper whitelist/phase authority 等机制。
- [Affects R23-R27][Technical] executor 内部 per-unit subagent 的证据格式与 `work.done` schema 字段。
- [Affects R28-R32][Technical] 审核 skill 先只更新规程与 references，还是同步新增 `preset_lint` 机械规则。

## Next Steps

-> `/ce:plan` 生成结构化实施计划。建议拆为三条并行但同一验收口径的工作流：

1. 单链执行 preset 扶正或 v2 化。
2. serial 专用机制清算与弃用路径。
3. `ralph-preset-author/review` 单链优先审计规则增强。
