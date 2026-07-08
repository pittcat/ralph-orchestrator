---
title: "fix: OPAC P0/P1/P2 逻辑断层修复（isolated 模式稳定性加固）"
type: fix
status: active
date: 2026-07-04
origin: docs/plans/2026-07-04-001-feat-opac-isolated-agent-discipline-plan.md
related_plans:
  - docs/plans/2026-07-04-001-feat-opac-isolated-agent-discipline-plan.md
  - docs/plans/2026-07-03-001-feat-supervisor-rusqlite-parallel-preset-plan.md
---

# fix: OPAC P0/P1/P2 逻辑断层修复（isolated 模式稳定性加固）

## Overview

`docs/plans/2026-07-04-001-feat-opac-isolated-agent-discipline-plan.md` 已实现 U1–U26 的全部代码提交，但对抗性审查发现：OPAC 的三层防御（L1 Prompt、L2 CLI ACL、L3 Runtime）在 **关键闭合路径** 上存在逻辑断层。代码“看起来都做了”，但直接拿去用会导致 130118 / 093813 类 P0 复发。

本计划针对审查报告中的 **全部 P0 / P1 / P2 问题** 制定修复路径，目标是在不推翻既有架构的前提下，把 OPAC 的 Observe → Precheck → Apply → Confirm 四阶段真正闭合。

---

## Problem Frame

### P0 级断层（目标脱轨）

| 问题 | 影响 | 根因 |
|------|------|------|
| **U7 completion-emit 告警读错文件** | agent 被误导重复 emit，触发 budget drop | `task_cli.rs` 把 `.ralph/current-hat-events` marker 文件当 JSONL 解析 |
| **U13 isolated budget carve-out 是死代码** | 130118 类 serial walk 仍被静默 drop | `event_loop/mod.rs` 的 exemption 分支被冗余守卫覆盖，永不可达 |
| **U16 task.resume 路由校验被绕过** | recovery 事件投给不订阅的 hat，造成 stall | 调用方传 `None` 给 `validate_resume_routing`；函数只 warn 不 block |

### P1 级偏离（计划效果大打折扣）

| 问题 | 影响 | 根因 |
|------|------|------|
| `completion_publishes` 双 SSOT | agent 看到的 completion 列表与 runtime 检查的不一致 | `hat_identity.rs` 用启发式，`completion_emit.rs` 用 `event_policy` |
| U7 用 owner 而非 caller hat | coordinator 关 worker task 时给出错误的 `expected_topics` | `task_cli.rs` 使用 `owner_hat.or_else(current_hat)` |
| `inspect loop` 报告 marker 大小 | agent 无法确认真实 hat-channel 状态 | `commands/inspect.rs` 直接 stat marker 文件 |
| `wave verify` 输出形状不符 | agent 无法按计划在 verify 阶段拿到 `wave_id` | `wave.rs` 输出 `{topic,count}` 而非 `{wave_id,topics}` |
| `wave verify` 未执行 origin guard | supervisor-only 协调 topic 可被 verify 放行 | precheck 只跑 schema / terminal / denied，缺 `event_origin` |
| `ce-executor-supervisor` event_policy fail-open | agent 可绕过 precheck | 未显式设置 `allow_unsafe_cli_emit: false` 等字段 |
| 4 个 preset emitter hat 未引用 OPAC skill | lint 绿但 agent instructions 缺纪律文档 | `instructions_opac.rs` 的 `EMITTER_TOPICS` 过窄 |

### P2 级妥协（次要但需补齐）

- U6/U7 空 channel 提示文案未共享
- U7 读取整个 channel 文件而非 tail
- `wave verify` 未强制为 `wave emit` 的前置步骤
- `inspect loop` supervisor 摘要缺少 `slot_summary` / `last_coordination_topics`
- `ce-executor-serial` 用 `exempt_topics` 而非 `business_topics` 作 serial-walk SSOT
- U25 / U19 / U18 的 BDD 覆盖缺失
- `merge-loop` / `merge-batch` 无 schema
- zsh 补全缺 `verify` / `verify-emit-bridge`

---

## Requirements Trace

