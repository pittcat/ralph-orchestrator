---
title: "fix: implementation-review default wave fan-in 与执行阶段共享同一有效 aggregate deadline"
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
product_contract_source: ce-plan-bootstrap
created: 2026-08-06
revised: 2026-08-06
type: fix
baseline_commit: a9dff24e3562cf42f1496108b369c651db6c35f7
supersedes: 2026-08-06-002（初版，对抗性审查后重写）
---

# fix: default wave fan-in 与执行阶段共享同一有效 aggregate deadline（重写版）

## 0. 计划状态

- **状态：** READY。所有实施关键决策置信度 ≥ 0.85。
- **基线：** `a9dff24e3562cf42f1496108b369c651db6c35f7`（分支 `pittcat-dev`）。本文所有行号锚点均在该基线上逐行核对。
- **重写原因（对抗性审查结论）：** 初版存在一个致命缺陷与两个事实错误，不得直接执行：
  1. **[致命] 初版 helper 公式不完整，修复后漂移仍然残留。** 初版声称执行阶段有效 deadline「约 930s」，并提议 `effective_detected_aggregate_timeout_secs(&DetectedWave) -> u64` 三层 fallback。实测调用链证明：失败 run 的执行路径是 `execute_wave_via_supervisor_with_executor`（bridge 恒存在，见 E3/E4），其有效 deadline 是 `attempt_aware_aggregate_timeout(configured=930s, …, retry_budget=1) = max(930, 2288) = **2288s**`（E5 公式推导）。初版 helper 只返回 930s，fan-in 与执行阶段仍差 1358s——任何在 930s~2288s 之间合法完成的 wave 仍会被误杀。helper 必须引入 bridge 参数（`max_concurrent_workers` / `slot_retry_budget`）并复现 attempt-aware floor。
  2. **[事实错误] 初版 U3 指定的测试文件 `crates/ralph-cli/src/loop_runner/wave/dispatcher/tests/misc.rs` 不存在。** 该 crate 没有 `dispatcher/` 目录；所有相关测试位于 `dispatcher.rs` 文件内的 `#[cfg(test)]` 内联模块（`make_wave` @6938、`terminal_context_preserves_elapsed_timeout_relation` @8334）。
  3. **[事实错误] 初版行号锚点漂移**（如称既有测试在「约 1576 行」，实际 8334），且 U1 出现「编译期保证运行时数值相等」这类无效表述。
- **调查范围：** `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`（deadline 计算、fan-in 调用点、测试模块）、`crates/ralph-core/src/supervisor/phase.rs`、`crates/ralph-core/src/config/loop_config.rs`、`crates/ralph-core/src/wave_detection.rs`、`crates/ralph-cli/src/loop_runner/wave/supervisor_bridge.rs`、`presets/en/implementation-review.yml`、诊断报告 `docs/report/2026-08-06-implementation-review-primary-20260806-090515-diagnosis.md`。
- **已执行的验证：** 全部证据锚点经 `sed`/`rg` 逐行核对；deadline 公式经手工推导（E5）；`SupervisorConfig` 全仓读取点唯一性经多行模式全仓扫描确认（E7）。未运行测试/构建（计划阶段）。
- **尚未执行的验证：** 所有 Red/Green、编译、nextest、最终基线留给执行阶段（第 9 节命令清单）。
- **阻塞项：** 无。

## 1. 功能目标

- **业务目标：** default-wave（`ralph wave emit` 触发的 wave，含 `implementation-review` 的 review wave）全部 slot 成功完成后，fan-in 不得因使用与执行阶段不同的 aggregate deadline 而误判 timeout，阻断下游 synthesizer/fix-planner。
- **用户或调用方：** A1 loop runner（`handle_wave_events` → fan-in）；A2 preset operator（依赖 `review.wave.complete` 触发后续 hat）；A3 诊断消费者（不再看到 `wave_failed:timeout` + `missing_dimensions: []` 的矛盾终态）。
- **当前行为（基线事实）：**
  - 执行阶段：`execute_wave_structured`（@1303）在 bridge 存在时（default-wave 恒存在，E3/E4）委派 `execute_wave_via_supervisor_with_executor`（@1345-1347 → @1696），其 deadline = `attempt_aware_aggregate_timeout(configured, wave_timeout, events.len(), effective_cap, bridge.slot_retry_budget())`（@1762-1768）；失败 run 中该值 = 2288s（E5）。
  - fan-in 阶段：同一 `handle_wave_events` 内（@920-924）读取 `event_loop.config().event_loop.supervisor.aggregate_timeout_secs`（默认 600，`loop_config.rs:1287`），传入 `run_supervisor_fan_in`（@2546）→ `PhaseInputs.aggregate_timeout_secs`（@2636-2645）。
  - `evaluate_phase`（`phase.rs`，timeout 判断 @133）先判 `elapsed_secs > aggregate_timeout_secs`，后判全部完成。失败 run：elapsed 726s > 600s → `Failed(Timeout)` → `review.wave.failed`。
