# ce-executor-serial review.passed 漂移双轮 review + 修复报告

**日期**: 2026-06-24
**review 类型**: 根因审查(5 维度) + 对抗性审查(8 维度)
**目标**: 验证 2026-06-23 修复 agent(bbee0c47)的 3 道防线是否治愈根因
**产出**: 1 个 P0 + 1 个 P1 已修复;2 个 P1 判定为设计意图保留;2 个 P2 留作 follow-up

---

## 1. 机制层修复回顾(来自 bbee0c47 transcript)

### 3 道防线

| 防线 | 位置 | 拦截对象 |
|---|---|---|
| A. 运行前 lint | `preset_lint::check_reviewer_dual_subscribe` + `check_publisher_terminal_completeness` + `CoordJoinMode` | plan-gate 双订阅 / publisher 缺 sibling / serial 漂移阈值 |
| B. 运行中 gate | `event_loop::process_events_from_jsonl` 的 `record_review_terminal_observation` | `review.complete` 先于 `review.passed` |
| C. 失败回拨 | 已有的 `verdict_gate` + `additional_topics:["report.done"]` | verdict 镜像失真 |

### 修复文件清单(来自修复 agent)

| 文件 | 性质 |
|---|---|
| `crates/ralph-core/src/preset_lint/review_terminal_coherence.rs` | 新建(13 单元测试) |
| `crates/ralph-core/src/preset_lint/finding_id.rs` | 改(2 常量) |
| `crates/ralph-core/src/preset_lint/mod.rs` | 改(注册新模块) |
| `crates/ralph-core/src/config/loop_config.rs` | 改(`exempt_consumers` 字段) |
| `crates/ralph-core/src/config/telemetry.rs` | 改(`CoordJoinMode` + `DriftConfig.coord_join_mode`) |
| `crates/ralph-core/src/config/mod.rs` | 改(重导出) |
| `crates/ralph-core/src/drift/detector.rs` | 改(`check_coord_join_rate` 按 mode 切换) |
| `crates/ralph-core/src/drift/tests.rs` | 改(helper + 2 串行测试) |
| `crates/ralph-core/src/event_loop/loop_state.rs` | 改(2 字段 + 2 方法 + 4 测试) |
| `crates/ralph-core/src/event_loop/mod.rs` | 改(事件循环 + recovery envelope + correction) |
| `crates/ralph-core/src/summary_writer.rs` | 改(test_state helper) |
| `crates/ralph-cli/tests/policy_check_handoff.rs` | 改(DriftConfig 字面量) |
| `docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` | 新建 |

---

## 2. 阶段 1:根因审查结果(5 维度)

| 维度 | 结论 | 备注 |
|---|---|---|
| 1. 根因定位 | **WARN** | 防线 A 治本(静态 lint 阻止结构错位),防线 B 治标(运行时补救),但**防线 B 的 per-step 状态机未接线** — 见 P0 |
| 2. 同类隐患扫描 | **WARN** | `check_publisher_terminal_completeness` 在原版会**误报**所有非 owner hat;P1 收窄后解决 |
| 3. 历史模式对照 | **PASS** | 与 noble-peacock 死循环、perky-maple guidance 风暴等历史反模式对比,本次机制层修复思路一致(显式状态机 + 静态 lint + 显式 correction 注入) |
| 4. 机制健壮性 | **WARN** | P0: `reset_review_terminal_track()` 在生产代码中从未被调用;P2: Serial 模式"all-or-nothing"语义对 fix-round 不友好 |
| 5. 可维护与可观测 | **PASS** | 13 个新单元测试 + 2 个 KTD-TTC-2 推迟测试 + 4 个 loop_state 测试覆盖核心路径;`recovery.jsonl` 增 `review_terminal_drift` reason_code 便于 grep |

### 关键发现:per-step invariant 未落地

修复文档(`docs/solutions/integration-issues/ce-executor-serial-mechanism-close-loop-2026-06-23.md` L106-121)与代码注释(`loop_state.rs:306-309`, `1318-1325`)**明确承诺**:
- "Reset when a new step's `work.ready` is admitted"
- "The flag is per-step: a new `work.ready` admission calls `reset_review_terminal_track` and clears both"

