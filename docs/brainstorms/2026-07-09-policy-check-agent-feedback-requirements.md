---
date: 2026-07-09
topic: policy-check-agent-feedback
related:
  - docs/achieved/brainstorms/2026-06-02-payload-contract-validation-requirements.md
  - docs/brainstorms/2026-07-02-event-emit-precheck-prompt-gate-requirements.md
  - docs/brainstorms/2026-07-06-ce-executor-serial-protocol-ssot-convergence-requirements.md
  - docs/guide/payload-contracts.md
---

# Policy-Check Agent 可修复反馈需求文档

## Problem Frame

Ralph 现在已经有 payload contract、`event_policy.schemas`、`ralph emit --policy-check`、schema-aware emit 示例和 strict lint。它们能挡住很多错误：缺字段、非法 topic、allowed values 不匹配、部分 handoff 对齐问题。

但对 agent 来说，当前反馈仍偏“机器拒收”：它经常知道“错了”，却不一定清楚“这个 topic 退出时到底该填什么、每个字段业务含义是什么、应该从本轮工作结果里取哪个值、如何改 payload 后重试”。这会让 agent 在 emit 终态/业务事件时继续猜字段，尤其是多 hat loop 里，payload 是下一跳能否稳定工作的交接合同。

本需求要增强的是 **policy-check 的可解释性和 prompt 里的可填写性**：让 agent 在 emit 前就知道要填什么，在 policy-check 失败时得到可直接修复的结构化反馈。它不改变事件流、不新增路由系统、不让 runtime 代替 agent 编造业务事实。

## Feedback Flow

```text
hat 准备 emit
  |
  v
prompt 中已有 schema-aware 字段说明和示例
  |
  v
agent 运行 ralph emit <topic> --policy-check --json '<payload>'
或 ralph wave emit --policy-check --payloads-stdin
  |
  +-- 通过 -> 去掉 --policy-check 正式 emit
  |
  +-- 失败 -> 返回字段级错误、含义、允许值、示例和重试命令
              |
              v
            agent 修 payload 后重新 policy-check
```

## Requirements

**Schema 语义增强**

- R1. Event schema 必须能表达字段级 agent 说明：对每个 required field，可以声明该字段的业务含义、来源提示和填写约束，供 prompt 与 policy-check 错误反馈复用。
- R2. 字段级说明必须是可选增强；旧 schema 只声明 `required_fields` 时，现有校验行为和序列化行为保持不变。
- R3. Schema 必须能声明 topic 级示例 payload 或示例值策略，用于生成更接近真实业务语义的 emit 示例；没有示例时继续使用现有占位符生成逻辑。
- R4. Schema 语义元数据不得成为第二套校验权威。字段是否必填、允许值是否合法，仍以现有 `required_fields`、`allowed_values`、`hat_allowed_values`、`element_constraints` 等机器约束为准。

**Policy-Check 失败反馈**

- R5. `ralph emit --policy-check` 和 `ralph wave emit --policy-check` 在缺 required field 时，必须明确输出：topic、缺失字段、字段说明、当前 payload 摘要、期望 payload 形状和下一步重试方式。
- R6. `ralph emit --policy-check` 和 `ralph wave emit --policy-check` 在字段值不合法时，必须明确输出：字段路径、当前值、允许值集合、字段说明和修复提示。
- R6a. 当 payload 类型、数组元素 shape、条件必填或非空约束不合法时，反馈必须说明是哪一层 shape 失败、期望形状是什么、当前值为何不满足，而不是只返回通用 schema rejection。
- R7. 若同一次 payload 有多个字段错误，反馈必须聚合展示，不得只报第一个错误后让 agent 反复试错。
- R8. JSON 输出模式必须提供稳定、机器可解析的错误结构，至少能表达 `code`、`field`、`message`、`expected`、`actual`、`field_description`、`suggested_payload_shape` 或 `suggested_command` 这类信息；其中 payload suggestion 只能表达结构和占位符，不能伪造业务事实。
- R9. 文本输出模式必须适合 agent 直接阅读，避免只返回内部 reason code；reason code 可以保留，但必须配套人类可读修复说明。
- R10. policy-check 不得自动补齐业务字段。缺少 `must_fix_now_count`、`verdict`、`synthesized_review_file` 等字段时，只能提示 agent 根据本轮产物重新计算或填写，不能替 agent 猜默认值。

**Prompt 中的可填写性**

