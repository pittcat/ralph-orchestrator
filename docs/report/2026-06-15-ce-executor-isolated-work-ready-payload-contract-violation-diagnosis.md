# work.ready Payload Contract Violation 根因诊断

> 📅 2026-06-15 | 🔖 ce-executor-isolated preset

---

## 1. 总体结论

| 维度 | 状态 | 说明 |
|------|------|------|
| 问题定位 | 🟢 已确认 | 完整错误链条已追踪到根因 |
| 修复方案 | 🟢 已明确 | 两处代码修改即可修复 |
| 风险等级 | 🟢 低 | 仅影响 coordinator 首次激活时的 work.ready 发射 |

**一句话总结**：`work.ready` payload contract violation 的根本原因不是 schema 定义错误，而是 **coordinator hat 的 prompt instructions 中缺少 `--json` 格式示例**，导致 LLM 以纯字符串形式发射 payload，与 schema 要求的 `payload: json_object` 不匹配。

---

## 2. 完整错误链条

```
Agent 读取 instructions:
  "Publish work.ready with payload: plan_path, plan_name, ..."
  (没有给出 `--json` 示例，只有字段描述)

Agent 执行:
  ralph emit work.ready "U1 scaffold 启动（v5 baseline @ 34970fd）：..."
  ↑ 没有 --json 标志

emit.rs:460-484 — payload 序列化:
  payload 不以 '{' 开头 → looks_like_json=false → Value::String("U1 scaffold...")
  ↑ 走分支 D（兜底字符串）

写入 events.jsonl:
  {"topic":"work.ready","payload":"U1 scaffold 启动...","hat":"coordinator"}

event_policy.rs:485-539 — schema 验证:
  schema 要求 payload: json_object
  serde_json::from_str::<Value>("U1 scaffold...") → Err
  → PayloadTypeMismatch { expected: "json_object", actual: "parse error" }

ViolationType::field() 返回 None:
  PayloadTypeMismatch 不是 field-scoped 违规
  → 诊断输出 "field": null（令人困惑但设计如此）

Loop 终止: not_retriable
```

---

## 3. 根因三要素

### 3a. Coordinator Instructions 缺少 `--json` 示例

**presets/en/ce-executor-isolated.yml:397-405** — coordinator 段的 Event Publishing 只列出了 payload 字段，没有给出完整的 `ralph emit work.ready --json '...'` 命令示例：

```yaml
### Event Publishing
- Publish `work.ready` with payload:
  - `plan_path`, `plan_name`, `complexity`
  - `task_id`: ...
  - `task_key`: ...
  - `step`: ...
  - `preflight_checks`: ...
- Stop — do not continue to other work
```

**对比**: executor 段有完整的 copy-paste 示例（L472-476）：
```yaml
ralph emit work.done --json '{"plan_name":"my-plan","plan_path":"...",...}'
```

### 3b. 通用 Prompt 模板默认为字符串格式

**crates/ralph-core/src/instructions.rs:163** — `build_custom_hat` 方法的 `must_publish` 段使用 `<summary>` 格式，暗示 LLM 使用字符串：

```
You MUST emit exactly ONE of these events via `ralph emit "<topic>" "<summary>"`: `work.ready`
Use `ralph emit "work.ready" "<summary>"` as the pattern.
```

### 3c. `looks_like_json` 自动检测不覆盖自然语言

**crates/ralph-cli/src/commands/emit.rs:117-120** — 自动检测函数只检查 payload 是否以 `{` 或 `[` 开头：

```rust
pub fn looks_like_json(payload: &str) -> bool {
    let trimmed = payload.trim_start();
    trimmed.starts_with('{') || trimmed.starts_with('[')
}
```

中文文本不以 `{` 开头 → 自动检测失败 → 回到 `Value::String`。

---

## 4. 三处修复建议

### 修复 1（高优先级）— Coordinator Instructions 添加 `--json` 示例

在 `presets/en/ce-executor-isolated.yml` coordinator 段末尾添加：

