---
title: "修复 Supervisor Runtime P0 控制面与收敛契约"
date: 2026-07-23
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
status: draft
origin:
  - docs/report/2026-07-23-ce-executor-supervisor-primary-20260723-082003-diagnosis.md
parallel_track: supervisor-preset-refactor
depends_on: []
---

# 修复 Supervisor Runtime P0 控制面与收敛契约

## Goal Capsule

修复 supervisor worker 在真实 worktree/PTY 执行中的事件路由、空结果误成功、wave 身份混用、slot/task 生命周期漂移和超时收敛缺陷。完成后，worker 的代码工作目录与控制面路径明确分离，每个 slot 只能产生一个有效终态，任何空结果、非法事件、超时或取消都会确定性失败并最终形成可审计的 `*.wave.failed`，不会产生 nested orphan ledger、伪成功或永久等待。

本计划与 `docs/plans/2026-07-23-005-refactor-supervisor-concurrent-pipeline-plan.md` 可以并发实施：两者文件写集互斥，依靠本文冻结的公开契约进行契约测试；只有合并后的发布级真实链路 E2E 是联合门禁，不构成两个开发分支的串行实现依赖。

## 1. 功能目标

### 业务目标

- 让 supervisor 对真实 Claude/PTY worker 的成功与失败判断可信。
- 让主 workspace 成为唯一控制面，slot worktree 只承载代码修改。
- 让 operator 能用同一个公开 `wave_id` 贯穿事件、诊断和恢复。
- 让 task、slot、wave 三层状态在成功、失败、超时、取消和重放下保持一致。

### 本次范围

- worker spawn 边界的主 workspace root、绝对 per-worker event channel、slot code root 透传与校验。
- public wave ID 与 internal store ID 的类型/持久化/恢复边界。
- `event_count=0`、无合法终态、非法终态、重复终态的 fail-close。
- slot 状态机和 task `open → started → done|failed` 的可恢复最终一致投影。
- FIFO 等待、per-worker timeout、aggregate timeout、取消和 fan-in 的确定性语义。
- 内存与 rusqlite store 的等价行为。
- runtime agent guide 中受影响的通用 wave、emit、task 使用说明。
- 真实 runtime 路径的 targeted integration、故障注入和回归测试。

### 非目标

- 不修改任何 preset 或 preset schema。
- 不修改 `presets/en/ce-executor-pipeline.yml`。
- 不新增 `WaveKind::Test` 或任何新 wave family。
- 不重构 `ce-executor-supervisor` hats、DAG 或报告拓扑。
- 不引入通用工作流平台、可视化 DAG 或新的外部存储。
- 不把历史 orphan 文件迁移回主账本；只保证新执行不再产生。

### 已知约束和假设

- 当前 dispatcher 已设置 `RALPH_EVENTS_FILE`，但需要验证它在真实 backend/tool 子进程中的最终值与继承链；不得把根因简化成“缺少一个 env”。
- `RALPH_EVENTS_FILE` 必须是主控制面下的规范化绝对路径；`RALPH_WORKSPACE_ROOT` 指向主 workspace；worker cwd/code root 指向 slot worktree。三者不可互相推导或混用。
- public wave ID 用于 payload、事件、日志、诊断和 Confirm；store ID 仅限 supervisor 内部持久化与状态迁移。
- worker 成功必须同时满足：进程成功、至少一个被接纳事件、恰好一个允许的 slot 终态、slot 未先超时/取消/失败。
- `event_count` 只统计通过 origin、policy、schema 和 slot identity 校验后被接纳的事件。
- timeout-with-events 保留 partial evidence，但 slot 的最终状态仍是 timeout，不得转为 completed。
- 测试只使用 `cargo nextest run` 系列；spawn human CLI 的测试必须 scrub 外层 hat env。

## Product Contract

### 冻结的公开输入

每个 worker request 必须获得：

- `public_wave_id`：对 agent/operator 可见的稳定关联 ID；
- `slot_index`、`wave_total`；
- `task_id`、`task_key`；
- 主 control-plane workspace root；
- 主控制面下规范化的绝对 per-worker event channel；
- slot code worktree 的绝对 cwd；
- 该 slot 允许的 ready/terminal topic 与必要 payload identity。

### 冻结的公开输出