- **目标行为：** fan-in 调用点与执行阶段通过同一 helper 取同一有效 deadline（失败 run 形状下 = 2288s）；elapsed 726s、6 slot 全完成 → `Integrate` → `review.wave.complete`；elapsed 超过该 deadline 时 timeout 语义保留。
- **行为差异：** 仅 fan-in 传入 `PhaseInputs.aggregate_timeout_secs` 的数值来源变化（600 → wave-derived 有效值）；`phase.rs` 判断顺序、`SupervisorConfig` 默认值、legacy 非 supervisor 路径均不变。
- **本次范围：** dispatcher.rs 内抽 helper、替换两处调用（fan-in @920-924、supervisor 执行内联式 @1742-1768）、新增回归测试、更新 `PhaseInputs` 文档注释。
- **非目标：** 不改 `phase.rs` 判断顺序；不改 `SupervisorConfig.aggregate_timeout_secs` 默认 600（仍服务其它消费方）；不在 preset 里临时加 `aggregate_timeout_secs: 930`（绕过，不治根）；不修 `w-rs-1`/`w-2` orphan 告警与 summary/handoff 误写成功（诊断报告 P1/P2，独立跟进）；不动 legacy 非 supervisor deadline 路径（@1388-1393，无 bridge、公式不同、default-wave 不可达）。
- **输入：** `DetectedWave`（`hat_config.timeout`/`aggregate`/`concurrency`、`consumer_aggregate_timeout`、`events.len()`）+ bridge 运行时参数（`max_concurrent_workers()`、`slot_retry_budget()`）。
- **输出：** 单一 `u64` deadline，供执行与 fan-in 共用。
- **状态变化：** 无新增持久化；supervisor store/ledger 写入路径不变。
- **错误语义：** timeout 路径保持 fail-close：elapsed 严格大于有效 deadline 时仍 `Failed(Timeout)`；本修复不放宽任何失败语义。
- **兼容性要求：** `run_supervisor_fan_in` 公开签名不变；既有 fan-in 测试（`tests/wave_supervisor.rs` 中大量以 600 直接调用的用例）不受影响。
- **性能要求：** 无新增 I/O；helper 为纯算术。
- **已知约束：** HARD RULE 1/2（nextest 入口）；行号锚点随基线固定，执行中若漂移按停止条件处理。
- **已确认假设：** 无（初版的「930s」假设已被证伪并修正为 2288s，见 E5）。
- **待验证假设：** 无低于阈值事项。

## 2. 代码库现状与证据

### 2.1 当前实现入口

```
handle_wave_events (@489)
  ├─ 懒构造 bridge (@619-677)：supervisor.enabled=false 也构造
  │    cap = u32::MAX, slot_retry_budget = 1（@627, @669-674）
  ├─ execute_wave_structured(@1303) ── bridge Some ──▶ execute_wave_via_supervisor_with_executor(@1696)
  │      configured = aggregate_timeout_for(900s,6,6) = 930s   (@1742-1747)
  │      effective_cap = min(hat.concurrency, bridge.max_concurrent_workers()).max(1)  (@1749-1754)
  │      aggregate_timeout = attempt_aware_aggregate_timeout(930s, 900s, 6, 6, 1) = 2288s  (@1762-1768)
  │      → 执行阶段实际预算 2288s（失败 run 形状）
  └─ wave 完成后 fan-in（@919-941）：
       aggregate_timeout_secs = config.event_loop.supervisor.aggregate_timeout_secs  (@920-924) ← BUG：600s
       run_supervisor_fan_in(bridge, completed, detected, events_file, 600, terminal_ctx{elapsed: 726s})
         → PhaseInputs{aggregate_timeout_secs: 600, elapsed_secs: 726}  (@2636-2645)
         → bridge.tick_with_slot_events → coordinator evaluate_phase
            elapsed(726) > 600 → Failed(Timeout)  (phase.rs @133，先于 complete 判断)
         → review.wave.failed → LOOP_COMPLETE(blocked)
```

### 2.2 Evidence Ledger

