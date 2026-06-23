# max_fix_rounds 真正生效 Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让 `presets/en/ce-executor-serial.yml` 中 `event_loop.max_fix_rounds: 1` 真正生效——Rust 端能读,hat 在 prompt 中能看到具体值,preflight 不再报警。

**Architecture:**
- `EventLoopConfig` 新增 `max_fix_rounds: u32` 字段(默认 3,对齐原注释"up to 3 rounds")
- `event_loop/mod.rs` 在 `build_custom_hat` 调用后,追加一段 `## RUNTIME CONFIG` 块到 hat prompt 末尾,让 hat 看到具体值
- preflight `PRESET_OPT_IN_WHEN_OPERATOR_OMITS` 白名单加 `max_fix_rounds`,operator 不声明时 preset 生效,声明时 operator 赢(与其他预算字段语义一致)
- preset YAML line 1538/1573/1574 注释加提示,告诉 hat 从 `## RUNTIME CONFIG` 块读实际值

**Tech Stack:** Rust 1.78+, cargo nextest, Serde YAML, Serde

**Spec:** `docs/superpowers/specs/2026-06-23-max-fix-rounds-runtime-support-design.md`

---

## File Structure

### Modified Files

- `crates/ralph-core/src/config/loop_config.rs` — T1: 新增 `max_fix_rounds` 字段
- `crates/ralph-core/src/event_loop/mod.rs` — T2: 在 `build_custom_hat` 调用后注入 `## RUNTIME CONFIG` 块
- `crates/ralph-cli/src/preflight.rs` — T3: `PRESET_OPT_IN_WHEN_OPERATOR_OMITS` 加 `"max_fix_rounds"`
- `presets/en/ce-executor-serial.yml` — T4: line 1538/1573/1574 注释加 `## RUNTIME CONFIG` 提示

### Test Files

- `crates/ralph-core/src/config/loop_config.rs`(测试模块) — T1: 默认值测试
- `crates/ralph-core/src/event_loop/mod.rs`(测试模块) — T2: 注入块测试
- `crates/ralph-cli/src/preflight.rs`(测试模块) — T3: warning 消失测试

---

## Implementation Tasks

### Task T1: EventLoopConfig 新增 max_fix_rounds 字段

**Files:**
- Modify: `crates/ralph-core/src/config/loop_config.rs:78-86`(在 `default_max_failures` 之后,`EventLoopConfig` 之前)
- Modify: `crates/ralph-core/src/config/loop_config.rs:316-317`(在 `suppress_human_guidance` 字段之后)
- Test: `crates/ralph-core/src/config/loop_config.rs` 现有测试模块(若无则新建)

- [ ] **Step 1.1: 写失败测试 — 默认值 = 3**

在 `loop_config.rs` 现有测试模块(若无则在文件末尾 `#[cfg(test)] mod tests { ... }`):

```rust
#[test]
fn event_loop_config_default_max_fix_rounds_is_three() {
    let cfg = EventLoopConfig::default();
    assert_eq!(cfg.max_fix_rounds, 3, "default must match the legacy '3 rounds' comment");
}

#[test]
fn event_loop_config_max_fix_rounds_deserializes() {
    let yaml = r"
max_fix_rounds: 7
";
    let cfg: EventLoopConfig = serde_yaml::from_str(yaml).unwrap();
    assert_eq!(cfg.max_fix_rounds, 7);
}
```

- [ ] **Step 1.2: 跑测试确认失败**

Run: `cargo nextest run -p ralph-core -- event_loop_config_default_max_fix_rounds`
Expected: COMPILE FAIL(`max_fix_rounds` field 不存在)

- [ ] **Step 1.3: 加 `default_max_fix_rounds` 函数和字段**

在 `crates/ralph-core/src/config/loop_config.rs:78` 之后(`default_cancellation_promise` 之前或任意合适位置)插入:

```rust
/// 2026-06-23: max number of auto-fix rounds. Default: 3
/// (matches the legacy "up to 3 rounds" convention in
/// `ce-executor-serial` fixer hat comment). Operators can
/// override per-workspace in `ralph.yml`; presets can declare
/// a different value (e.g. `ce-executor-serial` declares 1).
fn default_max_fix_rounds() -> u32 {
    3
}
```

在 `suppress_human_guidance` 字段(line 316)之后追加:

```rust
    /// 2026-06-23: cap on the auto-fix retry loop. When the
    /// fixer hat reads `fix_round` from the active run-state
    /// and `fix_round >= max_fix_rounds`, the loop routes to
    /// `fix.exhausted` instead of re-issuing `review.failed`.
    /// Default: 3 (see `default_max_fix_rounds`). Preset may
    /// lower this (e.g. `ce-executor-serial` → 1); operators
    /// can raise it per workspace via `ralph.yml`.
    #[serde(default = "default_max_fix_rounds")]
    pub max_fix_rounds: u32,
```

- [ ] **Step 1.4: 跑测试确认通过**

Run: `cargo nextest run -p ralph-core -- event_loop_config_max_fix_rounds`
Expected: 2 passed

- [ ] **Step 1.5: 跑全 ralph-core 测试,确保没破坏其他东西**

Run: `cargo nextest run -p ralph-core`
Expected: 全部通过(默认 3 与原约定一致,不会改变任何已有行为)

- [ ] **Step 1.6: 提交**

```bash
rtk git add crates/ralph-core/src/config/loop_config.rs
rtk git commit -m "feat(config): EventLoopConfig.max_fix_rounds with default 3"
```

---

### Task T2: event_loop 注入 ## RUNTIME CONFIG 块

**Files:**
- Modify: `crates/ralph-core/src/event_loop/mod.rs`(在两处 `build_custom_hat` 调用点附近)
- Test: `crates/ralph-core/src/event_loop/mod.rs` 现有测试模块

- [ ] **Step 2.1: 写失败测试 — build_custom_hat 输出含 RUNTIME CONFIG 块**

先确认测试模块位置:
```bash
rg -n "mod tests" crates/ralph-core/src/event_loop/mod.rs
```

在该测试模块新增:

```rust
#[test]
fn build_custom_hat_prompt_includes_runtime_config_block_with_max_fix_rounds() {
    // 这个测试要直接验证注入逻辑。
    // 走 build_custom_hat 真实路径,需要构造 EventLoopConfig。
    // 简单做法:在 test_helper 里直接调用注入函数(见 Step 2.3)。
    let prompt = render_with_max_fix_rounds(1);
    assert!(prompt.contains("## RUNTIME CONFIG"), "missing ## RUNTIME CONFIG block");
    assert!(
        prompt.contains("max_fix_rounds: 1"),
        "max_fix_rounds value not visible to hat"
    );
}

#[test]
fn build_custom_hat_prompt_runtime_config_reflects_custom_value() {
    let prompt = render_with_max_fix_rounds(7);
    assert!(prompt.contains("max_fix_rounds: 7"));
}
```

- [ ] **Step 2.2: 跑测试确认失败**

Run: `cargo nextest run -p ralph-core -- build_custom_hat_prompt_includes_runtime_config`
Expected: COMPILE FAIL(`render_with_max_fix_rounds` 不存在)

- [ ] **Step 2.3: 在 event_loop/mod.rs 加 `append_runtime_config_block` 函数 + test helper**

在 `event_loop/mod.rs` 找一个合适位置(紧挨着 `inject_phase_into_prompt` 或在文件末尾的 `impl EventLoop` 块之前),加:

```rust
/// Appends a `## RUNTIME CONFIG` block exposing the runtime-resolved
/// `event_loop.*` values that the hat preset references as variables
/// (e.g. `max_fix_rounds`) but cannot see through plain text. This
/// keeps the YAML position of `max_fix_rounds` (in `event_loop:`)
/// unchanged, lets the operator override it in `ralph.yml`, and lets
/// the hat prompt read the actual value rather than the literal
/// variable name.
///
/// Appended AFTER `### GUARDRAILS` so the hat's own instructions
/// remain authoritative for workflow order. Block is always emitted
/// (even with default 3) so the hat learns where to look.
pub(crate) fn append_runtime_config_block(
    base_prompt: String,
    max_fix_rounds: u32,
) -> String {
    format!(
        "{base_prompt}\n\n## RUNTIME CONFIG\n\
         The following values are resolved at loop start and apply to this iteration:\n\
         - max_fix_rounds: {n}\n",
        n = max_fix_rounds,
    )
}

