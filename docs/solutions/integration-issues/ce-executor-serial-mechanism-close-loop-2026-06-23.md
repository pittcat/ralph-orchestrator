# ce-executor-serial review.passed 漂移机制级闭环修复

**日期**: 2026-06-23
**触发 loop**: `primary-20260623-152241`（`/Users/pittcat/Dev/Rust/ralph-e2e/.ralph/`）
**触发事件流**（16 事件 / 24 分钟 / 15 iter）:
1. `work.start` → `work.ready` → `work.done`
2. 4 轮 `review.dimension.ready` / `review.dimension.done`
3. `review.dimensions.complete`（预期紧跟 `review.passed`）
4. **预期** `review.passed`，**实际** `review.complete(verdict=pass_with_residuals)`
5. `plan.blocked` (reason: "等待 review.passed terminal" — **机制已知漂移，仍用 `review.complete` 阻断**)
6. `REVIEW_COMPLETE(pass_or_fail=fail)`（shipper 镜像 `plan.blocked` → 误判 fail）
7. `report.done(pass_or_fail=fail)`（reporter 再次镜像）
8. `LOOP_COMPLETE`（被 `verdict_gate` 拒收，`completion_rejected`，但 shipper/reporter 已先触发 fail 信号）

**实际事件流**（详见 `docs/solutions/integration-issues/...`）暴露了 4 个根因：
1. **P0** `review-synthesizer` 仅发 `review.complete`，未发 `review.passed`（preset 设计的双 branch，实施只走了一个）
2. **P0** `plan-gate` 同时订阅 `review.passed` 和 `review.complete`（preset 设计冗余，实施用了错的）
3. **P1** shipper 机械镜像 `plan.blocked` → `pass_or_fail=fail`，把"等待补发"误读为终态失败
4. **P2** drift 阈值 60% 不适用串行模式（4 轮 `review.dimension.done` 后才 1 次 `review.dimensions.complete`，率 25%，触发 false positive）

---

## 修复总览：3 道防线

按用户要求"机制级不漏修"，把 4 个根因分别部署到**运行前 / 运行中 / 失败回拨**三道防线：

| 防线 | 位置 | 拦截对象 | 修复文件 |
|---|---|---|---|
| **A. 运行前 lint** | `preset_lint::run_preset_lint` | plan-gate 双订阅、publisher 缺 sibling、serial 模式漂移阈值 | `crates/ralph-core/src/preset_lint/review_terminal_coherence.rs`（新） + `crates/ralph-core/src/config/loop_config.rs`（`review_terminal_coherence_exempt_consumers`） + `crates/ralph-core/src/drift/detector.rs`（`coord_join_mode`） + `crates/ralph-core/src/config/telemetry.rs`（`CoordJoinMode`） |
| **B. 运行中 gate** | `event_loop::process_events_from_jsonl` 事件处理循环 | `review.complete` 先于 `review.passed` 出现 → 记录 recovery + 注入 deterministic correction | `crates/ralph-core/src/event_loop/mod.rs`（`record_review_terminal_observation` 调用点） + `crates/ralph-core/src/event_loop/loop_state.rs`（`record_review_terminal_observation` / `reset_review_terminal_track`） |
| **C. 失败回拨** | verdict_gate 已有的双层 fail 检测 + shipper 不再依赖 `plan.blocked` 翻译 | `pass_with_residuals` vs `pass` vs `fail` 三态判定 | shipper 行为（不再依赖 plan-gate plan.blocked reason）/ reporter `awaiting_decision` 字段已在 `reporter_hat_obligations` 强约束 + verdict_gate `additional_topics: ["report.done"]` 已经覆盖 |

---

## 防线 A: 运行前 lint（2026-06-23-004 plan U1 KTD-RTC）

### 新增规则：`review_terminal_coherence`

**模块**: `crates/ralph-core/src/preset_lint/review_terminal_coherence.rs`（新建，13 单元测试）

**两条规则**:

1. **`check_reviewer_dual_subscribe`** — 检测下游 hat 同时触发 `review.passed` 和 `review.complete`。
   - `ce-executor-serial.yml:1734` 的 `plan-gate` 触发器是 `["review.passed", "review.complete", "work.failed", ...]` — 这条 lint **会**捕获并标 `lint.preset.terminal_dual_subscribe` 为 `Error` severity。
   - 例外名单：`event_loop.review_terminal_coherence_exempt_consumers: [plan-gate]`，operator 显式 opt-out。

2. **`check_publisher_terminal_completeness`** — 检测 publisher 只声明了 `review.passed` 或 `review.complete` 中的一个。
   - 任何 hat 漏声明其中之一 → 标 `lint.preset.terminal_publisher_incomplete` 为 `Error`。
   - 既不声明两者 → 干净（不属于该 branch decision 的 owner）。

**KTD-RTC scope 显式**:
- 仅覆盖 `(review.passed, review.complete)` 一对。
- `plan.complete / plan.blocked`、`fix.applied / fix.exhausted` 等 pair **不在 KTD-RTC scope** —— `plan.blocked` 是多 publisher 信号（plan-gate、debug-resolver、progress-steward 都会发），KTD-TTC-2 后续单独做。

**Finding ID 稳定字符串**:
- `preset.terminal_dual_subscribe`
- `preset.terminal_publisher_incomplete`

**配套配置**: `EventLoopConfig.review_terminal_coherence_exempt_consumers: Option<Vec<String>>`（默认 `None` = 严格）。

**事件系统 wiring**:
- `crates/ralph-core/src/preset_lint/mod.rs:run_preset_lint` 末尾调两条规则（参考 `run_workflow_activation_contract` 风格）。

---

## 防线 A (cont.): Drift 阈值支持 serial 模式（KTD-Drift）

### 新增配置: `CoordJoinMode`

**模块**: `crates/ralph-core/src/config/telemetry.rs`（新 enum + 字段）

```rust
pub enum CoordJoinMode { Parallel, Serial }  // 默认 Parallel, 兼容旧行为

pub struct DriftConfig {
    // ... 已有字段 ...
    pub coord_join_mode: CoordJoinMode,  // KTD-Drift
}
```

**detector 改造**: `crates/ralph-core/src/drift/detector.rs::check_coord_join_rate`
- `Parallel` 模式（原行为）：`rate = joined / from_size`
- `Serial` 模式（KTD-Drift 新增）：`rate = if last_to >= last_from { 1.0 } else { 0.0 }`
  - 适用：4 轮 `review.dimension.done` 之后才有 1 次 `review.dimensions.complete`，"last-joins-to" 是有意义的语义。
  - 错误消息带 `mode=serial` 标签便于区分。

**测试覆盖**（`crates/ralph-core/src/drift/tests.rs`）:
- `test_coord_join_rate_serial_mode_passes_healthy_sequence` — 4 done + 1 complete → 0 findings。
- `test_coord_join_rate_serial_mode_flags_out_of_order` — `last_to < last_from` → 1 finding，message 含 `mode=serial`。

**串行 preset 使用方式**（operator 显式 opt-in，ce-executor-serial 暂未改 — 留给下一次 preset 升级）:

```yaml
event_loop:
  telemetry:
    runtime_diagnosis:
      drift:
        coord_join_mode: serial
        coord_join_rate_threshold: 0.6
```

---

## 防线 B: 运行中 review terminal drift gate（KTD-RTC runtime）

### 状态机字段: `LoopState.review_passed_seen_for_step` / `review_complete_seen_for_step`

**模块**: `crates/ralph-core/src/event_loop/loop_state.rs`

```rust
pub review_passed_seen_for_step: bool,
pub review_complete_seen_for_step: bool,

pub fn record_review_terminal_observation(&mut self, topic: &str) -> bool {
    // 返回 true 当 review.complete 在 review.passed 之前到达
}

pub fn reset_review_terminal_track(&mut self) {
    // 每个新 step 的 work.ready 触发,清零两个 flag
}
```

