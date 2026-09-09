---
title: "fix-forge-dag-p1-closure-plan 开发执行汇报（Ralph 阻塞）"
date: "2026-09-09"
status: "BLOCKED"
final_audit: "BLOCKED"
target_branch: "pittcat-dev"
base_commit: "7ede210e"
final_commit: "ba7d7ab9"
reporter: "Reporter"
plan_key: "2026-09-09-0917-fix-forge-dag-p1-closure-plan"
trigger: "forge.cleanup.done"
cleanup_status: "retained_for_diagnosis"
cleanup_report_path: ".ralph/forge/2026-09-09-0917-fix-forge-dag-p1-closure-plan/cleanup.md"
block_artifact_path: ".ralph/forge/2026-09-09-0917-fix-forge-dag-p1-closure-plan/blocks/ralph-blocked.md"
prior_block_artifact_path: ".ralph/forge/2026-09-09-0917-fix-forge-dag-p1-closure-plan/blocks/inspector-blocked.md"
prior_manager_report: "docs/reports/2026-09-09-2026-09-09-0917-fix-forge-dag-p1-closure-plan-manager-report.md"
---

# fix-forge-dag-p1-closure-plan 开发执行汇报（Ralph 阻塞）

> **本报告是「Ralph loop_stalled_max_iterations → 清理收口」路径的产物**。本计划在基线 `7ede210e` 下进入 loop primary-20260909-082921 派工，dispatcher hats 跑通了 inspector / planner / guardian 三个阶段并发出 `forge.concurrency.approved`，但下游 U3 / U9 executor 与 reviewer activations 在 iteration 4 撞上两件 runtime 故障：`dag_runtime` 虚拟消费者被 U16 handoff 误判为 misrouted，以及 isolated hat-channel merge 在 `guardian` 写出后回退到 main 事件流失败。loop 在 `loop_stalled_max_iterations` 上阻断并由 operator-side ralph hat 发出 `forge.plan.blocked`，cleanup 接到后保留了 integration 分支 (`ralph/2026-09-09-0917-fix-forge-dag-p1-closure-plan` @ `f5e5aabf`) 以备后续诊断与 re-issue。
>
> **本仓库中不存在的产物**（按 reporter 模板与 hat instructions 列出，但本次未生成）：`units/*-completion.md`、`waves/*/settlement.md`、`waves/*/review.md`、`integration-log.md`、`full-verification.md`、`final-audit.md`、`context_artifact_path`。下文凡涉及上述产物一律标注「未生成」或「不存在」，不伪造、不夸大。`development-plan.md` 与 `execution-plan.yml` 存在但其内容从未被本 loop 完整执行（planner 在 §4.1 inspection 之前已生成的 draft 留在磁盘上，U3 wiring 仍是占位、U4–U15 仍是序号桩）。

## 1. 一句话结论

- 任务是否完成：**未完成（Ralph 阻塞）**。plan 在 `forge.concurrency.approved` 之后、`forge.worktrees.ready` 之前停止；U3 与 U9 在 DAG runtime 派单后既未到达 `forge.unit.executed` 也未到达 `forge.unit.execution_failed`。
- 核心功能是否交付：**未交付**。U1/U2/U3 schema 与 `dag_runtime` 虚拟消费者修复已在基线 `7ede210e` 落地（`2bbea7bf` / `b893fc06` / `f5e5aabf` / `7ede210e`），U3 runtime wiring 仍待本计划执行；U4–U15 完全未开始。
- 全量测试是否通过：**信息缺失；无法确认**。本计划未触发任何 acceptance gate；当前 HEAD `ba7d7ab9` 的测试基线由既有 commit message 自报，block 描述的「`event_origin` 43/43 + `handoff_dispatch` 18/18 + `preset_lint::workflow_activation` 20/20 + `dag_scheduler` 166/166 + `RUSTFLAGS='-D warnings' cargo check` clean」属于 hotfix 自身的单包验证，不构成本计划的全量门禁。
- 需要关注的风险：**当前 `ralph run` 进程内存里仍持有 hotfix 前的 binary**（block 描述 §1 第 2 段），U3/U9 已 `job_terminal=failed` 写在 `.ralph/dag.db` 而 main events 收不到 `forge.unit.execution_failed`；operator 必须按 block §Operator action 1-2 重启 loop 才能用 `7ede210e` 的 post-fix binary 重新派发。**另外** `isolated hat-channel empty-after-activation` 是独立 runtime bug，已被显式排除在 P1 范围外（block 描述 §Out-of-scope follow-ups）。

## 2. 管理摘要

| 项目 | 结果 |
|---|---|
| 最终状态 | **阻塞**（Ralph loop 在 iteration 4 因 `loop_stalled_max_iterations` 阻断） |
| 原计划是否调整 | **是**（计划文件已在基线 `7ede210e` 下被前一份 USABLE inspection 重新核对，文档层与 `7ede210e` 一致；运行时由 operator hotfix 推进了 `ba7d7ab9`，仍按 re-issue 路径处理） |
| 计划内 Scenario | 计划声称 S1–S19（info 缺失；无法确认执行明细） |
| 已通过 Scenario | 0（未触发任何 acceptance gate） |
| Unit 总数 | 计划声称 15（U1–U15） |
| 已完成 Unit | 0（按本计划派工视角；U1/U2/U3 schema 与 dag_runtime 修复来自基线外 commit，U3 runtime wiring 与 U4–U15 未开始） |
| 未完成 Unit | 15 |
| 并发执行 Unit | 0 |
| 串行执行 Unit | 0 |
| 最终 Commit 数量 | 0（本 loop 派发未产出 commit；基线 `ba7d7ab9` 来自 prior merge + hotfix） |
| 合并冲突数量 | 0 |
| 增量测试 | 未执行（本 loop 派发面） |
| 全量测试 | 未执行（本 loop 派发面；hotfix 自带单包验证见 §1） |
| 最终审计 | **BLOCKED**（无 `forge.audit.done`；block 由 `forge.plan.blocked(reason=loop_stalled_max_iterations)` 替代） |
| 是否建议进入下一阶段 | **不建议**；必须按 block §Operator action 重启 `ralph run` 让 DAG scheduler 重建 topology 并 re-admit U3 / U9 |

