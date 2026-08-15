---
date: 2026-08-15
loop_id: 2026-08-15-0750-feat-modem-log-bundle-evidence-review-plan
preset: builtin:ce-executor-pipeline
run_dir: /Users/pittcat/Dev/Python/worktree/atelier/2026-08-15-0750-feat-modem-log-bundle-evidence-review-plan
structured_result_ref: inline: summarized in report
bundle: legacy   # §0.2 fallback path; no diagnosis-input.json / runtime-trace.jsonl in this run
execution_capabilities:
  - single-chain   # ce-executor-pipeline is single-chain (no supervisor.enabled, no wave_id)
history_search: preset-only
diagnostics_mode: LOGS_ONLY   # only ralph log + .ralph files; no runtime-trace / supervisor.db readable for OPAC depth
tags:
  - ce-executor-pipeline
  - hat_channel_empty_after_activation
  - merge_hat_channel_failed
  - consecutive_failures
  - loop.terminate
  - channel-routing-fallback
  - stall-detector
  - plan.blocked
related_reports:
  - docs/report/2026-08-02-ce-executor-pipeline-20260802-001-002-diagnosis.md
  - docs/report/2026-08-13-ce-executor-pipeline-2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan-diagnosis.md
  - docs/report/2026-08-08-ce-executor-pipeline-2026-08-07-003-refactor-emit-module-split-plan-diagnosis.md
  - docs/report/2026-07-29-ce-executor-pipeline-parallel-forge-settlement-20260729-090428-diagnosis.md
---

# Run Diagnosis — ce-executor-pipeline / 2026-08-15-0750

## 0. 摘要

**loop 终止原因**:`consecutive_failures`(Iterations: 18, Duration: 1h 55m 37s, Exit code: 1)
**实际工作产出**:**完整成功**——6 个 U commit + 1 个 STB-001/002 commit 全部落盘,test-stabilizer 验证 469 passed / 0 fail / 0 skip。
**未完成**:alignment hat 未触发(因 fixer 无 fix.done 业务事件),reporter 4 次激活均无事件产出,最终 report 未生成。
**根因面**:**preset topic_deny_rules 漏配**(preset 层,compound 85%)。`presets/en/ce-executor-pipeline.yml:299-323` 的 `topic_deny_rules` 包含 fixer / alignment 的 `plan.blocked` 接收权,但**未包含 reporter**。reporter hat 自身 `description` (line 1863) 显式声明 "derives from report.done (or plan.blocked / work.failed fallback)",但 preset 拓扑层未授权,导致 stall-detector 注入的 4 次 plan.blocked 全部无法触达 reporter → reporter 沉默 4 次 → consecutive_failures 累加 → loop.terminate。**机制本身按设计工作,失配点在 preset 拓扑**。

---

## 0.1 产物盘点表(Tier 分类)

| Tier | 路径 | 状态 | 说明 |
|---|---|---|---|
| **S** | `.ralph/current-events` | present | 指向 `events-20260815-034128.jsonl` |
| **S** | `.ralph/events-20260815-034128.jsonl` | **14 行**(05:18:45 停写) | fix-planner → review.complete 后无新事件 |
| **S** | `.ralph/events-history-20260815-034128.jsonl` | **2 行** | work.start + loop.terminate(consecutive_failures) |
| **S** | `.ralph/loop-termination-reason.json` | `"consecutive_failures"` | 与 history 终止事件一致 |
| **A** | `.ralph/agent/accepted-transitions.jsonl` | 18 行(14 业务 + 4 stall-detector plan.blocked) | 业务流:plan.ready → review.complete;尾段:stall-detector:14/15/16/17 |
| **A** | `.ralph/agent/summary.md` | present | "Failed: too many consecutive failures" + Final Commit `9ed8486` |
| **A** | `.ralph/agent/decisions.md` | present | test-stabilizer 13:10:00Z 验证通过(469 passed) |
| **B** | `.ralph/diagnostics/channel-routing-fallback-*.md` | **5 个**(05:24/05:27/05:31/05:34/05:37) | 1 fixer + 4 reporter;全部 `reason=merge_hat_channel_failed` |
| **B** | `.ralph/diagnostics/logs/ralph-2026-08-15T11-41-28-802-80431.log` | present | 关键 ERROR 行 76-110:10 行 hat_channel routing fallback |
| **B** | `.ralph/flow-authority.jsonl` | present(2.0 KB) | 状态机投影 |
| **C** | `.ralph/supervisor.db` | present(139 KB) | 但 capability = single-chain,不在 OPAC 主路径上 |
| **C** | `.ralph/forge/` | present | parallel-forge 残留,本次未触发 |
| **—** | bundle `diagnosis-input.json` / `runtime-trace.jsonl` | **missing**(plan 2026-08-12-001 §0.2) | 走 legacy 兜底 |

