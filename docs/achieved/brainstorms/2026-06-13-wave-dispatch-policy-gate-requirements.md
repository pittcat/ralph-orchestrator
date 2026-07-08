---
date: 2026-06-13
topic: wave-dispatch-policy-gate
---

# Wave 派发与 Policy Gate 脱节 — 需求文档

## Summary

修复 Ralph isolated 模式下 **wave 事件被 event policy 静默拒绝** 后，runner 仍误判「hat 未 emit」并触发 `missing_event_gate`、错误路由、loop 提前终止的问题。同时在 `ralph wave emit` 写入前增加 schema 预检，并在 preset 示例中补齐 `depth` 字段。本需求以**机制修复为主**，preset 改动保持最小。

---

## Problem Frame

2026-06-12 的 worktree loop（`ce-executor-isolated`，U1 scaffold step）中，用户观察到 TUI 未显示 7 个 `dimension-reviewer` worker 并发 review，loop 最终以 `payload_contract_violation` 终止。

现场证据（`docs/report/2026-06-13-review-wave-no-spawn.md`）与源码复核表明：

1. `review-coordinator` **确实**通过 `ralph wave emit` 写入了 7 个合规形态的 `review.wave.ready`（共享 `wave_id`、`wave_index` 0..6、`wave_total=7`）。
2. 7 个 payload **缺少** preset schema 要求的 `depth` 字段（`presets/en/ce-executor-isolated.yml` `event_policy.schemas.review.wave.ready.required_fields`）。
3. `process_events_from_jsonl_with_waves` 在 wave 分区路径上对 wave 事件做 policy 校验；缺失 `depth` 导致 `RejectWithResume`，**7 个事件全部从 `wave_events` 剔除**，`handle_wave_events` 不执行，0 worker spawn。
4. Runner 的 `missing_event_gate` 仅检查 `processed_events`（regular 路径）的 `candidate_topics`，**不包含**被 policy 拒绝的 wave 事件 → 误判「review-coordinator 本轮未 emit」→ 触发 hard gate。
5. 后续 iteration 中 hat 路由偏离（executor 被激活并 emit 裸字符串 `work.failed`）→ `payload_contract_violation` 终止 loop。

**根因定性**：触发条件是 agent 编排失误（缺 `depth`），但 **loop 机制在 policy 拒绝后缺乏可观测性与正确恢复路径**，属于机制缺陷。在 wave 路径上「缝补 preset 轮询指令」无法解决「写了但派发失败」类问题。

---

## Key Decisions

- **机制优先于编排补丁**：不采用「emit 后 poll `review.dimension.done`」作为 P0 修复；该指令在 worker 未 spawn 时无效，且掩盖 runner 盲区。
- **写入前失败优于写入后静默拒绝**：`ralph wave emit` 应在落盘前做与 loop 一致的 schema 校验（与 `ralph emit` policy check 对齐），避免 agent 误以为 emit 成功。
- **Agent-native 结构化错误**：预检失败时除 stderr 人类可读消息外，`--output json` 必须输出可机器解析的 `validation_errors[]`（含 payload 索引、字段名、reason_code），供 agent 程序化修 payload，而非反复试错。
- **Preset 强制 CLI policy check**：`ce-executor-isolated` 等在 preset 层显式 `require_policy_check_for_cli_emit: true` + `allow_unsafe_cli_emit: false`，关闭「形态合法但 schema 违规仍落盘」的默认后门。
- **obligation 判定必须看见 wave 路径的 reject**：`missing_event_gate` 与 `should_gate_missing_events` 必须把「agent 尝试 emit 但被 policy 拒绝」视为已尝试，而非「完全未 emit」。
- **hard gate 后的 resume 必须回到被 gate 的 hat**：`task.resume` / 下一 iteration 的激活 hat 不得因 `coordinator_hats` 默认顺序漂到 `executor`。
- **preset 最小改动**：仅补全示例 payload 的 `depth` 字段与文档说明；不扩展 review-coordinator 的等待/轮询逻辑。