- **R1** — 修复 U7 marker 解析，使 close→completion-emit warning 真正读取 hat-channel 文件（对应 P0 #1）。
- **R2** — 统一 `completion_publishes` 计算 SSOT，从 `event_policy.terminal_topics ∪ business_topics` 派生（对应 P1 #4）。
- **R3** — U7 warning 使用 caller hat 而非 owner hat（对应 P1 #5）。
- **R4** — 修复 U13 carve-out 死代码，让 serial/multi-publish 业务 topic 跨 activation 各 emit 一次（对应 P0 #2）。
- **R5** — 修复 U16，让 `task.resume` 在 consumer hat 不订阅 trigger topic 时被 block（对应 P0 #3）。
- **R6** — `ralph inspect loop` 报告解析后的 hat-channel 文件路径与大小（对应 P1 #6）。
- **R7** — `ralph wave verify` 输出 `{ok, wave_id, topics}` 并执行 origin guard（对应 P1 #7、#8）。
- **R8** — `ce-executor-supervisor` 显式 fail-closed `event_policy`（对应 P1 #9）。
- **R9** — `ce-executor-serial` 将 serial-walk topics 移入 `event_policy.business_topics`（对应 P2 #15）。
- **R10** — 扩展 `preset_lint` emitter 覆盖并补齐 4 个 preset 的 OPAC skill 引用（对应 P1 #10）。
- **R11** — 补齐 supervisor `inspect loop` 摘要、`wave` 两步顺序、空 channel 共享提示、BDD、schema、zsh 补全（对应 P2）。

---

## Scope Boundaries

### In scope

- `task_cli.rs` / `completion_emit.rs` / `hat_identity.rs` 的 U7 修复
- `event_loop/mod.rs` 的 U13 carve-out 与 U16 resume 路由修复
- `commands/inspect.rs` / `cli/emit_path.rs` 的 marker 解析统一
- `wave.rs` / `policy_check.rs` 的 verify 输出与 origin guard
- `ce-executor-supervisor.yml` / `ce-executor-serial.yml` 及 schema 的策略收紧
- `preset_lint/instructions_opac.rs` 与 4 个 preset instructions 补全
- `supervisor/mod.rs` inspect 摘要补全
- 缺失的 BDD scenarios、merge preset schemas、zsh 补全
- 全量验证：nextest、preset_lint strict、SSOT byte-equality、BDD、`check-cli-doc-drift.sh`、`validate-builtin-presets.sh --strict`

### Non-goals

- 不重构 OPAC 整体架构（只修断层）
- 不新增 event_loop 配置字段
- 不改人类 CLI 的 bypass 语义
- 不恢复完整 `hat_handoff` 五段文件机制

### Deferred to Follow-Up Work

- 跨进程 `wave verify` → `wave emit` 强制 token 机制（若 U21 的两步顺序必须字面兑现）：当前计划通过“emit 自身 precheck 已含 origin guard + agent context 默认 enforce”来闭合，不引入跨调用状态文件

---

## Context & Research

### Relevant Code and Patterns

| 文件 | 用途 |
|------|------|
| `crates/ralph-cli/src/task_cli.rs` | U7 completion-emit warning实现位置 |
| `crates/ralph-core/src/completion_emit.rs` | completion-class topic 计算 helper |
| `crates/ralph-core/src/hat_identity.rs` | `HatIdentitySnapshot` SSOT |
| `crates/ralph-cli/src/commands/events.rs` | marker → channel 解析逻辑（应复用） |
| `crates/ralph-cli/src/cli/emit_path.rs` | 共享 marker 解析 helper 候选位置 |
| `crates/ralph-core/src/event_loop/mod.rs` | U13 carve-out、U16 resume 路由 |
| `crates/ralph-cli/src/commands/inspect.rs` | `inspect loop` 实现 |
| `crates/ralph-cli/src/wave.rs` | `wave verify` / `wave emit` |
| `crates/ralph-cli/src/policy_check.rs` | `ValidationPipeline` / batch precheck |
| `crates/ralph-core/src/event_origin.rs` | origin guard / supervisor-only topic 列表 |
| `crates/ralph-core/src/supervisor/mod.rs` | supervisor summary API |
| `crates/ralph-core/src/preset_lint/instructions_opac.rs` | instructions lint rule |
| `presets/en/ce-executor-supervisor.yml` | supervisor preset event_policy |
| `presets/en/ce-executor-serial.yml` | serial preset business_topics |
| `presets/schemas/*.yml` | schema SSOT |
| `crates/ralph-core/tests/scenarios/` | BDD fixtures |
| `scripts/ralph-zsh-plugin.zsh` | zsh 补全 |

### Institutional Learnings

- `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md`：仅靠 prompt 不够，必须在 CLI 边界拒收 + 给出可读 recovery
- `docs/achieved/brainstorms/2026-05-31-agent-operation-guard-requirements.md`：`RALPH_CURRENT_HAT` 不可信，必须在 ingestion 端用 registry 校验
- 2026-06-24 P0-2/P0-3 教训：BDD 必须用 `run_workflow_guard_scenario` 断言事件，不能用 stub

### External References

- 无外部引用；问题全部来自本地代码审查

---

## Key Technical Decisions

