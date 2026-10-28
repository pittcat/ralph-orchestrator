---
title: feat(monitor): 跨项目监控基础与 React 总览
type: feat
date: 2026-10-28
baseline: 4c8718b77327d90087d6665e51654b58de90b805
execution: code
status: READY
---

# 跨项目监控基础与 React 总览

## 0. 计划状态

`READY` — 所有实施关键决策置信度均 ≥ 0.85，无 BLOCKED 项。

- **代码库基线**：`4c8718b77327d90087d6665e51654b58de90b805`（2026-09 之后 master）。
- **调查范围**：`crates/ralph-core`（loop_registry/loop_lock/loop_history/event_logger/loop_context/config/notifications）、`crates/ralph-cli`（commands/{inspect,events,run,resume,preset,emit}、loops.rs、web.rs、loop_runner/{inner,loop_owner,notifications}）、`crates/ralph-api`（transport/loop_domain/preset_domain/collection_domain/planning_domain/task_domain/stream_domain/config/mcp）、`crates/ralph-adapters`（cli_executor/claude_stream）、`frontend/ralph-web`（App.tsx、components/ui、rpc/trpc、hooks、stores、e2e、playwright.config、vite.config、index.css、package.json）、`backend/ralph-web-server`（package.json）、`presets/`（manifest/index/en）、`scripts/`（run-tests/check-cli-doc-drift/audit-file-sizes）、`docs/achieved/plan/`、`.cursor/rules/*.mdc`。
- **已执行验证命令**：`git log`/`git status`（确认 5 个计划为未跟踪新增）；`cargo metadata`；多处 `sed`/`rg` 源码与配置读取；`.ralph/` 实况目录检查。
- **尚未执行的验证**（留给对应 Unit 的 Red 阶段）：`ralph inspect run`（计划新增，不存在属预期）；`backend/ralph-monitor`（计划新增 workspace）；`presets/monitor/`（计划新增目录）。
- **阻塞项**：无。

本计划是 monitor 系列的基础计划。术语在本文件定义并被后续 4 个计划引用：

- **run（运行）**：一次 `ralph run`/`ralph resume`/`collection.run` 产生的 Ralph 编排实例（primary 或 worktree loop）。
- **run_id**：runtime 在启动时为 run 分配的稳定身份（worktree loop 为 `loop-<ts>-<hex>`；primary loop 为 `primary-<ts>-…`，Evidence E7）。
- **global_run_id**：`<machine_id>:<project_id>:<run_id>` 三段式——`run_id` 仅 workspace 内唯一（E4），跨项目需 `project_id` 段；`machine_id` 由 monitor 服务生成并持久化（`meta` 表，node:crypto `randomUUID`）。对外 detail 路由 `/runs/:global_run_id` 按三段匹配。
- **run manifest**：`.ralph/runs/<run_id>.manifest.json`（计划新增），runtime 启动写入、终态更新。
- **observation snapshot**：`ralph inspect run --format json`（计划新增）输出的 v1 结构化观测。
- **collector**：monitor 服务内按项目串行采集的组件。
- **projection**：monitor SQLite 中由观测数据重放的 `runs` 读模型。

## 1. 功能目标

业务目标：让用户在单一 Web 工作台中观察**多个项目目录**里的 Ralph 运行历史与当前状态。

| R-ID | 需求（可观察行为） | 行为差异（当前 → 目标） |
| ---- | ---- | ---- |
| R1 | 用户注册多个项目目录；monitor 自动发现其中以任何方式（终端 CLI、ralph web、collection.run）启动的运行，展示实际 cwd、项目根、worktree、任务、preset 与可选 plan | 现状：ralph-api `loop.list` 只看单 workspace（`ApiConfig.workspace_root`，E11）；terminal 启动的 primary loop 仅由 `.ralph/loop.lock` 短暂指示，进程死后无历史（E4）→ 目标：注册项目即采集，`inspect run` 返回 primary+worktree 全量运行及持久 manifest |
| R2 | run 具有跨目录稳定身份；进程退出、执行成功、交付成功、merge 状态分别表达；缺证据显示 `unknown`，不得虚构成功 | 现状：`"(primary)"` 是展示名而非身份（E6）；registry 以 PID 存活度判状态且自动清理死 PID（E4）→ 目标：`global_run_id` 稳定；状态机 `running/silent/exited_success/exited_failed/exited_merged/exited_unknown`，状态由 manifest terminal + merge queue + PID 存活证据组合 |
| R3 | 服务重启后保留运行历史与采集游标；registry 清理不丢 monitor 终态 | 现状：monitor 不存在 → 目标：SQLite 持久 projects/runs/source_cursors/projection_meta，manifest 落盘即终态证据 |
| R4 | 总览：按项目/时间浏览 run，搜索、按项目/preset/状态筛选、排序、选中后最小详情页 | 现状：`frontend/ralph-web` 只有 tasks/builder/plan/settings 路由（E13）→ 目标：新增 `/monitor` 总览与 `/monitor/runs/:id` 详情 |
| R5 | TypeScript 监控服务独立运行；首次接入只监听 loopback；项目路径只由用户注册 | 现状：默认 web 栈是 `ralph-api`（Rust）+ Vite 前端；Node `backend/ralph-web-server` 已标记 legacy/deprecated（E12、E14）→ 目标：新建 `backend/ralph-monitor` workspace，默认 `127.0.0.1`，路径白名单 |

范围：多项目目录注册与运行观测总览；单机本地。

非目标：多机器采集、团队协作、云托管、任意远程终端接入、可执行 UI 插件、Web 内发起运行（Plan 5）、事件级时间线/日志（Plan 2）、断线恢复协议 SSE（Plan 3）、通知（Plan 4）。

输入：用户注册的项目目录路径；runtime 写盘的 `.ralph/*` 证据文件。输出：`/api/monitor/v1/*` JSON；React 总览页。

错误语义：采集失败逐项目隔离，`runs` 保留上次快照并标 `stale`；无证据状态 `unknown`；API 输入非法返回结构化 4xx（`error{code,message,details}`，沿用 ralph-api envelope 风格 E10）。

兼容要求：不得修改 `ralph-api` 既有方法/流契约；`ralph run`/`resume`/`loops` 既有 CLI 输出不变（新增 manifest 落盘与 `inspect run` 子命令是加法）；`.ralph/runs/` 新目录不得干扰既有 `.ralph/*` 读取方。

性能：默认轮询 3 s 周期；单项目采集为一次子进程 + 一次 SQLite 事务；总览 500 runs 内首屏 < 1 s（本地）。

安全：默认 loopback；写操作需同源会话 + CSRF；项目路径仅注册路径；prompt 以预览+哈希存储不全文入库；日志脱敏（home 路径、prompt 截断）。

已确认假设：A1 `.ralph/runs/<run_id>.manifest.json` 是唯一新增 runtime 落盘物（决策 D2）；A2 前端依赖已满足，无框架升级（D6）。

待验证假设：无（全部已在 U 内以 Red 验证方式处理）。

## 2. 代码库现状与证据

### 2.1 当前实现入口与调用链

**run 生命周期（runtime 侧）**

```text
ralph run [-H preset] -p prompt           # crates/ralph-cli/src/main.rs:394 → commands/run.rs
  → loop_runner/run_impl.rs               # primary: LoopLock .ralph/loop.lock；worktree: worktree create + registry
    → loop_runner/loop_owner.rs           # LoopRegistry::register → .ralph/loops.json (E4)
    → loop_runner/inner.rs event loop     # EventLogger → .ralph/events-*.jsonl (E7)
    → handle_termination (inner.rs:1675+) # SummaryWriter→summary.md；LoopHistory→history.jsonl；
                                          # MergeQueue 更新；registry.deregister；RpcEvent::LoopTerminated
```

**观测入口（runtime 侧，现状）**

```text
ralph inspect loop --format json          # commands/inspect.rs：当前 loop marker/hat/events/scheduler 块（E8）
ralph events --file ... --format json     # commands/events.rs：EventHistory 读 JSONL（E9）
ralph loops list|stop|retry|merge         # loops.rs：registry + loop.lock + merge-queue 聚合（E5）
ralph-api loop.list                       # loop_domain.rs：LoopRecord{id,status,location,prompt,merge_commit}（E11）
```

**Web 栈（现状）**

