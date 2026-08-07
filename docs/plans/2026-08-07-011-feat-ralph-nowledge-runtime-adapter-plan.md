---
title: Ralph Nowledge Loop 生命周期适配 - Plan
type: feat
date: 2026-08-07
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
deepened: 2026-08-07
---

# Ralph Nowledge Loop 生命周期适配 - Plan

## Goal Capsule

让 Ralph runtime 成为 Nowledge 自动读写的唯一生命周期所有者：显式启用后，loop 在 payload/preset hard gate 通过且 backend 启动前检索相关 Memory；终态把原始目标、运行摘要和 agent 主动整理的结构化 handoff 写入稳定 Thread；只有新 handoff 批次才执行 triage + distill。所有 nmem 运行时失败均有界、可观测、fail-open，不改变原 loop 终态。

本计划只修改 Ralph Rust runtime、agent-facing guide、配置与测试，不创建或安装 Claude Code 插件。专用插件由独立计划 `docs/plans/2026-08-07-010-feat-nowledge-ralph-plugin-plan.md` 负责；该引用只说明职责归属，不是前置依赖。即使插件计划完全未实施，本计划仍通过直接调用 `nmem` CLI 与 fake-nmem 测试独立达到 Definition of Done，默认关闭路径也不要求任何插件存在。严格串行执行 `U1 → U2 → U3 → U4 → U5`。

## 0. 计划状态

- 状态：`READY`；所有关键决策置信度为 `0.90–0.98`。
- 基线：分支 `pittcat-dev`，提交 `434c51c916fbef0b71b9efdc80df8fc4901a62ed`。
- 调查范围：Ralph config/validation、prompt skill injection/preview、run/resume调用链、loop hard gates、LoopContext、summary/handoff、Worktree cleanup、notifications best-effort模式、CLI/core测试；真实nmem 0.10.53 CLI/JSON。
- 已执行验证：源码/测试/Git历史只读检查；`nmem --json status/stats/wm read/t list/search/show/m search`及相关help。
- 尚未执行：未运行测试、构建、Lint或外部写入；均为实施期门禁。
- 阻塞项：无。

## 1. 功能目标

- 业务目标：每次Ralph loop只读取相关、已提炼知识，只保存本轮可复用结论，不保存每个hat原始上下文。
- 用户/调用方：Ralph operator、所有可见hat、后续检索同一仓库知识的会话。
- 当前行为：Ralph只有本地`.ralph/agent/memories.md`；没有Nowledge配置、recall、Thread finalization或distill。通用捕获生成的真实Thread含临时prompt和developer环境，WM已陈旧。
- 目标行为：默认关闭；显式开启后实现“gate后recall→受控prompt→结构化handoff→幂等Thread→条件distill”。
- 行为差异：Ralph从被动被插件抓取，变为主动拥有loop级知识边界。
- 范围：Rust config、loop_runner adapter/lifecycle、builtin agent guide、LoopContext/cleanup、CLI/core测试和配置文档。
- 非目标：Claude插件包装/安装、Community修改、旧503个Thread迁移、WM自动注入、MCP/API client、preset/schema/event、UI、Telegram审批。
- 输入：`nowledge:`配置、objective、repo/loop identity、nmem search/show JSON、summary、可选handoff、TerminationReason。
- 输出：可选`<knowledge-context>`；source=`ralph`的稳定Thread；可选triage/distill；脱敏warning。
- 状态变化：仅enabled=true时访问外部Nowledge；新增临时`.ralph/agent/nowledge-handoff.md`。
- 错误语义：配置错误fail-closed；所有nmem subprocess/JSON/timeout错误fail-open，原prompt或TerminationReason保持。
- 兼容：默认关闭；现有`memories:`语义、CLI、preset/event schema、公开API不变。
- 性能：每loop最多一次recall；context/thread字符有界；recall/finalize分别有timeout。
- 安全：nmem argv直传不用shell；Memory按不可信证据转义；日志不含objective/Memory/handoff全文；不保存system/developer/tool transcript。
- 已知约束：nmem create/append/distill非单事务；append支持idempotency key；outer wrapper是全部`Ok`终态chokepoint。
- 已确认假设：inner只有outer一个生产调用方；summary在outer收到`Ok`前写完；LoopContext可定位repo/agent目录。
- 待验证假设：无。

### 1.1 独立执行合同

