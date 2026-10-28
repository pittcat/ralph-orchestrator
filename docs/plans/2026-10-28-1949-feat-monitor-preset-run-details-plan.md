---
title: feat(monitor): preset 运行详情与流程可视化
type: feat
date: 2026-10-28
baseline: 4c8718b77327d90087d6665e51654b58de90b805
execution: code
status: READY
depends_on: 2026-10-28-1949-feat-monitor-foundation-overview-plan
---

# Preset 运行详情与流程可视化

## 0. 计划状态

`READY` — 所有实施关键决策置信度均 ≥ 0.85，无 BLOCKED 项。

- **代码库基线**：`4c8718b77327d90087d6665e51654b58de90b805`。
- **前置依赖**：Plan 1（foundation-overview）交付的 `.ralph/runs/*.manifest.json`、`ralph inspect run --format json`、`backend/ralph-monitor` 骨架与 `features/monitor` 壳。本计划全部 Unit 假定其已完成；术语沿用：run/run_id/global_run_id/manifest/snapshot/collector/projection。
- **调查范围**：`crates/ralph-core/src/event_logger.rs`、`crates/ralph-cli/src/commands/events.rs`、`crates/ralph-cli/src/commands/inspect.rs`、`crates/ralph-core/src/supervisor/dag_inspect.rs`、`presets/{en,minimal,index.json,manifest.yml}`、`.cursor/rules/multi-hat-isolation.mdc`（forge.unit.* 语义与 JobContext）、`.ralph/` 实况目录、`frontend/ralph-web`（@xyflow/react、react-markdown、index.css、components/ui）、`backend/ralph-web-server/package.json`（zod/better-sqlite3 版本先例）、`crates/ralph-cli/src/loop_runner/inner.rs`（summary.md/诊断落盘点）。
- **已执行验证**：源码读取（见下）；`.ralph/` 实况文件确认（events-*.jsonl、current-events、agent/summary.md、flow-authority.jsonl、ledger.jsonl、dag.db、review/ 均存在）。
- **尚未执行**：`presets/monitor/`（计划新增）；`ralph inspect run` 的 `preset_descriptor` 字段（U1 新增）。
- **阻塞项**：无。

本计划新增术语：

- **monitor descriptor（监控描述符）**：`presets/monitor/<preset>.yml`（builtin）或项目内 `.ralph/monitor/<name>.yml`（custom 显式注册），版本化 sidecar，只决定展示，不决定成功语义或操作可用性。
- **source sequence**：monitor 为每条事件分配的持久单调序号（`run_id, seq`），排序依据 =（events 文件序，文件内字节偏移）。
- **timeline**：`/runs/:id/events` 按 source sequence 分页返回的事件流。
- **artifact**：run 关联的允许根目录内的可读证据文件（summary.md、review 产物、plan 文件等）。

## 1. 功能目标

| R-ID | 需求（可观察行为） | 行为差异（当前 → 目标） |
| ---- | ---- | ---- |
| R1 | 详情页展示运行阶段、当前 hat、Unit/task、轮次、耗时与**有证据**的进度；无可靠分母时显示阶段/轮次而非伪造百分比 | 现状：monitor 仅快照级 state（Plan 1）→ 目标：`/runs/:id/progress` 返回 `stage/current_hat/iteration/elapsed/units{done,total?,source}`，全部附证据来源 |
| R2 | 线性、循环、DAG preset 共享同一详情框架；无描述符 preset 回退到通用事件+日志视图 | 现状：无详情框架 → 目标：descriptor 驱动 stage 映射；unknown preset → `descriptor:null` 通用视图 |
| R3 | 日志可搜索、可暂停跟随、可加载历史；事件时间线关联状态与产物 | 现状：无事件/日志 API → 目标：`/runs/:id/events?cursor&limit&q&hat&topic` 分页；`/logs` 派生视图；时间线条目可跳对应产物 |
| R4 | plan、报告、验证结果可安全预览与文件定位；产物缺失给出明确提示 | 现状：无产物通道 → 目标：`/runs/:id/artifacts`（索引+内容），拒绝路径穿越/符号链接越界/超尺寸；缺失→typed `artifact_not_found` |

范围：详情页只读观测（含 DAG 图）。非目标：事件回写/emit（Plan 5）、SSE 实时流（Plan 3，本计划用轮询增量）、通知（Plan 4）、`.ralph/diagnostics/*` 深层诊断摄取（保留给后续）。

输入：`.ralph/events-*.jsonl`（经 `inspect run` 输出的 `events_file` 与目录枚举）、manifest 字段（preset/plan_path）、descriptor 文件。输出：REST JSON + React 详情页。

错误语义：descriptor 版本未知→`descriptor:null`+`diagnostic`；事件行坏 JSON→跳过并计入 `evidence.parse_errors`；产物路径越界→`403 path_outside_allowed_roots`；产物不存在→`404 artifact_not_found`（UI 显示"尚未生成"）。

兼容要求：descriptor 是纯新增目录 `presets/monitor/`，**不改** `presets/en/*.yml`、manifest、index.json、zsh completions（E16 四点同步规则不适用——不动 builtin preset）；不改 `inspect run` v1 字段（只加 `preset_descriptor` 可选字段，JSON 消费者向后兼容）。

性能：事件 ingest 增量 byte-cursor；timeline 分页默认 200；日志虚拟化窗口渲染；DAG 组件按需 `React.lazy`+`Suspense`。

安全：产物 allowlist 根集合 = `{workspace/.ralph, repo_root/docs, manifest.plan_path 所在目录}`；realpath 校验拒绝越界与逃逸 symlink；内容仅 `text/*`，尺寸上限 512 KiB；前端 Markdown 渲染用 react-markdown **不启** rehype-raw（默认禁 raw HTML，E15 版本 10.1 默认 `urlTransform` 过滤危险 URL）。

已确认假设：A1 `forge.unit.*` topic payload 携带 unit/plan 身份字段（E10 JobContext/规则文件）；A2 事件 payload 可能被截断 50000 字符（E9）→ UI 显示截断标记不尝试复原；A3 `inspect run` 的 `events_file` 字段已在 Plan 1 U1 交付。

待验证假设：无。

## 2. 代码库现状与证据

### 2.1 当前实现入口与调用链

