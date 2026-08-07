---
title: Ralph 专用 Nowledge Claude Code 插件 - Plan
type: feat
date: 2026-08-07
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
deepened: 2026-08-07
---

# Ralph 专用 Nowledge Claude Code 插件 - Plan

## Goal Capsule

在本仓库新增 `plugins/nowledge-mem-ralph/`，作为 Ralph 项目环境专用的 Claude Code 插件。插件只暴露受控的 Memory 查询和状态检查，不设置 Session/Stop/SubagentStop/SessionEnd hooks，不自动保存会话，不注入 Working Memory。`scripts/setup_nowledge_ralph.py` 只迁移 Claude Code 的 project scope：安装专用插件、移除项目级 `nowledge-mem@nowledge-community` 并保留数据，用户级通用插件保持不变。

本计划仅开发插件包装与安装迁移，不实现 Ralph runtime 的检索、Thread 保存或 distill；这些内容属于独立计划 `docs/plans/2026-08-07-011-feat-ralph-nowledge-runtime-adapter-plan.md`。该引用只说明职责归属，不是前置依赖：即使适配计划完全未实施，本计划仍必须独立达到 Definition of Done。严格串行执行 `U1 → U2`，每个 Unit 完成 Acceptance Red、Unit Red、Green、Refactor、Integration、Regression 后才能继续。

## 0. 计划状态

- 状态：`READY`；所有实施关键决策置信度均不低于 `0.93`。
- 基线：分支 `pittcat-dev`，提交 `434c51c916fbef0b71b9efdc80df8fc4901a62ed`。
- 调查范围：本仓库 marketplace、安装脚本、skills installer tests、Claude adapter setting-source 隔离、设计文档落点规则；只读对照 Community Claude 插件的 commands、skills 与 hooks。
- 已执行验证：`claude plugin list --json`、`claude plugin --help`、`claude plugin install/uninstall/marketplace add --help`、`nmem --version`、`nmem --json m search --help`、`nmem --json status --help`；源码/测试/Git 历史只读调查。
- 尚未执行：本轮只写计划，未运行测试、构建或真实插件安装。
- 阻塞项：无。

## 1. 功能目标

- 业务目标：避免 Ralph child session 被通用插件按 Claude 会话边界抓取，给 Ralph 项目提供明确、无自动写入的插件能力面。
- 用户/调用方：安装插件的 operator；Ralph 启动的 Claude Code child；仍使用用户级通用插件的人工 Claude 会话。
- 当前行为：`scripts/setup_nowledge_ralph.py` 安装 Community marketplace 和项目级通用插件；机器同时有用户级通用插件。通用插件 hooks 调用 `nmem t save --from claude-code`。
- 目标行为：project scope 只启用专用插件；user scope 通用插件不变；专用插件只有 namespaced search/status 与只读 skill，无 hooks/自动写入/WM。
- 行为差异：从“项目级通用自动捕获”变为“项目级专用只读能力”；不改变 nmem 数据。
- 范围：插件目录、root marketplace、安装脚本及其 Python contract tests、插件 README、计划新增的独立设计文档。
- 非目标：Ralph Rust runtime、`nowledge:` YAML、prompt recall、Thread create/append/distill、Community 源码、旧数据清洗。
- 输入：search query、status 请求、target project path、`--dry-run`、`claude plugin list --json`。
- 输出：有界的 nmem JSON 查询结果或可操作错误；可 validate 的插件包；确定的 project-scope marketplace/install/uninstall 调用；最终 scope 验证；设计与使用文档。
- 状态变化：真实安装仅改变 target project 的 Claude plugin scope；`--keep-data` 保留通用插件数据。
- 错误语义：空 search query 在调用 nmem 前显示用法并停止；nmem 不可用/服务异常时报告原错误且不得执行写命令；Claude CLI 缺失、JSON 无法解析、专用插件安装失败、generic 卸载失败或最终 scope 不符时 installer 非零退出；不得猜测性卸载。
- 兼容性：保留用户级通用插件；root `ralph-orchestrator` plugin 与 `PUBLIC_SKILLS` 行为不变。
- 性能：默认 memory search 最多返回 5 条；只有用户明确需要追溯原对话且 memory 结果不足时才可渐进读取 thread，每次最多 8 条消息、每条内容最多 1200 字符；安装命令数量有界；不引入运行时 hook。
- 安全/权限：所有 subprocess 使用 argv 和明确 cwd；只在 JSON 明确报告 project generic 时卸载该 scope。
- 已知约束：Ralph Claude adapter 固定加载 `project,local`，不加载 user settings。
- 已确认假设：Claude CLI 支持 local marketplace path、project scope、`--keep-data` 和 JSON list。
- 待验证假设：无。

### 1.1 独立执行合同