- 唯一外部运行依赖是 `nmem` CLI；Ralph runtime不得读取Claude plugin list、marketplace manifest或插件安装状态。
- 所有自动recall/save/distill由Rust runtime直接调用nmem；`plugins/nowledge-mem-ralph/`不存在时，enabled路径仍须在fake/真实nmem可用条件下工作。
- 本计划的测试和完成门禁不得调用 `scripts/setup_nowledge_ralph.py`、`claude plugin install` 或专用plugin validate。
- 本计划可在插件计划之前、之后或完全不执行插件计划的情况下完成；执行顺序不改变R1–R12的验收结果。
- 若实现发现必须修改 `.claude-plugin/marketplace.json`、`scripts/setup_nowledge_ralph.py` 或 `plugins/`，立即停止；不得把该修改吸收到本计划。

### Requirements

- `R1`：新增独立顶层`nowledge:`，默认disabled；不得复用`memories:`。
- `R2`：配置字段固定为`enabled=false`、`space=None`、`recall_limit=5`、`max_context_chars=6000`、`max_thread_chars=30000`、`recall_timeout_seconds=10`、`finalize_timeout_seconds=60`、`distill=true`、`distill_level=swift`；数值必须大于0，level仅swift/guided/expert。
- `R3`：enabled时条件注入`ralph-tools-nowledge`，preview/live一致；guide规定搜索触发、来源查看、禁止raw save及固定handoff格式。
- `R4`：payload/preset hard gate通过、context/loop ID/objective就绪后且backend前，调用一次`nmem --json m search <repo basename + normalized objective> --limit N`；配置了space时，仅向CLI实际支持该参数的`m search`、`t show`、`t create`传播。`t append`和`t distill`只按Thread ID定位，不传不受支持的space参数。
- `R5`：Memory block标为不可信历史证据，对id/title/content做XML文本转义，按Unicode字符边界限制；空结果不注入。
- `R6`：recall命令缺失/非零/timeout/非法JSON时warning并使用原prompt；hard gate拒绝时nmem调用为零。
- `R7`：所有enabled且真正执行的`Ok(TerminationReason)`保存Thread；warmup-only与`RestartRequested`跳过。
- `R8`：终态owner在emit前写固定格式handoff：Outcome、Durable Decisions(Decision/Why/Applies when)、Root Cause and Reusable Constraints、Verification、Unresolved；空节写None，禁止transcript/secret/大段代码。
- `R9`：Thread ID=`ralph-<repo_hash12>-<loop_hash12>`；hash与title/digest规范、marker和分页show规则按第3节D8固定；相同digest不追加，新digest只追加一次。
- `R10`：无handoff仍保存objective+summary审计批次但不distill；新handoff批次成功create/append且distill=true才调用一次`--triage --level`。
- `R11`：show/create/append/distill失败或不确定timeout不改原终态，同次不盲重试；show只有明确not-found才create。
- `R12`：Worktree runtime cleanup归档`nowledge-handoff.md`，防止旧内容被下一loop复用。

## 2. 代码库现状与证据

### 2.1 当前实现入口

- `crates/ralph-cli/src/commands/run.rs`/`resume.rs`调用`loop_runner::run_loop_impl`。
- `loop_runner/entry.rs::run_loop_impl`是outer wrapper；`inner.rs::run_loop_impl_inner`先执行payload/preset gate，再解析prompt、建立context/loop ID，最后经公共termination closure写summary。
- `skill_registry.rs`和`event_loop/mod.rs::SkillInjector::plan_auto_inject`共同控制live/preview skill。
- `LoopContext`集中定义summary/handoff路径；`worktree.rs::clean_worktree_runtime_artifacts`显式归档runtime文件。
- `loop_runner/notifications.rs`提供bounded best-effort终态side effect的相邻模式。

### 2.2 Evidence Ledger

