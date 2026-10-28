---
title: "parallel-forge 对抗性审查(三层):为什么 DAG 模式总是跑失败"
date: 2026-10-29
type: adversarial-review
subject_preset: builtin:parallel-forge (scheduler_mode: dag)
baseline: 4c8718b7
layers: [orchestration(preset), skill(injected ralph-tools docs), runtime(ralph base)]
agent_skill_audit: performed (DAG 相关段落)
key_stage_event_gate_audit: skipped (本次为机制对抗审查,非 AAF 门禁评审)
static_lint: "ralph preset check -H builtin:parallel-forge --strict → PASS"
---

# parallel-forge 对抗性审查:为什么 DAG 模式总是跑失败

## 0. 结论摘要

`ralph preset check --strict` 是 PASS,全量测试也是绿的,但 **DAG 执行面在真实 git 仓库里走不完一个多阶段 Unit**。
2026-09-17 那轮红队修掉的是 origin guard / exactly-once / shadow 这几处(commit `0736c2b9`),
这次查到的是更靠后的几段:**Unit 执行 → 评审 → 修正 → 集成 → 收尾**。这几段每一段都至少有一个确定性的断点,
而且断点大多不发诊断事件,只是"不再有待办工作"→ 主循环 fallback → 退出。

**最可能的主致死链**(每一环都读码闭合,其中两环有实证):

1. executor 正常完成并 commit → `forge.unit.executed` 被接受 → runtime 准备拉起 reviewer;
2. **R1**:`read_unit_branch_tip` 拼出 `refs/heads/ralph/<loop>/<unit>{commit}`(少了 `^`),`git rev-parse` 必定失败 → execute 阶段的 accepted_commit 回退成 plan base【实证】;
3. reviewer spawn 时用这个 base 去 `UnitWorktree::acquire` → 分支 tip(executor 的提交)≠ base → `BaseMismatch` → spawn 失败;
4. **R2**:runtime 把 Review 阶段的失败合成为 `forge.unit.execution_failed`,并且 `hat=reviewer`;reviewer 的 `publishes` 里没有这个 topic → isolated scope / origin guard 直接丢弃;
5. failure-handler 永远不会被触发;Unit 卡在 Review slot 上;`has_pending_work()==false` → pump 退出 → fallback → loop 终止,**没有 report,也没有 BLOCKED**。

就算 R1/R2 修好,后面还有 R3(修正重入是死路)、R4(第一次集成后宿主仓库被判为 dirty)、O1(planner 模板缺 `tests:`,导致集成门禁必失败)和 O2(收尾 hats 依赖 wave 时代的数据源)四道必死关。