| Evidence ID | 来源 | 观察结果 | 对计划的影响 | 可靠性 |
| --- | --- | --- | --- | --- |
| E1 | `.ralph/diagnostics/logs/ralph-2026-08-06T17-05-15-607-10178.log:24,31` + `.ralph/review/2026-08-05-001-…/wave-blocked.md:1` | 6 reviewer 全成功、duration 726.156s；fan-in 返回 `InjectedFailed`；终态 `wave_failed:timeout`、`missing_dimensions: []` | 失败模式成立；回归测试必须固定「726s 全完成不得 timeout」 | 高 |
| E2 | `dispatcher.rs:920-924` | fan-in 调用点读取 `…supervisor.aggregate_timeout_secs` | 唯一需要改的取值点 | 高 |
| E3 | `dispatcher.rs:602-679`（注释 + `lazy_bridge` 构造） | default-wave 路径在 `supervisor.enabled=false` 时也懒构造 bridge：`cap=u32::MAX`（@627）、`slot_retry_budget=1`（@669-674 注释「pin at historical default (1)」） | 失败 run 走 supervisor 执行路径；bridge 参数可预先确定 | 高 |
| E4 | `dispatcher.rs:1345-1347` | `execute_wave_structured` 在 bridge Some 时直接委派 `execute_wave_via_supervisor_with_executor` | 失败 run 的执行阶段 deadline 由 @1742-1768 决定，而非 @1388-1393 | 高 |
| E5 | `dispatcher.rs:4767`(SLACK=30)、`4762-4763`(NUM=8/DEN=10)、`4780-4797`(wave_work_budget)、`4800-4806`(aggregate_timeout_for)、`4809-4842`(aggregate_floor_for_attempts)、`4844-4856`(attempt_aware_aggregate_timeout) | 失败 run 形状：configured=900×ceil(6/6)×1+30=930；floor=ceil((900×1×2+30)×10/8)=ceil(18300/8)=2288；有效值 max(930,2288)=**2288** | **初版「930s」被证伪**；helper 必须复现 attempt-aware floor 才算「同一有效 deadline」 | 高 |
| E6 | `phase.rs:113`(evaluate_phase 为 pub 纯函数)、`@133`(`elapsed_secs > aggregate_timeout_secs` 严格大于、先于完成判断)、`@140-146`(pending=0 且 completed≥expected → Integrate) | timeout 优先于 complete；边界 `==` 不超时；`IncompleteEvidence` 检查在 coordinator（coordinator.rs:185）不在 evaluate_phase | 边界测试可直接对 evaluate_phase 写，snapshot 无需 terminal evidence | 高 |
| E7 | 全仓多行 rg 扫描 `supervisor\.aggregate_timeout_secs`（crates/ralph-cli） | 仅 `dispatcher.rs:923-924` 一处读取；redrive 路径无独立 fan-in aggregate 读取 | 修复只需覆盖一个调用点，无遗漏面 | 高 |
| E8 | `loop_config.rs:1287-1289`(默认 600)、`1296-1306`(SupervisorConfig::Default) | 600 是字段默认值，服务其它 supervisor 消费方 | 不改默认值（非目标） | 高 |
| E9 | `wave_detection.rs:25-95` | `DetectedWave` 提供 `hat_config`(`concurrency: u32`，config/hat.rs:538)、`consumer_aggregate_timeout`、`per_worker_timeout_secs()`(默认 300)、`aggregate_timeout_secs()`(hat.aggregate→consumer→per_worker)、`has_explicit_aggregate_timeout()` | helper 输入面完备 | 高 |
| E10 | `dispatcher.rs:2478-2495`(SupervisorFanInOutcome: InjectedComplete/InjectedFailed/…)、`2546-2553`(run_supervisor_fan_in 签名)、`2636-2645`(PhaseInputs 构造，elapsed 取 `terminal_ctx.elapsed.as_secs()`) | fan-in 语义与签名现状 | 签名不变；elapsed 透传已正确（既有 U1 Red 3 修复） | 高 |
| E11 | `dispatcher.rs:8334`(terminal_context_preserves_elapsed_timeout_relation)、`8339`(CapturingBridge)、`6938`(make_wave)、`~8590`(runner_terminates_on_terminal_fan_in_failure 使用 `include_str!` 源码守卫先例) | 测试全部位于 dispatcher.rs 内联 `#[cfg(test)]` 模块；**不存在** `dispatcher/tests/misc.rs`；源码守卫断言在本模块有既判先例 | 初版测试路径证伪；新测试落点与红测试手段确定 | 高 |
| E12 | `presets/en/implementation-review.yml:60-61`(~930s 注释)、`102`(max_concurrent_workers: 6)、`1277`(concurrency: 6)、`1281`(timeout: 900) | preset 与失败 run 参数一致；注释中的 930s 是 pre-floor 值 | preset 无需改动；注释偏差由文档跟进（非目标） | 高 |
| E13 | `flow_lifecycle.rs:732`(effective_wave_deadlines) | core 已有 deadline shim，但只包 `DetectedWave::aggregate_timeout_secs()`，不含 batch 公式与 floor | 只作风格先例，不复用（语义不足） | 高 |
| E14 | `phase.rs:17-19`(PhaseInputs.aggregate_timeout_secs 文档注释「supplied by SupervisorConfig::aggregate_timeout_secs」) | 修复后该注释失真 | U2 需同步注释（防 doc drift） | 高 |

### 2.3 受影响范围

- **生产模块：** `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`（helper 新增、@920-924 与 @1742-1768 替换）。
- **文档注释：** `crates/ralph-core/src/supervisor/phase.rs` `PhaseInputs.aggregate_timeout_secs` 注释（来源描述改为 wave-derived）。
- **测试模块：** `dispatcher.rs` 内联测试模块（新增，邻近 @8334）。
- **不受影响：** `run_supervisor_fan_in` 签名；`tests/wave_supervisor.rs` 既有以 600 直调的用例（它们测 fan-in 本体，不测调用点取值）；legacy 非 supervisor deadline 路径 @1388-1393；`presets/`、`manifest.yml`、`index.json`、zsh 补全、skill 文档（无 CLI 参数/事件/preset 变更）。

## 3. 决策记录与置信度

| Decision ID | 决策问题 | 候选方案 | 最终选择 | 支持证据 | 排除其他方案的原因 | 置信度 |
| --- | --- | --- | --- | --- | --- | --- |
| D1 | helper 放哪、签名如何？ | (a) dispatcher 内 `fn effective_detected_aggregate_deadline_secs(wave: &DetectedWave, bridge: &dyn SupervisorBridge) -> u64`；(b) 提升到 ralph-core；(c) 初版无 bridge 参数签名 | (a)。复用 `aggregate_timeout_for`/`attempt_aware_aggregate_timeout`（均为 dispatcher 私有函数），bridge 参数提供 `max_concurrent_workers()`/`slot_retry_budget()` | E3-E5、E9、E11 | (b) 需要把三个私有公式与常量搬进 core，扩面且无第二消费方；(c) 已被 E5 证伪（返回 930≠执行 2288，漂移残留） | 0.92 |
| D2 | 替换哪几处？ | 仅 fan-in；fan-in + supervisor 执行内联式；含 legacy 路径 | fan-in @920-924 + supervisor 执行 @1742-1768（两段合成一次 helper 调用）；legacy @1388-1393 不动 | E2、E4、E7 | legacy 路径无 bridge、无 floor、default-wave 不可达，动它只会扩大回归面 | 0.95 |
| D3 | 如何让「调用点走 helper」可测试？ | 端到端驱动 handle_wave_events；对 run_supervisor_fan_in 传参断言；源码守卫 + helper 值测试 + evaluate_phase 边界测试 | 第三种组合：①源码守卫（`include_str!` 断言 fan-in 区域含 helper 调用、不含 supervisor 配置读取）作 Acceptance Red；②helper 纯函数值测试；③evaluate_phase 边界测试；④run_supervisor_fan_in + 脚本 bridge 的 InjectedComplete 集成断言 | E6、E11（include_str 先例）、无既有 handle_wave_events 端到端测试 | 全仓无 handle_wave_events 测试 harness（已核查），现造一个超出本修复范围；②③④覆盖数值与决策语义，①绑定接线；残余风险已记录（见 R-1） | 0.88 |
| D4 | 是否改 phase.rs 判断顺序 / SupervisorConfig 默认？ | 改顺序让 complete 优先；默认改 930/2288 | 都不改 | E6、E8、诊断报告 6.3 | 改顺序篡改正常 timeout fast-fail 语义；改默认影响所有 supervisor wave 契约 | 0.97 |
| D5 | run_supervisor_fan_in 签名是否改（如直接收 DetectedWave 自己算）？ | 改签名内聚计算；保持 u64 参数 | 保持 `aggregate_timeout_secs: u64` 参数不变 | E10、E11（大量既有测试以 600 直调） | 改签名破坏全部既有 fan-in 测试且让 fan-in 承担 deadline 决策（决策权在调用点） | 0.96 |