- 每个 slot 恰好接受一个 `*.unit.done` 或 `*.unit.failed`。
- 成功结果保留可合并引用：`branch`、`worktree_path`、commit 或已有等价不可变引用。
- 任一 required slot 空结果、失败、超时或取消，wave 输出一个结构化 `*.wave.failed`。
- 全部 required slots 合法完成，wave 输出一个结构化 `*.wave.complete`。
- coordination event 和诊断只暴露 public wave ID，不暴露 store ID。
- 若 loop 已接受 `plan.blocked`，后续 reporter 的 `LOOP_COMPLETE` 只关闭报告链，不得把外部结果投影为成功：loop registry/inspect 显示失败，CLI 返回非零，最终诊断保留 blocked reason。没有先行 `plan.blocked` 的正常 `LOOP_COMPLETE` 仍是成功。
- first-terminal-wins 的“恰好一个终态”指恰好一个终态被接纳：第一个合法 done/failed 原子决定 slot；相同 event id 重放为 no-op；之后的异类终态被拒绝并诊断，但不反转已提交结果。

### 失败码最小集合

实现时沿用已有错误承载类型；若缺少结构化 reason，则扩展现有类型而不是创建平行错误系统。至少可区分：

- `invalid_control_plane_path`
- `orphan_event_route`
- `empty_worker_result`
- `missing_worker_terminal`
- `conflicting_worker_terminal`
- `worker_timeout`
- `aggregate_timeout`
- `worker_cancelled`
- `unknown_public_wave`
- `slot_out_of_range`

## Planning Contract

### 严格串行

```text
Unit 1 → Unit 2 → Unit 3 → Unit 4 → Unit 5 → Unit 6 → Unit 7
```

每个 Unit 完成验收 Red、单元 Red→Green→Refactor、集成验证和受影响回归后才可进入下一个 Unit。

### 文件所有权

本计划可修改：

- `crates/ralph-cli/src/loop_runner/execution.rs`
- `crates/ralph-cli/src/loop_runner/wave/`
- `crates/ralph-cli/src/commands/emit.rs`
- `crates/ralph-core/src/supervisor/`
- runtime task/event projection 的直接实现文件（实施前以 `rg` 确认准确路径）
- `crates/ralph-core/data/ralph-tools-wave.md`
- `crates/ralph-core/data/ralph-tools-emit.md`
- `crates/ralph-core/data/ralph-tools-tasks.md`
- 新建 `crates/ralph-cli/tests/integration_supervisor_runtime_p0.rs`

本计划禁止修改：

- `presets/en/ce-executor-supervisor.yml`
- `presets/schemas/ce-executor-supervisor.yml`
- `presets/en/ce-executor-pipeline.yml`
- preset lint、preset BDD 和 preset operator skills
- `crates/ralph-cli/tests/integration_supervisor_primary.rs`

## 2. BDD 行为规格

### Feature A1：worker 控制面与代码面隔离

```gherkin
Feature: Supervisor worker 使用主控制面且在 slot worktree 修改代码

  Scenario: worktree 深层 cwd 中的合法 emit 只进入主控制面
    Given 主 workspace、slot worktree 和主控制面 per-worker event channel 已建立
    And worker cwd 位于 slot worktree 的深层子目录
    When worker 发出带正确 wave、slot、task identity 的合法终态
    Then 事件只写入主控制面的 per-worker channel
    And slot worktree 下不会生成 .ralph/events.jsonl
    And 主 runner 能读取并验证该事件

  Scenario: 外层污染的 hat env 不覆盖显式 worker binding
    Given 测试进程带有另一 loop 的 RALPH_CURRENT_HAT、RALPH_EVENTS_FILE 和 RALPH_WORKSPACE_ROOT
    When supervisor spawn worker
    Then worker 获得当前 loop 的显式绝对控制面路径
    And 不读取外层残留路径

  Scenario: 非法或冲突控制面路径 fail-close
    Given event channel 是相对路径、指向 slot 子树、父路径不可安全创建、symlink 逃逸或与 workspace root 冲突
    When dispatcher 校验 worker request
    Then worker 不启动或其结果被拒绝
    And 返回 invalid_control_plane_path
    And 不创建 fallback orphan ledger
```

### Feature A2：公开 wave 身份唯一