| # | 层 | 严重度 | 发现 | 证据 | 置信度 |
|---|---|---|---|---|---|
| R1 | 基座 | P0 | `read_unit_branch_tip` 的 ref 少了 `^`,stage evidence 永远回退成 plan base,下游阶段 acquire 时 BaseMismatch | 实证(git) + 读码 | 95 |
| R2 | 基座 + 编排 | P0 | Review 阶段失败被合成为 `forge.unit.execution_failed@reviewer`,reviewer 无权发布 → 被丢弃 → Unit 静默卡死 | 读码闭合 | 92 |
| R3 | 基座 | P0 | 修正重入是死路:Verify/集成失败后 `Verify→Review` 是非法迁移(只打 debug 日志);execute 失败后的 Fix 强制需要 review artifact | 读码闭合 | 90 |
| R4 | 基座 | P0 | 集成用 `update-ref` 推进**宿主已 checkout 的分支** → 宿主 index 变旧,`git status` 出现 `D `/`M ` → 之后每次 acquire 都报 HostDirty;tester 测的是旧工作树 | 实证(git) + 读码 | 90 |
| O1 | 编排 | P0 | 集成要求每个 Unit 都有 `tests:` 和非空 `allowed_paths`,否则 fail-closed;但 unit 模板没有 `tests:`,planner 指令也没提 | 读码闭合 | 88 |
| O2 | 编排 | P0 | auditor / finalizer / cleanup 依赖 `forge.worktrees.ready`、`integration_branch`、`worktree-map.yml`、`waves/*/review.md`,DAG 模式下这些都没有生产方 | 读码闭合 | 88 |
| R5 | 基座 | P1 | 每次 spawn 都跑 `ensure_host_clean`:宿主仓库只要在 `.ralph/` 之外有一个未跟踪文件(比如未提交的 plan.md),所有 Unit 的所有阶段都起不来 | 读码 | 85 |
| R6 | 基座 | P1 | `admission_base_pin` 还是 SKELETON:有依赖的 Unit 仍然从批准时的 plan base 起步,看不到依赖方已集成的代码 | 读码(模块自述) | 85 |
| O3 | 编排 + 基座 | P1 | `.ralph/` 下 artifact 的两难:写在 worktree 里不提交 → `resume` 判为 untracked;提交进去 → 集成 `ForbiddenPath(.ralph)` | 读码 | 80 |
| R7 | 基座 | P1 | 卡死不可观测:修正被拒、迁移非法、事件被丢都只打 debug/warn 日志,没有 typed 事件,也不会 fail-close 成 `forge.plan.blocked` | 读码 | 85 |
| O4 | 编排 | P1 | failure-handler 自己数轮次,runtime 用 `MAX_FIX_ATTEMPTS=3`,两套计数;`final.correction.settled` 要引用 tester 还没写的日志;这条路径还会把未完成集成的 plan 送进成功链 | 读码 | 75 |
| O5 | 编排 | P1 | failure-handler 在全量验证失败时的 `work.failed` 指令缺 6 个 schema 必填字段,和 precheck 规则也冲突 | 读码 | 75 |
| S1 | Skill | P1 | `ralph-tools.md` 里 DAG JobContext 表与实现不符:unit_key 格式、task_key 带 `:step-<n>:`、stage 取值、attempt 从 1 开始,都写错了 | 读码 | 85 |
| O6 | 编排 | P2 | `slot_retry_budget: 2` 的注释("三次执行机会")在 DAG 模式下不成立;路径命名漂移(verifier / finalizer / reporter 之间对不上) | 读码 | 80 |

---

## 1. 基座层(ralph runtime)

### R1 [P0] stage evidence 的 ref 少了 `^`,每个下游阶段都会 BaseMismatch

- 缺陷位置:`crates/ralph-cli/src/loop_runner/dag_scheduler/spawn.rs:1537-1540`
  ```rust
  let reference = format!("refs/heads/ralph/{}/{}{}", exec.loop_id, unit_key, "{commit}");
  ```
  拼出来是 `refs/heads/ralph/<loop>/U1{commit}`。同名函数在 `integrate.rs:728` 里写的是正确的 `^{{commit}}`。
- 实证(本仓库):
  ```
  git rev-parse --verify "refs/heads/pittcat-dev{commit}"   → fatal: Needed a single revision (exit 128)
  git rev-parse --verify "refs/heads/pittcat-dev^{commit}"  → 4c8718b7… (exit 0)
  ```
- 后果:`record_accepted_evidence`(spawn.rs:1479-1523)读取必定失败,于是 `accepted_commit = base`(plan HEAD)。
  Review / Verify / Fix 的 `resolve_stage_base` 从 evidence 拿到的都是 plan base,`spawn_job` 再调 `UnitWorktree::acquire(.., base)`(spawn.rs:2044)→ `worktree.rs:224-245`:
  - executor 有 commit:tip ≠ base → `BaseMismatch`;
  - executor 没 commit:进 `resume(base, base)` → `resume.rs:126-135` 要求 `status --porcelain --untracked-files=all` 为空 → 失败。
  
  结果是**任何干了活的 Unit 都过不了 Review 的 spawn**。
- 测试为什么没抓到:`resolve_stage_base` 的测试(spawn.rs:3744-4050)直接往 store 灌 evidence;`real_dag_executor_canary_runs_in_git_worktree` 只跑到 Execute。没有测试覆盖"真实 commit → 真实 Review spawn"。
- 修复方向:改成 `^{commit}`,和 integrate.rs 共用同一个 helper;再补一条真实 git 的 Execute→Review canary。

### R2 [P0] Review 阶段的失败事件归属到 reviewer,而 reviewer 无权发布