无低于 0.85 的决策。D3 接近阈值：其残余风险（源码守卫不证明运行时数值真的流入 evaluate_phase）由 ②③④ 的运行时测试互补覆盖，且守卫先例为本模块既判模式。

## 4. BDD 行为规格

```gherkin
Feature: default-wave fan-in 与执行阶段共享同一有效 aggregate deadline

  Background:
    Given default-wave 路径（supervisor.enabled=false，懒 bridge cap=u32::MAX，slot_retry_budget=1）
    And wave 形状：6 events、hat.concurrency=6、hat.timeout=900、无显式 aggregate

  Scenario: S1 fan-in 使用 wave-derived 有效 deadline 而不是 SupervisorConfig 默认
    Given fan-in 调用点解析 aggregate deadline
    When 读取取值来源
    Then 来源是 effective_detected_aggregate_deadline_secs(wave, bridge)
    And 该值等于执行阶段 attempt_aware_aggregate_timeout 的结果（本形状 2288）
    And 不再读取 event_loop.supervisor.aggregate_timeout_secs

  Scenario: S2 失败 run 形状在 726s 全完成时不得 timeout
    Given 6 slot 全部 Completed、elapsed=726s、deadline=helper 值（2288）
    When evaluate_phase 评估
    Then 返回 Integrate
    And fan-in 经脚本 bridge（Integrate）产出 InjectedComplete

  Scenario: S3 边界：elapsed 等于 deadline 不算超时
    Given 全部 slot Completed、elapsed=deadline
    When evaluate_phase 评估
    Then 返回 Integrate（严格大于才超时）

  Scenario: S4 超时语义保留：elapsed 超过 deadline 仍失败
    Given 全部 slot Completed、elapsed=deadline+1
    When evaluate_phase 评估
    Then 返回 Failed(Timeout)

  Scenario: S5 显式 aggregate 配置优先
    Given hat.aggregate.timeout=300
    When helper 计算
    Then configured=300，最终值=max(300, floor)（floor 按 events/concurrency/budget 计算）
    And consumer_aggregate_timeout=Some(450) 且无 hat.aggregate 时 configured=450

  Scenario: S6 legacy 非 supervisor 路径不变
    Given execute_wave_structured 以 bridge=None 运行（仅测试直调路径）
    When deadline 计算
    Then 仍为 @1388-1393 原公式（无 attempt-aware floor）

  Scenario: S7 显式配置不得被 floor 反向压低
    Given 显式 aggregate 大于 floor
    When helper 计算
    Then 返回显式值（max 语义保持）
```

## 5. 验收与测试策略

| Scenario | 验收条件 | 测试入口 | 推荐测试层级 | 风险补充测试 | 是否需要 E2E |
| --- | --- | --- | --- | --- | --- |
| S1 | fan-in 区域源码含 helper 调用、不含 supervisor 配置读取 | `dispatcher.rs` 内联测试（include_str 守卫，先例 @~8590） | 单元（接线守卫） | — | 否 |
| S2 | PhaseInputs 数值正确 + Integrate + InjectedComplete | helper 值测试 + evaluate_phase 测试 + run_supervisor_fan_in 脚本 bridge 测试 | 单元 + 集成 | 数值表驱动（S5 各形状） | 否 |
| S3/S4 | 边界 `==` 不超时、`+1` 超时 | evaluate_phase 纯函数测试 | 单元 | — | 否 |
| S5 | 显式/consumer 配置优先 + floor 不压低显式大值 | helper 值测试 | 单元 | 表驱动 | 否 |
| S6 | legacy 公式不变 | 既有 dispatcher deadline 测试回归 | 回归 | — | 否 |
| S7 | max 语义 | helper 值测试 | 单元 | — | 否 |

所有测试位于 `dispatcher.rs` 内联 `#[cfg(test)]` 模块（E11），nextest 进程隔离天然满足。不 mock `evaluate_phase`/`aggregate_timeout_for` 等真实决策与公式函数。

## 6. 需求—测试追踪矩阵

| Requirement ID | 需求 | Scenario | 验收测试 | 单元测试 | 集成/契约测试 | E2E | Evidence |
| --- | --- | --- | --- | --- | --- | --- | --- |
| R1 | fan-in 与执行阶段共享同一有效 deadline（含 attempt-aware floor） | S1、S2 | U1 守卫测试 | helper 值测试 | run_supervisor_fan_in InjectedComplete | 否 | E2、E5、E7 |
| R2 | deadline 计算收敛到单一 helper，两处调用点共用 | S1、S5、S7 | U1 守卫测试 | helper 表驱动测试 | — | 否 | E5、E13 |
| R3 | 6-slot/726s 失败模式回归固定，timeout 语义不丢失 | S2、S3、S4 | evaluate_phase 边界测试 | helper 数值 | 脚本 bridge 集成 | 否 | E1、E6 |
| R4 | 门禁与既有测试无回归 | S6 | 既有 deadline/fan-in 测试 | — | tests/wave_supervisor.rs 子集 | 否 | E10、E11 |

