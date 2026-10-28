---
title: feat(monitor): Web 操作闭环与生产交付
type: feat
date: 2026-10-28
baseline: 4c8718b77327d90087d6665e51654b58de90b805
execution: code
status: READY
depends_on: 2026-10-28-1949-feat-monitor-notifications-inbox-plan
---

# Web 操作闭环与生产交付

## 0. 计划状态

`READY` — 所有实施关键决策置信度均 ≥ 0.85，无 BLOCKED 项。

- **代码库基线**：`4c8718b77327d90087d6665e51654b58de90b805`。
- **前置依赖**：Plan 1–4（monitor 服务/采集/projection/SSE/健康/通知 outbox/前端壳与详情页）。本计划关闭环：受控写操作 + 生产交付。
- **调查范围**：`crates/ralph-cli/src/loops.rs`（stop/retry/merge/merge_button_state 全部分支与 JSON 输出点）、`crates/ralph-cli/src/commands/{run.rs,emit/mod.rs,preset.rs}`、`crates/ralph-cli/src/web.rs`（spawn 结构/env/ready 检测/关闭序列）、`crates/ralph-cli/src/commands/inspect.rs`、`crates/ralph-api/src/{collection_domain.rs,idempotency.rs,auth.rs}`、`frontend/ralph-web`（vite.config/构建产物/ui 组件）、root `package.json`、`.nvmrc`、`scripts/check-cli-doc-drift.sh`。
- **已执行验证**：`loops` 子命令与信号语义（SIGTERM/`--force` SIGKILL）、`resolve_loop` 匹配链、merge-button-state JSON、`ralph emit --file` 参数、`preset check --format`、`collection.run` spawn 形态、`web.rs` 全部 flag 与子进程管理、`ApiConfig` env。
- **尚未执行**：`inspect run.actions`（U1 新增）、`ralph run --client-request-id`（U1 新增）、`loops <op> --format json`（U1 新增）、`ralph web --monitor`（U4 新增）、monitor 静态托管（U4）。
- **阻塞项**：无。

本计划新增术语：

- **capability（能力声明）**：`inspect run` 输出的 `actions[]`——runtime 基于当前 run 证据给出的可执行操作集，含 `available`/`reason`/`revision`/`input_schema`。
- **operation（操作）**：monitor 侧持久记录的一次受控写（intent→dispatch→confirm），含 `idempotency_key`/`client_request_id`/`receipt`/审计字段。
- **intent**：dispatch 前落库的操作意图；spawn 崩溃窗口对账的依据。
- **revision**：操作目标证据版本（`sha256(run_id|state|iteration|merge_state|pending)`）；mutation 必须携带期望值，过期→`revision_conflict`。
- **receipt**：runtime 返回的结构化回执（`loops <op> --format json` 或 manifest 回读证据）。

## 1. 功能目标

| R-ID | 需求（可观察行为） | 行为差异（当前 → 目标） |
| ---- | ---- | ---- |
| R1 | 在 Web 中选择已注册项目、builtin/custom preset、可选 plan/prompt，预检通过后启动运行 | 现状：仅终端/ralph-api `collection.run` 可启动（E5）→ 目标：Launch Dialog + `POST /operations{kind:"launch"}`，经 `ralph run -a` 真实启动并被采集确认 |
| R2 | 操作能力由 runtime 能力声明决定；输入、停止、重试、合并有结果确认；缺恢复协议/不支持的显示只读原因；**不得伪造**暂停/继续 | 现状：UI 无任何操作（Plan1–4 只读）→ 目标：`actions[]` 驱动菜单；`stop`→SIGTERM(`loops stop`)、`retry`→`loops retry`、`merge`→`loops merge`、`emit`→`ralph emit` 受限 topic 集；不可用项 disabled+reason |
| R3 | 重复提交/刷新/网络超时不产生重复运行或重复业务操作 | → `operations.idempotency_key` 唯一约束+`client_request_id` 落 manifest 对账；同键重放返回同一 operation |
| R4 | 单服务交付静态资源、安装启动说明、完整跨项目 E2E、视觉/键盘/reduced-motion 验收 | → monitor 托管 `dist` SPA（同源）+ `ralph web --monitor` + `docs/guide/monitor.md` + `monitor-journey.spec.ts` |
| R5 | 操作 API 可被自动化调用，校验与审计与 UI 一致 | → 同一 REST（CSRF/会话对 curl 同样要求）+ `operations` 表即审计；`GET /operations` |

范围：monitor 自身闭环与单机交付。非目标：多用户权限/RBAC、任意 CLI 透传（只暴露声明 action）、队列编排、跨机操作。

输入：操作请求 `{kind,project_id,run_gid?,params,idempotency_key,expected_revision?}`；capability。输出：`operations` JSON、SSE `operation.updated`、审计表。

错误语义：`capability_unavailable`（含 reason）、`revision_conflict`、`duplicate_operation`（返回既有 op）、`dispatch_failed`（spawn/exit 码/stderr 摘要）、`confirm_timeout`（回读窗口内未确认，不自动重试，显示 pending 状态可人工确认）。

兼容要求：`loops stop/retry/merge` human 输出不变（`--format json` 为加法 flag）；`ralph run` 新 flag 加法；`ralph web` 默认行为不变（`--monitor` 加法，不迁移默认入口——见 D10）；不改 ralph-api。

性能：操作串行/单目标（同一 run_gid 同时只有一个非终态 operation——DB 部分唯一约束）；回读确认窗口默认 15 s（轮询 `inspect run` 或直接复用常规采集周期 + 立即触发一次采集）。

安全：mutations 全走会话+CSRF（Plan1 D9 已落地）；参数数组 spawn 无 shell；plan/prompt 文件路径限项目根内（realpath+allowlist 同 Plan2-D7 组件复用）；emit topic 白名单由 runtime 声明（非自由文本）；操作审计含 actor（session id）+origin 记录。

已确认假设：A1 `ralph loops stop|retry|merge` 以 loop_id 定位且 primary loop 也可经 manifest run_id→registry/merge-queue 匹配链命中（`resolve_loop` partial/fallback，E4）；primary 的 stop 走 loop.lock pid（E4）。A2 `ralph emit --file` 对目标 run 的 events_file 写事件是 runtime 支持的注入面（E7）。A3 builtin preset 名经 `ralph preset builtin list --format json` 可枚举（E8）。

待验证假设：无。

## 2. 代码库现状与证据

### 2.1 当前实现入口与调用链

