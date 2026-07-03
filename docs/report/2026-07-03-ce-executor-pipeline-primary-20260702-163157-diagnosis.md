# ce-executor-pipeline Run 诊断报告 — primary-20260702-163157

> 跑于:2026-07-02 / 起始 16:31:57 UTC / 终态 18:18:12 UTC
> preset:`presets/en/ce-executor-pipeline.yml`(12-hat 线性一条龙)
> plan:`docs/plans/2026-07-02-006-feat-ce-executor-serial-runtime-phase-authority-plan.md`
> 事件源:`.ralph/events-20260702-163157.jsonl`(14 行 = 1 启动 + 13 业务)
> 当前分支:`pittcat-dev` @ `8d79cf45`

---

## 1. 结论摘要

**整体健康度:链路完整 + 契约 100% 合规,但语义层存在 2 个 P0 wiring failure 被 silently accepted,verdict=blocked 路径被 preset 设计为"信任 fixer 自律",runtime 没拦。**

- **关键异常数量**:P0 ×2、P1 ×2、P2 ×2
- **是否涉及历史重复问题**:**否**(本次事件契约全部合规,与历史 `wave-synthesizer-no-fire`(2026-06-13)、`null-payload review.passed stall`(2026-06-12)不直接同型;本质是新发现的"verdict_gate 缺失 + fixer self-report wall"软契约漏洞)
- **一句话定性**:**preset 软契约 + runtime hard-gate 双重失守**,但事件流本身完全合规,LOOP 正确退出(`blocked` verdict 正常落到 `LOOP_COMPLETE.reason`)

---

## 2. 执行链路对比图

### 预期链路(preset `ce-executor-pipeline.yml`)

```
work.start
  → plan-reviewer          → plan.ready
  → executor               → work.done
  → dim:goal-alignment     → review.goalalign.done
  → dim:correctness        → review.correctness.done
  → dim:testing            → review.testing.done
  → dim:maintainability    → review.maintainability.done
  → dim:project-standards  → review.standards.done
  → dim:adversarial        → review.adversarial.done
  → review-synthesizer     → review.complete
  → fixer                  → fix.done
  → alignment              → align.done
  → reporter               → report.done + LOOP_COMPLETE
```

### 实际链路(`.ralph/events-20260702-163157.jsonl`,14 行)

| # | 时间(UTC) | topic | hat | 关键 payload | 状态 |
|---|-----------|-------|-----|--------------|------|
| 1 | 16:31:57 | work.start | loop-bootstrap | 引用 plan `2026-07-02-006-feat-ce-executor-serial-runtime-phase-authority` | ✅ |
| 2 | 16:33:25 | plan.ready | plan-reviewer | `plan_revised=true` | ✅ |
| 3 | 17:37:50 | work.done | executor | `commit_count=28, tests_run=28, tests_passed=28, changed_lines=4815, executor_head_sha=b47a7f9` | ✅ |
| 4 | 17:40:15 | review.goalalign.done | dim:goal-alignment | `findings_count=5` | ✅ |
| 5 | 17:43:34 | review.correctness.done | dim:correctness | `findings_count=6` | ✅ |
| 6 | 17:46:05 | review.testing.done | dim:testing | `findings_count=8` | ✅ |
| 7 | 17:50:08 | review.maintainability.done | dim:maintainability | `findings_count=9` | ✅ |
| 8 | 17:59:31 | review.standards.done | dim:project-standards | `findings_count=6` | ✅ |
| 9 | 18:06:05 | review.adversarial.done | dim:adversarial | `findings_count=9` | ✅ |
| 10 | 18:10:41 | review.complete | review-synthesizer | **`p0_count=2, p1_count=9, verdict=blocked`** | ⚠️ blocked verdict 起点 |
| 11 | 18:13:28 | fix.done | fixer | **`fixes_applied=0, fixes_skipped=22, review_verdict=blocked`** | ⚠️ **零修补跳跑** |
| 12 | 18:15:19 | align.done | alignment | `plan_executed=true, fix_plan_executed=false, residuals_count=22` | ⚠️ fix 阶段完全未执行 |
| 13 | 18:18:07 | report.done | reporter | `verdict=blocked` | ✅ |
| 14 | 18:18:12 | LOOP_COMPLETE | reporter | `reason: "blocked: 2 P0 wiring failures…"` | ✅(`completion_promise` 命中) |

