---
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
---

# feat: 新增 ralph-task-discovery 前置任务发现 skill

## 0. 计划状态

**状态：READY**

本计划新增一个 Ralph 专用的前置 skill：`ralph-task-discovery`。它不直接生成 preset，也不替代 `ralph-preset-author`；它负责把“用户目标、项目事实、完成证据、候选执行方案”整理成结构化 `task-brief`，只有达到置信度门槛后才把 brief 交给 preset author。

本计划明确复用用户指定的外部 skill corpus，而不是重写一套通用访谈流程：

- `grilling`：逐个问题确认用户决策，事实由环境调查获得，决策由用户确认；
- `domain-modeling`：澄清术语、边界和具体场景，并检查用户描述与代码是否一致；
- `diagnosing-bugs`：bug 任务先建立能针对真实症状变红的反馈回路，再提出可证伪假设；
- `codebase-design`：使用 module / interface / seam / adapter / depth 词汇比较实现边界；
- `triage`：复用“查重复实现、验证请求、必要时 Grill、形成 agent-ready brief”的顺序；
- `wayfinder`：只吸收 fog、HITL/AFK、决策票据和阻塞边界思想；不从新 skill 内部调用这个 user-invoked skill；
- `grill-with-docs`、`to-spec`：不作为子调用入口；前者是 user-invoked wrapper，后者明确不进行访谈且面向 issue tracker 发布，均不符合本 skill 的直接 handoff 契约。

外部 skill 的用户指定来源为 `/Users/pittcat/Dev/agent_tools/skills`。该目录不属于本仓库，不在本计划中修改；新 skill 通过 adapter/reference 明确复用其方法，并在不可用时提供 Ralph-specific 最小降级规则，避免 arbitrary target project 因外部 corpus 未安装而静默失效。

**代码库基线：**分支 `pittcat-dev`，HEAD `74069d7436a71f1d7464c5f3e8a88dbbac74fbb1`。

**已调查范围：**

- `skills/ralph-preset-author/SKILL.md`、`references/author-checklist.md`、`references/commands.md`；
- `skills/ralph-preset-review/` 的 finding、AAF、prompt visibility 和 contract test 结构；
- `skills/ralph-project-bootstrap/` 的 public skill 目录、agent metadata、helper/test 组织方式；
- `skills/install.py`、`skills/README.md`、`.claude-plugin/marketplace.json`、`skills/tests/conftest.py`；
- 外部 `grilling`、`domain-modeling`、`diagnosing-bugs`、`codebase-design`、`triage`、`wayfinder` skill 全文相关段落；
- 现有 `docs/plans/2026-08-03-005-refactor-project-bootstrap-skill-plan.md` 的 public skill 编排、安装和测试模式。

**已执行的调查命令：**

- `rg --files`、`rg -n`、`sed`、`find`：确认 skill、reference、fixture、测试、安装和 manifest 入口；
- `git log`：确认近期 public skill 和 preset author/review 演进；
- `ralph` runtime 命令未执行：本计划不需要运行 Ralph loop，且 `ce-plan` 只负责调查和计划，不执行新行为。

**尚未执行：**本计划尚未新增文件或运行新增测试；最终验证命令见第 9 节。没有进行外部网络研究，因为需求是基于本仓库与用户指定本地 skill corpus 的组合设计。

**阻塞项：**无。所有进入实施计划的关键技术决策置信度均达到 `0.85`；尚未运行的行为验证属于实现阶段测试，不是计划前置阻塞。

## 1. 功能目标

### 1.1 业务目标与调用方

调用方是希望“针对任意项目和指定任务生成可执行 Ralph preset”的 operator/agent。调用方先提供目标项目 cwd 和自然语言任务；`ralph-task-discovery` 负责调查和收敛，`ralph-preset-author` 负责消费 brief 并设计 preset。

### 1.2 当前行为

当前 `ralph-preset-author` 已有强制 Discovery/user-confirmation gate，会读取仓库、询问用户意图、生成 Preset Intent Confirmation、AAF 表和 Payload Contract；但它的输入仍主要是当前对话和仓库状态，没有一个标准化的前置产物来统一表达：

- 用户真正确认的目标和非目标；
- 项目中已验证的入口、调用链、工具和限制；
- 什么证据可以证明任务完成；
- 哪些候选执行方案已经被验证或淘汰；
- 哪些决策因置信度不足必须停止。

外部 skill 已分别覆盖这些问题的局部方法，但它们的输出形状不同，且没有 Ralph author-ready 门禁。

### 1.3 目标行为

新增 `ralph-task-discovery` 后，完整调用链为：

```text
用户目标
  → 读取项目规则和代码事实
  → 复用 grilling / domain-modeling / 任务型专项 skill
  → 形成完成证据
  → 生成并比较至少一个必要的候选方案
  → 计算证据置信度和方案适配度
  → 低于阈值则调查、换方案或阻塞
  → author-ready task-brief
  → ralph-preset-author
```

当 `author_ready=true` 时，brief 必须包含可追踪的目标、项目事实、验收证据、决策记录、候选方案结论和剩余风险。任何关键维度低于阈值时，不得调用或推荐 preset author。

### 1.4 行为差异

- 已知项目事实必须由环境调查取得，不再反问用户；
- 影响业务结果、验收条件、权限范围、事实源、失败行为或人工确认的事项必须通过 `grilling` 风格逐题确认；
- bug 任务没有真实、可重复、能针对用户症状变红的反馈回路时，不能进入 author-ready；
- 重要执行方案需要有候选比较；低置信度或低适配度候选被标记为 `rejected`，不能被平均分掩盖；
- 只有每个关键置信度维度都达到 `0.85` 且硬门禁通过，brief 才能进入 `ralph-preset-author`；
- author skill 在收到 task brief 后复核其 `author_ready`、版本和项目根，不再把未确认的输入当作已确认事实；
- 所有失败都有下一动作：补调查、逐题问用户、换候选方案或输出 `BLOCKED`，不能以“先生成 preset 再说”收尾。

### 1.5 输入与输出

**输入：**

- 当前目标项目 cwd；
- 用户自然语言目标；
- 可选已有 issue/spec/plan 路径；
- 当前项目中的 `AGENTS.md`、`CLAUDE.md`、`CONTEXT.md`、ADR、构建配置、测试配置、CI 和相关源码；
- 可用的外部 skill corpus；
- 用户对关键决策的回答。

**业务输出：**

- `.ralph/task-discovery/<task-key>/task-brief.yml`：机器消费的 SSOT；
- `.ralph/task-discovery/<task-key>/evidence.md`：必要时保存长证据、调查摘要和被淘汰方案，避免把长内容塞进 brief 字段；
- 面向 operator 的摘要：状态、关键分数、未决问题、淘汰原因和交给 author 的路径。

**状态枚举：**

`draft`、`needs_investigation`、`needs_user_decision`、`blocked`、`author_ready`。

**author handoff：**

只有 `author_ready` 能交给 `ralph-preset-author`；其他状态只能返回具体阻塞和下一动作。author 收到的输入是 task brief 路径，不是复制进 prompt 的长文本。

### 1.6 置信度与方案评分契约

置信度表示“当前决策被证据支持的程度”，不是模型的主观自信；方案适配度表示“候选方案是否适合目标和项目”。两者不得合并成一个平均分。

**五个关键置信度维度：**

| 维度 | 含义 | author-ready 阈值 |
|---|---|---:|
| `goal_clarity` | 目标、范围、非目标和用户决策已确认 | `>= 0.85` |
| `project_fact_coverage` | 入口、调用链、现有模式、验证命令和影响面有证据 | `>= 0.85` |
| `acceptance_evidence` | 每个重要结果都有可执行或可观察完成证据 | `>= 0.85` |
| `execution_feasibility` | 至少一个候选方案能在当前项目能力和约束下执行 | `>= 0.85` |
| `risk_coverage` | 关键失败、兼容、权限、外部依赖和恢复风险已处理 | `>= 0.85` |

**证据等级：**

- `E0`：用户陈述或未验证直觉；只能形成待调查假设；
- `E1`：项目文档、配置或规则文件；可证明约定存在，不能单独证明运行行为；
- `E2`：源码、类型、调用链或真实测试入口；可支持实现边界判断；
- `E3`：实际执行的构建、测试、CLI、HTTP、浏览器或 replay 结果；可支持行为判断；
- `E4`：独立验收场景、真实用户路径或可复现的回归证据；可支持完成判定。

**硬门禁：**

