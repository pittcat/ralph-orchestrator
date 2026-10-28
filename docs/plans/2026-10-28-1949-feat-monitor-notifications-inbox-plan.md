---
title: feat(monitor): 可靠通知与待处理中心
type: feat
date: 2026-10-28
baseline: 4c8718b77327d90087d6665e51654b58de90b805
execution: code
status: READY
depends_on: 2026-10-28-1949-feat-monitor-health-recovery-plan
---

# 可靠通知与待处理中心

## 0. 计划状态

`READY` — 所有实施关键决策置信度均 ≥ 0.85，无 BLOCKED 项。

- **代码库基线**：`4c8718b77327d90087d6665e51654b58de90b805`。
- **前置依赖**：Plan 1–3（monitor 服务/持久 projection/`run_events` seq/`monitor_events` 流水/SSE/健康分层/前端连接层）。
- **调查范围**：`crates/ralph-core/src/config/notifications.rs` + `notifications/dispatch.rs`、`crates/ralph-cli/src/loop_runner/notifications.rs`、`crates/ralph-api/src/planning_domain.rs`（PlanningSessionRecord+sessions_dir）、`.ralph/planning-sessions/`、`crates/ralph-api/src/protocol.rs`（`planning.prompt.issued`/`planning.response.recorded` topics）、`frontend/ralph-web`（ui 组件/hooks/路由）、`docs/achieved/plan/`。
- **已执行验证**：runtime 既有 webhook 确认=best-effort 单发（无持久/重试/delivery id，E3）；planning session 落盘格式确认（`PlanningSessionRecord{id,prompt,status,created_at,updated_at,iterations}`，`sessions_dir=.ralph/planning-sessions`，E5）；`ralph emit`/topics 白名单确认。
- **尚未执行**：`notifications`/`deliveries`/`pending_interactions` 表（U1）；webhook 渠道（U2）；Inbox UI（U3）。
- **阻塞项**：无。

本计划新增术语：

- **notification（通知）**：由规则匹配产生的一次性记录（kind ∈ `run.completed|run.failed|interaction.pending`），含指向 run 证据的深链。
- **pending interaction（待处理交互）**：需要人工输入的会话（v1 = `.ralph/planning-sessions/*.json` 状态为待响应者 + descriptor `waiting.topics` 匹配事件）；消费后自动消退。
- **delivery（投递）**：某 notification 在某一渠道的一次至少一次投递尝试；`delivery_id` 供接收端去重。
- **outbox**：与事件 ingest 同事务写入的通知+投递队列，后台 worker 异步执行。

## 1. 功能目标

| R-ID | 需求（可观察行为） | 行为差异（当前 → 目标） |
| ---- | ---- | ---- |
| R1 | 运行完成、失败、受支持的等待输入事件进入**持久**通知与待处理中心；点击通知回到运行证据 | 现状：仅 runtime 终态 webhook（E3，无持久/重试/中心）→ 目标：`/monitor/inbox` + `GET /notifications` 持久列表；通知深链 `/monitor/runs/:gid?tab=…` |
| R2 | 浏览器关闭后 webhook 仍可投递；失败可见、可重试 | 现状：进程内 best-effort（E3）→ 目标：deliveries 表 + worker 指数退避 + 手动重试；浏览器不在场不影响 |
| R3 | 重复采集/重放不重复创建通知；已处理交互自动从待办消退 | → 幂等键 `sha256(run|event_seq|rule|rule_version)` 唯一约束；interaction 状态回读→`resolved` |
| R4 | 站内通知、用户授权的浏览器通知、一个通用 webhook；配置可测试 | → 三渠道独立 delivery；浏览器通知仅页面在场+权限授予时；webhook 可测试投递 |

范围：单机 monitor。非目标：邮件/IM 渠道（通用 webhook 覆盖）、多用户通知中心、通知偏好细粒度规则编辑（v1 规则集固定、渠道可开关）、对 runtime `notifications:` 配置的改写（只读提示）。

输入：`monitor_events`/`run_events`/`runs` 终态证据、planning session 文件、descriptor `waiting.topics`。输出：`/api/monitor/v1/notifications*`、Inbox UI、webhook HTTP POST。

错误语义：webhook 4xx→`failed`（不重试，可人工重试）；5xx/超时/连接错误→按退避重试至上限后 `dead`；SSRF 违规→`rejected`；渠道配置非法→422。

兼容要求：不改项目 `ralph.yml` 的 `notifications:` 配置（E3）；UI 对配置了 runtime webhook 的项目显示"可能重复"提示（KTD3）。

性能：outbox 同事务写入；worker 单循环每 5 s 拉取 due 投递（上限 20/批）；Inbox 分页 50。

安全（KTD4 落地）：webhook URL `https:` 默认；`http:` 仅当目标命中显式 allowlist；DNS 解析后校验全部地址（拒 loopback/link-local/private/multicast，allowlist 除外）；`redirect:'manual'` 禁跟随；secret 存 DB 但 API 掩码（`•••{last4}`）；日志脱敏 URL query；payload 只含 run 元数据不含 prompt 全文。