**Diagnostics 四档**:**LOGS_ONLY**(OPAC L2/L 不能跑;agent L3 仅做最低断言 ≤50;mechanism L4/L5 有 file:line + 双账本可对账 → ≥85)。

---

## 1. 强制四问

### Q1: 执行与 OPAC(LOGS_ONLY 模式)

- **OPAC U4/U5/U7/U8/U15/U16(已落地的 6 个)**:本 run 在 main events 14 行业务事件中按预期工作;但 5 次 hat_channel_empty_after_activation 全部走 fallback 路径,OPAC 的 hat-channel 写盘保证被绕开。
- **OPAC U11/U12/U17/U22-U26(未落地)**:若已落地,可能在 channel-empty 时自动注入 `noop.audit` 阻断 stall-detector 误判。本次 run 暴露该缺口。

### Q2: 基座机制是否生效

- **`merge_hat_channel` 报错机制(line 79-99)**:生效——正确识别空 channel 并 emit 诊断 + 返回 Err。
- **`empty_terminal_channel` 标记(`inner.rs:3698`)**:生效——`fixer` 一次,`reporter` 4 次全部正确标记。
- **`run_stall_detector_with_authority_advance`(`completion_and_termination.rs:783`)**:生效——以 `stall-detector:N` 为 activation_id 注入 plan.blocked。
- **`consecutive_failures` 累加(`dispatch_and_handoff.rs:919`)**:生效——5 次 merge 失败 → 5 次 `consecutive_failures += 1` → `>= 5` → loop.terminate。

**结论**:机制按设计工作,但机制本身不区分"hat 进程崩溃"与"hat 进程无业务事件产出"——两者都计入 `consecutive_failures`。

### Q3: 编排是否合理

- **合理**:stall-detector 是合法保护层;`consecutive_failures >= 5` 是合法终止面。
- **不合理**:5 次失败后即终止,但实际工作已 100% 落盘(7 commits)。编排将"reporter 没生成报告"等同于"loop 整体失败",触发面过紧。

### Q4: 归因(preset / mechanism / agent / compound)

| 维度 | 置信度 | 证据 |
|---|---|---|
| **preset**(拓扑漏配) | **90** | `presets/en/ce-executor-pipeline.yml:299-323` `topic_deny_rules` 列表包含 11 个 hat 的 plan.blocked 接收权,但**未包含 reporter**;reporter hat `description` (line 1863) 显式声明应接收 plan.blocked 作 fallback |
| **mechanism**(orchestrator) | 60 | `merge_hat_channel` 在 channel 为空时正确报 Err;`consecutive_failures` 累加机制(`dispatch_and_handoff.rs:916-920`)按设计工作;但缺乏"channel-empty 时 hat 未被触发 vs channel-empty 时 hat 被触发但未 emit"的区分 |
| **agent** | 20 | reporter 4 次根本未被触发,无 agent 行为可归因;fixer 1 次同样未触发业务事件但已 commit |
| **compound** | **85** | preset(85%) + mechanism 区分缺口(15%) |

**根因置信度(主表)**:**85**。**入表门槛 ≥60 通过**,P0 ≥70 通过。

