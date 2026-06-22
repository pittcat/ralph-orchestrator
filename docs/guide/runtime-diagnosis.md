# Runtime Diagnosis (运行时诊断)

> **Updated:** 2026-06-06 · **Schema:** recovery envelope v1 · diagnosis report v1

> **注意：** 本文档中提到的 `task.resume` 已被 `loop.resume` 和 deterministic correction block（`state.prompt_context`）取代。`task.resume` 仅在旧 JSONL replay  fixture 中作为 `loop.resume` 的 deprecated alias 保留。新的 recovery 信号不再以独立 bus 事件形式出现，而是直接注入下一轮 prompt 的 `## ORCHESTRATOR CORRECTION` / `## LOOP RESUME CONTEXT` 块。

Runtime Diagnosis 是 2026-06 drift-auto-calibration 计划（U0–U8）引入的**可观测性 + 自校准**层。它把“loop 看起来卡住了”这类主观感受变成可在 `.ralph/diagnostics/<session>/` 下审查的结构化证据，并由 `ralph diagnose` 命令离线渲染成人 / 机器可读的报告。

整个子系统是 **opt-in、additive、零回归** 的：未配置 `telemetry:` 时 orchestrator 的行为与历史完全一致；一旦启用，它会同时记录三类信号：

- **Recovery**：每个反压点（payload 合约、执行合约、workflow 守卫、stall、loop stale 等）写一条 `RecoveryDiagnosisEnvelope` 到 `recovery.jsonl`。
- **Drift**：每轮把 event bus 上的事件喂给 `DriftDetector`，3 个指标（field completeness / coord join rate / emit cadence）跌破阈值时落 `drift.jsonl`。
- **Loop summary**：loop 启动 / 终止时在 `.ralph/agent/summary.md` 末尾追加 `## Diagnostics` 段，并写 `diagnosis-summary.json` 种子供 `ralph diagnose` 复用。

本文是面向用户的入门 + 参考手册：能跑通、能解读、能用报告反向定位 preset / hat / hook 的问题。

---

## 1. 快速开始：3 步跑起来

```bash
# 1) 跑一个 loop，强制开启全量诊断 session。
RALPH_DIAGNOSTICS=1 ralph run -c ralph.yml -H builtin:ce-executor -p "fix flaky test" --max-iterations 20

# 2) 看 .ralph/diagnostics/<session>/ 下生成了哪些文件
ls -1 .ralph/diagnostics/$(ls -1 .ralph/diagnostics/ | grep -E '^[0-9]{4}-' | tail -1)/
#   预期看到：diagnosis-summary.json  drift.jsonl  errors.jsonl  orchestration.jsonl  recovery.jsonl

# 3) 渲染 Markdown 报告（也可 --format json 给 CI 用）
ralph diagnose --session latest
# 或显式指定：
ralph diagnose --session 2026-06-06T13-45-00 --format markdown
```

> 不想每次都开全量？把 `RALPH_DIAGNOSTICS=1` 换成在 `ralph.yml` 里写 `telemetry.runtime_diagnosis.write_artifacts: true`（见下一节）。

---

## 2. 配置文件：`telemetry` 段详解

`telemetry.runtime_diagnosis` 是整个诊断层的总开关。`telemetry:` 段整体可省，省略时等价于显式声明所有默认值（与历史行为字节级一致）。

```yaml
telemetry:
  runtime_diagnosis:
    enabled: true                 # 总开关；不写 = false
    write_artifacts: true         # 是否把 session 写到 .ralph/diagnostics/<ts>/
    prompt_injection_enabled: true # 是否把 alert 注入下一次 agent prompt（## Runtime Diagnosis Alert）
    max_prompt_findings: 5        # 单次注入最多折叠多少条 finding
    max_prompt_chars: 2000        # 注入块的硬上限（字符数）
    retry_window_iterations: 5    # responder 用来判断 "repeated" 的窗口大小
    max_repeated_recoveries: 3    # 同一 retry_key 在窗口内出现几次后从 Soft 升到 Hard
    artifact_retention: 10        # 磁盘上保留多少个最近 session；旧 session 会被 best-effort 清理
    malformed_jsonl_policy: warn  # 读到坏行怎么办：skip | warn（默认） | error

    drift:                        # U5 drift detector 阈值
      window_size: 50             # 滚动窗口里保留多少条 event
      field_completeness_threshold: 0.9   # (topic, field) 出现率阈值
      coord_join_rate_threshold:  0.6    # (from_topic, to_topic) 边出现率阈值
      emit_cadence_sigma:         2.0    # emit 间隔的标准差倍数阈值（>0）
```

### 字段级说明

| 字段 | 默认值 | 含义 / 何时该调 |
|---|---|---|
| `enabled` | `false` | 总开关。只想用 `ralph diagnose` 看历史而不希望当前 run 受影响，保持 `false` 即可。 |
| `write_artifacts` | `false` | 决定是否生成 session 目录与 JSONL 文件。仅设 `enabled: true` + `write_artifacts: false` 仍会让 responder 跑、prompt alert 生效，但磁盘上不留痕。 |
| `prompt_injection_enabled` | `false` | 关闭时 responder 的 `apply_runtime_diagnosis_prompt` 不会向 prompt 写 `## Runtime Diagnosis Alert`。开 / 关对磁盘 session 内容无影响。 |
| `max_prompt_findings` | `5` | responder 在一次 prompt 中最多折叠多少条 finding。`ralph_core` 内部还有一个硬上限 `HARD_MAX_FINDINGS=32`，永远不会写出超过 32 条。 |
| `max_prompt_chars` | `2000` | prompt alert 块的最大字符数。超出会被截断，避免极端 finding 撑爆 prompt。 |
| `retry_window_iterations` | `5` | responder 在跨多少次迭代内合并 `retry_key` 计数；超出窗口的旧 finding 被视作 "已忘记"。 |
| `max_repeated_recoveries` | `3` | 同一 `retry_key` 在窗口内出现第几次后升级为 Hard escalation。 |
| `artifact_retention` | `10` | reporter 在清理磁盘时保留多少个最近的 session 目录。`0` 被验证器拒绝。 |
| `malformed_jsonl_policy` | `warn` | 读到坏 JSONL 行的处理方式：`skip` 静默丢、`warn` 丢并记 warning、`error` 直接失败。 |
| `drift.window_size` | `50` | drift detector 滚动窗口。越大越平滑、越占内存。 |
| `drift.field_completeness_threshold` | `0.9` | (topic, field) 出现率 < 阈值 → 写一条 `field_completeness` finding。 |
| `drift.coord_join_rate_threshold` | `0.6` | declared `(from_topic, to_topic)` 边的出现率 < 阈值 → 写 `coord_join_rate` finding。 |
| `drift.emit_cadence_sigma` | `2.0` | topic 的 emit 间隔偏离历史均值超过 N 个标准差 → 写 `emit_cadence` finding。 |