---

## Requirements

### Wave policy 拒绝可观测性

- R1. 当 `process_events_from_jsonl_with_waves` 中 wave 分区事件的 policy 校验产生 `RejectWithResume` / `Hold` / `Block` 时，runner **必须**将拒绝详情（topic、hat、reason_code、缺失字段名）写入 `recovery.jsonl`，`source` 为 `payload_contract` 或专用 `wave_policy_rejection`（二选一，plan 阶段定名），`severity` ≥ `warning`。
- R2. 上述拒绝 **必须**在 diagnostics log 中留下可检索行（含 `wave_id`、`topic`、拒绝原因），不得静默 drop。
- R3. `ProcessedEventsWithWaves`（或 runner 等价结构）**必须**向 runner 暴露 wave 路径的 `policy_rejections`，与 regular 路径的 `ProcessedEvents.had_rejected_events` 语义对齐。

### missing_event_gate 与 obligation 判定

- R4. 当本轮 wave 路径存在 policy 拒绝，且拒绝的 topic 属于当前 hat 的 publish obligation（如 `review.wave.ready`）时，`missing_event_gate` **不得**触发。
- R5. `should_gate_missing_events` 的 `candidate_topics` **必须**合并 wave policy 拒绝的 topic（含 contract-rejected 与 policy-rejected），与 regular 路径行为一致（参见 `crates/ralph-cli/src/loop_runner/tests.rs` `test_contract_rejection_satisfies_any_valid_or_rejected` 先例）。
- R6. 当 wave 事件在 jsonl 中存在、通过 partition 识别为 wave dispatch、但被 policy 全部拒绝时，runner **必须**注入面向 agent 的 recovery guidance（`human.guidance` 或 `task.resume` payload），说明具体 schema 违规（如 `Missing required field: depth`），而非 generic「did not emit any event」。

### Wave 派发失败诊断

- R7. 当 jsonl 新读批次中存在带 `wave_id` 且 target hat `concurrency > 1` 的原始事件，但 post-policy `wave_events` 为空时，runner **必须**写 recovery envelope，`reason_code` 为 `wave_dispatch_blocked`（或等价命名），evidence 含原始事件数、拒绝原因摘要、期望 target hat。
- R8. 当 post-policy `wave_events` 非空但 `handle_wave_events` 后 0 worker 启动（无 `wave-{id}-{idx}.jsonl` 且 detection rejected），现有 `handle_wave_rejection` 路径 **必须**保持可用；本需求不重复实现 dispatcher 内部逻辑，但 R1–R7 须覆盖「policy 在 dispatch 前清空 wave_events」这一现场路径。

### Hard gate 后 hat 路由

- R9. `inject_missing_event_hard_gate_guidance` 触发的 recovery envelope 中，`target_hat` **必须**为被 gate 的 hat（如 `review-coordinator`），且 `safe_target=true` 时注入的 `task.resume` **必须**使下一 iteration 激活同一 hat，而非 `tasks.coordinator_hats` 列表首项。
- R10. Handoff dispatch timeout（`HandoffTracker::expired`）的 `safe_target` 逻辑 **不得**在「consumer 为 review-coordinator 且 escalation 来自 missing_event_gate」场景下将 resume 路由到 `executor`。
- R11. 当 hard gate 针对 hat H 触发时，`RALPH_CURRENT_HAT`（或 isolated 模式等价物）在 H 的 recovery iteration **必须**为 H，直至 H 再次 emit 合法 obligation 事件或 escalation 明确移交。

### ralph wave emit 写入前 schema 预检

