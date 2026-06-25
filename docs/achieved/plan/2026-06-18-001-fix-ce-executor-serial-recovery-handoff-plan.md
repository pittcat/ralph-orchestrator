---
title: 修复 ce-executor-serial 运行死锁：recovery 信号、handoff 链路、越权写入三重机制加固
type: fix
status: active
date: 2026-06-18
origin:
  - docs/brainstorms/2026-06-18-isolated-hat-handoff-requirements.md
  - docs/brainstorms/2026-06-17-agent-recovery-mechanism-gaps-requirements.md
  - docs/brainstorms/2026-06-16-ce-executor-loop-stability-requirements.md
  - docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md
---

# 修复 ce-executor-serial 运行死锁：recovery 信号、handoff 链路、越权写入三重机制加固

## Overview

`ce-executor-serial` preset 在 `warm-tiger` run 中死锁：review 序列卡在 2/4 维、`handoff` 产物完全缺失、`ralph` pseudo-hat  recovery 信号成死信、`executor` 越权事件部分落盘。表面上是单点配置或 agent 行为问题，实则是 **CLI gate 早返 / EventBus 路由覆盖 / trigger 语义缺失 / backpressure 未闭环 / 可观测性缺口** 五层机制未咬合。

本计划从机制层面修复，不针对单个事件补丁：

1. **让 gate 真的在门口查**：CLI `hat_handoff` gate 在 `hat=None` 时仍检查 macro edge；runtime 对缺少 provenance 的非 CLI business topic fail-closed。
2. **让 recovery 信号真的被消费**：补齐关键 hat 的 `task.resume` trigger + instructions；修复 `human.guidance` target 被 EventBus 吞掉的路由 bug；让 progress-steward 能收到被 `suppress_human_guidance` 误伤的 guidance。
3. **让 backpressure 回到 agent prompt**：把 runtime 拒收摘要注入 `## ORCHESTRATOR CONTEXT` / `## ROBOT GUIDANCE`，消除同一类 payload 错误反复探测；CLI 拒收仍通过 `recovery.jsonl` + 文档教 agent 主动读取。
4. **让 handoff 可观测可审计**：注入块写 tracing + 产物审计脚本，CI 捕获"0 handoff 文件"类静默失败。

---

## Problem Frame

### 谁在受影响

使用 `ce-executor-serial` / `ce-executor-isolated` 跑固定工作流的 operator；尤其是依赖 `handoff.enabled: true` + isolated 模式的多 hat 流水线。

### 核心症状

| 症状 | 来源报告 | 直接后果 |
|------|---------|---------|
| `.ralph/agent/hat-handoff/` 完全不存在 | P0-A | 下游 hat 看不到 `## HAT HANDOFF` 导航块，交接链断裂 |
| `dimension-reviewer` 完成 c/t 后卡死，m/r pending | P0-B / P1-B | `review.dimensions.complete` 永不 emit，synthesizer / plan-gate 永不启动 |
| `ralph` 伪 hat 连续发 `task.resume` / `human.guidance` 无效 | P2-A / P1-D | loop stuck 24h+，progress-steward 不产出 recovery 事件 |
| `executor` 越权 `review.dimension.done` / `review.passed` 部分落盘 | P0-C / P1-C | 假自评污染状态，recovery.jsonl 有拒收但 agent 反复重试 |
| `progress.md` 卡在 Step 1 | P2-B | state_projection 的 `queue.advance` 触发条件被上游死锁永久阻塞 |

### 一句话根因

不是缺功能，是 **gate 有洞、信号有阻、反馈有断、观测有盲** 四件事叠加，让已有机制无法自愈。

---

## Requirements Trace

### 来自 `docs/brainstorms/2026-06-18-isolated-hat-handoff-requirements.md`

- **R1.** `event_loop.hat_handoff.enabled` 默认关闭；开启后仅对配置的宏观边强制 handoff。
- **R2.** 宏观边 = `workflow_contract` 唯一消费者 topic；微观边默认豁免。
- **R4.** 宏观边 emit payload 必须含 `handoff_path`；缺字段或文件不存在 → 结构硬门拒收 + `task.resume` 提示。
- **R5 / R6.** 下游 `build_prompt` 注入 `## HAT HANDOFF` 块，标明 from/to/handoff_path。
- **R16.** 硬门校验 `## next` 必须含 `**动作**:` 与 `**阻塞**:` 行。

### 来自 `docs/brainstorms/2026-06-17-agent-recovery-mechanism-gaps-requirements.md`

- **R-A1.** CLI precheck 与 loop gate 对齐，business topic 在写盘前得到与 runtime 一致的拒收原因。
- **R-A2.** 预检失败输出结构化 `reason_code`。
- **R-B1.** `ralph diagnose` 文档覆盖 `handoff_dispatch_timeout` / `progress_task_mismatch` 排查。

### 来自 `docs/brainstorms/2026-06-16-ce-executor-loop-stability-requirements.md`

- **R-B1 / R-B3.** 可恢复违规类（`MissingRequiredField`、`PayloadTypeMismatch`、`TopicDenied`）应注入 `task.resume` + fix_hint，**不得** 让 runner 立即 `not_retriable` 终止。
- **R-B4.** 重试上限 3 次；第 4 次明确终止。
- **R-D1 / R-D2.** worktree 诊断须写入 loop 实际 workspace；`ralph diagnose` 能定位非空 session。

### 来自诊断报告

- **R-REP1.** 修复 `dimension-reviewer` / `executor` / `review-coordinator` / `plan-gate` 对 `task.resume` 的响应能力；`progress-steward` 通过 `human.guidance(target=steward)` 被唤醒（与现有 preset 设计一致，不额外占用 `task.resume`）。
- **R-REP2.** 修复 `human.guidance(target=progress-steward)` 被 EventBus `human.*` 前缀路由吞掉的 bug。
- **R-REP3.** 消除 CLI `hat=None` 时 `hat_handoff` gate 早返。
- **R-REP4.** 提供 handoff 产物审计，捕获"0 文件"静默失败。
- **R-REP5.** 把 recovery 拒收反馈回注到 agent prompt，打破 payload 错误死循环。

---

## Scope Boundaries

- **本次覆盖**：
  - `ce-executor-serial` 与 `ce-executor-isolated` 的 recovery / handoff 机制修复。
  - CLI emit gate、`event_origin` guard、`EventBus` 路由、`build_prompt` 注入、preset triggers/instructions。
  - `recovery.jsonl` 到 prompt 的 backpressure 闭环。
  - handoff 产物审计脚本与 CI lint。
  - 相关 BDD / 单元测试补充。

- **本次不覆盖**：
  - 重写 9-hat 拓扑或新增 hat。
  - 修改 operator 启动命令或 `PROMPT.md` 格式。
  - 内核级禁止 `echo >> events.jsonl`（仍靠 precheck + loop gate + backpressure 闭环）。
  - coordinator 模式下的 hat handoff（非 isolated）。
  - 从 `git diff` 自动生成 `## changed` 骨架。
  - 全量 skill 文档重写（仅更新与本次变更直接相关的 `ralph-tools-handoff.md` 行号/命令）。

