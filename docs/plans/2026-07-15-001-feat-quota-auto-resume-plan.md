---
title: "feat: MiniMax 双 Token Plan 配额门控与中断续跑"
type: feat
status: active
date: 2026-07-15
updated: 2026-07-16
deepened: 2026-07-16
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: user-confirmed-and-live-response-fixture
repository_baseline: 33385635
---

# MiniMax 双 Token Plan 配额门控与中断续跑 - Plan

## 1. 功能目标

### 计划定位与执行约束

- 本文是可直接执行的开发计划，不是 roadmap。每个 Unit 都冻结本单元的输入、输出、代码落点、状态变化、失败分支、测试场景和完成条件；实现者不得只完成“阶段目标”而把关键设计留到后续 Unit 猜测。
- 本计划按 `ce-unified-plan/v1` 被执行管线消费。Unit 标题与顺序是稳定执行身份；Requirement、Scenario、Unit、测试和最终门禁必须保持可追踪。
- 实现仍采用 test-first / characterization-first，但本文不预写具体 Rust 方法签名或生产代码。实现时允许根据编译器和现有类型边界调整局部命名，不得改变本文冻结的行为、提交边界与恢复语义。
- 仓库基线为 `33385635`（2026-07-16）。后续若目标文件再次变化，执行者必须先重新核对本节“当前源码基线”，再开始对应 Unit；不能按旧行号或旧 runner 顺序机械套用。

### 当前源码基线（2026-07-16 复核）

| 关注面 | 当前实现事实 | 本计划据此作出的实现约束 |
|---|---|---|
| activation 入口 | `crates/ralph-cli/src/loop_runner/runner.rs` 当前顺序是 `next_hat()` → `PostIterationStart` hooks/可能 suspend → `build_prompt()` → backend env/channel/spawn | quota gate 必须插在 `next_hat()` 成功之后、任何 `PostIterationStart` hook 之前；等待期间不得触发生命周期 hook、RPC iteration 更新、prompt/diagnostics、hat channel、backend env 或进程创建 |
| hat 选择 | isolated 模式的 `EventLoop::next_hat()` 会消费 `pending_recovery_hat` pin，并由 `EventBus::select_next_hat_with_pending` 推进 round-robin cursor；`build_prompt()` 才消费该 hat 的 pending batch | 选中后立即形成 activation lease，冻结 hat、selection provenance、cursor 后继位置与尚未消费的事件批次；等待/重启不得再次调用普通选择算法 |
| runtime 时间 | `LoopState::elapsed()` 仍直接返回 `started_at.elapsed()`，`check_termination()` 用它判断 `max_runtime_seconds` | quota active wait 必须通过 `LoopState` 的显式 pause accounting 扣除；不能只在 runner 日志层“认为暂停”，否则 core 仍会误触发 `MaxRuntime` |
| isolated channel | channel 在 backend spawn 前创建；backend 返回后，runner 当前先 `merge_hat_channel()` 到主/candidate JSONL，再 `process_output()`，随后 `process_events_from_jsonl_with_waves()` 得到 `ProcessedEvents.accepted_events` | quota 中断路径必须新增 activation-local prepare/commit 边界：验收前不得把候选业务事件永久并入主事件流；只有 runtime accepted event 才提交，未提交 batch 必须可丢弃且不能留下 recovery budget、projection 或 routing 副作用 |
| execution evidence | 非 PTY 的 `ExecutionResult` 保留 stdout/stderr 汇总、exit code 和 timeout；PTY 同时保留 raw、ANSI-stripped、extracted text。现有 `ExecutionOutcome.output` 只承载事件解析文本 | quota classifier 必须读取独立的完整 execution evidence，不得复用被规范化/抽取后的事件解析文本；事件解析与错误分类是两条字段明确分离的数据流 |
| continue | `ralph run --continue` 先检查 scratchpad；generic resume 初始化会注入 `task.resume`/`loop.resume`，并按普通选择继续 | 有 quota checkpoint 时必须先校验并走 quota-specific restore，禁止额外注入 generic resume event；没有 checkpoint 时保持现有 `--continue` 行为不变 |
| durable state | hook suspend 已有版本化 JSON + same-directory temp/rename 模式，但其 stop/restart 会清理 suspend artifact，且 store 没有 quota activation identity、owner-only/fsync 契约 | quota checkpoint 使用独立 store，可复用错误类型和原子替换思路，但必须自行实现 schema、0600、fsync、归属校验和 stop/restart 保留语义，不能直接复用 hook suspend record |
| 测试并发 | `.config/nextest.toml` 已删除 `ralph-cli` 的 `cli-serial` override；nextest 对所有包默认并发 | 所有新增测试必须并发安全、使用 per-test temp workspace/fake transport/虚拟时钟，不新增 process-global mutable fixture，也不依赖断言侧固定 sleep |

### 业务目标

- Ralph 通过既有 Proxy 使用两个 MiniMax 中国区订阅账号；Proxy 继续负责真实模型请求的账号路由，Ralph 只查询两个账号的套餐额度。
- 每次串行 hat activation 启动前，把两个账号当前 5 小时窗口的可用百分比汇总成一个池；只有池量严格大于单次 activation 预算（默认 20%）才启动 backend。
- 池量不足时，Ralph 不消耗触发事件、不构建 prompt、不启动 backend，而是等到最早可信刷新时间后重查；运行中命中套餐额度耗尽时，按本 activation 是否已经产生 runtime accepted event 决定提交或冷启动重放。
- 配额等待、stop/restart、跨进程 `--continue` 和恢复上限均可诊断、可恢复且不泄露两个订阅 Key。

### 本次范围

- MiniMax 中国区 `GET https://www.minimaxi.com/v1/token_plan/remains`，每个账号使用独立 Subscription Key/Bearer 认证。
- 恰好两个等容量套餐账号；串行查询 A 后查询 B；成功 observation 汇总，失败账号按未知且贡献 0 处理。
- 用户已于 2026-07-16 提供一次中国区真实成功响应；从 `model_remains` 中只读取 `model_name == "general"` 的 5 小时/周百分比、状态和刷新时间。该响应必须脱敏后固化为 committed fixture，真实 Key 永不进入仓库、测试命令、日志或 fixture。
- 默认预算 20%，仅当 `pool_remaining_percent > activation_budget_percent` 时启动；等于预算时等待。
- 等待 deadline、fallback 重查、`max_wait_seconds`、`max_rechecks`、stop/restart/workspace-gone、active-runtime 暂停计时。
- 运行中 quota signature 分类、accepted event 提交点、未提交 isolated channel 丢弃、原 activation 持久化和 cold-restart。
- CLI runner、termination sentinel、summary/display/diagnostics、配置文档和 agent 注入指南同步。

### 非目标

- 不支持国际区 `minimax.io`、其它供应商、任意账号数、不同容量套餐加权、并发 hats、wave worker 预留、跨 loop 全局配额账本。
- 不让 Ralph 替 Proxy 选账号或切换 backend Key；不修改 `ANTHROPIC_AUTH_TOKEN`、`ANTHROPIC_API_KEY`、Proxy URL 或现有 backend 环境变量。
- 不按 token 精确预测消耗，不自动回滚 quota 中断前的代码/task/memory/scratchpad 副作用。
- 不把普通 RPM/TPM 限流、任意 `429`、网络失败或认证失败一概当作 5 小时套餐耗尽。
- 不新增用户 CLI 命令，不修改 builtin preset、preset schema、hat topology 或 zsh completion。由于新增 `event_loop.quota` 及 activation/recovery 行为会影响 preset 作者与评审的判断，本计划只同步 preset operator skills 的通用参考规则，不把账号 Key 或 MiniMax 专用配置写进任何 preset。
- 不在测试中调用线上 MiniMax，不把所有验收场景升级成 E2E。

### 需求编号

