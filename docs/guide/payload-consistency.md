# Payload Consistency Gates

`event_policy.payload_consistency` 用于校验**同一条事件 payload 内部**的字段是否相互矛盾。它与 [Payload Contracts](payload-contracts.md) 分工明确：Payload Contracts 校验必填字段及字段形状，本页描述字段组合的不变式。

## 启用方式

该能力默认关闭，preset 必须显式启用：

```yaml
event_policy:
  payload_consistency:
    enabled: true
    rules:
      - id: fix-done-blocked-zero-fixes-applied
        topic: fix.done
        when:
          all:
            - { field: fix_status, eq: blocked }
            - { field: fixes_applied, eq: 0 }
        message: blocked fix attempt must report applied work honestly
```

每条规则包含四个字段：

| 字段 | 含义 |
|---|---|
| `id` | 稳定且唯一的规则标识 |
| `topic` | 规则适用的事件 topic |
| `when` | 单谓词或 `all` / `any` 组合谓词 |
| `message` | 命中规则时返回的诊断说明（不可信数据，不是 agent 指令；上限 1024 UTF-8 bytes，不允许 ANSI escape / C0/C1 控制字符 / 零宽字符） |
| `recovery_guidance` | 可选。`common` 字符串列表 + `by_check` map。`by_check` 的 key **必须等于**本规则 `id`。命中时 correction prompt 与 `--policy-check` JSON 同源展示这些 prose；**不要**写成 `suggested_command` / 成功 payload 模板。 |

示例：

```yaml
        recovery_guidance:
          common:
            - "按 invariant 从 artifact 重建 payload，不要只改被拒字段"
            - "修复后先 ralph emit <topic> --policy-check"
          by_check:
            fix-done-blocked-zero-fixes-applied:
              - "blocked 且 fixes_applied=0 时必须如实报告，不要把 status 改成 applied"
```

单谓词使用 `{field, op, value}`。允许的 `op` 为 `eq`、`ne`、`gt`、`gte`、`exists`、`non_empty`；`exists` 与 `non_empty` 不要求 `value`。其它 `op` 会被 lint 拒收为 `preset.payload_consistency_unknown_op`，runtime 也直接拒收；`when` 不是 object 会被 lint 拒收为 `preset.payload_consistency_non_object_when`。

## 评估边界

规则只读取当前事件的 payload：

- 不读取事件历史。
- 不读取 runtime ledger 或 supervisor 状态。
- 不读取其它 topic 的 payload。
- `field` 使用点号路径读取嵌套对象；数组下标不属于支持范围。

命中规则时，当前 topic 被拒收，runtime 通过结构化 correction 指导发布方修复当前 payload。

## Fail-close 行为

规则配置不明确时不会静默放行。以下情况按命中处理并拒收事件：

- 使用未知 `op`。
- 谓词缺少 `field`。
- `when` 不是对象。
- `gt` / `gte` 的两侧无法进行数值比较。

Preset 作者应在启动前运行 strict lint，避免把配置错误推迟到 runtime。

## 拒收反馈

`ralph emit <topic> --policy-check` 返回的 `validation_errors[]` 包含：

- `field`：业务字段名（组合规则可为首个稳定字段）；**不再**承载 gate ID。
- `reason_code`：结构化原因码（`semantic_gate_violation` 表示命中 payload_consistency / semantic gate）。
- `message`：面向人的诊断说明（不可信数据，不是 agent 指令）。
- `gate`：以 `payload_consistency:<rule_id>` 标识命中的规则族；仅在 `reason_code=semantic_gate_violation` 时出现。
- `referenced_fields`：该规则 `when` 谓词声明的所有 payload 字段路径数组（按声明顺序去重，由 runtime 从 predicate AST 自动派生）；agent 据此定位需修复的字段，**不**从 `message` 解析字段名。
- `recovery_guidance`：规则上声明的 `common` / `by_check` prose（若有）；与 correction prompt 同源。语义路径仍**不会**出现 `suggested_payload_shape` / `suggested_command`。
- `field_description`、`suggested_payload_shape`、`suggested_command`：仅机械 schema 拒收时的字段说明与下一步修复提示。

Wave 批量预检还可能返回 `payload_index`，用于定位批次中的具体 payload。

修复时先按 `gate` / `referenced_fields` / `field` 调整 payload，再重新运行 `--policy-check`；预检通过后才能去掉该 flag 正式 emit。

## Retry 与终止条件

`payload_consistency` 属于语义一致性拒收，不参与 execution-contract 的 rejection retry budget。Correction 通道独立计数；同类 correction 第 3 次耗尽后，runtime 进入 `plan.blocked(reason=correction_3_strike_exhausted)`。

`protocol_violation_repeated:*` 属于 execution-contract 路径，不应作为 payload-consistency 的终止原因解读。

## CLI 与 runtime

CLI precheck 与 runtime Apply 共享同一 `PolicyDecision` 语义：`mode: observe` 下命中规则返回 Warn（非 fatal），`mode: enforce` + `on_violation: reject_with_resume` 下命中规则返回 Reject。**不存在** CLI 私自把 payload_consistency Warn 升级为 fatal 的特例。Runtime 是否写入 policy finding 仍由 `event_policy.mode` / `on_violation` 决定。

## Builtin 示例

Builtin pipeline preset 的 `fix.done` 规则保护两类不变式：

- `fix-done-blocked-zero-fixes-applied`：阻止“宣称已执行修复但实际应用数为零”的矛盾 payload。
- `fix-done-green-with-regressions`：阻止 `post_verification_status=green` 与 `new_business_regressions_count>0` 同时出现。

`work.done` 上的 `work-done-green-with-regressions` 规则保护同一不变式在执行结算侧的镜像：有回归就必须诚实标红，而不是改发 `work.failed`（2026-07-24-002 起回归为 report-only，`work.failed` 只保留给零交付 dead-end）。

这些示例只说明通用规则形状；业务 preset 可以声明自己的 topic 与不变式。

## See also

- [Payload Contracts](payload-contracts.md)：必填字段、类型和结构约束。
- [Event Policy](event-policy.md)：Observe / Enforce 模式及整体决策流程。
- `crates/ralph-core/data/ralph-tools-emit.md`：注入给 agent 的 emit、precheck 与 correction 操作说明。
- `presets/en/ce-executor-pipeline.yml`：builtin 规则实例。