```text
# 既有写操作面（runtime CLI）
ralph loops stop <id> [--force]     # loops.rs:823 stop_loop → SIGTERM/SIGKILL → E4
ralph loops retry <id> [steering]   # loops.rs retry → 重新拉起 loop → E4
ralph loops merge <id> [--force]    # loops.rs merge → 合并 worktree → E4
ralph loops merge-button-state      # JSON {"state":"active|blocked|…"} → E4
ralph emit <topic> --file <events>  # commands/emit → 事件注入 → E7
ralph run -a -H <preset> -p <p>     # commands/run → 启动（-a autonomous 无 TUI）→ E6
ralph preset builtin list --format  # commands/preset → builtin 枚举 → E8
ralph preset check --format json    # preset.rs:264 → 契约校验 → E8
ralph web [--legacy-node-api]       # web.rs → spawn api+前端，ready 检测，Ctrl+C 顺序关闭 → E10
```

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
| ---- | ---- | ---- | ---- | ---- |
| E1 | `backend/ralph-monitor`（Plan1–4 产物） | projects/runs API、collector、SSE、session/CSRF、allowlist 工具、notification outbox | 操作复用同套基础设施；完成/失败可接通知 kind | 高 |
| E2 | `crates/ralph-core/src/run_manifest.rs`（Plan1）+ `inspect run` | manifest/快照为 run 权威观测；`preset_descriptor`/`scheduler`/`activity_file` 字段模式=加法兼容 | `actions[]` 字段沿用加法模式 | 高 |
| E3 | `crates/ralph-api/src/idempotency.rs` | 内存幂等先例（TTL 3600）；`meta.idempotencyKey` | monitor 用**持久**表（跨重启）而非内存 | 高 |
| E4 | `crates/ralph-cli/src/loops.rs` | `stop_loop`（SIGTERM/`--force` SIGKILL、`stop_requested` 文件证据）、`retry`（含 steering 参数）、`merge`（merge-button-state JSON `{"state":"active"}` 先例）、`resolve_loop`（exact/partial/merge-queue/worktree 匹配） | 三操作均有 CLI 执行面；回执需新增 `--format json`（先例=merge-button-state 已 JSON） | 高 |
| E5 | `crates/ralph-api/src/collection_domain.rs` | `collection.run`：写 `.ralph/collections/<id>-run.yml`→spawn `ralph run -H <yml> -a -p <prompt>`→5s 启动观察→返回 pid | monitor launch 复用同 spawn 形态但走持久 intent+manifest 对账（E5 无 intent 持久化） | 高 |
| E6 | `crates/ralph-cli/src/commands/run.rs`（RunArgs） | `-p/--prompt`、`-P/--prompt-file`、`-H/--hats`、`-b/--backend`、`-a/--autonomous`、`--max-iterations`、`--completion-promise`、`--dry-run` | launch 参数面限定在此集合；不新增启动模式 | 高 |
| E7 | `crates/ralph-cli/src/commands/emit/mod.rs` | `ralph emit <topic> [payload] --file --json --hat --triggered --policy-check/--unsafe-no-policy-check` | emit action=受限 topic 白名单写入目标 events_file（由 capability 声明 allowlist，非自由输入） | 高 |
| E8 | `crates/ralph-cli/src/commands/preset.rs` | `preset builtin list --format {human,json}`、`preset check --format {human,json} [--strict]` | Launch 预检：builtin 名经 list 校验；custom 经 check | 高 |
| E9 | `crates/ralph-cli/src/web.rs` | `ralph web` spawn `cargo run -p ralph-api`+`npm run dev`；flag 体系、ready pattern、端口检查、SIGINT 顺序关闭 | `--monitor` 加法 flag：spawn monitor（env RALPH_MONITOR_*）+（静态模式不再启 vite） | 高 |
| E10 | `frontend/ralph-web`（Plan1–4）+`vite.config.ts` | SPA build=`dist/`；`/api/monitor` dev proxy 已配；ui 组件/hooks 齐备 | monitor 托管 dist 即可同源生产形态；dev 仍走 vite proxy | 高 |
| E11 | `backend/ralph-web-server/package.json` | `@fastify/*` 依赖生态；fastify 5 | `@fastify/static` 官方插件匹配 | 高 |
| E12 | `crates/ralph-cli/tests/common/mod.rs` | `ralph_bin`+env scrub | U1/U4 Rust 测试 | 高 |
| E13 | `docs/guide/` | 无 `monitor.md`（确认缺失） | U4 新增 | 高 |
| E14 | `root package.json` | scripts：`dev`/`dev:api`/`dev:web`/`dev:legacy-server`/`start`/`build`/`build:server`/`build:web`/`test`/`lint` | 加法 `dev:monitor`/`build:monitor`/`start:monitor`，不改默认 | 高 |
| E15 | `crates/ralph-api/src/auth.rs` + `config.rs` | `trusted_local`/`token` 模式、loopback 强制 | monitor 保持 loopback+会话 CSRF（非 loopback 部署=显式 opt-in+文档 TLS/反代要求） | 高 |
| E16 | `.cursor/rules/observability.mdc` + `docs/achieved/plan/…` | runtime 诊断/计划格式 | 文档与结构遵循 | 中 |

### 2.3 受影响范围

**修改的既有文件**

- `crates/ralph-cli/src/commands/run.rs`（`--client-request-id` flag+manifest 写入）、`commands/inspect/run.rs`（`actions[]` 输出）、`loops.rs`（`--format json` 加到 stop/retry/merge）、`web.rs`（`--monitor` flag）。
- `backend/ralph-monitor`：collector（capability 缓存+run confirm 回读）、routes、migrations。
- `frontend/ralph-web`：`features/monitor`（LaunchDialog/ActionsMenu/OperationsSection）、`App.tsx`、`hooks.ts`。
- `package.json`（root）：scripts 加法。

**计划新增**

- `backend/ralph-monitor/migrations/0008_operations.sql`；`src/{ops/intents.ts,ops/dispatch.ts,ops/confirm.ts,ops/audit.ts,routes/operations.ts,static/spa.ts}`；`src/presets/catalog.ts`（builtin/custom 枚举采集）。
- `frontend/ralph-web/src/features/monitor/{launch/LaunchDialog.tsx,actions/ActionsMenu.tsx,actions/ConfirmDialog.tsx,ops/OperationsSection.tsx}`；`e2e/monitor-journey.spec.ts`。
- `docs/guide/monitor.md`。
- `crates/ralph-cli/tests/{integration_run_client_id.rs,integration_loops_json.rs,integration_inspect_run.rs(增补),integration_web_monitor.rs}`。

