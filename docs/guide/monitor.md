# Ralph Monitor

`ralph-monitor` 是跨项目运行观测工作台：把多个 `ralph run` 项目注册到一个
服务里，统一看运行状态、健康度、事件/活动/进度/DAG、产物、通知与受控操作。

## 安装

```bash
npm install                    # 根目录安装全部 workspace 依赖
npm run build:web              # 构建仪表盘产物 frontend/ralph-web/dist
npm run build:monitor          # 编译 monitor（可选，tsx 可直接跑 TS）
```

## 启动

```bash
ralph web --monitor            # 推荐：单进程（API + 静态托管），默认 :3200
ralph web --monitor --no-open  # 不自动打开浏览器
npm run dev:monitor            # 开发模式（tsx 直跑 src）
npm run start:monitor          # 生产入口
```

`ralph web --monitor` 会把 dashboard 端口设为 `--backend-port`（默认 3200），
并把 `RALPH_MONITOR_STATIC_DIR` 指向 `frontend/ralph-web/dist`；`--frontend-port`
在 monitor 模式下被忽略（会有提示）。Ctrl+C 优雅关停子进程。

## 注册项目

仪表盘首页「Add project」填入项目根路径（必须是 git 仓库），或：

```bash
curl -X POST http://127.0.0.1:3200/api/monitor/v1/projects \
  -H 'content-type: application/json' -d '{"path": "/abs/path/to/project"}'
```

注册后 collector 每 `RALPH_MONITOR_POLL_MS`（默认 3000ms）轮询一次
`ralph inspect run --format json`，把快照投影进 SQLite 并通过 SSE 推送变更。

## 安全模型

- **仅回环**：只接受 `Host: localhost / 127.0.0.1 / ::1`；非回环绑定被拒绝。
- **Origin 校验**：非 GET 方法要求同源 Origin。
- **双提交 CSRF**：`GET /api/monitor/v1/health` 下发 `monitor_sid` +
  `monitor_csrf` cookie；写请求须带 `x-monitor-csrf` 头。
- **无鉴权层**：不要把端口暴露到非回环接口。

## 操作（Operations）

运行详情页的 Actions 菜单由 runtime 声明的 `actions[]` 能力驱动（stop /
retry / merge / emit），非硬编码。每次操作需 idempotency key，支持：

- 幂等重放（同 key 返回原 operation）
- 每 run 并发保护（同一时间最多一个非终态 operation）
- `expected_revision` 乐观锁（能力快照过期返回 revision_conflict）
- `confirm_timeout` → 手动 recheck
- 全程审计：intent、dispatch 收据、确认状态均落库

「启动运行」走 `ralph run --client-request-id <uuid>`，launch 收据通过
manifest 的 `client_request_id` 匹配回对应 operation。

## Webhook 通知

通知规则（`notification_rules`）与渠道（`notification_channels`）存于
monitor DB；通用 webhook 经 SSRF 校验 + HMAC 签名投递，支持重试与退避。
项目根 `.ralph/ralph.yml` 中的 webhook 配置会在 `/projects` 响应里给出
`runtimeWebhookHint`。

## 环境变量

| 变量 | 默认 | 说明 |
|---|---|---|
| `RALPH_MONITOR_HOST` | `127.0.0.1` | 绑定地址（仅回环） |
| `RALPH_MONITOR_PORT` | `3200` | 监听端口 |
| `RALPH_MONITOR_DB_PATH` | `~/.ralph/monitor.db` | SQLite 路径（`:memory:` 可测试） |
| `RALPH_MONITOR_POLL_MS` | `3000` | collector 轮询间隔 |
| `RALPH_MONITOR_STATIC_DIR` | `frontend/ralph-web/dist` | SPA 产物目录；缺失时退化为 API-only + 提示页 |

## 数据库迁移

启动时自动应用 `backend/ralph-monitor/migrations/*.sql`（按文件名序，
`schema_migrations` 记录已应用版本）。当前至 `0008_operations`。

## SSE

`GET /api/monitor/v1/stream` 推送 `run.*` / `project.*` /
`notification.*` / `operation.updated` 事件；`?cursor=` 支持断线回放，
`/snapshot` 提供全量重同步。

## 端到端验收（Playwright）

`frontend/ralph-web` 下的 `e2e/monitor-journey.spec.ts` 用真实 monitor
进程 + fake-ralph fixture 项目跑完整旅程：注册两个项目 → 概览区分历史
run → 四步 LaunchDialog 启动 → run 详情 + 四层健康条 → stop 确认 →
operations 表 `已确认` → 通知深链回详情 → 杀掉 monitor 子进程由
supervisor 重拉 → 断连降级并恢复一致。运行：

```bash
cd frontend/ralph-web
npm run test:e2e                      # 全部 spec
npx playwright test e2e/monitor-journey.spec.ts   # 仅本旅程
```

helper（`e2e/helpers/monitor-server.ts`）在 `MONITOR_E2E_PORT`
（默认 3310）起真 monitor，fixture/临时库落在
`e2e/helpers/.tmp-monitor/`。截图产物写到
`e2e/artifacts/monitor-journey/`（不入库，随每次运行重生成）：

- `overview.png` — 1280×800 双项目概览
- `run-detail.png` — run 详情 + collector/runtime/backend 健康层
- `reconnected.png` — monitor 重启重同步后的详情页
- `viewport-375.png` — 375×812 移动视口概览
- `theme-current.png` / `theme-toggled.png` — 暗/亮主题对照
- `keyboard.png` — 纯键盘 Tab 焦点流（首焦落在交互元素上）
- `reduced-motion.png` — `prefers-reduced-motion` 模拟

截图只作人工审查记录，不做像素级硬断言；验收以语义等待
（`expect.poll` / `toBeVisible`）为准。

已知限制：`/api/monitor/v1/runs/:globalId` 的三段式 gid 长度可超过
find-my-way 默认 `maxParamLength`（100）；monitor 在 `buildApp` 中显式
提升到 512，过长 gid 不会被 SPA 通配兜底吞掉。