## 3. 本次任务要解决什么问题

- 原来存在什么问题：plan `2026-09-09-0917-fix-forge-dag-p1-closure-plan` 描述了 parallel-forge DAG 调度器在 V1 闭环中的 15 个执行点（U1 tick admission 与 live job 冲突、U2 cross-stage resource capacity accounting、U3 SpawnKind 与 base mismatch、U4–U15：U3 后续消费 schema、跨阶段持久化、approval 原子性、correction 预算、gate timeout、job context 等），详见 `docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md` §1–§2。
- 影响了谁：DAG runtime、parallel-forge preset、并发场景下的 unit 调度、跨 stage evidence 持久化、Unit 重入（resume）路径、approval 激活原子性、correction 预算不重复扣减语义。
- 本次增加、修改或修复什么：**U1/U2/U3 schema 端由基线外 commit 落地**（`b9221413` U1 slot refill、`ed926e93` U2 cross-stage resource capacity accounting、`2bbea7bf` U3 v18 migration + `CURRENT_VERSION` bump、`b893fc06` U3 release transaction fix、`f5e5aabf` U3 plan-namespaced Unit keys）；**U3 wiring 与 `dag_runtime` 虚拟消费者修复由 `7ede210e` 落地**。**本 loop primary-20260909-082921 派工没有产出任何 commit**——U3 wiring 仍待 executor 下次 activation 完成、U4–U15 完全未开始。
- 完成后的预期效果：**未达成**。plan 既未让 U3 runtime 真正接上 v18 evidence/base 存储的 Rust 端消费，也未启动 U4–U15 的执行面；当前 re-issue 路径依赖 operator 重启 loop 后由 post-hotfix binary 重建 receipt。

## 4. 原计划为什么需要调整

### 4.1 Plan refresh（基线由 `95186d4c` → `7ede210e`，commit `ba7d7ab9`）

`docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md` 在 inspector 第二次 USABLE 报告（基线 `7ede210e`，decision=USABLE）中确认：刷新仅 11 ins / 11 del 的事实层重述，与 `7ede210e` 的实际仓库状态完全一致。`7ede210e` 把 `dag_runtime` 加入 `is_virtual_runtime_consumer` allowlist 解决了本轮 block 的根因之一，但 live `ralph run` 进程仍持有 pre-fix binary，所以本 loop 派工结果仍是 `loop_stalled_max_iterations`。

### 4.2 计划文件的 5 项硬 drift 状态

| 序 | drift 项 | 计划声称 | 实际仓库 | 当前状态 |
|---|---|---|---|---|
| 1 | reviewed_head 前进 | `7ede210e`（refreshed） | HEAD `ba7d7ab9` | 一致（基线继续前进 1 个 docs commit） |
| 2 | U1/U2/U3 已合并 | 在 §0 第二段已明确为基线事实 | 5 个 commit 已在 `pittcat-dev` | 一致 |
| 3 | CURRENT_VERSION | v18（E19） | v18（`migrations.rs:76`） | 一致 |
| 4 | 迁移槽位 | v19–v23 待 U3 之后的 executor 续写 | v18.sql 已存在，v19+ 槽位保留 | 一致（planner 不可在 v18 重写） |
| 5 | Evidence / Decision ledger | E2/E3/E5/D1/D2/D3 「未来待办」 | 对应 commit 已在主线 | 与 plan refresh 一致（接受事实层重述） |

**说明**：上述 5 项 drift 已在 inspector 第二份 USABLE 报告中归类为「不构成 Planner 接手的执行阻塞」。但 §8 依赖图（`U1 → U2 → U3 → U4 → … → U15` 严格串行）与 U2–U15 "可依赖能力"措辞的描述性 drift 仍未在文档侧刷新（inspection-report §3.5 标注为「建议、非阻塞」）。本次 block 路径不强制 plan refresh。

### 4.3 依赖与并发/串行说明

- 计划 §8 假设 U1→U2→U3→U4→…→U15 严格串行；
- U1/U2/U3 schema 端已落地，U3 runtime wiring 与 U4–U15 完全未开始；本 loop 派工未越过 `forge.concurrency.approved` 边界；
- 本次未发生 Unit 拆分/合并/增删。

## 5. 最终执行方案

### 5.1 执行阶段

