---
title: Parallel Forge 跨 Hat / 跨 Wave Worktree Execution Resume
type: feat
date: 2026-08-03
origin: docs/plans/2026-08-03-003-feat-parallel-forge-trusted-worktree-reuse-plan.md
artifact_contract: ce-unified-plan/v1
artifact_readiness: implementation-ready
execution: code
---

# Parallel Forge 跨 Hat / 跨 Wave Worktree Execution Resume

## 0. 计划状态

- **状态：READY。** 目标限定为 `builtin:parallel-forge`；`ce-executor-pipeline` 只作为参考 preset，不修改、不纳入回归范围。
- **基线：** `pittcat-dev` HEAD `eaa2aab9`（2026-08-03）。
- **调查范围：** `--reuse-worktree`/`--continue` 启动路径、`task.resume` 目标 hat 与原始 trigger 机制、Parallel Forge preset 的 planner/guardian/worktree/dispatcher/executor/reviewer/integrator/verifier/correction/reporter 链、现有 wave BDD 和 worktree cleanup。
- **已执行验证：** 源码读取、preset/schema/BDD 搜索、`task.resume` payload 与 `WaveContextForResume` 调用链检查、相关 Git history 检查。
- **尚未执行：** 尚无生产实现，未运行实现阶段测试/build/lint；实现阶段命令见第 9 节。
- **阻塞项：** 无。若恢复 manifest 无法在不复制历史终态事件、不绕过 event policy 的情况下进入当前 Parallel Forge，必须停止 U1 并将计划改为 BLOCKED。

## 1. 功能目标

### 业务目标

用户用同一个 plan、同一个 Parallel Forge preset 和已存在的 worktree 重新运行时，系统应从上一次运行最后一个可信执行边界继续，而不是把整个 Parallel Forge 当作首次运行，也不是只根据目录或旧 artifact 猜测已完成。

目标命令保持用户形态：

```bash
ralph run --worktree --reuse-worktree \
  -H builtin:parallel-forge \
  --plan docs/plans/<plan>.md \
  -c ralph.pipeline.yml
```

### 当前行为

`run.rs` 的 `--worktree --reuse-worktree` 路径按 worktree name 找到旧 worktree，调用 `clean_worktree_runtime_artifacts`，然后创建/继续一个新的 `LoopContext`。cleanup 会归档旧 runtime，并删除 live runtime；它没有恢复 Parallel Forge 当前 hat、wave、原始 trigger 或未完成 activation 的协议。

`--continue` 是另一条 live-state 路径：检查 scratchpad，复用 `current-loop-id`，由 `EventLoop::initialize_resume` 产生 `task.resume`。它不能直接覆盖 reuse-worktree 在 cleanup 后已被归档的跨运行场景。

### 目标行为

`--reuse-worktree` 在 cleanup 前先创建并验证一个 resume manifest，然后启动新的 loop。新的 loop 根据 manifest：

- 已有可信终态事件的 hat：跳过该 hat，重放其已接受的 handoff 到下一条 Parallel Forge 边界；
- 没有终态事件但有中断 checkpoint 的 hat：以原始 trigger、hat、wave metadata 重新激活该 hat；
- 只有 artifact、没有接受事件的结果：不得直接视为成功，重跑 producer hat；
- wave 已经通过 `forge.wave.settled`：复用已关闭的当前 wave，进入 dispatcher 的下一 wave；
- evidence、plan/config/Git identity 或 event chain 矛盾：`plan.blocked`，不关闭 task、不推进 wave；
- 当前 run 完成后仍走现有 reviewer/integrator/verifier/correction/reporter 链。

### 行为差异

旧行为是“归档旧 runtime 后 fresh start”；目标行为是“归档旧 runtime，同时把最后可信 handoff 转换成当前 run 的受控 resume bootstrap”。恢复的是执行边界，不恢复不可恢复的 LLM 上下文。

### 范围