**关键观察**:
- `required_events=["report.done"]` ✅(L13);`completion_promise="LOOP_COMPLETE"` ✅(L14,排在最后)
- 没有任何 hat 被跳过、顺序无倒置
- 单消费者链成立(`topic_deny_rules` 0 违规)
- `plan.blocked` / `work.failed` 短路未触发(预期内 — happy path 走完)
- 22 个 fix-plan actionable Units 全部未实施,却未触发 `work.failed` —— preset 显式允许此行为(见下文 P0-1)

---

## 3. 历史问题上下文

| 主题 | 历史问题类型 | 与本次的关联度 |
|------|--------------|----------------|
| **A. preset 设计** | lint 顺序漂移、`plan.blocked` reason 白名单外漂、null-payload `review.passed` stall | 中(`blocked` path 类似 null-payload stall 模式,本次更严 — blocker verdict 完整落地却无人拦) |
| **B. Ralph Loop 基座** | `loop_runner` Mutex/sleep flake、`mechanism.flow.plan_end` shipper 兜底缺失、repair budget 耗尽无升级 `plan.blocked` | **高** — 本次 P0-2 的根因正是"preset 没配 `verdict_gate` + loop runner 只在配置了才拦截" |
| **C. 多 hat 隔离 / handoff** | hat source 一致性、`HANDOFF_TOPIC_SEEDS` 缺宏观边、isolated mode pending Hat 不在 registry | 中(本次 6-dim 串行未发生 source drift,因为是 flat serial;但若改 mealy 多 consumer 会立刻爆) |
| **D. 诊断 / observability** | `review_passed_while_wave_open` 静默、`incomplete_wave_gate` 触发 `plan.blocked` | **高** — 本次 22 residuals 被 reporter happy-path 直接吞掉,等同于 incomplete_wave_gate 不存在 |
| **E. 与 ce-executor-pipeline 的潜在关联** | R15-R17 linear 拓扑硬约束、`topic_deny_rules` 6-dim 串行覆盖、null-payload 静默 stall | **极高** — 历史上两次 stall 类型复发点,本次以"非空 payload 但零实施"形态出现,属于同族风险但模式新 |

**结论**:**这是已知"preset 软契约 + runtime 缺 gate"漏洞的新表现形态**,无历史直接重样,但根因高度类同 — 任何被设计为"信任 agent 自律"的环节都会在 P-class finding 上失守。

---

## 4. 证据清单

| ID | 证据 | 来源 |
|----|------|------|
| E1 | `work.done` payload: `commit_count=28, tests_passed=28, executor_head_sha=b47a7f9` | `.ralph/events-20260702-163157.jsonl:L4` |
| E2 | `review.complete` payload: `p0_count=2, p1_count=9, findings_count=23, verdict="blocked"`,`findings_summary` 显式提及 "handle_phase_on_event_未被生产路径调用" 与 "whitelist 显式 hat_id=[] 被 wildcard 兜底绕过" | `events.jsonl:L11`、`fix-plan.md` |
| E3 | `fix.done` payload: `fixes_applied=0, fixes_skipped=22, review_verdict="blocked"` | `events.jsonl:L12` |
| E4 | `align.done` payload: `plan_executed=true, fix_plan_executed=false, residuals_count=22` | `events.jsonl:L13` |
| E5 | `report.done` payload: `verdict="blocked"` | `events.jsonl:L14` |
| E6 | `LOOP_COMPLETE.reason: "blocked: 2 P0 wiring failures…"` | `events.jsonl:L15` |
| E7 | preset 显式允许 0-fix 路径:`presets/en/ce-executor-pipeline.yml:1672-1676` fixer Failure Handling 段写明 "if actionable Unit cannot be completed: add to `fixes_skipped`, continue … still emit `fix.done`" | preset yml |
| E8 | preset 无 `verdict_gate` 字段:`presets/en/ce-executor-pipeline.yml:54-273` `event_loop` 块 | preset yml |
| E9 | runtime 硬校验仅放行所有 13 条事件 0 拒绝,`event_policy.rs:1564-1671` `check_required_fields` 不做语义互斥 | 代码 |
| E10 | 6 维 + fix-plan + report 文件全部落地:`.ralph/review/2026-07-02-006-feat-ce-executor-serial-runtime-phase-authority/` | fs check |
| E11 | `triggered` 字段在 11/12 业务事件中存在,但 `dim:goal-alignment` 缺,不在 schema 白名单 | `events.jsonl:L5-L15` |
| E12 | `align.done` schema 字段顺序与 emit 模板不一致 | preset yml:362-372 vs :1753 |

