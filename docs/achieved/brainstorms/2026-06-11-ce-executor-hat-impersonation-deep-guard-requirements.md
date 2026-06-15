---
title: ce-executor hat impersonation 深度防护需求
date: 2026-06-11
status: brainstorms
scope: deep / feature
preset: ce-executor
---

# ce-executor hat impersonation 深度防护需求

> 摘要：把 hat impersonation（ralph hat 冒充其他 hat 发业务事件）从"修一次复现一次"的恶性状态转为"任何路径下的冒充都会被独立审计且主动暴露"。本需求按机制层→演练层→数据层→架构层→语义层顺序落地，每步可验证、可合并、可独立止损。

## 1. 背景与问题

### 1.1 已发生的事件

2026-06-10 在 ce-executor 编排过程中再次发现 hat impersonation：

- `iter=6` 出现 `hat:review-coordinator + topic:work.done` —— 但 `work.done` 应由 `executor` 发布
- `iter=7` 出现 `hat:coordinator + topic:review.wave.ready` —— `coordinator` 是 ralph 兜底身份，越权发 review 事件
- 用户反馈：TUI 头部显示 "Review Coordinator" 时，agent 实际在跑 `cargo test` 并改代码——视觉与执行不一致

### 1.2 已知修复历史

| Commit | 时间 | 修复内容 |
|---|---|---|
| `e272808` | 2026-06-10 15:06 | `ralph_control_only` origin guard 拒绝 ralph 发业务 topic |
| `524fb73` | 2026-06-10 15:06 | `topic_deny_rules` + `plan_name_equality_required` 锁定 |
| `efc0fd0` | 2026-06-10 | partial wave detection |
| `b51bfa9` | 2026-06-10 19:05 | 4 个 P1 守卫：`ralph emit` CLI 校验、wave spawn fail 写 recovery、task owner 校验、preset 加 `skip_reason` 枚举 + ralph deny |
| `87cc8eb` | 2026-06-10 20:10 | synthesizer 误判修、preset 白名单补 `plan.blocked`、`review.complete` 强制 `findings_count` |

**4 次 P0/P1 fix 落地后仍复发**——证明白名单 deny 模式追不上攻击面扩张。

### 1.3 根本问题

不是单一漏洞，而是**结构性矛盾**：

1. **ce-executor 是 coordinator 模式**——`ralph` 永远 `next_hat()`
2. **白名单 deny 模式**与"ralph 永远激活"的前提**根本矛盾**
3. **回退注入路径**（stall→hard_gate→scratchpad 注入→agent 重发）与**正常 agent 路径**共用 `events.jsonl`，下游消费方无法区分
4. **events.jsonl 的 `hat` 字段语义过载**——"发布者 hat"、"当前激活 hat"、"进程名"三套语义混着用
5. **诊断报告（TUI + drift + recovery）**只看"被冒充后留下的痕迹"，**不看"冒充本身"**

### 1.4 防御目标

任何来源的冒充事件必须满足：

1. **被独立审计**——来源、时机、伪装路径全部可追溯
2. **被主动暴露**——TUI 顶部出现 `⚠️ impersonation n` 计数，`invariant-violation.jsonl` 立即记录
3. **不影响正常编排**——默认不卡 loop（保持 Ralph 哲学）
4. **可严格化**——`strict_mode` 开启时硬停 loop

## 2. 范围与不在范围

### 2.1 范围内（5 层面，按 Approach C 顺序落地）

| 层面 | 落地顺序 | 产物 |
|---|---|---|
| **机制层** | **U1（首个交付）** | `loop_invariant_assertion`：每 iter 末校验本 iter 写入的所有事件的 `source` 字段符合预期；不通过即落 `invariant-violation.jsonl` + TUI 顶部 ⚠️ 计数 |
| **演练层** | U2 | 4 个新 BDD scenario：`ce_executor_impersonation_stall_fallback.yml` / `_human_guidance.yml` / `_robot_response.yml` / `_recovery_journal.yml` |
| **数据层** | U3 | events.jsonl 行增加 `_source` 字段（值：`agent` / `fallback_inject` / `system`）；`ralph emit` CLI 输出同步加 `--source`；下游消费方按 source 决定行为 |
| **架构层** | U4（可选） | 若 U1+U2+U3 仍未堵住，则拆 `events-xxx.jsonl` 为 `agent-events.jsonl` + `fallback-events.jsonl` 两路写入；下游按文件分流 |
| **语义层** | U5 | `EventRecord.hat` 拆为 `hat`（真实执行进程 hat_id，coordinator 模式下恒为 `ralph`）+ `role`（被激活的角色名）；TUI 内部读 `role`，drift 监测读 `hat` |