- 仅接入 `builtin:parallel-forge`。
- 支持 planner、guardian、worktree、forge-dispatcher、executor、reviewer、integrator、verifier、tester、auditor、reporter 和 correction handler 的恢复边界。
- 复用现有 `task.resume`、`target_hat`、`original_trigger_topic`、`original_trigger_payload`、`wave_id/wave_index/wave_total` 机制。
- 保持现有 wave settlement、`CloseTaskBatch`、correction exhaustion 和 fail-close authority。

### 非目标

- 不修改 `ce-executor-pipeline` preset 或其 instruction。
- 不把 Parallel Forge 改成线性 pipeline。
- 不恢复已经丢失的 LLM 上下文；中断 activation 只能依据持久化 checkpoint 重放原始 trigger。
- 不把 raw archived events 直接复制进当前 events ledger。
- 不按 artifact 存在、旧 task ID、目录名或 hat 名称直接判定成功。

### 输入、输出与错误

- 输入：worktree identity、plan path/digest、preset identity、config digest、旧 archive、最后可信事件、hat/wave/task metadata、checkpoint artifact。
- 输出：versioned `parallel-forge-resume-manifest.v1`、current-run `task.resume` bootstrap 或 `forge.plan.blocked`。
- 状态：当前 run 只在 accepted resume handoff 后推进；历史 verdict、retry budget、correction round 不直接继承。
- 错误：manifest 缺失、字段矛盾、原始 trigger 缺失、目标 hat 不存在、wave metadata 不一致、task identity 无法映射时 fail-closed。

### Requirements

- **R1.** cleanup 在删除 live runtime 前必须生成并完成校验 `parallel-forge-resume-manifest.v1`；不完整 manifest 不得启动 resume loop。
- **R2.** manifest 必须绑定 plan path/digest、preset/config digest、worktree/source HEAD、loop identity、pending hat、original trigger、wave metadata、accepted terminal events、task/unit mapping 和 checkpoint artifact digest。
- **R3.** resume 必须使用现有 `task.resume` recovery channel 的 target hat、original trigger 和 wave metadata；不得复制历史 events ledger 或伪造 business terminal event。
- **R4.** 已接受 terminal event 才能跳过 hat；只有 artifact、没有 terminal event 时必须重放该 hat 的原始 trigger。
- **R5.** Parallel Forge 的 task close、wave settlement、correction exhaustion 和 fail-close authority 保持不变；resume 不得直接调用 task store 关闭任务。
- **R6.** 相同 manifest 的重复 bootstrap 必须幂等；plan/preset/config/Git/worktree identity 漂移、manifest digest mismatch、target hat/trigger 缺失必须 `forge.plan.blocked` 且零业务副作用。
- **R7.** 本计划不得修改 `ce-executor-pipeline` 的 preset、instruction、schema、测试或执行语义。

## 2. 代码库现状与证据

### 2.1 当前调用链

```text
ralph run --worktree --reuse-worktree
  → crates/ralph-cli/src/commands/run.rs
  → find_reusable_worktree_by_name
  → clean_worktree_runtime_artifacts
  → LoopContext::worktree
  → loop_runner::run_loop_impl
  → current starting_event / EventLoop
```

当前 Parallel Forge 的业务链：

```text
forge.start
  → forge.plan.inspected
  → forge.plan.ready
  → forge.concurrency.approved
  → forge.worktrees.ready / forge.wave.worktrees.ready
  → exec.unit.ready → exec.unit.done|failed
  → exec.wave.complete|failed
  → forge.wave.reviewed|review.failed
  → forge.wave.integrated
  → forge.wave.verified|verification.failed
  → forge.wave.settled
  → next forge.wave.prepare 或 forge.exec.development.done
  → forge.full.verified → forge.audit.done → forge.report.done → LOOP_COMPLETE
```

### 2.2 Evidence Ledger

