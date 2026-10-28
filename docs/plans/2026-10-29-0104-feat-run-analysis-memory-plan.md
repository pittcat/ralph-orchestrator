---
title: 运行耗时诊断与记忆管理 - Plan
type: feat
date: 2026-10-29
artifact_contract: ce-unified-plan/v1
product_contract_source: ce-plan-bootstrap
execution: code
---

# 运行耗时诊断与记忆管理 - Plan

## Goal Capsule

- Objective：operator 能从一次运行的报告判断时间花在哪里、哪些环节拖慢交付、原因的证据是否充分，以及下一步应该优化什么。
- Means：关联运行记录、确定性耗时计算、证据驱动分析、独立 HTML 报告和有预算的 memory 选择。
- Authority：本计划 Product Contract 定义行为；仓库运行契约和权限规则约束实现。
- Execution profile：先做可重放采集与计算，再接报告和 skill；memory 独立实现后汇入报告。
- Stop conditions：身份无法确认时保留未知，不猜测归属；证据不足时不输出确定根因。
- Delivery：实施者完成各单元和验证后交付代码及样例报告；本计划不授权直接启动实现、发布或外部服务接入。

---

## Product Contract

### Summary

覆盖前述事项 1–4 和 6：运行记录关联、耗时拆分、瓶颈分析、HTML 诊断报告、memory 选择及有效性管理。
让诊断 skill 消费可复核的数据并生成交互报告。

### Problem Frame

当前记录分散，operator 难以区分等待、agent 执行、工具工作与返工的时间成本。
现有失败归因不能直接证明性能瓶颈，更不能从单次慢调用推断模型选择错误。
本地 memory 自动注入主要加载可见条目后截断，默认预算为无限，尚需任务相关性选择与有效性管理。

### Requirements

**关联与采集**

- R1. 新采集记录关联 loop、session、activation、job、attempt 和 stage；plan/unit/task 身份在适用时记录，不适用与缺失分别表示。
- R2. 并行、重试、重启后的执行身份不会混用；业务事件和产物引用能够定位到对应执行。
- R3. 记录依赖等待、容量等待、启动、agent 执行、工具执行、review/verify、集成的边界与结果；后端未暴露的细节明确为 unavailable。
- R4. 采集失败不改变业务接纳、调度或终态语义；报告显示采集覆盖和缺口。

**时间与解释**

- R5. 区分整轮经过时间、累计工作时间、父子区间的包含关系与并行重叠，未知时间单列。
- R6. 展示关键路径和就绪后的等待，依赖或边界缺失时只输出可支持的部分结果。
- R7. 瓶颈结论分别标记观测事实、候选原因、已验证原因和所需验证；模型、服务、工具环境、preset 编排和 runtime 分开分析。
- R8. 每条优化建议关联证据和验证方法；单次耗时与 memory 引用都不能直接证明因果效果。

**报告**

- R9. 生成无需服务和网络即可打开的 HTML，包含总览、热力图、时间线、瓶颈建议、memory 和证据缺口。
- R10. 支持按 Unit/hat/attempt 筛选、切换绝对耗时与占比、展开证据；未知格子有独立样式。
- R11. 数字由固定程序计算，skill 的文字分析与计算事实分开保存，不能覆盖统计值。
- R12. 报告不执行产物中的脚本或命令，不嵌入 job_token、完整 prompt 或原始工具输出；离线证据摘要足够理解结论。

**Memory**

- R13. 在权限过滤后按当前任务相关性选择完整条目并施加有限预算；自动注入与显式 prime 复用选择逻辑。
- R14. 支持适用范围、来源证据、最近验证时间、失效和替代关系；失效或被替代条目不自动注入。
- R15. 记录本轮选择和注入的 memory ID、选择理由、预算消耗及排除计数；引用未知时不能宣称已被使用或有效。
- R16. 原有 private/shared 权限继续生效，运行状态不转存为长期经验；外部插件的写入审批与去重责任不迁入 runtime。

### Scope Boundaries