- **KTD-1** — 把 marker → channel 解析提取为 `cli/emit_path.rs` 的共享 helper，U1 / U4 / U6 共用，避免三处漂移。
- **KTD-2** — `completion_publishes` 不在 `HatIdentitySnapshot` 中预存，统一用 `event_policy.terminal_topics ∪ business_topics` 实时计算。
- **KTD-3** — U7 warning 的 caller hat 取 `OperationContext::current_hat_id`；owner 仅作 fallback。
- **KTD-4** — U13 carve-out 分支去掉冗余 `!non_wave_business_event_accepted` 守卫，使其在 slot 已被占用时仍可触发；同一 activation 内重复 exempt topic 仍 drop/deny。
- **KTD-5** — U16 `validate_resume_routing` 改为 Allow/Block 决策，调用方在 Block 时跳过 publication 并写 diagnostic。
- **KTD-6** — `wave verify` 输出统一为 `{ok, wave_id, topics}`；若 `wave_id` 在 verify 阶段确实无法生成（例如由 runtime 分配），则在实现时更新本计划并说明理由。
- **KTD-7** — `wave` Precheck 的 origin guard 直接在 `run_wave_precheck` / `execute_verify` 中执行，确保 verify 与 emit 同源；emit 路径继续依赖 U15 的 agent-context policy-check enforce。
- **KTD-8** — `ce-executor-supervisor` 显式声明 `allow_unsafe_cli_emit: false`、`require_policy_check_for_cli_emit: true`、`business_topics`、`terminal_topics`、`completion_after_terminal: true`。
- **KTD-9** — `ce-executor-serial` 的 `review.dimension.ready` / `review.dimensions.complete` 从 `review-coordinator.exempt_topics` 移入 `event_policy.business_topics`，让 U13 carve-out 按设计生效。
- **KTD-10** — `preset_lint/instructions_opac.rs` 的 emitter 判定改为从每个 preset 的 `hat.publishes` 动态派生，不再维护固定 `EMITTER_TOPICS` 白名单。

---

## Open Questions

### Resolved During Planning

- **Q: 是否需要跨进程 verify → emit token？** — 否。通过“emit 自身 precheck 已含 origin guard + agent context 默认 enforce policy-check”闭合 OPAC Apply 阶段，不引入跨调用状态文件。
- **Q: `completion_publishes` 计算放在哪？** — 从 `HatIdentitySnapshot` 删除该字段，在 `completion_emit.rs` 提供 `derive_completion_publishes(config, hat_id)`，prompt/inspect/warning 三处按需调用。
- **Q: 是否修改已完成的 2026-07-04-001 plan？** — 不修改原 plan。本 plan 作为后续 fix plan 独立存在，原 plan 保持 `status: completed` 以记录历史。

### Deferred to Implementation

- 具体 `wave_id` 在 verify 阶段是否已确定（影响 KTD-6 输出形状），实现时确认后必要时更新本计划。
- `supervisor` 的 `last_coordination_topics` 具体来源（runtime 注入摘要 vs store 查询），实现时根据 `supervisor/mod.rs` 已有 API 选择。

---

## High-Level Technical Design

> *This illustrates the intended approach and is directional guidance for review, not implementation specification. The implementing agent should treat it as context, not code to reproduce.*

```mermaid
flowchart TB
  subgraph FixU1["U1/U4: hat-channel marker resolution"]
    M[.ralph/current-hat-events marker]
    R[emit_path::resolve_hat_channel_file]
    C[actual channel JSONL]
    M --> R --> C
  end

  subgraph FixU7["U7: completion-emit warning"]
    C --> T[tail parse last N lines]
    EP[event_policy terminal/business topics]
    H[caller hat publishes]
    EP --> DER[derive_completion_publishes]
    H --> DER
    T --> CHECK{any expected topic?}
    DER --> CHECK
    CHECK -->|no| WARN[stderr JSON warning]
  end

  subgraph FixU13["U13: carve-out"]
    B[budget admission]
    EX[is_isolated_exempt_topic]
    B -->|slot consumed + exempt| ADMIT[admit one]
  end

  subgraph FixU16["U16: resume routing"]
    TOP[original trigger topic]
    HI[HandoffIndex::consumer_of]
    TR[consumer triggers]
    TOP --> HI --> TR --> DEC{match?}
    DEC -->|no| BLOCK[block publication]
  end

  subgraph FixWave["U5: wave verify"]
    V[wave verify]
    OG[event_origin guard]
    OUT[{ok, wave_id, topics}]
    V --> OG --> OUT
  end
```

---

## Implementation Units

- [ ] **U1: 修复 U7 completion-emit 告警并统一 completion SSOT**

**Goal:** 让 `task close` 后的 completion-emit warning 真正读取正确的 hat-channel 文件，并用与 prompt/inspect 同源的 `event_policy` 计算 completion topics。