- 任一关键维度 `< 0.70`：立即丢弃当前决策/候选，不进入正式 Unit；重新调查或重新 Grill；
- `0.70 <= score < 0.85`：只能进入 `needs_investigation`，必须产生新的有效证据后重算；
- `score >= 0.85`：可以作为正式决策，但仍必须列出证据和未覆盖风险；
- `0.85` 不是所有分数的平均值；五个关键维度和每个 author-blocking Decision Record 都必须单独达标；
- 方案还必须满足 `goal_coverage >= 0.80`、`acceptance_coverage >= 0.85`、`project_fit >= 0.75`，否则即使置信度高也标记为 `rejected`；
- 连续三轮调查/替代方案仍无法达到 `0.85` 时输出 `blocked`，不得为了产出完整 brief 而猜测。

### 1.7 复用外部 skill 的边界

- `grilling`、`domain-modeling`、`diagnosing-bugs`、`codebase-design` 是可被本 skill 复用的 model-invoked 方法；新 skill 通过 adapter 明确触发条件、输入、输出和停止条件；
- `triage`、`wayfinder`、`grill-with-docs`、`to-spec` 是 user-invoked 流程，不能被新 skill 静默当作子流程调用；新 skill 只能吸收其已调查的状态机、fog、HITL/AFK、验证和 brief 结构；
- 外部 skill 负责方法，Ralph skill 负责统一数据契约、评分、阈值和 author handoff；
- `domain-modeling` 的 `CONTEXT.md`/ADR 写入不是本 skill 默认副作用。只有用户明确选择持久化领域决策时才调用其写入语义，否则只把结果记录在 task brief/evidence artifact 中；
- 外部 skill 缺失时，skill 必须显式报告 `external_skill_unavailable`，使用 references 中的最小 fallback，并降低对应证据等级；不能把 fallback 的结果伪装成外部 skill 已执行。

### 1.8 范围与非目标

**范围：**新增 task discovery skill、task brief schema/validator、外部 skill adapter 规程、证据与决策评分、候选方案淘汰/替代协议、preset author handoff、public skill 安装/manifest、contract/fixture/e2e 测试和使用文档。

**非目标：**

- 不修改 `/Users/pittcat/Dev/agent_tools/skills` 外部仓库；
- 不修改 Ralph runtime、事件 schema、CLI 子命令或 preset YAML；
- 不让新 skill 直接生成、修改或 review preset；
- 不自动运行完整 `ralph run`；
- 不替代 `ralph-project-bootstrap` 的项目套件生成；
- 不自动写目标项目的 `CONTEXT.md`、ADR、代码或业务测试，除非未来单独授权；
- 不把所有任务强制拆成同一种 preset 拓扑；任务类型和项目事实决定交给 author 的约束。

## 2. 代码库现状与证据

### 2.1 当前实现入口与调用链

当前 public skill 的安装入口是 `skills/install.py`，public catalog 是 `PUBLIC_SKILLS`，marketplace 列表在 `.claude-plugin/marketplace.json`，测试模块由 `skills/tests/conftest.py` 预加载 flat helper。`ralph-preset-author` 通过 `SKILL.md` 和 `references/author-checklist.md` 定义 Discovery、Intent Confirmation、AAF、Payload Contract 和交 review 前门禁，但没有 task brief 输入协议。

外部 corpus 的核心可复用接口是文档流程而不是 Python API：

- `grilling`：一问一答、事实查环境、决策问用户、确认后才行动；
- `domain-modeling`：术语冲突、边界场景、代码交叉核对、必要时记录 glossary/ADR；
- `diagnosing-bugs`：先建立 red-capable feedback loop，再最小化复现并生成可证伪假设；
- `codebase-design`：用 seam/interface/adapter/depth 约束接口选择和测试面；
- `triage`：验证请求、查重复实现/历史拒绝、必要时 Grill，最终形成 agent-ready brief；
- `wayfinder`：把无法清晰表述的未知保留为 fog，不提前伪造 ticket 或决策。

### 2.2 Evidence Ledger

| ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `skills/ralph-preset-author/SKILL.md` Discovery gate | author 已要求先查仓库事实，再询问用户意图，并生成 Preset Intent Confirmation | discovery skill 只补充 author 缺少的结构化输入，不重复 preset topology 设计 | 高 |
| E2 | `skills/ralph-preset-author/SKILL.md` 与 `references/author-checklist.md` | author 已要求目标、成功、阻塞、范围、事实源、artifact 和 author 假设 | task brief 必须字段化这些内容，并让 author 复核而非重新猜测 | 高 |
| E3 | `skills/ralph-preset-author/SKILL.md` AAF/Payload 规则 | author 已有 per-hat 可行性、字段可见性、artifact-first 和 policy-check 审查 | discovery 不审 hat；只提供可验证任务事实与验收证据 | 高 |
| E4 | `skills/install.py::PUBLIC_SKILLS` | public skill 必须同时加入 catalog、源目录和安装测试 | 新 skill 需要同步安装入口和 parity tests | 高 |
| E5 | `.claude-plugin/marketplace.json` | marketplace skills 列表独立于 Python catalog | manifest 必须与 `PUBLIC_SKILLS` 同步，否则 skill 不可分发 | 高 |
| E6 | `skills/tests/conftest.py` | helper 以 flat module 预加载，测试不应自行修改 `sys.path` | validator/score helper 必须按现有加载约定注册 | 高 |
| E7 | `skills/tests/test_install.py` | 测试覆盖 catalog、marketplace、磁盘存在、select/install/prune 和物理复制 | 新 skill 必须有相同的可安装性、双目标复制和无 symlink 回归 | 高 |
| E8 | `skills/tests/test_execution_model_contract.py` | agent-facing vocabulary 使用结构化 contract test 固定跨 skill 词汇与 capability-triggered 规则 | task brief 字段、状态枚举和门禁词汇需要同样的 contract anchor | 高 |
| E9 | `skills/tests/test_prompt_visibility_contract.py` 与 author `commands.md` | prompt visibility 依赖真实 `ralph inspect prompt`，不是文字猜测 | author handoff 必须把 brief 路径作为可见输入，并测试 unavailable/invalid 状态不被当作 ready | 高 |
| E10 | `skills/ralph-project-bootstrap/SKILL.md`、`audit.py::collect_project_facts` | bootstrap 已能发现部分技术栈和验证命令，但未知项目会退化为 agent 自行发现 | discovery brief 必须记录“已发现事实”和“未发现能力”，不能把 unknown 当作可验证事实 | 高 |
| E11 | 外部 `grilling/SKILL.md` | facts 查环境，decisions 问用户，一次问一个问题，确认前不行动 | 新 skill 的 HITL 分支、问题顺序和最终确认 gate 复用该规则 | 高 |
| E12 | 外部 `domain-modeling/SKILL.md` | 术语需对照 glossary、边界场景要具体化、用户陈述需与代码交叉核对 | goal_clarity 与 acceptance_evidence 需要 domain terms/scenarios 证据 | 高 |
| E13 | 外部 `diagnosing-bugs/SKILL.md` | 无 red-capable feedback loop 不进入假设阶段；假设必须可证伪 | bug 任务设置更高验收证据门禁，并在 brief 中保存预测与复现证据 | 高 |
| E14 | 外部 `codebase-design/SKILL.md` | interface/seam/adapter/depth 是设计与测试面的共享词汇 | 候选方案比较必须记录 seam、适配器、测试面和删除测试结果 | 高 |
| E15 | 外部 `triage/SKILL.md` | 先查重复实现和历史拒绝，再验证请求，必要时 Grill，最终形成 ready brief | discovery 必须有 redundancy/prior-decision 检查和明确状态迁移 | 高 |
| E16 | 外部 `wayfinder/SKILL.md` | fog 不能提前伪造成可执行决策；HITL/AFK 和 blocking edge 需要显式记录 | brief 对未确认问题采用 `needs_user_decision`/`blocked`，不让 author 消化 fog | 高 |
| E17 | `docs/plans/2026-08-03-005-refactor-project-bootstrap-skill-plan.md` | 同仓库 skill 通过 typed result、strict stage、fixture、安装同步形成可验证 pipeline | 本计划沿用“结构化结果 + failure short-circuit + fixture + handoff”模式 | 高 |
| E18 | Git history：近期 public skill/install/preset author-review plans | skill 文档、agent metadata、catalog、marketplace、tests 需要同步 | U5/U6 必须覆盖分发 parity 和文档 drift | 中 |

### 2.3 受影响范围

**新增：**