| 阶段 | 主要工作 | 执行方式 | 结果 |
|---|---|---|---|
| Loop start / anchor | 锚定 plan + supervisor enable | 自动 | OK（loop primary-20260909-082921） |
| Inspector 评审 | 校验 plan vs 仓库 | 只读 | **USABLE**（`inspection-report.md` decision=USABLE @ `7ede210e`） |
| Planner 派工 | 写 development-plan / execution-plan + 触发 `forge.plan.ready` | 一次性 draft | **发出 `forge.plan.ready`**（ORCHESTRATION KNOWLEDGE 记录 `accepted-event:2:0`） |
| Guardian 决策 | 写 concurrency-approval + 触发 `forge.concurrency.approved` | 一次性 | **发出 `forge.concurrency.approved`**（`accepted-event:3:0`），但 hat-channel 写出为空 → merge fallthrough |
| DAG 派发 | 重建 topology、admit U3/U9、spawn executor / reviewer jobs | DAG runtime | **U3 / U9 `job_terminal=failed`**，未合成 `forge.unit.execution_failed` 落 main |
| U16 misrouted check | virtual consumer 白名单判定 | runtime | **block 触发**：`task.resume.misrouted` 被错误发射，跳过 600s pending registration（hotfix `7ede210e` 已加白名单） |
| Operator ralph hat | 诊断 + 落 hotfix | agent context | commit `7ede210e` on `pittcat-dev` |
| Loop primary | iteration 4 之后无法继续 | — | **`forge.plan.blocked(reason=loop_stalled_max_iterations)`** |
| Cleanup | 处理 plan worktree + 整合分支 | cleanup hat | **integration 分支 retained_for_diagnosis**（`f5e5aabf`） |
| Reporter（本激活） | 落 manager 报告 + 发 `forge.report.done` + `LOOP_COMPLETE` | — | 进行中 |

### 5.2 依赖关系

```text
(plan 串行) U1 → U2 → U3 → U4 → U5 → U6 → U7 → U8 → U9 → U10 → U11 → U12 → U13 → U14 → U15
(本 loop 实际抵达) inspector → planner → guardian → (DAG 派 U3 / U9) → BLOCKED
```

依赖图按计划 §8 保持不变；U3 与 U9 是 DAG runtime 首批 admit 的 root units（分别对应「U3 完成 executor 已提交成果的 runtime 接续」与「U9 任务投影前登记可恢复 plan receipt」），均由 `forge.concurrency.approved` 触发但均未越过 `forge.unit.executed` / `forge.unit.execution_failed` 落点。

## 6. Scenario 验收结果

| Scenario | 外部可观察行为 | 验收测试 | 结果 | 证据 |
|---|---|---|---|---|
| S1–S19 | 计划 §4 BDD 行为规格 | 计划 §5 验收表 | **未执行** | 本 loop 派发未进入任何 acceptance gate |

- 未通过或未执行 Scenario 及原因：loop 在 `forge.concurrency.approved` 之后立即被 runtime U16 misroute + hat-channel merge 故障拦下，未进入 Unit dispatch 阶段；plan §4 BDD 列表中的 S1–S19 行为规格虽在 `development-plan.md` 描述，但 `crates/ralph-core/tests/scenarios/*.yml` 中是否为本计划新建 scenario 仍属信息缺失。
- 测试层级不足或环境限制：本 loop 派发面无任何测试运行；hotfix 自带单包测试见 §1，**不构成本计划的全量验收**。

## 7. 各 Unit 完成情况

### 7.1 通用前置说明

- plan `execution-plan.yml` 中声明 15 个 Unit（U1–U15），但本 loop 派发面未抵达 `forge.worktrees.ready`，因此 **`units/*-completion.md` 一律不存在**。
- U1/U2/U3 schema 端来自基线外 commit，**不计入本 plan 派工的 Unit 完成数**；其提交与 plan 派发面相互独立。
- 7.2 节只列出与本次 block 路径相关的 Unit（U3、U9、U16），其余 Unit 因未派发不展开。

### 7.2 U3：完成 executor 已提交成果的 runtime 接续

- **目标**：把 U3 schema（`dag_unit_bases` / `dag_stage_evidence`，v18.sql）接通到 Rust runtime 消费端；不重写 v18.sql，不重复 `CURRENT_VERSION` bump（见 plan §3 实施协议第 2 项与 U3 §6）。
- **完成情况**：**未完成**。本 loop iteration 4 中 executor activation 被 DAG runtime 派发但 `job_terminal=failed`（`.ralph/dag.db`），未合成 `forge.unit.executed` 到达 main。
- **主要修改**：无（runtime 端未产生 commit；既有 commit `2bbea7bf` / `b893fc06` / `f5e5aabf` 来自 prior merge，U3 wiring 部分尚未落地）。
- **风险与说明**：hotfix `7ede210e` 解决了 U16 misroute 副作用，但**不**让 U3 已失败的 activation 复活；operator 重启 loop 后 DAG scheduler 才会用 post-fix binary 重新 admit U3。

### 7.3 U9：任务投影前登记可恢复 plan receipt

- **目标**：在任务投影（task projection）发生前，把 plan receipt 写入 `.ralph/dag.db` 的 `dag_plan_receipts` 表，使后续 admit / resume 可定位到 plan identity 与 baseline。
- **完成情况**：**未完成**。`.ralph/dag.db` 中 `dag_plan_receipts` 对 plan_key `2026-09-09-0917-fix-forge-dag-p1-closure-plan` 仍为 `status=active`（block 描述 §Operator action 第 2 项），但下游 U9 executor activation 与 U3 同命运，`job_terminal=failed` 且无 `forge.unit.execution_failed` 落 main。
- **主要修改**：无（runtime 端未产生 commit）。
- **风险与说明**：U9 失败可能与 U3 共享同一根因（isolated hat-channel empty-after-activation），但 hotfix `7ede210e` 仅处理 U16 misroute 维度；重启后能否走通取决于 hat-channel merge 链路是否在新一轮 iteration 中恢复（block 描述 §Out-of-scope follow-ups 显式把它放到 future plan）。

### 7.4 U16：handoff 误判 dag_runtime 虚拟消费者

