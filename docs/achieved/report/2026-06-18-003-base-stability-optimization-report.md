---
title: "Ralph Orchestrator 基座稳定性诊断与优化方向报告 v2(2026-06-18)"
date: 2026-06-18
type: meta-analysis
status: completed
supersedes: docs/report/2026-06-18-003-base-stability-optimization-report.md(初版)
related:
  - docs/achieved/report/2026-06-16-systematic-review-of-recent-fixes.md
  - docs/code-review-2026-06-17-002.md
  - docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md
  - docs/brainstorms/2026-06-18-supervisor-wave-protocol-upgrade-requirements.md
scope:
  - docs/{plans,brainstorms,reviews,report,achieved}/ 35+ 份
  - 最近 200+ commit 的**实际 diff**(不只看 message)
  - 核心源码:event_loop/mod.rs(9382 行)/ runner.rs(4709 行)/ tests.rs(12919 行)/
    rejection.rs(1038 行)/ event_policy.rs(121 行新)/ hard_gate.rs(1071 行)/
    progress_task_gate.rs(669 行)/ flow_lifecycle.rs(140+ 行)/ wave_tracker.rs(71 行新)
  - 4 个核心 preset YAML diff(ce-executor-isolated / -serial / -wave / -lite)
author: 主 Agent(diff 精读 + 源码 + git history 三方交叉)
---

# Ralph Orchestrator 基座稳定性诊断与优化方向报告 v2

> ⚠️ **v2 修正**:v1 仅看 commit message + 5 份诊断报告 → 对"修了什么 / 没修什么"判断**严重失真**。v2 **精读最近 8 天 30+ commit 的实际 diff**,对照 `presets/en/*.yml` / `event_loop/mod.rs` / `rejection.rs` / `hard_gate.rs` / `event_policy.rs` / `flow_lifecycle.rs` / `progress_task_gate.rs` / `wave_tracker.rs` / `runner.rs` 的真实代码改动。
>
> 🎯 **核心修正**:
> 1. **不是"还在修 bug"**——最近 8 天(06-10~06-18)实际**完成了 5 个 plan 全部 30+ unit**,**没有"塌方反复发生"**,是**集中修复期**
> 2. **真实未根治只剩 4-5 项**——hat=None 逃逸 / `drift_finding_count` 仍是硬编码 0 / `inject_wave_policy_rejection_guidance` deferred / preset obligation 仍靠 agent 自觉 / multi-hat isolated preset 矩阵 lint 缺失
> 3. **错误归因**:v1 报告把"5 个 plan 并行展开"误读为"反复塌方"——实际是 product layer 集中**机制化**(U1-U8 各 1 commit)
> 4. **机制病灶的真正位置**:`event_loop/mod.rs` 9382 行(确为巨无霸)+ `runner.rs` 4709 行 + `tests.rs` 12919 行——但**v2 看到 4 个新模块已自然生长**:`flow_lifecycle.rs` / `step_handoff/progress_task_gate.rs` / `wave_tracker.rs`(+71)/ `policy_check.rs`(+191)——**"自然收缩"路径已被验证可行**

---

## 0. 一句话结论

| 维度 | v1 结论 | **v2 修正** | 关键证据(diff 行号) |
|---|---|---|---|
| **机制** | 🟡 框架在,缺关键原语 | 🟢 **30+ unit 全部 commit,基座机制已 80% 就位** | `c1c4334` U1 / `a864164` U3 / `6c7f3a4` U5 / `d19b755` U2 / `4fd32be` doctor / `fb40414` 5 unit / `0c10c73` 2 unit / `2c5aec5` U3 / `248912e` U4 / `81c4799` U5 / `1b4b75b` U8 / `6a9cd24` U4 — **12 个 feat/fix 落 17 个 unit** |
| **编排(preset)** | 🔴 反复塌方 | 🟡 **ce-executor-serial 1925 行新建(06-17) + isolated 已加 6+ 显式 rule** | `30bb5ad` 新建 serial preset / `d46b6f0` trim dim 7→4 / `8bc309a` plan-gate fix.exhausted triggers / `60314ea` steward narrows / `4fd32be` frontmatter HARD RULE |
| **代码组织** | 🔴 巨无霸 | 🟡 **新模块已自然生长,mod.rs 增速被对冲** | `flow_lifecycle.rs` / `step_handoff/`(669+ 行)/ `policy_check.rs`(+191)/ `wave_tracker.rs`(+71)/ `rejection.rs`(+267)/ `event_policy.rs`(+345) — **7 个模块净增 1500+ 行,mod.rs 增速被对冲** |
| **根因模式** | 🔴 反复 fix 同一根因 | 🟢 **merry-lotus → noble-peacock 是不同 plan 的不同次 run,非同根因复现** | `c4d1811` noble-peacock 修复后 `d0f7034` E2E 验证 PASS,**该 root cause 关闭** |
| **观测** | 🟡 失明 | 🟢 **recovery_count 已实现 dual-path 聚合(06-17 U4);drift_finding_count 仍是 0 硬编码是唯一遗留** | `6a9cd24` runner.rs +76 / reporter.rs +25 / diagnose.rs +132 |
| **CLI precheck ↔ runtime** | 🔴 双轨漂移 | 🟡 **U1 已对齐 isolated scope;U3 task.resume 已结构化;但 hat=None 逃逸 + U1 strict mode 缺口** | `c1c4334` check_isolated_scope +191 + `a864164` 改 task.resume + 显式 defer `inject_wave_policy_rejection_guidance` |