- `SpawnKind::Review.failure_topic()` 是 `"forge.unit.execution_failed"`(spawn.rs:393-399);`queue_failure_event` 用 `identity.hat`(也就是 `reviewer`)当 source(spawn.rs:1004-1011)。`FileEventMergeSink` 再把它写成 `hat: reviewer`(merge_sink.rs:160-164)。
- preset 里 reviewer 的 `publishes: [forge.unit.reviewed, forge.plan.blocked]`(parallel-forge.yml:835)。
- 接受链上有两道关会丢掉它:
  1. isolated scope:`scope_hat = event.hat`,`isolated_publish_allowed(reviewer, execution_failed)==false` → drop(legacy.rs:731);
  2. origin guard:`can_publish` 为 false → `out-of-scope topic for declared hat`(event_origin.rs:~519)。
- 会走到这条路径的情况:R1 导致的 spawn 失败、reviewer 超时、没出 verdict、reviewer 按自己的指令发了 `forge.plan.blocked`(`handle_completion` 只认 success/failure topic,spawn.rs:724-767,plan.blocked 会被当成 ContractViolation)。job prompt 还明确教 reviewer"失败时发 `forge.unit.execution_failed`"(prompt.rs:63),这个事件 CLI/scope 层也会拒。
- 后果:failure-handler 收不到;Unit 的 Review slot 永远不释放;依赖它的 Unit 不再被 admit;`has_pending_work()`(spawn.rs:654-661)变成 false → `pump_idle` 返回 false(inner.rs:2474-2480)→ fallback 终止。
- 修复方向(二选一,建议都做):Review 失败改成 runtime 身份发出(纳入 `DAG_RUNTIME_*` 白名单并按 dag 模式门控),或者把 source 统一成 `executor`(integrate.rs:634-639 的 `fail_integration` 已经这么做了);reviewer 的 `forge.plan.blocked` 要么移出 publishes,要么由 `handle_completion` 显式识别。

### R3 [P0] 修正重入:只有 "review REJECTED → Fix" 这一条能走通

`on_correction_requested`(dag_scheduler/mod.rs:670-727)对每个受影响的 Unit 执行 `pipeline.bump_attempt_and_advance(unit_key, Stage::Review)`:

| 失败来源 | 失败时 Unit 所处阶段 | 结果 |
|---|---|---|
| `forge.unit.verification_failed` | Verify(已 release) | `Verify→Review` 不在 `can_advance_to` 允许范围内(runtime_job/mod.rs:83-88)→ `IllegalStageTransition` → 只打 `debug!`(mod.rs:723);attempt 已经先 +1 |
| 集成失败(`fail_integration` → execution_failed@executor) | Verify | 同上,死路 |
| `forge.unit.execution_failed`(Execute) | Execute | 迁移到 Review 合法,但 Fix spawn 时强制 `consume_stage_artifact(.., "review", "review_report_path")`(spawn.rs:2194-2221);execute 失败时没有 review → ContractViolation → 又合成一个 execution_failed → failure-handler 再请求修正……直到 3 次耗尽 |

另外,`on_correction_requested` 读取的 `correction_request_path` 只当作 feedback 传下去,但 Fix 的 artifact_refs 实际绑定的是 review 报告。failure-handler 写的修正证据从来不会成为 fixer 的权威输入。

修复方向:修正应该从 Fix 阶段重新进入(给 `Stage` 增加 Fix,或允许 `Verify/Execute → Review(fix)` 的受控回退);Fix 的 artifact 选择要按失败来源切换(review 报告 / correction_request_path);被拒的修正要合成 typed 失败事件,不能只写 debug 日志。

### R4 [P0] 集成推进的是宿主正在 checkout 的分支

- `integrate_unit` 用 `plan.target_branch`(= `on_plan_ready` 时宿主的当前分支,mod.rs:805-811)当集成目标(integrate.rs:414-422);lane 的 `compare_and_swap_ff` 直接执行 `git update-ref refs/heads/<target>`(integration_lane.rs:686-700),**宿主的 index 和工作树都不会同步**。
- 实证(临时仓库):对已 checkout 的分支执行 `update-ref` 之后,`git status --porcelain` 输出 `D  g`。
- 后果:
  1. 下一次 `UnitWorktree::acquire` → `ensure_host_clean`(worktree.rs:216, 293-347)→ `HostDirty`。第一个 Unit 集成之后,**其他所有 Unit 的所有后续阶段都 spawn 失败**;
  2. tester / auditor / finalizer 都在宿主工作树里跑,看到的是集成之前的旧文件,而 HEAD 已经前进 → 全量验证测的是错误的代码,auditor 会看到一个"脏"仓库;
  3. finalizer 的设计("把 integration 分支 ff 到起始分支")和这一行为互相矛盾(见 O2)。