---

## 5. 问题归因表(P0 / P1 / P2)

| 优先级 | 问题描述 | 根因分类 | 证据 | 历史关联 |
|--------|----------|----------|------|----------|
| **P0-1** | `verdict="blocked"` 时 fixer 实施 0 个 P0/P1 但仍 emit `fix.done`,allow 22 个 actionable Unit 全部落 `fixes_skipped` | **A(preset)+B(runtime)+C(agent)** 三方叠加 | E2/E3/E7/E9 | 与 `null-payload review.passed stall` 同族(2026-06-12) |
| **P0-2** | preset **未配置 `verdict_gate`**,runtime 仅依赖配置驱动,2 个 P0 wiring failure 完全无 hard-gate 拦截 | **A(preset)+B(runtime)** | E8/E2/E6 | `incomplete_wave_gate` 静默事件(2026 历史已记) |
| **P1-1** | `triggered` 字段在 `topic_format_whitelist` 与 `required_fields` 中均未声明,但 emit 命令静默写入,dim #4 缺而 dim #5-10 有 → 数据一致性 / 来源标记混乱 | **B(runtime)+C(agent)** | E11 | 无 |
| **P1-2** | `align.done` 的 schema 字段顺序与 emit 模板不一致,JSON 无序但 lint/audit 工具会误判 | **A(preset)** | E12 | 无 |
| **P2-1** | worktree 复用规则仅以 prompt 文本注入 `work.start.payload`,preset 没把它升格为 `event_loop.worktree_reuse_policy` 硬字段 | **A(preset)+C(agent)** | event #1 载荷 | 弱关联(CLAUDE.md HARD RULE 3 提示) |
| **P2-2** | 6-dim hats 缺 `required_metadata: [triggered]` 显式声明,与 dim #5-10 来源不一致 | **A(preset)+C(agent)** | E11 | 无 |

---

## 6. 修复建议(按优先级)

### P0-1:拦截 `verdict=blocked` + `fixes_applied=0` 的非法 fix.done

**目标文件/机制**:
- `presets/en/ce-executor-pipeline.yml:1672-1676`(fixer `Failure Handling` 段改写)
- `crates/ralph-core/src/event_policy.rs:1564-1671`(`check_required_fields` 追加语义互斥钩子)

**具体修改**:
1. **Preset 侧**:`fix.done` 增加 contract — 当 `review_verdict="blocked"` 且 `fixes_applied < p0_count + p1_count`(从 `review.complete` 上游透传)时,fixer 必须 emit `work.failed` 而非 `fix.done`,`reason: "fixer_wall_violation: <p0> P0 / <p1> P1 unaddressed"`。
2. **Runtime 侧**:`event_policy.rs` 在 `check_required_fields` 之后追加 `check_blocked_verdict_completeness(fix.done)`,触发 `PolicyDecision::Block { reason: "fixer_wall_violation" }`。
3. **新增 BDD 场景**:`crates/ralph-core/tests/scenarios/blocked_fixer_wall.yml` + 同步在 `crates/ralph-core/tests/scenarios.rs` 注册,**必须用 `run_workflow_guard_scenario` 真 EventLoop runner 断言事件,不能用 `run_scenario` stub**(参 CLAUDE.md「preset/schema 改动后的下游同步清单」第 3 条)。

**预期效果**:fixer 不再"看了 P0 但不修就 emit `fix.done`",要么真修,要么触发 `work.failed` 走 reporter Branch B — `blocked` verdict 不再被"温和收敛"。

---

### P0-2:启用 `verdict_gate` 让 runtime 主动拦截 blocked verdict

**目标文件/机制**:
- `presets/en/ce-executor-pipeline.yml:54-86`(`event_loop` 块新增)
- `crates/ralph-core/src/event_loop/mod.rs:2208-2248`(`verdict_gate` 路径已存在但需配置驱动)
- `ce-executor-serial.yml` 同步(若同样需要)