```text
ralph web                                 # crates/ralph-cli/src/web.rs：spawn `cargo run -p ralph-api` + `npm run dev`（frontend/ralph-web）
frontend/ralph-web                        # React 19 SPA；vite proxy /rpc(ws)+/health → 127.0.0.1:3000（E15）
backend/ralph-web-server                  # Fastify+tRPC+SQLite legacy 后端，仅 --legacy-node-api 启用（E12）
```

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
| ---- | ---- | ---- | ---- | ---- |
| E1 | `crates/ralph-api/src/config.rs` | `ApiConfig` 默认 `127.0.0.1:3000`；env `RALPH_API_*`；`trusted_local` 拒绝非 loopback；`token` 模式可非 loopback | monitor 服务沿用同构 env 命名 `RALPH_MONITOR_*` 与 loopback 默认 | 高 |
| E2 | `crates/ralph-api/src/transport.rs` + `protocol.rs` | axum 路由 `/health`、`POST /rpc/v1`、`/rpc/v1/capabilities`、`/rpc/v1/stream`；envelope `{apiVersion:"v1",id,method,params,meta{idempotencyKey,auth,timeoutMs}}`；50 个 method 白名单 | monitor REST 不复用 RPC envelope，但沿用 `apiVersion` 与 `{error:{code,message,details}}` 形状 | 高 |
| E3 | `crates/ralph-api/src/idempotency.rs` | 内存幂等表 TTL 3600 s，`Idempotency-Key`/`meta.idempotencyKey` | monitor mutation 用持久表而非内存（Plan 5）；本计划只保留接口形状 | 高 |
| E4 | `crates/ralph-core/src/loop_registry.rs` | `.ralph/loops.json`；`LoopEntry{id:"loop-{ts}-{hex}",pid,started,prompt,worktree_path?,workspace,owner_hat_id?}`；每次操作先 `flock`+按 `is_pid_alive` 剪除死 PID；primary loop **不入 registry** | 不能拿 registry 当持久历史；monitor 需要 manifest 层补终态与 primary 身份 | 高 |
| E5 | `crates/ralph-cli/src/loops.rs` | `loops list` 聚合 `LoopLock`+registry+orphan worktrees+merge queue；`resolve_loop` 支持 exact/partial/merge-queue 匹配；`stop` = SIGTERM（`--force` SIGKILL） | 终止证据来源 = merge queue + manifest；`stop` 走信号不产终态记录，manifest terminal 由 runner 自己写（或缺失→unknown） | 高 |
| E6 | `crates/ralph-api/src/loop_domain.rs` | primary loop 在列表中 id 固定 `"(primary)"`；status∈`running/orphan/crashed/queued/merging/merged/needs-review/discarded`；`includeTerminal` 默认隐藏 merged/discarded | `"(primary)"` 仅展示名，禁止作 run_id；状态词表映射进 monitor state 机 | 高 |
| E7 | `.ralph/flow-authority.jsonl`（实况）+ `crates/ralph-cli/src/operation_guard.rs` | primary run 的 `loop_id` 形如 `primary-20260907-021122`；`OperationContext` 读 `.ralph/current-loop-id` 或 `RALPH_CURRENT_LOOP_ID` | primary run 有真实 runtime id；manifest 直接记录该 id，monitor 无需猜测 | 高 |
| E8 | `crates/ralph-cli/src/commands/inspect.rs` | 子命令 `profiles`/`loop`/`prompt`；`inspect loop --format json` 输出 `OperationContext`+events 路径+`scheduler` 块（dag 模式）；文件头注释声明 JSON 是机器可读 SSOT | `inspect run` 新子命令归属同一 namespace，遵循既有 `--format json` 契约 | 高 |
| E9 | `crates/ralph-cli/src/commands/events.rs` + `crates/ralph-core/src/event_logger.rs` | `ralph events --last/--topic/--iteration/--format/--file/--events-source auto|main|hat-channel`；`EventRecord{ts,iteration,hat,topic,triggered?,payload,blocked_count?,wave_*,phase?}`；`.ralph/events-YYYYMMDD-HHMMSS.jsonl`+`current-events` marker；hat-channel `.ralph/agent/events-hat-<hat>-<loop>-<n>.jsonl` | events 文件路径由 `inspect run` 输出提供；payload ≤50000 字符截断——monitor 不得假设全文 | 高 |
| E10 | `crates/ralph-cli/src/loop_runner/inner.rs`（handle_termination ~L1675-2030） | 统一终态管线：清 supervisor/worktree→SummaryWriter（`.ralph/agent/summary.md`）→`hist.record_completed/terminated`（`.ralph/history.jsonl`）→merge queue 转移→registry deregister→`RpcEvent::LoopTerminated` | manifest 终态更新的挂点 = 此统一管线内一处写入，覆盖成功/失败/中断全部路径 | 高 |
| E11 | `crates/ralph-api/src/loop_domain.rs` + `collection_domain.rs` | `LoopDomain` 仅绑定 `ApiConfig.workspace_root` 单目录；`collection.run` spawn `ralph run -H <yml> -a -p <prompt>` cwd=workspace | 跨目录观测是 monitor 层新职责；monitor 不得复用 ralph-api 单 workspace 假设 | 高 |
| E12 | `crates/ralph-cli/src/web.rs` | `ralph web` 默认 spawn `cargo run -p ralph-api`（env `RALPH_API_PORT/RALPH_API_WORKSPACE_ROOT`）+ `npm run dev -- --port <fp>`（cwd=`frontend/ralph-web`，env `RALPH_BACKEND_PORT`）；`--legacy-node-api` 才跑 `backend/ralph-web-server` | monitor 不替换默认 web 栈；Plan 5 以 additive flag 接入 | 高 |
| E13 | `frontend/ralph-web/src/App.tsx` + `components/layout/` | 路由 `/tasks /tasks/:id /builder /plan /settings`；`AppShell`+`Sidebar`+`NavItem`；`components/ui/` 已有 badge/button/card/input/label/textarea | `/monitor*` 加法进 App.tsx 与 Sidebar；缺 dialog/command/table/skeleton 组件（计划新增） | 高 |
| E14 | `backend/ralph-web-server/package.json` + `.cursor/rules/architecture-modules.mdc` | `@ralph-web/server`：fastify 5.7.1 / @trpc/server 11.8.1 / better-sqlite3 12.6.2 / drizzle-orm 0.45.1 / zod 4.3.6 / tsx 4.19；测试 `tsx --test`；规则文件标记 legacy/deprecated | 新 monitor 复用同一依赖家族（fastify/better-sqlite3/zod/tsx），但**不复用** tRPC/drizzle 与旧包代码 | 高 |
| E15 | `frontend/ralph-web/{package.json,vite.config.ts,playwright.config.ts,src/index.css}` | react 19.1 / router 7.13 / @tanstack/react-query 5.80 / zustand 5 / @xyflow/react 12.10 / react-markdown 10.1 / lucide 0.563 / tailwind 4.1.18 / vitest 3.1.4+jsdom+testing-library / playwright 1.58；vite proxy `/rpc`+`/health`→127.0.0.1:3000；e2e/ 已有 builder、closed-toggle 两个 spec（`page.route` 拦截 + `window.__observationStore` dev hook 模式）；index.css 是 Tailwind v4 `@theme`，**只有暗色 token** | 依赖已满足无需升级；monitor API 需新 proxy 前缀；亮色主题 token 需新增；e2e 沿用 route-intercept 模式 | 高 |
| E16 | `presets/{manifest.yml,index.json,en/*.yml}` | builtin 10 个 preset；`ce-executor-pipeline` 为线性主路径（work.start→…→report.done+LOOP_COMPLETE）；修改 builtin 需 4 处同步（en yml+manifest+presets.rs+index.json+zsh） | 本计划不改 preset；Plan 2 的 `presets/monitor/` 是新增目录不动 event topology | 高 |
| E17 | `scripts/run-tests.sh` + `mise.toml` + `crates/ralph-cli/tests/common/mod.rs` | 两阶段 nextest（P1 并发排除 `partial_timeout_events_visible|test_execute_wave_|wave_supervisor::`，P2 `-j1`）；nextest 钉 `0.9.140`（mise.toml SSOT）；`common::ralph_bin`+`scrub_agent_runtime_env`（清 10 个 `RALPH_*` 键） | Rust 测试全走 nextest；新集成测试复用 `tests/common` | 高 |
| E18 | `crates/ralph-core/src/loop_history.rs` | `.ralph/history.jsonl`（每 workspace 一份 append-only）；`HistoryEvent` kind：`loop_started/loop_completed{reason}/loop_terminated{signal}/merge_*/loop_resumed/…`；**无 loop_id 字段**，worktree loop 的 history 在其自身 `.ralph/` 下 | history 可作终态佐证但不能归因到 run——manifest 是唯一 run 级终态权威 | 高 |
| E19 | `crates/ralph-cli/src/loop_runner/notifications.rs` + `crates/ralph-core/src/config/notifications.rs` | 既有 loop 终止 webhook：`notifications:` config `{enabled,timeout_seconds=5,endpoints[{name,url,on,headers,body}]}`，best-effort、URL 脱敏、无重试/持久化/delivery id | Plan 4 的通知 outbox 是新系统；不得静默改写项目既有 notifications 配置 | 高 |
| E20 | `crates/ralph-api/src/stream_domain/` | 内存 stream：history `VecDeque` 2048、live broadcast 256、replay 200、keepalive 15 s、cursor 校验+单调 ack | monitor 的 durable seq + SSE 是新实现（Plan 3）；不得声称复用 ralph-api stream | 高 |
| E21 | `crates/ralph-api/src/mcp.rs` | `ralph mcp serve` stdio 暴露 control-plane methods+stream topics 为 MCP tools | monitor REST 不经过 MCP；Plan 5 自动化入口复用同一 REST | 中 |
| E22 | `docs/achieved/plan/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md` | 本仓库详细计划范例：0-11 节 + 20 项 Unit + 共享契约 G/H + 命令别名 | 本文件沿用其结构 | 高 |
| E23 | `root package.json` + `.nvmrc` + `package-lock.json` | workspaces `backend/*`、`frontend/*`；`node>=22`，`.nvmrc=24`；root lockfile 存在 | 新 workspace 自动纳入 `npm install`/`npm test`；测试脚本名遵循 workspace 约定 | 高 |
| E24 | `scripts/audit-file-sizes.sh` | 仓库有源码规模审计（loop_runner/config/event_loop/main/cli/commands） | Rust 新增模块保持单文件 ≤600 行目标，入审计清单 | 中 |

### 2.3 受影响范围

**修改的既有文件（已确认存在）**

- `crates/ralph-cli/src/commands/inspect.rs`：新增 `Run` 子命令枚举与 handler（E8）。
- `crates/ralph-cli/src/commands/inspect/` 子模块目录若存在则放实现；否则 `inspect.rs` 内新增 `run` 模块（**进入 U1 前确认**，现为候选位置）。
- `crates/ralph-cli/src/loop_runner/run_impl.rs` + `loop_owner.rs` + `inner.rs`（`handle_termination` 段）：manifest 写入/终态更新挂点（E10）。
- `crates/ralph-cli/src/commands/run.rs`：primary/worktree run 启动后写 manifest 的入口（复用 `repo_root`/context 计算）。
- `frontend/ralph-web/src/App.tsx`、`components/layout/Sidebar.tsx`、`vite.config.ts`、`src/index.css`（E13、E15）。
- `package.json`（root）：workspace 脚本 `dev:monitor`、`test:monitor` 加法（E23）。

**计划新增（明确标注）**

- `crates/ralph-cli/src/commands/inspect/run.rs`（或等价子模块）：`inspect run` 实现。
- `crates/ralph-cli/tests/integration_inspect_run.rs`：新集成测试。
- `crates/ralph-core/src/run_manifest.rs`：manifest schema+读写（`LoopContext` 旁，不动 EventLogger）。
- `backend/ralph-monitor/`（package/tsconfig/src/migrations/test）。
- `frontend/ralph-web/src/features/monitor/`、`src/lib/api.ts`（如不存在则随 U4 新建，候选）、`e2e/monitor-overview.spec.ts`、所需 `components/ui/*` 新组件（dialog/command/table/skeleton/sonner-风格 toast 以最小自实现）。
- `.ralph/runs/`（runtime 新目录，不入 git）。

