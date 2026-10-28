---
title: feat(monitor): 运行健康、网络观测与恢复
type: feat
date: 2026-10-28
baseline: 4c8718b77327d90087d6665e51654b58de90b805
execution: code
status: READY
depends_on: 2026-10-28-1949-feat-monitor-preset-run-details-plan
---

# 运行健康、网络观测与恢复

## 0. 计划状态

`READY` — 所有实施关键决策置信度均 ≥ 0.85，无 BLOCKED 项。

- **代码库基线**：`4c8718b77327d90087d6665e51654b58de90b805`。
- **前置依赖**：Plan 1（monitor 服务/采集器/projection/`/runs`）+ Plan 2（`run_events` 持久 seq、timeline/progress API、详情页）。本计划在其上叠加：monitor 全局事件流水（SSE）、四层健康分类、前端连接层。
- **调查范围**：`crates/ralph-adapters/src/{cli_executor.rs,claude_stream.rs}`、`crates/ralph-cli/src/loop_runner/{inner.rs,diagnostics}`、`crates/ralph-api/src/stream_domain/{mod.rs,filters.rs}`、`crates/ralph-api/src/transport.rs`、`frontend/ralph-web/src/hooks/{useTaskWebSocket,useLoopObservation,useNotifications}.ts`、`frontend/ralph-web/src/rpc/client.ts`、`.cursor/rules/observability.mdc`。
- **已执行验证**：adapter 结构化结果字段确认（exit_code/timeout/stderr/result event）；ralph-api stream 确认为**内存**实现（history 2048/broadcast 256/replay 200/keepalive 15s，重启即失）；现有前端 WS hook 确认面向 ralph-api `/rpc/v1/stream`。
- **尚未执行**：`.ralph/runs/<run_id>.activity.jsonl`（U1 新增）；monitor SSE 端点（U3）；`ConnectionProvider`（U4）。
- **阻塞项**：无。

本计划新增术语：

- **activity ledger**：`.ralph/runs/<run_id>.activity.jsonl`（U1 新增），runtime 追加写入的 per-hat 后端调用记录。
- **monitor event log**：monitor SQLite 全局流水表 `monitor_events(seq,kind,…)`，SSE 的持久游标依据。
- **watermark**：snapshot 响应携带的 monitor_events 最大 seq；客户端以 `snapshot→SSE(cursor=watermark)` 获得无洞序视图。
- **四层健康**：`connection`（浏览器↔monitor）、`collector`（monitor↔runtime 采集）、`runtime`（进程活性）、`backend`（模型请求证据）。

## 1. 功能目标

| R-ID | 需求（可观察行为） | 行为差异（当前 → 目标） |
| ---- | ---- | ---- |
| R1 | 浏览器连接、采集健康、runtime 活性、backend 请求观测四层**分别**展示，各有证据与最后有效时间 | 现状：仅 collector 成败标记（Plan1 S9）→ 目标：`/runs/:id/health` 与 `/projects/:id/health` 返回四层结构化证据；UI 分层展示 |
| R2 | 超时、DNS/连接错误、429、鉴权错误、未知状态按证据区分；**禁止**把"无输出"等同"断网" | 现状：adapter 只暴露 exit_code/timeout/is_error（E4）→ 目标：runtime 把每次后端调用的结构化结果追加到 activity ledger；monitor 按 `kind` 映射分类，证据不足一律 `unknown` |
| R3 | 断线、事件重复、乱序、服务重启、历史轮转后恢复一致视图；不误报完成 | 现状：ralph-api stream 是内存实现（E5）→ 目标：SQLite 持久 seq + SSE replay；重复事件 seq 去重；乱序由 (file,offset) 排序免疫；重启后 cursor 续读 |
| R4 | 恢复后补齐事件；cursor 过期返回显式 resync；保留最后有效状态与时间 | → `resync_required` 事件 + `last_valid{seq,at}`；客户端自动 snapshot→续订 |

范围：单机 monitor 自身与已注册项目的观测链路。非目标：多节点/跨机恢复、WebSocket（SSE 足够单向）、对模型 API 主动探活（E9 明确禁止付费探测）、通知（Plan4）、写操作（Plan5）。

输入：run_events（Plan2）、activity ledger（U1）、snapshot（Plan1）。输出：`/stream`（SSE）、`/snapshot`、`/runs/:id/health`、`/projects/:id/health`、前端连接层+健康面板。

错误语义：SSE cursor 早于保留窗口→首个事件 `{"type":"resync_required"}` 后关流；cursor 非法格式→400；慢消费者缓冲溢出→断开（客户端按协议 resync）；分类证据不足→`unknown`+`evidence` 明细。

兼容要求：SSE 端点为新增路径；不改 Plan1/2 既有 REST；`activity.jsonl` 为新增文件（旧 run 无此文件→`backend.healthy:"unknown"`，不报错）。

性能：SSE 心跳 15 s（沿用 ralph-api 先例 E5）；单页单连接（ConnectionProvider 单例）；monitor_events 保留窗口默认 50000 行（env 可调）。

安全：SSE 与 snapshot 同 loopback+会话校验（Plan1 D9 的 GET 免 CSRF 但需会话 cookie——决策：SSE 需会话 cookie，同 fetch 语义）。

已确认假设：A1 monitor 与 ralph 同机，`process.kill(pid,0)` 可做 PID 活性检查；A2 Linux `/proc/<pid>/stat` starttime 可做 PID 复用校验，非 Linux→`pid_unverified` 标注降级不误判；A3 写投影与 monitor_events 追加可同事务（better-sqlite3 同步事务保证）。

待验证假设：无。

## 2. 代码库现状与证据

### 2.1 当前实现入口与调用链