| ID | 来源 | 观察结果 | 对计划影响 | 可靠性 |
|---|---|---|---|---|
| E1 | `crates/ralph-cli/src/commands/run.rs:840-850,1065-1085` | `--continue` 检查 scratchpad；`--reuse-worktree` 先 cleanup，再进入新的 worktree loop。 | 新能力必须桥接 reuse 与 resume，不能只扩展 `--continue`。 | 高 |
| E2 | `crates/ralph-cli/src/loop_runner/runner.rs:360-372,462-521,1222-1225` | resume 复用 `current-loop-id`，`initialize_resume` 使用 `task.resume` 而非 fresh `task.start`。 | reuse bootstrap 应沿用已有 resume 入口和 loop identity 规则。 | 高 |
| E3 | `crates/ralph-core/src/event_loop/rejection.rs:451-662` | 现有 `build_task_resume_payload` 已携带 `target_hat`、`original_trigger_topic`、`original_trigger_payload`、`original_hat` 和 wave metadata。 | 不新增第二套 hat resume 消息；扩展现有 recovery payload/manifest contract。 | 高 |
| E4 | `crates/ralph-core/src/event_loop/loop_state.rs:548,686-695,1554-1562` | runtime 有 `pending_recovery_hat`，并要求 resume 指向原始 trigger，避免恢复到错误 hat。 | 中断 hat 的恢复必须绑定 target hat + 原始 trigger digest。 | 高 |
| E5 | `crates/ralph-core/src/event_loop/stages/repair_dispatch_stage.rs:160-205` | `task.resume` 进入主 bus，由 target hat 消费；预算耗尽会 fail-close。 | resume bootstrap 必须经过现有 recovery budget/policy，不直接注入 agent prompt。 | 高 |
| E6 | `presets/en/parallel-forge.yml:298-1077` | Parallel Forge hat 链明确由 topics 串接；dispatcher 消费 worktree/settled，integrator 产生 settled，reporter 产生 report.done。 | manifest 必须记录每个 hat 的原始 trigger 与 accepted terminal boundary，而不是只记录目录。 | 高 |
| E7 | `presets/en/parallel-forge.yml:757-835` | executor 由 `exec.unit.ready` 激活，task 只在 `forge.wave.settled` 由 `CloseTaskBatch` 关闭。 | executor 中断不能直接 close task；恢复需重新派发未完成 Unit，settlement 仍由现有 authority 完成。 | 高 |
| E8 | `presets/en/parallel-forge.yml:918-1005` | integrator/verifier 的顺序是 `forge.wave.integrated → forge.wave.verified → forge.wave.settled`；failure 进入 correction/blocked。 | 已完成 handoff 可跳过，缺终态的中间边界必须从最近合法 topic 重放。 | 高 |
| E9 | `crates/ralph-core/src/worktree.rs:748-884` | cleanup 当前逐项 rename runtime，写 `resume-context.md`，不生成结构化 resume manifest。 | U1 新增 manifest，但不能改变 tracked code/symlink 保护。 | 高 |
| E10 | `crates/ralph-core/tests/scenarios.rs:1803-1902,1935-1944` 及 `parallel_forge_*_runtime.yml` | BDD 使用真实 EventLoop，已有 task dispatch、duplicate handoff、two-wave settlement、correction、exhaustion、fail-close 场景。 | 新 resume 场景必须使用同一 `run_workflow_guard_scenario`，不可 source-only。 | 高 |
| E11 | `presets/en/ce-executor-pipeline.yml:1-40,1836-2200` | ce-executor-pipeline 是独立线性 preset；plan-reviewer 有 reuse guidance/flow audit，executor 用 Unit bill 和 checkpoint；它不是 Parallel Forge 的实现入口。 | 只提取 reuse guidance、checkpoint、handoff 设计模式；明确不改该 preset。 | 高 |
| E12 | `crates/ralph-core/src/event_loop/rejection.rs` tests around `build_task_resume_payload_includes_wave_context` | 已有测试证明 wave resume payload 包含 `wave_id/index/total`、`original_hat`、`target_hat`。 | U2 可以在该能力上增加 Parallel Forge resume manifest，而非另造字段体系。 | 高 |
| E13 | `crates/ralph-cli/tests/integration_resume.rs`、`integration_worktree_isolation.rs`、`integration_run.rs` | CLI 已有 resume、worktree isolation 和 run integration 测试入口。 | U1/U2 必须扩展这些真实 binary 边界，不创建只测 source text 的替代测试。 | 高 |