### Deferred to Follow-Up Work

- **自动从 `git diff` 生成 `## changed` 骨架**：需要研究 agent 可写范围与 diff 语义，独立 PR。
- **`handoff_path` 历史索引 UI / `ralph diagnose` 专节展示**：本计划只补排查文档，UI 后续跟进。
- **HTML 输出模式**：与本计划无关。

---

## Context & Research

### Relevant Code and Patterns

- **CLI emit 入口与 provenance 检查**：`crates/ralph-cli/src/commands/emit.rs:433-478`（hat 缺失 / ralph 业务 topic 拦截）。
- **CLI `hat_handoff` gate**：`crates/ralph-cli/src/policy_check.rs:479-579`，当前 `hat=None` 时早返（`let Some(hat_id) = hat else { return Ok(()) };`）。
- **runtime hat_handoff gate**：`crates/ralph-core/src/event_loop/mod.rs:7901-8040`，fail-closed 但事件未到达。
- **runtime scope enforcement（no-hat fallback）**：`crates/ralph-core/src/event_loop/mod.rs:6970-6974`，agent backend 输出无 hat 时会 fallback 到 `current_isolated_hat`。
- **EventBus target 路由**：`crates/ralph-proto/src/event_bus.rs:111-128`，`human.*` 前缀拦截会覆盖 `target` 字段。
- **task.resume payload 构造**：`crates/ralph-core/src/event_loop/rejection.rs:403-502`，已补齐 `reason` / `target_hat`。
- **publish_policy_rejection_resume**：`crates/ralph-core/src/event_loop/mod.rs:393-491`。
- **stall detector / progress-steward**：`crates/ralph-core/src/event_loop/mod.rs:9761-9905`。
- **suppress_human_guidance 路径**：`crates/ralph-core/src/event_loop/mod.rs:4730-4795`（update）、`4937-4977`（apply）。
- **build_prompt isolated 路径**：`crates/ralph-core/src/event_loop/mod.rs:4422-4599`，含 handoff 注入与事件过滤。
- **handoff 注入块构造**：`crates/ralph-core/src/hat_handoff/emit_instructions.rs:25-93`、`crates/ralph-core/src/hat_handoff/inject.rs:20-48`。
- **macro edge 判定**：`crates/ralph-core/src/hat_handoff/macro_edges.rs:30-71`。
- **serial preset 关键 hat 定义**：`presets/en/ce-executor-serial.yml:1140-1180`（dimension-reviewer）、`2155-2190`（progress-steward）。
- **isolated preset steward 定义**：`presets/en/ce-executor-isolated.yml:2328-2405`（progress-steward）。
- **serial preset 已存在的 ralph topic_deny_rule**：`presets/en/ce-executor-serial.yml:288` 已锁定 `ralph → work.ready`，无需新增；P1-A 中 L23 落盘属于非 CLI bypass。

### Institutional Learnings

- `docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md`：本次 run 的完整证据链；指出反复出现的历史模式（merry-lotus / noble-peacock / perky-maple）。
- `docs/achieved/plan/2026-06-18-005-fix-isolated-hat-handoff-review-findings-plan.md`：曾修复 CLI seq 误杀 + 缺 `task.resume`，但未覆盖 `hat=None` 早返与 agent 侧 trigger 语义。
- `docs/report/2026-06-18-003-base-stability-implementation-paths.md`：基座稳定性 4 块拼图（schema 同源、payload 恢复、bootstrap 隔离、worktree 诊断）。

### External References

- 无外部依赖；全部基于本地代码模式与已合并的同类修复。

---

## Key Technical Decisions

1. **CLI `hat_handoff` gate 对 `hat=None` 不再早返**：改为对 macro-edge business topic 返回 `hat_handoff_missing_path` 或 `isolated_scope_violation`。这样即使 agent 漏传 `--hat`，宏观边也不会无提示落盘。
   - *理由*：当前 `hat=None` 早返是 P0-A handoff 失效的最直接代码漏洞；与 `require_emit_provenance` 的 fail-closed 原则一致。

2. **EventBus `human.*` 前缀拦截改为先检查 `target`**：若 `human.guidance` 等 `human.*` 事件带显式 `target`，优先按 target 路由；无 target 再归入 `human_pending`。
   - *理由*：`ralph` 需要能定向唤醒 `progress-steward`；当前逻辑把带 target 的恢复信号也吞掉，是 P1-D 的根因之一。

3. **关键 hat 的 `triggers` 显式加入 `task.resume`，instructions 增加 recovery 语义**：覆盖 `dimension-reviewer`、`executor`、`review-coordinator`、`plan-gate`；`progress-steward` 通过 `human.guidance(target=steward)` 被唤醒（不占用 `task.resume`，与现有 preset 设计一致）。
   - *理由*：`task.resume` 已带 `target` 可直接路由，但 agent 不知道这是激活信号；补齐 triggers + instructions 才能让 recovery 信号真正被消费。progress-steward 现有设计已把 `task.resume` 保留给 ralph pseudo-hat，避免竞态。

4. **`recovery.jsonl` 拒收摘要通过 `## ORCHESTRATOR CONTEXT` / 运行时诊断 prompt 回注**：不是简单 tail 全文，而是按 `(reason_code, count, last_payload_hint)` 聚合后注入当前 hat prompt。
   - *理由*：P1-C 中同一类 payload 错误反复 5 轮，说明 agent 看不到 CLI 拒收；闭环反馈是打破死循环的唯一办法。

5. **`suppress_human_guidance` 对 progress-steward 自动豁免**：当事件 target=progress-steward 或 hat=progress-steward 时，`suppress_human_guidance` 不屏蔽 `human.guidance` 内容。
   - *理由*：serial preset 开启 `suppress_human_guidance` 是为防止 executor 探测风暴，但误伤了依赖 `human.guidance` 的 steward。

6. **runtime 对缺少 `source`/`triggered`/`hat` 的 isolated business topic fail-closed**：在 `process_parse_result` 的 isolated scope enforcement 阶段，对无法追溯到合法 provenance 的 business topic 拒绝，并给出明确 reason_code。
   - *理由*：P0-C 中部分越权事件落盘可能通过直接文件 append 或 loop-runner 内部 publish 绕过 CLI； origin guard 不能全局 reject 无 hat 事件（会破坏 agent backend 输出路径），所以改在 scope enforcement 阶段用 `source`/`triggered` 做区分。

7. **handoff 产物审计优先复用 Rust 解析**：不依赖 bash 解析 YAML，而是把审计逻辑加入 `ralph diagnose` 或 `ralph preset check --strict` 子命令，扫描 `.ralph/agent/hat-handoff/`。
   - *理由*：bash 解析 YAML 不可靠；项目已有 `RalphConfig` 解析器，复用可降低脆弱性。仅当审计脚本为纯文件扫描时保留 bash 包装。

