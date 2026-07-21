---
date: 2026-07-18
topic: ralph-cross-project-runtime-bootstrap-skill
---

# Ralph 跨项目运行套件 Skill 需求

## Problem Frame

Ralph 已有用于起草和评审 preset 的 operator skills，但“把一个已有 preset 带到任意目标项目、补齐项目级运行上下文并确认 loop 真正跑得起来”仍缺少独立、完整的工作流。用户目前需要手工理解目标项目、编写 `AGENTS.md` / `CLAUDE.md`、准备 Ralph 配置与 prompt、拼装启动参数并排查首次运行问题。这使一个正确的 preset 仍可能因为项目侧配置缺失、命令不匹配或约束漂移而无法稳定运行。

需要新增一个跨项目可调用的 skill。它在用户当前所在的目标项目中工作，以用户已创建或明确指定的 preset 为输入，生成合理的项目运行套件，并通过实际预检与受控 smoke run 证明 Ralph loop 可以启动和推进。与此同时，现有 `skills/ralph-hats` 必须被彻底删除，不迁移其职责、不保留兼容入口。

## Requirements

**职责与调用边界**

- R1. 新 skill 必须可从任意目标项目目录调用，不得假设目标项目是 `ralph-orchestrator`、使用某一种编程语言，或已有 Ralph 专用文件。
- R2. 新 skill 的输入必须是用户已创建或明确指定的 preset；它负责项目侧落地与运行，不负责设计、创建或重写 preset。
- R3. 新 skill 必须先读取目标项目的真实上下文，包括现有项目指令、技术栈、构建与测试入口、版本控制状态以及与 Ralph 运行有关的已有文件，再决定需要创建或更新什么。
- R4. 当缺少必需输入，或继续操作会覆盖无法安全合并的项目规则时，skill 必须停止并向用户说明具体缺口，不得凭空生成关键事实。

**项目运行套件**

- R5. 新 skill 必须能够创建或完善目标项目的 `AGENTS.md` 与 `CLAUDE.md`，使 Ralph 中的 agent 能获得准确、可执行、面向该项目的工作规则；不得用与目标项目不符的通用模板覆盖已有有效规则。
- R6. `AGENTS.md` 与 `CLAUDE.md` 的关系必须由目标项目现状决定：已有同步约束时保持同步；没有同步约束时仍需保证两者不存在相互矛盾的执行要求。
- R7. 新 skill 必须生成或完善项目级 Ralph 配置（典型产物为 `ralph.pipeline.yml`），覆盖所选 backend、运行时限、iteration 上限、prompt 入口、诊断能力以及目标项目所需的 guardrails；配置值必须来自目标项目事实、preset 契约或明确的用户选择。
- R8. 新 skill 必须生成或完善与配置匹配的执行 prompt（典型产物为 `PROMPT.pipeline.md`），清楚连接目标任务或 plan、目标项目约束与指定 preset，但不得复制或改写各 hat 自身的职责协议。
- R9. 如果目标项目还需要其他文件才能可靠启动，skill 可以生成最小必要的配套产物，但必须解释每个产物为何必需，并避免引入与运行无关的脚手架。
- R10. 对已存在的套件文件，skill 必须采用审慎的合并或更新策略，保留项目特有规则；任何可能造成语义丢失的替换都必须先获得用户确认。

**验证与正式运行交付**

- R11. 新 skill 必须探测已安装 Ralph CLI 的版本与实际可用能力，并通过该 CLI 的真实 preset check/lint 与配置解析路径校验 preset 契约兼容性、preset 路径、项目配置、prompt 引用、backend 可用性、目标项目命令以及配置合并后的有效行为，不能以“CLI 或文件存在”作为完成标准。若发现字段、命令或事件契约不兼容，必须将其报告为版本/能力阻塞并停止，不得擅自修改 preset。
- R12. 新 skill 必须执行不会承担完整业务任务的受控 preflight 和 smoke run，以证明 Ralph 能读取目标配置与 preset、启动 loop、进入预期执行路径并产生可诊断结果。
- R13. smoke run 必须有明确的时间、iteration 或任务边界，不能意外启动长时间自治执行；若任何验证可能产生显著成本、外部副作用或不可逆操作，必须先取得用户授权。
- R14. 验证失败时，skill 必须基于实际输出定位项目套件、preset 输入、backend 环境或目标项目命令中的阻塞点；只修复项目运行套件职责范围内的问题，不擅自修改 preset 或业务代码。
- R15. 完成时必须交付一条可重复使用的正式启动命令，明确其配置、preset 以及 plan/task 输入，并汇总已创建或更新的文件、验证结果和仍需 operator 知道的限制。
- R16. 对使用 worktree 的正式启动命令，必须遵循 Ralph 的显式复用键要求，使用明确的 plan 或 worktree name，不得从自然语言 prompt 猜测复用目标。

**安全性与幂等性**