已确认假设：A1 等待输入的可证据化来源=planning session 文件（E5）+ descriptor 可声明 `waiting.topics`（Plan2 schema 预留扩展位）；A2 浏览器通知仅页面打开时可用（无 service worker 推送计划，R2 由 webhook 兜底）；A3 monitor worker 进程内单实例足够（单机）。

待验证假设：无。

## 2. 代码库现状与证据

### 2.1 当前实现入口与调用链

```text
# runtime 既有通知（不可复用为可靠通道）
handle_termination → notify_loop_termination (loop_runner/notifications.rs)
  → NotificationsConfig{enabled,timeout_seconds=5,endpoints[{name,url,on,headers,body}]}   # config/notifications.rs
  → 模板变量 8 个（status/loop_id/prompt 等）；best-effort；URL 脱敏；FakeTransport 测试 → E3

# 等待输入证据（runtime）
ralph plan → planning session → .ralph/planning-sessions/<id>.json   # PlanningSessionRecord{status,…} → E5
ralph-api planning.start/respond → status 转移 → planning.prompt.issued / planning.response.recorded topics → E6
```

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
| ---- | ---- | ---- | ---- | ---- |
| E1 | `backend/ralph-monitor`（Plan1–3 产物） | ingest 事务、`monitor_events` 流水、`run_events`（含 terminal 证据经 runs.terminal）、SSE、会话/CSRF | outbox 与投递复用同一事务与流；UI 经 SSE 获 `notification.created` | 高 |
| E2 | `crates/ralph-cli/src/loop_runner/inner.rs` | 终态写 manifest `terminal{reason,at,merge_commit}`（Plan1-U1） | `run.completed/failed` 通知由 runs 终态转移触发，不依赖事件 topic 猜测 | 高 |
| E3 | `crates/ralph-core/src/config/notifications.rs` + `notifications/dispatch.rs` + `loop_runner/notifications.rs` | 项目级 `notifications:` webhook：enabled/endpoints[{name,url,on,headers,body}]/timeout 5s；best-effort、模板 8 变量、URL 脱敏、FakeTransport；无持久/重试/dedup/delivery id | monitor 通知是并行新系统；UI 提示共存重复风险；**禁止**改写该配置 | 高 |
| E4 | `.ralph/` 实况 + `ralph.yml`（本仓库根） | 项目配置文件存在（本仓库 `ralph.yml` 含/不含 notifications 节——进入 U3 时读取确认提示逻辑样例） | collector 可只读扫描 `<project>/ralph.yml` 提取 `notifications.enabled` 作重复提示 | 中 |
| E5 | `crates/ralph-api/src/planning_domain.rs`（`PlanningSessionRecord`+`sessions_dir`）+ `.ralph/planning-sessions/` | session JSON：`{id,prompt,status,created_at,updated_at,iterations}`；status 词表待读文件确认（进入 U1 时 fixture 实测） | pending interaction 主证据源；collector 枚举该目录纳入 ingest | 高 |
| E6 | `crates/ralph-api/src/protocol.rs` | stream topics 含 `planning.prompt.issued`、`planning.response.recorded` | 与文件证据互证；monitor 走文件侧（无需 ralph-api） | 中 |
| E7 | `frontend/ralph-web` ui/hooks（Plan1-U4 产物） | dialog/command/table/skeleton/toast 已建；api client+CSRF；ConnectionProvider SSE | Inbox/设置复用；浏览器通知挂 `notification.created` 流事件 | 高 |
| E8 | `frontend/ralph-web/package.json` | 无 service worker/web-push 依赖 | 浏览器通知=页面在场时 `new Notification`（权限显式授权）；不做 push | 高 |
| E9 | `docs/achieved/plan/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md` | 结构先例 | 沿用 | 高 |
| E10 | `crates/ralph-cli/src/commands/emit/mod.rs` | `ralph emit --file` 可向 ledger 写事件 | fixture 可构造 waiting topic 事件 | 中 |

### 2.3 受影响范围

**修改的既有文件**

- `backend/ralph-monitor`：`collector.ts`（ingest 通知检测+planning session 枚举）、`monitor_log.ts`（`notification.created/updated` kind）、`src/routes/`。
- `frontend/ralph-web`：`App.tsx`（`/monitor/inbox`）、`Sidebar`（入口+unread 徽章）、`hooks.ts`、ConnectionProvider 分发器（新 kind）。
- `presets/monitor/*.yml`（Plan2）：schema 增补可选 `waiting.topics`（向后兼容加法）。

**计划新增**

- `backend/ralph-monitor/migrations/0006_notifications.sql`；`src/{notify/rules.ts,notify/outbox.ts,notify/worker.ts,notify/channels/webhook.ts,notify/ssrf.ts,routes/notifications.ts}`。
- `frontend/ralph-web/src/features/monitor/{inbox/InboxPage.tsx,inbox/NotificationSettings.tsx,browserNotify.ts}`；`e2e/monitor-notifications.spec.ts`。