---

## Open Questions

### Resolved During Planning

- **Q1: 是否把所有 business topic 都纳入 `hat=None` 的 fail-closed？**  
  **A:** 仅在 `execution_mode == isolated` 时启用；coordinator 模式保持兼容，避免破坏旧工作流。

- **Q2: `task.resume` 加入 triggers 后，是否会导致 hat 被非预期激活？**  
  **A:** `task.resume` 已有 `target` 字段；只有被 target 的 hat 才会进入 pending 队列。加入 triggers 只是让 agent 在 prompt 语义上识别该信号，不改变路由范围。

- **Q3: `recovery.jsonl` 注入是否会导致 prompt 过长？**  
  **A:** 按 reason_code 聚合，只注入最近 N 条（默认 5 条）+ 每类 1 条示例，不超过 1KB。

### Deferred to Implementation

- **D1: `recovery.jsonl` 聚合格式最终字段名**：实现时与 `state_projection` / `diagnosis envelope` 命名对齐。
- **D2: `progress-steward` instructions 中 recovery 决策树的具体措辞**：需实现后与 preset 维护者 review。
- **D3: handoff 审计脚本对 wave 模式 macro edge 的豁免细节**：实现时读取 `HatHandoffConfig.exempt_topics` 与 `DEFAULT_EXEMPT_TOPICS`。

---

## Output Structure

无需新建目录；本计划只修改现有文件并新增一个脚本与一个文档。预期变更：

```text
crates/ralph-cli/src/policy_check.rs              # U1: hat=None 仍查 hat_handoff gate
crates/ralph-cli/src/commands/emit.rs             # U1/U6: CLI 拒收 reason_code 结构化
crates/ralph-proto/src/event_bus.rs               # U2: human.* target 路由修复
crates/ralph-core/src/event_loop/mod.rs           # U4/U5/U5b/U6/U7: scope enforcement、注入可观测、recovery 回注、suppress 豁免
crates/ralph-core/src/event_loop/loop_state.rs    # U6: recent_rejection_digest 字段
crates/ralph-core/src/hat_handoff/emit_instructions.rs  # U4: 注入块加显式执行契约
crates/ralph-core/src/hat_handoff/inject.rs       # U4: 注入结果 tracing/persist
# 注：instructions.rs 通常不需要改动；recovery 语义优先通过 preset YAML 或运行时注入实现
crates/ralph-core/src/config/loop_config.rs       # U7: ProgressStewardConfig 新增字段
presets/en/ce-executor-serial.yml                 # U3/U7: triggers + instructions + steward 配置
crates/ralph-cli/src/presets.rs                   # U3: 同步 builtin preset 内容
presets/manifest.yml                              # U3: 同步 builtin preset 内容
presets/index.json                                # U3: 同步用户可见 preset 内容
scripts/ralph-zsh-plugin.zsh                      # U3: 本次 preset 集合不变，通常无需同步
crates/ralph-cli/src/commands/audit_hat_handoff.rs # U8: 新增审计子命令（Rust 实现）
crates/ralph-cli/src/commands/mod.rs              # U8: 暴露审计子命令
scripts/audit-hat-handoff-artifacts.sh            # U8: bash 薄包装
scripts/ci-rust-gate.sh 或 .github/workflows/*    # U8: CI 集成审计
crates/ralph-core/data/ralph-tools-handoff.md     # U9: 文档同步行号/命令
docs/guide/runtime-diagnosis.md                   # U9: 增加 handoff stall / progress 漂移 / recovery 排查
docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md  # U9: 新建排查 runbook
AGENTS.md / CLAUDE.md                             # U9: 仅当正文描述变化时同步
```

---

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

### 修复后的事件写入路径

```text
agent / ralph pseudo-hat
    │
    ├─► ralph emit ──► CLI provenance check ──► CLI policy check ──► CLI hat_handoff gate ──► events.jsonl
    │                     (hat=None fail-closed)     (topic_deny)        (macro edge)
    │
    └─► 直接写文件 ──► runtime scope enforcement ──► state machine ──► runtime hat_handoff gate
                      (anonymous business topic reject)                (fail-closed)
```

### recovery 信号闭环

```text
missing-event gate / policy rejection
    │
    ▼
task.resume(target=<stuck_hat>)  ──► EventBus target 路由 ──► stuck_hat pending
    │
    ▼
build_prompt: triggers 含 task.resume ──► instructions 教 agent 如何响应
    │
    ▼
agent emit 修复后事件 ──► CLI/runtime gate ──► 接受 / 拒收(task.resume retry)
```

### backpressure 闭环

```text
runtime 拒收（origin/policy/hat_handoff gate）
    │
    ▼
state.recent_rejection_digest
    │
    ▼
build_prompt 注入 ## RECENT REJECTIONS 或并入 ## ORCHESTRATOR CONTEXT
    │
    ▼
agent prompt 看到"你最近 3 次因 isolated_scope_violation 被拒..."

CLI 拒收
    │
    ▼
recovery.jsonl
    │
    ▼
agent instructions / runtime-diagnosis.md 教 agent 读取
```

---

## Implementation Units

- [ ] U1. **CLI `hat_handoff` gate 在 `hat=None` 时仍检查宏观边**

**Goal:** 消除 `hat=None` 早返漏洞，确保宏观边 emit 不携带 `handoff_path` 时在 CLI 层即被拒收。

**Requirements:** R4（来自 handoff requirements）、R-REP3、R-A1（来自 agent recovery requirements）。

**Dependencies:** 无。

**Files:**
- Modify: `crates/ralph-cli/src/policy_check.rs`
- Test: `crates/ralph-cli/src/policy_check.rs` 现有 `hat_handoff_tests` 模块

**Approach:**
- 在 `check_hat_handoff_gate_with_env` 中删除 `let Some(hat_id) = hat else { return Ok(()) };` 早返。
- 对 `hat=None` 场景：
  - 不调用 `requires_handoff`（因为它需要 `from_hat` 做自环排除），而是直接用 `HandoffIndex::consumer_of(topic)` 判断「该 topic 是否有唯一下游消费者」。
  - 若存在唯一消费者（即 macro edge），且无合法 `handoff_path`，返回 `ValidationError { reason_code: "hat_handoff_missing_path", ... }`。
  - 若不存在唯一消费者（多消费者 / wildcard / 未注册 / 自环豁免），保持 `Ok(())`。
- `hat=None` 时的 seq/iter 校验始终 `skip_seq_check=true`（因为无法确定 from_hat）。
- 保持 env 不存在时的 `skip_seq_check=true` 降级逻辑。

**Patterns to follow:**
- 复用 `ralph_core::hat_handoff::gate::evaluate_event` 与 `HandoffIndex::consumer_of`。
- 错误消息格式与 runtime gate 一致（`crates/ralph-core/src/hat_handoff/gate.rs:124-136`）。