**明确不修改**：`ralph-api` 的 method/topic/流契约；`presets/en/*.yml` 与 manifest/index.json；`backend/ralph-web-server` 代码；`crates/ralph-core/src/event_logger.rs` 的 `EventRecord` 形状；`.ralph/loops.json` 格式；既有前端路由/页面逻辑。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除原因 | 置信度 |
| ---- | ---- | ---- | ---- | ---- | ---- | ---- |
| D1 | run 唯一身份 | A. registry LoopEntry.id；B. pid+cwd；C. `"(primary)"` 显示名；D. runtime 分配 `run_id`+monitor `machine_id` 复合 | D：`global_run_id = machine_id:project_id:run_id`（三段式；run_id 仅 workspace 内唯一）；`run_id` 用 loop_id/`primary-<ts>`；DB PK=`(project_id, run_id)` | E4（registry 剪死 PID、primary 不入、run_id 为 workspace-local）、E6（"(primary)" 非身份）、E7（primary 有真实 id） | A 不持久不覆盖 primary；B 可复用会碰撞；C 仅是字符串；两段式 `machine:run` 无法区分跨项目同名 run_id | 0.9 |
| D2 | run 级持久观测落点 | A. 扩展 `.ralph/loops.json`；B. 扩展 history.jsonl 加 loop_id；C. 新 `.ralph/runs/<id>.manifest.json` | C：append-once manifest，启动写+终态更新 | E4（registry 非历史库）、E18（history 无 run 归因且 append-only 不便更新）、E10（统一终态管线有挂点） | A 继续被剪除/缺 primary；B 改造历史格式影响既有读取方且不能"更新终态" | 0.87 |
| D3 | monitor↔runtime 采集面 | A. monitor 直读 `.ralph/*` 文件；B. `ralph inspect run --format json` 受支持 CLI；C. ralph-api RPC per project | B：版本化 JSON snapshot；文件读取仅作兜底降级路径（`inspect run` 内部实现可读文件，但 monitor 只调 CLI） | E8（inspect JSON=SSOT 定位）、E9（events CLI 同模式）、E11（ralph-api 单 workspace） | A 把内部文件格式耦合给 TS（schema 变更即破）；C 要求每项目跑一个 api 进程 | 0.88 |
| D4 | monitor 服务形态 | A. 扩展 `backend/ralph-web-server`；B. 扩展 `ralph-api`；C. 新 `backend/ralph-monitor` | C：独立 Fastify5+better-sqlite3+zod+tsx workspace，REST `/api/monitor/v1/*` | E14（旧后端 deprecated 且 tRPC/drizzle 不沿用）、E11/E12（ralph-api=每 workspace 控制面，非跨目录观测面）、E23（workspaces 通配自动纳入） | A 复活 deprecated 代码路径；B 让 Rust 控制面背跨项目职责+sqlite | 0.9 |
| D5 | SQLite 访问方式 | A. drizzle-orm；B. better-sqlite3 prepared statements + SQL migration 文件 | B：少一层 ORM；migrations/NNNN_*.sql 顺序应用 | E14（better-sqlite3 已在 legacy 工作区验证于 node24）；表 ~10 张、无复杂 join 需求 | A 引入不必要抽象与生成器；沿用 drizzle 仅为"看起来一致" | 0.86 |
| D6 | 前端依赖与结构 | A. 按原计划升级到 react19.3/router7.18/tailwind4.3；B. 保持现有版本，`features/monitor/` 目录化 | B：版本不动（lockfile 已存在）；新 feature 目录+缺失 ui 组件最小自实现 | E15（现有版本即近新稳定版且 e2e/vitest 已通）；升级无行为收益且引回归面 | A 无证据支持的版本跳跃，徒增风险 | 0.9 |
| D7 | 采集调度 | A. 全项目并行 spawn；B. 逐项目串行 + 失败隔离 | B：单进程池、逐项目 timeout、错误入 project health | E4/E9（ralph 子进程有 flock 交互，串行避免锁竞争）；失败隔离为一项明确需求（R5/错误语义） | A 无必要吞吐且锁竞争风险 | 0.86 |
| D8 | 状态机语义 | A. 沿用 ralph-api `LoopRecord.status` 词表；B. monitor 独立词表 + 证据字段 | B：`running/silent/exited_success/exited_failed/exited_merged/exited_unknown` + `state_evidence` | E6（ralph-api 词表缺"真实进程退出 vs 成功交付 vs merge"三分）、E10（terminal reason 全集） | A 无法表达 R2 要求的分离语义 | 0.87 |
| D9 | API 安全基线 | A. 无认证（信任 loopback）；B. token；C. 同源会话+CSRF+Host/Origin 校验 | C：loopback bind + 会话 cookie + double-submit CSRF + Origin 白名单 | E1（trusted_local 先例=loopback 即信任主体）、R5（"保护 v1 写操作"）、E2（envelope auth 先例） | A 挡不住浏览器 DNS-rebinding/跨站表单；B 与"首接默认免配置"目标冲突 | 0.85 |
| D10 | 测试层级分工 | A. 全 Rust；B. Rust 只测 runtime、TS/FE 自测、跨层 contract 走 fixture JSON | B：Rust nextest；monitor `tsx --test`（node:test，同 E14 先例）；前端 vitest+playwright route-intercept | E15（playwright intercept 已有先例）、E17（nextest 规则）、E14（tsx test 先例） | A 无法覆盖 UI；contract 用 golden JSON fixture 成本低 | 0.88 |

**低于 0.85 的决策**：无。

### 实施协议（引用仓库先例 E22 的公共契约）

- **验收契约（Close）**：每个 Unit 关闭前必须满足 §7 该 Unit 第 18 项全部条件；Rust 侧 `cargo fmt --all -- --check` + `cargo clippy -p <crate> -- -D warnings` + targeted nextest 全绿；TS 侧 `npm run typecheck && npm test`（对应 workspace）；前端 `npm run build`（tsc -b 前置）。
- **停止契约（Stop）**：源码与 Evidence 冲突／接口不存在／Red 非预期失败／需未计划依赖／公开调用方新增／兼容策略失败／置信度跌破 0.85／回归面扩大／Unit 无法保原子 → 停止→记录新 Evidence→更新影响→重新比较→重算置信度→修订计划。禁止猜测继续。
- **测试子进程 env**：任何 spawn `ralph` 的测试先经 `common::scrub_agent_runtime_env`（E17）；TS 侧 fixture 同理（env allowlist 注入 `PATH`+`RALPH_MONITOR_*`，清除其余 `RALPH_*`）。
- **fixture 策略**：TS/前端集成与 e2e 用 **fake `ralph` 可执行文件**（shell/node 脚本置于临时 `PATH` 前），输出 `inspect run` 契约 JSON + 可写 `.ralph/runs/*.json`；不依赖付费/真实 LLM。
- **Rust 测试入口**：一律 `cargo nextest run` 系列（E17，HARD RULE 1）；doctest 允许 `cargo test --doc`。
- **CLI 参数核对**：新增/修改 CLI 参数必须 `ralph <cmd> --help` 核对并跑 `./scripts/check-cli-doc-drift.sh`。
- **prompt 处理**：manifest 存 `prompt_preview`（≤160 字符）+`prompt_sha256`；manifest/API 不返回全文 prompt（沿用 loops.json 存全文为内部细节，不向外暴露）。

## 4. BDD 行为规格

```gherkin
Feature: 跨项目运行观测基础

  Background:
    Given monitor 服务以默认配置启动
    And monitor 只绑定 127.0.0.1

  Scenario: S1 双项目同名运行按目录区分
    Given 项目 A 与项目 B 均已注册
    And A 与 B 各有一次 prompt 相同的 run
    When 采集器完成一轮采集
    Then /api/monitor/v1/runs 返回两条记录
    And 两条记录 global_run_id 不同且 project_id 分别指向 A/B
    And 每条记录携带 cwd/repo_root/worktree 证据字段

  Scenario: S2 注册项目路径规范化去重
    Given 目录 /repo/x 已注册
    When 用户通过符号链接别名 /link/x 再次注册同一物理目录
    Then 服务返回 409 冲突并指向既有 project id
    And 不产生第二个 project

  Scenario: S3 primary 与 worktree run 均被自动发现
    Given 项目 A 已注册
    And A 中存在一个 primary run 与一个 worktree loop
    When 采集器运行
    Then 两条 run 均出现且 kind 字段分别标识 primary/worktree
    And primary run 的 run_id 形如 primary-* 而非 "(primary)"

  Scenario: S4 短命运行通过 manifest 被发现
    Given 项目 A 已注册
    And 一次 run 启动后 5 秒内进程退出且写出 manifest 终态
    When 采集器运行
    Then 该 run 出现在列表中且状态为对应 exited_* 终态
    And 即使 registry 已剪除其条目仍然可见

  Scenario: S5 状态分离表达
    Given 项目 A 中有进程退出且 manifest terminal=completed 的 run X
    And 进程退出且 merge queue 记录 merged 的 run Y
    And 进程退出但无 manifest 终态的 run Z
    When 采集器运行
    Then X 状态为 exited_success、Y 为 exited_merged、Z 为 exited_unknown

  Scenario: S6 无终态证据不虚构成功
    Given run Z 的进程已死且不存在 manifest
    When 用户查看 Z
    Then 状态为 exited_unknown
    And detail 中展示证据时间戳（最后事件/最后观测）而非成功标记

  Scenario: S7 服务重启保留历史与游标
    Given monitor 已采集若干 run 且存在终态
    When monitor 进程重启
    Then 既有 runs 与终态不丢失
    And 采集从持久游标继续而非全量重扫

  Scenario: S8 非法与安全边界
    Given 不存在的目录路径 P
    When 用户注册 P
    Then 返回 4xx 且错误体含可操作提示（目录不存在/非 git 仓库提示）
    When 浏览器跨站 POST /api/monitor/v1/projects（无同源会话/CSRF）
    Then 请求被拒绝

  Scenario: S9 单项目采集失败不影响其他项目
    Given 项目 A 的 ralph 可执行缺失，项目 B 正常
    When 采集器运行
    Then B 的 runs 正常更新
    And A 的项目健康标记为采集失败含错误摘要
    And A 既有 runs 标记 stale 而非被清空

  Scenario: S10 总览筛选与最小详情
    Given 列表含多个项目不同 preset/状态的 run
    When 用户在总览按状态 exited_failed + 关键词搜索
    Then 仅匹配行可见
    And URL query 反映筛选状态
    When 用户选中一行
    Then 进入详情页显示身份、状态、证据时间、目录与 prompt 预览
```

## 5. 验收与测试策略