- 修复方向:DAG 集成改用独立的 integration 分支(不 checkout),finalizer 负责 ff 到宿主;或者集成后对宿主做受控同步(`read-tree -m -u`),并把同步写入 lane 事务。

### R5 [P1] 每次 spawn 都要求宿主仓库完全干净

`acquire` 每次都会执行 `ensure_host_clean`,只豁免 `.ralph/`。宿主里有任何未跟踪或已修改的文件(常见的有:`-p` 传入但没提交的 `docs/plans/xxx.md`、`PROMPT.md`、`ralph.yml` overlay、IDE 文件),**所有 Unit 的 Execute 都会失败**(FailureClass::FilesystemPartial)。failure-handler 没有能力修复宿主上的这类问题,只会把 3 轮修正耗尽。inspector / guardian 也没有"宿主必须干净"这一前置检查。

修复方向:在 `forge.concurrency.approved` 这个边界做一次检查并 fail-close 成 `forge.plan.blocked`(带可读原因);spawn 时只校验 worktree 自身。

### R6 [P1] 依赖感知的 base pin 还没接线

`admission_base_pin.rs:1-15` 自述是 SKELETON,`#![allow(dead_code)]`。`resolve_stage_base` 在 Execute 阶段的取值顺序是 per-Unit pin → plan `verified_base_commit`(spawn.rs:1328-1346),后者在批准时就固定了。所以 `depends_on: [U1]` 的 U2 是从**没有 U1 代码**的 base 起步的,会出现以下情况之一:executor 重复实现、测试失败,或者集成时 `ApplyConflict`。这和 preset "foundation-before-dependent" 的承诺冲突。

### R7 [P1] 卡死不可观测

R2 和 R3 的失败路径最终只留下 `warn!` / `debug!`(`DAG seam: pipeline blocked`、`correction admission deferred or blocked`、`boundary_violation`),**没有一个 Unit 级别的终态业务事件**,也不会在"plan 还没全部集成而 `has_pending_work()==false`"时 fail-close 成 `forge.plan.blocked`。用户看到的就是 loop "莫名其妙结束"或者走到 fallback 上限。建议:pump 发现 `active plan && !all_integrated && !has_pending_work` 时,合成 `forge.plan.blocked{reason=dag_stalled, units=[...]}`,把诊断交给 cleanup/reporter。

---

## 2. 编排层(preset / schema / templates)

### O1 [P0] planner 产出的 execution-plan 必然过不了集成门禁

- 集成 fail-closed 的条件有:`allowed_paths` 为空 → `integration_path_policy_unconfigured`;`tests` 为空 → `integration_gate_unconfigured`(integrate.rs:443-462)。
- `tests` 在 handoff 层是可选字段(parallel_forge_handoff.rs:742),`preset check`、`forge.plan.ready` 的 policy-check 都**不会**拒绝缺 `tests` 的计划。
- `presets/templates/parallel-forge/unit.template.yml` **没有 `tests:` 字段**(只有 `acceptance_test.command` / `tdd.regression.commands`),planner 的 Step 3 自检清单也没提它。
- 另外 `tests` 是**不经过 shell** 的 argv 解析(integrate.rs:643-718),`cd x && cargo test` 这类写法会被当成普通参数执行失败。
- 后果:每个 Unit 都会在 verify 通过之后的集成阶段失败,再进入 R3 那条死路的修正循环。
- 修复方向:模板加入 `tests: []`,并写清"argv,不经 shell";把 `tests` / `allowed_paths` 非空提升为 `forge.plan.ready` 的确定性拒收条件(和 `unit_tasks` 拒收放在同一层)。

### O2 [P0] 收尾三个 hat 仍是 wave 时代的契约