**明确不修改**：`ralph-api`；`collection.run` 行为（并行存在，monitor launch 不复用）；`loops` human 输出；默认 `ralph web`；`presets/en/*`；adapter。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选 | 最终选择 | 支持证据 | 排除原因 | 置信度 |
| ---- | ---- | ---- | ---- | ---- | ---- | ---- |
| D1 | 能力声明位置 | A. monitor 推导；B. `inspect run.actions[]` runtime 声明 | B：runtime 读 manifest/registry/merge_queue/pending 计算 `{id,available,reason,revision,input_schema?}` | KTD1、E4（runtime 拥有真实状态机）、E2 | A 前端/TS 复刻状态机必然漂移（如 merge-button-state 规则） | 0.9 |
| D2 | action 集（v1） | A. 全 CLI 面；B. `stop/retry/merge/emit(allowlist)`+`launch` | B：四 mutation + launch；`respond`=emit topic 子集（runtime 声明 topics 如 `loop.cancel`/`task.resume`——进入 U1 时用 `rg` 核对可用输入 topic 全集并固定白名单来源） | E4/E7、R2"受支持的"限定 | A 暴露面失控 | 0.87 |
| D3 | mutation 回执 | A. 仅 exit code；B. `--format json` typed receipt | B：`loops <op> --format json` 加法（先例 merge-button-state E4）；launch 回执=manifest+pid | E4/E5 | A 无法区分"已受理"与"失败原因" | 0.88 |
| D4 | launch 幂等对账 | A. TS 表查重；B. `client_request_id`→manifest 落盘→回读匹配 | B：`ralph run --client-request-id <ulid>`（U1 新增，写入 manifest `client_request_id`）；intent 先落库→spawn→下一轮采集按 client_request_id 归属 run | KTD2（spawn 崩溃窗口不能仅 TS 表去重）、E5 | A 崩溃窗口漏配 | 0.88 |
| D5 | 目标 revision | A. 时间戳；B. 证据哈希 `sha256(run_id|state|iteration|merge_state|pending)` | B：capability 返回，mutation 校验 | KTD2 | A 时钟语义弱 | 0.9 |
| D6 | 操作状态机 | A. recorded→dispatched→confirmed|failed；B. 同 A+`confirm_timeout` 终态可人工确认 | B：状态持久在 operations 表；SSE `operation.updated` | R2 结果确认、R3 | 纯内存则重启丢失 | 0.9 |
| D7 | 审计 | A. 独立 audit 表；B. operations 表兼任（actor/session/origin/receipt 列） | B：operations 即审计源；`GET /operations` 可查 | R5、KTD3 | A 双写复杂度 | 0.9 |
| D8 | 静态托管 | A. 独立静态服务；B. monitor `@fastify/static` 托管 dist+SPA fallback | B：env `RALPH_MONITOR_STATIC_DIR`（默认探测 `frontend/ralph-web/dist`） | E10/E11 | A 两端口违背单服务交付 | 0.9 |
| D9 | `ralph web` 接入 | A. 默认改 monitor；B. `--monitor` 加法 flag | B：spawn `npm run start -w @ralph-monitor/server`（env `RALPH_MONITOR_PORT/STATIC_DIR`），不再启 vite/ralph-api；默认行为不变 | E9（flag 体系）、E14（不破坏既有 dev 流） | A 破坏兼容（原 KTD4"迁移默认"过度，证据=现有脚本/文档都假设 vite dev） | 0.88 |
| D10 | npm 入口策略 | A. `npm run dev` 默认改 monitor；B. 加法 `dev:monitor`/`start:monitor`/`build:monitor`，默认不动 | B：同 D9 理由 | E14 | A 同上 | 0.88 |
| D11 | 输入响应范围 | A. 任意 stdin；B. `emit` action 受限 topic 白名单（runtime 声明）；planning session respond 只读跳转 | B：v1 输入=emit topics（runtime 白名单）；session respond 不在 monitor 写（E6 planning.respond 属 ralph-api 会话域，monitor 不伪造） | E6/E7、R2"不得伪造" | A 无支持通道 | 0.85 |
| D12 | CSRF 对自动化 | A. curl 免 CSRF；B. 同源+CSRF 对全部 mutation 生效，自动化用 `X-Monitor-CSRF`+会话 cookie 显式获取 | B：自动化流程=先 `GET /api/monitor/v1/session` 拿 cookie+csrf token | R5（校验一致）、Plan1-D9 | A 双轨校验削弱安全 | 0.88 |

无 <0.85 决策。

实施协议：同 Plan1（验收/停止契约、nextest、env scrub、fake-ralph、drift/audit 脚本）。

## 4. BDD 行为规格

```gherkin
Feature: Web 操作闭环

  Scenario: S1 选择项目+preset 启动并确认
    Given 项目 A 已注册且 builtin preset ce-executor-pipeline 存在
    When 用户在 Launch Dialog 选择 A/preset/填入 prompt 并确认
    Then operations 出现 kind=launch 且 client_request_id 落库
    And 真实 ralph 子进程被 spawn（参数数组）
    And 下一轮采集发现 manifest 含同一 client_request_id 后 operation→confirmed
    And 新 run 出现在总览

  Scenario: S2 预检失败字段级回显
    Given 用户选择不存在的 preset 名或不可读的 plan 文件
    When 点击启动
    Then 不创建 operation
    And 对应字段显示可操作错误（preset 非法/plan 路径不在项目内）

  Scenario: S3 能力声明驱动操作菜单
    Given run 处于 running 且 merge_button_state=blocked
    When 打开操作菜单
    Then stop/retry 可用；merge 禁用并显示 reason
    And emit 可用项仅含 runtime 声明 topic（如 loop.cancel）

  Scenario: S4 停���操作有结果确认
    Given run running
    When 用户执行 stop 并确认（携带 expected_revision）
    Then 真实 SIGTERM 送达目标 pid
    And operation→dispatched→回读 dead/terminated 后→confirmed
    And UI 显示回执摘要与时间

  Scenario: S5 revision 过期拒绝
    Given 用户打开菜单时 revision=R1
    And 期间 run 状态变为 exited_failed（revision=R2）
    When 用户点击 retry
    Then 返回 revision_conflict 且界面提示刷新后重试

  Scenario: S6 重复提交/刷新/超时不重复
    Given 用户对同一 launch 表单双击两次/断网重试
    When 请求携带相同 idempotency_key
    Then 只存在一个 operation 与一个 run
    When 网络超时但服务端已 dispatch
    Then 重试查询返回既有 operation（不二次 spawn）

  Scenario: S7 spawn 崩溃窗口对账
    Given monitor 已落库 intent 后自身重启
    And ralph 子进程实际已启动并写出 manifest(client_request_id=X)
    When monitor 重启后采集
    Then operation 按 client_request_id 匹配到 run→confirmed
    And 不产生"失踪"操作

  Scenario: S8 不支持操作显示只读原因
    Given run 已 exited_success
    When 打开操作菜单
    Then stop/retry 禁用并显示"运行已结束"
    And merge 按 merge_state 显示可用或 blocked 原因
    And 不显示"暂停/继续"项

  Scenario: S9 审计与自动化一致
    Given 经 UI 与 curl（显式会话+CSRF）各执行一次操作
    When 查询 GET /operations
    Then 两条记录字段齐备（kind/target/actor/params/idempotency_key/receipt/confirmed_at）
    And curl 未经会话/CSRF 的 mutation 被拒

  Scenario: S10 单服务静态交付
    Given `npm run build -w @ralph-web/dashboard` 产物存在
    When 以 RALPH_MONITOR_STATIC_DIR=dist 启动 monitor
    Then GET / 返回 SPA、深层路由 /monitor/runs/x 返回 index（SPA fallback）
    And /api/monitor/v1/* 同源可用且跨域头不放开

  Scenario: S11 ralph web --monitor 集成
    Given frontend dist 已构建
    When 执行 `ralph web --monitor --no-open`
    Then 仅 spawn monitor 子进程（不启 vite/ralph-api）
    And 输出 Dashboard URL=http://localhost:3200
    And SIGINT 时子进程有序退出

  Scenario: S12 操作完成接入通知
    Given 一次 stop operation confirmed
    When outbox 匹配规则（operation.confirmed kind）
    Then 产生操作回执通知（含深链 operations 详情）

  Scenario: S13 完整 journey（跨项目+断线+通知跳转）
    Given 两个注册项目各一次运行历史
    When 用户在项目 A launch→观察到 running→执行 stop→confirmed→通知出现→点击深链→断网恢复
    Then 全旅程数据一致且通知/操作/run 三视图互链
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 | E2E |
| ---- | ---- | ---- | ---- | ---- | ---- |
| S1 | intent/spawn/manifest 匹配/confirmed | `test/integration/launch.test.ts`+`integration_run_client_id.rs` | 集成 | — | `monitor-journey` |
| S2 | 字段级错误、无 operation | `operations_api.test.ts`+`LaunchDialog.test.tsx` | 集成+组件 | — | 否 |
| S3 | actions 驱动渲染+禁用原因 | `ActionsMenu.test.tsx`+Rust capability 单测 | 单元+组件 | — | 否 |
| S4 | SIGTERM 到位+confirmed 链路 | `operations_api.test.ts`（假 ralph 收信号脚本） | 集成 | — | `monitor-journey` |
| S5 | revision_conflict | `operations_api.test.ts` | 集成 | Concurrency | 否 |
| S6 | idem 唯一/重放同 op/不二次 spawn | `intents.test.ts` | 单元+集成 | Idempotency | `monitor-journey` |
| S7 | 重启对账 | `launch.test.ts`（杀 monitor 进程） | 集成 | Fault injection | 否 |
| S8 | 终态禁用+reason+无暂停项 | capability 单测+`ActionsMenu` | 单元+组件 | — | 否 |
| S9 | 审计字段+CSRF curl 拒绝 | `operations_api.test.ts` | 集成 | Security | 否 |
| S10 | 静态/SPA fallback/同源 API | `static.test.ts` | 集成 | — | `monitor-journey`（间接） |
| S11 | 仅 monitor 子进程+URL+关闭 | `integration_web_monitor.rs` | Rust 集成 | — | 否 |
| S12 | operation.confirmed→通知 | `notify_outbox` 增补 | 集成 | — | 否 |
| S13 | 全旅程 | `monitor-journey.spec.ts` | E2E | a11y/reduced-motion sweep | 是 |

## 6. 需求—测试追踪矩阵

| Requirement | Scenario | 验收测试 | 单元测试 | 集成/契约 | E2E | Evidence |
| ---- | ---- | ---- | ---- | ---- | ---- | ---- |
| R1 | S1/S2 | launch 集成+Dialog | capability/校验 | ops API | journey | E5/E6/E8 |
| R2 | S3/S4/S5/S8 | ops API+菜单 | 状态机/revision | Rust capability | journey | E4/E7 |
| R3 | S6/S7 | intents/launch | idem 键/对账 | 集成 | journey | D4/D6 |
| R4 | S10/S11/S13 | static/web_monitor/journey | — | 静态/CLI 集成 | journey+视觉 sweep | E9/E10/E13 |
| R5 | S9/S12 | ops API/审计 | — | curl 矩阵 | — | E15/D12 |

Unit 映射：S1/S2/S6/S7→U2（+U1 的 client_request_id）；S3/S4/S5/S8→U1+U2+U3；S9→U2；S10/S11→U4；S12→U2；S13→U5。

## 7. 严格串行开发单元

```text
Unit 1（runtime：actions[] 能力声明 + --client-request-id + loops --format json）
  ↓