| Scenario | 验收条件（断言要点） | 测试入口 | 层级 | 风险补充 | E2E |
| ---- | ---- | ---- | ---- | ---- | ---- |
| S1 | 两条 run；global_run_id 唯一；project_id 正确 | `backend/ralph-monitor/test/integration/collector.test.ts` + `e2e/monitor-overview.spec.ts` | 集成+E2E | — | 是（fake ralph） |
| S2 | realpath 去重→409；alias 不产生第二行 | `test/unit/projects.test.ts` | 单元+HTTP 集成（fastify.inject） | — | 否 |
| S3 | primary/worktree 两 kind；run_id 非 "(primary)" | `crates/ralph-cli/tests/integration_inspect_run.rs` + collector 集成 | 集成 | Characterization（无 manifest 的旧 .ralph fixture） | 否 |
| S4 | 短命 run 以 manifest 发现且终态正确 | `integration_inspect_run.rs` + collector 集成 | 集成 | — | 否 |
| S5/S6 | 三态分离；unknown 展示证据时间 | `test/unit/state_machine.test.ts` + collector 集成 | 单元+集成 | Property（terminal reason → state 映射全集覆盖 E10 词表） | 否 |
| S7 | 重启后 runs/cursor 持久；无全量重扫（counter 断言） | `test/integration/persistence.test.ts` | 集成 | — | 否 |
| S8 | 4xx 可操作错误；跨站写被拒 | `test/unit/api_security.test.ts` + `test/integration/projects_api.test.ts` | 单元+集成 | Security（路径穿越/含 `..`/非目录/权限拒绝） | 否 |
| S9 | B 正常；A health=collector_error；A runs stale 不清空 | `test/integration/collector_isolation.test.ts` | 集成 | Fault injection（spawn 超时/非 0/坏 JSON/写盘失败） | 否 |
| S10 | 筛选/搜索/排序生效；URL 状态往返 | `frontend/.../features/monitor/*.test.tsx` + e2e | 组件+E2E | a11y（键盘选中行→详情） | 是 |

层级选择理由：状态映射与去重是纯规则→单元；采集是子进程+DB 协作→集成；总览交互→组件+1 条关键 E2E；安全边界→HTTP 集成（inject 即可，无需起真浏览器）。

## 6. 需求—测试追踪矩阵

| Requirement | Scenario | 验收测试 | 单元测试 | 集成/契约 | E2E | Evidence |
| ---- | ---- | ---- | ---- | ---- | ---- | ---- |
| R1 | S1/S3/S4 | collector 集成 + inspect_run 集成 | manifest schema 单测 | `inspect run` JSON contract fixture | monitor-overview | E4/E7/E8/E10 |
| R2 | S1/S5/S6 | state_machine 全词表单测 | identity/状态映射 | collector 集成 | monitor-overview | E5/E6/E10/E18 |
| R3 | S7 | persistence 集成 | migration/游标单测 | 重启场景集成 | — | E4/E18 |
| R4 | S10 | 组件测试 + e2e | filter/sort 纯函数 | — | monitor-overview | E13/E15 |
| R5 | S8/S9 | api_security + isolation | path 校验/错误类型 | fastify.inject 集成 | — | E1/E14 |

每个 Scenario 对应 Unit：S1→U3+U5；S2→U2；S3/S4→U1+U3；S5/S6→U1+U3；S7→U2+U3；S8→U2；S9→U3；S10→U4+U5。

## 7. 严格串行开发单元

```text
Unit 1（runtime 观测：run manifest + ralph inspect run JSON v1）
  ↓
Unit 2（monitor 服务骨架：workspace + SQLite + 项目注册 + 安全基线）
  ↓
Unit 3（采集器 + runs 投影 + /api/monitor/v1/runs 契约）
  ↓
Unit 4（前端设计系统 + monitor shell：tokens/主题/命令面板/api client/路由）
  ↓
Unit 5（总览页 + 最小详情页 + 浏览器 e2e）
```

### Unit 1：runtime 运行清单与 `ralph inspect run` JSON v1

#### 1. Unit 目标
任一方式启动的 `ralph run` 在 `.ralph/runs/<run_id>.manifest.json` 落盘身份证据，终态管线更新终态段；`ralph inspect run --format json` 输出聚合 primary+worktree 全部 run 的版本化观测 JSON。

#### 2. 对应需求与 Scenario
R1（部分：发现数据面）、R2（身份+终态证据）；S3/S4/S5/S6 的 runtime 侧；D1/D2/D3；E4/E5/E7/E8/E9/E10/E18。

#### 3. 外部可观察结果
`ralph inspect run --format json`（候选 flag 同 `inspect loop`）stdout 输出：

```json
{
  "schema_version": "run-observation/v1",
  "workspace": "<abs path>",
  "generated_at": "<rfc3339>",
  "runs": [
    {
      "run_id": "loop-…|primary-…",
      "kind": "primary|worktree",
      "state": "running|exited|unknown",
      "pid": 0,
      "cwd": "…", "workspace": "…", "repo_root": "…", "worktree_path": null,
      "prompt_preview": "…", "prompt_sha256": "…",
      "preset": "<builtin name|hats path|null>",
      "plan_path": null,
      "started_at": "…",
      "events_file": "…",
      "terminal": {"reason": "…", "at": "…", "merge_commit": null} | null,
      "manifest": ".ralph/runs/<id>.manifest.json",
      "evidence": {"manifest_present": true, "registry_present": false, "merge_state": "merged"}
    }
  ]
}
```

human 格式（默认 `--format human`）打印表格式列表。

#### 4. 当前行为基线
`inspect` 仅 `profiles/loop/prompt`（E8）；`inspect run` 不存在（预期 Red）。manifest 目录 `.ralph/runs/` 不存在（预期 Red）。run 启动只写 `loops.json`（worktree）或 `loop.lock`（primary）（E4/E6）。

#### 5. 输入与输出
输入：启动上下文（loop context/args：prompt、preset/hats source、plan path、worktree）；`inspect run [--root <path>] [--format json|human]`。输出：manifest 文件；JSON/human 输出；错误：`--root` 非 workspace→非零退出+stderr 提示。副作用：`.ralph/runs/` 目录创建；终态时 manifest `terminal` 字段更新。不变量：`EventRecord`/`loops.json`/`loop.lock`/`history.jsonl` 格式不变；不读 LLM。

#### 6. 修改位置
- `crates/ralph-core/src/run_manifest.rs`（计划新增，≤600 行目标 E24）：schema `RunManifest`（serde camelCase）、`write_start`、`write_terminal`、`read`、`discover_all(dir)`。
- `crates/ralph-core/src/lib.rs`：导出模块（按既有导出模式）。
- `crates/ralph-cli/src/loop_runner/loop_owner.rs`：worktree register 成功后写 manifest（E4 同址）。
- `crates/ralph-cli/src/commands/run.rs` + `loop_runner/run_impl.rs`：primary 路径写 manifest（primary 无 registry，落点在启动成功后、事件循环前；**进入 Unit 时以 `LoopContext`/`current-loop-id` 写入点为锚**，候选 run_impl.rs 中 lock 获取成功段）。
- `crates/ralph-cli/src/loop_runner/inner.rs` `handle_termination`（E10）：在 `hist.record_*` 同段调用 `run_manifest::write_terminal`，参数 = `TerminationReason::as_str()`（E10 词表全集）+ merge commit（CompletionPromise 分支已有 `git rev-parse HEAD` 值，E10）。
- `crates/ralph-cli/src/commands/inspect.rs`：`InspectCommands::Run` 子命令 + 分发（E8 同模式）。
- `crates/ralph-cli/src/commands/inspect/run.rs`（计划新增，候选位置；若 inspect.rs 为单文件模块结构则内联 `mod run`）：聚合 = 读 `.ralph/runs/*.manifest.json` ∪ registry ∪ loop.lock ∪ merge-queue，按 D8 计算 `state`/`evidence`，primary 显示名永不作为 run_id（E6）。
- `crates/ralph-core/src/event_loop/types.rs`：`TerminationReason::as_str` 已存在（E10）→ 直接复用，不修改。
- 测试：`crates/ralph-cli/tests/integration_inspect_run.rs`（计划新增）+ `crates/ralph-core/src/run_manifest.rs` 内 `#[cfg(test)]`。

#### 7. 可依赖能力
`LoopRegistry`/`LoopLock`/`MergeQueue`/`LoopHistory` 读取 API（E4/E5/E18）；`LoopContext::resolve_events_path`（E9）；`TerminationReason::as_str`/`is_success`（E10）；`common::ralph_bin`+`scrub_agent_runtime_env`（E17）；FAKE_PATH_BACKEND fixture 模式（`.cursor/rules` + 既有 `integration_run*.rs` 先例）。

#### 8. 禁止依赖的未来能力
monitor 服务、`presets/monitor/` descriptor、能力声明 actions（Plan 5）、durable seq/SSE（Plan 3）、通知（Plan 4）。manifest 不得包含 actions/health/notification 字段。

#### 9. 验收测试
- `integration_inspect_run.rs::run_start_writes_manifest_and_inspect_reports_running`：temp workspace，FAKE backend `ralph run` 后台启动→断言 `.ralph/runs/<id>.manifest.json` 存在字段齐全；`ralph inspect run --format json` 含该 run `state=running`。
- `integration_inspect_run.rs::termination_updates_manifest_terminal`：fake backend 完成→退出；`inspect run` 中 `terminal.reason=completed`。
- `integration_inspect_run.rs::primary_run_has_stable_id`：primary run 的 `run_id` 以 `primary-` 前缀且不为 `"(primary)"`。
- `integration_inspect_run.rs::legacy_workspace_without_manifest_reports_unknown`：手造 `.ralph/` 仅含死 PID loop.lock→`state=exited_unknown`-等价字段（`terminal=null,evidence.manifest_present=false`）。
- 运行命令：`cargo nextest run -p ralph-cli --test integration_inspect_run`（E17）。

#### 10. Acceptance Red
首次运行上条命令：编译错 `InspectCommands::Run` 不存在 / 子命令 `run` 未知（clap error exit 2）。有效 Red = "unrecognized subcommand"；无效 Red：fixture/env 错误、编译器版本错。

#### 11. 单元测试拆分
`run_manifest.rs` 内：① `write_start` 产出 schema_version/run_id/kind/pid/时间字段；② `write_terminal` 幂等（重复写不覆盖首个终态？——决策：终态只写一次，二次写忽略；测试断言）；③ `discover_all` 跳过坏 JSON 文件且计入 `evidence.manifest_corrupt`；④ prompt 预览截断 160 + sha256 与全文一致；⑤ `kind` 从 `worktree_path.is_some()` 推导。`inspect run` 聚合函数纯函数化（输入=文件视图 trait，输出=runs[]）：⑥ 无 manifest 无 registry → 空列表；⑦ manifest 在 registry 无（死 PID 已剪）→ `exited`+对应 terminal；⑧ merge_state=merged→terminal.merge_commit 回填；⑨ pid 活着但无 manifest→`running`(evidence.registry_only)；⑩ JSON 顶层含 `schema_version`。
不允许 Mock 的行为：真实 flock 语义用 tempdir 真实文件验证（不单测内 mock fs 掩盖并发）。

