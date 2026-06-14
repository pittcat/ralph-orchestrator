---
date: 2026-06-14
topic: ce-executor-isolated-agent-output-governance
---

# ce-executor-isolated 运行产物治理机制 — 需求文档

## Summary

针对 `ce-executor-isolated` preset 下反复出现的四类运行期失稳问题，建立一套统一的 **Agent Output Governance** 机制，让 Ralph runtime 在 agent 犯错时仍能兜底，而不是依赖 prompt 或事后人工 reset。

四类问题：

1. **review-synthesizer 不发射**：agent 自己数 wave 事件容易数错/漏数，导致 loop 饿死在 review 汇总阶段。
2. **源码树被 ephemeral 文件污染**：agent 把 `scratchpad.md` / notes 写到 `crates/` 等源码目录，触发无意义 review wave 和 P0 finding。
3. **CLI 写盘前不 enforce `topic_deny_rules`**：`executor` 乱发 `build.done` / `debug.step` 等违规 topic 仍能落盘。
4. **coordinator 预创建未来 task 并标 failed**：plan 把多 U 塞进一个 Step 时，runtime 默认按 Step 批量建 task，导致 U2–U4.5 任务状态错误。

本需求坚持 **机制优先于编排补丁**：不新增 preset 提示词 workaround，而是在 CLI 写入、runtime 注入、产物隔离、task 生命周期四个卡点上加硬规则。

---

## Problem Frame

### 现场证据

- Worktree `2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-calm-oak`：
  - U1 scaffold 已成功（commit `12b0f6f`，4019 pass / 44 pre-existing fails 与基线一致）。
  - review wave round-1 / round-2 已返回 7 维度 0 P0/P1，但 `review-synthesizer` 未发射任何终端事件。
  - 触发 round-3 wave 的唯一原因是 `crates/ralph-core/scratchpad.md`（agent 运行时笔记）作为 untracked 文件存在。
  - loop 最终 `consecutive_failures` 终止。
- 同一 worktree 的 events 文件显示 `executor` 多次发射 `build.done` / `debug.step`，违反 preset `topic_deny_rules`。
- `.ralph/agent/tasks.jsonl` 显示 coordinator 创建了 U2–U4.5 任务并秒级标为 `failed`。

### 根因定性

| 症状 | 直接诱因 | 机制缺失 |
|---|---|---|
| synthesizer 不发射 | agent 算不清应收/已收 dimension 数量 | runtime 未注入 wave 元数据 |
| scratchpad.md 进源码树 | agent 把笔记写到错误位置 | 无 ephemeral 文件隔离/自动清理机制 |
| build.done 落盘 | agent 不遵守 topic deny | CLI policy precheck 未真正 enforce deny rules |
| U2–U4.5 task 预失败 | plan 把 U1–U7 塞成一个 Step | coordinator 无"仅创建当前 U task"契约 |

---

## Key Decisions

- **不依赖 prompt 修点**：不在 preset 里加「数 wave 要小心」「不要把 scratchpad 写到 crates」之类提示词。
- **CLI 写入前兜底**：`ralph emit` / `ralph wave emit` 落盘前必须完成 policy 校验，包含 `topic_deny_rules` 与 schema required_fields。
- **Runtime 主动喂数据**：review-synthesizer 激活时，runner 必须把当前 wave 的 `wave_id` / `wave_total` / `received_count` / `missing_dimensions` 注入 prompt/env，不让 agent 自己算。
- **Ephemeral 自动隔离**：agent 在源码树创建 `scratchpad.md` / `notes.md` / `tmp*.md` 等运行时产物时，runtime 自动移到 `.ralph/agent/scratchpad.md` 或等价隔离区，并在 prompt 里说明原路径已迁移。
- **Task 创建按当前 U**：coordinator 必须保证一个 iteration 只创建当前正在执行的 Implementation Unit 对应的 runtime task；plan 若把多 U 写在一个 Step，由 coordinator 按 U 拆分或明确标记为 multi-U Step。

---

## Requirements

### R1. Runtime 向 review-synthesizer 注入 wave 上下文

- R1.1 当 `review-synthesizer` 被激活时，runner 必须构造当前 wave 的元数据：
  - `wave_id`
  - `wave_total`（来自 `review.wave.ready` 事件）
  - `received_count`（已收到的 `review.dimension.done` 数量，同 `wave_id`）
  - `missing_dimensions`（期望维度列表减去已收到维度列表）
  - `expected_dimensions`（来自 wave payload 的 dimension 列表）