```text
# 后端调用观测面（runtime）
hat dispatch → CliExecutor / PtyExecutor / AcpClient (crates/ralph-adapters)
  → cli_executor.rs：exit_code、timeout(watchdog)、stderr tail、logical error mapping
  → claude_stream.rs：stream-json result event {duration_ms,total_cost_usd,num_turns,is_error,usage}
  （现状：结果只回流 event loop 内部，不落盘结构化记录 → E4）

# ralph-api 推送面（现状，不可复用为 durable 流）
WS /rpc/v1/stream → StreamDomain（内存）：history VecDeque≤2048、replay≤200、keepalive 15s、cursor/ack → E5

# 前端推送面（现状）
useTaskWebSocket/useLoopObservation → /rpc/v1/stream（ralph-api WS）→ E6
```

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
| ---- | ---- | ---- | ---- | ---- |
| E1 | `crates/ralph-core/src/event_logger.rs` + Plan2-D3 | run_events 已有持久 seq/byte-cursor；main ledger 权威 | SSE 流水以 run_events 为源之一；乱序免疫已成立 | 高 |
| E2 | `crates/ralph-cli/src/loop_runner/inner.rs` `handle_termination` | 统一终态管线唯一出口；SIGKILL/crash 不经此管线（无终态证据） | 死亡无 manifest terminal → `exited_unknown`/`interrupted`（KTD2 落地） | 高 |
| E3 | `crates/ralph-core/src/loop_registry.rs` + `loop_lock.rs` | PID 活性判断 `is_pid_alive`（kill 0）先例；registry 以 PID+started 标识 | monitor 复用同语义：`process.kill(pid,0)`+started_at 交叉校验 | 高 |
| E4 | `crates/ralph-adapters/src/cli_executor.rs` + `claude_stream.rs` | executor 可得：exit_code、timed_out、stderr tail；claude result event：is_error/duration_ms/num_turns/cost；**无 HTTP 状态可见性**（429/401 只在 stderr 文本里） | backend 层分类词表只能承诺 `{ok,exit_error,timeout,transport_unknown,unknown}` 级粒度；HTTP 细分仅能在 stderr 出现已知模式时标 `hint`（不承诺） | 高 |
| E5 | `crates/ralph-api/src/stream_domain/{mod.rs,filters.rs}` | 内存 history 2048/replay 200/keepalive 15s/cursor 校验+单调 ack/topic 白名单 | monitor durable log 设计对照基线；不声称复用 | 高 |
| E6 | `frontend/ralph-web/src/hooks/{useTaskWebSocket,useLoopObservation}.ts` + `rpc/client.ts` | 现有推送走 ralph-api WS `/rpc/v1/stream`（JSON-RPC 风格订阅） | monitor 侧用 EventSource(SSE)，不复用该 hook；互不干扰 | 高 |
| E7 | `backend/ralph-monitor`（Plan1/2 产物） | ingest 事务、`source_cursors`/`run_events` 表、`health` 路由雏形 | monitor_events 追加并入既有 ingest 事务 | 高 |
| E8 | `crates/ralph-cli/src/loop_runner/notifications.rs` | 终态 webhook best-effort 先例（无重试无持久） | Plan4 参考；本计划不涉 | 高 |
| E9 | 原计划 KTD3 + adapter 现实（E4） | 禁止付费探活；CLI backend 不透出网络层 | backend 观测仅采信 activity ledger 落盘证据，monitor 永不主动探测 | 高 |
| E10 | `.cursor/rules/observability.mdc` | `RALPH_DIAGNOSTICS=1` 才写 `.ralph/diagnostics/<ts>/agent-output.jsonl`；默认关闭 | diagnostics 不能作为 always-on 证据源；activity ledger 必须无开关常驻 | 高 |
| E11 | `crates/ralph-cli/tests/common/mod.rs` + `scripts/run-tests.sh` | nextest+env scrub | Rust 侧 U1 测试路径 | 高 |
| E12 | `frontend/ralph-web/playwright.config.ts` + `e2e/` | route-intercept 先例；webServer 字段 | U4 e2e 可用拦截模拟断线（route.abort()） | 高 |
| E13 | `frontend/ralph-web/package.json` | 无 eventsource/polyfill 依赖；浏览器原生 EventSource | 零新依赖；EventSource 自动重连仅按浏览器策略——需自管游标（EventSource 不支持自定义 header/cursor 参数？EventSource 支持 url query → 用 `?cursor=`） | 高 |
| E14 | `docs/achieved/plan/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md` | 结构先例 | 沿用 | 高 |
| E15 | `presets/monitor/`（Plan2-U1 产物） | descriptor.artifacts/槽位 | 健康面板不依赖 descriptor | 低（不耦合） |

### 2.3 受影响范围

**修改的既有文件**

- `crates/ralph-cli/src/loop_runner/`：hat 执行边界处追加 activity 记录写调用（**候选挂点：`inner.rs` 中调用后端执行器的单一 dispatch 点，进入 U1 时用 `rg "CliExecutor|execute" crates/ralph-cli/src/loop_runner/` 定位；若分散多处则收敛到 executor 调用包装**）。
- `backend/ralph-monitor`：`collector.ts`（事务内追加 monitor_events）、`run_projection.ts`（health 字段）、routes。
- `frontend/ralph-web/src/features/monitor/`：新增 ConnectionProvider/HealthPanel；RunDetailPage 顶部接入；`hooks.ts` 查询→流式升级。

**计划新增**

- `crates/ralph-cli/src/loop_runner/activity_log.rs`（候选；append-only writer `.ralph/runs/<id>.activity.jsonl`）。
- `backend/ralph-monitor`：`migrations/0005_monitor_events.sql`、`src/{stream/sse.ts,stream/monitor_log.ts,health/classify.ts,routes/stream.ts,routes/health_detail.ts}`。
- `frontend/ralph-web/src/features/monitor/{connection/ConnectionProvider.tsx,connection/useMonitorStream.ts,health/HealthPanel.tsx,health/LayerBadge.tsx}`；`e2e/monitor-recovery.spec.ts`。