### 2.3 受影响范围

- 生产入口：`crates/ralph-cli/src/commands/run.rs`、`crates/ralph-cli/src/loop_runner/runner.rs`、现有 `crates/ralph-core/src/worktree.rs`。
- 恢复协议：`crates/ralph-core/src/event_loop/rejection.rs`、`loop_state.rs`、`repair_dispatch_stage.rs`。
- Parallel Forge contract：`presets/en/parallel-forge.yml`、`presets/schemas/parallel-forge.yml`、相关 `presets.rs` parity。
- 测试：`crates/ralph-core/tests/scenarios.rs`、现有 `parallel_forge_*_runtime.yml`、CLI worktree/run integration tests、runner/recovery tests。
- 文档：仅 Parallel Forge 和通用 agent resume guidance；`ce-executor-pipeline.yml` 不变。

## 3. 决策记录与置信度

| ID | 决策问题 | 候选 | 最终选择 | 证据 | 置信度 |
|---|---|---|---|---|---|
| D1 | 恢复消息是否新增 topic | 新 topic；复制旧 business event；复用 `task.resume` | 复用 `task.resume`，增加 manifest 驱动的结构化 resume payload | E2-E5,E12 | 0.92 |
| D2 | 历史事件如何进入新 ledger | 原样复制；重新 emit 原 business event；orchestrator-owned resume bootstrap | 使用 orchestrator-owned `task.resume`，payload 带原始 trigger snapshot，禁止复制历史 ledger | E2-E5 | 0.90 |
| D3 | hat 中断时如何判定成功 | artifact 存在即成功；只有终态 event 成功；按 artifact+event 双证据 | 只有 accepted terminal event 才能跳过；无终态则重放原始 trigger | E3,E6,E8 | 0.94 |
| D4 | wave 完成如何推进 | 新增 reuse close authority；伪造 settled；复用现有 settled/state projection | 复用已接受 `forge.wave.settled` 作为历史证据，当前 run 只重建合法下一-wave handoff；task close 仍由现有 projection | E7,E8 | 0.88 |
| D5 | resume scope | 所有 preset；Parallel Forge；修改 ce-executor-pipeline | 第一版只实现 Parallel Forge，通用 recovery primitive 只作必要共享改动；不改 ce-executor-pipeline | E6,E11 | 0.95 |
| D6 | 旧 retry/correction 如何处理 | 继承；全部清零；按当前未完成 wave 合同恢复 | 旧预算不直接继承；当前 correction/wave contract 重新计算，历史只作为 evidence | E6-E8 | 0.88 |

## 4. BDD 行为规格

### Feature: Parallel Forge worktree resume

Background:

  Given 当前命令使用同一个 Parallel Forge plan、preset、config 和显式 worktree
  And旧 run 已写入或尝试写入结构化 resume manifest

Scenario S1: planner 之后中断，从 planner handoff 恢复

  Given `forge.plan.ready` 已被接受但 `forge.concurrency.approved` 尚未产生
  When operator 使用 `--reuse-worktree` 重启
  Then 不重新执行 `forge.start` 的 plan inspection
  And恢复到 guardian 的原始 trigger

Scenario S2: executor wave 中断，已完成 Unit 不重复派发

  Given wave 中 Unit A 已产生 accepted `exec.unit.done`，Unit B 尚无终态事件
  When恢复该 wave
  Then Unit A 不再次派发
  AndUnit B 使用原始 `exec.unit.ready` trigger 重放
  Andtask 只在后续 `forge.wave.settled` 关闭