**Requirements:** R1, R2, R3

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-cli/src/task_cli.rs`
- Modify: `crates/ralph-core/src/completion_emit.rs`
- Modify: `crates/ralph-core/src/hat_identity.rs`
- Modify: `crates/ralph-cli/src/commands/events.rs`
- Modify: `crates/ralph-cli/src/cli/emit_path.rs`
- Test: `crates/ralph-cli/src/task_cli.rs`, `crates/ralph-core/src/hat_identity.rs`

**Approach:**
- 在 `cli/emit_path.rs` 新增共享 helper `resolve_hat_channel_file(root)`，读取 `.ralph/current-hat-events` marker 并返回指向的真实 channel 文件路径；`commands/events.rs` 的现有逻辑迁移到该 helper。
- `HatIdentitySnapshot` 删除 `completion_publishes` 字段；新增 `derive_completion_publishes(config, hat_id)` 函数，计算 `hat.publishes ∩ (event_policy.terminal_topics ∪ event_policy.business_topics)`。
- `emit_close_completion_warning` 改为：先 resolve marker → 读 channel tail → 解析 topic；使用 `ctx.current_hat_id` 作为 caller hat，不再使用 `owner_hat`。
- 共享空/不可读 channel 提示文案；解析最近 N 行（如 50）而非整个文件。

**Patterns to follow:**
- `commands/events.rs` 现有 marker 解析
- `completion_emit.rs` 现有 completion-class 计算

**Test scenarios:**
- Happy path: marker 指向包含 `work.done` 的 channel 文件；close 后无 warning
- Error path: marker 指向无 completion topic 的 channel 文件；close 后 stderr JSON 含 `expected_topics` 与 `next_step`
- Edge case: marker 文件缺失；warning 含 `channel_missing_marker`
- Edge case: channel 文件不可读；warning 含 `channel_unreadable`
- Edge case: caller hat 无 completion-class topics；无 warning
- Regression: 人类 CLI close（无 `RALPH_CURRENT_HAT`）保持静默
- Integration: marker 内容是相对路径字符串，不是 JSONL

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- task_cli` 绿
- `cargo nextest run -p ralph-core -- hat_identity` 绿

---

- [ ] **U2: 修复 U13 isolated budget carve-out 死代码**

**Goal:** 让声明为 serial/multi-publish 的业务 topic 在不同 activation 间各 emit 一次，不被单事件预算静默丢弃。

**Requirements:** R4

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/event_loop/tests/scope_enforcement.rs`（或新建 `serial_publish_carveout.rs`）

**Approach:**
- 定位 budget admission 中 `is_isolated_exempt_topic` 分支，移除冗余的 `!non_wave_business_event_accepted` 前置守卫，使该分支在 slot 已占用且 topic 属于 exemption 集合时触发。
- 限制同一 activation 内仅允许一条 exempt topic；重复 exempt topic 仍按原 budget 处理（drop/deny + diagnostic）。
- 保持对 `event_policy.business_topics` / `terminal_topics` 以及 hat `exempt_topics` 的兼容。

**Patterns to follow:**
- 现有 `is_isolated_exempt_topic` helper
- 现有 scope_enforcement 测试风格

**Test scenarios:**
- Happy path: 6 个 activation 各 emit 一条 `review.dimension.ready`；6 条全部 accept
- Error path: 同一 activation emit 两条 exempt topic；第二条 drop/deny
- Edge case: topic 不在 `business_topics` / `terminal_topics` / `exempt_topics` 中；原 budget 行为不变
- Regression: 非 exempt 业务事件预算仍生效

**Verification:**
- `cargo nextest run -p ralph-core -- scope_enforcement` 绿
- 现有 `u13_business_topics_carve_out_admits_serial_walk` 类测试仍绿

---

- [ ] **U3: 修复 U16 task.resume consumer 路由校验**

**Goal:** 让 `task.resume` 事件只投给真正订阅该 trigger topic 的 consumer hat。

**Requirements:** R5

**Dependencies:** None

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`
- Test: `crates/ralph-core/src/event_loop/tests/handoff_dispatch.rs`

**Approach:**
- 将所有 `validate_resume_routing(..., None)` 调用点改为传入原始 trigger topic。
- 将 `validate_resume_routing` 返回值从 `Option<String>` 改为 `Allow / Block` 决策。
- Block 时调用方跳过 publication 并发布 diagnostic event（不 silent stall）。
- fallback “no events” 注入点无原始 topic，保留 no-op 行为并加注释说明。

**Patterns to follow:**
- `HandoffIndex::consumer_of`（`crates/ralph-core/src/workflow_contract/handoff_index.rs`）
- registry `triggers` 匹配逻辑

