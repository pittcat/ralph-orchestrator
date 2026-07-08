---
date: 2026-06-20
topic: serial-preset-precheck-as-linter
status: superseded
superseded_by: docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md
superseded_revision: v3.1
related:
  - docs/report/2026-06-20-ce-executor-serial-primary-20260619-164313-loop-diagnosis.md
  - docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md
  - docs/brainstorms/2026-06-17-hat-orchestrator-state-projection-requirements.md
  - docs/plans/2026-06-18-001-fix-ce-executor-serial-recovery-handoff-plan.md
  - commit d623c09
supersedes: none
---

# ce-executor-serial Precheck-as-Linter 协议重构 — 需求文档

> **⚠️ SUPERSEDED（2026-06-20 v3.1）**  
> 本 brainstorm **不再作为实施 SSOT**。所有架构、需求、验收以  
> [`docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md`](../plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md) **v3.1** 为准。  
> 下文保留历史语境；与 plan 冲突处 **以 plan 为准**。

| 本 brainstorm（旧） | plan v3.1（现行） |
|---|---|
| SSOT = `ralph-proto/src/serial_protocol/` | SSOT = `presets/schemas/ce-executor-serial.yml` + `preset/engine/` |
| SP-R13 持久化 task.resume | LintResumeHint 内存态 + prompt（KTD-8） |
| SP-R14 超时 fallback policy_check | 超时 fail-closed（KTD-9） |
| SP-R17 off = 回 d623c09 | off = 仅关 linter；gate 仍走 engine |

## Summary

把 `ce-executor-serial` 当前 5 道 runtime 事后 gate（verdict_gate / progress_task_gate / execution_contract / hat_handoff gate / is_anonymous_business_topic）重新组织成"**协议 SSOT + Precheck-as-Linter**"机制：把 `ralph emit` 从"事后 fail-after"改为"事前 lint-before"。`ce-executor-isolated` 不在本次范围（按用户指示后续会从仓库移除）。

**North star**：`ralph emit <topic> --payload '<json>'` 之前必须先经过一个 **linter 阶段**，linter 镜像所有 runtime gate 的检查规则，告诉 agent "这个事件能发 / 不能发，缺哪个字段，会触发哪个 gate"，lint 不通过则**不落盘、不消耗迭代预算**。Runtime 兜底机制作为防御层保留（针对 loop_runner 内部 publish 等绕过路径），但**主战场从 runtime 拒收迁移到 lint-time fail**。

**配套策略（plan v3.1）**：将散落在 `presets/en/ce-executor-serial.yml` / agent instructions / runtime Rust 三处的 schema / gate 规则统一为 **`presets/schemas/ce-executor-serial.yml`（authoring SSOT）**；`build.rs` merge 进 embedded preset；**`preset/engine/`** 通用执行器从 `EventLoopConfig` / `ProtocolView` 读取，不在 Rust 重复字段表。

**历史草案（已废弃）**：~~`crates/ralph-proto/src/protocol.rs` / `serial_protocol` namespace~~ — 见 plan v3.1 KTD-10。

**渐进交付**：B+D 两段式——**Phase 1 (B)** 协议 SSOT 化（1 周，治漂移），**Phase 2 (D)** Precheck-as-Linter（再 1 周，治 agent 撞门）。每阶段独立可验收。

---

## Problem Frame

### 30 天同一条根因路径摔了 5 次

| Run | 日期 | preset | 终止原因 | 根因类别 |
|---|---|---|---|---|
| merry-lotus | 2026-06-17 | serial | 41m 后 cancelled | task.resume 缺字段 + dimension 卡死 |
| noble-peacock | 2026-06-17 | serial | 28m 后 cancelled | 同上同构 |
| perky-maple | 2026-06-18 | serial | 2h7m 后用户 abort | fix.applied dedup 缺 fix_round |
| warm-tiger | 2026-06-18 | serial | 用户 abort | dimension-reviewer 卡死 + 越权 emit |
| **本 run** | **2026-06-19** | **serial** | **2h25m, 41 iter, consecutive_failures** | **state_projection 缺 Completed Steps + executor task.resume 不响应 + hat_handoff 0 触发 + 8 对重复 emit** |