**明确不修改**：ralph-api stream/WS；cli_executor 返回形状；`EventRecord`；Plan2 API 形状（SSE 是加法）；adapter 接口。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选 | 最终选择 | 支持证据 | 排除原因 | 置信度 |
| ---- | ---- | ---- | ---- | ---- | ---- | ---- |
| D1 | backend 观测证据来源 | A. monitor 主动探测模型 API；B. 读 `.ralph/diagnostics/*`；C. runtime 追加 activity ledger | C：runner 在 hat 执行边界写 `.ralph/runs/<id>.activity.jsonl` append-only | E9（禁探活）、E10（diagnostics 默认关）、E4（executor 已有结构化字段可落盘） | A 违反约束且付费；B 默认无数据 | 0.87 |
| D2 | activity 分类词表 | A. HTTP 级（timeout/dns/429/401/5xx）；B. `{ok,exit_error,timeout,transport_unknown,unknown}`+可选 `hint` | B：承诺可证据化粒度；stderr 已知模式（"429"、"rate limit"、"unauthorized"、"ENOTFOUND"等）→`hint` | E4（adapter 无 HTTP 可见性）、R2（区分以证据为准） | A 多数情况无证据→虚假精确 | 0.9 |
| D3 | runtime 活性判定 | A. 仅 kill(0)；B. kill(0)+manifest started_at 对 /proc 交叉；C. heartbeat 文件 | B：POSIX kill0 + Linux `/proc/<pid>` starttime 近似比对（±60s 容差），非 Linux/读失败→`pid_unverified` 标注重降级 | E3（kill0 先例）、KTD2（PID 结合启动身份）、假设 A1/A2 | C 要求 writer 线程/定时器——runtime 现状无心跳写点，新增成本高且 crash 即停（语义同 kill0，不增信息） | 0.86 |
| D4 | 推送协议 | A. WebSocket；B. SSE EventSource；C. 长轮询 | B：`GET /api/monitor/v1/stream?cursor=<seq>` SSE；`id:` 行=seq | E13（原生 EventSource 零依赖、自动重连）、E5（15s keepalive 先例）、单向推送足够 | A 双向无用且 ws 库新依赖；C 自造轮询重复造轮子 | 0.9 |
| D5 | 全局游标位置 | A. per-run seq；B. monitor 全局 `monitor_events.seq` | B：所有投影变更（run 更新/事件批次/健康变更）追加到全局表，客户端单游标 | KTD1（snapshot watermark→补后续事件）、E7（同事务可追加） | A 客户端需每 run 一游标一连接——违反"每页单流" | 0.9 |
| D6 | snapshot→stream 衔接 | A. snapshot 内嵌最新事件；B. snapshot 只给 watermark，事件只走 SSE；C. snapshot 后客户端再 GET 增量 | B：`/snapshot{watermark,data}` → `EventSource(?cursor=watermark)`；服务端保证 watermark 之后的事件都可重放 | KTD1 明文 | A/C 双通道重复数据引入去重复杂度 | 0.9 |
| D7 | 保留窗口与 resync | A. 无限保留；B. 行数窗口+`resync_required`；C. 时间窗口 | B：默认 50000 行（`RALPH_MONITOR_STREAM_RETENTION`）；cursor<min(seq) → 首事件 `resync_required` 后关流 | KTD4 | A 无界增长；C 时间与写入率解耦不可控 | 0.9 |
| D8 | 健康分类放置层 | A. 前端推导；B. 服务端 `/health` 计算 | B：服务端按证据矩阵出 `{layer,status,evidence{…},last_valid_at}`；前端只渲染 | 规则集中可测；前端不同视图一致 | A 三处页面重复实现+难测 | 0.9 |
| D9 | 慢消费者 | A. 无限缓冲；B. per-connection 缓冲上限+断开让客户端 resync | B：缓冲 1000 事件（env 可调），溢出→关流；客户端见断线即 backoff 重连+按 cursor 续读 | KTD4（可断开可补读）、E5（broadcast 256 先例同思路） | A 内存膨胀 DoS | 0.9 |
| D10 | 前端连接结构 | A. 每组件 EventSource；B. ConnectionProvider 单例+topic 订阅分发 | B：单实例管理连接/游标/backoff/resync；hooks 订阅本地 store | KTD4（每页单流） | A 多连接浪费+游标不一致 | 0.92 |

无 <0.85 决策。

实施协议：同 Plan1（验收/停止契约、nextest、env scrub、fake-ralph、drift 检查）。

## 4. BDD 行为规格