- R1.2 上述元数据必须以结构化方式注入，优先顺序：
  1. 环境变量 `RALPH_WAVE_CONTEXT`（JSON 字符串）
  2. prompt 顶部固定格式块 `## WAVE CONTEXT`
- R1.3 当 `received_count == wave_total` 时，runner 必须在 prompt 中显式标注 `"ALL_DIMENSIONS_RECEIVED": true`，agent 只需做合并与 verdict，无需再计数。
- R1.4 当 aggregate timeout 触发且仍有 missing dimensions 时，runner 必须标注 `"AGGREGATE_TIMEOUT": true` 并列出缺失维度，agent 据此走 `plan.blocked` 路径。
- R1.5 注入内容必须精确到当前激活的 wave，不得把历史 wave 的 dimension done 事件混进来。

### R2. CLI 写入前强制 policy 校验（含 topic_deny）

- R2.1 `ralph emit` 与 `ralph wave emit` 必须在写入 `events.jsonl` 之前执行与 active preset / `ralph.yml` 一致的 event policy 校验。
- R2.2 校验必须包含：
  - `required_fields` 与 `payload: json_object`
  - `topic_deny_rules`（精确 hat_id + topic 匹配）
  - `plan_name_equality_required`
  - `allowed_values`（如 `skip_reason` 枚举）
- R2.3 当 preset 设置 `require_policy_check_for_cli_emit: true` 且 `allow_unsafe_cli_emit: false` 时，`--no-policy-check` 必须失败或忽略。
- R2.4 校验失败时 CLI 非零退出，**不得写入任何事件行**。
- R2.5 `--output json` 时必须输出结构化错误，包含 `payload_index`、缺失字段、`reason_code`，支持批量收集全部 violations 后一次性输出。
- R2.6 wave emit 批量校验时，必须扫描全部 payload 后再报错，禁止只报第一个错误。
- R2.7 预检实现必须复用 `crates/ralph-cli/src/policy_check.rs` 或 `ralph_core::validate_event` 等价逻辑，不得维护第二套规则。

### R3. Ephemeral 文件隔离机制

- R3.1 runtime 必须维护一份 **ephemeral file allowlist**：
  - 允许写入：`.ralph/agent/scratchpad.md`、`.agents/scratchpad/`、`/tmp/`、`/var/tmp/` 等显式运行时区域
  - 禁止落入源码树：`crates/`、`src/`、`backend/`、`frontend/` 等源码目录下的 `scratchpad.md`、`notes.md`、`tmp*.md`、`*.tmp.md`、`.agent-notes.md`
- R3.2 当 agent 在源码树创建 ephemeral 模式文件时，runtime 必须：
  1. 自动将文件内容追加到 `.ralph/agent/scratchpad.md`（或按 loop_id 分片）
  2. 删除源码树中的原文件
  3. 在下一轮 prompt 顶部注入 `"EPHEMERAL_RELOCATED": ["crates/ralph-core/scratchpad.md → .ralph/agent/scratchpad.md"]`
- R3.3 review-coordinator 触发 wave 前，runner 必须执行 `git ls-files --others --exclude-standard`；若结果仅包含 R3.1 定义的 ephemeral 模式文件，则：
  - 自动清理这些文件
  - 走 `review.passed` with `skip_reason: "empty_diff"`（因为无实质代码变更）
  - **不得**为这些文件单独发 review wave
- R3.4 若源码树出现新的 untracked 非 ephemeral 文件（如 agent 意外生成的新 `.rs`），按现有 wave 规则正常 review。
- R3.5 该机制对 `ce-executor-isolated` 默认启用；其他 preset 可通过 `event_loop.ephemeral_isolation: false` 关闭。

### R4. Coordinator task 创建契约

- R4.1 coordinator 必须遵守 **"当前 U 原则"**：一个 iteration 只创建与当前 `work.ready` 对应的 Implementation Unit 的 runtime task。
- R4.2 当 plan.md 在一个 Step 内列出多个 Implementation Units（U1, U2, U3...）时，coordinator 必须：
  - 识别当前 Step 内首个未完成的 U
  - 仅创建该 U 的 runtime task
  - 不得预创建后续 U 的 task