**明确不修改**：runtime `notifications:` 配置与 dispatch 代码（E3）；ralph-api；`planning.*` 行为；`presets/en/*`；Plan1–3 API 形状（加法）。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选 | 最终选择 | 支持证据 | 排除原因 | 置信度 |
| ---- | ---- | ---- | ---- | ---- | ---- | ---- |
| D1 | 通知产生点 | A. 前端推导；B. collector ingest 同事务内规则匹配 | B：终态转移/interaction 出现在 ingest 事务内即写 outbox | KTD1（同事务）、E1（事务已存在）、E2（终态权威=manifest） | A 重复采集/刷新即重发 | 0.92 |
| D2 | 幂等键 | A. 时间窗口去重；B. `sha256(run_id\|event_seq\|rule_id\|rule_version)` 唯一约束 | B：DB UNIQUE + upsert ignore | KTD1、R3 | A 边界漏/重 | 0.92 |
| D3 | 等待输入证据源 | A. descriptor `waiting.topics` 事件；B. planning session 文件；C. A+B | C：B 为主（文件证据稳定），A 为补充（descriptor 声明才启用） | E5（session 文件结构化）、E6（topic 先例） | 仅 A 要求 preset 作者声明→覆盖率低；仅 B 漏非 planning 等待 | 0.85 |
| D4 | 投递语义 | A. at-most-once；B. at-least-once+delivery_id | B：退避 30s/2m/10m/1h（上限 4 次）→`dead`+手动重试；`delivery_id`(ULID) 进 payload+`X-Ralph-Delivery-Id` 头；文档明示不承诺 exactly-once | KTD2 | A 不满足可靠投递 | 0.9 |
| D5 | webhook 安全 | A. 任意 URL+follow redirect；B. https 默认+DNS 解析校验+私网拒绝+allowlist+禁 redirect | B：见 §1 安全段；`resolve→validate→connect(固定 IP? 直接 fetch+校验后短窗口)`——v1 采用解析校验+禁 redirect+5s 超时；TOCTOU 已知残留记录 | KTD4、E3（runtime webhook 无防护是反面教材） | A SSRF 敞口 | 0.87 |
| D6 | 浏览器通知 | A. service worker push；B. 页面在场 `new Notification` | B：仅 `notification.created` 流事件触发；权限请求必须用户手势发起 | E8（无 sw/push 依赖）、A2 | A 需 push service+VAPID 体系超出范围 | 0.9 |
| D7 | runtime webhook 共存策略 | A. 静默并存；B. collector 读取 `<project>/ralph.yml` `notifications.enabled` → run/项目提示"可能重复" | B：只读检测+UI 提示；绝不改写项目配置 | KTD3、E3/E4 | A 违反显式选择要求 | 0.85 |
| D8 | 通知持久字段 | A. 仅 JSON blob；B. 结构化列+JSON detail | B：`notifications(id,idem_key unique,kind,run_gid,pending_interaction_id?,title,body,detail json,state:unread|read|muted|resolved,created_at,resolved_at)` | Inbox 过滤/排序/批量需要列 | A 查询受限 | 0.9 |
| D9 | 渠道矩阵 | A. 单 channels 配置；B. `deliveries(id,notification_id,channel:{in_app,browser,webhook},state:pending|delivered|failed|dead|rejected,attempts,next_attempt_at,delivery_id,error)`；browser 渠道仅登记不落 worker | B：in_app=创建即 delivered；browser=状态登记（实际投递在前端）；webhook=worker | 三渠道独立重试语义（R4） | A 混合语义难表达独立失败 | 0.9 |
| D10 | 测试 webhook 目标 | A. 仅 allowlist；B. 测试投递允许 `127.0.0.1` 测试服务器当 `?test=true` 且来自 loopback 会话 | B：`POST /channels/webhook/test` 用 `test_delivery`，允许 loopback 目标仅 test 通道 | 本地可验证闭环 | A 无法本地验证 webhook | 0.86 |

无 <0.85 决策。

实施协议：同 Plan1（验收/停止契约、nextest 不适用——本计划无 Rust 改动；env scrub 沿用 fixture）。

## 4. BDD 行为规格