**Test scenarios:**
- Happy path: consumer hat `triggers` 包含 topic；resume 正常注入
- Error path: consumer hat `triggers` 不包含 topic；resume 被 block + diagnostic
- Error path: `HandoffIndex::consumer_of` 返回的 consumer 与目标 hat 不一致；resume 被 block
- Edge case: fallback no-events 注入点（无原始 topic）允许继续
- Regression: 现有 recovery flow 不被破坏

**Verification:**
- `cargo nextest run -p ralph-core -- handoff_dispatch` 绿
- `cargo nextest run -p ralph-core --test scenarios` 无回归

---

- [ ] **U4: 修复 `ralph inspect loop` hat-channel 报告**

**Goal:** 让 `inspect loop` 输出解析后的真实 hat-channel 文件路径与大小，而不是 marker 文件。

**Requirements:** R6

**Dependencies:** U1

**Files:**
- Modify: `crates/ralph-cli/src/commands/inspect.rs`
- Test: `crates/ralph-cli/src/commands/inspect.rs` tests

**Approach:**
- 复用 U1 在 `cli/emit_path.rs` 中新增的 `resolve_hat_channel_file` helper。
- `inspect loop` JSON 输出中 `hat_channel_file` / `hat_channel_size` 改为 resolved channel 文件。
- 空/不可读 channel 的 warning 文案与 U1 共享 helper 一致。

**Patterns to follow:**
- `commands/inspect.rs` 现有 JSON 输出结构
- U1 共享 helper

**Test scenarios:**
- Happy path: marker 存在并解析为 channel 文件；JSON 含正确 resolved path 与 size
- Edge case: marker 缺失；输出清晰 note
- Edge case: resolved channel 文件 0 字节；warnings 数组非空
- Regression: human 格式仍含关键字段标题

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- inspect` 绿

---

- [ ] **U5: 修复 `ralph wave verify` 输出、origin guard 与两步顺序**

**Goal:** 让 `ralph wave verify` 与 `wave emit` 同源执行 origin guard，输出包含 `wave_id` + `topics`，并明确 Precheck 语义。

**Requirements:** R7

**Dependencies:** U3（origin guard 模式参考）

**Files:**
- Modify: `crates/ralph-cli/src/wave.rs`
- Modify: `crates/ralph-cli/src/policy_check.rs`
- Modify: `crates/ralph-core/data/ralph-tools-wave.md`
- Test: `crates/ralph-cli/src/wave.rs` tests

**Approach:**
- 调整 `wave verify` 输出为 `{ "ok": true, "wave_id": "...", "topics": [...] }`；若 `wave_id` 在该阶段不可生成，实现时更新本计划并同步文档。
- 在 `run_wave_precheck` / `execute_verify` 中调用 `event_origin::validate_event_origin`，对 `SUPERVISOR_COORDINATION_TOPICS` 等 agent 不可发 topic 给出与 runtime 一致的 rejection。
- `wave emit` 的 precheck 同样包含 origin guard；结合 U15 的 agent-context policy-check enforce，实现 Apply 阶段无法绕过 Precheck。
- 更新 `ralph-tools-wave.md` 中 verify 输出示例与 OPAC 章节。

**Patterns to follow:**
- `policy_check.rs` 现有 `validate_batch_against_config`
- `event_origin.rs` 现有 origin guard

**Test scenarios:**
- Happy path: dispatcher fixture wave verify 输出 `{ok, wave_id, topics}`
- Error path: verify 含 `review.wave.complete` 等 supervisor-only topic 被 origin guard 拒绝
- Error path: verify schema 不通过返回结构化 errors
- Integration: wave emit 含 supervisor-only topic 在 agent context 下同样被拒
- Regression: human CLI wave emit 带 `--unsafe-no-policy-check` opt-out 仍可用

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- wave` 绿
- `ralph wave verify --help` 与 `ralph-tools-wave.md` 一致

---

- [ ] **U6: 收紧 `ce-executor-supervisor` event_policy 并统一 `ce-executor-serial` business_topics SSOT**

**Goal:** supervisor preset 默认 fail-closed；serial preset 的 serial-walk topic 走 `event_policy.business_topics` 而非 `exempt_topics`。

**Requirements:** R8, R9

**Dependencies:** U2（carve-out 运行时修复）

**Files:**
- Modify: `presets/en/ce-executor-supervisor.yml`
- Modify: `presets/en/ce-executor-serial.yml`
- Modify: `presets/schemas/ce-executor-supervisor.yml`
- Modify: `presets/schemas/ce-executor-serial.yml`
- Test: `cargo nextest run -p ralph-cli --bin ralph -- preset_lint`，`cargo nextest run -p ralph-core -- preset_lint`