### 验证规则

`ralph run` 在启动 hard gate 里调用 `TelemetryConfig::validate`：

- **硬错误**（loop 拒绝启动）：所有 `*_findings / *_chars / *_iterations / *_recoveries / *_retention / window_size` 为 0；`field_completeness_threshold / coord_join_rate_threshold` 不在 `[0.0, 1.0]`；`emit_cadence_sigma <= 0`。
- **软警告**（启动但打印）：`enabled: false` 同时 `write_artifacts: true` —— collector 仍会创建 session 目录但响应器跑不起来，是常见脚滑。

---

## 3. 激活矩阵：env / config / 行为对照

| `runtime_diagnosis.enabled` | `write_artifacts` | `RALPH_DIAGNOSTICS=1` | 实际行为 |
|---|---|---|---|
| `false` | `false` | unset | 完全 no-op，无 session 目录 |
| `true` | `false` | unset | 内存里有 finding，prompt alert 生效，磁盘上无 session |
| `false` | `true` | unset | 启动时 warn，collector 仍创建 minimal session（误配兜底） |
| `true` | `true` | unset | minimal session + U3+ 的 recovery / drift logger 都开 |
| 任意 | 任意 | `1` | **full diagnostics**（包含 `orchestration.jsonl` / `errors.jsonl` 等历史 logger） |

> `RALPH_DIAGNOSTICS=1` 是历史遗留的"全量诊断"开关，开启时它会**覆盖** `runtime_diagnosis` 段：full session 是 minimal session 的超集。

---

## 4. Recovery Diagnosis Envelope 模型

U2 的 `RecoveryDiagnosisEnvelope`（schema_version=1）是 13 个 `source` × 4 个 `severity` × 6 个 `outcome` 的笛卡尔积，外加一个稳定的 `retry_key` 用来跨迭代聚合。

### 13 个 `source`（诊断源）

| source | 含义 | 典型 reason_code |
|---|---|---|
| `stall_recovery` | 某个 hat 整轮没产出 event，loop 注入了 `task.resume` | `no_events` |
| `missing_event_gate` | hat 有 publishing 义务但本轮没发 | `no_emit` |
| `workflow_guard` | workflow phase/chain 顺序拒收 | `out_of_order_phase` |
| `execution_contract` | 执行合约拒绝 `work.done` | `no_git_evidence`, `task_open`, `missing_field` |
| `payload_contract` | preset 声明的 `event_policy.schemas` 拒收 | `missing_field` |
| `drift_monitor` | U5 drift detector 检出指标跌破阈值 | `drift_field_completeness`, `drift_coord_join_rate`, `drift_emit_cadence` |
| `hook_retry` | pre/post agent hook 被重试 | `hook_timeout`, `hook_nonzero` |
| `loop_stale` | 整个 loop 跨迭代无进展 | `stale` |
| `topic_format` | topic 不在 hat publishes / system-control 白名单（U5/R9，non-retryable） | `topic_not_allowed` |
| `agent_doc_sync` | `ralph run` 启动时同步 managed doc blocks 失败或降级 | `sync_failed`, `sync_completed`, `sync_up_to_date` |
| `wave_dispatcher` | U2（2026-06-17-001）：wave worker 的 `tokio::spawn` 或 semaphore acquire 失败，无法 materialize workers | `wave_spawn_failed` |
| `cli_emit` | CLI `ralph emit` precheck 拒收（U1/U2，2026-06-14） | `not_in_allowlist`, `policy_check_failed` |
| `flow_lifecycle` | wave / parallel-flow 生命周期状态转换（U1/U3/U5/U7 2026-06-17-001） | `wave_spawn_failed`, `aggregate_timeout`, `wave_timeout_drift`, `phase_transition` |

### 6 个 `outcome`（终态）

| outcome | 含义 |
|---|---|
| `pending` | envelope 刚被写出来，responder 还没观察结果 |
| `recovered` | 同一个 `retry_key` 在后续 iteration 看到了自愈（target hat emit 了期望 topic） |
| `repeated` | 在 `retry_window_iterations` 窗口内又被观察到了 |
| `escalated` | 已升级为 Hard / Final（见 §6） |
| `failed` | 推动了 loop 终止或 hard pause |
| `not_retriable` | 不可由 retry 解决（通常是 preset / hat instructions 需要人工修） |

### 关键字段

```jsonc
{
  "schema_version": 1,
  "diagnosis_id": "uuid-v4",            // 同 retry_key 多次观察时身份不变
  "iteration": 12,
  "source": "payload_contract",
  "severity": "error",
  "source_hat": "builder",              // 谁犯了事
  "target_hat": "builder",              // 谁应该接锅（safe_target=true 时有效）
  "topic": "work.done",
  "reason_code": "missing_field",
  "message": "topic 'work.done' missing required field 'plan_name'",
  "expected_action": "include plan_name in the work.done payload",
  "evidence": [
    { "kind": "field", "ref_path": "plan_name", "snippet": "..." }
  ],
  "retry_key": "payload_contract:builder:work_done:missing_field:plan_name",
  "retry_attempt": 0,
  "safe_target": true,
  "outcome": "pending",
  "timestamp": "2026-06-06T13:45:00Z",
  "session_id": "2026-06-06T13-45-00"   // 由 U3 logger 回填
}
```

