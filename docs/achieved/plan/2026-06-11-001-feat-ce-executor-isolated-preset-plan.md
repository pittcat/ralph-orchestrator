# feat: ce-executor-isolated preset — 用 isolated mode 从架构层消除 hat impersonation

**Date:** 2026-06-11
**Status:** active
**Origin:** `docs/brainstorms/2026-06-11-ce-executor-hat-impersonation-deep-guard-requirements.md`
**Supersedes:** `docs/plans/2026-06-10-002-fix-ce-executor-hat-impersonation-remaining-guards-plan.md`

---

## 问题

ce-executor preset 的 coordinator mode 存在结构性 impersonation 问题：ralph（中央协调器）持续持有上下文，agent 在单个 prompt 中看到所有 hat 的 instructions，导致 agent 能以错误身份发事件。此前 4 次 P0/P1 fix 仍复发，证明白名单 deny 模式追不上攻击面扩张。

## 方案

不继续在 coordinator mode 上叠加 5 层防御（brainstorm 的 U1-U5），而是**新建 `ce-executor-isolated` preset**，启用 `execution_mode: isolated`，从架构层消除 impersonation 根因。同时仅移植 U1 的 `loop_invariant_assertion` 作为 stall recovery 注入路径的 defense-in-depth。

### 为什么 isolated mode 解决 impersonation

| 维度 | Coordinator mode（现状） | Isolated mode |
|---|---|---|
| `next_hat()` | 永远返回 `ralph` | 返回有 pending 事件的具体 hat |
| Prompt 构建 | 所有 hat instructions 注入一个 prompt | 每个 hat 只看到自己的 instructions + 自己的事件 |
| 事件 scope | `topic_deny_rules` 白名单（易漏） | `can_publish()` 硬检查：事件 topic 必须在当前 hat 的 `publishes` 列表中 |
| 单 iter 事件数 | 无限制 | 仅接受 1 个业务事件 + 系统事件 |
| `hat` 语义 | 过载（进程/角色/发布者混用） | 清晰（= 当前执行 hat） |

### 与 brainstorm 方案的关系

- **U1（机制层：loop_invariant_assertion）**：✅ 移植简化版（仅 INV-1/2/3，无 INV-4，无 strict_mode）
- **U2（演练层：BDD scenarios）**：❌ 不实现。isolated mode 的 scope check 由 `can_publish` 保障，不需要 regression scenarios
- **U3（数据层：`_source` 字段）**：❌ 不实现。isolated mode 下事件来源天然清晰
- **U4（架构层：events.jsonl 拆分）**：❌ 不需要。isolated mode 已消除 impersonation
- **U5（语义层：hat/role 拆分）**：❌ 不需要。isolated mode 下 `hat = 真实 hat_id`

---

## 实施单元

### U1. 创建 `ce-executor-isolated.yml` preset

**Goal:** 创建基于 isolated mode 的 ce-executor preset，与现有 coordinator mode 版共存

**Dependencies:** 无

**Files:**
- `presets/en/ce-executor-isolated.yml` — 新建（从 `presets/en/ce-executor.yml` 复制并修改）

**Approach:**
1. 复制 `ce-executor.yml` → `ce-executor-isolated.yml`
2. 在 `event_loop:` 下增加 `execution_mode: isolated`
3. 修改 instruction 中两处 "COORDINATOR MODE" 标题 → "EXECUTION MODE"（仅标题，不影响行为）
4. **保留所有既有保护措施不变：**
   - `topic_deny_rules`（继续作为 defense-in-depth）
   - `enforce_hat_scope: true`
   - `plan_name_equality_required: true`
   - `event_policy` schemas
   - `verdict_gate`
   - `execution_contracts`
   - `coordinator_hats`
5. **Instructions 不需要重写：** 分析确认 ce-executor 的 instructions 已足够自包含——上下文传递依赖文件系统（`progress.md`、`plan.md`、`context.md`）和 event payloads，而非"同 prompt 共享上下文"

**Test scenarios:**
- **Preset 解析成功：** 加载 `ce-executor-isolated` preset，assert `event_loop.execution_mode == HatExecutionMode::Isolated`
- **Preset 10 hat 拓扑完整：** assert 与 ce-executor 的 hat 数量、triggers、publishes 一致
- **Preset 与源文件差异可控：** assert 只有 `execution_mode` 字段和 "COORDINATOR→EXECUTION" 标题两个差异点

**Verification:**
- `cargo test -p ralph-cli` 中 preset 相关测试全部 pass
- `cargo test -p ralph-core` 中 `test_hat_execution_mode_explicit_isolated` pass

---

### U2. 注册 preset 到构建系统

**Goal:** 让 `ce-executor-isolated` 成为可用的 builtin preset

**Dependencies:** U1