**Test scenarios:**
- **Happy path:** `hat=None` + 非宏观边 topic（如 `review.dimension.ready`） → 通过。
- **Error path:** `hat=None` + 宏观边 topic（如 `work.ready`）+ 无 `handoff_path` → 返回 `hat_handoff_missing_path`。
- **Error path:** `hat=None` + 宏观边 topic + 非法 `handoff_path` → 返回对应 reason_code（`path_escape` / `filename_mismatch` / `file_not_found`）。
- **Edge case:** `hat=None` + 自环 topic（如 `queue.advance` 若 plan-gate 自消费）→ 通过，因为 `consumer_of` 返回自身或不在 index 中。
- **Edge case:** env 缺失时，`hat=None` 只检查 path jail / R15 / 结构，不校验 seq/iter。
- **Edge case:** 环境变量 `RALPH_LOOP_ITERATION` / `RALPH_HAT_HANDOFF_SEQ` 存在时，按真实值校验 seq/iter；这是 2026-06-18-005 的 regression 测试，本次不得破坏。
- **Integration:** macro edge 带合法 `handoff_path` 且文件存在 → CLI 通过；验证 env seq/iter 与文件名一致。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- hat_handoff` 通过。
- 手动 `ralph emit work.ready "{}"`（不带 `--hat`）在 `ce-executor-serial` worktree 中非零退出并打印 `hat_handoff_missing_path`。
- 在 loop 子进程中手动 `ralph emit work.ready --hat coordinator ...` 验证 env seq/iter 校验生效。

---

- [ ] U2. **修复 EventBus 对 `human.*` 的 target 路由覆盖**

**Goal:** 让 `human.guidance(target=progress-steward)` 等带显式 target 的 `human.*` 事件优先路由到 target hat，而不是被无条件收入 `human_pending`。

**Requirements:** R-REP2。

**Dependencies:** 无。

**Files:**
- Modify: `crates/ralph-proto/src/event_bus.rs`
- Test: `crates/ralph-proto/src/event_bus.rs` 现有 `test_direct_target` 等测试

**Approach:**
- 调整 `EventBus::publish` 中 `human.*` 前缀拦截与 `target` 路由的顺序：先检查 `event.target`；若有 target 且 target 在 registry 中，按 target 路由；否则再按 `topic.starts_with("human.")` 收入 `human_pending`。
- 保持无 target 的 `human.guidance` 仍走 `human_pending` 原路径，避免破坏现有行为。

**Patterns to follow:**
- `event_bus.rs:118-128` 的 direct target 逻辑。

**Test scenarios:**
- **Happy path:** `human.guidance(target=progress-steward)` → `progress-steward` 进入 pending，recipients 含 `progress-steward`。
- **Happy path:** `human.guidance` 无 target → 进入 `human_pending`，recipients 为空（保持原行为）。
- **Edge case:** target 指向未注册 hat → 与现有 direct target 行为一致（空 recipients）。
- **Integration:** 在含 `progress-steward` 的 registry 中 publish 带 target 的 `human.guidance`，验证 `take_pending(steward_id)` 非空。

**Verification:**
- `cargo nextest run -p ralph-proto` 通过。
- BDD / 集成测试中 `human.guidance(target=progress-steward)` 能唤醒 steward。

---

- [ ] U3. **补齐关键 hat 的 `task.resume` trigger 与 recovery instructions**

**Goal:** 让 `task.resume` 对关键 hat 既是路由目标，也是语义上可识别的恢复信号；progress-steward 通过 `human.guidance(target=steward)` 被唤醒，不占用 `task.resume`（与现有 preset 设计保持一致）。

**Requirements:** R-REP1、R-B1 / R-B3（来自 loop stability requirements）。

**Dependencies:** U2（`human.guidance` target 路由修复后，steward 才能被有效唤醒）。

**Files:**
- Modify: `presets/en/ce-executor-serial.yml`
- Modify: `presets/en/ce-executor-isolated.yml`（仅 progress-steward instructions，不添加 `task.resume` trigger）
- Modify: `crates/ralph-cli/src/presets.rs`（`PRESETS` 数组同步，仅内容）
- Modify: `presets/manifest.yml`（仅内容 hash/checksum 或说明）
- Modify: `presets/index.json`（如对用户可见，仅内容）
- Modify: `scripts/ralph-zsh-plugin.zsh`（`builtin:*` 补全同步，仅当 preset 集合变化时；本次不变化）
- 注意：本次不新增/删除/重命名 builtin preset，AGENTS.md / CLAUDE.md 中的 preset 列表通常无需同步。

**Approach:**
- 在 `dimension-reviewer` 的 `triggers` 增加 `"task.resume"`。
- 在该 hat instructions 顶部增加 `### Recovery Signals` 小节：
  - 收到 `task.resume(reason=..., target_hat=dimension-reviewer)` 时，说明上一轮未 emit terminal；必须重新检查当前维度状态并 emit `review.dimension.done` 或 `review.dimension.failed`。
  - 不得因为 `task.resume` 而切换维度或 emit 非本 hat 拥有的 topic。
- 对 `executor`、`review-coordinator`、`plan-gate` 做同样处理（视各自 publishes 调整措辞）。
- 对 `progress-steward`：
  - **不** 在 `triggers` 中添加 `"task.resume"`，以尊重 `ce-executor-isolated.yml:2335-2344` 中「`task.resume` 保留给 ralph pseudo-hat」的设计。
  - 在 instructions 中增加 `human.guidance` 处理小节：收到 `human.guidance(target=progress-steward)` 时，读取 state 并 emit 一个 recovery 事件（`work.ready` / `queue.advance` / `plan.blocked` 决策树）。
- 同步 builtin preset 的内容事实源（YAML / `presets.rs` / `manifest.yml` / `index.json`）。

**Patterns to follow:**
- 遵循 `presets/en/ce-executor-isolated.yml:2345` 的 steward instructions 风格。
- 触发器变更后必须跑 `ralph preset check --strict -H builtin:ce-executor-serial`。