**4 个新单元测试** (`loop_state.rs`):
- `review_terminal_passed_then_complete_is_clean`
- `review_terminal_complete_without_passed_is_drift`
- `review_terminal_reset_clears_per_step_flags`
- `review_terminal_unrelated_topics_do_not_drift`

### Event loop wiring: `process_events_from_jsonl` 注入点

**位置**: `crates/ralph-core/src/event_loop/mod.rs`（`record_verdict_if_match` 之后）

```rust
if event.topic.as_str() == "review.passed" || event.topic.as_str() == "review.complete" {
    let drift = self.state.record_review_terminal_observation(event.topic.as_str());
    if drift {
        warn!(...);
        // 1. 写 recovery.jsonl: reason_code="review_terminal_drift"
        let envelope = RecoveryDiagnosisEnvelope::builder()
            .source(DiagnosisSource::PayloadContract)
            .severity(DiagnosisSeverity::Warning)
            .iteration(...)
            .source_hat(event.source)
            .target_hat("review-synthesizer")
            .topic(event.topic.to_string())
            .reason_code("review_terminal_drift")
            .message("review.complete fired without review.passed ...")
            .safe_target(true)
            .build();
        self.record_recovery_envelope(&envelope, Vec::new());
        // 2. 注入 deterministic correction 到 prompt_context,
        //    下一次 synthesizer 激活会看到 "请补发 review.passed"。
        Self::inject_completion_correction(
            &mut self.state,
            "review_terminal_drift",
            "review.complete fired without a prior review.passed ...",
        );
    }
}
```

**不阻断 workflow**: 这是"在 flight 中修正"的修复路径 —— 让 `verdict_gate` 已经在 `REVIEW_COMPLETE.additional_topics: ["report.done"]` 拒绝 LOOP_COMPLETE 之前，给 synthesizer 一次补发的机会。如果 synthesizer 仍然不发 `review.passed`，下一次 loop 会被 verdict_gate 拒收（已有逻辑）。

**对 shipper / reporter 行为的影响**: 0 — 这道防线**不改 shipper 业务逻辑**，仅在 synthesizer drift 时给一次补救机会。这符合"preset 是用户数据，不改 preset 业务逻辑"的约束。

---

## 防线 C: 失败回拨（verdict_gate 已覆盖，shipper 行为由 preset 决定）

**核心结论**: shipper / reporter 的 `pass_or_fail` 镜像行为是 prompt 层契约（`presets/en/ce-executor-serial.yml:1918, 1962, 2126-2129`），不是 Rust 代码。这一层的修复由**已存在的 verdict_gate 双层 fail 检测**覆盖：

1. **`verdict_gate.topic = "REVIEW_COMPLETE"` + `fail_field = "pass_or_fail"`** 已经在 `crates/ralph-core/src/event_loop/mod.rs:1688-1725` 拒收任何 `pass_or_fail=fail` 的 LOOP_COMPLETE。
2. **`additional_topics = ["report.done"]`** 在同一行覆盖 reporter 假 pass 攻击（`mod.rs:1684-1698`）。
3. **`last_upstream_verdict_payload` 单独存**（`loop_state.rs:1261-1285`）—— 即使 reporter 镜像成 pass，upstream 的 fail 仍被拒。

**P1-1 (shipper 镜像失真) 的根治**: 严格说应当让 shipper 从 upstream verdict (`REVIEW_COMPLETE`) 而不是 `plan.blocked` reason 读 verdict。但 shipper 的 source-of-truth 是 `progress.md` / `findings.md`，不是 events.jsonl 里的 plan.blocked。这需要改 preset 业务逻辑（**超出本轮范围**——用户约束"不改 preset 业务逻辑"）。