```gherkin
Feature: 公开 wave ID 与 store ID 分离

  Scenario: 正常执行始终暴露同一个 public wave ID
    Given caller 触发一个包含多个 slot 的 wave
    When wave 注册、dispatch、fan-in、恢复并输出 coordination event
    Then所有公开 payload、日志和 Confirm 使用同一个 public wave ID
    And internal store ID 不出现在业务事件

  Scenario: 重启后恢复 public 到 store 的映射
    Given wave 已持久化且进程内缓存丢失
    When runner 恢复并再次处理同一 public wave ID
    Then 找回原 store wave
    And 不注册第二个 wave

  Scenario: 未知 wave 或越界 slot 被拒绝
    Given public wave ID 不存在或 slot_index 超出 wave_total
    When 结果到达
    Then 结果不改变任何 slot
    And 产生结构化诊断
```

### Feature A3：worker 结果真值表

```gherkin
Feature: 只有合法非空单终态结果可以完成 slot

  Scenario: 进程成功但零事件
    Given worker 以 exit 0 返回
    And event_count 等于 0
    When dispatcher 记录结果
    Then slot 为 failed 而不是 completed
    And reason 为 empty_worker_result

  Scenario: 有普通事件但没有允许的终态
    Given worker 产生至少一个被解析的非终态事件
    When worker 正常退出
    Then slot 为 failed
    And reason 为 missing_worker_terminal

  Scenario: 重复或冲突终态
    Given slot 已接受一个合法 done
    When 相同事件重放或随后到达 failed
    Then 重放是幂等 no-op
    And 冲突终态被拒绝
    And 已完成状态不被覆盖

  Scenario: timeout-with-events
    Given worker 在超时前产生部分合法非终态证据
    When per-worker timeout 到达且没有合法终态
    Then partial evidence 被保留
    And slot 最终为 failed
    And reason 为 worker_timeout
```

### Feature A4：slot、task 和 wave 一致收敛

```gherkin
Feature: Supervisor 将执行状态可恢复地投影到 task 和 wave

  Scenario: 正常任务生命周期
    Given task 为 open
    When slot 实际开始执行
    Then task 变为 started
    When slot 接受合法 done
    Then task 变为 done

  Scenario: slot 失败同步关闭 task
    Given task 为 started
    When slot 空结果、失败、超时或取消
    Then task 变为 failed
    And reason 与 slot failure 一致

  Scenario: 部分失败的 wave 确定性失败
    Given 同一 wave 中部分 slot 完成且一个 required slot 失败
    When fan-in 运行
    Then wave 只输出一次 wave.failed
    And 未开始或仍运行 slot 被有界取消和收割
    And 不输出 wave.complete
```

### Feature A5：FIFO 和两级超时

```gherkin
Feature: 调度等待、worker 执行和 aggregate wave 使用不同时间预算

  Scenario: FIFO 排队时间不消耗 worker timeout
    Given worker 因 max_concurrent_workers 在队列等待
    When slot 获得执行许可
    Then per-worker timeout 从实际启动时开始

  Scenario: aggregate timeout 终止整个 wave
    Given wave 从 durable registration commit 时点起超过 aggregate timeout
    When 仍有非终态 slot
    Then wave failed with aggregate_timeout
    And 所有剩余 slot 被取消并收割

  Scenario: 最后终态与 aggregate timeout 竞态
    Given 最后一个 slot 终态与 aggregate deadline 并发发生
    When 状态机原子决策
    Then deadline 时刻本身仍允许已到达的合法 slot 终态提交
    And 只有 now 严格大于 deadline 时 timeout 才可提交
    And 首个成功提交的 compare-and-set transition 胜出
    And wave 只有一个终态
    And 内存与 SQLite store 得到相同结果
```

## 3. 验收与测试策略

