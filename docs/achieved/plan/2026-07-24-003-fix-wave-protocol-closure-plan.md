---
title: "闭合 Wave 幂等、Ticket 事务与公开 Confirm 协议"
date: 2026-07-24
type: fix
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan-bootstrap
execution: code
deepened: 2026-07-24
origin: conversation-2026-07-24-wave-capability-audit
related_plans:
  - docs/achieved/plan/2026-07-22-001-feat-wave-protocol-suite-default-plan.md
  - docs/plans/2026-07-24-001-fix-supervisor-merge-closure-plan.md
---

# 闭合 Wave 幂等、Ticket 事务与公开 Confirm 协议

> **Executor 入口**：严格按 `Unit 1 → … → Unit 8` 串行。每 Unit：验收测 Red（记失败原因）→ 最小单测 Red→Green→Refactor → 集成 → 定向回归 → 关闭，再开下一 Unit。禁止并行改多 Unit。测试只用 `cargo nextest run` / `./scripts/run-tests.sh` / `cargo test --doc`。spawn `ralph` 的 human 测用 `crates/ralph-cli/tests/common/mod.rs` 的 `ralph_bin()` / `scrub_agent_runtime_env`。

---

## 1. 功能目标

### 业务目标

- 同一 `(loop_id, hat, topic, idempotency_key)` 的并发/重试 emit，最多形成一个公开 wave 批次；权威在 SupervisorStore，不在 sidecar。
- Agent verify ticket 只在 Apply 成功且可 Confirm 后消费；Apply 前失败可原输入重试。
- 提供 `ralph wave inspect <wave_id>` 公开 Confirm；active / terminal / unknown / unavailable 可区分。
- CLI help、JSON、注入 skill 对副作用与恢复说法一致，且不引导读内部 ledger。

### 本次范围

- Characterization 冻结当前缺口；公开 inspect；Store 单 wave read + availability。
- Emission reservation 状态机（InMemory + Rusqlite + v3 migration）。
- CLI idempotency 切 Store；合法 sidecar 一次性导入；正常路径不再读写 sidecar。
- Ticket：`prepared → claimed → consumed`；cleanup 失败报 applied。
- skill/help/drift；真 runtime BDD + 关键并发 E2E + 全量回归。

### 非目标

- 不重写 dispatcher/fan-in/worktree/compensation/builtin preset 拓扑。
- 不把 wave 变默认 CE 模式；不新增分布式事务；不允 agent 直查 SQLite/sidecar/events JSONL。
- 不强制无 key 的 agent emit；不改非 wave 的 `ralph emit` OPAC。

### 已知约束和假设（源码核实 2026-07-24）

| 事实 | 位置 |
|---|---|
| `WaveCommands` 仅 `Emit`/`Verify`，无 `Inspect` | `crates/ralph-cli/src/wave.rs` |
| 幂等实际走 sidecar：`write_wave_events_with_idempotency_with_scope` | 同文件 |
| 成功 JSON 含 `ok/wave_id/topic/count/events_file/deduplicated` | emit JSON 分支 |
| `read_and_consume_ticket` 先删文件再校验；emit 在写 JSONL 前 `require_ticket` | `wave_verify_gate.rs` + `wave.rs` |
| Store `register_wave`/`wave_id_for_idempotency_key` = dispatcher public id→store id，≠ CLI scope+digest | `supervisor/mod.rs` |
| migrations `CURRENT_VERSION = 2` | `supervisor/migrations.rs` |
| `summarize`/`build_supervisor_summary` Store 错误→`default()` | `supervisor/mod.rs`、`commands/inspect.rs` |
| 已有 `integration_wave_protocol_suite_u9.rs`（4 测）；无 `scenarios/wave_protocol/` | tests |
| skill 仍写「零写盘」且 Confirm 有 jq 读 ledger | `ralph-tools-wave.md` |

跨介质：JSONL↔SQLite 无原生事务 → reservation + operation identity + 零/完整/部分恢复。公开 `wave_id` 跨层身份；内部 `w-{seq}` 不进 agent API。CLI emit 不知 `WaveKind` → emission 与 runtime waves **分表**，public id 关联。

---

## 2. BDD 行为规格

