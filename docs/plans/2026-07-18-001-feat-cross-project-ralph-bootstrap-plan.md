---
title: "feat: 新增跨项目 Ralph 运行套件 Skill 并删除 ralph-hats"
type: feat
status: active
date: 2026-07-18
origin: docs/brainstorms/2026-07-18-ralph-cross-project-runtime-bootstrap-skill-requirements.md
deepened: 2026-07-18
---

# feat: 新增跨项目 Ralph 运行套件 Skill 并删除 ralph-hats

## 1. 功能目标

### 业务目标

- 为 Ralph operator 提供一个可在任意目标项目目录调用的 `ralph-project-bootstrap` skill：以已经存在且由用户明确指定的 preset 与 plan/task 为输入，审计目标项目、生成或安全更新项目运行套件、完成分级验证，并交付可重复使用的正式启动命令（见 origin: `docs/brainstorms/2026-07-18-ralph-cross-project-runtime-bootstrap-skill-requirements.md`）。
- 将“preset 编排正确”与“目标项目能加载并运行该 preset”分成清晰的 operator 边界：`ralph-preset-author` / `ralph-preset-review` 管 preset，`ralph-project-bootstrap` 管首次项目落地，`ralph-loop` 管后续 run/monitor/resume/merge/debug，`ralph-run-diagnosis` 管复杂运行后归因。
- 彻底删除 `skills/ralph-hats`，不迁移其职责，不保留 alias、shim、deprecated wrapper 或替代入口。

### 本次范围

- 新增公共 operator skill `skills/ralph-project-bootstrap/`，包含 agent 元数据、按需 references、结构化 fixtures 与验证测试。
- 定义并验证目标项目根确认、项目事实证据分级、preset/plan 输入门禁、Markdown managed section、YAML owned keys、冲突停止、幂等更新及写盘失败恢复。
- 生成或完善目标项目的 `AGENTS.md`、`CLAUDE.md`、`ralph.pipeline.yml`、`PROMPT.pipeline.md`；只管理明确 owned 内容，不整文件覆盖。
- 使用本机 Ralph CLI 的真实能力探测、strict preset check、strict preflight 和 `run --dry-run` 验证配置合并；仓库固定 replay harness 自动验证 smoke 机制，目标项目的任何 loop smoke 必须先获得明确授权。
- 交付按验证等级标注的报告和正式启动命令；worktree 模式强制显式 `--plan` 或 `--worktree-name` 复用键。
- 删除 `skills/ralph-hats/` 及所有非历史有效引用；同步公共安装器、marketplace、技能说明、preset author 边界、指南、`AGENTS.md` 与 `CLAUDE.md`。

### 非目标

- 不创建、修改、修复或评审 preset，不把 `ralph-hats` 的能力迁移到任何 skill。
- 不修改 Ralph CLI 生产行为，不新增 CLI smoke 子命令，不把 `--max-iterations 1` 宣称为无副作用模式。
- 不修改目标项目业务代码，不自动安装 backend、系统依赖或凭据，不自动提交、推送、切分支或创建额外 worktree。
- 不默认执行真实付费/可写 backend；没有安全 smoke 路径时，只完成静态验证并请求用户另行授权。
- 不重写 `docs/achieved/**` 等不可变历史材料；需求文档与本计划中描述删除目标的字符串也不是待清理的产品入口。
- 不改 builtin preset、event topic、runtime 配置字段或 `ralph` CLI 参数，因此预计无需更新 `crates/ralph-core/data/ralph-tools*.md`、preset schema、zsh builtin completion；最终门禁必须反向确认该 no-op 判断。

### 已知约束和假设

- 严格串行执行 Unit 1 → Unit 9；每个 Unit 的验收、Red→Green→Refactor、相关集成测试与回归全部完成后，才可开始下一个 Unit。禁止跨 Unit 交替开发。
- 所有 Rust 测试只能走 `cargo nextest run` 系列；最终走 `./scripts/run-tests.sh`。Python 测试必须使用仓库 `.venv`。不得裸跑 `cargo test -p ralph-cli`。
- 新 skill 是文件型 public operator skill，不进入 Ralph runtime 的 `ralph tools skill` 注入注册表。现有模式为 `skills/<name>/SKILL.md`、可选 `references/`、`agents/openai.yaml`，发现/安装入口为 `skills/install.py` 与 `.claude-plugin/marketplace.json`。
- `ralph.pipeline.yml` 不是默认发现文件；所有 check/preflight/dry-run/smoke/正式命令必须显式携带 `-c ralph.pipeline.yml -H <preset>`，防止 `$RALPH_CONFIG`、`ralph.yml` 或 `ralph.yaml` 抢占。
- `ralph run --dry-run` 能证明 config、preset、prompt、backend detection 与 auto-preflight 的静态装载，但不会 spawn backend；报告不得把它描述为真实 loop 闭环。
- 默认安全模型：仓库自带、内容固定且经过测试的 replay harness 只能自动证明 bootstrap/smoke 机制；任何目标项目提供的 mock/custom/replay 或真实 backend 都视为不可信执行，必须先展示实际命令、hooks、网络、写盘与成本边界并取得用户明确授权。未授权时状态保持 incomplete，且本 skill 不自行创建 smoke worktree。
- plan/task 是正式套件的必需输入。缺少时在任何持久写盘前停止；不得生成空泛 prompt、猜测 plan 或创建 preset。
- 项目事实权威顺序：目标项目明确的 agent 指令与可执行 CI/任务入口优先于说明性 README；多个当前有效入口冲突、无法安全试跑或项目根不明确时，列证据并请求用户决定。
- `AGENTS.md` 与 `CLAUDE.md` 若项目已有完全同步规则则保持完全同步；否则只管理 skill owned section，并验证二者不矛盾。Ralph runtime 自带 managed-block sync 与 bootstrap owned section 必须使用不同边界，避免互相覆盖。
- 写盘采用“先计算变更集，再原子替换 owned 文件/区块”的策略；写盘过程失败回滚本轮已写内容，验证失败则保留生成套件、标记为未验证并允许幂等修复，不伪装成功。
- 新 skill 携带确定性 Python helper：skill 负责读取项目、解释证据、向用户决策和组织流程；helper 负责可重复测试的路径/marker/YAML ownership、provenance、候选 diff、原子应用、CLI argv/timeout 与结构化结果分类。不得让测试试图执行自然语言本身，也不得把这些确定性职责留给模型自由发挥。
- provenance 采用项目根的 repo-relative `ralph.bootstrap.yml`：只记录生成器版本、输入身份、owned 字段/区块和上次生成值的摘要，不记录凭据或运行时 ledger。只有当前 owned 值仍匹配上次生成值时可自动升级；用户改动、缺失或损坏 provenance 时展示三方差异并停止。
- `docs/solutions/patterns/critical-patterns.md` 当前不存在；本计划不能宣称已应用该文件。已采用相关现有知识库文档中的 fail-closed、workspace root、契约证据和静态合法不等于闭环等经验。