| Scenario | 验收条件 | 推荐测试层级 | 是否需要 E2E |
| --- | --- | --- | --- |
| A1.1 深层 cwd 合法 emit | 主 channel 有事件，nested orphan 不存在 | CLI 集成测试 | 是，1 条 |
| A1.2 污染 env | 显式 binding 胜出，旧路径无写入 | 集成测试 | 否 |
| A1.3 非法路径 | spawn/record fail-close，有稳定 reason | 单元 + 集成 | 否 |
| A2.1 单 public ID | 全链公开 ID 相同，store ID 不泄露 | 契约 + 集成 | 否 |
| A2.2 重启恢复映射 | 不重复注册，Confirm 可查询 | SQLite 集成 | 否 |
| A2.3 未知/越界 | 无状态突变，有诊断 | 单元测试 | 否 |
| A3.1 零事件 | dispatcher 与两种 store 均拒绝完成；真值表其余组合保留在低层测试 | 单元 + 契约 + 代表性 CLI 集成 | 是，1 条代表路径 |
| A3.2 无终态 | 非终态事件不构成成功 | 单元测试 | 否 |
| A3.3 重复/冲突终态 | first-terminal-wins，幂等重放 | 状态机 + 并发 | 否 |
| A3.4 timeout-with-events | partial evidence 保留、最终失败 | 故障注入 | 否 |
| A4.1/A4.2 task 投影 | task 与 slot 终态一致 | 集成测试 | 否 |
| A4.3 部分失败 | 单一 wave.failed、剩余 worker 收割 | 集成 + 故障注入 | 是，失败主路径 |
| A5.1 FIFO | queue wait 不计 worker timeout | 时间控制单元测试 | 否 |
| A5.2 aggregate timeout | 全 wave 有界失败 | 状态机测试 | 否 |
| A5.3 deadline race | 单终态、Memory/SQLite parity | 并发 + Differential | 否 |

## 4. 需求—测试追踪矩阵

| 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E |
| --- | --- | --- | --- | --- | --- |
| R-A1 控制面绝对路由 | A1.1–A1.3 | `integration_supervisor_runtime_p0` routing cases | path validation/env precedence | emit + PTY worker | deep-cwd emit |
| R-A2 单 public wave ID | A2.1–A2.3 | restart/unknown-wave cases | typed ID/map rules | rusqlite restart contract | 否 |
| R-A3 非空单终态 | A3.1–A3.4 | worker outcome table | validator/state machine | dispatcher/store parity | empty-result failure |
| R-A4 task/slot/wave 一致 | A4.1–A4.3 | lifecycle projection | transition rules | runner + task API | partial failure |
| R-A5 两级超时 | A5.1–A5.3 | virtual-time acceptance | deadline arbitration | Memory/SQLite differential | aggregate failure |
| R-A6 agent guide 同步 | A1/A3/A5 | doc drift check | 不适用 | CLI help/smoke | 否 |

## 5. 严格串行开发单元

### Unit 1：建立可复用的 worker/runtime 契约测试夹具

- **Unit 目标**：提供一个可独立通过的测试夹具，能启动受控 worker、捕获实际 env/channel、读取 Memory/SQLite snapshot 和 task view，并注入 process/event/clock 故障；缺陷 Characterization 分别放入后续负责修复该行为的 Unit。
- **对应 Scenario**：为 A1–A5 提供测试入口，本 Unit 只验收夹具观测能力。
- **外部可观察结果**：夹具自身的 self-test 通过，能证明它观测的是实际 runner 边界而非自造结果。
- **输入与输出**：输入为 fake/PTY worker、临时主 workspace、临时 slot worktree、rusqlite store；输出为测试证据，不修改运行时契约。
- **可依赖的已完成能力**：现有 supervisor bridge、fake backend、`common::ralph_bin()`、内存/SQLite store。
- **明确禁止依赖的未来能力**：不得预先使用 Unit 2–6 的新 validator、状态机或超时仲裁。
- **验收测试**：新建 `crates/ralph-cli/tests/integration_supervisor_runtime_p0.rs`；self-test 覆盖 env 捕获、event injection、临时 worktree、store snapshot、task view 和受控时钟。
- **需要拆分的单元测试**：fixture builder 参数校验、临时路径隔离、Memory/SQLite 双后端选择、污染 env scrub/显式注入。
- **Red 预期失败原因**：统一夹具尚不存在，当前测试无法在同一入口观测真实 runner、store 与 task。
- **最小实现范围**：仅测试夹具和 self-test；不得修改生产行为，也不得提前加入跨缺陷的失败断言。
- **集成验证**：`cargo nextest run -p ralph-cli --test integration_supervisor_runtime_p0 fixture_self_test`。
- **回归范围**：已有 supervisor 单元测试应继续通过。
- **完成标准**：fixture self-test Green，后续 Unit 能复用且互不共享临时状态；无 prompt/YAML 文本锁定测试。
- **风险与注意事项**：避免把 nested orphan 的历史诊断路径写死；临时目录必须从测试返回值解析。

### Unit 2：分离并持久化 public wave ID 与 store ID