- **目标（harness 视角）**：U16 不是 plan §10 中显式声明的 Unit，而是 acceptance_and_lifecycle 流程中名为「U16」的 misrouted 检查点；`plan §10` 与本计划文件并未引用此编号。
- **完成情况**：**已通过 hotfix 修复（commit `7ede210e`）**。`event_origin.rs:255` 新增 `DAG_RUNTIME_CONSUMER = "dag_runtime"`，`is_virtual_runtime_consumer` 与 `supervisor` / `wave_runtime` 并列；测试 `u16_dag_runtime_concurrency_approved_no_misrouted` 复现 trace 并断言不发射 `task.resume.misrouted`。
- **主要修改**：`crates/ralph-core/src/event_origin.rs` + `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs:1031` 周边。
- **风险与说明**：hotfix 在 disk 上，但 live `ralph run` 进程仍持有 pre-fix binary，**本轮派工依然 block**；重启后此根因被消除。

### 7.5 U1 / U2 / U4–U8 / U10–U15

- **完成情况**：**未完成**。本 loop 派发面未抵达。
- **既有 commit 不计入本 plan 派工**：U1（`b9221413`）、U2（`ed926e93`）、U3 schema（`2bbea7bf` / `b893fc06` / `f5e5aabf`）是基线外常规开发流产物。

## 8. 并发开发情况

- Worktree 数量：**0**。本 loop 派发未抵达 `forge.worktrees.ready`；plan-level worktree 路径在 prior cleanup 已被移除（`/home/chaowen/Dev/agent_tools/worktree/ralph-orchestrator/2026-09-09-0917-fix-forge-dag-p1-closure-plan` 不存在）。
- 并发 Unit 列表：0（U3 / U9 在 DAG scheduler 内部并发 admit，但都没有越过 activation 完成边界）。
- 并发安全理由摘要：不适用。
- 越界修改 / 共享文件冲突：无（hotfix `7ede210e` 单文件改动 `event_origin.rs` + test）。
- Worktree 映射表：

| Unit | 分支 | Worktree | 最终状态 |
|---|---|---|---|
| U3 | `feat/unit-u3-evidence-handoff` | 不存在（未创建） | `job_terminal=failed`（仅 `.ralph/dag.db` 内部） |
| U9 | `feat/unit-u9-registration-receipt` | 不存在（未创建） | `job_terminal=failed`（仅 `.ralph/dag.db` 内部） |
| 其它 U1/U2/U4–U8/U10–U15 | `feat/unit-uN-…` | 不存在 | 未派发 |

`git show-ref --verify refs/heads/feat/unit-u*` 全部 ABSENT（cleanup §4 验证）。

## 9. 代码合入和 Commit 历史

### 9.1 合入过程

- 本 loop 派发未产生 commit；hotfix `7ede210e` 由 operator-side ralph hat 在 block 后产出。
- 既有 `pittcat-dev` 上与本 plan 范围相关的 commit：
  - `b9221413` U1 continuous slot refill after job completion
  - `ed926e93` U2 cross-stage resource capacity accounting
  - `2bbea7bf` U3 v18 migration + CURRENT_VERSION bump (schema only)
  - `b893fc06` U3 release transaction fix
  - `f5e5aabf` U3 plan-namespaced Unit keys in JobPipeline
  - `2b3dbe1a` merge: forge DAG P1 closure work
  - `95186d4c` chore: auto-commit before merge (loop primary)
  - `7ede210e` fix(dag): exempt dag_runtime virtual consumer from U16 misrouted check
  - `ba7d7ab9` docs(plan): refresh forge DAG closure continuation baseline（当前 HEAD）

### 9.2 最终 Commit 顺序

| 顺序 | Unit | Commit | Commit Message | 验证结果 |
|---|---|---|---|---|
| 1 | U1 | `b9221413` | U1 continuous slot refill after job completion | 既有 merge 自报 |
| 2 | U2 | `ed926e93` | U2 cross-stage resource capacity accounting | 既有 merge 自报 |
| 3 | U3 schema | `2bbea7bf` | U3 v18 migration + CURRENT_VERSION bump (schema only) | 既有 merge 自报 |
| 4 | U3 fix | `b893fc06` | U3 release transaction fix | 既有 merge 自报 |
| 5 | U3 fix | `f5e5aabf` | U3 plan-namespaced Unit keys in JobPipeline | 既有 merge 自报 |
| 6 | merge | `2b3dbe1a` | merge: forge DAG P1 closure work | merge commit |
| 7 | chore | `95186d4c` | chore: auto-commit before merge (loop primary) | 既有 |
| 8 | U16 fix (hotfix) | `7ede210e` | fix(dag): exempt dag_runtime virtual consumer from U16 misrouted check | `event_origin` 43/43 + `handoff_dispatch` 18/18 + `preset_lint::workflow_activation` 20/20 + `dag_scheduler` 166/166 + `RUSTFLAGS='-D warnings' cargo check` clean（block §Verification） |
| 9 | docs | `ba7d7ab9` | docs(plan): refresh forge DAG closure continuation baseline | docs only |

### 9.3 历史质量

- 线性历史：基本保持（既有 merge `2b3dbe1a` 后的 hotfix + docs commit 保持线性）。
- 无 Merge / WIP / fixup Commit：见上表。
- 每 Unit 一个 Commit：U1/U2/U3 各有 1–3 个 commit（U3 因需 schema + wiring + fix 被拆为 3 个），总体保持。
- 可按 Unit 回退 / bisect：理论可行，但 U3 runtime wiring 仍未在 `pittcat-dev` 落地，bisect 到 U3 commit 仍会停在 schema-only 状态。

## 10. 测试结果

### 10.1 测试总体结论