### 2.2 不在范围

- **TUI 头部显示文本不变**——`🔍 Review Coordinator @kiro` 保持原样（仅下游类型拆 `role` / `process`）
- **不动既有 4 次 P0/P1 fix**——`e272808` / `524fb73` / `b51bfa9` / `87cc8eb` 全部保留作为底层 defense-in-depth
- **不重写事件溯源框架**——不上"Git reflog 式不可变日志"那种全量重写
- **不修改 `recovery.jsonl` / `drift.jsonl` 现有 schema**——只新增 `invariant-violation.jsonl`
- **不修改 `ralph_control_only` / `topic_deny_rules` / `plan_name_equality_required`**——继续作为底层

### 2.3 重点防御对象（5 类高危事件）

| 事件类型 | 危险 | 必须严格 source |
|---|---|---|
| `review.*`（`review.wave.ready` / `review.passed` / `review.complete` / `review.failed` / `review.dimension.done`） | 流水线状态被误改 | source=agent（禁止 ralph 任何渠道发） |
| `plan.*`（`plan.blocked` / `plan.complete`） | loop 终态被误触发 | source=agent |
| `work.*`（`work.done` / `work.failed` / `work.ready` / `work.retry`） | 流水线状态被错发，导致下游跳动作业 | source=agent |
| `fallback_inject`（`task.resume` / `human.guidance`） | 引导事件混入 agent 流 | source=fallback_inject（独立审计） |
| 终末事件（`REVIEW_COMPLETE` / `LOOP_COMPLETE` / `report.done`） | 误发直接终止 loop | source=agent |

## 3. 详细需求

### U1 — 机制层：loop_invariant_assertion（首个交付）

**目标**：在 `EventLoop::run_iteration` 末尾添加 invariant assertion 校验；伪冒事件立即落 `invariant-violation.jsonl` + TUI 顶部 ⚠️ 计数。

#### 3.1.1 invariant 规则集

| 规则 | 触发条件 | 记录 |
|---|---|---|
| **INV-1：ralph 非控制 topic 越权** | events.jsonl 新增行 `hat=ralph` 且 `topic` 不在 `RALPH_CONTROL_TOPICS` | envelope.source = `loop_invariant_assertion`, reason = `ralph_impersonation_business_topic` |
| **INV-2：loop 自注入未带 source** | events.jsonl 新增行无 `_source` 字段但有 `hat` 字段 | reason = `fallback_inject_missing_source` |
| **INV-3：source 字段值非枚举** | `_source` 字段值不在 `agent` / `fallback_inject` / `system` | reason = `unknown_source_value` |
| **INV-4：fallback_inject 事件混入 agent 流** | 行有 `_source=fallback_inject` 但被任何 hat 消费方当作业务事件处理 | reason = `fallback_inject_treated_as_agent`（这是下游消费方检测） |

#### 3.1.2 invariant 失败行为

- **默认模式**（`ralph_config.strict_invariance = false`，缺省）：
  - 记录到 `.ralph/diagnostics/<session>/invariant-violation.jsonl`
  - 累加 `state.invariant_violation_count`
  - 累加 TUI 顶部 `⚠️ impersonation n` 计数（仅 n>0 时显示）
  - 写入 `.ralph/diagnostics/<session>/drift.jsonl`（U5 集成：drift 监测直接读 `invariant_violation_count`）
  - 写入 `.ralph/diagnostics/<session>/recovery.jsonl`（envelope.source = `loop_invariant_assertion`, outcome = `pending`）
  - **loop 不停**

- **strict 模式**（`ralph_config.strict_invariance = true`）：
  - 上述全部
  - **loop 立即硬终止**（return `LoopExit::InvariantViolation`）
  - 写 `.ralph/loop.lock` 旁的 `<session>/INVARIANT_BLOCKED.marker`
  - 下次 `ralph run` 启动时检测 marker → 拒绝启动并提示 "Human required: invariant violation at iter N, see invariant-violation.jsonl"