- R12. `ralph wave emit` 对每个 payload 在写入 `events.jsonl` **之前**执行与 active preset / `ralph.yml` 一致的 event policy schema 校验（至少 `required_fields` + `payload: json_object`）；校验失败时 CLI **非零退出**，stderr 输出缺失字段与 fix hint，**不写入任何 wave 行**。
- R13. 预检 **必须**复用 `ralph_core::validate_event`（或与其等价的共享函数），不得维护第二套 schema 规则。
- R14. 当 workspace 无 `ralph.yml` 或 `event_policy.enabled=false` 时，预检降级为现有 `validate_payload_shape`（JSON object 形态校验）；行为须在 `--help` 或 skill 文档中说明。
- R15. 预检与 `ralph emit` 的 `require_policy_check_for_cli_emit` 配置 **对齐**：preset 启用 strict CLI policy check 时，wave emit 默认 enforce；允许 `--no-policy-check` 仅当 config 显式 `allow_unsafe_cli_emit`。

### Preset 最小编排对齐

- R16. `presets/en/ce-executor-isolated.yml` 中 review-coordinator 的 wave payload 示例与 `required_fields` 列表 **必须**包含 `depth` 字段及合法示例值（`quick` | `standard` | `deep`）。
- R17. **不得**在 preset 中新增「emit 后 poll dimension.done / 5 分钟等待」类 P0 指令作为机制替代的 workaround。

### 文档与工具引用

- R18. 若 wave emit 新增 CLI 标志或改变退出码语义，**必须**同步 `crates/ralph-core/data/ralph-tools.md`（及 wave skill 若引用）并做 `ralph wave emit --help` 冒烟。

### Agent-native payload 校验（L1 强化）

- R19. `ralph wave emit` 在 schema 预检失败且 `--output json` 时，**必须**向 stdout 输出结构化 JSON（非零 exit code），至少包含：`ok: false`、`error: "policy_validation_failed"`、`topic`、`validation_errors` 数组；数组每项含 `payload_index`（0-based）、`field`（可空）、`reason_code`（如 `missing_required_field`）、`message`。
- R20. 批量 emit（N 个 payload）预检时，**必须**收集**全部** payload 的 violations 后一次性输出，禁止「只报第一个错误、agent 修完再发现下一个」的串行试错（尤其 N=7 的 review wave）。
- R21. `presets/en/ce-executor-isolated.yml` 的 `event_policy` **必须**显式设置 `require_policy_check_for_cli_emit: true` 与 `allow_unsafe_cli_emit: false`，使 `ralph wave emit` / `ralph emit` 在 loop 使用的同一 preset 语义下默认 enforce schema；`--no-policy-check` 在该 preset 下 **必须**失败或忽略（与 `ralph emit` strict 模式一致）。
- R22. 预检失败时 stderr **仍须**保留单行人类可读摘要（首个错误或「N violations in M payloads」）；stdout JSON 与 stderr 摘要 **必须**指向同一批 violations，不得矛盾。

---

## Key Flows

- F1. **合规 wave 派发（回归路径）**
  - **Trigger:** review-coordinator emit 7 个含 `depth` 的 `review.wave.ready`
  - **Actors:** review-coordinator agent、loop runner、dimension-reviewer × 7
  - **Steps:** wave emit 预检通过 → 写入 jsonl → partition → policy 通过 → `handle_wave_events` spawn 7 worker → 各 worker emit `review.dimension.done`
  - **Outcome:** TUI 显示 7 worker；无 `missing_event` recovery envelope

- F2. **缺字段 wave emit（CLI 预检拦截）**
  - **Trigger:** agent 调用 `ralph wave emit review.wave.ready` 且 payload 缺 `depth`
  - **Actors:** review-coordinator agent、ralph CLI
  - **Steps:** 预检 → `Missing required field: depth` → exit ≠ 0 → jsonl 无新增行
  - **Outcome:** agent 修正 payload 后重试；loop 未进入误 gate 状态

- F5. **Agent 程序化修 payload（JSON 结构化错误）**
  - **Trigger:** agent 使用 `ralph wave emit ... --output json --payloads-stdin`，7 条中多条缺 `depth`
  - **Actors:** review-coordinator agent、ralph CLI
  - **Steps:** 预检扫描全部 7 条 → stdout 输出 `validation_errors`（含各 `payload_index` 与 `field: "depth"`）→ exit ≠ 0 → 无写盘
  - **Outcome:** agent 一次调用即知全部缺字段位置，补全后第二次 emit 成功