```gherkin
Feature: Wave 幂等、Apply 与公开 Confirm 协议闭环
  作为 wave dispatcher hat 或 operator
  我希望同一 wave 操作在并发、失败和重启后仍有唯一且可查询的结果
  从而无需读取 runtime 内部 ledger，也不会重复派发 worker

  Background:
    Given 一个启用默认 supervisor-db 的临时 workspace
    And 一个声明 wave dispatcher 与 worker 权限的真实配置
    And 所有 human CLI fixture 已 scrub 外层 Ralph agent 环境

  Scenario: S1 正常 Apply 后公开 Confirm
    Given dispatcher hat 对 N 个合法 payload 执行 wave verify
    When 使用完全相同 payload 与稳定 idempotency key 执行 wave emit
    Then emit JSON 含 ok=true、唯一 public wave_id、deduplicated=false，且不含 events_file
    And ralph wave inspect <wave_id> 返回 registered=true 且 phase 为 dispatch|collect|integrate 之一
    And agent 未读取任何 .ralph/events*.jsonl / supervisor.db / sidecar

  Scenario: S2 同 key 同 payload 并发只 Apply 一次
    Given 两个 agent CLI 进程同 loop/hat/topic
    When 两者 barrier 同步后同时 emit 同一 payload 批次与 idempotency key
    Then 两者返回同一 public wave_id
    And 恰好一个响应 deduplicated=false，另一个 deduplicated=true
    And 主事件文件中该 wave_id 事件恰好 N 条

  Scenario: S3 不同 key 真实并发
    Given 两个合法批次使用不同 idempotency key
    When 两者并发 emit
    Then 得到两个不同 public wave_id 且均成功
    And 两进程执行窗口重叠（证明非全局串行，非 wall-clock SLA）

  Scenario: S4 同 key 不同 payload 冲突
    Given 某 scope 已用 payload P 成功 Apply
    When 同 key 提交 payload P2
    Then CLI 以稳定 idempotency conflict 失败
    And 事件数与 Store emission 不变

  Scenario: S5 Apply 前失败可重试
    Given agent 已 verify 且 ticket 匹配
    And 在 JSONL 写入前注入可恢复 I/O 失败
    When 修复后用相同输入重试 emit（不重新 verify）
    Then 重试成功且 ticket 只消费一次

  Scenario: S6 ticket 身份不匹配不消费
    Given 存在 prepared ticket
    When emit 使用错误 fingerprint/topic/loop/hat
    Then Apply 被拒且 ticket 仍为 prepared
    And 用原始匹配输入仍可成功 Apply

  Scenario: S7 Apply 后 cleanup 失败不诱导重发
    Given 事件批次与 Store 已 applied
    And ticket cleanup 故障注入失败
    When emit 返回
    Then 响应明确 applied（或等价稳定字段），引导 wave inspect
    And 不提示直接重发；重试不新增事件

  Scenario: S8 崩溃后完整批次恢复
    Given reservation 存在且事件批次完整，但 mark applied 前进程退出
    When 相同操作重试
    Then 返回原 wave_id 且 deduplicated=true
    And 不新增事件

  Scenario: S9 部分批次 fail-closed
    Given reservation expected_count=N 但 ledger 仅有 1..N-1 条对应事件
    When 相同操作重试
    Then 返回 partial emission 错误且不补写
    And wave inspect 暴露 recovery_required 或 failed（公开字段）

  Scenario: S10 合法旧 sidecar 导入
    Given 仅有旧 sidecar + 完整旧事件批次，Store 无 emission
    When 同 key 首次经新版本 emit
    Then 返回旧 public wave_id 且 deduplicated=true
    And 删除 sidecar 后再次 emit 仍 dedup

  Scenario: S11 非法旧 sidecar 不猜测导入
    Given sidecar 与事件的 digest/count/wave_id 不一致
    When 尝试导入
    Then migration conflict fail-closed，不创建第二 wave

  Scenario: S12 Inspect active 与 terminal
    Given Store 中有 active wave 与 done/failed wave（终态 row 仍在）
    When 分别 wave inspect
    Then active 返回当前 phase 与 pending/in_flight/completed/failed 计数
    And terminal 返回终态 phase 与最终计数

  Scenario: S13 Inspect unknown 与 unavailable
    Given 不存在的 public wave_id
    When wave inspect
    Then registered=false 且 status/语义为 unknown，与成功可区分
    When Store 文件存在但无法打开
    Then availability=unavailable，且不伪装成空成功

  Scenario: S14 Verify 契约诚实
    When 查看 wave verify --help 与加载 ralph-tools-wave
    Then 说明 verify 不写业务事件但会准备一次性 ticket
    And 不出现 zero-disk、读 events JSONL Confirm、读 supervisor.db 指导

  Scenario: S15 Human 与无 wave 非干扰
    Given human CLI（已 scrub）
    When 不带 idempotency key 连续 emit 两次
    Then 两次成功且 wave_id 不同
    Given 无 DetectedWave 的 pipeline
    When 跑完
    Then 不创建 supervisor DB / bridge
```