**Test scenarios:**
- **Happy path:** `ce-executor-serial` preset lint 通过（`ralph preset check --strict`）。
- **Happy path:** `dimension-reviewer` 的 `triggers` 包含 `task.resume` 且 `publishes` 未变；`progress-steward` 的 `triggers` 仍不含 `task.resume`。
- **Integration:** BDD scenario 模拟 `task.resume(target=dimension-reviewer)` 后 reviewer 被选中并 emit `review.dimension.done`。
- **Integration:** 模拟 `human.guidance(target=progress-steward)` 后 steward emit `plan.blocked` 或 `queue.advance`。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- preset` 通过。
- `./scripts/run-tests.sh` 中 preset 相关测试绿。

---

- [ ] U4. **增强 hat_handoff 注入可观测性**

**Goal:** 让 operator 和 agent 都能确认 "本 turn 是否真的把 handoff 块注入到了 prompt"。

**Requirements:** R5 / R6（来自 handoff requirements）、R-B1（来自 agent recovery requirements）。

**Dependencies:** U1（CLI gate 修复后，agent 才会真正去 prepare handoff）。

**Files:**
- Modify: `crates/ralph-core/src/hat_handoff/emit_instructions.rs`
- Modify: `crates/ralph-core/src/hat_handoff/inject.rs`
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`build_prompt` isolated 路径）
- Test: `crates/ralph-core/src/hat_handoff/` 各模块测试

**Approach:**
- 在 `build_emit_instructions` 返回的块首/尾增加显式提示：
  - "以下 topic 是宏观边，emit 前必须运行 `ralph tools handoff prepare ...` 并把返回的 `handoff_path` 写入 payload"。
- 在 `prepend_hat_handoff_from_pending` 中记录：
  - `tracing::info!`：本次为 hat X 注入了 handoff 块（含 handoff_path 列表）。
  - 将 `last_injected_hat_handoff_block_hash` 写入 `LoopState`（或 diagnostics），供 `ralph diagnose` 读取。
- 若注入块为空（文件缺失），emit `diagnostic.hat_handoff.inject_failed` 到 orchestration diagnostics。

**Patterns to follow:**
- 现有 `diagnostic.hat_handoff.rejected` 事件模式（`event_loop/mod.rs:7979-7994`）。
- `LoopState` 中已有 `hat_handoff_seq`，可扩展字段。

**Test scenarios:**
- **Happy path:** macro edge pending 事件含合法 `handoff_path` → prompt 中出现 `## HAT HANDOFF` 块 + tracing 记录。
- **Error path:** macro edge pending 事件含缺失 `handoff_path` → 不注入块 + 发出 `diagnostic.hat_handoff.inject_failed`。
- **Error path:** handoff 文件已生成，但下游 hat 的 pending 事件 payload 没带 `handoff_path` → `build_emit_instructions` 块仍出现并提示"必须带 handoff_path"，同时 `inject_failed` diagnostic 记录该不一致。
- **Edge case:** 注入块超过 `max_bytes` → `truncate_preserving_next` 保留完整 `## next`。

**Verification:**
- `cargo nextest run -p ralph-core -- hat_handoff` 通过。
- 运行一个最小 isolated preset 后，`ralph diagnose` 能看到 `diagnostic.hat_handoff.*` 事件。

---

- [ ] U5. **runtime 对缺少 provenance 的 isolated business topic fail-closed**

**Goal:** 防止通过直接文件 append 或 loop-runner 内部 publish 等非 CLI 路径写入的非法 business topic 进入 state projection。

**Requirements:** R-A1（来自 agent recovery requirements）、R-REP3、R-B1 / R-B3（来自 loop stability requirements）。

**Dependencies:** 无（可与 U1 并行）。

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`process_parse_result` isolated scope enforcement 分支）
- Modify: `crates/ralph-core/src/event_loop/rejection.rs`（新增/复用 reason_class）
- Test: `crates/ralph-core/src/event_loop/` 相关测试 + BDD scenarios

**Approach:**
- 在 `process_parse_result` 的 isolated 分支中，当 scope enforcement fallback 到 `current_isolated_hat` 之前，先检查事件是否携带可追溯 provenance：
  - 有 `hat` 字段；或
  - 有 `source` 字段且 source 是注册 hat；或
  - 有 `triggered` 字段且 triggered 是注册 hat；或
  - topic 属于 `RALPH_CONTROL_TOPICS` / orchestrator diagnostic topics。
- 若都不满足，则视为"匿名 business topic"，返回 `OriginCheck::Rejected` 或等效拒绝，reason 为 `isolated_anonymous_business_topic`。
- 该拒绝触发 `task.resume(target=ralph)` 或 escalation，使 ralph pseudo-hat 有机会报告异常。
- **关键原则**：不改变 `event_origin.rs` 对 agent backend 输出（无 hat 但会 fallback 到 `current_isolated_hat`）的放行行为；只拦截"连 source/triggered 都没有"的异常事件。

**Patterns to follow:**
- 复用 `event_origin.rs` 的 topic 分类（control / diagnostic / business）。
- 复用 `rejection_from_origin` 与 `publish_policy_rejection_resume` 的 recovery 路径。

**Test scenarios:**
- **Error path:** isolated 模式下 JSONL 中读入无 `hat`/`source`/`triggered` 的 `work.ready` → 在 scope enforcement 前拒绝，reason 为 `isolated_anonymous_business_topic`。
- **Happy path:** isolated 模式下 agent backend 输出无 hat 的 `work.ready`（但当前 `current_isolated_hat` 存在）→ 按现有 fallback 路径放行并做 scope enforcement。
- **Happy path:** isolated 模式下无 provenance 的 `human.guidance` / `task.resume` / `loop.cancel` → 放行。
- **Happy path:** coordinator 模式下无 provenance 的 `work.ready` → 保持现有放行行为。
- **Integration:** 该拒绝注入 `task.resume(target=ralph)`，ralph 被激活后 emit `human.guidance` 或 `plan.blocked`。

**Verification:**
- `cargo nextest run -p ralph-core -- event_loop` 通过。
- BDD scenario 中模拟直接 append 到 `events.jsonl` 的匿名 business topic，验证其被拒绝且不进入 state projection。

---

- [ ] U5b. **封堵 loop-runner 内部 publish 与文件 append 的 provenance 校验缺口**

**Goal:** 与 U1（CLI gate）和 U5（runtime scope enforcement）形成双门，覆盖"不经过 `ralph emit`"的写入路径。

**Requirements:** R-REP3、R-A1（来自 agent recovery requirements）。

**Dependencies:** U5。

**Files:**
- Modify: `crates/ralph-cli/src/loop_runner/runner.rs`（任何直接写 `events.jsonl` 或内部 `bus.publish` business topic 的代码）
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（JSONL 读取入口的 provenance 日志）
- Test: 新增 BDD / 集成测试

**Approach:**
- 审计 `loop_runner/runner.rs` 中所有直接 append 到 `events.jsonl` 或内部 `bus.publish` business topic 的调用点。
- 对必须内部产生的事件（如 `loop.start`、`loop.cancel`、runtime 注入的 `task.resume`、`human.guidance`），确保它们：
  - 要么带 `source: "ralph"` / `hat: "ralph"`；
  - 要么 topic 属于 control / diagnostic。
- 对 business topic（如 `work.ready`、`review.dimension.done`），loop runner 内部 **不得** 直接 publish 或 append；若必须恢复，应通过 `ralph` pseudo-hat 的合法路径。
- 在 `process_events_from_jsonl` 入口增加 `tracing::warn!`：当发现无 provenance 的 business topic 时，记录事件 topic、ts、是否有 hat/source/triggered，便于事后审计。
- 特别关注 `ralph` pseudo-hat 直接 emit business topic 的路径：虽然 `emit.rs:456-478` 已拦截，但若通过文件 append 或内部 bus.publish 出现 `hat=ralph` + business topic，同样应由 U5 的 scope enforcement 拒绝（因为 `ralph` 只允许 control topics）。