```text
# 事件生产（runtime）
hat emit → EventLogger → .ralph/events-YYYYMMDD-HHMMSS.jsonl   # EventRecord{ts,iteration,hat,topic,triggered?,payload,blocked_count?,wave_*,phase?}
         → .ralph/current-events（marker 指向当前文件）         # LoopContext::resolve_events_path
         → hat-channel .ralph/agent/events-hat-<hat>-<loop>-<n>.jsonl

# 事件消费（现状）
ralph events --file/--last/--topic/--iteration/--format/--events-source   # commands/events.rs → EventHistory::read_all

# DAG 观测（现状）
ralph inspect loop --format json → scheduler 块                 # dag_inspect.rs::SchedulerInspectSummary
                                                               # {scheduler_mode, plan_keys, total_observations, admitted_total, blocked_total}

# 终态产物（现状）
handle_termination → .ralph/agent/summary.md                    # loop_runner/inner.rs SummaryWriter
preset review 阶段 → .ralph/review/<plan>/…                      # 实况目录存在
DAG 运行态 → .ralph/dag.db（SQLite）                            # supervisor 模式
```

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
| ---- | ---- | ---- | ---- | ---- |
| E1 | `crates/ralph-core/src/event_logger.rs` | `EventRecord` 字段全集；`EventHistory{read_all,read_last,filter_by_topic,filter_by_iteration}` 整文件读；payload 截断 50000 字符 | monitor 需自做增量 ingest（byte-cursor），EventHistory 无增量接口；payload 可能截断→渲染层标 `truncated` | 高 |
| E2 | `crates/ralph-cli/src/commands/events.rs` | `--events-source auto|main|hat-channel`；`--file` 可指定任意 JSONL；`--format json`；`--clear --confirm` | events 文件读取是受支持通道；hat-channel 是 per-hat 副本，timeline 以 main ledger 为准 | 高 |
| E3 | `crates/ralph-cli/src/commands/inspect.rs` + Plan1-U1 | `inspect run` 已输出 `events_file`/`preset`/`plan_path`/`manifest` 字段；JSON=SSOT | descriptor 解析并入 `inspect run`（新可选字段），monitor 不直接碰 runtime 文件布局 | 高 |
| E4 | `.cursor/rules/multi-hat-isolation.mdc` + CLAUDE.md | DAG 模式 hat 可见协调 topic 为 `forge.unit.*` 族（ready/executed/failed/correction）；`forge.wave.*` 已弃用；JobContext typed 字段含 `plan_key/unit_key/task_key/stage/attempt` | DAG 进度与图数据从 `forge.unit.*` 事件 payload 提取，禁止用 `forge.wave.*` 作新订阅语义 | 高 |
| E5 | `crates/ralph-core/src/supervisor/dag_inspect.rs` | `SchedulerInspectSummary{scheduler_mode,plan_keys,total_observations,admitted_total,blocked_total}` 由 `inspect loop --format json` 在 mode≠wave 时输出；wave 模式无该键 | 活动 DAG run 可借 `inspect run` 增加同源 scheduler 块；历史 run 无 scheduler→从 `forge.unit.*` 事件重放构图 | 高 |
| E6 | `presets/en/ce-executor-pipeline.yml` + `presets/index.json` + `presets/manifest.yml` | 线性主路径 topic 链：`work.start→plan.ready→work.done→stabilization.done→review.{goalalign,correctness,testing,maintainability,standards,adversarial}.done→review.synthesized→review.complete→fix.done→align.done→report.done`+`LOOP_COMPLETE`；builtin 枚举 10 个 | ce-executor-pipeline sidecar 的 stage 映射直接来自此 topic 序列；builtin 集合=sidecar 覆盖清单 | 高 |
| E7 | `presets/`（目录枚举） | **不存在** `presets/monitor/` | sidecar 为新增目录；命名 `monitor-descriptor/v1` | 高 |
| E8 | `crates/ralph-cli/src/loop_runner/inner.rs`（handle_termination）+ `.ralph/agent/summary.md` 实况 | 终态统一写 `summary.md`；另有 `recovery.jsonl`、`diagnostics/`、`ledger.jsonl`、`flow-authority.jsonl` | artifact 索引的已知位置清单含 `.ralph/agent/summary.md` 等 | 高 |
| E9 | `frontend/ralph-web/package.json` + `src/index.css` + e2e 先例 | `@xyflow/react 12.10`（React Flow）已在依赖；`react-markdown 10.1`（默认禁 raw HTML、URL 过滤）；playwright route-intercept 先例；`LogViewer.tsx` 已有日志组件 | DAG 图零新依赖；Markdown 默认安全，需显式 `urlTransform` 白名单收紧；日志组件可复用或按 monitor 契约重写（进入 U4 时读 LogViewer 定边界） | 高 |
| E10 | `crates/ralph-cli/tests/common/mod.rs` + `scripts/run-tests.sh` | nextest 规则+env scrub helper | Rust 侧 descriptor resolver 测试走 nextest | 高 |
| E11 | `crates/ralph-api/src/preset_domain.rs` | preset 枚举：builtin(`builtin:<name>`) / directory(`.ralph/hats/*.yml`→`directory:<stem>`) / collection(`collection:<id>`) | descriptor 解析按同三源语义：builtin name→sidecar；directory/collection→custom 注册或 fallback | 高 |
| E12 | `backend/ralph-monitor`（Plan 1 U2/U3 产物） | `migrations/`、`collector` 事务 ingest、`source_cursors` 表、`src/ralph/client.ts` | 事件 ingest 复用同一事务与 cursor 表扩展（`event_cursors` 新表）；不新建第二套采集循环 | 高 |
| E13 | `crates/ralph-cli/src/commands/emit/mod.rs` | `ralph emit --file` 可向 events ledger 写事件 | 测试 fixture 可用真实 `ralph emit` 造事件文件（fake ralph 同样可手写 JSONL） | 中 |
| E14 | `.ralph/` 实况（本仓库） | `events-*.jsonl`、`current-events`、`current-hat-events`、`agent/events-hat-*`、`dag.db`、`supervisor.db`、`review/`、`post-merge/`、`planning-sessions/` | artifact allowlist 与目录枚举依据真实布局 | 高 |
| E15 | `docs/achieved/plan/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md` | 详细计划结构先例 | 沿用 | 高 |

### 2.3 受影响范围

**修改的既有文件**

- `crates/ralph-cli/src/commands/inspect.rs`（或 Plan1 新增的 `inspect/run.rs`）：snapshot 增加可选 `preset_descriptor` 字段输出。
- `crates/ralph-cli/tests/integration_inspect_run.rs`：补 descriptor 断言用例。
- `backend/ralph-monitor`：`src/` 新增 ingest/routes 模块；`migrations/0003_events.sql`。
- `frontend/ralph-web/src/features/monitor/RunDetailPage.tsx`（Plan1-U5 最小详情 → 本计划 tabs 版）。

**计划新增**

- `presets/monitor/{ce-executor-pipeline,ce-executor-pipeline-loop,parallel-forge,autoresearch}.yml`（builtin sidecar v1 覆盖 4 个代表 preset；其余 builtin → fallback）。
- `crates/ralph-cli/src/commands/inspect/descriptor.rs`（候选位置；resolver：builtin→`presets/monitor/`（安装数据目录）→custom→null）。
- `backend/ralph-monitor/src/{events/ingest.ts,events/progress.ts,events/graph.ts,artifacts/index.ts,artifacts/serve.ts,routes/events.ts,routes/artifacts.ts}`。
- `frontend/ralph-web/src/features/monitor/{RunTimeline,RunGraph,LogViewer(monitor 版或复用),ArtifactViewer,StageStrip}.tsx` 与 `e2e/monitor-details.spec.ts`。