`retry_key` 格式：`"{source}:{target_or_*}:{topic_or_*}:{reason_code}:{field_or_*}"`，5 段都是 snake_case + 大小写归一化。`ralph diagnose` 用它做跨迭代聚合，因此**改 source 枚举名 / 改 reason_code 字符串是 breaking change**。

### Flow Lifecycle Envelope（flow_lifecycle source）

`flow_lifecycle` 是 2026-06-17-001 计划（U2/U9）引入的第十三个诊断 source，专门追踪 wave / parallel-flow 的生命周期状态转换。每条 envelope 记录一次状态转移，写入 `recovery.jsonl`。

**状态机**（`FlowPhase`）：

```
Detected → Spawning
Spawning → WorkersActive
WorkersActive → Aggregating | PartialClosed | Failed
Aggregating → Closed | Degraded
PartialClosed → Degraded
Failed → Degraded
```

**Envelope 字段：**

| 字段 | 类型 | 说明 |
|---|---|---|
| `flow_unit_id` | string | wave 唯一标识（首版即 `wave_id`），也是 `retry_key` 的第一段 |
| `target_hat` | string | owning hat id |
| `wave_total` | u32 | `wave_total`，声明的 worker 总数 |
| `received_count` | u32 | 已上报结果的 worker 数量 |
| `missing_indices` | [u32] | 未上报的 worker 索引（如 `PartialClosed` 时填充） |
| `phase` | string | 当前 `FlowPhase.label()`（`detected` / `spawning` / `workers_active` / `aggregating` / `closed` / `partial_closed` / `failed` / `degraded`） |
| `reason_code` | string? | 仅终态填充，典型值：`wave_spawn_failed` / `aggregate_timeout` / `partial_threshold` / `all_workers_reported` |
| `reason_message` | string? | 人类可读原因描述 |
| `configured_aggregate_secs` | u64 | 该 wave 配置的聚合超时秒数 |
| `configured_worker_secs` | u64 | 该 wave 配置的单 worker 超时秒数 |
| `started_at` | string | RFC3339 时间戳，记录 wave 首次检测时间 |
| `last_transition_at` | string | RFC3339 时间戳，最近一次状态转换时间 |

**典型 reason_code 映射：**

| 触发条件 | reason_code | 对应 phase |
|---|---|---|
| dispatcher 发出 spawn 请求 | `spawn_requested` | `spawning` |
| 所有 worker 成功 spawn | `workers_spawned` | `workers_active` |
| spawn 失败或全被 isolated scope 拒绝 | `wave_spawn_failed` | `failed` |
| 所有 worker 均已上报 | `all_workers_reported` | `closed` |
| 部分上报且达到 partial 阈值 | `partial_threshold` | `partial_closed` |
| mechanism 发出 degraded terminal | `aggregate_timeout` / `escalation` | `degraded` |

> `flow_lifecycle` envelope 是只读的观测信号，不影响 `WaveDispatchOutcome` 或 `PartialWavePolicy` 的内部行为。它通过 `FlowLifecycleRegistry`（`crates/ralph-core/src/flow_lifecycle.rs`）写入，供 `ralph diagnose` 渲染 wave 健康状态。

### Semantic Gate Envelope（semantic_gate_violation）

2026-06-17-003 计划（U1+U2）引入的 `SemanticGateViolation` envelope kind，用于区分 schema 级 schema-mismatch 与**wave 状态级**的 semantic gate 拒收。`semantic_gate_violation` 不归入 fatal `PayloadContractViolation` 桶——它落在独立 bucket，loop 继续。

**触发场景：**

| gate 名 | 触发条件 | 含义 |
|---|---|---|
| `review_passed_while_wave_open` | `hat=review-coordinator` emit `review.passed` 而 `ReviewStepTracker.open_wave_id` 仍非空 | review-coordinator 在 wave 未闭合时不能走 empty_diff fast-path；agent 必须等待机制层 `plan.blocked`（U2）或补全维度 |

**envelope 字段（`payload_contract` source, severity=error, reason_code=semantic_gate_violation）：**

```jsonc
{
  "source": "payload_contract",
  "severity": "error",
  "source_hat": "review-coordinator",
  "target_hat": "review-coordinator",  // R5 hard-gate routing
  "topic": "review.passed",
  "reason_code": "semantic_gate_violation",
  "message": "review_passed_while_wave_open: review-coordinator must not emit review.passed while wave 'w-...' is incomplete (4/11 dimensions)",
  "retry_key": "payload_contract:review-coordinator:review.passed:semantic_gate_violation:gate=review_passed_while_wave_open",
  "expected_action": "等待机制 plan.blocked 或补全维度后重发 review.passed",
  "outcome": "pending"  // 不进 U2_REJECTION_RETRY_LIMIT 桶
}
```

> U1 修复后，`review_passed_while_wave_open` 不再误标为 `InvalidFieldValue { field: "skip_reason" }`，避免 payload-contract-error.json 的 `field/value` 误导审计。`gate` 字段是规范名（canonical name），`context` 字段是 wave 状态摘要。

### Incomplete Wave 机制收摊（plan.blocked）

2026-06-17-003 计划 U2 引入的**机制层**收摊路径：当 review wave 收齐维度不足且 `now - last_dimension_at > 0.8 * aggregate_timeout_secs` 时，`EventLoop::maybe_emit_incomplete_wave_blocked` 自动 emit `plan.blocked` 而非依赖 `review-synthesizer` agent 自觉。

**机制 vs 编排分工：**

| 卡点 | 机制（Rust） | 编排（preset） |
|------|--------------|----------------|
| wave 没收齐 | `incomplete_wave_gate::evaluate` 触发 `plan.blocked`（route: `review-synthesizer` → `shipper`） | `review-synthesizer` 在 `publishes` 中声明 `plan.blocked`（preset 校验硬门） |
| empty_diff 旁路 | semantic gate recoverable（见上） | preset empty_diff 强约束 `wave_closed + received == wave_total` |
| 二次 work.done | event_policy dedup（U4） | executor 指令「禁止重发」 |

