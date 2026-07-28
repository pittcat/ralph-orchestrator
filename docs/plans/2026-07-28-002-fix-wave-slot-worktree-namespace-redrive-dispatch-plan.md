---
title: Wave Slot Worktree 命名空间迁移 + Redrive 派发闭环（含 descriptor 生产闭环补齐）
type: fix
date: 2026-07-28
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
product_contract_source: ce-plan
execution: code
rewrite_of: 同一文件 v1 首版与 v1.1 重写版（审计发现 G1/G2/G3 生产闭环缺口后修订）
---

# Wave Slot Worktree 命名空间迁移 + Redrive 派发闭环（含 descriptor 生产闭环补齐）

## Goal Capsule

- **目标：** 修复同一根因链上的四层机制缺口：
  1. `bind_slot` branch 命名不含 `wave_id` → 同 loop 跨 wave 同 slot_index 必撞名 → `slot_never_started`；
  2. `persist_slot_descriptor` 生产零调用 → 没有任何 descriptor 被持久化；
  3. rusqlite 生产 store 无 descriptor 存取实现（无 v10 表、无 override）→ 默认 impl 恒 `DescriptorUnavailable`；
  4. `create_redrive_wave` 不把父 wave descriptor 复制/重映射到子 wave（且 child slot 被重编号）→ 即使 2/3 修好，`take(child_id, child_slot)` 也永远拿不到；随后 runner boot 无 redrive 派发步骤。
- **审计来源：** 本会话对 v1.1 的逐条源码核验（E1-E12 + G1-G3），修订点见「修订日志」。
- **执行方式：** U1 → U2 → U3 → U4 → U5 严格串行；每 Unit Acceptance Red → Green → 回归 → 独立提交边界。
- **停止条件：** 真实调用链与 Evidence 冲突、预期 Red 未触达目标逻辑、需要新增未计划的公开接口或依赖、任一关键决策置信度降到 0.85 以下。

---

## 修订日志（相对 v1.1）

| # | v1.1 的问题 | 本版处置 | 证据 |
|---|---|---|---|
| M1 | E2 称「`worktree.rs` 无 `AlreadyExists` 变体」——字面错误 | 修正为：变体存在（`worktree.rs:113-115`），但失败链经 `worktree_bind.rs:94-100` From-impl 塌缩为 `CreateFailed`、经 `supervisor_bridge.rs:346` 归 `BridgeError::Store`，结论不变 | E2 |
| M2 | E5 引用 `1f955f6b` 时略去「rusqlite store + v10 migration 同上（留待后续）」半句 | 补全引用；rusqlite override + v10 列入正式范围（U2） | E5 |
| M3 | KTD4「不改 store 层，memory override 已存在」置信度 0.95 虚高——对生产 rusqlite 为假 | 废止 KTD4，替换为 KTD4'：store 层 descriptor 闭环必须补齐（G1） | E6/E7 |
| M4 | 未发现 `persist_slot_descriptor` 生产零调用（G2） | U3：dispatcher spawn 时 persist 接线 | E8 |
| M5 | 未发现 `create_redrive_wave` 不复制 descriptor + child slot 重编号（G3）——v1.1 的 S2 即使在 in-memory + 手动 persist 下也是以「测试手动往 child key 塞 descriptor」的方式造假通过 | U2：`create_redrive_wave` 复制并重映射 descriptor（两 store）；`list_redrive_pending_child_waves` 返回携带 parent slot 映射与期望 digest 的 enriched 元组 | E9/E10 |
| M6 | R7 强制「调 `take_dispatchable_redrive_descriptor`」但未解决「take 前不知道 parent slot 映射与 expected_digest」的鸡生蛋问题 | KTD8：list 方法返回 enriched 元组（child_slot → parent_slot + expected_digest），take 按 004 契约做 store 侧三态校验 | E9/E11 |
| M7 | 实现注记写 `ctx.wave_id`——实际 `wave_id` 是 `bind_slot` 的直接参数（`supervisor_bridge.rs:307`） | 修正 | E1 |
| M8 | 「boot 能否凑齐 executor 参数」作为 <0.85 假设混入 READY | 已勘察闭合：合成 `DetectedWave` 的字段来源全部定位（KTD9），pre-registered 注册模式消除 idempotency key 缺口 | E11/E12 |
| M9 | S2-S4 全部 in-memory，生产 rusqlite 闭环零覆盖 | U4 至少一个 rusqlite-backed boot dispatch 集成测试（feature `supervisor-db` 默认启用） | E7 |

---

## Product Contract

### 0. 计划状态

**READY（v2 修订版）。**

- **代码基线：** `8968e44a`（`docs(plans): 新增 Wave Slot 与 Worker 启动宽限期两份修复计划`）。
- **调查范围：** `crates/ralph-cli/src/loop_runner/wave/{supervisor_bridge,dispatcher}.rs`、`crates/ralph-cli/src/loop_runner/runner.rs`、`crates/ralph-core/src/supervisor/{mod,memory,rusqlite,worktree_bind,u4_descriptor_tests}.rs`、`crates/ralph-core/src/{worktree.rs,wave_detection.rs}`、`crates/ralph-proto/src/event.rs`、`crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs`、`1f955f6b` commit。
- **已执行验证：** 全部 Evidence 逐条源码/grep 核验（含 v1.1 未发现的 G1/G2/G3 三处零命中验证）；commit message 原文核对。
- **尚未执行验证：** 各 Unit 的 Acceptance Red/Green、build、clippy、全量测试。
- **阻塞项：** 无。

### 1. 功能目标

#### 业务目标

1. 同一 loop 内任意新 exec/fix wave（含第二波、redrive 子 wave）的 slot 都拿到**全新且独立**的 worktree（branch 名含 `wave_id`）。
2. slot 派发时其 activation descriptor 被**生产路径**持久化（memory + rusqlite 双 store）。
3. `ralph wave redrive` 创建的子 wave 携带从父 wave 复制/重映射的 descriptor；`ralph run --resume` boot 时扫描处于 `dispatch` 相位的子 wave，经三态校验后复用现有 dispatcher 链路真正派发 worker。

#### 当前行为（均有证据）