## 高层技术方向

> 本节只描述实施方向，不是可复制的生产代码。

Outside-In 的主链为：目标项目 fixture 的可观察结果 → skill 流程契约 → managed 内容与配置 ownership → CLI 能力/结果分类 → 安全 smoke → 正式交付。测试优先使用临时外部项目、fake CLI transcript 和结构化断言，不锁定整份 Markdown 文案。

验证状态必须单向提升：

```text
未审计
  → 输入与项目根已确认
  → 套件已生成但未验证
  → preset strict check 通过
  → strict preflight 通过
  → run --dry-run 静态装载通过
  → 安全 loop smoke 通过（仅有安全路径时）
```

任一层失败都停留在当前等级并输出阻塞分类；不得把低等级证明包装成高等级成功。只有目标项目的授权 smoke 通过后，才可交付“正式启动命令并完成”；静态验证通过但 smoke 未授权时仅交付“候选命令”，整体状态保持 incomplete。

## 2. BDD 行为规格

### Feature A：在任意项目落地已有 Ralph preset

#### Scenario S1：新项目生成完整运行套件

```gherkin
Given 当前目录已确认为目标项目根
And 用户指定了可读且兼容的 preset 与可读的 plan/task
And Ralph CLI 与安全 mock/replay backend 可用
When 用户调用 ralph-project-bootstrap
Then 项目中生成相互一致的 AGENTS.md、CLAUDE.md、ralph.pipeline.yml 与 PROMPT.pipeline.md
And 所有项目命令均来自可验证项目事实或用户选择
And 所有验证命令显式绑定目标 config 与 preset
And 分级验证结果和可重复使用的正式启动命令被交付
```

#### Scenario S2：不同技术栈只采用可证实命令

```gherkin
Given 目标项目分别表现为 Rust、Node、Python 或未知技术栈
When skill 审计构建、测试、lint 与格式化入口
Then 已知技术栈只写入实际存在且可验证的入口
And 未知或相互冲突的入口触发停止与用户选择
And skill 不臆造语言、工具或命令
```

#### Scenario S3：项目根不明确时停止

```gherkin
Given 当前目录、VCS 根或最近的 AGENTS.md 作用域不一致
When skill 确认目标项目根
Then 在写盘前列出所有候选根与证据
And 不自动切换目录或写入任一候选项目
```

#### Scenario S4：preset 或 plan/task 输入非法

```gherkin
Given preset 或 plan/task 未提供、不存在、不可读或类型不正确
When skill 执行输入门禁
Then 在持久写盘和 backend 调用前停止
And 不猜测 builtin、不创建 preset、不生成空泛 prompt
And 输出精确缺失项
```

#### Scenario S5：CLI 与 preset 能力不兼容

```gherkin
Given Ralph CLI 缺少必需命令或参数
Or strict preset check 返回解析、lint、schema 或能力错误
When skill 执行 capability gate
Then 将结果归类为 CLI/preset compatibility blocker
And 保持 preset 字节不变
And 不继续 preflight 或 smoke
```

#### Scenario S6：backend 不可用时 fail closed

```gherkin
Given 配置的 backend 未知、可执行文件缺失或凭据未就绪
When strict preflight 检查 backend
Then skill 区分并报告具体失败类型
And 不静默切换到另一个 backend
And 不开始真实 smoke
```

### Feature B：安全、幂等地维护项目文件

#### Scenario S7：项目指令文件为空白或不存在

```gherkin
Given AGENTS.md 与 CLAUDE.md 均不存在
When skill 生成项目指令
Then 两者包含项目可执行的命令、来源、触发条件和失败停止条件
And 不包含本机绝对路径、Ralph 内部 ledger 或特定语言假设
```

#### Scenario S8：保留已有用户内容

```gherkin
Given AGENTS.md、CLAUDE.md、ralph.pipeline.yml 或 PROMPT.pipeline.md 已有用户内容
And owned 边界完整且现有规则与探测事实兼容
When skill 更新套件
Then 只更新 managed section 或明确 owned YAML keys
And 用户内容逐字保留
And 不产生重复区块或重复 YAML key
```

#### Scenario S9：冲突或损坏边界时停止

```gherkin
Given 两份 agent 文档对同一权限或测试门禁冲突
Or managed marker 损坏
Or YAML 无法安全解析或 owned key 与用户值冲突
When skill 计算变更集
Then 在持久写盘前停止
And 展示冲突位置、证据与需用户决定的选项
And 不整文件覆盖
```

#### Scenario S10：重复运行无变化

```gherkin
Given 同一项目、preset、plan 与探测事实未改变
When skill 再次运行
Then 所有套件文件保持无 diff
And 不重复 managed block
And 输出 no-op 与当前验证等级
```

#### Scenario S11：配置优先级不会验证错文件

```gherkin
Given 项目同时存在 RALPH_CONFIG、ralph.yml 与 ralph.pipeline.yml
When skill 执行所有验证并生成正式命令
Then 每个命令都显式使用 -c ralph.pipeline.yml 与 -H 指定 preset
And dry-run 展示的 effective config、prompt 与 preset 均来自目标套件
```

#### Scenario S12：中途写盘失败可恢复

```gherkin
Given 多文件变更集已计算
And 第 N 个文件的原子写入失败
When skill 处理失败
Then 本轮已经写入的 owned 变更被恢复
And 用户原有内容不丢失
And 结果不被标记为已生成或已验证
```

### Feature C：分级验证与安全 smoke

#### Scenario S13：静态三段验证通过

```gherkin
Given 套件已经生成
When skill 依次执行 strict preset check、strict preflight 与 run --dry-run
Then 每一层都使用同一显式 config、preset 与 prompt
And 结构化证据被记录
And 报告只声明静态装载通过，不宣称 loop 已闭环
```

#### Scenario S14：存在安全 smoke 路径

```gherkin
Given skill 已展示目标项目实际 smoke 命令与副作用面
And 用户明确授权该目标项目 smoke
When skill 执行有 iteration、idle 与 wall-clock 边界的 smoke
Then config、preset、prompt 与 backend 被实际加载
And loop 到达约定的首个可观察事件或有界终态
And 诊断结果可读
And 不修改业务文件或访问未授权外部系统
```

#### Scenario S15：不存在安全 smoke 路径

```gherkin
Given 目标项目 smoke 尚未获得明确授权
When 静态三段验证通过
Then skill 停止并请求真实 smoke 授权
And 不把 dry-run 记录为完整运行成功
And 只交付标记 incomplete 的候选命令
And 不自行创建 worktree 或调用目标项目 backend
```

#### Scenario S16：smoke 超时或失败

