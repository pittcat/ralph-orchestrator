---
title: "feat: MiniMax 双 Coding Plan 配额门控与中断续跑"
type: feat
status: active
date: 2026-07-15
updated: 2026-07-15
deepened: 2026-07-15
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: user-confirmed
---

# MiniMax 双 Coding Plan 配额门控与中断续跑 - Plan

## 1. 功能目标

### 业务目标

- Ralph 通过既有 Proxy 使用两个 MiniMax 中国区订阅账号；Proxy 继续负责真实模型请求的账号路由，Ralph 只查询两个账号的套餐额度。
- 每次串行 hat activation 启动前，把两个账号当前 5 小时窗口的可用百分比汇总成一个池；只有池量严格大于单次 activation 预算（默认 20%）才启动 backend。
- 池量不足时，Ralph 不消耗触发事件、不构建 prompt、不启动 backend，而是等到最早可信刷新时间后重查；运行中命中套餐额度耗尽时，按本 activation 是否已经产生 runtime accepted event 决定提交或冷启动重放。
- 配额等待、stop/restart、跨进程 `--continue` 和恢复上限均可诊断、可恢复且不泄露两个订阅 Key。

### 本次范围

- MiniMax 中国区 `GET https://www.minimaxi.com/v1/token_plan/remains`，每个账号使用独立 Subscription Key/Bearer 认证。
- 恰好两个等容量套餐账号；串行查询 A 后查询 B；成功 observation 汇总，失败账号按未知且贡献 0 处理。
- 用户确认的当前响应契约：从 `model_remains` 中读取 `model_name == "general"` 的 5 小时/周额度和刷新时间。实现前必须先用脱敏实测 fixture 固化该契约。
- 默认预算 20%，仅当 `pool_remaining_percent > activation_budget_percent` 时启动；等于预算时等待。
- 等待 deadline、fallback 重查、`max_wait_seconds`、`max_rechecks`、stop/restart/workspace-gone、active-runtime 暂停计时。
- 运行中 quota signature 分类、accepted event 提交点、未提交 isolated channel 丢弃、原 activation 持久化和 cold-restart。
- CLI runner、termination sentinel、summary/display/diagnostics、配置文档和 agent 注入指南同步。

### 非目标

- 不支持国际区 `minimax.io`、其它供应商、任意账号数、不同容量套餐加权、并发 hats、wave worker 预留、跨 loop 全局配额账本。
- 不让 Ralph 替 Proxy 选账号或切换 backend Key；不修改 `ANTHROPIC_AUTH_TOKEN`、`ANTHROPIC_API_KEY`、Proxy URL 或现有 backend 环境变量。
- 不按 token 精确预测消耗，不自动回滚 quota 中断前的代码/task/memory/scratchpad 副作用。
- 不把普通 RPM/TPM 限流、任意 `429`、网络失败或认证失败一概当作 5 小时套餐耗尽。
- 不新增用户 CLI 命令，不修改 preset、preset schema、hat topology、zsh completion 或 preset operator skills。
- 不在测试中调用线上 MiniMax，不把所有验收场景升级成 E2E。

### 需求编号

- **R1 配置与 opt-in**：缺少 quota、`enabled: false` 或未配置账号时，旧配置和旧 runner 行为完全不变；启用时必须恰好两个唯一命名、非空 Key 的账号。
- **R2 查询契约**：分别查询两个账号，只接受 HTTP/JSON/业务状态均有效且存在 `general` 条目的 observation；请求有超时、响应体大小上限且凭据不跨 host 转发。
- **R3 池化判断**：两个有效账号的 5 小时可用量求和，周额度为 0 的账号贡献 0；总量严格大于预算才允许 activation。
- **R4 智能等待**：不足时取每个账号可用候选 deadline 的最早值并加 buffer；查询失败或无可信未来窗口时使用 fallback，过期/陈旧 deadline 不得形成忙循环。
- **R5 独立预算与时间**：一次双账号查询只计一次 recheck；quota active wait 不增加 iteration/failure/fallback/recovery，也不计入 `max_runtime_seconds`，但受自身等待与重查上限约束。
- **R6 人工与工作区状态**：等待期间 stop 保留 pending state 并以 `Stopped` 退出，restart 保留 state 并沿用现有自动重启，workspace-gone 终止且不承诺恢复。
- **R7 运行中分类**：仅命中经脱敏 fixture 固化的 MiniMax 套餐耗尽 signature 才进入 quota 分支；普通错误保持现有路径。
- **R8 activation 提交点**：本 activation 归属的 runtime accepted 业务事件或终态事件是唯一提交点；loop-internal/diagnostic event 不构成提交。已有提交事件不重放，无提交事件则不计普通失败并重放同一 activation。
- **R9 跨进程恢复**：quota checkpoint 保存同一 loop/workspace、hat、尚未消费的触发事件批次、task 上下文、iteration/attempt、deadline 和预算计数；`--continue` 校验状态归属后恢复。
- **R10 新鲜度与副作用**：未提交 activation 至少冷却一个 fallback 周期并重新查询两个账号；保留工作区副作用，清理未提交 isolated channel，恢复 prompt 要求先检查现状。
- **R11 凭据与认证边界**：Key 不进入 Debug/Display、日志、TUI/RPC、summary、diagnostics、termination sentinel、quota checkpoint、prompt 或 backend env。
- **R12 操作文档与零回归**：配置、恢复语义和 agent 可见动作有文档；disabled/default 路径无 HTTP、quota 文件 I/O、额外延迟、环境变量或错误分类变化。