## 7. 严格串行开发单元

```
Unit 1 → Unit 2 → Unit 3 → Unit 4（门禁）
```

### Unit 1：接线守卫与失败模式固定（Acceptance Red）

1. **Unit 目标：** 以可执行测试声明目标行为：fan-in 调用点必须走统一 helper；当前实现不满足 → 真实 Red。本 Unit 不写生产代码（除测试）。
2. **对应需求与 Scenario：** R1/R3；S1、S2；D3；E1、E2、E5、E11。
3. **外部可观察结果：** 新增测试在基线上失败（Red），失败原因可复述。
4. **当前行为基线：** @920-924 读取 `SupervisorConfig.aggregate_timeout_secs`（E2）；无 helper（E5 推导 2288 vs fan-in 600）。
5. **输入与输出：** 测试输入 = 失败 run 形状常量（6 events、timeout 900、concurrency 6、cap u32::MAX、budget 1）；输出 = 断言失败信息。
6. **修改位置：** `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` 内联测试模块（`terminal_context_preserves_elapsed_timeout_relation` @8334 附近新增）。只加测试，不动生产代码。
7. **可依赖能力：** `include_str!` 守卫先例（@~8590）；`make_wave`（@6938）；`CapturingBridge`（@8339）；`evaluate_phase`（phase.rs pub）；`run_supervisor_fan_in`（E10）。
8. **禁止依赖的未来能力：** 不得引用尚不存在的 helper 符号（引用会使测试编译失败——无效 Red）。守卫测试用字符串断言 helper 名称，不 use 符号。
9. **验收测试：**
   - `fan_in_deadline_uses_wave_derived_helper`（源码守卫）：`let src = include_str!("dispatcher.rs");` 定位 fan-in 区域（`run_supervisor_fan_in(` 调用点上方 30 行窗口），断言窗口内含 `effective_detected_aggregate_deadline_secs(`，且不含 `.aggregate_timeout_secs;` 的 supervisor 配置读取模式。
   - 前置：无；动作：`cargo nextest run`；断言：窗口包含 helper 调用。
10. **Acceptance Red：** 运行守卫测试。预期失败：窗口内找到的是 `event_loop.config().event_loop.supervisor.aggregate_timeout_secs` 读取而非 helper 调用，断言消息明确。无效 Red 排除：编译错误、include_str 路径错误、窗口定位错误（定位用稳定锚串 `run_supervisor_fan_in(`，先单独断言锚串存在）。
11. **单元测试拆分：** 本 Unit 只有守卫测试一个行为；helper 数值/边界测试属于 U2/U3（helper 尚不存在）。
12. **Red → Green → Refactor 顺序：** 守卫测试 Red（记录实际失败输出）→ 提交 Red 证据 → 进入 U2 实现 → Green。
13. **最小实现范围：** 仅新增测试函数；不改任何生产行为。
14. **集成验证：** 不涉及。
15. **风险驱动测试：** 源码守卫在此是接线契约测试（本模块既判模式，E11）；风险依据：fan-in 调用点深嵌 async 大函数，无既有端到端 harness（已核查），守卫是成本最低的接线绑定。
16. **回归范围：** `cargo nextest run -p ralph-cli --bin ralph -- fan_in`（确认新测试被选中、其余不回归）。
17. **预期文件变更：** `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`（新增测试函数）| 新增测试 | 固定 S1 | E2、E11。
18. **完成标准：** 守卫测试编译通过且以预期原因失败；失败输出已记录；无生产代码变更；可独立提交（测试先行提交允许标 `// RED: 见计划 U1`，但仓库若 CI 拦截失败测试，则与 U2 合并为一次提交——由执行时 CI 行为决定，两种方式均不违反串行）。
19. **停止条件：** include_str 窗口定位不稳定（重构敏感）→ 改为对整文件断言「`.event_loop\n.supervisor\n.aggregate_timeout_secs` 多行模式在 fan-in 函数体内不出现」；若仍不稳定，停止并回到 D3 重评。
20. **风险与注意事项：** 守卫测试对源码格式敏感；断言模式必须宽松于空白、严格于语义（读法：先 compress 空白再匹配）。

### Unit 2：helper 实现与两处调用点切换（Green）

1. **Unit 目标：** 新增 `effective_detected_aggregate_deadline_secs(wave: &ralph_core::DetectedWave, bridge: &dyn ralph_core::supervisor::SupervisorBridge) -> u64`，在 fan-in 与 supervisor 执行路径共用；U1 守卫转 Green；执行语义逐位不变。
2. **对应需求与 Scenario：** R1、R2；S1、S5、S7；D1、D2、D5；E3-E5、E9。
3. **外部可观察结果：** U1 守卫测试通过；新增 helper 值测试通过；既有执行路径测试无回归。
4. **当前行为基线：** @1742-1768 内联式（E4/E5）；@920-924 读 600（E2）。
5. **输入与输出：** helper 输入 = wave + bridge；输出 = `u64` 秒。语义 = `attempt_aware_aggregate_timeout(configured, per_worker, events.len(), effective_cap, bridge.slot_retry_budget()).as_secs()`，其中 `configured` = 显式/consumer ? `wave.aggregate_timeout_secs()` : `aggregate_timeout_for(per_worker, events.len(), wave.hat_config.concurrency as usize)`，`effective_cap = wave.hat_config.concurrency.min(bridge.max_concurrent_workers()).max(1)`。**必须逐字镜像 @1742-1768 现有语义**（含 `.max(1)`）。
6. **修改位置：**
   - `dispatcher.rs`：新增私有 fn（放在 `attempt_aware_aggregate_timeout` @4844 附近）；替换 @920-924（改为 `let aggregate_timeout_secs = effective_detected_aggregate_deadline_secs(&detected, bridge.as_ref());`）；替换 @1742-1768 三段为一个 helper 调用（`let aggregate_timeout = Duration::from_secs(effective_detected_aggregate_deadline_secs(wave, bridge.as_ref()));`）。
   - `phase.rs:17-19`：`PhaseInputs.aggregate_timeout_secs` 注释改为「wave-derived 有效 deadline（default-wave）或 SupervisorConfig（supervisor 显式配置路径）供给」——只改注释，不改类型/默认。
   - **明确不改：** @1388-1393 legacy 路径；`run_supervisor_fan_in` 签名；`SupervisorConfig`。