```gherkin
Feature: 观测链路健康与断线恢复

  Scenario: S1 四层健康分别呈现
    Given run 进程存活且事件持续推进、collector 正常、浏览器已连
    When 查看健康面板
    Then 四层均为 ok 且各层显示证据与 last_valid_at

  Scenario: S2 runtime 无事件不等于断网
    Given run 的 pid 存活但 10 分钟无新事件（activity ledger 也无记录）
    When 查看健康
    Then runtime 层显示 silent（pid_alive=true,last_event_at=…）
    And backend 层显示 unknown（无最近调用证据）而非"断网/离线"

  Scenario: S3 backend 超时与退出错误区分
    Given activity ledger 记录一次 timeout 调用与一次 exit_error(1) 调用
    When 查看 backend 层
    Then 显示最近错误类别 timeout 与 exit_error 及各自时间
    And 不将其渲染为网络断开

  Scenario: S4 进程死亡无终态→interrupted
    Given run 的 pid 已死、manifest.terminal 为空
    When 采集器运行
    Then run 状态为 exited_unknown/interrupted
    And 健康面板 runtime 层 dead_pid_evidence 明示
    And 不误报 completed

  Scenario: S5 PID 复用不误判
    Given manifest 记录 pid=4242 已退出
    And 系统新进程恰好复用 pid 4242
    When 活性检查运行（/proc 交叉）
    Then runtime 层显示 dead 或 pid_unverified
    And 不显示 running

  Scenario: S6 浏览器断线后补齐
    Given 客户端已同步到 seq=100
    And 浏览器与 monitor 断开 5 分钟（期间 seq 推进到 140）
    When 连接恢复且客户端以 cursor=100 重订
    Then 收到 101..140 全部事件且按 seq 有序
    And 视图与持续在线一致

  Scenario: S7 cursor 超出保留窗口显式 resync
    Given 保留窗口 50000 行，客户端持旧 cursor=10 而当前 min_seq=60000
    When 客户端以 cursor=10 重订
    Then 首事件为 resync_required
    And 客户端自动重新 /snapshot 并以新 watermark 续订

  Scenario: S8 事件重复/乱序恢复一致
    Given ingest 因故障重放同一段事件文件
    When 恢复完成
    Then run_events 无重复 seq
    And timeline 顺序严格 (file,offset)

  Scenario: S9 服务重启后恢复
    Given monitor 重启
    When 客户端重连
    Then snapshot watermark 与持久 seq 连续
    And 客户端无需全量重建历史（按 cursor 续订）
    And run 终态不丢失

  Scenario: S10 慢消费者断开后可补读
    Given 客户端处理速度低于事件产生速度
    When 服务端缓冲溢出
    Then 服务端断开该连接
    And 客户端按最后 seq 重新订阅即可补齐
    And UI 展示"已断开，正在重同步"而非数据缺失

  Scenario: S11 collector 故障层独立呈现
    Given 项目 A 采集连续失败而项目 B 正常
    When 查看健康
    Then A 的 collector 层 error（含错误摘要+失败次数）
    And B 不受影响
    And A 的 runs 标 stale 且 last_valid_at 保留

  Scenario: S12 终态后不出现幽灵告警
    Given run 已 exited_success 且进程已退出
    When 时间流逝
    Then runtime/backend 层不新增"无活动"告警
    And 面板显示终态收敛说明
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
| ---- | ---- | ---- | ---- | ---- | ---- |
| S1 | 四层 ok+证据字段 | `health/classify.test.ts`+`HealthPanel.test.tsx` | 单元+组件 | — | 否 |
| S2 | silent≠offline；backend=unknown | `classify.test.ts`（pid alive+陈旧事件 fixture） | 单元 | — | 否 |
| S3 | timeout/exit_error 分类呈现 | `classify.test.ts`+activity fixture | 单元+集成 | — | 否 |
| S4 | dead+无终态→exited_unknown | `classify.test.ts`+collector 集成 | 单元+集成 | — | 否 |
| S5 | /proc starttime 不符→非 running | `classify.test.ts`（mock fs /proc 注入——**决策：proc 读取抽象为接口可注入**，真实路径仅 linux 集成冒烟） | 单元 | — | 否 |
| S6 | 101..140 有序补齐 | `test/integration/stream.test.ts`（真 SSE client） | 集成 | Idempotency | `monitor-recovery` |
| S7 | resync_required 首事件+自动快照 | `stream.test.ts`+`useMonitorStream.test.ts` | 集成+组件 | — | `monitor-recovery` |
| S8 | 重放无重复 seq | `events_ingest`（Plan2）增补 fault 用例 | 集成 | Idempotency | 否 |
| S9 | 重启连续+终态保留 | `stream.test.ts`（重启 app 实例） | 集成 | — | 否 |
| S10 | 溢出断开+补读 | `stream.test.ts`（调小缓冲 env 注入） | 集成 | Fault injection | 否 |
| S11 | 项目级隔离+stale | `collector_isolation`（Plan1）补 health 断言 | 集成 | — | 否 |
| S12 | 终态收敛无告警 | `classify.test.ts` | 单元 | — | 否 |

## 6. 需求—测试追踪矩阵

| Requirement | Scenario | 验收测试 | 单元测试 | 集成/契约 | E2E | Evidence |
| ---- | ---- | ---- | ---- | ---- | ---- | ---- |
| R1 | S1/S11 | classify+HealthPanel | 四层矩阵 | health API 集成 | recovery | E3/E7 |
| R2 | S2/S3/S4/S5 | classify 矩阵 | 分类词表 | activity 集成 | — | E4/E9/E10 |
| R3 | S6/S8/S9 | stream+ingest | seq 去重 | SSE 集成 | recovery | E1/E5 |
| R4 | S7/S10/S12 | stream+classify | resync/终态收敛 | SSE 集成 | recovery | D6/D7 |

Unit 映射：S2–S5/S12→U1+U2；S1/S11→U2+U4；S6–S10→U3+U4。

## 7. 严格串行开发单元

```text
Unit 1（runtime：activity ledger 写入）
  ↓
Unit 2（monitor：四层健康分类 + /health 扩展）
  ↓
Unit 3（monitor_events 持久流水 + snapshot/SSE + resync）
  ↓