### 已知约束和假设

- 源码现状已确认：`crates/ralph-cli/src/loop_runner/runner.rs` 在 `next_hat()` 后调用 `EventLoop::build_prompt()`；后者会消费 pending events。因此 pre-activation gate 必须位于 hat 选择之后、prompt 构建之前，等待期间只冻结 selection，不得消费事件或清除 handoff obligation。
- `EventLoop::next_hat()` 会推进 isolated round-robin cursor，但不会消费 pending queue；quota checkpoint 必须记录 selection 和待消费事件批次，跨进程恢复不得再次调用一般选择算法来猜 hat。
- `crates/ralph-core/src/event_loop/loop_state.rs` 的 `elapsed()` 当前只使用 `started_at.elapsed()`；需要显式累计 quota pause，且保持其它 runtime limit 语义不变。
- 当前 CLI/PTY 两条存续执行面都汇总 backend output；先做 characterization test 证明非零退出/错误证据不会被丢弃，再引入 quota classifier。删除的 ACP production path 不在范围内。
- 官方文档已把旧 Coding Plan 表述为 Token Plan，并说明额度是统一池；中国区仍公开 remains endpoint、5 小时和周窗口，但没有公开承诺本计划依赖的响应字段。Unit 2 若发现脱敏 fixture 不再包含 `model_remains/general`，必须停止并回到规格修订，禁止猜字段、回退到“汇总所有模型”或静默 fail-open。参考 [MiniMax 中国区 Token Plan FAQ](https://platform.minimaxi.com/docs/token-plan/faq)。
- 两账号百分比求和成立的业务前提是套餐容量相同且 Proxy 确实聚合两者；不满足时不得启用。
- 目标账号不依赖 Purchased Credits 或 pay-as-you-go 余额继续运行；MiniMax 官方说明 Credits 可在套餐额度后自动承接用量，因此若部署实际依赖 Credits，本 gate 可能保守阻塞，本期不得启用。Ralph 不查询或估算 Credits。
- 选择在 Ralph 而非 Proxy 实现恢复，是因为 Proxy 只能路由模型请求，无法观察 runtime accepted event、hat/trigger identity、isolated channel 和 `--continue` 状态；仅在 Proxy 重试会造成重复交接或错 hat。
- Subscription Key 允许按用户确认直接放入私有 YAML；本期不迁移 keychain。操作文档必须要求配置文件 owner-only、不得提交版本库，并说明 Key 轮换只需更新配置；runtime 产物仍禁止复制 Key。
- 两次查询严格串行，单次 gate 最坏网络等待不超过两个账号 timeout 之和且没有隐藏 HTTP retry；这是用户选择的可预测延迟换安全判断，timeout 默认值和实际延迟必须可观测。
- `max_wait_seconds` 只累计 Ralph 实际处于 quota wait 的时间；进程已停止/离线时间不计入该上限。恢复时若 wall-clock deadline 已过则立即重查，`max_rechecks` 跨进程累计。
- 同一时刻出现多个控制信号时沿用现有控制优先级；deadline 不得压过已经可观察到的 stop/restart/workspace-gone。
- 外部网络只用于运行时查询；测试使用本地 mock transport/server，并遵循 `docs/solutions/test-failures/reqwest-no-proxy-loopback-test-failures.md` 的 loopback no-proxy 约束。
- 测试入口严格遵循 AGENTS.md：`ralph-cli` 用 nextest cli-serial，禁止裸跑 `cargo test -p ralph-cli`。

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
- **Then** T 前不构建 prompt、不启动 backend；重查后启动原 hat/原事件批次，等待不增加 iteration/failure 且不消耗 max runtime

#### Scenario S5：单账号查询失败按未知处理

- **Given** A 查询超时，B 有效剩余 25% 或 15%
- **When** 形成池决策
- **Then** B=25% 时仍启动；B=15% 时等待，并优先按 A 的 fallback deadline 重查而不是等待 B 的较晚窗口

#### Scenario S6：无效响应、周耗尽和陈旧窗口 fail-closed

- **Given** 响应为 401/429/5xx、业务状态失败、畸形 JSON、缺少 `general`、百分比非法、周额度为 0，或窗口时间已过但额度未刷新
- **When** client 和决策器处理 observation
- **Then** 无法证明的额度贡献 0；周耗尽使用周刷新时间；陈旧时间最多触发一次立即重查，随后使用 fallback，绝不零延迟自旋

### Feature: 可中断等待与跨进程恢复

#### Scenario S7：stop 后只有显式 continue 才恢复

- **Given** activation 正在 quota wait，checkpoint 已保存且未消费触发事件
- **When** 操作者发出 stop，之后执行 `ralph run --continue --loop-id <same-loop>`
- **Then** 第一次运行以 `Stopped` 退出并保留脱敏 state；continue 校验 loop/workspace/config 后按 deadline 等待或重查，仍恢复同一 activation

#### Scenario S8：restart 与 workspace-gone 保持现有控制语义

- **Given** activation 正在 quota wait
- **When** 收到 restart 或检测到 workspace-gone
- **Then** restart 保留 state 并交给现有自动重启路径；workspace-gone 终止且不声称可以恢复，不启动 backend

#### Scenario S9：quota 自身预算耗尽给出专用终止原因

- **Given** 累计 active wait 达到 `max_wait_seconds`，或双账号 recheck 次数达到 `max_rechecks`
- **When** gate 尝试继续等待/重查
- **Then** 以专用 quota exhausted 原因终止；不冒充 MaxRuntime/MaxCost/ConsecutiveFailures，sentinel、summary、display 和 RPC 的现有 generic-error 映射一致

#### Scenario S10：损坏或不属于当前运行的 state 不得重放

- **Given** checkpoint schema 未知、JSON 损坏、loop_id/workspace 不匹配、hat 已不存在，或 continue 时 quota 已关闭
- **When** Ralph 尝试恢复
- **Then** fail-closed 并给出可操作错误，不查询 usage、不选择新 hat、不运行陈旧 activation

### Feature: 运行中配额中断的提交与重放

#### Scenario S11：无 accepted event 时冷启动重放同一 activation

- **Given** backend 命中确认过的 MiniMax quota signature，当前 activation 没有 runtime accepted event，但工作区可能已有部分修改
- **When** runner 完成 isolated channel 验收并分类 execution evidence
- **Then** 不增加普通失败/iteration/fallback/recovery，丢弃未提交 channel，持久化原 activation；至少冷却一个 fallback 周期并 fresh recheck 后，以恢复提示重启同一 hat/事件/task 上下文

#### Scenario S12：已有 accepted event 时提交且不重放

- **Given** backend 已写出一个通过 runtime policy 的业务或终态事件，随后命中 quota/non-zero exit
- **When** runner 先合并 channel并获得本 activation 的 accepted-event snapshot
- **Then** 该 activation 归属的 accepted 业务/终态事件作为提交点只路由一次，activation 正常收尾，不创建 pending replay；仅有 loop-internal/diagnostic event 时仍不得视为提交

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
| S9 | 两个上限分别产生 QuotaExhausted，并贯通输出面 | 状态机单测 + termination 集成 | 否 |
| S10 | schema/归属/config mismatch fail-closed | state-store/continue 集成 | 否 |
| S11 | quota+无 accepted event 不计普通失败，fresh recheck 后同一 activation 重放 | runner acceptance integration，fake quota + fake backend | 是，作为关键主路径验收 |
| S12 | activation-owned 业务/终态事件先验收后提交，只出现一次；internal/diagnostic event 不提交 | runner acceptance integration | 是，与 S11 共用最小场景 |
| S13 | 非 quota fixture 与现有错误行为差分一致 | adapter characterization + differential regression | 否 |
| S14 | 对所有产物执行哨兵 Key 断言，backend env 无 quota Key | 单元 + runner 安全集成 | 否 |

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
- **验收测试**：缺省/disabled 零差异；合法双账号默认值；账号数、重复名、空 Key、预算 1/199 边界及越界、timeout/上限为 0；错误与 Debug 的哨兵 Key 搜索。
- **需要拆分的单元测试**：secret wrapper Debug/Clone/accessor；quota enabled 判定；字段级 validation；disabled 不校验 accounts 的 characterization。
- **Red 预期失败原因**：serde 不认识 quota 字段或缺少校验/脱敏类型，非法配置被接受或 Key 出现在 Debug。
- **最小实现范围**：配置结构、默认值、只读 secret 表达、显式 validation；不添加运行时行为。
- **集成验证**：`RalphConfig::parse_yaml`/`validate` 走真实配置入口；合法旧 fixture 与新 fixture 均通过。
- **回归范围**：`ralph-core` config tests、所有 embedded preset parse/validate；确认无 preset 需要新增 quota。
- **TDD 闭环**：①先启用 S1/S2 验收；②确认因缺字段/校验正确失败；③拆 secret/default/validation 单测；④逐个 Red→Green→Refactor；⑤跑 config 集成；⑥跑 `ralph-core` 受影响回归；⑦无 ignored/削弱断言后关闭；⑧方可进入 Unit 2。
- **完成标准**：配置契约冻结，disabled 与缺省完全等价，任一错误产物不含 Key。
- **风险与注意事项**：不要派生明文 Debug；不要把完整 config 放入通用 diagnostics；无需修改 preset schema/operator skills。

### Unit 2：单账号 MiniMax remains 契约客户端

- **Unit 目标**：对一个中国区 Subscription Key 发出一次安全请求并产生一个脱敏 `AccountObservation`。
- **对应 Scenario**：S3、S5、S6、S14（HTTP 面）。
- **外部可观察结果**：有效 fixture 得到 `general` observation；所有协议/解析失败变成账号级 unavailable，且请求/错误不泄密。
- **输入与输出**：输入为账号配置、timeout、clock/transport；输出为有效 observation 或脱敏 unavailable 类别。
- **可依赖的已完成能力**：Unit 1 的 validated account/secret 契约；workspace 已有 `reqwest`、`tokio`、`serde_json`。
- **明确禁止依赖的未来能力**：不得汇总两个账号、决定 Start/Wait、sleep、写 checkpoint 或调用 runner/backend。
- **文件**：创建 `crates/ralph-cli/src/loop_runner/quota/mod.rs`、`crates/ralph-cli/src/loop_runner/quota/client.rs`；机械注册 `crates/ralph-cli/src/loop_runner/mod.rs`；模块内契约测试。
- **验收测试**：GET 固定 path、Bearer 正确、timeout；有效脱敏 fixture；video/其它条目不被误读为 general；401/429/5xx、业务 status、畸形 JSON、缺 general、非法百分比/时间、超出大小上限的响应体；redirect 不向其它 host 携带认证。
- **需要拆分的单元测试**：response parser、业务状态校验、百分比 0..100 校验、epoch-ms 转换、redacted error rendering。
- **Red 预期失败原因**：client/parser 尚不存在，或错误对象/请求 debug 暴露 Authorization。
- **最小实现范围**：单账号请求、解析、超时、redirect policy、脱敏 observation；production endpoint 固定，测试仅通过内部 transport/base-url seam 注入本地服务。
- **集成验证**：本地 mock server 捕获真实 HTTP method/path/header，并返回 fixture；测试 client 显式规避环境 proxy。
- **回归范围**：`ralph-cli` quota client 子集；现有 reqwest 使用方不改；确认没有线上请求。
- **TDD 闭环**：①先写单账号契约验收；②确认因无 client/解析失败；③拆 parser/security 单测；④逐个 Red→Green→Refactor；⑤跑 local HTTP contract；⑥跑 `ralph-cli` 相关回归；⑦固定脱敏 fixture 并关闭；⑧方可进入 Unit 3。
- **完成标准**：任意输入都得到一个确定、脱敏 observation，不 panic、不参与调度。
- **风险与注意事项**：若实测 schema 与 `model_remains/general` 不符立即阻塞并修订规格；官方已改称 Token Plan，不能按旧字段猜测新语义。

### Unit 3：双账号池化与 deadline 纯决策

- **Unit 目标**：把恰好两个 observation 纯函数式转换为 `Start` 或 `Wait(deadline, reason)`。
- **对应 Scenario**：S3、S4、S5、S6。
- **外部可观察结果**：12+15 启动、10+10 等待、10+11 启动；失败账号贡献 0；最早可信 future deadline/fallback 唯一确定。
- **输入与输出**：输入为两个 observation、预算、buffer、fallback 和当前时间；输出为池摘要与 Start/Wait decision。
- **可依赖的已完成能力**：Unit 2 observation；Unit 1 validated budget。
- **明确禁止依赖的未来能力**：不得发 HTTP、sleep、读写 state、选择 hat、启动 backend 或产生终止原因。
- **文件**：创建 `crates/ralph-cli/src/loop_runner/quota/decision.rs`；模块内表驱动/property tests。
- **验收测试**：严格 `>` 边界、单账号足够、双失败、周额度 0、不同 interval/weekly deadline、失败 fallback 更早、全部无可信时间、过去时间防忙循环。
- **需要拆分的单元测试**：账号贡献计算、deadline 候选、最早值、stale-deadline fallback、一次 pool decision 的摘要脱敏。
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
- **输入与输出**：输入为 selected hat、quota decision source、clock、控制信号和独立预算；输出为 Proceed、Stopped、Restart、WorkspaceGone 或 QuotaExhausted outcome 及 paused duration。
- **可依赖的已完成能力**：Unit 1–3；现有 `next_hat()`、`build_prompt()`、stop/restart/workspace 检查。
- **明确禁止依赖的未来能力**：不得处理跨进程 continue、backend quota signature、accepted event、isolated channel commit 或最终 UI 文案。
- **文件**：创建 `crates/ralph-cli/src/loop_runner/quota/wait.rs`、`crates/ralph-cli/src/loop_runner/quota/gate.rs`；修改 `crates/ralph-cli/src/loop_runner/runner.rs` 的 pre-prompt 区域和最小内部依赖注入 seam；修改 `crates/ralph-core/src/event_loop/loop_state.rs`；创建 `crates/ralph-cli/src/loop_runner/tests/quota.rs` 并在 tests `mod.rs` 注册。
- **验收测试**：disabled zero-call；20→wait→21 后同一 hat 启动；deadline/control 同 tick；stop/restart/workspace-gone；max wait/rechecks；虚拟等待 5 小时但 active elapsed 不增长；pending queue 在等待期间未被 take。
- **需要拆分的单元测试**：wait 状态机、signal precedence、recheck 计数、paused-duration accumulator、LoopState active elapsed。
- **Red 预期失败原因**：runner 当前会直接 build prompt/spawn，LoopState wall-clock 会把等待算入 max runtime。
- **最小实现范围**：在 quota facade 内串行调用 Unit 2 两次并交给 Unit 3，一次双账号查询只增加一次 recheck；实现 pre-prompt gate、虚拟时钟 seam 和 active pause accounting；production wrapper 的公开签名保持不变。
- **集成验证**：真实 EventLoop pending queue + fake quota facade + fake backend probe，证明 gate 的实际插入顺序。
- **回归范围**：loop_runner legacy/fake-path、max-runtime、next-hat fairness、handoff timeout、disabled default path。
- **TDD 闭环**：①先写 runner acceptance；②确认 backend 过早启动/elapsed 增长；③拆 wait/clock 单测；④Red→Green→Refactor；⑤跑真实 EventLoop 集成；⑥跑 `ralph-cli` 受影响回归；⑦确认无 sleep 型脆弱测试；⑧方可进入 Unit 5。
- **完成标准**：同进程等待闭环可独立工作，且没有消费或改变尚未放行的 activation。
- **风险与注意事项**：gate 不得放在 `build_prompt()` 之后；不要复用 hook suspend 的固定轮询实现，应以 deadline timer 与控制信号选择。

### Unit 5：durable checkpoint 与 `--continue` 恢复

- **Unit 目标**：把 waiting/pending activation 以版本化、原子、owner-only checkpoint 持久化，并在同 loop/workspace 的 continue 中恢复。
- **对应 Scenario**：S7、S8、S10、S11（state 基础）、S14（state）。
- **外部可观察结果**：进程退出再启动后仍恢复同一 hat、该 hat 待消费事件和本 activation 会消费的 human guidance 批次；过期 deadline 立即重查；损坏/错属 state fail-closed。
- **输入与输出**：输入为 selected activation snapshot、pool summary、deadline、计数、loop/workspace identity；输出为 ContinueWaiting、Recheck、ReplayPending、IgnoreCompleted 或拒绝恢复。
- **可依赖的已完成能力**：Unit 4 pre-prompt selection/gate；`SuspendStateStore` 的版本化与 temp+rename 模式；现有 `--continue` 入口。
- **明确禁止依赖的未来能力**：不得识别 backend quota、决定 accepted-event commit、修改普通 next-hat 算法或把完整 prompt/backend output 写盘。
- **文件**：创建 `crates/ralph-cli/src/loop_runner/quota/state.rs`；修改 `crates/ralph-cli/src/loop_runner/runner.rs` 的 startup/continue 区域、`crates/ralph-cli/src/commands/run.rs` 的既有 continue 编排（仅需时）；按最小需要为 `crates/ralph-core/src/event_loop/mod.rs`/`crates/ralph-proto/src/event_bus.rs` 增加 snapshot/restore 契约及测试。
- **验收测试**：checkpoint round-trip；hat+pending events+task identity 一致；stop 保留、restart 保留、completed 不重放；deadline 未到/已过；未知版本、损坏 JSON、loop/workspace/config/hat mismatch；Key 不在 JSON/错误。
- **需要拆分的单元测试**：schema version、原子替换失败清理、0600 权限（Unix）、resume decision、activation snapshot/restore 不重复 fan-out。
- **Red 预期失败原因**：当前没有 quota checkpoint，`--continue` 只能依赖现有 scratchpad/event replay，不能保证原 selection。
- **最小实现范围**：在 prompt 构建前先保留 activation snapshot 于内存，仅在 Wait/PendingReplay 状态转换时持久化；实现 checkpoint store、identity 校验、snapshot/restore、continue 分流；不接 post-execution quota 分支。
- **集成验证**：用两个新建 EventLoop 实例模拟进程重启，第二个实例经 continue 恢复同一 activation；不得手工编辑 runtime state，测试通过 store API 建 fixture。
- **回归范围**：现有 `integration_resume`、scratchpad/loop-id 校验、stop/restart、EventBus routing/fairness。
- **TDD 闭环**：①先写 restart/continue acceptance；②确认恢复选错/无 state；③拆 store/identity 单测；④Red→Green→Refactor；⑤跑双实例恢复集成；⑥跑 continue/EventBus 回归；⑦确认 state 无 secret 和陈旧重放；⑧方可进入 Unit 6。
- **完成标准**：checkpoint 契约冻结，恢复不需要猜 hat，损坏或越权 state 不会运行。
- **风险与注意事项**：只持久化重建 activation 所需事件批次和元数据，不持久化 prompt；checkpoint 以 owner-only 权限保存可能敏感的事件 payload，Commit 后清理，stop/restart 保留，拒绝恢复/损坏状态不得静默长期留存；Key 轮换后可用新配置 fresh recheck，因此 state 不存 Key/hash。

### Unit 6：CLI/PTY quota evidence 分类

- **Unit 目标**：从当前两条 executor surface 保留的完整 output/error evidence 中识别确认过的 MiniMax 套餐耗尽，且不改变普通错误分类。
- **对应 Scenario**：S11、S12、S13、S14（execution evidence）。
- **外部可观察结果**：同一 quota fixture 在 CLI/PTY 得到相同 typed classification；普通 429/watchdog/post-event timeout 与修改前差分一致。
- **输入与输出**：输入为 executor output、exit/termination、error chain；输出为 QuotaExhausted evidence 或 ExistingOutcome，原始普通结果仍可下游处理。
- **可依赖的已完成能力**：Unit 1 enabled flag、Unit 2 脱敏错误约束；现有 `ExecutionOutcome`、CLI/PTY result。
- **明确禁止依赖的未来能力**：不得查询 usage、等待、写 checkpoint、读取 accepted events 或决定 commit/replay。
- **文件**：修改 `crates/ralph-cli/src/loop_runner/execution.rs` 和 runner 的 execution result 收敛点；只在 characterization 证明证据缺失时最小修改 `crates/ralph-adapters/src/cli_executor.rs`、`crates/ralph-adapters/src/pty_executor.rs`；对应 adapter/legacy tests。
- **验收测试**：用户脱敏 quota fixture；CLI/PTY parity；普通短时 429、401、网络错误、模型错误、一般 non-zero、watchdog、post-event timeout；disabled 时 classifier bypass。
- **需要拆分的单元测试**：signature matcher、证据规范化、false-positive corpus、redacted classification Debug。
- **Red 预期失败原因**：当前 `ExecutionOutcome` 只有 success/termination/watchdog，没有 typed quota evidence，部分 `?` 路径可能提前丢失 error chain。
- **最小实现范围**：evidence 保留与纯 classifier；signature 必须窄匹配 fixture，不允许仅匹配 `429`/`quota` 单词。
- **集成验证**：真实 fake CLI process 和 PTY fixture 产生非零退出，证明 production executor output 到 classifier 的链路。
- **回归范围**：adapter executor suites、loop_runner watchdog/post-event timeout/ordinary failure tests。
- **TDD 闭环**：①先加 characterization 和 quota parity 验收；②确认 typed 分类缺失；③拆 matcher 单测；④Red→Green→Refactor；⑤跑 executor integration；⑥跑 adapter+CLI failure 回归；⑦确认无宽匹配；⑧方可进入 Unit 7。
- **完成标准**：classifier 只回答“是否是已确认套餐耗尽”，不承担恢复策略且零 false-positive fixture。
- **风险与注意事项**：实际 signature 尚需用户提供脱敏样本；没有样本不得用猜测实现 Green。

### Unit 7：accepted-event 提交点与 cold replay

- **Unit 目标**：在真实 runner post-execution 顺序中，以本 activation 的 runtime accepted events 决定提交或持久化重放。
- **对应 Scenario**：S11、S12、S13、S14（backend env/prompt）。
- **外部可观察结果**：quota 后已有 accepted event 只提交一次；无 accepted event 不计普通失败并在 fresh recheck 后重放同一 activation。
- **输入与输出**：输入为 Unit 6 classification、本 activation channel/reader 增量和 Unit 5 snapshot；输出为 Commit、PendingReplay 或 ExistingFailure 路由。
- **可依赖的已完成能力**：Unit 1–6 的冻结契约、`ProcessedEvents.accepted_events`、isolated channel merge、checkpoint restore。
- **明确禁止依赖的未来能力**：不得重写 client/decision/state 内部算法、扩大 quota signature、修改 preset/hat instructions。
- **文件**：修改 `crates/ralph-cli/src/loop_runner/runner.rs` 的 post-execution 区域、`crates/ralph-cli/src/loop_runner/hat_channel.rs`；扩展 `crates/ralph-cli/src/loop_runner/tests/quota.rs`，必要的 `crates/ralph-cli/src/loop_runner/tests/legacy.rs` characterization。
- **验收测试**：quota+activation-owned accepted business/terminal event→Commit；quota+仅 internal/diagnostic event 或 empty/rejected/malformed/partial channel→PendingReplay；在落 PendingReplay 前完成一次 activation-local channel/reader drain；至少 fallback cooldown+双账号 fresh query；同 hat/event/task/iteration/attempt；恢复提示要求检查 workspace；quota Key 不进入 backend env/prompt。
- **需要拆分的单元测试**：activation-local accepted snapshot 判定、channel commit/discard disposition、freshness requirement、replay prompt metadata（不锁精确文案）。
- **Red 预期失败原因**：runner 当前先 `process_output` 再读 JSONL，quota non-zero 会过早进入普通失败；build_prompt 已消费 trigger 且没有 replay 分支。
- **最小实现范围**：只对 typed quota outcome 调整顺序：先合并/验收本 activation 增量，再 Commit/PendingReplay；非 quota 保持原顺序和结果。
- **集成验证**：production runner + fake quota facade + fake backend + real EventLoop policy/channel，断言事件次数、failure/iteration 计数和 replay identity。
- **回归范围**：missing-event/hard-gate、execution contract rejection、text fallback、isolated one-event budget、normal completion、post-event timeout。
- **TDD 闭环**：①先写 S11/S12 runner acceptance；②确认现有顺序导致失败/重复；③拆 disposition 单测；④Red→Green→Refactor；⑤跑 real runner integration；⑥跑完整 loop_runner 相关回归；⑦确认非 quota differential 一致；⑧方可进入 Unit 8。
- **完成标准**：accepted event 成为唯一提交点，未提交 activation 可重复恢复而不污染普通恢复预算。
- **风险与注意事项**：accepted snapshot 必须来自 activation-start cursor 之后、完成本 activation 最终 drain 的 runtime 验收结果，不能使用全局历史或 grep output；channel 被 policy 拒绝、loop-internal 或 diagnostic event 都不算提交。

### Unit 8：专用终止原因与全可观察面脱敏

- **Unit 目标**：把 quota wait/recheck 上限、state 恢复错误和控制结果一致映射到 operator-facing termination/summary/sentinel/RPC，并验证全产物无 Key。
- **对应 Scenario**：S7、S8、S9、S10、S14。
- **外部可观察结果**：QuotaExhausted 可读且不冒充其它预算；RPC 沿用现有 `Error` 粗粒度，不新增 proto 变体；resume hint 与 stop/restart/workspace 状态一致。
- **输入与输出**：输入为 Unit 4/5/7 outcomes；输出为 typed `TerminationReason`、exit code、display/summary/sentinel/RPC event。
- **可依赖的已完成能力**：Unit 1–7；现有 termination hook/summary/display/subprocess sentinel 管线。
- **明确禁止依赖的未来能力**：不得修改 quota decision/replay、增加 CLI 命令或引入新的 RPC 协议枚举。
- **文件**：修改 `crates/ralph-core/src/event_loop/types.rs`、`crates/ralph-core/src/event_loop/termination_impl.rs`、`crates/ralph-core/src/summary_writer.rs`、`crates/ralph-cli/src/display.rs`、`crates/ralph-cli/src/commands/run.rs`、`crates/ralph-cli/src/loop_runner/runner.rs` 及对应 tests。
- **验收测试**：wait/recheck 两种 exhausted；Stopped/Restart/WorkspaceGone；resume hint；sentinel round-trip；parent exit code；summary/status；RPC generic Error；对所有 artifacts 执行 Key 哨兵断言。
- **需要拆分的单元测试**：termination status text、success/exit mapping、resume hint、serde round-trip、redacted diagnostic payload。
- **Red 预期失败原因**：现有 `TerminationReason` 无 quota variant，各 exhaustive match 不完整或会落入含糊 generic 文案。
- **最小实现范围**：新增一个 typed quota exhausted reason及其所有现有消费者；state corruption 仍以可操作 error 返回，不伪装 quota exhausted。
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
- **明确禁止依赖的未来能力**：不得在本 Unit 补生产逻辑、改变冻结契约、修改 preset/schema/zsh/operator skills 或新增 CLI。
- **文件**：修改 `docs/guide/configuration.md`、`docs/guide/cost-management.md`、`docs/guide/backends.md`、`docs/reference/troubleshooting.md`、`crates/ralph-core/data/ralph-tools.md`；不改 `ralph-tools-cmdref.md`，因为没有命令语法变化；完善 `crates/ralph-cli/src/loop_runner/tests/quota.rs` 的 Scenario 命名/追踪注释。
- **验收测试**：字段/default 与 serde contract 一致；文档明确 Subscription Key 仅查询、不注入 backend、私有配置需 owner-only 且不得提交版本库，并解释 Purchased Credits 不在决策内；stop/continue、restart、workspace-gone、上限与恢复提示；agent guide 只要求检查 workspace/task/scratchpad，不出现内部文件/函数名。
- **需要拆分的单元测试**：无新增业务单元；文档契约由 config fixture、CLI doc drift、Scenario trace 完整性和 secret scan 验证。
- **Red 预期失败原因**：现有文档没有 quota 配置/恢复语义，agent guide 不知道 cold-restart 后应检查副作用。
- **最小实现范围**：文档和验收索引；若验收暴露生产缺陷，返回责任 Unit 修复，不能在 Unit 9 就地改算法。
- **集成验证**：运行全部 S1–S14 acceptance、`scripts/check-cli-doc-drift.sh`、agent skill doc 反向检查。
- **回归范围**：现有 mock E2E、所有 docs/static checks、core/CLI/adapters 相关 suites。
- **TDD 闭环**：①先让文档/trace 检查暴露缺口；②确认因缺章节/契约漂移失败；③补最小文档断言；④Red→Green→Refactor 文档结构；⑤跑 acceptance integration；⑥跑全受影响回归；⑦无新增 skipped/ignored 后关闭；⑧进入最终质量门禁。
- **完成标准**：Coding Agent 不需要猜配置、测试或恢复行为；文档不包含实现内部细节且与代码一致。
- **风险与注意事项**：`crates/ralph-core/data/*.md` 必须从 agent 下一步动作写，不写计划编号、内部 state 路径、函数名或一次性事故背景。

## 6. 最终质量门禁

最终门禁不是补实现的 Unit。任何失败都必须回到责任 Unit，重新完成该 Unit 的 Red→Green→Refactor、集成与回归，再按顺序复验后续 Unit。

- [ ] S1–S14 全部通过，需求—测试追踪矩阵无空白，关键 runner acceptance 明确证明“不 spawn”“同一 activation 重放”“accepted event 不重放”。
- [ ] 所有新增及受影响单元测试通过；property/state-machine/fault-injection 测试通过且无随机 flake。
- [ ] 所有必要的集成/契约测试通过：本地 MiniMax HTTP contract、client→decision、pre-prompt gate、双实例 continue、CLI/PTY evidence、real EventLoop channel acceptance、termination artifacts。
- [ ] 关键 E2E 通过：S7/S11/S12 的 production runner acceptance 通过；现有 `cargo run -p ralph-e2e -- --mock` 通过。线上 MiniMax 调用不是验收条件。
- [ ] `cargo nextest run -p ralph-core -- <quota/config/termination subsets>`、`cargo nextest run -p ralph-adapters -- <executor subsets>`、`cargo nextest run -p ralph-cli --bin ralph -- <quota/runner subsets>` 均通过；不得裸跑 ralph-cli 的 `cargo test`。
- [ ] `cargo fmt --check`、`cargo clippy`、`cargo build` 和必要 doctest 通过；命令语法/文档检查 `scripts/check-cli-doc-drift.sh` 通过。
- [ ] 最终运行 `./scripts/run-tests.sh`；仅明确的时序 flake 才可按仓库规则用 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 复核，serial 仍失败即为真失败。
- [ ] 没有新增失败、ignored、skip、`.only`、削弱断言、无解释 snapshot/golden 更新或 mock 掉 production runner 行为。
- [ ] disabled/default 差分证明：HTTP 0 次、quota state I/O 0 次、prompt/事件/termination/backend env 与基线一致，且无可测启动延迟。
- [ ] 两个哨兵 Key 在日志、TUI/RPC、summary、diagnostics、sentinel、checkpoint、prompt、backend env、错误/Debug 中均零命中。
- [ ] `crates/ralph-core/data/ralph-tools.md` 与用户文档反向核对完成；确认不需要修改 `ralph-tools-cmdref.md`、preset/schema、zsh completion、operator preset skills。
- [ ] 未验证内容明确记录：MiniMax response schema 和 quota error signature 以用户提供的脱敏 fixture 为验收前置；若 fixture 未取得或与假设冲突，计划状态不得标为完成。
- [ ] 剩余风险明确接受：两账号等容量与 Proxy 聚合是部署前提；套餐/官方响应未来可能漂移；固定 20% 不能保证单次 activation 一定完成，运行中 cold replay 是兜底而非精确配额预测。