- **Unit 目标**：公开身份在 register、dispatch、fan-in、恢复、Confirm 全链稳定，内部 ID 永不泄漏。
- **对应 Scenario**：A2.1–A2.3。
- **外部可观察结果**：同一 public ID 可在重启后恢复；重复注册幂等；未知 ID/越界 slot fail-close。
- **输入与输出**：输入 public wave key；输出可恢复的 store binding 与只含 public ID 的 coordination payload。
- **可依赖的已完成能力**：Unit 1 的 runtime 契约夹具；本 Unit 先新增 public/store identity Characterization Red。
- **明确禁止依赖的未来能力**：不依赖路径修复、零事件门禁或 task 投影。
- **验收测试**：先启用 public/store restart contract；确认 DuplicateKey fallback 不返回未经验证的 caller key。
- **需要拆分的单元测试**：映射持久化、同 public ID 幂等、并发不同 wave 分配、unknown/stale ID、slot bounds、coordination serialization 不泄漏。
- **Red 预期失败原因**：当前 bridge 的进程内 map 丢失后无法可靠恢复，方法参数和 fallback 混用 ID 语义。
- **最小实现范围**：收紧 bridge/store API 语义和持久化映射；不顺便新增 wave kind。
- **集成验证**：`cargo nextest run -p ralph-core -- supervisor`；`cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`。
- **回归范围**：register idempotency、100-way concurrent ID allocation、recover、fan-in merge。
- **完成标准**：public ID 是唯一公开关联键；store ID 仅内部使用；Memory/SQLite 语义一致。
- **风险与注意事项**：若需要 migration，必须前向升级并验证旧 DB 可打开；backward compatibility 非目标，但不能静默错绑。

### Unit 3：固定 worker 控制面绝对路径与 workspace/code-root 边界

- **Unit 目标**：无论 worker cwd、子进程层级或外层 env 如何，事件只回到主控制面。
- **对应 Scenario**：A1.1–A1.3。
- **外部可观察结果**：深层 worktree cwd 中运行 `ralph emit` 后主 channel 可见，slot 子树无 `.ralph/events.jsonl`。
- **输入与输出**：输入主 workspace root、主 events parent、slot worktree；输出经验证的绝对 channel、workspace root 和 code cwd。
- **可依赖的已完成能力**：Unit 2 的 public identity。
- **明确禁止依赖的未来能力**：不得用 Unit 4 的终态 validator 掩盖路由失败。
- **验收测试**：先运行 deep-cwd、外层污染 env、relative/symlink-escape/uncreatable-parent/subtree-conflict cases 并确认 Red；父目录不存在但可创建是正常路径。
- **需要拆分的单元测试**：路径规范化、主控制面包含关系、binding merge precedence、backend/tool subprocess env 继承、orphan detection。
- **Red 预期失败原因**：当前 per-worker channel 未强制绝对化，`inject_hat_execution_env` 未显式提供主 workspace root，真实子进程可能重新按 cwd 解析。
- **最小实现范围**：在 request 构造与 spawn 边界注入/校验；`emit.rs` 只做必要的 fail-close，不创建另一套路由。
- **集成验证**：带污染 env 运行 `cargo nextest run -p ralph-cli --test integration_supervisor_runtime_p0 routing`。
- **回归范围**：human CLI emit、isolated channel、worktree binding、review shared-readonly binding。
- **完成标准**：合法路径全绿；非法组合不 spawn 或明确失败；测试扫描确认无 nested orphan ledger。
- **风险与注意事项**：不要把 code worktree root 当作 workspace root；Windows/Unix 路径差异按现有路径工具处理。

### Unit 4：禁止空结果和无合法终态完成 slot

- **Unit 目标**：实现完整 worker outcome 真值表，并在 dispatcher 与 store 两层 fail-close。
- **对应 Scenario**：A3.1–A3.4。
- **外部可观察结果**：exit 0 + 0 events、只有普通事件、非法事件或没有合法终态都失败；timeout partial evidence 不丢失。
- **输入与输出**：输入 process outcome、被接纳 events、terminal classification；输出 validated slot outcome。
- **可依赖的已完成能力**：Unit 3 确保读取的是正确 channel。
- **明确禁止依赖的未来能力**：不依赖 task 投影或 aggregate fan-in 修复。
- **验收测试**：表驱动覆盖 success/failure/timeout/cancel × event_count 0/>0 × terminal none/done/failed/conflict。
- **需要拆分的单元测试**：accepted event counting、allowed topic/schema/identity、store zero-event rejection、partial timeout evidence。
- **Red 预期失败原因**：dispatcher 对 `Ok(..., true)` 无条件 record；两种 store 无条件设 completed。
- **最小实现范围**：一个共享 validator 或现有 outcome 类型扩展；store 保留第二道不变量检查。
- **集成验证**：runtime P0 integration 的 empty/missing/conflict/timeout cases。
- **回归范围**：现有成功 worker、partial-timeout、policy rejection、worker read retry。
- **完成标准**：只有恰好一个合法终态可完成；Memory/SQLite 对所有真值表行一致。
- **风险与注意事项**：不得把所有 timeout-with-events 归零；不得把 rejected event 计入 event_count。