---

## 3. 验收与测试策略

| Scenario | 验收条件 | 推荐测试层级 | 是否需要 E2E |
|---|---|---|---|
| S1 | emit 公开字段；inspect registered；无 ledger 读 | CLI 集成 + BDD | 是 |
| S2 | 同 id；N 条事件；一真一假 dedup | 多进程集成 | 是 |
| S3 | 两 id 成功；重叠执行 | 并发集成 | 否 |
| S4 | 稳定 conflict；零新增写 | Store 单元 + CLI | 否 |
| S5 | 不 re-verify；ticket 一次消费 | Fault injection | 否 |
| S6 | 拒；ticket prepared | 状态机 + CLI | 否 |
| S7 | applied；inspect Confirm；不重发 | Fault injection CLI | 是 |
| S8 | 原 id + dedup | 状态机 + reopen | 否 |
| S9 | fail-closed；不补写 | 状态机 + 集成 | 否 |
| S10 | 旧 id；删 sidecar 仍 dedup | Migration differential | 否 |
| S11 | migration conflict | Migration 集成 | 否 |
| S12 | phase/计数 | Store contract + CLI | 否 |
| S13 | unknown≠unavailable | CLI + fault injection | 否 |
| S14 | help/skill/drift | CLI contract + drift | 否 |
| S15 | human bypass；零 DB | 既有回归 + BDD | 是（复用） |

风险驱动：Characterization（U1）、State-machine（emission/ticket）、Concurrency（双进程+UNIQUE）、Fault Injection（写前/cleanup/Store open）、Differential（InMemory↔Rusqlite、sidecar 导入）。不新增 property/fuzz 依赖。

---

## 4. 需求—测试追踪矩阵

| 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约 | E2E |
|---|---|---|---|---|---|
| 基线冻结 | 现状 | `integration_wave_protocol_baseline.rs` | 既有 idempotency | u9 | 否 |
| 公开 Confirm | S1,S12,S13 | `integration_wave_inspect.rs` | view/DTO | Store read | S1 |
| Store emission | S2–S4,S8,S9 | `integration_supervisor_emission_idempotency.rs` | 状态机表 | SQLite barrier | S2 |
| sidecar+CLI cutover | S1,S2,S4,S8–S11,S15 | `integration_wave_idempotency_store.rs` | importer 表 | 多进程 CLI | S2 |
| ticket 事务 | S1,S5–S7,S15 | `integration_wave_ticket_recovery.rs` | claim/restore | fault injection | S7 |
| 文档契约 | S14 | help + drift | 禁止词扫描 | `check-cli-doc-drift.sh` | 否 |
| 非干扰 | S15 | 既有 pipeline 测 | bridge 既有 | scenarios | 既有 |
| 真 runtime | S1,S8,S9 | `scenarios/wave_protocol/*.yml` | — | `run_workflow_guard_scenario` | `integration_wave_protocol_closure.rs` |

---

## 5. 严格串行开发单元

### Unit 1 — Characterization：冻结当前协议缺口基线

- **Unit 目标**：改行为前钉住「今天真实发生的事」，防后续假绿。
- **对应 Scenario**：为 S1–S15 提供对照（本 Unit **不修复**）。
- **外部可观察结果**：基线测对当前主线为绿；明确记录将被后续 Unit 翻转的断言。
- **输入与输出**：临时 workspace + `ralph_bin`；观测 JSON / sidecar 是否存在 / ticket 是否在写盘前消失 / 坏 DB 时 inspect loop 形态。
- **可依赖**：现有 emit/verify/gate、u9、`inspect loop`。
- **禁止依赖**：新 inspect、emission API、ticket claim、文档改写。
- **验收测试**（新建 `crates/ralph-cli/tests/integration_wave_protocol_baseline.rs`）：
  1. `baseline_emit_json_includes_events_file` — 成功 JSON **含** `events_file`。
  2. `baseline_idempotency_writes_sidecar` — 带 key emit 后 sidecar 存在。
  3. `baseline_ticket_removed_before_event_write_on_io_failure` — 事件不可写时 ticket 已不存在。
  4. `baseline_inspect_loop_swallows_corrupt_store` — 坏 DB 时 loop inspect 成功且无 availability=unavailable。