7. **可依赖能力：** `aggregate_timeout_for`/`attempt_aware_aggregate_timeout`/`aggregate_timeout_secs()` 等（E5、E9）；U1 守卫测试。
8. **禁止依赖的未来能力：** 不实现 U3 的边界测试集；不顺手重命名既有公式函数。
9. **验收测试（helper 值测试，表驱动）：**
   - `helper_failed_run_shape_is_2288`：make_wave 形状（6 events、timeout Some(900)、concurrency 6）+ stub bridge（max_concurrent_workers=u32::MAX、slot_retry_budget=1）→ 2288。
   - `helper_explicit_hat_aggregate_wins_over_formula`：hat.aggregate.timeout=300 → max(300, floor(…))；另造显式 5000 > floor → 5000（S7）。
   - `helper_consumer_aggregate_used_when_no_hat_aggregate`：consumer=Some(450) → max(450, floor)。
   - `helper_zero_retry_budget_still_applies_floor`：budget=0 → floor 按 attempts=1 计算（=ceil(930×10/8)=1163），结果 1163。
   - `helper_matches_inline_expression`：对同一组随机化表（events 0/1/6/13、concurrency 0/1/6、timeout 300/900），断言 helper 输出 == 原内联表达式逐步计算值（本测试是语义保持的差分守卫，重构前后同值）。
   - stub bridge：复用/仿照 CapturingBridge 的 trait 实现，只填 `max_concurrent_workers` 与 `slot_retry_budget`，其余方法 panic-unreachable。
   - 运行命令：`cargo nextest run -p ralph-cli --bin ralph -- effective_detected_aggregate_deadline`。
10. **Acceptance Red：** 本 Unit 的 Red 由 U1 守卫承担（helper 不存在 → 守卫失败已记录）。helper 值测试随 helper 引入，不产生「编译错误式 Red」——这是 extract-method 重构的标准处理：行为 Red 在 U1，结构绿在 U2。若执行者坚持为 helper 值测试造 Red，正确做法是先写测试、确认编译失败**不算**有效 Red、再实现；禁止把编译失败记录为 Red。
11. **单元测试拆分：** 见第 9 节五个值测试；每个测试对象 = helper 纯函数；不允许 mock `aggregate_timeout_for`/`attempt_aware_aggregate_timeout`（它们是被复用的真实公式）。
12. **Red → Green → Refactor 顺序：** U1 守卫（已 Red）→ 实现 helper + 值测试 → 值测试 Green → 替换 fan-in 调用点 → 守卫 Green → 替换 supervisor 执行内联式 → `helper_matches_inline_expression` Green → Refactor（仅命名/注释）→ 全量相关测试 Green。
13. **最小实现范围：** helper + 两处替换 + phase.rs 注释；错误处理：无新错误路径（纯算术，saturating 语义沿用既有函数）；不变量：`execute_wave_via_supervisor_with_executor` 的 `aggregate_timeout` 数值逐位不变（由差分测试证明）。
14. **集成验证：** `cargo nextest run -p ralph-cli --bin ralph -- aggregate` 与 `-- supervisor`（既有 deadline/fan-in 相关测试子集）必须无回归；`cargo nextest run -p ralph-core -- phase` 确认 core 无回归。
15. **风险驱动测试：** Differential（`helper_matches_inline_expression`：新旧同输入同输出）——风险依据：替换的是生产执行路径的 deadline，任何语义漂移都会改变 timeout 行为。
16. **回归范围：** dispatcher 全部内联测试、`tests/wave_supervisor.rs` 子集（`cargo nextest run -p ralph-cli --test loop_runner -- wave_supervisor` 或等价 nextest 过滤；以 crate 实际测试目标为准）、`ralph-core` phase 测试。原因：两处调用点分别影响执行与 fan-in。
17. **预期文件变更：** `dispatcher.rs`（新增 helper、替换两处）| 修改生产 | S1/S5/S7 | E2-E5；`phase.rs`（注释）| 修改文档注释 | E14。
18. **完成标准：** U1 守卫 Green；五个值测试 Green；差分测试 Green；既有测试零回归；`cargo fmt --check`/`cargo clippy` 通过；可独立提交。
19. **停止条件：** 差分测试发现 helper 与内联式不同值（说明 E5 推导或镜像有误）→ 停止，重对 @1742-1768；发现第二个 supervisor 配置读取点（与 E7 冲突）→ 停止更新影响分析；clippy 对 helper 提出结构性反对且修复会改变语义 → 停止重评 D1。
20. **风险与注意事项：** `effective_cap` 的 `.max(1)` 与 `bridge.max_concurrent_workers()` 返回 u32::MAX 时的 min 语义必须逐字保留；`Duration` 与 `u64` 秒的转换边界（helper 返回 u64 秒，执行侧需要 Duration——在调用侧 `Duration::from_secs(...)`）。

### Unit 3：失败模式回归与边界固定