**明确不修改**：`presets/en/*.yml`、`presets/index.json`、`presets/manifest.yml`、`crates/ralph-cli/src/presets.rs`、event topology、`EventRecord` 形状、`ralph emit`、`backend/ralph-web-server`、`.ralph/dag.db` schema（只读探测，不写）。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选 | 最终选择 | 支持证据 | 排除原因 | 置信度 |
| ---- | ---- | ---- | ---- | ---- | ---- | ---- |
| D1 | descriptor 存在形态 | A. 写进 preset yml 顶层 `monitor:` 节；B. `presets/monitor/` sidecar；C. monitor 服务内置 TS 常量 | B：独立 sidecar 目录 `schema_version: monitor-descriptor/v1` | E7（目录不存在=加法）、E16-Plan1（改 builtin preset 需 4 处同步+lint 风险）、原 KTD1 | A 动 event topology 邻近字段引入 preset_lint 面；C 无法版本化/不可由 runtime 解析 | 0.9 |
| D2 | descriptor 如何到达 monitor | A. monitor 直读 `presets/monitor/*.yml` 文件系统；B. `inspect run` 内嵌解析后 JSON；C. 独立 `ralph inspect descriptor` 命令 | B：snapshot `preset_descriptor` 可选字段（含 `schema_version`+`resolved_from:builtin\|custom\|none`） | E3（inspect JSON=SSOT）、E11（三源语义在 runtime 侧已统一） | A 把 runtime 安装目录布局耦合给 TS 且跨项目路径不同；C 多一次 spawn，unnecessary | 0.88 |
| D3 | 事件 ingest 机制 | A. 每轮 `ralph events --format json` 全量重读；B. monitor 直接 byte-cursor 读 `.ralph/events-*.jsonl`；C. 每轮 `ralph events --last N` | B：monitor 对 events 文件做 (file,offset) 持久游标增量读；坏行跳过计数 | E1（EventHistory 无增量接口且整文件读 O(n)/轮）、E14（文件布局实况）、E2（`--file` 契约=稳定 JSONL 格式） | A/C 每轮全量 spawn 成本随历史线性增长且 `--last` 无法表达游标 | 0.86 |
| D4 | source sequence 排序键 | A. ts；B. (file_index, byte_offset) 单调序；C. hat-channel 优先 | B：文件按文件名时间戳序→字节偏移；`seq` 为 monitor 分配的连续整数 | 原 KTD3（"按 source sequence，timestamp 仅展示"）、E1（同 ts 可并列） | A 同刻乱序不稳；C hat-channel 是副本非权威（E2） | 0.9 |
| D5 | 进度分母策略 | A. 强制百分比；B. `units{done,total?,source}`+`stage`+`iteration` 三选一证据展示 | B：descriptor 声明 `progress.denominator: stages|units|none`；DAG=units（forge.unit.*）、线性=stages、无 descriptor=none | E6（线性有确定 stage 总数）、E4/E5（DAG units 从事件/scheduler 得） | A 无分母时伪造=违反 R1 | 0.9 |
| D6 | DAG 图数据来源 | A. 只读 `dag.db`；B. `forge.unit.*` 事件重放 +（活动 run）scheduler 块；C. preset hats 拓扑静态图 | B：事件为权威（历史 run 也可构图）；活动 run 叠加 scheduler 计数校验 | E5（scheduler 块仅活动可得）、E4（forge.unit.* payload 带 unit 身份） | A sqlite schema 私有、历史 run 可用但耦合内部；C 静态拓扑无执行态 | 0.85 |
| D7 | artifact 通道 | A. monitor 任意读项目文件；B. allowlist 根集合+realpath+类型/尺寸限制 | B：`.ralph/`、`repo_root/docs/`、`manifest.plan_path` 目录 | R4 安全要求、E14（真实布局） | A 等于把任意文件读能力暴露给浏览器 | 0.92 |
| D8 | Markdown 安全 | A. react-markdown 默认；B. +rehype-raw；C. 自研渲染 | A + 显式 `urlTransform` 白名单（http/https/mailto/相对路径），禁用 `javascript:`/`data:` | E9（v10 默认不渲染 raw HTML） | B 引入 XSS 面；C 重复造轮子 | 0.9 |
| D9 | 日志视图数据源 | A. 独立 log 文件摄取（diagnostics）；B. 事件 payload 派生日志行 | B：log view = 事件流的人读视图（hat/topic/payload 渲染+搜索），`pause-follow` 前端态 | E1（事件 ledger 是 always-on 证据；diagnostics 需 RALPH_DIAGNOSTICS 才写） | A 默认关闭→无数据可显示 | 0.87 |
| D10 | 时间线↔产物关联 | A. 按事件 payload 内路径引用解析；B. 仅 manifest/descriptor 声明的 artifact 槽位 | B：artifact 索引 = 声明槽位+目录枚举探测；事件不解析路径 | payload 是自由文本/截断（E1） | A 脆弱且把不可信文本当路径用（安全） | 0.88 |

无低于 0.85 决策。

实施协议（沿用 Plan1 §实施协议）：验收契约/停止契约/nextest/env scrub/fake-ralph/CLI drift 检查同 Plan 1，不再重复列出。

## 4. BDD 行为规格