Scenario S3: reviewer/integrator/verifier 边界中断

  Given `forge.wave.integrated` 已接受但 `forge.wave.verified` 未接受
  When恢复
  Then只恢复 verifier，不重新执行 executor/reviewer/integrator

Scenario S4: correction 中断

  Given `forge.correction.requested` 已接受但 `forge.correction.done` 未接受
  When恢复
  Then目标 hat 为 correction executor，wave metadata 与 failure fingerprint 一致
  And不继承已耗尽的旧 correction round

Scenario S5: artifact 存在但 terminal event 缺失

  Given某 hat 的 output artifact 存在但 accepted terminal event 不存在
  When恢复
  Then该 hat 重放原始 trigger
  Andartifact 不单独证明完成

Scenario S6: manifest identity 漂移

  Given plan/config/preset/Git identity 与旧 manifest 不一致
  When恢复
  Then产生 `forge.plan.blocked`
  And不关闭 task、不推进 wave、不派发 executor

Scenario S7: accepted resume 重放幂等

  Given同一个 resume manifest 已经 bootstrap 一次
  When相同 manifest 再次 bootstrap
  Then不重复插入恢复 obligation、不重复派发已接受 handoff

## 5. 验收与测试策略

| Scenario | 验收断言 | 测试入口/层级 | 风险测试 |
|---|---|---|---|
| S1 | guardian 只收到一次原始 plan handoff；事件顺序合法 | real EventLoop BDD | interrupted bootstrap |
| S2 | A 无第二次 `exec.unit.ready`；B 有一次；settled 才 close task | real EventLoop BDD + dispatcher unit | idempotency/state machine |
| S3 | verifier 被恢复，前置 hat activation count 不增加 | real EventLoop BDD | terminal boundary characterization |
| S4 | target hat/failure fingerprint/wave fields 完整，旧 budget 不继承 | real EventLoop BDD + recovery unit | correction exhaustion |
| S5 | artifact-only 不跳过 hat | core decision unit | tamper/missing event |
| S6 | exact blocked reason，零 state side effect | CLI/runtime integration | plan/config/Git drift |
| S7 | manifest digest replay no-op | core + EventLoop integration | duplicate bootstrap |

## 6. 需求—测试追踪矩阵

| Requirement | 需求 | Scenario | 测试 | Unit | Evidence |
|---|---|---|---|---|---|
| R1 | 保存 Parallel Forge resume manifest | S1-S5 | manifest unit/integration | U1 | E6,E9 |
| R2 | target hat + original trigger 恢复 | S1-S4 | recovery payload tests | U2 | E3-E5,E12 |
| R3 | accepted terminal event 才可跳过 | S2,S3,S5 | EventLoop BDD | U2/U3 | E6-E8 |
| R4 | wave/task authority 不变 | S2,S3 | existing settlement regression + BDD | U3 | E7,E8 |
| R5 | identity 漂移 fail-closed | S6 | CLI/runtime integration | U1/U2 | E1,E6,E9,E13 |
| R6 | resume replay 幂等 | S7 | projection/replay BDD | U3 | E4,E5 |
| R7 | ce-executor-pipeline 不受影响 | regression | existing preset lint/parity tests | U4 | E11 |

## 7. 严格串行开发单元

### Unit 1：Parallel Forge Resume Manifest 与 Worktree Bootstrap