- **R1 配置与 opt-in**：缺少 quota、`enabled: false` 或未配置账号时，旧配置和旧 runner 行为完全不变；启用时必须恰好两个唯一命名、非空 Key 的账号。在 Unix 上，若明文 Key 来自可定位的 YAML 文件且该文件对 group/other 可读写，preflight 必须拒绝启动并只报告权限修复动作，不回显 Key；无法定位来源或非 Unix 时至少输出不含路径敏感内容的安全警告并继续遵循现有配置来源语义。
- **R2 查询契约**：分别查询两个账号，只接受 HTTP/JSON/业务状态均有效且存在 `general` 条目的 observation；请求有超时、响应体大小上限且凭据不跨 host 转发。
- **R3 池化判断**：两个有效账号的 5 小时 `current_interval_remaining_percent` 求和；只有周剩余百分比明确为 0，或已由脱敏耗尽 fixture 证明的周耗尽状态，才使该账号贡献 0。`*_total_count == 0` 不代表耗尽；总量严格大于预算才允许 activation。
- **R4 智能等待**：不足时取每个账号可用候选 deadline 的最早值并加 buffer；查询失败或无可信未来窗口时使用 fallback，过期/陈旧 deadline 不得形成忙循环。
- **R5 独立预算与时间**：首次 A→B 双账号查询叫 `initial_check`，不占 `max_rechecks`；每次等待后的完整 A→B 查询叫一次 `recheck`，无论单账号成功/失败都只加 1。quota active wait 不增加 iteration/failure/fallback/recovery，也不计入 `max_runtime_seconds`，但受自身等待与重查上限约束；本地等待预算耗尽与供应商返回套餐耗尽必须使用不同 typed 名称。
- **R6 人工与工作区状态**：等待期间 stop 保留 pending state 并以 `Stopped` 退出，restart 保留 state 并沿用现有自动重启，workspace-gone 终止且不承诺恢复。
- **R7 运行中分类**：仅命中经脱敏 fixture 固化的 MiniMax 套餐耗尽 signature 才进入 quota 分支；普通错误保持现有路径。
- **R8 activation 提交点**：本 activation 归属的 runtime accepted 业务事件或终态事件是唯一提交点；loop-internal/diagnostic event 不构成提交。已有提交事件不重放，无提交事件则不计普通失败并重放同一 activation。
- **R9 跨进程恢复**：quota checkpoint 保存同一 loop/workspace、hat、尚未消费的触发事件批次、task 上下文、iteration/attempt、deadline 和预算计数；`--continue` 校验状态归属后恢复。
- **R10 新鲜度与副作用**：未提交 activation 至少冷却一个 fallback 周期并重新查询两个账号；保留工作区副作用，清理未提交 isolated channel，恢复 prompt 要求先检查现状。
- **R11 凭据与认证边界**：Key 不进入 Debug/Display、日志、TUI/RPC、summary、diagnostics、termination sentinel、quota checkpoint、prompt 或 backend env。
- **R12 操作文档与零回归**：配置、恢复语义和 agent 可见动作有文档；disabled/default 路径无 HTTP、quota 文件 I/O、额外延迟、环境变量或错误分类变化。

### 已知约束和假设