#### 12. Red → Green → Refactor 顺序
```text
test(①-⑤ manifest 单测) Red → 新模块最小实现 → Green
→ test(integration: start→manifest+inspect running) Red → CLI 子命令+启动挂点 → Green
→ test(integration: terminal) Red → handle_termination 挂点 → Green
→ test(unknown/legacy + merged 回填) Red → 聚合补逻辑 → Green
→ Refactor：聚合纯函数抽取、manifest schema 常量集中
```

#### 13. 最小实现范围
必须：manifest schema v1+读写+目录发现；三处写入挂点；`inspect run` 聚合+human/json；`unknown` 证据字段。必须不实现：事件流、健康分类（silent 判定在 Plan 3）、actions、通知；不改 EventRecord/history/loops.json 形状；不引入新 crate 依赖（serde/chrono/nix 已在 workspace）。

#### 14. 集成验证
`cargo nextest run -p ralph-cli --test integration_inspect_run`（fake backend 真实 spawn）；`cargo nextest run -p ralph-core -- run_manifest`；既有 `integration_run.rs`/`integration_loops_merge.rs` 回归（写 manifest 不得破坏 run/merge 路径）。

#### 15. 风险驱动测试
- Characterization：构造“旧格式 `.ralph/`”（无 runs/ 目录、仅 loop.lock+history）→ inspect 输出 unknown（覆盖 R2 兼容）。
- Fault injection：manifest 文件截断/坏 JSON→该 run `evidence.manifest_corrupt=true` 且不 panic。
- Idempotency：同一 run 重复 `write_terminal`（interrupted 后 cleanup 再次调用路径）→ 首个终态保留。

#### 16. 回归范围
`cargo nextest run -p ralph-cli --test integration_run`、`integration_loops_merge`、`integration_resume`、`integration_wave_inspect`（inspect 命名空间与 run 生命周期同域）；`cargo nextest run -p ralph-core -- event_logger loop_registry loop_history`；`./scripts/check-cli-doc-drift.sh`（新子命令文档漂移）；`scripts/audit-file-sizes.sh`（新增模块入审计）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `crates/ralph-core/src/run_manifest.rs` | 新增生产 | manifest schema+IO | D2/E10 |
| `crates/ralph-core/src/lib.rs` | 修改 | 模块导出 | — |
| `crates/ralph-cli/src/commands/inspect.rs` | 修改 | 子命令注册 | E8 |
| `crates/ralph-cli/src/commands/inspect/run.rs`（候选） | 新增生产 | 聚合实现 | E8 |
| `crates/ralph-cli/src/loop_runner/{run_impl.rs,loop_owner.rs}` | 修改 | 启动写 manifest | E4/E10 |
| `crates/ralph-cli/src/loop_runner/inner.rs` | 修改 | 终态写 manifest | E10 |
| `crates/ralph-cli/tests/integration_inspect_run.rs` | 新增测试 | ATDD | E17 |
| `docs/guide/cli-reference.md`（若 inspect 章节存在） | 修改 | doc drift | `check-cli-doc-drift.sh` |

#### 18. 完成标准
见实施协议验收契约 + 本节 S3/S4/S5/S6 runtime 断言绿；`cargo fmt --all -- --check`、`cargo clippy -p ralph-core -p ralph-cli -- -D warnings`、`cargo build -p ralph-cli` 通过；无 skip/.only/断言弱化；manifest 字段含 schema_version 供未来兼容。

#### 19. 停止条件
primary 写 manifest 的真实挂点与 Evidence 不符（如 primary 无 current-loop-id）→ 记录并改为在 lock 获取成功后由 run_impl 直接生成 `primary-<ts>-<rand>` id 写 marker+manifest；若发现 `inspect.rs` 为单文件且新增子命令引发 >600 行——拆模块再继续。其余按停止契约。

#### 20. 风险与注意事项
- 风险：crash/SIGKILL 时终态永不写入→依赖 monitor `unknown` 语义（设计使然，非缺陷）；检测=manifest `terminal=null` 且 pid 死。
- 风险：worktree 清理时 `.ralph/runs` 随 worktree 删除→monitor 侧已采集副本不受影响（projection 持久）；`inspect run` 须在 worktree 内仍可见时读到。
- 风险：同一 pid 复用→manifest 以 run_id 而非 pid 为键天然免疫（D1）。

---

### Unit 2：monitor 服务骨架 — workspace + SQLite + 项目注册 + 安全基线

#### 1. Unit 目标
`backend/ralph-monitor` 可独立启动（默认 `127.0.0.1:3200`），提供 `/api/monitor/v1/projects` CRUD：注册目录经 realpath 规范化去重、非法输入结构化 4xx、跨站写被拒、重启后数据在 SQLite 持久。

#### 2. 对应需求与 Scenario
R5 全部 + R3 的存储地基；S2/S8；D4/D5/D9；E1/E14/E23。

#### 3. 外部可观察结果
`POST /api/monitor/v1/projects {path}`→`201 {project{id,canonicalPath,displayName,createdAt}}`；alias→`409 {error{code:"project_exists",details{projectId}}}`；不存在路径→`422 {error{code:"invalid_path",message:"…不存在或不是目录"}}`；`GET`/`DELETE`；跨 Origin POST→`403`。服务重启后 GET 返回同集。

#### 4. 当前行为基线
`backend/ralph-monitor` 不存在（预期 Red）；root workspaces glob `backend/*` 自动纳入（E23）；`npm test` 根脚本会跑该 workspace `test`。

#### 5. 输入与输出
输入：HTTP JSON `{path}`；env `RALPH_MONITOR_HOST/PORT/DB_PATH`（默认 `127.0.0.1/3200/~/.ralph/monitor.db`，沿 E1 命名风格）。输出：项目资源 JSON；错误 `{error{code,message,details}}`。状态变化：`projects` 表 + `projection_meta` 表 + `machine_id` 生成持久。副作用：DB 文件创建+migration。不变量：只读写用户注册路径；服务不 spawn `ralph`（本 Unit）。

#### 6. 修改位置
计划新增（全部）：
- `backend/ralph-monitor/package.json`（`@ralph-monitor/server`；scripts：`dev`=`tsx src/server.ts`、`start`=`node --import tsx src/server.ts` 或 `tsx`、`test`=`tsx --test test/`、`build`=`tsc -b`、`typecheck`=`tsc -b --noEmit`？——与 frontend `tsc -b` 风格一致取 `tsc -p tsconfig.json --noEmit`；依赖 fastify@5、@fastify/cors、better-sqlite3、zod、tsx(开发)、typescript(开发)、@types/better-sqlite3(开发)——版本与 E14 对齐取 `^` 兼容范围）。
- `backend/ralph-monitor/tsconfig.json`（ES2022+NodeNext、strict，对齐 frontend/tsconfig 风格）。
- `src/server.ts`（入口+bind）、`src/app.ts`（fastify 实例工厂，测试可 inject）、`src/config.ts`（env 解析+zod）。
- `src/db/index.ts`（better-sqlite3 打开+PRAGMA journal_mode=WAL、foreign_keys=ON）、`src/db/migrations.ts`（migrations/0001_init.sql 顺序应用+`schema_migrations` 表）。
- `src/routes/projects.ts`（CRUD）、`src/lib/paths.ts`（realpath 规范化+校验：存在/目录/可读；符号链接解析；拒绝 `~` 展开歧义→服务端解析）、`src/lib/machine_id.ts`（meta 表生成 ULID/UUID——用 node:crypto `randomUUID`，不引依赖）。
- `src/plugins/security.ts`（Host/Origin 校验+会话 cookie 签发+CSRF double-submit 中间件；GET 免 CSRF，POST/PUT/DELETE 必查）。
- `test/unit/{paths,machine_id,security}.test.ts`、`test/integration/projects_api.test.ts`（fastify.inject）。
- `migrations/0001_init.sql`：`projects(id pk, canonical_path unique, display_name, created_at)`、`meta(key pk,value)`、`schema_migrations`；`runs`/`source_cursors`/`events`/`notifications`/`operations` 表在各自 Unit/Plan 内迁移文件新增（本 Unit 只建 0001）。
- root `package.json` scripts 加法：`dev:monitor`=`npm run dev -w @ralph-monitor/server`、`test:monitor`、`build:monitor`（E23）。

#### 7. 可依赖能力
fastify 5 inject 测试；better-sqlite3 同步 API；`node:test`+`tsx --test`（E14 既有模式）；root workspaces 通配。

#### 8. 禁止依赖的未来能力
不实现 runs/collector/SSE/通知/操作 API；不写 `runs` 表（留 U3 迁移）；不启动对 `ralph` 的子进程调用。

#### 9. 验收测试
- `projects_api.test.ts::register_normalizes_and_dedups`：realpath 别名→409+既有 id。
- `projects_api.test.ts::register_invalid_path_422`；`::nonexistent_dir`；`::file_not_dir`。
- `projects_api.test.ts::restart_persists`：关闭 app→新 app 实例同 db→GET 同集。
- `security.test.ts`：Origin≠Host 的 POST→403；无 CSRF header 的 DELETE→403；GET 免校验；Host 头白名单（127.0.0.1:3200/localhost:3200）。
- 运行：`npm test -w @ralph-monitor/server`。

#### 10. Acceptance Red
`npm test -w @ralph-monitor/server` → "Cannot find module"/无 workspace（有效 Red=workspace 不存在）；随后 projects_api 测试 Red=404 route 不存在。

#### 11. 单元测试拆分
`paths.ts`：realpath 解析、循环 symlink 报错、相对路径→绝对、尾斜杠归一、`..` 穿越被拒（含 `realpathSync.native`）。`machine_id`：首次生成+复用+DB 重开后不变。`security`：Origin/Host 矩阵（同源放行/跨源 403/null origin 非 GET 拒）、CSRF token 绑定会话。`migrations`：0001 幂等（重跑不报错）、未知版本拒绝启动。
不允许 mock：sqlite 用 `:memory:`+临时文件双模式真实验证持久。

#### 12. Red → Green → Refactor
```text
workspace 骨架可启动（health 200）
→ projects_api Red（404）→ routes+迁移实现 → Green
→ security Red → security 插件 → Green
→ persistence Red → （已是 sqlite）确认+补强 → Green
→ Refactor：错误体形制统一 `reply.code().send({error})` helper
```