- **需要拆分的单元测试**：无新业务逻辑。
- **Red 预期失败原因**：本 Unit 特殊——应对**当前代码绿**；若红先修 fixture/scrub/feature。
- **最小实现范围**：**只加测试**，零产品改动。
- **集成验证**：`cargo nextest run -p ralph-cli --test integration_wave_protocol_baseline`；`… --test integration_wave_protocol_suite_u9`。
- **回归范围**：u9 四测。
- **完成标准**：4 基线绿；写下「将被 U5/U6/U3 翻转」清单；无产品 diff。
- **风险**：后续翻转禁止静默删断言；须在归属 Unit 记录。

### Unit 2 — Outside-In：公开 `ralph wave inspect` 合同

- **Unit 目标**：新增 `ralph wave inspect <wave_id> [--output json|text]`；用可测 view/Fake 固定 agent-safe 合同，不依赖生产 emission。
- **对应 Scenario**：S12、S13（形状）；S1 Confirm 命令表面。
- **外部可观察结果**：`--help` 含 inspect。JSON 最小字段：

```json
{
  "ok": true,
  "wave_id": "<public>",
  "registered": true,
  "availability": "available",
  "phase": "dispatch|collect|integrate|done|failed",
  "expected_total": 0,
  "completed_count": 0,
  "failed_count": 0,
  "pending_count": 0,
  "in_flight_count": 0,
  "cancel_requested": false
}
```

unknown：`registered=false`；unavailable：`availability=unavailable`。负例：无 `db_path`/`events_file`/内部 `store_id`/`pid`/payload/ticket 路径。
- **输入与输出**：wave_id + output format → agent-safe view。
- **可依赖**：U1；`WavePhase`/`WaveSnapshot`；`commands/inspect.rs` 同源 view 模式。
- **禁止依赖**：U3 生产 reader、emission、ticket。
- **验收测试**（新建 `crates/ralph-cli/tests/integration_wave_inspect.rs`）：`inspect_help_lists_subcommand`；`inspect_unknown_wave_id`；view 单测覆盖 active/terminal/unavailable/敏感字段负例；污染 hat env 下 scrub 仍可解析。
- **需要拆分的单元测试**：clap 缺 id/非法 output；四态序列化；计数边界。
- **Red 预期失败原因**：无 `Inspect` variant → unknown subcommand。
- **最小实现范围**：`wave.rs` 增加 Inspect + execute；可测 `render_wave_inspect_view`（名 TDD）；生产 DB 接线可留给 U3，但 **DTO 与负例本 Unit 必须完成**。help + unknown 集成必须绿。
- **集成验证**：`cargo nextest run -p ralph-cli --test integration_wave_inspect`；`cargo nextest run -p ralph-cli --bin ralph -- wave`。
- **回归范围**：emit/verify JSON（U1 基线仍绿）。
- **完成标准**：help 有 inspect；DTO/负例绿；无 skip。
- **风险**：phase 字符串与现有 `WavePhase` Display 一致；不发明未存在的 `cancelled` phase（用 `failed`+`cancel_requested`）。

### Unit 3 — Store 单 wave read model + availability