```gherkin
Given 安全 smoke 出现超时、无事件、非零退出或错误事件
When 边界触发
Then 子进程被有界终止且诊断证据被保留
And 失败被归类到 suite、preset、backend 或 project-command
And 只允许修复 suite owned 内容后从 strict check 重新验证
And 不修改 preset 或业务代码
```

#### Scenario S17：脏工作树受保护

```gherkin
Given 用户已有未提交改动
When skill 审计或验证套件
Then 不删除、回退、提交或吸收这些改动
And 只修改明确 owned 的套件内容
And 任何可能触碰业务文件的 smoke 都停止请求授权
```

#### Scenario S18：正式交付具有正确 worktree 复用键

```gherkin
Given 静态门禁与目标项目授权 smoke 全部通过
When skill 生成正式启动命令
Then 命令显式包含 config、preset 与 plan/task
And 使用 worktree 时包含 --reuse-worktree 以及 --plan 或 --worktree-name
And 报告列出 created、updated、no-op、验证等级和剩余限制
```

### Feature D：彻底删除 ralph-hats

#### Scenario S19：删除所有有效入口且不迁移

```gherkin
Given 当前公共 skill 包与有效文档
When 删除 ralph-hats
Then skills/ralph-hats 整个目录消失
And installer、marketplace、skill README、项目规则、preset author 边界与当前指南不再发现或推荐它
And 仓库中不存在 alias、shim、deprecated wrapper 或职责迁移
```

#### Scenario S20：历史材料保持不变

```gherkin
Given docs/achieved 下存在历史 ralph-hats 引用
When 执行非历史残留扫描
Then 历史材料不被改写
And 除需求与计划对删除目标的规范性描述外，有效内容扫描结果为零
```

#### Scenario S21：其他 public skills 安装发现不回归

```gherkin
Given 新 skill 已进入 public catalog 且 ralph-hats 已删除
When 用户 list、默认安装、指定安装或 prune public skills
Then ralph-project-bootstrap、ralph-loop、ralph-preset-author、ralph-preset-review 与 ralph-run-diagnosis 可正确发现和安装
And ralph-hats 不可发现或安装
And preset shared references 仍被正确复制
```

## 3. 验收与测试策略

| Scenario | 验收条件 | 推荐测试层级 | 是否需要 E2E |
|---|---|---|---|
| S1 | 空白外部 fixture 生成四类套件，三级静态验证与安全 smoke 证据完整 | fixture 集成测试 + fake CLI 契约测试 | 是，1 条 mock 主路径 |
| S2 | 多技术栈只采用可证实命令，未知栈停止 | 参数化单元/集成测试 | 否 |
| S3 | root/作用域歧义写盘前停止 | 单元测试 + 临时 Git fixture | 否 |
| S4 | 非法 preset/plan 不产生持久文件或 backend 调用 | 输入校验单元测试 | 否 |
| S5 | 能力缺失或 strict check 失败准确分类且 preset 不变 | fake CLI contract test | 否 |
| S6 | backend 三类失败可区分且不 fallback | fake preflight contract test | 否 |
| S7 | 新 agent docs 可执行、无绝对路径/内部 ledger | 结构化内容测试 | 否 |
| S8 | owned 内容更新且用户区逐字保持 | 单元测试 + property-based 幂等测试 | 否 |
| S9 | marker/YAML/语义冲突 fail closed | 单元测试；对 marker parser 做 fuzz/生成式边界测试 | 否 |
| S10 | 第二次运行零 diff、零重复区块 | Idempotency 集成测试 | 否 |
| S11 | 所有 argv 显式绑定 config/preset且 effective source 正确 | fake CLI contract + 集成测试 | 否 |
| S12 | 第 N 次写入故障只回滚本轮 owned 变更 | Fault-injection 单元/集成测试 | 否 |
| S13 | 三段验证按序、证据分级且不夸大结论 | 状态机单元测试 + CLI contract test | 否 |
| S14 | 授权后的目标 smoke 进入约定事件/终态，副作用与授权边界一致 | fake contract + 人工授权路径验收；仓库固定 replay 只验证机制 | 是，固定 replay 机制 E2E |
| S15 | 无安全路径不 spawn backend，输出授权阻塞 | 集成测试 | 否 |
| S16 | timeout/nonzero/no-event 可终止、保留诊断、分类重试 | Fault-injection 集成测试 | 否 |
| S17 | 脏树内容与状态保持，不自动提交/回退 | 临时 Git 集成测试 | 否 |
| S18 | 正式命令字段完整，worktree 复用键合法 | 命令构造单元/契约测试 | 否 |
| S19 | skill 目录与全部有效入口消失，无替代层 | installer/catalog 集成测试 + 静态负向扫描 | 否 |
| S20 | 历史引用保留，active-scope 扫描为零 | 静态范围测试 | 否 |
| S21 | public skills list/install/prune 与 shared refs 正常 | Python installer 集成测试 | 是，custom temp dir |

## 4. 需求—测试追踪矩阵

| 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E |
|---|---|---|---|---|---|
| R1-R4 | S1-S5 | 外部项目入口、root 与输入门禁 fixtures | root、证据优先级、输入分类 | fake CLI capability contract | S1 |
| R5-R6 | S7-S10 | agent docs 创建/合并/冲突/no-op | managed section parser、同步规则 | 多状态文档 fixture | S1 |
| R7-R10 | S8-S12 | config/prompt owned 更新、优先级、回滚 | YAML owned keys、prompt ownership、事务状态 | CLI argv/effective source contract | S1 |
| R11 | S5-S6、S11、S13 | CLI/preset/backend/配置兼容门禁 | 结果分类状态机 | preset check/preflight/dry-run fake contract | S1 |
| R12-R14 | S13-S17 | 分级验证、授权 smoke、失败恢复 | 证明等级、授权决策、失败分类 | timeout/no-event/nonzero fault injection | 固定 replay 机制 E2E + 目标项目授权验收 |
| R15-R16 | S18 | 正式命令与交付报告 | 命令字段与复用键校验 | fake argv contract | S1 |
| R17-R20 | S2-S3、S7-S12、S17 | 可移植路径、幂等、dirty tree、可执行规则 | 路径规范化、marker、property-based 幂等 | 临时 Git/多技术栈 fixtures | S1 |
| R21-R24 | S19-S21 | 删除、历史豁免、安装发现回归 | public catalog selection | installer/marketplace/docs parity | S21 |

## 5. 严格串行开发单元

> 每个 Unit 必须独立完成下述 TDD 闭环。Executor 不得在当前 Unit 未关闭时预写下一 Unit 的生产内容或测试。

### Unit 1：建立公共 skill 发现与安装契约