- 构建/安装前置是已安装的 `claude` CLI；实际执行 search/status 时另要求 `nmem` CLI 和其服务可用。不得检测或要求 `nowledge:` Ralph 配置、`NowledgeLifecycle`、`ralph-tools-nowledge` 或任何适配计划产物。
- 本计划的测试和完成门禁不得运行或依赖 `crates/ralph-cli/tests/integration_nowledge.rs`。
- 本计划可在适配计划之前、之后或完全不执行适配计划的情况下完成；执行顺序不改变R1–R8的验收结果。
- 若实现发现必须修改 `crates/ralph-core/`、`crates/ralph-cli/` 或 Ralph agent注入指南，立即停止；不得把该修改吸收到本计划。

### Requirements

- `R1`：新增版本 `0.1.0`、name=`nowledge-mem-ralph` 的插件；manifest 不声明 hooks。
- `R2`：插件提供 namespaced `search`、`status` command 和只读 `search-memory` skill；不得提供 save-thread、自动捕获或 WM 注入。
- `R3`：root marketplace 新增第二 plugin entry，source 固定为 `./plugins/nowledge-mem-ralph`；原 root plugin 的 skills 集合不变。
- `R4`：installer 在 target project cwd 使用本仓库绝对路径添加 project marketplace，安装专用插件，并仅迁移 `scope=project` 且 canonical `projectPath` 等于 target root 的 generic；不得把其他项目的project条目算作当前状态。
- `R5`：首次安装、已安装重跑、dry-run 和各 mutation 失败均确定；先验证 dedicated project 安装成功，再卸载 project generic；最终用 `plugin list --json` 验证 dedicated project 存在、generic project 不存在，且安装前存在的 user generic 完全不变、安装前不存在时也不得创建。
- `R6`：新增 `.ralph/specs/nowledge-mem-ralph-plugin-design.md`，锁定插件与 Ralph runtime 的职责边界、组件合同、信任边界、installer 状态机、失败恢复、scope 矩阵、测试追踪和版本策略。
- `R7`：plugin README 必须让 operator 不读源码即可选择插件、安装、验证、使用、排障和卸载；必须明确通用插件用于人工 Claude user scope、专用插件用于 Ralph target project scope，以及专用插件不自动保存、不读取 Working Memory、不拥有 distill 生命周期。
- `R8`：search/status command 与 `search-memory` skill 只允许调用 `nmem --json m search`、`nmem --json status`，以及确需追溯时的有界 `nmem --json t search/t show`；禁止所有 memory/thread 写命令和 Working Memory 读取。

## 2. 代码库现状与证据

### 2.1 当前实现入口

- `.claude-plugin/marketplace.json` 是本仓库 Claude marketplace，目前只有 `ralph-orchestrator` plugin。
- `scripts/setup_nowledge_ralph.py` 是既有安装入口，接收 target path/`--dry-run`，当前安装 Community plugin。
- `skills/tests/test_install.py` 与 `skills/tests/test_e2e_bootstrap_e2e.py` 读取 marketplace，部分代码假设目标为 `plugins[0]`。
- `crates/ralph-adapters/src/cli_backend.rs` 的所有 Claude 构造器传入 `--setting-sources project,local`。

### 2.2 Evidence Ledger

| Evidence | 来源 | 观察结果 | 计划影响 | 可靠性 |
|---|---|---|---|---|
| E1 | Git HEAD/status | 基线明确，原工作树无生产变更 | 两份计划可独立追踪 | 高 |
| E2 | Community `nowledge-mem-claude-code-plugin/hooks/hooks.json` | 通用插件在多个 session hook 自动保存 | 专用插件必须无 hooks | 高 |
| E3 | Community `nmem-hook-save.py` | 写入命令是 `nmem t save --from claude-code` | 专用插件不得暴露该路径 | 高 |
| E4 | 真实 `nmem t show` 样本 | 捕获内容包含 developer 指令和工具环境 | 证明 raw session 不适合 Ralph 知识 | 高 |
| E5 | target cwd中的`claude plugin list --json` | generic 同时存在user/project scope；project条目含`projectPath` | installer必须按id+scope+canonical target path迁移 | 高 |
| E6 | `cli_backend.rs` 与隔离测试 | Ralph Claude child 只加载 project/local | user generic 可安全保留 | 高 |
| E7 | Git `240ad50d` | setting-source 隔离是有意兼容合同 | 不修改 adapter | 高 |
| E8 | `scripts/setup_nowledge_ralph.py`、Git `90e399d8` | 已有安装入口但目标是 Community | 原位修改而非第二套 installer | 高 |
| E9 | `.claude-plugin/marketplace.json` | marketplace 支持 plugin source/skills | 新 entry 落在现有 SSOT | 高 |
| E10 | `skills/tests/test_install.py` | catalog consumer 存在首项假设 | 新增 entry 前先修 name-based lookup | 高 |
| E11 | root `.venv` | pytest 9.1.1 可用 | Python tests 必须经 `.venv` | 高 |
| E12 | `AGENTS.md` 的 Specs & Tasks 硬规则 | design docs/specs 必须放在 `.ralph/specs` | 独立设计文档不得放进 `docs/plans` 或 plugin README 代替 | 高 |
| E13 | Community `commands/search.md`、`commands/status.md`、`skills/search-memory/SKILL.md` | 上游已有 search/status 与渐进 thread 查看模式，但还同时包含写入 hooks/skills | 复用成熟查询语义，按 Ralph 边界删除全部写能力 | 高 |
| E14 | `nmem --json m search --help`、`nmem --json status --help` | search 支持 `--limit` 且 query 必填；status 支持 JSON；thread 查询参数已由 CLI help 验证 | 命令合同可锁定为 JSON、有界、空输入不调用 | 高 |
| E15 | `claude plugin install/uninstall/marketplace add --help` | 三个 mutation 均支持 project scope；uninstall 支持 `--keep-data` | installer 可显式控制 scope 与数据保留 | 高 |