- R4.3 plan-gate 推进到下一 U 时，由**新的 coordinator 激活**负责创建下一 U task（复用现有 `work.start` / `work.ready` 流程）。
- R4.4 若 preset 希望支持 "单 Step 多 U" 模式，必须在 `event_loop` 配置中显式声明 `multi_unit_step: true`，否则 coordinator 按默认单 U 处理。
- R4.5 `ralph tools task ensure` 必须对同一 `(loop_id, plan_name, step, unit)` 幂等；重复 ensure 不得创建重复 task 或把已关闭 task 重新打开。
- R4.6 runtime 在 coordinator 创建 task 后，必须校验 task 状态：
  - 若同一 step/unit 已存在 failed/closed task，不得直接复用
  - 应创建新 task 或走 `plan.blocked` 让 agent 决定

### R5. Hard gate 后 hat 路由稳定性

- R5.1 当 `missing_event_gate` 因某 hat H 触发时，注入的 `task.resume` 必须使下一轮激活 hat = H，不得漂到 `tasks.coordinator_hats` 首项。
- R5.2 当 H = `review-synthesizer` 时，必须同时注入 R1 的 wave 上下文，避免 agent 反复被同样信息卡住。
- R5.3 当 H = `review-coordinator` 且 wave 事件被 policy 拒绝时，按 `docs/brainstorms/2026-06-13-wave-dispatch-policy-gate-requirements.md` 的 R4–R7 处理，不得把 resume 路由到 `executor`。

---

## Key Flows

### F1. 正常 review wave 通过

- review-coordinator emit 7 个合规 `review.wave.ready` → CLI 预检通过 → 写入 jsonl → wave partition → policy 通过 → spawn 7 worker → 7 `review.dimension.done` → synthesizer 激活时拿到 `wave_total=7, received=7, ALL_DIMENSIONS_RECEIVED=true` → emit `review.passed`。

### F2. synthesizer 自动获得 wave 上下文

- 7 维度返回后，runner 构造 `RALPH_WAVE_CONTEXT` 注入 synthesizer prompt。
- agent 不再需要数 `events.jsonl` 行数，直接基于注入信息做 verdict。

### F3. agent 把 scratchpad 写到 crates/

- agent 写入 `crates/ralph-core/scratchpad.md`。
- runtime 检测到 ephemeral 模式命中，将内容追加到 `.ralph/agent/scratchpad.md`，删除原文件。
- 下一轮 prompt 告诉 agent "已迁移"，避免重复写入。
- review-coordinator 触发 wave 前发现无实质 untracked 文件，直接 `review.passed skip_reason=empty_diff`。

### F4. CLI 拦截违规 topic

- executor 调用 `ralph emit build.done`。
- CLI 在写入前匹配 `topic_deny_rules`（`hat_id: executor, topic: build.done`）。
- 非零退出，stderr 显示 `topic 'build.done' is denied for hat 'executor'`，events jsonl 行数不变。

### F5. 单 Step 多 U plan 的正确 task 创建

- plan.md Step 1 含 U1–U7。
- coordinator 第一次只创建 U1 task，emit `work.ready`。
- executor 完成 U1 → review 通过 → plan-gate 推进到 U2 → coordinator 再次被激活 → 创建 U2 task → emit `work.ready`。
- U2–U7 不会在 U1 开始前被预创建。

---

## Acceptance Examples

### AE1. Synthesizer 拿到 wave 上下文（R1）

- **Given**: 7 维度 `review.dimension.done` 全部返回，wave_id = `w-abc`。
- **When**: runner 激活 `review-synthesizer`。
- **Then**: prompt 中必须包含 `"wave_id": "w-abc"`, `"wave_total": 7`, `"received_count": 7`, `"ALL_DIMENSIONS_RECEIVED": true`。

### AE2. CLI 拦截 build.done（R2）

- **Given**: `ce-executor-isolated` preset，当前 hat = `executor`。
- **When**: `ralph emit build.done --json '{"ok":true}'`。
- **Then**: exit code ≠ 0；stderr 含 `topic 'build.done' is denied for hat 'executor'`；events jsonl 无新增行。

### AE3. scratchpad 自动迁移（R3）

- **Given**: agent 写入 `crates/ralph-core/scratchpad.md`，内容 `"## Notes\nfoo\n"`。
- **When**: runtime 检测到该文件。
- **Then**: 原文件被删除；`.ralph/agent/scratchpad.md` 追加该内容；下一轮 prompt 含 `"EPHEMERAL_RELOCATED"`。