1. **Unit 目标：** 以运行时测试固定「6 slot / 726s / 全完成 → Integrate → InjectedComplete」与「deadline+1 → Failed(Timeout)」，覆盖 S2/S3/S4。
2. **对应需求与 Scenario：** R3；S2、S3、S4；D3；E1、E6、E10。
3. **外部可观察结果：** 回归测试在 U2 后通过；若有人把 fan-in 传值改回 600，守卫（U1）失败；若有人改公式，值测试失败；若有人改 phase 边界，本 Unit 失败。
4. **当前行为基线：** evaluate_phase 严格大于判断（E6）；run_supervisor_fan_in Integrate → InjectedComplete（E10）。
5. **输入与输出：** 构造 `WaveSnapshot`（expected_total=6、completed_count=6、pending=0、in_flight=0、无失败 slot；**无需 terminal evidence**——IncompleteEvidence 在 coordinator 层，E6）+ `PhaseInputs{aggregate_timeout_secs: helper 值 2288, elapsed_secs: 726|2288|2289, cancel_requested: false}`。
6. **修改位置：** `dispatcher.rs` 内联测试模块新增测试（邻近 @8334）；不改生产代码。
7. **可依赖能力：** U2 helper（用于产生 2288）；`evaluate_phase`（pub）；`run_supervisor_fan_in` + 脚本 bridge（wave_supervisor.rs @4514+ 有可参照的 bridge 脚本模式）。
8. **禁止依赖的未来能力：** 不驱动 handle_wave_events；不新建端到端 harness。
9. **验收测试：**
   - `regression_six_slots_726s_integrates_under_wave_deadline`：evaluate_phase(snapshot_6_completed, PhaseInputs{2288, 726}) == Integrate。
   - `regression_elapsed_equals_deadline_not_timeout`：elapsed=2288 → Integrate。
   - `regression_elapsed_past_deadline_still_times_out`：elapsed=2289 → Failed{reason: Timeout}。
   - `fan_in_injects_complete_with_wave_deadline`：脚本 bridge（tick_with_slot_events 返回 Integrate）+ run_supervisor_fan_in(…, aggregate=2288, terminal_ctx{elapsed:726s}) → SupervisorFanInOutcome::InjectedComplete，且主 ledger 写入 `review.wave.complete`（沿用 wave_supervisor.rs 既有断言模式）。
10. **Acceptance Red：** 本 Unit 测试在 U2 完成后应直接 Green（它们固定的是 U2 已实现的行为 + 既有 phase 语义）。真正的 Red 已发生在 U1。执行时先运行确认全绿；若任何一项 Red，说明 U2 实现有误，回 U2 修复，不得在本 Unit 现场改生产代码绕过。
11. **单元测试拆分：** 见第 9 节四项；snapshot 构造可抽局部 helper（仅测试内）。
12. **Red → Green → Refactor 顺序：** 编写四项测试 → 全绿验证（U2 行为证明）→ 若红回 U2 → Refactor 测试 helper → 复跑全绿。
13. **最小实现范围：** 仅测试代码。
14. **集成验证：** `cargo nextest run -p ralph-cli --bin ralph -- regression_six_slots`、`-- fan_in_injects_complete`。
15. **风险驱动测试：** State-Machine 边界（==/＋1/远小于）——风险依据：timeout 判断是严格不等式，历史修复（U1 Red 3 注释 @8328-8333）证明 elapsed 透传曾出过 bug，边界必须钉死。
16. **回归范围：** U2 全部测试 + `terminal_context_preserves_elapsed_timeout_relation`（@8334，确认不受影响）。
17. **预期文件变更：** `dispatcher.rs`（新增 4 个测试）| 新增测试 | S2-S4 | E1、E6。
18. **完成标准：** 四项测试 Green；既有 terminal_context 测试 Green；无生产代码变更；可独立提交。
19. **停止条件：** snapshot 构造遇到 WaveSnapshot 字段不可在测试中合法置位（如 store 绑定字段）→ 停止，评估改用 coordinator 层测试或降级为 PhaseInputs-only 断言，不得伪造字段。
20. **风险与注意事项：** `fan_in_injects_complete_with_wave_deadline` 中 bridge 需满足 register_wave_if_absent/record_* 等最小契约，参照 wave_supervisor.rs 既有脚本 bridge 逐方法实现。

### Unit 4：最终门禁（无生产变更）

1. **Unit 目标：** 全量门禁验证。
2. **对应需求与 Scenario：** R4；S6。
3. **外部可观察结果：** 第 10 节质量门禁全部通过。
4-8. （无生产修改；依赖 U1-U3 全部完成。）
9. **验收测试：** 第 9 节命令清单的「最终」行。
10-13. 无 Red/Green 新增。
14-16. **集成验证/回归：** `./scripts/run-tests.sh`（两阶段 nextest + doctest）；`cargo run -p ralph-e2e -- --mock` 仅当既有 E2E fixture 覆盖 wave fan-in（执行时确认，未覆盖则记录并跳过，不得现造 E2E）。
17. **预期文件变更：** 无（允许 `cargo fmt` 产生的格式归零）。
18. **完成标准：** 第 10 节门禁全绿；未验证项与剩余风险已书面记录。
19. **停止条件：** 全量基线出现与本计划无关的 flake → 按仓库规则 `RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh` 兜底诊断；若失败与本计划相关，回对应 Unit。
20. **风险与注意事项：** 不得以「定向子集通过」替代 `run-tests.sh` 最终门禁。

## 8. Unit 串行依赖图

```
Unit 1（守卫 Red）
  ↓ U2 依赖 U1 的失败断言作为 Green 判据
Unit 2（helper + 两处替换）
  ↓ U3 依赖 U2 的 helper 与已切换的调用点
Unit 3（回归与边界）
  ↓ U4 依赖 U1-U3 全部落地
Unit 4（最终门禁）
```