Unit 4（前端 ConnectionProvider + HealthPanel + recovery e2e）
```

### Unit 1：runtime activity ledger — hat 执行边界结构化落盘

#### 1. Unit 目标
`ralph run` 期间每次后端（模型 CLI）调用在 `.ralph/runs/<run_id>.activity.jsonl` 追加 `{ts,seq_in_run,hat,backend,phase:"start"|"end",exit_code?,timed_out?,is_error?,duration_ms?,stderr_tail_hint?}`；文件无开关常驻（不依赖 RALPH_DIAGNOSTICS）；SIGKILL 时已写行可消费。

#### 2. 对应需求与 Scenario
R2 证据源；S3 输入；D1/D2；E4/E9/E10。

#### 3. 外部可观察结果
run 目录出现 activity.jsonl；每 hat 调用产生 start+end 两行（或 crash 仅 start）；`inspect run` 增补 `activity_file` 字段输出路径（加法字段）。

#### 4. 当前行为基线
无该文件（Red）；executor 结果只在内存回流（E4）。

#### 5. 输入与输出
输入：hat 执行的 spawn 参数（backend 名）与结束结果（exit_code/timed_out/is_error/duration）。输出：JSONL 行。副作用：append 写（小文件锁或 O_APPEND）。不变量：不阻塞执行路径（写入失败记 stderr 不 panic——参照 notifications best-effort 先例 E8）；单行 ≤4KB（stderr tail 只存 `hint` 分类不存原文——隐私/体积）。

#### 6. 修改位置
- `crates/ralph-cli/src/loop_runner/activity_log.rs`（新增）：`ActivityLog::open(run_dir)`、`record_start`、`record_end`（内部 seq 自增、fsync 策略=每次 write（行小量低））。
- `crates/ralph-cli/src/loop_runner/inner.rs`：hat 执行边界两处调用（**进入 Unit 时用 `rg -n "CliExecutor|PtyExecutor|execute_hat|dispatch_hat|run_hat" crates/ralph-cli/src/loop_runner/` 确定唯一插桩点；若经 `EventLoop::dispatch` 统一出口则只插桩该出口**）。
- `crates/ralph-core/src/run_manifest.rs`（Plan1 新增处）：manifest 增补 `activity_file` 字段（或 inspect run 层枚举 `.ralph/runs/<id>.activity.jsonl` 直接推导——**决策：inspect run 推导即可，manifest 不加字段，减少写入面**）。
- `crates/ralph-cli/src/commands/inspect/run.rs`：run 项输出 `activity_file`（存在与否+路径）。
- `stderr_tail_hint`：executor 侧小函数 `classify_stderr(&str)->Option<"rate_limited"|"unauthorized"|"dns"|"timeout"|"unknown_pattern">`（模式清单小且保守）。
- 测试：`activity_log.rs` 内单测 + `integration_inspect_run.rs`/`integration_activity_log.rs`（新增）。

#### 7. 可依赖能力
CliExecutor 结果字段（E4）；manifest/runs 目录（Plan1）；FAKE_PATH_BACKEND fixture（可造不同退出码/超时/输出行）。

#### 8. 禁止依赖的未来能力
monitor 读取端（U2）；SSE；不改 adapter trait。

#### 9. 验收测试
- `activity_log` 单测：start/end 行形状、seq 自增、写失败不 panic、hint 分类矩阵。
- `integration_activity_log.rs`：fake backend `ralph run` → activity.jsonl 存在且含 ≥1 对 start/end；timeout fixture→`timed_out:true`；stderr "rate limit"→hint=rate_limited。
- `inspect run` 输出 `activity_file`。
- 命令：`cargo nextest run -p ralph-cli --test integration_activity_log`、`-- activity_log`。

#### 10. Acceptance Red
断言文件/字段不存在（有效 Red）。

#### 11. 单元测试拆分
如上；另：并发 append（两 hat 并行写→行不交错？——append 单行原子（≤PIPE_BUF 不适用文件——用 `OpenOptions.append` + 单次 write_all 保证单行原子；测试多线程写后每行可解析）。

#### 12. Red → Green → Refactor
```text
单测 Red → writer+hint → Green → integration Red → 插桩接线 → Green → inspect 字段 Red → Green → Refactor
```

#### 13. 最小实现范围
writer+插桩+hint+inspect 字段。不实现：rotation（run 级单文件有界——hat 调用次数有限，估算 <10k 行可接受）、内容上传、网络字段探测。

#### 14. 集成验证
fake backend 三态（ok/exit1/timeout）；`integration_run*` 回归（插桩不得破执行）。

#### 15. 风险驱动测试
Fault：磁盘满/只读→执行继续（断言 run 不因写失败终止——用不可写路径注入）；并发 append 行完整性。

#### 16. 回归范围
`integration_run`、`integration_run_presets`、`integration_events_isolation`（执行路径相邻）；`audit-file-sizes`（新模块入单）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `loop_runner/activity_log.rs` | 新增 | ledger writer | D1 |
| `loop_runner/inner.rs`（或 dispatch 点） | 修改 | 插桩 | 候选挂点见 §6 |
| `inspect/run.rs` | 修改 | activity_file 字段 | — |
| `tests/integration_activity_log.rs` | 新增 | ATDD | E11 |

#### 18. 完成标准
上列测试绿；Rust 门禁；写失败路径不 panic 有日志。

#### 19. 停止条件
hat 执行无统一插桩点且分散 >3 处→stop：重构收敛出口或改在 `cli_executor` 层加可选 sink（需评估 ralph-adapters 反向依赖 ralph-cli 的类型——可能需把 writer trait 定义在 adapters 侧；重算置信度）。

#### 20. 风险与注意事项
- 插桩遗漏某 backend 路径（ACP/Pty）→测试矩阵覆盖三种 executor；残留=未覆盖路径 → 无行 → monitor 显示 unknown（安全降级语义已含）。

---

### Unit 2：monitor 四层健康分类 + `/runs/:id/health` + `/projects/:id/health` 扩展

#### 1. Unit 目标
服务端按证据矩阵计算四层健康；`GET /runs/:id/health`→`{run_id,layers:{collector,runtime,backend},terminal_converged,last_valid_at}`；`/projects/:id/health` 扩展为 `{collector:{status,error?,consecutive_failures,last_collected_at},degraded_runs}`；浏览器连接层由前端自评（不进 API）。

#### 2. 对应需求与 Scenario
R1/R2；S1–S5/S11/S12；D2/D3/D8；E3/E4/E9。

#### 3. 外部可观察结果
API 如上；分类词表：collector `{ok,error,stale}`；runtime `{running,silent,exited,dead_unverified,pid_unverified,unknown}`；backend `{ok,timeout,exit_error,transport_unknown,unknown}`+`hint?`；`terminal_converged`（终态后抑制告警 S12）。

#### 4. 当前行为基线
`/projects/:id/health` 仅 collector（Plan1-U3）；run 级无 health（Red）。

#### 5. 输入与输出
输入：run 行（pid/started_at/terminal）、event_cursors/last_event_at、activity.jsonl（U1，collector 追加 ingest 该文件——本 Unit 同时扩展 ingest：activity byte-cursor 入 `run_activity` 表同 U2 事务）、collector 失败计数。输出：health JSON。状态变化：`run_activity(project_id,run_id,seq,phase,kind,at,detail)`、`activity_cursors`。不变量：无证据→unknown；终态收敛标记抑制 runtime silent。

#### 6. 修改位置
- `migrations/0004_activity.sql`（与 monitor_events 同批但独立编号 `0004_activity.sql`，monitor_events 用 `0005`）——**编号统一：本计划迁移为 `0004_activity.sql`+`0005_monitor_events.sql`**。
- `src/activity/ingest.ts`：复用 Plan2 byte-cursor 框架读 activity.jsonl。
- `src/health/classify.ts`：纯函数 `classify({run,lastEventAt,activityRows,collector,pidCheck,now})`；`pidCheck` 抽象接口（注入实现：posix kill0+proc starttime 解析 `src/health/pid_check.ts`）。
- `src/routes/health.ts` 扩展+`runs.ts` 子路由。
- collector ingest 事务接入 activity 文件（E7）。
- 测试：`test/unit/classify.test.ts`（全矩阵）、`test/integration/{activity_ingest,health_api}.test.ts`。

#### 7. 可依赖能力
Plan2 ingest 框架；U1 fixture；node `process.kill(pid,0)`（POSIX）。

#### 8. 禁止依赖的未来能力
SSE（U3）；前端展示（U4）。

#### 9. 验收测试
S1–S5/S11/S12 的 API 断言矩阵；`pid_check` 单测（kill0 成功/ESRCH/EPERM（存活但无权）/proc 读失败分支）。

#### 10. Acceptance Red
`GET /runs/:id/health` 404。

#### 11. 单元测试拆分
分类全矩阵（约 20 case：pid 活+新鲜事件→running；pid 活+陈旧→silent；pid 死+terminal→exited；pid 死+无终态→dead_unverified(exited_unknown 映射)；proc 不符→dead_unverified+pid_unverified；activity 近 ok→backend.ok；timeout→timeout；exit!=0→exit_error；仅 start 无 end+pid 活→transport_unknown?——决策：`in_flight` 标 `unknown`+`pending:true`；终态→terminal_converged=true）。
不允许 mock：sqlite/fs 用真临时路径；pid 用 spawn 真子进程（sleep）验证 kill0。

#### 12. Red → Green → Refactor
```text
pid_check Red→Green → classify 矩阵 Red→Green → activity ingest Red→Green → health API Red→Green → Refactor
```

#### 13. 最小实现范围
分类+API+activity ingest。不实现：阈值多档（silent 阈值固定 5min，env `RALPH_MONITOR_SILENT_MS`）、趋势图数据、告警（Plan4）。

#### 14. 集成验证
fake ralph fixture：活 run（sleep 假进程+真 pid 写入 manifest fixture）+activity.jsonl 样本；collector 端到端。

#### 15. 风险驱动测试
Fault：activity 文件缺失（旧 run）→backend.unknown 不报错；文件半截行→cursor 停顿；proc 不存在（macOS）→pid_unverified。
State-machine：终态后所有层收敛断言。

#### 16. 回归范围
Plan1/2 全部 monitor 测试；runs API 形状兼容（加法字段）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `migrations/0004_activity.sql` | 新增 | activity 投影 | D1 |
| `src/{activity/ingest,health/classify,health/pid_check,routes/health}.ts` | 新增/修改 | 分类+API | D3/D8 |
| `src/collector/collector.ts` | 修改 | ingest 接入 | E7 |

#### 18. 完成标准
S1–S5/S11/S12 数据面绿；门禁绿。

#### 19. 停止条件
kill0 在某 CI 平台对僵尸 pid 语义差异→降级标注路径兜底；其余按停止契约。

#### 20. 风险与注意事项
- pid 复用竞态（manifest 写后进程退出复用）：proc starttime 交叉+`pid_unverified` 明示而非误报 running。
- silent 阈值误报长思考 hat：阈值可配+面板展示"无事件≠离线"说明文案（S2）。

---

### Unit 3：monitor_events 持久流水 + `/snapshot` + `/stream` SSE + resync

#### 1. Unit 目标
每次 ingest 事务把投影变更（run 新增/更新、event 批次到达、health 变更、project 变更）追加到 `monitor_events(seq AUTOINCREMENT,kind,project_id,run_id,payload,at)`；`GET /api/monitor/v1/snapshot`→`{watermark,projects,runs,health}`；`GET /api/monitor/v1/stream?cursor=N` SSE：cursor>min→按 seq 重放后转 live；cursor<min→`resync_required`；心跳 15s；慢消费者溢出断开；重启后 seq 连续。

#### 2. 对应需求与 Scenario
R3/R4；S6/S7/S9/S10；D4/D5/D6/D7/D9；E5/E7。

#### 3. 外部可观察结果
SSE 事件：`id:<seq>`、`event:<kind>`、`data:<json>`；`resync_required`；`heartbeat`（注释行 `:hb`+或 event heartbeat）。snapshot watermark 数值。

#### 4. 当前行为基线
无 monitor_events 表/无端点（Red）；run_events 行内 seq 已存在（Plan2）。

#### 5. 输入与输出
输入：ingest 事务输出变更集；SSE cursor。输出：snapshot/stream。状态变化：monitor_events 表+保留窗口裁剪（每次事务后 `DELETE WHERE seq < max-50000`）。副作用：SSE 连接注册/断开。不变量：同事务原子性（投影与流水要么都写要么都不写——A3）；seq 永不重用；重放内容与历史一致。

#### 6. 修改位置
- `migrations/0005_monitor_events.sql`。
- `src/stream/monitor_log.ts`：`append(txn,kind,payload)`；collector/projection 写点枚举（run upsert diff 才发 `run.updated`——减少噪音；新 run→`run.created`；event ingest→`run.events{run_id,from_seq,to_seq,count}`；health 变化→`run.health`/`project.health`——**决策：投影 diff 由 ingest 返回"已变更集合"产生，避免全表比对**）。
- `src/stream/sse.ts`：连接管理（cursor 解析/重放/缓冲/心跳/溢出断开/resync 事件）。
- `src/routes/stream.ts` + `snapshot.ts`（复用 U3 投影读取）。
- 测试：`test/unit/monitor_log.test.ts`、`test/integration/{stream,snapshot,resync,slow_consumer}.test.ts`（起真端口 EventSource 客户端——node22 无原生 EventSource→测试内用 `fetch`+手动解析 stream 或最小 sse parser helper ~50 行；决策：测试 helper `parseSse(response.body)`）。

#### 7. 可依赖能力
better-sqlite3 事务；fastify reply.raw 流写出；Plan1/2 ingest 事务点。

#### 8. 禁止依赖的未来能力
前端（U4）；通知事件类型（Plan4 加 kind 枚举即可，本 Unit 预置 kind 字符串可扩展）。

#### 9. 验收测试
- `stream.test.ts`：cursor 续读有序补齐（S6）；非法 cursor→400；cursor 0/空→从最早重放；
- `resync.test.ts`：cursor<min→首事件 resync_required 后流关（S7）；
- `slow_consumer.test.ts`：注入小缓冲+阻塞读→断开（S10）；
- `snapshot.test.ts`：watermark=当前 max seq；重启后 seq 连续（S9）；
- `monitor_log` 单测：同事务、裁剪边界、kind 枚举。

#### 10. Acceptance Red
端点 404/模块缺失。

#### 11. 单元测试拆分
裁剪（留窗+边界等值）、cursor 解析（负数/非数/超大）、缓冲溢出计数、心跳间隔写、kind payload schema per kind。
不允许 mock：sqlite 真事务；SSE 真 socket。

#### 12. Red → Green → Refactor
```text
monitor_log Red→Green → snapshot Red→Green → stream 重放 Red→Green → resync/慢消费者 Red→Green → Refactor
```

#### 13. 最小实现范围
流水+两端点+resync+心跳+溢出。不实现：分 topic 过滤订阅（v1 全量流，客户端侧过滤——记录；若 run 数大再加 filter param）、WebSocket、压缩。

#### 14. 集成验证
真端口+node fetch SSE 客户端集成；与 collector 联动（ingest→流水可见）。

#### 15. 风险驱动测试
- Fault：连接中途 socket 断→服务端清理注册；写端 DB 忙→该轮流水同事务回滚不外泄半状态。
- Concurrency：10 并发连接各自 cursor。
- Idempotency：重放两次输出字节级一致。

#### 16. 回归范围
monitor 全部；`snapshot` 与既有 `/runs` 一致性差分断言（同数据源）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `migrations/0005_monitor_events.sql` | 新增 | 持久流水 | D5 |
| `src/stream/{monitor_log,sse}.ts`、`src/routes/{stream,snapshot}.ts` | 新增 | 协议 | D4/D6 |
| `src/collector/collector.ts`、`src/db/run_projection.ts` | 修改 | 变更集→流水 | E7 |
| `test/**` | 新增 | — | — |

#### 18. 完成标准
S6/S7/S9/S10 绿；门禁绿；watermark→replay 无缝断言存在。

#### 19. 停止条件
fastify 对 SSE 长连接默认超时/缓冲拦截→关闭对应 timeout 配置并验证；sqlite 长事务与 SSE 重放并发读写锁→WAL 已开（Plan1），必要时把重放读取放只读连接。

#### 20. 风险与注意事项
- 事务内追加流水的写放大：变更集 diff 策略控制事件粒度（run.events 批次化不单事件）。
- 保留窗口硬删与进行中 SSE 重放竞态：重放先 snapshot min/max 再按页读，读到已删段→发 resync_required。

---

### Unit 4：前端 ConnectionProvider + HealthPanel + recovery e2e

#### 1. Unit 目标
`ConnectionProvider`（monitor feature 顶层）管理单 EventSource：启动取 `/snapshot`→写 react-query 缓存基线→`EventSource(?cursor=watermark)` 增量应用→断线 backoff 重连（指数+jitter，cursor=last_seq）→收到 `resync_required` 自动重 snapshot；顶部连接徽章（connected/reconnecting/offline+last_valid_at）；详情页 HealthPanel 渲染四层；e2e 断线恢复旅程。

#### 2. 对应需求与 Scenario
R1/R3/R4 用户面；S1/S6/S7/S10/S11/S12；D8/D9/D10；E12/E13。

#### 3. 外部可观察结果
见 Unit 目标；列表/详情数据由流驱动（`run.updated`→invalidate/patch react-query 缓存——决策：事件驱动 queryClient.setQueryData 增量+周期兜底 invalidate）；离线时保留最后数据+灰化+banner。

#### 4. 当前行为基线
数据走 3s react-query 轮询（Plan1/2）；无 SSE/连接态（Red）。

#### 5. 输入与输出
输入：snapshot+SSE。输出：缓存更新+UI 层。副作用：单 EventSource；`document.hidden` 时保持连接（决策：不断开，心跳便宜）。不变量：重复 seq 去重（last_seq 比较）；不丢 last_valid。

#### 6. 修改位置
- `features/monitor/connection/{ConnectionProvider.tsx,useMonitorStream.ts,sseClient.ts}`（新增：EventSource 封装、事件→queryClient 分发器、backoff）。
- `features/monitor/health/{HealthPanel.tsx,LayerBadge.tsx}`（新增）；`RunDetailPage` 顶栏接入+`MonitorLayout` 顶部连接徽章。
- `hooks.ts`：useRuns 等由"轮询"改"snapshot 基线+流增量"（refetchInterval 保留 60s 兜底）。
- `e2e/monitor-recovery.spec.ts`（新增：route.abort() 断流→断言 offline 徽章→恢复→断言数据一致——EventSource 拦截：playwright 对 SSE 拦截用 `page.route` 返回受控流或改打 service worker；**进入 U4 时验证拦截手段，备选用真实 monitor+杀进程**）。
- 测试：`useMonitorStream.test.ts`（重放/resync/backoff/去重）、`HealthPanel.test.tsx`、`LayerBadge.test.tsx`。

#### 7. 可依赖能力
U3 协议；react-query；jsdom EventSource 需 polyfill for tests→`eventsource` npm？——决策：单测不依赖原生 EventSource：sseClient 抽象成 fetch 流解析（同服务端测试 helper 思路），生产用原生 EventSource 仅一行薄封装；测试全可控。

#### 8. 禁止依赖的未来能力
通知 UI（Plan4）；actions（Plan5）。

#### 9. 验收测试
`useMonitorStream`：S6 补齐/S7 resync/去重/backoff 上限/卸载清理；`HealthPanel`：四层矩阵渲染+terminal_converged 收敛文案；e2e 断线恢复（S6/S7/S10 旅程）。

#### 10. Acceptance Red
Provider/组件缺失；e2e 徽章元素不存在。

#### 11. 单元测试拆分
sseClient 解析（多行 data/comment/id/重连 last-event-id——决策：自定义 `?cursor=` 为准，不依赖 Last-Event-ID 头）；dispatcher kind→queryClient 映射表；backoff（max 30s、jitter±25%）；seq 去重。

#### 12. Red → Green → Refactor
```text
sseClient Red→Green → dispatcher/store Red→Green → Provider+徽章 Red→Green → HealthPanel Red→Green → e2e Red→Green
```

#### 13. 最小实现范围
连接层+徽章+面板+e2e。不实现：多 tab 页协调（BroadcastChannel 领导者选举——记录为后续）、主题化告警。

#### 14. 集成验证
vitest；对真 monitor 手工断开验证；playwright spec。

#### 15. 风险驱动测试
Fault：SSE 中连损坏 chunk→解析容错；e2e `route.abort()`；React StrictMode 双挂载→单连接断言（provider 内部 refcount/effect 清理）。

#### 16. 回归范围
vitest 全量；Plan1/2 e2e；`App.test`。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `connection/*`、`health/*` | 新增 | 连接+面板 | D8/D10 |
| `MonitorLayout`/`RunDetailPage`/`hooks.ts` | 修改 | 接入 | — |
| `e2e/monitor-recovery.spec.ts` | 新增 | ATDD | E12 |

#### 18. 完成标准
S1/S6/S7/S10/S11/S12 用户面绿；全门禁；单连接断言。

#### 19. 停止条件
playwright 无法可控拦截 EventSource→备选：e2e 起真 monitor（webServer）+kill/restart 进程验证恢复——记录切换不降级断言。

#### 20. 风险与注意事项
- 浏览器后台标签节流：心跳到 60s 级别可能被延——可接受（数据由重连补）；文档注明。
- StrictMode 重放副作用：订阅逻辑纯化。

## 8. Unit 串行依赖图

```text
Unit 1 → Unit 2 → Unit 3 → Unit 4
```

- U2 依赖 U1 的 activity 文件契约（fixture 先行）；U3 依赖 U2 之前的 ingest 事务与投影变更点；U4 依赖 U3 协议。
- U2 与 U3 无严格代码依赖（流水可在 classify 前落），但串行纪律保持且 U3 事件 kind 含 health 变更需要 U2 字段。

## 9. 执行命令清单

```bash
# Rust（U1）
C1  cargo nextest run -p ralph-cli --test integration_activity_log
C2  cargo nextest run -p ralph-cli -- activity_log
C3  cargo nextest run -p ralph-cli --test integration_run integration_run_presets integration_events_isolation
C4  cargo nextest run -p ralph-cli --test integration_inspect_run
C5  ./scripts/check-cli-doc-drift.sh && ./scripts/audit-file-sizes.sh
C6  cargo fmt --all -- --check && cargo clippy -p ralph-cli -- -D warnings

# monitor（U2/U3）
C7  npm test -w @ralph-monitor/server
C8  npm run typecheck -w @ralph-monitor/server && npm run build -w @ralph-monitor/server

# frontend（U4）
C9  npm test -w @ralph-web/dashboard
C10 npm run build -w @ralph-web/dashboard
C11 cd frontend/ralph-web && npx playwright test e2e/monitor-recovery.spec.ts
C12 cd frontend/ralph-web && npx playwright test

B1  npm test && npm run build
B2  ./scripts/run-tests.sh
```

## 10. 最终质量门禁

- [ ] S1–S12 全绿；R1–R4 追踪无空洞；
- [ ] 四层分类矩阵单元测试覆盖全部词表分支；
- [ ] seq 去重/重放一致/保留窗口 resync/慢消费者断开 通过；
- [ ] "无输出≠断网" 断言（S2）、"终态收敛无告警"（S12）存在；
- [ ] PID 复用交叉验证路径有测试；
- [ ] SSE 心跳、cursor 校验、同会话要求验证；
- [ ] Rust 门禁+monitor typecheck/test/build+前端 vitest/build/playwright 全绿；
- [ ] 无 skip/.only/断言弱化；无付费 API 探测代码路径（E9）；
- [ ] 置信度复核 ≥0.85；变更不超范围。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
| ---- | ---- | ---- |
| 实施计划而非 Roadmap | 是 | — |
| Executor 仍需关键设计决策 | 否 | D1–D10；唯一候选点（插桩位置）给了定位命令与备选 |
| 文件/接口有代码库证据 | 是 | — |
| 关键决策置信度 ≥0.85 | 是 | 最低 D3=0.86 |
| 未处理低置信度假设 | 否 | — |
| 每 Unit 单一可观察行为 | 是 | — |
| 每 Unit 可独立验证 | 是 | — |
| 每 Unit 有真实 Red | 是 | — |
| 每 Unit 有回归范围 | 是 | — |
| 无未来 Unit 依赖 | 是 | — |
| 无泛化描述 | 是 | — |
| Scenario→测试→Unit 追踪 | 是 | §6 |
| 关键决策有 Evidence | 是 | — |
| 可严格串行 | 是 | §8 |

---

**Series**: `feat/monitor-product` — 依赖 Plan 1+2；Plan 4 的通知将消费 `monitor_events`/`run_events` 与 terminal 证据；Plan 5 的操作确认复用 SSE 流观察回执。