- **目标：** 在 cleanup 前记录旧 run 的 Parallel Forge identity、accepted event boundary、pending hat、original trigger、wave metadata、task/unit mapping 和 artifact references，并在新 run 启动前完成完整性校验。
- **修改位置：** `crates/ralph-core/src/parallel_forge_resume.rs`（计划新增，保存 manifest/identity/decision DTO）、`crates/ralph-core/src/lib.rs`、`crates/ralph-core/src/worktree.rs`、`crates/ralph-cli/src/commands/run.rs`、`crates/ralph-cli/tests/integration_worktree_isolation.rs`、`integration_resume.rs`。
- **当前 Red：** 当前 cleanup 只有 timestamp archive 和 `resume-context.md`，没有结构化 manifest；新增 S1/S5 integration test 应因 manifest 不存在失败。
- **最小实现：** manifest v1、source identity digest、accepted/pending boundary、原始 trigger snapshot、bounded path validation；不执行 resume，不修改 Parallel Forge topology。
- **测试顺序：** manifest schema Red → identity Red → interrupted boundary Red → tamper/partial archive Red → Green → Refactor → run integration → worktree regression。
- **完成标准：** S1/S5 的 manifest 与 identity assertions 通过；cleanup failure 不启动 loop；tracked code/symlink 和现有 cleanup tests 全绿。
- **停止条件：** 无法从现有 events/history/hat channel 得到唯一最后边界，或 cleanup 与现有 worktree protection 冲突。

### Unit 2：Target Hat Resume Payload 与 Parallel Forge Hat Handoff

- **目标：** 使用已有 `task.resume` recovery contract，把 manifest 中的 pending hat 重新绑定到原始 trigger；已有 accepted terminal boundary 直接交给下一合法 Parallel Forge topic。
- **修改位置：** `crates/ralph-core/src/event_loop/rejection.rs`、`loop_state.rs`、`repair_dispatch_stage.rs`（仅必要扩展）、`crates/ralph-cli/src/loop_runner/runner.rs`，以及 recovery unit tests。
- **可复用能力：** `target_hat`、`original_trigger_topic/payload`、`WaveContextForResume`、`pending_recovery_hat`、existing resume budget/policy。
- **当前 Red：** 新 manifest bootstrap 尚不能把 Parallel Forge 的 arbitrary pending topic 交给指定 hat；新增 S1-S4 recovery tests 应在目标 hat/trigger assertion 失败。
- **最小实现：** manifest → structured `task.resume` conversion、target hat validation、original trigger digest validation、wave metadata propagation；不改变普通 `--continue` 语义。
- **测试顺序：** target validation Red → original trigger Red → wave fields Red → budget/fail-close Red → Green → Refactor → runner integration。
- **完成标准：** S1-S4 payload 与 target assertions 通过；existing hard-gate/recovery tests 通过；错误恢复不 round-robin 到无关 hat。
- **停止条件：** `task.resume` policy 无法安全承载 Parallel Forge recovery，或需要复制历史 business event 才能恢复。

### Unit 3：Parallel Forge Wave Resume、Task/Settlement 与幂等

- **目标：** 在真实 Parallel Forge chain 中恢复 executor/reviewer/integrator/verifier/correction 的中断边界，并保持 wave settlement/task close authority 不变。
- **修改位置：** `presets/en/parallel-forge.yml`、`presets/schemas/parallel-forge.yml`、必要的 `crates/ralph-cli/src/presets.rs` parity、`crates/ralph-core/tests/scenarios.rs` 和新增 resume BDD fixtures；仅当 Rust dispatcher 不能消费既有 recovery state 时才修改 dispatcher，并以 Red 证据证明。
- **当前 Red：** 现有 scenarios 没有“旧 run 中断后新 run 只恢复 pending hat”的 runtime path；S2-S4 应在重复 activation 或缺少 resume route 处失败。
- **最小实现：** accepted terminal topic skip、pending trigger replay、wave/task identity check、duplicate bootstrap no-op；不新增 task-close authority、不伪造 `forge.wave.settled`。
- **测试顺序：** S2 executor Red → S3 verification Red → S4 correction Red → S7 replay Red → Green → Refactor → existing settlement/correction/exhaustion/fail-close regression。
- **完成标准：** `run_workflow_guard_scenario` 真实执行 S2-S4/S7；现有 two-wave settlement、correction、round exhaustion、fail-close 全绿。
- **停止条件：** 恢复需要直接关闭 task、复制旧 settled event、改变 wave authority 或引入第二套 dispatcher state。