### 2.3 受影响范围

- 生产/包装：`.claude-plugin/marketplace.json`、`scripts/setup_nowledge_ralph.py`、计划新增 `plugins/nowledge-mem-ralph/`。
- 测试：计划新增 plugin/installer pytest；修改现有 marketplace consumers。
- 文档：计划新增 `.ralph/specs/nowledge-mem-ralph-plugin-design.md`；plugin README 的选择、安装、验证、使用、排障、卸载和隐私说明。
- 不影响：Rust crates、Ralph config/event/preset/schema、Nowledge 数据、Community 仓库。

## 3. 决策记录与置信度

| Decision | 问题 | 候选方案 | 最终选择 | 证据 | 排除原因 | 置信度 |
|---|---|---|---|---|---|---:|
| D1 | 修改范围 | Community；当前仓库；两者 | 仅当前仓库专用插件（session-settled: user-directed） | 用户确认、E2–E4 | 上游修改扩大范围且不拥有 loop 生命周期 | 0.99 |
| D2 | 插件 scope | user；project；两者 | dedicated project、generic user | E5–E7 | 与 Ralph setting sources 精确对齐 | 0.99 |
| D3 | 插件写入模式 | hooks；manual save；无写入 | 无 hooks、无写入能力 | E2–E4 | 自动/手动 raw capture 都绕过 Ralph curation | 0.98 |
| D4 | 安装入口 | 新脚本；修改既有脚本 | 修改 `setup_nowledge_ralph.py` | E8 | 避免两套安装状态机 | 0.97 |
| D5 | marketplace 布局 | 替换 root plugin；第二 entry | 新增独立 entry | E9,E10 | 保留现有 public skills plugin | 0.96 |
| D6 | 卸载判定 | settings字符串扫描；仅JSON scope；JSON id+scope+projectPath | 用`plugin list --json`按完整id、scope和canonical target projectPath精确判定 | E5 | 字符串扫描与仅scope都可能误中其他plugin或其他project | 0.98 |
| D7 | subprocess 上下文 | 隐式 cwd；target cwd+绝对 source | target cwd，source=repo root绝对路径 | E8,E9 | 支持为其他项目安装且无相对路径歧义 | 0.93 |
| D8 | 设计说明放置 | 仅README；`docs/plans`；`.ralph/specs`独立文档 | 新增 `.ralph/specs/nowledge-mem-ralph-plugin-design.md`，README只承担操作说明 | E12 | README不足以承载状态机/信任边界；plan不是稳定设计合同 | 0.99 |
| D9 | 查询能力面 | 完全复制Community；只做memory search/status；允许有界thread追溯 | 默认memory search/status；仅在追溯原对话确有必要时有界thread search/show；永不写入或读取WM | E3,E4,E13,E14 | 完全复制会重新引入错误生命周期；禁止thread会丢失必要来源核验能力 | 0.96 |
| D10 | migration顺序 | 先卸generic再装dedicated；先装dedicated并验证再卸generic；事务回滚 | 安装并验证dedicated后才卸project generic；install、权威list验证或uninstall失败时非零并保留可恢复状态 | E5,E15 | 先卸载会制造无插件窗口；Claude CLI无跨命令事务，伪回滚会增加不可靠状态 | 0.97 |
| D11 | marketplace add非零 | 立即失败；全部忽略；警告后继续并以后续install/list裁决 | 记录警告后继续；只有install或dedicated终检失败才终止且保留generic | E8,E15 | 已存在marketplace会产生可恢复非零；无条件忽略又会掩盖真正不可用状态 | 0.95 |

## 4. BDD 行为规格