**Patterns to follow:**
- 与 `event_origin.rs` 的 topic 分类保持一致。
- 参考 `crates/ralph-cli/src/loop_runner/hard_gate.rs` 中直接写文件的事件，确保它们只写 control / diagnostic / task.resume。

**Test scenarios:**
- **Happy path:** loop runner 内部 emit 的 `task.resume(target=dimension-reviewer)` 带 `source=ralph` → 通过 origin guard。
- **Error path:** loop runner 内部直接 append 一条 `review.dimension.done`（无 provenance）→ U5 的 runtime 校验拒绝。
- **Integration:** BDD scenario 验证：模拟外部脚本 `echo >> events.jsonl` 写入匿名 business topic，loop 不推进并产生 `isolated_anonymous_business_topic` diagnostic。

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- loop_runner` 通过。
- 手动用 shell 向 worktree 的 `events.jsonl` append 匿名 `work.ready`，验证 loop 拒绝并记录 diagnostic。

---

- [ ] U6. **把 runtime 拒收摘要注入 agent prompt，CLI 拒收通过 `recovery.jsonl` + 文档闭环**

**Goal:** 让 agent 在 prompt 里看到最近 **runtime** 拒收原因，停止重复非法 emit；CLI 拒收继续写入 `recovery.jsonl`，并通过文档/ instructions 教 agent 主动读取。

**Requirements:** R-A2（来自 agent recovery requirements）、R-REP5、R-B1 / R-B3（来自 loop stability requirements）。

**Dependencies:** 无（可与 U1/U5 并行）。

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（在 origin / policy / hat_handoff gate 拒收时更新 `state.recent_rejection_digest`）
- Modify: `crates/ralph-core/src/event_loop/loop_state.rs`（新增 digest 字段，带 `#[serde(default)]`）
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`apply_runtime_diagnosis_prompt` 或新增 helper）
- Modify: `crates/ralph-cli/src/commands/emit.rs`（确保 CLI 拒收 reason_code 结构化）
- Modify: `docs/guide/runtime-diagnosis.md`（教 agent 读取 `recovery.jsonl`）
- Test: `crates/ralph-core/src/diagnosis/` 与 `crates/ralph-cli/tests/` 集成测试

**Approach:**
- **Runtime 拒收摘要（内存闭环）：**
  - 在 `process_parse_result` 的 origin guard、policy check、hat_handoff gate 拒收分支中，更新 `self.state.recent_rejection_digest`。
  - digest 结构：`BTreeMap<reason_code, RejectionDigestEntry { count, last_message, last_ts }>`，只保留最近 5 个不同 reason_code。
  - 在 `build_prompt` 的 coordinator 与 isolated 路径中，把 digest 以 `## RECENT REJECTIONS` 块（或并入 `## ORCHESTRATOR CONTEXT`）注入当前 hat prompt：
    - 格式：`{reason_code}: {count} time(s), last at {ts}: {message}`。
    - 对 `task.resume`、`human.guidance` 等 recovery topic 本身不生成 digest，避免循环。
- **CLI 拒收闭环（文件 + 文档）：**
  - CLI 拒收继续写入 `recovery.jsonl`，不尝试跨进程实时同步到 loop runner（避免 IPC 复杂度）。
  - 在 `dimension-reviewer`、`executor`、`review-coordinator` 等 hat 的 instructions 中增加：
    - "如果你最近的 `ralph emit` 被静默忽略，先读 `.ralph/recovery.jsonl` 最后的 `reason_code`；不要反复用相同 payload 探测。"
  - 在 `docs/guide/runtime-diagnosis.md` 中增加 "agent 反复 emit 失败" 排查路径。

**Patterns to follow:**
- 复用 `diagnosis envelope` 与 `apply_runtime_diagnosis_prompt` 的注入位置（`event_loop/mod.rs:4595`）。
- `LoopState` 新增字段必须 `#[serde(default)]`，保证旧 `loops.json` 兼容。

**Test scenarios:**
- **Happy path:** runtime 连续 3 次拒绝 `executor → review.passed`（origin guard 或 policy）→ digest 中 `isolated_scope_violation: 3` 出现在 prompt。
- **Happy path:** agent 看到 digest 后停止同类错误 emit（通过 mock LLM output 或 BDD 验证）。
- **Edge case:** 无拒收时 prompt 中不出现空块。
- **Edge case:** recovery 事件（`task.resume`、`human.guidance`）不产生 digest。
- **Edge case:** 旧 `loops.json` 反序列化时 `recent_rejection_digest` 缺失 → 默认空 map，不 panic。
- **Integration:** CLI 拒收写入 `recovery.jsonl` 后，文档 instructions 能指引 agent 读取。
- **Integration:** `review.passed` payload 中 `skip_reason=aggregate_timeout`（不在 serial preset 白名单）被 policy 拒收 → digest 中出现 `invalid_field_value` 或对应 reason_code。

**Verification:**
- `cargo nextest run -p ralph-core -- diagnosis` 通过。
- 手动触发一次 runtime 非法事件，确认 prompt 中出现 `## RECENT REJECTIONS`。
- 手动触发一次 CLI 非法 emit，确认 `recovery.jsonl` 含结构化 reason_code 且文档指引可读。

---

- [ ] U7. **`suppress_human_guidance` 对 progress-steward 自动豁免**

**Goal:** 解决 `suppress_human_guidance=true` 与 `human.guidance(target=progress-steward)` 的冲突，让 steward 能看到 guidance 内容。

**Requirements:** R-REP2、R-D3（来自 loop stability requirements）。

**Dependencies:** U2（target 路由先修复，否则 target 到不了 steward）。

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`（`update_robot_guidance` / `apply_robot_guidance` / `collect_robot_guidance` 路径，约 `event_loop/mod.rs:4730-4795` 与 `4937-4977`）
- Modify: `crates/ralph-core/src/config/loop_config.rs`（`ProgressStewardConfig` 新增可选字段）
- Modify: `presets/en/ce-executor-serial.yml`（覆盖新配置默认值，若需要）
- Test: `crates/ralph-core/src/event_loop/` 相关测试

**Approach:**
- 为 `ProgressStewardConfig` 增加字段 `exempt_from_suppress_human_guidance: bool`，默认 `true`（backward-compatible：旧 preset 缺失该字段时 steward 仍被豁免）。
- 在 `update_robot_guidance` / `apply_robot_guidance` / `collect_robot_guidance` 路径中，判断豁免条件：
  - guidance 事件的 `target` == `progress-steward`；或
  - 当前 build_prompt 的 `hat_id` == `progress-steward` 且配置启用豁免。
- 满足豁免时，即使 `event_loop.suppress_human_guidance == true`，仍把 guidance 内容注入 `## ROBOT GUIDANCE` 块。
- 保持 `executor` 等 hat 的 suppress 行为不变。