#### 3.1.3 新增 config schema

```yaml
# ralph.yml
ralph_config:
  strict_invariance: false  # 缺省 false。开启后任何 invariant 违例硬停 loop
  invariant_rules:
    INV-1: true             # 缺省全开
    INV-2: true
    INV-3: true
    INV-4: true
```

#### 3.1.4 TUI 顶部 ⚠️ 计数集成

- 修改 `crates/ralph-tui/src/widgets/header.rs:120`
- 在 `hat_with_backend` 行下方加一行（仅 `invariant_violation_count > 0` 时显示）：
  ```
  ⚠️ impersonation 3  (latest: INV-1 @ iter=6, see .ralph/diagnostics/.../invariant-violation.jsonl)
  ```
- 显示规则：行数≤2 不截断；行数>2 显示前 2 行 + `(N more in invariant-violation.jsonl)`
- 不影响正常 hat 显示——`🔍 Review Coordinator @kiro` 保持原位

#### 3.1.5 验收

- 测试用例 `loop_invariant_assertion_inv1_ralph_business_topic`：构造 events.jsonl 含 `hat=ralph, topic=work.done`，assert 落 `invariant-violation.jsonl` 且 `state.invariant_violation_count = 1`
- 测试用例 `loop_invariant_assertion_strict_mode_terminates_loop`：`strict_invariance = true` 时构造 INV-1 违例，assert loop 退出码为 `LoopExit::InvariantViolation`
- 测试用例 `loop_invariant_assertion_does_not_affect_normal_iter`：正常 iter 不产生 `invariant-violation.jsonl` 写入
- 测试用例 `tui_header_shows_warning_only_when_count_gt_zero`：`invariant_violation_count = 0` 时 TUI 头部不显示 ⚠️ 行

### U2 — 演练层：4 个 BDD impersonation scenario

**目标**：把"4 次复发"对应的攻击面全部变成回归测试，每次改 preset 必跑。

#### 3.2.1 scenario 列表

| 文件 | 攻击路径 | 期望行为 |
|---|---|---|
| `ce_executor_impersonation_stall_fallback.yml` | review-coordinator 19 分钟 stall → hard_gate 注入 `task.resume` → 落 events.jsonl | events.jsonl 该行 `_source=fallback_inject`；`invariant-violation.jsonl` 写 INV-2 记录；TUI ⚠️ = 1 |
| `ce_executor_impersonation_human_guidance.yml` | runner 注入 `human.guidance`（无 hat 字段）→ 任何 hat 消费 | events.jsonl 该行 `_source=fallback_inject`；INV-2 不触发（无 hat 字段）；TUI ⚠️ = 0（合规） |
| `ce_executor_impersonation_robot_response.yml` | RObot service 发 `human.response`（带 `source=robot-trusted`） | events.jsonl 该行 `_source=system`（因 `robot-trusted` 标记）；TUI ⚠️ = 0 |
| `ce_executor_impersonation_recovery_journal.yml` | `recovery.jsonl` 与 `events.jsonl` 时间戳冲突 | `recovery.jsonl` 不影响 events.jsonl 的 source 校验；TUI ⚠️ = 0（合规） |

#### 3.2.2 验收

- 4 个 scenario 全部加入 `crates/ralph-core/tests/scenarios/`
- `cargo test -p ralph-core ce_executor_impersonation` 全部 pass
- 每次 `presets/en/ce-executor.yml` 改动必须自动跑这 4 个 scenario（在 `just test-parallel` 中加 hook）

### U3 — 数据层：events.jsonl 增加 `_source` 字段

**目标**：让 events.jsonl 每行明确标记"写入来源"，下游消费方按 source 决定行为。

#### 3.3.1 schema 变更

- events.jsonl 每行增加 `_source: "agent" | "fallback_inject" | "system"` 字段
- 缺省值：使用既有 `last_active_hat_ids[0]` 反推：
  - 若 `hat` 字段在 `RALPH_CONTROL_TOPICS` 对应允许的 hat_id 集合内 → `_source = "agent"`
  - 若行无 `hat` 字段且 topic ∈ `RALPH_CONTROL_TOPICS` → `_source = "fallback_inject"`
  - 若行有 `source=robot-trusted` 标记 → `_source = "system"`
