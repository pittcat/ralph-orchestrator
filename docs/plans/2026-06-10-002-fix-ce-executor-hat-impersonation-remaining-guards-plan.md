---
title: "fix: ce-executor hat impersonation 剩余 P1 守卫"
type: fix
status: active
date: 2026-06-10
origin: docs/report/2026-06-10-ce-executor-ralph-hat-impersonation-diagnosis.md
---

# fix: ce-executor hat impersonation 剩余 P1 守卫

## Summary

针对诊断报告 `docs/report/2026-06-10-ce-executor-ralph-hat-impersonation-diagnosis.md` 中识别的问题，经过对代码库的全面源码验证：

**已由先期 plan 修复的 P0（commit `e272808` / `524fb73` / `efc0fd0`）：**
- ✅ `EventOriginGuard` 已拒绝 `ralph` hat 发业务 topic（JSONL 读入路径）
- ✅ `topic-deny rules` + `plan_name` 锁已实现
- ✅ Partial wave dispatch + 超时保护已实现
- ✅ execution_contract 拒绝后直接发 `task.resume` + guidance（不再走 `pending→recovered` 软路径）

**本 plan 修复的 4 个仍待处理的 P1 缺陷：**

| # | 缺陷 | 验证结论 | 影响面 |
|---|---|---|---|
| U1 | `ralph emit` CLI 路径缺 hat 身份校验 | `emit_command_with_root` 只调 policy schema check，不检查 `ralph` hat 是否越权发业务 topic | emit 路径绕过 origin guard |
| U2 | Wave dispatcher worker spawn 失败无可观测性 | `execute_wave` (dispatcher.rs:283) 的 `semaphore.acquire_owned().await?` 失败直接 `?` 传播，不写 `recovery.jsonl` | 0/N dimension.done 无法归因 |
| U3 | TaskStore 创建/更新 task 无 owner_hat_id 校验 | `TaskStore::add` (task_store.rs:193) 不校验 `owner_hat_id` 是否在 `coordinator_hats` 白名单 | stall 路径上 ralph 可越权建 task |
| U4 | ce-executor preset schema 枚举 + deny rules + all-dimensions-timeout 不完整 | `skip_reason` 无 `allowed_values` 枚举；缺 `ralph` deny rule；缺 short-circuit 禁令 | 自创 `skip_reason` 通过；preset 层防守不够 |

## Problem Frame

### 真实事件链回顾（来自诊断报告）

worktree loop `smart-hawk` 执行时，`dimension-reviewer` 14 次 wave emit 后 0 响应，`ralph` hat 越权发出 `review.wave.ready` × 7、`review.passed`、`queue.advance`、`task.resume` × 2，绕过 `review-synthesizer` / `plan-gate` / `coordinator` 三道关，把 U1 推到 U2。

### 已修 P0 vs 未修 P1 分层

```
JSONL 读入路径:
  event_origin.rs validate_event_origin()  ─── ✅ 已修 ralph_control_only
  event_policy.rs validate_event()         ─── ✅ 已修 topic_deny + plan_name
  
ralph emit CLI 路径:
  emit_command_with_root()                 ─── ❌ 未修 (U1)
  
Wave dispatch:
  execute_wave()                           ─── ❌ 未修 worker spawn envelope (U2)
  
Task 存储:
  TaskStore::add()                         ─── ❌ 未修 owner_hat_id (U3)

Preset 层:
  ce-executor.yml event_policy.schemas     ─── ❌ 缺 skip_reason 枚举 (U4)
  ce-executor.yml topic_deny_rules         ─── ❌ 缺 ralph deny rule (U4)
  ce-executor.yml review-synthesizer inst  ─── ❌ 缺 all-dimensions-timeout (U4)
```

## Requirements