#### 13. 最小实现范围
projects CRUD+规范化+安全基线+migration 框架+machine_id。不实现：runs、collector、SSE、auth 用户体系、HTTPS；CSRF 用 double-submit cookie（SameSite=Lax+HttpOnly 会话 id+自定义头 `x-monitor-csrf` 比对），不引第三方 CSRF 插件。

#### 14. 集成验证
`npm test -w @ralph-monitor/server`（inject 全覆盖）；手动 `npm run dev -w @ralph-monitor/server` + `curl -H 'Origin: http://evil' -X POST …/projects` 观察 403（记录在 Unit 完成证据）。

#### 15. 风险驱动测试
Security：路径穿越矩阵（`..`、编码 `%2e%2e`、UNC、`~`）；Idempotency：重复 POST 同路径→幂等 409 语义（明确选择 409 而非 200，记录理由）；Fuzz-lite：超长路径/非 UTF8 路径输入不崩溃。

#### 16. 回归范围
`npm test`（root，确认 workspace 纳入不拖挂既有 server/dashboard 测试）；`npm run build -w @ralph-monitor/server`；node24 `.nvmrc` 下安装验证（E23）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `backend/ralph-monitor/**` | 新增 | 新服务 | D4/E14/E23 |
| `package.json`（root） | 修改 | dev/test/build:monitor scripts | E23 |
| `package-lock.json` | 修改 | npm install 产物 | E23 |

#### 18. 完成标准
S2/S8 绿；`npm run typecheck -w @ralph-monitor/server`、`npm run build -w @ralph-monitor/server` 通过；root `npm test` 无新增失败；无 skip/.only；DB 默认路径不外漏进 git（`.gitignore` 无需改——DB 在 `~/.ralph/`）。

#### 19. 停止条件
better-sqlite3 在 node24 原生绑定不可用 → 评估 `node:sqlite`（node22+ 内置）替换并重算置信度；fastify inject 无法覆盖 Origin 校验→改起真端口测试。其余按停止契约。

#### 20. 风险与注意事项
- better-sqlite3 native build 失败率：缓解=CI/本地 `npm install` 验证步骤；残留=`node:sqlite` 备选已记录。
- 会话 CSRF 自实现的正确性：缓解=单元矩阵全覆盖；残留=无跨会话重放保护（v1 可接受，loopback-only）。
- `~` 展开歧义：决策=仅服务端 `os.homedir()` 显式展开 `~/` 前缀，其他 `~user` 拒绝。

---

### Unit 3：采集器 + runs 投影 + `/api/monitor/v1/runs` 契约

#### 1. Unit 目标
服务周期性对每个注册项目 spawn `ralph inspect run --format json`（参数数组、无 shell），将 snapshot 解析（zod）后**在同一 SQLite 事务**内更新 `runs` 投影与 `source_cursors`；`GET /api/monitor/v1/runs`（过滤/排序）与 `/runs/:id` 返回投影；采集失败逐项目隔离并标 `stale`；服务重启后从持久游标继续。

#### 2. 对应需求与 Scenario
R1/R2/R3 的 monitor 侧主体；S1/S4/S5/S6/S7/S9；D1/D3/D5/D7/D8；E3/E4/E5/E6/E10/E18。

#### 3. 外部可观察结果
`GET /api/monitor/v1/runs?project=<id>&state=<csv>&q=<kw>&sort=<-startedAt>&limit/offset`→`{runs:[…],total,watermark}`；`GET /runs/:globalId`→详情（含 `state_evidence`+`evidence_timestamps`）；`GET /api/monitor/v1/projects/:id/health`→`{collector:"ok|error",error?,last_collected_at}`。

#### 4. 当前行为基线
U2 仅 projects CRUD；runs 相关路由不存在（预期 Red）；`ralph inspect run` 由 U1 提供（测试用 fake `ralph` 可执行注入 `PATH`——实施协议 fixture 策略）。

#### 5. 输入与输出
输入：DB projects 行；`inspect run` JSON v1（zod schema `RunObservationV1`）。输出：runs API JSON；project health。状态变化：`runs`、`source_cursors`（last_collected_at、last_snapshot_sha256、consecutive_failures）、`projects.health`（派生）。副作用：每轮每项目一个子进程（timeout 10 s、kill）。不变量：partial snapshot 不提交；同 run_id 行幂等 upsert；`(primary)` 永不入库为 id。

#### 6. 修改位置
计划新增：
- `backend/ralph-monitor/migrations/0002_runs.sql`：`runs(global_run_id pk, project_id fk, run_id, kind, state, state_evidence json, cwd, repo_root, worktree_path, prompt_preview, prompt_sha256, preset, plan_path, started_at, terminal json, last_event_at, last_observed_at, manifest_path, snapshot_sha256)`、`source_cursors(project_id pk, last_collected_at, last_snapshot_sha256, consecutive_failures)`、`collection_errors(project_id, at, error)`（限幅保留）。
- `src/ralph/client.ts`：`inspectRun(cwd)` spawn(`ralph`, `['inspect','run','--format','json']`, {cwd, env: 允许表, timeout})→stdout JSON；错误分类 `{spawn_error|timeout|nonzero|bad_json}`。
- `src/collector/collector.ts`：逐项目串行循环（默认 3000 ms，env `RALPH_MONITOR_POLL_MS`）、每项目独立 try、事务 `ingest(project, snapshot)`。
- `src/db/run_projection.ts`：upsert/查询（filter/sort/paginate）、stale 标记（`stale = consecutive_failures>0` 或 watermark 老于阈值——仅返回字段不清空）。
- `src/routes/runs.ts`、`src/routes/health.ts`；`src/lib/state.ts`：`snapshot.state`→monitor 词表映射（D8）纯函数。
- 测试：`test/unit/{state,projection,parsers}.test.ts`、`test/integration/{collector,runs_api,persistence,collector_isolation}.test.ts`；`test/fixtures/fake-ralph/*`（fake ralph 脚本 + canned JSON）；`test/helpers.ts`（temp project dirs + PATH 注入 + env scrub）。

#### 7. 可依赖能力
U1 `inspect run` JSON 契约（fixture 先行）；U2 app/db/security；node `child_process.spawn` 参数数组；zod。

#### 8. 禁止依赖的未来能力
事件 ingest（Plan 2 byte-cursor 属 U3-Plan2 迁移）；SSE/健康面板（Plan 3）；actions（Plan 5）；通知（Plan 4）。本 Unit 不读 events.jsonl 内容——`last_event_at` 取 snapshot 提供字段。

#### 9. 验收测试
- `collector.test.ts::ingests_two_projects_distinct_runs`（S1）；`::short_lived_run_via_manifest`（S4）；`::states_map_three_way`（S5/S6：fixture 三组→exited_success/exited_merged/exited_unknown）。
- `persistence.test.ts::restart_resumes_without_rescan_loss`（S7）：kill app→新实例→runs 同集且 cursor 续进。
- `collector_isolation.test.ts::failing_project_does_not_block_others`（S9）：A 的 fake ralph exit1→A.health=error+B 正常+A.runs stale 保留。
- `runs_api.test.ts`：filters/sort/pagination/`total`/`watermark`；id 不存在→404 typed。
- 运行：`npm test -w @ralph-monitor/server`。

#### 10. Acceptance Red
`runs_api` 测试→404；`collector` 测试→模块不存在 import 错。有效 Red=路由/模块缺失；无效=fake ralph 脚本权限位未加（fixture 本身问题，须区分）。

#### 11. 单元测试拆分
`state.ts`：snapshot.state+terminal+evidence→`{state,state_evidence}` 全词表映射（含 unknown 分支矩阵）；`parsers.ts`：schema_version 不符→`unsupported_version` 错；缺字段→定位路径的错误；`projection.ts`：upsert 幂等（同 snapshot 两次→一行）、terminal 不可回退（exited_*→running 拒绝；记录 `state_conflict`）；`client.ts`：timeout 分类/非零 exit/stderr 摘要截断/env 只含允许表。
不允许 mock：spawn 用真 fake 脚本；事务用真 sqlite。

#### 12. Red → Green → Refactor
```text
parsers Red → zod schema+错误类型 → Green
→ projection Red → 表+upsert+stale → Green
→ client Red → spawn 封装 → Green
→ collector 集成 Red → 循环+事务 ingest → Green
→ runs_api Red → routes → Green
→ Refactor：ingest 事务函数抽出便于 Plan2/3 复用
```

#### 13. 最小实现范围
轮询采集+投影+三 API+隔离失败+游标持久+stale。不实现：SSE、增量事件、跨项目汇总统计、删除项目级联清理策略（删除项目→保留 runs 标 `project_deleted`？——决策：级联删除 runs+cursors，记录审计行；测试断言）。

#### 14. 集成验证
fake ralph 双项目 fixture 全流程；`inspect run` 契约 fixture 与 U1 真实输出 diff 校验（测试内 golden JSON fixture，差异→更新需注明行为原因）。

#### 15. 风险驱动测试
- Fault injection：timeout/kill/坏 JSON/巨型 JSON（行内 size cap 2 MB→`payload_too_large`）；DB 写失败→本轮游标不进位。
- Idempotency/并发：同 global_run_id 重放→单行；两项目同名 run_id（不同项目）→不互踩——按 D1 三段式：DB PK=`(project_id, run_id)`，对外 `global_run_id = machine_id:project_id:run_id`，detail 路由按三段匹配；测试断言两项目同名 `run_id` 共存且各行独立。
- Differential：projection 重建（清空重放 snapshot 历史？——v1 无存 snapshot 历史，跳过；改测"同 snapshot 重放不变"）。

#### 16. 回归范围
U2 全部测试；root `npm test`；`npm run build` 两 workspace（server+dashboard 不受影响）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `backend/ralph-monitor/migrations/0002_runs.sql` | 新增 | 投影存储 | D5 |
| `src/{ralph/client,collector/collector,db/run_projection,routes/runs,routes/health,lib/state}.ts` | 新增 | 采集与 API | D3/D7/D8 |
| `test/**` | 新增 | ATDD+风险测试 | §5 |
| `test/fixtures/fake-ralph/**` | 新增 fixture | 隔离 LLM/真 ralph | 实施协议 |

#### 18. 完成标准
S1/S4/S5/S6/S7/S9 绿；typecheck/build/test 全绿；`GET /runs` 响应含 `watermark` 供 Plan3 复用；无对未来能力的提前实现。

#### 19. 停止条件
`inspect run` 真实输出与 fixture 契约漂移→先修正 fixture 再继续（不得改断言适配）；spawn 在 Windows 无 `.cmd` shim→本计划仅承诺 POSIX（记录）；其余按停止契约。