### Unit 5：实现 slot 单终态状态机与 task 生命周期可恢复投影

- **Unit 目标**：防止晚到、重复或冲突结果覆盖终态，并以 supervisor transition 为事实源把 task 最终一致地闭合。
- **对应 Scenario**：A3.3、A4.1、A4.2。
- **外部可观察结果**：dispatch 后 task started；合法 slot terminal 后 task done/failed；重复不二次计数，冲突被诊断。
- **输入与输出**：输入 slot transition；输出已提交的 supervisor transition、可重放的 pending task projection、task 更新确认和可审计 conflict。
- **可依赖的已完成能力**：Unit 4 validated outcome。
- **明确禁止依赖的未来能力**：不依赖 Unit 6 的 wave timeout 仲裁。
- **验收测试**：state-machine sequence、并发 done/failed、cancel 后晚到 success、重启 replay、task projection failure injection。
- **需要拆分的单元测试**：合法 transition table、first-terminal-wins、same-event idempotency、task identity match、pending projection 写入/确认、task 写失败、两次持久化之间 crash、重启重放。
- **Red 预期失败原因**：store 当前可直接覆盖 terminal，task 与 slot 是分离写入或没有投影。
- **最小实现范围**：收紧现有 transition API；supervisor store 是 slot 事实源。slot transition 与 pending projection 在同一个 Memory/SQLite store mutation 中提交；随后 projector 幂等写 `tasks.jsonl`，成功后确认 projection。第二步失败或进程在两次写之间崩溃时，恢复流程重放 pending projection；重复写同一 task terminal 是 no-op。runtime 是 task 投影唯一写者，worker 不双写。
- **集成验证**：Memory/SQLite differential + task API view。
- **回归范围**：cancel、recover、merged_to_events idempotency、duplicate delivery。
- **完成标准**：所有 transition 有确定结果；task/slot 不出现 done/failed 分叉；重复 fan-in 不重复 merge。
- **风险与注意事项**：SQLite transition 与 pending projection 必须共享事务；JSONL 不参加 SQLite 事务，通过 durable pending projection 达成可恢复的最终一致性。Memory/SQLite 都要实现相同恢复协议。

### Unit 6：收敛 FIFO、per-worker timeout、aggregate timeout 与取消

- **Unit 目标**：建立两级 deadline 与 fan-in 的单终态仲裁。
- **对应 Scenario**：A4.3、A5.1–A5.3。
- **外部可观察结果**：队列等待不误伤 worker；任一 required failure 使 wave 有界失败；deadline race 不双终态。
- **输入与输出**：输入 enqueue/start/deadline/slot terminal/cancel；输出唯一 wave terminal 和剩余 worker 收割结果。
- **可依赖的已完成能力**：Unit 5 单 slot 状态机。
- **明确禁止依赖的未来能力**：不依赖 Unit 7 E2E 或 preset failure consumer。
- **验收测试**：virtual time FIFO、partial failure、aggregate timeout、operator cancel、last-terminal race、queued/running reap。
- **需要拆分的单元测试**：aggregate epoch=durable registration commit、per-worker epoch=running transition commit、`now==deadline` terminal 优先、`now>deadline` timeout、compare-and-set 仲裁、release capacity、recovery 使用持久化 epoch。
- **Red 预期失败原因**：当前 worker timeout 与 aggregate timeout 的触发点/日志语义混杂，竞态可能产生错误失败或永久等待。
- **最小实现范围**：调整现有 dispatcher/coordinator/phase/recover，不引入复杂重试框架。
- **集成验证**：故障注入 runner + Memory/SQLite differential。
- **回归范围**：backpressure cap、FIFO、公平释放、recover timeout、fan-in idempotency。
- **完成标准**：所有 wave 最终恰好 complete 或 failed；后台无遗留 worker；两类 epoch 与 equality 优先级固定；Memory/SQLite 对同一 transition trace 一致。
- **风险与注意事项**：避免真实 sleep；使用受控时钟或现有短有界 timeout fixture。