```gherkin
Feature: Ralph 专用 Nowledge 插件包
  Scenario S1: 插件可被 Claude 严格校验
    Given 专用插件目录存在
    When 运行 claude plugin validate --strict
    Then plugin name/version/commands/skill 均合法

  Scenario S2: 插件不会自动捕获任何 Claude 生命周期
    Given 专用插件 manifest 与资源目录
    When contract test 检查插件的激活入口
    Then manifest 不声明 hooks 且插件目录不存在 hook 脚本
    And session start/stop/subagent stop/session end 没有可注册的自动动作

  Scenario S3: 有查询词时执行有界 memory search
    Given operator 或 agent 提供非空查询词
    When 调用 namespaced search command
    Then 执行 nmem --json m search 并把结果限制为 5 条
    And 不调用任何写命令或 Working Memory

  Scenario S4: 空查询词不会调用 nmem
    Given search command 没有收到查询词
    When command 被调用
    Then command合同要求显示带 query 的使用说明
    And 不生成或建议任何 nmem 查询命令

  Scenario S5: status 只检查连接状态
    Given nmem CLI 可执行
    When 调用 namespaced status command
    Then 只执行 nmem --json status
    And 服务错误被原样归类为可操作故障而不降级为写入

  Scenario S6: 文档完整说明插件设计与使用合同
    Given 新 operator 不阅读插件或 installer 源码
    When 阅读独立设计文档和 plugin README
    Then 能判断人工 Claude 与 Ralph child 分别应使用哪个 scope 的插件
    And 能手动完成专用插件的 project-scope 安装、验证、查询、排障和卸载
    And 能确认插件不会自动保存、读取 Working Memory 或承担 distill
```