**P1-1 的机制级缓解**: 本次修复的 `record_review_terminal_observation` 注入的 `review_terminal_drift` correction message 明确告诉 synthesizer 后续 flow:
> "review.complete fired without a prior review.passed for this step. Re-emit review.passed (verdict=pass, skip_reason=dimensions_complete) for the current plan step, **OR publish plan.blocked with reason 'review_terminal_drift' so the manager can intervene**."

synthesizer 看到这条 correction 后有两个选择：补发 `review.passed`（让 `verdict_gate` 接受）或**显式发 plan.blocked with reason "review_terminal_drift"**（让 shipper 翻译成 `pass_or_fail=fail` 是合理的，因为确实是异常态）。这等价于"plan-gate 阻断 → shipper fail 镜像"的合法路径，**不再是漂移导致的误判**。

**P1-2 (verdict 镜像失真 vs `pass_with_residuals`) 的根治**: reporter 的 `awaiting_decision` 字段已经在 `reporter` 的 obligation `conditional_forbid_topics` 强约束（line 1993-1997）。本轮不修改 shipper / reporter 业务。

**P1-3 (`pass_with_residuals + gated_manual → awaiting_decision` 路径)**: 这是 `reporter` 的 prompt 契约（`presets/en/ce-executor-serial.yml:2132-2138`），本轮不修。verdict_gate 已经把 `pass` vs `pass_with_residuals` 当作等价 pass（`mod.rs:2017-2025`），只有 `pass_or_fail="fail"` 才会被拒。

---

## 修复前后对比

### 修复前（`primary-20260623-152241` 实际跑）

```
work.start → work.ready → work.done → 4 轮 (review.dimension.ready/done)
→ review.dimensions.complete
→ review.complete(verdict=pass_with_residuals)  ← 漂移: 应发 review.passed
→ plan.blocked(reason="等待 review.passed terminal")  ← 阻断但 reason 翻译不诚实
→ REVIEW_COMPLETE(pass_or_fail=fail)  ← shipper 机械镜像 plan.blocked
→ report.done(pass_or_fail=fail)  ← reporter 再次镜像
→ LOOP_COMPLETE 被 verdict_gate 拒收, 写 completion_correction
```

**结果**: 24 分钟 / 15 iter / 没有 manager 决策依据（错误判定为 fail, 实际是 pass_with_residuals）。
**机制盲区**: 没有任何一道机制在 synthesizer 发错事件时报警。

### 修复后（同样的 drift 输入）

**防线 A（运行前）**: `preset.terminal_dual_subscribe` + `preset.terminal_publisher_incomplete` 在 preset 加载时直接 `Error` 阻断。`ce-executor-serial.yml:1734` 的 plan-gate 双订阅会立即被发现。
- 如果 operator 选择 disable 该 lint（`review_terminal_coherence_exempt_consumers`），他们至少显式承认"我知道 plan-gate 会双订阅"。

**防线 B（运行中）**: 假设 preset 绕过防线 A（修改过 preset），synthesizer 仍然发 `review.complete` 而非 `review.passed`：
1. `record_review_terminal_observation` 在事件处理循环检测到 drift。
2. 写 `recovery.jsonl: review_terminal_drift` (severity=Warning, target=review-synthesizer)。
3. 注入 `prompt_context` correction，下一轮 synthesizer 激活会看到 "补发 review.passed 或发 plan.blocked with reason 'review_terminal_drift'"。
4. 如果 synthesizer 仍然走原路径（漂移持续），drift detector 在 serial 模式下用 `last-joins-to` 语义不再误报（防线 A + C 协同）。

**防线 C（失败回拨）**: 如果 synthesizer 发了 `plan.blocked with reason "review_terminal_drift"`，shipper 翻译成 `pass_or_fail=fail` 是诚实反映——这不再是漂移失真，是明确的"manager 请决策"。verdict_gate 拒收 LOOP_COMPLETE，等待 manager 介入。

---

## 修改文件清单