但**实际生产路径中** `reset_review_terminal_track()` **从未被调用**(`Grep` 验证:`crates/ralph-core/src/event_loop/**` 只有 1 处定义 + 1 处测试,无调用方;`state_projection/**` 0 处)。

**根因**:此 API 是新增的,防线 B 的事件循环只调用了 `record_review_terminal_observation` 而忘了 `reset_*`。从 fix plan 角度看,这是 `event_loop/mod.rs:9468` 旁路修复不完整。

**影响**(必须修复):
- step 1 正常通过后,`review_passed_seen_for_step = true`
- step 2 后续如果 synthesizer 漂移,`review.complete` 先于 `review.passed`:**drift 检测被屏蔽**(`record_review_terminal_observation("review.complete")` 看到 `passed` 已 true,不返回 drift)
- 这正是 `ce-executor-serial-primary-20260623-152241` 失败模式的同型复发路径

---

## 3. 阶段 2:对抗性审查结果

| 维度 | 结论 | 备注 |
|---|---|---|
| 1. 隐藏副作用 | **PASS** | `record_review_terminal_observation` 仅修改 `LoopState` 自身字段;correction 注入走既有的 `inject_completion_correction` 路径 |
| 2. 兼容性破坏 | **PASS** | `DriftConfig` 新增 `coord_join_mode` 字段 + `Default = Parallel`,旧 YAML 字面量补字段;`exempt_consumers: Option<Vec<String>>` 默认 None |
| 3. 边界情况 | **WARN** | P0 per-step reset 未接;P2 Serial 模式 fix-round 盲点 |
| 4. 性能风险 | **PASS** | 每次 `record_*` 调用 O(1) match;`reset_*` 是简单字段清零;`CoordJoinMode` 切换不增加 detector 时间复杂度 |
| 5. 安全风险 | **PASS** | correction 注入通过既有 `inject_completion_correction` 路径(已被 verdict_gate / additional_topics 覆盖),不绕过 verdict_gate |
| 6. 命名误导 | **PASS** | `record_review_terminal_observation` 返回 `bool` 表示 drift,命名清晰;`reset_review_terminal_track` 不带 `pub(crate)` 限制是合理公开 API |
| 7. 测试不足 | **WARN** | P1 `check_publisher_terminal_completeness` 缺 owner 语义测试(已补);loop_state 缺跨 step 集成测试(已补) |
| 8. 维护成本 | **PASS** | 防御性注释充分,`KTD-RTC` / `KTD-Drift` 命名清晰,KTD-TTC-2 推迟路径用单测 pin 住 scope |

### P0 / P1 / P2 清单

| ID | 等级 | 位置 | 描述 | 状态 |
|---|---|---|---|---|
| P0-1 | P0 | `event_loop/mod.rs:9468` 旁路 | `reset_review_terminal_track()` 未在 step boundary 调用 → step 2 起 drift 检测被屏蔽 | **已修** |
| P1-1 | P1 | `preset_lint::check_publisher_terminal_completeness:137` | 规则未区分 owner vs non-owner hat,会对 shipper 类 readback hat 误报 | **已修**(收窄到 owner hat) |
| P1-2 | P1 | `coord_join_mode: serial` 的 fix-round 场景 | Serial 模式二值语义:fix-round 触发的 from-after-to 漂移被 last_to≥last_from 掩盖 | 设计意图保留(防 fix round 假阳性是 Serial 模式的核心卖点),但缺乏 follow-up |
| P2-1 | P2 | `record_review_terminal_observation` | 仅在 `review.complete` 时检查 `!review_passed_seen_for_step`;如果 synthesizer 反过来先发 `review.failed`(语义上不属于 pair),仍无法被检测 | 留给 KTD-TTC-2 |
| P2-2 | P2 | `inject_completion_correction` 的 correction message 长度 | 9500-9505 的 message 约 350 chars;`max_prompt_chars=2000` 单条 finding 远低于,无截断风险 | PASS,留作 follow-up monitor |

---

## 4. P0/P1 修复记录

### P0-1:step boundary 调 `reset_review_terminal_track()`