| Evidence | 来源 | 观察结果 | 计划影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `config/mod.rs::RalphConfig`、`ralph_config.rs::validate` | 顶层default/validate模式明确 | 新建独立NowledgeConfig | 高 |
| E2 | `config/memories.rs` | memories是本地markdown、默认enabled | 不复用该配置 | 高 |
| E3 | `skill_registry.rs`、preview tests | builtin skill与条件gate有现成模式 | 新guide沿同源gate | 高 |
| E4 | `entry.rs`/`inner.rs`/`prompt.rs` | gate→prompt→context→loop ID→backend；outer为Ok chokepoint | shared lifecycle接入点确定 | 高 |
| E5 | `summary_writer.rs`、`LoopContext` | summary存在但不足以表达决策缘由 | summary+结构化handoff | 高 |
| E6 | `worktree.rs`与tests | runtime markdown必须显式归档 | 加handoff cleanup | 高 |
| E7 | `notifications.rs` | best-effort side effect不改终态 | finalizer沿用错误语义 | 高 |
| E8 | nmem 0.10.53 help | m search、t show/create/append/idempotency、distill triage/level可用 | CLI contract确定 | 高 |
| E9 | 真实nmem JSON | search memories与thread messages shape已观察 | DTO/parser不靠猜测 | 高 |
| E10 | nmem stats/search/show | 503 Threads且样本含developer环境/临时prompt | 禁止raw transcript | 高 |
| E11 | `nmem wm read` | WM停留2026-05-07 | 不自动注入WM | 高 |
| E12 | OpenClaw connector `capture.js` | 内部session排除、idempotency、门控triage模式 | 采用行为模式 | 中高 |
| E13 | `tests/common::ralph_bin`、AGENTS HARD RULE5 | CLI tests必须scrub外层RALPH env | E2E复用helper | 高 |
| E14 | AGENTS Build/Test | nextest与两阶段全量入口固定 | 执行命令确定 | 高 |
| E15 | capability inventory与prompt visibility refs | prompt能力变化需同步operator技能 | U1文档影响确定 | 高 |

### 2.3 受影响范围

- 生产：计划新增`crates/ralph-core/src/config/nowledge.rs`、`crates/ralph-cli/src/loop_runner/nowledge.rs`、`crates/ralph-core/data/ralph-tools-nowledge.md`；修改config、skill registry/event loop、entry/inner、LoopContext/worktree。
- 测试：core config/preview/worktree；CLI loop_runner与计划新增`crates/ralph-cli/tests/integration_nowledge.rs`。
- 文档：`docs/guide/configuration.md`、author/review两份`prompt-visibility.md`，必要时capability inventory。
- 不影响：`.claude-plugin/marketplace.json`、`scripts/setup_nowledge_ralph.py`、`plugins/`、preset/schema/event/API/UI。

## 3. 决策记录与置信度

| Decision | 问题 | 候选 | 最终选择 | 证据/排除 | 置信度 |
|---|---|---|---|---|---:|
| D1 | 生命周期所有者 | Claude hooks；hat；Ralph loop | Ralph runtime（session-settled: user-directed） | E4,E7,E10；只有loop有完整边界 | 0.98 |
| D2 | 配置 | memories复用；默认开；独立默认关 | 独立`nowledge:`默认关 | E1,E2；语义/兼容分离 | 0.97 |
| D3 | 外部实现 | SDK/MCP；shell；Tokio process | 固定`nmem` binary、argv直传、可注入fake | E8,E9；无新依赖/注入面 | 0.95 |
| D4 | recall来源 | WM；Thread；Memory | 只搜Memory | E10,E11；Memory已提炼 | 0.96 |
| D5 | recall seam | outer先搜；逐hat；inner gate后 | shared`NowledgeLifecycle`，inner gate后单次recall | E4；保留gate错误优先级/避免重复 | 0.96 |
| D6 | prompt信任 | 原样注入；转义并标证据 | XML转义+不可信声明 | E9,E10；防历史内容伪造prompt | 0.94 |
| D7 | 保存材料 | raw transcript；summary；summary+handoff | objective+summary+固定handoff | E5,E10；兼顾审计与知识质量 | 0.94 |
| D8 | Thread/幂等 | 每次新建；稳定ID+marker | repo/loop各SHA-256前12；title=`Ralph: <repo>/<loop≤80>`；digest为带字段名与UTF-8字节长度的termination/objective/summary/handoff规范串SHA-256；marker=`[ralph-summary-sha256:<64hex>]`；分页show+append key=digest | E8,E12；支持resume且规避非法字符/重复 | 0.94 |
| D9 | distill门控 | 每终态；只成功；新handoff | 新handoff批次成功后triage distill | E8,E12；失败根因也有价值，无handoff不提炼 | 0.94 |
| D10 | 错误语义 | fail loop；重试；bounded fail-open | warning、原result、不确定写入同次不重试 | E7；记忆是辅助能力 | 0.96 |
| D11 | space | 自动repo；default；显式且按CLI能力传递 | 配置时原样传给`m search`/`t show`/`t create`；append/distill只用Thread ID | E8；0.10.53的append/distill无space参数，且避免space膨胀 | 0.98 |
| D12 | agent指导 | plugin prompt；builtin条件skill | `ralph-tools-nowledge` | E3,E15；preview/live可审计 | 0.96 |
| D13 | 测试 | 全E2E；全unit；分层 | pure unit+module integration+1条fake-nmem CLI E2E | E13,E14；最低成本真实路径 | 0.96 |