- **Unit 目标**：按 **public wave id** 查含终态 snapshot；missing/open 失败/unknown 可区分；`wave inspect` 与 `inspect loop` 共用 availability。
- **对应 Scenario**：S12、S13；S1 生产 Confirm。
- **外部可观察结果**：真实 DB 可查 active/done；坏 DB→unavailable；`inspect loop` 不再空 default 冒充（翻转 U1 `baseline_inspect_loop_swallows_corrupt_store`）。
- **输入与输出**：public id → Found(snapshot) / Unknown / Unavailable{reason_code}（类型名 TDD）。
- **可依赖**：U2 DTO；`recover_active_waves`/`list_wave_ids`；双实现。
- **禁止依赖**：emission 表、sidecar、ticket。
- **验收测试**：扩展 `integration_wave_inspect.rs`（真实 DB + corrupt）；Store contract 增 public-id 查询；loop inspect availability。
- **需要拆分的单元测试**：InMemory↔Rusqlite differential；终态可读而 `recover_active_waves` 仍只 active；`summarize`/`build_supervisor_summary` 错误映射。
- **Red 预期失败原因**：无按 public id 读终态 API；错误→default。
- **最小实现范围**：`supervisor/mod.rs` trait 只读方法；`memory.rs`/`rusqlite.rs`；CLI 接线；**不** bump migration。
- **集成验证**：`cargo nextest run -p ralph-core -- supervisor`；`… -p ralph-cli --test integration_wave_inspect`；`… --bin ralph -- inspect`。
- **回归范围**：recovery、diagnose、loop inspect schema。
- **完成标准**：四态可区分；敏感字段仍缺席。
- **风险**：reason_code 稳定；原始 sqlite 错误不进 agent JSON。

### Unit 4 — Emission reservation 状态机（只改 Store）

- **Unit 目标**：独立 emission 状态机；同 scope 唯一 owner；同 digest dedup；异 digest conflict；崩溃分类。**CLI 零改动**。
- **对应 Scenario**：S2–S4、S8、S9（Store 层）。
- **外部可观察结果**：contract/并发测绿；`CURRENT_VERSION`→3。
- **输入与输出**：`scope_key`（同现 `compute_scope_key`）、digest、`expected_count` → Reserved/AlreadyApplied/Conflict/RecoveryRequired/FailedPartial。
- **状态**：`reserved → applying → applied | recovery_required | failed`；零写失败可接管。
- **可依赖**：U3；migrations 模式。
- **禁止依赖**：sidecar importer、CLI cutover、ticket。
- **验收测试**（新建 `crates/ralph-core/tests/integration_supervisor_emission_idempotency.rs`）：同 scope barrier 唯一；异 scope 全成功；reopen 保留；digest 冲突。
- **需要拆分的单元测试**：合法/非法迁移表；`expected_count` 边界；v2→v3 保留 waves/slots/seq。
- **Red 预期失败原因**：无 emission API/表；现有 waves.idempotency_key ≠ CLI scope。
- **最小实现范围**：新类型+trait；`migrations/v3.sql`；双实现；**不**改 `wave.rs`。
- **集成验证**：`cargo nextest run -p ralph-core --test integration_supervisor_emission_idempotency`；`… -- supervisor`；`… -- migrations`。
- **回归范围**：register_wave、backpressure、recover、cancel、fan-in、v1/v2。
- **完成标准**：双实现一致；SQLite 并发唯一（禁止进程 Mutex 冒充）；CLI 与 U1 基线仍一致。
- **风险**：public id 关联，不用内部 `w-{seq}` 当 CLI wave_id。

### Unit 5 — Sidecar 导入 + `wave emit` 切 Store

- **Unit 目标**：带 key 路径只问 Store；合法 sidecar 仅 miss 时导入；之后不读写 sidecar。
- **对应 Scenario**：S1、S2、S4、S8–S11、S15。
- **外部可观察结果**：成功 JSON 保留 `ok/wave_id/topic/count/deduplicated`，**移除** `events_file`；新 Apply 不写 sidecar（翻转 U1 sidecar 基线）；双进程同 key→同 id + N 事件。
- **输入与输出**：现有 `WaveEmitArgs` + U4 API + 只读 sidecar parser。
- **可依赖**：U1–U4；FileLock；batch 写；`register_wave_if_absent`。
- **禁止依赖**：U6 ticket 改造（保持现 require_ticket；human/预置 ticket 测 cutover）。
- **验收测试**（新建 `crates/ralph-cli/tests/integration_wave_idempotency_store.rs`）：双进程 dedup；payload conflict；sidecar 导入后删除仍 dedup；mismatch fail-closed；partial fail-closed；JSON 无 `events_file`；human 无 key 两 id。
- **需要拆分的单元测试**：scope/digest 映射（复用现函数，禁复制）；importer 表驱动；zero/full/partial 核对；dispatcher 幂等注册扩展。
- **Red 预期失败原因**：仍先读/写 sidecar。
- **最小实现范围**：`wave.rs` Store orchestrator；必要时 `dispatcher.rs`/`supervisor_bridge.rs`；若 batch append 非原子，**本 Unit 内**补 characterization+最小原子写，禁止留到 U8；Store 路径复用现有 resolver。
- **集成验证**：`… --test integration_wave_idempotency_store`；u9；`… --bin ralph -- wave`；`… wave_supervisor`。
- **回归范围**：policy-check、worker deny、无 wave 零 DB、fan-in。
- **完成标准**：新路径无 sidecar 写；合法导入一次；并发不双写。
- **风险**：importer 仅 miss；不一致 fail-closed；部分批次禁补写。