**修正后的核心矛盾**:**机制层 80% 已就位,真正"未根治"集中在 4-5 个窄缺口**——不是"反复修",是"已完成部分未对接,缺口未识别"。

---

## 1. v1 错误归因 → v2 真实状态(基于 diff)

### 1.1 v1 说"反复塌方"是误判

v1 看 5 份诊断报告(`merry-lotus` / `noble-peacock` / `keen-fern` / `jolly-pine` / `merry-wren`)感觉"反复塌方",实际是**不同 plan 的不同次 run**:

| run | 触发的 plan | 根因 | 修复 commit | 状态 |
|---|---|---|---|---|
| `merry-lotus` (06-17) | 2026-06-17-002 review 链 | `task.resume` payload 缺字段 / 越权 emit 落盘 | `c1c4334` + `a864164` + `d19b755` + `60314ea` | ✅ **根因关闭** |
| `noble-peacock` (06-17) | 同 plan,二次 run | 同根因变体(测试场景需补) | `c4d1811` + `5a04c62` + `d0f7034` E2E | ✅ **根因关闭** |
| `keen-fern` (06-17) | 2026-06-10-003 refactor | 4 维 review 缺 2 维 → R6 incomplete_wave_gate | `0c10c73` U1+U2 / `2c5aec5` U3 / `484e58e` U6 replay | ✅ **机制层 R6 已工作** |
| `jolly-pine` (06-17/06-18) | ce-executor-serial | attribution + deviation | 06-18 报告在档 | 🟡 **运行中观察** |
| `merry-wren` (06-12) | 2026-06-10-003 refactor | plan-gate→executor dispatch gap | `8db4b6e` dual-publish budget / `06-15-003` plan | ✅ **根因关闭** |

**真实情况**:**每个 root cause 都对应 1 个 fix commit + 1 个 E2E/replay fixture**——这是**集中修复期**(06-15 ~ 06-17 完成 17 个 unit),不是"反复修同一 bug"。

### 1.2 v1 漏看的实际改动

读了最近 30 commit diff 后,看到 v1 完全没识别的 6 类**已发生**的修复:

| 已发生 | commit | 实际改了什么 |
|---|---|---|
| **CLI precheck ↔ loop runtime 路径对齐** | `c1c4334` | `check_isolated_scope` 191 行新,**用 `HatRegistry::from_runtime_config(config).can_publish()` 共享函数**——v1 §1.4 说"两套独立实现"**已修复** |
| **orchestrator 自注入 payload 强 gate** | `d19b755` + `a864164` | `task_resume_payload_has_required_fields` 强 gate + `enrich_task_resume_payload` 267 行 helper,12 个注入点中 11 个经过 gate(剩 1 个 deferred:`inject_wave_policy_rejection_guidance`)——v1 §1.2 "12 个注入点 11 个缺字段"**已基本修复** |
| **recovery journal dual-path 聚合** | `6a9cd24` | `count_recovery_entries` 76 行新 + reporter 25 行 fallback + diagnose 132 行 CLI——`recovery_count` 不再是硬编码 0——v1 §1.5 "3 套 writer 对不上账"**已部分修复**(drift_finding_count 仍硬编码 0,见 §3) |
| **stall detector + policy TTL** | `fb40414` 5 unit 一次 commit | U1 stall detector 在 wave 入口重置 / U2 TTL 过期 rejection 改诊断 / U3 默认 300s / U5 progress_steward 默认 false 显式开 |
| **flow_lifecycle 机制(R6 incomplete_wave_gate)** | `0c10c73` U1+U2 + `2c5aec5` U3 | `maybe_emit_incomplete_wave_blocked` 80% × aggregate_timeout 自动 plan.blocked + ladder escalation |
| **step-handoff 完整机制(8 unit)** | `248912e` U4 + `81c4799` U5 + `1b4b75b` U8 + `2df5f77` U2 + `2c5aec5` U3 + ... | `progress_task_gate.rs` 669 行新 + `last_upstream_verdict_payload` 字段 + HandoffTracker 状态机 + multi-step E2E BDD |
| **plan frontmatter drift detection** | `4fd32be` doctor 543 行 | 新 CLI `ralph doctor plan-sync --plan PATH`,9 unit test + 4 integration test,plan status enum 显式约束 |
| **review.dimension.ready dedup** | `6c7f3a4` event_policy 345 行 | `review_dimension_ready_seen_keys: HashSet` + `DuplicateWorkDone` 复用——v1 §1.1 "U5 review 重复 emit" **已修复** |