- 缺省推断失败的行 → 写入 `invariant-violation.jsonl`（INV-3）

#### 3.3.2 CLI 同步

- `ralph emit --source <agent|fallback_inject|system>` 新增 `--source` 参数
- 缺省 `--source = agent`
- 若 `--source = fallback_inject` 且 topic ∈ `RALPH_CONTROL_TOPICS`，允许（用于 hard_gate 注入场景）
- 若 `--source = agent` 且 `hat = ralph` 且 topic ∉ `RALPH_CONTROL_TOPICS`，仍被 `ralph_control_only` 拒（双层防御）

#### 3.3.3 下游消费方适配

- `drift_detector`：只处理 `_source = agent` 的事件计算 field completeness / coord join rate / emit cadence
- `stall_recovery`：只处理 `_source = agent` 的"超时不发事件"判定；`fallback_inject` 不算"超时"
- `state_machine`：照旧，不依赖 source（lifecycle 是 hat 内部行为）
- `event_projection`：照旧（projection 是按 topic 投影，不按 source）
- `task_cli`：照旧（task 走独立 .jsonl）

#### 3.3.4 验收

- 测试 `events_jsonl_backfill_source_legacy_replay`：读既有 4 次 fix 前的 events.jsonl（无 `_source`），backfill 推断出 source 字段；缺省推断失败率 < 1%
- 测试 `ralph_emit_with_source_fallback_inject`：`ralph emit --source fallback_inject --hat ralph human.guidance` 成功（既有 CLI guard 不拒）
- 测试 `ralph_emit_with_source_agent_rejected`：`ralph emit --source agent --hat ralph work.done` 被 `ralph_control_only` 拒
- 测试 `drift_detector_ignores_fallback_inject`：构造含 `fallback_inject` 的 events.jsonl，drift 报告 field completeness 仍按 `agent` 事件计算

### U4 — 架构层：events.jsonl 拆分（可选，U1+U2+U3 未堵住时再做）

**目标**：彻底把 agent 写入和 loop 注入分到两个文件。

#### 3.4.1 触发条件

满足以下任一条件才启动 U4：

- U3 落地后 30 天内 `invariant-violation.jsonl` 仍记录 ≥3 条 INV-2 违例
- 用户明确反馈"现有防护不够"

#### 3.4.2 拆分方案

- `.ralph/agent-events-xxx.jsonl`：只含 `_source=agent` 写入
- `.ralph/fallback-events-xxx.jsonl`：只含 `_source=fallback_inject` / `_source=system` 写入
- 保留 `events-xxx.jsonl` 作为合并视图（read-only，下游默认读）

#### 3.4.3 验收

- 同 U3 验收，外加：U1 INV-2 违例数 ≤0（因为 fallback 注入已分流）

### U5 — 语义层：EventRecord.hat 拆为 `hat` + `role`

**目标**：让 `events.jsonl.hat` 语义清晰（真实执行进程），TUI 显示的"角色"用独立 `role` 字段。

#### 3.5.1 schema 变更

- `EventRecord` 新增 `role: Option<String>` 字段
- 在 ce-executor coordinator 模式下：
  - `hat = "ralph"`（真实执行进程）
  - `role = "<被激活的角色名>"`（如 `review-coordinator` / `coordinator` / `executor`）
- 在 isolated 模式下：
  - `hat = "<真实 hat_id>"`
  - `role = hat`（两者一致）

#### 3.5.2 TUI 内部消费方

- `crates/ralph-tui/src/widgets/header.rs:120` 读 `role`（保持显示文本不变）
- `crates/ralph-tui/src/rpc_source.rs:240-254` 的 `RpcEvent::IterationStart.hat_display` 重命名为 `role_display`（**breaking change**）

#### 3.5.3 不变

- TUI 顶部显示文本 `🔍 Review Coordinator @kiro` 保持原样（仅内部字段名变）

#### 3.5.4 验收