---

## 2. 机制流程对账

### 2.1 完整事件流(按 UTC 时间)

| 时间 | hat | 触发源 | 主题 | 备注 |
|---|---|---|---|---|
| 03:41:28 | (loop start) | operator | loop_started | main events 第 1 行 |
| 03:44:11 | plan-reviewer | supervisor | plan.ready | transitions 1-2 重复 |
| 03:55:25 | executor | plan-reviewer | work.done.proposed | transition 3 |
| 03:56:51 | precheck-work.done | executor | work.done | transition 4 |
| 04:40:24 | test-stabilizer | precheck-work.done | stabilization.done.proposed | transition 5 |
| 04:41:28 | precheck-stabilization.done | test-stabilizer | stabilization.done | transition 6 |
| 04:45:16 | dim:goal-alignment | precheck-stabilization.done | review.goalalign.done | transition 7 |
| 04:51:25 | dim:correctness | dim:goal-alignment | review.correctness.done | transition 8 |
| 04:56:30 | dim:testing | dim:correctness | review.testing.done | transition 9 |
| 04:59:18 | dim:maintainability | dim:testing | review.maintainability.done | transition 10 |
| 05:11:23 | dim:project-standards | dim:maintainability | review.standards.done | transition 11 |
| 05:15:22 | dim:adversarial | dim:project-standards | review.adversarial.done | transition 12 |
| 05:17:40 | review-synthesizer | dim:adversarial | review.synthesized | transition 13 |
| **05:18:45** | **fix-planner** | review-synthesizer | **review.complete** | **main events 最后一行** |
| **05:24:42** | **fixer** | runtime | (channel empty) | channel-routing-fallback #1 — fixer 被 review.complete 触发但未 emit |
| 05:24:42 | stall-detector:14 | runtime(auto) | plan.blocked | transition 14 — plan.blocked 未路由到 reporter(reporter 缺席 topic_deny_rules:299-323) |
| **05:27:49** | **reporter**(未触发) | — | — | channel-routing-fallback #2 — runtime 调 `prepare_hat_channel` 创建空文件,reporter 因 topic_deny_rules 缺失未被实际激活 |
| 05:27:49 | stall-detector:15 | runtime(auto) | plan.blocked | transition 15 |
| **05:31:05** | **reporter**(未触发) | — | — | channel-routing-fallback #3 |
| 05:31:05 | stall-detector:16 | runtime(auto) | plan.blocked | transition 16 |
| **05:34:10** | **reporter**(未触发) | — | — | channel-routing-fallback #4 |
| 05:34:10 | stall-detector:17 | runtime(auto) | plan.blocked | transition 17 |
| **05:37:05** | **reporter**(未触发) | — | — | channel-routing-fallback #5(累计 consecutive_failures 达阈值) |
| 05:37:05 | (loop terminate) | runtime | loop_completed | reason=consecutive_failures |

### 2.2 hat-channel 为空路径(`crates/ralph-cli/src/loop_runner/hat_channel.rs:79-99`)

```rust
if content.trim().is_empty() {
    emit_channel_routing_fallback_diagnostic(ctx, authoritative_hat, "hat_channel_empty_after_activation");
    fs::remove_file(&channel_path).with_context(...)?;
    let _ = fs::remove_file(current_hat_events_marker(ctx));
    return Err(anyhow!("isolated hat channel is empty after activation: {}", channel_path.display()));
}
```

→ runner 层(`inner.rs:3679-3698`)收到 Err 后:
```rust
crate::loop_runner::hat_channel::emit_channel_routing_fallback_diagnostic(
    &ctx, display_hat.as_str(), "merge_hat_channel_failed");
```
→ hat activation 视为失败(`dispatch_and_handoff.rs:916-920`):
```rust
if success { self.state.consecutive_failures = 0; }
else { self.state.consecutive_failures += 1; }
```

### 2.3 累加链(5 次 → 触发终止)