## 4. BDD 行为规格

```gherkin
Feature: Nowledge配置与agent能力
  Scenario S1: 默认关闭保持旧行为
    Given 配置省略nowledge或enabled=false
    When 构建prompt或运行loop
    Then 不注入skill且不调用nmem
  Scenario S2: 非法配置在启动前失败
    Given 数值为零或level非法
    When 解析校验配置
    Then 返回配置错误且backend/nmem均未启动
  Scenario S3: enabled的preview与live一致
    Given nowledge.enabled=true
    When inspect prompt且hat实际运行
    Then 两者都注入ralph-tools-nowledge
```

```gherkin
Feature: Loop启动相关知识检索
  Scenario S4: gate通过且Memory命中
    Given hard gate通过且nmem返回合法memories
    When loop启动
    Then 一次search并注入已转义、有界、不可信证据block
  Scenario S5: 空结果保持原prompt
    Given memories为空
    When loop启动
    Then 无knowledge block且原prompt不变
  Scenario S6: recall失败仍继续
    Given nmem缺失、非零、timeout或非法JSON
    When loop启动
    Then warning且backend收到原prompt
  Scenario S7: hard gate拒绝不访问nmem
    Given payload或preset gate拒绝
    When启动loop
    Then 返回原gate错误且nmem调用为零
  Scenario S8: space按CLI支持范围显式传播
    Given space可选
    When执行search/show/create/append/distill
    Then配置时search/show/create携带space
    And append/distill始终不携带不受支持的space参数
```

