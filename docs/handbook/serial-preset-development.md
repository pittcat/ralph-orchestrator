# 单链 preset 开发手册

> **目标读者**: 维护 `ce-executor-pipeline` preset 和其协议 SSOT 的工程师。
> **适用范围**: `presets/schemas/ce-executor-pipeline.yml` + 其在 builtin 嵌入 + 由 `ralph emit --schema` 暴露的协议视图。
> **协议 SSOT**: [`presets/schemas/ce-executor-pipeline.yml`](../../presets/schemas/ce-executor-pipeline.yml)。

---

## 协议职责

`ce-executor-pipeline` 的运行时协议不应散落在多个位置。`required_fields`、`verdict_gate`、`workflow_contract`、`state_projection` 与 `execution_contracts` 统一从协议视图读取，避免在 Rust 里维护重复字段表。

| 旧位置 | 现位置 |
|---|---|
| preset 内联块 | `presets/schemas/ce-executor-pipeline.yml` 顶层 topic 键 + 协议节 |
| 手写静态表 | `crates/ralph-core/src/preset/engine/protocol.rs::ProtocolView` 派生 |
| agent prompt 里重复的 schema/gate 段落 | engine 生成的 `{generated_from: protocol}` 块 |

`build.rs` 会在编译时把 `presets/schemas/ce-executor-pipeline.yml` deep-merge 进 embedded preset。运行时路径统一从 `ProtocolView` 读值。

---

## 协议结构

`presets/schemas/ce-executor-pipeline.yml` 顶层包含两类内容:

1. **顶层 topic 键** → `event_policy.schemas.<topic>.required_fields`
   - 例: `work.done:` / `work.failed:`
   - 每个 topic 声明 `required_fields` 和可选 `payload`
   - 这是 payload 字段的协议来源

2. **协议节**(`event_loop.*` 的镜像,供 build.rs merge)
   - `event_loop.execution_mode`: 当前主线使用 `isolated`
   - `event_loop.event_policy`: 已存在的 `event_policy.schemas` 段
   - `event_loop.verdict_gate`: review-time 约束
   - `event_loop.workflow_contract.step_handoff.progress_task_gate`: step handoff 约束
   - `event_loop.state_projection.actions.<topic>`: 投影动作

---

## 修改步骤

### 新增或调整 `work.done.required_fields`

1. 编辑 `presets/schemas/ce-executor-pipeline.yml`。
2. 删除 preset 内联块里重复的同名字段，避免双写。
3. 运行：

```bash
cargo build
ralph emit --schema work.done
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
```

### 新增 gated topic

1. 在 `presets/schemas/ce-executor-pipeline.yml` 加顶层 topic 键和 `event_policy.schemas` 段。
2. 不要手写 Rust 常量表，继续从 `ProtocolView` 派生。
3. 补一条真实 BDD scenario，确保 runtime 路径被覆盖。

### 修改投影动作

1. 改 `event_loop.state_projection.actions.<topic>` 的动作顺序。
2. 用 `cargo nextest run -p ralph-core --test scenarios -- ce_executor_pipeline` 验证投影路径。
3. 如影响 CLI emit / task gate，同步更新相关注释和测试说明。

---

## 检查清单

- `build.rs` 仍然负责把顶层 topic 键 merge 进 embedded preset
- `ProtocolView` 仍然是 read-only 派生视图
- BDD scenario 覆盖新增字段和新增 topic
- 没有把协议表硬编码回 Rust

修改后，至少跑：

```bash
cargo nextest run -p ralph-cli --bin ralph -- preset_lint
cargo nextest run -p ralph-core -- preset_lint
cargo nextest run -p ralph-core --test scenarios
```

---

## 常见误区

### 1. 只改了一层

如果 inline preset 仍然覆盖同名字段，协议视图不会变化。改动后先核对 embedded 结果。

### 2. 在 Rust 里补硬编码表

协议视图是 read-only 派生结果。新约束应进入 YAML 协议 SSOT，而不是新增一份 Rust 表。

### 3. 改错层

本项目约定：

- 顶层 topic 键是 authoring 端 SSOT
- `event_loop.event_policy.schemas` 是 embedded 端真值
- 不要在 preset 内重复维护相同字段

### 4. 忘记重建

改了 SSOT 后，必须重新 build，否则嵌入物不会更新。

---

## 相关文件

| 路径 | 角色 |
|---|---|
| `presets/schemas/ce-executor-pipeline.yml` | 协议 SSOT |
| `crates/ralph-core/src/preset/engine/protocol.rs` | `ProtocolView` 定义 |
| `crates/ralph-core/src/preset/engine/gates.rs` | gate 读取协议视图 |
| `crates/ralph-core/src/preset/engine/lint_emit.rs` | linter 读取协议视图 |
| `crates/ralph-cli/src/commands/emit.rs::schema_view` | `ralph emit --schema` 渲染 |
| `crates/ralph-cli/build.rs` | merge 协议 SSOT 进 embedded preset |
| `docs/brainstorms/2026-07-02-ce-executor-pipeline-preset-requirements.md` | pipeline 协议需求背景 |
| `docs/plans/2026-07-07-006-refactor-ralph-single-chain-execution-primary-plan.md` | 当前主线迁移 plan |