- **R1**: `ralph emit` 加 `--hat ralph` + 业务 topic → 硬拒绝，提示只允许 control topic
- **R2**: `execute_wave` worker spawn 失败时 → 写 `recovery.jsonl` envelope，source=`wave_dispatcher`
- **R3**: `TaskStore::create` 或 `Coordinator` 创建 task 时 → 校验 `owner_hat_id ∈ coordinator_hats`
- **R4**: ce-executor.yml 的 `review.passed` schema 加 `allowed_values.skip_reason` 枚举
- **R5**: ce-executor.yml 的 `topic_deny_rules` 加 ralph 禁止 domain topic 的精确匹配规则（`review.wave.ready` / `review.passed` / `queue.advance` / `plan.complete` / `plan.blocked`）
- **R6**: ce-executor.yml review-synthesizer instructions 加 all-dimensions-timeout 短接禁令
- **R7**: 4 个修改互不耦合，各自独立可测
- **R8**: 现有测试全绿，新增 scenario 验证新行为

## Key Technical Decisions

### KTD-1: `ralph emit` hat 校验粒度

**选项 A**: 只在 `ralph emit --policy-check` (Enforce 模式) 时校验 ralph hat
**选项 B**: 无条件校验，不管有没有 `--policy-check`

**选 B**。理由：`ralph emit` 写入 JSONL 文件后，loop runner 读取时 origin guard 会拒绝。但这个拒绝发生在**好几秒之后**——agent 以为事件发出了，loop 下一轮才开始报错。在 emit 命令本身提前拒绝，能提供即时的 backpressure。`emit_command_with_root` 中已经有 hat 字段（line 332），判断成本极低。

### KTD-2: TaskStore owner 校验位置

**选项 A**: 在 `TaskStore::add` 内部加 coordinator_hats 白名单 → 需要把 preset/coordinator_hats 传入 store
**选项 B**: 在调用方（`task_store.create_task` 的 CLI/event_loop 调用点）校验

**选 B**（分层校验）。理由：`TaskStore` 是纯存储层，不关心业务语义（`coordinator_hats` 是 preset 概念）。在创建 task 的上层（CLI 的 `ralph tools task create` + event_loop 的 coordinator 路径）校验更符合关注分离。

### KTD-3: `skip_reason` 枚举限定的边界

使用 `EventSchema.allowed_values` 机制（已在 event_policy.rs:469-487 实现），只枚举三个合法值：`empty_diff` / `trivial_step` / `aggregate_timeout`。不符合的事件在 policy 层直接 `RejectWithResume`。

## Implementation Units

### U1. `ralph emit` 增加 hat 身份校验

**Goal**: 在 `ralph emit` 命令路径上增加 hat 身份校验，阻止 `ralph` hat 发出业务 topic

**Requirements**: R1

**Dependencies**: 无

**Files**:
- `crates/ralph-cli/src/commands/emit.rs` （修改）
- `crates/ralph-core/src/event_origin.rs` （复用 `RALPH_CONTROL_TOPICS` 常量）

**Approach**:
1. 在 `emit_command_with_root` 中，解析 `hat` 后立即检查：如果 `hat == "ralph"` 且 topic 不在 `RALPH_CONTROL_TOPICS` 中，直接 `anyhow::bail!` 拒绝
2. 为了不把 `RALPH_CONTROL_TOPICS` 从 `event_origin` 模块复制出来，把它提升为 `pub` 常量，emit.rs 直接引用
3. 这个检查在 policy check 之前、也在任何 I/O 之前执行，是最轻量的 guard

执行方案：
```rust
// 在 emit_command_with_root 中，hat 解析之后，event 写入之前
if let Some(hat_id) = &hat && hat_id == "ralph" {
    if !crate::event_origin::RALPH_CONTROL_TOPICS.contains(&args.topic.as_str()) {
        anyhow::bail!(
            "Builtin ralph hat may only emit control topics: {:?}. \
             Topic '{}' is a business topic and cannot be emitted by ralph.",
            crate::event_origin::RALPH_CONTROL_TOPICS,
            args.topic
        );
    }
}
```

**Patterns to follow**: 复用 `event_origin.rs` 已有的 `RALPH_CONTROL_TOPICS` 常量和拒绝逻辑

**Test scenarios**:
1. `hat=ralph` + topic=`LOOP_COMPLETE` → 接受（control topic）
2. `hat=ralph` + topic=`human.guidance` → 接受（control topic）
3. `hat=ralph` + topic=`review.passed` → 拒绝，提示 `ralph_control_only`
4. `hat=ralph` + topic=`work.start` → 拒绝，提示 `ralph_control_only`
5. `hat=executor` + topic=`work.done` → 接受（非 ralph hat 不受限）
6. `hat=ralph` + topic=`task.resume` → 接受（contrl topic 之一）
7. 无 `--hat` 时 → 不触发 ralph check（直接通过或走其他检查）