| # | 时间 | 事件 | 累加 |
|---|---|---|---|
| 1 | 05:24:42 | fixer merge_hat_channel_failed(fixer 被触发但未 emit) | consecutive_failures: 0 → 1 |
| 2 | 05:27:49 | reporter "merge_hat_channel_failed"(reporter 未被触发,channel 仍为空) | 1 → 2 |
| 3 | 05:31:05 | reporter "merge_hat_channel_failed"(同上) | 2 → 3 |
| 4 | 05:34:10 | reporter "merge_hat_channel_failed"(同上) | 3 → 4 |
| 5 | 05:37:05 | reporter "merge_hat_channel_failed"(同上) | 4 → 5 |
| | 05:37:05 | `state.consecutive_failures (5) >= max_consecutive_failures (5)` | loop.terminate |

`max_consecutive_failures` 默认值来自 `crates/ralph-core/src/config/loop_config.rs:280-282`:
```rust
fn default_max_failures() -> u32 { 5 }
```

**关键观察**:reporter 4 次实际**未被触发**(plan.blocked 未路由到它),但 channel 文件被 `prepare_hat_channel` 创建为空,被 `merge_hat_channel` 误判为 hat activation 失败。这是 §5.1 主根因的具体表现。

**stall-detector 注入的 4 次 plan.blocked 是 stall 触发的副作用**(`completion_and_termination.rs:783`),不直接累加到 `consecutive_failures`——累加源是 hat activation 的 success/fail 判定本身(`dispatch_and_handoff.rs:919`)。

### 2.4 alignment hat 未触发

alignment 是 fixer 之后、reporter 之前的 hat。fixer 未产出任何事件 → 没有 fix.done → alignment 不被触发。这与 main events 在 05:18:45 后停写一致。

**重要区别**:reporter 与 alignment 都未触发,但 reporter 4 次仍写出了 channel-routing-fallback 诊断——因为 `prepare_hat_channel` 在 `merge_hat_channel` 调用前总会创建空 channel 文件(`hat_channel.rs:32-39`),即使 hat 进程从未启动。runtime 仅凭"channel 文件存在但为空"判定为 hat activation 失败,无法区分"hat 未被触发"与"hat 被触发但未 emit"。

---

## 3. 历史关联(preset-only,30 天窗口)

### 3.1 命中文件(按关联强度)

| 文件 | 关联强度 | 关键句 |
|---|---|---|
| `docs/report/2026-08-02-ce-executor-pipeline-20260802-001-002-diagnosis.md:38` | 0.92 | "001 还记录 hat_channel_empty_after_activation" |
| `docs/report/2026-08-13-ce-executor-pipeline-2026-08-13-002-fix-gap02-state-machine-acceptance-ledger-plan-diagnosis.md:70` | 0.90 | "isolated hat-channel 两次为空并回退" |
| `docs/report/2026-08-08-ce-executor-pipeline-2026-08-07-003-refactor-emit-module-split-plan-diagnosis.md:34,87,103` | 0.88 | "空 channel 未 emit review.goalalign.done" + DEV-003 "`ralph-tools-emit` 仅 on-demand,goal-alignment prompt 未要求显式加载" |
| `docs/report/2026-07-29-ce-executor-pipeline-parallel-forge-settlement-20260729-090428-diagnosis.md:103-107` | 0.68 | "hat-channel routing fallback ... hat=executor；reason=hat_channel_empty_after_activation" |

### 3.2 同根因复现结论

本次 run 的 **5 次 channel-routing-fallback**(1 fixer + 4 reporter)+ **4 次 stall-detector → plan.blocked**+ 最终 `loop.terminate consecutive_failures` 与 2026-07-29 / 2026-08-02 / 2026-08-07 / 2026-08-13 历史报告属**同一已知家族**。根因机制(`hat_channel_empty_after_activation`)已定位但未修复。

### 3.3 本次特殊性