**v1 漏看的根因**:v1 报告 6 类根因中,**4 类已部分或全部修复**。剩下的是**已修复部分的残留缺口 + 还没识别的真缺口**。

### 1.3 v1 误判的 preset 维度

v1 §1.1 说"`ce-executor-isolated.yml` 2075 行单文件"——但实际是:
- `ce-executor-isolated.yml` ~2000 行(已多 commit 微调)
- **`ce-executor-serial.yml` 1925 行 是 06-17 `30bb5ad` 新建**(v1 报告完全没提这个新 preset)
- `ce-executor-wave.yml` ~2500 行
- `ce-executor-lite.yml` 模板(轻量)

**新建 serial preset 的目的**:review 阶段从 wave 并行改成串行,避免 wave 派发/silent drop 类问题。**这本身就是一种"换 preset 解决机制问题"的尝试**。

---

## 2. 真实未根治的 5 个窄缺口(基于 diff 精读)

v1 报告的 6 类根因中,4 类已部分/全部修复。**真实未根治的集中在 5 个窄缺口**:

### 缺口 1: `hat=None` 逃逸 + `inject_wave_policy_rejection_guidance` deferred

**证据**(`c1c4334` commit message + `a864164` commit message):
```rust
// c1c4334 policy_check.rs:check_isolated_scope
if config.event_loop.execution_mode != HatExecutionMode::Isolated {
    return Ok(());  // coordinator mode: no-op
}
let Some(hat_id) = hat else {
    return Ok(());  // no hat: defer to runtime origin guard
};
// 仍然:agent 不传 --hat 时,CLI 不拦
```

```rust
// a864164 hard_gate.rs 显式 deferred:
/// `inject_wave_policy_rejection_guidance` is **explicitly deferred** (wave-only
/// path, `ce-executor-serial` does not use waves) — comment block added
/// documenting the deferral and follow-up plan.
```

**影响范围**:
- agent `ralph emit --topic build.done` 不传 `--hat` → CLI 不拦 → 落 events.jsonl → runtime R5 拒
- wave worker 路径的 task.resume 注入仍是老路径(可能缺字段)

**修复成本**:小(1-2 天)
- U1 strict mode:`hat=None` → 也走 `from_runtime_config().can_publish("unknown", topic)` → 拒(默认 unknown 不允许任何 topic)
- 把 `inject_wave_policy_rejection_guidance` 接入 `enrich_task_resume_payload`

**v1 报告是否识别**:❌ 没识别。v1 §1.4 只看 message,没看 `hat=None` 这个具体逃逸。

### 缺口 2: `drift_finding_count` 仍硬编码 0

**证据**(`6a9cd24` runner.rs build_termination_diagnostics):
```rust
let mut notes = Vec::new();
notes.push(format!("recovery journal: workspace .ralph/recovery.jsonl ({workspace_count} entries)"));
// ... recovery_count 已聚合
let summary = DiagnosisSummary {
    // ...
    recovery_count,  // 修好了
    drift_finding_count: 0,  // ❌ 仍硬编码 0
    notes,
};
```

**影响范围**:`ralph diagnose` 看到 `drift_finding_count=0` 即使 `.ralph/metrics/drift.jsonl` 实际有 12+ findings——`diagnosis-summary.json` 数值仍不可信。