- 测试 `event_record_role_field_in_coordinator_mode`：构造 ce-executor 编排 1 iter，assert events.jsonl 每行有 `hat="ralph"` 和 `role="<被激活角色>"`
- 测试 `tui_header_uses_role_field`：mock `RpcEvent::IterationStart.role_display`，assert header.rs 显示对应文本
- 测试 `isolated_mode_hat_equals_role`：构造 isolated preset 编排，assert `hat == role`

## 4. 验收标准

### 4.1 U1 完成时

- `loop_invariant_assertion` 已加入 `EventLoop::run_iteration` 末尾
- 4 条 INV 规则全部实现
- `ralph_config.strict_invariance` 配置生效
- TUI 头部 ⚠️ 计数集成完毕
- 4 个新单元测试 pass
- 现有 `cargo test` 全部 pass（无回归）

### 4.2 U2 完成时

- 4 个 BDD scenario 落地
- 每次 preset 改动自动跑这 4 个 scenario

### 4.3 U3 完成时

- events.jsonl 行带 `_source` 字段
- `ralph emit --source` CLI 参数生效
- 4 条 INV 规则的 INV-2 违例数 ≤既有 4 次 fix 后的复发数（验证有效）
- 既有 4 个 fix 的回归测试全部 pass

### 4.4 U4 完成时（条件性）

- events.jsonl 拆分为双文件
- U1 INV-2 违例数 = 0

### 4.5 U5 完成时

- `EventRecord` 拆 `hat` / `role`
- TUI 内部读 `role`
- 显示文本无变化（用户感知不变）

### 4.6 端到端回归门（每个 U 都必须过）

每个 U 落地时**必须**通过端到端回归门。**这是用户硬性要求——不允许引入回归**。

#### 4.6.1 两层端到端测试（前置 + 终验）

用户明确两层测试**必须分阶段跑**——**前置自动断言 + 终验手跑真看产物**。两层都过才允许合并。

| 层 | 名称 | 谁做 | 防什么 | 成本 |
|---|---|---|---|---|
| **前置** | **自动化 E2E（机器跑）** | Python 脚本 + CI | **中间产物格式正确**：events.jsonl 字段对、invariant-violation.jsonl 该有就有、TUI 计数 == 注入数 | 便宜（几分钟一次，可反复跑） |
| **终验** | **真跑 E2E（人手跑）** | 用户手跑 + 看产物 | **功能行为正确**：agent 真写了 bubble_sort.py、pytest 真过、events.jsonl 看起来"干净" | 贵（10 iter + 真 backend，分钟级） |

**为什么分两层**：

- 前置**只能查"产物"**——查不到 agent 是不是写出能跑的代码（修 `_source` 字段时 drift 行为变差、replay 工具 fallback 错乱、TUI 渲染 bug——这三种前置都查不出来）
- 终验**才能查"行为"**——但成本高、不能反复
- **先过前置保证"机器觉得没问题"→再过终验保证"人也觉得没问题"**——任何中间产物格式不对就别想浪费真 backend 时间

#### 4.6.2 端到端测试任务设计

**任务名**：`bubble_sort_e2e_regression`——实现一个 Python 冒泡排序函数（10-15 行代码量级）。理由：

- 任务足够小（1 个文件、1 个函数、1 个 test），ce-executor 全 10 hat 链路（coordinator→executor→review-coordinator→dimension-reviewer→synthesizer→plan-gate→shipper→reporter）能跑完
- 任务足够真实（有 plan、有 code、有 test、有 review），能暴露"伪冒"和"防伪冒"两条路径的回归
- 任务足够轻（10 iter 内必结束），适合反复跑

#### 4.6.3 前置：自动化 E2E（机器跑）

**目的**：在起真 backend 之前，把"机器能查的产物问题"全过一遍。**这是必要不充分条件**。

| 类别 | 触发 | 自动化检查（Python 脚本） |
|---|---|---|
| **E2E-Auto-A**：mock backend 跑完整 10 iter | `ralph run -c ralph.ce-executor.yml -H builtin:ce-executor -p "write a Python bubble_sort function" --max-iterations 10 --worktree /tmp/bubble-e2e --mock` | 解析 events-*.jsonl（断言行数 ≥10 且每行有合法 topic + _source 字段）；解析 recovery.jsonl（断言无 `outcome=failed` 记录）；断言 invariant-violation.jsonl **不存在**（happy path） |
| **E2E-Auto-C**：invariant violation 注入 | Python 脚本故意往 events.jsonl 注入 INV-1/2/3 违例行，再跑 `ralph run` | 解析 invariant-violation.jsonl（断言行数 = 注入数）；解析 TUI stdout（断言 ⚠️ 计数 == 注入数）；default mode 跑到底、strict mode 触发硬停 |