- 本次包含正常、失败返工、并行、重启恢复和缺失证据的报告路径。
- Deferred to Follow-Up Work：Loki/Grafana、OTel exporter、跨运行模型对照实验、向量数据库和自动调参。
- Considered and not built：自动更换模型、自动修改 preset、自动执行报告建议；报告可供人工评估，不需要新增自动控制面。
- 不重写现有失败 causal 规则，不将性能分类塞入其封闭责任域枚举。

### Acceptance Examples

- AE1. 三个 job 各执行 10 分钟且完全重叠：经过时间为 10 分钟，累计 job 时间为 30 分钟。Covers R5。
- AE2. agent 总耗时 20 分钟，工具区间覆盖 15 分钟：报告展示包含关系，不把全部时间标成模型推理。Covers R3, R5。
- AE3. attempt 1 中断、attempt 2 成功：两次记录和产物分别归属，未闭合区间标记中断或未知。Covers R2。
- AE4. 后端只有纯文本输出：保留 agent 总耗时，模型请求和工具细分显示 unavailable。Covers R3, R7。
- AE5. 有相关共享条目、其他 hat 私有条目和已失效条目：只在权限内选择有效条目，并符合预算。Covers R13–R16。

---

## Planning Contract

### Research Basis

- `crates/ralph-core/src/diagnostics/runtime_trace.rs` 已有 sequence、phase、causal loop/iteration，作为关联基础。
- `crates/ralph-core/src/diagnostics/performance.rs` 已定义粗粒度指标；本次源码搜索到的 `log_performance` 调用均为测试，实施时必须接通生产入口。
- `crates/ralph-adapters/src/stream_handler.rs` 提供 tool call/result 和 session completion 回调；不能假设每个后端都有同等覆盖。
- `crates/ralph-cli/src/loop_runner/execution.rs` 有不同执行路径和结果转换；headless 路径中的零 token/cost 不应被报告当成测得的真实零值。
- `crates/ralph-cli/src/loop_runner/runtime_job/worker.rs` 已测量 job elapsed，DAG identity 来自 runtime job/JobContext。
- `crates/ralph-core/src/diagnosis/causal/domain.rs` 是失败责任域，性能分析使用独立模型。
- `crates/ralph-core/src/event_loop/prompt_injection.rs` 与 `crates/ralph-cli/src/memory.rs` 有不同注入入口，必须统一选择语义。
- `skills/ralph-run-diagnosis/SKILL.md` 当前只允许最终 Markdown，实施时需同步修改产物规则、模板及测试。
- `docs/solutions/state-management/2026-08-02-disable-mode-expected-observation-artifacts.md` 要求区分采集关闭与机制失效。

### Key Technical Decisions