1. **命名撞名（G0）：** `supervisor_bridge.rs:342` 与 `worktree_bind.rs:166` 两处 branch 命名均为 `{loop_id}-{kind}-{slot_index}`，不含 `wave_id`；slot_index 是 per-wave 0..N 重编（`memory.rs:1420-1435` 亦是），同 loop 第二波 exec/fix 必撞 → `factory.create` 失败 → `BridgeError::Store` → `slot_never_started`。
2. **persist 零调用（G2）：** `persist_slot_descriptor` 全仓生产零调用（grep 仅 `u4_descriptor_tests.rs:64/113/135` 三处测试调用）。
3. **rusqlite 无 descriptor 实现（G1）：** `rusqlite.rs` 全文 grep `descriptor` 零命中——无 v10 表、无 override；trait 默认 impl 恒返回 `Ok(())` / `DescriptorUnavailable`（`mod.rs:1485-1520`，注释明写 "production stores MUST override"）。
4. **redrive 复制缺失 + slot 重编号（G3）：** `create_redrive_wave`（`memory.rs:1281-1462`）把 parent 失败 slot 按 `enumerate()` 重编号为 child slot 0..n（`memory.rs:1420-1435`），**不复制** `slot_descriptors`；`take` 按 `(child_wave_id, child_slot)` 直查（`memory.rs:1496-1497`）→ 永远 `DescriptorUnavailable`。
5. **boot 无派发（v1.1 已识别）：** `take_dispatchable_redrive_descriptor` 生产零调用；`runner.rs:1373-1396 / 1462-1484` 两处启动恢复只做超时标记 + projection 重放。`1f955f6b` commit message 自承：「`ralph run --resume` 启动时消费 take_dispatchable_redrive_descriptor 的接线留待后续 plan 收尾；**rusqlite store + v10 migration 同上**」。

#### 目标行为与行为差异

- `bind_slot` 命名改为 `{loop_id}-{kind}-{wave_id}-{slot_index}`（两处对称，`wave_id` = store wave id，即 `bind_slot` 已有的直接参数）。
- dispatcher 在 slot 绑定成功、worker spawn 前持久化 `SlotDescriptor`（topic/payload/wave_kind/digest 全部来自当前 dispatch 上下文）。
- `create_redrive_wave` 把 parent 失败 slot 的 descriptor 按「parent_slot → child_slot」重映射复制到 child key；descriptor 内 `slot_index` 字段保留 **parent** 原始下标。
- `ralph run --resume` boot 时对每个 pending child slot：经 enriched list 取得 `(child_slot, parent_slot, expected_digest)` → `take_dispatchable_redrive_descriptor` 三态校验 → `Dispatchable` 则以 descriptor 合成单 wave 的 `DetectedWave` 复用 `execute_wave_via_supervisor_with_executor`（pre-registered 模式）派发；`DescriptorUnavailable` / `DescriptorConflict` fail-closed 标 `slot_never_started` + 诊断。

#### 输入 / 输出 / 状态变化 / 错误语义

- **输入：** `SupervisorStore` wave 拓扑与 descriptor 存储、`ralph run --resume` 启动路径、preset hat 配置（合成 `DetectedWave` 的 `hat_config` 来源）。
- **输出：** 每次 `bind_slot` 返回全新 `Worktree { path, branch }`；boot 时 redrive 子 wave 的 dispatchable slot 被派发（日志可见）；失败 slot 诊断落 `diagnostics`。
- **状态变化：** 同 loop 多 wave 不再受命名空间限制；descriptor 生命周期完整（spawn persist → redrive copy → boot take）。
- **错误语义：** descriptor 缺失/digest 冲突/映射缺失一律 fail-closed（`slot_never_started` + 诊断），绝不静默派发。

#### 兼容、性能、安全与约束

- **兼容：** 老命名残留 worktree 不被新 wave 撞名（新命名多 `wave_id` 段），由既有 `finalize_terminal_cleanup` 回收；v10 migration 为纯增量表，旧 DB 打开自动迁移；`create_redrive_wave` 的 API 签名与 `RedriveResult` 不变（仅行为补齐：多写 descriptor 副本）。
- **性能：** persist 是每 slot 一次小写；boot 扫描一次性；无热路径开销。
- **安全：** 不放宽 EventOriginGuard / HatCommandPolicy / wave permit 边界；descriptor 不含 prompt/agent stdout/凭据（`mod.rs:883-885` 既有约束）；digest 比较 fail-closed。
- **测试入口：** 严禁裸跑 `cargo test -p ralph-cli`；一律 nextest 两阶段（`AGENTS.md` HARD RULE 1/2）；spawn `ralph` 的测试遵守 HARD RULE 5。

#### 本次范围

- 命名空间迁移（`supervisor_bridge.rs:342` + `worktree_bind.rs:166` 两处 + `worktree_bind.rs` 测试断言同步）。
- v10 migration（`slot_descriptors` 表）+ rusqlite `persist_slot_descriptor` / `take_dispatchable_redrive_descriptor` / `slot_descriptor`（读）三个 override + memory 对称读方法。
- `create_redrive_wave`（memory + rusqlite）descriptor 复制与 slot 重映射。
- dispatcher spawn 时 persist 接线（新 `SupervisorBridge::persist_slot_descriptor` 转发方法）。
- `SupervisorStore::list_redrive_pending_child_waves`（enriched：child_slot / parent_slot / expected_digest）trait + 双 store 实现。
- runner boot `dispatch_pending_redrive_waves`（两处对称插入点）+ 合成 `DetectedWave` + pre-registered 派发模式。
- 测试：`wave_supervisor.rs`（S1 真 git worktree、S2-S4 含 rusqlite-backed）+ store 层单测。

#### 非目标

- 不引入「撞名兜底」（命名迁移后跨 wave 不撞名；同 wave 同 slot 重派是 DB 异常场景）。
- 不在 dispatcher tick 兜底 redrive 扫描（boot 一次扫描覆盖主路径；operator 运行时 redrive 的二次消费属未来用例）。
- 不改 `create_redrive_wave` 的 API 签名 / `RedriveResult` / `RedriveTakeOutcome` 三态定义。
- 不改 worker 进程模型 / PTY / `worker.rs`；不改 preset 拓扑与 hat 指令。
- 不重构 merge-queue / loops.json / `--reuse-worktree` 整 loop 路径；不回收老命名残留（既有 cleanup 负责）。
- 不改 `crates/ralph-core/data/*.md`：`ralph-tools-wave.md:314` 已按目标行为成文（"operator 必须接着执行 `ralph run --resume`，由 loop 启动 seam 消费 child descriptor"），本计划是把实现补齐到既有文档语义；U5 仅核对不扩写。

### Requirements