**Patterns to follow:**
- `event_loop/mod.rs:4730-4795` 的 `update_robot_guidance` 与 `4937-4977` 的 `apply_robot_guidance`。
- `ProgressStewardConfig` 在 `crates/ralph-core/src/config/loop_config.rs` 约 318 行附近。

**Test scenarios:**
- **Happy path:** `suppress_human_guidance=true` + `human.guidance(target=progress-steward)` → steward prompt 中可见 `## ROBOT GUIDANCE`。
- **Happy path:** `suppress_human_guidance=true` + `human.guidance(target=executor)` → executor prompt 中不可见 guidance（保持原行为）。
- **Edge case:** `progress_steward.exempt_from_suppress_human_guidance=false` → steward 也被屏蔽，验证配置生效。
- **Edge case:** 旧 `ralph.yml` / preset 无 `exempt_from_suppress_human_guidance` → 默认 true，steward 可见 guidance。

**Verification:**
- `cargo nextest run -p ralph-core -- progress_steward` 或相关测试通过。
- BDD scenario 中 steward 被 `human.guidance` 激活后 emit recovery 事件。

---

- [ ] U8. **新增 handoff 产物审计并接入 CI**

**Goal:** 自动捕获"handoff 已开启但 0 文件"的静默失败。

**Requirements:** R-REP4、R-B1（来自 agent recovery requirements）。

**Dependencies:** U1 / U4（机制修复后审计才有意义）。

**Files:**
- Create: `crates/ralph-cli/src/commands/audit_hat_handoff.rs`（或复用 `diagnose` / `preset_check` 子命令）
- Create: `scripts/audit-hat-handoff-artifacts.sh`（bash 包装，调用 `ralph audit hat-handoff` 或等效命令）
- Modify: `crates/ralph-cli/src/commands/mod.rs`（暴露新子命令）
- Modify: `scripts/ci-rust-gate.sh` 或 `.github/workflows/*.yml`
- Test: `crates/ralph-cli/tests/` 集成测试 + CI smoke

**Approach:**
- **核心审计逻辑用 Rust 实现**，复用 `RalphConfig` 解析器：
  - 读取 `ralph.yml` / builtin preset 判断 `hat_handoff.enabled` 与 `execution_mode`。
  - 扫描 `.ralph/agent/hat-handoff/` 下文件数、文件名格式（`{iter}-{seq}-{from}-{to}.md`）、iter/seq 单调性。
  - 输出 JSON / 文本报告；发现 0 文件或格式错误时非零退出。
- **Bash 脚本只是薄包装**：调用 `ralph audit hat-handoff --path <.ralph>`（或等效），避免在 bash 中解析 YAML。
- 接入 CI：在 `ci-rust-gate.sh` 末尾或作为独立 job 运行；对示例 worktree 跑一个最小 isolated preset 后调用脚本。

**Patterns to follow:**
- 参考 `scripts/audit-file-sizes.sh` 与 `scripts/check-cli-doc-drift.sh` 的风格。
- 复用 `crates/ralph-cli/src/commands/diagnose.rs` 读取 `RalphConfig` 的模式。

**Test scenarios:**
- **Happy path:** handoff 开启且存在合法文件 → 审计命令退出 0。
- **Error path:** handoff 开启但 0 文件 → 退出非 0 并打印原因。
- **Error path:** 文件名不符合 `{iter}-{seq}-{from}-{to}.md` → 退出非 0。
- **Edge case:** handoff 关闭 → 审计命令跳过或退出 0。
- **Integration:** CI gate 在示例 preset 运行后调用脚本，验证不静默失败。

**Verification:**
- `cargo nextest run -p ralph-cli -- audit_hat_handoff`（或对应测试路径）通过。
- `./scripts/audit-hat-handoff-artifacts.sh` 在合规与不合规 `.ralph/` 上行为正确。
- CI 中新增 job 绿。

---

- [ ] U9. **文档同步与反向验证**

**Goal:** 保证文档、skill 与代码一致；满足 AGENTS.md 中"反向验证"硬规则。

**Requirements:** 仓库 AGENTS.md 硬规则、R-B1（来自 agent recovery requirements）。

**Dependencies:** U1 / U3 / U8（代码与 preset 变更后文档才需同步）。

**Files:**
- Modify: `crates/ralph-core/data/ralph-tools-handoff.md`（行号/命令反向验证）
- Modify: `docs/guide/runtime-diagnosis.md`
- Create: `docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md`（新建排查 runbook）
- 注意：本次不新增/删除/重命名 builtin preset，因此 AGENTS.md / CLAUDE.md 中的 preset **列表**通常无需同步；只有当 U3 改动了 AGENTS.md/CLAUDE.md 正文中 builtin preset 列表之外的描述时才更新。

**Approach:**
- 对 `ralph-tools-handoff.md` 中所有 `xxx.rs:NN-MM` 引用，用 `sed -n 'NN,MMp' <file>` 复核是否仍指向正确代码；若行号漂移立即修正。
- 更新 `runtime-diagnosis.md`：
  - 增加 "handoff stall" 排查清单（检查 `hat_handoff.enabled` / `execution_mode` / macro edge / prompt 注入 / 审计脚本）。
  - 增加 "progress_task_mismatch" 与 "review sequence stuck" 排查路径。
  - 增加 "agent 反复 emit 失败" 排查路径（读取 `recovery.jsonl`）。
- 新建 `docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md`：
  - 解释 handoff 机制、常见失败模式、修复动作、5 步检查清单。
- 若 U3 导致 AGENTS.md/CLAUDE.md 中相关段落需要更新，则按 AGENTS.md 规则 `cp CLAUDE.md AGENTS.md` 保持两者一致。

**Patterns to follow:**
- AGENTS.md 中"反向验证"硬规则：改完代码必须复核文档中的源码引用。
- 文档使用中文撰写（AGENTS.md 中文输出规则）。

**Test scenarios:**
- **Integration:** 运行 `ralph tools handoff prepare --help` 与相关命令做冒烟测试。
- **Integration:** `ralph diagnose` 在新 worktree 上能展示 handoff stall / progress 漂移排查步骤。
- **Integration:** 文档中 `xxx.rs:NN-MM` 引用全部复核通过。

**Verification:**
- 文档中无过时行号引用。
- `./scripts/check-cli-doc-drift.sh`（如存在）通过或至少无 handoff 相关 drift。
- 若改动了 CLAUDE.md，AGENTS.md 与 CLAUDE.md 内容一致（`diff` 为空）。

---

## System-Wide Impact

### Interaction Graph