| hat | 需要的输入 | DAG 模式下的来源 |
|---|---|---|
| auditor | `target_branch` / `target_start_sha` / `target_status_fingerprint` "from the fan-out projection"(yml:979-983;schema 的 source 写的是 `forge.worktrees.ready.*`,schemas:483-494) | **无**。生产代码里没有任何地方发出 `forge.worktrees.ready`(只在 tests/scenarios 中出现) |
| auditor precheck | Step 2:"still match the fan-out projection"(yml:219) | 无法满足 → 3 次拒绝 → `forge.plan.blocked(precheck_failed)` |
| finalizer | `integration_branch`、"target 与 integration 是不同的真实分支",然后 `merge --ff-only`(yml:1008-1031) | DAG 直接集成到宿主分支,**不存在 integration 分支** → 必然 block |
| finalizer | 要求 `waves/<unit_key>/review.md` 必须存在(yml:1022) | reviewer 写的是 `units/<unit_id>-review.md`(yml:857) |
| cleanup | `forge.worktrees.ready` 投影 / `worktree-map.yml`(yml:1065-1068) | DAG worktree 在 `.ralph/worktrees/<loop>-<unit>`,分支是 `ralph/<loop>/<unit>`,没有 map → 什么也清不掉 |
| tester | `waves/*/verification.md`(yml:928) | 不存在 |

即使开发阶段全部成功,收尾也只能走到 BLOCKED,或者逼 agent 编造 SHA。

### O3 [P1] `.ralph/` artifact 的两难

executor / reviewer / verifier 按指令把报告写到 cwd 下的 `.ralph/forge/<plan>/...`,而 cwd 是 Unit worktree:
- 不提交:下一阶段 `resume` 要求 worktree 干净且没有 untracked 文件 → 失败,除非目标仓库提交过的 `.gitignore` 里包含 `.ralph/`;
- 提交:集成的 changed-path guard 禁止 `.ralph` 前缀(changed_path_guard.rs:152)→ `ForbiddenPath`。

此外 executor 指令里的 `.ralph/forge/<plan_key>/development-plan.md`、`ralph tools task list` 都是相对 worktree 解析的,而 worktree 里**没有**宿主的 `.ralph/forge` 和 `tasks.jsonl`。指令也没有明确的 `git commit` 步骤(只在 guardrail 里写了"one atomic commit per Unit")。

修复方向:runtime 把 stage artifact 的写入目录固定到宿主的 `.ralph/forge/<plan>/units/`(绝对路径注入,例如 `RALPH_DAG_ARTIFACT_DIR`),并在 executor 指令里加上明确的 commit 步骤。

### O4 [P1] 修正预算有两套计数,final 路径可能落到成功链

- failure-handler 通过"读取之前的 correction artifact"自己确定轮次(yml:682-684);runtime 用的是 `MAX_FIX_ATTEMPTS=3`(jobs.rs:404),而且 R3 中 attempt 会无效 +1,两边必然不同步。
- `forge.final.correction.settled` 必填的 `verification_log_path` 被定义为 "written by the tester"(schemas:144-148),但 tester 恰恰是由这个事件触发的 → 鸡生蛋问题。
- `full_verify` 同时接受 `final.correction.settled`(yml:108-110):某个 Unit 修正耗尽、plan **还没全部集成**时,仍然可能进入 full_verify → audit → finalize,这是 single-chain 审查里的 `fallback_reaches_success_terminal` 形态。

### O5 [P1] 全量验证失败路径的 `work.failed` 指令和 schema 冲突

failure-handler 这一节只要求 5 个字段(yml:672-676),schema 有 11 个必填字段(schemas:709-721,含 `dead_end_*` / `failure_evidence_status` / `correction_status` / `dead_end_gate_passed`)。precheck Step 3 要求 `correction_status=exhausted|not_applicable`。agent 要靠 policy-check 被拒之后自己猜着补齐,很容易 3 次耗尽,最终以 `precheck_failed` 结束,而不是 FAILED。

### O6 [P2] 配置注释和路径漂移

- `slot_retry_budget: 2` 的注释写"三次 executor 机会"(yml:168-172),但 DAG spawn 路径完全不读这个字段:一次执行失败就直接进 failure-handler。
- verifier 写 `incremental-verification.md`(yml:899),schema 示例是 `units/<id>-verification.md`;finalizer / reporter 读的是 `waves/*/review.md`;executor Step 1 读的 `execution_plan_path` / `worktree` 对应的真实环境变量名是 `RALPH_DAG_VERIFIED_EXECUTION_PLAN_PATH` / `RALPH_DAG_WORKTREE`。

