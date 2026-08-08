# Preset Author Notes — post-merge-converge

## Revision 2026-08-09 (U4 mixed history / unknown hard gate)

- change-mapper: D6 commit attribution (plan_owned / interleaved / unknown) via `git rev-list --topo-order --reverse`
- change-mapper: D6 hunk attribution (plan_owned / overridden_later / reverted / shared_by_multiple_plans / unknown) with hunk key normalization
- change-mapper: critical_paths union of plan expected files + candidate patch paths + explicit claim/symbol files
- change-mapper: critical unknown → `proceed:false`, `scope_status:blocked`
- change-mapper: `merge_boundary_path` + `cross_check=0` → boundary conflict blocker
- change-mapper: pre-emit HEAD/tree drift recheck before emitting changemap.ready
- change-mapper emits: `interleaved_diff_path`, `override_diff_path`, `unknown_diff_path`, `interleaved_commits_count`, `override_commits_count`, `unknown_commits_count`
- scope_status=resolved requires: `critical_unknown_count==0` AND `unknown_commits_count==0` (already in critical_unknown_count; explicit)
- system-auditor: reads scope_manifest_path and uses it to focus audit on plan_owned/shared_by_multiple_plans regions
- resolved gate: scope_status=resolved AND overall_confidence>=90 AND critical_unknown_count==0 AND no boundary conflict AND no drift

## Revision 2026-08-09 (U3 scope resolution)

- change-mapper follows D3/D5/D6 fixed scope protocol: explicit scope_base validation, merge_boundary_path as optional cross-check, first-parent/merge-parent base derivation
- scope-manifest.json written as D3 artifact with byte-stable schema
- diffs/{full,scoped,interleaved,override,unknown}.patch written under .ralph/post-merge/diffs/
- postmerge.changemap.ready carries all U1 scope fields (scope_manifest_path/digest/status/confidence/base/source)
- resolved gate: scope_status=resolved AND overall_confidence>=90 AND critical_unknown_count=0

## Revision 2026-07-26 (sequential same-branch plan coverage)

- Consumer Observe binds trigger `artifact_path` (reproducer / system-auditor / change-mapper)
- test-gap lists full `.ralph/post-merge/03…08` paths
- change-mapper treats prompt plans as authoritative and otherwise discovers ordered plans from git chronology / branch subjects, not just merge ancestry
- fixer selects Finding from `.ralph/post-merge/findings/` when `reproduce.ready` has no `finding_id`
- Artifact lifecycle declared: owner + retention + operator-owned cleanup; agents must not wipe dir

## Revision 2026-07-25 (operator report + confidence)

- Artifacts root: `.ralph/post-merge/` (not docs/)
- Hats: 8 — added `reporter` after `closer`
- `closer` emits `postmerge.reviewed`; only `reporter` emits `postmerge.complete` with required `report_path`
- Confidence redo-first on change-map / Finding / repro / root-cause / verdict; LOW_CONFIDENCE cannot VERIFIED; report lists 待核实

## Preset Intent Confirmation

- **目标：** 一个或多个开发计划已在当前最终分支/最终代码树中完成后，将当前最终代码视为完整系统，串行完成跨计划组合审计、系统级测试补充、按 Finding 最小修复、回归与干净环境验证、独立终审。
- **操作者与启动路径：** `ralph run -c ralph.post-merge.yml -H builtin:post-merge-converge`；`ralph.post-merge.yml` 指向 `.ralph/post-merge.prompt.md`（gitignore）；仓库内 `post-merge.prompt.md` 为可复制模板。
- **输入与事实源：** 最少输入=当前最终分支/最终代码树且基线可验证；prompt 可给计划列表，未给则 git log / commit chronology / branch subjects 匹配 `docs/plans/`，同分支顺序完成的计划也在范围内。
- **成功条件：** Final Review `PASS` 或 `PASS WITH ACCEPTED RISKS`；P0/P1 关闭或 accepted；干净环境通过。
- **阻塞条件：** 基线无效或 P0/P1 未关闭且未 accepted → `postmerge.fix.ready{passed:false}` → closer `FAIL` / `success:false`（merge-batch 式全路径收敛，不用 plan.blocked 旁路）。
- **允许的修改范围：** 审计/地图/test-gap/终审只读生产代码；reproducer 只写失败测试；fixer 按 Finding 最小修复。
- **必须独立执行的评审：** closer 的 Final Review，不补前面工作。
- **重要 artifact：** `.ralph/post-merge/01…15` + `findings/PMI-*.md` + `REPORT.md`。
- **execution_model：** single-chain  
  **why：** 严格串行，无 wave/supervisor 需求。