- **本 loop 派发面**：未执行任何 acceptance test；plan §5 验收表全部为「未执行」。
- **hotfix 维度（与本 plan 范围部分相关）**：`event_origin` 43/43、`handoff_dispatch` 18/18、`preset_lint::workflow_activation` 20/20、`dag_scheduler` 166/166、`RUSTFLAGS='-D warnings' cargo check --workspace --all-targets` clean（block §Verification 表 5 行）。
- **全量基线**：信息缺失；无法确认。本 plan 未触发 `./scripts/run-tests.sh` 入口；既有 commit 的测试基线由 commit message 自报，不构成本 plan 的全量门禁。

### 10.2 测试统计

| 测试类型 | 执行数量 | 通过 | 失败 | 跳过 | 结果 |
|---|---:|---:|---:|---:|---|
| plan 派发面 acceptance | 0 | 0 | 0 | 0 | 未执行 |
| hotfix 单包（`event_origin` 等） | 247 | 247 | 0 | 0 | 通过（block 自报） |
| 全量 `cargo nextest` workspace | 信息缺失 | — | — | — | 无法确认 |

> 若工具无法提供准确总数，写明：项目测试工具未提供准确用例总数（本 plan 派发面 0 用例）。

### 10.3 全量测试命令

```bash
# 本 plan 应在重启 loop 且 U3 / U4–U15 跑通后跑：
./scripts/run-tests.sh
# 全量基线出现竞态/时序 flake 时仅作文档化单线程兜底：
# RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh

# hotfix 自带单包验证（已运行）：
cargo nextest run -p ralph-core --lib event_origin
cargo nextest run -p ralph-core --lib event_loop::tests::handoff_dispatch
cargo nextest run -p ralph-core --lib preset_lint::workflow_activation
cargo nextest run -p ralph-cli --bin ralph -- dag_scheduler
RUSTFLAGS='-D warnings' cargo check --workspace --all-targets
```

## 11. 开发过程中发现的问题

| 问题 | 影响 | 处理方式 | 当前状态 |
|---|---|---|---|
| U16 misroute 把 `dag_runtime` 虚拟消费者判为 misrouted | 跳过后续 hat 派发；本 loop primary-20260909-082921 iteration 4 block | `7ede210e` 把 `dag_runtime` 加入 `is_virtual_runtime_consumer` allowlist；新增 `u16_dag_runtime_concurrency_approved_no_misrouted` 回归 | 修复已 commit；live binary 仍 pre-fix |
| isolated hat-channel empty-after-activation | `events-hat-guardian-primary-20260909-074906-3.jsonl` 写出 0 字节 → merge fallthrough → 下游 U3/U9 `job_terminal=failed` 且无 `forge.unit.execution_failed` 落 main | block §Out-of-scope follow-ups 显式排除 P1 范围 | **未处理**；预计 future plan |
| 计划 §8 描述性 drift（"可依赖能力" 措辞） | 阅读误导风险 | inspection-report §3.5 标注为「建议、非阻塞」 | **未处理**；re-issue 时可顺手刷新 |

## 12. 与原计划相比发生了什么变化

| 计划项 | 原计划 | 实际执行 | 变化原因 |
|---|---|---|---|
| 执行起点 | U1 → U2 → U3 wiring → U4–U15 | inspector / planner / guardian 跑通；U3 / U9 在 DAG admit 后失败；loop block | runtime U16 + hat-channel 两件故障并发 |
| CURRENT_VERSION | v18（E19） | v18（一致） | — |
| 迁移槽位 | v19–v23 待 executor 续写 | 不变 | — |
| U3 schema 实施者 | 本 plan executor | 基线外 `2bbea7bf`（计划 refresh 前后均如此） | 计划 refresh 已声明事实 |
| U3 wiring 实施者 | 本 plan executor | **未实施** | runtime block |
| U4–U15 实施者 | 本 plan executor | **未实施** | runtime block |
| Loop 终结事件 | `LOOP_COMPLETE` | **经过 `forge.plan.blocked`** | operator-side ralph hat 触发 `loop_stalled_max_iterations` |
| Cleanup 路径 | 通常 `forge.cleanup.done(cleanup_status=ok)` | `forge.cleanup.done(cleanup_status=retained_for_diagnosis)` | plan-level worktree 已在 prior cleanup 移除，integration 分支需保留供 re-issue |
| Reporter 路径 | 通常 `status: COMPLETED` | **`status: BLOCKED`** | plan blocked，模板 status mapping 强制走 blocked 路径 |

## 13. 风险和遗留事项

| 风险 | 等级 | 影响 | 建议动作 | 负责人建议 |
|---|---|---|---|---|
| Live `ralph run` 进程持有 pre-fix binary | 高 | 重启前再 dispatch 必失败 | 立即停掉当前 loop，按 block §Operator action 第 1 项 `pkill` 或 Ctrl-C 退出；按第 2 项 `ralph run --plan docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md` 重新启动 | operator |
| isolated hat-channel empty-after-activation 未修 | 中 | 下游 hat activations 可能再次 `job_terminal=failed` 而无 `forge.unit.execution_failed` 落 main | 在重启后的 iteration 中观察 `events-hat-*.jsonl` 写出；若再次撞上，开 future plan 修 `DagSchedulerRuntime::fail_job` 之前 hat-channel merge 路径 | runtime maintainer |
| Integration 分支 `ralph/2026-09-09-0917-fix-forge-dag-p1-closure-plan` 仍保留 | 低 | 后续 re-issue 仍可基于此分支 | 若 operator 决定彻底放弃此 plan，手动 `git branch -D ralph/2026-09-09-0917-fix-forge-dag-p1-closure-plan`（其 history 已合入 `pittcat-dev`） | operator |
| Plan §8 描述性 drift 未刷 | 低 | 阅读误导 | 未来 plan refresh 顺手修 | plan author |
| U3 wiring 与 U4–U15 仍待执行 | 高 | P1 closure 目标未达成 | 重启 loop 后让 DAG scheduler 用 post-hotfix binary 重新派发 | loop runner |