**Verification**: `cargo test -p ralph-cli` 全绿；手动测试 `ralph emit review.passed --hat ralph` 必须硬拒绝

---

### U2. Wave dispatcher worker 启动失败写 recovery envelope

**Goal**: 当 `execute_wave` 的 worker spawn 失败时，写入 recovery.jsonl envelope，提供可观测性

**Requirements**: R2

**Dependencies**: 无

**Files**:
- `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` （修改）
- `crates/ralph-core/src/diagnosis/envelope.rs` （确认 `WaveDispatcher` 作为 `DiagnosisSource`）
- `crates/ralph-core/src/diagnosis/mod.rs` （若需要增加 source variant）

**Approach**:
1. 确认 `DiagnosisSource` 枚举是否有 `WaveDispatcher` 变体。若无，新增
2. 在 `execute_wave` 的 worker 循环中（目前 dispatcher.rs:349: `tokio::spawn`），捕获 spawn 失败
3. 当 spawn 失败时，构造 `RecoveryDiagnosisEnvelope` 并写入 diagnostics collector
4. 注意：`execute_wave` 是 cli crate 的函数，而 `RecoveryDiagnosisEnvelope` 在 ralph-core。需要把 `DiagnosticsCollector` 引用传入 `execute_wave`

**关键设计**：
- `execute_wave` 的签名可能尚未接受 `DiagnosticsCollector`，需要：
  - (a) 在 `execute_wave` 参数中增加 `Option<&DiagnosticsCollector>`；或
  - (b) 通过 `ralph_core::diagnosis::RecoveryLogger` 直接写文件
- 优先选 (b)：`execute_wave` 有 `loop_id` 和 worktree 路径，可以定位 diagnostics 目录
- 如果没有 diagnostics 目录（未启用诊断），则静默跳过

执行方案：
```rust
// execute_wave 中，spawn 循环内
let handle = match tokio::spawn(async move { ... }) {
    Ok(h) => h,
    Err(e) => {
        // 写 recovery envelope
        if let Some(logger) = &diagnostics_logger {
            let env = RecoveryDiagnosisEnvelope::builder()
                .source(DiagnosisSource::WaveDispatcher)
                .severity(DiagnosisSeverity::Error)
                .iteration(0)
                .reason_code("worker_spawn_failed")
                .message(format!("Failed to spawn wave worker {}: {}", index, e))
                .source_hat(wave.target_hat.as_str())
                .safe_target(false)
                .build();
            let _ = logger.log(&RecoveryJournalEntry::from_envelope(env, vec![]));
        }
        continue; // 跳过该 worker，继续 spawn 其余 worker
    }
};
```

**Patterns to follow**: `recovery.rs` 中的 `RecoveryLogger` 模式；`diagnosis/envelope.rs` 中已有 `source: DiagnosisSource` 字段

**Test scenarios**:
1. 正常 wave worker 全部启动成功 → 不写 recovery envelope
2. 部分 worker spawn 失败（如系统资源不足） → 写 `source=wave_dispatcher, reason_code=worker_spawn_failed` 的 envelope
3. 全部 worker spawn 失败 → 写 N 个 envelope 后返回 partial wave
4. diagnostics 未启用 → 静默跳过，不 panic

**Verification**: 模拟 spawn 失败场景（如注入虚假 semaphore 错误），验证 recovery.jsonl 包含预期 envelope

---

### U3. Task 创建时校验 owner_hat_id

**Goal**: 阻止非 `coordinator_hats` 列表中的 hat 创建/更新 task

**Requirements**: R3

**Dependencies**: 无

**Files**:
- `crates/ralph-cli/src/commands/task_cli.rs` （修改，`add_task_with_args` 加校验）
- `crates/ralph-core/src/task_store.rs` （可选，加 `validate_owner_hat` 辅助方法）