- **R1.** branch 命名含 `wave_id`（store wave id），同 loop 跨 wave 不撞名；两处在代码中只剩两处。
- **R2.** 命名满足 git branch 合法字符集与长度约束；exec/fix/review 一致（review 仍 SharedReadonly 不建 worktree）。
- **R3.** slot 派发成功绑定后、spawn 前，descriptor 被持久化到当前 store（memory + rusqlite）；persist 失败 fail-closed（slot 记 `slot_never_started` 等价诊断，不静默继续）。
- **R4.** rusqlite descriptor 存取与 memory 语义一致（v10 表 + 三 override）；`InMemory` 新增 `slot_descriptor` 读方法，trait 默认返回 `None`。
- **R5.** `create_redrive_wave` 把每个 target parent slot 的 descriptor 复制到 child key（child slot 下标），descriptor 内 `slot_index` 保留 parent 下标；无 descriptor 的 parent slot 在 enriched list 中 `expected_digest = None`（boot fail-closed 为 Unavailable）。
- **R6.** `list_redrive_pending_child_waves` 返回 `Vec<RedrivePendingChild>`：`{ child_wave_id, parent_wave_id, kind, slots: Vec<{ child_slot_index, parent_slot_index, expected_digest: Option<String> }> }`，过滤 `parent_wave_id IS NOT NULL AND phase = 'dispatch'`。
- **R7.** boot 对每个 pending child slot 调 `take_dispatchable_redrive_descriptor(child_wave_id, child_slot, expected_digest)`，三态分支：`Dispatchable` → 派发；其余 → `slot_never_started` + 诊断。
- **R8.** 派发复用 `execute_wave_via_supervisor_with_executor`，通过 pre-registered 模式跳过 `register_wave_if_absent`（child 已注册，无 idempotency key）；不新增 dispatch 旁路函数。
- **R9.** 合成 `DetectedWave`：`wave_id` = child store wave id；`events` 按 child_slot 序由 descriptor 还原（topic/payload + `wave_index = child_slot`、`wave_total = child.expected_total`、`wave_id = child_wave_id`）；`target_hat`/`hat_config` 经 `HatRegistry::find_by_trigger(descriptor.topic)` 解析；`consumer_aggregate_timeout = None`（走 dispatcher 既有回退公式）。
- **R10.** S2-S4 至少各一个集成测试；S2 必须含 rusqlite-backed 变体。
- **R11.** 命名迁移集成测试 S1 用真 git worktree。

### BDD 行为规格

```gherkin
Feature: Wave slot worktree namespace migration

  Scenario S1: 同 loop 第二波 exec 不撞第一波 worktree
    Given wave w-1 exec slot 0 已在 branch loop-1-exec-w-1-0 建 worktree 且终态
    When wave w-2 exec slot 0 绑定
    Then bind 成功且 branch 为 loop-1-exec-w-2-0
    And 无任何 factory 错误

Feature: Descriptor 生产闭环

  Scenario S2a: spawn 时 descriptor 持久化（双 store）
    Given 一个 exec wave 注册并绑定 slot 0
    When worker spawn 链路执行到绑定成功
    Then store 的 (wave_id, 0) 处可读到 SlotDescriptor
    And topic/payload/wave_kind 与当前 dispatch 上下文一致
    And payload_digest == fingerprint_payload(payload)

  Scenario S2b: redrive 复制 descriptor 并重映射
    Given 父 wave w-1 slot 4 已持久化 descriptor 且 slot 4 Failed
    When operator 执行 ralph wave redrive --wave-id w-1
    Then 子 wave w-2 的 (w-2, 0) 处可读到同一 descriptor
    And 该 descriptor 的 slot_index 字段 == 4（parent 下标）

Feature: Redrive boot 派发

  Scenario S3: resume 后子 wave 自动派发
    Given 父 wave 失败 slot 已 persist descriptor，redrive 已创建子 wave w-2（phase=dispatch）
    And loop 崩溃重启
    When ralph run --resume 完成 recover_active_waves_at_startup
    Then dispatch_pending_redrive_waves 扫描到 w-2
    And take 返回 Dispatchable
    And execute_wave_via_supervisor_with_executor 以 pre-registered 模式 spawn worker（spawn 计数 == 1）

  Scenario S4: descriptor 缺失 fail-closed
    Given 子 wave w-2 slot 0 pending 但无 descriptor（pre-U4 legacy 行）
    When boot 扫描执行
    Then take 返回 DescriptorUnavailable
    And slot 标 slot_never_started + 诊断，不 spawn

  Scenario S5: digest 冲突 fail-closed
    Given 子 wave descriptor 的 digest 与父 descriptor digest 不一致（复制后被篡改）
    When boot 扫描执行
    Then take 返回 DescriptorConflict
    And slot 标 slot_never_started + 诊断，不 spawn

  Scenario S6: 非 resume 的全新 boot 不触发 redrive 扫描
    Given 一个全新 loop（无 --resume）
    When loop 启动
    Then 不执行 dispatch_pending_redrive_waves
```

---

## Planning Contract

### 2. 代码库现状与证据

#### 2.1 当前实现入口

- **命名点（两处对称）：** `crates/ralph-cli/src/loop_runner/wave/supervisor_bridge.rs:342`（内联，`bind_slot(kind, wave_id, slot_index)` 签名于 `:304-309`，`wave_id` 为直接参数）；`crates/ralph-core/src/supervisor/worktree_bind.rs:166`（helper，`bind_slot_worktree(..., wave_id: &str, ...)` 签名于 `:114-128`，wave_id 已传但未用于命名）。
- **失败链：** `worktree.rs:113-115` `AlreadyExists` 变体存在；`worktree_bind.rs:94-100` From-impl 把非 `NotARepo` 一律塌缩为 `CreateFailed`；`supervisor_bridge.rs:346` 一律归 `BridgeError::Store`；`dispatcher.rs:1786-1792` `fail_closed_on_bind_error` → slot skipped。
- **spawn 派发上下文（persist 接线点）：** `dispatcher.rs:1783` `bridge.bind_slot(...)` 成功分支；`wave.events[index]`（topic/payload）与 `wave_kind` 同作用域。
- **descriptor 契约：** `mod.rs:887-912`（`SlotDescriptor` + `digest_of`）；`mod.rs:787` `fingerprint_payload`；`mod.rs:1485-1520`（trait persist/take 默认 impl）；`memory.rs:1469-1514`（memory override）；`u4_descriptor_tests.rs`（4 个三态单测，但 persist/take 同 key，未覆盖 parent→child 流程）。
- **redrive 创建：** `memory.rs:1281-1462`（child slot `enumerate()` 重编号于 `:1420-1435`；`WaveRow` 无 descriptor 复制、无 idempotency key 注册）；`rusqlite.rs:1687-1889`（同语义 SQL 版）；`crates/ralph-cli/src/wave.rs:376-450` CLI。
- **注册契约：** `memory.rs:388-400`（重复注册 kind/total/budget 不一致报错）；`rusqlite.rs:347,379-384`（`waves.idempotency_key` 列存在，注释 `:419-422` 称其 alias public id）。
- **boot 恢复点：** `runner.rs:1373-1396` 与 `1462-1484`（两处对称）；`runner.rs:796` `run_loop_impl_inner`。
- **合成 DetectedWave 依据：** `wave_detection.rs:25-46`（字段）；`wave_detection.rs:353-361`（构造：target_hat/hat_config/consumer_aggregate_timeout 解析）；`wave_detection.rs:368-374`（`HatRegistry::find_by_trigger`）；`dispatcher.rs:1615-1620`（`consumer_aggregate_timeout=None` 时的回退公式）；`event.rs:21-31`（`wave_id/wave_index/wave_total` 为 Event 一等字段）。
- **目标行为既有文档：** `crates/ralph-core/data/ralph-tools-wave.md:314`（resume 消费 child descriptor 的语义已成文）。

#### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `supervisor_bridge.rs:304-309,342` + `worktree_bind.rs:114-128,166` | 两处命名均 `{loop_id}-{kind}-{slot_index}`；`wave_id` 在两处签名都已可访问（直接参数） | 迁移对称且无需新增字段 | 高 |
| E2 | `worktree.rs:113-115` + `worktree_bind.rs:94-100` + `supervisor_bridge.rs:346` | `AlreadyExists` 变体存在；From-impl 塌缩为 `CreateFailed` → `BridgeError::Store` | 失败链结论成立；v1.1「无 AlreadyExists 变体」陈述错误已修正 | 高 |
| E3 | `memory.rs:1420-1435` + dispatcher slot 语义 | slot_index per-wave 0..N 重编 | 第二波必撞（G0） | 高 |
| E4 | grep `take_dispatchable_redrive_descriptor` 生产路径 | 零调用（仅 `u4_descriptor_tests.rs:68/91/117`） | 必须接 boot 消费 | 高 |
| E5 | `1f955f6b` commit message 原文 | resume 接线 **与** rusqlite store + v10 migration 均自承留待后续 | 两项都入范围（废止 v1.1 KTD4） | 高 |
| E6 | grep `rusqlite.rs` `descriptor` | 零命中：无 v10 表、无 override | U2 必须补 rusqlite 实现 | 高 |
| E7 | `mod.rs:1485-1520` | 默认 impl：`persist → Ok(())`、`take → DescriptorUnavailable`；注释 "production stores MUST override" | 生产 rusqlite 当前恒 fail-closed；双 store 实现是硬要求 | 高 |
| E8 | grep `persist_slot_descriptor` 生产路径 | 零调用（仅测试 `u4_descriptor_tests.rs:64/113/135`） | U3 persist 接线是硬要求 | 高 |
| E9 | `memory.rs:1420-1435, 1496-1497` | child slot 按 `enumerate()` 重编号；take 按 `(child_wave_id, child_slot)` 直查；`create_redrive_wave` 不复制 descriptor | U2 必须复制+重映射 descriptor；list 必须带 parent slot 映射 | 高 |
| E10 | `u4_descriptor_tests.rs:51-70` | 既有测试 persist/take 同 wave 同 slot，未覆盖 parent→child | v1.1 的 S2 测试设计造假路径已识别；新测试必须走真 redrive 流程 | 高 |
| E11 | `wave_detection.rs:25-46,353-361,368-374` + `dispatcher.rs:1615-1620` | `DetectedWave` 构造依赖 HatRegistry 解析 + `consumer_aggregate_timeout` 私有 helper；dispatcher 有 None 回退 | 合成 wave 可行；`consumer_aggregate_timeout=None` 有既有回退承接 | 高 |
| E12 | `event.rs:21-31` + `memory.rs:1437-1453` | Event 一等字段 `wave_id/wave_index/wave_total`；child `WaveRow` 无 idempotency key | 合成 Event 合法；pre-registered 模式（跳过 register）是必需而非可选 | 高 |
| E13 | `runner.rs:1360-1405, 796` | boot 作用域持有 concrete bridge（`store()` 可取）、supervisor_cfg、loop_id | boot 派发所需上下文齐备 | 高 |
| E14 | `mod.rs:1531-1543` | emit 侧 `reserve_emission` 已做 scope_key digest Conflict | digest 冲突的首要防线在 emit 侧；boot 侧 digest 用「同 DB 父 descriptor」作锚即可，无需 ledger 扫描 | 高 |

#### 2.3 受影响范围

- **生产：** `supervisor_bridge.rs`（cli）、`worktree_bind.rs`、`mod.rs`（trait+类型）、`memory.rs`、`rusqlite.rs`、`migrations/v10.sql`（新增）、`migrations.rs`（注册）、`dispatcher.rs`（persist 接线 + pre-registered 模式）、`runner.rs`（boot 步骤）。
- **测试：** `wave_supervisor.rs`、`u4_descriptor_tests.rs`（或同目录新文件）、`worktree_bind.rs` 内联测试断言。
- **不受影响：** DB 既有表结构（v10 纯增量）、preset/CLI 表面/worker.rs/`create_redrive_wave` 签名、`crates/ralph-core/data/*.md`（语义已成文）。

