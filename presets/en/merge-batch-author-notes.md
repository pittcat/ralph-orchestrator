# Preset Author Notes — merge-batch

## Revision 2026-08-16 (scope contract、self-loop 与 artifact-first 修复)

- `merge.retest` 是本 preset 唯一允许的业务 self-loop：stabilizer 同时在
  `triggers` 与 `publishes` 声明该 topic，运行时据此放行；stabilizer 可以显式
  指向 `stabilizer`，但普通 isolated handoff 仍不能自指。
- integrator 新增独立的 `.ralph/merge/scope-manifest.json`，它与
  `merge-boundary.json` 分工不同：前者是下游是否可继续的 scope 合同，后者
  是 merge window 的交叉证据。
- `merge.integrated`、`merge.stabilized`、`merge.batch.complete` 必须传递并
  校验 `scope_manifest_path`、`scope_digest`、`scope_status`、`scope_base_sha`、
  `scope_source`、`overall_confidence`、`critical_unknown_count`、`proceed`。
- scope manifest 的 `scope_digest` 计算方式固定为：去掉自身 digest 字段后，
  对 canonical JSON（sorted keys、compact、末尾一个 newline）做 SHA-256。
- `proceed=true` 仅允许在 integration complete、scope resolved、
  `overall_confidence >= 90` 且 `critical_unknown_count == 0` 时出现。
- stabilizer 在每次 attempt 前必须读取并验收 `integration_path` 与
  `scope_manifest_path`，不能只相信 trigger 中存在路径。

- integrator writes `.ralph/merge/merge-boundary.json` after each `git merge --no-ff`
  - captures target identity (pre/post merge SHA + tree) per branch
  - computes `boundary_digest` (SHA-256 of canonical JSON, `boundary_digest` field excluded)
  - `boundary_status: complete` when every branch merged; `incomplete` when any branch failed
- boundary artifact is required event evidence for this preset, but is **NOT a downstream authority**
  - reporter reads boundary to cross-verify SHA/tree consistency
  - missing boundary or digest mismatch forces a failed terminal outcome; it does not authorize conclusions about commits outside the batch
  - downstream presets (post-merge-converge, red-team-attack) MUST NOT treat merge-boundary as their scope contract or required input
- schema fields added to `merge.integrated`: `merge_boundary_path`, `merge_boundary_digest`, `merge_boundary_status`
- schema fields added to `merge.stabilized`: `merge_boundary_path`, `merge_boundary_digest` (echoed from `merge.integrated`)

## Preset Intent Confirmation

- **目标：** Git-first batch merge — review design intent and conflict strategy, merge multiple worktree branches into a target branch, stabilize with verify-fix-retest self-loop, then write a merge report.
- **操作者与启动路径：** `ralph run -H builtin:merge-batch -P merge.prompt.md` 或 `ralph run -c ralph.merge.yml -H builtin:merge-batch`
- **输入与事实源：** target branch（默认 main）+ ordered branch list from `git worktree list`；prompt 不提供时用 git-first discovery
- **成功条件：** `merge.stabilized(passed: true)` + `merge.batch.complete(success: true)` + readable `.ralph/merge/REPORT.md`
- **阻塞条件：** 任何 branch merge 失败 → `integration_complete: false` → stabilizer short-circuits → reporter 写 FAIL report
- **允许的修改范围：** review.md / integration.md / stabilize-log.md / stabilize-state.json / report-input.md / scope-manifest.json / merge-boundary.json / REPORT.md
- **必须独立执行的评审：** 无独立评审 hat；linear chain reviewer → integrator → stabilizer → reporter
- **重要 artifact：**
  - `.ralph/merge/review.md` — reviewer 写，integrator 读
  - `.ralph/merge/integration.md` — integrator 写，stabilizer 读并验收
  - `.ralph/merge/scope-manifest.json` — integrator 写，stabilizer 验收，reporter 读并写入最终报告
  - `.ralph/merge/stabilize-log.md` — stabilizer 写/读，贯穿自环
  - `.ralph/merge/stabilize-state.json` — stabilizer 写/读，贯穿自环
  - `.ralph/merge/report-input.md` — stabilizer 写，reporter 读
  - `.ralph/merge/merge-boundary.json` — integrator 写，stabilizer/reporter 读并校验（event evidence）
  - `.ralph/merge/REPORT.md` — reporter 写，操作者读（required before `merge.batch.complete`）