**Approach**:
1. `coordinator_hats` 已经在 `task_cli.rs:477` 和 `:522` 被传入并注释为 `reserved for future use` —— 基础设施已就绪，只缺实现
2. 在 `add_task_with_args`（L482）中，`store.add()` 之前，加校验：从 `coordinator_hats` 检查 `owner_hat_id`（已由 `add_common_task_fields` 自动从 `RALPH_CURRENT_HAT` 填入，见 L285-286）
3. 拒绝时返回清晰的错误信息（参照 L267-272 已有的 close 路径校验风格）
4. 不作 `event_loop/mod.rs` 的修改——目前 coordinator 创建 task 走的是 CLI 路径（event_loop 代理通过 `ralph tools task add` 调用），cli 路径已覆盖

具体修改位置：
```rust
// add_task_with_args (L482-509)，在 store.add(task.clone()) 之前
if let Some(owner) = task.owner_hat_id.as_deref() {
    if !coordinator_hats.iter().any(|h| h == owner) {
        bail!(
            "owner_hat_id '{}' is not in coordinator_hats. Allowed: {:?}",
            owner,
            coordinator_hats
        );
    }
}
```

**具体方案**：

```rust
// 校验函数（可放在 task.rs 或 task_definition.rs）
pub fn validate_owner_hat(
    owner_hat_id: &str,
    coordinator_hats: &[String],
) -> Result<(), String> {
    if coordinator_hats.contains(&owner_hat_id.to_string()) {
        Ok(())
    } else {
        Err(format!(
            "owner_hat_id '{owner_hat_id}' is not in coordinator_hats. \
             Allowed: {:?}",
            coordinator_hats
        ))
    }
}
```

**Patterns to follow**: presets/en/ce-executor.yml:35-42 已有的 `coordinator_hats` 定义

**Test scenarios**:
1. `owner_hat_id=coordinator` + `coordinator_hats=[coordinator, executor]` → 接受
2. `owner_hat_id=ralph` + `coordinator_hats=[coordinator, executor]` → 拒绝
3. `owner_hat_id=executor` + `coordinator_hats=[coordinator, executor]` → 接受
4. `coordinator_hats` 为空列表 → 所有 owner 拒绝（fail-closed）
5. `coordinator_hats` 未配置 → 默认拒绝（fail-closed）
6. 现有 `ralph tools task create --owner ralph` 的集成测试应验证被拒绝

**Verification**: `cargo test -p ralph-cli -p ralph-core` 全绿；手动模拟 ralph 建 task 应被拒绝

---

### U4. ce-executor preset schema 枚举 + deny rule + all-dimensions-timeout 兜底

**Goal**: 加强 preset 层防守；补 `skip_reason` 枚举、补 `ralph` deny rule、补 review-synthesizer short-circuit 禁令

**Requirements**: R4, R5, R6

**Dependencies**: 无

**Files**:
- `presets/en/ce-executor.yml` （修改，3 处）

**Approach**:
三处修改，完全独立：

**1. review.passed schema 加 `allowed_values.skip_reason` 枚举**（L129-134）：

```yaml
review.passed:
  required_fields: [plan_name, task_id, task_key, step, findings_count, fix_round, verdict, skip_reason]
  payload: json_object
  allowed_values:
    skip_reason: ["empty_diff", "trivial_step", "aggregate_timeout"]
```

**2. topic_deny_rules 增加 ralph 禁止规则**（L103-104）：

```yaml
topic_deny_rules:
  - {hat_id: executor, topic: build.done}
  - {hat_id: ralph, topic: review.wave.ready}
  - {hat_id: ralph, topic: review.passed}
  - {hat_id: ralph, topic: queue.advance}
  - {hat_id: ralph, topic: plan.complete}
  - {hat_id: ralph, topic: plan.blocked}
```

注意：需要精确主题匹配，不支持 glob。每条规则写全名。

**3. review-synthesizer instructions 加 all-dimensions-timeout 禁令**：

在 review-synthesizer hat 的 instructions 中增加硬规则。找到 review-synthesizer 的 `Decision Logic` 段（诊断报告指出约 L818），追加：