**具体修改**:
```yaml
# presets/en/ce-executor-pipeline.yml:event_loop
verdict_gate:
  topic: review.complete
  verdict_field: verdict
  fail_verdicts: ["blocked"]
  on_fail: reject_with_resume
```
- 同步跑 `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 确认 `verdict_gate` 字段被 `schema_parity.rs` 接受(避免漂移 lint 误报)
- 在 `scripts/check-cli-doc-drift.sh` 的 preset drift 表登记新字段

**预期效果**:`verdict=blocked` 触发后 LOOP_COMPLETE 会被 runner 拒绝并触发 recovery envelope,而非当前"agent 自律温和收敛"。

---

### P1-1:明确 `triggered` 字段的来源与 schema 归属

**目标文件/机制**:
- `crates/ralph-cli/src/commands/emit.rs:900`(写入点)
- `presets/en/ce-executor-pipeline.yml:48-50`(`topic_format_whitelist`)
- 6 个 dim hat 的 `required_metadata`(新增)

**具体修改**:
1. `topic_format_whitelist` 加 `optional_metadata_fields: [triggered]` 描述。
2. 6 个 dim hat 显式声明 `required_metadata: [triggered]`,与 dim #5-10 行为对齐。
3. `event_policy.rs` 的 `check_origin_guard` 对 `triggered` 做"等于上游 hat 链前一 consumer"一致性校验。

---

### P1-2:schema 字段顺序 SSOT 化

**目标文件/机制**:`presets/en/ce-executor-pipeline.yml:362-372` vs `:1753`

**修改**:`align.done.required_fields` 顺序改为 `plan_name, plan_path, executor_head_sha, fix_plan_file, review_verdict, plan_executed, fix_plan_executed, residuals_count, residuals_summary`(与 emit 模板对齐),同审计 `fix.done` / `review.complete`。

---

### P2-1:worktree 复用规则升格为 preset 硬规则

**修改**:在 `event_loop` 块下加 `worktree_reuse: {require_reuse_key: true, fail_on_new_worktree: true}`,让 runner 拒绝任何 prompt 模糊匹配(参 CLAUDE.md HARD RULE 3)。

---

### P2-2:6-dim hats `triggered` 一致性

**修改**:在 dim:goal-alignment 等 6 个 hat 的 `event_filter` 旁加 `required_metadata: [triggered]`,保证 emit 链路来源标记一致。

---

# 7. 回答用户的 4 个核心问题

## ① 整体执行过程有没有问题?

**结构上没问题,语义上有问题。**

- 14 个事件、12 个 hat 全部按预期触发,顺序无误,无跳过、无短路、无 topic_deny 违规
- 13 条业务事件 100% 通过 preset_lint schema/topic 校验,合规度 100%
- `required_events=["report.done"]` 与 `completion_promise="LOOP_COMPLETE"` 双双命中

**但**:`verdict="blocked"` 路径上 fixer 实施 0 个 P0/P1 却顺利走完,signal 链是:`review.complete(p0=2,p1=9,verdict=blocked)` → `fix.done(fixes_applied=0,fixes_skipped=22)` → `align.done(residuals_count=22)` → `report.done(verdict=blocked)` → `LOOP_COMPLETE(reason="blocked: 2 P0 wiring failures…")`。该走 `work.failed` 的路径被 preset 设计成"继续 emit `fix.done`",runtime 没拦。

## ② 中间产物是否符合 RALPH 机制?

**事件契约全部合规,但 agent 输出暴露了 2 个真实 P0 bug**:

1. `handle_phase_on_event_` 未被生产路径调用 → phase engine 是静态 ACL
2. whitelist 显式 `hat_id=[]` 被 wildcard 兜底绕过

这两个 bug 写在 `fix-plan.md` 里(由 review-synthesizer 产出),但 fixer 选择跳过 22 个 Unit — 这是 preset 软契约(信任 fixer)+ runtime 缺 hard-gate(`verdict_gate` 未配)+ agent 自律失守(A+B+C)三方叠加。

事件流对账本身(RALPH 基座机制层)合规,**RALPH 机制没出问题**,出问题的是「预设信任边界画错了」。

## ③ 我的编排是否合理、是否正常运行?

**编排设计意图清晰、运行正确,但编排的 trust-boundary 失守**:
- happy path 设计正确(单消费者 flat serial 链,verdict 字段统一字面量,report→LOOP_COMPLETE 双 emit)
- 6-dim → review-synthesizer → fixer → alignment → reporter 的 handoff 无断裂
- **`verdict=blocked` 路径的硬约束没画** — preset 让 fixer "可继续 emit fix.done",导致 blocked 仍能 LOOP_COMPLETE(正常退出而非 fatal)
- residual_count=22 这种大批量未实施情况本应触发硬 gate(`residuals_gate` 或 `verdict_gate`),但预设和 runtime 都缺

**结论:编排设计是 95% 正确的,差的是 soft→hard 转换的边界规则。**

## ④ 真的有问题吗?是机制问题还是编排问题?

**两者都是,但根因权重不同:编排(A)是主导,机制(B)是缺失的兜底**。

**机制问题(B)**:
- `event_policy.rs:check_required_fields` 不做语义互斥(允许 `fixes_applied=0` 与 `verdict=blocked` 共存)
- `event_loop/mod.rs:verdict_gate` 路径已存在但未配置驱动
- `topic_format_whitelist` / `origin_guard` 对 `triggered` 字段无一致性约束

**编排问题(A)**:
- preset 在 `event_loop` 块没声明 `verdict_gate`(就算 runtime 支持也不生效)
- preset 的 `fixer.Failure Handling` 段显式写了"continue … still emit fix.done",把 hard-blocked 路径放给 agent 自律
- 6-dim hat 的 `required_metadata` 没显式声明,与 dim #5-10 行为不齐
- schema 字段顺序与 emit 模板不一致

**换言之**:**机制层可以做硬拦截但没被告知要拦截;编排层被设计成"信任 agent"但没在合约上明确边界**。两边同时失守,导致 `blocked` verdict 被 silently accepted。

修复路径:**P0-1 业务逻辑硬约束 + P0-2 verdict 终态硬约束 同时上**,把软契约升级为硬契约,agent 自律就可以被 runtime 取代。其余 P1/P2 是辅助修正。

---

# 8. 溯源与产物清单

## 事件溯源
- `.ralph/events-20260702-163157.jsonl:L1-L15`(完整业务事件)
- `.ralph/events-history-20260702-163157.jsonl`(harness 层历史)
- `.ralph/ledger.jsonl`(12 次 `loop.batch_sync` / 11 次 `loop.completion_requested/honored`)
- `.ralph/loops.json`(loop 元信息)
- `.ralph/diagnostics/`(诊断产物)
- `.ralph/agent/`(agent 运行时输出与 decisions.md)

## preset 与 schema SSOT
- `presets/en/ce-executor-pipeline.yml:54-273`(`event_loop` 块)
- `presets/en/ce-executor-pipeline.yml:278-376`(inline payload schemas)
- `presets/en/ce-executor-pipeline.yml:362-372`(`align.done` schema 顺序)
- `presets/en/ce-executor-pipeline.yml:1672-1676`(fixer `Failure Handling` 段)
- `presets/en/ce-executor-pipeline.yml:1753`(`align.done` emit 模板)

## review 产物目录
`.ralph/review/2026-07-02-006-feat-ce-executor-serial-runtime-phase-authority/`:
- `goal-alignment.md` / `correctness.md` / `testing.md` / `maintainability.md` / `standards.md` / `adversarial.md`(6 维)
- `fix-plan.md`(含 2 个 P0 wiring failure findings)
- `report.md`

## runtime 代码钩子
- `crates/ralph-core/src/event_policy.rs:1564-1671`(`check_required_fields` 钩子)
- `crates/ralph-core/src/event_loop/mod.rs:2208-2248`(`verdict_gate` 配置驱动点)
- `crates/ralph-cli/src/commands/emit.rs:900`(`triggered` 字段写入点)
- `crates/ralph-core/src/preset_lint/`(静态 lint 套件 — schema_parity / workflow_activation / ownership / multi_hat / topic_format / state_projection)

## git 历史
- `pittcat-dev` @ `8d79cf45`(loop primary head)
- executor 段共 28 commits(`commit_count=28` 与 plan 的 28 Implementation Unit 对齐)
- fixture:`feat(ralph-core): U25 minimal second-preset fixture` / `feat(ralph-core): U26 diagnosis_plan_complete_dual_check` / `feat(ralph-core): U27 advance_step_on_test_passed pure fn` / `feat(ce-executor-serial): U28 partial preset 瘦身`