### 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
|---|---|---|---|---|---|---|
| KTD1 | 命名空间维度 | 加 wave_id；加 attempt_epoch；加 retry budget | `{loop_id}-{kind}-{wave_id}-{slot_index}`，`wave_id` = store wave id（`bind_slot` 直接参数） | E1, E3 | attempt_epoch 不覆盖跨 wave；budget 破坏「同 wave 同 slot 同 branch」 | 0.93 |
| KTD2 | 撞名兜底 | fail-closed；wave-phase 回收；不兜底 | 不兜底（跨 wave 不撞名后，撞名即 DB 异常） | E1, E3 | 用户决策；引入 store 状态查询得不偿失 | 0.90 |
| KTD3 | redrive 消费点 | dispatcher tick；boot 一次扫描；独立子命令 | boot 一次扫描（两处对称插入点） | E4, E5 | 用户决策；覆盖 `ralph wave redrive` 后重启主路径 | 0.90 |
| KTD4' | store 层缺口处置 | 只做 runner 接线（v1.1）；补 rusqlite + v10 + 复制 | **补齐 store 闭环**：v10 表 + rusqlite 三 override + memory 读方法 + `create_redrive_wave` descriptor 复制（双 store） | E5, E6, E7, E9 | v1.1 方案在生产 rusqlite 恒 fail-closed（Dead on arrival）；G3 使 in-memory 也不闭环 | 0.92 |
| KTD5 | worker prompt 携带 redrive 标记 | 携带；不携带 | 不携带（hat 经 task ledger/trigger payload 识别上下文） | prompt 简洁性；worker 环境一致原则 | 行为分裂风险 | 0.85 |
| KTD6 | persist 接线点与通道 | dispatcher spawn 前经 bridge 转发；runner 侧事后补写；emit 时写 | `dispatcher.rs:1783` bind 成功后、spawn 前，经新 `SupervisorBridge::persist_slot_descriptor` 转发（无默认实现，编译器驱动全 impl） | E8 + `dispatcher.rs:1783` 作用域证据 + 004 设计原文（"registers the ready-event snapshot at spawn time"） | runner 补写脱离 dispatch 上下文（topic/payload 不在手）；emit 时写早于 bind/dispatch 成功，会残留虚假 descriptor | 0.88 |
| KTD7 | digest 锚 | ledger 扫描父 ready 批次；同 DB 父 descriptor | **同 DB 父 descriptor**（enriched list 携带 `expected_digest`） | E14（emit 侧已有 digest Conflict 首道防线） | ledger 扫描需 public↔store 双 ID 换算 + 轮换脆弱性；同 DB 锚的增量风险已被 emit 侧承接 | 0.86 |
| KTD8 | take 的 expected_digest 来源 | list 返回 enriched 元组；boot 先读后比 | `list_redrive_pending_child_waves` 返回 `(child_slot, parent_slot, expected_digest)`，take 按 004 契约 store 侧三态 | E9, E10 | 「先读 child descriptor 拿 parent_slot 再比」把三态校验移出 store，削弱 fail-closed 构造保证 | 0.87 |
| KTD9 | 合成 DetectedWave 与注册 | 新建旁路 dispatch；pre-registered 模式复用现有函数 | `execute_wave_via_supervisor_with_executor` 增加 pre-registered 入口（`store_wave_id` 已存在时跳过 `register_wave_if_absent`）；`DetectedWave.wave_id` = child store id；`consumer_aggregate_timeout = None` | E11, E12 | child 无 idempotency key，走 register 必然新铸 wave 或撞 E12 mismatch；旁路函数违背 R8 | 0.85 |
| KTD10 | child slot 重编号语义 | 改 create_redrive_wave 不重编号；保留重编号 + descriptor 携带 parent 下标 | 保留重编号；复制的 descriptor `slot_index` 字段 = parent 下标 | E9；`RedriveResult.slots` 契约不变 | 改重编号破坏 004 已发布行为与 CLI 输出语义 | 0.90 |

### 4. 实施契约

#### 4.1 命名空间契约

`{loop_id}-{kind}-{wave_id}-{slot_index}`，`wave_id` = store wave id（`bind_slot`/`bind_slot_worktree` 的直接参数）。仅 `supervisor_bridge.rs:342` 与 `worktree_bind.rs:166` 两处；`worktree_bind.rs:276/318/386` 三处测试断言同步（已核验 `:276` 现状为 `"loop-1-exec-0"`）。

#### 4.2 Descriptor 生命周期契约

```text
spawn（U3）          redrive CLI（U2）              boot（U4）
dispatcher bind 成功 → persist(parent_wave, parent_slot)
                     → create_redrive_wave 复制:
                       (parent, p) → (child, c)，descriptor.slot_index 保持 p
                     → list 返回 (c, p, expected_digest=parent digest)
                     → take(child, c, expected_digest) 三态
                     → Dispatchable → 合成 wave → pre-registered 派发
```

- v10.sql：`slot_descriptors(wave_id TEXT, slot_index INTEGER, slot_index_in_parent INTEGER, topic TEXT, payload_json TEXT, wave_kind TEXT, payload_digest TEXT, PRIMARY KEY (wave_id, slot_index))`（`slot_index_in_parent` 支撑 enriched list 的 SQL JOIN；memory 侧用 descriptor.slot_index 字段即可，无需镜像列）。
- rusqlite override 语义与 memory 逐条对齐（persist 未知 wave → `UnknownWave`；take 三态；digest 比较严格相等）。

#### 4.3 Boot dispatch 契约

1. 两处 `recover_active_waves_at_startup` 之后（`runner.rs:1373-1396` / `1462-1484`）插入 `dispatch_pending_redrive_waves`，仅 resume/带 supervisor store 路径执行（S6）。
2. `list_redrive_pending_child_waves` → 逐 child slot：`expected_digest = None` → Unavailable 分支；否则 `take(child, c, digest)` 三态。
3. `Dispatchable` → 按 R9 合成 `DetectedWave` → `execute_wave_via_supervisor_with_executor(..., pre_registered = Some(child_wave_id))`。
4. 失败分支写诊断（沿用 `RecoveryDiagnosisEnvelope` 既有通道）+ slot `slot_never_started`。
5. 幂等：dispatched 后 slot 非 pending，二次扫描自然跳过（单测锁定）。

#### 4.4 封闭文件清单

| # | 文件 | 动作 | 原因 |
|---:|---|---|---|
| 1 | `crates/ralph-cli/src/loop_runner/wave/supervisor_bridge.rs` | 修改 | `:342` 命名加 `wave_id` 段 |
| 2 | `crates/ralph-core/src/supervisor/worktree_bind.rs` | 修改 | `:166` 命名 + `:276/318/386` 断言 |
| 3 | `crates/ralph-core/src/supervisor/mod.rs` | 修改 | `list_redrive_pending_child_waves` / `slot_descriptor` trait 方法与类型；`SupervisorBridge::persist_slot_descriptor` 转发（ralph-core 侧 trait 若在别处则就近） |
| 4 | `crates/ralph-core/src/supervisor/memory.rs` | 修改 | 三个 descriptor 方法 + `create_redrive_wave` 复制 + list 实现 |
| 5 | `crates/ralph-core/src/supervisor/rusqlite.rs` | 修改 | v10 读写 + 三个 override + `create_redrive_wave` 复制 + list 实现 |
| 6 | `crates/ralph-core/src/supervisor/migrations/v10.sql` | 新增 | `slot_descriptors` 表 |
| 7 | `crates/ralph-core/src/supervisor/migrations.rs` | 修改 | 注册 v10 |
| 8 | `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` | 修改 | spawn persist 接线 + pre-registered 模式 + bridge trait 新方法的 impl/stub 同步 |
| 9 | `crates/ralph-cli/src/loop_runner/runner.rs` | 修改 | boot `dispatch_pending_redrive_waves`（两处对称） |
| 10 | `crates/ralph-cli/src/loop_runner/tests/wave_supervisor.rs` | 修改 | S1-S6 集成测试（含 rusqlite-backed S3） |
| 11 | `crates/ralph-core/src/supervisor/u4_descriptor_tests.rs` | 修改 | parent→child 复制/重映射/enriched list/三态单测 |