```gherkin
Feature: Loop终态Thread与提炼
  Scenario S9: 新handoff创建稳定Thread
    Given summary和新结构化handoff存在
    When loop返回成功或失败终态
    Then Thread含objective/termination/summary/handoff/marker且source=ralph
  Scenario S10: 重复finalize幂等且resume可追加
    Given当前digest已存在
    When再次finalize
    Then不append不distill
    When后续digest变化
    Then只append一个新批次
  Scenario S11: 无handoff只保存审计批次
    Given summary存在而handoff为空或不存在
    When finalize
    Then保存objective+summary但不distill
  Scenario S12: 新handoff才distill
    Given新handoff批次已成功保存且distill=true
    When finalize继续
    Then恰好一次带triage和level的distill
  Scenario S13: finalize失败不改终态
    Given show/create/append/distill失败或timeout
    When loop finalize
    Then原TerminationReason不变且同次不盲重试
  Scenario S14: warmup/restart不写Thread
    Given warmup-only或RestartRequested
    When outer收尾
    Then不执行Thread写入/distill
  Scenario S15: Worktree复用不沿用旧handoff
    Given live agent目录存在旧nowledge-handoff.md
    When cleanup执行
    Then旧文件归档且live路径消失
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 层级/入口 | 风险测试 | E2E |
|---|---|---|---|---|
| S1–S3 | serde默认/错误、preview/live marker与gate | core unit + `tests/inspect_prompt.rs` | Characterization/contract | S3 CLI integration |
| S4–S8 | fake argv、JSON、escaping、Unicode边界、原prompt、zero-call gate | loop_runner unit/integration | Property、fault injection | S4,S6,S7 |
| S9–S14 | ID/title/messages/digest、分页marker、create/append、skip与终态相等 | nowledge adapter + outer wrapper | Idempotency、fault injection、resume differential | 是 |
| S15 | live消失、archive相同、用户文件保留 | worktree unit | data-integrity characterization | 否 |

所有测试断言命令次数/argv、prompt/终态不变量和文件副作用；不得Mock parser、renderer、digest、gate或真实filesystem cleanup，只Fake外部nmem/backend。

## 6. 需求—测试追踪矩阵

| Requirement | Scenario | 单元测试 | 集成/契约 | E2E | Evidence | Unit |
|---|---|---|---|---|---|---|
| R1,R2 | S1,S2 | config defaults/validate | config load | 否 | E1,E2 | U1 |
| R3 | S3 | registry/gate parity | inspect prompt | 是 | E3,E15 | U1 |
| R4,R5 | S4,S5,S8 | query/parser/renderer/argv | inner prompt seam | 是 | E4,E8,E9 | U2 |
| R6 | S6,S7 | error mapping | gate ordering/fake process | 是 | E4,E7 | U2 |
| R7,R8 | S9,S11,S14 | terminal predicate/payload | outer lifecycle | 是 | E4,E5 | U3 |
| R9 | S10 | ID/digest/paging | show/create/append | 是 | E8,E12 | U3 |
| R10 | S11,S12 | distill truth table | finalizer sequence | 是 | E8,E12 | U4 |
| R11 | S13 | timeout/error mapping | result preservation | 是 | E7 | U4 |
| R12 | S15 | cleanup archive | Worktree cleanup | 否 | E6 | U3 |

## 7. 严格串行开发单元

### U1：显式配置与条件agent指南

1. **Unit目标**：默认无Nowledge行为，enabled时preview/live一致注入指南。
2. **对应需求与Scenario**：R1–R3；S1–S3；D2,D12；E1–E3,E15。
3. **外部可观察结果**：旧YAML不变；非法配置启动前报错；inspect prompt显示skill。
4. **当前行为基线**：仅memories/tasks/opac gates；先扩展disabled characterization。
5. **输入输出**：输入nowledge YAML；输出NowledgeConfig/ConfigError/prompt block；无nmem I/O。
6. **修改位置**：计划新增`crates/ralph-core/src/config/nowledge.rs`、`crates/ralph-core/data/ralph-tools-nowledge.md`；修改`config/{mod.rs,ralph_config.rs,error.rs}`、`skill_registry.rs`、`event_loop/mod.rs`及preview tests、`capability_inventory.rs`、author/review两份`references/prompt-visibility.md`。不改commands/finding-rubric/preset。
7. **可依赖能力**：serde default/validation、SkillRegistry、preview/live equivalence。
8. **禁止未来能力**：不执行nmem、不注入Memory、不保存Thread。
9. **验收测试**：default/false无marker；true preview/live同skill；每个零值和非法level错误；默认值精确；guide结构/visibility anchors。
10. **Acceptance Red**：inspect/config测试因字段/skill不存在失败；编译fixture错误不算Red。
11. **单元测试拆分**：serde defaults、数值边界、level enum、registry、enabled gate、preview parity。
12. **TDD顺序**：config Red→类型/default→Green；validation Red→校验→Green；registry Red→register→Green；gate Red→同源注入→Green；anchor Red→文档同步→Green；Refactor。
13. **最小实现范围**：D2字段；guide解释触发/命令/字段来源/停止条件及R8 handoff；无内部函数/ledger/preset专属内容。
14. **集成验证**：真实inspect CLI配置加载；EventLoop live renderer不Mock。
15. **风险驱动测试**：Characterization默认关；Contract preview/live与prompt visibility。
16. **回归范围**：config parse/default、all prompt preview、capability inventory、author/review anchors、prompt size。
17. **预期文件变更**：上述3个计划新增/多个现有core与reference修改；依据E1,E3,E15。
18. **完成标准**：S1–S3、targeted nextest、anchor/doc drift、prompt size、fmt/lint/build全绿；可独立提交。
19. **停止条件**：gate无法同源、需要event/preset/CLI变更、guide必须泄露内部实现时停止。
20. **风险注意**：enabled会增加prompt token；仅条件注入并以size guard检测，剩余风险为所有hat共同承担短guide。

### U2：hard gate后的有界Memory recall

1. **Unit目标**：backend只在gate通过后获得一次有界相关Memory，失败获得原prompt。
2. **对应需求与Scenario**：R4–R6；S4–S8；D3–D6,D11；E4,E7–E11。
3. **外部可观察结果**：fake nmem log与captured backend prompt符合成功/空/失败/gate矩阵。
4. **当前行为基线**：inner gate后解析prompt/context/loop ID，无nmem；先固定原prompt与gate ordering。
5. **输入输出**：objective/repo/space/search JSON；输出enriched或原prompt；无持久化。
6. **修改位置**：计划新增`crates/ralph-cli/src/loop_runner/nowledge.rs`；修改`loop_runner/{mod.rs,entry.rs,inner.rs}`及tests。wrapper创建`Arc<NowledgeLifecycle>`；inner唯一调用在权威loop ID后登记session并recall。公开`run_loop_impl`签名、终态分支和backend adapters不改。
7. **可依赖能力**：U1 config；resolve_prompt_content；Tokio process/time；serde_json；唯一inner caller。
8. **禁止未来能力**：不show/create/append/distill，不读handoff。
9. **验收测试**：3 memories顺序/总长；标签注入攻击转义；空无block；Unicode；missing/exit/timeout/invalid JSON原prompt；gate拒绝零调用；search的显式/缺省space矩阵。
10. **Acceptance Red**：success captured prompt缺block、failure阻断loop或gate前有调用；测试未spawn目标逻辑不算Red。
11. **单元测试拆分**：query normalization、DTO、XML escape/untrusted header/bounds、argv、error warning、lifecycle session登记。
12. **TDD顺序**：query Red→Green；parser Red→Green；renderer Red→Green；session Red→Green；gate order Red→inner seam→Green；failure逐项Red→fail-open→Green；Refactor。
13. **最小实现范围**：一次m search；只id/title/score/content；不读WM/thread；escaped后字符计数；日志不打印全文。
14. **集成验证**：真实inner path+fake nmem/backend，inline/file prompt与run/resume；nmem必须真实spawn。
15. **风险驱动测试**：Property Unicode；malformed JSON corpus；fault injection；bounded timeout。
16. **回归范围**：prompt resolve、payload/preset gates、run/resume shortcut、notifications、Claude settings；disabled/gate拒绝零调用。
17. **预期文件变更**：新增nowledge module/tests；修改entry/mod/inner/test module；依据E4,E8,E9。
18. **完成标准**：S4–S8、U1回归、targeted nextest、fmt/lint/build通过；独立提交。
19. **停止条件**：必须在gate前调用、JSON与E9冲突、只能用shell或需新依赖时停止。
20. **风险注意**：启动延迟受10秒默认上限；Memory仍可能事实过时，block强制要求以当前代码测试验证。

### U3：稳定Thread与handoff生命周期

1. **Unit目标**：真实执行的loop终态留下一个幂等审计/知识批次，旧handoff不污染复用。
2. **对应需求与Scenario**：R7–R9,R12；S9–S11,S14,S15；D7,D8,D10,D11；E4–E9,E12。
3. **外部可观察结果**：fake nmem create或show+append；ID/messages/marker确定；重复零append；cleanup归档。
4. **当前行为基线**：summary已写、普通handoff存在、无Nowledge handoff/Thread；先固定summary-before-wrapper和cleanup。
5. **输入输出**：session objective/context/loop ID、reason、summary、optional handoff；输出source=ralph Thread batch；本地artifact归档。
6. **修改位置**：`LoopContext`新增`nowledge_handoff_path()`；修改`worktree.rs`清理/tests；扩展`loop_runner/nowledge.rs`的lifecycle为ID/payload/show/create/append；`entry.rs`在Ok后finalize。不得改SummaryWriter。
7. **可依赖能力**：U2 lifecycle session/process adapter；sha2既有依赖；nmem show/create/append。
8. **禁止未来能力**：不distill；不保存events/transcript/system/developer/tool prompt。
9. **验收测试**：success/failure create；ID/title/digest规范；warmup/restart零调用；分页marker命中零append；新digest一次append/key；无handoff审计；cleanup归档。
10. **Acceptance Red**：outer测试无Thread，cleanup旧file仍live；fake未启动不算Red。
11. **单元测试拆分**：session snapshot、双hash ID/title、length-prefixed digest、payload bound、paged show、not-found decision、terminal skip、cleanup。
12. **TDD顺序**：ID/digest Red→Green；payload Red→Green；show/paging Red→Green；create Red→Green；append Red→Green；wrapper Red→Green；cleanup Red→Green；Refactor。
13. **最小实现范围**：messages固定user objective+assistant termination/summary/optional handoff/marker；Unicode≤30000；明确not-found才create；errors fail-open。
14. **集成验证**：真实filesystem+fake nmem state machine；同loop resume同ID；parser/digest/cleanup不Mock。
15. **风险驱动测试**：Idempotency、分页、fault injection、data-integrity；同loop已有lock单owner，不额外做并发锁。
16. **回归范围**：所有Ok shortcut、notifications、summary/handoff、Worktree reuse、resume、default disabled。
17. **预期文件变更**：修改loop_context/worktree/entry/nowledge/tests；依据E4–E7。
18. **完成标准**：S9–S11,S14,S15、相关nextest/worktree回归、fmt/lint/build通过；独立提交。
19. **停止条件**：无法获得权威ID/context、summary未在outer前写、发现多owner并发finalize或需事务exactly-once时停止。
20. **风险注意**：create成功响应丢失有歧义；同次不重试，后续稳定ID/show收敛。剩余风险为服务端同ID语义变化。

### U4：只提炼新的结构化handoff

1. **Unit目标**：新handoff批次成功后一次triage distill，其他情况不调用且失败不改终态。
2. **对应需求与Scenario**：R10–R11；S10–S13；D9–D11；E7,E8,E12。
3. **外部可观察结果**：fake log中distill次数/argv精确，Ralph reason/exit与U3相同。
4. **当前行为基线**：U3 lifecycle返回`NoChange|AuditSaved|HandoffSaved`，无distill；先固定枚举合同。
5. **输入输出**：finalize outcome、config/space；输出一次请求或skip；无Ralph状态变化。
6. **修改位置**：仅修改`loop_runner/nowledge.rs`、`entry.rs`及tests；不改core event/state/prompt。
7. **可依赖能力**：U3 marker/outcome；nmem distill triage/level；finalize deadline。
8. **禁止未来能力**：无retry queue/background daemon/人工审批/Memory回写解析。
9. **验收测试**：HandoffSaved+true一次；AuditSaved/NoChange/false零次；level正确且distill argv不含space；timeout/nonzero/invalid JSON warning、终态相等、同次零retry。
10. **Acceptance Red**：HandoffSaved后fake log无distill；必须先证明Thread保存成功。
11. **单元测试拆分**：truth table、argv、deadline剩余预算、error sanitization、result preservation。
12. **TDD顺序**：gate Red→predicate→Green；argv Red→Green；success Red→call→Green；fault逐项Red→fail-open→Green；Refactor。
13. **最小实现范围**：一次`--triage`+level；总60秒默认预算；不消费生成Memory；ambiguous timeout不retry。
14. **集成验证**：outer+fake nmem顺序show/create|append→distill；success/failure reason differential。
15. **风险驱动测试**：State truth table、fault injection、idempotency；单owner无需concurrency test。
16. **回归范围**：U3全部、notifications、failure exit mapping、default disabled。
17. **预期文件变更**：nowledge/entry/tests修改；依据E7,E8,E12。
18. **完成标准**：S10–S13、U3回归、targeted nextest、fmt/lint/build通过；独立提交。
19. **停止条件**：distill contract不支持triage/level、需异步轮询或错误覆盖result时停止。
20. **风险注意**：timeout后服务端可能继续执行；不重试降低重复但不承诺事务exactly-once，文档必须声明。

### U5：真实CLI ATDD与文档回归

1. **Unit目标**：一条真实Ralph CLI+fake nmem路径证明recall→work→save→conditional distill并完成文档同步。
2. **对应需求与Scenario**：R1–R12；S1–S15；D1–D13；E1–E15。
3. **外部可观察结果**：CLI终态正确；fake log无WM/raw save，有一次search、正确Thread写和条件distill；operator可按文档启用。
4. **当前行为基线**：U1–U4只有分层测试；本Unit先写跨层ATDD，不能用source grep代替runtime。
5. **输入输出**：temp repo/YAML/fake backend+nmem/success/failure；输出exit、captured prompt、nmem log、Thread state。
6. **修改位置**：计划新增`crates/ralph-cli/tests/integration_nowledge.rs`；修改`docs/guide/configuration.md`；复核`ralph-tools.md`/cmdref（无新CLI则不改并记录结论）。复用`tests/common`。
7. **可依赖能力**：U1–U4、`common::ralph_bin()`、fake PATH backend。
8. **禁止未来能力**：不修改plugin/installer/preset/API/UI；不新增未定义生产行为。
9. **验收测试**：disabled零nmem；enabled success完整序列；recall失败仍run/save；terminal failure仍save且exit不变；污染RALPH env被scrub；禁止WM/`t save --from claude-code`。
10. **Acceptance Red**：真实CLI调用序列不完整；仅文档grep失败不算主Red。
11. **单元测试拆分**：不新增业务单元；若E2E暴露新纯逻辑，停止并回修所属Unit计划。
12. **TDD顺序**：success E2E Red→接通已有seams→Green；failure E2E Red→最小接线→Green；docs smoke Red→同步→Green；去测试重复Refactor。
13. **最小实现范围**：综合harness和配置文档；不改变U1–U4合同。
14. **集成验证**：真实Ralph binary/filesystem，fake外部process；随后doc drift、prompt size、mock E2E、workspace全量。
15. **风险驱动测试**：Contract argv/JSON、fault injection、disabled differential、禁止命令negative assertion。
16. **回归范围**：全部R/S测试、core/cli、prompt、Worktree、mock E2E、全workspace；变更跨config/prompt/lifecycle/filesystem。
17. **预期文件变更**：新增integration test；修改configuration和确有drift的agent docs；依据E13–E15。
18. **完成标准**：S1–S15、所有第9节门禁通过；无skip/only/弱断言；diff不超范围；独立提交。
19. **停止条件**：E2E要求新CLI/event/preset/dependency、serial fallback仍失败或代码与Evidence冲突时停止重规划。
20. **风险注意**：真实nmem不进CI；剩余版本兼容风险由受控人工smoke覆盖，不污染现有数据。

## 8. Unit 串行依赖图

```text
U1 配置与条件skill
  ↓ U2使用已验证config/gate