- **Unit 目标：** 先用 characterization/ATDD 锁定 public skill 的单一清单、marketplace parity、默认/指定安装与 shared references 行为，再让空骨架 `ralph-project-bootstrap` 成为可发现、可安装的 public skill。
- **对应 Scenario：** S21 的“新增 skill 可发现安装”部分；S19 的“无职责迁移”前置边界。
- **外部可观察结果：** list/默认安装/指定安装在临时 custom dir 中包含新 skill；其他 public skills 不回归；非 public 目录不会被默认安装。
- **输入与输出：** 输入为 `skills/`、public catalog 和临时安装目录；输出为新 skill 骨架、统一 catalog/manifest 更新及安装行为证据。
- **可依赖的已完成能力：** `skills/install.py`、`.claude-plugin/marketplace.json`、现有 skill 包结构。
- **明确禁止依赖的未来能力：** 不依赖目标项目审计、文件生成、CLI 验证、smoke 或 `ralph-hats` 删除。
- **Files：**
  - Create: `skills/ralph-project-bootstrap/SKILL.md`
  - Create: `skills/ralph-project-bootstrap/agents/openai.yaml`
  - Create: `skills/tests/test_install.py`
  - Modify: `skills/install.py`
  - Modify: `.claude-plugin/marketplace.json`
  - Modify: `skills/README.md`
- **验收测试：** 在 `.venv` 中新增 installer characterization：public list 与 marketplace skills 一致；custom dir dry-run 无写入；force 安装新 skill；prune 仅清理未请求 public skill；preset skills 的 shared references 可读。
- **需要拆分的单元测试：** public selection 不再由“所有含 SKILL.md 的目录”隐式决定；未知 skill 明确报错；重复请求去重；新 skill frontmatter/name/agent metadata 一致。
- **Red 预期失败原因：** 当前 `discover_skills()` 会发现所有含 `SKILL.md` 的目录，`PUBLIC_SKILLS` 未成为实际默认过滤权威；catalog 中没有新 skill。
- **最小实现范围：** 仅统一 public discovery contract、加入新 skill 最小骨架和安装元数据；SKILL.md 此时只声明边界与后续 references 占位，不实现后续流程。
- **TDD 闭环：** 先启用 installer 验收测试并确认因发现集合/parity 缺口失败；再拆 selection/metadata 最小测试逐个 Red→Green；重构重复 public list 读取但不引入通用插件框架；运行本 Unit 集成测试与现有安装路径回归。
- **集成验证：** 临时 custom dir 执行 list/dry-run/install/prune，检查文件树而非完整文本 golden。
- **回归范围：** `ralph-loop`、`ralph-preset-author`、`ralph-preset-review`、`ralph-run-diagnosis` 安装；shared common copy；installer CLI help。
- **完成标准：** Unit 1 全部 Python 测试通过，新 skill 可安装，尚未宣称能生成套件；无测试跳过或削弱。
- **风险与注意事项：** 不在本 Unit 删除 `ralph-hats`，以免同时改变基线与新增入口；不让测试锁定整份 README 或 SKILL 文案。

### Unit 2：目标项目根、事实证据与输入门禁

- **Unit 目标：** 让 skill 在写盘前确认目标根、preset、plan/task 与项目事实来源，并对缺失、歧义或冲突 fail closed。
- **对应 Scenario：** S2-S4。
- **外部可观察结果：** 合法 fixture 产出结构化审计结论；未知技术栈、root 歧义、缺 preset/plan、不可读输入均停止且零持久写入。
- **输入与输出：** 输入为 cwd、VCS root、嵌套指令文件、preset path/builtin id、plan/task path；输出为审计清单、证据来源与 blocking decision。
- **可依赖的已完成能力：** Unit 1 的 skill package/fixtures 入口。
- **明确禁止依赖的未来能力：** 不读取未来生成的 config/prompt，不调用 backend，不依赖 managed writer 或 smoke。
- **Files：**
  - Create: `skills/ralph-project-bootstrap/references/context-audit.md`
  - Create: `skills/ralph-project-bootstrap/scripts/bootstrap.py`
  - Create: `skills/ralph-project-bootstrap/fixtures/projects/{blank,rust,node,python,unknown,ambiguous-root}/`
  - Create/Modify: `skills/tests/test_project_bootstrap_contract.py`
  - Modify: `skills/ralph-project-bootstrap/SKILL.md`
- **验收测试：** 参数化 fixture 验证项目事实采集、权威顺序、root 不一致阻塞、preset/plan 不合法阻塞、所有持久输出路径为项目相对路径。
- **需要拆分的单元测试：** 路径存在/文件/可读分类；cwd 与 VCS root/最近 AGENTS scope 判定；CI/任务入口优先于 README；冲突证据必须触发用户决策。
- **Red 预期失败原因：** 新 skill 骨架没有输入契约、root 确认或事实证据策略，fixture 无法获得确定停止结果。
- **最小实现范围：** 完成 procedural workflow 与测试 harness 所需的结构化 contract；不生成任何套件文件。
- **TDD 闭环：** 先写外部 fixture 验收并确认因缺审计契约失败；再为路径/root/证据分类逐个 Red→Green→Refactor；运行 fixture 集成与 Unit 1 installer 回归。
- **集成验证：** 从 fixture 根与子目录分别调用审计，验证不会向 skill 仓库、错误子树或 `.ralph` 内部 ledger 写文件。
- **回归范围：** Unit 1 public discovery；现有 skill 安装不受 fixture harness 影响。
- **完成标准：** S2-S4 可独立通过；缺关键输入时零写盘、零 backend 调用。S5 的 CLI/preset 能力兼容性由 Unit 5 完整验收。
- **风险与注意事项：** monorepo 不做自动猜测；非 Git 项目只有在用户明确确认 cwd 为根时才继续。

### Unit 3：安全维护 AGENTS.md 与 CLAUDE.md

- **Unit 目标：** 用明确 owned section 生成/更新 agent docs，保留用户内容、处理项目同步规则、检测冲突和损坏 marker，并保证幂等。
- **对应 Scenario：** S7-S10、S12 的 Markdown 写盘部分、S17。
- **外部可观察结果：** 空白项目获得可执行规则；已有项目只更新 owned section；第二次运行零 diff；冲突/损坏不写盘；dirty tree 不被清理或提交。
- **输入与输出：** 输入为 Unit 2 审计结果与现有 docs；输出为候选 diff、原子应用结果或 blocker。
- **可依赖的已完成能力：** Unit 2 的项目根与事实证据。
- **明确禁止依赖的未来能力：** 不依赖 pipeline config、prompt、CLI 验证或 smoke；不依赖 Ralph runtime managed-block sync 来替代项目规则。
- **Files：**
  - Create: `skills/ralph-project-bootstrap/references/agent-docs.md`
  - Modify: `skills/ralph-project-bootstrap/scripts/bootstrap.py`
  - Add fixtures: `skills/ralph-project-bootstrap/fixtures/projects/{existing-docs,conflicting-docs,broken-markers,dirty-tree}/`
  - Modify: `skills/tests/test_project_bootstrap_contract.py`
  - Modify: `skills/ralph-project-bootstrap/SKILL.md`