- R11. Hat prompt 的 publish section 必须利用 schema 语义元数据展示每个可 publish topic 的必填字段、字段说明、允许值和示例 payload。
- R12. Prompt 只能展示该 hat 有权 publish 的 topic 形状，继续遵守现有 hat-scoping 约束，不能泄漏其它 hat 的 payload 合同。
- R13. Prompt 中的 emit 示例必须继续强调先跑 `--policy-check`，通过后再正式 emit；不能弱化现有强预检规则。
- R14. 当 schema 缺少字段说明时，prompt 仍应退回到现有 required field 示例，不应导致 prompt 构建失败。
- R14a. 如果 hat 使用 batch/wave emit，prompt 必须说明每个 payload item 都按同一 topic schema 校验，并在失败时能定位到具体 payload index。

**Agent 使用路径与文档注入**

- R14b. Agent 必须能从注入 skill 文档中学会这套流程：先查看本 hat 可 publish topic 的 schema-aware emit 提示，按字段说明填写 payload，运行 `--policy-check`，根据字段级错误修正，再正式 emit。
- R14c. `crates/ralph-core/data/ralph-tools-emit.md` 必须新增或更新 agent-facing 章节，说明 policy-check 失败反馈的读取方法，包括 `code`、`field`、`expected`、`actual`、`field_description`、`suggested_payload_shape`、`suggested_command` 等字段的含义，以及 agent 收到这些字段后下一步该做什么。
- R14d. `crates/ralph-core/data/ralph-tools-cmdref.md` 或等价命令速查文档必须同步展示新版 `ralph emit --policy-check` / `ralph wave emit --policy-check` 的最小正确用法，避免 agent 只知道旧命令形状。
- R14e. Builtin preset 的 hat `instructions:` 涉及 emit、policy-check 或 payload 字段填写时，必须引用注入 skill 文档中的章节，不得复制完整命令规则或字段解释；具体 topic 的业务字段含义来自 schema-aware prompt section。
- R14f. Prompt builder 生成的 schema-aware publish section 必须是 agent 当轮可直接执行的说明：列出该 hat 可发 topic、必填字段、字段说明、允许值、示例 payload、policy-check 命令和失败后的修复动作。
- R14g. 若某个 preset instruction 仍要求 agent 手写 payload，但没有引用新版 emit skill 或没有依赖 schema-aware publish section，strict lint 应能提示 preset 作者补齐引用；第一版可先针对 builtin/high-risk preset enforce。

**SSOT 与作者体验**

- R15. Builtin preset 的字段说明应优先写在 `presets/schemas/<name>.yml` 这类 schema SSOT 中；inline schema 仅作为覆盖层或没有 sibling schema 的临时路径。
- R16. Strict lint 应能发现明显的 schema 说明漂移：例如 required field 没有字段说明时，可在严格模式下提示 preset 作者补齐，但第一版不应强制所有旧 preset 一次性补全。
- R17. 对高风险 topic，尤其是终态事件、handoff 事件、review/fix loop 事件，需求要求补充字段说明和示例，优先试点 `ce-executor-pipeline-loop` 中近期新增的 review/fix 收敛 topic。

**非回归**

- R18. 未配置字段说明和示例的 preset，policy-check 的接受/拒绝结果必须与当前行为一致。
- R19. 本需求不得改变 topic routing、hat trigger 规则、business event budget、terminal event 语义、step handoff 语义或 event loop 状态机。
- R20. 错误反馈中不得泄漏绝对 workspace 路径或其它无关内部 ledger 路径；需要定位时使用已有的安全摘要或 repo-relative 信息。
- R21. 若实现改变 agent 可见的 emit/policy-check 输出或操作流程，必须同步更新注入给 agent 的 `crates/ralph-core/data/ralph-tools-emit.md` 和相关命令速查文档；未同步文档视为功能未完成。

## Success Criteria

- SC1. Agent 对一个缺 required field 的 payload 运行 `--policy-check` 后，能从输出中直接知道缺哪个字段、字段是什么意思、应如何重试。
- SC2. Agent 对一个 allowed value 错误的 payload 运行 `--policy-check` 后，能看到允许值集合和当前错误值。
- SC2a. Agent 对一个 payload type 或数组元素 shape 错误运行 `--policy-check` 后，能看到失败字段路径、期望形状和当前值摘要。
- SC3. JSON 输出模式可以被后续 agent 或工具稳定解析，不依赖自然语言字符串匹配。
- SC4. Hat prompt 中的 emit section 对试点 topic 展示字段说明和示例，agent 不需要回头读 preset 才知道退出 payload 怎么填。
- SC4a. Agent skill 文档中存在明确流程：如何读 schema-aware emit section、如何运行 policy-check、如何根据新版错误反馈修 payload、何时正式 emit。
- SC4b. 试点 preset 的 relevant hat instructions 只引用新版 emit skill 和 schema-aware prompt section，不重复复制字段说明；字段解释由 schema SSOT 驱动展示。
- SC5. 未采用新字段说明的旧 preset 行为不变；既有 payload contract、policy-check、strict lint 测试不因本增强产生语义回归。
- SC6. `ce-executor-pipeline-loop` 的 review/fix 收敛事件可以作为示例：`must_fix_now_count`、`residual_findings_count`、`synthesized_review_file`、`review_round` 等字段在 prompt 和 policy-check 错误中都有清晰说明。