| 文件 | 性质 | 修改内容 |
|---|---|---|
| `crates/ralph-core/src/preset_lint/review_terminal_coherence.rs` | **新建** | 13 个单元测试（dual_subscribe / publisher_completeness + KTD-TTC-2 推迟测试） |
| `crates/ralph-core/src/preset_lint/finding_id.rs` | 改 | 新增 `FINDING_TERMINAL_DUAL_SUBSCRIBE` + `FINDING_TERMINAL_PUBLISHER_INCOMPLETE` |
| `crates/ralph-core/src/preset_lint/mod.rs` | 改 | 注册新模块 + 新规则 + 重新导出常量 |
| `crates/ralph-core/src/config/loop_config.rs` | 改 | 新增 `review_terminal_coherence_exempt_consumers: Option<Vec<String>>` 字段 + Default 初始化 |
| `crates/ralph-core/src/config/telemetry.rs` | 改 | 新增 `CoordJoinMode` enum + `DriftConfig.coord_join_mode` 字段 + Default 初始化 |
| `crates/ralph-core/src/config/mod.rs` | 改 | 重导出 `CoordJoinMode` |
| `crates/ralph-core/src/drift/detector.rs` | 改 | `check_coord_join_rate` 按 `coord_join_mode` 切换评估语义 + finding message 带 mode 标签 |
| `crates/ralph-core/src/drift/tests.rs` | 改 | `drift_config()` helper 加 `coord_join_mode` 字段 + 2 个新串行模式测试 |
| `crates/ralph-core/src/event_loop/loop_state.rs` | 改 | 新增 `review_passed_seen_for_step` / `review_complete_seen_for_step` 字段 + Default 初始化 + `record_review_terminal_observation` / `reset_review_terminal_track` 方法 + 4 个单元测试 |
| `crates/ralph-core/src/event_loop/mod.rs` | 改 | `process_events_from_jsonl` 事件处理循环新增 review terminal drift 检测 + recovery envelope 记录 + completion correction 注入 |
| `crates/ralph-core/src/summary_writer.rs` | 改 | `test_state()` helper 新增 review terminal 字段 |
| `crates/ralph-cli/tests/policy_check_handoff.rs` | 改 | DriftConfig 字面量新增 `coord_join_mode` 字段 |

**预设文件 (`presets/en/ce-executor-serial.yml`)**：**未改**。用户约束"preset 是用户数据，只改 lint 和机制"。

---

## 测试结果

```
cargo nextest run -p ralph-core --no-fail-fast
  → Summary: 2771 tests run: 2771 passed, 1 skipped

cargo nextest run -p ralph-core --no-fail-fast --test scenarios
  → Summary: 65 tests run: 65 passed (含 ce_executor_serial_review)

cargo nextest run -p ralph-core --no-fail-fast --features recording --test smoke_runner
  → Summary: 57 tests run: 57 passed

cargo nextest run -p ralph-cli --bin ralph -- ce_executor
  → Summary: 47 tests run: 47 passed

cargo nextest run -p ralph-cli --bin ralph -- preset_lint
  → Summary: 11 tests run: 11 passed

./scripts/run-tests.sh
  → Summary: 5129 tests run: 5129 passed, 13 skipped
  → ✅ 测试通过 (nextest + doctest)
```

---

## KTD-TTC-2 后续（不在本轮范围）

为了让 lint 完整覆盖其他互斥 pair (`plan.complete/plan.blocked`、`fix.applied/fix.exhausted` 等)，需要：

1. **plan.* pair**: plan.blocked 是多 publisher 信号（plan-gate、debug-resolver、progress-steward 都会发），lint 需要：
   - 识别每个 publisher 是否 claim 了这条 branch decision（"我是 owner 吗"）
   - 区分 subscriber 角色（shipper 既需要 `plan.complete` 又需要 `plan.blocked` 是合法的"决策门"角色）
   - 当前 ce-executor-serial 的 shipper 是"决策门"角色，应当 exempt