- **非目标：** 不重跑各计划、不重新 merge、不绑语言/CI。
- **用户确认：** 6.1 已确认（2026-07-25）。

## Fusion notes（15→8，保留步骤）

| Hat | PDF steps | 融合注意 |
|---|---|---|
| baseline | 01 | 未合并 |
| change-mapper | 02 | 未合并；计划发现=prompt 优先否则 git  chronology / branch subjects |
| system-auditor | 03–08 | **六份独立文档仍分写**；只读；Finding 在此创建 |
| test-gap | 09 | 只设计不实现失败测试 |
| reproducer | 10 | 自环；一 Finding/activation；禁改生产代码 |
| fixer | 11–13 | **同 activation 内强制顺序** 根因→修复→回归；回归失败发 `fix.next` 再进，不在「回归心态」里继续改 |
| closer | 14–15 | **先 14 后 15**；终审不补活；发 `postmerge.reviewed` |
| reporter | （收口） | 写 `.ralph/post-merge/REPORT.md`；唯一 `postmerge.complete`（必填 `report_path`） |

Hard questions — single-chain-first: ✓（无 supervisor/wave）  
Hard questions — wave / supervisor: N/A  
Hard questions — Artifact-First: ✓（`.ralph/post-merge/` 为业务 artifact；event 只带路径/短状态；lifecycle 已声明）

## Hat: baseline

- **Q1 使命:** 固定合入后基线，只记录不分析不改码
- **Q2 输入:** prompt 分支/验证命令；`git` 状态；现有测试
- **Q3 执行:** Observe 仓库 → Precheck 无 merge 中 → Apply 跑现状验证并写 `.ralph/post-merge/01-baseline.md` → Confirm emit
- **Q4 输出:** `postmerge.baseline.ready`
- **Q5 交接:** `artifact_path` → change-mapper `cat` 该路径

### Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| postmerge.baseline.ready | baseline_valid | bool | 完成门禁 | 本 hat 计算 | 不涉及 | change-mapper 短路 | field_docs | 不落盘 + 短控制字段 |
| postmerge.baseline.ready | artifact_path | path | 本 hat 写入 | 本 hat | 不涉及 | 读完整基线 | field_docs | 必填 · `.ralph/post-merge/01-baseline.md` |
| postmerge.baseline.ready | head_sha | string | git rev-parse | 本 hat | 禁手写 | 审计/终审对照 | field_docs | 不落盘 + 可从 git 重算；全文环境在 artifact |
| postmerge.baseline.ready | branch | string | git / prompt | 本 hat | 不涉及 | 同上 | field_docs | 不落盘 + 短路由 |

## Hat: change-mapper

- **Q1 使命:** 建立整体变更地图
- **Q2 输入:** 读 trigger `artifact_path`（baseline）；计划列表或 git 发现（按 commit chronology / branch subjects 排序，同分支顺序完成也在范围内）
- **Q3:** 短路面 → 建四表 → emit
- **Q4:** `postmerge.changemap.ready`
- **Q5:** trigger `artifact_path`（地图）→ system-auditor

### Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| postmerge.changemap.ready | proceed | bool | baseline_valid+地图完成（按 prompt / git chronology） | trigger+本 hat | 不涉及 | auditor 短路 | field_docs | 不落盘 + 短控制 |
| postmerge.changemap.ready | artifact_path | path | 本 hat 写入 | 本 hat | 不涉及 | 读四表 | field_docs | 必填 · `02-change-map.md` |
| postmerge.changemap.ready | high_risk_crossings_count | int | 高风险表行数 | artifact | 不涉及 | 审计优先级 | field_docs | 不落盘 + 可从 artifact 重算 |
| postmerge.changemap.ready | scope_manifest_path | path | change-mapper writes scope-manifest.json | 本 hat | 不涉及 | 审计读取 resolved scope | field_docs | 必填 · `.ralph/post-merge/scope-manifest.json` |
| postmerge.changemap.ready | scope_digest | string | SHA-256 of scope-manifest.json bytes | 本 hat | 禁手写 | 一致性核验 | field_docs | 不落盘 + 可重算 |
| postmerge.changemap.ready | scope_status | enum | change-mapper 解析决策 | 本 hat | 不涉及 | resolved gate | allowed_values | 不落盘 + 控制门禁 |
| postmerge.changemap.ready | overall_confidence | int | D7 confidence score | 本 hat | 不涉及 | resolved gate | field_docs | 不落盘 + 0/85/90/100 |
| postmerge.changemap.ready | critical_unknown_count | int | 未解关键归属数 | 本 hat | 不涉及 | resolved gate | field_docs | 不落盘 + resolved 须为 0 |
| postmerge.changemap.ready | scope_base_sha | string | Git SHA of scope base | 本 hat | 须为 ancestor | 下游 diff 起点 | field_docs | 不落盘 + 40-char hex |
| postmerge.changemap.ready | scope_source | enum | 如何推导 base | 本 hat | 不涉及 | 溯源 | allowed_values | 不落盘 + explicit/direct-target/branch-merge/inferred |