```gherkin
Feature: 项目级插件迁移
  Scenario S7: 首次安装迁移 project generic
    Given user generic 存在且 project generic 存在
    When operator 运行 installer
    Then dedicated project 先安装并验证成功
    And project generic 随后以 keep-data 移除
    And user generic 条目保持不变

  Scenario S8: 已完成安装再次执行幂等
    Given dedicated project 已存在且 project generic 不存在
    When 再次运行 installer
    Then 不执行错误卸载并最终验证成功

  Scenario S9: dry-run 不改变外部状态
    Given 任意初始 plugin list
    When 使用 --dry-run
    Then 只报告拟执行动作且不调用变更命令

  Scenario S10: 不可解析的 scope 状态 fail-closed
    Given plugin list 输出非法 JSON 或缺少 scope
    When installer 评估迁移
    Then 非零退出且不执行 uninstall

  Scenario S11: dedicated 安装失败时保留 project generic
    Given project generic 存在且 dedicated project 不存在
    When dedicated install 或安装后验证失败
    Then installer 非零退出
    And project generic 的 uninstall 调用次数为 0

  Scenario S12: generic 卸载失败时报告可恢复的部分状态
    Given dedicated project 已安装并验证且 project generic 存在
    When project generic 的 keep-data uninstall 失败
    Then installer 非零退出并报告 dedicated 与 generic 并存
    And user scope 条目保持不变

  Scenario S13: 初始不存在 user generic 时不得创建它
    Given plugin list 不含 user generic
    When operator 运行 installer
    Then dedicated project 达到目标状态
    And plugin list 仍不含 user generic

  Scenario S14: README 完整说明 installer 操作与恢复
    Given scope-aware installer 已完成
    When operator 只阅读 plugin README
    Then 能执行正常安装、dry-run、安装后验证和幂等重跑
    And 能处理 dedicated 安装失败与 generic 卸载失败
    And 能卸载或重装 dedicated project plugin 且不删除 nmem 数据

  Scenario S15: marketplace add 非零但 dedicated 可安装时继续
    Given local marketplace 已经在当前project声明
    When marketplace add 返回非零而dedicated install和终检成功
    Then installer记录add警告并继续迁移
    And 最终状态仍满足dedicated project存在且generic project不存在

  Scenario S16: 其他project条目不会被误迁移
    Given plugin list包含另一个projectPath下的generic或dedicated条目
    When installer为target project规划动作
    Then 这些条目不计入target project初始或最终状态
    And 不因这些条目执行uninstall或跳过install
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口/层级 | 副作用与不变量 | E2E |
|---|---|---|---|---|
| S1–S2 | manifest 可解析；hooks 声明和hook文件集合均为空 | `plugins/nowledge-mem-ralph/tests/test_plugin_contract.py`，contract | root plugin 不变；不能用一句文案grep代替结构断言 | plugin validate smoke |
| S3 | search frontmatter声明必填query；命令只含JSON memory search与`--limit 5`；写能力denylist为空 | 同上，contract | 可选thread追溯必须同时满足“用户需要原对话/来源不足”与有界参数；默认不触发 | 否 |
| S4 | command frontmatter/body明确空输入停止；解析出的空输入分支不含nmem命令 | 同上，command prompt contract | 不把空字符串当全库查询；因Claude command是agent-facing prompt，不虚构可执行wrapper | 否 |
| S5 | 解析出的唯一status命令为`nmem --json status`；错误处理指令要求保留失败信息并停止 | 同上，command prompt contract | 失败时不建议第二个nmem子命令；人工smoke确认错误可读 | 否 |
| S6 | 设计文档覆盖边界/状态机/安全/版本/追踪；README覆盖选型、前置、手动project安装/验证、search/status、nmem排障、dedicated卸载、隐私 | 同上，documentation contract + 人工审查清单 | U1文档不得声称尚未完成的installer已经可用，也不得声称依赖runtime适配计划 | 否 |
| S7 | fake Claude argv/cwd顺序为add→install→list验证→uninstall keep-data→final list；projectPath等于target canonical root；原user entry深比较相等 | `scripts/tests/test_setup_nowledge_ralph.py`，integration | 仅target project scope变更 | 否 |
| S8 | 重跑不重复install/uninstall且exit 0 | 同上，idempotency | 最终列表一致 | 否 |
| S9 | mutation command count=0；输出按真实初始状态生成 | 同上 | 不调用add/install/uninstall | 否 |
| S10–S12 | exit非零；分别断言uninstall未调用或部分状态被准确报告 | 同上，fault injection | 不猜测scope、不伪造回滚成功 | 否 |
| S13 | user generic初始和最终均不存在 | 同上，negative state | installer不承担user scope安装 | 否 |
| S14 | README中的installer命令与真实`--help`和脚本参数一致；覆盖dry-run、终检、两种部分失败状态和恢复命令 | 同上，documentation contract + dry-run smoke | 只在U2脚本Green后补充，不得在U1提前宣称可用 | 否 |
| S15 | add非零时记录warning，随后install/list成功则exit 0并完成迁移 | 同上，fault recovery | 不能因warning跳过dedicated终检 | 否 |
| S16 | 相同id但projectPath不匹配的fixture不触发target状态分支 | 同上，scope isolation | 不修改其他project | 否 |

## 6. 需求—测试追踪矩阵

| Requirement | Scenario | 验收测试 | 单元/契约 | 集成 | Evidence | Unit |
|---|---|---|---|---|---|---|
| R1 | S1,S2 | plugin validate/manifest与resource assertions | plugin contract | validate smoke | E2,E9 | U1 |
| R2 | S2,S3,S4,S5 | command argv、空输入、status错误、allow/deny assertions | command prompt contract | plugin validate/人工smoke | E3,E4,E13,E14 | U1 |
| R3 | S1 | name-based marketplace lookup | marketplace tests | catalog regression | E9,E10 | U1 |
| R4 | S7,S10,S11,S12,S16 | argv/cwd/id/scope/projectPath/order assertions | command planner | fake Claude | E5,E8,E15 | U2 |
| R5 | S7,S8,S9,S10,S11,S12,S13,S14,S15 | first/repeat/dry/error/user-absent/docs/add-warning matrix | state parser | installer subprocess | E5,E8,E11,E15 | U2 |
| R6 | S6 | design documentation contract/checklist | document structure contract | 人工设计审查 | E12 | U1 |
| R7 | S6,S14 | README plugin/installer usage contracts | document structure contract | plugin validate + installer dry-run | E12,E13,E15 | U1,U2 |
| R8 | S2,S3,S4,S5 | read-only argv allowlist + write/WM denylist | command prompt contract | plugin validate/人工smoke | E3,E13,E14 | U1 |

## 7. 严格串行开发单元

### U1：无自动写入的专用插件包

1. **Unit 目标**：Claude 能安装并校验一个只读、无 hooks 的专用插件。
2. **对应需求与 Scenario**：R1–R3、R6–R8；S1–S6；D1,D3,D5,D8,D9；E2–E4,E9,E10,E12–E14。
3. **外部可观察结果**：plugin validate 通过；commands/skill 可发现且查询合同有界；任何 session 事件无自动命令；operator 可仅凭设计文档和README正确选型与操作。
4. **当前行为基线**：专用目录不存在；root marketplace 只有 root plugin；先写失败 contract。
5. **输入与输出**：输入插件目录、search query或status动作；输出合法manifest、2 commands、1 skill、README、独立设计文档，以及JSON只读查询合同；无runtime状态变化。
6. **修改位置**：计划新增 `plugins/nowledge-mem-ralph/.claude-plugin/plugin.json`（name/version/description）、`README.md`（operator操作合同）、`commands/search.md`（必填query与有界search）、`commands/status.md`（JSON健康检查）、`skills/search-memory/SKILL.md`（何时查/何时停止/只读allowlist）、`tests/test_plugin_contract.py`（manifest/capability/docs contract）、`.ralph/specs/nowledge-mem-ralph-plugin-design.md`（稳定设计合同）；修改 `.claude-plugin/marketplace.json`（第二个name-based entry）、`skills/tests/test_install.py` 与 `skills/tests/test_e2e_bootstrap_e2e.py`（移除`plugins[0]`位置假设）。不改 `PUBLIC_SKILLS`、root plugin内容或Rust代码。
7. **可依赖能力**：Claude plugin manifest/marketplace格式、nmem 0.10.53 search/status CLI。
8. **禁止依赖未来能力**：不得实现 installer、Ralph config/runtime、Thread save/distill。
9. **验收测试**：`test_manifest_and_marketplace_expose_dedicated_plugin_by_name`验证name=`nowledge-mem-ralph`、version=`0.1.0`、source与资源；`test_plugin_has_no_lifecycle_entrypoints`验证manifest无hooks且不存在hooks/scripts自动入口；`test_search_contract_is_bounded_and_read_only`验证query必填、JSON memory search、limit=5、仅条件式有界thread search/show；`test_status_contract_is_single_read_only_call`验证JSON status且失败不降级；`test_capability_denylist_has_no_write_or_working_memory`解析所有command/skill代码块并拒绝`t create/append/save/distill`、`m add/update/delete`、`wm read`；`test_design_and_readme_cover_required_contracts`按稳定章节/决策表检查文档覆盖。运行相关pytest与plugin validate。
10. **Acceptance Red**：先新增并运行contract test；有效Red必须是断言报告`plugins/nowledge-mem-ralph/.claude-plugin/plugin.json`不存在，或按name查找marketplace时找不到`nowledge-mem-ralph`。测试未收集、测试文件路径错误、frontmatter parser自身异常或fixture缺失不算有效Red。
11. **单元测试拆分**：manifest schema与无hooks；marketplace按name选择；资源集合；frontmatter parser；shell代码块argv parser；读allowlist/写denylist；design/README稳定章节集合。不得只grep一句文案代替结构验证；不得把自然语言措辞逐字锁死。
12. **Red → Green → Refactor**：manifest/无hooks Red→最小plugin骨架→Green；catalog Red→第二entry与name lookup→Green；search/status contract Red→最小commands→Green；skill allow/deny Red→最小skill→Green；design/README coverage Red→写完整设计与操作说明→Green；最后抽取仅供测试的frontmatter/代码块helper并全量复跑。
13. **最小实现范围**：version 0.1.0；search默认执行`nmem --json m search "$ARGUMENTS" --limit 5`，空query先显示用法；status执行`nmem --json status`；只有追溯原对话确有必要时才允许`t search --limit 5`和`t show --limit 8 --offset 0 --content-limit 1200`，翻页必须由仍缺信息触发；没有hooks/save/WM。设计文档必须包含：问题与目标/非目标、插件—runtime边界、组件树、command/skill合同、信任边界、scope矩阵、installer desired-state与失败状态、数据/隐私、版本策略、需求—测试追踪。U1版README必须包含：人工Claude与Ralph选型表、前置、直接通过local marketplace完成project-scope安装与验证、search/status示例、无自动捕获保证、nmem故障排查、dedicated project卸载且保留数据、隐私说明、与runtime适配计划“相关但不依赖”的说明；不得提前声称U2 installer已经可用。
14. **集成验证**：真实 `claude plugin validate` 解析目录；pytest 解析root marketplace和plugin manifest。
15. **风险驱动测试**：Contract防插件结构漂移；negative capability test防未来误加自动捕获；文档contract只锁章节和命令/边界，不锁可演进文案；人工Claude smoke仅检查命令可发现和错误可读，不将LLM措辞作为CI断言。
16. **回归范围**：root plugin PUBLIC_SKILLS、E2E bootstrap catalog、所有 plugin manifest consumers。
17. **预期文件变更**：

    | 位置 | 变更类型 | 变更原因 | Evidence |
    |---|---|---|---|
    | `plugins/nowledge-mem-ralph/.claude-plugin/plugin.json` | 新增插件manifest | 声明独立name/version且不注册hooks | E2,E9 |
    | `plugins/nowledge-mem-ralph/commands/{search,status}.md` | 新增commands | 暴露有界JSON只读查询与状态检查 | E13,E14 |
    | `plugins/nowledge-mem-ralph/skills/search-memory/SKILL.md` | 新增skill | 定义搜索触发、跳过、渐进追溯和停止条件 | E13,E14 |
    | `plugins/nowledge-mem-ralph/README.md` | 新增使用文档 | 提供U1可用的选型、手动安装、使用和安全说明 | E12,E13 |
    | `plugins/nowledge-mem-ralph/tests/test_plugin_contract.py` | 新增contract test | 固定manifest、能力和文档合同 | E9–E14 |
    | `.ralph/specs/nowledge-mem-ralph-plugin-design.md` | 新增设计文档 | 固定跨U1/U2的职责、状态机、信任与测试设计 | E12 |
    | `.claude-plugin/marketplace.json` | 修改marketplace | 新增第二个独立plugin entry | E9,E10 |
    | `skills/tests/test_install.py` | 修改回归测试 | 将首项位置假设改为name-based lookup | E10 |
    | `skills/tests/test_e2e_bootstrap_e2e.py` | 修改回归测试 | 保持多entry marketplace consumer正确 | E10 |
18. **完成标准**：S1–S6全绿；plugin validate和相关pytest通过；设计文档与README逐项通过审查清单；无skip/弱断言；可独立提交。
19. **停止条件**：manifest格式与真实CLI冲突、必须添加hook才能发现能力、root plugin被迫替换时停止并重决策。
20. **风险与注意事项**：Claude command/skill本质是agent-facing指令，不是强制沙箱；结构化allow/deny contract能防插件自身引导写入，但不能限制agent在插件外自行执行任意shell。用无hooks、最小能力面、明确skill停止条件和Ralph自身权限边界缓解；不得在文档中虚假声称“技术上禁止所有写入”。插件内容可能与通用插件命令同名，依靠plugin namespace隔离并以真实validate检测。剩余风险是Claude plugin格式版本漂移。

### U2：scope-aware 安装与迁移

1. **Unit 目标**：installer 只把 target project 从generic project迁移到dedicated project，保留user generic。
2. **对应需求与 Scenario**：R4–R5、R7；S7–S16；D2,D4,D6,D7,D10,D11；E5–E8,E11,E15。
3. **外部可观察结果**：首次/重跑/dry-run/错误路径的命令、cwd、exit和最终plugin list符合合同。
4. **当前行为基线**：脚本添加Community marketplace、安装generic project并用宽泛字符串检查；先用fake Claude固定旧行为后启用新验收。
5. **输入与输出**：target path、dry-run、每次list JSON及mutation退出码；输出按顺序的迁移动作、失败时实际部分状态和最终验证；只改变project plugin state。
6. **修改位置**：修改 `scripts/setup_nowledge_ralph.py` 的plugin inventory解析、desired-state规划、subprocess顺序与终检；新增 `scripts/tests/test_setup_nowledge_ralph.py`；在U1 README真实说明之上补充installer安装、dry-run、终检、部分失败恢复、幂等重跑和卸载/重装说明。相邻nmem安装/健康检查职责保留，设计决策以U1设计文档为SSOT。
7. **可依赖能力**：U1 plugin/marketplace；现脚本command runner/argument parsing；fake executable。
8. **禁止依赖未来能力**：不编辑 `ralph.yml`、`.claude/settings.json`字符串、不调用Ralph runtime。
9. **验收测试**：覆盖target project generic存在/不存在、user generic存在/不存在、其他projectPath同名条目、dedicated已存在、invalid JSON、marketplace add非零后恢复、install失败、dedicated验证失败、uninstall失败、final list失败和dry-run；逐项断言完整argv、cwd、调用顺序/次数、stderr/exit及最终不变量。fake Claude以响应队列模拟每次list发生的真实状态变化，不允许固定返回同一快照掩盖终检错误。
10. **Acceptance Red**：先保留现有脚本通过的characterization test，再启用S7验收；有效Red必须显示实际argv仍是Community URL和`nowledge-mem@nowledge-community --scope project`，或最终inventory缺少dedicated id。fake binary未创建、响应队列耗尽、测试未收集或JSON fixture本身非法（除S10专用fixture）不算有效Red。
11. **单元测试拆分**：JSON shape与完整id/scope/projectPath parser；canonical target path匹配；initial inventory snapshot；desired-state planner；repo-root absolute marketplace source；mutation sequencing；recoverable add warning；pre-uninstall dedicated verifier；final verifier；错误分类和部分状态报告。测试user不变量时比较完整entry而非只比较name。
12. **Red → Green → Refactor**：inventory/path isolation Red→最小parser→Green；desired state/dry-run Red→纯planner→Green；首次安装顺序 Red→add/install/list验证→Green；add-warning恢复 Red→继续install并裁决→Green；generic迁移 Red→验证后uninstall keep-data/final list→Green；rerun与user-absent Red→幂等branches→Green；逐个权威mutation fault Red→fail-closed与部分状态报告→Green；最后Refactor runner/planner边界并复跑全部矩阵。
13. **最小实现范围**：所有命令cwd为target project；marketplace source为repo root绝对路径；project inventory条目必须同时满足完整plugin id、`scope=project`和canonical `projectPath=target root`。mutation固定为：初始JSON inventory→dedicated缺失时执行`marketplace add --scope project <repo-root>`（非零仅warning）→`plugin install nowledge-mem-ralph@ralph-orchestrator --scope project`→重新list并确认target dedicated project；若install/终检失败则停止并保留generic→若target generic存在才执行`plugin uninstall nowledge-mem@nowledge-community --scope project --keep-data`→最终list验证目标状态与user不变量。uninstall失败时不删除dedicated、不声称回滚，非零退出并打印恢复命令。不得创建/卸载user scope或触碰其他projectPath。
14. **集成验证**：真实Python脚本调用fake Claude；可选人工smoke必须先记录scope，且不由自动测试触碰真实用户状态。
15. **风险驱动测试**：Idempotency覆盖重跑；Fault Injection覆盖install/list/uninstall，add非零则覆盖可恢复与后续失败两条组合；negative user mutation断言覆盖user存在与不存在；scope isolation覆盖其他projectPath；路径含空格覆盖argv非shell拼接；state-machine测试覆盖每次list快照，防安装成功但终检读取旧状态。
16. **回归范围**：脚本原nmem检查、target path校验、U1 catalog/validate、skills installer tests。
17. **预期文件变更**：

    | 位置 | 变更类型 | 变更原因 | Evidence |
    |---|---|---|---|
    | `scripts/setup_nowledge_ralph.py` | 修改现有生产脚本 | 从Community安装改为target-aware dedicated迁移状态机 | E5,E8,E15 |
    | `scripts/tests/test_setup_nowledge_ralph.py` | 新增集成测试 | 以fake Claude覆盖scope、顺序、幂等与故障矩阵 | E5,E11,E15 |
    | `plugins/nowledge-mem-ralph/README.md` | 修改使用文档 | 在脚本Green后补充installer、dry-run和恢复说明 | E12,E15 |
18. **完成标准**：S7–S16全绿且S6的插件使用说明仍准确；U1回归、pytest、dry-run smoke通过；无真实用户或其他project状态污染；可独立提交。
19. **停止条件**：plugin list无法稳定区分scope、local source无法project安装、CLI要求卸载所有scope时停止。
20. **风险与注意事项**：Claude CLI没有跨命令事务；本方案通过“先获得dedicated，再移除generic”保证失败时至少保留一个可用插件。uninstall失败会短暂并存两个project插件，必须明确报告而不是伪装成功。真实Claude版本差异可能改变JSON字段；parser必须拒绝未知shape而不是猜测。剩余风险由受控人工smoke验证。

## 8. Unit 串行依赖图

```text
U1 专用插件包与 marketplace contract
  ↓ U2 只能安装已通过真实 validate 的插件