**触发条件（所有 4 条同时满足）：**

1. `workflow_contract.incomplete_wave_gate.enabled = true`（仅 `ce-executor-isolated` preset 显式开启）
2. `ReviewStepTracker.open_wave_id` 非空 + `received < expected`
3. `last_dimension_at` 已设置（至少 1 个 dimension.done 到过）
4. `now - last_dimension_at > 0.8 * aggregate_timeout_secs`（**仅 staleness**，不含 handoff timeout；handoff 归 U3 ladder）
5. `FlowLifecycleRegistry` 中 wave phase ∉ `{WorkersActive, Spawning}`（避免与 U4 `inject_review_aggregate_timeouts` 抢跑）

**emit 的 `plan.blocked` payload shape：**

```jsonc
{
  "reason": "dimension_reviewers_failed_to_converge",
  "wave_id": "w-...",
  "plan_name": "...",      // 由 runner 填自 tracker
  "task_id": "...",
  "step": "...",
  "expected": 11,
  "received": 4,           // unique count
  "missing_dimensions": [],
  "staleness_secs": 1440,  // 0.8 * 1800
  "aggregate_timeout_secs": 1800
}
```

**routing：** `Event::with_source("review-synthesizer").with_target("shipper")`（plan-gate `triggers` 不含 `plan.blocked`，不可假定 plan-gate 消费）。

emit 后 tracker 调用 `close_wave`，避免同 wave 在下一 iteration 重复 emit。

---

## 5. U5 的 3 个 drift 指标

`DriftDetector` 维护一个滚动窗口（默认 50 条 event），每条都跑 3 个独立 metric：

| 指标 | metric 字段 | reason_code | 何时被触发 |
|---|---|---|---|
| Field completeness | `field_completeness` | `drift_field_completeness` | `(topic, field)` 的窗口内出现率 < `field_completeness_threshold`（默认 0.9） |
| Coord join rate | `coord_join_rate` | `drift_coord_join_rate` | preset / `ExecutionContractsConfig` 声明的 `(from_topic, to_topic)` 边出现率 < `coord_join_rate_threshold`（默认 0.6） |
| Emit cadence | `emit_cadence` | `drift_emit_cadence` | 同一 topic 的 emit 间隔偏离历史均值超过 `emit_cadence_sigma`（默认 2.0）个标准差；少于 `EMIT_CADENCE_MIN_SAMPLES=5` 个样本不评估，避免冷启动误报 |

每个 finding 会同时落到 `drift.jsonl`（结构化）和 `recovery.jsonl`（envelope 形式，便于和 `soft / hard` 升级逻辑联动）。

---

## 6. RecoveryResponder 的 3 级升级

`ralph_core` 内的 `RecoveryResponder`（U6）是唯一会把 envelope 翻译成 runtime 行动的模块。**三档动作**如下表：

| Level | 何时触发 | 实际动作 | 不变量 |
|---|---|---|---|
| **Soft** | 新 finding，或 attempt 仍 < `max_repeated_recoveries` | 在下一次 prompt 注入 `## Runtime Diagnosis Alert` 块 | 不发新 event，不影响终止 |
| **Hard** | 同一 `retry_key` 已尝试 `max_repeated_recoveries` 次以上 **且** `safe_target=true` | runner 合成 `task.resume` 发给 target hat | target 必须是已注册 hat；不在 source hat 上重复触发 |
| **Final** | 无 safe target，或 retry 窗口已耗尽 | 给 loop runner 一个 `TerminationHint` | 不会覆盖 `TerminationReason::PayloadContractViolation` 等已有原因 |

`apply_runtime_diagnosis_prompt` 的格式：

```
## Runtime Diagnosis Alert

- [payload_contract/error] topic 'work.done' missing field 'plan_name' (retry_key=…:…, attempt=1, outcome=pending)
  -> expected action: include plan_name in the work.done payload
- [drift_monitor/warning] field 'plan_name' completeness dropped to 0.4 < 0.9 on topic 'work.done' (…)
  -> expected action: Investigate field_completeness drift: observed 0.4000 below threshold 0.9000
```

> block 内的总字符数受 `max_prompt_chars` 限流；超过时截断，行为可在 U6 单元测试 `truncate_to_max_chars_*` 里复现。

---

## 7. `ralph diagnose` 报告解读

`ralph diagnose --session latest` 默认渲染 Markdown 报告，结构稳定（CI 解析用 `--format json`，schema v1）。

### 7.1 Run summary 段

- `session path`：本次选择的 session 目录绝对路径
- `session id / loop started / loop terminated / total iterations / termination reason`：从 `diagnosis-summary.json` 种子读取；缺失时显式标注 `summary seed: not present (run did not write diagnosis-summary.json)`
- `recovery journal entries` / `drift findings`：journal 中已写入的条数

### 7.2 Top findings（核心表格）

按以下顺序排序：

1. `severity` 高 → 低（`critical > error > warning > info`）
2. `escalated` 优先于未升级
3. `outcome == Failed` 优先于其他 outcome
4. `occurrences` 多 → 少
5. `last_iteration` 新 → 旧
6. `retry_key` 字典序（稳定 tiebreaker）

每行展示 `severity / source / target / topic / occurrences / first→last iter / outcome / retry_key`。**`retry_key` 是跨 run 反查的稳定 key**，看到它时优先用 `grep -r "<retry_key>" .ralph/diagnostics/` 找原 envelope。

### 7.3 Recovery timeline

按文件顺序展示 `recovery.jsonl` 的每一行，每行包含 `iter / hat / severity / outcome / target / topic / reason / message`。这适合"按时间回放"，与 Top findings 互补。

### 7.4 Drift findings

把 `drift.jsonl` 整体渲染成 `metric / topic / field / observed / threshold / severity / iter / message` 表格。**`observed < threshold` 是 finding 被触发的方向** —— 如果你要调阈值，先确认你的业务场景里"正常"是 0.85 还是 0.95，再调而不是一味降低阈值。