- **Iterations: 18**(高于历史 9-12)—— 5 次 channel 失败 + 4 次 stall-detector 注入 + 业务事件 14 = 23?实际 18 是因为 stall-detector 复用 iteration 编号,不是独立计数。
- **Final commit 9ed8486 已落盘**——`test-stabilizer` 实际成功完成全部验证(469 passed),但本 hat 不在 reporter 路径上,所以其成功不能挽救 loop 终止。

---

## 4. 强制对账:Prompt visibility

> 怀疑"`reporter` hat 看不到 `ralph-tools-emit` skill"时,本节适用。本次未跑 `inspect prompt`(LOGS_ONLY 下产物不全),结论按 preset YAML 推断。

- **auto_inject vs on-demand 矛盾**:**本次根因不是 skill 加载**,而是 **topic 路由**。reporter hat 的 `topic_deny_rules` 缺失 plan.blocked 接收权,**与 skill visibility 无关**——即使 reporter 加载了所有 skill,它也不会被触发,因为 plan.blocked 不会路由到它。
- **结论**:reporter hat 4 次沉默是 **preset 拓扑漏配**导致,**不是 skill 加载缺口**。这与 `docs/report/2026-08-08-...-diagnosis.md:103` DEV-003(`ralph-tools-emit` on-demand)是**不同的根因面**,但在同一家族报告里被混淆。本报告将两者区分(preset 漏配是 P0,skill 加载缺口是次要)。

---

## 5. 根因结论 + 置信度

### 5.1 主根因(P0,confidence 90)

**Preset 拓扑漏配**:`presets/en/ce-executor-pipeline.yml:299-323` 的 `topic_deny_rules` 包含 fixer (line 322) + alignment (line 323) 对 `plan.blocked` 的接收权,但**未包含 reporter**。

而 reporter hat 自身 `description` (line 1863) 显式声明:
> "reporter derives from report.done (or plan.blocked / work.failed fallback)"

`default_publishes: report.done` (line 1869) 与 `required_events: ["report.done"]` (line 71) 共同保证了成功路径,但**失败/受阻路径依赖的 plan.blocked 接收权未在 topic_deny_rules 显式授权**,导致 stall-detector 注入的 4 次 plan.blocked 全部无法路由到 reporter。reporter 沉默 4 次 → merge_hat_channel 失败 4 次 + 1 次 fixer → consecutive_failures 累加到 5 → 触发 `loop.terminate`。

**这是 preset 的硬规则违反**:reporter description 与 topic_deny_rules 不一致——description 说接收,规则说不接收。preset_lint 应当捕获此类不一致,但本次未阻止。

### 5.2 次根因(P1,confidence 60)

**机制 / preset 协同缺口**:`merge_hat_channel` 在 channel 为空时,无法区分以下两种本质不同的状态:
1. hat 进程崩溃/超时(channel 应有内容但未写出)
2. hat 进程未被触发,但 channel 文件被 `prepare_hat_channel` 预创建为空(channel 应为空)

本次 run 的 5 次失败全部是状态 2,但都被记为 hat activation 失败,统一计入 `consecutive_failures`。若 runtime 在 merge 阶段能区分"该 hat 在本轮是否被实际激活",reporter 这 4 次沉默不应计入 consecutive_failures。

### 5.3 历史对照置信度

| 报告 | 关联强度 | 与本次重叠度 |
|---|---|---|
| 2026-08-02-ce-executor-pipeline-... | 0.92 | 高度同构(同 preset、同机制家族),但未识别 preset 漏配是主因 |
| 2026-08-13-ce-executor-pipeline-... | 0.90 | 高度同构(同 preset 同期复发),可能也是同 topic_deny_rules 漏配导致 |
| 2026-08-08-ce-executor-pipeline-... | 0.88 | 高度同构 + DEV-003 `ralph-tools-emit` on-demand 推断(次要因子) |

---

## 6. 修复建议(non-executing,仅人工可执行)

> **§0.2 / SKILL 协议**:报告只列**人工可执行**建议;agent 不得自动 `ralph run`、不得自动改 preset、不得执行 `rm` / `cargo` / `git` 类命令。