- R17. 新 skill 必须把用户当前目录视为目标项目根进行确认，所有生成的持久文件使用项目相对路径；不得把 `ralph-orchestrator` 仓库内的本机路径或临时运行状态写入目标项目配置。
- R18. 新 skill 必须尊重目标项目已有的 `AGENTS.md`、版本控制状态和测试规则，不得删除无关改动、提交用户未授权的内容、推送远端或创建额外 worktree。
- R19. 对同一项目与同一 preset 重复运行时，skill 必须能够审计和更新既有套件，而不是无条件复制出重复文件；无变化时应给出清晰的 no-op 结果。
- R20. 生成的项目指令与 prompt 必须面向 agent 可执行行为，说明触发条件、应运行的命令、信息来源和失败停止条件，避免依赖 agent 看不到的 Ralph 内部 ledger 或实现细节。

**彻底删除 `ralph-hats`**

- R21. 必须删除整个 `skills/ralph-hats` skill 及其所有随附元数据和资源。
- R22. 必须清除当前有效代码、安装器、技能清单、说明文档和项目指令中对 `ralph-hats` 或 `skills/ralph-hats` 的全部引用，确保它不再可发现、安装或调用。
- R23. 不得迁移、合并或重新暴露 `ralph-hats` 原有职责，也不得提供 alias、shim、deprecated wrapper 或兼容提示；删除后的 preset 起草、preset 评审和新项目运行套件保持各自明确边界。
- R24. 历史归档、既有运行报告或不可变历史记录不要求改写；除这些明确的历史材料外，仓库的有效内容搜索不得残留 `ralph-hats` 引用。

## Success Criteria

- 在一个没有 Ralph 项目套件的外部示例项目中，用户指定已有 preset 后，新 skill 能依据该项目事实生成完整且相互一致的运行文件。
- 在一个已有 `AGENTS.md`、`CLAUDE.md` 或 Ralph 配置的项目中重复运行，新 skill 能保留有效规则、指出冲突并进行幂等更新。
- 生成后的 preflight 与有界 smoke run 能证明配置和 preset 被实际加载，失败时能给出可操作的阻塞原因。
- 最终启动命令可以由用户在目标项目中再次执行，无需重新猜测配置、preset 或 plan 的连接方式。
- `skills/ralph-hats` 目录消失，非历史有效内容中不存在其安装入口、清单项、文档链接或调用说明。

## Scope Boundaries

- 不设计、创建、修复或评审 preset；这些仍由 preset author/review 工作流负责。
- 不接管 `ralph-hats` 的用户 hat collection 创建能力，也不提供替代兼容层。
- 不负责修改目标项目的业务代码来让 smoke run 通过。
- 不自动安装系统级依赖、backend CLI 或凭据；可以检测并报告缺失项，安装需用户另行授权。
- 不默认执行完整、长时间或高成本的 Ralph 自治任务；正式运行由用户使用交付命令启动，除非用户明确要求当场运行。
- 不把某个特定 preset、backend、语言或测试框架固化为唯一支持路径。

## Key Decisions

- 新增独立的跨项目运行套件 skill：preset 的正确性和项目运行环境的正确性是两个不同问题，应有清晰的职责边界。
- 采用“实际可运行”而非“生成模板”作为完成标准：必须包含配置验证与有界 smoke run。
- 项目指令属于运行套件：`AGENTS.md` 与 `CLAUDE.md` 会直接决定 loop 内 agent 的行为，不能被当作可选文档。
- `ralph-hats` 彻底删除且不迁移：用户明确不需要保留其能力或兼容性，避免继续维护重叠概念。
- 新 skill 暂以 `ralph-project-bootstrap` 作为工作名称；最终名称可在规划时结合现有 skill 命名与触发描述确认。

## Dependencies / Assumptions

- 用户在调用时位于目标项目中，并能指定一个现存、可读取的 preset。
- Ralph CLI 已可执行；若不可执行，新 skill 只诊断并请求用户处理或授权安装。
- smoke run 所需 backend 已具备可用凭据，或存在项目允许的本地/mock/replay 验证路径。
- 规划阶段需要核对 Ralph 当前 CLI 的 preflight、preset check、run、worktree 与配置优先级，以选择真实支持的验证命令。

## Outstanding Questions

### Deferred to Planning

- [Affects R12][Needs research] 当前 CLI 中哪组命令能够以最低成本证明“配置 + preset + prompt + backend”完整装载，同时保证不会进入长时间业务执行？
- [Affects R5-R10][Technical] 新 skill 应携带哪些最小模板或检查表，才能适配不同项目而不把本仓库的 pipeline 参数硬编码到外部项目？
- [Affects R24][Technical] 清理验收应如何区分有效内容与历史归档，避免为了字符串清零而篡改历史报告？
- [Affects R19][Technical] 如何对自动生成段落建立稳定 ownership，使重复运行可以安全更新，同时保留用户手写内容？

## Next Steps

-> `/ce:plan` 制定结构化实施计划。