Unit 2（monitor：operations 持久机 + launch/stop/retry/merge/emit dispatch+confirm + 审计 + capability 缓存）
  ↓
Unit 3（前端：LaunchDialog + ActionsMenu + OperationsSection）
  ↓
Unit 4（静态托管 + ralph web --monitor + npm 入口 + docs/guide/monitor.md）
  ↓
Unit 5（全旅程 e2e + 视觉/键盘/reduced-motion 验收收尾）
```

### Unit 1：runtime 能力声明与回执面

#### 1. Unit 目标
`inspect run` 输出每 run `actions[]`（`stop/retry/merge/emit` 的 available/reason/revision/input_schema）；`ralph run --client-request-id <id>` 将值写入 manifest；`ralph loops stop|retry|merge --format json` 输出 typed receipt（human 输出不变）。

#### 2. 对应需求与 Scenario
R2/R3 的 runtime 面；S1/S3–S6/S8；D1–D5；E4/E5/E6/E7。

#### 3. 外部可观察结果
snapshot run：`"actions":[{"id":"stop","available":true,"revision":"<sha>","input_schema":null},{"id":"merge","available":false,"reason":"merge queue 无此 run 条目","revision":"<sha>"},{"id":"emit","available":true,"revision":"<sha>","input_schema":{"topics":["loop.cancel","task.resume"]}}]`；receipt：`{"ok":true,"action":"stop","loop_id":"…","detail":"SIGTERM sent","revision_after":"…"}`。

#### 4. 当前行为基线
无 actions 字段/flag/JSON 回执（Red）；`loops` 各操作只 human。

#### 5. 输入与输出
输入：run 证据（manifest/registry/merge_queue/pid）。输出：actions 数组；`--client-request-id` 字符串（校验 `[a-zA-Z0-9-]{8,64}` 否则拒绝）；receipt JSON。不变量：human 输出字节级不变；`emit` 白名单不新增语义。

#### 6. 修改位置
- `crates/ralph-cli/src/commands/inspect/run.rs`（候选；Plan1 产物）：`compute_actions(run,evidence) -> Vec<ActionDecl>` 纯函数——规则表：stop=pid 活着才 available；retry=终态（exited_failed/interrupted/cancelled）或 orphan 可 retry（`loops retry` 自身语义复用：registry 死条目/merge_queue 非终态?——**进入 U1 时读 `loops.rs::retry_loop` 前置校验逐条映射**，规则与 CLI 保持一致是唯一正确源）；merge=merge_queue 存在该 loop_id 且 state∈{merged?no, queued/merging/needs-review→可用性按 merge-button-state 逻辑}（复用 `merge_button_state` 内部函数而非重抄——进入 U1 时评估其可调用性，若耦合 stdout 则抽纯函数）；emit=pid 活着且 events_file 存在，input_schema.topics=固定白名单 `{loop.cancel, task.resume}`（v1 保守两枚；**进入 U1 时用 `rg "task.resume|loop.cancel" crates/` 核对 runtime 消费端确实存在**——task.resume 由 recovery responder 使用（observability.mdc 有记载），loop.cancel 对应 Cancelled 终态——核对后固定；若 `task.resume` 无消费端→白名单仅 `loop.cancel`）。
- `revision`：`sha256(run_id|state|iteration_max|merge_state|pending_count)`。
- `crates/ralph-cli/src/commands/run.rs`：`--client-request-id` arg→传 run_impl→manifest 字段 `client_request_id`（run_manifest.rs 加可选字段，向后兼容——旧 manifest 无字段=none）。
- `crates/ralph-cli/src/loops.rs`：`StopArgs/RetryArgs/MergeArgs` 加 `#[arg(long)] format: OutputFormat`（复用既有 `--format` 枚举模式——loops.rs 内已有 `OutputFormat`?进入 U1 时确认，若无则小枚举 json|human）；成功路径 `println` 分支出 JSON。
- 测试：`integration_inspect_run`（actions 断言）、`integration_run_client_id.rs`、`integration_loops_json.rs`（新增）；能力规则单测（`inspect/run.rs` 内 `#[cfg(test)]`）。