每次死循环后出 1 份诊断报告 + 1 份加固 plan + 1 份加固 commit（d623c09 是最新一次）。**30 天累计 6 份诊断报告 + 5 份加固 plan + 1 份加固 commit**。**没有一次把根因打掉**——每次都打到根因的某个儿子，根因本身（共享可变状态 + 散落 schema + fail-after 模式）每次都活下来。

### 4 个反复出现的根因（耦合，不独立）

| 根因 | 历史出现 | 本次状态 | 治法 |
|---|---|---|---|
| **State projection 不写 Completed Steps** | warm-tiger P2-B（同症状，根因反转） | **P0-A**：progress.md 45 字节从未更新 | Phase 1 SSOT 化 + state_projection 补 `mark_step_completed` |
| **progress_task_gate 误拒 / 范围过窄** | warm-tiger P2-B + 本次 P0-A | **P0-A**：3 次 progress_not_found 误拒 | Phase 1 SSOT 化 + Phase 2 linter 提前告知 |
| **agent 撞门不响应（task.resume 后无 emit）** | warm-tiger P0-B（dimension）→ 本次 P0-B（executor） | **P0-B**：8m stall | Phase 2 linter 提前告知 + task.resume prompt 强化 |
| **重复 emit（`--policy-check` 落盘 / agent 误判丢失重发）** | perky-maple P1-2（135 条）→ 本次 P1-A（8 对） | **P1-A**：8 对重复 | Phase 2 linter 把 precheck 改为不落盘 |

**用户已确认**：4 个根因**耦合**，必须**一起改**——只改任何一个都无法独立收敛。

### 为什么是 4 个一起改的根因

散落 schema + 共享可变状态 + fail-after 三个条件同时满足时，必然产生"4 个儿子"：
1. 共享可变状态（progress.md / tasks.jsonl / plan frontmatter）→ **任一 hat 可改 → 漂移**
2. schema 散落（preset / instructions / runtime）→ **改一处漂一处 → drift 永远抓不干净**
3. fail-after（emit 后 gate 拒收）→ **agent 看不到 gate 视角 → 撞门才知**

任何一个儿子死掉，另几个会**换一种形式重新出现**。这就是为什么 30 天打了 5 个 patch 都没收敛。

### 谁在受影响

- **Operator（用户本人）**：30 天里在手动诊断循环里度过，**最重负担是"诊断循环本身"**——他说"手动诊断+打补丁"
- **Agent**：每次撞门消耗 ~3-8 分钟 stall + 1 次 LLM 调用 + 0 业务进展
- **机制层开发者**：每加一道 gate 都在擦共享状态屁股，carrying cost 单调上升
- **未来维护者**：现有架构如不重构，30 天后会再摔 5 次

### 现状的 cost shape

- 30 天：6 份诊断报告 + 5 份加固 plan + 1 份加固 commit
- 5 次死循环每次 ~30 分钟 ~ 2 小时，**agent 视角看 5 次白跑**
- 现有 serial preset 成功率 < 50%（5 次中 0 次跑完 plan 全部 12 U）

---

## Key Decisions

### D-1. 选 D 方案（Precheck-as-Linter + 协议 SSOT）

**理由**：
- 用户原话"precheck 跑之前的检查,比跑之后 fail 好"明确指向 Linter 方向
- 用户答"只选一个就会失败"——4 个根因必须一起改
- 用户答"同意重构,接受 1-2 周中断"——B+D 两段式有 1-2 周预算
- A / B / C 单独做都治不了根：单 A 治不了撞门（linter 还是要加）；单 B 治不了漂移（agent 还是看不到 schema）；单 C 加 hat 违反 Ralph 哲学

**trade-off**：
- **整体重构 vs 增量加固**：放弃增量加固，换取根本性收敛
- **运行时 reject + 落盘 + recovery vs lint-time fail + 不落盘 + 立即 feedback**：重构后，95% 的 recovery.jsonl 不再产生（recovery 是 fail-after 的副产品）；5% 兜底保留（针对 loop_runner 内部 publish 等绕过路径）
- **协议 SSOT 选 Rust 代码 vs 选 JSON Schema**：选 Rust 代码 + derive，因为同仓 Rust 代码本身已经在跑协议生成，JSON Schema 会引入新工具链