```gherkin
Feature: 可靠通知与待处理中心

  Scenario: S1 终态事件产生持久通知
    Given 一次 run 在 ingest 中出现 terminal.reason=completed
    When ingest 事务提交
    Then notifications 表新增一条 kind=run.completed（unread）
    And in_app delivery 立即 delivered
    And SSE 发出 notification.created 事件

  Scenario: S2 失败终态与成功区分
    Given terminal.reason=interrupted（非 completed）
    When ingest 提交
    Then 通知 kind=run.failed，标题含 run_id 与 reason

  Scenario: S3 重复采集与重放不重复通知
    Given 同一终态证据被再次 ingest（重放/重启续读）
    When 规则匹配再次命中
    Then 不产生第二条通知（idem_key 唯一约束）

  Scenario: S4 等待输入进入待处理中心
    Given 项目内出现 .ralph/planning-sessions/<id>.json 且 status=pending
    When ingest 运行
    Then 生成 kind=interaction.pending 通知且列入 pending_interactions
    When session 文件 status 变为 responded 或 run 终态
    Then 该待处理项 resolved 且通知 state=resolved（从待办消退）

  Scenario: S5 点击通知回到运行证据
    Given Inbox 中存在 run.completed 通知
    When 用户点击
    Then 跳转 /monitor/runs/<gid>（含来源 tab 参数）
    And 通知被标记 read

  Scenario: S6 webhook 浏览器关闭仍可投递
    Given 配置了 webhook 渠道且无浏览器在线
    When 一次 run.failed 产生
    Then worker 在窗口内完成 POST 且 deliveries.state=delivered
    And 服务端回执含 X-Ralph-Delivery-Id 与 payload.delivery_id

  Scenario: S7 webhook 失败可见可重试
    Given webhook 目标返回 500 或连接超时
    When worker 尝试
    Then deliveries.attempts 递增并按 30s/2m/10m/1h 退避
    And 4 次后 state=dead 且界面显示错误与"重试"按钮
    When 用户点击重试
    Then 重新入队（attempts 继续累计保留历史）

  Scenario: S8 webhook 4xx 不重试
    Given 目标返回 404/401
    When worker 投递
    Then state=failed（不再自动重试）+ 错误详情可见

  Scenario: S9 SSRF 防护
    Given 用户配置 webhook 指向 http://169.254.169.254/latest 或 http://127.0.0.1:secret（未 allowlist）
    When 保存配置或投递
    Then 被拒绝（配置期 422 / 投递期 rejected）
    And DNS 解析到私网地址的 https 域名同样被拒

  Scenario: S10 测试投递
    Given 用户配置 webhook 指向本地测试接收端（test 通道）
    When 点击"发送测试通知"
    Then 真实 POST 到达且 payload.test=true
    And 返回投递结果与 delivery_id

  Scenario: S11 浏览器通知用户授权
    Given 用户未授权通知权限
    When Inbox 设置中点击"启用浏览器通知"（用户手势）
    Then 浏览器弹出权限请求
    When 授权且 notification.created 到达
    Then 系统通知显示，点击聚焦并跳转到 run 详情

  Scenario: S12 静音不等于删除证据
    Given 用户对一条通知选择静音
    When 刷新 Inbox
    Then 该条不出现在默认列表（过滤器含已静音可找回）
    And 通知行与 deliveries 历史仍保留可查

  Scenario: S13 runtime webhook 重复提示
    Given 项目 ralph.yml 配置了 notifications.endpoints
    When 用户查看该项目 run 或通知设置
    Then 显示"项目还配置了 runtime 终态 webhook，可能产生重复通知"提示
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
| ---- | ---- | ---- | ---- | ---- | ---- |
| S1/S2 | kind/state/delivery/SSE 断言 | `test/integration/notify_outbox.test.ts` | 集成 | — | 否 |
| S3 | 唯一约束；重放幂等 | `notify_outbox`（重放同一 ingest 事务） | 集成 | Idempotency | 否 |
| S4 | pending 生成/resolved 消退 | `notify_outbox`+`pending.test.ts` | 单元+集成 | — | 否 |
| S5 | 深链+read 标记 | `InboxPage.test.tsx` | 组件 | — | `monitor-notifications` |
| S6 | 离线投递成功+delivery_id | `worker.test.ts`（本地 http 接收器） | 集成 | — | 否 |
| S7 | 退避序列/dead/手动重试 | `worker.test.ts`（时钟注入） | 单元+集成 | Fault injection | 否 |
| S8 | 4xx→failed 不重试 | `worker.test.ts` | 集成 | — | 否 |
| S9 | 私网/loopback/重定向/DNS 矩阵 | `ssrf.test.ts`+`webhook_route.test.ts` | 单元+集成 | Security | 否 |
| S10 | test 投递真到达+test 标记 | `webhook_route.test.ts` | 集成 | — | 否 |
| S11 | 权限手势/显示/点击深链 | `browserNotify.test.ts`+`InboxPage` | 组件 | — | `monitor-notifications`（注入 Notification mock） |
| S12 | muted 过滤+证据保留 | `InboxPage`+`notifications_api` | 组件+集成 | — | 否 |
| S13 | ralph.yml 检测+提示展示 | `collector`（fixture）+`NotificationSettings` | 集成+组件 | — | 否 |

## 6. 需求—测试追踪矩阵

| Requirement | Scenario | 验收测试 | 单元测试 | 集成/契约 | E2E | Evidence |
| ---- | ---- | ---- | ---- | ---- | ---- | ---- |
| R1 | S1/S2/S4/S5 | outbox+Inbox | rules/idem | notify 集成 | monitor-notifications | E2/E5/E6 |
| R2 | S6/S7/S8 | worker+webhook | 退避/状态机 | 本地接收器集成 | — | E3/D4 |
| R3 | S3/S4 | outbox 幂等+pending | 唯一约束/resolved | — | — | D2/D3 |
| R4 | S9–S12 | ssrf/route/Inbox/browserNotify | SSRF 矩阵 | webhook route | monitor-notifications | E8/D5/D6 |

Unit 映射：S1–S4/S12（持久层）→U1；S6–S10/S13（渠道+配置）→U2；S5/S11/S12（UI）→U3。

## 7. 严格串行开发单元

```text
Unit 1（outbox + 规则匹配 + pending interactions + notifications API）
  ↓
