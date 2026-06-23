# max_fix_rounds 真正生效 — 设计

**Date**: 2026-06-23
**Status**: Draft (待用户审阅)
**Branch**: pittcat-dev
**触发问题**: `ralph -H builtin:ce-executor-serial run` 触发 warning

```
warning: hat collection preset declared event_loop.max_fix_rounds=1 but it is
filtered by the operator/hat-collection security boundary. Set this field in
your operator ralph.yml (event_loop.*) instead, or the loop will fall back to
the framework default.
```

## 根因

`max_fix_rounds` 是 2026-06-23 在 `presets/en/ce-executor-serial.yml:81`
引入的字段,但:

1. **Rust 完全没实现**:`rg "max_fix_rounds" crates/` → 0 命中。
   `EventLoopConfig` 结构体里没有这个字段。
2. **Hat 看不到值**:`build_custom_hat` 是纯字符串拼接,无模板插值。
   hat 在 prompt 里只看到字面量 `max_fix_rounds`(变量名),**不知道等于 1**。
3. **位置对但意图错**:YAML 放在 `event_loop:` 块下,preflight 把
   `max_fix_rounds` 当作"未识别 overlay key",operator 没声明就 warn。

### 现有死字段的传播链

| 出现位置 | 受众 | 效果 |
|---|---|---|
| `presets/en/ce-executor-serial.yml:16` 注释 | 人类 | 描述用 |
| `presets/en/ce-executor-serial.yml:81` YAML | preflight + Rust | **死代码**:Rust 不读、preflight 报警 |
| `presets/en/ce-executor-serial.yml:1538, 1573, 1574` hat prompt | hat(LLM) | 只看到变量名 `max_fix_rounds`,**看不到值** |

所以 hat 实际行为不受 `max_fix_rounds: 1` 约束——它只能从 prompt 上下文推断。
这是"配置漂移 + 静默失效"双重问题。

## 设计目标

1. **真正生效**:`max_fix_rounds` 在 Rust 端可读,hat 能从 prompt 看到值
2. **消 warning**:让 preflight 认识这个字段
3. **零回归**:默认值 = 原约定值 3,其他 preset 不受影响
4. **最小改动**:不实现完整模板引擎,仅追加一个运行时配置块

## 方案

### 1. `EventLoopConfig` 新增字段

`crates/ralph-core/src/config/loop_config.rs`:

```rust
/// Maximum number of auto-fix rounds before the fix-exhausted hat path triggers.
/// Default: 3 (matches the original "Fixer applies safe_auto findings up to 3
/// rounds" convention documented in the preset YAML comments and hat prompt).
#[serde(default = "default_max_fix_rounds")]
pub max_fix_rounds: u32,

fn default_max_fix_rounds() -> u32 { 3 }
```

### 2. `InstructionBuilder` 注入 `## RUNTIME CONFIG` 块

`crates/ralph-core/src/instructions.rs`:

新增方法:

```rust
/// Injects the runtime-resolved `event_loop.*` fields that the hat
/// preset references as variables (e.g. `max_fix_rounds`) but cannot
/// see through plain text. Appended AFTER `### GUARDRAILS` so the
/// hat's own instructions remain authoritative for workflow order.
fn append_runtime_config_block(base_prompt: String, event_loop: &EventLoopConfig) -> String {
    format!(
        "{base_prompt}\n\n## RUNTIME CONFIG\n\
         The following values are resolved at loop start and apply to this iteration:\n\
         - max_fix_rounds: {n}\n",
        n = event_loop.max_fix_rounds,
    )
}
```

`build_custom_hat` 在返回前调用一次。

### 3. preflight 白名单

`crates/ralph-cli/src/preflight.rs:723` 把 `max_fix_rounds` 加入
`PRESET_OPT_IN_WHEN_OPERATOR_OMITS`:

```rust
const PRESET_OPT_IN_WHEN_OPERATOR_OMITS: &[&str] = &[
    "state_projection",
    "hat_handoff",
    "suppress_human_guidance",
    "workflow_contract",
    "ephemeral_isolation",
    "enforce_current_unit",
    "max_fix_rounds",  // NEW
];
```

operator 不声明 → preset 生效;operator 声明 → operator 赢。
与其他预算字段(max_runtime_seconds 等)语义一致。

### 4. preset YAML

`presets/en/ce-executor-serial.yml`:

- **保留** line 81 的 `max_fix_rounds: 1`(用户要求"位置不变")—— 现在
  preflight 认识它,不再 warn;Rust 能读它;hat 能从 prompt 看到。
- **更新** line 1538、1573、1574 的注释,加一句 "Read the actual value from
  the `## RUNTIME CONFIG` block at the bottom of your prompt."

### 5. 不在范围内

- 不实现通用模板引擎(其他 hat prompt 中的 `< xxx >` 字面量继续是字面量)
- 不引入新依赖
- 不改 `ce-executor-serial.yml` 之外其他 preset(都不引用 `max_fix_rounds`)

## 文件改动清单

| 文件 | 改动 |
|---|---|
| `crates/ralph-core/src/config/loop_config.rs` | + `max_fix_rounds: u32` 字段 |
| `crates/ralph-core/src/config/mod.rs` | 暴露新字段(如有 re-export) |
| `crates/ralph-core/src/instructions.rs` | 新增 `append_runtime_config_block`,在 `build_custom_hat` 调用 |
| `crates/ralph-cli/src/preflight.rs` | `PRESET_OPT_IN_WHEN_OPERATOR_OMITS` 加 `"max_fix_rounds"` |
| `presets/en/ce-executor-serial.yml` | line 1538/1573/1574 注释加 `## RUNTIME CONFIG` 提示 |
| `crates/ralph-core/src/config/loop_config.rs`(测试) | 默认值 3,serde 反序列化正确 |
| `crates/ralph-core/src/instructions.rs`(测试) | `append_runtime_config_block` 输出含 `max_fix_rounds: N` |
| `crates/ralph-cli/src/preflight.rs`(测试) | `ce-executor-serial` 不再触发 `max_fix_rounds` warning |

## 验证

```bash
# 1. 单包测试
cargo nextest run -p ralph-core -- instructions
cargo nextest run -p ralph-core -- config
cargo nextest run -p ralph-cli -- preflight

# 2. 集成测试
cargo nextest run -p ralph-cli -- ce_executor_recovery

# 3. 回归基线(ralph-cli 串行,其他并行)
./scripts/run-tests.sh

# 4. 端到端冒烟
ralph -H builtin:ce-executor-serial run --help 2>&1 | grep -i warning
# 预期:无 "max_fix_rounds" warning
```

## 风险

- **注入位置**:追加在 `### GUARDRAILS` 之后,不会改变现有模板 token 顺序。
  任何读 prompt 头部位置的代码不受影响。
- **默认值 = 3**:等于 fixer hat 注释 "up to 3 rounds"。`ce-executor-serial`
  当前设 `1`—— Rust 现在真的会读它。**修复前是死代码;修复后真的限 1 轮**。
  这是**行为变更**但符合 2026-06-23 注释"set to 3 for multi-round behavior"
  的意图。
- **如果 operator ralph.yml 没设 max_fix_rounds**:以前是死代码(默认 0 个字段,
  反正 Rust 不读);现在会默认 3 轮。如果以前有哪个 hat 隐式依赖"无上限",
  现在会变成 3 轮。**这种依赖是 fragile 的**,我推荐坚持。
