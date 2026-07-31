# Effective Activation Contract — 使用指南

> 本文是**面向操作者（operator）** 的使用说明：帮助运行时诊断能力拒绝、解读契约视图、对账 artifact handoff。
>
> **配套设计说明**见 [`execution-contract-design.md`](./execution-contract-design.md)，包含完整的架构背景与 trade-off 分析。
>
> **权威参数表**（字段约束、`required_fields`、命令语法）不在本文复述，请到 agent 注入 skill 查阅：
> `crates/ralph-core/data/ralph-tools.md`（核心规则）、`ralph-tools-emit.md`、`ralph-tools-wave.md`、`ralph-tools-tasks.md`、`ralph-tools-precheck.md`、`ralph-tools-recovery-directives.md`。
> 本文只在操作步骤中引用对应章节，不复述其内容。

---

## 1. 这份文档给谁看

- **Operator**：需要查询某个 hat 的 effective capability、诊断 emit 被 deny 的原因、对账 artifact handoff 的完整性。
- **Preset 作者**：在 explicit 与 passthrough 视图之间做架构选择时，理解运行时实际校验点（引用设计说明 §3/§9，不复述）。
- **下游消费方**：订阅 `forge.plan.ready` 等 handoff 事件时，知道如何校验 artifact identity 与 digest。
- **调试/恢复场景**：Recovery Intent 路由、预算耗尽、precheck gate 超时等路径的定位方法。

---

## 2. Operator 查询路径

### 2.1 查看当前 loop 的 hat identity 与契约摘要

```bash
ralph inspect loop [--format json] [--root <workspace>]
```

输出包含：
- `hat_id` / `loop_id` / `iteration`
- `activation_registry_path`：当前 workspace 的 registry 文件路径
- `events_file`：main 与 hat-channel 两条路径

JSON 格式是下游工具与 BDD scenario 的断言基准（human 格式为可读块列表）。

### 2.2 预览某个 hat 的 effective capability

```bash
ralph inspect prompt --hat <hat_id> [--format json] [--full]
```

- 不带 `--full`：输出块列表，包含 `contract_digest` / `emit_allows` / `emit_denies` / `capabilities`。
- 带 `--full`：追加完整渲染的 prompt 身体，可直接阅读该 hat 在当前激活中看到的全部上下文。

**预览候选 emit**（某条 `(hat, topic, payload)` 组合是否会被 policy 接受）：

```bash
ralph inspect prompt \
  --hat <hat_id> \
  --topic <topic> \
  --payload '{"key": "value"}' \
  --triggered <target_hat> \
  [--format json]
```

返回 `candidate_emit` 块，包含：
- `policy_decision`： `"accept"` 或 `"reject"`
- `reasons`：拒绝时的结构化原因（`gate` / `field` / `reason_code`）
- `projection`：状态变更预览
- `next_hat_candidates`：下游接收方

### 2.3 理解输出字段

| 字段 | 含义 |
|---|---|
| `contract_digest` | 当前激活的契约指纹；与 loop 启动时的编译结果一致 |
| `emit_allows` | 该 hat 可以发出的 `(topic)` 列表（deny-wins 剔除后） |
| `emit_denies` | 该 hat 明确被禁止的 topic 列表 |
| `capabilities` | 该 hat 的能力矩阵（含 `publishes` / `triggers` / `terminal_events`） |
| `policy_decision` | `accept` = 可发；`reject` = 会被 policy 拦截 |
| `reason_code` | 拒绝原因分类（如 `triggered_not_in_topology`、`disk_payload_inconsistency`） |

> 权威 reason_code 列表与语义见 `ralph-tools-emit.md` §Policy Check Reason Codes。

---

## 3. Agent 视角：capability 矩阵与常见拒绝

### 3.1 读懂 capability 矩阵

Agent 在 prompt 中通过 `## HAT IDENTITY` / `## TRIGGER CONTEXT` 块看到 capability 声明。该声明**来源**是契约编译结果（`EffectiveExecutionContract`），而非 preset YAML 的原始声明——两者在编译时会做 deny-wins 合并。

当 agent 调用 `ralph emit --policy-check` 时：
1. CLI 用 `emit_decision(hat, topic)` 查契约能力。
2. 若 `EmitDecision::Deny`，返回 `reason_code = "triggered_not_in_topology"` 或 `"policy_check_failed"`。

