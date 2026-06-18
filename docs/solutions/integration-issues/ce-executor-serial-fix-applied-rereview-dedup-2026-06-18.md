---
title: "ce-executor-serial fix→re-review dedup: PolicyRuntimeState + obligations + contract"
date: 2026-06-18
origin: docs/report/2026-06-18-003-perky-maple-loop-link-diagnosis.md
plan: docs/plans/2026-06-18-004-fix-ce-executor-serial-perky-maple-orchestration-gaps-plan.md
tags: [ce-executor-serial, policy, dedup, obligations, plan-gate, fix.applied, review.dimensions.complete]
---

# ce-executor-serial fix→re-review dedup — 编排缺口根因与修复

## TL;DR

`ce-executor-serial` 在 perky-maple 03 plan run 中卡死 step-01 闭环的根因
**不是** plan-gate triggers 缺 `fix.applied`(诊断报告初版 P1-1 误判),
而是 **`review.dimension.ready` 与 `review.dimensions.complete` 的 dedup key
都缺 `fix_round`**,导致 fix → re-review 路径在 policy 层被永久阻断,
触发 HARD GATE spiral。修复通过 U0/U1/U3/U5/U6 组合落地,**禁止**给
plan-gate 加 `fix.applied` trigger(违反 KTD3 编排约定)。

## 根因分层

### P1-3(policy,真根因)
- `review.dimension.ready` dedup key = `{plan}::{step}::{task}::{dim}`,
  不含 `fix_round`。`fix.applied` accept 时未 prune 该 bucket;loop
  重启时 `from_events` replay 也不 prune。
- 后果:fix_round≥1 时 review-coordinator 重发 readiness 被
  `DuplicateWorkDone` 拒 → 06:19/06:25 两次 duplicate 拒 →
  06:26 HARD GATE 卡死 → 06:35 误发第 5 次 `review.dimensions.complete`
  → 06:41 review-synthesizer 静默 → 06:49 用户 abort。

### P1-1 误判(诊断报告)
- 初版报告归因为 "plan-gate triggers 缺 `fix.applied` / `review.failed`",
  对应 KTD3 `docs/achieved/plan/2026-06-02-004-fix-ce-executor-plan-gate-plan.md`
  的 plan-gate 设计约定:**plan-gate 只在终态 verdict 上 dispatch**,
  不监听 step-boundary events。
- perky-maple 时间线证明 loop **从未到达** `review.passed` —
  re-review dedup 阻断让 plan-gate 永远收不到 `review.passed`。
  给 plan-gate 加 `fix.applied` 触发会让它跳过 re-review 直接推进,
  违反 KTD3 架构约定。

### 排他性关系
- 修 dedup prune 与修 plan-gate triggers **互斥**:前者让 re-review
  走完,后者跳过 re-review。**只能选其一**,2026-06-18-004 计划选前者
  (KTD1,经对抗性审查确认)。

## 修复策略(2026-06-18-004 plan)

| U | 目标 | 关键变更 |
|---|---|---|
| **U0** | schema SSOT | `review.dimensions.complete` 必填 `fix_round` |
| **U1** | dedup prune | `fix.applied` accept prune `PolicyRuntimeState::review_dimension_ready_seen_keys` + `from_events` replay 对称 |
| **U2** | guidance 抑制 | `event_loop.suppress_human_guidance: true`(serial preset) |
| **U3** | obligations 收窄 | `review-coordinator` 在 `fix.applied` 上只允许 `review.dimension.ready`(关掉 `review.dimensions.complete` 捷径) |
| **U4** | BDD + 静态 | `ce_executor_serial_fix_applied_rereview.yml` 端到端;plan-gate 负向断言 |
| **U5** | complete dedup | `review.dimensions.complete` dedup key 含 `fix_round`,2nd emit RejectWithResume |
| **U6** | contract | `fix.applied` rule `commit_only`(非 `diff_or_commit`、非虚构 `strict`);错误文案动态化 |

### 为什么不是给 plan-gate 加 trigger

KTD3(`docs/achieved/plan/2026-06-02-004-fix-ce-executor-plan-gate-plan.md`)
把 plan-gate 设计为终态 dispatcher,只听:
- `review.passed`(review-synthesizer 通过)
- `review.complete`(shipper 复审通过)
- `work.failed`(失败路径 → `plan.blocked`)
- `fix.exhausted` / `debug.exhausted`(兜底)