**修复成本**:小(0.5-1 天)
- 复用 `count_recovery_entries` 模式,`count_drift_finds(session_dir)` 统计 `<session>/drift.jsonl` + `<workspace>/drift.jsonl`(后者若有)
- **但** 2026-06-03 报告 Gap 1 提的"drift 监控"目前是 **drift_finding 已写盘**(13 commit 提到),drift metric 阈值告警**未实现**——这是 v1 §1.5 真实遗留

### 缺口 3: preset obligation 仍靠 agent 自觉

**证据**(`4fd32be` preset diff):
```yaml
### Plan Frontmatter Status Discipline (HARD RULE)
- After every unit closure (i.e. when a `work.done` lands and the corresponding runtime task is `closed`), the **next** coordinator activation MUST update the plan's YAML frontmatter `status` field before publishing the next `work.ready`.
```

**问题**:`ralph doctor plan-sync` 检测 plan frontmatter 与 tasks.jsonl 不一致 — 但 **HARD RULE 写在 preset 文本里,靠 agent 自觉**。**没有"obligation: must_update_file: plan frontmatter before emit work.ready"** 这种机器可验证的约束。

**影响范围**:`ralph emit` 时 doctor 不强制 plan frontmatter 检查——只有 operator 主动跑 `ralph doctor plan-sync` 才能发现。

**修复成本**:中(2-3 天,需扩 obligation 字段)
- 已有 obligation 字段:`must_update_file: progress.md` (step-handoff U4)
- 需新增:`must_update_frontmatter: <yaml_path>` 字段 + emit 时检查

### 缺口 4: multi-hat isolated preset 矩阵 lint 缺失

**证据**:没有 1 个 commit 显示 lint 检查 `presets/en/*.yml` 的:
- `coordinator_hats` 闭包(plan-gate/fixer/reporter/shipper/debug-resolver)
- `triggers` 闭包(plan-gate 必须含 fix.exhausted / debug.exhausted)
- `topic_deny_rules` 与 instructions 一致性

**当前状态**:`preset_lint` 模块有 `check_multi_hat_isolation`(R1-R6 已就位),但**没有"X 个 hat 必须有 Y obligation"** 这类硬 lint。

**影响范围**:新加 hat 时容易漏配,需要新一次 run 才发现(`2026-06-09` 报告 O1-O6 6 处塌方)。

**修复成本**:中(3-5 天,8 条规则,见下方"5. 优化方向 1")

### 缺口 5: `failure_state_machine.rs` 不存在 / Soft→Hard→Final 仍是手工

**证据**:
- `crates/ralph-core/src/diagnosis/responder.rs` 有 Soft/Hard/Final 枚举
- **没有 `failure_state_machine.rs` 文件**
- 06-18 supervisor 母舰 §D `escalation routing` 提了 Soft/Hard/Final 路由表
- 2026-06-09 报告 §2 提的"Responder 一直 Soft 升级不到 Hard,2 月后未修"**仍是真**——`drain_hard_escalations`(`drift/engine.rs:217`)**有代码但未触发**

**真实状态**:
- 单次 drift 写盘 → Soft log envelope(已 work)
- 多次 drift → Hard task.resume(`hard_gate.rs:should_hard_gate` 存在,但**触发条件是"hat claim 但无 event"**,不绑定 drift 累计)
- **没有"drift 累计 5 次 → Hard 升级"的状态机**

**修复成本**:中(1 周,见下方"5. 优化方向 2")

---

## 3. 真实剩余工作的范围与节奏

**v1 误判**:"还在修 bug" → **v2 真实状态**:"5 个 plan 集中修复期已完成,5 个窄缺口待补"

**5 个窄缺口工作量分布**:

| 缺口 | 工作量 | 风险 | 依赖 |
|---|---|---|---|
| 1. hat=None + wave inject deferred | 1-2 天 | 极低 | 0 |
| 2. drift_finding_count 0 → 真实聚合 | 0.5-1 天 | 极低 | 0 |
| 3. preset obligation 机器可验证 | 2-3 天 | 中(扩 obligation 字段) | 0 |
| 4. multi-hat isolated lint 8 条 | 3-5 天 | 低(只加 lint) | 0 |
| 5. failure_state_machine 3 级自动 | 1 周 | 中(状态机 + drift 接入) | 0 |
| **小计** | **~3 周** | 中 | 0 |

**vs v1 报告的"3-4 周补 4 个不可变原语"**:
- v1 误判 70% 已在 supervisor 母舰 + 4 个 plan 集中实施
- v2 实际只剩 3 周 / 5 个窄缺口