### D-2. 两段式交付（Phase 1 = B，Phase 2 = D）

| 阶段 | 内容 | 工期 | 验收标准 | 失败回退点 |
|---|---|---|---|---|
| **Phase 1** | 协议 SSOT 化（`presets/schemas/` + build.rs merge） | 1 周 | 改 schemas 只动 1 处；inline 双写清除 | 不改 emit/linter；**会改** state_projection（U3a/U3b） |
| **Phase 2** | Precheck-as-Linter 阶段 | 1 周 | 同 plan 跑完 LOOP_COMPLETE | Phase 1 已治漂移，可独立停下 |

**为什么分两段**：
- Phase 1 风险低（不改 runtime 行为，只动 SSOT 派生），可独立验收
- Phase 2 风险中高（加 linter 阶段引入延迟 + 改变 emit 路径），失败可回滚到 Phase 1
- 每段都有独立价值：Phase 1 治漂移，Phase 2 治撞门

### D-3. isolated preset 不动

按用户原话："isolated yml 不需要改，不用管，这个 yml，后续会去掉"。

**scope 限制**：
- 只动 `ce-executor-serial` 专用代码路径
- `crates/ralph-proto/src/protocol.rs` 公共 SSOT 允许 serial 单独引入独立 namespace，不与 isolated 共享
- 现有 2026-06-20-001 plan 中 U5b / U7 / U6 等跨 preset 改动，**只对 serial preset 生效**
- `ce-executor-isolated.yml` 在本次改动中**不被修改、不被读取、不被验证**

### D-4. 协议 SSOT 落在 `presets/schemas/ce-executor-serial.yml`（plan v3.1）

**现行（plan）**：
- authoring SSOT = `presets/schemas/ce-executor-serial.yml`（扩节，见 plan Merge 映射表）
- 运行时 = embedded preset → `RalphConfig` / `EventLoopConfig` → `ProtocolView`
- Rust 执行器 = `crates/ralph-core/src/preset/engine/`（preset 无关）

**历史草案（废弃）**：~~`ralph-proto/serial_protocol`~~。

### D-5. Linter 失败时硬阻塞（不 emit）

**理由**：
- Linter 设计的核心是"不落盘就不消耗迭代预算"
- 软反馈（emit 但 warning）会让 fail-after 仍然发生，软反馈 = 半个 linter
- 硬阻塞迫使 agent 重新 lint 直到通过，保证 emit 的事件 100% 通过 runtime gate

**trade-off**：
- Linter 引入额外延迟（单次 ~50-200ms）
- 极端情况：linter 误报 → agent 永远过不去 → 死循环
- 防御：linter 必须有 `--bypass-lint` escape hatch（operator 显式开启 + 审计日志）

### D-6. 保持现有 runtime gate 作为兜底

**理由**：
- d623c09 落地的 5 道 gate（U1/U2/U3/U4/U5/U6/U7/U8）保留
- 防御 loop_runner 内部 publish 等绕过路径
- Linter 通过 ≠ 100% 通过 runtime（schema 派生可能有 bug、agent payload 字段值异常等）

**trade-off**：
- 5 道 gate 的代码实现要重构成"linter 镜像 + runtime 兜底"双层
- 短期内代码量不减少（gate 实现 + linter 实现），但新增 linter 是单点收敛（不再分散）

### D-7. 编辑工作流改变

| 工作流 | 改前 | 改后 |
|---|---|---|
| 改 schema 字段 | preset YAML + agent 提示词 + runtime 代码 3 处 | **`presets/schemas/ce-executor-serial.yml` 1 处** + `cargo build` |
| 改 gate 规则 | preset YAML + runtime 代码 2 处 | schemas 文件 1 处 + engine 读 `ProtocolView` |
| 改 preset 业务参数（如 `max_iterations`） | preset YAML | preset YAML（同前） |
| 改 preset 结构定义（如新增 hat） | preset YAML + instructions | `protocol.rs` + preset YAML |