**Python 脚本**：`scripts/e2e_hat_impersonation_regression.py`，负责 worktree 准备、fixture 注入、子进程启动、产物解析、清理。

**PASS 条件**：

- E2E-Auto-A：所有断言通过
- E2E-Auto-C：注入 N 条 → 产物 N 条，TUI 计数 == N
- **任一失败 → 该 U 不允许进入终验阶段**（别浪费真 backend 时间）

**FAIL 处理**：直接 revert 该 U 的全部改动 → 不留半成品。

#### 4.6.4 终验：真跑 E2E（人手跑）

**目的**：在自动化 E2E 全过后，**真人开真 backend 跑 10 iter 看产物**——这是"5 层面改动没引入回归"的最终证据。

**操作**（每个 U 落地时由用户手跑）：

1. 开一个新 worktree：`git worktree add /tmp/bubble-e2e-ux -b e2e/bubble-ux-N`
2. 跑：`ralph -H builtin:ce-executor run -p "write a Python bubble_sort function" --max-iterations 10 --worktree /tmp/bubble-e2e-ux`
3. **看 TUI**：
   - 头部 `🔍 Review Coordinator @kiro` 显示是否正常切换
   - 顶部 ⚠️ 计数（U1 之后）是否仅在违规时出现
4. **看产物**（worktree 末态）：
   - `bubble_sort.py` 是否真存在、函数签名是否符合需求
   - `test_bubble_sort.py` 是否真存在、case 是否合理
5. **跑 pytest**：在 worktree 内 `cd /tmp/bubble-e2e-ux && pytest -v` → 必须 pass
6. **看 events.jsonl**：
   - 行数 ≥10
   - **没看到 `hat=ralph + topic=work.*` 之类伪冒**（关键——这正是 5 层面防的对象）
   - `_source` 字段（U3 之后）每行都有且值合法
7. **看 drift/recovery**：
   - `recovery.jsonl` 没 `outcome=failed`
   - `drift.jsonl` 无 P0 级告警

**PASS 条件**：

- TUI 头部切换正常
- pytest 跑过
- events.jsonl **没伪冒**（关键防线）
- drift/recovery 干净

**FAIL 处理**：revert 该 U 改动；**用本节产物手工补一个 E2E-Auto-D scenario 跑前置**（即"为什么前置没拦住"）→ 修前置 → 再跑终验。

#### 4.6.5 首次引入时

- 在 `scripts/` 下新增 `e2e_hat_impersonation_regression.py`（前置自动化）
- 在 `scripts/fixtures/e2e/bubble_sort/` 落 bubble_sort 任务 fixture
- 在 `Justfile` 新增 `just e2e-regression`（前置自动化）
- 在 `.github/workflows/` 新增 `e2e-hat-impersonation.yml`（前置自动化 CI）
- 在 `docs/guide/e2e-regression.md` 写两份文档：
  - **前置自动化使用**（机器怎么跑）
  - **终验手跑清单**（人怎么跑 7 步看什么）
- 全部就位后，**U1 落地时 E2E-Auto-A 必须先 pass（baseline），U1 改动后再 pass 才有资格进入终验**

### 4.7 回归防护"硬性 4 件套"

每个 U 落地时**必须同时满足**：

1. **既有 `cargo test --workspace --exclude ralph-e2e` 全部 pass**（既有回归）
2. **新增 4 个 BDD scenario pass**（U2 及之后）
3. **前置 E2E-Auto 全过**（E2E-Auto-A + E2E-Auto-C）
4. **终验 E2E 手跑全过**（TUI、产物、pytest、events.jsonl 无伪冒、drift/recovery 干净）

**顺序**：1 → 2 → 3 → 4。**任一失败 → 该 U 不允许合并**。**这是用户硬性要求——5 层面任何一层引入回归都不接受**。

**两层 E2E 缺一不可**：