**Approach:**
- `ce-executor-supervisor.yml` 的 `event_policy` 显式设置：
  - `allow_unsafe_cli_emit: false`
  - `require_policy_check_for_cli_emit: true`
  - `business_topics`（含 `work.ready`, `review.complete`, `fix.done`, `plan.complete` 等 handoff 类 topic）
  - `terminal_topics`（含 `LOOP_COMPLETE`, `plan.complete` 等）
  - `completion_after_terminal: true`
- `ce-executor-serial.yml` 将 `review-coordinator.exempt_topics` 中的 `review.dimension.ready` / `review.dimensions.complete` 移入 `event_policy.business_topics`，并删除冗余 `exempt_topics`（保留其他真正的 scope exception）。
- 同步 schema 中的 `required_fields`、`topic_deny_rules` 与 `event_policy` 字段。

**Patterns to follow:**
- `ce-executor-serial.yml` 现有 `event_policy` 块结构
- AGENTS.md preset/schema 改动下游同步清单

**Test scenarios:**
- `ralph preset check -H builtin:ce-executor-supervisor --strict` 绿
- `ralph preset check -H builtin:ce-executor-serial --strict` 绿
- SSOT byte-equality `test_ce_executor_*_preset_matches_embedded` 绿
- 6-dim serial walk BDD 仍 accept 6 条 `review.dimension.ready`

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- preset_lint` 绿
- `cargo nextest run -p ralph-core -- preset_lint` 绿

---

- [ ] **U7: 扩展 preset_lint emitter 覆盖并补齐 4 个 preset instructions**

**Goal:** 所有 emitter hat 的 instructions 必须引用 OPAC skill 文档。

**Requirements:** R10

**Dependencies:** U6（preset 结构稳定）

**Files:**
- Modify: `crates/ralph-core/src/preset_lint/instructions_opac.rs`
- Modify: `presets/en/autoresearch.yml`
- Modify: `presets/en/debug.yml`
- Modify: `presets/en/merge-loop.yml`
- Modify: `presets/en/merge-batch.yml`
- Test: `crates/ralph-core/src/preset_lint/instructions_opac.rs`

**Approach:**
- 将 `instructions_opac.rs` 的 emitter 判定从固定 `EMITTER_TOPICS` 改为动态派生：hat 的 `publishes` 非空即视为 emitter（或至少覆盖所有 builtin preset 的业务 topic）。
- 对 `autoresearch`、`debug`、`merge-loop`、`merge-batch` 的 emitter hat instructions 追加 `ralph-tools-opac` 与 `ralph-tools-emit` §5 precheck 的引用；只引用不复制内容。

**Patterns to follow:**
- 现有 `instructions_opac.rs` regex 与 finding 定义
- HARD RULE 4/8：hat instructions 引用 skill，不复述

**Test scenarios:**
- Error path: emitter hat instructions 缺少 `ralph-tools-opac` / `ralph-tools-emit` 引用 → finding
- Happy path: 合规 instructions → 无 finding
- Regression: 非 emitter hat 不被误报
- Integration: 7 个 embedded preset lint strict 全绿

**Verification:**
- `cargo nextest run -p ralph-core -- preset_lint instructions` 绿
- `./scripts/validate-builtin-presets.sh --strict` 绿

---

- [ ] **U8: 补全 `ralph inspect loop` supervisor 摘要**

**Goal:** 让 supervisor preset 的 Observe 阶段可看到 slot 状态与最近协调 topic，而无需读 db 或 events.jsonl tail。

**Requirements:** R11（supervisor 摘要部分）

**Dependencies:** U4

**Files:**
- Modify: `crates/ralph-core/src/supervisor/mod.rs`
- Modify: `crates/ralph-cli/src/commands/inspect.rs`
- Modify: `crates/ralph-core/data/ralph-tools-opac.md`
- Test: `crates/ralph-cli/src/commands/inspect.rs` tests

**Approach:**
- 在 `supervisor/mod.rs` 新增或复用 list-slots API，返回 slot_id / hat / status。
- `summarize()` 填充 `slot_summary[]` 与 `last_coordination_topics[]`（来源可为 runtime 注入摘要或 supervisor store 最近协调事件，不含 db 路径）。
- `inspect loop --format json` 在 `event_loop.supervisor.enabled` 时输出这些字段。
- 更新 `ralph-tools-opac.md` Observe 章节。

**Patterns to follow:**
- `supervisor/mod.rs` 现有 `summarize()` 结构
- U22 既有 inspect loop supervisor 块

**Test scenarios:**
- supervisor disabled → JSON 无 `supervisor` 键
- mock active wave → `active_waves`、`queue_depth`、`slot_summary` populated
- `last_coordination_topics` 返回最近协调 topic 列表
- 输出不含 db 路径等内部 ledger 信息

**Verification:**
- `cargo nextest run -p ralph-cli --bin ralph -- inspect` 绿
- `cargo nextest run -p ralph-core -- supervisor` 绿（若存在）

---

- [ ] **U9: 补齐缺失的 BDD scenarios**

**Goal:** 用真 EventLoop runner 覆盖 fix-unit 链、6-dim serial walk、supervisor fan-out/fan-in、macro-edge hint。

**Requirements:** R11（BDD 覆盖部分）

**Dependencies:** U2, U3, U6, U8

**Files:**
- Create: `crates/ralph-core/tests/scenarios/opac/ce_executor_serial_fix_unit_chain.yml`
- Create or扩展: `crates/ralph-core/tests/scenarios/opac/ce_executor_serial_serial_walk_6dim.yml`
- Create: `crates/ralph-core/tests/scenarios/supervisor/ce_executor_supervisor_exec_wave_fanout.yml`
- Create: `crates/ralph-core/tests/scenarios/supervisor/ce_executor_supervisor_review_batch.yml`
- Modify: `crates/ralph-core/tests/scenarios.rs`
- Test: `cargo nextest run -p ralph-core --test scenarios`

**Approach:**
- 全部使用 `run_workflow_guard_scenario` 并断言 `expected.events`。
- fix-unit chain：fresh mint → `work.ready` → close → `work.done` → `test.passed`。
- serial walk：6 activation 各 emit 一条 `review.dimension.ready`，断言 6 条 accept。
- supervisor exec wave：3 `exec.unit.ready` → worker → `work.done`/`test.passed` → `exec.wave.complete`。
- review batch：`review.unit.ready` M → `review.complete`（agent）accept；agent emit `review.wave.complete` reject。
- macro-edge：emit 带 `next_hint` → 下游 prompt 含 `## NEXT ACTION`。

