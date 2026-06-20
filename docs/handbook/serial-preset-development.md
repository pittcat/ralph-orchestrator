# Serial Preset 开发手册

> **目标读者**: 维护 `ce-executor-serial` preset 的工程师和 preset 作者。
> **适用范围**: `presets/schemas/ce-executor-serial.yml` + 其在 builtin 嵌入 + 由 `ralph emit --schema` 暴露的协议视图。
> **协议 SSOT**: [`presets/schemas/ce-executor-serial.yml`](../../presets/schemas/ce-executor-serial.yml) — 编辑这一个文件即可。

---

## 什么是 serial preset 的协议 SSOT

`ce-executor-serial` 的运行时协议(每个 topic 的 `required_fields` / `verdict_gate` / `workflow_contract` / `state_projection` / `hat_handoff` / `execution_contracts`)不再散落在三个地方:

| 旧位置 | 新位置 |
|---|---|
| `presets/en/ce-executor-serial.yml` 内的 inline block | `presets/schemas/ce-executor-serial.yml` 顶层 topic 键 + 协议节 |
| Rust 静态 RULES 表(如旧的 `preset/serial/contracts.rs`,已废弃) | `crates/ralph-core/src/preset/engine/protocol.rs::ProtocolView` 派生 |
| agent instructions 内手写 schema/gate 段落 | engine 生成的 `{generated_from: protocol}` 块 |

`build.rs` 在编译时把 `presets/schemas/ce-executor-serial.yml` deep-merge 进 embedded preset,所有 runtime 路径(CLI emit precheck、loop gate、drift engine、prompt builder、linter、lint_mirror)统一从 `ProtocolView` 读值。**禁止在 Rust 维护 duplicate 字段表**——plan 2026-06-20-001 U1 + KTD-10 明确要求。

---

## 协议结构(SSOT 文件的 schema)

`presets/schemas/ce-executor-serial.yml` 顶层包含两类内容:

1. **顶层 topic 键** → `event_policy.schemas.<topic>.required_fields`
   - 例: `work.done:` / `review.passed:` / `queue.advance:`
   - 每个 topic 声明 `required_fields`(list of strings)+ 可选 `payload: json_object`
   - 这是 **payload 字段 SSOT**,运行时 gate 与 linter 都从这里读

2. **协议节**(`event_loop.*` 的镜像,供 build.rs merge)
   - `event_loop.execution_mode`: `coordinator` / `isolated`(4+ hats 必须 isolated)
   - `event_loop.event_policy`: 已存在的 `event_policy.schemas` 段;`execution_mode` 和 `mode` 在此层
   - `event_loop.verdict_gate`: review-time fail field 配置
   - `event_loop.workflow_contract.step_handoff.progress_task_gate`: step handoff flags
   - `event_loop.state_projection.actions.<topic>`: 投影动作(`close_task` / `mark_step_completed` / …),顺序即语义
   - `event_loop.hat_handoff`: artifact 规则 / linter 配置 / macro & exempt topics

完整的 Merge 映射表见 [`docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md`](../plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md) 的 §"Merge 映射表"。

---

## 改一条规则的标准流程

### 场景 A:改某个 topic 的 required field

例: 给 `work.done` 加一个 `commit_sha` 字段。

1. 编辑 `presets/schemas/ce-executor-serial.yml`,在 `work.done:` 下加字段:
   ```yaml
   work.done:
     required_fields:
       - plan_name
       - plan_path
       - task_id
       - task_key
       - step
       - commit_count
       - changed_lines
       - commit_sha     # ← 新增
     payload: json_object
   ```

2. 删除 `presets/en/ce-executor-serial.yml` 内对应的 inline `work.done.required_fields` 段(若仍在 inline 中)。

3. `cargo build` —— `build.rs` 重新生成 embedded preset。

4. 验证:
   ```bash
   ralph emit --schema work.done
   ```
   输出 JSON 中 `required_fields` 必须包含 `commit_sha`,`protocol_hash` 必须变化。

5. 跑相关测试:
   ```bash
   cargo nextest run -p ralph-cli --bin ralph -- 'test_emit_schema'
   cargo nextest run -p ralph-core --test scenarios serial_lint
   ```

### 场景 B:改 hat_handoff 的 macro 列表

例: 把 `queue.advance` 加入 macro edge(强制要求 handoff_path)。

1. 编辑 `presets/schemas/ce-executor-serial.yml`:
   ```yaml
   event_loop:
     hat_handoff:
       macro_topics:
         - work.done
         - review.passed
         - queue.advance    # ← 新增
   ```

2. `cargo build`。

3. 验证:
   ```bash
   ralph emit --schema queue.advance
   ```
   `is_macro_edge` 必须是 `true`,`hat_handoff.macro_topics` 必须包含新条目。

### 场景 C:加新 topic(扩展协议)

1. 在 `presets/schemas/ce-executor-serial.yml` 加顶层 topic 键 + `event_policy.schemas` 段(若需要 execution contract 增量约束,加 `execution_contracts.rules.<topic>`)。

2. 如果新 topic 是 macro edge,加入 `event_loop.hat_handoff.macro_topics` + 写到 `event_loop.workflow_contract.handoff_topic_seeds`。

3. `cargo build`。

4. 验证:
   ```bash
   ralph emit --schema <new.topic>
   ```

5. 在 [`crates/ralph-core/tests/scenarios/serial_lint/`](../../crates/ralph-core/tests/scenarios/) 至少加 1 个 scenario(plan 2026-06-20-002 BDD harness 扩展跟踪)。

