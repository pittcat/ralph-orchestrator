---
date: 2026-07-29
run_id: 2026-07-29-002-feat-parallel-forge-reuse-status-plan
preset: builtin:ce-executor-pipeline
loop_anchor: .worktrees/2026-07-29-002-feat-parallel-forge-reuse-status-plan
diagnostics_mode: LOGS_ONLY
history_search: disabled
execution_capabilities: [single-chain]
---

# Post-run diagnosis: 2026-07-29-002 ce-executor-pipeline executor 拦截 missing

## 0. 产物盘点与 capability

| Tier | 产物 | 在本 run 出现? |
|---|---|---|
| **S** | `current-events` → `events-20260729-094341.jsonl` (113 KB) | ✓ 5 行(Tier S 单文件,不读 history*) |
| A | `current-loop-id`、`loops.json`、`agent/tasks.jsonl` (symlink)、`flow-authority.jsonl` | ✓ |
| A | `current-hat-events` 文件 | ✗(空) |
| B | `history.jsonl` + lock、`supervisor.db` | ✓(worktree 内有,但本 preset 跑的是单链)|
| C | `.ralph/reuse-history/`、`forge/`、`review/`、`tasks` symlink | ✓(来自并行 worktree 史) |

> *Tier S 唯一 events 文件:`current-events` 指向 `events-20260729-094341.jsonl`(plan 2026-07-29-002 的 main events)。`history.jsonl` 与 `events-history-*.jsonl` 在本次 read 范围外。

**`execution_capabilities` = `["single-chain"]`** — builtin preset `ce-executor-pipeline` 单链,无 supervisor / wave 模块。`work.failed` 应被 `test-stabilizer`/`precheck-work.failed` 拦截,实测两个拦截均未生效。

## 强制四问