---

## 3. Skill 层(注入的 ralph-tools 文档)

### S1 [P1] `ralph-tools.md` 的 DAG JobContext 表与实现不符

| 文档(ralph-tools.md:36-48) | 实现(spawn.rs:2339-2365 / 1925-1933) |
|---|---|
| `RALPH_DAG_UNIT_KEY` 例如 `U3` | `forge:<plan_key>:U3` |
| `RALPH_DAG_TASK_KEY` "含 `:step-<n>:` 段" | 等于 unit_key,没有 step 段 |
| `RALPH_DAG_STAGE` 取 `execution` / `review` / `verification` | `execute` / `review` / `verify` / `fix` |
| `RALPH_DAG_ATTEMPT` "从 1 开始" | 首次 spawn 时 attempt = 0 |

agent 照这张表构造 payload 或做自检时会得出"身份不一致"的结论,然后自行阻塞或改写。runtime 会覆盖 identity 字段,不至于误路由,但会诱导 agent 走失败分支(按 HARD RULE,AI skill guide 必须和实现同步)。`ralph-tools-emit.md:662` 还要求 payload 带 `candidate_commit_sha` / `artifact_refs`,schema 里都没有,属于多余负担。

---

## 4. 为什么静态门禁和测试都是绿的

- `preset check --strict` 只验证 topology、shape 和 ownership,不验证 "DAG 模式下某个 topic 有没有生产方"(O2)以及"runtime 合成事件的归属是否合法"(R2)。
- DAG 的单测大多直接调用 seam(`observe_unit_event_dag` / `on_unit_integrated_accepted`),或者往 store 灌 evidence,绕过了真实 git 和真实 EventLoop 接受链。唯一的真实 canary 只到 Execute。
- BDD / e2e 大量使用 wave 时代的场景(`parallel_forge_worktrees_ready_gate_runtime.yml` 等),用 mock 文本事件驱动,覆盖不到 R1-R4。

## 5. 建议的修复顺序与最小终验

1. R1(一行修复)+ R2(失败事件归属)→ 加一条真实 git canary:Execute(commit)→ Review spawn 成功 → `forge.unit.reviewed` 被接受;
2. R4 + O2 一起处理:确定"集成到独立 integration 分支,由 finalizer ff 回宿主",同时补上 DAG 产出 `target_start_sha` / `integration_branch` 的 runtime 投影,替代 `forge.worktrees.ready`;
3. O1:模板加 `tests:`,`forge.plan.ready` 对空 `tests` / `allowed_paths` 确定性拒收;
4. R3 + R7:修正从 Fix 重入;卡死时 fail-close 成 `forge.plan.blocked`;
5. R5 / R6 / O3 / O4 / O5 / S1。

**最小终验**:用一个 2-Unit 的计划(U2 depends_on U1,各有 `tests` 和 `allowed_paths`),配 mock backend(会真实 commit),在干净的临时仓库里跑完整 loop,断言事件序列:
`forge.unit.executed×2 → reviewed×2 → verified×2 → integrated×2 → forge.exec.development.done → full.verified → audit.done → finalized → cleanup.done → report.done(COMPLETED)`,并检查宿主 `git status` 干净、U2 的 worktree 包含 U1 的代码。

## 6. 方法与边界

- 证据等级:R1、R4 的 git 行为已在本机实测;其余结论为逐环读码闭合,没有跑完整的真实 DAG loop(用户要求只出报告)。
- 已修复项(09-17 的 F1-F6,commit `0736c2b9`)不在本次范围内,这次没有复现它们。
- 输入材料:`presets/en/parallel-forge.yml`、`presets/schemas/parallel-forge.yml`、`presets/templates/parallel-forge/*`、`crates/ralph-cli/src/loop_runner/dag_scheduler/*`、`crates/ralph-core/src/{event_origin.rs,event_loop/parse_and_emit/legacy.rs,supervisor/integration_lane.rs,supervisor/changed_path_guard.rs,parallel_forge_handoff.rs}`、`crates/ralph-core/data/ralph-tools*.md`,以及 docs/report 中的历史 parallel-forge 报告。