#### 7. 可依赖能力
E4 全部 loops 函数；run_manifest（Plan1）；inspect 框架；`common::*` 测试基建。

#### 8. 禁止依赖的未来能力
monitor dispatch/前端；emit 新 topic；pause/resume 语义（**永不引入**——R2 禁止伪造）。

#### 9. 验收测试
- actions 矩阵：running→stop/emit available、merge 按 queue 状态；exited→stop 不可用 reason；无 events_file→emit 不可用。
- `--client-request-id`：启动后 manifest.client_request_id=值；非法格式→非零退出。
- `--format json`：stop 真子进程→`ok:true`+signal 细节；retry/merge 各一 happy+case；human 默认不变（diff 基线）。
- 命令：`cargo nextest run -p ralph-cli --test integration_run_client_id --test integration_loops_json --test integration_inspect_run`。

#### 10. Acceptance Red
字段/flag 不存在→断言失败或 clap error。

#### 11. 单元测试拆分
compute_actions 全状态矩阵（约 15 case）；revision 哈希稳定性；merge 可用性与 merge-button-state 一致性（differential：同 fixture 两路径同结论）；client_request_id 校验。

#### 12. Red → Green → Refactor
```text
compute_actions 单测 Red → 实现 → Green
→ inspect 字段 Red → Green
→ client_request_id Red → 参数+manifest → Green
→ loops --format Red → 三处回执 → Green
→ Refactor（能力规则表数据化便于 diff）
```

#### 13. 最小实现范围
三处能力面。不实现：action 参数复杂 schema（v1 emit 只 topic 枚举+可选 payload 字符串）；`respond` action（并入 emit topic 语义）；cancel 独立 action（=emit loop.cancel）。

#### 14. 集成验证
fake backend `ralph run`→inspect actions 断言；真 `loops stop` 对 spawn 的 sleep 假进程发 SIGTERM（fixture 脚本 trap 记录）。

#### 15. 风险驱动测试
- Differential：merge 可用性 vs `merge-button-state` 实际命令同 fixture。
- Contract：receipt JSON schema 固化为 monitor 侧 zod 同源 golden。
- Fault：`--client-request-id` 重复值两 run（不防复用——manifest 记原样，monitor 对账取最近+告警 duplicate——**决策：monitor 侧 U2 处理重复 client_request_id 归属冲突→选最早 intent 匹配并记 audit warning**）。

#### 16. 回归范围
`integration_loops_merge`/`integration_merge_button`/`integration_run*`/`integration_wave_inspect`/`integration_resume`；`check-cli-doc-drift.sh`（新 flag 文档）；`audit-file-sizes.sh`。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `inspect/run.rs` | 修改 | actions 输出 | D1 |
| `commands/run.rs`、`run_manifest.rs`（core） | 修改 | client_request_id | D4 |
| `loops.rs` | 修改 | --format json | D3 |
| `tests/integration_{run_client_id,loops_json}.rs` | 新增 | ATDD | E12 |
| `docs/guide/cli-reference.md` | 修改 | drift | — |

#### 18. 完成标准
actions/receipt/client_request_id 测试绿；Rust 门禁；human 输出回归 diff 空。

#### 19. 停止条件
`loops retry` 前置校验与 capability 规则表达不一致（如 retry 只允许特定状态）→以 `retry_loop` 实际实现为准修正规则表；merge-button-state 无法抽出纯函数→在 inspect 内小复制并在注释标同步点+diff 测试防漂。

#### 20. 风险与注意事项
- action 规则与 loops 实现漂移：differential 测试同 fixture 双路径。
- emit 白名单扩张需求→descriptor 可声明 `interactions.topics`（预留，本计划不实现）。

---

### Unit 2：monitor operations 持久机 — intents/dispatch/confirm/审计/preset 目录

#### 1. Unit 目标
`POST /api/monitor/v1/operations`（kind∈launch|stop|retry|merge|emit）：校验+写 intent（idempotency_key 唯一）→dispatch 参数数组 spawn（cwd=项目）→confirm（回读 manifest/receipt）→confirmed|failed|confirm_timeout；`GET /operations[:id]` 审计；preset 目录 `GET /projects/:id/presets`（builtin list+custom `.ralph/hats/*.yml`）；SSE `operation.updated`；operation.confirmed→通知 outbox kind。

#### 2. 对应需求与 Scenario
R1/R2/R3/R5 服务面；S1–S9/S12；D3–D7/D11/D12；E3–E8/E15。

#### 3. 外部可观察结果
API：`POST→201 {operation{id,state,...}}`；同 idempotency_key→`200` 返回既有；revision 不符→409；capability 不可用→422+reason；confirm_timeout→状态可见+`POST /:id/recheck`。审计列表字段齐。

#### 4. 当前行为基线
无 operations 表/路由/dispatch（Red）；capability 字段由 U1 进 snapshot→collector 缓存 `run.actions`（本 Unit 在 ingest 中持久 actions JSON 列）。

#### 5. 输入与输出
输入：`{kind,project_id,run_gid?,params,idempotency_key,expected_revision?}`。输出：operation JSON；副作用：spawn `ralph`（env scrub+PATH）；审计写。状态机：`recorded→dispatching→dispatched→confirmed|failed|confirm_timeout`；`failed` 含 `receipt.error{code,stderr_tail}`。不变量：单 run 同时至多一非终态 op（部分唯一索引 `UNIQUE(run_gid) WHERE state NOT IN confirmed|failed`）；spawn 不 shell；launch 参数仅白名单映射 `{preset,prompt|prompt_file,plan?,max_iterations?,backend?}`；prompt_file 路径限项目根（allowlist 复用）。

#### 6. 修改位置
- `migrations/0008_operations.sql`：`operations(id pk,idempotency_key unique,kind,project_id,run_gid,client_request_id,params json,expected_revision,state,receipt json,actor,created_at,dispatched_at,confirmed_at,error)`+部分唯一索引；`runs` 表加 `actions json`（migration 同文件 ALTER）。
- `src/ops/intents.ts`：校验（zod schema+kind 参数表）+idempotency 命中逻辑+capability/revision 校验。
- `src/ops/dispatch.ts`：kind→argv 映射：`launch→['run','-a','-H',preset,'-p',prompt|'--prompt-file',pf,'--client-request-id',id]`；`stop→['loops','stop',run_id,'--format','json']`（`--force` 可选参数）；`retry→['loops','retry',run_id,steering?,'--format','json']`；`merge→['loops','merge',run_id,'--format','json']`；`emit→['emit',topic,payload?,'--file',events_file,'--json']`（topic 必须是该 run capability.input_schema.topics 成员，否则 422）。
- `src/ops/confirm.ts`：launch→等下一次采集（或立即触发该 project 采集）匹配 manifest.client_request_id；其余→receipt JSON 直接判定+回读 run 状态校验 revision_after；timeout 15s（env）。
- `src/ops/audit.ts`：actor=session id、origin、params（prompt 截断 160）、receipt 摘要。
- `src/presets/catalog.ts`：`ralph preset builtin list --format json`（spawn，缓存 projects.presets_cache+at）+`.ralph/hats/*.yml` 文件名枚举（custom 列表 id=`directory:<stem>`，E11-Plan2 语义）。
- `src/routes/operations.ts`、`src/routes/presets.ts`；`src/stream/monitor_log.ts` 新 kind `operation.updated`；`src/notify/rules.ts` 增补 `operation.confirmed→通知` 规则（v2 rule_version 仍=1？——决策：rules 版本表已有 version 列，新规则行 `id:"op_confirmed",version:1`）。
- `src/collector/collector.ts`：snapshot.actions→runs.actions 列；confirm 挂接（每轮采集后驱动 pending ops 的 confirm 判定）。
- 测试：`test/unit/{intents,dispatch_map,confirm}.test.ts`、`test/integration/{operations_api,launch,preset_catalog}.test.ts`、fake ralph 支持全部子命令脚本。