- **实施前安全前置条件**：2026-07-16 调试会话中曾把一个真实 Subscription Key 粘贴到交互记录。该 Key 必须先在 MiniMax 侧撤销/轮换；实现、fixture 和验收只能使用轮换后的运行时配置与测试哨兵 Key，不得把已暴露值复制到任何命令、文件、issue、日志或诊断产物。
- 源码现状已确认：`crates/ralph-cli/src/loop_runner/runner.rs` 在 `next_hat()` 与 `EventLoop::build_prompt()` 之间新增了 `PostIterationStart` hook、suspend gate、RPC iteration 更新和显示状态更新。pre-activation quota gate 必须位于 `next_hat()` 成功之后且早于这些副作用；等待期间只冻结 selection，不得消费事件、触发生命周期 hook、推进 operator-facing iteration 或清除 handoff obligation。
- `EventLoop::next_hat()` 不消费 pending queue，但会推进 isolated round-robin cursor，并可能 `take()` 一次 `pending_recovery_hat`。quota checkpoint 必须保存 activation lease：selected hat、选择来源（recovery pin / priority handoff / round-robin / coordinator）、待消费事件批次、cursor 恢复信息和 task/iteration identity；跨进程恢复不得再次调用一般选择算法来猜 hat。
- `crates/ralph-core/src/event_loop/loop_state.rs` 的 `elapsed()` 当前只使用 `started_at.elapsed()`；需要显式累计 quota pause，且保持其它 runtime limit 语义不变。
- 当前 CLI/PTY 两条存续执行面都持有足够的原始 backend evidence，但 `ExecutionOutcome.output` 已被定义为事件解析文本：CLI 会按 output format 规范化，PTY 会优先选 `extracted_text` 而不是 raw/stripped output。先做 characterization test，再为 `ExecutionOutcome` 增加与解析文本分离的、脱敏后可分类的 execution evidence；删除的 ACP production path 不在范围内。
- 官方文档已把旧 Coding Plan 表述为 Token Plan，并说明额度是统一池；中国区仍公开 remains endpoint、5 小时和周窗口，但没有公开承诺本计划依赖的响应字段。Unit 2 若发现脱敏 fixture 不再包含 `model_remains/general`，必须停止并回到规格修订，禁止猜字段、回退到“汇总所有模型”或静默 fail-open。参考 [MiniMax 中国区 Token Plan FAQ](https://platform.minimaxi.com/docs/token-plan/faq)。
- 两账号百分比求和成立的业务前提是套餐容量相同且 Proxy 确实聚合两者；不满足时不得启用。
- 目标账号不依赖 Purchased Credits 或 pay-as-you-go 余额继续运行；MiniMax 官方说明 Credits 可在套餐额度后自动承接用量，因此若部署实际依赖 Credits，本 gate 可能保守阻塞，本期不得启用。Ralph 不查询或估算 Credits。
- 选择在 Ralph 而非 Proxy 实现恢复，是因为 Proxy 只能路由模型请求，无法观察 runtime accepted event、hat/trigger identity、isolated channel 和 `--continue` 状态；仅在 Proxy 重试会造成重复交接或错 hat。
- Subscription Key 允许按用户确认直接放入私有 YAML；本期不迁移 keychain。操作文档必须要求配置文件 owner-only、不得提交版本库，并说明 Key 轮换只需更新配置；runtime 产物仍禁止复制 Key。
- 两次查询严格串行，单次 gate 最坏网络等待不超过两个账号 timeout 之和且没有隐藏 HTTP retry；这是用户选择的可预测延迟换安全判断，timeout 默认值和实际延迟必须可观测。
- `max_wait_seconds` 只累计 Ralph 实际处于 quota wait 的时间；进程已停止/离线时间不计入该上限。恢复时若 wall-clock deadline 已过则立即重查，`max_rechecks` 只累计 initial check 之后的完整双账号 recheck，并跨进程累计。
- 同一时刻出现多个控制信号时沿用现有控制优先级；deadline 不得压过已经可观察到的 stop/restart/workspace-gone。
- 外部网络只用于运行时查询；测试使用本地 mock transport/server，并遵循 `docs/solutions/test-failures/reqwest-no-proxy-loopback-test-failures.md` 的 loopback no-proxy 约束。
- 测试入口严格遵循 AGENTS.md 与 `.config/nextest.toml`：所有包（含 `ralph-cli`）走 nextest 默认并发，禁止裸跑 `cargo test -p ralph-cli`。旧文档/脚本注释中的 `cli-serial` 字样不作为当前测试模型依据。

### 已验证的 MiniMax 响应事实与 fixture 规则

2026-07-16 的一次真实成功响应已确认以下 wire shape；计划只保留脱敏后的结构和值，不保留请求凭据：

| 字段 | `general` 实测值 | 计划结论 |
|---|---:|---|
| `base_resp.status_code` / `status_msg` | `0` / `success` | 业务成功必须同时满足 HTTP 成功与 `status_code == 0` |
| `current_interval_remaining_percent` | `51` | 这是本账号 5 小时池贡献的直接来源 |
| `current_interval_total_count` / `usage_count` | `0` / `0` | count 为 0 仍可能有 51% 剩余，不能用 count 推导百分比或耗尽 |
| `current_interval_status` | `1` | 作为 observed-success 状态纳入 fixture；没有反例前不把未知状态自行映射成“可用” |
| `current_weekly_remaining_percent` | `100` | 周窗口仍可用，不阻断 5 小时贡献 |
| `current_weekly_total_count` / `usage_count` | `0` / `0` | 再次证明 total count 的 0 不是周耗尽信号 |
| `current_weekly_status` | `3` | `status == 3` 与 100% 可用同时出现，禁止把 3 猜成耗尽；状态语义只由 fixture/官方契约驱动 |
| `start_time` / `end_time` | 实测相差 4 小时 | 产品仍称 5 小时窗口，但实现不得断言固定 5 小时时长；只信任当前响应的 `end_time`/`remains_time` 作为刷新依据，并把观测到的 interval 原样记录为脱敏诊断摘要 |
| `end_time` / `weekly_end_time` | epoch-millis future timestamp | 用作候选刷新 deadline；测试 clock 固定，不能依赖执行当天 wall clock |
| `model_name == "video"` | 同响应中存在 | parser 必须忽略，不得汇总到 `general` 池 |

Fixture 纪律：

- 创建 `crates/ralph-cli/src/loop_runner/quota/fixtures/minimax-remains-general-51.json` 保存上述真实 shape；只允许调整时间值使 fixture 可读/稳定，不改字段名、状态组合或 `general`/`video` 并存事实。
- 从该 fixture 派生 table-driven 变体时，必须在测试名中写清只改了什么，例如 `general_12_percent`、`weekly_zero_percent`、`missing_general`、`stale_interval_end`；不要复制一批来源不明的巨大 JSON。
- fixture 和 decision tests 不得断言 `end_time - start_time == 5h`；必须覆盖实测 4h span 仍按 `end_time` 正常决策，防止产品命名覆盖真实 wire semantics。
- 目前尚无真实“套餐耗尽”错误响应和周耗尽响应。Unit 6 的运行中 signature、Unit 3 的显式周耗尽状态仍以新的脱敏实测 fixture 为前置；未取得前只能测试“百分比为 0”的 fail-closed 决策，不能猜 status code 或错误文案。

### Outside-In 能力分解

```text
操作者配置/继续运行
  → runner 在 prompt 前门控所选 activation
    → MiniMax client 产生两个脱敏 observation
      → 纯决策器给出 Start / Wait(deadline)
        → 可中断等待 + durable checkpoint
          → backend execution evidence 分类
            → runtime accepted event 判定 Commit / Replay
              → termination、summary、诊断与文档
```

## 2. BDD 行为规格

### Feature: Opt-in 配额门控

#### Scenario S1：默认与显式禁用保持旧行为

- **Given** 配置没有 `event_loop.quota`，或 quota 显式 disabled
- **When** runner 选择下一个 hat 并执行 activation
- **Then** 不创建 HTTP client、不访问 quota checkpoint、不增加 backend 环境变量，并按现有 prompt、执行、事件和失败路径运行

#### Scenario S2：非法启用配置被安全拒绝

- **Given** quota enabled，但账号数不是 2、名称重复、Key 为空、预算/timeout/等待上限越界之一成立
- **When** Ralph 解析并验证配置
- **Then** 在启动 runner 前返回指向字段和账号名的错误，错误、Debug 和诊断中不出现 Key

#### Scenario S3：池量充足时启动一次原 activation

- **Given** 两个有效 `general` observation 分别剩余 12% 和 15%，预算为 20%
- **When** runner 在构建 prompt 前完成双账号查询
- **Then** 池量 27% 允许启动，pending events 只在 prompt 构建时消费一次，backend 仍只收到原 Proxy 认证

#### Scenario S4：池量等于预算时等待，恢复后再启动

- **Given** 两账号合计正好 20%，最早未来窗口在 T，且后续重查得到 21%
- **When** gate 评估、等待至 T 加 buffer 并重查
- **Then** T 前不触发 `PostIterationStart` hook、不更新 RPC/display iteration、不构建 prompt/diagnostics/hat channel、不注入 backend env、不启动 backend；重查后上述 activation 生命周期只执行一次，并启动原 hat/原事件批次；等待不增加 iteration/failure 且不消耗 max runtime

#### Scenario S5：单账号查询失败按未知处理

- **Given** A 查询超时，B 有效剩余 25% 或 15%
- **When** 形成池决策
- **Then** B=25% 时仍启动；B=15% 时等待，并优先按 A 的 fallback deadline 重查而不是等待 B 的较晚窗口

#### Scenario S6：无效响应、周耗尽和陈旧窗口 fail-closed

- **Given** 响应为 401/429/5xx、业务状态失败、畸形 JSON、缺少 `general`、百分比非法、周剩余百分比为 0，或窗口时间已过但额度未刷新；另有实测合法组合 `weekly_total_count=0 + weekly_status=3 + weekly_remaining_percent=100`
- **When** client 和决策器处理 observation
- **Then** 无法证明的额度贡献 0；明确周耗尽使用周刷新时间；实测合法组合保留 5 小时贡献，不因 total count/status 猜测而阻断；陈旧时间最多触发一次立即重查，随后使用 fallback，绝不零延迟自旋

### Feature: 可中断等待与跨进程恢复

#### Scenario S7：stop 后只有显式 continue 才恢复

- **Given** activation 正在 quota wait，checkpoint 已保存且未消费触发事件
- **When** 操作者发出 stop，之后执行 `ralph run --continue --loop-id <same-loop>`
- **Then** 第一次运行以 `Stopped` 退出并保留脱敏 state；continue 在 generic `task.resume`/`loop.resume` 初始化之前校验 loop/workspace/config，按 deadline 等待或重查并恢复同一 activation；恢复路径不额外注入通用 resume event，也不再次推进 hat cursor

#### Scenario S8：restart 与 workspace-gone 保持现有控制语义

- **Given** activation 正在 quota wait
- **When** 收到 restart 或检测到 workspace-gone
- **Then** restart 保留 state 并交给现有自动重启路径；workspace-gone 终止且不声称可以恢复，不启动 backend

#### Scenario S9：quota 自身等待预算耗尽给出专用终止原因

- **Given** 累计 active wait 达到 `max_wait_seconds`，或双账号 recheck 次数达到 `max_rechecks`
- **When** gate 尝试继续等待/重查
- **Then** 以专用 `QuotaWaitBudgetExhausted` 原因终止并指出触发的是 active-wait 还是 recheck 上限；不冒充供应商 `QuotaPlanExhausted`、MaxRuntime、MaxCost 或 ConsecutiveFailures，sentinel、summary、display 和 RPC 的现有 generic-error 映射一致

#### Scenario S10：损坏或不属于当前运行的 state 不得重放

- **Given** checkpoint schema 未知、JSON 损坏、loop_id/workspace 不匹配、hat 已不存在，或 continue 时 quota 已关闭
- **When** Ralph 尝试恢复
- **Then** fail-closed 并给出可操作错误，不查询 usage、不选择新 hat、不运行陈旧 activation

### Feature: 运行中配额中断的提交与重放

#### Scenario S11：无 accepted event 时冷启动重放同一 activation

- **Given** backend 命中确认过的 MiniMax quota signature，当前 activation 没有 runtime accepted event，但工作区可能已有部分修改
- **When** runner 完成 isolated channel 验收并分类 execution evidence
- **Then** 不增加普通失败/iteration/fallback/recovery；activation-local candidate 经 canonical runtime policy 的 prepare 阶段确认没有 accepted event 后，丢弃未提交 channel 与所有 staged routing/projection/recovery-budget 变化，持久化原 activation；至少冷却一个 fallback 周期并 fresh recheck 后，以恢复提示重启同一 hat/事件/task 上下文

#### Scenario S12：已有 accepted event 时提交且不重放

- **Given** backend 已写出一个通过 runtime policy 的业务或终态事件，随后命中 quota/non-zero exit
- **When** runner 先合并 channel并获得本 activation 的 accepted-event snapshot
- **Then** activation-local prepare 结果中的 accepted 业务/终态事件作为提交点，按原顺序一次性写入主事件流并只路由一次；随后执行正常 `process_output`/iteration 收尾，不创建 pending replay；仅有 loop-internal/diagnostic event 时仍不得视为提交

#### Scenario S13：普通限流和执行错误不被误判

- **Given** 普通 RPM/TPM 429、认证失败、网络断开、模型不存在、watchdog 或一般非零退出
- **When** classifier 检查完整 output/error evidence
- **Then** 仍走现有 success/failure/watchdog/missing-event/hard-gate 路径，quota disabled 时连 signature 也不改变旧分类

#### Scenario S14：所有可观察面均不泄露 Key

- **Given** 两个带可识别哨兵值的测试 Key，且覆盖成功、查询失败、等待、stop、恢复、quota exhausted 和运行中中断
- **When** 收集日志、TUI/RPC 输出、summary、diagnostics、sentinel、checkpoint、prompt、backend env 和错误链
- **Then** 所有产物均不含完整 Key、Key 前缀、Authorization header 或完整 quota 配置；backend auth 与 quota Key 保持隔离

## 3. 验收与测试策略

| Scenario | 验收条件 | 推荐测试层级 | 是否需要 E2E |
|---|---|---|---|
| S1 | disabled/default 时 quota facade、HTTP、state store 零调用，旧事件序列不变 | 配置单测 + runner characterization/integration | 否；现有 mock E2E 做回归 |
| S2 | 所有非法组合在 runner 前失败且错误脱敏 | `ralph-core` 单元测试 | 否 |
| S3 | 12+15>20，只启动一次，认证不串线 | client 契约测试 + runner 集成测试 | 否；runner 真路径足够 |
| S4 | 等于 20 不启动；虚拟时间后 21 启动同一 activation；active runtime 不增长 | 决策单测 + paused-time runner 集成 | 否 |
| S5 | 部分失败贡献 0，已知额度仍可启动，fallback deadline 优先 | 决策单测/性质测试 | 否 |
| S6 | 无效输入不 panic、不贡献额度；周/陈旧窗口按规则选择 | parser 单测 + property-based decision test | 否 |
| S7 | stop 保留 state，continue 恢复相同 identity | state-store 集成 + runner restart harness | 是，使用 runner 的生产入口与 fake backend |
| S8 | restart/workspace-gone 不 spawn 且原因正确 | runner 集成测试 | 否 |
| S9 | 两个上限分别产生 QuotaWaitBudgetExhausted，并贯通输出面 | 状态机单测 + termination 集成 | 否 |
| S10 | schema/归属/config mismatch fail-closed | state-store/continue 集成 | 否 |
| S11 | quota+无 accepted event 不计普通失败，fresh recheck 后同一 activation 重放 | runner acceptance integration，fake quota + fake backend | 是，作为关键主路径验收 |
| S12 | activation-owned 业务/终态事件先验收后提交，只出现一次；internal/diagnostic event 不提交 | runner acceptance integration | 是，与 S11 共用最小场景 |
| S13 | 非 quota fixture 与现有错误行为差分一致 | adapter characterization + differential regression | 否 |
| S14 | 对所有产物执行哨兵 Key 断言，backend env 无 quota Key | 单元 + runner 安全集成 | 否 |

### Mock 演练矩阵（必须落地，不调用线上服务）

这里的“端到端”指 **Ralph 进程内完整生产路径**：真实 config parse/validate → 真实 quota HTTP client → 本地 mock HTTP server → 真实 pool decision/wait state machine → 真实 runner/EventLoop/hat channel/policy → fake backend process。唯一替换的是外部 MiniMax 服务与模型 CLI，因此可以稳定复现且不会消耗真实套餐。

| Drill | Account A mock | Account B mock | Backend mock | 预期 runner 行为与必须断言 |
|---|---|---|---|---|
| D1 实测形状可解析 | committed `general=51%` + `video=100%` fixture | 同形状或不查询 | 不启动 | 只取 `general`；count=0/status=3 不误判；Authorization 只发给本地期望 host；输出 observation 不含 Key |
| D2 池充足 Go | `general=12%` | `general=15%` | 成功并 emit accepted event | pool=27>20；两个请求严格 A→B；只启动一次原 activation；pending batch 只消费一次 |
| D3 临界值 No-Go | `general=10%` | `general=10%` | probe 若启动即失败测试 | pool=20 不满足严格 `>`；hook/prompt/channel/env/backend 全部零调用；进入 Wait |
| D4 不足后刷新 Go | 首轮 8%，次轮 12% | 首轮 10%，次轮 11% | 第二轮成功 | initial check 得 18，等待到最早 deadline+buffer；一次 recheck 得 23 后启动；总 query round=2、recheck=1 |
| D5 单账号失败但仍 Go | timeout/500/畸形 JSON | 25% | 成功 | A 贡献 0、B 贡献 25，仍 Go；错误脱敏；没有隐藏 HTTP retry |
| D6 单账号失败且等待 | timeout | 15% | 不启动 | pool=15；选择 A fallback 与 B refresh 中更早可信 deadline；不 busy-loop |
| D7 周窗口阻断 | interval 80%、weekly 0% | interval 0%、weekly 100% | 不启动 | A 因明确 weekly remaining=0 贡献 0，使用 weekly deadline；B 贡献 0；注意不使用 total_count 推断 |
| D8 陈旧 deadline | 10%，interval end 已过 | 5%，weekly end 已过 | 不启动 | 最多一次立即 recheck，之后 fallback；虚拟时间推进，无真实 sleep/flaky |
| D9 stop/continue | 15% | 0% | 不启动 | Wait 中 stop 保留 checkpoint；第二个 runner 进程 `--continue --loop-id` 恢复 exact lease，不注入 generic resume event |
| D10 quota 中断无提交 | preflight 足够 | preflight 足够 | 输出真实 quota-exhausted fixture，不 emit | candidate Discard，普通 failure/iteration 不增；cooldown+fresh recheck 后同一 activation cold replay |
| D11 quota 中断有提交 | preflight 足够 | preflight 足够 | 先 emit 一个 runtime accepted event，再输出 quota fixture/non-zero | prepared batch Commit 一次，不 replay；事件在主流只出现一次 |
| D12 非 quota 负例 | preflight 足够 | preflight 足够 | 普通 429、401、network error、watchdog、post-event timeout 各一例 | 全部保持现有路径；classifier 零误判；disabled 模式不扫描 evidence |
| D13 双 Key 隔离 | server A 只接受哨兵 Key A | server B 只接受哨兵 Key B | 捕获 backend env | A/B header 不串线；backend env、prompt、checkpoint、summary、diagnostics 中均找不到 A/B 哨兵值 |

演练实现约束：

- 本地 server 绑定 `127.0.0.1:0`，client 使用 `.no_proxy()` 或仓库既有 loopback 规则；每个 test 自己持有 listener、请求脚本和 temp workspace，支持 nextest 并发。
- mock server 用有序 response queue 明确表达“首轮/次轮”，并记录 method、path、host、Authorization、请求顺序和次数；断言必须检查 A→B 串行，而不只检查最终 decision。
- runner 时间全部走 `tokio::time::pause/advance` 或注入 clock；测试断言侧用事件/通道/有界超时，不用固定 `thread::sleep` 等真实 5 小时窗口。
- fake backend 必须是 runner 真正 spawn 的测试进程/脚本接口，不能直接调用 disposition helper 冒充 acceptance；Unit 3/6 的纯函数测试仍单独保留，二者职责不同。
- committed fixture 与派生 mock 都使用 `TEST_SUBSCRIPTION_KEY_A/B` 这类明显哨兵值。任何测试日志或失败快照出现用户真实 Key 都视为 P0 安全失败。

风险驱动补充：

- **Characterization/Differential**：先锁定 disabled runner、CLI/PTY 非零退出、post-event timeout、watchdog 和普通 429 的现状，再修改共享执行路径。
- **Contract Test**：MiniMax 请求方法、host、Authorization、timeout、redirect 和脱敏响应 fixture 用本地 server/transport 验证；不调用线上服务。
- **Property-Based Test**：对两个 0..100 observation、预算 1..199、未来/过去 deadline 组合验证“只有 sum>budget 才 Start”“选择最早有效 deadline”“永不产生零延迟无限重查”。若新增 `proptest`，仅作为 `ralph-cli` dev-dependency。
- **State-Machine Test**：覆盖 Ready→Waiting→Recheck→Ready、Waiting→Stopped/Restart/WorkspaceGone/Exhausted、PendingReplay→Cooldown→Recheck→Replay；控制信号优先于同 tick deadline。
- **Fault Injection**：timeout、连接失败、401、429、5xx、业务状态失败、畸形 JSON、state 原子写入失败、损坏/未知版本。
- **Concurrency**：本期没有并发 activation/reservation；只测试 timer 与控制信号同 tick 的确定性优先级，不扩张成跨 loop 并发方案。
- **E2E 边界**：新增 quota 行为使用 production runner + fake facade 的 acceptance integration，避免为固定 HTTPS endpoint增加测试专用生产配置；最终仍运行现有 `ralph-e2e --mock` 作为全局主路径回归。

## 4. 需求—测试追踪矩阵

| 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E |
|---|---|---|---|---|---|
| R1 | S1, S2 | quota opt-in/invalid-config acceptance | config defaults、validation、secret Debug | disabled runner zero-call | 现有 mock E2E 回归 |
| R2 | S3, S5, S6 | 两账号请求与响应契约 | parser/observation | local HTTP contract | 不调用线上 |
| R3 | S3, S4, S5, S6 | Start/Wait 表驱动验收 | pool/deadline property tests | preflight gate integration | 否 |
| R4 | S4, S5, S6 | 虚拟时间等待验收 | deadline/fallback | runner paused-time | 否 |
| R5 | S4, S9 | 上限与计数验收 | wait state machine、LoopState active elapsed | runner termination | 否 |
| R6 | S7, S8 | stop/restart/workspace-gone acceptance | signal priority | runner control integration | S7 runner 主路径 |
| R7 | S11, S13 | classifier fixture acceptance | signature classifier | CLI/PTY differential | 否 |
| R8 | S11, S12 | commit/replay acceptance | disposition decision | real event acceptance + channel integration | S11/S12 runner 主路径 |
| R9 | S7, S10, S11 | checkpoint round-trip/restart | schema、identity、ownership | continue/replay integration | S7/S11 |
| R10 | S11 | fresh recheck/cooldown acceptance | cooldown decision | replay integration | S11 |
| R11 | S2, S14 | secret sentinel scan | secret wrapper、redacted errors | artifacts/env integration | 否 |
| R12 | S1, S9, S14 | docs/zero-regression acceptance | termination formatting | CLI doc drift + full runner regression | 现有 mock E2E |

## 5. 严格串行开发单元

唯一合法顺序：

```text
Unit 1 → Unit 2 → Unit 3 → Unit 4 → Unit 5 → Unit 6 → Unit 7 → Unit 8 → Unit 9
```

任一 Unit 未完成其验收、Red→Green→Refactor、相关集成和受影响回归，不得开始下一 Unit。后续 Unit 若发现前置契约错误，必须回到责任 Unit 修正并重新顺序验证，不得在后续 Unit 偷补。

### Unit 1：opt-in 配置、校验与 secret 边界

- **Unit 目标**：仅建立可解析、默认关闭、启用时可验证且 Debug/错误脱敏的 quota 配置。
- **对应 Scenario**：S1、S2、S14（配置面）。
- **外部可观察结果**：旧 YAML 原样解析；合法 quota 配置通过；非法配置在 runner 前失败且不回显 Key。
- **输入与输出**：输入为 YAML/`RalphConfig`；输出为 validated quota config、默认值或 `ConfigError`。
- **可依赖的已完成能力**：现有 `EventLoopConfig` serde/default、`RalphConfig::validate()`、配置错误模式。
- **明确禁止依赖的未来能力**：不得构建 HTTP client、访问 runner/state、定义 MiniMax response model 或启动 backend。
- **文件**：修改 `crates/ralph-core/src/config/loop_config.rs`、`crates/ralph-core/src/config/ralph_config.rs`、`crates/ralph-core/src/config/error.rs`、必要的 `crates/ralph-core/src/config/mod.rs`；测试放在相同 config 模块。
- **验收测试**：缺省/disabled 零差异；合法双账号默认值；账号数、重复名、空 Key、预算 1/199 边界及越界、timeout/上限为 0；Unix 0600/0640/0644 配置权限（enabled 才检查，0600 通过，group/other 任一权限拒绝）；无法定位来源/非 Unix 的安全警告；错误与 Debug 的哨兵 Key 搜索。
- **需要拆分的单元测试**：secret wrapper Debug/Clone/accessor；quota enabled 判定；字段级 validation；配置来源权限判定；disabled 不校验 accounts/权限的 characterization。
- **Red 预期失败原因**：serde 不认识 quota 字段或缺少校验/脱敏类型，非法配置被接受或 Key 出现在 Debug。
- **最小实现范围**：配置结构、默认值、只读 secret 表达、显式 validation，以及 enabled 明文文件配置的 preflight 权限检查；不添加 HTTP/等待/backend 行为。
- **集成验证**：`RalphConfig::parse_yaml`/`validate` 走真实配置入口；合法旧 fixture 与新 fixture 均通过。
- **回归范围**：`ralph-core` config tests、所有 embedded preset parse/validate；确认无 preset 需要新增 quota。
- **TDD 闭环**：①先启用 S1/S2 验收；②确认因缺字段/校验正确失败；③拆 secret/default/validation 单测；④逐个 Red→Green→Refactor；⑤跑 config 集成；⑥跑 `ralph-core` 受影响回归；⑦无 ignored/削弱断言后关闭；⑧方可进入 Unit 2。
- **完成标准**：配置契约冻结，disabled 与缺省完全等价，任一错误产物不含 Key。
- **风险与注意事项**：不要派生明文 Debug；不要把完整 config 放入通用 diagnostics；无需修改 builtin preset/schema，但 Unit 9 必须同步 operator skills 对新 `event_loop.quota` 配置边界的认知。

### Unit 2：单账号 MiniMax remains 契约客户端

- **Unit 目标**：对一个中国区 Subscription Key 发出一次安全请求并产生一个脱敏 `AccountObservation`。
- **对应 Scenario**：S3、S5、S6、S14（HTTP 面）。
- **外部可观察结果**：有效 fixture 得到 `general` observation；所有协议/解析失败变成账号级 unavailable，且请求/错误不泄密。
- **输入与输出**：输入为账号配置、timeout、clock/transport；输出为有效 observation 或脱敏 unavailable 类别。
- **可依赖的已完成能力**：Unit 1 的 validated account/secret 契约；workspace 已有 `reqwest`、`tokio`、`serde_json`。
- **明确禁止依赖的未来能力**：不得汇总两个账号、决定 Start/Wait、sleep、写 checkpoint 或调用 runner/backend。
- **文件**：创建 `crates/ralph-cli/src/loop_runner/quota/mod.rs`、`crates/ralph-cli/src/loop_runner/quota/client.rs`、`crates/ralph-cli/src/loop_runner/quota/fixtures/minimax-remains-general-51.json`；机械注册 `crates/ralph-cli/src/loop_runner/mod.rs`；parser 单测放 quota 模块，本地 HTTP 契约测试放 `crates/ralph-cli/src/loop_runner/tests/quota.rs`。
- **验收测试**：GET 固定 path、Bearer 正确、A/B Key 不串线、timeout、无隐藏 retry；实测脱敏 fixture 得到 general=51%；`current_interval_total_count=0` 仍接受 51%；`current_weekly_total_count=0 + status=3 + remaining=100` 不判耗尽；video/其它条目不被误读为 general；401/429/5xx、业务 status、畸形 JSON、缺 general、非法百分比/时间、超出大小上限的响应体；redirect 不向其它 host 携带认证。
- **需要拆分的单元测试**：response parser、实测 status/count 组合、业务状态校验、百分比 0..100 校验、epoch-ms 转换、重复/多个 general 的 fail-closed 决策、redacted error rendering。
- **Red 预期失败原因**：client/parser 尚不存在，或错误对象/请求 debug 暴露 Authorization。
- **最小实现范围**：单账号请求、解析、超时、redirect policy、脱敏 observation；production endpoint 固定，测试仅通过内部 transport/base-url seam 注入本地服务。
- **集成验证**：本地 mock server 捕获真实 HTTP method/path/host/header，并按有序 response queue 返回实测 fixture 与派生变体；测试 client 显式规避环境 proxy，使用两个不同哨兵 Key 证明账号认证隔离。
- **回归范围**：`ralph-cli` quota client 子集；现有 reqwest 使用方不改；确认没有线上请求。
- **TDD 闭环**：①先写单账号契约验收；②确认因无 client/解析失败；③拆 parser/security 单测；④逐个 Red→Green→Refactor；⑤跑 local HTTP contract；⑥跑 `ralph-cli` 相关回归；⑦固定脱敏 fixture 并关闭；⑧方可进入 Unit 3。
- **完成标准**：任意输入都得到一个确定、脱敏 observation，不 panic、不参与调度。
- **风险与注意事项**：实测已确认 `model_remains/general`，但只证明一个成功状态组合；不得从 `total_count` 推导剩余量，不得猜 `status` 枚举含义。若未来 fixture 与当前字段冲突，立即阻塞并修订规格；官方已改称 Token Plan，不能按旧字段猜测新语义。

### Unit 3：双账号池化与 deadline 纯决策

- **Unit 目标**：把恰好两个 observation 纯函数式转换为 `Start` 或 `Wait(deadline, reason)`。
- **对应 Scenario**：S3、S4、S5、S6。
- **外部可观察结果**：12+15 启动、10+10 等待、10+11 启动；失败账号贡献 0；最早可信 future deadline/fallback 唯一确定。
- **输入与输出**：输入为两个 observation、预算、buffer、fallback 和当前时间；输出为池摘要与 Start/Wait decision。
- **可依赖的已完成能力**：Unit 2 observation；Unit 1 validated budget。
- **明确禁止依赖的未来能力**：不得发 HTTP、sleep、读写 state、选择 hat、启动 backend 或产生终止原因。
- **文件**：创建 `crates/ralph-cli/src/loop_runner/quota/decision.rs`；模块内表驱动/property tests。
- **验收测试**：严格 `>` 边界、单账号足够、双失败、weekly remaining=0、实测 weekly total=0/status=3/remaining=100、不同 interval/weekly deadline、失败 fallback 更早、全部无可信时间、过去时间防忙循环；D2–D8 的 table-driven decision 全覆盖。
- **需要拆分的单元测试**：账号贡献计算（只读 remaining percent，不从 count 推导）、weekly blocker、deadline 候选、最早值、stale-deadline fallback、一次 pool decision 的摘要脱敏。
- **Red 预期失败原因**：尚无池决策器，或错误使用 `>=`、等待最晚窗口、把未知额度算入池。
- **最小实现范围**：无副作用决策模型；不做计时或重试。
- **集成验证**：Unit 2 的两个 fixture observation 直接送入 decision，证明 client→decision 契约兼容。
- **回归范围**：quota client tests + decision tests；property test 验证 sum/threshold/deadline 不变量。
- **TDD 闭环**：①先写 S3–S6 decision 验收；②确认边界因缺逻辑失败；③拆贡献/deadline 单测；④Red→Green→Refactor；⑤跑 client→decision 集成；⑥重跑 Unit 2 回归；⑦不留未解释 golden 更新；⑧方可进入 Unit 4。
- **完成标准**：相同输入永远得到相同决策，所有边界由测试固定。
- **风险与注意事项**：百分比必须有限且在范围内；过期服务端时间不能形成立即重试死循环。

### Unit 4：prompt 前 gate、可中断等待与 active-runtime 暂停

- **Unit 目标**：在真实 hat selection 与 prompt 构建之间执行 quota gate；不足时不消费 activation，等待/重查，充足时只放行一次。
- **对应 Scenario**：S1、S4、S5、S8、S9（内部 exhausted outcome）、S13（disabled）。
- **外部可观察结果**：生产 gate 严格按 A→B 各查询一次并形成一个 pool decision；等待前后仍是同一 hat/pending batch；deadline 前 prompt/backend 零调用；等待不增加 iteration/failure/max-runtime active elapsed。
- **输入与输出**：输入为 activation lease（selected hat + selection provenance + pending batch identity）、quota decision source、clock、控制信号和独立预算；输出为 Proceed、Stopped、Restart、WorkspaceGone 或 QuotaWaitBudgetExhausted outcome 及 paused duration。Proceed 必须原样携带 lease，禁止重新 selection。
- **可依赖的已完成能力**：Unit 1–3；现有 `next_hat()`、`build_prompt()`、stop/restart/workspace 检查。
- **明确禁止依赖的未来能力**：不得处理跨进程 continue、backend quota signature、accepted event、isolated channel commit 或最终 UI 文案。
- **文件**：创建 `crates/ralph-cli/src/loop_runner/quota/wait.rs`、`crates/ralph-cli/src/loop_runner/quota/gate.rs`、`crates/ralph-cli/src/loop_runner/quota/activation.rs`；修改 `crates/ralph-cli/src/loop_runner/runner.rs` 的 selection/pre-hook 区域和最小内部依赖注入 seam；修改 `crates/ralph-core/src/event_loop/loop_state.rs`；按最小需要为 `crates/ralph-core/src/event_loop/mod.rs` 与 `crates/ralph-proto/src/event_bus.rs` 暴露只读 pending/cursor identity；创建 `crates/ralph-cli/src/loop_runner/tests/quota.rs`，并在 `crates/ralph-cli/src/loop_runner/tests/mod.rs` 注册。
- **验收测试**：disabled zero-call；20→wait→21 后同一 hat 启动；recovery pin 与 priority handoff selection 等待后不丢失；deadline/control 同 tick；stop/restart/workspace-gone；max wait/rechecks；虚拟等待 5 小时但 active elapsed 不增长；pending queue 在等待期间未被 take；`PostIterationStart` hook、RPC/display iteration、prompt logger、hat channel 和 backend probe 在等待时均为零调用，放行后各一次。
- **需要拆分的单元测试**：activation lease 构造与稳定 identity、wait 状态机、signal precedence、recheck 计数、paused-duration accumulator、LoopState active elapsed、重复 Proceed 防护。
- **Red 预期失败原因**：runner 当前会直接 build prompt/spawn，LoopState wall-clock 会把等待算入 max runtime。
- **最小实现范围**：在 quota facade 内串行调用 Unit 2 两次并交给 Unit 3；initial check 不增加 recheck，只有等待后的完整双账号 round 增加一次 recheck。`next_hat()` 返回后立即生成 activation lease，再先过 quota gate，最后才进入现有 `PostIterationStart` hook → prompt → channel/env → spawn 链；为 `LoopState` 增加累计 quota pause，并让 `elapsed()` 返回 wall elapsed 减 pause 的饱和值；production `run_loop_impl` 公开签名保持不变。
- **集成验证**：真实 EventLoop pending queue + 可计数 lifecycle hook + fake quota facade + fake backend probe，证明 gate 的实际插入顺序和每个副作用的一次性；另以 `max_runtime_seconds` 很小、虚拟 quota wait 很长的场景证明 core termination check 读取的是 active elapsed。
- **回归范围**：loop_runner legacy/fake-path、max-runtime、next-hat fairness、handoff timeout、disabled default path。
- **TDD 闭环**：①先写 runner acceptance；②确认 backend 过早启动/elapsed 增长；③拆 wait/clock 单测；④Red→Green→Refactor；⑤跑真实 EventLoop 集成；⑥跑 `ralph-cli` 受影响回归；⑦确认无 sleep 型脆弱测试；⑧方可进入 Unit 5。
- **完成标准**：同进程等待闭环可独立工作，且没有消费或改变尚未放行的 activation。
- **风险与注意事项**：gate 不得放在 `build_prompt()` 之后；不要复用 hook suspend 的固定轮询实现，应以 deadline timer 与控制信号选择。

### Unit 5：durable checkpoint 与 `--continue` 恢复

- **Unit 目标**：把 waiting/pending activation 以版本化、原子、owner-only checkpoint 持久化，并在同 loop/workspace 的 continue 中恢复。
- **对应 Scenario**：S7、S8、S10、S11（state 基础）、S14（state）。
- **外部可观察结果**：进程退出再启动后仍恢复同一 hat、该 hat 待消费事件和本 activation 会消费的 human guidance 批次；过期 deadline 立即重查；损坏/错属 state fail-closed。
- **输入与输出**：输入为 Unit 4 activation lease、pool summary、deadline、active-wait/recheck 计数、loop/workspace/config identity 和 checkpoint lifecycle；输出为 ContinueWaiting、Recheck、ReplayPending、IgnoreCompleted 或拒绝恢复。checkpoint lifecycle 至少区分 `waiting_before_prompt`、`pending_replay_after_backend`、`committed`，禁止用布尔值混淆阶段。
- **可依赖的已完成能力**：Unit 4 pre-prompt selection/gate；`SuspendStateStore` 的版本化与 temp+rename 模式；现有 `--continue` 入口。
- **明确禁止依赖的未来能力**：不得识别 backend quota、决定 accepted-event commit、修改普通 next-hat 算法或把完整 prompt/backend output 写盘。
- **文件**：创建 `crates/ralph-cli/src/loop_runner/quota/state.rs`；修改 `crates/ralph-cli/src/loop_runner/runner.rs` 的 EventLoop 构造后/通用 resume 初始化前分支，必要时修改 `crates/ralph-cli/src/commands/run.rs` 以便把 `--continue` identity 原样传入；为 `crates/ralph-core/src/event_loop/mod.rs` 和 `crates/ralph-proto/src/event_bus.rs` 增加 activation lease restore 契约及对应模块测试；扩展 `crates/ralph-cli/tests/integration_resume.rs` 覆盖真实 `ralph run --continue --loop-id` 分流。
- **验收测试**：checkpoint round-trip；hat+selection provenance+pending events+task identity+iteration/attempt 一致；stop 保留、restart 保留、committed 清理且不重放；deadline 未到/已过；未知版本、损坏 JSON、loop/workspace/config digest/hat mismatch；quota checkpoint 存在时不注入 generic resume event；无 checkpoint 时现有 continue event 序列完全不变；Key 不在 JSON/错误。
- **需要拆分的单元测试**：schema/version/lifecycle、配置摘要只覆盖非 secret 调度字段、same-directory temp + file fsync + rename + parent-dir fsync、失败临时文件清理、0600 权限（Unix）、resume decision、activation lease restore 不重复 fan-out/不重复推进 cursor、checkpoint cleanup 幂等。
- **Red 预期失败原因**：当前没有 quota checkpoint，`--continue` 只能依赖现有 scratchpad/event replay，不能保证原 selection。
- **最小实现范围**：selection 后、任何 activation lifecycle 副作用前先保留 lease 于内存；只在进入 Wait 或后续 Unit 7 的 PendingReplay 时持久化。恢复顺序固定为：读取 → schema/secret-free parse → loop/workspace/config/hat 校验 → 构造 EventLoop → 恢复 exact pending batch 与 cursor/pin 语义 → 按 deadline 决定继续等或 fresh recheck；只有确认没有 quota checkpoint 才执行当前 generic `initialize_resume*`。本 Unit 先实现 `waiting_before_prompt`，为 `pending_replay_after_backend` 保留 schema 但不接执行分类。
- **集成验证**：用两个新建 EventLoop 实例模拟进程重启，第二个实例经 continue 恢复同一 activation；再通过 `integration_resume.rs` 驱动真实 CLI 入口，断言 quota 与 generic resume 分流互斥。不得手工编辑 runtime state，测试通过 store API 建 fixture。
- **回归范围**：现有 `integration_resume`、scratchpad/loop-id 校验、stop/restart、EventBus routing/fairness。
- **TDD 闭环**：①先写 restart/continue acceptance；②确认恢复选错/无 state；③拆 store/identity 单测；④Red→Green→Refactor；⑤跑双实例恢复集成；⑥跑 continue/EventBus 回归；⑦确认 state 无 secret 和陈旧重放；⑧方可进入 Unit 6。
- **完成标准**：checkpoint 契约冻结，恢复不需要猜 hat，损坏或越权 state 不会运行。
- **风险与注意事项**：只持久化重建 activation 所需事件批次和元数据，不持久化 prompt/backend output；checkpoint 以 owner-only 权限保存可能敏感的事件 payload，Commit 后清理，stop/restart 保留。损坏或错属 state fail-closed，并在错误中给出通过正式 CLI 清理/重启的动作，不自动删除取证材料。Key 轮换后用新配置 fresh recheck，因此 state 不存 Key、Key hash、Authorization 或完整 quota config；配置 identity 只对预算、endpoint contract version、账号逻辑名等非 secret 调度字段做稳定摘要。

### Unit 6：CLI/PTY quota evidence 分类

- **Unit 目标**：从当前两条 executor surface 保留的完整 output/error evidence 中识别确认过的 MiniMax 套餐耗尽，且不改变普通错误分类。
- **对应 Scenario**：S11、S12、S13、S14（execution evidence）。
- **外部可观察结果**：同一 quota fixture 在 CLI/PTY 得到相同 typed classification；普通 429/watchdog/post-event timeout 与修改前差分一致。
- **输入与输出**：输入为独立的 execution evidence（raw/stripped backend text、exit code、PTY termination、CLI timeout/post-event-timeout、error chain）和用于事件解析的 normalized output；输出为 `ExecutionClassification::QuotaPlanExhausted` 或 `ExistingOutcome`，同时原样保留 normalized output 给现有事件解析。classifier 不得改变 `success`、watchdog 或 termination 字段本身。
- **可依赖的已完成能力**：Unit 1 enabled flag、Unit 2 脱敏错误约束；现有 `ExecutionOutcome`、CLI/PTY result。
- **明确禁止依赖的未来能力**：不得查询 usage、等待、写 checkpoint、读取 accepted events 或决定 commit/replay。
- **文件**：修改 `crates/ralph-cli/src/loop_runner/execution.rs` 的 outcome/evidence 表达和 `crates/ralph-cli/src/loop_runner/runner.rs` 的 CLI/PTY 收敛点；扩展 `crates/ralph-cli/src/loop_runner/tests/legacy.rs` 的 executor characterization，并在 `crates/ralph-cli/src/loop_runner/tests/quota.rs` 放分类验收；只有 characterization 证明现有返回值确实丢失证据时，才最小修改 `crates/ralph-adapters/src/cli_executor.rs` 或 `crates/ralph-adapters/src/pty_executor.rs` 及其模块测试。
- **验收测试**：用户脱敏 quota fixture；CLI 的 stdout/stderr 与 PTY raw/stripped/extracted 三种载体 parity；普通短时 429、401、网络错误、模型错误、一般 non-zero、watchdog、post-event timeout；quota signature 与 valid event 同时出现；disabled 时 classifier bypass 且不扫描 evidence。
- **需要拆分的单元测试**：signature matcher、evidence source precedence/去 ANSI/大小上限、false-positive corpus、redacted classification Debug、normalized event output 与 classification evidence 不串线。
- **Red 预期失败原因**：当前 `ExecutionOutcome` 只有 success/termination/watchdog，没有 typed quota evidence，部分 `?` 路径可能提前丢失 error chain。
- **最小实现范围**：为 `ExecutionOutcome` 增加 bounded、不可泄密的 classification evidence，CLI 取完整 accumulated stdout/stderr，PTY 至少保留 stripped raw evidence；现有 `output` 继续只服务事件解析。signature matcher 必须窄匹配 fixture 中稳定的供应商错误 code/结构组合，不允许仅匹配 `429`、`quota`、`rate limit` 或自然语言片段。分类只在 quota enabled 时运行，分类完成后不得把 evidence 写入 diagnostics/checkpoint/prompt。
- **集成验证**：真实 fake CLI process 和 PTY fixture 产生非零退出，证明 production executor output 到 classifier 的链路。
- **回归范围**：adapter executor suites、loop_runner watchdog/post-event timeout/ordinary failure tests。
- **TDD 闭环**：①先加 characterization 和 quota parity 验收；②确认 typed 分类缺失；③拆 matcher 单测；④Red→Green→Refactor；⑤跑 executor integration；⑥跑 adapter+CLI failure 回归；⑦确认无宽匹配；⑧方可进入 Unit 7。
- **完成标准**：classifier 只回答“是否是已确认套餐耗尽”，不承担恢复策略且零 false-positive fixture。
- **风险与注意事项**：实际 signature 尚需用户提供脱敏样本；没有样本不得用猜测实现 Green。

### Unit 7：accepted-event 提交点与 cold replay

- **Unit 目标**：在真实 runner post-execution 顺序中，以本 activation 的 runtime accepted events 决定提交或持久化重放。
- **对应 Scenario**：S11、S12、S13、S14（backend env/prompt）。
- **外部可观察结果**：quota 后已有 accepted event 只提交一次；无 accepted event 不计普通失败并在 fresh recheck 后重放同一 activation。
- **输入与输出**：输入为 Unit 6 classification、Unit 5 activation lease、activation-local channel batch 和 activation-start reader/state cursor；输出为 `Commit(prepared_events)`、`PendingReplay(clean_lease)` 或 `ExistingFailure`。`prepared_events` 必须来自 canonical runtime parser/policy/contract pipeline，不能由字符串匹配或 `ralph emit --policy-check` 结果代替。
- **可依赖的已完成能力**：Unit 1–6 的冻结契约、`ProcessedEvents.accepted_events`、isolated channel merge、checkpoint restore。
- **明确禁止依赖的未来能力**：不得重写 client/decision/state 内部算法、扩大 quota signature、修改 preset/hat instructions。
- **文件**：修改 `crates/ralph-cli/src/loop_runner/runner.rs` 的 backend-return 到 `process_output` 区域；重构 `crates/ralph-cli/src/loop_runner/hat_channel.rs`，把“读取/authoritative stamp”和“append/cleanup”拆成 prepare 与 commit/discard；为 `crates/ralph-core/src/event_loop/mod.rs` 增加 activation-local prepare/commit API，并在 `crates/ralph-core/src/event_loop/tests/` 新增提交边界测试文件；扩展 `crates/ralph-cli/src/loop_runner/tests/quota.rs`，必要时先在 `crates/ralph-cli/src/loop_runner/tests/legacy.rs` 固化非 quota 顺序。
- **验收测试**：quota+activation-owned accepted business/terminal event→Commit；quota+仅 internal/diagnostic event 或 empty/rejected/malformed/partial channel→PendingReplay；prepare 阶段不追加主/candidate JSONL、不发布 EventBus、不更新 projection/phase/recovery/contract budget、不写 recovery side artifact；Commit 时按原顺序只应用一次；Discard 后 channel/marker 清理、主 reader cursor 不变；至少 fallback cooldown+双账号 fresh query；同 hat/event/task/iteration/attempt；恢复提示要求检查 workspace；quota Key 不进入 backend env/prompt。
- **需要拆分的单元测试**：authoritative stamp 的纯转换、canonical validation 的 prepared result、prepare 无副作用、commit 幂等保护、discard 幂等、activation-local accepted snapshot 判定、freshness requirement、replay prompt metadata（断言结构字段/行为，不锁精确文案）。
- **Red 预期失败原因**：runner 当前会先把 isolated channel append 到主/candidate JSONL，再由 `process_output` 更新 iteration/failure，之后才取得 `accepted_events`；没有无副作用 prepare、没有 activation lease restore，也没有 replay 分支。
- **最小实现范围**：
  1. backend 返回后先基于 Unit 6 evidence 分类，不调用 `process_output`；
  2. 非 quota outcome 完全走当前 merge → process_output → process-events 顺序，作为差分基线；
  3. typed quota outcome 读取 activation channel 为内存 batch，做 authoritative stamp，并通过 core 的 canonical prepare API得到 accepted/rejected/malformed 结果，但暂不写主事件流、不应用 EventBus/state mutation；
  4. 若存在本 activation accepted 业务/终态 event，则 Commit prepared batch，再执行一次现有 output/iteration 收尾；即使 backend non-zero，也不得把已提交 activation 重新计为普通失败；
  5. 若不存在提交点，则 Discard prepared batch，恢复 Unit 5 lease 所代表的 pending batch/cursor/pin，保持 iteration/failure/fallback/recovery/phase/projection 不变，写 `pending_replay_after_backend` checkpoint；
  6. replay 必须先等待至少一个 fallback 周期，再重新查询两个账号；放行后 build 新 prompt，并注入结构化恢复提示，提示 agent 检查 workspace/task/scratchpad 的既有副作用；不得复用旧 prompt 或 backend session。
- **集成验证**：production runner + fake quota facade + fake backend + real EventLoop canonical policy/channel，分别断言 Commit/Discard 前后的主 JSONL、EventBus pending、projection、phase、recovery budget、iteration/failure、channel marker、事件次数和 replay identity；再做一次 non-quota differential，证明原顺序与产物未变化。
- **回归范围**：missing-event/hard-gate、execution contract rejection、text fallback、isolated one-event budget、normal completion、post-event timeout。
- **TDD 闭环**：①先写 S11/S12 runner acceptance；②确认现有顺序导致失败/重复；③拆 disposition 单测；④Red→Green→Refactor；⑤跑 real runner integration；⑥跑完整 loop_runner 相关回归；⑦确认非 quota differential 一致；⑧方可进入 Unit 8。
- **完成标准**：accepted event 成为唯一提交点，未提交 activation 可重复恢复而不污染普通恢复预算。
- **风险与注意事项**：accepted snapshot 必须来自 activation lease 对应 batch 的 canonical prepare result，不能使用全局历史、main events tail、grep output 或 agent 自报；channel 被 policy 拒绝、loop-internal 或 diagnostic event 都不算提交。prepare/commit 不得复制第二套 policy 实现：应把现有 parse/validate/apply 路径拆成可延迟 apply 的共同内核，否则普通路径与 quota 路径会长期漂移。

### Unit 8：专用终止原因与全可观察面脱敏

- **Unit 目标**：把 quota wait/recheck 上限、state 恢复错误和控制结果一致映射到 operator-facing termination/summary/sentinel/RPC，并验证全产物无 Key。
- **对应 Scenario**：S7、S8、S9、S10、S14。
- **外部可观察结果**：`QuotaWaitBudgetExhausted` 可读、可区分 wait/recheck limit，且不冒充 Unit 6 的供应商 `QuotaPlanExhausted` 或其它预算；RPC 沿用现有 `Error` 粗粒度，不新增 proto 变体；resume hint 与 stop/restart/workspace 状态一致。
- **输入与输出**：输入为 Unit 4/5/7 outcomes；输出为 typed `TerminationReason::QuotaWaitBudgetExhausted { limit_kind, active_wait_seconds, rechecks }` 语义、exit code、display/summary/sentinel/RPC event。字段名称可随现有 style 调整，但三项诊断信息不得丢失。
- **可依赖的已完成能力**：Unit 1–7；现有 termination hook/summary/display/subprocess sentinel 管线。
- **明确禁止依赖的未来能力**：不得修改 quota decision/replay、增加 CLI 命令或引入新的 RPC 协议枚举。
- **文件**：修改 `crates/ralph-core/src/event_loop/types.rs`、`crates/ralph-core/src/event_loop/termination_impl.rs`、`crates/ralph-core/src/summary_writer.rs`、`crates/ralph-cli/src/display.rs`、`crates/ralph-cli/src/commands/run.rs`、`crates/ralph-cli/src/loop_runner/runner.rs`、`crates/ralph-tui/src/rpc_source.rs` 及各自邻近测试；用 `rg 'TerminationReason::'` 重新枚举消费者，若出现新文件必须纳入本 Unit。
- **验收测试**：wait/recheck 两种 exhausted；Stopped/Restart/WorkspaceGone；resume hint；sentinel round-trip；parent exit code；summary/status；RPC generic Error；对所有 artifacts 执行 Key 哨兵断言。
- **需要拆分的单元测试**：termination status text、success/exit mapping、resume hint、serde round-trip、redacted diagnostic payload。
- **Red 预期失败原因**：现有 `TerminationReason` 无 quota wait-budget variant，各 exhaustive match 不完整或会落入含糊 generic 文案。
- **最小实现范围**：新增一个 typed quota wait-budget reason及其所有现有消费者；供应商套餐耗尽只作为可恢复 execution classification，不直接作为 loop termination；state corruption 仍以可操作 error 返回，不伪装 quota wait exhausted。
- **集成验证**：runner 触发 exhausted 后收集 sentinel、summary、display/RPC 测试输出并做一致性/secret scan。
- **回归范围**：全部 termination variants、subprocess TUI reason resolution、summary writer、hook termination、RPC/TUI generic mapping。
- **TDD 闭环**：①先写 S9/S14 输出面验收；②确认缺 variant/match；③拆 formatter/serde 单测；④Red→Green→Refactor；⑤跑 runner termination integration；⑥跑 core+CLI termination 回归；⑦确认无遗漏 match/secret；⑧方可进入 Unit 9。
- **完成标准**：所有可观察面语义一致，RPC 保持兼容，Key 哨兵在任何产物中零命中。
- **风险与注意事项**：`TerminationReason` 消费点很多，必须用 `rg` 枚举而不是只修编译报错；不要把 checkpoint 内部路径写入 agent prompt。

### Unit 9：用户/agent 文档同步与最终 acceptance 固化

- **Unit 目标**：让操作者能正确配置、诊断和恢复，让 cold-restarted agent 知道先检查现有副作用，并固化所有 Scenario 的可执行入口。
- **对应 Scenario**：S1–S14（文档与总验收）。
- **外部可观察结果**：配置字段/default、认证边界、等待/终止/continue 语义与实现一致；agent guide 只描述可执行动作，不泄漏 runtime 内部 ledger。
- **输入与输出**：输入为 Unit 1–8 已冻结契约；输出为用户文档、agent guide、BDD/ATDD trace 和通过的最终 acceptance suite。
- **可依赖的已完成能力**：全部前置 Unit。
- **明确禁止依赖的未来能力**：不得在本 Unit 补生产逻辑、改变冻结契约、修改 builtin preset/schema/zsh completion 或新增 CLI。
- **文件**：修改 `docs/guide/configuration.md`、`docs/guide/cost-management.md`、`docs/guide/backends.md`、`docs/reference/troubleshooting.md`；更新 `crates/ralph-core/data/ralph-tools-recovery-directives.md` 的通用 cold-replay 动作，并仅在需要增加发现入口时最小更新 `crates/ralph-core/data/ralph-tools.md`；不改 `crates/ralph-core/data/ralph-tools-cmdref.md`，因为没有命令语法变化。同步 `skills/ralph-preset-common/references/agent-native-model.md`、`skills/ralph-preset-common/references/author-checklist.md`、`skills/ralph-preset-common/references/patterns.md`，明确 quota 是 loop 外 operator 配置、secret 不得进入 preset/hat instructions、quota replay directive 是 runtime 注入上下文；完善 `crates/ralph-cli/src/loop_runner/tests/quota.rs` 的 Scenario 命名/追踪注释。
- **验收测试**：字段/default 与 serde contract 一致；文档明确 Subscription Key 仅查询、不注入 backend、私有配置需 owner-only 且不得提交版本库，并解释 Purchased Credits 不在决策内；stop/continue、restart、workspace-gone、上限与恢复提示；agent guide 仅写触发条件、agent 应执行的 workspace/task/scratchpad 检查、字段来源和停止条件，不出现内部函数名、源码行号、ledger/checkpoint 路径或计划专用编号；operator skill 负面 fixture 仍能识别“把 secret/quota 内部实现复制进 hat instructions”为违规。
- **需要拆分的单元测试**：无新增业务单元；文档契约由 config fixture、CLI doc drift、Scenario trace 完整性和 secret scan 验证。
- **Red 预期失败原因**：现有文档没有 quota 配置/恢复语义，agent guide 不知道 cold-restart 后应检查副作用。
- **最小实现范围**：文档和验收索引；若验收暴露生产缺陷，返回责任 Unit 修复，不能在 Unit 9 就地改算法。
- **集成验证**：运行全部 S1–S14 acceptance、`scripts/check-cli-doc-drift.sh`、agent skill doc 反向检查，并按 `ralph-preset-review` 对 `skills/ralph-preset-common/fixtures/aaf-review-negative-fixture.yml` 或等价场景重跑说明；`commands.md` 与 finding rubric 因无 CLI/lint finding 变化只需复核并记录“不需修改”的证据。
- **回归范围**：现有 mock E2E、所有 docs/static checks、core/CLI/adapters 相关 suites。
- **TDD 闭环**：①先让文档/trace 检查暴露缺口；②确认因缺章节/契约漂移失败；③补最小文档断言；④Red→Green→Refactor 文档结构；⑤跑 acceptance integration；⑥跑全受影响回归；⑦无新增 skipped/ignored 后关闭；⑧进入最终质量门禁。
- **完成标准**：Coding Agent 不需要猜配置、测试或恢复行为；文档不包含实现内部细节且与代码一致。
- **风险与注意事项**：`crates/ralph-core/data/*.md` 必须从 agent 下一步动作写，不写计划编号、内部 state 路径、函数名或一次性事故背景；hat instructions 不复制 recovery skill 内容。preset operator 文档面向 loop 外作者/评审，可以说明配置边界，但同样不得放真实 Key 或把供应商专用行为误写成 builtin preset 通用拓扑要求。

## 6. 最终质量门禁

最终门禁不是补实现的 Unit。任何失败都必须回到责任 Unit，重新完成该 Unit 的 Red→Green→Refactor、集成与回归，再按顺序复验后续 Unit。

- [ ] S1–S14 全部通过，需求—测试追踪矩阵无空白，关键 runner acceptance 明确证明“不 spawn”“同一 activation 重放”“accepted event 不重放”。
- [ ] 所有新增及受影响单元测试通过；property/state-machine/fault-injection 测试通过且无随机 flake。
- [ ] 所有必要的集成/契约测试通过：本地 MiniMax HTTP contract、client→decision、pre-hook quota gate、双实例/真实 CLI continue、CLI/PTY evidence、real EventLoop activation prepare/commit/discard、termination artifacts。
- [ ] 关键 E2E 通过：S7/S11/S12 的 production runner acceptance 通过；现有 `cargo run -p ralph-e2e -- --mock` 通过。线上 MiniMax 调用不是验收条件。
- [ ] `cargo nextest run -p ralph-core -- <quota/config/termination subsets>`、`cargo nextest run -p ralph-adapters -- <executor subsets>`、`cargo nextest run -p ralph-cli --bin ralph -- <quota/runner subsets>` 均按 nextest 默认并发通过；不得恢复 `cli-serial` override，不得裸跑 ralph-cli 的 `cargo test`。
- [ ] `cargo fmt --check`、`cargo clippy`、`cargo build` 和必要 doctest 通过；命令语法/文档检查 `scripts/check-cli-doc-drift.sh` 通过。
- [ ] 最终运行 `./scripts/run-tests.sh`；仅明确的时序 flake 才可按仓库规则用 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 复核，serial 仍失败即为真失败。
- [ ] 没有新增失败、ignored、skip、`.only`、削弱断言、无解释 snapshot/golden 更新或 mock 掉 production runner 行为。
- [ ] disabled/default 差分证明：HTTP 0 次、quota state I/O 0 次、prompt/事件/termination/backend env 与基线一致，且无可测启动延迟。
- [ ] 两个哨兵 Key 在日志、TUI/RPC、summary、diagnostics、sentinel、checkpoint、prompt、backend env、错误/Debug 中均零命中。
- [ ] `crates/ralph-core/data/ralph-tools-recovery-directives.md`（及必要时 `ralph-tools.md`）与用户文档反向核对完成；`scripts/check-cli-doc-drift.sh` 通过；确认不需要修改 `ralph-tools-cmdref.md`、builtin preset/schema、zsh completion。
- [ ] preset operator skills 已完成反向检查：`agent-native-model.md`、`author-checklist.md`、`patterns.md` 与新 `event_loop.quota`/cold replay 边界一致，AAF negative fixture review 仍成立；`commands.md` 与 `finding-rubric.md` 的“不需修改”结论有记录。
- [ ] 未验证内容明确记录：成功 response schema 已由 2026-07-16 实测 fixture 固化；运行中 quota error signature 与明确周耗尽 status 仍需新的脱敏 fixture。两者未取得前不得猜 matcher/status，也不得把对应 Unit 标为完成。
- [ ] 剩余风险明确接受：两账号等容量与 Proxy 聚合是部署前提；套餐/官方响应未来可能漂移；固定 20% 不能保证单次 activation 一定完成，运行中 cold replay 是兜底而非精确配额预测。