```gherkin
Feature: 运行详情与流程可视化

  Scenario: S1 线性 preset 阶段化进度
    Given 一次 ce-executor-pipeline run 的事件 ledger 已有 plan.ready 与 work.done
    When 请求 /runs/:id/progress
    Then 返回 stage="work"、current_hat=最后事件的 hat、iteration=最大值
    And units.source="stages" 且 done/total 按 descriptor stage 表计算
    And progress 不含伪造百分比

  Scenario: S2 无描述符 preset 的通用视图
    Given 一次 run 的 preset 无匹配 descriptor
    When 打开详情页
    Then 顶部显示通用进度（状态/轮次/最后事件时间）
    And events 与 logs 标签页可用
    And 页面展示"无 preset 描述符，显示通用视图"提示

  Scenario: S3 事件时间线按 source sequence 分页
    Given 该 run 的 ledger 有 500 条事件
    When 请求 /events?limit=200
    Then 返回前 200 条按 (file,offset) 排序且带单调 seq
    When 携带 cursor=seq200 再请求
    Then 返回后续且不重复不遗漏

  Scenario: S4 日志搜索与暂停跟随
    Given 详情页 logs 标签已打开且跟随中
    When 新事件到达且用户点击暂停
    Then 视图冻结在当前窗口
    And 输入关键词过滤后仅匹配行可见
    And 点击"加载更早"拉取上一页历史

  Scenario: S5 DAG preset 单元进度与图
    Given parallel-forge run 存在 forge.unit.ready/executed/failed 事件
    When 请求 /progress 与 /graph
    Then progress.units.source="forge-units" 且 done/total 与事件一致
    And /graph 返回 unit 节点与状态，前端图懒加载渲染且列表视图同数据可用

  Scenario: S6 artifact 安全预览
    Given run 的 .ralph/agent/summary.md 存在
    When 请求 /artifacts 索引
    Then 列表含 summary 条目（kind/summary/size/mtime）
    When 请求其内容
    Then 返回文本且前端渲染 sanitized Markdown
    When 请求越界路径 ../../etc/passwd
    Then 返回 403 path_outside_allowed_roots

  Scenario: S7 artifact 缺失的明确提示
    Given run 无 plan 文件与 summary
    When 打开 artifacts 标签
    Then 每个缺失槽位显示"尚未生成"而非报错或空区域

  Scenario: S8 事件行损坏的容错
    Given ledger 中间存在一行坏 JSON 与一行 payload 截断事件
    When ingest 运行
    Then 坏行计入 evidence.parse_errors 且不中断
    And 截断事件正常入 timeline 且 UI 显示截断标记

  Scenario: S9 时间线条目跳转产物
    Given descriptor 声明 report.done → summary 产物槽位
    When 用户在时间线点击 report.done 事件的关联入口
    Then 打开对应 artifact 预览并高亮定位

  Scenario: S10 循环 preset 的阶段回退展示
    Given ce-executor-pipeline-loop run 的 fix.done 后出现新一轮 review.* 事件
    When 查看 progress 与 stage strip
    Then 当前阶段指向最新阶段而非"已完成即停留"
    And 阶段历史显示多轮循环（round 计数）
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
| ---- | ---- | ---- | ---- | ---- | ---- |
| S1 | stage/units/迭代断言全中 | `crates/ralph-cli` descriptor resolver 单测 + monitor `progress.test.ts` | 单元+集成 | Property（全部 stage topic→stage 映射无遗漏，取自 E6 词表） | 否 |
| S2 | descriptor:null+通用视图提示 | `RunDetailPage.test.tsx` | 组件 | — | 否 |
| S3 | seq 单调、无重复遗漏、cursor 正确 | `test/integration/events_ingest.test.ts` | 集成 | Idempotency（重 ingest 同文件不产生重复 seq） | 否 |
| S4 | 暂停冻结/过滤/历史分页 | `LogViewer.test.tsx` | 组件 | — | 否 |
| S5 | units 计数与事件一致；graph 节点/边正确；列表降级 | `graph.test.ts`+`RunGraph.test.tsx` | 单元+组件 | — | monitor-details spec |
| S6 | allowlist 放行/越界 403/symlink 逃逸拒/超尺寸拒 | `artifacts.test.ts` | 单元+集成 | Security（`..`/编码/symlink/硬链接/非 text/512KiB 上限） | 否 |
| S7 | 缺失→typed not_found 且 UI"尚未生成" | `artifacts.test.ts`+`ArtifactViewer.test.tsx` | 集成+组件 | — | 否 |
| S8 | parse_errors 计数、不中断、截断标记 | `events_ingest.test.ts`+`RunTimeline.test.tsx` | 集成+组件 | Fault injection（坏行/截断/空文件/文件切换 current-events） | 否 |
| S9 | descriptor 槽位→artifact 定位跳转 | `RunDetailPage.test.tsx` | 组件 | — | monitor-details spec |
| S10 | stage 跟随最新+round 计数 | `progress.test.ts`（loop descriptor） | 单元 | — | 否 |

## 6. 需求—测试追踪矩阵

| Requirement | Scenario | 验收测试 | 单元测试 | 集成/契约 | E2E | Evidence |
| ---- | ---- | ---- | ---- | ---- | ---- | ---- |
| R1 | S1/S10 | progress 测试+resolver 单测 | stage 映射/迭代/耗时 | events_ingest | — | E4/E5/E6 |
| R2 | S2/S5 | detail 组件+graph 测试 | fallback/graph 构建 | — | monitor-details | E5/E7/E11 |
| R3 | S3/S4/S9 | ingest 集成+log/时间线组件 | seq 排序/过滤 | events API | monitor-details | E1/E2/E4 |
| R4 | S6/S7/S8 | artifacts 集成+viewer 组件 | allowlist/realpath/sanitize | — | monitor-details | E8/E9/E14 |

Unit 映射：S1/S10→U1+U2；S3/S8→U2；S2→U4；S4/S9→U4；S5→U2+U5；S6/S7→U3+U5。

## 7. 严格串行开发单元

```text
Unit 1（descriptor sidecar schema + builtin 4 份 + inspect run 字段）
  ↓
Unit 2（事件 ingest + source sequence + events/progress/graph API）
  ↓
Unit 3（artifacts 索引与内容安全通道）
  ↓
Unit 4（RunDetailPage：header 进度 + stage strip + timeline + logs）
  ↓