**根因**:`LoopState::record_review_terminal_observation` 和 `reset_review_terminal_track` 一对 API,后者定义了但生产路径零调用方。`work.ready` 触发的"新 step"路径在 `event_loop/mod.rs` 多处实现(`update_bootstrap_flags_from_accepted`、`work_done` 路径),但**没有一处**会同步调用 reset。

**修复**:`event_loop/mod.rs:8440` 的 step-close 分支(`queue.advance` / `review.failed` / `fix.applied`)在 `prune_work_done_bucket` 旁加上 `self.state.reset_review_terminal_track()`。这是 step boundary 的统一不变量挂点。

| 修复点 | 文件:行 |
|---|---|
| `event_loop/mod.rs:8440` 旁 | `crates/ralph-core/src/event_loop/mod.rs` |
| 测试新增 | `loop_state.rs::review_terminal_must_be_reset_between_steps`(line 1702 附近) |

**验证**:
- `cargo nextest run -p ralph-core -- review_terminal` → 15/15 passed
- `cargo nextest run -p ralph-core` → 2774/2774 passed
- `./scripts/run-tests.sh` → 5132/5132 passed(从 5129 增 3 个新测试,0 失败,13 skipped)

### P1-1:`check_publisher_terminal_completeness` 收窄到 owner hat

**根因**:原规则对所有"声明了 review.passed 但没声明 review.complete"的 hat 报错。但 shipper 类 hat 经常在 `publishes` 里声明 `review.complete` 作为状态 readback(从 upstream verdict 镜像),**不会**真正触发该 topic 的 emit 分支。规则把这种 readback 当作"未声明的 publisher"误报。

**修复**:在 `check_publisher_terminal_completeness` 入口先查 `config.topic_owners`。只有当 hat 是某个 pair topic 的登记 owner 时,才检查双声明完整性。非 owner hat 的"单边声明"被允许。

语义定义:
- 登记 owner + 双声明 → clean
- 登记 owner + 单边声明 → **flag**(原行为)
- 登记 owner + 都不声明 → clean(不参与该 pair 决策)
- 非 owner + 单边声明 → **silent**(新行为,P1 修复)
- 非 owner + 双声明 → silent
- `topic_owners` 未登记 owner → silent for entire pair(新行为)

| 修复点 | 文件:行 |
|---|---|
| 规则收窄 | `preset_lint/review_terminal_coherence.rs:137-180` |
| 既有测试加 `topic_owners` | `preset_lint/review_terminal_coherence.rs:393-425` |
| 新测试:non-owner readback | `preset_lint/review_terminal_coherence.rs:438-473` |
| 新测试:unowned pair silent | `preset_lint/review_terminal_coherence.rs:482-505` |

**验证**:
- `cargo nextest run -p ralph-core -- review_terminal` → 17/17 passed
- `cargo nextest run -p ralph-core -- preset_lint` → 100/100 passed
- `cargo nextest run -p ralph-core` → 2774/2774 passed
- `cargo nextest run -p ralph-cli --bin ralph -- ce_executor` → 47/47 passed
- `cargo nextest run -p ralph-core --test scenarios` → 65/65 passed
- `cargo nextest run -p ralph-core --features recording --test smoke_runner` → 57/57 passed
- `cargo nextest run -p ralph-cli --bin ralph -- policy_check` → 71/71 passed
- `./scripts/run-tests.sh` → 5132/5132 passed(0 failed, 13 skipped)

---

## 5. 剩余风险与 TODO

### 4 个「未修」项的二次判定

| 项 | 原报告说法 | 二次判定 |
|---|---|---|
| 1. `presets/en/ce-executor-serial.yml` plan-gate triggers / review-synthesizer publishes | "preset 是用户数据,只改 lint 和机制" | **维持**:lint 已经把这种结构错位标 Error,改 preset 反而绕开 lint 信号 |
| 2. shipper / reporter verdict 镜像行为 | "prompt 契约,非 Rust 代码" | **维持**:verdict_gate 双层 fail 检测已覆盖;correction 注入给 synthesizer 提供"诚实"翻译路径 |
| 3. `plan.complete/plan.blocked` / `fix.applied/fix.exhausted` 等其他互斥 pair | "KTD-TTC-2 范围" | **维持**:当前 lint 用测试 pin 住 scope,KTD-TTC-2 单独立项 |
| 4. hat-handoff 校验 | "历史 fix 已关闭该机制,本轮不重启" | **维持**:本轮无 hat-handoff 相关事件被违反 |