**trade-off**：
- operator 必须先 `cargo build` 才能让 preset 改动生效
- 需在 `ralph-tools-presets.md` 中显式标注："schema / gate 相关改动请改 `presets/schemas/ce-executor-serial.yml`"
- 工作流改变需培训 / 文档（1 份 `docs/handbook/serial-preset-development.md` 新增）

---

## Requirements

> **历史 SP-R1–SP-R18 条文**仍使用 `serial_protocol` 措辞；实施与验收以 plan v3.1 的 R1–R22 / KTD 为准（见文首对照表）。

### Phase 1 — 协议 SSOT 化

- **SP-R1**. 现有 `presets/en/ce-executor-serial.yml` 中的 `execution_contracts.rules.*.require_payload_fields` / `require_task` / `require_git_change` / `require_test_evidence` 必须从 `crates/ralph-proto/src/serial_protocol/contracts.rs` 自动派生。`cargo build` 时若 preset 与 SSOT 不一致则 panic。
- **SP-R2**. 现有 `presets/en/ce-executor-serial.yml` 中的 `verdict_gate` / `workflow_contract.step_handoff.progress_task_gate` / `verdict_gate.additional_topics` 必须从 `serial_protocol::gates.rs` 派生。同上，build-time 校验。
- **SP-R3**. 现有 `presets/en/ce-executor-serial.yml` 中的 `state_projection.actions.*.kind` / `current_step` / `completed_step` 必须是 `serial_protocol::projection::ActionKind` 枚举的子集。新增 action 必须先在 `serial_protocol` 定义才能在 preset 引用。
- **SP-R4**. `serial_protocol` SSOT 必须包含 `mark_step_completed` action（治 state_projection 不写 Completed Steps 根因）。当 `work.done` 被 emit 时，state_projector 必须**先** close_task **再** mark_step_completed（顺序硬约束，编译期保证）。
- **SP-R5**. Agent instructions（`presets/en/ce-executor-serial.yml` 中各 hat 的 instructions 段）涉及 schema / payload 字段 / gate 触发的部分必须从 `serial_protocol` 派生或通过 `serde_json` 引用。指令中"凭记忆手写"的 schema 段落必须删除，改为 `{generated_from: serial_protocol::<X>}` 形式的引用块。
- **SP-R6**. `ralph emit --schema <topic>` 子命令必须输出当前 `serial_protocol` SSOT 派生的 schema JSON，供 agent 在 precheck 阶段拉取。子命令输出必须与 `serial_protocol` build-time 一致（hash 比对断言）。
- **SP-R7**. 现有 2026-06-20-001 plan 中 U5b / U7 / U6 三个加固项**必须**走 `serial_protocol` 路径落地，不允许绕过 SSOT 直接改 runtime。

### Phase 2 — Precheck-as-Linter

- **SP-R8**. `ralph emit` 必须新增 `lint` 阶段（在 `policy_check` 之前），调用 `serial_protocol::linter::lint_emit(hat, topic, payload)`，镜像全部 5 道 runtime gate 的检查规则。lint 失败时 emit 命令返回非零退出码 + 详细 diff（指出哪个字段、哪个 gate 期望什么）。
- **SP-R9**. Linter 失败时**不落盘** `events.jsonl`、**不消耗迭代预算**、**不**写 `recovery.jsonl`。改为 lint 失败时直接输出 `## LINT FAILED` 块到 stdout，供 agent 看到后立即修正。
- **SP-R10**. `ralph emit --bypass-lint` flag 必须存在且必须**显式 opt-in**。每次 bypass 必须在 `recovery.jsonl` 写一条 `severity=warning, source=bypass_lint, hat=<hat>, topic=<topic>` 审计记录。该 flag 在 `policy_check.rs` 中必须独立判定，不被 linter 路径覆盖。
- **SP-R11**. Linter 必须能识别 `loop_runner/runner.rs` 与 `hard_gate.rs` 内部 publish 的事件（未来 d623c09 U5b 实现后可能调用 `lint_emit`），通过 `source=ralph` 或 `source=internal` 显式标记。Linter 对 internal source 跳过 schema 检查但保留 deny-rule 检查。
- **SP-R12**. Linter 必须在 `build_prompt` 阶段把当前 `serial_protocol` 派生的 schema 镜像成 `## LINT MIRROR` 块注入 agent prompt。块中包含：当前 hat 的 allowed topics 表（`publishes` + `triggers` 派生）、当前 hat 的 required payload fields（`require_payload_fields` 派生）、当前 step 的 gate state（`progress.md` 派生）。Agent 在 emit 之前看到这一块。
- **SP-R13**. ~~Linter 失败时自动 emit `task.resume`~~ **已修订（plan KTD-8）**：lint 失败写 `LoopState.pending_lint_resume`；下一帧 `build_prompt` 注入 `## LINT RESUME REQUIRED`；**不**写 recovery.jsonl、**不**走 `rejection.rs` 持久化 task.resume。
- **SP-R14**. Linter p95 < 200ms。**超时 fail-closed**（plan KTD-9）：输出 `## LINT TIMEOUT`、exit ≠ 0；**禁止** fallback 到 policy_check 放行。
- **SP-R15**. Linter 镜像的 5 道 gate 必须与 runtime gate 实现共用同一组 SSOT 派生函数（`serial_protocol::gates::check_X`），不允许 linter / runtime 各写一份。**唯一允许的差异**：linter 在 `build_prompt` 阶段跑（无状态），runtime 在事件循环跑（有状态）。差异通过 trait 抽象（`LintCheck` vs `RuntimeCheck`）隔离。