U2 gate后Memory recall
  ↓ U3复用lifecycle/process adapter
U3 Thread与handoff幂等
  ↓ U4依赖HandoffSaved/NoChange合同
U4 条件triage/distill
  ↓ U5只组合已完成行为
U5 真实CLI ATDD与文档
```

顺序不可交换；每个后续Unit只依赖已完成前置能力。插件计划不是代码依赖：本计划默认关闭且测试用fake nmem，可独立执行。

## 9. 执行命令清单

| 时机 | 命令 | 目的 | 失败后继续？ |
|---|---|---|---|
| U1 | `cargo nextest run -p ralph-core -- nowledge` | config/skill/gate | 否 |
| U1 CLI | `cargo nextest run -p ralph-cli --test inspect_prompt -- nowledge` | preview真实入口 | 否 |
| U2–U4 | `cargo nextest run -p ralph-cli --bin ralph -- nowledge` | lifecycle adapter | 否 |
| U3 cleanup | `cargo nextest run -p ralph-core -- clean_worktree_runtime_artifacts` | handoff归档 | 否 |
| U5 | `cargo nextest run -p ralph-cli --test integration_nowledge` | 真实CLI+fake nmem | 否 |
| docs | `bash scripts/check-cli-doc-drift.sh --strict` | agent/CLI文档漂移 | 否 |
| prompt | `scripts/guard-prompt-size.sh` | prompt预算 | 否 |
| format | `just fmt-check` | Rust格式 | 否 |
| lint/typecheck | `just lint` | all-targets/all-features clippy | 否 |
| build | `cargo build --workspace` | workspace构建 | 否 |
| E2E | `cargo run -p ralph-e2e -- --mock` | 既有主路径 | 否 |
| 全量 | `./scripts/run-tests.sh` | 两阶段nextest+doctest | 否 |
| flake兜底 | `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` | 仅默认全量疑似竞态时区分flake | serial失败不得继续 |

禁止裸跑`cargo test -p ralph-cli`。每Unit先记录最窄真实Red，关闭前跑相关回归；U5才跑全量。

## 10. 最终质量门禁

- S1–S15、R1–R12全部通过并追踪到Unit。
- config/parser/renderer/session/ID/digest/gate单测、lifecycle integration、cleanup、CLI E2E通过。
- 默认关闭、hard gate零调用、Unicode/XML注入、分页幂等、fault injection、resume differential通过。
- 无WM/raw transcript/`t save --from claude-code`执行路径；日志无全文；space仅显式传播到search/show/create，append/distill不传不支持参数。
- `ralph-tools-nowledge`与prompt visibility/capability inventory同步；确认无需改preset/schema/commands/finding-rubric。
- fmt、lint/typecheck、build、doc drift、prompt size、mock E2E、全量测试通过。
- 无skip/only/弱断言/无解释snapshot；无plugin/Community修改；所有Decision≥0.85；5个Unit严格串行且独立提交。
- 插件计划完全未实施、专用插件目录不存在时，本计划的fake-nmem验收与全部Rust门禁仍通过。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是Roadmap | 是 | 5个纵向行为Unit完整TDD闭环 |
| Executor仍需关键设计决策 | 否 | D1–D13锁定config、seam、信任、幂等、失败语义 |
| 所有文件和接口有证据 | 是 | 现有见E；新增均明确标记 |
| 所有关键决策≥0.85 | 是 | 最低0.94 |
| 未处理低置信度假设 | 否 | 无待验证假设 |
| 每Unit一个可观察行为 | 是 | 配置、recall、Thread、distill、ATDD各一项 |
| 每Unit独立验证 | 是 | 窄命令/断言/提交边界明确 |
| 每Unit有真实Red | 是 | 各Unit第10项 |
| 每Unit含回归范围 | 是 | 各Unit第16项 |
| 存在未来Unit依赖 | 否 | 仅依赖已完成前置Unit |
| 存在泛化任务描述 | 否 | 行为/位置/错误/断言具体 |
| Scenario追踪到测试和Unit | 是 | 第5/6节 |
| 关键决策有Evidence | 是 | D1–D13均引用E |
| 可严格串行执行 | 是 | U1→U2→U3→U4→U5 |

实施中若Evidence冲突、Red未触达目标逻辑、需要新依赖/公开CLI/event/preset/plugin、出现多owner finalizer、回归范围扩大或Decision降至0.85以下，必须停止：记录新证据→更新影响→比较方案→重新决策→修订当前及后续Unit。