监听 `fix.applied` 会让 plan-gate 在 re-review 走完之前就 dispatch,
绕开 review-synthesizer 的 verdict 阶段,导致 silent out-of-order handoff。

## 关键设计点

### KTD1(dedup prune > plan-gate trigger)

**选择**: `fix.applied` accept 时 prune
`PolicyRuntimeState.review_dimension_ready_seen_keys` 匹配
`{plan}::{step}::{task_id}::` 前缀;同步改 `from_events` replay
遇 `fix.applied` 执行同等 prune。

**理由**: re-review 走完后会自然产生 `review.passed` 唤醒 plan-gate,
不需要给 plan-gate 加新 trigger。

**防御**:
- `test_ce_executor_serial_plan_gate_must_not_listen_to_fix_applied`
  静态测试(已在 `presets.rs`)。
- BDD `ce_executor_serial_fix_applied_rereview.yml` 端到端覆盖。

### KTD3(complete dedup key 含 `fix_round`)

`review.dimensions.complete` dedup key = `{plan}::{step}::{task}::{fix_round}`。
缺省 `0`(legacy 兼容)。同 fix_round 第二次 complete → RejectWithResume
(非 silent extra-business drop),带 recovery hint 指向「用 next round
的 fix_round=N+1 + 先走 ready 序列」。

### KTD2(serial 禁 human guidance 进 prompt)

perky-maple P1-2 探针风暴(135 条 policy 拒绝,6 轮 × 22+ 变体)
根因是 executor 收到 free-text guidance 后盲试 emit。

**选择**: `event_loop.suppress_human_guidance: true` + 三处注入点
(`update_robot_guidance` / `apply_robot_guidance` / `prepend_scratchpad`)
跳过 guidance。

**保留**:全局 `human.guidance` topic + scratchpad 持久化可留作审计;
与「不进 prompt」不矛盾。

## 与历史 P1 的对比

| 历史报告 | 同根 / 不同 | 链接 |
|---|---|---|
| merry-lotus (2026-06-17) | 同根(plan-gate triggers 缺 `fix.applied`),已预言本次问题 | `docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md` |
| noble-peacock (2026-06-17) | 同根(review-chain stalled),同样预言 | `docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md` |
| ce-executor-isolated-dispatch-gap | 同根架构层(plan-gate 桥接),但 isolated preset 而非 serial | `memory/ce-executor-isolated-dispatch-gap.md` |

3 个 worktree run 均落入同坑;本次(perky-maple)由对抗性审查确认
**根因是 dedup 而非 plan-gate 触发**,merry-lotus / noble-peacock 的
"plan-gate triggers 缺 fix.applied" 分析同误。

## 反模式(避免再次落入)

1. **勿给 plan-gate 加 `fix.applied` / `review.failed` triggers** —
   违反 KTD3,会跳过 re-review。
2. **勿仅用 instructions 教 agent 忍 guidance** —
   executor 看到 guidance 仍会误读。`ce-executor-serial` 直接
   `suppress_human_guidance`。
3. **勿虚构 `require_git_change.mode: strict`** —
   不存在该值。用 `commit_only` 即可。
4. **勿在 `fix.applied` 上保留 `review.dimensions.complete` 捷径** —
   配合 U1 + U5 的 dedup,缺这一步仍会落入 perky-maple P2-5
   spiral。

## 后续工作(非本计划)

- **U7**:`loops.json` stale 清理(P3,不阻塞 release)
- **U8**:`hat_lifecycle` WARN 上下文(P3)
- **U9**:`hat-channel` 路由 serial preset 失效(P3,可降级文档)

## 验证

```bash
cargo nextest run -p ralph-core -E 'test(/u1_(prune|fix_applied|dedup_helper)/)'
cargo nextest run -p ralph-core -E 'test(/u2_(prepend|human_guidance|update_robot|apply_robot)/)'
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_serial_review_coordinator_fix_applied
cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_serial_plan_gate_must_not_listen_to_fix_applied
cargo nextest run -p ralph-core -E 'test(/u5_/)'
cargo nextest run -p ralph-core -E 'test(/u6_fix_applied/)'
cargo nextest run -p ralph-core --test scenarios ce_executor_serial_fix_applied_rereview
./scripts/run-tests.sh
```