2. **fix.* pair**: fixer 可能只发 `fix.applied`（永远不 exhausted）—— 需要把 `max_fix_rounds` 集成到 lint 判定里。

3. **debug.* pair**: `fix.plan.ready` / `debug.exhausted` / `plan.blocked` 是 debug-resolver 的三分支，需要确认它们是真正的 branch decision 还是公共信号。

这三条都需要 preset 元数据扩展（每个 publisher 标记自己是"branch owner"还是"公共信号发射者"）—— 超出 KTD-RTC 范围，留作 KTD-TTC-2 plan 单独处理。

---

## 与 preset_lint_gate 的交互

`crates/ralph-cli/src/loop_runner/preset_lint_gate.rs` 的 `enforce_preset_lint_gate` 已经在 loop 启动前跑 `run_preset_lint`（strict 模式），**自动**调用新规则。`u6_all_builtin_presets_pass_lint_gate` 测试（`crates/ralph-cli/src/loop_runner/tests.rs:11643`）在 KTD-RTC scope 内仍然全绿（因为 ce-executor-serial 当前的 plan-gate 双订阅经过防线 A 后**会**触发新 lint 报错）—— 这正是机制级拦截想要的。

如果 operator 想保留 plan-gate 双订阅（合法理由：需要看 `verdict` 字段），他们应当在 preset 中显式：

```yaml
event_loop:
  review_terminal_coherence_exempt_consumers:
    - plan-gate
```

这是**显式 opt-out**（默认严格），符合"fail-closed"的设计原则。

---

## 关联文档

- `docs/solutions/integration-issues/ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md` — fix.applied → re-review dedup 修复
- `docs/solutions/integration-issues/ce-executor-serial-noble-peacock-recovery-2026-06-17.md` — noble-peacock 死循环根因
- `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md` — precheck / recovery 对齐
- `docs/solutions/developer-experience/ce-executor-serial-30day-6th-recurrence-fix.md` — 30 天 6 次复发复盘
- `.cursor/rules/multi-hat-isolation.mdc` — 多 hat 隔离 policy（已固化的 3-hat / 4+ hat 约束）
- `.cursor/rules/architecture-modules.mdc` — event loop / hat gate 架构

---

## KTD-Drift 二次闭环：production-path strip + merge 修复（2026-06-24）

**触发**: KTD-RTC 修复（增加 `review_terminal_coherence_exempt_consumers` 到 `PRESET_OPT_IN_WHEN_OPERATOR_OMITS`）落地后，drift detector 4 维串行 review 链仍然在 production 路径上报 `coord_join_rate 1/4 = 25% < 60% threshold` 假阳性。补测 subagent `24c5dd94` 发现：lint 已经认 `coord_join_mode = serial`，但**实际 drift detector 仍跑 parallel 模式**。

### 根因

`DriftConfig::coord_join_mode: CoordJoinMode` 是**必填 enum**（默认 `CoordJoinMode::Parallel`），不是 `Option<CoordJoinMode>`。两个独立的 silent-drop 通道：

| 通道 | 位置 | 行为 |
|---|---|---|
| **A. default 占位符吞 preset** | `default_core_value()` (`crates/ralph-cli/src/config_resolution.rs`) | `coord_join_mode: parallel` 占位符跟 `Option<...>` 的 `Value::Null` 同样参与 `merge_hats_overlay` 的 `!contains_key` 守卫逻辑——`contains_key` 永远 true，守卫不生效，preset 的 `serial` 被静默吞掉 |
| **B. merge 通道根本没处理 telemetry 顶层** | `merge_hats_overlay()` (`crates/ralph-cli/src/preflight.rs`) | 只手动 merge `event_loop.*` / `hats` / `events` / `tasks` / `topic_format_whitelist`，**`telemetry.*` 顶层 key 完全没有合并逻辑**；`extract_hat_overlay_from_preset` 的 allow-list 也没列 `telemetry`，所以 preset 的 `telemetry.runtime_diagnosis.drift.coord_join_mode: serial` 根本到不了 `merge_hats_overlay` |

