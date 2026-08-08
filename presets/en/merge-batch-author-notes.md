# Preset Author Notes — merge-batch

## Revision 2026-08-09 (Unit 2: merge-boundary artifact)

- integrator writes `.ralph/merge/merge-boundary.json` after each `git merge --no-ff`
  - captures target identity (pre/post merge SHA + tree) per branch
  - computes `boundary_digest` (SHA-256 of canonical JSON, `boundary_digest` field excluded)
  - `boundary_status: complete` when every branch merged; `incomplete` when any branch failed
- boundary artifact is **OPTIONAL cross-check evidence** — NOT a downstream authority
  - reporter reads boundary to cross-verify SHA/tree consistency
  - missing boundary or digest mismatch is recorded in report but does NOT change completion outcome
  - downstream presets (post-merge-converge, red-team-attack) MUST NOT treat merge-boundary as a required input
- schema fields added to `merge.integrated`: `merge_boundary_path`, `merge_boundary_digest`, `merge_boundary_status`
- schema fields added to `merge.stabilized`: `merge_boundary_path`, `merge_boundary_digest` (echoed from `merge.integrated`)

## Preset Intent Confirmation

- **目标：** Git-first batch merge — review design intent and conflict strategy, merge multiple worktree branches into a target branch, stabilize with verify-fix-retest self-loop, then write a merge report.
- **操作者与启动路径：** `ralph run -H builtin:merge-batch -P merge.prompt.md` 或 `ralph run -c ralph.merge.yml -H builtin:merge-batch`
- **输入与事实源：** target branch（默认 main）+ ordered branch list from `git worktree list`；prompt 不提供时用 git-first discovery
- **成功条件：** `merge.stabilized(passed: true)` + `merge.batch.complete(success: true)` + readable `.ralph/merge/REPORT.md`
- **阻塞条件：** 任何 branch merge 失败 → `integration_complete: false` → stabilizer short-circuits → reporter 写 FAIL report
- **允许的修改范围：** review.md / integration.md / stabilize-log.md / stabilize-state.json / report-input.md / merge-boundary.json / REPORT.md
- **必须独立执行的评审：** 无独立评审 hat；linear chain reviewer → integrator → stabilizer → reporter
- **重要 artifact：**
  - `.ralph/merge/review.md` — reviewer 写，integrator 读
  - `.ralph/merge/integration.md` — integrator 写，stabilizer 读
  - `.ralph/merge/stabilize-log.md` — stabilizer 写/读，贯穿自环
  - `.ralph/merge/stabilize-state.json` — stabilizer 写/读，贯穿自环
  - `.ralph/merge/report-input.md` — stabilizer 写，reporter 读
  - `.ralph/merge/merge-boundary.json` — integrator 写，reporter 选读（OPTIONAL cross-check）
  - `.ralph/merge/REPORT.md` — reporter 写，操作者读（required before `merge.batch.complete`）

## Hard Questions — Artifact-First

1. **每条写入型 hat 是否声明了当前 `.ralph/merge/` 下的 artifact 路径集合？** ✓ — merge-batch.yml header 注释列出 artifact 清单；每条 hat 的 instructions 明确写出了自己写入的路径。
2. **每条 consumer hat 的 instructions 是否要求显式读取 artifact？** ✓ — integrator reads `merge.reviewed.review_path`；stabilizer reads `stabilize-log.md` / `stabilize-state.json` / `merge.integrated` payload；reporter reads `report_input_file` + all upstream artifacts.
3. **每条传递的长内容或摘要是否先落盘、event 只保留短状态/路径？** ✓ — merge events carry only short control fields + artifact paths; no inlined review, integration log, or stabilize state.
4. **是否有 hat 把 runtime internal ledger（`.ralph/events.jsonl` / `.ralph/supervisor.db`）当作自定义状态？** ✗ — guardrails 明确禁止；所有 hat 状态通过 trigger payload 和 `.ralph/merge/` business artifact 传递。
5. **merge-boundary artifact 是否在 schema 中声明为可选交叉验证、不是下游权威？** ✓ — schema `field_docs.merge_boundary_path` 说明"real readable path only; reject emit if file is missing"（文件存在性检查是 schema 对字段存在性的校验，不影响 reporter 是否将 boundary 作为 required input）；reporter instructions 明确 boundary is OPTIONAL cross-check evidence, NOT a downstream authority。

## Hard Questions — single-chain / wave / supervisor

- execution_model: single-chain — linear hat chain with one stabilizer self-loop
- wave: N/A
- supervisor: N/A

## Payload Contract Summary

| topic | hat | 必填 artifact 落盘 | 说明 |
|---|---|---|---|
| merge.reviewed | reviewer | `.ralph/merge/review.md` | design intent, risks, conflict strategy |
| merge.integrated | integrator | `.ralph/merge/integration.md` | merge order, SHAs, conflicts; **新增** `merge_boundary_path/digest/status` |
| merge.retest | stabilizer | `.ralph/merge/stabilize-log.md` / `stabilize-state.json` | self-loop retry signal |
| merge.stabilized | stabilizer | `.ralph/merge/report-input.md` | passed + evidence manifest; **新增** `merge_boundary_path/digest` echo |
| merge.batch.complete | reporter | `.ralph/merge/REPORT.md` | terminal completion token |

## 7-Point Sync Checklist

1. runtime step-close：无新终态语义，`merge.batch.complete` 走标准 completion_promise 路径
2. preset_lint：boundary fields 已加到 schema SSOT，preset_lint 需确认 `merge.integrated` 和 `merge.stabilized` 的 payload_consistency 规则与 schema 对齐
3. BDD：新增 `merge_batch_boundary.yml` fixture（success path + failure path）
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
- author notes：boundary artifact 语义澄清（OPTIONAL cross-check, NOT downstream authority）
- operator prompt 更新：说明 boundary 只描述 batch 窗口，不声明后续 direct commits
- BDD fixture：`merge_batch_boundary.yml`（success path + failure path）

Unit 3+ 将处理：post-merge-converge 独立解析 direct-target scope（不依赖 merge-boundary 作为 required input）