计划文件本身不计入实施 diff。

### 风险与系统影响

| 风险 | 触发条件 | 检测 | 缓解 | 剩余 |
|---|---|---|---|---|
| v10 migration 与既有 DB 不兼容 | 旧 supervisor.db 打开 | rusqlite 打开路径既有 migration 测试 + U2 集成 | 纯增量表；migration 失败按既有 fail-closed | 低 |
| persist 失败阻断派发 | store 写错误 | persist 返回 Err → slot 诊断 | fail-closed 标诊断，不静默继续 | 低 |
| enriched list 的 parent digest 缺失 | pre-U4 legacy 行 | `expected_digest = None` 分支 | 按 Unavailable fail-closed（S4） | 低 |
| 合成 wave 的 hat 解析失败 | descriptor.topic 无消费者 hat | `find_by_trigger` None → 诊断 | fail-closed 不派发 | 低 |
| pre-registered 模式误用于未注册 wave | 调用方传错 id | register 跳过路径断言 wave 存在（`fan_in_status` 或 UnknownWave） | fail-closed | 低 |
| 测试 fixture 依赖旧 branch 名 | 硬编码旧名 | targeted nextest 回归 | 同步更新 | 低 |

---

## Verification Contract

### 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 层级 | 风险补充 |
|---|---|---|---|---|
| S1 | 第二波 exec 不撞名 | `wave_supervisor.rs`（真 git worktree） | 集成 | 命名唯一性 |
| S2a | spawn persist（双 store） | `wave_supervisor.rs` + store 单测 | 集成+单元 | 写失败 fault injection |
| S2b | redrive 复制+重映射 | `u4_descriptor_tests.rs`（真 create_redrive_wave 流程） | 单元 | 映射边界（多失败 slot） |
| S3 | resume boot 派发 | `wave_supervisor.rs`（**rusqlite-backed** + in-memory 双变体） | 集成 | dispatch 闭环 |
| S4 | descriptor 缺失 fail-closed | store 单测 + 集成 | 单元+集成 | fault injection |
| S5 | digest 冲突 fail-closed | store 单测 + 集成 | 单元+集成 | fault injection |
| S6 | 非 resume 不扫描 | `wave_supervisor.rs` | 集成 | 状态机 |

### 6. 需求—测试追踪矩阵

| Req | Scenario | 验收测试 | 单元测试 | 集成 | Evidence | Unit |
|---|---|---|---|---|---|---|
| R1/R2 | S1 | 跨 wave 不撞 + 字符合法 | 命名规则 | wave_supervisor | E1, E3 | U1 |
| R3 | S2a | spawn persist | bridge 转发 | wave_supervisor | E8 | U3 |
| R4 | S2a/S4 | 双 store 语义对齐 | rusqlite+memory 表驱动 | — | E6, E7 | U2 |
| R5 | S2b | 复制+重映射 | u4_descriptor_tests 扩展 | — | E9 | U2 |
| R6 | S3-S5 | enriched list 三态内容 | list 单测（空/单/多/legacy） | — | E9 | U2 |
| R7 | S3-S5 | take 三态分支 | 既有 4 测 + 新增 | wave_supervisor | E7, E10 | U4 |
| R8/R9 | S3 | pre-registered 派发 + 合成 wave 字段 | — | wave_supervisor（rusqlite 变体） | E11, E12 | U4 |
| R10 | S3 | rusqlite-backed 变体存在且绿 | — | — | E6 | U4 |
| R11 | S1 | 真 git worktree | — | — | E1 | U1 |
| — | S6 | 非 resume 不扫描 | — | wave_supervisor | E13 | U4 |

---

## Implementation Units

### 7. 严格串行开发单元

```text
U1（命名空间迁移）
  ↓
U2（store descriptor 闭环：v10 + rusqlite + 复制 + enriched list）
  ↓
U3（dispatcher spawn persist 接线）
  ↓
U4（runner boot redrive 派发 + rusqlite-backed 集成）
  ↓
U5（全量回归 + 文档核对）
```

#### U1：命名空间迁移

1. **目标：** branch 命名含 store wave id；同 loop 跨 wave 不撞名。
2. **对应：** R1/R2/R11；S1；KTD1/KTD2；E1/E3。
3. **可观察结果：** 两波 exec 的 slot branch 分别为 `…-w-1-0` / `…-w-2-0`。
4. **基线：** 两处 `{loop_id}-{kind}-{slot_index}`（E1）。
5. **修改位置：** `supervisor_bridge.rs:342`（用直接参数 `wave_id`，非 `ctx.wave_id`）；`worktree_bind.rs:166`；`worktree_bind.rs:276/318/386` 断言同步；`wave_supervisor.rs` 新增 S1。
6. **验收：** S1 真 git worktree 两波 exec，断言第二波 bind 成功且 branch 名不同；命令 `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` + `cargo nextest run -p ralph-core -- worktree`。
7. **Acceptance Red：** S1 当前必失败（撞名 + 断言新命名格式不符）。
8. **单测拆分：** 命名格式（kind × wave_id 矩阵）、唯一性、git 合法性。
9. **TDD 顺序：** 命名断言 Red → 两处 Green → S1 集成 Red → Green → Refactor。
10. **最小实现：** 两个 format! 串 + 断言更新；不动 helper 签名（wave_id 已在参数里）。
11. **集成验证：** S1 真 git worktree。
12. **风险测试：** 命名撞名边界（同一 wave 重复 bind 的幂等由 store 既有语义承接，不加新逻辑）。
13. **回归：** `cargo nextest run -p ralph-cli --bin ralph -- supervisor` + `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` + `cargo nextest run -p ralph-core -- supervisor`。
14. **变更：** 文件清单 #1/#2/#10（仅 S1 部分）。
15. **完成标准：** S1 绿 + 回归绿 + build/clippy/fmt 绿 + 独立提交。
16. **停止条件：** `bind_slot` 调用方传入的 `wave_id` 不是 store wave id（与 `dispatcher.rs:1783` 证据冲突）。
17. **风险：** 测试 fixture 硬编码旧名——回归扫出即同步。

#### U2：store descriptor 闭环（v10 + rusqlite + 复制 + enriched list）