**Files:**
- `presets/manifest.yml` — 在 `embedded:` 列表添加 `ce-executor-isolated`
- `crates/ralph-cli/src/presets.rs` — 添加 `EmbeddedPreset` 条目 + `presets_array_matches_manifest` 会自动校验
- `presets/index.json` — 添加可见条目（public preset）
- `scripts/ralph-zsh-plugin.zsh` — 添加 `"builtin:ce-executor-isolated"` 和 description 到 `_RALPH_BUILTIN_HAT_VALUES` / `_RALPH_BUILTIN_HAT_DESCRIPTIONS`
- `crates/ralph-cli/build.rs` — 无需手动修改（它自动读取 manifest.yml；但需验证不会因新文件名产生问题）

**Approach:**
1. 按现有模式在 4 处注册。`presets.rs` 的 `PRESETS` 数组用 `include_str!(concat!(env!("OUT_DIR"), "/presets/ce-executor-isolated.yml"))`
2. `build.rs` 已经实现了从 manifest.yml 读取 `embedded` 列表并复制文件到 `$OUT_DIR/presets/`——将 `ce-executor-isolated` 加入 manifest 后自动生效
3. 执行 `cargo build` 验证编译通过，新 preset 可被 `preset_names()` 返回
4. 同步更新 `CLAUDE.md` 和 `AGENTS.md` 的 builtin preset 列表

**Test scenarios:**
- **Preset 列表包含新条目：** `list_presets()` 返回中包含 `ce-executor-isolated`
- **Preset 内容有效 YAML：** `test_preset_content_is_valid_yaml` pass
- **Preset 有 description：** `test_all_presets_have_description` pass
- **manifest 与 presets.rs 一致：** `presets_array_matches_manifest` pass
- **index.json 有对应条目：** `test_public_preset_names_in_index_json` pass
- **zsh 补全包含新条目：** `test_index_json_entries_have_zsh_completion` pass

**Verification:**
- `cargo test -p ralph-cli` 全部 pass
- `cargo build` 成功
- `ralph preset list` 输出包含 `ce-executor-isolated`

---

### U3. loop_invariant_assertion（简化版）

**Goal:** 从 brainstorm U1 移植 invariant assertion 机制，作为 stall recovery / hard_gate 注入路径的后备防护

**Dependencies:** 无（与 U1/U2 独立，可以并行或提前实施）

**Files:**
- `crates/ralph-core/src/config/ralph_config.rs` — 新增 `strict_invariance: bool` 和 `invariant_rules` 配置
- `crates/ralph-core/src/config/core.rs` — 新增 `LoopInvariantConfig` 结构体
- `crates/ralph-core/src/config/mod.rs` — 导出新类型
- `crates/ralph-core/src/event_loop/mod.rs` — 在 `run_iteration` 末尾添加 invariant 校验
- `crates/ralph-core/src/event_loop/loop_state.rs` — 新增 `invariant_violation_count: u32`
- `crates/ralph-core/src/diagnostics/` — 新增 `invariant_violation.rs` 模块（invariant-violation.jsonl 写入器）
- `crates/ralph-tui/src/widgets/header.rs` — 新增 ⚠️ 计数行
- `crates/ralph-tui/src/state.rs` — 新增 `invariant_violation_count` 字段传导

**Approach:**

**范围裁剪**（与 brainstorm U1 的差异）：
- ✅ INV-1：ralph 发非控制 topic（保留，defense-in-depth）
- ✅ INV-2：fallback 注入未带 `_source`（保留，关键防护）
- ✅ INV-3：`_source` 值非法（保留）
- ❌ INV-4：fallback_inject 被当作业务事件消费（裁剪——isolated mode 已防止）
- ❌ `strict_mode` 硬停 loop（裁剪——keep it simple）
- ❌ 按规则开关（裁剪——全开即可）

**校验时机**：`EventLoop::run_iteration` 末尾，在 `process_events` 之后、`save_state` 之前

**校验逻辑**：
```rust
// 伪码
for event in iteration_new_events {
    if event.source.is_none() && event.hat.is_some() {
        // INV-2: fallback_inject_missing_source
        record_violation("INV-2", &event);
    }
    if let Some(source) = &event.source {
        if !["agent", "fallback_inject", "system"].contains(&source.as_str()) {
            // INV-3: unknown_source_value
            record_violation("INV-3", &event);
        }
    }
    if event.hat == "ralph" && !RALPH_CONTROL_TOPICS.contains(&event.topic) {
        // INV-1: ralph_impersonation_business_topic
        record_violation("INV-1", &event);
    }
}
```

**写入**：invariant-violation.jsonl 格式：
```jsonl
{"ts":"...","iter":6,"invariant":"INV-1","hat":"ralph","topic":"work.done","source":"invariant_violation"}
```

**TUI 集成**：在 header.rs 的 `hat_with_backend` 行下方，仅 `invariant_violation_count > 0` 时显示：
```
⚠️ impersonation 3  (latest: INV-1 @ iter=6)
```

