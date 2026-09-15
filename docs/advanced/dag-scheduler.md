# DAG 调度器操作手册

本文是 `event_loop.supervisor.scheduler_mode` 的 operator 向使用文档：`dag_shadow` 验证流程与 `dag` 模式的日常操作。设计动机与选型论证见[调度模式设计说明](../explanation/scheduler-modes.md)。

## 模式速览

| `scheduler_mode` | 语义 |
|---|---|
| `wave`（默认） | legacy `WaveTracker` 执行面：dispatcher wave fan-out + supervisor wave 账本 + `forge.wave.*` 事件链 |
| `dag_shadow` | legacy 路径照常执行，DAG 调度器旁路观察（dry-run：无 DB 写、无投影副作用），用于 cutover 窗口验证 |
| `dag` | 声明运行时自有 work-conserving DAG 调度器：runtime 负责 per-Unit admission、job launch/recovery、integration、task-close acknowledgement 与 exactly-once `forge.exec.development.done` |

## 启用方式

### 配置示例

在 preset 或 `ralph.yml` 的 `event_loop` 段配置（以 builtin `presets/en/parallel-forge.yml` 为实际范例）：

```yaml
event_loop:
  execution_mode: isolated
  supervisor:
    enabled: true
    db_path: .ralph/supervisor.db
    scheduler_mode: dag_shadow   # 或 dag；缺省 wave
```

### fail-closed 校验

`dag_shadow` / `dag` 必须同时满足 `event_loop.supervisor.enabled: true` **且** `event_loop.execution_mode: isolated`。不满足的组合在 `ralph preset check` / preflight / `ralph run` 启动即被拒绝，错误信息含字段路径 `event_loop.supervisor.scheduler_mode`，不会静默降级回 `wave`。`wave` 与任意组合恒合法。

启动前静态校验：

```bash
ralph preset check -c ralph.yml -H builtin:parallel-forge --strict
```

## dag_shadow 验证流程

### 什么场景开 shadow

在把某个 preset 从 `wave` 切到 `dag` 之前的观察窗口：业务仍由 legacy wave 路径执行（结果与切换前完全一致），DAG 调度器在旁路跑 dry-run，operator 通过 inspect 观察「如果切到 dag，调度器会放行/阻塞什么」。shadow 不产生任何 DB 写入或投影副作用，可以随时切回 `wave`。

### 观察窗口期做什么

1. 按上文配置把 `scheduler_mode` 改为 `dag_shadow`，正常启动 loop。
2. 运行期间用 inspect 读取 scheduler 块：

```bash
ralph inspect loop --format json | jq '.scheduler'
```

`scheduler_mode` 不是 `wave` 时输出会多一个脱敏的 `scheduler` 块（`wave` 模式无该键，v2 JSON shape 不变）。字段含义（以 `crates/ralph-core/src/supervisor/dag_inspect.rs` 的 `SchedulerInspectSummary` 为准）：

| 字段 | 含义 |
|---|---|
| `scheduler_mode` | 当前配置的调度模式字符串（`dag_shadow` / `dag`） |
| `plan_keys` | 已观测到的去重排序后的 plan key 列表 |
| `oldest_observation_ms` | 最早一次观测的时间戳（毫秒）；无任何观测时该键缺省 |
| `total_observations` | 观测/记录总数 |
| `admitted_total` | 已放行计数 |
| `blocked_total` | 被阻塞计数 |

inspect 是只读入口：不会写 shadow sink、不会改 receipt registry、不会 spawn 任何后端。

### 如何解读 admitted / blocked

注意 admitted/blocked 是 **plan 收据（receipt）粒度**，不是 Unit/job 粒度：

- 在 `dag` 模式下，计数来自 `.ralph/dag.db` 的 receipt 行：`admitted_total` = 已到达 `Active`（或 admission 后终态 `Consumed`）的 receipt 数；`blocked_total` = 仍为 `Pending`（已登记但尚未被 `forge.concurrency.approved` 激活）的 receipt 数。`blocked_total` 非零通常意味着 planner 已提交执行计划、guardian 尚未放行。
- 在 `dag_shadow` 模式下，shadow 观测在进程内按 tick 记录候选被放行还是被依赖/资源/并发上限三类原因阻塞，但这些计数**不会**跨进程暴露（见下方已知限制）。

该入口目前不能区分「正在工作」「因资源等待」「恢复后已 blocked」等 Unit/job 级运行态；需要这些事实时结合 `ralph tools task list` 与 loop 日志判断。

### 已知限制（F11）

以下来自 2026-09-09 parallel-forge DAG 红队评审 F11，写排障结论前务必对照：

- shadow sink 的观测**只存在于 loop 进程内存**，另一个 `ralph inspect` 进程读不到它。
- 计数**跨重启/跨进程不可累积**：loop 重启后从零开始。
- 无 DB 时返回**空计数**：`dag_shadow` 永不打开 `.ralph/dag.db`，因此 inspect 的 scheduler 块在 shadow 模式下恒为零计数（仅 `scheduler_mode` 标签正确）。零计数不代表「没有观测」，只代表「没有可跨进程读取的持久记录」。
- 反过来，存在旧 DAG DB 时，inspect 会读取其中的旧 receipt 行并贴上**当前** mode 标签——切换模式前若不复位账本，读数可能混有历史数据。

## dag 模式 operator 面

### `.ralph/dag.db` 位置与懒打开