#### 7. 可依赖能力
Plan1–4 全部 monitor 设施；U1 契约；fake ralph 扩展（loops/emit/preset/run 子命令）。

#### 8. 禁止依赖的未来能力
前端（U3）；静态托管（U4）；`respond` 写 session（D11 已排除）。

#### 9. 验收测试
S1–S9/S12 全断言（§5 对应行）；argv 映射单测（无 shell 证据：spawn args 断言）；audit 字段；SSE kind。

#### 10. Acceptance Red
路由/表不存在；dispatch 未实现。

#### 11. 单元测试拆分
校验矩阵（kind×缺参/错参/路径越界/preset 不在目录/emit topic 越白名单/revision 不符）；idempotency 命中/跨 kind 键空间；状态机迁移合法性；confirm_timeout 分支；actor/origin 记录。
不允许 mock：spawn 用 fake ralph；sqlite 真约束。

#### 12. Red → Green → Refactor
```text
intents+校验 Red→Green → dispatch 映射 Red→Green → confirm Red→Green → routes/审计 Red→Green → catalog Red→Green → SSE/通知接入 → Refactor
```

#### 13. 最小实现范围
五 kind+审计+catalog+确认机。不实现：批量/排队调度、操作取消（operation 自身不可取消——记录）、webhook 通知操作结果（Plan4 渠道已通用）。

#### 14. 集成验证
fake ralph 双项目：launch→manifest 匹配；stop→sigterm 记录脚本；retry/merge happy；重启对账（S7）。

#### 15. 风险驱动测试
- Idempotency：同键并发双 POST（better-sqlite3 串行写天然串行——验证 UNIQUE 冲突→200 返回既有）。
- Concurrency：同一 run 两不同 op→部分唯一约束拒第二。
- Fault：spawn ENOENT（ralph 不在 PATH）→dispatch_failed+error；confirm 期 run 消失→confirm_timeout→recheck。
- Security：params 注入尝试（`--evil` 字符串进 prompt→进 argv 单元素不解析——断言 argv 数组原样）。

#### 16. 回归范围
monitor 全部；Plan4 通知规则兼容（新规则不扰既有）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `migrations/0008_operations.sql` | 新增 | 持久 | D6/D7 |
| `src/ops/{intents,dispatch,confirm,audit}.ts`、`src/presets/catalog.ts`、`src/routes/{operations,presets}.ts` | 新增 | 操作面 | D3–D7 |
| `src/{collector/collector,stream/monitor_log,notify/rules}.ts`、`src/routes/runs.ts` | 修改 | 接入 | E1 |
| `test/**` | 新增 | — | — |

#### 18. 完成标准
S1–S9/S12 服务面绿；门禁；fake ralph argv 断言齐全。

#### 19. 停止条件
`loops retry` steering 参数真实形状与假设不符→按 `--help` 实测修正映射；`preset builtin list` JSON 形状差异→按实测 zod。

#### 20. 风险与注意事项
- confirm 依赖采集周期→launch confirm 可能延至一轮 poll：实现"操作后立即对该 project 触发一次采集"缩短窗口。
- operations 表增长：保留全量（审计需求）+索引 created_at；记录未做归档。

---

### Unit 3：前端 LaunchDialog + ActionsMenu + OperationsSection

#### 1. Unit 目标
总览/详情提供"启动运行"入口：多步 Dialog（项目→preset（builtin/custom 目录）→prompt/plan/max_iterations/backend→预检→确认）；详情页 `ActionsMenu` 按 `actions[]` 渲染（禁用+reason tooltip）、ConfirmDialog（显示将执行动作+expected_revision 隐性携带）、操作进行中 inline 状态、操作历史 `OperationsSection`（详情页区+`/monitor/operations` 页面可选——决策：详情页内区+总览页全局 tab？取详情页区+Audit 页并入 settings？**简化为：详情页 OperationsSection + Inbox 旁 `/monitor/operations` 列表页**）；idempotency_key 由客户端生成（ULID 每意图一次，重试复用）。

#### 2. 对应需求与 Scenario
R1/R2/R3/R5 用户面；S1–S6/S8；D11/D12；E10。

#### 3. 外部可观察结果
Dialog 分步校验（每步 Next 校验+字段错误内联）；启动后 toast+自动跳转 run 详情（confirmed 后）；ActionsMenu 禁用项 title=reason；确认对话框显示 revision 截断+动作摘要；OperationsSection 行（kind/状态/时间/receipt 摘要/retry recheck 按钮 confirm_timeout 时）。

#### 4. 当前行为基线
只读 UI（Red）；ui/dialog/command/table 组件在（E10-Plan1）。

#### 5. 输入与输出
输入：`/projects`、`/projects/:id/presets`、`/runs/:id`(actions)、`POST /operations`、`GET /operations`。输出：UI+URL。副作用：mutation POST（CSRF 头已内置 api client）。不变量：表单失败保留内容与焦点；idempotency_key=每"打开表单+意图"一个（提交失败重试同键；重新打开新键）。

#### 6. 修改位置
- `features/monitor/launch/LaunchDialog.tsx`（新增；多步 stepper）、`features/monitor/actions/{ActionsMenu,ConfirmDialog}.tsx`、`features/monitor/ops/{OperationsSection,OperationsPage}.tsx`（新增）。
- `hooks.ts`：usePresets/useOperation/useCreateOperation（mutation→invalidate runs+operations）；ulid 生成（自写小函数或 nanoid？——无 nanoid 依赖→`crypto.randomUUID()`）。
- `App.tsx`：`/monitor/operations` 路由；`OverviewPage`/详情页接入入口按钮。
- 测试：组件 `.test.tsx`（步骤校验/revision 冲突呈现/重试同键断言/操作历史渲染）。

#### 7. 可依赖能力
Plan1–4 UI 设施；U2 API。

#### 8. 禁止依赖的未来能力
静态交付（U4）/e2e journey（U5）。

#### 9. 验收测试
S1–S6/S8 用户面断言（§5）。

#### 10. Acceptance Red
入口/组件缺失。

#### 11. 单元测试拆分
stepper 状态机；字段校验映射（API 422 detail→字段）；idem 键生命周期；ConfirmDialog revision 显示；OperationsSection 状态渲染矩阵。

#### 12. Red → Green → Refactor
```text
hooks/mutation Red→Green → LaunchDialog Red→Green → ActionsMenu/Confirm Red→Green → OperationsSection/Page Red→Green
```