## Hard Questions — Artifact-First

1. **每条写入型 hat 是否声明了当前 `.ralph/merge/` 下的 artifact 路径集合？** ✓ — merge-batch.yml header 注释列出 artifact 清单；每条 hat 的 instructions 明确写出了自己写入的路径。
2. **每条 consumer hat 的 instructions 是否要求显式读取 artifact？** ✓ — integrator reads `merge.reviewed.review_path`；stabilizer reads and validates `integration_path` / `scope_manifest_path` plus `stabilize-log.md` / `stabilize-state.json`；reporter reads `report_input_file` + all upstream artifacts.
3. **每条传递的长内容或摘要是否先落盘、event 只保留短状态/路径？** ✓ — merge events carry only short control fields + artifact paths; no inlined review, integration log, or stabilize state.
4. **是否有 hat 把 runtime internal ledger（`.ralph/events.jsonl` / `.ralph/supervisor.db`）当作自定义状态？** ✗ — guardrails 明确禁止；所有 hat 状态通过 trigger payload 和 `.ralph/merge/` business artifact 传递。
5. **scope manifest 是否独立落盘并被 downstream 消费？** ✓ — integrator 写入并计算 digest，stabilizer 读取、验收并保持字段不升级，reporter 再读盘并把 scope 身份写入最终报告。
6. **merge-boundary artifact 的权威边界是否清晰？** ✓ — event schema 要求 path/digest 可读且一致；它是 merge batch 内的必需交接证据，但不替代 scope manifest，也不裁决 batch 外的 direct commits。

## Hard Questions — single-chain / wave / supervisor

- execution_model: single-chain — linear hat chain with one stabilizer self-loop
- wave: N/A
- supervisor: N/A

## Payload Contract Summary

| topic | hat | 必填 artifact 落盘 | 说明 |
|---|---|---|---|
| merge.reviewed | reviewer | `.ralph/merge/review.md` | design intent, risks, conflict strategy |
| merge.integrated | integrator | `.ralph/merge/integration.md` / `scope-manifest.json` | merge order, SHAs, conflicts; boundary + 独立 scope contract |
| merge.retest | stabilizer | `.ralph/merge/stabilize-log.md` / `stabilize-state.json` | self-loop retry signal |
| merge.stabilized | stabilizer | `.ralph/merge/report-input.md` / `scope-manifest.json` | passed + evidence manifest + boundary/scope echo |
| merge.batch.complete | reporter | `.ralph/merge/REPORT.md` / `scope-manifest.json` | terminal completion token + final scope identity |

## 7-Point Sync Checklist

1. runtime step-close：无新终态语义，`merge.batch.complete` 走标准 completion_promise 路径
2. preset_lint：boundary、scope fields 已加到 schema SSOT；确认 payload_consistency 规则、self-loop topology 与 schema 对齐
3. BDD / verify：success、integration failure、stabilizer self-loop、no-progress 与 terminal closure 场景必须携带完整 scope contract；旧 fixture 不得继续作为 v1 证据
4. config 字段：无新 event_loop 全局字段
5. CLI presets.rs：无新增 preset，无需变更
6. manifest + index.json：无新增 preset，无需变更
7. CLAUDE/AGENTS + zsh：无新增 preset，无需变更

## 与 Unit 1 的边界

Unit 1 落地了：
- schema `merge_boundary_path` / `merge_boundary_digest` / `merge_boundary_status` 字段定义
- `payload_consistency` 规则（boundary path root、status allowed、stabilized echo）

Unit 2 落地了：
- integrator instructions：实际写入 boundary JSON 的步骤 + digest 计算
- reporter instructions：boundary cross-check（读 path + recompute digest）
- author notes：boundary artifact 语义澄清（required event evidence, NOT downstream authority）
- operator prompt 更新：说明 boundary 只描述 batch 窗口，不声明后续 direct commits
- BDD fixture：`merge_batch_boundary.yml`（success path + failure path）

Unit 3+ 将处理：post-merge-converge 独立解析 direct-target scope（不依赖 merge-boundary 作为 required input）