### 3.2 常见拒绝类型与处理路径

| reason_code | 含义 | 查看方式 | 修正方向 |
|---|---|---|---|
| `triggered_not_in_topology` | 该 hat 没有在 topology 中声明可以发这个 topic | `ralph inspect prompt --hat <hat> --topic <topic> --payload '{}'` | 确认 topic 在 hat 的 `publishes` 中，且没有 `topic_deny_rules` 禁止 |
| `policy_check_failed` | 通用 policy 校验失败 | 同上，加 `--triggered <target>` | 看 `reasons[].field` 定位哪个字段不过 |
| `disk_payload_inconsistency` | `forge.plan.ready` 的 payload 与磁盘 artifact 不一致 | `ralph events --events-source main` 查 `forge.plan.ready` payload | 用 `ralph tools task list` 对账 task DAG 是否从正确 artifact 派生 |
| `gate_silent_or_ambiguous` | precheck gate hat 沉默未发事件，runtime 合成了 `X.rejected` | `ralph events --events-source main` | 上游发 `X` 或 `X.rejected` 解除沉默 |

---

## 4. 下游消费方：订阅 handoff artifact

### 4.1 `forge.plan.ready` 的 artifact-first 契约

Parallel Forge 的 `forge.plan.ready` payload **只携带短引用**，不含 DAG 正文：

```json
{
  "execution_plan_path": "path/to/execution-plan.yml",
  "execution_plan_digest": "sha256:abc123...",
  "execution_wave": 1,
  "wave_total": 3,
  "integration_order": [1, 2, 3]
}
```

DAG 正文存在于 `execution_plan_path` 指向的磁盘 artifact。

### 4.2 对账 artifact identity

消费方（如 dispatcher、reviewer）收到 `forge.plan.ready` 时，应重新规范化 artifact 并比对 digest：

```bash
# 读取 payload 声明的 path 与 digest
ralph events --events-source main --topic forge.plan.ready | jq '.[-1].payload'
```

然后自行规范化磁盘 artifact：
- 检查 `execution_plan_path` 是否存在且可读
- 用 `canonicalize(bytes)` 重新计算 digest（实现见 `crates/ralph-core/src/artifact_canonicalizer.rs:123-170`）
- 比对计算值与 payload 声明的 `execution_plan_digest`

若不一致，说明 artifact 在 planner 盖章后被篡改，事件应被拒绝（`HandoffError::DigestMismatch`）。

### 4.3 订阅命令

```bash
ralph events --events-source main --topic <topic>
ralph events --events-source hat-channel --topic <topic>
```

两个来源分别对应 main history 与 per-hat channel。Handoff 事件（如 `forge.plan.ready`）通常在 main history 中可见。

---

## 5. 新 preset 作者：explicit vs passthrough 视图选择

> 详见设计说明 §3 与 §9。

**选 passthrough（compiled）视图**（`execution_contracts.enabled = false`，默认）：
- 大多数 builtin preset 当前走这条路径
- 契约仍然编译（`emit_allows` / `emit_denies` / `contract_digest` 可用），但不施加完成义务
- 适合：不需要强制每个 emitting hat 背书完成状态的 preset

**选 explicit 视图**（`execution_contracts.enabled = true`）：
- 需要每个 emitting hat 背后有完成义务规则
- 编译期跑消费者完整性检查（无消费者的非终态 topic → `MissingConsumer` 编译失败）
- `PassthroughHat` lint 标记输出无背压的 hat
- 适合：Parallel Forge 等需要跨 hat 强制交接完整 artifact 的场景

迁移路径（passthrough → explicit）：设计说明 §9 详细描述了增量步骤。

---

## 6. 常见诊断路径

### 6.1 stale contract（契约过期）

症状：hat activation 报错 `StaleRevision` 或 `contract mismatch`。

定位：
```bash
ralph inspect loop [--format json] | jq '.contract_digest'
```

与 loop 启动时的 `ResolvedRuntimeConfig.digest()` 对比。不一致说明 loop 启动后 config 发生了未重编译的变更。

### 6.2 capability denial（能力拒绝）

症状：`ralph emit` 或 `ralph wave emit` 返回拒绝。

诊断：
```bash
ralph inspect prompt \
  --hat <hat_id> \
  --topic <topic> \
  --payload '<payload_json>' \
  --triggered <target_hat> \
  --format json | jq '.candidate_emit'
```