**Patterns to follow:**
- `crates/ralph-core/tests/scenarios/opac/isolated_agent_discipline.yml`
- `crates/ralph-core/tests/scenarios/supervisor/ce_executor_supervisor_minimal.yml`
- `run_workflow_guard_scenario`（非 stub）

**Test scenarios:**
- FV-1: fix-unit chain 事件链完整
- FV-2: 6 条 `review.dimension.ready` 无 silent drop
- SB-1: fan-out 3 unit 触发 integrator
- SB-2: `review.complete` accept，`review.wave.complete` 被 origin guard reject
- ME-1: macro-edge `next_hint` 出现在下游 prompt

**Verification:**
- `cargo nextest run -p ralph-core --test scenarios opac`
- `cargo nextest run -p ralph-core --features supervisor-db --test scenarios ce_executor_supervisor`

---

- [ ] **U10: 补齐 merge preset schemas 与 zsh 补全**

**Goal:** 让 `merge-loop` / `merge-batch` 有 schema 支撑，并让 zsh 补全覆盖 `verify` / `verify-emit-bridge`。

**Requirements:** R11（schema / zsh 部分）

**Dependencies:** U7

**Files:**
- Create: `presets/schemas/merge-loop.yml`
- Create: `presets/schemas/merge-batch.yml`
- Modify: `presets/en/merge-loop.yml`
- Modify: `presets/en/merge-batch.yml`
- Modify: `scripts/ralph-zsh-plugin.zsh`
- Test: `zsh -n scripts/ralph-zsh-plugin.zsh`，`cargo nextest run -p ralph-cli --bin ralph -- preset_lint`

**Approach:**
- 为 merge-loop / merge-batch 创建 schema，定义 merge 领域 topic（`merge.ready`, `merge.done`, `merge.reviewed`, `merge.integrated`, `merge.retest`, `merge.stabilized`, `merge.batch.complete`, `conflict.*`, `tests.*` 等）、`required_fields`、`topic_deny_rules`。
- 在 preset YAML 中引用对应 schema。
- 在 `_ralph_task_args` 中增加 `verify`（镜像 start/close/fail/reopen）与 `verify-emit-bridge`（`--task-id`, `--task-key`, `--step`）的补全分支。

**Patterns to follow:**
- 现有 `presets/schemas/ce-executor-serial.yml` 结构
- 现有 `_ralph_task_args` 补全风格

**Test scenarios:**
- `ralph preset check -H builtin:merge-loop --strict` 绿
- `ralph preset check -H builtin:merge-batch --strict` 绿
- `zsh -n scripts/ralph-zsh-plugin.zsh` 通过
- 手动 smoke：`ralph tools task verify <TAB>` 列出子命令

**Verification:**
- `./scripts/validate-builtin-presets.sh --strict` 绿
- zsh 语法检查通过

---

- [ ] **U11: 最终验证与文档同步**

**Goal:** 确保所有修复不引入漂移，全量测试通过。

**Requirements:** 全部 R1–R11

**Dependencies:** U1–U10