### 7.5 Preset topology health

聚合 `orchestration.jsonl`，按 hat 展示 `selected / published / backpressure / contract rejections / routed / no target` 计数。**`backpressure` 持续 > 0** 是 preset 拓扑太紧的早期信号；**`contract rejections` > 0 且 `no target` > 0** 说明拒收事件但没有 hat 接，需要补 publishing contract。

### 7.6 Contract health

把 `orchestration.jsonl` 里的 `ExecutionContractRejected` / `ContractRecoveryRouted` 单独拉出来。`routed to <hat>` 表示响应器成功给出了一个 `task.resume`；`no target: <reason>` 表示 responder 走了 Final 路径。

### 7.7 Errors

直接渲染 `errors.jsonl` 中的 `iter / hat / error_type / message`，不做过滤。

### 7.8 Suggested next actions

针对 `top_findings` 按 source 类型给修复方向（最多 5 条）：

| source | 建议 |
|---|---|
| `payload_contract` | 修改 preset 的 `event_policy.schemas.<topic>.required_fields`，给该 topic 补齐缺失字段 |
| `execution_contract` | 更新 hat `<target>` 的 instructions，让 emit `<topic>` 时附上 `<reason_code>` 字段 |
| `missing_event_gate` | 在 hat `<target>` 的 publishing contract 里强制补上 `<topic>` 事件 |
| `workflow_guard` | 检查 preset 的 workflow phase 顺序，确认 hat `<target>` 在正确的 phase emit `<topic>` |
| `drift_monitor` | 调整 `telemetry.runtime_diagnosis.drift` 阈值，或修复 hat instructions 中缺失字段 |
| `stall_recovery` | 检查 hat 是否被 OOM / 网络中断打断；考虑调低 max_iterations 触发更早的 steering |
| `hook_retry` | 检查 `pre_agent` / `post_agent` hook 是否有超时或非零退出码 |
| `loop_stale` | 运行 `ralph loops` 确认是否有并行 loop 在 hold state |
| `agent_doc_sync` | 检查 `ralph.yml` 中 `agent_doc_sync.on_error` 设置；如为 `warn`，sync 失败不阻塞启动但会记录；如为 `strict`，进程已退出 78。确认目标文件（`CLAUDE.md` / `AGENTS.md`）可写且文件锁无竞争。详见 [Managed Agent Doc Blocks](managed-blocks.md) |
| `flow_lifecycle` | 查看 `recovery.jsonl` 中 `flow_lifecycle` 条目的 `phase` 与 `reason_code`：若为 `failed` 说明 spawn 失败或被 isolated scope 全量拒绝；若为 `degraded` 说明 mechanism 触发了 degraded 路径；若 `received_count < wave_total` 且 `phase=workers_active` 说明 worker 还在运行或部分卡住 |

如果某个 finding 已经 `escalated`，会附加一句"retry_key `<X>` 已 escalation N 次（first→last），建议人工介入或调高 `telemetry.runtime_diagnosis.max_repeated_recoveries`"。

### 7.9 Warnings

`recovery.jsonl` / `drift.jsonl` / `orchestration.jsonl` / `errors.jsonl` 任一缺失或某行 JSON 解析失败都会在这里列出来；报告本身仍然渲染成功。

---

## 8. 常见症状 → 报告定位流程

### 症状 A："loop 看起来在合法地空转"

直接看 `Drift findings` 表格：

- `field_completeness`：hat 还在 emit 事件但字段开始掉 → 修 hat instructions。
- `coord_join_rate`：preset 声明的边几乎不出现 → 拓扑不对，需要重排 hat publishing contract。
- `emit_cadence`：单 topic 突然慢 / 快 → 看 `ralph events` 对照时间线，是否有 hook 卡住。

### 症状 B："治愈无效，循环重复同样的 recovery"

1. 在 `Top findings` 里找到 `occurrences` 高、`outcome=repeated` 的行。
2. 复制 `retry_key`，在 `Recovery timeline` 里 grep 该 key，逐行看 `expected_action` 是否被 hat 实际执行。
3. 如果是 → 调高 `max_repeated_recoveries` 让 responder 提前升级；如果是 → 修复 hat 让它真的 emit 期望 topic。

### 症状 C："preset 渐进式断裂"

看 `Preset topology health` 段：

- 多个 hat 的 `backpressure` 持续上升 → hat 之间的 triggers / publishes 配错。
- 某 hat 长期 `selected=0` 但 `published>0` → 该 hat 在错误 phase 被驱动，触发 `workflow_guard`（参见 `Contract health` 段）。

### 症状 D："loop 莫名终止"

`Run summary` 的 `termination reason` 是关键：

- `payload_contract_violation`：preset 静态 hard gate 直接拒收，不需要看 diagnose。
- `max_iterations`：纯算力耗尽。
- 其他 `*` / 缺失：看 `Recovery timeline` 最后几行的 `outcome == Failed`，再回查 `Suggested next actions`。

---

## 9. 命令参考：`ralph diagnose --help` 全参数