- **验收测试：** S7-S10、S12 写盘故障、S17；断言用户区逐字保持和 managed section 语义，不对整份 Markdown做 golden equality。
- **需要拆分的单元测试：** marker 0/1/重复/嵌套/截断；同步模式与非同步模式；绝对路径与内部 ledger 拒绝；任意用户前后缀保持；重复应用幂等；provenance 缺失/损坏/版本变化/用户修改 owned 值时的三方差异。对 marker parser 增加生成式/fuzz 边界，尤其非 UTF-8/损坏输入的停止行为。
- **Red 预期失败原因：** 尚无 ownership、冲突判定或原子多文件更新契约。
- **最小实现范围：** 只覆盖两份 agent docs；定义 bootstrap marker 与 Ralph runtime managed block 不重叠；写盘失败回滚本轮 owned 变更。
- **TDD 闭环：** 先以已有内容 fixture 写失败验收；再逐个完成 marker/同步/冲突/原子写入 Red→Green→Refactor；运行 dirty-tree 集成与 Unit 1-2 回归。
- **集成验证：** 在临时 Git repo 中比较运行前后 status/diff，确认只出现预期 docs owned 变化；注入第 N 次写入失败验证恢复。
- **回归范围：** 项目根、输入门禁、installer；不改变根仓现有 agent doc runtime sync 行为。
- **完成标准：** 所有 agent docs 场景通过，重复运行无 diff，冲突时零写盘，用户内容无丢失。
- **风险与注意事项：** 不创建 symlink 作为跨工具兼容的默认方案；若目标项目已有完全同步硬规则，最终内容必须遵守其规则。

### Unit 4：生成并幂等维护 pipeline config 与执行 prompt

- **Unit 目标：** 从已验证项目事实、preset 与 plan/task 生成最小 `ralph.pipeline.yml` 和 `PROMPT.pipeline.md`，安全维护 owned keys/section并显式绑定执行目标。
- **对应 Scenario：** S1、S8-S12、S18 的配置与 prompt 部分。
- **外部可观察结果：** 配置包含 backend、预算、prompt、诊断、preflight 与项目 guardrails；prompt 连接 plan/task 和项目规则但不复制 hat instructions；已有用户内容保持；冲突 fail closed；重复运行 no-op。
- **输入与输出：** 输入为 Unit 2 审计、Unit 3 agent docs 状态、用户 backend/预算选择；输出为候选/已写 config 与 prompt。
- **可依赖的已完成能力：** Unit 1-3。
- **明确禁止依赖的未来能力：** 不依赖 CLI check/preflight/dry-run 的结果来决定应写什么；不调用真实 backend；不修改 preset。
- **Files：**
  - Create: `skills/ralph-project-bootstrap/references/suite-authoring.md`
  - Create: `skills/ralph-project-bootstrap/fixtures/projects/ralph.bootstrap.yml.example`
  - Modify: `skills/ralph-project-bootstrap/scripts/bootstrap.py`
  - Add fixtures: `skills/ralph-project-bootstrap/fixtures/projects/{existing-suite,config-precedence,invalid-yaml}/`
  - Modify: `skills/tests/test_project_bootstrap_contract.py`
  - Modify: `skills/ralph-project-bootstrap/SKILL.md`
- **验收测试：** 配置/prompt 新建、已有内容合并、重复 YAML key/parse error、owned key conflict、plan unreadable、absolute path 泄漏、第二次运行零 diff；preset/backend/plan 变化且 owned 值未被用户修改时自动升级；用户修改 owned 值、provenance 缺失或损坏时展示三方差异并停止。
- **需要拆分的单元测试：** backend/budget/diagnostic 字段来源；prompt_file 与实际文件一致；plan/task 项目相对路径；hat instructions 不被复制；YAML 非 owned 任意键 property-based 保持；命令上下文始终要求显式 `-c/-H`。
- **Red 预期失败原因：** 尚无套件 ownership 或字段来源契约，根 `ralph.pipeline.yml` 的本仓库特定参数若直接复制会使多技术栈 fixture 失败。
- **最小实现范围：** 生成四类固定核心产物与 `ralph.bootstrap.yml` provenance；额外配套文件必须由“为何必需”门禁批准，不引入通用脚手架系统。YAML 格式化、注释、anchor 或 key 顺序无法无损保持时，不得静默重写整文件；只更新受控区块/可证明 owned 内容，否则停止。
- **TDD 闭环：** 先写 suite fixture 验收并确认缺文件/错误复制而失败；再按 config ownership、prompt ownership、幂等/冲突逐个 Red→Green→Refactor；运行 Unit 1-3 回归。
- **集成验证：** 在 blank/existing/config-precedence fixtures 中生成两次并比较结构化 YAML、owned section 和 diff。
- **回归范围：** agent docs、项目事实、installer；确认不触碰 preset 文件字节。
- **完成标准：** S1 的“生成”部分与 S8-S12 全通过；尚未宣称配置能被 CLI 加载。
- **风险与注意事项：** 不锁定本仓库 `ralph.pipeline.yml` 文本；它只能作为字段分层参考，不能作为跨项目模板。

### Unit 5：CLI 能力门禁与三级静态验证

- **Unit 目标：** 基于真实 CLI surface 建立 capability probe，并严格按 preset check → preflight → run dry-run 顺序给出分级证据和阻塞分类。
- **对应 Scenario：** S5-S6、S11、S13。
- **外部可观察结果：** 能力或兼容错误在正确阶段停止；所有 argv 显式使用目标 config/preset；dry-run 只提升到静态装载通过；backend 不被静默替换。
- **输入与输出：** 输入为 Unit 4 套件、CLI help/version/JSON 输出；输出为阶段状态、结构化证据与 blocker。
- **可依赖的已完成能力：** Unit 1-4。
- **明确禁止依赖的未来能力：** 不依赖真实 smoke 或正式交付；不通过修改 CLI 生产代码来满足 skill。
- **Files：**
  - Create: `skills/ralph-project-bootstrap/references/validation.md`
  - Create: `skills/ralph-project-bootstrap/fixtures/cli/`（版本化 fake transcripts/argv expectations）
  - Modify: `skills/ralph-project-bootstrap/scripts/bootstrap.py`
  - Modify: `skills/tests/test_project_bootstrap_contract.py`
  - Modify: `skills/ralph-project-bootstrap/SKILL.md`