## Scope Boundaries

- 不做 payload routing 或按 payload 自动改下一跳 prompt 的完整路由系统；本轮只增强 emit 前后的反馈。
- 不做自动修复 payload；runtime 不替 agent 填业务事实。
- 不把 schema 升级成完整 JSON Schema DSL；优先补 Ralph 当前事件合同需要的字段说明、示例和错误反馈。
- 不改变已有事件是否被接受的判定标准；只改变可解释性和提示质量。
- 不改变 wave 调度、batch fan-out 或 worker 聚合语义；只让 batch/wave policy-check 的错误定位同样可修复。
- 不要求每个 preset instructions 手工解释所有 payload 字段；字段解释应来自 schema SSOT 和 prompt builder，instructions 只负责引用使用流程。
- 不要求所有 builtin preset 在第一轮全部补齐字段说明；先支持机制和高风险试点 topic。

## Key Decisions

- 字段说明进入 schema SSOT：payload 合同本来就是事件交接的合同，字段含义也应跟合同放在一起，避免 prompt instructions 里散落重复解释。
- Policy-check 只做可修复反馈，不做业务推断：缺字段通常意味着 agent 没从产物里提取事实，runtime 无权替它编值。
- 先增强现有机制，不引入路由系统：当前最痛的是 agent 不知道怎么填 emit payload；routing hints 是下一阶段能力，不能混进第一版扩大回归面。
- 文本与 JSON 双输出都要好：文本服务 agent 直接阅读，JSON 服务后续工具、diagnose、可能的自动重试或 UI 展示。
- Agent adoption 必须通过 skill + prompt builder 双入口完成：skill 教通用流程，prompt builder 给当轮 topic 的具体字段合同，preset instructions 只做引用，避免三处复制后漂移。

## Dependencies / Assumptions

- 当前 `EventSchema` 已覆盖机器校验所需的 required fields、allowed values、hat-aware allowed values 和 array element constraints；本需求是在其上增加 agent-facing metadata。
- 当前 `PolicyCheckReport` 已有 reason codes 和 suggestions；计划阶段需要判断是在现有响应上扩展，还是引入更明确的字段级 error envelope。
- 当前 `emit_schema_hint` 已经为 prompt 和 CLI fix hint 生成 schema-aware emit 示例；本需求应复用这条路径，避免 prompt 示例和 CLI 错误反馈漂移。
- 当前 builtin preset schema SSOT merge 已存在；试点 preset 若新增 `presets/schemas/<name>.yml`，必须遵守 schema parity 和 builtin preset 同步规则。
- 项目硬规则要求 agent 可见能力变化必须同步 `crates/ralph-core/data/*.md` 注入文档；本需求改变了 agent 使用 `policy-check` 的方式，因此文档同步是交付的一部分。

## Outstanding Questions

### Resolve Before Planning

- 无。核心产品方向已确定：先增强 policy-check 错误反馈和 prompt 可填写性，不做 payload routing。

### Deferred to Planning

- [Affects R1][Technical] 字段说明的最终 YAML 形状如何命名，才能既清晰又不与现有 `allowed_values`、`element_constraints` 混淆。
- [Affects R8][Technical] JSON 输出是扩展现有 `PolicyCheckReport`，还是新增版本化 envelope，以避免破坏已有消费者。
- [Affects R16][Technical] required field 缺字段说明在 strict lint 中第一版应是 warning 还是仅针对试点 preset enforce。
- [Affects R17][Technical] `ce-executor-pipeline-loop` 是否先迁移到 sibling schema SSOT，再补字段说明；还是先在 inline schema 试点。
- [Affects R14g][Technical] preset instructions 引用新版 emit skill 的 lint 应检测哪些短语或章节锚点，才能避免误报和复制 skill 内容。

## Next Steps

-> `/ce:plan` for structured implementation planning.