**Config schema**（添加到现有 `ralph_config` 段即可，不需要新建配置文件）：
```yaml
ralph_config:
  strict_invariance: false  # 默认 false。true 时硬停 loop
```

**Test scenarios:**

- **INV-1 正常触发：** 构造 events.jsonl 含 `hat=ralph, topic=work.done`，assert 落 `invariant-violation.jsonl` 且 `state.invariant_violation_count = 1`
- **INV-2 正常触发：** 构造 events.jsonl 行无 `_source` 但有 `hat`，assert INV-2 记录
- **INV-3 正常触发：** 构造 events.jsonl 行 `_source=invalid_value`，assert INV-3 记录
- **正常 iter 不误报：** 正常 10 hat 编排不产生 invariant-violation.jsonl
- **TUI 头部仅违规时显示：** `invariant_violation_count = 0` 时不显示 ⚠️；=1 时显示
- **strict_mode 硬停 loop：** `strict_invariance = true` 时构造 INV-1，assert loop 退出码为变异退出

**Verification:**
- `cargo test -p ralph-core` 中新增测试全部 pass
- 既有 `cargo test --workspace --exclude ralph-e2e` 全部 pass（无回归）

---

### U4. 冒烟验证

**Goal:** 验证 isolated preset 可完整跑通 ce-executor 10-hat 事件链

**Dependencies:** U1, U2

**Files:**
- 无需新建测试文件。使用既有测试基础设施

**Approach:**
1. Preset 解析测试：`cargo test -p ralph-cli`（现有框架覆盖 YAML 解析 + schema 校验）
2. isolated mode 事件 scope 测试：`cargo test -p ralph-core`（`test_isolated_mode_accepts_only_first_business_event` 等既有测试）
3. BDD scenario 测试：在 presets.rs 中加一个测试，加载 ce-executor-isolated preset 并验证其 hat 拓扑与 ce-executor 一致

**Test scenarios:**
- **Preset 加载验证：** 加载 `ce-executor-isolated` preset，assert `hats` 数量、triggers、publishes 与 `ce-executor` 一致
- **Isolated scope 验证：** 启用 preset 后发越权事件（如 hat A 发 hat B 的 topic），assert 被 `can_publish` 拒绝

**Verification:**
- `cargo test --workspace --exclude ralph-e2e` 全部 pass
- 用户手动跑一次 `ralph run -H builtin:ce-executor-isolated -p "simple task" --max-iterations 3` 验证 loop 能正常启动和退出

---

## 关键技术决策

### KTD-1：不实现 U2-U5

**决定：** 仅移植 brainstorm 的 U1。U2（BDD scenarios）在 isolated mode 下无必要——scope check 由 `can_publish` 硬保障而非白名单。U3（`_source` 字段）、U4（events.jsonl 拆分）、U5（hat/role 拆分）均被 isolated mode 的架构性防护替代。

**理由：** isolated mode 的 `can_publish` + 单业务事件边界 提供了比所有 U2-U5 加起来更强的防护。多余的防御层增加维护成本而不增加安全性。

### KTD-2：不与既有 ce-executor 合并

**决定：** 以独立 preset 存在，用户通过 `-H builtin:ce-executor-isolated` 选择

**理由：** coordinator mode 在某些场景仍有优势（快速原型、单 hat 任务），两个 preset 满足不同用例。用户可根据需要选择。

### KTD-3：INV-1 保留但重要性降低

**决定：** INV-1（`ralph` 发非控制 topic）仍实现，但在 isolated mode 下不应触发——isolated mode 的 `next_hat()` 从来不返回 `ralph`（除非是 solo mode），所以 ralph 不会有机会发业务事件

**理由：** defense-in-depth。如果未来的改动导致 isolated mode 下 `next_hat()` 返回了 `ralph`+业务事件，INV-1 会立即告警。

---

## 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| Isolated mode 下每个 hat 起独立 backend，冷启动增加延迟 | 1-3s/iter 延迟 | 这是 isolated mode 的固有开销，用户预期之内 |
| Instructions 隐式依赖共享上下文 | 某个 hat 收到聚焦 prompt 后行为异常 | U1 中已 review 全部 instructions，确认无此依赖 |
| Wave 执行与 isolated mode 的兼容性 | wave review 可能不工作 | 确认 wave dispatcher 在 loop_runner 层，与 execution_mode 解耦 |
| 现有 `ce-executor` 回归 | 改动影响既有行为 | 新建 preset 不碰既有文件，不存在回归风险 |

---

## 依赖与顺序

```
U3 (loop_invariant_assertion)
  └── 独立，可先做
U1 (preset YAML)
  └── U2 (注册) — 串行依赖
U4 (冒烟验证)
  └── U1 + U2
```

U3 与 U1/U2 无依赖关系，可并行实施。

---

## 开放问题

无。技术方案已明确，所有决策已在 KTD 中记录。