## 14. 需要经理关注或决定的事项

| 决策项 | 背景 | 可选方案 | 建议 |
|---|---|---|---|
| 是否重启 loop | 当前 live binary pre-fix，U3 / U9 仍 `job_terminal=failed` | A. 立即 `pkill` 后用 `--plan` 重启；B. 等待本 reporter 落 `LOOP_COMPLETE` 后再启动新 loop | **A**（block §Operator action 推荐） |
| 是否保留 integration 分支 | 后续 re-issue 还需要它 | A. 保留；B. 立即 `git branch -D` | **A**（cleanup 已 retained，re-issue 前不要主动删） |
| `isolated hat-channel empty-after-activation` 是否纳入 P1 | 显式排除在 P1 范围外 | A. 留 future plan；B. 拉入后续 P1 收口 | **A**（与 hotfix 同迭代会让 root cause 不清晰） |

> 当前没有需要经理额外决策的事项：唯一对外建议即按 block §Operator action 重启 loop。

## 15. 是否建议进入下一阶段

- [ ] 建议进入下一阶段
- [ ] 满足条件后进入下一阶段
- [x] **不建议进入下一阶段**

理由：plan 在 `forge.concurrency.approved` 之后被 runtime 双故障拦下；U3 wiring 与 U4–U15 完全未开始；需要先按 block §Operator action 重启 loop 让 DAG scheduler 用 post-hotfix binary 重新派发，**之后**再讨论下一阶段。

## 16. 清理结果

| 清理项 | 结果 | 说明 |
|---|---|---|
| 临时 Worktree | **保留** | plan-level worktree 路径在 prior cleanup 已被移除；本轮 path absent；Unit worktree 从未创建 |
| 临时分支 | **保留（integration branch only）** | `ralph/2026-09-09-0917-fix-forge-dag-p1-closure-plan` @ `f5e5aabf` `retained_for_diagnosis`（operator 重启 re-issue 时可能复用） |
| 临时日志 | **保留** | `.ralph/diagnostics/2026-09-09T15-49-06/trace.jsonl:25-28` 是 block 诊断核心证据；不要清理 |
| 构建产物 | **保留** | 本 loop 派发面无新构建；既有 `target/` 不变 |
| 最终报告 | **保留** | 本文件 + `docs/reports/2026-09-09-2026-09-09-0917-fix-forge-dag-p1-closure-plan-manager-report.md`（prior Inspector 阻塞版） |

## 17. 最终结论

- **最终审计结论**：BLOCKED。无 `forge.audit.done`；block 由 `forge.plan.blocked(reason=loop_stalled_max_iterations)` 替代。
- **功能交付结论**：U1/U2/U3 schema 与 `dag_runtime` 虚拟消费者修复已在基线 `7ede210e` 落地（既有 commit）；U3 runtime wiring 与 U4–U15 未交付。
- **测试结论**：本 plan 派发面 0 acceptance test 执行；hotfix 单包验证全绿（247/247）；全量基线信息缺失。
- **Git 历史结论**：`pittcat-dev` 保持线性，已含 5 个 P1-相关 commit + 1 个 hotfix + 1 个 docs commit（HEAD `ba7d7ab9`）；integration 分支 `f5e5aabf` 保留。
- **风险结论**：live `ralph run` 仍持 pre-fix binary（高）；isolated hat-channel empty 未修（中）；其它均为低。
- **下一步建议**：按 block §Operator action 1-2 重启 loop，让 DAG scheduler 重建 topology 并 re-admit U3 / U9；本 reporter **未**发出 `LOOP_COMPLETE`（task gate `OpenTasksBlocking` 阻断，详见技术附录 G.1 / G.3），等 stale-breaker 第 3 次自动 `LoopStale` 终止 loop，或 operator 路径 A 重启 loop 后由下一次 reporter activation 按 §Resume rules 重发 `LOOP_COMPLETE`（带本 `report_path`）。

---

# 技术附录

## A. 最终 Git 状态

```text
$ git log --oneline -10
ba7d7ab9 docs(plan): refresh forge DAG closure continuation baseline
7ede210e fix(dag): exempt dag_runtime virtual consumer from U16 misrouted check
95186d4c chore: auto-commit before merge (loop primary)
2b3dbe1a merge: forge DAG P1 closure work
f5e5aabf fix(dag): namespace live Unit keys by plan in JobPipeline
b893fc06 fix(dag): wrap release_resources_for_unit in Immediate transaction
2bbea7bf feat(dag): U3 v18 migration + CURRENT_VERSION bump (schema only)
ed926e93 feat(dag): U2 cross-stage resource capacity accounting
b9221413 feat(dag): U1 continuous slot refill after job completion
369ad01d docs(plan): 深化 2026-09-09 forge DAG P1 收口计划(实施协议版)

$ git worktree list --porcelain
worktree /home/chaowen/Dev/agent_tools/ralph-orchestrator
HEAD ba7d7ab90453602eea67336b9d48d769d05389d9
branch refs/heads/pittcat-dev
```

## B. 最终 Commit 列表

```text
$ git log --oneline 2b3dbe1a..ba7d7ab9
ba7d7ab9 docs(plan): refresh forge DAG closure continuation baseline
7ede210e fix(dag): exempt dag_runtime virtual consumer from U16 misrouted check
95186d4c chore: auto-commit before merge (loop primary)
```

## C. Worktree 记录