```yaml
      ### Event Publishing
      - Publish `work.ready` with payload:
        - `plan_path`, `plan_name`, `complexity`
        - `task_id`: ...
        - `task_key`: ...
        - `step`: ...
        - `preflight_checks`: ...
      - Copy-pasteable example:
        ```bash
        ralph emit work.ready --json '{"plan_name":"my-plan","plan_path":"docs/plans/my-plan.md","task_id":"task-xxx","task_key":"ce-executor:my-plan:step-01:u0-impl","step":"step-01","complexity":"small"}'
        ```
      - Stop — do not continue to other work
```

### 修复 2（中优先级）— `build_custom_hat` 模板根据 schema 动态选择格式

在 `crates/ralph-core/src/instructions.rs` 中，让 `must_publish` 模板感知 schema 的 `payload` 类型：当 topic 的 schema 要求 `json_object` 时，提示使用 `--json` 格式而非 `<summary>` 格式。

### 修复 3（低优先级）— 诊断输出改进

在诊断渲染层将 `PayloadTypeMismatch` 的 `field: None` 显示为 `"field": "payload"` 或 `"field": "(entire payload)"`，避免 `null` 造成的困惑。

---

## 5. 需要您拍板的事

1. **修复范围**
   - 选项 A：只修复 coordinator instructions（最小改动，5 分钟）
   - 选项 B：修复 coordinator instructions + `build_custom_hat` 模板（彻底修复，但涉及 Rust 编译）
   - **建议**：选项 B。只修 instructions 治标不治本——下一个有 `json_object` schema 的 hat 仍可能踩同一个坑。

2. **诊断输出改进**
   - 是否值得为 `PayloadTypeMismatch` 单独改 `field()` 渲染？目前 `field: null` 不影响机器解析（recovery.jsonl 的 reason_code 已足够定位问题），仅影响人读体验。
   - **建议**：低优先级。可以提交一个单独的 PR 顺手修掉。

---

## 6. 下一步计划

1. 修复 coordinator instructions（添加 `--json` 示例）
2. 修复 `build_custom_hat` 模板（schema-aware 格式选择）
3. 重新运行 ce-executor-isolated preset 验证 work.ready 通过 contract
4. （可选）改进诊断输出中 PayloadTypeMismatch 的 field 渲染

---

## 附录：技术详情

<details>
<summary>展开查看技术细节</summary>

### 相关代码位置

| 组件 | 文件 | 行号 |
|------|------|------|
| Payload 序列化 | `crates/ralph-cli/src/commands/emit.rs` | 460-484 |
| `looks_like_json` | `crates/ralph-cli/src/commands/emit.rs` | 117-120 |
| `payload: json_object` 验证 | `crates/ralph-core/src/event_policy.rs` | 485-539 |
| `ViolationType::field()` 返回 None | `crates/ralph-core/src/event_policy.rs` | 53-64 |
| `build_custom_hat` 模板 | `crates/ralph-core/src/instructions.rs` | 136-220 |
| Coordinator instructions | `presets/en/ce-executor-isolated.yml` | 397-405 |
| Executor `--json` 示例（可参考） | `presets/en/ce-executor-isolated.yml` | 472-476 |
| `EventSchema` 定义 | `crates/ralph-core/src/config/loop_config.rs` | 26-36 |

### 错误证据

```
recovery.jsonl:
  severity: critical
  reason_code: payload_contract_violation
  topic: work.ready
  field: null
  outcome: not_retriable

payload-contract-error JSON:
  error_type: payload_type_mismatch
  payload_excerpt: "U1 scaffold 启动（v5 baseline @ 34970fd）..."
```

### 诊断文件路径

- `.ralph/loop-termination-reason.json`
- `.ralph/diagnostics/payload-contract-error-*.json`
- `.ralph/diagnostics/2026-06-15T15-26-18/recovery.jsonl`
- `.ralph/diagnostics/2026-06-15T15-26-18/diagnosis-summary.json`

</details>