#### 20. 风险与注意事项
- fake ralph 与真实 U1 输出漂移：缓解=golden fixture+U1 集成测试同 shape 断言。
- 轮询 3s × N 项目子进程开销：缓解=串行+timeout+`RALPH_MONITOR_POLL_MS` 可调；残留=大量项目(>50)未测（非目标）。
- run_id 仅 workspace 唯一→global id 三段式（已在 §15 修正决策）。

---

### Unit 4：前端设计系统扩展 + monitor shell

#### 1. Unit 目标
前端具备 monitor 所需视觉与交互地基：light/dark 主题 token、语义状态色、缺省 ui 组件（dialog/command/table 骨架/skeleton/toast）、`/monitor` 路由挂载+Sidebar 入口、monitor API client（typed fetch+react-query hooks）、命令面板可跳转运行/项目。

#### 2. 对应需求与 Scenario
R4 的地基；S10 部分（壳+路由）；D6；E13/E15。

#### 3. 外部可观察结果
`/monitor` 路由渲染空态总览（无 runs 引导注册）；主题切换暗↔亮全局生效且持久；命令面板（`⌘K`/`Ctrl+K`）列出项目与最近 runs（数据源暂为 mock hooks——**修正**：本 Unit 即接真 API，因 U3 已完成；命令面板搜 runs/projects 真数据）；组件库新增组件 Storyless 但有组件测试。

#### 4. 当前行为基线
routes 不含 `/monitor`（Red）；index.css 仅暗色 @theme（E15）；components/ui 仅 6 件（E13）；无 api client 层（`rpc/client.ts` 是 ralph-api RPC 客户端，非 monitor REST）。

#### 5. 输入与输出
输入：monitor REST（U3 契约）、用户主题选择。输出：新路由/组件/hook。副作用：localStorage theme 键 `ralph-monitor-theme`（沿用 `ralph-ui` persist 模式 zustand，E15）。不变量：既有路由/页面行为不变；`/rpc` proxy 保留；新增 `/api/monitor` proxy→`127.0.0.1:3200`。

#### 6. 修改位置
- `frontend/ralph-web/src/index.css`：`@theme` 增补语义 token（`--color-status-running/-success/-failed/-merged/-unknown/-stale`）+ `[data-theme="light"]`（或 `@media (prefers-color-scheme)`+class 覆盖——决策：class `.light`/`html[data-theme]` + 显式 toggle，默认 dark 保持现状）。
- `src/components/ui/`（新增，shadcn new-york 风格自实现，沿用现有 cva+clsx+tailwind-merge 模式）：`dialog.tsx`（@radix-ui/react-dialog **新依赖**——检查：package.json 目前只有 `@radix-ui/react-slot`；决策：加 `@radix-ui/react-dialog` 单依赖，证据=E15 已有 radix 先例）、`command.tsx`（自实现 combobox 模式，不引 cmdk？——决策：引 `cmdk`？权衡：自实现成本低且零依赖→选自实现 listbox+kbd 焦点管理，若复杂度过高则 stop 改 cmdk）、`table.tsx`、`skeleton.tsx`、`toast.tsx`（自实现 toaster 区域+queue）。
- `src/features/monitor/`（新增）：`api.ts`（fetch wrapper：`apiGet/apiPost` 带 CSRF 头（与 U2 契约对齐：从 cookie 读 token 回显 header）、typed schemas zod）、`hooks.ts`（useProjects/useRuns/useRunDetail query hooks，key 前缀 `["monitor",…]` 隔离既有 QUERY_NAMESPACE）、`theme.ts`（zustand persist theme store）、`MonitorLayout.tsx`（页面壳+topbar 插槽）。
- `src/App.tsx`：`/monitor`、`/monitor/runs/:globalId`（占位详情 U5 填充——本 Unit 详情页渲染"最小骨架"占位组件）。
- `src/components/layout/Sidebar.tsx`：NavItem `Monitor` 加在 Tasks 后。
- `vite.config.ts`：proxy `/api/monitor`→`http://127.0.0.1:3200`（保留 ws:false）。
- `src/test/setup.ts`：必要时加 matchMedia stub（jsdom 无其实现）。
- 测试：`src/features/monitor/{api,hooks}.test.ts`、`src/components/ui/{dialog,command,table,toast}.test.tsx`、`src/features/monitor/MonitorLayout.test.tsx`（路由+空态+主题切换）。

#### 7. 可依赖能力
react-query/router/zustand/lucide/cva 全部已在依赖中；U3 REST 契约；既有 ui 组件模式（badge/button/card 参考写法）。

#### 8. 禁止依赖的未来能力
详情页内容（U5）、时间线/图（Plan2）、SSE/健康 UI（Plan3）、inbox/actions（Plan4/5）。命令面板本版仅搜索 projects+runs（无 action 命令）。

#### 9. 验收测试
- `MonitorLayout.test.tsx`：导航 Monitor→路由 `/monitor`；无 runs→空态引导文案+“注册项目”动作；主题 toggle→`documentElement` class 切换+localStorage。
- `command.test.tsx`：Ctrl+K 开→输入筛选→Enter 导航；Esc 关；焦点归还触发元素。
- `api.test.ts`：CSRF 头注入、错误体解包、404 typed。
- `hooks.test.ts`：useRuns 缓存键含 filter；refetchInterval 3s 与 collector 对齐。
- `dialog/table/skeleton/toast` 组件测试（键盘关闭、行渲染、aria-busy、toast 自动消失）。
- 运行：`npm test -w @ralph-web/dashboard`。

#### 10. Acceptance Red
新测试文件 import `@/features/monitor/...`→resolve 失败；`/monitor` 路由→404/No routes matched。有效 Red=模块/路由缺失。

#### 11. 单元测试拆分
逐组件行为如上；`theme.ts`：初始化 dark 默认/persist 恢复/系统偏好只在无存储时生效；`api.ts`：envelope 解包/错误映射/baseURL 注入可配。
不允许 mock：`fetch` 用 MSW？——决策：不引 msw，用 `vi.stubGlobal('fetch')` 按请求断言（先例：现有测试怎么打？无网络测试先例→自选 stub，记录）。

#### 12. Red → Green → Refactor
```text
ui 组件测试逐个 Red→Green（dialog→command→table→skeleton→toast）
→ theme/api/hooks Red→Green
→ MonitorLayout+路由+Sidebar Red→Green
→ Refactor：tokens 抽取、api error 类型统一
```

#### 13. 最小实现范围
tokens/主题/五组件/monitor api+hooks/壳路由/nav/proxy/命令面板（projects+runs 搜索跳转）。不实现：总览表格内容（U5）、详情内容（U5）、任何写操作 UI、SSE。

#### 14. 集成验证
`npm test -w @ralph-web/dashboard`；`npm run build -w @ralph-web/dashboard`（tsc -b+vite）；起 dev server 手动 `/monitor` 截图记录（视觉初检）。

#### 15. 风险驱动测试
a11y：命令面板/对话框键盘全覆盖+focus trap+Esc；主题：FOUC 检查（html 初始 class 同步注入 index.html 小脚本——决策：在 `index.html` 加 3 行同步 script 读 localStorage 设 `data-theme`）；Contract：api.ts 对 U3 错误体形状 golden。

#### 16. 回归范围
`App.test.tsx`（路由断言需更新）、既有 5 页面 smoke、e2e 两个既有 spec 不被 proxy 改动破坏（`npm exec playwright test` 于 frontend workspace）。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `src/index.css` | 修改 | 主题+语义 token | E15 |
| `src/components/ui/{dialog,command,table,skeleton,toast}.tsx` | 新增 | 缺失组件 | E13 |
| `src/features/monitor/**` | 新增 | 壳+api+hooks+theme | D6 |
| `src/App.tsx` / `Sidebar.tsx` / `vite.config.ts` / `index.html` | 修改 | 路由/nav/proxy/FOUC | E13/E15 |
| `package.json`（frontend） | 修改 | +@radix-ui/react-dialog | E15 |
| 新增测试文件 | 新增 | ATDD | §5 |

#### 18. 完成标准
组件+shell 测试绿；`npm test`/`build`/`typecheck` 绿；主题切换持久无 FOUC；既有路由回归绿；e2e 既有 spec 不受影响。

#### 19. 停止条件
命令面板自实现焦点管理超 ~150 行仍不完整→stop 决策换 `cmdk` 依赖重算；radix dialog 版本与 react19 冲突→降版本或自实现。

#### 20. 风险与注意事项
- Tailwind v4 @theme 变量在 light 下的对比度：逐 token WCAG AA 抽查（文本 4.5:1）；记录检查表。
- `data-theme` 与既有 dark-only 组件硬编码色：grep `zinc-`/具体色值清单→改为 token（限 monitor 新组件强制，旧组件不扩面——记录残留）。

---

### Unit 5：总览页 + 最小运行详情 + 浏览器 e2e

#### 1. Unit 目标
`/monitor` 总览页提供 runs 表格（项目/preset/状态/时间列）、搜索、筛选、排序、选中行→`/monitor/runs/:globalId` 最小详情（身份、状态徽章、证据时间戳、cwd/repo/worktree、prompt 预览）；URL query 承载筛选状态可往返；双项目 e2e 通过 fake ralph 后端真实全栈验证。

#### 2. 对应需求与 Scenario
R4 全部 + R1/R2 的用户面收尾；S1/S5/S6/S10；D6/D8；E13/E15。

#### 3. 外部可观察结果
表格列：状态徽章、run_id（短显）、项目、preset、开始时间、证据时间；工具栏：搜索框、项目 select、状态多选、preset select、排序列头点击；空态/采集失败 stale 标记（行内 badge+顶部横幅按 project health）；详情页：标题 run_id、状态+state_evidence 摘要、字段表（cwd/repo_root/worktree/kind/preset/plan/prompt_preview）、证据时间戳（last_event_at/last_observed_at/terminal.at）、返回链接保留筛选 URL。

#### 4. 当前行为基线
U4 提供壳/空态/api hooks/命令面板；表格与详情为空（预期 Red）。

#### 5. 输入与输出
输入：`GET /runs` 查询参数态（URL `?q&project&state&preset&sort`）；`GET /runs/:id`。输出：表格/详情渲染；浏览器历史。副作用：无写操作。不变量：刷新/前进后退恢复视图；500 行内虚拟化非必需（决策：不引虚拟化，limit/offset 分页 50/页——R4 仅要求排序筛选浏览）。