- **验收测试：** CLI missing、required flag missing、preset strict fail、preflight config/backend/git/tools fail、dry-run source mismatch、全部静态通过；验证短路顺序。
- **需要拆分的单元测试：** capability 不只比 semver；JSON/human fallback 的可观察分类；unknown backend/executable/auth readiness；证明等级状态机禁止跳级/降级伪成功；所有命令含 `-c ralph.pipeline.yml -H <preset>`。
- **Red 预期失败原因：** 当前 skill 没有 CLI probe、结构化结果分类或验证状态机。
- **最小实现范围：** 复用当前 CLI 已有命令，不新增 Rust CLI surface；若当前版本缺所需能力，报告 blocker。
- **TDD 闭环：** 先用 fake CLI 契约写验收，确认因缺命令顺序/分类失败；逐个完成 capability、阶段短路、证据分级 Red→Green→Refactor；运行适用的 `ralph-cli` targeted nextest characterization，随后 Unit 1-4 回归。
- **集成验证：** fake CLI 记录 argv；另通过 Cargo 生成并显式传入当前 workspace 的 Ralph 可执行产物，对最小 fixture 做 strict check/preflight/dry-run，测试不得回退到 PATH 上可能过期的全局 `ralph`，且不能调用真实 LLM。
- **回归范围：** `crates/ralph-cli/tests/integration_preflight.rs`、`integration_run.rs`、`integration_run_presets.rs` 相关 nextest 子集；Python contract suite。
- **完成标准：** 静态三段门禁通过/失败均有正确证据；dry-run 报告绝不包含“loop 闭环通过”。
- **风险与注意事项：** `ralph run` 默认不一定启用 preflight，套件必须显式配置并仍独立运行 strict preflight；不得使用 `--skip-preflight` 绕过失败。

### Unit 6：安全 loop smoke 与失败恢复

- **Unit 目标：** 只在明确安全 backend 能力存在时执行有界 loop smoke；否则停止请求授权；对超时、无事件、错误事件与外部失败保留诊断并准确归因。
- **对应 Scenario：** S14-S17。
- **外部可观察结果：** 安全 fixture 能到达约定首事件/有界终态且业务树无 diff；无安全路径零 backend spawn；失败不被包装成成功且不改 preset/业务代码。
- **输入与输出：** 输入为 Unit 5 静态通过状态、安全能力声明、运行前 Git baseline；输出为 smoke 证据或授权/失败 blocker。
- **可依赖的已完成能力：** Unit 1-5。
- **明确禁止依赖的未来能力：** 不依赖正式命令/报告 Unit；不自行创建 worktree；不允许用未来诊断 skill 掩盖当前失败分类。
- **Files：**
  - Create: `skills/ralph-project-bootstrap/references/smoke.md`
  - Add fixtures: `skills/ralph-project-bootstrap/fixtures/cli/{safe-smoke,timeout,no-event,error-event}/`
  - Modify: `skills/ralph-project-bootstrap/scripts/bootstrap.py`
  - Modify: `skills/tests/test_project_bootstrap_contract.py`
  - Modify: `skills/ralph-project-bootstrap/SKILL.md`
- **验收测试：** safe smoke、真实 backend 未授权、timeout、nonzero、无事件、错误事件、dirty worktree；断言业务文件 diff、spawn 记录、诊断保留和状态分类。
- **需要拆分的单元测试：** safe capability 判定；iteration/idle/wall-clock 三重边界；首事件/终态证据；process termination；suite/preset/backend/project-command 分类；重试必须从 Unit 5 第一门禁重新开始。
- **Red 预期失败原因：** 当前 skill 没有安全路径判定，`--max-iterations 1` 仍可能第一轮写业务文件，超时也不提供回滚保证。
- **最小实现范围：** 仓库固定 replay harness 自动路径只证明机制；任何目标项目 mock/custom/replay/真实 backend 均需展示实际命令与 hooks/网络/写盘/成本边界后取得明确授权。未授权时状态 incomplete，只交付候选命令。自动 E2E 不调用真实付费或目标项目外部服务。
- **TDD 闭环：** 先用 safe/unsafe fake backend 写验收并确认 unsafe 被误启动或 safe 无证据；再逐个实现判定/边界/归因 Red→Green→Refactor；运行 Unit 5 contract 与此前回归。
- **集成验证：** replay/mock fixture 的少量 E2E，比较运行前后业务树；fault injection 验证 timeout/no-event/nonzero。
- **回归范围：** 三级静态门禁、dirty tree、原子 owned 写入、现有 replay/smoke runner 的 targeted nextest（若复用真实 runtime fixture）。
- **完成标准：** S14-S17 全通过；固定 replay 机制 E2E 无真实网络/付费调用；目标项目 smoke 的完成声明必须有授权与实际事件证据；失败证据完整。
- **风险与注意事项：** 若现有 replay 无法在外部临时项目证明预期事件，本 Unit 必须报告技术阻塞而不是降级断言 exit 0。

### Unit 7：正式启动命令与交付报告

- **Unit 目标：** 根据验证等级生成候选命令或复制可用的正式命令和完整 operator handoff；未完成目标项目授权 smoke 时只能标注“静态已验证、闭环未验证、状态 incomplete”。
- **对应 Scenario：** S1、S15、S18。
- **外部可观察结果：** 报告列出文件变更、CLI 能力、各门禁证据、剩余限制；命令显式 config/preset/plan；worktree 参数满足复用键硬规则。
- **输入与输出：** 输入为 Unit 1-6 状态；输出为中文交付摘要，以及与验证等级对应的候选命令或正式命令。
- **可依赖的已完成能力：** Unit 1-6。
- **明确禁止依赖的未来能力：** 不依赖 `ralph-hats` 删除或最终全量回归；不启动正式长任务。
- **Files：**
  - Create: `skills/ralph-project-bootstrap/references/handoff.md`
  - Modify: `skills/ralph-project-bootstrap/scripts/bootstrap.py`
  - Modify: `skills/tests/test_project_bootstrap_contract.py`
  - Modify: `skills/ralph-project-bootstrap/SKILL.md`
- **验收测试：** static-only 必须输出 incomplete + 候选命令、authorized-smoke-pass 才输出 complete + 正式命令、blocked 不输出可执行交付三类报告；non-worktree 与 worktree/plan/worktree-name 命令；created/updated/no-op 汇总。
- **需要拆分的单元测试：** 必需 argv 字段；worktree 复用键互斥/缺失；验证等级措辞；失败不得输出“可正式运行”；repo-relative plan 与 shell-safe 展示边界。
- **Red 预期失败原因：** 当前流程没有稳定 handoff contract，可能遗漏 config 或把 dry-run 包装为成功。
- **最小实现范围：** 按验证等级输出候选命令或正式命令与结构化摘要；日常 monitor/resume/merge/debug 明确转交 `ralph-loop`，复杂运行后诊断转交 `ralph-run-diagnosis`，不复制其手册。
- **TDD 闭环：** 先写三类 handoff 验收并确认字段/措辞缺失；逐个完成命令与报告规则 Red→Green→Refactor；运行 Unit 1-6 回归。
- **集成验证：** 将生成命令交给 fake CLI parser 验证 argv 可解析，但不执行正式 loop。
- **回归范围：** 所有 bootstrap fixtures；`ralph-loop` / diagnosis 边界文本静态检查。
- **完成标准：** S15 与 S18 的交付子条件通过；S1 只完成交付子条件，完整 S1 必须等 Unit 9 跨层 E2E 后关闭；用户无需猜测 config、preset 或 plan 连接方式；结论与证据等级一致。
- **风险与注意事项：** 不输出本机绝对路径；包含空格的路径在展示层需安全引用，但计划不规定具体 shell 实现。

