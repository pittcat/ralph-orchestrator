---
date: 2026-07-09
topic: schema-backed-trigger-context
related:
  - docs/brainstorms/2026-07-09-policy-check-agent-feedback-requirements.md
  - docs/achieved/brainstorms/2026-07-02-event-routing-table-requirements.md
  - docs/achieved/brainstorms/2026-07-02-event-emit-precheck-prompt-gate-requirements.md
  - docs/achieved/brainstorms/2026-07-06-ce-executor-serial-protocol-ssot-convergence-requirements.md
---

# Schema-Backed Trigger Context 需求文档

## Problem Frame

多 hat 编排里，上游 hat 通常通过 JSON payload 把状态交给下游 hat。当前下游 agent 需要自己读完整 payload，再从 natural-language instructions 里判断“这些字段意味着我现在该做什么”。当 payload 字段越来越多时，这种模式会让 agent 容易漏读、误读或把不同类别的问题混在一起处理。

典型例子是 review/fix loop：`review.synthesized` payload 里可能同时包含 `must_fix_now_count`、`residual_findings_count`、`verdict`、`synthesized_review_file` 等字段。对 runtime 来说，这些只是合法 JSON；对下游 agent 来说，这些字段其实决定了它的任务视角：是准备接受、生成 fix plan、还是阻塞报告。如果这些分支只写在 hat instructions 的长段自然语言里，agent 很容易把 residual findings 当作 must-fix，或者忽略 `must_fix_now_count == 0` 的退出语义。

本需求要建立一个薄层能力：preset/schema 作者可以声明 trigger payload 中哪些字段应被提炼成下游 agent 的 **Trigger Context**，以及有限的 **Routing Hints**。runtime/prompt builder 在激活下游 hat 时，把这些字段和命中的 hint 注入 prompt。下游 agent 不再需要自己从原始 payload 里推断任务视角。

这不是事件路由表，也不是动态拓扑。已有 `event-routing-table` 需求解决“发生事件后谁是下一跳”；本需求解决“下一跳 agent 被激活后，应如何理解这次 trigger payload”。

## Trigger Context Flow

```text
upstream hat emits JSON payload
  |
  v
event policy validates payload shape
  |
  v
runtime selects downstream hat by existing routing/subscription rules
  |
  v
trigger-context builder reads declared summary fields + hints
  |
  v
downstream prompt receives:
  - compact trigger field summary
  - missing-field markers
  - matched routing hints as task guidance
  |
  v
agent acts from narrowed task context
```

## Requirements

**Trigger Context 声明**

- R1. Preset/schema 必须能为某个 trigger topic 声明 `summary_fields`：这些字段会从 trigger payload 中提取并注入给下游 hat。
- R2. `summary_fields` 必须引用 schema 中已声明的字段。字段可以来自 `required_fields`，也可以来自未来明确声明的 optional/known fields；不得引用完全未知字段。
- R3. Trigger Context 声明必须是可选增强；没有声明的 preset 行为与当前完全一致。
- R4. Trigger Context 必须基于已经通过 event policy 的 payload 构建，不得绕过现有 schema/policy-check。
- R5. 当 summary field 在 payload 中缺失时，prompt 必须显式显示 `<missing>` 或等价标记，不能默默省略，也不能把缺失当作默认值。
- R6. Trigger Context 只注入当前 activation 的 trigger payload 摘要，不得要求 hat 直接读取 `.ralph/events.jsonl`、`.ralph/supervisor.db` 或其它 runtime ledger。

**Routing Hints**

- R7. Preset/schema 必须能为 trigger topic 声明有限的 `routing_hints`：当 payload 满足条件时，向下游 agent 注入一段短任务指导。
- R8. 第一版 hint 条件只支持小而确定的谓词集合：字段等于某个值、字段不等于某个值、数字大于/等于/小于某个常量、字段存在、字段缺失。
- R9. Hint 条件只能读取 trigger payload 中的字段，不得读取文件、git 状态、任务状态、事件历史或调用 LLM。
- R10. Hint 输出必须是 agent-facing task guidance，例如“只为 `must_fix_now` findings 生成 fix units”；不得是 runtime 控制命令，也不得修改 topic routing、hat 权限或工具权限。
- R11. 多个 hint 命中时，第一版采用声明顺序全部输出；作者必须保持 hint 文案彼此兼容。strict lint 应能发现明显冲突的 hint 标签或重复互斥描述，但不要求做自然语言理解。
- R12. 没有 hint 命中时，runtime 仍注入 summary fields；不得改变下游 hat 激活、事件接受/拒绝或 loop 终态。

**Prompt 注入与 Agent 使用**

- R13. 下游 prompt 必须有稳定的 Trigger Context 区块，清楚列出：source topic、source hat（若可得）、payload summary fields、matched routing hints。
- R14. Trigger Context 区块必须短、结构化、可扫读，不能把完整 payload 原样塞进 prompt。
- R15. Matched hint 必须用“本轮你应该如何处理”的语言表达，避免让 agent 再次解释底层条件。
- R16. 如果某个 hat instructions 已经手写大量 payload if/else，试点 preset 应将其收敛为“引用 Trigger Context 区块 + schema/hints 提供具体分支”，避免自然语言规则和 schema hints 双写漂移。
- R17. Agent-facing skill 文档必须说明 Trigger Context 的读取顺序：先看 injected Trigger Context，再按 hat instructions 执行；不要重新从 events ledger 推断同一信息。

**SSOT 与校验**