#### 6. 修改位置
- `src/features/monitor/OverviewPage.tsx`（新增）：工具栏+表格+分页。
- `src/features/monitor/RunDetailPage.tsx`（新增）：本 Unit“最小详情”版（字段表+证据时间+返回）。Plan2 将替换为 tab 版——本 Unit 只实现占位级内容但**真实数据**。
- `src/features/monitor/components/{RunStateBadge,StaleBanner,FilterBar,Pagination}.tsx`（新增）。
- `src/features/monitor/hooks.ts`：useRuns 参数化（读 URL searchParams）。
- `e2e/monitor-overview.spec.ts`（新增；route-intercept `**/api/monitor/v1/runs*`，或真 monitor+fake ralph——决策：e2e 用真 fake-ralph 双项目 fixture 起 monitor 服务（playwright webServer 启动 `npm run dev -w @ralph-monitor/server`+`vite dev`？成本权衡：route-intercept 与既有 builder.spec 模式一致且快；**决策：overview e2e 用 route-intercept（同 E15 先例），全栈 fake-ralph e2e 留给 Plan5 U5 journey spec**，因 Plan5 明确要双项目真实 journey——记录此选择避免重复）。
- 测试：`OverviewPage.test.tsx`（筛选/搜索/排序/URL/选中/空态/stale）、`RunDetailPage.test.tsx`（unknown 证据时间显示、字段齐全、返回保留 query）。

#### 7. 可依赖能力
U4 hooks/api/ui 组件；U3 REST 全部参数。

#### 8. 禁止依赖的未来能力
事件时间线/日志/图/artifacts（Plan2）；健康面板/SSE（Plan3）；通知跳转（Plan4）；actions（Plan5）。详情页不渲染 tabs（Plan2 加）。

#### 9. 验收测试
- 组件层如上；e2e：`monitor-overview.spec.ts::filters_via_url_and_opens_detail`——拦截 runs 响应（双项目三态 fixture）→筛选 failed→表格收敛→点击行→详情断言状态+证据时间→back→筛选仍生效。
- 运行：`npm test -w @ralph-web/dashboard`；`npm exec playwright test -w @ralph-web/dashboard`（无 -w 语义？playwright 在 frontend dir 运行：`cd frontend/ralph-web && npx playwright test`——写进命令清单）。

#### 10. Acceptance Red
组件测试 import 缺失；e2e `getByRole('table')` 无匹配（页面仅壳）。有效 Red=元素不存在。

#### 11. 单元测试拆分
FilterBar 受控状态↔URL 双向；排序映射（`sort=-startedAt`）；状态徽章词表全色映射；unknown 详情必含证据时间；stale banner 触发条件（health=error）；分页边界。
不允许 mock 掉 URL 同步逻辑（用 MemoryRouter+真 history）。

#### 12. Red → Green → Refactor
```text
Badge/FilterBar/Pagination 组件 Red→Green
→ OverviewPage Red→Green
→ RunDetailPage Red→Green
→ e2e Red→Green
→ Refactor：URL state codec 抽出
```

#### 13. 最小实现范围
表格+筛选+搜索+排序+分页+详情+URL 状态+stale 显示+e2e。不实现：虚拟滚动、列定制、批量操作、详情 tabs。

#### 14. 集成验证
vitest 全量；playwright overview spec；`npm run build`；与真 monitor 手动联调（dev server 起 U3 服务+fake ralph 单项目，截图记录）。

#### 15. 风险驱动测试
a11y：表格键盘导航+行激活；E2E route-intercept 验证请求 URL 参数正确性（assert intercepted query）；视觉快照不引入（决策：Playwright screenshot diff 不纳入——记录理由：主题 token 初建易 churn，改走人工截图记录）。

#### 16. 回归范围
App.test 路由表；既有 e2e；`npm test` 根。

#### 17. 预期文件变更
| 位置 | 类型 | 原因 | Evidence |
| ---- | ---- | ---- | ---- |
| `features/monitor/{OverviewPage,RunDetailPage}.tsx` | 新增 | R4 | E13 |
| `features/monitor/components/*` | 新增 | 表格部件 | — |
| `features/monitor/hooks.ts` | 修改 | URL 参数化 | — |
| `e2e/monitor-overview.spec.ts` | 新增 | ATDD e2e | E15 |

#### 18. 完成标准
S10+S1/S5/S6 用户面绿；全部测试/build/typecheck 绿；URL 往返正确；详情 unknown 有证据时间；可独立提交。

#### 19. 停止条件
runs API 参数与表格需求不符（缺 facet 计数）→stop：决策加 `/runs/facets` 或前端本地 facet（选本地：数据量小，记录）。其余按停止契约。

#### 20. 风险与注意事项
- URL 状态 codec 与 react-router 版本行为：锁定 `useSearchParams` 用法并测试。
- 大列表性能：分页 50 行默认；记录 >500 runs 未实测（非目标）。
- e2e 稳定性：route-intercept 无网络依赖；playwright webServer 字段已配 vite dev（E15）。

## 8. Unit 串行依赖图

```text
Unit 1 → Unit 2 → Unit 3 → Unit 4 → Unit 5
```

- U2 依赖 U1？**不依赖代码产物**（U2 不 spawn ralph），但顺序固定为先 U1——理由：契约 fixture 由 U1 定型；U2 可在 U1 未完成时开发但本计划要求严格串行，且 U3 的 golden fixture 必须对齐 U1 真实输出。
- U3 依赖 U1 的 `inspect run` 契约（fixture）+U2 的 app/db/security。
- U4 依赖 U3 的 REST 契约（api.ts typed client）。
- U5 依赖 U4 组件/壳/路由与 U3 API。
- 交换顺序为何不可：U3 前无 U1→无契约；U5 前无 U4→无组件。

## 9. 执行命令清单

```bash
# Rust（HARD RULE：全部 nextest；测试子进程 env 由 common helper 处理）
C1  cargo nextest run -p ralph-cli --test integration_inspect_run     # U1 targeted
C2  cargo nextest run -p ralph-core -- run_manifest                   # U1 单测
C3  cargo nextest run -p ralph-cli --test integration_run             # 回归
C4  cargo nextest run -p ralph-cli --test integration_loops_merge     # 回归
C5  cargo nextest run -p ralph-cli --test integration_resume          # 回归
C6  cargo nextest run -p ralph-cli --test integration_wave_inspect    # inspect 命名空间回归
C7  cargo nextest run -p ralph-core -- event_logger loop_registry loop_history  # 相邻回归
C8  ./scripts/check-cli-doc-drift.sh                                  # 新子命令 drift
C9  ./scripts/audit-file-sizes.sh                                     # 规模审计
C10 cargo fmt --all -- --check
C11 cargo clippy -p ralph-core -p ralph-cli -- -D warnings
C12 cargo build -p ralph-cli

# monitor 服务
C13 npm install                                                       # workspace 纳入+依赖
C14 npm run dev -w @ralph-monitor/server                              # 手动启动（默认 127.0.0.1:3200）
C15 npm test -w @ralph-monitor/server                                 # tsx --test 全量
C16 npm run typecheck -w @ralph-monitor/server
C17 npm run build -w @ralph-monitor/server

# 前端
C18 npm test -w @ralph-web/dashboard                                  # vitest
C19 npm run build -w @ralph-web/dashboard                             # tsc -b && vite build
C20 npm run dev -w @ralph-web/dashboard                               # dev server
C21 cd frontend/ralph-web && npx playwright test                      # e2e（全部）
C22 cd frontend/ralph-web && npx playwright test e2e/monitor-overview.spec.ts

# 汇总门禁
B1  npm test                                                          # root：全部 workspace
B2  npm run build                                                     # server+dashboard+monitor
B3  ./scripts/run-tests.sh                                            # Rust 全量两阶段（最终）
B4  git status --short && git diff --stat                             # 变更面审查
```

失败即停：任一命令失败不得进入下一步（E22 契约）。

## 10. 最终质量门禁

- [ ] S1–S10 全部通过；
- [ ] R1–R5 均有测试覆盖（§6 矩阵无空洞）；
- [ ] Rust：`cargo fmt --check`+`clippy -D warnings`+目标 nextest+`./scripts/run-tests.sh` 全绿；
- [ ] TS：`typecheck`+`test`+`build`（monitor）绿；前端 `vitest`+`tsc -b && vite build`+playwright 绿；
- [ ] `check-cli-doc-drift.sh` 绿；`audit-file-sizes.sh` 无超限新增；
- [ ] 幂等（重复 ingest/重复 terminal 写）、Fault injection（spawn 超时/坏 JSON/DB 失败）、Characterization（无 manifest 旧 workspace）测试存在且通过；
- [ ] 无新增 skip/.only；无未解释 snapshot 更新；无断言弱化；
- [ ] 安全基线验证记录（Origin/Host/CSRF 矩阵 curl 实测）；
- [ ] 决策置信度复核 ≥0.85；Evidence Ledger 增补实现期新发现；
- [ ] 变更不超 §2.3 范围；每个 Unit 完成闭环且可独立提交；
- [ ] `docs/guide/monitor.md` 占位不在本计划（Plan5 统一写）；`.ralph/runs/` 未入库。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
| ---- | ---- | ---- |
| 这是实施计划而不是 Roadmap | 是 | 每 Unit 有真实 Red/修改位置/命令/完成标准 |
| Executor 是否仍需做关键设计决策 | 否 | D1–D10 全部定案；候选位置均已标注验证动作 |
| 所有文件和接口是否有代码库证据 | 是 | 既有文件全部 E 引用；新增文件显式标注 |
| 所有关键决策置信度 ≥0.85 | 是 | 最低 D9=0.85 |
| 是否存在未处理的低置信度假设 | 否 | — |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 观测写盘+查询；U2 项目注册；U3 采集+API；U4 壳+主题；U5 总览+详情 |
| 每个 Unit 是否可独立验证 | 是 | §9 命令逐 Unit 对应 |
| 每个 Unit 是否有真实 Red | 是 | 第 10 项均指明失败形态 |
| 每个 Unit 是否包含回归范围 | 是 | 第 16 项 |
| 是否存在未来 Unit 依赖 | 否 | U3 仅依赖 U1 契约 fixture（可先于服务真机集成） |
| 是否存在泛化任务描述 | 否 | — |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | §6 |
| 所有关键决策是否有 Evidence | 是 | §3 |
| 计划是否可以严格串行执行 | 是 | §8 |

---

**Series**: `feat/monitor-product` — Plan 2 `monitor-preset-run-details`、Plan 3 `monitor-health-recovery`、Plan 4 `monitor-notifications-inbox`、Plan 5 `monitor-control-delivery` 依赖本计划交付的 run manifest、`inspect run` 契约、`backend/ralph-monitor` 骨架、`features/monitor` 前端地基与术语表。
