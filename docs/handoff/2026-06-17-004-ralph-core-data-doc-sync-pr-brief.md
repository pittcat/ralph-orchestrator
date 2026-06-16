---
title: "feat: ralph-core/data agent 文档闭环（loop 纠偏）"
type: pr-brief
status: ready-for-review
date: 2026-06-17
worktree: .worktrees/004
branch: feat/2026-06-17-004-ralph-core-data-doc-sync
base: pittcat-dev
plan: docs/plans/2026-06-17-004-feat-ralph-core-data-doc-sync-plan.md
parallel_with: docs/plans/2026-06-17-005-fix-agent-recovery-mechanism-gaps-plan.md
---

# PR Brief: feat: ralph-core/data agent 文档闭环（loop 纠偏）

## 摘要

在近两个月机制已落地（`task.resume` 路由、step handoff gate、incomplete wave `plan.blocked` 等）的前提下，补 **agent 每轮可见** 的修复说明书：

- **P0** `ralph-tools.md`（auto-inject）新增「收到 `task.resume` 时」段 + 修正 unsafe 误导 + 修正所有 `*.rs:NN` 行号引用
- **P1** 扩展按需 `ralph-tools-emit.md`（NULL_PAYLOAD_REJECT_TOPICS、policy-check 边界）+ 新建按需 `ralph-tools-handoff.md`（ce-executor handoff 深参考）
- **P1** `skill_registry` 注册 handoff 为 builtin（不加入 auto-inject 白名单）
- **P2** 扩展 `runtime-diagnosis.md` §12.1「emit rejection → task.resume → 修复」决策树
- **P0** 6 个新测试覆盖 Tier 1 内容锚点 + Tier 2 `build_prompt` 注入断言

**不改 runtime**；与 Plan B（017-005）分 PR。

---

## 改动统计

```
3 commits on feat/2026-06-17-004-ralph-core-data-doc-sync
  b349c9d docs(ralph-tools, data-sync): loop 纠偏 R0 + emit/handoff 深参考（plan 004）
  33184c7 docs(guide, runtime-diagnosis): §12.1 emit rejection → task.resume 决策树（plan 004 U5）
  82f9d1c test(ralph-core, agent-tools): Tier 1 锚点 + Tier 2 build_prompt 注入断言（plan 004 U6）

7 files changed, 427 insertions(+), 6 deletions(-)
```

| 文件 | 改动 |
|------|------|
| `crates/ralph-core/data/ralph-tools.md` | +R0 段、unsafe 误导修正、行号修正、速查表加 handoff（166 → 183 行，预算 ≤200）|
| `crates/ralph-core/data/ralph-tools-emit.md` | +文首指针、+9 topic NULL_PAYLOAD 表、+policy-check 边界、+修复表对齐、+诊断短链 |
| `crates/ralph-core/data/ralph-tools-handoff.md` | **新建** 107 行 — 7 节：先读 R0 / topic 归属 / progress 修复 / handoff timeout / wave 收摊 / plan.blocked provenance / 校验命令 / 相关文档 |
| `crates/ralph-core/src/skill_registry.rs` | +`include_str!` const、+`register_builtin("ralph-tools-handoff")`、+2 单测（注册 + 不注入断言）|
| `crates/ralph-core/src/event_loop/tests/build_prompt.rs` | +3 测：isolated R0 注入 / tasks-only 分支 / handoff 不注入 |
| `crates/ralph-cli/tests/integration_agent_reference.rs` | +3 测：list handoff / load handoff 锚 / load ralph-tools R0 锚 |
| `docs/guide/runtime-diagnosis.md` | +§12.1「emit rejection → task.resume → 修复」决策树（含 ASCII 决策树图）|

---

## 验收 Tiers

### Tier 1 — CI 硬门

| 检查 | 命令 / 位置 | 结果 |
|------|------------|------|
| 行数预算 | `wc -l crates/ralph-core/data/ralph-tools.md` ≤ 200 | ✅ 183 |
| 行号审计 | 7 处 `*.rs:NN` sed 复核 | ✅ 全部命中 |
| 内容锚点 | ralph-tools 含 `收到 task.resume 时` / `required_fields` / `--policy-check`，不含旧 unsafe 首选 | ✅ grep -c = 1/3/3/0 |
| CLI 冒烟 | `ralph tools skill list` (含 handoff) / `load ralph-tools-handoff` / `ralph emit --help` | ✅ 全部通过 |
| 回归 | `cargo nextest run -p ralph-core -p ralph-proto -p ralph-adapters -p ralph-telegram -p ralph-tui -p ralph-api` | ✅ 3163/3163 |
| 回归 | `cargo nextest run -p ralph-cli`（串行） | ✅ 1272/1272（1 leaky / 3 skipped 与本改动无关）|

### Tier 2 — 运行时注入链