```text
$ git worktree list
/home/chaowen/Dev/agent_tools/ralph-orchestrator  ba7d7ab9  [pittcat-dev]

$ git show-ref --verify refs/heads/ralph/2026-09-09-0917-fix-forge-dag-p1-closure-plan
f5e5aabf856ed8d6f0e193835fe7d7ae08b9b56b refs/heads/ralph/2026-09-09-0917-fix-forge-dag-p1-closure-plan
(retained for diagnosis; cleared on next re-issue success or operator manual delete)

$ git show-ref --verify --quiet refs/heads/feat/unit-u3-evidence-handoff   # ABSENT
... (其它 12 个 unit branch 全部 ABSENT；详见 cleanup §4)
```

## D. 完整测试命令与结果

```text
# hotfix 自带单包验证（block §Verification）
$ cargo nextest run -p ralph-core --lib event_origin
  → 43 / 43 pass
$ cargo nextest run -p ralph-core --lib event_loop::tests::handoff_dispatch
  → 18 / 18 pass (incl. u16_dag_runtime_concurrency_approved_no_misrouted)
$ cargo nextest run -p ralph-core --lib preset_lint::workflow_activation
  → 20 / 20 pass
$ cargo nextest run -p ralph-cli --bin ralph -- dag_scheduler
  → 166 / 166 pass
$ RUSTFLAGS='-D warnings' cargo check --workspace --all-targets
  → clean

# 本 plan 派发面 acceptance: 未执行（loop 在 forge.concurrency.approved 之后 block）
# 全量基线 ./scripts/run-tests.sh: 未执行（本 plan 派发面）；重启 loop 并 U3/U4-U15 跑通后应跑一次
```

## E. 关键文件变更

| 文件或目录 | 变更目的 | 所属 Unit |
|---|---|---|
| `crates/ralph-core/src/event_origin.rs` | 把 `dag_runtime` 加入 `is_virtual_runtime_consumer` allowlist；新增 `DAG_RUNTIME_CONSUMER` 常量 | U16 hotfix（block §What this hat did） |
| `crates/ralph-core/src/event_loop/acceptance_and_lifecycle.rs:1031` | U16 misrouted check 周边（predicate 调整） | U16 hotfix |
| `crates/ralph-core/src/event_origin.rs` 新增测试 `u7_virtual_supervisor_consumer_predicate_positive` 扩展 | 覆盖 `dag_runtime` 命中 | U16 hotfix |
| `crates/ralph-core/src/event_origin.rs` 新增测试 `u16_dag_runtime_concurrency_approved_no_misrouted` | 复现 trace 并断言不发射 `task.resume.misrouted` | U16 hotfix |
| `docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md` | refresh baseline `7ede210e` / reviewed_head / E19 / §3 实施协议第 2 项 / U3 标题与 §6 / §18 / §0 续执行基线声明 | docs commit `ba7d7ab9` |
| `crates/ralph-core/src/supervisor/migrations.rs:76` | `CURRENT_VERSION = 18` | U3 schema（既有） |
| `crates/ralph-core/src/supervisor/migrations/v18.sql` | U3 schema：dag_unit_bases + dag_stage_evidence | U3 schema（既有） |
| `crates/ralph-core/src/supervisor/dag_store_rusqlite/admission.rs` | U2 cross-stage resource capacity accounting | U2（既有） |

## F. 已知限制

- 本 plan 派发面未进入 `forge.worktrees.ready` 阶段，所有 Unit worktree 路径从未创建；integration 分支 `ralph/2026-09-09-0917-fix-forge-dag-p1-closure-plan` @ `f5e5aabf` 仍保留。
- `.ralph/dag.db` 中 `dag_plan_receipts` 对 plan_key `2026-09-09-0917-fix-forge-dag-p1-closure-plan` 仍 `status=active`；重启 loop 后 DAG scheduler 会读取 receipt 并重建 topology。
- `events-hat-guardian-primary-20260909-074906-3.jsonl` 0 字节是 hat-channel 写不出 root cause，本 plan 不修。
- plan §8 "可依赖能力" 措辞的描述性 drift 在本次 block 路径不阻塞；re-issue 时可顺手刷新。

---

## Reporter 自检（§23 — _emit 前逐项确认_）

- [x] 报告文件已创建，路径符合命名规则（`docs/reports/2026-09-09-2026-09-09-0917-fix-forge-dag-p1-closure-plan-ralph-blocked-manager-report.md`；与 prior Inspector 阻塞报告 `-manager-report.md` 后缀不同，避免覆盖）
- [x] 开头明确最终结果（`status: BLOCKED` + `final_audit: BLOCKED`）；经理可读，非日志堆砌
- [x] 所有 Unit / Scenario / 测试 / 风险 / 决策项已覆盖（按 7.1 通用前置说明 + 7.2-7.4 重点 Unit + 7.5 其余 Unit 列表）
- [x] 数字有依据；不确定项标「信息缺失；无法确认」或「未执行」
- [x] status 与 final_audit 映射符合 §22：`forge.plan.blocked` → BLOCKED / BLOCKED（cleanup_status=retained_for_diagnosis 显式不可映射 COMPLETED）
- [x] 自检通过；准备 `ralph emit forge.report.done --policy-check` 预检

## G. Reporter Fail-Close OPAC 收尾（`task.resume` 拒收 → 停止状态变更）

> **本节记录 reporter 实际遭遇的 emit 拒收与 OPAC 收尾纪律，区别于前序 §17 的理想路径。**

### 1. 拒收事实链