---

## 验证协议视图 — `ralph emit --schema <TOPIC>`

U5 / R6 引入的只读子命令,plan 2026-06-20-001 §"U5. `--schema` + handbook + 文档"。**核心契约**:

- `--schema <TOPIC>` **不写 events.jsonl**,**不消耗 iteration**,**不触发 lint**
- 输出 JSON 视图 + `protocol_hash`
- `protocol_hash` 是 `ProtocolView::from_event_loop` 计算的 stable hash,**跨 render 一致**,但**build-time 嵌入变化时会变**

### 使用示例

```bash
# 查 work.done 协议视图
ralph emit --schema work.done
```

输出形如:

```json
{
  "topic": "work.done",
  "protocol_hash": "a1b2c3d4e5f6...",
  "is_macro_edge": true,
  "required_fields": ["plan_name","plan_path","task_id","task_key","step","commit_count","changed_lines"],
  "all_topics": { ... },
  "verdict_gate": { ... },
  "workflow_contract": { ... },
  "state_projection": { ... },
  "execution_contracts": { ... },
  "hat_handoff": { ... }
}
```

### 检测 drift(嵌入 vs. authoring)

最简方式:对比 build 前后 `protocol_hash`:

```bash
cargo build
ralph emit --schema work.done | jq -r .protocol_hash   # 改前

# 编辑 presets/schemas/ce-executor-serial.yml
cargo build
ralph emit --schema work.done | jq -r .protocol_hash   # 改后,值必变
```

如果 hash 没变,说明改动没生效——通常是:

- inline `presets/en/ce-executor-serial.yml` 还在覆盖(per-key override 层未清理)
- 改错了文件(`presets/en/` 而非 `presets/schemas/`)
- `build.rs` merge 路径异常(看 build 日志)

### Schema 模式约束

- 必须有可发现的 `ralph.yml`(否则 fail-closed 报错"no ralph.yml found")
- 与 `--policy-check` / `--json` / `payload` 互斥(`clap` 自动拒绝)
- 走 `ProtocolView::from_event_loop(&cfg.event_loop)`,所以输出的字段是 **embedded 协议** 而非 raw authoring YAML 的字面值——merge 后的真值

---

## 常见误区

### 1. inline block 还在覆盖

`presets/en/ce-executor-serial.yml` 在过渡期保留为 per-key override 层。如果 inline 里有 `event_policy.schemas.work.done.required_fields`,它会 **覆盖** SSOT。改 SSOT 没效果时,**先检查 inline**。

过渡期结束(每个 topic 都迁出 inline)后,inline 这层消失,SSOT 是唯一真值。

### 2. 改 Rust 而非改 YAML

`ProtocolView` 是 **read-only** 派生视图;改 `effective_required_fields` 的算法等于绕过 SSOT。**禁止在 Rust 维护 hardcoded field table**。如需新约束,加到 `presets/schemas/ce-executor-serial.yml`。

### 3. 改 `event_loop.event_policy.schemas` 而不是顶层 topic 键

两种写法功能上等价(build.rs merge 后是同一份),但本项目约定:
- **顶层 topic 键** 是 authoring 端的 SSOT(`presets/schemas/ce-executor-serial.yml`)
- **`event_loop.event_policy.schemas`** 是 embedded 端的真值,build.rs 把顶层键 merge 进来
- **禁止** 在 `presets/en/ce-executor-serial.yml` 内重复 `event_policy.schemas` 段(那是 inline 层的覆盖,过渡期已逐步清空)

### 4. 忘记 `cargo build`

`ProtocolView` 读 embedded 协议,**不读 `presets/schemas/*.yml` 直读**。改了 SSOT 不 build 等于没改。

---

## 相关文件清单

| 路径 | 角色 |
|---|---|
| `presets/schemas/ce-executor-serial.yml` | **协议 SSOT(编辑这一个)** |
| `presets/en/ce-executor-serial.yml` | inline preset(含 builtin embedded 路径,过渡期有 inline 覆盖层) |
| `crates/ralph-core/src/preset/engine/protocol.rs` | `ProtocolView` 类型定义 + `from_event_loop` 派生 |
| `crates/ralph-core/src/preset/engine/gates.rs` | `run_gates`,读 `ProtocolView::required_fields(topic)` |
| `crates/ralph-core/src/preset/engine/lint_emit.rs` | linter,同样读 `ProtocolView` |
| `crates/ralph-cli/src/commands/emit.rs::schema_view` | `ralph emit --schema` 的渲染实现 |
| `crates/ralph-cli/build.rs` | merge `presets/schemas/*.yml` 进 embedded preset |
| `docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md` | 协议 SSOT 重构的完整 plan |
| `docs/plans/2026-06-20-002-feat-bdd-harness-extension-for-runtime-state-inspection-plan.md` | BDD harness 扩展(独立 plan,跟踪 in-loop hint 路径) |

---

## 进一步阅读

- [`docs/plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md`](../plans/2026-06-20-001-feat-serial-preset-precheck-as-linter-plan.md) — U1/U3a/U3b/U2/U4/U4b/U7 已 ship,U5(本文档)+ U6(BDD)跟进
- [`.cursor/rules/multi-hat-isolation.mdc`](../../.cursor/rules/multi-hat-isolation.mdc) — 4+ hats 必须 isolated 的强制规则
- [`.cursor/rules/architecture-modules.mdc`](../../.cursor/rules/architecture-modules.mdc) — 模块路径速查