- F3. **Policy 拒绝后不误触 missing_event_gate（机制路径）**
  - **Trigger:** 历史 run 或 `--no-policy-check` 绕过预检，缺 `depth` 事件已落盘
  - **Actors:** loop runner、review-coordinator
  - **Steps:** read jsonl → wave partition → policy reject 全部 7 条 → R1 recovery 写入 → candidate_topics 含 `review.wave.ready` → **不**触发 missing_event_gate → guidance 说明 schema 错误 → 下一 iteration 仍激活 review-coordinator
  - **Outcome:** loop 不终止于 `payload_contract_violation`；agent 有机会补发

- F4. **Hard gate 后 hat 稳定（路由路径）**
  - **Trigger:** 非 wave 场景下 review-coordinator 真正未 emit（obligation 未满足）
  - **Actors:** loop runner、review-coordinator
  - **Steps:** missing_event_gate → hard gate guidance → task.resume → **下一 iteration hat = review-coordinator**
  - **Outcome:** executor 不被误激活代发 review 终态

---

## Acceptance Examples

- AE1. **Covers R12, R16, F2**
  - **Given:** preset `ce-executor-isolated`，payload 缺 `depth`
  - **When:** `ralph wave emit review.wave.ready --payloads-stdin` 传入 7 行 JSONL
  - **Then:** CLI exit ≠ 0；stderr 含 `depth`；events jsonl 行数不变

- AE6. **Covers R19, R20, R22, F5**
  - **Given:** `ce-executor-isolated` preset 已启用 `require_policy_check_for_cli_emit: true`
  - **When:** `ralph wave emit review.wave.ready --output json --payloads-stdin`，7 条均缺 `depth`
  - **Then:** stdout JSON 含 `ok: false` 与 `validation_errors` 长度 7；每项 `payload_index` 0..6、`field`=`depth`；jsonl 无新增行；`--no-policy-check` 不能绕过

- AE2. **Covers R1, R4, R6, R7, F3**
  - **Given:** jsonl 中已有 7 条缺 `depth` 的 `review.wave.ready`（模拟历史落盘）
  - **When:** loop runner 执行下一 iteration `process_events_from_jsonl_with_waves`
  - **Then:** `wave_events` 为空；`recovery.jsonl` 含 wave/policy 拒绝记录；**无** `missing_event_gate` + `source_hat=review-coordinator`；human-facing guidance 提及 `depth`

- AE3. **Covers R9, R11, F4**
  - **Given:** review-coordinator obligation 未满足（无 emit、无 wave reject 掩盖）
  - **When:** missing_event_gate 触发
  - **Then:** 随后 iteration 的激活 hat 为 `review-coordinator`；executor 的 `RALPH_CURRENT_HAT` 不为 review 阶段代写者

- AE4. **Covers F1, R8（回归）**
  - **Given:** 7 条合法 payload（含 `depth: "standard"`）
  - **When:** 完整 review wave 迭代
  - **Then:** `.ralph/wave-*.jsonl` ≥ 7；events 含 7× `review.dimension.done`；TUI/log 含 wave worker 启动信息

- AE5. **Covers R5（单元级）**
  - **Given:** `ProcessedEvents` 模拟 wave policy rejection 合并后 `had_rejected_events=true`，obligation topic 在 rejection 列表
  - **When:** runner 计算 `should_gate_missing_events`
  - **Then:** 返回 false

---

## Success Criteria