- 本次 reporter activation 触发自 `forge.cleanup.done`（`cleanup_status=retained_for_diagnosis`），按 reporter hat instructions §3 状态映射写入 `status=BLOCKED / final_audit=BLOCKED` 并落盘 `docs/reports/2026-09-09-2026-09-09-0917-fix-forge-dag-p1-closure-plan-ralph-blocked-manager-report.md`。
- 预检：`ralph emit forge.report.done --policy-check -j '{...}'` 返回 `Error: Builtin ralph hat may only emit control topics: ["LOOP_COMPLETE", "loop.cancel", "loop.start", "task.resume", "plan.blocked"]. Topic 'forge.report.done' is a business topic and cannot be emitted by ralph. Set --hat to a registered workflow hat (e.g. coordinator, executor, review-synthesizer) instead.` —— 当前 hat id 被 runtime 解析为 orchestrator sentinel `ralph`（而非 preset 中的 `reporter`），precheck 因此拒绝。这是 `crates/ralph-core/src/event_loop/completion_and_termination.rs:72` 强制约束（ralph 只能发 control topic），不是 preset drift。
- 由于 `ralph` hat **不在 preset 的 `tasks.coordinator_hats`**（`presets/en/parallel-forge.yml` 未声明），且 13 个 open tasks 的 `owner_hat_id="executor"`（loop 派发时由 DAG runtime 注入），`ralph tools task close` / `fail` / `reopen` 在 agent context 下被拒绝（`crates/ralph-core/src/task.rs:405` `task_lifecycle_denied_message`）。换言之：`forge.report.done` / `LOOP_COMPLETE` 的 task gate（`crates/ralph-core/src/event_loop/wave_scope.rs:925` `OpenTasksBlocking`）所需的「close / fail / reopen outstanding tasks」前置条件在当前 hat 上下文无法合法满足。
- runtime 注入 `task.resume(kind=open_tasks_blocking, target_hat="ralph", message="Close, fail, or reopen outstanding tasks before emitting the completion promise.")`，与 2026-07-30 fail-close β 同根因路径一致。

### 2. 为何不重发 / 改路径 / 调权限

- **不重发 `forge.report.done`**：precheck 已拒；按 OPAC 纪律重发同一 payload 不会越过 `ralph_business_topic_rejected`。
- **不 emit `LOOP_COMPLETE`**：task gate 仍 blocking；同 signature + 无 progress 会持续计数（`wave_scope.rs:1107` `consecutive_completion_rejections`），第 3 次后 stale-breaker 自动返回 `TerminationReason::LoopStale`，**这是预期的 fail-safe**，不需要手动绕过。
- **不调 `task close` / `fail` / `reopen`**：当前 hat 不在 `coordinator_hats` 也非 owner，权限被拒。unset `RALPH_CURRENT_HAT` 等 agent env 走 human CLI bypass 是**明确禁止**的（HARD RULE 5 反模式；且会污染 `.ralph/current-loop-id` marker 与 task ledger 上下文）。
- **不发 `loop.cancel`**：reporter hat 在 `presets/en/parallel-forge.yml` 的 `publishes=[forge.report.done, LOOP_COMPLETE]` 不含 `loop.cancel`；且当前 hat 是 `ralph` sentinel，cancel 会绕过 cleanup 收尾路径，与 §Operator action 的「重启 loop 让 post-hotfix binary 重建 topology」建议冲突。

### 3. OPAC 收尾纪律（同款 2026-07-30 BLOCKED 报告 §23 / §17）

- 本 reporter **不 emit 任何业务事件**（含 `forge.report.done` / `LOOP_COMPLETE`）。
- 等待 stale-breaker 在第 3 次同 signature task.resume 后自动终止 loop（`TerminationReason::LoopStale`），或 operator 按 block §Operator action 1-2 重启 `ralph run --worktree --reuse-worktree --plan docs/plans/2026-09-09-0917-fix-forge-dag-p1-closure-plan.md` 让 DAG scheduler 用 post-`7ede210e` binary 重建 topology 并 re-admit U3 / U9。
- **下次 reporter 触发时**应直接复用本报告：reporter hat instructions §Resume rules 明确「若 loop 在 `forge.report.done` 后停止，resume **不得**重写已有 audit/report 事实制造新成功；只能重发 `LOOP_COMPLETE`（带同一 `report_path`）」；本轮 task gate 阻断期间 `LOOP_COMPLETE` 走 `task.resume` 路径，最终 stale-breaker 兜底终止 loop，本报告路径一致。

### 4. 待 Operator 决定

- 路径 A（推荐）：按 `blocks/ralph-blocked.md` §Operator action 重启 loop，让 DAG scheduler 用 post-hotfix binary 重建 receipt 并 re-issue U3 / U9；本报告作为 reference，下一个 reporter activation 时按 §Resume rules 重发 `LOOP_COMPLETE`（带本 `report_path`）即可。
- 路径 B（人工 reset）：operator 用 human CLI 直接 `ralph tools task fail task-1788942799-4721..task-1788942799-472d`（human CLI bypass `is_agent_context` gate），然后重启 loop；这会绕开 stale-breaker 兜底，但**会**留下跨 loop task ledger 漂移（需手动 reconcile `.ralph/agent/tasks.jsonl`）。

---

## H. 报告终态：未 emit 任何业务事件

- `forge.report.done` — **未 emit**（precheck 被 `ralph_business_topic_rejected` 拒绝；详见附录 G.1）
- `LOOP_COMPLETE` — **未 emit**（task gate `OpenTasksBlocking` 阻断；详见附录 G.1 / G.2）
- `loop.cancel` — **未 emit**（reporter hat `publishes` 不含；详见附录 G.2）
- `forge.plan.blocked` — **未 emit**（已被 prior ralph hat 在 iteration 4 发出，本次仅消费 trigger 上下文）
- 等待 stale-breaker 第 3 次自动 `TerminationReason::LoopStale` 终止 loop，或 operator 走路径 A / B