- KTD1. 复用现有诊断 bundle，新增版本化 timing sidecar 与 performance report；不新增业务账本或调度权威。Governs R1–R4。
- KTD2. 同进程耗时用单调时钟，跨进程用 UTC 定位并记录 clock/process identity；重启开启新 clock epoch，禁止直接相减不同进程的单调时间。Governs R2, R5。
- KTD3. 记录 span ID、parent/link、开始结束、状态、观测精度和原始引用；缺 end 的记录是 censored，不补成零。父子和工具并行按区间并集计算 exclusive time。Governs R3, R5。
- KTD4. 从已有 DAG 依赖与实际阶段边界计算最长依赖路径；有缺口时显示 partial，非 DAG 模式仅据实际交接构建路径。工具等待和 agent 包含区间不参与重复加总。Governs R5, R6。
- KTD5. 新增只读性能分析模块，复用已有 causal 输出作为失败背景，不复刻其规则。观察到服务等待与证明模型不适配使用不同证据等级。Governs R7, R8。
- KTD6. 扩展 `ralph diagnose` 的 session 输出，增加 `--performance`、`--format html` 和 HTML 专用 `--analysis-file`。JSON 的 performance 块独立版本化；`--analysis-file` 必须绑定输入摘要和有效证据 ID，只能添加解释。与 ledger-only 模式组合时明确拒绝，缺 timing 的 session 仍生成带缺口报告。Governs R9–R12。
- KTD7. HTML 使用仓库内置 CSS/JS 与安全序列化数据，不依赖 CDN。渲染纯文本使用安全 DOM API，证据链接仅允许经过验证的引用。Governs R9, R12。参考 [OWASP DOM XSS guidance](https://cheatsheetseries.owasp.org/cheatsheets/DOM_based_XSS_Prevention_Cheat_Sheet.html)。
- KTD8. Memory 保留 Markdown 存储，扩展结构化元数据，不新增数据库。缺元数据旧条目标为 unverified，不伪造来源；明确失效或替代的条目排除。新增更新操作沿用文件独占锁并原子替换。Governs R14, R16。
- KTD9. Memory 选择先权限、状态与已有 filter，再以任务文本和 tags 的确定性词项重合排序，最近验证时间及 ID 打破平局；缺任务文本时按可解释的稳定顺序选择。格式化后的完整条目按 UTF-8 bytes 保守估算预算，不截断 JSON 或条目。默认预算改为 2000 个估算 token，显式 0 保留无限语义并在报告显示。Governs R13, R15。
- KTD10. Memory 遥测只记录 ID 和选择统计，不记录私有内容；operator 报告和 agent 查询分别沿用现有权限入口。外部 memory 插件不做双写和自动迁移。Governs R15, R16。

### High-Level Technical Design

采集层写结构化边界，离线分析层生成统一数据，渲染器和 skill 共用该结果。

| Producer | Data | Consumer |
|---|---|---|
| loop/DAG/worker | identity、stage、wait、integration spans | timing reader |
| adapter callbacks | backend/tool spans、可用 usage | timing reader |
| memory selector | selection receipt | performance analysis |
| timing reader | 标准区间、覆盖与异常 | accounting / bottleneck analysis |
| analysis | 版本化 performance JSON | HTML renderer / diagnosis skill |
| diagnosis skill | 带证据的解释 annotations | HTML renderer |

执行顺序：确定身份 → 记录 start → 观察事件与子区间 → 记录 end → bundle 完整性索引 → 离线校验与计算 → skill 解释 → HTML。
区间状态包括 open、completed、interrupted、invalid；重启不将新进程的 end 自动闭合旧进程的 open。

### Module Boundaries and Impact

新增采集模型放 `diagnostics/timing/`，计算和报告放 `diagnosis/performance/`，HTML 模板放独立资源目录。
`loop_runner/inner.rs` 当前 4888 行，禁止继续堆积功能；只保留调用点，新代码进入独立 support 模块。
`diagnosis/reporter.rs` 当前 3834 行，本次不将性能计算或 HTML 实现放入该文件。
全部新增和修改源码文件不得超过 5000 行；完成时记录实际行数。

### Assumptions and Deferred Details

- 默认 HTML 报告目录沿用 `docs/report/`；只保留最终报告，中间 JSON/annotations 位于临时目录，报告内嵌其必要数据。
- 首版不承诺提速百分比，目标是准确定位；报告性能预算在 U8 用固定规模 fixture 验证。
- 各后端内部请求可见性由 U3 实施时验证，未支持能力不阻塞整体验收，但必须显示覆盖差异。

---

## Implementation Units

### U1. Identity and Timing Contract

- Goal：定义关联身份、区间与采集覆盖契约。
- Requirements：R1–R4；Dependencies：无。
- Files：新增 `crates/ralph-core/src/diagnostics/timing/{mod,model,writer,tests}.rs`；修改 `diagnostics/mod.rs`、`diagnostics/input_bundle.rs`、`config/telemetry.rs`。
- Approach：按 KTD1–KTD3 增加 timing capture 配置和 sidecar 索引；新采集默认跟随现有诊断落盘开关，不隐式开启完整输出采集。
- Patterns：runtime trace logger 的 best-effort 写入和 bundle integrity。
- Test scenarios：相同 iteration 的两个 job 身份独立；attempt 重启后无冲突；写入失败仅标 degraded；采集关闭为 not_collected；坏行与缺边界可被 reader 识别。
- Verification：新记录可完整往返解析，bundle 正确声明采集状态。

### U2. Runtime Stage Instrumentation

- Goal：接通等待、执行、集成和恢复相关阶段的真实采集。
- Requirements：R1–R6；Dependencies：U1。
- Files：新增 `crates/ralph-cli/src/loop_runner/timing_support.rs`、`crates/ralph-cli/tests/timing_runtime.rs`；修改 `loop_runner/mod.rs`、`execution.rs`、`runtime_job/worker.rs`、`dag_scheduler/{admission,spawn,integration,terminal_delivery}.rs` 及必要调用点。
- Approach：复用 runtime job identity；记录 admission reason 变化及解除边界，避免每 tick 重复产生相同等待区间；coordinator/isolated/wave/DAG 均有最少 activation 总耗时。
- Patterns：现有 admission decision 和 worker elapsed。
- Test scenarios：有容量且就绪后的等待可计算；资源阻塞不被标成模型耗时；并行 job 无归属串台；真实 worker 被中断再恢复后保持 attempt 区分；timing 开关不改变业务结果。
- Verification：fake backend 驱动真实 runtime 路径，输出可复核边界；不以源码文本断言代替运行验证。

### U3. Backend and Tool Observations

- Goal：采集后端能实际提供的启动、首输出、工具区间和 usage。
- Requirements：R3–R5；Dependencies：U1、U2。
- Files：新增 `crates/ralph-adapters/src/timing_handler.rs`、`crates/ralph-adapters/tests/timing_replay.rs`；修改 `lib.rs`、`cli_executor.rs`、`pty_executor.rs` 及 runner 的 handler 装配入口。
- Approach：在 adapters 层包装 StreamHandler，避免 core 依赖 adapters；各 parser 用录制流验证能力。首文本延迟与服务端 TTFT 分开命名，工具间隙不称为纯模型推理。
- Patterns：现有 StreamHandler 回调、SessionResult 和录制 parser 测试。
- Test scenarios：交错 tool ID 正确配对；并行工具计算并集；缺 result 保留未闭合；纯文本只有总耗时；中断不伪造完成；未提供 usage 与真实零值区分。
- Verification：重放固定输入得到相同观测，所有执行显示模式沿用原有输出行为。

### U4. Deterministic Time Accounting

- Goal：产出可以复算的时间总账和关键路径。
- Requirements：R5、R6；Dependencies：U1–U3。
- Files：新增 `crates/ralph-core/src/diagnosis/performance/{mod,model,reader,accounting,path,tests}.rs`、`crates/ralph-core/tests/performance_accounting.rs`；修改 `diagnosis/mod.rs`。
- Approach：按 KTD3、KTD4 去重并校验区间；同一输入稳定排序，记录输入 digest、计算版本和 evidence ID。已覆盖时间与未知时间采用明确分母，跨 epoch 不做错误减法。
- Patterns：现有离线 reporter 的纯数据分析边界。
- Test scenarios：Covers AE1–AE4；嵌套及重叠区间不重复计数；墙钟回拨不生成负耗时；重复记录只算一次；有环或缺依赖的输入显示 partial；重启间隙单列。
- Verification：人工可计算 fixture 的各项毫秒值完全一致，重复分析的事实部分一致。

### U5. Evidence-Based Bottleneck Analysis

- Goal：从时间事实提出可验证的优化方向。
- Requirements：R7、R8；Dependencies：U4。
- Files：新增 `diagnosis/performance/{bottleneck,evidence}.rs`、`crates/ralph-core/tests/performance_bottlenecks.rs`。
- Approach：给出阶段热点、关键路径贡献和等待原因；与既有 causal 报告建立引用，不扩展或替换其责任域。节省时间只在明确假设下估算，不能简单相加。
- Patterns：现有 evidence refs 和 incomplete 状态处理。
- Test scenarios：慢测试指向工具观测；有空闲容量的就绪等待支持调度候选；无工具细分的慢 agent 保持未知；一次调用不足以判定模型不适配；失败 causal 与性能结论可以并存。
- Verification：每条候选和建议都有有效证据或明确缺口，不能由缺失数据生成确定根因。

### U6. Memory Metadata and Lifecycle

- Goal：持久保存可审计的有效性与替代关系。
- Requirements：R14、R16；Dependencies：无。
- Files：修改 `crates/ralph-core/src/{memory,memory_parser,memory_store}.rs`、`crates/ralph-cli/src/memory.rs`；新增 `crates/ralph-cli/tests/memory_lifecycle.rs`。
- Approach：KTD8 定义可选版本化 JSON 元数据块；新增 `ralph tools memory update <id>` 支持适用范围、来源引用、verified-at、active/invalid 状态和 superseded-by。校验替代目标存在且可见，拒绝自引用与环；检查和写入处于同一独占锁内。
- Patterns：现有 visibility 检查与 FileLock。
- Test scenarios：旧格式往返不丢内容；非 owner 无法修改 private 条目；失效与替代可查询；替代环拒绝且文件不变；并发更新不丢失其他条目；失败写入不留下半份文件。
- Verification：agent/human 权限入口与新操作一致，生命周期可追溯。

### U7. Budgeted Memory Selection and Receipts

- Goal：统一自动注入与 prime 的相关性选择并记录结果。
- Requirements：R13、R15、R16；Dependencies：U1、U6。
- Files：新增 `crates/ralph-core/src/memory_selection.rs`、`crates/ralph-core/tests/memory_selection.rs`；修改 `config/memories.rs`、`event_loop/prompt_injection.rs`、`crates/ralph-cli/src/memory.rs`、`crates/ralph-cli/tests/integration_memory.rs`。
- Approach：按 KTD9 提供共享 selector；prime 新增 `--query`，结构化输出先选条目再序列化。自动注入使用当前 activation 已有任务/触发摘要，不扫描整个仓库。选择 receipt 进入诊断，未采集引用时只报告 injected。
- Patterns：load_visible 和现有 filter；保持现有操作上下文权限。
- Test scenarios：Covers AE5；稳定排序；中文与长条目不截断；零匹配不注入无关内容；默认预算有限；显式 0 可见；JSON prime 可解析；相同上下文两入口选择一致；receipt 不泄漏私有正文。
- Verification：最终格式化内容符合预算计算，输入/输出与排除理由可复核。

### U8. Offline HTML and CLI Integration

- Goal：提供统一数据的交互式离线报告。
- Requirements：R9–R12、R15；Dependencies：U4、U5、U7。
- Files：新增 `diagnosis/performance/{html,annotations}.rs`、`crates/ralph-core/data/diagnosis-report/{report.html,report.css,report.js}`、`crates/ralph-cli/tests/diagnose_performance.rs`、`frontend/ralph-web/e2e/diagnosis-report.spec.ts`；修改 `commands/diagnose.rs` 和现有 browser test 配置。
- Approach：落实 KTD6、KTD7；HTML 独立资源不作为 agent skill 注册。annotations 只接受受限 schema，不执行 HTML，绑定事实输入 digest。为离线测试单设无 dashboard server 依赖的 Playwright project。
- Patterns：现有 diagnose session resolution 与 output discipline。
- Test scenarios：四类运行及 legacy 缺数据报告；筛选、占比切换、证据展开正确；无外部请求；恶意标签和 script closing 内容仅显示文本；错误 digest 拒绝；ledger 组合给明确错误。
- Verification：固定 10000 个 span 的报告打开后可筛选和展开，首屏及一次筛选各在参考机器 2 秒内完成，记录机器和浏览器版本；数值与 JSON 一致。

### U9. Diagnosis Skill and Documentation

- Goal：让 operator 用现有诊断 skill 完成采集检查、解释和 HTML 交付。
- Requirements：R7–R12、R15；Dependencies：U8。
- Files：修改 `skills/ralph-run-diagnosis/SKILL.md` 及 `references/{artifact-discovery,artifact-manifest,report-template,verification-pipeline,confidence-rubric}.md`、`skills/tests/test_run_diagnosis_contract.py`、`docs/guide/runtime-diagnosis.md`；新增 `skills/ralph-run-diagnosis/references/performance-report.md`。
- Approach：保留现有失败归因门禁，新增性能分析独立章节；解除“仅最终 Markdown”的冲突，默认生成 HTML，Markdown 为显式选择。中间文件进临时目录。同步通过仓库 skill installer 安装的项目副本。
- Test scenarios：数据完备时生成 HTML；关闭采集时给缺口；受限后端不虚构 TTFT；解释不能改事实数值；历史检索仍保留既有授权边界。
- Verification：对相同录制数据实际执行 skill 评估，逐项检查事实、建议和交互报告；静态 anchor 测试只检查稳定接口，不锁定完整文案。

### U10. Cross-Mode Acceptance and Agent Guide Sync

- Goal：验证整条链路与 agent 可见文档一致。
- Requirements：R1–R16；Dependencies：U1–U9。
- Files：新增 `crates/ralph-cli/tests/run_analysis_e2e.rs` 和最小脱敏 replay fixtures；修改 `crates/ralph-core/data/ralph-tools{,-memories,-cmdref}.md`、`skills/ralph-preset-{author,review}/references/{commands,finding-rubric}.md` 及受影响的可见性说明。
- Approach：真实 runtime 驱动成功、返工、并行、中断恢复；验证诊断只读，不改变业务结果。agent 指南只描述 loop 内可执行 memory 操作，HTML operator 流程放 operator skill。
- Test scenarios：采集开关前后业务事件/任务终态一致；重启旧 attempt 不混入新 attempt；外层 hat 环境污染下 human CLI fixture 正常；跨 worktree memory 读取来自正确 workspace。
- Verification：完成下述 Verification Contract，并检查文件规模、未使用实现与临时产物清理。

---

## Verification Contract

以下命令由实施阶段执行；计划编写阶段不运行测试。
所有 targeted nextest 命令仅对子进程清理 `RALPH_CURRENT_HAT`、`RALPH_CURRENT_LOOP_ID`、`RALPH_EVENTS_FILE`、`RALPH_WAVE_WORKER`、`RALPH_TRIGGERED_HAT`、`RALPH_HATS_SOURCE`、`RALPH_CONFIG`。
spawn human CLI 的测试使用 `common::ralph_bin()`，模拟 agent 时先 scrub 再显式设置。

| Gate | Verification | Completion signal |
|---|---|---|
| Core | `cargo nextest run -p ralph-core -- performance`，以及 memory 相关子集 | U1/U4/U5/U6/U7 精确区间与权限场景通过 |
| Adapters | `cargo nextest run -p ralph-adapters -- timing` | U3 replay 通过，无 live API |
| CLI | `cargo nextest run -p ralph-cli --test diagnose_performance` 及新增 runtime/memory/e2e test targets | U2/U6/U8/U10 真实路径通过 |
| Browser | 新增离线 Playwright project，仅运行 `diagnosis-report.spec.ts` | 离线、交互、数值和脚本注入场景通过 |
| Skill | `.venv/bin/python -m pytest skills/tests/test_run_diagnosis_contract.py`，加 U9 的实际 skill 评估 | 静态接口和行为交付均有证据 |
| CLI docs | 新命令 `--help` 冒烟、`scripts/check-cli-doc-drift.sh`，复核 data 文档内源码引用 | 参数与指南一致 |
| Full baseline | `./scripts/run-tests.sh` | 两阶段 nextest 与 doctest 通过；仅时序 flake 才用仓库规定 serial fallback |
| Hygiene | 检查修改源码行数、最终 diff 与临时目录 | 无源码超过 5000 行，无过程产物或弃用实验代码 |

本计划不修改 preset event 拓扑和 schema；若实施必须改动，先补充影响与对应 preset/schema 校验，不把它当作顺手修改。

---

## Definition of Done

- U1–U10 的验收证据可定位，所有 R-ID 均有覆盖。
- 正常完成、失败返工、并行、中断恢复各有可复现的脱敏报告。
- AE1–AE5 的计算和权限结果符合预期。
- operator 能从报告回答主要耗时、关键阻塞、证据强弱和下一步验证动作。
- memory 在权限和预算内选择，失效/替代关系生效，报告不把注入解释成效果证明。
- 全量基线通过，CLI/agent/operator 文档同步，无临时报告、残留草稿和弃用实现进入提交。