### Unit 6 — Ticket claim/apply/consume

- **Unit 目标**：校验+claim 原子；Apply 前失败 restore；Store applied 后 consume；cleanup 失败→applied + 引导 inspect。
- **对应 Scenario**：S1、S5、S6、S7、S15。
- **外部可观察结果**：翻转 U1 ticket 基线；S7 响应含 `applied`/`applied_cleanup_pending`；重试不增事件。
- **输入与输出**：ticket 文件（可演进，agent 不可见）+ U5 emission 结果。
- **可依赖**：U5；`wave_verify_gate.rs`；u9。
- **禁止依赖**：U7 文档、U8 BDD 补逻辑。
- **验收测试**（新建 `crates/ralph-cli/tests/integration_wave_ticket_recovery.rs`）：写前失败可重试；mismatch 保持 prepared；并发单 claim；cleanup 失败可 inspect；human bypass。
- **需要拆分的单元测试**：prepared↔claimed→consumed；mismatch 表；stale 恢复；更新而非删除既有 ~12 gate 测。
- **Red 预期失败原因**：先删后校验；无 claim/restore。
- **最小实现范围**：仅 wave ticket + emit 编排；不改 task gate。测试 hook 注入写前失败与 cleanup 失败；禁止 mock 掉「事件是否写入」。
- **集成验证**：`… --test integration_wave_ticket_recovery`；u9；`… --bin ralph -- wave_verify`。
- **回归范围**：u9、human bypass、unsafe-policy 正交、worker deny。
- **完成标准**：S5–S7 绿；Apply 前无需 re-verify。
- **风险**：错误不泄漏 ticket 路径/哈希。

### Unit 7 — Agent/CLI 契约文档与 drift

- **Unit 目标**：help + `ralph-tools-wave.md` + cmdref + 必要 operator references 与行为一致；删 zero-disk、ledger Confirm、sidecar 权威。
- **对应 Scenario**：S14。
- **外部可观察结果**：skill 路径为 verify→emit→`wave inspect`；有停止/重试条件。
- **可依赖**：U2–U6 最终字段/子命令。
- **禁止依赖**：把逻辑补丁留到 U8。
- **验收测试**：help 允许/禁止词；`scripts/check-cli-doc-drift.sh --strict`；禁止 `zero-disk`、Confirm 读 `.ralph/events.jsonl`、`supervisor.db`。
- **需要拆分的单元测试**：无；静态门禁。禁锁定整段 preset prompt。
- **Red 预期失败原因**：help/skill 旧文案。
- **最小实现范围**：`ralph-tools-wave.md`、cmdref、`wave.rs` help、必要时 `skills/ralph-preset-common/references/commands.md`；遵守可读性/去计划化。
- **集成验证**：三命令 `--help` + drift strict。
- **回归范围**：全部 `ralph-tools*.md` 引用。
- **完成标准**：S14 绿；恢复规则含触发/动作/字段来源/停止条件。
- **风险**：「不写业务事件」≠「无副作用」。

### Unit 8 — 真 Runtime BDD + 关键 E2E + 全量门禁