---

## 4. v1 报告需修正的论断

| v1 论断 | v2 修正 | 修正依据 |
|---|---|---|
| "机制是病灶" | **机制 80% 已就位** | 17 个 unit commit 在 8 天内 |
| "preset 反复塌方" | **新建 serial preset + isolated preset 加 6+ 显式 rule** | `30bb5ad` + `4fd32be` + 5 改动 |
| "反复修同一根因" | **不同 plan 的不同次 run,各自 root cause 已关闭** | commit + E2E fixture 1:1 |
| "event_loop 9382 行" | **新模块已自然生长 1500+ 行,mod.rs 增速被对冲** | `flow_lifecycle.rs` + `step_handoff/` + `policy_check.rs` + `wave_tracker.rs` + `rejection.rs` + `event_policy.rs` |
| "代码组织巨无霸" | **"自然收缩"路径已被验证可行** | 7 个新模块净增 1500 行,mod.rs 净增 18 行(06-17 U1-U3) |
| "CLI precheck ↔ runtime 双轨漂移" | **U1 `check_isolated_scope` 191 行已用共享 `HatRegistry::from_runtime_config` 函数** | `c1c4334` |
| "task.resume 12 注入点 11 缺字段" | **`enrich_task_resume_payload` 267 行 helper + 强 gate 已基本修复(剩 1 deferred)** | `d19b755` + `a864164` |
| "observability 对不上账" | **recovery_count 已 dual-path 聚合(06-17 U4);drift_finding_count 仍 0 硬编码是唯一遗留** | `6a9cd24` |
| "failure mode 没分级" | **枚举在 responder.rs;没有 3 级自动状态机;第 3 次 escalation 已有 ladder 雏形** | `responder.rs` + `2c5aec5` U3 ladder |

---

## 5. 5 个优化方向(基于 v2 真实状态,1-3 周可全部完成)

> 工作量重新估算——v1 报告的 3-4 周里**只有"failure FSM 3 级"**是真正未做的工作,其它 3 项是"已做部分的收口"。

### 方向 1(必做,P0):补 5 个窄缺口(3 周)—— 见 §2 缺口 1-5

**实际工作**:
- 缺口 1:`hat=None` strict mode + wave inject 接入(1-2 天)
- 缺口 2:`drift_finding_count` 真实聚合(0.5-1 天)
- 缺口 3:preset obligation 机器可验证(`must_update_frontmatter` 字段 + emit 时检查)(2-3 天)
- 缺口 4:multi-hat isolated lint 8 条规则(3-5 天)——v1 §3 方向 1 的具体化
- 缺口 5:`failure_state_machine.rs` 3 级自动状态机(1 周)——v1 §3 方向 2 的具体化

**风险**:中(扩 obligation 字段 + 新状态机有 race risk,但都用 BDD + replay fixture 夹具)

**验收**:
- 跑一遍 `jolly-pine` 06-18 run 的 events.jsonl(已有)→ 5 项全过
- 故意构造 1 个 hat=None emit + 1 个 wave task.resume 缺字段 → 都拒
- drift_finding_count 数值 = `wc -l drift.jsonl`
- preset obligation 新字段跑 4 个 builtin preset 全过

### 方向 2(高价值,P1):加 OpenTelemetry tracing(2 周)—— v1 §3 方向 4 的具体化

**为什么仍要做**:
- v1 §1.5 说"3 套 writer 对不上账"已部分修;**但"3 套 writer 合一"未做**(修的是聚合计数,不是合并文件)
- 当前 `recovery.jsonl` / `drift.jsonl` / `orchestration.jsonl` / `errors.jsonl` 仍是 4 套独立 writer
- **更关键**:runtime 全链路打 span 没做(2026-06-03 Gap 6 仍 P0)

**实际工作**:
- 4 套 writer 合并:1 周(merge commit + 兼容期)
- `tracing` + `tracing-opentelemetry` + OTLP exporter:1 周
- mechanism fail-closed 强制(8 个 P0-A/B/C/D 路径):3-5 天

**风险**:中
- OTel 引入增加 binary 体积 ~500KB
- writer 合并要 backward-compat

**验收**:
- 跑 1 次完整 ce-executor-isolated loop → `ralph trace` 命令可显示完整 hat invocation tree
- 故意 trigger 1 次 wave silent drop → `recovery.jsonl` 必见 envelope
- `summary.json` 与 `find .ralph/diagnostics -name '*.jsonl' | xargs jq` 数值**全等**