```text
$ ralph diagnose --help
Build an offline diagnosis report from `.ralph/diagnostics/<session>/` (U7)

Usage: ralph diagnose [OPTIONS]

Options:
      --session <SESSION>
          Session to read from. Accepts:
          - "latest" (default) — pick the most recent timestamped session
          - an absolute path
          - a relative path
          - a timestamped session id relative to `--diagnostics-root`

      --format <FORMAT>
          Output format. Markdown (default) is human-readable; JSON is the
          stable CI contract (schema_version="1").
          [possible values: markdown, json]

      --output <PATH>
          Write the report to this path instead of stdout. When set, stdout
          receives only the written path (Markdown) or a short summary line (JSON).

      --diagnostics-root <PATH>
          Path to the diagnostics root. Default resolution (U3): read
          `<workspace>/.ralph/loops.json`; take the latest active loop's
          `workspace.workspace` field; use `<that-workspace>/.ralph/diagnostics`.
          Falls back to `<workspace>/.ralph/diagnostics` when `loops.json`
          is missing/empty or its latest active entry points at a dead
          worktree. Pass `--diagnostics-root` to bypass the registry.

### Worktree loops（2026-06-13-004 U10）

When you run a loop inside a git worktree, the diagnostics files
land in the **worktree's** `.ralph/diagnostics/<session>/`, not the
main repo's. The default resolution above (`loops.json` →
`workspace.workspace`) handles this transparently: as long as the
worktree loop registered itself in the main repo's
`.ralph/loops.json` (the default for `ralph run` and parallel
loops), `ralph diagnose --session latest` from either the main
repo or the worktree will pick up the correct session.

If `ralph diagnose` cannot find the loop in `loops.json` (e.g. the
worktree was deleted without unregistering), pass
`--diagnostics-root <worktree>/.ralph/diagnostics` to point at the
session directory directly.

EventLoop construction (`EventLoop::with_context`) uses
`LoopContext::workspace()` to build the `DiagnosticsCollector`, so
the collector's session directory matches the worktree's
`.ralph/diagnostics/` automatically. No preset or config change
is required to opt a worktree loop into runtime diagnosis — set
`RALPH_DIAGNOSTICS=1` (or `telemetry.runtime_diagnosis.enabled: true`)
in the usual way.

### Session pointer 回退（2026-06-16-002 Unit 5）

Loop 启动时会在**主仓**写入 `.ralph/diagnostics-session-pointer.json`
（`{"session_path","written_at"}`），指向当前 workspace 的诊断 session。
Loop **正常结束 / TUI 退出 / 可恢复终止** 时会再次回写，指向最终 session。

`ralph diagnose --session latest` 解析顺序：

1. `loops.json` 里最新 active loop 的 workspace
2. 主仓 `diagnostics-session-pointer.json`
3. 扫描主仓 `.ralph/diagnostics/*/`，取最近修改且 `recovery.jsonl` 非空（或存在 `diagnosis-summary.json`）的 session
4. 回退到 `<cwd>/.ralph/diagnostics`

并发 worktree 时 pointer 为 **last-write-wins**（最后完成的 loop 覆盖）。
需要精确 session 时用 `--diagnostics-root <worktree>/.ralph/diagnostics`。

  -c, --config <CONFIG>   父命令通用参数：ralph.yml 或 core.field=value 覆盖
  -H, --hats <HATS>       父命令通用参数：hat 集合（一般 diagnose 不需要）
  -v, --verbose           父命令通用参数
      --color <COLOR>     auto | always | never
  -h, --help              简短 help（'--help' 看完整描述）
```

### 退出码

| Code | 含义 |
|---|---|
| `0` | 报告渲染成功（含 warnings 也算成功） |
| `2` | 找不到任何 session（`latest` 时根目录空、显式路径不存在） |
| `3` | `--session` 给了非时间戳字符串且不在文件系统中 |
| `4` | I/O 错误读 session 文件 |

> 仅 `2` 在脚本里是"loop 跑过但什么都没记录"的可靠信号；`3` / `4` 是参数或环境问题。

### 常见调用

```bash
# 默认：读最近一个 session，渲染 Markdown 到 stdout
ralph diagnose

# 写到文件
ralph diagnose --output ./reports/last-run.md

# CI：渲染 JSON 并用 jq 抽取 critical finding
ralph diagnose --format json --output report.json
jq '.top_findings[] | select(.severity == "critical")' report.json

# 显式指定 session id（相对 .ralph/diagnostics/）
ralph diagnose --session 2026-06-06T13-45-00

# 自定义根目录
ralph diagnose --diagnostics-root /var/log/ralph/sessions

# 当根目录空时，CLI 会打印 hint：
#   error: no diagnostics sessions at <path>
#   Hint: re-run with `RALPH_DIAGNOSTICS=1 ralph run ...`,
#         or set `telemetry.runtime_diagnosis.enabled: true` and
#         `telemetry.runtime_diagnosis.write_artifacts: true` in ralph.yml.
```

---

## 10. 故障诊断

| 症状 | 根因 / 修复方向 |
|---|---|
| `ralph diagnose` 报 `no diagnostics sessions at .ralph/diagnostics` | 还没产生过 session —— 跑一次 `RALPH_DIAGNOSTICS=1 ralph run ...`，或在 `ralph.yml` 写 `telemetry.runtime_diagnosis.write_artifacts: true` |
| `recovery.jsonl` 全是 malformed warning | JSONL 写盘被截断（OOM / SIGKILL）；检查 runner 日志，确认是 loop 中断而非 schema 不兼容 |
| `Drift findings` 一直空 | `window_size` 设得太大，超过 session 内事件量；或 `field_completeness_threshold` 调得太低，<br/>`emit_cadence` 冷启动时 `EMIT_CADENCE_MIN_SAMPLES=5` 不会触发 |
| Top findings 全是 `stall_recovery` | 某个 hat 整轮没 emit 事件；看 timeline 的 `hat` 列定位，常见于模型超时 / PTY 复用 bug |
| prompt 里反复出现 `## Runtime Diagnosis Alert` | responder 一直在 Soft 阶段，target hat 没有真的 emit 期望 topic；先用 `ralph diagnose` 定位 `retry_key`，再修 hat instructions |
| `Preset topology health` 里 `no target` 列很大 | execution / payload contract 拒收但没有 hat 接；看 `Contract health` 段的 `no target: <reason>`，补 publishing contract |
| `Suggested next actions` 提示"调高 `max_repeated_recoveries`" | 该 retry_key 一直在 Soft 阶段循环，确实需要升级到 Hard；先确认 hat 真的在执行 `expected_action`，再考虑调阈值 |

---

## 11. 磁盘文件清单

每次有效 run（开启了 `write_artifacts: true` 或 `RALPH_DIAGNOSTICS=1`）在 `.ralph/diagnostics/<timestamp>/` 下产生：

| 文件 | 谁写 | 内容 |
|---|---|---|
| `diagnosis-summary.json` | U3 / U8 | 1 个 JSON 对象，schema_version=1；包含 session_id、起止时间、count、note |
| `recovery.jsonl` | U3 / U4 / U6 | 每行 1 个 `RecoveryJournalEntry`（schema_version=1） |
| `drift.jsonl` | U3 / U5 | 每行 1 个 `DriftJournalEntry`（schema_version=1） |
| `orchestration.jsonl` | U3 (full diagnostics) | 每行 1 个 `OrchestrationEntry`，U5 也会用 finding 写入 |
| `errors.jsonl` | U3 (full diagnostics) | 解析 / 校验失败记录 |
| `agent_doc_sync.json` | agent_doc_sync | 紧凑快照（`synced` / `skipped` / `failed` / `last_success_at`），供 `ralph doctor` O(1) 读取 |

> U3 还会在 `.ralph/diagnostics/logs/` 下保留 TUI 模式的最近 5 份 `ralph-{ts}.log`；`ralph clean --diagnostics` 会清理它们（不会动 session 目录）。
>
> U8 会在 `.ralph/agent/summary.md` 末尾追加 `## Diagnostics` 段，写入 `## Diagnostic hint` / `recovery_journal_path` / `drift_journal_path` 等关键信息。

---

## 12. Serial review 链 recovery 形状（2026-06-17-004）

`builtin:ce-executor-serial` 把 4 个 review 维度串行走完。当 `dimension-reviewer` 声称 emit 了事件但实际上没有写入时，orchestrator 会注入一条 `task.resume` 并把它 pin 回同一个 hat。为了让 reviewer 在第二次激活时知道该 review 哪个维度，`task.resume` payload 必须携带原始触发上下文：

| 字段 | 类型 | 含义 |
|---|---|---|
| `stage` | string | `emit_claimed_but_not_written` 或 `missing_event` |
| `target_hat` | string | `dimension-reviewer` |
| `original_trigger_topic` | string | `review.dimension.ready` |
| `original_trigger_payload` | object | 包含 `dimension`、`focus`、`depth`、`diff_base`、`intent_summary`、`changed_files` 等 |
| `allowed_topics` | array | 该 hat 可发的 topic 列表 |

排查命令：

```bash
# 查看最近一次 task.resume 是否携带 original_trigger_*
jq -s 'map(select(.topic == "task.resume")) | last' .ralph/events-*.jsonl

# 如果 original_trigger_topic 缺失，说明 claim-but-no-write 路径未正确 replay
# 检查 recovery.jsonl 中的 stage
jq 'select(.topic == "task.resume") | {stage, target_hat, original_trigger_topic}' .ralph/diagnostics/latest/recovery.jsonl
```

典型恢复成功的 wire shape：

```text
work.done
  → review.dimension.ready(dimension=correctness)
  → [dimension-reviewer silent, no event]
  → task.resume(stage=emit_claimed_but_not_written, original_trigger_topic=review.dimension.ready)
  → review.dimension.done(dimension=correctness)
  → review.dimension.ready(dimension=testing)
  → ...
```

---

## 12.1 `work.start` 未进入 events.jsonl 排查（2026-06-17-004 U5）

从 plan 2026-06-17-004 U5 开始，loop 启动时会把配置的 `starting_event`（serial preset 下为 `work.start`）持久化到当前 `.ralph/events-{run_id}.jsonl` 的第一行，并立即把 `EventReader` cursor 推到文件末尾，避免 live loop 重复消费。

自检：

```bash
# 1. 确认 current-events marker 指向的文件
CURRENT=$(cat .ralph/current-events)

# 2. 第一行必须是 work.start
head -1 "$CURRENT" | jq '{topic, source}'
# 预期: { "topic": "work.start", "source": "loop-bootstrap" }

# 3. 不能出现两行 work.start（resume 路径不应重复注入）
grep -c '"topic":"work.start"' "$CURRENT"
# 预期: 1
```

如果第一行不是 `work.start`：

- 检查是否使用了 `--continue` / `resume` 模式；resume 路径使用 `task.resume`，不注入新的 `work.start`。
- 检查 `ralph.yml` / preset 的 `event_loop.starting_event` 是否被覆盖为 `task.start`。
- 检查 `.ralph/` 目录是否有写权限；I/O 失败会在日志中输出 `U5: failed to persist starting_event`。

---

## 13. Step Handoff 诊断（2026-06-17-002）

`ce-executor-isolated` preset 在 2026-06-17-002 中新增了两类 Step Handoff 诊断事件，均写入 `recovery.jsonl`：

| 信号 | `source` | `reason_code` | 含义 / 排查 |
|---|---|---|---|
| `handoff_dispatch_timeout` | `stall_recovery` | `handoff_dispatch_timeout` | `work.ready` 等唯一消费者 handoff 在 `event_loop.workflow_contract.handoff_dispatch_timeout_seconds`（默认 30s，上限 120s）内未被激活。排查：消费者 hat 是否崩溃 / 被隔离预算阻塞 / 后端未返回事件。 |
| `progress_task_mismatch` | `PayloadContract` / `WorkflowGuard` | `progress_task_mismatch` | `queue.advance` 或 `plan.complete` 被 pre-handoff gate 拒绝，原因是 `progress.md` 与 `tasks.jsonl` 不一致。排查：agent 是否正确关闭任务并回写 `## Completed Steps`；`tasks.jsonl` 中任务状态是否为 `closed`。 |

对应排查命令：

```bash
# 查看 handoff 超时记录
jq 'select(.reason_code == "handoff_dispatch_timeout")' .ralph/diagnostics/latest/recovery.jsonl

# 查看 progress/task 不一致记录
jq 'select(.reason_code == "progress_task_mismatch")' .ralph/diagnostics/latest/recovery.jsonl

# 查看当前 progress.md / tasks.jsonl 状态
cat .ralph/agent/progress.md
cat .ralph/agent/tasks.jsonl
```

---

## 13.1 emit rejection → task.resume → 修复 决策树

agent 在 loop 内收到 `task.resume` 后，按以下决策树定位问题层并修复：

```text
emit 失败 / 拒收
  │
  ├─ CLI 入口拒收（`ralph emit` 非零退出）
  │   ├─ stderr 提到 `not in allowlist` → 检查 RALPH_EVENTS_FILE / --file 路径
  │   ├─ stderr 提到 `policy check failed` / `validation_errors` → 读 `validation_errors[].field`，
  │   │     修正 payload 后用 `ralph emit --policy-check` 预检
  │   └─ stderr 提到 `isolated` / 越权 → 改用 hat `publishes` 列表内 topic（`ralph hats list`）
  │
  ├─ Loop 端拒收（events.jsonl 末尾出现 `task.resume`）
  │   ├─ `stage` = `origin` → 越权 topic（多发）或 unknown hat；改用 hat 实际可发 topic
  │   ├─ `stage` = `policy` + `required_fields` 非空 → 按字段补齐 payload
  │   ├─ `stage` = `execution_contract` → 读 `violation`；缺字段补字段，类型不匹配改类型
  │   └─ `stage` = `payload_contract` → 通常是直写 events.jsonl；停手走 CLI
  │
  └─ 复杂 violation（progress_task_mismatch / handoff_dispatch_timeout /
       plan.blocked 越权 / review_passed_while_wave_open）
      → ralph tools skill load ralph-tools-handoff
```

**自纠步骤**（按顺序）：

1. 读 PENDING EVENTS 里 `task.resume` payload（`stage` / `topic` / `violation` / `required_fields` / `allowed_topics`）。
2. 按决策树第一分支定位层。
3. 不要用 `--unsafe-no-policy-check`（`ce-executor-isolated` preset 默认 `allow_unsafe_cli_emit: false` 时该参数被拒）；不要直写 `events.jsonl`。
4. 修复后用 `ralph emit --policy-check` 预检；同源通过再正式发。
5. 仍不明：`ralph diagnose --session latest` 出报告；本 guide §10 解释 schema；本节决策树与 §13 互补。

**相关文档**：

- 自动注入的速查：`crates/ralph-core/data/ralph-tools.md` 「收到 `task.resume` 时」段
- emit 详表：`crates/ralph-core/data/ralph-tools-emit.md`
- handoff 深参考：`crates/ralph-core/data/ralph-tools-handoff.md`
- 机制设计：`docs/plans/2026-06-17-002-feat-ce-executor-step-handoff-plan.md`
- 机制边角（CLI 预检）：`docs/plans/2026-06-17-005-fix-agent-recovery-mechanism-gaps-plan.md`

---

## 14. 计划 / 设计文档

- 计划: `docs/plans/2026-06-04-004-feat-drift-auto-calibration-plan.md`（U0–U9 完整分解）
- 关键源文件：
  - 配置：`crates/ralph-core/src/config/telemetry.rs`
  - envelope / journal：`crates/ralph-core/src/diagnosis/{envelope,journal}.rs`
  - 写入与种子：`crates/ralph-core/src/diagnostics/{orchestration,drift}.rs`、`crates/ralph-core/src/diagnostics/mod.rs`
  - drift detector / observer：`crates/ralph-core/src/drift/{detector,window,alert}.rs`
  - 响应器与 prompt 注入：`crates/ralph-core/src/diagnosis/responder.rs`
  - 报告：`crates/ralph-core/src/diagnosis/reporter.rs`
  - CLI 子命令：`crates/ralph-cli/src/commands/diagnose.rs`
  - summary 集成：`crates/ralph-core/src/summary_writer.rs`（`append_diagnosis_hint`）

---

## 15. 一分钟自检清单

提交新 hat / preset 之前可以这样自检：

1. 跑一次 `RALPH_DIAGNOSTICS=1 ralph run ...` —— 必须能生成 `recovery.jsonl` 与（如果开了 drift 配置）`drift.jsonl`。
2. `ralph diagnose --session latest` —— 报告应能在 CI 中以 `jq` 解析（`schema_version == "1"`）。
3. `Top findings` 不应出现 `critical` / `error` 级 finding；如果出现，按 `Suggested next actions` 修复后再合并。
4. `## Runtime Diagnosis Alert` 块不应反复出现同一个 `retry_key`（>3 次）—— 出现说明 hat 真的没在执行 `expected_action`。
5. `summary.md` 末尾的 `## Diagnostics` 段应包含至少一条 `recovery_journal_path` / `drift_journal_path`。

---

## 10. `--from-ledger` 选项(U8 + U11-T3)

`ralph diagnose --from-ledger` 优先读取 `.ralph/recovery.jsonl` 和 `.ralph/ledger.jsonl`,
输出由 `correction::emit_correction_context` 写入的结构化 `RejectionRecord` 列表,
按 `retry_key`(hat + topic + reason_code)聚合。适用于冷启动后诊断历史 rejection
(U7a production wire-up,U11-T3 commit `d568437e`)。

**fallback 路径**:若 ledger 不存在(冷工作空间),降级读取 legacy `.ralph/recovery.jsonl`,
输出历史 recovery envelope(无 ledger-side 审计轨迹)。

**与 `--session` 的区别**:
- `--session <id>`:读取 `.ralph/diagnostics/<id>/` 下的 session-scoped 诊断(snapshot + recovery envelope + diagnose report)
- `--from-ledger`:读取 ledger 全局持久化 rejection log(跨 session 聚合)

**用法**:

```bash
# 查看整个工作空间的历史 rejection 聚合
ralph diagnose --from-ledger

# 与 session 模式合并(读取 session + ledger)
ralph diagnose --session latest --from-ledger
```

**输出示例**:

```text
## Ledger Rejection Summary

| retry_key | count | last_seen | last_message |
|---|---|---|---|
| executor:queue.advance:required_fields:missing | 3 | 2026-06-22T14:30:00Z | payload must contain `task_id` |
| reviewer:review.passed:semantic_gate:wave_open | 1 | 2026-06-22T14:25:00Z | wave is open; cannot pass empty diff |
```