### Unit 7：真实 runtime 纵向验收、guide 同步与全量回归

- **Unit 目标**：证明 P0 在真实 runner 路径闭环，并同步 agent 可执行说明。
- **对应 Scenario**：全部 A scenarios。
- **外部可观察结果**：成功波主账本唯一、失败波确定终止、无 orphan、task 闭合、公开 ID 一致；先出现 `plan.blocked` 的 loop 即使由 reporter 发出 `LOOP_COMPLETE` 收尾，inspect/registry/CLI 仍显示失败。
- **输入与输出**：输入真实 CLI/fake backend replay；输出主 events、task view、SQLite snapshot 和进程退出结果。
- **可依赖的已完成能力**：Unit 1–6。
- **明确禁止依赖的未来能力**：不依赖 supervisor preset 重构计划；使用最小 synthetic registry/fixture。
- **验收测试**：成功、empty result、partial failure、timeout、cancel、restart、外层污染 env、`plan.blocked → reporter → LOOP_COMPLETE` 的失败状态与非零退出。
- **需要拆分的单元测试**：本 Unit 不新增业务逻辑；仅补发现的测试缺口。
- **Red 预期失败原因**：若纵向测试失败，必须定位到此前 Unit 漏掉的真实接线，不得在 E2E 中 mock 掉。
- **最小实现范围**：修正真实接线与文档；不修改 preset。
- **集成验证**：运行 targeted nextest、污染 env 回归、CLI help/guide 中列出的受影响命令 smoke。
- **回归范围**：`cargo nextest run -p ralph-cli --test integration_supervisor_runtime_p0`、`cargo nextest run -p ralph-core -- supervisor`、`cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`、`scripts/check-cli-doc-drift.sh`、`./scripts/run-tests.sh`。
- **完成标准**：所有 Scenario 通过；无 nested orphan；文档只描述通用 agent 动作，不泄漏内部 ledger/函数/事故路径。
- **风险与注意事项**：全量前先 targeted；若并发 flake，按仓库规则使用 `RALPH_BASELINE_SERIAL=1` 仅作兜底。

## Verification Contract

### 风险驱动测试

- Characterization：Unit 1 固定旧行为。
- State-machine：slot/task/wave transition。
- Idempotency/Concurrency：重复终态、并发终态、wave 注册、deadline race。
- Fault Injection：worker exit、timeout、cancel、DB restart、task projection failure。
- Differential：Memory 与 SQLite store 对同一 transition trace 返回一致 snapshot。
- Property-Based：若现有测试基础支持，对 transition 序列生成验证“最多一个终态、终态不可逆”；否则使用穷举表驱动，不新增依赖。

### 测试纪律

- 每个 Unit 先证明 Red 的失败原因正确，再修改生产代码。
- 禁止删除/削弱断言、skip、`.only`、无解释更新 snapshot/golden。
- BDD/集成必须经过真实 runner/emit/store 路径，不能只搜索源码文本。
- 所有 spawn human CLI 的测试使用 `common::ralph_bin()` 或显式 scrub helper。

## 6. 最终质量门禁

- 所有计划内 Scenario 通过。
- 所有新增及受影响单元测试通过。
- Memory/SQLite 契约测试通过。
- 真实 deep-cwd emit、empty-result failure、partial-failure、restart E2E 通过。
- `cargo fmt --check`、`cargo clippy`、`cargo build` 通过。
- `scripts/check-cli-doc-drift.sh` 通过。
- `./scripts/run-tests.sh` 通过；无新增失败、skip 或 ignored。
- 主控制面外不生成 nested `.ralph/events.jsonl`。
- event_count=0 永不进入 completed。
- 每 slot、task、wave 最多一个终态。
- store ID 不出现在公开业务事件。
- 未验证内容与剩余风险记录在计划实施结果中。

## Definition of Done

- P0-A：真实 worker 控制面路径可靠，orphan event 回归关闭。
- P0-B：空/非法/无终态结果无法伪装成功。
- task/slot/wave 生命周期和超时/取消收敛一致。
- targeted contract tests 可在本计划分支独立通过。
- 与 preset 重构分支合并后，再运行一次共同 supervisor 成功/失败全链 E2E；该联合门禁不要求本计划修改 preset 文件。