### Unit 4：Parallel Forge Operator Contract 与完整回归

- **目标：** 让 Parallel Forge operator 能继续使用原命令，并能看到 resume/blocked 原因；同步通用 agent resume guide，但不改 ce-executor-pipeline 文档或 preset。
- **修改位置：** Parallel Forge 相关 `crates/ralph-core/data/*.md`、`AGENTS.md`/`CLAUDE.md`（保持一致）、Parallel Forge preset instructions、zsh completion（若 builtin help surface 改变）和 docs/CLI tests。
- **当前 Red：** 文档未描述 Parallel Forge resume manifest、pending hat、原始 trigger 和停止条件；help/doc drift/skill anchor 应先暴露缺口。
- **最小实现：** 命令、字段来源、resume/reuse/rerun/block 语义和 fail-close 操作说明；不写内部 ledger 路径或 ce-executor-pipeline 专用规则。
- **测试顺序：** help/doc Red → data guide Green → AGENTS/CLAUDE parity → zsh/anchor → `./scripts/run-tests.sh`。
- **完成标准：** R7 回归通过；`ce-executor-pipeline.yml` 与其测试无变更且 existing parity 通过。
- **停止条件：** 文档需要引入未经 U1-U3 验证的字段或改变其他 preset contract。

### 7.1 预期文件变更

| 位置 | 类型 | 边界 |
|---|---|---|
| `crates/ralph-core/src/parallel_forge_resume.rs` | 新增生产模块与单测 | Parallel Forge manifest、identity、accepted/pending boundary、digest 和 bootstrap DTO |
| `crates/ralph-core/src/lib.rs` | 修改生产文件 | 注册 core resume module |
| `crates/ralph-core/src/worktree.rs` | 修改生产文件与现有测试 | cleanup 前采集 manifest、staging、完整性和 live/archive 边界 |
| `crates/ralph-cli/src/commands/run.rs` | 修改生产文件 | reuse-worktree 读取 manifest、调用 resume bootstrap；保持 worktree name binding |
| `crates/ralph-cli/src/loop_runner/runner.rs` | 修改生产文件与测试 | new-run resume bootstrap、loop identity、现有 `task.resume` 入口接入 |
| `crates/ralph-core/src/event_loop/rejection.rs`、`loop_state.rs`、`repair_dispatch_stage.rs` | 修改生产文件与测试 | manifest 字段映射到 target hat/original trigger/wave context、budget/fail-close |
| `presets/en/parallel-forge.yml`、`presets/schemas/parallel-forge.yml` | 修改配置/schema | 仅补充 Parallel Forge resume-required fields/accepted handoff，不改变正常 wave topology |
| `crates/ralph-core/tests/scenarios.rs`、`tests/scenarios/parallel_forge_resume_*.yml` | 测试 | S1-S7 真实 EventLoop BDD |
| `crates/ralph-cli/tests/integration_resume.rs`、`integration_worktree_isolation.rs`、`integration_run.rs` | 测试 | 真实 CLI reuse/resume/worktree contract |
| `crates/ralph-core/data/*.md`、`AGENTS.md`、`CLAUDE.md`、`scripts/ralph-zsh-plugin.zsh` | 文档/补全 | Parallel Forge resume 操作契约；不修改 ce-executor-pipeline |

明确不修改：`presets/en/ce-executor-pipeline.yml`、其 schema、测试和 instruction。

## 8. Unit 串行依赖图

```text
U1 manifest/bootstrap
  ↓ 已验证 identity、pending boundary、original trigger
U2 target-hat task.resume handoff
  ↓ 已验证 recovery payload 与 runner 路由
U3 Parallel Forge wave/task/settlement resume
  ↓ 已验证真实 BDD 和现有 settlement regression
U4 operator/docs/final regression
```