#### 13. 最小实现范围
三组件+路由+接线。不实现：操作批量、计划任务、prompt 模板库。

#### 14. 集成验证
vitest；对真 monitor 手工旅程（记录截图）。

#### 15. 风险驱动测试
a11y：dialog focus trap/esc/初始焦点、菜单键盘、禁用原因可读（aria-describedby）；表单失败焦点回到错误字段。

#### 16. 回归范围
前端 vitest 全量；既有 e2e。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `features/monitor/{launch,actions,ops}/*` | 新增 | UI | E10 |
| `App.tsx`/`hooks.ts`/`OverviewPage`/`RunDetailPage` | 修改 | 接入 | — |

#### 18. 完成标准
S1–S6/S8 用户面绿；门禁。

#### 19. 停止条件
preset 目录 API 形状不足（custom 需 descriptor 提示）→补字段记录；mutation 错误体形制不齐→统一 `{error{code,field_errors?}}` 决策修订。

#### 20. 风险与注意事项
- 双击竞态：提交中按钮 loading 禁用+服务端 UNIQUE 双保险（已实现断言）。
- 长 prompt：textarea 计数+上限 4000（客户端软限）。

---

### Unit 4：静态托管 + `ralph web --monitor` + npm 入口 + 操作文档

#### 1. Unit 目标
monitor 经 `RALPH_MONITOR_STATIC_DIR` 托管 `frontend/ralph-web/dist`（SPA fallback、no-store index、静态缓存）；`ralph web --monitor` spawn monitor 单进程（不启 vite/ralph-api）并输出 URL、Ctrl+C 顺序关闭；root 新增 `dev:monitor`/`build:monitor`/`start:monitor`；`docs/guide/monitor.md`（安装/启动/注册/安全/操作/webhook/env 表/迁移说明）。

#### 2. 对应需求与 Scenario
R4；S10/S11；D8/D9/D10；E9/E10/E11/E13/E14。

#### 3. 外部可观察结果
`curl http://127.0.0.1:3200/`→index.html；`/monitor/runs/x`→index；`ralph web --monitor --no-open` 输出 `Dashboard http://localhost:3200` 且进程树仅 monitor。

#### 4. 当前行为基线
monitor 无静态；`ralph web` 无 `--monitor`（Red）。

#### 5. 输入与输出
输入：dist 目录/env flag。输出：静态文件/子进程管理。不变量：dist 缺失→启动警告不致命（API 仍可用，/ 返回提示页）；API 前缀不被 SPA fallback 吞掉。

#### 6. 修改位置
- `backend/ralph-monitor/package.json`：`+@fastify/static`；scripts `start`（生产入口）。
- `src/static/spa.ts`：注册 static root+`setNotFoundHandler`（GET 且非 `/api/`→index.html；HEAD 同）。
- `src/server.ts`/`config.ts`：`RALPH_MONITOR_STATIC_DIR`（默认 `<repo>/frontend/ralph-web/dist` 探测——monitor 在 repo workspaces 内时路径相对包；生产通用性靠 env 显式）。
- `crates/ralph-cli/src/web.rs`：`--monitor` flag→spawn `npm run start -w @ralph-monitor/server`（cwd=repo root；env：`RALPH_MONITOR_PORT=backend_port`、`RALPH_MONITOR_STATIC_DIR=<frontend dist 绝对路径>`）；ready pattern=`listening`（与 monitor server 输出对齐——U4 统一日志行 `ralph-monitor listening on http://127.0.0.1:<port>`）；URL=backend_port（无独立前端端口——`--frontend-port` 在 monitor 模式被忽略并提示）；关闭序列复用。
- root `package.json`：scripts 三枚加法。
- `docs/guide/monitor.md`（新增，操作向；**不入 `crates/ralph-core/data/*.md`**——那是 agent 上下文面）。
- `crates/ralph-cli/tests/integration_web_monitor.rs`（新增）：`ralph web --monitor --no-open`（用 `web.rs` 现有测试基建/fake npm？——`integration_web.rs` 先例：进入 U4 时读其实现，同模式 fake spawn 桩）。

#### 7. 可依赖能力
web.rs 子进程管理（E9）；fastify static 插件；既有 npm scripts。

#### 8. 禁止依赖的未来能力
U5 e2e；不改默认 `ralph web`。

#### 9. 验收测试
`static.test.ts`：index/fallback/api 隔离/无 CORS 放开头；`integration_web_monitor.rs`：spawn 参数/URL/关闭序列（沿用 integration_web.rs 模式）；`monitor.md` 链接与命令准确性自检（文档清单对照实现）。

#### 10. Acceptance Red
GET / →404/无 `--monitor` flag。

#### 11. 单元测试拆分
fallback 路由矩阵（/api 不吞、`.js` 资源直出、深层路径 index）；dist 缺失降级；env 优先级。

#### 12. Red → Green → Refactor
```text
static Red→Green → web flag Red→Green → scripts/docs → Green → Refactor
```

#### 13. 最小实现范围
托管+flag+脚本+文档。不实现：TLS、压缩中间件（fastify 默认足够）、缓存策略复杂化。

#### 14. 集成验证
`npm run build -w @ralph-web/dashboard`→真启动 monitor→curl 矩阵；`ralph web --monitor` 手动验证（记录）。

#### 15. 风险驱动测试
路径穿越到静态（`..%2f`）；SPA fallback 不误伤 `/api` 404（API 404 仍 JSON）；压缩 mime。

#### 16. 回归范围
`integration_web.rs`（既有 flag 不回归）；monitor 全部；root npm scripts smoke。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `src/static/spa.ts`、`src/{server,config}.ts` | 新增/修改 | 托管 | D8 |
| `backend/ralph-monitor/package.json` | 修改 | +@fastify/static | E11 |
| `crates/ralph-cli/src/web.rs` + `tests/integration_web_monitor.rs` | 修改/新增 | flag | D9/E9 |
| `package.json`（root） | 修改 | scripts | D10/E14 |
| `docs/guide/monitor.md` | 新增 | 操作文档 | E13 |

#### 18. 完成标准
S10/S11 绿；门禁；文档命令逐条实测记录。

#### 19. 停止条件
`integration_web.rs` 测试基建不支持新 flag 桩→扩展该基建最小改动；SPA fallback 与 fastify notFound 冲突→自定义 404 handler 明确分流。

#### 20. 风险与注意事项
- dist 与 monitor 版本漂移（旧 dist 新 server）：启动日志打 dist mtime 提示（轻量）。
- 非 loopback 部署：文档明确仅 loopback 支持；`RALPH_MONITOR_HOST` 非 loopback→启动警告+要求显式 `--allow-non-loopback`（fail-closed）。

---

### Unit 5：全旅程 e2e + 视觉/键盘/reduced-motion 验收收尾

#### 1. Unit 目标
`e2e/monitor-journey.spec.ts` 用**真 monitor 服务+fake ralph 双项目**跑通：注册两项目→各造历史 run（fake events/manifest）→总览区分→A 项目 launch→running 出现→详情四层健康→stop→confirmed→通知产生→点击深链→模拟断线（杀 monitor 进程重启）→恢复一致；附加 sweep：两 viewport（1280×800/375×812）、dark/light、纯键盘 Tab 旅程、`prefers-reduced-motion` 截图记录。