- **Unit 目标**：跨层证明 S1/S2/S7/S15 等；**禁止**新功能。缺口回归属 Unit。
- **对应 Scenario**：全 Feature。
- **可依赖**：U1–U7 关闭。
- **禁止依赖**：`run_scenario` stub；「仅 fixture 未注册」当有效 Red。
- **验收测试**：新建 `crates/ralph-core/tests/scenarios/wave_protocol/normal_apply_confirm.yml`、`recovery_required.yml`；`scenarios.rs` 用 **`run_workflow_guard_scenario`**；新建 `crates/ralph-cli/tests/integration_wave_protocol_closure.rs`（双进程、cleanup 失败、污染 env、无 wave 零 DB）。
- **Red 预期失败原因**：受控故障 seam（禁 Store commit / ticket restore）下能抓住重复写或伪 Confirm。
- **最小实现范围**：fixture/harness/修编译。
- **集成验证**（顺序）：
  - `cargo nextest run -p ralph-cli --test integration_wave_inspect`
  - `cargo nextest run -p ralph-cli --test integration_wave_idempotency_store`
  - `cargo nextest run -p ralph-cli --test integration_wave_ticket_recovery`
  - `cargo nextest run -p ralph-cli --test integration_wave_protocol_closure`
  - `cargo nextest run -p ralph-core --test integration_supervisor_emission_idempotency`
  - `cargo nextest run -p ralph-core --test scenarios -- wave_protocol`
  - preset_lint（cli+core）；`scripts/check-cli-doc-drift.sh --strict`
  - `cargo fmt --all --check`；`cargo clippy --workspace --all-targets`；`cargo build --workspace`
  - `./scripts/run-tests.sh`（仅 flake 时 `RALPH_BASELINE_SERIAL=1`）
- **回归范围**：全 workspace nextest+doctest。
- **完成标准**：S1–S15 均有文件证据；无 skip/ignore；无 ephemeral 入 diff。
- **风险**：Cargo 锁≠产品并发；BDD 须断言 events/公开输出。

---

## 6. 最终质量门禁

1. S1–S15 全部可追踪且通过。
2. 单元测试全绿；InMemory↔Rusqlite contract 一致。
3. inspect / idempotency store / ticket recovery / emission concurrency / migration / u9 通过。
4. 关键 E2E：Apply→Confirm、同 key 双进程、cleanup 失败通过。
5. 真 EventLoop BDD（`run_workflow_guard_scenario`）通过。
6. 未改 preset/schema 则记录「无需同步」；改了则 lint/parity 绿。
7. 三命令 help 与 skill 一致；drift strict 通过。
8. fmt / clippy / build / `./scripts/run-tests.sh` 通过。
9. 无新增 failed/skipped/ignored、无削弱断言、无无解释 golden、无 Mock 代替真实并发/写盘断言。
10. 注入 skill 符合可读性/去计划化；operator skills 按需同步。
11. 未改则不动 `CLAUDE.md`/`AGENTS.md`；若改命令列表则两者一致。
12. 残留仅非阻塞：极端双损坏人工恢复、跨主机多写、强制 agent key、Web/TUI——不在本计划。

### Executor 速查

| Unit | 主要路径 |
|---|---|
| 1 | 新建 `crates/ralph-cli/tests/integration_wave_protocol_baseline.rs` |
| 2 | `crates/ralph-cli/src/wave.rs`；新建 `integration_wave_inspect.rs` |
| 3 | `supervisor/{mod,memory,rusqlite}.rs`；`commands/inspect.rs` |
| 4 | 新建 `migrations/v3.sql`；Store trait/双实现；新建 emission 集成测 |
| 5 | `wave.rs`；必要时 dispatcher/bridge；新建 `integration_wave_idempotency_store.rs` |
| 6 | `wave_verify_gate.rs`；新建 `integration_wave_ticket_recovery.rs` |
| 7 | `ralph-tools-wave.md`；help；drift |
| 8 | `scenarios/wave_protocol/*.yml`；`integration_wave_protocol_closure.rs`；全量门禁 |

### 关键决策（压缩）

1. Emission 与 runtime waves 分表；public wave id 关联。
2. SQLite UNIQUE 裁决并发；InMemory 镜像。
3. Sidecar 只读迁移；成功后零依赖。
4. Ticket prepared→claimed→consumed；cleanup 失败不回滚 applied。
5. Confirm=`wave inspect`；loop inspect 加 availability。
6. 部分批次 fail-closed；失败可恢复优先于假成功。

### 参考

- `crates/ralph-cli/src/wave.rs`、`wave_verify_gate.rs`、`commands/inspect.rs`
- `loop_runner/wave/dispatcher.rs`、`supervisor_bridge.rs`
- `crates/ralph-core/src/supervisor/{mod,memory,rusqlite,bridge,migrations}.rs`
- `crates/ralph-cli/tests/integration_wave_protocol_suite_u9.rs`
- `crates/ralph-core/data/ralph-tools-wave.md`
- `docs/achieved/plan/2026-07-22-001-feat-wave-protocol-suite-default-plan.md`
