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

3. **topic-level schema metadata**（2026-07-09 起，U1-U9 plan 001 / plan 003 落地）
   - `field_docs.<field>: { meaning, source, fill_rule }` —— author 对字段含义、值源、填法的可读注解；不参与 runtime accept/reject，只在 agent prompt 与 `--policy-check` 拒收时回显。
   - `examples` —— topic 级示例 payload，供给 prompt builder 用作 schema-aware publish section。
   - `known_fields` —— schema 已知存在但非必填的 pass-through 字段，供 `trigger_context` 引用。
   - `trigger_context: { summary_fields, routing_hints }` —— 在下游 hat prompt 顶部注入 `## TRIGGER CONTEXT` 区块（source topic/source hat/summary fields/matched hints）；不为 runtime 控制命令、不改 routing/权限、不替 `--policy-check`。
   详细字段形状、写入流程、与 `--policy-check` 的边界见
   [Payload Contracts → Schema Metadata / Policy-Check 反馈 / Trigger Context](../guide/payload-contracts.md)。

> **去双写硬规则**:hat `instructions` 不复述 schema 提示的字段含义，也不复述 `trigger_context.routing_hints` 的判定条件；要写就只引用 `## TRIGGER CONTEXT` 区块 / `--policy-check` 的 `field_docs`/`examples`。Lint 会检查 emitter hat 的 instructions 是否引用了 `ralph-tools-emit` 与新章节（详见 `skills/ralph-preset-common/references/finding-rubric.md`）。

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

### 增改 schema metadata（`field_docs` / `examples` / `known_fields` / `trigger_context`）

> 来自 plan 2026-07-09-001（policy-check 反馈）与 plan 2026-07-09-003（trigger context）。改动 schema metadata **不改变 runtime accept/reject 语义**，但 strict preset lint 会卡住字段引用、谓词集合与 topology 可见性。

1. 在 `presets/schemas/ce-executor-pipeline.yml` 的 topic 键下补 `field_docs` / `examples` / `known_fields` / `trigger_context`。详见 [Payload Contracts → Schema Metadata / Policy-Check 反馈 / Trigger Context](../guide/payload-contracts.md)。
2. **保持 inline / sibling 一致**：如 `presets/en/ce-executor-pipeline*.yml` 仍有 inline `event_policy.schemas.<topic>` 块，必须在同一份 inline 复制同一份 metadata；schema parity lint 会拒收 drift。
3. 增 trigger context 时确认 source topic 的下游 hats 在 `triggers` / `subscribes_to` 覆盖该 topic；否则 lint 会以 `trigger_context_no_consumer` Error 拒收。
4. hat `instructions` 不复述 metadata。Emitted-event hat 的 `instructions` 必须引用 `ralph-tools-emit` 的「Policy-Check 反馈」与「## TRIGGER CONTEXT」章节；不要复制字段表或 hint 判定条件。
5. 修改后跑：
    ```bash
    ralph preset check -H builtin:ce-executor-pipeline[-loop] --strict
    cargo nextest run -p ralph-core -- preset_lint
    cargo nextest run -p ralph-cli --bin ralph -- test_ce_executor_root_preset_matches_embedded
    ```
    全绿后才算 schema metadata 闭环。

---

## 检查清单

- `build.rs` 仍然负责把顶层 topic 键 merge 进 embedded preset
- `ProtocolView` 仍然是 read-only 派生视图
- BDD scenario 覆盖新增字段和新增 topic
- 没有把协议表硬编码回 Rust
- 输入计划按语义规范化为单一 artifact；下游消费稳定 R/S/U、digest 与 trace，不重新解释源 Markdown 标题
- executor 与 fixer 的生产 HEAD 都经过 test-stabilizer；稳定化产生的 production correction 也进入独立 review
- 线性流水线显式传递 `review_phase`：`initial` 可进入一次 fix plan，`post_fix` 只能 accepted 或 blocked
- 真实 EventLoop BDD 覆盖 executor 后稳定化、fixer 后稳定化、post-fix 接受和 post-fix 阻塞

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