- U1→U2：U2 的 Green 判据就是 U1 守卫转绿；顺序不可换（先有可失败断言再有实现）。
- U2→U3：U3 的 2288 数值来自 U2 helper；不可提前写 U3（helper 不存在时写会编译失败，非有效 Red）。
- 避免提前实现：U1 不引用 helper 符号；U2 不写边界回归；U3 不动生产代码。

## 9. 执行命令清单

| 时机 | 命令 | 验证目的 | 预期结果 | 失败处理 |
| --- | --- | --- | --- | --- |
| U1 Red | `cargo nextest run -p ralph-cli --bin ralph -- fan_in_deadline_uses_wave_derived_helper` | 守卫测试真实 Red | 断言失败（窗口内为 supervisor 配置读取） | 若为编译/定位错误，修正测试后重取 Red |
| U2 Green | `cargo nextest run -p ralph-cli --bin ralph -- effective_detected_aggregate_deadline` | helper 值测试 | 全绿 | 停止 U2 |
| U2 守卫转绿 | `cargo nextest run -p ralph-cli --bin ralph -- fan_in_deadline_uses_wave_derived_helper` | 接线完成 | 绿 | 停止 U2 |
| U2 差分 | 同 U2 命令（`helper_matches_inline_expression`） | 语义逐位保持 | 绿 | 停止，重对 @1742-1768 |
| U3 | `cargo nextest run -p ralph-cli --bin ralph -- regression_six_slots`；`cargo nextest run -p ralph-cli --bin ralph -- fan_in_injects_complete_with_wave_deadline` | 失败模式回归 | 全绿 | 回 U2 |
| U2/U3 相邻回归 | `cargo nextest run -p ralph-cli --bin ralph -- aggregate`；`cargo nextest run -p ralph-core -- phase` | 相邻模块 | 无回归 | 停止 |
| U3 既有锚点 | `cargo nextest run -p ralph-cli --bin ralph -- terminal_context_preserves_elapsed_timeout_relation` | 既有透传契约 | 绿 | 停止 |
| U4 格式/lint | `cargo fmt --all -- --check`；`cargo clippy` | 风格门禁 | 0 diff / 无新 lint | 修复重跑 |
| U4 最终 | `./scripts/run-tests.sh` | workspace 两阶段 + doctest | 全绿 | 按仓库 flake 兜底规则诊断 |

失败一律不得进入下一步；环境/命令错误先修正环境再重新取证。

## 10. 最终质量门禁

- S1-S7 全部有测试覆盖且通过；R1-R4 均可追踪到测试。
- fan-in 与执行阶段数值相等（差分测试 + 守卫测试双重证明）；失败 run 形状下该值 = 2288 而非 600/930。
- 726s/6-slot 全完成 → Integrate；deadline+1 → Failed(Timeout)；`==` 不超时。
- `phase.rs` 顺序、`SupervisorConfig` 默认、legacy @1388-1393 路径、`run_supervisor_fan_in` 签名均未变。
- 既有 `tests/wave_supervisor.rs` fan-in 用例、`terminal_context_preserves_elapsed_timeout_relation` 无回归。
- fmt/clippy/run-tests.sh 通过；无新增 skip/ignore/弱化断言；无未解释 snapshot 变更。
- 剩余风险（书面确认）：① 无 handle_wave_events 端到端测试，接线绑定依赖源码守卫 + 运行时测试组合（D3，置信度 0.88）；② `presets/en/implementation-review.yml:60-61` 注释中的「~930s」是 pre-floor 值，属文档偏差，按非目标另行跟进；③ 诊断报告 P1（summary/handoff 误写成功）与 P2（wave-id orphan）不在本计划。
- 复跑验证（可选，operator 执行）：对同一 plan 复跑 `implementation-review`，6 reviewer 全成功且耗时 < 2288s 时 `fan_in=InjectedComplete`、产出 `review.wave.complete`。

## 11. 最终计划自检

| 检查项 | 结果 | 证据或说明 |
| --- | --- | --- |
| 这是实施计划而不是 Roadmap | 是 | 每个 Unit 有真实入口、行为、Red/Green、回归与停止条件 |
| Executor 是否仍需做关键设计决策 | 否 | helper 签名/公式/替换点/测试落点/边界数值均已固定 |
| 所有文件和接口是否有代码库证据 | 是 | E1-E14 全部逐行核对于基线 a9dff24e；已修正初版虚构路径 |
| 所有关键决策置信度是否 ≥ 0.85 | 是 | D1-D5：0.92/0.95/0.88/0.97/0.96 |
| 是否存在未处理的低置信度假设 | 否 | 初版「930s」假设已证伪并重算为 2288s（E5） |
| 每个 Unit 是否只有一个可观察行为 | 是 | U1 守卫 Red / U2 helper 切换 / U3 回归固定 / U4 门禁 |
| 每个 Unit 是否可以独立验证 | 是 | 各有命令与判据 |
| 每个 Unit 是否有真实 Red | 是 | 行为 Red 在 U1（守卫断言失败）；U2/U3 的 Red 归属与 extract-method 语义已说明 |
| 每个 Unit 是否包含回归范围 | 是 | 见各 Unit 16 节 |
| 是否存在未来 Unit 依赖 | 否 | 依赖图线性且每 Unit 禁止提前实现 |
| 是否存在泛化任务描述 | 否 | 全部绑定具体符号/行号/数值 |
| 所有 Scenario 是否可追踪到测试和 Unit | 是 | 第 5、6 节矩阵 |
| 所有关键决策是否有 Evidence | 是 | D1-D5 引用 E2-E14 |
| 计划是否可以严格串行执行 | 是 | 第 8、9 节 |