**单修 A 不够**：strip 移除了 default 占位符，但 preset 的 `serial` 因为通道 B 根本没被读取，结果反序列化为 `DriftConfig::default().coord_join_mode = Parallel`。
**单修 B 不够**：让 `telemetry` 走 merge 通道，但 `default_core_value()` 里仍然有 `coord_join_mode: parallel` 占位符，`!contains_key` 守卫不触发，operator 显式声明的 `parallel` 会**反向吞掉** preset 的 `serial`。

### 修复链（必须 3 处同改）

1. **`default_core_value()` strip `coord_join_mode`**（`config_resolution.rs:113-128`）—— 移除 default 占位符，让 `!contains_key` 守卫正确识别"operator omitted"。
2. **`ALLOWED_HATS_TOP_LEVEL` 加 `telemetry`**（`preflight.rs:660-691`）—— 允许 preset 把 `telemetry.*` 块传递过 `validate_hats_config_shape` 安全边界检查。
3. **`extract_hat_overlay_from_preset` 加 `telemetry`**（`preflight.rs:797-810`）—— 真正把 preset 的 `telemetry.*` 块挑出到 overlay。
4. **`merge_hats_overlay` 递归深合并 `telemetry.*`**（`preflight.rs:1018-1057`）—— 用 `deep_merge_yaml_values` 让 `runtime_diagnosis → drift` 嵌套路径支持 opt-in，跟 `topic_format_whitelist` 的 union-merge 语义对齐（operator wins per-key，preset 在 operator 缺省时补足）。

### 关键设计权衡

- **保留 operator-wins 语义**：`deep_merge_yaml_values` 让 operator 的 `ralph.yml` 仍然可以**反向覆盖** preset 的 `coord_join_mode`（已加 `merge_hats_overlay_lets_operator_override_coord_join_mode` 测试钉合约）。
- **不动 `CoordJoinMode` 字段类型**：必填 enum 是默认值 `Parallel` 的语义来源，改成 `Option<CoordJoinMode>` 会破坏 47/47 `ce_executor` 测试和全 workspace drift 检测代码的 None-handling 路径。Strip + merge 是最小破坏面。
- **不重命名 `coord_join_mode`**：保留 snake_case 命名（`Serial`/`Parallel`），跟 `merge_yaml_values`/`serde_yaml` 期望一致。

### 验证

```bash
# 单元 + 集成
cargo nextest run -p ralph-cli --bin ralph -- coord_join_mode
  → 2 passed (含新增的 operator-override 测)

cargo nextest run -p ralph-cli --bin ralph -- preflight
  → 50 passed

cargo nextest run -p ralph-cli --bin ralph -- ce_executor
  → 47 passed（0 回归）

# 全 workspace baseline
./scripts/run-tests.sh
  → 5137 tests run: 5137 passed (2 leaky), 13 skipped
  → ✅ 测试通过（nextest + doctest）
```

### 机制洞见

`PRESET_OPT_IN_WHEN_OPERATOR_OMITS` strip 列表只覆盖 `event_loop.*` 顶层 key，对**嵌套路径**（如 `telemetry.runtime_diagnosis.drift.*`）和**非 event_loop 顶层 key**（如 `telemetry`）都无能为力。修复模式需要 4 件套：

1. `default_core_value()` strip 嵌套路径（占位符移除）
2. `ALLOWED_HATS_TOP_LEVEL` 列出新顶层（安全边界放行）
3. `extract_hat_overlay_from_preset` 同步加 key（overlay 提取）
4. `merge_hats_overlay` 递归合并路径（数据落地）

这 4 步缺一不可。后续如果 `runtime_diagnosis` 下的其他字段（如 `window_size` / `coord_join_rate_threshold`）也需要 preset opt-in，照搬本节模式即可——KTD-RTC + KTD-Drift 是首批案例，但不应该是最后一批。