1. **目标：** 双 store 具备完整 descriptor 存取；`create_redrive_wave` 复制并重映射；enriched list 可查。
2. **对应：** R4/R5/R6；S2a(store 侧)/S2b/S4/S5(store 侧)；KTD4'/KTD7/KTD8/KTD10；E5/E6/E7/E9/E10。
3. **可观察结果：** rusqlite 与 memory 的 persist/take/slot_descriptor 语义一致；redrive 后 child key 可读 descriptor（slot_index=parent 下标）；list 返回 enriched 元组。
4. **基线：** rusqlite 零实现（E6）；memory 有 persist/take 无 `slot_descriptor` 读；`create_redrive_wave` 不复制（E9）。
5. **修改位置：** `migrations/v10.sql`（新增）+ `migrations.rs`（注册）；`mod.rs`（trait：`slot_descriptor` 读 + `list_redrive_pending_child_waves` + `RedrivePendingChild` 类型，默认 impl 空/None）；`memory.rs`（读方法、复制逻辑、list 实现）；`rusqlite.rs`（三 override + 复制 + list）；`u4_descriptor_tests.rs`（扩展）。
6. **验收（表驱动 + 流程测试）：** persist/take/slot_descriptor 双 store 对齐矩阵；`create_redrive_wave` 复制（单失败 slot / 多失败 slot 重编号序 / 无 descriptor 的 slot → list 中 `expected_digest=None`）；list 过滤（空 / 非 dispatch 相位排除 / legacy 行）；命令 `cargo nextest run -p ralph-core -- supervisor`。
7. **Acceptance Red：** rusqlite persist 后 `slot_descriptor` 读回（当前：无方法编译失败）；redrive 后 take(child, 0, parent_digest) → Dispatchable（当前：DescriptorUnavailable）。
8. **单测拆分：** v10 迁移幂等（旧 v9 DB 打开升级）；digest 严格相等比较；复制时 `descriptor.slot_index` 保持 parent 下标。
9. **TDD 顺序：** v10+rusqlite persist/read Red→Green → take Red→Green → memory 读方法 Red→Green → 复制 Red→Green → enriched list Red→Green → Refactor。
10. **最小实现：** 一张表 + 三个 override + 一个读方法 + 复制段 + list 查询；不改 `create_redrive_wave` 签名。
11. **集成验证：** rusqlite 真实文件 DB 跑全部新增单测（既有 rusqlite 测试模式）。
12. **风险测试：** migration fault（损坏 DB）；digest 冲突（fault injection）；多 slot 重编号（property 式矩阵）。
13. **回归：** `cargo nextest run -p ralph-core -- supervisor` 全量。
14. **变更：** 文件清单 #3/#4/#5/#6/#7/#11。
15. **完成标准：** 上述全绿 + build/clippy 绿 + 独立提交。
16. **停止条件：** rusqlite `create_redrive_wave` 的 SQL 结构与 memory 语义存在未盘点差异（停并对齐后再写复制段）；`waves` 表实际列与 E12 不符。
17. **风险：** v10 与进行中的其他 migration 冲突——migrations 目录顺序执行，冲突即停。

#### U3：dispatcher spawn persist 接线

1. **目标：** slot 绑定成功、spawn 前，descriptor 落当前 store。
2. **对应：** R3；S2a；KTD6；E8。
3. **可观察结果：** 派发后 store 中 `(store_wave_id, slot)` 存在 descriptor，字段与 dispatch 上下文一致。
4. **基线：** 生产零调用（E8）。
5. **修改位置：** `dispatcher.rs:1783` bind 成功分支（构建 `SlotDescriptor { slot_index: index_u32, topic: event.topic.clone(), payload_json: event.payload.clone(), wave_kind, payload_digest: fingerprint_payload(&event.payload) }`，经新 `SupervisorBridge::persist_slot_descriptor` 转发）；`SupervisorBridge` trait 及全部 impl/stub（编译器驱动：core bridge、cli supervisor_bridge、dispatcher 内联 stub、wave_supervisor stub）；测试。
6. **验收：** `wave_supervisor.rs`：派发后经 `slot_descriptor(store_wave_id, 0)` 断言四字段 + digest；persist 失败注入 → slot 诊断且不 spawn（fail-closed）；命令 `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`。
7. **Acceptance Red：** `SupervisorBridge::persist_slot_descriptor` 不存在（编译 Red）；断言读回 descriptor（语义 Red）。
8. **单测拆分：** descriptor 构建（topic/payload/digest/wave_kind 四字段）；persist 失败分支。
9. **TDD 顺序：** trait+impl 编译 Red→Green → S2a 集成 Red→Green → 失败注入 Red→Green → Refactor。
10. **最小实现：** 一个 trait 转发 + 调用点构建 + 失败处理；不改 spawn 其余流程。
11. **集成验证：** in-memory + rusqlite 双变体（store 由 U2 备好）。
12. **风险测试：** persist 与 record_slot_result 的顺序（persist 先于 worker 完成，崩溃后 descriptor 仍在——这正是设计意图，测试锁定）。
13. **回归：** wave_supervisor + supervisor 全量。
14. **变更：** 文件清单 #8/#10。
15. **完成标准：** S2a 双变体绿 + 回归绿 + 独立提交。
16. **停止条件：** bind 成功分支的实际作用域无法同时拿到 `event` 与 `store_wave_id`（停并重定位接线点，备选：worker request 构建段）。
17. **风险：** review 波（SharedReadonly）bind 返回 None——review slot 也应 persist descriptor（redrive 对 review 波同样适用）；若 review 分支作用域不同，就地在该分支同构接线，不停（两个分支都在同一函数内，证据 `dispatcher.rs:1783-1808`）。

#### U4：runner boot redrive 派发

1. **目标：** resume boot 扫描 enriched list → take 三态 → 合成 wave 派发；rusqlite-backed 闭环可证。
2. **对应：** R6-R10；S3/S4/S5/S6；KTD3/KTD8/KTD9；E4/E11/E12/E13。
3. **可观察结果：** rusqlite-backed：构造崩溃现场后 resume，mock executor spawn 计数 == 1；三态失败分支各有诊断。
4. **基线：** 生产零消费（E4）。
5. **修改位置：** `runner.rs`（两处对称插入 `dispatch_pending_redrive_waves`；S6 仅在带 store 的 resume 路径执行）；`dispatcher.rs`（pre-registered 模式：`store_wave_id` 入参为 `Some` 时跳过 `register_wave_if_absent` 并校验 wave 存在）；`wave_supervisor.rs`（S3 双变体 / S4 / S5 / S6）。
6. **验收：**
   - S3（双变体）：完整链路 persist→fail→redrive→崩溃态→resume→spawn==1，且 record/bind 落在 child store wave id 上。
   - S4：legacy 行（无 descriptor）→ Unavailable 诊断、不 spawn。
   - S5：篡改 child descriptor digest → Conflict 诊断、不 spawn。
   - S6：全新 boot → mock executor spawn==0。
   - 命令 `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor`。