**Files：** 无新增代码文件；可能修改：
- `CLAUDE.md`（若 preset 列表/描述变化）
- `AGENTS.md`（与 CLAUDE.md 同步）

**Approach：**
- 跑 `./scripts/run-tests.sh`（含 nextest + doctest）
- 跑 `scripts/check-cli-doc-drift.sh`
- 跑 `./scripts/validate-builtin-presets.sh --strict`
- 跑 `cargo nextest run -p ralph-core --features supervisor-db --test scenarios ce_executor_supervisor`
- 若 preset 列表或描述有变，执行 `cp CLAUDE.md AGENTS.md`

**Test scenarios：**
- 全 workspace nextest 绿
- doctest 绿
- preset_lint strict 绿
- validate-builtin-presets --strict 绿
- check-cli-doc-drift 绿

**Verification：**
- 上述所有检查通过后方可标记本 plan `status: completed`

---

## System-Wide Impact

- **Interaction graph:**
  - `ralph tools task close` → `emit_close_completion_warning` → resolved hat-channel → stderr JSON
  - `ralph emit` / `ralph wave emit` → `policy_check` → origin guard → rejection/writing
  - `event_loop` budget admission → carve-out branch → admit exempt topic
  - `event_loop` recovery → `validate_resume_routing` → block/allow
  - `ralph inspect loop` → `emit_path::resolve_hat_channel_file` + supervisor summary
- **Error propagation:**
  - U7 warning为 warn-only，不改 exit code；U13/U16 的 block 通过 diagnostic event 传播
  - wave verify 的 origin guard  rejection 使用与 runtime 同源 error 结构
- **State lifecycle risks:**
  - `resolve_hat_channel_file` 必须只读，不写 marker
  - `task verify` 保持零写盘
  - 不引入跨调用 verify token 状态文件
- **API surface parity：**
  - `wave verify` 输出形状变更属于接口契约，需同步 `ralph-tools-wave.md`
  - `inspect loop` JSON 增加/修正 `hat_channel_file` 字段
- **Integration coverage：**
  - U9 BDD 覆盖 fix-unit chain、serial walk、supervisor fan-out/fan-in、macro-edge hint
- **Unchanged invariants：**
  - 人类 CLI bypass + warning 策略不变
  - coordinator 模式主路径不变
  - `HandoffTracker` / session `handoff.md` / `step_handoff` gate 不变

---

## Risks & Dependencies

| Risk | Mitigation |
|------|------------|
| U1 共享 helper 改动影响 `events.rs` 现有行为 | 迁移 `events.rs` 到 helper 后跑 events 单测 + U6 BDD round-trip |
| U13 carve-out 修后可能过宽 | 限制同一 activation 仅一条 exempt topic；BDD 断言重复仍 drop |
| U16 resume block 可能破坏现有 recovery | 仅对 mismatch 触发 block，fallback no-events 路径保留 |
| `wave verify` 输出形状变更影响外部调用 | 同步 skill doc 与 BDD mock_responses |
| `ce-executor-supervisor` event_policy 收紧导致现有 loop 行为变 | 该 preset 下 agent 路径本就应 fail-closed；人类可 bypass |
| U7 删除 `HatIdentitySnapshot.completion_publishes` 影响 inspect JSON | inspect 如需要 completion-class 改为现场计算 |
| merge preset schema 新建导致 `--strict` 新失败 | 与 preset_lint 同步迭代，确保最终 `--strict` 绿 |

---

## Documentation / Operational Notes

- 更新 `ralph-tools-wave.md` 中 `wave verify` 输出示例与 OPAC 章节
- 更新 `ralph-tools-opac.md` Observe 章节（`inspect loop` supervisor 摘要）
- zsh 补全安装：`cp scripts/ralph-zsh-plugin.zsh ~/.oh-my-zsh/plugins/ralph/ralph.plugin.zsh`
- 不手改 `.ralph/` 运行时文件

---

## Sources & References

- **Origin plan:** `docs/plans/2026-07-04-001-feat-opac-isolated-agent-discipline-plan.md`
- **对抗性审查报告：** 本会话生成的 OPAC 目标达成度与逻辑一致性审查报告（P0/P1/P2 分级）
- **Related requirements:**
  - `docs/brainstorms/2026-06-18-isolated-hat-handoff-requirements.md`
  - `docs/brainstorms/2026-07-03-supervisor-rusqlite-parallel-preset-requirements.md`
- **Related plans:**
  - `docs/plans/2026-07-03-001-feat-supervisor-rusqlite-parallel-preset-plan.md`
- **Institutional learnings:**
  - `docs/solutions/integration-issues/ce-executor-serial-precheck-recovery-alignment-2026-06-17.md`
  - `docs/achieved/brainstorms/2026-05-31-agent-operation-guard-requirements.md`