U2 project-scope installer迁移
```

U2 使用U1的name/source/capability合同；顺序不可交换，否则installer测试只能安装不存在的目标。不得在U1提前实现scope迁移。

## 9. 执行命令清单

| 时机 | 命令 | 验证目的 | 失败后继续？ |
|---|---|---|---|
| U1 Red/Green | `.venv/bin/python -m pytest plugins/nowledge-mem-ralph/tests skills/tests/test_install.py skills/tests/test_e2e_bootstrap_e2e.py -q` | plugin与catalog contract | 否 |
| U1 集成 | `claude plugin validate --strict plugins/nowledge-mem-ralph` | 真实插件解析 | 否 |
| U2 Red/Green | `.venv/bin/python -m pytest scripts/tests/test_setup_nowledge_ralph.py -q` | installer状态矩阵 | 否 |
| 相关回归 | `.venv/bin/python -m pytest plugins/nowledge-mem-ralph/tests scripts/tests/test_setup_nowledge_ralph.py skills/tests/test_install.py skills/tests/test_e2e_bootstrap_e2e.py -q` | 两Unit联合 | 否 |
| 文档漂移 | `bash scripts/check-cli-doc-drift.sh --strict` | CLI/文档引用 | 否 |
| 最终回归 | `./scripts/run-tests.sh` | workspace既有行为 | 否 |

本计划不新增Rust生产代码；最终全量仍按仓库硬规则执行，不得用裸 `cargo test -p ralph-cli` 替代。

## 10. 最终质量门禁

- S1–S16、R1–R8全部追踪并通过。
- 插件严格validate；无hooks/save/WM；root plugin与PUBLIC_SKILLS不变。
- installer首次/重跑/dry-run/非法JSON/命令失败均通过；user scope无uninstall。
- Python tests使用`.venv`；doc drift与workspace回归通过。
- 无skip/only/弱断言/无解释snapshot；无Community/Rust/runtime改动。
- 适配计划完全未实施时，本计划的所有测试与门禁仍通过；不得读取或断言其文件存在。
- 两个Unit各自完整TDD闭环、独立提交，所有Decision仍≥0.85。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是Roadmap | 是 | 2个行为Unit含真实Red、文件、断言、回归 |
| Executor仍需关键设计决策 | 否 | D1–D11锁定scope、能力、文档布局、查询边界和安装语义 |
| 所有文件和接口有证据 | 是 | 现有路径见E；新路径均标记计划新增 |
| 关键决策置信度≥0.85 | 是 | 最低0.93 |
| 未处理低置信度假设 | 否 | 无待验证假设 |
| 每Unit一个可观察行为 | 是 | 插件包、scope迁移各一项 |
| 每Unit可独立验证 | 是 | pytest/validate边界明确 |
| 每Unit有真实Red | 是 | 第10项明确能力缺失失败 |
| 每Unit含回归范围 | 是 | 第16项明确 |
| 存在未来Unit依赖 | 否 | 只依赖已完成U1 |
| 存在泛化任务描述 | 否 | 行为、文件、断言均具体 |
| Scenario追踪到测试和Unit | 是 | 第5/6节 |
| 关键决策有Evidence | 是 | D1–D11均引用E |
| 可严格串行执行 | 是 | U1→U2 |

实施中若真实CLI contract与E冲突、Red未触达目标逻辑、需要修改Community/Ralph runtime或任一Decision降至0.85以下，必须停止：记录证据→重新比较→重新决策→修订计划，禁止猜测。