Unit 5（DAG 图懒加载 + artifacts 预览 + e2e monitor-details）
```

### Unit 1：monitor descriptor sidecar — schema、builtin 4 份、`inspect run.preset_descriptor`

#### 1. Unit 目标
`ralph inspect run --format json` 在 run 有 preset 时输出 `preset_descriptor`（`monitor-descriptor/v1` 解析结果）；仓库新增 `presets/monitor/{ce-executor-pipeline,ce-executor-pipeline-loop,parallel-forge,autoresearch}.yml` 四份 sidecar；无匹配→`preset_descriptor:null` 且 `resolved_from:"none"`。

#### 2. 对应需求与 Scenario
R2 数据面；S1/S2/S10 的映射来源；D1/D2/D5；E5/E6/E7/E11。

#### 3. 外部可观察结果
snapshot run 项新增：

```json
"preset_descriptor": {
  "schema_version": "monitor-descriptor/v1",
  "resolved_from": "builtin",
  "preset": "ce-executor-pipeline",
  "stages": [{"id":"plan","label":"Plan","topics":["work.start","plan.ready"]}],
  "progress": {"denominator": "stages"},
  "artifacts": [{"id":"summary","path_glob":".ralph/agent/summary.md","kind":"markdown"}],
  "dag": {"enabled": false}
} | null
```

#### 4. 当前行为基线
`presets/monitor/` 不存在（E7）；`inspect run` 无该字段（Plan1-U1 输出契约）。Red=字段缺失。

#### 5. 输入与输出
输入：run 的 `preset` 字段（builtin name / `builtin:<name>` / `directory:<stem>` / collection id）。输出：可选 descriptor JSON；schema_version 不符→`resolved_from:"unsupported_version"` 诊断而非 panic。副作用：无（只读）。不变量：不改 preset yml；descriptor 不参与 run 执行。

#### 6. 修改位置
- `presets/monitor/*.yml`（新增 4 份；字段：schema_version/preset/stages[{id,label,topics[]}]/progress{denominator}/artifacts[{id,path_glob,kind}]/dag{enabled,unit_topic_prefix}）。ce-executor-pipeline 的 stages 严格按 E6 序列；loop 版按 `presets/en/ce-executor-pipeline-loop.yml` 实际 topic（进入 U1 时读取该文件核对）；parallel-forge `dag.enabled=true, unit_topic_prefix="forge.unit."`（E4）；autoresearch 按其文件。
- `crates/ralph-cli/src/commands/inspect/descriptor.rs`（新增，候选）：resolver `resolve(preset_ref, workspace) -> Option<Descriptor>`；解析顺序 builtin sidecar（运行时定位 `presets/monitor/`——与 `presets/index.json` 同根，**进入 U1 时确认 builtin preset 数据目录解析函数** `presets.rs::load_builtin` 或等价并复用）→ workspace `.ralph/monitor/<name>.yml` → none。
- `crates/ralph-cli/src/commands/inspect/run.rs`（Plan1 新增处）：snapshot 组装调用 resolver。
- 单测与集成断言（integration_inspect_run.rs 增补）。

#### 7. 可依赖能力
serde_yaml 已在 workspace（检查：ralph-core/adapters 已用 yaml 解析 preset——E11 preset_domain 读 yml 先例存在，依赖可用）；Plan1-U1 snapshot 组装点。

#### 8. 禁止依赖的未来能力
monitor 端 API；不读 events；不在 descriptor 声明 actions/notifications。

#### 9. 验收测试
- `descriptor` 单测：builtin hit/miss、custom `.ralph/monitor/x.yml` hit、版本不符→unsupported、坏 YAML→`resolved_from:"invalid"`+diagnostic。
- `integration_inspect_run.rs`：fake manifest `preset:"ce-executor-pipeline"`→JSON 含完整 descriptor；`preset:"unknown-x"`→null。
- 命令：`cargo nextest run -p ralph-cli --test integration_inspect_run -- descriptor`；`cargo nextest run -p ralph-cli -- inspect::descriptor`。

#### 10. Acceptance Red
断言 `preset_descriptor` 字段 → JSON 无此键（有效 Red）。

#### 11. 单元测试拆分
resolver 顺序（builtin>custom>none）；schema_version 校验；stages topics 去重告警→diagnostic；denominator 枚举校验；dag.unit_topic_prefix 默认 `forge.unit.`；path_glob 拒绝绝对路径/`..`（descriptor 自身即安全边界——防恶意 sidecar）。

#### 12. Red → Green → Refactor
```text
schema/校验单测 Red → schema+parser → Green
→ resolver 单测 Red → resolver → Green
→ integration 字段 Red → snapshot 接线 → Green
→ 4 份 sidecar 落盘 + 各自"topics 覆盖 preset publishes"自检单测（descriptor 校验器读 en preset 的 publishes 对比 topics⊆声明——轻量一致性）
→ Refactor
```

#### 13. 最小实现范围
schema/parse/resolve/输出字段/4 份文件/校验自检。不实现：custom descriptor 的 monitor 端注册 API（v1 仅项目 `.ralph/monitor/` 目录约定）；不渲染。

#### 14. 集成验证
nextest 上列；`ralph inspect run --format json` 对真实 `.ralph/` 手工跑（本仓库有 real run 痕迹）。

#### 15. 风险驱动测试
Property：descriptor.stages[].topics ⊆ preset publishes+LOOP_COMPLETE（校验器）；Differential：4 份 sidecar 与对应 en preset 的 hat publishes 对照表人工核对记录。

#### 16. 回归范围
`integration_inspect_run` 全量；`integration_preset_builtin`（builtin 枚举未变）；`check-cli-doc-drift.sh`（inspect 输出文档）；`validate-builtin-presets.sh`（presets/ 目录新增子目录须确认脚本不误伤——进入 U1 时试跑，若脚本硬编码顶层文件集合则需为其加白名单，记录为计划内改动）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `presets/monitor/*.yml` ×4 | 新增 | sidecar | D1/E6/E7 |
| `crates/ralph-cli/src/commands/inspect/descriptor.rs`（候选） | 新增 | resolver | D2 |
| `crates/ralph-cli/src/commands/inspect/run.rs` | 修改 | 输出字段 | E3 |
| `tests/integration_inspect_run.rs` | 修改 | 断言 | — |
| `scripts/validate-builtin-presets.sh`（若误伤） | 修改 | 允许 monitor/ 子目录 | 进入 Unit 时确认 |

#### 18. 完成标准
S1 数据面绿（snapshot 有正确 descriptor）；4 份 sidecar 与其 preset publishes 一致性自检绿；Rust 门禁全绿。

#### 19. 停止条件
builtin preset 数据目录在发行形态（cargo install）下不可寻址 → stop：改将 sidecar `include_str!` 内嵌编译（参照 presets.rs 内嵌 manifest.yml 先例 E16）并重算。

#### 20. 风险与注意事项
- sidecar 与 preset topic 漂移：自检单测+CI 脚本（`check-aaf-fixtures.sh` 模式可仿）；残留=人工同步清单进 `docs/guide/monitor` 章节（Plan5 文档）。
- descriptor 恶意 path_glob：校验器拒绝绝对路径与 `..`。

---

### Unit 2：事件 ingest + source sequence + `/events` `/progress` `/graph` API

#### 1. Unit 目标
collector 在同一 ingest 事务中按持久 byte-cursor 增量读 run 关联 `.ralph/events-*.jsonl`（main ledger 与 hat-channel 仅取 main），为每条事件分配 `seq`；`/runs/:id/events`、`/runs/:id/progress`、`/runs/:id/graph` 返回投影；坏行容错；重复 ingest 幂等。

#### 2. 对应需求与 Scenario
R1/R3 数据面 + R2 的 DAG 面；S1/S3/S5/S8/S10；D3/D4/D5/D6/D9；E1/E2/E4/E5/E12。

#### 3. 外部可观察结果
`GET /api/monitor/v1/runs/:gid/events?cursor&limit&hat&topic&q`→`{events:[{seq,ts,iteration,hat,topic,payload,truncated}],next_cursor,watermark}`；`GET /progress`→`{stage,stage_history[{stage,at_seq}],current_hat,iteration,elapsed_ms,units:{done,total?,source},denominator}`；`GET /graph`→`{nodes:[{unit,status,last_seq}],edges:[…],source:"forge-events"|"scheduler"|"none"}`。

#### 4. 当前行为基线
monitor DB 无 events 表；无上述路由（Red）；snapshot 已含 `events_file`+`preset_descriptor`（U1）。

#### 5. 输入与输出
输入：events 文件集（`events_file`+`.ralph/events-*.jsonl` 枚举按文件名时间排序，窗口=[started_at, terminal.at|now]）。输出：事件投影+派生 progress/graph。状态变化：表 `run_events(project_id,run_id,seq pk,file,byte_offset,ts,iteration,hat,topic,payload,truncated)`、`event_cursors(project_id,run_id,file,byte_offset)`、`run_evidence`（parse_errors 计数）。副作用：事务写。不变量：seq 严格单调不重用；同 (file,offset) 重放不重复行；current-events 切换不丢尾部。

#### 6. 修改位置
- `migrations/0003_events.sql`：上述表 + index `(run,seq)`、`(run,topic)`、`(run,iteration)`。
- `src/events/ingest.ts`：文件枚举→cursor 读→行解析（zod-loose：必填 ts/iteration?/hat?/topic——宽松字段，缺 hat→`hat:null`）→seq 分配→事务写。
- `src/events/progress.ts`：纯函数 derive(rows, descriptor, snapshot)→progress；loop descriptor 的 stage_history 按 topic 出现序列计 round。
- `src/events/graph.ts`：forge.unit.* payload 取 unit 身份+状态机（ready→executed|failed→correction）；活动 run 叠加 `inspect run` scheduler 块（若 snapshot 含——U1 未加 scheduler 块；**进入 U2 时：如需要则在 inspect run 增补 `scheduler` 透传（`inspect loop` 已提供实现可复用，E5），作为本 Unit 的并行小变更；否则仅事件重放**——决策：U2 内增补 `inspect run` 输出 `scheduler` 透传字段（当 mode≠wave），小改动有独立测试）。
- `src/routes/events.ts` + `runs.ts` 扩展；collector ingest 同事务接入（E12）。
- 测试：`test/unit/{progress,graph,seq}.test.ts`、`test/integration/{events_ingest,events_api}.test.ts`、fixture `.ralph/events-*.jsonl` 手写集。

#### 7. 可依赖能力
Plan1-U3 事务/游标框架；U1 descriptor；fake-ralph fixture；zod。

#### 8. 禁止依赖的未来能力
SSE 推流（Plan3）；通知映射（Plan4）；artifact 内容（U3 平行）。

#### 9. 验收测试
如上 S1/S3/S5/S8/S10 各断言；`events_api`：cursor/limit/filter/q；`events_ingest`：增量、幂等、坏行、文件轮转（改名新增文件后尾部续读）、payload 截断标记（>50000 字符 payload fixture→truncated=true）。

#### 10. Acceptance Red
`GET /events`→404；ingest 相关测试模块不存在。

#### 11. 单元测试拆分
seq 分配（跨文件连续）；排序键（file_index,offset）；truncated 判定；progress derive：线性 stage 推进、loop round 计数、units from forge topics、无 descriptor→denominator:none；graph：节点状态机全迁移（ready/executed/failed/correction/超时残留→stale？——决策：图中无终态事件且 run 仍 running→`running`，run 终态后未 executed→`abandoned`）；坏行计数。
不允许 mock：sqlite 真库；fixture 真文件。

#### 12. Red → Green → Refactor
```text
seq/cursor 单测 Red → ingest → Green
→ events_api Red → routes → Green
→ progress 单测 Red → derive → Green
→ graph Red → Green
→ 集成（collector 接入）→ Green → Refactor
```

#### 13. 最小实现范围
增量 ingest+三端点+容错+幂等+scheduler 透传小改。不实现：payload 内路径解析、全文检索索引（q=SQL LIKE 即可，记录规模假设）、hat-channel 摄取（main ledger 覆盖全部 hat 字段——E1）。

#### 14. 集成验证
fake ralph fixture（两段 events 文件+一轮 current-events 切换）；与 U1 真 `ralph emit` 造文件对照（E13：可用真 ralph emit 生成 fixture 再固化）。

#### 15. 风险驱动测试
- Idempotency：重放同字节区间→无重复 seq。
- Fault injection：写一半的行（无换行尾）→按未完成行处理（cursor 停在行首）；文件被截断重写→检测 offset 倒退→重置该文件 cursor 并记 `source_reset`。
- Property：随机乱序事件流→seq 严格单调。
- State-machine：graph 非法迁移（executed→ready）→保留首态+diagnostic。

#### 16. 回归范围
Plan1-U3 全部；`integration_inspect_run`（scheduler 透传改动面）；`inspect loop` 相关 `integration_wave_inspect`/`integration_dag_scheduler`（透传不改原命令）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `migrations/0003_events.sql` | 新增 | 事件投影 | D3/D4 |
| `src/events/{ingest,progress,graph}.ts`、`src/routes/events.ts` | 新增 | API | — |
| `src/collector/collector.ts` | 修改 | ingest 接入 | E12 |
| `crates/ralph-cli/src/commands/inspect/run.rs` | 修改 | scheduler 透传 | E5 |
| `tests/integration_inspect_run.rs` | 修改 | 透传断言 | — |
| `test/**` fixtures | 新增 | — | — |

#### 18. 完成标准
S1/S3/S5/S8/S10 数据面绿；幂等/fault/property 测试绿；`npm test -w @ralph-monitor/server`+nextest 绿。

#### 19. 停止条件
EventRecord 行结构出现 schema 外字段被 zod-strict 拒→换 loose schema（决策已选 loose）；forge.unit.* payload 无 unit 身份字段（与 E4 假设冲突）→stop：检查真实 payload→若只有 topic 无 unit key，改用 topic 内嵌段解析或 dag.db 只读探测，重算置信度。

#### 20. 风险与注意事项
- 事件文件被 `ralph events --clear` 清理→`source_reset` 处理+run_evidence 记录，UI 显示"历史已被清理"。
- 巨型 ledger：byte-cursor 保证增量成本恒定；LIKE 搜索在大数据量退化→记录已知限制（>1e5 事件未测）。
- payload 嵌套 JSON 字符串二次转义→显示层原样展示不强行格式化。

---

### Unit 3：artifacts 索引与内容安全通道

#### 1. Unit 目标
`GET /runs/:id/artifacts` 返回 run 关联产物索引（声明槽位+探测枚举）；`GET /runs/:id/artifacts/:artifactId/content` 返回文本内容；越界/symlink 逃逸/超尺寸/不存在→typed 错误；缺失槽位→`status:"not_generated"`。

#### 2. 对应需求与 Scenario
R4 数据面；S6/S7；D7/D10；E8/E14。

#### 3. 外部可观察结果
索引：`{artifacts:[{id,kind,path?,status:present|not_generated|outside_allowed,size?,mtime?}]}`；内容：`{id,content,truncated}`（>512KiB→`content_too_large` 413）。

#### 4. 当前行为基线
无 artifacts 端点（Red）；allowlist 根 = `{workspace/.ralph, repo_root/docs, plan_path dir}`（D7）。

#### 5. 输入与输出
输入：run 行（workspace/repo_root/manifest.plan_path/descriptor.artifacts）。输出：索引/内容 JSON；错误 typed。副作用：无写。不变量：仅 GET；realpath 必须在 allowlist 根内；拒绝非 regular file。

#### 6. 修改位置
- `src/artifacts/{index,serve}.ts`：`knownSlots`（descriptor.artifacts ∪ 内置槽位 `{summary:.ralph/agent/summary.md, handoff:.ralph/agent/handoff.md, diagnosis:.ralph/agent/diagnosis-summary.json}` ∪ `.ralph/review/**` 浅枚举一级 ∪ `.ralph/post-merge/**` 浅枚举一级）→realpath 校验→stat。
- `src/routes/artifacts.ts`；`src/lib/allowlist.ts`（realpath+根前缀判定+拒绝 symlink 出界——realpath 后的路径前缀比较）。
- 测试：`test/unit/allowlist.test.ts`、`test/integration/artifacts.test.ts` + fixture 树（含 symlink 逃逸案例）。

#### 7. 可依赖能力
Plan1 workspace/repo_root 字段；U1 descriptor.artifacts。

#### 8. 禁止依赖的未来能力
前端渲染（U5）；二进制内容（v1 仅 text/*）。

#### 9. 验收测试
S6/S7 全断言；allowlist 矩阵（`..`/`%2e`/symlink 内→外/外→内/硬链接/目录/设备文件）；size cap；kind 推断（.md→markdown,.json→json,.yml→yaml,else→text）。

#### 10. Acceptance Red
404 route。

#### 11. 单元测试拆分
realpath 前缀判定（大小写/尾斜杠/相对化）；symlink 逃逸矩阵；槽位合并去重（descriptor 与内置同 id→descriptor 优先？——决策：descriptor 覆盖同 id 内置）；stat 失败→not_generated；内容编码 UTF-8，非法→`binary_or_invalid_encoding` 415。

#### 12. Red → Green → Refactor
```text
allowlist 单测 Red → 实现 → Green
→ index Red → 槽位+探测 → Green
→ serve Red → 内容通道 → Green
```

#### 13. 最小实现范围
索引+内容+校验+typed 错误。不实现：目录浏览 UI 数据（只一级枚举）、diff、编辑。

#### 14. 集成验证
fixture 树走真 fs；inject 全矩阵。

#### 15. 风险驱动测试
Security 全矩阵（§5）；Fault：读取期间文件消失→404 优雅；大文件 stat 后增长竞态→读时仍强 cap（read with limit）。

#### 16. 回归范围
monitor 全部测试。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `src/artifacts/{index,serve}.ts`、`src/lib/allowlist.ts`、`src/routes/artifacts.ts` | 新增 | R4 通道 | D7 |
| `test/**` | 新增 | ATDD | — |

#### 18. 完成标准
S6/S7 绿；安全矩阵全绿；typecheck/build/test。

#### 19. 停止条件
realpath 在某文件系统不解析 symlink（如 Windows junction）→记录并按 OS gate；其余按停止契约。

#### 20. 风险与注意事项
- allowlist 根本身被 symlink（repo_root 是 link）→规范化顺序：先 realpath 根再 realpath 目标，二者比较。
- `.ralph/review/**` 枚举深度 1 防爆炸。

---

### Unit 4：RunDetailPage — header 进度、stage strip、timeline、logs

#### 1. Unit 目标
详情页升级为 tabs 版：header（run_id/状态徽章/项目/耗时/current_hat）、stage strip（descriptor 驱动，当前高亮+round）、tabs：`概览/事件时间线/日志`（本 Unit 三个）；时间线条目可关联 artifact 槽位（点击→artifacts 标签锚点）；日志支持暂停跟随/搜索/加载更早；URL 承载 tab+选中事件 seq。

#### 2. 对应需求与 Scenario
R1/R2/R3 用户面；S1/S2/S4/S9/S10；D5/D9；E9。

#### 3. 外部可观察结果
见 §3 Unit 目标；无 descriptor→通用视图横幅（S2）；键盘可在 tab 间切换、时间线行 Enter 选中、日志 `ctrl+f` 页内搜索框。

#### 4. 当前行为基线
Plan1-U5 最小详情页（字段表）；无 tabs/时间线/日志（Red）。

#### 5. 输入与输出
输入：`/progress` `/events`（轮询 3s 复用 react-query refetchInterval；游标增量追加缓存）。输出：渲染+URL。副作用：无。不变量：轮询停于 run 终态+idle 5min（减耗）；切页保留 tab。

#### 6. 修改位置
- `features/monitor/RunDetailPage.tsx`：tabs 重构。
- `features/monitor/detail/{DetailHeader,StageStrip,OverviewTab,TimelineTab,LogsTab}.tsx`（新增）。
- `features/monitor/components/{EventRow,LogList,StageNode}.tsx`（新增）；日志虚拟化：`@tanstack/react-virtual` 新依赖？——检查现有：无该依赖；行高固定→自实现窗口化 ~80 行即可，决策=自实现（避免新依赖）。
- `hooks.ts`：useProgress/useEvents(cursor 追加)/useRunGraph(占位 U5 用)。
- 测试：上述组件 `.test.tsx`。

#### 7. 可依赖能力
U2 API；Plan1-U4 ui/主题/hooks。

#### 8. 禁止依赖的未来能力
graph/artifacts tabs 内容（U5）；SSE（Plan3 换轮询为流式，接口已按 cursor 设计兼容）。

#### 9. 验收测试
组件断言：S1 header/stage、S2 fallback 横幅、S4 暂停/搜索/历史、S9 条目→artifact 槽位跳转（mock descriptor 槽位）、S10 round 计数显示。

#### 10. Acceptance Red
页面无 tabs/stage strip（元素不存在）。

#### 11. 单元测试拆分
StageStrip：stage 高亮=progress.stage；round=N 显示；无 descriptor→隐藏。TimelineTab：seq 渲染、选中态、键盘导航、truncated 标记。LogsTab：pause 冻结/恢复补数据、filter、加载更早 cursor。虚拟窗口：大列表只渲染视窗±overscan。

#### 12. Red → Green → Refactor
```text
header/stage Red→Green → timeline Red→Green → logs Red→Green → tabs/URL → Refactor
```

#### 13. 最小实现范围
上述三 tabs+URL+轮询。不实现：graph/artifacts（占位 tab disabled 显示"计划中"或直接隐藏——决策：隐藏，待 U5 出现）、导出。

#### 14. 集成验证
vitest 全量；route-intercept e2e 中详情页主路径（并入 U5 spec 或本 Unit 内小 spec——决策：并入 U5 的 monitor-details.spec.ts）。

#### 15. 风险驱动测试
a11y（tabs roving tabindex、行激活）；性能冒烟（5k 事件渲染 <100ms 帧断言宽松——jsdom 不量帧，改断言渲染节点数上限）。

#### 16. 回归范围
Plan1-U5 详情测试更新；`App.test.tsx`；vitest 全量。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `features/monitor/detail/*`、`components/{EventRow,LogList,StageNode}.tsx` | 新增 | UI | E9 |
| `RunDetailPage.tsx`、`hooks.ts` | 修改 | tabs/数据 | — |

#### 18. 完成标准
S1/S2/S4/S9/S10 用户面绿；typecheck/build/vitest 绿；URL 往返；无新依赖（虚拟化自实现确认）。

#### 19. 停止条件
LogViewer.tsx 既有组件可复用度评估失败（接口耦合 task WS）→按已选自实现继续不算停止；轮询数据竞态（cursor 跳跃）→以 seq 去重合并。

#### 20. 风险与注意事项
- 轮询+窗口渲染的内存：缓存 cap 10k 事件（超出丢弃最旧窗口——记录决策）。
- timestamp 显示本地时区+原始 iso tooltip。

---

### Unit 5：DAG 图（React Flow 懒加载）+ artifacts 预览 + e2e monitor-details

#### 1. Unit 目标
dag run 详情出现 `图` 标签：React Flow `React.lazy` 加载渲染 `/graph` 数据（节点状态着色、边、点击节点筛选时间线到该 unit 事件），同时提供同数据可访问列表视图；`产物` 标签：索引列表+Markdown 预览（sanitized）+文件定位（路径+行锚）+缺失槽位"尚未生成"；e2e `monitor-details.spec.ts` 覆盖详情主旅程。

#### 2. 对应需求与 Scenario
R2（DAG 统一框架）+R4 用户面；S5/S6/S7/S9；D6/D7/D8；E4/E9。

#### 3. 外部可观察结果
如图+artifacts 两 tab 完整可用；图不可用（无 dag 数据）→tab 隐藏；列表视图与图数据一致；e2e 通过。

#### 4. 当前行为基线
U4 tabs 缺 graph/artifacts（Red）；`@xyflow/react` 已在依赖（E9）。

#### 5. 输入与输出
输入：`/graph` `/artifacts` `/artifacts/:id/content`。输出：图渲染+预览。不变量：图仅 dag 显示；Markdown 无 raw HTML/危险 URL。

#### 6. 修改位置
- `features/monitor/detail/{GraphTab,ArtifactsTab}.tsx`（新增）；`features/monitor/components/{RunGraph,GraphListView,ArtifactList,MarkdownViewer}.tsx`（新增）。
- `hooks.ts`：useGraph/useArtifacts/useArtifactContent。
- `e2e/monitor-details.spec.ts`（新增，route-intercept 模式 E9）。
- 测试组件 `.test.tsx`。

#### 7. 可依赖能力
U2 graph API、U3 artifacts API、U4 tabs 框架。

#### 8. 禁止依赖的未来能力
SSE（轮询即可）；artifact 编辑。

#### 9. 验收测试
S5 图数据渲染+列表等价+节点筛选；S6 预览渲染与 sanitize（`<script>`/javascript: 链接不生效）；S7 缺失提示；S9 全链跳转；e2e 详情旅程（进入→tab 切换→日志搜索→图节点→artifact 打开）。

#### 10. Acceptance Red
tabs/组件不存在；e2e 元素缺失。

#### 11. 单元测试拆分
RunGraph：nodes/edges 映射、状态色、空数据；GraphListView 同数据；MarkdownViewer：XSS 向量（`javascript:`、`data:text/html`、`<img onerror>`——react-markdown 默认已挡 html，断言 url 过滤）;ArtifactList 槽位状态展示。

#### 12. Red → Green → Refactor
```text
ArtifactsTab Red→Green → MarkdownViewer Red→Green → GraphTab(懒加载+Suspense fallback) Red→Green → e2e Red→Green
```

#### 13. 最小实现范围
两 tab+e2e。不实现：图布局算法定制（用 dagre？——决策：@xyflow 自带简单布局不够；引入 `dagre`+`@dagrejs/dagre`？检查依赖无；自实现层次布局对 forge DAG 足够（按 stage/attempt 分层），决策=简单分层布局自实现，不引 dagre）、minimap。

#### 14. 集成验证
vitest；playwright spec；build。

#### 15. 风险驱动测试
a11y：图 aria-hidden+列表等价（决策：图 role=img+隐藏真交互，列表为交互权威）；XSS 矩阵；Suspense fallback 渲染。

#### 16. 回归范围
前端全量 vitest+playwright；`App.test`。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `detail/{GraphTab,ArtifactsTab}.tsx`、`components/{RunGraph,GraphListView,ArtifactList,MarkdownViewer}.tsx` | 新增 | UI | E9 |
| `e2e/monitor-details.spec.ts` | 新增 | ATDD | E9 |
| `hooks.ts`、`RunDetailPage.tsx` | 修改 | 数据/挂载 | — |

#### 18. 完成标准
S5/S6/S7/S9 绿；e2e 绿；构建无新依赖（布局自实现）；全门禁。

#### 19. 停止条件
React Flow 懒加载 chunk 拆分失败（vite 配置冲突）→改非懒加载但路由级 code-split 已天然存在；若图数据为空仍渲染空画布→隐藏 tab 已定。

#### 20. 风险与注意事项
- React Flow 在 jsdom 不可测→组件测试测映射函数+列表视图，图本体仅 e2e。
- 大 DAG（>200 节点）：列表视图限 500 行截断+提示；图仅渲前 200 节点+提示（记录）。

## 8. Unit 串行依赖图

```text
Unit 1 → Unit 2 → Unit 3 → Unit 4 → Unit 5
```

- U2 用 U1 的 `preset_descriptor`（progress/graph derive）与 events_file；U3 用 U1 的 `artifacts` 槽位声明；U4 用 U2 API；U5 用 U2 graph+U3 artifacts+U4 tabs。
- 无依赖亦排序：U3 与 U2 无代码依赖但串行纪律执行。

## 9. 执行命令清单

```bash
# Rust
C1  cargo nextest run -p ralph-cli --test integration_inspect_run     # U1+U2 透传
C2  cargo nextest run -p ralph-cli -- inspect::descriptor             # U1 单测
C3  cargo nextest run -p ralph-cli --test integration_preset_builtin  # builtin 枚举回归
C4  cargo nextest run -p ralph-cli --test integration_wave_inspect    # inspect 回归
C5  cargo nextest run -p ralph-cli --test integration_dag_scheduler   # scheduler 透传回归
C6  ./scripts/check-cli-doc-drift.sh
C7  ./scripts/validate-builtin-presets.sh                             # sidecar 目录不误伤
C8  cargo fmt --all -- --check && cargo clippy -p ralph-cli -- -D warnings
C9  ./scripts/run-tests.sh                                            # 最终

# monitor
C10 npm test -w @ralph-monitor/server                                 # tsx --test
C11 npm run typecheck -w @ralph-monitor/server
C12 npm run build -w @ralph-monitor/server

# frontend
C13 npm test -w @ralph-web/dashboard
C14 npm run build -w @ralph-web/dashboard
C15 cd frontend/ralph-web && npx playwright test e2e/monitor-details.spec.ts
C16 cd frontend/ralph-web && npx playwright test                      # 全量 e2e 回归

B1  npm test && npm run build
```

## 10. 最终质量门禁

- [ ] S1–S10 全绿；R1–R4 追踪无空洞；
- [ ] descriptor↔preset publishes 一致性自检绿；builtin 枚举覆盖清单核对（4 sidecar + 其余 fallback 明示）；
- [ ] 幂等（重复 ingest）、并发（多文件轮转）、fault injection（坏行/截断/文件重置）通过；
- [ ] artifacts 安全矩阵全绿；Markdown XSS 矩阵绿；
- [ ] Rust 门禁（fmt/clippy/nextest/run-tests.sh/drift/validate-builtin）绿；TS/前端门禁绿；
- [ ] 无 skip/.only/弱化断言/未解释 golden；
- [ ] DAG 图与列表视图数据一致性断言存在；
- [ ] 无 `forge.wave.*` 新语义引入（E4 红线）；
- [ ] 置信度复核 ≥0.85；变更不超范围。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
| ---- | ---- | ---- |
| 实施计划而非 Roadmap | 是 | — |
| Executor 仍需关键设计决策 | 否 | D1–D10；两处"进入 Unit 时确认"已给验证动作与备选 |
| 文件/接口有代码库证据 | 是 | — |
| 关键决策置信度 ≥0.85 | 是 | 最低 D6=0.85 |
| 未处理低置信度假设 | 否 | — |
| 每 Unit 单一可观察行为 | 是 | — |
| 每 Unit 可独立验证 | 是 | — |
| 每 Unit 有真实 Red | 是 | — |
| 每 Unit 有回归范围 | 是 | — |
| 无未来 Unit 依赖 | 是 | U4 的 graph 依赖 U2（前置）非未来 |
| 无泛化描述 | 是 | — |
| Scenario→测试→Unit 追踪 | 是 | §6 |
| 关键决策有 Evidence | 是 | — |
| 可严格串行 | 是 | §8 |

---

**Series**: `feat/monitor-product` — 依赖 Plan 1（foundation）；Plan 3 将在本计划 `seq`/cursor/投影基础上实现 SSE 与恢复；Plan 4 的通知判定复用 timeline 与 terminal 证据。