#### 2. 对应需求与 Scenario
R4 的验收层 + 全系列回归面；S13；E12（playwright 配置）。

#### 3. 外部可观察结果
spec 全绿；截图工件清单（`e2e/artifacts/monitor-journey/*.png`）+发现的问题列表（若非阻塞记入文档注释，阻塞则修）。

#### 4. 当前行为基线
无该 spec；webServer 配置已有（E12）。

#### 5. 输入与输出
输入：fake ralph PATH 注入+临时项目对+临时 DB。输出：页面断言+截图。副作用：起真 monitor 端口（动态端口 `RALPH_MONITOR_PORT=0`?——fastify listen 0 取实际端口，webServer 用 `url:http://127.0.0.1:<port>`——**进入 U5 时验证 port=0 模式取实际端口写入 stdout/health 探测**；若不支持则固定测试端口 3300）。

#### 6. 修改位置
- `e2e/monitor-journey.spec.ts`（新增）+`e2e/helpers/monitor-server.ts`（启动子进程+fixture 项目生成器+fake ralph 写入 PATH——复用 monitor `test/fixtures/fake-ralph` 脚本扩展全部子命令）。
- `playwright.config.ts`：`webServer` 数组（monitor+既有 vite dev——决策：e2e 前端仍走 vite dev 避免每测 build；视觉 sweep 同源 proxy 一致）。
- `frontend/ralph-web/package.json`：`"test:e2e":"playwright test"`（缺失补脚本，E15-Plan1 确认无此 script）。
- `docs/guide/monitor.md`：验收记录段补截图路径（或 `e2e/artifacts/README`）。

#### 7. 可依赖能力
全部 Unit 1–4；fake ralph 全集；route.abort/拦截手段（Plan3-U4 已选手段）。

#### 8. 禁止依赖的未来能力
无（收尾 Unit）。

#### 9. 验收测试
S13 全旅程+sweep；`test:e2e` 脚本可用。

#### 10. Acceptance Red
spec 文件缺失→运行即失败（playwright no tests）。

#### 11. 单元测试拆分
e2e helper 自身：fixture 项目生成（manifest/events/locks 齐全）；fake ralph 子命令覆盖表（run/inspect/loops/emit/preset/events）。

#### 12. Red → Green → Refactor
```text
helper Red→Green → 旅程 spec Red→Green → sweep Red→Green
```

#### 13. 最小实现范围
一 spec+helper+脚本+记录。不实现：跨浏览器（chromium 单通道同既有 config）、CI 矩阵。

#### 14. 集成验证
`cd frontend/ralph-web && npx playwright test e2e/monitor-journey.spec.ts`；全量 e2e 回归。

#### 15. 风险驱动测试
E2E 本身即 fault 场景（杀进程）；flake 防护：waitFor 语义断言非 sleep（`expect.poll`/`toBeVisible` 超时内轮询）。

#### 16. 回归范围
全部 e2e；`npm test`（root）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `e2e/monitor-journey.spec.ts`、`e2e/helpers/monitor-server.ts` | 新增 | ATDD | E12 |
| `playwright.config.ts`、`frontend/ralph-web/package.json` | 修改 | webServer/test:e2e | E12 |
| `docs/guide/monitor.md` | 修改 | 验收记录 | — |

#### 18. 完成标准
S13 绿；sweep 记录完整；root 全量门禁。

#### 19. 停止条件
playwright 无法注入 PATH 给 webServer→改 spawn env 显式（已定方案）；port 0 不支持→固定 3300 并加并发锁标记。

#### 20. 风险与注意事项
- 截图 diff 不做硬断言（决策同 Plan1：视觉用人工记录）。
- fake ralph 行为覆盖不足→helper 自检表（子命令→fixture 文件映射断言）。

## 8. Unit 串行依赖图

```text
Unit 1 → Unit 2 → Unit 3 → Unit 4 → Unit 5
```

- U2 依赖 U1 三处 runtime 契约；U3 依赖 U2 API；U4 依赖 monitor 全部功能+dist；U5 依赖 U1–U4 全部。

## 9. 执行命令清单

```bash
# Rust（U1/U4）
C1  cargo nextest run -p ralph-cli --test integration_run_client_id
C2  cargo nextest run -p ralph-cli --test integration_loops_json
C3  cargo nextest run -p ralph-cli --test integration_inspect_run
C4  cargo nextest run -p ralph-cli --test integration_web integration_web_monitor
C5  cargo nextest run -p ralph-cli --test integration_loops_merge integration_merge_button
C6  ./scripts/check-cli-doc-drift.sh && ./scripts/audit-file-sizes.sh
C7  cargo fmt --all -- --check && cargo clippy -p ralph-cli -- -D warnings
C8  cargo build -p ralph-cli

# monitor（U2/U4）
C9  npm test -w @ralph-monitor/server
C10 npm run typecheck -w @ralph-monitor/server && npm run build -w @ralph-monitor/server
C11 npm run start -w @ralph-monitor/server          # 手工验证静态托管

# frontend（U3/U5）
C12 npm test -w @ralph-web/dashboard
C13 npm run build -w @ralph-web/dashboard           # 产出 dist 供 U4/U5
C14 cd frontend/ralph-web && npm run test:e2e        # 全部 e2e（新脚本）
C15 cd frontend/ralph-web && npx playwright test e2e/monitor-journey.spec.ts

B1  npm test && npm run build
B2  ./scripts/run-tests.sh
```

## 10. 最终质量门禁

- [ ] S1–S13 全绿；R1–R5 追踪无空洞；
- [ ] idempotency（同键重放/并发）、revision 冲突、spawn 崩溃对账、confirm_timeout、单 op 约束全有测试；
- [ ] `--format json` receipt 契约 golden 与 monitor zod 同源；
- [ ] emit topic 白名单仅 runtime 声明值可执行；
- [ ] CSRF/会话对 curl 矩阵验证（含拒绝）；
- [ ] human 输出回归 diff 为空；`ralph web` 默认路径未变；
- [ ] 静态托管：SPA fallback/资源/API 隔离/路径穿越矩阵；
- [ ] `docs/guide/monitor.md` 命令逐条实测；截图/视口/键盘/reduced-motion 记录完整；
- [ ] 操作通知接入 outbox 生效；
- [ ] Rust 门禁+全部 npm 测试+playwright 全量+`./scripts/run-tests.sh` 绿；无 skip/.only/弱化断言；
- [ ] 置信度复核 ≥0.85；变更不超范围；五计划交叉引用一致（术语/契约/迁移编号）。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
| ---- | ---- | ---- |
| 实施计划而非 Roadmap | 是 | — |
| Executor 仍需关键设计决策 | 否 | D1–D12；候选挂点均给定位命令 |
| 文件/接口有代码库证据 | 是 | — |
| 关键决策置信度 ≥0.85 | 是 | 最低 D11=0.85 |
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

**Series**: `feat/monitor-product` — 依赖 Plan 1–4 全部交付物；本计划关闭环并交付生产形态。完成本计划即完成 monitor v1 产品面。