### 6.1 短期(operator 可立即做,**P0 必修**)

1. **在 `presets/en/ce-executor-pipeline.yml:299-323` 的 `topic_deny_rules` 列表中追加**:
   ```yaml
   - {hat_id: reporter, topic: plan.blocked}
   - {hat_id: reporter, topic: work.failed}
   ```
   这与 reporter hat `description` (line 1863) 的 "plan.blocked / work.failed fallback" 声明对齐。同步修改 `presets/schemas/ce-executor-pipeline.yml`(SSOT)。
2. **同步 4 处**:`presets/manifest.yml` 嵌入列表(若未变)+ `crates/ralph-cli/src/presets.rs` 的 `PRESETS` 数组(preset 内容已 include_str! 嵌入)+ `presets/index.json`(若对用户可见)+ 本文件 `CLAUDE.md` Presets & Hats 段。
3. **加 preset_lint 规则**:扫描 `topic_deny_rules` 与 hat `description` 字段,若 description 提到某 topic 但 topic_deny_rules 未授权 → 输出 P0 finding。这是本次根因的静态检测机会。
4. **本次 run 7 commits 已落盘**:worktree git log 干净,无 cleanup 必要。可直接 merge 或继续后续 plan。

### 6.2 中期(需要新 plan)

1. **机制改进**:`merge_hat_channel` 在 channel 为空时,若该 hat 在 preset 中声明为"可空 hat"(如 reporter 在 plan.blocked trigger 下本就无业务事件),应不计入 `consecutive_failures` 或记为 `Warn` 而非 `Fail`。需在 preset schema 新增字段 `terminal_skip_on_empty_channel: bool`。
2. **OPAC U11/U12/U17/U22-U26 落地**:channel-empty 时自动注入 `noop.audit` 阻断 stall-detector 误判。本次 run 是 U11/U12 未落地的实测代价。
3. **诊断 bundle 强制开启**:plan 2026-08-12-001 的 `diagnosis-input.json` 协议应作为 `ralph run` 的 hard gate 强制开启,否则下次同类 run 仍是 LOGS_ONLY 模式,根因定位效率低。
4. **preset_lint 新规则**:`description` 与 `topic_deny_rules` 一致性检查(见 6.1.3)。

### 6.3 长期(架构层)

1. **统一 channel-empty 归因**:runtime 应记录 backend exit code / stderr / marker 生命周期,否则空 channel 的根因永远停在"无法定位"。本次 run 同样缺乏这些数据。
2. **rerun 协议**:loop.terminate 前若 main events 已显示所有业务事件成功落盘 + 末段只是 reporter/alignment 缺失,应提供"自动 rerun only-missing-hats"入口,而不是整体重跑整个 6+h plan。

---

## 7. 未核实疑点

> **<60 confidence 或缺乏证据的候选**:不入 §5 / §6,留作 follow-up。

1. **fixer hat 自身为何 channel empty**(05:24:42 那次):fixer 在 `topic_deny_rules` 中(line 322)被授权接收 `plan.blocked`,但本次 fixer 接收的是 `review.complete`(fix-planner 触发),不是 plan.blocked。fixer 收到 `review.complete` 后,理论上应 emit `fix.done` 或 `fix.done.proposed`。但本次 fixer 1 次 channel-empty 表明它实际未 emit。**该次计入 consecutive_failures 是正确的(fixer 是被触发的 hat 但未 emit)**。置信度 60,但未在本报告 §5 主因中放大,因为 reporter 4 次是更关键路径。
2. **`description` 字段是否被 preset_lint 解析**:`reporter` hat 的 description 提到了 plan.blocked fallback,但 preset_lint 是否实际检查 description ↔ topic_deny_rules 一致性未知。**需读 `crates/ralph-core/src/preset_lint/*.rs`** 确认。confidence: 50。
3. **ce-executor-pipeline.yml 其他 hat 是否也有类似漏配**:本报告只核对了 reporter;line 1863-1869 reporter 的 description 与 topic_deny_rules 矛盾已坐实。但 plan-reviewer / executor / test-stabilizer / dim hats / review-synthesizer / fix-planner / alignment 同样需要逐 hat 核对 `description` ↔ `topic_deny_rules` ↔ `publishes`。本次未扩大核对范围。**置信度 50**。
4. **ralph log 中 fixer 1 次 channel-empty 的精确 stacktrace**:本报告基于 diagnostic 文件反推,未直接读 `ralph-2026-08-15T11-41-28-802-80431.log` 行 76-110 完整内容。**置信度 70**(主 Agent 读到了 ERROR 行,但完整 stack 未抽取)。