### 跨阶段约束

- **SP-R16**. 重构期间（Phase 1 + Phase 2 共 2 周）**只做机制重构**，不再出新诊断报告、不再加新 patch（除非 Phase 1 / Phase 2 验收失败需要 hotfix）。所有新发现的 bug 走 `docs/issues/2026-06-20-serial-refactor-known-issues.md` 集中记录，Phase 2 验收后批量修。
- **SP-R17**. `RALPH_SERIAL_LINT_MODE=off` **仅关 linter**；gate 仍走 `preset/engine`（**不是** d623c09 全量回滚）。
- **SP-R18**. `ce-executor-isolated.yml` 在本次改动中**不被修改**（按 D-3）。任何发现 isolated 也有同样 bug 的证据，必须另起 plan 处理，不并入本需求。

---

## Key Flows

### F1. 改 schema 字段的开发者工作流

- **Trigger:** 开发者要改 `work.done` 的 required payload fields
- **Actors:** preset 维护者、runtime 维护者、agent 维护者
- **Steps:**
  1. 改 `crates/ralph-proto/src/serial_protocol/contracts.rs` 加 1 个字段
  2. `cargo build`，SSOT 一致性自动校验
  3. preset YAML 自动派生新字段（无需手改）
  4. agent prompt 中 `## LINT MIRROR` 块自动更新（无需手改）
  5. runtime gate 自动使用新字段（无需手改）
- **Outcome:** 1 处改动，3 处自动更新
- **Covers:** SP-R1, SP-R2, SP-R3, SP-R4, SP-R5, SP-R15

### F2. agent 在 emit 之前的 linter 触发

- **Trigger:** agent 准备 emit `queue.advance`
- **Actors:** 当前 hat、linter、runtime gate
- **Steps:**
  1. agent 准备 payload `{next_step: "step-02", completed_step: "step-01"}`
  2. linter 镜像 progress_task_gate 检查：progress.md `## Current Step` 是否为 step-01、## Completed Steps 是否含 step-01
  3. linter 发现 progress.md 缺 `## Completed Steps`（state_projection 漏写）
  4. linter 输出 `## LINT FAILED: progress_task_gate.expected=## Completed Steps, found=(empty)`
  5. linter 自动 emit `task.resume(target=plan-gate, reason=lint_failed, expected_fix=progress.md:## Completed Steps:append step-01)`
  6. plan-gate 收到 task.resume，触发 state_projection 补写（因为 SP-R4 强制 mark_step_completed 在 close_task 之后）
  7. plan-gate 重新 emit `queue.advance`
  8. linter 这次通过 → emit 落盘 → runtime gate 兜底校验通过
- **Outcome:** progress.md 补全后，emit 成功；不消耗 dead iteration
- **Covers:** SP-R4, SP-R8, SP-R9, SP-R12, SP-R13