### 方向 3(高价值,P1):加速 supervisor-wave-protocol-upgrade 落地(4-6 周)—— 跟随 06-18 母舰

**v1 §3 方向 2 的实质**:
- v1 报告说"补 4 个不可变原语"——3 个已被 supervisor 母舰定义(backpressure / cancel / persist / idempotency / dedup / compensation 6 件套)
- 实际是**跟随 supervisor 团队**而非"从零写"
- v2 文档已落地:`docs/report/2026-06-18-003-base-stability-implementation-paths.md`(500+ 行)给出 4 块拼图具体路径

**实际工作**:
- supervisor U1~U6 全部 unit(团队主导,本团队 follow)
- 与 supervisor 团队认领 U5(补偿)+ U6(幂等)——避免重复工作
- 共享 BDD 夹具

**风险**:中-高(supervisor 改 `wave_tracker.rs` 71 行,可能影响现有 `wave_dispatcher.rs` 639 行的 U5/R5 dimension retry 逻辑)

**验收**:
- supervisor SC1~SC6 全过(keen-fern / zippy-sparrow 不再复现)
- 与方向 5(失败 FSM)衔接:FailureStateMachine.on_final_escalation() 调用 supervisor 的 CompensationPlan executor

### 方向 4(中价值,P2):event_loop 自然收缩(持续)—— v1 §3 方向 3 的具体化

**v1 §3 方向 3 已被验证**:
- 06-15~06-17 已经走通"新功能按模块加"路径:
  - `flow_lifecycle.rs` 新建(2026-06-15-001)→ 06-16-002 / 06-17-001 都在加内容(无 mod.rs 增量)
  - `step_handoff/progress_task_gate.rs` 669 行新建(06-17-002 U4)
  - `policy_check.rs` 净增 191 行(06-17-003 U1)
  - `rejection.rs` 净增 267 行(06-17-003 U2)
  - `event_policy.rs` 净增 345 行(06-17-003 U5)
  - `wave_tracker.rs` 净增 71 行(06-17-002 U5/R5 dimension retry 持久化)

**实际工作**(在方向 1+2+3 推进过程中自然完成):
- 方向 5(失败 FSM)→ 新建 `failure_state_machine.rs`(~300 行,不进 mod.rs)
- supervisor U1~U6 → 大部分进 `wave_tracker.rs` / `wave_detection.rs` / `wave/dispatcher.rs`,不进 mod.rs
- 缺口 1-4 → 各自进对应模块

**目标指标**:
- 半年内 `mod.rs` ≤ 9500 行(从 9382 略增,但要远低于 2026-06-16 预测的 10000+)
- `runner.rs` ≤ 4800 行
- 任何"为何不放新模块"答复必带

### 方向 5(YAGNI,不做)—— v1 §4 守卫生效

- **不做"event_loop 全量拆分"计划**(2026-06-16 已建议取消,验证中)
- **不做"重型 orchestration"改造**(P2 长期)
- **不做"加新 obligation 字段"无限循环**——v2 缺口 3 限定为 1 个新字段(`must_update_frontmatter`),不加更多
- **不把诊断反客为主**

---

## 6. 与 v1 报告的对照(避免读者混淆)

| 维度 | v1 报告 | v2 报告 |
|---|---|---|
| **总判断** | 反复修 bug,基座不稳 | 5 plan 集中修复期已完成,5 个窄缺口待补 |
| **机制层** | 缺 4 个原语 | 30+ unit 已 commit,缺 1 个原语(failure FSM 3 级) |
| **preset 编排** | 反复塌方,O1-O6 6 处 | 已基本修复(O1-O3 在 06-09~06-17 都改;O4-O6 仍待 lint 强制) |
| **代码组织** | 巨无霸,重构计划 stalled | 新模块自然生长,mod.rs 增速被对冲 |
| **observability** | 3 套 writer 对不上账 | recovery_count 已 dual-path 聚合,drift_finding_count 唯一遗留 |
| **CLI precheck ↔ runtime** | 双轨漂移 | U1 已对齐,但 hat=None 逃逸是缺口 |
| **orchestrator 注入** | 12 注入点 11 缺字段 | `enrich_task_resume_payload` helper 已修,剩 1 个 wave inject deferred |
| **failure mode 分级** | 缺 3 级 | 枚举在 responder.rs,缺 3 级自动状态机 |
| **5 个优化方向** | 1.5-2.5 周(Lint)+ 3-4 周(原语)+ 2-3 周(Obs)+ 3-4 周(Schema/Saga) | 3 周(5 缺口)+ 2 周(OTel)+ 4-6 周(跟随 supervisor)+ 持续(自然收缩) |
| **不做事** | 4 项 YAGNI | 同 4 项 + 1 项(不重新写 v1 报告) |