- `skills/ralph-task-discovery/SKILL.md`；
- `skills/ralph-task-discovery/agents/openai.yaml`；
- `skills/ralph-task-discovery/references/task-brief-schema.md`；
- `skills/ralph-task-discovery/references/external-skill-adapters.md`；
- `skills/ralph-task-discovery/references/confidence-and-candidate-rubric.md`；
- `skills/ralph-task-discovery/references/author-handoff.md`；
- `skills/ralph-task-discovery/scripts/task_brief.py`；
- `skills/ralph-task-discovery/scripts/brief_validator.py`；
- `skills/tests/test_task_discovery_contract.py`；
- `skills/tests/test_task_discovery_e2e.py`；
- `skills/ralph-task-discovery/fixtures/` 下的 valid、low-confidence、alternative、blocked、bug-loop fixture。

**修改：**

- `skills/ralph-preset-author/SKILL.md`：增加 task brief 输入、校验、复核和失败 handoff；
- `skills/ralph-preset-author/references/author-checklist.md`：增加 brief SSOT 对账项；
- `skills/install.py`：加入 `ralph-task-discovery` public catalog；
- `skills/README.md`：加入 skill 目的、调用顺序和外部 skill 复用说明；
- `.claude-plugin/marketplace.json`：加入 public skill；
- `skills/tests/conftest.py`：注册新 helper 的 flat module preload；
- `skills/tests/test_install.py`：加入新 skill catalog/marketplace/install parity 断言。

**明确不修改：**

- `/Users/pittcat/Dev/agent_tools/skills`；
- `crates/ralph-*` runtime 和 CLI；
- `presets/`、`presets/schemas/`；
- 目标项目业务文件；
- `.ralph/events.jsonl`、`.ralph/loops.json`、其他 runtime internal ledger。

## 3. 决策记录与置信度

### D1：新建 `ralph-task-discovery`，不把全部逻辑塞进 `ralph-preset-author`

- **候选：**A. 只扩展 author；B. 新建 discovery skill，再让 author 消费 brief；C. 把逻辑放进 runtime。
- **选择：**B。
- **支持证据：**E1–E3 表明 author 已经拥有 preset-specific AAF/topology 职责；E11–E16 表明用户 Grill、领域建模、bug 复现和候选比较是前置决策流程；E17 表明 public skill 可以用独立 typed pipeline 解决编排问题。
- **排除原因：**A 会让 author 同时承担用户需求发现、项目审计和 preset topology，职责变浅且难以测试；C 会把 agent 方法论和 runtime 机制耦合，超出用户请求。
- **置信度：**0.96。

### D2：`task-brief.yml` 是 discovery 到 author 的唯一 handoff SSOT

- **候选：**A. 只在对话中交接；B. 写 Markdown spec；C. 结构化 YAML brief，长证据单独落盘。
- **选择：**C。
- **支持证据：**E2 要求 author 能消费目标、成功、阻塞、范围、事实源和假设；E3 的 artifact-first 规则要求重要证据可恢复；E17 的 typed pipeline 模式说明结构化结果更适合阶段门禁。
- **排除原因：**A 不可恢复且无法让 author 可靠校验；B 对人可读但不利于状态/阈值/字段校验；C 同时满足机器校验、人工审查和路径 handoff。
- **置信度：**0.94。

### D3：置信度与候选适配度分开计算，关键维度采用硬门禁

- **候选：**A. 一个总平均分；B. 仅模型自评 confidence；C. 证据置信度 + 方案适配度分离，关键维度硬门禁。
- **选择：**C。
- **支持证据：**E11–E16 的规则分别处理事实、决策、反馈回路、方案边界和阻塞；E13 明确没有 red-capable loop 就不得进入假设阶段；E17 体现 typed failure 不由自由文本推断。
- **排除原因：**A 会让“目标明确但验收不明”被其他高分掩盖；B 无法审计证据；C 能区分“我很确定但方案不适合”和“方案不错但事实还没确认”。
- **置信度：**0.95。

### D4：阈值采用用户指定的 `0.85`，低于阈值不进入 author-ready

- **选择：**每个关键维度和 author-blocking Decision Record 均需 `>= 0.85`；`0.70–0.84` 触发补证据；`<0.70` 丢弃当前判断并换方案/重调查；三轮仍不足则 `blocked`。
- **支持证据：**用户明确要求指标、阈值、低分丢弃和寻找替代方案；E13/E16 提供了“无证据停止”和“fog 不得伪装成决定”的方法基础。
- **置信度：**0.99（用户已明确指定）。

### D5：外部 skill 通过 adapter 复用，不复制全文，也不静默调用 user-invoked wrapper

- **选择：**reference 记录每个外部 skill 的触发条件、输入、输出、停止条件和 provenance；运行时优先调用可用的 model-invoked skill，缺失时使用最小 fallback 并降低证据等级；`triage`/`wayfinder`/`grill-with-docs`/`to-spec` 只吸收规则，不作为隐式子调用。
- **支持证据：**外部 skill 的 frontmatter 和工作流明确区分 model-invoked 与 `disable-model-invocation`；E11–E16 直接证明其职责边界。
- **置信度：**0.93。

### D6：brief 业务 artifact 放在 `.ralph/task-discovery/<task-key>/`，不污染 runtime ledger

- **候选：**A. repo root 的 `ralph-task-brief.yml`；B. `.ralph/task-discovery/<task-key>/`；C. 写进 `.ralph/events.jsonl`。
- **选择：**B。
- **支持证据：**E3 记录 author 的 artifact-first 约束；E10–E17 说明 `.ralph/` 已承载业务 artifact 与 runtime ledger 的明确边界；E17 还要求不把内部 ledger 当业务接口。
- **排除原因：**A 容易和多个任务冲突且不利于生命周期；C 明确违反内部 ledger 隔离。
- **置信度：**0.90。

### D7：author 只接受 `author_ready=true` 且通过 validator 的 brief

- **候选：**A. author 自己重新判断全部输入；B. author 信任 brief 的文本声明；C. author 读取并验证 brief schema、root、状态和版本后再设计 preset。
- **选择：**C。
- **支持证据：**E1–E3 的 author discovery gate 仍必须存在；E8/E9 的 contract test 模式要求结构化字段与真实可见性证据；E16 要求阻塞显式传播。
- **排除原因：**A 重复发现并形成两套事实源；B 会把低置信度 brief 误放行；C 保留 author 独立审查，同时把前置结果纳入可验证输入。
- **置信度：**0.92。

## 4. BDD 行为规格

### Feature: 从任意项目任务生成可审计的 Ralph author brief

#### Scenario TD-01：已知事实从仓库调查获得

```gherkin
Given 目标项目包含项目规则、依赖清单、入口代码和测试配置
When discovery 需要判断技术栈、验证命令或入口
Then 它读取并记录对应文件/符号/命令作为 Evidence
And 不把可由环境确认的事实作为用户问题
```

#### Scenario TD-02：业务决策一次只问一个问题

```gherkin
Given 目标范围或成功条件会改变 preset 行为
When discovery 需要用户决策
Then 它按 grilling 规则逐题提问并给出推荐答案
And 在用户确认前不生成 author_ready brief
```

#### Scenario TD-03：术语和边界与项目事实冲突时暂停

```gherkin
Given 用户使用的业务术语与项目 glossary 或调用链含义不一致
When domain-modeling adapter 发现冲突
Then brief 记录冲突、涉及证据和待确认决策
And 状态为 needs_user_decision 而不是 author_ready
```

#### Scenario TD-04：bug 任务必须先有 red-capable feedback loop

```gherkin
Given 用户目标属于 bug 或性能回归
When discovery 尚未找到能驱动真实症状变红的命令/测试/replay
Then brief 状态为 needs_investigation
And 不生成已确认根因或执行方案
When 连续调查仍无法形成 feedback loop
Then 状态为 blocked，并列出需要的环境、artifact 或授权
```

#### Scenario TD-05：完成证据不足时不得 author-ready

```gherkin
Given 目标描述清晰且项目入口已确认
When acceptance_evidence < 0.85
Then brief 不得将 author_ready 设为 true
And 必须列出缺失的可执行断言、行为场景或外部验证
```

#### Scenario TD-06：低置信度决策触发补证据

```gherkin
Given 一个 Decision Record 的 confidence 在 0.70 到 0.84
When validator 处理 brief
Then 状态为 needs_investigation
And 输出新的调查动作、预期证据和重新决策条件
And 不允许该决策驱动正式 author handoff
```

#### Scenario TD-07：低于 0.70 的方案被丢弃

```gherkin
Given 候选方案的关键假设 confidence < 0.70
When discovery 计算候选状态
Then 候选被标记为 rejected_low_confidence
And brief 保留淘汰原因与支持证据
And discovery 进入替代方案或补调查分支
```

#### Scenario TD-08：高置信度但验收覆盖不足的方案仍被丢弃