### AE4. 无实质变更时跳过 wave（R3）

- **Given**: commit_count=0, changed_lines=0，且唯一 untracked 文件是 `crates/ralph-core/scratchpad.md`。
- **When**: review-coordinator 触发。
- **Then**: runtime 清理 scratchpad.md 后，coordinator emit `review.passed` with `skip_reason: "empty_diff"`，不发 wave。

### AE5. Coordinator 不预创建未来 task（R4）

- **Given**: plan.md Step 1 含 U1–U7，当前执行 U1。
- **When**: coordinator 创建 task。
- **Then**: 仅存在 U1 的 runtime task；U2–U7 task 不存在或状态不为 failed。

---

## Success Criteria

- SC1. `ce-executor-isolated` 下合规 review wave 能稳定完成，synthesizer 不再因"数不清 dimension"而不发射。
- SC2. `ralph emit` / `ralph wave emit` 对 `topic_deny_rules` 和 schema 违规 fail-fast，违规事件不再落盘。
- SC3. agent 写到源码树的 `scratchpad.md` 类文件被自动隔离，不再触发无意义 review wave。
- SC4. coordinator 不再预创建未来 U 的 task 并标 failed。
- SC5. 新增/修改测试覆盖 R1–R4 核心路径；`cargo nextest run --workspace --exclude ralph-e2e` 通过。
- SC6. 相关 skill 文档 `crates/ralph-core/data/ralph-tools.md` 同步更新，并做 `--help` 冒烟。

---

## Scope Boundaries

### Deferred for later

- `ralph emit` / `ralph wave emit` 独立 `dry-run` 子命令。
- dimension-reviewer worker 启动时写 `worker.started` 遥测事件。
- 对 non-isolated / coordinator 模式的深度优化（保持 regression 覆盖即可）。
- 自动修复已存在的旧 worktree 中的 failed tasks。

### Outside this product's identity

- 放宽 `review.wave.ready` schema 来掩盖 agent 错误。
- 为单次 incident 增加 preset/loop_id 豁免。
- 在 orchestrator 内实现 agent 级 poll/wait 状态机替代机制。

---

## Dependencies / Assumptions

- 依赖 `docs/brainstorms/2026-06-13-wave-dispatch-policy-gate-requirements.md` 中 R1–R7、R12–R22 已规划或同步实现。
- 依赖现有 `crates/ralph-cli/src/policy_check.rs`、`crates/ralph-cli/src/commands/emit.rs`、`crates/ralph-cli/src/wave.rs` 作为 CLI 预检入口。
- 依赖 `crates/ralph-core/src/event_loop/mod.rs` 的 wave partition / policy 校验逻辑。
- 假设 `ce-executor-isolated` 的 `event_policy.mode: enforce` 与 `on_violation: reject_with_resume` 保持不变。

---

## Outstanding Questions

### Deferred to Planning

- Q1: `RALPH_WAVE_CONTEXT` 以环境变量还是 prompt 块注入？还是两者都提供？
- Q2: ephemeral 文件模式列表是否可配置，还是 hardcoded 在 runtime？
- Q3: coordinator 识别 "当前 U" 时，是解析 plan.md 的 `### N. U<N>:` 标题，还是依赖 task_key 中的 `uN-` 前缀？
- Q4: 当 plan 明确是 "单 Step 多 U" 时，是否引入新的 `event_loop.multi_unit_step: true` 配置，还是通过 preset 覆盖 coordinator 行为？
- Q5: topic_deny 与 schema 违规的 CLI 错误输出是否需要统一错误码体系？

---

## Sources / Research

- 本次现场 worktree：`.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-calm-oak/.ralph/`
- 现场 preset：`presets/en/ce-executor-isolated.yml`
- 相关 brainstorm：`docs/brainstorms/2026-06-13-wave-dispatch-policy-gate-requirements.md`
- 相关源码：
  - `crates/ralph-cli/src/policy_check.rs`
  - `crates/ralph-cli/src/commands/emit.rs`
  - `crates/ralph-cli/src/wave.rs`
  - `crates/ralph-cli/src/loop_runner/runner.rs`
  - `crates/ralph-cli/src/loop_runner/hard_gate.rs`
  - `crates/ralph-core/src/event_loop/mod.rs`
  - `crates/ralph-core/src/event_policy.rs`
  - `crates/ralph-core/src/hatless_ralph.rs`
  - `crates/ralph-core/src/task_store.rs`