`EventLoop::build_prompt` 在 isolated 模式下断言：
- prompt 含 `<ralph-tools-skill>` 块
- 块内含 R0 段头「收到 `task.resume` 时」、`required_fields`、`--policy-check`
- 块内**不含**旧表述「确认配置允许 `--unsafe-no-policy-check`」

→ `test_build_prompt_injects_ralph_tools_skill_r0_block` ✅
→ `test_build_prompt_injects_ralph_tools_via_tasks_only` ✅
→ `test_handoff_skill_indexed_but_not_auto_injected` ✅

### Tier 3 — 人工 dogfood（合并前勾选，不进 CI）

```
- [ ] 故意缺字段 `ralph emit work.ready`，确认 stderr / R0 指引走 `--policy-check`
- [ ] loop 内出现 `task.resume`，肉眼确认 PENDING EVENTS + 注入段可同时读到
- [ ] handoff 复杂 violation 时，`skill load ralph-tools-handoff` 可加载
```

---

## 反向验证（CLAUDE.md 硬门）

7 处 `*.rs:NN` 引用 sed 复核：

| 引用 | 位置 | 内容 | 状态 |
|------|------|------|------|
| `event_loop/mod.rs:4862-4873` | ralph-tools.md L10 | ralph-tools 注入块 | ✅ |
| `rejection.rs:324-398` | ralph-tools.md L23 | `pub fn build_task_resume_payload` | ✅ |
| `hats.rs:170` | ralph-tools.md L56 | `fn validate_hats<W: Write>` | ✅ |
| `skill_cli.rs:78-87` | ralph-tools.md L58 | `RALPH_CURRENT_HAT` fail-closed | ✅ |
| `emit_path.rs:32-120` | ralph-tools.md L62 | `pub(crate) fn resolve_emit_path` | ✅ |
| `wave.rs:551-560` | ralph-tools.md L71 | `pub fn resolve_events_file` | ✅ |
| `event_policy.rs:502-512` | ralph-tools-emit.md L64 | `pub const NULL_PAYLOAD_REJECT_TOPICS` + 9 topic | ✅ |

---

## 关键设计决策（来自 plan KTD）

- **KTD1** 文档与机制分 PR：本计划只改 `data/` + guide + tests；`progress_task_gate` CLI 预检、`plan.blocked` provenance 等机制边角归 017-005
- **KTD2** R0 在 `ralph-tools.md` 自动注入：每轮可见 ROI 最高；handoff skill 不能替代
- **KTD3** handoff 仅按需 load：不 patch preset、不扩 `prepend_auto_inject_skills` 白名单（KTD3 单测强制）
- **KTD4** 禁止 bypass 文档：移除 `--unsafe-no-policy-check` 作为首选修复的表述
- **KTD5** `ralph-tools.md` ≤200 行：token 预算；R0 + Governance 去重压缩
- **KTD6** 三层验收：Tier 1+2 进 CI / Tier 3 人工 checklist

---

## 风险 & 缓解

| 风险 | 缓解 |
|------|------|
| R0 + Governance 超 200 行 | 合并重复、压缩表格（已 183 行）|
| 与 017-005 文档重复 | emit 文档写明 policy-check 边界；guide §12.1 互链分工 |
| 行号再次漂移 | R1 sed 表 + CLAUDE.md 反向验证规则（本次已 7/7 复核）|

---

## 合并前人工决策项

- [ ] 审 PR diff（worktree `.worktrees/004`，3 commits）
- [ ] 跑 Tier 1 复核脚本（行数 + sed + 内容锚点）
- [ ] 选 Tier 3 checklist 一项做 dogfood（建议：故意缺字段跑 `ralph emit work.ready`）
- [ ] 决定是否合入 `pittcat-dev`（agent 无权执行 merge，等人工 `git merge` / `gh pr merge`）
- [ ] 合并后**姊妹 PR 017-005** 仍可独立 review / 合并（与 004 不互锁）

---

## Sources

- 计划：`docs/plans/2026-06-17-004-feat-ralph-core-data-doc-sync-plan.md`
- 脑暴：`docs/brainstorms/2026-06-17-ralph-core-data-ce-executor-sync-requirements.md`
- 关联：`docs/plans/2026-06-17-002-feat-ce-executor-step-handoff-plan.md` / `docs/plans/2026-06-17-003-fix-ce-executor-wave-stall-bypass-plan.md` / `docs/plans/2026-06-17-005-fix-agent-recovery-mechanism-gaps-plan.md`
- 复盘：`docs/report/2026-06-16-systematic-review-of-recent-fixes.md`
- 关键源：`crates/ralph-core/src/event_loop/{mod,rejection}.rs`、`crates/ralph-core/src/event_policy.rs`、`crates/ralph-core/src/skill_registry.rs`