### 新发现 P2 留作 follow-up

| ID | 描述 | 建议路径 |
|---|---|---|
| P2-A | Serial `CoordJoinMode` 在 fix-round 场景可能漏报 | KTD-Drift-2:引入"从 from 集群中段提早 close"的检测;或用 separate `review.fix.*` 边 |
| P2-B | `record_review_terminal_observation` 不区分 `review.failed` | KTD-TTC-2:把 `review.failed` 视作 sibling `pair`(`review.passed` vs `review.failed` 是真正的"正负"互斥,`review.complete` 是 residual 旁路) |
| P2-C | correction message 文本硬编码 350+ chars | KTD-RTC-2:提为 `templates/review_drift_correction.md` 模板 |

### 历史模式对照(doc:line 引用)

- **noble-peacock 死循环**(`docs/solutions/integration-issues/ce-executor-serial-noble-peacock-2026-06-17.md`):本次"per-step reset"未接线是同型反模式 — **显式状态机 + reset hook** 是 noble-peacock 的根因修复路径,本次**新增**字段但**未**挂上 step boundary,根因相似
- **perky-maple guidance 风暴**:`loop_runner/tests.rs` 周边代码 + `suppress_human_guidance` 字段是 2026-06-18 修复;本次 correction message 走 `inject_completion_correction` 路径(与 `task.resume` 同源),不绕过 verdict_gate,安全
- **hat_handoff filename_mismatch 30 天 6 次复发**:`2026-06-23-004` plan U2 SSOT 化已治本;本轮与该机制正交,无回归

---

## 6. 修复前后对比

### 修复前(原始 bbee0c47 提交)

```
ralph-core/src/event_loop/mod.rs   → drift 检测在 step 1 起作用,step 2 起被 step 1 残留屏蔽
ralph-core/src/preset_lint/review_terminal_coherence.rs → 误报所有非 owner hat 的单边声明
```

### 修复后(本轮)

```
ralph-core/src/event_loop/mod.rs:8440
   + self.state.reset_review_terminal_track()
   ↳ step boundary 触发,确保 per-step 状态机不跨步泄漏

ralph-core/src/preset_lint/review_terminal_coherence.rs:137
   + 入口处查 config.topic_owners 收窄规则到 owner hat
   ↳ shipper 类 readback 不再误报
```

### 测试覆盖增量

| 测试名 | 文件 | 覆盖 invariant |
|---|---|---|
| `review_terminal_must_be_reset_between_steps` | `loop_state.rs` | step boundary 必须 reset |
| `non_owner_publishing_one_terminal_is_NOT_flagged` | `review_terminal_coherence.rs` | owner 语义收窄 |
| `unowned_pair_is_silent_for_entire_pair` | `review_terminal_coherence.rs` | 无 owner → silent |

### 最终验证

```
cargo nextest run -p ralph-core       → 2774 passed, 1 skipped
cargo nextest run -p ralph-core --test scenarios → 65 passed
cargo nextest run -p ralph-core --features recording --test smoke_runner → 57 passed
cargo nextest run -p ralph-cli --bin ralph -- ce_executor → 47 passed
cargo nextest run -p ralph-cli --bin ralph -- preset_lint → 11 passed
cargo nextest run -p ralph-cli --bin ralph -- policy_check → 71 passed
./scripts/run-tests.sh              → 5132 passed, 0 failed, 13 skipped ✅
```

---

## 7. 结论

- **P0-1 修复**:`reset_review_terminal_track()` step boundary 接线 → 治本(per-step 状态机不再跨步泄漏)
- **P1-1 修复**:`check_publisher_terminal_completeness` owner 语义收窄 → 消除 shipper 类 readback 误报
- **P1-2 / P2 留作 follow-up**:Serial 模式 fix-round 盲点 + correction message 模板化,均为设计层决策,不在 P0/P1 范围
- **4 个「未修」项二次判定**:全部维持,理由充分

机制层修复闭环 — **PASS**。建议合入。