- `crates/ralph-cli/src/commands/emit.rs` 与 `crates/ralph-cli/src/policy_check.rs`：U1 改动让 CLI 拒收更早、更严。
- `crates/ralph-proto/src/event_bus.rs`：U2 改动影响所有 `human.*` 事件路由，包括 TUI / Telegram 的 human interaction。
- `crates/ralph-core/src/event_loop/mod.rs`：U4/U5/U5b/U6/U7 改动影响 scope enforcement、prompt 构建、运行时诊断、human guidance 注入。
- `crates/ralph-core/src/event_loop/loop_state.rs`：U6 新增 digest 字段。
- `crates/ralph-core/src/config/loop_config.rs`：U7 新增 steward 配置字段。
- `presets/en/ce-executor-serial.yml` / `ce-executor-isolated.yml`：U3 改动影响 agent 行为契约。

### Error Propagation

- CLI gate 拒收：stderr + `recovery.jsonl`；agent 通过 instructions / 文档学习读取 recovery.jsonl。
- Runtime origin / policy / hat_handoff gate 拒收：
  - `event.isolation.boundary_violation` / `diagnostic.hat_handoff.rejected` + `task.resume`（若可恢复）。
  - U6 的 `state.recent_rejection_digest` 注入当前 hat prompt。
- Anonymous business topic 拒收（U5）：`event.isolation.boundary_violation` + `task.resume(target=ralph)`。
- handoff 注入失败：`diagnostic.hat_handoff.inject_failed`。

### State Lifecycle Risks

- **Partial-write:** U5/U5b 让缺少 provenance 的匿名 business topic 无法进入 state projection，避免污染。
- **Duplicate resume:** U3 加入 `task.resume` trigger 后，若 hat 连续收到多个 `task.resume`，需确保不重复 emit terminal；依赖现有 per-turn dedup 与 retry budget。
- **Prompt length:** U6 的 recovery digest 与 U4 的 handoff 注入均有上限，避免 prompt 爆炸。
- **State compatibility:** U6 新增 `LoopState` 字段必须 `#[serde(default)]`，保证旧 `loops.json` 兼容。

### API Surface Parity

- CLI `ralph emit` 行为更严格（U1），但只影响 isolated 模式 + 无 hat / 宏观边缺失场景。
- Runtime scope enforcement 更严格（U5/U5b），但只影响 isolated 模式下缺少 provenance 的匿名 business topic。
- `EventBus::publish` 行为对 `human.*` + target 改变，需确认 TUI / Telegram 不依赖旧行为。

### Integration Coverage

- BDD scenario 需要覆盖：
  - `dimension-reviewer` 被 `task.resume` 重新激活并 emit terminal。
  - `progress-steward` 被 `human.guidance(target=steward)` 激活并 emit recovery。
  - `handoff` 开启后宏观边 emit 无 `handoff_path` 被 CLI/runtime 双拒。
  - 缺少 provenance 的匿名 business topic 被 runtime scope enforcement 拒绝。
  - CLI 非法 emit 写入 `recovery.jsonl` 并可通过文档指引读取。
  - runtime 拒收后 prompt 中出现 `## RECENT REJECTIONS`。

### Unchanged Invariants

- coordinator 模式下的 event routing 与 origin guard 行为不变。
- `presets/schemas/` SSOT 机制不变（已在其他计划落地）。
- `ralph-tools.md` 自动注入机制不变（只更新内容）。

---

## Risks & Dependencies

| Risk | Likelihood | Impact | Mitigation |
|------|-----------|--------|------------|
| U2 改变 `human.*` 路由，破坏 TUI / Telegram 的 human interaction | 中 | 高 | 添加专门测试覆盖无 target 的 `human.*` 行为；保持无 target 时原逻辑不变；检查 `crates/ralph-tui` / `crates/ralph-telegram` 是否依赖 `human_pending` |
| U5/U5b 让旧 worktree 中合法的匿名 business topic suddenly 被拒 | 中 | 中 | 仅在 isolated 模式启用；只拦截缺少 `hat`/`source`/`triggered` 的事件；提供明确 reason_code 与 `task.resume(target=ralph)` 恢复路径 |
| U3 的 instructions 过长，挤占 prompt token | 低 | 中 | recovery 语义小节控制在 200 词以内；可放到 skill 深参考而非每轮注入 |
| U6 的 recovery digest 让 agent 过度关注历史错误 | 低 | 中 | 只保留最近 5 条 reason_code + 1 条示例；超限时提示"联系 operator" |
| preset 内容事实源同步遗漏 | 中 | 高 | U3 明确列出 YAML / `presets.rs` / `manifest.yml` / `index.json` 清单；zsh 补全与 AGENTS.md 仅在集合变化时更新 |
| 测试运行时间增加 | 低 | 低 | 新测试优先用单元测试；BDD 只加 3-4 个关键 scenario |
| U8 Rust 审计子命令增加 CLI 维护面 | 低 | 低 | 审计逻辑尽量复用 `RalphConfig` / `HandoffIndex`；bash 脚本为薄包装 |

---

## Documentation / Operational Notes

- **新建排查 runbook:** `docs/solutions/2026-06-18-002-feat-isolated-hat-handoff.md` 需包含：
  - handoff 未生效 5 步检查清单。
  - review sequence stuck 的排查路径。
  - progress-steward 不响应的排查路径。
  - agent 反复 emit 失败的排查路径（读取 `recovery.jsonl`）。
- **更新 `docs/guide/runtime-diagnosis.md`:** 增加 `handoff_dispatch_timeout`、`progress_task_mismatch`、`review_sequence_stuck` 的症状与修复动作。
- **CI:** 新增 `scripts/audit-hat-handoff-artifacts.sh`（调用 Rust 审计子命令），确保 handoff 开启 preset 的示例运行后至少产生 1 个 handoff 文件。
- **Operator 沟通:** 本次改动会让 `ce-executor-serial` 对无 hat / 无 `handoff_path` 的 emit 更早失败，并让缺少 provenance 的匿名 business topic 无法进入 state projection，属于预期中的 fail-closed 收紧。

---

## Sources & References

- **Origin documents:**
  - `docs/brainstorms/2026-06-18-isolated-hat-handoff-requirements.md`
  - `docs/brainstorms/2026-06-17-agent-recovery-mechanism-gaps-requirements.md`
  - `docs/brainstorms/2026-06-16-ce-executor-loop-stability-requirements.md`
- **Diagnosis report:** `docs/report/2026-06-19-ce-executor-serial-warm-tiger-loop-diagnosis.md`
- **Related plans:**
  - `docs/plans/2026-06-18-002-feat-isolated-hat-handoff-plan.md`
  - `docs/plans/2026-06-18-005-fix-isolated-hat-handoff-review-findings-plan.md`
  - `docs/plans/2026-06-16-001-feat-ce-executor-bootstrap-recovery-plan.md`
- **Related reports:**
  - `docs/report/2026-06-18-003-base-stability-implementation-paths.md`