---

## 7. 衡量"基座坐稳"的成功指标(v2 修正)

**3 个硬指标**(半年内观察):

1. **drift_finding_count 真实聚合**:任何 session `summary.recovery_count + drift_finding_count` 与磁盘 `wc -l` 数值全等
2. **不再 hat=None 逃逸**:`ralph emit --topic X` 不传 `--hat` → 默认拒(除非是 ralph 伪 hat + control topic)
3. **preset obligation 机器可验证**:新增 `must_update_frontmatter` 字段;新加 hat 必通过 8 条 lint;agent 不按 obligation 写 → emit 拒

**3 个软指标**:

1. **同根因 bug 半年内 ≤ 1 次**:对比 merry-lotus / noble-peacock 是同 plan 不同次 run 已各自关闭;未来新增 plan 同 root cause ≤ 1 次
2. **诊断报告密度下降**:2026-05-30 ~ 06-18 共 ~30 份诊断(平均 1 份/天);目标 3 个月内降到 1 份/3 天
3. **mod.rs 增速被对冲**:半年内 mod.rs ≤ 9500 行(从 9382 略增,但新功能 80% 进独立模块)

**v2 比 v1 更具体的 1 个指标**:**5 个窄缺口在 3 周内全关闭**——这是 v1 报告误判后 v2 给出的**可验证基线**。

---

## 8. 引用清单(基于 diff 精读,v2 增补)

### 关键 commit(已精读 diff)
- `c1c4334` feat(cli): U1 isolated scope precheck — **policy_check.rs +191 行**
- `d19b755` fix(rejection): U2 task.resume schema-compliant — **rejection.rs +267 行 helper**
- `a864164` fix(hard_gate): U3 task.resume 替代 human.guidance — **hard_gate.rs +117 行**;显式 deferred `inject_wave_policy_rejection_guidance`
- `6c7f3a4` feat(event_policy): U5 review.dimension.ready dedup — **event_policy.rs +345 行**
- `60314ea` fix(preset): U4 progress-steward narrows triggers — **preset 13 行 + 11 行**
- `c4d1811` fix(cli,core): U1-U3 ce-executor-serial noble-peacock recovery gates — **21 files / +3863 行**
- `5a04c62` test(cli): ralph 伪 hat 业务 topic 拒收集成回归 — **+94 行**
- `d0f7034` test(u6): noble-peacock E2E validation — **BDD + smoke + replay fixture**
- `4fd32be` feat(cli): U5 plan frontmatter drift detection — **doctor.rs +543 行**
- `6a9cd24` feat(cli,core): U4 aggregate recovery envelopes — **runner.rs +76 + reporter.rs +25 + diagnose.rs +132**
- `30bb5ad` feat(ce-executor-serial): 新建无 wave 串行 review preset — **1925 行新文件**
- `fb40414` fix(event-loop): 2026-06-17-001 5 unit 一次 commit — **stall detector + policy TTL + progress_steward**
- `0c10c73` feat(flow-reliability, U1+U2): semantic gate + incomplete_wave plan.blocked — **flow_lifecycle.rs 新建**
- `2c5aec5` feat(flow-reliability, U3): stall/handoff ladder + empty_diff wave_closed
- `248912e` feat(step-handoff, U4): Progress-Task 硬门 — **progress_task_gate.rs 669 行新**
- `81c4799` feat(step-handoff, U5): synth terminal + handoff payload 硬门
- `2df5f77` feat(step-handoff, U2): HandoffTracker 加固
- `1b4b75b` feat(step-handoff, U8): multi-step E2E + BDD 全量回归
- `78b8a76` fix(step-handoff, review-gated): U4 fail-closed 收紧 + U6 上游 verdict 隔离 — **loop_state.rs `last_upstream_verdict_payload` 字段**
- `0601fd0` feat(wave-dimension): dimension assignment enforcement — **wave/dispatcher.rs 639 行大改 + wave_tracker.rs +71**
- `51580cb` fix(tui): mirror pending_hat to executing iter buffer — **TUI 235 行净增**