### F3. operator 紧急 bypass

- **Trigger:** linter 误报导致 loop 永远过不去
- **Actors:** operator、linter、审计日志
- **Steps:**
  1. operator 跑 `RALPH_SERIAL_LINT_MODE=off ralph run` 或 `ralph emit --bypass-lint`
  2. linter 跳过 schema 检查（仍保留 deny-rule 检查）
  3. 事件落盘 + recovery.jsonl 写 audit 记录
  4. 事后在 `docs/issues/2026-06-20-serial-refactor-known-issues.md` 记录误报
  5. Phase 2 验收后批量修 linter 误报
- **Outcome:** operator 有 hotfix 退路，loop 不被 linter bug 永久卡死
- **Covers:** SP-R10, SP-R17

### F4. isolated preset 隔离（不交叉污染）

- **Trigger:** 任何对 `serial_protocol` 的改动
- **Actors:** 编译系统、isolated preset
- **Steps:**
  1. 改动只发生在 `crates/ralph-proto/src/serial_protocol/` namespace
  2. `ce-executor-isolated.yml` 不引用 `serial_protocol`，仍使用 isolated 自己的 inline schema
  3. `cargo build` 通过
  4. isolated preset 的 run 不被影响
- **Outcome:** serial 改动不污染 isolated
- **Covers:** SP-R18, D-3

---

## Acceptance Examples

### AE-1. SSOT 派生验证

- **Given:** 开发者改 `serial_protocol/contracts.rs` 中 `work.done` 的 `require_payload_fields` 加 `new_field`
- **When:** `cargo build`
- **Then:**
  - 编译通过（SSOT 与 preset 一致）
  - `ralph emit --schema work.done` 输出含 `new_field`
  - `presets/en/ce-executor-serial.yml` 的 `execution_contracts.rules.work.done.require_payload_fields` 自动派生含 `new_field`
  - agent prompt 的 `## LINT MIRROR` 块含 `new_field`
- **Covers:** SP-R1, SP-R5, SP-R6

### AE-2. state_projection 顺序硬约束

- **Given:** state_projector 收到 `work.done` 事件
- **When:** 调用 `apply(work.done)`
- **Then:**
  - close_task 必须**先**执行
  - mark_step_completed 必须**后**执行
  - 顺序违反时编译期报错（用 typestate pattern 或 similar）
  - 运行期 progress.md 必须在 close_task 与 mark_step_completed 之间写入
- **Covers:** SP-R4

### AE-3. Linter 失败时不落盘

- **Given:** agent 跑 `ralph emit queue.advance --payload '{next_step: "step-02"}'`（缺 `completed_step`）
- **When:** lint 阶段跑
- **Then:**
  - lint 输出 `## LINT FAILED: progress_task_gate.expected=completed_step, found=(missing)`
  - events.jsonl **不增加**新行
  - recovery.jsonl **不增加**新行
  - exit code = 1
  - iteration 预算不消耗
- **Covers:** SP-R8, SP-R9

### AE-4. Linter 失败时 task.resume 注入

- **Given:** 上一步 AE-3 的 linter 失败
- **When:** linter 自动 task.resume 注入
- **Then:**
  - 注入 `task.resume(target=plan-gate, reason=lint_failed, expected_fix=...)`
  - task.resume 走原 d623c09 U4 schema（reason / target_hat 字段必填）
  - plan-gate 收到 task.resume 后能正确解析
- **Covers:** SP-R13

### AE-5. Bypass 审计

- **Given:** operator 跑 `ralph emit queue.advance --bypass-lint --payload '{...}'`
- **When:** emit 执行
- **Then:**
  - lint 跳过 schema 检查
  - deny-rule 检查**仍生效**（topic 不在白名单则拒）
  - events.jsonl 增加新行
  - recovery.jsonl 增加新行（severity=warning, source=bypass_lint）
  - 审计记录含 hat / topic / timestamp
- **Covers:** SP-R10

### AE-6. 同一 plan 跑完 LOOP_COMPLETE