## Hat: system-auditor

- **Q1 使命:** 串行完成契约/状态/生命周期/配置/失败恢复/并发幂等六维只读审计并建 Finding
- **Q2 输入:** `02-change-map.md` + baseline
- **Q3:** 六段审计各写文档 + PMI files → emit
- **Q4:** `postmerge.audit.ready`
- **Q5:** docs 03–08 + findings → test-gap

### Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| postmerge.audit.ready | proceed | bool | 上游+六文档 | 本 hat | 不涉及 | test-gap | field_docs | 不落盘 + 短控制 |
| postmerge.audit.ready | findings_created_count | int | findings 目录 | 本 hat | 不涉及 | 下游规模预期 | field_docs | 必填关联 · `findings/PMI-*.md` |
| postmerge.audit.ready | audit_docs_complete | bool | 03–08 存在 | 本 hat | 不涉及 | 门禁 | field_docs | 必填 · 六份 audit md |

## Hat: test-gap

- **Q1 使命:** 风险→可执行测试计划（不修码、不写失败测试实现）
- **Q2:** 读 03–08 + findings
- **Q3:** 写 `09-test-gap-plan.md` → emit
- **Q4:** `postmerge.testplan.ready`
- **Q5:** plan → reproducer

### Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| postmerge.testplan.ready | proceed | bool | 上游+计划完整 | 本 hat | 不涉及 | reproducer | field_docs | 不落盘 + 短控制 |
| postmerge.testplan.ready | artifact_path | path | 本 hat | 本 hat | 不涉及 | 读场景 | field_docs | 必填 · `09-test-gap-plan.md` |
| postmerge.testplan.ready | p0_p1_scenario_count | int | 计划 P0/P1 段 | artifact | 不涉及 | 工作量 | field_docs | 不落盘 + 可重算 |

## Hat: reproducer

- **Q1 使命:** 每 Finding 固化稳定失败测试；禁修生产代码
- **Q2:** test plan + findings；自环 `reproduce.next`
- **Q3:** 单 Finding 复现 → 更新状态 → next 或 ready
- **Q4:** `postmerge.reproduce.next` / `postmerge.reproduce.ready`
- **Q5:** REPRODUCED findings → fixer

### Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| postmerge.reproduce.next | finding_id | string | Finding 文件 | 本 hat | 须存在 PMI | 自环续跑 | field_docs | 必填关联 · Finding + 失败测试路径写在 Finding 内 |
| postmerge.reproduce.next | remaining_count | int | Finding 板 | 本 hat | 不涉及 | 自环 | field_docs | 不落盘 + 短控制 |
| postmerge.reproduce.ready | proceed | bool | 上游+板状态 | 本 hat | 不涉及 | fixer | field_docs | 不落盘 + 短控制 |
| postmerge.reproduce.ready | reproduced_count | int | 板 | 本 hat | 不涉及 | fixer | field_docs | 不落盘 + 可从板重算 |
| postmerge.reproduce.ready | unreproducible_count | int | 板 | 本 hat | 不涉及 | fixer/closer | field_docs | 不落盘 + 详情在 Finding |

## Hat: fixer

- **Q1 使命:** 单 Finding：根因→最小修复→分层回归
- **Q2:** reproduce.ready / fix.next + Finding + 失败测试
- **Q3:** 11→12→13 顺序；回归失败发 fix.next
- **Q4:** `postmerge.fix.next` / `postmerge.fix.ready`
- **Q5:** `fix.ready` → closer；`13-regression-report.md` 供终审

### Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| postmerge.fix.next | finding_id | string | 当前 Finding | 本 hat | 须存在 | 自环 | field_docs | 必填关联 · Finding 更新 |
| postmerge.fix.next | remaining_count | int | 板 | 本 hat | 不涉及 | 自环 | field_docs | 不落盘 + 短控制 |
| postmerge.fix.next | last_status | enum | Finding 状态 | Finding | 不涉及 | 自环策略 | allowed_values | 不落盘 + 状态在 Finding |
| postmerge.fix.ready | passed | bool | open_p0_p1_count==0 | 本 hat | 不涉及 | closer | field_docs | 不落盘 + 短控制 |
| postmerge.fix.ready | verified_count | int | 板 | 本 hat | 不涉及 | closer | field_docs | 不落盘 + 可重算 |
| postmerge.fix.ready | open_p0_p1_count | int | 板 | 本 hat | 不涉及 | closer 门禁 | field_docs | 不落盘 + 可重算 |
| （回归正文） | — | — | 本 hat | — | — | closer | — | 必填 · `13-regression-report.md` |

## Hat: closer

- **Q1 使命:** 干净环境验证 + 独立终审；不写 REPORT、不 emit complete
- **Q2:** `fix.ready` + 全部 `.ralph/post-merge/**`
- **Q3:** 先 14 后 15 → emit reviewed
- **Q4:** `postmerge.reviewed`（`verdict` / `verdict_confidence` / `final_review_path` / `clean_validation_path`）
- **Q5:** → reporter；操作者读 `15-final-review.md`

### Payload Contract

| topic | 字段 | 类型 | 值源 | 可见性 | 身份检查 | 下游消费 | schema metadata | artifact 落盘 |
|---|---|---|---|---|---|---|---|---|
| postmerge.reviewed | verdict | enum | `.ralph/post-merge/15-final-review.md` 结论 | 本 hat | 不涉及 | reporter | allowed_values + field_docs | 必填正文 · `15-final-review.md` |
| postmerge.reviewed | verdict_confidence | enum | 审稿人判定 | 本 hat | 不涉及 | reporter 透传 | allowed_values + field_docs | 不落盘 + 短控制 |
| postmerge.reviewed | final_review_path | path | 本 hat 写入 | 本 hat | 不涉及 | reporter `cat` | field_docs | 必填 · `15-final-review.md` |
| postmerge.reviewed | clean_validation_path | path | 本 hat 写入 | 本 hat | 不涉及 | reporter `cat` | field_docs | 必填 · `14-clean-validation.md` |

## Hat: reporter

- **Q1:** 操作者报告：结论、做了什么、产物路径、待核实、下一步
- **Q2:** `postmerge.reviewed` + `.ralph/post-merge/**`
- **Q3:** 写 REPORT → policy-check → complete（Confirm 必须暴露 report_path）
- **Q4:** `postmerge.complete` 必填 `report_path`
- **Q5:** 对人；loop 结束

### Payload Contract (reporter)

| topic | 字段 | artifact 落盘 |
|---|---|---|
| postmerge.complete | report_path | 必填 · `.ralph/post-merge/REPORT.md` |
| postmerge.complete | success/verdict | 不落盘 + 短控制；正文在 REPORT |

## 7-point sync checklist（builtin）

1. runtime step-close：无旧终态依赖，新 topic 自洽 ✓  
2. preset_lint：✓ 已跑（2026-07-28 评审 `builtin:post-merge-converge` 0 Error / 0 Warn；`cargo nextest -p ralph-core -- preset_lint` 267/267 PASS；`cargo nextest -p ralph-cli --bin ralph -- preset_lint` 11/11 PASS；`cargo nextest -p ralph-cli --bin ralph -- presets` 57/57 PASS）  
3. BDD：本轮未加 scenario（merge-batch 式 workflow preset）— 可后续补  
4. config 字段：无新 event_loop 全局字段 ✓  
5. CLI presets.rs + 计数测试 ✓  
6. manifest + index.json ✓  
7. CLAUDE/AGENTS + zsh ✓  

## Pre-review gate

- [x] 每 hat AAF + Payload Contract  
- [x] hat 数 notes=YAML=8  
- [x] single-chain / Artifact-First hard questions  
- [x] emitter 引用 OPAC / policy-check  
- [x] lint 实测（2026-07-28 评审通过；见 `.ralph/reviews/post-merge-converge-2026-07-28.md`）