```yaml
### All-Dimensions-Timeout 守则（硬规则）
If aggregate `wait_for_all` reaches timeout (300s) without ALL dimension.done events:
  - DO NOT emit `review.passed` under any circumstance
  - DO NOT invent a `skip_reason`
  - DO NOT short-circuit the review
  - Emit `plan.blocked` with: reason="dimension_reviewers_failed_to_converge", details=<missing dimensions>
  - This routes to shipper → REVIEW_COMPLETE with pass_or_fail=fail
```

**Test scenarios**：
1. `ralph emit review.passed --hat review-synthesizer --json --payload '{"skip_reason":"empty_diff",...}'` + policy check → 接受
2. `ralph emit review.passed --hat review-synthesizer --json --payload '{"skip_reason":"dimension_reviewer_no_response",...}'` + policy check → 拒绝（枚举不符）
3. `ralph emit review.passed --hat review-synthesizer --json --payload '...,"skip_reason":""}'` + policy check → 拒绝
4. topic_deny_rules 生效验证：`ralph emit review.wave.ready --hat ralph` → 被 policy 拒绝（topic_denied）
5. 回归：`ralph emit build.done --hat executor` → 被 topic_deny 拒绝（已有规则，确保不退化）
6. 回归：`ralph emit work.done --hat executor --policy-check` → 正常接受

**Verification**: `cargo test` 全绿；手动用 `ralph emit` 测试枚举拒绝和 deny rule 拒绝

---

### 未纳入本计划的项

以下为诊断报告提及但判定为**不需要代码修复**或**已由其他 plan 覆盖**的项：

| 诊断项 | 判定 | 理由 |
|---|---|---|
| execution_contract fail-soft（5.1.2） | ✅ 已修 | 当前代码直接 publish task.resume + guidance，不走 pending→recovered |
| stall_recovery safe_target（5.1.4） | ⏸ 推迟 | 当前 stall_recovery 通过 event_loop::inject_fallback_event 定位 `last_hat`，已有效避免 ralph 回退。深度预设感知需跨模块改造，留待后续 |
| R4 隔离/ R5 worktree_path（5.2.4） | ✅ 已有 plan | 由 active plan `2026-06-10-001` 处理 |
| 主仓 events.jsonl 分离 | P2 可选 | 不影响行为安全 |
| scratchpad short-circuit 策略 | P2 | 已有 U4 #3 的 preset instructions 禁止 |

## Test Plan

除各 U 自带的单元测试外，新增以下集成/BDD 场景：

1. **U1+rpc**: `ralph emit` 拒绝 ralph 发 business topic 的端到端测试
2. **U2+rpc**: Wave dispatcher 失败时 recovery.jsonl 包含 envelope（用 mock 模拟 spawn 失败）
3. **U3+rpc**: `ralph tools task create --owner ralph` 被拒
4. **U4+rpc**: `ralph emit review.passed --json --payload '{"skip_reason":"bad_value",...}'` 被 schema 枚举拒
5. **U4+rpc**: `ralph emit review.wave.ready --hat ralph` 被 topic_deny 拒
6. 回归：现有 BDD scenarios 全部通过

## Source Files & Research

- **诊断报告**: `docs/report/2026-06-10-ce-executor-ralph-hat-impersonation-diagnosis.md`
- **EventOriginGuard**: `crates/ralph-core/src/event_origin.rs`（已含 `RALPH_CONTROL_TOPICS` 常量）
- **ralph emit 命令**: `crates/ralph-cli/src/commands/emit.rs`（`emit_command_with_root` 为中心）
- **Wave dispatcher**: `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs`（`execute_wave`）
- **TaskStore**: `crates/ralph-core/src/task_store.rs`
- **Task CLI**: `crates/ralph-cli/src/commands/task_cli.rs`
- **EventPolicy**: `crates/ralph-core/src/event_policy.rs`（`allowed_values` 已支持枚举）
- **Recovery 日志**: `crates/ralph-core/src/diagnostics/recovery.rs`
- **诊断 envelope**: `crates/ralph-core/src/diagnosis/envelope.rs`
- **Preset**: `presets/en/ce-executor.yml`
- **先期修复约定**: `docs/solutions/ce-executor-task-ownership.md`
- **先期修复约定**: `docs/solutions/ce-executor-wave-emit-policy.md`