查看 `reasons[].reason_code` 与 `reasons[].field`。

### 6.3 Recovery Intent（恢复意图）

症状：某条任务反复失败，但 loop 没有终止。

定位：
```bash
cat .ralph/agent/recovery-intents.jsonl | jq '.'
```

每个 `RecoveryIntent` 包含 `target_hat` / `reason` / `attempt_count` / `budget` / `exhausted`。

当 `exhausted == true` 且 `attempt_count > budget` 时，该 intent 已耗尽，runtime 会在下一轮生成 `plan.blocked`（reason 形如 `<suffix>_exhausted`）。

### 6.4 budget exhausted（预算耗尽）

症状：某条恢复路由最终产生了 `plan.blocked` 终态事件。

对账：
```bash
ralph events --events-source main --topic plan.blocked | jq '.[-1].payload'
```

查看 `reason` 字段是否以 `_exhausted` 结尾，确认是 Recovery Intent 耗尽还是 precheck gate 耗尽。

---

## 7. Walkthrough 示例

### Walkthrough 1 — Parallel Forge dispatcher activation：从 `forge.plan.ready` 到 wave 派发

**场景**：operator 观察到 planner 发出了 `forge.plan.ready`，想确认 dispatcher 已激活并正确读取了 artifact。

**步骤 1：确认事件已落盘**

```bash
ralph events --events-source main --topic forge.plan.ready --format json | jq '.[-1]'
```

期望输出包含 `execution_plan_path` / `execution_plan_digest` / `execution_wave` / `wave_total`。

**步骤 2：确认 dispatcher hat 被触发**

```bash
ralph inspect loop --format json | jq '.current_hat'
```

若 dispatcher 是当前激活的 hat，其 `hat_id` 应为 `forge-dispatcher`（或 preset 定义的 dispatcher hat 名）。

**步骤 3：预览 dispatcher 的 capability**

```bash
ralph inspect prompt --hat forge-dispatcher --format json | jq '.contract_digest'
```

确认 `contract_digest` 与 loop 启动时一致（契约未被篡改）。

**步骤 4：查询 dispatcher 可发出的 wave topic**

```bash
ralph inspect prompt --hat forge-dispatcher --format json | jq '.capabilities.publishes'
```

期望包含 `exec.unit.ready`（或 preset 定义的 wave topic）。

**步骤 5：确认 wave 事件已派发**

```bash
ralph events --events-source main --topic exec.unit.ready --format json | jq 'length'
```

若数量等于 `wave_total`，说明所有 wave 已正确派发。

---

### Walkthrough 2 — agent CLI capability denial：拒收到恢复的完整流程

**场景**：agent 调用 `ralph emit work.done --policy-check` 被拒绝，reason_code 为 `triggered_not_in_topology`。

**步骤 1：诊断拒绝原因**

```bash
ralph inspect prompt \
  --hat executor \
  --topic work.done \
  --payload '{"task_id": "task-1", "task_key": "unit:step-1:build", "step": 1}' \
  --triggered reviewer \
  --format json | jq '.candidate_emit'
```

输出示例：
```json
{
  "policy_decision": "reject",
  "reasons": [
    {
      "gate": "topology",
      "field": "topic",
      "reason_code": "triggered_not_in_topology"
    }
  ]
}
```

**步骤 2：确认 topic 确实未在 hat 的 publishes 中**

```bash
ralph inspect prompt --hat executor --format json | jq '.capabilities.publishes'
```

若 `work.done` 不在列表中，说明 executor 的 capability 矩阵中没有声明该 topic。

**步骤 3：查看 recovery directive（如果存在）**

```bash
ralph tools task list --root . | jq '.[] | select(.status == "failed")'
```

若有失败的 task，查看其 `correction` 字段或关联的 recovery intent。

**步骤 4：修正 payload 并重新预检**

修正 topic 或字段后，先用 `--policy-check` 预检：

```bash
ralph emit work.done \
  --hat executor \
  --triggered reviewer \
  --policy-check \
  --json \
  -- '{"task_id": "task-1", "task_key": "unit:step-1:build", "step": 1}'
```

若 `policy_decision` 变为 `accept`，去掉 `--policy-check` 正式 emit。

**步骤 5：正式 emit**