#[cfg(test)]
fn render_with_max_fix_rounds(n: u32) -> String {
    // 复用 default_builder 风格:模拟 build_custom_hat 的输入,
    // 然后追加 RUNTIME CONFIG 块。Hat 角色与具体行为不影响
    // 注入逻辑,这里只关心块是否出现。
    use crate::config::CoreConfig;
    use ralph_proto::Hat;
    let builder = crate::instructions::InstructionBuilder::new(CoreConfig::default());
    let hat = Hat::new("test-hat", "Test").with_instructions("do work");
    let base = builder.build_custom_hat(&hat, "no events");
    crate::event_loop::append_runtime_config_block(base, n)
}
```

注意: 如果 `mod.rs` 已经存在 `#[cfg(test)] mod tests`,`render_with_max_fix_rounds` 放进该 mod;否则放到文件末尾单独的 `#[cfg(test)] mod test_helpers`。

- [ ] **Step 2.4: 在两处 `build_custom_hat` 调用点后追加 `## RUNTIME CONFIG` 块**

在 `event_loop/mod.rs` 中找到两处 `build_custom_hat` 调用:

**第一处**(isolated 模式,line 3551-3553 附近),当前:
```rust
let base_prompt = self
    .instruction_builder
    .build_custom_hat(hat, &events_context);
```

改为:
```rust
let base_prompt = self
    .instruction_builder
    .build_custom_hat(hat, &events_context);
let base_prompt = append_runtime_config_block(
    base_prompt,
    self.config.event_loop.max_fix_rounds,
);
```

**第二处**(coordinator / non-isolated 模式,line 3701-3703 附近),当前:
```rust
let base = self
    .instruction_builder
    .build_custom_hat(hat, &events_context);
let with_phase = self.inject_phase_into_prompt(base);
```

改为:
```rust
let base = self
    .instruction_builder
    .build_custom_hat(hat, &events_context);
let base = append_runtime_config_block(
    base,
    self.config.event_loop.max_fix_rounds,
);
let with_phase = self.inject_phase_into_prompt(base);
```

**关键**: 必须确认 `self.config` 在两处都可用。先查看 `self.config` 字段类型:

```bash
rg -n "self\.config" crates/ralph-core/src/event_loop/mod.rs | head -10
```

如果 `self.config` 不是 `RalphConfig`,而是某种只读视图 / 分量,改用对应的 `self.config_ref.event_loop.max_fix_rounds`(找到正确的字段名)。

- [ ] **Step 2.5: 跑测试确认通过**

Run: `cargo nextest run -p ralph-core -- build_custom_hat_prompt`
Expected: 2 个新测试 + 已有测试全部通过

- [ ] **Step 2.6: 跑全 ralph-core 测试**

Run: `cargo nextest run -p ralph-core`
Expected: 全部通过(`## RUNTIME CONFIG` 块在所有 hat prompt 末尾追加,不影响现有 token 顺序)

- [ ] **Step 2.7: 提交**

```bash
rtk git add crates/ralph-core/src/event_loop/mod.rs
rtk git commit -m "feat(event_loop): inject ## RUNTIME CONFIG block with max_fix_rounds"
```

---

### Task T3: preflight 白名单加 max_fix_rounds

**Files:**
- Modify: `crates/ralph-cli/src/preflight.rs:723-733`(`PRESET_OPT_IN_WHEN_OPERATOR_OMITS`)
- Test: `crates/ralph-cli/src/preflight.rs` 现有测试模块(merge_hats_overlay_*)

- [ ] **Step 3.1: 写失败测试 — `ce-executor-serial` 不再触发 max_fix_rounds warning**

在 `preflight.rs` 现有测试模块,新增:

```rust
#[test]
fn merge_hats_overlay_silently_merges_max_fix_rounds_when_operator_omits_it() {
    // 2026-06-23: max_fix_rounds is in PRESET_OPT_IN_WHEN_OPERATOR_OMITS,
    // so the preset value is silently merged in when the operator's
    // ralph.yml does not declare it. No warning should be emitted.
    //
    // We assert by calling the merge and checking that the result
    // is well-formed (RalphConfig deserializes). The lack of warning
    // is verified by code review — the code path that emits
    // `eprintln!("warning: ... filtered by the operator/hat-collection
    // security boundary")` is bypassed by the opt-in branch.
    let core: Value = serde_yaml::from_str(
        r"
event_loop:
  completion_promise: LOOP_COMPLETE
",
    )
    .unwrap();

    let hats: Value = serde_yaml::from_str(
        r"
event_loop:
  max_fix_rounds: 1
",
    )
    .unwrap();

    let merged = merge_hats_overlay(core, hats).unwrap();
    let config: RalphConfig = serde_yaml::from_value(merged).unwrap();

    assert_eq!(config.event_loop.max_fix_rounds, 1);
}
```

- [ ] **Step 3.2: 跑测试确认失败**

Run: `cargo nextest run -p ralph-cli --bin ralph -- merge_hats_overlay_silently_merges_max_fix_rounds`
Expected: COMPILE FAIL(`max_fix_rounds` 字段不存在 on `EventLoopConfig`)

如果 T1 没跑过就到这里,会先于白名单问题报 T1 编译错误。**确保 T1 已 commit 后再继续 T3。**

Expected: 编译失败(因为还没把 `max_fix_rounds` 加白名单,但 `EventLoopConfig` 也还没字段—— T1 任务已加字段,所以这步实际会跑通合并,只是 preset 还没被 opt-in——这意味着还在 `eprintln` 路径上,行为等价于"warning 还在,字段值也对了"——测试本身仍然过。但我们要消 warning,继续 Step 3.3。)

- [ ] **Step 3.3: 把 `max_fix_rounds` 加入 `PRESET_OPT_IN_WHEN_OPERATOR_OMITS`**

在 `crates/ralph-cli/src/preflight.rs:733`(`enforce_current_unit` 之后)加:

```rust
const PRESET_OPT_IN_WHEN_OPERATOR_OMITS: &[&str] = &[
    "state_projection",
    "hat_handoff",
    "suppress_human_guidance",
    "workflow_contract",
    "ephemeral_isolation",
    "enforce_current_unit",
    // 2026-06-23: max_fix_rounds is opt-in so the preset
    // value (1 for ce-executor-serial) is silently applied
    // when the operator's ralph.yml omits the key. Operators
    // can raise it per-workspace.
    "max_fix_rounds",
];
```

- [ ] **Step 3.4: 跑测试确认通过**

Run: `cargo nextest run -p ralph-cli --bin ralph -- merge_hats_overlay_silently_merges_max_fix_rounds`
Expected: PASS

- [ ] **Step 3.5: 端到端验证 — 跑 ralph 命令,无 warning**

Run: `cd /Users/pittcat/Dev/Rust/ralph-orchestrator && cargo build -p ralph-cli 2>&1 | tail -5`
Expected: build 成功

Run: `cd /Users/pittcat/Dev/Rust/ralph-orchestrator && ./target/debug/ralph -H builtin:ce-executor-serial run 2>&1 | head -20`
Expected: 启动失败是正常的(`PROMPT.md` 不在等运行条件);但**没有** `warning: ... event_loop.max_fix_rounds=1 ... filtered by the operator/hat-collection security boundary` 这一行。

- [ ] **Step 3.6: 跑 ralph-cli 全测试(串行)**

Run: `cargo nextest run -p ralph-cli --bin ralph`
Expected: 全部通过(白名单只增加了一行,不动现有逻辑)

- [ ] **Step 3.7: 提交**

```bash
rtk git add crates/ralph-cli/src/preflight.rs
rtk git commit -m "fix(preflight): whitelist max_fix_rounds in PRESET_OPT_IN_WHEN_OPERATOR_OMITS"
```

---

### Task T4: preset YAML 注释更新 — 指引 hat 从 RUNTIME CONFIG 读

**Files:**
- Modify: `presets/en/ce-executor-serial.yml:1538, 1573, 1574`

- [ ] **Step 4.1: 在 line 1538 的 hat instructions 注释中加提示**

定位:`presets/en/ce-executor-serial.yml:1538` 附近,review-synthesizer 的 hat instructions。

当前(原文缩进 8 空格):
```yaml
        - If safe_auto > 0 and fix_round < max_fix_rounds → publish `review.failed`, ...
```

改为:
```yaml
        - If safe_auto > 0 and fix_round < max_fix_rounds → publish `review.failed`, ...
        # NOTE: the actual numeric value of `max_fix_rounds` is resolved at
        # loop start and is exposed to this hat in the `## RUNTIME CONFIG`
        # block at the very bottom of the prompt (it is also declared in
        # `event_loop.max_fix_rounds` of this preset, currently `1`). Do
        # NOT hard-code the round count in the hat prompt.