7. **Acceptance Red：** S3 当前必失败（无 boot 步骤，spawn==0）。
8. **单测拆分：** 合成 `DetectedWave` 字段（topic 解析、wave_index/total 赋值、consumer_aggregate_timeout=None）；pre-registered 跳过注册但校验存在；扫描幂等（二次调用 spawn 不重复）。
9. **TDD 顺序：** pre-registered Red→Green → 合成 wave Red→Green → S4 Red→Green → S5 Red→Green → S3 in-memory Red→Green → S3 rusqlite Red→Green → S6 Red→Green → Refactor。
10. **最小实现：** 一个扫描函数（runner 内不外抽）+ pre-registered 分支 + 合成段；不引入新 CLI/新事件。
11. **集成验证：** rusqlite-backed 变体是生产闭环判定的硬证据（in-memory 变体防回归）。
12. **风险测试：** 幂等（state-machine）；三态（fault injection）；boot 与 in-flight wave 冲突（recover 先跑，串行化已由插入点保证）。
13. **回归：** wave_supervisor + supervisor 全量 + `cargo nextest run -p ralph-core -- supervisor`。
14. **变更：** 文件清单 #8/#9/#10。
15. **完成标准：** S3-S6 全绿 + 回归绿 + 独立提交。
16. **停止条件：** 合成 wave 所需 `HatRegistry` 在 runner 插入点作用域不可得（停并重定位插入点，备选：event_loop 启动早期 seam）；pre-registered 与 fan-in 的 `register_wave_if_absent` 第二调用点（`dispatcher.rs:2347`）产生 E12 mismatch（停并统一两处策略）。
17. **风险：** `consumer_aggregate_timeout=None` 改变聚合超时回退——由 `dispatcher.rs:1615-1620` 既有公式承接，测试断言 wave 正常收敛即可。

#### U5：全量回归 + 文档核对

1. **目标：** `./scripts/run-tests.sh` 全绿；`ralph-tools-wave.md:314` 与 CONCEPTS.md redrive 段与实际行为逐句核对（预期无需改动——语义已成文；如有漂移仅改漂移句）。
2. **对应：** 全部 R 的收口；KTD 全表。
3. **验收：** 全量两阶段 + doctest + clippy + fmt；`git diff --name-only` 命中 4.4 白名单。
4. **完成标准：** 全绿 + 无 skipped/`.only`/削弱断言 + 各 Unit 独立提交边界清晰。
5. **停止条件：** 出现与本计划无关的基线红——记录并上报，不顺手修。

---

## Definition of Done

### 8. Unit 串行依赖图

```text
U1 → U2 → U3 → U4 → U5
```

- U2 依赖 U1？否（无代码耦合），但串行执行以隔离回归归因；U1 先行因 redrive 派发也依赖命名迁移后的 worktree 绑定。
- U3 依赖 U2：persist 需要双 store 实现就绪。
- U4 依赖 U2+U3：boot 闭环需要「已 persist + 已复制」的 descriptor 存在。
- U5 依赖全部。

### 9. 执行命令清单

| 时机 | 命令 | 目的 | 放行 |
|---|---|---|---|
| U1 | `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` + `cargo nextest run -p ralph-core -- worktree` | 命名迁移 | 否 |
| U2 | `cargo nextest run -p ralph-core -- supervisor` | store 闭环 | 否 |
| U3/U4 | `cargo nextest run -p ralph-cli --bin ralph -- wave_supervisor` + `cargo nextest run -p ralph-cli --bin ralph -- supervisor` | persist/派发 | 否 |
| 每 Unit | `cargo build --workspace` + `cargo clippy --workspace --all-targets` + `cargo fmt --all -- --check` | 门禁 | 否 |
| U4 | `git diff --name-only` 对 4.4 白名单 | 范围封闭 | 越界即停 |
| U5 | `./scripts/run-tests.sh`（flake 兜底 `RALPH_BASELINE_SERIAL=1` 一次） | 全量 | 否 |

HARD RULE 5：新增 spawn `ralph` 的测试用 `common::ralph_bin()` / `scrub_agent_runtime_env`，污染环境复跑。

### 10. 最终质量门禁

- S1-S6 全绿，R1-R11 全部可追踪到可执行测试；S3 含 rusqlite-backed 变体。
- descriptor 生命周期三段（persist/copy/take）双 store 语义一致；生产路径不再有「默认 impl 恒 Unavailable」缺口。
- 命名规则在代码中只剩两处且都含 `wave_id`。
- 未改 `create_redrive_wave` 签名 / `RedriveTakeOutcome` / preset / worker.rs / CLI 表面。
- fmt/build/clippy/targeted/全量全绿；无 skipped/`.only`/削弱断言。
- `git diff --name-only` 全部命中 4.4 白名单（11 项）。
- 每个 Unit 独立提交；KTD 全部 ≥0.85 且无 BLOCKED。

### 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
|---|---|---|
| 这是实施计划而不是 Roadmap 吗 | 是 | 5 Unit 均绑定符号级入口、Red/Green、命令与完成门 |
| Executor 是否仍需做关键设计决策 | 否 | KTD1-KTD10 锁定；v1.1 遗留的「boot 凑参数」开放项已由 E11-E13 闭合为 KTD9 |
| 所有文件和接口是否有代码库证据 | 是 | E1-E14 全部 file:line / commit 原文 |
| 所有关键决策置信度是否 ≥0.85 | 是 | 最低 KTD5/KTD9=0.85 |
| 是否存在未处理的低置信度假设 | 否 | v1.1 的 KTD4（0.95 虚高）已废止并重估 |
| 每个 Unit 是否只有一个可观察行为 | 是 | 命名 / store 闭环 / persist / boot 派发 / 回归核对 |
| 每个 Unit 是否可以独立验证 | 是 | targeted nextest + 完成门 |
| 每个 Unit 是否有真实 Red | 是 | 编译 Red 与语义 Red 分别注明 |
| 每个 Unit 是否包含回归范围 | 是 | 各 Unit 回归项 |
| 是否存在未来 Unit 依赖 | 否 | 依赖图单向 |
| 是否存在泛化任务描述 | 否 | 全部绑定文件、符号、断言、命令 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 追踪矩阵 |
| 所有关键决策是否有 Evidence | 是 | KTD 表逐行引用 E 编号 |
| 生产闭环是否可验证（rusqlite-backed） | 是 | S3  rusqlite 变体为硬门禁（v1.1 缺失项 M9 已补） |
| 计划是否可以严格串行执行 | 是 | 单链 5 Unit |