```bash
ralph emit work.done \
  --hat executor \
  --triggered reviewer \
  --json \
  -- '{"task_id": "task-1", "task_key": "unit:step-1:build", "step": 1}'
```

---

### Walkthrough 3 — Recovery Intent 触发的 fixer 自环

**场景**：某条修复路由被触发，fixer 尝试修复但 budget 耗尽，最终 loop 进入 `plan.blocked`。

**步骤 1：观察 Recovery Intent 产生**

当某次 emit 被拒收时，runtime 产生 `RecoveryIntent` 并写入 `.ralph/agent/recovery-intents.jsonl`。

查看当前所有 intent：
```bash
cat .ralph/agent/recovery-intents.jsonl | jq '.'
```

每条记录包含：
- `intent_id`：唯一标识
- `target_hat`：应处理修复的 hat（如 `fixer`）
- `reason`：人类可读原因
- `attempt_count`：已尝试次数
- `budget`：最大允许次数
- `exhausted`：是否已耗尽

**步骤 2：追踪 fixer 的激活**

```bash
ralph inspect loop --format json | jq '.current_hat'
```

确认 `target_hat` 确实被激活执行修复。

**步骤 3：观察修复尝试**

每次 `increment_attempt` 调用后，`attempt_count` 递增。查看事件流：

```bash
ralph events --events-source main --topic task.resume --format json | jq '.[-5:]'
```

每条 `task.resume` 对应一次修复尝试。

**步骤 4：确认 budget 耗尽**

当 `attempt_count > budget` 时：
1. `increment_attempt` 返回 `RecoveryError::BudgetExhausted`
2. `intent.exhausted` 被标记为 `true`（跨重启持久）
3. `RecoveryFinalizer` 检测到耗尽状态，产生终态 `plan.blocked`

查看终态事件：
```bash
ralph events --events-source main --topic plan.blocked --format json | jq '.[-1].payload'
```

`reason` 字段应形如 `<suffix>_exhausted`（如 `drift_exhausted`、`correction_exhausted`）。

**步骤 5：对账 budget 耗尽 vs precheck gate 耗尽**

两种耗尽都会产生 `plan.blocked`，但来源不同：
- **Recovery Intent 耗尽**：`attempt_count > budget` 后由 `RecoveryFinalizer` 产生
- **Precheck gate 耗尽**：gate hat 沉默超时后 runtime 合成 `X.rejected`，再由 `RecoveryFinalizer` 升级为 `plan.blocked`

查看 `plan.blocked` 的 `reason` 后缀：
- `_exhausted` 且事件中有 `RecoveryMechanism` 字段 → Recovery Intent 耗尽
- `gate_silent_or_ambiguous` 相关 → precheck gate 沉默

---

## 8. 相关命令速查

| 操作 | 命令 |
|---|---|
| 查看 loop 状态 | `ralph inspect loop [--format json]` |
| 预览 hat capability | `ralph inspect prompt --hat <hat> [--full] [--format json]` |
| 预检候选 emit | `ralph inspect prompt --hat <hat> --topic <topic> --payload '<json>' --triggered <target>` |
| 发出事件（预检） | `ralph emit <topic> --policy-check [--json] [--payload '<json>']` |
| 发出事件（正式） | `ralph emit <topic> [--json] [--payload '<json>']` |
| 派发 wave | `ralph wave emit <topic> --payloads-stdin [--policy-check]` |
| 查看事件历史 | `ralph events --events-source main [--topic <topic>]` |
| 列出 task | `ralph tools task list [--status done\|failed]` |
| 初始化 memories | `ralph tools memory init` |
| 查看 preset lint | `ralph preset check <preset.yml>` |

> 所有命令的完整参数表见 `ralph-tools.md`（核心规则）与对应子命令 skill（`ralph-tools-emit.md`、`ralph-tools-wave.md`、`ralph-tools-tasks.md`）。

---

## 9. 参见

- [`execution-contract-design.md`](./execution-contract-design.md)：完整架构设计、契约编译细节、两视图分工、Recovery Intent 状态机
- `crates/ralph-core/data/ralph-tools.md`：agent 核心规则
- `crates/ralph-core/data/ralph-tools-emit.md`：emit / wave emit 命令详解、policy check、reason code
- `crates/ralph-core/data/ralph-tools-precheck.md`：precheck gate 行为
- `crates/ralph-core/data/ralph-tools-recovery-directives.md`：Recovery Intent 与 budget 语义