- R18. Trigger Context 和 routing hints 的声明应优先放在 schema SSOT 或同等 protocol SSOT 中，而不是散落在多个 hat instructions 中。
- R19. Strict lint 必须检查 `summary_fields` 和 hint 条件引用的字段存在于对应 topic schema 中；未知字段应报错。
- R20. Strict lint 必须检查 hint 条件使用第一版允许的谓词集合；不允许任意表达式语言或字符串拼接执行。
- R21. Strict lint 必须检查 hint 的目标 topic 和 consuming hats 在拓扑上合理：只给实际订阅该 trigger topic 的 hats 注入，不泄漏给无关 hat。
- R22. 生成的 prompt 只允许展示当前 hat 当前 trigger 的 context；不得展示其它 hats、其它 trigger topics 的 payload shape 或 hint。

**试点场景**

- R23. 第一版应以 `ce-executor-pipeline-loop` 的 review/fix 收敛事件作为试点，至少覆盖 `review.synthesized -> review-gate` 和 `review.accepted/fix.requested -> downstream` 这一类分支。
- R24. 试点必须覆盖 `must_fix_now_count == 0`、`must_fix_now_count > 0`、`review_round >= max` 这类实际影响 agent 行为的 hint。
- R25. 试点必须明确 residual findings 的处理方式：当 hint 说明 residual 是 report-only 时，下游 agent 不应为它们生成 fix units。

**非回归**

- R26. 本需求不得改变 topic routing、hat trigger 匹配、event bus 队列、公平调度、event policy 接受/拒绝或 terminal event 语义。
- R27. 本需求不得动态选择不同 hat，也不得动态修改 subscribes/publishes。
- R28. 本需求不得替代 policy-check；payload 形状错误仍由 policy-check/schema gate 拦截。
- R29. 未声明 Trigger Context 的 preset，prompt 和 runtime 行为必须保持当前语义。

## Success Criteria

- SC1. 对声明了 `summary_fields` 的 trigger topic，下游 prompt 中出现稳定的 Trigger Context 区块，列出字段值和缺失字段标记。
- SC2. 当 `must_fix_now_count == 0` 时，下游 prompt 能直接显示“准备接受/残留只进报告”这类 task guidance，而不是要求 agent 自己推断。
- SC3. 当 `must_fix_now_count > 0` 时，下游 prompt 能直接显示“只处理 must_fix_now findings”这类 task guidance。
- SC4. 当 payload 缺少被声明的 summary field 时，prompt 显示 `<missing>`，且不把缺失推断为 `0`、`false` 或空字符串。
- SC5. Strict lint 能捕获 hint 引用未知字段、使用不支持谓词、或向非订阅 hat 泄漏 context 的配置错误。
- SC6. 未采用 Trigger Context 的 preset 行为不变。
- SC7. `ce-executor-pipeline-loop` 的试点能减少 relevant hat instructions 中手写 payload 分支判断，不复制 schema/hint 内容。

## Scope Boundaries

- 不做动态下一跳选择；已有 event routing / subscription 仍决定谁被激活。
- 不做任意表达式 DSL；第一版只支持有限、可静态校验的谓词。
- 不让 routing hints 调用工具、读文件、读事件历史或做 LLM 判断。
- 不让 hints 改工具权限、hat 权限、topic 权限或 policy-check 结果。
- 不把完整 payload 注入 prompt；只注入作者声明的 summary fields 和 matched hints。
- 不要求一次性迁移所有 preset；先试点高风险 review/fix loop。

## Key Decisions

- 命名采用 **Trigger Context / Routing Hints**，不采用 “Payload Routing Engine”。这样能避免实现者误以为要改 runtime 下一跳或动态拓扑。
- Hint 是 prompt guidance，不是 runtime command。runtime 只负责稳定提取和注入，业务执行仍由 agent 完成。
- 字段缺失必须显式显示，禁止默认值推断。对 agent 来说，`missing` 和 `0` 的业务含义完全不同。
- 声明放在 schema/protocol SSOT 中，instructions 只引用 Trigger Context 区块。这样字段意义、分支条件和 prompt 注入不会三处漂移。
- 第一版允许多个 hint 按声明顺序全部输出，不做复杂冲突求解。作者负责保持 hints 兼容，lint 只抓结构性错误。

## Dependencies / Assumptions

- 依赖 `policy-check-agent-feedback` 需求中的 schema 字段说明能力：Trigger Context 的字段摘要应复用同一套 schema metadata，不另造字段解释来源。
- 依赖现有 prompt builder 能识别当前 trigger payload 并注入 context。计划阶段需确认当前 isolated activation 的 trigger event 在 prompt 构建链路中可用。
- 依赖已有 preset lint 框架扩展字段引用检查。
- 假设第一版只覆盖 top-level 或 dot-path payload 字段；数组过滤、JSONPath、复杂聚合留给后续评估。

## Outstanding Questions

### Resolve Before Planning

- 无。核心产品方向已确定：不改下一跳，只把 trigger payload 提炼成下游 agent prompt context。

### Deferred to Planning

- [Affects R1][Technical] Trigger Context 声明最终放在 `event_policy.schemas.<topic>` 下，还是放在 sibling protocol block 中再 merge。
- [Affects R8][Technical] 第一版谓词的 YAML 形状如何表达，才能易读且易 lint。
- [Affects R11][Technical] 多 hint 命中时是否需要 `exclusive: true` 模式；第一版默认全部输出，计划阶段可评估是否要支持互斥组。
- [Affects R13][Technical] 当前 prompt builder 如何取得 source topic/source hat/trigger payload，是否需要补一层 trigger context data structure。
- [Affects R23][Technical] `ce-executor-pipeline-loop` 具体哪些 hat instructions 可以被 Trigger Context 替换，避免同时维护自然语言 if/else 和 schema hints。

## Next Steps

-> `/ce:plan` for structured implementation planning.