- 没有前置：终验成本高，回归没拦住前会被反复触发真 backend
- 没有终验：前置只能查格式，agent 行为回归（如 drift detector 失灵、replay fallback 错乱、TUI 渲染 bug）**完全无防护**

## 5. 风险与缓解

| 风险 | 影响 | 缓解 |
|---|---|---|
| TUI 头部 ⚠️ 计数与既有设计哲学冲突 | 用户感知"告警噪音" | 仅 `count > 0` 显示；行数限 2 + `(N more)` |
| `strict_mode` 硬停 loop 与 Ralph Tenet 1 冲突 | 用户启用后频繁硬停 | 默认 `false`；开关显式；提示信息明确 |
| events.jsonl schema 演进兼容性 | 老 replay 工具读不到 `_source` | `_source` 为 optional；老消费者忽略即可 |
| `ralph emit --source` CLI breaking change | 自动化脚本可能需要更新 | 提供 deprecation 期 1 个 minor version；老命令仍兼容（默认 `--source = agent`） |
| U4 拆分文件波及 drift/stall/recovery 3 套诊断 | 漏适配导致静默读旧文件 | U4 触发条件严格（30 天观察期 + 用户明确反馈） |
| `last_active_hat_ids` 推断 source 字段缺省值 | 缺省推断错误的行 → INV-3 误报 | backfill 测试覆盖既有 4 次 fix 前的样本；推断失败率 < 1% 才视为通过 |

## 6. 假设

- 既有 `ralph_control_only` + `topic_deny_rules` + `plan_name_equality_required` 三层防御继续保留作为底层
- 既有的 16 个 BDD scenario 不重写，新 4 个 scenario 增量添加
- 既有 `recovery.jsonl` + `drift.jsonl` schema 不破坏
- `ralph_config.strict_invariance` 是新增 top-level 字段，向后兼容

## 7. 显式不在范围（用于拦截"过度设计"）

- 不引入事件溯源框架
- 不实现"事件链 hash 校验"
- 不实现"跨 loop 的全局审计"（仅单 loop 内）
- 不修改 TUI 显示文本
- 不实现"伪冒时自动 rollback 状态"

## 8. 后续

- U1 → U2 → U3 是**必做路径**
- U4 是**条件性必做**（30 天观察期）
- U5 是**收尾**（事件溯源完整闭环）
- 实施计划（ce-plan）应按 U1 → U2 → U3 → U4（条件触发）→ U5 顺序
- 每步完成都应能独立合并到 main，**不留半成品**

## 9. 引用与关联

- 现状诊断：`docs/report/2026-06-10-ce-executor-ralph-hat-impersonation-diagnosis.md`
- 在跑 001-plan：`docs/plans/2026-06-10-001-fix-ce-executor-worktree-isolation-plan.md`
- 等待启动 002-plan：`docs/plans/2026-06-10-002-fix-ce-executor-hat-impersonation-remaining-guards-plan.md`（**本需求落地后 002-plan 应被本需求替代**）
- 相关 fix commits：`e272808` / `524fb73` / `b51bfa9` / `87cc8eb` / `efc0fd0`
- BDD 既有：`crates/ralph-core/tests/scenarios/four-p0-guards/u{1,2,3,4}-*.yml`
- TUI 集成点：`crates/ralph-tui/src/widgets/header.rs:120`、`crates/ralph-tui/src/rpc_source.rs:240-254`
- 机制集成点：`crates/ralph-cli/src/loop_runner/runner.rs:1619-1657`、`crates/ralph-cli/src/loop_runner/hard_gate.rs:380-465`、`crates/ralph-core/src/event_loop/mod.rs:2004-2068`
- E2E 回归脚本（本需求首次引入）：`scripts/e2e_hat_impersonation_regression.py`
- E2E fixture（bubble sort 任务）：`scripts/fixtures/e2e/bubble_sort/`（含 `bubble_sort.py` 模板、`test_bubble_sort.py`、`pyproject.toml`）
- Justfile target：`just e2e-regression`（包裹 Python 脚本）
- CI workflow：`.github/workflows/e2e-hat-impersonation.yml`（mock backend 跑 E2E-A + E2E-C）
- E2E 使用文档：`docs/guide/e2e-regression.md`
- 既有 mock CLI（E2E-A 依赖）：`crates/ralph-e2e/src/mock_cli.rs`
