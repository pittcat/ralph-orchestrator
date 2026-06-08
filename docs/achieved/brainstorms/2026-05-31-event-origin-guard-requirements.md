---
date: 2026-05-31
topic: event-origin-guard
---

# 事件来源防护机制（Event Origin Guard）

## Summary

在 EventBus、process_parse_result 和 emit_command 三个层面增加事件来源校验，彻底阻断 LLM 输出的假事件（fake/demo events）污染事件总线。未注册的 hat、未声明的 publish topic、以及伪造的时间戳均会被拒绝。

---

## Problem Frame

ce-executor preset 运行时，约 55% 的事件是 LLM 模型输出的 demo 事件：`strategist`、`ralph` 等未在 preset 中注册的 hat 名，`build.done`、`debug.step`、`LOOP_COMPLETE {"reason":"retry"}` 等不在事件链中的 topic，以及 `2024-01-01T00:00:00Z` 这样来自训练数据的假时间戳。

现有防护有两处失效：
- **`HatRegistry::can_publish()` 对未知 hat 返回 `true`**——让未注册 hat 可以任意发布事件
- **scope enforcement 只在 isolated mode 执行**——ce-executor 的 coordinator mode 完全不走校验
- **`--ts` 参数由 LLM 控制**——假时间戳直接写入事件文件

根本原因是事件来源验证的缺失，而非配置问题。在每一层（总线、处理管道、写入工具）增加验证，才能从机制上杜绝。

---

## Requirements

### EventBus 层：来源 hat 注册校验
- R1. `EventBus::publish()` 在路由前检查事件 source：如果 source 被设置但对应 hat 不在注册列表中，拒绝该事件
- R2. 系统事件（source 为 `None` 的事件，例如 `loop.terminate`、`event.malformed`）不受此限制，始终放行
- R3. 拒绝的事件静默丢弃，不发布诊断事件（防止诊断事件本身引发二次路由问题）

### process_parse_result 层：publish scope 全面强制
- R4. `process_parse_result()` 在所有 execution mode 下（不仅 isolated mode）执行 hat publish scope 校验
- R5. 校验逻辑：对每条事件，从 `hat` 字段提取来源 hat，调用 `registry.can_publish()`。如果返回 `false`，丢弃该事件
- R6. `HatRegistry::can_publish()` 修改：当查询的 hat_id 不在 registry 中时，返回 `false` 而非 `true`

### emit_command 层：移除 --ts 参数
- R7. `ralph emit` 完全移除 `--ts` 参数，永远使用 `chrono::Utc::now()` 生成时间戳
- R8. 已有调用 `--ts` 的代码（如 wave emit）不受影响——移除 CLI 参数后，emit 函数内部自动生成当前时间

---

## Success Criteria

- 运行 ce-executor preset 时，事件文件中不再出现未注册 hat（如 `strategist`、`ralph`）的事件
- 运行 ce-executor preset 时，事件 chain 之外的 topic（如 `debug.step`、`build.done`）不会出现在事件总线上
- 所有事件的时间戳均为实际运行时间，不再有 `2024-01-01` 等假时间
- 现有主线编排（executor → review-coordinator → dimension-reviewer → review-synthesizer → shipper → reporter）不受影响，正常完成

---

## Scope Boundaries

- **不包括**：为已注册 hat 设置事件频率/数量限制（这是高级防滥用，不在本次范围内）
- **不包括**：修改 preset 配置（防护是机制级的，不依赖每一层的配置启用）
- **不包括**：Retroactive 清理已有事件文件
- **不包括**：在 EventBus 中添加 payload 内容验证（已有 EventPolicy 处理）

---

## Key Decisions

- **EventBus 校验 vs 仅在 process_parse_result 校验**：两者都做（用户选择）。EventBus 确保即使有绕过 JSONL 路径的事件也能被拦截；process_parse_result 提供语义更丰富的 scope 校验（可访问 HatRegistry）
- **拒绝方式**：silent drop vs publish diagnostic event。选择 silent drop 以防止诊断事件本身被路由到不可预期的 hat 造成二次污染

---

## Dependencies / Assumptions

- EventBus 已经维护了注册 hat 的列表（`self.hats`），可以直接用于 source 校验
- `ralph emit --ts` 的现有调用方（wave emit、session_recorder）在移除参数后仍然正常工作——它们不需要固定时间戳
- `hat_registry::can_publish()` 的签名为 `fn can_publish(&self, hat_id: &HatId, topic: &str) -> bool`，修改返回策略不需要改签名

---

## 实现计划指引

给后续 ce-plan 的参考信息：

**修改文件列表：**
1. `crates/ralph-proto/src/event_bus.rs` — `publish()` 方法添加 source 注册校验
2. `crates/ralph-core/src/hat_registry.rs` — `can_publish()` 未知 hat 返回 `false`
3. `crates/ralph-core/src/event_loop/mod.rs` — `process_parse_result()` 添加 publish scope 校验层
4. `crates/ralph-cli/src/main.rs` — 移除 `--ts` 参数定义和 emit_command 中的 `ts` 处理逻辑

**测试策略：**
- `EventBus` 新增测试：未知 source hat 的 publish 被拒绝、系统事件不受限
- `HatRegistry` 修改测试：`can_publish()` 对未知 hat 返回 `false`
- `process_parse_result` 新增测试：coordinator mode 下 scope 外事件被丢弃