U2 不能先于 U1，因为没有可信 manifest 就无法确定恢复 hat；U3 不能先于 U2，因为 preset 不应自行构造 recovery payload；U4 不能先于 U3，因为文档必须描述已通过真实 runtime 的字段和停止条件。

## 9. 执行命令清单

- `cargo nextest run -p ralph-core -- worktree rejection task_resume loop_state`：U1/U2 单元 Red/Green；失败不得进入下一步。
- `cargo nextest run -p ralph-cli --test integration_resume`：现有真实 binary resume contract；U2 先扩展并运行该入口。
- `cargo nextest run -p ralph-cli --test integration_worktree_isolation`：现有 reuse-worktree/worktree isolation boundary；U1 扩展并运行该入口。
- `cargo nextest run -p ralph-cli --test integration_run`：run command/preset startup regression；U1/U2 运行。
- `cargo nextest run -p ralph-core --test scenarios -- parallel_forge`：U3 真实 EventLoop BDD；必须断言 topics、hat、task/state 和副作用。
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`：Parallel Forge preset/schema lint。
- `cargo nextest run -p ralph-core -- preset_lint`：core preset contract regression。
- `cargo nextest run -p ralph-cli --bin ralph -- presets`：embedded/source preset parity。
- `scripts/check-cli-doc-drift.sh`：U4 CLI/data docs drift。
- `./scripts/run-tests.sh`：最终 workspace 两阶段 nextest + doctest；完成前不得宣称计划完成。
- `cargo run -p ralph-e2e -- --mock`：Parallel Forge 共享 E2E contract 回归；U3 修改共享启动/recovery contract 后必须执行。

禁止裸跑 `cargo test -p ralph-cli` 或 `cargo test -p ralph-cli --bin ralph`。

## 10. 最终质量门禁

- S1-S7 全部通过；无新增 skip/only/ignored。
- 任意中断 hat 都能按 manifest 恢复到正确 target hat 或明确 blocked。
- accepted terminal event 才能跳过 hat；artifact-only 不得跳过。
- executor task 仍只在现有 `forge.wave.settled → CloseTaskBatch` 路径关闭。
- wave/correction/retry/exhaustion/fail-close 现有场景全绿。
- resume bootstrap 幂等；重复启动不重复派发或推进 wave。
- plan/preset/config/Git/worktree identity 漂移 fail-closed。
- `ce-executor-pipeline.yml`、其 instruction、schema 和测试不被修改。
- lint、build、nextest、docs drift、preset parity 和必要 E2E 全部通过。

## 11. 最终计划自检

| 检查项 | 结果 | 说明 |
|---|---|---|
| 这是 Parallel Forge 专用计划 | 是 | 目标和生产变更均限定 Parallel Forge；ce-executor-pipeline 只作 E11 参考。 |
| 是否解决跨 hat / 跨 wave resume | 是 | S1-S4 覆盖 planner、executor、verification、correction 边界。 |
| 是否保留现有 settlement authority | 是 | U3 明确禁止第二套 task close 或伪造 settled。 |
| Executor 是否仍需关键设计决策 | 否 | D1-D6 固定 resume channel、证据边界、scope 和 authority。 |
| 每个 Unit 是否有真实 Red、Green、Integration、Regression | 是 | U1-U4 均明确闭环和命令。 |
| 是否修改 ce-executor-pipeline | 否 | 明确排除并设回归门禁。 |
| 是否存在未确认的低置信度决策 | 否 | 关键恢复通道均有 E2-E5/E12 直接证据。 |
| 是否可以严格串行执行 | 是 | U1 → U2 → U3 → U4。 |

## Appendix: 参考范围

`presets/en/ce-executor-pipeline.yml` 只用于参考 plan-reviewer 的 reuse guidance、flow audit、checkpoint 和 handoff 设计；本计划不得修改该 preset，也不得把其线性 topology 移植到 Parallel Forge。