- **Given:** `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`（3 UNIT × 12 U）
- **When:** `ralph run -H builtin:ce-executor-serial -p <plan>`
- **Then:**
  - 0 次 abort
  - iteration ≤ 50
  - 0 次 `consecutive_failures` 退出
  - 0 次 `consecutive_failures` 兜底（recovery.jsonl 无此 reason_code）
  - 走到 `LOOP_COMPLETE`（最后一个 UNIT 的 shipper → reporter → LOOP_COMPLETE）
- **Covers:** 成功标准（用户原话"跑同一个 plan 能到 LOOP_COMPLETE"）

### AE-7. isolated preset 不受影响

- **Given:** isolated preset 跑同样的 plan
- **When:** `cargo build` 后
- **Then:**
  - isolated 行为与 d623c09 后完全一致
  - `presets/en/ce-executor-isolated.yml` 文件**未被修改**
  - isolated run 不引用 `serial_protocol`
- **Covers:** SP-R18, D-3

---

## Success Criteria

- **SC-1.** 跑 `docs/plans/2026-06-20-001-feat-python-sort-algorithms-plan.md`（3 UNIT × 12 U）能完成全部 12 U，触发 `LOOP_COMPLETE`，无 abort / 无 `consecutive_failures`。
- **SC-2.** 改 `serial_protocol` 任意 schema 字段只需要改 1 处，preset / agent prompt / runtime 全部自动更新。
- **SC-3.** Linter lint 时间 < 200ms（p95），单 iteration 总开销 < 1s（p95）。
- **SC-4.** 30 天后同类 run（state_projection 漂移 / progress_task_gate 误拒 / task.resume 不响应 / 重复 emit）不再出现。即 2026-07-20 之前不再出新诊断报告。
- **SC-5.** `ce-executor-isolated.yml` 在本次重构期间不被修改。

---

## Scope Boundaries

### 本次覆盖（plan v3.1 为准）

见 [`docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md`](../plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md) Scope Boundaries。

~~`crates/ralph-proto/src/serial_protocol/`~~ **已废弃**。

### 本次不覆盖

- `ce-executor-isolated.yml` 任何修改（按 D-3，后续会从仓库移除）
- Telegram / TUI 的 `human.guidance` target 路由回归测试（用户已指示）
- 重写 9-hat 拓扑或新增 hat
- 完整重做 `recovery.jsonl` 格式（向后兼容，旧格式只读）
- 自动化从 git diff 生成 `## changed` 骨架
- 删 `ce-executor-isolated.yml`（按用户指示是后续动作）
- 删 `ce-executor-wave` preset（不在本范围）

### Deferred to Follow-Up Work

- `ralph audit hat-handoff` 集成到 `ralph diagnose` UI（plan Open Questions L102）
- `RecoveryEnvelope` 反序列化的运行时校验（`recovery.jsonl` 历史兼容性）
- `ralph-cli` 增加 `scenario-fault-injection` 子命令预防 6 类典型越权模式
- Linter 镜像的 5 道 gate 进一步覆盖到 `ce-executor-wave`（按用户指示 wave 不在本范围，但 isolated 被移除后 wave 可能成为唯一并行 preset）

---

## Dependencies / Assumptions

### Dependencies

- **D-DEP-1.** commit d623c09 已落地（当前 HEAD）—— 5 道 gate 基础已就位
- **D-DEP-2**. ~~`ralph-proto/serial_protocol`~~ → `presets/schemas/` + `preset/engine/`（plan v3.1）
- **D-DEP-3.** 现有 BDD scenarios harness `crates/ralph-core/tests/scenarios/` 可复用
- **D-DEP-4.** `crates/ralph-cli/src/commands/emit.rs` 当前已实现 `policy_check` 阶段，可在其前插入 `lint` 阶段

### Assumptions

- **D-ASSUM-1.** Linter 实现用 Rust 代码（不是 JSON Schema / external linter）—— 减少工具链
- **D-ASSUM-2.** Operator 接受 preset 编辑工作流改变（schema 改完需 `cargo build`）—— 已通过 D-7 显式记录
- **D-ASSUM-3.** R7 加固项（U5b/U7/U6）落地到 **plan v3.1** 的 `presets/schemas/` + `preset/engine/` 路径
- **D-ASSUM-4.** 用户接受重构期间 1-2 周中断 + 重构期间不再出新诊断报告（除非验收失败需要 hotfix）
- **D-ASSUM-5.** `ce-executor-isolated.yml` 后续会被删除（按用户原话）—— 不需要为它维护兼容