### Unit 8：彻底删除 ralph-hats 及所有有效入口

- **Unit 目标：** 在新 skill 已独立可用后，原子删除 `ralph-hats` skill 和所有 active references，不迁移其职责，并保持历史材料不变。
- **对应 Scenario：** S19-S21。
- **外部可观察结果：** 无法 list/install/call `ralph-hats`；其他 public skills 与新 skill 正常；active docs 无旧入口；历史引用保留。
- **输入与输出：** 输入为 Unit 1 的 public catalog contract 与引用清单；输出为删除后的 skill 包、installer/catalog/docs 状态与负向扫描证据。
- **可依赖的已完成能力：** Unit 1-7；特别是 Unit 1 安装 characterization。
- **明确禁止依赖的未来能力：** 不把删除后的缺失能力留给 Unit 9；不在未来 Unit 补 alias、shim 或迁移。
- **Files：**
  - Delete: `skills/ralph-hats/SKILL.md`
  - Delete: `skills/ralph-hats/agents/openai.yaml`
  - Delete: `skills/ralph-hats/references/commands.md`
  - Delete: `skills/ralph-hats/references/examples.md`
  - Delete: `skills/ralph-hats/references/schema.md`
  - Delete if present: `.claude/skills/ralph-hats/` and `.agents/skills/ralph-hats/` repository-local copies/symlinks
  - Modify: `skills/install.py`
  - Modify: `.claude-plugin/marketplace.json`
  - Modify: `skills/README.md`
  - Modify: `skills/ralph-preset-author/SKILL.md`
  - Modify: `docs/guide/preset-authoring.md`
  - Modify identically: `AGENTS.md`, `CLAUDE.md`
  - Modify: `skills/tests/test_install.py`
  - Modify: `skills/tests/test_project_bootstrap_contract.py`
- **验收测试：** 先把 public list/marketplace/active-reference 期望改为“无 ralph-hats”并确认 Red；删除后 list/install 指定旧名失败、默认安装无旧目录、其他 skills 可用；active-scope `rg` 为零，历史不变。
- **需要拆分的单元测试：** installer unknown old skill；catalog parity；精确文件/结构字段级历史 allowlist；Markdown 旧路径链接与可执行安装/调用示例负向检查；frontmatter/agent metadata 旧名称负向检查；preset author 只移除无效转介，不复制旧 inspect/tune 工作流；AGENTS/CLAUDE byte equality。
- **Red 预期失败原因：** 当前目录、installer、marketplace、README、preset author、指南和项目规则均存在有效引用。
- **最小实现范围：** 精确删除引用与目录；不改历史 `docs/achieved/**`，不新增替代能力，不改 builtin preset 或 zsh completion。
- **TDD 闭环：** 先启用删除验收并确认正确失败；按 catalog→installer→docs→目录顺序最小修改使测试转绿；重构 reference scan，使豁免仅能命中精确历史文件或需求/计划中的叙述性删除事实，禁止旧路径链接和可执行调用示例借豁免残留；运行 Unit 1-7 全回归。
- **集成验证：** custom dir list/install/prune；marketplace skills 路径存在性；active-scope 负向扫描；AGENTS/CLAUDE 完全一致。
- **回归范围：** 所有 public skill 安装、preset shared refs、新 bootstrap 全流程；`ralph-preset-author` / review 边界。
- **完成标准：** S19-S21 全通过；非历史有效入口无残留；旧职责未迁移；当前 Unit 自身完成全部清理，不留给 Unit 9。
- **风险与注意事项：** 需求/计划为规范性描述，历史 Git 内容天然保留；扫描规则要区分产品入口与规范/历史证据，不能为了字符串清零篡改归档。

### Unit 9：跨项目主路径、文档漂移与全量回归门禁

- **Unit 目标：** 在所有行为已分别落地后，以少量真实跨层 E2E 和全仓门禁证明整体交付，无新增失败/跳过或文档漂移。
- **对应 Scenario：** S1-S21 全部汇总；不新增行为。
- **外部可观察结果：** blank/existing/conflict/safe-smoke 外部 fixture 端到端结果符合规格；CLI docs/skill catalogs/agent docs 同步；全量测试通过。
- **输入与输出：** 输入为 Unit 1-8 已验证能力；输出为最终测试证据、no-op 同步结论与剩余风险清单。
- **可依赖的已完成能力：** Unit 1-8 全部完成。
- **明确禁止依赖的未来能力：** 无；不得发现问题后跳过测试或把当前计划必需修复推迟到未来任务。
- **Files：**
  - Create/Modify: `skills/tests/test_project_bootstrap_e2e.py`
  - Modify only if drift found: 本计划已列明的 skill/operator docs；不得借机扩 scope
- **验收测试：** 新项目 mock 主路径、已有套件幂等更新、冲突 fail-closed、安全 smoke 失败恢复、ralph-hats 不可安装、其他 skills 安装成功。
- **需要拆分的单元测试：** 本 Unit 不新增业务单元逻辑；若 E2E 暴露缺口，在 Unit 9 未关闭状态下为对应能力补最小 Red→Green→Refactor，并重新验证从该能力所属 Unit 到 Unit 9 的线性链，不在 E2E 中放宽断言。
- **Red 预期失败原因：** 首次汇总可能暴露跨层 wiring、fixture 环境或文档 drift；失败必须能定位到所属能力层，而不是用 E2E mock 掉真实边界。
- **最小实现范围：** 只补跨层验收与必要 wiring 修正；不新增功能。
- **TDD 闭环：** 先启用端到端验收并确认任何 wiring 缺口以正确原因失败。若发现缺口，Unit 9 保持未关闭；在对应能力层补最小测试与修复后，严格按受影响能力所属 Unit → Unit 9 的顺序重新运行全部验收，不并行、不交替，直到此前完成状态重新获得验证；随后重构测试重复并依次运行 Python suite、targeted nextest、doc drift、全量 workspace 门禁。
- **集成验证：** 显式使用当前 workspace 构建产物执行 strict check/preflight/dry-run，并以仓库固定 replay harness 验证 smoke 机制；禁止真实付费 API。目标项目“complete”状态仅通过显式授权路径的人工/受控验收证明。
- **回归范围：** 全部 Python tests；相关 `ralph-cli`/`ralph-core` nextest；`./scripts/run-tests.sh`；CLI doc drift；public skill packaging。
- **完成标准：** 全部 Scenario、追踪矩阵和最终质量门禁通过；没有新增失败/跳过/only；未验证内容和剩余风险写入最终报告。
- **风险与注意事项：** 若全量基线出现竞态/时序 flake，只能按项目规则使用 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底；serial 仍失败即真实失败，必须修复。