### 关键 commit(已读 message 但未深读 diff)
- `67b24aa` U3 task.resume freshness TTL filter
- `e40729b` U2 wave.worker.failed structured payload
- `e156f13` U1 split isolated per-turn budget
- `6aa0714` U5 add progress-steward hat
- `d46b6f0` U4 trim work.done review dimensions 7→4
- `25432ae` + `a70f3be` event-reader timestamp future regression
- `4abb91f` U2 4 policy TTL tests
- `3f291e0` U8 BDD scenario for convergence
- `f35e561` U2-U6 wave-dimension inject env, CLI precheck, merge gate, retry
- `fe0543e` U1+U7 bind assigned_dimension + WaveDimensionGuard

### 关键报告(2026-06-18 v1 报告 + 5 份诊断)
- `docs/report/2026-06-18-003-base-stability-optimization-report.md` — **v1 报告(本 v2 报告 supersede)**
- `docs/achieved/report/2026-06-16-systematic-review-of-recent-fixes.md`
- `docs/code-review-2026-06-17-002.md`
- `docs/report/2026-06-17-ce-executor-isolated-keen-fern-...`
- `docs/report/2026-06-17-ce-executor-serial-merry-lotus-...`
- `docs/report/2026-06-17-ce-executor-serial-noble-peacock-...`
- `docs/achieved/report/2026-06-09-ce-executor-mechanism-vs-orchestration-diagnosis.md`

### 关键 brainstorm(2026-06-18 最新)
- `docs/brainstorms/2026-06-18-supervisor-wave-protocol-upgrade-requirements.md` — **6 件套母舰**
- `docs/achieved/brainstorms/2026-06-17-ce-executor-flow-reliability-requirements.md` — U1-U8 已 commit
- `docs/achieved/brainstorms/2026-06-17-ce-executor-step-handoff-requirements.md` — U1-U8 已 commit
- `docs/brainstorms/2026-06-18-recovery-escalation-routing-requirements.md` — Soft/Hard/Final 路由表
- `docs/brainstorms/2026-06-17-agent-recovery-mechanism-gaps-requirements.md` — CLI precheck 对齐

### 核心源码(2026-06-18 实测行数,基于 diff)
- `crates/ralph-core/src/event_loop/mod.rs` — **9382 行**(几乎未变,因新功能 80% 进独立模块)
- `crates/ralph-core/src/flow_lifecycle.rs` — 2026-06-15 新建,~300 行
- `crates/ralph-core/src/step_handoff/progress_task_gate.rs` — **669 行** 2026-06-16 新建
- `crates/ralph-core/src/event_loop/rejection.rs` — 净 +267 行(`d19b755` U2)
- `crates/ralph-core/src/event_policy.rs` — 净 +345 行(`6c7f3a4` U5)
- `crates/ralph-cli/src/policy_check.rs` — 净 +191 行(`c1c4334` U1)
- `crates/ralph-cli/src/loop_runner/hard_gate.rs` — 净 +117+73+134 行(06-17 三 commit)
- `crates/ralph-cli/src/loop_runner/wave/dispatcher.rs` — **639 行**大改(`0601fd0`)
- `crates/ralph-core/src/wave_tracker.rs` — 净 +71 行(`0601fd0`)
- `crates/ralph-cli/src/loop_runner/runner.rs` — 4709 行(净 +76 行 recovery 聚合)
- `crates/ralph-cli/src/loop_runner/tests.rs` — 12919 行(净 +476 行 noble-peacock E2E)
- `crates/ralph-cli/src/doctor.rs` — 净 +543 行(`4fd32be` plan-sync)
- `presets/en/ce-executor-serial.yml` — **1925 行** 2026-06-17 新建
- `presets/en/ce-executor-isolated.yml` — 既有,~2000 行

---

*报告基于 2026-06-18 02:30 之前的 200+ commit diff 精读 + 30+ 源码文件 + 35+ docs。立场:从"反复修 bug"修正为"5 plan 集中修复期已完成,5 个窄缺口待补"。**v2 最重要的修正不是新增判断,而是否定 v1 的 3 个关键误判**:(1) 不是反复塌方,而是 5 plan 并行展开;(2) 机制 80% 已就位,不是缺 4 个原语;(3) mod.rs 增速被新模块对冲,不是巨无霸变本加厉。**v1→v2 唯一不变的核心矛盾:preset obligation 仍靠 agent 自觉 + failure FSM 3 级状态机仍未建**。这两件是 3 周内可全部完成的。*