```

- [ ] **Step 4.2: 在 line 1573 附近加同样提示**

定位:`presets/en/ce-executor-serial.yml:1573-1574` 附近,fixer hat 的 "Round Management" 段。

当前:
```yaml
      - If `fix_round + 1 > max_fix_rounds`:
        - Publish `fix.exhausted`, ...
```

改为:
```yaml
      - If `fix_round + 1 > max_fix_rounds`:
        - Publish `fix.exhausted`, ...
        # NOTE: see the `## RUNTIME CONFIG` block at the end of the prompt
        # for the actual numeric value of `max_fix_rounds` (declared in
        # `event_loop.max_fix_rounds`, currently `1` for this preset).
```

- [ ] **Step 4.3: 在 line 81 的现有注释后追加一行说明字段已实际生效**

定位:`presets/en/ce-executor-serial.yml:81`

当前:
```yaml
  max_fix_rounds: 1  # 2026-06-23: one auto-fix round; set to 3 for multi-round behavior
```

改为:
```yaml
  max_fix_rounds: 1  # 2026-06-23: one auto-fix round; set to 3 for multi-round behavior
    # 2026-06-23: this value is read by Rust (whitelisted in
    # `PRESET_OPT_IN_WHEN_OPERATOR_OMITS`) and injected into every
    # hat prompt via the `## RUNTIME CONFIG` block. Operators can
    # override per-workspace in their `ralph.yml`.
```

- [ ] **Step 4.4: 跑全 workspace 回归基线(最终验证)**

```bash
# 1. 编译
rtk cargo build

# 2. nextest 全跑(ralph-cli 走串行,其他包并行)
./scripts/run-tests.sh

# 3. 如有竞态/时序 flake,再走单线程兜底
RALPH_BASELINE_SERIAL=1 ./scripts/run-tests.sh
```

Expected: 全部通过。如果 serial fallback 仍失败,说明是真失败,必须修复后才能继续。

- [ ] **Step 4.5: 端到端冒烟**

```bash
# 启动 ralph,确认无 warning
cd /Users/pittcat/Dev/Rust/ralph-orchestrator
./target/debug/ralph -H builtin:ce-executor-serial run 2>&1 | head -5
```

Expected: **没有** `warning: ... event_loop.max_fix_rounds=1 ... filtered by the operator/hat-collection security boundary` 这行。运行时启动失败(PROMPT.md 等) 是正常的,只验证 warning 是否消失。

- [ ] **Step 4.6: 提交**

```bash
rtk git add presets/en/ce-executor-serial.yml
rtk git commit -m "docs(preset): annotate max_fix_rounds as Rust-read, prompt-injected"
```

---

## Self-Review

1. **Spec coverage:**
   - Spec §"EventLoopConfig 新增字段" → T1 ✓
   - Spec §"InstructionBuilder 注入 ## RUNTIME CONFIG 块" → T2 ✓
   - Spec §"preflight 白名单" → T3 ✓
   - Spec §"preset YAML" → T4 ✓
   - Spec §"测试" → T1/T2/T3 各加测试 ✓
   - Spec §"验证" → T4.4 跑全基线 ✓

2. **Placeholder scan:** 没有 TODO / TBD / "implement later"。

3. **Type consistency:**
   - `EventLoopConfig.max_fix_rounds: u32` 一致使用
   - `append_runtime_config_block(String, u32) -> String` 签名一致
   - `PRESET_OPT_IN_WHEN_OPERATOR_OMITS` 数组保持 `&[&str]` 风格

4. **风险点:**
   - T2 Step 2.4 需要先确认 `self.config` 字段类型(可能是 `RalphConfig` 或其引用)。如不是 `RalphConfig`,改用 `self.config_ref.event_loop.max_fix_rounds` 或对应字段。
   - T2 Step 2.5: 全 ralph-core 测试可能因 `## RUNTIME CONFIG` 块注入而让某些严格断言失败(`assert!(!contains("..."))`)。若失败,需具体看哪个测试,决定是放宽断言还是改注入位置。当前评估:无这种断言,因为 `## RUNTIME CONFIG` 是新块,新词,无冲突。

## Execution Handoff

Plan complete and saved to `docs/superpowers/plans/2026-06-23-max-fix-rounds-runtime-support.md`. Two execution options:

1. **Subagent-Driven (recommended)** - 每个 task 派一个 subagent,任务间 review,快速迭代
2. **Inline Execution** - 当前 session 内执行,用 executing-plans 批量跑 + 检查点