### Risks

- **D-RISK-1.** Phase 2 Linter 误报风险——linter 与 runtime gate 实现必须共用 SSOT 派生函数（SP-R15），否则漂移
- **D-RISK-2.** Phase 2 Linter 延迟 — 超时 **fail-closed**（plan KTD-9），不 fallback 放行
- **D-RISK-3.** 工作流改变风险——operator 改 preset 不 `cargo build` 会以为生效，需在 `ralph-tools-presets.md` 强提示
- **D-RISK-4.** ~~serial_protocol 命名冲突~~ → 已消除：SSOT 在 `presets/schemas/`，engine 在 `ralph-core/preset/engine/`

---

## Outstanding Questions

### Resolve Before Planning

- **D-OQ-1** ~~serial_protocol 命名~~ → **已决**：`preset/engine/` + `presets/schemas/`（plan v3.1）
- **D-OQ-2** ~~Linter 失败 task.resume~~ → **已决**：LintResumeHint（plan KTD-8）

### Deferred to Planning

见 plan v3.1 Open Questions / Implementation Units。

---

## Sources / Research

### 历史诊断报告（已读 6 份 + 本次 1 份）

- `docs/report/2026-06-16-loop-diagnostic-report.md`（652s stall 实证）
- `docs/report/2026-06-17-ce-executor-serial-merry-lotus-review-chain-stalled-diagnosis.md`（41m 后 cancelled）
- `docs/report/2026-06-17-ce-executor-serial-noble-peacock-review-chain-stalled-diagnosis.md`（28m 后 cancelled）
- `docs/report/2026-06-17-ce-executor-isolated-keen-fern-review-verdict-failed-diagnosis.md`（1h47m failed）
- `docs/report/2026-06-18-003-perky-maple-loop-link-diagnosis.md`（2h7m 用户 abort）
- `docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md`（用户 abort）
- `docs/report/2026-06-20-ce-executor-serial-primary-20260619-164313-loop-diagnosis.md`（本 run，2h25m consecutive_failures）

### 现有计划（4 份）

- `docs/plans/2026-06-18-001-fix-ce-executor-serial-recovery-handoff-plan.md`（d623c09 落地源 plan）
- `docs/plans/2026-06-18-004-fix-ce-executor-serial-perky-maple-orchestration-gaps-plan.md`（perky-maple 独立修复，worktree 7 commit）
- `docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md`（**现行实施 plan v3.1**）

### 现有 brainstorm（前置）

- `docs/brainstorms/2026-06-17-hat-orchestrator-state-projection-requirements.md`（state_projection 概念源头）
- `docs/brainstorms/2026-06-14-ce-executor-isolated-agent-output-governance-requirements.md`（GOV-R 系列）

### 关键源码位置（按主题）

| 主题 | 路径 |
|---|---|
| 当前 5 道 gate 实现 | `crates/ralph-core/src/event_loop/mod.rs:994, 1024, 3790-3820, 5643-5711, 7057-7097` |
| progress_task_gate | `crates/ralph-core/src/step_handoff/progress_task_gate.rs:32, 402-446, 431-440` |
| Loop runner（未来 U5b 落地点） | `crates/ralph-cli/src/loop_runner/runner.rs`, `hard_gate.rs` |
| 当前 emit 流程 | `crates/ralph-cli/src/commands/emit.rs:512-514, 721-747` |
| State projection actions | `presets/en/ce-executor-serial.yml:116-139` |
| verdict_gate / workflow_contract | `presets/en/ce-executor-serial.yml:149-160, 175-220` |
| 协议字段定义起点 | `crates/ralph-proto/src/` |

### 关键解决方案文档

- `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md`（task.resume 字段补齐）
- `docs/solutions/integration-issues/ce-executor-serial-fix-applied-rereview-dedup-2026-06-18.md`（dedup prune 修复）
- `docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md`（R1-R19 handoff 机制）