Unit 2（webhook 渠道 + 安全配置 + delivery worker + ralph.yml 重复提示）
  ↓
Unit 3（Inbox UI + 设置 + 浏览器通知 + e2e）
```

### Unit 1：notification outbox — 规则匹配、幂等、pending interactions、REST API

#### 1. Unit 目标
ingest 事务内规则匹配终态转移与 pending interaction 出现/消退，写 `notifications`+`deliveries`(in_app 即 delivered)+`pending_interactions`；幂等键唯一约束防重；`GET /api/monitor/v1/notifications`（过滤/分页）与 `POST /:id/read|/read-all|/:id/mute`、`GET /pending-interactions` 可用；SSE 发 `notification.created/updated`。

#### 2. 对应需求与 Scenario
R1/R3 数据面；S1–S5/S12 数据面；D1/D2/D3/D8/D9；E1/E2/E5/E6。

#### 3. 外部可观察结果
API：通知列表项 `{id,kind,run_gid,title,state,created_at,link}`；pending `{id,run_gid?,session_id,prompt,status,created_at}`；mute/read 幂等（重复调用 200 同态）。

#### 4. 当前行为基线
无表/无路由/无检测（Red）；ingest 事务可挂接（E1）。

#### 5. 输入与输出
输入：ingest 变更集（run terminal 转移、events waiting topic、planning session 文件状态 diff——collector 本轮新增枚举 `<workspace>/.ralph/planning-sessions/*.json`）。输出：通知行/待处理行/REST。状态变化：三表+monitor_events kind。副作用：无外部 IO（webhook 在 U2）。不变量：同一终态证据永不重复通知；resolved 不回退 unread（除新一轮 pending）。

#### 6. 修改位置
- `migrations/0006_notifications.sql`：`notifications(idem_key unique,…)`、`deliveries(notification_id,channel,state,attempts,next_attempt_at,delivery_id,error,created_at,updated_at)`、`pending_interactions(id,session_id unique?,run_gid?,status,resolved_at)`、`notification_rules(id,version,kind,matcher json,enabled)`（种子：3 规则 v1）。
- `src/notify/rules.ts`：纯函数 `match(delta)->intent[]`（terminal→completed/failed；session pending→interaction.pending；session resolved→resolve intent）。
- `src/notify/outbox.ts`：intent→行（idem 键计算/upsert ignore/delivery in_app delivered/SSE kind 追加）。
- `src/collector/collector.ts`：delta 产出+session 枚举（`<root>/.ralph/planning-sessions/*.json` 读取+diff，文件缺失/坏 JSON→跳过+计数）。
- `src/routes/notifications.ts`：list/read/read-all/mute/pending。
- 测试：`test/unit/{rules,outbox,idem}.test.ts`、`test/integration/{notify_outbox,notifications_api,pending}.test.ts`、session fixture 文件集。

#### 7. 可依赖能力
Plan1–3 ingest/事务/流/REST；better-sqlite3；zod。

#### 8. 禁止依赖的未来能力
webhook 渠道（U2 才实现 worker）；前端（U3）；descriptor `waiting.topics` 匹配（schema 预留，本 Unit 仅 session 证据——**修正**：规则 matcher 结构留 `topic` 字段，U1 同时实现 descriptor waiting.topics 匹配若 Plan2 已交付该字段；否则记录为仅 session 来源，决策点已含）。

#### 9. 验收测试
S1–S4/S12 数据面断言（§5）；notifications API 全参数；idem 冲突用例；session fixture 生命周期（pending→responded→resolved）。

#### 10. Acceptance Red
表/路由不存在→注入失败/404。

#### 11. 单元测试拆分
rules：completed/failed/未知 reason→不通知（`completed` 唯一成功词？——决策：reason∈{completed}→completed；其余终态→failed（interrupted/cancelled/payload_violation/recovery_exhausted 等 E2 词表全集映射）；`unknown` 态不通知）；session status 词表映射（pending|awaiting_response→pending；responded|completed|closed→resolved；进入 Unit 时 fixture 实测实际词表并固定）；idem 键稳定/跨 run 隔离；resolved 不回退。
不允许 mock：sqlite 真约束。

#### 12. Red → Green → Refactor
```text
rules Red→Green → outbox+idem Red→Green → session 枚举+pending Red→Green → API Red→Green → Refactor
```

#### 13. 最小实现范围
三表+规则+API+SSE kind。不实现：规则 UI 编辑（DB 可改无 UI）；多渠道 worker；通知清理策略（v1 保留全部，记录）。

#### 14. 集成验证
fake ralph fixture：终态 run+session 文件序列；双 ingest 重放幂等。

#### 15. 风险驱动测试
Idempotency：同事务重放/跨重启重放；State-machine：pending→resolved→（同 session 再 pending?）→新轮次新行（session 更新复用同 idem?——决策：idem 含 `occurrence`（session.updated_at 轮次）允许同 session 多轮 pending 各成通知，resolved 关当轮）；Fault：session 文件删除中→resolved（保守消退，记 evidence）。

#### 16. 回归范围
Plan1–3 monitor 测试；SSE kind 新增不破坏 U4 分发（未知 kind 默认忽略——需在 Plan3 分发器容忍未知 kind，若无则本 Unit 补容错）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `migrations/0006_notifications.sql` | 新增 | 持久 | D8/D9 |
| `src/notify/{rules,outbox}.ts`、`src/routes/notifications.ts` | 新增 | 核心 | D1/D2 |
| `src/collector/collector.ts` | 修改 | session 枚举+delta | E5 |
| `src/stream/monitor_log.ts` | 修改 | 新 kind | E1 |
| `presets/monitor/*.yml` + descriptor schema | 修改 | 可选 waiting.topics | D3 |

#### 18. 完成标准
S1–S4/S12 数据面绿；门禁绿；idem 约束在 DDL 层存在（UNIQUE）。

#### 19. 停止条件
planning session status 词表与假设不符（如枚举外新值）→按实测词表修正映射表并记录；session 文件无 run 关联→notification.run_gid 可空，深链退化到项目页（记录）。

#### 20. 风险与注意事项
- session↔run 关联缺字段：v1 关联=同 workspace+时间窗近似→`run_gid` 允许 null，待处理项独立列出（不强绑）。
- 通知洪峰（批量终态）：worker/API 分页+SSE 事件批次合并（notification.created 可聚合 count 版——记录）。

---

### Unit 2：webhook 渠道 + 安全配置 + delivery worker + 重复源提示

#### 1. Unit 目标
`webhook` 渠道可配置（URL/secret/enable）并经 worker 至少一次投递；SSRF 防护与 secret 掩码落地；`POST /notifications/channels/webhook/test` 真测试投递；失败退避/dead/手动重试 API；collector 读取项目 `ralph.yml` 提取 `notifications.enabled` 供重复提示。

#### 2. 对应需求与 Scenario
R2/R4 渠道面；S6–S10/S13；D4/D5/D7/D9/D10；E3/E4。

#### 3. 外部可观察结果
`PUT /api/monitor/v1/notifications/channels/webhook {url,secret?,enabled}`；`GET`→`{url,secret:"•••"+last4,enabled,last_error?}`；`POST /:deliveryId/retry`；deliveries 列表可查（`GET /notifications/:id/deliveries`）；重复提示字段 `project.runtime_webhook_hint` 进 `/projects`。

#### 4. 当前行为基线
仅 in_app delivery（Red）；无配置表（migration 加 `channel_config(channel pk,config json,updated_at)`）。

#### 5. 输入与输出
输入：webhook 配置；deliveries due 行。输出：HTTP POST `{delivery_id,kind,run_gid,title,at,test?,link}`+头 `X-Ralph-Delivery-Id`+`X-Ralph-Signature: sha256=<hmac(secret,body)>`（secret 配置时）。状态变化：deliveries 状态机 pending→delivered|failed|dead|rejected；错误可见。副作用：外部 POST。不变量：永不 follow redirect；secret 不出 API/日志；4xx 不重试。

#### 6. 修改位置
- `migrations/0007_channels.sql`：`channel_config`+`deliveries` 已有（U1）索引 `next_attempt_at`。
- `src/notify/ssrf.ts`：`validateWebhookUrl(url,{allowlist})`：parse→协议→dns.lookup(all)→IP 分类（node:net isIPv4/isIPv6+私网表：10/8,172.16/12,192.168/16,127/8,169.254/16,::1,fc00::/7,fe80::/10,0.0.0.0,multicast）→allowlist 例外（精确 host 或 CIDR 字串，配置 `RALPH_MONITOR_WEBHOOK_ALLOWLIST` env 起步，v1 不做 UI allowlist——记录）。
- `src/notify/channels/webhook.ts`：`deliver(delivery,config)`：undici fetch（node 内置 `fetch` redirect:'manual',signal 5s,body JSON）；3xx→failed(`redirect_rejected`)；hmac(node:crypto)；结果→状态。
- `src/notify/worker.ts`：每 5s `SELECT … WHERE state=pending AND next_attempt_at<=now LIMIT 20` → deliver → 更新状态/退避（30s,2m,10m,1h）；时钟可注入（测试）；进程崩溃→下次启动扫 pending（stale next_attempt_at 即 due）。
- `src/routes/notifications.ts`：channels get/put/test/deliveries list/retry。
- `src/collector/collector.ts`：`ralph.yml` 只读检测 `notifications:` enabled→`projects.runtime_webhook_hint`（YAML 轻解析：用现有 yaml 依赖？monitor 无 yaml 依赖——决策：加 `yaml` 依赖（E14 先例 legacy 有 yaml 2.8.2）或子串检测？——选 `yaml` 解析准确；若文件坏→hint=unknown）。
- 测试：`test/unit/{ssrf,worker,webhook_sign}.test.ts`、`test/integration/{webhook_route,worker_e2e}.test.ts` + 本地 http 接收器 helper（node:http 测试内）。

#### 7. 可依赖能力
U1 deliveries/outbox；node fetch/crypto/dns/net；fastify。

#### 8. 禁止依赖的未来能力
前端设置 UI（U3）；多 webhook endpoint（v1 单 endpoint——schema 按单对象设计）。

#### 9. 验收测试
S6–S10/S13；secret 掩码；retry 重置；allowlist env 生效；hmac 正确性（给定期望签名 fixture）。

#### 10. Acceptance Red
worker/路由不存在；投递无 effect。

#### 11. 单元测试拆分
ssrf 全矩阵（私网各段/loopback/ipv6/解析失败/allowlist hit/miss/非 http(s) 协议/用户名密码 URL→拒）；退避序列；状态机非法迁移防；hmac；配置校验（url 必填/secret 可空/enabled 默认 false）。
不允许 mock 掉真 fetch（用本地 http server）。

#### 12. Red → Green → Refactor
```text
ssrf Red→Green → webhook.deliver Red→Green → worker(时钟注入) Red→Green → routes Red→Green → ralph.yml hint Red→Green
```

#### 13. 最小实现范围
单 webhook 渠道+worker+ssrf+test+retry+hint。不实现：多 endpoint、payload 模板编辑（固定 schema）、TLS pinning、proxy 支持（env HTTP_PROXY 不 honored——记录残留）。

#### 14. 集成验证
本地接收器收到 POST 校验 header/body/签名；kill worker 进程→重启续投（stale due）。

#### 15. 风险驱动测试
Fault：连接超时（接收端 hang）、DNS 慢、半开连接、响应 3xx、大响应体（不读 body>64KB 截断——决策：丢弃响应体只取 status）；Security：配置回显掩码断言、日志不含 secret（grep 测试日志）；Idempotency：同 delivery_id 重试接收端去重演示（记录示例）。

#### 16. 回归范围
U1 全部；rate 限制：worker 串行防自身洪峰（已含 LIMIT）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `migrations/0007_channels.sql` | 新增 | 配置 | D9 |
| `src/notify/{worker,channels/webhook,ssrf}.ts` | 新增 | 渠道 | D4/D5 |
| `src/routes/notifications.ts` | 修改 | 渠道路由 | — |
| `src/collector/collector.ts` + `src/routes/projects.ts` | 修改 | ralph.yml hint | D7 |
| `package.json`（monitor） | 修改 | +yaml 依赖 | E14 先例 |

#### 18. 完成标准
S6–S10/S13 绿；门禁绿；签名/掩码/SSRF 断言齐。

#### 19. 停止条件
node fetch 无法禁 redirect→换 undici request 手动；dns.lookup 在某环境不解析 mDNS/hosts→校验失败默认拒绝（fail-closed，记错）。

#### 20. 风险与注意事项
- TOCTOU（校验后 DNS 变化）：v1 接受该窗口，记录残留（彻底需 connect 固定 IP+Host 头，后续版）。
- 退避时钟与 wall-clock：用 `Date.now`+注入时钟测试。
- webhook 目标内网合法需求→env allowlist 是唯一放行途径（文档注明）。

---

### Unit 3：Inbox UI + 通知设置 + 浏览器通知 + e2e

#### 1. Unit 目标
`/monitor/inbox` 页面：通知列表（kind 徽章/时间/项目-run 深链）、未读计数徽章（Sidebar）、过滤器（全部/未读/已静音/待处理）、操作（标记已读/全部已读/静音）、待处理交互分区（pending 项+消退自动刷新）；`NotificationSettings`（抽屉或 `/monitor/settings` 节）：webhook 配置（掩码回显/保存/测试投递结果展示/失败列表+重试）、浏览器通知开关（权限状态机）、runtime webhook 重复提示；e2e `monitor-notifications.spec.ts`。

#### 2. 对应需求与 Scenario
R1/R2/R4 用户面 + R3 消退呈现；S5/S11/S12/S13 UI；D6/D7/D10；E7/E8。

#### 3. 外部可观察结果
见 Unit 目标；SSE `notification.created`→未读计数即时+1 且列表顶插（浏览器在场）；点击通知→深链+自动 read。

#### 4. 当前行为基线
无 inbox 路由/组件（Red）；SSE/连接层已就绪（Plan3-U4）。

#### 5. 输入与输出
输入：U1/U2 API+SSE。输出：页面/设置/系统通知。副作用：read/mute/write 配置（CSRF 保护 POST）。不变量：浏览器通知仅权限 granted+开关 on；权限请求必须按钮手势触发。

#### 6. 修改位置
- `features/monitor/inbox/{InboxPage.tsx,NotificationRow.tsx,PendingSection.tsx,NotificationSettings.tsx}`（新增）；`features/monitor/browserNotify.ts`（权限+show+click→navigate）。
- `App.tsx`/`Sidebar.tsx`：路由+徽章（useNotifications unread count hook 接 SSE 增量）。
- `hooks.ts`：useNotifications/usePending/useChannelConfig/useDeliveries；ConnectionProvider 分发 `notification.created`→queryClient 更新+（可选）browserNotify。
- `e2e/monitor-notifications.spec.ts`：route-intercept+`page.addInitScript` 注入 Notification stub（权限 granted 假实现）。
- 测试：组件 `.test.tsx`（列表/过滤/操作/待处理/设置/掩码/权限状态机）。

#### 7. 可依赖能力
Plan1-U4 ui/hooks；Plan3-U4 连接层；`new Notification`（浏览器原生，stub 测试）。

#### 8. 禁止依赖的未来能力
操作按钮（Plan5）；通知渠道扩展 UI。

#### 9. 验收测试
S5/S11/S12/S13 UI 断言；设置保存/测试投递展示；deliveries 失败重试按钮流。

#### 10. Acceptance Red
路由/组件缺失。

#### 11. 单元测试拆分
未读计数派生（列表+SSE 增量去重）；过滤器状态机；mute 过滤切换；浏览器通知分支矩阵（denied→禁用文案；granted+off/on；default→启用按钮引导手势）；设置表单校验（url 必填 https 提示）。

#### 12. Red → Green → Refactor
```text
Inbox 列表+过滤 Red→Green → pending 分区 Red→Green → settings（webhook+browser）Red→Green → sidebar 徽章+SSE 联动 Red→Green → e2e Red→Green
```

#### 13. 最小实现范围
页面+设置+徽章+浏览器通知+e2e。不实现：通知声音、批量静音、键盘全操作（基础可达性仍要求：行可聚焦 Enter 打开）。

#### 14. 集成验证
vitest；playwright spec（注入 Notification stub+route 拦截）；手动真联调 webhook 至本地接收器。

#### 15. 风险驱动测试
a11y（列表语义+操作可达）；SSE 通知与轮询去重（同 id 不重复计数）；权限被拒后引导文案。

#### 16. 回归范围
前端全量 vitest+e2e；Sidebar 既有项。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `features/monitor/inbox/*`、`browserNotify.ts` | 新增 | UI | E7/E8 |
| `App.tsx`/`Sidebar.tsx`/`hooks.ts`/ConnectionProvider | 修改 | 接入 | — |
| `e2e/monitor-notifications.spec.ts` | 新增 | ATDD | E7 |

#### 18. 完成标准
S5/S11/S12/S13 UI 绿；e2e 绿；门禁。

#### 19. 停止条件
jsdom/测试环境 Notification 全局缺失→stub（已定）；Sidebar 徽章 SSE 与轮询双源不一致→以 SSE 为准+周期 reconcile。

#### 20. 风险与注意事项
- 浏览器通知点击在无聚焦权限下不聚焦窗口→catch 后仅 navigate（记残留）。
- 大量通知渲染：分页 50 已定。

## 8. Unit 串行依赖图

```text
Unit 1 → Unit 2 → Unit 3
```

- U2 依赖 U1 deliveries/outbox 与通知行；U3 依赖 U1/U2 全部 API。
- 顺序不可交换：无 outbox 无投递对象；无 API 无 UI。

## 9. 执行命令清单

```bash
# monitor（全部 Unit；本计划无 Rust 改动）
C1  npm test -w @ralph-monitor/server
C2  npm run typecheck -w @ralph-monitor/server && npm run build -w @ralph-monitor/server

# frontend
C3  npm test -w @ralph-web/dashboard
C4  npm run build -w @ralph-web/dashboard
C5  cd frontend/ralph-web && npx playwright test e2e/monitor-notifications.spec.ts
C6  cd frontend/ralph-web && npx playwright test

# 手工验证（记录）
C7  node -e "启动本地 webhook 接收器" &   # 测试接收器
C8  curl -X PUT http://127.0.0.1:3200/api/monitor/v1/notifications/channels/webhook -d '{"url":"http://127.0.0.1:9/test","secret":"s3"}' -H 'content-type: application/json' -H "x-monitor-csrf: <t>" -b 'monitor_session=<s>'

B1  npm test && npm run build
```

## 10. 最终质量门禁

- [ ] S1–S13 全绿；R1–R4 追踪无空洞；
- [ ] 幂等（重放/重启/重复 ingest）+并发（两 pending 同 session）+fault injection（超时/4xx/5xx/3xx/DNS 失败）+SSRF 安全矩阵全绿；
- [ ] deliveries 状态机全迁移有测试；delivery_id 出现于 payload+header；
- [ ] secret 掩码断言；日志脱敏检查；
- [ ] runtime webhook 只读提示存在且不改写项目配置；
- [ ] 浏览器通知权限仅用户手势触发；
- [ ] monitor typecheck/test/build+前端 vitest/build/playwright 全绿；无 skip/.only/弱化断言；
- [ ] 置信度复核 ≥0.85；变更不超范围。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
| ---- | ---- | ---- |
| 实施计划而非 Roadmap | 是 | — |
| Executor 仍需关键设计决策 | 否 | D1–D10；两个"进入 Unit 时确认"已给实测动作 |
| 文件/接口有代码库证据 | 是 | — |
| 关键决策置信度 ≥0.85 | 是 | 最低 D3/D7=0.85 |
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

**Series**: `feat/monitor-product` — 依赖 Plan 1–3；Plan 5 的操作回执（操作完成/失败）将作为新的通知 kind 接入本计划 outbox。