DAG 调度器拥有自己的 SQLite 账本，默认路径 `<repo_root>/.ralph/dag.db`，可用环境变量 `RALPH_DAG_STORE_PATH`（绝对路径）覆盖。它与 wave 账本 `.ralph/supervisor.db` 刻意分离：`supervisor.db` 仍是 wave 执行面的唯一权威，DAG 的表都在自己的文件里。

文件是**懒打开**的：仅在第一个相关 `forge.*` 事件被接受时才创建。一个从未见到 DAG seam 事件的 `dag` 模式 run 不会留下 `dag.db`，`.ralph` 目录文件清单与 `wave` run 完全一致。`dag_shadow` 模式永不打开该文件（全部观测在内存中）。

### SQL migrations v19–v23

DAG store 的表结构由 migration 管理；新安装自动迁移，旧库用 `ralph loops clean --ledger` 配合 migration runner 升级。各表一句话职责：

| migration | 表 | 职责 |
|---|---|---|
| v19 | `dag_checkout_intents` | 目标分支 checkout 意图：CAS-then-checkout 原子性的持久物化状态（prepared / ref_advanced / materialized / superseded / blocked） |
| v20 | `dag_terminal_deliveries` | 终态事件交付状态：replay once / 去重（prepared / appending / delivered / blocked） |
| v21 | `dag_registration_evidence` + `dag_approval_evidence` | DAG plan 注册证据与并发审批证据：correction reentry 的持久化凭据 |
| v22 | `dag_correction_requests` | 执行失败 correction 请求账本：带 typed `artifact_refs` 的有界反馈通道 |
| v23 | `dag_integration_failures` | 集成失败事实表：绑定 verify attempt 与 checkout generation，路由到 `dag_runtime` 虚拟 target |

### job 恢复语义（worktree 复用）

复用 `parallel-forge` 的 worktree 遵循通用 worktree 复用规则：必须显式给出复用键（`--plan <plan.md>` 或 `--worktree-name <name>`）。cleanup 前 runtime 会先记录恢复边界：

- **身份校验通过**（plan / preset / 配置 / worktree 名称一致）：loop 经 `task.resume` 恢复通道从 pending hat 继续，而不是重启整个流程。
- **身份不一致**：复用在 loop 启动前被拒绝，错误信息指向该 worktree `.ralph/reuse-history/` 下的归档记录。

```bash
# 按 plan 复用（推荐）
ralph run --worktree --reuse-worktree --plan docs/plans/<your-plan>.md

# 按精确 worktree 名称复用
ralph run --worktree --reuse-worktree --worktree-name <plan-name>-<suffix>
```

正在运行的同名 worktree 不可复用。

### exactly-once 交付

DAG runtime 通过 `dag_terminal_deliveries` 持久去重表保证终态事件恰好交付一次：同一 `unit_key` + `job_id` + `job_token` 三元组的 `forge.unit.executed` 只能成功发送一次，重复发送被 runtime 拦截（emitter 会收到 `duplicate_forge_unit_executed` 类拒绝）。`unit_key` 不是 `wave` 执行面的概念，无需 operator 干预；该表的意义是 loop 崩溃重启后，已交付的终态收据不会被重放第二次。

## `forge.unit.*` topic 参考表

`dag` 模式下 hat 可见的协调 topic 是 `forge.unit.*` 族。以下为 builtin `parallel-forge`（`presets/en/parallel-forge.yml` + `presets/schemas/parallel-forge.yml`）实际声明的全集：

| topic | 含义 |
|---|---|
| `forge.unit.executed` | 某 Unit 执行完成（executor 发送；取代旧 `forge.wave.verified`；受三元组 exactly-once 约束） |
| `forge.unit.execution_failed` | 某 Unit 执行失败（executor 发送；进入 correction 通道） |
| `forge.unit.reviewed` | 某 Unit 评审完成，payload `verdict` 值域 `ACCEPTED` / `REJECTED`（reviewer 发送） |
| `forge.unit.verified` | 某 Unit 验证通过（verifier 发送） |
| `forge.unit.verification_failed` | 某 Unit 验证失败（verifier 发送；进入 failure handler） |
| `forge.unit.integrated` | 某 Unit 已整合：runtime 在 lane CAS fast-forward 后发射，不由 agent 发送 |

上游 plan 阶段的 hat 仍使用 `forge.plan.inspected` / `forge.plan.ready` / `forge.concurrency.approved` / `forge.plan.blocked` 完成注册与并发审批；它们不属于 `forge.unit.*` 族，此处仅作定位说明。

**旧 `forge.wave.*` 族（如 `forge.wave.verified`）在 `dag` 模式下不再由 hat 订阅**（仅 dispatcher 内部 seam 保留为过渡期）；hat context 下 `ralph emit forge.wave.verified` 会被 `event_policy` 拒收。同理，DAG runtime 自有的 `forge.unit.*` 收据由 runtime 发射，agent 不得用 `ralph wave emit` 伪造 wave 收据。

## 相关文档

- [调度模式设计说明](../explanation/scheduler-modes.md) — 三态的设计动机与选型论证
- [配置参考 · supervisor 段](../guide/configuration.md#supervisor) — `event_loop.supervisor.*` 字段表
- [supervisor redrive CLI](../solutions/supervisor-redrive/redrive-cli.md) — supervisor 账本重放/恢复操作