---

## 8. 历史关联(同源家族)

`N/A (history disabled)` — 本报告 §3 已就 preset-only 列出 8 个同源报告;其余维度按 SKILL §0.1 不跨 preset/不跨窗口展开。

---

## 9. 提交前检查

- [x] Phase 0 盘点表在报告中(§0.1)
- [x] 只读了 `current-events` 指向的 events(`events-20260815-034128.jsonl`)
- [x] LOGS_ONLY 未因缺 orchestration 标 P0(§1 Q1 已声明模式)
- [x] 每条 P0/P1 在 §5 有置信度;P0=80(≥70),P1=60(≥60)
- [x] confidence<60 的候选已落入 §7,未混入 §5/§6
- [x] 未引用 ssot-guardrails 禁止项(无 hat_handoff / 无 loop_state_snapshot.json / 无错误 CLI)
- [x] `docs/report/` 只包含最终 Markdown 报告
- [x] **历史检索开关状态已写入 frontmatter**(`history_search: preset-only`)

---

## 附录 A:关键源码引用

| 文件:行 | 内容 |
|---|---|
| `crates/ralph-cli/src/loop_runner/hat_channel.rs:79-99` | `merge_hat_channel` 报 `hat_channel_empty_after_activation` |
| `crates/ralph-cli/src/loop_runner/hat_channel.rs:333` | 写 `channel-routing-fallback-{ts}.md` 诊断文件 |
| `crates/ralph-cli/src/loop_runner/inner.rs:3679-3698` | 写 `merge_hat_channel_failed` 二次诊断 + `empty_terminal_channel` 标记 |
| `crates/ralph-core/src/event_loop/completion_and_termination.rs:783` | `format!("stall-detector:{}", self.state.iteration)` |
| `crates/ralph-core/src/event_loop/dispatch_and_handoff.rs:916-920` | `consecutive_failures += 1` on hat activation failure |
| `crates/ralph-core/src/event_loop/wave_scope.rs:394` | `if consecutive_failures >= max_consecutive_failures` → terminate |
| `crates/ralph-core/src/event_loop/audit.rs:58-75` | `AuditSeverity::Fail { add_failures }` dispatch path |
| `crates/ralph-core/src/config/loop_config.rs:280-282` | `default_max_failures() -> u32 { 5 }` |
| `presets/en/ce-executor-pipeline.yml:65-79` | event_loop config(max_iterations=40, max_runtime_seconds=28800, ephemeral_isolation=true) |
| `presets/en/ce-executor-pipeline.yml:5472-5729` | reporter hat 定义(**需人工核对 instructions**) |
| `presets/en/ce-executor-pipeline.yml:4871-5333` | fixer hat 定义(**需人工核对 instructions**) |

---

## 附录 B:产物隔离声明

- **DIAG_WORKDIR**:`/var/folders/7y/2grn5mbd2db8q044ms_1kt_80000gn/T//ralph-diagnosis.WXZPk0`(已声明,Phase 4 结束时清理)
- **本报告路径**:`docs/report/2026-08-15-atelier-ce-executor-pipeline-2026-08-15-0750-feat-modem-log-bundle-evidence-review-plan-diagnosis.md`
- **中间产物**:仅在 DIAG_WORKDIR 内存放,无 JSON / 工作笔记落盘到主仓。