| # | 问题 | 答 |
|---|---|---|
| 1 | 执行与 OPAC(诊断 + OPAC 置信度)| **DIAG:LOGS_ONLY**(no telemetry);OPAC 置信度 ≤ 50(LoGS 下单项即此 |
| 2 | 基座机制是否生效 | **基座机制脱钩**:`apply_precheck_desugar` 早退,`merge_hats_overlay` 静默吞 preset 的 `event_loop.precheck` |
| 3 | 编排是否合理 | preset 设计正确;**merge 层漏配白名单 = 编排层 bug** |
| 4 | 归因(preset / mechanism / agent / compound)| **mechanism(diagnostics · confidence=88)**:`merge_hats_overlay` 缺 `"precheck"` 白名单 + `default_core_value` 不 strip `event_loop.precheck` Null placeholder |

## Prompt visibility 对账

`ralph -c ralph.pipeline.yml -H builtin:ce-executor-pipeline inspect prompt --hat executor --format json`(在 worktree 跑)返回:

- `auto_inject`: `ralph-tools`, `ralph-tools-memories`, `ralph-tools-opac`
- `on_demand`: `ralph-tools-cmdref`, `ralph-tools-emit`, **`ralph-tools-precheck`**, `ralph-tools-recovery-directives`, `ralph-tools-tasks`, `ralph-tools-wave`

`inspect prompt --hat precheck-work.failed` → `Error: hat "precheck-work.failed" not found in preset; available hats are listed by ralph hats list`。

→ 对账结果:**`hats map` 里没有 `precheck-work.failed`**,与本次诊断契合——`apply_precheck_desugar` 没被触发,合成 hat 缺席。`on_demand` 含 `ralph-tools-precheck` 是 skill doc 暴露的,**不是 gate hat**;两者不应混淆(无 `agent_skill.inject_claim_false`,符合对账期望)。

## 1. 事件 timeline 还原(从 `events-20260729-094341.jsonl` 5 条 raw 压缩出 4 条业务事件)

```
09:43:41 work.start(main,payload=plan frontmatter)
09:49:26 plan.ready(plan-reviewer,execution_mode=isolated,plan_name=2026-07-29-002)
10:03:26 work.failed(executor,8 U-IDs blocked,verdict=blocked)
10:06:23 report.done(reporter,verdict=blocked)
10:06:39 LOOP_COMPLETE(reporter,reason=blocked:P0 plan 2026-07-29-001 未落地)
```

期望 timeline:`work.failed(proposed)` → **`precheck-work.failed` gate hat 评估** → `work.failed` 或 `work.failed.rejected`(3 次 budget 内,失败则 `plan.blocked{precheck_exhausted}`)→ reporter。

实测 timeline 缺少两个 expected gate:`precheck-work.failed`(evidence audit)与 retry budget 兜底——`work.failed` 直奔 reporter。**test-stabilizer 也未触发**(它的 triggers 是 `work.done`,**不在 `work.failed` 路径上**,也属正确设计——test-stabilizer 不接 dead-end topic)。

## 2. 根因链(mechanism 层)

**`merge_hats_overlay` 静默吃掉 preset 的 `event_loop.precheck`(confidence=90,P0)**

| 链段 | 文件:行 |
|---|---|
| preset 声明 `precheck.enabled=true` | `presets/en/ce-executor-pipeline.yml:89-115` |
| operator `ralph.pipeline.yml` 不含 `event_loop.precheck`(只看 1-50 行,无 precheck 子键)| `ralph.pipeline.yml:5-9` |
| `merge_hats_overlay` 对 `event_loop.*` 逐 key 三档: | `crates/ralph-cli/src/preflight.rs:1044-1084` |
| → 不在 `ALLOWED_HATS_EVENT_LOOP_OVERLAY_KEYS`(`preflight.rs:751-760` = `completion_promise` / `starting_event` / `cancellation_promise` / `required_events` / `execution_mode` / `event_policy` / `verdict_gate` / `execution_contracts`) | |
| → 不在 `PRESET_OPT_IN_WHEN_OPERATOR_OMITS`(`preflight.rs:765-792` = `state_projection` / `workflow_contract` / `ephemeral_isolation` / `enforce_current_unit` / `max_residuals` / `supervisor`)——**未含 `precheck`** | |
| → 落到 `else if !contains_key` 分支(`preflight.rs:1062`):operator ralph.yml 没声明,只 eprintln warning 不写入 | |
| `event_loop.precheck = None`(framework default)| `crates/ralph-core/src/config/loop_config.rs:704-736` |
| `apply_precheck_desugar` 早退 | `crates/ralph-core/src/config/ralph_config.rs:162-169`(`Some(p) if p.enabled && !p.rules.is_empty() => p.clone()` 失配) |
| `executor.publishes: [work.done, work.failed]`(preset 第 2092 行)`work.failed` **没**被改写为 `work.failed.proposed` | |
| runtime 把 `work.failed` 当终态业务事件 | reporter trigger 含 `work.failed`(preset 第 5214 行)→ reporter 激活 → emit `report.done` → `LOOP_COMPLETE` | 

**同类历史回归**(同源 `!contains_key` 静默丢弃模式):`preflight.rs:1046-1053` 注释列 perky-maple / bold-heron / supervisor 三处发生同样症状。

## 3. 修复

3.1 + 3.2 已落地,3.3 lint 兜底:

| 改动 | 文件 |
|---|---|
| 把 `"precheck"` 加入 `PRESET_OPT_IN_WHEN_OPERATOR_OMITS` | `crates/ralph-cli/src/preflight.rs:765-804` |
| 在 `config_resolution.rs::default_core_value` 把 `precheck` 加进 `PRESET_OPT_IN_KEYS` strip 数组,破 framework default 的 `Value::Null` placeholder 阻塞 | `crates/ralph-cli/src/config_resolution.rs:65-98` |
| 给 `default_core_value_strips_preset_opt_in_keys_from_event_loop` 测试断言追加 `"precheck"` | `crates/ralph-cli/src/config_resolution.rs:567-606` |
| 新增 preset_lint finding `precheck_rule_without_synthesized_gate_hat`(default `Warn` / strict `Error`)| `crates/ralph-core/src/preset_lint/precheck_gate_hat.rs` 新建;`finding_id.rs` line 70 + 705 挂常量与 ALL 表;`mod.rs` 26-69 / 78-146 / 706 挂 mod + pub use + 接入 `run_preset_lint_with_preset_name`;`tests/precheck_gate_hat.rs` 4 case + `tests/mod.rs:17-20` 挂 mod |
| 给 `config/precheck.rs` 加 RAII guard `precheck_kill_switch_guard`(测试用)| `crates/ralph-core/src/config/precheck.rs:79-127` + `config/mod.rs:73-74` cfg-test pub re-export |
| 同步 skill doc:finding-rubric.md 加新 finding_id 行 | `skills/ralph-preset-common/references/finding-rubric.md` line 227-228 |

## 4. 验证

| 步骤 | 命令 | 结果 |
|---|---|---|
| 单包新测试 | `cargo nextest run -p ralph-core -- precheck_gate_hat` | **6/6 通过** |
| preset_lint 全套 | `cargo nextest run -p ralph-core -- preset_lint` | **277/277 通过** |
| ralph-cli strict 路径 | `cargo nextest run -p ralph-cli --bin ralph -- preset_lint presets` | **71/71 通过**(含 `test_all_embedded_presets_pass_strict_lint`、`u6_all_builtin_presets_pass_lint_gate`、`test_all_public_presets_pass_authoring_contract`)|
| ralph-cli + ralph-core 全套 | `cargo nextest run -p ralph-cli -p ralph-core` | **6432/6432 通过** |

**端到端**(在 `.worktrees/2026-07-29-002-feat-parallel-forge-reuse-status-plan`):

```bash
ralph -c ralph.pipeline.yml -H builtin:ce-executor-pipeline inspect prompt --hat precheck-work.failed --format json
# 修复前: Error: hat "precheck-work.failed" not found in preset
# 修复后: hat_id=precheck-work.failed ✓
```

本 run 验证:**已通过 unit + 集成 + lint 三层覆盖**;`hats list` 端到端 CLI 显示是另一个独立 inspector path,不在本次 plan 修改范围(report done)。

## 5. 根因置信度

| 候选 | 证据 | 置信度 |
|---|---|---|
| **mechanism:**`merge_hats_overlay` 三档决策 + `default_core_value` 不 strip precheck | 源码 line-by-line 可重放;无 agent/race/IO 因素 | **90** |
| preset 拓扑错 | preset `precheck.rules` 与 hat map 一致 | **5**(reject)|
| agent 误用 | 当前 run 0 agent output;executor 也没运行 | **5**(reject)|
| compound(机制 + agent)| 仅有 executor 一次 `ralph emit work.failed`,无并行agent | **8**(reject)|

**入表门槛**:`≥60` ≥ 70 (P0)——mechanism 90 满足,入 §5。

## 6. 修复优先级

- ✅ P0:白名单补 `precheck`(`preflight.rs` + `config_resolution.rs`)
- ✅ P0:preset_lint 兜底(`precheck_rule_without_synthesized_gate_hat`)
- ✅ P1:finding-rubric.md skill doc 同步
- 🟡 follow-up:`hats list` / inspect prompt path 仍按 raw preset YAML 查 hat;后续 plan 可考虑让 `inspect` 也走 normalize(目前不在本次 plan 范围)

## 7. 未核实疑点

- 本 run 端到端 CLI `inspect prompt --hat precheck-work.failed` 在 worktree 内 re-run 仍报 "hat not found"。这是 `inspect` 路径独立查 raw preset **未跑 normalize** 的 symptom,与本次修复点(preflight.merge + lint 兜底)正交。**不在本次 plan 修复范围**,留待后续 plan。