## 6. 最终质量门禁

- [ ] S1-S21 全部通过，且需求 R1-R24 在追踪矩阵中都有可执行证据。
- [ ] 所有 Python 单元、property-based/fuzz 边界、installer 与 bootstrap contract 测试在 `.venv` 中通过。
- [ ] 所有必要的 CLI contract、临时 Git fixture、fault injection 与幂等集成测试通过。
- [ ] blank project mock 主路径和 safe replay/mock smoke 两条关键 E2E 通过；没有真实付费/外部副作用调用。
- [ ] 相关 Rust 测试仅通过 `cargo nextest run` 执行并通过；不得裸跑 `cargo test -p ralph-cli`。
- [ ] `./scripts/run-tests.sh` 全量通过；如发生时序 flake，按硬规则使用 serial fallback 并记录结果。
- [ ] `cargo fmt --check`、`cargo clippy`/项目既有 lint、Rust build/typecheck 以及 Python 静态检查（若仓库已有配置）通过。
- [ ] `scripts/check-cli-doc-drift.sh` 通过；涉及的 `ralph preset check --help`、`ralph preflight --help`、`ralph run --help` 与 skill references 一致。
- [ ] 反向检查 `crates/ralph-core/data/ralph-tools*.md`：本变更未改 agent loop 内 CLI/事件/配置能力，确认无需同步；若实施意外改变这些能力，必须立即同步并按 AGENTS.md 跑对应 smoke/drift 检查。
- [ ] 反向检查 `skills/ralph-preset-author`、`skills/ralph-preset-review` 与 `skills/ralph-preset-common/references/`：仅 author 边界去除无效 `ralph-hats` 转介；若实现触及 preset CLI/AAF/事件契约，必须同步 shared rubric/commands/fixtures 并跑最低验收。
- [ ] `AGENTS.md` 与 `CLAUDE.md` 内容完全一致。
- [ ] `skills/ralph-hats/`、repository-local installed copy、installer、marketplace、active docs 与当前产品说明中不存在可发现/可安装/可调用入口；没有 alias、shim、deprecated wrapper 或职责迁移。
- [ ] `docs/achieved/**` 历史材料未被改写；active-scope 负向扫描的 allowlist 只包含需求/计划的规范性删除描述与明确历史路径。
- [ ] 没有删除或削弱断言、跳过测试、`.only`、无解释 snapshot/golden 更新，且没有用 mock 替代必须验证的 CLI/文件系统边界。
- [ ] Git diff 不含 `.ralph/` 运行状态、临时 fixture 输出、凭据、本机绝对路径或其他 ephemeral 文件。
- [ ] 最终交付明确区分“静态装载已验证”与“安全 loop smoke 已验证”；任何未验证 backend 凭据、真实项目闭环或外部副作用风险均明确列出。

## 系统影响与回归关注

- **Interaction graph：** operator skill catalog → installer/marketplace → target-project audit → owned docs/config/prompt → CLI capability/preset check/preflight/dry-run → optional safe smoke → handoff；`ralph-loop` 与 `ralph-run-diagnosis` 仅作为后续边界，不成为 bootstrap 内部依赖。
- **错误传播：** 输入/root/ownership/CLI/preset/backend/project-command 任一错误都 fail closed，保留当前验证等级和证据，不允许默认成功。
- **状态生命周期：** 候选变更先计算；写盘失败回滚本轮 owned 变更；验证失败保留套件但标记未验证；重复运行幂等修复。
- **接口一致性：** `skills/install.py`、`.claude-plugin/marketplace.json`、`skills/README.md` 必须对 public skill 集合一致；`AGENTS.md`/`CLAUDE.md` 完全同步。
- **不变约束：** 不改变 Ralph CLI、preset runtime、event schemas、builtin preset、`ralph tools` 注入 skill 或 zsh builtin completion。

## 风险与缓解

| 风险 | 缓解 |
|---|---|
| 真实 smoke 首轮产生副作用 | 只自动运行仓库固定且审计过的 replay harness 证明机制；任何目标项目 smoke 均先展示副作用面并取得授权，且不自行创建 worktree |
| dry-run 被误报为真实闭环 | 验证等级状态机 + 报告措辞测试；只有观察到约定事件/终态才提升 smoke 等级 |
| 覆盖目标项目用户规则 | managed section/YAML owned keys、冲突 fail closed、原子写入与 fault injection |
| 多技术栈误判命令 | 项目事实证据分级；冲突/未知时询问，不从通用模板猜测 |
| 配置优先级加载错文件 | 每条命令显式 `-c ralph.pipeline.yml -H <preset>`，fake argv + effective source 验证 |
| CLI 与 preset 版本错配 | capability probe + 当前 CLI 真实 strict check，不只比较 semver |
| installer 默认发现泄漏 internal skills | Unit 1 characterization 后让 public catalog 成为真实 authority |
| 删除 ralph-hats 误伤历史或迁移职责 | active/historical scope 分离测试；后置原子删除；负向扫描 + 边界审查 |
| prompt 文本测试脆弱 | 只断言结构化语义、owned boundaries 与可观察行为，不锁整文件 golden |

## 参考来源

- Origin: `docs/brainstorms/2026-07-18-ralph-cross-project-runtime-bootstrap-skill-requirements.md`
- Public skills: `skills/README.md`, `skills/install.py`, `.claude-plugin/marketplace.json`
- Existing boundaries: `skills/ralph-loop/SKILL.md`, `skills/ralph-preset-author/SKILL.md`, `skills/ralph-preset-review/SKILL.md`, `skills/ralph-run-diagnosis/SKILL.md`
- CLI behavior: `crates/ralph-cli/src/commands/run.rs`, `crates/ralph-cli/src/commands/preset.rs`, `crates/ralph-cli/src/preflight.rs`, `docs/guide/runtime-contracts.md`
- Tests/patterns: `crates/ralph-cli/tests/integration_preflight.rs`, `crates/ralph-cli/tests/integration_run.rs`, `crates/ralph-cli/tests/integration_run_presets.rs`
- Institutional learning: `docs/solutions/developer-experience/agent-execution-contract-gates-2026-06-03.md`
- Institutional learning: `docs/solutions/developer-experience/wac-rollout-tiered-gates-2026-06-12.md`
- Institutional learning: `docs/solutions/integration-issues/emit-workspace-root-cwd-drift.md`
- Institutional learning: `docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md`