- SC1. 复现 worktree 场景（缺 `depth`）时，loop **不再**因 `missing_event_gate` 误触发而 cascade 到 `payload_contract_violation`。
- SC2. 合规 7-dimension wave 在 isolated + `ce-executor-isolated` 下 **稳定 spawn** 7 worker（与 2026-06-09 wave batching fix 后预期一致）。
- SC3. `ralph wave emit` 在 schema 违规时 **fail-fast**，agent 在 CLI 层即可修正，无需读 recovery 才知晓。
- SC4. 新增/修改测试覆盖 R4–R7、R12 核心路径；`cargo test` workspace（exclude ralph-e2e）通过。
- SC5. Agent 使用 `--output json` 时，**单次**预检失败响应即可定位全部违规 payload 索引与字段（R19–R20），无需 7 轮试错。
- SC6. `ce-executor-isolated` 运行时 `ralph wave emit` 默认 enforce schema（R21），与 loop `event_policy.mode: enforce` 语义一致。

---

## Scope Boundaries

### Deferred for later

- `ralph emit` / `ralph wave emit` 独立 **dry-run** 子命令（`validate-event`）— 本需求由 L1 预检 + JSON 错误覆盖主路径，dry-run 可作为 follow-up
- scratchpad `human.guidance` 去重（报告 P1 噪声项）
- executor 误发 `build.done` 的 agent prompt 强化（已有 `topic_deny_rules`）
- dimension-reviewer worker 启动时写 `worker.started` 遥测事件（可观测性增强，非本需求 P0）
- `enforce_wave_isolated_scope` 时序问题（现场无证据，不纳入本需求）

### Outside this product's identity

- 在 orchestrator 内实现「agent 级 poll/wait dimension.done」状态机
- 放宽 `review.wave.ready` schema（删除 `depth` required）以掩盖 agent 错误
- 为单次 incident 增加 preset 名称或 loop id 豁免

---

## Dependencies / Assumptions

- 假设现场根因链（缺 `depth` → policy reject → gate 误判）已在 `docs/report/2026-06-13-review-wave-no-spawn.md` 与 worktree artifacts 上复核成立。
- 依赖现有 `ralph emit` policy check 实现（`crates/ralph-cli/src/commands/emit.rs`）作为 wave 预检的模式来源。
- 依赖 `ce-executor-isolated` preset 的 `event_policy.mode: enforce` 与 `on_violation: reject_with_resume` 保持不变。
- 假设修复针对 **isolated execution mode** 下的 wave dispatch 路径；coordinator 模式 regression 通过现有 smoke/scenario 覆盖即可。

---

## Outstanding Questions

### Resolved During Planning

- Q1: 是否以 preset 轮询替代机制修复？**否** — 用户明确要求机制问题系统性修，见 Key Decisions。
- Q2: `enforce_wave_isolated_scope` 是否为现场根因？**否** — log/recovery 无 scope violation 证据；不纳入 scope。

### Deferred to Planning

- D1: wave policy rejection 的 recovery `source` 枚举名最终选用 `payload_contract` 还是新增 `wave_policy_gate` — plan 阶段与现有 diagnosis taxonomy 对齐。
- D2: wave emit 预检是否默认开启或跟随 `require_policy_check_for_cli_emit` — plan 阶段读 config 默认值后定案。

---

## Sources / Research

- 现场报告：`docs/report/2026-06-13-review-wave-no-spawn.md`
- Worktree events：`.worktrees/2026-06-10-003-refactor-event-loop-and-loop-runner-tests-split-plan-cheery-hawk/.ralph/events-20260612-161708.jsonl`（incident 索引；实现以 repo 内行为描述为准）
- Wave 分区与 policy：`crates/ralph-core/src/event_loop/mod.rs` `process_events_from_jsonl_with_waves`
- missing_event_gate：`crates/ralph-cli/src/loop_runner/runner.rs`、`hard_gate.rs`
- wave emit 写入：`crates/ralph-cli/src/wave.rs` `write_wave_events_with_provenance`
- emit policy 预检先例：`crates/ralph-cli/src/commands/emit.rs`
- 机构经验：`docs/solutions/integration-issues/ce-executor-wave-emission-must-batch-in-single-emit-2026-06-09.md`
- 机构经验：`docs/solutions/integration-issues/ce-executor-isolated-preset-dispatch-gap-plan-gate-executor-2026-06-12.md`