```gherkin
Given 候选方案 confidence >= 0.85
And acceptance_coverage < 0.85 或 goal_coverage < 0.80
When discovery 比较候选
Then 候选被标记为 rejected_insufficient_coverage
And 不得因为总体 confidence 高而 author-ready
```

#### Scenario TD-09：候选方案达标后才进入 author

```gherkin
Given 五个关键置信度维度都 >= 0.85
And 至少一个候选方案满足所有硬门禁
And 用户已确认目标、范围、完成证据和关键失败边界
When validator 验证 brief
Then 状态为 author_ready
And handoff 只传 brief 路径、目标项目 root 和 provenance
```

#### Scenario TD-10：三轮仍无法达标时阻塞

```gherkin
Given discovery 已完成三轮调查/候选替换
And 关键维度仍低于 0.85
When discovery 处理最后一轮结果
Then 状态为 blocked
And 输出缺失证据、已尝试方案、下一步人工输入和不可执行原因
And 不创建 author-ready handoff
```

#### Scenario TD-11：author 校验 brief 后再设计 preset

```gherkin
Given author 收到 task-brief.yml
When brief 不存在、schema 失效、root 不匹配或 author_ready=false
Then author 停止在 Discovery gate 并报告具体错误
When brief 有效且 author_ready=true
Then author 将 brief 作为已确认输入读取
And 仍执行现有 Intent Confirmation、AAF、Payload Contract 和 review handoff
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
|---|---|---|---|---|---|
| TD-01 | 可确认事实被写入 Evidence，未知事实进入 unknowns | `test_fact_sources_are_typed` | unit | unknown project fixture | 否 |
| TD-02 | 决策问题顺序、推荐项和确认 gate 可从 SKILL contract 找到 | `test_grilling_adapter_contract` | contract | 检查禁止自答 | 否 |
| TD-03 | glossary/code 冲突导致 `needs_user_decision` | `test_domain_conflict_blocks_ready` | unit | conflicting-doc fixture | 否 |
| TD-04 | 无 red-capable loop 不允许 bug author-ready | `test_bug_without_feedback_loop_blocks` | contract/e2e | diagnosing-bugs adapter fixture | 是，使用 replay transcript |
| TD-05 | acceptance 分数低于阈值拒绝 handoff | `test_acceptance_threshold_is_hard_gate` | unit | 边界值 0.84/0.85 | 否 |
| TD-06 | 0.70–0.84 触发新证据要求，不能直接放行 | `test_medium_confidence_requires_redecision` | unit | score boundary | 否 |
| TD-07 | `<0.70` 候选转 rejected 并保留原因 | `test_low_confidence_candidate_is_discarded` | unit | reason/provenance assertions | 否 |
| TD-08 | 高 confidence 但 coverage 不足仍 reject | `test_candidate_coverage_gate_is_independent` | unit | no averaging | 否 |
| TD-09 | 全部 hard gate 通过才产生 author-ready | `test_author_ready_requires_all_gates` | integration | brief schema + validator | 是 |
| TD-10 | 第三轮失败后 blocked 且不再自动循环 | `test_three_failed_attempts_block` | unit | loop cap | 否 |
| TD-11 | author 拒绝 invalid brief，接受 valid brief 后继续现有 gate | `test_author_brief_handoff_contract` | contract | author SKILL anchor test | 是 |

验收测试必须验证状态、分数、Evidence ID、候选状态、下一动作和 handoff 是否存在；不能只测试 Markdown 是否包含某个词。允许固定词汇的测试仅限稳定的 agent-facing contract、状态枚举、schema key 和安装 catalog，并在测试注释中说明契约原因。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | Unit |
|---|---|---|---|---|
| R1 | 调查结果必须区分事实、假设和决策 | TD-01, TD-03 | `test_fact_sources_are_typed`, `test_domain_conflict_blocks_ready` | U1/U2 |
| R2 | 用户关键决策必须逐题确认 | TD-02 | `test_grilling_adapter_contract` | U2 |
| R3 | 完成证据必须可执行或可观察 | TD-04, TD-05 | `test_bug_without_feedback_loop_blocks`, `test_acceptance_threshold_is_hard_gate` | U2/U3 |
| R4 | 置信度低于阈值不得进入 author | TD-06, TD-07, TD-10 | `test_medium_confidence_requires_redecision`, `test_low_confidence_candidate_is_discarded`, `test_three_failed_attempts_block` | U3 |
| R5 | 候选方案必须按目标覆盖、验收覆盖和项目适配筛选 | TD-08, TD-09 | `test_candidate_coverage_gate_is_independent`, `test_author_ready_requires_all_gates` | U3 |
| R6 | author 只消费经过校验的 brief | TD-11 | `test_author_brief_handoff_contract` | U4 |
| R7 | 新 skill 必须可安装、可分发、可测试 | TD-09, TD-11 | `test_catalog_and_marketplace_parity`, `test_task_discovery_installs_to_both_targets` | U5 |
| R8 | 外部 skill 复用必须有 provenance 和 fallback | TD-01–TD-04 | `test_external_adapter_provenance_and_fallback` | U2 |

## 7. 严格串行开发单元

### U1. 建立 task brief 数据契约与硬门禁 validator

**1. Unit 目标**

让一个结构化 brief 能被机器验证：字段、状态、Evidence、Decision Record、关键置信度、候选方案、author-ready 条件和失败状态必须有明确语义。

**2. 对应需求与 Scenario**

R1、R3、R4、R5；TD-05、TD-06、TD-07、TD-08、TD-09、TD-10；D2、D3、D4、D6。

**3. 外部可观察结果**

给定 valid/invalid brief，validator 返回结构化 `valid`、错误路径、状态建议、缺失证据和禁止 handoff 原因；validator 不会因为总平均分高而绕过单维度硬门禁。

**4. 当前行为基线**

仓库当前没有 Ralph task-discovery brief schema 或 validator；author 直接消费对话/仓库事实，现有 skill 测试主要固定 agent-facing contract 和安装行为。该空白由 E1/E2/E8 支持。

**5. 输入与输出**

- 输入：YAML brief 文本或解析后的 mapping；
- 输出：typed validation result；
- 错误：unknown status、missing required field、invalid score、unreferenced evidence、author-ready gate violation；
- 状态变化：validator 只建议/确认状态，不修改 target project；
- 不变量：`author_ready=true` 必须同时满足五个关键维度、候选硬门禁、用户确认、无 blocking decision、schema/version/root 校验。

**6. 修改位置**

- 新增 `skills/ralph-task-discovery/scripts/task_brief.py`：定义 brief、Evidence、Decision、Candidate、GateResult 的纯数据结构；
- 新增 `skills/ralph-task-discovery/scripts/brief_validator.py`：执行 schema、引用完整性、阈值和状态转换验证；
- 新增 `skills/ralph-task-discovery/references/task-brief-schema.md`：给 agent 看的字段来源、填充规则和失败停止条件；
- 新增 `skills/tests/test_task_discovery_contract.py`：测试 validator 行为；
- 新增 `skills/ralph-task-discovery/fixtures/`：valid、missing-evidence、medium-confidence、low-confidence、coverage-fail、blocked fixtures。

不得在本 Unit 修改 author、安装 catalog 或外部 skill source。

**7. 可依赖能力**

可复用现有 skills/tests 的 Python flat-module preload、fixture 和结构化 contract test 模式；可复用 Ralph preset review 中的 confidence/policy-feedback 词汇，但不复制其 preset finding schema。

**8. 禁止依赖的未来能力**

不得假设外部 skill 会返回特定 Python object；不得在本 Unit 实现用户访谈、项目扫描、preset 生成或 author 调用。

**9. 验收测试**

- valid brief：五维分数均 `0.85`、候选覆盖达标、决策均有 Evidence，返回 `author_ready=true`；
- `0.84` 任一关键维度：返回 `needs_investigation`；
- `0.69`：返回 `rejected`/`blocked`，并要求替代方案或新证据；
- 缺 Evidence ID、Evidence source 或 Decision support evidence：拒绝；
- 高置信度但 `acceptance_coverage=0.84`：拒绝 author-ready；
- `attempt_count=3` 且仍不达标：返回 `blocked`，不得建议第四轮自动调查。

**10. Acceptance Red**

首先运行 validator contract tests。当前不存在 brief validator，测试必须因缺少数据契约、阈值计算或引用检查而失败；编译环境、YAML 语法错误或 fixture 路径错误不算有效 Red。

**11. 单元测试拆分**

- Evidence 引用存在性和证据等级合法性；
- Decision confidence 与关键维度阈值；
- Candidate coverage 与 confidence 分离；
- author-ready hard gate 不可被平均分绕过；
- 三轮失败后的 blocked 状态；
- 状态只能按允许的单向转换进入 author-ready；
- brief schema version 和 project root provenance。

**12. Red → Green → Refactor 顺序**

1. 先写 valid brief 验收测试并观察当前无 validator；
2. 写最小 data model，使 valid brief 能解析；
3. 加缺失字段/证据的 Red 和 validator；
4. 加阈值边界 Red 和 gate evaluator；
5. 加候选 coverage 独立 gate；
6. 加三轮 blocked 状态；
7. 在 contract tests 保护下简化字段和错误输出。

**13. 最小实现范围**

必须实现：稳定 schema、typed validation result、五维硬门禁、候选独立评分、证据引用校验、阻塞状态和 JSON/YAML 可读输出。不得实现：LLM 评分、项目扫描、外部 skill 调用和 preset 生成。

**14. 集成验证**

使用 fixture 直接调用 validator；不需要真实 backend、Ralph CLI 或 issue tracker。真实 YAML 解析必须执行，不能只用 Python dict 构造绕过格式错误。

**15. 风险驱动测试**

边界值测试是必要的，因为 `0.70`、`0.85` 和 `0.84` 会改变状态；不需要 E2E 或 fuzz。

**16. 回归范围**

只回归新 helper、skills test loader、既有 installer tests 不受影响。不能修改或依赖 preset runtime。

**17. 预期文件变更**

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `skills/ralph-task-discovery/scripts/task_brief.py` | 新增 helper | brief typed model | E1/E2 |
| `skills/ralph-task-discovery/scripts/brief_validator.py` | 新增 helper | hard gate | E8/E13/E16 |
| `skills/ralph-task-discovery/references/task-brief-schema.md` | 新增文档 | agent-facing contract | E2/E3 |
| `skills/tests/test_task_discovery_contract.py` | 新增测试 | behavior contract | E8 |
| `skills/ralph-task-discovery/fixtures/*` | 新增 fixture | boundary evidence | D4 |

**18. 完成标准**

所有 fixture 断言通过；每个错误有稳定 code/path/next action；低分不放行；`author_ready` 只能由全部硬门禁产生；无跳过测试、宽松断言或隐藏默认值。

**19. 停止条件**

如果现有 Python 版本、YAML 依赖或 skills test loader 与计划冲突，停止并记录新 Evidence；不得在实现时改成未经决策的 JSON-only 或无 validator 文档方案。

### U2. 接入外部 skill adapter 与项目/用户事实发现流程

**1. Unit 目标**

让 discovery 按外部 skill corpus 的方法完成事实调查、用户决策确认、领域术语澄清和任务类型分流，并把每个结果绑定到 Evidence/Decision，而不是只产生自由文本。

**2. 对应需求与 Scenario**

R1、R2、R3、R8；TD-01、TD-02、TD-03、TD-04；D1、D5。

**3. 外部可观察结果**

调用 discovery 时，已知事实由项目读取获得；业务决策按一问一答获取；bug 任务进入 red-capable loop 分支；外部 skill 不可用时显式记录 provenance/fallback，不伪造已执行。

**4. 当前行为基线**

author 有自己的 Discovery gate，但没有统一 brief 输出；外部 skill 各自规定方法而无 Ralph handoff。E1、E11–E16 支持该基线。

**5. 输入与输出**

- 输入：目标、cwd、可选 spec/issue/plan、可用 skill 能力；
- 输出：`goal`、`project_facts`、`domain_terms`、`unknowns`、`user_decisions`、`task_type`、Evidence Ledger；
- 错误：root ambiguous、skill unavailable、fact conflict、user decision missing、bug feedback loop missing；
- 不变量：事实不能由用户回答覆盖，决策不能由 agent 自行替用户确认，外部 skill 结果必须有 source/provenance。

**6. 修改位置**

- 新增 `skills/ralph-task-discovery/SKILL.md`：定义可执行 workflow、调用顺序、状态和停止条件；
- 新增 `skills/ralph-task-discovery/references/external-skill-adapters.md`：逐个映射外部 skill 的触发条件、输入、输出、证据等级、fallback 和 user-invoked 限制；
- 新增 `skills/ralph-task-discovery/references/task-brief-schema.md` 的 Evidence/Decision sections；
- 新增 `skills/tests/test_task_discovery_e2e.py` 的 discovery transcript fixtures；
- 新增 `skills/ralph-task-discovery/agents/openai.yaml` 的名称、描述、输入输出和边界。

SKILL.md 必须明确引用外部 skill 的实际逻辑，而不是复制其全文；不得修改外部目录。

**7. 可依赖能力**

复用 `grilling` 的一题一问/确认 gate，`domain-modeling` 的术语冲突和场景压力测试，`diagnosing-bugs` 的 feedback loop/可证伪假设，`codebase-design` 的 seam/interface/adapter 方案语言。`triage` 与 `wayfinder` 只作为 reference pattern，不静默调用。

**8. 禁止依赖的未来能力**

不得在本 Unit 选择 preset hat 数、topic 或 execution model；不得把 brief 直接当成 author 完成结果；不得自动写 `CONTEXT.md`/ADR。

**9. 验收测试**

- transcript 中已有 package/test/CI 事实时，问题清单不重复询问这些事实；
- 用户对成功条件给出模糊回答时，产生一个带推荐项的下一问题；
- 用户确认前 brief 状态保持 `needs_user_decision`；
- glossary 与代码冲突时，记录冲突而不是自动覆盖 glossary；
- bug fixture 无 red-capable command 时进入 `needs_investigation`；
- 外部 corpus 不可用时，brief 包含 `external_skill_unavailable` 和 fallback provenance。

**10. Acceptance Red**

先运行 transcript contract tests。当前没有 discovery workflow 和 adapter contract，测试应能证明无法产生预期状态/证据；不能用“输出包含某句 prompt”作为唯一 Red。

**11. 单元测试拆分**

- fact vs decision 分类；
- 一次只生成一个用户问题；
- 推荐项和用户回答回写 Decision Record；
- domain conflict 状态；
- bug task 分流；
- external adapter unavailable 状态和 provenance。

**12. Red → Green → Refactor 顺序**

1. 先锁定 transcript 输入输出结构；
2. 实现 fact/decision 分离和问题 gate；
3. 加入 domain conflict 和 terminology evidence；
4. 加入 bug feedback-loop branch；
5. 加入 external adapter provenance/fallback；
6. 合并为 SKILL workflow 并删除重复文字。

**13. 最小实现范围**

必须明确调用外部 skill 的方法边界、事实调查动作、用户问题格式、确认 gate、状态迁移和 evidence 写入。不得实现自动回答用户决策、隐式调用 user-only skill、或自动改目标项目文档。

**14. 集成验证**

使用 deterministic transcript 和 fixture project 验证 discovery 输出；外部 skill 本身不在本仓库执行，因此测试必须检查 adapter contract/provenance，而不是假装拥有外部 skill runtime。

**15. 风险驱动测试**

需要 conflict fixture、unknown project fixture、bug-without-loop fixture 和 unavailable external skill fixture；不需要真实外部服务。

**16. 回归范围**

回归 U1 validator、现有 skill installer tests 和 `skills/tests/conftest.py` loader；不触碰 author/preset 逻辑直到 U4。

**17. 预期文件变更**

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `skills/ralph-task-discovery/SKILL.md` | 新增 skill | workflow | E11–E16 |
| `skills/ralph-task-discovery/references/external-skill-adapters.md` | 新增 reference | reuse/provenance | D5 |
| `skills/ralph-task-discovery/agents/openai.yaml` | 新增 metadata | public invocation | E4/E5 |
| `skills/tests/test_task_discovery_e2e.py` | 新增测试 | transcript behavior | R1/R2/R3/R8 |

**18. 完成标准**

每个外部 skill 都有 adapter row；每个 adapter 都有 unavailable fallback 和停止条件；所有事实/决策/假设可追踪到 brief；用户确认 gate 不可绕过。

**19. 停止条件**

如果无法从外部 skill 文件确认某条规则，必须标为 unverified 并降低证据等级；不得自行补写为“外部 skill 支持”。

### U3. 实现候选方案比较、置信度重算和低分替代流程

**1. Unit 目标**

让 discovery 在重要任务上产生、比较和淘汰候选执行方案，并在低置信度时按有限次数调查/替换，最终只能得到达标方案或明确 blocked。

**2. 对应需求与 Scenario**

R3、R4、R5；TD-04、TD-05、TD-06、TD-07、TD-08、TD-09、TD-10；D3、D4。

**3. 外部可观察结果**

brief 显示每个候选的 `goal_coverage`、`acceptance_coverage`、`project_fit`、`risk_coverage`、`confidence`、evidence refs、status 和 rejection reason；低分候选不会继续驱动 author；新证据会使 score 可追溯地重算。

**4. 当前行为基线**

author 的 decision confidence gate 主要针对 key-hat 运行决策；当前没有任务级候选方案和低分替换协议。外部 `codebase-design` 提供设计多个方案、比较 seam/depth 的方法；外部 `diagnosing-bugs` 提供 ranked falsifiable hypotheses；E13/E14 支持。

**5. 输入与输出**

- 输入：U1/U2 的 brief、目标类型、项目事实、完成证据和待验证假设；
- 输出：候选方案数组、评分、排序、淘汰原因、补证据动作、最终 selected candidate；
- 错误：所有候选均低于硬门禁、score 无 evidence、重复方案、超过三轮仍未达标；
- 不变量：confidence 不得由重复拷贝同一证据提升；候选评分和 confidence 不得互相替代；selected candidate 必须引用所有关键需求和验收证据。

**6. 修改位置**

- `skills/ralph-task-discovery/scripts/task_brief.py`：扩展 Candidate、Score、InvestigationAttempt、Rejection；
- `skills/ralph-task-discovery/scripts/brief_validator.py`：实现维度阈值、候选硬门禁和最多三轮状态；
- `skills/ralph-task-discovery/references/confidence-and-candidate-rubric.md`：冻结评分规则、证据等级、低分分类、替代方案选择和禁止平均分掩盖；
- `skills/tests/test_task_discovery_contract.py`：增加边界、重复证据、候选替代和 blocked 测试；
- `skills/tests/test_task_discovery_e2e.py`：增加多候选 transcript。

**7. 可依赖能力**

复用外部 `codebase-design` 的 seam/interface/adapter/depth 比较；复用 `diagnosing-bugs` 的 3–5 个可证伪假设思想，仅在 bug 任务启用；复用 `wayfinder` 的 fog/blocked 语义，不把未知事项伪造成候选结论。

**8. 禁止依赖的未来能力**

不得让候选评分直接决定 preset YAML；不得自动选择用户拥有的业务决策；不得用 LLM judge 取代 Evidence；不得实现无限 retry 或复杂 runtime recovery。

**9. 验收测试**

- 两个候选都高 confidence，但 A coverage 低：A rejected，B selected；
- 候选 confidence `<0.70`：立即 rejected_low_confidence；
- 候选 `0.70–0.84`：生成补证据动作，状态不变为 author-ready；
- 新增 E3/E4 后 score 从 `0.78` 重算到 `0.87`，且记录 attempt/provenance；
- 同一 Evidence 重复引用不能提升 score；
- 三次仍无达标候选：blocked，列出已尝试候选和人工需要的输入。

**10. Acceptance Red**

首先运行候选 fixture；当前无候选 evaluator 和 replacement protocol，测试应因缺少 candidate status/score/rejection 而失败。仅检查最终 selected 字符串不算有效 Red。

**11. 单元测试拆分**

- 证据等级到可用支持度映射；
- confidence 五维聚合与 hard floor；
- candidate coverage/fit 分数；
- rejected_low_confidence 与 rejected_insufficient_coverage；
- 新证据导致可审计重算；
- attempt cap 和 blocked；
- selected candidate 的需求/验收引用完整性。

**12. Red → Green → Refactor 顺序**

1. 先写两个候选对比验收测试；
2. 实现 Candidate/Score 数据结构；
3. 实现独立 confidence hard floors；
4. 实现 coverage/fit gates；
5. 实现低分 rejection 和替代候选；
6. 实现证据追加后的重算和三轮 blocked；
7. 重构评分规则为 reference 与 helper 共用的单一事实源。

**13. 最小实现范围**

必须实现“评分→门禁→淘汰/补证据→替代→blocked/selected”的有限状态流程；不实现自动生成所有可能方案，不实现跨轮 runtime 记忆，不改变 Ralph event loop。

**14. 集成验证**

通过 U2 transcript 生成 brief，再由 validator 评估多个候选；验证 output 中 evidence refs、rejection reason 和 next action 完整。

**15. 风险驱动测试**

需要边界值、重复证据、矛盾证据、候选等分和 no-candidate blocked 测试；bug 分支增加 falsifiable prediction fixture。

**16. 回归范围**

回归 U1 schema/state tests、U2 adapter tests、installer 不受影响；评分规则变更必须保证所有 fixture 的状态解释一致。

**17. 预期文件变更**

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `skills/ralph-task-discovery/references/confidence-and-candidate-rubric.md` | 新增 reference | 评分 SSOT | D3/D4 |
| `skills/ralph-task-discovery/scripts/task_brief.py` | 修改 helper | candidate model | R4/R5 |
| `skills/ralph-task-discovery/scripts/brief_validator.py` | 修改 helper | candidate gates | E13/E14/E16 |
| `skills/tests/test_task_discovery_contract.py` | 修改测试 | threshold behavior | TD-06–TD-10 |
| `skills/tests/test_task_discovery_e2e.py` | 修改测试 | multi-candidate flow | R5 |

**18. 完成标准**

所有分数都可回溯到 Evidence；低分候选必淘汰或补证据；三轮上限有效；selected candidate 满足所有硬门禁；没有自由文本或平均分绕过。

**19. 停止条件**

如果评分规则无法区分“证据置信度”和“方案适配度”，停止并修订 D3；如果新证据与旧证据矛盾，进入 blocked/needs_user_decision，不覆盖旧事实。

### U4. 让 ralph-preset-author 消费并复核 task brief

**1. Unit 目标**

使 `ralph-preset-author` 能以 task brief 为前置输入，校验其完整性和 author-ready 状态，复用已确认事实，同时保留现有 Intent Confirmation、AAF、Payload Contract 和 review gate。

**2. 对应需求与 Scenario**

R6；TD-11；D1、D2、D7。

**3. 外部可观察结果**

author 收到 invalid/blocked brief 时停止并指出错误；收到 valid author-ready brief 时，从 brief 读取目标、范围、验收证据、候选结论和风险，再进入现有 preset 设计流程；author 不会把 brief 的 author-ready 标志当作跳过 AAF/review 的许可。

**4. 当前行为基线**

author workflow 0 要求先发现并确认用户意图，但没有 task brief schema/handoff；E1–E3、E9 支持需要扩展入口而非重写 author。

**5. 输入与输出**

- 输入：repo-relative `task_brief_path`；
- 输出：author 的 Preset Intent Confirmation 必须引用 brief 的 Goal/Acceptance/Scope/Evidence；
- 错误：brief missing、schema mismatch、root mismatch、author_ready false、stale provenance；
- 不变量：author 仍独立检查 prompt visibility、AAF、Payload Contract、schema required fields 和 review handoff。

**6. 修改位置**

- `skills/ralph-preset-author/SKILL.md`：Workflow 0 增加 brief discovery/validation/handoff；
- `skills/ralph-preset-author/references/author-checklist.md`：增加 task brief SSOT 对账、Evidence/Decision 引用和 failure handoff checklist；
- `skills/ralph-task-discovery/references/author-handoff.md`：定义 author 读取顺序、最小可见字段、stale/root mismatch 和 stop conditions；
- `skills/tests/test_task_discovery_e2e.py`：增加 author handoff contract fixture；
- 如现有 skill metadata 需要声明前置关系，更新 `skills/ralph-preset-author/agents/openai.yaml` 的 `when_to_use`/boundary，不改变 preset author 的 public name。

不得修改 preset YAML、schema、runtime CLI 或外部 skill source。

**7. 可依赖能力**

U1 的 validator、U2 的 brief 结构、U3 的 author-ready gate；author 现有 Discovery/AAF/Payload/Review 规则；E9 的 `ralph inspect prompt` evidence 机制。

**8. 禁止依赖的未来能力**

不得把 author workflow 改成自动选 hat/topology；不得让 author 读取 `.ralph/events.jsonl` 等 internal ledger；不得跳过用户确认或 review。

**9. 验收测试**

- invalid brief：author 输出 `task_brief_invalid` 并停止；
- `author_ready=false`：author 不生成 YAML；
- valid brief：author Intent Confirmation 包含 brief 的目标、成功条件、阻塞条件、scope 和 evidence refs；
- brief 中候选被 rejected：author 不把 rejected 方案当作 selected；
- valid brief 仍要求现有 AAF/Payload/review gate。

**10. Acceptance Red**

先运行 author handoff contract fixture；当前 author 不识别 `task_brief_path`，测试应因缺少校验/引用而失败。不能通过修改测试把 invalid brief 当作 valid。

**11. 单元测试拆分**

- brief path/root/provenance 校验；
- status/author_ready 校验；
- Goal/Acceptance/Scope/Evidence 到 Intent Confirmation 的引用；
- rejected candidate 不可消费；
- author existing gates remain required。

**12. Red → Green → Refactor 顺序**

1. 先增加 invalid brief handoff test；
2. 加入 author brief validation instructions；
3. 加入 valid brief 引用和 Intent Confirmation mapping；
4. 加入 rejected candidate、stale provenance 和 root mismatch；
5. 回归现有 author checklist 的 gate anchors。

**13. 最小实现范围**

只增加 author 对 task brief 的输入识别、校验和引用；不把 discovery 逻辑复制进 author，不改变 author 输出 preset 的 schema/topology 规则。

**14. 集成验证**

使用 fixture brief 和 author skill textual/contract harness 验证 handoff；如果已有 author test runner 能调用真实 CLI，则增加最小 `inspect prompt`/preset check 证据，否则保持为 agent-facing contract test并明确静态性质。

**15. 风险驱动测试**

需要 root mismatch、stale brief、author-ready false 和 existing gate regression；不需要真实 backend run。

**16. 回归范围**

必须回归 `skills/tests/test_execution_model_contract.py`、`test_prompt_visibility_contract.py`、现有 author references anchor tests 和新 brief handoff tests；确保新输入不会削弱现有 author/review gate。

**17. 预期文件变更**

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `skills/ralph-preset-author/SKILL.md` | 修改文档 | consume brief | E1/E2 |
| `skills/ralph-preset-author/references/author-checklist.md` | 修改文档 | SSOT 对账 | E3/E9 |
| `skills/ralph-task-discovery/references/author-handoff.md` | 新增 reference | handoff contract | D7 |
| `skills/tests/test_task_discovery_e2e.py` | 修改测试 | TD-11 | R6 |

**18. 完成标准**

invalid brief 永远不生成 author-ready handoff；valid brief 被 author 引用且不跳过既有门禁；所有 handoff 字段可从 brief visible context 获得；不出现两套事实源。

**19. 停止条件**

如果 author 无法在现有 workflow 0 中消费 brief 而必须复制 discovery 逻辑，停止并重新评估 D1；不得把两个 skill 合并成一个大 skill。

### U5. 将新 skill 纳入 public catalog、安装、metadata 和用户文档

**1. Unit 目标**

让 `ralph-task-discovery` 像现有 public skill 一样可发现、可安装、可复制到 `.claude/skills` 和 `.agents/skills`，并让 operator 看到正确的调用顺序与边界。

**2. 对应需求与 Scenario**

R7；TD-09、TD-11；D5。

**3. 外部可观察结果**

`skills/install.py` 能选择并复制新 skill；marketplace 与 Python catalog 一致；README 和 metadata 说明 discovery → author → review → bootstrap 的关系；安装目标不产生 symlink。

**4. 当前行为基线**

E4–E7 已证明 catalog、marketplace、metadata、physical-copy install 和双目标测试是现有 public skill 契约。

**5. 输入与输出**

- 输入：public skill name `ralph-task-discovery`；
- 输出：源目录、metadata、marketplace、local/global install 均可见；
- 错误：catalog/marketplace drift、缺 SKILL.md、缺 metadata、destination symlink；
- 不变量：现有五个 public skill 的 install 行为不变。

**6. 修改位置**

- `skills/install.py`：加入 `PUBLIC_SKILLS`；
- `.claude-plugin/marketplace.json`：加入 skill path；
- `skills/README.md`：加入用途、调用顺序、外部 corpus 复用和边界；
- `skills/ralph-task-discovery/agents/openai.yaml`：补充 display/inputs/outputs/boundaries；
- `skills/tests/test_install.py`：复制现有 project-bootstrap public catalog/install assertions；
- `skills/tests/conftest.py`：加载新 helper（若 U1 helper 需要 test import）。

**7. 可依赖能力**

U1–U4 的 skill 目录和 handoff contract；现有 install.py catalog/physical-copy 机制。

**8. 禁止依赖的未来能力**

不得在本 Unit 改安装器语义、增加 symlink、修改外部 skill repo 或重命名既有 public skill。

**9. 验收测试**

- `PUBLIC_SKILLS` 与 marketplace skill names 集合相等；
- source `SKILL.md`/`agents/openai.yaml` 存在且非空；
- custom install 后目标有完整目录、无 symlink；
- local install 复制到两个目标；
- global dry-run 输出两个绝对目标但不写盘；
- 旧 skill selection 和 prune tests 仍通过。

**10. Acceptance Red**

先运行 install tests；新 skill 未加入 catalog/manifest 时，新增 parity test 必须失败。不能只手动复制目录而跳过 installer contract。

**11. 单元测试拆分**

- catalog membership；
- marketplace parity；
- on-disk metadata；
- custom/local/global install；
- no symlink/physical copy；
- existing catalog regression。

**12. Red → Green → Refactor 顺序**

1. 增加 catalog/manifest failing assertions；
2. 加入新目录 metadata；
3. 更新 catalog/manifest/README；
4. 运行 installer tests；
5. 清理重复说明并固定 source-of-truth。

**13. 最小实现范围**

只做 public distribution plumbing 和文档；不把 discovery workflow 逻辑写进 README 代替 SKILL.md。

**14. 集成验证**

通过 `skills/.venv/bin/python -m pytest skills/tests/test_install.py` 验证真实 installer；不需要 Ralph runtime。

**15. 风险驱动测试**

需要 physical-copy/no-symlink 和 marketplace drift 测试；不需要额外 fuzz。

**16. 回归范围**

完整 `test_install.py`、`test_execution_model_contract.py`、`test_prompt_visibility_contract.py`；README 的 public skill list 与实际 catalog 对账。

**17. 预期文件变更**

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `skills/install.py` | 修改 catalog | public distribution | E4 |
| `.claude-plugin/marketplace.json` | 修改 manifest | plugin discovery | E5 |
| `skills/README.md` | 修改文档 | operator routing | E7 |
| `skills/tests/test_install.py` | 修改测试 | install contract | E7 |

**18. 完成标准**

新 skill 可被 catalog 选择、安装和分发；旧 skill 测试不回归；文档明确外部 corpus 来源和 unavailable fallback。

**19. 停止条件**

如果 catalog 与 marketplace 的 source-of-truth 无法保持集合一致，停止并修复 manifest 设计；不得仅在 README 宣称已发布。

### U6. 完成 discovery-to-author 端到端 contract 与全量质量门禁

**1. Unit 目标**

验证从目标项目/用户 transcript 到 author-ready task brief，再到 author 拒绝或接受 handoff 的完整外部行为，确保低置信度不会穿透链路。

**2. 对应需求与 Scenario**

R1–R8；TD-01–TD-11；D1–D7。

**3. 外部可观察结果**

一个绿色 fixture 能完成 discovery → validated brief → author handoff；低置信度、冲突、无验收证据、外部 skill unavailable 和三轮失败 fixture 均在 author 前停止，并输出可定位的状态、Evidence、Decision 和 next action。

**4. 当前行为基线**

现有 skill tests 分别覆盖 installer、prompt visibility、execution-model vocabulary 和 project bootstrap；没有跨 discovery/author 的 brief pipeline。E7–E10 支持新增端到端 contract。

**5. 输入与输出**

- 输入：fixture project、用户 transcript、外部 adapter availability、候选方案；
- 输出：task brief、validation result、author handoff result、静态 report；
- 错误：每个阶段 failure-short-circuit，禁止后续 author 设计；
- 不变量：author-ready 只来自 validator；任何 rejected/blocked candidate 不会变成 selected。

**6. 修改位置**

- `skills/tests/test_task_discovery_e2e.py`：覆盖 green、low-confidence、alternative、blocked、bug-loop、invalid-author handoff；
- `skills/tests/conftest.py`：保证 helper 以现有 flat module 方式加载；
- `skills/ralph-task-discovery/references/author-handoff.md`：补充端到端证据/错误映射；
- `skills/README.md`：补充标准调用链和验证等级。

**7. 可依赖能力**

U1–U5 完成后的 brief、validator、adapter、author handoff、install/catalog。

**8. 禁止依赖的未来能力**

不得把完整 `ralph run`、真实收费 backend 或外部项目写操作作为本 Unit 的必要条件；E2E 使用 deterministic fixture/replay。

**9. 验收测试**

- green flow：最终 `author_ready=true`，author 接受 brief，仍保留现有 author gates；
- low confidence：进入 needs_investigation，author 不启动；
- rejected candidate：保留 rejection reason，替代方案达标后才继续；
- blocked：报告缺失证据和人工动作，不伪造 launch/preset；
- external unavailable：结果注明 static fallback 和 confidence impact；
- bug loop：red-capable replay 通过后才提升 acceptance evidence；
- installer and contract parity：所有分发文件一致。

**10. Acceptance Red**

先运行端到端 fixture。当前没有 discovery-to-author pipeline，至少 green flow 和 low-confidence flow 应失败；任何只断言 exit code、不检查 brief state/evidence/handoff 的失败无效。

**11. 单元测试拆分**

- pipeline stage short-circuit；
- valid brief handoff；
- invalid brief rejection；
- candidate replacement；
- blocked report；
- bug feedback evidence upgrade；
- installer/manifest parity。

**12. Red → Green → Refactor 顺序**

1. 写 green/blocked 两条端到端 fixture；
2. 接通 U1 validator 和 U2 workflow；
3. 接通 U3 candidate replacement；
4. 接通 U4 author handoff；
5. 接通 U5 install/catalog checks；
6. 扩展 bug/unavailable/invalid fixtures；
7. 运行全量 skill tests 并删除重复或过宽断言。

**13. 最小实现范围**

只验证现有模块之间的 contract；不引入新的 runtime orchestration，不启动真实 loop。

**14. 集成验证**

运行 skill test suite、安装测试、author contract tests；最终执行仓库规定的 `./scripts/run-tests.sh` 作为全量 Rust 基线，另执行 Python skill tests。若 Rust 全量基线出现时序 flake，只按仓库规则使用 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底。

**15. 风险驱动测试**

需要 failure-short-circuit、重复证据、stale brief、冲突事实、bug replay 和 unavailable dependency fixtures；不需要真实外部 API。

**16. 回归范围**

- `skills/tests/test_install.py`；
- `skills/tests/test_execution_model_contract.py`；
- `skills/tests/test_prompt_visibility_contract.py`；
- `skills/tests/test_project_bootstrap_contract.py`；
- 新增 task discovery contract/e2e tests；
- `./scripts/run-tests.sh`；
- 若修改 author 命令/文档引用，执行 `scripts/check-cli-doc-drift.sh --strict`。

**17. 预期文件变更**

| 位置 | 类型 | 原因 | Evidence |
|---|---|---|---|
| `skills/tests/test_task_discovery_e2e.py` | 修改测试 | full handoff | E7–E10 |
| `skills/ralph-task-discovery/references/author-handoff.md` | 修改文档 | error mapping | D7 |
| `skills/README.md` | 修改文档 | operator flow | E4/E5 |

**18. 完成标准**

所有 Scenario、validator、author handoff、installer、现有 skill contract 和规定全量门禁通过；无低于 `0.85` 的 author-blocking decision；无未解释的 blocked；没有新增 skip/.only/弱断言；所有失败分支都有可执行 next action。

**19. 停止条件**

任意端到端 fixture 发现 author-ready 可以绕过 validator、rejected candidate 可以进入 handoff、或 external fallback 被标成已验证，立即停止并修正 U1–U4，不继续扩大 fixture 数量。

## 8. Unit 串行依赖图

```text
U1 brief contract / validator
  ↓
U2 external adapters / fact + decision discovery
  ↓
U3 candidate scoring / threshold recovery
  ↓
U4 preset-author handoff
  ↓
U5 public catalog / install / docs
  ↓
U6 end-to-end contract / full regression
```

- U2 依赖 U1，因为 discovery 不能产生没有可验证结构的自由文本；
- U3 依赖 U2，因为候选必须引用真实项目事实、用户决策和完成证据；
- U4 依赖 U3，因为 author 只能消费已完成阈值判断的 brief；
- U5 依赖 U4，因为 public metadata 必须描述最终 handoff，而不是未定接口；
- U6 依赖 U1–U5，因为它验证完整链路和分发契约；
- 不允许提前在 U4 设计 preset topology，也不允许 U6 通过新增宽松 fallback 掩盖前置失败。

## 9. 执行命令清单

以下命令均基于当前仓库已确认的 Python skill tests、Rust workspace 和文档 drift 入口；命令属于实现阶段验证，不在本计划阶段执行。

| 时机 | 命令 | 目的 | 失败处理 |
|---|---|---|---|
| U1–U3 每次变更后 | `skills/.venv/bin/python -m pytest skills/tests/test_task_discovery_contract.py` | validator/schema/threshold Red-Green | 不得进入下一 Unit |
| U2–U4 每次变更后 | `skills/.venv/bin/python -m pytest skills/tests/test_task_discovery_e2e.py` | discovery/adapters/author handoff | 不得进入下一 Unit |
| U5 | `skills/.venv/bin/python -m pytest skills/tests/test_install.py` | catalog/marketplace/physical install | 修复后重跑 |
| U4/U6 | `skills/.venv/bin/python -m pytest skills/tests/test_execution_model_contract.py skills/tests/test_prompt_visibility_contract.py` | author/review 既有 agent-facing contract | 不得声明 author integration 完成 |
| U6 | `skills/.venv/bin/python -m pytest skills/tests` | Python skill 全量回归 | 不得交付 |
| U4/U6（若修改 CLI 文档引用） | `scripts/check-cli-doc-drift.sh --strict` | CLI 文档引用漂移 | 修正文档后重跑 |
| U6 最终 | `./scripts/run-tests.sh` | Rust workspace 规定全量基线 | 按仓库规则修复；仅时序 flake 使用 `RALPH_BASELINE_SERIAL=1` 兜底 |
| U6 最终 | `cargo nextest run -p ralph-core --test scenarios`（若 Rust 变更影响 scenario） | BDD runtime 回归 | 只有相关 Rust 变更时执行 |

Python 测试必须使用仓库现有 `.venv`；不得用裸 `cargo test -p ralph-cli`。新 skill 本身不应引入 Rust 生产变更，但最终基线仍遵循仓库硬规则。

## 10. 最终质量门禁

- task brief schema、状态和 Evidence 引用全部通过；
- 五个关键置信度维度和所有 author-blocking Decision Record 均 `>= 0.85`；
- `<0.70` 候选已被淘汰，`0.70–0.84` 已补证据或明确 blocked；
- 方案适配度和完成证据 coverage 未被总平均分掩盖；
- 三轮失败后稳定进入 blocked；
- 外部 skill provenance、fallback 和 user-invoked 边界可审计；
- author 能拒绝 invalid/blocked brief，接受 valid brief 但不跳过既有 AAF/Payload/review；
- catalog、marketplace、metadata、安装目标和 README 一致；
- 所有 Scenario 可追踪到测试和 Unit；
- 所有 feature-bearing Unit 有真实 Red、Green、Integration、Regression、Close；
- 没有新增跳过测试、`.only`、弱断言、无解释 snapshot/golden 或伪造完成证据；
- Python skill tests 全部通过；仓库要求的 Rust 全量基线通过；
- 没有修改外部 `/Users/pittcat/Dev/agent_tools/skills`；
- 没有修改 runtime ledger、preset YAML 或目标项目业务代码。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 6 个串行 U-ID，均有文件、行为、Red、测试、回归和停止条件 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D7 已冻结职责、artifact、阈值、替代和 handoff |
| 所有文件和接口是否有代码库证据 | 是 | E1–E18；新增路径均明确标为新增 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1 0.96、D2 0.94、D3 0.95、D4 0.99、D5 0.93、D6 0.90、D7 0.92 |
| 是否存在未处理的低置信度假设 | 否 | 未确认事项进入运行时 brief 状态；实现前没有依赖它们 |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 validator、U2 discovery、U3 candidate gate、U4 handoff、U5 distribution、U6 full contract |
| 每个 Unit 是否可以独立验证 | 是 | 每个 Unit 有真实 Red、fixture/test entry 和完成标准 |
| 每个 Unit 是否有真实 Red | 是 | 每个 Unit 明确当前缺失行为和失败原因 |
| 每个 Unit 是否包含回归范围 | 是 | 第 7 节每个 Unit 的 Regression 段已列出 |
| 是否存在未来 Unit 依赖 | 否 | 依赖只沿 U1→U2→U3→U4→U5→U6 |
| 是否存在泛化任务描述 | 否 | 文件、字段、状态、证据、断言和命令均已指定 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 第 5/6 节矩阵覆盖 TD-01–TD-11 |
| 所有关键决策是否有 Evidence | 是 | D1–D7 均引用 E1–E18 或用户明确阈值 |
| 计划是否可以严格串行执行 | 是 | 第 8 节明确顺序和禁止提前实现 |

本计划不修改生产代码；下一步由 `ce-work` 按 U1→U6 执行。
