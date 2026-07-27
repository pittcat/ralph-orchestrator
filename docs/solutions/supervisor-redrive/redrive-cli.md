# `ralph wave redrive` — Operator CLI Reference

> 2026-07-25-005 plan U11 引入的 supervisor wave redrive 操作员命令。
> 2026-07-27-004 plan U4: 文档修正执行模型 — redrive 只创建
> 子 attempt wave + 复制 slot 元数据,**不会**自动派发 worker。
> Operator 必须接着执行 `ralph run --resume` 让 loop 启动 seam
> 消费 child descriptor 并复用现有 dispatcher 重新派发。
> 仅限 operator 在 loop 外手动干预使用；agent 无权调用。

## 什么是 redrive

`ralph wave redrive` 为已关闭 wave（phase `done` 或 `failed`）中处于 `failed` 状态的具体 slot 创建子 attempt wave：

- 子 wave 继承父 wave 的 `kind` 与 `slot_retry_budget`
- `attempt_epoch = parent.attempt_epoch + 1`
- 父 wave ledger 不被重写——父 wave 的 `done`/`failed` 记录保持不变
- 子 wave 是 supervisor store 中独立的新行；**仅**写 store 元数据 + 复制 bounded activation descriptor（topic、payload、slot index、kind、digest）。子 wave 注册后处于 `Pending`，**不会**自动 spawn worker

## 何时使用

典型 operator 干预场景：

1. **部分 slot 执行器崩溃**：wave 的某些 slot 在执行过程中节点宕机或进程被 kill，slot 状态为 `failed`，但 wave 已进入终态（`done`/`failed`）。
2. **确认根因**：operator 用 `ralph wave inspect <wave_id>` 确认 wave 确实处于终态且 `failed_count > 0`，排除"还在进行中"的可能。
3. **手动 redrive**：operator 针对具体失败 slot 调用 `ralph wave redrive`，创建子 wave 复制 slot 元数据。
4. **必须 resume 才执行**：operator 接着执行 `ralph run --resume`。loop 启动 seam 消费 child descriptor，复用现有 supervisor dispatcher / worker executor 重新派发；agent 不应自己启动新进程。inspect 子 wave 确认恢复进度。

## 何时不用

- ❌ 对仍在 `dispatch` / `collect` / `integrate` 阶段的 live wave 使用——phase 校验会拒绝。
- ❌ 用 redrive 绕过 FlowStepScope——hand-patched `exec.unit.done` 等业务事件仍被 scope 拒绝；redrive 只解决"slot 需要重新调度"的问题，不改变业务事件语义校验规则。
- ❌ Agent 在 loop 激活内调用——redrive 是 operator 手动干预工具，不是常规 hat 工作流的一部分。
- ❌ 所有 slot 均已完成（无 failed 槽）时调用——会返回 `no failed slots to redrive`。

## 语法

```bash
# 所有失败 slot 均 redrive
ralph wave redrive --wave-id <PARENT_WAVE_ID>

# 指定具体 slot
ralph wave redrive --wave-id <PARENT_WAVE_ID> --slots 0,2,5

# JSON 输出（适合脚本解析）
ralph wave redrive --wave-id <PARENT_WAVE_ID> --output json

# 指定配置文件（与 preset 联用时）
ralph wave redrive --wave-id <PARENT_WAVE_ID> -c ralph.yml
```

## 示例会话

```bash
# 1. 检查 wave 状态
$ ralph wave inspect w-abc123 --output json
{
  "ok": true,
  "wave_id": "w-abc123",
  "registered": true,
  "availability": "available",
  "phase": "done",
  "expected_total": 7,
  "completed_count": 5,
  "failed_count": 2,
  "pending_count": 0,
  "in_flight_count": 0,
  "cancel_requested": false
}

# 2. 发现 2 个 failed slot（索引 1 和 4），operator 手动 redrive
$ ralph wave redrive --wave-id w-abc123 --slots 1,4 --output text
ok
parent_wave_id: w-abc123
child_wave_id: w-xyz789
attempt_epoch: 2
slots: [1, 4]

# 3. 确认子 wave 已创建
$ ralph wave inspect w-xyz789 --output json
{
  "ok": true,
  "wave_id": "w-xyz789",
  "registered": true,
  "availability": "available",
  "phase": "dispatch",
  "expected_total": 2,
  "completed_count": 0,
  "failed_count": 0,
  "pending_count": 0,
  "in_flight_count": 2
}
```

## 拒绝错误

| 错误信息 | 含义 |
|----------|------|
| `cannot redrive a wave in phase 'done'` | 父 wave 已成功完成（所有 slot 都 completed）；没有需要恢复的失败 slot |
| `cannot redrive a wave in phase 'integrate'` | 父 wave 仍在集成中；redrive 应在终态后操作 |
| `no failed slots to redrive` | 指定 wave_id 的所有 slot 均非 failed 状态 |
| `unknown wave '<id>'` | wave_id 不存在于 supervisor store |
| `supervisor store not found` | `.ralph/supervisor.db` 不存在且 `RALPH_EMISSION_STORE_PATH` 未设置 |
| `failed to open supervisor store` | store 文件存在但打开失败（如损坏、权限问题） |

## 幂等性

同一 `(parent wave_id, slot index, epoch)` 三元组重复调用返回已有子 wave，不创建重复记录。例如：

```bash
# 首次调用
$ ralph wave redrive --wave-id w-abc123 --slots 1 --output json
{"ok":true,"parent_wave_id":"w-abc123","child_wave_id":"w-xyz789","attempt_epoch":2,"slots":[1],"redrive_request_id":"req-001"}

# 重复调用——返回相同 child_wave_id，不报错
$ ralph wave redrive --wave-id w-abc123 --slots 1 --output json
{"ok":true,"parent_wave_id":"w-abc123","child_wave_id":"w-xyz789","attempt_epoch":2,"slots":[1],"redrive_request_id":"req-001"}
```

## FlowStepScope 不被绕过

`ralph wave redrive` 是纯 operator 维护命令，仅写入 supervisor store 元数据（创建子 wave 行），不 emit 任何业务事件。因此它：

- 不触发 FlowStepScope 检查（因为没有业务事件写入）
- 不需要 agent context 的 hat 权限

但这并不意味着后续 agent 可以在子 wave 中 hand-patch 业务事件——`exec.unit.done` 等事件在写入时仍受 FlowStepScope 约束，redrive 只是让 slot 有机会重新执行，并不改变业务事件的校验规则。

## 与 salvage 的区别

| | `ralph wave redrive` | salvage merge |
|---|---|---|
| 触发时机 | Operator 手动调用 | 运行时自动触发（dispatcher 在 fan-in 时） |
| 作用于 | `done`/`failed` wave 的 failed slot | `failed` wave 的已 `completed` slot |
| 创建新 wave | 是 | 否（复用原 wave） |
| 继承关系 | 子 wave → 父 wave | 无子 wave，salvage 合并到 main ledger |
| 调用者 | Operator（loop 外） | Runtime（dispatcher 自动） |
